//! The area's colliders for the sim's physics queries: `wr-import phys-pack` turns the
//! scenes' collider components into `wr_phys::World` (`cache/sim/phys/<guid>.wrphys`).
//!
//! - `TerrainCollider`: the `TerrainData` heightmap as Unity hands it to PhysX: a height field
//!   with rows along x and columns along z (`m_Heights` is stored z-major, so it's transposed),
//!   every sample's tess flag set (the diagonal from (x, z) to (x+1, z+1)), materials 0, height
//!   scale `m_Scale.y / 32766`, row and column scales `m_Scale.x` and `m_Scale.z`, the actor at
//!   the origin (the only terrain pose the records establish; others are refused). Checked
//!   against every recorded query (`docs/poc5/physics.md`).
//! - `MeshCollider` (non-convex): the triangles PhysX cooked, decoded from the mesh's
//!   `m_BakedTriangleCollisionMesh` (PhysX's `NXS`/`MESH` stream), not the render mesh.
//! - `BoxCollider`: centre and size under the GameObject's `localToWorldMatrix`.
//! - Others (capsule, sphere, character controller, convex mesh): their world bounds.

use anyhow::{Context, Result, bail, ensure};
use wr_phys::hf::{HeightField, Sample};
use wr_phys::px::{Quat, Transform, Vec3};
use wr_phys::{Shape, ShapeKind, World};

use crate::area::Universe;
use crate::scene::SceneData;
use crate::unity::value::Value;

/// Unity's `kMaxHeight`: heightmap samples run 0..=32766.
const TERRAIN_MAX_HEIGHT: f32 = 32766.0;

/// What the build saw, for the report.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct PackReport {
    pub shapes: usize,
    pub by_kind: std::collections::BTreeMap<String, usize>,
    /// Colliders left out: disabled, or on an inactive GameObject.
    pub skipped_inactive: usize,
    /// Each cooked mesh against its render mesh.
    pub meshes: Vec<MeshReport>,
}

/// A cooked mesh against its render mesh.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MeshReport {
    pub mesh: String,
    /// PhysX's mesh format version and midphase (0 BVH33, 1 BVH34).
    pub version: u32,
    pub midphase: u32,
    pub cooked_vertices: usize,
    /// Cooked vertices equal, bit for bit, to some render vertex.
    pub cooked_in_render: usize,
    pub render_vertices: usize,
    pub cooked_triangles: usize,
    pub render_triangles: usize,
    /// The BV4 tree's packed nodes.
    pub tree_nodes: usize,
}

fn vec3(v: &Value, key: &str) -> Result<[f32; 3]> {
    v.get(key).and_then(Value::vec3).with_context(|| key.to_owned())
}

fn f32_of(v: &Value, key: &str) -> Result<f32> {
    v.f32_at(key).with_context(|| key.to_owned())
}

fn bool_of(v: &Value, key: &str) -> Option<bool> {
    v.get(key).and_then(Value::as_bool)
}

