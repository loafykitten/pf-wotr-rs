//! The impatience controller (decompiled, not copied: `Kingmaker.Controllers.UnitImpatienceController`
//! and `CombatAiData`'s impatience state). docs/poc5/commands.md, slice G.
//!
//! Each unit's `HardTargets`, `DislikedTargets`, `BotherUnits` (.NET `HashSet`s: [`CsHashSet`]),
//! `ConsequentAttacksWithoutEffect` (commands), `LastOffensiveStandardActionTarget`,
//! `ImpatiencePriorityTarget` and `ConsequentInefficientAttacksCount`. None is saved: the window's
//! initial state is recorded (WrathOracle's `imp_init`); a unit constructed since starts empty.
//!
//! The controller's tick and `UpdatePriorityTarget` (from the tick or `TurnController.Prepare`)
//! run here with what they read outside the sim as inputs ([`ImpInputs`]: `Player.IsInCombat`,
//! memory visibility, distances, projectiles on the way, `UnitState.Impatience`), and their two
//! `Random.Range` draws. The handlers (a command's start, `RuleDealDamage`, the offensive actions
//! `OffensiveActionsController` raises on `RuleAttackRoll`, a death) run inside other code: each
//! queues its operation ([`Op`]) with what the sim computes at that point, and the driven replay
//! applies the queue in order as the handlers' records come (`TryAddBotherUnit`'s visibility read
//! is its record's). Leaving combat's `CombatAiData.Clear` goes through the queue too, so every
//! change keeps the game's order. Reported, not modelled: abilities (`UnitUseAbility`), heals,
//! summons, mounts, a fake attack roll.

use std::collections::{HashMap, VecDeque};

use super::commands::{self as uc, Desync, R, class, res};
use crate::csharp::arith::store_f32;
use crate::csharp::hashset::CsHashSet;
use crate::rng::CallSite;
use crate::world::{EntityIdx, World};

/// `UnitImpatienceController.UpdatePriorityTarget`: `Random.Range(0, 100) < switchTargetChance`.
pub const SWITCH_DRAW: CallSite = CallSite(
    "Assembly-CSharp|Kingmaker.Controllers.UnitImpatienceController|UpdatePriorityTarget(UnitEntityData)|#0@121|Range(int,int)",
);
/// `UnitImpatienceController.SetupPriorityTarget`: `Random.Range(0f, sum)`.
pub const SETUP_DRAW: CallSite = CallSite(
    "Assembly-CSharp|Kingmaker.Controllers.UnitImpatienceController|SetupPriorityTarget(UnitEntityData)|#0@190|Range(float,float)",
);

/// A unit's impatience state (`CombatAiData`).
#[derive(Debug, Clone, Default)]
pub struct UnitImpatience {
    /// The state is the game's: the window's initial state adopted (`imp_init`), or the unit
    /// constructed since.
    pub known: bool,
    pub hard: CsHashSet<EntityIdx>,
    pub disliked: CsHashSet<EntityIdx>,
    pub bother: CsHashSet<EntityIdx>,
    /// `ConsequentAttacksWithoutEffect`: the commands' ids.
    pub attacks: Vec<u32>,
    pub last_target: Option<EntityIdx>,
    pub priority: Option<EntityIdx>,
    pub inefficient: i32,
}

/// A handler's operation, applied when its record comes (in order).
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// `HandleUnitCommandDidStart(c)` of executor `u`: `target` the command's when it counts (in
    /// combat, a standard `UnitAttack` on an enemy). Record: `imp_s` `start`.
    Start { u: EntityIdx, c: u32, target: Option<EntityIdx> },
    /// `OnEventDidTrigger(RuleDealDamage)`'s list update (no record of its own): when `active`
    /// (in combat, damage of 1 or more, an enemy) and the initiator in combat, its attacks without
    /// effect up to the one whose attack roll (`roll`) it is are dropped.
    DamageList { initiator: EntityIdx, target: EntityIdx, roll: Option<u64>, in_combat: bool },
    /// The handler's end. Record: `imp_s` `dmg` (the initiator's).
    DamageEnd { initiator: EntityIdx },
    /// `TryAddBotherUnit(u, b)`: `cond` is `u.IsInCombat && u.IsEnemy(b)` (then the visibility
    /// read decides); `damage`: the damage handler's (else an offensive action's). Record: `imp_s`
    /// `bother` (with its read), in that handler.
    Bother { u: EntityIdx, b: EntityIdx, cond: bool, damage: bool },
    /// `HandleUnitDeath(dead)`: when in combat, `dead` leaves every awake unit's bother units.
    /// No record of its own: a [`Op::DeathCheck`] follows for each awake unit, in order. `muts`:
    /// the sim's mutation records before the notification (where its records belong).
    Death { dead: EntityIdx, active: bool, awake: Vec<EntityIdx>, muts: usize },
    /// An awake unit's state after a death. Record: `imp_s` `death`.
    DeathCheck { dead: EntityIdx, u: EntityIdx },
    /// `CombatAiData.Clear` (leaving combat). No record.
    Clear { u: EntityIdx },
}

