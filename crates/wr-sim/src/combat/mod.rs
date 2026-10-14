//! Combat (POC-5): turn-based combat headless, replayed against WrathOracle recordings.
//! See `docs/poc5/combat.md`.
//!
//! - [`oracle`]: reading a fixture trace, the per-site replay tape.
//! - [`project`]: the T0 projection both sides are compared on, and the diff.
//! - [`fire`]: triggering rules from combat code and writing the sim's T0 records.
//! - [`order`]: turn and initiative order comparers.
//! - [`cooldowns`]: units' cooldowns and the turn's movement budget.
//! - [`commands`]: units' commands (`UnitCommands`, `UnitCommandController`, the base lifecycle).
//! - [`rules`]: combat rules.
//! - [`weapon`]: weapons, dice, a unit's weapon slots.

pub mod ai;
pub mod attack;
pub mod commands;
pub mod components;
pub mod cooldowns;
pub mod counts;
pub mod engage;
pub mod fire;
pub mod groups;
pub mod initiative;
pub mod inspect;
pub mod life;
pub mod loot;
pub mod oracle;
pub mod order;
pub mod project;
pub mod replay;
pub mod rules;
pub mod turn_tick;
pub mod turns;
pub mod units;
pub mod weapon;

use crate::csharp::TimeSpan;
use crate::world::{EntityIdx, World};

/// `UnitCombatState.Cooldowns` (seconds).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Cooldowns {
    pub initiative: f32,
    pub standard_action: f32,
    pub move_action: f32,
    pub swift_action: f32,
    pub attack_of_opportunity: f32,
}

/// `UnitEntityData.CombatState` and combat membership.
#[derive(Debug, Clone, Default)]
pub struct UnitCombat {
    /// `IsInCombat`.
    pub in_combat: bool,
    /// `Initiative`, `InitiativeRandom`.
    pub initiative: order::Initiative,
    /// `Prepared` (initiative rolled).
    pub prepared: bool,
    pub cooldown: Cooldowns,
    /// `HadTurnInTMBCombat`.
    pub had_turn: bool,
    /// The engagement (`m_EngagedUnits`, `m_EngagedBy`, the attacks of opportunity's count).
    pub eng: engage::UnitEngage,
}

/// A timer (UniRx `Observable.Timer`) that removes a buff when it fires
/// (`DropLootAndDestroyOnDeactivate`). The original's fires in a coroutine after `Game.Tick`
/// once Unity's `Time.time` has passed it; the sim approximates it with `Player.RealTime` and
/// reports that (the frame it fires in is not established).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timer {
    /// `real_time` (100 ns ticks) at which it fires.
    pub due: i64,
    pub fact: crate::world::FactIdx,
}

