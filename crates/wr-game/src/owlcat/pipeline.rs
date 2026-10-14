//! Pipelines around Owlcat's shaders: bind group layouts that reproduce each shader's
//! Metal slots, vertex layouts from Unity's mesh channels, and Unity render state.

use wr_assets::{
    Binding, Material, Mesh, PassState, Program, Resource, Stage, StateValue, TexKind,
};

/// The caster pass's per-face constant buffer (`ShadowCasterBuffer`, a global in Unity).
pub const SHADOW_CASTER_CB: &str = "ShadowCasterBuffer";
/// The sampler the deferred lighting reads the shadow atlas through.
pub const SHADOW_SAMPLER: &str = "sampler_ShadowmapRT";

/// One bind group, entries ordered stage by stage (vertex, fragment or compute) and,
/// within a stage, buffers then samplers then textures by slot. wgpu-hal's Metal backend
/// assigns slots per stage sequentially in this order, so they land on the shader's own
/// `[[buffer(n)]]`, `[[sampler(n)]]` and `[[texture(n)]]` indices (the importer checked
/// that each table is gap-free).
pub fn layout_entries(stages: &[&Program]) -> Vec<(Stage, Binding, wgpu::BindGroupLayoutEntry)> {
    let mut out = Vec::new();
    for p in stages {
        let vis = match p.stage {
            Stage::Vertex => wgpu::ShaderStages::VERTEX,
            Stage::Fragment => wgpu::ShaderStages::FRAGMENT,
            Stage::Compute => wgpu::ShaderStages::COMPUTE,
        };
        for b in &p.bindings {
            let ty = match &b.res {
                Resource::Uniform { .. } => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    // `ShadowCasterPass` rewrites these globals before every face of every
                    // entry; here each (entry, face) is one record of a buffer, selected
                    // by a dynamic offset.
                    has_dynamic_offset: b.name == SHADOW_CASTER_CB,
                    min_binding_size: None,
                },
                Resource::Storage { read_only, .. } => wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: *read_only },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                // The shadow atlas is read with `sample_compare`, which needs a comparison
                // sampler; every other sampler filters.
                Resource::Sampler if b.name == SHADOW_SAMPLER => {
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)
                }
                Resource::Sampler => {
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
                }
                Resource::Texture { kind, sample_type, write } => {
                    let view_dimension = view_dim(*kind);
                    if *write {
                        wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::ReadWrite,
                            format: wgpu::TextureFormat::Rgba16Float,
                            view_dimension,
                        }
                    } else {
                        // Passthrough shaders are not reflected, so wgpu doesn't pair
                        // textures with samplers; "unfilterable float" is the most
                        // permissive entry (any float or depth format binds to it).
                        let st = if matches!(kind, TexKind::Depth2 | TexKind::Depth2Array) {
                            wgpu::TextureSampleType::Depth
                        } else if sample_type.starts_with("uint") {
                            wgpu::TextureSampleType::Uint
                        } else if sample_type.starts_with("int") {
                            wgpu::TextureSampleType::Sint
                        } else {
                            wgpu::TextureSampleType::Float { filterable: false }
                        };
                        wgpu::BindingType::Texture {
                            sample_type: st,
                            view_dimension,
                            multisampled: false,
                        }
                    }
                }
            };
            let entry = wgpu::BindGroupLayoutEntry {
                binding: out.len() as u32,
                visibility: vis,
                ty,
                count: None,
            };
            out.push((p.stage, b.clone(), entry));
        }
    }
    out
}

#[must_use]
pub fn view_dim(k: TexKind) -> wgpu::TextureViewDimension {
    use wgpu::TextureViewDimension as D;
    match k {
        TexKind::D2 | TexKind::Depth2 => D::D2,
        TexKind::D2Array | TexKind::Depth2Array => D::D2Array,
        TexKind::D3 => D::D3,
        TexKind::Cube => D::Cube,
        TexKind::CubeArray => D::CubeArray,
    }
}

