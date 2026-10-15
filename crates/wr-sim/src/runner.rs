//! The headless dialogue runner: drives a hydrated world through a fixture's steps on the
//! sim clock, with the original's per-tick order, and compares the run with a WrathOracle
//! recording of the same steps.
//!
//! **Tick** (`Game.Tick` → `GameMode.Tick` → controllers, in `GameModesFactory`'s
//! registration order for the modes that matter here; decompiled, not copied:
//! `Kingmaker.GameModes/GameModesFactory.cs`, `Kingmaker/Game.cs`):
//!
//! 1. `Game.ExecuteBeforeTickActions`: `ScheduleAction`, `StartMode` and `StopMode` tasks
//!    queued since the last tick, in one FIFO (the dialogue's start, then its mode; the
//!    dialogue controller's `Start`/`Stop` run here);
//! 2. `TimeController.Tick` (a fixed 1/60 s frame, as WrathOracle's fixture mode feeds it),
//!    then the cutscene controller's recorded progress, `UnitActivatableAbilitiesController`
//!    and, outside Cutscene mode, `UnitBuffsController`;
//! 3. in Default mode, the step's script-zone input (`ScriptZoneController`'s position);
//! 4. in Dialog mode, `DialogController.Tick` (plays the scheduled cue);
//! 5. `EtudeSystemController.Tick` (`UpdateEtudes` when conditions are dirty).
//!
//! **Steps** are WrathOracle's plan steps (`oracle/mods/WrathOracle/Driver.cs`), with the
//! same waiting rules: `waitdialog`, `waitnodialog`, `answer:N`, `answerbp:GUID`,
//! `dialogrun[:G1,G2,…]` (answer until the dialogue ends: the first listed GUID shown, each
//! used once, else answer 0), `dialog:DIALOG[:UNIT]`, `wait:N`. A driver step waits up to
//! 3600 ticks; an answer is followed by one tick (the driver yields a frame). Movement is
//! T1, so the oracle's `move` is replaced here by the T0 event it causes:
//! `zone:SCRIPTZONE` (the main character enters that script zone: its trigger conditions,
//! then its enter actions, in a `ScriptZoneTriggerData` scope).
//!
//! **RNG**: the run draws from a tape made of the oracle recording's sim draws at the call
//! sites the sim models (`rng::sites::ALL`), in order; a perturbed replay's trace already
//! carries the perturbed value.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Value, json};
use wr_content::Guid;

use crate::rng::{Draw, DrawKind, DrawValue};
use crate::world::{BeforeTick, World};

/// The most ticks a waiting step waits (`Driver.WaitUntil`'s 3600 frames).
pub const MAX_WAIT: u32 = 3600;
/// How long an optional step waits (`Driver`: 120 frames).
pub const MAX_WAIT_OPTIONAL: u32 = 120;
/// Answers `dialogrun` gives before it calls the dialogue a loop.
pub const MAX_ANSWERS: u32 = 200;
/// WrathOracle's fixed frame delta.
pub const FIXED_DELTA: f32 = 1.0 / 60.0;

/// The outcome of a run.
#[derive(Debug, Default, Clone, Serialize)]
pub struct RunOutcome {
    pub ticks: i64,
    pub steps_done: usize,
    /// A step that couldn't complete (the run stops there), as the driver's `fail`.
    pub failure: Option<String>,
    /// Answers chosen, in order (GUIDs).
    pub answers: Vec<String>,
}

/// Reads a trace (`.jsonl` or `.jsonl.gz`) as JSON values.
///
/// # Errors
///
/// I/O or JSON errors.
pub fn read_trace(path: &Path) -> Result<Vec<Value>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut text = String::new();
    if path.extension().is_some_and(|e| e == "gz") {
        flate2::read::GzDecoder::new(&bytes[..]).read_to_string(&mut text)?;
    } else {
        text = String::from_utf8(bytes)?;
    }
    text.lines().filter(|l| !l.trim().is_empty()).map(|l| Ok(serde_json::from_str(l)?)).collect()
}

/// How a recorded sim draw's call site relates to the sim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteClass {
    /// A site the sim draws at (`rng::sites::ALL`): on the tape and compared.
    Modelled,
    /// A timing-coupled (T1) site, audited one by one (the same list as the oracle's
    /// `oracle-trace-diff` `AUDITED_T1_SITES`): left out, counted.
    AuditedT1,
    /// Anything else: behaviour the sim doesn't run. The comparison fails.
    Unmodelled,
}

/// `oracle/tools/oracle-trace-diff` `AUDITED_T1_SITES` (matched on `type|method`).
const AUDITED_T1_SITES: &[&str] = &[
    "Kingmaker.AreaLogic.Cutscenes.Commands.CommandDelay|OnRun(",
    "Kingmaker.AreaLogic.Cutscenes.Commands.CommandUnitPlayCutsceneAnimation|OnRun(",
    "Kingmaker.Controllers.Units.UnitAnimationController|Activate(",
    "Kingmaker.Controllers.Units.UnitRoamingController|RollNextPoint(",
    "Kingmaker.View.Roaming.RoamingWaypointData|SelectIdleTime(",
    "Kingmaker.UnitLogic.Commands.Base.UnitCommand|TickApproaching(",
    "Kingmaker.View.UnitMovementAgent|SlowDown(",
    "Kingmaker.Visual.Animation.",
];

/// The class of a site key.
#[must_use]
pub fn site_class(key: &str) -> SiteClass {
    if crate::rng::sites::ALL.iter().any(|s| s.0 == key) {
        return SiteClass::Modelled;
    }
    let parts: Vec<&str> = key.split('|').collect();
    let tm = if parts.len() >= 3 { format!("{}|{}", parts[1], parts[2]) } else { key.to_owned() };
    if AUDITED_T1_SITES.iter().any(|a| tm.starts_with(a)) {
        SiteClass::AuditedT1
    } else {
        SiteClass::Unmodelled
    }
}

fn site_keys(records: &[Value]) -> std::collections::HashMap<i64, String> {
    let mut keys = std::collections::HashMap::new();
    if let Some(b) = records.iter().find(|r| r["k"] == "begin")
        && let Some(sites) = b["rng_sites"].as_array()
    {
        for s in sites {
            if let (Some(id), Some(key)) = (s[0].as_i64(), s[1].as_str()) {
                keys.insert(id, key.to_owned());
            }
        }
    }
    keys
}

/// A recorded sim draw as the sim's [`Draw`] (site key, kind, value), or why it isn't one.
fn draw_of(
    r: &Value,
    keys: &std::collections::HashMap<i64, String>,
) -> std::result::Result<Draw, String> {
    let key = r["site"]
        .as_i64()
        .and_then(|id| keys.get(&id).cloned())
        .or_else(|| r["key"].as_str().map(str::to_owned))
        .ok_or_else(|| "draw without a site".to_owned())?;
    let a = &r["a"];
    #[allow(clippy::cast_possible_truncation)]
    let (kind, value) = match r["m"].as_str() {
        Some("Range_ii") => match (a[0].as_i64(), a[1].as_i64(), r["v"].as_i64()) {
            (Some(min), Some(max), Some(v)) => {
                (DrawKind::RangeInt { min: min as i32, max: max as i32 }, DrawValue::Int(v as i32))
            }
            _ => return Err(format!("{key}: malformed Range(int,int) draw")),
        },
        Some("Range_ff") => match (a[0].as_f64(), a[1].as_f64(), r["v"].as_f64()) {
            (Some(min), Some(max), Some(v)) => (
                DrawKind::RangeFloat { min: min as f32, max: max as f32 },
                DrawValue::Float(v as f32),
            ),
            _ => return Err(format!("{key}: malformed Range(float,float) draw")),
        },
        Some("value") => match r["v"].as_f64() {
            Some(v) => (DrawKind::Value, DrawValue::Float(v as f32)),
            None => return Err(format!("{key}: malformed value draw")),
        },
        other => return Err(format!("{key}: member {other:?}")),
    };
    Ok(Draw { seq: 0, site: key, kind, value })
}

/// The tape for the sim: the trace's sim draws at the sites the sim models, in order.
/// Draws at other sites are classified by [`oracle_stream`].
#[must_use]
pub fn tape_from_trace(records: &[Value]) -> Vec<Draw> {
    let keys = site_keys(records);
    let mut out = Vec::new();
    for r in records.iter().filter(|r| r["k"] == "draw" && r["c"] == "sim") {
        if let Ok(mut d) = draw_of(r, &keys)
            && site_class(&d.site) == SiteClass::Modelled
        {
            d.seq = out.len() as u64;
            out.push(d);
        }
    }
    out
}

fn mode_of(w: &World) -> crate::time::GameMode {
    match w.player.mode {
        "Dialog" => crate::time::GameMode::Dialog,
        "Cutscene" => crate::time::GameMode::Cutscene,
        _ => crate::time::GameMode::Default,
    }
}

