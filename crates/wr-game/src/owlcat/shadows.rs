//! Owlcat's clustered shadows, ported from `ClusteredShadows`, `ShadowmapEntry`,
//! `MaxRectsBinPack` and `TetrahedronUtils`: which lights get an atlas tile, how big,
//! the world-to-shadow matrices (spot: one; point: four tetrahedral faces) with their
//! guard angles and biases, and the two structured buffers the caster and the deferred
//! lighting shader read (`_ShadowDataBuffer`, `_ShadowMatricesBuffer`).
//!
//! Everything here is CPU-side planning: no GPU objects. Citations are to the decompiled
//! `Owlcat.Runtime.Visual.RenderPipeline.Shadows` classes (`CS` = `ClusteredShadows.cs`).
//! Directional lights are not planned yet (the cave's is inactive); see
//! `docs/poc6/render-round3.md`.

use bevy::math::{Mat4, Vec3, Vec4};
use wr_assets::Light;

/// `kMaxShadowEntries` (CS:13).
pub const MAX_ENTRIES: usize = 64;
/// `kMaxShadowMatrices` (CS:15).
pub const MAX_MATRICES: usize = 256;
/// `Marshal.SizeOf(ShadowData)` (`ShadowData.cs`): float4, float4, int, int, float2.
pub const DATA_STRIDE: usize = 48;
/// `Marshal.SizeOf(ShadowMatrix)` (`ShadowMatrix.cs`).
pub const MATRIX_STRIDE: usize = 112;
/// `kFov0` / `kFov1` (CS:17, CS:19): the tetrahedral face field of view in degrees.
const FOV0: f32 = 143.9857;
const FOV1: f32 = 125.26439;
/// `kResolutionReducingDelta` (CS:21), the punctual resolution quantum.
const RES_STEP: f32 = 128.0;

/// `TetrahedronUtils.FaceVectors` (`TetrahedronUtils.cs:13`).
pub const FACE_VECTORS: [Vec3; 4] = [
    Vec3::new(0.0, -0.577_350_26, 0.816_496_6),
    Vec3::new(0.0, -0.577_350_26, -0.816_496_6),
    Vec3::new(-0.816_496_6, 0.577_350_26, 0.0),
    Vec3::new(0.816_496_6, 0.577_350_26, 0.0),
];

/// The pipeline asset's shadow settings (`ShadowSettings.cs` defaults: the shipped asset
/// is not imported yet, see the handoff).
#[derive(Debug, Clone)]
pub struct Settings {
    pub atlas_size: u32,
    pub spot_resolution: u32,
    pub point_resolution: u32,
    pub depth_bias: f32,
    pub normal_bias: f32,
    pub distance: f32,
    /// Debugging filters (`WR_SHADOW_KIND=spot|point`, `WR_SHADOW_PICK=n`: only the n-th
    /// eligible light). Not part of Owlcat's behaviour.
    pub only_kind: Option<i32>,
    pub pick: Option<usize>,
}

impl Settings {
    /// The pipeline defaults, with the debugging filters read from the environment.
    #[must_use]
    pub fn from_env() -> Self {
        let kind = std::env::var("WR_SHADOW_KIND").ok().and_then(|k| match k.as_str() {
            "spot" => Some(0),
            "point" => Some(2),
            _ => None,
        });
        let pick = std::env::var("WR_SHADOW_PICK").ok().and_then(|v| v.parse().ok());
        Self { only_kind: kind, pick, ..Self::default() }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            atlas_size: 4096,
            spot_resolution: 512,
            point_resolution: 512,
            depth_bias: 2.6,
            normal_bias: 0.1,
            distance: 50.0,
            only_kind: None,
            pick: None,
        }
    }
}

/// `MaxRectsBinPack` (`MaxRectsBinPack.cs`), without rotations, `RectBestLongSideFit`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone)]
pub struct MaxRects {
    free: Vec<Rect>,
}

