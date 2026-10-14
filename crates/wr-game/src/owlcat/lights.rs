//! Owlcat's clustered-light setup, ported from `ClusteredLights`, `MinMaxZJob`,
//! `RadixSortJob`, `ExtractLightDataJob` and `ZBinningJob`: the constant buffers the
//! light-culling kernel and the deferred-lighting shader read.

use bevy::math::{Mat4, Vec3, Vec4, Vec4Swizzles};
use wr_assets::Light;

use super::consts::linear;

pub const MAX_LIGHTS: usize = 1024;
pub const ZBINS: usize = 4096;

/// What the frame needs from the light setup.
#[derive(Debug, Clone)]
pub struct LightBuffers {
    /// `LightDataCB`: 4 × 1024 float4.
    pub light_data: Vec<[f32; 4]>,
    /// `LightVolumeDataCB`: 3 × 1024 float4.
    pub volume_data: Vec<[f32; 4]>,
    /// `ZBinsCB`: 4096 × (min, max) u16 pairs = 1024 float4.
    pub zbins: Vec<u32>,
    /// `_LightDataParams`: directional count, punctual count, total.
    pub params: [f32; 4],
    /// `_ClusteringParams`: tiles x, tiles y, tile size, z-bins per metre.
    pub clustering: [f32; 4],
    pub visible: usize,
}

struct Desc {
    kind: i32,
    m: Mat4,
    range: f32,
    spot_angle: f32,
    inner_spot_angle: f32,
    color: Vec3,
    shadow_strength: f32,
    inner_radius: f32,
    falloff: i32,
    snap: bool,
    baked: bool,
    /// `ShadowDataIndex`: the light's entry in the shadow buffers, if it got a tile.
    shadow: Option<u32>,
    min_z: f32,
    max_z: f32,
}

/// Unity's visible-light selection, approximated: enabled lights on active objects that
/// contribute at runtime (not fully baked) and whose range sphere touches the frustum.
#[must_use]
pub fn visible_lights(lights: &[Light], view_proj: Mat4) -> Vec<&Light> {
    let rows = [view_proj.row(0), view_proj.row(1), view_proj.row(2), view_proj.row(3)];
    // Planes (Gribb–Hartmann) for a reversed-Z [0, 1] depth range: left, right,
    // bottom, top, near (w - z ≥ 0 at z = w), far (z ≥ 0).
    let planes = [
        rows[3] + rows[0],
        rows[3] - rows[0],
        rows[3] + rows[1],
        rows[3] - rows[1],
        rows[3] - rows[2],
        rows[2],
    ];
    lights
        .iter()
        .filter(|l| l.enabled && l.active && l.bake_type != 2 && l.intensity > 0.0)
        .filter(|l| {
            if l.kind == 1 {
                return true;
            }
            let p = Vec3::new(l.matrix[12], l.matrix[13], l.matrix[14]);
            planes.iter().all(|pl| {
                let n = pl.xyz();
                let len = n.length();
                len == 0.0 || (n.dot(p) + pl.w) / len >= -l.range
            })
        })
        .take(MAX_LIGHTS)
        .collect()
}

/// `ExtractLightDataJob.PackData` (`ExtractLightDataJob.cs`): bit 0 legacy falloff, bit 1
/// snapped specular, bit 2 "has a shadow record", bits 3..9 its index. (Bits 9+ carry a
/// shadowmask channel; the cave has none.)
fn pack_data(falloff: i32, snap: bool, shadow: Option<u32>) -> u32 {
    let mut n = u32::from(falloff == 1) | if snap { 2 } else { 0 };
    if let Some(i) = shadow.filter(|i| *i < 64) {
        n |= 4 | (i << 3);
    }
    n
}