/// `Game.ExecuteBeforeTickActions`: the queue in order, including tasks the tasks add
/// (the C# loop re-reads `Count`), then cleared.
pub fn execute_before_tick(w: &mut World) {
    let mut i = 0;
    while i < w.before_tick.len() {
        match w.before_tick[i] {
            BeforeTick::Action(a) => crate::dialog::run_scheduled_action(w, a),
            // `DoStartMode`: pushes the mode unless it is the current one. Pause starts only
            // over Default and outside turn-based combat; Default, Dialog, Cutscene and Rest
            // stop a Pause on top first.
            BeforeTick::StartMode(name) => {
                if w.player.mode == name {
                    w.log("info", format!("Game mode with type {name} already active"));
                } else if name == "Pause" && (w.combat.turn_based || w.player.mode != "Default") {
                    w.log("info", format!("Preventing starting Pause over {}", w.player.mode));
                } else {
                    if matches!(name, "Default" | "Dialog" | "Cutscene" | "Rest") {
                        while w.player.mode == "Pause" {
                            w.player.mode = w.player.modes_below.pop().unwrap_or_else(|| {
                                w.coverage
                                    .unsupported("Game.DoStopMode: the last mode on the stack");
                                "Default"
                            });
                            activate_mode(w);
                        }
                    }
                    let below = std::mem::replace(&mut w.player.mode, name);
                    w.player.modes_below.push(below);
                    activate_mode(w);
                    if name == "Dialog" {
                        crate::dialog::controller_start(w);
                    }
                }
            }
            // `DoStopMode`: pops the mode if it is the current one.
            BeforeTick::StopMode(name) => {
                if w.player.mode == name {
                    w.player.mode = w.player.modes_below.pop().unwrap_or_else(|| {
                        w.coverage.unsupported("Game.DoStopMode: the last mode on the stack");
                        "Default"
                    });
                    activate_mode(w);
                    if name == "Dialog" {
                        crate::dialog::controller_stop(w);
                    }
                } else {
                    w.log("warning", format!("Cannot stop game mode {name}"));
                }
            }
            BeforeTick::CutsceneLockMode { was_locked } => {
                update_mode_for_cutscene_lock(w, was_locked)
            }
        }
        i += 1;
    }
    w.before_tick.clear();
}

/// `GameMode.OnActivate` of the new current mode: its controllers' `Activate` (the time
/// controller's; `UnitBuffsController`'s in the modes that have it).
fn activate_mode(w: &mut World) {
    w.clock.activate();
    if matches!(w.player.mode, "Default" | "Dialog" | "Rest" | "GlobalMap") {
        crate::combat::life::buffs_controller_activate(w);
    }
}

/// `Game.StartMode(type)`: the request, refused when the mode is current and stays so after
/// the queue's starts and stops of it (the queue still holds this pass's run tasks).
pub fn start_mode(w: &mut World, name: &'static str) {
    if w.player.mode == name {
        let count = |t: BeforeTick| w.before_tick.iter().filter(|x| **x == t).count();
        if 1 + count(BeforeTick::StartMode(name)) > count(BeforeTick::StopMode(name)) {
            w.log("error", format!("Game mode with type {name} already active"));
            return;
        }
    }
    w.before_tick.push(BeforeTick::StartMode(name));
}

/// `CutsceneLock.SetCutsceneLock(lock, player)`: the guard's count, and the mode update
/// scheduled once until it runs.
pub fn set_cutscene_lock(w: &mut World, lock: bool) {
    let (count, scheduled) = w.player.cutscene_lock;
    let was_locked = count > 0;
    let count = if lock {
        count + 1
    } else if count <= 0 {
        // `CountingGuard`: an error, the count stays.
        w.log("error", "CountingGuard went negative: 0".to_owned());
        count
    } else {
        count - 1
    };
    w.player.cutscene_lock = (count, true);
    if !scheduled {
        w.before_tick.push(BeforeTick::CutsceneLockMode { was_locked });
    }
}

/// `CutsceneLock.UpdateModeForCutsceneLock(wasLocked)`: Cutscene mode starts or stops when
/// the lock changed since the request. (Rest and the global map's cutscene mode are reported.)
fn update_mode_for_cutscene_lock(w: &mut World, was_locked: bool) {
    let locked = w.player.cutscene_lock.0 > 0;
    if was_locked != locked {
        if matches!(w.player.mode, "Rest" | "GlobalMap")
            || w.player.modes_below.contains(&"GlobalMap")
        {
            w.coverage.unsupported("CutsceneLock: a lock in Rest or GlobalMap mode");
        }
        if locked {
            start_mode(w, "Cutscene");
        } else {
            w.before_tick.push(BeforeTick::StopMode("Cutscene"));
        }
    }
    w.player.cutscene_lock.1 = false;
}

/// One `Game.Tick` (see the module note); `zone` is a script-zone input for this tick. The
/// before-tick tasks run before the tick count advances, as in the oracle's records.
pub fn tick(w: &mut World, zone: Option<Guid>) {
    // 1. Before-tick tasks: scheduled actions and mode changes, in request order (a recorded
    // pause, a UI request, last).
    crate::elements::cutscene::recorded_pause(w, w.events.tick);
    execute_before_tick(w);
    w.events.tick += 1;
    // Pause mode has none of the controllers below (not even the clock).
    if w.player.mode == "Pause" {
        let t = w.events.tick - 1;
        crate::elements::cutscene::check_recorded_mode(w, t);
        crate::elements::cutscene::recorded_paused_tick(w, t);
        if zone.is_some() {
            w.coverage.unsupported("ScriptZoneController: a zone input in Pause mode");
        }
        return;
    }
    // 1b. UnitGroupsController (Default, Dialog, Cutscene; registered before the clock), with a
    // recording's group states checked after it.
    if w.combat.groups.on && matches!(w.player.mode, "Default" | "Dialog" | "Cutscene") {
        crate::combat::groups::tick(w);
        crate::combat::groups::check_recorded(w, w.events.tick - 1);
    }
    // 2. TimeController.
    let frame = crate::time::Frame {
        delta_time: FIXED_DELTA,
        unscaled_delta_time: FIXED_DELTA,
        mode: mode_of(w),
        turn_based_scale: None,
    };
    if let Err(e) = w.tick_clock(frame) {
        w.log("error", format!("TimeController: {e:?}"));
    }
    // 2b. UnitActivatableAbilitiesController (Default, Dialog, Cutscene, Rest).
    let dialog_or_cutscene = matches!(w.player.mode, "Dialog" | "Cutscene");
    // (The sim's ticks count from 1, the oracle's from 0.)
    let t = w.events.tick - 1;
    crate::elements::cutscene::check_recorded_mode(w, t);
    // 2a. CutsceneController (registered before the units' controllers): the recorded progress.
    crate::elements::cutscene::recorded_tick(w, t);
    crate::spawn::recorded_awake_for_tick(w, t);
    crate::spawn::activatable_abilities_tick(w, dialog_or_cutscene);
    if let Some(r) = w.recorded_awake.as_mut() {
        r.now = None;
    }
    // 2c. UnitBuffsController (Default, Dialog, Rest, GlobalMap; not in a cutscene). Its
    // `IsSuspended` stays false: `CommandSuspendBuffs`, which retains it, isn't a command the
    // cutscene classification accepts.
    if matches!(w.player.mode, "Default" | "Dialog" | "Rest" | "GlobalMap") {
        crate::combat::life::buffs_tick(w);
    }
    // 2d. UnitRoamingController (Default, Dialog, Cutscene, Rest): the recorded producers of
    // its choices, audited now.
    if matches!(w.player.mode, "Default" | "Dialog" | "Cutscene" | "Rest") {
        roaming_tick(w, t);
    }
    // 3. ScriptZoneController (Default and Cutscene modes).
    if let Some(z) = zone {
        if !matches!(w.player.mode, "Default" | "Cutscene") {
            w.coverage.unsupported("ScriptZoneController: a zone input outside its modes");
        }
        enter_script_zone(w, z);
    }
    // 4. DialogController (Dialog mode).
    if w.player.mode == "Dialog" {
        crate::dialog::tick(w);
    }
    // 4b. CutsceneController, without a recording of its progress: the T0 actions of the
    // cutscenes started so far, in order (their timing is T1).
    if w.recorded_cs.is_none() {
        crate::elements::cutscene::run_pending(w);
    }
    // 5. EtudeSystemController.
    crate::narrative::etudes::controller_tick(w);
    // 6. EntityCreationController, EntityDestructionController (registered after the
    // etudes' controller; the creation queue is empty unless a unit was queued after
    // the action's own `Tick`).
    crate::spawn::entity_creator_tick(w);
    crate::spawn::entity_destroyer_tick(w);
}

/// The main character enters a script zone: the oracle's T1 walk into it, as the zone's
/// tick sees it (`ScriptZone.TickUnit` → `TryAddUnit`, `crate::scene`).
pub fn enter_script_zone(w: &mut World, zone: Guid) {
    if w.content.get(zone).is_none() {
        w.coverage.unsupported("ScriptZone: blueprint missing");
        return;
    }
    crate::scene::enter_by_blueprint(w, zone);
}

/// `Driver.DialogWaitsForAnswer`.
#[must_use]
pub fn waits_for_answer(w: &World) -> bool {
    w.dialog.dialog.is_some()
        && w.dialog.current_cue.is_some()
        && !w.dialog.answers.is_empty()
        && !w.dialog.cue_play_scheduled
}

/// A dialogue is running or Dialog mode is active (`Driver.DialogRunning`).
#[must_use]
pub fn dialog_running(w: &World) -> bool {
    w.dialog.dialog.is_some() || w.player.mode == "Dialog"
}

fn wait_until(w: &mut World, cond: fn(&World) -> bool, max: u32) -> bool {
    for _ in 0..max {
        if cond(w) {
            return true;
        }
        tick(w, None);
    }
    cond(w)
}

/// Before a `dialog:` or `answer:` input, with a recording: the recording's next such input
/// must be the same step, and the sim ticks until the tick it was applied in (an input is
/// applied after its tick's controllers; the sim's ticks count from 1, the recording's from 0).
/// One the sim reaches later than the recording, or a different one, is reported.
fn align_input(w: &mut World, step: &str) {
    let Some(next) = w.recorded_inputs.as_mut().map(std::collections::VecDeque::pop_front) else {
        return;
    };
    let Some((t, recorded)) = next else {
        w.coverage.unsupported("dialogue replay: an input the recording doesn't have");
        return;
    };
    if recorded != step {
        w.coverage.unsupported("dialogue replay: an input unlike the recording's");
        return;
    }
    while w.events.tick < t + 1 {
        tick(w, None);
    }
    if w.events.tick > t + 1 {
        w.coverage.unsupported("dialogue replay: an input later than the recording's");
    }
}

