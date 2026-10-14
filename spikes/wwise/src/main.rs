//! POC-0 Wwise spike runner. See README.md.
//!
//! Loads the game's `AkSoundEngine.bundle`, initialises it exactly as the game does, loads
//! the preload packages and banks, then runs four scenarios while Wwise captures its output
//! to a WAV under `cache/`:
//!
//! - A: the first voiced cue of `MeetSeelahAnevia` (`Cue_0001`), posted on the dialogue
//!   speaker object at the listener, as `LocalizedString.PlayCueVoiceOver` does;
//! - B: the next cue (`Cue_0002`, 15.6 s) on a separate emitter that sweeps from the
//!   listener's left to its right, then recedes past the event's 25 m attenuation radius;
//! - C: suspend and resume mid-cue (focus loss with the frame loop stopped), then the same
//!   with `renderAnyway`;
//! - D: shutdown in the game's order, optionally repeated (`--cycles`).
//!
//! It then analyses the capture and writes a text report.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use wwise_spike::akpk::Package;
use wwise_spike::bank_info::{BankInfo, EventInfo};
use wwise_spike::engine::{
    Callback, EngineOptions, EnginePaths, GameObjectId, SoundEngine, require_platform,
};
use wwise_spike::ffi::{AkVec3, callback_type as cbt};
use wwise_spike::proc_stats::{self, Snapshot};
use wwise_spike::settings::{self, MAC};
use wwise_spike::shortid::short_id;
use wwise_spike::sound_json::SoundPack;
use wwise_spike::wav::Capture;

/// `MeetSeelahAnevia/Cue_0001.jbp` -> `Text.m_Key` (Anevia's first line).
const CUE_KEY: &str = "37dd6a74-125f-459d-9f2c-cacf563a1bcb";
/// `MeetSeelahAnevia/Cue_0002.jbp` -> `Text.m_Key` (the next line, 15.6 s).
const MOVING_CUE_KEY: &str = "5fabe5dd-7abe-4deb-940b-48b092a6a4e7";

/// A preloaded, streamed voice line whose media has a cue marker (found by scanning the
/// preload packages' `.wem` RIFF headers for `cue ` chunks).
const MARKER_EVENT: &str = "Banter_Penta_Song_4_2";

const LISTENER: GameObjectId = 1;
const DIALOG_SPEAKER: GameObjectId = 2;
const EMITTER: GameObjectId = 3;

const FRONT: AkVec3 = AkVec3::new(0.0, 0.0, 1.0);
const UP: AkVec3 = AkVec3::new(0.0, 1.0, 0.0);
const ORIGIN: AkVec3 = AkVec3::new(0.0, 0.0, 0.0);

struct Args {
    app: PathBuf,
    out: PathBuf,
    results: PathBuf,
    device: String,
    comms: bool,
    cycles: u32,
    frame_hz: f64,
}

fn repo_root() -> PathBuf {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    p.canonicalize().unwrap_or(p)
}

fn parse_args() -> Result<Args> {
    let mut a = Args {
        app: std::env::var_os("WRATH_APP")
            .map_or_else(|| repo_root().join("builds/arm64-oracle/Wrath.app"), PathBuf::from),
        out: repo_root().join("cache/wwise-spike"),
        results: Path::new(env!("CARGO_MANIFEST_DIR")).join("results"),
        device: "No_Output".into(),
        comms: true,
        cycles: 1,
        frame_hz: 60.0,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--app" => a.app = PathBuf::from(val()?),
            "--out" => a.out = PathBuf::from(val()?),
            "--results" => a.results = PathBuf::from(val()?),
            "--device" => {
                a.device = match val()?.as_str() {
                    "default" | "game" => "default".into(),
                    "system" | "System" => "System".into(),
                    "none" | "no-output" | "No_Output" => "No_Output".into(),
                    other => bail!("unknown device {other} (default | system | no-output)"),
                }
            }
            "--no-comms" => a.comms = false,
            "--cycles" => a.cycles = val()?.parse()?,
            "--frame-hz" => a.frame_hz = val()?.parse()?,
            "-h" | "--help" => {
                println!(
                    "wwise-spike [--app Wrath.app] [--device no-output|system] [--no-comms] \
                     [--cycles N] [--frame-hz 60] [--out DIR] [--results DIR]"
                );
                std::process::exit(0);
            }
            other => bail!("unknown argument {other}"),
        }
    }
    Ok(a)
}

/// The text report, printed and saved.
#[derive(Default)]
struct Report(String);

