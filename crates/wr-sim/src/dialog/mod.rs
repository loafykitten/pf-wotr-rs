//! The dialogue controller (decompiled, not copied: `Kingmaker.Controllers.Dialog/
//! DialogController.cs`, `CueSequence.cs`, `SkillCheckResult.cs`, `StartDialogData.cs`;
//! `Kingmaker.DialogSystem/{CueSelection,CharacterSelection,DialogSpeaker}.cs`;
//! `Kingmaker.DialogSystem.Blueprints/{BlueprintDialog,BlueprintCueBase,BlueprintCue,
//! BlueprintAnswer,BlueprintAnswersList,BlueprintCheck,BlueprintCueSequence}.cs`;
//! `Kingmaker.Blueprints.Root/DialogExtensions.cs`).
//!
//! Timing, as in the C#: `SelectAnswer` runs the answer's effects and only *schedules*
//! the next cue; the controller's `Tick` (once per game tick in Dialog mode) plays it, so
//! the completed transition is observed after a tick (`dlg_transition`). Playing a cue
//! sets `CurrentCue`, whose setter runs the previous cue's `OnStop` then the new cue's
//! `OnShow`. A check plays immediately into its success or failure cue, in the same tick.
//!
//! Presentation coupling kept on purpose (`docs/poc1/oracle.md`): showing a cue's answers
//! makes the UI compute each skill-check answer's `SkillChecksDC`, which triggers real
//! `RuleRollD20`s on the global RNG. [`DialogController::ui_preview_calls`] says how many
//! times the UI evaluates it per answer per cue shown (2, measured in the oracle's
//! `dialog-checks` fixture); the sim makes those draws so a replayed tape stays aligned.
//! Our own UI is free to compute previews without dice.
//!
//! Not ported (reported if reached): book pages, `StartPosition` evaluators, dual
//! companions, cutscene-controlled first speakers, `Random` character selection is ported
//! (`CharacterSelection` site), `Manual`/`Companion` selection report unsupported.

pub mod recruit;

use std::rc::Rc;

use serde_json::{Value, json};
use wr_content::Guid;

use crate::content::{Blueprint, View};
use crate::csharp::dict::CsDict;
use crate::elements::{self, Ctx};
use crate::events::unit_ref;
use crate::narrative::ScheduledDialog;
use crate::rules::checks::{RulePartyStatCheck, stat_check_create};
use crate::stats::types::StatType;
use crate::world::{EntityIdx, World};

/// `SkillCheckResult`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct SkillCheckResult {
    pub unit: Option<String>,
    pub stat: i32,
    pub dc: i32,
    pub passed: bool,
    pub roll_result: i32,
    pub d20: i32,
    pub stat_value: i32,
}

/// `CueSequence`.
#[derive(Debug, Clone)]
pub struct CueSequenceState {
    pub blueprint: Guid,
    pub next: usize,
}

/// `DialogController` state.
#[derive(Debug, Clone)]
pub struct DialogController {
    pub dialog: Option<Guid>,
    pub current_cue: Option<Guid>,
    pub initiator: Option<EntityIdx>,
    pub first_speaker: Option<EntityIdx>,
    pub current_speaker: Option<EntityIdx>,
    pub acting_unit: Option<EntityIdx>,
    pub capital_party_checks_enabled: bool,
    pub dialog_position: [f32; 3],
    pub involved_units: Vec<EntityIdx>,
    pub answers: Vec<Guid>,
    pub continue_cue: Option<Guid>,
    pub local_shown_cues: CsDict<Guid, ()>,
    pub local_selected_answers: CsDict<Guid, ()>,
    pub local_shown_answer_lists: CsDict<Guid, ()>,
    pub local_passed_checks: CsDict<Guid, ()>,
    pub local_failed_checks: CsDict<Guid, ()>,
    pub cue_to_play: Option<Guid>,
    pub cue_play_scheduled: bool,
    pub dialog_stop_scheduled: bool,
    pub cues_played_this_frame: i32,
    pub sequences: Vec<CueSequenceState>,
    pub skill_checks: Vec<SkillCheckResult>,
    pub alignment_shifts: Vec<(i32, i32)>,
    pub old_player_alignment: i32,
    /// Headless mode (the game's own flag): no camera, turns or animations; the dialogue
    /// starts at once instead of on the next frame.
    pub headless: bool,
    /// UI evaluations of `SkillChecksDC` per skill-check answer per cue shown.
    pub ui_preview_calls: u32,
    /// `m_CustomSpeakerName`: the scheduled dialogue's `CustomSpeakerName` (`None`: null).
    pub custom_speaker_name: Option<String>,
    /// `m_CustomSpeakerColor`, which `Clear` keeps: `None` while it's still the value from
    /// before the run (unknown), else the colour (`None` inside: null).
    pub custom_speaker_color: Option<Option<String>>,
    /// Write the dialogue's text records (`dlg_history`, `dlg_text`; the recording declares
    /// `dialog_text`).
    pub text_records: bool,
}

impl Default for DialogController {
    fn default() -> Self {
        Self {
            dialog: None,
            current_cue: None,
            initiator: None,
            first_speaker: None,
            current_speaker: None,
            acting_unit: None,
            capital_party_checks_enabled: false,
            dialog_position: [0.0; 3],
            involved_units: Vec::new(),
            answers: Vec::new(),
            continue_cue: None,
            local_shown_cues: CsDict::new(),
            local_selected_answers: CsDict::new(),
            local_shown_answer_lists: CsDict::new(),
            local_passed_checks: CsDict::new(),
            local_failed_checks: CsDict::new(),
            cue_to_play: None,
            cue_play_scheduled: false,
            dialog_stop_scheduled: false,
            cues_played_this_frame: 0,
            sequences: Vec::new(),
            skill_checks: Vec::new(),
            alignment_shifts: Vec::new(),
            old_player_alignment: 4,
            headless: false,
            ui_preview_calls: 2,
            custom_speaker_name: None,
            custom_speaker_color: None,
            text_records: false,
        }
    }
}

fn bp(w: &World, g: Guid) -> Option<Rc<Blueprint>> {
    w.content.get(g)
}

fn uid(w: &World, u: Option<EntityIdx>) -> Value {
    u.map_or(Value::Null, |u| unit_ref(&w.entity(u).unique_id))
}

fn g(x: Guid) -> String {
    x.to_string()
}

