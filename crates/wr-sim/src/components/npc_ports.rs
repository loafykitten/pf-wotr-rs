//! Components met on the NPCs of Prologue_Neathholm (decompiled, not copied):
//! `AddBuffActions` (`Kingmaker.UnitLogic.Buffs.Components/`, an `AddFactContextActions`),
//! `AddUndetectableAlignment`, `AddUnitFactEquipment` and `AddUnitFeatureEquipment`
//! (`Kingmaker.Designers.Mechanics.EquipmentEnchants/`), `ArmorCheckPenaltyIncrease`,
//! `MaxDexBonusIncrease`, `ArmorSpeedPenaltyRemoval`, `WeaponTrainingBonuses`
//! (`Kingmaker.Designers.Mechanics.Facts|Buffs/`), `WeaponTraining`
//! (`Kingmaker.UnitLogic.FactLogic/`), `UnitPartWeaponTraining` (`Kingmaker.UnitLogic.Parts/`),
//! `AddVendorItems` (`Kingmaker.UnitLogic.FactLogic/`) and
//! `RestrictionsHelper.CheckHasArmor/CheckHasShield`
//! (`Kingmaker.UnitLogic.ActivatableAbilities.Restrictions/`).
//!
//! Each port implements exactly the hooks the C# type overrides, except where noted: the
//! item-enchantment components only their post-load fix (their activation isn't reached), so
//! a use of the others is reported by the dispatcher.

use super::mechanics::RulePort;
use super::{Port, all_hooks, bits, context_value, fact_rank, owner_unit, settings, with_settings};
use crate::armor::{RuleCalculateArmorCheckPenalty, RuleCalculateArmorMaxDexBonusLimit};
use crate::content::View;
use crate::registry::{Hook, Registry};
use crate::rulebook::RuleEvent;
use crate::stats::ops;
use crate::stats::types::{ModifierDescriptor as D, StatType};
use crate::stats::value::ChangedHandler;
use crate::world::{CompRef, EntityIdx, FactIdx, World};

/// `MechanicsFeatureType.ImmunityToArmorSpeedPenalty` (`UnitMechanicFeatures
/// .ImmuneToArmorSpeedPenalty`) and `ImmunityToMediumArmorSpeedPenalty`.
const IMMUNITY_TO_ARMOR_SPEED_PENALTY: i64 = 14;
const IMMUNITY_TO_MEDIUM_ARMOR_SPEED_PENALTY: i64 = 30;

fn no_hooks(_: &mut World, _: CompRef, _: Hook) {}

pub fn register(r: &mut Registry) {
    // `AddBuffActions : AddFactContextActions`, no overrides of its own.
    r.register(Box::new(Port::new(
        "AddBuffActions",
        bits(&[Hook::Activate, Hook::Deactivate, Hook::Dispose]),
        super::mechanics::add_fact_context_actions,
    )));
    // `AddUndetectableAlignment` is a runtime provider: the runtime's base hooks do nothing.
    r.register(Box::new(Port::new("AddUndetectableAlignment", all_hooks(), undetectable)));
    r.register(Box::new(Port::new(
        "AddUnitFactEquipment",
        bits(&[Hook::Activate, Hook::Deactivate, Hook::ApplyPostLoadFixes]),
        add_unit_fact_equipment,
    )));
    r.register(Box::new(Port::new(
        "AddUnitFeatureEquipment",
        bits(&[Hook::ApplyPostLoadFixes]),
        |w, c, h| enchantment_post_load_fix(w, c, h, "AppliedFact", false),
    )));
    r.register(Box::new(RulePort {
        name: "ArmorCheckPenaltyIncrease",
        bits: bits(&[Hook::TurnOn]),
        hooks: armor_check_penalty_hooks,
        handlers: &["IInitiatorRulebookHandler<RuleCalculateArmorCheckPenalty>"],
        rule: armor_check_penalty_rule,
    }));
    r.register(Box::new(RulePort {
        name: "MaxDexBonusIncrease",
        bits: bits(&[Hook::TurnOn, Hook::TurnOff]),
        hooks: max_dex_hooks,
        handlers: &["IInitiatorRulebookHandler<RuleCalculateArmorMaxDexBonusLimit>"],
        rule: max_dex_rule,
    }));
    r.register(Box::new(Port::new(
        "ArmorSpeedPenaltyRemoval",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        armor_speed_penalty_removal,
    )));
    r.register(Box::new(Port::new(
        "WeaponTraining",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        weapon_training,
    )));
    let mut p = Port::new(
        "WeaponTrainingBonuses",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        weapon_training_bonuses_hooks,
    );
    p.handlers =
        &["IUnitActiveEquipmentSetHandler", "IUnitEquipmentHandler", "IUnitPartUpdatedHandler"];
    p.ev = Some(weapon_training_bonuses_event);
    r.register(Box::new(p));
    let mut p = Port::new("AddVendorItems", 0, no_hooks);
    p.handlers = &["IUnitSpawnHandler"];
    p.ev = Some(add_vendor_items_event);
    r.register(Box::new(p));
}

