//! In-app GPU frame timing on Metal.
//!
//! wgpu on Apple Silicon offers `TIMESTAMP_QUERY` only at pass boundaries (Metal counter
//! sampling `AtStageBoundary`; no draw/blit-boundary sampling), so Bevy's
//! `RenderDiagnosticsPlugin` records CPU spans but no GPU spans. We bracket each frame's GPU work
//! with pass-boundary timestamps: a begin marker (a one-thread compute pass pushed before the
//! camera driver records anything) and an end marker (a render pass on the swapchain texture,
//! so Metal orders it after the last write to the drawable; see `end_marker`). The span is
//! "GPU starts this frame's work" to "drawable finished"; on a TBDR GPU the begin marker may run
//! while the previous frame is still finishing, so treat it as an upper bound on per-frame GPU
//! cost, and use Metal System Trace (`scripts/trace-scene.sh`) for per-pass busy time. As a
//! rough cross-check we also read `GPUStartTime`/`GPUEndTime` of two empty raw
//! `MTLCommandBuffer`s placed before and after the frame (not dependency-ordered either).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::renderer::{
    PendingCommandBuffers, RenderDevice, RenderGraph, RenderGraphSystems, RenderQueue,
};
use bevy::render::view::ExtractedWindow;
use block2::RcBlock;
use objc2_metal::MTLCommandBuffer;

const RING: usize = 8;

/// One frame's GPU timing.
#[derive(Debug, Clone, Copy, Default)]
pub struct GpuFrame {
    pub frame: u64,
    /// End-minus-begin of the timestamp-query bracket, ms.
    pub timestamp_ms: f64,
    /// Host time (s) when the first command buffer started on the GPU.
    pub cb_gpu_start: f64,
    /// Host time (s) when the last command buffer finished on the GPU.
    pub cb_gpu_end: f64,
}

/// Shared between the render world (writer) and the main world (reader).
#[derive(Resource, Clone, Default)]
pub struct GpuTimingLog(pub Arc<Mutex<Vec<GpuFrame>>>);

#[derive(Debug, Default, Clone, Copy, PartialEq)]
enum SlotState {
    #[default]
    Free,
    /// Markers submitted; waiting for the end command buffer to complete.
    Recorded,
    /// Resolve + copy submitted this frame; map after submission.
    Resolved,
    Mapping,
}

#[derive(Debug)]
struct Slot {
    readback: wgpu::Buffer,
    state: SlotState,
    frame: u64,
    mapped: Arc<std::sync::atomic::AtomicBool>,
    cb_start: Arc<AtomicU64>,
    cb_end: Arc<AtomicU64>,
}

#[derive(Resource)]
struct GpuTimer {
    set: wgpu::QuerySet,
    /// A 1-thread no-op compute pipeline: Metal does not sample counters for empty encoders
    /// (both timestamps read back as 0), so each marker pass dispatches it once.
    noop: wgpu::ComputePipeline,
    /// Per swapchain format: a render pipeline that draws a zero-area triangle with no colour
    /// writes, used for the end marker (see `end_marker`).
    end_pipelines: std::collections::HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    resolve: wgpu::Buffer,
    slots: Vec<Slot>,
    current: Option<usize>,
    frame: u64,
    period_ns: f64,
    log: GpuTimingLog,
}

pub struct GpuTimingPlugin {
    pub log: GpuTimingLog,
}

impl std::fmt::Debug for GpuTimingPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GpuTimingPlugin")
    }
}

impl Plugin for GpuTimingPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.log.clone());
    }

    fn finish(&self, app: &mut App) {
        let log = self.log.clone();
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        let device = render_app.world().resource::<RenderDevice>().clone();
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY) {
            warn!("TIMESTAMP_QUERY unavailable: GPU frame timing disabled");
            return;
        }
        let queue = render_app.world().resource::<RenderQueue>().clone();
        let dev = device.wgpu_device();
        let set = dev.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("spike_gpu_frame"),
            ty: wgpu::QueryType::Timestamp,
            count: (RING * 2) as u32,
        });
        let resolve = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spike_gpu_resolve"),
            size: RING as u64 * wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let slots = (0..RING)
            .map(|_| Slot {
                readback: dev.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("spike_gpu_readback"),
                    size: 16,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: SlotState::Free,
                frame: 0,
                mapped: Default::default(),
                cb_start: Default::default(),
                cb_end: Default::default(),
            })
            .collect();
        render_app
            .insert_resource(GpuTimer {
                noop: noop_pipeline(dev),
                end_pipelines: Default::default(),
                set,
                resolve,
                slots,
                current: None,
                frame: 0,
                period_ns: f64::from(queue.get_timestamp_period()),
                log,
            })
            .add_systems(
                RenderGraph,
                (
                    begin_marker.in_set(RenderGraphSystems::Begin),
                    end_marker.after(RenderGraphSystems::Render).before(RenderGraphSystems::Submit),
                    request_maps.in_set(RenderGraphSystems::Finish),
                ),
            );
    }
}

