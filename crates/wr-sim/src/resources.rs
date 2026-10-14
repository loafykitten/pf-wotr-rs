//! Ability resources (decompiled, not copied: `Kingmaker.UnitLogic/UnitAbilityResource.cs`,
//! `UnitAbilityResourceCollection.cs`, `Kingmaker.Blueprints/BlueprintAbilityResource.cs`):
//! the unit's counted resources, their maximum from class levels and stats plus the bonus
//! components raised through `IResourceAmountBonusHandler`, and restoring them.

use wr_content::Guid;

use crate::content::{Blueprint, View, is_a};
use crate::csharp::arith;
use crate::stats::ops;
use crate::stats::types::StatType;
use crate::stats::value::StatKind;
use crate::world::{EntityIdx, World};

/// A `UnitAbilityResource`.
#[derive(Debug, Clone)]
pub struct AbilityResource {
    pub blueprint: Guid,
    pub amount: i32,
    pub retain_count: i32,
}

/// `MechanicsFeatureType.DomainsByIntelligence`, by its name in the content schema.
fn domains_by_intelligence(w: &World, u: EntityIdx) -> bool {
    crate::components::state::has_mechanics_feature(w, u, &["DomainsByIntelligence"])
}

/// `UnitAbilityResource.GetMaxAmount(owner)`.
pub fn max_amount(w: &mut World, u: EntityIdx, resource: Guid) -> i32 {
    let Some(b) = w.content.get(resource) else { return 0 };
    if is_a(b.obj.ty, "BlueprintAbilityResource") {
        return blueprint_max_amount(w, u, &b);
    }
    // `AddAbilityResources.UseThisAsResource`: the component's amount.
    for o in b.components_array() {
        if is_a(o.ty, "AddAbilityResources") && View(o).bool("UseThisAsResource") {
            return View(o).i32("Amount");
        }
    }
    w.log("error", "Can't extract resource amount");
    0
}

fn class_level(w: &World, u: EntityIdx, class: Guid) -> i32 {
    w.unit(u)
        .and_then(|x| x.progression.classes.iter().find(|c| c.class == class))
        .map_or(0, |c| c.level)
}

/// The archetype test of the class loops: after the last archetype checked, `classData ==
/// null || !class.Archetypes.Contains(archetype) || classData.Archetypes.Contains(archetype)`.
fn archetype_ok(w: &World, u: EntityIdx, class: Guid, archetypes: &[Guid]) -> bool {
    let mut flag = true;
    for a in archetypes {
        let data = w.unit(u).and_then(|x| x.progression.classes.iter().find(|c| c.class == class));
        let in_class =
            w.content.get(class).is_some_and(|c| c.view().guids("m_Archetypes").contains(a));
        flag = data.is_none_or(|d| !in_class || d.archetypes.contains(a));
    }
    flag
}

/// `BlueprintAbilityResource.GetMaxAmount(unit)`.
fn blueprint_max_amount(w: &mut World, u: EntityIdx, bp: &Blueprint) -> i32 {
    let v = bp.view();
    let Some(m) = v.obj("m_MaxAmount") else { return 0 };
    let mut num = m.i32("BaseValue");
    if m.bool("IncreasedByLevel") {
        let archetypes = m.guids("m_Archetypes");
        for class in m.guids("m_Class") {
            let ok = archetype_ok(w, u, class, &archetypes);
            let add =
                if ok { arith::mul(m.i32("LevelIncrease"), class_level(w, u, class)) } else { 0 };
            num = arith::add(num, add);
        }
    }
    if m.bool("IncreasedByStat") {
        let mut t = StatType(m.i32("ResourceBonusStat"));
        if domains_by_intelligence(w, u) && v.bool("IsDomainAbility") {
            t = StatType::Intelligence;
        }
        if matches!(w.stats(u).v(t).kind, StatKind::Attribute { .. }) {
            let perm = ops::permanent_value_without_temp_buffs(w, u, t);
            num = arith::add(num, arith::sub(perm / 2, 5));
        } else {
            w.log("error", "Can't use stat in ability resource's count formula");
        }
    }
    if m.bool("IncreasedByLevelStartPlusDivStep") {
        let mut num2 = 0i32;
        let archetypes = m.guids("m_ArchetypesDiv");
        for class in m.guids("m_ClassDiv") {
            let ok = archetype_ok(w, u, class, &archetypes);
            num2 = arith::add(num2, if ok { class_level(w, u, class) } else { 0 });
        }
        let character_level = crate::unit::character_level(w, u);
        // `(int)((float)(CharacterLevel - num2) * OtherClassesModifier)`: float operands, the
        // product evaluated in double and truncated.
        let product = f64::from(arith::sub(character_level, num2) as f32)
            * f64::from(m.f32("OtherClassesModifier"));
        num2 = arith::add(num2, arith::to_int(product));
        if m.i32("StartingLevel") <= num2 {
            if m.i32("LevelStep") == 0 {
                w.log("error", "m_MaxAmount.LevelStep is 0. Can't divide by 0");
            } else {
                let step =
                    arith::mul(m.i32("PerStepIncrease"), arith::sub(num2, m.i32("StartingLevel")))
                        / m.i32("LevelStep");
                let inc =
                    arith::add(m.i32("StartingIncrease"), step).max(m.i32("MinClassLevelIncrease"));
                num = arith::add(num, inc);
            }
        }
    }
    // `EventBus.RaiseEvent(unit, h => h.CalculateMaxResourceAmount(this, ref bonus))`.
    w.bus.resource_bonus.push((bp.guid, 0));
    crate::rulebook::bus::raise_unit(w, u, "IResourceAmountBonusHandler");
    let bonus = w.bus.resource_bonus.pop().map_or(0, |(_, b)| b);
    let capped = if v.bool("m_UseMax") { num.min(v.i32("m_Max")) } else { num };
    v.i32("m_Min").max(arith::add(capped, bonus))
}

