//! Cutscenes as T1 events (PLAN.md's contract: positions, facing and timing within
//! tolerance; the same T0 events). Full playback (`CutscenePlayerData`, tracks, gates,
//! command timing) isn't ported (decompiled, not copied: `Kingmaker.AreaLogic.Cutscenes/
//! {CutscenePlayerData,CutscenePlayerGateData,CutscenePlayerTrackData,Gate,Track,CommandBase,
//! CommandActivateGate}.cs` and the commands of `Kingmaker.AreaLogic.Cutscenes.Commands`).
//! What runs instead:
//!
//! - `PlayCutscene`/`StopCutscene` record a `t1` event (cutscene, parameters' units).
//! - The cutscene's command closure (its tracks, the gates they end in or signal) is
//!   classified. Commands whose only effects are timing-coupled presentation (moving and
//!   translocating units, camera, animations, delays, fades, control locks, look-at) are
//!   **stubbed**: listed in the coverage `stubbed` map.
//! - `CommandAction`s' `HideUnit` and `Spawn` are T0 and run, in the order the cutscene
//!   reaches them, at the cutscene controller's next tick. That order is the original's only
//!   when it doesn't depend on timing or on state the classification doesn't evaluate, so a
//!   closure with T0 actions must have: every T0 track on one chain of gates (a track finishes
//!   before its `And` end gate starts its tracks), no T0 action on a continuous or repeating
//!   track (a gate's activation force-stops it), only `AllTracks` and `And` gates, no gate
//!   reactivation or command signals, no `CommandRequireUnits`, and every gate with T0 tracks
//!   reached. Anything else is reported.
//! - A `RandomTrack` gate (`SelectTrackToStart` draws from the global RNG which of its tracks
//!   start) is reported in a closure with T0 actions. Without them its choice only picks
//!   presentation, but the sim can't make the draw itself (when the gate activates is cutscene
//!   timing): it is reported unless the run takes the cutscene's progress from a recording
//!   ([`RecordedCutscene`]: WrathOracle's `gate_track` and `cs_action`), which then makes each
//!   draw at its recorded place, for the gates it accepted only.
//! - With such a recording, a cutscene's T0 actions run when the recording ran their
//!   `CommandAction`, at the cutscene controller's place in the tick, and in order.
//! - `CommandLockControls` ticks every unit's buffs at its start and postpones them at its stop
//!   (`UnitBuffsController.TickAll`/`PostponeAll`), and its lock puts the game in Cutscene mode
//!   (no `UnitBuffsController`). When it runs is cutscene timing, so it is reported unless the
//!   recording also has the locks (WrathOracle's `cs_lock`): then each start and stop runs at
//!   its recorded place, and every tick's mode is checked against the recording's.
//! - A cutscene's `OnStopped` actions and a fade's in-fade actions are accepted when they are
//!   presentation (a cutscene they start is classified in turn); anything else is reported.
//! - With the recording's cutscene commands too (WrathOracle's `cs_dialog` and `cs_entry`), a
//!   `CommandStartDialog` without a speaker or an evaluator is a T0 step: its
//!   `StartDialogWithoutTarget` runs where the recording ran the command (the dialogue then
//!   blocks the track, which only delays the track's later commands: their runs are recorded
//!   too). A command whose `EntryCondition` fails with `SkipCommand` is skipped where the
//!   recording checked it (its conditions can read positions). And T0 steps on parallel tracks
//!   are accepted: each recorded run takes the first step left on its own track.
//! - Otherwise reported: a command with an `EntryCondition` (`StartNextCommand` checks it and
//!   applies `OnFail`), `CommandStartDialog` (the dialogue blocks its track and isn't started by
//!   the sim).
//! - Always reported: a cutscene started by a cutscene with T0 actions of its own, and a
//!   `CommandDismountSpan` while any unit has a rider part.
//! - Any other command or nested action makes the run invalid (unsupported).
//!
//! `CommandDelay.OnRun` draws from the global RNG (`Range(float,float)`); those draws are
//! T1-coupled (they shift with frame timing in the original, `docs/poc1/oracle.md`) and
//! aren't part of the T0 tape. So is pausing (`HasNoActiveAnchors`): it delays a cutscene.

use std::rc::Rc;

use serde_json::json;
use wr_content::Guid;

use super::Result;
use crate::content::{Blueprint, View};
use crate::world::World;

/// Commands with presentation-only effects.
const T1_COMMANDS: &[&str] = &[
    "CommandLockControls",
    "CommandControlCamera",
    "CommandMoveUnit",
    "CommandDelay",
    "CommandUnitPlayCutsceneAnimation",
    "CommandUnitLookAt",
    "CommandDismountSpan",
    "CommandFadeout",
    "CutsceneCommandFadeout",
    // `CommandCameraShake` is camera presentation; `CommandMarkUnitControlled` only marks a
    // unit as controlled (`GetControlledUnit`: control arbitration is not modelled).
    "CommandCameraShake",
    "CommandMarkUnitControlled",
    // Flow control within the cutscene (`ReactivateGate`, skipping to the end gate when a
    // required unit is missing) and a fog-of-war revealer (presentation).
    "CommandActivateGate",
    "CommandRequireUnits",
    "CommandAddFogOfWarRevealer",
];

/// Actions inside `CommandAction`s that only manage cutscenes, move units or play sounds.
const T1_ACTIONS: &[&str] =
    &["StopCutscene", "PlayCutscene", "TranslocatePlayer", "Play3DSound", "SpawnFx"];

/// Actions inside `CommandAction`s with T0 effects the sim ports: they run, in the order the
/// cutscene reaches them, at the cutscene's start (the timing of a cutscene is T1).
/// (`TranslocateUnit` clears the unit's `IsExtra`, `MarkNoExtra`.)
const T0_ACTIONS: &[&str] = &["HideUnit", "Spawn", "TranslocateUnit"];

/// `Gate.ActivationModeType`.
const ACTIVATION_FIRST_TRACK: i64 = 1;
const ACTIVATION_RANDOM_TRACK: i64 = 2;
/// `Operation.Or`.
const OP_OR: i64 = 1;
/// `CommandBase.EntryFailResult.SkipCommand`.
const ENTRY_SKIP_COMMAND: i64 = 2;

/// A step with T0 effects: a `CommandAction` action, or (with the recording's cutscene commands)
/// a `CommandStartDialog`'s start.
#[derive(Debug, Clone)]
pub struct T0Step {
    pub command: Rc<Blueprint>,
    pub kind: StepKind,
    /// The command's occurrence in the closure's walk (a track can run one command blueprint
    /// twice: each run has its own steps).
    pub invocation: usize,
    /// The track of the closure's walk the command is on.
    pub track: usize,
    /// The T0 tracks that finish before this one starts (through the gates between them).
    pub after: Vec<usize>,
}

/// What a [`T0Step`] runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// The `CommandAction`'s action at this index.
    Action(usize),
    /// `CommandStartDialog.OnRun` (no speaker, no evaluator).
    StartDialog,
}

/// What a cutscene's closure contains.
#[derive(Debug, Default, Clone)]
pub struct Classified {
    pub t1_commands: Vec<&'static str>,
    /// Actions inside `CommandAction`s that only move units, play sounds or manage
    /// cutscenes (T1: positions and timing).
    pub t1_actions: Vec<&'static str>,
    /// The T0 actions in the order the cutscene reaches them: the T0 tracks along their chain
    /// of gates, each track's commands in order.
    pub t0_steps: Vec<T0Step>,
    pub dialogs: Vec<Guid>,
    pub unsupported: Vec<String>,
    pub cutscenes: Vec<Guid>,
    /// `RandomTrack` gates in closures without T0 actions, with the cutscene whose player
    /// activates them: their draws only pick presentation ([`settle_recorded`]).
    pub random_gates: Vec<(Guid, Guid)>,
}

