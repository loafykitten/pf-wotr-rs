//! Runtime asset packs.
//!
//! `wr-import` builds one pack per area offline from the game's Unity bundles; the engine
//! memory-maps it and never parses a Unity format. A pack is a single file:
//!
//! ```text
//! 0      header (64 bytes): magic, version, meta offset/length/checksum, file length
//! 4096   blobs, each 256-byte aligned: vertex and index buffers, texture payloads
//!        (shipped BC blocks unchanged), Metal libraries, MSL sources
//! end    metadata (postcard): textures, meshes, shader programs, materials,
//!        renderers, lights, lightmaps, scene render settings
//! ```
//!
//! Every object keeps its original Unity identity ([`AssetId`]: serialized file name plus
//! path id), so packs can be diffed against the game and against each other. Blobs are
//! borrowed straight from the mapping ([`Pack::blob`]); metadata is decoded once at open
//! (it is small: tens of kilobytes to a few megabytes).

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};

pub mod model;

pub use model::*;

/// File magic: `WRPACK`, then a NUL and the format generation.
pub const MAGIC: [u8; 8] = *b"WRPACK\x001";
/// Bumped whenever [`PackMeta`] or the layout changes incompatibly.
pub const VERSION: u32 = 4;
/// Where blob data starts.
pub const DATA_START: u64 = 4096;
/// Alignment of every blob (and of the metadata).
pub const BLOB_ALIGN: u64 = 256;
const HEADER_LEN: usize = 64;

/// A byte range inside the pack file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct BlobRef {
    pub offset: u64,
    pub len: u64,
}

/// FNV-1a, 64 bit: the metadata checksum (detects truncation and stale or torn files,
/// not tampering).
#[must_use]
pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn align_up(x: u64, a: u64) -> u64 {
    x.div_ceil(a) * a
}

/// Writes a pack: blobs first, then the metadata and the header. The file is written
/// under a temporary name and renamed into place by [`PackWriter::finish`], so a reader
/// never sees a half-written pack at the final path.
#[derive(Debug)]
pub struct PackWriter {
    out: BufWriter<File>,
    tmp: PathBuf,
    path: PathBuf,
    pos: u64,
}

impl PackWriter {
    pub fn create(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // A temporary name unique to this writer, created exclusively: concurrent writers
        // never share an inode, and the final name is only ever replaced by rename.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let tmp = path.with_extension(format!("tmp-{}-{nanos}", std::process::id()));
        let f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .with_context(|| format!("create {}", tmp.display()))?;
        let mut out = BufWriter::with_capacity(1 << 22, f);
        out.write_all(&vec![0u8; usize::try_from(DATA_START)?])?;
        Ok(Self { out, tmp, path: path.to_owned(), pos: DATA_START })
    }

    fn pad_to(&mut self, align: u64) -> Result<()> {
        let next = align_up(self.pos, align);
        if next > self.pos {
            self.out.write_all(&vec![0u8; usize::try_from(next - self.pos)?])?;
            self.pos = next;
        }
        Ok(())
    }

    /// Appends one blob (aligned) and returns where it is.
    pub fn blob(&mut self, data: &[u8]) -> Result<BlobRef> {
        self.pad_to(BLOB_ALIGN)?;
        let r = BlobRef { offset: self.pos, len: data.len() as u64 };
        self.out.write_all(data)?;
        self.pos += data.len() as u64;
        Ok(r)
    }

    /// Bytes written so far (blobs only, before the metadata).
    #[must_use]
    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Writes the metadata and header, syncs, and renames the file into place.
    pub fn finish(mut self, meta: &PackMeta) -> Result<u64> {
        let bytes = postcard::to_allocvec(meta).context("encode pack metadata")?;
        self.pad_to(BLOB_ALIGN)?;
        let meta_offset = self.pos;
        self.out.write_all(&bytes)?;
        self.pos += bytes.len() as u64;
        let file_len = self.pos;
        let mut header = [0u8; HEADER_LEN];
        header[0..8].copy_from_slice(&MAGIC);
        header[8..12].copy_from_slice(&VERSION.to_le_bytes());
        header[16..24].copy_from_slice(&meta_offset.to_le_bytes());
        header[24..32].copy_from_slice(&(bytes.len() as u64).to_le_bytes());
        header[32..40].copy_from_slice(&fnv1a64(&bytes).to_le_bytes());
        header[40..48].copy_from_slice(&file_len.to_le_bytes());
        let mut f = self.out.into_inner().map_err(|e| e.into_error())?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&self.tmp, &self.path)
            .with_context(|| format!("rename into {}", self.path.display()))?;
        Ok(file_len)
    }
}

