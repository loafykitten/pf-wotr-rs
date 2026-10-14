//! Components that give a unit spellbook content, ability resources and activatable-ability
//! group sizes. Sources (decompiled, not copied): `Kingmaker.UnitLogic.FactLogic/
//! AddSpecialSpellList.cs`, `AddSpecialSpellListForArchetype.cs`, `LearnSpells.cs`,
//! `IncreaseActivatableAbilityGroupSize.cs`, `IncreaseResourceAmount.cs`,
//! `IncreaseResourcesByClass.cs`, `IncreaseResourceAmountBySharedValue.cs`,
//! `Kingmaker.Designers.Mechanics.Facts/AddAbilityResources.cs`, `AutoMetamagic.cs`
//! (`OnTurnOn`/`OnTurnOff` only: its rule handlers aren't ported and report when they fire),
//! `Kingmaker.UnitLogic.Parts/UnitPartActivatableAbility.cs`, `UnitPartSpecialMetamagic.cs`.

use wr_content::Guid;

use super::{Port, bits, owner_unit, with_settings};
use crate::content::{View, is_a};
use crate::registry::{Hook, Registry};
use crate::world::{CompRef, EntityIdx, World};

pub fn register(r: &mut Registry) {
    r.register(Box::new(Port::new(
        "AddSpecialSpellList",
        bits(&[Hook::Activate, Hook::Deactivate]),
        add_special_spell_list,
    )));
    r.register(Box::new(Port::new(
        "AddSpecialSpellListForArchetype",
        bits(&[Hook::Activate, Hook::Deactivate]),
        add_special_spell_list_for_archetype,
    )));
    r.register(Box::new(Port::new("LearnSpells", bits(&[Hook::Activate]), learn_spells)));
    r.register(Box::new(Port::new(
        "AddSpellsPerDay",
        bits(&[Hook::Activate, Hook::Deactivate]),
        add_spells_per_day,
    )));
    let mut p = Port::new("AddKnownSpell", bits(&[Hook::Activate]), add_known_spell);
    p.handlers = &["ILevelUpCompleteUIHandler"];
    p.ev = Some(add_known_spell_event);
    r.register(Box::new(p));
    let mut p = Port::new("LearnSpellList", bits(&[Hook::TurnOn]), learn_spell_list);
    p.handlers = &["ILevelUpCompleteUIHandler"];
    p.ev = Some(learn_spell_list_event);
    r.register(Box::new(p));
    let mut p = Port::new(
        "AddAbilityResources",
        bits(&[Hook::Activate, Hook::Deactivate]),
        add_ability_resources,
    );
    p.handlers = &["IUnitReapplyFeaturesOnLevelUpHandler"];
    p.ev = Some(add_ability_resources_event);
    r.register(Box::new(p));
    for name in [
        "IncreaseResourceAmount",
        "IncreaseResourcesByClass",
        "IncreaseResourceAmountBySharedValue",
    ] {
        let mut p = Port::new(name, 0, |_, _, _| {});
        p.handlers = &["IResourceAmountBonusHandler"];
        p.ev = Some(resource_bonus_event);
        r.register(Box::new(p));
    }
    r.register(Box::new(Port::new(
        "IncreaseActivatableAbilityGroupSize",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        increase_group_size,
    )));
    r.register(Box::new(Port::new(
        "AutoMetamagic",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        auto_metamagic,
    )));
}

/// `UnitProgressionData.GetClassData(class)?.Archetypes`.
fn class_archetypes(w: &World, u: EntityIdx, class: Guid) -> Option<Vec<Guid>> {
    w.unit(u)?.progression.classes.iter().find(|c| c.class == class).map(|c| c.archetypes.clone())
}

/// `UnitDescriptor.DemandSpellbook(characterClass)`: the spellbook of the unit's class data;
/// without class data, the class's own spellbook; a class with none can't give one.
fn demand_class_spellbook(w: &mut World, u: EntityIdx, class: Guid) -> Option<Guid> {
    let data =
        w.unit(u).and_then(|x| x.progression.classes.iter().find(|c| c.class == class).cloned());
    let sb = match data {
        Some(cd) => cd.spellbook,
        None => w.content.get(class).and_then(|c| c.view().guid("m_Spellbook")),
    };
    let Some(sb) = sb else {
        w.coverage.unsupported("UnitDescriptor.DemandSpellbook: class without a spellbook");
        return None;
    };
    crate::spellbook::demand_spellbook(w, u, sb);
    Some(sb)
}