// ---- AddUndetectableAlignment ---------------------------------------------------------

/// `Runtime.OnActivate`/`OnDeactivate`: `Owner.Descriptor.Alignment.Undetectable.Retain()`/
/// `Release()` on the unit's own (saved) `UnitAlignment.Undetectable`, a `CountableFlag`
/// (`Kingmaker.Utility/CountableFlag.cs`: a `short` count; `Release` logs below one and floors
/// at zero).
fn undetectable(w: &mut World, c: CompRef, h: Hook) {
    if !matches!(h, Hook::Activate | Hook::Deactivate) {
        return;
    }
    let Some(u) = owner_unit(w, c) else { return };
    let Some(a) = w.unit_mut(u).and_then(|x| x.alignment.as_mut()) else {
        w.coverage.unsupported("AddUndetectableAlignment: unit without a UnitAlignment");
        return;
    };
    // The saved count is a `short`.
    #[allow(clippy::cast_possible_truncation)]
    let count = a.undetectable as i16;
    if h == Hook::Activate {
        a.undetectable = i32::from(count.wrapping_add(1));
        return;
    }
    // `(short)Math.Max(0, m_Count - 1)`: the subtraction is in `int`.
    a.undetectable = (i32::from(count) - 1).max(0);
    if count < 1 {
        w.log("error", "Can't release countable flag: no one retain it");
    }
}

// ---- item enchantment components ------------------------------------------------------

/// `AddUnitFactEquipment`: `OnActivate` adds the blueprint's fact to the wielder with the
/// enchantment's context as its parent (`Owner.Wielder.AddFact(Blueprint, Context)`) and the
/// item as its source; `OnDeactivate` removes it (`Wielder.RemoveFact(AddedFact)`); the post-load
/// fix is [`enchantment_post_load_fix`]'s.
fn add_unit_fact_equipment(w: &mut World, c: CompRef, h: Hook) {
    match h {
        Hook::ApplyPostLoadFixes => enchantment_post_load_fix(w, c, h, "AddedFact", true),
        Hook::Activate => {
            let Some(item) = w.fact(c.fact).owner.filter(|o| w.item(*o).is_some()) else { return };
            let Some(wielder) = w.item(item).and_then(|i| i.wielder) else {
                // `Owner.Wielder.AddFact`: a null wielder throws.
                w.coverage.unsupported("AddUnitFactEquipment.OnActivate: no wielder");
                return;
            };
            let Some(g) = with_settings(w, c, |v| v.guid("m_Blueprint")).flatten() else {
                // `AddFact(null)` returns null; `null.SetSourceItem` throws.
                w.coverage.unsupported("AddUnitFactEquipment.OnActivate: no blueprint");
                return;
            };
            let Some(bp) = w.content.get(g) else {
                w.coverage.unsupported("AddUnitFactEquipment.OnActivate: blueprint missing");
                return;
            };
            if crate::unit_facts::kind_for(&bp).0 != crate::world::FactKind::Feature {
                // A buff takes `AddBuff(blueprint, parentContext)`; other classes reject a parent
                // context in the sim's `CreateFact`.
                w.coverage
                    .unsupported("AddUnitFactEquipment.OnActivate: a fact that isn't a feature");
                return;
            }
            let parent = w.fact(c.fact).context.clone();
            let f = crate::unit_facts::create_fact(w, &bp, wielder, parent.as_ref());
            let Some(f) = crate::unit_facts::add(w, wielder, f) else {
                w.coverage.unsupported("AddUnitFactEquipment.OnActivate: the fact wasn't added");
                return;
            };
            w.component_mut(c).fact_refs.insert("AddedFact".to_owned(), vec![Some(f)]);
            w.fact_mut(f).source_item = Some(item);
        }
        Hook::Deactivate => {
            let added = w
                .component(c)
                .fact_refs
                .get("AddedFact")
                .and_then(|v| v.first().copied().flatten());
            let wielder = w.fact(c.fact).owner.and_then(|o| w.item(o)).and_then(|i| i.wielder);
            match (wielder, added) {
                (Some(u), Some(f)) => crate::unit_facts::remove(w, u, f),
                // `RemoveFact(null)` does nothing.
                (Some(_), None) => {}
                (None, _) => {
                    w.coverage.unsupported("AddUnitFactEquipment.OnDeactivate: no wielder");
                    return;
                }
            }
            w.component_mut(c).fact_refs.insert("AddedFact".to_owned(), vec![None]);
        }
        _ => {}
    }
}

