//! Height fields: `Gu::HeightField` as `loadFromDesc` builds it and PhysX 4.1's
//! `raycast_heightField` (`GuRaycastTests.cpp`) with `HeightFieldUtil::traceSegment`
//! (`GuHeightFieldUtil.h`) and `HFTraceSegmentCallback::faceHit`, for a single closest hit
//! (no `eMESH_MULTIPLE`). Checked against the player's own function (`native_check`).

use serde::{Deserialize, Serialize};

use crate::px::{
    Bounds, Transform, Vec3, intersect_ray_aabb2, intersect_ray_triangle, ray_aabb_intersect2,
    select_max, select_min, sign,
};

/// `PxHeightFieldMaterial::eHOLE`.
pub const HOLE: u8 = 127;

/// One `PxHeightFieldSample`: the height, and the two 7-bit material indices with their high
/// bits (`materialIndex0`'s is the tess flag, "zeroth vertex shared"; `materialIndex1`'s is
/// PhysX's own collision-vertex mark, which a raycast never reads).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sample {
    pub height: i16,
    pub material0: u8,
    pub material1: u8,
}

impl Sample {
    #[must_use]
    pub fn tess_flag(self) -> bool {
        self.material0 & 0x80 != 0
    }
    #[must_use]
    pub fn material_index0(self) -> u8 {
        self.material0 & 0x7f
    }
    #[must_use]
    pub fn material_index1(self) -> u8 {
        self.material1 & 0x7f
    }
}

/// A height field shape: `PxHeightField` data (row-major samples, rows along local x and
/// columns along local z) and its `PxHeightFieldGeometry` scales.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeightField {
    pub rows: u32,
    pub columns: u32,
    #[serde(with = "packed_samples")]
    pub samples: Vec<Sample>,
    pub height_scale: f32,
    pub row_scale: f32,
    pub column_scale: f32,
    /// `PxMeshGeometryFlag::eDOUBLE_SIDED` on the geometry.
    pub double_sided: bool,
}

/// A hit as `PxRaycastHit` reports it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RayHit {
    pub distance: f32,
    pub position: Vec3,
    pub face_index: u32,
}

impl HeightField {
    fn height(&self, vertex: u32) -> f32 {
        f32::from(self.samples[vertex as usize].height)
    }

    /// `mMinHeight`/`mMaxHeight` as `loadFromDesc` computes them (over the `PxI16`s).
    fn min_max_height(&self) -> (f32, f32) {
        let mut lo = i16::MAX;
        let mut hi = i16::MIN;
        for s in &self.samples {
            lo = if s.height < lo { s.height } else { lo };
            hi = if s.height > hi { s.height } else { hi };
        }
        (f32::from(lo), f32::from(hi))
    }

    /// `mData.mAABB` as `loadFromDesc` stores it: a `CenterExtents` of (0, min height, 0) to
    /// (rows - 1, max height, columns - 1).
    #[must_use]
    pub fn data_aabb(&self) -> (Vec3, Vec3) {
        let (lo, hi) = self.min_max_height();
        let b = Bounds {
            min: Vec3::new(0.0, lo, 0.0),
            max: Vec3::new((self.rows - 1) as f32, hi, (self.columns - 1) as f32),
        };
        (b.center(), b.extents())
    }

    /// `HeightFieldUtil::computeLocalBounds`: the data's bounds (`getMin()`/`getMax()` of the
    /// `CenterExtents`) through the scale matrix.
    fn local_bounds(&self) -> Bounds {
        let (center, extents) = self.data_aabb();
        let (min, max) = (center.minus(extents), center.plus(extents));
        let (rs, hs, cs) = (self.row_scale, self.height_scale, self.column_scale);
        // `PxMeshScale(...).toMat33()` with an identity rotation is diag(rs, hs, cs) with exact
        // zeros elsewhere; `PxMat33::transform` then adds the zero products.
        let t = |v: Vec3| {
            Vec3::new(
                rs * v.x + 0.0 * v.y + 0.0 * v.z,
                0.0 * v.x + hs * v.y + 0.0 * v.z,
                0.0 * v.x + 0.0 * v.y + cs * v.z,
            )
        };
        let mut b = Bounds { min: t(min), max: t(max) };
        // `GU_MIN_AABB_EXTENT * 0.5f - (max.y - min.y)`.
        let delta_y = 1e-3 * 0.5 - (b.max.y - b.min.y);
        if delta_y > 0.0 {
            b.max.y += delta_y * 0.6;
            b.min.y -= delta_y * 0.6;
        }
        b
    }

