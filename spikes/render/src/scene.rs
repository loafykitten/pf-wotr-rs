//! `scene`: skinned characters on Bevy 0.20 / Metal, with a pipeline-gated load, frame pacing
//! measured at the display, and GPU frame timing. Writes results to `--out DIR`.

use std::f32::consts::PI;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::light::CascadeShadowConfigBuilder;
use bevy::prelude::*;
use bevy::render::diagnostic::RenderDiagnosticsPlugin;
use bevy::render::render_resource::{CachedPipelineState, PipelineCache};
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::window::{MonitorSelection, PresentMode, WindowMode, WindowResolution};
use bevy::winit::WinitSettings;
use bevy::world_serialization::WorldInstanceReady;

use crate::gpu_timing::{GpuTimingLog, GpuTimingPlugin};
use crate::owlcat_bevy::{OwlcatInBevyPlugin, OwlcatReport};
use crate::stats::{self, Summary};
use crate::{flag, opt, present};

#[derive(Resource, Debug, Clone)]
struct Args {
    count: usize,
    parts: usize,
    present: PresentMode,
    latency: u32,
    seconds: f32,
    warmup: f32,
    gate: bool,
    windowed: bool,
    variants: bool,
    refresh_hz: f64,
    shadows: bool,
    owlcat: bool,
    pipelined: bool,
    out: PathBuf,
}

impl Args {
    fn parse(a: &[String]) -> Result<Self, String> {
        let num = |name: &str, d: f64| -> Result<f64, String> {
            opt(a, name).map_or(Ok(d), |v| v.parse().map_err(|e| format!("{name}: {e}")))
        };
        let present = match opt(a, "--present").unwrap_or("fifo") {
            "fifo" => PresentMode::Fifo,
            "immediate" => PresentMode::Immediate,
            "auto-vsync" => PresentMode::AutoVsync,
            "auto-novsync" => PresentMode::AutoNoVsync,
            "mailbox" => PresentMode::Mailbox,
            other => return Err(format!("unknown present mode {other}")),
        };
        Ok(Self {
            count: num("--count", 150.0)? as usize,
            parts: num("--parts", 1.0)?.max(1.0) as usize,
            present,
            latency: num("--latency", 2.0)? as u32,
            seconds: num("--seconds", 20.0)? as f32,
            warmup: num("--warmup", 2.0)? as f32,
            gate: !flag(a, "--no-gate"),
            windowed: flag(a, "--windowed"),
            variants: !flag(a, "--no-variants"),
            refresh_hz: num("--refresh-hz", 240.0)?,
            shadows: !flag(a, "--no-shadows"),
            owlcat: flag(a, "--owlcat"),
            pipelined: !flag(a, "--no-pipelined"),
            out: PathBuf::from(opt(a, "--out").unwrap_or("results/scene")),
        })
    }
}

/// Render-world pipeline cache statistics, shared with the main world.
#[derive(Debug, Default)]
struct PipeStats {
    render_frames: u64,
    total: usize,
    ok: usize,
    pending: usize,
    err: usize,
    /// Render frames since `total` last changed.
    stable_frames: u64,
    created_total: usize,
    create_ms_total: f64,
    /// (render frame, host time s, pipelines created, ms spent in `process_queue`).
    log: Vec<(u64, f64, usize, f64)>,
}

