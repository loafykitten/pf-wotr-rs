//! The turn-based combat controller's turn order (decompiled, not copied):
//! `TurnBased.Controllers.CombatController`'s `m_Units` (`TBUnitInfo`), `m_NextUnit`,
//! `ChooseNextUnit`, `StartRound`, `SortUnits`, `AddUnit`/`RemoveUnit`, `m_IsUnitsChanged` with
//! `HasEnemyInCombat`, `TickTime`'s `m_IsPassing` and `IsPassing()`.
//!
//! The port runs for recordings with WrathOracle's `choose_next` records ([`Turns::on`]). The
//! turn events it makes (`CurrentTurn = null`, `SortUnits`, a new round) are logged in order
//! ([`TbEvent`]) for the replay to match against the recording's `tb` records. What decides a
//! turn order that the sim can't compute is a recorded input: `IsVisibleForPlayer` (fog of war,
//! the view) at each `ChooseNextUnit` call, and the surprise flags set at combat start (from
//! commands). When a turn starts and ends is the recording's (`StartTurn` follows the units'
//! cooldowns and `TurnController.End` their commands): the port checks that a turn starts with
//! the unit it chose.

use std::collections::VecDeque;

use super::cooldowns::time_to_next_turn;
use super::order::{Initiative, TbUnitInfo, tb_compare};
use crate::world::{EntityIdx, World};

/// A turn event the sim made, in the vocabulary of WrathOracle's `tb` records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TbEvent {
    /// `CurrentTurn = null` (`turn_clear`): the turn's rider.
    Clear(EntityIdx),
    /// `SortUnits(reason)` (`sort`): the units in their new order, each with its
    /// `Initiative`, `InitiativeOverride` and `InitiativeSequence` as sorted.
    Sort { reason: String, units: Vec<(EntityIdx, i32, Option<i32>, i32)> },
    /// `StartRound` advanced the round (`round_start`): the new round number.
    RoundStart(i32),
}

/// One unit of a recorded `choose_next` (`ChooseNextUnit`'s prefix).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChooseUnit {
    pub id: String,
    pub surprised: bool,
    pub acting: bool,
    pub marked: bool,
    /// `IsVisibleForPlayer`; `None` when the oracle didn't find the unit.
    pub visible: Option<bool>,
    /// `SaddledPart != null` (the unit can carry a rider).
    pub saddled: bool,
}

/// A recorded `choose_next`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChooseInput {
    pub current: Option<String>,
    pub next: Option<String>,
    pub units: Vec<ChooseUnit>,
}

