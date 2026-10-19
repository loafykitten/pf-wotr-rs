//! CPU skinning of a decoded Unity mesh: the bind pose (or any pose) baked into the vertex
//! streams.
//!
//! Unity skins `position`, `normal` and the tangent's xyz with up to four bones per vertex:
//! `v' = sum_i w_i * (bone_i * bindpose_i) * v`, where `bone_i` is the bone transform's
//! `localToWorldMatrix`. The mesh keeps the weights (channel 12) and bone indices (channel 13)
//! in its vertex streams; `position`, `normal` and `tangent` are float32 in these meshes.

use anyhow::{Context, Result, bail, ensure};
use glam::{Mat3, Mat4, Vec3};
use wr_assets::Channel;

use crate::pack::mesh::{DecodedMesh, format_size};

const POSITION: usize = 0;
const NORMAL: usize = 1;
const TANGENT: usize = 2;
const BLEND_WEIGHTS: usize = 12;
const BLEND_INDICES: usize = 13;

/// One component of a vertex attribute as a float or an integer, by Unity `VertexFormat`.
fn read_component(format: u8, b: &[u8]) -> Option<f64> {
    Some(match format {
        0 => f64::from(f32::from_le_bytes(b.get(..4)?.try_into().ok()?)),
        1 => f64::from(half_to_f32(u16::from_le_bytes(b.get(..2)?.try_into().ok()?))),
        2 => f64::from(*b.first()?) / 255.0,
        3 => (f64::from(*b.first()? as i8) / 127.0).max(-1.0),
        4 => f64::from(u16::from_le_bytes(b.get(..2)?.try_into().ok()?)) / 65535.0,
        5 => (f64::from(i16::from_le_bytes(b.get(..2)?.try_into().ok()?)) / 32767.0).max(-1.0),
        6 => f64::from(*b.first()?),
        7 => f64::from(*b.first()? as i8),
        8 => f64::from(u16::from_le_bytes(b.get(..2)?.try_into().ok()?)),
        9 => f64::from(i16::from_le_bytes(b.get(..2)?.try_into().ok()?)),
        10 => f64::from(u32::from_le_bytes(b.get(..4)?.try_into().ok()?)),
        11 => f64::from(i32::from_le_bytes(b.get(..4)?.try_into().ok()?)),
        _ => return None,
    })
}

fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1F);
    let man = f32::from(h & 0x3FF);
    sign * match exp {
        0 => man * 2f32.powi(-24),
        31 => {
            if man == 0.0 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => (1.0 + man / 1024.0) * 2f32.powi(exp - 15),
    }
}

fn channel(mesh: &DecodedMesh, i: usize) -> Option<Channel> {
    mesh.channels.get(i).copied().flatten()
}

/// The bytes of vertex `v`'s attribute `ch` in its stream.
fn attr(mesh: &DecodedMesh, ch: Channel, v: usize) -> Option<&[u8]> {
    let (data, stride) = mesh.streams.get(usize::from(ch.stream))?;
    let o = v * *stride as usize + usize::from(ch.offset);
    let len = (format_size(ch.format)? * u32::from(ch.dimension)) as usize;
    data.get(o..o + len)
}

/// The first `n` components of vertex `v`'s attribute `ch` as floats (the rest zero).
#[must_use]
pub fn read_attr(mesh: &DecodedMesh, ch: Channel, v: usize, n: usize) -> Option<[f32; 4]> {
    let b = attr(mesh, ch, v)?;
    let sz = format_size(ch.format)? as usize;
    let mut out = [0f32; 4];
    for (k, o) in out.iter_mut().enumerate().take(n.min(usize::from(ch.dimension))) {
        *o = read_component(ch.format, b.get(k * sz..)?)? as f32;
    }
    Some(out)
}

