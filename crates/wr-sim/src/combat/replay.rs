//! The driven replay (POC-5): hydrate a save, then fire each root rule of a WrathOracle
//! turn-based recording from its snapshot, in recorded order, on the recording's draw
//! tape, and compare the sim's T0 projection (rule, result, draw, mut) with the
//! recording's.
//!
//! What the recording supplies is what the sim doesn't run yet: the player's and the AI's
//! inputs (which attack, when), the attack roll's environment (`env` records), who joins
//! combat (`join_combat` observations), the turn controller's turns, and when the UI, the
//! AI and the commands ask for flat-footed checks and attack counts. A recording with root
//! call sites (WrathOracle's `origin`) has those roots computed in their caller's phase
//! ([`Outcome::by_origin`]); in an older one they are *echoed* (their recorded projection is
//! appended as it is) and counted in [`Outcome::echoed`]: not computed, not verified.
//! Everything else the sim computes: the attack chain, initiative, damage, death and
//! experience.
//!
//! Every recorded event is placed in its frame's controller phase ([`Phase`]); a fact the
//! recording doesn't establish (an event outside its phase, a position at impact, the turn
//! controller's clearing of an ended turn) is reported unsupported, never assumed.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use serde_json::{Value as J, json};

use super::fire::{self, Cx, DrawSink, Recording};
use super::oracle::Trace;
use super::project;
use super::units::AttackEnv;
use crate::world::{EntityIdx, World};

/// Rule classes whose root instances come from the presentation layer (the UI's and the AI's
/// predictions) and the commands' setup: computed where their recorded call site places them,
/// else echoed from the recording.
const ECHOED: &[&str] =
    &["RuleCheckTargetFlatFooted", "RuleCalculateAttacksCount", "RuleCalculateAttacksCountLimbs"];

/// The result of a driven replay.
#[derive(Debug, Default)]
pub struct Outcome {
    /// The recording's projection (rule/result/draw/mut only), exclusions removed.
    pub oracle: Vec<J>,
    /// The sim's.
    pub sim: Vec<J>,
    pub diff: project::Diff,
    /// Presentation root rules left out of the comparison (not computed, not verified), by class.
    pub echoed: BTreeMap<String, usize>,
    /// The records those roots held in the recording (rule and result records only).
    pub echoed_records: usize,
    /// Presentation roots that held a draw, a mutation or a nested rule: not echoed, reported.
    pub echo_rejected: BTreeMap<String, usize>,
    /// Recorded `mut`s left out of the comparison and why (presentation or unported).
    pub excluded: BTreeMap<String, usize>,
    /// Root rules the driver could not run.
    pub skipped: BTreeMap<String, usize>,
    /// Root rules driven.
    pub roots: usize,
    /// Root rules folded into the sim's own logic (the initiative tie-break d20), by class.
    pub folded: BTreeMap<String, usize>,
    /// The unit whose tie-break d20 follows (`UnitCombatPrepareController.Tick`).
    tiebreak: Option<String>,
    /// Unsupported kinds reported by hydration, before the replay.
    pub load_unsupported: usize,
    /// The turn controller's `sort` events checked against `UnitsOrderComaprer` run on the
    /// sim's rolled initiatives (with the recorded overrides and sequences as inputs).
    pub sorts: SortChecks,
    /// Ranged attacks' resolves: positions at impact established by the observations, and not.
    pub impact_positions: [usize; 2],
    /// Driven roots computed with a decision taken from the recording (a fact the recording
    /// doesn't establish, such as the distance at impact): left out of both streams like the
    /// echoed roots, not verified, by class.
    pub unverified: BTreeMap<String, usize>,
    /// The records those roots held in the recording.
    pub unverified_records: usize,
    /// Awake states the sim couldn't establish (`AwakeTimer`, fog of war) taken from the
    /// recording's `awake` observations, per frame and unit.
    pub awake_recorded: usize,
    /// Units' cooldowns taken from the recording (`cooldowns`, at the prepare controller's
    /// prefix) where they differed from the sim's: every time in a recording without WrathOracle's
    /// cooldown records (the sim doesn't compute them then), else only after a reported mismatch.
    pub cooldowns_recorded: usize,
    /// The recorded calls that decide units' cooldowns, taken as inputs (a recording with
    /// WrathOracle's `cd_*` records), by kind: a command acting (`SpendAction`), the movement
    /// agent's delta (`TickMovement`), `ForceToEnd`, a command ending.
    pub cooldown_inputs: BTreeMap<&'static str, usize>,
    /// Units' cooldowns compared with the recording's at the prepare controller's prefixes
    /// (unit-frames).
    pub cooldowns_checked: usize,
    /// Roots of the [`ECHOED`] classes computed at their recorded call site, by caller
    /// ([`Caller`]).
    pub by_origin: BTreeMap<&'static str, usize>,
    /// `TurnController.Tick` calls whose status change and end wait the sim computed (a recording
    /// with WrathOracle's `tt*` records), checked against the recorded status and counters.
    pub turn_ticks: usize,
    /// Turn ends (`TurnController.End`) the sim decided, each checked against a `turn_end`.
    pub turn_ends: usize,
    /// The commands model (a recording with WrathOracle's `uc_*` records): its state checked against
    /// the recorded rows (around each controller tick), and the rows taken after a difference.
    pub commands_checked: usize,
    pub commands_recorded: usize,
    /// Units' commands ticked by the sim's `UnitCommandController` (unit-frames), runs, class-code
    /// calls taken as inputs, ends the sim made (each checked), ends taken as inputs (class code,
    /// the movement agent, the AI), and `SpendAction` calls the sim made (each checked).
    pub commands_ticked: usize,
    pub commands_run: usize,
    pub commands_fx: usize,
    pub commands_ends: usize,
    pub commands_input_ends: usize,
    pub commands_spends: usize,
    /// The turn controller's command reads (`Commands.Empty`, `IsRunning()`) checked against the
    /// sim's commands.
    pub commands_values: usize,
    /// `UnitAttack`'s class code (the `attacks` observer): calls the sim computed (each checked
    /// against its `uc_fx`), and the rules it fired (each checked against its recorded root).
    pub attacks_calls: usize,
    pub attacks_rules: usize,
    /// The `approach` observer: `UnitMoveTo`'s and `UnitAttackOfOpportunity`'s class code the sim
    /// computed (each checked against its `uc_fx`), the movement agent calls it made and
    /// `TickApproaching`'s draws (each checked against its record).
    pub approach_class: usize,
    pub approach_calls: usize,
    pub approach_draws: usize,
    /// The attacks of opportunity's rules the sim fired (each checked against its root).
    pub approach_rules: usize,
    /// The AI (`ai` observer): its entries run, draws taken, commands run, scores checked and
    /// decisions (`SelectAction`) made.
    pub ai_entries: usize,
    pub ai_draws: usize,
    pub ai_runs: usize,
    pub ai_scores: usize,
    pub ai_decisions: usize,
    /// The engagement controller (`engage` observer): its ticks run, engagement changes checked,
    /// attacks of opportunity (each checked) and those it ran, the count's cooldown writes, counts
    /// adopted from a tick (a unit constructed in the window) and rule provokes matched.
    pub eng_entries: usize,
    pub eng_changes: usize,
    pub eng_aoos: usize,
    pub eng_runs: usize,
    pub eng_count_writes: usize,
    pub eng_counts_adopted: usize,
    pub eng_provokes: usize,
    /// Units' targets (`targets` observer): the sim's writes checked, the reads checked.
    pub tgt_checked: usize,
    pub tgt_reads: usize,
    /// The impatience controller (`impatience` observer): its entries run, the handlers' records
    /// matched, the states checked in its entries, its draws.
    pub imp_entries: usize,
    pub imp_handlers: usize,
    pub imp_states: usize,
    pub imp_draws: usize,
    /// The memory controller's `AddToMemory` calls in offensive actions (`memory` observer), each
    /// matched with the sim's.
    pub mem_adds: usize,
    /// The movement agent (`agent` observer): its entries run (`TickUnit`, `PathTo`,
    /// `OnPathComplete`, calls), the states checked, the units ticked, the requests checked, its
    /// draws, and what it still takes from the recording, by kind.
    pub ag_entries: usize,
    pub ag_states: usize,
    pub ag_ticks: usize,
    pub ag_requests: usize,
    pub ag_draws: usize,
    pub ag_inputs: std::collections::BTreeMap<&'static str, usize>,
}

/// Who triggered a root rule, from its recorded call site (`origin`: the game's methods,
/// innermost first, joined by `" < "`), and the frame phase that places it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    /// The UI after `Game.Tick` (UniRx's `LateUpdate`: the initiative tracker; the cursor).
    Ui,
    /// A coroutine after `Game.Tick`: the turn controller's hover predictions and the oracle's
    /// driver issuing the player's commands.
    Input,
    /// `AiBrainController` in the turn controller's tick (`CombatController.Tick`).
    Ai,
    /// `AiBrainController` from `TurnController.Prepare` inside `CombatController.StartTurn`
    /// (`TimeController.Tick`).
    AiStartTurn,
    /// `UnitCommandController`: a command starts or acts.
    Command,
}

impl Caller {
    /// Classifies a recorded call site; `None` for one the replay can't place.
    #[must_use]
    pub fn of(origin: &str) -> Option<Self> {
        let frames: Vec<&str> = origin.split(" < ").collect();
        let has = |p: &str| frames.iter().any(|f| f.starts_with(p));
        if has("Kingmaker.Game.Tick") {
            return if has("Kingmaker.Controllers.Units.UnitCommandController.") {
                Some(Self::Command)
            } else if has("Kingmaker.AI.AiBrainController.")
                && has("TurnBased.Controllers.CombatController.StartTurn")
                && has("Kingmaker.Controllers.TimeController.Tick")
            {
                Some(Self::AiStartTurn)
            } else if has("Kingmaker.AI.AiBrainController.")
                && has("TurnBased.Controllers.TurnController.Tick")
                && has("TurnBased.Controllers.CombatController.Tick")
                && !has("TurnBased.Controllers.CombatController.StartTurn")
            {
                Some(Self::Ai)
            } else {
                None
            };
        }
        if has("UniRx.MainThreadDispatcher.LateUpdate") {
            Some(Self::Ui)
        } else if has("UnityEngine.SetupCoroutine.InvokeMoveNext") {
            Some(Self::Input)
        } else {
            None
        }
    }

    #[must_use]
    pub fn phase(self) -> Phase {
        match self {
            Self::Ui | Self::Input => Phase::Post,
            Self::Ai => Phase::Turn,
            Self::AiStartTurn => Phase::Time,
            Self::Command => Phase::Commands,
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Input => "input",
            Self::Ai => "ai",
            Self::AiStartTurn => "ai (StartTurn)",
            Self::Command => "command",
        }
    }
}

/// What the `sort` events showed.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SortChecks {
    pub checked: usize,
    /// Orders equal to the recorded one.
    pub equal: usize,
    /// Orders that needed the comparer's identity-hash fallback (the recording decides).
    pub hash_fallback: usize,
    /// Rolled initiatives that differ from the recorded ones.
    pub initiative_mismatch: usize,
    /// Sorts whose units (`m_Units`) differ from the sim's.
    pub members_mismatch: usize,
}

fn unit_id(v: &J) -> Option<&str> {
    v.get("$unit").and_then(J::as_str)
}

fn short(class: &str) -> &str {
    class.rsplit(['.', '+']).next().unwrap_or(class)
}

/// The kinds the driven replay compares.
fn compared(r: &J) -> bool {
    matches!(r["k"].as_str(), Some("rule" | "result" | "bonuses" | "draw" | "mut"))
}

/// `GameModeType`s the replay models.
pub mod mode {
    pub const DEFAULT: i64 = 1;
    pub const DIALOG: i64 = 3;
    pub const PAUSE: i64 = 4;
    pub const CUTSCENE: i64 = 7;
}

/// The end index of the root rule starting at `i` (its `rule_end`).
fn root_end(recs: &[J], i: usize) -> usize {
    let id = recs[i]["id"].as_i64().unwrap_or(-1);
    (i..recs.len())
        .find(|&j| recs[j]["k"] == "rule_end" && recs[j]["id"].as_i64() == Some(id))
        .unwrap_or(recs.len() - 1)
}

fn is_root(r: &J) -> bool {
    r["k"] == "rule" && r["depth"].as_i64() == Some(0)
}

/// The presentation roots of a recording without call sites: the record spans of root rules of
/// an [`ECHOED`] class that hold nothing but their own rule and result (no draw, no mutation,
/// no nested rule), and the classes of those that hold more. A root with a recorded `origin`
/// is computed instead ([`plan_origins`]).
fn plan_echoes(t: &Trace) -> (Vec<(usize, usize)>, BTreeMap<String, usize>) {
    let recs = &t.records;
    let (mut ok, mut rejected) = (Vec::new(), BTreeMap::new());
    let mut i = 0;
    while i < recs.len() {
        let r = &recs[i];
        if is_root(r) {
            let end = root_end(recs, i);
            let class = short(r["class"].as_str().unwrap_or(""));
            if ECHOED.contains(&class) && r.get("origin").is_none() {
                if echo_span_is_inert(&recs[i + 1..=end]) {
                    ok.push((i, end));
                } else {
                    *rejected.entry(class.to_owned()).or_default() += 1;
                }
            }
            i = end + 1;
        } else {
            i += 1;
        }
    }
    (ok, rejected)
}

/// A presentation root may be echoed only if nothing it contains changes state or consumes the
/// tape: no sim-class draw, no mutation, no nested rule, no attack environment.
fn echo_span_is_inert(inner: &[J]) -> bool {
    inner.iter().all(|r| match r["k"].as_str() {
        Some("draw") => r["c"] == "pres",
        Some("mut" | "env") => false,
        Some("rule") => false,
        _ => true,
    })
}

/// The roots of an [`ECHOED`] class with a recorded call site, by start: their end, and their
/// caller when the call site places them and the span is inert but for its own environment
/// (`env`, a root flat-footed check's).
fn plan_origins(t: &Trace) -> HashMap<usize, (usize, Option<Caller>)> {
    let recs = &t.records;
    let mut out = HashMap::new();
    let mut i = 0;
    while i < recs.len() {
        let r = &recs[i];
        if !is_root(r) {
            i += 1;
            continue;
        }
        let end = root_end(recs, i);
        let class = short(r["class"].as_str().unwrap_or(""));
        if ECHOED.contains(&class)
            && let Some(origin) = r["origin"].as_str()
        {
            let inert = origin_span_is_inert(r, &recs[i + 1..=end]);
            out.insert(i, (end, Caller::of(origin).filter(|_| inert)));
        }
        i = end + 1;
    }
    out
}

/// A root computed at its call site may hold only its own rule's steps and handlers, draws of the
/// presentation class and at most one `env`, its own (a flat-footed check's, for the same units):
/// no observation (a combat join), mutation, nested rule, sim draw or handler exception.
fn origin_span_is_inert(root: &J, inner: &[J]) -> bool {
    let class = short(root["class"].as_str().unwrap_or(""));
    let own_env = |x: &J| {
        class == "RuleCheckTargetFlatFooted"
            && x["rule"] == class
            && unit_id(&x["initiator"]) == unit_id(&root["initiator"])
            && unit_id(&x["target"]) == unit_id(&root["target"])
    };
    let envs = inner.iter().filter(|x| x["k"] == "env").count();
    envs <= 1
        && inner.iter().all(|x| match x["k"].as_str() {
            Some("about" | "trig" | "trig_end" | "did" | "rule_end" | "h" | "h_end") => true,
            Some("draw") => x["c"] == "pres",
            Some("env") => own_env(x),
            _ => false,
        })
}

/// The recording's records without the given spans.
fn without(t: &Trace, spans: &[(usize, usize)]) -> Vec<J> {
    t.records
        .iter()
        .enumerate()
        .filter(|(i, _)| !spans.iter().any(|(a, b)| a <= i && i <= b))
        .map(|(_, r)| r.clone())
        .collect()
}

/// The recording's stream, as compared: the compared kinds only, without the echoed and the
/// unverified roots.
fn oracle_stream(
    t: &Trace,
    echoed: &[(usize, usize)],
    unverified: &[(usize, usize)],
    out: &mut Outcome,
) -> Vec<J> {
    let n = |spans: &[(usize, usize)]| project::project_records(t, &without(t, spans)).len();
    let all = n(&[]);
    out.echoed_records = all.saturating_sub(n(echoed));
    out.unverified_records = all.saturating_sub(n(unverified));
    let kept = without(t, &[echoed, unverified].concat());
    let mut v = Vec::new();
    for r in project::project_records(t, &kept) {
        if !compared(&r) {
            continue;
        }
        // The tutorial system (`Kingmaker.Tutorial`) is UI-driven; its facts are not ported.
        if r["k"] == "mut" && r["class"] == "Tutorial" {
            *out.excluded.entry("fact_attach Tutorial".into()).or_default() += 1;
            continue;
        }
        v.push(r);
    }
    v
}

/// The phases of a frame the replay places events in or runs, in the registration order of
/// `GameModesFactory.Initialize` (`GameMode.Tick` runs its controllers in that order), then what
/// runs after `Game.Tick` (`Runner.Update`) in the same Unity frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// `GameMode.Tick`'s prefix: the `tick` record, the observation rows and gates.
    Prefix,
    /// `UnitGroupsController`: units back in their groups, empty groups dropped.
    Groups,
    /// `TimeController.Tick` (the `time` record is its postfix; `CombatController.StartTurn`
    /// runs inside it, from `TickTime`).
    Time,
    /// `CombatController.Tick`'s prefix (the oracle's `turn_tick`).
    TurnPrefix,
    /// `CombatController.Tick` (`TurnBasedCombatController`): with the units changed,
    /// `HasEnemyInCombat`; then the current turn ticks (`turn_end`).
    Turn,
    /// The rest of `CombatController.Tick`: an ended (or missing) turn makes it choose the next
    /// unit, and is cleared (`turn_clear`).
    TurnChoose,
    /// `UnitFollowUnitController`: followers' moves (commands, run there).
    Follow,
    /// `UnitMoveController`: units move; the current unit's movement spends its turn's budget
    /// (`TurnController.TickMovement`, the oracle's `cd_move`).
    Move,
    /// `SleepingUnitsController`: the frame's awake units.
    Sleeping,
    /// `UnitCombatJoinController`: combat joins (and the party's combat start).
    Join,
    /// `UnitCombatPrepareController`: initiative.
    Prepare,
    /// `UnitCombatCooldownsController`: cooldowns run down.
    Cooldowns,
    /// `UnitImpatienceController`.
    Impatience,
    /// `AiBrainController` (outside the turn controller's own ticks).
    Ai,
    /// `UnitCombatEngagementController`: attacks of opportunity are run.
    Engagement,
    /// `UnitCommandController`: commands act (an attack's root rule).
    Commands,
    /// `UnitBuffsController`.
    Buffs,
    /// `UnitRoamingController`: roaming units' moves.
    Roaming,
    /// `ProjectileController`: a projectile arrives (its resolve).
    Projectiles,
    /// `FxSpawnController`: queued effects spawn (`DismemberUnitFX.Init`'s `AddBuff`).
    Fx,
    /// `UnitLifeController`.
    Life,
    /// `UnitCombatLeaveController.Tick`'s prefix: the oracle's `leave_tick` and `cmds` inputs.
    LeavePrefix,
    /// `UnitCombatLeaveController`.
    Leave,
    /// `UnitProneController`.
    Prone,
    /// `UnitHelplessController`.
    Helpless,
    /// `InspectUnitsController`: dead units' knowledge checks.
    Inspect,
    /// After `Game.Tick`: the UI, coroutines (UniRx timers), the oracle's inputs.
    Post,
}

/// The phases that run controller code of their own.
const WORK: [Phase; 12] = [
    Phase::Groups,
    Phase::Turn,
    Phase::TurnChoose,
    Phase::Move,
    Phase::Sleeping,
    Phase::Cooldowns,
    Phase::Buffs,
    Phase::Life,
    Phase::Leave,
    Phase::Helpless,
    Phase::Inspect,
    Phase::Post,
];

/// Whether game mode `m` registers the phase's controller (`GameModesFactory.Initialize`);
/// `None` for a mode the replay doesn't model.
#[must_use]
pub fn registered(m: i64, p: Phase) -> Option<bool> {
    use mode::{CUTSCENE, DEFAULT, DIALOG, PAUSE};
    if ![DEFAULT, DIALOG, PAUSE, CUTSCENE].contains(&m) {
        return None;
    }
    Some(match p {
        Phase::Prefix | Phase::Post => true,
        Phase::Sleeping | Phase::Helpless => true,
        Phase::Time | Phase::Commands | Phase::Projectiles | Phase::Fx | Phase::Life => m != PAUSE,
        Phase::Join | Phase::Prepare | Phase::LeavePrefix | Phase::Leave => m == DEFAULT,
        Phase::Groups | Phase::Move => m != PAUSE,
        Phase::Cooldowns => m == DEFAULT,
        Phase::Inspect => m == DEFAULT || m == DIALOG || m == CUTSCENE,
        Phase::TurnPrefix | Phase::Turn | Phase::TurnChoose => m == DEFAULT || m == CUTSCENE,
        Phase::Buffs => m == DEFAULT || m == DIALOG,
        Phase::Follow | Phase::Impatience | Phase::Ai | Phase::Engagement => m == DEFAULT,
        Phase::Roaming | Phase::Prone => m == DEFAULT || m == DIALOG || m == CUTSCENE,
    })
}

/// Why an event can't be placed in its frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Misplaced {
    /// The frame is already past the event's phase: the recorded order contradicts the
    /// controllers' order.
    Late,
    /// The frame's game mode doesn't run the phase's controller.
    NotRegistered,
    /// A game mode the replay doesn't model, or no frame yet.
    NoModel,
}