/// Combat controllers' state.
#[derive(Debug, Default)]
pub struct CombatWorld {
    /// `CombatController.IsInTurnBasedCombat()`: [`Self::tb_mode_on`] and combat active and not a
    /// cutscene (`life::update_turn_based`).
    pub turn_based: bool,
    /// `Player.IsTurnBasedModeOn()`.
    pub tb_mode_on: bool,
    /// `Player.IsInCombat`.
    pub combat_active: bool,
    /// `Game.Instance.CurrentMode == GameModeType.Cutscene`.
    pub cutscene: bool,
    /// `CombatController.TurnStartTime`.
    pub turn_start_time: TimeSpan,
    /// Driven attack environments queued by a replay, per (initiator, target).
    pub env: Vec<((EntityIdx, EntityIdx), units::AttackEnv)>,
    /// Driven inputs consumed (reported with the run).
    pub driven_inputs: u64,
    /// `CombatController.m_KnownInitiative`: the initiative each unit rolled, by unit id,
    /// until the turn-based controller resets.
    pub known_initiative: std::collections::HashMap<String, i32>,
    /// Ranged attacks' `RuleAttackWithWeaponResolve` rules waiting for their projectile to
    /// arrive (`ProjectileController`), in launch order.
    pub pending_resolves: Vec<attack::RuleAttackWithWeaponResolve>,
    /// `CombatController.Rider`: the unit whose turn it is (turn-based).
    pub current_unit: Option<EntityIdx>,
    /// `CombatController.m_Units`, as a set (its order is the recorded `sort`'s): `None` while
    /// the controller isn't initialized (`Clear()` empties it).
    pub tb_units: Option<Vec<EntityIdx>>,
    /// The current turn the sim's `RemoveUnit` cleared, whose recorded `turn_clear` is due.
    pub cleared_by_remove: Option<EntityIdx>,
    /// The current turn has ended (`TurnController.End`) and the turn controller may already
    /// have cleared it (`CombatController.Tick` sets `CurrentTurn = null` at a point the
    /// recording doesn't show): whether [`Self::current_unit`] is still current is not
    /// established until the next frame's recorded turn status.
    pub current_unit_maybe_cleared: bool,
    /// `Game.Instance.State.AwakeUnits` for the frame, as far as known (`life::update_awake`),
    /// and the units whose awake state can't be established.
    pub awake: Vec<EntityIdx>,
    pub awake_unknown: Vec<EntityIdx>,
    /// Pending real-time timers, in the order they were started.
    pub timers: Vec<Timer>,
    /// The recording gives the frame each timer fires in (WrathOracle's `loot_timer`).
    pub timer_frames_recorded: bool,
    /// `DistanceDamageBonus` outcomes taken from a recording, by fact id, for a rule whose
    /// positions the recording doesn't establish (a driven replay counts that rule as
    /// unverified), and how many were used.
    pub recorded_distance: std::collections::HashMap<String, bool>,
    pub recorded_distance_used: usize,
    /// The recording combat run, if any (`fire::current`).
    pub recording: Option<fire::Cx>,
    /// Unit groups (`UnitGroupsController`), when modelled.
    pub groups: groups::Groups,
    /// The turn-based controller's turn order, when ported (`turns::Turns::on`).
    pub turns: turns::Turns,
    /// `LeaveCombat` calls the sim made, not yet matched with the recording's (while the turn
    /// port runs).
    pub leave_log: std::collections::VecDeque<EntityIdx>,
    /// `InspectUnitsController.m_Units`: dead units waiting for their knowledge check.
    pub inspect_queue: Vec<EntityIdx>,
    /// The cooldowns `UnitCombatState.Clear` cleared (leaving combat), not yet matched with the
    /// recording's `cd_clear`: logged when a replay computes the cooldowns.
    pub cooldown_clears: Option<std::collections::VecDeque<(EntityIdx, Cooldowns)>>,
    /// Units' commands, when modelled ([`commands::Commands::on`]).
    pub commands: commands::Commands,
    /// The AI (`AiBrainController`; docs/poc5/commands.md, slice D).
    pub ai: ai::Ai,
    /// The engagement controller (`UnitCombatEngagementController`; docs/poc5/commands.md, slice E).
    pub engage: engage::Engage,
    /// `Game.Instance.IsPaused` (the pause mode), for the commands' lifecycle.
    pub commands_paused: bool,
}

/// `UnitEntityData.IsInCombat`.
#[must_use]
pub fn is_in_combat(w: &World, u: EntityIdx) -> bool {
    w.unit(u).is_some_and(|x| x.combat.in_combat)
}

/// `UnitDescriptor.Damage` setter: `m_Damage = Math.Max(0, value)`.
pub fn set_damage(w: &mut World, u: EntityIdx, v: i32) {
    if let Some(x) = w.unit_mut(u) {
        x.damage = v.max(0);
    }
}

/// The time a buff's duration and ticks count from: game time, or the turn's start time
/// in turn-based combat (`BuffCollection.PrepareFactForAttach`, `Buff.OnAttach`).
#[must_use]
pub fn buff_start_time(w: &World) -> TimeSpan {
    if w.combat.turn_based { w.combat.turn_start_time } else { w.clock.game_time }
}
