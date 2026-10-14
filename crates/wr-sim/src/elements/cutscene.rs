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
//! - A cutscene's `OnStopped` actions and a fade's in-fade actions are accepted when they are
//!   presentation (a cutscene they start is classified in turn); anything else is reported.
//! - Always reported: a command with an `EntryCondition` (`StartNextCommand` checks it and
//!   applies `OnFail`), `CommandStartDialog` (the dialogue blocks its track and isn't started by the sim), a
//!   cutscene started by a cutscene with T0 actions of its own, and a `CommandDismountSpan`
//!   while any unit has a rider part.
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

/// A `CommandAction` action with T0 effects: the command blueprint and the action's index.
#[derive(Debug, Clone)]
pub struct T0Step {
    pub command: Rc<Blueprint>,
    pub action: usize,
    /// The command's occurrence in the closure's walk (a track can run one command blueprint
    /// twice: each run has its own steps).
    pub invocation: usize,
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
    /// activates them: their draws only pick presentation ([`settle_random_gates`]).
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
    if !ordered {
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
    out.t0_steps = order.into_iter().flat_map(|t| walk.tracks[t].steps.clone()).collect();
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
    if has_entry_condition(v) {
        push_unique(
            &mut out.unsupported,
            "cutscene command with an EntryCondition (StartNextCommand's OnFail)".into(),
        );
    }
    walk.invocations += 1;
    let invocation = walk.invocations;
    if let Some(t1) = T1_COMMANDS.iter().find(|c| crate::content::is_a(b.obj.ty, c)) {
        out.t1_commands.push(t1);
        // `RunAway`: `UnitRunAway.DoRunAway` takes the unit out of the game or destroys it
        // (and `BecameStalker` changes the player's state): not presentation.
        if *t1 == "CommandMoveUnit" && v.bool("RunAway") {
            push_unique(&mut out.unsupported, "CommandMoveUnit with RunAway".into());
        }
        // (Not checked yet: `CommandLockControls` ticks every unit's buffs at its start and
        // postpones them at its stop, `UnitBuffsController.TickAll`/`PostponeAll`; with timed
        // buffs that changes deadlines by the cutscene's duration. PLAN, Horgus.)
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
            if let Some(d) = v.guid("m_Dialog") {
                out.dialogs.push(d);
            }
            node.dialogs += 1;
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
                    node.steps.push(T0Step { command: b.clone(), action: i, invocation });
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

/// A closure's random gates: accepted when the run takes cutscene progress from a recording
/// (the recording then makes their draws), else reported.
pub fn settle_random_gates(w: &mut World, cls: &Classified) {
    w.scene.classified_cutscenes.extend(cls.cutscenes.iter().copied());
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
}

/// A cutscene's progress as a recording observed it, per oracle tick, in order.
#[derive(Debug, Default)]
pub struct RecordedCutscene {
    pub ticks: std::collections::BTreeMap<i64, Vec<CutsceneEvent>>,
    /// The current tick's events not consumed yet: what the cutscene controller's place can't
    /// take waits for a later one in the tick (a cutscene's start activates its root gate at
    /// once: `CutscenePlayerData.Start`).
    pub queue: std::collections::VecDeque<CutsceneEvent>,
    /// Cutscenes started (`PlayCutscene`) whose root gate's draw hasn't been taken yet.
    pub started: Vec<Guid>,
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

    /// # Errors
    ///
    /// A malformed record, or one before any tick.
    pub fn of(records: &[serde_json::Value]) -> std::result::Result<Self, String> {
        let guid = |r: &serde_json::Value, f: &str| {
            r[f].as_str().and_then(Guid::parse).ok_or_else(|| format!("{}: {f} {}", r["ev"], r[f]))
        };
        let mut out = Self::default();
        let mut tick = None;
        for r in records {
            if r["k"] == "tick" {
                tick = Some(r["t"].as_i64().ok_or("a tick record without t")?);
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
                _ => continue,
            };
            let t = tick.ok_or("a cutscene record before any tick")?;
            out.ticks.entry(t).or_default().push(ev);
        }
        Ok(out)
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
        }
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

fn recorded_action(w: &mut World, cutscene: Guid, command: Guid) {
    let pending = w.scene.cutscene_pending.iter().position(|p| p.cutscene == cutscene);
    let first = pending.and_then(|p| w.scene.cutscene_pending[p].steps.first().cloned());
    match (pending, first) {
        (Some(p), Some(f)) if f.command.guid == command => {
            let lead = w.scene.cutscene_pending[p]
                .steps
                .iter()
                .take_while(|s| s.invocation == f.invocation)
                .count();
            let run: Vec<T0Step> = w.scene.cutscene_pending[p].steps.drain(..lead).collect();
            if w.scene.cutscene_pending[p].steps.is_empty() {
                w.scene.cutscene_pending.remove(p);
            }
            w.scene.in_cutscene_step = true;
            for step in run {
                run_step(w, cutscene, &step);
            }
            w.scene.in_cutscene_step = false;
        }
        (Some(p), _)
            if w.scene.cutscene_pending[p].steps.iter().any(|s| s.command.guid == command) =>
        {
            w.coverage.unsupported("cutscene: a recorded T0 command run out of order");
        }
        _ if !w.scene.classified_cutscenes.contains(&cutscene) => {
            // A cutscene the sim never classified: its other commands weren't checked.
            w.coverage
                .unsupported("cutscene: a recorded command of a cutscene the sim didn't classify");
        }
        _ => {
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
            settle_random_gates(w, &out);
            for a in actions {
                let av = View(a);
                if av.type_name() == "PlayCutscene"
                    && let Some(c) = av.guid("m_Cutscene")
                    && w.scene.random_gates_t1.iter().any(|(x, _)| *x == c)
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
    // Only a cutscene with a random gate the sim accepted can draw inside its start (its root,
    // or a gate a skipped track signals).
    if !w.scene.random_gates_t1.iter().any(|(c, _)| *c == cutscene) {
        return;
    }
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
    if w.recorded_cs.as_ref().is_some_and(|r| r.ticks.range(last_tick + 1..).next().is_some()) {
        w.coverage.unsupported("cutscene: recorded cutscene events after the run's last tick");
    }
}

fn run_step(w: &mut World, cutscene: Guid, step: &T0Step) {
    let list = step.command.view().obj("Action");
    let Some(a) = list.map(|l| l.objs("Actions")).and_then(|v| v.get(step.action).copied()) else {
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
/// (its dialogue would block the track, and the sim doesn't start it): its T0 actions aren't
/// queued.
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
    settle_random_gates(w, &cls);
    recorded_start(w, c);
    for u in &cls.unsupported {
        w.coverage.unsupported(format!("cutscene: {u}"));
    }
    if !cls.dialogs.is_empty() {
        w.coverage.unsupported(
            "cutscene: CommandStartDialog (the dialogue blocks its track; not started)",
        );
    }
    w.events.push(
        "t1",
        json!({ "what": "cutscene_play", "cutscene": c.to_string(), "put_in_queue": a.bool("PutInQueue"), "start_dialogs": cls.dialogs.iter().map(ToString::to_string).collect::<Vec<_>>() }),
    );
    if !cls.t0_steps.is_empty() && cls.dialogs.is_empty() {
        w.scene.cutscene_pending.push(Pending { cutscene: c, steps: cls.t0_steps });
    }
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
