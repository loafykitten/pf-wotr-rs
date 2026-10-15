//! Physics queries, exact to the game: Unity 2020.3's `Physics.Raycast` over PhysX 4.1 and
//! Owlcat's `PhysicUtils.HeuristicDiscCastDown`, against an area's static colliders.
//!
//! What is exact: the terrain (`TerrainCollider`, a PhysX height field), whose raycast is a port
//! of `raycast_heightField` checked against the player's own function, and whose construction
//! from the `TerrainData` is checked against every recorded query. Boxes, triangle meshes and
//! the other collider kinds are kept with their geometry but not yet verified against the game
//! (no recorded query reaches one): a query that may touch one fails closed with
//! [`QueryError::Unverified`] instead of guessing. `docs/poc5/physics.md` has the details.
//!
//! An area's colliders reach the sim through the physics file ([`PhysFile`]), which
//! `wr-import phys-pack` writes once from the area's scenes into
//! `cache/sim/phys/<area guid>.wrphys`:
//!
//! ```text
//! 0   magic (8 bytes), version (u32), reserved (u32), payload length (u64), payload FNV-1a (u64)
//! 32  payload (postcard): PhysFile
//! ```

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub mod boxes;
pub mod conservative;
pub mod hf;
#[cfg(test)]
mod native_check;
pub mod px;
pub mod query;

pub use query::{QueryError, RaycastHit};

/// File magic: `WRPHYS`, then NULs and the format generation.
pub const MAGIC: [u8; 8] = *b"WRPHYS\0\x01";
/// Bumped whenever [`PhysFile`] or anything it holds changes incompatibly.
pub const VERSION: u32 = 1;
const HEADER_LEN: usize = 32;

/// An area's colliders.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhysFile {
    /// The area blueprint's guid.
    pub area: String,
    /// The scene bundles the colliders came from.
    pub scenes: Vec<String>,
    pub world: World,
}

/// The colliders PhysX holds for the area's static scenes, in scene order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct World {
    pub shapes: Vec<Shape>,
}

/// One collider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    /// The GameObject's hierarchy path and the collider class, for reports.
    pub name: String,
    /// The GameObject's layer (as the game runs it).
    pub layer: u32,
    pub is_trigger: bool,
    /// A `*_!Visual` GameObject: the game moves these to layer 11 (`OptimizeShadow`) at load
    /// for a reason not traced yet (`docs/poc0/extraction.md`), so its layer is `layer` or 11.
    pub visual_layer_move: bool,
    pub kind: ShapeKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ShapeKind {
    /// A `TerrainCollider`: the height field and the actor's pose.
    HeightField { pose: px::Transform, field: hf::HeightField },
    /// A `BoxCollider` (`center`, `size`) under its GameObject's `localToWorldMatrix`.
    Box { local_to_world: [f32; 16], center: [f32; 3], size: [f32; 3] },
    /// A non-convex `MeshCollider`: the triangles PhysX cooked (the mesh's
    /// `m_BakedTriangleCollisionMesh`) under the GameObject's `localToWorldMatrix`.
    Mesh { local_to_world: [f32; 16], vertices: Vec<[f32; 3]>, triangles: Vec<[u32; 3]> },
    /// Another collider kind (capsule, sphere, character controller, convex mesh): its
    /// world-space bounding box, enough to fail closed.
    Other { class: String, min: [f32; 3], max: [f32; 3] },
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

/// `cache/sim/phys/<area guid>.wrphys`.
#[must_use]
pub fn path_for(area_guid: &str) -> PathBuf {
    wr_paths::cache_root().join("sim/phys").join(format!("{area_guid}.wrphys"))
}

impl PhysFile {
    /// Writes the file whole: to a temporary file beside it, then renamed over it.
    pub fn write(&self, path: &Path) -> Result<()> {
        let payload = postcard::to_allocvec(self).context("encoding the physics file")?;
        let mut bytes = Vec::with_capacity(HEADER_LEN + payload.len());
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&fnv1a64(&payload).to_le_bytes());
        bytes.extend_from_slice(&payload);
        let dir = path.parent().context("physics file path without a directory")?;
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        static WRITES: AtomicU64 = AtomicU64::new(0);
        let tmp = dir.join(format!(
            ".{}.{}.{}.tmp",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("phys"),
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

    /// Reads and checks a physics file: magic, version, length, checksum, whole payload.
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::decode(&bytes).with_context(|| format!("physics file {}", path.display()))
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= HEADER_LEN, "truncated header ({} bytes)", bytes.len());
        ensure!(bytes[..8] == MAGIC, "not a physics file");
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
        file.world.validate()?;
        Ok(file)
    }
}

impl World {
    /// Structural checks a query relies on (sample counts, indices in range).
    pub fn validate(&self) -> Result<()> {
        for s in &self.shapes {
            match &s.kind {
                ShapeKind::HeightField { field, .. } => {
                    ensure!(
                        field.rows >= 2 && field.columns >= 2,
                        "{}: height field smaller than 2 x 2",
                        s.name
                    );
                    ensure!(
                        field.samples.len() as u64
                            == u64::from(field.rows) * u64::from(field.columns),
                        "{}: {} samples for {} x {}",
                        s.name,
                        field.samples.len(),
                        field.rows,
                        field.columns
                    );
                }
                ShapeKind::Mesh { vertices, triangles, .. } => {
                    ensure!(
                        triangles.iter().flatten().all(|&i| (i as usize) < vertices.len()),
                        "{}: triangle index out of range",
                        s.name
                    );
                }
                ShapeKind::Box { .. } | ShapeKind::Other { .. } => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
