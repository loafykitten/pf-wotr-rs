//! Owlcat's render path, hosted on Bevy's wgpu device: the game's own shaders
//! (offline-compiled metallibs) drawn through a port of the passes around them.
//!
//! Implemented passes, in `ClusteredRenderer` order: clear; G-buffer (6 targets,
//! `Owlcat/Lit` materials with lightmaps; recorded once into a render bundle); light
//! culling (`LightCulling.BuildLightTiles` compute); deferred reflections (default
//! reflection, reflection probes, blend with emission); deferred lighting
//! (`Hidden/Owlcat/DeferredLighting`, clustered realtime lights); fog (`Hidden/Owlcat/Fog`);
//! post as `PostProcessPass.Render` runs it for the cave profile (Stop NaN, BloomEnhanced,
//! the grading LUT, Uber with HDR grading and vignette); and a final blit to the display
//! target (Y flip; the after-post colour already holds sRGB-encoded values). Before the
//! G-buffer, `ShadowCasterPass`: the spot and point (tetrahedral) shadows of the visible
//! lights into one depth atlas through the materials' own `ShadowCaster` programs, read by
//! the `SHADOWS_SOFT` deferred lighting (`docs/poc6/render-round3.md`). Not yet: directional
//! shadows, light probes, decals, HBAO, SSR, skybox, transparents, particles, VFX Graph,
//! distortion, fog of war, film grain, dithering, SMAA.

pub mod consts;
pub mod gpu;
pub mod lights;
pub mod pipeline;
pub mod post;
pub mod shadows;

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use bevy::math::{Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;
use wr_assets::{Binding, CBufferLayout, Material, Pack, PassState, Program, Resource, Stage};

use consts::{Consts, fill, linear};
use gpu::Gpu;
use pipeline::{RenderState, VertexLayout, layout_entries, resolve_state, vertex_layout};

/// G-buffer formats (`GBuffer.Initialize`): albedo (sRGB), normals, camera colour
/// (HDR), baked GI (R11G11B10F), shadowmask, translucency.
pub const GBUFFER: [wgpu::TextureFormat; 6] = [
    wgpu::TextureFormat::Rgba8UnormSrgb,
    wgpu::TextureFormat::Rgba8Unorm,
    wgpu::TextureFormat::Rgba16Float,
    wgpu::TextureFormat::Rg11b10Ufloat,
    wgpu::TextureFormat::Rgba8Unorm,
    wgpu::TextureFormat::Rgba8Unorm,
];
pub const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// `_CameraDeferredReflectionsRT` with SSR on: `ColorPyramidFormat` (R11G11B10F).
pub const REFLECTIONS: wgpu::TextureFormat = wgpu::TextureFormat::Rg11b10Ufloat;
/// The owlcat `TileSize` for light culling.
pub const TILE: u32 = 16;

/// A camera exactly as the original rendered it (from the route export).
#[derive(Debug, Clone, Copy)]
pub struct CameraPose {
    pub world_to_camera: Mat4,
    /// `GL.GetGPUProjectionMatrix(projection, renderIntoTexture: true)`.
    pub gpu_projection: Mat4,
    pub projection: Mat4,
    pub position: Vec3,
    pub near: f32,
    pub far: f32,
    pub width: u32,
    pub height: u32,
    /// The camera rig's pose (position, rotation): the rig light follows it.
    pub rig: Option<(Vec3, bevy::math::Quat)>,
}

/// What renderer construction measured.
#[derive(Debug, Clone, Default)]
pub struct LoadStats {
    pub pack_open_ms: f64,
    pub upload_ms: f64,
    pub pipelines_ms: f64,
    pub bind_groups_ms: f64,
    pub pipelines: usize,
    pub shader_modules: usize,
    pub draws: usize,
    pub uploaded_bytes: u64,
    pub skipped: Vec<String>,
}

struct Targets {
    gbuffer: Vec<wgpu::TextureView>,
    depth: wgpu::TextureView,
    reflections: wgpu::TextureView,
}

impl Targets {
    fn new(device: &wgpu::Device, w: u32, h: u32) -> Self {
        let make = |format, label: &str| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let names = [
            "_CameraAlbedoRT",
            "_CameraNormalsRT",
            "_CameraColorRT",
            "_CameraBakedGIRT",
            "_CameraShadowmaskRT",
            "_CameraTranslucencyRT",
        ];
        Self {
            gbuffer: GBUFFER.iter().zip(names).map(|(f, n)| make(*f, n)).collect(),
            depth: make(DEPTH, "_CameraDepthRT"),
            reflections: make(REFLECTIONS, "_CameraDeferredReflectionsRT"),
        }
    }

    fn by_name(&self, name: &str) -> Option<&wgpu::TextureView> {
        Some(match name {
            "_CameraAlbedoRT" => &self.gbuffer[0],
            "_CameraNormalsRT" => &self.gbuffer[1],
            "_CameraColorRT" => &self.gbuffer[2],
            "_CameraBakedGIRT" => &self.gbuffer[3],
            "_CameraShadowmaskRT" => &self.gbuffer[4],
            "_CameraTranslucencyRT" => &self.gbuffer[5],
            "_CameraDepthRT" | "_CameraDepthCopyRT" => &self.depth,
            "_CameraDeferredReflectionsRT" => &self.reflections,
            _ => return None,
        })
    }
}

/// A bind group layout, its pipeline layout and the bindings in entry order.
type LayoutEntry = (wgpu::BindGroupLayout, wgpu::PipelineLayout, Vec<(Stage, Binding)>);

/// Where a bound resource comes from, before buffers exist.
#[derive(Clone)]
enum Res {
    /// Offset and size in the per-draw/per-material uniform arena.
    Arena(u64, u64),
    /// A camera-scope constant buffer, by index.
    Global(usize),
    Buffer(wgpu::Buffer),
    View(wgpu::TextureView),
    Sampler(wgpu::Sampler),
    /// The `ShadowCasterBuffer` window (size in bytes) at a dynamic offset.
    Caster(u64),
    /// A binding we can't satisfy (unknown layout or storage buffer): the draw is
    /// skipped rather than bound to something arbitrary.
    Invalid(String),
}

struct GlobalCb {
    layout: CBufferLayout,
    buffer: wgpu::Buffer,
}

/// A recorded draw.
struct Draw {
    pipeline: usize,
    bind: wgpu::BindGroup,
    /// Vertex buffers in slot order: mesh streams, then constant defaults.
    vbufs: Vec<wgpu::Buffer>,
    index: Option<(wgpu::Buffer, wgpu::IndexFormat)>,
    range: std::ops::Range<u32>,
    base_vertex: i32,
}

/// One post-processing blit: its draw and target.
struct PostStep {
    label: String,
    target: wgpu::TextureView,
    /// The grading LUT (drawn only when `lut_dirty`).
    lut: bool,
    draw: Draw,
}

/// `RenderTextureFormat.Shadowmap` with 16 depth bits (`ClusteredRenderer.cs:427`). The
/// Metal format Unity picks for it is native; 16-bit unorm is the best-supported guess [I].
pub const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth16Unorm;
/// One (entry, face) record of the caster constant buffer: the dynamic-offset stride.
const CASTER_RECORD: u64 = 256;
/// Most (entry, face) records one frame can need: 64 entries of four faces.
const CASTER_RECORDS: u64 = (shadows::MAX_ENTRIES * 4) as u64;

/// The shadow atlas and the buffers that describe it.
struct ShadowRes {
    atlas: wgpu::Texture,
    view: wgpu::TextureView,
    /// `sample_compare` sampler: linear, clamped, reversed-Z `GreaterEqual` [I].
    sampler: wgpu::Sampler,
    /// `_ShadowDataBuffer`: 64 × 48 bytes.
    data: wgpu::Buffer,
    /// `_ShadowMatricesBuffer`: 256 × 112 bytes.
    matrices: wgpu::Buffer,
    /// The per-face `ShadowCasterBuffer` records.
    caster_cb: wgpu::Buffer,
    /// The layout of `ShadowCasterBuffer_Type`, from the first caster program seen.
    cb_layout: Option<CBufferLayout>,
    settings: shadows::Settings,
}

/// A caster's draw for one `GEOMETRY_CLIP` state.
struct CasterBind {
    /// Pipelines by raster slope bias (`f32` bits): point lights set a bias per light.
    pipelines: Vec<(u32, usize)>,
    bind: wgpu::BindGroup,
    vbufs: Vec<wgpu::Buffer>,
    index: (wgpu::Buffer, wgpu::IndexFormat),
    range: std::ops::Range<u32>,
    base_vertex: i32,
}

/// One renderer slot that casts shadows: world bounds for culling, and its draws.
struct Caster {
    lo: Vec3,
    hi: Vec3,
    layer: u32,
    /// `ShadowCastingMode`: 0 off, 1 on, 2 two-sided, 3 shadows only.
    cast: u8,
    /// The renderer's `renderingLayerMask` (`useRenderingLayerMaskTest`).
    rendering_layer_mask: u32,
    /// Indexed by `GEOMETRY_CLIP`.
    binds: [Option<CasterBind>; 2],
}

/// A caster draw's slot: index into `Renderer::casters`, `GEOMETRY_CLIP` state, and the
/// pipelines per raster slope bias.
type CasterPlan = (usize, bool, Vec<(u32, usize)>);

/// One `SetViewport` + `DrawShadows` of the caster pass.
struct ShadowFace {
    viewport: shadows::Rect,
    /// Dynamic offset of this face's `ShadowCasterBuffer` record.
    offset: u32,
    clip: bool,
    slope_bias: u32,
    draws: Vec<u32>,
}

/// What the last `set_camera` planned (diagnostics).
#[derive(Debug, Clone, Default)]
pub struct ShadowStats {
    pub candidates: usize,
    pub entries: usize,
    pub spots: usize,
    pub points: usize,
    pub dropped: usize,
    pub faces: usize,
    pub draws: usize,
    /// (resolution, x, y, is point) per entry, in atlas order.
    pub tiles: Vec<(u32, u32, u32, bool)>,
}

/// Which camera-scope constant buffers are global (per camera) rather than per
/// material or per draw.
fn scope(ty: &str) -> u8 {
    match ty {
        "UnityPerMaterial_Type" => 1,
        "UnityPerDraw_Type" | "FGlobals_Type" | "TerrainLayerDataCB_Type" => 2,
        _ => 0,
    }
}

pub struct Renderer {
    pub pack: Arc<Pack>,
    gpu: Gpu,
    targets: Targets,
    pipelines: Vec<wgpu::RenderPipeline>,
    pipeline_index: HashMap<String, usize>,
    layouts: HashMap<String, LayoutEntry>,
    globals: Vec<GlobalCb>,
    global_index: HashMap<String, usize>,
    gbuffer_draws: Vec<Draw>,
    /// The G-buffer draws pre-recorded once at load (`WR_NO_BUNDLES` records them per
    /// frame instead): wgpu validates and tracks a bundle's resources when it's made, so
    /// replaying it costs much less CPU per frame than recording ~900 draws.
    gbuffer_bundle: Option<wgpu::RenderBundle>,
    reflection_draws: Vec<Draw>,
    lighting_draws: Vec<Draw>,
    fog_draws: Vec<Draw>,
    blit: (wgpu::RenderPipeline, wgpu::BindGroupLayout, wgpu::Sampler),
    culling: Option<(wgpu::ComputePipeline, wgpu::BindGroup, u32)>,
    light_data: wgpu::Buffer,
    light_volume: wgpu::Buffer,
    zbins: wgpu::Buffer,
    tiles: wgpu::Buffer,
    culling_cb: Consts,
    pub consts: Consts,
    pub stats: LoadStats,
    pub width: u32,
    pub height: u32,
    pub lights_visible: usize,
    front_ccw: bool,
    missing: BTreeSet<String>,
    scene_settings: wr_assets::SceneSettings,
    defaults_vb: (wgpu::Buffer, wgpu::Buffer),
    /// Scene lights after `LightController` applied its config.
    runtime_lights: Vec<wr_assets::Light>,
    /// The camera rig's light (relative matrix), when the controller enables it.
    rig_light: Option<wr_assets::Light>,
    /// Post steps in order: label, target, whether it's the grading LUT, the draw.
    post_steps: Vec<PostStep>,
    /// How many post steps the profile needs (post runs only if all were built).
    post_expected: usize,
    /// `_TempTarget`: Stop NaN's output, the linear source of bloom and Uber.
    temp_target: wgpu::TextureView,
    lut_view: wgpu::TextureView,
    /// The grading LUT needs (re)building. The original rebuilds it every frame; its
    /// inputs here are constant (global volumes, weight 1), so it's built once and kept,
    /// which saves ~0.5 ms of GPU per frame for an identical image. Nothing re-arms it
    /// yet: anything that changes the post settings at run time must set it again.
    lut_dirty: AtomicBool,
    after_post: wgpu::TextureView,
    pub post_components: Vec<String>,
    /// The player's gamma setting (`gamma-correction-new`), if known.
    pub gamma_setting: Option<f32>,
    /// The final blit's bind group, made once at load.
    final_bind: Option<wgpu::BindGroup>,
    post_on: bool,
    shadow: ShadowRes,
    casters: Vec<Caster>,
    shadow_faces: Vec<ShadowFace>,
    /// `WR_NO_SHADOWS=1` turns the shadow pass off and uses the no-shadow lighting.
    shadows_on: bool,
    /// Raster slope biases point lights can use (`f32` bits).
    point_biases: Vec<u32>,
    pub shadow_stats: ShadowStats,
}

/// Resolution context for one draw's textures and samplers.
struct Bind<'a> {
    material: Option<&'a Material>,
    lightmap: Option<(Option<u32>, Option<u32>, Option<u32>)>,
    extra_tex: &'a [(&'a str, wgpu::TextureView, Option<wgpu::Sampler>)],
    /// Put every constant buffer in the arena (draws that need their own copy of
    /// otherwise camera-wide buffers, e.g. blits with their own view-projection).
    local_cbs: bool,
}