impl MaxRects {
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self { free: vec![Rect { x: 0.0, y: 0.0, w: width as f32, h: height as f32 }] }
    }

    /// `FindPositionForNewNodeBestLongSideFit` then `Insert`'s split and prune
    /// (`MaxRectsBinPack.cs:49`, `:258`, `:409`, `:450`). `None` when nothing fits (the
    /// original returns a zero-height rect).
    pub fn insert(&mut self, width: u32, height: u32) -> Option<Rect> {
        let (w, h) = (width as f32, height as f32);
        let (mut best_long, mut best_short) = (i32::MAX, 0i32);
        let mut node = Rect::default();
        for f in &self.free {
            if f.w >= w && f.h >= h {
                let a = (f.w as i32 - width as i32).abs();
                let b = (f.h as i32 - height as i32).abs();
                let (short, long) = (a.min(b), a.max(b));
                if long < best_long || (long == best_long && short < best_short) {
                    node = Rect { x: f.x, y: f.y, w, h };
                    best_short = short;
                    best_long = long;
                }
            }
        }
        if node.h == 0.0 {
            return None;
        }
        let mut n = self.free.len();
        let mut i = 0;
        while i < n {
            let f = self.free[i];
            if self.split(f, node) {
                self.free.remove(i);
                n -= 1;
            } else {
                i += 1;
            }
        }
        self.prune();
        Some(node)
    }

    fn split(&mut self, f: Rect, u: Rect) -> bool {
        if u.x >= f.x + f.w || u.x + u.w <= f.x || u.y >= f.y + f.h || u.y + u.h <= f.y {
            return false;
        }
        if u.x < f.x + f.w && u.x + u.w > f.x {
            if u.y > f.y && u.y < f.y + f.h {
                self.free.push(Rect { h: u.y - f.y, ..f });
            }
            if u.y + u.h < f.y + f.h {
                self.free.push(Rect { y: u.y + u.h, h: f.y + f.h - (u.y + u.h), ..f });
            }
        }
        if u.y < f.y + f.h && u.y + u.h > f.y {
            if u.x > f.x && u.x < f.x + f.w {
                self.free.push(Rect { w: u.x - f.x, ..f });
            }
            if u.x + u.w < f.x + f.w {
                self.free.push(Rect { x: u.x + u.w, w: f.x + f.w - (u.x + u.w), ..f });
            }
        }
        true
    }

    fn prune(&mut self) {
        let contained = |a: Rect, b: Rect| {
            a.x >= b.x && a.y >= b.y && a.x + a.w <= b.x + b.w && a.y + a.h <= b.y + b.h
        };
        let mut i = 0;
        while i < self.free.len() {
            let mut j = i + 1;
            let mut removed_i = false;
            while j < self.free.len() {
                if contained(self.free[i], self.free[j]) {
                    self.free.remove(i);
                    removed_i = true;
                    break;
                }
                if contained(self.free[j], self.free[i]) {
                    self.free.remove(j);
                } else {
                    j += 1;
                }
            }
            if !removed_i {
                i += 1;
            }
        }
    }
}

/// `CalcGuardAnglePerspective` (CS:483).
#[must_use]
pub fn guard_angle(
    angle_deg: f32,
    resolution: f32,
    filter_width: f32,
    normal_bias_max: f32,
    max_deg: f32,
) -> f32 {
    let half = angle_deg * 0.5 * (std::f32::consts::PI / 180.0);
    let texel = 2.0 / resolution;
    let mut n = half.cos() * texel;
    let bias_angle = (normal_bias_max * n * std::f32::consts::SQRT_2).atan();
    n = (half + bias_angle).tan() * texel;
    let mut g = ((resolution + filter_width.ceil()) * n * 0.5).atan() * 2.0 * 57.295_78 - angle_deg;
    g *= 2.0;
    if g < max_deg { g } else { max_deg }
}

/// `GetFilterWidth` (CS:499).
const FILTER_WIDTH: f32 = 5.0;

/// `Matrix4x4.Perspective(fov, aspect, near, far)`: OpenGL clip conventions, looking
/// down -Z.
#[must_use]
pub fn unity_perspective(fov_deg: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
    let f = 1.0 / (fov_deg.to_radians() * 0.5).tan();
    let d = near - far;
    Mat4::from_cols(
        Vec4::new(f / aspect, 0.0, 0.0, 0.0),
        Vec4::new(0.0, f, 0.0, 0.0),
        Vec4::new(0.0, 0.0, (far + near) / d, -1.0),
        Vec4::new(0.0, 0.0, 2.0 * far * near / d, 0.0),
    )
}

