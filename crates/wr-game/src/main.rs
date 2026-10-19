//! `wr-game`: POC-6 first light. Loads an area pack behind a load gate (every texture,
//! buffer, pipeline and bind group is created before the first frame), draws it through
//! Owlcat's own shaders from the camera poses the original recorded on the Arbiter
//! route, and either captures each point (`--mode shots`) or measures frame times
//! (`--mode bench`).
//!
//! ```text
//! wr-game --route fixtures/captures/poc0/oracle-2/route-pass1.json \
//!         --out fixtures/captures/poc6/first-light [--mode shots|bench|view]
//!         [--pack cache/poc6/packs/prologue_caves_1.wrpack] [--windowed]
//!         [--present fifo|immediate] [--settle 10] [--dwell 37.5] [--point N]
//! ```
//!
//! `--mode anim` (POC-7 step 2) captures an image sequence of the units animating, from the first
//! point or `--look`: `--frames N --fps F [--t0 SECONDS]` frames at `t0 + k / F` seconds of
//! animation time, and `--walk UNIT[,SPEED[,START[,DISTANCE]]]` sends one unit (by its id in the
//! spec) on a straight walk (default 3.3 m/s from t = 1 s for 6 m).

mod frames;
mod owlcat;
mod route;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::RenderApp;
use bevy::render::render_asset::RenderAssets;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::renderer::{RenderDevice, RenderGraph, RenderGraphSystems, RenderQueue};
use bevy::render::texture::GpuImage;
use bevy::window::{MonitorSelection, PresentMode, WindowMode, WindowResolution};
use bevy::winit::WinitSettings;

use owlcat::Renderer;
use route::RoutePoint;

static START: OnceLock<Instant> = OnceLock::new();

fn since_start() -> f64 {
    START.get().map_or(0.0, |s| s.elapsed().as_secs_f64())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Shots,
    Bench,
    View,
    Anim,
}

#[derive(Debug, Clone)]
struct Args {
    pack: PathBuf,
    route: PathBuf,
    out: PathBuf,
    mode: Mode,
    windowed: bool,
    present: PresentMode,
    settle: u32,
    dwell: f64,
    point: Option<usize>,
    /// `--look ex,ey,ez,tx,ty,tz`: one extra point looking from the eye at the target with
    /// the route's first lens (for units, which the recorded route doesn't frame).
    look: Option<(bevy::math::Vec3, bevy::math::Vec3)>,
    front_ccw: bool,
    /// Raise the frame's threads (render thread, compute pool) to user-interactive QoS.
    qos_interactive: bool,
    /// `--latency N`: the surface's maximum frame latency (wgpu's default when unset).
    latency: Option<u32>,
    /// `--mode anim`: frames to capture, their rate and the animation time of the first.
    frames: usize,
    fps: f32,
    t0: f32,
    walk: Option<owlcat::skinning::Walk>,
}

/// `UNIT[,SPEED[,START[,DISTANCE]]]`.
fn parse_walk(s: &str) -> Option<owlcat::skinning::Walk> {
    let mut it = s.split(',');
    let unit = it.next()?.to_owned();
    let mut num = |d: f32| it.next().map_or(Some(d), |v| v.parse().ok());
    let speed = num(3.3)?;
    let start = num(1.0)?;
    let distance = num(6.0)?;
    let ok = |x: f32| x.is_finite() && x >= 0.0;
    let fits = !unit.is_empty() && speed.is_finite() && speed > 0.0 && ok(start) && ok(distance);
    (fits && it.next().is_none()).then_some(owlcat::skinning::Walk { unit, start, speed, distance })
}

fn opt<'a>(a: &'a [String], k: &str) -> Option<&'a str> {
    a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).map(String::as_str)
}

