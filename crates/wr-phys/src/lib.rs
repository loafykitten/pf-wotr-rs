//! Physics queries, exact to the game: Unity 2020.3's `Physics.Raycast` over PhysX 4.1 and
//! Owlcat's `PhysicUtils.HeuristicDiscCastDown`, against an area's static colliders.
//!
//! What is exact: the terrain (`TerrainCollider`, a PhysX height field), whose raycast is a port
//! of `raycast_heightField` checked against the player's own function, and whose construction
//! from the `TerrainData` is checked against every recorded query; triangle meshes (the BV4
//! raycast) and boxes with the transforms a recording holds for them (`ph_col`). Other collider
//! kinds, and meshes and boxes without a recorded transform, are kept with their geometry but
//! not verified against the game: a query that may touch one fails closed with
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
pub mod bv4;
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
pub const VERSION: u32 = 3;
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
    /// The GameObject's sibling indices below its root (`Transform.GetSiblingIndex()`): with
    /// the scene and path, they identify it where names repeat.
    pub sibling_path: Vec<u32>,
    /// The GameObject's layer (as the game runs it).
    pub layer: u32,
    pub is_trigger: bool,
    /// A `*_!Visual` GameObject: the game moves these to layer 11 (`OptimizeShadow`) at load
    /// for a reason not traced yet (`docs/poc0/extraction.md`), so its layer is `layer` or 11.
    pub visual_layer_move: bool,
    pub kind: ShapeKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)] // a few hundred shapes per area, read once
pub enum ShapeKind {
    /// A `TerrainCollider`: the height field and the actor's pose.
    HeightField { pose: px::Transform, field: hf::HeightField },
    /// A `BoxCollider` (`center`, `size`) under its GameObject's `localToWorldMatrix`, the
    /// GameObject's local transforms from the root down (its own last), and with `runtime`
    /// the transform the game was recorded holding, which the queries use.
    Box {
        local_to_world: [f32; 16],
        center: [f32; 3],
        size: [f32; 3],
        chain: Vec<boxes::LocalTrs>,
        runtime: Option<boxes::BoxRuntime>,
    },
    /// A non-convex `MeshCollider`: the BVH34 mesh PhysX cooked (the mesh's
    /// `m_BakedTriangleCollisionMesh`) under the GameObject's transform: its
    /// `localToWorldMatrix`, and its world pose and lossy scale as the importer computes them;
    /// with `runtime`, the transform the game was recorded holding, which the queries use.
    Mesh {
        local_to_world: [f32; 16],
        pose: px::Transform,
        lossy_scale: [f32; 3],
        mesh: bv4::Bv4Mesh,
        runtime: Option<MeshRuntime>,
    },
    /// Another collider kind (capsule, sphere, character controller, convex mesh): a
    /// world-space box sure to enclose it, enough to fail closed.
    Other { class: String, min: [f32; 3], max: [f32; 3] },
}

/// A mesh collider as the game holds it, recorded by WrathOracle's `phys_probe` (`ph_col`):
/// Unity gives PhysX the GameObject's world position and rotation as the static actor's pose
/// (the shape's local pose identity) and its lossy scale as the geometry's `PxMeshScale`
/// (identity rotation), which every recorded probe of a mesh establishes (`docs/poc5/physics.md`).
/// The importer's own world transforms aren't Unity's to the bit (the lossy scale in
/// particular), so these are recorded inputs. The recording's JSON writes a zero without its
/// sign: a query whose answer depends on a zero component's sign fails closed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshRuntime {
    /// `Transform.position`, `.rotation` (`x, y, z, w`), `.lossyScale`.
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub lossy_scale: [f32; 3],
    /// The GameObject's layer at run time (the load moves `*_!Visual` objects to 11).
    pub layer: u32,
    /// The recording it came from, for reports.
    pub source: String,
}

/// A mesh or box collider as a recording names it (observer `phys_probe`'s `ph_col`), to
/// check a physics file's recorded transforms against another run's.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedCollider {
    /// `scene:path (MeshCollider)` or `(BoxCollider)`, as the importer names shapes.
    pub name: String,
    pub sibling_path: Vec<u32>,
    /// Active in the hierarchy and enabled.
    pub present: bool,
    pub layer: u32,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub lossy_scale: [f32; 3],
    pub trigger: bool,
    pub kind: RecordedKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordedKind {
    /// `MeshCollider.convex` and `.cookingOptions`.
    Mesh { convex: bool, cooking: i64 },
    /// `BoxCollider.center` and `.size` (a GameObject can have several boxes).
    Box { center: [f32; 3], size: [f32; 3] },
}

/// Bit-equal, zeros of either sign equal (the recording doesn't keep a zero's sign).
fn same_bits(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits() || (*x == 0.0 && *y == 0.0))
}