impl ChooseInput {
    /// Parses a `choose_next` observation.
    #[must_use]
    pub fn parse(r: &serde_json::Value) -> Option<Self> {
        let units = r["units"]
            .as_array()?
            .iter()
            .map(|u| {
                let s = u.as_str()?;
                let mut it = s.split('|');
                let id = it.next()?.to_owned();
                let flags = it.next()?.as_bytes();
                let vis = it.next()?.strip_prefix("vis=")?;
                let saddled = it.next()?.strip_prefix("saddled=")? == "1";
                if flags.len() != 3 {
                    return None;
                }
                Some(ChooseUnit {
                    id,
                    surprised: flags[0] == b'1',
                    acting: flags[1] == b'1',
                    marked: flags[2] == b'1',
                    visible: match vis {
                        "1" => Some(true),
                        "0" => Some(false),
                        _ => None,
                    },
                    saddled,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            current: r["current"].as_str().map(str::to_owned),
            next: r["next"].as_str().map(str::to_owned),
            units,
        })
    }
}

/// The ported part of `CombatController`.
#[derive(Debug, Default)]
pub struct Turns {
    /// The port runs.
    pub on: bool,
    /// `m_Units`, in order (empty until `HandleCombatStart`, and after `Clear()`).
    pub units: Vec<TbUnitInfo>,
    /// `m_NextUnit`.
    pub next: Option<EntityIdx>,
    /// `m_NextUnitChosenThisTick`.
    pub chosen_this_tick: bool,
    /// `m_IsUnitsChanged`.
    pub units_changed: bool,
    /// `HasEnemyInCombat` (`m_HasEnemyInCombat`) and `HadEnemyAtSomePoint`.
    pub has_enemy: bool,
    pub had_enemy: bool,
    /// `m_IsPassing` (static, set by `TickTime`).
    pub passing: bool,
    /// `RoundNumber`.
    pub round: i32,
    /// `TimeSinceStart` (seconds passed between turns since combat start).
    pub time_since_start: f32,
    /// `m_HasSurpriseRound`.
    pub surprise_round: bool,
    /// The current turn has ended (`TurnController.End`) and is still `CurrentTurn`.
    pub current_ended: bool,
    /// `WaitingForUI`'s guard count.
    pub waiting_for_ui: i32,
    /// The turn events made, not yet matched with the recording's.
    pub log: VecDeque<TbEvent>,
    /// This frame's recorded `ChooseNextUnit` inputs, in call order.
    pub choose: VecDeque<ChooseInput>,
    /// `UnitCombatPrepareController.Tick`'s rolls so far (unit, result), whose handlers run at
    /// its end.
    pub prepared: Vec<(EntityIdx, i32)>,
    /// The current turn's movement stats (the rider's), when the cooldowns are computed.
    pub movement: super::cooldowns::MovementStats,
    /// The current turn's `ImmuneAttackOfOpportunityOnDisengage` (`None`: not established, a
    /// turn that started before the window and hasn't ticked in it): a new turn's is false, its
    /// tick clears it, `TickMovement` sets it on a five-foot step.
    pub immune_on_disengage: Option<bool>,
    /// The current turn controller's status and counters, when they are computed (a new one at
    /// each turn start; a cleared turn's is never read again).
    pub ctl: super::turn_tick::TurnCtl,
    /// The comparers' recorded identity-hash decisions (`None`: the recording has none to give,
    /// and a fallback is reported).
    pub ties: Option<super::ties::Ties>,
}

fn initiative(w: &World, u: EntityIdx) -> Initiative {
    w.unit(u).map(|x| x.combat.initiative).unwrap_or_default()
}

fn index_of(w: &World, u: EntityIdx) -> Option<usize> {
    w.combat.turns.units.iter().position(|i| i.unit == u)
}

/// Keeps the legacy `tb_units` (read by `StartTurn`'s early return and the join/leave paths)
/// equal to `m_Units`.
fn sync(w: &mut World) {
    let units: Vec<EntityIdx> = w.combat.turns.units.iter().map(|i| i.unit).collect();
    if w.combat.tb_units.is_some() {
        w.combat.tb_units = Some(units);
    }
}

/// `CombatController.SortUnits(reason)`: `m_Units.OrderBy(u => u, UnitsOrderComaprer).ToList()`.
/// The game's `System.Core` sorts a map of indices with `Array.Sort` (the introsort of
/// [`crate::csharp::sort`], on an array of the count's length) and `EnumerableSorter.CompareAnyKeys`
/// (the comparer, then the indices), so the comparer sees the game's pairs in the game's order:
/// its identity-hash fallbacks take the recorded decisions in turn ([`super::ties`]).
pub fn sort_units(w: &mut World, reason: &str) {
    use super::order::Cmp;
    use super::ties::{Tie, TieCmp, decide};
    let infos = w.combat.turns.units.clone();
    let inits: Vec<Initiative> = infos.iter().map(|i| initiative(w, i.unit)).collect();
    let n = infos.len();
    let mut idx: Vec<usize> = (0..n).collect();
    let mut fallback = false;
    let mut decided = 0;
    crate::csharp::sort::introsort(&mut idx, n, &mut |&x, &y| {
        let c = match tb_compare(w, &infos[x], inits[x], &infos[y], inits[y]) {
            Cmp::Less => -1,
            Cmp::Greater => 1,
            Cmp::Equal => 0,
            Cmp::HashFallback => {
                let tie = decide(w, TieCmp::Turn, decided, infos[x].unit, infos[y].unit);
                decided += 1;
                match tie {
                    Tie::Decided(r) => r,
                    Tie::Off => {
                        fallback = true;
                        0
                    }
                    Tie::Missing => 0,
                }
            }
        };
        // `CompareAnyKeys`: equal keys in index order.
        if c == 0 {
            i32::try_from(x).unwrap_or(i32::MAX) - i32::try_from(y).unwrap_or(i32::MAX)
        } else {
            c.signum()
        }
    });
    super::ties::end_call(w, TieCmp::Turn);
    if fallback {
        w.coverage.unsupported(
            "CombatController.SortUnits: the comparer's identity-hash fallback (object identity)",
        );
    }
    w.combat.turns.units = idx.iter().map(|&i| infos[i].clone()).collect();
    sync(w);
    let units = w
        .combat
        .turns
        .units
        .iter()
        .map(|i| {
            (i.unit, initiative(w, i.unit).initiative, i.initiative_override, i.initiative_sequence)
        })
        .collect();
    w.combat.turns.log.push_back(TbEvent::Sort { reason: reason.to_owned(), units });
}

/// `CombatController.StartRound()`.
fn start_round(w: &mut World) {
    let t = &w.combat.turns;
    if t.units.is_empty() || !w.combat.combat_active || t.units.iter().all(|i| i.marked_as_leave) {
        return;
    }
    // `RoundStartTime = GameTime` (read by nothing the sim ports).
    for i in &mut w.combat.turns.units {
        i.surprising = false;
        i.surprised = false;
        i.acting_in_surprise_round = false;
        i.initiative_override = None;
    }
    sort_units(w, "StartRound");
    // `TimeToNextRound = 6f` (it times the round, which the recording's turn starts give).
    w.combat.turns.round += 1;
    let round = w.combat.turns.round;
    w.combat.turns.log.push_back(TbEvent::RoundStart(round));
}

/// The recorded inputs of this `ChooseNextUnit` call, checked against the sim's state.
fn choose_input(w: &mut World) -> Option<ChooseInput> {
    let Some(input) = w.combat.turns.choose.pop_front() else {
        w.coverage
            .unsupported("CombatController.ChooseNextUnit: a call the recording doesn't have");
        return None;
    };
    let id = |w: &World, u: Option<EntityIdx>| u.map(|u| w.entity(u).unique_id.clone());
    let ours: Vec<(String, bool, bool, bool)> = w
        .combat
        .turns
        .units
        .iter()
        .map(|i| {
            (
                w.entity(i.unit).unique_id.clone(),
                i.surprised,
                i.acting_in_surprise_round,
                i.marked_as_leave,
            )
        })
        .collect();
    let theirs: Vec<(String, bool, bool, bool)> =
        input.units.iter().map(|u| (u.id.clone(), u.surprised, u.acting, u.marked)).collect();
    if input.current != id(w, w.combat.current_unit)
        || input.next != id(w, w.combat.turns.next)
        || ours != theirs
    {
        w.coverage.unsupported(
            "CombatController.ChooseNextUnit: the turn controller's state differs from the recording's",
        );
    }
    Some(input)
}

/// `CombatController.ChooseNextUnit()`.
pub fn choose_next_unit(w: &mut World) {
    let input = choose_input(w);
    let anchor = w.combat.current_unit.or(w.combat.turns.next);
    // `FindIndex`: -1 when the unit isn't there.
    let num: i64 = anchor.and_then(|a| index_of(w, a)).map_or(-1, |i| i as i64);
    let mut num2: i64 = -1;
    let mut k = 1;
    while k <= w.combat.turns.units.len() as i64 {
        let n = w.combat.turns.units.len() as i64;
        num2 = num + k;
        if num2 >= n {
            if num2 == n {
                start_round(w);
            }
            num2 %= n;
        }
        let info = w.combat.turns.units[num2 as usize].clone();
        if !skipped(w, &info, input.as_ref()) {
            break;
        }
        k += 1;
    }
    let n = w.combat.turns.units.len() as i64;
    if (0..n).contains(&num2) {
        w.combat.turns.next = Some(w.combat.turns.units[num2 as usize].unit);
        w.combat.turns.chosen_this_tick = true;
    } else if n > 0 {
        w.log("error", "Unable to find a suitable next unit");
    }
}

/// `ChooseNextUnit`'s skip: surprised, or acting in the surprise round without an offensive
/// command; or not the party's enemy and not visible to the player; or carrying a rider.
fn skipped(w: &mut World, info: &TbUnitInfo, input: Option<&ChooseInput>) -> bool {
    let mut skip = info.surprised;
    if !skip && info.acting_in_surprise_round {
        // `!Unit.HasOffensiveCommand()`: commands at this point aren't recorded.
        w.coverage.unsupported(
            "CombatController.ChooseNextUnit: a unit acting in the surprise round (its commands)",
        );
    }
    let party = super::groups::get_group(w, super::units::PLAYER_PARTY_GROUP);
    let enemy =
        super::groups::group_of(w, info.unit).is_some_and(|g| super::groups::is_enemy(w, party, g));
    let rec = input.and_then(|i| i.units.iter().find(|u| u.id == w.entity(info.unit).unique_id));
    if !enemy {
        match rec.and_then(|u| u.visible) {
            Some(v) => skip |= !v,
            None => w.coverage.unsupported(
                "CombatController.ChooseNextUnit: IsVisibleForPlayer is not recorded for a unit",
            ),
        }
    }
    if rec.is_some_and(|u| u.saddled) {
        // `GetRider() != null`: mounts aren't modelled.
        w.coverage.unsupported("CombatController.ChooseNextUnit: a unit that can carry a rider");
    }
    skip
}

/// `CombatController.RemoveUnit(unit)` (`HandleUnitLeaveCombat`, `HandleUnitDeath`,
/// `HandleUnitDestroyed`, while initialized).
pub fn remove_unit(w: &mut World, u: EntityIdx) {
    if index_of(w, u).is_none() {
        return;
    }
    if w.combat.current_unit == Some(u) || w.combat.turns.next == Some(u) {
        choose_next_unit(w);
        if let Some(c) = w.combat.current_unit.take() {
            w.combat.turns.log.push_back(TbEvent::Clear(c));
        }
        w.combat.turns.current_ended = false;
    }
    w.combat.turns.units_changed = true;
    if let Some(i) = index_of(w, u) {
        w.combat.turns.units.remove(i);
    }
    sync(w);
}

/// `CombatController.AddUnit(unit)` (`HandleUnitJoinCombat`, `HandleUnitSpawned`, while
/// initialized).
pub fn add_unit(w: &mut World, u: EntityIdx) {
    if super::is_in_combat(w, u) && index_of(w, u).is_none() {
        w.combat.turns.units.push(TbUnitInfo::new(u));
        w.combat.turns.units_changed = true;
        sync(w);
    }
}

/// `UnitCombatLeaveController.TickGroup`: `FindUnitInfo(unit).MarkedAsLeave = true`.
pub fn mark_as_leave(w: &mut World, u: EntityIdx) {
    if let Some(i) = index_of(w, u) {
        w.combat.turns.units[i].marked_as_leave = true;
    }
}

/// `CombatController.Clear()`'s part here: no units, no next unit, the enemy flags and the round
/// reset.
pub fn clear(w: &mut World) {
    let t = &mut w.combat.turns;
    t.units.clear();
    t.next = None;
    t.units_changed = false;
    t.has_enemy = false;
    t.had_enemy = false;
    t.round = 0;
    t.time_since_start = 0.0;
    t.surprise_round = false;
    t.current_ended = false;
}

/// `HandleCombatStart`: `Clear()`, then `m_Units` from the units in combat of `State.Units`
/// with the recorded surprise flags (they come from commands), the units changed, the round.
pub fn combat_start(w: &mut World, units: Vec<TbUnitInfo>, surprise_round: bool) {
    clear(w);
    w.combat.turns.units = units;
    w.combat.turns.units_changed = true;
    w.combat.turns.surprise_round = surprise_round;
    w.combat.turns.round = i32::from(!surprise_round);
    sync(w);
}

/// `CombatController.Tick`'s first part (the turn controller's phase): with the units changed,
/// `HasEnemyInCombat` is the party group's; with no units or the party out of combat, the
/// party's buffs see the combat end (`BuffCollection.OnCombatEnded`) and the tick stops there.
/// Returns whether the tick goes on.
pub fn tick_units_changed(w: &mut World) -> bool {
    w.combat.turns.chosen_this_tick = false;
    if !w.combat.turns.units_changed {
        return true;
    }
    if w.combat.turns.units.is_empty() || !w.combat.combat_active {
        on_combat_ended(w);
        return false;
    }
    let party = super::groups::get_group(w, super::units::PLAYER_PARTY_GROUP);
    let has = super::groups::has_enemy_in_combat(w, party);
    w.combat.turns.has_enemy = has;
    w.combat.turns.had_enemy |= has;
    w.combat.turns.units_changed = false;
    true
}

/// `foreach (allCharacter in Player.AllCharacters) allCharacter.Buffs.OnCombatEnded()`: with
/// `RemoveNegativeEffectsOnRest`, paralysis and petrification buffs are removed after a delay.
pub fn on_combat_ended(w: &mut World) {
    match w
        .settings
        .difficulty
        .settings
        .raw
        .get("settings.difficulty.remove-negative-levels-on-rest")
        .and_then(serde_json::Value::as_bool)
    {
        Some(false) => {}
        Some(true) => w.coverage.unsupported(
            "BuffCollection.OnCombatEnded: RemoveNegativeEffectsOnRest (paralysis and petrification buffs)",
        ),
        None => {
            w.coverage.unsupported("SettingsRoot.Difficulty.RemoveNegativeEffectsOnRest missing");
        }
    }
}

/// `CombatController.Tick`'s second part, after the current turn ticked: in the default mode,
/// in combat and not waiting for the UI, an ended or missing turn with no next unit chosen makes
/// it choose one and clears the turn.
pub fn tick_choose(w: &mut World, default_mode: bool) {
    if !default_mode || !w.combat.combat_active || w.combat.turns.waiting_for_ui > 0 {
        return;
    }
    let ended = w.combat.current_unit.is_none() || w.combat.turns.current_ended;
    if ended && w.combat.turns.next.is_none() {
        choose_next_unit(w);
        if let Some(c) = w.combat.current_unit.take() {
            w.combat.turns.log.push_back(TbEvent::Clear(c));
        }
        w.combat.turns.current_ended = false;
    }
}

/// `CombatController.TickTime()`'s `m_IsPassing` (from `TimeController.Tick`), given whether
/// the recorded `StartTurn` ran in it.
pub fn tick_time(w: &mut World, started: bool) {
    let delta = w.clock.game_delta_time();
    let t = &mut w.combat.turns;
    t.passing = false;
    if t.waiting_for_ui > 0 || t.next.is_none() {
        return;
    }
    if !started {
        // `TimeSinceStart += GameDeltaTime` (the delta already clamped to the next turn).
        t.time_since_start += delta;
        t.passing = true;
    }
}

/// `CombatController.IsPassing()`.
#[must_use]
pub fn is_passing(w: &World) -> bool {
    let t = &w.combat.turns;
    w.combat.current_unit.is_none() && t.waiting_for_ui == 0 && t.next.is_some() && t.passing
}

/// `StartTurn(m_NextUnit)` from `TickTime`: the recorded turn's unit must be the one chosen, and
/// the next unit is taken. A turn that started (past `StartTurn`'s early return) marks the units
/// changed (`m_IsUnitsChanged = true`, so the next tick recomputes `HasEnemyInCombat`).
pub fn turn_started(w: &mut World, u: Option<EntityIdx>, started: bool) {
    if started {
        w.combat.turns.units_changed = true;
    }
    if w.combat.turns.next != u || w.combat.turns.chosen_this_tick {
        w.coverage.unsupported(
            "CombatController.TickTime: a turn started for another unit than the one chosen",
        );
    }
    w.combat.turns.next = None;
    w.combat.turns.current_ended = false;
}

/// The reason `HandleUnitRollsInitiative` sorts with, before the unit's `CharacterName` (which
/// the replay matches as a prefix: names are localized).
pub const ROLLS_INITIATIVE: &str = "HandleUnitRollsInitiative for ";

/// `CombatController.HandleUnitRollsInitiative(rule)`'s turn-order part (in turn-based combat):
/// a unit rolling in the surprise round is surprised; one joining later at or above the current
/// (or next) unit's initiative takes that unit's place in the order (`InitiativeOverride` and
/// `InitiativeSequence`, the following ties shifted) and becomes the next unit between turns;
/// then the units are sorted and the unit is processed.
pub fn unit_rolls_initiative(w: &mut World, u: EntityIdx) {
    let Some(i) = index_of(w, u) else {
        w.log("error", "HandleUnitRollsInitiative: unit info not found");
        return;
    };
    let surprise = w.combat.turns.surprise_round && w.combat.turns.round == 0;
    if surprise && !w.combat.turns.units[i].surprising {
        w.combat.turns.units[i].surprised = true;
    } else if w.combat.turns.time_since_start > 0.0 {
        let between = w.combat.current_unit.is_none();
        let anchor = w.combat.current_unit.or(w.combat.turns.next);
        if let Some(a) = anchor
            && initiative(w, u).initiative >= initiative(w, a).initiative
        {
            let Some(ai) = index_of(w, a) else {
                w.coverage.unsupported(
                    "HandleUnitRollsInitiative: the current unit is not in the turn order",
                );
                return;
            };
            let a_init = initiative(w, a).initiative;
            let units = &mut w.combat.turns.units;
            let num = units[ai].initiative_override.unwrap_or(a_init);
            units[i].initiative_override = Some(num);
            units[i].initiative_sequence = units[ai].initiative_sequence + i32::from(!between);
            let mut num2 = units[i].initiative_sequence;
            let start = ai + usize::from(!between);
            for j in start..w.combat.turns.units.len() {
                if j == i {
                    continue;
                }
                let x = &w.combat.turns.units[j];
                if x.initiative_override.unwrap_or(initiative(w, x.unit).initiative) != num {
                    break;
                }
                num2 += 1;
                w.combat.turns.units[j].initiative_sequence = num2;
            }
            if between {
                w.combat.turns.next = Some(u);
            }
        }
    }
    if w.combat.turns.units[i].acting_in_surprise_round
        && let Some(x) = w.unit_mut(u)
    {
        x.combat.cooldown.initiative = 0.0;
    }
    sort_units(w, ROLLS_INITIATIVE);
    if w.combat.turns.time_since_start > 0.0 {
        joiner_cooldown(w, u);
    }
    if let Some(i) = index_of(w, u) {
        w.combat.turns.units[i].initiative_processed = true;
    }
}

/// `HandleUnitRollsInitiative`'s cooldown for a unit rolling after combat started: half the time to
/// the next processed unit's turn when it is the next unit, else the mean of its processed
/// neighbours' times (the walks as written, including the next-unit walk that restarts at 0 when
/// the unit is last).
fn joiner_cooldown(w: &mut World, u: EntityIdx) {
    let Some(i) = index_of(w, u) else { return };
    let units = &w.combat.turns.units;
    let n = units.len();
    let last = n - 1;
    let processed = |k: usize| units[k].initiative_processed;
    let value = if w.combat.turns.next == Some(u) {
        let mut k = if i == last { 0 } else { i + 1 };
        let mut steps = 0;
        while !processed(k) && k != i {
            k = if i == last { 0 } else { k + 1 };
            steps += 1;
            if k >= n || steps > n {
                // The game's walk runs past the list (an exception) or doesn't end (it hangs).
                w.coverage.unsupported("HandleUnitRollsInitiative: the next-unit walk doesn't end");
                return;
            }
        }
        time_to_next_turn(w, units[k].unit) / 2.0
    } else {
        let mut prev = if i == 0 { last } else { i - 1 };
        let mut next = if i == last { 0 } else { i + 1 };
        while !processed(prev) && prev != i {
            prev = if prev == 0 { last } else { prev - 1 };
        }
        while !processed(next) && next != i {
            next = if next == last { 0 } else { next + 1 };
        }
        (time_to_next_turn(w, units[next].unit) + time_to_next_turn(w, units[prev].unit)) / 2.0
    };
    if let Some(x) = w.unit_mut(u) {
        x.combat.cooldown.initiative = value;
    }
}

#[cfg(test)]
pub(crate) mod tests;
