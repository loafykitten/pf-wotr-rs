//! Uploading a pack to the GPU: textures (BC blocks as shipped), samplers from Unity's
//! texture settings, vertex and index buffers, Metal libraries as shader modules, and
//! Unity's default textures.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use wgpu::util::DeviceExt;
use wr_assets::{Pack, Program, TexDimension, TexFormat, TexKind, Texture};

pub struct GpuTexture {
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub size: (u32, u32),
}

pub struct GpuMesh {
    pub streams: Vec<wgpu::Buffer>,
    pub index: wgpu::Buffer,
    /// A skinned mesh's weights and bone indices (`wr_assets::Mesh::skin`); its streams are
    /// also bound as storage then, for the skinning pass.
    pub skin: Option<wgpu::Buffer>,
}

/// Unity's built-in default textures, by kind.
pub struct Defaults {
    pub white: wgpu::TextureView,
    pub black: wgpu::TextureView,
    pub gray: wgpu::TextureView,
    pub bump: wgpu::TextureView,
    pub white_cube: wgpu::TextureView,
    pub black_cube: wgpu::TextureView,
    pub black_3d: wgpu::TextureView,
    pub white_array: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    pub sampler_clamp: wgpu::Sampler,
}

pub struct Gpu {
    pub textures: Vec<Option<GpuTexture>>,
    pub meshes: Vec<GpuMesh>,
    pub modules: HashMap<u64, wgpu::ShaderModule>,
    pub defaults: Defaults,
    /// Bytes uploaded (textures, buffers).
    pub uploaded: u64,
    pub skipped_textures: Vec<String>,
}

fn wgpu_format(t: &Texture) -> wgpu::TextureFormat {
    use wgpu::TextureFormat as F;
    match (t.format, t.srgb) {
        (TexFormat::Bc1, true) => F::Bc1RgbaUnormSrgb,
        (TexFormat::Bc1, false) => F::Bc1RgbaUnorm,
        (TexFormat::Bc3, true) => F::Bc3RgbaUnormSrgb,
        (TexFormat::Bc3, false) => F::Bc3RgbaUnorm,
        (TexFormat::Bc4, _) => F::Bc4RUnorm,
        (TexFormat::Bc5, _) => F::Bc5RgUnorm,
        (TexFormat::Bc6hUfloat, _) => F::Bc6hRgbUfloat,
        (TexFormat::Bc7, true) => F::Bc7RgbaUnormSrgb,
        (TexFormat::Bc7, false) => F::Bc7RgbaUnorm,
        (TexFormat::Rgba8, true) => F::Rgba8UnormSrgb,
        (TexFormat::Rgba8, false) => F::Rgba8Unorm,
        (TexFormat::Bgra8, true) => F::Bgra8UnormSrgb,
        (TexFormat::Bgra8, false) => F::Bgra8Unorm,
        (TexFormat::R8, _) => F::R8Unorm,
        (TexFormat::Rg8, _) => F::Rg8Unorm,
        (TexFormat::Rgba16Float, _) => F::Rgba16Float,
        (TexFormat::R16Float, _) => F::R16Float,
        (TexFormat::Rgba32Float, _) => F::Rgba32Float,
    }
}

/// Unity `TextureWrapMode` → wgpu.
fn address(w: u8) -> wgpu::AddressMode {
    match w {
        1 => wgpu::AddressMode::ClampToEdge,
        2 | 3 => wgpu::AddressMode::MirrorRepeat,
        _ => wgpu::AddressMode::Repeat,
    }
}

/// A sampler from Unity's texture settings. Unity's bilinear filters within a mip and
/// takes the nearest mip; trilinear blends mips. Anisotropy in wgpu needs all-linear
/// filtering, so an anisotropic bilinear texture samples mips linearly here.
pub fn sampler_for(device: &wgpu::Device, wrap: [u8; 3], filter: u8, aniso: u8) -> wgpu::Sampler {
    let lin = filter >= 1;
    let mut mip =
        if filter >= 2 { wgpu::MipmapFilterMode::Linear } else { wgpu::MipmapFilterMode::Nearest };
    let an = if lin && aniso > 1 { u16::from(aniso.min(16)) } else { 1 };
    if an > 1 {
        mip = wgpu::MipmapFilterMode::Linear;
    }
    let f = if lin { wgpu::FilterMode::Linear } else { wgpu::FilterMode::Nearest };
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: None,
        address_mode_u: address(wrap[0]),
        address_mode_v: address(wrap[1]),
        address_mode_w: address(wrap[2]),
        mag_filter: f,
        min_filter: f,
        mipmap_filter: mip,
        anisotropy_clamp: an,
        ..Default::default()
    })
}

