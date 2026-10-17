//! What `UnitMemoryController` and `SleepingUnitsController` read about areas and units, as the sim
//! can decide it (decompiled, not copied; docs/poc5/memory.md, "The tick's other leaves"):
//!
//! - `LoadedAreaState.Settings.Peaceful` (a `CountableFlag` that `EtudePeacefulZone` retains);
//! - `unit.Passive` (an `ObservableCountFlag`: `EtudeBracketMakePassive`, and a pet's follows its
//!   master's through `UnitPartPet`'s synchronizer);
//! - `unit.InStealthFor(group)` (`State.IsInStealth && !Stealth.IsSpottedBy(group)`);
//! - whether a group's members see an invisible unit (`IsSeeInvisibility`, `UnitPartBlindsense`),
//!   as far as it doesn't need line of sight;
//! - `SleepingUnitsController`'s ready-for-combat test of a group, and the part of
//!   `IsInFogOfWar` it can answer (`AlwaysRevealedInFogOfWar`: `IsDirectlyControllable`).
//!
//! A function answers only what the sim's state decides. Where a producer of the state that the
//! sim doesn't run may have acted (a fact with a `MarkPassive` or `Blindsense` component, a unit
//! that is or may go into stealth, a unit whose direct control depends on state the sim doesn't
//! hold), it answers `None` / [`Tri::Unknown`] and the caller takes the recording's value, counted
//! as taken. Producers that are fail-closed elsewhere (a spawner's `SpawnerMakePassivePart`:
//! `scene_spawn` refuses a spawner component it has no port for; the cutscene commands
//! `CommandMarkPassiveSpan` and `CommandIgnoreCombat`: the cutscene player refuses any command it
//! has no port for) make the run invalid before this is asked, so they are not repeated here.

use super::groups::{self, GroupIdx};
use super::memory::sim as memsim;
use crate::unit_state::has_condition;
use crate::world::{EntityIdx, World};

/// `UnitCondition.SeeInvisibility`, `TrueSeeing`, `Invisible` and `Frightened`.
const SEE_INVISIBILITY: i32 = 17;
const TRUE_SEEING: i32 = 18;
const INVISIBLE: i32 = 7;
const FRIGHTENED: i32 = 13;
/// `CompanionState`: `Remote`, `ExCompanion`.
const COMPANION_REMOTE: i32 = 1;
const COMPANION_EX: i32 = 3;
/// `CompanionState.InPartyDetached`.
const COMPANION_DETACHED: i32 = 4;

/// A `CountableFlag` kept as a named counter (etude brackets retain and release it).
fn flag(w: &World, key: &str) -> bool {
    w.player.counters.get(key).is_some_and(|n| *n > 0)
}

/// `LoadedAreaState.Settings.Peaceful`.
#[must_use]
pub fn area_peaceful(w: &World) -> bool {
    flag(w, "area.Peaceful")
}

/// Whether the unit's facts or blueprint carry a component of this type (a producer the sim has no
/// port for, or the fact scan of a part's creator).
fn carries_component(w: &World, u: EntityIdx, ty: &str) -> bool {
    let on_blueprint = w
        .unit(u)
        .and_then(|x| x.blueprint)
        .and_then(|g| w.content.get(g))
        .is_some_and(|b| crate::unit_facts::has_component(&b, ty));
    on_blueprint
        || w.entity(u).facts.list.iter().any(|f| {
            w.fact(*f).bp.as_ref().is_some_and(|b| crate::unit_facts::has_component(b, ty))
        })
}

/// `(bool)unit.Passive`. A pet (`UnitPartPet` with its master) mirrors its master's flag: the
/// synchronizer retains or releases until the counts are equal and reverts any change the pet's
/// own sources make. `None` when the unit (or its master) carries a `MarkPassive` component,
/// whose runtime the sim doesn't port.
#[must_use]
pub fn passive(w: &World, u: EntityIdx) -> Option<bool> {
    if carries_component(w, u, "MarkPassive") {
        return None;
    }
    match w.unit(u) {
        Some(x) if x.pet_type.is_some() && x.master.is_some_and(|m| m != u) => {
            x.master.and_then(|m| passive(w, m))
        }
        _ => Some(flag(w, &format!("unit:{}.Passive", w.entity(u).unique_id))),
    }
}

/// `UnitDescriptor.IsSeeInvisibility`: `SeeInvisibility` or `TrueSeeing`.
#[must_use]
pub fn sees_invisibility(w: &World, u: EntityIdx) -> bool {
    has_condition(w, u, SEE_INVISIBILITY) || has_condition(w, u, TRUE_SEEING)
}

/// `UnitStealth.IsSpottedBy(group)`: a spotter that resolves, is conscious and is in the group.
fn is_spotted_by(w: &mut World, u: EntityIdx, g: GroupIdx) -> bool {
    let spotters: Vec<String> = w.unit(u).map(|x| x.stealth_spotted_by.clone()).unwrap_or_default();
    for id in spotters {
        let Some(s) = w.by_id.get(&id).copied() else { continue };
        if memsim::resolve(w, s) == super::memory::Resolved::Missing
            || !super::units::is_conscious(w, s)
        {
            continue;
        }
        if groups::group_of(w, s) == Some(g) {
            return true;
        }
    }
    false
}

/// `unit.InStealthFor(observer)` (`UnitStealthExtension`) from the state as saved: in stealth and
/// not spotted by the group. Valid only while `UnitStealthController` (not run by the sim) hasn't
/// changed the unit: see [`in_stealth_for`].
pub fn in_stealth_for_as_saved(w: &mut World, u: EntityIdx, g: GroupIdx) -> bool {
    w.unit(u).is_some_and(|x| x.in_stealth) && !is_spotted_by(w, u, g)
}