/// The controller's state.
#[derive(Debug, Default)]
pub struct Impatience {
    /// The model runs (a recording with WrathOracle's `impatience` observer).
    pub on: bool,
    /// The handlers' operations not yet applied.
    pub queue: VecDeque<Op>,
    /// Each `UnitAttack`'s attack rolls (`AllAttacks[i].Rule.AttackRoll`), by command, as its
    /// attack rules fire.
    pub rolls: HashMap<u32, Vec<u64>>,
    /// The memory controller's `AddToMemory(observer, target)` calls an offensive action makes
    /// (`UnitMemoryController.HandleUnitMakeOffensiveAction`, the other subscriber of the event),
    /// not yet matched with their `mem_q` records; `None` without WrathOracle's `memory` observer
    /// (docs/poc5/memory.md).
    pub memory_adds: Option<VecDeque<MemoryCall>>,
    /// Its `AddToMemory` calls at a command's act (`HandleUnitCommandDidAct`), not yet matched;
    /// `None` as `memory_adds`.
    pub memory_acts: Option<VecDeque<MemoryCall>>,
}

/// An `AddToMemory(observer, target)` call the sim made, the target's `Faction.Peaceful` it read
/// and whether the memory's queue took the pair (both `None` while the memory isn't the sim's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryCall {
    pub observer: EntityIdx,
    pub target: EntityIdx,
    pub peaceful: Option<bool>,
    pub queued: Option<bool>,
}

/// `UnitMemoryController.AddToMemory(observer, target)` on the sim's controller (the target's
/// `Faction.Peaceful`, then the queue's dedup), when the memory is the sim's.
fn add_to_memory(w: &mut World, observer: EntityIdx, target: EntityIdx) -> MemoryCall {
    let none = MemoryCall { observer, target, peaceful: None, queued: None };
    if !w.combat.memory.on {
        return none;
    }
    let Some(peaceful) = super::units::faction_peaceful(w, target) else {
        w.coverage.unsupported("memory: AddToMemory of a target without a faction");
        return none;
    };
    let q = &mut w.combat.memory.ctl;
    let before = q.units_for_add.len();
    q.add_to_memory(observer, target, peaceful);
    MemoryCall { peaceful: Some(peaceful), queued: Some(q.units_for_add.len() > before), ..none }
}

/// `unit.Memory.ContainsVisible(o)` (the impatience controller's): the sim's memory, while it's
/// the sim's.
fn visible<I: ImpInputs>(w: &mut World, inp: &mut I, u: EntityIdx, o: EntityIdx) -> R<bool> {
    let recorded = inp.imp_visible(w, o)?;
    Ok(crate::combat::memory::sim::check(w, "impatience's ContainsVisible", recorded, |w| {
        crate::combat::memory::sim::contains(w, u, o, true)
    }))
}

/// `UnitMemoryController.HandleUnitCommandDidAct(command)`'s `AddToMemory` calls, as the command
/// acts: a players' enemy's command on a unit of the player's faction makes the executor remember
/// the target (unless it's invisible: reported, see-invisibility isn't ported) and the target the
/// executor. Matched with their `mem_q` records as they come (the memory queue itself isn't wired).
pub fn memory_command_did_act(w: &mut World, executor: Option<EntityIdx>, target: Option<&str>) {
    if w.combat.impatience.memory_acts.is_none() {
        return;
    }
    // `command.TargetUnit == null` returns before anything.
    let Some(t) = target.and_then(|id| w.by_id.get(id).copied()).filter(|t| w.unit(*t).is_some())
    else {
        return;
    };
    let Some(e) = executor else {
        w.coverage.unsupported("memory: a command acting without its executor");
        return;
    };
    if !(super::units::is_players_enemy(w, e) && super::units::is_player_faction(w, t)) {
        return;
    }
    if crate::unit_state::has_condition(w, t, crate::combat::memory::INVISIBLE) {
        w.coverage.unsupported("memory: a command acting on an invisible target");
        return;
    }
    let calls = [add_to_memory(w, e, t), add_to_memory(w, t, e)];
    if let Some(q) = w.combat.impatience.memory_acts.as_mut() {
        q.extend(calls);
    }
}