/// `BlueprintRoot.Dialog`'s system answers: continue, exit, interchapter continue and exit.
fn system_answers(w: &World) -> [Option<Guid>; 4] {
    let d = bp(w, crate::unit::BLUEPRINT_ROOT).and_then(|r| {
        r.view().obj("Dialog").map(|d| {
            [
                d.guid("m_ContinueAnswer"),
                d.guid("m_ExitAnswer"),
                d.guid("m_InterchapterContinueAnswer"),
                d.guid("m_InterchapterExitAnswer"),
            ]
        })
    });
    d.unwrap_or([None; 4])
}

/// `Dialog.GetContinueAnswer()`.
fn continue_answer(w: &World) -> Option<Guid> {
    let s = system_answers(w);
    if matches!(dialog_type(w), 0 | 1) { s[0] } else { s[2] }
}

/// `Dialog.GetExitAnswer()`.
fn exit_answer(w: &World) -> Option<Guid> {
    let s = system_answers(w);
    if matches!(dialog_type(w), 0 | 1) { s[1] } else { s[3] }
}

fn is_continue(w: &World, a: Guid) -> bool {
    let s = system_answers(w);
    Some(a) == s[0] || Some(a) == s[2]
}

fn is_exit(w: &World, a: Guid) -> bool {
    let s = system_answers(w);
    Some(a) == s[1] || Some(a) == s[3]
}

fn is_system(w: &World, a: Guid) -> bool {
    system_answers(w).contains(&Some(a))
}

// ---- starting -----------------------------------------------------------------------

/// `StartDialog(dialog, initiator, unit, mapObject: null, speakerName)`: schedules the
/// dialogue and starts it (`StartDialogWithoutTarget` / `StartDialogWithUnit`).
pub fn start_dialog(
    w: &mut World,
    dialog: Guid,
    initiator: Option<EntityIdx>,
    unit: Option<EntityIdx>,
    custom_speaker_name: Result<String, wr_text::TextError>,
) -> Result<(), wr_text::TextError> {
    w.events.push(
        "dlg_request",
        json!({ "dialog": g(dialog), "initiator": uid(w, initiator), "unit": uid(w, unit) }),
    );
    if w.player.dialog.scheduled.is_some() {
        w.log("error", "Another dialog already scheduled!");
    }
    // The speaker name becomes a string as the start data is built: an exception there (or
    // a name the sim can't make) ends the call before anything is scheduled.
    let custom_speaker_name = custom_speaker_name?;
    w.player.dialog.scheduled =
        Some(ScheduledDialog { dialog, initiator, unit, custom_speaker_name, is_scheduled: false });
    // `PreventDialogStart`: Rest mode active or about to be (not in these runs).
    start_scheduled_dialog(w);
    Ok(())
}

/// `StartScheduledDialog()`: marks it scheduled and starts it now (headless) or through
/// `Game.ScheduleAction` (the next tick's before-tick queue).
pub fn start_scheduled_dialog(w: &mut World) {
    let Some(s) = w.player.dialog.scheduled.as_mut() else {
        w.log("error", "Has no scheduled dialog");
        return;
    };
    if s.is_scheduled {
        w.log("error", "Dialog already scheduled");
        return;
    }
    s.is_scheduled = true;
    if w.dialog.headless {
        start_scheduled_dialog_immediately(w);
    } else {
        w.before_tick.push(crate::world::BeforeTick::Action(
            crate::world::ScheduledAction::StartScheduledDialog,
        ));
    }
}

/// A scheduled action's body (run by the before-tick queue).
pub fn run_scheduled_action(w: &mut World, a: crate::world::ScheduledAction) {
    match a {
        crate::world::ScheduledAction::StartScheduledDialog => {
            start_scheduled_dialog_immediately(w)
        }
        crate::world::ScheduledAction::StopDialogInteraction => {
            crate::rulebook::bus::raise_global_call(
                w,
                "IDialogInteractionHandler",
                "StopDialogInteraction",
                None,
            )
        }
    }
}

/// `StartScheduledDialogImmediately()`.
fn start_scheduled_dialog_immediately(w: &mut World) {
    let Some(s) = w.player.dialog.scheduled.take() else {
        w.log("error", "Has no scheduled dialog");
        return;
    };
    if w.player.mode == "Dialog" {
        w.log("error", "Trying to start dialog twice");
        return;
    }
    clear(w);
    w.dialog.dialog = Some(s.dialog);
    w.dialog.initiator = s.initiator;
    w.dialog.first_speaker = s.unit;
    w.dialog.current_speaker = s.unit;
    w.dialog.custom_speaker_name = Some(s.custom_speaker_name.clone());
    // `m_CustomSpeakerColor = CurrentSpeakerColor` (no current cue now): the speaker's
    // blueprint's colour, else the field keeps its value.
    if let Some(b) = current_speaker_blueprint(w) {
        w.dialog.custom_speaker_color = crate::text::blueprint_color_html(w, b).ok().map(Some);
    }
    let Some(db) = bp(w, s.dialog) else {
        w.coverage.unsupported("StartDialog: dialog blueprint missing");
        return;
    };
    fill_start_position(w, &db);
    let mut ok = true;
    if !elements::check_conditions(w, &db, db.view().obj("Conditions")) {
        ok = false;
    }
    let first = select_cue(w, &db, db.view().obj("FirstCue"));
    if first.is_none() {
        ok = false;
    }
    if first.is_some_and(|c| bp(w, c).is_some_and(|b| b.type_name() == "BlueprintBookPage")) {
        w.coverage.unsupported("DialogController: book pages");
    }
    let scoped = |w: &mut World, f: &dyn Fn(&mut World)| match s.unit {
        Some(u) => elements::with_ctx(w, Ctx::ClickedUnit(u), |w| f(w)),
        None => f(w),
    };
    if !ok {
        let db2 = db.clone();
        scoped(w, &move |w| elements::run_actions(w, &db2, db2.view().obj("ReplaceActions")));
        clear(w);
        w.events.push("dlg_failed", json!({ "dialog": g(s.dialog) }));
        crate::rulebook::bus::raise_global(w, "IDialogFinishHandler", None);
        return;
    }
    let db2 = db.clone();
    scoped(w, &move |w| elements::run_actions(w, &db2, db2.view().obj("StartActions")));
    add_involved_unit(w, s.unit);
    add_involved_unit(w, s.initiator);
    if s.unit.is_some() && s.initiator.is_some() && db.view().bool("TurnFirstSpeaker") {
        w.coverage.stub("DialogController: TurnUnit (facing)");
    }
    w.dialog.old_player_alignment = player_alignment(w);
    if !w.dialog.headless {
        // `Game.Instance.StartMode(Dialog)`: a before-tick action; the mode (and the
        // controller's `Start`) begins at the next tick.
        crate::runner::start_mode(w, "Dialog");
        w.coverage.stub("DialogController: UI and camera on dialogue start");
    }
    schedule_cue(w, first);
    w.player.dialog.shown_dialogs.insert(s.dialog, ());
    w.events.push(
        "dlg_started",
        json!({ "dialog": g(s.dialog), "first_speaker": uid(w, s.unit), "scheduled_cue": first.map(g) }),
    );
}