/// Bone indices and weights of every vertex (four slots each; unused slots weigh zero).
///
/// Unity writes only as many slots per vertex as the mesh needs: indices `uint32 x N` in channel 13
/// and, for N > 1, weights `float x N` in channel 12; a mesh bound one bone per vertex has no
/// weight channel at all and every weight is one.
pub fn skin_data(mesh: &DecodedMesh) -> Result<Vec<([u32; 4], [f32; 4])>> {
    let i = channel(mesh, BLEND_INDICES).context("mesh has no blend indices")?;
    let w = channel(mesh, BLEND_WEIGHTS);
    ensure!((1..=4).contains(&i.dimension), "{} bone slots per vertex", i.dimension);
    if let Some(w) = w {
        ensure!(
            w.dimension == i.dimension,
            "weights x{} for indices x{}",
            w.dimension,
            i.dimension
        );
    }
    let isz = format_size(i.format).context("index format")? as usize;
    let mut out = Vec::with_capacity(mesh.vertex_count as usize);
    for v in 0..mesh.vertex_count as usize {
        let ib = attr(mesh, i, v).context("indices out of range")?;
        let mut idx = [0u32; 4];
        let mut wt = [0f32; 4];
        for k in 0..usize::from(i.dimension) {
            idx[k] = read_component(i.format, &ib[k * isz..]).context("index")? as u32;
        }
        match w {
            Some(w) => {
                let wsz = format_size(w.format).context("weight format")? as usize;
                let wb = attr(mesh, w, v).context("weights out of range")?;
                for k in 0..usize::from(w.dimension) {
                    wt[k] = read_component(w.format, &wb[k * wsz..]).context("weight")? as f32;
                }
            }
            None => wt[0] = 1.0,
        }
        out.push((idx, wt));
    }
    Ok(out)
}

/// A mesh's skin in the pack's layout (`wr_assets::SKIN_STRIDE` bytes per vertex): four weights
/// (float32) then four bone indices (uint32). Weights are normalised to sum to one, as Unity's
/// skinning does; a vertex with no weight keeps all four at zero (it stays in the bind pose).
///
/// # Errors
///
/// A mesh without skin channels.
pub fn skin_blob(mesh: &DecodedMesh) -> Result<Vec<u8>> {
    Ok(skin_bytes(&skin_data(mesh)?))
}

/// [`skin_blob`] of per-vertex indices and weights already read.
#[must_use]
pub fn skin_bytes(data: &[([u32; 4], [f32; 4])]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 32);
    for (idx, wt) in data {
        let total: f32 = wt.iter().filter(|w| **w > 0.0).sum();
        for w in wt {
            let w = if total > 0.0 && *w > 0.0 { w / total } else { 0.0 };
            out.extend_from_slice(&w.to_le_bytes());
        }
        for i in idx {
            out.extend_from_slice(&i.to_le_bytes());
        }
    }
    out
}

/// Sets the mesh's bounds to cover every pose in `skins` (each a full set of skin matrices, as
/// for [`pose`]), then grows them by a tenth of their size and 0.1 m, so a renderer culled or
/// shadow-tested by them stays inside while it animates.
///
/// # Errors
///
/// As [`pose`] and [`refresh_bounds`].
pub fn animated_bounds(mesh: &mut DecodedMesh, skins: &[Vec<Mat4>]) -> Result<()> {
    type Box3 = ([f32; 3], [f32; 3]);
    let union = |a: Option<Box3>, b: Option<Box3>| match (a, b) {
        (Some((alo, ahi)), Some((blo, bhi))) => Some((
            [alo[0].min(blo[0]), alo[1].min(blo[1]), alo[2].min(blo[2])],
            [ahi[0].max(bhi[0]), ahi[1].max(bhi[1]), ahi[2].max(bhi[2])],
        )),
        (a, b) => a.or(b),
    };
    let mut aabb: Option<Box3> = None;
    let mut subs: Vec<Option<Box3>> = vec![None; mesh.submeshes.len()];
    for s in skins {
        let mut m = mesh.clone();
        pose(&mut m, s)?;
        refresh_bounds(&mut m)?;
        let (c, e) = m.aabb;
        let b = ([c[0] - e[0], c[1] - e[1], c[2] - e[2]], [c[0] + e[0], c[1] + e[1], c[2] + e[2]]);
        aabb = union(aabb, Some(b));
        for (acc, sm) in subs.iter_mut().zip(&m.submeshes) {
            *acc = union(*acc, sm.bounds);
        }
    }
    let grow = |b: Box3| {
        let mut b = b;
        for k in 0..3 {
            let pad = (b.1[k] - b.0[k]) * 0.1 + 0.1;
            b.0[k] -= pad;
            b.1[k] += pad;
        }
        b
    };
    if let Some(b) = aabb {
        let (lo, hi) = grow(b);
        mesh.aabb = (
            [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0],
            [(hi[0] - lo[0]) / 2.0, (hi[1] - lo[1]) / 2.0, (hi[2] - lo[2]) / 2.0],
        );
    }
    for (sm, b) in mesh.submeshes.iter_mut().zip(subs) {
        sm.bounds = b.map(grow);
    }
    Ok(())
}

