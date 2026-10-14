//! Components that follow a unit's equipment (decompiled, not copied): `ShieldFocus`,
//! `ArmorFocus`, `MonkNoArmorFeatureUnlock`, `MonkNoArmorAndMonkWeaponFeatureUnlock`
//! (`Kingmaker.Designers.Mechanics.Buffs/*.cs`) and `DeflectArrows`
//! (`Kingmaker.UnitLogic.FactLogic/DeflectArrows.cs`). Each reacts to
//! `IUnitEquipmentHandler.HandleEquipmentSlotUpdated(slot, previousItem)` (the slot's owner
//! and activity are in [`crate::rulebook::bus::EventBus::slot`]) and to
//! `IUnitActiveEquipmentSetHandler.HandleUnitChangeActiveEquipmentSet(unit)`.

use super::{Port, bits, owner_unit};
use crate::combat::weapon::Weapon;
use crate::content::View;
use crate::registry::{Hook, Registry};
use crate::stats::ops;
use crate::stats::types::{ModifierDescriptor, StatType};
use crate::world::{CompRef, EntityIdx, FactIdx, World};

const EQUIPMENT: &str = "IUnitEquipmentHandler";
const EQUIPMENT_SET: &str = "IUnitActiveEquipmentSetHandler";

pub fn register(r: &mut Registry) {
    r.register(Box::new(Port {
        name: "ShieldFocus",
        bits: bits(&[Hook::TurnOn, Hook::TurnOff]),
        f: shield_focus,
        handlers: &[EQUIPMENT_SET, EQUIPMENT],
        ev: Some(shield_focus_event),
    }));
    r.register(Box::new(Port {
        name: "ArmorFocus",
        bits: bits(&[Hook::TurnOn, Hook::TurnOff]),
        f: armor_focus,
        handlers: &[EQUIPMENT_SET, EQUIPMENT],
        ev: Some(armor_focus_event),
    }));
    r.register(Box::new(Port {
        name: "DeflectArrows",
        bits: bits(&[Hook::TurnOn, Hook::TurnOff]),
        f: deflect_arrows,
        handlers: &[EQUIPMENT, EQUIPMENT_SET],
        ev: Some(deflect_arrows_event),
    }));
    r.register(Box::new(Port {
        name: "MonkNoArmorFeatureUnlock",
        bits: bits(&[Hook::Activate, Hook::Deactivate]),
        f: monk_no_armor,
        handlers: &[
            EQUIPMENT_SET,
            EQUIPMENT,
            "IPolymorphActivatedHandler",
            "IPolymorphDeactivatedHandler",
        ],
        ev: Some(monk_no_armor_event),
    }));
    r.register(Box::new(Port {
        name: "MonkNoArmorAndMonkWeaponFeatureUnlock",
        bits: bits(&[Hook::Activate, Hook::Deactivate]),
        f: monk_weapon,
        handlers: &[EQUIPMENT_SET, EQUIPMENT, "IUnitBuffHandler", "IUnitEmptyHandWeaponHandler"],
        ev: Some(monk_weapon_event),
    }));
}

/// The unit argument of the event being raised (`slot.Owner`, `unit`).
fn event_unit(w: &World) -> Option<EntityIdx> {
    w.bus.call.last().and_then(|c| c.1)
}

/// `UnitBody.SecondaryHand.HasShield` of the current hands set.
fn secondary_has_shield(w: &World, u: EntityIdx) -> bool {
    crate::items::current_hand_item(w, u, false)
        .is_some_and(|i| w.entity(i).clr == "ItemEntityShield")
}

/// `UnitBody.Armor.HasArmor && Armor.Armor.Blueprint.IsArmor` (`RestrictionsHelper.CheckHasArmor`
/// without polymorph).
fn has_armor(w: &World, u: EntityIdx) -> Option<EntityIdx> {
    let it = crate::items::slot_item(w, u, "Armor")?;
    let bp = w.item(it)?.blueprint.and_then(|g| w.content.get(g))?;
    let ty = bp.view().guid("m_Type").and_then(|g| w.content.get(g))?;
    // `BlueprintArmorType.IsArmor` (default true).
    if ty.view().bool("m_IsArmor") { Some(it) } else { None }
}

