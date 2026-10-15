//! The turn controller's status (decompiled, not copied): `TurnController.Tick`'s status change
//! and end wait (`Preparing` → `Acting` → `Ending` → `Ended`), `Prepare`'s status, `ToEnd`,
//! `ContinueActing`, `ContinueWaiting`, `IsAllActed`/`IsActed` and the counters they keep
//! (`TimeWaitedForIdleAI`, `TimeWaitedToEndTurn`, `FramesWaitedForStuckAI`, `AIForcedTickCount`).
//!
//! What these read that the sim doesn't run yet is a recorded input (WrathOracle's `tt*` records,
//! read by the replay): whether commands are running or queued, the view's get-up animation, a
//! launched projectile, the movement agent (motion this tick, `WantsToMove`), whether the unit can
//! act (`IsAbleToAct`: the animation manager), whether it is directly controllable, the AI's choice
//! in its forced tick, the camera's scroll, the frame's time. When the turn ends follows from them
//! and the sim's cooldowns. Mounts aren't modelled: a turn with a mount is reported.

use super::cooldowns::METERS_OF_FIVE_FOOT_STEP;
use crate::world::{EntityIdx, World};

/// `TurnController.TurnStatus`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    None,
    Scrolling,
    Preparing,
    Acting,
    Delayed,
    Ending,
    Ended,
}

impl Status {
    const ALL: [(Self, &'static str); 7] = [
        (Self::None, "None"),
        (Self::Scrolling, "Scrolling"),
        (Self::Preparing, "Preparing"),
        (Self::Acting, "Acting"),
        (Self::Delayed, "Delayed"),
        (Self::Ending, "Ending"),
        (Self::Ended, "Ended"),
    ];

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.iter().find(|(_, n)| *n == s).map(|(x, _)| *x)
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        Self::ALL.iter().find(|(x, _)| *x == self).map_or("?", |(_, n)| n)
    }
}

/// The turn controller's counters.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Counters {
    /// `TimeWaitedForIdleAI`.
    pub idle: f32,
    /// `TimeWaitedToEndTurn`.
    pub end_wait: f32,
    /// `FramesWaitedForStuckAI`.
    pub stuck: i32,
    /// `AIForcedTickCount`.
    pub ai_forced: i32,
}

impl Counters {
    /// WrathOracle's `idle|end_wait|stuck|ai_forced` (round-trip floats).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.split('|');
        let c = Self {
            idle: it.next()?.parse().ok().filter(|x: &f32| x.is_finite())?,
            end_wait: it.next()?.parse().ok().filter(|x: &f32| x.is_finite())?,
            stuck: it.next()?.parse().ok()?,
            ai_forced: it.next()?.parse().ok()?,
        };
        it.next().is_none().then_some(c)
    }
}

/// The frame's time as the turn controller reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Time {
    /// `TimeController.GameDeltaTime`.
    pub game_delta: f32,
    /// `TimeController.DeltaTime`.
    pub delta: f32,
    /// `TimeController.PlayerTimeScale`.
    pub player_scale: f32,
    /// `TimeController.DebugTimeScale`.
    pub debug_scale: f32,
    /// `m_AnimationTimeDevider`: `Player.UISettings.TimeScaleAverage`.
    pub animation_divider: f32,
}

impl Time {
    /// WrathOracle's `game_delta|delta|player_scale|debug_scale|time_scale_average`; finite
    /// values only (the waits compare with `Math.Max`, which a NaN would decide).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let mut it = s.split('|').map(|x| x.parse::<f32>().ok().filter(|v| v.is_finite()));
        let t = Self {
            game_delta: it.next()??,
            delta: it.next()??,
            player_scale: it.next()??,
            debug_scale: it.next()??,
            animation_divider: it.next()??,
        };
        it.next().is_none().then_some(t)
    }
}

/// `TurnController.m_IdleWaitTimeBaseValue`.
const IDLE_WAIT_TIME_BASE: f32 = 0.2;
/// `TurnController.FramesToWaitForStuckAI`.
const FRAMES_TO_WAIT_FOR_STUCK_AI: i32 = 15;
/// `TurnController.TimeToWaitForEndingTurnBaseValue`.
const TIME_TO_WAIT_FOR_ENDING_TURN_BASE: f32 = 0.03;
/// `AIForcedTickCount`'s limit before the tick ends a stuck AI's turn.
const AI_FORCED_TICK_LIMIT: i32 = 50;

/// Where the current `TurnController.Tick` is (the replay follows it through its records).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Stage {
    /// Not in a tick.
    #[default]
    Out,
    /// Before the status change: the scroll's end (`Prepare`) or the forced AI tick.
    Start,
    /// The status change's start is recorded; its `IsAbleToAct` calls follow.
    StatusPending,
    /// The status change will call `ContinueActing` for the rider.
    ContinueActing,
    /// The status change is over: the end wait follows.
    StatusDone,
    /// The end wait is over.
    Done,
}