/// `FillStartPosition()`.
fn fill_start_position(w: &mut World, db: &Rc<Blueprint>) {
    if db.view().obj("StartPosition").is_some() {
        w.coverage.unsupported("DialogController.FillStartPosition: StartPosition evaluator");
    }
    let pos = match w.dialog.first_speaker {
        Some(u) => w.unit(u).map(|x| x.position),
        None => w.main_character.and_then(|mc| w.unit(mc).map(|x| x.position)),
    };
    w.dialog.dialog_position = pos.unwrap_or([0.0; 3]);
}

/// `AddInvolvedUnit(unit)`: interrupts its commands and makes it leave stealth (T1),
/// then records it.
fn add_involved_unit(w: &mut World, u: Option<EntityIdx>) {
    let Some(u) = u else { return };
    w.coverage.stub("DialogController.AddInvolvedUnit: interrupt commands, exit stealth");
    if !w.dialog.involved_units.contains(&u) {
        w.dialog.involved_units.push(u);
    }
}

/// `Clear()`.
fn clear(w: &mut World) {
    let headless = w.dialog.headless;
    let preview = w.dialog.ui_preview_calls;
    // `CurrentCue = null` runs the current cue's `OnStop` through the setter.
    set_current_cue(w, None);
    let stop_scheduled = w.dialog.dialog_stop_scheduled;
    w.dialog = DialogController {
        headless,
        ui_preview_calls: preview,
        dialog_stop_scheduled: stop_scheduled,
        custom_speaker_name: w.dialog.custom_speaker_name.take(),
        custom_speaker_color: w.dialog.custom_speaker_color.take(),
        text_records: w.dialog.text_records,
        initiator: w.dialog.initiator,
        first_speaker: w.dialog.first_speaker,
        current_speaker: w.dialog.current_speaker,
        ..DialogController::default()
    };
}

/// `IControllerStart.Start()` when Dialog mode starts.
pub fn controller_start(w: &mut World) {
    if w.dialog.dialog.is_none() {
        return;
    }
    let turn_player =
        w.dialog.dialog.and_then(|d| bp(w, d)).is_some_and(|b| b.view().bool("TurnPlayer"));
    if turn_player {
        w.coverage.stub("DialogController.Start: TurnUnit (facing)");
    }
    let fs = w.dialog.first_speaker;
    add_involved_unit(w, fs);
    w.coverage.stub("DialogController.Start: interrupt all units' commands");
    crate::rulebook::bus::raise_global(w, "IDialogStartHandler", None);
}

/// `DialogController.Tick()` (a Dialog-mode controller).
pub fn tick(w: &mut World) {
    if w.dialog.dialog.is_none() {
        stop_dialog(w);
    } else if w.dialog.cue_play_scheduled {
        w.dialog.cues_played_this_frame = 0;
        w.dialog.cue_play_scheduled = false;
        let c = w.dialog.cue_to_play;
        play_cue(w, c);
        transition_record(w);
    }
    w.dialog.dialog_stop_scheduled = false;
}

fn schedule_cue(w: &mut World, cue: Option<Guid>) {
    match cue {
        None => stop_dialog(w),
        Some(c) => {
            w.dialog.cue_to_play = Some(c);
            w.dialog.cue_play_scheduled = true;
        }
    }
}

/// `StopDialog()`. WrathOracle's `dlg_stop` record is a prefix: written on entry while a
/// dialogue is set, before the guard and before the current cue's `OnStop` actions run.
pub fn stop_dialog(w: &mut World) {
    let dialog = w.dialog.dialog;
    if let Some(d) = dialog {
        w.events.push("dlg_stop", json!({ "dialog": g(d) }));
    }
    if w.dialog.dialog_stop_scheduled {
        return;
    }
    w.dialog.dialog_stop_scheduled = true;
    if !w.dialog.headless {
        // `Game.Instance.StopMode(Dialog)`: a before-tick task (the controller's `Stop`
        // runs when the mode stops, at the next tick).
        w.before_tick.push(crate::world::BeforeTick::StopMode("Dialog"));
        w.before_tick.push(crate::world::BeforeTick::Action(
            crate::world::ScheduledAction::StopDialogInteraction,
        ));
    }
    w.coverage.stub("DialogController.StopDialog: speaker orientation and animations");
    clear(w);
    if let Some(d) = dialog.and_then(|d| bp(w, d)) {
        elements::run_actions(w, &d, d.view().obj("FinishActions"));
    }
}

/// `IControllerStop.Stop()` when Dialog mode stops.
pub fn controller_stop(w: &mut World) {
    crate::rulebook::bus::raise_global(w, "IDialogFinishHandler", None);
    clear(w);
    w.coverage.stub("DialogController.Stop: sound state, cutscene control of involved units");
}

// ---- answers ------------------------------------------------------------------------

/// `SelectAnswer(answer, manualUnitSelection)`.
pub fn select_answer(w: &mut World, answer: Guid, manual: Option<EntityIdx>) {
    let Some(cur) = w.dialog.current_cue else { return };
    if !is_system(w, answer) && !w.dialog.answers.contains(&answer) {
        w.log("error", "Trying to select invalid dialog answer");
        return;
    }
    let Some(ab) = bp(w, answer) else {
        w.coverage.unsupported("SelectAnswer: answer blueprint missing");
        return;
    };
    let av = ab.view();
    let sel_type = av.obj("CharacterSelection").map_or(0, |c| c.int("SelectionType"));
    if manual.is_none() && sel_type == 4 {
        w.log("error", "A unit must be specified for selected answer");
        return;
    }
    let index = w.dialog.answers.iter().position(|a| *a == answer);
    w.events.push(
        "dlg_answer",
        json!({ "answer": g(answer), "index": index, "cue": g(cur), "manual_unit": uid(w, manual) }),
    );
    if w.dialog.text_records {
        history_entries(w, cur, answer, &ab);
    }
    let first_time = !w.dialog.local_selected_answers.contains_key(&answer);
    if !w.player.dialog.selected_answers.contains_key(&answer) {
        w.player.dialog.selected_answers.insert(answer, ());
    }
    if first_time {
        w.dialog.local_selected_answers.insert(answer, ());
    }
    w.dialog.acting_unit = select_unit(w, &ab, manual, true);
    if sel_type != 1 {
        w.dialog.capital_party_checks_enabled = capital_party_checks_enabled(w, &ab);
    }
    elements::run_actions(w, &ab, av.obj("OnSelect"));
    apply_alignment_shift(w, &ab, true);
    let exp = av.int("Experience");
    if exp != 0 && first_time {
        let cr_override = dialog_cr_override(w);
        #[allow(clippy::cast_possible_truncation)]
        let xp = crate::xp::get_dialog_xp(w, exp as i32, None, cr_override);
        crate::xp::gain_experience(w, xp);
    }
    let next = select_next_cue(w, &ab, answer);
    if next.is_none() {
        stop_dialog(w);
    }
    schedule_cue(w, next);
    w.events.push(
        "dlg_answer_end",
        json!({ "scheduled": w.dialog.cue_play_scheduled, "scheduled_cue": w.dialog.cue_to_play.map(g), "acting_unit": uid(w, w.dialog.acting_unit), "dialog_active": w.dialog.dialog.is_some() }),
    );
    crate::rulebook::bus::raise_global(w, "ISelectAnswerHandler", None);
}