/// A raw-only command buffer (wgpu-core forbids mixing raw and wgpu encoding on one encoder)
/// whose completed handler stores its GPUStartTime or GPUEndTime (as f64 bits) into `out`.
fn timing_command_buffer(
    device: &RenderDevice,
    out: Arc<AtomicU64>,
    start: bool,
) -> wgpu::CommandBuffer {
    let mut encoder =
        device.wgpu_device().create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some(if start { "spike_cb_start" } else { "spike_cb_end" }),
        });
    // SAFETY: we only add a completion handler to the empty command buffer wgpu-hal opened for
    // this encoder; nothing else is encoded into it.
    unsafe {
        encoder.as_hal_mut::<wgpu::hal::api::Metal, _, _>(|hal| {
            let Some(cb) = hal.and_then(|h| h.raw_command_buffer()) else { return };
            let block = RcBlock::new(
                move |cb: std::ptr::NonNull<
                    objc2::runtime::ProtocolObject<dyn MTLCommandBuffer>,
                >| {
                    let cb = cb.as_ref();
                    let t = if start { cb.GPUStartTime() } else { cb.GPUEndTime() };
                    out.store(t.to_bits(), Ordering::Relaxed);
                },
            );
            cb.addCompletedHandler(RcBlock::as_ptr(&block));
        });
    }
    encoder.finish()
}

fn noop_pipeline(dev: &wgpu::Device) -> wgpu::ComputePipeline {
    let module = dev.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("spike_noop"),
        source: wgpu::ShaderSource::Wgsl("@compute @workgroup_size(1) fn main() {}".into()),
    });
    dev.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("spike_noop"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    })
}

fn marker_pass(
    encoder: &mut wgpu::CommandEncoder,
    noop: &wgpu::ComputePipeline,
    set: &wgpu::QuerySet,
    index: u32,
    begin: bool,
) {
    let writes = wgpu::ComputePassTimestampWrites {
        query_set: set,
        beginning_of_pass_write_index: begin.then_some(index),
        end_of_pass_write_index: (!begin).then_some(index),
    };
    let pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
        label: Some(if begin { "spike_gpu_begin" } else { "spike_gpu_end" }),
        timestamp_writes: Some(writes),
    });
    let mut pass = pass;
    pass.set_pipeline(noop);
    pass.dispatch_workgroups(1, 1, 1);
    drop(pass);
}

fn begin_marker(
    mut timer: ResMut<GpuTimer>,
    device: Res<RenderDevice>,
    mut pending: ResMut<PendingCommandBuffers>,
) {
    harvest(&mut timer);
    // Resolve slots whose end command buffer has completed. Resolving in the same command
    // buffer that samples the end-of-encoder counter returns 0 on Metal.
    let mut resolve_enc: Option<wgpu::CommandEncoder> = None;
    let stride = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
    for i in 0..RING {
        if timer.slots[i].state == SlotState::Recorded
            && timer.slots[i].cb_end.load(Ordering::Relaxed) != 0
        {
            let enc = resolve_enc.get_or_insert_with(|| {
                device.wgpu_device().create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("spike_gpu_resolve"),
                })
            });
            let q = (i * 2) as u32;
            enc.resolve_query_set(&timer.set, q..q + 2, &timer.resolve, i as u64 * stride);
            enc.copy_buffer_to_buffer(
                &timer.resolve,
                i as u64 * stride,
                &timer.slots[i].readback,
                0,
                16,
            );
            timer.slots[i].state = SlotState::Resolved;
        }
    }
    if let Some(enc) = resolve_enc {
        pending.push([enc.finish()]);
    }
    let frame = timer.frame;
    timer.frame += 1;
    let idx = (frame as usize) % RING;
    if timer.slots[idx].state != SlotState::Free {
        timer.current = None; // readback still in flight; skip this frame
        return;
    }
    timer.current = Some(idx);
    let mut encoder = device
        .wgpu_device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("spike_gpu_begin") });
    marker_pass(&mut encoder, &timer.noop, &timer.set, (idx * 2) as u32, true);
    let slot = &mut timer.slots[idx];
    slot.frame = frame;
    slot.cb_start.store(0, Ordering::Relaxed);
    slot.cb_end.store(0, Ordering::Relaxed);
    let start_cb = timing_command_buffer(&device, slot.cb_start.clone(), true);
    pending.push([start_cb, encoder.finish()]);
}