// ---- ShieldFocus --------------------------------------------------------------------

fn shield_focus(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::TurnOn => check_shield(w, c, u),
        Hook::TurnOff => ops::remove_modifiers_from(w, u, StatType::AC, c),
        _ => {}
    }
}

fn check_shield(w: &mut World, c: CompRef, u: EntityIdx) {
    if secondary_has_shield(w, u) {
        ops::add_modifier_unique(w, u, StatType::AC, 1, c, ModifierDescriptor::Focus);
    } else {
        ops::remove_modifiers_from(w, u, StatType::AC, c);
    }
}

fn shield_focus_event(w: &mut World, c: CompRef, iface: &str) {
    let Some(u) = owner_unit(w, c) else { return };
    match iface {
        // `if (Owner == unit.Unit)`.
        EQUIPMENT_SET if event_unit(w) == Some(u) => check_shield(w, c, u),
        // `if (!(slot.Owner != Owner))`.
        EQUIPMENT if event_unit(w) == Some(u) => check_shield(w, c, u),
        _ => {}
    }
}

// ---- ArmorFocus ---------------------------------------------------------------------

fn armor_focus(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::TurnOn => check_armor(w, c, u),
        Hook::TurnOff => ops::remove_modifiers_from(w, u, StatType::AC, c),
        _ => {}
    }
}

fn check_armor(w: &mut World, c: CompRef, u: EntityIdx) {
    let category = super::with_settings(w, c, |v| v.int("ArmorCategory")).unwrap_or(0);
    // `RestrictionsHelper.CheckArmor(Owner, ArmorCategory)`: `Armor.ArmorType() == category`.
    let ok = has_armor(w, u).is_some_and(|a| crate::armor::item_armor_type(w, a) == category);
    if ok {
        ops::add_modifier_unique(w, u, StatType::AC, 1, c, ModifierDescriptor::ArmorFocus);
    } else {
        ops::remove_modifiers_from(w, u, StatType::AC, c);
    }
}

fn armor_focus_event(w: &mut World, c: CompRef, iface: &str) {
    let Some(u) = owner_unit(w, c) else { return };
    match iface {
        // `HandleUnitChangeActiveEquipmentSet(unit)` checks whoever's set changed.
        EQUIPMENT_SET => check_armor(w, c, u),
        EQUIPMENT if event_unit(w) == Some(u) => check_armor(w, c, u),
        _ => {}
    }
}

// ---- DeflectArrows ------------------------------------------------------------------

fn feature_enabled(w: &World, c: CompRef) -> bool {
    w.component(c).data.as_ref().is_some_and(|d| d["FeatureEnabled"] == true)
}

fn set_feature_enabled(w: &mut World, c: CompRef, v: bool) {
    let mut d = w.component(c).data.clone().unwrap_or_else(|| serde_json::json!({}));
    d["FeatureEnabled"] = serde_json::Value::Bool(v);
    w.component_mut(c).data = Some(d);
}

/// `State.Features.DeflectArrows.Retain()`/`Release()` (`MechanicsFeatureType` by name).
fn retain_deflect_arrows(w: &mut World, u: EntityIdx, delta: i32) {
    let key = wr_content::schema::enums()
        .iter()
        .find(|e| e.name.ends_with("MechanicsFeatureType"))
        .and_then(|e| e.members.iter().find(|(n, _)| *n == "DeflectArrows").map(|(_, v)| *v));
    let Some(key) = key else { return };
    let e = w.unit_mut(u).expect("unit").mechanics_features.entry(key).or_insert(0);
    *e = (*e + delta).max(0);
}

fn deflect_arrows(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::TurnOn => update_deflect(w, c, u),
        Hook::TurnOff if feature_enabled(w, c) => {
            retain_deflect_arrows(w, u, -1);
            set_feature_enabled(w, c, false);
        }
        _ => {}
    }
}

fn update_deflect(w: &mut World, c: CompRef, u: EntityIdx) {
    let flag = deflect_restriction(w, c, u);
    let on = feature_enabled(w, c);
    if flag && !on {
        retain_deflect_arrows(w, u, 1);
        set_feature_enabled(w, c, true);
    } else if !flag && on {
        retain_deflect_arrows(w, u, -1);
        set_feature_enabled(w, c, false);
    }
}