impl Report {
    fn line(&mut self, s: impl AsRef<str>) {
        println!("{}", s.as_ref());
        self.0.push_str(s.as_ref());
        self.0.push('\n');
    }
}

macro_rules! say {
    ($r:expr, $($t:tt)*) => { $r.line(format!($($t)*)) };
}

fn mem(r: &mut Report, label: &str, s: &Snapshot) {
    say!(r, "  [{label}] resident {:.1} MiB, threads {}", s.resident_mib(), s.threads);
}

/// A logged callback with its wall-clock time since the scenario started.
#[derive(Debug, Clone)]
struct Logged {
    t: f64,
    cb: Callback,
}

/// Per-frame main-thread cost of the audio calls (`WakeupFromSuspend`, callbacks,
/// positions, `RenderAudio`), with the scenario time of each frame.
#[derive(Default)]
struct FrameCost(Vec<(f64, Duration)>);

impl FrameCost {
    fn summary(&self) -> String {
        let Some(&(worst_t, _)) = self.0.iter().max_by_key(|(_, d)| *d) else {
            return "no frames".into();
        };
        let mut v: Vec<Duration> = self.0.iter().map(|(_, d)| *d).collect();
        v.sort();
        let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize].as_secs_f64() * 1e6;
        format!(
            "{} frames, p50 {:.0} µs, p99 {:.0} µs, max {:.0} µs (at {worst_t:.2} s)",
            v.len(),
            p(0.5),
            p(0.99),
            p(1.0)
        )
    }
}

struct Frames<'e> {
    engine: &'e SoundEngine,
    period: Duration,
    start: Instant,
    next: Instant,
    log: Vec<Logged>,
    cost: FrameCost,
    monitoring: BTreeMap<String, usize>,
}

impl<'e> Frames<'e> {
    fn new(engine: &'e SoundEngine, hz: f64) -> Self {
        let now = Instant::now();
        Self {
            engine,
            period: Duration::from_secs_f64(1.0 / hz),
            start: now,
            next: now,
            log: Vec::new(),
            cost: FrameCost::default(),
            monitoring: BTreeMap::new(),
        }
    }

    fn elapsed(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// One `LateUpdate`: caller's position updates, then the game's order.
    fn frame(&mut self, update: impl FnOnce(&SoundEngine, f64) -> Result<()>) -> Result<()> {
        let t0 = Instant::now();
        let t = self.elapsed();
        update(self.engine, t)?;
        self.engine.wakeup()?;
        let mut got = Vec::new();
        self.engine.poll_callbacks(|cb| got.push(cb));
        self.engine.render_audio()?;
        self.cost.0.push((t, t0.elapsed()));
        for cb in got {
            if let Callback::Monitoring { message: Some(m), .. } = &cb {
                *self.monitoring.entry(m.clone()).or_default() += 1;
            }
            self.log.push(Logged { t, cb });
        }
        self.next += self.period;
        let now = Instant::now();
        if self.next > now {
            std::thread::sleep(self.next - now);
        } else {
            self.next = now;
        }
        Ok(())
    }

    fn ended(&self, cookie: usize) -> Option<f64> {
        self.log.iter().find_map(|l| match l.cb {
            Callback::EndOfEvent { cookie: c, .. } if c == cookie => Some(l.t),
            _ => None,
        })
    }
}

/// Ticks of `uNumSamplesPerFrame` samples since capture start -> capture seconds.
fn tick_to_capture_s(tick: u32, capture_tick0: u32) -> f64 {
    f64::from(tick.wrapping_sub(capture_tick0)) * f64::from(MAC.user.samples_per_frame)
        / f64::from(MAC.user.sample_rate)
}

fn find_event<'a>(
    info: &'a BankInfo,
    pack: &SoundPack,
    key: &str,
) -> Result<(String, &'a EventInfo)> {
    let name = pack.event_for(key).with_context(|| format!("{key} not voiced in Sound.json"))?;
    let ev = info.event(name).with_context(|| format!("{name} not in SoundbanksInfo.xml"))?;
    Ok((name.to_owned(), ev))
}