/// `AddUnitFactEquipment.OnApplyPostLoadFixes` (an inactive enchantment forgets its fact)
/// and `AddUnitFeatureEquipment.OnApplyPostLoadFixes`: the applied fact's source item is the
/// enchanted item again.
fn enchantment_post_load_fix(
    w: &mut World,
    c: CompRef,
    h: Hook,
    member: &str,
    forget_inactive: bool,
) {
    if h != Hook::ApplyPostLoadFixes {
        return;
    }
    if forget_inactive && !w.fact(c.fact).is_active {
        w.component_mut(c).fact_refs.insert(member.to_owned(), vec![None]);
    }
    let applied = w.component(c).fact_refs.get(member).and_then(|v| v.first().copied().flatten());
    let Some(f) = applied else { return };
    let Some(item) = w.fact(c.fact).owner.filter(|o| w.item(*o).is_some()) else { return };
    // `Ability.SetSourceItem` also updates the ability's metamagic from the item.
    if w.fact(f).kind == crate::world::FactKind::Ability {
        w.coverage.unsupported("Ability.SetSourceItem: UpdateMetamagicFromItem");
    }
    w.fact_mut(f).source_item = Some(item);
}

// ---- armor components -----------------------------------------------------------------

/// `RestrictionsHelper.CheckHasArmor`: the armor slot holds a real armor (`Type.IsArmor`).
fn armor_of(w: &World, u: EntityIdx) -> Option<EntityIdx> {
    let it = crate::items::slot_item(w, u, "Armor")?;
    if w.entity(it).clr != "ItemEntityArmor" {
        return None;
    }
    let bp = w.item(it)?.blueprint.and_then(|g| w.content.get(g))?;
    let ty = w.content.get(bp.view().guid("m_Type")?)?;
    ty.view().bool("m_IsArmor").then_some(it)
}

/// `RestrictionsHelper.CheckHasShield`: the current set's secondary hand holds a shield and
/// the hands are enabled; the shield's armor component.
fn shield_armor_of(w: &World, u: EntityIdx) -> Option<EntityIdx> {
    let unit = w.unit(u)?;
    if unit.body.hands_disabled {
        return None;
    }
    let name = format!("HandsEquipmentSets[{}].SecondaryHand", unit.body.current_hand_set);
    let it = crate::items::slot_item(w, u, &name)?;
    (w.entity(it).clr == "ItemEntityShield").then(|| w.item(it)?.armor_component).flatten()
}

fn recalc_armor_and_shield(w: &mut World, u: EntityIdx) {
    if let Some(a) = armor_of(w, u) {
        crate::armor::recalculate_stats(w, a);
    }
    if let Some(s) = shield_armor_of(w, u) {
        crate::armor::recalculate_stats(w, s);
    }
}