fn dialog_cr_override(w: &World) -> bool {
    w.dialog
        .dialog
        .and_then(|d| bp(w, d))
        .is_some_and(|b| b.view().obj("m_OverrideAreaCR").is_some())
}

/// `BlueprintAnswer.CapitalPartyChecksEnabled`.
fn capital_party_checks_enabled(w: &World, ab: &Rc<Blueprint>) -> bool {
    match ab.view().obj("CharacterSelection").map_or(0, |c| c.int("SelectionType")) {
        6 => true,
        0 => w.player.capital_party_mode,
        _ => false,
    }
}

/// `CharacterSelection.SelectUnit(answer, manualSelection, forceManual)`.
fn select_unit(
    w: &mut World,
    ab: &Rc<Blueprint>,
    manual: Option<EntityIdx>,
    force_manual: bool,
) -> Option<EntityIdx> {
    let cs = ab.view().obj("CharacterSelection");
    match cs.map_or(0, |c| c.int("SelectionType")) {
        0 | 6 => None,
        1 => w.dialog.acting_unit,
        2 => {
            let party = crate::narrative::party::party(w);
            let n = i32::try_from(party.len()).unwrap_or(i32::MAX);
            let i = w.rng.range_int(crate::rng::sites::CHARACTER_SELECTION, 0, n);
            usize::try_from(i).ok().and_then(|i| party.get(i).copied())
        }
        3 => w.main_character,
        4 => {
            if force_manual {
                return manual;
            }
            w.coverage.unsupported("CharacterSelection: Manual with comparison stats");
            manual
        }
        5 => {
            w.coverage.unsupported("CharacterSelection: Companion (ActingCompanion)");
            None
        }
        _ => {
            w.coverage.unsupported("CharacterSelection: unknown type");
            None
        }
    }
}

/// `SelectNextCue(answer)`.
fn select_next_cue(w: &mut World, ab: &Rc<Blueprint>, answer: Guid) -> Option<Guid> {
    if is_continue(w, answer) {
        if w.dialog.continue_cue.is_none() {
            w.log("error", "Continue answer was selected but continue cue is not specified");
        }
        return w.dialog.continue_cue;
    }
    if is_exit(w, answer) {
        return None;
    }
    let mut next = select_cue(w, ab, ab.view().obj("NextCue"));
    if next.is_none() && !w.dialog.sequences.is_empty() {
        let mut seq = w.dialog.sequences.last().cloned().expect("sequence");
        next = poll_next_cue(w, &mut seq);
        if let Some(top) = w.dialog.sequences.last_mut() {
            *top = seq.clone();
        }
        if next.is_none() {
            w.dialog.sequences.pop();
            next = sequence_exit(w, seq.blueprint).and_then(|e| {
                let eb = bp(w, e)?;
                select_cue(w, &eb, eb.view().obj("Continue"))
            });
        }
    }
    next
}

fn sequence_exit(w: &World, seq: Guid) -> Option<Guid> {
    bp(w, seq).and_then(|b| b.view().guid("m_Exit"))
}

/// `CueSequence.PollNextCue()`.
fn poll_next_cue(w: &mut World, seq: &mut CueSequenceState) -> Option<Guid> {
    let cues = bp(w, seq.blueprint).map(|b| b.view().guids_with_nulls("Cues")).unwrap_or_default();
    while seq.next < cues.len() {
        if let Some(c) = cues[seq.next]
            && can_show_cue(w, c)
        {
            break;
        }
        seq.next += 1;
    }
    while seq.next < cues.len() {
        let i = seq.next;
        seq.next += 1;
        if let Some(c) = cues[i] {
            return Some(c);
        }
    }
    None
}

/// `CueSelection.Select()`.
pub fn select_cue(w: &mut World, _owner: &Rc<Blueprint>, sel: Option<View<'_>>) -> Option<Guid> {
    let sel = sel?;
    let random = sel.int("Strategy") == 1;
    let mut list = Vec::new();
    for c in sel.guids("Cues") {
        if bp(w, c).is_some() && can_show_cue(w, c) {
            if !random {
                return Some(c);
            }
            list.push(c);
        }
    }
    if list.is_empty() {
        return None;
    }
    if random {
        let n = i32::try_from(list.len()).unwrap_or(i32::MAX);
        let i = w.rng.range_int(crate::rng::sites::CUE_SELECTION, 0, n);
        return usize::try_from(i).ok().and_then(|i| list.get(i).copied());
    }
    None
}

/// `BlueprintCueBase.CanShow()` and the subclass overrides.
pub fn can_show_cue(w: &mut World, c: Guid) -> bool {
    let Some(b) = bp(w, c) else { return false };
    let v = b.view();
    if v.bool("ShowOnce") {
        if v.bool("ShowOnceCurrentDialog") {
            if w.dialog.local_shown_cues.contains_key(&c) {
                return false;
            }
        } else if w.player.dialog.shown_cues.contains_key(&c) {
            return false;
        }
    }
    if !elements::check_conditions(w, &b, v.obj("Conditions")) {
        return false;
    }
    match b.type_name() {
        // `BlueprintCue.CanShow`: a speaker blueprint needs a unit.
        "BlueprintCue" => {
            let speaker = v.obj("Speaker");
            let needs = speaker.and_then(|s| s.guid("m_Blueprint"));
            if let Some(sb) = needs
                && speaker_entity(w, sb, speaker.is_some_and(|s| s.bool("SwitchDual"))).is_none()
            {
                return false;
            }
            true
        }
        // `BlueprintCueSequence.CanShow`: some cue of it can show.
        "BlueprintCueSequence" => {
            let cues = v.guids("Cues");
            cues.into_iter().any(|x| can_show_cue(w, x))
        }
        _ => true,
    }
}