impl Args {
    fn parse() -> Result<Self, String> {
        let a: Vec<String> = std::env::args().collect();
        let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
        // `--mode anim`'s timing: finite, a positive rate, a start in range, a bounded count.
        let number = |k: &str, d: f32| -> Result<f32, String> {
            match opt(&a, k) {
                None => Ok(d),
                Some(v) => v
                    .parse::<f32>()
                    .ok()
                    .filter(|x| x.is_finite())
                    .ok_or_else(|| format!("{k} {v}: want a finite number")),
            }
        };
        let fps = number("--fps", 12.0)?;
        let t0 = number("--t0", 0.0)?;
        if fps <= 0.0 || !(0.0..=100_000.0).contains(&t0) {
            return Err(format!(
                "--fps {fps} --t0 {t0}: want a positive rate and a start of 0 to 100000 s"
            ));
        }
        let frames = match opt(&a, "--frames") {
            None => 24,
            Some(v) => v
                .parse::<usize>()
                .ok()
                .filter(|n| (1..=100_000).contains(n))
                .ok_or_else(|| format!("--frames {v}: want 1 to 100000"))?,
        };
        let walk = match opt(&a, "--walk") {
            None => None,
            Some(v) => Some(parse_walk(v).ok_or_else(|| {
                format!("--walk {v}: want UNIT[,SPEED[,START[,DISTANCE]]] with finite, non-negative numbers")
            })?),
        };
        Ok(Self {
            pack: opt(&a, "--pack").map_or_else(
                || root.join("cache/poc6/packs/prologue_caves_1.wrpack"),
                PathBuf::from,
            ),
            route: opt(&a, "--route").map_or_else(
                || root.join("fixtures/captures/poc0/oracle-2/route-pass1.json"),
                PathBuf::from,
            ),
            out: opt(&a, "--out")
                .map_or_else(|| root.join("fixtures/captures/poc6/first-light"), PathBuf::from),
            mode: match opt(&a, "--mode").unwrap_or("shots") {
                "shots" => Mode::Shots,
                "bench" => Mode::Bench,
                "view" => Mode::View,
                "anim" => Mode::Anim,
                m => return Err(format!("unknown mode {m}")),
            },
            windowed: a.iter().any(|x| x == "--windowed"),
            present: match opt(&a, "--present").unwrap_or("fifo") {
                "immediate" => PresentMode::Immediate,
                _ => PresentMode::Fifo,
            },
            settle: opt(&a, "--settle").and_then(|v| v.parse().ok()).unwrap_or(10),
            dwell: opt(&a, "--dwell").and_then(|v| v.parse().ok()).unwrap_or(37.5),
            point: opt(&a, "--point").and_then(|v| v.parse().ok()),
            look: opt(&a, "--look").and_then(route::parse_look),
            front_ccw: opt(&a, "--front").is_none_or(|v| v != "cw"),
            qos_interactive: opt(&a, "--qos").is_none_or(|v| v != "default"),
            latency: opt(&a, "--latency").and_then(|v| v.parse().ok()),
            frames,
            fps,
            t0,
            walk,
        })
    }
}

/// State shared between the main and render worlds.
#[derive(Debug, Default)]
struct Shared {
    point: usize,
    /// Capture this point after the frame that renders it (main → render).
    capture: Option<(usize, PathBuf)>,
    /// `--mode anim`: the animation time the capture frame shows (main → render).
    anim_time: Option<f32>,
    /// Captures done (render → main).
    captured: Vec<(usize, String, f64)>,
    first_frame_s: Option<f64>,
    load: Option<serde_json::Value>,
    error: Option<String>,
    lights_visible: Vec<(usize, usize)>,
    /// Per point: what the shadow planner did.
    shadows: Vec<(usize, owlcat::ShadowStats)>,
    missing_constants: Vec<String>,
}

#[derive(Resource, Clone)]
struct SharedRes(Arc<Mutex<Shared>>);

#[derive(Resource)]
struct Display(Handle<Image>);

#[derive(Resource)]
struct Run {
    args: Args,
    points: Vec<RoutePoint>,
    idx: usize,
    frames_here: u32,
    point_started: Option<Instant>,
    frame_times: Vec<Vec<f64>>,
    last_frame: Option<Instant>,
    waiting_capture: bool,
    /// `--mode anim`: frames captured so far.
    anim_step: usize,
}

/// Render-world state.
#[derive(Resource)]
struct RenderSide {
    renderer: Option<Renderer>,
    shared: SharedRes,
    display: AssetId<Image>,
    points: Vec<RoutePoint>,
    current: Option<usize>,
    frames: u64,
    /// Linear view of the display image (made once).
    display_view: Option<bevy::render::render_resource::TextureView>,
}

