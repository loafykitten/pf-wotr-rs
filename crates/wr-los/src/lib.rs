//! Line of sight, exact to the game: `Owlcat.Runtime.Visual`'s `LineOfSightGeometry` (managed 2D
//! geometry over the scene's `FogOfWarBlocker` polylines, in 3.9 m cells), as Mono's float
//! arithmetic evaluates it. No physics is involved.
//!
//! An area's blockers reach the sim through the line-of-sight file ([`LosFile`]), which
//! `wr-import los-pack` writes once from the area's scenes into `cache/sim/los/<area guid>.wrlos`:
//!
//! ```text
//! 0   magic (8 bytes), version (u32), reserved (u32), payload length (u64), payload FNV-1a (u64)
//! 32  payload (postcard): LosFile
//! ```

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub mod geometry;

pub use geometry::{Bounds, Geometry, LosError};

/// File magic: `WRLOS`, then NULs and the format generation.
pub const MAGIC: [u8; 8] = *b"WRLOS\0\0\x01";
/// Bumped whenever [`LosFile`] or anything it holds changes incompatibly.
pub const VERSION: u32 = 2;
const HEADER_LEN: usize = 32;

/// One `FogOfWarBlocker` as `Rebuild` leaves it: the polyline `LineOfSightGeometry` registers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blocker {
    /// The scene file it is in.
    pub scene: String,
    /// `Root/…/Name` of its GameObject.
    pub path: String,
    /// Sibling indices from just below the scene root down to the GameObject.
    pub sibling_path: Vec<u32>,
    /// `FogOfWarBlocker.Points` (x, z of the world points).
    pub points: Vec<[f32; 2]>,
    /// `FogOfWarBlocker.Closed`.
    pub closed: bool,
    /// `HeightMinMax.y`: a segment lower than the eye is ignored.
    pub height_y: f32,
    /// Where the points come from: `polygon` (`FogOfWarBlockerPolygon`) or `mesh` (the convex hull
    /// of the `MeshFilter`'s vertices).
    pub source: String,
    /// A `FogOfWarBlockerUpdater` rebuilds it when its transform changes.
    pub updatable: bool,
}

/// A `FogOfWarArea` of the scenes: where `SceneLoader.SetFogOfWarParameters` places the area's
/// bounds (`bounds.center -= position`, then `GetWorldBounds` adds it back).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FogArea {
    pub scene: String,
    pub path: String,
    /// The transform's world position.
    pub position: [f32; 3],
}

/// An area's line-of-sight inputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LosFile {
    /// The area blueprint's guid.
    pub area: String,
    /// The scene bundles the blockers came from.
    pub scenes: Vec<String>,
    /// The enabled blockers of active GameObjects (`FogOfWarBlocker.All` after the scenes load).
    pub blockers: Vec<Blocker>,
    pub areas: Vec<FogArea>,
    /// `SpawnerGuardSettings` with an `ExtendedVisionArea` in the scenes (units that see a zone
    /// beyond their line of sight): none are modelled.
    pub extended_vision_spawners: u32,
}

/// `cache/sim/los/<area guid>.wrlos`.
#[must_use]
pub fn path_for(area_guid: &str) -> PathBuf {
    wr_paths::cache_root().join("sim/los").join(format!("{area_guid}.wrlos"))
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

impl LosFile {
    /// Writes the file whole: to a temporary file beside it, then renamed over it.
    ///
    /// # Errors
    ///
    /// Encoding or the file system.
    pub fn write(&self, path: &Path) -> Result<()> {
        let payload = postcard::to_allocvec(self).context("encoding the line-of-sight file")?;
        let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&fnv1a64(&payload).to_le_bytes());
        bytes.extend_from_slice(&payload);
        let dir = path.parent().context("line-of-sight file path without a directory")?;
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        static WRITES: AtomicU64 = AtomicU64::new(0);
        let tmp = dir.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("los"),
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

    /// Reads and checks a line-of-sight file: magic, version, length, checksum, whole payload.
    ///
    /// # Errors
    ///
    /// A missing or damaged file.
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::decode(&bytes).with_context(|| format!("line-of-sight file {}", path.display()))
    }

    /// # Errors
    ///
    /// Damaged bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= HEADER_LEN, "truncated header ({} bytes)", bytes.len());
        ensure!(bytes[..8] == MAGIC, "not a line-of-sight file");
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
        for b in &file.blockers {
            ensure!(
                b.points.iter().flatten().all(|x| x.is_finite()) && !b.height_y.is_nan(),
                "{}: a blocker that isn't finite",
                b.path
            );
        }
        Ok(file)
    }
}

#[cfg(test)]
mod tests;
