//! The AI (decompiled, not copied: `Kingmaker.AI`): `AiBrainController` (`Tick`, `ForceTick`,
//! `TickBrain`, `SelectAction`, `FindBestAction`, `CalculateActionScore`,
//! `CalculateTargetsScore`), the static `DecisionContext` it decides in, `TargetInfo`,
//! `AiAction`'s scoring (`ScoreActor`, `ScoreTarget`, `ScorePath`), `AiActionSimple`,
//! `BlueprintAiAction` and `BlueprintAiAttack`, and the considerations the fights use
//! (`ComplexConsideration`, `InRangeConsideration`, `LifeStateConsideration`,
//! `FactConsideration`, `NotImpatientConsideration`). docs/poc5/commands.md, slice D.
//!
//! Scores are C# `decimal`s ([`Decimal`]); `float` expressions follow Mono's double evaluation
//! ([`store_f32`] where a value is stored). What the code reads outside itself is an input,
//! recorded where the game reads it ([`AiInputs`]): the movement agent and pathing (paths, the
//! movement simulation, line of sight, distances, corpulences), the Unity clock and
//! `NextCommandTime`, the group memory, the impatience controller's state, the settings. The
//! commands it runs and interrupts go through the commands model ([`super::commands`]).
//! Reported, not modelled: other actions and considerations, custom actions, an auto-use
//! ability, confusion, personal enemies, a magus' spell combat, other target types, an aggro
//! filter, action cooldowns and combat counts, `ActionList`s after an action.

use wr_content::Guid;

use super::commands::{self as uc, Desync, Inputs, Of, R};
use crate::csharp::Decimal;
use crate::csharp::arith::store_f32;
use crate::world::{EntityIdx, World};

/// `UnitBrain` as saved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Brain {
    /// `Blueprint`.
    pub blueprint: Option<Guid>,
    /// `AvailableActions`, in order: each `AiActionSimple`'s blueprint (`None` for another kind
    /// of action, or one the save doesn't name).
    pub available: Vec<Option<Guid>>,
    /// `CustomActions.Count`.
    pub custom: usize,
    /// `m_AutoUseAbility` is set.
    pub auto_use: bool,
}

/// A path as `ScorePath` holds it (`Pathfinding.Path`): `error`, `CompleteState` and
/// `vectorPath`.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    pub error: bool,
    pub state: i64,
    pub points: Vec<[f32; 3]>,
}

/// `PathCompleteState.Complete`.
pub const PATH_COMPLETE: i64 = 2;

/// One step of `ScorePath`'s movement simulation (`SimulateTickMovementForTurnBased`): its
/// result and the simulator after it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimStep {
    pub ret: bool,
    pub position: [f32; 3],
    pub last_move_length: f32,
    pub reached_max: bool,
    pub waypoint: i32,
}

/// A group of `Game.ReadyForCombatUnitGroups`: its id (`None` for a fake one), its units and
/// `IsInCombat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRow {
    pub id: Option<String>,
    pub units: Vec<String>,
    pub in_combat: bool,
}

/// What `ai_sc` checks: the score a scoring step left.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoreCheck {
    pub f: &'static str,
    pub action: Guid,
    pub target: Option<String>,
    pub score: Decimal,
    pub best: Decimal,
    /// `ScorePath`'s: `CurrentEnableFiveFootStep` and `CurrentPath`.
    pub path: Option<(bool, Option<Vec<[f32; 3]>>)>,
}

/// What `ai_best` checks: `FindBestAction`'s results.
#[derive(Debug, Clone, PartialEq)]
pub struct BestCheck {
    pub unit: String,
    pub action: Option<Guid>,
    pub target: Option<String>,
    pub best: Decimal,
    pub cur: Decimal,
    /// `BestEnableFiveFootStep` (`None`: not established, the window opened after it was set).
    pub five: Option<bool>,
    pub path: Option<Vec<[f32; 3]>>,
}

