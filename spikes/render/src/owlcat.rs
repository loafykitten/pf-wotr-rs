//! `owlcat`: draw with real Owlcat shaders, unmodified, through wgpu's MSL passthrough.
//!
//! Wrath's Metal player ships each shader variant as plain MSL (Unity's HLSLcc output), which
//! `cache/poc0-render/scripts/extract_msl.py` dumps from the game's `shaders` bundle into
//! `cache/` (never committed). This experiment loads one vertex/fragment pair, derives a wgpu
//! bind group layout from the MSL's explicit slots, fills Unity's constant buffers by member
//! name, renders a sphere into Owlcat's six-target G-buffer with a BC1 base map, reads the
//! targets back, checks them numerically, and writes PNGs to `cache/`. `owlcat-bench` builds many
//! real variants to measure Metal pipeline creation cost.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use bevy::math::{Mat4, Vec3, Vec4};

use crate::msl::{self, Interface, Resource, Stage, TexKind};
use crate::{gpu, opt, stats};

const W: u32 = 512;
const H: u32 = 512;

pub fn default_msl_dir() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../cache/poc0-render/shaders/msl-all/Owlcat_Lit"
    ))
}

/// Owlcat's deferred G-buffer formats (GBuffer.cs; survey §2): albedo (sRGB), normals, camera
/// colour (HDR), baked GI (R11G11B10F), shadowmask, translucency.
pub fn gbuffer_formats(n: u32) -> Vec<wgpu::TextureFormat> {
    use wgpu::TextureFormat as F;
    match n {
        6 => vec![
            F::Rgba8UnormSrgb,
            F::Rgba8Unorm,
            F::Rgba16Float,
            F::Rg11b10Ufloat,
            F::Rgba8Unorm,
            F::Rgba8Unorm,
        ],
        3 => vec![F::Rgba8Unorm, F::Rg11b10Ufloat, F::Rgba8Unorm], // forward prepass
        n => vec![F::Rgba16Float; n as usize],
    }
}

/// Values for Unity built-ins and Owlcat material properties, written into every constant
/// buffer that has a member of that name.
fn params(model: Mat4, view: Mat4, proj: Mat4, cam: Vec3) -> HashMap<&'static str, Vec<f32>> {
    let m = |x: Mat4| x.to_cols_array().to_vec();
    let v4 = |x: Vec4| x.to_array().to_vec();
    let near = 0.1f32;
    let far = 100.0f32;
    let mut p: HashMap<&'static str, Vec<f32>> = HashMap::new();
    p.insert("hlslcc_mtx4x4unity_ObjectToWorld", m(model));
    p.insert("hlslcc_mtx4x4unity_WorldToObject", m(model.inverse()));
    p.insert("hlslcc_mtx4x4unity_MatrixVP", m(proj * view));
    p.insert("hlslcc_mtx4x4unity_MatrixV", m(view));
    p.insert("hlslcc_mtx4x4unity_MatrixInvV", m(view.inverse()));
    p.insert("hlslcc_mtx4x4glstate_matrix_projection", m(proj));
    p.insert("hlslcc_mtx4x4_ViewProjMatrix", m(proj * view));
    p.insert("hlslcc_mtx4x4_NonJitteredViewProjMatrix", m(proj * view));
    p.insert("hlslcc_mtx4x4_PrevViewProjMatrix", m(proj * view));
    p.insert("hlslcc_mtx4x4_ViewMatrix", m(view));
    p.insert("hlslcc_mtx4x4_ProjMatrix", m(proj));
    p.insert("hlslcc_mtx4x4_InvViewProjMatrix", m((proj * view).inverse()));
    p.insert("hlslcc_mtx4x4_InvCameraViewProj", m((proj * view).inverse()));
    p.insert("hlslcc_mtx4x4_InvViewMatrix", m(view.inverse()));
    p.insert("hlslcc_mtx4x4_InvProjMatrix", m(proj.inverse()));
    p.insert("unity_WorldTransformParams", vec![0.0, 0.0, 0.0, 1.0]);
    p.insert("_WorldSpaceCameraPos", cam.to_array().to_vec());
    // Unity: x = 1 (no flip), y near, z far, w 1/far.
    p.insert("_ProjectionParams", vec![1.0, near, far, 1.0 / far]);
    p.insert("_ScreenParams", vec![W as f32, H as f32, 1.0 + 1.0 / W as f32, 1.0 + 1.0 / H as f32]);
    p.insert("_ScreenSize", vec![W as f32, H as f32, 1.0 / W as f32, 1.0 / H as f32]);
    // Reversed Z (UNITY_REVERSED_Z on Metal).
    p.insert("_ZBufferParams", vec![-1.0 + far / near, 1.0, (-1.0 + far / near) / far, 1.0 / far]);
    p.insert("_BaseMap_ST", vec![2.0, 1.0, 0.0, 0.0]);
    p.insert("_BaseColor", v4(Vec4::new(1.0, 1.0, 1.0, 1.0)));
    p.insert("_Roughness", vec![1.0]);
    p.insert("_Metallic", vec![1.0]);
    p.insert("_BumpScale", vec![1.0]);
    p.insert("_Thickness", vec![1.0]);
    p.insert("_TranslucencyColor", vec![1.0, 0.5, 0.25]);
    p.insert("_EmissionColor", vec![0.0, 0.0, 0.0, 1.0]);
    p.insert("_Cutoff", vec![0.5]);
    p.insert("_DoubleSidedConstants", vec![1.0, 1.0, -1.0, 0.0]);
    // Flat ambient SH (L0 only): ~0.2 grey.
    for (k, v) in [("unity_SHAr", 0.2f32), ("unity_SHAg", 0.2), ("unity_SHAb", 0.2)] {
        p.insert(k, vec![0.0, 0.0, 0.0, v]);
    }
    p
}