fn main() {
    START.get_or_init(Instant::now);
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let points = match route::load(&args.route) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("route: {e:#}");
            std::process::exit(2);
        }
    };
    let mut points = points;
    let mut args = args;
    if let Some((eye, target)) = args.look {
        points = vec![route::look_at(&points[0], eye, target)];
        args.point = Some(0);
    }
    let (w, h) = (points[0].pose.width, points[0].pose.height);
    let window = Window {
        title: "wr-game: Prologue_Caves_1 first light".into(),
        present_mode: args.present,
        desired_maximum_frame_latency: args.latency.and_then(std::num::NonZeroU32::new),
        mode: if args.windowed {
            WindowMode::Windowed
        } else {
            WindowMode::BorderlessFullscreen(MonitorSelection::Primary)
        },
        resolution: WindowResolution::new(w, h),
        ..default()
    };
    // Keep the display awake while we measure or capture: first light's 1.09 s stall and
    // the unpaced frames after it most likely came from the display going idle mid-run
    // (see docs/poc6/render-round2.md §1). As the original does, hold a no-display-sleep
    // assertion for the process's lifetime (`caffeinate -d
    // -w` releases it when we exit).
    let _caffeinate = (args.mode != Mode::View)
        .then(|| {
            std::process::Command::new("/usr/bin/caffeinate")
                .args(["-d", "-i", "-w", &std::process::id().to_string()])
                .spawn()
                .ok()
        })
        .flatten();
    if args.mode == Mode::Bench {
        // 240 Hz for the whole route, plus slack.
        let frames = args.dwell * points.len() as f64 * 300.0;
        frames::enable(if frames.is_finite() { (frames as usize).min(10_000_000) } else { 0 });
    }
    let shared = SharedRes(Arc::new(Mutex::new(Shared::default())));
    let mut app = App::new();
    // The compute pool runs the render world's parallel systems: give its threads the
    // main thread's QoS (see `frames::raise_qos`). `--qos default` leaves them as Bevy
    // makes them.
    let mut pools = TaskPoolOptions::default();
    if args.qos_interactive {
        pools.compute.on_thread_spawn = Some(Arc::new(frames::raise_qos));
    }
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin { primary_window: Some(window), ..default() })
            .set(TaskPoolPlugin { task_pool_options: pools }),
    )
    .insert_resource(WinitSettings::continuous())
    .insert_resource(shared.clone())
    .insert_resource(Run {
        args: args.clone(),
        points: points.clone(),
        idx: args.point.unwrap_or(0),
        frames_here: 0,
        point_started: None,
        frame_times: vec![Vec::new(); points.len()],
        last_frame: None,
        waiting_capture: false,
        anim_step: 0,
    })
    .add_systems(Startup, setup)
    .add_systems(Update, drive);
    app.add_plugins(OwlcatPlugin { shared, args, points });
    app.run();
}

/// The player's gamma setting from the capture's `general_settings.json` (next to the
/// route file), as the original applied it.
fn gamma_setting(route: &std::path::Path) -> Option<f32> {
    let j: serde_json::Value =
        serde_json::from_slice(&std::fs::read(route.parent()?.join("general_settings.json")).ok()?)
            .ok()?;
    fn find(v: &serde_json::Value) -> Option<f32> {
        match v {
            serde_json::Value::Object(m) => m.iter().find_map(|(k, x)| {
                if k.ends_with("gamma-correction-new") {
                    x.as_str().and_then(|s| s.parse().ok()).or_else(|| x.as_f64().map(|f| f as f32))
                } else {
                    find(x)
                }
            }),
            serde_json::Value::Array(a) => a.iter().find_map(find),
            _ => None,
        }
    }
    find(&j)
}