/// `UnitMemoryController.HandleUnitMakeOffensiveAction(unit, target)`'s `AddToMemory` calls, as
/// the event raises it (`OffensiveActionsController`'s gates passed: both units in the game): the
/// unit remembers the target unless it's invisible to it, then the target the unit. The queue
/// itself is the memory model's (`combat/memory.rs`), not wired: each call's record is matched.
/// Reported: the target's `UnitAggroFilter` (its `OnAggroAction` runs first), an invisible target
/// (see-invisibility and blindsense aren't ported).
fn memory_offensive_action(w: &mut World, unit: EntityIdx, target: EntityIdx) {
    if w.combat.impatience.memory_adds.is_none() {
        return;
    }
    let bp = w.unit(target).and_then(|x| x.blueprint).and_then(|g| w.content.get(g));
    if bp.is_some_and(|b| crate::unit_facts::has_component(&b, "UnitAggroFilter")) {
        w.coverage.unsupported("memory: an offensive action on a target with a UnitAggroFilter");
    }
    if crate::unit_state::has_condition(w, target, crate::combat::memory::INVISIBLE) {
        w.coverage.unsupported("memory: an offensive action on an invisible target");
        return;
    }
    let calls = [add_to_memory(w, unit, target), add_to_memory(w, target, unit)];
    if let Some(q) = w.combat.impatience.memory_adds.as_mut() {
        q.extend(calls);
    }
}

