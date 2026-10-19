//! The pack's metadata model. Names and enums follow Unity's serialized data so the
//! importer can copy values without reinterpreting them; the runtime maps them to wgpu.

use serde::{Deserialize, Serialize};

use crate::BlobRef;

/// Schema string written into every pack.
pub const SCHEMA: &str = "wr-assets/pack/1";

/// A Unity object's identity in the shipped data: serialized file (`CAB-…` or a player
/// file name) and path id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct AssetId {
    pub file: String,
    pub path_id: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PackMeta {
    pub schema: String,
    pub area: String,
    /// Free-form provenance: game version, source bundles, importer version.
    pub source: Vec<(String, String)>,
    pub textures: Vec<Texture>,
    pub meshes: Vec<Mesh>,
    pub programs: Vec<Program>,
    pub shaders: Vec<Shader>,
    pub materials: Vec<Material>,
    pub renderers: Vec<Renderer>,
    pub lights: Vec<Light>,
    pub lightmaps: Vec<Lightmap>,
    pub scenes: Vec<SceneSettings>,
    pub reflection_probes: Vec<ReflectionProbe>,
    /// The light scene's `LightController` setup (what it applies at load).
    pub lighting: Option<LightingSetup>,
    /// Programs of the render pipeline itself (deferred lighting, light culling, …).
    pub pipeline_programs: Vec<PipelineProgram>,
    /// Bone hierarchies of the animated units.
    pub rigs: Vec<wr_anim::Rig>,
    /// Animation clips (shared by the units that play them).
    pub clips: Vec<wr_anim::Clip>,
    /// Animator controllers (monster locomotion).
    pub controllers: Vec<wr_anim::Controller>,
    /// One per animated unit: its rig, clips and locomotion. Skinned renderers refer to it.
    pub animations: Vec<wr_anim::loco::UnitAnimation>,
    /// What the importer skipped and why (class or shader → count), for the report.
    pub skipped: Vec<(String, u32)>,
}

// ---------------------------------------------------------------------------------------
// Textures

/// Pixel formats as stored in the pack. BC formats carry the shipped blocks unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TexFormat {
    #[default]
    Rgba8,
    Bgra8,
    R8,
    Rg8,
    Rgba16Float,
    R16Float,
    Rgba32Float,
    Bc1,
    Bc3,
    Bc4,
    Bc5,
    Bc6hUfloat,
    Bc7,
}

impl TexFormat {
    /// Bytes per 4×4 block for block formats, or per pixel otherwise (`(bytes, block)`).
    #[must_use]
    pub fn unit(self) -> (u32, bool) {
        match self {
            Self::Bc1 | Self::Bc4 => (8, true),
            Self::Bc3 | Self::Bc5 | Self::Bc6hUfloat | Self::Bc7 => (16, true),
            Self::R8 => (1, false),
            Self::Rg8 | Self::R16Float => (2, false),
            Self::Rgba8 | Self::Bgra8 => (4, false),
            Self::Rgba16Float => (8, false),
            Self::Rgba32Float => (16, false),
        }
    }