/// Right-handed view matrix (camera looks down -Z), as Unity's `unity_MatrixV`.
fn look_at_rh(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let f = (target - eye).normalize();
    let s = f.cross(up).normalize();
    let u = s.cross(f);
    Mat4::from_cols(
        Vec4::new(s.x, u.x, -f.x, 0.0),
        Vec4::new(s.y, u.y, -f.y, 0.0),
        Vec4::new(s.z, u.z, -f.z, 0.0),
        Vec4::new(-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
    )
}

/// Bytes per vertex: position, normal, tangent, uv (48), blend weights (16), blend indices (4).
const STRIDE: u64 = 68;

/// A UV sphere: position, normal, tangent (xyz + sign), uv, blend weights (1,0,0,0) and blend
/// indices (0,0,0,0 as bytes, stored in the last float slot).
fn sphere(rings: u32, segs: u32) -> (Vec<[f32; 17]>, Vec<u32>) {
    let mut v = Vec::new();
    for r in 0..=rings {
        let th = std::f32::consts::PI * r as f32 / rings as f32;
        for s in 0..=segs {
            let ph = 2.0 * std::f32::consts::PI * s as f32 / segs as f32;
            let n = Vec3::new(th.sin() * ph.cos(), th.cos(), th.sin() * ph.sin());
            let t = Vec3::new(-ph.sin(), 0.0, ph.cos());
            v.push([
                n.x,
                n.y,
                n.z,
                n.x,
                n.y,
                n.z,
                t.x,
                t.y,
                t.z,
                1.0,
                s as f32 / segs as f32,
                r as f32 / rings as f32,
                1.0,
                0.0,
                0.0,
                0.0,
                0.0,
            ]);
        }
    }
    let mut idx = Vec::new();
    for r in 0..rings {
        for s in 0..segs {
            let a = r * (segs + 1) + s;
            let b = a + segs + 1;
            idx.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
        }
    }
    (v, idx)
}

/// Encode a 2-colour BC1 checkerboard (8x8-texel squares).
fn bc1_checker(size: u32) -> Vec<u8> {
    let c0: u16 = 0xF800; // red   (RGB565)
    let c1: u16 = 0x07E0; // green
    let mut out = Vec::new();
    for by in 0..size / 4 {
        for bx in 0..size / 4 {
            let odd = ((bx / 2) + (by / 2)) % 2 == 1;
            // c0 > c1 selects 4-colour mode; index 0 = c0, 1 = c1 for the whole block.
            let idx: u32 = if odd { 0x5555_5555 } else { 0 };
            out.extend_from_slice(&c0.to_le_bytes());
            out.extend_from_slice(&c1.to_le_bytes());
            out.extend_from_slice(&idx.to_le_bytes());
        }
    }
    out
}

struct Built {
    layout: wgpu::BindGroupLayout,
    entries: Vec<(Stage, msl::Binding)>,
}

/// One bind group, entries ordered stage-by-stage and buffers→samplers→textures by slot, so
/// wgpu-hal's per-stage sequential slot counters reproduce the MSL's explicit indices.
fn build_layout(device: &wgpu::Device, stages: &[&Interface]) -> Result<Built, String> {
    let mut wl = Vec::new();
    let mut entries = Vec::new();
    for iface in stages {
        msl::check_contiguous(iface)?;
        let vis = match iface.stage {
            Stage::Vertex => wgpu::ShaderStages::VERTEX,
            Stage::Fragment => wgpu::ShaderStages::FRAGMENT,
            Stage::Compute => wgpu::ShaderStages::COMPUTE,
        };
        for b in &iface.bindings {
            let ty = match &b.res {
                Resource::Uniform { .. } => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                Resource::Storage { read_only, .. } => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: *read_only },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                Resource::Sampler => {
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
                }
                Resource::Texture { kind, .. } => {
                    let (view_dimension, depth) = tex_dims(*kind);
                    wgpu::BindingType::Texture {
                        sample_type: if depth {
                            wgpu::TextureSampleType::Depth
                        } else {
                            wgpu::TextureSampleType::Float { filterable: true }
                        },
                        view_dimension,
                        multisampled: false,
                    }
                }
            };
            wl.push(wgpu::BindGroupLayoutEntry {
                binding: wl.len() as u32,
                visibility: vis,
                ty,
                count: None,
            });
            entries.push((iface.stage, b.clone()));
        }
    }
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("owlcat"),
        entries: &wl,
    });
    Ok(Built { layout, entries })
}

fn tex_dims(k: TexKind) -> (wgpu::TextureViewDimension, bool) {
    use wgpu::TextureViewDimension as D;
    match k {
        TexKind::D2 => (D::D2, false),
        TexKind::D2Array => (D::D2Array, false),
        TexKind::D3 => (D::D3, false),
        TexKind::Cube => (D::Cube, false),
        TexKind::CubeArray => (D::CubeArray, false),
        TexKind::Depth2 => (D::D2, true),
        TexKind::Depth2Array => (D::D2Array, true),
    }
}

/// (format, offset into our interleaved vertex) for an HLSLcc input semantic.
fn vertex_attr(semantic: &str) -> Option<(wgpu::VertexFormat, u64)> {
    use wgpu::VertexFormat as F;
    Some(match semantic {
        // Unity feeds float3 positions; Metal's vertex fetch expands to float4 with w = 1.
        "POSITION0" => (F::Float32x3, 0),
        "NORMAL0" => (F::Float32x3, 12),
        "TANGENT0" => (F::Float32x4, 24),
        "TEXCOORD0" | "TEXCOORD1" | "TEXCOORD2" | "TEXCOORD3" => (F::Float32x2, 40),
        // No colour stream in our mesh: read the normal as a stand-in.
        "COLOR0" => (F::Float32x3, 12),
        // Skinned / PBD variants (uint4 indices in MSL; Metal widens uchar4 to uint4).
        "BLENDWEIGHTS0" => (F::Float32x4, 48),
        "BLENDINDICES0" => (F::Uint8x4, 64),
        _ => return None,
    })
}

fn passthrough_module(
    device: &wgpu::Device,
    label: &str,
    src: &str,
    entry: &str,
) -> wgpu::ShaderModule {
    // SAFETY: the MSL is the game's own compiled shader; we describe its bindings with a layout
    // derived from its explicit slot attributes (checked contiguous). wgpu cannot validate it.
    unsafe {
        device.create_shader_module_passthrough(wgpu::ShaderModuleDescriptorPassthrough {
            label: Some(label),
            entry_points: vec![wgpu::PassthroughShaderEntryPoint {
                name: entry.to_string().into(),
                workgroup_size: (0, 0, 0),
            }]
            .into(),
            msl: Some(src.into()),
            ..Default::default()
        })
    }
}

struct PipelineBuild {
    pipeline: wgpu::RenderPipeline,
    layout: Built,
    vs_ms: f64,
    fs_ms: f64,
    pipeline_ms: f64,
}