#[derive(Resource, Clone, Default)]
struct SharedPipeStats(Arc<Mutex<PipeStats>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Phase {
    #[default]
    Loading,
    Warmup,
    Measuring,
    Done,
}

#[derive(Resource, Default)]
struct Run {
    phase: Phase,
    start_host: f64,
    load_done_host: f64,
    phase_started: f64,
    ready_instances: usize,
    expected_instances: usize,
    frame_starts: Vec<f64>,
    pipelines_at_load: usize,
    first_visible_frame: Option<u64>,
    frame_no: u64,
}

#[derive(Component)]
struct LoadingOverlay;

#[derive(Component)]
struct Ring {
    radius: f32,
    dir: f32,
}

#[derive(Resource)]
struct Animations {
    node: AnimationNodeIndex,
    graph: Handle<AnimationGraph>,
}

pub fn run(a: &[String]) -> Result<(), String> {
    let args = Args::parse(a)?;
    present::install()?;
    std::fs::create_dir_all(&args.out).map_err(|e| e.to_string())?;
    let window = Window {
        title: "render-spike scene".into(),
        present_mode: args.present,
        desired_maximum_frame_latency: std::num::NonZeroU32::new(args.latency),
        mode: if args.windowed {
            WindowMode::Windowed
        } else {
            WindowMode::BorderlessFullscreen(MonitorSelection::Primary)
        },
        resolution: WindowResolution::new(2560, 1440),
        ..default()
    };
    let gpu_log = GpuTimingLog::default();
    let args_owlcat = args.owlcat;
    let pipe = SharedPipeStats::default();
    let mut defaults = DefaultPlugins
        .set(WindowPlugin { primary_window: Some(window), ..default() })
        .set(AssetPlugin { file_path: asset_dir(), ..default() });
    if !args.pipelined {
        // Render in the same frame as the simulation: one frame less latency, no overlap.
        defaults =
            defaults.disable::<bevy::render::pipelined_rendering::PipelinedRenderingPlugin>();
    }
    let mut app = App::new();
    app.add_plugins((
        defaults,
        FrameTimeDiagnosticsPlugin::default(),
        RenderDiagnosticsPlugin,
        GpuTimingPlugin { log: gpu_log },
    ))
    .insert_resource(WinitSettings::continuous())
    .insert_resource(OwlcatReport::default())
    .insert_resource(pipe.clone())
    .insert_resource(Run { start_host: present::host_time(), ..default() })
    .insert_resource(args)
    .add_systems(Startup, setup)
    .add_systems(First, frame_start)
    .add_systems(Update, (rotate_rings, drive_phases));
    if args_owlcat {
        let report = app.world().resource::<OwlcatReport>().clone();
        app.add_plugins(OwlcatInBevyPlugin { report });
    }
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        render_app.insert_resource(pipe).add_systems(
            Render,
            (
                // After Specialize/Queue have queued this frame's pipelines and before Prepare,
                // where upscaling calls `block_on_render_pipeline` (an untimed process_queue).
                create_pipelines_timed.after(RenderSystems::Queue).before(RenderSystems::PhaseSort),
                create_pipelines_timed.in_set(RenderSystems::PrepareBindGroups),
                count_pipelines.in_set(RenderSystems::Cleanup),
            ),
        );
    }
    app.run();
    Ok(())
}

fn asset_dir() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/assets").to_string()
}

fn setup(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    args: Res<Args>,
    mut run: ResMut<Run>,
) {
    let clip =
        asset_server.load(GltfAssetLabel::Animation(2).from_asset("models/animated/Fox.glb"));
    let (graph, node) = AnimationGraph::from_clip(clip);
    commands.insert_resource(Animations { node, graph: graphs.add(graph) });
    let fox = asset_server.load(GltfAssetLabel::Scene(0).from_asset("models/animated/Fox.glb"));

    // Concentric rings, as in Bevy's many_foxes, packed at ~1.6 m spacing.
    let spacing = 1.6;
    let mut remaining = args.count;
    let mut radius = spacing;
    let mut ring_i = 0;
    while remaining > 0 {
        let dir = if ring_i % 2 == 0 { 1.0 } else { -1.0 };
        let ring = commands
            .spawn((Transform::default(), Visibility::default(), Ring { radius, dir }))
            .id();
        let n = ((2.0 * PI * radius / spacing) as usize).clamp(1, remaining);
        for i in 0..n {
            let ang = i as f32 * 2.0 * PI / n as f32;
            let (s, c) = ang.sin_cos();
            let rot = Quat::from_rotation_y(if dir > 0.0 { PI } else { 0.0 } - ang);
            for p in 0..args.parts {
                // Extra parts emulate a character built from several skinned meshes
                // (body + equipment layers), each with its own skeleton instance.
                let scale = 0.01 * (1.0 - 0.02 * p as f32);
                commands.entity(ring).with_children(|b| {
                    b.spawn((
                        WorldAssetRoot(fox.clone()),
                        Transform::from_xyz(radius * c, 0.0, radius * s)
                            .with_scale(Vec3::splat(scale))
                            .with_rotation(rot),
                    ))
                    .observe(start_animation);
                });
                run.expected_instances += 1;
            }
        }
        remaining -= n;
        radius += spacing;
        ring_i += 1;
    }

    // Wrath-like camera: perspective, high pitch, looking down at the party.
    let r = radius.max(6.0);
    commands.spawn((
        Camera3d::default(),
        Msaa::Off,
        Transform::from_xyz(0.0, r * 0.9, r * 0.9).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(r * 4.0, r * 4.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.3, 0.45, 0.3))),
    ));
    commands.spawn((
        Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.0, 1.0, -PI / 4.0)),
        DirectionalLight { shadow_maps_enabled: args.shadows, ..default() },
        CascadeShadowConfigBuilder {
            num_cascades: 2,
            first_cascade_far_bound: r,
            maximum_distance: 3.0 * r,
            ..default()
        }
        .build(),
    ));
    commands.spawn((
        PointLight { shadow_maps_enabled: args.shadows, range: 12.0, ..default() },
        Transform::from_xyz(2.0, 3.0, 2.0),
    ));

    if args.variants {
        spawn_material_variants(&mut commands, &mut meshes, &mut materials, r);
    }

    if args.gate {
        commands.spawn((
            LoadingOverlay,
            Node {
                width: percent(100),
                height: percent(100),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::BLACK),
            GlobalZIndex(i32::MAX),
            children![(Text::new("Loading (building pipelines)..."), TextColor(Color::WHITE))],
        ));
    }
}