/// Builds the world from the area's scenes.
pub fn build(universe: &Universe, scenes: &[SceneData]) -> Result<(World, PackReport)> {
    let mut world = World::default();
    let mut rep = PackReport::default();
    for s in scenes {
        for g in s.game_objects() {
            for (class, v) in s.components_of(g.path_id) {
                let kind_name = match class {
                    "BoxCollider"
                    | "MeshCollider"
                    | "CapsuleCollider"
                    | "SphereCollider"
                    | "TerrainCollider"
                    | "CharacterController" => class,
                    _ => continue,
                };
                if !g.active_in_hierarchy || bool_of(v, "m_Enabled") == Some(false) {
                    rep.skipped_inactive += 1;
                    continue;
                }
                let name = format!("{}:{} ({kind_name})", s.scene, g.path);
                let layer = u32::try_from(g.layer).with_context(|| format!("{name}: layer"))?;
                ensure!(layer < 32, "{name}: layer {layer}");
                let is_trigger = bool_of(v, "m_IsTrigger").unwrap_or(false);
                let m = g.world.matrix;
                // A sheared transform (a rotated child of a non-uniformly scaled parent) makes the
                // matrix's shape differ from what Unity hands PhysX (a pose and a scale): refused.
                if class != "TerrainCollider" && !orthogonal(&m) {
                    bail!("{name}: a sheared transform (Unity's shape isn't established)");
                }
                let kind = match class {
                    "BoxCollider" => ShapeKind::Box {
                        local_to_world: m,
                        center: vec3(v, "m_Center")?,
                        size: vec3(v, "m_Size")?,
                    },
                    "MeshCollider" => mesh_shape(universe, s, v, &g, m, &name, &mut rep)?,
                    "TerrainCollider" => terrain_shape(universe, s, v, &g, &name)?,
                    _ => {
                        // Spheres, capsules and controllers: Unity scales the radius by the
                        // largest (transverse) scale, so a sphere of radius (r + h/2) times the
                        // largest column norm around the centre encloses them.
                        let c = vec3(v, "m_Center")?;
                        let r = f32_of(v, "m_Radius")?.abs();
                        let h = v.f32_at("m_Height").unwrap_or(0.0).abs();
                        let (lo, hi) = sphere_bounds(&m, c, r + h * 0.5);
                        ShapeKind::Other { class: class.to_owned(), min: lo, max: hi }
                    }
                };
                let kind_key = match &kind {
                    ShapeKind::HeightField { .. } => "height field".to_owned(),
                    ShapeKind::Box { .. } => "box".to_owned(),
                    ShapeKind::Mesh { .. } => "mesh".to_owned(),
                    ShapeKind::Other { class, .. } => class.clone(),
                };
                *rep.by_kind.entry(kind_key).or_default() += 1;
                world.shapes.push(Shape {
                    name,
                    sibling_path: g.sibling_path.clone(),
                    layer,
                    is_trigger,
                    visual_layer_move: g.name.contains("_!Visual"),
                    kind,
                });
            }
        }
    }
    rep.shapes = world.shapes.len();
    world.validate()?;
    Ok((world, rep))
}

/// The 3x3 part's columns are orthogonal (relative tolerance 1e-4).
fn orthogonal(m: &[f32; 16]) -> bool {
    let col = |j: usize| [0, 1, 2].map(|k| f64::from(m[4 * j + k]));
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let (c0, c1, c2) = (col(0), col(1), col(2));
    let n = [dot(c0, c0).sqrt(), dot(c1, c1).sqrt(), dot(c2, c2).sqrt()];
    [(c0, c1, 0, 1), (c0, c2, 0, 2), (c1, c2, 1, 2)]
        .iter()
        .all(|&(a, b, i, j)| dot(a, b).abs() <= 1e-4 * n[i] * n[j])
}

/// World bounds of the sphere of local radius `r` around the local point `c`, scaled by the
/// largest column norm, grown by 1 cm.
fn sphere_bounds(m: &[f32; 16], c: [f32; 3], r: f32) -> ([f32; 3], [f32; 3]) {
    let norm = |j: usize| (0..3).map(|k| f64::from(m[4 * j + k]).powi(2)).sum::<f64>().sqrt();
    let s = norm(0).max(norm(1)).max(norm(2));
    let radius = (f64::from(r) * s + 0.01) as f32;
    let (lo, hi) = world_bounds(m, c, [0.0; 3]);
    (lo.map(|x| x - radius), hi.map(|x| x + radius))
}

/// World bounds of the local box `c ± e` under `m`, in `f64`, grown by the game's `f32`
/// placement error (a millionth of the operands: the translation and each column times the
/// local size) and rounded outwards to `f32`.
fn world_bounds(m: &[f32; 16], c: [f32; 3], e: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let m = m.map(f64::from);
    let (c, e) = (c.map(f64::from), e.map(f64::from));
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for corner in 0..8 {
        let p = [
            c[0] + if corner & 1 == 0 { -e[0] } else { e[0] },
            c[1] + if corner & 2 == 0 { -e[1] } else { e[1] },
            c[2] + if corner & 4 == 0 { -e[2] } else { e[2] },
        ];
        for k in 0..3 {
            let w = m[k] * p[0] + m[4 + k] * p[1] + m[8 + k] * p[2] + m[12 + k];
            lo[k] = lo[k].min(w);
            hi[k] = hi[k].max(w);
        }
    }
    let norm = |j: usize| (0..3).map(|k| m[4 * j + k].powi(2)).sum::<f64>().sqrt();
    let local = (0..3).map(|k| c[k].abs() + e[k].abs()).fold(0.0, f64::max) * 3f64.sqrt();
    let err = 1e-6 * (norm(3) + (norm(0) + norm(1) + norm(2)) * local);
    let out = |x: f64, up: bool| {
        let v = x as f32;
        let pad = v.abs() * 2.4e-7 + 1e-6;
        if up { v + pad } else { v - pad }
    };
    (lo.map(|x| out(x - err, false)), hi.map(|x| out(x + err, true)))
}

