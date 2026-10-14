//! Components that react to level-ups. Sources (decompiled, not copied):
//! `Kingmaker.Designers.Mechanics.Facts/ToughnessLogic.cs`,
//! `Kingmaker.UnitLogic.FactLogic/AddSkillPointPerCharacterLevel.cs`,
//! `Kingmaker.Designers.Mechanics.Facts/AddFeatureOnClassLevel.cs`, `AddFeatureIfHasFact.cs`,
//! `ReplaceCasterLevelOfAbility.cs` (`CalculateClassLevel`), `SavesFixerRecalculate.cs`,
//! `Kingmaker.Blueprints/PretendUnit.cs`, `Kingmaker.UnitLogic.FactLogic/AddMythicEnemyHitPointsBonus.cs`.

use serde_json::{Value, json};

use super::{Port, bits, owner_unit, with_settings};
use crate::registry::{Hook, Registry};
use crate::stats::ops;
use crate::stats::types::{ModifierDescriptor as D, StatType};
use crate::world::{CompRef, EntityIdx, FactIdx, World};

pub fn register(r: &mut Registry) {
    let mut p = Port::new("ToughnessLogic", bits(&[Hook::TurnOn, Hook::TurnOff]), toughness_hook);
    p.handlers = &["IOwnerGainLevelHandler"];
    p.ev = Some(toughness_event);
    r.register(Box::new(p));
    let mut p = Port::new("AddSkillPointPerCharacterLevel", 0, |_, _, _| {});
    p.handlers = &["IUnitCalculateSkillPointsOnLevelupHandler"];
    p.ev = Some(skill_point_per_level_event);
    r.register(Box::new(p));
    let mut p = Port::new(
        "AddFeatureIfHasFact",
        bits(&[Hook::Activate, Hook::Deactivate]),
        add_feature_if_has_fact,
    );
    p.handlers = &["IOwnerGainLevelHandler"];
    p.ev = Some(add_feature_if_has_fact_event);
    r.register(Box::new(p));
    r.register(Box::new(Port::new(
        "SavesFixerRecalculate",
        bits(&[Hook::Activate, Hook::ApplyPostLoadFixes]),
        saves_fixer_recalculate,
    )));
    r.register(Box::new(Port::new(
        "RemoveFeatureOnApply",
        bits(&[Hook::Activate]),
        remove_feature_on_apply,
    )));
    r.register(Box::new(Port::new(
        "PretendUnit",
        bits(&[Hook::Activate, Hook::Deactivate]),
        pretend_unit,
    )));
    let mut p = Port::new(
        "AddMythicEnemyHitPointsBonus",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        mythic_enemy_hp_hook,
    );
    p.handlers = &["IUnitReapplyFeaturesOnLevelUpHandler"];
    p.ev = Some(mythic_enemy_hp_event);
    r.register(Box::new(p));
}

/// `AddMythicEnemyHitPointsBonus.SetActive(active)`: on, the owner's hit points get
/// `CR * 2 * MythicLevel` (`Mythic`) when positive; off, the component's modifiers go.
fn mythic_enemy_hp_set_active(w: &mut World, c: CompRef, active: bool) {
    let Some(u) = owner_unit(w, c) else { return };
    if active {
        // `Owner.CR`: `Blueprint.CR`.
        let cr = super::difficulty::blueprint_cr(w, w.unit(u).and_then(|x| x.blueprint));
        let n = crate::csharp::arith::mul(
            crate::csharp::arith::mul(cr, 2),
            crate::unit::mythic_level(w, u),
        );
        if n > 0 {
            ops::add_modifier_from_component(w, u, StatType::HitPoints, n, c, D::Mythic);
        }
    } else {
        ops::remove_modifiers_from(w, u, StatType::HitPoints, c);
    }
}

fn mythic_enemy_hp_hook(w: &mut World, c: CompRef, h: Hook) {
    match h {
        Hook::TurnOn => mythic_enemy_hp_set_active(w, c, true),
        Hook::TurnOff => mythic_enemy_hp_set_active(w, c, false),
        _ => {}
    }
}

/// `HandleUnitReapplyFeaturesOnLevelUp`: if the fact is on, off and on again.
fn mythic_enemy_hp_event(w: &mut World, c: CompRef, _iface: &str) {
    if w.fact(c.fact).is_turned_on {
        mythic_enemy_hp_set_active(w, c, false);
        mythic_enemy_hp_set_active(w, c, true);
    }
}

