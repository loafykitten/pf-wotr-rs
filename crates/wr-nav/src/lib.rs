//! Navigation, exact to the game: A* Pathfinding Project 4.3.10's graphs as it holds them
//! at runtime (`world`), its search (`astar`), nearest-node rules (`nearest`) and the path
//! modifiers Owlcat's seekers use (`funnel`, `modifiers`), with Mono's and Unity's float
//! semantics (`cs`); `query` runs the query kinds WrathOracle records.
//!
//! An area's graphs reach the sim through the nav file ([`NavFile`]), which `wr-import
//! nav-pack` writes once from the game's `.nav` bundle into `cache/sim/nav/<area guid>.wrnav`:
//!
//! ```text
//! 0   magic (8 bytes), version (u32), reserved (u32), payload length (u64), payload FNV-1a (u64)
//! 32  payload (postcard): NavFile
//! ```

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub mod astar;
pub mod cs;
pub mod funnel;
pub mod modifiers;
#[cfg(test)]
mod native_check;
pub mod nearest;
pub mod query;
pub mod world;

/// Integer coordinates in millimetres (`Int3`, precision 1000).
pub type Int3 = [i32; 3];

pub const INT3_PRECISION: f32 = 1000.0;

/// `GraphNode.Flags` bits kept by serialization (`& 0xFFF80001`).
pub const FLAG_WALKABLE: u32 = 1;
pub const FLAG_TAG_SHIFT: u32 = 19;
pub const FLAG_TAG_MASK: u32 = 0x00F8_0000;
pub const FLAG_GRAPH_SHIFT: u32 = 24;

/// File magic: `WRNAV`, then NULs and the format generation.
pub const MAGIC: [u8; 8] = *b"WRNAV\0\0\x01";
/// Bumped whenever [`NavFile`] or anything it holds changes incompatibly.
pub const VERSION: u32 = 1;
const HEADER_LEN: usize = 32;

/// Where the graph came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// The `.nav` bundle, e.g. `prologue_caves_1.nav`.
    pub bundle: String,
    /// Length and FNV-1a of the graph cache (the bundle's `TextAsset` bytes).
    pub cache_len: u64,
    pub cache_fnv: u64,
}

/// An area's navigation graphs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavFile {
    /// The area blueprint's guid.
    pub area: String,
    pub source: Source,
    pub world: world::World,
}