fn display_image(w: u32, h: u32) -> Image {
    let mut img = Image::new_fill(
        Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
        | TextureUsages::COPY_DST
        | TextureUsages::COPY_SRC
        | TextureUsages::RENDER_ATTACHMENT;
    // The renderer writes already-encoded sRGB bytes through a linear view.
    img.texture_descriptor.view_formats = &[TextureFormat::Rgba8Unorm];
    img
}

fn setup(mut commands: Commands, display: Res<Display>, run: Res<Run>, windows: Query<&Window>) {
    let pose = run.points[0].pose;
    let handle = display.0.clone();
    commands.spawn(Camera2d);
    let size =
        windows.iter().next().map_or(Vec2::new(pose.width as f32, pose.height as f32), |w| {
            Vec2::new(w.width(), w.height())
        });
    commands.spawn(Sprite { image: handle, custom_size: Some(size), ..default() });
}

fn percentile(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let i = ((p / 100.0) * s.len() as f64).ceil() as usize;
    s[i.clamp(1, s.len()) - 1]
}

fn summary(ms: &[f64]) -> serde_json::Value {
    serde_json::json!({
        "frames": ms.len(),
        "mean_ms": if ms.is_empty() { 0.0 } else { ms.iter().sum::<f64>() / ms.len() as f64 },
        "p50_ms": percentile(ms, 50.0), "p95_ms": percentile(ms, 95.0), "p99_ms": percentile(ms, 99.0),
        "max_ms": ms.iter().copied().fold(0.0, f64::max),
        "over_33ms": ms.iter().filter(|x| **x >= 33.0).count(),
        "over_50ms": ms.iter().filter(|x| **x >= 50.0).count(),
    })
}

fn finish(run: &Run, shared: &Shared) {
    let _ = std::fs::create_dir_all(&run.args.out);
    let all: Vec<f64> = run.frame_times.iter().flatten().copied().collect();
    let warm: Vec<f64> = run.frame_times.iter().flat_map(|v| v.iter().skip(15).copied()).collect();
    let report = serde_json::json!({
        "schema": "wr-game/first-light-run/0",
        "mode": format!("{:?}", run.args.mode),
        "pack": run.args.pack.display().to_string(),
        "route": run.args.route.display().to_string(),
        "present": format!("{:?}", run.args.present),
        "qos_interactive": run.args.qos_interactive,
        "max_frame_latency": run.args.latency,
        "load": shared.load,
        "first_frame_s": shared.first_frame_s,
        "captures": shared.captured.iter().map(|(i, p, t)| serde_json::json!({"point": i, "png": p, "at_s": t})).collect::<Vec<_>>(),
        "lights_visible": shared.lights_visible,
        "shadows": shared.shadows.iter().map(|(p, st)| serde_json::json!({
            "point": p, "candidates": st.candidates, "entries": st.entries,
            "spots": st.spots, "points": st.points, "dropped": st.dropped,
            "faces": st.faces, "caster_draws": st.draws, "tiles": st.tiles,
        })).collect::<Vec<_>>(),
        "missing_constants": shared.missing_constants,
        "frame_times": {
            "all": summary(&all),
            "after_warmup": summary(&warm),
            "per_point": run.frame_times.iter().map(|v| summary(v)).collect::<Vec<_>>(),
        },
        "error": shared.error,
        "frame_log": (run.args.mode == Mode::Bench)
            .then(|| frames::write(&run.args.out.join("frames.csv")))
            .flatten(),
    });
    let name = match run.args.mode {
        Mode::Shots => "run-shots.json",
        Mode::Bench => "run-bench.json",
        Mode::View => "run-view.json",
        Mode::Anim => "run-anim.json",
    };
    let _ = std::fs::write(
        run.args.out.join(name),
        serde_json::to_vec_pretty(&report).unwrap_or_default(),
    );
    if run.args.mode == Mode::Shots {
        write_route(run, shared);
    }
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&report["frame_times"]["all"]).unwrap_or_default()
    );
}

/// Writes our shots as a `wrath-camera-replay-v1` route (the source route with our
/// screenshots), so `oracle/tools/compare-shots` can compare them with the original.
fn write_route(run: &Run, shared: &Shared) {
    let Ok(bytes) = std::fs::read(&run.args.route) else { return };
    let Ok(mut j) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return };
    if let Some(points) = j["points"].as_array_mut() {
        for (i, p) in points.iter_mut().enumerate() {
            match shared.captured.iter().find(|c| c.0 == i) {
                Some(c) => {
                    let file = std::path::Path::new(&c.1)
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    p["screenshot"] = serde_json::Value::String(file);
                    p["screenshot_method"] = serde_json::Value::String(
                        "wr-game: display target read back after the frame".into(),
                    );
                }
                None => p["screenshot"] = serde_json::Value::Null,
            }
        }
    }
    j["profile"]["engine"] =
        serde_json::Value::String("wr-game (Rust/Bevy), POC-6 first light".into());
    let _ = std::fs::write(
        run.args.out.join("route-pass1.json"),
        serde_json::to_vec_pretty(&j).unwrap_or_default(),
    );
}