/// The float32 channel `c` as `dim` floats per vertex starting at `offset`, or an error.
fn float_channel(mesh: &DecodedMesh, i: usize, dim: u8) -> Result<Channel> {
    let c = channel(mesh, i).with_context(|| format!("mesh has no channel {i}"))?;
    ensure!(c.format == 0 && c.dimension >= dim, "channel {i} is not float32x{dim}");
    Ok(c)
}

fn put(mesh: &mut DecodedMesh, ch: Channel, v: usize, values: &[f32]) -> Result<()> {
    let (data, stride) = mesh.streams.get_mut(usize::from(ch.stream)).context("stream")?;
    let o = v * *stride as usize + usize::from(ch.offset);
    for (k, x) in values.iter().enumerate() {
        data.get_mut(o + k * 4..o + k * 4 + 4)
            .context("vertex out of range")?
            .copy_from_slice(&x.to_le_bytes());
    }
    Ok(())
}

fn get3(mesh: &DecodedMesh, ch: Channel, v: usize) -> Result<Vec3> {
    let b = attr(mesh, ch, v).context("vertex out of range")?;
    let f = |k: usize| f32::from_le_bytes([b[k * 4], b[k * 4 + 1], b[k * 4 + 2], b[k * 4 + 3]]);
    Ok(Vec3::new(f(0), f(1), f(2)))
}

/// Recomputes `aabb` and every submesh's bounds from the mesh's (posed) float32 positions.
///
/// # Errors
///
/// A mesh without float32 positions or with an index past its vertices.
pub fn refresh_bounds(mesh: &mut DecodedMesh) -> Result<()> {
    let pos = float_channel(mesh, POSITION, 3)?;
    let n = mesh.vertex_count as usize;
    let mut points = Vec::with_capacity(n);
    for v in 0..n {
        points.push(get3(mesh, pos, v)?);
    }
    let bounds = |it: &mut dyn Iterator<Item = Vec3>| -> Option<([f32; 3], [f32; 3])> {
        let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        let mut any = false;
        for p in it {
            lo = lo.min(p);
            hi = hi.max(p);
            any = true;
        }
        any.then(|| (lo.to_array(), hi.to_array()))
    };
    if let Some((lo, hi)) = bounds(&mut points.iter().copied()) {
        let (lo, hi) = (Vec3::from_array(lo), Vec3::from_array(hi));
        mesh.aabb = (((lo + hi) * 0.5).to_array(), ((hi - lo) * 0.5).to_array());
    }
    let isz = if mesh.index_u32 { 4 } else { 2 };
    for sm in &mut mesh.submeshes {
        let mut used = Vec::new();
        for i in sm.first_index..sm.first_index + sm.index_count {
            let o = i as usize * isz;
            let raw = mesh.indices.get(o..o + isz).context("index out of range")?;
            let idx = if mesh.index_u32 {
                u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])
            } else {
                u32::from(u16::from_le_bytes([raw[0], raw[1]]))
            };
            let v = i64::from(sm.base_vertex) + i64::from(idx);
            used.push(*points.get(usize::try_from(v)?).context("index past the vertices")?);
        }
        sm.bounds = bounds(&mut used.into_iter());
    }
    Ok(())
}

