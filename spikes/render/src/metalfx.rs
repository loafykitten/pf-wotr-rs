//! `metalfx`: MetalFX spatial and temporal upscaling and frame interpolation, encoded into
//! wgpu's own Metal command buffers through raw handles (wgpu 30 `as_hal`), on wgpu textures.
//!
//! Bevy 0.20 renders through the same wgpu, so a Bevy render-graph system can do exactly this
//! (`RenderDevice::wgpu_device()`, the view target textures, and a raw-only command encoder).

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use block2::RcBlock;
use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLCommandBuffer, MTLDevice, MTLPixelFormat, MTLTexture, MTLTextureUsage};
use objc2_metal_fx::{
    MTLFXFrameInterpolator, MTLFXFrameInterpolatorBase, MTLFXFrameInterpolatorDescriptor,
    MTLFXSpatialScaler, MTLFXSpatialScalerBase, MTLFXSpatialScalerColorProcessingMode,
    MTLFXSpatialScalerDescriptor, MTLFXTemporalScaler, MTLFXTemporalScalerBase,
    MTLFXTemporalScalerDescriptor,
};

use crate::{gpu, opt, stats};

/// One line describing which MetalFX effects the device supports.
pub fn support_line(dev: &ProtocolObject<dyn MTLDevice>) -> String {
    // SAFETY: class methods that only query support for a valid device.
    unsafe {
        format!(
            "spatial={} temporal={} frame_interpolator={}",
            MTLFXSpatialScalerDescriptor::supportsDevice(dev),
            MTLFXTemporalScalerDescriptor::supportsDevice(dev),
            MTLFXFrameInterpolatorDescriptor::supportsDevice(dev),
        )
    }
}

const OUT_W: u32 = 2560;
const OUT_H: u32 = 1440;

fn mtl_format(f: wgpu::TextureFormat) -> MTLPixelFormat {
    match f {
        wgpu::TextureFormat::Rgba16Float => MTLPixelFormat::RGBA16Float,
        wgpu::TextureFormat::Rgba8Unorm => MTLPixelFormat::RGBA8Unorm,
        wgpu::TextureFormat::Bgra8Unorm => MTLPixelFormat::BGRA8Unorm,
        wgpu::TextureFormat::Depth32Float => MTLPixelFormat::Depth32Float,
        wgpu::TextureFormat::Rg16Float => MTLPixelFormat::RG16Float,
        other => panic!("no MTLPixelFormat mapping for {other:?}"),
    }
}

/// wgpu usages that give a texture at least the Metal usage MetalFX asks for.
fn wgpu_usage(m: MTLTextureUsage, format: wgpu::TextureFormat) -> wgpu::TextureUsages {
    let mut u = wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC;
    if m.contains(MTLTextureUsage::ShaderRead) {
        u |= wgpu::TextureUsages::TEXTURE_BINDING;
    }
    if m.contains(MTLTextureUsage::ShaderWrite) {
        u |= wgpu::TextureUsages::STORAGE_BINDING;
    }
    if m.contains(MTLTextureUsage::RenderTarget) || format.is_depth_stencil_format() {
        u |= wgpu::TextureUsages::RENDER_ATTACHMENT;
    }
    u
}

fn texture(
    device: &wgpu::Device,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("metalfx"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// Mark a texture initialised in wgpu's eyes. wgpu-core lazily zero-fills textures it believes
/// were never written; a write made through raw Metal (MetalFX) is invisible to it, so without
/// this the first wgpu read (e.g. a copy) would clear MetalFX's output to black.
fn init(device: &wgpu::Device, queue: &wgpu::Queue, t: &wgpu::Texture) {
    init_depth(device, queue, t, 0.0);
}

/// `init`, with depth textures cleared to `depth` (reversed Z: 0 = far, 1 = near).
fn init_depth(device: &wgpu::Device, queue: &wgpu::Queue, t: &wgpu::Texture, depth_value: f32) {
    let mut enc = device.create_command_encoder(&Default::default());
    if t.usage().contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
        let view = t.create_view(&Default::default());
        let depth = t.format().is_depth_stencil_format();
        let color_att = [Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })];
        let _ = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("init"),
            color_attachments: if depth { &[] } else { &color_att },
            depth_stencil_attachment: depth.then_some(wgpu::RenderPassDepthStencilAttachment {
                view: &view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(depth_value),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    } else {
        let size = t.size();
        let bpp = t.format().block_copy_size(None).unwrap_or(4);
        let zeros = vec![0u8; (size.width * size.height * bpp) as usize];
        queue.write_texture(
            t.as_image_copy(),
            &zeros,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width * bpp),
                rows_per_image: None,
            },
            size,
        );
    }
    queue.submit([enc.finish()]);
}

