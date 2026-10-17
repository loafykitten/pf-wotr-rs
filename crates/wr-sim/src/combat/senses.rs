//! What `UnitMemoryController` and `SleepingUnitsController` read about areas and units, as the sim
//! can compute it (decompiled, not copied; docs/poc5/memory.md, "Leaves the sim computes"):
//!
//! - `LoadedAreaState.Settings.Peaceful` (a `CountableFlag` that `EtudePeacefulZone` retains);
//! - `unit.Passive` (an `ObservableCountFlag`: `EtudeBracketMakePassive`, and a pet's follows its
//!   master's through `UnitPartPet`'s synchronizer);
//! - `unit.InStealthFor(group)` (`State.IsInStealth && !Stealth.IsSpottedBy(group)`);
//! - whether a group's members see an invisible unit (`IsSeeInvisibility`, `UnitPartBlindsense`),
//!   as far as it doesn't need line of sight;
//! - `SleepingUnitsController`'s ready-for-combat test of a group.
//!
//! Each returns what the sim's state says. What the sim doesn't model (the sources of `Passive`
//! other than etude brackets and pets, `RuleEnterStealth`, fog of war) shows as a difference when
//! a recording is compared with it, or as an unknown part where the answer needs it.

use super::groups::{self, GroupIdx};
use super::memory::sim as memsim;
use crate::unit_state::has_condition;
use crate::world::{EntityIdx, World};

/// `UnitCondition.SeeInvisibility` and `TrueSeeing` (`UnitDescriptor.IsSeeInvisibility`).
const SEE_INVISIBILITY: i32 = 17;
const TRUE_SEEING: i32 = 18;

/// A `CountableFlag` kept as a named counter (etude brackets retain and release it).
fn flag(w: &World, key: &str) -> bool {
    w.player.counters.get(key).is_some_and(|n| *n > 0)
}

/// `LoadedAreaState.Settings.Peaceful`.
#[must_use]
pub fn area_peaceful(w: &World) -> bool {
    flag(w, "area.Peaceful")
}

/// `(bool)unit.Passive`. A pet (`UnitPartPet` with its master) mirrors its master's flag: the
/// synchronizer retains or releases until the counts are equal and reverts any change the pet's
/// own sources make.
#[must_use]
pub fn passive(w: &World, u: EntityIdx) -> bool {
    match w.unit(u) {
        Some(x) if x.pet_type.is_some() && x.master.is_some_and(|m| m != u) => {
            x.master.is_some_and(|m| passive(w, m))
        }
        _ => flag(w, &format!("unit:{}.Passive", w.entity(u).unique_id)),
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

/// `unit.InStealthFor(observer)` (`UnitStealthExtension`): in stealth and not spotted by the group.
pub fn in_stealth_for(w: &mut World, u: EntityIdx, g: GroupIdx) -> bool {
    w.unit(u).is_some_and(|x| x.in_stealth) && !is_spotted_by(w, u, g)
}

/// The loop over a group's members for an invisible target (`UnitMemoryController.UpdateVisibility`):
/// a member that sees invisibility and has line of sight to it, or whose blindsense reaches it.
/// `Some(false)` when no member could (none sees invisibility, none has blindsense); `None` when
/// the answer rests on a member's line of sight (or blindsense range), which this doesn't compute.
#[must_use]
pub fn invisible_target_seen(w: &World, members: &[EntityIdx]) -> Option<bool> {
    let could =
        |m: &EntityIdx| sees_invisibility(w, *m) || w.unit(*m).is_some_and(|x| x.has_blindsense);
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
/// awake state is unknown, or that is out of combat and not of the party (its fog of war, and its
/// `AwakeTimer`, aren't modelled), leaves the answer unknown unless another member decides it. A
/// party member is never in fog of war (the revealers).
#[must_use]
pub fn ready_for_combat(w: &World, g: GroupIdx, party: &[EntityIdx]) -> Tri {
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
        if awake && (super::is_in_combat(w, u) || party.contains(&u)) {
            return Tri::Yes;
        }
        unknown = true;
    }
    if unknown { Tri::Unknown } else { Tri::No }
}

#[cfg(test)]
mod tests;