    /// `raycast_heightField(geom, pose, rayOrigin, rayDir, maxDist, hitFlags, 1, hits)` with
    /// `eMESH_BOTH_SIDES` as given: the closest hit, if any.
    #[must_use]
    pub fn raycast(
        &self,
        pose: Transform,
        ray_origin: Vec3,
        ray_dir: Vec3,
        max_dist: f32,
        mesh_both_sides: bool,
    ) -> Option<RayHit> {
        let inv = pose.inverse();
        let local_orig = inv.transform(ray_origin);
        let local_dir = inv.rotate(ray_dir);
        let both_sides = self.double_sided || mesh_both_sides;
        let one_over_row = 1.0 / self.row_scale;
        let one_over_column = 1.0 / self.column_scale;
        let norm_dir = local_dir.normalize_safe();
        let mut bounds = self.local_bounds();
        let center = bounds.center();
        let extents = bounds.extents().scale(1.01);
        bounds.min = center.minus(extents);
        bounds.max = center.plus(extents);
        let mut impact = Vec3::ZERO;
        let mut t = 0.0;
        if ray_aabb_intersect2(bounds.min, bounds.max, local_orig, local_dir, &mut impact, &mut t)
            == 0
        {
            return None;
        }
        if t > max_dist {
            return None;
        }
        let mut offset = 0.0f32;
        let mut orig_offset = local_orig;
        if t > 0.0 {
            offset = t - RAY_SURFACE_OFFSET;
            orig_offset = local_orig.plus(norm_dir.scale(offset));
        }
        let span = select_max(
            bounds.max.x - bounds.min.x,
            select_max(bounds.max.y - bounds.min.y, bounds.max.z - bounds.min.z),
        );
        let max_dist_offset = select_min(max_dist - offset, RAY_SURFACE_OFFSET + 2.0 * span);
        let found = self.trace_segment(
            orig_offset,
            norm_dir,
            max_dist_offset,
            bounds,
            !both_sides,
            one_over_row,
            one_over_column,
        )?;
        // `HFTraceSegmentCallback::faceHit`.
        let (point, face_index) = found;
        let distance = select_max(0.0, point.minus(local_orig).dot(local_dir));
        Some(RayHit { distance, position: pose.transform(point), face_index })
    }

