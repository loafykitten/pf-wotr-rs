//! The compiled content pack: every decoded blueprint, memory-mapped, with indices by GUID
//! and by type.
//!
//! ```text
//! header   (128 bytes)  magic, version, schema fingerprint, source zip identity,
//!                       counts, section offsets
//! index    count × 40   sorted by GUID: guid[16] type u32 payload_off u64
//!                       payload_len u32 path_off u32 path_len u32 (all little-endian)
//! by type  types × 8    (first, count) into the `by type` entries
//!          count × 4    entry numbers grouped by concrete type, GUID order within a type
//! paths    UTF-8        the zip path of each blueprint
//! payload  …            one rkyv archive of a [`PNode`] per blueprint, 16-byte aligned
//! ```
//!
//! Opening validates the header and section bounds only, so the pack is ready in well
//! under a millisecond; each payload is validated by rkyv's checker when it's accessed.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use memmap2::Mmap;
use rkyv::rancor;

use crate::decode::{self, Tally};
use crate::guid::Guid;
use crate::node::{Node, Object};
use crate::schema::{self, TypeIdx};
use crate::source::BlueprintZip;

const MAGIC: &[u8; 8] = b"WRCPACK1";
/// Bump on any change to the layout below or to [`PNode`] (rkyv can validate bytes, not
/// what a variant number means).
const VERSION: u32 = 2;
const HEADER: usize = 128;
const ENTRY: usize = 40;
const ALIGN: usize = 16;