fn build_pipeline(
    device: &wgpu::Device,
    vs_src: &str,
    fs_src: &str,
    vs: &Interface,
    fs: &Interface,
) -> Result<PipelineBuild, String> {
    let layout = build_layout(device, &[vs, fs])?;
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("owlcat"),
        bind_group_layouts: &[Some(&layout.layout)],
        immediate_size: 0,
    });
    let t = Instant::now();
    let vm = passthrough_module(device, "owlcat-vs", vs_src, &vs.entry);
    let vs_ms = t.elapsed().as_secs_f64() * 1e3;
    let t = Instant::now();
    let fm = passthrough_module(device, "owlcat-fs", fs_src, &fs.entry);
    let fs_ms = t.elapsed().as_secs_f64() * 1e3;
    let mut attrs = Vec::new();
    for vi in &vs.vertex_inputs {
        let (format, offset) =
            vertex_attr(&vi.semantic).ok_or_else(|| format!("vertex semantic {}", vi.semantic))?;
        attrs.push(wgpu::VertexAttribute { format, offset, shader_location: vi.attribute });
    }
    let targets: Vec<Option<wgpu::ColorTargetState>> = gbuffer_formats(fs.color_outputs)
        .into_iter()
        .map(|format| {
            Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })
        })
        .collect();
    let t = Instant::now();
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("owlcat"),
        layout: Some(&pl),
        vertex: wgpu::VertexState {
            module: &vm,
            entry_point: Some(&vs.entry),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: STRIDE,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attrs,
            })],
        },
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::GreaterEqual), // reversed Z
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &fm,
            entry_point: Some(&fs.entry),
            compilation_options: Default::default(),
            targets: &targets,
        }),
        multiview_mask: None,
        cache: None,
    });
    let pipeline_ms = t.elapsed().as_secs_f64() * 1e3;
    Ok(PipelineBuild { pipeline, layout, vs_ms, fs_ms, pipeline_ms })
}

fn scope_error(device: &wgpu::Device) -> wgpu::ErrorScopeGuard {
    device.push_error_scope(wgpu::ErrorFilter::Validation)
}

/// A real Owlcat vertex/fragment pair, ready to draw a sphere into Owlcat's G-buffer on any
/// wgpu 30 device: the headless one here, or Bevy's `RenderDevice` (see `scene --owlcat`).
pub struct OwlcatDraw {
    pipeline: wgpu::RenderPipeline,
    bind_group: wgpu::BindGroup,
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    index_count: u32,
    targets: Vec<wgpu::Texture>,
    views: Vec<wgpu::TextureView>,
    depth_view: wgpu::TextureView,
    formats: Vec<wgpu::TextureFormat>,
    model: Mat4,
    view: Mat4,
    proj: Mat4,
    /// Creation timings and interface notes for the report.
    pub notes: String,
}

impl std::fmt::Debug for OwlcatDraw {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OwlcatDraw")
    }
}

