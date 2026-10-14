//! Units' cooldowns (decompiled, not copied): `UnitCombatCooldownsController`,
//! `UnitEntityData.SpendAction` and `GetTimeToNextTurn`, and the turn controller's writes:
//! `TurnController.Prepare`'s clear, `End`, `ForceToEnd`, `HandleUnitCommandDidEnd` and
//! `TickMovement` with the turn's movement stats and budget (`GetRemainingMovementTime`,
//! `GetRemainingTime`, the five-foot step).
//!
//! What decides them that the sim doesn't run yet is a recorded input (WrathOracle's `cd_*`
//! records, read by the replay): a command acting (`SpendAction`'s call), the movement agent's
//! delta (`TickMovement`'s call), the end-turn button or the AI ending its turn (`ForceToEnd`), a
//! command ending, and the turn's movement limit (the UI's and the AI command's choice). Mounts
//! aren't modelled: a turn with a mount is reported.

use super::Cooldowns;
use crate::unit_state::{cond, has_condition};
use crate::world::{EntityIdx, World};

/// `TurnController.MetersOfFiveFootStep`: `7.5f.Feet().Meters` (`7.5f * 0.3048f`).
pub const METERS_OF_FIVE_FOOT_STEP: f32 = 7.5 * 0.3048;

/// `UnitCommand.CommandType`.
pub mod command_type {
    pub const FREE: i64 = 0;
    pub const STANDARD: i64 = 1;
    pub const SWIFT: i64 = 2;
    pub const MOVE: i64 = 3;
}

/// `TurnController.MovementLimit`.
pub mod limit {
    pub const TWO_ACTIONS: i64 = 0;
    pub const FIVE_FOOT_STEP: i64 = 1;
    pub const ONE_ACTION: i64 = 2;
}

/// `TurnController.MovementStats` (the rider's; a new turn controller starts them at zero).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MovementStats {
    pub time_moved: f32,
    pub time_moved_in_force_mode: f32,
    pub time_moved_by_five_foot_step: f32,
    pub meters_moved_by_five_foot_step: f32,
}

/// What a `TickMovement` call reads that the sim doesn't compute: the movement agent's delta and
/// force mode and its `NearTheEnd`, the turn's movement limit state (`CurrentMovementLimit`,
/// `m_ModifyMovementLimit`'s current and requested values and whether the unit is moving,
/// `m_AIUsedFiveFootStep`, `m_AutoStopAfterFirstMoveAction`), the unit's speed
/// (`CurrentSpeedMps`, when the oracle could read it) and panic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveInput {
    pub dt: f32,
    pub force: bool,
    pub near_the_end: bool,
    pub limit: i64,
    pub modify_current: bool,
    pub modify_request: bool,
    pub moving: bool,
    pub ai_used_five_foot_step: bool,
    pub auto_stop: bool,
    pub speed: Option<f32>,
    pub panicked: bool,
}

fn cooldown(w: &World, u: EntityIdx) -> Cooldowns {
    w.unit(u).map(|x| x.combat.cooldown).unwrap_or_default()
}

fn cooldown_mut(w: &mut World, u: EntityIdx) -> Option<&mut Cooldowns> {
    w.unit_mut(u).map(|x| &mut x.combat.cooldown)
}

/// `UnitEntityData.GetTimeToNextTurn()`: `Cooldown.Initiative + Max(StandardAction, MoveAction)`.
#[must_use]
pub fn time_to_next_turn(w: &World, u: EntityIdx) -> f32 {
    let c = cooldown(w, u);
    c.initiative + c.standard_action.max(c.move_action)
}

fn info(w: &World, u: EntityIdx) -> Option<&super::order::TbUnitInfo> {
    w.combat.turns.units.iter().find(|i| i.unit == u)
}

/// `CombatController.IsSurprised(unit)`: in the surprise round (round 0 of a combat that has one).
#[must_use]
pub fn is_surprised(w: &World, u: EntityIdx) -> bool {
    let t = &w.combat.turns;
    t.surprise_round && t.round == 0 && info(w, u).is_some_and(|i| i.surprised)
}

pub(crate) fn is_surprising(w: &World, u: EntityIdx) -> bool {
    let t = &w.combat.turns;
    t.surprise_round && t.round == 0 && info(w, u).is_some_and(|i| i.surprising)
}

fn is_acting_surprise_commands(w: &World, u: EntityIdx) -> bool {
    let t = &w.combat.turns;
    t.surprise_round && t.round == 0 && info(w, u).is_some_and(|i| i.acting_in_surprise_round)
}