/// A decoded value in pack form: like [`Node`], with JSON kept as text.
#[derive(Debug, Clone, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(serialize_bounds(
    __S: rkyv::ser::Writer + rkyv::ser::Allocator,
    __S::Error: rkyv::rancor::Source,
))]
#[rkyv(deserialize_bounds(__D::Error: rkyv::rancor::Source))]
#[rkyv(bytecheck(bounds(
    __C: rkyv::validation::ArchiveContext,
    __C::Error: rkyv::rancor::Source,
)))]
pub enum PNode {
    Absent,
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    F32(f32),
    F64(f64),
    Str(String),
    Enum(i64),
    BpRef([u8; 16]),
    WeakLink(Option<String>),
    WeakLinkLegacy(String, Option<i64>),
    /// Key and asset GUID.
    SharedString(Option<(String, Option<String>)>),
    Asset(Option<(String, i64)>),
    AssetIndex(i32),
    Floats(Vec<f32>),
    Ints(Vec<i32>),
    /// Keys as `[time, value, inTangent, outTangent, inWeight, outWeight]` + modes.
    Curve {
        pre: i32,
        post: i32,
        keys: Vec<[f32; 6]>,
        modes: Vec<i32>,
    },
    Gradient {
        colors: Vec<[f32; 4]>,
        alphas: Vec<[f32; 2]>,
        mode: u8,
    },
    Seq(#[rkyv(omit_bounds)] Vec<PNode>),
    Object {
        ty: u32,
        tagged: bool,
        #[rkyv(omit_bounds)]
        fields: Vec<PNode>,
        /// Properties the schema doesn't have: name and JSON text.
        extra: Vec<(String, String)>,
    },
    Discarded(String),
    Coerced(#[rkyv(omit_bounds)] Box<PNode>, String),
    Opaque {
        json: String,
        reason: String,
    },
}

impl PNode {
    /// Drops every object's unknown properties.
    pub fn strip_extra(&mut self) {
        match self {
            Self::Seq(v) => v.iter_mut().for_each(Self::strip_extra),
            Self::Coerced(n, _) => n.strip_extra(),
            Self::Object { fields, extra, .. } => {
                extra.clear();
                fields.iter_mut().for_each(Self::strip_extra);
            }
            _ => {}
        }
    }

    #[must_use]
    pub fn from_node(n: &Node) -> Self {
        match n {
            Node::Absent => Self::Absent,
            Node::Null => Self::Null,
            Node::Bool(b) => Self::Bool(*b),
            Node::Int(i) => Self::Int(*i),
            Node::UInt(u) => Self::UInt(*u),
            Node::F32(f) => Self::F32(*f),
            Node::F64(f) => Self::F64(*f),
            Node::Str(s) => Self::Str(s.to_string()),
            Node::Enum(e) => Self::Enum(*e),
            Node::BpRef(g) => Self::BpRef(g.to_bytes()),
            Node::WeakLink(s) => Self::WeakLink(s.as_deref().map(str::to_owned)),
            Node::WeakLinkLegacy(g, f) => Self::WeakLinkLegacy(g.to_string(), *f),
            Node::SharedString(s) => Self::SharedString(
                s.as_ref().map(|r| (r.key.to_string(), r.asset.as_deref().map(str::to_owned))),
            ),
            Node::Asset(a) => Self::Asset(a.as_ref().map(|a| (a.guid.to_string(), a.file_id))),
            Node::AssetIndex(i) => Self::AssetIndex(*i),
            Node::Floats(f) => Self::Floats(f.to_vec()),
            Node::Ints(v) => Self::Ints(v.to_vec()),
            Node::Curve(c) => Self::Curve {
                pre: c.pre_wrap,
                post: c.post_wrap,
                keys: c
                    .keys
                    .iter()
                    .map(|k| {
                        [k.time, k.value, k.in_tangent, k.out_tangent, k.in_weight, k.out_weight]
                    })
                    .collect(),
                modes: c.keys.iter().map(|k| k.weighted_mode).collect(),
            },
            Node::Gradient(g) => {
                Self::Gradient { colors: g.colors.clone(), alphas: g.alphas.clone(), mode: g.mode }
            }
            Node::Seq(v) => Self::Seq(v.iter().map(Self::from_node).collect()),
            Node::Object(o) => Self::Object {
                ty: o.ty,
                tagged: o.tagged,
                fields: o.fields.iter().map(Self::from_node).collect(),
                extra: o.extra.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            },
            Node::Discarded(v) => Self::Discarded(v.to_string()),
            Node::Coerced(n, v) => Self::Coerced(Box::new(Self::from_node(n)), v.to_string()),
            Node::Opaque(o) => {
                Self::Opaque { json: o.value.to_string(), reason: format!("{:?}", o.reason) }
            }
        }
    }
}

/// Build statistics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BuildStats {
    pub blueprints: usize,
    pub bytes: u64,
    pub payload_bytes: u64,
    pub decode_and_encode_seconds: f64,
    pub write_seconds: f64,
    pub total_seconds: f64,
}

fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_u64(b: &mut [u8], at: usize, v: u64) {
    b[at..at + 8].copy_from_slice(&v.to_le_bytes());
}

fn get_u32(b: &[u8], at: usize) -> u32 {
    let mut a = [0u8; 4];
    a.copy_from_slice(&b[at..at + 4]);
    u32::from_le_bytes(a)
}

fn get_u64(b: &[u8], at: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(a)
}

/// Identity of the source zip that a pack was built from: size and modification time.
fn zip_identity(zip_path: &Path) -> Result<(u64, u64)> {
    let m = std::fs::metadata(zip_path)?;
    let mtime = m
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
    Ok((m.len(), mtime))
}

struct Built {
    guid: Guid,
    ty: TypeIdx,
    path: String,
    payload: rkyv::util::AlignedVec,
}

/// What goes into the pack.
#[derive(Debug, Clone, Copy, Default)]
pub struct BuildOptions {
    /// Keep JSON properties the runtime schema doesn't have (editor-only and stale
    /// fields; 1.28 M of them, mostly `LocalizedString` editor paths). Off by default:
    /// the runtime never reads them, and the reports count them.
    pub keep_extra: bool,
}

/// Builds the pack from the zip into `out` (written to a temporary file, then renamed,
/// so an open pack is never modified in place).
///
/// # Errors
///
/// I/O errors, or a blueprint that isn't valid JSON or has no `AssetId`.
pub fn build(zip_path: &Path, out: &Path, opts: BuildOptions) -> Result<BuildStats> {
    let t0 = std::time::Instant::now();
    let zip = BlueprintZip::open(zip_path)?;
    let (zlen, zmtime) = zip_identity(zip_path)?;
    let parts: Vec<Result<Vec<Built>>> = zip.par_fold(
        || Ok(Vec::new()),
        |acc: &mut Result<Vec<Built>>, _pos, path, bytes| {
            let Ok(list) = acc else { return };
            let r = (|| -> Result<Built> {
                let bytes = bytes?;
                let mut tally = Tally::default();
                let d = decode::decode_jbp(&bytes, &mut tally)?;
                let guid = d.guid.with_context(|| format!("{path}: no AssetId"))?;
                let data = decode::data(&d.wrapper).with_context(|| format!("{path}: no Data"))?;
                let mut p = PNode::from_node(&Node::Object(Box::new(data.clone())));
                if !opts.keep_extra {
                    p.strip_extra();
                }
                let payload = rkyv::to_bytes::<rancor::Error>(&p)?;
                Ok(Built { guid, ty: data.ty, path: path.to_owned(), payload })
            })();
            match r {
                Ok(b) => list.push(b),
                Err(e) => *acc = Err(e),
            }
        },
    );
    let mut all = Vec::with_capacity(zip.len());
    for p in parts {
        all.extend(p?);
    }
    all.sort_by_key(|b| b.guid);
    for w in all.windows(2) {
        ensure!(w[0].guid != w[1].guid, "duplicate GUID {}", w[0].guid);
    }
    let t_enc = t0.elapsed().as_secs_f64();
    let t1 = std::time::Instant::now();

    let n = all.len();
    let types = schema::types().len();
    let index_off = HEADER;
    let by_type_off = index_off + n * ENTRY;
    let by_type_entries_off = by_type_off + types * 8;
    let paths_off = by_type_entries_off + n * 4;
    let paths_len: usize = all.iter().map(|b| b.path.len()).sum();
    let payload_off = (paths_off + paths_len).next_multiple_of(ALIGN);

    let mut index = vec![0u8; n * ENTRY];
    let mut payload_cursor = 0usize;
    let mut path_cursor = 0usize;
    for (i, b) in all.iter().enumerate() {
        let e = &mut index[i * ENTRY..(i + 1) * ENTRY];
        e[0..16].copy_from_slice(&b.guid.to_bytes());
        put_u32(e, 16, b.ty);
        put_u64(e, 20, payload_cursor as u64);
        put_u32(e, 28, u32::try_from(b.payload.len())?);
        put_u32(e, 32, u32::try_from(path_cursor)?);
        put_u32(e, 36, u32::try_from(b.path.len())?);
        payload_cursor = (payload_cursor + b.payload.len()).next_multiple_of(ALIGN);
        path_cursor += b.path.len();
    }
    // By type: entry numbers grouped by type (stable, so GUID order within a type).
    let mut order: Vec<u32> = (0..u32::try_from(n)?).collect();
    order.sort_by_key(|&i| all[i as usize].ty);
    let mut ranges = vec![0u8; types * 8];
    let mut i = 0;
    while i < order.len() {
        let t = all[order[i] as usize].ty as usize;
        let start = i;
        while i < order.len() && all[order[i] as usize].ty as usize == t {
            i += 1;
        }
        put_u32(&mut ranges, t * 8, u32::try_from(start)?);
        put_u32(&mut ranges, t * 8 + 4, u32::try_from(i - start)?);
    }
    let total = payload_off + payload_cursor;
    let mut header = vec![0u8; HEADER];
    header[0..8].copy_from_slice(MAGIC);
    put_u32(&mut header, 8, VERSION);
    put_u64(&mut header, 16, schema::fingerprint());
    put_u64(&mut header, 24, zlen);
    put_u64(&mut header, 32, zmtime);
    put_u32(&mut header, 40, u32::try_from(n)?);
    put_u32(&mut header, 44, u32::try_from(types)?);
    put_u64(&mut header, 48, index_off as u64);
    put_u64(&mut header, 56, by_type_off as u64);
    put_u64(&mut header, 64, by_type_entries_off as u64);
    put_u64(&mut header, 72, paths_off as u64);
    put_u64(&mut header, 80, payload_off as u64);
    put_u64(&mut header, 88, total as u64);
    put_u32(&mut header, 96, u32::try_from(std::mem::size_of::<ArchivedPNode>())?);

    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // A temporary name of our own, so concurrent builds can't rename each other's
    // half-written files into place.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let tmp = out.with_extension(format!("tmp.{}.{nanos}", std::process::id()));
    {
        let mut w = BufWriter::with_capacity(8 << 20, File::create(&tmp)?);
        w.write_all(&header)?;
        w.write_all(&index)?;
        w.write_all(&ranges)?;
        for i in &order {
            w.write_all(&i.to_le_bytes())?;
        }
        for b in &all {
            w.write_all(b.path.as_bytes())?;
        }
        let mut pos = paths_off + paths_len;
        let pad = [0u8; ALIGN];
        w.write_all(&pad[..payload_off - pos])?;
        pos = payload_off;
        for b in &all {
            w.write_all(&b.payload)?;
            pos += b.payload.len();
            let next = pos.next_multiple_of(ALIGN);
            w.write_all(&pad[..next - pos])?;
            pos = next;
        }
        debug_assert_eq!(pos, total);
        let f = w.into_inner().map_err(|e| e.into_error())?;
        f.sync_all()?;
        // Read-only on disk: packs are replaced, never edited (see `Pack::open`).
        let mut perm = f.metadata()?.permissions();
        perm.set_readonly(true);
        f.set_permissions(perm)?;
    }
    std::fs::rename(&tmp, out)?;
    Ok(BuildStats {
        blueprints: n,
        bytes: total as u64,
        payload_bytes: payload_cursor as u64,
        decode_and_encode_seconds: t_enc,
        write_seconds: t1.elapsed().as_secs_f64(),
        total_seconds: t0.elapsed().as_secs_f64(),
    })
}

/// An open pack.
#[derive(Debug)]
pub struct Pack {
    map: Mmap,
    count: usize,
    index_off: usize,
    by_type_off: usize,
    by_type_entries_off: usize,
    paths_off: usize,
    payload_off: usize,
    types: usize,
}

/// One index entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub guid: Guid,
    pub ty: TypeIdx,
    payload_off: u64,
    payload_len: u32,
    path_off: u32,
    path_len: u32,
}