impl Renderer {
    /// Opens the pack and builds everything the route needs: textures, buffers, every
    /// pipeline and bind group (the load gate: nothing is created after this).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pack_path: &std::path::Path,
        width: u32,
        height: u32,
        front_ccw: bool,
        gamma_setting: Option<f32>,
    ) -> Result<Self> {
        let t = Instant::now();
        // SAFETY: packs are written by `wr-import` through `PackWriter` (new file, renamed
        // into place) and never modified in place; nothing writes this path while we run.
        let pack = Arc::new(unsafe { Pack::open(pack_path) }?);
        pack.prefetch();
        let pack_open_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let gpu = Gpu::upload(device, queue, &pack)?;
        let upload_ms = t.elapsed().as_secs_f64() * 1e3;
        let scene_settings = pack
            .meta
            .scenes
            .iter()
            .find(|s| s.scene.ends_with("_Light"))
            .or_else(|| pack.meta.scenes.iter().find(|s| s.fog))
            .cloned()
            .unwrap_or_default();
        let storage = |size: u64, label: &str, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let tiles_n = u64::from(width.div_ceil(TILE) * height.div_ceil(TILE) * 32);
        let defaults_vb = (
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("vb-zero"),
                contents: &[0u8; 16],
                usage: wgpu::BufferUsages::VERTEX,
            }),
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("vb-white"),
                contents: &[1.0f32, 1.0, 1.0, 1.0]
                    .iter()
                    .flat_map(|f| f.to_le_bytes())
                    .collect::<Vec<u8>>(),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        );
        let shadows_on = std::env::var_os("WR_NO_SHADOWS").is_none();
        let shadow_settings = shadows::Settings::from_env();
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("_ShadowmapRT"),
            size: wgpu::Extent3d {
                width: shadow_settings.atlas_size,
                height: shadow_settings.atlas_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let shadow = ShadowRes {
            view: atlas.create_view(&Default::default()),
            atlas,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("sampler_ShadowmapRT"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                address_mode_w: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                compare: Some(wgpu::CompareFunction::GreaterEqual),
                ..Default::default()
            }),
            data: storage(
                (shadows::MAX_ENTRIES * shadows::DATA_STRIDE) as u64,
                "_ShadowDataBuffer",
                wgpu::BufferUsages::STORAGE,
            ),
            matrices: storage(
                (shadows::MAX_MATRICES * shadows::MATRIX_STRIDE) as u64,
                "_ShadowMatricesBuffer",
                wgpu::BufferUsages::STORAGE,
            ),
            caster_cb: storage(
                CASTER_RECORD * CASTER_RECORDS,
                "ShadowCasterBuffer",
                wgpu::BufferUsages::UNIFORM,
            ),
            cb_layout: None,
            settings: shadow_settings,
        };
        // Raster slope biases point lights can ask for: `-DepthBias` of the pipeline, or of
        // the light when it doesn't use the pipeline settings (`ShadowmapEntry.SetLight`).
        let mut point_biases: Vec<u32> = Vec::new();
        for l in pack.meta.lights.iter().filter(|l| l.kind == 2 && l.shadows != 0) {
            let bias = if l.owlcat.as_ref().is_none_or(|o| o.use_pipeline_settings) {
                shadow.settings.depth_bias
            } else {
                l.shadow_bias
            };
            let bits = (-bias).to_bits();
            if !point_biases.contains(&bits) {
                point_biases.push(bits);
            }
        }
        if point_biases.is_empty() {
            point_biases.push((-shadow.settings.depth_bias).to_bits());
        }
        let mut r = Self {
            shadow,
            casters: Vec::new(),
            shadow_faces: Vec::new(),
            shadows_on,
            point_biases,
            shadow_stats: ShadowStats::default(),
            targets: Targets::new(device, width, height),
            pipelines: Vec::new(),
            pipeline_index: HashMap::new(),
            layouts: HashMap::new(),
            globals: Vec::new(),
            global_index: HashMap::new(),
            gbuffer_draws: Vec::new(),
            gbuffer_bundle: None,
            reflection_draws: Vec::new(),
            lighting_draws: Vec::new(),
            fog_draws: Vec::new(),
            blit: blit_pipeline(device),
            culling: None,
            light_data: storage(65536, "LightDataCB", wgpu::BufferUsages::UNIFORM),
            light_volume: storage(49152, "LightVolumeDataCB", wgpu::BufferUsages::UNIFORM),
            zbins: storage(16384, "ZBinsCB", wgpu::BufferUsages::UNIFORM),
            tiles: storage(tiles_n * 4, "_LightTilesBuffer", wgpu::BufferUsages::STORAGE),
            culling_cb: Consts::default(),
            consts: Consts::default(),
            stats: LoadStats {
                pack_open_ms,
                upload_ms,
                uploaded_bytes: gpu.uploaded,
                ..Default::default()
            },
            gpu,
            pack,
            width,
            height,
            lights_visible: 0,
            front_ccw,
            missing: BTreeSet::new(),
            runtime_lights: Vec::new(),
            rig_light: None,
            post_steps: Vec::new(),
            post_expected: 0,
            temp_target: rt(device, width, height, "_TempTarget"),
            lut_view: rt(device, post::LUT_SIZE * post::LUT_SIZE, post::LUT_SIZE, "_InternalLut"),
            lut_dirty: AtomicBool::new(true),
            after_post: rt(device, width, height, "_AfterPostProcessColorRT"),
            post_components: Vec::new(),
            gamma_setting,
            final_bind: None,
            post_on: false,
            scene_settings,
            defaults_vb,
        };
        r.apply_light_controller();
        r.stats.skipped.extend(r.gpu.skipped_textures.clone());
        r.set_static_globals();
        r.build(device, queue)?;
        Ok(r)
    }

    /// `LightController.ApplySceneRendreringSettings` and `SetupCamera`: the config's
    /// trilight ambient, linear fog colour and distances, the main light's colour,
    /// intensity and rotation, and the camera rig light.
    fn apply_light_controller(&mut self) {
        let mut lights = self.pack.meta.lights.clone();
        if let Some(lc) = self.pack.meta.lighting.clone() {
            let s = &mut self.scene_settings;
            s.ambient_mode = 1;
            s.ambient_sky = lc.sky_ambient;
            s.ambient_equator = lc.equator_ambient;
            s.ambient_ground = lc.ground_ambient;
            s.fog_mode = 1;
            s.fog_color = lc.fog_color;
            s.fog_start = lc.fog_start;
            s.fog_end = lc.fog_end;
            if let Some(main) = &lc.main_light
                && let Some(l) = lights.iter_mut().find(|l| &l.id == main)
            {
                l.color = lc.main_light_color;
                l.intensity = lc.main_light_intensity;
                l.shadow_strength = lc.main_light_shadow_strength;
                let [x, y, z] = lc.main_light_rotation.map(f32::to_radians);
                // Quaternion.Euler applies Z, then X, then Y.
                let rot = bevy::math::Quat::from_euler(bevy::math::EulerRot::YXZ, y, x, z);
                let m = Mat4::from_cols_array(&l.matrix);
                let (scale, _, t) = m.to_scale_rotation_translation();
                l.matrix = Mat4::from_scale_rotation_translation(scale, rot, t).to_cols_array();
            }
            if lc.camera_light == 1 {
                self.rig_light = lc.rig_light.clone();
            }
        }
        self.runtime_lights = lights;
    }

    fn set_static_globals(&mut self) {
        let s = self.scene_settings.clone();
        let c = &mut self.consts;
        let lin = |v: [f32; 4], k: f32| {
            let l = linear(v);
            Vec4::new(l[0] * k, l[1] * k, l[2] * k, l[3])
        };
        c.v4("unity_AmbientSky", lin(s.ambient_sky, s.ambient_intensity));
        c.v4("unity_AmbientEquator", lin(s.ambient_equator, s.ambient_intensity));
        c.v4("unity_AmbientGround", lin(s.ambient_ground, s.ambient_intensity));
        c.v4("glstate_lightmodel_ambient", lin(s.ambient_sky, s.ambient_intensity) * 0.5);
        let p = &s.ambient_probe;
        let l0 = |i: usize| p.get(i * 9).copied().unwrap_or(0.0);
        c.f("unity_IndirectSpecColor", &[l0(0), l0(1), l0(2), 1.0]);
        // `_GlossyEnvironmentColor`: ambient probe L0 × reflection intensity, linear.
        c.f(
            "_GlossyEnvironmentColor",
            &[
                l0(0) * s.reflection_intensity,
                l0(1) * s.reflection_intensity,
                l0(2) * s.reflection_intensity,
                1.0,
            ],
        );
        let fc = linear(s.fog_color);
        c.f("unity_FogColor", &fc);
        // unity_FogParams: (density/sqrt(ln 2), density/ln 2, -1/(end-start), end/(end-start)).
        let d = s.fog_density;
        let span = (s.fog_end - s.fog_start).max(1e-4);
        c.f(
            "unity_FogParams",
            &[
                d * std::f32::consts::LN_2.sqrt().recip(),
                d / std::f32::consts::LN_2,
                -1.0 / span,
                s.fog_end / span,
            ],
        );
        c.v4("unity_ShadowColor", Vec4::from(linear(s.subtractive_shadow_color)));
        // `ShadowCommonBuffer`: texel size of the atlas, the linear distance fade
        // (`ShadowCasterPass.GetScaleAndBiasForLinearDistanceFade`: scale 1/(0.1 d), bias
        // -0.9 d/(0.1 d)) and the tetrahedral face vectors (float3s at 16-byte stride).
        let a = self.shadow.settings.atlas_size as f32;
        c.f("_ShadowmapRT_TexelSize", &[1.0 / a, 1.0 / a, a, a]);
        c.i("_ShadowCurrentAlgorithm", &[0]);
        let d = self.shadow.settings.distance;
        let fade = d - 0.9 * d;
        c.f("_ShadowFadeDistanceScaleAndBias", &[1.0 / fade, -(0.9 * d) / fade]);
        let fv: Vec<f32> =
            shadows::FACE_VECTORS.iter().flat_map(|v| [v.x, v.y, v.z, 0.0]).collect();
        c.f("_FaceVectors", &fv);
        c.f("_FogOfWarGlobalFlag", &[0.0]);
        c.f("_OccludedObjectHighlightingFeatureEnabled", &[0.0]);
        c.f("unity_StereoScaleOffset", &[1.0, 1.0, 0.0, 0.0]);
        c.f("_DepthPyramidSamplingRatio", &[1.0, 1.0]);
        c.f("_FogOfWarMask_ST", &[1.0, 1.0, 0.0, 0.0]);
    }

    /// Sets every camera-dependent constant and rebuilds the light buffers.
    pub fn set_camera(&mut self, queue: &wgpu::Queue, cam: &CameraPose, time: f32) {
        let c = &mut self.consts;
        let view = cam.world_to_camera;
        let proj = cam.gpu_projection;
        let vp = proj * view;
        let (w, h) = (cam.width as f32, cam.height as f32);
        let (n, f) = (cam.near, cam.far);
        c.f("_Time", &[time / 20.0, time, time * 2.0, time * 3.0]);
        c.f("_SinTime", &[(time / 8.0).sin(), (time / 4.0).sin(), (time / 2.0).sin(), time.sin()]);
        c.f("_CosTime", &[(time / 8.0).cos(), (time / 4.0).cos(), (time / 2.0).cos(), time.cos()]);
        c.f("_UnscaledTime", &[time / 20.0, time, time * 2.0, time * 3.0]);
        c.f("unity_DeltaTime", &[1.0 / 80.0, 80.0, 1.0 / 80.0, 80.0]);
        c.v3("_WorldSpaceCameraPos", cam.position);
        // x = -1: rendering with the flipped "into texture" projection.
        c.f("_ProjectionParams", &[-1.0, n, f, 1.0 / f]);
        c.f("_ScreenParams", &[w, h, 1.0 + 1.0 / w, 1.0 + 1.0 / h]);
        c.f("_ScreenSize", &[w, h, 1.0 / w, 1.0 / h]);
        // Reversed Z.
        let zx = -1.0 + f / n;
        c.f("_ZBufferParams", &[zx, 1.0, zx / f, 1.0 / f]);
        c.f("unity_OrthoParams", &[0.0, 0.0, 0.0, 0.0]);
        c.m("glstate_matrix_projection", proj);
        c.m("unity_MatrixV", view);
        c.m("unity_MatrixInvV", view.inverse());
        c.m("unity_MatrixVP", vp);
        c.m("_ViewMatrix", view);
        c.m("_InvViewMatrix", view.inverse());
        c.m("_ProjMatrix", proj);
        c.m("_InvProjMatrix", proj.inverse());
        c.m("_ViewProjMatrix", vp);
        c.m("_NonJitteredViewProjMatrix", vp);
        c.m("_PrevViewProjMatrix", vp);
        c.m("_InvViewProjMatrix", vp.inverse());
        c.m("_InvCameraViewProj", vp.inverse());
        let inv_view = view.inverse();
        c.v3("_CamBasisUp", inv_view.y_axis.truncate());
        c.v3("_CamBasisSide", inv_view.x_axis.truncate());
        c.v3("_CamBasisFront", -inv_view.z_axis.truncate());
        // Lights.
        let mut lights_now = self.runtime_lights.clone();
        if let (Some(l), Some((pos, rot))) = (&self.rig_light, cam.rig) {
            let mut l = l.clone();
            let world =
                Mat4::from_rotation_translation(rot, pos) * Mat4::from_cols_array(&l.matrix);
            l.matrix = world.to_cols_array();
            lights_now.push(l);
        }
        let vis = lights::visible_lights(&lights_now, vp);
        let plan = self.plan_shadows(queue, cam, vp, &vis);
        let lb = lights::build(&vis, &plan.slot, view, n, f, cam.width, cam.height, TILE);
        self.lights_visible = lb.visible;
        self.consts.f("_ClusteringParams", &lb.clustering);
        self.consts.f("_LightDataParams", &lb.params);
        queue.write_buffer(
            &self.light_data,
            0,
            &lb.light_data.iter().flatten().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
        );
        queue.write_buffer(
            &self.light_volume,
            0,
            &lb.volume_data.iter().flatten().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
        );
        queue.write_buffer(
            &self.zbins,
            0,
            &lb.zbins.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
        );
        // LightCullingPass.GetScreenProjMatrix: viewport transform × projectionMatrix.
        let screen = Mat4::from_cols(
            Vec4::new(0.5 * w, 0.0, 0.0, 0.0),
            Vec4::new(0.0, 0.5 * h, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.5, 0.0),
            Vec4::new(0.5 * w, 0.5 * h, 0.5, 1.0),
        );
        self.culling_cb.m("_ScreenProjMatrix", screen * cam.projection);
        let tiles_n = (cam.width.div_ceil(TILE) * cam.height.div_ceil(TILE) * 32) as i32;
        self.culling_cb.i("_LightTilesBufferUAVSize", &[tiles_n]);
        // Rewrite every camera-scope constant buffer.
        for g in &self.globals {
            let data = fill(&g.layout, &[&self.culling_cb, &self.consts], &mut self.missing);
            queue.write_buffer(&g.buffer, 0, &data);
        }
    }

    /// `GetShadowCasterBounds(i)` (native in Unity), approximated: some shadow caster
    /// whose layer the light's culling mask includes lies within the light's range, and the
    /// light is within shadow distance of the camera [I].
    fn light_has_casters(&self, l: &wr_assets::Light, cam: Vec3) -> bool {
        let m = Mat4::from_cols_array(&l.matrix);
        let pos = m.w_axis.truncate();
        if (pos - cam).length() - l.range > self.shadow.settings.distance {
            return false;
        }
        self.casters.iter().any(|c| {
            c.cast != 0
                && light_reaches(l, c.layer, c.rendering_layer_mask)
                && shadows::box_in_sphere(c.lo, c.hi, pos, l.range)
        })
    }

    /// Plans the frame's shadows for `vis` (unsorted, in visibility order), uploads the
    /// two structured buffers and the per-face caster constants, and culls each face's
    /// casters. Returns the plan (the lights need its shadow indices).
    fn plan_shadows(
        &mut self,
        queue: &wgpu::Queue,
        cam: &CameraPose,
        vp: Mat4,
        vis: &[&wr_assets::Light],
    ) -> shadows::Plan {
        self.shadow_faces.clear();
        self.shadow_stats = ShadowStats::default();
        if !self.shadows_on || self.casters.is_empty() {
            return shadows::Plan { slot: vec![None; vis.len()], ..Default::default() };
        }
        let info = shadows::CameraInfo { width: cam.width, height: cam.height, view_proj: vp };
        let plan = shadows::plan(&self.shadow.settings, &info, vis, &|i| {
            self.light_has_casters(vis[i], cam.position)
        });
        queue.write_buffer(&self.shadow.data, 0, &plan.data_bytes());
        queue.write_buffer(&self.shadow.matrices, 0, &plan.matrix_bytes());
        let clips = shadows::point_clips();
        let mut records: Vec<u8> = Vec::new();
        let layout = self.shadow.cb_layout.clone();
        let mut missing = BTreeSet::new();
        for (ei, e) in plan.entries.iter().enumerate() {
            for f in 0..e.faces() {
                // `ShadowCasterPass.Execute` (:42): per entry `_ShadowEntryIndex`, per face
                // `_FaceId`; points also `_Clips`, `_LightDirection` = (position, 1),
                // `_PunctualNearClip` and a face count of 4.
                let mut c = Consts::default();
                c.i("_ShadowEntryIndex", &[ei as i32]);
                c.i("_FaceId", &[f as i32]);
                c.i("_ShadowFaceCount", &[if e.point { 4 } else { 1 }]);
                if e.point {
                    let cl: Vec<f32> = clips.iter().flat_map(|v| [v.x, v.y, v.z, 0.0]).collect();
                    c.f("_Clips", &cl);
                    c.f("_PunctualNearClip", &[e.near_plane]);
                    c.f("_LightDirection", &[e.position.x, e.position.y, e.position.z, 1.0]);
                }
                let mut rec =
                    layout.as_ref().map(|l| fill(l, &[&c], &mut missing)).unwrap_or_default();
                rec.resize(CASTER_RECORD as usize, 0);
                let offset = (records.len()) as u32;
                records.extend_from_slice(&rec);
                // Casters this face can see: layer mask, shadow-casting mode, and the
                // face's frustum (points: and the range sphere).
                let light = vis[e.light];
                let draws: Vec<u32> = self
                    .casters
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| {
                        c.cast != 0
                            && light_reaches(light, c.layer, c.rendering_layer_mask)
                            && shadows::box_in_sphere(c.lo, c.hi, e.position, e.range)
                            && shadows::box_in_frustum(e.cull[f], c.lo, c.hi)
                    })
                    .map(|(i, _)| i as u32)
                    .collect();
                self.shadow_stats.draws += draws.len();
                self.shadow_faces.push(ShadowFace {
                    viewport: e.viewport,
                    offset,
                    clip: e.point,
                    slope_bias: if e.point { (-e.depth_bias).to_bits() } else { 0 },
                    draws,
                });
            }
        }
        if !records.is_empty() {
            queue.write_buffer(&self.shadow.caster_cb, 0, &records);
        }
        let st = &mut self.shadow_stats;
        st.candidates = vis.iter().filter(|l| l.shadows != 0).count();
        st.entries = plan.entries.len();
        st.points = plan.entries.iter().filter(|e| e.point).count();
        st.spots = st.entries - st.points;
        st.dropped = plan.dropped;
        st.faces = self.shadow_faces.len();
        st.tiles = plan
            .entries
            .iter()
            .map(|e| (e.viewport.w as u32, e.viewport.x as u32, e.viewport.y as u32, e.point))
            .collect();
        plan
    }

    fn global(&mut self, device: &wgpu::Device, layout: &CBufferLayout) -> usize {
        let key = format!("{}:{}:{}", layout.ty, layout.size, layout.members.len());
        if let Some(&i) = self.global_index.get(&key) {
            return i;
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(&layout.ty),
            size: u64::from(layout.size.max(16)).div_ceil(16) * 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.globals.push(GlobalCb { layout: layout.clone(), buffer });
        let i = self.globals.len() - 1;
        self.global_index.insert(key, i);
        i
    }

    fn layout(&mut self, device: &wgpu::Device, progs: &[&Program]) -> String {
        let key =
            progs.iter().map(|p| format!("{:016x}", p.msl_hash)).collect::<Vec<_>>().join("+");
        if !self.layouts.contains_key(&key) {
            let e = layout_entries(progs);
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("owlcat"),
                entries: &e.iter().map(|x| x.2).collect::<Vec<_>>(),
            });
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("owlcat"),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });
            self.layouts
                .insert(key.clone(), (bgl, pl, e.into_iter().map(|(s, b, _)| (s, b)).collect()));
        }
        key
    }

    #[expect(clippy::too_many_arguments, reason = "pipeline description")]
    fn render_pipeline(
        &mut self,
        device: &wgpu::Device,
        vs: &Program,
        fs: &Program,
        state: &RenderState,
        vl: Option<&VertexLayout>,
        formats: &[wgpu::TextureFormat],
        depth: Option<wgpu::TextureFormat>,
    ) -> usize {
        let lkey = self.layout(device, &[vs, fs]);
        let key = format!("{lkey}|{state:?}|{vl:?}|{formats:?}|{depth:?}");
        if let Some(&i) = self.pipeline_index.get(&key) {
            return i;
        }
        let pack = Arc::clone(&self.pack);
        let vm = self.gpu.module_for(device, &pack, vs);
        let fm = self.gpu.module_for(device, &pack, fs);
        let mut buffers: Vec<Option<wgpu::VertexBufferLayout<'_>>> = Vec::new();
        let attrs: Vec<Vec<wgpu::VertexAttribute>> = vl
            .map(|vl| {
                let mut v: Vec<Vec<wgpu::VertexAttribute>> = vl
                    .streams
                    .iter()
                    .map(|(_, _, a)| {
                        a.iter()
                            .map(|(f, o, l)| wgpu::VertexAttribute {
                                format: *f,
                                offset: *o,
                                shader_location: *l,
                            })
                            .collect()
                    })
                    .collect();
                v.extend(vl.defaults.iter().map(|(l, f, _)| {
                    vec![wgpu::VertexAttribute { format: *f, offset: 0, shader_location: *l }]
                }));
                v
            })
            .unwrap_or_default();
        if let Some(vl) = vl {
            for (i, (_, stride, _)) in vl.streams.iter().enumerate() {
                buffers.push(Some(wgpu::VertexBufferLayout {
                    array_stride: *stride,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs[i],
                }));
            }
            for (k, _) in vl.defaults.iter().enumerate() {
                buffers.push(Some(wgpu::VertexBufferLayout {
                    array_stride: 0,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs[vl.streams.len() + k],
                }));
            }
        }
        let targets: Vec<Option<wgpu::ColorTargetState>> =
            formats.iter().enumerate().map(|(i, f)| Some(state.color_target(i, *f))).collect();
        let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(&fs.shader),
            layout: Some(&self.layouts[&lkey].1),
            vertex: wgpu::VertexState {
                module: &vm,
                entry_point: Some(&vs.entry),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: state.primitive(self.front_ccw),
            depth_stencil: state.depth(depth),
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
        self.pipelines.push(p);
        let i = self.pipelines.len() - 1;
        self.pipeline_index.insert(key, i);
        i
    }

    /// Resolves one binding to a planned resource.
    #[expect(clippy::too_many_arguments, reason = "binding context")]
    fn plan(
        &mut self,
        device: &wgpu::Device,
        stage: Stage,
        b: &Binding,
        prog: &Program,
        ctx: &Bind<'_>,
        sources: &[&Consts],
        arena: &mut Vec<u8>,
        shader: Option<&wr_assets::Shader>,
    ) -> Res {
        let _ = stage;
        match &b.res {
            // Light buffers are bound by name later (`patch_light_buffers`).
            Resource::Uniform { .. }
                if matches!(b.name.as_str(), "LightDataCB" | "ZBinsCB" | "LightVolumeDataCB") =>
            {
                Res::Buffer(self.light_data.clone())
            }
            Resource::Uniform { ty } => {
                let Some(layout) = prog.cbuffers.iter().find(|c| &c.ty == ty).cloned() else {
                    return Res::Invalid(format!("{}: no layout for {ty}", prog.shader));
                };
                if b.name == pipeline::SHADOW_CASTER_CB {
                    // Per (entry, face): a record of the caster buffer at a dynamic offset.
                    self.shadow.cb_layout.get_or_insert_with(|| layout.clone());
                    return Res::Caster(u64::from(layout.size.max(16)));
                }
                if scope(ty) == 0 && !ctx.local_cbs {
                    Res::Global(self.global(device, &layout))
                } else {
                    let data = fill(&layout, sources, &mut self.missing);
                    let off = (arena.len() as u64).div_ceil(256) * 256;
                    arena.resize(off as usize, 0);
                    arena.extend_from_slice(&data);
                    Res::Arena(off, data.len() as u64)
                }
            }
            Resource::Storage { .. } => match b.name.as_str() {
                "_LightTilesBuffer" | "_LightTilesBufferUAV" => Res::Buffer(self.tiles.clone()),
                "_ShadowDataBuffer" => Res::Buffer(self.shadow.data.clone()),
                "_ShadowMatricesBuffer" => Res::Buffer(self.shadow.matrices.clone()),
                other => {
                    Res::Invalid(format!("{}: unsupported storage buffer {other}", prog.shader))
                }
            },
            Resource::Sampler if b.name == pipeline::SHADOW_SAMPLER => {
                Res::Sampler(self.shadow.sampler.clone())
            }
            Resource::Sampler => {
                let tex = b.name.strip_prefix("sampler").unwrap_or(&b.name);
                if let Some((_, _, Some(smp))) = ctx.extra_tex.iter().find(|(n, _, _)| *n == tex) {
                    return Res::Sampler(smp.clone());
                }
                if let Some(m) = ctx.material
                    && let Some(tp) = m.textures.iter().find(|t| t.name == tex)
                    && let Some(Some(g)) = tp
                        .texture
                        .map(|i| self.gpu.textures.get(i as usize).and_then(|x| x.as_ref()))
                {
                    return Res::Sampler(g.sampler.clone());
                }
                if tex.starts_with("unity_")
                    || tex.starts_with("_Camera")
                    || tex.starts_with("_SpecCube")
                {
                    return Res::Sampler(self.gpu.defaults.sampler_clamp.clone());
                }
                Res::Sampler(self.gpu.defaults.sampler.clone())
            }
            Resource::Texture { kind, .. } => {
                let name = b.name.as_str();
                if name == "_ShadowmapRT" {
                    return Res::View(self.shadow.view.clone());
                }
                if let Some((_, v, _)) = ctx.extra_tex.iter().find(|(n, _, _)| *n == name) {
                    return Res::View(v.clone());
                }
                if let Some(v) = self.targets.by_name(name) {
                    return Res::View(v.clone());
                }
                let tex_index = |i: Option<u32>| {
                    i.and_then(|i| self.gpu.textures.get(i as usize).and_then(|x| x.as_ref()))
                };
                if let Some((color, dir, mask)) = ctx.lightmap {
                    let pick = match name {
                        "unity_Lightmap" => Some(color),
                        "unity_LightmapInd" => Some(dir),
                        "unity_ShadowMask" => Some(mask),
                        _ => None,
                    };
                    if let Some(p) = pick
                        && let Some(g) = tex_index(p)
                    {
                        return Res::View(g.view.clone());
                    }
                }
                if let Some(m) = ctx.material
                    && let Some(tp) = m.textures.iter().find(|t| t.name == name)
                    && let Some(g) = tex_index(tp.texture)
                {
                    return Res::View(g.view.clone());
                }
                let def = shader
                    .and_then(|s| s.properties.iter().find(|p| p.name == name))
                    .and_then(|p| p.default_texture.clone());
                let def = def.or_else(|| match name {
                    "unity_ShadowMask" | "_FogOfWarMask" => Some("white".to_owned()),
                    _ => None,
                });
                Res::View(self.gpu.defaults.view(*kind, def.as_deref()).clone())
            }
        }
    }

    fn materialize(
        &self,
        device: &wgpu::Device,
        lkey: &str,
        plan: &[Res],
        arena: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        let (bgl, _, _) = &self.layouts[lkey];
        let entries: Vec<wgpu::BindGroupEntry<'_>> = plan
            .iter()
            .enumerate()
            .map(|(i, r)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: match r {
                    Res::Arena(off, size) => wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: arena,
                        offset: *off,
                        size: std::num::NonZeroU64::new(*size),
                    }),
                    Res::Global(g) => self.globals[*g].buffer.as_entire_binding(),
                    Res::Buffer(b) => b.as_entire_binding(),
                    Res::Caster(size) => wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.shadow.caster_cb,
                        offset: 0,
                        size: std::num::NonZeroU64::new(*size),
                    }),
                    Res::View(v) => wgpu::BindingResource::TextureView(v),
                    Res::Sampler(s) => wgpu::BindingResource::Sampler(s),
                    Res::Invalid(_) => {
                        unreachable!("invalid plans are filtered before materializing")
                    }
                },
            })
            .collect();
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: bgl,
            entries: &entries,
        })
    }

    /// Material constants: shader property defaults, then the material's values, with
    /// colours converted to linear (unless declared `[Gamma]`) and texture `_ST`,
    /// `_TexelSize` and `_HDR` vectors.
    fn material_consts(&self, m: &Material) -> Consts {
        let shader = &self.pack.meta.shaders[m.shader as usize];
        let mut c = Consts::default();
        // In a linear-space project Unity converts colour properties from sRGB to linear,
        // and float/vector properties marked `[Gamma]` (flag 32) as well.
        let gamma =
            |name: &str| shader.properties.iter().any(|p| p.name == name && p.flags & 32 != 0);
        let lin1 = |v: f32| consts::gamma_to_linear(v);
        for p in &shader.properties {
            let g = p.flags & 32 != 0;
            match p.kind {
                0 => c.f(&p.name, &linear(p.default_value)),
                1 => c.f(&p.name, &if g { linear(p.default_value) } else { p.default_value }),
                2 | 3 => {
                    c.f(&p.name, &[if g { lin1(p.default_value[0]) } else { p.default_value[0] }])
                }
                _ => {}
            }
        }
        for (k, v) in &m.floats {
            c.f(k, &[if gamma(k) { lin1(*v) } else { *v }]);
        }
        for (k, v) in &m.colors {
            let decl = shader.properties.iter().find(|p| &p.name == k);
            // Undeclared entries of `m_Colors` are taken as colours.
            let convert = decl.is_none_or(|p| p.kind == 0 || p.flags & 32 != 0);
            c.f(k, &if convert { linear(*v) } else { *v });
        }
        for t in &m.textures {
            c.f(&format!("{}_ST", t.name), &[t.scale[0], t.scale[1], t.offset[0], t.offset[1]]);
            let (w, h) = t
                .texture
                .and_then(|i| self.gpu.textures.get(i as usize).and_then(|x| x.as_ref()))
                .map_or((4, 4), |g| g.size);
            c.f(
                &format!("{}_TexelSize", t.name),
                &[1.0 / w as f32, 1.0 / h as f32, w as f32, h as f32],
            );
            c.f(&format!("{}_HDR", t.name), &[1.0, 1.0, 0.0, 0.0]);
        }
        c
    }

    fn draw_consts(&self, matrix: Mat4, lightmap_st: [f32; 4]) -> Consts {
        let mut c = Consts::default();
        c.m("unity_ObjectToWorld", matrix);
        c.m("unity_WorldToObject", matrix.inverse());
        c.f(
            "unity_WorldTransformParams",
            &[0.0, 0.0, 0.0, if matrix.determinant() < 0.0 { -1.0 } else { 1.0 }],
        );
        c.f("unity_LightmapST", &lightmap_st);
        c.f("unity_DynamicLightmapST", &[1.0, 1.0, 0.0, 0.0]);
        c.f("unity_ProbesOcclusion", &[1.0; 4]);
        c.f("unity_LODFade", &[0.0; 4]);
        c.f("unity_RenderingLayer", &[1.0, 0.0, 0.0, 0.0]);
        // Light probes aren't interpolated yet: L0 of the scene's ambient probe.
        let p = &self.scene_settings.ambient_probe;
        let l0 = |ch: usize| p.get(ch * 9).copied().unwrap_or(0.0) * 0.282_095;
        for (k, ch) in [("unity_SHAr", 0), ("unity_SHAg", 1), ("unity_SHAb", 2)] {
            c.f(k, &[0.0, 0.0, 0.0, l0(ch)]);
        }
        c.f("unity_SpecCube0_HDR", &[1.0, 1.0, 0.0, 0.0]);
        c
    }

    fn build(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> Result<()> {
        let pack = Arc::clone(&self.pack);
        let meta = &pack.meta;
        let t = Instant::now();
        let mut arena: Vec<u8> = Vec::new();
        // Plans: (pass, pipeline, layout key, resources, draw data).
        struct Planned {
            pass: u8,
            pipeline: usize,
            lkey: String,
            plan: Vec<Res>,
            vbufs: Vec<wgpu::Buffer>,
            index: Option<(wgpu::Buffer, wgpu::IndexFormat)>,
            range: std::ops::Range<u32>,
            base_vertex: i32,
            /// Post steps: label, target, whether it's the grading LUT.
            post: Option<(String, wgpu::TextureView, bool)>,
            /// Shadow casters: index into `self.casters`, `GEOMETRY_CLIP` state, and the
            /// pipelines per raster slope bias.
            caster: Option<CasterPlan>,
        }
        let mut planned: Vec<Planned> = Vec::new();
        let gformats = GBUFFER.to_vec();
        let mut mat_consts: HashMap<u32, Consts> = HashMap::new();
        let globals_now = self.consts.clone();
        // G-buffer draws.
        for r in &meta.renderers {
            if !(r.enabled && r.active) {
                continue;
            }
            let mesh = &meta.meshes[r.mesh as usize];
            let (gstreams, gindex) = (
                self.gpu.meshes[r.mesh as usize].streams.clone(),
                self.gpu.meshes[r.mesh as usize].index.clone(),
            );
            let batched = r.static_batch.1 > 0;
            let matrix = if batched { Mat4::IDENTITY } else { Mat4::from_cols_array(&r.matrix) };
            // Static batching bakes each renderer's lightmap scale/offset into the combined
            // mesh's UV1, so batched renderers draw with the identity transform.
            let lightmap_st = if batched { [1.0, 1.0, 0.0, 0.0] } else { r.lightmap_st };
            let mut dc = self.draw_consts(matrix, lightmap_st);
            let mut pb_tex: Vec<(&str, wgpu::TextureView, Option<wgpu::Sampler>)> = Vec::new();
            if let Some(pb) = &r.property_block {
                for (k, v) in &pb.floats {
                    dc.f(k, &[*v]);
                }
                for (k, v) in &pb.vectors {
                    dc.f(k, &v.iter().flatten().copied().collect::<Vec<f32>>());
                }
                for (k, t) in &pb.textures {
                    if let Some(Some(g)) = self.gpu.textures.get(*t as usize) {
                        pb_tex.push((k.as_str(), g.view.clone(), Some(g.sampler.clone())));
                    }
                }
            }
            let lm = meta
                .lightmaps
                .iter()
                .find(|l| l.scene == r.scene && l.index == u32::from(r.lightmap_index))
                .map(|l| (l.color, l.direction, l.shadowmask));
            for (slot, mi) in r.materials.iter().enumerate() {
                let Some(mi) = mi else { continue };
                let m = &meta.materials[*mi as usize];
                let sub = if batched {
                    usize::from(r.static_batch.0) + slot
                } else {
                    slot.min(mesh.submeshes.len().saturating_sub(1))
                };
                let Some(sm) = mesh.submeshes.get(sub) else { continue };
                if sm.topology != 0 {
                    continue;
                }
                // Shadow casters (`ShadowCasterPass`): the material's own `ShadowCaster`
                // programs, one draw per `GEOMETRY_CLIP` state, for every light that
                // reaches this renderer (culled per face in `plan_shadows`).
                if self.shadows_on && r.cast_shadows != 0 && !m.casters.is_empty() {
                    let shader = &meta.shaders[m.shader as usize];
                    if let Some(cpass) = shader
                        .passes
                        .iter()
                        .find(|p| p.light_mode().eq_ignore_ascii_case("ShadowCaster"))
                    {
                        let local = sm.bounds.unwrap_or_else(|| {
                            let (c, e) = mesh.aabb;
                            (
                                [c[0] - e[0], c[1] - e[1], c[2] - e[2]],
                                [c[0] + e[0], c[1] + e[1], c[2] + e[2]],
                            )
                        });
                        let (lo, hi) = world_bounds(local, matrix);
                        let ci = self.casters.len();
                        let mc = mat_consts
                            .entry(*mi)
                            .or_insert_with(|| self.material_consts(m))
                            .clone();
                        let mut any = false;
                        for cv in &m.casters {
                            let vs = &meta.programs[cv.vertex as usize];
                            let fs = &meta.programs[cv.fragment as usize];
                            let Ok(vl) = vertex_layout(vs, mesh) else { continue };
                            let base = caster_state(
                                resolve_state(&cpass.state, Some(m), 0),
                                r.cast_shadows,
                            );
                            let biases =
                                if cv.geometry_clip { self.point_biases.clone() } else { vec![0] };
                            let mut pipes = Vec::new();
                            for b in biases {
                                let st = base.clone().with_slope_bias(f32::from_bits(b));
                                let pi = self.render_pipeline(
                                    device,
                                    vs,
                                    fs,
                                    &st,
                                    Some(&vl),
                                    &[],
                                    Some(SHADOW_FORMAT),
                                );
                                pipes.push((b, pi));
                            }
                            let lkey = self.layout(device, &[vs, fs]);
                            let ctx = Bind {
                                material: Some(m),
                                lightmap: None,
                                extra_tex: &pb_tex,
                                local_cbs: false,
                            };
                            let entries = self.layouts[&lkey].2.clone();
                            let mut plan = Vec::new();
                            for (stage, b) in &entries {
                                let prog = if *stage == Stage::Vertex { vs } else { fs };
                                plan.push(self.plan(
                                    device,
                                    *stage,
                                    b,
                                    prog,
                                    &ctx,
                                    &[&dc, &mc, &globals_now],
                                    &mut arena,
                                    Some(shader),
                                ));
                            }
                            let mut vbufs: Vec<wgpu::Buffer> =
                                vl.streams.iter().map(|(si, _, _)| gstreams[*si].clone()).collect();
                            vbufs.extend(vl.defaults.iter().map(|(_, _, white)| {
                                if *white {
                                    self.defaults_vb.1.clone()
                                } else {
                                    self.defaults_vb.0.clone()
                                }
                            }));
                            let fmt = if mesh.index_u32 {
                                wgpu::IndexFormat::Uint32
                            } else {
                                wgpu::IndexFormat::Uint16
                            };
                            planned.push(Planned {
                                pass: 5,
                                pipeline: pipes[0].1,
                                lkey,
                                plan,
                                vbufs,
                                index: Some((gindex.clone(), fmt)),
                                range: sm.first_index..sm.first_index + sm.index_count,
                                base_vertex: sm.base_vertex,
                                post: None,
                                caster: Some((ci, cv.geometry_clip, pipes)),
                            });
                            any = true;
                        }
                        if any {
                            self.casters.push(Caster {
                                lo,
                                hi,
                                layer: r.layer,
                                cast: r.cast_shadows,
                                rendering_layer_mask: r.rendering_layer_mask,
                                binds: [None, None],
                            });
                        }
                    }
                }
                let rkw: Vec<String> = if lm.is_some() {
                    m.variants
                        .iter()
                        .find(|v| v.renderer_keywords.iter().any(|k| k == "LIGHTMAP_ON"))
                        .map(|v| v.renderer_keywords.clone())
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                let Some(var) = m
                    .variants
                    .iter()
                    .find(|v| v.renderer_keywords == rkw)
                    .or_else(|| m.variants.first())
                else {
                    continue;
                };
                let vs = &meta.programs[var.vertex as usize];
                let fs = &meta.programs[var.fragment as usize];
                let shader = &meta.shaders[m.shader as usize];
                let Some(pass) =
                    shader.passes.iter().find(|p| p.light_mode().eq_ignore_ascii_case("GBuffer"))
                else {
                    continue;
                };
                let state = resolve_state(&pass.state, Some(m), 6);
                let vl = match vertex_layout(vs, mesh) {
                    Ok(v) => v,
                    Err(e) => {
                        self.stats.skipped.push(format!("{}: {e}", r.path));
                        continue;
                    }
                };
                let pipeline =
                    self.render_pipeline(device, vs, fs, &state, Some(&vl), &gformats, Some(DEPTH));
                let lkey = self.layout(device, &[vs, fs]);
                let mc = mat_consts.entry(*mi).or_insert_with(|| self.material_consts(m)).clone();
                let ctx =
                    Bind { material: Some(m), lightmap: lm, extra_tex: &pb_tex, local_cbs: false };
                let entries = self.layouts[&lkey].2.clone();
                let mut plan = Vec::new();
                for (stage, b) in &entries {
                    let prog = if *stage == Stage::Vertex { vs } else { fs };
                    plan.push(self.plan(
                        device,
                        *stage,
                        b,
                        prog,
                        &ctx,
                        &[&dc, &mc, &globals_now],
                        &mut arena,
                        Some(shader),
                    ));
                }
                let mut vbufs: Vec<wgpu::Buffer> =
                    vl.streams.iter().map(|(s, _, _)| gstreams[*s].clone()).collect();
                vbufs.extend(vl.defaults.iter().map(|(_, _, white)| {
                    if *white { self.defaults_vb.1.clone() } else { self.defaults_vb.0.clone() }
                }));
                let fmt = if mesh.index_u32 {
                    wgpu::IndexFormat::Uint32
                } else {
                    wgpu::IndexFormat::Uint16
                };
                planned.push(Planned {
                    pass: 0,
                    pipeline,
                    lkey,
                    plan,
                    vbufs,
                    index: Some((gindex.clone(), fmt)),
                    range: sm.first_index..sm.first_index + sm.index_count,
                    base_vertex: sm.base_vertex,
                    post: None,
                    caster: None,
                });
            }
        }
        // Pipeline passes.
        let find = |role: &str| meta.pipeline_programs.iter().find(|p| p.role == role);
        let fullscreen = |s: &mut Self,
                          vsr: &str,
                          fsr: &str,
                          formats: &[wgpu::TextureFormat],
                          extra: &[(&str, wgpu::TextureView, Option<wgpu::Sampler>)],
                          fg: &Consts,
                          arena: &mut Vec<u8>|
         -> Option<(usize, String, Vec<Res>)> {
            let (v, f) = (find(vsr)?, find(fsr)?);
            let (vs, fs) = (&meta.programs[v.program as usize], &meta.programs[f.program as usize]);
            let state = resolve_state(
                f.state.as_ref().unwrap_or(&PassState::default()),
                None,
                formats.len(),
            );
            let p = s.render_pipeline(device, vs, fs, &state, None, formats, None);
            let lkey = s.layout(device, &[vs, fs]);
            let ctx = Bind { material: None, lightmap: None, extra_tex: extra, local_cbs: false };
            let entries = s.layouts[&lkey].2.clone();
            let globals = s.consts.clone();
            let mut plan = Vec::new();
            for (stage, b) in &entries {
                let prog = if *stage == Stage::Vertex { vs } else { fs };
                plan.push(s.plan(device, *stage, b, prog, &ctx, &[fg, &globals], arena, None));
            }
            Some((p, lkey, plan))
        };
        let refl_view = |i: Option<u32>, s: &Self| {
            i.and_then(|i| s.gpu.textures.get(i as usize).and_then(|x| x.as_ref()))
                .map(|g| (g.view.clone(), g.sampler.clone()))
        };
        // Deferred reflections: default reflection (scene's generated skybox cubemap).
        let (sky, sky_sampler) =
            refl_view(self.scene_settings.reflection, self).unwrap_or_else(|| {
                (self.gpu.defaults.black_cube.clone(), self.gpu.defaults.sampler_clamp.clone())
            });
        let mut fg = Consts::default();
        fg.f("_SpecCube0_HDR", &[1.0, 1.0, 0.0, 0.0]);
        fg.f("_UseBoxProjection", &[0.0]);
        let extra = [("_SpecCube0", sky, Some(sky_sampler))];
        if let Some((p, l, plan)) = fullscreen(
            self,
            "Hidden/Owlcat/DeferredReflections#0.vs[]",
            "Hidden/Owlcat/DeferredReflections#0.fs[]",
            &[REFLECTIONS],
            &extra,
            &fg,
            &mut arena,
        ) {
            planned.push(Planned {
                pass: 1,
                pipeline: p,
                lkey: l,
                plan,
                vbufs: vec![],
                index: None,
                range: 0..3,
                base_vertex: 0,
                post: None,
                caster: None,
            });
        }
        // Reflection probes (cube volumes; Unity sorts by importance, then size).
        let cube = cube_mesh(device);
        let mut probes: Vec<&wr_assets::ReflectionProbe> = meta
            .reflection_probes
            .iter()
            .filter(|p| p.enabled && p.active && p.texture.is_some())
            .collect();
        probes.sort_by(|a, b| {
            a.importance.cmp(&b.importance).then(
                (b.box_size[0] * b.box_size[1] * b.box_size[2])
                    .total_cmp(&(a.box_size[0] * a.box_size[1] * a.box_size[2])),
            )
        });
        if let (Some(v), Some(f)) = (
            find("Hidden/Owlcat/DeferredReflections#1.vs[]"),
            find("Hidden/Owlcat/DeferredReflections#1.fs[]"),
        ) {
            let (vs, fs) = (&meta.programs[v.program as usize], &meta.programs[f.program as usize]);
            let state = resolve_state(f.state.as_ref().unwrap_or(&PassState::default()), None, 1);
            let cube_layout = VertexLayout {
                streams: vec![(
                    0,
                    12,
                    vec![(
                        wgpu::VertexFormat::Float32x3,
                        0,
                        vs.vertex_inputs.first().map_or(0, |i| i.attribute),
                    )],
                )],
                defaults: vec![],
            };
            let pipeline = self.render_pipeline(
                device,
                vs,
                fs,
                &state,
                Some(&cube_layout),
                &[REFLECTIONS],
                None,
            );
            let lkey = self.layout(device, &[vs, fs]);
            for p in probes {
                let center = Vec3::from(p.position) + Vec3::from(p.box_offset);
                let size = Vec3::from(p.box_size);
                let m =
                    Mat4::from_scale_rotation_translation(size, bevy::math::Quat::IDENTITY, center);
                let dc = self.draw_consts(m, [1.0, 1.0, 0.0, 0.0]);
                let mut fg = Consts::default();
                let half = size * 0.5;
                fg.f("_SpecCube0_HDR", &[p.intensity, 1.0, 0.0, 0.0]);
                fg.v4("_SpecCube0_ProbePosition", Vec3::from(p.position).extend(p.blend_distance));
                fg.v4("_SpecCube0_BoxMin", (center - half).extend(0.0));
                fg.v4("_SpecCube0_BoxMax", (center + half).extend(0.0));
                fg.f("_UseBoxProjection", &[if p.box_projection { 1.0 } else { 0.0 }]);
                let (view, smp) = refl_view(p.texture, self).unwrap_or_else(|| {
                    (self.gpu.defaults.black_cube.clone(), self.gpu.defaults.sampler_clamp.clone())
                });
                let extra = [("_SpecCube0", view, Some(smp))];
                let ctx =
                    Bind { material: None, lightmap: None, extra_tex: &extra, local_cbs: false };
                let entries = self.layouts[&lkey].2.clone();
                let globals = self.consts.clone();
                let mut plan = Vec::new();
                for (stage, b) in &entries {
                    let prog = if *stage == Stage::Vertex { vs } else { fs };
                    plan.push(self.plan(
                        device,
                        *stage,
                        b,
                        prog,
                        &ctx,
                        &[&dc, &fg, &globals],
                        &mut arena,
                        None,
                    ));
                }
                planned.push(Planned {
                    pass: 1,
                    pipeline,
                    lkey: lkey.clone(),
                    plan,
                    vbufs: vec![cube.0.clone()],
                    index: Some((cube.1.clone(), wgpu::IndexFormat::Uint16)),
                    range: 0..36,
                    base_vertex: 0,
                    post: None,
                    caster: None,
                });
            }
        }
        // Blend reflections and emission (pass 2 shares pass 0's vertex program).
        if let Some((p, l, plan)) = fullscreen(
            self,
            "Hidden/Owlcat/DeferredReflections#2.vs[]",
            "Hidden/Owlcat/DeferredReflections#2.fs[]",
            &[REFLECTIONS],
            &[],
            &Consts::default(),
            &mut arena,
        ) {
            planned.push(Planned {
                pass: 1,
                pipeline: p,
                lkey: l,
                plan,
                vbufs: vec![],
                index: None,
                range: 0..3,
                base_vertex: 0,
                post: None,
                caster: None,
            });
        }
        // Deferred lighting: the `SHADOWS_SOFT` variant (`ShadowCasterPass.BindShadowData`
        // enables it for `ShadowQuality.All`), whose bindings differ from the no-shadow one.
        let dl = if self.shadows_on { "SHADOWS_SOFT" } else { "" };
        if let Some((p, l, plan)) = fullscreen(
            self,
            &format!("Hidden/Owlcat/DeferredLighting#0.vs[{dl}]"),
            &format!("Hidden/Owlcat/DeferredLighting#0.fs[{dl}]"),
            &[GBUFFER[2]],
            &[],
            &Consts::default(),
            &mut arena,
        ) {
            let plan = self.patch_light_buffers(&l, plan);
            planned.push(Planned {
                pass: 2,
                pipeline: p,
                lkey: l,
                plan,
                vbufs: vec![],
                index: None,
                range: 0..3,
                base_vertex: 0,
                post: None,
                caster: None,
            });
        }
        // Fog.
        if self.scene_settings.fog {
            let kw = match self.scene_settings.fog_mode {
                1 => "DEFERRED_ON FOG_LINEAR",
                _ => "DEFERRED_ON",
            };
            if let Some((p, l, plan)) = fullscreen(
                self,
                &format!("Hidden/Owlcat/Fog#0.vs[{kw}]"),
                &format!("Hidden/Owlcat/Fog#0.fs[{kw}]"),
                &[GBUFFER[2]],
                &[],
                &Consts::default(),
                &mut arena,
            ) {
                planned.push(Planned {
                    pass: 3,
                    pipeline: p,
                    lkey: l,
                    plan,
                    vbufs: vec![],
                    index: None,
                    range: 0..3,
                    base_vertex: 0,
                    post: None,
                    caster: None,
                });
            }
        }
        // Post-processing, as `PostProcessPass.Render` runs it for this profile: Stop NaN
        // (camera colour → `_TempTarget`; its fragment also decodes the sRGB-encoded
        // values lighting and fog wrote back to linear), the BloomEnhanced pyramid on
        // that linear source, the grading LUT (LutBuilderHdr), then Uber (bloom, vignette,
        // HDR grading; encodes to sRGB) into `_AfterPostProcessColorRT`. All are Unity
        // blits: a quad with their own view-projection.
        if let Some(lc) = meta.lighting.as_ref().filter(|l| !l.volumes.is_empty()) {
            let p =
                post::Post::from_volumes(&lc.volumes, self.width, self.height, self.gamma_setting);
            self.post_components = p.components.clone();
            let kw = match p.tonemap {
                1 => "_TONEMAP_NEUTRAL",
                2 => "_TONEMAP_ACES",
                _ => "",
            };
            let quad = quad_mesh(device);
            let mut blit_consts = Consts::default();
            blit_consts.m("unity_ObjectToWorld", Mat4::IDENTITY);
            blit_consts.m("unity_WorldToObject", Mat4::IDENTITY);
            blit_consts.m("unity_MatrixVP", blit_vp());
            let clamp = self.gpu.defaults.sampler_clamp.clone();
            let texel = |w: u32, h: u32| [1.0 / w as f32, 1.0 / h as f32, w as f32, h as f32];
            let curves: Vec<(&str, wgpu::TextureView, Option<wgpu::Sampler>)> = [
                ("_CurveMaster", true),
                ("_CurveRed", true),
                ("_CurveGreen", true),
                ("_CurveBlue", true),
                ("_CurveHueVsHue", false),
                ("_CurveHueVsSat", false),
                ("_CurveSatVsSat", false),
                ("_CurveLumVsSat", false),
            ]
            .into_iter()
            .map(|(n, id)| (n, curve_texture(device, queue, id), None))
            .collect();
            // (label, vertex role, fragment role, textures, constants, target, is_lut)
            type Step<'t> = (
                String,
                String,
                String,
                Vec<(&'t str, wgpu::TextureView, Option<wgpu::Sampler>)>,
                Consts,
                wgpu::TextureView,
                bool,
            );
            let role = |shader: &str, pass: u32, kw: &str| {
                (
                    format!("Hidden/Owlcat Render Pipeline/{shader}#{pass}.vs[{kw}]"),
                    format!("Hidden/Owlcat Render Pipeline/{shader}#{pass}.fs[{kw}]"),
                )
            };
            let mut steps: Vec<Step<'_>> = Vec::new();
            let (v, f) = role("Stop NaN", 0, "");
            steps.push((
                "Stop NaN".into(),
                v,
                f,
                vec![("_MainTex", self.targets.gbuffer[2].clone(), Some(clamp.clone()))],
                Consts::default(),
                self.temp_target.clone(),
                false,
            ));
            let bloom = p.bloom.clone().filter(|_| std::env::var_os("WR_NO_BLOOM").is_none());
            let mut uber_kw = "_HDR_GRADING".to_owned();
            let mut uber_tex = vec![
                ("_BlitTex", self.temp_target.clone(), Some(clamp.clone())),
                ("_InternalLut", self.lut_view.clone(), Some(clamp.clone())),
            ];
            let mut uber_consts = p.uber.clone();
            if let Some(b) = &bloom {
                // Prefilter at half size, `iterations` downsamples (pass 1, then 2), then
                // upsamples back (pass 3) each blended with the matching down level.
                let pre = rt(device, b.half.0, b.half.1, "_BloomEnhancedPrefilteredRt");
                let levels = b.levels();
                let down: Vec<wgpu::TextureView> = levels
                    .iter()
                    .enumerate()
                    .map(|(i, (w, h))| rt(device, *w, *h, &format!("_BloomMipDown{i}")))
                    .collect();
                let up: Vec<wgpu::TextureView> = levels
                    .iter()
                    .enumerate()
                    .map(|(i, (w, h))| rt(device, *w, *h, &format!("_BloomMipUp{i}")))
                    .collect();
                let mat = |src: (u32, u32)| {
                    let mut c = b.material.clone();
                    c.f("_MainTex_TexelSize", &texel(src.0, src.1));
                    c
                };
                let (v, f) =
                    role("BloomEnhanced", 0, if b.anti_flicker { "ANTI_FLICKER" } else { "" });
                steps.push((
                    "Bloom Prefilter".into(),
                    v,
                    f,
                    vec![("_MainTex", self.temp_target.clone(), Some(clamp.clone()))],
                    mat((self.width, self.height)),
                    pre.clone(),
                    false,
                ));
                let mut src = (pre, b.half);
                for (i, (dst, size)) in down.iter().zip(&levels).enumerate() {
                    let (v, f) = role("BloomEnhanced", if i == 0 { 1 } else { 2 }, "");
                    steps.push((
                        format!("Bloom Down {i}"),
                        v,
                        f,
                        vec![("_MainTex", src.0.clone(), Some(clamp.clone()))],
                        mat(src.1),
                        dst.clone(),
                        false,
                    ));
                    src = (dst.clone(), *size);
                }
                for i in (0..levels.len().saturating_sub(1)).rev() {
                    let (v, f) = role("BloomEnhanced", 3, "");
                    steps.push((
                        format!("Bloom Up {i}"),
                        v,
                        f,
                        vec![
                            ("_MainTex", src.0.clone(), Some(clamp.clone())),
                            ("_BaseTex", down[i].clone(), Some(clamp.clone())),
                        ],
                        mat(src.1),
                        up[i].clone(),
                        false,
                    ));
                    src = (up[i].clone(), levels[i]);
                }
                // With one level the pyramid has no upsample and Uber reads MipUp0 as
                // the original does (left empty there too).
                let top = levels.first().copied().unwrap_or((1, 1));
                uber_tex.push(("_Bloom_Texture", up[0].clone(), Some(clamp.clone())));
                uber_consts.f("_Bloom_Texture_TexelSize", &texel(top.0, top.1));
                // Lens dirt (`_BLOOM_HQ_DIRT`) needs the dirt texture, which the pack doesn't
                // carry yet; the cave profile has none (intensity 0).
                if b.dirt > 0.0 {
                    self.stats.skipped.push("post: bloom lens dirt not imported".into());
                }
                uber_kw = "_BLOOM_HQ _HDR_GRADING".to_owned();
            }
            let (v, f) = role("LutBuilderHdr", 0, kw);
            steps.push((
                "Color Grading LUT".into(),
                v,
                f,
                curves,
                p.lut.clone(),
                self.lut_view.clone(),
                true,
            ));
            let (v, f) = role("UberPost", 0, &uber_kw);
            steps.push((
                "Uber Post".into(),
                v,
                f,
                uber_tex,
                uber_consts,
                self.after_post.clone(),
                false,
            ));
            self.post_expected = steps.len();
            for (label, vsr, fsr, extra, fg, target, is_lut) in steps {
                let (Some(v), Some(f)) = (find(&vsr), find(&fsr)) else {
                    self.stats.skipped.push(format!("post: no program {fsr}"));
                    continue;
                };
                let (vs, fs) =
                    (&meta.programs[v.program as usize], &meta.programs[f.program as usize]);
                let state =
                    resolve_state(f.state.as_ref().unwrap_or(&PassState::default()), None, 1);
                let loc = |sem: &str| {
                    vs.vertex_inputs.iter().find(|i| i.semantic == sem).map_or(0, |i| i.attribute)
                };
                let vl = VertexLayout {
                    streams: vec![(
                        0,
                        20,
                        vec![
                            (wgpu::VertexFormat::Float32x3, 0, loc("POSITION0")),
                            (wgpu::VertexFormat::Float32x2, 12, loc("TEXCOORD0")),
                        ],
                    )],
                    defaults: vec![],
                };
                let pipeline = self.render_pipeline(
                    device,
                    vs,
                    fs,
                    &state,
                    Some(&vl),
                    &[wgpu::TextureFormat::Rgba16Float],
                    None,
                );
                let lkey = self.layout(device, &[vs, fs]);
                let ctx =
                    Bind { material: None, lightmap: None, extra_tex: &extra, local_cbs: true };
                let entries = self.layouts[&lkey].2.clone();
                let mut plan = Vec::new();
                for (stage, b) in &entries {
                    let prog = if *stage == Stage::Vertex { vs } else { fs };
                    plan.push(self.plan(
                        device,
                        *stage,
                        b,
                        prog,
                        &ctx,
                        &[&fg, &blit_consts, &globals_now],
                        &mut arena,
                        None,
                    ));
                }
                planned.push(Planned {
                    pass: 4,
                    pipeline,
                    lkey,
                    plan,
                    vbufs: vec![quad.0.clone()],
                    index: Some((quad.1.clone(), wgpu::IndexFormat::Uint16)),
                    range: 0..6,
                    base_vertex: 0,
                    post: Some((label, target, is_lut)),
                    caster: None,
                });
            }
        }
        // Light culling compute.
        if let Some(c) = find("LightCulling.BuildLightTiles[]") {
            let prog = &meta.programs[c.program as usize];
            let lkey = self.layout(device, &[prog]);
            let module = self.gpu.module_for(device, &pack, prog);
            let cp = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("LightCulling"),
                layout: Some(&self.layouts[&lkey].1),
                module: &module,
                entry_point: Some(&prog.entry),
                compilation_options: Default::default(),
                cache: None,
            });
            let entries = self.layouts[&lkey].2.clone();
            let ctx = Bind { material: None, lightmap: None, extra_tex: &[], local_cbs: false };
            let globals = self.consts.clone();
            let mut plan = Vec::new();
            for (stage, b) in &entries {
                plan.push(self.plan(device, *stage, b, prog, &ctx, &[&globals], &mut arena, None));
            }
            let plan = self.patch_light_buffers(&lkey, plan);
            if let Some(Res::Invalid(why)) = plan.iter().find(|r| matches!(r, Res::Invalid(_))) {
                anyhow::bail!("light culling: {why}");
            }
            let arena_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("tmp"),
                size: 256,
                usage: wgpu::BufferUsages::UNIFORM,
                mapped_at_creation: false,
            });
            let bg = self.materialize(device, &lkey, &plan, &arena_buf);
            self.culling = Some((cp, bg, prog.threads[0].max(1)));
        }
        self.stats.pipelines_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        arena.resize(arena.len().div_ceil(256).max(1) * 256, 0);
        let arena_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("uniform-arena"),
            contents: &arena,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        for p in planned {
            if let Some(Res::Invalid(why)) = p.plan.iter().find(|r| matches!(r, Res::Invalid(_))) {
                self.stats.skipped.push(format!("draw skipped: {why}"));
                continue;
            }
            let bind = self.materialize(device, &p.lkey, &p.plan, &arena_buf);
            if let (Some((ci, clip, pipelines)), Some(index)) = (p.caster, p.index.clone()) {
                self.casters[ci].binds[usize::from(clip)] = Some(CasterBind {
                    pipelines,
                    bind,
                    vbufs: p.vbufs,
                    index,
                    range: p.range,
                    base_vertex: p.base_vertex,
                });
                continue;
            }
            let d = Draw {
                pipeline: p.pipeline,
                bind,
                vbufs: p.vbufs,
                index: p.index,
                range: p.range,
                base_vertex: p.base_vertex,
            };
            match (p.pass, p.post) {
                (0, _) => self.gbuffer_draws.push(d),
                (1, _) => self.reflection_draws.push(d),
                (2, _) => self.lighting_draws.push(d),
                (3, _) => self.fog_draws.push(d),
                (_, Some((label, target, lut))) => {
                    self.post_steps.push(PostStep { label, target, lut, draw: d });
                }
                (_, None) => {}
            }
        }
        self.post_on = self.post_expected > 0
            && self.post_steps.len() == self.post_expected
            && std::env::var_os("WR_NO_POST").is_none();
        let shown =
            if self.post_on { self.after_post.clone() } else { self.targets.gbuffer[2].clone() };
        self.final_bind = Some(self.blit_bind(device, &shown));
        self.stats.bind_groups_ms = t.elapsed().as_secs_f64() * 1e3;
        self.stats.pipelines = self.pipelines.len() + usize::from(self.culling.is_some());
        self.stats.shader_modules = self.gpu.modules.len();
        self.stats.draws = self.gbuffer_draws.len();
        if std::env::var_os("WR_NO_BUNDLES").is_none() {
            let formats: Vec<Option<wgpu::TextureFormat>> =
                GBUFFER.iter().map(|f| Some(*f)).collect();
            let mut be =
                device.create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
                    label: Some("GBuffer bundle"),
                    color_formats: &formats,
                    depth_stencil: Some(wgpu::RenderBundleDepthStencil {
                        format: DEPTH,
                        depth_read_only: false,
                        stencil_read_only: true,
                    }),
                    sample_count: 1,
                    multiview: None,
                });
            self.draw_all(&mut be, &self.gbuffer_draws);
            self.gbuffer_bundle =
                Some(be.finish(&wgpu::RenderBundleDescriptor { label: Some("GBuffer bundle") }));
        }
        let _ = queue;
        Ok(())
    }

    /// Light buffers by name in a planned bind group (they are global, not per draw).
    fn patch_light_buffers(&self, lkey: &str, mut plan: Vec<Res>) -> Vec<Res> {
        for (i, (_, b)) in self.layouts[lkey].2.iter().enumerate() {
            let buf = match b.name.as_str() {
                "LightDataCB" => &self.light_data,
                "LightVolumeDataCB" => &self.light_volume,
                "ZBinsCB" => &self.zbins,
                "_LightTilesBuffer" | "_LightTilesBufferUAV" => &self.tiles,
                "_ShadowDataBuffer" => &self.shadow.data,
                "_ShadowMatricesBuffer" => &self.shadow.matrices,
                _ => continue,
            };
            plan[i] = Res::Buffer(buf.clone());
        }
        plan
    }

    /// Constant members no source provided (diagnostics).
    #[must_use]
    pub fn missing_constants(&self) -> Vec<String> {
        self.missing.iter().cloned().collect()
    }

    /// Records one frame into `enc`, ending with the blit into `target` (a non-sRGB
    /// view of the display image, which already holds sRGB-encoded values).
    pub fn encode(&self, enc: &mut wgpu::CommandEncoder, target: &wgpu::TextureView) {
        self.encode_shadows(enc);
        // Light culling.
        if let Some((cp, bg, threads)) = &self.culling {
            let tiles = self.width.div_ceil(TILE) * self.height.div_ceil(TILE);
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Light Culling"),
                timestamp_writes: None,
            });
            pass.set_pipeline(cp);
            pass.set_bind_group(0, bg, &[]);
            let n = (self.lights_visible as u32).max(1);
            pass.dispatch_workgroups(tiles.div_ceil(*threads), 1, n.div_ceil(32).max(1));
        }
        // G-buffer.
        {
            let atts: Vec<Option<wgpu::RenderPassColorAttachment<'_>>> = self
                .targets
                .gbuffer
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
                label: Some("GBuffer Pass"),
                color_attachments: &atts,
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            match &self.gbuffer_bundle {
                Some(b) => pass.execute_bundles(std::iter::once(b)),
                None => self.draw_all(&mut pass, &self.gbuffer_draws),
            }
        }
        self.pass(
            enc,
            "Deferred Reflections",
            &self.targets.reflections,
            &self.reflection_draws,
            true,
        );
        // Owlcat's deferred-lighting and fog fragments write sRGB-encoded values into the
        // (float) camera colour; post's first step, Stop NaN, decodes them back to linear.
        self.pass(enc, "Deferred Lighting", &self.targets.gbuffer[2], &self.lighting_draws, false);
        if std::env::var_os("WR_NO_FOG").is_none() {
            self.pass(enc, "Fog", &self.targets.gbuffer[2], &self.fog_draws, false);
        }
        // Post: Stop NaN, bloom, grading LUT (only when its inputs changed), Uber.
        if self.post_on {
            for st in &self.post_steps {
                if st.lut && !self.lut_dirty.swap(false, Ordering::Relaxed) {
                    continue;
                }
                self.pass(enc, &st.label, &st.target, std::slice::from_ref(&st.draw), true);
            }
        }
        // Final blit: flip Y (Unity renders into textures upside down).
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Final Blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.blit.0);
        pass.set_bind_group(0, self.final_bind.as_ref(), &[]);
        pass.draw(0..3, 0..1);
    }

    /// `ShadowCasterPass.Execute`: clear the atlas once, then per entry and face set the
    /// entry's viewport and draw its casters with the face's constants.
    fn encode_shadows(&self, enc: &mut wgpu::CommandEncoder) {
        if !self.shadows_on {
            return;
        }
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Draw Shadows"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.shadow.view,
                // Reversed Z: 0 is the far plane (Unity's `ClearRenderTarget(depth 1)`
                // flipped; the native clear value needs a capture to confirm).
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        for f in &self.shadow_faces {
            let v = f.viewport;
            pass.set_viewport(v.x, v.y, v.w, v.h, 0.0, 1.0);
            let mut last = usize::MAX;
            for &ci in &f.draws {
                let Some(cb) = &self.casters[ci as usize].binds[usize::from(f.clip)] else {
                    continue;
                };
                let Some(&(_, pi)) = cb
                    .pipelines
                    .iter()
                    .find(|(b, _)| *b == f.slope_bias)
                    .or_else(|| cb.pipelines.first())
                else {
                    continue;
                };
                if pi != last {
                    pass.set_pipeline(&self.pipelines[pi]);
                    last = pi;
                }
                pass.set_bind_group(0, Some(&cb.bind), &[f.offset]);
                for (i, b) in cb.vbufs.iter().enumerate() {
                    pass.set_vertex_buffer(i as u32, Some(b.slice(..)));
                }
                pass.set_index_buffer(cb.index.0.slice(..), cb.index.1);
                pass.draw_indexed(cb.range.clone(), cb.base_vertex, 0..1);
            }
        }
    }

    /// The shadow atlas as 16-bit values (reversed Z: larger is nearer the light), for
    /// diagnostics.
    pub fn read_atlas(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> Result<(u32, Vec<u16>)> {
        let n = self.shadow.settings.atlas_size;
        let row = (n * 2).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("atlas readback"),
            size: u64::from(row * n),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.shadow.atlas,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::DepthOnly,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d { width: n, height: n, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::PollType::wait_indefinitely()).context("poll")?;
        let data =
            buf.slice(..).get_mapped_range().map_err(|e| anyhow::anyhow!("map: {e:?}"))?.to_vec();
        buf.unmap();
        let mut out = Vec::with_capacity((n * n) as usize);
        for y in 0..n {
            let s = (y * row) as usize;
            out.extend(
                data[s..s + (n * 2) as usize]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_le_bytes(*c)),
            );
        }
        Ok((n, out))
    }

    fn blit_bind(&self, device: &wgpu::Device, src: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &self.blit.1,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.blit.2),
                },
            ],
        })
    }

    fn pass(
        &self,
        enc: &mut wgpu::CommandEncoder,
        label: &str,
        view: &wgpu::TextureView,
        draws: &[Draw],
        clear: bool,
    ) {
        if draws.is_empty() && !clear {
            return;
        }
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.draw_all(&mut pass, draws);
    }

    fn draw_all<'a>(&'a self, pass: &mut impl wgpu::util::RenderEncoder<'a>, draws: &'a [Draw]) {
        let mut last = usize::MAX;
        for d in draws {
            if d.pipeline != last {
                pass.set_pipeline(&self.pipelines[d.pipeline]);
                last = d.pipeline;
            }
            pass.set_bind_group(0, Some(&d.bind), &[]);
            for (i, b) in d.vbufs.iter().enumerate() {
                pass.set_vertex_buffer(i as u32, Some(b.slice(..)));
            }
            match &d.index {
                Some((b, f)) => {
                    pass.set_index_buffer(b.slice(..), *f);
                    pass.draw_indexed(d.range.clone(), d.base_vertex, 0..1);
                }
                None => pass.draw(d.range.clone(), 0..1),
            }
        }
    }
}

