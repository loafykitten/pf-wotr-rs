//! Skinned units: the animation of every unit in the pack and the GPU skinning of its renderers.
//!
//! A unit's bones are posed on the CPU each frame (`wr-anim`: clips and controllers sampled into a
//! rig's pose, then the skin matrices of each renderer). A compute pass then skins every
//! renderer's mesh: it copies the mesh's position stream (positions, normals and tangents
//! interleaved with whatever else that stream holds) into a buffer of the renderer's own and
//! replaces position, normal and tangent by the skinned ones. That buffer is what the G-buffer and
//! shadow caster draws bind as the vertex stream, so both passes see the animated mesh through
//! Owlcat's own vertex shaders, unchanged.
//!
//! Units that share a mesh (the four centipedes) skin it into a buffer each.

use anyhow::{Result, ensure};
use bevy::math::{Mat4, Quat, Vec3};
use wgpu::util::DeviceExt;
use wr_anim::loco::UnitPlayer;
use wr_assets::{Pack, Renderer};

use super::gpu::Gpu;

const WGSL: &str = r"
struct Params {
    vertex_count: u32,
    stride: u32,
    pos: u32,
    nrm: u32,
    tan: u32,
    palette: u32,
    pad0: u32,
    pad1: u32,
};
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> src: array<u32>;
@group(0) @binding(2) var<storage, read> skin: array<u32>;
@group(0) @binding(3) var<storage, read> palette: array<mat4x4<f32>>;
@group(0) @binding(4) var<storage, read_write> dst: array<u32>;

const NONE: u32 = 0xffffffffu;

fn load3(at: u32) -> vec3<f32> {
    return vec3<f32>(bitcast<f32>(src[at]), bitcast<f32>(src[at + 1u]), bitcast<f32>(src[at + 2u]));
}

fn store3(at: u32, v: vec3<f32>) {
    dst[at] = bitcast<u32>(v.x);
    dst[at + 1u] = bitcast<u32>(v.y);
    dst[at + 2u] = bitcast<u32>(v.z);
}

@compute @workgroup_size(64)
fn skin_vertices(@builtin(global_invocation_id) gid: vec3<u32>) {
    let v = gid.x;
    if (v >= p.vertex_count) {
        return;
    }
    let base = v * p.stride;
    for (var i = 0u; i < p.stride; i = i + 1u) {
        dst[base + i] = src[base + i];
    }
    let sb = v * 8u;
    let pos = load3(base + p.pos);
    var nrm = vec3<f32>(0.0);
    var tan = vec3<f32>(0.0);
    if (p.nrm != NONE) { nrm = load3(base + p.nrm); }
    if (p.tan != NONE) { tan = load3(base + p.tan); }
    var sp = vec3<f32>(0.0);
    var sn = vec3<f32>(0.0);
    var st = vec3<f32>(0.0);
    var total = 0.0;
    for (var k = 0u; k < 4u; k = k + 1u) {
        let w = bitcast<f32>(skin[sb + k]);
        if (w <= 0.0) {
            continue;
        }
        let m = palette[p.palette + skin[sb + 4u + k]];
        sp = sp + (m * vec4<f32>(pos, 1.0)).xyz * w;
        sn = sn + (m * vec4<f32>(nrm, 0.0)).xyz * w;
        st = st + (m * vec4<f32>(tan, 0.0)).xyz * w;
        total = total + w;
    }
    if (total <= 0.0) {
        return;
    }
    store3(base + p.pos, sp / total);
    if (p.nrm != NONE) {
        let l = length(sn);
        store3(base + p.nrm, select(sn, sn / l, l > 0.0));
    }
    if (p.tan != NONE) {
        let l = length(st);
        store3(base + p.tan, select(st, st / l, l > 0.0));
    }
}
";

/// Seconds the animation advances per step: the game's frame length at 60 Hz, so a state machine
/// sees the same cadence whatever the capture rate.
pub const STEP: f32 = 1.0 / 60.0;

/// A unit walking a straight line: a demonstration script, not the movement agent.
#[derive(Debug, Clone, PartialEq)]
pub struct Walk {
    /// The unit's id in the spec.
    pub unit: String,
    /// When it sets off (seconds of animation time) and how fast (metres per second).
    pub start: f32,
    pub speed: f32,
    /// How far it goes before it stops (metres).
    pub distance: f32,
}