/// One recorded frame: its game mode and the last phase reached.
#[derive(Debug)]
pub struct Frame {
    mode: Option<i64>,
    at: Phase,
}

impl Default for Frame {
    fn default() -> Self {
        Self { mode: None, at: Phase::Post }
    }
}

impl Frame {
    /// A `tick` record: the previous frame's remaining phases, then a new frame.
    pub fn start(&mut self, m: i64) -> Vec<Phase> {
        let rest = self.finish();
        self.mode = Some(m);
        self.at = Phase::Prefix;
        rest
    }

    /// The end of a frame (the next `tick` record or the end of the recording): its work phases
    /// that haven't run.
    pub fn finish(&mut self) -> Vec<Phase> {
        let rest = match self.mode.and_then(|m| registered(m, Phase::Post).map(|_| m)) {
            Some(m) => self.work_until(m, Phase::Post),
            None => Vec::new(),
        };
        self.mode = None;
        self.at = Phase::Post;
        rest
    }

    fn work_until(&self, m: i64, p: Phase) -> Vec<Phase> {
        WORK.iter()
            .copied()
            .filter(|q| self.at < *q && *q <= p && registered(m, *q) == Some(true))
            .collect()
    }

    /// An event of phase `p`: the work phases before it (and `p`'s own, for [`Phase::Post`])
    /// that haven't run, in order.
    ///
    /// # Errors
    ///
    /// The event can't be placed ([`Misplaced`]); nothing is run then.
    pub fn to(&mut self, p: Phase) -> Result<Vec<Phase>, Misplaced> {
        let Some(m) = self.mode else {
            // Before the first frame only an input (after `Game.Tick`) can happen.
            return if p == Phase::Post { Ok(Vec::new()) } else { Err(Misplaced::NoModel) };
        };
        match registered(m, p) {
            None => return Err(Misplaced::NoModel),
            Some(false) => return Err(Misplaced::NotRegistered),
            Some(true) => {}
        }
        if p < self.at {
            return Err(Misplaced::Late);
        }
        let run = self.work_until(m, p);
        self.at = p;
        Ok(run)
    }
}

/// The sim's T0 events in the projection's vocabulary.
#[must_use]
pub fn sim_stream(w: &World) -> Vec<J> {
    sim_stream_without(w, &[])
}

/// [`sim_stream`] without the event log's index ranges `skip` (`start..end`).
fn sim_stream_without(w: &World, skip: &[(usize, usize)]) -> Vec<J> {
    let mut v = Vec::new();
    for (i, e) in w.events.records.iter().enumerate() {
        if skip.iter().any(|(a, b)| *a <= i && i < *b) {
            continue;
        }
        let mut m = e.fields.clone();
        let rec = match e.k {
            "rule" | "result" | "bonuses" | "draw" => {
                m.insert("k".into(), json!(e.k));
                J::Object(m)
            }
            "mut" => {
                let what = m.get("what").and_then(J::as_str).unwrap_or("").to_owned();
                match what.as_str() {
                    "damage" => {
                        json!({"k": "mut", "what": what, "unit": m["unit"], "old": m["old"], "new": m["new"]})
                    }
                    "xp" => {
                        json!({"k": "mut", "what": what, "unit": m["unit"], "gained": m["gained"], "total": m["total"]})
                    }
                    // The tutorial system's facts are left out of both streams (UI-driven); a
                    // `ShowNewTutorial` the sim ran creates one, and the recorded attach's id is
                    // checked against it (`Driver::step`).
                    "fact_attach" | "fact_detach" if m["class"] == "Tutorial" => continue,
                    "fact_attach" | "fact_detach" => {
                        json!({"k": "mut", "what": what, "owner": m["owner"],
                        "blueprint": m["fact"]["blueprint"], "class": m["class"]})
                    }
                    _ => continue,
                }
            }
            _ => continue,
        };
        v.push(rec);
    }
    v
}

/// Hydrates `save` with the clocks at the fixture run's start state, and loads the run's
/// trace.
///
/// # Errors
///
/// A missing save, content or capture file.
pub fn prepare_world(
    save: &str,
    dir: &std::path::Path,
    run_name: &str,
) -> anyhow::Result<(World, Trace)> {
    use anyhow::Context as _;
    let path = wr_save::report::find_save(save)?;
    let s = crate::hydrate::Save::read(&path)?;
    let content = Rc::new(crate::content::Content::open_default()?);
    let registry = Rc::new(crate::components::registry());
    let (mut w, _) =
        crate::hydrate::hydrate(content, registry, &s, &crate::hydrate::Options::default())?;
    let trace = Trace::load(&dir.join(format!("{run_name}.trace.jsonl.gz")))?;
    let start: J = serde_json::from_slice(
        &std::fs::read(dir.join(format!("state-start-{run_name}.json")))
            .context("state-start dump")?,
    )?;
    if let (Some(gt), Some(rt)) =
        (start["game"]["game_time"].as_i64(), start["game"]["real_time"].as_i64())
    {
        w.clock.game_time = crate::csharp::timespan::TimeSpan(gt);
        w.clock.real_time = crate::csharp::timespan::TimeSpan(rt);
    }
    Ok((w, trace))
}

/// [`prepare_world`], then [`run`].
///
/// # Errors
///
/// A missing save, content or capture file.
pub fn run_fixture(
    save: &str,
    dir: &std::path::Path,
    run_name: &str,
) -> anyhow::Result<(Outcome, World)> {
    let (mut w, trace) = prepare_world(save, dir, run_name)?;
    let load_unsupported = w.coverage.unsupported.len();
    let mut out = run(&mut w, &trace);
    out.load_unsupported = load_unsupported;
    Ok((out, w))
}

/// Replays `trace`'s root rules on `w` (a hydrated save at the recording's start).
pub fn run(w: &mut World, trace: &Trace) -> Outcome {
    run_with_tape(w, trace, trace)
}

/// A unit's observation row (`Observe.Row`): its position and whether its view is moving, at a
/// frame's prefix.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    pub pos: [f32; 3],
    pub moving: bool,
}

impl Row {
    /// `x,y,z|mv=0|...`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.split('|');
        let head: Vec<f32> =
            parts.next()?.split(',').map(str::parse).collect::<Result<_, _>>().ok()?;
        let [x, y, z] = head[..] else { return None };
        let mv = parts.find_map(|p| p.strip_prefix("mv="))?;
        Some(Self { pos: [x, y, z], moving: mv != "0" })
    }
}

/// What the observation rows establish about units' positions, frame by frame.
///
/// `Observe.Tick` writes, at each frame's prefix, a row for each unit of `State.AwakeUnits` that
/// was awake at the window's first tick, when its row changed. So for such a unit, at a frame
/// where it is (established) awake, the latest row is its state at that prefix; at any other
/// frame the recording says nothing about it.
#[derive(Debug, Default)]
pub struct Positions {
    /// The units with rows (awake at the first observed tick).
    pub known: HashSet<String>,
    pub last: HashMap<String, Row>,
    /// Per unit, its row at each prefix where it was established awake.
    pub seen: HashMap<String, BTreeMap<i64, Row>>,
}

impl Positions {
    /// The units whose rows the window's first observed tick wrote.
    #[must_use]
    pub fn from_trace(t: &Trace) -> Self {
        let rows = || t.records.iter().filter(|r| r["k"] == "obs" && r["ev"] == "u");
        let first = rows().next().and_then(|r| r["t"].as_i64());
        let known = rows()
            .filter(|r| r["t"].as_i64() == first && r["u"] != "~fx")
            .filter_map(|r| r["u"].as_str().map(str::to_owned))
            .collect();
        Self { known, ..Self::default() }
    }

    /// The frame's prefix: the rows of the units established awake there.
    pub fn note_prefix(&mut self, w: &World, tick: i64) {
        for &u in &w.combat.awake {
            let id = &w.entity(u).unique_id;
            if let (true, Some(row)) = (self.known.contains(id), self.last.get(id)) {
                self.seen.entry(id.clone()).or_default().insert(tick, *row);
            }
        }
    }

    /// Whether `unit` provably stood still at `at` from the roll (in frame `roll`) to the
    /// impact (in frame `impact`): at every frame prefix after the roll up to and including the
    /// impact's, and at the next frame's (`next`: its row there, if established), it was
    /// observed there and not moving. The roll's position is the `env` record's; the next
    /// prefix bounds the impact frame's own movement (`UnitMoveController` runs before
    /// `ProjectileController`).
    #[must_use]
    pub fn still(
        &self,
        unit: &str,
        at: [f32; 3],
        roll: i64,
        impact: i64,
        next: Option<Row>,
    ) -> bool {
        let want = Row { pos: at, moving: false };
        let Some(seen) = self.seen.get(unit) else { return false };
        (roll + 1..=impact).all(|t| seen.get(&t) == Some(&want)) && next == Some(want)
    }
}

/// The driver's state across the recording.
struct Driver<'t> {
    trace: &'t Trace,
    cx: Cx,
    /// Echoed root spans, by start.
    echoed: HashMap<usize, usize>,
    /// Roots computed at their recorded call site ([`plan_origins`]), by start.
    origins: HashMap<usize, (usize, Option<Caller>)>,
    echo_spans: Vec<(usize, usize)>,
    /// The attacks' root rules by recorded id (their positions at the roll).
    root_at: HashMap<i64, usize>,
    frame: Frame,
    /// This frame's `tick` record, its tick, and whether its `time` record was seen.
    frame_at: Option<usize>,
    tick: i64,
    time_seen: bool,
    /// The last frame's game mode.
    mode: Option<i64>,
    pos: Positions,
    /// Per unit, the command classes started and not ended (`cmd` records).
    running: HashMap<String, Vec<String>>,
    /// Unverified roots: their record spans, and the sim's event log ranges they wrote.
    unverified: Vec<(usize, usize)>,
    sim_skip: Vec<(usize, usize)>,
    /// The recording has WrathOracle's turn-controller records (`turn_start_begin`,
    /// `turn_clear`, `loot_timer`); older ones don't.
    turn_records: bool,
    /// This frame's `turn_start_begin`: its index and whether `StartTurn` went past its early
    /// return.
    turn_begin: Option<(usize, bool)>,
    /// `State.AwakeUnits` at each recorded frame prefix (the `awake_set` and `awake`
    /// observations), by tick.
    awake_sets: BTreeMap<i64, std::collections::BTreeSet<String>>,
    /// `Player.destroy_marks` already reported.
    destroy_marks: usize,
    /// The turn port runs (the recording has WrathOracle's `choose_next` records): the turn
    /// controller's order, unit groups and the leave controller are the sim's.
    port: bool,
    /// This frame's `CombatController.Tick` went past its units-changed check.
    turn_go_on: bool,
    /// The game mode of a frame whose remaining phases run as the next frame starts (or the
    /// recording ends).
    draining: Option<i64>,
    /// A recorded `StartTurn` ran in this frame's `TimeController.Tick`.
    started_this_frame: bool,
    /// The recorded `ChooseNextUnit` inputs, by tick.
    choose_by_tick: BTreeMap<i64, Vec<super::turns::ChooseInput>>,
    /// The cooldowns are computed (the recording has WrathOracle's `cd_*` records, with the turn
    /// port): the recorded `cooldowns` rows are checks.
    cd: bool,
    /// The last recorded `cooldowns` row of each unit.
    cd_rows: HashMap<String, super::Cooldowns>,
    /// The tick whose `cooldowns` rows were last checked against the sim's.
    cd_checked: Option<i64>,
    /// The current turn's `TurnController.Prepare` cleared its rider's cooldowns, and its `End`
    /// wrote them.
    turn_prepared: bool,
    turn_end_applied: bool,
    /// The turn controller's status is computed (the recording has WrathOracle's `tt*` records,
    /// with the cooldowns computed): the recorded `turn_end` is a check.
    tt: bool,
    /// The tick of the last `tt` record (`TurnController.Tick` ran in it).
    tt_tick: Option<i64>,
    /// The sim's tick ended the turn: the recording's `turn_end` must follow in it.
    tt_end_due: bool,
    /// A `tt` whose `tt_end` hasn't come yet.
    tt_open: bool,
    /// The tick's status change waiting for its `IsAbleToAct` calls: its inputs and `IsAllActed`.
    tt_sc: Option<(super::turn_tick::StatusInput, bool)>,
    /// The next record to process (an entry of the commands model pulls the records after it).
    i: usize,
    /// Units' commands are computed (the recording has WrathOracle's `uc_*` records, with the turn
    /// controller's tick computed).
    uc: bool,
    uc_rows: commands::Rows,
    /// Skipping an entry's records after a desync, the commands' state not established until the
    /// next rows, lifecycle code running, a `SpendAction` the sim's commands made.
    uc_skipping: bool,
    uc_dirty: bool,
    uc_active: usize,
    uc_spend_ok: bool,
    /// The tick of the last `uc_ctl` (the command controller ticked in it).
    uc_ctl_tick: Option<i64>,
    /// The active command entry's call site (its record's origin, or the controller's chain).
    uc_origin: String,
    /// `TickApproaching` is the sim's (the `approach` observer): its draws are lifecycle records,
    /// of these numeric sites (the trace's table).
    uc_approach: bool,
    uc_approach_sites: Vec<i64>,
    /// The AI is the sim's (the `ai` observer): `SelectAction`'s draws are lifecycle records, of
    /// these numeric sites.
    ai: bool,
    ai_sites: Vec<i64>,
    /// The engagement controller is the sim's (the `engage` observer); the frame of its last tick.
    eng: bool,
    eng_tick_frame: Option<i64>,
    /// The units an `eng_prov` provoked since the last engagement tick, one per call; whether the
    /// window's initial engagement (`eng_init`) is closed (the first engagement tick ran).
    eng_prov_seen: Vec<(EntityIdx, &'static str)>,
    eng_init_closed: bool,
    /// The targets are the sim's (the `targets` observer); the window's initial targets
    /// (`tgt_init`) are closed (its first frame ended).
    tgt: bool,
    tgt_init_closed: bool,
    /// `eng_init`'s units in order (`tgt_init` follows them) and the `tgt_init`s seen.
    tgt_init_units: Vec<EntityIdx>,
    tgt_init_n: usize,
    /// The `UnitCombatState.Clear` writing (after its `cd_clear`), and each unit's last
    /// `leave_combat` callers.
    tgt_clear: targets::TgtClear,
    tgt_leave_by: std::collections::HashMap<EntityIdx, String>,
    /// The driver's click waiting for its `Run`; the active `Run`s' recorded chains.
    tgt_click: Option<targets::TgtClick>,
    tgt_runs: Vec<String>,
    /// The impatience controller is the sim's (the `impatience` observer): the `imp_init`s seen
    /// and whether their block closed, the frame of its last tick, the `UpdatePriorityTarget`
    /// entries from `TurnController.Prepare` not yet matched, its draws' numeric sites.
    imp: bool,
    /// The movement agent is the sim's (the `agent` observer with `ag_init`): path contents by
    /// key, the agents in `AllAgents` order, each unit's last recorded agent state, the current
    /// `TickMovement`'s and `PathTo`'s records, `SlowDown`'s draw sites, the grid's erosion
    /// settings (the area's nav file).
    ag: bool,
    ag_paths: HashMap<String, Vec<[f32; 3]>>,
    ag_all: Vec<EntityIdx>,
    ag_last_state: HashMap<EntityIdx, J>,
    ag_tick_rec: Option<J>,
    ag_req_rec: Option<J>,
    ag_slow_sites: Vec<i64>,
    ag_erosion: Result<Option<(i32, i32)>, String>,
    /// The agents whose next recorded state is taken (a real-time span came before it).
    ag_after_rt: std::collections::HashSet<EntityIdx>,
    /// The frame of each unit's last `TickUnit`.
    ag_last_ctl: HashMap<EntityIdx, i64>,
    /// The capture records FindPath's callback (`ag_findclr`).
    ag_has_findclr: bool,
    imp_init_n: usize,
    imp_init_closed: bool,
    imp_tick_frame: Option<i64>,
    imp_prepared: Vec<EntityIdx>,
    imp_prepare_tail: Option<String>,
    /// `Prepare`'s rider once its cooldowns' `Clear` came (its `UpdatePriorityTarget` may follow).
    imp_prepare_ready: Option<EntityIdx>,
    imp_sites: Vec<i64>,
    /// The frame of the last `AiBrainController.Tick` entry; the turn controller's `ForceTick`
    /// entries since its last tick start (its forced AI tick) and since its last `Prepare`; the
    /// active `ForceTick`'s call site.
    ai_tick_frame: Option<i64>,
    ai_forced: u32,
    ai_prepared: u32,
    ai_force_origin: String,
    /// `Time.time` as the frame's first AI read had it (frame, bits).
    ai_time: Option<(i64, f32)>,
    /// The rule ids the trace allocated (what a rule reference may name).
    rule_ids: HashSet<i64>,
    /// Each achievement handler's multiplicity in attacks' rules (see `span_ok`).
    achievements: std::cell::RefCell<HashMap<String, usize>>,
    /// The trace is a replay's (`begin.mode`).
    replay_mode: bool,
    /// The game's ids of units' limb items (fresh at load in the game and the sim), by the sim's
    /// item (a limb granted again holds a new one), as the limbs rule's slots (or a first attack
    /// list) showed them.
    limb_ids: HashMap<EntityIdx, String>,
    out: Outcome,
}

/// A unit's recorded commands (`cmds`: `Raw` then `Queue`, `class|target|SF` each).
fn parse_commands(s: &str) -> Vec<super::groups::Command> {
    s.split(['/', ';'])
        .filter(|c| !c.is_empty())
        .filter_map(|c| {
            let mut it = c.split('|');
            let class = it.next()?.to_owned();
            let target = it.next().filter(|t| !t.is_empty()).map(str::to_owned);
            let flags = it.next()?.as_bytes();
            Some(super::groups::Command {
                class,
                target,
                started: flags.first() == Some(&b'1'),
                finished: flags.get(1) == Some(&b'1'),
            })
        })
        .collect()
}

/// A unit's commands in the sim's model as WrathOracle's `cmds` rows have them (`Raw`, then `Queue`).
fn model_commands(w: &World, unit: &str) -> Vec<super::groups::Command> {
    let m = &w.combat.commands;
    m.all_of(unit)
        .iter()
        .filter_map(|c| m.get(*c))
        .map(|x| super::groups::Command {
            class: x.class.clone(),
            target: x.s.target.unit().map(str::to_owned),
            started: x.s.started,
            finished: x.s.finished,
        })
        .collect()
}

/// A recorded `sort` entry: unit, initiative, override, sequence.
type SortEntry<'a> = (Option<&'a str>, Option<i64>, Option<i64>, Option<i64>);

/// A recorded group row of `leave_tick`: `count|timer|dict=…|list=…|f=…|af=…|efe=…`.
struct GroupRow {
    in_combat: i32,
    timer: f32,
    memory: super::groups::Memory,
    factions: String,
}

impl GroupRow {
    fn parse(s: &str) -> Option<Self> {
        let mut it = s.splitn(3, '|');
        let in_combat = it.next()?.parse().ok()?;
        let timer = it.next()?.parse().ok()?;
        let rest = it.next()?;
        let (mem, factions) = rest.split_once("|f=")?;
        let (dict, list) = mem.strip_prefix("dict=")?.split_once("|list=")?;
        let ids = |s: &str| s.split(',').filter(|x| !x.is_empty()).map(str::to_owned).collect();
        Some(Self {
            in_combat,
            timer,
            memory: super::groups::Memory {
                dict: ids(dict),
                list: (list != "~invalid").then(|| ids(list)),
            },
            factions: format!("f={factions}"),
        })
    }
}

/// Two faction rows (`f=…|af=…|efe=…`) agree: the same faction counts (`m_Factions` is a
/// dictionary whose key order follows its history, not the members'), attack factions and flag.
fn same_factions(a: &str, b: &str) -> bool {
    let split = |s: &str| -> Option<(Vec<String>, String)> {
        let (f, rest) = s.strip_prefix("f=")?.split_once('|')?;
        let mut f: Vec<String> =
            f.split(',').filter(|x| !x.is_empty()).map(str::to_owned).collect();
        f.sort();
        Some((f, rest.to_owned()))
    };
    split(a).is_some_and(|x| Some(x) == split(b))
}

/// A group's faction caches in WrathOracle's form (`f=…|af=…|efe=…`), from its members.
fn faction_row(w: &World, g: super::groups::GroupIdx) -> String {
    let (f, af, efe) = super::groups::factions(w, g);
    let f: Vec<String> = f.iter().map(|(g, n)| format!("{g}:{n}")).collect();
    let af: Vec<String> = af.iter().map(|g| g.to_string()).collect();
    format!("f={}|af={}|efe={}", f.join(","), af.join(","), u8::from(efe))
}

/// [`run`] with the draws taken from another trace's tape (a perturbed tape must make the
/// comparison fail).
pub fn run_with_tape(w: &mut World, trace: &Trace, tape_trace: &Trace) -> Outcome {
    let mut d = Driver::start(w, trace, tape_trace);
    while d.i < trace.records.len() {
        let i = d.i;
        d.i = d.record(w, i);
    }
    d.finish(w)
}