impl Pack {
    /// Maps the pack and validates its header and section bounds.
    ///
    /// # Errors
    ///
    /// Wrong magic or version, a pack built with a different schema, or sections that
    /// don't fit the file.
    ///
    /// # Safety
    ///
    /// The file must not be modified or truncated while the returned `Pack`, or anything
    /// borrowed from it, is alive: the pack hands out references into the mapping, and
    /// rkyv's validation only holds for the bytes it checked. [`build`] never writes a
    /// pack in place (it writes a uniquely named file, marks it read-only and renames it
    /// over the old one, which leaves existing mappings intact), so packs it produces are
    /// safe to open as long as nothing else edits them.
    pub unsafe fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the caller guarantees the file isn't modified while mapped. Every read
        // below goes through bounds-checked slices, and payloads through rkyv's validator.
        let map = unsafe { Mmap::map(&file)? };
        Self::from_map(map)
    }

    fn from_map(map: Mmap) -> Result<Self> {
        let b: &[u8] = &map;
        ensure!(b.len() >= HEADER, "pack too small");
        ensure!(&b[0..8] == MAGIC, "not a content pack");
        ensure!(get_u32(b, 8) == VERSION, "pack version {}", get_u32(b, 8));
        if get_u64(b, 16) != schema::fingerprint() {
            bail!("pack was built with a different schema; rebuild it");
        }
        ensure!(
            get_u32(b, 96) as usize == std::mem::size_of::<ArchivedPNode>(),
            "pack node layout differs; rebuild it"
        );
        let count = get_u32(b, 40) as usize;
        let types = get_u32(b, 44) as usize;
        ensure!(types == schema::types().len(), "type count differs from the schema");
        let off = |at: usize| usize::try_from(get_u64(b, at)).unwrap_or(usize::MAX);
        let (index_off, by_type_off, by_type_entries_off, paths_off, payload_off, total) =
            (off(48), off(56), off(64), off(72), off(80), off(88));
        ensure!(total == b.len(), "pack length {} but header says {total}", b.len());
        ensure!(index_off == HEADER, "bad index offset");
        ensure!(by_type_off == index_off + count * ENTRY, "bad by-type offset");
        ensure!(by_type_entries_off == by_type_off + types * 8, "bad by-type entries offset");
        ensure!(paths_off == by_type_entries_off + count * 4, "bad paths offset");
        ensure!(payload_off >= paths_off && payload_off <= total, "bad payload offset");
        ensure!(payload_off % ALIGN == 0, "payload misaligned");
        Ok(Self {
            map,
            count,
            index_off,
            by_type_off,
            by_type_entries_off,
            paths_off,
            payload_off,
            types,
        })
    }

    /// Whether the pack was built from the zip at `zip_path` as it is now (size and
    /// modification time).
    ///
    /// # Errors
    ///
    /// If the zip can't be stat'ed.
    pub fn is_current(&self, zip_path: &Path) -> Result<bool> {
        let (len, mtime) = zip_identity(zip_path)?;
        Ok(get_u64(&self.map, 24) == len && get_u64(&self.map, 32) == mtime)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.count
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    #[must_use]
    pub fn bytes(&self) -> usize {
        self.map.len()
    }

    /// The `i`-th entry in GUID order.
    #[must_use]
    pub fn entry(&self, i: usize) -> Entry {
        let at = self.index_off + i * ENTRY;
        let e = &self.map[at..at + ENTRY];
        let mut g = [0u8; 16];
        g.copy_from_slice(&e[0..16]);
        Entry {
            guid: Guid::from_bytes(g),
            ty: get_u32(e, 16),
            payload_off: get_u64(e, 20),
            payload_len: get_u32(e, 28),
            path_off: get_u32(e, 32),
            path_len: get_u32(e, 36),
        }
    }

    fn guid_at(&self, i: usize) -> u128 {
        let at = self.index_off + i * ENTRY;
        let mut g = [0u8; 16];
        g.copy_from_slice(&self.map[at..at + 16]);
        u128::from_be_bytes(g)
    }

    /// Binary search by GUID.
    #[must_use]
    pub fn find(&self, g: Guid) -> Option<Entry> {
        let (mut lo, mut hi) = (0usize, self.count);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            match self.guid_at(mid).cmp(&g.0) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => return Some(self.entry(mid)),
            }
        }
        None
    }

    /// Entries whose concrete type is `t`, in GUID order. Ranges or entry numbers that
    /// fall outside the pack (a corrupt file) are skipped.
    pub fn of_type(&self, t: TypeIdx) -> impl Iterator<Item = Entry> + '_ {
        let (start, n) = if (t as usize) < self.types {
            let at = self.by_type_off + t as usize * 8;
            (get_u32(&self.map, at) as usize, get_u32(&self.map, at + 4) as usize)
        } else {
            (0, 0)
        };
        let end = start.saturating_add(n).min(self.count);
        (start.min(end)..end).filter_map(move |k| {
            let i = get_u32(&self.map, self.by_type_entries_off + k * 4) as usize;
            (i < self.count).then(|| self.entry(i))
        })
    }

    /// The zip path an entry came from.
    ///
    /// # Errors
    ///
    /// Out-of-bounds offsets or invalid UTF-8 (a corrupt pack).
    pub fn path(&self, e: &Entry) -> Result<&str> {
        let a = self.paths_off.checked_add(e.path_off as usize).context("path offset overflows")?;
        let b = a.checked_add(e.path_len as usize).context("path length overflows")?;
        ensure!(b <= self.payload_off, "path outside the paths section");
        Ok(std::str::from_utf8(&self.map[a..b])?)
    }

    /// The validated archived blueprint of an entry (zero-copy).
    ///
    /// # Errors
    ///
    /// Out-of-bounds payload or a payload that fails rkyv validation.
    pub fn archived(&self, e: &Entry) -> Result<&ArchivedPNode> {
        let a = usize::try_from(e.payload_off)
            .ok()
            .and_then(|o| self.payload_off.checked_add(o))
            .context("payload offset overflows")?;
        let b = a.checked_add(e.payload_len as usize).context("payload length overflows")?;
        let bytes = self.map.get(a..b).context("payload out of bounds")?;
        Ok(rkyv::access::<ArchivedPNode, rancor::Error>(bytes)?)
    }

    /// The blueprint of an entry, deserialized.
    ///
    /// # Errors
    ///
    /// As [`Pack::archived`].
    pub fn load(&self, e: &Entry) -> Result<PNode> {
        Ok(rkyv::deserialize::<PNode, rancor::Error>(self.archived(e)?)?)
    }
}

