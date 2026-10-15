//! Buffs that enchant their owner's weapons (decompiled, not copied):
//! `BuffEnchantSpecificWeaponWorn` and `BuffEnchantAnyWeapon`
//! (`Kingmaker.UnitLogic.FactLogic/BuffEnchantSpecificWeaponWorn.cs`, `BuffEnchantAnyWeapon.cs`,
//! their `…Data.cs`).
//!
//! Each adds an enchantment to a weapon with the buff's context as its parent
//! (`ItemEntity.AddEnchantment(enchantment, Context)`) and keeps the enchantments it added in its
//! runtime data (saved), in the component's `fact_refs`: `Enchantment` and `SecondEnchantment`,
//! or `Enchantment` and `m_EnchantmentsList`. Turning on first removes the ones it holds.

use super::{Port, bits, owner_unit, with_settings};
use crate::items::{hand_has_item, hand_has_weapon, hand_maybe_weapon, limb_slots};
use crate::registry::{Hook, Registry};
use crate::world::{CompRef, EntityIdx, FactIdx, World};

pub fn register(r: &mut Registry) {
    r.register(Box::new(Port::new(
        "BuffEnchantSpecificWeaponWorn",
        bits(&[Hook::TurnOn, Hook::Deactivate]),
        specific_weapon_worn,
    )));
    let mut any = Port::new(
        "BuffEnchantAnyWeapon",
        bits(&[Hook::TurnOn, Hook::Activate, Hook::Deactivate]),
        any_weapon,
    );
    any.handlers = &["IUnitActiveEquipmentSetHandler", "IUnitEquipmentHandler"];
    any.ev = Some(any_weapon_event);
    r.register(Box::new(any));
}

fn held(w: &World, c: CompRef, key: &str) -> Vec<Option<FactIdx>> {
    w.component(c).fact_refs.get(key).cloned().unwrap_or_default()
}

fn set_held(w: &mut World, c: CompRef, key: &str, v: Vec<Option<FactIdx>>) {
    w.component_mut(c).fact_refs.insert(key.to_owned(), v);
}

/// `enchantment?.Owner?.RemoveEnchantment(enchantment)`.
fn remove(w: &mut World, f: Option<FactIdx>) {
    let Some(f) = f else { return };
    if let Some(item) = w.fact(f).owner.filter(|o| w.item(*o).is_some()) {
        crate::items::remove_enchantment(w, item, f);
    }
}

/// `item.AddEnchantment(Enchantment, Context)`: the component's enchantment blueprint with the
/// buff's context as the parent (a missing blueprint or context is reported).
fn add(w: &mut World, c: CompRef, item: EntityIdx, name: &'static str) -> Option<FactIdx> {
    let Some(bp) = with_settings(w, c, |v| v.guid("m_EnchantmentBlueprint"))
        .flatten()
        .and_then(|g| w.content.get(g))
    else {
        w.coverage.unsupported(format!("{name}: enchantment blueprint missing"));
        return None;
    };
    let Some(ctx) = w.fact(c.fact).context.clone() else {
        w.coverage.unsupported(format!("{name}: the buff has no context"));
        return None;
    };
    crate::items::add_enchantment_with_parent(w, item, &bp, ctx)
}

fn item_blueprint(w: &World, item: EntityIdx) -> Option<wr_content::Guid> {
    w.item(item).and_then(|i| i.blueprint)
}

/// `BuffEnchantSpecificWeaponWorn`: on turn-on (after clearing) the current primary and secondary
/// hands' weapons (`MaybeWeapon`, the secondary one if it is another weapon) of the component's
/// weapon blueprint each get the enchantment; deactivation clears. `ClearEnchantment` removes
/// both enchantments but forgets only the first.
fn specific_weapon_worn(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::TurnOn => {
            clear_specific(w, c);
            let weapon_bp = with_settings(w, c, |v| v.guid("m_WeaponBlueprint")).flatten();
            let primary = hand_maybe_weapon(w, u, true);
            let secondary = hand_maybe_weapon(w, u, false);
            if let Some(p) = primary
                && weapon_bp.is_some()
                && item_blueprint(w, p) == weapon_bp
            {
                let e = add(w, c, p, "BuffEnchantSpecificWeaponWorn");
                set_held(w, c, "Enchantment", vec![e]);
            }
            if let Some(s) = secondary
                && secondary != primary
                && weapon_bp.is_some()
                && item_blueprint(w, s) == weapon_bp
            {
                let e = add(w, c, s, "BuffEnchantSpecificWeaponWorn");
                set_held(w, c, "SecondEnchantment", vec![e]);
            }
        }
        Hook::Deactivate => clear_specific(w, c),
        _ => {}
    }
}