fn mesh_shape(
    universe: &Universe,
    s: &SceneData,
    v: &Value,
    g: &crate::scene::GoSnapshot,
    m: [f32; 16],
    name: &str,
    rep: &mut PackReport,
) -> Result<ShapeKind> {
    let p = v.get("m_Mesh").and_then(Value::as_pptr).context("m_Mesh")?;
    let o = universe.resolve(s.file, p)?.with_context(|| format!("{name}: mesh not found"))?;
    let mv = o.read()?;
    let mesh_name = mv.str_at("m_Name").unwrap_or_default().to_owned();
    if bool_of(v, "m_Convex") == Some(true) {
        let c = mv.path("m_LocalAABB.m_Center").and_then(Value::vec3).unwrap_or_default();
        let e = mv.path("m_LocalAABB.m_Extent").and_then(Value::vec3).unwrap_or_default();
        // Grown by 1% and 1 cm (cooking may inflate the hull slightly).
        let (lo, hi) = world_bounds(&m, c, e.map(|x| x.abs() * 1.01 + 0.01));
        return Ok(ShapeKind::Other { class: "MeshCollider (convex)".into(), min: lo, max: hi });
    }
    let baked = mv
        .get("m_BakedTriangleCollisionMesh")
        .and_then(Value::as_bytes)
        .filter(|b| !b.is_empty())
        .with_context(|| {
            format!("{name}: mesh {mesh_name} has no baked collision mesh (cooking isn't modelled)")
        })?;
    let cooked = decode_cooked_mesh(baked).with_context(|| format!("{name}: {mesh_name}"))?;
    // Compare with the render mesh, for the report.
    let render = crate::pack::mesh::decode(universe, &mv)?;
    let render_pos = positions(&render);
    let render_set: std::collections::HashSet<[u32; 3]> =
        render_pos.iter().map(|p| p.map(f32::to_bits)).collect();
    let mesh = cooked.mesh;
    let same = mesh.vertices.iter().filter(|p| render_set.contains(&p.map(f32::to_bits))).count();
    let render_tris: u32 = render.submeshes.iter().map(|sm| sm.index_count / 3).sum();
    // `mGeomEpsilon` as cooking computes it from the local bounds (2^-22 of the largest absolute
    // coordinate), against the cooked value the raycast uses.
    let extent = mesh.vertices.iter().flatten().fold(0.0f32, |a, &x| a.max(x.abs()));
    ensure!(
        mesh.geom_epsilon.to_bits() == (extent * 2f32.powi(-22)).to_bits(),
        "{name}: cooked mGeomEpsilon {} isn't 2^-22 of the largest coordinate {extent}",
        mesh.geom_epsilon
    );
    rep.meshes.push(MeshReport {
        mesh: mesh_name,
        version: cooked.version,
        midphase: cooked.midphase,
        cooked_vertices: mesh.vertices.len(),
        cooked_in_render: same,
        render_vertices: render_pos.len(),
        cooked_triangles: mesh.triangles.len(),
        render_triangles: render_tris as usize,
        tree_nodes: mesh.tree.nodes.len(),
    });
    let w = g.world;
    Ok(ShapeKind::Mesh {
        local_to_world: m,
        pose: Transform {
            q: Quat { x: w.rotation[0], y: w.rotation[1], z: w.rotation[2], w: w.rotation[3] },
            p: Vec3::from_array(w.position),
        },
        lossy_scale: w.lossy_scale,
        mesh,
        runtime: None,
    })
}