/// Poses `mesh` in place: `skin[i]` is `bone_i.localToWorld * bindpose_i`. Positions, normals
/// (renormalised) and tangent xyz are replaced (bounds are not: see [`refresh_bounds`]); the weights
/// and indices stay in their channels.
///
/// # Errors
///
/// A mesh without float32 position/normal/tangent or without skin channels, or a bone index
/// past `skin`.
pub fn pose(mesh: &mut DecodedMesh, skin: &[Mat4]) -> Result<()> {
    let data = skin_data(mesh)?;
    let pos = float_channel(mesh, POSITION, 3)?;
    let nrm = float_channel(mesh, NORMAL, 3).ok();
    let tan = float_channel(mesh, TANGENT, 3).ok();
    // Unity skins normals and tangents with the bone matrix' linear part, not its inverse transpose.
    let linear: Vec<Mat3> = skin.iter().map(|m| Mat3::from_mat4(*m)).collect();
    for (v, (idx, wt)) in data.iter().enumerate() {
        let p = get3(mesh, pos, v)?;
        let n = nrm.map(|c| get3(mesh, c, v)).transpose()?;
        let t = tan.map(|c| get3(mesh, c, v)).transpose()?;
        let (mut sp, mut sn, mut st) = (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
        let mut total = 0.0;
        for k in 0..4 {
            let w = wt[k];
            if w <= 0.0 {
                continue;
            }
            let b = idx[k] as usize;
            let (Some(m), Some(lm)) = (skin.get(b), linear.get(b)) else {
                bail!("vertex {v}: bone index {b} past {} bones", skin.len());
            };
            sp += m.transform_point3(p) * w;
            if let Some(n) = n {
                sn += *lm * n * w;
            }
            if let Some(t) = t {
                st += *lm * t * w;
            }
            total += w;
        }
        // Unity normalises weights; a vertex with none stays in the bind pose.
        if total <= 0.0 {
            continue;
        }
        let inv = 1.0 / total;
        put(mesh, pos, v, &(sp * inv).to_array())?;
        if let Some(c) = nrm {
            put(mesh, c, v, &sn.normalize_or_zero().to_array())?;
        }
        if let Some(c) = tan {
            put(mesh, c, v, &st.normalize_or_zero().to_array())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wr_assets::SubMesh;

    /// Two vertices: position f32x3, normal f32x3, tangent f32x4 in stream 0; weights f32x4 and
    /// indices u32x4 in stream 1.
    fn mesh(weights: [[f32; 4]; 2], indices: [[u32; 4]; 2]) -> DecodedMesh {
        let verts = [([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), ([0.0, 2.0, 0.0], [0.0, 0.0, 1.0])];
        let mut s0 = Vec::new();
        for (p, n) in verts {
            for f in p.iter().chain(n.iter()).chain([1.0f32, 0.0, 0.0, 1.0].iter()) {
                s0.extend_from_slice(&f.to_le_bytes());
            }
        }
        let mut s1 = Vec::new();
        for v in 0..2 {
            for f in weights[v] {
                s1.extend_from_slice(&f.to_le_bytes());
            }
            for i in indices[v] {
                s1.extend_from_slice(&i.to_le_bytes());
            }
        }
        let mut channels = vec![None; 14];
        channels[0] = Some(Channel { stream: 0, offset: 0, format: 0, dimension: 3 });
        channels[1] = Some(Channel { stream: 0, offset: 12, format: 0, dimension: 3 });
        channels[2] = Some(Channel { stream: 0, offset: 24, format: 0, dimension: 4 });
        channels[12] = Some(Channel { stream: 1, offset: 0, format: 0, dimension: 4 });
        channels[13] = Some(Channel { stream: 1, offset: 16, format: 10, dimension: 4 });
        DecodedMesh {
            name: "t".into(),
            vertex_count: 2,
            channels,
            streams: vec![(s0, 40), (s1, 32)],
            index_u32: false,
            indices: Vec::new(),
            submeshes: vec![SubMesh::default()],
            aabb: Default::default(),
        }
    }

    fn pos(m: &DecodedMesh, v: usize) -> Vec3 {
        get3(m, channel(m, POSITION).unwrap(), v).unwrap()
    }

    #[test]
    fn identity_skin_keeps_the_bind_pose() {
        let mut m = mesh([[1.0, 0.0, 0.0, 0.0]; 2], [[0; 4]; 2]);
        pose(&mut m, &[Mat4::IDENTITY]).unwrap();
        assert_eq!(pos(&m, 0), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(pos(&m, 1), Vec3::new(0.0, 2.0, 0.0));
    }

    #[test]
    fn blended_bones_move_and_rotate_vertices() {
        // Vertex 0 follows bone 1 (a quarter turn about Z then a shift); vertex 1 is half bone 0
        // (a shift up) and half bone 1.
        let turn = Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let up = Mat4::from_translation(Vec3::new(0.0, 4.0, 0.0));
        let mut m =
            mesh([[1.0, 0.0, 0.0, 0.0], [0.5, 0.5, 0.0, 0.0]], [[1, 0, 0, 0], [0, 1, 0, 0]]);
        pose(&mut m, &[up, turn]).unwrap();
        let p0 = pos(&m, 0);
        assert!((p0 - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-5, "{p0:?}");
        let p1 = pos(&m, 1);
        // Half of (0,6,0) and half of (-2,0,0).
        assert!((p1 - Vec3::new(-1.0, 3.0, 0.0)).length() < 1e-5, "{p1:?}");
        // The normal of vertex 0 (+Y) turns to -X.
        let n0 = get3(&m, channel(&m, NORMAL).unwrap(), 0).unwrap();
        assert!((n0 - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-5, "{n0:?}");
    }

    #[test]
    fn rigid_and_two_bone_meshes_skin_too() {
        // One bone per vertex: indices only, weight one.
        let mut m = mesh([[1.0, 0.0, 0.0, 0.0]; 2], [[1, 0, 0, 0], [0, 0, 0, 0]]);
        m.channels[12] = None;
        m.channels[13] = Some(Channel { stream: 1, offset: 16, format: 10, dimension: 1 });
        let up = Mat4::from_translation(Vec3::new(0.0, 4.0, 0.0));
        pose(&mut m, &[Mat4::IDENTITY, up]).unwrap();
        assert_eq!(pos(&m, 0), Vec3::new(1.0, 4.0, 0.0));
        assert_eq!(pos(&m, 1), Vec3::new(0.0, 2.0, 0.0));
        // Two slots: weights x2 and indices x2.
        let mut two = mesh([[0.5, 0.5, 0.0, 0.0]; 2], [[0, 1, 0, 0]; 2]);
        two.channels[12] = Some(Channel { stream: 1, offset: 0, format: 0, dimension: 2 });
        two.channels[13] = Some(Channel { stream: 1, offset: 16, format: 10, dimension: 2 });
        pose(&mut two, &[Mat4::IDENTITY, up]).unwrap();
        assert_eq!(pos(&two, 0), Vec3::new(1.0, 2.0, 0.0));
    }

    #[test]
    fn bounds_follow_the_posed_positions() {
        let mut m = mesh([[1.0, 0.0, 0.0, 0.0]; 2], [[0; 4]; 2]);
        m.indices = [0u16, 1, 1].iter().flat_map(|i| i.to_le_bytes()).collect();
        m.submeshes[0].index_count = 3;
        pose(&mut m, &[Mat4::from_translation(Vec3::new(0.0, 10.0, 0.0))]).unwrap();
        refresh_bounds(&mut m).unwrap();
        assert_eq!(m.aabb, ([0.5, 11.0, 0.0], [0.5, 1.0, 0.0]));
        assert_eq!(m.submeshes[0].bounds, Some(([0.0, 10.0, 0.0], [1.0, 12.0, 0.0])));
    }

    #[test]
    fn normals_use_the_linear_part_under_non_uniform_scale() {
        let mut m = mesh([[1.0, 0.0, 0.0, 0.0]; 2], [[0; 4]; 2]);
        let nrm = channel(&m, NORMAL).unwrap();
        put(&mut m, nrm, 0, &[1.0, 1.0, 0.0]).unwrap();
        pose(&mut m, &[Mat4::from_scale(Vec3::new(2.0, 1.0, 1.0))]).unwrap();
        // Unity: (2, 1, 0) normalised, not the inverse transpose's (0.5, 1, 0).
        let n0 = get3(&m, nrm, 0).unwrap();
        assert!((n0 - Vec3::new(2.0, 1.0, 0.0).normalize()).length() < 1e-6, "{n0:?}");
    }

    #[test]
    fn a_bone_past_the_end_is_an_error() {
        let mut m = mesh([[1.0, 0.0, 0.0, 0.0]; 2], [[3, 0, 0, 0]; 2]);
        assert!(pose(&mut m, &[Mat4::IDENTITY]).is_err());
    }

    #[test]
    fn half_floats_and_normalised_formats_decode() {
        assert_eq!(read_component(1, &0x3C00u16.to_le_bytes()), Some(1.0));
        assert_eq!(read_component(2, &[255]), Some(1.0));
        assert_eq!(read_component(8, &7u16.to_le_bytes()), Some(7.0));
    }
}