fn drive(mut run: ResMut<Run>, shared: Res<SharedRes>, mut exit: MessageWriter<AppExit>) {
    let now = Instant::now();
    frames::mark(frames::Mark::Update, run.idx as u32);
    let Ok(mut sh) = shared.0.lock() else { return };
    if let Some(e) = sh.error.clone() {
        eprintln!("render error: {e}");
        finish(&run, &sh);
        exit.write(AppExit::error());
        return;
    }
    if sh.first_frame_s.is_none() {
        return; // still behind the load gate
    }
    let n = run.points.len();
    if let Some(last) = run.last_frame
        && run.frames_here > 0
    {
        let i = run.idx;
        run.frame_times[i].push(now.duration_since(last).as_secs_f64() * 1e3);
    }
    run.last_frame = Some(now);
    sh.point = run.idx;
    run.frames_here += 1;
    match run.args.mode {
        Mode::View => {}
        Mode::Shots => {
            if run.waiting_capture {
                if sh.captured.iter().any(|c| c.0 == run.idx) {
                    run.waiting_capture = false;
                    if run.args.point.is_some() || run.idx + 1 >= n {
                        finish(&run, &sh);
                        exit.write(AppExit::Success);
                        return;
                    }
                    run.idx += 1;
                    run.frames_here = 0;
                }
            } else if run.frames_here >= run.args.settle {
                let p = &run.points[run.idx];
                let path = run.args.out.join(format!("point{}.png", p.sample));
                sh.capture = Some((run.idx, path));
                run.waiting_capture = true;
            }
        }
        Mode::Anim => {
            if run.waiting_capture {
                if sh.captured.len() > run.anim_step {
                    run.waiting_capture = false;
                    run.anim_step += 1;
                    run.frames_here = 0;
                    if run.anim_step >= run.args.frames {
                        finish(&run, &sh);
                        exit.write(AppExit::Success);
                    }
                }
            } else if run.frames_here >= run.args.settle.min(3) {
                let t = run.args.t0 + run.anim_step as f32 / run.args.fps;
                let path = run.args.out.join(format!("frame{:03}.png", run.anim_step));
                sh.anim_time = Some(t);
                sh.capture = Some((run.idx, path));
                run.waiting_capture = true;
            }
        }
        Mode::Bench => {
            let started = *run.point_started.get_or_insert(now);
            if now.duration_since(started).as_secs_f64() >= run.args.dwell {
                if run.args.point.is_some() || run.idx + 1 >= n {
                    finish(&run, &sh);
                    exit.write(AppExit::Success);
                    return;
                }
                run.idx += 1;
                run.frames_here = 0;
                run.point_started = Some(now);
            }
        }
    }
}

struct OwlcatPlugin {
    shared: SharedRes,
    args: Args,
    points: Vec<RoutePoint>,
}

impl Plugin for OwlcatPlugin {
    fn build(&self, _app: &mut App) {}

    fn finish(&self, app: &mut App) {
        let pose0 = self.points[0].pose;
        let handle = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(display_image(pose0.width, pose0.height));
        let display = handle.id();
        app.insert_resource(Display(handle));
        let Some(render_app) = app.get_sub_app_mut(RenderApp) else { return };
        let device = render_app.world().resource::<RenderDevice>().clone();
        let queue = render_app.world().resource::<RenderQueue>().clone();
        let pose = self.points[0].pose;
        let t = Instant::now();
        let gamma = gamma_setting(&self.args.route);
        let renderer = match Renderer::new(
            device.wgpu_device(),
            &queue,
            &self.args.pack,
            pose.width,
            pose.height,
            self.args.front_ccw,
            gamma,
        ) {
            Ok(r) => Some(r),
            Err(e) => {
                if let Ok(mut s) = self.shared.0.lock() {
                    s.error = Some(format!("{e:#}"));
                }
                None
            }
        };
        let mut renderer = renderer;
        if let Some(r) = renderer.as_mut() {
            let ids = r.animated_units();
            if !ids.is_empty() {
                eprintln!("animated units: {}", ids.join(" "));
            }
            r.set_walk(self.args.walk.clone());
        }
        if let (Some(r), Ok(mut s)) = (&renderer, self.shared.0.lock()) {
            let st = &r.stats;
            s.load = Some(serde_json::json!({
                "renderer_build_ms": t.elapsed().as_secs_f64() * 1e3,
                "process_start_to_ready_s": since_start(),
                "pack_open_ms": st.pack_open_ms,
                "upload_ms": st.upload_ms,
                "pipelines_and_plans_ms": st.pipelines_ms,
                "bind_groups_ms": st.bind_groups_ms,
                "pipelines": st.pipelines,
                "shader_modules": st.shader_modules,
                "gbuffer_draws": st.draws,
                "uploaded_bytes": st.uploaded_bytes,
                "skipped": st.skipped,
                "post_components": r.post_components,
                "gamma_setting": r.gamma_setting,
            }));
        }
        render_app
            .insert_resource(RenderSide {
                renderer,
                shared: self.shared.clone(),
                display,
                points: self.points.clone(),
                current: None,
                frames: 0,
                display_view: None,
            })
            .add_systems(RenderGraph, record.in_set(RenderGraphSystems::Begin));
        frames::add_render_marks(render_app);
        render_app.insert_resource(frames::RaiseRenderQos(self.args.qos_interactive));
    }
}