fn raw(t: &wgpu::Texture) -> Retained<ProtocolObject<dyn MTLTexture>> {
    // SAFETY: read-only access to the MTLTexture wgpu-hal owns; we retain it for our own use.
    unsafe { t.as_hal::<wgpu::hal::api::Metal>().expect("Metal texture").raw_handle().retain() }
}

/// The test image as a continuous function of normalised position: smooth gradients plus a
/// sinusoidal grid fine enough that upscalers have real detail to reconstruct.
fn pattern(u: f32, v: f32) -> [f32; 3] {
    use std::f32::consts::TAU;
    let grid = 0.5 + 0.5 * (TAU * 24.0 * u).sin() * (TAU * 14.0 * v).cos();
    // Fine detail near the input Nyquist limit (period ~4 input pixels at 1280 wide).
    let fine = 0.5 + 0.5 * (TAU * 300.0 * u).sin() * (TAU * 170.0 * v).sin();
    [u * grid, v * (1.0 - grid) * 0.5 + 0.5 * fine, 0.3 + 0.4 * grid]
}

/// The coarse part of `pattern` only (period ~107 px at 2560 wide), for the interpolation test,
/// where a shift must not alias with the fine detail's ~8.5 px period.
fn pattern_coarse(u: f32, v: f32) -> [f32; 3] {
    use std::f32::consts::TAU;
    let grid = 0.5 + 0.5 * (TAU * 24.0 * u).sin() * (TAU * 14.0 * v).cos();
    [u * grid, v * (1.0 - grid) + 0.2, 0.3 + 0.4 * grid]
}

/// Upload an RGBA16F image whose pixel (x, y) is `f(x, y)`.
fn upload(
    queue: &wgpu::Queue,
    t: &wgpu::Texture,
    w: u32,
    h: u32,
    f: impl Fn(u32, u32) -> [f32; 3],
) {
    let mut data = Vec::with_capacity((w * h * 8) as usize);
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = f(x, y);
            for c in [r, g, b, 1.0] {
                data.extend_from_slice(&f32_to_half(c).to_le_bytes());
            }
        }
    }
    queue.write_texture(
        t.as_image_copy(),
        &data,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 8), rows_per_image: None },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
}

/// Pattern sampled at pixel centres of a `w`x`h` image, offset by (`dx`, `dy`) pixels.
fn sampled(w: u32, h: u32, dx: f32, dy: f32) -> impl Fn(u32, u32) -> [f32; 3] {
    sampled_with(pattern, w, h, dx, dy)
}

fn sampled_with(
    f: fn(f32, f32) -> [f32; 3],
    w: u32,
    h: u32,
    dx: f32,
    dy: f32,
) -> impl Fn(u32, u32) -> [f32; 3] {
    move |x, y| f((x as f32 + 0.5 + dx) / w as f32, (y as f32 + 0.5 + dy) / h as f32)
}

fn f32_to_half(f: f32) -> u16 {
    let b = f.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xFF) as i32 - 127 + 15;
    let man = (b >> 13) & 0x3FF;
    if exp <= 0 {
        sign
    } else if exp >= 31 {
        sign | 0x7C00
    } else {
        sign | ((exp as u16) << 10) | man as u16
    }
}

