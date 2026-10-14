//! `Recruit` (decompiled, not copied: `Kingmaker.Designers.EventConditionActionSystem.
//! Actions/Recruit.cs`, `Kingmaker.Controllers/EntityCreationController.cs` `RecruitNPC`,
//! `ChangeUnitBlueprint`, `Tick`; `Kingmaker.UnitLogic/UnitHelper.cs` `Replenish`).
//!
//! Ported: the companion lookup, `AddCompanion` (party list, `UnitPartCompanion` state),
//! the move to the cross-scene state, the unique id the new unit takes, the experience
//! match (`AdvanceExperienceTo` the main character's), `OnRecruitImmediate`/`OnRecruit` in
//! a `RecruitedUnitData` scope, and the party-size check.
//!
//! **Not ported (unsupported, the run is invalid):** `ChangeUnitBlueprint` creates a *new*
//! `UnitEntityData` from the companion blueprint (stats, class levels through
//! `AddClassLevels`, facts with fresh ids) and destroys the NPC. The sim keeps the NPC's
//! hydrated unit in its place (same stats and facts, the new id), so the recruited
//! companion's character sheet and fact ids aren't the original's. `Replenish` is ported
//! only when it has nothing to do (no resources, spellbooks or negative buffs).

use std::rc::Rc;

use serde_json::json;

use crate::content::{Blueprint, View};
use crate::elements::{Ctx, ElementError, Result, run_actions, try_get_unit, with_ctx};
use crate::world::{EntityIdx, World};

/// `Recruit.RunAction()`.
pub fn run(w: &mut World, owner: &Rc<Blueprint>, a: View<'_>) -> Result<()> {
    let mut recruited: Vec<EntityIdx> = Vec::new();
    for d in a.objs("Recruited") {
        let dv = View(d);
        if let Some(u) = switch_to_companion(w, owner, a, dv)? {
            recruited.push(u);
        }
    }
    // `Game.Instance.EntityCreator.Tick()`: created units join their state and raise
    // `IUnitHandler`/`IUnitSpawnHandler.HandleUnitSpawned`.
    crate::spawn::entity_creator_tick(w);
    if crate::narrative::party::party(w).len() > 6 {
        return Err(crate::elements::unsupported(
            w,
            "action",
            "Recruit (party over six: group changer UI)",
        ));
    }
    for u in recruited {
        with_ctx(w, Ctx::RecruitedUnit(u), |w| run_actions(w, owner, a.obj("OnRecruit")));
    }
    Ok(())
}

/// `SwitchToCompanion(data)`.
fn switch_to_companion(
    w: &mut World,
    owner: &Rc<Blueprint>,
    a: View<'_>,
    d: View<'_>,
) -> Result<Option<EntityIdx>> {
    let Some(companion_bp) = d.guid("m_CompanionBlueprint") else {
        return Err(ElementError::Other("NullReferenceException: companion blueprint".into()));
    };
    let lists = crate::narrative::party::character_lists(w);
    let existing = lists
        .all_characters
        .iter()
        .copied()
        .find(|u| w.unit(*u).and_then(|x| x.blueprint) == Some(companion_bp));
    let state = existing.and_then(|u| w.unit(u).and_then(|x| x.companion_state));
    let ex = state == Some(3);
    let remote = state == Some(1);
    let npc = match d.obj("NPCUnit") {
        Some(ev) => try_get_unit(w, owner, Some(ev))?,
        None => None,
    };
    let unit = if existing.is_none() || ex {
        recruit_npc(w, npc, companion_bp)?
    } else if !remote {
        w.log("error", "Attempted to double-recruit");
        return Ok(None);
    } else {
        return Err(crate::elements::unsupported(w, "action", "Recruit (remote companion)"));
    };
    if let Some(u) = w.unit_mut(unit) {
        u.group_id = Some("<directly-controllable-unit>".into());
    }
    let only_active =
        w.settings.difficulty.settings.only_active_companions_receive_experience.unwrap_or(false);
    if !only_active || a.bool("MatchPlayerXpExactly") {
        let mc = w.main_character.expect("main character");
        #[allow(clippy::cast_possible_truncation)]
        let xp = w.unit(mc).map_or(0, |x| x.progression.experience as i32);
        crate::xp::advance_experience_to(w, unit, xp);
    }
    replenish(w, unit);
    with_ctx(w, Ctx::RecruitedUnit(unit), |w| run_actions(w, owner, a.obj("OnRecruitImmediate")));
    if w.unit(unit).is_some_and(|x| x.master.is_some()) {
        return Err(crate::elements::unsupported(w, "action", "Recruit (pet)"));
    }
    if !a.bool("AddToParty") {
        return Err(crate::elements::unsupported(
            w,
            "action",
            "Recruit (AddToParty false: RemoveCompanion)",
        ));
    }
    w.events.mutation(
        "recruit",
        json!({ "companion": companion_bp.to_string(), "unit": crate::events::unit_ref(&w.entity(unit).unique_id) }),
    );
    Ok(Some(unit))
}