fn armor_check_penalty_hooks(w: &mut World, c: CompRef, h: Hook) {
    if h != Hook::TurnOn {
        return;
    }
    if let Some(u) = owner_unit(w, c) {
        recalc_armor_and_shield(w, u);
    }
}

/// `ArmorCheckPenaltyIncrease.OnEventAboutToTrigger`.
fn armor_check_penalty_rule(w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
    if !about {
        return;
    }
    let Some(r) = rule.as_any().downcast_mut::<RuleCalculateArmorCheckPenalty>() else { return };
    let Some((bp, i)) = settings(w, c) else { return };
    let v = View(bp.components_array()[i]);
    let (check_category, category, per_rank) =
        (v.bool("CheckCategory"), v.int("Category"), v.i32("BonesPerRank"));
    let armor = r.armor;
    let is_shield = w
        .item(armor)
        .and_then(|it| it.blueprint)
        .and_then(|g| w.content.get(g))
        .and_then(|b| b.view().guid("m_Type"))
        .and_then(|g| w.content.get(g))
        .is_some_and(|t| t.view().bool("m_IsShield"));
    if (!is_shield && !check_category)
        || (check_category && crate::armor::item_armor_type(w, armor) == category)
    {
        let bonus = context_value(w, c, v.obj("Bonus"));
        let rank = fact_rank(w, c.fact);
        if let Some(r) = rule.as_any().downcast_mut::<RuleCalculateArmorCheckPenalty>() {
            r.bonus = crate::csharp::arith::add(
                r.bonus,
                crate::csharp::arith::add(bonus, per_rank.wrapping_mul(rank)),
            );
        }
    }
}

/// The six attributes `MaxDexBonusIncrease` watches, in its subscription order.
const WATCHED: [StatType; 6] = [
    StatType::Charisma,
    StatType::Constitution,
    StatType::Dexterity,
    StatType::Strength,
    StatType::Wisdom,
    StatType::Intelligence,
];

fn handler_of(c: CompRef) -> ChangedHandler {
    ChangedHandler::MaxDexBonus(c.fact.0, u32::try_from(c.comp).unwrap_or(u32::MAX))
}

fn max_dex_hooks(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::TurnOn => {
            for s in WATCHED {
                ops::subscribe_changed_handler(w, u, s, handler_of(c));
            }
            max_dex_stat_changed(w, c);
        }
        Hook::TurnOff => {
            for s in WATCHED {
                ops::unsubscribe_changed_handler(w, u, s, handler_of(c));
            }
        }
        _ => {}
    }
}

/// `MaxDexBonusIncrease.StatChangeHandler()` (an attribute changed, or the turn-on).
pub fn max_dex_stat_changed(w: &mut World, c: CompRef) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((check, category)) =
        with_settings(w, c, |v| (v.bool("CheckCategory"), v.int("Category")))
    else {
        return;
    };
    if check {
        if let Some(a) = armor_of(w, u)
            && crate::armor::item_armor_type(w, a) == category
        {
            crate::armor::recalculate_stats(w, a);
        }
        if let Some(s) = shield_armor_of(w, u)
            && crate::armor::item_armor_type(w, s) == category
        {
            crate::armor::recalculate_stats(w, s);
        }
    } else {
        recalc_armor_and_shield(w, u);
    }
}

/// `MaxDexBonusIncrease.OnEventAboutToTrigger`.
fn max_dex_rule(w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
    if !about {
        return;
    }
    let Some(r) = rule.as_any().downcast_mut::<RuleCalculateArmorMaxDexBonusLimit>() else {
        return;
    };
    let armor = r.armor;
    let Some((bp, i)) = settings(w, c) else { return };
    let v = View(bp.components_array()[i]);
    if v.bool("CheckCategory") && crate::armor::item_armor_type(w, armor) != v.int("Category") {
        return;
    }
    let mut num = v.i32("Bonus");
    if v.bool("UseContextInstead") {
        num = context_value(w, c, v.obj("ContextBonus"));
    } else if v.bool("ReplaceContextIfGreater") {
        let n2 = context_value(w, c, v.obj("ContextBonus"));
        if n2 > num {
            num = n2;
        }
    }
    let total =
        crate::csharp::arith::add(num, v.i32("BonesPerRank").wrapping_mul(fact_rank(w, c.fact)));
    if let Some(r) = rule.as_any().downcast_mut::<RuleCalculateArmorMaxDexBonusLimit>() {
        r.bonus = crate::csharp::arith::add(r.bonus, total);
    }
}