/// What the AI's code reads outside itself, and its checks (the driven replay's side:
/// WrathOracle's observer `ai`).
pub trait AiInputs: Inputs {
    /// A boolean leaf named `f` (of unit `u`, when the record names one).
    fn ai_bool(&mut self, w: &mut World, f: &'static str, u: Option<&str>) -> R<bool>;
    /// A `float` leaf.
    fn ai_float(&mut self, w: &mut World, f: &'static str, u: Option<&str>) -> R<f32>;
    /// A unit leaf (`manual`, `last`, `rider`, `ipt`): a unit id or none.
    fn ai_unit(&mut self, w: &mut World, f: &'static str, u: Option<&str>) -> R<Option<String>>;
    /// A blueprint leaf of unit `u` (`auto`, `ptf`): the blueprint or none.
    fn ai_blueprint(&mut self, w: &mut World, f: &'static str, u: &str) -> R<Option<Guid>>;
    /// `UnitEntityData.DistanceTo(to)` of `from`.
    fn ai_dist(&mut self, w: &mut World, from: &str, to: &str) -> R<f32>;
    /// The action blueprint's `GetRange`.
    fn ai_range(&mut self, w: &mut World, action: Guid) -> R<f32>;
    /// `CurrentDifficultyStatAdjustmentCompareTo(option)`.
    fn ai_diff(&mut self, w: &mut World, option: i64) -> R<i64>;
    /// `GetRemainingMovementRange(u, total: true, singleActionMove: false)`.
    fn ai_movement_range(&mut self, w: &mut World, u: &str) -> R<f32>;
    /// The path `FixPathIfTooShort` leaves.
    fn ai_path(&mut self, w: &mut World) -> R<Path>;
    /// `UnitCommand.IsUnitCloseEnough(..., radius, ...)` in `ScorePath`.
    fn ai_close(&mut self, w: &mut World, radius: f32) -> R<bool>;
    /// A movement simulation step, its `maxMoveLength` argument `max`.
    fn ai_sim_step(&mut self, w: &mut World, max: f32) -> R<SimStep>;
    /// `GetNearest(position).node.AllyBlocked`.
    fn ai_blocked(&mut self, w: &mut World) -> R<i64>;
    /// `EngagementSimulator.CheckForAttacksOfOpportunity`.
    fn ai_aoo(&mut self, w: &mut World) -> R<i64>;
    /// `Group.Memory.Enemies`: the units' ids.
    fn ai_enemies(&mut self, w: &mut World) -> R<Vec<String>>;
    /// `Game.ReadyForCombatUnitGroups`.
    fn ai_groups(&mut self, w: &mut World) -> R<Vec<GroupRow>>;
    /// `SelectAction`'s `Random.Range(0f, 0.1f)`.
    fn ai_draw(&mut self, w: &mut World) -> R<f32>;
    /// `UnitCommands.Run(cmd)` of a command `SelectAction` made, in set `set`: its id (the
    /// recording's), once its description (`uc_new`) is checked against `spec`.
    fn ai_new_run(&mut self, w: &mut World, set: u32, spec: &NewCommand) -> R<u32>;
    /// `ForceTick` called by `Tick` (a nested entry) and its end.
    fn ai_nested(&mut self, w: &mut World, end: bool) -> R<()>;
    fn ai_score(&mut self, w: &mut World, check: &ScoreCheck) -> R<()>;
    fn ai_best(&mut self, w: &mut World, check: &BestCheck) -> R<()>;
    /// `SelectAction`'s result, and the unit's `NextCommandTime` when the sim set it.
    fn ai_selected(&mut self, w: &mut World, u: &str, ret: bool, nct: Option<f32>) -> R<()>;
    /// `SelectAction`'s write of unit `u`'s `NextCommandTime` (`ai_nct`).
    fn ai_nct_write(&mut self, w: &mut World, u: &str, v: f32) -> R<()>;
}

/// A command `SelectAction` creates (`BlueprintAiAttack.CreateCommand`: `new UnitAttack(target)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCommand {
    pub class: &'static str,
    pub ty: u8,
    pub target: String,
}

/// The AI's state the sim keeps.
#[derive(Debug, Clone, Default)]
pub struct Ai {
    /// The model runs (a recording with WrathOracle's `ai` observer).
    pub on: bool,
    /// `AiBrainController.LimitedAI` (the entry records it).
    pub limited: bool,
    pub ctx: Context,
    /// Each AI command's action (`AiAction`, by its blueprint: one `AiActionSimple` per blueprint
    /// in a brain), `ForcedPath` and `AiEnableFiveFootStep`, for the commands the sim's AI made.
    pub commands: std::collections::HashMap<u32, AiCommand>,
    /// Each unit's `NextCommandTime` once established (the first read in the window, then every
    /// recorded write: the AI's own checked, the others inputs).
    pub nct: std::collections::HashMap<EntityIdx, f32>,
}

impl Ai {
    /// `BestEnableFiveFootStep` as the recording establishes it (it outlives decisions: the
    /// window's first decision without a best action can't compute it).
    pub fn ctx_adopt_best_five(&mut self, v: bool) {
        self.ctx.best_five = Some(v);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AiCommand {
    pub action: Guid,
    pub forced_path: Option<Vec<[f32; 3]>>,
    pub five: bool,
}

/// `TargetInfo`.
#[derive(Debug, Clone)]
struct TargetInfo {
    unit: EntityIdx,
    id: String,
    distance: f32,
    weight: f32,
    /// `LosStatus` (`None`: `Unknown`).
    los: Option<bool>,
}

/// The static `DecisionContext` (it outlives a decision: the lists last for a group, the paths
/// and `BestEnableFiveFootStep` beyond it).
#[derive(Debug, Clone, Default)]
pub struct Context {
    enemies: Vec<TargetInfo>,
    enemies_init: bool,
    enemies_sorted: bool,
    personal: bool,
    unit: Option<EntityIdx>,
    is_player_faction: bool,
    /// `Target`: an index into `enemies` (`None`: `TargetInfo.Empty`).
    target: Option<usize>,
    action: Option<Guid>,
    action_range: f32,
    best: Decimal,
    cur: Decimal,
    current_path: Option<Path>,
    best_path: Option<Path>,
    current_five: bool,
    best_five: Option<bool>,
    /// The reads that can't change within a decision (a unit's corpulence, targets, movement
    /// gate, AI switch), by leaf and unit: a later read must agree.
    stable: Vec<(&'static str, String, Stable)>,
    /// The group memory's enemies (`Memory.Enemies.Count`), as the enemies list read it.
    memory_enemies: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
enum Stable {
    Float(u32),
    Unit(Option<String>),
    Bool(bool),
    Int(i64),
    Blueprint(Option<Guid>),
}

fn desync<T>(what: impl Into<String>) -> R<T> {
    Err(Desync(what.into()))
}

fn unsupported<T>(what: &str) -> R<T> {
    desync(format!("{what} (not modelled)"))
}

fn dec(f: f32) -> R<Decimal> {
    Decimal::from_f32(f).map_err(|e| Desync(format!("(decimal){f}: {e}")))
}

fn mul(a: Decimal, b: Decimal) -> R<Decimal> {
    a.checked_mul(b).map_err(|e| Desync(format!("{a} * {b}: {e}")))
}

fn uid(w: &World, u: EntityIdx) -> String {
    w.entity(u).unique_id.clone()
}

fn unit_of(w: &World, id: &str) -> R<EntityIdx> {
    match w.by_id.get(id).copied().filter(|e| w.unit(*e).is_some()) {
        Some(u) => Ok(u),
        None => desync(format!("the AI reads unit {id}, which the sim doesn't have")),
    }
}

fn unit_opt(w: &World, id: Option<&str>) -> R<Option<EntityIdx>> {
    id.map(|i| unit_of(w, i)).transpose()
}

fn is_player_faction(w: &World, u: EntityIdx) -> bool {
    w.unit(u).is_some_and(|x| x.is_player_faction)
}

fn turn_based(w: &World) -> bool {
    w.combat.turn_based
}

/// `UnitCondition.Sleeping`.
const SLEEPING: i32 = 16;

// ---- the actions' blueprints ----

/// `AiActionSimple`'s `m_BaseScore` (`UpdateBaseScoreAndConsiderations`): the action's base score
/// times each consideration's modifier (target considerations first, null references skipped);
/// `None` when a blueprint is unknown.
fn base_score(w: &World, g: Guid) -> Option<f32> {
    let bp = w.content.get(g)?;
    let v = bp.view();
    let mut base = v.f32("BaseScore");
    for c in v.guids("m_TargetConsiderations").into_iter().chain(v.guids("m_ActorConsiderations")) {
        base = store_f32(
            f64::from(base) * f64::from(w.content.get(c)?.view().f32("BaseScoreModifier")),
        );
    }
    Some(base)
}

/// A new unit's brain: `new UnitBrain(descriptor)` (`SetBrain(brain)`: an `AiActionSimple` per
/// action of the brain blueprint), then `RestoreAvailableActions` (each action, a cast spell's only
/// when `ShouldBeInActionsList`: kept as `None`, reported when the AI reaches it; then sorted by
/// base score, descending: `List.Sort`'s introsort over a list grown by `Add`).
#[must_use]
pub fn new_brain(w: &World, brain: Option<Guid>) -> Brain {
    let actions: Vec<Guid> = brain
        .and_then(|g| w.content.get(g))
        .map(|b| b.view().guids("m_Actions"))
        .unwrap_or_default();
    let mut list: Vec<(Option<Guid>, f32)> = actions
        .into_iter()
        .map(|g| {
            let simple = w.content.get(g).is_some_and(|b| b.type_name() != "BlueprintAiCastSpell");
            (simple.then_some(g), base_score(w, g).unwrap_or(f32::NAN))
        })
        .collect();
    let cap = crate::csharp::sort::grown_capacity(list.len());
    crate::csharp::introsort(&mut list, cap, &mut |a, b| float_compare(b.1, a.1));
    Brain {
        blueprint: brain,
        available: list.into_iter().map(|x| x.0).collect(),
        custom: 0,
        auto_use: false,
    }
}

/// An `AiActionSimple` of a `BlueprintAiAttack`, as the port supports it.
#[derive(Debug, Clone)]
struct Action {
    guid: Guid,
    base_score: f32,
    actor: Vec<Guid>,
    target: Vec<Guid>,
    min_difficulty: i64,
    invert_difficulty: bool,
    use_on_limited: bool,
    use_when_disabled: bool,
}

fn action(w: &World, g: Guid) -> R<Action> {
    let Some(bp) = w.content.get(g) else {
        return desync(format!("the AI action blueprint {g} is unknown"));
    };
    if bp.type_name() != "BlueprintAiAttack" {
        return unsupported(&format!("an AI action of type {}", bp.type_name()));
    }
    let v = bp.view();
    if v.bool("AdditionalBehaviour") {
        return unsupported("an AI action's AdditionalBehaviour");
    }
    if v.bool("OncePerRound")
        || v.i32("CooldownRounds") != 0
        || v.obj("CooldownDice").is_some_and(|d| d.i32("m_Rolls") != 0)
        || v.i32("StartCooldownRounds") != 0
        || v.i32("CombatCount") != 0
    {
        return unsupported("an AI action's cooldown or combat count");
    }
    if !v.obj("Actions").is_none_or(|a| a.objs("Actions").is_empty()) {
        return unsupported("an AI action's ActionList");
    }
    let actor = v.guids_with_nulls("m_ActorConsiderations");
    let target = v.guids_with_nulls("m_TargetConsiderations");
    let Some(base) = base_score(w, g) else {
        return desync(format!("a consideration of the AI action {g} is unknown"));
    };
    Ok(Action {
        guid: g,
        base_score: base,
        actor: actor.into_iter().flatten().collect(),
        target: target.into_iter().flatten().collect(),
        min_difficulty: v.int("MinDifficulty"),
        invert_difficulty: v.bool("InvertDifficultyRequirements"),
        use_on_limited: v.bool("UseOnLimitedAI"),
        use_when_disabled: v.bool("UseWhenAIDisabled"),
    })
}

// ---- entries ----

/// `AiBrainController.Tick`.
///
/// # Errors
///
/// A desync between the sim and the recording, or something not modelled.
pub fn tick<I: AiInputs>(w: &mut World, inp: &mut I) -> R<()> {
    if turn_based(w) || inp.ai_bool(w, "wfui", None)? {
        return Ok(());
    }
    inp.ai_nested(w, false)?;
    force_tick(w, inp)?;
    inp.ai_nested(w, true)
}

/// `AiBrainController.ForceTick`.
///
/// # Errors
///
/// As [`tick`].
pub fn force_tick<I: AiInputs>(w: &mut World, inp: &mut I) -> R<()> {
    let tb = turn_based(w);
    let groups = inp.ai_groups(w)?;
    // `ReadyForCombatUnitGroups`: in `UnitGroupsController.Groups`' order, each once.
    let mut last: Option<usize> = None;
    let mut listed = Vec::new();
    for g in groups {
        let gi = check_group(w, &g)?;
        let at = w.combat.groups.list.iter().position(|x| *x == gi);
        if at.is_none() || last.is_some_and(|l| at <= Some(l)) {
            return desync(format!(
                "the AI's groups: {:?} out of the sim's group order (or twice)",
                g.id
            ));
        }
        last = at;
        listed.push(gi);
        if provably_not_ready(w, gi) {
            return desync(format!("the AI's groups list group {:?}, which isn't ready", g.id));
        }
        if !g.in_combat {
            continue;
        }
        for id in &g.units {
            let u = unit_of(w, id)?;
            if tb
                && super::is_in_combat(w, u)
                && (inp.leaf(w, "ctrl", Of::Unit(Some(id.clone())))? || !is_current_unit(w, u))
            {
                continue;
            }
            tick_brain(w, inp, u)?;
        }
        release_group(w);
    }
    // Every group the sim can prove ready is listed (`SleepingUnitsController`: one awake member,
    // not of a peaceful faction, in combat).
    for gi in w.combat.groups.list.clone() {
        if !listed.contains(&gi) && provably_ready(w, gi) {
            return desync(format!(
                "the AI's groups miss group {:?}, which is ready",
                w.combat.groups.get(gi).id
            ));
        }
    }
    Ok(())
}

/// A recorded group against the sim's (`UnitGroup`: its units and `IsInCombat`).
fn check_group(w: &mut World, g: &GroupRow) -> R<super::groups::GroupIdx> {
    let groups = &w.combat.groups;
    if !groups.on {
        return unsupported("the AI without the group model");
    }
    // `SleepingUnitsController.Tick` filters `UnitGroupsController.Groups`: a group of that list (a
    // fake one never is).
    let found = g.id.as_deref().and_then(|id| groups.find(id));
    let Some(gi) = found else {
        return desync(format!("the AI's group {:?} is unknown to the sim", g.id));
    };
    let x = groups.get(gi);
    let units: Vec<&str> = x.units.iter().map(|u| w.entity(*u).unique_id.as_str()).collect();
    if units != g.units.iter().map(String::as_str).collect::<Vec<_>>()
        || (x.in_combat > 0) != g.in_combat
    {
        return desync(format!(
            "the AI's group {:?}: sim {units:?} in combat {}, recorded {:?} {}",
            g.id,
            x.in_combat > 0,
            g.units,
            g.in_combat
        ));
    }
    Ok(gi)
}

/// A group `SleepingUnitsController` necessarily lists: a member the sim knows is awake, of a faction
/// that isn't peaceful, in combat.
fn provably_ready(w: &World, g: super::groups::GroupIdx) -> bool {
    w.combat.groups.get(g).units.iter().any(|u| {
        !w.combat.awake_unknown.contains(u)
            && w.combat.awake.contains(u)
            && super::is_in_combat(w, *u)
            && w.unit(*u)
                .and_then(|x| x.faction)
                .and_then(|f| w.content.get(f))
                .is_some_and(|f| !f.view().bool("Peaceful"))
    })
}

/// A group `SleepingUnitsController` can't list: every member known asleep or of a peaceful faction.
fn provably_not_ready(w: &World, g: super::groups::GroupIdx) -> bool {
    w.combat.groups.get(g).units.iter().all(|u| {
        let asleep = !w.combat.awake_unknown.contains(u) && !w.combat.awake.contains(u);
        let peaceful = w
            .unit(*u)
            .and_then(|x| x.faction)
            .and_then(|f| w.content.get(f))
            .is_some_and(|f| f.view().bool("Peaceful"));
        asleep || peaceful
    })
}

/// `UnitEntityData.IsCurrentUnit()`: the current turn's rider (mounts aren't modelled).
fn is_current_unit(w: &World, u: EntityIdx) -> bool {
    uc::current_turn(w) == Some(u)
}

/// `TickBrain(unit)`.
fn tick_brain<I: AiInputs>(w: &mut World, inp: &mut I, u: EntityIdx) -> R<()> {
    let id = uid(w, u);
    let sleeping = inp.ai_bool(w, "sleep", Some(&id))?;
    // `IsSleeping` is what `SleepingUnitsController` decided this frame: the awake units' list,
    // where the sim has it.
    if !w.combat.awake_unknown.contains(&u) && sleeping == w.combat.awake.contains(&u) {
        return desync(format!("{id}'s IsSleeping {sleeping} against the frame's awake units"));
    }
    if sleeping {
        return Ok(());
    }
    // `IsInCombat && Brain != null && (!Passive || IsDirectlyControllable)` (a unit always has a
    // brain).
    let brain = w.unit(u).and_then(|x| x.brain.clone());
    if super::is_in_combat(w, u)
        && (brain.is_none()
            || !inp.ai_bool(w, "passive", None)?
            || inp.leaf(w, "ctrl", Of::Unit(Some(id.clone())))?)
    {
        let Some(brain) = brain else {
            return desync(format!("unit {id} has no brain in the sim"));
        };
        let nct = read_nct(w, inp, u, &id)?;
        let time = inp.ai_float(w, "time", None)?;
        if nct >= time {
            return Ok(());
        }
        let mut can = true;
        let mut any = false;
        let set = unit_set(w, &id)?;
        for c in w.combat.commands.sets[&set].slots {
            let Some(c) = c else { continue };
            any = true;
            if !can_interrupt(w, inp, c)? {
                can = false;
                break;
            }
        }
        if can
            && (!any || inp.leaf(w, "ctrl", Of::Unit(Some(id.clone())))? || {
                let nct = read_nct(w, inp, u, &id)?;
                let time = inp.ai_float(w, "time", None)?;
                // `!(NextCommandTime + 1f >= Time.time)`, in double.
                f64::from(nct) + 1.0 < f64::from(time)
            })
        {
            init_unit(w, u);
            select_action(w, inp, u, &brain)?;
            release_unit(w, inp)?;
        }
    } else {
        // `InterruptAll(cmd => cmd.AiAction != null)` (base code: no entry record of its own).
        let set = unit_set(w, &id)?;
        uc::interrupt_all_where(w, inp, set, |x| x.ai)?;
    }
    Ok(())
}

/// A read of `NextCommandTime`: the sim's once established (every write is recorded).
fn read_nct<I: AiInputs>(w: &mut World, inp: &mut I, u: EntityIdx, id: &str) -> R<f32> {
    let v = inp.ai_float(w, "nct", Some(id))?;
    match w.combat.ai.nct.get(&u) {
        Some(k) if k.to_bits() != v.to_bits() => {
            desync(format!("unit {id}'s NextCommandTime: the sim's {k}, recorded {v}"))
        }
        _ => {
            w.combat.ai.nct.insert(u, v);
            Ok(v)
        }
    }
}

/// A read that can't change within the decision ([`Context::stable`]).
fn stable(w: &mut World, f: &'static str, u: &str, v: Stable) -> R<()> {
    let c = &mut w.combat.ai.ctx;
    match c.stable.iter().find(|(g, x, _)| *g == f && x == u) {
        Some((_, _, x)) if *x != v => {
            desync(format!("{f} of {u} read {x:?}, then {v:?} in one decision"))
        }
        Some(_) => Ok(()),
        None => {
            c.stable.push((f, u.to_owned(), v));
            Ok(())
        }
    }
}

fn stable_float<I: AiInputs>(w: &mut World, inp: &mut I, f: &'static str, u: &str) -> R<f32> {
    let v = inp.ai_float(w, f, Some(u))?;
    stable(w, f, u, Stable::Float(v.to_bits()))?;
    Ok(v)
}

fn stable_unit<I: AiInputs>(
    w: &mut World,
    inp: &mut I,
    f: &'static str,
    u: &str,
) -> R<Option<String>> {
    let v = inp.ai_unit(w, f, Some(u))?;
    stable(w, f, u, Stable::Unit(v.clone()))?;
    Ok(v)
}

fn stable_bool<I: AiInputs>(w: &mut World, inp: &mut I, f: &'static str, u: &str) -> R<bool> {
    let v = inp.ai_bool(w, f, Some(u))?;
    stable(w, f, u, Stable::Bool(v))?;
    Ok(v)
}

fn unit_set(w: &World, id: &str) -> R<u32> {
    match w.combat.commands.unit_set.get(id) {
        Some(s) => Ok(*s),
        None => desync(format!("unit {id}'s command set is unknown")),
    }
}

/// `CanInterrupt(cmd)`: `AiCanInterrupt` an input (the follow controller's and the engagement
/// controller's commands carry a mark the descriptions don't have); a `UnitAttack`'s checked.
fn can_interrupt<I: AiInputs>(w: &mut World, inp: &mut I, c: u32) -> R<bool> {
    let x = &w.combat.commands.cmds[&c];
    if x.s.finished {
        return Ok(true);
    }
    let expect = (x.class == uc::class::ATTACK && !x.rider).then_some(x.ai && !x.s.started);
    let v = inp.leaf(w, "aici", Of::Cmd(c))?;
    if expect.is_some_and(|e| e != v) {
        return desync(format!("command {c}'s AiCanInterrupt: sim {expect:?}, recorded {v}"));
    }
    Ok(v)
}

// ---- the decision context ----

fn ctx(w: &mut World) -> &mut Context {
    &mut w.combat.ai.ctx
}

/// `DecisionContext.InitUnit(unit)` (its `Self` list isn't kept: no supported action targets
/// itself).
fn init_unit(w: &mut World, u: EntityIdx) {
    let pf = is_player_faction(w, u);
    let c = ctx(w);
    c.unit = Some(u);
    c.target = None;
    c.action_range = 0.0;
    c.best = Decimal::ZERO;
    c.best_path = None;
    c.cur = Decimal::ZERO;
    c.current_path = None;
    c.current_five = false;
    c.is_player_faction = pf;
    c.stable.clear();
    if c.enemies_init {
        for t in &mut c.enemies {
            t.los = None;
        }
    }
}

/// `ReleaseUnit`.
fn release_unit<I: AiInputs>(w: &mut World, inp: &mut I) -> R<()> {
    let c = ctx(w);
    c.enemies_sorted = false;
    if c.personal {
        // `ReleaseTargets(Enemies)`: the getter sorts the list again first.
        enemies(w, inp)?;
        let c = ctx(w);
        c.enemies.clear();
        c.personal = false;
        c.enemies_init = false;
    }
    let c = ctx(w);
    c.unit = None;
    c.action = None;
    c.is_player_faction = false;
    Ok(())
}

/// `ReleaseGroup`.
fn release_group(w: &mut World) {
    let c = ctx(w);
    if c.enemies_init {
        c.enemies.clear();
        c.enemies_init = false;
    }
    c.memory_enemies = None;
}

fn ctx_unit(w: &World) -> R<EntityIdx> {
    match w.combat.ai.ctx.unit {
        Some(u) => Ok(u),
        None => desync("the AI's context has no unit"),
    }
}

/// `DecisionContext.Enemies` (`CalculateEnemies(forDead: false, ...)`).
fn enemies<I: AiInputs>(w: &mut World, inp: &mut I) -> R<()> {
    let me = ctx_unit(w)?;
    let me_id = uid(w, me);
    if !w.combat.ai.ctx.enemies_init {
        if stable_bool(w, inp, "pe", &me_id)? {
            return unsupported("a personal enemy (UnitPartPersonalEnemy)");
        }
        let mut list = Vec::new();
        let pf = is_player_faction(w, me);
        let memory = inp.ai_enemies(w)?;
        ctx(w).memory_enemies = Some(memory.len());
        for id in memory {
            let u = unit_of(w, &id)?;
            if stable_bool(w, inp, "pe", &id)? {
                ctx(w).personal = true;
            }
            if super::units::is_dead(w, u)
                || inp.leaf(w, "untgt", Of::Pair(Some(me_id.clone()), Some(id.clone())))?
                || (pf && super::units::is_conscious(w, u) && !inp.ai_bool(w, "hpcm", None)?)
            {
                continue;
            }
            if is_player_faction(w, u) && has_aggro_filter(w, me) {
                return unsupported("UnitAggroFilter");
            }
            let distance = update_distance(w, inp, me, u)?;
            list.push(TargetInfo { unit: u, id, distance, weight: 0.0, los: None });
        }
        let manual = stable_unit(w, inp, "manual", &me_id)?;
        if pf && let Some(m) = manual {
            let mu = unit_of(w, &m)?;
            if !super::units::is_dead(w, mu)
                && !inp.leaf(w, "untgt", Of::Pair(Some(me_id.clone()), Some(m.clone())))?
                && !list.iter().any(|t| t.unit == mu)
            {
                let distance = update_distance(w, inp, me, mu)?;
                list.push(TargetInfo { unit: mu, id: m, distance, weight: 0.0, los: None });
            }
        }
        ctx(w).enemies = list;
        sort(w, inp)?;
        let c = ctx(w);
        c.enemies_init = true;
        c.enemies_sorted = true;
    } else if !w.combat.ai.ctx.enemies_sorted {
        for i in 0..w.combat.ai.ctx.enemies.len() {
            let u = w.combat.ai.ctx.enemies[i].unit;
            let d = update_distance(w, inp, me, u)?;
            ctx(w).enemies[i].distance = d;
        }
        sort(w, inp)?;
        ctx(w).enemies_sorted = true;
    }
    Ok(())
}

fn has_aggro_filter(w: &World, u: EntityIdx) -> bool {
    w.unit(u).and_then(|x| x.blueprint).and_then(|g| w.content.get(g)).is_some_and(|bp| {
        bp.components_array().iter().any(|c| crate::content::View(c).is("UnitAggroFilter"))
    })
}

/// `TargetInfo.UpdateDistance`: 0 for the context's own unit.
fn update_distance<I: AiInputs>(w: &mut World, inp: &mut I, me: EntityIdx, u: EntityIdx) -> R<f32> {
    if me == u {
        return Ok(0.0);
    }
    let (a, b) = (uid(w, me), uid(w, u));
    inp.ai_dist(w, &a, &b)
}

/// `TargetInfo.Sort`: each target's weight, then `List.Sort` by weight, descending (Mono's
/// introsort: an insertion sort up to 16 targets, whatever the list's capacity).
fn sort<I: AiInputs>(w: &mut World, inp: &mut I) -> R<()> {
    for i in 0..w.combat.ai.ctx.enemies.len() {
        fill_weight(w, inp, i)?;
    }
    let list = &mut ctx(w).enemies;
    if list.len() > 16 {
        return unsupported("sorting more than 16 AI targets (the list's capacity decides)");
    }
    let cap = crate::csharp::sort::grown_capacity(list.len());
    // `CompareTo(other)`: `other.Weight.CompareTo(Weight)` (`float.CompareTo`: NaN first).
    crate::csharp::introsort(list, cap, &mut |a: &TargetInfo, b: &TargetInfo| {
        float_compare(b.weight, a.weight)
    });
    Ok(())
}

/// `float.CompareTo(value)`.
fn float_compare(a: f32, b: f32) -> i32 {
    if a < b {
        -1
    } else if a > b {
        1
    } else if a == b {
        0
    } else if a.is_nan() {
        if b.is_nan() { 0 } else { -1 }
    } else {
        1
    }
}

/// `TargetInfo.FillWeight`.
fn fill_weight<I: AiInputs>(w: &mut World, inp: &mut I, i: usize) -> R<()> {
    let me = ctx_unit(w)?;
    let me_id = uid(w, me);
    let t = w.combat.ai.ctx.enemies[i].clone();
    let mut weight = 0.0f32;
    let saddled = inp.saddled(w, Some(&t.id))?;
    unit_opt(w, saddled.as_deref())?;
    if saddled.is_some() {
        weight = -1_000_000.0;
    }
    // `Unit == ManualTarget || Unit.GetRider() == ManualTarget` (`UnitEntityData ==`: two nulls
    // are equal).
    let manual = stable_unit(w, inp, "manual", &me_id)?;
    let hit = manual.as_deref() == Some(t.id.as_str()) || {
        let rider = stable_unit(w, inp, "rider", &t.id)?;
        let manual = stable_unit(w, inp, "manual", &me_id)?;
        rider == manual
    };
    if hit {
        weight = store_f32(f64::from(weight) + 1_000_000.0);
    }
    let range = w.combat.ai.ctx.action_range;
    let c1 = stable_float(w, inp, "corp", &me_id)?;
    let c2 = stable_float(w, inp, "corp", &t.id)?;
    if f64::from(t.distance) <= f64::from(range) + f64::from(c1) + f64::from(c2) {
        if stable_unit(w, inp, "last", &me_id)?.as_deref() == Some(t.id.as_str()) {
            weight = store_f32(f64::from(weight) + 1.0);
        }
    } else if !turn_based(w) {
        weight = store_f32(f64::from(weight) + f64::from(t.distance) * f64::from(-0.001f32));
    }
    ctx(w).enemies[i].weight = weight;
    Ok(())
}

/// `ActionRangeWithCorpulence` (the target must be a unit).
fn action_range_with_corpulence<I: AiInputs>(w: &mut World, inp: &mut I) -> R<f32> {
    let me = ctx_unit(w)?;
    let Some(t) = w.combat.ai.ctx.target else {
        return unsupported("ActionRangeWithCorpulence without a target");
    };
    let tid = w.combat.ai.ctx.enemies[t].id.clone();
    let range = w.combat.ai.ctx.action_range;
    let me_id = uid(w, me);
    let c1 = stable_float(w, inp, "corp", &me_id)?;
    let c2 = stable_float(w, inp, "corp", &tid)?;
    Ok(store_f32(f64::from(range) + f64::from(c1) + f64::from(c2)))
}

// ---- the decision ----

/// `SelectAction(unit, Context, debugScope)`.
fn select_action<I: AiInputs>(w: &mut World, inp: &mut I, u: EntityIdx, brain: &Brain) -> R<()> {
    let id = uid(w, u);
    let (best, target) = find_best_action(w, inp, u, brain)?;
    let set = unit_set(w, &id)?;
    let Some(best) = best else {
        uc::interrupt_ai_commands(w, inp, set)?;
        let nct = w.combat.ai.nct.get(&u).copied();
        return inp.ai_selected(w, &id, false, nct);
    };
    let Some(target) = target else { return desync("a best AI action without a target") };
    let target_id = uid(w, target);
    if !turn_based(w) {
        // `NonMoveTo`: Standard ?? Swift ?? Free ?? Move, not a `UnitMoveTo`.
        let slots = w.combat.commands.sets[&set].slots;
        let non_move = [uc::ty::STANDARD, uc::ty::SWIFT, uc::ty::FREE, uc::ty::MOVE]
            .into_iter()
            .find_map(|t| slots[usize::from(t)])
            .filter(|c| w.combat.commands.cmds[c].class != uc::class::MOVE_TO);
        if let Some(c) = non_move {
            let x = &w.combat.commands.cmds[&c];
            let (is_attack, same_target) =
                (x.class == uc::class::ATTACK, x.s.target.unit() == Some(target_id.as_str()));
            let same_action = match (x.ai, w.combat.ai.commands.get(&c)) {
                (false, _) => false,
                (true, Some(a)) => a.action == best.guid,
                (true, None) => return unsupported("an AI command the sim's AI didn't make"),
            };
            if same_action && same_target {
                // `CanKeepCommand`: a `UnitAttack` and no auto-use ability.
                let auto = inp.ai_blueprint(w, "auto", &id)?;
                if auto.is_some() {
                    return unsupported("an auto-use ability");
                }
                if !is_attack {
                    return unsupported("CanKeepCommand of a command other than UnitAttack");
                }
                ctx(w).best_path = None;
                let nct = w.combat.ai.nct.get(&u).copied();
                return inp.ai_selected(w, &id, true, nct);
            }
        }
    }
    init_action(w, inp, &best)?;
    // `BlueprintAiAttack.CreateCommand`: no auto-use ability; `UnitAttack.CreateAttackCommand`:
    // no magus' spell before the attack.
    if inp.ai_blueprint(w, "auto", &id)?.is_some() {
        return unsupported("an auto-use ability");
    }
    if inp.ai_bool(w, "magus", Some(&id))? {
        return unsupported("UnitAttack.CreateAttackCommand for a magus");
    }
    let path = ctx(w).best_path.take();
    let five = w.combat.ai.ctx.best_five.unwrap_or(false);
    // `NextCommandTime = Time.time`; a `UnitAttack` is a standard action:
    // `NextCommandTime += 0.3f + Random.Range(0f, 0.1f)`.
    let time = inp.ai_float(w, "time", None)?;
    inp.ai_nct_write(w, &id, time)?;
    let r = inp.ai_draw(w)?;
    let nct = store_f32(f64::from(time) + (f64::from(0.3f32) + f64::from(r)));
    inp.ai_nct_write(w, &id, nct)?;
    w.combat.ai.nct.insert(u, nct);
    let spec = NewCommand { class: uc::class::ATTACK, ty: uc::ty::STANDARD, target: target_id };
    let c = inp.ai_new_run(w, set, &spec)?;
    w.combat
        .ai
        .commands
        .insert(c, AiCommand { action: best.guid, forced_path: path.map(|p| p.points), five });
    uc::run(w, inp, set, c, false, false)?;
    inp.nested_run_end(w, set, c)?;
    // (`Run`'s `UpdateCombatTarget` may change the unit's targets.)
    ctx(w).stable.retain(|(f, _, _)| *f != "manual" && *f != "last");
    inp.ai_selected(w, &id, true, Some(nct))
}

/// `FindBestAction`: the best action and target.
fn find_best_action<I: AiInputs>(
    w: &mut World,
    inp: &mut I,
    u: EntityIdx,
    brain: &Brain,
) -> R<(Option<Action>, Option<EntityIdx>)> {
    let id = uid(w, u);
    if inp.ai_bool(w, "conf", Some(&id))? {
        return unsupported("a confused unit's AI (UnitPartConfusion)");
    }
    if is_player_faction(w, u) && inp.ai_blueprint(w, "auto", &id)?.is_some() {
        return unsupported("an auto-use ability");
    }
    if brain.custom > 0 {
        return unsupported("an AI's custom actions");
    }
    let mut best: Option<Action> = None;
    let mut target = None;
    // `bestAction == null || context.CurrentScore < 0.1m`.
    if best.is_none() || w.combat.ai.ctx.cur < Decimal::lit(1, 1) {
        for g in &brain.available {
            let Some(g) = g else { return unsupported("an AI action other than AiActionSimple") };
            let a = action(w, *g)?;
            if w.combat.ai.ctx.best > dec(a.base_score)? {
                continue;
            }
            calculate_action_score(w, inp, &a, &mut best, &mut target)?;
        }
    }
    let c = &w.combat.ai.ctx;
    let check = BestCheck {
        unit: id,
        action: best.as_ref().map(|a| a.guid),
        target: target.map(|t| uid(w, t)),
        best: c.best,
        cur: c.cur,
        five: c.best_five,
        path: c.best_path.as_ref().map(|p| p.points.clone()),
    };
    inp.ai_best(w, &check)?;
    Ok((best, target))
}

/// `DecisionContext.InitAction(action)`.
fn init_action<I: AiInputs>(w: &mut World, inp: &mut I, a: &Action) -> R<()> {
    // `InitContext`: nothing for an attack; `GetRange`: the weapon's reach (an input);
    // `GetRadius`: 0.
    let range = inp.ai_range(w, a.guid)?;
    stable(w, "range", &a.guid.to_string(), Stable::Float(range.to_bits()))?;
    let base = dec(a.base_score)?;
    let c = ctx(w);
    c.action = Some(a.guid);
    c.action_range = range;
    c.cur = base;
    c.target = None;
    Ok(())
}

/// `CalculateActionScore`.
fn calculate_action_score<I: AiInputs>(
    w: &mut World,
    inp: &mut I,
    a: &Action,
    best: &mut Option<Action>,
    target: &mut Option<EntityIdx>,
) -> R<()> {
    init_action(w, inp, a)?;
    score_actor(w, inp, a)?;
    if w.combat.ai.ctx.cur <= w.combat.ai.ctx.best {
        return Ok(());
    }
    let actor_score = w.combat.ai.ctx.cur;
    // `BlueprintAiAttack`: enemies, alive only.
    enemies(w, inp)?;
    calculate_targets_score(w, inp, a, actor_score, best, target)?;
    Ok(())
}

/// `ScoreActor` (with `IsActorValidInternal`; no cooldowns or combat counts: [`action`]).
fn score_actor<I: AiInputs>(w: &mut World, inp: &mut I, a: &Action) -> R<()> {
    let me = ctx_unit(w)?;
    let id = uid(w, me);
    // `BlueprintAiAction.IsActorValid`.
    let valid = 'v: {
        if !stable_bool(w, inp, "aien", &id)? && !a.use_when_disabled {
            break 'v false;
        }
        if w.combat.ai.limited
            && is_player_faction(w, me)
            && inp.leaf(w, "ctrl", Of::Unit(Some(id.clone())))?
            && !a.use_on_limited
        {
            break 'v false;
        }
        // `(!Invert && CompareTo(MinDifficulty) < 0) || (Invert && CompareTo(MinDifficulty) > 0)`:
        // one comparison either way.
        let cmp = inp.ai_diff(w, a.min_difficulty)?;
        stable(w, "diff", &a.min_difficulty.to_string(), Stable::Int(cmp))?;
        if (!a.invert_difficulty && cmp < 0) || (a.invert_difficulty && cmp > 0) {
            break 'v false;
        }
        // `IsAvailable` and `IsExpended`: an attack's are true and false.
        true
    };
    if valid {
        score_considerations(w, inp, &a.actor)?;
        // `ScoreActor`'s full-round check is for abilities only.
    } else {
        ctx(w).cur = Decimal::ZERO;
    }
    let c = &w.combat.ai.ctx;
    let check = ScoreCheck {
        f: "ScoreActor",
        action: a.guid,
        target: None,
        score: c.cur,
        best: c.best,
        path: None,
    };
    inp.ai_score(w, &check)
}

/// `ScoreConsiderations(context, considerations)`.
fn score_considerations<I: AiInputs>(w: &mut World, inp: &mut I, list: &[Guid]) -> R<()> {
    for g in list {
        if w.combat.ai.ctx.cur <= w.combat.ai.ctx.best {
            break;
        }
        let num = consideration(w, inp, *g)?;
        let c = &mut w.combat.ai.ctx;
        c.cur = mul(c.cur, dec(num)?)?;
    }
    Ok(())
}

/// A consideration's `Score(context)`.
fn consideration<I: AiInputs>(w: &mut World, inp: &mut I, g: Guid) -> R<f32> {
    let Some(bp) = w.content.get(g) else {
        return desync(format!("the AI consideration blueprint {g} is unknown"));
    };
    let v = bp.view();
    let t = w.combat.ai.ctx.target.map(|i| w.combat.ai.ctx.enemies[i].clone());
    let me = ctx_unit(w)?;
    // `Target.Unit ?? Unit`.
    let subject = t.as_ref().map_or(me, |t| t.unit);
    match bp.type_name() {
        "ComplexConsideration" => {
            for c in v.guids_with_nulls("m_Considerations") {
                let Some(c) = c else {
                    return desync("a null consideration in a ComplexConsideration");
                };
                let num = consideration(w, inp, c)?;
                let x = &mut w.combat.ai.ctx;
                x.cur = mul(x.cur, dec(num)?)?;
                if x.cur <= x.best {
                    return Ok(0.0);
                }
            }
            Ok(1.0)
        }
        "InRangeConsideration" => {
            let Some(t) = t else { return desync("InRangeConsideration without a target") };
            if v.bool("OnlyIfThreated") {
                return unsupported("InRangeConsideration.OnlyIfThreated");
            }
            let range = action_range_with_corpulence(w, inp)?;
            Ok(if t.distance <= range { v.f32("InRangeScore") } else { v.f32("OutOfRangeScore") })
        }
        "LifeStateConsideration" => Ok(match w.unit(subject).map_or(-1, |x| x.life_state) {
            0 => v.f32("AliveScore"),
            1 => v.f32("UnconsciousScore"),
            2 => v.f32("DeadScore"),
            _ => 0.0,
        }),
        "FactConsideration" => {
            let has = v.guids("m_Fact").into_iter().any(|f| {
                w.entity(subject).facts.list.iter().any(|x| w.fact(*x).blueprint == Some(f))
            });
            Ok(if has { v.f32("HasFactScore") } else { v.f32("NoFactScore") })
        }
        "NotImpatientConsideration" => Ok(1.0),
        other => unsupported(&format!("the AI consideration {other}")),
    }
}

/// `CalculateTargetsScore(context, action, Enemies, actorScore, ...)`.
fn calculate_targets_score<I: AiInputs>(
    w: &mut World,
    inp: &mut I,
    a: &Action,
    actor_score: Decimal,
    best: &mut Option<Action>,
    target: &mut Option<EntityIdx>,
) -> R<()> {
    let me = ctx_unit(w)?;
    let me_id = uid(w, me);
    for i in 0..w.combat.ai.ctx.enemies.len() {
        let t = w.combat.ai.ctx.enemies[i].clone();
        if t.weight <= -1_000_000.0 {
            continue;
        }
        let c = ctx(w);
        c.cur = actor_score;
        c.target = Some(i);
        score_target(w, inp, a)?;
        if w.combat.ai.ctx.cur > Decimal::ZERO && turn_based(w) {
            score_path(w, inp, a)?;
        }
        let mut not_priority = false;
        let ptf = inp.ai_blueprint(w, "ptf", &me_id)?;
        stable(w, "ptf", &me_id, Stable::Blueprint(ptf))?;
        if let Some(f) = ptf
            && !w.entity(t.unit).facts.list.iter().any(|x| w.fact(*x).blueprint == Some(f))
        {
            not_priority = true;
            let c = ctx(w);
            c.cur = mul(c.cur, Decimal::lit(1, 8))?;
        }
        // `IsAffectedByImpatience`: an attack is.
        if !w.combat.ai.ctx.is_player_faction && !not_priority {
            if stable_bool(w, inp, "disliked", &t.id)? {
                let c = ctx(w);
                c.cur = mul(c.cur, Decimal::lit(1, 2))?;
            }
            let ipt = inp.ai_unit(w, "ipt", None)?;
            stable(w, "ipt", &me_id, Stable::Unit(ipt.clone()))?;
            if let Some(p) = ipt {
                // The second read of `ImpatiencePriorityTarget`.
                let again = inp.ai_unit(w, "ipt", None)?;
                stable(w, "ipt", &me_id, Stable::Unit(again.clone()))?;
                if again.as_deref() != Some(t.id.as_str()) {
                    let c = ctx(w);
                    c.cur = mul(c.cur, Decimal::lit(1, 4))?;
                }
                let _ = p;
            }
        }
        let list = &w.combat.ai.ctx.enemies;
        let sleeping =
            list.iter().filter(|x| crate::unit_state::has_condition(w, x.unit, SLEEPING)).count();
        if sleeping < list.len() && crate::unit_state::has_condition(w, t.unit, SLEEPING) {
            let c = ctx(w);
            c.cur = mul(c.cur, Decimal::lit(1, 3))?;
        }
        let c = ctx(w);
        if c.cur > c.best {
            c.best = c.cur;
            c.best_path = c.current_path.take();
            c.best_five = Some(c.current_five);
            *best = Some(a.clone());
            *target = Some(t.unit);
            let d = c.best.checked_sub(actor_score).map_err(|e| Desync(format!("{e}")))?;
            if d.abs() < Decimal::lit(1, 6) {
                break;
            }
        }
    }
    Ok(())
}

/// `ScoreTarget` (with `IsTargetValidInternal`).
fn score_target<I: AiInputs>(w: &mut World, inp: &mut I, a: &Action) -> R<()> {
    let me = ctx_unit(w)?;
    let me_id = uid(w, me);
    let Some(i) = w.combat.ai.ctx.target else { return desync("ScoreTarget without a target") };
    let t = w.combat.ai.ctx.enemies[i].clone();
    let valid = 'v: {
        if stable_bool(w, inp, "amf", &me_id)? || stable_bool(w, inp, "unreach", &t.id)? {
            let range = action_range_with_corpulence(w, inp)?;
            if t.distance > range {
                break 'v false;
            }
            if !in_los(w, inp, i)? {
                break 'v false;
            }
        }
        // `IsTargetValid`: an attack's is true.
        true
    };
    if valid {
        // `DestinationPoint` (the target's position) feeds the movement leaves only.
        score_considerations(w, inp, &a.target)?;
    } else {
        ctx(w).cur = Decimal::ZERO;
    }
    let c = &w.combat.ai.ctx;
    let check = ScoreCheck {
        f: "ScoreTarget",
        action: a.guid,
        target: Some(t.id),
        score: c.cur,
        best: c.best,
        path: None,
    };
    inp.ai_score(w, &check)
}

/// `TargetInfo.InLos`: cached until the next `InitUnit`.
fn in_los<I: AiInputs>(w: &mut World, inp: &mut I, i: usize) -> R<bool> {
    if let Some(v) = w.combat.ai.ctx.enemies[i].los {
        return Ok(v);
    }
    let v = !inp.ai_bool(w, "los", None)?;
    ctx(w).enemies[i].los = Some(v);
    Ok(v)
}

/// `AiAction.ScorePath`.
fn score_path<I: AiInputs>(w: &mut World, inp: &mut I, a: &Action) -> R<()> {
    let me = ctx_unit(w)?;
    let me_id = uid(w, me);
    let Some(i) = w.combat.ai.ctx.target else { return desync("ScorePath without a target") };
    let t = w.combat.ai.ctx.enemies[i].clone();
    if t.unit == me {
        return score_path_end(w, inp, a, &t.id);
    }
    // `CurrentTurn?.GetRemainingMovementRange(unit, total: true, singleActionMove: false)
    //  ?? CurrentSpeedMps * 6f`.
    let num = if uc::current_turn(w).is_some() {
        let v = inp.ai_movement_range(w, &me_id)?;
        stable(w, "mrange", &me_id, Stable::Float(v.to_bits()))?;
        v
    } else {
        let s = stable_float(w, inp, "speed", &me_id)?;
        store_f32(f64::from(s) * 6.0)
    };
    // `FindPath(destination, ..., ActionRangeWithCorpulence, needLOS: true)`, then
    // `FixPathIfTooShort` (an input: the path it leaves).
    action_range_with_corpulence(w, inp)?;
    let path = inp.ai_path(w)?;
    let failed = path.error || path.state != PATH_COMPLETE;
    ctx(w).current_path = Some(path.clone());
    if failed {
        ctx(w).cur = Decimal::lit(0, 1);
        return score_path_end(w, inp, a, &t.id);
    }
    let Some(&last) = path.points.last() else { return desync("a complete path without points") };
    let radius = action_range_with_corpulence(w, inp)?;
    if inp.ai_close(w, radius)? {
        return score_path_end(w, inp, a, &t.id);
    }
    if num <= 0.0 {
        ctx(w).cur = Decimal::lit(0, 1);
        return score_path_end(w, inp, a, &t.id);
    }
    // `(vectorPath[last] - destinationPoint).magnitude > ActionRangeWithCorpulence + 0.5f`.
    let mag = inp.ai_float(w, "mag", None)?;
    let range = action_range_with_corpulence(w, inp)?;
    if f64::from(mag) > f64::from(range) + 0.5 {
        let c = ctx(w);
        c.cur = mul(c.cur, Decimal::lit(1, 5))?;
        return score_path_end(w, inp, a, &t.id);
    }
    // `Path.GetTotalLength()` over the path's points (`Vector3.Distance` each, summed).
    let total = inp.ai_float(w, "plen", None)?;
    if total.to_bits() != path_length(&path.points).to_bits() {
        return desync(format!(
            "ScorePath: the path's length {total} isn't its points' {}",
            path_length(&path.points)
        ));
    }
    let mut num2 = 1.0f32;
    let mut five = false;
    if total > num {
        num2 = store_f32(f64::from(0.75f32) - f64::from(total) * f64::from(0.005f32));
    } else if total <= remaining_five_foot(w, inp, &me_id)? && {
        // `HasFiveFootStep`: false when the sim's movement stats restrict it whatever the speed.
        let h5 = stable_bool(w, inp, "h5", &me_id)?;
        if h5 && super::cooldowns::five_foot_step_ruled_out(w) {
            return desync(format!("{me_id}'s HasFiveFootStep, ruled out by its movement"));
        }
        h5
    } {
        // (`(totalLength > num) & flag2` can't hold here.)
        five = true;
        // `HasFullRoundAction()`: the sim's cooldowns (the recorded value checked).
        let full = inp.ai_bool(w, "hfra", Some(&me_id))?;
        if full != super::cooldowns::has_full_round_action(w, me) {
            return desync(format!(
                "{me_id}'s HasFullRoundAction: the sim's differs from the recorded {full}"
            ));
        }
        if full {
            num2 = 1.1;
        }
    }
    ctx(w).current_five = five;
    // `new MovementSimulator(Position, CurrentPath, destination, ActionRangeWithCorpulence, ...)`.
    action_range_with_corpulence(w, inp)?;
    let mut aoo = 0i64;
    let mut remaining = num;
    let mut position = [0.0f32; 3];
    let mut kept = 0i32;
    let mut free;
    // `position` starts as the unit's own (not established: reported if no step is free).
    let mut any_free = false;
    // The simulator's position and `LastMoveLength` after the previous step (its first position,
    // the unit's, isn't established).
    let mut prev: Option<([f32; 3], f32)> = None;
    // `new MovementSimulator(...)`: waypoint 1 of a path of two points or more (`FixPathIfTooShort`
    // leaves at least two); a step keeps it or advances it once.
    let mut waypoint = 1i32;
    // `GetNearest(p).node.AllyBlocked` per position read (one node per position: nothing updates
    // the graph during `ScorePath`).
    let mut blocked: Vec<([u32; 3], i64)> = Vec::new();
    loop {
        let step = inp.ai_sim_step(w, remaining)?;
        let len = w.combat.ai.ctx.current_path.as_ref().map_or(0, |p| p.points.len());
        // `AdvanceWaypoint` stops at the last point.
        if step.last_move_length < 0.0 || usize::try_from(step.waypoint).map_or(true, |i| i >= len)
        {
            return desync(
                "a movement simulation step with a negative length or a waypoint off the path",
            );
        }
        // A step sets `Position` once (`LastMoveLength = (old - new).To2D().magnitude`) or not at
        // all (the length unchanged).
        if let Some((p, l)) = prev {
            let moved = horizontal_distance(p, step.position);
            let ok = if p.map(f32::to_bits) == step.position.map(f32::to_bits) {
                // (A step that goes on has set `Position`: length 0.)
                step.last_move_length == 0.0
                    || (!step.ret && step.last_move_length.to_bits() == l.to_bits())
            } else {
                step.last_move_length.to_bits() == moved.to_bits()
            };
            if !ok {
                return desync("a movement simulation step whose length isn't its move's");
            }
        }
        prev = Some((step.position, step.last_move_length));
        if step.waypoint != waypoint && step.waypoint != waypoint + 1 {
            return desync(format!(
                "a movement simulation step from waypoint {waypoint} to {}",
                step.waypoint
            ));
        }
        // `ReachedMaxDistance` of a step that went on: `maxMoveLength <= 0` after advancing a
        // waypoint, else `LastMoveLength >= maxMoveLength`.
        if step.ret {
            let rmd = if step.waypoint == waypoint + 1 {
                remaining <= 0.0
            } else {
                step.last_move_length >= remaining
            };
            if rmd != step.reached_max {
                return desync("a movement simulation step's ReachedMaxDistance isn't its move's");
            }
        }
        waypoint = step.waypoint;
        let stop = !step.ret;
        if blocked_at(w, inp, &mut blocked, step.position)? <= 0 {
            position = step.position;
            kept = if step.waypoint > 0 { step.waypoint - 1 } else { step.waypoint };
            free = true;
            any_free = true;
        } else {
            free = false;
        }
        if stop {
            break;
        }
        remaining = store_f32(f64::from(remaining) - f64::from(step.last_move_length));
        if step.reached_max || remaining < 0.0001 {
            break;
        }
        if !five {
            aoo += inp.ai_aoo(w)?;
            // `EngagementSimulator` counts each of the memory's enemies once at most.
            if w.combat.ai.ctx.memory_enemies.is_some_and(|n| aoo > n as i64) {
                return desync("more simulated attacks of opportunity than the memory has enemies");
            }
        }
    }
    if !free && !any_free {
        return unsupported(
            "ScorePath: no free simulation step (the path ends at the unit's position)",
        );
    }
    if !free {
        // `RemoveRange(num6 + 1, Count - num6 - 1)`, then the last free position.
        let Some(p) = ctx(w).current_path.as_mut() else { return desync("no current path") };
        let keep = usize::try_from(kept + 1).map_err(|_| Desync("a negative waypoint".into()))?;
        if keep > p.points.len() {
            return desync("a path truncated past its end");
        }
        p.points.truncate(keep);
        p.points.push(position);
    }
    #[allow(clippy::cast_precision_loss)]
    let aoo_f = aoo as f32;
    num2 = store_f32(f64::from(num2) - f64::from(aoo_f) * f64::from(0.1f32));
    if num2 < 0.1 {
        num2 = 0.1;
    }
    // `GetNearest(vector)`: the path's last point, or the last free position it was cut to.
    let end = if free { last } else { position };
    if blocked_at(w, inp, &mut blocked, end)? > 0 {
        ctx(w).cur = Decimal::ZERO;
    } else {
        let c = ctx(w);
        c.cur = mul(c.cur, dec(num2)?)?;
    }
    score_path_end(w, inp, a, &t.id)
}

/// An `AllyBlocked` read at `p`, the same as an earlier read at the same position.
fn blocked_at<I: AiInputs>(
    w: &mut World,
    inp: &mut I,
    seen: &mut Vec<([u32; 3], i64)>,
    p: [f32; 3],
) -> R<i64> {
    let v = inp.ai_blocked(w)?;
    let key = p.map(f32::to_bits);
    match seen.iter().find(|(k, _)| *k == key) {
        Some((_, x)) if *x != v => desync(format!("AllyBlocked at {p:?} read {x}, then {v}")),
        Some(_) => Ok(v),
        None => {
            seen.push((key, v));
            Ok(v)
        }
    }
}

/// `(a - b).To2D().magnitude`: the differences stored as `float`s, the square root in double.
fn horizontal_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dx = store_f32(f64::from(a[0]) - f64::from(b[0]));
    let dz = store_f32(f64::from(a[2]) - f64::from(b[2]));
    store_f32((f64::from(dx) * f64::from(dx) + f64::from(dz) * f64::from(dz)).sqrt())
}

/// `Path.GetTotalLength()`: `Vector3.Distance` between consecutive points, summed (each
/// difference stored as a `float`, the square root in double).
fn path_length(points: &[[f32; 3]]) -> f32 {
    let mut total = 0.0f32;
    for p in points.windows(2) {
        let d = [0, 1, 2].map(|i| store_f32(f64::from(p[0][i]) - f64::from(p[1][i])));
        let sq = d.iter().map(|x| f64::from(*x) * f64::from(*x)).sum::<f64>();
        total = store_f32(f64::from(total) + f64::from(store_f32(sq.sqrt())));
    }
    total
}

/// `GetRemainingFiveFootStepRange(unit)` of the current turn's rider: `MetersOfFiveFootStep -
/// MetersMovedByFiveFootStep`, the sim's (the recorded value checked).
fn remaining_five_foot<I: AiInputs>(w: &mut World, inp: &mut I, id: &str) -> R<f32> {
    let v = inp.ai_float(w, "r5", Some(id))?;
    let moved = w.combat.turns.movement.meters_moved_by_five_foot_step;
    let sim = store_f32(f64::from(super::cooldowns::METERS_OF_FIVE_FOOT_STEP) - f64::from(moved));
    if sim.to_bits() != v.to_bits() {
        return desync(format!("the remaining five-foot step: the sim's {sim}, recorded {v}"));
    }
    Ok(v)
}

fn score_path_end<I: AiInputs>(w: &mut World, inp: &mut I, a: &Action, target: &str) -> R<()> {
    let c = &w.combat.ai.ctx;
    let check = ScoreCheck {
        f: "ScorePath",
        action: a.guid,
        target: Some(target.to_owned()),
        score: c.cur,
        best: c.best,
        path: Some((c.current_five, c.current_path.as_ref().map(|p| p.points.clone()))),
    };
    inp.ai_score(w, &check)
}

#[cfg(test)]
mod tests;