/// A grid of meshes with different material features, so the load has to build a realistic
/// spread of pipeline variants (alpha modes, unlit, double-sided, emissive, textured).
fn spawn_material_variants(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    r: f32,
) {
    let sphere = meshes.add(Sphere::new(0.3).mesh().uv(24, 16));
    let cube = meshes.add(Cuboid::new(0.5, 0.5, 0.5));
    let alpha = [
        AlphaMode::Opaque,
        AlphaMode::Mask(0.5),
        AlphaMode::Blend,
        AlphaMode::Premultiplied,
        AlphaMode::Add,
        AlphaMode::Multiply,
        AlphaMode::AlphaToCoverage,
    ];
    let mut i = 0;
    for (ai, a) in alpha.iter().enumerate() {
        for unlit in [false, true] {
            for double_sided in [false, true] {
                let m = materials.add(StandardMaterial {
                    base_color: Color::srgba(0.8, 0.3 + 0.1 * ai as f32, 0.2, 0.6),
                    alpha_mode: *a,
                    unlit,
                    double_sided,
                    cull_mode: if double_sided {
                        None
                    } else {
                        Some(bevy::render::render_resource::Face::Back)
                    },
                    emissive: if i % 3 == 0 {
                        LinearRgba::rgb(0.5, 0.2, 0.0)
                    } else {
                        LinearRgba::BLACK
                    },
                    perceptual_roughness: 0.3 + 0.05 * (i % 10) as f32,
                    ..default()
                });
                let x = -r + 0.8 * (i % 14) as f32;
                let z = -r - 1.5 - 0.8 * (i / 14) as f32;
                let mesh = if i % 2 == 0 { sphere.clone() } else { cube.clone() };
                commands.spawn((Mesh3d(mesh), MeshMaterial3d(m), Transform::from_xyz(x, 0.4, z)));
                i += 1;
            }
        }
    }
}

fn start_animation(
    ready: On<WorldInstanceReady>,
    animations: Res<Animations>,
    mut commands: Commands,
    children: Query<&Children>,
    mut players: Query<&mut AnimationPlayer>,
    mut run: ResMut<Run>,
) {
    for child in children.iter_descendants(ready.entity) {
        if let Ok(mut player) = players.get_mut(child) {
            player.play(animations.node).repeat().seek_to(ready.entity.index_u32() as f32 / 10.0);
            commands.entity(child).insert(AnimationGraphHandle(animations.graph.clone()));
        }
    }
    run.ready_instances += 1;
}

fn rotate_rings(time: Res<Time>, mut rings: Query<(&Ring, &mut Transform)>) {
    for (ring, mut t) in &mut rings {
        t.rotate_y(ring.dir * 1.5 / ring.radius * time.delta_secs());
    }
}

fn frame_start(mut run: ResMut<Run>) {
    run.frame_no += 1;
    if run.phase == Phase::Measuring {
        run.frame_starts.push(present::host_time());
    }
}

/// Render world: build every queued pipeline here (synchronously, as Bevy does on macOS) and time
/// it. Bevy's own `process_pipeline_queue_system` then finds nothing left to do.
fn create_pipelines_timed(mut cache: ResMut<PipelineCache>, shared: Res<SharedPipeStats>) {
    let before =
        cache.pipelines().filter(|p| matches!(p.state, CachedPipelineState::Ok(_))).count();
    let t0 = std::time::Instant::now();
    cache.process_queue();
    let ms = t0.elapsed().as_secs_f64() * 1e3;
    let after = cache.pipelines().filter(|p| matches!(p.state, CachedPipelineState::Ok(_))).count();
    let created = after.saturating_sub(before);
    if let Ok(mut s) = shared.0.lock() {
        let frame = s.render_frames;
        if created > 0 {
            s.created_total += created;
            s.create_ms_total += ms;
            s.log.push((frame, present::host_time(), created, ms));
        }
    }
}