/// Copies a file without leaving it in the page cache (`F_NOCACHE` on both ends), so the
/// copy can be opened cold. Returns the fraction of the copy's pages resident afterwards
/// (from `mincore`), which should be near zero.
///
/// # Errors
///
/// I/O errors.
pub fn cold_copy(src: &Path, dst: &Path) -> Result<f64> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    let mut r = File::open(src)?;
    let mut w = File::create(dst)?;
    // SAFETY: plain fcntl calls on file descriptors we own.
    unsafe {
        libc::fcntl(r.as_raw_fd(), libc::F_NOCACHE, 1);
        libc::fcntl(w.as_raw_fd(), libc::F_NOCACHE, 1);
    }
    let mut buf = vec![0u8; 8 << 20];
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n])?;
    }
    w.sync_all()?;
    drop(w);
    resident_fraction(dst)
}

/// Fraction of a file's pages in the page cache (`mincore` on a fresh mapping).
///
/// # Errors
///
/// I/O errors.
pub fn resident_fraction(path: &Path) -> Result<f64> {
    let f = File::open(path)?;
    // SAFETY: read-only mapping of a file nobody else writes; we only pass its address
    // to mincore.
    let map = unsafe { Mmap::map(&f)? };
    // SAFETY: sysconf has no preconditions.
    let page = usize::try_from(unsafe { libc::sysconf(libc::_SC_PAGESIZE) }).unwrap_or(16384);
    let pages = map.len().div_ceil(page);
    let mut vec = vec![0 as libc::c_char; pages];
    // SAFETY: `map` covers `map.len()` bytes and `vec` has one byte per page.
    let rc = unsafe { libc::mincore(map.as_ptr().cast(), map.len(), vec.as_mut_ptr()) };
    ensure!(rc == 0, "mincore failed");
    let resident = vec.iter().filter(|&&c| c & 1 != 0).count();
    #[allow(clippy::cast_precision_loss)]
    Ok(resident as f64 / pages.max(1) as f64)
}