/// `DeflectArrows.CheckRestriction()`.
fn deflect_restriction(w: &mut World, c: CompRef, u: EntityIdx) -> bool {
    let hands_enabled = w
        .unit(u)
        .and_then(|x| x.original_blueprint)
        .and_then(|g| w.content.get(g))
        .and_then(|b| b.view().obj("Body").map(|b| !b.bool("DisableHands")))
        .unwrap_or(true);
    if !hands_enabled {
        return false;
    }
    let (primary, secondary) =
        (crate::items::current_hand_item(w, u, true), crate::items::current_hand_item(w, u, false));
    match super::with_settings(w, c, |v| v.int("m_Restriction")).unwrap_or(0) {
        // `!Primary.HasItem || !Secondary.HasItem` → `GripType == OneHanded`.
        0 => {
            if primary.is_none() || secondary.is_none() {
                !crate::combat::weapon::current_grip_two_handed(w, u)
            } else {
                false
            }
        }
        1 => is_one_handed_melee_piercing(w, u, true) || is_one_handed_melee_piercing(w, u, false),
        // `Primary.HasShield ? !Secondary.HasShield : true`.
        _ => {
            if primary.is_some_and(|i| w.entity(i).clr == "ItemEntityShield") {
                !secondary.is_some_and(|i| w.entity(i).clr == "ItemEntityShield")
            } else {
                true
            }
        }
    }
}

/// `DeflectArrows.IsOneHandedMeleePiercing(hand)`.
fn is_one_handed_melee_piercing(w: &mut World, u: EntityIdx, primary: bool) -> bool {
    let hand = if primary {
        crate::combat::weapon::Hand::Primary
    } else {
        crate::combat::weapon::Hand::Secondary
    };
    let Some(wi) = crate::combat::weapon::hand_weapon(w, u, hand) else { return false };
    let Some(wp) = Weapon::of(w, wi) else { return false };
    let light = wp.ty.view().bool("m_IsLight");
    if !(wp.is_melee() && (!wp.is_two_handed() || light)) {
        return false;
    }
    let Some(dt) = wp.damage_type() else { return false };
    // `DamageType.Physical` is 0; `PhysicalDamageForm.Piercing` is 2.
    if dt.int("Type") != 0 {
        return false;
    }
    if dt.obj("Physical").map_or(0, |p| p.int("Form")) != 2 {
        // `WeaponCategory.DuelingSword` (49) with `Features.DuelingMastery`.
        return wp.category() == 49
            && crate::components::state::has_mechanics_feature(w, u, &["DuelingMastery"]);
    }
    true
}

fn deflect_arrows_event(w: &mut World, c: CompRef, iface: &str) {
    let Some(u) = owner_unit(w, c) else { return };
    match iface {
        // `slot.Owner == Owner && slot is HandSlot`.
        EQUIPMENT if event_unit(w) == Some(u) && event_slot_is_hand(w) => update_deflect(w, c, u),
        EQUIPMENT_SET if event_unit(w) == Some(u) => update_deflect(w, c, u),
        _ => {}
    }
}

fn event_slot_is_hand(w: &World) -> bool {
    w.bus.slot.last().is_some_and(|(_, n)| n.starts_with("HandsEquipmentSets["))
}

// ---- the monk unlocks ---------------------------------------------------------------

fn applied_fact(w: &World, c: CompRef) -> Option<FactIdx> {
    w.component(c).fact_refs.get("AppliedFact").and_then(|v| v.first().copied().flatten())
}

fn set_applied_fact(w: &mut World, c: CompRef, f: Option<FactIdx>) {
    w.component_mut(c).fact_refs.insert("AppliedFact".to_owned(), vec![f]);
}

/// `AddFact()`: `Owner.AddFact(NewFact)` unless one is applied.
fn monk_add_fact(w: &mut World, c: CompRef, u: EntityIdx) {
    if applied_fact(w, c).is_some() {
        return;
    }
    let Some(bp) = super::with_settings(w, c, |v| v.guid("m_NewFact"))
        .flatten()
        .and_then(|g| w.content.get(g))
    else {
        return;
    };
    let f = crate::unit_facts::add_fact(w, u, &bp);
    set_applied_fact(w, c, f);
}