/// What a driver (the sim's timeline) says one unit is doing at one frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrivePose {
    /// World position (metres) and facing (degrees about +Y, Unity's convention).
    pub position: Vec3,
    pub yaw: f32,
    /// Metres per second it is seen to move: what `UnitAnimationManager.Speed` is fed.
    pub speed: f32,
    /// The unit is in the game: a unit that is not is not drawn.
    pub in_game: bool,
    /// Conscious and not prone. The player has no clip for anything else yet, so a unit that is
    /// down keeps the pose it had (its clock stops).
    pub up: bool,
    /// In combat (the focus of a following camera).
    pub in_combat: bool,
}

/// One unit's poses, frame by frame, from frame `first`.
#[derive(Debug, Clone, PartialEq)]
pub struct DriveTrack {
    /// The unit's id in the spec.
    pub unit: String,
    pub first: usize,
    pub poses: Vec<DrivePose>,
}

/// The units driven from outside: one pose per unit per frame of 1/60 s.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Drive {
    pub frames: usize,
    pub tracks: Vec<DriveTrack>,
}

impl Drive {
    fn pose(&self, track: usize, frame: usize) -> Option<&DrivePose> {
        let t = &self.tracks[track];
        let last = t.first + t.poses.len();
        // Before its first sample a unit stands where it first was; after the last it stays.
        t.poses.get(frame.clamp(t.first, last.saturating_sub(1)).checked_sub(t.first)?)
    }
}

struct UnitRt {
    id: String,
    player: UnitPlayer,
    rig: usize,
    /// The unit's placement (`T R S`) in the world, from its first skinned renderer.
    placement: Mat4,
    /// Bone world matrices of the current pose, relative to the unit's root space.
    worlds: Vec<[f32; 16]>,
    /// Metres walked along the unit's forward so far.
    walked: f32,
    /// Its track in the drive, if it has one.
    track: Option<usize>,
    /// The placement turns with the driver's facing (not for a view that forbids rotation).
    turns: bool,
    /// Its placement now: the delta from `placement` is folded into the skin matrices.
    now: Mat4,
}

/// A caster slot of a skinned renderer: its index and bounds at the start.
#[derive(Debug, Clone, Copy)]
pub struct CasterBox {
    pub index: usize,
    pub lo: Vec3,
    pub hi: Vec3,
}

struct SkinDraw {
    unit: usize,
    bones: Vec<u32>,
    bind: Vec<[f32; 16]>,
    bind_group: wgpu::BindGroup,
    vertex_count: u32,
    casters: Vec<CasterBox>,
}

/// The animation and skinning of the pack's units.
pub struct Skinning {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
    palette: wgpu::Buffer,
    palette_capacity: u32,
    palette_used: u32,
    units: Vec<UnitRt>,
    draws: Vec<SkinDraw>,
    /// The animation clock in 1/60 s ticks.
    tick: u64,
    max_groups: u32,
    max_binding: u64,
    walk: Option<Walk>,
    drive: Option<Drive>,
    scratch: Vec<f32>,
}

impl std::fmt::Debug for Skinning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Skinning")
            .field("units", &self.units.len())
            .field("draws", &self.draws.len())
            .field("tick", &self.tick)
            .finish_non_exhaustive()
    }
}

/// The skinning compute pipeline and its bind group layout: parameters (uniform), the source
/// stream, the skin, the palette (storage, read) and the destination stream (storage, write).
fn skin_pipeline(device: &wgpu::Device) -> (wgpu::ComputePipeline, wgpu::BindGroupLayout) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("skin"),
        source: wgpu::ShaderSource::Wgsl(WGSL.into()),
    });
    let entry = |binding: u32, read_only: bool, uniform: bool| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: if uniform {
                wgpu::BufferBindingType::Uniform
            } else {
                wgpu::BufferBindingType::Storage { read_only }
            },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("skin"),
        entries: &[
            entry(0, true, true),
            entry(1, true, false),
            entry(2, true, false),
            entry(3, true, false),
            entry(4, false, false),
        ],
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("skin"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("skin"),
        layout: Some(&pl),
        module: &module,
        entry_point: Some("skin_vertices"),
        compilation_options: Default::default(),
        cache: None,
    });
    (pipeline, layout)
}