/// `UnitEntityData.IsMoveActionRestricted()`.
pub(crate) fn is_move_action_restricted(w: &World, u: EntityIdx) -> bool {
    if !is_surprising(w, u) || is_acting_surprise_commands(w, u) {
        return has_condition(w, u, cond::STAGGERED);
    }
    true
}

/// `UnitEntityData.UsedStandardAction()`.
fn used_standard_action(w: &World, u: EntityIdx) -> bool {
    cooldown(w, u).standard_action > 0.0
        || has_condition(w, u, cond::NAUSEATED)
        || has_condition(w, u, cond::CANT_USE_STANDARD_ACTIONS)
}

/// `UnitEntityData.HasFullRoundAction()`.
pub(crate) fn has_full_round_action(w: &World, u: EntityIdx) -> bool {
    !used_standard_action(w, u) && !used_one_move_action(w, u)
}

/// `UnitEntityData.UsedOneMoveAction()`.
pub(crate) fn used_one_move_action(w: &World, u: EntityIdx) -> bool {
    cooldown(w, u).move_action > 0.0 || is_move_action_restricted(w, u)
}

/// `UnitEntityData.UsedTwoMoveAction()`.
fn used_two_move_action(w: &World, u: EntityIdx) -> bool {
    cooldown(w, u).move_action > if is_move_action_restricted(w, u) { 0.0 } else { 3.0 }
}

/// `UnitEntityData.HasStandardAction()`.
fn has_standard_action(w: &World, u: EntityIdx) -> bool {
    !used_standard_action(w, u) && !used_two_move_action(w, u)
}

/// `UnitEntityData.HasMoveAction()`.
fn has_move_action(w: &World, u: EntityIdx) -> bool {
    if used_standard_action(w, u) {
        !used_one_move_action(w, u)
    } else {
        !used_two_move_action(w, u)
    }
}

/// `UnitCombatState.HasCooldownForCommand(commandType)`; `move_running` is whether the unit's move
/// command runs (`Unit.Commands.GetCommand(Move)?.IsRunning`).
pub fn has_cooldown_for_type(
    w: &World,
    u: EntityIdx,
    ty: u8,
    move_running: impl Fn(&World, EntityIdx) -> bool,
) -> bool {
    let c = cooldown(w, u);
    let t = i64::from(ty);
    if w.combat.turn_based && super::is_in_combat(w, u) {
        return match t {
            command_type::FREE => false,
            command_type::MOVE => !has_move_action(w, u),
            command_type::STANDARD => move_running(w, u) || !has_standard_action(w, u),
            _ => c.swift_action > 0.0,
        };
    }
    match t {
        command_type::FREE => false,
        command_type::MOVE => c.move_action > 0.0,
        command_type::STANDARD => move_running(w, u) || c.standard_action > 0.0,
        _ => c.swift_action > 0.0,
    }
}

/// `UnitEntityData.SpendAction(type, isFullRound, timeSinceCommandStart)`. Returns false for a
/// command type it doesn't know (the game throws).
pub fn spend_action(w: &mut World, u: EntityIdx, ty: i64, full_round: bool, since: f32) -> bool {
    if !super::is_in_combat(w, u) {
        return true;
    }
    let tb = w.combat.turn_based;
    let Some(c) = cooldown_mut(w, u) else { return true };
    match (tb, ty) {
        (_, command_type::FREE) => {}
        (true, command_type::MOVE) => c.move_action += 3.0,
        (true, command_type::STANDARD) => {
            c.standard_action += 6.0;
            if full_round {
                c.move_action += 3.0;
            }
        }
        (true, command_type::SWIFT) => c.swift_action += 6.0,
        (false, command_type::MOVE) => c.move_action = 3.0 - since,
        (false, command_type::STANDARD) => {
            c.standard_action = 6.0 - since;
            c.move_action = 3.0;
        }
        (false, command_type::SWIFT) => c.swift_action = 6.0 - since,
        _ => return false,
    }
    true
}