/// Before a `zone:` step, with recorded zone entries: the recording's next entry must be the
/// main character's into that zone's blueprint, and the sim ticks up to the tick before it (the
/// step's own tick then enters at the script zones' place). An entry the sim reaches later, or
/// a different one, is reported.
fn align_zone(w: &mut World, zone: Guid) {
    let Some(next) = w.recorded_cs.as_mut().and_then(|r| r.zones.as_mut()).map(|z| z.pop_front())
    else {
        return;
    };
    let Some(e) = next else {
        w.coverage.unsupported("dialogue replay: a zone entry the recording doesn't have");
        return;
    };
    if e.zone != Some(zone) || !e.main {
        w.coverage.unsupported("dialogue replay: a zone entry unlike the recording's");
        return;
    }
    // The step's tick is the sim's `t + 1`.
    while w.events.tick < e.tick {
        tick(w, None);
    }
    if w.events.tick > e.tick {
        w.coverage.unsupported("dialogue replay: a zone entry later than the recording's");
    }
}

fn answer(w: &mut World, a: Guid, n: usize, out: &mut RunOutcome) {
    align_input(w, &format!("answer:{n}"));
    let of = w.dialog.answers.len();
    w.events
        .push("input", json!({ "step": format!("answer:{n}"), "answer": a.to_string(), "of": of }));
    // `Manual` selection answers get the main character (`Driver.Answer`).
    let manual = w
        .content
        .get(a)
        .and_then(|b| b.view().obj("CharacterSelection").map(|c| c.int("SelectionType")))
        .filter(|t| *t == 4)
        .and(w.main_character);
    crate::dialog::select_answer(w, a, manual);
    out.answers.push(a.to_string());
    tick(w, None);
}

/// Runs the steps. Stops at the first step that fails, as the driver does.
pub fn run(w: &mut World, steps: &[String]) -> RunOutcome {
    let mut out = RunOutcome::default();
    if w.player.mode.is_empty() {
        w.player.mode = "Default";
    }
    for (i, s) in steps.iter().enumerate() {
        let (name, arg) = s.split_once(':').map_or((s.as_str(), None), |(a, b)| (a, Some(b)));
        let optional = name.ends_with('?');
        let name = name.trim_end_matches('?');
        // Optional steps give up after 120 frames instead of 3600 (`Driver.Optional`).
        let max = if optional { MAX_WAIT_OPTIONAL } else { MAX_WAIT };
        let r = run_step(w, name, arg, max, &mut out);
        if let Err(e) = r {
            if optional {
                continue;
            }
            out.failure = Some(format!("step {i} `{s}`: {e}"));
            break;
        }
        out.steps_done = i + 1;
    }
    out.ticks = w.events.tick;
    out
}

fn run_step(
    w: &mut World,
    name: &str,
    arg: Option<&str>,
    max: u32,
    out: &mut RunOutcome,
) -> Result<()> {
    let guid = |s: &str| Guid::parse(s).with_context(|| format!("bad GUID {s}"));
    match name {
        "wait" => {
            let n: u32 = arg.unwrap_or("1").parse()?;
            for _ in 0..n {
                tick(w, None);
            }
        }
        "zone" => {
            let z = guid(arg.context("zone needs a script zone blueprint")?)?;
            align_zone(w, z);
            w.events.push("input", json!({ "step": "zone", "zone": z.to_string() }));
            tick(w, Some(z));
        }
        "dialog" => {
            let arg = arg.context("dialog needs a blueprint")?;
            let mut parts = arg.split(':');
            let d = guid(parts.next().unwrap_or_default())?;
            if parts.next().is_some() {
                bail!("dialog with a unit isn't supported by the runner");
            }
            align_input(w, &format!("dialog:{arg}"));
            w.events.push("input", json!({ "step": format!("dialog:{arg}") }));
            // `StartDialogWithoutTarget(dialog, null, Main_)`.
            let mc = w.main_character;
            crate::dialog::start_dialog(w, d, mc, None);
            tick(w, None);
        }
        "waitdialog" => {
            if !wait_until(w, waits_for_answer, max) {
                bail!("waitdialog: timed out");
            }
        }
        "waitnodialog" => {
            if !wait_until(w, |w| !dialog_running(w), max) {
                bail!("waitnodialog: timed out");
            }
        }
        "answer" | "answerbp" => {
            wait_until(w, waits_for_answer, max);
            let answers = w.dialog.answers.clone();
            let n = if name == "answer" {
                arg.context("answer:N")?.parse::<usize>()?
            } else {
                let g = guid(arg.context("answerbp:GUID")?)?;
                answers
                    .iter()
                    .position(|a| *a == g)
                    .context("answerbp: not among the answers shown")?
            };
            if !waits_for_answer(w) || n >= answers.len() {
                bail!("answer:{n}: no such answer ({} answers)", answers.len());
            }
            answer(w, answers[n], n, out);
        }
        "dialogrun" => {
            let mut prefs: Vec<Guid> = match arg {
                Some(a) if !a.is_empty() => a.split(',').map(guid).collect::<Result<_>>()?,
                _ => Vec::new(),
            };
            let mut answered = 0u32;
            loop {
                for _ in 0..MAX_WAIT {
                    if !dialog_running(w) || waits_for_answer(w) {
                        break;
                    }
                    tick(w, None);
                }
                if !dialog_running(w) {
                    w.events
                        .push("input", json!({ "step": "dialogrun_end", "answered": answered }));
                    break;
                }
                if !waits_for_answer(w) {
                    bail!("dialogrun: timed out after {MAX_WAIT} ticks");
                }
                if answered >= MAX_ANSWERS {
                    bail!("dialogrun: {MAX_ANSWERS} answers without the dialogue ending");
                }
                let answers = w.dialog.answers.clone();
                let mut n = 0;
                if let Some(p) = prefs.iter().position(|p| answers.contains(p)) {
                    n = answers.iter().position(|a| *a == prefs[p]).unwrap_or(0);
                    prefs.remove(p);
                }
                answer(w, answers[n], n, out);
                answered += 1;
            }
        }
        other => bail!("step {other} isn't supported by the runner (T1 steps are given as zone:)"),
    }
    Ok(())
}

/// The spec's steps with their answers replaced by the oracle run's recorded ones, for
/// fixtures whose optional steps depend on T1 timing: each `dialogrun` becomes the
/// `answerbp:` steps of the answers it gave (up to its `dialogrun_end` record), and the one
/// run of `answer`/`answerbp` steps (optional or not) becomes the recorded plain answers.
/// Every other step stays in place.
///
/// # Errors
///
/// More than one run of plain answer steps (their recorded answers can't be assigned).
pub fn steps_with_oracle_answers(steps: &[String], records: &[Value]) -> Result<Vec<String>> {
    let name = |s: &str| s.split(':').next().unwrap_or("").trim_end_matches('?').to_owned();
    let inputs: Vec<&Value> = records.iter().filter(|r| r["k"] == "input").collect();
    let mut runs: Vec<Vec<String>> = Vec::new();
    let mut plain: Vec<String> = Vec::new();
    let mut current: Option<Vec<String>> = None;
    for r in inputs {
        let step = r["step"].as_str().unwrap_or("");
        let via_run = r["via"] == "dialogrun";
        if step.starts_with("answer") {
            let a = format!("answerbp:{}", r["answer"].as_str().unwrap_or(""));
            if via_run {
                current.get_or_insert_with(Vec::new).push(a);
            } else {
                plain.push(a);
            }
        } else if step == "dialogrun_end" {
            runs.push(current.take().unwrap_or_default());
        }
    }
    let mut out = Vec::new();
    let mut runs = runs.into_iter();
    let mut plain_used = false;
    let mut in_plain = false;
    for s in steps {
        let n = name(s);
        if n == "dialogrun" {
            in_plain = false;
            out.extend(runs.next().context("a dialogrun step without its recorded answers")?);
            // `DialogRun` then waits until no dialogue runs (`dialogrun_end`).
            out.push("waitnodialog".to_owned());
        } else if n == "answer" || n == "answerbp" {
            if !in_plain {
                if plain_used {
                    bail!("several runs of answer steps: recorded answers are ambiguous");
                }
                plain_used = true;
                in_plain = true;
                out.append(&mut plain);
            }
        } else {
            in_plain = false;
            out.push(s.clone());
        }
    }
    Ok(out)
}

// ---- comparison with the oracle trace ------------------------------------------------------

/// A trace's T0 records in their recorded order, normalized for comparison: dialogue
/// records and answer inputs, narrative mutations, rules (pushed, and their results) and
/// the draws at modelled sites. Records the sim doesn't make (handlers, ticks, T1) are left
/// out.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Stream {
    pub items: Vec<Value>,
    /// Draws at audited T1 sites left out, by site.
    pub t1_excluded: std::collections::BTreeMap<String, u64>,
    /// Sim draws at sites the sim doesn't model (behaviour it doesn't run), by site.
    pub unmodelled: std::collections::BTreeMap<String, u64>,
    /// Why the recording isn't a valid comparison target ([`oracle_health`]).
    pub health_problems: Vec<String>,
}

