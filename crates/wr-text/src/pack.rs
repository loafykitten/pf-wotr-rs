//! Localization packs (decompiled, not copied: `Kingmaker.Localization/LocalizationPack.cs`,
//! `LocalizationManager.cs`).
//!
//! The game reads `StreamingAssets/Localization/<locale>.json` (its build doesn't use the
//! packed `.bin` form): one object, `{"strings": {key: text, …}}`, deserialized into a
//! `Dictionary<string, StringEntry>` (a repeated key keeps the last text, a `null` text
//! reads as ""). We read that JSON once and write our own format, `cache/text/<locale>.wrtext`,
//! which opens by memory-mapping with no parse: the keys sorted by their UTF-8 bytes (an
//! ordinal sort, so lookups are the C# dictionary's exact-match lookups), each with its text.
//!
//! Layout (little-endian): the magic `WRTEXT\0\x01`, the source JSON's length and
//! modification time (u64 nanoseconds since the epoch; a rebuild is due when either
//! changes), the entry count `n` (u32), then `n` entries of four u32s (key offset, key
//! length, text offset, text length, into the blob), then the blob. Opening checks every
//! entry's bounds and the keys' order; a lookup checks the strings' UTF-8.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result, bail, ensure};
use memmap2::Mmap;

const MAGIC: &[u8; 8] = b"WRTEXT\0\x01";
const HEADER: usize = 8 + 8 + 8 + 4;
const ENTRY: usize = 16;

/// `Locale`: the packs the game ships (`Sound` maps keys to voice-over events).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Locale {
    EnGb,
    RuRu,
    DeDe,
    FrFr,
    ZhCn,
    EsEs,
    ItIt,
    PtBr,
    Sound,
}

impl Locale {
    /// The pack's file stem (the enum's C# name).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::EnGb => "enGB",
            Self::RuRu => "ruRU",
            Self::DeDe => "deDE",
            Self::FrFr => "frFR",
            Self::ZhCn => "zhCN",
            Self::EsEs => "esES",
            Self::ItIt => "itIT",
            Self::PtBr => "ptBR",
            Self::Sound => "Sound",
        }
    }

    /// Parses a pack name (`enGB`, `Sound`, …).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::EnGb,
            Self::RuRu,
            Self::DeDe,
            Self::FrFr,
            Self::ZhCn,
            Self::EsEs,
            Self::ItIt,
            Self::PtBr,
            Self::Sound,
        ]
        .into_iter()
        .find(|l| l.name() == s)
    }
}

/// The game's JSON pack for `locale` in Ember's install.
#[must_use]
pub fn source_path(locale: Locale) -> PathBuf {
    wr_paths::GamePaths::from_env()
        .streaming_assets()
        .join("Localization")
        .join(format!("{}.json", locale.name()))
}

/// Our cached pack for `locale`.
#[must_use]
pub fn cache_path(locale: Locale) -> PathBuf {
    wr_paths::cache_root().join("text").join(format!("{}.wrtext", locale.name()))
}

/// Reads a game JSON pack: its strings, sorted by key (ordinal), the last of a repeated
/// key kept, `null` read as "".
///
/// # Errors
///
/// If the file can't be read or isn't a pack.
pub fn read_json(path: &Path) -> Result<BTreeMap<String, String>> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    let Some(strings) = v.get("strings").and_then(serde_json::Value::as_object) else {
        bail!("{}: no `strings` object", path.display());
    };
    // serde_json's map keeps the last of a repeated key, as Newtonsoft's dictionary
    // indexer does.
    let mut out = BTreeMap::new();
    for (k, t) in strings {
        let text = match t {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Null => String::new(),
            other => bail!("{}: key {k}: text is {other}", path.display()),
        };
        out.insert(k.clone(), text);
    }
    Ok(out)
}

fn fingerprint(path: &Path) -> Result<(u64, u64)> {
    let m = std::fs::metadata(path).with_context(|| format!("stat {}", path.display()))?;
    let mtime = m
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
    Ok((m.len(), mtime))
}