fn armor_speed_penalty_removal(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let delta = match h {
        Hook::TurnOn => 1,
        Hook::TurnOff => -1,
        _ => return,
    };
    let rank = fact_rank(w, c.fact);
    let mut flags = vec![IMMUNITY_TO_MEDIUM_ARMOR_SPEED_PENALTY];
    if rank >= 2 {
        flags.push(IMMUNITY_TO_ARMOR_SPEED_PENALTY);
    }
    for f in flags {
        let e = w.unit_mut(u).expect("unit").mechanics_features.entry(f).or_insert(0);
        // `CountableFlag.Retain` / `Release` (release doesn't go below zero).
        *e = (*e + delta).max(0);
    }
    if let Some(a) = armor_of(w, u) {
        crate::armor::recalculate_stats(w, a);
    }
}

// ---- weapon training ------------------------------------------------------------------

/// `UnitPartWeaponTraining.GetWeaponRank(weapon)`.
fn weapon_rank(w: &World, u: EntityIdx, weapon: Option<EntityIdx>) -> i32 {
    let Some(weapon) = weapon else { return 0 };
    let Some(wp) = crate::combat::weapon::Weapon::of(w, weapon) else { return 0 };
    // `weapon.Blueprint.FighterGroup` is the type's `m_FighterGroupFlags` (an `int` enum).
    #[allow(clippy::cast_possible_truncation)]
    let groups = wp.ty.view().int("m_FighterGroupFlags") as i32;
    let two_handed = wp.is_two_handed();
    let mut num = 0;
    for &f in w.unit(u).map_or(&[][..], |x| x.weapon_trainings.as_slice()) {
        for comp in &w.fact(f).components {
            let Some(o) = comp.bp_component() else { continue };
            let v = View(o);
            if crate::content::is_a(o.ty, "WeaponGroupAttackBonus") {
                // `WeaponFighterGroupHelper.Contains`: `flags & ToFlags(group)`, where
                // `ToFlags` is 0 for `None` and `1 << group` (an `int` shift: the count is
                // taken mod 32).
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let g = v.int("WeaponGroup") as i32;
                let flag = if g == 0 { 0 } else { 1i32.wrapping_shl(g as u32) };
                if groups & flag != 0 {
                    num = num.max(fact_rank(w, f));
                }
            }
            if crate::content::is_a(o.ty, "WeaponParametersAttackBonus")
                && v.bool("OnlyTwoHanded")
                && two_handed
            {
                num = num.max(fact_rank(w, f));
            }
        }
    }
    num
}

/// `UnitPartWeaponTraining.GetMaxWeaponRank()`: the ranks of `Body.PrimaryHand` and
/// `SecondaryHand` (`HandSlot.MaybeWeapon`, through the one port of it,
/// [`crate::combat::weapon::hand_weapon`]: the empty-hand weapon, with double weapons and
/// shield bashes reported) and of each additional limb (`WeaponSlot.MaybeWeapon`: the empty-hand
/// weapon when the slot is empty or inactive, else the item if it is a weapon).
fn max_weapon_rank(w: &mut World, u: EntityIdx) -> i32 {
    use crate::combat::weapon::{Hand, Weapon, hand_weapon};
    let primary = hand_weapon(w, u, Hand::Primary);
    let secondary = hand_weapon(w, u, Hand::Secondary);
    let mut num = weapon_rank(w, u, primary).max(weapon_rank(w, u, secondary));
    let limbs: Vec<Option<EntityIdx>> = w
        .unit(u)
        .map(|x| {
            x.body
                .limbs
                .iter()
                .map(|s| match s.item {
                    Some(i) if s.active => Weapon::of(w, i).map(|_| i),
                    _ => x.body.empty_hand,
                })
                .collect()
        })
        .unwrap_or_default();
    for l in limbs {
        num = num.max(weapon_rank(w, u, l));
    }
    num
}