impl OwlcatDraw {
    /// Build the pipeline (passthrough MSL), its bind group, the mesh and the G-buffer targets.
    /// The device needs `PASSTHROUGH_SHADERS`, `TEXTURE_COMPRESSION_BC` and
    /// `RG11B10UFLOAT_RENDERABLE`.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vs_src: &str,
        fs_src: &str,
    ) -> Result<Self, String> {
        let need = wgpu::Features::PASSTHROUGH_SHADERS
            | wgpu::Features::TEXTURE_COMPRESSION_BC
            | wgpu::Features::RG11B10UFLOAT_RENDERABLE;
        if !device.features().contains(need) {
            return Err(format!("device lacks {:?}", need - device.features()));
        }
        let vs = msl::parse(vs_src)?;
        let fs = msl::parse(fs_src)?;
        let mut notes = String::new();
        let _ = writeln!(
            notes,
            "vs: {} bindings, {} vertex inputs; fs: {} bindings, {} colour outputs",
            vs.bindings.len(),
            vs.vertex_inputs.len(),
            fs.bindings.len(),
            fs.color_outputs
        );
        let scope = scope_error(device);
        let built = build_pipeline(device, vs_src, fs_src, &vs, &fs)?;
        if let Some(e) = bevy::tasks::block_on(scope.pop()) {
            return Err(format!("pipeline validation: {e}"));
        }
        let _ = writeln!(
            notes,
            "create: vs module {:.2} ms, fs module {:.2} ms, render pipeline {:.2} ms",
            built.vs_ms, built.fs_ms, built.pipeline_ms
        );

        let cam = Vec3::new(0.0, 0.0, 3.0);
        let view = look_at_rh(cam, Vec3::ZERO, Vec3::Y);
        // Reversed-Z perspective (near -> 1, far -> 0), as Unity's GPU projection on Metal.
        let proj = bevy::math::proj::perspective(0.9, W as f32 / H as f32, 100.0, 0.1);
        let model = Mat4::from_rotation_y(0.4);
        let p = params(model, view, proj, cam);
        let mut written: Vec<String> = Vec::new();
        let mut buffers: Vec<wgpu::Buffer> = Vec::new();
        let mut textures: Vec<wgpu::TextureView> = Vec::new();
        let mut samplers: Vec<wgpu::Sampler> = Vec::new();
        // One uniform buffer per (stage, slot), filled by member name.
        for (stage, b) in &built.layout.entries {
            match &b.res {
                Resource::Uniform { ty } => {
                    let iface = if *stage == Stage::Vertex { &vs } else { &fs };
                    let layout =
                        iface.structs.get(ty).ok_or_else(|| format!("no layout for {ty}"))?;
                    let mut data = vec![0u8; layout.size.max(16) as usize];
                    for (k, v) in &p {
                        if msl::write_member(&mut data, layout, k, v) {
                            written.push(format!("{ty}.{k}"));
                        }
                    }
                    let buf = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(&b.name),
                        size: data.len() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    queue.write_buffer(&buf, 0, &data);
                    buffers.push(buf);
                }
                Resource::Storage { .. } => {
                    buffers.push(device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(&b.name),
                        size: 1 << 16,
                        usage: wgpu::BufferUsages::STORAGE,
                        mapped_at_creation: false,
                    }))
                }
                Resource::Sampler => {
                    samplers.push(device.create_sampler(&wgpu::SamplerDescriptor {
                        address_mode_u: wgpu::AddressMode::Repeat,
                        address_mode_v: wgpu::AddressMode::Repeat,
                        mag_filter: wgpu::FilterMode::Nearest,
                        min_filter: wgpu::FilterMode::Nearest,
                        ..Default::default()
                    }))
                }
                Resource::Texture { kind, .. } => {
                    textures.push(make_texture(device, queue, &b.name, *kind))
                }
            }
        }
        written.sort();
        written.dedup();
        let _ = writeln!(notes, "constant-buffer members written by name: {}", written.len());
        let (mut bi, mut ti, mut si) = (0, 0, 0);
        let bg_entries: Vec<wgpu::BindGroupEntry> = built
            .layout
            .entries
            .iter()
            .enumerate()
            .map(|(i, (_, b))| {
                let resource = match b.res {
                    Resource::Uniform { .. } | Resource::Storage { .. } => {
                        bi += 1;
                        buffers[bi - 1].as_entire_binding()
                    }
                    Resource::Sampler => {
                        si += 1;
                        wgpu::BindingResource::Sampler(&samplers[si - 1])
                    }
                    Resource::Texture { .. } => {
                        ti += 1;
                        wgpu::BindingResource::TextureView(&textures[ti - 1])
                    }
                };
                wgpu::BindGroupEntry { binding: i as u32, resource }
            })
            .collect();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("owlcat"),
            layout: &built.layout.layout,
            entries: &bg_entries,
        });

        let (verts, idx) = sphere(48, 64);
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sphere"),
            size: verts.len() as u64 * STRIDE,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &vbuf,
            0,
            &verts.iter().flatten().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>(),
        );
        let ibuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sphere-idx"),
            size: (idx.len() * 4) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &ibuf,
            0,
            &idx.iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<u8>>(),
        );

        let formats = gbuffer_formats(fs.color_outputs);
        let size = wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 };
        let targets: Vec<wgpu::Texture> = formats
            .iter()
            .map(|&format| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("gbuffer"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
            })
            .collect();
        let views = targets.iter().map(|t| t.create_view(&Default::default())).collect();
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&Default::default());
        Ok(Self {
            pipeline: built.pipeline,
            bind_group,
            vbuf,
            ibuf,
            index_count: idx.len() as u32,
            targets,
            views,
            depth_view,
            formats,
            model,
            view,
            proj,
            notes,
        })
    }

    /// Record the G-buffer pass. `timestamps` = a 2-entry query set for pass-boundary timing.
    pub fn encode(&self, enc: &mut wgpu::CommandEncoder, timestamps: Option<&wgpu::QuerySet>) {
        let atts: Vec<Option<wgpu::RenderPassColorAttachment>> = self
            .views
            .iter()
            .map(|v| {
                Some(wgpu::RenderPassColorAttachment {
                    view: v,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })
            })
            .collect();
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("owlcat-gbuffer"),
            color_attachments: &atts,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: timestamps.map(|qs| wgpu::RenderPassTimestampWrites {
                query_set: qs,
                beginning_of_pass_write_index: Some(0),
                end_of_pass_write_index: Some(1),
            }),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vbuf.slice(..));
        pass.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }

    /// Read every G-buffer target back (blocks until the GPU is idle).
    pub fn read_targets(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<Vec<(Vec<u8>, wgpu::TextureFormat)>, String> {
        self.targets
            .iter()
            .zip(&self.formats)
            .map(|(t, f)| Ok((read_texture(device, queue, t, *f)?, *f)))
            .collect()
    }

    /// Numerical checks of the targets (normals, BC1 albedo, metallic, smoothness).
    pub fn check(&self, images: &[(Vec<u8>, wgpu::TextureFormat)]) -> Result<String, String> {
        verify(images, self.model, self.view, self.proj)
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let dir = opt(args, "--dir").map(PathBuf::from).unwrap_or_else(default_msl_dir);
    let vs_path = dir.join(opt(args, "--vs").unwrap_or("blob153.metal"));
    let fs_path = dir.join(opt(args, "--fs").unwrap_or("blob869.metal"));
    let out_dir = PathBuf::from(
        opt(args, "--out")
            .unwrap_or(concat!(env!("CARGO_MANIFEST_DIR"), "/../../cache/poc0-render/out")),
    );
    let report = opt(args, "--report").map(PathBuf::from);
    let vs_src = std::fs::read_to_string(&vs_path)
        .map_err(|e| format!("{}: {e} (run the extractor; see README)", vs_path.display()))?;
    let fs_src =
        std::fs::read_to_string(&fs_path).map_err(|e| format!("{}: {e}", fs_path.display()))?;
    let (adapter, device, queue) = gpu::headless(
        wgpu::Features::PASSTHROUGH_SHADERS
            | wgpu::Features::TIMESTAMP_QUERY
            | wgpu::Features::TEXTURE_COMPRESSION_BC
            | wgpu::Features::RG11B10UFLOAT_RENDERABLE,
    )?;
    let draw = OwlcatDraw::new(&device, &queue, &vs_src, &fs_src)?;
    let mut rep = String::new();
    let _ = writeln!(rep, "# owlcat passthrough ({}, headless wgpu 30)", adapter.get_info().name);
    let _ = writeln!(
        rep,
        "pair: {} + {}",
        vs_path.file_name().unwrap_or_default().to_string_lossy(),
        fs_path.file_name().unwrap_or_default().to_string_lossy()
    );
    rep.push_str(&draw.notes);
    let qs = device.create_query_set(&wgpu::QuerySetDescriptor {
        label: Some("pass"),
        ty: wgpu::QueryType::Timestamp,
        count: 2,
    });
    let qbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("pass-resolve"),
        size: 16,
        usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let mut gpu_ms = Vec::new();
    let mut images = Vec::new();
    // Frame 0 is read back and checked; frames 1.. time the pass on the GPU.
    for frame in 0..21 {
        let mut enc = device.create_command_encoder(&Default::default());
        draw.encode(&mut enc, Some(&qs));
        queue.submit([enc.finish()]);
        gpu::wait_idle(&device);
        // Resolve after the pass's command buffer completed (Metal returns 0 otherwise).
        let mut enc = device.create_command_encoder(&Default::default());
        enc.resolve_query_set(&qs, 0..2, &qbuf, 0);
        let ts = read_buffer(&device, &queue, enc, &qbuf, 16)?;
        let a = u64::from_le_bytes(ts[0..8].try_into().unwrap_or_default());
        let b = u64::from_le_bytes(ts[8..16].try_into().unwrap_or_default());
        if frame > 0 && b > a {
            gpu_ms.push((b - a) as f64 * f64::from(queue.get_timestamp_period()) / 1e6);
        }
        if frame == 0 {
            images = draw.read_targets(&device, &queue)?;
        }
    }
    let _ = match stats::summarize(&gpu_ms) {
        Some(s) => writeln!(
            rep,
            "GPU time of the G-buffer pass (render-pass timestamp_writes, 512x512, 1 sphere, 6 MRT): {}",
            s.line()
        ),
        None => writeln!(rep, "GPU pass timestamps: none returned"),
    };
    rep.push_str(&draw.check(&images)?);
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    for (i, (px, fmt)) in images.iter().enumerate() {
        write_png(&out_dir.join(format!("gbuffer_rt{i}.png")), px, *fmt)?;
    }
    let _ = writeln!(rep, "PNGs: {} (gitignored cache)", out_dir.display());
    print!("{rep}");
    if let Some(r) = report {
        std::fs::write(&r, &rep).map_err(|e| e.to_string())?;
    }
    if rep.contains("WARNING: a check failed") {
        return Err("Owlcat G-buffer checks failed".into());
    }
    Ok(())
}

fn make_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    name: &str,
    kind: TexKind,
) -> wgpu::TextureView {
    let (dim, view_dim, layers) = match kind {
        TexKind::D3 => (wgpu::TextureDimension::D3, wgpu::TextureViewDimension::D3, 1),
        TexKind::Cube => (wgpu::TextureDimension::D2, wgpu::TextureViewDimension::Cube, 6),
        TexKind::CubeArray => {
            (wgpu::TextureDimension::D2, wgpu::TextureViewDimension::CubeArray, 6)
        }
        TexKind::D2Array | TexKind::Depth2Array => {
            (wgpu::TextureDimension::D2, wgpu::TextureViewDimension::D2Array, 1)
        }
        TexKind::D2 | TexKind::Depth2 => {
            (wgpu::TextureDimension::D2, wgpu::TextureViewDimension::D2, 1)
        }
    };
    let depth = matches!(kind, TexKind::Depth2 | TexKind::Depth2Array);
    if name == "_BaseMap" && kind == TexKind::D2 {
        // The shipped textures are BC1/BC3: sample BC1 directly (TEXTURE_COMPRESSION_BC).
        let size = 64;
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("_BaseMap (BC1)"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bc1RgbaUnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            tex.as_image_copy(),
            &bc1_checker(size),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size / 4 * 8),
                rows_per_image: None,
            },
            wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        );
        return tex.create_view(&Default::default());
    }
    let format =
        if depth { wgpu::TextureFormat::Depth32Float } else { wgpu::TextureFormat::Rgba8Unorm };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(name),
        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: layers },
        mip_level_count: 1,
        sample_count: 1,
        dimension: dim,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | if depth {
                wgpu::TextureUsages::RENDER_ATTACHMENT
            } else {
                wgpu::TextureUsages::empty()
            },
        view_formats: &[],
    });
    if !depth {
        // DXT5nm-style flat normal (x = r*a = 0.5, y = g = 0.5); masks: roughness 0.5 (x),
        // metallic 0.25 (z), thickness 0.5 (w); everything else mid-grey.
        let px: [u8; 4] = match name {
            "_BumpMap" => [255, 128, 128, 128],
            "_MasksMap" => [128, 0, 64, 128],
            _ => [128, 128, 128, 255],
        };
        for layer in 0..layers {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                    aspect: wgpu::TextureAspect::All,
                },
                &px,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4),
                    rows_per_image: Some(1),
                },
                wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
            );
        }
    }
    tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(view_dim),
        ..Default::default()
    })
}