/// An open, memory-mapped pack.
pub struct Pack {
    map: Mmap,
    /// Decoded metadata.
    pub meta: PackMeta,
    /// Where blobs may live: `[DATA_START, meta_offset)`.
    data_end: u64,
}

impl std::fmt::Debug for Pack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pack")
            .field("len", &self.map.len())
            .field("area", &self.meta.area)
            .finish_non_exhaustive()
    }
}

impl Pack {
    /// Maps a pack and decodes its metadata. Blob references, and each program's MSL
    /// against its recorded hash, are checked here, so [`Pack::blob`] cannot read outside
    /// the data region.
    ///
    /// # Safety
    ///
    /// The file must not be modified or truncated while the returned `Pack` (or any slice
    /// borrowed from it) is alive: the mapping hands out `&[u8]` that would change
    /// underneath (undefined behaviour) or fault (SIGBUS). [`PackWriter`] upholds this
    /// for packs it writes: it creates a new file exclusively and renames it into place,
    /// so rebuilding a pack replaces the directory entry and leaves the inode an existing
    /// mapping uses untouched. Callers must not open files other tools may write in place.
    pub unsafe fn open(path: &Path) -> Result<Self> {
        let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the caller guarantees the file isn't modified while mapped (see above).
        let map = unsafe { Mmap::map(&f) }.with_context(|| format!("mmap {}", path.display()))?;
        Self::from_map(map)
    }

    fn from_map(map: Mmap) -> Result<Self> {
        ensure!(map.len() >= HEADER_LEN, "pack shorter than its header");
        ensure!(map[0..8] == MAGIC, "not a wr pack (bad magic)");
        let u32_at = |o: usize| u32::from_le_bytes(map[o..o + 4].try_into().unwrap_or_default());
        let u64_at = |o: usize| u64::from_le_bytes(map[o..o + 8].try_into().unwrap_or_default());
        let version = u32_at(8);
        ensure!(version == VERSION, "pack version {version}, expected {VERSION}; rebuild it");
        let (meta_offset, meta_len, checksum, file_len) =
            (u64_at(16), u64_at(24), u64_at(32), u64_at(40));
        ensure!(file_len == map.len() as u64, "pack length {} != header {file_len}", map.len());
        let meta_end = meta_offset.checked_add(meta_len).context("metadata range overflows")?;
        ensure!(
            meta_offset >= DATA_START && meta_end <= file_len,
            "metadata range {meta_offset}..{meta_end} outside the file"
        );
        let bytes = &map[usize::try_from(meta_offset)?..usize::try_from(meta_end)?];
        ensure!(fnv1a64(bytes) == checksum, "metadata checksum mismatch");
        let meta: PackMeta = postcard::from_bytes(bytes).context("decode pack metadata")?;
        let pack = Self { map, meta, data_end: meta_offset };
        pack.validate()?;
        Ok(pack)
    }

    fn check(&self, r: BlobRef, what: &str) -> Result<()> {
        let end = r.offset.checked_add(r.len).context("blob range overflows")?;
        if r.len > 0 && (r.offset < DATA_START || end > self.data_end) {
            bail!("{what}: blob {}..{end} outside the data region", r.offset);
        }
        Ok(())
    }