/// The current turn controller's state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnCtl {
    pub status: Status,
    pub counters: Counters,
    /// `m_WaitForGetUp`.
    pub wait_for_get_up: bool,
    /// `m_ActingInSurpriseRound`: the unit's `TBUnitInfo.ActingInSurpriseRound` as its turn started.
    pub acting_in_surprise_round: bool,
    /// The tick in progress.
    pub stage: Stage,
    /// The tick's start calls the forced AI tick (`AiBrainController.ForceTick`).
    pub ai_due: bool,
    /// `SavedCommandPath_ForSave`: the path a command's approach last followed (`TickApproaching`).
    pub saved_command_path: Option<Vec<[f32; 3]>>,
    /// The last `ContinueActing`'s `CheckAndClearWantsToMoveInTurnBased()` cleared the agent's
    /// `m_NoMoreTurnBasedMovementForUinterruptableCommand` (the replay's agent applies it).
    pub cleared_no_more: bool,
}

/// `TurnController.Prepare`'s status: a controllable rider prepares, any other acts; it waits for
/// a prone rider to get up; the stuck-AI frames restart.
pub fn prepare(t: &mut TurnCtl, controllable: bool, prone: bool) {
    t.wait_for_get_up = prone;
    t.status = if controllable { Status::Preparing } else { Status::Acting };
    t.counters.stuck = 0;
}

/// `TurnController.ToEnd()`.
pub fn to_end(t: &mut TurnCtl) {
    t.status = Status::Ending;
}

/// The tick's forced AI tick, with whether the rider has commands after it (the AI's choice):
/// counted, and a turn whose AI keeps deciding ends.
pub fn after_forced_ai_tick(t: &mut TurnCtl, commands_empty: bool) {
    if commands_empty {
        return;
    }
    t.counters.ai_forced += 1;
    if t.counters.ai_forced > AI_FORCED_TICK_LIMIT {
        to_end(t);
    } else {
        t.counters.idle = 0.0;
    }
}

/// `TurnController.IsActed(unit)` for the rider: moved, or any action's cooldown running.
#[must_use]
pub fn is_acted(w: &World, u: EntityIdx) -> bool {
    let c = w.unit(u).map(|x| x.combat.cooldown).unwrap_or_default();
    w.combat.turns.movement.time_moved > 0.0
        || c.standard_action > 0.0
        || c.move_action > 0.0
        || c.swift_action > 0.0
}

/// A recorded input the sim needed that the recording doesn't have (the game didn't evaluate it
/// there, or the record lost it): its name.
pub type Missing = &'static str;

fn need(v: Option<bool>, what: Missing) -> Result<bool, Missing> {
    v.ok_or(what)
}

/// What the status change reads that the sim doesn't compute. The values the game reads only
/// under a condition are recorded only under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusInput {
    /// The rider's `Commands.Empty` (`m_RiderCommands`).
    pub commands_empty: bool,
    /// The rider's `IsAbleToAct()` (a preparing turn whose rider hasn't acted and has no command).
    pub able: Option<bool>,
    /// `HasRunningCommands`.
    pub running: bool,
    /// `ProjectileController.HasLaunchedProjectile(Rider, Mount)` (an acting turn waiting for
    /// neither a get-up nor a running command).
    pub projectile: Option<bool>,
}

/// The tick's status change up to `ContinueActing`: a preparing turn acts once the rider has
/// acted, has a command or can't act; an acting turn whose rider waits for nothing asks
/// `ContinueActing` (returns true then). Without a mount (`m_MountCommands.Empty`).
///
/// # Errors
///
/// An input it reads isn't recorded (nothing is changed then).
pub fn status_change(t: &mut TurnCtl, all_acted: bool, i: StatusInput) -> Result<bool, Missing> {
    if t.status == Status::Preparing
        && (all_acted || !i.commands_empty || !need(i.able, "the rider's IsAbleToAct")?)
    {
        t.status = Status::Acting;
    }
    if t.status != Status::Acting || t.wait_for_get_up || i.running {
        return Ok(false);
    }
    Ok(!need(i.projectile, "HasLaunchedProjectile")?)
}