/// Mesh channel index for an HLSLcc input semantic.
fn channel_of(semantic: &str) -> Option<usize> {
    Some(match semantic {
        "POSITION0" => 0,
        "NORMAL0" => 1,
        "TANGENT0" => 2,
        "COLOR0" => 3,
        "BLENDWEIGHTS0" | "BLENDWEIGHT0" => 12,
        "BLENDINDICES0" => 13,
        s => 4 + s.strip_prefix("TEXCOORD")?.parse::<usize>().ok().filter(|i| *i < 8)?,
    })
}

/// Unity `VertexFormat` and dimension → wgpu.
fn vertex_format(format: u8, dim: u8) -> Option<wgpu::VertexFormat> {
    use wgpu::VertexFormat as F;
    Some(match (format, dim) {
        (0, 1) => F::Float32,
        (0, 2) => F::Float32x2,
        (0, 3) => F::Float32x3,
        (0, 4) => F::Float32x4,
        (1, 2) => F::Float16x2,
        (1, 4) => F::Float16x4,
        (2, 4) => F::Unorm8x4,
        (2, 2) => F::Unorm8x2,
        (3, 4) => F::Snorm8x4,
        (3, 2) => F::Snorm8x2,
        (4, 2) => F::Unorm16x2,
        (4, 4) => F::Unorm16x4,
        (5, 2) => F::Snorm16x2,
        (5, 4) => F::Snorm16x4,
        (6, 4) => F::Uint8x4,
        (6, 2) => F::Uint8x2,
        (7, 4) => F::Sint8x4,
        (8, 2) => F::Uint16x2,
        (8, 4) => F::Uint16x4,
        (9, 2) => F::Sint16x2,
        (9, 4) => F::Sint16x4,
        (10, 1) => F::Uint32,
        (10, 2) => F::Uint32x2,
        (10, 4) => F::Uint32x4,
        (11, 1) => F::Sint32,
        (11, 4) => F::Sint32x4,
        _ => return None,
    })
}

/// One mesh stream's use: (stream index, stride, attributes as (format, offset, location)).
pub type StreamUse = (usize, u64, Vec<(wgpu::VertexFormat, u64, u32)>);

/// A vertex input layout: mesh streams used, then constant default slots.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VertexLayout {
    /// (mesh stream, stride, attributes as (format, offset, location)).
    pub streams: Vec<StreamUse>,
    /// Attributes the mesh lacks: (location, format, white?) fed by a stride-0 buffer.
    pub defaults: Vec<(u32, wgpu::VertexFormat, bool)>,
}

/// Matches a vertex program's inputs to a mesh's channels. Missing inputs read a
/// constant: white for `COLOR`, zero otherwise (Unity's defaults).
pub fn vertex_layout(vs: &Program, mesh: &Mesh) -> Result<VertexLayout, String> {
    let mut streams: Vec<StreamUse> = Vec::new();
    let mut defaults = Vec::new();
    for vi in &vs.vertex_inputs {
        let ch = channel_of(&vi.semantic).ok_or_else(|| format!("semantic {}", vi.semantic))?;
        match mesh.channels.get(ch).copied().flatten() {
            Some(c) => {
                let f = vertex_format(c.format, c.dimension)
                    .ok_or_else(|| format!("vertex format {}x{}", c.format, c.dimension))?;
                let s = usize::from(c.stream);
                let stride = u64::from(mesh.streams.get(s).map_or(0, |x| x.stride));
                match streams.iter_mut().find(|x| x.0 == s) {
                    Some(e) => e.2.push((f, u64::from(c.offset), vi.attribute)),
                    None => streams.push((s, stride, vec![(f, u64::from(c.offset), vi.attribute)])),
                }
            }
            None => {
                let f = if vi.ty.starts_with("uint") {
                    wgpu::VertexFormat::Uint32x4
                } else if vi.ty.starts_with("int") {
                    wgpu::VertexFormat::Sint32x4
                } else {
                    wgpu::VertexFormat::Float32x4
                };
                defaults.push((vi.attribute, f, ch == 3));
            }
        }
    }
    streams.sort_by_key(|s| s.0);
    Ok(VertexLayout { streams, defaults })
}