/// What [`apply_recorded_colliders`] found.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ColliderReport {
    /// Mesh shapes given their recorded transform.
    pub meshes: usize,
    /// Of those, the importer's own position, rotation (zeros' signs aside) and lossy scale
    /// equal to the recording's, bit for bit.
    pub position_equal: usize,
    pub rotation_equal: usize,
    pub lossy_equal: usize,
    /// Mesh shapes left out: recorded disabled.
    pub disabled: Vec<String>,
    /// Mesh shapes recorded with a zero scale, left without a recorded transform.
    pub degenerate: Vec<String>,
}

/// Gives every mesh shape the transform and layer a recording's `ph_col` records hold for it
/// (WrathOracle's `phys_probe`): one record per shape, matched by scene, hierarchy path,
/// sibling path and type, active, not a trigger, non-convex, with the importer's cooking
/// options, and close to the importer's transform (the identity check; the values used are
/// the recording's). A mesh recorded disabled is left out of the world.
pub fn apply_recorded_colliders(
    world: &mut World,
    trace: &std::path::Path,
    source: &str,
) -> Result<ColliderReport> {
    use std::io::BufRead;
    let f = std::fs::File::open(trace).with_context(|| format!("opening {}", trace.display()))?;
    let reader: Box<dyn BufRead> = if trace.extension().is_some_and(|e| e == "gz") {
        Box::new(std::io::BufReader::new(flate2::read::GzDecoder::new(f)))
    } else {
        Box::new(std::io::BufReader::new(f))
    };
    let mut cols: Vec<serde_json::Value> = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if !line.contains("\"ph_col\"") {
            continue;
        }
        let r: serde_json::Value = serde_json::from_str(&line)?;
        if r["k"] == "obs" && r["ev"] == "ph_col" {
            cols.push(r);
        }
    }
    ensure!(!cols.is_empty(), "{}: no ph_col records (observer phys_probe)", trace.display());
    // Floats are written round-trip ("R"): the nearest f32 is the game's value.
    let fv = |v: &serde_json::Value, what: &str| -> Result<f32> {
        let x = v.as_f64().with_context(|| format!("{what}: not a number"))?;
        #[allow(clippy::cast_possible_truncation)]
        let y = x as f32;
        ensure!(y.is_finite(), "{what}: {x} isn't a finite f32");
        Ok(y)
    };
    let arr = |v: &serde_json::Value, n: usize, what: &str| -> Result<Vec<f32>> {
        let a = v.as_array().with_context(|| format!("{what}: not an array"))?;
        ensure!(a.len() == n, "{what}: {} values", a.len());
        a.iter().map(|x| fv(x, what)).collect()
    };
    let mut rep = ColliderReport::default();
    let mut dropped = Vec::new();
    for (i, s) in world.shapes.iter_mut().enumerate() {
        let ShapeKind::Mesh { pose, lossy_scale, runtime, .. } = &mut s.kind else { continue };
        let found: Vec<&serde_json::Value> = cols
            .iter()
            .filter(|c| {
                c["type"] == "MeshCollider"
                    && format!(
                        "{}:{} (MeshCollider)",
                        c["scene"].as_str().unwrap_or(""),
                        c["path"].as_str().unwrap_or("")
                    ) == s.name
                    && c["sib"].as_array().is_some_and(|a| {
                        a.iter().map(serde_json::Value::as_u64).collect::<Vec<_>>()
                            == s.sibling_path
                                .iter()
                                .map(|&x| Some(u64::from(x)))
                                .collect::<Vec<_>>()
                    })
            })
            .collect();
        let [c] = found.as_slice() else {
            bail!("{}: {} ph_col records match it", s.name, found.len());
        };
        let name = &s.name;
        ensure!(c["active"] == true, "{name}: recorded inactive");
        if c["enabled"] == false {
            // Disabled at load (a loot corpse's collider): not in PhysX's scene.
            rep.disabled.push(name.clone());
            dropped.push(i);
            continue;
        }
        ensure!(c["enabled"] == true, "{name}: no enabled flag");
        ensure!(
            c["trigger"].as_bool() == Some(s.is_trigger),
            "{name}: recorded trigger flag differs"
        );
        ensure!(c["convex"] == false, "{name}: recorded convex");
        ensure!(c["cooking"] == 30, "{name}: recorded cooking options {}", c["cooking"]);
        let layer = u32::try_from(c["layer"].as_u64().context("layer")?)?;
        ensure!(
            layer == s.layer || (s.visual_layer_move && layer == 11),
            "{name}: recorded on layer {layer}, the scene says {}",
            s.layer
        );
        let p = arr(&c["pos"], 3, "pos")?;
        let q = arr(&c["rot"], 4, "rot")?;
        let l = arr(&c["lossy"], 3, "lossy")?;
        let ours = [pose.p.x, pose.p.y, pose.p.z];
        let ourq = [pose.q.x, pose.q.y, pose.q.z, pose.q.w];
        let close = |a: &[f32], b: &[f32]| {
            a.iter().zip(b).all(|(x, y)| (x - y).abs() <= 1e-4 * (1.0 + x.abs()))
        };
        ensure!(
            close(&p, &ours) && close(&q, &ourq) && close(&l, lossy_scale),
            "{name}: recorded transform {p:?} {q:?} {l:?} isn't the scene's {ours:?} {ourq:?} {lossy_scale:?}"
        );
        // Zeros compare equal whatever their sign (the recording doesn't keep it).
        let same = |a: &[f32], b: &[f32]| {
            a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits() || (*x == 0.0 && *y == 0.0))
        };
        if l.contains(&0.0) {
            // A collapsed scale (a loot corpse's collider): not a valid PhysX geometry; the
            // conservative test answers for it.
            rep.degenerate.push(name.clone());
            continue;
        }
        rep.meshes += 1;
        rep.position_equal += usize::from(same(&p, &ours));
        rep.rotation_equal += usize::from(same(&q, &ourq));
        rep.lossy_equal += usize::from(same(&l, lossy_scale));
        *runtime = Some(wr_phys::MeshRuntime {
            position: [p[0], p[1], p[2]],
            rotation: [q[0], q[1], q[2], q[3]],
            lossy_scale: [l[0], l[1], l[2]],
            layer,
            source: source.to_owned(),
        });
    }
    let mut idx = 0;
    world.shapes.retain(|_| {
        idx += 1;
        !dropped.contains(&(idx - 1))
    });
    world.validate()?;
    Ok(rep)
}