#[expect(clippy::too_many_arguments, reason = "camera description")]
/// Builds the light buffers for a camera (`world_to_camera` as Unity's
/// `worldToCameraMatrix`) from the visible lights in visibility order, with each one's
/// shadow entry (the shadow planner runs on the same unsorted list, before the mean-Z
/// sort, as in `ClusteredLights.InitializeSingleThreadedParameters`).
#[must_use]
pub fn build(
    visible: &[&Light],
    shadow_slots: &[Option<u32>],
    world_to_camera: Mat4,
    near: f32,
    far: f32,
    width: u32,
    height: u32,
    tile: u32,
) -> LightBuffers {
    let to_view = Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0)) * world_to_camera;
    let mut descs: Vec<Desc> = visible
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let c = linear(l.color);
            let o = l.owlcat.clone().unwrap_or_default();
            Desc {
                kind: l.kind,
                m: Mat4::from_cols_array(&l.matrix),
                range: l.range,
                spot_angle: l.spot_angle,
                inner_spot_angle: l.inner_spot_angle,
                color: Vec3::new(c[0], c[1], c[2]) * l.intensity,
                shadow_strength: l.shadow_strength,
                inner_radius: o.inner_radius,
                falloff: o.falloff,
                snap: o.snap_specular_to_inner_radius,
                baked: l.bake_type == 2,
                shadow: shadow_slots.get(i).copied().flatten(),
                min_z: 0.0,
                max_z: 0.0,
            }
        })
        .collect();
    // MinMaxZJob.
    let mut mean = Vec::with_capacity(descs.len());
    for d in &mut descs {
        if d.kind == 1 {
            d.min_z = -10000.0;
            d.max_z = -10000.0;
            mean.push(-10000.0f32);
            continue;
        }
        let pos = d.m.w_axis.xyz();
        let pv = (to_view * pos.extend(1.0)).xyz();
        d.min_z = pv.z - d.range;
        d.max_z = pv.z + d.range;
        if d.kind == 0 {
            let half = d.spot_angle.to_radians() * 0.5;
            let cos = half.cos();
            let n3 = d.range * cos;
            let fwd = d.m.z_axis.xyz();
            let end = pos + fwd * n3;
            let ev = (to_view * end.extend(1.0)).xyz();
            let x = std::f32::consts::FRAC_PI_2 - half;
            let n4 = d.range * cos * half.sin() / x.sin();
            let f5 = ev - pv;
            let n5 = (1.0 - f5.z * f5.z / f5.dot(f5)).sqrt();
            if -f5.z < n3 * cos {
                d.min_z = pv.z.min(ev.z - n5 * n4);
            }
            if f5.z < n3 * cos {
                d.max_z = pv.z.max(ev.z + n5 * n4);
            }
        }
        d.min_z = d.min_z.max(0.0);
        d.max_z = d.max_z.max(0.0);
        mean.push((d.min_z + d.max_z) / 2.0);
    }
    // RadixSortJob: ascending; negative keys (directional lights) first, in reverse.
    let mut order: Vec<usize> = (0..descs.len()).collect();
    order.sort_by(|&a, &b| {
        let (ka, kb) = (mean[a], mean[b]);
        match (ka < 0.0, kb < 0.0) {
            (true, true) => b.cmp(&a),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal),
        }
    });
    let descs: Vec<&Desc> = order.iter().map(|&i| &descs[i]).collect();
    let n = descs.len().min(MAX_LIGHTS);
    let ndir = descs.iter().take_while(|d| d.kind == 1).count();
    let mut light_data = vec![[0.0f32; 4]; 4 * MAX_LIGHTS];
    let mut volume_data = vec![[0.0f32; 4]; 3 * MAX_LIGHTS];
    // ExtractLightDataJob.
    for (i, d) in descs.iter().take(n).enumerate() {
        let mut value = Vec4::ZERO;
        let mut v2 = Vec4::new(1.0, 0.0, 0.0, 1.0);
        let mut v3 = Vec4::ZERO;
        let mut v4 = Vec4::ZERO;
        let v6 = Vec4::new(d.min_z, d.max_z, 0.0, 0.0);
        if d.kind == 1 {
            value = (-d.m.z_axis.xyz()).extend(0.0);
        } else {
            value = d.m.w_axis.xyz().extend(value.w);
        }
        let packed = f32::from_bits(pack_data(d.falloff, d.snap, d.shadow));
        value.w = packed;
        let color = d.color.extend(d.shadow_strength);
        let mut w = 0.0;
        if d.kind != 1 {
            let falloff = if d.baked { 0 } else { d.falloff };
            w = if d.baked { 0.0 } else { d.inner_radius };
            w = w.clamp(0.0, 0.999);
            let r2 = d.range * d.range;
            if falloff == 0 {
                let n5 = w * w * r2 - r2;
                let n6 = 1.0 / n5;
                let y = -r2 / n5;
                let n7 = 1.0 / (0.0001f32).max(r2);
                v2.x = if w > 0.0 { n6 } else { n7 };
                v2.y = y;
            } else {
                v2.x = 1.0 / (0.0001f32).max(r2);
                v2.y = 25.0 / (0.0001f32).max(r2);
            }
            w *= d.range;
            v4 = to_view * d.m.w_axis.xyz().extend(1.0);
            v4.w = d.range;
        }
        let v5 = if d.kind == 0 {
            let dir = -d.m.z_axis.xyz();
            v3 = dir.extend(0.0);
            let outer = (std::f32::consts::PI / 180.0 * d.spot_angle * 0.5).cos();
            let inner = if d.inner_spot_angle > 0.0 {
                (d.inner_spot_angle.to_radians() * 0.5).cos()
            } else {
                (2.0 * ((d.spot_angle * 0.5).to_radians().tan() * 46.0 / 64.0).atan() * 0.5).cos()
            };
            let n10 = (0.001f32).max(inner - outer);
            let n11 = 1.0 / n10;
            v2.z = n11;
            v2.w = -outer * n11;
            let m3 = bevy::math::Mat3::from_mat4(to_view);
            (m3 * -v3.xyz()).extend((d.spot_angle * 0.5).to_radians())
        } else {
            Vec4::new(0.0, 0.0, 0.0, -1.0)
        };
        v3.w = w;
        light_data[i] = value.to_array();
        light_data[MAX_LIGHTS + i] = v2.to_array();
        light_data[2 * MAX_LIGHTS + i] = color.to_array();
        light_data[3 * MAX_LIGHTS + i] = v3.to_array();
        volume_data[i] = v4.to_array();
        volume_data[MAX_LIGHTS + i] = v5.to_array();
        volume_data[2 * MAX_LIGHTS + i] = v6.to_array();
    }
    // ZBinningJob.
    let zbin_factor = ZBINS as f32 / (far - near);
    let mut zbins = vec![0xFFFF_FFFFu32; ZBINS];
    for (j, d) in descs.iter().enumerate().take(n).skip(ndir) {
        let lo = (((d.min_z - near) * zbin_factor) as i32).max(0);
        let hi = (((d.max_z - near) * zbin_factor) as i32).min(ZBINS as i32 - 1);
        for k in lo..=hi {
            let b = &mut zbins[k as usize];
            let min = (*b & 0xFFFF).min(j as u32);
            *b = min | ((j as u32) << 16);
        }
    }
    let tiles_x = width.div_ceil(tile);
    let tiles_y = height.div_ceil(tile);
    LightBuffers {
        light_data,
        volume_data,
        zbins,
        params: [ndir as f32, (n - ndir) as f32, n as f32, 0.0],
        clustering: [tiles_x as f32, tiles_y as f32, tile as f32, zbin_factor],
        visible: n,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(x: f32, z: f32, range: f32) -> Light {
        let mut m = Mat4::IDENTITY.to_cols_array();
        m[12] = x;
        m[14] = z;
        Light {
            enabled: true,
            active: true,
            kind: 2,
            color: [1.0; 4],
            intensity: 2.0,
            range,
            bake_type: 4,
            matrix: m,
            ..Default::default()
        }
    }

    #[test]
    fn point_lights_sort_by_depth_and_fill_zbins() {
        // Camera at the origin looking down +Z (Unity view space looks down -Z, so
        // worldToCamera flips z).
        let w2c = Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0));
        let proj = bevy::math::proj::perspective(1.0, 2.0, 1001.0, 1.0);
        let lights = [point(0.0, 20.0, 2.0), point(0.0, 10.0, 2.0), {
            let mut b = point(0.0, 5.0, 1.0);
            b.bake_type = 2; // baked: not visible at runtime
            b
        }];
        let vis = visible_lights(&lights, proj * w2c);
        let slots = [None, Some(3)];
        let lb = build(&vis, &slots, w2c, 1.0, 1001.0, 64, 32, 16);
        assert_eq!(lb.visible, 2);
        assert_eq!(lb.params[0..3], [0.0, 2.0, 2.0]);
        // Nearer light first.
        assert_eq!(lb.light_data[0][2], 10.0);
        assert_eq!(lb.light_data[1][2], 20.0);
        // Colour × intensity, linear.
        assert_eq!(lb.light_data[2 * MAX_LIGHTS][0], 2.0);
        // Z bins: light 0 spans view z 8..12 → bins (7..11) × 4.096.
        let bin = |z: f32| ((z - 1.0) * 4.096) as usize;
        assert_eq!(lb.zbins[bin(10.0)] & 0xFFFF, 0);
        assert_eq!(lb.zbins[bin(20.0)], 1 | (1 << 16));
        assert_eq!(lb.zbins[bin(15.0)], 0xFFFF_FFFF);
        assert_eq!(lb.clustering[0..2], [4.0, 2.0]);
        // Shadow bits ride with the light through the sort: the nearer light (second in
        // the input, shadow entry 3) is first in the buffer.
        assert_eq!(lb.light_data[0][3].to_bits() & 4, 4);
        assert_eq!(lb.light_data[0][3].to_bits() >> 3, 3);
        assert_eq!(lb.light_data[1][3].to_bits() & 4, 0);
    }
}