    /// `traceSegment<HFTraceSegmentCallback, false, false>` stopping at the first face hit: that
    /// hit's shape-space point and triangle index.
    #[allow(clippy::too_many_arguments)]
    fn trace_segment(
        &self,
        a_p0: Vec3,
        ray_dir: Vec3,
        ray_length: f32,
        bounds: Bounds,
        backface_cull: bool,
        one_over_row: f32,
        one_over_column: f32,
    ) -> Option<(Vec3, u32)> {
        let (ok, tnear, tfar) =
            intersect_ray_aabb2(bounds.min, bounds.max, a_p0, ray_dir, ray_length);
        if !ok {
            return None;
        }
        let p0 = a_p0.plus(ray_dir.scale(tnear));
        let p1 = a_p0.plus(ray_dir.scale(tfar));
        let (expandu, expandv) = (0.0f32, 0.0f32);
        let (row_scale, column_scale, height_scale) =
            (self.row_scale, self.column_scale, self.height_scale);
        let nb_vi = self.columns as i32;
        let nb_ui = self.rows as i32;
        let clamp_eps = 1e-7f32;
        let nb_ucells = (nb_ui - 1) as f32 * (1.0 - clamp_eps);
        let nb_vcells = (nb_vi - 1) as f32 * (1.0 - clamp_eps);
        let uu0 = p0.x * one_over_row;
        let u0 = select_min(select_max(uu0, 1e-7 - expandu), nb_ucells + expandu);
        let uv0 = p0.z * one_over_column;
        let v0 = select_min(select_max(uv0, 1e-7 - expandv), nb_vcells + expandv);
        let h0 = p0.y;
        let uu1 = p1.x * one_over_row;
        let uv1 = p1.z * one_over_column;
        let h1 = p1.y;
        let mut du = uu1 - uu0;
        let mut dv = uv1 - uv0;
        let dh = h1 - h0;
        let step_uif = sign(du);
        let step_vif = sign(dv);
        let step_ui = step_uif as i32;
        let step_vi = step_vif as i32;
        let div_epsilon = 1e-10f32;
        if du.abs() < div_epsilon {
            du = step_uif * div_epsilon;
        }
        if dv.abs() < div_epsilon {
            dv = step_vif * div_epsilon;
        }
        let auh_p0 = Vec3::new(a_p0.x * one_over_row, a_p0.y, a_p0.z * one_over_column);
        let duhv = Vec3::new(
            ray_dir.x * ray_length * one_over_row,
            ray_dir.y * ray_length,
            ray_dir.z * ray_length * one_over_column,
        );
        let duhv_length = duhv.magnitude();
        let mut duhv_n = duhv;
        if duhv_length > crate::px::NORMALIZATION_EPSILON {
            duhv_n = duhv_n.scale(1.0 / duhv_length);
        }
        let mut ui = if du > 0.0 { u0.floor() as i32 } else { u0.ceil() as i32 };
        let mut vi = if dv > 0.0 { v0.floor() as i32 } else { v0.ceil() as i32 };
        let uhit0 = if du > 0.0 { ceil_up(u0) } else { floor_down(u0) };
        let vhit0 = if dv > 0.0 { ceil_up(v0) } else { floor_down(v0) };
        let mut tu = (uhit0 - uu0) / du;
        let mut tv = (vhit0 - uv0) / dv;
        if tu < 0.0 {
            tu = (clamp_eps / du).abs();
        }
        if tv < 0.0 {
            tv = (clamp_eps / dv).abs();
        }
        let step_tu = 1.0 / du.abs();
        let step_tv = 1.0 / dv.abs();
        let h_eps = 1e-4f32;
        let mut uif = ui as f32;
        let mut vif = vi as f32;
        let uflip = (1 - step_ui) as usize; // 0 or 2
        let vflip = ((1 - step_vi) / 2) as usize; // 0 or 1
        let t_end = 1.0f32 - 1e-4;
        let mut h_line_prev = h0 + 0.0 * dh;
        loop {
            let t_min_uv = select_min(tu, tv);
            let h_line_next = h0 + t_min_uv * dh;
            let col0 = (nb_vi * ui + vi) as u32;
            let col1 = (nb_vi * (ui + step_ui) + vi) as u32;
            let sv = step_vi as u32;
            let h = [
                self.height(col0) * height_scale,
                self.height(col0.wrapping_add(sv)) * height_scale,
                self.height(col1) * height_scale,
                self.height(col1.wrapping_add(sv)) * height_scale,
            ];
            let min_h = select_min(select_min(h[0], h[1]), select_min(h[2], h[3]));
            let max_h = select_max(select_max(h[0], h[1]), select_max(h[2], h[3]));
            let range_min = select_min(h_line_prev, h_line_next);
            let range_max = select_max(h_line_prev, h_line_next);
            if !(range_min - h_eps > max_h || range_max + h_eps < min_h) {
                let h00 = h[uflip + vflip];
                let h01 = h[1 + uflip - vflip];
                let h10 = h[2 - uflip + vflip];
                let h11 = h[3 - uflip - vflip];
                let minuif = select_min(uif, uif + step_uif);
                let maxuif = select_max(uif, uif + step_uif);
                let minvif = select_min(vif, vif + step_vif);
                let maxvif = select_max(vif, vif + step_vif);
                let p00 = Vec3::new(minuif, h00, minvif);
                let p01 = Vec3::new(minuif, h01, maxvif);
                let p10 = Vec3::new(maxuif, h10, minvif);
                let p11 = Vec3::new(maxuif, h11, maxvif);
                let enlarge = 0.0001f32;
                let (mut p00a, mut p01a, mut p10a, mut p11a) = (p00, p01, p10, p11);
                let minui = (ui + step_ui).min(ui) as u32;
                let minvi = (vi + step_vi).min(vi) as u32;
                let vert_index = self.columns * minui + minvi;
                let cell_index = vert_index;
                let s = self.samples[vert_index as usize];
                if !s.tess_flag() {
                    p10a = p00;
                    p00a = p01;
                    p01a = p11;
                    p11a = p10;
                }
                let tri0 = intersect_ray_triangle(
                    auh_p0,
                    duhv_n,
                    p10a,
                    p00a,
                    p11a,
                    backface_cull,
                    enlarge,
                )
                .filter(|&(t, _, _)| t >= 0.0 && t <= duhv_length && s.material_index0() != HOLE);
                let tri1 = intersect_ray_triangle(
                    auh_p0,
                    duhv_n,
                    p01a,
                    p11a,
                    p00a,
                    backface_cull,
                    enlarge,
                )
                .filter(|&(t, _, _)| t >= 0.0 && t <= duhv_length && s.material_index1() != HOLE);
                let t0 = tri0.map_or(f32::MAX, |x| x.0);
                let t1 = tri1.map_or(f32::MAX, |x| x.0);
                let point = |t: f32| {
                    Vec3::new(
                        (auh_p0.x + duhv_n.x * t) * row_scale,
                        auh_p0.y + duhv_n.y * t,
                        (auh_p0.z + duhv_n.z * t) * column_scale,
                    )
                };
                if tri0.is_some() && t0 <= t1 {
                    return Some((point(t0), cell_index * 2));
                } else if tri1.is_some() && t1 <= t0 {
                    return Some((point(t1), cell_index * 2 + 1));
                }
            }
            if tu < tv {
                ui += step_ui;
                if ui + step_ui < 0 || ui + step_ui >= nb_ui {
                    break;
                }
                uif += step_uif;
                tu += step_tu;
            } else {
                vi += step_vi;
                if vi + step_vi < 0 || vi + step_vi >= nb_vi {
                    break;
                }
                vif += step_vif;
                tv += step_tv;
            }
            h_line_prev = h_line_next;
            if t_min_uv >= t_end {
                break;
            }
        }
        None
    }
}