/// Whether a light's shadow draw includes a caster: the light's culling mask holds the
/// caster's layer, and (`useRenderingLayerMaskTest`, `ShadowCasterPass.cs:87`) the rendering
/// layer masks intersect.
fn light_reaches(l: &wr_assets::Light, layer: u32, rendering_layer_mask: u32) -> bool {
    l.culling_mask & (1u32 << layer.min(31)) != 0
        && l.rendering_layer_mask & rendering_layer_mask != 0
}

/// The caster pass's state for a renderer: `ShadowCastingMode.TwoSided` disables face
/// culling for shadow rendering only.
fn caster_state(mut s: RenderState, cast: u8) -> RenderState {
    if cast == 2 {
        s.cull = 0;
    }
    s
}

/// The world-space bounds of a local (min, max) box under `m`.
fn world_bounds(b: ([f32; 3], [f32; 3]), m: Mat4) -> (Vec3, Vec3) {
    let (lo, hi) = (Vec3::from(b.0), Vec3::from(b.1));
    let (mut wl, mut wh) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for i in 0..8 {
        let c = Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        );
        let w = m.transform_point3(c);
        wl = wl.min(w);
        wh = wh.max(w);
    }
    (wl, wh)
}

/// A unit cube centred on the origin (`RenderingUtils.CubeMesh`).
fn cube_mesh(device: &wgpu::Device) -> (wgpu::Buffer, wgpu::Buffer) {
    let v: [[f32; 3]; 8] = [
        [-0.5, -0.5, -0.5],
        [0.5, -0.5, -0.5],
        [0.5, 0.5, -0.5],
        [-0.5, 0.5, -0.5],
        [-0.5, -0.5, 0.5],
        [0.5, -0.5, 0.5],
        [0.5, 0.5, 0.5],
        [-0.5, 0.5, 0.5],
    ];
    let i: [u16; 36] = [
        0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0, 7, 3,
        1, 2, 6, 1, 6, 5,
    ];
    let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("cube"),
        contents: &v.iter().flatten().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>(),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("cube"),
        contents: &i.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
        usage: wgpu::BufferUsages::INDEX,
    });
    (vb, ib)
}