    /// Bytes of one mip level of `w × h`.
    #[must_use]
    pub fn level_size(self, w: u32, h: u32) -> u64 {
        let (unit, block) = self.unit();
        if block {
            u64::from(w.div_ceil(4)) * u64::from(h.div_ceil(4)) * u64::from(unit)
        } else {
            u64::from(w) * u64::from(h) * u64::from(unit)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TexDimension {
    #[default]
    D2,
    Cube,
    D2Array,
    D3,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Texture {
    pub id: AssetId,
    pub name: String,
    pub dimension: TexDimension,
    pub format: TexFormat,
    pub srgb: bool,
    pub width: u32,
    pub height: u32,
    /// Array layers, cube faces (6) or depth.
    pub layers: u32,
    pub mips: u32,
    /// All levels, Unity's order: for each layer/face, mip 0..n (cubes: face-major).
    pub data: BlobRef,
    /// `TextureWrapMode` per axis (0 repeat, 1 clamp, 2 mirror, 3 mirror once).
    pub wrap: [u8; 3],
    /// `FilterMode` (0 point, 1 bilinear, 2 trilinear).
    pub filter: u8,
    pub aniso: u8,
    pub mip_bias: f32,
    /// Unity `TextureFormat` as shipped.
    pub unity_format: i32,
    /// Set when the payload isn't the shipped bytes (e.g. crunch decoded to RGBA8).
    pub transcoded: Option<String>,
}

// ---------------------------------------------------------------------------------------
// Meshes

/// One of Unity's 14 vertex channels (2019+ order: position, normal, tangent, colour,
/// uv0–7, blend weights, blend indices).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Channel {
    pub stream: u8,
    pub offset: u8,
    /// Unity `VertexFormat` (0 float32, 1 float16, 2 unorm8, 3 snorm8, 4 unorm16,
    /// 5 snorm16, 6 uint8, 7 sint8, 8 uint16, 9 sint16, 10 uint32, 11 sint32).
    pub format: u8,
    pub dimension: u8,
}

pub const CHANNEL_NAMES: [&str; 14] = [
    "POSITION",
    "NORMAL",
    "TANGENT",
    "COLOR",
    "TEXCOORD0",
    "TEXCOORD1",
    "TEXCOORD2",
    "TEXCOORD3",
    "TEXCOORD4",
    "TEXCOORD5",
    "TEXCOORD6",
    "TEXCOORD7",
    "BLENDWEIGHTS",
    "BLENDINDICES",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VertexStream {
    pub data: BlobRef,
    pub stride: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubMesh {
    /// First index (in indices, not bytes).
    pub first_index: u32,
    pub index_count: u32,
    pub base_vertex: i32,
    /// Unity `MeshTopology` (0 triangles, 2 lines, 3 line strip, 4 points; 1 = quads).
    pub topology: u8,
    /// Bounds of the vertices this submesh's indices reference, as (min, max) in mesh
    /// space (world space for a static-batched combined mesh). `None` when the position
    /// format isn't decoded; callers fall back to the whole mesh's `aabb`.
    pub bounds: Option<([f32; 3], [f32; 3])>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Mesh {
    pub id: AssetId,
    pub name: String,
    pub vertex_count: u32,
    pub channels: Vec<Option<Channel>>,
    pub streams: Vec<VertexStream>,
    pub index_u32: bool,
    pub indices: BlobRef,
    pub submeshes: Vec<SubMesh>,
    /// Local AABB: centre and extent.
    pub aabb: ([f32; 3], [f32; 3]),
    /// A skinned mesh in its bind pose: [`SKIN_STRIDE`] bytes per vertex, four bone weights
    /// (float32, summing to one) then four bone indices (uint32) into the renderer's
    /// [`SkinBinding`]. `channels` still describe the position, normal and tangent the skinning
    /// replaces; its other channels are used as they are.
    pub skin: Option<BlobRef>,
}

/// Bytes per vertex of [`Mesh::skin`].
pub const SKIN_STRIDE: u64 = 32;

// ---------------------------------------------------------------------------------------
// Shaders

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum Stage {
    #[default]
    Vertex,
    Fragment,
    Compute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum TexKind {
    #[default]
    D2,
    D2Array,
    D3,
    Cube,
    CubeArray,
    Depth2,
    Depth2Array,
}

/// What a shader binds at a Metal argument-table slot.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Resource {
    /// `constant T& name [[buffer(n)]]`; `ty` names a [`Program::cbuffers`] layout.
    Uniform {
        ty: String,
    },
    /// `device T* name [[buffer(n)]]`.
    Storage {
        ty: String,
        read_only: bool,
    },
    Texture {
        kind: TexKind,
        sample_type: String,
        write: bool,
    },
    Sampler,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Binding {
    pub slot: u32,
    pub name: String,
    pub res: Resource,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VertexInput {
    pub attribute: u32,
    /// MSL type, e.g. `float4`.
    pub ty: String,
    /// HLSLcc semantic, e.g. `POSITION0`, `TEXCOORD1`.
    pub semantic: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub ty: String,
    pub count: u32,
    pub offset: u32,
    pub size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct CBufferLayout {
    pub ty: String,
    pub size: u32,
    pub members: Vec<Member>,
}

/// One compiled shader stage: a Unity subprogram as shipped (MSL), its Metal library
/// compiled offline, and its resource interface.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Program {
    pub stage: Stage,
    pub shader: String,
    pub pass: String,
    pub keywords: Vec<String>,
    pub entry: String,
    pub msl: BlobRef,
    pub metallib: Option<BlobRef>,
    /// FNV-1a of the MSL text (the pipeline cache key).
    pub msl_hash: u64,
    /// Sorted: buffers, samplers, textures, each by slot.
    pub bindings: Vec<Binding>,
    pub vertex_inputs: Vec<VertexInput>,
    pub color_outputs: u32,
    pub cbuffers: Vec<CBufferLayout>,
    /// Compute: threads per group.
    pub threads: [u32; 3],
}

/// A render-state value: a constant, or bound to a material property (`[_CullMode]`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct StateValue {
    pub value: f32,
    pub property: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct BlendState {
    pub src: StateValue,
    pub dst: StateValue,
    pub src_alpha: StateValue,
    pub dst_alpha: StateValue,
    pub op: StateValue,
    pub op_alpha: StateValue,
    pub color_mask: StateValue,
}

/// Unity's serialized pass state (`SerializedShaderState`), values as Unity enums.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct PassState {
    pub blend: Vec<BlendState>,
    pub separate_blend: bool,
    pub z_test: StateValue,
    pub z_write: StateValue,
    pub cull: StateValue,
    pub offset_factor: StateValue,
    pub offset_units: StateValue,
    pub alpha_to_mask: StateValue,
    pub stencil_ref: StateValue,
    pub stencil_read_mask: StateValue,
    pub stencil_write_mask: StateValue,
    /// Front (or both) faces: pass, fail, z-fail, compare.
    pub stencil_front: [StateValue; 4],
    pub stencil_back: [StateValue; 4],
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShaderPass {
    pub name: String,
    pub tags: Vec<(String, String)>,
    pub state: PassState,
}

impl ShaderPass {
    #[must_use]
    pub fn light_mode(&self) -> &str {
        self.tags
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("LightMode"))
            .map_or("", |(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ShaderProperty {
    pub name: String,
    /// Unity `SerializedPropertyType` (0 colour, 1 vector, 2 float, 3 range, 4 texture).
    pub kind: i32,
    pub default_value: [f32; 4],
    /// Textures: the default (`white`, `black`, `gray`, `bump`, …).
    pub default_texture: Option<String>,
    /// `SerializedPropertyFlag` bits (16 HDR, 32 Gamma, …): colours are converted to
    /// linear unless Gamma is set.
    pub flags: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Shader {
    pub id: AssetId,
    pub name: String,
    pub passes: Vec<ShaderPass>,
    pub properties: Vec<ShaderProperty>,
}

// ---------------------------------------------------------------------------------------
// Materials

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TexProp {
    pub name: String,
    pub texture: Option<u32>,
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}

/// The programs a material uses for one pass under one renderer configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MaterialVariant {
    pub pass: String,
    /// Renderer-side keywords this variant was selected with (e.g. `LIGHTMAP_ON`).
    pub renderer_keywords: Vec<String>,
    pub vertex: u32,
    pub fragment: u32,
    /// Whether both stages matched the enabled keywords exactly.
    pub exact: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Material {
    pub id: AssetId,
    pub name: String,
    pub shader: u32,
    pub keywords: Vec<String>,
    pub render_queue: i32,
    pub disabled_passes: Vec<String>,
    pub textures: Vec<TexProp>,
    pub floats: Vec<(String, f32)>,
    pub colors: Vec<(String, [f32; 4])>,
    pub variants: Vec<MaterialVariant>,
    /// The `ShadowCaster` pass programs for this material, one per `GEOMETRY_CLIP` state
    /// (`ShadowCasterPass` sets that keyword globally: on for point lights, off for
    /// spots). Empty when the shader has no such pass or the material disables it.
    pub casters: Vec<CasterVariant>,
}

/// A material's `ShadowCaster` programs for one `GEOMETRY_CLIP` state.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CasterVariant {
    pub geometry_clip: bool,
    pub vertex: u32,
    pub fragment: u32,
    /// Whether both stages matched the enabled keywords exactly.
    pub exact: bool,
}

impl Material {
    #[must_use]
    pub fn float(&self, name: &str) -> Option<f32> {
        self.floats.iter().find(|(k, _)| k == name).map(|(_, v)| *v)
    }
}

// ---------------------------------------------------------------------------------------
// Scene objects

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RendererKind {
    #[default]
    Mesh,
    Skinned,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Renderer {
    pub renderer: AssetId,
    pub game_object: AssetId,
    pub scene: String,
    pub path: String,
    pub kind: RendererKind,
    pub enabled: bool,
    pub active: bool,
    pub layer: u32,
    /// `localToWorldMatrix`, column-major.
    pub matrix: [f32; 16],
    pub mesh: u32,
    /// One per submesh slot (`m_Materials`); `None` = missing.
    pub materials: Vec<Option<u32>>,
    /// Scene-local lightmap index (65535 = none) and its scale/offset.
    pub lightmap_index: u16,
    pub lightmap_st: [f32; 4],
    pub cast_shadows: u8,
    pub receive_shadows: bool,
    pub light_probe_usage: u8,
    pub reflection_probe_usage: u8,
    /// Static batching: `(first submesh, count)` into a combined mesh.
    pub static_batch: (u16, u16),
    pub rendering_layer_mask: u32,
    /// Values a script sets through `MaterialPropertyBlock` (e.g. `BakedTerrain`).
    pub property_block: Option<PropertyBlock>,
    /// A skinned renderer of an animated unit: the bones that pose its (bind pose) mesh.
    pub skin: Option<SkinBinding>,
}

/// How a skinned renderer's mesh follows a unit's rig.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkinBinding {
    /// Index into [`PackMeta::animations`].
    pub animation: u32,
    /// The rig bone of each of the mesh's bones (`SkinnedMeshRenderer.bones`).
    pub bones: Vec<u32>,
    /// `Mesh.bindposes`, column-major, one per bone. A vertex moves by
    /// `rig_world[bones[i]] * bind_poses[i]`, in the unit's root space (the renderer's matrix
    /// places that space in the world).
    pub bind_poses: Vec<[f32; 16]>,
}

/// A `MaterialPropertyBlock`: per-renderer overrides of material properties.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PropertyBlock {
    pub floats: Vec<(String, f32)>,
    pub vectors: Vec<(String, Vec<[f32; 4]>)>,
    pub textures: Vec<(String, u32)>,
    /// Which script set it and why.
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OwlcatLight {
    /// `LightFalloffType` (0 inverse squared, 1 legacy).
    pub falloff: i32,
    pub inner_radius: f32,
    pub snap_specular_to_inner_radius: bool,
    /// `UsePipelineSettings`: shadow biases come from the pipeline asset (true) or from
    /// the light's own `shadowBias` / `shadowNormalBias`.
    pub use_pipeline_settings: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Light {
    pub id: AssetId,
    pub game_object: AssetId,
    pub scene: String,
    pub path: String,
    pub enabled: bool,
    pub active: bool,
    /// Unity `LightType` (0 spot, 1 directional, 2 point, 3 area).
    pub kind: i32,
    /// Linear colour as serialized (`m_Color`, gamma-space values) and intensity.
    pub color: [f32; 4],
    pub intensity: f32,
    pub range: f32,
    pub spot_angle: f32,
    pub inner_spot_angle: f32,
    /// `LightShadows` (0 none, 1 hard, 2 soft) and its parameters.
    pub shadows: i32,
    pub shadow_strength: f32,
    pub shadow_bias: f32,
    pub shadow_normal_bias: f32,
    pub shadow_near_plane: f32,
    /// `LightmapBakeType` (4 realtime, 2 baked, 1 mixed) and `MixedLightingMode`.
    pub bake_type: i32,
    pub mixed_mode: i32,
    pub occlusion_mask_channel: i32,
    pub culling_mask: u32,
    /// `Light.renderingLayerMask` (shadow draws test it against the caster's).
    pub rendering_layer_mask: u32,
    pub render_mode: i32,
    pub use_color_temperature: bool,
    pub color_temperature: f32,
    pub matrix: [f32; 16],
    pub owlcat: Option<OwlcatLight>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Lightmap {
    pub scene: String,
    /// Scene-local index (what renderers' `lightmap_index` refers to).
    pub index: u32,
    pub color: Option<u32>,
    pub direction: Option<u32>,
    pub shadowmask: Option<u32>,
}

/// A scene's `RenderSettings` and `LightmapSettings` essentials.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SceneSettings {
    pub scene: String,
    pub fog: bool,
    pub fog_color: [f32; 4],
    /// `FogMode` (1 linear, 2 exponential, 3 exp²).
    pub fog_mode: i32,
    pub fog_density: f32,
    pub fog_start: f32,
    pub fog_end: f32,
    /// `AmbientMode` (0 skybox, 1 trilight, 3 flat, 4 custom).
    pub ambient_mode: i32,
    pub ambient_sky: [f32; 4],
    pub ambient_equator: [f32; 4],
    pub ambient_ground: [f32; 4],
    pub ambient_intensity: f32,
    /// `m_AmbientProbe`: 27 SH coefficients (3 × 9), Unity's order.
    pub ambient_probe: Vec<f32>,
    pub skybox_material: Option<u32>,
    /// `m_GeneratedSkyboxReflection`: the default reflection cubemap.
    pub reflection: Option<u32>,
    pub reflection_intensity: f32,
    pub lightmaps_mode: i32,
    pub subtractive_shadow_color: [f32; 4],
}

/// `LightController` (Kingmaker.Visual.DayNightCycle): the scene light config it applies
/// at load (`ApplySceneRendreringSettings`), the camera settings it sets, and the
/// post-processing volumes it enables.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LightingSetup {
    pub scene: String,
    /// Which config applied: `solo` or a time of day.
    pub config_kind: String,
    /// `CameraClearFlags` it sets on the game camera.
    pub camera_clear_flag: i32,
    /// `CameraLightInitStatus` (1 enabled, 2 disabled).
    pub camera_light: i32,
    pub main_light: Option<AssetId>,
    pub main_light_rotation: [f32; 3],
    pub main_light_color: [f32; 4],
    pub main_light_intensity: f32,
    pub main_light_indirect_intensity: f32,
    pub main_light_shadow_strength: f32,
    pub sky_ambient: [f32; 4],
    pub equator_ambient: [f32; 4],
    pub ground_ambient: [f32; 4],
    pub fog_color: [f32; 4],
    pub fog_start: f32,
    pub fog_end: f32,
    /// Post-processing volumes enabled for this config.
    pub volumes: Vec<Volume>,
    /// The game camera rig's `CameraLight` (from the player's `level2`), its matrix
    /// relative to the rig. `LightController` enables it when `camera_light` is 1.
    pub rig_light: Option<Light>,
}

/// A post-processing `Volume` and its profile's components.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Volume {
    pub path: String,
    pub is_global: bool,
    pub priority: f32,
    pub weight: f32,
    pub blend_distance: f32,
    pub profile: String,
    /// (component class, its serialized fields as JSON).
    pub components: Vec<(String, String)>,
}

/// A baked reflection probe.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReflectionProbe {
    pub id: AssetId,
    pub scene: String,
    pub path: String,
    pub enabled: bool,
    pub active: bool,
    pub position: [f32; 3],
    pub box_size: [f32; 3],
    pub box_offset: [f32; 3],
    pub box_projection: bool,
    pub blend_distance: f32,
    pub importance: i32,
    pub intensity: f32,
    pub hdr: bool,
    pub texture: Option<u32>,
}

/// A render-pipeline program (not tied to a material), by role.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PipelineProgram {
    /// e.g. `DeferredLighting.vs`, `LightCulling.BuildLightTiles`.
    pub role: String,
    pub program: u32,
    /// Index of the pass state in [`Shader`] form, if any (copied here for convenience).
    pub state: Option<PassState>,
}
