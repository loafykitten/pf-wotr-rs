//! Unit inspection on death (decompiled, not copied): `InspectUnitsController`
//! (`Kingmaker.Controllers`) and `InspectUnitsManager.TryMakeKnowledgeCheck(unit)`
//! (`Kingmaker.Inspect`). A unit that dies is queued; the controller's next tick makes the
//! party's knowledge check on its blueprint, adding the blueprint's `UnitInfo` (saved) if it has
//! none. The check itself (`RuleSkillCheck`, `UnitInfo.SetCheck`) isn't ported: a party member
//! who would roll is reported. A unit revealed by fog of war (`HandleEntityRevealed`) is queued
//! when the recording says it was revealed (WrathOracle's `inspect_reveal`): the sim doesn't run
//! fog of war.

use wr_content::Guid;

use crate::stats::types::StatType;
use crate::world::{EntityIdx, World};

/// `InspectUnitsManager.UnitInfo` (saved in `Player.InspectUnitsManager.m_UnitInfos`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitInfo {
    pub blueprint: Guid,
    /// `m_Parts` (`UnitInfoPart` flags, as saved).
    pub parts: String,
    /// `m_CheckValue`.
    pub check_value: i32,
}

/// `MechanicsFeatureType.MakeKnowledgeCheckUntrained`.
const MAKE_KNOWLEDGE_CHECK_UNTRAINED: i64 = 29;

/// `BlueprintUnit.CR`: its `Experience` component's, except none or the placeholder `CR 1` with
/// `Modifier 0.5` (0).
fn blueprint_cr(w: &World, bp: Guid) -> i32 {
    let Some(b) = w.content.get(bp) else { return 0 };
    let Some(e) = b
        .components_array()
        .into_iter()
        .find(|o| wr_content::schema::ty(o.ty).short == "Experience")
        .map(crate::content::View)
    else {
        return 0;
    };
    let (cr, modifier) = (e.i32("CR"), e.f32("Modifier"));
    if cr == 1 && (modifier - 0.5).abs() < 0.001 { 0 } else { cr }
}

/// `UnitInfo.KnownPartsCount`: `Min(4, 1 + margin / 5)` past the DC (`10 + Floor(CR * 1.25f)`),
/// else 0.
fn known_parts(w: &World, info: &UnitInfo) -> i32 {
    let cr = blueprint_cr(w, info.blueprint);
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let dc = 10 + ((cr as f32) * 1.25f32).floor() as i32;
    let margin = info.check_value - dc;
    if margin < 0 { 0 } else { (1 + margin / 5).min(4) }
}

/// `InspectUnitsController.HandleEntityRevealed` of a unit: queued when the party has no
/// `UnitInfo` of its `BlueprintForInspection`; whether the call added it to the set. A polymorph's
/// replacement blueprint (`ReplaceBlueprintForInspection`) is reported (`None`).
pub fn entity_revealed(w: &mut World, u: EntityIdx) -> Option<bool> {
    if polymorphed(w, u) {
        w.coverage.unsupported("InspectUnitsController: a polymorphed unit's inspection blueprint");
        return None;
    }
    // `BlueprintForInspection`: `ReplaceBlueprintForInspection ?? Blueprint`.
    let bp = w.unit(u).and_then(|x| x.replace_blueprint_for_inspection.or(x.blueprint));
    if w.player.inspect_infos.iter().any(|x| Some(x.blueprint) == bp) {
        return Some(false);
    }
    let added = !w.combat.inspect_queue.contains(&u);
    queue(w, u);
    Some(added)
}

/// A unit with a `Polymorph` fact: the sim doesn't port its activation (`UnitBody.
/// ApplyPolymorphEffect`, which sets `Body.IsPolymorphed`, and `ReplaceBlueprintForInspection`), so
/// a reader of those reports it.
pub(crate) fn polymorphed(w: &World, u: EntityIdx) -> bool {
    w.entity(u).facts.list.iter().any(|f| {
        w.fact(*f).bp.as_ref().is_some_and(|b| {
            b.components_array().iter().any(|o| wr_content::schema::ty(o.ty).short == "Polymorph")
        })
    })
}