    /// What the skinning pass reads of a skinned mesh's vertex streams: float32 positions (and
    /// normals and tangents when they share the stream), word aligned and inside the stride, in a
    /// stream that holds every vertex.
    fn check_skinned_streams(x: &Mesh) -> Result<()> {
        let pos = x
            .channels
            .first()
            .copied()
            .flatten()
            .with_context(|| format!("{}: skinned mesh without positions", x.name))?;
        ensure!(pos.format == 0 && pos.dimension >= 3, "{}: positions are not float32x3", x.name);
        let s = x
            .streams
            .get(usize::from(pos.stream))
            .with_context(|| format!("{}: position stream {} missing", x.name, pos.stream))?;
        ensure!(s.stride % 4 == 0 && s.stride > 0, "{}: stride {}", x.name, s.stride);
        let need = u64::from(s.stride)
            .checked_mul(u64::from(x.vertex_count))
            .with_context(|| format!("{}: vertex data overflows", x.name))?;
        ensure!(s.data.len >= need, "{}: {} bytes of stream for {need}", x.name, s.data.len);
        for (i, c) in x.channels.iter().enumerate().take(3) {
            let Some(c) = c.filter(|c| c.stream == pos.stream) else { continue };
            if i == 0 || (c.format == 0 && c.dimension >= 3) {
                ensure!(
                    c.offset % 4 == 0 && u32::from(c.offset) + 12 <= s.stride,
                    "{}: channel {i} at {} in a stride of {}",
                    x.name,
                    c.offset,
                    s.stride
                );
            }
        }
        Ok(())
    }