/// Whether `UnitStealthController` can have changed the unit's stealth or its spotters since the
/// save: it is in stealth (the controller exits it, adds spotters), or the controller's entry
/// conditions hold (`Invisible`, `WantEnterStealth`, `InAmbush`, `ForceEnterStealth`).
fn stealth_may_have_changed(w: &World, u: EntityIdx) -> bool {
    w.unit(u).is_some_and(|x| x.in_stealth || x.stealth_pending) || has_condition(w, u, INVISIBLE)
}

/// `unit.InStealthFor(observer)`: false for a unit the stealth controller has no reason to touch
/// (not in stealth, none of its entry conditions: `RuleEnterStealth` comes from a command or
/// ability the replay would reject as a rule it doesn't run); `None` otherwise, the saved state
/// being only where the controller started.
pub fn in_stealth_for(w: &World, u: EntityIdx) -> Option<bool> {
    (!stealth_may_have_changed(w, u)).then_some(false)
}

/// `UnitEntityData.IsDirectlyControllable`, as far as the sim holds its inputs: `Yes` only where
/// every test of the getter is decided (a unit in the game, not prevented by an etude bracket, a
/// faction that is directly controllable, not finally dead, not panicked, a companion in the party
/// and not detached); `No` where one decided test fails; `Unknown` for what the sim doesn't hold
/// (the capital party mode, tactical combat, a summoned monster, `IsPanicked` while the unit is
/// frightened).
#[must_use]
pub fn directly_controllable(w: &World, u: EntityIdx) -> Tri {
    let Some(x) = w.unit(u) else { return Tri::No };
    if !w.entity(u).is_in_game
        || flag(w, &format!("unit:{}.PreventDirectControl", w.entity(u).unique_id))
    {
        return Tri::No;
    }
    if w.player.capital_party_mode() {
        return Tri::Unknown;
    }
    let area = w.player.area.and_then(|g| w.content.get(g));
    if area.is_some_and(|a| crate::content::is_a(a.obj.ty, "BlueprintTacticalCombatArea")) {
        return Tri::Unknown;
    }
    let faction = x.faction.and_then(|f| w.content.get(f));
    let Some(faction) = faction else { return Tri::Unknown };
    if !faction.view().bool("IsDirectlyControllable") || x.finally_dead {
        return Tri::No;
    }
    // `State.IsPanicked` is `Frightened && …` (`UnitFearController`): not frightened, not panicked.
    if has_condition(w, u, FRIGHTENED) {
        return Tri::Unknown;
    }
    if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartSummonedMonster") {
        return Tri::Unknown;
    }
    // `Master?.Get<UnitPartCompanion>() ?? Get<UnitPartCompanion>()`, and `IsDetached`.
    let master = x.master.filter(|m| *m != u);
    let state =
        master.and_then(|m| w.unit(m)).and_then(|m| m.companion_state).or(x.companion_state);
    let detached = match master {
        Some(m) => w.unit(m).and_then(|m| m.companion_state) == Some(COMPANION_DETACHED),
        None => x.companion_state == Some(COMPANION_DETACHED),
    };
    match state {
        Some(s) if s != COMPANION_EX && s != COMPANION_REMOTE && !detached => Tri::Yes,
        _ => Tri::No,
    }
}

/// The loop over a group's members for an invisible target (`UnitMemoryController.UpdateVisibility`):
/// a member that sees invisibility and has line of sight to it, or whose blindsense reaches it.
/// `Some(false)` when no member could (none sees invisibility, none has blindsense: the part, or a
/// fact whose `Blindsense` component creates it); `None` when the answer rests on a member's line
/// of sight (or blindsense range), which this doesn't compute.
#[must_use]
pub fn invisible_target_seen(w: &World, members: &[EntityIdx]) -> Option<bool> {
    let could = |m: &EntityIdx| {
        sees_invisibility(w, *m)
            || w.unit(*m).is_some_and(|x| x.has_blindsense)
            || carries_component(w, *m, "Blindsense")
    };
    if members.iter().any(could) { None } else { Some(false) }
}

/// What the sim can establish of a statement about a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tri {
    Yes,
    No,
    Unknown,
}

/// Whether `SleepingUnitsController` lists the group in `ReadyForCombatUnitGroups`: some member is
/// not sleeping, not of a peaceful faction, and (not in fog of war, or in combat, or with
/// `AwakeTimer > 0`). From the frame's awake units ([`super::life::update_awake`]): a member whose
/// awake state is unknown, or that is out of combat and not certain to be out of fog of war (its
/// `IsInFogOfWar` and `AwakeTimer` aren't modelled; only `AlwaysRevealedInFogOfWar`, a directly
/// controllable unit, is certain), leaves the answer unknown unless another member decides it.
#[must_use]
pub fn ready_for_combat(w: &World, g: GroupIdx) -> Tri {
    let mut unknown = false;
    for &u in &w.combat.groups.get(g).units {
        let asleep = !w.combat.awake_unknown.contains(&u) && !w.combat.awake.contains(&u);
        if asleep {
            continue;
        }
        match super::units::faction_peaceful(w, u) {
            Some(true) => continue,
            Some(false) => {}
            None => {
                unknown = true;
                continue;
            }
        }
        let awake = w.combat.awake.contains(&u) && !w.combat.awake_unknown.contains(&u);
        if awake && (super::is_in_combat(w, u) || directly_controllable(w, u) == Tri::Yes) {
            return Tri::Yes;
        }
        unknown = true;
    }
    if unknown { Tri::Unknown } else { Tri::No }
}

#[cfg(test)]
mod tests;