/// `InspectUnitsController.HandleUnitDeath`: the unit is queued (a `HashSet`).
pub fn queue(w: &mut World, u: EntityIdx) {
    if !w.combat.inspect_queue.contains(&u) {
        w.combat.inspect_queue.push(u);
    }
}

/// `InspectUnitsHelper.IsInspectAllow(unit)`.
fn inspect_allowed(w: &mut World, u: EntityIdx) -> bool {
    let has = |w: &World, p: &str| w.entity(u).parts.iter().any(|x| x.clr == p);
    if super::units::is_players_enemy(w, u)
        || super::units::faction_neutral(w, u)
        || super::is_in_combat(w, u)
        || has(w, "UnitPartSummonedMonster")
        || has(w, "UnitPartTacticalCombat")
    {
        return true;
    }
    // `unit.IsLeader()`.
    w.coverage.unsupported("InspectUnitsHelper.IsInspectAllow: UnitEntityData.IsLeader");
    false
}

/// `InspectUnitsController.Tick()`: each queued unit's knowledge check, then the queue cleared.
pub fn tick(w: &mut World) {
    let queued = std::mem::take(&mut w.combat.inspect_queue);
    let mut rolls = 0;
    for &u in &queued {
        rolls += try_make_knowledge_check(w, u);
    }
    if rolls > 0 && queued.len() > 1 {
        // `HashSet<UnitEntityData>` order follows object hashes.
        w.coverage.unsupported("InspectUnitsController: the order of several queued units");
    }
}

/// `InspectUnitsManager.TryMakeKnowledgeCheck(unit)`; the number of party members who would roll.
fn try_make_knowledge_check(w: &mut World, u: EntityIdx) -> usize {
    if !inspect_allowed(w, u) {
        return 0;
    }
    if polymorphed(w, u) {
        // `ReplaceBlueprintForInspection` (a polymorph's).
        w.coverage.unsupported("InspectUnitsManager: a polymorphed unit's inspection blueprint");
        return 0;
    }
    let Some(bp) = w.unit(u).and_then(|x| x.blueprint) else { return 0 };
    let i = match w.player.inspect_infos.iter().position(|x| x.blueprint == bp) {
        Some(i) => i,
        None => {
            w.player.inspect_infos.push(UnitInfo {
                blueprint: bp,
                parts: "None".to_owned(),
                check_value: 0,
            });
            w.player.inspect_infos.len() - 1
        }
    };
    let info = w.player.inspect_infos[i].clone();
    if known_parts(w, &info) == 4 {
        return 0;
    }
    // `Blueprint.Type ? Type.KnowledgeStat : SkillLoreNature`.
    let stat = w
        .content
        .get(bp)
        .and_then(|b| b.view().guid("m_Type"))
        .and_then(|t| w.content.get(t))
        .and_then(|t| i32::try_from(t.view().int("KnowledgeStat")).ok())
        .map_or(StatType::SkillLoreNature, StatType);
    let mut rolls = 0;
    for m in crate::narrative::party::character_lists(w).party {
        if !super::units::is_conscious(w, m) {
            continue;
        }
        let trained = w.stats(m).v(stat).base_value > 0;
        if trained || super::weapon::has_feature(w, m, MAKE_KNOWLEDGE_CHECK_UNTRAINED) {
            rolls += 1;
        }
    }
    if rolls > 0 {
        w.coverage.unsupported(
            "InspectUnitsManager.TryMakeKnowledgeCheck: the knowledge check (RuleSkillCheck)",
        );
    }
    // `IKnowledgeHandler.HandleKnowledgeUpdated`: the UI's, and the game log's handler
    // (`GameLogEventKnowledge`: no thread handles it, but it takes a place in the queue).
    crate::game_log::knowledge_updated(w);
    rolls
}