fn positions(m: &crate::pack::mesh::DecodedMesh) -> Vec<[f32; 3]> {
    let Some(ch) = m.channels.first().copied().flatten() else { return Vec::new() };
    if ch.format != 0 || ch.dimension < 3 {
        return Vec::new();
    }
    let (data, stride) = &m.streams[usize::from(ch.stream)];
    (0..m.vertex_count as usize)
        .map(|i| {
            let b = i * *stride as usize + usize::from(ch.offset);
            let f =
                |k: usize| f32::from_le_bytes(data[b + 4 * k..b + 4 * k + 4].try_into().unwrap());
            [f(0), f(1), f(2)]
        })
        .collect()
}

/// A PhysX 4.1 cooked triangle mesh as `Gu::MeshFactory`'s `loadMeshData` reads it: the
/// header and the BVH34 mesh the raycast needs (vertices, triangles in the tree's order,
/// `mGeomEpsilon`, the tree); materials, the face remap, adjacencies and the extra triangle
/// data are read past (no raycast distance or point depends on them).
#[derive(Debug, Clone, PartialEq)]
pub struct CookedMesh {
    pub version: u32,
    pub midphase: u32,
    pub mesh: wr_phys::bv4::Bv4Mesh,
    /// The cooked local bounds (`mAABB`).
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
}

