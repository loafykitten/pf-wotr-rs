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

/// Bone indices and weights of every vertex (four slots each; unused slots weigh zero).
fn skin_data(mesh: &DecodedMesh) -> Result<Vec<([u32; 4], [f32; 4])>> {
    let w = channel(mesh, BLEND_WEIGHTS).context("mesh has no blend weights")?;
    let i = channel(mesh, BLEND_INDICES).context("mesh has no blend indices")?;
    ensure!(w.dimension == 4 && i.dimension == 4, "skin channels must be four wide");
    let (wsz, isz) = (
        format_size(w.format).context("weight format")? as usize,
        format_size(i.format).context("index format")? as usize,
    );
    let mut out = Vec::with_capacity(mesh.vertex_count as usize);
    for v in 0..mesh.vertex_count as usize {
        let (wb, ib) = (
            attr(mesh, w, v).context("weights out of range")?,
            attr(mesh, i, v).context("indices out of range")?,
        );
        let mut idx = [0u32; 4];
        let mut wt = [0f32; 4];
        for k in 0..4 {
            wt[k] = read_component(w.format, &wb[k * wsz..]).context("weight")? as f32;
            idx[k] = read_component(i.format, &ib[k * isz..]).context("index")? as u32;
        }
        out.push((idx, wt));
    }
    Ok(out)
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

/// Poses `mesh` in place: `skin[i]` is `bone_i.localToWorld * bindpose_i`. Positions, normals
/// (renormalised) and tangent xyz are replaced; the weights and indices stay, so the mesh can
/// be posed again from its original streams.
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
    // Normals follow the inverse transpose, as a non-uniform bone scale needs.
    let normal_mats: Vec<Mat3> =
        skin.iter().map(|m| Mat3::from_mat4(*m).inverse().transpose()).collect();
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
            let (Some(m), Some(nm), Some(lm)) = (skin.get(b), normal_mats.get(b), linear.get(b))
            else {
                bail!("vertex {v}: bone index {b} past {} bones", skin.len());
            };
            sp += m.transform_point3(p) * w;
            if let Some(n) = n {
                sn += *nm * n * w;
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