/// `GL.GetGPUProjectionMatrix(m, renderIntoTexture: true)` on Metal: Y flipped, depth
/// remapped to reversed Z in [0, 1] (checked against the route's exported camera:
/// `gpu_projection_render_into_texture` = diag(1, -1, -0.5, 1) with row 2 += 0.5 row 3).
#[must_use]
pub fn gpu_projection(m: Mat4) -> Mat4 {
    let r = |i: usize| m.row(i);
    let rows = [r(0), -r(1), -0.5 * r(2) + 0.5 * r(3), r(3)];
    Mat4::from_cols(
        Vec4::new(rows[0].x, rows[1].x, rows[2].x, rows[3].x),
        Vec4::new(rows[0].y, rows[1].y, rows[2].y, rows[3].y),
        Vec4::new(rows[0].z, rows[1].z, rows[2].z, rows[3].z),
        Vec4::new(rows[0].w, rows[1].w, rows[2].w, rows[3].w),
    )
}

/// `CreatePerspective` (CS:536): separate horizontal and vertical field of view.
fn punctual_perspective(fov_x: f32, fov_y: f32, near: f32, far: f32) -> Mat4 {
    let mut p = unity_perspective(fov_y, 1.0, near, far);
    p.x_axis.x = 1.0 / (std::f32::consts::PI / 180.0 * fov_x * 0.5).tan();
    p.y_axis.y = 1.0 / (std::f32::consts::PI / 180.0 * fov_y * 0.5).tan();
    gpu_projection(p)
}

/// `CalculateTetrahedronMatrices` (CS:518): per face, rotation about the light.
fn tetra_rotations() -> [Mat4; 4] {
    let rx = |d: f32| Mat4::from_rotation_x(d.to_radians());
    let ry = |d: f32| Mat4::from_rotation_y(d.to_radians());
    let rz = |d: f32| Mat4::from_rotation_z(d.to_radians());
    [
        rx(27.367_805) * ry(180.0),
        rz(90.0) * rx(27.367_805) * ry(0.0),
        rx(-27.367_805) * ry(270.0),
        rz(90.0) * rx(-27.367_805) * ry(90.0),
    ]
}

/// `m_PointScaleMatrices` (CS:27).
fn point_scales() -> [Mat4; 4] {
    [
        Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)),
        Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0)),
        Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)),
        Mat4::from_scale(Vec3::new(1.0, -1.0, 1.0)),
    ]
}

/// `m_PointLightTexMatrices` (CS:109): where each face sits in the clip square.
fn point_placements() -> [Mat4; 4] {
    let m = |sx: f32, sy: f32, tx: f32, ty: f32| {
        Mat4::from_cols(
            Vec4::new(sx, 0.0, 0.0, 0.0),
            Vec4::new(0.0, sy, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0, 0.0),
            Vec4::new(tx, ty, 0.0, 1.0),
        )
    };
    [m(1.0, 0.5, 0.0, 0.5), m(0.5, 1.0, 0.5, 0.0), m(1.0, 0.5, 0.0, -0.5), m(0.5, 1.0, -0.5, 0.0)]
}

/// `CalculateLineEquationCoeffs` (CS:513).
fn line(p1: (f32, f32), p2: (f32, f32)) -> Vec3 {
    Vec3::new(p1.1 - p2.1, p2.0 - p1.0, p1.0 * p2.1 - p2.0 * p1.1)
}

/// `m_PointLightClips` (CS:101): the two clip-distance equations per face (`_Clips`).
#[must_use]
pub fn point_clips() -> [Vec3; 8] {
    [
        line((0.0, 0.0), (-1.0, 1.0)),
        line((1.0, 1.0), (0.0, 0.0)),
        line((0.0, 0.0), (1.0, 1.0)),
        line((1.0, -1.0), (0.0, 0.0)),
        line((0.0, 0.0), (1.0, -1.0)),
        line((-1.0, -1.0), (0.0, 0.0)),
        line((0.0, 0.0), (-1.0, -1.0)),
        line((-1.0, 1.0), (0.0, 0.0)),
    ]
}