fn clear_specific(w: &mut World, c: CompRef) {
    let first = held(w, c, "Enchantment").first().copied().flatten();
    let second = held(w, c, "SecondEnchantment").first().copied().flatten();
    remove(w, first);
    remove(w, second);
    set_held(w, c, "Enchantment", vec![None]);
}

/// `EquipSlotBase.SlotType`.
const SLOT_PRIMARY_HAND: i64 = 0;
const SLOT_SECONDARY_HAND: i64 = 1;
const SLOT_ADDITIONAL_LIMB: i64 = 19;

/// `EquipSlotBase.ExtractSlot(type, body)`: the slot's name in [`crate::unit::Body`] (`None`
/// for `AdditionalLimb`, which throws there).
fn extract_slot(w: &World, u: EntityIdx, ty: i64) -> Option<String> {
    let set = w.unit(u).map_or(0, |x| x.body.current_hand_set);
    let named = |n: &str| Some(n.to_owned());
    match ty {
        SLOT_PRIMARY_HAND => Some(format!("HandsEquipmentSets[{set}].PrimaryHand")),
        SLOT_SECONDARY_HAND => Some(format!("HandsEquipmentSets[{set}].SecondaryHand")),
        2 => named("Armor"),
        3 => named("Belt"),
        4 => named("Head"),
        5 => named("Feet"),
        6 => named("Gloves"),
        7 => named("Neck"),
        8 => named("Ring1"),
        9 => named("Ring2"),
        10 => named("Wrist"),
        11 => named("Shoulders"),
        12..=16 => Some(format!("QuickSlot{}", ty - 12)),
        17 => named("Glasses"),
        18 => named("Shirt"),
        _ => None,
    }
}

/// `BuffEnchantAnyWeapon`: on turn-on and activation (after `RemoveAndClear`) the enchantment
/// goes on the weapon of its slot: the primary hand's weapon or the empty-hand weapon, the
/// secondary hand's weapon (or its shield's weapon component), or every additional limb's
/// weapon (the empty-hand weapon without limbs), each unless it already has the enchantment
/// (`StackAddItem`). Deactivation removes them.
fn any_weapon(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::TurnOn | Hook::Activate => any_weapon_on_activate(w, c, u),
        Hook::Deactivate => remove_and_clear(w, c),
        _ => {}
    }
}

fn any_weapon_slot(w: &World, c: CompRef) -> i64 {
    with_settings(w, c, |v| v.int("Slot")).unwrap_or(-1)
}

fn any_weapon_on_activate(w: &mut World, c: CompRef, u: EntityIdx) {
    remove_and_clear(w, c);
    let empty_hand = w.unit(u).and_then(|x| x.body.empty_hand);
    match any_weapon_slot(w, c) {
        SLOT_PRIMARY_HAND => {
            // `PrimaryHand.HasWeapon ? PrimaryHand.MaybeWeapon : Body.EmptyHandWeapon`.
            let item = if hand_has_weapon(w, u, true) {
                hand_maybe_weapon(w, u, true)
            } else {
                empty_hand
            };
            if let Some(i) = item {
                stack_add_item(w, c, i);
            }
        }
        SLOT_SECONDARY_HAND => {
            // `EquipSlotBase.ExtractSlot(SecondaryHand, body)`: the slot's item, a shield's
            // weapon component, or nothing.
            if !hand_has_item(w, u, false) {
                return;
            }
            let Some(item) = crate::items::current_hand_item(w, u, false) else { return };
            let item = match w.entity(item).clr {
                "ItemEntityShield" => match w.item(item).and_then(|i| i.weapon_component) {
                    Some(wc) => wc,
                    None => return,
                },
                "ItemEntityWeapon" => item,
                _ => return,
            };
            stack_add_item(w, c, item);
        }
        SLOT_ADDITIONAL_LIMB => {
            let limbs = limb_slots(w, u);
            if !limbs.is_empty() {
                // `additionalLimbs[i].MaybeWeapon`.
                for i in limbs.into_iter().filter_map(|l| l.1) {
                    stack_add_item(w, c, i);
                }
            } else if let Some(e) = empty_hand {
                stack_add_item(w, c, e);
            }
        }
        _ => {}
    }
}