/// `EntityCreationController.RecruitNPC(npc, companionBlueprint)`.
fn recruit_npc(
    w: &mut World,
    npc: Option<EntityIdx>,
    companion_bp: wr_content::Guid,
) -> Result<EntityIdx> {
    // `UnitPartCompanion.FindCompanion(blueprint, ExCompanion)`: no ex-companions here.
    let Some(npc) = npc else {
        return Err(crate::elements::unsupported(
            w,
            "action",
            "Recruit without an NPC unit (create from blueprint)",
        ));
    };
    // `ChangeUnitBlueprint(npc, companionBlueprint, toCrossState: true)`.
    let Some(npc) = crate::spawn::change_unit_blueprint(w, npc, companion_bp, true) else {
        return Err(crate::elements::unsupported(
            w,
            "action",
            "Recruit (ChangeUnitBlueprint failed)",
        ));
    };
    w.events.mutation(
        "unit_recreated",
        json!({ "new": w.entity(npc).unique_id.clone(), "blueprint": companion_bp.to_string() }),
    );
    if !w.unit(npc).is_some_and(|x| x.is_pet) {
        crate::narrative::party::add_companion(w, npc);
    }
    Ok(npc)
}

/// `UnitHelper.Replenish(descriptor)`.
fn replenish(w: &mut World, u: EntityIdx) {
    if can_heal_negative_effects(w, u) {
        remove_negative_effects(w, u);
    }
    crate::resources::restore_all(w, u);
    crate::spellbook::rest_unit(w, u);
    w.coverage.stub("UnitDescriptor.Replenish: weariness reset, brain actions");
}

/// `UnitRestExtension.CanHealNegativeEffects`: a player-faction unit that is a pet, or when
/// the difficulty removes negative effects on rest or declines stats.
fn can_heal_negative_effects(w: &mut World, u: EntityIdx) -> bool {
    let Some(unit) = w.unit(u) else { return false };
    if !unit.is_player_faction {
        return false;
    }
    if unit.is_pet {
        return true;
    }
    // `RemoveNegativeEffectsOnRest || StatsAdjustments == Decline` (`Decline` is 2).
    if w.settings.difficulty.settings.stats_adjustments == Some(2) {
        return true;
    }
    match w
        .settings
        .difficulty
        .settings
        .raw
        .get("settings.difficulty.remove-negative-levels-on-rest")
    {
        Some(serde_json::Value::Bool(b)) => *b,
        _ => {
            w.coverage.unsupported("SettingsRoot.Difficulty.RemoveNegativeEffectsOnRest missing");
            false
        }
    }
}

/// `UnitHelper.RemoveNegativeEffects`: the negative-levels part (not ported) and every buff
/// whose context spell descriptor is a negative effect, unless its blueprint has
/// `IsPositiveEffect`.
fn remove_negative_effects(w: &mut World, u: EntityIdx) {
    if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartNegativeLevels") {
        w.coverage.unsupported("UnitPartNegativeLevels.RemoveAll");
    }
    let doomed: Vec<_> =
        w.entity(u)
            .facts
            .list
            .iter()
            .copied()
            .filter(|f| {
                let fact = w.fact(*f);
                fact.kind == crate::world::FactKind::Buff
                    && fact.context.as_ref().is_some_and(|c| {
                        crate::combat::rules::is_negative_effect(c.spell_descriptor)
                    })
                    && !fact.bp.as_ref().is_some_and(|bp| {
                        bp.components_array()
                            .iter()
                            .any(|o| crate::content::is_a(o.ty, "IsPositiveEffect"))
                    })
            })
            .collect();
    for f in doomed {
        crate::unit_facts::remove(w, u, f);
    }
}