fn count_pipelines(cache: Res<PipelineCache>, shared: Res<SharedPipeStats>) {
    let (mut ok, mut pending, mut err) = (0, 0, 0);
    for p in cache.pipelines() {
        match p.state {
            CachedPipelineState::Ok(_) => ok += 1,
            // Bevy parks pipelines whose shader asset hasn't loaded yet in Err(ShaderNotLoaded)
            // and retries them: pending, not failed.
            CachedPipelineState::Err(bevy::shader::ShaderCacheError::ShaderNotLoaded(_)) => {
                pending += 1;
            }
            CachedPipelineState::Err(_) => err += 1,
            _ => pending += 1,
        }
    }
    let waiting = cache.waiting_pipelines().count();
    if let Ok(mut s) = shared.0.lock() {
        let total = ok + pending + err;
        if total != s.total || pending + waiting > 0 {
            s.stable_frames = 0;
        } else {
            s.stable_frames += 1;
        }
        s.total = total;
        s.ok = ok;
        s.pending = pending + waiting;
        s.err = err;
        s.render_frames += 1;
    }
}

#[allow(clippy::too_many_arguments)]
fn drive_phases(
    mut commands: Commands,
    mut run: ResMut<Run>,
    args: Res<Args>,
    shared: Res<SharedPipeStats>,
    overlay: Query<Entity, With<LoadingOverlay>>,
    keys: Res<ButtonInput<KeyCode>>,
    diagnostics: Res<DiagnosticsStore>,
    gpu: Res<GpuTimingLog>,
    owlcat: Res<OwlcatReport>,
    mut exit: MessageWriter<AppExit>,
) {
    let now = present::host_time();
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
    match run.phase {
        Phase::Loading => {
            let (stable, total, pending, failed) = shared
                .0
                .lock()
                .map(|s| (s.stable_frames, s.total, s.pending, s.err))
                .unwrap_or((0, 0, 1, 0));
            if failed > 0 {
                error!("{failed} pipelines failed to build");
                exit.write(AppExit::error());
                return;
            }
            let instances_ready = run.ready_instances >= run.expected_instances;
            // Gate: every character instance spawned and animating, no failed pipelines, and the
            // render world's pipeline cache has had nothing queued and no new pipelines for 30
            // frames. This covers what the load rendered; a pipeline first needed later (an
            // object that becomes visible after the load) needs an explicit pre-specialised set.
            let ready =
                !args.gate || (instances_ready && pending == 0 && stable >= 30 && total > 0);
            if ready {
                for e in &overlay {
                    commands.entity(e).despawn();
                }
                run.load_done_host = now;
                run.pipelines_at_load = total;
                run.first_visible_frame = Some(run.frame_no + 1);
                run.phase = if args.gate { Phase::Warmup } else { Phase::Measuring };
                run.phase_started = now;
                if !args.gate {
                    present::set_recording(true);
                }
                info!(
                    "load complete in {:.2}s: {} pipelines, {} instances",
                    now - run.start_host,
                    total,
                    run.ready_instances
                );
            } else if now - run.start_host > 120.0 {
                error!("load gate timed out");
                exit.write(AppExit::error());
            }
        }
        Phase::Warmup => {
            if now - run.phase_started >= f64::from(args.warmup) {
                run.phase = Phase::Measuring;
                run.phase_started = now;
                run.frame_starts.clear();
                present::set_recording(true);
                if let Ok(mut g) = gpu.0.lock() {
                    g.clear();
                }
            }
        }
        Phase::Measuring => {
            if now - run.phase_started >= f64::from(args.seconds) {
                present::set_recording(false);
                run.phase = Phase::Done;
                // Let in-flight presented handlers and GPU readbacks land.
                std::thread::sleep(Duration::from_millis(100));
                if let Err(e) = write_results(&run, &args, &shared, &diagnostics, &gpu, &owlcat) {
                    error!("writing results: {e}");
                    exit.write(AppExit::error());
                    return;
                }
                let owlcat_failed = args.owlcat
                    && owlcat.0.lock().map_or(true, |o| {
                        !o.contains("verdict:")
                            || o.contains("WARNING: a check failed")
                            || o.contains("not built")
                    });
                if owlcat_failed {
                    error!("Owlcat-in-Bevy check failed or did not run");
                    exit.write(AppExit::error());
                } else {
                    exit.write(AppExit::Success);
                }
            }
        }
        Phase::Done => {}
    }
}

