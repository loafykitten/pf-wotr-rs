//! Unity 2020.3 `Mesh` objects: vertex streams and index buffer, copied byte for byte.
//!
//! Vertex data is one blob (inline `m_VertexData.m_DataSize`, or a `.resS` slice named
//! by `m_StreamData`) holding the streams back to back, each starting on a 16-byte
//! boundary; a stream's stride is the sum of its channels' sizes. Channels follow the
//! 2019+ layout (14 channels, `VertexFormat` enum).

use anyhow::{Context, Result, bail, ensure};
use wr_assets::{Channel, SubMesh};

use crate::area::Universe;
use crate::unity::value::Value;

/// Bytes per component of a Unity `VertexFormat`.
#[must_use]
pub fn format_size(format: u8) -> Option<u32> {
    Some(match format {
        0 | 10 | 11 => 4,
        1 | 4 | 5 | 8 | 9 => 2,
        2 | 3 | 6 | 7 => 1,
        _ => return None,
    })
}

#[derive(Debug, Clone)]
pub struct DecodedMesh {
    pub name: String,
    pub vertex_count: u32,
    pub channels: Vec<Option<Channel>>,
    /// (bytes, stride) per stream.
    pub streams: Vec<(Vec<u8>, u32)>,
    pub index_u32: bool,
    pub indices: Vec<u8>,
    pub submeshes: Vec<SubMesh>,
    pub aabb: ([f32; 3], [f32; 3]),
}

fn u64_of(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::UInt(u)) => *u,
        Some(Value::Int(i)) => u64::try_from(*i).unwrap_or(0),
        _ => 0,
    }
}

/// Splits Unity's interleaved vertex blob into streams.
pub fn split_streams(
    data: &[u8],
    vertex_count: u32,
    channels: &[Option<Channel>],
) -> Result<Vec<(Vec<u8>, u32)>> {
    let nstreams = channels.iter().flatten().map(|c| usize::from(c.stream) + 1).max().unwrap_or(0);
    let mut out = Vec::new();
    let mut offset = 0usize;
    for s in 0..nstreams {
        let stride: u32 = channels
            .iter()
            .flatten()
            .filter(|c| usize::from(c.stream) == s)
            .map(|c| format_size(c.format).unwrap_or(0) * u32::from(c.dimension))
            .sum();
        let len = stride as usize * vertex_count as usize;
        ensure!(offset + len <= data.len(), "stream {s} runs past the vertex data");
        out.push((data[offset..offset + len].to_vec(), stride));
        offset = (offset + len).div_ceil(16) * 16;
    }
    Ok(out)
}

/// Decodes one half-precision float.
fn f16_to_f32(h: u16) -> f32 {
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

/// Bounds of the positions a submesh's indices reference (float32 or float16 positions;
/// `None` for other formats or empty submeshes).
fn submesh_bounds(
    channels: &[Option<Channel>],
    streams: &[(Vec<u8>, u32)],
    indices: &[u8],
    index_u32: bool,
    vertex_count: u32,
    sm: &SubMesh,
) -> Option<([f32; 3], [f32; 3])> {
    let ch = channels.first().copied().flatten()?;
    let (data, stride) = streams.get(usize::from(ch.stream))?;
    let stride = *stride as usize;
    let (comp, read): (usize, fn(&[u8]) -> f32) = match (ch.format, ch.dimension) {
        (0, 3 | 4) => (4, |b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        (1, 3 | 4) => (2, |b| f16_to_f32(u16::from_le_bytes([b[0], b[1]]))),
        _ => return None,
    };
    let isz = if index_u32 { 4 } else { 2 };
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for i in sm.first_index..sm.first_index + sm.index_count {
        let o = i as usize * isz;
        let raw = indices.get(o..o + isz)?;
        let idx = if index_u32 {
            u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])
        } else {
            u32::from(u16::from_le_bytes([raw[0], raw[1]]))
        };
        let v = i64::from(sm.base_vertex) + i64::from(idx);
        if v < 0 || v >= i64::from(vertex_count) {
            return None;
        }
        let base = v as usize * stride + usize::from(ch.offset);
        for k in 0..3 {
            let p = read(data.get(base + k * comp..base + (k + 1) * comp)?);
            lo[k] = lo[k].min(p);
            hi[k] = hi[k].max(p);
        }
    }
    (lo[0] <= hi[0]).then_some((lo, hi))
}