/// What the controller's code reads outside the sim, its draws and its checks.
pub trait ImpInputs {
    /// `Player.IsInCombat` (the controller's `IsInCombat`).
    fn imp_in_combat(&mut self, w: &mut World) -> R<bool>;
    /// `unit.Memory.ContainsVisible(o)` of `u`'s memory.
    fn imp_visible(&mut self, w: &mut World, o: EntityIdx) -> R<bool>;
    /// `u.DistanceTo(o)`.
    fn imp_distance(&mut self, w: &mut World, u: EntityIdx, o: EntityIdx) -> R<f32>;
    /// `UnitAttack.LaunchedProjectiles`' `OnTheWay`, of command `c`.
    fn imp_on_the_way(&mut self, w: &mut World, c: u32) -> R<Vec<bool>>;
    /// `unit.State.Impatience`.
    fn imp_level(&mut self, w: &mut World, u: EntityIdx) -> R<i32>;
    /// `UpdatePriorityTarget`'s `Random.Range(0, 100)`: the sim's draw, checked with its record.
    fn imp_draw_switch(&mut self, w: &mut World) -> R<i32>;
    /// `SetupPriorityTarget`'s `Random.Range(0f, max)`: the sim's draw, checked with its record.
    fn imp_draw_setup(&mut self, w: &mut World, max: f32) -> R<f32>;
    /// `u`'s state after `at` (`upd`, `setup`).
    fn imp_state(&mut self, w: &mut World, at: &'static str, u: EntityIdx) -> R<()>;
    /// The tick's own `UpdatePriorityTarget(u)` starts (a nested entry) and ends.
    fn imp_nested(&mut self, w: &mut World, u: EntityIdx) -> R<()>;
    fn imp_nested_end(&mut self, w: &mut World) -> R<()>;
}

impl Op {
    /// Whether the operation has a record of its own (the others apply silently, in order).
    #[must_use]
    pub fn recorded(&self) -> bool {
        matches!(
            self,
            Self::Start { .. }
                | Self::DamageEnd { .. }
                | Self::Bother { .. }
                | Self::DeathCheck { .. }
        )
    }
}

fn uid(w: &World, u: EntityIdx) -> String {
    w.entity(u).unique_id.clone()
}

fn desync<T>(what: impl Into<String>) -> R<T> {
    Err(Desync(what.into()))
}

fn unsupported<T>(w: &mut World, what: &str) -> R<T> {
    w.coverage.unsupported(format!("impatience: {what}"));
    desync(format!("not modelled: {what}"))
}

/// Unit `u`'s state, when established.
pub fn state(w: &World, u: EntityIdx) -> R<&UnitImpatience> {
    match w.unit(u) {
        Some(x) if x.combat.imp.known => Ok(&x.combat.imp),
        Some(_) => desync(format!("unit {}'s impatience isn't established", uid(w, u))),
        None => desync("the impatience of an entity that isn't a unit"),
    }
}

fn state_mut(w: &mut World, u: EntityIdx) -> R<&mut UnitImpatience> {
    state(w, u)?;
    Ok(&mut w.unit_mut(u).expect("a unit").combat.imp)
}

/// `BlueprintRoot.SystemMechanics.Impatience`'s entry of `level`: its threshold and switch
/// chance (`int.MaxValue` and 0 without one).
fn settings(w: &World, level: i32) -> (i32, i32) {
    let root = w.content.get(crate::unit::BLUEPRINT_ROOT);
    let found = root.as_ref().and_then(|r| {
        r.view().obj("SystemMechanics").and_then(|m| {
            m.objs("Impatience")
                .into_iter()
                .map(crate::content::View)
                .find(|i| i.i32("Type") == level)
                .map(|i| (i.i32("InefficientAttacksCountThreshold"), i.i32("SwitchTargetChance")))
        })
    });
    found.unwrap_or((i32::MAX, 0))
}

/// `Tick()` over `awake` (`Game.Instance.State.AwakeUnits`, in its order).
pub fn tick<I: ImpInputs + uc::Inputs>(w: &mut World, inp: &mut I, awake: &[EntityIdx]) -> R<()> {
    if !inp.imp_in_combat(w)? {
        return Ok(());
    }
    for &u in awake {
        if !super::is_in_combat(w, u) {
            continue;
        }
        inp.imp_nested(w, u)?;
        update_priority_target(w, inp, u)?;
        inp.imp_nested_end(w)?;
        let p = state(w, u)?.priority;
        if let Some(p) = p
            && (!super::units::is_conscious(w, p) || !visible(w, inp, u, p)?)
        {
            setup_priority_target(w, inp, u)?;
        }
    }
    Ok(())
}

/// `UpdatePriorityTarget(unit)`.
pub fn update_priority_target<I: ImpInputs + uc::Inputs>(
    w: &mut World,
    inp: &mut I,
    u: EntityIdx,
) -> R<()> {
    let last = state(w, u)?.last_target;
    // `GetConsequentInefficientAttacksCount`: from the last, the commands that succeeded with no
    // projectile on the way.
    let mut count = 0;
    for c in state(w, u)?.attacks.clone().into_iter().rev() {
        let Some(x) = w.combat.commands.cmds.get(&c) else {
            return desync(format!(
                "command {c} of {}'s attacks without effect is unknown",
                uid(w, u)
            ));
        };
        if x.s.result != res::SUCCESS {
            continue;
        }
        if x.is(class::USE_ABILITY) {
            return unsupported(w, "an ability among the attacks without effect");
        }
        if x.is(class::ATTACK) && inp.imp_on_the_way(w, c)?.into_iter().any(|b| b) {
            continue;
        }
        count += 1;
    }
    if state(w, u)?.inefficient != count {
        state_mut(w, u)?.inefficient = count;
        let level = inp.imp_level(w, u)?;
        let (threshold, _) = settings(w, level);
        if count >= threshold {
            state_mut(w, u)?.hard.add(last);
        }
        if last.is_some() && state(w, u)?.hard.contains(last) {
            let level = inp.imp_level(w, u)?;
            let (_, chance) = settings(w, level);
            if inp.imp_draw_switch(w)? < chance {
                let s = state_mut(w, u)?;
                s.hard.remove(last);
                s.disliked.add(last);
                setup_priority_target(w, inp, u)?;
            }
        }
    }
    inp.imp_state(w, "upd", u)
}

/// `SetupPriorityTarget(unit)`: a weighted pick (by inverse distance) among the bother units
/// that are neither hard nor disliked, conscious and visible.
pub fn setup_priority_target<I: ImpInputs + uc::Inputs>(
    w: &mut World,
    inp: &mut I,
    u: EntityIdx,
) -> R<()> {
    state_mut(w, u)?.priority = None;
    let mut sum = 0f32;
    let mut list = Vec::new();
    let bother: Vec<Option<EntityIdx>> = state(w, u)?.bother.iter().collect();
    for b in bother {
        let s = state(w, u)?;
        if s.hard.contains(b) || s.disliked.contains(b) {
            continue;
        }
        let Some(b) = b else {
            return unsupported(w, "a null bother unit (State.IsConscious throws)");
        };
        if !super::units::is_conscious(w, b) || !visible(w, inp, u, b)? {
            continue;
        }
        let weight = store_f32(1.0 / f64::from(inp.imp_distance(w, u, b)?));
        sum = store_f32(f64::from(sum) + f64::from(weight));
        list.push((b, weight));
    }
    if sum >= 1.0 {
        let r = inp.imp_draw_setup(w, sum)?;
        let mut acc = 0f32;
        for (b, weight) in list {
            acc = store_f32(f64::from(acc) + f64::from(weight));
            if r <= acc {
                state_mut(w, u)?.priority = Some(b);
                break;
            }
        }
    }
    inp.imp_state(w, "setup", u)
}

/// `HandleUnitCommandDidStart(c)` (`UnitCommand.Start`, after `OnStart`).
pub fn command_started(w: &mut World, c: u32) {
    if !w.combat.impatience.on {
        return;
    }
    let Some(x) = w.combat.commands.cmds.get(&c) else { return };
    let exec = x.s.executor.as_deref().and_then(|e| w.by_id.get(e).copied());
    let Some(u) = exec.filter(|e| w.unit(*e).is_some()) else {
        w.coverage.unsupported("impatience: a command start without its executor");
        return;
    };
    let (ty, attack, ability) = (x.s.ty, x.is(class::ATTACK), x.is(class::USE_ABILITY));
    let target =
        x.s.target.unit().and_then(|t| w.by_id.get(t).copied()).filter(|t| w.unit(*t).is_some());
    let counts = w.combat.combat_active && ty == uc::ty::STANDARD && (attack || ability);
    if counts && ability {
        w.coverage.unsupported("impatience: an ability's start (UnitUseAbility)");
    }
    let target = match target {
        Some(t) if counts && attack && super::units::is_enemy(w, u, t) => Some(t),
        _ => None,
    };
    w.combat.impatience.queue.push_back(Op::Start { u, c, target });
}

/// The controller's and `OffensiveActionsController`'s `OnEventDidTrigger` for the rules they
/// handle: `RuleDealDamage` (the attacks without effect, then the damaged unit's bother units),
/// `RuleAttackRoll` (an offensive action: the target's bother units).
pub fn rule_did_trigger(w: &mut World, rule: &mut dyn crate::rulebook::RuleEvent) {
    use super::attack::{RuleAttackRoll, RuleDealDamage};
    if !w.combat.impatience.on {
        return;
    }
    let any = rule.as_any();
    if let Some(r) = any.downcast_mut::<RuleDealDamage>() {
        let (a, t, result, fake, roll) =
            (r.initiator, r.target, r.result, r.is_fake, r.link.roll_seq);
        if fake {
            w.coverage.unsupported("impatience: a fake damage rule");
        }
        let active = w.combat.combat_active && result >= 1 && super::units::is_enemy(w, a, t);
        let q = &mut w.combat.impatience.queue;
        if active {
            let in_combat = super::is_in_combat(w, a);
            let cond = super::is_in_combat(w, t) && super::units::is_enemy(w, t, a);
            let q = &mut w.combat.impatience.queue;
            q.push_back(Op::DamageList { initiator: a, target: t, roll, in_combat });
            q.push_back(Op::Bother { u: t, b: a, cond, damage: true });
            q.push_back(Op::DamageEnd { initiator: a });
        } else {
            q.push_back(Op::DamageEnd { initiator: a });
        }
    } else if let Some(r) = any.downcast_mut::<RuleAttackRoll>() {
        let (a, t) = (r.initiator, r.target);
        // `!evt.IsFake && (Initiator.IsEnemy(Target) || Target.Faction.Neutral)`, both suitable
        // (in the state, in the game, not fake, not disposed).
        if !(super::units::is_enemy(w, a, t) || super::units::faction_neutral(w, t)) {
            return;
        }
        let pool = crate::scene::units_pool(w);
        let suitable = |w: &World, u| pool.contains(&u) && w.entity(u).is_in_game;
        if !suitable(w, a) || !suitable(w, t) {
            return;
        }
        if w.entity(a).parts.iter().any(|p| p.clr == "UnitPartRider" || p.clr == "UnitPartSaddled")
        {
            w.coverage.unsupported("impatience: an offensive action of a rider or a mount");
            return;
        }
        memory_offensive_action(w, a, t);
        let cond = super::is_in_combat(w, t) && super::units::is_enemy(w, t, a);
        w.combat.impatience.queue.push_back(Op::Bother { u: t, b: a, cond, damage: false });
    }
}

/// `HandleUnitDeath(dead)`: queued with the awake units the records come for.
pub fn unit_death(w: &mut World, dead: EntityIdx) {
    if !w.combat.impatience.on {
        return;
    }
    if !w.combat.awake_unknown.is_empty() {
        w.coverage.unsupported("impatience: a death while the awake units aren't established");
    }
    let awake = w.combat.awake.clone();
    let active = w.combat.combat_active;
    let muts = w.events.records.iter().filter(|e| compared_mutation(e.k, &e.fields)).count();
    let q = &mut w.combat.impatience.queue;
    q.push_back(Op::Death { dead, active, awake: awake.clone(), muts });
    q.extend(awake.into_iter().map(|u| Op::DeathCheck { dead, u }));
}

/// A mutation record both T0 streams keep (damage, experience, a fact's attach or detach that
/// isn't the tutorial's): what a death's records are placed by.
#[must_use]
pub fn compared_mutation(k: &str, r: &serde_json::Map<String, serde_json::Value>) -> bool {
    k == "mut"
        && match r.get("what").and_then(serde_json::Value::as_str) {
            Some("damage" | "xp") => true,
            Some("fact_attach" | "fact_detach") => {
                r.get("class").and_then(serde_json::Value::as_str) != Some("Tutorial")
            }
            _ => false,
        }
}

/// `CombatAiData.Clear()`'s impatience part (leaving combat), in the queue's order.
pub fn clear(w: &mut World, u: EntityIdx) {
    if w.combat.impatience.on {
        w.combat.impatience.queue.push_back(Op::Clear { u });
    }
}

/// Applies a queued operation (`vis`: `TryAddBotherUnit`'s visibility read, when it made one);
/// returns `Add`'s result for a bother unit.
pub fn apply(w: &mut World, op: &Op, vis: Option<bool>) -> R<bool> {
    match op {
        Op::Start { u, c, target } => {
            if let Some(t) = target {
                let s = state_mut(w, *u)?;
                if s.last_target != Some(*t) {
                    s.last_target = Some(*t);
                    s.attacks.clear();
                }
                s.attacks.push(*c);
            }
        }
        Op::DamageList { initiator, target, roll, in_combat } => {
            if *in_combat && state(w, *initiator)?.last_target == Some(*target) {
                let attacks = state(w, *initiator)?.attacks.clone();
                for (i, c) in attacks.iter().enumerate().rev() {
                    let Some(x) = w.combat.commands.cmds.get(c) else {
                        return desync(format!(
                            "command {c} among the attacks without effect is unknown"
                        ));
                    };
                    if x.is(class::USE_ABILITY) {
                        return unsupported(w, "an ability among the attacks without effect");
                    }
                    let hit = x.is(class::ATTACK)
                        && roll.is_some_and(|r| {
                            w.combat.impatience.rolls.get(c).is_some_and(|v| v.contains(&r))
                        });
                    if hit {
                        state_mut(w, *initiator)?.attacks.drain(..=i);
                        break;
                    }
                }
            }
        }
        Op::DamageEnd { .. } | Op::DeathCheck { .. } => {}
        Op::Bother { u, b, cond, .. } => {
            // `unit.Memory.ContainsVisible(botherUnit)`, read when `cond` holds.
            let vis = vis.map(|v| {
                crate::combat::memory::sim::check(w, "impatience's ContainsVisible", v, |w| {
                    crate::combat::memory::sim::contains(w, *u, *b, true)
                })
            });
            if *cond && vis == Some(true) {
                return Ok(state_mut(w, *u)?.bother.add(Some(*b)));
            }
        }
        Op::Death { dead, active, awake, .. } => {
            if *active {
                for &u in awake {
                    state_mut(w, u)?.bother.remove(Some(*dead));
                }
            }
        }
        Op::Clear { u } => {
            let s = state_mut(w, *u)?;
            s.hard.clear();
            s.disliked.clear();
            s.bother.clear();
            s.attacks.clear();
            s.last_target = None;
            s.priority = None;
            s.inefficient = 0;
        }
    }
    Ok(false)
}

/// The AI's reads: `ImpatiencePriorityTarget` and `DislikedTargets.Contains(t)` of `u`.
pub fn priority(w: &World, u: EntityIdx) -> R<Option<EntityIdx>> {
    Ok(state(w, u)?.priority)
}

pub fn disliked(w: &World, u: EntityIdx, t: EntityIdx) -> R<bool> {
    Ok(state(w, u)?.disliked.contains(Some(t)))
}