impl Shape {
    /// The recorded collider is this shape (name, sibling path, and a box's centre and size).
    fn is_recorded(&self, c: &RecordedCollider) -> bool {
        c.name == self.name
            && c.sibling_path == self.sibling_path
            && match (&self.kind, &c.kind) {
                (ShapeKind::Mesh { .. }, RecordedKind::Mesh { .. }) => true,
                (
                    ShapeKind::Box { center, size, .. },
                    RecordedKind::Box { center: rc, size: rs },
                ) => same_bits(center, rc) && same_bits(size, rs),
                _ => false,
            }
    }
}

impl World {
    /// The recording's mesh and box colliders against the shapes' recorded transforms: each
    /// shape with one is recorded exactly once, present, on its layer, with its transform
    /// (zeros' signs aside, which the recording doesn't keep) and trigger flag (a mesh
    /// non-convex with the baked mesh's cooking options); and every present mesh or box
    /// collider of the world's scenes is one of its shapes (the units' colliders, in other
    /// scenes, aren't modelled).
    pub fn check_recorded_colliders(&self, recorded: &[RecordedCollider]) -> Result<(), String> {
        for s in &self.shapes {
            if matches!(
                s.kind,
                ShapeKind::Mesh { runtime: Some(_), .. } | ShapeKind::Box { runtime: Some(_), .. }
            ) {
                let n = recorded.iter().filter(|c| s.is_recorded(c)).count();
                if n != 1 {
                    return Err(format!("{} {:?}: recorded {n} times", s.name, s.sibling_path));
                }
            }
        }
        let scene = |name: &str| name.split(':').next().unwrap_or("").to_owned();
        let scenes: std::collections::HashSet<String> =
            self.shapes.iter().map(|s| scene(&s.name)).collect();
        for c in recorded.iter().filter(|c| scenes.contains(&scene(&c.name))) {
            let shape = self.shapes.iter().find(|s| s.is_recorded(c));
            let differs = |layer: u32, position: &[f32], rotation: &[f32], lossy: &[f32]| {
                !c.present
                    || c.layer != layer
                    || !same_bits(&c.position, position)
                    || !same_bits(&c.rotation, rotation)
                    || !same_bits(&c.lossy_scale, lossy)
            };
            let bad = match shape.map(|s| (s, &s.kind)) {
                None => c.present.then(|| "not in the physics file".to_owned()),
                Some((s, ShapeKind::Mesh { runtime: Some(r), .. })) => {
                    (differs(r.layer, &r.position, &r.rotation, &r.lossy_scale)
                        || c.trigger != s.is_trigger
                        || c.kind != RecordedKind::Mesh { convex: false, cooking: 30 })
                    .then(|| {
                        format!("recorded {c:?}, the physics file's (from {}) {r:?}", r.source)
                    })
                }
                Some((s, ShapeKind::Box { runtime: Some(r), .. })) => {
                    (differs(r.layer, &r.position, &r.rotation, &r.lossy_scale)
                        || c.trigger != s.is_trigger)
                        .then(|| {
                            format!("recorded {c:?}, the physics file's (from {}) {r:?}", r.source)
                        })
                }
                _ => None,
            };
            if let Some(e) = bad {
                return Err(format!("{} {:?}: {e}", c.name, c.sibling_path));
            }
        }
        Ok(())
    }
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
                ShapeKind::Mesh { mesh, runtime, .. } => {
                    mesh.validate().with_context(|| s.name.clone())?;
                    if let Some(r) = runtime {
                        let all = r.position.iter().chain(&r.rotation).chain(&r.lossy_scale);
                        ensure!(
                            all.clone().all(|x| x.is_finite()),
                            "{}: a recorded transform that isn't finite",
                            s.name
                        );
                        ensure!(
                            r.lossy_scale.iter().all(|&x| x != 0.0),
                            "{}: a recorded scale of zero",
                            s.name
                        );
                        ensure!(r.layer < 32, "{}: recorded layer {}", s.name, r.layer);
                    }
                }
                ShapeKind::Box { chain, runtime, .. } => {
                    if let Some(r) = runtime {
                        let all = r
                            .world_center
                            .iter()
                            .chain(&r.rotation)
                            .chain(&r.lossy_scale)
                            .chain(&r.position);
                        ensure!(
                            all.clone().all(|x| x.is_finite()),
                            "{}: a recorded transform that isn't finite",
                            s.name
                        );
                        ensure!(
                            !chain.is_empty(),
                            "{}: a recorded box without its transforms",
                            s.name
                        );
                        ensure!(r.layer < 32, "{}: recorded layer {}", s.name, r.layer);
                    }
                }
                ShapeKind::Other { .. } => {}
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