/// `AddSpecialSpellList.OnActivate` / `OnDeactivate`.
fn add_special_spell_list(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((class, list, for_archetype, archetype)) = with_settings(w, c, |v| {
        (
            v.guid("m_CharacterClass"),
            v.guid("m_SpellList"),
            v.bool("ForArchetypeOnly"),
            v.guid("m_Archetype"),
        )
    }) else {
        return;
    };
    let (Some(class), Some(list)) = (class, list) else { return };
    match h {
        Hook::Activate => {
            // `Progression.IsArchetype(archetype)`: some class data carries it.
            let is_archetype = archetype.is_some_and(|a| {
                w.unit(u).is_some_and(|x| {
                    x.progression.classes.iter().any(|c| c.archetypes.contains(&a))
                })
            });
            if (!for_archetype || is_archetype)
                && let Some(sb) = demand_class_spellbook(w, u, class)
            {
                crate::spellbook::add_special_list(w, u, sb, list);
            }
        }
        Hook::Deactivate => remove_list(w, u, class, list),
        _ => {}
    }
}

/// `Owner.GetSpellbook(characterClass)?.RemoveSpecialList(spellList)`.
fn remove_list(w: &mut World, u: EntityIdx, class: Guid, list: Guid) {
    if let Some(sb) = crate::spellbook::class_spellbook(w, u, class) {
        crate::spellbook::remove_special_list(w, u, sb, list);
    }
}

/// `AddSpecialSpellListForArchetype.OnActivate` / `OnDeactivate`.
fn add_special_spell_list_for_archetype(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((class, list, archetype)) = with_settings(w, c, |v| {
        (v.guid("m_CharacterClass"), v.guid("m_SpellList"), v.guid("m_Archetype"))
    }) else {
        return;
    };
    let (Some(class), Some(list)) = (class, list) else { return };
    match h {
        Hook::Activate => {
            let has = archetype.is_some_and(|a| {
                class_archetypes(w, u, class).is_some_and(|archs| archs.contains(&a))
            });
            if has && let Some(sb) = demand_class_spellbook(w, u, class) {
                crate::spellbook::add_special_list(w, u, sb, list);
            }
        }
        Hook::Deactivate => remove_list(w, u, class, list),
        _ => {}
    }
}

/// `LearnSpells.OnActivate`: each spell on the class's spell list is learned at its level.
fn learn_spells(w: &mut World, c: CompRef, h: Hook) {
    if h != Hook::Activate {
        return;
    }
    let Some(u) = owner_unit(w, c) else { return };
    let Some((class, spells)) =
        with_settings(w, c, |v| (v.guid("m_CharacterClass"), v.guids("m_Spells")))
    else {
        return;
    };
    let Some(class) = class else { return };
    let Some(sb) = crate::spellbook::class_spellbook(w, u, class) else { return };
    let Some(sbp) = w.content.get(sb) else { return };
    let list = sbp.view().guid("m_SpellList");
    let filtered =
        list.and_then(|l| w.content.get(l)).and_then(|l| l.view().guid("m_FilteredList"));
    for spell in spells {
        let Some(sp) = w.content.get(spell) else { continue };
        for o in sp.components_array() {
            if !is_a(o.ty, "SpellListComponent") {
                continue;
            }
            let v = View(o);
            let sl = v.guid("m_SpellList");
            if sl.is_some() && (sl == list || sl == filtered) {
                crate::spellbook::add_known(w, u, sb, v.i32("SpellLevel"), spell, true);
            }
        }
    }
}

/// `AddSpellsPerDay.OnActivate` / `OnDeactivate`: the bonus per spell level on the unit's part.
fn add_spells_per_day(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((amount, levels)) = with_settings(w, c, |v| {
        let levels: Vec<usize> = v
            .seq("Levels")
            .iter()
            .filter_map(|n| match n {
                wr_content::Node::Int(i) | wr_content::Node::Enum(i) => usize::try_from(*i).ok(),
                _ => None,
            })
            .collect();
        (v.i32("Amount"), levels)
    }) else {
        return;
    };
    let sign = match h {
        Hook::Activate => 1,
        Hook::Deactivate => -1,
        _ => return,
    };
    ensure_part(w, u, "UnitPartExtraSpellsPerDay");
    let bonus =
        w.unit_mut(u).expect("unit").extra_spells_per_day.get_or_insert_with(|| vec![0; 11]);
    for l in levels {
        if l >= bonus.len() {
            bonus.resize(l + 1, 0);
        }
        bonus[l] = crate::csharp::arith::add(bonus[l], crate::csharp::arith::mul(sign, amount));
    }
}