fn write_results(
    run: &Run,
    args: &Args,
    shared: &SharedPipeStats,
    diagnostics: &DiagnosticsStore,
    gpu: &GpuTimingLog,
    owlcat: &OwlcatReport,
) -> Result<(), String> {
    let out = &args.out;
    let io = |e: std::io::Error| e.to_string();
    let (presented, acquired) = present::take();
    let shown: Vec<&present::Presented> =
        presented.iter().filter(|p| p.presented_time > 0.0).collect();
    let dropped = presented.len() - shown.len();
    let shown_t: Vec<f64> = shown.iter().map(|p| p.presented_time * 1e3).collect();
    let displayed_iv = stats::intervals(&shown_t);
    let frame_iv = stats::intervals(&run.frame_starts.iter().map(|t| t * 1e3).collect::<Vec<_>>());
    let acq_to_present: Vec<f64> =
        shown.iter().map(|p| (p.presented_time - p.acquired_time) * 1e3).collect();
    let gpu_frames = gpu.0.lock().map(|g| g.clone()).unwrap_or_default();
    let gpu_ts: Vec<f64> = gpu_frames.iter().map(|g| g.timestamp_ms).collect();
    let gpu_cb: Vec<f64> = gpu_frames
        .iter()
        .filter(|g| g.cb_gpu_start > 0.0 && g.cb_gpu_end > g.cb_gpu_start)
        .map(|g| (g.cb_gpu_end - g.cb_gpu_start) * 1e3)
        .collect();

    stats::write_csv(
        &out.join("frame_starts.csv"),
        "host_time_ms",
        &run.frame_starts.iter().map(|t| format!("{:.4}", t * 1e3)).collect::<Vec<_>>(),
    )
    .map_err(io)?;
    stats::write_csv(
        &out.join("presented.csv"),
        "drawable_id,acquired_ms,presented_ms",
        &presented
            .iter()
            .map(|p| {
                format!(
                    "{},{:.4},{:.4}",
                    p.drawable_id,
                    p.acquired_time * 1e3,
                    p.presented_time * 1e3
                )
            })
            .collect::<Vec<_>>(),
    )
    .map_err(io)?;
    stats::write_csv(
        &out.join("gpu_frames.csv"),
        "frame,timestamp_ms,cb_gpu_start_ms,cb_gpu_end_ms",
        &gpu_frames
            .iter()
            .map(|g| {
                format!(
                    "{},{:.4},{:.4},{:.4}",
                    g.frame,
                    g.timestamp_ms,
                    g.cb_gpu_start * 1e3,
                    g.cb_gpu_end * 1e3
                )
            })
            .collect::<Vec<_>>(),
    )
    .map_err(io)?;
    let pipe = shared.0.lock().map_err(|e| e.to_string())?;
    stats::write_csv(
        &out.join("pipelines.csv"),
        "render_frame,host_time_ms,created,process_queue_ms",
        &pipe
            .log
            .iter()
            .map(|(f, t, n, ms)| format!("{f},{:.3},{n},{ms:.3}", t * 1e3))
            .collect::<Vec<_>>(),
    )
    .map_err(io)?;

    let mut s = String::new();
    let line = |s: &mut String, name: &str, v: &[f64]| {
        let _ = match stats::summarize(v) {
            Some(x) => writeln!(s, "{name:<26} {}", x.line()),
            None => writeln!(s, "{name:<26} (no samples)"),
        };
    };
    let _ = writeln!(s, "# scene run {}", out.display());
    let _ = writeln!(s, "config: {args:?}");
    let _ = writeln!(s, "layer: {}", present::layer_info().unwrap_or_default());
    let _ = writeln!(
        s,
        "instances: {} skinned scene instances ({} characters x {} parts)",
        run.expected_instances, args.count, args.parts
    );
    let _ = writeln!(
        s,
        "load: {:.3} s to first visible frame (gate={})",
        run.load_done_host - run.start_host,
        args.gate
    );
    let _ = writeln!(
        s,
        "pipelines: {} at load, {} at end ({} ok, {} pending, {} err); timed: created {} in {:.1} ms of process_queue ({:.2} ms/pipeline); untimed (built elsewhere, e.g. upscaling's block_on): {}",
        run.pipelines_at_load,
        pipe.total,
        pipe.ok,
        pipe.pending,
        pipe.err,
        pipe.created_total,
        pipe.create_ms_total,
        pipe.create_ms_total / pipe.created_total.max(1) as f64,
        pipe.ok.saturating_sub(pipe.created_total)
    );
    let after_load: Vec<_> =
        pipe.log.iter().filter(|(_, t, _, _)| *t > run.load_done_host).collect();
    let _ = writeln!(
        s,
        "pipelines added to the cache after the first visible frame (all creation paths): {}",
        pipe.total.saturating_sub(run.pipelines_at_load)
    );
    let _ = writeln!(
        s,
        "  of which timed: {} (in {} frames, max {:.1} ms)",
        after_load.iter().map(|x| x.2).sum::<usize>(),
        after_load.len(),
        after_load.iter().map(|x| x.3).fold(0.0, f64::max)
    );
    let _ = writeln!(
        s,
        "drawables acquired while recording: {acquired}; presented handlers: {}; shown: {}; dropped (presentedTime=0): {dropped}",
        presented.len(),
        shown.len()
    );
    let _ = writeln!(s, "\n## intervals (ms)");
    line(&mut s, "displayed (presentedTime)", &displayed_iv);
    line(&mut s, "app frame (First schedule)", &frame_iv);
    line(&mut s, "acquire->displayed", &acq_to_present);
    line(&mut s, "GPU frame (timestamps)", &gpu_ts);
    line(&mut s, "GPU frame (MTLCommandBuffer)", &gpu_cb);
    for (name, v) in [("displayed", &displayed_iv), ("app frame", &frame_iv)] {
        let _ = writeln!(
            s,
            "{name}: >5.0ms {} | >8.33ms {} | >16.7ms {} | >33ms {} | >50ms {}",
            stats::count_above(v, 5.0),
            stats::count_above(v, 8.33),
            stats::count_above(v, 16.7),
            stats::count_above(v, 33.0),
            stats::count_above(v, 50.0)
        );
    }
    // Displayed intervals as multiples of the display's refresh period (vsync'd modes only):
    // 1x = a new frame every refresh, 2x = a frame shown twice, ...
    let vsynced = matches!(
        args.present,
        PresentMode::Fifo | PresentMode::AutoVsync | PresentMode::FifoRelaxed
    );
    if vsynced && !displayed_iv.is_empty() {
        let period = 1000.0 / args.refresh_hz;
        let mut hist = [0usize; 5];
        for v in &displayed_iv {
            let k = ((v / period).round() as usize).clamp(1, 5);
            hist[k - 1] += 1;
        }
        let _ = writeln!(
            s,
            "displayed vsync multiples (period {period:.3} ms): 1x {} | 2x {} | 3x {} | 4x {} | >=5x {}",
            hist[0], hist[1], hist[2], hist[3], hist[4]
        );
    }
    if let Some(d) = stats::summarize(&displayed_iv) {
        let _ = writeln!(s, "displayed rate: {:.1} Hz (from mean interval)", 1000.0 / d.mean);
    }
    let _ = writeln!(s, "\n## Bevy RenderDiagnosticsPlugin (mean over its history window)");
    let mut diag: Vec<(String, f64)> = diagnostics
        .iter()
        .filter(|d| d.path().as_str().starts_with("render/"))
        .filter_map(|d| d.average().map(|v| (d.path().as_str().to_string(), v)))
        .collect();
    diag.sort_by(|a, b| a.0.cmp(&b.0));
    let gpu_spans = diag.iter().filter(|(p, _)| p.contains("elapsed_gpu")).count();
    let _ = writeln!(s, "spans: {} ({} elapsed_gpu)", diag.len(), gpu_spans);
    for (p, v) in &diag {
        let _ = writeln!(s, "{p}: {v:.4}");
    }
    if let Ok(o) = owlcat.0.lock()
        && !o.is_empty()
    {
        let _ = writeln!(s, "\n## Owlcat passthrough inside Bevy\n{o}");
    }
    let _ = writeln!(s, "\nsummary columns: {}", Summary::CSV_HEADER);
    for (name, v) in [
        ("displayed", &displayed_iv),
        ("app_frame", &frame_iv),
        ("gpu_ts", &gpu_ts),
        ("gpu_cb", &gpu_cb),
    ] {
        if let Some(x) = stats::summarize(v) {
            let _ = writeln!(s, "{name},{}", x.csv_row());
        }
    }
    std::fs::write(out.join("summary.txt"), &s).map_err(io)?;
    print!("{s}");
    Ok(())
}