/// What `ContinueActing(unit)` reads that the sim doesn't compute. `able`, `controllable` and
/// `extra_action` are what the game computed during the call (`None` where it didn't).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActingInput {
    /// `unit.Commands.IsRunning()`.
    pub running: bool,
    /// `unit.View.IsGetUp`.
    pub get_up: bool,
    /// `Descriptor.State.IsPanicked`.
    pub panicked: bool,
    /// `IsAbleToAct()`.
    pub able: Option<bool>,
    /// `UnitCanGetUpOnCommand.Value`.
    pub can_get_up_on_command: bool,
    /// `IsDirectlyControllable`.
    pub controllable: Option<bool>,
    /// `HasExtraAction(unit)` (the controllable unit's spell combat or spell strike, or a turn
    /// that doesn't end by itself: the magus' state isn't modelled).
    pub extra_action: Option<bool>,
    /// `m_ActingInSurpriseRound`.
    pub surprise_round: bool,
    /// `HasMotionThisTick`.
    pub motion: bool,
    /// `View.AgentASP.WantsToMove`.
    pub wants_to_move: bool,
    /// The agent's `m_NoMoreTurnBasedMovementForUinterruptableCommand`.
    pub no_more_movement: bool,
    pub time: Time,
}

/// What the sim computes for `ContinueActing(unit)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ActingState {
    /// `unit.IsInCombat`.
    pub in_combat: bool,
    /// `GetRemainingTime(unit)`.
    pub remaining_time: f32,
    /// `GetMetersMovedByFiveFootStep(unit)`.
    pub meters_five_foot: f32,
}

/// `TurnController.ContinueActing(unit)` (its `TrySelectMovementLimit` is the UI's: the limit is
/// an input where it's read). An AI's turn waits while its unit moves or wants to (up to
/// `FramesToWaitForStuckAI` frames without motion) and then idles a little.
/// `CheckAndClearWantsToMoveInTurnBased()` in turn-based combat: wants to move and isn't held by
/// an uninterruptable command (whose flag it clears, read again at the next call).
///
/// # Errors
///
/// An input it reads isn't recorded (the counters may have changed then).
pub fn continue_acting(t: &mut TurnCtl, i: &ActingInput, s: &ActingState) -> Result<bool, Missing> {
    t.cleared_no_more = false;
    let busy = i.running || i.get_up;
    if i.panicked
        && s.meters_five_foot > 0.0
        && METERS_OF_FIVE_FOOT_STEP - s.meters_five_foot < 0.01
    {
        return Ok(false);
    }
    if !s.in_combat
        || (!need(i.able, "IsAbleToAct")? && !i.can_get_up_on_command)
        || (!busy && s.remaining_time <= 0.0 && !need(i.extra_action, "HasExtraAction")?)
    {
        return Ok(false);
    }
    if !need(i.controllable, "IsDirectlyControllable")? || i.surprise_round {
        if !i.motion && i.wants_to_move {
            t.counters.stuck += 1;
            if t.counters.stuck > FRAMES_TO_WAIT_FOR_STUCK_AI {
                return Ok(false);
            }
        } else {
            t.counters.stuck = 0;
        }
        let wants = i.wants_to_move && !i.no_more_movement;
        // (Evaluated only when not busy and without motion: it clears the flag it reads.)
        t.cleared_no_more = !busy && !i.motion && i.wants_to_move && i.no_more_movement;
        if !busy && !i.motion && !wants {
            // `float val = base / divider * PlayerTimeScale * DebugTimeScale`: Mono evaluates the
            // expression wide and rounds once, at the store.
            let wait = crate::csharp::arith::store_f32(
                f64::from(IDLE_WAIT_TIME_BASE) / f64::from(i.time.animation_divider)
                    * f64::from(i.time.player_scale)
                    * f64::from(i.time.debug_scale),
            );
            t.counters.idle += i.time.game_delta;
            if t.counters.idle > wait.max(i.time.delta) {
                return Ok(false);
            }
        } else {
            t.counters.idle = 0.0;
        }
    }
    Ok(true)
}

/// What `ContinueWaiting()` reads that the sim doesn't compute.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaitingInput {
    /// The rider's `m_RiderCommands.IsRunning()`.
    pub running: bool,
    /// `Rider.View.IsGetUp`.
    pub get_up: bool,
    pub time: Time,
}

/// `TurnController.ContinueWaiting()` (without a mount): an ending turn waits for its commands,
/// then a little more (`TimeToWaitForEndingTurn`).
pub fn continue_waiting(t: &mut TurnCtl, i: &WaitingInput) -> bool {
    if !i.running || i.get_up {
        let wait = TIME_TO_WAIT_FOR_ENDING_TURN_BASE / i.time.animation_divider;
        t.counters.end_wait += i.time.game_delta;
        if t.counters.end_wait > wait.max(i.time.delta) {
            return false;
        }
    } else {
        t.counters.end_wait = 0.0;
    }
    true
}

#[cfg(test)]
mod tests;