fn state_value(v: &StateValue, mat: Option<&Material>) -> f32 {
    match (&v.property, mat) {
        (Some(p), Some(m)) => m.float(p).unwrap_or(v.value),
        _ => v.value,
    }
}

fn blend_factor(v: f32) -> wgpu::BlendFactor {
    use wgpu::BlendFactor as B;
    match v as i32 {
        0 => B::Zero,
        2 => B::Dst,
        3 => B::Src,
        4 => B::OneMinusDst,
        5 => B::SrcAlpha,
        6 => B::OneMinusSrc,
        7 => B::DstAlpha,
        8 => B::OneMinusDstAlpha,
        9 => B::SrcAlphaSaturated,
        10 => B::OneMinusSrcAlpha,
        _ => B::One,
    }
}

fn blend_op(v: f32) -> wgpu::BlendOperation {
    match v as i32 {
        1 => wgpu::BlendOperation::Subtract,
        2 => wgpu::BlendOperation::ReverseSubtract,
        3 => wgpu::BlendOperation::Min,
        4 => wgpu::BlendOperation::Max,
        _ => wgpu::BlendOperation::Add,
    }
}

/// Unity `CompareFunction`, flipped for reversed Z as Unity does on Metal.
fn compare(v: f32) -> wgpu::CompareFunction {
    use wgpu::CompareFunction as C;
    match v as i32 {
        1 => C::Never,
        2 => C::Greater,
        3 => C::Equal,
        4 => C::GreaterEqual,
        5 => C::Less,
        6 => C::NotEqual,
        7 => C::LessEqual,
        _ => C::Always,
    }
}

/// Resolved fixed-function state for one pass and material.
#[derive(Debug, Clone, PartialEq, Hash, Eq)]
pub struct RenderState {
    pub cull: u8,
    pub ztest: u8,
    pub zwrite: bool,
    /// (src, dst, src α, dst α, op, op α, mask) as Unity enums, per target.
    pub blend: Vec<[u8; 7]>,
    /// Raster slope-scaled depth bias, as `f32` bits (`Offset [_OffsetFactor], …`; 0 = none).
    pub slope_bias: u32,
}

#[must_use]
pub fn resolve_state(s: &PassState, mat: Option<&Material>, targets: usize) -> RenderState {
    let mut blend = Vec::new();
    for i in 0..targets {
        let b = if s.separate_blend { s.blend.get(i) } else { s.blend.first() };
        let Some(b) = b else {
            blend.push([1, 0, 1, 0, 0, 0, 15]);
            continue;
        };
        let g = |v: &StateValue| state_value(v, mat) as u8;
        blend.push([
            g(&b.src),
            g(&b.dst),
            g(&b.src_alpha),
            g(&b.dst_alpha),
            g(&b.op),
            g(&b.op_alpha),
            g(&b.color_mask),
        ]);
    }
    RenderState {
        cull: state_value(&s.cull, mat) as u8,
        ztest: state_value(&s.z_test, mat) as u8,
        zwrite: state_value(&s.z_write, mat) > 0.5,
        blend,
        slope_bias: 0,
    }
}

impl RenderState {
    /// The same state with a slope-scaled raster depth bias.
    #[must_use]
    pub fn with_slope_bias(mut self, slope: f32) -> Self {
        self.slope_bias = slope.to_bits();
        self
    }