/// One matrix record (`ShadowMatrix`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ShadowMatrix {
    pub world_to_shadow: Mat4,
    pub light_direction: Vec3,
    pub normal_bias: f32,
    pub depth_bias: f32,
}

impl ShadowMatrix {
    /// The 112-byte record the shaders read as raw words (`DL/blob4.metal:84`).
    #[must_use]
    pub fn bytes(&self) -> [u8; MATRIX_STRIDE] {
        let mut b = [0u8; MATRIX_STRIDE];
        let mut put = |o: usize, v: &[f32]| {
            for (i, x) in v.iter().enumerate() {
                b[o + i * 4..o + i * 4 + 4].copy_from_slice(&x.to_le_bytes());
            }
        };
        put(0, &self.world_to_shadow.to_cols_array());
        // 64..84: spherePosition, sphereRadius, sphereRadiusSq (directional only).
        put(84, &self.light_direction.to_array());
        put(96, &[self.normal_bias, self.depth_bias]);
        b
    }
}

/// `ShadowFlags`.
pub const FLAG_POINT: i32 = 1;
pub const FLAG_SOFT: i32 = 2;

/// An atlas tile: one spot (one face) or one point light (four).
#[derive(Debug, Clone)]
pub struct Entry {
    /// Index into the visible-light list this entry was planned from.
    pub light: usize,
    pub point: bool,
    /// Pixels in the atlas.
    pub viewport: Rect,
    pub flags: i32,
    pub matrix_indices: [i32; 4],
    /// `(width/A, height/A, x/A, y/A)`.
    pub scale_offset: [f32; 4],
    /// The configured depth bias (point lights use it as the raster slope bias).
    pub depth_bias: f32,
    pub near_plane: f32,
    pub range: f32,
    pub position: Vec3,
    /// Per face: the clip matrix before the face placement (for caster culling).
    pub cull: Vec<Mat4>,
}

impl Entry {
    #[must_use]
    pub fn faces(&self) -> usize {
        if self.point { 4 } else { 1 }
    }

    /// The 48-byte `ShadowData` record.
    #[must_use]
    pub fn data_bytes(&self) -> [u8; DATA_STRIDE] {
        let mut b = [0u8; DATA_STRIDE];
        let mut put = |o: usize, v: &[f32]| {
            for (i, x) in v.iter().enumerate() {
                b[o + i * 4..o + i * 4 + 4].copy_from_slice(&x.to_le_bytes());
            }
        };
        put(0, &self.matrix_indices.map(|i| i as f32));
        put(16, &self.scale_offset);
        b[32..36].copy_from_slice(&self.flags.to_le_bytes());
        b
    }
}

/// The camera facts the planner needs.
#[derive(Debug, Clone, Copy)]
pub struct CameraInfo {
    pub width: u32,
    pub height: u32,
    /// `GL.GetGPUProjectionMatrix(projection, true) * worldToCameraMatrix`.
    pub view_proj: Mat4,
}

/// A frame's shadow plan.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub entries: Vec<Entry>,
    pub matrices: Vec<ShadowMatrix>,
    /// Per visible light: its entry index (the shader's shadow index).
    pub slot: Vec<Option<u32>>,
    /// Lights that asked for a shadow and were dropped (no room or no tile).
    pub dropped: usize,
}

impl Plan {
    #[must_use]
    pub fn data_bytes(&self) -> Vec<u8> {
        self.entries.iter().flat_map(|e| e.data_bytes()).collect()
    }

    #[must_use]
    pub fn matrix_bytes(&self) -> Vec<u8> {
        self.matrices.iter().flat_map(|m| m.bytes()).collect()
    }
}