/// Samples in the file: the heights as little-endian bytes, and the materials once when every
/// sample has the same (a terrain without holes), else per sample.
mod packed_samples {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::Sample;

    #[derive(Serialize, Deserialize)]
    enum Materials {
        Uniform(u8, u8),
        PerSample(Vec<u8>),
    }

    #[derive(Serialize, Deserialize)]
    struct Packed {
        heights: Vec<u8>,
        materials: Materials,
    }

    pub fn serialize<S: Serializer>(samples: &[Sample], s: S) -> Result<S::Ok, S::Error> {
        let heights = samples.iter().flat_map(|x| x.height.to_le_bytes()).collect();
        let materials = match samples.first() {
            Some(f)
                if samples
                    .iter()
                    .all(|x| (x.material0, x.material1) == (f.material0, f.material1)) =>
            {
                Materials::Uniform(f.material0, f.material1)
            }
            _ => Materials::PerSample(
                samples.iter().flat_map(|x| [x.material0, x.material1]).collect(),
            ),
        };
        Packed { heights, materials }.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Sample>, D::Error> {
        let p = Packed::deserialize(d)?;
        if p.heights.len() % 2 != 0 {
            return Err(serde::de::Error::custom("odd height bytes"));
        }
        let n = p.heights.len() / 2;
        let material = |i: usize| match &p.materials {
            Materials::Uniform(a, b) => Some((*a, *b)),
            Materials::PerSample(m) => Some((*m.get(2 * i)?, *m.get(2 * i + 1)?)),
        };
        if let Materials::PerSample(m) = &p.materials
            && m.len() != 2 * n
        {
            return Err(serde::de::Error::custom("material count differs from the heights'"));
        }
        (0..n)
            .map(|i| {
                let (material0, material1) =
                    material(i).ok_or_else(|| serde::de::Error::custom("materials"))?;
                Ok(Sample {
                    height: i16::from_le_bytes([p.heights[2 * i], p.heights[2 * i + 1]]),
                    material0,
                    material1,
                })
            })
            .collect()
    }
}

/// `GU_RAY_SURFACE_OFFSET`.
const RAY_SURFACE_OFFSET: f32 = 10.0;

fn floor_down(x: f32) -> f32 {
    let f = x.floor();
    if f == x { f - 1.0 } else { f }
}

fn ceil_up(x: f32) -> f32 {
    let f = x.ceil();
    if f == x { f + 1.0 } else { f }
}
