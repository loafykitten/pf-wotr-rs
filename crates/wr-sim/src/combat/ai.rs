//! The AI (decompiled, not copied: `Kingmaker.AI`): `AiBrainController` (`Tick`, `ForceTick`,
//! `TickBrain`, `SelectAction`, `FindBestAction`, `CalculateActionScore`,
//! `CalculateTargetsScore`), the static `DecisionContext` it decides in, `TargetInfo`,
//! `AiAction`'s scoring (`ScoreActor`, `ScoreTarget`, `ScorePath`), `AiActionSimple`,
//! `BlueprintAiAction`, `BlueprintAiAttack` and `BlueprintAiCastSpell` (an ability fact's cast at
//! an enemy: docs/poc5/ai-abilities.md), and the considerations the fights use
//! (`ComplexConsideration`, `InRangeConsideration`, `LifeStateConsideration`,
//! `FactConsideration`, `NotImpatientConsideration`, `ThreatedByConsideration`,
//! `DistanceConsideration`). docs/poc5/commands.md, slice D.
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
use super::spells::{self, Ability};
use crate::content::is_a;
use crate::csharp::Decimal;
use crate::csharp::arith::{clamp01, store_f32};
use crate::world::{EntityIdx, FactIdx, World};

/// `UnitBrain` as saved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Brain {
    /// `Blueprint`.
    pub blueprint: Option<Guid>,
    /// `AvailableActions`, in order: each `AiActionSimple`'s blueprint (`None` for another kind
    /// of action, one the save doesn't name, or a cast spell whose place in the list the sim
    /// can't establish).
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
    fn ai_blocked(&mut self, w: &mut World, p: [f32; 3]) -> R<i64>;
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

/// A command `SelectAction` creates (`BlueprintAiAttack.CreateCommand`: `new UnitAttack(target)`;
/// `BlueprintAiCastSpell.CreateCommand`: `new UnitUseAbility(RuntimeActionType, ability, target)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewCommand {
    pub class: &'static str,
    pub ty: u8,
    pub target: String,
    /// The constructor's `NeedLoS` (a `UnitUseAbility`'s: no `ILineOfSightIgnore` component).
    pub los: bool,
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
    /// `BlueprintAiCastSpell.Data.AbilityData` of each unit's cast action (by its blueprint):
    /// the ability fact `InitContext` found the first time, kept until `ClearCombatInfo`
    /// (leaving combat). `ActionData` isn't saved and a new brain's actions are new, so every
    /// cache is empty at the load.
    pub cast_data: std::collections::HashMap<(EntityIdx, Guid), FactIdx>,
}

