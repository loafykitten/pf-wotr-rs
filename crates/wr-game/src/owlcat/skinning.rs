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
use bevy::math::{Mat4, Vec3};
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
    time: f32,
    walk: Option<Walk>,
    scratch: Vec<f32>,
}

impl std::fmt::Debug for Skinning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Skinning")
            .field("units", &self.units.len())
            .field("draws", &self.draws.len())
            .field("time", &self.time)
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
        let capacity: usize = m
            .renderers
            .iter()
            .filter_map(|r| r.skin.as_ref())
            .map(|s| s.bones.len())
            .sum::<usize>()
            .max(1);
        let palette = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("skin palette"),
            size: (capacity * 64) as u64,
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
            });
        }
        Ok(Some(Self {
            pipeline,
            layout,
            palette,
            palette_capacity: u32::try_from(capacity)?,
            palette_used: 0,
            units,
            draws: Vec::new(),
            time: 0.0,
            walk: None,
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
        let pos = mesh.channels.first().copied().flatten();
        let Some(pos) = pos.filter(|c| c.format == 0 && c.dimension >= 3) else {
            anyhow::bail!("{}: positions are not float32x3", mesh.name);
        };
        let stream = usize::from(pos.stream);
        let stride = mesh.streams[stream].stride;
        ensure!(stride.is_multiple_of(4), "{}: stream stride {stride}", mesh.name);
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
        ensure!(base + bones <= self.palette_capacity, "palette overflow");
        self.palette_used += bones;
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&format!("{} (skinned)", r.path)),
            size: g.streams[stream].size(),
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
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: g.streams[stream].as_entire_binding(),
                },
                wgpu::BindGroupEntry { binding: 2, resource: skin_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: self.palette.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: out.as_entire_binding() },
            ],
        });
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

    fn walk_speed(&self, unit: &UnitRt) -> f32 {
        match &self.walk {
            Some(w) if w.unit == unit.id && self.time >= w.start && unit.walked < w.distance => {
                w.speed
            }
            _ => 0.0,
        }
    }

    /// Advances the animation to time `t` (in steps of [`STEP`]), poses every unit and uploads the
    /// skin matrices. Returns the world displacement of each caster box that moved.
    pub fn advance_to(
        &mut self,
        queue: &wgpu::Queue,
        pack: &Pack,
        t: f32,
    ) -> Vec<(usize, Vec3, Vec3)> {
        let m = &pack.meta;
        while self.time + 1e-6 < t {
            let dt = (t - self.time).min(STEP);
            for i in 0..self.units.len() {
                let speed = self.walk_speed(&self.units[i]);
                let remaining =
                    self.walk.as_ref().map_or(0.0, |w| w.distance - self.units[i].walked);
                let u = &mut self.units[i];
                u.player.set_speed(speed);
                u.player.update(&m.clips, dt);
                if speed > 0.0 {
                    u.walked += (speed * dt).min(remaining.max(0.0));
                }
            }
            self.time += dt;
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
            // The unit has walked `walked` metres along its forward: in its root space that is a
            // shift of the same vector through the inverse placement.
            let forward = u.placement.transform_vector3(Vec3::Z).normalize_or_zero();
            let world = forward * u.walked;
            let local = u.placement.inverse().transform_vector3(world);
            let pre = Mat4::from_translation(local).to_cols_array();
            let first = data.len();
            wr_anim::palette(&u.worlds, &d.bones, &d.bind, &pre, &mut data);
            debug_assert_eq!(data.len() - first, d.bones.len() * 16);
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