/// `UnitAbilityResourceCollection.Add(blueprint, restoreAmount)`.
pub fn add(w: &mut World, u: EntityIdx, resource: Guid, restore_amount: bool) {
    let unit = w.unit_mut(u).expect("unit");
    let r = unit.resources.get_or_insert_with(resource, || AbilityResource {
        blueprint: resource,
        amount: 0,
        retain_count: 0,
    });
    r.retain_count += 1;
    if restore_amount {
        let max = max_amount(w, u, resource);
        if let Some(r) = w.unit_mut(u).and_then(|x| x.resources.get_mut(&resource)) {
            r.amount = max;
        }
    }
}

/// `UnitAbilityResourceCollection.Remove(blueprint)`.
pub fn remove(w: &mut World, u: EntityIdx, resource: Guid) {
    let unit = w.unit_mut(u).expect("unit");
    let Some(r) = unit.resources.get_mut(&resource) else {
        w.log("error", "UnitAbilityResourceCollection.Remove: has no resource");
        return;
    };
    r.retain_count -= 1;
    if r.retain_count < 1 {
        unit.resources.remove(&resource);
    }
}

/// `UnitAbilityResourceCollection.Restore(blueprint)`: back to the maximum.
pub fn restore(w: &mut World, u: EntityIdx, resource: Guid) {
    if !w.unit(u).is_some_and(|x| x.resources.contains_key(&resource)) {
        w.log("error", "UnitAbilityResourceCollection.Restore: has no resource");
        return;
    }
    let old = w.unit(u).and_then(|x| x.resources.get(&resource)).map_or(0, |r| r.amount);
    let max = max_amount(w, u, resource);
    if let Some(r) = w.unit_mut(u).and_then(|x| x.resources.get_mut(&resource)) {
        r.amount = max;
    }
    let _ = old;
    crate::rulebook::bus::raise_global_call(
        w,
        "IUnitAbilityResourceHandler",
        "HandleAbilityResourceChange",
        Some(u),
    );
}

/// `UnitHelper.Replenish`'s resources: every resource back to its maximum.
pub fn restore_all(w: &mut World, u: EntityIdx) {
    let keys: Vec<Guid> = w.unit(u).map(|x| x.resources.keys().collect()).unwrap_or_default();
    for k in keys {
        restore(w, u, k);
    }
}

/// The `resources[]` entries of a sheet, ascending by blueprint GUID.
#[must_use]
pub fn dump(w: &World, u: EntityIdx) -> Vec<serde_json::Value> {
    let Some(unit) = w.unit(u) else { return Vec::new() };
    let mut rs: Vec<&AbilityResource> = unit.resources.iter().map(|(_, r)| r).collect();
    rs.sort_by_key(|r| r.blueprint.to_string());
    rs.into_iter()
        .map(|r| {
            let g = r.blueprint.to_string();
            serde_json::json!({
                "blueprint": g, "amount": r.amount,
                "persisted": {"Blueprint": g, "Amount": r.amount, "RetainCount": r.retain_count},
            })
        })
        .collect()
}