/// `AddKnownSpell.AddSpell`.
fn add_known_spell_apply(w: &mut World, c: CompRef) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((class, level, spell, archetype, exclude)) = with_settings(w, c, |v| {
        (
            v.guid("m_CharacterClass"),
            v.i32("SpellLevel"),
            v.guid("m_Spell"),
            v.guid("m_Archetype"),
            v.guid("m_ExcludeArchetype"),
        )
    }) else {
        return;
    };
    let (Some(class), Some(spell)) = (class, spell) else { return };
    let Some(data) =
        w.unit(u).and_then(|x| x.progression.classes.iter().find(|c| c.class == class).cloned())
    else {
        return;
    };
    if archetype.is_some_and(|a| !data.archetypes.contains(&a))
        || exclude.is_some_and(|a| data.archetypes.contains(&a))
    {
        return;
    }
    let Some(target) = data.spellbook else { return };
    for sb in crate::spellbook::spellbooks_of(w, u) {
        let is_class_book = sb == target;
        let known = crate::spellbook::knows_at(w, u, sb, level, spell);
        let enough = crate::spellbook::max_spell_level(w, u, sb) >= level;
        if is_class_book && !known && enough {
            crate::spellbook::add_known(w, u, sb, level, spell, true);
        }
    }
}

fn add_known_spell(w: &mut World, c: CompRef, h: Hook) {
    if h == Hook::Activate {
        add_known_spell_apply(w, c);
    }
}

fn add_known_spell_event(w: &mut World, c: CompRef, _iface: &str) {
    add_known_spell_apply(w, c);
}

/// `LearnSpellList.LearnList`.
fn learn_spell_list_apply(w: &mut World, c: CompRef) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((class, list, archetype)) = with_settings(w, c, |v| {
        (v.guid("m_CharacterClass"), v.guid("m_SpellList"), v.guid("m_Archetype"))
    }) else {
        return;
    };
    let (Some(class), Some(list)) = (class, list) else { return };
    let Some(data) =
        w.unit(u).and_then(|x| x.progression.classes.iter().find(|c| c.class == class).cloned())
    else {
        return;
    };
    if archetype.is_some_and(|a| !data.archetypes.contains(&a)) {
        return;
    }
    let Some(sb) = demand_class_spellbook(w, u, class) else { return };
    let Some(l) = w.content.get(list) else { return };
    let levels: Vec<i32> =
        l.view().objs("SpellsByLevel").into_iter().map(|o| View(o).i32("SpellLevel")).collect();
    for (i, level) in levels.into_iter().enumerate() {
        if crate::spellbook::max_spell_level(w, u, sb) < level {
            continue;
        }
        let idx = i32::try_from(i).unwrap_or(0);
        for spell in crate::spellbook::list_spells(w, list, idx).iter() {
            if !crate::spellbook::knows_at(w, u, sb, level, *spell) {
                crate::spellbook::add_known(w, u, sb, level, *spell, true);
            }
        }
    }
}

fn learn_spell_list(w: &mut World, c: CompRef, h: Hook) {
    if h == Hook::TurnOn {
        learn_spell_list_apply(w, c);
    }
}

fn learn_spell_list_event(w: &mut World, c: CompRef, _iface: &str) {
    learn_spell_list_apply(w, c);
}

/// The resource of an `AddAbilityResources`: its own fact's blueprint, or `Resource`.
fn resource_of(w: &World, c: CompRef) -> Option<Guid> {
    let (use_this, resource) =
        with_settings(w, c, |v| (v.bool("UseThisAsResource"), v.guid("m_Resource")))?;
    if use_this { w.fact(c.fact).blueprint } else { resource }
}

/// `AddAbilityResources.OnActivate` / `OnDeactivate`: not while the feature is reapplying.
fn add_ability_resources(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some(res) = resource_of(w, c) else { return };
    if w.fact(c.fact).reapplying && w.fact(c.fact).kind == crate::world::FactKind::Feature {
        return;
    }
    match h {
        Hook::Activate => {
            let restore = with_settings(w, c, |v| v.bool("RestoreAmount")).unwrap_or(false);
            crate::resources::add(w, u, res, restore);
        }
        Hook::Deactivate => crate::resources::remove(w, u, res),
        _ => {}
    }
}

/// `AddAbilityResources.HandleUnitReapplyFeaturesOnLevelUp`.
fn add_ability_resources_event(w: &mut World, c: CompRef, _iface: &str) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some(res) = resource_of(w, c) else { return };
    if with_settings(w, c, |v| v.bool("RestoreOnLevelUp")).unwrap_or(false) {
        crate::resources::restore(w, u, res);
    }
}