fn pick(r: &Value, fields: &[&str]) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("k".into(), r["k"].clone());
    for f in fields {
        m.insert((*f).into(), r.get(*f).cloned().unwrap_or(Value::Null));
    }
    Value::Object(m)
}

const NARRATIVE_MUTS: &[&str] = &[
    "objective",
    "xp",
    "alignment",
    "fact_attach",
    "fact_detach",
    "etude_StartEtude",
    "etude_MarkEtudeCompleted",
    "etude_UnstartEtude",
    "flag_Unlock",
    "flag_Lock",
    "flag_SetFlagValue",
];

/// Result fields compared per rule class (both sides record them at the rule's end).
fn rule_fields(class: &str) -> &'static [&'static str] {
    match class {
        "RuleRollD20" => &["Result"],
        "RuleSkillCheck" => &[
            "StatType",
            "StatValue",
            "DC",
            "OriginalDC",
            "DifficultyDCAdjustment",
            "D20",
            "RollResult",
            "BaseRollResult",
            "Success",
        ],
        "RulePartyStatCheck" => &["StatValue", "Roller", "D20", "RollResult"],
        "RuleCalculateAbilityParams" => &["Result"],
        _ => &[],
    }
}

fn short_class(c: &Value) -> String {
    c.as_str().map(|s| s.rsplit('.').next().unwrap_or(s).to_owned()).unwrap_or_default()
}

/// A rule reference (`{"$rule": id, "result": v}`) as its result.
fn rule_value(v: &Value) -> Value {
    match v.get("result") {
        Some(r) if v.get("$rule").is_some() => r.clone(),
        _ => v.clone(),
    }
}

fn rule_result(class: &str, fields: &serde_json::Map<String, Value>) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("k".into(), json!("rule_result"));
    m.insert("class".into(), json!(class));
    for f in rule_fields(class) {
        let mut v = fields.get(*f).map_or(Value::Null, rule_value);
        if let (Some(keep), Value::Object(o)) = (rule_subfields(class, f), &mut v) {
            o.retain(|k, _| keep.contains(&k.as_str()));
        }
        m.insert((*f).into(), v);
    }
    Value::Object(m)
}

/// The members compared inside a rule's field: the stored ones (the oracle's snapshot also
/// lists pure getters, such as `AbilityParams.IsChanged`).
fn rule_subfields(class: &str, field: &str) -> Option<&'static [&'static str]> {
    match (class, field) {
        ("RuleCalculateAbilityParams", "Result") => Some(&[
            "SpellSource",
            "CasterLevel",
            "SpellLevel",
            "DC",
            "Concentration",
            "Metamagic",
            "RankBonus",
        ]),
        _ => None,
    }
}

/// A dialogue, input or mutation record, normalized; `None` for the others.
fn normalize(r: &Value) -> Option<Value> {
    let k = r["k"].as_str().unwrap_or("");
    let d = match k {
        "dlg_request" => pick(r, &["dialog", "initiator", "unit"]),
        "dlg_started" => pick(r, &["dialog", "first_speaker", "scheduled_cue"]),
        "dlg_play" => pick(r, &["cue", "class"]),
        "dlg_check" => pick(r, &["check", "stat", "authored_dc", "hidden"]),
        "dlg_check_end" => pick(r, &["check", "passed", "failed"]),
        "dlg_answer" => pick(r, &["answer", "index", "cue", "manual_unit"]),
        "dlg_answer_end" => {
            pick(r, &["scheduled", "scheduled_cue", "acting_unit", "dialog_active"])
        }
        "dlg_transition" => pick(
            r,
            &[
                "dialog",
                "cue",
                "answers",
                "continue_cue",
                "current_speaker",
                "acting_unit",
                "skill_checks",
                "local_shown_cues",
                "local_selected_answers",
                "local_passed_checks",
                "local_failed_checks",
                "global_shown_cues",
                "global_selected_answers",
                "global_shown_dialogs",
                "alignment",
            ],
        ),
        "dlg_stop" => pick(r, &["dialog"]),
        "input" if r["step"].as_str().is_some_and(|s| s.starts_with("answer")) => {
            pick(r, &["answer"])
        }
        "mut" => {
            let what = r["what"].as_str().unwrap_or("");
            if !NARRATIVE_MUTS.contains(&what) {
                return None;
            }
            match what {
                "objective" => pick(r, &["what", "objective", "state"]),
                "xp" => pick(r, &["what", "unit", "gained", "total"]),
                "alignment" => pick(r, &["what", "direction", "value", "now"]),
                "fact_attach" | "fact_detach" => {
                    let mut v = pick(r, &["what", "owner", "class", "rank"]);
                    v["fact"] = r["fact"].clone();
                    v
                }
                w if w.starts_with("flag_") => pick(r, &["what", "flag", "unlocked", "value"]),
                _ => pick(r, &["what", "etude"]),
            }
        }
        _ => return None,
    };
    Some(d)
}

/// What makes a recording unusable as a comparison target (`oracle-trace-diff --check`'s
/// capture checks): a missing `begin` or `end`, no health, and any failed, dropped or
/// structurally broken record, hook exception, unknown unhooked RNG step or off-thread
/// draw. Known unhooked steps (the oracle's `--allow-unhooked`, measured in presentation
/// helpers) and replay tape counters (T1 sites) are reported, not failures.
#[must_use]
pub fn oracle_health(records: &[Value]) -> (Vec<String>, Value) {
    let mut problems = Vec::new();
    if !records.iter().any(|r| r["k"] == "begin") {
        problems.push("no begin record".to_owned());
    }
    let Some(end) = records.iter().rev().find(|r| r["k"] == "end") else {
        problems.push("no end record".to_owned());
        return (problems, Value::Null);
    };
    let h = &end["health"];
    if !h.is_object() {
        problems.push("no health in the end record".to_owned());
        return (problems, Value::Null);
    }
    for k in [
        "records_failed",
        "records_dropped",
        "structural_errors",
        "hook_exceptions_caught",
        "rng_unhooked_unknown",
        "rng_offthread_draws",
        "snapshot_value_failures",
    ] {
        if h[k].as_i64().unwrap_or(0) != 0 {
            problems.push(format!("{k} = {}", h[k]));
        }
    }
    if h["errors"].as_array().is_some_and(|a| !a.is_empty()) {
        problems.push(format!("errors: {}", h["errors"]));
    }
    (problems, h.clone())
}

/// The roaming method behind a draw whose site key names only a `LinqExtensions` helper or
/// the method: `RoamingWaypointData.SelectNextPoint`'s `WeightedRandom` and `SelectPrevPoint`'s
/// `Random`. The waypoint they choose is where the roaming unit walks next (T1); what it does
/// there is another recorded choice (`SelectCutscene`, audited by [`audit_roaming`]).
fn roaming_method(key: &str) -> Option<&'static str> {
    if !key.starts_with("global|")
        && key.contains("|Kingmaker.Utility.LinqExtensions|WeightedRandom")
    {
        Some("SelectNextPoint")
    } else if key.starts_with("global|")
        && key.contains("|Kingmaker.View.Roaming.RoamingWaypointData|SelectPrevPoint(")
        && key.contains("|via LinqExtensions.Random|")
    {
        Some("SelectPrevPoint")
    } else {
        None
    }
}

/// Whether `p`, the record just before the draw `r` (WrathOracle writes `roam_point` right
/// before the method draws, nothing in between), is its producer: that method's `roam_point`
/// in the same tick, asked by the roaming controller for a unit, with candidates to draw from,
/// and the draw's range theirs (`Random`: `[0, n)`; `WeightedRandom`: from 0).
fn roaming_producer(p: Option<&Value>, r: &Value, method: &str) -> bool {
    let Some(p) = p else { return false };
    let n = p["n"].as_i64();
    p["k"] == "obs"
        && p["ev"] == "roam_point"
        && p["m"] == method
        && p["t"].is_i64()
        && p["t"] == r["t"]
        && p["caller"] == "controller"
        && p["unit"].is_string()
        && p["point"].is_string()
        && n.is_some_and(|n| n > 0)
        && match method {
            "SelectPrevPoint" => {
                r["m"] == "Range_ii" && r["a"][0].as_i64() == Some(0) && r["a"][1].as_i64() == n
            }
            _ => r["m"] == "Range_ff" && r["a"][0].as_f64() == Some(0.0),
        }
}