/// FNV-1a, 64 bit (detects truncation and stale or torn files, not tampering).
#[must_use]
pub fn fnv1a64(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// `cache/sim/nav/<area guid>.wrnav`.
#[must_use]
pub fn path_for(area_guid: &str) -> PathBuf {
    wr_paths::cache_root().join("sim/nav").join(format!("{area_guid}.wrnav"))
}

impl NavFile {
    /// Writes the file whole: to a temporary file beside it, then renamed over it, so a
    /// reader never sees a partial file and concurrent writers each replace it whole.
    pub fn write(&self, path: &Path) -> Result<()> {
        let payload = postcard::to_allocvec(self).context("encoding the nav file")?;
        let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&fnv1a64(&payload).to_le_bytes());
        bytes.extend_from_slice(&payload);
        let dir = path.parent().context("nav file path without a directory")?;
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        // A name no other write uses (process, then a per-process counter), created
        // exclusively, removed if anything fails before the rename.
        static WRITES: AtomicU64 = AtomicU64::new(0);
        let tmp = dir.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("nav"),
            std::process::id(),
            WRITES.fetch_add(1, Ordering::Relaxed)
        ));
        let written = (|| -> Result<()> {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .with_context(|| format!("creating {}", tmp.display()))?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            drop(f);
            fs::rename(&tmp, path).with_context(|| format!("renaming onto {}", path.display()))
        })();
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        written
    }

    /// Reads and checks a nav file: magic, version, length and checksum, and that the
    /// payload decodes whole.
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::decode(&bytes).with_context(|| format!("nav file {}", path.display()))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= HEADER_LEN, "truncated header ({} bytes)", bytes.len());
        ensure!(bytes[..8] == MAGIC, "not a nav file");
        let u32_at = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().expect("4 bytes"));
        let u64_at = |o: usize| u64::from_le_bytes(bytes[o..o + 8].try_into().expect("8 bytes"));
        let version = u32_at(8);
        ensure!(
            version == VERSION,
            "format version {version}, this build reads {VERSION}: rebuild it"
        );
        let len = u64_at(16);
        let payload = &bytes[HEADER_LEN..];
        ensure!(
            payload.len() as u64 == len,
            "payload is {} bytes, header says {len}",
            payload.len()
        );
        ensure!(fnv1a64(payload) == u64_at(24), "checksum mismatch");
        let (file, rest): (Self, _) =
            postcard::take_from_bytes(payload).context("decoding the payload")?;
        ensure!(rest.is_empty(), "{} bytes after the payload", rest.len());
        Ok(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> NavFile {
        NavFile {
            area: "bad465e5404604346bdf71ef50b3299a".into(),
            source: Source { bundle: "prologue_caves_1.nav".into(), cache_len: 3, cache_fnv: 7 },
            world: world::World {
                tris: vec![world::Tri {
                    tile: 0,
                    index_in_tile: 0,
                    v: [0, 1, 2],
                    position: [1, -2, 3],
                    flags: FLAG_WALKABLE,
                    conns: vec![world::Conn { node: 0, cost: 5, shape_edge: 1 }],
                }],
                recast: None,
                grid: None,
                area: vec![1],
                max_nearest_node_distance: 100.0,
                full_get_nearest_search: false,
            },
        }
    }

    fn encode(f: &NavFile) -> Vec<u8> {
        static CALLS: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "wr-nav-test-{}-{}",
            std::process::id(),
            CALLS.fetch_add(1, Ordering::Relaxed)
        ));
        let path = dir.join("a.wrnav");
        f.write(&path).unwrap();
        let bytes = fs::read(&path).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        bytes
    }

    #[test]
    fn round_trips() {
        let f = sample();
        let back = NavFile::decode(&encode(&f)).unwrap();
        assert_eq!(postcard::to_allocvec(&back).unwrap(), postcard::to_allocvec(&f).unwrap());
        assert_eq!(back.world.tris[0].conns[0].cost, 5);
    }

    #[test]
    fn concurrent_writers_each_replace_the_file_whole() {
        let dir = std::env::temp_dir().join(format!("wr-nav-test-conc-{}", std::process::id()));
        let path = dir.join("a.wrnav");
        let want = postcard::to_allocvec(&sample()).unwrap();
        std::thread::scope(|s| {
            for _ in 0..4 {
                s.spawn(|| {
                    for _ in 0..50 {
                        sample().write(&path).unwrap();
                        let back = NavFile::read(&path).unwrap();
                        assert_eq!(postcard::to_allocvec(&back).unwrap(), want);
                    }
                });
            }
        });
        let left: Vec<_> = fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!(left, ["a.wrnav"], "temporary files left behind");
    }

    #[test]
    fn damaged_files_are_rejected() {
        let good = encode(&sample());
        let mut cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("truncated header", good[..20].to_vec()),
            ("truncated payload", good[..good.len() - 1].to_vec()),
            ("trailing byte", [good.as_slice(), &[0]].concat()),
        ];
        let mut magic = good.clone();
        magic[0] ^= 1;
        cases.push(("magic", magic));
        let mut version = good.clone();
        version[8] = 2;
        cases.push(("version", version));
        let mut flipped = good.clone();
        *flipped.last_mut().unwrap() ^= 0x40;
        cases.push(("payload byte", flipped));
        for (what, bytes) in cases {
            assert!(NavFile::decode(&bytes).is_err(), "{what} accepted");
        }
    }
}