fn solid(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    px: [u8; 4],
    dim: wgpu::TextureViewDimension,
) -> wgpu::TextureView {
    let (d, layers) = match dim {
        wgpu::TextureViewDimension::Cube => (wgpu::TextureDimension::D2, 6),
        wgpu::TextureViewDimension::D3 => (wgpu::TextureDimension::D3, 1),
        _ => (wgpu::TextureDimension::D2, 1),
    };
    let tex = device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("default"),
            size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: layers },
            mip_level_count: 1,
            sample_count: 1,
            dimension: d,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &px.repeat(layers as usize),
    );
    tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(dim), ..Default::default() })
}

impl Defaults {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        use wgpu::TextureViewDimension as D;
        Self {
            white: solid(device, queue, [255; 4], D::D2),
            black: solid(device, queue, [0, 0, 0, 0], D::D2),
            gray: solid(device, queue, [128, 128, 128, 128], D::D2),
            // Unity's "bump": a flat normal for both RG and DXT5nm (x = r·a) decodes.
            bump: solid(device, queue, [128, 128, 255, 255], D::D2),
            white_cube: solid(device, queue, [255; 4], D::Cube),
            black_cube: solid(device, queue, [0, 0, 0, 0], D::Cube),
            black_3d: solid(device, queue, [0, 0, 0, 0], D::D3),
            white_array: solid(device, queue, [255; 4], D::D2Array),
            sampler: sampler_for(device, [0; 3], 1, 1),
            sampler_clamp: sampler_for(device, [1; 3], 1, 1),
        }
    }

    /// A default view for a texture kind and Unity default name.
    #[must_use]
    pub fn view(&self, kind: TexKind, name: Option<&str>) -> &wgpu::TextureView {
        match kind {
            TexKind::Cube | TexKind::CubeArray => {
                if name == Some("white") {
                    &self.white_cube
                } else {
                    &self.black_cube
                }
            }
            TexKind::D3 => &self.black_3d,
            TexKind::D2Array | TexKind::Depth2Array => &self.white_array,
            _ => match name {
                Some("white") => &self.white,
                Some("gray" | "grey") => &self.gray,
                Some("bump") => &self.bump,
                Some("black") | None => &self.black,
                _ => &self.gray,
            },
        }
    }
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pack: &Pack,
    t: &Texture,
) -> Result<GpuTexture> {
    let format = wgpu_format(t);
    let (bw, bh) = format.block_dimensions();
    if !t.width.is_multiple_of(bw) || !t.height.is_multiple_of(bh) {
        bail!("{}: {}x{} not a multiple of the {bw}x{bh} block", t.name, t.width, t.height);
    }
    let (dim, view_dim, layers) = match t.dimension {
        TexDimension::D2 => (wgpu::TextureDimension::D2, wgpu::TextureViewDimension::D2, 1),
        TexDimension::Cube => (wgpu::TextureDimension::D2, wgpu::TextureViewDimension::Cube, 6),
        TexDimension::D2Array => {
            (wgpu::TextureDimension::D2, wgpu::TextureViewDimension::D2Array, t.layers)
        }
        TexDimension::D3 => (wgpu::TextureDimension::D3, wgpu::TextureViewDimension::D3, t.layers),
    };
    // Block-compressed mips below 4×4 are copied with the extent rounded up to whole
    // blocks (the physical size), as WebGPU requires.
    let mips = t.mips.max(1);
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(&t.name),
        size: wgpu::Extent3d { width: t.width, height: t.height, depth_or_array_layers: layers },
        mip_level_count: mips,
        sample_count: 1,
        dimension: dim,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let data = pack.blob(t.data);
    let mut off = 0usize;
    // Unity stores each image (face/slice) with its whole mip chain.
    for layer in 0..layers {
        for m in 0..mips {
            let w = (t.width >> m).max(1);
            let h = (t.height >> m).max(1);
            let size = t.format.level_size(w, h) as usize;
            let bytes = data.get(off..off + size).context("texture payload too short")?;
            let (unit, block) = t.format.unit();
            let (bpr, rows) =
                if block { (w.div_ceil(4) * unit, h.div_ceil(4)) } else { (w * unit, h) };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: m,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: layer },
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(rows),
                },
                wgpu::Extent3d {
                    width: w.div_ceil(bw) * bw,
                    height: h.div_ceil(bh) * bh,
                    depth_or_array_layers: 1,
                },
            );
            off += size;
        }
    }
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(view_dim),
        ..Default::default()
    });
    Ok(GpuTexture {
        view,
        sampler: sampler_for(device, t.wrap, t.filter, t.aniso),
        size: (t.width, t.height),
    })
}