const BLIT_WGSL: &str = r"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
struct V { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vs(@builtin(vertex_index) i: u32) -> V {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: V;
    o.pos = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    // Unity's intermediate targets hold the image bottom-up: flip on the way out.
    o.uv = vec2<f32>(uv.x, 1.0 - uv.y);
    return o;
}
@fragment fn fs(v: V) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(src, smp, v.uv, 0.0);
    return vec4<f32>(c.rgb, 1.0);
}
";

/// The final blit (ours, not Owlcat's `FinalBlit`): copies the after-post colour (or, with
/// post off, the camera colour) to the display target. Both hold **sRGB-encoded** values
/// (Uber, like deferred lighting and fog, ends in a linear→sRGB conversion), so the blit
/// copies them through a non-sRGB view of the display image: no second encoding. The
/// original's `FinalBlit` decodes them and lets the sRGB drawable re-encode, which gives
/// the same bytes up to rounding.
fn blit_pipeline(
    device: &wgpu::Device,
) -> (wgpu::RenderPipeline, wgpu::BindGroupLayout, wgpu::Sampler) {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("blit"),
        source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("blit"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                count: None,
            },
        ],
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("blit"),
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let p = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("blit"),
        layout: Some(&pl),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    });
    let s = device
        .create_sampler(&wgpu::SamplerDescriptor { label: Some("blit"), ..Default::default() });
    (p, bgl, s)
}