/// Decodes a little-endian `NXS`/`MESH` stream with the BVH34 midphase (the BVH33 R-tree
/// isn't ported) and checks it whole, as `loadMeshData` and `BV4Tree::load` read it.
pub fn decode_cooked_mesh(b: &[u8]) -> Result<CookedMesh> {
    struct Reader<'a> {
        b: &'a [u8],
        at: usize,
    }
    impl<'a> Reader<'a> {
        fn take(&mut self, n: usize) -> Result<&'a [u8]> {
            let end = self.at.checked_add(n).context("cooked mesh size overflows")?;
            let s = self.b.get(self.at..end).context("cooked mesh truncated")?;
            self.at = end;
            Ok(s)
        }
        fn dword(&mut self) -> Result<u32> {
            Ok(u32::from_le_bytes(self.take(4)?.try_into().expect("4 bytes")))
        }
        fn float(&mut self) -> Result<f32> {
            Ok(f32::from_bits(self.dword()?))
        }
        fn vec3(&mut self) -> Result<[f32; 3]> {
            Ok([self.float()?, self.float()?, self.float()?])
        }
        fn count(&mut self, n: usize, size: usize) -> Result<&'a [u8]> {
            self.take(n.checked_mul(size).context("cooked mesh size overflows")?)
        }
    }
    const IMSF_MATERIALS: u32 = 1 << 0;
    const IMSF_FACE_REMAP: u32 = 1 << 1;
    const IMSF_8BIT: u32 = 1 << 2;
    const IMSF_16BIT: u32 = 1 << 3;
    const IMSF_ADJACENCIES: u32 = 1 << 4;
    const IMSF_GRB_DATA: u32 = 1 << 5;
    let mut r = Reader { b, at: 0 };
    let nxs = r.take(4)?;
    ensure!(&nxs[..3] == b"NXS", "not a PhysX stream");
    ensure!(nxs[3] & 1 == 1, "big-endian cooked data isn't supported");
    ensure!(r.take(4)? == b"MESH", "not a triangle mesh");
    let version = r.dword()?;
    ensure!(version == 15, "mesh format version {version} (PhysX 4.1 writes 15)");
    let midphase = r.dword()?;
    ensure!(midphase == 1, "midphase {midphase} (only BVH34 is ported)");
    let flags = r.dword()?;
    ensure!(flags & !0x3f == 0, "unknown serialization flags {flags:#x}");
    ensure!(flags & IMSF_GRB_DATA == 0, "GPU data in the cooked mesh isn't read");
    let nv = r.dword()? as usize;
    let nt = r.dword()? as usize;
    let mut vertices = Vec::with_capacity(nv.min(b.len() / 12));
    for _ in 0..nv {
        vertices.push(r.vec3()?);
    }
    let isz = if flags & IMSF_8BIT != 0 {
        1
    } else if flags & IMSF_16BIT != 0 {
        2
    } else {
        4
    };
    let raw = r.count(nt, 3 * isz)?;
    let idx = |i: usize| -> u32 {
        match isz {
            1 => u32::from(raw[i]),
            2 => u32::from(u16::from_le_bytes([raw[2 * i], raw[2 * i + 1]])),
            _ => u32::from_le_bytes(raw[4 * i..4 * i + 4].try_into().expect("4 bytes")),
        }
    };
    let triangles: Vec<[u32; 3]> =
        (0..nt).map(|t| [idx(3 * t), idx(3 * t + 1), idx(3 * t + 2)]).collect();
    if flags & IMSF_MATERIALS != 0 {
        r.count(nt, 2)?;
    }
    if flags & IMSF_FACE_REMAP != 0 {
        // `readIndices(maxIndex, ...)`: bytes, words or dwords by the largest index.
        let max = r.dword()?;
        let width = if max <= 0xff {
            1
        } else if max <= 0xffff {
            2
        } else {
            4
        };
        r.count(nt, width)?;
    }
    if flags & IMSF_ADJACENCIES != 0 {
        r.count(nt, 12)?;
    }
    // `BV4Tree::load`.
    ensure!(r.take(4)? == b"BV4 ", "no BV4 tree");
    let tree_version = r.dword()?;
    ensure!(tree_version == 3, "BV4 tree version {tree_version} (PhysX 4.1 writes 3)");
    let center = r.vec3()?;
    let extents_magnitude = r.float()?;
    let init_data = r.dword()?;
    let min_coeff = r.vec3()?;
    let max_coeff = r.vec3()?;
    let quantized = r.dword()?;
    ensure!(quantized == 1, "a non-quantized BV4 tree (the player reads quantized nodes only)");
    let nb_nodes = r.dword()? as usize;
    let raw = r.count(nb_nodes, 16)?;
    let word = |i: usize| u32::from_le_bytes(raw[4 * i..4 * i + 4].try_into().expect("4 bytes"));
    let nodes: Vec<[u32; 4]> = (0..nb_nodes)
        .map(|n| [word(4 * n), word(4 * n + 1), word(4 * n + 2), word(4 * n + 3)])
        .collect();
    let geom_epsilon = r.float()?;
    let aabb_min = r.vec3()?;
    let aabb_max = r.vec3()?;
    let extra = r.dword()? as usize;
    ensure!(extra == 0 || extra == nt, "{extra} extra triangle bytes for {nt} triangles");
    r.count(extra, 1)?;
    ensure!(r.at == b.len(), "{} bytes after the cooked mesh", b.len() - r.at);
    let mesh = wr_phys::bv4::Bv4Mesh {
        vertices,
        triangles,
        geom_epsilon,
        tree: wr_phys::bv4::Bv4Tree {
            center,
            extents_magnitude,
            init_data,
            min_coeff,
            max_coeff,
            nodes,
        },
    };
    mesh.validate()?;
    Ok(CookedMesh { version, midphase, mesh, aabb_min, aabb_max })
}