/// A unit's start phase in `[0, 1)`: the game draws it with `Random.value`; the port takes a
/// fixed value of the unit's id so a capture repeats.
#[must_use]
pub fn phase_of(id: &str) -> f32 {
    (wr_assets::fnv1a64(id.as_bytes()) >> 40) as f32 / (1u64 << 24) as f32
}

/// The axis-aligned box of `lo..hi` moved by `m`.
fn transform_box(m: &Mat4, lo: Vec3, hi: Vec3) -> (Vec3, Vec3) {
    let (mut a, mut b) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for i in 0..8 {
        let corner = Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        );
        let p = m.transform_point3(corner);
        a = a.min(p);
        b = b.max(p);
    }
    (a, b)
}

fn mat(m: &[f32; 16]) -> Mat4 {
    Mat4::from_cols_array(m)
}

impl Skinning {
    /// Prepares the units of the pack. Returns `None` when the pack animates nothing.
    ///
    /// # Errors
    ///
    /// A unit whose clips or rig don't fit together.
    pub fn new(device: &wgpu::Device, pack: &Pack) -> Result<Option<Self>> {
        let m = &pack.meta;
        if m.animations.is_empty() {
            return Ok(None);
        }
        let limits = device.limits();
        let capacity: usize = m
            .renderers
            .iter()
            .filter_map(|r| r.skin.as_ref())
            .map(|s| s.bones.len())
            .sum::<usize>()
            .max(1);
        let palette_bytes = u64::try_from(capacity)?
            .checked_mul(64)
            .filter(|b| *b <= limits.max_storage_buffer_binding_size);
        let Some(palette_bytes) = palette_bytes else {
            anyhow::bail!("{capacity} skin matrices don't fit a storage binding");
        };
        let palette_capacity = u32::try_from(capacity)?;
        let palette = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("skin palette"),
            size: palette_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (pipeline, layout) = skin_pipeline(device);
        let mut units = Vec::new();
        for a in &m.animations {
            let rig = &m.rigs[a.rig as usize];
            let player = UnitPlayer::new(a, rig, &m.clips, &m.controllers, phase_of(&a.unit))?;
            units.push(UnitRt {
                id: a.unit.clone(),
                player,
                rig: a.rig as usize,
                placement: Mat4::IDENTITY,
                worlds: Vec::new(),
                walked: 0.0,
                track: None,
                turns: true,
                now: Mat4::IDENTITY,
            });
        }
        Ok(Some(Self {
            pipeline,
            layout,
            palette,
            palette_capacity,
            palette_used: 0,
            units,
            draws: Vec::new(),
            tick: 0,
            max_groups: limits.max_compute_workgroups_per_dimension,
            max_binding: limits.max_storage_buffer_binding_size,
            walk: None,
            drive: None,
            scratch: Vec::new(),
        }))
    }

    /// The units' ids, for choosing a walker.
    #[must_use]
    pub fn unit_ids(&self) -> Vec<&str> {
        self.units.iter().map(|u| u.id.as_str()).collect()
    }