/// `CalculateMaxResourceAmount(resource, ref bonus)` of the three bonus components.
fn resource_bonus_event(w: &mut World, c: CompRef, _iface: &str) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some(&(resource, _)) = w.bus.resource_bonus.last() else { return };
    let Some(ty) = w.component(c).ty() else { return };
    let name = wr_content::schema::ty(ty).short;
    let active = w.fact(c.fact).is_active;
    let rank = super::fact_rank(w, c.fact);
    let delta = match name {
        "IncreaseResourceAmount" => {
            let Some((r, value)) = with_settings(w, c, |v| (v.guid("m_Resource"), v.i32("Value")))
            else {
                return;
            };
            if active && r == Some(resource) { crate::csharp::arith::mul(value, rank) } else { 0 }
        }
        "IncreaseResourceAmountBySharedValue" => {
            let Some((r, decrease)) =
                with_settings(w, c, |v| (v.guid("m_Resource"), v.bool("Decrease")))
            else {
                return;
            };
            if active && r == Some(resource) {
                let cv = with_settings(w, c, |v| v.obj("Value").map(|o| o.0.clone())).flatten();
                let value = super::context_value(w, c, cv.as_ref().map(View));
                if decrease { crate::csharp::arith::neg(value) } else { value }
            } else {
                0
            }
        }
        "IncreaseResourcesByClass" => {
            let Some((r, class, archetype, stat, base)) = with_settings(w, c, |v| {
                (
                    v.guid("m_Resource"),
                    v.guid("m_CharacterClass"),
                    v.guid("m_Archetype"),
                    v.i32("Stat"),
                    v.i32("BaseValue"),
                )
            }) else {
                return;
            };
            if r != Some(resource) {
                return;
            }
            let t = crate::stats::types::StatType(stat);
            let attr =
                matches!(w.stats(u).v(t).kind, crate::stats::value::StatKind::Attribute { .. });
            let mut bonus = if attr { crate::stats::ops::attribute_bonus(w, u, t) } else { 0 };
            bonus = crate::csharp::arith::add(base, bonus);
            if let Some(cl) = class {
                let level = w
                    .unit(u)
                    .and_then(|x| x.progression.classes.iter().find(|c| c.class == cl))
                    .map_or(0, |c| c.level);
                bonus = crate::csharp::arith::add(bonus, level);
            }
            if let Some(a) = archetype {
                let hit = w.unit(u).and_then(|x| {
                    x.progression
                        .classes
                        .iter()
                        .find(|c| Some(c.class) != class && c.archetypes.contains(&a))
                        .map(|c| c.level)
                });
                if let Some(level) = hit {
                    bonus = crate::csharp::arith::add(bonus, level);
                }
            }
            bonus
        }
        _ => 0,
    };
    if let Some(last) = w.bus.resource_bonus.last_mut() {
        last.1 = crate::csharp::arith::add(last.1, delta);
    }
}

/// `UnitPartActivatableAbility`'s group count: `EnumUtils.GetMaxValue<ActivatableAbilityGroup>()`.
fn group_count() -> usize {
    wr_content::schema::enums()
        .iter()
        .find(|e| e.name.ends_with("ActivatableAbilityGroup"))
        .and_then(|e| e.members.iter().map(|(_, v)| *v).max())
        .and_then(|m| usize::try_from(m).ok())
        .unwrap_or(0)
}

fn ensure_part(w: &mut World, u: EntityIdx, part: &'static str) {
    if !w.entity(u).parts.iter().any(|p| p.clr == part) {
        w.entity_mut(u).parts.push(crate::world::Part { clr: part });
    }
}

/// `IncreaseActivatableAbilityGroupSize.OnTurnOn` / `OnTurnOff`.
fn increase_group_size(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some(group) = with_settings(w, c, |v| v.int("Group")) else { return };
    let delta = match h {
        Hook::TurnOn => 1,
        Hook::TurnOff => -1,
        _ => return,
    };
    // `Owner.Ensure<UnitPartActivatableAbility>()` on both.
    ensure_part(w, u, "UnitPartActivatableAbility");
    let n = group_count();
    let unit = w.unit_mut(u).expect("unit");
    if unit.group_size_increases.is_empty() {
        unit.group_size_increases = vec![0; n];
    }
    match usize::try_from(group).ok().and_then(|g| unit.group_size_increases.get_mut(g)) {
        Some(x) => *x += delta,
        None => w.coverage.unsupported("UnitPartActivatableAbility: group index out of range"),
    }
}

/// `AutoMetamagic.OnTurnOn` / `OnTurnOff`: the unit's special-metamagic part keeps the fact.
fn auto_metamagic(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::TurnOn => {
            ensure_part(w, u, "UnitPartSpecialMetamagic");
            w.unit_mut(u).expect("unit").special_metamagic.push(c.fact);
        }
        Hook::TurnOff => {
            if !w.entity(u).parts.iter().any(|p| p.clr == "UnitPartSpecialMetamagic") {
                return;
            }
            let unit = w.unit_mut(u).expect("unit");
            unit.special_metamagic.retain(|f| *f != c.fact);
            if unit.special_metamagic.is_empty() {
                w.entity_mut(u).parts.retain(|p| p.clr != "UnitPartSpecialMetamagic");
            }
        }
        _ => {}
    }
}