/// `UnitCombatCooldownsController.TickOnUnit` for the awake units (`BaseUnitController`): alive
/// and in combat (`BaseUnitCombatController`). In turn-based combat the cooldowns run down only
/// while time passes between turns, the initiative first (not a surprised unit's); otherwise all
/// run down, a unit waiting for its initiative only that.
pub fn controller_tick(w: &mut World) {
    let ticks = |w: &World, u: EntityIdx| !super::units::is_dead(w, u) && super::is_in_combat(w, u);
    for u in w.combat.awake_unknown.clone() {
        if ticks(w, u) && cooldown(w, u) != Cooldowns::default() {
            w.coverage.unsupported(
                "UnitCombatCooldownsController: a unit in combat whose awake state isn't established",
            );
        }
    }
    for u in w.combat.awake.clone() {
        if ticks(w, u) {
            tick_on_unit(w, u);
        }
    }
}

fn tick_on_unit(w: &mut World, u: EntityIdx) {
    let dt = w.clock.game_delta_time();
    if w.combat.turn_based {
        // `TickOnUnitTurnBased`: the unit is in combat, so it returns false.
        if !super::turns::is_passing(w) {
            return;
        }
        let surprised = is_surprised(w, u);
        let Some(c) = cooldown_mut(w, u) else { return };
        let mut num = dt;
        if c.initiative > 0.0 && !surprised {
            if num >= c.initiative {
                num -= c.initiative;
                c.initiative = 0.0;
            } else {
                c.initiative -= num;
                num = 0.0;
            }
        }
        if num > 0.0 {
            c.standard_action = (c.standard_action - num).max(0.0);
            c.move_action = (c.move_action - num).max(0.0);
            c.swift_action = (c.swift_action - num).max(0.0);
            c.attack_of_opportunity = (c.attack_of_opportunity - num).max(0.0);
        }
        return;
    }
    // `IsWaitingInitiative` outside turn-based combat (mounts aren't modelled: no rider).
    let Some(c) = cooldown_mut(w, u) else { return };
    if c.initiative > 0.0 {
        c.initiative = (c.initiative - dt).max(0.0);
        return;
    }
    let had_standard = c.standard_action > 0.0;
    c.standard_action = (c.standard_action - dt).max(0.0);
    c.move_action = (c.move_action - dt).max(0.0);
    c.swift_action = (c.swift_action - dt).max(0.0);
    c.attack_of_opportunity = (c.attack_of_opportunity - dt).max(0.0);
    let refill = c.attack_of_opportunity <= 0.0;
    let ended = had_standard && c.standard_action <= 0.0;
    // The attacks of opportunity's count refill: the engagement's model, when it runs (else a use
    // of one is a reported cooldown write).
    if refill {
        super::engage::refill(w, u);
    }
    if ended {
        new_combat_round(w, u, "UnitCombatCooldownsController");
    }
}

/// `UnitCombatState.OnNewRound()` and `IUnitNewCombatRoundHandler.HandleNewCombatRound(unit)`:
/// the combat state's round counters aren't modelled, and the handlers are reported where the
/// unit has one (`NewRoundTrigger`, `ShieldWall`, `ShifterGrabInitiatorBuff`; the kineticist's
/// burn isn't modelled).
pub fn new_combat_round(w: &mut World, u: EntityIdx, from: &str) {
    const HANDLERS: [&str; 3] = ["NewRoundTrigger", "ShieldWall", "ShifterGrabInitiatorBuff"];
    for name in HANDLERS {
        if has_component(w, u, |n| n == name) {
            w.coverage.unsupported(format!("{from}: HandleNewCombatRound: {name}"));
        }
    }
}

/// `UnitTicksController.TickNextRound(unit)` from `TurnController.Prepare`: the AI's round counters
/// (not modelled) and `ITickEachRound.OnNewRound` of the unit's facts other than buffs. Only
/// components whose `OnNewRound` is empty are run; the others are reported.
pub fn tick_next_round(w: &mut World, u: EntityIdx) {
    let facts: Vec<_> = w
        .entity(u)
        .facts
        .list
        .iter()
        .copied()
        .filter(|f| w.fact(*f).kind != crate::world::FactKind::Buff)
        .collect();
    for f in facts {
        for c in crate::facts::comps(w, f) {
            let Some(ty) = w.component(c).ty() else { continue };
            let name = wr_content::schema::ty(ty).short;
            if !crate::unit_facts::ticks_each_round(name) {
                continue;
            }
            match name {
                // `ImpatienceWatcherTickingResolve.OnNewRound` is empty.
                "ImpatienceWatcherTickingResolve" => {}
                // `AddFactContextActions.OnNewRound` runs its `NewRound` actions: none, nothing.
                "AddFactContextActions"
                    if !crate::components::mechanics::fact_context_actions_any(
                        w, c, "NewRound",
                    ) => {}
                _ => w.coverage.unsupported(format!(
                    "UnitTicksController.TickNextRound: ITickEachRound.OnNewRound: {name}"
                )),
            }
        }
    }
}