/// Reads the display texture back as tightly packed RGBA8 (sRGB-encoded) rows.
pub fn read_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &wgpu::Texture,
    w: u32,
    h: u32,
) -> Result<Vec<u8>> {
    let row = (w * 4).div_ceil(256) * 256;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).context("poll")?;
    let data =
        buf.slice(..).get_mapped_range().map_err(|e| anyhow::anyhow!("map: {e:?}"))?.to_vec();
    buf.unmap();
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        let s = (y * row) as usize;
        out.extend_from_slice(&data[s..s + (w * 4) as usize]);
    }
    Ok(out)
}

/// An RGBA16F render target view.
fn rt(device: &wgpu::Device, w: u32, h: u32, label: &str) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

/// A 128×1 curve texture (`TextureCurve.GetTexture`).
fn curve_texture(device: &wgpu::Device, queue: &wgpu::Queue, identity: bool) -> wgpu::TextureView {
    let v: Vec<u8> = post::curve_texels(identity)
        .into_iter()
        .flat_map(|x| {
            let h = f16_bits(x).to_le_bytes();
            [h[0], h[1], h[0], h[1], h[0], h[1], 0, 0x3c]
        })
        .collect();
    device
        .create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some("curve"),
                size: wgpu::Extent3d { width: 128, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &v,
        )
        .create_view(&Default::default())
}