fn read_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    mut enc: wgpu::CommandEncoder,
    src: &wgpu::Buffer,
    size: u64,
) -> Result<Vec<u8>, String> {
    let rb = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rb"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    enc.copy_buffer_to_buffer(src, 0, &rb, 0, size);
    queue.submit([enc.finish()]);
    rb.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    gpu::wait_idle(device);
    let v = rb.slice(..).get_mapped_range().map_err(|e| format!("map: {e:?}"))?.to_vec();
    rb.unmap();
    Ok(v)
}

fn bytes_per_px(fmt: wgpu::TextureFormat) -> u32 {
    match fmt {
        wgpu::TextureFormat::Rgba16Float => 8,
        _ => 4,
    }
}

fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    fmt: wgpu::TextureFormat,
) -> Result<Vec<u8>, String> {
    let bpp = bytes_per_px(fmt);
    let row = (W * bpp).div_ceil(256) * 256;
    let rb = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("tex-rb"),
        size: u64::from(row * H),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &rb,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    rb.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    gpu::wait_idle(device);
    let data = rb.slice(..).get_mapped_range().map_err(|e| format!("map: {e:?}"))?.to_vec();
    rb.unmap();
    let mut out = Vec::with_capacity((W * H * bpp) as usize);
    for y in 0..H {
        let s = (y * row) as usize;
        out.extend_from_slice(&data[s..s + (W * bpp) as usize]);
    }
    Ok(out)
}

fn px(img: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2], img[i + 3]]
}

/// Decode Owlcat's G-buffer normal: 12-bit octahedral x/y packed into three bytes.
pub fn decode_normal(rgb: [u8; 3]) -> Vec3 {
    let (r, g, b) = (u32::from(rgb[0]), u32::from(rgb[1]), u32::from(rgb[2]));
    let xi = r | ((b & 0xF) << 8);
    let yi = g | ((b >> 4) << 8);
    let ox = xi as f32 / 4095.0 * 2.0 - 1.0;
    let oy = yi as f32 / 4095.0 * 2.0 - 1.0;
    let mut n = Vec3::new(ox, oy, 1.0 - ox.abs() - oy.abs());
    let t = (-n.z).max(0.0);
    n.x += if n.x >= 0.0 { -t } else { t };
    n.y += if n.y >= 0.0 { -t } else { t };
    n.normalize()
}