/// `RemoveFact()`.
fn monk_remove_fact(w: &mut World, c: CompRef, u: EntityIdx) {
    if let Some(f) = applied_fact(w, c) {
        if w.fact(f).owner == Some(u) {
            crate::unit_facts::remove(w, u, f);
        }
        set_applied_fact(w, c, None);
    }
}

fn monk_no_armor(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::Activate => monk_no_armor_check(w, c, u),
        Hook::Deactivate => monk_remove_fact(w, c, u),
        _ => {}
    }
}

/// `MonkNoArmorFeatureUnlock.CheckEligibility()`: the fact is on without a shield and armor.
fn monk_no_armor_check(w: &mut World, c: CompRef, u: EntityIdx) {
    let has_shield = secondary_has_shield(w, u);
    let armor = has_armor(w, u).is_some();
    if !has_shield && !armor {
        monk_add_fact(w, c, u);
    } else {
        monk_remove_fact(w, c, u);
    }
}

fn monk_no_armor_event(w: &mut World, c: CompRef, iface: &str) {
    let Some(u) = owner_unit(w, c) else { return };
    // Every handler is `UpdateBuffStateIfCorrectUnit(unit)`.
    let _ = iface;
    if event_unit(w) == Some(u) {
        monk_no_armor_check(w, c, u);
    }
}

fn monk_weapon(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::Activate => monk_weapon_check(w, c, u),
        Hook::Deactivate => monk_remove_fact(w, c, u),
        _ => {}
    }
}

/// `MonkNoArmorAndMonkWeaponFeatureUnlock.CheckEligibility()`.
fn monk_weapon_check(w: &mut World, c: CompRef, u: EntityIdx) {
    let Some((zen, sohei)) =
        super::with_settings(w, c, |v: View<'_>| (v.bool("IsZenArcher"), v.bool("IsSohei")))
    else {
        return;
    };
    if zen || sohei {
        w.coverage.unsupported("MonkNoArmorAndMonkWeaponFeatureUnlock: zen archer or sohei");
        return;
    }
    // `PrimaryHand.Weapon.Blueprint.IsMonk` (`Weapon` throws without a weapon: the
    // empty-hand weapon answers when the hand is empty).
    let weapon = crate::combat::weapon::hand_weapon(w, u, crate::combat::weapon::Hand::Primary)
        .and_then(|i| Weapon::of(w, i));
    let monk = weapon.as_ref().is_some_and(|x| x.ty.view().bool("m_IsMonk"));
    if weapon.is_none() {
        w.coverage.unsupported("MonkNoArmorAndMonkWeaponFeatureUnlock: no primary weapon");
    }
    if !secondary_has_shield(w, u) && has_armor(w, u).is_none() && monk {
        monk_add_fact(w, c, u);
    } else {
        monk_remove_fact(w, c, u);
    }
}

fn monk_weapon_event(w: &mut World, c: CompRef, iface: &str) {
    let Some(u) = owner_unit(w, c) else { return };
    match iface {
        EQUIPMENT_SET => monk_weapon_check(w, c, u),
        // `!(slot.Owner != Owner) && slot.Active`.
        EQUIPMENT => {
            let active =
                w.bus.slot.last().is_some_and(|(su, n)| crate::items::slot_is_active(w, *su, n));
            if event_unit(w) == Some(u) && active {
                monk_weapon_check(w, c, u);
            }
        }
        // `buff.Blueprint == RapidShotBuff` for `HandleBuffDidAdded`/`HandleBuffDidRemoved`.
        "IUnitBuffHandler" => {
            let Some(Some(buff)) = w.bus.call_fact.last().copied() else { return };
            let rapid = super::with_settings(w, c, |v| v.guid("m_RapidshotBuff")).flatten();
            if rapid.is_some() && w.fact(buff).blueprint == rapid {
                monk_weapon_check(w, c, u);
            }
        }
        _ => monk_weapon_check(w, c, u),
    }
}