/// Normalised screen rect (x, y, w, h) of a punctual light's bounds, an approximation of
/// `VisibleLight.screenRect` (native; the exact rule needs a capture): the projected
/// bounds of the range sphere (spots: the cone's apex and base circle), clamped to the
/// screen; the full screen if any point is behind the camera plane.
#[must_use]
pub fn screen_rect(l: &Light, view_proj: Mat4) -> [f32; 4] {
    let m = Mat4::from_cols_array(&l.matrix);
    let p = m.w_axis.truncate();
    let mut pts: Vec<Vec3> = Vec::new();
    let half = l.spot_angle.to_radians() * 0.5;
    if l.kind == 0 && half < std::f32::consts::FRAC_PI_2 {
        let fwd = m.z_axis.truncate().normalize_or_zero();
        let up = if fwd.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
        let u = fwd.cross(up).normalize_or_zero();
        let v = fwd.cross(u);
        let c = p + fwd * l.range;
        let r = half.tan() * l.range;
        pts.push(p);
        for k in 0..16 {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            pts.push(c + (u * a.cos() + v * a.sin()) * r);
        }
    } else {
        for sx in [-1.0, 1.0] {
            for sy in [-1.0, 1.0] {
                for sz in [-1.0, 1.0] {
                    pts.push(p + Vec3::new(sx, sy, sz) * l.range);
                }
            }
        }
    }
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for q in pts {
        let c = view_proj * q.extend(1.0);
        // Reversed Z: in front of the camera means 0 < z <= w; behind the eye w <= 0.
        if c.w <= 1e-4 {
            return [0.0, 0.0, 1.0, 1.0];
        }
        let (x, y) = (c.x / c.w * 0.5 + 0.5, c.y / c.w * 0.5 + 0.5);
        lo = [lo[0].min(x), lo[1].min(y)];
        hi = [hi[0].max(x), hi[1].max(y)];
    }
    let (x0, y0) = (lo[0].clamp(0.0, 1.0), lo[1].clamp(0.0, 1.0));
    let (x1, y1) = (hi[0].clamp(0.0, 1.0), hi[1].clamp(0.0, 1.0));
    [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
}

/// `CalculatePunctualResolution` (CS:469).
#[must_use]
pub fn punctual_resolution(cam: &CameraInfo, rect: [f32; 4], point: bool, max: u32) -> u32 {
    let mut n = (cam.width as f32 * rect[2]).max(cam.height as f32 * rect[3]).trunc();
    if point {
        n *= 4.0;
    }
    if n > max as f32 {
        return max;
    }
    ((n / RES_STEP).trunc() * RES_STEP).max(RES_STEP) as u32
}

/// `CalculateShadowBias` (CS:374).
fn shadow_bias(
    depth_bias: f32,
    normal_bias: f32,
    soft: bool,
    frustum: f32,
    width: f32,
) -> (f32, f32) {
    let k = frustum / width;
    let (mut d, mut n) = (-depth_bias * k, -normal_bias * k);
    if soft {
        let f = FILTER_WIDTH * 0.5;
        d *= f;
        n *= f;
    }
    (d, n)
}

/// `ProcessRequests` + `ProcessLight` (CS:221, :238) for spots and points, over the
/// visible lights in order. `has_casters(i)` stands for `GetShadowCasterBounds(i)`.
pub fn plan(
    settings: &Settings,
    cam: &CameraInfo,
    lights: &[&Light],
    has_casters: &dyn Fn(usize) -> bool,
) -> Plan {
    let mut out = Plan { slot: vec![None; lights.len()], ..Default::default() };
    let mut packer = MaxRects::new(settings.atlas_size, settings.atlas_size);
    let atlas = settings.atlas_size as f32;
    let proj = tetra_rotations();
    let scales = point_scales();
    let place = point_placements();
    let mut eligible = 0usize;
    for (i, l) in lights.iter().enumerate() {
        if !has_casters(i) {
            continue;
        }
        if l.shadows == 0 || (l.kind != 0 && l.kind != 2) {
            continue;
        }
        if settings.only_kind.is_some_and(|k| k != l.kind) {
            continue;
        }
        eligible += 1;
        if settings.pick.is_some_and(|p| p != eligible - 1) {
            continue;
        }
        if out.entries.len() >= MAX_ENTRIES {
            out.dropped += 1;
            continue;
        }
        let point = l.kind == 2;
        let rect = screen_rect(l, cam.view_proj);
        let res = punctual_resolution(
            cam,
            rect,
            point,
            if point { settings.point_resolution } else { settings.spot_resolution },
        );
        let Some(tile) = packer.insert(res, res) else {
            out.dropped += 1;
            continue;
        };
        if out.matrices.len() + if point { 4 } else { 1 } > MAX_MATRICES {
            out.dropped += 1;
            continue;
        }
        let soft = l.shadows == 2;
        let pipeline_bias = l.owlcat.as_ref().is_none_or(|o| o.use_pipeline_settings);
        let (depth_bias, normal_bias) = if pipeline_bias {
            (settings.depth_bias, settings.normal_bias)
        } else {
            (l.shadow_bias, l.shadow_normal_bias)
        };
        let m = Mat4::from_cols_array(&l.matrix);
        let pos = m.w_axis.truncate();
        let mut e = Entry {
            light: i,
            point,
            viewport: tile,
            flags: if point { FLAG_POINT } else { 0 } | if soft { FLAG_SOFT } else { 0 },
            matrix_indices: [0; 4],
            scale_offset: [tile.w / atlas, tile.h / atlas, tile.x / atlas, tile.y / atlas],
            depth_bias,
            near_plane: l.shadow_near_plane,
            range: l.range,
            position: pos,
            cull: Vec::new(),
        };
        if point {
            // CalculatePointLightMatrices (CS:387).
            let g0 = guard_angle(FOV0, tile.w, FILTER_WIDTH, 0.5, 36.014_297);
            let g1 = guard_angle(FOV1, tile.w, FILTER_WIDTH, 0.5, 54.735_61);
            let near = l.shadow_near_plane * 0.5;
            let p = [
                punctual_perspective(FOV0 + g0, FOV1 + g1, near, l.range),
                punctual_perspective(FOV1 + g1, FOV0 + g0, near, l.range),
            ];
            let tr = Mat4::from_translation(-pos);
            for f in 0..4 {
                let view = scales[f] * proj[f] * tr;
                let clip = p[f & 1] * view;
                let frustum = (1.256_512_3f32).tan() * l.range;
                let (_, nb) = shadow_bias(depth_bias, normal_bias, soft, frustum, tile.w);
                e.matrix_indices[f] = out.matrices.len() as i32;
                e.cull.push(clip);
                out.matrices.push(ShadowMatrix {
                    world_to_shadow: place[f] * clip,
                    light_direction: -FACE_VECTORS[f],
                    normal_bias: nb,
                    // `item.depthBias = 0f` (CS:416): points use the raster slope bias.
                    depth_bias: 0.0,
                });
            }
        } else {
            // Spot (CS:278).
            let view = Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0)) * m.inverse();
            let guard = guard_angle(l.spot_angle, tile.w, FILTER_WIDTH, 4.0, 180.0 - l.spot_angle);
            let proj = gpu_projection(unity_perspective(
                l.spot_angle + guard,
                1.0,
                l.shadow_near_plane,
                l.range,
            ));
            let frustum = (l.spot_angle * 0.5 * (std::f32::consts::PI / 180.0)).tan() * l.range;
            let (db, nb) = shadow_bias(depth_bias, normal_bias, soft, frustum, tile.w);
            e.matrix_indices[0] = out.matrices.len() as i32;
            e.cull.push(proj * view);
            out.matrices.push(ShadowMatrix {
                world_to_shadow: proj * view,
                light_direction: -m.z_axis.truncate(),
                normal_bias: nb,
                depth_bias: db,
            });
        }
        out.slot[i] = Some(out.entries.len() as u32);
        out.entries.push(e);
    }
    out
}

