//! The party (decompiled, not copied: `Kingmaker/Player.cs` `UpdateCharacterLists`,
//! `AddCharacterToLists`, `AddCompanion`; `Kingmaker.UnitLogic.Parts/UnitPartCompanion.cs`
//! `SetState`).
//!
//! `Party`, `PartyAndPets`, `AllCharacters` and `RemoteCompanions` are rebuilt from
//! `PartyCharacters` then the cross-scene units, in that order, whenever they are read
//! after an invalidation; here they are computed on each read, which gives the same lists
//! (the fix-ups `AddCharacterToLists` applies to `PartyCharacters` are reported if they
//! would change it).

use serde_json::json;

use crate::world::{EntityIdx, World};

/// The four lists `UpdateCharacterLists` builds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CharacterLists {
    pub party: Vec<EntityIdx>,
    pub party_and_pets: Vec<EntityIdx>,
    pub all_characters: Vec<EntityIdx>,
    pub remote_companions: Vec<EntityIdx>,
}

fn companion_state(w: &World, u: EntityIdx) -> Option<i32> {
    w.unit(u).and_then(|x| x.companion_state)
}

/// `Player.UpdateCharacterLists()`.
pub fn character_lists(w: &mut World) -> CharacterLists {
    let mut out = CharacterLists::default();
    let mut order: Vec<EntityIdx> = w.player.party_characters.clone();
    order.extend(w.player.cross_scene_units.iter().copied());
    for u in order {
        if out.all_characters.contains(&u) {
            continue;
        }
        let Some(unit) = w.unit(u) else { continue };
        let master = unit.master.unwrap_or(u);
        let state = companion_state(w, master);
        let in_party = state == Some(2);
        let is_pet = w.unit(u).is_some_and(|x| x.is_pet);
        if in_party && !w.player.party_characters.contains(&master) {
            w.coverage.unsupported("Player.AddCharacterToLists: unit in party but not in list");
        }
        if !in_party && w.player.party_characters.contains(&master) {
            w.coverage.unsupported("Player.AddCharacterToLists: unit in list but not in party");
        }
        let mut flag = in_party;
        if w.player.capital_party_mode {
            w.coverage.unsupported("Player.AddCharacterToLists: capital party mode");
            flag = Some(master) == w.main_character;
        }
        if !is_pet && flag {
            out.party.push(u);
        }
        let remote = state == Some(1) || (state == Some(2) && !flag);
        if remote {
            out.remote_companions.push(u);
        }
        if flag && w.entity(u).is_in_game {
            out.party_and_pets.push(u);
        }
        out.all_characters.push(u);
    }
    out
}

/// `Player.Party`.
pub fn party(w: &mut World) -> Vec<EntityIdx> {
    character_lists(w).party
}

/// `UnitPartCompanion.SetState(s)` (pets aren't modelled).
pub fn set_companion_state(w: &mut World, u: EntityIdx, s: i32) {
    if let Some(unit) = w.unit_mut(u) {
        unit.companion_state = Some(s);
    }
    if !w.entity(u).parts.iter().any(|p| p.clr == "UnitPartNonStackBonuses") {
        w.entity_mut(u).parts.push(crate::world::Part { clr: "UnitPartNonStackBonuses" });
    }
}

/// `Player.AddCompanion(unit)`.
pub fn add_companion(w: &mut World, u: EntityIdx) {
    if w.unit(u).is_some_and(|x| x.is_pet) {
        w.log("error", "Can't add pet to PartyCharacters list");
        return;
    }
    if !w.entity(u).parts.iter().any(|p| p.clr == "UnitPartCompanion") {
        w.entity_mut(u).parts.push(crate::world::Part { clr: "UnitPartCompanion" });
    }
    set_companion_state(w, u, 2);
    w.player.party_characters.push(u);
    // `AdvanceMythicExperience(MythicExperience)`: mythic experience 0 in the prologue.
    if w.player.mythic_experience != 0 {
        w.coverage.unsupported("Player.AddCompanion: AdvanceMythicExperience");
    }
    w.events
        .mutation("party_add", json!({ "unit": crate::events::unit_ref(&w.entity(u).unique_id) }));
    crate::rulebook::bus::raise_global_call(w, "IPartyHandler", "HandleAddCompanion", Some(u));
}

/// `IsPartyChecksDisabled(unit)`: `UnitPartCompanion.IsPartyChecksDisabled` (set by the
/// `DisableCompanionPartyChecks` etude bracket; not tracked: none on the meetings' path).
#[must_use]
pub fn is_party_checks_disabled(_w: &World, _u: EntityIdx) -> bool {
    false
}