/// A track of a gate of the closure.
#[derive(Debug, Default)]
struct TrackNode {
    gate: Guid,
    end: Option<Guid>,
    /// `Track.IsContinuous`: `Repeat`, or the last command `IsContinuous`. Such a track
    /// never signals its end gate, counts as done for it, and is force-stopped when the end
    /// gate activates.
    continuous: bool,
    repeat: bool,
    steps: Vec<T0Step>,
    dialogs: usize,
}

/// The walk's state: the gates' tracks and the strict-mode findings (reported only when
/// the closure has T0 actions).
#[derive(Default)]
struct Walk {
    seen: Vec<Guid>,
    gates: Vec<Guid>,
    tracks: Vec<TrackNode>,
    strict: Vec<String>,
    /// The cutscenes being classified, outermost first (a cutscene that starts one of them
    /// isn't classified again).
    playing: Vec<Guid>,
    /// Command occurrences walked so far ([`T0Step::invocation`]).
    invocations: usize,
    /// Cutscenes whose `OnStopped` a `StopCutscene` already classified.
    stopped: Vec<Guid>,
}

/// Walks a cutscene's tracks, end gates and the gates its commands activate or signal.
pub fn classify(w: &World, root: Guid) -> Classified {
    classify_in(w, root, Vec::new())
}