fn record(
    mut side: ResMut<RenderSide>,
    device: Res<RenderDevice>,
    queue: Res<RenderQueue>,
    images: Res<RenderAssets<GpuImage>>,
) {
    let Some(target) = images.get(side.display) else { return };
    let (point, capture, anim_time) = {
        let Ok(s) = side.shared.0.lock() else { return };
        (s.point, s.capture.clone(), s.anim_time)
    };
    let side = &mut *side;
    let Some(r) = side.renderer.as_mut() else { return };
    frames::mark(frames::Mark::RecordStart, 0);
    if side.current != Some(point) {
        frames::mark(frames::Mark::Point, point as u32);
        let pose = side.points[point.min(side.points.len() - 1)].pose;
        r.set_camera(&queue, &pose, 100.0);
        side.current = Some(point);
        if let Ok(mut s) = side.shared.0.lock() {
            s.lights_visible.push((point, r.lights_visible));
            s.shadows.push((point, r.shadow_stats.clone()));
            s.missing_constants = r.missing_constants();
        }
    }
    if let Some(t) = anim_time {
        // Animation capture: pose the units at `t`, then plan the frame's shadows again (a walking
        // unit's casters moved).
        r.animate(&queue, t);
        let pose = side.points[point.min(side.points.len() - 1)].pose;
        r.set_camera(&queue, &pose, 100.0 + t);
    }
    let dev = device.wgpu_device();
    let mut enc =
        dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("owlcat frame") });
    let view = side.display_view.get_or_insert_with(|| {
        target.texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(wgpu::TextureFormat::Rgba8Unorm),
            ..Default::default()
        })
    });
    r.encode(&mut enc, view);
    queue.submit([enc.finish()]);
    frames::mark(frames::Mark::RecordEnd, 0);
    side.frames += 1;
    if side.frames == 1
        && let Ok(mut s) = side.shared.0.lock()
    {
        s.first_frame_s = Some(since_start());
    }
    if let Some((cp, path)) = capture
        && cp == point
    {
        if std::env::var_os("WR_DUMP_ATLAS").is_some() {
            // Diagnostics: the shadow atlas as a 16-bit grey PNG next to the shot.
            let atlas = r.read_atlas(dev, &queue).and_then(|(n, px)| {
                let bytes: Vec<u8> = px.iter().flat_map(|v| v.to_ne_bytes()).collect();
                let p = path.with_file_name(format!("point{cp}-atlas.png"));
                image::save_buffer(&p, &bytes, n, n, image::ExtendedColorType::L16)?;
                Ok(())
            });
            if let Err(e) = atlas {
                eprintln!("atlas dump: {e:#}");
            }
        }
        let res =
            owlcat::read_rgba(dev, &queue, &target.texture, r.width, r.height).and_then(|px| {
                if let Some(d) = path.parent() {
                    std::fs::create_dir_all(d)?;
                }
                image::save_buffer(&path, &px, r.width, r.height, image::ExtendedColorType::Rgba8)?;
                Ok(())
            });
        if let Ok(mut s) = side.shared.0.lock() {
            s.capture = None;
            s.anim_time = None;
            match res {
                Ok(()) => s.captured.push((cp, path.display().to_string(), since_start())),
                Err(e) => s.error = Some(format!("capture: {e:#}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_parse_with_defaults_and_reject_nonsense() {
        let w = parse_walk("3708").unwrap();
        assert_eq!((w.speed, w.start, w.distance), (3.3, 1.0, 6.0));
        let w = parse_walk("3708,2.5,0.5,4").unwrap();
        assert_eq!((w.speed, w.start, w.distance), (2.5, 0.5, 4.0));
        for bad in ["", "u,0", "u,-1", "u,inf", "u,1,nan", "u,1,1,-3", "u,1,1,1,1", "u,x"] {
            assert!(parse_walk(bad).is_none(), "{bad}");
        }
    }
}
