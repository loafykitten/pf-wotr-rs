//! Wwise file packages (`.pck`, magic `AKPK`): the lookup tables, without the payload.
//!
//! The game ships its banks and streamed media inside 36 packages. The native low-level IO
//! (`CAkFilePackageLowLevelIO` in the Unity integration) resolves bank and media IDs
//! through these tables after `LoadFilePackage`. We read them to know which package holds
//! what, and to check that a cue's bank and media are covered by the loaded packages.
//!
//! Layout (version 1, little-endian on Mac): `"AKPK"`, header size, version, then the
//! byte sizes of four sections: language map, bank LUT, streamed-file LUT, external-source
//! LUT. Bank and streamed LUTs hold 32-bit IDs (20-byte entries); the external LUT holds
//! 64-bit IDs (24-byte entries). A file's offset is `start_block * block_size`.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

/// One file in a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// Bank short ID, or streamed media (`.wem`) ID.
    pub id: u64,
    /// Block size (alignment unit) for `start_block`.
    pub block_size: u32,
    /// Size in bytes.
    pub size: u32,
    /// Start, in blocks.
    pub start_block: u32,
    /// Language ID (index into the language map; 0 = SFX).
    pub language: u32,
}

impl Entry {
    /// Byte offset in the package.
    #[must_use]
    pub fn offset(&self) -> u64 {
        u64::from(self.start_block) * u64::from(self.block_size)
    }
}

/// A package's tables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Package {
    /// `(id, name)` language map entries.
    pub languages: Vec<(u32, String)>,
    /// Soundbanks.
    pub banks: Vec<Entry>,
    /// Streamed media.
    pub streamed: Vec<Entry>,
    /// External sources.
    pub externals: Vec<Entry>,
}

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn u32(&mut self) -> Result<u32> {
        let b = self.buf.get(self.pos..self.pos + 4).context("truncated AKPK header")?;
        self.pos += 4;
        Ok(u32::from_le_bytes(b.try_into()?))
    }

    fn u64(&mut self) -> Result<u64> {
        let b = self.buf.get(self.pos..self.pos + 8).context("truncated AKPK header")?;
        self.pos += 8;
        Ok(u64::from_le_bytes(b.try_into()?))
    }
}

fn lut(section: &[u8], wide_ids: bool) -> Result<Vec<Entry>> {
    let mut c = Cursor { buf: section, pos: 0 };
    let n = c.u32()? as usize;
    let entry_size = if wide_ids { 24 } else { 20 };
    ensure!(4 + n * entry_size <= section.len(), "LUT larger than its section");
    (0..n)
        .map(|_| {
            Ok(Entry {
                id: if wide_ids { c.u64()? } else { u64::from(c.u32()?) },
                block_size: c.u32()?,
                size: c.u32()?,
                start_block: c.u32()?,
                language: c.u32()?,
            })
        })
        .collect()
}

fn language_map(section: &[u8]) -> Result<Vec<(u32, String)>> {
    let mut c = Cursor { buf: section, pos: 0 };
    let n = c.u32()? as usize;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let offset = c.u32()? as usize;
        let id = c.u32()?;
        let bytes = section.get(offset..).context("language name out of range")?;
        // Packages built on Windows store UTF-16 names; the game's Mac packages store
        // 8-bit names. Detect by the second byte.
        let name = if bytes.len() >= 2 && bytes[0] != 0 && bytes[1] == 0 {
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&p| u16::from_le_bytes(p))
                .take_while(|&u| u != 0)
                .collect();
            String::from_utf16(&units)?
        } else {
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            std::str::from_utf8(&bytes[..end])?.to_owned()
        };
        out.push((id, name));
    }
    Ok(out)
}