/// `WeaponTraining.OnTurnOn`/`OnTurnOff`: the unit's `UnitPartWeaponTraining` (an
/// `OldStyleUnitPart` list of the training facts) gains or loses the fact and raises
/// `IUnitPartUpdatedHandler` for the unit.
fn weapon_training(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::TurnOn => {
            if !w.entity(u).parts.iter().any(|p| p.clr == "UnitPartWeaponTraining") {
                w.entity_mut(u).parts.push(crate::world::Part { clr: "UnitPartWeaponTraining" });
            }
            w.unit_mut(u).expect("unit").weapon_trainings.push(c.fact);
        }
        Hook::TurnOff => {
            if !w.entity(u).parts.iter().any(|p| p.clr == "UnitPartWeaponTraining") {
                return;
            }
            let list = &mut w.unit_mut(u).expect("unit").weapon_trainings;
            if let Some(p) = list.iter().position(|f| *f == c.fact) {
                list.remove(p);
            }
        }
        _ => return,
    }
    crate::rulebook::bus::raise_unit_fact(
        w,
        u,
        "IUnitPartUpdatedHandler",
        "HandleUnitPartUpdated",
        None,
        None,
    );
}

/// `WeaponTrainingBonuses.UpdateModifier`/`RemoveModifier`.
fn update_modifier(w: &mut World, c: CompRef) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((stat, desc)) =
        with_settings(w, c, |v| (StatType(v.i32("Stat")), D(v.i32("Descriptor"))))
    else {
        return;
    };
    ops::remove_modifiers_from(w, u, stat, c);
    let num = if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartWeaponTraining") {
        max_weapon_rank(w, u)
    } else {
        0
    };
    if w.stats(u).get(stat).is_some() && num > 0 {
        ops::add_modifier_unique(w, u, stat, num, c, desc);
    }
}

fn weapon_training_bonuses_hooks(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::TurnOn => update_modifier(w, c),
        Hook::TurnOff => {
            if let Some(stat) = with_settings(w, c, |v| StatType(v.i32("Stat"))) {
                ops::remove_modifiers_from(w, u, stat, c);
            }
        }
        _ => {}
    }
}

fn weapon_training_bonuses_event(w: &mut World, c: CompRef, iface: &str) {
    let Some(owner) = owner_unit(w, c) else { return };
    let (method, unit) = w.bus.call.last().copied().unwrap_or(("", None));
    match iface {
        // `HandleUnitChangeActiveEquipmentSet(unit)`.
        "IUnitActiveEquipmentSetHandler" if unit == Some(owner) => update_modifier(w, c),
        // `HandleEquipmentSlotUpdated(slot, previousItem)`: the slot's owner.
        "IUnitEquipmentHandler" if method == "HandleEquipmentSlotUpdated" => {
            if w.bus.slot.last().is_some_and(|(o, _)| *o == owner) {
                update_modifier(w, c);
            }
        }
        // `HandleUnitPartUpdated(part)`: only `UnitPartWeaponTraining` raises it here.
        "IUnitPartUpdatedHandler" => update_modifier(w, c),
        "IUnitActiveEquipmentSetHandler" => {}
        _ => w.coverage.unsupported(format!("WeaponTrainingBonuses: {iface}.{method}")),
    }
}

// ---- AddVendorItems -------------------------------------------------------------------

/// `AddVendorItems.HandleUnitSpawned`: acts only for its own unit; stocking a vendor
/// (`UnitPartVendor`, generated loot) isn't ported and is reported.
fn add_vendor_items_event(w: &mut World, c: CompRef, _iface: &str) {
    let (_, unit) = w.bus.call.last().copied().unwrap_or(("", None));
    if unit.is_none() || unit != owner_unit(w, c) {
        return;
    }
    if with_settings(w, c, |v| v.guid("m_Loot")).flatten().is_some() {
        w.coverage.unsupported("AddVendorItems.HandleUnitSpawned: vendor loot");
    }
}

/// `FactIdx` of a training, for tests.
#[must_use]
pub fn trainings(w: &World, u: EntityIdx) -> Vec<FactIdx> {
    w.unit(u).map(|x| x.weapon_trainings.clone()).unwrap_or_default()
}