fn has_component(w: &World, u: EntityIdx, pred: impl Fn(&str) -> bool) -> bool {
    w.entity(u).facts.list.iter().any(|f| {
        crate::facts::comps(w, *f)
            .into_iter()
            .any(|c| w.component(c).ty().is_some_and(|ty| pred(wr_content::schema::ty(ty).short)))
    })
}

/// `TurnController.Prepare`'s cooldown part for the rider: `m_RiderCooldown.Clear()` (the acting
/// commands' `SpendAction` follow as recorded calls).
pub fn prepare_clear(w: &mut World, u: EntityIdx) {
    if let Some(c) = cooldown_mut(w, u) {
        *c = Cooldowns::default();
    }
}

/// `TurnController.End()`: the rider's standard action spent, its move action at most a round.
pub fn end(w: &mut World, u: EntityIdx) {
    if let Some(c) = cooldown_mut(w, u) {
        c.standard_action = 6.0;
        c.move_action = c.move_action.min(6.0);
    }
}

/// `TurnController.ForceToEnd(setCooldowns)`'s writes (then `ToEnd()`: the turn is ending).
pub fn force_to_end(w: &mut World, u: EntityIdx, set_cooldowns: bool) {
    if !set_cooldowns {
        return;
    }
    w.combat.turns.movement.meters_moved_by_five_foot_step = METERS_OF_FIVE_FOOT_STEP;
    if let Some(c) = cooldown_mut(w, u) {
        c.standard_action = 6.0;
        c.move_action = 3.0;
        c.swift_action = 6.0;
    }
}

/// `TurnController.HandleUnitCommandDidEnd(command)`'s writes for the rider's command, given
/// `PointerController.SimulatingClick`, the turn's status and the command's
/// `DoesCommandEmptiesMovement()` as the handler evaluated it. Returns false when the handler
/// evaluates it and it isn't known (nothing is changed then).
pub fn command_did_end(
    w: &mut World,
    u: EntityIdx,
    simulating: bool,
    status: &str,
    empties: Option<bool>,
) -> bool {
    if simulating || matches!(status, "Preparing" | "Scrolling") {
        return true;
    }
    let Some(empties) = empties else { return false };
    if !empties {
        return true;
    }
    if let Some(c) = cooldown_mut(w, u)
        && c.move_action > 0.0
        && c.move_action < 3.0
    {
        c.move_action = 3.0;
    }
    let m = &mut w.combat.turns.movement;
    if m.meters_moved_by_five_foot_step > 0.0
        && m.meters_moved_by_five_foot_step < METERS_OF_FIVE_FOOT_STEP
    {
        m.meters_moved_by_five_foot_step = METERS_OF_FIVE_FOOT_STEP;
    }
    true
}

/// `TurnController.m_ModifyMovementLimit`: its getter takes the requested value unless the turn's
/// units are moving.
fn modify_movement_limit(i: &MoveInput) -> bool {
    if i.modify_current != i.modify_request && !i.moving {
        i.modify_request
    } else {
        i.modify_current
    }
}

/// `TurnController.GetEnabledFiveFootStep(unit)`.
fn enabled_five_foot_step(i: &MoveInput, m: &MovementStats) -> bool {
    if i.limit == limit::FIVE_FOOT_STEP || modify_movement_limit(i) {
        return if i.panicked { m.meters_moved_by_five_foot_step > 0.0 } else { true };
    }
    false
}

/// `TurnController.ShouldRestrictFiveFootStep(unit)` (the C#'s negated comparisons kept); `None`
/// when it reads the unit's speed and that isn't known. Mono compares `speed * 3f` in double.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn should_restrict_five_foot_step(m: &MovementStats, speed: Option<f32>) -> Option<bool> {
    if !(m.time_moved > 0.0) || m.time_moved_by_five_foot_step != 0.0 {
        return Some(f64::from(speed?) * 3.0 <= f64::from(METERS_OF_FIVE_FOOT_STEP));
    }
    Some(true)
}