impl Package {
    /// Parses a package header (the first `8 + header_size` bytes of the file).
    ///
    /// # Errors
    ///
    /// If the magic, version or section sizes are wrong.
    pub fn parse(header: &[u8]) -> Result<Self> {
        ensure!(header.get(..4) == Some(b"AKPK"), "not an AKPK file");
        let mut c = Cursor { buf: header, pos: 4 };
        let header_size = c.u32()? as usize;
        ensure!(header.len() >= 8 + header_size, "truncated AKPK header");
        let version = c.u32()?;
        if version != 1 {
            bail!("unsupported AKPK version {version}");
        }
        let sizes = [c.u32()?, c.u32()?, c.u32()?, c.u32()?].map(|s| s as usize);
        ensure!(28 + sizes.iter().sum::<usize>() <= 8 + header_size, "sections overflow header");
        let mut at = 28;
        let mut take = |len: usize| {
            let s = &header[at..at + len];
            at += len;
            s
        };
        let (lang, banks, stm, ext) =
            (take(sizes[0]), take(sizes[1]), take(sizes[2]), take(sizes[3]));
        Ok(Self {
            languages: language_map(lang)?,
            banks: lut(banks, false)?,
            streamed: lut(stm, false)?,
            externals: if ext.len() >= 4 { lut(ext, true)? } else { Vec::new() },
        })
    }

    /// Reads a package's header from disk (not its payload).
    ///
    /// # Errors
    ///
    /// If the file can't be read or parsed.
    pub fn load(path: &Path) -> Result<Self> {
        let mut f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let mut head = [0u8; 8];
        f.read_exact(&mut head)?;
        let size = u32::from_le_bytes(head[4..8].try_into()?) as usize;
        let mut header = head.to_vec();
        header.resize(8 + size, 0);
        f.read_exact(&mut header[8..])?;
        Self::parse(&header).with_context(|| format!("parsing {}", path.display()))
    }

    /// The bank with this short ID.
    #[must_use]
    pub fn bank(&self, id: u32) -> Option<&Entry> {
        self.banks.iter().find(|e| e.id == u64::from(id))
    }

    /// The streamed media file with this ID.
    #[must_use]
    pub fn streamed_file(&self, id: u32) -> Option<&Entry> {
        self.streamed.iter().find(|e| e.id == u64::from(id))
    }
}

#[cfg(test)]
mod tests {
    use super::Package;

    fn build(banks: &[(u32, u32, u32)], streamed: &[(u32, u32, u32)]) -> Vec<u8> {
        let mut lang = Vec::new();
        lang.extend(1u32.to_le_bytes());
        lang.extend(12u32.to_le_bytes()); // offset of the string, from the section start
        lang.extend(0u32.to_le_bytes());
        lang.extend(b"sfx\0");
        let lut = |rows: &[(u32, u32, u32)]| {
            let mut v = Vec::new();
            v.extend(u32::try_from(rows.len()).unwrap().to_le_bytes());
            for &(id, size, start) in rows {
                for x in [id, 1, size, start, 0] {
                    v.extend(x.to_le_bytes());
                }
            }
            v
        };
        let (b, s, e) = (lut(banks), lut(streamed), 0u32.to_le_bytes().to_vec());
        let mut out = b"AKPK".to_vec();
        let header_size = 20 + lang.len() + b.len() + s.len() + e.len();
        out.extend(u32::try_from(header_size).unwrap().to_le_bytes());
        out.extend(1u32.to_le_bytes());
        for len in [lang.len(), b.len(), s.len(), e.len()] {
            out.extend(u32::try_from(len).unwrap().to_le_bytes());
        }
        for sec in [lang, b, s, e] {
            out.extend(sec);
        }
        out
    }

    #[test]
    fn parses_tables_like_the_prologue_vo_package() {
        // Same shape as Wrath_Main_VO_Dialogues_Prologue.pck: one bank, many .wem files.
        let bytes = build(&[(247_461_405, 0x7f99, 0x1280)], &[(0x0088_3366, 0x3_9910, 0x9219)]);
        let p = Package::parse(&bytes).unwrap();
        assert_eq!(p.languages, [(0, "sfx".to_owned())]);
        assert_eq!(p.bank(247_461_405).unwrap().offset(), 0x1280);
        assert_eq!(p.streamed_file(0x0088_3366).unwrap().size, 0x3_9910);
        assert!(p.externals.is_empty());
    }

    #[test]
    fn rejects_garbage() {
        assert!(Package::parse(b"RIFF....").is_err());
        let mut bytes = build(&[], &[]);
        bytes[8] = 2; // version
        assert!(Package::parse(&bytes).is_err());
    }
}