/// `ToughnessLogic.Apply`: the owner's hit points get `max(3, level)` (mythic level when
/// `CheckMythicLevel`), replacing the component's earlier modifier.
fn toughness_apply(w: &mut World, c: CompRef) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some(mythic) = with_settings(w, c, |v| v.bool("CheckMythicLevel")) else { return };
    ops::remove_modifiers_from(w, u, StatType::HitPoints, c);
    let n =
        if mythic { crate::unit::mythic_level(w, u) } else { crate::unit::character_level(w, u) };
    let value = if mythic { n } else { 3.max(n) };
    ops::add_modifier_from_component(w, u, StatType::HitPoints, value, c, D::UntypedStackable);
}

fn toughness_hook(w: &mut World, c: CompRef, h: Hook) {
    match h {
        Hook::TurnOn => toughness_apply(w, c),
        Hook::TurnOff => {
            if let Some(u) = owner_unit(w, c) {
                ops::remove_modifiers_from(w, u, StatType::HitPoints, c);
            }
        }
        _ => {}
    }
}

fn toughness_event(w: &mut World, c: CompRef, _iface: &str) {
    toughness_apply(w, c);
}

/// `AddSkillPointPerCharacterLevel.HandleUnitCalculateSkillPointsOnLevelup`:
/// `extraSkillPoints += state.NextCharacterLevel`.
fn skill_point_per_level_event(w: &mut World, _c: CompRef, _iface: &str) {
    if let Some((next_level, extra)) = w.bus.skill_points.last_mut() {
        *extra = extra.wrapping_add(*next_level);
    }
}

fn applied_fact(w: &World, c: CompRef) -> Option<FactIdx> {
    w.component(c).fact_refs.get("AppliedFact").and_then(|v| v.first().copied().flatten())
}

fn set_applied_fact(w: &mut World, c: CompRef, f: Option<FactIdx>) {
    w.component_mut(c).fact_refs.insert("AppliedFact".to_owned(), vec![f]);
}

/// `UnitDescriptor.RemoveFact(fact)` (null-safe).
fn remove_fact(w: &mut World, u: EntityIdx, f: Option<FactIdx>) {
    if let Some(f) = f
        && w.fact(f).owner == Some(u)
    {
        crate::unit_facts::remove(w, u, f);
    }
}

/// `ReplaceCasterLevelOfAbility.CheckArchetype`.
fn check_archetype(
    w: &World,
    u: EntityIdx,
    class: wr_content::Guid,
    archetypes: &[wr_content::Guid],
) -> bool {
    let class_archetypes =
        w.content.get(class).map(|b| b.view().guids("m_Archetypes")).unwrap_or_default();
    if archetypes.is_empty() || !archetypes.iter().all(|a| class_archetypes.contains(a)) {
        return true;
    }
    let data = w.unit(u).and_then(|x| x.progression.classes.iter().find(|cd| cd.class == class));
    archetypes.iter().any(|a| data.is_some_and(|cd| cd.archetypes.contains(a)))
}

/// `ReplaceCasterLevelOfAbility.CalculateClassLevel`.
fn calculate_class_level(
    w: &World,
    u: EntityIdx,
    class: Option<wr_content::Guid>,
    additional: &[wr_content::Guid],
    archetypes: &[wr_content::Guid],
) -> i32 {
    let Some(class) = class else { return crate::unit::character_level(w, u) };
    let level_of = |g: wr_content::Guid| -> i32 {
        w.unit(u)
            .and_then(|x| x.progression.classes.iter().find(|cd| cd.class == g))
            .map_or(0, |cd| cd.level)
    };
    let mut n = 0i32;
    if check_archetype(w, u, class, archetypes) {
        n = n.wrapping_add(level_of(class));
    }
    for g in additional {
        if check_archetype(w, u, *g, archetypes) {
            n = n.wrapping_add(level_of(*g));
        }
    }
    n
}

/// `AddFeatureOnClassLevel.Apply`.
pub(super) fn add_feature_on_class_level_apply(w: &mut World, c: CompRef) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((class, level, feature, before, additional, archetypes)) = with_settings(w, c, |v| {
        (
            v.guid("m_Class"),
            v.i32("Level"),
            v.guid("m_Feature"),
            v.bool("BeforeThisLevel"),
            v.guids("m_AdditionalClasses"),
            v.guids("m_Archetypes"),
        )
    }) else {
        return;
    };
    let n = calculate_class_level(w, u, class, &additional, &archetypes);
    // `IsFeatureShouldBeApplied`.
    let should = if before && n >= level {
        false
    } else if n >= level || !before {
        n >= level && !before
    } else {
        true
    };
    if should {
        if applied_fact(w, c).is_none()
            && let Some(bp) = feature.and_then(|g| w.content.get(g))
        {
            let f = crate::unit_facts::add_fact(w, u, &bp);
            set_applied_fact(w, c, f);
        }
    } else if let Some(f) = applied_fact(w, c) {
        remove_fact(w, u, Some(f));
        set_applied_fact(w, c, None);
    }
}

