//! `archive`: persist a real Owlcat pipeline across launches with `MTLBinaryArchive`, through raw
//! Metal (wgpu 30 builds its MTLRenderPipelineDescriptor internally and has no hook for
//! `binaryArchives`; its `PipelineCache` is Vulkan-only).
//!
//! `archive build` compiles the pair, creates the pipeline cold, adds it to an archive and
//! serializes it to `cache/`. `archive load` (a fresh process) loads the archive and creates the
//! pipeline with `FailOnBinaryArchiveMiss`, which errors unless the archive supplied it.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::{NSArray, NSString, NSURL};
use objc2_metal::{
    MTLBinaryArchive, MTLBinaryArchiveDescriptor, MTLCompileOptions, MTLDevice, MTLLibrary,
    MTLPipelineOption, MTLPixelFormat, MTLRenderPipelineDescriptor, MTLVertexDescriptor,
    MTLVertexFormat, MTLVertexStepFunction,
};

use crate::owlcat::default_msl_dir;
use crate::{gpu, opt};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

fn library(
    dev: &ProtocolObject<dyn MTLDevice>,
    src: &str,
) -> Result<Retained<ProtocolObject<dyn MTLLibrary>>, String> {
    let opts = MTLCompileOptions::new();
    dev.newLibraryWithSource_options_error(&NSString::from_str(src), Some(&opts))
        .map_err(|e| e.to_string())
}