pub fn decode(universe: &Universe, v: &Value) -> Result<DecodedMesh> {
    let name = v.str_at("m_Name").unwrap_or_default().to_owned();
    if v.i64_at("m_MeshCompression").unwrap_or(0) != 0 {
        bail!("{name}: compressed meshes not supported");
    }
    let vd = v.get("m_VertexData").context("no m_VertexData")?;
    let vertex_count = u32::try_from(vd.i64_at("m_VertexCount").unwrap_or(0))?;
    let mut channels = Vec::new();
    for c in vd.get("m_Channels").and_then(Value::as_array).unwrap_or_default() {
        let dim = c.i64_at("dimension").unwrap_or(0) as u8 & 0x0F;
        channels.push((dim > 0).then(|| Channel {
            stream: c.i64_at("stream").unwrap_or(0) as u8,
            offset: c.i64_at("offset").unwrap_or(0) as u8,
            format: c.i64_at("format").unwrap_or(0) as u8,
            dimension: dim,
        }));
    }
    let inline = vd.get("m_DataSize").and_then(Value::as_bytes).unwrap_or_default();
    let stream_size = u64_of(v.path("m_StreamData.size"));
    let data: Vec<u8> = if stream_size > 0 {
        let path = v.str_at("m_StreamData.path").unwrap_or_default();
        let sname = path.rsplit('/').next().unwrap_or_default();
        let s = universe.stream(sname)?.with_context(|| format!("stream {sname}"))?;
        let off = usize::try_from(u64_of(v.path("m_StreamData.offset")))?;
        let len = usize::try_from(stream_size)?;
        s.get(off..off + len).context("mesh stream slice out of range")?.to_vec()
    } else {
        inline.to_vec()
    };
    let streams = split_streams(&data, vertex_count, &channels)?;
    let index_u32 = v.i64_at("m_IndexFormat").unwrap_or(0) == 1;
    let indices = v.get("m_IndexBuffer").and_then(Value::as_bytes).unwrap_or_default().to_vec();
    let isz = if index_u32 { 4 } else { 2 };
    let mut submeshes = Vec::new();
    for s in v.get("m_SubMeshes").and_then(Value::as_array).unwrap_or_default() {
        let first_byte = u64_of(s.get("firstByte"));
        let index_count = u32::try_from(s.i64_at("indexCount").unwrap_or(0))?;
        ensure!(
            first_byte + u64::from(index_count) * isz <= indices.len() as u64,
            "{name}: submesh indices out of range"
        );
        submeshes.push(SubMesh {
            first_index: u32::try_from(first_byte / isz)?,
            index_count,
            base_vertex: i32::try_from(s.i64_at("baseVertex").unwrap_or(0))?,
            topology: s.i64_at("topology").unwrap_or(0) as u8,
            bounds: None,
        });
    }
    for sm in &mut submeshes {
        sm.bounds = submesh_bounds(&channels, &streams, &indices, index_u32, vertex_count, sm);
    }
    let aabb = (
        v.path("m_LocalAABB.m_Center").and_then(Value::vec3).unwrap_or_default(),
        v.path("m_LocalAABB.m_Extent").and_then(Value::vec3).unwrap_or_default(),
    );
    Ok(DecodedMesh { name, vertex_count, channels, streams, index_u32, indices, submeshes, aabb })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn submesh_bounds_cover_only_referenced_vertices() {
        // Four float32x3 vertices; the submesh (base vertex 1) indexes two of them.
        let verts: [[f32; 3]; 4] =
            [[9.0, 9.0, 9.0], [1.0, 2.0, 3.0], [-1.0, 5.0, 0.0], [7.0, 7.0, 7.0]];
        let data: Vec<u8> = verts.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
        let ch = vec![Some(Channel { stream: 0, offset: 0, format: 0, dimension: 3 })];
        let idx: Vec<u8> = [0u16, 1, 0].iter().flat_map(|i| i.to_le_bytes()).collect();
        let sm = SubMesh { first_index: 0, index_count: 3, base_vertex: 1, ..Default::default() };
        let b = submesh_bounds(&ch, &[(data, 12)], &idx, false, 4, &sm).unwrap();
        assert_eq!(b, ([-1.0, 2.0, 0.0], [1.0, 5.0, 3.0]));
        // Half-precision positions decode.
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xC000), -2.0);
    }

    #[test]
    fn streams_are_16_aligned_and_strided() {
        // Stream 0: position f32×3 (12 B); stream 1: uv f16×2 (4 B). 3 vertices.
        let ch = vec![
            Some(Channel { stream: 0, offset: 0, format: 0, dimension: 3 }),
            None,
            Some(Channel { stream: 1, offset: 0, format: 1, dimension: 2 }),
        ];
        let mut data = vec![1u8; 36];
        data.extend_from_slice(&[0u8; 12]); // pad 36 → 48
        data.extend_from_slice(&[2u8; 12]);
        let s = split_streams(&data, 3, &ch).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].0.len(), s[0].1), (36, 12));
        assert_eq!((s[1].0.as_slice(), s[1].1), (&[2u8; 12][..], 4));
        assert!(split_streams(&data[..40], 3, &ch).is_err());
    }
}