/// `f32` → IEEE half bits (round to nearest even; finite inputs).
fn f16_bits(x: f32) -> u16 {
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xFF) as i32 - 127 + 15;
    let mant = b & 0x7F_FFFF;
    if exp <= 0 {
        if exp < -10 {
            return sign;
        }
        let m = (mant | 0x80_0000) >> (1 - exp);
        let half = (m + 0x1000 + ((m >> 13) & 1) - 1) >> 13;
        return sign | half as u16;
    }
    if exp >= 31 {
        return sign | 0x7C00;
    }
    let r = (mant + 0xFFF + ((mant >> 13) & 1)) >> 13;
    let (exp, r) = if r == 0x400 { (exp + 1, 0) } else { (exp, r) };
    sign | ((exp as u16) << 10) | r as u16
}

/// Unity's blit quad: positions (0..1, 0..1, 0) with UVs equal to them.
fn quad_mesh(device: &wgpu::Device) -> (wgpu::Buffer, wgpu::Buffer) {
    let v: [[f32; 5]; 4] = [
        [0.0, 0.0, 0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0, 1.0, 0.0],
        [0.0, 1.0, 0.0, 0.0, 1.0],
        [1.0, 1.0, 0.0, 1.0, 1.0],
    ];
    let i: [u16; 6] = [0, 1, 2, 2, 1, 3];
    (
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("blit-quad"),
            contents: &v.iter().flatten().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>(),
            usage: wgpu::BufferUsages::VERTEX,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("blit-quad"),
            contents: &i.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>(),
            usage: wgpu::BufferUsages::INDEX,
        }),
    )
}

