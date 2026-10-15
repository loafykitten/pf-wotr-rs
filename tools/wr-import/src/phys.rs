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
                let kind = match class {
                    "BoxCollider" => ShapeKind::Box {
                        local_to_world: m,
                        center: vec3(v, "m_Center")?,
                        size: vec3(v, "m_Size")?,
                    },
                    "MeshCollider" => mesh_shape(universe, s, v, m, &name, &mut rep)?,
                    "TerrainCollider" => terrain_shape(universe, s, v, &g, &name)?,
                    _ => {
                        let c = vec3(v, "m_Center")?;
                        let r = f32_of(v, "m_Radius")?;
                        let h = v.f32_at("m_Height").unwrap_or(0.0);
                        let e = r.max(h * 0.5).abs();
                        let (lo, hi) = world_bounds(&m, c, [e, e, e]);
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

fn world_bounds(m: &[f32; 16], c: [f32; 3], e: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
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
    (lo, hi)
}

fn mesh_shape(
    universe: &Universe,
    s: &SceneData,
    v: &Value,
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
        let (lo, hi) = world_bounds(&m, c, e);
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
    let same = cooked.vertices.iter().filter(|p| render_set.contains(&p.map(f32::to_bits))).count();
    let render_tris: u32 = render.submeshes.iter().map(|sm| sm.index_count / 3).sum();
    rep.meshes.push(MeshReport {
        mesh: mesh_name,
        version: cooked.version,
        midphase: cooked.midphase,
        cooked_vertices: cooked.vertices.len(),
        cooked_in_render: same,
        render_vertices: render_pos.len(),
        cooked_triangles: cooked.triangles.len(),
        render_triangles: render_tris as usize,
    });
    Ok(ShapeKind::Mesh {
        local_to_world: m,
        vertices: cooked.vertices,
        triangles: cooked.triangles,
    })
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

/// A PhysX 4.1 cooked triangle mesh: its vertices and triangles (the midphase, materials,
/// remap and adjacency that follow aren't needed here).
#[derive(Debug, Clone, PartialEq)]
pub struct CookedMesh {
    pub version: u32,
    pub midphase: u32,
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// Decodes the head of `Gu::MeshFactory`'s `loadMeshData` stream (little-endian only).
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
    }
    let mut r = Reader { b, at: 0 };
    let nxs = r.take(4)?;
    ensure!(&nxs[..3] == b"NXS", "not a PhysX stream");
    ensure!(nxs[3] & 1 == 1, "big-endian cooked data isn't supported");
    ensure!(r.take(4)? == b"MESH", "not a triangle mesh");
    let version = r.dword()?;
    ensure!(version >= 14, "mesh format version {version} (PhysX 4.1 writes 15)");
    let midphase = r.dword()?;
    ensure!(midphase <= 1, "unknown midphase {midphase}");
    let flags = r.dword()?;
    let nv = r.dword()? as usize;
    let nt = r.dword()? as usize;
    let mut take = |n: usize| r.take(n);
    let mut vertices = Vec::with_capacity(nv.min(b.len() / 12));
    for _ in 0..nv {
        let raw = take(12)?;
        let f = |k: usize| f32::from_le_bytes(raw[4 * k..4 * k + 4].try_into().expect("4 bytes"));
        vertices.push([f(0), f(1), f(2)]);
    }
    const IMSF_8BIT: u32 = 1 << 2;
    const IMSF_16BIT: u32 = 1 << 3;
    let isz = if flags & IMSF_8BIT != 0 {
        1
    } else if flags & IMSF_16BIT != 0 {
        2
    } else {
        4
    };
    let raw = take(nt.checked_mul(3 * isz).context("triangle count overflows")?)?;
    let idx = |i: usize| -> u32 {
        match isz {
            1 => u32::from(raw[i]),
            2 => u32::from(u16::from_le_bytes([raw[2 * i], raw[2 * i + 1]])),
            _ => u32::from_le_bytes(raw[4 * i..4 * i + 4].try_into().expect("4 bytes")),
        }
    };
    let triangles: Vec<[u32; 3]> =
        (0..nt).map(|t| [idx(3 * t), idx(3 * t + 1), idx(3 * t + 2)]).collect();
    ensure!(triangles.iter().flatten().all(|&i| (i as usize) < nv), "triangle index out of range");
    Ok(CookedMesh { version, midphase, vertices, triangles })
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
        b
    }

    #[test]
    fn cooked_meshes_decode_with_each_index_width() {
        let v = [[0.0, 1.0, 2.0], [3.0, 4.0, 5.0], [6.0, 7.0, 8.0]];
        for flags in [4, 8, 0] {
            let m = decode_cooked_mesh(&stream(flags, &v, &[0, 2, 1])).unwrap();
            assert_eq!((m.version, m.midphase), (15, 1));
            assert_eq!(m.vertices, v);
            assert_eq!(m.triangles, [[0, 2, 1]]);
        }
        assert!(decode_cooked_mesh(&stream(8, &v, &[0, 3, 1])).is_err(), "index out of range");
        let s = stream(8, &v, &[0, 2, 1]);
        assert!(decode_cooked_mesh(&s[..s.len() - 1]).is_err(), "truncated");
        let mut bad = s.clone();
        bad[4] = b'X';
        assert!(decode_cooked_mesh(&bad).is_err(), "not a mesh");
    }
}