    #[must_use]
    pub fn color_target(&self, i: usize, format: wgpu::TextureFormat) -> wgpu::ColorTargetState {
        let b = self.blend.get(i).copied().unwrap_or([1, 0, 1, 0, 0, 0, 15]);
        let opaque = b[0] == 1 && b[1] == 0 && b[2] == 1 && b[3] == 0 && b[4] == 0 && b[5] == 0;
        let mask = b[6];
        // Unity ColorWriteMask: A 1, B 2, G 4, R 8.
        let mut w = wgpu::ColorWrites::empty();
        if mask & 8 != 0 {
            w |= wgpu::ColorWrites::RED;
        }
        if mask & 4 != 0 {
            w |= wgpu::ColorWrites::GREEN;
        }
        if mask & 2 != 0 {
            w |= wgpu::ColorWrites::BLUE;
        }
        if mask & 1 != 0 {
            w |= wgpu::ColorWrites::ALPHA;
        }
        wgpu::ColorTargetState {
            format,
            blend: (!opaque).then(|| wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: blend_factor(f32::from(b[0])),
                    dst_factor: blend_factor(f32::from(b[1])),
                    operation: blend_op(f32::from(b[4])),
                },
                alpha: wgpu::BlendComponent {
                    src_factor: blend_factor(f32::from(b[2])),
                    dst_factor: blend_factor(f32::from(b[3])),
                    operation: blend_op(f32::from(b[5])),
                },
            }),
            write_mask: w,
        }
    }

    /// Culling. Our targets are rendered with Unity's flipped "render into texture"
    /// projection, which mirrors screen-space winding; `flip` selects which winding is
    /// front (set once for the whole frame, see `Renderer`).
    #[must_use]
    pub fn primitive(&self, front_ccw: bool) -> wgpu::PrimitiveState {
        wgpu::PrimitiveState {
            front_face: if front_ccw { wgpu::FrontFace::Ccw } else { wgpu::FrontFace::Cw },
            cull_mode: match self.cull {
                1 => Some(wgpu::Face::Front),
                2 => Some(wgpu::Face::Back),
                _ => None,
            },
            ..Default::default()
        }
    }

    #[must_use]
    pub fn depth(&self, format: Option<wgpu::TextureFormat>) -> Option<wgpu::DepthStencilState> {
        format.map(|format| wgpu::DepthStencilState {
            format,
            depth_write_enabled: Some(self.zwrite),
            depth_compare: Some(compare(f32::from(self.ztest))),
            stencil: Default::default(),
            bias: wgpu::DepthBiasState {
                constant: 0,
                slope_scale: f32::from_bits(self.slope_bias),
                clamp: 0.0,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wr_assets::{Channel, VertexInput, VertexStream};

    #[test]
    fn semantics_and_layout() {
        assert_eq!(channel_of("TEXCOORD1"), Some(5));
        assert_eq!(channel_of("COLOR0"), Some(3));
        let mut mesh = Mesh { channels: vec![None; 14], ..Default::default() };
        mesh.channels[0] = Some(Channel { stream: 0, offset: 0, format: 0, dimension: 3 });
        mesh.channels[4] = Some(Channel { stream: 0, offset: 12, format: 1, dimension: 2 });
        mesh.streams = vec![VertexStream { stride: 16, ..Default::default() }];
        let vs = Program {
            vertex_inputs: vec![
                VertexInput { attribute: 0, ty: "float4".into(), semantic: "POSITION0".into() },
                VertexInput { attribute: 3, ty: "float2".into(), semantic: "TEXCOORD0".into() },
                VertexInput { attribute: 5, ty: "float4".into(), semantic: "COLOR0".into() },
            ],
            ..Default::default()
        };
        let l = vertex_layout(&vs, &mesh).unwrap();
        assert_eq!(l.streams.len(), 1);
        assert_eq!(l.streams[0].1, 16);
        assert_eq!(l.streams[0].2[1], (wgpu::VertexFormat::Float16x2, 12, 3));
        assert_eq!(l.defaults, vec![(5, wgpu::VertexFormat::Float32x4, true)]);
    }

    #[test]
    fn reversed_z_flips_comparisons() {
        assert_eq!(compare(4.0), wgpu::CompareFunction::GreaterEqual);
        assert_eq!(compare(8.0), wgpu::CompareFunction::Always);
        let s = RenderState {
            cull: 2,
            ztest: 4,
            zwrite: true,
            blend: vec![[1, 0, 1, 0, 0, 0, 15]],
            slope_bias: 0,
        };
        assert!(s.color_target(0, wgpu::TextureFormat::Rgba8Unorm).blend.is_none());
        assert_eq!(
            s.color_target(0, wgpu::TextureFormat::Rgba8Unorm).write_mask,
            wgpu::ColorWrites::ALL
        );
    }
}