fn verify(
    images: &[(Vec<u8>, wgpu::TextureFormat)],
    model: Mat4,
    view: Mat4,
    proj: Mat4,
) -> Result<String, String> {
    let mut s = String::new();
    if images.len() < 6 {
        return Ok(format!(
            "checks: skipped ({} targets, expected 6)\nWARNING: a check failed\n",
            images.len()
        ));
    }
    let (rt0, rt1, rt2, rt3, rt4, rt5) =
        (&images[0].0, &images[1].0, &images[2].0, &images[3].0, &images[4].0, &images[5].0);
    let inv = (proj * view).inverse();
    let cam = view.inverse().w_axis.truncate();
    let inv_model = model.inverse();
    // Analytic expectations at a grid of pixels inside the sphere's silhouette: the surface point
    // under the pixel gives the world normal and, through the inverse model matrix, the sphere's
    // UV (u = longitude, v = colatitude, as the mesh was built) scaled by `_BaseMap_ST` = (2, 1).
    let (mut n_px, mut worst_dot, mut albedo_ok, mut albedo_n, mut albedo_skipped) =
        (0, 1.0f32, 0, 0, 0);
    let mut exact_bytes_ok = true;
    let mut notes = Vec::new();
    for gy in 0..9 {
        for gx in 0..9 {
            let (x, y) = (176 + gx * 20, 176 + gy * 20);
            let ndc = Vec4::new(
                (x as f32 + 0.5) / W as f32 * 2.0 - 1.0,
                1.0 - (y as f32 + 0.5) / H as f32 * 2.0,
                0.5,
                1.0,
            );
            let wp = inv * ndc;
            let dir = (wp.truncate() / wp.w - cam).normalize();
            let b = cam.dot(dir);
            let disc = b * b - (cam.length_squared() - 1.0);
            if disc < 0.0 {
                continue;
            }
            let hit = cam + dir * (-b - disc.sqrt());
            // Stay away from the silhouette, where the tessellated mesh and the sphere differ.
            if hit.normalize().dot(-dir) < 0.3 {
                continue;
            }
            n_px += 1;
            let p1 = px(rt1, x, y);
            worst_dot = worst_dot.min(decode_normal([p1[0], p1[1], p1[2]]).dot(hit.normalize()));
            // Expected checker cell from the analytic UV (Repeat, Nearest; 64 texels, 8-texel cells).
            let o = (inv_model * hit.extend(1.0)).truncate().normalize();
            let u = (o.z.atan2(o.x).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU
                * 2.0)
                .fract();
            let v = o.y.clamp(-1.0, 1.0).acos() / std::f32::consts::PI;
            let (tu, tv) = (u * 64.0, v * 64.0);
            // Skip samples within half a texel of a cell edge (linear UV interpolation across
            // triangles differs slightly from the analytic UV there).
            let near_edge = |t: f32| (t / 8.0 - (t / 8.0).round()).abs() * 8.0 < 0.6;
            let a = px(rt0, x, y);
            if near_edge(tu) || near_edge(tv) {
                albedo_skipped += 1;
            } else {
                albedo_n += 1;
                let green = ((tu as u32 / 8) + (tv as u32 / 8)) % 2 == 1;
                let want: [u8; 3] = if green { [0, 255, 0] } else { [255, 0, 0] };
                if a[..3] == want {
                    albedo_ok += 1;
                } else if notes.len() < 3 {
                    notes.push(format!("albedo at ({x},{y}) {a:?}, expected {want:?}"));
                }
            }
            // Per-pixel constants (stored bytes): metallic = masks.z 64/255 * _Metallic 1 -> 64;
            // smoothness = 1 - masks.x 128/255 * _Roughness 1 -> 127; shadowmask = 1;
            // translucency = masks.w 128/255 * (1, .5, .25) -> (128, 64, 32), flag 10/255.
            let p4 = px(rt4, x, y);
            let p5 = px(rt5, x, y);
            let checks = [
                (a[3], 64u8, "metallic"),
                (p1[3], 127, "smoothness"),
                (p4[0], 255, "shadowmask.r"),
                (p4[3], 255, "shadowmask.a"),
                (p5[0], 128, "translucency.r"),
                (p5[1], 64, "translucency.g"),
                (p5[2], 32, "translucency.b"),
                (p5[3], 10, "material flag"),
            ];
            for (got, want, what) in checks {
                if got.abs_diff(want) > 1 {
                    exact_bytes_ok = false;
                    if notes.len() < 6 {
                        notes.push(format!("{what} at ({x},{y}) = {got}, expected {want}"));
                    }
                }
            }
            // Emission (RGBA16F) must be zero; baked GI (R11G11B10F) = flat SH ambient 0.2.
            let i2 = ((y * W + x) * 8) as usize;
            if rt2[i2..i2 + 6].iter().any(|&b| b != 0) {
                exact_bytes_ok = false;
                notes.push(format!("emission at ({x},{y}) not zero"));
            }
            let i3 = ((y * W + x) * 4) as usize;
            let v3 = u32::from_le_bytes([rt3[i3], rt3[i3 + 1], rt3[i3 + 2], rt3[i3 + 3]]);
            let gi = [uf(v3 & 0x7FF, 6), uf((v3 >> 11) & 0x7FF, 6), uf((v3 >> 22) & 0x3FF, 5)];
            if gi.iter().any(|g| (g - 0.2).abs() > 0.002) {
                exact_bytes_ok = false;
                if notes.len() < 6 {
                    notes.push(format!("baked GI at ({x},{y}) = {gi:?}, expected 0.2"));
                }
            }
        }
    }
    let ok_n = n_px >= 20 && worst_dot > 0.999;
    let ok_a = albedo_n >= 10 && albedo_ok == albedo_n;
    let _ = writeln!(s, "checks at {n_px} interior pixels:");
    let _ = writeln!(s, "  normals: worst dot(decoded RT1, analytic) = {worst_dot:.5}");
    let _ = writeln!(
        s,
        "  BC1 albedo (RT0): {albedo_ok}/{albedo_n} exact texel colour at the analytic UV ({albedo_skipped} near cell edges skipped)"
    );
    let _ = writeln!(
        s,
        "  metallic, smoothness, emission, baked GI, shadowmask, translucency: {}",
        if exact_bytes_ok {
            "expected values (bytes within 1, floats within 0.002)"
        } else {
            "MISMATCH"
        }
    );
    for n in &notes {
        let _ = writeln!(s, "  {n}");
    }
    let _ = writeln!(
        s,
        "verdict: normals {} | BC1 albedo {} | other targets {}",
        pf(ok_n),
        pf(ok_a),
        pf(exact_bytes_ok)
    );
    if !(ok_n && ok_a && exact_bytes_ok) {
        let _ = writeln!(s, "WARNING: a check failed");
    }
    Ok(s)
}

fn pf(b: bool) -> &'static str {
    if b { "PASS" } else { "FAIL" }
}