/// The oracle trace's stream.
#[must_use]
pub fn oracle_stream(records: &[Value]) -> Stream {
    let keys = site_keys(records);
    let mut s = Stream { health_problems: oracle_health(records).0, ..Stream::default() };
    // Per rule id: its class and its fields (the push snapshot, then each phase's changes).
    let mut rules: std::collections::HashMap<i64, (String, serde_json::Map<String, Value>)> =
        std::collections::HashMap::new();
    for (i, r) in records.iter().enumerate() {
        let k = r["k"].as_str().unwrap_or("");
        match k {
            // A roaming point choice with candidates draws right after its record: one without
            // its draw is a recording the comparison can't read.
            "obs"
                if r["ev"] == "roam_point"
                    && matches!(r["m"].as_str(), Some("SelectNextPoint" | "SelectPrevPoint"))
                    && r["n"].as_i64().is_some_and(|n| n > 0) =>
            {
                let m = r["m"].as_str().unwrap_or("");
                let next = records.get(i + 1).filter(|d| d["k"] == "draw" && d["c"] == "sim");
                let tied = next.is_some_and(|d| {
                    let key = d["site"]
                        .as_i64()
                        .and_then(|id| keys.get(&id).cloned())
                        .or_else(|| d["key"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    roaming_method(&key) == Some(m) && roaming_producer(Some(r), d, m)
                });
                if !tied {
                    *s.unmodelled.entry(format!("roaming: a {m} without its draw")).or_default() +=
                        1;
                }
            }
            "rule" => {
                let class = short_class(&r["class"]);
                let fields = r["fields"].as_object().cloned().unwrap_or_default();
                if let Some(id) = r["id"].as_i64() {
                    rules.insert(id, (class.clone(), fields));
                }
                s.items.push(json!({ "k": "rule", "class": class, "initiator": r["initiator"] }));
            }
            // Each phase record carries the rule's field changes since the previous record
            // (handlers' `h_end` keyed by their `rule`); `rule_end` adds the final ones.
            "about" | "trig" | "trig_end" | "did" | "h_end" | "rule_end" => {
                let id = if k == "h_end" { r["rule"].as_i64() } else { r["id"].as_i64() };
                if let (Some(id), Some(ch)) = (id, r["changes"].as_array())
                    && let Some((_, f)) = rules.get_mut(&id)
                {
                    for c in ch {
                        if let Some(name) = c[0].as_str() {
                            f.insert(name.to_owned(), c[1].clone());
                        }
                    }
                }
                if k == "rule_end"
                    && let Some((class, f)) = r["id"].as_i64().and_then(|id| rules.remove(&id))
                {
                    s.items.push(rule_result(&class, &f));
                }
            }
            "draw" if r["c"] == "sim" => {
                let key = r["site"]
                    .as_i64()
                    .and_then(|id| keys.get(&id).cloned())
                    .or_else(|| r["key"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "draw without a site".to_owned());
                match site_class(&key) {
                    SiteClass::Modelled => match draw_of(r, &keys) {
                        Ok(d) => s
                            .items
                            .push(json!({ "k": "draw", "site": d.site, "v": draw_value(d.value) })),
                        Err(e) => *s.unmodelled.entry(e).or_default() += 1,
                    },
                    SiteClass::AuditedT1 => *s.t1_excluded.entry(key).or_default() += 1,
                    SiteClass::Unmodelled => match roaming_method(&key) {
                        Some(m)
                            if roaming_producer(i.checked_sub(1).map(|j| &records[j]), r, m) =>
                        {
                            *s.t1_excluded
                                .entry(format!(
                                    "roaming: RoamingWaypointData.{m} (its producer recorded)"
                                ))
                                .or_default() += 1;
                        }
                        _ => *s.unmodelled.entry(key).or_default() += 1,
                    },
                }
            }
            _ => {
                if let Some(v) = normalize(r) {
                    s.items.push(v);
                }
            }
        }
    }
    s
}

fn draw_value(v: DrawValue) -> Value {
    match v {
        DrawValue::Int(i) => json!(i),
        DrawValue::Float(f) => json!(f),
    }
}

/// The sim run's stream: its trace records with its draws put back in place (each record
/// carries the number of draws made before it).
#[must_use]
pub fn sim_stream(w: &World) -> Stream {
    let draws = w.rng.log();
    let mut s = Stream::default();
    let mut next = 0;
    let push_draws = |s: &mut Stream, upto: usize, next: &mut usize| {
        while *next < upto.min(draws.len()) {
            let d = &draws[*next];
            s.items.push(json!({ "k": "draw", "site": d.site, "v": draw_value(d.value) }));
            *next += 1;
        }
    };
    for e in &w.events.records {
        push_draws(&mut s, e.d, &mut next);
        let mut m = e.fields.clone();
        m.insert("k".into(), json!(e.k));
        let r = Value::Object(m);
        match e.k {
            "rule" => s.items.push(
                json!({ "k": "rule", "class": short_class(&r["class"]), "initiator": r["initiator"] }),
            ),
            "rule_end" => {
                let class = short_class(&r["class"]);
                let f = r["result"].as_object().cloned().unwrap_or_default();
                s.items.push(rule_result(&class, &f));
            }
            _ => {
                if let Some(v) = normalize(&r) {
                    s.items.push(v);
                }
            }
        }
    }
    push_draws(&mut s, usize::MAX, &mut next);
    s
}

fn rename_runtime_ids(stream: &mut [Value], ids: &RuntimeIds) {
    let mut map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for v in stream {
        rename_in(v, ids, &mut map);
    }
}

/// Which ids were allocated during a run: hex ids at or above the save's counter, and
/// `System.Guid` ids (36 characters) the save doesn't have.
#[derive(Debug, Clone, Default)]
pub struct RuntimeIds {
    pub first_hex: u64,
    pub saved: std::collections::HashSet<String>,
}

impl RuntimeIds {
    /// From a world right after hydration.
    #[must_use]
    pub fn of(w: &World) -> Self {
        let mut saved: std::collections::HashSet<String> = w.by_id.keys().cloned().collect();
        saved.extend(w.facts.iter().map(|f| f.unique_id.clone()).filter(|s| !s.is_empty()));
        Self { first_hex: w.player.max_unique_id, saved }
    }

    #[must_use]
    pub fn is_runtime(&self, s: &str) -> bool {
        (is_runtime_hex_id(s, self.first_hex) && !self.saved.contains(s))
            || (s.len() == 36 && s.matches('-').count() == 4 && !self.saved.contains(s))
    }
}

/// Whether a string is an entity or fact id the game allocated after the save was
/// written (`Player.GetNewUniqueId`: hex at or above the save's counter). Blueprint GUIDs
/// (32 hex digits) don't fit a `u64` and never match.
#[must_use]
pub fn is_runtime_hex_id(s: &str, first_runtime_id: u64) -> bool {
    !s.is_empty()
        && s.len() <= 16
        && u64::from_str_radix(s, 16).is_ok_and(|n| n >= first_runtime_id)
}

/// Renames run-time ids by first appearance (one bijection per side, over the whole
/// compared sequence, so aliasing is kept).
pub fn rename_in(
    v: &mut Value,
    ids: &RuntimeIds,
    map: &mut std::collections::HashMap<String, String>,
) {
    match v {
        Value::String(s) if ids.is_runtime(s) => {
            let n = map.len();
            *s = map.entry(s.clone()).or_insert_with(|| format!("new#{n}")).clone();
        }
        Value::Array(a) => a.iter_mut().for_each(|x| rename_in(x, ids, map)),
        Value::Object(o) => o.values_mut().for_each(|x| rename_in(x, ids, map)),
        _ => {}
    }
}

/// One sequence's comparison.
#[derive(Debug, Default, Clone, Serialize)]
pub struct StreamDiff {
    pub ours: usize,
    pub oracle: usize,
    pub equal_prefix: usize,
    pub equal: bool,
    /// The first difference: index, ours, oracle.
    pub first_difference: Option<(usize, Value, Value)>,
}

fn diff_stream(a: &[Value], b: &[Value]) -> StreamDiff {
    let n = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    StreamDiff {
        ours: a.len(),
        oracle: b.len(),
        equal_prefix: n,
        equal: n == a.len() && n == b.len(),
        first_difference: (n < a.len().max(b.len())).then(|| {
            (n, a.get(n).cloned().unwrap_or(Value::Null), b.get(n).cloned().unwrap_or(Value::Null))
        }),
    }
}

/// The T0 comparison of a sim run with an oracle trace.
#[derive(Debug, Default, Clone, Serialize)]
pub struct TraceDiff {
    /// The whole ordered stream: the equality predicate.
    pub all: StreamDiff,
    /// The same without fact attaches and detaches on entities created during the run: a
    /// unit the sim doesn't build yet (`ChangeUnitBlueprint`) would otherwise hide every
    /// later difference. A measurement beside `all`, never instead of it.
    pub all_except_new_owners: StreamDiff,
    /// Per kind, for diagnosis (subsequences of `all`).
    pub dialog: StreamDiff,
    pub mutations: StreamDiff,
    pub rules: StreamDiff,
    pub draws: StreamDiff,
    pub t1_excluded: std::collections::BTreeMap<String, u64>,
    /// Oracle sim draws at sites the sim doesn't model: fails the comparison.
    pub unmodelled_draws: std::collections::BTreeMap<String, u64>,
    /// The recording's health problems: any fails the comparison.
    pub oracle_health_problems: Vec<String>,
}

impl TraceDiff {
    #[must_use]
    pub fn equal(&self) -> bool {
        self.all.equal && self.unmodelled_draws.is_empty() && self.oracle_health_problems.is_empty()
    }
}

/// Compares the two streams after renaming run-time ids (one correspondence per compared
/// sequence and side).
#[must_use]
pub fn compare_streams(ours: &Stream, oracle: &Stream, ids: &RuntimeIds) -> TraceDiff {
    let renamed = |v: Vec<Value>| -> Vec<Value> {
        let mut v = v;
        rename_runtime_ids(&mut v, ids);
        v
    };
    let filtered = |s: &Stream, keep: &dyn Fn(&Value) -> bool| -> Vec<Value> {
        renamed(s.items.iter().filter(|v| keep(v)).cloned().collect())
    };
    let kind =
        |p: &'static [&'static str]| move |v: &Value| p.contains(&v["k"].as_str().unwrap_or(""));
    let is_dialog =
        |v: &Value| v["k"].as_str().is_some_and(|k| k.starts_with("dlg_") || k == "input");
    let not_new_owner = |v: &Value| {
        !(matches!(v["what"].as_str(), Some("fact_attach" | "fact_detach"))
            && v["owner"].as_str().is_some_and(|o| ids.is_runtime(o)))
    };
    let all = |_: &Value| true;
    TraceDiff {
        all: diff_stream(&filtered(ours, &all), &filtered(oracle, &all)),
        all_except_new_owners: diff_stream(
            &filtered(ours, &not_new_owner),
            &filtered(oracle, &not_new_owner),
        ),
        dialog: diff_stream(&filtered(ours, &is_dialog), &filtered(oracle, &is_dialog)),
        mutations: diff_stream(
            &filtered(ours, &kind(&["mut"])),
            &filtered(oracle, &kind(&["mut"])),
        ),
        rules: diff_stream(
            &filtered(ours, &kind(&["rule", "rule_result"])),
            &filtered(oracle, &kind(&["rule", "rule_result"])),
        ),
        draws: diff_stream(&filtered(ours, &kind(&["draw"])), &filtered(oracle, &kind(&["draw"]))),
        t1_excluded: oracle.t1_excluded.clone(),
        unmodelled_draws: oracle.unmodelled.clone(),
        oracle_health_problems: oracle.health_problems.clone(),
    }
}

// ---- a whole fixture -----------------------------------------------------------------------

/// The recording's roaming producers (`roam_point`, `roam_roll`) by oracle tick, for
/// [`roaming_tick`] (one without a tick is reported there, under tick -1).
#[must_use]
pub fn recorded_roaming(records: &[Value]) -> std::collections::BTreeMap<i64, Vec<Value>> {
    let mut out: std::collections::BTreeMap<i64, Vec<Value>> = std::collections::BTreeMap::new();
    for r in records
        .iter()
        .filter(|r| r["k"] == "obs" && matches!(r["ev"].as_str(), Some("roam_point" | "roam_roll")))
    {
        out.entry(r["t"].as_i64().unwrap_or(-1)).or_default().push(r.clone());
    }
    out
}

/// At `UnitRoamingController`'s place in the oracle's tick `t` (Default, Dialog, Cutscene,
/// Rest): the tick's recorded producers, audited against the world as it is then
/// ([`audit_roaming`]). A producer in an earlier tick the sim's roaming controller didn't run is
/// reported.
pub fn roaming_tick(w: &mut World, t: i64) {
    let Some(mut rec) = w.recorded_roaming.take() else { return };
    let earlier: Vec<i64> = rec.range(..t).map(|(k, _)| *k).collect();
    if !earlier.is_empty() {
        for k in earlier {
            rec.remove(&k);
        }
        w.coverage.unsupported(
            "roaming: a recorded producer in a tick the sim's roaming controller didn't run",
        );
    }
    if let Some(now) = rec.remove(&t) {
        audit_roaming(w, &now);
    }
    w.recorded_roaming = Some(rec);
}

/// Roaming's recorded producers (`roam_point`, `roam_roll`) against the sim's world: a roaming
/// choice only moves its unit (T1) when the roaming controller makes it for a roaming unit the
/// sim has, at a waypoint of the area's scene, and it can't start an idle cutscene (the
/// waypoint's and the unit's settings' lists empty, as the recording says too). Anything else is
/// reported: the draws tied to these records ([`oracle_stream`]) are accepted only so.
pub fn audit_roaming(w: &mut World, records: &[Value]) {
    // The unit through the recording's identities (a saved id as is, a run-created one through
    // its match): without the awake records, identities aren't established.
    let unit = |w: &mut World, r: &Value| -> bool {
        let Some(id) = r["unit"].as_str() else { return false };
        let Some(mut awake) = w.recorded_awake.take() else { return false };
        let e = awake.resolve(w, id);
        w.recorded_awake = Some(awake);
        e.is_some_and(|e| w.entity(e).parts.iter().any(|p| p.clr == "UnitPartRoaming"))
    };
    let empty = |v: &Value| v.as_array().is_some_and(Vec::is_empty);
    for r in records.iter().filter(|r| r["k"] == "obs") {
        match r["ev"].as_str() {
            Some("roam_point") => {
                let m = r["m"].as_str().unwrap_or("");
                if r["caller"] != "controller" {
                    w.coverage.unsupported(
                        "roaming: a waypoint choice not made by UnitRoamingController (or its caller not recorded)",
                    );
                    continue;
                }
                if !unit(w, r) {
                    w.coverage.unsupported("roaming: a roaming unit the sim doesn't have");
                }
                let view = r["point"].as_str().and_then(|p| {
                    w.scene
                        .cache
                        .as_ref()?
                        .view(p)
                        .cloned()
                        .filter(|v| v.class.ends_with(".RoamingWaypointView"))
                });
                let Some(view) = view else {
                    w.coverage.unsupported("roaming: a waypoint the scene cache doesn't have");
                    continue;
                };
                let fields = view
                    .components
                    .iter()
                    .find(|c| c.class.ends_with(".RoamingWaypointView"))
                    .map(|c| c.fields.clone())
                    .unwrap_or(Value::Null);
                let count = |f: &str| fields[f].as_array().map(|a| a.len() as i64);
                match m {
                    "SelectCutscene" => {
                        if !(empty(&fields["IdleCutscenes"])
                            && empty(&r["idle"])
                            && r["n"].as_i64() == Some(0))
                        {
                            w.coverage
                                .unsupported("roaming: a waypoint's idle cutscene (not modelled)");
                        }
                    }
                    "SelectNextPoint" if r["n"].as_i64() == count("NextWaypoints") => {}
                    // `PrevWaypoints` is built from the other waypoints' links at `OnEnable`.
                    "SelectPrevPoint" if r["n"].as_i64().is_some_and(|n| n >= 0) => {}
                    _ => w.coverage.unsupported(
                        "roaming: a waypoint choice unlike the scene cache's waypoint",
                    ),
                }
            }
            // `RollNextPoint`: a point of the unit's own around its origin; an idle cutscene from
            // its settings would play on arrival.
            Some("roam_roll") => {
                if !empty(&r["idle"]) {
                    w.coverage.unsupported(
                        "roaming: a roaming unit's idle cutscenes (RollNextPoint, not modelled)",
                    );
                }
                if !unit(w, r) {
                    w.coverage.unsupported("roaming: a roaming unit the sim doesn't have");
                }
            }
            _ => {}
        }
    }
}

fn read_json(p: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(
        &std::fs::read(p).with_context(|| format!("reading {}", p.display()))?,
    )?)
}

/// What [`run_fixture`] needs.
#[derive(Debug, Clone)]
pub struct FixtureOptions<'a> {
    /// The fixture spec (`crates/wr-sim/fixtures/poc4.json`) and the fixture's name in it.
    pub spec: &'a Path,
    pub fixture: &'a str,
    /// The oracle's fixture directory and which of its runs to replay (`record`,
    /// `replay-1`, `replay-2-perturbed`…).
    pub oracle: Option<&'a Path>,
    pub run: &'a str,
    /// Answer with the oracle run's recorded answers (see [`steps_with_oracle_answers`]).
    pub oracle_inputs: bool,
    /// `AbilityParams.Empty` when the oracle's load began, if known.
    pub ability_params_empty: Option<(i32, i32)>,
}