fn terrain_shape(
    universe: &Universe,
    s: &SceneData,
    v: &Value,
    g: &crate::scene::GoSnapshot,
    name: &str,
) -> Result<ShapeKind> {
    let p = v.get("m_TerrainData").and_then(Value::as_pptr).context("m_TerrainData")?;
    let o = universe.resolve(s.file, p)?.with_context(|| format!("{name}: terrain data"))?;
    let td = o.read()?;
    let hm = td.get("m_Heightmap").context("m_Heightmap")?;
    let res = usize::try_from(hm.i64_at("m_Resolution").context("m_Resolution")?)?;
    ensure!(res >= 2, "{name}: heightmap resolution {res}");
    let scale = hm.get("m_Scale").and_then(Value::vec3).context("m_Scale")?;
    let heights = hm.get("m_Heights").and_then(Value::as_array).context("m_Heights")?;
    ensure!(heights.len() == res * res, "{name}: {} heights for {res}²", heights.len());
    let holes = match hm.get("m_Holes") {
        Some(Value::Bytes(b)) => b.len(),
        Some(Value::Array(a)) => a.len(),
        _ => 0,
    };
    // Tree colliders: Unity adds the tree prototypes' colliders as shapes of the terrain's actor
    // (not scene components), so a terrain with trees and tree colliders on is refused.
    let trees = td
        .path("m_DetailDatabase.m_TreeInstances")
        .and_then(Value::as_array)
        .map(<[Value]>::len)
        .with_context(|| format!("{name}: no m_DetailDatabase.m_TreeInstances"))?;
    if trees != 0 && bool_of(v, "m_EnableTreeColliders") != Some(false) {
        bail!("{name}: {trees} trees with tree colliders on (their shapes aren't imported)");
    }
    if holes != 0 {
        bail!("{name}: the terrain has a holes map (hole materials aren't modelled)");
    }
    let w = g.world;
    // Only a terrain at the origin, unrotated and unscaled, is established by the records (the
    // pose Unity gives the height field's actor elsewhere isn't).
    if w.position != [0.0; 3] || w.rotation != [0.0, 0.0, 0.0, 1.0] || w.lossy_scale != [1.0; 3] {
        bail!(
            "{name}: a terrain at {:?}, rotation {:?}, scale {:?} (only the origin is established)",
            w.position,
            w.rotation,
            w.lossy_scale
        );
    }
    // PhysX rows run along x, columns along z; `m_Heights` is [z][x].
    let mut samples = Vec::with_capacity(res * res);
    for x in 0..res {
        for z in 0..res {
            let h = heights[z * res + x].as_i64().context("height")?;
            let height = i16::try_from(h).with_context(|| format!("{name}: height {h}"))?;
            samples.push(Sample { height, material0: 0x80, material1: 0 });
        }
    }
    let n = u32::try_from(res)?;
    // The records establish the value, not Unity's expression for it: `y / 32766` and
    // `y * (1 / 32766)` agree for this terrain; where they don't, which one Unity uses is open.
    let height_scale = scale[1] / TERRAIN_MAX_HEIGHT;
    if height_scale.to_bits() != (scale[1] * (1.0 / TERRAIN_MAX_HEIGHT)).to_bits() {
        bail!("{name}: the height scale's two candidate expressions differ (not established)");
    }
    Ok(ShapeKind::HeightField {
        pose: Transform { q: Quat::IDENTITY, p: Vec3::from_array(w.position) },
        field: HeightField {
            rows: n,
            columns: n,
            samples,
            height_scale,
            row_scale: scale[0],
            column_scale: scale[2],
            double_sided: false,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(flags: u32, verts: &[[f32; 3]], idx: &[u32]) -> Vec<u8> {
        let mut b = b"NXS\x01MESH".to_vec();
        for d in [15u32, 1, flags, verts.len() as u32, (idx.len() / 3) as u32] {
            b.extend_from_slice(&d.to_le_bytes());
        }
        for v in verts.iter().flatten() {
            b.extend_from_slice(&v.to_le_bytes());
        }
        for &i in idx {
            match flags & 0b1100 {
                4 => b.push(i as u8),
                8 => b.extend_from_slice(&(i as u16).to_le_bytes()),
                _ => b.extend_from_slice(&i.to_le_bytes()),
            }
        }
        let nt = idx.len() / 3;
        if flags & 1 != 0 {
            b.extend(std::iter::repeat_n(0u8, 2 * nt)); // materials
        }
        if flags & 2 != 0 {
            b.extend_from_slice(&300u32.to_le_bytes()); // remap, as words
            b.extend(std::iter::repeat_n(0u8, 2 * nt));
        }
        if flags & 16 != 0 {
            b.extend(std::iter::repeat_n(0xffu8, 12 * nt)); // adjacencies
        }
        b.extend_from_slice(b"BV4 ");
        b.extend_from_slice(&3u32.to_le_bytes());
        for x in [1.0f32, 2.0, 3.0, 4.0] {
            b.extend_from_slice(&x.to_le_bytes());
        }
        b.extend_from_slice(&0u32.to_le_bytes()); // init data
        for x in [0.5f32; 6] {
            b.extend_from_slice(&x.to_le_bytes());
        }
        b.extend_from_slice(&1u32.to_le_bytes()); // quantized
        b.extend_from_slice(&0u32.to_le_bytes()); // no nodes: brute force
        let eps = 8.0f32 * 2f32.powi(-22);
        for x in [eps, 0.0, 1.0, 2.0, 6.0, 7.0, 8.0] {
            b.extend_from_slice(&x.to_le_bytes());
        }
        b.extend_from_slice(&0u32.to_le_bytes()); // extra triangle data
        b
    }

    #[test]
    fn cooked_meshes_decode_with_each_index_width() {
        let v = [[0.0, 1.0, 2.0], [3.0, 4.0, 5.0], [6.0, 7.0, 8.0]];
        for flags in [4, 8, 0, 4 | 1 | 2 | 16] {
            let m = decode_cooked_mesh(&stream(flags, &v, &[0, 2, 1])).unwrap();
            assert_eq!((m.version, m.midphase), (15, 1));
            assert_eq!(m.mesh.vertices, v);
            assert_eq!(m.mesh.triangles, [[0, 2, 1]]);
            assert_eq!(m.mesh.tree.center, [1.0, 2.0, 3.0]);
            assert_eq!(m.mesh.geom_epsilon, 8.0 * 2f32.powi(-22));
            assert_eq!((m.aabb_min, m.aabb_max), ([0.0, 1.0, 2.0], [6.0, 7.0, 8.0]));
        }
        assert!(decode_cooked_mesh(&stream(8, &v, &[0, 3, 1])).is_err(), "index out of range");
        let s = stream(8, &v, &[0, 2, 1]);
        assert!(decode_cooked_mesh(&s[..s.len() - 1]).is_err(), "truncated");
        assert!(decode_cooked_mesh(&[s.as_slice(), &[0]].concat()).is_err(), "trailing byte");
        let mut bad = s.clone();
        bad[4] = b'X';
        assert!(decode_cooked_mesh(&bad).is_err(), "not a mesh");
        let mut bvh33 = s.clone();
        bvh33[12] = 0;
        assert!(decode_cooked_mesh(&bvh33).is_err(), "the BVH33 midphase isn't ported");
    }
}