impl<'t> Driver<'t> {
    /// The driver at the recording's start, with the tape installed.
    fn start(w: &mut World, trace: &'t Trace, tape_trace: &Trace) -> Self {
        let mut out = Outcome::default();
        let (spans, rejected) = plan_echoes(trace);
        out.echo_rejected = rejected;
        // The recording's tape; draws made inside rules reach the event log.
        let sink = Rc::new(RefCell::new(DrawSink::default()));
        let tape = tape_trace.tape();
        w.rng = Box::new(Recording { inner: tape, sink: sink.clone() });
        w.events.on = true;
        // Turn-based mode, combat and the game mode follow the recording's own records.
        w.combat.tb_mode_on = false;
        w.combat.combat_active = false;
        w.combat.cutscene = false;
        super::life::update_turn_based(w);
        let cx = Cx { sink };
        fire::set_current(w, Some(cx.clone()));
        let recs = &trace.records;
        let port = recs.iter().any(|r| r["k"] == "obs" && r["ev"] == "choose_next")
            && recs.iter().any(|r| r["k"] == "obs" && r["ev"] == "turn_tick");
        let mut choose_by_tick: BTreeMap<i64, Vec<super::turns::ChooseInput>> = BTreeMap::new();
        if port {
            w.combat.turns = super::turns::Turns { on: true, ..Default::default() };
            // `UnitGroupsController.Start()` after the load; the window's first record checks it.
            super::groups::start(w);
            for r in recs.iter().filter(|r| r["k"] == "obs" && r["ev"] == "choose_next") {
                match super::turns::ChooseInput::parse(r) {
                    Some(c) => {
                        choose_by_tick.entry(r["t"].as_i64().unwrap_or(-1)).or_default().push(c)
                    }
                    None => {
                        w.coverage.unsupported("driven replay: a choose_next record it can't read")
                    }
                }
            }
        }
        let d = Driver {
            trace,
            cx,
            echoed: spans.iter().copied().collect(),
            origins: plan_origins(trace),
            echo_spans: spans,
            root_at: recs
                .iter()
                .enumerate()
                .filter(|(_, r)| is_root(r))
                .filter_map(|(i, r)| r["id"].as_i64().map(|id| (id, i)))
                .collect(),
            frame: Frame::default(),
            frame_at: None,
            tick: -1,
            time_seen: false,
            mode: None,
            pos: Positions::from_trace(trace),
            running: HashMap::new(),
            unverified: Vec::new(),
            sim_skip: Vec::new(),
            turn_records: recs.iter().any(|r| {
                r["k"] == "tb"
                    && matches!(
                        r["ev"].as_str(),
                        Some("turn_start_begin" | "turn_clear" | "loot_timer")
                    )
            }),
            turn_begin: None,
            awake_sets: awake_sets(trace),
            destroy_marks: w.player.destroy_marks.len(),
            port,
            turn_go_on: true,
            draining: None,
            started_this_frame: false,
            choose_by_tick,
            // Any of WrathOracle's cooldown records (a trace that lost its prefixes reports them).
            cd: port
                && recs.iter().any(|r| {
                    r["k"] == "obs" && r["ev"].as_str().is_some_and(|e| e.starts_with("cd_"))
                }),
            cd_rows: HashMap::new(),
            cd_checked: None,
            turn_prepared: false,
            turn_end_applied: false,
            tt: false,
            tt_tick: None,
            tt_end_due: false,
            tt_open: false,
            tt_sc: None,
            i: 0,
            uc: false,
            uc_rows: commands::Rows::default(),
            uc_skipping: false,
            uc_dirty: false,
            uc_active: 0,
            uc_spend_ok: false,
            uc_ctl_tick: None,
            uc_origin: String::new(),
            uc_approach: false,
            uc_approach_sites: Vec::new(),
            ai: false,
            ai_sites: Vec::new(),
            eng: false,
            eng_tick_frame: None,
            eng_prov_seen: Vec::new(),
            eng_init_closed: false,
            tgt: false,
            tgt_init_closed: false,
            tgt_init_units: Vec::new(),
            tgt_init_n: 0,
            tgt_clear: targets::TgtClear::default(),
            tgt_leave_by: std::collections::HashMap::new(),
            tgt_click: None,
            tgt_runs: Vec::new(),
            imp: false,
            ag: false,
            ag_paths: HashMap::new(),
            ag_all: Vec::new(),
            ag_last_state: HashMap::new(),
            ag_tick_rec: None,
            ag_req_rec: None,
            ag_slow_sites: Vec::new(),
            ag_erosion: Ok(None),
            ag_after_rt: std::collections::HashSet::new(),
            ag_last_ctl: HashMap::new(),
            ag_has_findclr: false,
            imp_init_n: 0,
            imp_init_closed: false,
            imp_tick_frame: None,
            imp_prepared: Vec::new(),
            imp_prepare_tail: None,
            imp_prepare_ready: None,
            imp_sites: Vec::new(),
            ai_tick_frame: None,
            ai_forced: 0,
            ai_prepared: 0,
            ai_force_origin: String::new(),
            ai_time: None,
            achievements: std::cell::RefCell::new(HashMap::new()),
            rule_ids: recs
                .iter()
                .filter(|r| r["k"] == "rule")
                .filter_map(|r| r["id"].as_i64())
                .collect(),
            replay_mode: recs
                .iter()
                .find(|r| r["k"] == "begin")
                .is_some_and(|b| b["mode"] == "replay"),
            limb_ids: HashMap::new(),
            out,
        };
        let mut d = d;
        // Any of WrathOracle's tick records (a trace that lost some reports them).
        let tt_family = recs.iter().any(|r| {
            r["k"] == "obs"
                && matches!(
                    r["ev"].as_str(),
                    Some("tt" | "tt_sc" | "tt_able" | "tt_prep" | "tt_ca" | "tt_cw" | "tt_end")
                )
        });
        d.tt = d.cd && tt_family;
        // Any of WrathOracle's command records, with the turn controller's tick computed.
        let uc_family = recs
            .iter()
            .any(|r| r["k"] == "obs" && r["ev"].as_str().is_some_and(|e| e.starts_with("uc_")));
        d.uc = d.tt && uc_family;
        if uc_family && !d.tt {
            w.coverage.unsupported(
                "driven replay: the command records without the turn controller's tick records",
            );
        }
        if tt_family && !d.cd {
            w.coverage.unsupported(
                "driven replay: the turn controller's tick records without the cooldown records",
            );
        }
        // A recording that declares its observers (WrathOracle's `begin` record) must have each
        // one's records: an older one, without the list, replays with what it has.
        let declared: Vec<&str> =
            match recs.iter().find(|r| r["k"] == "begin").and_then(|b| b.get("observers")) {
                None => Vec::new(),
                Some(J::Array(a)) if a.iter().all(J::is_string) => {
                    a.iter().filter_map(J::as_str).collect()
                }
                Some(_) => {
                    w.coverage.unsupported(
                        "driven replay: a begin record whose observers list it can't read",
                    );
                    Vec::new()
                }
            };
        // The trace's mode: a record's or a replay's (`Trace::load` requires the begin record).
        if recs
            .iter()
            .find(|r| r["k"] == "begin")
            .is_some_and(|b| !matches!(b["mode"].as_str(), Some("record" | "replay")))
        {
            w.coverage
                .unsupported("driven replay: a trace whose begin record has no mode it can read");
        }
        // Every record carries its trace sequence number, in order (`Trace.Emit`).
        // (`Trace.Emit` numbers every record, from 0: a gap is a record lost or removed.)
        let mut seq = -1;
        for r in recs {
            match r["seq"].as_i64() {
                Some(s) if s == seq + 1 => seq = s,
                _ => {
                    w.coverage.unsupported(
                        "driven replay: a record without its sequence number, or out of order (or one missing)",
                    );
                    break;
                }
            }
        }
        // WrathOracle allocates rule and handler ids in order through the window, from 1 (`Rules.Reset`,
        // then `++`): a record that breaks the sequence (a reused, skipped or damaged id) is reported.
        for k in ["rule", "h"] {
            let mut last = 0;
            for r in recs.iter().filter(|r| r["k"] == k) {
                // (A C# `int`.)
                match r["id"].as_i64().filter(|i| i32::try_from(*i).is_ok()) {
                    Some(id) if id == last + 1 => last = id,
                    _ => {
                        w.coverage.unsupported(format!(
                            "driven replay: a {k} record whose id breaks the allocation order"
                        ));
                        break;
                    }
                }
            }
        }
        // `UnitAttack`'s class code is the sim's when the recording has the `attacks` observer.
        w.combat.commands.attacks = d.uc && declared.contains(&"attacks");
        if declared.contains(&"attacks") && !d.uc {
            w.coverage.unsupported(
                "driven replay: the recording declares its attacks observer without the command records",
            );
        }
        // `TickApproaching`, `UnitMoveTo`'s and the attack of opportunity's code are the sim's with
        // the `approach` observer (which WrathOracle installs with `attacks`).
        w.combat.commands.approach = d.uc && declared.contains(&"approach");
        d.uc_approach = w.combat.commands.approach;
        d.uc_approach_sites = trace
            .sites
            .iter()
            .filter(|(_, v)| v.0 == crate::rng::sites::TICK_APPROACHING.0)
            .map(|(k, _)| *k)
            .collect();
        // The AI's code is the sim's with the `ai` observer (WrathOracle installs it with the others).
        w.combat.ai.on = w.combat.commands.approach && declared.contains(&"ai");
        d.ai = w.combat.ai.on;
        d.ai_sites = trace
            .sites
            .iter()
            .filter(|(_, v)| v.0 == crate::rng::sites::AI_NEXT_COMMAND_TIME.0)
            .map(|(k, _)| *k)
            .collect();
        // The engagement controller's code is the sim's with the `engage` observer.
        w.combat.engage.on = w.combat.ai.on && declared.contains(&"engage");
        d.eng = w.combat.engage.on;
        if declared.contains(&"engage") && !w.combat.engage.on {
            w.coverage.unsupported(
                "driven replay: the recording declares its engage observer without the ai, command, attack and approach records",
            );
        }
        // Units' targets are the sim's with the `targets` observer.
        w.combat.targets.on = w.combat.engage.on && declared.contains(&"targets");
        d.tgt = w.combat.targets.on;
        // The impatience controller is the sim's with the `impatience` observer.
        w.combat.impatience.on = w.combat.targets.on && declared.contains(&"impatience");
        d.imp = w.combat.impatience.on;
        // With the `memory` observer, an offensive action's handler records the memory
        // controller's `AddToMemory` calls (`mem_q`): matched with the sim's.
        w.combat.impatience.memory_adds =
            (w.combat.impatience.on && declared.contains(&"memory")).then(Default::default);
        // The movement agent is the sim's with the `agent` observer's initial states (`ag_init`,
        // since poc5-tb-106); an older capture's agent records are taken as before.
        d.ag = d.imp && declared.contains(&"agent") && recs.iter().any(|r| r["ev"] == "ag_init");
        d.ag_has_findclr = d.ag && recs.iter().any(|r| r["ev"] == "ag_findclr");
        if d.ag {
            d.ag_slow_sites = trace
                .sites
                .iter()
                .filter(|(_, v)| v.0 == crate::rng::sites::AGENT_SLOW_DOWN.0)
                .map(|(k, _)| *k)
                .collect();
            d.ag_erosion = match w.player.area {
                None => Err("the save names no current area (its nav file)".to_owned()),
                Some(g) => match wr_nav::NavFile::read(&wr_nav::path_for(&g.to_string())) {
                    Ok(f) => {
                        Ok(f.world.grid.as_ref().map(|g| (g.erode_iterations, g.erosion_first_tag)))
                    }
                    Err(e) => Err(format!("the area's nav file: {e:#}")),
                },
            };
        }
        d.imp_sites = trace
            .sites
            .iter()
            .filter(|(_, v)| {
                v.0 == crate::combat::impatience::SWITCH_DRAW.0
                    || v.0 == crate::combat::impatience::SETUP_DRAW.0
            })
            .map(|(k, _)| *k)
            .collect();
        if declared.contains(&"impatience") && !w.combat.impatience.on {
            w.coverage.unsupported(
                "driven replay: the recording declares its impatience observer without the targets records",
            );
        }
        if declared.contains(&"targets")
            && (!w.combat.targets.on || !declared.contains(&"cooldowns"))
        {
            w.coverage.unsupported(
                "driven replay: the recording declares its targets observer without the engage and cooldown records",
            );
        }
        if declared.contains(&"ai") && (!w.combat.ai.on || !declared.contains(&"groups")) {
            w.coverage.unsupported(
                "driven replay: the recording declares its ai observer without the group, command, attack and approach records",
            );
        }
        if declared.contains(&"approach")
            && (!d.uc || !declared.contains(&"commands") || !declared.contains(&"attacks"))
        {
            w.coverage.unsupported(
                "driven replay: the recording declares its approach observer without the command and attack records",
            );
        }
        for (name, present) in
            [("groups", d.port), ("cooldowns", d.cd), ("turn_ticks", d.tt), ("commands", d.uc)]
        {
            if declared.contains(&name) && !present {
                w.coverage.unsupported(format!(
                    "driven replay: the recording declares its {name} observer, but its records are missing"
                ));
            }
        }
        if !d.turn_records && recs.iter().any(|r| r["k"] == "tb" && r["ev"] == "turn_start") {
            w.coverage.unsupported(
                "driven replay: a recording without WrathOracle's turn-controller records (turn_start_begin, turn_clear, loot_timer)",
            );
        }
        w.combat.timer_frames_recorded = d.turn_records;
        w.combat.cooldown_clears = d.cd.then(std::collections::VecDeque::new);
        d
    }

    /// The last frame's remaining phases, then the comparison.
    fn finish(mut self, w: &mut World) -> Outcome {
        self.cd_frame_end(w);
        self.uc_frame_end(w);
        self.tgt_frame_end(w);
        self.imp_frame_end(w);
        self.uc_finish(w);
        let outgoing = self.frame.mode;
        let rest = self.frame.finish();
        self.draining = outgoing;
        self.run_phases(w, &rest);
        if self.tt {
            self.tt_close(w);
        }
        if self.eng {
            self.eng_finish(w);
        }
        self.frame_leftovers(w);
        // Draws left pending after the last record.
        let pending = std::mem::take(&mut self.cx.sink.borrow_mut().pending);
        for p in pending {
            w.events.push("draw", p);
        }
        fire::set_current(w, None);
        let trace = self.trace;
        let mut out = self.out;
        out.oracle = oracle_stream(trace, &self.echo_spans, &self.unverified, &mut out);
        out.sim = sim_stream_without(w, &self.sim_skip);
        out.diff = project::diff(&out.oracle, &out.sim);
        out
    }
}