fn write_png(path: &Path, data: &[u8], fmt: wgpu::TextureFormat) -> Result<(), String> {
    let rgba: Vec<u8> = if bytes_per_px(fmt) == 8 {
        data.as_chunks::<8>()
            .0
            .iter()
            .flat_map(|c| {
                (0..4).map(move |k| {
                    let h = u16::from_le_bytes([c[2 * k], c[2 * k + 1]]);
                    (half_to_f32(h).clamp(0.0, 1.0) * 255.0) as u8
                })
            })
            .collect()
    } else if fmt == wgpu::TextureFormat::Rg11b10Ufloat {
        data.as_chunks::<4>()
            .0
            .iter()
            .flat_map(|c| {
                let v = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                let r = uf(v & 0x7FF, 6);
                let g = uf((v >> 11) & 0x7FF, 6);
                let b = uf((v >> 22) & 0x3FF, 5);
                [r, g, b].into_iter().map(|x| (x.clamp(0.0, 1.0) * 255.0) as u8).chain([255])
            })
            .collect()
    } else {
        data.to_vec()
    };
    image::save_buffer(path, &rgba, W, H, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())
}

fn half_to_f32(h: u16) -> f32 {
    let s = if h >> 15 == 1 { -1.0 } else { 1.0 };
    let e = i32::from((h >> 10) & 31);
    let m = f32::from(h & 1023);
    match e {
        0 => s * m * 2f32.powi(-24),
        31 => f32::NAN,
        _ => s * (1.0 + m / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Unsigned small float with a 5-bit exponent and `mbits` mantissa bits.
fn uf(v: u32, mbits: u32) -> f32 {
    let e = (v >> mbits) as i32;
    let m = (v & ((1 << mbits) - 1)) as f32;
    let scale = (1u32 << mbits) as f32;
    match e {
        0 => m / scale * 2f32.powi(-14),
        31 => f32::NAN,
        _ => (1.0 + m / scale) * 2f32.powi(e - 15),
    }
}

/// `owlcat-bench`: build many real Owlcat/Lit variant pipelines (VS + FS pairs) through wgpu
/// passthrough and time each stage of creation, optionally on several threads.
pub fn bench(args: &[String]) -> Result<(), String> {
    let dir = opt(args, "--dir").map(PathBuf::from).unwrap_or_else(default_msl_dir);
    let limit: usize = opt(args, "--limit").and_then(|v| v.parse().ok()).unwrap_or(200);
    let threads: usize = opt(args, "--threads").and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
    let pass = opt(args, "--pass").unwrap_or("GBUFFER");
    let out = opt(args, "--out").map(PathBuf::from);
    // `--salt X` appends a comment to every source so macOS's Metal shader cache (keyed by the
    // source) misses: a repeatable cold-compile measurement without deleting the cache.
    let salt = opt(args, "--salt").map(|s| format!("\n// salt {s}\n")).unwrap_or_default();
    let index = std::fs::read_to_string(dir.join("index.tsv"))
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut vs_rows = Vec::new();
    let mut fs_rows = Vec::new();
    for l in index.lines().skip(1) {
        let c: Vec<&str> = l.split('\t').collect();
        // Match the pass name or its LightMode tag (the shadow caster pass is unnamed).
        if c.len() < 7 || (c[1] != pass && c[2] != pass) {
            continue;
        }
        let kw: Vec<String> = c[6].split_whitespace().map(str::to_string).collect();
        match c[3] {
            "progVertex" => vs_rows.push((c[4].to_string(), kw)),
            "progFragment" => fs_rows.push((c[4].to_string(), kw)),
            _ => {}
        }
    }
    let read = |b: &str| {
        std::fs::read_to_string(dir.join(format!("blob{b}.metal")))
            .map(|s| s + &salt)
            .map_err(|e| e.to_string())
    };
    let mut vs_parsed = Vec::new();
    for (b, kw) in &vs_rows {
        let src = read(b)?;
        let iface = msl::parse(&src)?;
        vs_parsed.push((b.clone(), kw.clone(), src, iface));
    }
    // Pair each fragment variant with the vertex variant Unity would select: the vertex stage's
    // keywords are the fragment's keywords restricted to those the vertex stage knows about
    // (Unity strips per stage). Exact matches are counted; when there is none we fall back to the
    // closest linkable variant and count it. Metal must also accept the link (varyings).
    let vs_universe: std::collections::HashSet<&String> =
        vs_parsed.iter().flat_map(|(_, k, _, _)| k).collect();
    struct Job {
        vb: String,
        fb: String,
        kw: String,
        vs_src: String,
        fs_src: String,
        vs: Interface,
        fs: Interface,
        exact: bool,
        fs_first: bool,
    }
    let mut jobs = Vec::new();
    let mut failures: HashMap<String, usize> = HashMap::new();
    let mut unique = std::collections::HashSet::new();
    let mut seen_fs = std::collections::HashSet::new();
    let mut vs_used = std::collections::HashSet::new();
    for (fblob, fkw) in fs_rows.iter().take(limit) {
        let fs_src = read(fblob)?;
        let fs = match msl::parse(&fs_src) {
            Ok(f) => f,
            Err(e) => {
                *failures.entry(format!("parse: {e}")).or_default() += 1;
                continue;
            }
        };
        let mut want: Vec<&String> = fkw.iter().filter(|k| vs_universe.contains(k)).collect();
        want.sort();
        let usable = |vs: &Interface| {
            msl::links(vs, &fs)
                && vs.vertex_inputs.iter().all(|v| vertex_attr(&v.semantic).is_some())
        };
        let exact = vs_parsed.iter().find(|(_, vk, _, vs)| {
            let mut k: Vec<&String> = vk.iter().collect();
            k.sort();
            k == want && usable(vs)
        });
        let pick = exact.or_else(|| {
            vs_parsed.iter().filter(|(_, _, _, vs)| usable(vs)).max_by_key(|(_, vk, _, _)| {
                let shared = vk.iter().filter(|k| fkw.contains(k)).count() as i64;
                shared * 100 - (vk.len() as i64 - shared)
            })
        });
        let Some((vblob, _, vs_src, vs)) = pick else {
            *failures.entry("no linkable vertex variant".into()).or_default() += 1;
            continue;
        };
        unique.insert(fs_src.clone());
        unique.insert(vs_src.clone());
        vs_used.insert(vblob.clone());
        let fs_first = seen_fs.insert(fs_src.clone());
        jobs.push(Job {
            vb: vblob.clone(),
            fb: fblob.clone(),
            kw: fkw.join(" "),
            vs_src: vs_src.clone(),
            fs_src,
            vs: vs.clone(),
            fs,
            exact: exact.is_some(),
            fs_first,
        });
    }
    let n_exact = jobs.iter().filter(|j| j.exact).count();
    let (_adapter, device, _queue) = gpu::headless(
        wgpu::Features::PASSTHROUGH_SHADERS
            | wgpu::Features::TEXTURE_COMPRESSION_BC
            | wgpu::Features::RG11B10UFLOAT_RENDERABLE,
    )?;
    // wgpu reports Metal pipeline-state errors as Internal (not Validation) errors; anything
    // uncaptured would hit the default handler, which panics.
    device.on_uncaptured_error(std::sync::Arc::new(|e| eprintln!("uncaptured wgpu error: {e}")));
    type Row = (String, bool, Result<(f64, f64, f64, f64), String>);
    let t_all = Instant::now();
    let chunk = jobs.len().div_ceil(threads).max(1);
    let results: Vec<Row> = std::thread::scope(|sc| {
        let handles: Vec<_> = jobs
            .chunks(chunk)
            .map(|part| {
                let device = &device;
                sc.spawn(move || {
                    part.iter()
                        .map(|j| {
                            let v = device.push_error_scope(wgpu::ErrorFilter::Validation);
                            let i = device.push_error_scope(wgpu::ErrorFilter::Internal);
                            let t = Instant::now();
                            let r = build_pipeline(device, &j.vs_src, &j.fs_src, &j.vs, &j.fs);
                            let total = t.elapsed().as_secs_f64() * 1e3;
                            let ie = bevy::tasks::block_on(i.pop());
                            let ve = bevy::tasks::block_on(v.pop());
                            let res = match (r, ie.or(ve)) {
                                (Ok(b), None) => Ok((b.vs_ms, b.fs_ms, b.pipeline_ms, total)),
                                (Err(e), _) => Err(e.chars().take(100).collect()),
                                (_, Some(e)) => Err(e
                                    .to_string()
                                    .lines()
                                    .last()
                                    .unwrap_or("")
                                    .chars()
                                    .take(140)
                                    .collect()),
                            };
                            (
                                format!(
                                    "{},{},\"{}\",{},{}",
                                    j.vb,
                                    j.fb,
                                    j.kw,
                                    u8::from(j.exact),
                                    u8::from(j.fs_first)
                                ),
                                j.fs_first,
                                res,
                            )
                        })
                        .collect::<Vec<Row>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });
    let wall = t_all.elapsed().as_secs_f64();
    let (mut vs_ms, mut fs_ms, mut pl_ms, mut total_ms, mut rows) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut first_total, mut repeat_total, mut first_pl) = (Vec::new(), Vec::new(), Vec::new());
    for (key, first, r) in results {
        match r {
            Ok((a, b, c, d)) => {
                vs_ms.push(a);
                fs_ms.push(b);
                pl_ms.push(c);
                total_ms.push(d);
                if first {
                    first_total.push(d);
                    first_pl.push(c);
                } else {
                    repeat_total.push(d);
                }
                rows.push(format!("{key},{a:.3},{b:.3},{c:.3},{d:.3}"));
            }
            Err(e) => *failures.entry(e).or_default() += 1,
        }
    }
    let n_failed: usize = failures.values().sum();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "# owlcat-bench pass={pass}: {} fragment variants tried (of {}), {} built, {n_failed} failed; {} unique MSL sources; {threads} thread(s); salt {:?}; wall {:.2} s",
        fs_rows.len().min(limit),
        fs_rows.len(),
        total_ms.len(),
        unique.len(),
        opt(args, "--salt").unwrap_or("none (OS shader cache as it stands)"),
        wall
    );
    let _ = writeln!(
        s,
        "vertex pairing: {n_exact} exact (Unity's per-stage keyword rule), {} fallback; {} of {} vertex variants used",
        jobs.len() - n_exact,
        vs_used.len(),
        vs_rows.len()
    );
    for (name, v) in [
        ("vs module (MSL compile)", &vs_ms),
        ("fs module (MSL compile)", &fs_ms),
        ("render pipeline (PSO)", &pl_ms),
        ("total per pipeline", &total_ms),
        ("total, first use of fs src", &first_total),
        ("PSO, first use of fs src", &first_pl),
        ("total, repeated fs src", &repeat_total),
    ] {
        if let Some(x) = stats::summarize(v) {
            let _ = writeln!(s, "{name:<27} {} sum={:.1}", x.line(), v.iter().sum::<f64>());
        }
    }
    if threads > 1 {
        let _ =
            writeln!(s, "(with several threads, 'first use' is by list order, not compile order)");
    }
    let mut f: Vec<_> = failures.into_iter().collect();
    f.sort_by_key(|e| std::cmp::Reverse(e.1));
    for (e, n) in f {
        let _ = writeln!(s, "failed x{n}: {e}");
    }
    print!("{s}");
    if let Some(out) = out {
        std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
        std::fs::write(out.join("summary.txt"), &s).map_err(|e| e.to_string())?;
        stats::write_csv(
            &out.join("pipelines.csv"),
            "vs_blob,fs_blob,fs_keywords,exact_pair,fs_first_use,vs_ms,fs_ms,pipeline_ms,total_ms",
            &rows,
        )
        .map_err(|e| e.to_string())?;
    }
    if n_failed > 0 {
        return Err(format!("{n_failed} pipelines failed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encode a unit normal the way Owlcat's G-buffer fragment does (octahedral, 12 bits/axis).
    fn encode(n: Vec3) -> [u8; 3] {
        let l1 = n.x.abs() + n.y.abs() + n.z.abs().max(1.0 / 1024.0);
        let (mut x, mut y) = (n.x / l1, n.y / l1);
        if n.z < 0.0 {
            let (ox, oy) = (x, y);
            x = (1.0 - oy.abs()) * ox.signum();
            y = (1.0 - ox.abs()) * oy.signum();
        }
        let xi = ((x * 0.5 + 0.5).clamp(0.0, 1.0) * 4095.5) as u32;
        let yi = ((y * 0.5 + 0.5).clamp(0.0, 1.0) * 4095.5) as u32;
        [(xi & 0xFF) as u8, (yi & 0xFF) as u8, ((yi >> 8) * 16 + (xi >> 8)) as u8]
    }

    #[test]
    fn normal_roundtrip() {
        for n in [
            Vec3::Z,
            Vec3::X,
            -Vec3::Y,
            Vec3::new(0.3, -0.5, 0.8).normalize(),
            Vec3::new(-0.2, 0.4, -0.9).normalize(),
        ] {
            let d = decode_normal(encode(n));
            assert!(d.dot(n) > 0.9995, "{n:?} -> {d:?}");
        }
    }

    #[test]
    fn bc1_block_layout() {
        let b = bc1_checker(16);
        assert_eq!(b.len(), 16 / 4 * 16 / 4 * 8);
        assert_eq!(&b[0..2], &0xF800u16.to_le_bytes());
    }
}
