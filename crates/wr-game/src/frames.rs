//! Frame timeline for stall hunting: timestamps of each main-world update and of the
//! render world's phases (start after extract, swapchain acquire, our recording, end after
//! present), written as `frames.csv` in bench mode. Each mark also records the calling
//! thread's CPU time and the process's context-switch and page-fault counters, so a long
//! phase can be told apart as work (CPU time grows), waiting (it doesn't; voluntary
//! switches) or preemption (involuntary switches). The cost is a few syscalls, one
//! uncontended lock and a push per mark.

use std::io::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use bevy::prelude::*;
use bevy::render::view::{ExtractedWindow, prepare_windows};
use bevy::render::{Render, RenderSystems};

/// What a timestamp marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// The main world's `Update` (our `drive` system).
    Update,
    /// The render world's schedule starts (after extract).
    RenderStart,
    /// Just before Bevy's `prepare_windows` (swapchain acquire). The acquire marks are
    /// separate systems that may run on other workers than `prepare_windows`: use their
    /// wall-time difference, not their CPU-time difference.
    AcquireStart,
    /// Just after it; `arg` is 1 when no drawable was obtained (occluded window).
    AcquireEnd,
    /// Our frame recording and submit (`record`).
    RecordStart,
    RecordEnd,
    /// The render world's `Cleanup` set: after Bevy's render graph submitted and presented.
    RenderEnd,
    /// The route point changed (`arg` = new point).
    Point,
}

#[derive(Debug, Clone, Copy)]
struct Event {
    mark: Mark,
    t_ms: f64,
    arg: u32,
    /// The calling thread's CPU time.
    cpu_ms: f64,
    /// Process-wide voluntary and involuntary context switches, minor and major faults.
    nvcsw: i64,
    nivcsw: i64,
    minflt: i64,
    majflt: i64,
    thread: u64,
    /// The calling thread's QoS class (`qos_class_t`, e.g. 0x21 user-interactive).
    qos: u32,
}

fn thread_qos() -> u32 {
    let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
    let mut prio = 0;
    // SAFETY: `pthread_self` is the calling thread; both out-pointers are writable.
    unsafe { libc::pthread_get_qos_class_np(libc::pthread_self(), &raw mut class, &raw mut prio) };
    class as u32
}

/// Raises the calling thread to the user-interactive QoS class, the class of the app's
/// main thread: the scheduler then prefers performance cores for it and lets it preempt
/// default-QoS work (other processes' builds, our own async pools).
pub fn raise_qos() {
    // SAFETY: plain call on the current thread with a valid class.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0)
    };
}

fn thread_cpu_ms() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid, writable timespec; the clock id is a constant.
    let r = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut ts) };
    if r == 0 { ts.tv_sec as f64 * 1e3 + ts.tv_nsec as f64 * 1e-6 } else { 0.0 }
}

fn rusage() -> libc::rusage {
    // SAFETY: an all-zero `rusage` is a valid value of this plain C struct.
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: `ru` is a valid, writable rusage.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &raw mut ru) };
    ru
}

fn thread_id() -> u64 {
    let mut id = 0u64;
    // SAFETY: a null thread means the calling thread; `id` is writable.
    unsafe { libc::pthread_threadid_np(0, &raw mut id) };
    id
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
/// Marks are collected only when enabled (bench mode), so other modes pay nothing.
static ENABLED: AtomicBool = AtomicBool::new(false);

/// Starts collecting marks, with room for about `frames` frames (8 marks each).
pub fn enable(frames: usize) {
    if let Ok(mut e) = EVENTS.lock() {
        e.reserve(frames.saturating_mul(8).min(1 << 27));
    }
    ENABLED.store(true, Ordering::Relaxed);
}

/// Records `mark` now.
pub fn mark(mark: Mark, arg: u32) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let t_ms = crate::since_start() * 1e3;
    let cpu_ms = thread_cpu_ms();
    let ru = rusage();
    let ev = Event {
        mark,
        t_ms,
        arg,
        cpu_ms,
        nvcsw: ru.ru_nvcsw,
        nivcsw: ru.ru_nivcsw,
        minflt: ru.ru_minflt,
        majflt: ru.ru_majflt,
        thread: thread_id(),
        qos: thread_qos(),
    };
    if let Ok(mut e) = EVENTS.lock() {
        e.push(ev);
    }
}

/// An exclusive system, so it runs on the thread driving the render schedule (Bevy's
/// render thread under pipelined rendering); raises its QoS once when asked.
fn render_start(world: &mut World) {
    if let Some(mut q) = world.get_resource_mut::<RaiseRenderQos>()
        && q.0
    {
        raise_qos();
        q.0 = false;
    }
    mark(Mark::RenderStart, 0);
}

/// Raise the render thread's QoS on the next render frame.
#[derive(Resource, Debug)]
pub struct RaiseRenderQos(pub bool);

fn acquire_start() {
    mark(Mark::AcquireStart, 0);
}

fn acquire_end(windows: Query<&ExtractedWindow>) {
    let occluded = windows.iter().any(|w| w.swap_chain_texture.is_none());
    mark(Mark::AcquireEnd, u32::from(occluded));
}

fn render_end() {
    mark(Mark::RenderEnd, 0);
}

/// Adds the render-world marks to the render app.
pub fn add_render_marks(render_app: &mut SubApp) {
    render_app.add_systems(
        Render,
        (
            render_start.in_set(RenderSystems::ExtractCommands),
            acquire_start.in_set(RenderSystems::PrepareViews).before(prepare_windows),
            acquire_end.in_set(RenderSystems::PrepareViews).after(prepare_windows),
            render_end.in_set(RenderSystems::Cleanup),
        ),
    );
}

/// Writes the timeline as CSV (`mark,t_ms,arg,cpu_ms,…`), and returns a summary of render frames:
/// per frame, the time from render start to end and how long the acquire took.
pub fn write(path: &std::path::Path) -> Option<serde_json::Value> {
    let events = EVENTS.lock().ok()?.clone();
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).ok()?);
    let _ = writeln!(f, "mark,t_ms,arg,cpu_ms,nvcsw,nivcsw,minflt,majflt,thread,qos");
    for e in &events {
        let _ = writeln!(
            f,
            "{:?},{:.4},{},{:.4},{},{},{},{},{},{:#x}",
            e.mark, e.t_ms, e.arg, e.cpu_ms, e.nvcsw, e.nivcsw, e.minflt, e.majflt, e.thread, e.qos
        );
    }
    // Summaries: acquire durations and occluded frames.
    let mut acquire = Vec::new();
    let mut occluded = 0usize;
    let mut start: Option<f64> = None;
    for e in &events {
        match e.mark {
            Mark::AcquireStart => start = Some(e.t_ms),
            Mark::AcquireEnd => {
                if let Some(s) = start.take() {
                    acquire.push(e.t_ms - s);
                }
                occluded += e.arg as usize;
            }
            _ => {}
        }
    }
    let max = acquire.iter().copied().fold(0.0, f64::max);
    Some(serde_json::json!({
        "events": events.len(),
        "acquires": acquire.len(),
        "acquire_max_ms": max,
        "acquire_over_33ms": acquire.iter().filter(|x| **x >= 33.0).count(),
        "occluded_frames": occluded,
    }))
}