/// Whether a world-space box intersects the frustum of a (reversed-Z, Metal-style)
/// clip matrix.
#[must_use]
pub fn box_in_frustum(clip: Mat4, lo: Vec3, hi: Vec3) -> bool {
    let r = [clip.row(0), clip.row(1), clip.row(2), clip.row(3)];
    let planes = [r[3] + r[0], r[3] - r[0], r[3] + r[1], r[3] - r[1], r[3] - r[2], r[2]];
    planes.iter().all(|p| {
        let v = Vec3::new(
            if p.x >= 0.0 { hi.x } else { lo.x },
            if p.y >= 0.0 { hi.y } else { lo.y },
            if p.z >= 0.0 { hi.z } else { lo.z },
        );
        p.x * v.x + p.y * v.y + p.z * v.z + p.w >= 0.0
    })
}

/// Whether a box touches a sphere.
#[must_use]
pub fn box_in_sphere(lo: Vec3, hi: Vec3, c: Vec3, r: f32) -> bool {
    let d = c.clamp(lo, hi) - c;
    d.length_squared() <= r * r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_rects_packs_in_order() {
        let mut p = MaxRects::new(1024, 1024);
        let a = p.insert(512, 512).unwrap();
        let b = p.insert(512, 512).unwrap();
        let c = p.insert(256, 256).unwrap();
        assert_eq!((a.x, a.y), (0.0, 0.0));
        // Splitting pushes the strip below first, then the one to the right; both fit with
        // the same long side, and ties keep the earlier free rectangle.
        assert_eq!((b.x, b.y), (0.0, 512.0));
        assert!(c.x >= 0.0 && c.y >= 0.0);
        // Nothing overlaps.
        let r = [a, b, c];
        for i in 0..3 {
            for j in i + 1..3 {
                let (p, q) = (r[i], r[j]);
                assert!(
                    p.x + p.w <= q.x || q.x + q.w <= p.x || p.y + p.h <= q.y || q.y + q.h <= p.y
                );
            }
        }
        let mut full = MaxRects::new(128, 128);
        assert!(full.insert(128, 128).is_some());
        assert!(full.insert(128, 128).is_none());
    }

    #[test]
    fn punctual_resolution_is_quantised() {
        let cam = CameraInfo { width: 2560, height: 1440, view_proj: Mat4::IDENTITY };
        assert_eq!(punctual_resolution(&cam, [0.0, 0.0, 1.0, 1.0], false, 512), 512);
        // 10% of the width = 256 px -> 256; a point light asks for 4x -> 1024 -> 512.
        assert_eq!(punctual_resolution(&cam, [0.0, 0.0, 0.1, 0.05], false, 512), 256);
        assert_eq!(punctual_resolution(&cam, [0.0, 0.0, 0.1, 0.05], true, 512), 512);
        assert_eq!(punctual_resolution(&cam, [0.0, 0.0, 0.01, 0.01], false, 512), 128);
        // 200 px -> trunc(200/128)*128 = 128.
        assert_eq!(punctual_resolution(&cam, [0.0, 0.0, 200.0 / 2560.0, 0.0], false, 512), 128);
    }

    #[test]
    fn guard_angle_shrinks_with_resolution_and_is_capped() {
        let g512 = guard_angle(90.0, 512.0, 5.0, 4.0, 90.0);
        let g128 = guard_angle(90.0, 128.0, 5.0, 4.0, 90.0);
        assert!(g128 > g512 && g512 > 0.0);
        assert_eq!(guard_angle(90.0, 128.0, 5.0, 4.0, 1.0), 1.0);
    }

    #[test]
    fn gpu_projection_matches_the_route_export() {
        // Route point 0: projection -> gpu_projection_render_into_texture.
        let proj = Mat4::from_cols(
            Vec4::new(2.099_278_5, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 3.732_050_7, 0.0, 0.0),
            Vec4::new(0.0, 0.0, -1.002_002, -1.0),
            Vec4::new(0.0, 0.0, -2.002_002, 0.0),
        );
        let g = gpu_projection(proj);
        assert!((g.y_axis.y + 3.732_050_7).abs() < 1e-5);
        assert!((g.z_axis.z - 0.001_001_000_4).abs() < 1e-6);
        assert!((g.w_axis.z - 1.001_001).abs() < 1e-5);
        assert_eq!(g.z_axis.w, -1.0);
    }

    #[test]
    fn tetrahedral_faces_cover_their_own_direction() {
        // A point along FaceVectors[f] from the light must land inside face f's triangle:
        // both clip distances (-(dot(xy, clip.xy) + clip.z)) non-negative.
        let l = Light {
            enabled: true,
            active: true,
            kind: 2,
            range: 10.0,
            shadows: 2,
            shadow_near_plane: 0.2,
            matrix: Mat4::from_translation(Vec3::new(3.0, 4.0, 5.0)).to_cols_array(),
            ..Default::default()
        };
        let cam = CameraInfo { width: 2560, height: 1440, view_proj: Mat4::IDENTITY };
        let plan = plan(&Settings::default(), &cam, &[&l], &|_| true);
        assert_eq!(plan.entries.len(), 1);
        let e = &plan.entries[0];
        let clips = point_clips();
        for f in 0..4 {
            let m = plan.matrices[e.matrix_indices[f] as usize].world_to_shadow;
            let p = e.position + FACE_VECTORS[f] * 5.0;
            let c = m * p.extend(1.0);
            let ndc = c.truncate() / c.w;
            assert!(ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0, "face {f}: {ndc:?}");
            assert!((0.0..=1.0).contains(&ndc.z), "face {f} depth {ndc:?}");
            for k in 0..2 {
                let q = clips[f * 2 + k];
                let d = -(q.x * ndc.x + q.y * ndc.y + q.z);
                assert!(d >= -1e-4, "face {f} clip {k}: {d}");
            }
        }
    }

    #[test]
    fn spot_matrix_centres_its_axis_and_uses_reversed_z() {
        let l = Light {
            enabled: true,
            active: true,
            kind: 0,
            range: 20.0,
            spot_angle: 90.0,
            shadows: 2,
            shadow_near_plane: 0.2,
            matrix: Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)).to_cols_array(),
            ..Default::default()
        };
        let cam = CameraInfo { width: 2560, height: 1440, view_proj: Mat4::IDENTITY };
        let plan = plan(&Settings::default(), &cam, &[&l], &|_| true);
        let m = plan.matrices[0];
        // Light forward is +Z.
        let near = m.world_to_shadow * Vec4::new(1.0, 2.0, 3.5, 1.0);
        let far = m.world_to_shadow * Vec4::new(1.0, 2.0, 22.5, 1.0);
        assert!((near.x / near.w).abs() < 1e-5 && (near.y / near.w).abs() < 1e-5);
        assert!(near.z / near.w > far.z / far.w, "reversed Z: near deeper than far");
        assert!(near.z / near.w <= 1.0 && far.z / far.w >= 0.0);
        assert_eq!(m.light_direction, Vec3::new(0.0, 0.0, -1.0));
        // Biases are negative world-space offsets scaled by texels.
        assert!(m.depth_bias < 0.0 && m.normal_bias < 0.0);
        assert_eq!(plan.slot, vec![Some(0)]);
    }

    #[test]
    fn records_have_the_shader_strides() {
        assert_eq!(ShadowMatrix::default().bytes().len(), MATRIX_STRIDE);
        let e = Entry {
            light: 0,
            point: true,
            viewport: Rect::default(),
            flags: FLAG_POINT | FLAG_SOFT,
            matrix_indices: [4, 5, 6, 7],
            scale_offset: [0.125, 0.125, 0.5, 0.25],
            depth_bias: 2.6,
            near_plane: 0.2,
            range: 1.0,
            position: Vec3::ZERO,
            cull: vec![],
        };
        let b = e.data_bytes();
        assert_eq!(b.len(), DATA_STRIDE);
        assert_eq!(f32::from_le_bytes(b[4..8].try_into().unwrap()), 5.0);
        assert_eq!(f32::from_le_bytes(b[24..28].try_into().unwrap()), 0.5);
        assert_eq!(i32::from_le_bytes(b[32..36].try_into().unwrap()), 3);
    }

    #[test]
    fn frustum_and_sphere_tests() {
        let clip = gpu_projection(unity_perspective(60.0, 1.0, 0.1, 100.0))
            * Mat4::from_scale(Vec3::new(1.0, 1.0, -1.0));
        assert!(box_in_frustum(clip, Vec3::new(-1.0, -1.0, 5.0), Vec3::new(1.0, 1.0, 6.0)));
        assert!(!box_in_frustum(clip, Vec3::new(-1.0, -1.0, -6.0), Vec3::new(1.0, 1.0, -5.0)));
        assert!(!box_in_frustum(clip, Vec3::new(500.0, 0.0, 5.0), Vec3::new(501.0, 1.0, 6.0)));
        assert!(box_in_sphere(Vec3::ZERO, Vec3::ONE, Vec3::new(2.0, 0.5, 0.5), 1.0));
        assert!(!box_in_sphere(Vec3::ZERO, Vec3::ONE, Vec3::new(3.0, 0.5, 0.5), 1.0));
    }
}