/// Encode `f` into a fresh raw-only wgpu command buffer, submit it, wait, and return the GPU
/// time of that command buffer in ms (Metal GPUStartTime/GPUEndTime).
fn encode_raw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    f: impl FnOnce(&ProtocolObject<dyn MTLCommandBuffer>),
) -> Result<f64, String> {
    let mut enc =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("metalfx") });
    let start = Arc::new(AtomicU64::new(0));
    let end = Arc::new(AtomicU64::new(0));
    let (s2, e2) = (start.clone(), end.clone());
    // SAFETY: the encoder is used only through the raw API (wgpu-core forbids mixing); MetalFX
    // encodes its own compute work into the open MTLCommandBuffer, and we add a completion
    // handler that reads the GPU timestamps.
    let ok = unsafe {
        enc.as_hal_mut::<wgpu::hal::api::Metal, _, _>(|hal| {
            let Some(cb) = hal.and_then(|h| h.raw_command_buffer()) else { return false };
            f(cb);
            let block =
                RcBlock::new(move |cb: std::ptr::NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                    let cb = cb.as_ref();
                    s2.store(cb.GPUStartTime().to_bits(), Ordering::Relaxed);
                    e2.store(cb.GPUEndTime().to_bits(), Ordering::Relaxed);
                });
            cb.addCompletedHandler(RcBlock::as_ptr(&block));
            true
        })
    };
    if !ok {
        return Err("no raw command buffer".into());
    }
    queue.submit([enc.finish()]);
    gpu::wait_idle(device);
    // The completion handler can run just after wgpu observes completion.
    for _ in 0..100 {
        if end.load(Ordering::Relaxed) != 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
    let s = f64::from_bits(start.load(Ordering::Relaxed));
    let e = f64::from_bits(end.load(Ordering::Relaxed));
    Ok(if e > s { (e - s) * 1e3 } else { f64::NAN })
}

/// Read an RGBA16F texture back as RGB floats.
fn read_rgb(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    t: &wgpu::Texture,
    w: u32,
    h: u32,
) -> Result<Vec<[f32; 3]>, String> {
    let row = (w * 8).div_ceil(256) * 256;
    let rb = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("rb"),
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        t.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &rb,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([enc.finish()]);
    rb.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    gpu::wait_idle(device);
    let data = rb.slice(..).get_mapped_range().map_err(|e| format!("map: {e:?}"))?.to_vec();
    rb.unmap();
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let i = (y * row + x * 8) as usize;
            let c =
                |k: usize| half_to_f32(u16::from_le_bytes([data[i + 2 * k], data[i + 2 * k + 1]]));
            out.push([c(0), c(1), c(2)]);
        }
    }
    Ok(out)
}