/// A fixture run and its comparison with the oracle.
#[derive(Debug)]
pub struct FixtureRun {
    pub world: World,
    pub save: String,
    pub outcome: RunOutcome,
    pub first_runtime_id: u64,
    pub load_unsupported: std::collections::BTreeMap<String, u64>,
    pub run_ms: f64,
    pub narrative_start: Option<Value>,
    pub narrative_end: Value,
    pub sheet_end: Value,
    pub start_diff: Option<crate::narrative::dump::NarrativeDiff>,
    pub end_diff: Option<crate::narrative::dump::NarrativeDiff>,
    pub units_diff: Option<crate::compare::Report>,
    pub trace_diff: Option<TraceDiff>,
    pub streams: Option<(Stream, Stream)>,
}

impl FixtureRun {
    /// The run is valid when nothing it ran is unsupported and the oracle drew nowhere the
    /// sim doesn't: a draw the original makes at a site the sim neither models nor audits as
    /// timing-coupled (T1) is behaviour the sim lacks (`TraceDiff::unmodelled_draws`).
    #[must_use]
    pub fn valid(&self) -> bool {
        self.world.valid() && self.trace_diff.as_ref().is_none_or(|t| t.unmodelled_draws.is_empty())
    }

    /// What ran that isn't supported, beyond what loading the save reported.
    #[must_use]
    pub fn run_unsupported(&self) -> std::collections::BTreeMap<String, u64> {
        self.world
            .coverage
            .unsupported
            .iter()
            .filter(|(k, v)| self.load_unsupported.get(*k) != Some(v))
            .map(|(k, v)| (k.clone(), *v - self.load_unsupported.get(k).copied().unwrap_or(0)))
            .collect()
    }
}

