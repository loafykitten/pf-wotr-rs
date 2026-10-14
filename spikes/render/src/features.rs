//! `features`: what wgpu 30 and the raw Metal device report on this Mac.

use std::fmt::Write as _;

use objc2_metal::{MTLCounterSamplingPoint, MTLDevice, MTLGPUFamily};

use crate::gpu;

pub fn run(out: Option<&str>) -> Result<(), String> {
    let (adapter, _device, _queue) = gpu::headless(wgpu::Features::empty())?;
    let info = adapter.get_info();
    let f = adapter.features();
    let l = adapter.limits();
    let mut s = String::new();
    let _ = writeln!(s, "# wgpu 30.0.1 adapter report");
    let _ = writeln!(s, "adapter: {} ({:?}, {:?})", info.name, info.backend, info.device_type);
    let _ = writeln!(s, "driver: {} {}", info.driver, info.driver_info);
    let want = [
        ("TIMESTAMP_QUERY", wgpu::Features::TIMESTAMP_QUERY),
        ("TIMESTAMP_QUERY_INSIDE_ENCODERS", wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS),
        ("TIMESTAMP_QUERY_INSIDE_PASSES", wgpu::Features::TIMESTAMP_QUERY_INSIDE_PASSES),
        ("PIPELINE_STATISTICS_QUERY", wgpu::Features::PIPELINE_STATISTICS_QUERY),
        ("PASSTHROUGH_SHADERS", wgpu::Features::PASSTHROUGH_SHADERS),
        ("TEXTURE_COMPRESSION_BC", wgpu::Features::TEXTURE_COMPRESSION_BC),
        ("TEXTURE_COMPRESSION_BC_SLICED_3D", wgpu::Features::TEXTURE_COMPRESSION_BC_SLICED_3D),
        ("TEXTURE_COMPRESSION_ASTC", wgpu::Features::TEXTURE_COMPRESSION_ASTC),
        ("TEXTURE_COMPRESSION_ETC2", wgpu::Features::TEXTURE_COMPRESSION_ETC2),
        ("FLOAT32_FILTERABLE", wgpu::Features::FLOAT32_FILTERABLE),
        ("DEPTH32FLOAT_STENCIL8", wgpu::Features::DEPTH32FLOAT_STENCIL8),
        ("INDIRECT_FIRST_INSTANCE", wgpu::Features::INDIRECT_FIRST_INSTANCE),
        ("MULTI_DRAW_INDIRECT_COUNT", wgpu::Features::MULTI_DRAW_INDIRECT_COUNT),
        ("IMMEDIATES", wgpu::Features::IMMEDIATES),
        ("TEXTURE_BINDING_ARRAY", wgpu::Features::TEXTURE_BINDING_ARRAY),
        ("DUAL_SOURCE_BLENDING", wgpu::Features::DUAL_SOURCE_BLENDING),
        ("SHADER_F16", wgpu::Features::SHADER_F16),
        ("EXPERIMENTAL_RAY_QUERY", wgpu::Features::EXPERIMENTAL_RAY_QUERY),
    ];
    let _ = writeln!(s, "\n## features");
    for (name, flag) in want {
        let _ = writeln!(s, "{name}: {}", f.contains(flag));
    }
    let _ = writeln!(s, "\n## limits (selected)");
    let _ = writeln!(s, "max_color_attachments: {}", l.max_color_attachments);
    let _ = writeln!(
        s,
        "max_color_attachment_bytes_per_sample: {}",
        l.max_color_attachment_bytes_per_sample
    );
    let _ = writeln!(s, "max_bind_groups: {}", l.max_bind_groups);
    let _ = writeln!(
        s,
        "max_sampled_textures_per_shader_stage: {}",
        l.max_sampled_textures_per_shader_stage
    );
    let _ = writeln!(s, "max_samplers_per_shader_stage: {}", l.max_samplers_per_shader_stage);
    let _ = writeln!(
        s,
        "max_uniform_buffers_per_shader_stage: {}",
        l.max_uniform_buffers_per_shader_stage
    );
    let _ = writeln!(s, "max_uniform_buffer_binding_size: {}", l.max_uniform_buffer_binding_size);
    let _ = writeln!(s, "max_texture_dimension_2d: {}", l.max_texture_dimension_2d);
    let _ = writeln!(s, "max_vertex_buffers: {}", l.max_vertex_buffers);
    let _ = writeln!(s, "max_immediate_size: {}", l.max_immediate_size);

    let _ = writeln!(s, "\n## BC format features");
    for fmt in [
        wgpu::TextureFormat::Bc1RgbaUnormSrgb,
        wgpu::TextureFormat::Bc3RgbaUnormSrgb,
        wgpu::TextureFormat::Bc5RgUnorm,
        wgpu::TextureFormat::Bc7RgbaUnormSrgb,
    ] {
        let ff = adapter.get_texture_format_features(fmt);
        let _ = writeln!(s, "{fmt:?}: usages={:?} flags={:?}", ff.allowed_usages, ff.flags);
    }

    // SAFETY: we only read properties from the raw MTLDevice that wgpu-hal owns; it outlives
    // the guard because `adapter` is alive for the whole block.
    unsafe {
        if let Some(hal) = adapter.as_hal::<wgpu::hal::api::Metal>() {
            let dev = hal.raw_device();
            let _ = writeln!(s, "\n## raw MTLDevice");
            let _ = writeln!(s, "name: {}", dev.name());
            for (n, fam) in [
                ("Apple7", MTLGPUFamily::Apple7),
                ("Apple8", MTLGPUFamily::Apple8),
                ("Apple9", MTLGPUFamily::Apple9),
                ("Mac2", MTLGPUFamily::Mac2),
                ("Metal3", MTLGPUFamily::Metal3),
            ] {
                let _ = writeln!(s, "supportsFamily({n}): {}", dev.supportsFamily(fam));
            }
            for (n, p) in [
                ("AtStageBoundary", MTLCounterSamplingPoint::AtStageBoundary),
                ("AtDrawBoundary", MTLCounterSamplingPoint::AtDrawBoundary),
                ("AtDispatchBoundary", MTLCounterSamplingPoint::AtDispatchBoundary),
                ("AtTileDispatchBoundary", MTLCounterSamplingPoint::AtTileDispatchBoundary),
                ("AtBlitBoundary", MTLCounterSamplingPoint::AtBlitBoundary),
            ] {
                let _ =
                    writeln!(s, "supportsCounterSampling({n}): {}", dev.supportsCounterSampling(p));
            }
            let _ =
                writeln!(s, "supportsBCTextureCompression: {}", dev.supportsBCTextureCompression());
            let _ = writeln!(s, "hasUnifiedMemory: {}", dev.hasUnifiedMemory());
            let _ = writeln!(
                s,
                "recommendedMaxWorkingSetSize: {:.1} GiB",
                dev.recommendedMaxWorkingSetSize() as f64 / (1u64 << 30) as f64
            );
            let _ = writeln!(s, "metalfx: {}", crate::metalfx::support_line(dev));
        }
    }
    let _ = writeln!(
        s,
        "\n## present modes (wgpu-hal metal/adapter.rs: Fifo always; Immediate where \
         CAMetalLayer.displaySyncEnabled exists, i.e. all current macOS). Mailbox: not offered."
    );
    print!("{s}");
    if let Some(path) = out {
        std::fs::write(path, &s).map_err(|e| format!("write {path}: {e}"))?;
        eprintln!("wrote {path}");
    }
    Ok(())
}