/// Pipeline descriptor for an Owlcat/Lit G-buffer pair (VS blob153 inputs: position, normal,
/// tangent, uv0; six colour targets; reversed-Z depth).
fn descriptor(
    dev: &ProtocolObject<dyn MTLDevice>,
    vs: &str,
    fs: &str,
) -> Result<(Retained<MTLRenderPipelineDescriptor>, f64), String> {
    let t = Instant::now();
    let lv = library(dev, vs)?;
    let lf = library(dev, fs)?;
    let compile_ms = ms(t);
    let name = NSString::from_str("xlatMtlMain");
    let fv = lv.newFunctionWithName(&name).ok_or("no vertex fn")?;
    let ff = lf.newFunctionWithName(&name).ok_or("no fragment fn")?;
    let d = MTLRenderPipelineDescriptor::new();
    d.setVertexFunction(Some(&fv));
    d.setFragmentFunction(Some(&ff));
    let vd = MTLVertexDescriptor::new();
    // SAFETY: indices are within Metal's attribute/layout table limits (31).
    unsafe {
        for (i, fmt, off) in [
            (0usize, MTLVertexFormat::Float3, 0usize),
            (1, MTLVertexFormat::Float3, 12),
            (2, MTLVertexFormat::Float4, 24),
            (3, MTLVertexFormat::Float2, 40),
        ] {
            let a = vd.attributes().objectAtIndexedSubscript(i);
            a.setFormat(fmt);
            a.setOffset(off);
            a.setBufferIndex(30);
        }
        let l = vd.layouts().objectAtIndexedSubscript(30);
        l.setStride(48);
        l.setStepFunction(MTLVertexStepFunction::PerVertex);
    }
    d.setVertexDescriptor(Some(&vd));
    let formats = [
        MTLPixelFormat::RGBA8Unorm_sRGB,
        MTLPixelFormat::RGBA8Unorm,
        MTLPixelFormat::RGBA16Float,
        MTLPixelFormat::RG11B10Float,
        MTLPixelFormat::RGBA8Unorm,
        MTLPixelFormat::RGBA8Unorm,
    ];
    for (i, f) in formats.iter().enumerate() {
        // SAFETY: colour attachment index < 8.
        unsafe { d.colorAttachments().objectAtIndexedSubscript(i).setPixelFormat(*f) };
    }
    d.setDepthAttachmentPixelFormat(MTLPixelFormat::Depth32Float);
    Ok((d, compile_ms))
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mode = args.first().map(String::as_str).unwrap_or("build");
    let dir = opt(args, "--dir").map(PathBuf::from).unwrap_or_else(default_msl_dir);
    let salt = opt(args, "--salt").map(|s| format!("\n// salt {s}\n")).unwrap_or_default();
    let path = PathBuf::from(opt(args, "--archive").unwrap_or(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../cache/poc0-render/owlcat-lit.metalarchive"
    )));
    let vs = std::fs::read_to_string(dir.join("blob153.metal")).map_err(|e| e.to_string())? + &salt;
    let fs = std::fs::read_to_string(dir.join("blob869.metal")).map_err(|e| e.to_string())? + &salt;
    let (_a, device, _q) = gpu::headless(wgpu::Features::empty())?;
    // SAFETY: borrow of the raw device; retained for this function.
    let dev: Retained<ProtocolObject<dyn MTLDevice>> = unsafe {
        device.as_hal::<wgpu::hal::api::Metal>().ok_or("not Metal")?.raw_device().clone()
    };
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let mut rep = String::new();
    let (desc, compile_ms) = descriptor(&dev, &vs, &fs)?;
    let _ = writeln!(rep, "archive {mode}: MSL->library for VS+FS {compile_ms:.2} ms");
    match mode {
        "build" => {
            let t = Instant::now();
            dev.newRenderPipelineStateWithDescriptor_error(&desc).map_err(|e| e.to_string())?;
            let _ = writeln!(rep, "pipeline without archive: {:.2} ms", ms(t));
            let archive = dev
                .newBinaryArchiveWithDescriptor_error(&MTLBinaryArchiveDescriptor::new())
                .map_err(|e| e.to_string())?;
            let t = Instant::now();
            archive
                .addRenderPipelineFunctionsWithDescriptor_error(&desc)
                .map_err(|e| e.to_string())?;
            let _ = writeln!(rep, "addRenderPipelineFunctions: {:.2} ms", ms(t));
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            archive.serializeToURL_error(&url).map_err(|e| e.to_string())?;
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let _ = writeln!(rep, "serialized {} ({} KiB)", path.display(), size / 1024);
        }
        "load" => {
            let ad = MTLBinaryArchiveDescriptor::new();
            ad.setUrl(Some(&url));
            let t = Instant::now();
            let archive =
                dev.newBinaryArchiveWithDescriptor_error(&ad).map_err(|e| e.to_string())?;
            let _ = writeln!(rep, "load archive: {:.2} ms", ms(t));
            let arr: Retained<NSArray<ProtocolObject<dyn MTLBinaryArchive>>> =
                NSArray::from_retained_slice(&[archive]);
            desc.setBinaryArchives(Some(&arr));
            let t = Instant::now();
            let r = dev.newRenderPipelineStateWithDescriptor_options_reflection_error(
                &desc,
                MTLPipelineOption::FailOnBinaryArchiveMiss,
                None,
            );
            let _ = match r {
                Ok(_) => writeln!(
                    rep,
                    "pipeline from archive (FailOnBinaryArchiveMiss => hit): {:.2} ms",
                    ms(t)
                ),
                Err(e) => writeln!(rep, "archive MISS: {e}"),
            };
            desc.setBinaryArchives(None);
            let t = Instant::now();
            dev.newRenderPipelineStateWithDescriptor_error(&desc).map_err(|e| e.to_string())?;
            let _ = writeln!(
                rep,
                "pipeline without archive (OS Metal cache as it stands): {:.2} ms",
                ms(t)
            );
        }
        other => return Err(format!("archive mode {other}: use build|load")),
    }
    print!("{rep}");
    if let Some(out) = opt(args, "--out") {
        let mut prev = std::fs::read_to_string(out).unwrap_or_default();
        prev.push_str(&rep);
        std::fs::write(out, prev).map_err(|e| e.to_string())?;
    }
    Ok(())
}