/// `DialogSpeaker.GetEntity()` (`Kingmaker.DialogSystem/DialogSpeaker.cs`): the nearest
/// in-game, not suppressed, not dead unit of the loaded state, the party or the creation queue
/// whose `Blueprint` (the one `PretendUnit` fakes for a placeholder's unit) is the speaker's,
/// to the dialogue position. Units built from placeholders are units like the rest.
fn speaker_entity(w: &mut World, blueprint: Guid, switch_dual: bool) -> Option<EntityIdx> {
    if switch_dual {
        w.coverage.unsupported("DialogSpeaker: SwitchDual");
    }
    let pos = w.dialog.dialog_position;
    let mut best: Option<(EntityIdx, f32)> = None;
    let units: Vec<EntityIdx> = (0..w.entities.len())
        .map(|i| EntityIdx(u32::try_from(i).unwrap_or(0)))
        .filter(|e| w.unit(*e).is_some() && w.entity(*e).is_in_game)
        .collect();
    for u in units {
        let x = w.unit(u).expect("unit");
        if x.blueprint != Some(blueprint) || x.life_state == 2 {
            continue;
        }
        let (life, position) = (x.life_state, x.position);
        if life == 1 {
            w.coverage.unsupported("DialogSpeaker: unconscious speaker made conscious");
        }
        let d = dist2(position, pos);
        if best.is_none_or(|(_, bd)| d < bd) {
            best = Some((u, d));
        }
    }
    // A placeholder that wasn't turned into a unit (`Options::skip_placeholders`) would be a
    // candidate the game has.
    let unbuilt = w.player.placeholders.iter().any(|(id, p)| {
        p.blueprint == Some(blueprint) && w.by_id.get(id).is_some_and(|e| w.unit(*e).is_none())
    });
    if unbuilt {
        w.coverage.unsupported("DialogSpeaker: a placeholder that wasn't built into a unit");
    }
    best.map(|b| b.0)
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// `DialogSpeaker.GetSpeaker(defaultSpeaker)`.
fn get_speaker(
    w: &mut World,
    speaker: Option<View<'_>>,
    default: Option<EntityIdx>,
) -> Option<EntityIdx> {
    let s = speaker?;
    if s.bool("NoSpeaker") {
        return None;
    }
    match s.guid("m_Blueprint") {
        Some(b) => speaker_entity(w, b, s.bool("SwitchDual")),
        None => default,
    }
}

// ---- playing cues -------------------------------------------------------------------

/// `PlayCue(cue)`.
fn play_cue(w: &mut World, cue: Option<Guid>) {
    w.dialog.cues_played_this_frame += 1;
    if w.dialog.cues_played_this_frame > 1001 {
        w.coverage.unsupported("DialogController.PlayCue: stack overflow guard");
        return;
    }
    let Some(c) = cue else {
        stop_dialog(w);
        return;
    };
    w.dialog.local_shown_cues.insert(c, ());
    w.player.dialog.shown_cues.insert(c, ());
    w.dialog.answers.clear();
    w.dialog.continue_cue = None;
    let Some(b) = bp(w, c) else { return };
    let class = b.type_name();
    w.events.push("dlg_play", json!({ "cue": g(c), "class": class }));
    match class {
        "BlueprintCue" => play_basic_cue(w, c, &b),
        "BlueprintCheck" => play_check(w, c, &b),
        "BlueprintCueSequence" => play_sequence(w, c),
        "BlueprintBookPage" => w.coverage.unsupported("DialogController.PlayBookPage"),
        other => w.coverage.unsupported(format!("DialogController.PlayCue: {other}")),
    }
}

/// The `CurrentCue` setter.
fn set_current_cue(w: &mut World, value: Option<Guid>) {
    let old = w.dialog.current_cue;
    if let Some(o) = old.and_then(|o| bp(w, o)) {
        elements::run_actions(w, &o, o.view().obj("OnStop"));
    }
    w.dialog.current_cue = value;
    if let Some(cb) = value.and_then(|v| bp(w, v)) {
        let first = w.dialog.first_speaker;
        let speaker = get_speaker(w, cb.view().obj("Speaker"), first);
        w.dialog.current_speaker = speaker;
        add_involved_unit(w, speaker);
        w.coverage
            .stub("DialogController.CurrentCue: speaker animation, fog reveal, camera, turning");
        elements::run_actions(w, &cb, cb.view().obj("OnShow"));
    }
}

/// `PlayBasicCue(cue)`.
fn play_basic_cue(w: &mut World, c: Guid, b: &Rc<Blueprint>) {
    set_current_cue(w, Some(c));
    apply_alignment_shift(w, b, false);
    let cont = select_cue(w, b, b.view().obj("Continue"));
    let shown_checks = std::mem::take(&mut w.dialog.skill_checks);
    let alignment = player_alignment(w);
    let new_alignment = (w.dialog.old_player_alignment != alignment).then_some(alignment);
    if new_alignment.is_some() {
        w.dialog.old_player_alignment = alignment;
    }
    w.dialog.alignment_shifts.clear();
    let answers = b.view().guids("Answers");
    add_answers(w, &answers, cont);
    w.events.push(
        "dlg_cue_show",
        json!({ "cue": g(c), "skill_checks": shown_checks, "new_alignment": new_alignment }),
    );
    ui_preview_skill_checks(w);
    crate::rulebook::bus::raise_global(w, "IDialogCueHandler", None);
    let exp = b.view().int("Experience");
    if exp != 0 && (w.dialog.current_cue == Some(c) || !w.dialog.local_shown_cues.contains_key(&c))
    {
        let cr_override = dialog_cr_override(w);
        #[allow(clippy::cast_possible_truncation)]
        let xp = crate::xp::get_dialog_xp(w, exp as i32, None, cr_override);
        crate::xp::gain_experience(w, xp);
    }
}

/// The UI's `BlueprintAnswer.SkillChecksDC`, evaluated [`DialogController::ui_preview_calls`]
/// times per answer when a cue is shown: each non-hidden check after the answer is
/// `Calculate()`d, which rolls a real d20.
fn ui_preview_skill_checks(w: &mut World) {
    let n = w.dialog.ui_preview_calls;
    if n == 0 {
        return;
    }
    for a in w.dialog.answers.clone() {
        let Some(ab) = bp(w, a) else { continue };
        let checks: Vec<Guid> = ab
            .view()
            .obj("NextCue")
            .map(|s| s.guids("Cues"))
            .unwrap_or_default()
            .into_iter()
            .filter(|c| {
                bp(w, *c).is_some_and(|cb| {
                    cb.type_name() == "BlueprintCheck" && !cb.view().bool("Hidden")
                })
            })
            .collect();
        if checks.is_empty() {
            continue;
        }
        w.coverage.stub("UI: SkillChecksDC preview rolls (presentation drawing on the dice RNG)");
        for _ in 0..n {
            let mc = w.main_character;
            let unit = select_unit(w, &ab, mc, false);
            for c in &checks {
                let Some(cb) = bp(w, *c) else { continue };
                if cb.view().obj("m_UnitEvaluator").is_some() {
                    w.coverage.unsupported("BlueprintCheck.GetTargetUnit: unit evaluator");
                }
                let stat = StatType(cb.view().i32("Type"));
                let dc = check_dc(w, &cb);
                match unit {
                    Some(u) => {
                        if let Some(mut r) = stat_check_create(w, u, stat, dc) {
                            r.calculate(w);
                        }
                    }
                    None => {
                        let cap = capital_party_checks_enabled(w, &ab);
                        let mut r = RulePartyStatCheck::new(w, stat, dc, cap);
                        r.calculate(w, false);
                    }
                }
            }
        }
    }
}

/// `BlueprintCheck.GetDC()`: the DC plus the modifiers whose conditions pass.
fn check_dc(w: &mut World, cb: &Rc<Blueprint>) -> i32 {
    let mut dc = cb.view().i32("DC");
    for m in cb.view().objs("DCModifiers") {
        let mv = View(m);
        let has =
            !mv.obj("Conditions").map(|c| c.objs("Conditions")).unwrap_or_default().is_empty();
        if has && elements::check_conditions(w, cb, mv.obj("Conditions")) {
            dc = crate::csharp::arith::add(dc, mv.i32("Mod"));
        }
    }
    dc
}

/// `AddAnswers(answers, continueCue)`.
fn add_answers(w: &mut World, answers: &[Guid], cont: Option<Guid>) {
    if cont.is_none() {
        for a in answers {
            let Some(ab) = bp(w, *a) else { continue };
            if ab.type_name() == "BlueprintAnswersList" && answers_list_can_select(w, *a, &ab) {
                w.player.dialog.shown_answer_lists.insert(*a, ());
                w.dialog.local_shown_answer_lists.insert(*a, ());
                let inner = ab.view().guids("Answers");
                add_answers(w, &inner, cont);
                return;
            }
        }
    }
    w.dialog.answers.clear();
    w.dialog.continue_cue = None;
    if let Some(c) = cont {
        if let Some(ca) = continue_answer(w) {
            w.dialog.answers.push(ca);
        }
        w.dialog.continue_cue = Some(c);
    } else {
        for a in answers {
            let Some(ab) = bp(w, *a) else { continue };
            if ab.type_name() == "BlueprintAnswer" && answer_can_show(w, *a, &ab) {
                w.dialog.answers.push(*a);
            }
        }
    }
    if !w.dialog.answers.is_empty() {
        return;
    }
    if let Some(mut seq) = w.dialog.sequences.last().cloned() {
        let next = poll_next_cue(w, &mut seq);
        if let Some(top) = w.dialog.sequences.last_mut() {
            *top = seq.clone();
        }
        if let Some(n) = next {
            if let Some(ca) = continue_answer(w) {
                w.dialog.answers.push(ca);
            }
            w.dialog.continue_cue = Some(n);
            return;
        }
        w.dialog.sequences.pop();
        match sequence_exit(w, seq.blueprint).and_then(|e| bp(w, e)) {
            Some(eb) => {
                let ans = eb.view().guids("Answers");
                let c = select_cue(w, &eb, eb.view().obj("Continue"));
                add_answers(w, &ans, c);
            }
            None => {
                if let Some(e) = exit_answer(w) {
                    w.dialog.answers.push(e);
                }
            }
        }
    } else if let Some(e) = exit_answer(w) {
        w.dialog.answers.push(e);
    }
}

/// `BlueprintAnswersList.CanSelect()`.
fn answers_list_can_select(w: &mut World, a: Guid, ab: &Rc<Blueprint>) -> bool {
    if ab.view().bool("ShowOnce") && w.player.dialog.shown_answer_lists.contains_key(&a) {
        return false;
    }
    elements::check_conditions(w, ab, ab.view().obj("Conditions"))
}

/// `BlueprintAnswer.CanShow()`.
fn answer_can_show(w: &mut World, a: Guid, ab: &Rc<Blueprint>) -> bool {
    let v = ab.view();
    if v.bool("DebugMode") {
        // `DebugMode && !Debug.isDebugBuild`: the shipped build isn't a debug build.
        return false;
    }
    if v.bool("ShowOnce") {
        if v.bool("ShowOnceCurrentDialog") {
            if w.dialog.local_selected_answers.contains_key(&a) {
                return false;
            }
        } else if w.player.dialog.selected_answers.contains_key(&a) {
            return false;
        }
    }
    let show_check = v.obj("ShowCheck");
    let check_type = show_check.map_or(0, |s| s.int("Type"));
    if check_type != 0 {
        if let Some(r) = w.player.dialog.answer_checks.get(&a) {
            if *r == 1 {
                return false;
            }
        } else {
            #[allow(clippy::cast_possible_truncation)]
            let stat = StatType(check_type as i32);
            let dc = show_check.map_or(0, |s| s.i32("DC"));
            let cap = capital_party_checks_enabled(w, ab);
            let mut r = RulePartyStatCheck::new(w, stat, dc, cap);
            crate::rules::trigger_rule(w, &mut r);
            let ok = r.success();
            w.player.dialog.answer_checks.insert(a, i32::from(!ok));
            if !ok {
                return false;
            }
        }
    }
    if v.bool("RequireValidCue") && select_cue(w, ab, v.obj("NextCue")).is_none() {
        return false;
    }
    elements::check_conditions(w, ab, v.obj("ShowConditions"))
}

/// `PlayCheck(check)`.
fn play_check(w: &mut World, c: Guid, b: &Rc<Blueprint>) {
    let v = b.view();
    if v.obj("m_UnitEvaluator").is_some() {
        w.coverage.unsupported("BlueprintCheck.GetTargetUnit: unit evaluator");
    }
    let stat = StatType(v.i32("Type"));
    let dc = check_dc(w, b);
    w.events.push(
        "dlg_check",
        json!({ "check": g(c), "stat": stat.0, "authored_dc": v.i32("DC"), "hidden": v.bool("Hidden") }),
    );
    let unit = w.dialog.acting_unit;
    let result = if let Some(u) = unit {
        // `GameHelper.TriggerStatCheck(RuleStatCheck.Create(...), null, !BanPartyCheckInCamp)`:
        // a party check in capital mode, else the unit's own check.
        if !v.bool("BanPartyCheckInCamp") && w.player.capital_party_mode {
            w.coverage.unsupported("GameHelper.TriggerStatCheck: capital party mode");
        }
        let Some(mut r) = stat_check_create(w, u, stat, dc) else { return };
        crate::rules::trigger_rule(w, &mut r);
        SkillCheckResult {
            unit: Some(w.entity(u).unique_id.clone()),
            stat: r.stat_type.0,
            dc: r.non_negative_dc(),
            passed: r.success(),
            roll_result: r.roll_result(),
            d20: r.d20_value(),
            stat_value: r.stat_value,
        }
    } else {
        let cap = w.dialog.capital_party_checks_enabled;
        let mut r = RulePartyStatCheck::new(w, stat, dc, cap);
        crate::rules::trigger_rule(w, &mut r);
        let roller = r.roller;
        match &r.stat_check {
            Some(sc) => SkillCheckResult {
                unit: roller.map(|u| w.entity(u).unique_id.clone()),
                stat: sc.stat_type.0,
                dc: sc.non_negative_dc(),
                passed: sc.success(),
                roll_result: sc.roll_result(),
                d20: sc.d20_value(),
                stat_value: sc.stat_value,
            },
            None => {
                // `new SkillCheckResult(null, …)` throws a NullReferenceException.
                w.coverage.unsupported("PlayCheck: party check without a roller");
                return;
            }
        }
    };
    if result.passed {
        w.dialog.local_passed_checks.insert(c, ());
        w.dialog.local_failed_checks.remove(&c);
        let exp = v.int("Experience");
        if exp != 0 {
            let cr_override = dialog_cr_override(w);
            #[allow(clippy::cast_possible_truncation)]
            let xp = crate::xp::get_dialog_xp(w, exp as i32, Some(dc), cr_override);
            let roller = result.unit.as_ref().and_then(|id| w.by_id.get(id).copied());
            if let Some(r) = roller {
                crate::xp::gain_experience_for_skill_check(w, xp, r);
            }
        }
    } else if !w.dialog.local_passed_checks.contains_key(&c) {
        w.dialog.local_failed_checks.insert(c, ());
    }
    if !v.bool("Hidden") || result.passed {
        w.dialog.skill_checks.push(result.clone());
    }
    let next = if result.passed { v.guid("m_Success") } else { v.guid("m_Fail") };
    play_cue(w, next);
    // WrathOracle's `PlayCheck` postfix: after the success or failure cue played, reading
    // the dialogue's passed and failed sets then (a check passed earlier in the dialogue
    // stays passed).
    let passed = w.dialog.local_passed_checks.contains_key(&c);
    let failed = w.dialog.local_failed_checks.contains_key(&c);
    w.events.push(
        "dlg_check_end",
        json!({ "check": g(c), "passed": passed, "failed": failed, "result": result }),
    );
}

/// `PlaySequence(sequence)`.
fn play_sequence(w: &mut World, c: Guid) {
    let mut seq = CueSequenceState { blueprint: c, next: 0 };
    w.dialog.sequences.push(seq.clone());
    let first = poll_next_cue(w, &mut seq);
    if let Some(top) = w.dialog.sequences.last_mut() {
        *top = seq;
    }
    match first {
        None => {
            w.log("error", "Could not select first cue in cue sequence");
            stop_dialog(w);
        }
        Some(f) => play_cue(w, Some(f)),
    }
}

// ---- alignment ----------------------------------------------------------------------

fn player_alignment(w: &World) -> i32 {
    w.main_character
        .and_then(|mc| w.unit(mc))
        .and_then(|u| u.alignment.as_ref())
        .map_or(4, crate::narrative::alignment::UnitAlignment::value)
}

/// `AlignmentShiftExtension.ApplyAlignmentShift(provider)` for an answer or a cue.
fn apply_alignment_shift(w: &mut World, b: &Rc<Blueprint>, _is_answer: bool) {
    let Some(s) = b.view().obj("AlignmentShift") else { return };
    let value = s.i32("Value");
    if value < 1 {
        return;
    }
    let dir = s.i32("Direction");
    let Some(mc) = w.main_character else { return };
    let radius = bp(w, crate::unit::BLUEPRINT_ROOT)
        .and_then(|r| r.view().obj("Dialog").map(|d| d.i32("AlignmentRadius")))
        .unwrap_or(0);
    let Some(mut al) = w.unit_mut(mc).and_then(|u| u.alignment.take()) else {
        w.coverage.unsupported("AlignmentShift: main character alignment missing");
        return;
    };
    al.shift(w, dir, value, radius, Some(b.guid));
    w.events.mutation(
        "alignment",
        json!({ "unit": w.entity(mc).unique_id, "direction": dir, "value": value, "vector": al.vector, "now": crate::narrative::alignment::enum_value(al.value()) }),
    );
    if let Some(u) = w.unit_mut(mc) {
        u.alignment = Some(al);
    }
    w.dialog.alignment_shifts.push((dir, value));
}

// ---- trace --------------------------------------------------------------------------

/// The completed transition record (WrathOracle's `dlg_transition`).
fn transition_record(w: &mut World) {
    let d = &w.dialog;
    let rec = json!({
        "dialog": d.dialog.map(g),
        "cue": d.current_cue.map(g),
        "answers": d.answers.iter().map(|a| g(*a)).collect::<Vec<_>>(),
        "continue_cue": d.continue_cue.map(g),
        "scheduled": d.cue_play_scheduled,
        "acting_unit": uid(w, d.acting_unit),
        "current_speaker": uid(w, d.current_speaker),
        // WrathOracle's `SkillCheckResult` form.
        "skill_checks": d.skill_checks.iter().map(|c| json!({
            "unit": c.unit.as_deref().map_or(Value::Null, unit_ref),
            "stat": c.stat, "dc": c.dc, "passed": c.passed, "roll": c.roll_result,
            "d20": c.d20, "stat_value": c.stat_value,
        })).collect::<Vec<_>>(),
        "local_shown_cues": d.local_shown_cues.len(),
        "local_selected_answers": d.local_selected_answers.len(),
        "local_passed_checks": d.local_passed_checks.keys().map(g).collect::<Vec<_>>(),
        "local_failed_checks": d.local_failed_checks.keys().map(g).collect::<Vec<_>>(),
        "global_shown_cues": w.player.dialog.shown_cues.len(),
        "global_selected_answers": w.player.dialog.selected_answers.len(),
        "global_shown_dialogs": w.player.dialog.shown_dialogs.len(),
        "alignment": crate::narrative::alignment::enum_value(player_alignment(w)),
    });
    w.events.push("dlg_transition", rec);
    if w.dialog.text_records {
        text_record(w);
    }
}

// ---- text ---------------------------------------------------------------------------

fn dialog_type(w: &World) -> i64 {
    w.dialog.dialog.and_then(|d| bp(w, d)).map_or(0, |b| b.view().int("Type"))
}

/// Whether the current dialogue is a book event (`DialogType.Book`).
#[must_use]
pub fn current_dialog_is_book(w: &World) -> bool {
    dialog_type(w) == 1
}

/// The current cue's `Speaker.SpeakerPortrait`.
fn speaker_portrait(w: &World) -> Option<Guid> {
    let c = bp(w, w.dialog.current_cue?)?;
    c.view().obj("Speaker")?.guid("m_SpeakerPortrait")
}

/// `CurrentSpeakerBlueprint`: the current cue's speaker portrait, else the current
/// speaker's blueprint.
fn current_speaker_blueprint(w: &World) -> Option<Guid> {
    speaker_portrait(w).or_else(|| w.dialog.current_speaker.and_then(|u| w.unit(u)?.blueprint))
}

/// `CurrentSpeakerName`.
fn current_speaker_name(w: &World) -> Result<String, wr_text::TextError> {
    if let Some(p) = speaker_portrait(w) {
        return crate::text::blueprint_character_name(w, p);
    }
    if let Some(u) = w.dialog.current_speaker {
        return crate::text::character_name(w, u);
    }
    match &w.dialog.custom_speaker_name {
        Some(n) if !n.is_empty() && n != "<null>" => Ok(n.clone()),
        _ => Ok(String::new()),
    }
}

/// `CurrentSpeakerColor` (`None`: null).
fn current_speaker_color(w: &World) -> Result<Option<String>, wr_text::TextError> {
    match current_speaker_blueprint(w) {
        Some(b) => crate::text::blueprint_color_html(w, b).map(Some),
        None => w.dialog.custom_speaker_color.clone().ok_or_else(|| {
            wr_text::TextError::Unsupported(
                "CurrentSpeakerColor: the custom speaker colour from before the run".into(),
            )
        }),
    }
}

/// A cue's or an answer's `DisplayText` (its `Text`).
fn display_text(w: &World, g: Guid) -> Result<String, wr_text::TextError> {
    let b = bp(w, g).ok_or_else(|| wr_text::TextError::Unsupported(format!("{g} missing")))?;
    crate::text::localized_field(w, b.view(), "Text")
}

/// A text value for a record: the text, or null with the reason reported.
fn text_value(w: &mut World, r: Result<String, wr_text::TextError>) -> Value {
    match r {
        Ok(s) => json!(s),
        Err(e) => {
            w.coverage.unsupported(format!("dialogue text: {e}"));
            Value::Null
        }
    }
}

/// `SelectAnswer`'s `AddHistoryEntry(CurrentCue, CurrentSpeakerName, CurrentSpeakerColor)`
/// and `AddHistoryEntry(answer)` (WrathOracle's `dlg_history`: the lines they raise).
fn history_entries(w: &mut World, cue: Guid, answer: Guid, ab: &Rc<Blueprint>) {
    let line = (|| {
        let name = current_speaker_name(w)?;
        let text = display_text(w, cue)?;
        Ok(if name.is_empty() {
            // `NarratorsTextFormat`: `{0}` (the colour, an argument too, isn't used).
            text
        } else {
            let color = current_speaker_color(w)?;
            let narrator = crate::text::narrator_html(w)?;
            match color.filter(|c| !c.is_empty()) {
                Some(c) => {
                    crate::text::formats::speaker_with_color_name("", &c, &name, &text, &narrator)
                }
                None => crate::text::formats::speaker_with_name("", &name, &text, &narrator),
            }
        })
    })();
    let line = text_value(w, line);
    w.events.push("dlg_history", json!({ "kind": "cue", "of": g(cue), "line": line }));
    if !ab.view().bool("AddToHistory") {
        return;
    }
    let line = (|| {
        let text = display_text(w, answer)?;
        if dialog_type(w) != 0 {
            // `AnswerFormatWithoutName`: `{0}`.
            return Ok(text);
        }
        let pc = w
            .main_character
            .ok_or_else(|| wr_text::TextError::Throws("history: no player character".into()))?;
        let name = crate::text::character_name(w, pc)?;
        let bp = w.unit(pc).and_then(|u| u.blueprint).ok_or_else(|| {
            wr_text::TextError::Unsupported("player character's blueprint".into())
        })?;
        let color = crate::text::blueprint_color_html(w, bp)?;
        Ok(crate::text::formats::answer_with_color_name(&color, &name, &text))
    })();
    let line = text_value(w, line);
    w.events.push("dlg_history", json!({ "kind": "answer", "of": g(answer), "line": line }));
}

/// WrathOracle's `dlg_text`, after a transition: the current cue's and the shown answers'
/// display text and the current speaker's name and colour.
fn text_record(w: &mut World) {
    let cue = w.dialog.current_cue;
    let cue_text = match cue {
        Some(c) => {
            let r = display_text(w, c);
            text_value(w, r)
        }
        None => Value::Null,
    };
    let mut answers = Vec::new();
    for a in w.dialog.answers.clone() {
        let r = display_text(w, a);
        let t = text_value(w, r);
        answers.push(json!({ "answer": g(a), "text": t }));
    }
    let name = current_speaker_name(w);
    let name = text_value(w, name);
    let color = match current_speaker_color(w) {
        Ok(c) => json!(c),
        Err(e) => {
            w.coverage.unsupported(format!("dialogue text: {e}"));
            Value::Null
        }
    };
    w.events.push(
        "dlg_text",
        json!({ "cue": cue.map(g), "cue_text": cue_text, "answers": answers, "speaker_name": name, "speaker_color": color }),
    );
}