fn end_pipeline(dev: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
    let module = dev.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("spike_end_marker"),
        source: wgpu::ShaderSource::Wgsl(
            "@vertex fn vs() -> @builtin(position) vec4<f32> { return vec4<f32>(0.0, 0.0, 0.0, 1.0); }\n\
             @fragment fn fs() -> @location(0) vec4<f32> { return vec4<f32>(0.0); }"
                .into(),
        ),
    });
    dev.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("spike_end_marker"),
        layout: None,
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
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::empty(),
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// End of the frame bracket. An independent compute pass would *not* wait for the frame's
/// rendering (Metal only orders encoders through resource dependencies, and a trace showed such
/// a marker finishing ~0.8 ms before `upscaling`). So the end marker is a render pass on the
/// swapchain texture itself (`LoadOp::Load`, a zero-area triangle, no colour writes): Metal's
/// hazard tracking orders it after the last pass that wrote the drawable.
fn end_marker(
    mut timer: ResMut<GpuTimer>,
    device: Res<RenderDevice>,
    windows: Query<&ExtractedWindow>,
    mut pending: ResMut<PendingCommandBuffers>,
) {
    let Some(idx) = timer.current else { return };
    let mut encoder = device
        .wgpu_device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("spike_gpu_end") });
    let target = windows.iter().find_map(|w| {
        Some((w.swap_chain_texture_view.clone()?, w.swap_chain_texture_view_format?))
    });
    let q = (idx * 2 + 1) as u32;
    match target {
        Some((view, format)) => {
            timer
                .end_pipelines
                .entry(format)
                .or_insert_with(|| end_pipeline(device.wgpu_device(), format));
            let att = [Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })];
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("spike_gpu_end"),
                color_attachments: &att,
                depth_stencil_attachment: None,
                timestamp_writes: Some(wgpu::RenderPassTimestampWrites {
                    query_set: &timer.set,
                    beginning_of_pass_write_index: None,
                    end_of_pass_write_index: Some(q),
                }),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&timer.end_pipelines[&format]);
            pass.draw(0..3, 0..1);
        }
        None => marker_pass(&mut encoder, &timer.noop, &timer.set, q, false),
    }
    let end_cb = timing_command_buffer(&device, timer.slots[idx].cb_end.clone(), false);
    pending.push([encoder.finish(), end_cb]);
    timer.slots[idx].state = SlotState::Recorded;
}

fn request_maps(mut timer: ResMut<GpuTimer>) {
    timer.current = None;
    for slot in &mut timer.slots {
        if slot.state != SlotState::Resolved {
            continue;
        }
        slot.state = SlotState::Mapping;
        let flag = slot.mapped.clone();
        slot.readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            if r.is_ok() {
                flag.store(true, Ordering::Release);
            }
        });
    }
}

/// Collect finished slots into the shared log.
fn harvest(timer: &mut GpuTimer) {
    let period = timer.period_ns;
    let mut done = Vec::new();
    for slot in &mut timer.slots {
        if slot.state != SlotState::Mapping || !slot.mapped.load(Ordering::Acquire) {
            continue;
        }
        let read = {
            let Ok(view) = slot.readback.slice(..).get_mapped_range() else { continue };
            let mut w = [0u8; 8];
            w.copy_from_slice(&view[0..8]);
            let a = u64::from_le_bytes(w);
            w.copy_from_slice(&view[8..16]);
            (a, u64::from_le_bytes(w))
        };
        let (a, b) = read;
        slot.readback.unmap();
        slot.mapped.store(false, Ordering::Relaxed);
        slot.state = SlotState::Free;
        let cb_start = f64::from_bits(slot.cb_start.load(Ordering::Relaxed));
        let cb_end = f64::from_bits(slot.cb_end.load(Ordering::Relaxed));
        let ts = if b > a { (b - a) as f64 * period / 1e6 } else { f64::NAN };
        if std::env::var_os("SPIKE_TS_DEBUG").is_some() {
            eprintln!("ts raw a={a:#x} b={b:#x}");
        }
        done.push(GpuFrame {
            frame: slot.frame,
            timestamp_ms: ts,
            cb_gpu_start: cb_start,
            cb_gpu_end: cb_end,
        });
    }
    if !done.is_empty()
        && let Ok(mut log) = timer.log.0.lock()
    {
        log.extend(done);
    }
}