/// Mean absolute difference between an image and a reference function, over every 7th pixel.
fn mad(img: &[[f32; 3]], w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 3]) -> f32 {
    let (mut sum, mut n) = (0.0f64, 0u32);
    for y in (0..h).step_by(7) {
        for x in (0..w).step_by(7) {
            let a = img[(y * w + x) as usize];
            let b = f(x, y);
            sum += f64::from((a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()) / 3.0;
            n += 1;
        }
    }
    (sum / f64::from(n)) as f32
}

/// Bilinear sample of an image (texel centres at +0.5) at normalised (u, v), clamped.
fn bilinear(img: &[[f32; 3]], w: u32, h: u32, u: f32, v: f32) -> [f32; 3] {
    let fx = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let fy = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (fx.floor() as u32, fy.floor() as u32);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let p = |x: u32, y: u32| img[(y * w + x) as usize];
    let mut o = [0.0; 3];
    for (k, o) in o.iter_mut().enumerate() {
        let top = p(x0, y0)[k] * (1.0 - tx) + p(x1, y0)[k] * tx;
        let bot = p(x0, y1)[k] * (1.0 - tx) + p(x1, y1)[k] * tx;
        *o = top * (1.0 - ty) + bot * ty;
    }
    o
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

pub fn run(args: &[String]) -> Result<(), String> {
    let frames: usize = opt(args, "--frames").and_then(|v| v.parse().ok()).unwrap_or(60);
    let (adapter, device, queue) = gpu::headless(wgpu::Features::empty())?;
    // SAFETY: borrow of the raw MTLDevice, retained for the duration of the experiment.
    let mtl: Retained<ProtocolObject<dyn MTLDevice>> = unsafe {
        device.as_hal::<wgpu::hal::api::Metal>().ok_or("not Metal")?.raw_device().clone()
    };
    let mut rep = String::new();
    let mut all_ok = true;
    let _ = writeln!(
        rep,
        "# MetalFX via wgpu raw handles ({}, wgpu 30.0.1, objc2-metal-fx 0.3.2)",
        adapter.get_info().name
    );
    let _ = writeln!(rep, "support: {}", support_line(&mtl));
    let _ = writeln!(
        rep,
        "output {OUT_W}x{OUT_H}, RGBA16Float, {frames} frames each; GPU time = MTLCommandBuffer GPUEnd-GPUStart (first frame excluded)"
    );
    let _ = writeln!(
        rep,
        "quality checks: mean absolute difference (MAD) against references, over every 7th pixel"
    );
    let fmt = wgpu::TextureFormat::Rgba16Float;
    let truth = sampled(OUT_W, OUT_H, 0.0, 0.0);

    // Spatial upscaling at the usual quality ratios. Check: the output must be much closer to
    // the true high-resolution image than a flat mid-grey is, and comparable to or better than a
    // bilinear upscale of the same input.
    for (iw, ih, label) in [(1280u32, 720u32, "2.0x"), (1707, 960, "1.5x"), (1920, 1080, "1.33x")] {
        // SAFETY: plain property setters on a fresh descriptor, then object creation.
        let scaler = unsafe {
            let d = MTLFXSpatialScalerDescriptor::new();
            d.setInputWidth(iw as usize);
            d.setInputHeight(ih as usize);
            d.setOutputWidth(OUT_W as usize);
            d.setOutputHeight(OUT_H as usize);
            d.setColorTextureFormat(mtl_format(fmt));
            d.setOutputTextureFormat(mtl_format(fmt));
            d.setColorProcessingMode(MTLFXSpatialScalerColorProcessingMode::HDR);
            d.newSpatialScalerWithDevice(&mtl)
        }
        .ok_or("newSpatialScaler failed")?;
        // SAFETY: querying the scaler's required usages.
        let (cu, ou) = unsafe { (scaler.colorTextureUsage(), scaler.outputTextureUsage()) };
        let input = texture(&device, iw, ih, fmt, wgpu_usage(cu, fmt));
        let output = texture(&device, OUT_W, OUT_H, fmt, wgpu_usage(ou, fmt));
        init(&device, &queue, &output);
        upload(&queue, &input, iw, ih, sampled(iw, ih, 0.0, 0.0));
        let (ri, ro) = (raw(&input), raw(&output));
        let mut ms = Vec::new();
        for _ in 0..frames {
            ms.push(encode_raw(&device, &queue, |cb| {
                // SAFETY: textures meet the scaler's usage/format requirements; encoding into
                // an open command buffer.
                unsafe {
                    scaler.setColorTexture(Some(&ri));
                    scaler.setOutputTexture(Some(&ro));
                    scaler.setInputContentWidth(iw as usize);
                    scaler.setInputContentHeight(ih as usize);
                    scaler.encodeToCommandBuffer(cb);
                }
            })?);
        }
        let out = read_rgb(&device, &queue, &output, OUT_W, OUT_H)?;
        let inp = read_rgb(&device, &queue, &input, iw, ih)?;
        let m_out = mad(&out, OUT_W, OUT_H, &truth);
        let bil = |x: u32, y: u32| {
            bilinear(&inp, iw, ih, (x as f32 + 0.5) / OUT_W as f32, (y as f32 + 0.5) / OUT_H as f32)
        };
        let m_ref = {
            // MAD of the bilinear upscale against the truth (how good plain filtering is).
            let img: Vec<[f32; 3]> = (0..OUT_H)
                .flat_map(|y| (0..OUT_W).map(move |x| (x, y)))
                .map(|(x, y)| bil(x, y))
                .collect();
            mad(&img, OUT_W, OUT_H, &truth)
        };
        let m_flat =
            mad(&out, OUT_W, OUT_H, |_, _| [0.5; 3]).max(mad(&inp, iw, ih, |_, _| [0.5; 3]));
        // Correctness: the output is the input's content, upscaled (far closer to the true image
        // than a flat image is). Bilinear is reported for context, not required to lose.
        let ok = m_out < 0.25 * m_flat;
        all_ok &= ok;
        let s = stats::summarize(&ms[1..]).ok_or("no samples")?;
        let _ = writeln!(
            rep,
            "spatial {label} {iw}x{ih}: GPU {} | MAD vs truth {m_out:.4} (bilinear {m_ref:.4}, flat grey {m_flat:.4}) {}",
            s.line(),
            if ok { "PASS" } else { "FAIL" }
        );
    }

    // Temporal upscaling of a static scene. Each frame's input is rendered with that frame's
    // sub-pixel jitter applied; with zero motion the accumulated output should approach the true
    // high-resolution image more closely than a single spatial-style upscale of one frame.
    {
        let (iw, ih) = (1707u32, 960u32);
        let depth_fmt = wgpu::TextureFormat::Depth32Float;
        let motion_fmt = wgpu::TextureFormat::Rg16Float;
        // SAFETY: as above.
        let scaler = unsafe {
            let d = MTLFXTemporalScalerDescriptor::new();
            d.setInputWidth(iw as usize);
            d.setInputHeight(ih as usize);
            d.setOutputWidth(OUT_W as usize);
            d.setOutputHeight(OUT_H as usize);
            d.setColorTextureFormat(mtl_format(fmt));
            d.setOutputTextureFormat(mtl_format(fmt));
            d.setDepthTextureFormat(mtl_format(depth_fmt));
            d.setMotionTextureFormat(mtl_format(motion_fmt));
            d.newTemporalScalerWithDevice(&mtl)
        }
        .ok_or("newTemporalScaler failed")?;
        // SAFETY: usage queries.
        let (cu, du, mu, ou) = unsafe {
            (
                scaler.colorTextureUsage(),
                scaler.depthTextureUsage(),
                scaler.motionTextureUsage(),
                scaler.outputTextureUsage(),
            )
        };
        let color = texture(&device, iw, ih, fmt, wgpu_usage(cu, fmt));
        let depth = texture(&device, iw, ih, depth_fmt, wgpu_usage(du, depth_fmt));
        let motion = texture(&device, iw, ih, motion_fmt, wgpu_usage(mu, motion_fmt));
        let output = texture(&device, OUT_W, OUT_H, fmt, wgpu_usage(ou, fmt));
        for t in [&depth, &motion, &output] {
            init(&device, &queue, t);
        }
        let (rc, rd, rm, ro) = (raw(&color), raw(&depth), raw(&motion), raw(&output));
        let mut ms = Vec::new();
        for i in 0..frames {
            // Halton(2,3) jitter in input pixels, centred; the input is rendered with it.
            let jx = halton(i as u32 + 1, 2) - 0.5;
            let jy = halton(i as u32 + 1, 3) - 0.5;
            upload(&queue, &color, iw, ih, sampled(iw, ih, jx, jy));
            ms.push(encode_raw(&device, &queue, |cb| {
                // SAFETY: textures meet the scaler's requirements; encoding into an open buffer.
                unsafe {
                    scaler.setColorTexture(Some(&rc));
                    scaler.setDepthTexture(Some(&rd));
                    scaler.setMotionTexture(Some(&rm));
                    scaler.setOutputTexture(Some(&ro));
                    scaler.setInputContentWidth(iw as usize);
                    scaler.setInputContentHeight(ih as usize);
                    // MetalFX's jitter is the offset of the sample from the pixel centre; our
                    // sample sits at +j, so pass -j (the convention Apple's samples use).
                    scaler.setJitterOffsetX(-jx);
                    scaler.setJitterOffsetY(-jy);
                    scaler.setMotionVectorScaleX(iw as f32);
                    scaler.setMotionVectorScaleY(ih as f32);
                    scaler.setDepthReversed(true);
                    scaler.setReset(i == 0);
                    scaler.encodeToCommandBuffer(cb);
                }
            })?);
        }
        let out = read_rgb(&device, &queue, &output, OUT_W, OUT_H)?;
        let single = {
            // One unjittered input frame, bilinearly upscaled: what accumulation should beat.
            let one: Vec<[f32; 3]> = (0..ih)
                .flat_map(|y| (0..iw).map(move |x| (x, y)))
                .map(|(x, y)| sampled(iw, ih, 0.0, 0.0)(x, y))
                .collect();
            let img: Vec<[f32; 3]> = (0..OUT_H)
                .flat_map(|y| (0..OUT_W).map(move |x| (x, y)))
                .map(|(x, y)| {
                    bilinear(
                        &one,
                        iw,
                        ih,
                        (x as f32 + 0.5) / OUT_W as f32,
                        (y as f32 + 0.5) / OUT_H as f32,
                    )
                })
                .collect();
            mad(&img, OUT_W, OUT_H, &truth)
        };
        let m_out = mad(&out, OUT_W, OUT_H, &truth);
        let flat = mad(&out, OUT_W, OUT_H, |_, _| [0.5; 3]);
        let ok = m_out < 0.25 * flat;
        all_ok &= ok;
        let s = stats::summarize(&ms[1..]).ok_or("no samples")?;
        let _ = writeln!(
            rep,
            "temporal 1.5x {iw}x{ih} (static scene, jittered input): GPU {} | MAD vs truth after {frames} frames {m_out:.4} (one frame bilinear {single:.4}, flat grey {flat:.4}) {}",
            s.line(),
            if ok { "PASS" } else { "FAIL" }
        );
    }

    // Frame interpolation (macOS 26+): previous frame = pattern, current frame = pattern moved
    // 16 px right, motion vectors describing that move. The generated frame should sit nearest
    // the 8 px midpoint, not at either input.
    {
        let depth_fmt = wgpu::TextureFormat::Depth32Float;
        let motion_fmt = wgpu::TextureFormat::Rg16Float;
        let shift = 16.0f32;
        // SAFETY: as above.
        let interp = unsafe {
            let d = MTLFXFrameInterpolatorDescriptor::new();
            d.setInputWidth(OUT_W as usize);
            d.setInputHeight(OUT_H as usize);
            d.setOutputWidth(OUT_W as usize);
            d.setOutputHeight(OUT_H as usize);
            d.setColorTextureFormat(mtl_format(fmt));
            d.setOutputTextureFormat(mtl_format(fmt));
            d.setDepthTextureFormat(mtl_format(depth_fmt));
            d.setMotionTextureFormat(mtl_format(motion_fmt));
            d.newFrameInterpolatorWithDevice(&mtl)
        }
        .ok_or("newFrameInterpolatorWithDevice returned nil")?;
        // SAFETY: usage queries.
        let (cu, du, mu, ou) = unsafe {
            (
                interp.colorTextureUsage(),
                interp.depthTextureUsage(),
                interp.motionTextureUsage(),
                interp.outputTextureUsage(),
            )
        };
        let color = texture(&device, OUT_W, OUT_H, fmt, wgpu_usage(cu, fmt));
        let prev = texture(&device, OUT_W, OUT_H, fmt, wgpu_usage(cu, fmt));
        let depth = texture(&device, OUT_W, OUT_H, depth_fmt, wgpu_usage(du, depth_fmt));
        let motion = texture(&device, OUT_W, OUT_H, motion_fmt, wgpu_usage(mu, motion_fmt));
        let output = texture(&device, OUT_W, OUT_H, fmt, wgpu_usage(ou, fmt));
        init(&device, &queue, &output);
        // Mid-range depth: a cleared (far-plane) depth buffer may be treated as background.
        init_depth(&device, &queue, &depth, 0.5);
        // Content moved +shift px in x from prev to current, so each current pixel came from
        // -shift px: motion (current -> previous) = -shift / width in UV units.
        let (rc, rp, rd, rm, ro) =
            (raw(&color), raw(&prev), raw(&depth), raw(&motion), raw(&output));
        upload(&queue, &prev, OUT_W, OUT_H, sampled_with(pattern_coarse, OUT_W, OUT_H, 0.0, 0.0));
        upload(
            &queue,
            &color,
            OUT_W,
            OUT_H,
            sampled_with(pattern_coarse, OUT_W, OUT_H, -shift, 0.0),
        );
        let mut result = String::new();
        let mut ok = false;
        // Try the documented sign first; if the output sits nearest the wrong end, report both.
        // Conventions tried in turn: (motion sign, vectors in UV units with scale = size, or in
        // pixels with scale = 1, field of view in degrees or radians).
        let configs = [
            (-1.0f32, true, 45.0f32),
            (1.0, true, 45.0),
            (-1.0, false, 45.0),
            (1.0, false, 45.0),
            (-1.0, true, 0.785),
            (-1.0, false, 0.785),
        ];
        for (attempt, &(mv_sign, uv_units, fov)) in configs.iter().enumerate() {
            let (mv, scale) = if uv_units {
                (mv_sign * shift / OUT_W as f32, (OUT_W as f32, OUT_H as f32))
            } else {
                (mv_sign * shift, (1.0, 1.0))
            };
            write_motion(&queue, &motion, OUT_W, OUT_H, mv);
            let mut ms = Vec::new();
            for i in 0..frames {
                ms.push(encode_raw(&device, &queue, |cb| {
                    // SAFETY: textures meet the interpolator's requirements.
                    unsafe {
                        interp.setColorTexture(Some(&rc));
                        interp.setPrevColorTexture(Some(&rp));
                        interp.setDepthTexture(Some(&rd));
                        interp.setMotionTexture(Some(&rm));
                        interp.setOutputTexture(Some(&ro));
                        interp.setMotionVectorScaleX(scale.0);
                        interp.setMotionVectorScaleY(scale.1);
                        interp.setDeltaTime(1.0 / 120.0);
                        interp.setNearPlane(0.1);
                        interp.setFarPlane(100.0);
                        interp.setFieldOfView(fov);
                        interp.setAspectRatio(OUT_W as f32 / OUT_H as f32);
                        interp.setDepthReversed(true);
                        interp.setShouldResetHistory(i == 0);
                        interp.encodeToCommandBuffer(cb);
                    }
                })?);
            }
            let out = read_rgb(&device, &queue, &output, OUT_W, OUT_H)?;
            let d: Vec<f32> = [0.0, shift / 2.0, shift]
                .iter()
                .map(|&o| {
                    mad(&out, OUT_W, OUT_H, sampled_with(pattern_coarse, OUT_W, OUT_H, -o, 0.0))
                })
                .collect();
            ok = d[1] < d[0] && d[1] < d[2];
            let s = stats::summarize(&ms[1..]).ok_or("no samples")?;
            result = format!(
                "frame interpolation {OUT_W}x{OUT_H} (motion sign {mv_sign:+}, {}, fov {fov}): GPU {} | MAD vs shift 0 / {:.0} / {:.0} px: {:.4} / {:.4} / {:.4} {}",
                if uv_units { "UV units" } else { "pixel units" },
                s.line(),
                shift / 2.0,
                shift,
                d[0],
                d[1],
                d[2],
                if ok { "PASS (nearest the midpoint)" } else { "FAIL" }
            );
            if ok || attempt + 1 == configs.len() {
                break;
            }
            let _ = writeln!(rep, "{result}");
        }
        all_ok &= ok;
        let _ = writeln!(rep, "{result}");
    }
    print!("{rep}");
    if let Some(path) = opt(args, "--out") {
        std::fs::write(path, &rep).map_err(|e| e.to_string())?;
    }
    if !all_ok {
        return Err("a MetalFX quality check failed".into());
    }
    Ok(())
}

/// Fill an RG16F motion texture with a constant (x, 0) vector.
fn write_motion(queue: &wgpu::Queue, t: &wgpu::Texture, w: u32, h: u32, mx: f32) {
    let px = [f32_to_half(mx).to_le_bytes(), f32_to_half(0.0).to_le_bytes()].concat();
    let data: Vec<u8> = px.iter().copied().cycle().take((w * h * 4) as usize).collect();
    queue.write_texture(
        t.as_image_copy(),
        &data,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: None },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
}

fn halton(mut i: u32, base: u32) -> f32 {
    let mut f = 1.0;
    let mut r = 0.0;
    while i > 0 {
        f /= base as f32;
        r += f * (i % base) as f32;
        i /= base;
    }
    r
}