/// Writes our pack format for `strings` to `out`, through a temporary file renamed into
/// place (so a mapped older pack is never changed under its reader).
///
/// # Errors
///
/// On I/O errors, or a pack too large for 32-bit offsets.
pub fn write(strings: &BTreeMap<String, String>, source: (u64, u64), out: &Path) -> Result<()> {
    let n = u32::try_from(strings.len()).context("too many strings")?;
    let mut index = Vec::with_capacity(strings.len() * ENTRY);
    let mut blob = Vec::new();
    for (k, t) in strings {
        for s in [k, t] {
            let off = u32::try_from(blob.len()).context("pack over 4 GiB")?;
            let len = u32::try_from(s.len()).context("string over 4 GiB")?;
            index.extend_from_slice(&off.to_le_bytes());
            index.extend_from_slice(&len.to_le_bytes());
            blob.extend_from_slice(s.as_bytes());
        }
    }
    u32::try_from(blob.len()).context("pack over 4 GiB")?;
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // A temporary file of this writer's own (created exclusively), so concurrent builds never
    // write through one another's file.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let tmp = out.with_extension(format!("wrtext.{}.{nanos}.{seq}.tmp", std::process::id()));
    {
        let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        let mut f = std::io::BufWriter::new(file);
        f.write_all(MAGIC)?;
        f.write_all(&source.0.to_le_bytes())?;
        f.write_all(&source.1.to_le_bytes())?;
        f.write_all(&n.to_le_bytes())?;
        f.write_all(&index)?;
        f.write_all(&blob)?;
        f.into_inner().map_err(std::io::IntoInnerError::into_error)?.sync_all()?;
    }
    std::fs::rename(&tmp, out).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// Builds our pack for `locale` from the install's JSON when it's missing or stale.
/// Returns whether it was (re)built.
///
/// # Errors
///
/// If the source can't be read or the pack written.
pub fn ensure_built(locale: Locale) -> Result<bool> {
    let src = source_path(locale);
    let out = cache_path(locale);
    let fp = fingerprint(&src)?;
    let mut h = [0u8; HEADER];
    if File::open(&out).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut h)).is_ok()
        && &h[..8] == MAGIC
        && h[8..16] == fp.0.to_le_bytes()
        && h[16..24] == fp.1.to_le_bytes()
    {
        return Ok(false);
    }
    let strings = read_json(&src)?;
    write(&strings, fp, &out)?;
    Ok(true)
}

/// A localization pack in our format, memory-mapped.
#[derive(Debug)]
pub struct Pack {
    locale: Locale,
    map: Mmap,
    n: usize,
    blob: usize,
}

impl Pack {
    /// Opens our pack for `locale` from the cache, building it from the install first if
    /// it's missing or stale.
    ///
    /// # Errors
    ///
    /// If the pack can't be built or is malformed.
    pub fn load(locale: Locale) -> Result<Self> {
        ensure_built(locale)?;
        // SAFETY: `write` is the only writer of the cache's packs, and it replaces a pack by
        // renaming a new file over it, never by writing in place, so this mapping's file
        // doesn't change while mapped.
        unsafe { Self::open(&cache_path(locale), locale) }
    }

    /// Opens a pack file in our format.
    ///
    /// # Safety
    ///
    /// The file must not be modified while the pack is open (replacing it by a rename is
    /// fine). Every read goes through bounds-checked slices.
    ///
    /// # Errors
    ///
    /// If the file can't be mapped or is malformed.
    pub unsafe fn open(path: &Path, locale: Locale) -> Result<Self> {
        let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
        // SAFETY: the caller guarantees the file isn't modified while mapped.
        let map = unsafe { Mmap::map(&f) }.with_context(|| format!("mmap {}", path.display()))?;
        Self::from_map(map, locale).with_context(|| format!("pack {}", path.display()))
    }