/// Whether the current turn's `HasFiveFootStep` must be false whatever the unit's speed:
/// `ShouldRestrictFiveFootStep` after a normal move, or the step used up.
pub(crate) fn five_foot_step_ruled_out(w: &World) -> bool {
    let m = &w.combat.turns.movement;
    should_restrict_five_foot_step(m, None) == Some(true)
        || m.meters_moved_by_five_foot_step >= METERS_OF_FIVE_FOOT_STEP
}

/// `TurnController.ShouldRestrictNormalMovement(unit)` (likewise).
#[allow(clippy::neg_cmp_op_on_partial_ord)]
fn should_restrict_normal_movement(i: &MoveInput, m: &MovementStats) -> bool {
    if i.ai_used_five_foot_step || !(m.meters_moved_by_five_foot_step > 0.0) {
        return m.time_moved_in_force_mode > 0.0;
    }
    true
}

/// `TurnController.GetRemainingTime(unit)`: the move actions left, in seconds.
pub fn remaining_time(w: &World, u: EntityIdx) -> f32 {
    let moves = if is_move_action_restricted(w, u) { 1.0 } else { 2.0 };
    let standard = if used_standard_action(w, u) { 1.0 } else { 0.0 };
    (3.0_f32 * (moves - standard) - cooldown(w, u).move_action).clamp(0.0, 6.0)
}

/// `TurnController.GetRemainingMovementTime(unit, total, singleActionMove)`; `None` when it needs
/// the unit's speed and the recording doesn't have it.
fn remaining_movement_time(
    w: &World,
    u: EntityIdx,
    i: &MoveInput,
    m: &MovementStats,
    total: bool,
    single: bool,
) -> Option<f32> {
    if enabled_five_foot_step(i, m) && !single {
        // `HasFiveFootStep(unit)`.
        let has = !should_restrict_five_foot_step(m, i.speed)?
            && m.meters_moved_by_five_foot_step < METERS_OF_FIVE_FOOT_STEP;
        if !has {
            return Some(0.0);
        }
        return Some((METERS_OF_FIVE_FOOT_STEP - m.meters_moved_by_five_foot_step) / i.speed?);
    }
    if !should_restrict_normal_movement(i, m) {
        let rt = remaining_time(w, u);
        if single {
            if rt > 3.0 {
                return Some(rt - 3.0);
            }
            if cooldown(w, u).move_action > 0.0 {
                return Some(0.0);
            }
            return Some(rt);
        }
        if !total && rt > 3.0 {
            return Some(rt - 3.0);
        }
        if rt > 0.0 {
            return Some(rt);
        }
    }
    Some(0.0)
}

/// `TurnController.TickMovement(unit, ref deltaTime, isInForceMode)` for the rider: the delta it
/// lets the agent move (clamped to the turn's movement budget unless the agent is near its path's
/// end), counted as movement time and spent from the move action or the five-foot step. Returns
/// the delta out; `None` when it needs the unit's speed and the recording doesn't have it (nothing
/// is changed then). Selecting the movement limit (`TrySelectMovementLimit`, the UI's
/// `SwitchStop`, the auto-stop flag) is the recording's: each call's limit state is an input.
pub fn tick_movement(w: &mut World, u: EntityIdx, i: &MoveInput) -> Option<f32> {
    let mut m = w.combat.turns.movement;
    let mut dt = i.dt;
    if i.force {
        m.time_moved += dt;
        m.time_moved_in_force_mode += dt;
        w.combat.turns.movement = m;
        return Some(dt);
    }
    let single = i.limit == limit::ONE_ACTION && !i.panicked;
    let remaining = remaining_movement_time(w, u, i, &m, false, single)?;
    if dt >= remaining && !i.near_the_end {
        dt = remaining;
        if dt <= 0.0 {
            return Some(dt);
        }
    }
    if i.auto_stop {
        return Some(0.0);
    }
    m.time_moved += dt;
    if enabled_five_foot_step(i, &m) {
        let speed = i.speed?;
        w.combat.turns.immune_on_disengage = Some(true);
        m.time_moved_by_five_foot_step += dt;
        // Mono evaluates `meters += deltaTime * speed` in double and rounds on the store.
        #[allow(clippy::cast_possible_truncation)]
        {
            m.meters_moved_by_five_foot_step = (f64::from(m.meters_moved_by_five_foot_step)
                + f64::from(dt) * f64::from(speed))
                as f32;
        }
    } else if let Some(c) = cooldown_mut(w, u) {
        c.move_action += dt;
    }
    w.combat.turns.movement = m;
    Some(dt)
}

#[cfg(test)]
mod tests;