/// `UnitCombatState.Clear()`'s `Brain.ClearCombatInfo()`: the unit's cast actions forget their
/// `AbilityData`.
pub fn clear_combat_info(w: &mut World, u: EntityIdx) {
    w.combat.ai.cast_data.retain(|(x, _), _| *x != u);
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
    /// `Ability`: the action's `AbilityData` (a cast action's, from `InitContext`).
    ability: Option<Ability>,
    /// `ExpendedActions` (by blueprint), cleared by `InitUnit`.
    expended: Vec<Guid>,
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
/// when `ShouldBeInActionsList` ([`should_be_in_actions_list`]: kept as `None` where the sim can't
/// tell, reported when the AI reaches it); then sorted by base score, descending: `List.Sort`'s
/// introsort over a list grown by `Add`). `owner`'s facts are the ones `Initialize` added before.
#[must_use]
pub fn new_brain(w: &mut World, owner: EntityIdx, brain: Option<Guid>) -> Brain {
    let actions: Vec<Guid> = brain
        .and_then(|g| w.content.get(g))
        .map(|b| b.view().guids("m_Actions"))
        .unwrap_or_default();
    // `ShouldBeInActionsList`'s `GetAbilityForUse` makes the fact's `AbilityData` here: a random
    // variant draws now, whether or not the unit ever decides.
    let draws = actions.iter().any(|g| {
        w.content.get(*g).is_some_and(|b| {
            let v = b.view();
            b.type_name() == "BlueprintAiCastSpell"
                && v.bool("m_RandomVariant")
                && v.guid("m_Ability").is_some_and(|a| spells::ability_fact(w, owner, a).is_some())
        })
    });
    if draws {
        w.coverage.unsupported(
            "UnitBrain.RestoreAvailableActions: a cast action's random variant (CreateAbilityData draws)",
        );
    }
    let w: &World = w;
    let mut list: Vec<(Option<Guid>, f32)> = actions
        .into_iter()
        .filter_map(|g| {
            let cast = w.content.get(g).is_some_and(|b| b.type_name() == "BlueprintAiCastSpell");
            let listed = if cast { should_be_in_actions_list(w, owner, g) } else { Some(true) };
            match listed {
                Some(false) => None,
                Some(true) => Some((Some(g), base_score(w, g).unwrap_or(f32::NAN))),
                None => Some((None, base_score(w, g).unwrap_or(f32::NAN))),
            }
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

/// `BlueprintAiCastSpell.ShouldBeInActionsList(unit)`: `!IsExpended(GetAbilityForUse(unit),
/// GetSpellbookForUse(unit))`, true when the unit has the ability as a fact or a spellbook that
/// can spend it. `None` where the sim can't tell: no fact but a spellbook (`Spellbook.CanSpend`
/// isn't ported), a random variant (`CreateAbilityData` draws), no ability.
fn should_be_in_actions_list(w: &World, u: EntityIdx, g: Guid) -> Option<bool> {
    let v = w.content.get(g)?;
    let v = v.view();
    let ability = v.guid("m_Ability")?;
    if spells::ability_fact(w, u, ability).is_some() {
        // (`CreateAbilityData` with a fact: a random variant draws.)
        return (!v.bool("m_RandomVariant")).then_some(true);
    }
    // `unit.Spellbooks.FirstOrDefault(sb => sb.CanSpend(Ability))`: none without spellbooks.
    w.unit(u).is_some_and(|x| x.spellbooks.is_empty()).then_some(false)
}

/// What a `BlueprintAiCastSpell` adds to an action.
#[derive(Debug, Clone)]
struct Cast {
    /// `m_Ability`.
    ability: Guid,
    force_self: bool,
    force_enemy: bool,
    point_under_target: bool,
    /// `m_DeadTargetType` (`AliveOnly` 0).
    dead: i64,
}

/// An `AiActionSimple` of a `BlueprintAiAttack` or a `BlueprintAiCastSpell`, as the port supports
/// it.
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
    /// `IsAffectedByImpatience`: an attack's is; a cast's is `m_AffectedByImpatience`.
    impatience: bool,
    /// A cast action's (`None`: an attack).
    cast: Option<Cast>,
}

fn action(w: &World, g: Guid) -> R<Action> {
    let Some(bp) = w.content.get(g) else {
        return desync(format!("the AI action blueprint {g} is unknown"));
    };
    let v = bp.view();
    let cast = match bp.type_name() {
        "BlueprintAiAttack" => None,
        "BlueprintAiCastSpell" => {
            let Some(ability) = v.guid("m_Ability") else {
                return unsupported("a BlueprintAiCastSpell without an ability");
            };
            // `CreateAbilityData`: a variant makes `new AbilityData(abilityData, variant)` (a
            // random one draws).
            if v.bool("m_RandomVariant") || v.guid("m_Variant").is_some() {
                return unsupported("a BlueprintAiCastSpell's ability variant");
            }
            // (`GetTargetType`, `IsTargetValid` and `CreateCommand` read the locators.)
            if !v.seq("Locators").is_empty() {
                return unsupported("a BlueprintAiCastSpell's locators");
            }
            Some(Cast {
                ability,
                force_self: v.bool("m_ForceTargetSelf"),
                force_enemy: v.bool("m_ForceTargetEnemy"),
                point_under_target: v.bool("m_TargetPointUnderTarget"),
                dead: v.int("m_DeadTargetType"),
            })
        }
        other => return unsupported(&format!("an AI action of type {other}")),
    };
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
        impatience: cast.is_none() || v.bool("m_AffectedByImpatience"),
        cast,
    })
}

/// `GetRange` of a cast action (`BlueprintAiCastSpell.GetRange`) where the blueprint decides it:
/// `Ability.GetRange()` (no `AbilityData`: the blueprint's `Range`) in meters (`Feet.Meters`:
/// `feet * 0.3048f`). `None` where it reads the unit (touch: `GetWeaponRange`; double move:
/// `CombatSpeedMps * 6`; weapon: the first weapon's `AttackRange`): the recorded range is then
/// an input, as an attack's reach is.
fn cast_range(w: &World, ability: Guid) -> R<Option<f32>> {
    let Some(bp) = w.content.get(ability) else {
        return desync(format!("the AI's ability {ability} is unknown"));
    };
    let v = bp.view();
    let meters = |feet: f32| store_f32(f64::from(feet) * f64::from(0.3048f32));
    // `AbilityRange`: Touch, Close, Medium, Long, Projectile, Unlimited, Personal, Custom,
    // DoubleMove, Weapon.
    Ok(match v.int("Range") {
        0 | 8 | 9 => None,
        1 => Some(meters(30.0)),
        2 => Some(meters(40.0)),
        3 => Some(meters(50.0)),
        // (`new Feet(float.PositiveInfinity)`: the replay's leaves are finite.)
        5 => return unsupported("BlueprintAbility.GetRange of an unlimited range"),
        6 => Some(meters(0.0)),
        7 => Some(meters(v.obj("CustomRange").map_or(0.0, |f| f.f32("m_Value")))),
        _ => return unsupported("BlueprintAbility.GetRange of a projectile range"),
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
    c.expended.clear();
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
    c.ability = None;
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

/// `DecisionContext.Unit` of the decision running, if any.
#[must_use]
pub fn context_unit(w: &World) -> Option<EntityIdx> {
    w.combat.ai.ctx.unit
}

fn ctx_unit(w: &World) -> R<EntityIdx> {
    match w.combat.ai.ctx.unit {
        Some(u) => Ok(u),
        None => desync("the AI's context has no unit"),
    }
}

/// `DecisionContext.Enemies` (`CalculateEnemies(forDead: false, ...)`).
/// `unit.Group.Memory.HasPlayerCharacterInMemory()` (the remembered enemy's group's memory).
fn hpcm<I: AiInputs>(w: &mut World, inp: &mut I, u: EntityIdx) -> R<bool> {
    let recorded = inp.ai_bool(w, "hpcm", None)?;
    Ok(crate::combat::memory::sim::check(w, "AI's HasPlayerCharacterInMemory", recorded, |w| {
        crate::combat::memory::sim::has_player_character(w, u).and_then(Result::ok)
    }))
}

fn enemies<I: AiInputs>(w: &mut World, inp: &mut I) -> R<()> {
    let me = ctx_unit(w)?;
    let me_id = uid(w, me);
    if !w.combat.ai.ctx.enemies_init {
        if stable_bool(w, inp, "pe", &me_id)? {
            return unsupported("a personal enemy (UnitPartPersonalEnemy)");
        }
        let mut list = Vec::new();
        let pf = is_player_faction(w, me);
        // `Unit.Group.Memory.Enemies`: the sim's memory, while it's the sim's.
        let recorded = inp.ai_enemies(w)?;
        let memory = crate::combat::memory::sim::check(w, "AI's Enemies", recorded, |w| {
            crate::combat::memory::sim::enemies(w, me)
                .map(|v| crate::combat::memory::sim::ids(w, &v))
        });
        ctx(w).memory_enemies = Some(memory.len());
        for id in memory {
            let u = unit_of(w, &id)?;
            if stable_bool(w, inp, "pe", &id)? {
                ctx(w).personal = true;
            }
            if super::units::is_dead(w, u)
                || inp.leaf(w, "untgt", Of::Pair(Some(me_id.clone()), Some(id.clone())))?
                || (pf && super::units::is_conscious(w, u) && !hpcm(w, inp, u)?)
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
    // `ExpendedActions`: each removed from `AvailableActions` (`List.Remove`: the first).
    for g in std::mem::take(&mut ctx(w).expended) {
        if let Some(b) = w.unit_mut(u).and_then(|x| x.brain.as_mut())
            && let Some(i) = b.available.iter().position(|x| *x == Some(g))
        {
            b.available.remove(i);
        }
    }
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
                // `CanKeepCommand`: a cast's is the base's (true); an attack's a `UnitAttack`
                // and no auto-use ability.
                if best.cast.is_none() {
                    let auto = inp.ai_blueprint(w, "auto", &id)?;
                    if auto.is_some() {
                        return unsupported("an auto-use ability");
                    }
                    if !is_attack {
                        return unsupported("CanKeepCommand of a command other than UnitAttack");
                    }
                }
                ctx(w).best_path = None;
                let nct = w.combat.ai.nct.get(&u).copied();
                return inp.ai_selected(w, &id, true, nct);
            }
        }
    }
    init_action(w, inp, &best)?;
    // `ApplyAction()`: nothing (neither blueprint overrides it). `CreateCommand`:
    let (spec, ability) = match &best.cast {
        None => {
            // `BlueprintAiAttack.CreateCommand`: no auto-use ability;
            // `UnitAttack.CreateAttackCommand`: no magus' spell before the attack.
            if inp.ai_blueprint(w, "auto", &id)?.is_some() {
                return unsupported("an auto-use ability");
            }
            if inp.ai_bool(w, "magus", Some(&id))? {
                return unsupported("UnitAttack.CreateAttackCommand for a magus");
            }
            let spec = NewCommand {
                class: uc::class::ATTACK,
                ty: uc::ty::STANDARD,
                target: target_id,
                los: true,
            };
            (spec, None)
        }
        Some(cast) => {
            let (spec, a) = create_cast_command(w, cast, target_id)?;
            (spec, Some(a))
        }
    };
    let path = ctx(w).best_path.take();
    let five = w.combat.ai.ctx.best_five.unwrap_or(false);
    // `NextCommandTime = Time.time`; then, for a command neither free nor swift,
    // `NextCommandTime += 0.3f + Random.Range(0f, 0.1f)`.
    let time = inp.ai_float(w, "time", None)?;
    inp.ai_nct_write(w, &id, time)?;
    let mut nct = time;
    if spec.ty != uc::ty::FREE && spec.ty != uc::ty::SWIFT {
        let r = inp.ai_draw(w)?;
        nct = store_f32(f64::from(time) + (f64::from(0.3f32) + f64::from(r)));
        inp.ai_nct_write(w, &id, nct)?;
    }
    w.combat.ai.nct.insert(u, nct);
    let c = inp.ai_new_run(w, set, &spec)?;
    // The command's `Ability` is the context's `AbilityData` (its spellcasting is
    // `combat::spells`').
    if let Some(a) = ability {
        match w.combat.commands.cmds.get_mut(&c) {
            Some(x) => x.ability = Some(a),
            None => return desync(format!("the AI's command {c} isn't described")),
        }
    }
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

/// Components implementing `ILineOfSightIgnore` (a `UnitUseAbility`'s `NeedLoS` is false with
/// one).
const LOS_IGNORE: &[&str] = &[
    "AbilityEffectStickyTouch",
    "AbilityTargetIsDeadAnimalCompanion",
    "AbilityTargetIsDeadCompanion",
    "AbilityTargetStoneToFlesh",
    "DimensionDoorRestrictionIgnorance",
    "LineOfSightIgnorance",
];

/// `BlueprintAiCastSpell.CreateCommand(context, target)` (no locators: [`action`]):
/// `UnitUseAbility.CreateCastCommand(ability, target)` of an available ability, the target a
/// unit, without a touch part or a magus' part (reported: the touch ability or a spellstrike
/// attack may replace it), `new UnitUseAbility(RuntimeActionType, ability, target)`. A missing
/// or unavailable ability's `UnitMoveTo` and a point target are reported.
fn create_cast_command(w: &mut World, cast: &Cast, target: String) -> R<(NewCommand, Ability)> {
    let me = ctx_unit(w)?;
    let ability = w.combat.ai.ctx.ability.clone();
    let available = match &ability {
        Some(a) => spells::is_available(w, a).map_err(|e| {
            Desync(format!("BlueprintAiCastSpell.CreateCommand: {e} (not modelled)"))
        })?,
        None => false,
    };
    let Some(a) = ability.filter(|_| available) else {
        return unsupported(
            "BlueprintAiCastSpell.CreateCommand without an available ability (UnitMoveTo)",
        );
    };
    if cast.point_under_target {
        return unsupported("BlueprintAiCastSpell.CreateCommand at the point under its target");
    }
    if let Some(p) = w
        .entity(me)
        .parts
        .iter()
        .map(|p| p.clr)
        .find(|p| *p == "UnitPartTouch" || *p == "UnitPartMagus")
    {
        return unsupported(&format!("UnitUseAbility.CreateCastCommand with {p}"));
    }
    let ty = spells::runtime_action_type(w, &a)
        .map_err(|e| Desync(format!("UnitUseAbility.CreateCastCommand: {e} (not modelled)")))?;
    let Some(bp) = a.bp(w) else { return desync("the AI's ability blueprint is missing") };
    let los = !bp.components_array().iter().any(|o| LOS_IGNORE.iter().any(|n| is_a(o.ty, n)));
    Ok((NewCommand { class: uc::class::USE_ABILITY, ty, target, los }, a))
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
            let Some(g) = g else {
                return unsupported(
                    "an AI action the sim can't establish (not an AiActionSimple, or a cast spell's ShouldBeInActionsList)",
                );
            };
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

/// `DecisionContext.InitAction(action)`: `Ability = null`, `InitContext` (an attack's nothing, a
/// cast's [`init_cast_context`]), `GetRange` (the weapon's reach, an input; a cast's
/// [`cast_range`], the recorded value checked where the blueprint decides it), `GetRadius` (read
/// by nothing the port runs).
fn init_action<I: AiInputs>(w: &mut World, inp: &mut I, a: &Action) -> R<()> {
    ctx(w).ability = None;
    if let Some(cast) = &a.cast {
        init_cast_context(w, a, cast)?;
    }
    let range = inp.ai_range(w, a.guid)?;
    if let Some(cast) = &a.cast
        && let Some(sim) = cast_range(w, cast.ability)?
        && sim.to_bits() != range.to_bits()
    {
        return desync(format!(
            "the cast action {}'s range: the sim's {sim}, the recording's {range}",
            a.guid
        ));
    }
    stable(w, "range", &a.guid.to_string(), Stable::Float(range.to_bits()))?;
    let base = dec(a.base_score)?;
    let c = ctx(w);
    c.action = Some(a.guid);
    c.action_range = range;
    c.cur = base;
    c.target = None;
    Ok(())
}

/// `BlueprintAiCastSpell.InitContext(context)`: the action's `Data.AbilityData`, made the first
/// time it is null (`GetAbilityForUse`: `new AbilityData(fact)` of the unit's `Ability` fact, no
/// variant: [`action`]; without one the spellbook's, `GetSpellbookForUse`, reported when the unit
/// has a spellbook), kept in [`Ai::cast_data`]; `context.Ability` is it.
fn init_cast_context(w: &mut World, a: &Action, cast: &Cast) -> R<()> {
    let me = ctx_unit(w)?;
    let key = (me, a.guid);
    let fact = match w.combat.ai.cast_data.get(&key) {
        Some(f) => Some(*f),
        None => match spells::ability_fact(w, me, cast.ability) {
            Some(f) => {
                w.combat.ai.cast_data.insert(key, f);
                Some(f)
            }
            None if w.unit(me).is_some_and(|x| x.spellbooks.is_empty()) => None,
            None => {
                return unsupported(
                    "BlueprintAiCastSpell.GetSpellbookForUse (Spellbook.CanSpend of an ability)",
                );
            }
        },
    };
    // (A brain listing one cast action twice would keep two caches.)
    let listed = w
        .unit(me)
        .and_then(|x| x.brain.as_ref())
        .map_or(0, |b| b.available.iter().filter(|x| **x == Some(a.guid)).count());
    if listed > 1 {
        return unsupported("a brain listing one cast action twice");
    }
    let ability = match fact {
        Some(f) => match Ability::of_fact(w, f) {
            Some(x) => Some(x),
            None => return desync("the AI's ability fact has no owner or blueprint"),
        },
        None => None,
    };
    ctx(w).ability = ability;
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
    // `GetTargetType` and `GetDeadTargetType`: an attack's enemies, alive only; a cast's
    // ([`cast_targets`]) the same where it forces an enemy target.
    if let Some(cast) = &a.cast {
        cast_targets(w, cast)?;
    }
    enemies(w, inp)?;
    calculate_targets_score(w, inp, a, actor_score, best, target)?;
    Ok(())
}

/// `BlueprintAiCastSpell.GetTargetType` (`ForceTargetSelf`: `Self`; `ForceTargetEnemy`: `Enemy`;
/// else by the ability's anchor and targets) and `GetDeadTargetType` (`AliveOnly`, or the
/// action's when the ability can target the dead): only enemies, alive only, are modelled.
fn cast_targets(w: &World, cast: &Cast) -> R<()> {
    if cast.force_self {
        return unsupported("an AI cast action's Self targets (ForceTargetSelf)");
    }
    if !cast.force_enemy {
        return unsupported("BlueprintAiCastSpell.GetTargetType without a forced target");
    }
    let Some(bp) = w.content.get(cast.ability) else {
        return desync(format!("the AI's ability {} is unknown", cast.ability));
    };
    if cast.dead != 0 && spells::can_cast_to_dead_target(&bp) {
        return unsupported("an AI cast action's dead targets (DeadTargetType)");
    }
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
        // `IsAvailable` and `IsExpended`: an attack's are true and false. A cast's: the
        // context's `AbilityData.IsAvailable` (true without one), then expended without one
        // (`IsExpended(ability, ability?.Spellbook)`: an ability fact's has no spellbook).
        if a.cast.is_some() {
            let ability = w.combat.ai.ctx.ability.clone();
            match ability {
                Some(ab) => match spells::is_available(w, &ab) {
                    Ok(true) => {}
                    Ok(false) => break 'v false,
                    Err(e) => {
                        return unsupported(&format!("BlueprintAiCastSpell.IsAvailable: {e}"));
                    }
                },
                None => {
                    ctx(w).expended.push(a.guid);
                    break 'v false;
                }
            }
        }
        true
    };
    if valid {
        score_considerations(w, inp, &a.actor)?;
        // In turn-based combat, an ability that needs a full-round action scores 0 for a
        // subject (`Target.Unit ?? Unit`) neither surprising nor with one.
        let ability = w.combat.ai.ctx.ability.clone();
        if turn_based(w)
            && w.combat.ai.ctx.cur > Decimal::ZERO
            && let Some(ab) = ability
        {
            let full = spells::require_full_round_action(w, &ab).map_err(|e| {
                Desync(format!("AiAction.ScoreActor: RequireFullRoundAction: {e} (not modelled)"))
            })?;
            let t = w.combat.ai.ctx.target.map_or(me, |i| w.combat.ai.ctx.enemies[i].unit);
            if full
                && !super::cooldowns::is_surprising(w, t)
                && !super::cooldowns::has_full_round_action(w, t)
            {
                ctx(w).cur = Decimal::ZERO;
            }
        }
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
        "ThreatedByConsideration" => {
            // `(Target.Unit ?? Unit).CombatState.EngagedBy.Count`: the engagement model's.
            let count = engaged_by_count(w, subject)?;
            let (min, max) = (v.i32("MinCount"), v.i32("MaxCount"));
            if count < min {
                return Ok(v.f32("BelowMinScore"));
            }
            #[allow(clippy::cast_precision_loss)]
            let t = inverse_lerp(min as f32, max as f32, count as f32);
            let mut num = lerp_unclamped(v.f32("MinScore"), v.f32("MaxScore"), t);
            if count > max {
                #[allow(clippy::cast_precision_loss)]
                let extra = count.wrapping_sub(max) as f32;
                num = store_f32(
                    f64::from(num) + f64::from(v.f32("ExtraTargetScore")) * f64::from(extra),
                );
            }
            Ok(clamp01(num))
        }
        "DistanceConsideration" => {
            // Without a target: an error logged, 0.
            let Some(t) = t else { return Ok(0.0) };
            let x = inverse_lerp(v.f32("MinDistance"), v.f32("MaxDistance"), t.distance);
            Ok(lerp(v.f32("MinDistanceScore"), v.f32("MaxDistanceScore"), x))
        }
        other => unsupported(&format!("the AI consideration {other}")),
    }
}

/// `UnitCombatState.EngagedBy.Count` of `u`: the engagement model's (reported without it, or
/// before the unit's state is the game's).
fn engaged_by_count(w: &World, u: EntityIdx) -> R<i32> {
    let known = w.unit(u).filter(|x| w.combat.engage.on && x.combat.eng.known);
    match known.map(|x| i32::try_from(x.combat.eng.engaged_by.len())) {
        Some(Ok(n)) => Ok(n),
        _ => unsupported("ThreatedByConsideration's EngagedBy without the engagement model"),
    }
}

/// `Mathf.InverseLerp(a, b, value)`: `Clamp01((value - a) / (b - a))` (its argument stored as a
/// `float`), 0 when `a == b`.
fn inverse_lerp(a: f32, b: f32, value: f32) -> f32 {
    if a == b {
        return 0.0;
    }
    clamp01(store_f32((f64::from(value) - f64::from(a)) / (f64::from(b) - f64::from(a))))
}

/// `Mathf.LerpUnclamped(a, b, t)`: `a + (b - a) * t`.
fn lerp_unclamped(a: f32, b: f32, t: f32) -> f32 {
    store_f32(f64::from(a) + (f64::from(b) - f64::from(a)) * f64::from(t))
}

/// `Mathf.Lerp(a, b, t)`: `a + (b - a) * Clamp01(t)`.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    lerp_unclamped(a, b, clamp01(t))
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
        // `IsAffectedByImpatience`: an attack is; a cast is when its blueprint says.
        if a.impatience && !w.combat.ai.ctx.is_player_faction && !not_priority {
            let disliked = stable_bool(w, inp, "disliked", &t.id)?;
            // (The impatience controller's, when modelled: a check.)
            if w.combat.impatience.on {
                let me = ctx_unit(w)?;
                if super::impatience::disliked(w, me, t.unit)? != disliked {
                    return desync(format!(
                        "{me_id}'s DislikedTargets.Contains({}): the sim's differs",
                        t.id
                    ));
                }
            }
            if disliked {
                let c = ctx(w);
                c.cur = mul(c.cur, Decimal::lit(1, 2))?;
            }
            let ipt = inp.ai_unit(w, "ipt", None)?;
            stable(w, "ipt", &me_id, Stable::Unit(ipt.clone()))?;
            if w.combat.impatience.on {
                let me = ctx_unit(w)?;
                let sim = super::impatience::priority(w, me)?.map(|p| uid(w, p));
                if sim != ipt {
                    return desync(format!(
                        "{me_id}'s ImpatiencePriorityTarget: the sim's {sim:?}, the recording's {ipt:?}"
                    ));
                }
            }
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
        // `IsTargetValid`: an attack's is true; a cast's [`cast_target_valid`].
        match &a.cast {
            Some(cast) => cast_target_valid(w, cast, me, t.unit)?,
            None => true,
        }
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

/// Components implementing `IAbilityMinRangeProvider` (`IsTargetValid` reads the distance to the
/// target against it).
const MIN_RANGE_PROVIDERS: &[&str] = &[
    "AbilityCustomCharge",
    "AbilityCustomFly",
    "AbilityCustomOverrun",
    "AbilityCustomTongueGrab",
    "AbilityDemonCharge",
    "AbilityGriffinAttack",
];

/// `BlueprintAiCastSpell.IsTargetValid(context)` (no locators: [`action`]): the static
/// `IsTargetValid(Ability, Unit, Target.Unit ?? Unit)`, a minimum range (reported) and the
/// ability's target restrictions ([`spells::target_restrictions_passed`]), then the base's
/// (true).
fn cast_target_valid(w: &World, cast: &Cast, me: EntityIdx, target: EntityIdx) -> R<bool> {
    let Some(bp) = w.content.get(cast.ability) else {
        return desync(format!("the AI's ability {} is unknown", cast.ability));
    };
    if !bp.view().bool("IgnoreMinimalRangeLimit")
        && bp.components_array().iter().any(|o| MIN_RANGE_PROVIDERS.iter().any(|n| is_a(o.ty, n)))
    {
        return unsupported("BlueprintAiCastSpell.IsTargetValid: a minimum range");
    }
    spells::target_restrictions_passed(w, &bp, me, target)
        .map_err(|e| Desync(format!("BlueprintAiCastSpell.IsTargetValid: {e} (not modelled)")))
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
    let v = inp.ai_blocked(w, p)?;
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