fn padded(b: &[u8]) -> Vec<u8> {
    let mut v = b.to_vec();
    v.resize(b.len().div_ceil(4).max(1) * 4, 0);
    v
}

/// Creates the shader module for a program: its offline metallib, or its MSL when the
/// pack has no library for it.
pub fn module(device: &wgpu::Device, pack: &Pack, p: &Program) -> wgpu::ShaderModule {
    let entry = vec![wgpu::PassthroughShaderEntryPoint {
        name: p.entry.clone().into(),
        workgroup_size: (p.threads[0], p.threads[1], p.threads[2]),
    }];
    let desc = match p.metallib {
        Some(lib) => wgpu::ShaderModuleDescriptorPassthrough {
            label: Some(&p.shader),
            entry_points: entry.into(),
            metallib: Some(pack.blob(lib).into()),
            ..Default::default()
        },
        None => wgpu::ShaderModuleDescriptorPassthrough {
            label: Some(&p.shader),
            entry_points: entry.into(),
            msl: Some(String::from_utf8_lossy(pack.blob(p.msl)).into_owned().into()),
            ..Default::default()
        },
    };
    // SAFETY: these are the game's own compiled shaders. Their resource interface is
    // described by layouts built from the MSL's explicit slots (checked contiguous by
    // the importer); wgpu cannot validate passthrough code itself.
    unsafe { device.create_shader_module_passthrough(desc) }
}

impl Gpu {
    pub fn upload(device: &wgpu::Device, queue: &wgpu::Queue, pack: &Pack) -> Result<Self> {
        let m = &pack.meta;
        let mut uploaded = 0u64;
        let mut skipped_textures = Vec::new();
        let mut textures = Vec::with_capacity(m.textures.len());
        for t in &m.textures {
            match upload_texture(device, queue, pack, t) {
                Ok(g) => {
                    uploaded += t.data.len;
                    textures.push(Some(g));
                }
                Err(e) => {
                    skipped_textures.push(format!("{e:#}"));
                    textures.push(None);
                }
            }
        }
        let mut meshes = Vec::with_capacity(m.meshes.len());
        for mesh in &m.meshes {
            let usage = if mesh.skin.is_some() {
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE
            } else {
                wgpu::BufferUsages::VERTEX
            };
            let streams = mesh
                .streams
                .iter()
                .map(|s| {
                    uploaded += s.data.len;
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(&mesh.name),
                        contents: &padded(pack.blob(s.data)),
                        usage,
                    })
                })
                .collect();
            let skin = mesh.skin.map(|b| {
                uploaded += b.len;
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&mesh.name),
                    contents: &padded(pack.blob(b)),
                    usage: wgpu::BufferUsages::STORAGE,
                })
            });
            uploaded += mesh.indices.len;
            let index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&mesh.name),
                contents: &padded(pack.blob(mesh.indices)),
                usage: wgpu::BufferUsages::INDEX,
            });
            meshes.push(GpuMesh { streams, index, skin });
        }
        Ok(Self {
            textures,
            meshes,
            modules: HashMap::new(),
            defaults: Defaults::new(device, queue),
            uploaded,
            skipped_textures,
        })
    }

    pub fn module_for(
        &mut self,
        device: &wgpu::Device,
        pack: &Pack,
        p: &Program,
    ) -> wgpu::ShaderModule {
        self.modules.entry(p.msl_hash).or_insert_with(|| module(device, pack, p)).clone()
    }
}