    /// Registers a skinned renderer and returns the buffer that replaces its position stream
    /// (the stream's index too) in its draws.
    ///
    /// # Errors
    ///
    /// A mesh the skin pass can't read: positions that aren't float32x3 or a stream that isn't
    /// whole words.
    pub fn add_draw(
        &mut self,
        device: &wgpu::Device,
        pack: &Pack,
        gpu: &Gpu,
        r: &Renderer,
    ) -> Result<Option<(usize, wgpu::Buffer)>> {
        let Some(sk) = &r.skin else { return Ok(None) };
        let mesh = &pack.meta.meshes[r.mesh as usize];
        let g = &gpu.meshes[r.mesh as usize];
        let Some(skin_buf) = &g.skin else { return Ok(None) };
        let Some(pos) =
            mesh.channels.first().copied().flatten().filter(|c| c.format == 0 && c.dimension >= 3)
        else {
            anyhow::bail!("{}: positions are not float32x3", mesh.name);
        };
        let stream = usize::from(pos.stream);
        let (Some(src), Some(vs)) = (g.streams.get(stream), mesh.streams.get(stream)) else {
            anyhow::bail!("{}: position stream {stream} missing", mesh.name);
        };
        let stride = vs.stride;
        ensure!(stride.is_multiple_of(4), "{}: stream stride {stride}", mesh.name);
        ensure!(
            src.size() <= self.max_binding && skin_buf.size() <= self.max_binding,
            "{}: stream of {} bytes exceeds a storage binding",
            mesh.name,
            src.size()
        );
        ensure!(
            mesh.vertex_count.div_ceil(64) <= self.max_groups,
            "{}: {} vertices need more workgroups than the device dispatches",
            mesh.name,
            mesh.vertex_count
        );
        let word = |c: Option<wr_assets::Channel>, dim: u8| -> Result<u32> {
            match c {
                Some(c)
                    if usize::from(c.stream) == stream && c.format == 0 && c.dimension >= dim =>
                {
                    ensure!(c.offset % 4 == 0, "{}: unaligned channel", mesh.name);
                    Ok(u32::from(c.offset) / 4)
                }
                _ => Ok(u32::MAX),
            }
        };
        let nrm = word(mesh.channels.get(1).copied().flatten(), 3)?;
        let tan = word(mesh.channels.get(2).copied().flatten(), 3)?;
        let base = self.palette_used;
        let bones = u32::try_from(sk.bones.len())?;
        ensure!(
            base.checked_add(bones).is_some_and(|e| e <= self.palette_capacity),
            "palette overflow"
        );
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("{} (skinned)", r.path)),
            size: src.size(),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let params: [u32; 8] =
            [mesh.vertex_count, stride / 4, u32::from(pos.offset) / 4, nrm, tan, base, 0, 0];
        let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("skin params"),
            contents: &params.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("skin"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: src.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: skin_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: self.palette.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: out.as_entire_binding() },
            ],
        });
        // Nothing below can fail: the palette slice is taken only now, so draws and palette stay in
        // the same order.
        self.palette_used += bones;
        let unit = sk.animation as usize;
        self.units[unit].placement = mat(&r.matrix);
        self.draws.push(SkinDraw {
            unit,
            bones: sk.bones.clone(),
            bind: sk.bind_poses.clone(),
            bind_group,
            vertex_count: mesh.vertex_count,
            casters: Vec::new(),
        });
        Ok(Some((stream, out)))
    }

    /// Notes the shadow caster slots of the draw added last (their bounds follow a walking unit).
    pub fn set_casters(&mut self, casters: Vec<CasterBox>) {
        if let Some(d) = self.draws.last_mut() {
            d.casters = casters;
        }
    }

    pub fn set_walk(&mut self, walk: Option<Walk>) {
        self.walk = walk;
    }

    /// Hands the units' motion to a driver (the sim's timeline): each unit with a track in `drive`
    /// goes where its poses say, facing as they say, its locomotion fed the speed they give.
    /// Returns the ids of tracks that have no unit in the pack.
    pub fn set_drive(&mut self, drive: Drive) -> Vec<String> {
        let mut missing = Vec::new();
        for u in &mut self.units {
            u.track = None;
        }
        for (ti, t) in drive.tracks.iter().enumerate() {
            let Some(u) = self.units.iter_mut().find(|u| u.id == t.unit) else {
                missing.push(t.unit.clone());
                continue;
            };
            u.track = Some(ti);
            // A view that forbids rotation keeps its prefab's facing whatever the unit's: its
            // placement doesn't turn with the first pose.
            if let Some(p) = t.poses.first() {
                let (_, rot, _) = u.placement.to_scale_rotation_translation();
                let yaw = rot.to_euler(bevy::math::EulerRot::YXZ).0.to_degrees();
                let diff = (yaw - p.yaw + 180.0).rem_euclid(360.0) - 180.0;
                u.turns = diff.abs() < 1.0;
            }
        }
        self.drive = Some(drive);
        missing
    }

    /// Where the driven units are at the animation clock's frame: the centre and radius (metres,
    /// in the plane) of the units a camera should frame: those in combat and up, else every unit
    /// that is up. `None` without a drive or a unit.
    #[must_use]
    pub fn focus(&self) -> Option<(Vec3, f32)> {
        let d = self.drive.as_ref()?;
        let frame = usize::try_from(self.tick).ok()?;
        let poses: Vec<&DrivePose> = self
            .units
            .iter()
            .filter_map(|u| d.pose(u.track?, frame))
            .filter(|p| p.in_game && p.up)
            .collect();
        let fight: Vec<&&DrivePose> = poses.iter().filter(|p| p.in_combat).collect();
        let pts: Vec<Vec3> = if fight.is_empty() {
            poses.iter().map(|p| p.position).collect()
        } else {
            fight.iter().map(|p| p.position).collect()
        };
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in &pts {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        (!pts.is_empty()).then(|| {
            let c = (lo + hi) * 0.5;
            (c, (hi - lo).truncate().length() * 0.5)
        })
    }

    fn walk_speed(&self, unit: &UnitRt) -> f32 {
        let time = self.tick as f32 * STEP;
        match &self.walk {
            Some(w) if w.unit == unit.id && time >= w.start && unit.walked < w.distance => w.speed,
            _ => 0.0,
        }
    }

    /// The speed a unit's locomotion is fed at the clock's frame, and whether its clock runs: the
    /// driver's pose, else the walk script.
    fn drive_speed(&self, u: &UnitRt) -> (f32, bool) {
        match (&self.drive, u.track) {
            (Some(d), Some(t)) => {
                let frame = usize::try_from(self.tick).unwrap_or(usize::MAX);
                match d.pose(t, frame) {
                    Some(p) if p.in_game && p.up => (p.speed, true),
                    Some(_) => (0.0, false),
                    None => (0.0, true),
                }
            }
            _ => (self.walk_speed(u), true),
        }
    }

    /// A driven unit's placement at the clock's frame (`T R S` of the driver's pose, the scale of
    /// the unit's own placement): `None` when it is not in the game.
    fn driven_placement(&self, u: &UnitRt) -> Option<Mat4> {
        let (Some(d), Some(t)) = (&self.drive, u.track) else { return Some(u.placement) };
        let p = d.pose(t, usize::try_from(self.tick).ok()?)?;
        if !p.in_game {
            return None;
        }
        let (scale, rot, _) = u.placement.to_scale_rotation_translation();
        let rot = if u.turns { Quat::from_rotation_y(p.yaw.to_radians()) } else { rot };
        Some(Mat4::from_scale_rotation_translation(scale, rot, p.position))
    }

    /// Advances the animation to time `t`, snapped to the nearest tick of the 60 Hz grid the game
    /// steps on (so a capture rate that is a multiple of 1/60 s lands on ticks exactly, and the
    /// simulation doesn't depend on it); the clock never goes back. Then poses every unit and
    /// uploads the skin matrices. Returns the world bounds of each caster box that moved.
    pub fn advance_to(
        &mut self,
        queue: &wgpu::Queue,
        pack: &Pack,
        t: f32,
    ) -> Vec<(usize, Vec3, Vec3)> {
        let m = &pack.meta;
        let target = if t.is_finite() { (t.clamp(0.0, 1.0e6) * 60.0).round() as u64 } else { 0 };
        while self.tick < target {
            for i in 0..self.units.len() {
                let (speed, runs) = self.drive_speed(&self.units[i]);
                let remaining =
                    self.walk.as_ref().map_or(0.0, |w| w.distance - self.units[i].walked);
                let u = &mut self.units[i];
                if !runs {
                    continue;
                }
                u.player.set_speed(speed);
                u.player.update(&m.clips, STEP);
                if speed > 0.0 && u.track.is_none() {
                    u.walked += (speed * STEP).min(remaining.max(0.0));
                }
            }
            self.tick += 1;
        }
        // Where the driven units stand now.
        for i in 0..self.units.len() {
            if self.units[i].track.is_some() {
                self.units[i].now = self.driven_placement(&self.units[i]).unwrap_or(Mat4::ZERO);
            }
        }
        // Poses.
        for u in &mut self.units {
            let rig = &m.rigs[u.rig];
            let pose = u.player.pose(&m.clips, rig, &mut self.scratch);
            rig.world_arrays(&pose, &mut u.worlds);
        }
        // Palettes.
        let mut data: Vec<f32> = Vec::new();
        let mut moved = Vec::new();
        for d in &self.draws {
            let u = &self.units[d.unit];
            if u.track.is_some() {
                // The unit's root space goes where the driver puts it: the skin matrices carry
                // the change from the placement the draw constants still hold. A unit that
                // is out of the game collapses to nothing.
                let to = u.now;
                let pre = if to == Mat4::ZERO { Mat4::ZERO } else { u.placement.inverse() * to };
                wr_anim::palette(&u.worlds, &d.bones, &d.bind, &pre.to_cols_array(), &mut data);
                let delta = to * u.placement.inverse();
                for c in &d.casters {
                    let (lo, hi) = if to == Mat4::ZERO {
                        (Vec3::ZERO, Vec3::ZERO)
                    } else {
                        transform_box(&delta, c.lo, c.hi)
                    };
                    moved.push((c.index, lo, hi));
                }
                continue;
            }
            // The unit has walked `walked` metres along its forward: in its root space that is a
            // shift of the same vector through the inverse placement.
            let forward = u.placement.transform_vector3(Vec3::Z).normalize_or_zero();
            let world = forward * u.walked;
            let local = u.placement.inverse().transform_vector3(world);
            let pre = Mat4::from_translation(local).to_cols_array();
            wr_anim::palette(&u.worlds, &d.bones, &d.bind, &pre, &mut data);
            if u.walked > 0.0 {
                for c in &d.casters {
                    moved.push((c.index, c.lo + world, c.hi + world));
                }
            }
        }
        // Draws were added in palette order, so the buffer is one contiguous write.
        queue.write_buffer(
            &self.palette,
            0,
            &data.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
        );
        moved
    }

    /// Skins every renderer: one compute pass.
    pub fn encode(&self, enc: &mut wgpu::CommandEncoder) {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("Skinning"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.pipeline);
        for d in &self.draws {
            pass.set_bind_group(0, &d.bind_group, &[]);
            pass.dispatch_workgroups(d.vertex_count.div_ceil(64), 1, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(f: &[f32]) -> Vec<u8> {
        f.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    /// Runs the skinning shader on a tiny mesh and returns the destination stream's floats.
    /// `None` when the machine has no GPU adapter.
    fn gpu_skin(
        stream: &[f32],
        stride_words: u32,
        skin: &[u8],
        palette: &[f32],
        params: [u32; 8],
    ) -> Option<Vec<f32>> {
        let instance = wgpu::Instance::default();
        let adapter = bevy::tasks::block_on(instance.request_adapter(&Default::default())).ok()?;
        let (device, queue) =
            bevy::tasks::block_on(adapter.request_device(&Default::default())).ok()?;
        let (pipeline, layout) = skin_pipeline(&device);
        let buf = |data: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: data,
                usage,
            })
        };
        let size = (stream.len() * 4) as u64;
        let dst = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params_b = buf(
            &params.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
            wgpu::BufferUsages::UNIFORM,
        );
        let src = buf(&words(stream), wgpu::BufferUsages::STORAGE);
        let skin_b = buf(skin, wgpu::BufferUsages::STORAGE);
        let pal = buf(&words(palette), wgpu::BufferUsages::STORAGE);
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params_b.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: src.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: skin_b.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: pal.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: dst.as_entire_binding() },
            ],
        });
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(params[0].div_ceil(64), 1, 1);
        }
        enc.copy_buffer_to_buffer(&dst, 0, &read, 0, size);
        queue.submit([enc.finish()]);
        let slice = read.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).ok()?;
        let data = slice.get_mapped_range().ok()?;
        let _ = stride_words;
        Some(data.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect())
    }

    /// The compute shader moves positions with the weighted palette, rotates normals and tangents by
    /// its linear part, leaves the other attributes of the stream alone, and keeps a vertex with no
    /// weight where it was.
    #[test]
    fn the_skin_shader_matches_the_cpu_formula() {
        // Stream: position (3), normal (3), tangent (4), one UV pair (2): 12 words per vertex.
        let verts: [[f32; 12]; 3] = [
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.25, 0.75],
            [0.0, 2.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, -1.0, 0.5, 0.5],
            [5.0, 5.0, 5.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.9, 0.1],
        ];
        let stream: Vec<f32> = verts.iter().flatten().copied().collect();
        // Bone 0 shifts up by 4; bone 1 turns a quarter about Z.
        let up = Mat4::from_translation(Vec3::new(0.0, 4.0, 0.0));
        let turn = Mat4::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let mut palette = up.to_cols_array().to_vec();
        palette.extend(turn.to_cols_array());
        // The skin: vertex 0 on bone 1; vertex 1 half bone 0, half bone 1; vertex 2 none.
        let skin_rows: [([f32; 4], [u32; 4]); 3] = [
            ([1.0, 0.0, 0.0, 0.0], [1, 0, 0, 0]),
            ([0.5, 0.5, 0.0, 0.0], [0, 1, 0, 0]),
            ([0.0; 4], [0; 4]),
        ];
        let skin: Vec<u8> = skin_rows
            .iter()
            .flat_map(|(w, i)| {
                w.iter()
                    .flat_map(|x| x.to_le_bytes())
                    .chain(i.iter().flat_map(|x| x.to_le_bytes()))
                    .collect::<Vec<u8>>()
            })
            .collect();
        let params = [3, 12, 0, 3, 6, 0, 0, 0];
        let Some(out) = gpu_skin(&stream, 12, &skin, &palette, params) else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let v = |i: usize, o: usize| {
            Vec3::new(out[i * 12 + o], out[i * 12 + o + 1], out[i * 12 + o + 2])
        };
        // Vertex 0: (1,0,0) turned a quarter about Z is (0,1,0); its normal (0,1,0) becomes (-1,0,0).
        assert!((v(0, 0) - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-5, "{:?}", v(0, 0));
        assert!((v(0, 3) - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-5, "{:?}", v(0, 3));
        // Vertex 1: half of (0,6,0) and half of (-2,0,0); the normal (0,0,1) is unchanged by both.
        assert!((v(1, 0) - Vec3::new(-1.0, 3.0, 0.0)).length() < 1e-5, "{:?}", v(1, 0));
        assert!((v(1, 3) - Vec3::new(0.0, 0.0, 1.0)).length() < 1e-5);
        // The tangent's w and the UVs are copied untouched.
        assert_eq!(out[9], 1.0);
        assert_eq!((out[10], out[11]), (0.25, 0.75));
        assert_eq!(out[12 + 9], -1.0);
        // No weight: the vertex stays put.
        assert_eq!(v(2, 0), Vec3::new(5.0, 5.0, 5.0));
        assert_eq!(v(2, 3), Vec3::new(1.0, 0.0, 0.0));
    }

    fn pose(x: f32) -> DrivePose {
        DrivePose {
            position: Vec3::new(x, 0.0, 0.0),
            yaw: 0.0,
            speed: 0.0,
            in_game: true,
            up: true,
            in_combat: false,
        }
    }

    /// A unit that comes into the game late stands at its first pose before it, and at its last
    /// after the timeline's end.
    #[test]
    fn a_drive_holds_the_first_and_last_poses_outside_a_track() {
        let d = Drive {
            frames: 6,
            tracks: vec![DriveTrack {
                unit: "u".into(),
                first: 2,
                poses: vec![pose(1.0), pose(2.0)],
            }],
        };
        let x = |f| d.pose(0, f).map(|p| p.position.x);
        assert_eq!(
            [x(0), x(2), x(3), x(5), x(999)],
            [Some(1.0), Some(1.0), Some(2.0), Some(2.0), Some(2.0)]
        );
        let empty = Drive {
            frames: 1,
            tracks: vec![DriveTrack { unit: "u".into(), first: 0, poses: vec![] }],
        };
        assert!(empty.pose(0, 0).is_none());
    }

    /// A box turned a quarter about Y covers the turned corners.
    #[test]
    fn a_moved_box_is_the_box_of_its_moved_corners() {
        let m = Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0))
            * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let (lo, hi) = transform_box(&m, Vec3::new(-1.0, 0.0, -2.0), Vec3::new(1.0, 3.0, 2.0));
        assert!((lo - Vec3::new(8.0, 0.0, -1.0)).length() < 1e-5, "{lo:?}");
        assert!((hi - Vec3::new(12.0, 3.0, 1.0)).length() < 1e-5, "{hi:?}");
    }

    #[test]
    fn phases_are_stable_and_in_range() {
        for id in ["3708", "35fffa0a-06ba-4cba-be40-7914924be190", ""] {
            let p = phase_of(id);
            assert!((0.0..1.0).contains(&p), "{p}");
            assert_eq!(p, phase_of(id));
        }
        assert_ne!(phase_of("3708"), phase_of("3709"));
    }
}