/// Hydrates the fixture's save, runs its steps (on the oracle run's tape and clock, if
/// given) and compares the trace and the end state with the oracle's.
///
/// # Errors
///
/// Missing spec, save, content or capture files.
pub fn run_fixture(o: &FixtureOptions<'_>) -> Result<FixtureRun> {
    use std::rc::Rc;
    let spec = read_json(o.spec)?;
    let f = &spec["fixtures"][o.fixture];
    let save = f["save"].as_str().context("fixture not in the spec")?.to_owned();
    let mut steps: Vec<String> = f["steps"]
        .as_array()
        .context("steps")?
        .iter()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect();
    // The recording, before the load: whether it has the cutscenes' progress decides how the
    // load treats random gates.
    let records = match o.oracle {
        Some(dir) => read_trace(&dir.join(format!("{}.trace.jsonl.gz", o.run)))?,
        None => Vec::new(),
    };
    let recorded_cutscenes = crate::elements::cutscene::RecordedCutscene::present(&records);
    let recorded_locks =
        recorded_cutscenes && crate::elements::cutscene::RecordedCutscene::has_locks(&records);
    let path = wr_save::report::find_save(&save)?;
    let s = crate::hydrate::Save::read(&path)?;
    let content = Rc::new(crate::content::Content::open_default()?);
    let registry = Rc::new(crate::components::registry());
    let (mut w, _) = crate::hydrate::hydrate(
        content,
        registry,
        &s,
        &crate::hydrate::Options {
            ability_params_empty: o.ability_params_empty,
            recorded_cutscenes,
            recorded_locks,
            ..crate::hydrate::Options::default()
        },
    )?;
    let first_runtime_id = w.player.max_unique_id;
    let runtime_ids = RuntimeIds::of(&w);
    let load_unsupported = w.coverage.unsupported.clone();
    let mut narrative_start = None;
    let mut start_diff = None;
    if let Some(dir) = o.oracle {
        if o.oracle_inputs {
            steps = steps_with_oracle_answers(&steps, &records)?;
        }
        w.rng = Box::new(crate::rng::Tape::new(tape_from_trace(&records)));
        // The recording's awake states (newer recordings have them): inputs where the sim
        // can't establish a unit's.
        // The recording's dialogue inputs, at their ticks (with the cutscenes' recorded progress,
        // whose records interleave with the dialogue's by tick).
        if recorded_cutscenes {
            w.recorded_inputs = Some(
                records
                    .iter()
                    .filter(|r| r["k"] == "input")
                    .filter_map(|r| Some((r["t"].as_i64()?, r["step"].as_str()?.to_owned())))
                    .filter(|(_, s)| s.starts_with("dialog:") || s.starts_with("answer:"))
                    .collect(),
            );
            match crate::elements::cutscene::RecordedCutscene::of(&records) {
                Ok(r) => w.recorded_cs = Some(Box::new(r)),
                Err(e) => w
                    .coverage
                    .unsupported(format!("dialogue replay: a malformed cutscene record ({e})")),
            }
        }
        match crate::spawn::RecordedAwake::of(&records, runtime_ids.saved.clone()) {
            Ok(r) => w.recorded_awake = r.map(Box::new),
            Err(e) => {
                w.coverage.unsupported(format!("dialogue replay: a malformed awake record ({e})"))
            }
        }
        // Roaming's producers, audited at their ticks.
        w.recorded_roaming = Some(Box::new(recorded_roaming(&records)));
        // The unit groups, with the recording's states (`UnitGroupsController.Start` ran at the
        // load); without them a change of membership is reported.
        match crate::combat::groups::RecordedGroups::of(&records) {
            Ok(Some(r)) => {
                w.recorded_groups = Some(Box::new(r));
                crate::combat::groups::start_without_memory(&mut w);
            }
            Ok(None) => {}
            Err(e) => w
                .coverage
                .unsupported(format!("dialogue replay: a malformed unit group record ({e})")),
        }
        // The window starts after the oracle's settle ticks: its clock.
        let start = read_json(&dir.join(format!("state-start-{}.json", o.run)))?;
        if let (Some(gt), Some(rt)) =
            (start["game"]["game_time"].as_i64(), start["game"]["real_time"].as_i64())
        {
            w.clock.game_time = crate::csharp::timespan::TimeSpan(gt);
            w.clock.real_time = crate::csharp::timespan::TimeSpan(rt);
        }
        let ours = crate::narrative::dump::dump(&mut w, "start");
        start_diff = Some(crate::narrative::dump::compare(&ours, &start, &runtime_ids));
        narrative_start = Some(ours);
    }
    w.events.on = true;
    w.events.tick = 0;
    // Count draws into the trace so records and draws keep one order.
    let inner = std::mem::replace(&mut w.rng, Box::new(crate::rng::PlayRng::new(0)));
    w.events.draws.set(inner.log().len());
    w.rng = Box::new(crate::rng::Counted { inner, n: w.events.draws.clone() });
    let t0 = std::time::Instant::now();
    let outcome = run(&mut w, &steps);
    // Recorded inputs the run didn't reach, and recorded cutscene progress past its last tick.
    if w.recorded_inputs.as_ref().is_some_and(|q| !q.is_empty()) {
        w.coverage.unsupported("dialogue replay: a recorded input the run didn't reach");
    }
    let last = w.events.tick - 1;
    crate::elements::cutscene::recorded_finish(&mut w, last);
    if w.recorded_roaming.as_ref().is_some_and(|r| !r.is_empty()) {
        w.coverage.unsupported("roaming: a recorded producer in a tick the run didn't reach");
    }
    crate::combat::groups::recorded_finish(&mut w, last);
    let run_ms = t0.elapsed().as_secs_f64() * 1e3;
    w.finish_run();
    let narrative_end = crate::narrative::dump::dump(&mut w, "end");
    let units: Vec<crate::world::EntityIdx> = (0..w.entities.len())
        .map(|i| crate::world::EntityIdx(u32::try_from(i).unwrap_or(0)))
        .filter(|e| w.unit(*e).is_some())
        .collect();
    let sheet_end = crate::sheet::dump(&w, &units, "end");
    let (mut end_diff, mut units_diff, mut trace_diff, mut streams) = (None, None, None, None);
    if let Some(dir) = o.oracle {
        let ours = sim_stream(&w);
        let theirs = oracle_stream(&records);
        trace_diff = Some(compare_streams(&ours, &theirs, &runtime_ids));
        streams = Some((ours, theirs));
        let end = read_json(&dir.join(format!("state-end-{}.json", o.run)))?;
        end_diff = Some(crate::narrative::dump::compare(&narrative_end, &end, &runtime_ids));
        units_diff = Some(crate::compare::compare(&sheet_end, &end));
    }
    Ok(FixtureRun {
        world: w,
        save,
        outcome,
        first_runtime_id,
        load_unsupported,
        run_ms,
        narrative_start,
        narrative_end,
        sheet_end,
        start_diff,
        end_diff,
        units_diff,
        trace_diff,
        streams,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    const NEXT_SITE: &str = "Assembly-CSharp|Kingmaker.Utility.LinqExtensions|WeightedRandom`1(IList`1)|#0@40|Range(float,float)";
    const PREV_KEY: &str = "global|Assembly-CSharp|Kingmaker.View.Roaming.RoamingWaypointData|SelectPrevPoint()|via LinqExtensions.Random|Range_ii";

    fn roam(m: &str, n: i64, caller: &str) -> Value {
        json!({ "k": "obs", "t": 5, "ev": "roam_point", "m": m, "point": "p1", "idle": [], "n": n,
                "caller": caller, "unit": "u1" })
    }

    fn next_draw() -> Value {
        json!({ "k": "draw", "t": 5, "site": 7, "c": "sim", "m": "Range_ff", "a": [0.0, 2.0], "v": 1.5 })
    }

    fn prev_draw(n: i64) -> Value {
        json!({ "k": "draw", "t": 5, "site": -1, "key": PREV_KEY, "c": "sim", "m": "Range_ii", "a": [0, n], "v": 0 })
    }

    fn stream(body: &[Value]) -> super::Stream {
        let mut records = vec![json!({ "k": "begin", "rng_sites": [[7, NEXT_SITE]] })];
        records.extend(body.iter().cloned());
        super::oracle_stream(&records)
    }

    /// Roaming's helper draws are excluded only right after their producer's record, made by
    /// the roaming controller with candidates and the draw's range; anything else stays
    /// unmodelled, and a producer with candidates but no draw is reported.
    #[test]
    fn roaming_draws_count_only_with_their_producer() {
        let ok = stream(&[roam("SelectNextPoint", 2, "controller"), next_draw()]);
        assert!(ok.unmodelled.is_empty(), "{:?}", ok.unmodelled);
        assert_eq!(ok.t1_excluded.values().sum::<u64>(), 1);
        let ok = stream(&[roam("SelectPrevPoint", 3, "controller"), prev_draw(3)]);
        assert!(ok.unmodelled.is_empty(), "{:?}", ok.unmodelled);
        for (what, body) in [
            ("no producer", vec![next_draw()]),
            ("advance", vec![roam("SelectNextPoint", 2, "advance"), next_draw()]),
            ("other method", vec![roam("SelectPrevPoint", 2, "controller"), next_draw()]),
            ("wrong range", vec![roam("SelectPrevPoint", 2, "controller"), prev_draw(3)]),
            (
                "not adjacent",
                vec![
                    roam("SelectNextPoint", 2, "controller"),
                    json!({ "k": "tick", "t": 5 }),
                    next_draw(),
                ],
            ),
            ("no draw", vec![roam("SelectNextPoint", 2, "controller")]),
        ] {
            let s = stream(&body);
            assert!(!s.unmodelled.is_empty(), "{what}: accepted");
        }
        // Without candidates the method draws nothing, and nothing is expected.
        let none = stream(&[roam("SelectNextPoint", 0, "controller")]);
        assert!(none.unmodelled.is_empty());
    }

    /// `audit_roaming`: a choice is movement only for a roaming unit the sim has, at a waypoint
    /// of the scene cache, by the controller, with no idle cutscene to start.
    #[test]
    fn roaming_producers_are_audited_against_the_world() {
        use crate::scene::{CompInfo, SceneCache, ViewInfo};
        let mut w = crate::combat::turns::tests::world();
        let u = crate::combat::turns::tests::unit(&mut w, "u1", Some("g"), 0);
        w.entity_mut(u).parts.push(crate::world::Part { clr: "UnitPartRoaming" });
        // A run-created unit recorded as "n1" whose sim id is "u1"'s namesake elsewhere: "n2" in
        // the sim, without the part, while the recording's "u2" names a sim unit with it.
        let bp = wr_content::Guid(0x77);
        let n = crate::combat::turns::tests::unit(&mut w, "n2", Some("g"), 0);
        w.unit_mut(n).expect("unit").blueprint = Some(bp);
        let other = crate::combat::turns::tests::unit(&mut w, "n1", Some("g"), 0);
        w.entity_mut(other).parts.push(crate::world::Part { clr: "UnitPartRoaming" });
        w.recorded_awake = Some(Box::new(crate::spawn::RecordedAwake {
            saved: ["u1".to_owned()].into_iter().collect(),
            new_bp: [("n1".to_owned(), bp)].into_iter().collect(),
            ..crate::spawn::RecordedAwake::default()
        }));
        let view = |id: &str, idle: Value| ViewInfo {
            unique_id: id.into(),
            class: "Kingmaker.View.Roaming.RoamingWaypointView".into(),
            scene: "s".into(),
            active_in_hierarchy: true,
            is_in_game: true,
            components: vec![CompInfo {
                class: "Kingmaker.View.Roaming.RoamingWaypointView".into(),
                fields: json!({ "IdleCutscenes": idle, "NextWaypoints": [{}, {}] }),
            }],
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            group: None,
        };
        w.scene.cache = Some(std::rc::Rc::new(SceneCache {
            schema: String::new(),
            area: String::new(),
            name: String::new(),
            source: String::new(),
            zones: Vec::new(),
            views: vec![view("p1", json!([])), view("p2", json!(["c"]))],
        }));
        let cutscene = |point: &str| {
            json!({ "k": "obs", "t": 5, "ev": "roam_point", "m": "SelectCutscene", "point": point,
                    "idle": [], "n": 0, "caller": "controller", "unit": "u1" })
        };
        let mut audit = |records: &[Value]| {
            let mut w2 = std::mem::take(&mut w.coverage);
            super::audit_roaming(&mut w, records);
            std::mem::swap(&mut w.coverage, &mut w2);
            w2.unsupported.keys().cloned().collect::<Vec<_>>()
        };
        assert!(audit(&[roam("SelectNextPoint", 2, "controller"), cutscene("p1")]).is_empty());
        for (what, r) in [
            ("an idle cutscene in the cache", cutscene("p2")),
            ("unknown waypoint", cutscene("p3")),
            ("candidates unlike the cache", roam("SelectNextPoint", 1, "controller")),
            ("advance", roam("SelectNextPoint", 2, "advance")),
            (
                "no caller",
                json!({ "k": "obs", "t": 5, "ev": "roam_point", "m": "SelectNextPoint", "point": "p1", "idle": [], "n": 2 }),
            ),
            (
                "unknown unit",
                json!({ "k": "obs", "t": 5, "ev": "roam_point", "m": "SelectNextPoint", "point": "p1",
                        "idle": [], "n": 2, "caller": "controller", "unit": "u9" }),
            ),
            (
                "roll with idle",
                json!({ "k": "obs", "t": 5, "ev": "roam_roll", "unit": "u1", "settings": true, "idle": ["c"] }),
            ),
            // The recorded run-created "n1" is the sim's "n2" (no roaming part), not the sim's
            // own "n1".
            (
                "a run-created id resolved by its match",
                json!({ "k": "obs", "t": 5, "ev": "roam_point", "m": "SelectNextPoint", "point": "p1",
                        "idle": [], "n": 2, "caller": "controller", "unit": "n1" }),
            ),
        ] {
            assert!(!audit(&[r]).is_empty(), "{what}: not reported");
        }
        assert!(
            audit(&[json!({ "k": "obs", "t": 5, "ev": "roam_roll", "unit": "u1", "settings": true, "idle": [] })])
                .is_empty()
        );
        // At their ticks: a producer naming a unit the run creates only later is reported then,
        // and one in a tick the controller didn't run (or the run didn't reach) is reported.
        let late = json!({ "k": "obs", "t": 1, "ev": "roam_roll", "unit": "u7", "settings": true, "idle": [] });
        let early = json!({ "k": "obs", "t": 0, "ev": "roam_roll", "unit": "u1", "settings": true, "idle": [] });
        w.recorded_roaming = Some(Box::new(super::recorded_roaming(&[early.clone(), late])));
        let before = std::mem::take(&mut w.coverage);
        super::roaming_tick(&mut w, 0);
        assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
        super::roaming_tick(&mut w, 1);
        assert!(!w.coverage.unsupported.is_empty());
        w.coverage = crate::registry::Coverage::default();
        w.recorded_roaming = Some(Box::new(super::recorded_roaming(&[early])));
        super::roaming_tick(&mut w, 2);
        assert!(!w.coverage.unsupported.is_empty(), "a skipped tick");
        w.coverage = before;
    }

    /// The groups' recorded states: read only when declared, strictly; after each groups tick
    /// the sim's state must equal the latest recorded one, and a record in a tick the sim's
    /// controller didn't run is reported.
    #[test]
    fn recorded_groups_are_checked_after_each_tick() {
        use crate::combat::groups::{
            RecordedGroups, check_recorded, recorded_finish, start_without_memory,
        };
        use crate::combat::turns::tests::{unit, world};
        let begin = json!({ "k": "begin", "observers": ["group_ticks"] });
        let state = |t: i64, units: Value| {
            json!({ "k": "obs", "t": t, "ev": "grp_tick",
                    "groups": [{ "id": "g", "units": units, "in_combat": 0, "timer": 0.0 }],
                    "units": [["a", "g"], ["b", "g"]] })
        };
        assert!(RecordedGroups::of(&[state(0, json!(["a", "b"]))]).unwrap().is_none());
        assert!(
            RecordedGroups::of(&[begin.clone(), state(0, json!(["a"])), state(0, json!(["a"]))])
                .is_err()
        );
        assert!(
            RecordedGroups::of(&[
                begin.clone(),
                json!({ "k": "obs", "t": 0, "ev": "grp_tick", "groups": 3 })
            ])
            .is_err()
        );
        let run = |records: &[Value], ticks: &[i64]| {
            let mut w = world();
            unit(&mut w, "a", Some("g"), 0);
            unit(&mut w, "b", Some("g"), 0);
            start_without_memory(&mut w);
            assert!(w.combat.groups.list.iter().all(|g| w.combat.groups.get(*g).memory.is_none()));
            w.recorded_groups = RecordedGroups::of(records).unwrap().map(Box::new);
            for t in ticks {
                crate::combat::groups::tick(&mut w);
                check_recorded(&mut w, *t);
            }
            recorded_finish(&mut w, *ticks.last().unwrap());
            w.coverage.unsupported.keys().cloned().collect::<Vec<_>>()
        };
        assert!(run(&[begin.clone(), state(0, json!(["a", "b"]))], &[0, 1, 2]).is_empty());
        assert!(!run(&[begin.clone(), state(0, json!(["b", "a"]))], &[0, 1]).is_empty());
        assert!(
            !run(&[begin.clone(), state(0, json!(["a", "b"])), state(1, json!(["a"]))], &[0, 1])
                .is_empty()
        );
        // A record in a tick the sim's controller didn't run (the sim skipped tick 1).
        assert!(
            !run(
                &[begin.clone(), state(0, json!(["a", "b"])), state(1, json!(["a", "b"]))],
                &[0, 2]
            )
            .is_empty()
        );
        // A unit the run created, "y" in the sim and "x" in the recording (announced with its
        // blueprint by the awake records): matched before the renaming, in the first check.
        {
            let bp = wr_content::Guid(0x78);
            let mut w = world();
            unit(&mut w, "a", Some("g"), 0);
            let y = unit(&mut w, "y", Some("g"), 0);
            w.unit_mut(y).expect("unit").blueprint = Some(bp);
            w.recorded_awake = Some(Box::new(crate::spawn::RecordedAwake {
                saved: ["a".to_owned()].into_iter().collect(),
                new_bp: [("x".to_owned(), bp)].into_iter().collect(),
                ..crate::spawn::RecordedAwake::default()
            }));
            start_without_memory(&mut w);
            let rec = json!({ "k": "obs", "t": 0, "ev": "grp_tick",
                    "groups": [{ "id": "g", "units": ["a", "x"], "in_combat": 0, "timer": 0.0 }],
                    "units": [["a", "g"], ["x", "g"]] });
            w.recorded_groups = RecordedGroups::of(&[begin.clone(), rec]).unwrap().map(Box::new);
            crate::combat::groups::tick(&mut w);
            check_recorded(&mut w, 0);
            assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
        }
        // A record after the run's last tick: the run didn't reach it.
        assert!(
            !run(&[begin, state(0, json!(["a", "b"])), state(2, json!(["a", "b"]))], &[0, 1])
                .is_empty()
        );
    }

    /// `RuleCalculateAbilityParams.Result` is compared on its stored members: the oracle's
    /// snapshot also has the `IsChanged` getter, the sim's record doesn't.
    #[test]
    fn ability_params_results_compare_their_stored_members() {
        let p = json!({ "SpellSource": 2, "CasterLevel": 3, "SpellLevel": 1, "DC": 12,
            "Concentration": 3, "Metamagic": 0, "RankBonus": 0 });
        let mut oracle = p.clone();
        oracle["IsChanged"] = json!(true);
        let class = "RuleCalculateAbilityParams";
        let fields = |r: serde_json::Value| json!({ "Result": r }).as_object().cloned().unwrap();
        let ours = super::rule_result(class, &fields(p.clone()));
        assert_eq!(ours["Result"], p, "the result is compared, whole");
        assert_eq!(ours, super::rule_result(class, &fields(oracle)), "IsChanged is not");
        let mut other = p;
        other["DC"] = json!(13);
        assert_ne!(ours, super::rule_result(class, &fields(other)), "a stored member is");
    }
}