/// The blit view-projection: quad (0..1)² to clip space, UV v = 0 on the target's
/// first row (Unity's convention for render textures on Metal).
fn blit_vp() -> Mat4 {
    Mat4::from_cols(
        Vec4::new(2.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, -2.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, 0.0),
        Vec4::new(-1.0, 1.0, 0.5, 1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disjoint_rendering_layers_do_not_cast() {
        let l = wr_assets::Light {
            culling_mask: u32::MAX,
            rendering_layer_mask: 0b01,
            ..Default::default()
        };
        assert!(light_reaches(&l, 3, 0b11));
        assert!(!light_reaches(&l, 3, 0b10));
        let l = wr_assets::Light { culling_mask: 1, rendering_layer_mask: 1, ..Default::default() };
        assert!(!light_reaches(&l, 3, 1));
    }

    #[test]
    fn two_sided_casters_disable_culling() {
        let s = RenderState { cull: 2, ztest: 4, zwrite: true, blend: vec![], slope_bias: 0 };
        assert_eq!(caster_state(s.clone(), 2).cull, 0);
        assert_eq!(caster_state(s.clone(), 1).cull, 2);
    }

    #[test]
    fn half_conversion() {
        assert_eq!(f16_bits(0.0), 0);
        assert_eq!(f16_bits(1.0), 0x3C00);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(-2.0), 0xC000);
        assert_eq!(f16_bits(1.0 / 3.0), 0x3555);
    }

    #[test]
    fn blit_quad_maps_to_clip() {
        let m = blit_vp();
        assert_eq!(m * Vec4::new(0.0, 0.0, 0.0, 1.0), Vec4::new(-1.0, 1.0, 0.5, 1.0));
        assert_eq!(m * Vec4::new(1.0, 1.0, 0.0, 1.0), Vec4::new(1.0, -1.0, 0.5, 1.0));
    }
}