impl Driver<'_> {
    /// At a frame's end (the turn port): the sim's turn events and `LeaveCombat` calls the
    /// recording didn't have, and recorded `ChooseNextUnit` calls the sim didn't make.
    fn frame_leftovers(&mut self, w: &mut World) {
        if !self.port {
            return;
        }
        super::initiative::prepare_flush(w);
        if !w.combat.turns.log.is_empty() {
            w.combat.turns.log.clear();
            w.coverage.unsupported("driven replay: a turn event the recording doesn't have");
        }
        if !w.combat.leave_log.is_empty() {
            w.combat.leave_log.clear();
            w.coverage.unsupported("driven replay: a LeaveCombat the recording doesn't have");
        }
        if !w.combat.turns.choose.is_empty() {
            w.combat.turns.choose.clear();
            w.coverage.unsupported("driven replay: a ChooseNextUnit call the sim didn't make");
        }
    }

    /// `group_state` (the window's first prefix): the groups the sim made after its load
    /// (`UnitGroupsController.Start`, then each frame's tick) against the game's: the same groups
    /// (their list order taken from the recording), members, combat counts and timers.
    fn group_state(&mut self, w: &mut World, r: &J) {
        let Some(groups) = r["groups"].as_array() else { return };
        let ours: Vec<String> = w
            .combat
            .groups
            .list
            .iter()
            .map(|g| w.combat.groups.get(*g).id.clone().unwrap_or_default())
            .collect();
        let theirs: Vec<String> =
            groups.iter().map(|g| g["id"].as_str().unwrap_or("").to_owned()).collect();
        // The list's order is the order the groups were made in: the area load's first `Group`
        // accesses (its units before the party's, which `UnitGroupsController.Start` reaches
        // later) aren't modelled, so the recorded order is taken when the groups are the same.
        let (mut a, mut b) = (ours.clone(), theirs.clone());
        a.sort();
        b.sort();
        if a == b {
            let list = theirs.iter().filter_map(|id| w.combat.groups.find(id)).collect();
            w.combat.groups.list = list;
        } else {
            w.coverage.unsupported("driven replay: the unit groups differ from the recording's");
        }
        for g in groups {
            let Some(gi) = g["id"].as_str().and_then(|id| w.combat.groups.find(id)) else {
                continue;
            };
            let units: Vec<String> = w
                .combat
                .groups
                .get(gi)
                .units
                .iter()
                .map(|u| w.entity(*u).unique_id.clone())
                .collect();
            let rec: Vec<String> = g["units"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            let grp = w.combat.groups.get(gi);
            if units != rec
                || i64::from(grp.in_combat) != g["in_combat"].as_i64().unwrap_or(-1)
                || g["timer"]
                    .as_f64()
                    .is_none_or(|t| (t as f32).to_bits() != grp.leave_timer.to_bits())
            {
                w.coverage.unsupported(
                    "driven replay: a unit group's members or state differ from the recording's",
                );
            }
        }
    }

    /// `leave_tick` (`UnitCombatLeaveController.Tick`'s prefix, every call): the gate is checked;
    /// each changed group's combat count, timer and faction caches are checked and its memory
    /// taken.
    fn leave_tick(&mut self, w: &mut World, r: &J) {
        // `cmds` rows are changes from empty: from the first `leave_tick` on, a unit without a
        // row has no commands.
        w.combat.groups.commands.get_or_insert_with(HashMap::new);
        if r["gate"].as_str() != Some(gate_row(w).as_str()) {
            w.coverage.unsupported(
                "driven replay: the leave controller's gate differs from the recording's",
            );
        }
        for row in r["groups"].as_array().into_iter().flatten() {
            let (Some(id), Some(Some(g))) = (row[0].as_str(), row[1].as_str().map(GroupRow::parse))
            else {
                w.coverage.unsupported("driven replay: a leave_tick group row it can't read");
                continue;
            };
            let Some(gi) = w.combat.groups.find(id) else {
                w.coverage.unsupported("driven replay: a recorded unit group the sim doesn't have");
                continue;
            };
            let grp = w.combat.groups.get(gi);
            if grp.in_combat != g.in_combat || grp.leave_timer.to_bits() != g.timer.to_bits() {
                w.coverage.unsupported(
                    "driven replay: a unit group's combat count or leave timer differs from the recording's",
                );
            }
            if !same_factions(&faction_row(w, gi), &g.factions) {
                w.coverage.unsupported(
                    "driven replay: a unit group's faction caches differ from its members' (a deferred update)",
                );
            }
            w.combat.groups.get_mut(gi).memory = Some(g.memory);
        }
    }

    /// `leave_combat` (`UnitEntityData.LeaveCombat`'s prefix): placed by its caller, and the sim's
    /// next `LeaveCombat` must be the same unit's.
    fn leave_combat(&mut self, w: &mut World, r: &J) {
        let by = r["by"].as_str().unwrap_or("");
        if by.contains("UnitCombatLeaveController") {
            self.place(w, Phase::Leave, "a LeaveCombat");
        } else if by.contains("UnitLifeController") {
            self.place(w, Phase::Life, "a LeaveCombat");
        } else {
            w.coverage.unsupported(format!(
                "driven replay: a LeaveCombat from a source the sim doesn't run ({by})"
            ));
            return;
        }
        let u = r["u"].as_str().and_then(|id| w.by_id.get(id).copied());
        if let Some(u) = u {
            self.tgt_leave(u, by);
        }
        match w.combat.leave_log.pop_front() {
            Some(x) if Some(x) == u => {}
            Some(_) => w.coverage.unsupported(
                "driven replay: the sim's LeaveCombat is another unit's than the recording's",
            ),
            None => w.coverage.unsupported("driven replay: a LeaveCombat the sim didn't make"),
        }
    }

    /// `ui_guard` (`WaitingForUI` changed): the count after it. The turn and leave controllers'
    /// prefixes are recorded every frame (`turn_tick`, `leave_tick`), so the trace orders the
    /// change: after a prefix the frame has reached, it comes after that controller's tick.
    fn ui_guard(&mut self, w: &mut World, r: &J) {
        let Some(count) = r["d"]
            .as_str()
            .and_then(|d| d.split_once("count="))
            .and_then(|(_, n)| n.split(' ').next())
            .and_then(|n| n.parse::<i32>().ok())
        else {
            w.coverage.unsupported("driven replay: a ui_guard record it can't read");
            return;
        };
        let at = self.frame.at;
        if at == Phase::LeavePrefix {
            self.place(w, Phase::Leave, "a UI guard change");
        } else if (Phase::TurnPrefix..Phase::TurnChoose).contains(&at) {
            self.place(w, Phase::TurnChoose, "a UI guard change");
        }
        w.combat.turns.waiting_for_ui = count;
    }

    /// `HandleCombatStart` with the turn port: `m_Units` are the sim's units in combat, with the
    /// recorded surprise flags (they come from commands).
    fn port_combat_start(&mut self, w: &mut World, r: &J, units: Vec<EntityIdx>) {
        use super::order::TbUnitInfo;
        let rec = r["units"].as_array().cloned().unwrap_or_default();
        let ids: Vec<Option<EntityIdx>> = rec
            .iter()
            .map(|e| e["unit"].as_str().and_then(|id| w.by_id.get(id).copied()))
            .collect();
        if ids != units.iter().map(|u| Some(*u)).collect::<Vec<_>>() {
            w.coverage.unsupported(
                "driven replay: the turn controller's units at combat start differ from the recording's",
            );
        }
        let infos = units
            .iter()
            .map(|&u| {
                let mut info = TbUnitInfo::new(u);
                if let Some(e) =
                    rec.iter().find(|e| e["unit"].as_str() == Some(&w.entity(u).unique_id))
                {
                    info.surprising = e["surprising"].as_bool().unwrap_or(false);
                    info.surprised = e["surprised"].as_bool().unwrap_or(false);
                    info.acting_in_surprise_round =
                        e["acting_in_surprise_round"].as_bool().unwrap_or(false);
                }
                info
            })
            .collect();
        let surprise = r["surprise_round"].as_bool().unwrap_or(false);
        if r["party_combat_state_changed"].as_bool() != Some(true) {
            // Turn-based mode switched on mid-fight: `m_Units` ordered by `GetTimeToNextTurn()`
            // with descending `InitiativeOverride`s (cooldowns aren't established here).
            w.coverage.unsupported(
                "CombatController.HandleCombatStart: turn-based mode entered mid-fight (the order by time to next turn)",
            );
        }
        super::turns::combat_start(w, infos, surprise);
    }

    /// A recorded `sort`, `round_start` or `turn_clear` with the turn port: the sim's next turn
    /// event. With none made yet, the frame runs on to the controllers that make them (the turn
    /// controller's choice, the life controller's and the leave controller's removals).
    fn turn_event(&mut self, w: &mut World, i: usize) {
        use super::turns::TbEvent;
        let r = &self.trace.records[i];
        if w.combat.turns.log.is_empty() {
            super::initiative::prepare_flush(w);
        }
        for p in [Phase::TurnChoose, Phase::Life, Phase::Leave] {
            if !w.combat.turns.log.is_empty() {
                break;
            }
            let m = self.frame.mode.unwrap_or(-1);
            if self.frame.at < p && registered(m, p) == Some(true) {
                self.place(w, p, "a turn event");
            }
        }
        let Some(ev) = w.combat.turns.log.pop_front() else {
            w.coverage.unsupported("driven replay: a turn event the sim didn't make");
            return;
        };
        let id = |u: EntityIdx| w.entity(u).unique_id.clone();
        let equal = match (&ev, r["ev"].as_str()) {
            (TbEvent::Clear(u), Some("turn_clear")) => unit_id(&r["unit"]) == Some(id(*u).as_str()),
            (TbEvent::RoundStart(n), Some("round_start")) => {
                r["round"].as_i64() == Some(i64::from(*n))
            }
            (TbEvent::Sort { reason, units }, Some("sort")) => {
                self.out.sorts.checked += 1;
                let rec_reason = r["reason"].as_str().unwrap_or("");
                let reason_ok = if reason == super::turns::ROLLS_INITIATIVE {
                    rec_reason.starts_with(super::turns::ROLLS_INITIATIVE)
                } else {
                    rec_reason == reason
                };
                let rec: Vec<SortEntry<'_>> = r["units"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|e| {
                                (
                                    e["unit"].as_str(),
                                    e["initiative"].as_i64(),
                                    e["override"].as_i64(),
                                    e["sequence"].as_i64(),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let ours: Vec<(String, i64, Option<i64>, i64)> = units
                    .iter()
                    .map(|(u, init, ov, seq)| {
                        (id(*u), i64::from(*init), ov.map(i64::from), i64::from(*seq))
                    })
                    .collect();
                let same = rec.len() == ours.len()
                    && rec.iter().zip(&ours).all(|(a, b)| {
                        a.0 == Some(b.0.as_str())
                            && a.1 == Some(b.1)
                            && a.2 == b.2
                            && a.3 == Some(b.3)
                    });
                if same {
                    self.out.sorts.equal += 1;
                }
                reason_ok && same
            }
            _ => false,
        };
        if !equal {
            w.coverage.unsupported(
                "driven replay: the sim's turn event differs from the recording's (sort, round start or turn clear)",
            );
        }
    }

    fn run_phases(&mut self, w: &mut World, phases: &[Phase]) {
        for p in phases {
            match p {
                Phase::Groups => super::groups::tick(w),
                Phase::Turn if self.port => {
                    self.turn_go_on = super::turns::tick_units_changed(w);
                }
                Phase::TurnChoose if self.port && self.turn_go_on => {
                    // A frame's remaining phases run when the next one starts: its own mode.
                    let m = self.draining.or(self.frame.mode);
                    self.tt_close(w);
                    if self.tt
                        && self.turn_ticks(w, m == Some(mode::DEFAULT))
                        && self.tt_tick != Some(self.tick)
                    {
                        w.coverage.unsupported(
                            "driven replay: the turn controller ticks in this frame, but its tick (tt) isn't recorded",
                        );
                    }
                    super::turns::tick_choose(w, m == Some(mode::DEFAULT));
                }
                Phase::Leave if self.port => {
                    let gate = super::groups::Gate {
                        has_enemy: w.combat.turns.has_enemy,
                        had_enemy: w.combat.turns.had_enemy,
                        passing: super::turns::is_passing(w),
                    };
                    super::groups::leave_tick(w, gate);
                }
                Phase::Sleeping => {
                    // The prefix's rows were written for the previous snapshot's units.
                    self.pos.note_prefix(w, self.tick);
                    super::life::update_awake(w);
                    // This frame's snapshot is what the next frame's prefix observes.
                    if let Some(set) = self.awake_sets.get(&(self.tick + 1)) {
                        self.out.awake_recorded += apply_recorded_awake(w, set);
                    }
                }
                // The prepare controller's batch ends before the cooldowns controller ticks.
                Phase::Cooldowns => {
                    super::initiative::prepare_flush(w);
                    if self.cd {
                        super::cooldowns::controller_tick(w);
                    }
                }
                Phase::Buffs => super::life::buffs_tick(w),
                Phase::Life => super::life::life_tick(w),
                Phase::Helpless => super::life::helpless_tick(w),
                Phase::Inspect => super::inspect::tick(w),
                // A recording with `loot_timer` records fires each timer at its record.
                Phase::Post => {
                    // `EntityDestructionController` (late in the frame) destroys marked entities
                    // and removes destroyed units from `AwakeUnits`, so the next prefix's set
                    // isn't this frame's snapshot for them.
                    if w.player.destroy_marks.len() > self.destroy_marks {
                        self.destroy_marks = w.player.destroy_marks.len();
                        w.coverage.unsupported(
                            "driven replay: an entity marked for destroy (EntityDestructionController, AwakeUnits) is not modelled",
                        );
                    }
                    if !self.turn_records {
                        super::life::fire_timers(w);
                    }
                }
                _ => {}
            }
        }
    }

    /// Places an event of phase `p` in its frame, running the phases before it; reports it
    /// when it can't be placed (the event is still processed where it was recorded).
    fn place(&mut self, w: &mut World, p: Phase, what: &str) {
        if p > Phase::Prepare {
            super::initiative::prepare_flush(w);
        }
        match self.frame.to(p) {
            Ok(run) => self.run_phases(w, &run),
            Err(e) => w.coverage.unsupported(format!(
                "driven replay: {what} can't be placed in its frame's {p:?} phase ({e:?})"
            )),
        }
    }

    /// The frame's recorded `cooldowns` rows (the prepare controller's prefix, `cd_prefix`: the last
    /// row of each unit in combat) against the sim's, once per frame; a difference is reported and
    /// the recorded row taken.
    fn cd_check(&mut self, w: &mut World) {
        if self.cd_checked == Some(self.tick) {
            w.coverage.unsupported("driven replay: two prepare controller prefixes in a frame");
            return;
        }
        self.cd_checked = Some(self.tick);
        let units: Vec<EntityIdx> =
            w.by_id.values().copied().filter(|u| super::is_in_combat(w, *u)).collect();
        for u in units {
            let Some(want) = self.cd_rows.get(&w.entity(u).unique_id).copied() else {
                w.coverage.unsupported(
                    "driven replay: a unit in combat without a recorded cooldowns row at the prepare controller's prefix",
                );
                continue;
            };
            self.out.cooldowns_checked += 1;
            if let Some(x) = w.unit_mut(u)
                && x.combat.cooldown != want
            {
                if std::env::var_os("WR_DEBUG_COOLDOWNS").is_some() {
                    eprintln!(
                        "DEBUG cd tick {} {:?} sim {:?} rec {:?}",
                        self.tick, u, x.combat.cooldown, want
                    );
                }
                x.combat.cooldown = want;
                self.out.cooldowns_recorded += 1;
                w.coverage.unsupported(
                    "driven replay: a unit's cooldowns differ from the recording's (UnitCombatPrepareController's prefix)",
                );
            }
        }
    }

    /// The end of a frame: one that runs the prepare controller must have had its prefix
    /// (`cd_prefix`), and the sim's combat-state clears must all have been recorded.
    fn cd_frame_end(&mut self, w: &mut World) {
        if !self.cd {
            return;
        }
        if self.frame.mode == Some(mode::DEFAULT) && self.cd_checked != Some(self.tick) {
            w.coverage.unsupported(
                "driven replay: a frame without the prepare controller's prefix (cd_prefix)",
            );
        }
        if w.combat.cooldown_clears.as_ref().is_some_and(|q| !q.is_empty()) {
            w.combat.cooldown_clears = Some(std::collections::VecDeque::new());
            w.coverage
                .unsupported("driven replay: a UnitCombatState.Clear the recording doesn't have");
        }
    }

    /// The end of a frame: one whose mode runs the command controller must have had its tick
    /// (`uc_ctl`).
    fn uc_frame_end(&mut self, w: &mut World) {
        let Some(m) = self.frame.mode else { return };
        if self.uc
            && registered(m, Phase::Commands) == Some(true)
            && self.uc_ctl_tick != Some(self.tick)
        {
            w.coverage.unsupported(
                "driven replay: a frame without the command controller's tick (uc_ctl)",
            );
        }
        if std::mem::take(&mut self.ai_prepared) != 0 {
            w.coverage.unsupported(
                "driven replay: TurnController.Prepare without exactly one AI entry (ForceTick)",
            );
        }
        if self.ai
            && registered(m, Phase::Ai) == Some(true)
            && self.ai_tick_frame != Some(self.tick)
        {
            w.coverage
                .unsupported("driven replay: a frame without the AI controller's tick (uc_ai)");
        }
        if self.eng
            && registered(m, Phase::Engagement) == Some(true)
            && self.eng_tick_frame != Some(self.tick)
        {
            w.coverage.unsupported(
                "driven replay: a frame without the engagement controller's tick (uc_eng)",
            );
        }
    }

    /// `CombatController.TickTime`'s decision (the `time` record, before the sim's `tick_time`):
    /// without a recorded `StartTurn` this frame, the next unit's time to its turn hasn't run out
    /// (or it was chosen this tick), and the frame's game delta is clamped to that time.
    fn check_tick_time(&mut self, w: &mut World, r: &J) {
        let t = &w.combat.turns;
        let Some(next) = t.next else { return };
        if t.waiting_for_ui > 0 || self.started_this_frame {
            return;
        }
        let ttnt = super::cooldowns::time_to_next_turn(w, next);
        if ttnt <= 0.0 && !t.chosen_this_tick {
            w.coverage.unsupported(
                "driven replay: the sim's cooldowns start a turn the recording doesn't (GetTimeToNextTurn <= 0)",
            );
        }
        #[allow(clippy::cast_possible_truncation)]
        let delta = r["game_delta"].as_f64().map(|d| d as f32);
        if delta.is_none_or(|d| d > ttnt.max(0.0)) {
            w.coverage.unsupported(
                "driven replay: the frame's game delta isn't clamped to the next unit's time to its turn",
            );
        }
    }

    /// `TurnController.End`'s writes for the turn's rider, once per turn (a command it interrupts
    /// ends after them, with the turn already `Ended`).
    fn apply_end(&mut self, w: &mut World, u: Option<EntityIdx>) {
        if self.turn_end_applied {
            return;
        }
        self.turn_end_applied = true;
        match u {
            Some(u) if Some(u) == w.combat.current_unit => super::cooldowns::end(w, u),
            _ => w
                .coverage
                .unsupported("driven replay: a turn end of another unit than the current one"),
        }
    }

    /// A unit's cooldowns as a `cd_*` record has them before (`b`) the call: a difference from
    /// the sim's is reported and the recorded row taken.
    fn cd_before(&mut self, w: &mut World, u: EntityIdx, b: &J, what: &str) {
        let Some(want) = b.as_str().and_then(parse_cooldowns) else {
            w.coverage
                .unsupported(format!("driven replay: {what}: its unit's cooldowns are unknown"));
            return;
        };
        if let Some(x) = w.unit_mut(u)
            && x.combat.cooldown != want
        {
            x.combat.cooldown = want;
            self.out.cooldowns_recorded += 1;
            w.coverage.unsupported(format!(
                "driven replay: a unit's cooldowns differ from the recording's before {what}"
            ));
        }
    }

    /// The sim's cooldowns after a `cd_*` record's call against the recorded ones (`a`): a
    /// difference is reported and the recorded row taken.
    fn cd_after(&mut self, w: &mut World, u: EntityIdx, a: &J, what: &str) {
        let Some(want) = a.as_str().and_then(parse_cooldowns) else {
            w.coverage.unsupported(format!(
                "driven replay: {what}: its unit's cooldowns after it are unknown"
            ));
            return;
        };
        if let Some(x) = w.unit_mut(u)
            && x.combat.cooldown != want
        {
            x.combat.cooldown = want;
            self.out.cooldowns_recorded += 1;
            w.coverage.unsupported(format!(
                "driven replay: the sim's cooldowns after {what} differ from the recording's"
            ));
        }
    }

    /// The turn's movement stats around a `cd_*` record's call (`sb`, `sa`), likewise.
    fn stats_check(&mut self, w: &mut World, s: &J, what: &str) {
        let Some(want) = s.as_str().and_then(parse_stats) else {
            w.coverage.unsupported(format!(
                "driven replay: {what}: the turn's movement stats are unknown"
            ));
            return;
        };
        if w.combat.turns.movement != want {
            w.combat.turns.movement = want;
            w.coverage.unsupported(format!(
                "driven replay: the turn's movement stats differ from the recording's at {what}"
            ));
        }
    }

    /// The tick's status change, once its `IsAbleToAct` calls (`tt_able`) are in: the sim's must
    /// ask exactly the ones recorded.
    fn tt_resolve(&mut self, w: &mut World) {
        use super::turn_tick::{self as tt, Stage, Status};
        let Some((input, all_acted)) = self.tt_sc.take() else { return };
        let t = &mut w.combat.turns.ctl;
        let asks = t.status == Status::Preparing && !all_acted && input.commands_empty;
        match tt::status_change(t, all_acted, input) {
            Ok(ca) => t.stage = if ca { Stage::ContinueActing } else { Stage::StatusDone },
            Err(what) => {
                // The status change can't go on: the tick's later records are reported.
                t.stage = Stage::StatusDone;
                w.coverage.unsupported(format!(
                    "driven replay: the tick's status change: {what} isn't recorded"
                ));
            }
        }
        if input.able.is_some() && !asks {
            w.coverage
                .unsupported("driven replay: an IsAbleToAct the sim's status change doesn't ask");
        }
    }

    /// The frame's turn controller tick must be complete by the end of `CombatController.Tick`
    /// (and of the frame, and of the recording): its `tt_end` and the `turn_end` of a turn it ended.
    fn tt_close(&mut self, w: &mut World) {
        if w.combat.turns.ctl.stage == super::turn_tick::Stage::StatusPending {
            self.tt_resolve(w);
        }
        if std::mem::take(&mut self.tt_open) {
            w.coverage
                .unsupported("driven replay: a turn controller tick without its end (tt_end)");
            w.combat.turns.ctl.stage = super::turn_tick::Stage::Out;
        }
        if std::mem::take(&mut self.tt_end_due) {
            w.coverage.unsupported(
                "driven replay: the sim's tick ended the turn, but the recording has no turn_end",
            );
        }
    }

    /// `CombatController.Tick` ticks the current turn (`CurrentTurn?.Tick()`): past its
    /// units-changed check, in the default mode, in combat, not waiting for the UI.
    fn turn_ticks(&self, w: &World, default_mode: bool) -> bool {
        self.turn_go_on
            && default_mode
            && w.combat.combat_active
            && w.combat.turns.waiting_for_ui == 0
            && w.combat.current_unit.is_some()
    }

    /// The turn controller's status and counters against a record's (`st`, `w`, `wfgu`): a
    /// difference is reported and the recorded ones taken.
    fn tt_check(&mut self, w: &mut World, r: &J, what: &str) {
        use super::turn_tick::{Counters, Status};
        let (Some(st), Some(c), Some(wfgu)) = (
            r["st"].as_str().and_then(Status::parse),
            r["w"].as_str().and_then(Counters::parse),
            r["wfgu"].as_bool(),
        ) else {
            w.coverage.unsupported(format!("driven replay: {what}: a record it can't read"));
            return;
        };
        let t = &mut w.combat.turns.ctl;
        if (t.status, t.counters, t.wait_for_get_up) != (st, c, wfgu) {
            if std::env::var_os("WR_DEBUG_TURNS").is_some() {
                eprintln!(
                    "DEBUG tt tick {} {what}: sim {:?} {:?} {} rec {st:?} {c:?} {wfgu}",
                    self.tick, t.status, t.counters, t.wait_for_get_up
                );
            }
            (t.status, t.counters, t.wait_for_get_up) = (st, c, wfgu);
            w.coverage.unsupported(format!(
                "driven replay: the turn controller's status or counters differ from the recording's at {what}"
            ));
        }
    }

    /// The counters a `tt_ca`/`tt_cw` record has before (`wb`) or after (`wa`) the call.
    fn tt_counters(&mut self, w: &mut World, v: &J, what: &str) {
        let Some(c) = v.as_str().and_then(super::turn_tick::Counters::parse) else {
            w.coverage.unsupported(format!("driven replay: {what}: counters it can't read"));
            return;
        };
        let t = &mut w.combat.turns.ctl;
        if t.counters != c {
            if std::env::var_os("WR_DEBUG_TURNS").is_some() {
                eprintln!("DEBUG tt tick {} {what}: sim {:?} rec {c:?}", self.tick, t.counters);
            }
            t.counters = c;
            w.coverage.unsupported(format!(
                "driven replay: the turn controller's counters differ from the recording's at {what}"
            ));
        }
    }

    /// A value of the units' commands a turn controller record holds (`Commands.Empty`,
    /// `IsRunning()`), against the sim's commands when they're modelled: a difference is reported
    /// and the recorded value taken.
    fn uc_value(&mut self, w: &mut World, rec: bool, sim: Option<bool>, what: &str) -> bool {
        if !self.uc || !self.uc_rows.ready {
            return rec;
        }
        self.out.commands_values += 1;
        if sim != Some(rec) {
            if std::env::var_os("WR_DEBUG_COMMANDS").is_some() {
                eprintln!("DEBUG commands tick {} {what}: sim {sim:?} rec {rec}", self.tick);
            }
            w.coverage.unsupported(format!(
                "driven replay: the turn controller's {what} differs from the sim's commands"
            ));
        }
        rec
    }

    /// A record's frame time (`tt`, `tt_ca`, `tt_cw`) against the sim's clock.
    fn tt_time(&mut self, w: &mut World, r: &J, what: &str) -> Option<super::turn_tick::Time> {
        let Some(t) = r["time"].as_str().and_then(super::turn_tick::Time::parse) else {
            w.coverage.unsupported(format!("driven replay: {what}: a time it can't read"));
            return None;
        };
        let c = &w.clock;
        if (t.game_delta, t.delta, t.player_scale, t.debug_scale)
            != (c.game_delta_time(), c.delta_time(), c.player_time_scale, c.debug_time_scale)
        {
            w.coverage.unsupported(format!(
                "driven replay: {what}: the frame's time differs from the sim's clock"
            ));
        }
        Some(t)
    }

    /// The end of the tick's status change (before its end wait): the rider no longer waits to get
    /// up. The status change's records must be complete.
    fn tt_status_done(&mut self, w: &mut World, what: &str) {
        use super::turn_tick::Stage;
        match w.combat.turns.ctl.stage {
            Stage::StatusDone => {}
            // (A pending status change is resolved before this.)
            Stage::Start | Stage::StatusPending => w.coverage.unsupported(format!(
                "driven replay: {what} without the tick's status change (tt_sc)"
            )),
            Stage::ContinueActing => w.coverage.unsupported(format!(
                "driven replay: {what}, but the status change's ContinueActing isn't recorded"
            )),
            Stage::Out | Stage::Done => {
                w.coverage.unsupported(format!("driven replay: {what} out of its tick's order"));
                return;
            }
        }
        w.combat.turns.ctl.wait_for_get_up = false;
        w.combat.turns.ctl.stage = Stage::Done;
    }

    /// WrathOracle's `tt*` records: `TurnController.Tick`, `Prepare`, `ContinueActing`,
    /// `ContinueWaiting`. The sim computes the status and counters; each record's reads it can't
    /// compute are its inputs, its status and counters checks.
    fn turn_tick(&mut self, w: &mut World, ev: &str, r: &J) {
        use super::turn_tick::{self as tt, Stage, Status};
        if ev == "tt_prep" && r["in_tick"].as_bool() == Some(false) {
            self.place(w, Phase::Time, "TurnController.Prepare (in StartTurn)");
        } else {
            self.place(w, Phase::Turn, "the turn controller's tick");
        }
        // Each record is of the frame it's read in.
        if r["t"].as_i64() != Some(self.tick) {
            w.coverage.unsupported(format!("driven replay: {ev} of another frame"));
            return;
        }
        let Some(u) = w.combat.current_unit else {
            w.coverage.unsupported(format!("driven replay: {ev} without a current turn"));
            return;
        };
        let rider = w.entity(u).unique_id.clone();
        let flag = |k: &str| r[k].as_bool();
        // A value the game reads only under a condition: present, `null` where it wasn't read.
        let opt = |k: &str| match r.get(k) {
            Some(J::Bool(b)) => Some(Some(*b)),
            Some(J::Null) => Some(None),
            _ => None,
        };
        if ev != "tt_able" && w.combat.turns.ctl.stage == Stage::StatusPending {
            self.tt_resolve(w);
        }
        match ev {
            "tt" => {
                if !self.turn_ticks(w, self.frame.mode == Some(mode::DEFAULT))
                    || self.tt_tick == Some(self.tick)
                {
                    w.coverage.unsupported(
                        "driven replay: a TurnController.Tick where CombatController.Tick doesn't tick the turn (or twice)",
                    );
                }
                self.tt_tick = Some(self.tick);
                // `ImmuneAttackOfOpportunityOnDisengage = false` as the tick starts.
                w.combat.turns.immune_on_disengage = Some(false);
                // `TimeController.Tick` runs first in the frame: the tick reads its clock.
                if !self.time_seen {
                    w.coverage.unsupported(
                        "driven replay: a turn controller tick without its frame's time record",
                    );
                }
                if w.combat.turns.ctl.stage != Stage::Out {
                    w.coverage
                        .unsupported("driven replay: TurnController.Tick inside its own tick");
                }
                if r["u"].as_str() != Some(rider.as_str()) {
                    w.coverage.unsupported(
                        "driven replay: TurnController.Tick of another unit than the current one",
                    );
                }
                if flag("mount") != Some(false) {
                    w.coverage.unsupported(
                        "driven replay: TurnController.Tick: a turn with a mount (or not recorded)",
                    );
                }
                self.tt_check(w, r, "TurnController.Tick's start");
                let time = self.tt_time(w, r, "TurnController.Tick");
                let (Some(ctrl), Some(empty), Some(_), Some(_), Some(surprise)) =
                    (opt("ctrl"), flag("r_empty"), time, flag("ucgu"), flag("surprise"))
                else {
                    w.coverage.unsupported("driven replay: a tt record it can't read");
                    return;
                };
                if surprise != w.combat.turns.ctl.acting_in_surprise_round {
                    w.coverage.unsupported(
                        "driven replay: TurnController.Tick: the turn's surprise-round flag differs from the sim's",
                    );
                }
                let empty = self.uc_value(
                    w,
                    empty,
                    w.combat.commands.empty(&rider),
                    "Tick's Commands.Empty",
                );
                self.tt_open = true;
                self.out.turn_ticks += 1;
                let t = &mut w.combat.turns.ctl;
                t.stage = Stage::Start;
                // `else if (Status == Acting && Rider.Commands.Empty && !Rider.IsDirectlyControllable)`.
                t.ai_due = false;
                if std::mem::take(&mut self.ai_forced) != 0 {
                    w.coverage.unsupported(
                        "driven replay: a forced AI tick outside the turn controller's tick",
                    );
                }
                if t.status == Status::Acting && empty {
                    match ctrl {
                        Some(c) => t.ai_due = !c,
                        None => w.coverage.unsupported(
                            "driven replay: TurnController.Tick: the rider's IsDirectlyControllable isn't recorded",
                        ),
                    }
                }
            }
            "tt_prep" => {
                let (Some(ctrl), Some(prone), Some(in_tick)) =
                    (flag("ctrl"), flag("prone"), flag("in_tick"))
                else {
                    w.coverage.unsupported("driven replay: a tt_prep record it can't read");
                    return;
                };
                if r.get("mctrl") != Some(&J::Null) {
                    w.coverage.unsupported(
                        "TurnController.Prepare: a turn with a mount (or not recorded)",
                    );
                }
                if r["u"].as_str() != Some(rider.as_str()) {
                    w.coverage.unsupported("driven replay: TurnController.Prepare of another unit than the current one");
                }
                let t = &w.combat.turns.ctl;
                let expected = if in_tick {
                    // The camera's scroll is over (an input): `Prepare` from the tick's start.
                    t.stage == Stage::Start && t.status == Status::Scrolling
                } else {
                    // `CurrentTurn.Start` without a scroll, inside `StartTurn`.
                    self.turn_begin.is_some() && t.stage == Stage::Out && t.status == Status::None
                };
                if !expected {
                    w.coverage.unsupported("driven replay: TurnController.Prepare where the sim's turn controller doesn't call it");
                }
                let sim_prone = w.unit(u).is_some_and(|x| x.prone_active);
                if sim_prone != prone {
                    w.coverage.unsupported("driven replay: TurnController.Prepare: the rider's prone state differs from the recording's");
                }
                // `Prepare` updates the rider's priority target (`UnitImpatienceController`), then
                // ticks the AI (`ForceTick`), before its end.
                if self.imp && std::mem::take(&mut self.imp_prepared) != [u] {
                    w.coverage.unsupported(
                        "driven replay: TurnController.Prepare without exactly one UpdatePriorityTarget of its rider",
                    );
                }
                if self.ai && std::mem::take(&mut self.ai_prepared) != 1 {
                    w.coverage.unsupported(
                        "driven replay: TurnController.Prepare without exactly one AI entry (ForceTick)",
                    );
                }
                tt::prepare(&mut w.combat.turns.ctl, ctrl, sim_prone);
                self.tt_check(w, r, "TurnController.Prepare's end");
            }
            "tt_sc" => {
                if w.combat.turns.ctl.stage != Stage::Start {
                    w.coverage
                        .unsupported("driven replay: the tick's status change out of its order");
                }
                // `m_RunningCommands.Count`: a C# `int`, never negative.
                let running =
                    r["running"].as_i64().filter(|n| (0..=i64::from(i32::MAX)).contains(n));
                let (Some(empty), Some(running), Some(proj)) =
                    (flag("r_empty"), running, flag("proj"))
                else {
                    w.coverage.unsupported("driven replay: a tt_sc record it can't read");
                    return;
                };
                if flag("m_empty") != Some(true) {
                    w.coverage.unsupported(
                        "driven replay: the tick's status change: a turn with a mount (or not recorded)",
                    );
                }
                let empty = self.uc_value(
                    w,
                    empty,
                    w.combat.commands.empty(&rider),
                    "the status change's Commands.Empty",
                );
                let sim_running =
                    i64::try_from(w.combat.commands.running.len()).unwrap_or(i64::MAX);
                let running = if self.uc && self.uc_rows.ready && sim_running != running {
                    w.coverage.unsupported(
                        "driven replay: the turn's running commands differ from the sim's at the status change",
                    );
                    running
                } else {
                    running
                };
                // The forced AI tick ran before: whether the rider has commands now is its choice.
                // With the ai observer the forced AI tick is the AI's entry: one exactly when due.
                if self.ai && self.ai_forced != u32::from(w.combat.turns.ctl.ai_due) {
                    w.coverage.unsupported(
                        "driven replay: the turn controller's forced AI tick differs from the AI's entries",
                    );
                }
                self.ai_forced = 0;
                if w.combat.turns.ctl.ai_due {
                    tt::after_forced_ai_tick(&mut w.combat.turns.ctl, empty);
                }
                let want = r["st"].as_str().and_then(Status::parse);
                if want != Some(w.combat.turns.ctl.status) {
                    w.coverage.unsupported("driven replay: the turn's status differs from the recording's at the tick's status change");
                    if let Some(s) = want {
                        w.combat.turns.ctl.status = s;
                    }
                }
                // The section's `IsAbleToAct` calls follow (`tt_able`): it's decided at the next
                // record of the tick.
                let all_acted = tt::is_acted(w, u);
                let input = tt::StatusInput {
                    commands_empty: empty,
                    able: None,
                    running: running > 0,
                    projectile: Some(proj),
                };
                self.tt_sc = Some((input, all_acted));
                w.combat.turns.ctl.stage = Stage::StatusPending;
            }
            "tt_able" => {
                let Some(res) = flag("res") else {
                    w.coverage.unsupported("driven replay: a tt_able record it can't read");
                    return;
                };
                match &mut self.tt_sc {
                    Some((i, _))
                        if w.combat.turns.ctl.stage == Stage::StatusPending
                            && r["u"].as_str() == Some(rider.as_str())
                            && i.able.is_none() =>
                    {
                        i.able = Some(res);
                    }
                    _ => w.coverage.unsupported(
                        "driven replay: an IsAbleToAct in the status change of another unit (a mount) or out of its order",
                    ),
                }
            }
            "tt_ca" => {
                if w.combat.turns.ctl.stage != Stage::ContinueActing
                    || flag("rider") != Some(true)
                    || r["u"].as_str() != Some(rider.as_str())
                {
                    w.coverage.unsupported("driven replay: ContinueActing where the sim's status change doesn't call it");
                }
                let time = self.tt_time(w, r, "ContinueActing");
                let input = (|| {
                    Some(tt::ActingInput {
                        running: flag("run")?,
                        get_up: flag("getup")?,
                        panicked: flag("panicked")?,
                        able: opt("able")?,
                        can_get_up_on_command: flag("ucgu")?,
                        controllable: opt("ctrl")?,
                        extra_action: opt("extra")?,
                        surprise_round: flag("surprise")?,
                        motion: flag("motion")?,
                        wants_to_move: flag("wants")?,
                        no_more_movement: flag("nomore")?,
                        time: time?,
                    })
                })();
                let (Some(mut input), Some(res)) = (input, flag("res")) else {
                    w.coverage.unsupported("driven replay: a tt_ca record it can't read");
                    return;
                };
                let sim_run = w.combat.commands.is_running(&rider);
                input.running =
                    self.uc_value(w, input.running, sim_run, "ContinueActing's IsRunning");
                let state = tt::ActingState {
                    in_combat: super::is_in_combat(w, u),
                    remaining_time: super::cooldowns::remaining_time(w, u),
                    meters_five_foot: w.combat.turns.movement.meters_moved_by_five_foot_step,
                };
                if flag("in_combat") != Some(state.in_combat) {
                    w.coverage.unsupported("driven replay: ContinueActing: the unit's IsInCombat differs from the recording's");
                }
                if input.surprise_round != w.combat.turns.ctl.acting_in_surprise_round {
                    w.coverage.unsupported("driven replay: ContinueActing: the turn's surprise-round flag differs from the sim's");
                }
                self.tt_counters(w, &r["wb"], "ContinueActing's start");
                let t = &mut w.combat.turns.ctl;
                t.stage = Stage::StatusDone;
                let go_on = match tt::continue_acting(t, &input, &state) {
                    Ok(g) => g,
                    Err(what) => {
                        w.coverage.unsupported(format!(
                            "driven replay: ContinueActing: {what} isn't recorded"
                        ));
                        res
                    }
                };
                if !go_on {
                    tt::to_end(&mut w.combat.turns.ctl);
                }
                if go_on != res {
                    w.coverage.unsupported(
                        "driven replay: ContinueActing's result differs from the recording's",
                    );
                    w.combat.turns.ctl.status = if res { Status::Acting } else { Status::Ending };
                }
                self.tt_counters(w, &r["wa"], "ContinueActing's end");
            }
            "tt_cw" => {
                self.tt_status_done(w, "ContinueWaiting");
                if w.combat.turns.ctl.status != Status::Ending {
                    w.coverage
                        .unsupported("driven replay: ContinueWaiting of a turn that isn't ending");
                }
                let time = self.tt_time(w, r, "ContinueWaiting");
                let (Some(running), Some(get_up), Some(time), Some(res)) =
                    (flag("r_run"), flag("getup"), time, flag("res"))
                else {
                    w.coverage.unsupported("driven replay: a tt_cw record it can't read");
                    return;
                };
                if flag("m_run") != Some(false) {
                    w.coverage.unsupported("driven replay: ContinueWaiting: a turn with a mount");
                }
                let sim_run = w.combat.commands.is_running(&rider);
                let running = self.uc_value(w, running, sim_run, "ContinueWaiting's IsRunning");
                self.tt_counters(w, &r["wb"], "ContinueWaiting's start");
                let go_on = tt::continue_waiting(
                    &mut w.combat.turns.ctl,
                    &tt::WaitingInput { running, get_up, time },
                );
                if go_on != res {
                    w.coverage.unsupported(
                        "driven replay: ContinueWaiting's result differs from the recording's",
                    );
                }
                self.tt_counters(w, &r["wa"], "ContinueWaiting's end");
                if !res {
                    // `End()`: its writes, `Ended`, then its interrupted commands end.
                    self.apply_end(w, Some(u));
                    w.combat.turns.ctl.status = Status::Ended;
                    w.combat.turns.current_ended = true;
                    self.out.turn_ends += 1;
                    self.tt_end_due = true;
                }
            }
            "tt_end" => {
                match w.combat.turns.ctl.stage {
                    Stage::Done => {}
                    _ => {
                        self.tt_status_done(w, "TurnController.Tick's end");
                        if w.combat.turns.ctl.status == Status::Ending {
                            w.coverage.unsupported(
                                "driven replay: an ending turn's tick without its ContinueWaiting",
                            );
                        }
                    }
                }
                self.tt_check(w, r, "TurnController.Tick's end");
                if !std::mem::take(&mut self.tt_open) {
                    w.coverage.unsupported("driven replay: a tt_end without its tick's start (tt)");
                }
                if std::mem::take(&mut self.tt_end_due) {
                    w.coverage.unsupported(
                        "driven replay: the sim's tick ended the turn, but the recording has no turn_end",
                    );
                }
                let t = &mut w.combat.turns.ctl;
                t.stage = Stage::Out;
                t.ai_due = false;
            }
            _ => {}
        }
    }

    /// WrathOracle's `cd_*` records: the calls that write units' cooldowns.
    fn cd_record(&mut self, w: &mut World, ev: &str, r: &J) {
        let u = r["u"]
            .as_str()
            .and_then(|id| w.by_id.get(id).copied())
            .filter(|u| w.unit(*u).is_some());
        let caller = r["caller"].as_str().unwrap_or("");
        let Some(u) = u else {
            // `Cooldowns.Clear` of a cooldowns object no live unit holds (the oracle leaves out
            // disposed units): only leaving combat, matched with the sim's clear of a disposed unit.
            let ownerless = ev == "cd_clear"
                && r["u"].is_null()
                && r.get("u").is_some()
                && caller.contains("UnitCombatState.Clear");
            let logged = w.combat.cooldown_clears.as_ref().and_then(|q| q.front().copied());
            match logged {
                Some((x, before))
                    if ownerless && (w.entity(x).is_disposed || w.entity(x).disposing) =>
                {
                    if let Some(q) = &mut w.combat.cooldown_clears {
                        q.pop_front();
                    }
                    if r["b"].as_str().and_then(parse_cooldowns) != Some(before) {
                        w.coverage.unsupported(
                            "driven replay: a unit's cooldowns differ from the recording's before UnitCombatState.Clear",
                        );
                    }
                }
                _ => w
                    .coverage
                    .unsupported(format!("driven replay: {ev} of an unknown unit (or not a unit)")),
            }
            return;
        };
        // The turn's records say whether it has a mount (not modelled).
        if matches!(ev, "cd_move" | "cd_force_end" | "cd_cmd_end")
            && r["mount"].as_bool() != Some(false)
        {
            w.coverage
                .unsupported(format!("driven replay: {ev}: a turn with a mount (or not recorded)"));
        }
        match ev {
            // A command acted (`UnitCommandController`), or acts as the turn is prepared: an input.
            "cd_spend" => {
                if caller.contains("Kingmaker.Controllers.Units.UnitCommandController.") {
                    self.place(w, Phase::Commands, "a command's SpendAction");
                    if self.uc && !self.uc_spend_ok && !self.uc_skipping {
                        w.coverage.unsupported(
                            "driven replay: a command's SpendAction the sim's commands didn't make",
                        );
                    }
                }
                self.cd_before(w, u, &r["b"], "SpendAction");
                #[allow(clippy::cast_possible_truncation)]
                let since = r["tsc"].as_str().and_then(|x| x.parse::<f32>().ok());
                let (Some(ty), Some(full), Some(since)) =
                    (r["type"].as_i64(), r["full"].as_bool(), since)
                else {
                    w.coverage.unsupported("driven replay: a cd_spend record it can't read");
                    return;
                };
                if !super::cooldowns::spend_action(w, u, ty, full, since) {
                    w.coverage.unsupported("UnitEntityData.SpendAction: an unknown command type");
                }
                // A command's SpendAction the sim's commands made isn't an input.
                if !self.uc_spend_ok {
                    *self.out.cooldown_inputs.entry("SpendAction").or_default() += 1;
                }
                self.cd_after(w, u, &r["a"], "SpendAction");
            }
            // The movement agent moved the turn's unit (`UnitMoveController`): its delta is an input.
            "cd_move" => {
                self.place(w, Phase::Move, "a turn's movement (TickMovement)");
                self.cd_before(w, u, &r["b"], "TickMovement");
                self.stats_check(w, &r["sb"], "TickMovement's start");
                let f = |k: &str| r[k].as_str().and_then(|x| x.parse::<f32>().ok());
                let b = |k: &str| r[k].as_bool();
                // Every input is required; only the speed may be null (the oracle couldn't read it).
                let speed = match &r["speed"] {
                    J::Null if r.get("speed").is_some() => Some(None),
                    J::String(x) => x.parse::<f32>().ok().map(Some),
                    _ => None,
                };
                let flags = (b("force"), b("near"), b("modify"), b("modify_req"), b("moving"));
                let more = (b("ai5"), b("auto"), b("panicked"));
                let (
                    (Some(dt), Some(limit), Some(speed)),
                    (
                        Some(force),
                        Some(near),
                        Some(modify_current),
                        Some(modify_request),
                        Some(moving),
                    ),
                    (Some(ai5), Some(auto_stop), Some(panicked)),
                ) = ((f("dt"), r["limit"].as_i64(), speed), flags, more)
                else {
                    w.coverage.unsupported("driven replay: a cd_move record it can't read");
                    return;
                };
                let input = super::cooldowns::MoveInput {
                    dt,
                    force,
                    near_the_end: near,
                    limit,
                    modify_current,
                    modify_request,
                    moving,
                    ai_used_five_foot_step: ai5,
                    auto_stop,
                    speed,
                    panicked,
                };
                if Some(u) != w.combat.current_unit {
                    w.coverage.unsupported(
                        "driven replay: TickMovement of another unit than the current one",
                    );
                }
                match super::cooldowns::tick_movement(w, u, &input) {
                    Some(out) if Some(out) == f("dt_out") => {}
                    Some(_) => w.coverage.unsupported(
                        "driven replay: TickMovement's delta out differs from the recording's",
                    ),
                    None => w.coverage.unsupported(
                        "driven replay: TickMovement needs the unit's speed, which the recording doesn't have",
                    ),
                }
                *self.out.cooldown_inputs.entry("TickMovement").or_default() += 1;
                self.cd_after(w, u, &r["a"], "TickMovement");
                self.stats_check(w, &r["sa"], "TickMovement's end");
            }
            // The end-turn button (the oracle's driver), an interrupted AI command, a stuck AI.
            "cd_force_end" => {
                self.cd_before(w, u, &r["b"], "ForceToEnd");
                self.stats_check(w, &r["sb"], "ForceToEnd's start");
                if Some(u) != w.combat.current_unit {
                    w.coverage.unsupported(
                        "driven replay: ForceToEnd of another unit than the current one",
                    );
                }
                let Some(set) = r["set"].as_bool() else {
                    w.coverage.unsupported("driven replay: a cd_force_end record it can't read");
                    return;
                };
                super::cooldowns::force_to_end(w, u, set);
                if self.tt {
                    super::turn_tick::to_end(&mut w.combat.turns.ctl);
                }
                *self.out.cooldown_inputs.entry("ForceToEnd").or_default() += 1;
                self.cd_after(w, u, &r["a"], "ForceToEnd");
                self.stats_check(w, &r["sa"], "ForceToEnd's end");
            }
            // The rider's command ended: whether it empties the movement is the command's.
            "cd_cmd_end" => {
                const STATUSES: [&str; 7] =
                    ["None", "Scrolling", "Preparing", "Acting", "Delayed", "Ending", "Ended"];
                let (Some(status), Some(simulating), true) = (
                    r["status"].as_str().filter(|s| STATUSES.contains(s)),
                    r["simulating"].as_bool(),
                    r["empties"].is_null() && r.get("empties").is_some()
                        || r["empties"].is_boolean(),
                ) else {
                    w.coverage.unsupported("driven replay: a cd_cmd_end record it can't read");
                    return;
                };
                if self.tt && status != w.combat.turns.ctl.status.name() {
                    w.coverage.unsupported(
                        "driven replay: HandleUnitCommandDidEnd: the turn's status differs from the sim's",
                    );
                }
                // `End()` sets `Ended` after its writes, then interrupts the rider's commands.
                if status == "Ended" {
                    self.apply_end(w, Some(u));
                }
                self.cd_before(w, u, &r["b"], "HandleUnitCommandDidEnd");
                self.stats_check(w, &r["sb"], "HandleUnitCommandDidEnd's start");
                if r["exec"].as_str() != r["u"].as_str() {
                    w.coverage
                        .unsupported("TurnController.HandleUnitCommandDidEnd: the mount's command");
                }
                if !super::cooldowns::command_did_end(
                    w,
                    u,
                    simulating,
                    status,
                    r["empties"].as_bool(),
                ) {
                    w.coverage.unsupported(
                        "driven replay: HandleUnitCommandDidEnd: the command's DoesCommandEmptiesMovement isn't recorded",
                    );
                }
                *self.out.cooldown_inputs.entry("HandleUnitCommandDidEnd").or_default() += 1;
                self.cd_after(w, u, &r["a"], "HandleUnitCommandDidEnd");
                self.stats_check(w, &r["sa"], "HandleUnitCommandDidEnd's end");
            }
            "cd_clear" => {
                // The prepare controller's batch is the sim's own, after the controller's rolls (its
                // clears precede the sim's flush); so is leaving combat (its clears are logged).
                if caller.contains("UnitCombatPrepareController.Tick") {
                    self.place(w, Phase::Prepare, "the prepare controller's batch");
                    self.cd_before(w, u, &r["b"], "the prepare controller's batch");
                    return;
                }
                if caller.contains("UnitCombatState.Clear") {
                    self.tgt_clear_open(w, u);
                    let logged = w
                        .combat
                        .cooldown_clears
                        .as_mut()
                        .and_then(std::collections::VecDeque::pop_front);
                    match logged {
                        Some((x, before)) if x == u => {
                            if r["b"].as_str().and_then(parse_cooldowns) != Some(before) {
                                w.coverage.unsupported(
                                    "driven replay: a unit's cooldowns differ from the recording's before UnitCombatState.Clear",
                                );
                            }
                        }
                        _ => w.coverage.unsupported(
                            "driven replay: a UnitCombatState.Clear the sim didn't make (or another unit's)",
                        ),
                    }
                    return;
                }
                if !caller.contains("TurnBased.Controllers.TurnController.Prepare") {
                    w.coverage.unsupported(format!("driven replay: Cooldowns.Clear from {caller}"));
                    return;
                }
                if caller.contains("CombatController.StartTurn") {
                    self.place(w, Phase::Time, "TurnController.Prepare (in StartTurn)");
                } else if caller.contains("TurnController.Tick") {
                    // The camera's scroll to the unit ended (`TurnStatus.Scrolling`).
                    self.place(
                        w,
                        Phase::Turn,
                        "TurnController.Prepare (after the camera's scroll)",
                    );
                } else {
                    w.coverage.unsupported(format!(
                        "driven replay: TurnController.Prepare from {caller}"
                    ));
                }
                if Some(u) != w.combat.current_unit || self.turn_prepared {
                    w.coverage.unsupported(
                        "driven replay: TurnController.Prepare of another unit or twice",
                    );
                }
                self.turn_prepared = true;
                // (`Prepare`'s `UpdatePriorityTarget` comes after its interrupts and this clear.)
                self.imp_prepare_ready = Some(u);
                self.cd_before(w, u, &r["b"], "TurnController.Prepare");
                super::cooldowns::prepare_clear(w, u);
                // `Prepare` goes on (after its acting commands' `SpendAction`): the rider's
                // attacks of opportunity refilled (mounts aren't modelled), the new round.
                super::engage::refill(w, u);
                super::cooldowns::new_combat_round(w, u, "TurnController.Prepare");
                super::cooldowns::tick_next_round(w, u);
            }
            "cd_write" => {
                let writer = r["writer"].as_str().unwrap_or("?");
                self.cd_before(w, u, &r["b"], writer);
                if writer == "UnitCombatState.set_AttackOfOpportunityCount" && self.eng {
                    // The engagement's own writes are pulled as its checks.
                    w.coverage.unsupported(
                        "driven replay: an attacks of opportunity count write the sim didn't make",
                    );
                } else if writer == "UnitCombatState.set_AttackOfOpportunityCount" {
                    // Using an attack of opportunity with the round's count full (the engagement
                    // controller and the command aren't ported: an input) starts its cooldown.
                    if let Some(x) = w.unit_mut(u) {
                        x.combat.cooldown.attack_of_opportunity = 5.399_999_6;
                    }
                    *self.out.cooldown_inputs.entry("AttackOfOpportunityCount").or_default() += 1;
                } else {
                    w.coverage.unsupported(format!("cooldowns written by {writer} (not ported)"));
                }
                self.cd_after(w, u, &r["a"], writer);
            }
            _ => {}
        }
    }

    /// Processes the record at `i`; returns the next index.
    fn record(&mut self, w: &mut World, i: usize) -> usize {
        let trace = self.trace;
        let recs = &trace.records;
        let r = &recs[i];
        self.tgt_clear_boundary(w, r);
        match r["k"].as_str() {
            // A frame starts at the `tick` record (`GameMode.Tick`'s prefix): the previous
            // frame's remaining phases run first.
            Some("tick") => {
                let m = r["mode"].as_i64().unwrap_or(-1);
                // A new game mode activates its controllers (`GameMode.OnActivate`: e.g.
                // `UnitBuffsController.Activate`), which the replay doesn't run.
                if self.mode.is_some_and(|prev| prev != m) {
                    w.coverage.unsupported(
                        "driven replay: a game mode change (its controllers' Activate/Deactivate)",
                    );
                }
                self.mode = Some(m);
                self.cd_frame_end(w);
                self.uc_frame_end(w);
                self.tgt_frame_end(w);
                self.imp_frame_end(w);
                let outgoing = self.frame.mode;
                let rest = self.frame.start(m);
                self.draining = outgoing;
                self.run_phases(w, &rest);
                self.draining = None;
                if self.tt {
                    self.tt_close(w);
                }
                if self.port {
                    self.frame_leftovers(w);
                    let t = r["t"].as_i64().unwrap_or(-1);
                    w.combat.turns.choose =
                        self.choose_by_tick.remove(&t).unwrap_or_default().into();
                    self.started_this_frame = false;
                    self.turn_go_on = true;
                }
                let first = self.frame_at.is_none();
                // `WR_DEBUG_UNSUPPORTED`: each report printed under its frame (see `Coverage`).
                if std::env::var_os("WR_DEBUG_UNSUPPORTED").is_some() {
                    eprintln!("DEBUG tick {}", r["t"]);
                }
                self.frame_at = Some(i);
                self.tick = r["t"].as_i64().unwrap_or(-1);
                self.time_seen = false;
                if self.turn_begin.take().is_some() {
                    w.coverage
                        .unsupported("driven replay: a turn_start_begin without its turn_start");
                }
                // The window's first prefix observes the snapshot made before it.
                if first && let Some(set) = self.awake_sets.get(&self.tick) {
                    super::life::update_awake(w);
                    self.out.awake_recorded += apply_recorded_awake(w, set);
                }
                if registered(m, Phase::Prefix).is_none() {
                    w.coverage.unsupported(format!(
                        "driven replay: game mode {m}: its controllers are not modelled"
                    ));
                }
                w.combat.cutscene = m == mode::CUTSCENE;
                w.combat.commands_paused = m == mode::PAUSE;
                super::life::update_turn_based(w);
            }
            Some("obs")
                if self.uc
                    && r["ev"].as_str().is_some_and(|e| {
                        e.starts_with("uc_")
                            || e.starts_with("ua_")
                            || e.starts_with("ai_")
                            || e.starts_with("eng_")
                            || e.starts_with("tgt_")
                            || e.starts_with("imp_")
                    }) =>
            {
                return self.uc_record(w, i);
            }
            Some("obs") if self.ag && r["ev"].as_str().is_some_and(|e| e.starts_with("ag_")) => {
                return self.ag_record(w, i);
            }
            Some("obs") if self.ag && r["ev"] == "path_ret" => self.ag_path_ret(w, r),
            Some("obs") => self.obs(w, r),
            // The clocks after `TimeController.Tick`.
            Some("time") => {
                self.place(w, Phase::Time, "the time record");
                // Its frame's own clock: a record of another frame is reported, not taken as this one's.
                if r["t"].as_i64() == Some(self.tick) {
                    self.time_seen = true;
                } else {
                    w.coverage.unsupported("driven replay: a time record of another frame");
                }
                // The turn controller's tick reads the frame's deltas and time scales: required.
                if self.tt
                    && ["game_delta", "delta", "player_time_scale", "debug_time_scale"]
                        .iter()
                        .any(|k| !r[*k].is_number())
                {
                    w.coverage.unsupported(
                        "driven replay: a time record without the deltas or time scales the turn controller reads",
                    );
                }
                set_clock(w, r);
                // `TimeController.Tick` calls `TickTime` in turn-based combat.
                if self.port && r["turn_based"].as_bool() == Some(true) {
                    if self.cd {
                        self.check_tick_time(w, r);
                    }
                    super::turns::tick_time(w, self.started_this_frame);
                }
                if r["turn_based"].as_bool() != Some(w.combat.turn_based) {
                    w.coverage.unsupported(
                        "driven replay: turn-based combat state differs from the time record's",
                    );
                }
            }
            // The oracle's inputs are made after `Game.Tick`.
            Some("input") => {
                self.place(w, Phase::Post, "an input");
                match r["step"].as_str() {
                    Some("tbattack" | "move") => self.tgt_driver_input(w, i),
                    Some(x) if x.starts_with("attack:") => self.tgt_driver_input(w, i),
                    Some("tbm:on") => {
                        w.combat.tb_mode_on = true;
                        super::life::update_turn_based(w);
                    }
                    Some("tbm:off") => {
                        w.combat.tb_mode_on = false;
                        super::life::update_turn_based(w);
                    }
                    _ => {}
                }
            }
            Some("cmd") => {
                // (`UnitCommand.Start` raised the impatience controller's handler before
                // WrathOracle's `cmd` note of it.)
                if let Err(d) = self.imp_due(w) {
                    w.coverage.unsupported(format!("driven replay: commands: {}", d.0));
                    w.combat.impatience.queue.clear();
                }
                let (Some(u), Some(class)) = (unit_id(&r["unit"]), r["class"].as_str()) else {
                    return i + 1;
                };
                let list = self.running.entry(u.to_owned()).or_default();
                match r["ev"].as_str() {
                    Some("start") => list.push(class.to_owned()),
                    Some("end") => {
                        if let Some(p) = list.iter().position(|c| c == class) {
                            list.remove(p);
                        }
                    }
                    _ => {}
                }
            }
            Some("tb") => self.tb(w, i),
            // The tutorial system (UI-driven) attaches a fact: it takes the next unique id
            // (`Player.GetNewUniqueId`). A `ShowNewTutorial` the sim ran (a combat trigger's
            // actions) has attached it already, and the ids must agree; otherwise the sim takes
            // the id here, and its next must equal the recorded one.
            Some("mut") if r["what"] == "fact_attach" && r["class"] == "Tutorial" => {
                let recorded = r["fact"]["$fact"].as_str();
                let computed = recorded.is_some_and(|id| {
                    w.facts.iter().any(|f| f.clr == "Tutorial" && f.unique_id == id)
                });
                let id =
                    if computed { None } else { Some(crate::narrative::Player::new_unique_id(w)) };
                if id.is_some_and(|id| recorded != Some(id.as_str())) {
                    w.coverage.unsupported(
                        "driven replay: a tutorial fact's unique id differs from the sim's next id",
                    );
                }
            }
            Some("rule") if is_root(r) => return self.root(w, i),
            // With the approach observer `TickApproaching`'s draws are the sim's commands' (each
            // taken where it draws): one reaching here wasn't (its tape draw is taken to keep the
            // site in step).
            Some("draw") if self.approach_draw_record(r) => {
                let _ = w.rng.range_float(crate::rng::sites::TICK_APPROACHING, 0.2, 0.6);
                if !self.uc_skipping {
                    w.coverage.unsupported(
                        "driven replay: a TickApproaching draw the sim's commands didn't make",
                    );
                }
            }
            // With the ai observer `SelectAction`'s draws are the sim's AI's.
            Some("draw") if self.ai_draw_record(r) => {
                let _ = w.rng.range_float(crate::rng::sites::AI_NEXT_COMMAND_TIME, 0.0, 0.1);
                if !self.uc_skipping {
                    w.coverage.unsupported("driven replay: an AI draw the sim's AI didn't make");
                }
            }
            Some("draw") if !self.imp && self.imp_family(r) => {
                w.coverage.unsupported(
                    "driven replay: an impatience record without the impatience observer",
                );
            }
            Some("draw") if self.imp_draw_record(r) => {
                self.imp_skip_draw(w, r);
                if !self.uc_skipping {
                    w.coverage.unsupported(
                        "driven replay: an impatience draw the sim's controller didn't make",
                    );
                }
            }
            _ => {}
        }
        i + 1
    }

    fn obs(&mut self, w: &mut World, r: &J) {
        match r["ev"].as_str() {
            Some("u") => {
                if let (Some(u), Some(row)) =
                    (r["u"].as_str(), r["s"].as_str().and_then(Row::parse))
                {
                    self.pos.last.insert(u.to_owned(), row);
                }
            }
            // `Observe.Tick`'s gate: the current turn at this prefix (`none` or `<unit> <status>`).
            Some("turn_status") if r["g"] == true => {
                let now = r["d"].as_str().and_then(|d| d.split_once("->")).map(|(_, n)| n);
                if self.turn_records {
                    // The turn controller's records set the current turn; the gate checks it.
                    let want = match now {
                        Some("none") => Some(None),
                        Some(s) => {
                            let u =
                                w.by_id.get(s.rsplit_once(' ').map_or(s, |(id, _)| id)).copied();
                            if u.is_none() {
                                w.coverage.unsupported(
                                    "driven replay: the current turn's unit is unknown",
                                );
                                return;
                            }
                            Some(u)
                        }
                        None => None,
                    };
                    if want.is_some_and(|x| x != w.combat.current_unit) {
                        w.coverage.unsupported(
                            "driven replay: the current turn differs from the recorded turn status",
                        );
                        w.combat.current_unit = want.flatten();
                    }
                    let status = now.and_then(|s| s.rsplit_once(' ')).map(|(_, st)| st);
                    if self.tt && status.is_some_and(|st| st != w.combat.turns.ctl.status.name()) {
                        w.coverage.unsupported(
                            "driven replay: the turn's status differs from the recorded turn status",
                        );
                    }
                    return;
                }
                match now {
                    Some("none") => {
                        w.combat.current_unit = None;
                        w.combat.current_unit_maybe_cleared = false;
                    }
                    Some(s) => {
                        let (id, status) = s.rsplit_once(' ').unwrap_or((s, ""));
                        w.combat.current_unit = w.by_id.get(id).copied();
                        // An ended turn waits for the turn controller to clear it.
                        w.combat.current_unit_maybe_cleared = status == "Ended";
                        if w.combat.current_unit.is_none() {
                            w.coverage
                                .unsupported("driven replay: the current turn's unit is unknown");
                        }
                    }
                    None => {}
                }
            }
            // The turn port's inputs and checks (WrathOracle's group observations).
            Some("group_state") if self.port => self.group_state(w, r),
            Some("leave_tick") if self.port => {
                if r["via"] != "Tick" {
                    w.coverage.unsupported(
                        "UnitCombatLeaveController.TickGroups called directly (DialogController.Start)",
                    );
                    return;
                }
                self.place(w, Phase::LeavePrefix, "the leave controller's inputs");
                self.leave_tick(w, r);
            }
            Some("cmds") if self.port => {
                self.place(w, Phase::LeavePrefix, "the units' commands");
                let Some(u) = r["u"].as_str() else { return };
                let list = parse_commands(r["s"].as_str().unwrap_or("/"));
                if self.uc && self.uc_rows.ready && model_commands(w, u) != list {
                    if std::env::var_os("WR_DEBUG_COMMANDS").is_some() {
                        eprintln!(
                            "DEBUG commands tick {} cmds {u}: sim {:?} rec {list:?}",
                            self.tick,
                            model_commands(w, u)
                        );
                    }
                    w.coverage.unsupported(
                        "driven replay: a unit's commands at the leave controller differ from the sim's",
                    );
                }
                w.combat
                    .groups
                    .commands
                    .get_or_insert_with(HashMap::new)
                    .insert(u.to_owned(), list);
            }
            Some("leave_combat") if self.port => self.leave_combat(w, r),
            Some("turn_tick") if self.port => {
                self.place(w, Phase::TurnPrefix, "the turn controller's tick");
                let s = r["s"].as_str().unwrap_or("");
                let want = format!(
                    "guard={}|changed={}",
                    w.combat.turns.waiting_for_ui,
                    u8::from(w.combat.turns.units_changed)
                );
                if s != want {
                    w.coverage.unsupported(
                        "driven replay: the turn controller's UI guard or units-changed flag differs from the recording's",
                    );
                }
            }
            // Cooldowns aren't ticked by the sim (`UnitCombatCooldownsController`, the turns'
            // action accounting): the recorded ones are inputs.
            Some("cooldowns") if self.cd => {
                self.place(w, Phase::Prepare, "the units' cooldowns");
                match (r["u"].as_str(), r["s"].as_str().and_then(parse_cooldowns)) {
                    (Some(u), Some(c)) => {
                        self.cd_rows.insert(u.to_owned(), c);
                    }
                    _ => w.coverage.unsupported("driven replay: a cooldowns record it can't read"),
                }
            }
            Some(ev @ ("tt" | "tt_sc" | "tt_able" | "tt_prep" | "tt_ca" | "tt_cw" | "tt_end"))
                if self.tt =>
            {
                self.turn_tick(w, ev, r);
            }
            // The prefix's rows are complete.
            Some("cd_prefix") if self.cd => {
                self.place(w, Phase::Prepare, "the units' cooldowns");
                self.cd_check(w);
            }
            Some(
                ev @ ("cd_spend" | "cd_move" | "cd_force_end" | "cd_cmd_end" | "cd_clear"
                | "cd_write"),
            ) if self.cd => self.cd_record(w, ev, r),
            Some("cooldowns") if self.port => {
                self.place(w, Phase::Prepare, "the units' cooldowns");
                let u = r["u"].as_str().and_then(|id| w.by_id.get(id).copied());
                let v: Vec<f32> = r["s"]
                    .as_str()
                    .unwrap_or("")
                    .split('|')
                    .filter_map(|x| x.parse().ok())
                    .collect();
                match (u, v.as_slice()) {
                    (Some(u), [i, s, m, sw, aoo]) => {
                        let c = super::Cooldowns {
                            initiative: *i,
                            standard_action: *s,
                            move_action: *m,
                            swift_action: *sw,
                            attack_of_opportunity: *aoo,
                        };
                        if let Some(x) = w.unit_mut(u)
                            && x.combat.cooldown != c
                        {
                            x.combat.cooldown = c;
                            self.out.cooldowns_recorded += 1;
                        }
                    }
                    _ => w.coverage.unsupported("driven replay: a cooldowns record it can't read"),
                }
            }
            // `WaitingForUI` (the UI's guard), in the order it changed.
            Some("ui_guard") if self.port => self.ui_guard(w, r),
            Some("ui_guard_poll") if self.port && r["g"] == true => {
                let now = r["d"].as_str().and_then(|d| d.split_once("->")).map(|(_, n)| n);
                if now.and_then(|n| n.parse::<i32>().ok()) != Some(w.combat.turns.waiting_for_ui) {
                    w.coverage.unsupported(
                        "driven replay: the UI guard differs from the recording's at a prefix",
                    );
                }
            }
            // `UnitEntityData.JoinCombat` (`UnitCombatJoinController`): who joins is an input.
            Some("join_combat") => {
                self.place(w, Phase::Join, "a combat join");
                let Some(u) = r["u"].as_str().and_then(|id| w.by_id.get(id).copied()) else {
                    w.coverage.unsupported("driven replay: a combat join of an unknown unit");
                    return;
                };
                // Outside turn-based mode `JoinCombat` returns early for a unit whose standard
                // command has acted, which the recording doesn't show.
                if !w.combat.tb_mode_on {
                    w.coverage.unsupported(
                        "UnitEntityData.JoinCombat outside turn-based mode: its early return is not recorded",
                    );
                }
                super::life::join_combat(w, u);
            }
            _ => {}
        }
    }

    fn tb(&mut self, w: &mut World, i: usize) {
        let trace = self.trace;
        let r = &trace.records[i];
        let game_time = r["game_time"].as_i64().map(crate::csharp::timespan::TimeSpan);
        match r["ev"].as_str() {
            Some("sort" | "round_start" | "turn_clear") if self.port => self.turn_event(w, i),
            Some("sort") => {
                if r["reason"] == "HandleDelayTurn" {
                    w.coverage
                        .unsupported("driven replay: a delayed turn (CombatController.m_NextUnit)");
                }
                let before = self.out.sorts;
                check_sort(w, r, &mut self.out.sorts);
                if self.out.sorts.members_mismatch > before.members_mismatch {
                    w.coverage.unsupported(
                        "driven replay: the turn controller's units differ from the sort's",
                    );
                }
                if self.out.sorts.initiative_mismatch > before.initiative_mismatch {
                    w.coverage.unsupported(
                        "driven replay: a sorted unit's initiative differs from the sim's",
                    );
                }
            }
            // `HandleCombatStart`: `Clear()` (no turn, the initiative cache emptied), then
            // `TurnStartTime = GameTime` (the record's: it is written as the call returns).
            Some("combat_start") => {
                self.place_combat_change(w, "a combat start");
                w.combat.combat_active = true;
                super::life::update_turn_based(w);
                clear_turns(w);
                // `m_Units`: every unit of `State.Units` in combat; then initialized.
                // `foreach u in State.Units`: the pool (`NotFake`, the states' units) through its
                // enumerator (in the game, not suppressed).
                let units: Vec<EntityIdx> = crate::scene::units_pool(w)
                    .into_iter()
                    .filter(|u| crate::scene::enumerated_by_pool(w, *u) && in_combat(w, *u))
                    .collect();
                w.combat.tb_units = Some(units.clone());
                if self.port {
                    self.port_combat_start(w, r, units);
                }
                match game_time {
                    Some(t) => w.combat.turn_start_time = t,
                    None => w.coverage.unsupported("driven replay: combat start without its time"),
                }
            }
            // `StartTurn`'s entry (its prefix): unless it returns early (the unit isn't in combat),
            // the turn is installed and every awake unit's buffs tick with the previous
            // `TurnStartTime`. What the recording holds before this in the frame comes before
            // the sweep.
            Some("turn_start_begin") => {
                if self.time_seen {
                    w.coverage.unsupported(
                        "driven replay: a turn start after its frame's TimeController record",
                    );
                }
                self.place(w, Phase::Time, "a turn start");
                // `!unit.IsInCombat || FindUnitInfo(unit) == null`: it returns early.
                let u = unit_id(&r["unit"]).and_then(|id| w.by_id.get(id).copied());
                if u.is_none() {
                    w.coverage.unsupported("driven replay: a turn start of an unknown unit");
                }
                let started = u.is_some_and(|u| {
                    in_combat(w, u) && w.combat.tb_units.as_ref().is_some_and(|m| m.contains(&u))
                });
                if self.port {
                    // `TickTime` starts the next unit's turn when its time to the next turn has
                    // run out.
                    if self.cd && u.is_some_and(|u| super::cooldowns::time_to_next_turn(w, u) > 0.0)
                    {
                        w.coverage.unsupported(
                            "driven replay: a turn starts before the sim's cooldowns let it (GetTimeToNextTurn > 0)",
                        );
                    }
                    // `StartTurn(m_NextUnit); m_NextUnit = null` from `TickTime`.
                    super::turns::turn_started(w, u, started);
                    self.started_this_frame = true;
                }
                if started {
                    // A new `TurnController`: its movement stats start at zero, its status `None`.
                    w.combat.turns.movement = super::cooldowns::MovementStats::default();
                    w.combat.turns.immune_on_disengage = Some(false);
                    let surprise = u.is_some_and(|u| {
                        w.combat
                            .turns
                            .units
                            .iter()
                            .any(|x| x.unit == u && x.acting_in_surprise_round)
                    });
                    w.combat.turns.ctl = super::turn_tick::TurnCtl {
                        acting_in_surprise_round: surprise,
                        ..Default::default()
                    };
                    self.turn_prepared = false;
                    self.turn_end_applied = false;
                }
                if started {
                    w.combat.current_unit = u;
                    w.combat.current_unit_maybe_cleared = false;
                    // A new `TurnController`: no running commands.
                    if self.uc {
                        w.combat.commands.running.clear();
                    }
                    super::life::turn_start_sweep(w);
                }
                self.turn_begin = Some((i, started));
            }
            // `StartTurn`'s postfix: `TurnStartTime = GameTime` was set before `CurrentTurn.Start`.
            Some("turn_start") if self.turn_begin.is_some() => {
                let Some((begin, started)) = self.turn_begin.take() else { return };
                if self.inside_start_turn(begin, i) {
                    w.coverage.unsupported(
                        "CombatController.StartTurn: a root rule, input or turn event inside it",
                    );
                }
                if unit_id(&trace.records[begin]["unit"]) != unit_id(&r["unit"]) {
                    w.coverage
                        .unsupported("driven replay: turn_start of another unit than its entry");
                }
                // `CurrentTurn.Start` prepares the turn unless the camera scrolls to the unit
                // first (`TurnStatus.Scrolling`: `Prepare` runs from the turn controller's tick
                // once the scroll ends, on the camera's clock: its `cd_clear` places it).
                if self.tt && started {
                    use super::turn_tick::Status;
                    let t = &mut w.combat.turns.ctl;
                    match r["status"].as_str().and_then(Status::parse) {
                        // `TryScrollToUnit` (an input: the camera setting, the rider's visibility).
                        Some(Status::Scrolling) if t.status == Status::None => {
                            t.status = Status::Scrolling;
                        }
                        Some(s) if s != Status::Scrolling && s == t.status => {}
                        _ => w.coverage.unsupported(
                            "driven replay: the turn's status at its start differs from the sim's",
                        ),
                    }
                }
                if self.cd && started && !self.turn_prepared && r["status"] != "Scrolling" {
                    w.coverage.unsupported(
                        "driven replay: a turn started without TurnController.Prepare clearing its cooldowns",
                    );
                }
                if started {
                    match game_time {
                        Some(t) => w.combat.turn_start_time = t,
                        None => {
                            w.coverage.unsupported("driven replay: turn start without its time");
                        }
                    }
                }
            }
            // `StartTurn` without its recorded entry (an older recording): the turn is installed,
            // every awake unit's buffs tick with the previous `TurnStartTime`, then
            // `TurnStartTime = GameTime`.
            Some("turn_start") => {
                if self.time_seen {
                    w.coverage.unsupported(
                        "driven replay: a turn start after its frame's TimeController record",
                    );
                }
                self.place(w, Phase::Time, "a turn start");
                if self.frame_at.is_some_and(|f| self.unordered_before(f, i)) {
                    w.coverage.unsupported(
                        "CombatController.StartTurn: records before turn_start in its frame can't be ordered against the buff sweep (its entry is not recorded)",
                    );
                }
                w.combat.current_unit = unit_id(&r["unit"]).and_then(|id| w.by_id.get(id).copied());
                w.combat.current_unit_maybe_cleared = false;
                super::life::turn_start_sweep(w);
                match game_time {
                    Some(t) => w.combat.turn_start_time = t,
                    None => w.coverage.unsupported("driven replay: turn start without its time"),
                }
            }
            // `TurnController.End`: the turn stays `CurrentTurn` until it is cleared
            // (`turn_clear`; an older recording doesn't show when).
            Some("turn_end") => {
                // `TurnController.End` runs only in `TurnController.Tick`, from
                // `CombatController.Tick`.
                self.place(w, Phase::Turn, "a turn end");
                if self.tt {
                    // The sim's tick has ended the turn (`ContinueWaiting`) and applied `End`.
                    let u = unit_id(&r["unit"]).and_then(|id| w.by_id.get(id).copied());
                    if r["t"].as_i64() != Some(self.tick)
                        || !std::mem::take(&mut self.tt_end_due)
                        || w.combat.turns.ctl.status != super::turn_tick::Status::Ended
                        || u != w.combat.current_unit
                    {
                        w.coverage.unsupported(
                            "driven replay: a turn end the sim's turn controller didn't make",
                        );
                    }
                } else if self.port {
                    w.combat.turns.current_ended = true;
                }
                if self.cd && !self.tt {
                    let u = unit_id(&r["unit"]).and_then(|id| w.by_id.get(id).copied());
                    self.apply_end(w, u);
                }
                if !self.turn_records {
                    w.coverage.unsupported(
                        "driven replay: when an ended turn is cleared is not recorded",
                    );
                    w.combat.current_unit_maybe_cleared = true;
                }
            }
            // `CombatController.CurrentTurn = null`: from `CombatController.Tick` (an ended turn,
            // `HandleDelayTurn`), `Clear()` at combat start and end, or `RemoveUnit` (a unit
            // leaving combat, in whichever controller made it leave).
            Some("turn_clear") => self.turn_clear(w, i),
            // `DropLootAndDestroyOnDeactivate`'s UniRx timer fires (a coroutine after `Game.Tick`).
            Some("loot_timer") => {
                self.place(w, Phase::Post, "a loot timer");
                let owner = unit_id(&r["owner"]).and_then(|id| w.by_id.get(id).copied());
                let fact = r["fact"].as_str();
                if !super::life::fire_timer(w, owner, fact) {
                    w.coverage.unsupported("driven replay: a loot timer the sim didn't start");
                }
            }
            Some("combat_end") => {
                self.place_combat_change(w, "a combat end");
                if self.port {
                    // `Clear()`: the current turn, if any, is cleared (its `turn_clear` follows).
                    if let Some(c) = w.combat.current_unit {
                        w.combat.turns.log.push_back(super::turns::TbEvent::Clear(c));
                    }
                    super::turns::clear(w);
                }
                clear_turns(w);
                w.combat.tb_units = None;
                w.combat.combat_active = false;
                super::life::update_turn_based(w);
            }
            _ => {}
        }
    }

    /// The party's combat state changes (`HandleCombatStart`/`HandleCombatEnd`) from
    /// `Player.UpdateIsInCombat` in `UnitCombatJoinController`, or from the turn-based setting
    /// (an input, after `Game.Tick`). It changes what the frame's later controllers read
    /// (turn-based combat, `TurnStartTime`), so it must be placed.
    fn place_combat_change(&mut self, w: &mut World, what: &str) {
        if self.frame.at != Phase::Post {
            self.place(w, Phase::Join, what);
        }
    }

    /// A `turn_clear` record (`CurrentTurn = null`). Its source decides its phase: the sim's own
    /// `RemoveUnit` (a death or a leave it ran) has cleared the turn already; `Clear()` in
    /// `HandleCombatStart`/`HandleCombatEnd` goes with those records; `CombatController.Tick`
    /// clears an ended turn right after `TurnController.End` (with only `ChooseNextUnit`'s
    /// `sort`/`round_start` between). Any other source (a `RemoveUnit` the sim didn't run) is
    /// reported while the frame's buffs controller, which reads the current turn, is pending.
    fn turn_clear(&mut self, w: &mut World, i: usize) {
        let trace = self.trace;
        let recs = &trace.records;
        let r = &recs[i];
        let u = unit_id(&r["unit"]).and_then(|id| w.by_id.get(id).copied());
        if u.is_some() && w.combat.cleared_by_remove.take() == u {
            return;
        }
        let in_frame = |j: &&J| j["t"] == r["t"];
        let next_tb =
            recs[i + 1..].iter().take_while(|j| j["k"] != "tick").find(|j| j["k"] == "tb");
        let prev_tb = recs[..i].iter().rev().take_while(in_frame).find(|j| j["k"] == "tb");
        let combat_end = prev_tb.is_some_and(|j| j["ev"] == "combat_end");
        let combat_change = combat_end || next_tb.is_some_and(|j| j["ev"] == "combat_start");
        if !combat_end && (u.is_none() || u != w.combat.current_unit) {
            w.coverage.unsupported("driven replay: a turn clear of a unit that isn't current");
        }
        // The records since the turn's `turn_end`, in this frame: only `ChooseNextUnit`'s.
        let after_end = recs[..i]
            .iter()
            .rev()
            .take_while(in_frame)
            .take_while(|j| {
                j["k"] == "tb"
                    && matches!(j["ev"].as_str(), Some("sort" | "round_start" | "turn_end"))
            })
            .any(|j| j["ev"] == "turn_end" && unit_id(&j["unit"]) == unit_id(&r["unit"]));
        if after_end {
            self.place(w, Phase::Turn, "a turn clear");
        } else if !combat_change
            && self.frame.at < Phase::Life
            && u.is_some_and(|u| super::life::life_tick_would_leave_combat(w, u))
        {
            // The unit dies in this frame's life controller: its `LeaveCombat` removes it
            // (`RemoveUnit`). The clear is that one only if the sim's own removal makes it.
            self.place(w, Phase::Life, "a turn clear");
            if u.is_some() && w.combat.cleared_by_remove.take() == u {
                return;
            }
            w.coverage.unsupported(
                "driven replay: a turn clear the life controller's RemoveUnit doesn't make",
            );
        } else if !combat_change
            && self.frame.mode.and_then(|m| registered(m, Phase::Buffs)) == Some(true)
            && self.frame.at < Phase::Buffs
        {
            w.coverage.unsupported(
                "driven replay: a turn clear whose source is not established (RemoveUnit the sim didn't run): its order against the buffs controller is not established",
            );
        }
        w.combat.current_unit = None;
        w.combat.current_unit_maybe_cleared = false;
    }

    /// The end of the root at `j` if it changes nothing (an echoed root, or one computed at its
    /// call site: a check or a count, which reads state and writes none).
    fn inert_root(&self, j: usize) -> Option<usize> {
        self.echoed
            .get(&j)
            .copied()
            .or_else(|| self.origins.get(&j).filter(|(_, c)| c.is_some()).map(|(e, _)| *e))
    }

    /// Whether anything inside `StartTurn` (between its entry and its postfix) is a root rule the
    /// driver would run, an input, a turn event or a combat join: the sweep's own mutations and
    /// draws, observations, commands and inert roots (echoed, or the AI's checks and counts in
    /// `TurnController.Prepare`) are not.
    fn inside_start_turn(&self, from: usize, to: usize) -> bool {
        let trace = self.trace;
        let recs = &trace.records;
        let mut j = from + 1;
        while j < to {
            let r = &recs[j];
            if is_root(r) {
                match self.inert_root(j) {
                    Some(end) => {
                        j = end + 1;
                        continue;
                    }
                    None => return true,
                }
            }
            match r["k"].as_str() {
                Some("tb" | "input") => return true,
                Some("obs") if r["ev"] == "join_combat" => return true,
                _ => {}
            }
            j += 1;
        }
        false
    }

    /// Whether anything between this frame's `tick` record and the record at `to` changes what
    /// the sim computes or is compared: a driven root rule, a mutation, a turn-controller
    /// event, an input or a combat join. Observation rows, out-of-rule draws, commands and the
    /// echoed (inert) roots don't.
    fn unordered_before(&self, from: usize, to: usize) -> bool {
        let trace = self.trace;
        let recs = &trace.records;
        let mut j = from + 1;
        while j < to {
            let r = &recs[j];
            if is_root(r) {
                match self.inert_root(j) {
                    Some(end) => {
                        j = end + 1;
                        continue;
                    }
                    None => return true,
                }
            }
            match r["k"].as_str() {
                Some("mut" | "tb" | "input") => return true,
                Some("obs") if r["ev"] == "join_combat" => return true,
                _ => {}
            }
            j += 1;
        }
        false
    }

    fn root(&mut self, w: &mut World, i: usize) -> usize {
        let next = self.root_inner(w, i);
        // The impatience handlers' records inside it, after the sim fired it (or didn't).
        self.imp_span(w, i, next - 1);
        next
    }

    fn root_inner(&mut self, w: &mut World, i: usize) -> usize {
        let trace = self.trace;
        let recs = &trace.records;
        let r = &recs[i];
        let end = root_end(recs, i);
        let class = short(r["class"].as_str().unwrap_or("")).to_owned();
        self.out.roots += 1;
        // With the attacks observer, `UnitAttack`'s rules are its sim code's (`commands::attack`):
        // one reaching here wasn't triggered by it (processed as before).
        if w.combat.commands.attacks
            && !self.uc_skipping
            && r["origin"].as_str().is_some_and(|o| {
                commands::in_unit_attack(o, "InitAttacks")
                    || commands::in_unit_attack(o, "TriggerAttackRule")
            })
        {
            w.coverage.unsupported(format!(
                "driven replay: a root {class} of UnitAttack's code the sim's commands didn't trigger"
            ));
        }
        // With the approach observer an attack of opportunity's rule is its sim code's too.
        if w.combat.commands.approach
            && !self.uc_skipping
            && r["origin"].as_str().is_some_and(|o| {
                o.starts_with("Kingmaker.UnitLogic.Commands.UnitAttackOfOpportunity.")
            })
        {
            w.coverage.unsupported(format!(
                "driven replay: a root {class} of UnitAttackOfOpportunity's code the sim's commands didn't trigger"
            ));
        }
        if self.echoed.get(&i) == Some(&end) {
            // Its origin is not established (a command, the UI, the turn preparation, a
            // tutorial trigger can all make these rules), so it marks no controller phase.
            *self.out.echoed.entry(class).or_default() += 1;
            w.coverage.unsupported(
                "driven replay: presentation root rules echoed, their origin is not established",
            );
            return end + 1;
        }
        if let Some(&(_, caller)) = self.origins.get(&i) {
            let Some(c) = caller else {
                w.coverage.unsupported(format!(
                    "driven replay: a root {class} whose call site isn't placed (or that changes state)"
                ));
                *self.out.skipped.entry(class).or_default() += 1;
                return end + 1;
            };
            // `TurnController.Prepare` runs inside `StartTurn`, after its buff sweep.
            if c == Caller::AiStartTurn && self.turn_begin.is_none() {
                w.coverage.unsupported(format!(
                    "driven replay: a root {class} from StartTurn outside its recorded StartTurn"
                ));
            }
            self.place(w, c.phase(), &format!("a root {class} ({})", c.label()));
            *self.out.by_origin.entry(c.label()).or_default() += 1;
            self.drive_root(w, i, end);
            return end + 1;
        }
        if recs[i..=end].iter().any(|x| x["k"] == "obs" && x["ev"] == "join_combat") {
            w.coverage.unsupported("driven replay: a combat join inside a rule is not modelled");
        }
        let tiebreak = class == "RuleRollD20"
            && self.out.tiebreak.is_some()
            && self.out.tiebreak.as_deref() == unit_id(&r["initiator"]);
        let phase = match class.as_str() {
            "RuleInitiativeRoll" => Some(Phase::Prepare),
            _ if tiebreak => Some(Phase::Prepare),
            "RuleAttackWithWeapon" => Some(Phase::Commands),
            "RuleAttackWithWeaponResolve" => Some(Phase::Projectiles),
            "RuleCanApplyBuff" => Some(Phase::Fx),
            _ => None,
        };
        match phase {
            Some(p) => {
                self.place(w, p, &format!("a root {class}"));
                self.drive_root(w, i, end);
            }
            None => {
                w.coverage.unsupported(format!("driven replay: root rule {class}"));
                *self.out.skipped.entry(class).or_default() += 1;
            }
        }
        end + 1
    }

    fn drive_root(&mut self, w: &mut World, start: usize, end: usize) {
        let trace = self.trace;
        let cx = self.cx.clone();
        let r = &trace.records[start];
        let class = short(r["class"].as_str().unwrap_or("")).to_owned();
        let a = unit_id(&r["initiator"]).and_then(|id| w.by_id.get(id).copied());
        let t = unit_id(&r["target"]).and_then(|id| w.by_id.get(id).copied());
        match class.as_str() {
            "RuleInitiativeRoll" => {
                let Some(a) = a else {
                    *self.out.skipped.entry(class).or_default() += 1;
                    return;
                };
                // `UnitCombatPrepareController` prepares units already in combat; the join is
                // recorded (`join_combat`).
                if !w.unit(a).is_some_and(|x| x.combat.in_combat) {
                    w.coverage.unsupported(
                        "driven replay: initiative of a unit whose combat join is not recorded",
                    );
                    super::life::join_combat(w, a);
                }
                super::initiative::prepare_unit(w, a);
                self.out.tiebreak = unit_id(&r["initiator"]).map(str::to_owned);
            }
            // The UI's, the AI's and the commands' attack counts (`UnitAttack.EnumerateFullAttack`,
            // `CreateSingleAttack`, `UIUtilityItem`).
            "RuleCalculateAttacksCount" => {
                let Some(a) = a else {
                    *self.out.skipped.entry(class).or_default() += 1;
                    return;
                };
                let mut rule =
                    super::counts::RuleCalculateAttacksCount::from_snapshot(w, a, &r["fields"]);
                fire::fire(w, &cx, &mut rule, super::counts::RuleCalculateAttacksCount::result);
            }
            "RuleCalculateAttacksCountLimbs" => {
                let Some(a) = a else {
                    *self.out.skipped.entry(class).or_default() += 1;
                    return;
                };
                let mut rule = super::counts::RuleCalculateAttacksCountLimbs::from_snapshot(
                    w,
                    a,
                    &r["fields"],
                );
                fire::fire(
                    w,
                    &cx,
                    &mut rule,
                    super::counts::RuleCalculateAttacksCountLimbs::result,
                );
            }
            // The initiative tracker's flat-footed badges (`InitiativeTrackerUnitVM.UpdateData`):
            // what the check reads besides conditions is the root's recorded `env`.
            "RuleCheckTargetFlatFooted" => {
                let (Some(a), Some(t)) = (a, t) else {
                    *self.out.skipped.entry(class).or_default() += 1;
                    return;
                };
                let f = &r["fields"];
                if f["IgnoreVisibility"].as_bool() == Some(true)
                    || f["IgnoreConcealment"].as_bool() == Some(true)
                {
                    w.coverage.unsupported(
                        "RuleCheckTargetFlatFooted: IgnoreVisibility or IgnoreConcealment set",
                    );
                }
                let Some(env) = flat_footed_env(trace, start, end, a, t, w) else {
                    w.coverage.unsupported(
                        "driven replay: a root flat-footed check without its recorded environment",
                    );
                    *self.out.skipped.entry(class).or_default() += 1;
                    return;
                };
                let mut rule = super::attack::RuleCheckTargetFlatFooted {
                    base: crate::rulebook::RuleBase::default(),
                    initiator: a,
                    target: t,
                    force_flat_footed: f["ForceFlatFooted"].as_bool().unwrap_or(false),
                    env,
                    is_flat_footed: false,
                };
                fire::fire(w, &cx, &mut rule, |r| Some(json!({"IsFlatFooted": r.is_flat_footed})));
            }
            // The tie-break d20 `UnitCombatPrepareController` rolls right after the initiative.
            "RuleRollD20" => {
                self.out.tiebreak = None;
                *self.out.folded.entry(class).or_default() += 1;
            }
            "RuleAttackWithWeapon" => {
                let (Some(a), Some(t)) = (a, t) else {
                    *self.out.skipped.entry(class).or_default() += 1;
                    return;
                };
                // A root attack is a command's (`UnitAttack.OnAction` in `UnitCommandController`)
                // only if the initiator runs an attack command; other sources (abilities,
                // cutscenes) act in other phases.
                let running = self.running.get(&w.entity(a).unique_id).is_some_and(|l| {
                    l.iter().any(|c| c == "UnitAttack" || c == "UnitAttackOfOpportunity")
                });
                if self.uc && self.uc_rows.ready {
                    let m = &w.combat.commands;
                    let sim = m.all_of(&w.entity(a).unique_id).iter().any(|c| {
                        m.get(*c).is_some_and(|x| {
                            x.s.running() && (x.is("UnitAttack") || x.is("UnitAttackOfOpportunity"))
                        })
                    });
                    if sim != running {
                        w.coverage.unsupported(
                            "driven replay: a root attack's running attack command differs from the sim's commands",
                        );
                    }
                }
                if !running {
                    w.coverage.unsupported(
                        "driven replay: a root attack without a running attack command (its controller is not established)",
                    );
                }
                if let Some(env) = env_in(trace, start, end, a, t, w) {
                    w.combat.env.push(((a, t), env));
                }
                let before = w.combat.pending_resolves.len();
                match super::attack::attack_from_snapshot(w, a, t, &r["fields"]) {
                    Some(mut rule) => {
                        fire::fire(w, &cx, &mut rule, |_| None);
                        // The resolves this attack launched, by the recorded rule identity.
                        let id = r["id"].as_i64();
                        for p in w.combat.pending_resolves.iter_mut().skip(before) {
                            p.source_id = id;
                        }
                    }
                    None => {
                        w.coverage
                            .unsupported("driven replay: attack snapshot (weapon) not resolved");
                        *self.out.skipped.entry(class).or_default() += 1;
                    }
                }
            }
            // A ranged attack's projectile arrives (`ProjectileController`): its resolve rule.
            "RuleAttackWithWeaponResolve" => {
                // The resolve names its attack (`AttackWithWeapon`): the recorded rule's id.
                let source = r["fields"]["AttackWithWeapon"]["$rule"].as_i64();
                let cands = matching_resolves(
                    &w.combat.pending_resolves,
                    a.unwrap_or(EntityIdx(u32::MAX)),
                    t.unwrap_or(EntityIdx(u32::MAX)),
                    source,
                );
                if cands.len() > 1 {
                    w.coverage.unsupported("driven replay: several projectiles of one attack");
                }
                let Some(&p) = cands.first() else {
                    w.coverage.unsupported("driven replay: resolve without a launched projectile");
                    *self.out.skipped.entry(class).or_default() += 1;
                    return;
                };
                let mut rule = w.combat.pending_resolves.remove(p);
                let ids = (
                    w.entity(rule.initiator).unique_id.clone(),
                    w.entity(rule.target).unique_id.clone(),
                );
                let at_impact = resolve_env(trace, start, end, &ids.0, &ids.1);
                // The positions at impact (the resolve's damage reads the distance then): the
                // resolve's `env` record, or else the roll's, only where the observations
                // establish both units stood still since.
                if let (Some(d), Some(at)) = (&mut rule.damage, at_impact) {
                    d.positions = Some(at);
                    self.out.impact_positions[0] += 1;
                } else if let Some(d) = &mut rule.damage {
                    let from = rule.source_id.and_then(|id| self.root_at.get(&id).copied());
                    let kept = d.positions.zip(from).and_then(|((pa, pt), from)| {
                        let roll = trace.records[from]["t"].as_i64()?;
                        let impact = r["t"].as_i64()?;
                        let (ia, it) = (
                            w.entity(rule.initiator).unique_id.clone(),
                            w.entity(rule.target).unique_id.clone(),
                        );
                        let still = |u: &str, at, x: EntityIdx| {
                            let next = self.next_row(w, end, impact, u, x);
                            self.pos.still(u, at, roll, impact, next)
                        };
                        (still(&ia, pa, rule.initiator) && still(&it, pt, rule.target))
                            .then_some((pa, pt))
                    });
                    if kept.is_none() {
                        w.coverage.unsupported(
                            "driven replay: positions at impact not established by the observations",
                        );
                        // What the distance decides is then taken from the recording (the
                        // `DistanceDamageBonus` handlers' outcomes in this resolve), and the
                        // resolve is not verified.
                        w.combat.recorded_distance = recorded_distance(&trace.records[start..=end]);
                    }
                    self.out.impact_positions[usize::from(kept.is_none())] += 1;
                    d.positions = kept;
                }
                let first_event = w.events.records.len();
                w.combat.recorded_distance_used = 0;
                fire::fire(w, &cx, &mut rule, |_| None);
                let pending = std::mem::take(&mut cx.sink.borrow_mut().pending);
                for p in pending {
                    w.events.push("draw", p);
                }
                if w.combat.recorded_distance_used > 0 {
                    w.coverage.unsupported(
                        "driven replay: a resolve computed with a recorded DistanceDamageBonus outcome (unverified)",
                    );
                    *self.out.unverified.entry(class.clone()).or_default() += 1;
                    self.unverified.push((start, end));
                    self.sim_skip.push((first_event, w.events.records.len()));
                }
                w.combat.recorded_distance.clear();
                w.combat.recorded_distance_used = 0;
            }
            // `DismemberUnitFX.Init`: `Descriptor.AddBuff(DismemberedUnitBuff, owner, Delay)`,
            // made when `FxSpawnController` spawns the dying unit's queued blood effect. The rule
            // is the buff collection's `PrepareFactForAttach`.
            "RuleCanApplyBuff" => {
                let f = &r["fields"];
                let bp = f["Blueprint"].as_str().and_then(wr_content::Guid::parse);
                let dismember =
                    crate::spawn::root_ref(w, "SystemMechanics", "m_DismemberedUnitBuff")
                        .map(|b| b.guid);
                if bp.is_none() || bp != dismember {
                    // Any other `AddBuff` from outside a rule has an origin the replay can't place.
                    w.coverage
                        .unsupported("driven replay: a root AddBuff of an unestablished origin");
                    *self.out.skipped.entry(class).or_default() += 1;
                    return;
                }
                // `SummonedUnitBuff.OnRemoved` also adds it, directly, where the buff is removed.
                if a.is_some_and(|u| has_component(w, u, "SummonedUnitBuff")) {
                    w.coverage.unsupported(
                        "driven replay: DismemberedUnitBuff of a summoned unit (two origins)",
                    );
                }
                let caster =
                    unit_id(&f["Context"]["caster"]).and_then(|id| w.by_id.get(id).copied());
                let duration = f["Duration"].as_i64().map(crate::csharp::timespan::TimeSpan);
                match (a, bp.and_then(|g| w.content.get(g))) {
                    (Some(owner), Some(bp)) => {
                        let ctx =
                            crate::unit_facts::context(w, caster.or(Some(owner)), Some(owner), &bp);
                        crate::unit_facts::add_buff(w, owner, &bp, ctx, duration);
                    }
                    _ => {
                        w.coverage
                            .unsupported("driven replay: AddBuff root without unit or blueprint");
                        *self.out.skipped.entry(class).or_default() += 1;
                    }
                }
            }
            _ => {
                w.coverage.unsupported(format!("driven replay: root rule {class}"));
                *self.out.skipped.entry(class).or_default() += 1;
            }
        }
    }

    /// The unit's row at the prefix of the frame after `impact` (read ahead in the recording),
    /// if that is established: the unit is in the snapshot `Observe.Tick` will read
    /// (`SleepingUnitsController` ran in this frame) and has rows; the row there, or else its
    /// latest one, unchanged.
    fn next_row(
        &self,
        w: &World,
        from: usize,
        impact: i64,
        unit: &str,
        u: EntityIdx,
    ) -> Option<Row> {
        if !w.combat.awake.contains(&u) || !self.pos.known.contains(unit) {
            return None;
        }
        let trace = self.trace;
        let recs = &trace.records;
        let tick = (from..recs.len()).find(|&j| recs[j]["k"] == "tick")?;
        if recs[tick]["t"].as_i64() != Some(impact + 1) {
            return None;
        }
        let row = recs[tick + 1..]
            .iter()
            .take_while(|x| x["k"] == "obs")
            .filter(|x| x["ev"] == "u" && x["u"] == unit)
            .find_map(|x| x["s"].as_str().and_then(Row::parse));
        row.or_else(|| self.pos.last.get(unit).copied())
    }
}

/// The recorded outcomes of the `DistanceDamageBonus` handlers in a span, by fact id: whether
/// the handler (`OnEventAboutToTrigger`, the `about` phase; its did-trigger handler is empty)
/// changed its rule (it adds the bonus to the damage bundle only within range).
fn recorded_distance(span: &[J]) -> HashMap<String, bool> {
    let mut out = HashMap::new();
    for h in span.iter().filter(|r| r["k"] == "h" && r["phase"] == "about") {
        let is = h["class"].as_str().is_some_and(|c| short(c) == "DistanceDamageBonus");
        let Some(fact) = h["fact"]["$fact"].as_str().filter(|_| is) else { continue };
        let changed = span
            .iter()
            .find(|e| e["k"] == "h_end" && e["id"] == h["id"])
            .and_then(|e| e["changes"].as_array())
            .is_some_and(|c| !c.is_empty());
        out.insert(fact.to_owned(), changed);
    }
    out
}

/// `State.AwakeUnits` at each frame prefix the recording observed: the window's first prefix
/// writes the whole set (`awake_set`), later prefixes its changes (`awake`).
fn awake_sets(t: &Trace) -> BTreeMap<i64, std::collections::BTreeSet<String>> {
    let mut out = BTreeMap::new();
    let mut cur: Option<std::collections::BTreeSet<String>> = None;
    let mut tick = None;
    for r in &t.records {
        match (r["k"].as_str(), r["ev"].as_str()) {
            (Some("tick"), _) => {
                if let (Some(t), Some(c)) = (tick, &cur) {
                    out.insert(t, c.clone());
                }
                tick = r["t"].as_i64();
            }
            (Some("obs"), Some("awake_set")) => {
                cur = Some(
                    r["units"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect(),
                );
            }
            (Some("obs"), Some("awake")) => {
                if let (Some(c), Some(u)) = (&mut cur, r["u"].as_str()) {
                    if r["on"] == true {
                        c.insert(u.to_owned());
                    } else {
                        c.remove(u);
                    }
                }
            }
            _ => {}
        }
    }
    if let (Some(t), Some(c)) = (tick, cur) {
        out.insert(t, c);
    }
    out
}

/// The sim's awake snapshot against the recorded `State.AwakeUnits` of the same frame: a unit
/// whose state the sim establishes must agree (else reported, and the recording's state taken);
/// a unit whose state the sim can't establish takes the recording's. The snapshot keeps the
/// sim's order (`State.Units.All`). Returns how many unknown states the recording settled.
fn apply_recorded_awake(w: &mut World, set: &std::collections::BTreeSet<String>) -> usize {
    let known: HashSet<EntityIdx> = w.combat.awake.iter().copied().collect();
    let unknown: HashSet<EntityIdx> = w.combat.awake_unknown.iter().copied().collect();
    let mut settled = 0;
    let mut awake = Vec::new();
    for u in super::life::all_units(w) {
        let rec = set.contains(&w.entity(u).unique_id);
        if unknown.contains(&u) {
            settled += 1;
        } else if known.contains(&u) != rec {
            w.coverage.unsupported(
                "SleepingUnitsController: the sim's awake state differs from the recording's",
            );
        }
        if rec {
            awake.push(u);
        }
    }
    let ids: HashSet<&str> =
        super::life::all_units(w).into_iter().map(|u| w.entity(u).unique_id.as_str()).collect();
    if set.iter().any(|id| !ids.contains(id.as_str())) {
        w.coverage.unsupported("driven replay: a recorded awake unit the sim doesn't have");
    }
    w.combat.awake = awake;
    w.combat.awake_unknown.clear();
    settled
}

/// The positions at impact: the `env` record WrathOracle writes as a weapon attack's resolve
/// triggers (`RuleAttackWithWeaponResolve.OnTrigger` prefix), inside the resolve's span.
fn resolve_env(
    trace: &Trace,
    start: usize,
    end: usize,
    a: &str,
    t: &str,
) -> Option<([f32; 3], [f32; 3])> {
    let e = trace.records[start..=end].iter().find(|e| {
        e["k"] == "env"
            && e["rule"] == "RuleAttackWithWeaponResolve"
            && unit_id(&e["initiator"]) == Some(a)
            && unit_id(&e["target"]) == Some(t)
    })?;
    pos_pair(&e["at"])
}

/// `CombatController.Clear()` (at combat start and end): no current turn, the initiative cache
/// emptied.
fn clear_turns(w: &mut World) {
    w.combat.cleared_by_remove = None;
    w.combat.current_unit = None;
    w.combat.current_unit_maybe_cleared = false;
    w.combat.known_initiative.clear();
}

fn in_combat(w: &World, u: EntityIdx) -> bool {
    w.unit(u).is_some_and(|x| x.combat.in_combat)
}

/// Whether one of the unit's facts has a component of type `short`.
fn has_component(w: &World, u: EntityIdx, short: &str) -> bool {
    w.entity(u).facts.list.iter().any(|f| {
        w.fact(*f).bp.as_ref().is_some_and(|b| {
            b.components_array().iter().any(|o| wr_content::schema::ty(o.ty).short == short)
        })
    })
}

/// A `sort` event: `m_Units` ordered by `UnitsOrderComaprer`. The recorded override and
/// sequence of each unit are inputs (the turn controller's bookkeeping isn't ported); the
/// order and the rolled initiatives are the sim's.
fn check_sort(w: &World, r: &J, c: &mut SortChecks) {
    use super::order::{Initiative, TbUnitInfo, order_by, tb_compare};
    let Some(list) = r["units"].as_array() else { return };
    let mut infos: Vec<(TbUnitInfo, Initiative)> = Vec::new();
    let mut recorded: Vec<EntityIdx> = Vec::new();
    for e in list {
        let Some(u) = e["unit"].as_str().and_then(|id| w.by_id.get(id).copied()) else { return };
        let Some(unit) = w.unit(u) else { return };
        let mut info = TbUnitInfo::new(u);
        info.initiative_override = e["override"].as_i64().and_then(|v| i32::try_from(v).ok());
        info.initiative_sequence =
            e["sequence"].as_i64().and_then(|v| i32::try_from(v).ok()).unwrap_or(0);
        if e["initiative"].as_i64() != Some(i64::from(unit.combat.initiative.initiative)) {
            c.initiative_mismatch += 1;
        }
        infos.push((info, unit.combat.initiative));
        recorded.push(u);
    }
    let mut ours = w.combat.tb_units.clone().unwrap_or_default();
    let mut theirs = recorded.clone();
    ours.sort_unstable();
    theirs.sort_unstable();
    if ours != theirs {
        c.members_mismatch += 1;
    }
    // The sort starts from the units in `m_Units` order, which the recording doesn't give
    // before sorting: the comparer must order any permutation, so start from the reverse.
    let mut idx: Vec<usize> = (0..infos.len()).rev().collect();
    let clean =
        order_by(&mut idx, |a, b| tb_compare(w, &infos[a].0, infos[a].1, &infos[b].0, infos[b].1));
    c.checked += 1;
    if !clean {
        c.hash_fallback += 1;
    } else if idx.iter().map(|&i| infos[i].0.unit).eq(recorded.iter().copied()) {
        c.equal += 1;
    }
}

/// The pending resolves of one attack: same units, and the recorded identity of the attack when
/// the resolve names it.
fn matching_resolves(
    pending: &[super::attack::RuleAttackWithWeaponResolve],
    a: EntityIdx,
    t: EntityIdx,
    source: Option<i64>,
) -> Vec<usize> {
    pending
        .iter()
        .enumerate()
        .filter(|(_, p)| {
            p.initiator == a && p.target == t && (source.is_none() || p.source_id == source)
        })
        .map(|(i, _)| i)
        .collect()
}

/// A WrathOracle cooldowns row: `Initiative|StandardAction|MoveAction|SwiftAction|AttackOfOpportunity`.
fn parse_cooldowns(s: &str) -> Option<super::Cooldowns> {
    let v: Vec<f32> = s.split('|').map(str::parse).collect::<Result<_, _>>().ok()?;
    let [i, st, m, sw, aoo] = v.as_slice() else { return None };
    Some(super::Cooldowns {
        initiative: *i,
        standard_action: *st,
        move_action: *m,
        swift_action: *sw,
        attack_of_opportunity: *aoo,
    })
}

/// A WrathOracle movement stats row:
/// `TimeMoved|TimeMovedInForceMode|TimeMovedByFiveFootStep|MetersMovedByFiveFootStep`.
fn parse_stats(s: &str) -> Option<super::cooldowns::MovementStats> {
    let v: Vec<f32> = s.split('|').map(str::parse).collect::<Result<_, _>>().ok()?;
    let [a, b, c, d] = v.as_slice() else { return None };
    Some(super::cooldowns::MovementStats {
        time_moved: *a,
        time_moved_in_force_mode: *b,
        time_moved_by_five_foot_step: *c,
        meters_moved_by_five_foot_step: *d,
    })
}

fn set_clock(w: &mut World, r: &J) {
    if let (Some(gt), Some(rt)) = (r["game_time"].as_i64(), r["real_time"].as_i64()) {
        w.clock.game_time = crate::csharp::timespan::TimeSpan(gt);
        w.clock.real_time = crate::csharp::timespan::TimeSpan(rt);
    }
    #[allow(clippy::cast_possible_truncation)]
    if let Some(d) = r["game_delta"].as_f64() {
        w.clock.set_game_delta_time(d as f32);
    }
    #[allow(clippy::cast_possible_truncation)]
    if let Some(d) = r["delta"].as_f64() {
        w.clock.set_delta_time(d as f32);
    }
    #[allow(clippy::cast_possible_truncation)]
    if let (Some(p), Some(d)) = (r["player_time_scale"].as_f64(), r["debug_time_scale"].as_f64()) {
        w.clock.player_time_scale = p as f32;
        w.clock.debug_time_scale = d as f32;
    }
}

/// The leave controller's gate in WrathOracle's `leave_tick` form.
fn gate_row(w: &World) -> String {
    let id = |u: Option<EntityIdx>| u.map(|u| w.entity(u).unique_id.clone()).unwrap_or_default();
    let b = |v: bool| u8::from(v);
    format!(
        "tb={}|has_enemy={}|had_enemy={}|passing={}|current={}|next={}|player_in_combat={}",
        b(w.combat.turn_based),
        b(w.combat.turns.has_enemy),
        b(w.combat.turns.had_enemy),
        b(super::turns::is_passing(w)),
        id(w.combat.current_unit),
        id(w.combat.turns.next),
        b(w.combat.combat_active),
    )
}

/// The `env` record of the attack roll made inside the root rule at `start..=end`. Every value
/// the roll reads must be recorded: `target_flanked` is null outside turn-based combat (the
/// oracle can't read it passively there), and a missing value is not taken as false or zero.
fn env_in(
    trace: &Trace,
    start: usize,
    end: usize,
    a: EntityIdx,
    t: EntityIdx,
    w: &mut World,
) -> Option<AttackEnv> {
    let (ai, ti) = (w.entity(a).unique_id.clone(), w.entity(t).unique_id.clone());
    let e = trace.records[start..=end].iter().find(|e| {
        e["k"] == "env"
            && e["rule"] == "RuleAttackRoll"
            && unit_id(&e["initiator"]) == Some(ai.as_str())
            && unit_id(&e["target"]) == Some(ti.as_str())
    })?;
    let int = |k: &str| e[k].as_i64().and_then(|v| i32::try_from(v).ok());
    let (
        Some(target_flanked),
        Some(target_remembers_initiator),
        Some(target_can_act_in_combat),
        Some(concealment_target),
        Some(concealment_initiator),
    ) = (
        e["target_flanked"].as_bool(),
        e["target_remembers_initiator"].as_bool(),
        e["target_can_act_in_combat"].as_bool(),
        int("concealment_target_from_initiator"),
        int("concealment_initiator_from_target"),
    )
    else {
        w.coverage.unsupported("driven replay: an attack environment value is not recorded");
        return None;
    };
    check_engagement_env(w, t, e, target_flanked);
    Some(AttackEnv {
        target_flanked,
        target_remembers_initiator,
        target_can_act_in_combat,
        concealment_target,
        concealment_initiator,
        target_is_get_up: e["target_is_get_up"].as_bool(),
        target_engaged_by: e["target_engaged_by"].as_array().map(|a| {
            a.iter().filter_map(|v| v.as_str().and_then(|id| w.by_id.get(id).copied())).collect()
        }),
        positions: pos_pair(&e["at"]),
    })
}

/// With the engagement modelled, an attack environment's engagement is a check: `EngagedBy` (the
/// recording's ordinal-sorted ids) and, in turn-based combat, `IsFlanked` (`EngagedBy.Count > 1`
/// unless `CannotBeFlanked`).
fn check_engagement_env(w: &mut World, t: EntityIdx, e: &J, flanked: bool) {
    let Some(x) = w.unit(t).filter(|x| w.combat.engage.on && x.combat.eng.known) else { return };
    let mut sim: Vec<String> =
        x.combat.eng.engaged_by.keys().map(|u| w.entity(u).unique_id.clone()).collect();
    sim.sort_unstable();
    let n = sim.len();
    let recorded: Option<Vec<String>> = e["target_engaged_by"]
        .as_array()
        .and_then(|a| a.iter().map(|v| v.as_str().map(str::to_owned)).collect());
    if recorded.as_ref() != Some(&sim) {
        w.coverage.unsupported(
            "driven replay: an attack environment's EngagedBy differs from the sim's engagement",
        );
    }
    if w.combat.turn_based && super::is_in_combat(w, t) {
        let sim_flanked =
            n > 1 && !crate::components::state::has_mechanics_feature(w, t, &["CannotBeFlanked"]);
        if sim_flanked != flanked {
            w.coverage.unsupported(
                "driven replay: an attack environment's IsFlanked differs from the sim's engagement",
            );
        }
    }
}

/// A root flat-footed check's recorded environment (WrathOracle's `env` for
/// `RuleCheckTargetFlatFooted`: memory, whether the target can act, concealment); the rest of
/// the attack environment is not read by the check.
fn flat_footed_env(
    trace: &Trace,
    start: usize,
    end: usize,
    a: EntityIdx,
    t: EntityIdx,
    w: &World,
) -> Option<AttackEnv> {
    let (ai, ti) = (w.entity(a).unique_id.as_str(), w.entity(t).unique_id.as_str());
    let e = trace.records[start..=end].iter().find(|e| {
        e["k"] == "env"
            && e["rule"] == "RuleCheckTargetFlatFooted"
            && unit_id(&e["initiator"]) == Some(ai)
            && unit_id(&e["target"]) == Some(ti)
    })?;
    let mut env = AttackEnv::neutral();
    env.target_remembers_initiator = e["target_remembers_initiator"].as_bool()?;
    env.target_can_act_in_combat = e["target_can_act_in_combat"].as_bool()?;
    env.concealment_initiator =
        e["concealment_initiator_from_target"].as_i64().and_then(|v| i32::try_from(v).ok())?;
    env.target_is_get_up = None;
    Some(env)
}

fn pos(v: &J) -> Option<[f32; 3]> {
    let a = v.as_array()?;
    #[allow(clippy::cast_possible_truncation)]
    let f = |i: usize| a.get(i).and_then(J::as_f64).map(|x| x as f32);
    Some([f(0)?, f(1)?, f(2)?])
}

fn pos_pair(at: &J) -> Option<([f32; 3], [f32; 3])> {
    Some((pos(&at["initiator_pos"])?, pos(&at["target_pos"])?))
}

mod agent;
mod ai;
mod commands;
mod engage;
mod impatience;
mod targets;

#[cfg(test)]
mod tests;