/// `StackAddItem(item)`: unless the item has no enchantment collection (yet), or its
/// enchantments have one of the blueprint, the new enchantment is kept (`Data.AddEnchantment`: the first in `Enchantment`,
/// the others in `m_EnchantmentsList`).
fn stack_add_item(w: &mut World, c: CompRef, item: EntityIdx) {
    if !crate::items::has_enchantments_collection(w, item) {
        return;
    }
    let bp = with_settings(w, c, |v| v.guid("m_EnchantmentBlueprint")).flatten();
    let has = w.entity(item).facts.list.iter().any(|f| {
        w.fact(*f).kind == crate::world::FactKind::ItemEnchantment && w.fact(*f).blueprint == bp
    });
    if has {
        return;
    }
    let Some(e) = add(w, c, item, "BuffEnchantAnyWeapon") else { return };
    if held(w, c, "Enchantment").first().copied().flatten().is_none() {
        set_held(w, c, "Enchantment", vec![Some(e)]);
    } else {
        let mut list = held(w, c, "m_EnchantmentsList");
        list.push(Some(e));
        set_held(w, c, "m_EnchantmentsList", list);
    }
}

/// `Data.RemoveAndClear()`: every enchantment held is removed from its item.
fn remove_and_clear(w: &mut World, c: CompRef) {
    let mut all: Vec<Option<FactIdx>> =
        held(w, c, "Enchantment").first().copied().into_iter().collect();
    all.extend(held(w, c, "m_EnchantmentsList"));
    for e in all {
        remove(w, e);
    }
    set_held(w, c, "Enchantment", vec![None]);
    set_held(w, c, "m_EnchantmentsList", Vec::new());
}

/// `HandleEquipmentSlotUpdated(slot, previous)` for the owner's slot: with the limb slot type,
/// every limb holding an item enchants its weapon (`limb?.MaybeItem?.Blueprint` and the
/// enchantment compare as booleans); else, when the slot is the component's, the enchantments
/// held are removed and the hand's weapon (or the empty-hand weapon) is enchanted again.
/// `HandleUnitChangeActiveEquipmentSet` isn't raised by the sim (the active set never changes):
/// reported for the owner.
fn any_weapon_event(w: &mut World, c: CompRef, iface: &str) {
    let Some(u) = owner_unit(w, c) else { return };
    if iface != "IUnitEquipmentHandler" {
        if w.bus.call.last().and_then(|x| x.1).is_none_or(|x| x == u) {
            w.coverage.unsupported(format!("BuffEnchantAnyWeapon: {iface}"));
        }
        return;
    }
    let Some((owner, slot)) = w.bus.slot.last().cloned() else {
        w.coverage.unsupported("BuffEnchantAnyWeapon: IUnitEquipmentHandler without a slot");
        return;
    };
    if owner != u {
        return;
    }
    let ty = any_weapon_slot(w, c);
    if ty == SLOT_ADDITIONAL_LIMB {
        let has_bp = with_settings(w, c, |v| v.guid("m_EnchantmentBlueprint")).flatten().is_some();
        for (with_item, weapon) in limb_slots(w, u) {
            if with_item == has_bp
                && let Some(wp) = weapon
            {
                stack_add_item(w, c, wp);
            }
        }
        return;
    }
    if extract_slot(w, u, ty).as_deref() != Some(slot.as_str()) {
        return;
    }
    remove_and_clear(w, c);
    let empty_hand = w.unit(u).and_then(|x| x.body.empty_hand);
    let primary = match ty {
        SLOT_PRIMARY_HAND => Some(true),
        SLOT_SECONDARY_HAND => Some(false),
        _ => None,
    };
    // `Hand.HasWeapon ? Hand.MaybeWeapon : Body.EmptyHandWeapon`.
    if let Some(p) = primary {
        let item = if hand_has_weapon(w, u, p) { hand_maybe_weapon(w, u, p) } else { empty_hand };
        if let Some(i) = item {
            stack_add_item(w, c, i);
        }
    }
}