    /// Every bone weight of a skin blob is a number and every used influence names one of the
    /// renderer's `bones`.
    fn check_skin_indices(&self, x: &Mesh, bones: usize) -> Result<()> {
        let Some(sk) = x.skin else { return Ok(()) };
        for (v, row) in self.blob(sk).as_chunks::<32>().0.iter().enumerate() {
            for k in 0..4 {
                let w = f32::from_le_bytes(row[k * 4..k * 4 + 4].try_into().unwrap_or_default());
                let i =
                    u32::from_le_bytes(row[16 + k * 4..20 + k * 4].try_into().unwrap_or_default());
                ensure!(w.is_finite(), "{}: vertex {v} has a weight that isn't a number", x.name);
                ensure!(
                    w <= 0.0 || (i as usize) < bones,
                    "{}: vertex {v} weighs bone {i} of {bones}",
                    x.name
                );
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        let m = &self.meta;
        for t in &m.textures {
            self.check(t.data, &t.name)?;
        }
        for x in &m.meshes {
            self.check(x.indices, &x.name)?;
            for s in &x.streams {
                self.check(s.data, &x.name)?;
            }
            if let Some(sk) = x.skin {
                self.check(sk, &x.name)?;
                ensure!(
                    sk.len == u64::from(x.vertex_count) * SKIN_STRIDE,
                    "{}: skin of {} bytes for {} vertices",
                    x.name,
                    sk.len,
                    x.vertex_count
                );
                Self::check_skinned_streams(x)?;
            }
        }
        for p in &m.programs {
            self.check(p.msl, &p.shader)?;
            if let Some(l) = p.metallib {
                self.check(l, &p.shader)?;
            }
            ensure!(
                fnv1a64(self.blob(p.msl)) == p.msl_hash,
                "{}: MSL doesn't match its recorded hash",
                p.shader
            );
            for b in &p.bindings {
                if let Resource::Uniform { ty } = &b.res {
                    ensure!(
                        p.cbuffers.iter().any(|c| &c.ty == ty),
                        "{}: no layout for constant buffer {ty}",
                        p.shader
                    );
                }
            }
        }
        let nt = m.textures.len();
        let ensure_idx = |i: u32, n: usize, what: &str| -> Result<()> {
            ensure!((i as usize) < n, "{what} index {i} out of range ({n})");
            Ok(())
        };
        for mat in &m.materials {
            ensure_idx(mat.shader, m.shaders.len(), "shader")?;
            for t in mat.textures.iter().filter_map(|t| t.texture) {
                ensure_idx(t, nt, "texture")?;
            }
            for v in &mat.variants {
                ensure_idx(v.vertex, m.programs.len(), "program")?;
                ensure_idx(v.fragment, m.programs.len(), "program")?;
            }
            for c in &mat.casters {
                ensure_idx(c.vertex, m.programs.len(), "caster program")?;
                ensure_idx(c.fragment, m.programs.len(), "caster program")?;
            }
        }
        for rig in &m.rigs {
            rig.validate()?;
        }
        for c in &m.clips {
            c.validate()?;
        }
        for c in &m.controllers {
            c.validate(m.clips.len())?;
        }
        for a in &m.animations {
            a.validate(m.rigs.len(), m.clips.len(), &m.controllers)?;
        }
        let mut checked_skins = std::collections::HashSet::new();
        for r in &m.renderers {
            ensure_idx(r.mesh, m.meshes.len(), "mesh")?;
            for mat in r.materials.iter().flatten() {
                ensure_idx(*mat, m.materials.len(), "material")?;
            }
            if let Some(sk) = &r.skin {
                // The skinning pass indexes the renderer's palette with the skin's bone indices.
                if checked_skins.insert((r.mesh, sk.bones.len())) {
                    self.check_skin_indices(&m.meshes[r.mesh as usize], sk.bones.len())?;
                }
                ensure_idx(sk.animation, m.animations.len(), "animation")?;
                let rig = &m.rigs[m.animations[sk.animation as usize].rig as usize];
                ensure!(
                    sk.bones.len() == sk.bind_poses.len(),
                    "{}: {} bones for {} bind poses",
                    r.path,
                    sk.bones.len(),
                    sk.bind_poses.len()
                );
                ensure!(
                    m.meshes[r.mesh as usize].skin.is_some(),
                    "{}: skinned renderer on a mesh without skin",
                    r.path
                );
                for b in &sk.bones {
                    ensure_idx(*b, rig.bones.len(), "rig bone")?;
                }
            }
        }
        for l in &m.lightmaps {
            for t in [l.color, l.direction, l.shadowmask].into_iter().flatten() {
                ensure_idx(t, nt, "lightmap texture")?;
            }
        }
        for p in &m.pipeline_programs {
            ensure_idx(p.program, m.programs.len(), "pipeline program")?;
        }
        Ok(())
    }

    /// A blob's bytes, borrowed from the mapping. Empty if the range is invalid (which
    /// [`Pack::open`] has already ruled out for every reference in the metadata).
    #[must_use]
    pub fn blob(&self, r: BlobRef) -> &[u8] {
        let (Ok(a), Ok(n)) = (usize::try_from(r.offset), usize::try_from(r.len)) else {
            return &[];
        };
        if r.offset + r.len > self.data_end {
            return &[];
        }
        &self.map[a..a + n]
    }

    /// Size of the mapped file.
    #[must_use]
    pub fn file_len(&self) -> u64 {
        self.map.len() as u64
    }

    /// Asks the kernel to read the whole file ahead (`madvise(WILLNEED)`).
    pub fn prefetch(&self) {
        let _ = self.map.advise(memmap2::Advice::WillNeed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("wr-assets-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn roundtrip_and_validation() {
        let path = tmpdir().join("a.wrpack");
        let mut w = PackWriter::create(&path).unwrap();
        let b1 = w.blob(b"hello").unwrap();
        let b2 = w.blob(&[7u8; 1000]).unwrap();
        assert_eq!(b1.offset, DATA_START);
        assert_eq!(b2.offset % BLOB_ALIGN, 0);
        let mut meta = PackMeta { area: "test".into(), ..Default::default() };
        meta.textures.push(Texture { name: "t".into(), data: b2, ..Default::default() });
        w.finish(&meta).unwrap();
        // SAFETY: the test's own file, not modified while open.
        let p = unsafe { Pack::open(&path) }.unwrap();
        assert_eq!(p.meta.area, "test");
        assert_eq!(p.blob(b1), b"hello");
        assert_eq!(p.blob(p.meta.textures[0].data).len(), 1000);
        assert!(p.blob(BlobRef { offset: 1 << 40, len: 4 }).is_empty());

        // A corrupted metadata byte is caught by the checksum.
        let mut bytes = std::fs::read(&path).unwrap();
        let n = bytes.len();
        bytes[n - 1] ^= 0xFF;
        let bad = path.with_extension("bad");
        std::fs::write(&bad, &bytes).unwrap();
        // SAFETY: as above.
        assert!(unsafe { Pack::open(&bad) }.is_err());
    }

    #[test]
    fn out_of_range_references_are_rejected() {
        let path = tmpdir().join("b.wrpack");
        let w = PackWriter::create(&path).unwrap();
        let mut meta = PackMeta::default();
        meta.textures.push(Texture {
            name: "t".into(),
            data: BlobRef { offset: DATA_START, len: 1 << 20 },
            ..Default::default()
        });
        w.finish(&meta).unwrap();
        // SAFETY: as above.
        assert!(unsafe { Pack::open(&path) }.is_err());
    }

    #[test]
    fn out_of_range_caster_programs_are_rejected() {
        let path = tmpdir().join("c.wrpack");
        let w = PackWriter::create(&path).unwrap();
        let mut meta = PackMeta::default();
        meta.shaders.push(Shader::default());
        meta.materials.push(Material {
            casters: vec![CasterVariant {
                geometry_clip: false,
                vertex: u32::MAX,
                fragment: u32::MAX,
                exact: true,
            }],
            ..Default::default()
        });
        w.finish(&meta).unwrap();
        // SAFETY: as above.
        assert!(unsafe { Pack::open(&path) }.is_err());
    }

    /// A pack with one animated unit: a rig of two bones, a constant clip, a clip-based locomotion,
    /// a skinned mesh of two vertices and the renderer that binds it.
    fn animated(w: &mut PackWriter) -> PackMeta {
        use wr_anim::clip::{ATTR_POSITION, Binding, TYPE_TRANSFORM};
        use wr_anim::loco::{Locomotion, UnitAnimation};
        use wr_anim::rig::{Bone, Rig, Trs};
        let bone = |name: &str, parent| Bone {
            name: name.into(),
            hash: wr_anim::clip::path_hash(name),
            parent,
            rest: Trs::IDENTITY,
        };
        let mut meta = PackMeta { area: "anim".into(), ..Default::default() };
        meta.rigs.push(Rig {
            name: "rig".into(),
            bones: vec![bone("", u32::MAX), bone("hip", 0)],
            root: glam_identity(),
            overrides: Vec::new(),
        });
        meta.clips.push(wr_anim::Clip {
            name: "idle".into(),
            stop: 1.0,
            looping: true,
            bindings: vec![Binding {
                path: wr_anim::clip::path_hash("hip"),
                attribute: ATTR_POSITION,
                type_id: TYPE_TRANSFORM,
            }],
            constant: vec![0.0, 1.0, 0.0],
            ..Default::default()
        });
        meta.animations.push(UnitAnimation {
            unit: "u".into(),
            rig: 0,
            clips: vec![0],
            loco: Locomotion::Clips { idle: 0, walk: vec![None, None], transition: 0.1 },
        });
        // Two vertices of one float3 position each (12 bytes), the first on bone 1, the second
        // split between both bones.
        let skin_rows: [([f32; 4], [u32; 4]); 2] =
            [([1.0, 0.0, 0.0, 0.0], [1, 0, 0, 0]), ([0.5, 0.5, 0.0, 0.0], [0, 1, 0, 0])];
        let skin_bytes: Vec<u8> = skin_rows
            .iter()
            .flat_map(|(w, i)| {
                w.iter()
                    .flat_map(|x| x.to_le_bytes())
                    .chain(i.iter().flat_map(|x| x.to_le_bytes()))
                    .collect::<Vec<u8>>()
            })
            .collect();
        let skin = w.blob(&skin_bytes).unwrap();
        let stream = w.blob(&[0u8; 24]).unwrap();
        meta.meshes.push(Mesh {
            name: "m".into(),
            vertex_count: 2,
            channels: vec![Some(Channel { stream: 0, offset: 0, format: 0, dimension: 3 })],
            streams: vec![VertexStream { data: stream, stride: 12 }],
            skin: Some(skin),
            ..Default::default()
        });
        meta.renderers.push(Renderer {
            path: "unit:u/m".into(),
            kind: RendererKind::Skinned,
            skin: Some(SkinBinding {
                animation: 0,
                bones: vec![1, 0],
                bind_poses: vec![glam_identity(), glam_identity()],
                forbid_rotation: false,
            }),
            ..Default::default()
        });
        meta
    }

    fn glam_identity() -> [f32; 16] {
        let mut m = [0.0; 16];
        for i in 0..4 {
            m[i * 5] = 1.0;
        }
        m
    }

    fn write_and_open(name: &str, build: impl FnOnce(&mut PackWriter) -> PackMeta) -> Result<Pack> {
        let path = tmpdir().join(name);
        let mut w = PackWriter::create(&path).unwrap();
        let meta = build(&mut w);
        w.finish(&meta).unwrap();
        // SAFETY: the test's own file, not modified while open.
        unsafe { Pack::open(&path) }
    }

    #[test]
    fn animated_units_round_trip() {
        let p = write_and_open("d.wrpack", animated).unwrap();
        let m = &p.meta;
        assert_eq!(m.rigs[0].bones[1].name, "hip");
        assert_eq!(m.clips[0].constant, vec![0.0, 1.0, 0.0]);
        assert_eq!(m.animations[0].clips, vec![0]);
        let skin = m.renderers[0].skin.as_ref().unwrap();
        assert_eq!(skin.bones, vec![1, 0]);
        assert_eq!(p.blob(m.meshes[0].skin.unwrap()).len() as u64, 2 * SKIN_STRIDE);
    }

    #[test]
    fn broken_animation_references_are_rejected() {
        let bad = |name: &str, f: fn(&mut PackMeta)| {
            write_and_open(name, |w| {
                let mut m = animated(w);
                f(&mut m);
                m
            })
            .is_err()
        };
        // A skin blob that isn't 32 bytes a vertex.
        assert!(bad("e1.wrpack", |m| m.meshes[0].vertex_count = 3));
        // A bone the rig doesn't have.
        assert!(bad("e2.wrpack", |m| m.renderers[0].skin.as_mut().unwrap().bones[0] = 2));
        // Bones and bind poses of different lengths.
        assert!(bad("e3.wrpack", |m| {
            m.renderers[0].skin.as_mut().unwrap().bind_poses.pop();
        }));
        // An animation that isn't there.
        assert!(bad("e4.wrpack", |m| m.renderers[0].skin.as_mut().unwrap().animation = 1));
        // A skin that weighs a bone the renderer doesn't have (vertex 0 weighs bone 1 of 1).
        assert!(bad("e8.wrpack", |m| {
            let sk = m.renderers[0].skin.as_mut().unwrap();
            sk.bones.truncate(1);
            sk.bind_poses.truncate(1);
        }));
        // Streams the skinning pass can't read: a missing stream, a misaligned or overrunning
        // position, a stream too short for the vertices.
        assert!(bad("e9.wrpack", |m| m.meshes[0].channels[0].as_mut().unwrap().stream = 3));
        assert!(bad("e10.wrpack", |m| m.meshes[0].channels[0].as_mut().unwrap().offset = 2));
        assert!(bad("e11.wrpack", |m| m.meshes[0].channels[0].as_mut().unwrap().offset = 4));
        assert!(bad("e12.wrpack", |m| m.meshes[0].streams[0].stride = 16));
        // A skinned renderer on a mesh without skin.
        assert!(bad("e5.wrpack", |m| m.meshes[0].skin = None));
        // A locomotion clip the unit doesn't list.
        assert!(bad("e6.wrpack", |m| m.animations[0].clips.clear()));
        // A rig whose parent comes after the child.
        assert!(bad("e7.wrpack", |m| m.rigs[0].bones[1].parent = 1));
    }

    #[test]
    fn packs_of_another_version_are_rejected() {
        let path = tmpdir().join("f.wrpack");
        let w = PackWriter::create(&path).unwrap();
        w.finish(&PackMeta::default()).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        // The previous format generation (before rigs, clips and skins).
        bytes[8..12].copy_from_slice(&(VERSION - 1).to_le_bytes());
        let old = path.with_extension("old");
        std::fs::write(&old, &bytes).unwrap();
        // SAFETY: as above.
        let err = unsafe { Pack::open(&old) }.unwrap_err().to_string();
        assert!(err.contains("rebuild"), "{err}");
    }

    #[test]
    fn fnv_known_value() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