/// `OnDeactivate` of the components that keep one applied fact: `Owner.RemoveFact(Data.AppliedFact)`.
pub(super) fn remove_applied_fact(w: &mut World, c: CompRef) {
    if let Some(u) = owner_unit(w, c) {
        let f = applied_fact(w, c);
        remove_fact(w, u, f);
        set_applied_fact(w, c, None);
    }
}

pub(super) fn add_feature_on_class_level_event(w: &mut World, c: CompRef, _iface: &str) {
    add_feature_on_class_level_apply(w, c);
}

/// `AddFeatureIfHasFact.Apply`.
fn add_feature_if_has_fact_apply(w: &mut World, c: CompRef) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((checked, feature, not)) =
        with_settings(w, c, |v| (v.guid("m_CheckedFact"), v.guid("m_Feature"), v.bool("Not")))
    else {
        return;
    };
    if applied_fact(w, c).is_some() {
        return;
    }
    let has = checked
        .is_some_and(|g| w.entity(u).facts.list.iter().any(|f| w.fact(*f).blueprint == Some(g)));
    if has != not
        && let Some(bp) = feature.and_then(|g| w.content.get(g))
    {
        let f = crate::unit_facts::add_fact(w, u, &bp);
        set_applied_fact(w, c, f);
    }
}

fn add_feature_if_has_fact(w: &mut World, c: CompRef, h: Hook) {
    match h {
        Hook::Activate => add_feature_if_has_fact_apply(w, c),
        Hook::Deactivate => remove_applied_fact(w, c),
        _ => {}
    }
}

fn add_feature_if_has_fact_event(w: &mut World, c: CompRef, _iface: &str) {
    add_feature_if_has_fact_apply(w, c);
}

/// `SavesFixerRecalculate.ReapplyIfNecessary`: a missing or older `Version` bumps it and asks
/// the fact to reapply after the post-load fixes.
fn saves_fixer_recalculate(w: &mut World, c: CompRef, _h: Hook) {
    let Some(version) = with_settings(w, c, |v| v.i32("Version")) else { return };
    if w.fact(c.fact).reapplying {
        return;
    }
    let current =
        w.component(c).data.as_ref().and_then(|d| d.get("Version")).and_then(Value::as_i64);
    if current.is_none_or(|v| v < i64::from(version)) {
        // `Data.Version++` (a fresh `Data` starts at 0).
        let next = current.unwrap_or(0) + 1;
        w.component_mut(c).data = Some(json!({"Version": next}));
        w.fact_mut(c.fact).reapply_on_apply_post_load_fixes = true;
    }
}

/// `PretendUnit.OnActivate` / `OnDeactivate`: the owner's `Blueprint` becomes the pretended
/// unit's (`SetFakeBlueprint`) and back.
fn pretend_unit(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let applied =
        w.component(c).data.as_ref().and_then(|d| d.get("Applied")).and_then(Value::as_bool)
            == Some(true);
    match h {
        Hook::Activate => {
            let unit = w.unit(u).expect("unit");
            if unit.blueprint != unit.original_blueprint {
                w.log(
                    "error",
                    "PretendUnit.OnActivate: Owner.Blueprint != Owner.OriginalBlueprint",
                );
                return;
            }
            let Some(fake) = with_settings(w, c, |v| v.guid("m_Unit")).flatten() else { return };
            w.unit_mut(u).expect("unit").blueprint = Some(fake);
            w.component_mut(c).data = Some(json!({"Applied": true}));
        }
        Hook::Deactivate if applied => {
            let unit = w.unit_mut(u).expect("unit");
            unit.blueprint = unit.original_blueprint;
            w.component_mut(c).data = Some(json!({"Applied": false}));
        }
        _ => {}
    }
}

/// `RemoveFeatureOnApply.OnActivate`: `Owner.RemoveFact(Feature)`, the first fact of that
/// blueprint.
fn remove_feature_on_apply(w: &mut World, c: CompRef, h: Hook) {
    if h != Hook::Activate {
        return;
    }
    let Some(u) = owner_unit(w, c) else { return };
    let Some(feature) = with_settings(w, c, |v| v.guid("m_Feature")).flatten() else { return };
    let f = w.entity(u).facts.list.iter().copied().find(|f| w.fact(*f).blueprint == Some(feature));
    remove_fact(w, u, f);
}