/// Turns an archived object back into the decoder's types (used by tests and tools).
#[must_use]
pub fn to_node(p: &PNode) -> Node {
    use crate::node::{AssetRef, Curve, Gradient, Keyframe};
    match p {
        PNode::Absent => Node::Absent,
        PNode::Null => Node::Null,
        PNode::Bool(b) => Node::Bool(*b),
        PNode::Int(i) => Node::Int(*i),
        PNode::UInt(u) => Node::UInt(*u),
        PNode::F32(f) => Node::F32(*f),
        PNode::F64(f) => Node::F64(*f),
        PNode::Str(s) => Node::Str(s.as_str().into()),
        PNode::Enum(e) => Node::Enum(*e),
        PNode::BpRef(g) => Node::BpRef(Guid::from_bytes(*g)),
        PNode::WeakLink(s) => Node::WeakLink(s.as_deref().map(Into::into)),
        PNode::WeakLinkLegacy(g, f) => Node::WeakLinkLegacy(g.as_str().into(), *f),
        PNode::SharedString(s) => Node::SharedString(s.as_ref().map(|(k, a)| {
            Box::new(crate::node::SharedRef {
                key: k.as_str().into(),
                asset: a.as_deref().map(Into::into),
            })
        })),
        PNode::Asset(a) => Node::Asset(
            a.as_ref().map(|(g, f)| Box::new(AssetRef { guid: g.as_str().into(), file_id: *f })),
        ),
        PNode::AssetIndex(i) => Node::AssetIndex(*i),
        PNode::Floats(f) => Node::Floats(f.clone().into()),
        PNode::Ints(v) => Node::Ints(v.clone().into()),
        PNode::Curve { pre, post, keys, modes } => Node::Curve(Box::new(Curve {
            pre_wrap: *pre,
            post_wrap: *post,
            keys: keys
                .iter()
                .zip(modes)
                .map(|(k, m)| Keyframe {
                    time: k[0],
                    value: k[1],
                    in_tangent: k[2],
                    out_tangent: k[3],
                    in_weight: k[4],
                    out_weight: k[5],
                    weighted_mode: *m,
                })
                .collect(),
        })),
        PNode::Gradient { colors, alphas, mode } => Node::Gradient(Box::new(Gradient {
            colors: colors.clone(),
            alphas: alphas.clone(),
            mode: *mode,
        })),
        PNode::Seq(v) => Node::Seq(v.iter().map(to_node).collect()),
        PNode::Object { ty, tagged, fields, extra } => Node::Object(Box::new(Object {
            ty: *ty,
            tagged: *tagged,
            fields: fields.iter().map(to_node).collect(),
            extra: extra
                .iter()
                .map(|(k, v)| {
                    (k.as_str().into(), serde_json::from_str(v).unwrap_or(serde_json::Value::Null))
                })
                .collect(),
        })),
        PNode::Discarded(s) => {
            Node::Discarded(Box::new(serde_json::from_str(s).unwrap_or(serde_json::Value::Null)))
        }
        PNode::Coerced(n, v) => Node::Coerced(
            Box::new(to_node(n)),
            Box::new(serde_json::from_str(v).unwrap_or(serde_json::Value::Null)),
        ),
        PNode::Opaque { json, .. } => Node::Opaque(Box::new(crate::node::Opaque {
            value: serde_json::from_str(json).unwrap_or(serde_json::Value::Null),
            reason: crate::node::Reason::ConverterError("from pack"),
        })),
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    const BP: &str = r#"{"AssetId": "00000000000000000000000000000abc", "Data": {
        "$type": "3ec4f91d40b87d34197f44f40a969d92, BlueprintScriptableObject",
        "PrototypeLink": "", "m_Overrides": ["x"], "Components": [], "Comment": "c", "Editor": 1}}"#;

    #[test]
    fn build_open_find_load() {
        let dir = std::env::temp_dir().join(format!("wr-content-pack-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zip_path = dir.join("bp.zip");
        {
            let mut z = zip::ZipWriter::new(File::create(&zip_path).unwrap());
            let o = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            z.start_file("A/One.jbp", o).unwrap();
            z.write_all(BP.as_bytes()).unwrap();
            z.start_file("B/Two.jbp", o).unwrap();
            z.write_all(BP.replace("abc", "abd").as_bytes()).unwrap();
            z.finish().unwrap();
        }
        let out = dir.join("p.wrcpack");
        let s = build(&zip_path, &out, BuildOptions { keep_extra: true }).unwrap();
        assert_eq!(s.blueprints, 2);
        // SAFETY: the test owns the file and doesn't modify it while it's open.
        let p = unsafe { Pack::open(&out) }.unwrap();
        assert!(p.is_current(&zip_path).unwrap());
        let g = Guid::parse("00000000000000000000000000000abd").unwrap();
        let e = p.find(g).unwrap();
        assert_eq!(p.path(&e).unwrap(), "B/Two.jbp");
        assert_eq!(schema::ty(e.ty).short, "BlueprintScriptableObject");
        assert!(p.find(Guid(5)).is_none());
        assert_eq!(p.of_type(e.ty).count(), 2);
        // Round trip: the loaded node equals the decoded one, editor field included.
        let node = to_node(&p.load(&e).unwrap());
        let o = node.as_object().unwrap();
        assert_eq!(o.get("Comment"), Some(&Node::Str("c".into())));
        assert_eq!(o.extra.len(), 1);
        // A pack from another schema or a damaged one is refused.
        let mut bytes = std::fs::read(&out).unwrap();
        bytes[16] ^= 1;
        std::fs::write(dir.join("bad.wrcpack"), &bytes).unwrap();
        drop(p);
        // SAFETY: as above.
        assert!(unsafe { Pack::open(&dir.join("bad.wrcpack")) }.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
