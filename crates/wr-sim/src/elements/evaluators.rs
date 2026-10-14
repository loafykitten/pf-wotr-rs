//! Evaluators (each cites its decompiled file under
//! `Kingmaker.Designers.EventConditionActionSystem.Evaluators/`). These return the
//! `GetValueInternal` result: `None` is a null value (which `GetValue` turns into
//! `FailToEvaluateException`).

use std::rc::Rc;

use super::{Ctx, ElementError, Result, ctx_unit, get_unit, unsupported};
use crate::content::{Blueprint, View};
use crate::world::{EntityIdx, World};

pub const IMPLEMENTED: &[&str] = &[
    "PlayerCharacter",
    "UnitFromSpawner",
    "RecruitedUnit",
    "ScriptZoneUnit",
    "CompanionInParty",
    "PetEvaluator",
    "IntConstant",
    "FlagValue",
    "PartyUnit",
    "FactOwner",
    "SpawnedUnit",
    "CombatUnit",
];

/// A unit evaluator's `GetValueInternal()`.
pub fn unit_value(w: &mut World, owner: &Rc<Blueprint>, ev: View<'_>) -> Result<Option<EntityIdx>> {
    w.coverage.executed(format!("evaluator {}", ev.type_name()));
    match ev.type_name() {
        // `PlayerCharacter.cs` → `GameHelper.GetPlayerCharacter()`.
        "PlayerCharacter" => Ok(w.main_character),
        // `UnitFromSpawner.cs`: the spawner's `SpawnedUnit` reference, resolved through the
        // entity service (a unit entity of that id).
        "UnitFromSpawner" => {
            // `EntityReference.UniqueId` (its JSON name is `_entity_id`).
            let id = ev
                .obj("Spawner")
                .and_then(|s| s.str("UniqueId").or_else(|| s.str("_entity_id")))
                .map(str::to_owned);
            let Some(id) = id else { return Ok(None) };
            Ok(spawned_unit(w, &id))
        }
        // `RecruitedUnit.cs`: `ContextData<RecruitedUnitData>.Current?.Unit`.
        "RecruitedUnit" => Ok(ctx_unit(w, |c| match c {
            Ctx::RecruitedUnit(u) => Some(*u),
            _ => None,
        })),
        // `PartyUnit.cs`: `ContextData<PartyUnitData>.Current?.Unit`.
        "PartyUnit" => Ok(ctx_unit(w, |c| match c {
            Ctx::PartyUnit(u) => Some(*u),
            _ => None,
        })),
        // `ScriptZoneUnit.cs`: `ContextData<ScriptZoneTriggerData>.Current?.Unit`.
        "ScriptZoneUnit" => Ok(ctx_unit(w, |c| match c {
            Ctx::ScriptZoneUnit(u) => Some(*u),
            _ => None,
        })),
        // `SpawnedUnit.cs`: `ContextData<SpawnedUnitData>.Current?.Unit`.
        "SpawnedUnit" => Ok(ctx_unit(w, |c| match c {
            Ctx::SpawnedUnit(u) => Some(*u),
            _ => None,
        })),
        // `CombatUnit.cs`: `ContextData<EvaluatedUnitCombatTrigger.UnitData>.Current?.Unit`.
        "CombatUnit" => Ok(ctx_unit(w, |c| match c {
            Ctx::CombatUnit(u) => Some(*u),
            _ => None,
        })),
        // `FactOwner.cs`: `ContextData<FactData>.Current?.Fact?.Owner as UnitEntityData`, else
        // the current `Buff.Data`'s buff owner.
        "FactOwner" => {
            let current =
                |pick: fn(&Ctx) -> Option<crate::world::FactIdx>| w.ctx.iter().rev().find_map(pick);
            let fact_owner = current(|c| match c {
                Ctx::Fact(f) => Some(*f),
                _ => None,
            })
            .and_then(|f| w.fact(f).owner)
            .filter(|o| w.unit(*o).is_some());
            let buff_owner = current(|c| match c {
                Ctx::BuffData(f) => Some(*f),
                _ => None,
            })
            .and_then(|f| w.fact(f).owner);
            Ok(fact_owner.or(buff_owner))
        }
        // `CompanionInParty.cs`: the first cross-scene unit whose blueprint is the companion
        // (or has it as a prototype), unless its companion state excludes it.
        "CompanionInParty" => {
            let Some(comp) = ev.guid("m_Companion") else { return Ok(None) };
            let found = w.player.cross_scene_units.iter().copied().find(|u| {
                w.unit(*u).and_then(|x| x.blueprint).is_some_and(|b| is_companion(w, b, comp))
            });
            let Some(u) = found else { return Ok(None) };
            let state = w.unit(u).and_then(|x| x.companion_state).unwrap_or(0);
            if (state == 1 && !ev.bool("IncludeRemote"))
                || (state == 3 && !ev.bool("IncludeExCompanions"))
                || (state == 4 && !ev.bool("IncludeDettached"))
            {
                return Ok(None);
            }
            Ok(Some(u))
        }
        // `PetEvaluator.cs`: `Master.GetValue().GetPet(PetType)`. Pets aren't modelled; the
        // prologue's main character has none (`UnitPartPetMaster` absent).
        "PetEvaluator" => {
            let m = get_unit(w, owner, ev.obj("Master"))?;
            if w.entity(m).parts.iter().any(|p| p.clr == "UnitPartPetMaster") {
                return Err(unsupported(w, "evaluator", "PetEvaluator (unit with pets)"));
            }
            Ok(None)
        }
        other => Err(unsupported(w, "evaluator", other)),
    }
}

/// `CompanionInParty.IsCompanion(unit)`: the blueprint is the companion, or its
/// `PrototypeLink` chain reaches it (authoring metadata the `.bbp` keeps as null).
fn is_companion(w: &World, unit_bp: wr_content::Guid, comp: wr_content::Guid) -> bool {
    let _ = w;
    unit_bp == comp
}

/// The unit a spawner's `SpawnedUnit` names, if that id is a unit entity.
#[must_use]
pub fn spawned_unit(w: &World, spawner: &str) -> Option<EntityIdx> {
    let s = w.player.spawners.get(spawner)?;
    let id = s.spawned_unit.as_ref()?;
    let e = *w.by_id.get(id)?;
    w.unit(e).map(|_| e)
}

/// An int evaluator's value.
pub fn int_value(w: &mut World, _owner: &Rc<Blueprint>, ev: View<'_>) -> Result<i32> {
    w.coverage.executed(format!("evaluator {}", ev.type_name()));
    match ev.type_name() {
        // `IntConstant.cs`.
        "IntConstant" => Ok(ev.i32("Value")),
        // `FlagValue.cs`: `Flag.Value` (`UnlockableFlagsManager.GetFlagValue`: 0 when
        // locked).
        "FlagValue" => match ev.guid("m_Flag") {
            Some(f) => Ok(crate::narrative::flags::value(w, f)),
            None => Err(ElementError::Other("NullReferenceException: flag".into())),
        },
        other => Err(unsupported(w, "evaluator", other)),
    }
}

/// Fails with the evaluator name (for evaluators whose kind the caller expected).
#[must_use]
pub fn fail(ev: View<'_>) -> ElementError {
    ElementError::FailToEvaluate(ev.type_name().into())
}