fn main() -> Result<()> {
    require_platform()?;
    let args = parse_args()?;
    let mut r = Report::default();
    let passed = run(&args, &mut r)?;
    std::fs::create_dir_all(&args.results)?;
    let name = format!("run-{}.txt", args.device.to_lowercase().replace('_', "-"));
    std::fs::write(args.results.join(&name), &r.0)?;
    println!("\nreport written to {}", args.results.join(name).display());
    ensure!(passed, "a capture check failed or was inconclusive (see the verdict line)");
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn run(args: &Args, r: &mut Report) -> Result<bool> {
    let data = args.app.join("Contents/Resources/Data");
    let streaming = data.join("StreamingAssets");
    // AkBasePathGetter: StreamingAssets + m_BasePath (slashes fixed) + platform "Mac".
    let banks_dir = streaming.join(settings::MAC.user.base_path.replace('\\', "/")).join("Mac");
    let paths = EnginePaths {
        library: args
            .app
            .join("Contents/PlugIns/AkSoundEngine.bundle/Contents/MacOS/AkSoundEngine"),
        plugin_dir: args.app.join("Contents/Plugins"),
        sound_banks: banks_dir.clone(),
        persistent: args.out.join("persistent"),
        decoded_banks: args.out.join("DecodedBanks"),
    };
    std::fs::create_dir_all(&args.out)?;
    // "default" keeps the game's setting: an empty shareset, i.e. ID 0 (the system device).
    let shareset = (args.device != "default").then(|| args.device.clone());
    let shareset_id = shareset.as_deref().map_or(0, short_id);
    let opts = EngineOptions { device_shareset: shareset, init_comms: args.comms };

    say!(r, "# Wwise spike run ({})", args.device);
    say!(r, "app: {}", args.app.display());
    say!(
        r,
        "device shareset: {} (id {}), comms: {}, frame rate: {} Hz",
        args.device,
        shareset_id,
        args.comms,
        args.frame_hz
    );

    // --- Pure lookups: cue -> event -> bank, media, package.
    let t = Instant::now();
    let pack = SoundPack::load(&streaming.join("Localization/Sound.json"))?;
    let info = BankInfo::load(&banks_dir.join("SoundbanksInfo.xml"))?;
    say!(r, "\n## Lookups ({:.0} ms)", t.elapsed().as_secs_f64() * 1e3);
    say!(
        r,
        "Sound.json: {} voiced keys; SoundbanksInfo.xml: {} events in {} banks",
        pack.len(),
        info.event_count(),
        info.banks().len()
    );
    let (cue, cue_ev) = find_event(&info, &pack, CUE_KEY)?;
    let (moving, moving_ev) = find_event(&info, &pack, MOVING_CUE_KEY)?;
    let mut packages = Vec::new();
    for name in settings::PRELOAD_PACKAGES {
        packages.push((*name, Package::load(&banks_dir.join("Packages").join(name))?));
    }
    for (key, name, ev) in [(CUE_KEY, &cue, cue_ev), (MOVING_CUE_KEY, &moving, moving_ev)] {
        say!(
            r,
            "{key} -> {name} (id {}, short_id {}), banks {:?}, {:?} {:.3} s, max attenuation {:?} m, streamed {:?}",
            ev.id,
            short_id(name),
            ev.banks,
            ev.duration_type.as_deref().unwrap_or("?"),
            ev.duration_max.unwrap_or(f32::NAN),
            ev.max_attenuation,
            ev.streamed_files
        );
        for bank in &ev.banks {
            let holder =
                packages.iter().find(|(_, p)| p.bank(short_id(bank)).is_some()).map(|(n, _)| *n);
            say!(
                r,
                "  bank {bank} ({}) is in {}",
                short_id(bank),
                holder.unwrap_or("no preload package")
            );
        }
        for media in &ev.streamed_files {
            let holder =
                packages.iter().find_map(|(n, p)| p.streamed_file(*media).map(|e| (*n, *e)));
            match holder {
                Some((n, e)) => {
                    say!(r, "  media {media} is in {n} at offset {} ({} bytes)", e.offset(), e.size)
                }
                None => say!(r, "  media {media} is in no preload package"),
            }
        }
    }

    // --- Load and init.
    say!(r, "\n## Startup");
    let base = proc_stats::snapshot();
    mem(r, "before dlopen", &base);
    // SAFETY: `paths.library` is the game's AkSoundEngine binary; `ffi` declares the
    // signatures of Wwise 2019.2.15, and the version is printed and checked below.
    let mut engine = unsafe { SoundEngine::load(&paths.library)? };
    ensure!(engine.version().0 == 2019, "unexpected Wwise version {:?}", engine.version());
    let v = engine.version();
    say!(
        r,
        "Wwise SDK {}.{}.{} build {} ({} symbols resolved)",
        v.0,
        v.1,
        v.2,
        v.3,
        wwise_spike::ffi::Api::SYMBOLS
    );
    mem(r, "after dlopen", &proc_stats::snapshot());
    engine.init(&MAC, &paths, &opts)?;
    let after_init = proc_stats::snapshot();
    mem(r, "after Init + Init.bnk", &after_init);
    say!(r, "  threads: {}", proc_stats::describe_threads());
    if let Some(l) = engine.layout {
        say!(r, "native AkInitializationSettings layout (offsets from SWIG getters): {l:?}");
    }
    say!(r, "settings: field = Wwise default -> applied (game value)");
    for ((k, d), (_, a)) in engine.defaults.fields.iter().zip(&engine.applied.fields) {
        say!(r, "  {k} = {d} -> {a}{}", if d == a { "" } else { "   *" });
    }
    for (what, res) in &engine.init_log {
        say!(r, "  {what}: {res}");
    }
    say!(r, "current language: {:?}", engine.current_language());
    let pk = engine.load_packages(&banks_dir, settings::PRELOAD_PACKAGES)?;
    say!(
        r,
        "loaded {} preload packages (ids {:?})",
        pk.len(),
        pk.iter().map(|p| p.1).collect::<Vec<_>>()
    );
    // DefaultListener.Register: AudioObject registration + position, then default listener.
    engine.register(LISTENER, "DefaultListener")?;
    engine.set_position(LISTENER, ORIGIN, FRONT, UP)?;
    engine.set_default_listeners(&[LISTENER])?;
    for b in settings::PRELOAD_BANKS {
        let id = engine.load_bank(b)?;
        ensure!(id == short_id(b), "bank {b}: id {id} != short_id {}", short_id(b));
    }
    say!(
        r,
        "loaded {} preload banks: {:?}",
        settings::PRELOAD_BANKS.len(),
        settings::PRELOAD_BANKS
    );
    let mut bank_bytes = 0u64;
    for b in settings::PRELOAD_BANKS {
        let hit = packages.iter().find_map(|(n, p)| p.bank(short_id(b)).map(|e| (*n, e.size)));
        if let Some((n, size)) = hit {
            bank_bytes += u64::from(size);
            say!(r, "  {b}: {:.1} MiB, in {n}", f64::from(size) / 1048576.0);
        }
    }
    say!(
        r,
        "  preload banks total {:.1} MiB (loaded into memory; streamed media stays on disk)",
        bank_bytes as f64 / 1048576.0
    );
    let tm = engine.timings;
    say!(
        r,
        "timings: dlopen {:.1} ms, engine init {:.1} ms, packages {:.1} ms, banks {:.1} ms, total {:.1} ms",
        tm.load.as_secs_f64() * 1e3,
        tm.engine.as_secs_f64() * 1e3,
        tm.packages.as_secs_f64() * 1e3,
        tm.banks.as_secs_f64() * 1e3,
        (tm.load + tm.engine + tm.packages + tm.banks).as_secs_f64() * 1e3
    );
    let loaded = proc_stats::snapshot();
    mem(r, "after packages + banks", &loaded);
    for name in [cue.as_str(), "Dialogs_Prologue_ENG", "Wrath_Main_VO_Dialogues_Prologue.pck"] {
        ensure!(
            engine.id_from_string(name) == short_id(name),
            "GetIDFromString({name}) disagrees with short_id"
        );
    }
    say!(r, "GetIDFromString agrees with our FNV short_id");
    say!(r, "Wwise images loaded: {:?}", proc_stats::loaded_images("Ak"));
    say!(r, "output channels (count, mask): {:?}", engine.output_channels());
    for e in SoundEngine::take_native_errors() {
        say!(r, "  native error: {e}");
    }

    // --- Capture.
    // Captures are written relative to the most recently added base path, so put ours first.
    let capture_dir = args.out.join("captures");
    std::fs::create_dir_all(&capture_dir)?;
    let capture_name = format!("capture-{}.wav", args.device.to_lowercase().replace('_', "-"));
    let capture = capture_dir.join(&capture_name);
    let _ = std::fs::remove_file(&capture);
    engine.add_base_path(&capture_dir)?;
    engine.start_output_capture(&capture_name)?;
    let capture_tick0 = engine.buffer_tick();
    engine.start_resource_monitoring();

    // The 2D dialogue object (DefaultListener.DialogSpeaker, a child at local origin).
    engine.register(DIALOG_SPEAKER, "DialogSpeaker")?;
    engine.set_position(DIALOG_SPEAKER, ORIGIN, FRONT, UP)?;
    // SoundUtility.SetGenderFlags / SetRaceFlags. Manual_13's character isn't loaded here;
    // these values only steer PC barks, not companion VO.
    engine.set_switch("CharacterTitle", "Baron", DIALOG_SPEAKER)?;
    engine.set_switch("CharacterSex", "Female", DIALOG_SPEAKER)?;
    engine.register(EMITTER, "Seelah (moving emitter)")?;
    engine.set_position(EMITTER, AkVec3::new(-8.0, 0.0, 2.0), FRONT, UP)?;

    let mut f = Frames::new(&engine, args.frame_hz);
    let cpu0 = proc_stats::snapshot();
    let mut peak_wwise_cpu = 0f32;
    let mut peak_voices = 0;
    let mut peak_rss = loaded.resident;

    // --- A: the dialogue cue, posted like PlayCueVoiceOver (plus marker/duration flags).
    say!(r, "\n## A: voiced cue {cue} on the dialogue speaker");
    let flags =
        cbt::END_OF_EVENT | cbt::MARKER | cbt::DURATION | cbt::ENABLE_GET_SOURCE_PLAY_POSITION;
    let a_tick = engine.buffer_tick();
    let a_t = f.elapsed();
    let a_id = engine.post_event(&cue, DIALOG_SPEAKER, flags, 1);
    ensure!(a_id != 0, "PostEvent({cue}) returned 0");
    let mut positions = Vec::new();
    while f.ended(1).is_none() && f.elapsed() - a_t < 12.0 {
        f.frame(|_, _| Ok(()))?;
        if let Some(p) = engine.source_play_position(a_id, true) {
            positions.push((f.elapsed() - a_t, p));
        }
    }
    let a_end = f.ended(1).context("no EndOfEvent for cue A within 12 s")?;
    say!(
        r,
        "playing id {a_id}; EndOfEvent after {:.3} s wall clock (SoundbanksInfo: {:.3} s)",
        a_end - a_t,
        cue_ev.duration_max.unwrap_or(0.0)
    );
    if let (Some(first), Some(last)) = (positions.first(), positions.last()) {
        say!(
            r,
            "GetSourcePlayPosition: {} samples, first {} ms at {:.3} s, last {} ms at {:.3} s",
            positions.len(),
            first.1,
            first.0,
            last.1,
            last.0
        );
    }
    let a_capture_start = tick_to_capture_s(a_tick, capture_tick0);

    // --- A2: marker callbacks. No prologue VO media carries cue markers (0 of 233 in
    // Wrath_Main_VO_Dialogues_Prologue.pck), so use a preloaded banter line whose single
    // streamed .wem has one.
    say!(r, "\n## A2: marker callback on {MARKER_EVENT}");
    let m_tick = engine.buffer_tick();
    let m_t = f.elapsed();
    let m_id = engine.post_event(
        MARKER_EVENT,
        DIALOG_SPEAKER,
        cbt::END_OF_EVENT | cbt::MARKER | cbt::DURATION,
        4,
    );
    ensure!(m_id != 0, "PostEvent({MARKER_EVENT}) returned 0");
    while f.ended(4).is_none() && f.elapsed() - m_t < 12.0 {
        f.frame(|_, _| Ok(()))?;
    }
    let m_end = f.ended(4).context("no EndOfEvent for the marker event")?;
    let markers =
        f.log.iter().filter(|l| matches!(l.cb, Callback::Marker { cookie: 4, .. })).count();
    say!(r, "EndOfEvent after {:.3} s; {markers} marker callback(s)", m_end - m_t);
    let m_capture_start = tick_to_capture_s(m_tick, capture_tick0);

    // --- B: moving 3D emitter.
    say!(r, "\n## B: moving emitter with {moving}");
    let sweep = 6.0;
    let recede = 6.0;
    let path = move |t: f64| -> AkVec3 {
        if t < sweep {
            AkVec3::new((-8.0 + 16.0 * t / sweep) as f32, 0.0, 2.0)
        } else {
            let u = ((t - sweep) / recede).min(1.0);
            AkVec3::new(8.0 * (1.0 - u) as f32, 0.0, (2.0 + 38.0 * u) as f32)
        }
    };
    for _ in 0..30 {
        f.frame(|_, _| Ok(()))?; // half a second of silence between scenarios
    }
    let b_tick = engine.buffer_tick();
    let b_t = f.elapsed();
    engine.set_position(EMITTER, path(0.0), FRONT, UP)?;
    let b_id = engine.post_event(&moving, EMITTER, cbt::END_OF_EVENT | cbt::DURATION, 2);
    ensure!(b_id != 0, "PostEvent({moving}) returned 0");
    // Positions follow audio time (buffer ticks), not wall time: the No_Output sink renders
    // faster than real time. Each entry is (capture seconds, position applied).
    let mut trajectory: Vec<(f64, AkVec3)> = Vec::new();
    while f.ended(2).is_none() && f.elapsed() - b_t < 25.0 {
        f.frame(|e, _| {
            let tick = e.buffer_tick();
            let p = path(tick_to_capture_s(tick, b_tick));
            trajectory.push((tick_to_capture_s(tick, capture_tick0), p));
            e.set_position(EMITTER, p, FRONT, UP)
        })?;
        if let Some(s) = engine.resource_summary() {
            peak_wwise_cpu = peak_wwise_cpu.max(s.total_cpu);
            peak_voices = peak_voices.max(s.physical_voices);
        }
        peak_rss = peak_rss.max(proc_stats::snapshot().resident);
    }
    let b_end = f.ended(2).context("no EndOfEvent for cue B")?;
    let b_rate = f64::from(engine.buffer_tick().wrapping_sub(b_tick)) / (f.elapsed() - b_t);
    say!(
        r,
        "EndOfEvent after {:.3} s (SoundbanksInfo: {:.3} s)",
        b_end - b_t,
        moving_ev.duration_max.unwrap_or(0.0)
    );
    say!(
        r,
        "path: x -8 -> +8 m at z = 2 m over {sweep} s (left to right), then to (0, 0, 40) over {recede} s, in audio time"
    );
    say!(
        r,
        "audio frames rendered per wall-clock second during B: {b_rate:.1} (real time = 48000/1024 = 46.9)"
    );
    let b_capture_start = tick_to_capture_s(b_tick, capture_tick0);
    let cpu1 = proc_stats::snapshot();
    say!(r, "threads after B: {}", proc_stats::describe_threads());

    // --- C: suspend / resume.
    say!(r, "\n## C: suspend and resume");
    for _ in 0..30 {
        f.frame(|_, _| Ok(()))?;
    }
    let c_t = f.elapsed();
    let c_id = engine.post_event(
        &cue,
        DIALOG_SPEAKER,
        cbt::END_OF_EVENT | cbt::ENABLE_GET_SOURCE_PLAY_POSITION,
        3,
    );
    ensure!(c_id != 0, "PostEvent({cue}) returned 0");
    while f.elapsed() - c_t < 1.0 {
        f.frame(|_, _| Ok(()))?;
    }
    let (tick_before, pos_before) =
        (engine.buffer_tick(), engine.source_play_position(c_id, false));
    // OnApplicationFocus(false) -> Suspend(RenderDuringFocusLoss = false), then the frame
    // loop stops (Unity doesn't update an unfocused player unless it runs in background).
    engine.suspend(MAC.advanced.render_during_focus_loss)?;
    engine.render_audio()?;
    let suspended_at = Instant::now();
    std::thread::sleep(Duration::from_millis(1500));
    let (tick_suspended, pos_suspended) =
        (engine.buffer_tick(), engine.source_play_position(c_id, false));
    // OnApplicationFocus(true) -> WakeupFromSuspend + RenderAudio.
    engine.wakeup()?;
    engine.render_audio()?;
    let suspend_wall = suspended_at.elapsed().as_secs_f64();
    say!(
        r,
        "Suspend(false) for {suspend_wall:.2} s: buffer tick {tick_before} -> {tick_suspended} (+{}), play position {pos_before:?} -> {pos_suspended:?} ms",
        tick_suspended.wrapping_sub(tick_before)
    );
    let resumed = f.elapsed();
    let mut pos_after = None;
    for _ in 0..12 {
        f.frame(|_, _| Ok(()))?;
        pos_after = engine.source_play_position(c_id, false).or(pos_after);
    }
    say!(r, "after WakeupFromSuspend: play position {pos_after:?} ms, 0.2 s later");
    while f.ended(3).is_none() && f.elapsed() - c_t < 15.0 {
        f.frame(|_, _| Ok(()))?;
    }
    let c_end = f.ended(3).context("no EndOfEvent for cue C")?;
    say!(
        r,
        "EndOfEvent {:.3} s after post ({:.3} s audio + {suspend_wall:.2} s suspended); resumed at {:.3} s",
        c_end - c_t,
        cue_ev.duration_max.unwrap_or(0.0),
        resumed - c_t
    );
    // Suspend(renderAnyway = true): Wwise keeps rendering (silence from the device side).
    let tick0 = engine.buffer_tick();
    engine.suspend(true)?;
    std::thread::sleep(Duration::from_millis(500));
    let tick1 = engine.buffer_tick();
    engine.wakeup()?;
    engine.render_audio()?;
    say!(
        r,
        "Suspend(true) for 0.5 s: buffer tick +{} (rendering continues)",
        tick1.wrapping_sub(tick0)
    );

    // --- Wrap up the capture and the callback log.
    engine.stop_output_capture()?;
    let summary = engine.resource_summary();
    say!(r, "\n## Callbacks (cookie: 1 = A, 4 = A2, 2 = B, 3 = C)");
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for l in &f.log {
        let k = match &l.cb {
            Callback::EndOfEvent { .. } => "EndOfEvent",
            Callback::Marker { .. } => "Marker",
            Callback::Duration { .. } => "Duration",
            Callback::Monitoring { .. } => "Monitoring",
            Callback::Other { .. } => "Other",
        };
        *kinds.entry(k.into()).or_default() += 1;
        if !matches!(l.cb, Callback::Monitoring { .. }) {
            say!(r, "  {:7.3} s  {:?}", l.t, l.cb);
        }
    }
    say!(r, "counts: {kinds:?}");
    for (m, n) in &f.monitoring {
        say!(r, "  monitoring x{n}: {m}");
    }

    // --- Cost.
    say!(r, "\n## Cost");
    say!(
        r,
        "process CPU during scenarios A+B: {:.1}% of one core (Wwise's own audio-thread estimate peaked at {peak_wwise_cpu:.1}%, {peak_voices} physical voices)",
        cpu1.cpu_percent_since(&cpu0)
    );
    say!(r, "Wwise resource summary at the end: {summary:?}");
    say!(r, "main-thread audio work per frame: {}", f.cost.summary());
    say!(
        r,
        "resident: {:.1} MiB before dlopen, {:.1} after Init, {:.1} after packages/banks, peak {:.1} during playback",
        base.resident_mib(),
        after_init.resident_mib(),
        loaded.resident_mib(),
        peak_rss as f64 / 1048576.0
    );
    say!(r, "threads: {} before dlopen, {} after Init", base.threads, after_init.threads);

    // --- D: shutdown.
    say!(r, "\n## D: shutdown");
    for id in [EMITTER, DIALOG_SPEAKER, LISTENER] {
        engine.unregister(id)?;
    }
    drop(f);
    let t = Instant::now();
    let drained = engine.shutdown();
    let after_term = proc_stats::snapshot();
    say!(
        r,
        "TerminateSoundEngine sequence: {:.1} ms, {} callbacks drained, IsInitialized = {}",
        t.elapsed().as_secs_f64() * 1e3,
        drained.len(),
        engine.is_initialized()
    );
    mem(r, "after Term", &after_term);
    say!(r, "  threads: {}", proc_stats::describe_threads());
    for e in SoundEngine::take_native_errors() {
        say!(r, "  native error: {e}");
    }
    for cycle in 2..=args.cycles {
        engine.init(&MAC, &paths, &opts)?;
        engine.load_packages(&banks_dir, settings::PRELOAD_PACKAGES)?;
        for b in settings::PRELOAD_BANKS {
            engine.load_bank(b)?;
        }
        engine.register(LISTENER, "DefaultListener")?;
        engine.set_default_listeners(&[LISTENER])?;
        let id = engine.post_event(&cue, LISTENER, cbt::END_OF_EVENT, 9);
        ensure!(id != 0, "cycle {cycle}: PostEvent failed");
        let mut f = Frames::new(&engine, args.frame_hz);
        for _ in 0..30 {
            f.frame(|_, _| Ok(()))?;
        }
        drop(f);
        engine.unregister(LISTENER)?;
        engine.shutdown();
        let s = proc_stats::snapshot();
        say!(
            r,
            "cycle {cycle}: init -> packages -> banks -> post -> 0.5 s -> term: resident {:.1} MiB, threads {}",
            s.resident_mib(),
            s.threads
        );
    }

    // --- Analyse the capture.
    say!(r, "\n## Capture analysis ({})", capture.display());
    ensure!(capture.exists(), "no capture file at {}", capture.display());
    let c = Capture::load(&capture)?;
    say!(
        r,
        "{} Hz, {} channels, {:.3} s, peak {:.1} dBFS",
        c.sample_rate,
        c.channels,
        c.duration(),
        c.peak_db()
    );
    let win = 0.1;
    let w = c.windows(win)?;
    let span_in = |from: f64, to: f64, thr: f64| {
        let a = w
            .iter()
            .find(|x| x.start >= from && x.start < to && x.level_db() > thr)
            .map(|x| x.start);
        let b = w
            .iter()
            .rev()
            .find(|x| x.start >= from && x.start < to && x.level_db() > thr)
            .map(|x| x.start + win);
        a.zip(b)
    };
    let a_span = span_in(a_capture_start, m_capture_start, -60.0);
    say!(
        r,
        "A: audible (> -60 dBFS) {:?} s, length {:.2} s; capture-relative post time {a_capture_start:.2} s",
        a_span,
        a_span.map_or(0.0, |(x, y)| y - x)
    );
    let mean =
        |v: &[f64]| if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 };
    let mut left_bal = Vec::new();
    let mut center_bal = Vec::new();
    let mut right_bal = Vec::new();
    let mut near = Vec::new();
    let mut far = Vec::new();
    let mut far_peak = 0f32;
    say!(r, "B: per 0.5 s: audio time, emitter (x, z) applied, level dBFS, balance R-L dB");
    let pos_at = |cap_s: f64| trajectory.iter().rev().find(|(c, _)| *c <= cap_s).map(|(_, p)| *p);
    let b_len = f64::from(moving_ev.duration_max.unwrap_or(15.6));
    for x in &w {
        let t = x.start - b_capture_start;
        if !(0.0..b_len).contains(&t) {
            continue;
        }
        let Some(p) = pos_at(x.start + win / 2.0) else { continue };
        let dist = p.x.hypot(p.z);
        if dist < 9.0 && x.level_db() > -90.0 {
            near.push(x.level_db()); // speech, not the pauses
        } else if dist > 30.0 {
            far.push(x.level_db()); // everything, silence included
            far_peak = far_peak.max(c.peak_between(x.start, x.start + win));
        }
        // Balance only where there's speech (pauses carry just the reverb tail).
        if x.level_db() > -45.0 && dist < 9.0 {
            if p.x < -4.0 {
                left_bal.push(x.balance_db());
            } else if p.x > 4.0 {
                right_bal.push(x.balance_db());
            } else if p.x.abs() < 1.5 {
                center_bal.push(x.balance_db());
            }
        }
        if ((t * 10.0).round() as i64) % 5 == 0 {
            say!(
                r,
                "  {t:5.1} s  ({:5.1}, {:5.1})  {:7.1}  {:+6.1}",
                p.x,
                p.z,
                x.level_db(),
                x.balance_db()
            );
        }
    }
    let p90 = |v: &[f64]| {
        let mut v = v.to_vec();
        v.sort_by(f64::total_cmp);
        v.get(((v.len() as f64 - 1.0) * 0.9).round().max(0.0) as usize).copied().unwrap_or(f64::NAN)
    };
    say!(
        r,
        "B: mean balance with emitter left (x < -4 m): {:+.1} dB ({} windows), centre (|x| < 1.5 m): {:+.1} dB ({}), right (x > 4 m): {:+.1} dB ({})",
        mean(&left_bal),
        left_bal.len(),
        mean(&center_bal),
        center_bal.len(),
        mean(&right_bal),
        right_bal.len()
    );
    say!(
        r,
        "B: 90th-percentile level within 9 m: {:.1} dBFS ({} windows); beyond 30 m (max attenuation 25 m): {:.1} dBFS ({} windows)",
        p90(&near),
        near.len(),
        p90(&far),
        far.len()
    );
    let pan_ok = left_bal.len() >= 3
        && right_bal.len() >= 3
        && mean(&left_bal) < -3.0
        && mean(&right_bal) > 3.0;
    // Attenuation needs evidence on both sides; past the 25 m radius every sample must be
    // exactly zero.
    let atten_ok = near.len() >= 10 && far.len() >= 10 && far_peak == 0.0;
    say!(
        r,
        "B: largest |sample| in the {} windows past 30 m: {far_peak:e} (exact zero = fully attenuated)",
        far.len()
    );
    let a_ok = a_span.is_some_and(|(x, y)| {
        ((y - x) - f64::from(cue_ev.duration_max.unwrap_or(0.0))).abs() < 0.5
    });
    let loud = c.peak_db() > -40.0;
    say!(
        r,
        "verdict: non-silent {loud}, A length matches {a_ok}, panning follows the source {pan_ok}, attenuation {atten_ok}"
    );
    Ok(loud && a_ok && pan_ok && atten_ok)
}