    fn from_map(map: Mmap, locale: Locale) -> Result<Self> {
        ensure!(map.len() >= HEADER && &map[..8] == MAGIC, "not a wr-text pack");
        let n = u32::from_le_bytes(map[24..28].try_into()?) as usize;
        let blob = n.checked_mul(ENTRY).and_then(|i| i.checked_add(HEADER)).context("count")?;
        ensure!(blob <= map.len(), "index past the end");
        let p = Self { locale, map, n, blob };
        let blob_len = p.map.len() - blob;
        let mut prev: Option<&[u8]> = None;
        for i in 0..n {
            let [ko, kl, to, tl] = p.entry(i);
            for (o, l) in [(ko, kl), (to, tl)] {
                ensure!(o.checked_add(l).is_some_and(|e| e <= blob_len), "entry {i} out of bounds");
            }
            let k = &p.map[blob + ko..blob + ko + kl];
            ensure!(std::str::from_utf8(k).is_ok(), "entry {i}: key isn't UTF-8");
            ensure!(prev.is_none_or(|q| q < k), "keys out of order at entry {i}");
            prev = Some(k);
        }
        Ok(p)
    }

    fn entry(&self, i: usize) -> [usize; 4] {
        let at = HEADER + i * ENTRY;
        let mut out = [0; 4];
        for (j, o) in out.iter_mut().enumerate() {
            let b = &self.map[at + 4 * j..at + 4 * j + 4];
            *o = u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
        }
        out
    }

    fn bytes(&self, off: usize, len: usize) -> &[u8] {
        &self.map[self.blob + off..self.blob + off + len]
    }

    /// The pack's locale.
    #[must_use]
    pub fn locale(&self) -> Locale {
        self.locale
    }

    /// The number of strings.
    #[must_use]
    pub fn len(&self) -> usize {
        self.n
    }

    /// Whether the pack has no strings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// The text stored under `key`, if the pack has the key.
    ///
    /// # Errors
    ///
    /// If the stored text isn't UTF-8 (a damaged pack).
    pub fn find(&self, key: &str) -> Result<Option<&str>> {
        let k = key.as_bytes();
        let (mut lo, mut hi) = (0, self.n);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let [ko, kl, to, tl] = self.entry(mid);
            match self.bytes(ko, kl).cmp(k) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => {
                    let t = std::str::from_utf8(self.bytes(to, tl))
                        .with_context(|| format!("key {key}: text isn't UTF-8"))?;
                    return Ok(Some(t));
                }
            }
        }
        Ok(None)
    }

    /// `LocalizationPack.GetText(key, reportUnknown)`: the key's text, else
    /// `[unknown key: …]` when reporting unknown keys, else "".
    ///
    /// # Errors
    ///
    /// If the stored text isn't UTF-8 (a damaged pack).
    pub fn get_text(&self, key: &str, report_unknown: bool) -> Result<String> {
        Ok(match self.find(key)? {
            Some(t) => t.to_owned(),
            None if report_unknown => format!("[unknown key: {key}]"),
            None => String::new(),
        })
    }

    /// Every key in order, with its text.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        (0..self.n).filter_map(|i| {
            let [ko, kl, to, tl] = self.entry(i);
            Some((
                std::str::from_utf8(self.bytes(ko, kl)).ok()?,
                std::str::from_utf8(self.bytes(to, tl)).ok()?,
            ))
        })
    }
}

/// `LocalizedString`'s key lookup (`LoadString` through `GetActualKey`): "" for an empty
/// key, else the pack's text with unknown keys reported. The content pack already holds a
/// shared string's key in `m_Key` (its build-time rewrite), so the key is the actual key.
///
/// # Errors
///
/// If the stored text isn't UTF-8 (a damaged pack).
pub fn load_string(pack: Option<&Pack>, key: &str) -> Result<String> {
    match pack {
        Some(p) if !key.is_empty() => p.get_text(key, true),
        _ => Ok(String::new()),
    }
}