fn classify_in(w: &World, root: Guid, mut playing: Vec<Guid>) -> Classified {
    let mut out = Classified::default();
    playing.push(root);
    let mut walk = Walk { playing, ..Walk::default() };
    walk_gate(w, root, &mut out, &mut walk);
    let t0_tracks: Vec<usize> =
        (0..walk.tracks.len()).filter(|&t| has_t0(&walk.tracks[t])).collect();
    if t0_tracks.is_empty() {
        return out;
    }
    // This closure's own random gates are reported with the strict findings below.
    out.random_gates.retain(|(c, _)| *c != root);
    // The T0 actions' order and occurrence rest on the structure: report what makes them
    // depend on timing or on unevaluated state.
    for s in std::mem::take(&mut walk.strict) {
        push_unique(&mut out.unsupported, s);
    }
    let activated = activated_gates(&walk, root);
    for &t in &t0_tracks {
        let tr = &walk.tracks[t];
        if tr.continuous {
            push_unique(
                &mut out.unsupported,
                "T0 commands on a continuous or repeating track (force-stopped by its end gate)"
                    .into(),
            );
        }
        if !activated.contains(&tr.gate) {
            push_unique(
                &mut out.unsupported,
                "T0 commands in a gate that no finishing track signals".into(),
            );
        }
    }
    // The T0 tracks must follow one another: each pair ordered by the gates between them.
    let reach = |a: usize, b: usize| reaches(&walk, a, b);
    let mut ordered = true;
    for (i, &a) in t0_tracks.iter().enumerate() {
        for &b in &t0_tracks[i + 1..] {
            if !reach(a, b) && !reach(b, a) {
                ordered = false;
            }
        }
    }
    // With the commands' runs recorded, the recording orders parallel tracks, as long as each
    // recorded run names its track: a T0 command on several tracks doesn't.
    if w.scene.recorded_commands {
        let shared = t0_tracks.iter().enumerate().any(|(i, &a)| {
            t0_tracks[i + 1..].iter().any(|&b| {
                walk.tracks[a]
                    .steps
                    .iter()
                    .any(|s| walk.tracks[b].steps.iter().any(|o| o.command.guid == s.command.guid))
            })
        });
        if shared {
            push_unique(
                &mut out.unsupported,
                "a T0 command on several tracks (its recorded runs don't name the track)".into(),
            );
        }
    }
    if !ordered && !w.scene.recorded_commands {
        push_unique(
            &mut out.unsupported,
            "T0 commands in parallel tracks (order is timing-dependent)".into(),
        );
    }
    let mut order = t0_tracks;
    order.sort_by(|&a, &b| {
        if reach(a, b) {
            std::cmp::Ordering::Less
        } else if reach(b, a) {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    let before: Vec<(usize, Vec<usize>)> = order
        .iter()
        .map(|&t| (t, order.iter().copied().filter(|&a| a != t && reach(a, t)).collect()))
        .collect();
    out.t0_steps = before
        .into_iter()
        .flat_map(|(t, after)| {
            walk.tracks[t].steps.iter().cloned().map(move |mut s| {
                s.after.clone_from(&after);
                s
            })
        })
        .collect();
    out
}

fn has_t0(t: &TrackNode) -> bool {
    !t.steps.is_empty() || t.dialogs > 0
}

fn push_unique(v: &mut Vec<String>, s: String) {
    if !v.contains(&s) {
        v.push(s);
    }
}

/// The gates the root's activation leads to: a gate activates when one of its finishing
/// (non-continuous) incoming tracks signals it and every such incoming track belongs to an
/// activated gate (`Operation.And`: the incoming tracks are finished or continuous).
fn activated_gates(walk: &Walk, root: Guid) -> Vec<Guid> {
    let mut on = vec![root];
    loop {
        let mut changed = false;
        for &g in &walk.gates {
            if on.contains(&g) {
                continue;
            }
            let incoming: Vec<&TrackNode> =
                walk.tracks.iter().filter(|t| t.end == Some(g) && !t.continuous).collect();
            if !incoming.is_empty() && incoming.iter().all(|t| on.contains(&t.gate)) {
                on.push(g);
                changed = true;
            }
        }
        if !changed {
            return on;
        }
    }
}

/// Track `b` starts only after track `a` finished: `b` belongs to a gate that `a`'s end
/// gate leads to, along finishing tracks.
fn reaches(walk: &Walk, a: usize, b: usize) -> bool {
    let mut stack = vec![a];
    let mut seen = vec![a];
    while let Some(t) = stack.pop() {
        let tr = &walk.tracks[t];
        if tr.continuous {
            continue;
        }
        let Some(end) = tr.end else { continue };
        for (n, next) in walk.tracks.iter().enumerate() {
            if next.gate != end || seen.contains(&n) {
                continue;
            }
            if n == b {
                return true;
            }
            seen.push(n);
            stack.push(n);
        }
    }
    false
}

/// A condition whose check only reads (the recorded result can stand for it: one that draws, a
/// `DiceFormulaEvaluator` say, can't): `UnitInScriptZone` with a read-only unit evaluator and a
/// scene map object, `FlagUnlocked`, `EtudeStatus`.
fn pure_condition(v: View<'_>) -> bool {
    match v.type_name() {
        "UnitInScriptZone" => {
            v.obj("Unit").is_some_and(super::actions::pure_evaluator)
                && v.obj("ScriptZone").is_some_and(|z| z.type_name() == "MapObjectFromScene")
        }
        "FlagUnlocked" | "EtudeStatus" => true,
        _ => false,
    }
}

/// `CommandBase.HasConditions`: an `EntryCondition` with conditions.
fn has_entry_condition(v: View<'_>) -> bool {
    v.obj("EntryCondition").is_some_and(|c| !c.objs("Conditions").is_empty())
}

/// `CommandBase.IsContinuous` of the commands the classification accepts.
fn is_continuous(ty: &str, v: View<'_>) -> bool {
    match ty {
        "CommandLockControls" | "CommandDismountSpan" | "CommandAddFogOfWarRevealer" => true,
        "CommandUnitLookAt" | "CommandCameraShake" | "CutsceneCommandFadeout" => {
            v.bool("m_Continuous")
        }
        "CommandMarkUnitControlled" => v.f32("UnmarkAfter") <= 0.0,
        "CommandUnitPlayCutsceneAnimation" => v.bool("m_ClipIsLooped") && !v.bool("m_FiniteLoop"),
        _ => false,
    }
}

/// The gate a `CommandSignalData` names (`m_Gate`).
fn signal_gate(v: View<'_>, field: &str) -> Option<Guid> {
    v.obj(field).and_then(|s| s.guid("m_Gate"))
}

fn walk_gate(w: &World, g: Guid, out: &mut Classified, walk: &mut Walk) {
    if walk.seen.contains(&g) {
        return;
    }
    walk.seen.push(g);
    let Some(b) = w.content.get(g) else {
        out.unsupported.push(format!("cutscene element {g} missing"));
        return;
    };
    let ty = b.type_name();
    if ty != "Cutscene" && ty != "Gate" {
        out.unsupported.push(format!("cutscene gate of type {ty}"));
        return;
    }
    if ty == "Cutscene" {
        out.cutscenes.push(g);
        // `Cutscene.OnStopped` runs when the player finishes or stops
        // (`CutscenePlayerData.Tick`/`Stop`).
        let stopped = b.view().obj("OnStopped").map(|l| l.objs("Actions")).unwrap_or_default();
        side_actions(w, stopped, "Cutscene.OnStopped", out, walk);
    }
    walk.gates.push(g);
    let v = b.view();
    match v.int("m_ActivationMode") {
        ACTIVATION_RANDOM_TRACK => {
            if let Some(&root) = walk.playing.last() {
                out.random_gates.push((root, g));
            }
            push_unique(
                &mut walk.strict,
                "Gate RandomTrack (SelectTrackToStart draws from the global RNG)".into(),
            );
        }
        ACTIVATION_FIRST_TRACK => push_unique(
            &mut walk.strict,
            "Gate FirstTrack (the first track whose entry condition holds)".into(),
        ),
        _ => {}
    }
    if v.int("m_Op") == OP_OR {
        push_unique(&mut walk.strict, "Gate Or (the first signal force-stops the others)".into());
    }
    let mut ends = Vec::new();
    for t in v.objs("m_Tracks") {
        let tv = View(t);
        let mut node = TrackNode {
            gate: g,
            end: tv.guid("m_EndGate"),
            repeat: tv.bool("m_Repeat"),
            ..TrackNode::default()
        };
        let mut last_continuous = false;
        let mut follow = Vec::new();
        for c in tv.guids("m_Commands") {
            last_continuous = command(w, c, &mut node, out, walk, &mut follow);
        }
        node.continuous = node.repeat || last_continuous;
        if let Some(e) = node.end {
            ends.push(e);
        }
        walk.tracks.push(node);
        for f in follow {
            walk_gate(w, f, out, walk);
        }
    }
    for e in ends {
        walk_gate(w, e, out, walk);
    }
}

/// One command of a track; returns its `IsContinuous`. Gates it activates or signals are
/// pushed to `follow`.
fn command(
    w: &World,
    g: Guid,
    node: &mut TrackNode,
    out: &mut Classified,
    walk: &mut Walk,
    follow: &mut Vec<Guid>,
) -> bool {
    let Some(b) = w.content.get(g) else {
        out.unsupported.push(format!("cutscene element {g} missing"));
        return false;
    };
    let ty = b.type_name();
    let v = b.view();
    // `StartNextCommand` checks the condition; `SkipCommand` skips the command, which the
    // recorded checks place (`cs_entry`). The other results stop or drop the track.
    if has_entry_condition(v)
        && !(w.scene.recorded_commands
            && v.int("OnFail") == ENTRY_SKIP_COMMAND
            && v.obj("EntryCondition")
                .is_some_and(|c| c.objs("Conditions").iter().all(|o| pure_condition(View(o)))))
    {
        push_unique(
            &mut out.unsupported,
            "cutscene command with an EntryCondition (StartNextCommand's OnFail)".into(),
        );
    }
    walk.invocations += 1;
    let invocation = walk.invocations;
    // The node is pushed after its commands.
    let track = walk.tracks.len();
    if let Some(t1) = T1_COMMANDS.iter().find(|c| crate::content::is_a(b.obj.ty, c)) {
        out.t1_commands.push(t1);
        // `RunAway`: `UnitRunAway.DoRunAway` takes the unit out of the game or destroys it
        // (and `BecameStalker` changes the player's state): not presentation.
        if *t1 == "CommandMoveUnit" && v.bool("RunAway") {
            push_unique(&mut out.unsupported, "CommandMoveUnit with RunAway".into());
        }
        // A command that controls a unit (`GetControlledUnit`) marks it at its start, which
        // revives it in Cutscene mode when it's unconscious or dead but not finally
        // (`CutsceneControlledUnit.ReviveUnsconscious`): with such a unit around, not
        // presentation.
        if matches!(
            *t1,
            "CommandMarkUnitControlled"
                | "CommandMoveUnit"
                | "CommandUnitLookAt"
                | "CommandUnitPlayCutsceneAnimation"
        ) && w.entities.iter().any(|e| match &e.kind {
            crate::world::EntityKind::Unit(u) => u.life_state != 0 && !u.finally_dead,
            _ => false,
        }) {
            push_unique(
                &mut out.unsupported,
                "a command controlling a unit while one is unconscious or dead (revival)".into(),
            );
        }
        match *t1 {
            // `ReactivateGate`: the gate's tracks run again.
            "CommandActivateGate" => {
                push_unique(&mut walk.strict, "CommandActivateGate (a gate's tracks rerun)".into());
                follow.extend(signal_gate(v, "SignalData"));
            }
            // `ForceGoToEndGate` when a required unit is missing.
            "CommandRequireUnits" => push_unique(
                &mut walk.strict,
                "CommandRequireUnits (skips to the end gate when a unit is missing)".into(),
            ),
            // A continuous look-at or fade signals a gate when it turns or fades (timing).
            "CommandUnitLookAt" | "CutsceneCommandFadeout" if v.bool("m_Continuous") => {
                let field = if *t1 == "CommandUnitLookAt" { "m_OnTurned" } else { "m_OnFaded" };
                if let Some(s) = signal_gate(v, field) {
                    push_unique(&mut walk.strict, format!("{t1}: signals a gate"));
                    follow.push(s);
                }
            }
            _ => {}
        }
        // `CutsceneCommandFadeout.m_InFadeActions` run at half the fade's lifetime.
        if *t1 == "CutsceneCommandFadeout" {
            let actions = v.obj("m_InFadeActions").map(|l| l.objs("Actions")).unwrap_or_default();
            side_actions(w, actions, "CutsceneCommandFadeout in-fade", out, walk);
        }
        // `CommandDismountSpan` dismounts riders (`UnitPartRider`) and remounts them at its
        // end; with no rider anywhere it does nothing.
        if *t1 == "CommandDismountSpan"
            && w.entities.iter().any(|e| e.parts.iter().any(|p| p.clr == "UnitPartRider"))
        {
            push_unique(&mut out.unsupported, "CommandDismountSpan with a rider".into());
        }
        return is_continuous(t1, v);
    }
    match ty {
        // `CommandStartDialog` blocks its track until the dialogue ends.
        "CommandStartDialog" => {
            if v.obj("Speaker").is_some() || v.obj("DialogEvaluator").is_some() {
                out.unsupported.push("CommandStartDialog with speaker or evaluator".into());
            }
            match v.guid("m_Dialog") {
                Some(d) => out.dialogs.push(d),
                // `OnRun` logs that no dialogue was found and finishes.
                None => push_unique(
                    &mut out.unsupported,
                    "CommandStartDialog without a dialogue".into(),
                ),
            }
            node.dialogs += 1;
            if w.scene.recorded_commands {
                node.steps.push(T0Step {
                    command: b.clone(),
                    kind: StepKind::StartDialog,
                    invocation,
                    track,
                    after: Vec::new(),
                });
            }
        }
        "CommandAction" => {
            // A start of another cutscene draws its root gate inside the action list, before
            // any T0 action after it: the sim's steps don't keep that order.
            let acts: Vec<&str> = v
                .obj("Action")
                .map(|l| l.objs("Actions"))
                .unwrap_or_default()
                .iter()
                .map(|a| View(a).type_name())
                .collect();
            if acts.contains(&"PlayCutscene") && acts.iter().any(|a| T0_ACTIONS.contains(a)) {
                push_unique(
                    &mut out.unsupported,
                    "CommandAction that starts a cutscene and runs T0 actions".into(),
                );
            }
            for (i, a) in v
                .obj("Action")
                .map(|l| l.objs("Actions"))
                .unwrap_or_default()
                .into_iter()
                .enumerate()
            {
                let av = View(a);
                let at = av.type_name();
                if T1_ACTIONS.contains(&at) {
                    out.t1_actions.push(at);
                } else if at == "TranslocateUnit" && translocate_is_presentation(w, av) {
                    out.t1_actions.push("TranslocateUnit");
                } else if T0_ACTIONS.contains(&at) {
                    node.steps.push(T0Step {
                        command: b.clone(),
                        kind: StepKind::Action(i),
                        invocation,
                        track,
                        after: Vec::new(),
                    });
                } else {
                    out.unsupported.push(format!("CommandAction action {at}"));
                }
                nested_play(w, av, out, walk);
                stopped_target(w, av, out, walk);
            }
        }
        _ => out.unsupported.push(format!("cutscene command {ty}")),
    }
    false
}

/// A `PlayCutscene` action: the cutscene it starts plays on its own player. Its T1 content is
/// classified here; T0 content of its own isn't ordered with this one's (reported).
fn nested_play(w: &World, av: View<'_>, out: &mut Classified, walk: &mut Walk) {
    if av.type_name() != "PlayCutscene" {
        return;
    }
    let Some(c) = av.guid("m_Cutscene") else { return };
    if walk.seen.contains(&c) || walk.playing.contains(&c) {
        return;
    }
    walk.seen.push(c);
    let mut nested = classify_in(w, c, walk.playing.clone());
    if !nested.t0_steps.is_empty() || !nested.dialogs.is_empty() {
        push_unique(&mut out.unsupported, "a cutscene started by a cutscene has T0 actions".into());
    }
    out.t1_commands.append(&mut nested.t1_commands);
    out.t1_actions.append(&mut nested.t1_actions);
    out.cutscenes.append(&mut nested.cutscenes);
    out.random_gates.append(&mut nested.random_gates);
    for u in nested.unsupported {
        push_unique(&mut out.unsupported, u);
    }
}

/// `TranslocateUnit` changes nothing but a position when it has none to go to, or when its unit
/// (a spawner's, resolved now) isn't an extra: `MarkNoExtra` is its only T0 effect.
fn translocate_is_presentation(w: &World, av: View<'_>) -> bool {
    if av.obj("translocatePosition").is_none() && av.obj("translocatePositionEvaluator").is_none() {
        return true;
    }
    let Some(ev) = av.obj("Unit") else { return false };
    if ev.type_name() != "UnitFromSpawner" {
        return false;
    }
    let id = ev.obj("Spawner").and_then(|s| s.str("UniqueId").or_else(|| s.str("_entity_id")));
    id.and_then(|id| super::evaluators::spawned_unit(w, id))
        .and_then(|e| w.unit(e))
        .is_some_and(|u| !u.is_extra)
}

/// A `StopCutscene` action: the cutscene it stops runs its `OnStopped`, classified here; one
/// that stops every cutscene of a unit (`m_AllCutscenesFromUnit`) names none: reported.
fn stopped_target(w: &World, av: View<'_>, out: &mut Classified, walk: &mut Walk) {
    if av.type_name() != "StopCutscene" {
        return;
    }
    if av.bool("m_AllCutscenesFromUnit") {
        push_unique(&mut out.unsupported, "StopCutscene of all a unit's cutscenes".into());
        return;
    }
    let Some(c) = av.guid("m_Cutscene") else { return };
    if walk.stopped.contains(&c) {
        return;
    }
    walk.stopped.push(c);
    let Some(b) = w.content.get(c) else {
        push_unique(&mut out.unsupported, format!("cutscene element {c} missing"));
        return;
    };
    let stopped = b.view().obj("OnStopped").map(|l| l.objs("Actions")).unwrap_or_default();
    side_actions(w, stopped, "a stopped cutscene's OnStopped", out, walk);
}

/// An action list that runs outside the tracks' command order (a fade's in-fade actions, a
/// cutscene's `OnStopped`): presentation actions are accepted (a cutscene one starts is
/// classified in turn); anything else is reported, its timing being outside the T0 order.
fn side_actions(
    w: &World,
    actions: Vec<&wr_content::node::Object>,
    what: &str,
    out: &mut Classified,
    walk: &mut Walk,
) {
    side_actions_in(w, actions, what, out, walk, false);
}

/// [`side_actions`]; `recorded`: the list's run has a recorded place (a recorded command), so
/// a start in it can take its draw there.
fn side_actions_in(
    w: &World,
    actions: Vec<&wr_content::node::Object>,
    what: &str,
    out: &mut Classified,
    walk: &mut Walk,
    recorded: bool,
) {
    for a in actions {
        let av = View(a);
        let at = av.type_name();
        if T1_ACTIONS.contains(&at) {
            out.t1_actions.push(at);
            // A start here (outside the tracks' commands) has no recorded place: a random root's
            // draw couldn't be placed.
            if !recorded
                && at == "PlayCutscene"
                && av
                    .guid("m_Cutscene")
                    .and_then(|c| w.content.get(c))
                    .is_some_and(|b| b.view().int("m_ActivationMode") == ACTIVATION_RANDOM_TRACK)
            {
                push_unique(
                    &mut out.unsupported,
                    format!("{what} action PlayCutscene of a random-track cutscene"),
                );
            }
            nested_play(w, av, out, walk);
            stopped_target(w, av, out, walk);
        } else if at == "TranslocateUnit" && translocate_is_presentation(w, av) {
            out.t1_actions.push("TranslocateUnit");
        } else {
            push_unique(&mut out.unsupported, format!("{what} action {at}"));
        }
    }
}

/// What a closure needs from a recording: its random gates are accepted when the run takes
/// cutscene progress from one (the recording then makes their draws), its control locks when
/// the recording also has the locks; else reported.
pub fn settle_recorded(w: &mut World, cls: &Classified) {
    w.scene.classified_cutscenes.extend(cls.cutscenes.iter().copied());
    if cls.t1_commands.contains(&"CommandLockControls") && !w.scene.recorded_locks {
        w.coverage.unsupported(
            "cutscene: CommandLockControls without recorded locks (its buffs tick and postpone, Cutscene mode)",
        );
    }
    for &(c, g) in &cls.random_gates {
        if w.scene.recorded_cutscenes {
            w.scene.random_gates_t1.insert((c, g));
        } else {
            w.coverage.unsupported(
                "cutscene: Gate RandomTrack (SelectTrackToStart draws from the global RNG)",
            );
        }
    }
}

/// A recorded step of a cutscene's progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CutsceneEvent {
    /// `CutscenePlayerGateData.SelectTrackToStart` of a random gate: the player's cutscene, the
    /// gate, its started tracks' count, and whether it ran inside the player's `Start` (the
    /// root gate's first activation).
    Gate { cutscene: Guid, gate: Guid, started: i64, start: bool },
    /// `CommandAction.OnRun`: the player's cutscene and the command.
    Action { cutscene: Guid, command: Guid },
    /// `CommandLockControls.OnRun` (`on`) or `OnStop`: the player's cutscene and the command;
    /// `start` is the cutscene whose player's `Start` it ran inside (`StopCopies`), if any;
    /// `controller` whether it ran inside `CutsceneController.Tick`.
    Lock { cutscene: Guid, command: Guid, on: bool, start: Option<Guid>, controller: bool },
    /// `CommandStartDialog.OnRun`: the player's cutscene and the command, placed as `Lock`.
    Dialog { cutscene: Guid, command: Guid, start: Option<Guid>, controller: bool },
    /// `CutscenePlayerTrackData.StartNextCommand`'s check of a command's `EntryCondition`: the
    /// player's cutscene, the command, whether it held, its `OnFail`, placed as `Lock`.
    Entry {
        cutscene: Guid,
        command: Guid,
        passed: bool,
        on_fail: i64,
        start: Option<Guid>,
        controller: bool,
    },
}

/// A cutscene's progress as a recording observed it, per oracle tick, in order.
#[derive(Debug, Default)]
pub struct RecordedCutscene {
    pub ticks: std::collections::BTreeMap<i64, Vec<CutsceneEvent>>,
    /// The current tick's events not consumed yet: what the cutscene controller's place can't
    /// take waits for a later one in the tick (a cutscene's start activates its root gate at
    /// once: `CutscenePlayerData.Start`).
    pub queue: std::collections::VecDeque<CutsceneEvent>,
    /// Cutscenes started (`PlayCutscene`) in the current tick.
    pub started: Vec<Guid>,
    /// Each oracle tick's game mode (`GameModeType`), when the recording has the locks.
    pub modes: std::collections::BTreeMap<i64, i64>,
    /// `ScriptZone.TryAddUnit`'s calls of the main character in order, when the recording has
    /// them (`zone_enter`): its entries (movement is T1), each a `zone:` step's
    /// ([`Self::claim_zone_steps`] keeps only the steps' own).
    pub zones: Option<std::collections::VecDeque<ZoneEntry>>,
    /// The other calls by oracle tick, in order: entries no step makes, which the sim makes at
    /// the zones' place in that tick (a cutscene or a teleport moved the unit: T1).
    pub tick_zones: std::collections::BTreeMap<i64, Vec<ZoneEntry>>,
}

/// A recorded `ScriptZone.TryAddUnit`: the oracle tick, the zone's blueprint, and whether the
/// unit is the main character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneEntry {
    pub tick: i64,
    pub zone: Option<Guid>,
    pub main: bool,
    /// The unit's recorded id.
    pub unit: Option<String>,
    /// The record's place among the recording's entries.
    pub seq: usize,
}

impl RecordedCutscene {
    /// Whether a recording has these observations: its begin record declares the observer
    /// (`cutscene_progress`), so a recording without a single command run still counts.
    #[must_use]
    pub fn present(records: &[serde_json::Value]) -> bool {
        records.iter().find(|r| r["k"] == "begin").is_some_and(|b| {
            b["observers"].as_array().is_some_and(|o| o.iter().any(|x| x == "cutscene_progress"))
        })
    }

    /// Whether the recording also has the cutscene locks (`cutscene_lock`).
    #[must_use]
    pub fn has_locks(records: &[serde_json::Value]) -> bool {
        records.iter().find(|r| r["k"] == "begin").is_some_and(|b| {
            b["observers"].as_array().is_some_and(|o| o.iter().any(|x| x == "cutscene_lock"))
        })
    }

    /// Whether the recording also has the dialogue starts and entry checks of cutscene commands
    /// (`cutscene_commands`).
    #[must_use]
    pub fn has_commands(records: &[serde_json::Value]) -> bool {
        records.iter().find(|r| r["k"] == "begin").is_some_and(|b| {
            b["observers"].as_array().is_some_and(|o| o.iter().any(|x| x == "cutscene_commands"))
        })
    }

    /// # Errors
    ///
    /// A malformed record, or one before any tick.
    pub fn of(records: &[serde_json::Value]) -> std::result::Result<Self, String> {
        let guid = |r: &serde_json::Value, f: &str| {
            r[f].as_str().and_then(Guid::parse).ok_or_else(|| format!("{}: {f} {}", r["ev"], r[f]))
        };
        let mut out = Self::default();
        let locks = Self::has_locks(records);
        let commands = Self::has_commands(records);
        // Where a command ran: inside a player's `Start` (that player's cutscene) or not, and
        // whether inside `CutsceneController.Tick`.
        let place = |r: &serde_json::Value| -> std::result::Result<(Option<Guid>, bool), String> {
            if r["skipping"] != false && r["ev"] != "cs_entry" {
                return Err(format!("{}: skipping {}", r["ev"], r["skipping"]));
            }
            let start = match &r["start"] {
                serde_json::Value::Null => None,
                _ => Some(guid(r, "start")?),
            };
            let controller =
                r["controller"].as_bool().ok_or_else(|| format!("{}: controller", r["ev"]))?;
            Ok((start, controller))
        };
        let zones = records.iter().find(|r| r["k"] == "begin").is_some_and(|b| {
            b["observers"].as_array().is_some_and(|o| o.iter().any(|x| x == "zone_enter"))
        });
        if zones {
            out.zones = Some(std::collections::VecDeque::new());
        }
        let mut tick = None;
        let mut zone_seq = 0;
        for r in records {
            if r["k"] == "tick" {
                let t = r["t"].as_i64().ok_or("a tick record without t")?;
                tick = Some(t);
                if locks {
                    let mode = r["mode"].as_i64().ok_or("a tick record without mode")?;
                    if out.modes.insert(t, mode).is_some() {
                        return Err(format!("two tick records for tick {t}"));
                    }
                }
                continue;
            }
            if r["k"] != "obs" {
                continue;
            }
            let ev = match r["ev"].as_str() {
                Some("cs_action") => {
                    if r["skipping"] != false {
                        return Err(format!("cs_action: skipping {}", r["skipping"]));
                    }
                    CutsceneEvent::Action {
                        cutscene: guid(r, "cutscene")?,
                        command: guid(r, "command")?,
                    }
                }
                Some("gate_track") => CutsceneEvent::Gate {
                    cutscene: guid(r, "cutscene")?,
                    gate: guid(r, "gate")?,
                    started: r["started"].as_i64().ok_or("gate_track: started")?,
                    start: r["start"].as_bool().ok_or("gate_track: start")?,
                },
                Some("cs_lock") if locks => {
                    if r["skipping"] != false {
                        return Err(format!("cs_lock: skipping {}", r["skipping"]));
                    }
                    CutsceneEvent::Lock {
                        cutscene: guid(r, "cutscene")?,
                        command: guid(r, "command")?,
                        on: r["on"].as_bool().ok_or("cs_lock: on")?,
                        start: match &r["start"] {
                            serde_json::Value::Null => None,
                            _ => Some(guid(r, "start")?),
                        },
                        controller: r["controller"].as_bool().ok_or("cs_lock: controller")?,
                    }
                }
                Some("cs_lock") => return Err("cs_lock without the cutscene_lock observer".into()),
                Some("cs_dialog") if commands => {
                    let (start, controller) = place(r)?;
                    CutsceneEvent::Dialog {
                        cutscene: guid(r, "cutscene")?,
                        command: guid(r, "command")?,
                        start,
                        controller,
                    }
                }
                Some("cs_entry") if commands => {
                    let (start, controller) = place(r)?;
                    CutsceneEvent::Entry {
                        cutscene: guid(r, "cutscene")?,
                        command: guid(r, "command")?,
                        passed: r["passed"].as_bool().ok_or("cs_entry: passed")?,
                        on_fail: r["on_fail"].as_i64().ok_or("cs_entry: on_fail")?,
                        start,
                        controller,
                    }
                }
                Some("cs_dialog" | "cs_entry") => {
                    return Err(format!("{} without the cutscene_commands observer", r["ev"]));
                }
                Some("zone_enter") => {
                    let t = tick.ok_or("a zone record before any tick")?;
                    let zone = match &r["zone"] {
                        serde_json::Value::Null => None,
                        _ => Some(guid(r, "zone")?),
                    };
                    let main = r["main"].as_bool().ok_or("zone_enter: main")?;
                    let unit = r["unit"].as_str().map(str::to_owned);
                    let zones =
                        out.zones.as_mut().ok_or("zone_enter without the zone_enter observer")?;
                    let e = ZoneEntry { tick: t, zone, main, unit, seq: zone_seq };
                    zone_seq += 1;
                    if main {
                        zones.push_back(e);
                    } else {
                        out.tick_zones.entry(t).or_default().push(e);
                    }
                    continue;
                }
                _ => continue,
            };
            let t = tick.ok_or("a cutscene record before any tick")?;
            out.ticks.entry(t).or_default().push(ev);
        }
        Ok(out)
    }
}

impl RecordedCutscene {
    /// Keeps as step entries the main character's entries the `zone:` steps make: each step's
    /// zone, in step order, the first entry of it after the previous step's. Every other entry
    /// goes to [`Self::tick_zones`]. A step whose entry isn't found ends the claims (it and the
    /// later steps find none: reported by the step).
    pub fn claim_zone_steps(&mut self, steps: &[String]) {
        let Some(zones) = self.zones.as_mut() else { return };
        let mut all: Vec<ZoneEntry> = zones.drain(..).collect();
        all.extend(std::mem::take(&mut self.tick_zones).into_values().flatten());
        all.sort_by_key(|e| e.seq);
        let mut claimed = vec![false; all.len()];
        let mut from = 0;
        for s in steps {
            let Some(z) = s.strip_prefix("zone:").and_then(Guid::parse) else { continue };
            let Some(i) = (from..all.len()).find(|&i| all[i].main && all[i].zone == Some(z)) else {
                break;
            };
            claimed[i] = true;
            from = i + 1;
        }
        for (e, c) in all.into_iter().zip(claimed) {
            if c {
                zones.push_back(e);
            } else {
                self.tick_zones.entry(e.tick).or_default().push(e);
            }
        }
    }
}

/// The cutscene controller's recorded progress in the oracle tick `tick`, in order, from the
/// controller's place in the tick: a random gate's draw (`Random.Range(0, n)` over its started
/// tracks with commands: an entry condition is reported by the classification) for a gate the
/// sim accepted, a `FirstTrack` gate's selection (no draw); a `CommandAction`'s T0 steps, its
/// invocation's in its cutscene's pending steps. A started count unlike the gate's, a step out
/// of order, or a recorded T0 command the sim has no step for is reported. A gate the sim
/// hasn't accepted yet stops the processing: the rest waits for [`recorded_start`].
pub fn recorded_tick(w: &mut World, tick: i64) {
    // The previous tick's events had its controllers and its inputs (applied after them) to be
    // taken by.
    recorded_tick_end(w);
    let Some(rec) = w.recorded_cs.as_mut() else { return };
    rec.queue = rec.ticks.get(&tick).cloned().unwrap_or_default().into();
    drain_queue(w);
}

/// Processes the queue's events until one the sim can't take yet.
fn drain_queue(w: &mut World) {
    loop {
        let Some(ev) = w.recorded_cs.as_ref().and_then(|r| r.queue.front().cloned()) else {
            return;
        };
        match ev {
            CutsceneEvent::Gate { cutscene, gate, started, start } => {
                let Some(b) = w.content.get(gate) else {
                    w.coverage.unsupported("cutscene: a recorded gate missing");
                    pop_queue(w);
                    continue;
                };
                if b.view().int("m_ActivationMode") != ACTIVATION_RANDOM_TRACK {
                    // `FirstTrack` selects without a draw.
                    pop_queue(w);
                    continue;
                }
                if !w.scene.random_gates_t1.contains(&(cutscene, gate)) {
                    return;
                }
                // A draw inside a player's start waits for the sim's start of that cutscene
                // (`PlayCutscene`); a later activation (a reactivated root gate, an inner gate)
                // is the controller's.
                // (One start can activate several gates: a start covers its tick.)
                if start && !w.recorded_cs.as_ref().is_some_and(|r| r.started.contains(&cutscene)) {
                    return;
                }
                pop_queue(w);
                gate_draw(w, &b, started);
            }
            CutsceneEvent::Action { cutscene, command } => {
                pop_queue(w);
                recorded_action(w, cutscene, command);
            }
            CutsceneEvent::Lock { cutscene, command, on, start, controller } => {
                // (`StopCopies` stops a copy inside a player's `Start`.)
                match placed(w, start, controller, "lock") {
                    Placed::Wait => return,
                    Placed::Elsewhere => continue,
                    Placed::Here => recorded_lock(w, cutscene, command, on),
                }
            }
            CutsceneEvent::Dialog { cutscene, command, start, controller } => {
                match placed(w, start, controller, "dialogue start") {
                    Placed::Wait => return,
                    Placed::Elsewhere => continue,
                    Placed::Here => recorded_dialog(w, cutscene, command),
                }
            }
            CutsceneEvent::Entry { cutscene, command, passed, on_fail, start, controller } => {
                match placed(w, start, controller, "entry check") {
                    Placed::Wait => return,
                    Placed::Elsewhere => continue,
                    Placed::Here => recorded_entry(w, cutscene, command, passed, on_fail),
                }
            }
        }
    }
}

enum Placed {
    /// Not yet: inside a start the sim hasn't made.
    Wait,
    /// Taken from the queue and reported.
    Elsewhere,
    /// Taken from the queue; the caller runs it.
    Here,
}

/// Where a recorded command event runs: inside a player's `Start` at the sim's start of that
/// cutscene, inside the controller's tick here; elsewhere (a `StopCutscene` action, an area's
/// unload) it isn't placed (reported).
fn placed(w: &mut World, start: Option<Guid>, controller: bool, what: &str) -> Placed {
    if let Some(s) = start {
        if !w.recorded_cs.as_ref().is_some_and(|r| r.started.contains(&s)) {
            return Placed::Wait;
        }
    } else if !controller {
        pop_queue(w);
        w.coverage.unsupported(format!(
            "cutscene: a recorded {what} outside the cutscene controller and a start"
        ));
        return Placed::Elsewhere;
    }
    pop_queue(w);
    Placed::Here
}

/// `CommandLockControls.OnRun` (`on`: `TickAll`, then the lock) or `OnStop` (`PostponeAll`, then
/// the release). (`OnRun`'s scheduled `InterruptAll` of the party's non-cutscene commands has
/// nothing to interrupt in a dialogue run: no unit commands run there.) The command must be a
/// lock of a cutscene the sim classified, and not a background one (`OnRun` would clear the
/// blueprint's `IsBackground`).
fn recorded_lock(w: &mut World, cutscene: Guid, command: Guid, on: bool) {
    if !w.scene.classified_cutscenes.contains(&cutscene) {
        w.coverage.unsupported("cutscene: a recorded lock of a cutscene the sim didn't classify");
        return;
    }
    let lock = w.content.get(command);
    if !lock.as_ref().is_some_and(|b| crate::content::is_a(b.obj.ty, "CommandLockControls")) {
        w.coverage.unsupported("cutscene: a recorded lock that isn't a CommandLockControls");
        return;
    }
    if on && w.content.get(cutscene).is_some_and(|b| b.view().bool("IsBackground")) {
        w.coverage
            .unsupported("CommandLockControls in a background cutscene (IsBackground cleared)");
    }
    if on {
        crate::combat::life::buffs_tick_all(w);
    } else {
        crate::combat::life::buffs_postpone_all(w);
    }
    crate::runner::set_cutscene_lock(w, on);
}

/// With recorded locks, Pause mode (a UI request: a tutorial window, the pause key) is taken from
/// the recording: before the oracle tick `tick`'s tasks run, the sim requests Pause when the
/// recording ticks in it and the sim isn't, and stops it when the recording has left it.
pub fn recorded_pause(w: &mut World, tick: i64) {
    if !w.scene.recorded_locks {
        return;
    }
    let Some(rec) = w.recorded_cs.as_ref() else { return };
    let paused = rec.modes.get(&tick) == Some(&4);
    if paused && w.player.mode != "Pause" {
        crate::runner::start_mode(w, "Pause");
    } else if !paused && w.player.mode == "Pause" {
        w.before_tick.push(crate::world::BeforeTick::StopMode("Pause"));
    }
}

/// A tick in Pause mode (no cutscene controller): the previous tick's events are settled, and a
/// recorded event of this tick is reported.
pub fn recorded_paused_tick(w: &mut World, tick: i64) {
    recorded_tick_end(w);
    if w.recorded_cs.as_ref().is_some_and(|r| r.ticks.get(&tick).is_some_and(|e| !e.is_empty())) {
        w.coverage.unsupported("cutscene: a recorded cutscene event in Pause mode");
    }
    if w.recorded_cs.as_ref().is_some_and(|r| r.tick_zones.contains_key(&tick)) {
        w.coverage.unsupported("dialogue replay: a recorded zone entry in Pause mode");
    }
}

/// `ScriptZoneController` in the oracle tick `tick`: the recorded entries no step makes (a
/// cutscene or a teleport moved the unit; the sim doesn't move units), after a step's (`main`:
/// a `zone:` step this tick, whose order with them isn't kept: reported). Each unit is the
/// recording's id resolved as the awake inputs' are.
pub fn recorded_unit_zones(w: &mut World, tick: i64, main: bool) {
    let Some(entries) = w.recorded_cs.as_mut().and_then(|r| r.tick_zones.remove(&tick)) else {
        return;
    };
    if main {
        w.coverage.unsupported("dialogue replay: zone entries of several units in one tick");
    }
    if !matches!(w.player.mode, "Default" | "Cutscene") {
        w.coverage.unsupported("ScriptZoneController: a recorded zone entry outside its modes");
        return;
    }
    for e in entries {
        let mut awake = w.recorded_awake.take();
        let unit = match (&mut awake, &e.unit) {
            (Some(a), Some(id)) => a.resolve(w, id),
            _ => None,
        };
        w.recorded_awake = awake;
        match (unit, e.zone) {
            (Some(u), Some(z)) => crate::scene::enter_by_blueprint_unit(w, z, u),
            _ => {
                w.coverage.unsupported("dialogue replay: a recorded zone entry the sim can't make")
            }
        }
    }
}

/// With recorded locks: the game mode the sim ticks in must be the recording's in each tick.
pub fn check_recorded_mode(w: &mut World, tick: i64) {
    if !w.scene.recorded_locks {
        return;
    }
    let Some(rec) = w.recorded_cs.as_ref() else { return };
    let recorded = match rec.modes.get(&tick) {
        Some(1) => Some("Default"),
        Some(3) => Some("Dialog"),
        Some(4) => Some("Pause"),
        Some(7) => Some("Cutscene"),
        _ => None,
    };
    if recorded.is_none() {
        w.coverage.unsupported("dialogue replay: a tick without a recorded mode the sim models");
    } else if recorded != Some(w.player.mode) {
        if std::env::var_os("WR_DEBUG_MODES").is_some() {
            eprintln!("DEBUG mode tick {tick}: sim {} recorded {recorded:?}", w.player.mode);
        }
        w.coverage.unsupported("dialogue replay: a game mode unlike the recording's");
    }
}

fn pop_queue(w: &mut World) {
    if let Some(r) = w.recorded_cs.as_mut() {
        r.queue.pop_front();
    }
}

fn gate_draw(w: &mut World, b: &Blueprint, started: i64) {
    let tracks = b.view().objs("m_Tracks");
    if i64::try_from(tracks.len()).ok() != Some(started) {
        w.coverage.unsupported("cutscene: a random gate's started tracks differ");
        return;
    }
    let n = tracks.iter().filter(|t| !View(t).guids("m_Commands").is_empty()).count();
    if let Ok(n) = i32::try_from(n)
        && n > 0
    {
        w.rng.range_int(crate::rng::sites::GATE_SELECT_TRACK, 0, n);
    }
}

/// The pending steps of the recorded run of `command` in `cutscene`, taken: the command's next
/// invocation, which must be the first step left (with the recording's cutscene commands, the
/// first left on its track: the recording orders parallel tracks). `Err(true)`: the command has
/// pending steps but not there (out of order); `Err(false)`: it has none.
fn take_run(
    w: &mut World,
    cutscene: Guid,
    command: Guid,
) -> std::result::Result<Vec<T0Step>, bool> {
    let Some(p) = w.scene.cutscene_pending.iter().position(|p| p.cutscene == cutscene) else {
        return Err(false);
    };
    let steps = &w.scene.cutscene_pending[p].steps;
    // (The recording orders parallel tracks; the tracks the gates put before this one must be
    // done.)
    let head = if w.scene.recorded_commands {
        steps.iter().position(|s| {
            s.command.guid == command
                && steps
                    .iter()
                    .find(|o| o.track == s.track)
                    .is_some_and(|o| o.invocation == s.invocation)
                && !steps.iter().any(|o| s.after.contains(&o.track))
        })
    } else {
        steps.first().filter(|f| f.command.guid == command).map(|_| 0)
    };
    let Some(i) = head else {
        return Err(steps.iter().any(|s| s.command.guid == command));
    };
    let invocation = steps[i].invocation;
    let lead = steps[i..].iter().take_while(|s| s.invocation == invocation).count();
    let run: Vec<T0Step> = w.scene.cutscene_pending[p].steps.drain(i..i + lead).collect();
    if w.scene.cutscene_pending[p].steps.is_empty() {
        w.scene.cutscene_pending.remove(p);
    }
    Ok(run)
}

fn run_steps(w: &mut World, cutscene: Guid, run: &[T0Step]) {
    w.scene.in_cutscene_step = true;
    for step in run {
        run_step(w, cutscene, step);
    }
    w.scene.in_cutscene_step = false;
}

/// A recorded `CommandStartDialog.OnRun`: its step, taken as an action's.
fn recorded_dialog(w: &mut World, cutscene: Guid, command: Guid) {
    match take_run(w, cutscene, command) {
        Ok(run) => run_steps(w, cutscene, &run),
        Err(true) => w.coverage.unsupported("cutscene: a recorded T0 command run out of order"),
        Err(false) if !w.scene.classified_cutscenes.contains(&cutscene) => w
            .coverage
            .unsupported("cutscene: a recorded command of a cutscene the sim didn't classify"),
        Err(false) => {
            w.coverage.unsupported("cutscene: a recorded dialogue start the sim has no step for");
        }
    }
}

/// A recorded entry check: a failed `SkipCommand` check skips the command's pending steps, if it
/// has any (a presentation command has none). A failed check of another kind (the track stops or
/// is dropped: the classification reported it) and a result unlike the blueprint's `OnFail` are
/// reported.
fn recorded_entry(w: &mut World, cutscene: Guid, command: Guid, passed: bool, on_fail: i64) {
    if !w.scene.classified_cutscenes.contains(&cutscene) {
        w.coverage
            .unsupported("cutscene: a recorded command of a cutscene the sim didn't classify");
        return;
    }
    let blueprint = w.content.get(command).map(|b| b.view().int("OnFail"));
    if blueprint != Some(on_fail) {
        w.coverage.unsupported("cutscene: a recorded entry check unlike its command");
        return;
    }
    if passed {
        return;
    }
    if on_fail != ENTRY_SKIP_COMMAND {
        w.coverage.unsupported("cutscene: a failed entry check that stops or drops its track");
        return;
    }
    if let Err(true) = take_run(w, cutscene, command) {
        w.coverage.unsupported("cutscene: a recorded T0 command run out of order");
    }
}

fn recorded_action(w: &mut World, cutscene: Guid, command: Guid) {
    match take_run(w, cutscene, command) {
        Ok(run) => run_steps(w, cutscene, &run),
        Err(true) => {
            w.coverage.unsupported("cutscene: a recorded T0 command run out of order");
        }
        Err(false) if !w.scene.classified_cutscenes.contains(&cutscene) => {
            // A cutscene the sim never classified: its other commands weren't checked.
            w.coverage
                .unsupported("cutscene: a recorded command of a cutscene the sim didn't classify");
        }
        Err(false) => {
            // Only a command whose every action is presentation (the cutscenes it starts or
            // stops classified in turn) can run without the sim's pending steps; any other, or
            // a missing one, is reported. The cutscenes it starts are started for their root
            // gates' recorded draws.
            let Some(b) = w.content.get(command) else {
                w.coverage.unsupported("cutscene: a recorded command missing");
                return;
            };
            let actions = b.view().obj("Action").map(|l| l.objs("Actions")).unwrap_or_default();
            let mut out = Classified::default();
            let mut walk = Walk::default();
            side_actions_in(w, actions.clone(), "a recorded command's", &mut out, &mut walk, true);
            if !out.unsupported.is_empty() {
                w.coverage
                    .unsupported("cutscene: a recorded T0 command the sim has no pending step for");
                return;
            }
            settle_recorded(w, &out);
            for a in actions {
                let av = View(a);
                if av.type_name() == "PlayCutscene"
                    && let Some(c) = av.guid("m_Cutscene")
                    && let Some(r) = w.recorded_cs.as_mut()
                {
                    r.started.push(c);
                }
            }
        }
    }
}

/// `PlayCutscene` with recorded progress: the start activates the cutscene's root gate at once
/// (`CutscenePlayerData.Start` → `ActivateGate`), so its recorded draw (the queue's next event)
/// is made here; then the queue goes on.
pub fn recorded_start(w: &mut World, cutscene: Guid) {
    let Some(rec) = w.recorded_cs.as_mut() else { return };
    // (Only a cutscene with a random gate the sim accepted draws inside its start: its root,
    // or a gate a skipped track signals; a lock stops inside it when it stops a copy.)
    rec.started.push(cutscene);
    drain_queue(w);
}

/// The end of a tick with recorded progress: an event the sim didn't reach is reported.
pub fn recorded_tick_end(w: &mut World) {
    if let Some(r) = w.recorded_cs.as_mut() {
        r.started.clear();
    }
    let left = w.recorded_cs.as_mut().map(|r| std::mem::take(&mut r.queue).len()).unwrap_or(0);
    if left > 0 {
        w.coverage.unsupported("cutscene: a recorded cutscene event the sim didn't reach");
    }
}

/// The end of the run: recorded events of ticks the sim didn't run are reported.
pub fn recorded_finish(w: &mut World, last_tick: i64) {
    recorded_tick_end(w);
    if w.recorded_cs.as_ref().and_then(|r| r.zones.as_ref()).is_some_and(|z| !z.is_empty()) {
        w.coverage.unsupported("dialogue replay: a recorded zone entry the sim didn't make");
    }
    if w.recorded_cs.as_ref().is_some_and(|r| r.ticks.range(last_tick + 1..).next().is_some()) {
        w.coverage.unsupported("cutscene: recorded cutscene events after the run's last tick");
    }
    if w.recorded_cs.as_ref().is_some_and(|r| !r.tick_zones.is_empty()) {
        w.coverage.unsupported("dialogue replay: a recorded zone entry the sim didn't make");
    }
}

fn run_step(w: &mut World, cutscene: Guid, step: &T0Step) {
    let index = match step.kind {
        StepKind::Action(i) => i,
        StepKind::StartDialog => {
            start_dialog_command(w, cutscene, step.command.view());
            return;
        }
    };
    let list = step.command.view().obj("Action");
    let Some(a) = list.map(|l| l.objs("Actions")).and_then(|v| v.get(index).copied()) else {
        return;
    };
    let v = View(a);
    let ty = v.type_name();
    w.coverage.executed(format!("action {ty}"));
    if let Err(e) = super::actions::run(w, &step.command, v) {
        w.log("error", format!("cutscene {cutscene}: action {ty}: {e:?}"));
        w.events.push("element_exception", json!({ "action": ty, "error": format!("{e:?}") }));
    }
}

/// `CommandStartDialog.OnRun` without a speaker or an evaluator (the classification reports
/// those): `StartDialogWithoutTarget(Dialog, SpeakerName)`. (Its `Finisher` only tells the track
/// when the dialogue ends: the track's later commands have their recorded runs.)
fn start_dialog_command(w: &mut World, cutscene: Guid, v: View<'_>) {
    let Some(d) = v.guid("m_Dialog") else { return };
    w.coverage.executed("cutscene command CommandStartDialog");
    // The speaker name becomes a string as the start data is built: an exception there ends
    // `OnRun`, which `StartCommand` catches (the track goes on, the dialogue not started).
    let name = crate::text::localized_field(w, v, "SpeakerName");
    match crate::dialog::start_dialog(w, d, None, None, name) {
        Ok(()) => {}
        Err(wr_text::TextError::Throws(e)) => {
            w.log("error", format!("cutscene {cutscene}: CommandStartDialog: speaker name: {e}"));
            w.coverage
                .unsupported("cutscene: CommandStartDialog: an exception in its speaker name");
        }
        Err(e) => {
            w.coverage.unsupported(format!("cutscene: CommandStartDialog (speaker name: {e})"))
        }
    }
}

/// A cutscene's T0 actions waiting for the cutscene controller's next tick.
#[derive(Debug, Clone)]
pub struct Pending {
    pub cutscene: Guid,
    pub steps: Vec<T0Step>,
}

/// `CutsceneController.Tick` for the T0 actions of the cutscenes started so far: each
/// pending cutscene runs its `CommandAction`s' T0 actions in order. (Command durations are T1:
/// the actions run at the controller's next tick, in the order the cutscene reaches them.)
pub fn run_pending(w: &mut World) {
    let pending = std::mem::take(&mut w.scene.cutscene_pending);
    w.scene.in_cutscene_step = true;
    for p in pending {
        for step in &p.steps {
            run_step(w, p.cutscene, step);
        }
    }
    w.scene.in_cutscene_step = false;
}

/// `PlayCutscene.RunAction()` as a T1 event. A cutscene with `CommandStartDialog` is reported
/// (its dialogue would block the track, and the sim doesn't start it) and its T0 actions aren't
/// queued, unless the recording has the cutscene commands (the start is then a step).
pub fn play(w: &mut World, _owner: &Rc<Blueprint>, a: View<'_>) -> Result<()> {
    let Some(c) = a.guid("m_Cutscene") else {
        return Err(super::ElementError::Other("NullReferenceException: cutscene".into()));
    };
    let cls = classify(w, c);
    for t in &cls.t1_commands {
        w.coverage.stub(format!("cutscene command {t} (T1)"));
    }
    for t in &cls.t1_actions {
        w.coverage.stub(format!("cutscene action {t} (T1: positions, cutscene control)"));
    }
    settle_recorded(w, &cls);
    for u in &cls.unsupported {
        w.coverage.unsupported(format!("cutscene: {u}"));
    }
    // With the commands' runs recorded, a dialogue start is a step; without, it's reported.
    let dialogs = !cls.dialogs.is_empty() && !w.scene.recorded_commands;
    if dialogs {
        w.coverage.unsupported(
            "cutscene: CommandStartDialog (the dialogue blocks its track; not started)",
        );
    }
    w.events.push(
        "t1",
        json!({ "what": "cutscene_play", "cutscene": c.to_string(), "put_in_queue": a.bool("PutInQueue"), "start_dialogs": cls.dialogs.iter().map(ToString::to_string).collect::<Vec<_>>() }),
    );
    // Queued before the start: the start's own recorded runs take their steps.
    if !cls.t0_steps.is_empty() && !dialogs {
        w.scene.cutscene_pending.push(Pending { cutscene: c, steps: cls.t0_steps });
    }
    recorded_start(w, c);
    Ok(())
}

/// `StopCutscene.RunAction()` as a T1 event.
pub fn stop(w: &mut World, _owner: &Rc<Blueprint>, a: View<'_>) -> Result<()> {
    if a.bool("m_AllCutscenesFromUnit") || a.obj("WithUnit").is_some() {
        w.coverage.stub("StopCutscene by unit (T1)");
    }
    let g = a.guid("m_Cutscene");
    // A stopped cutscene's commands stop where they are: its pending T0 actions would not
    // all run, and where it stands is timing.
    if g.is_some_and(|g| w.scene.cutscene_pending.iter().any(|p| p.cutscene == g))
        || (a.bool("m_AllCutscenesFromUnit") || a.obj("WithUnit").is_some())
            && !w.scene.cutscene_pending.is_empty()
    {
        w.coverage.unsupported("StopCutscene: a cutscene with T0 actions still to run");
    }
    let c = g.map(|g| g.to_string());
    w.coverage.stub("StopCutscene (T1)");
    w.events.push("t1", json!({ "what": "cutscene_stop", "cutscene": c }));
    Ok(())
}
