//! Items created at run time and the unit body that holds them (decompiled, not copied):
//! `ItemsEntityFactory.CreateEntity` and the `ItemEntity` constructors
//! (`Kingmaker.Items/ItemsEntityFactory.cs`, `ItemEntity*.cs`), `new UnitBody`,
//! `UnitBody.Initialize`/`TryInsertItem` (`Kingmaker.Items/UnitBody.cs:325–590`), the slots
//! (`Kingmaker.Items.Slots/*.cs`), `ItemsCollection.Add`/`Insert`/`Extract`/`Remove`
//! (`Kingmaker.Items/ItemsCollection.cs`), `ItemEntity.OnDidEquipped`/`OnWillUnequip`
//! (`ItemEntity.cs:839–905`) and `UnitDescriptor.AddStartingInventory`
//! (`UnitDescriptor.cs:889`).
//!
//! A slot is named as in the save (`HandsEquipmentSets[i].PrimaryHand`, `Armor`,
//! `QuickSlot0`, `AdditionalLimbs[0]`…) and found in [`crate::unit::Body`]. A collection is
//! the identity `Item::collection` carries; every unit holding a collection keeps its own
//! copy of the list (the player's shared inventory is held by every party member), so list
//! changes go through [`collection_edit`]. Behaviour that isn't ported (slot locks, polymorph,
//! double weapons, item facts, unidentified items…) reports unsupported.

use std::rc::Rc;

use wr_content::Guid;

use crate::combat::weapon::{AttackType, Weapon};
use crate::content::{Blueprint, View, is_a};
use crate::csharp::arith::{add, sub};
use crate::facts::lifecycle as fl;
use crate::item::Item;
use crate::rulebook::bus::{self, Sub};
use crate::unit::{Body, Slot};
use crate::world::{Entity, EntityIdx, EntityKind, FactIdx, FactKind, FactsManager, World};

/// The named equipment slots of `UnitBody`, in `CollectAllSlots` order after the hand sets.
const NAMED_SLOTS: [&str; 12] = [
    "Armor",
    "Shirt",
    "Belt",
    "Head",
    "Glasses",
    "Feet",
    "Gloves",
    "Neck",
    "Ring1",
    "Ring2",
    "Wrist",
    "Shoulders",
];

// ---- bodies -------------------------------------------------------------------------

/// `new UnitBody(descriptor)` for the unit `e` (whose `OriginalBlueprint` is set): four hands
/// sets, the additional limbs' slots, the quick slots and the named slots; the empty-hand
/// weapon when hands are enabled.
pub fn new_body(w: &mut World, e: EntityIdx) {
    let Some(bp) = w.unit(e).and_then(|u| u.original_blueprint).and_then(|g| w.content.get(g))
    else {
        return;
    };
    let body_bp = bp.view().obj("Body");
    let hands = !body_bp.is_some_and(|b| b.bool("DisableHands"));
    let mut slots = Vec::new();
    for i in 0..4 {
        for hand in ["PrimaryHand", "SecondaryHand"] {
            slots.push(Slot {
                name: format!("HandsEquipmentSets[{i}].{hand}"),
                item: None,
                active: true,
            });
        }
    }
    for n in NAMED_SLOTS {
        slots.push(Slot { name: n.to_owned(), item: None, active: true });
    }
    let quick = body_bp.map_or(0, |b| b.seq("m_QuickSlots").len());
    for i in 0..quick {
        slots.push(Slot { name: format!("QuickSlot{i}"), item: None, active: true });
    }
    let limbs_n = body_bp.map_or(0, |b| {
        b.seq("m_AdditionalLimbs").len() + b.seq("m_AdditionalSecondaryLimbs").len()
    });
    let limbs = (0..limbs_n)
        .map(|i| Slot { name: format!("AdditionalLimbs[{i}]"), item: None, active: true })
        .collect();
    if let Some(u) = w.unit_mut(e) {
        u.body = Body {
            current_hand_set: 0,
            slots,
            empty_hand: None,
            limbs,
            grip_types: vec![0; 4],
            hands_disabled: !hands,
            ..Body::default()
        };
    }
    // `m_EmptyHandWeapon = body.EmptyHandWeapon.CreateEntity<ItemEntityWeapon>()`, then
    // `IsPermanentEmptyHand = true; OnDidEquipped(Owner.Unit)`.
    if hands
        && let Some(g) = body_bp.and_then(|b| b.guid("m_EmptyHandWeapon"))
        && let Some(wbp) = w.content.get(g)
        && let Some(item) = create_entity(w, &wbp)
    {
        on_did_equipped(w, item, e);
        if let Some(u) = w.unit_mut(e) {
            u.body.empty_hand = Some(item);
            // `m_EmptyHandWeapon.IsPermanentEmptyHand = true`.
            u.body.empty_hand_flags.push((item, true, false));
        }
    }
}

/// `UnitBody.Initialize()`: hand sets, limbs, quick slots and equipment from the unit
/// blueprint's `Body` (`TryInsertItem` does nothing under `DoNotCreateItems`).
pub fn body_initialize(w: &mut World, e: EntityIdx) {
    let Some(bp) = w.unit(e).and_then(|u| u.original_blueprint).and_then(|g| w.content.get(g))
    else {
        return;
    };
    let Some(body) = bp.view().obj("Body") else { return };
    let hands_enabled = !body.bool("DisableHands");
    // `InitializeHands`.
    if hands_enabled {
        let active = body.i32("ActiveHandSet");
        let current = if (0..4).contains(&active) { active } else { 0 };
        if let Some(u) = w.unit_mut(e) {
            u.body.current_hand_set = current;
        }
        for i in 0..4usize {
            let (main, off) = hand_equipment(body, i);
            let value = i32::try_from(i).is_ok_and(|i| i == current);
            for hand in ["PrimaryHand", "SecondaryHand"] {
                update_active(w, e, &format!("HandsEquipmentSets[{i}].{hand}"), value);
            }
            try_insert_item(w, e, main, &format!("HandsEquipmentSets[{i}].PrimaryHand"));
            try_insert_item(w, e, off, &format!("HandsEquipmentSets[{i}].SecondaryHand"));
        }
    }
    let limbs = seq_guids(body, "m_AdditionalLimbs");
    let secondary = seq_guids(body, "m_AdditionalSecondaryLimbs");
    for (i, g) in limbs.iter().enumerate() {
        let set0_primary_empty = slot_item(w, e, "HandsEquipmentSets[0].PrimaryHand").is_none();
        if !hands_enabled && set0_primary_empty {
            try_insert_item(w, e, *g, "HandsEquipmentSets[0].PrimaryHand");
        } else {
            try_insert_item(w, e, *g, &format!("AdditionalLimbs[{i}]"));
        }
    }
    for (n, g) in secondary.iter().enumerate() {
        let name = format!("AdditionalLimbs[{}]", limbs.len() + n);
        try_insert_item(w, e, *g, &name);
        if let Some(it) = slot_item(w, e, &name)
            && Weapon::of(w, it).is_some()
            && let Some(item) = w.item_mut(it)
        {
            item.force_secondary = true;
        }
    }
    for (j, g) in seq_guids(body, "m_QuickSlots").into_iter().enumerate() {
        try_insert_item(w, e, g, &format!("QuickSlot{j}"));
    }
    for (field, name) in [
        ("m_Armor", "Armor"),
        ("m_Shirt", "Shirt"),
        ("m_Belt", "Belt"),
        ("m_Head", "Head"),
        ("m_Glasses", "Glasses"),
        ("m_Feet", "Feet"),
        ("m_Gloves", "Gloves"),
        ("m_Neck", "Neck"),
        ("m_Ring1", "Ring1"),
        ("m_Ring2", "Ring2"),
        ("m_Wrist", "Wrist"),
        ("m_Shoulders", "Shoulders"),
    ] {
        try_insert_item(w, e, body.guid(field), name);
    }
}

/// A blueprint-reference array with its empty entries kept (`None`).
fn seq_guids(v: View<'_>, field: &str) -> Vec<Option<Guid>> {
    v.seq(field)
        .iter()
        .map(|n| match n {
            wr_content::Node::BpRef(g) if *g != Guid::EMPTY => Some(*g),
            _ => None,
        })
        .collect()
}

/// `BlueprintUnit.UnitBody.GetHandEqupment(set, main)`.
fn hand_equipment(body: View<'_>, set: usize) -> (Option<Guid>, Option<Guid>) {
    let (m, o) = match set {
        0 => ("m_PrimaryHand", "m_SecondaryHand"),
        1 => ("m_PrimaryHandAlternative1", "m_SecondaryHandAlternative1"),
        2 => ("m_PrimaryHandAlternative2", "m_SecondaryHandAlternative2"),
        _ => ("m_PrimaryHandAlternative3", "m_SecondaryHandAlternative3"),
    };
    (body.guid(m), body.guid(o))
}

/// `UnitBody.TryInsertItem(bpItem, slot)`.
fn try_insert_item(w: &mut World, u: EntityIdx, bp: Option<Guid>, slot: &str) {
    let Some(g) = bp else { return };
    if w.build.do_not_create_items {
        return;
    }
    let Some(bp) = w.content.get(g) else {
        w.coverage.unsupported("UnitBody.TryInsertItem: item blueprint missing");
        return;
    };
    let Some(item) = create_entity(w, &bp) else { return };
    if !can_insert_item(w, u, slot, item, false) {
        let owner = w.entity(u).unique_id.clone();
        w.log("error", format!("'{owner}' can't insert item '{}' to slot '{slot}'", bp.name));
        // (`Owner.Inventory.Add(itemEntity)` is outside the `SuppressEvents` scope.)
        inventory_add(w, u, item, false, false);
        return;
    }
    // `using (ContextData<ItemsCollection.SuppressEvents>.Request())`.
    insert_item(w, u, slot, item, true);
}

/// `UnitDescriptor.AddStartingInventory(unit, blueprint)`: the blueprint's starting items
/// into the inventory (nothing under `DoNotCreateItems`), events suppressed.
pub fn add_starting_inventory(w: &mut World, e: EntityIdx, bp: &Rc<Blueprint>) {
    if w.build.do_not_create_items {
        return;
    }
    for g in bp.view().guids("m_StartingInventory") {
        let Some(ib) = w.content.get(g) else {
            w.coverage.unsupported("UnitDescriptor.AddStartingInventory: item blueprint missing");
            continue;
        };
        // `unit.Inventory.Add(item)` → `Add(newBpItem)` → `Add(newBpItem.CreateEntity())`.
        if let Some(item) = create_entity(w, &ib) {
            inventory_add(w, e, item, false, true);
        }
    }
}

// ---- slots --------------------------------------------------------------------------

/// What a slot name denotes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SlotKind {
    /// `HandSlot` of hand set `set`.
    Hand {
        set: usize,
        primary: bool,
    },
    Armor,
    /// `EquipmentSlot<T>` taking blueprints of the type.
    Equipment(&'static str),
    Usable,
    /// `WeaponSlot` of the additional limbs.
    Limb,
}

fn slot_kind(name: &str) -> SlotKind {
    if let Some(rest) = name.strip_prefix("HandsEquipmentSets[")
        && let Some((i, hand)) = rest.split_once("].")
    {
        return SlotKind::Hand { set: i.parse().unwrap_or(0), primary: hand == "PrimaryHand" };
    }
    match name {
        "Armor" => SlotKind::Armor,
        "Shirt" => SlotKind::Equipment("BlueprintItemEquipmentShirt"),
        "Belt" => SlotKind::Equipment("BlueprintItemEquipmentBelt"),
        "Head" => SlotKind::Equipment("BlueprintItemEquipmentHead"),
        "Glasses" => SlotKind::Equipment("BlueprintItemEquipmentGlasses"),
        "Feet" => SlotKind::Equipment("BlueprintItemEquipmentFeet"),
        "Gloves" => SlotKind::Equipment("BlueprintItemEquipmentGloves"),
        "Neck" => SlotKind::Equipment("BlueprintItemEquipmentNeck"),
        "Ring1" | "Ring2" => SlotKind::Equipment("BlueprintItemEquipmentRing"),
        "Wrist" => SlotKind::Equipment("BlueprintItemEquipmentWrist"),
        "Shoulders" => SlotKind::Equipment("BlueprintItemEquipmentShoulders"),
        n if n.starts_with("QuickSlot") => SlotKind::Usable,
        _ => SlotKind::Limb,
    }
}

fn find_slot<'a>(w: &'a World, u: EntityIdx, name: &str) -> Option<&'a Slot> {
    let b = &w.unit(u)?.body;
    b.slots.iter().chain(b.limbs.iter()).find(|s| s.name == name)
}

fn find_slot_mut<'a>(w: &'a mut World, u: EntityIdx, name: &str) -> Option<&'a mut Slot> {
    let b = &mut w.unit_mut(u)?.body;
    b.slots.iter_mut().chain(b.limbs.iter_mut()).find(|s| s.name == name)
}

/// `ItemSlot.MaybeItem`: an item being removed from the slot isn't there any more.
#[must_use]
pub fn slot_item(w: &World, u: EntityIdx, name: &str) -> Option<EntityIdx> {
    if w.unit(u)?.body_pending {
        return None;
    }
    let it = find_slot(w, u, name)?.item?;
    if w.item(it).is_some_and(|i| i.remove_from_slot_in_progress) { None } else { Some(it) }
}

fn slot_active(w: &World, u: EntityIdx, name: &str) -> bool {
    find_slot(w, u, name).is_some_and(|s| s.active)
}

fn pair_name(name: &str) -> Option<String> {
    match slot_kind(name) {
        SlotKind::Hand { set, primary } => Some(format!(
            "HandsEquipmentSets[{set}].{}",
            if primary { "SecondaryHand" } else { "PrimaryHand" }
        )),
        _ => None,
    }
}

/// `HandSlot.UpdateActive(value)`: `Active = !Disabled & value` (the deactivate flag isn't
/// modelled); a change equips or unequips the slot's item.
fn update_active(w: &mut World, u: EntityIdx, name: &str, value: bool) {
    let Some(s) = find_slot_mut(w, u, name) else { return };
    if s.active == value {
        return;
    }
    s.active = value;
    // `OnActiveChanged`: `MaybeItem?.OnDidEquipped(Owner.Unit)` / `OnWillUnequip()`.
    if let Some(it) = slot_item(w, u, name) {
        if value {
            on_did_equipped(w, it, u);
        } else {
            on_will_unequip(w, it);
        }
    }
}

/// The owner's `State.Features.<name>` is on.
fn has_feature(w: &World, u: EntityIdx, name: &str) -> bool {
    crate::components::state::has_mechanics_feature(w, u, &[name])
}

/// `ItemEntityWeapon.CanTakeOneHand(unit)`.
fn can_take_one_hand(w: &World, wp: &Weapon, u: EntityIdx) -> bool {
    if wp.is_two_handed()
        && wp.attack_type() == AttackType::Melee
        && has_feature(w, u, "FighterOnHandedGrip")
    {
        return true;
    }
    wp.can_take_one_hand()
}

/// `WeaponSlot.MaybeWeapon` / `HandSlot.MaybeWeapon`.
fn maybe_weapon(w: &mut World, u: EntityIdx, name: &str) -> Option<EntityIdx> {
    let base = |w: &World| -> Option<EntityIdx> {
        match slot_item(w, u, name) {
            Some(it) if slot_active(w, u, name) => Weapon::of(w, it).map(|_| it),
            _ => w.unit(u).and_then(|x| x.body.empty_hand),
        }
    };
    let SlotKind::Hand { primary, .. } = slot_kind(name) else { return base(w) };
    let pair = pair_name(name).unwrap_or_default();
    let pair_weapon = if primary { None } else { maybe_weapon(w, u, &pair) };
    let pair_double = pair_weapon.and_then(|p| Weapon::of(w, p)).is_some_and(|x| x.is_double());
    if primary || pair_weapon.is_none() || !pair_double {
        if has_feature(w, u, "ShieldBash")
            && let Some(sh) = slot_item(w, u, name)
                .and_then(|i| w.item(i))
                .filter(|i| i.shield.is_none() && i.weapon_component.is_some())
        {
            return sh.weapon_component;
        }
        return base(w);
    }
    w.coverage.unsupported("HandSlot.MaybeWeapon: second part of a double weapon");
    base(w)
}

/// `HandSlot.MaybeShield`.
fn maybe_shield(w: &World, u: EntityIdx, name: &str) -> Option<EntityIdx> {
    slot_item(w, u, name).filter(|i| w.entity(*i).clr == "ItemEntityShield")
}

/// `HandSlot.IsItemTypeSupported(item)`.
fn hand_item_type_supported(w: &World, u: EntityIdx, name: &str, item: EntityIdx) -> bool {
    let SlotKind::Hand { primary, .. } = slot_kind(name) else { return false };
    let is_shield = w.entity(item).clr == "ItemEntityShield";
    let wp = Weapon::of(w, item);
    let first = !is_shield || primary;
    let second = match &wp {
        None => true,
        Some(x) => !can_take_one_hand(w, x, u) && !primary,
    };
    // `(first && second) ? item is BlueprintItemEquipmentHandSimple : true`, as decompiled.
    if first && second {
        return w
            .item(item)
            .and_then(|i| i.blueprint)
            .and_then(|g| w.content.get(g))
            .is_some_and(|b| is_a(b.obj.ty, "BlueprintItemEquipmentHandSimple"));
    }
    true
}

/// `ItemSlot.IsItemSupported(item)` of each slot class.
fn is_item_supported(w: &mut World, u: EntityIdx, name: &str, item: EntityIdx) -> bool {
    match slot_kind(name) {
        SlotKind::Limb => Weapon::of(w, item).is_some(),
        SlotKind::Usable => w.entity(item).clr == "ItemEntityUsable",
        SlotKind::Armor => {
            !crate::combat::is_in_combat(w, u) && w.entity(item).clr == "ItemEntityArmor"
        }
        SlotKind::Equipment(ty) => {
            !crate::combat::is_in_combat(w, u)
                && w.item(item)
                    .and_then(|i| i.blueprint)
                    .and_then(|g| w.content.get(g))
                    .is_some_and(|b| is_a(b.obj.ty, ty))
        }
        SlotKind::Hand { .. } => {
            if !hand_item_type_supported(w, u, name, item) {
                return false;
            }
            if slot_item(w, u, name).is_some() && !can_remove_item(w, u, name) {
                return false;
            }
            let pair = pair_name(name).unwrap_or_default();
            if slot_item(w, u, &pair).is_some() {
                let pw = maybe_weapon(w, u, &pair);
                let num = maybe_shield(w, u, &pair).is_some()
                    || pw
                        .and_then(|p| Weapon::of(w, p))
                        .is_some_and(|x| can_take_one_hand(w, &x, u));
                let flag = can_remove_item(w, u, &pair);
                let flag2 = num | flag;
                if maybe_shield(w, u, name).is_none() {
                    let mw = maybe_weapon(w, u, name);
                    if mw
                        .and_then(|p| Weapon::of(w, p))
                        .is_none_or(|x| !can_take_one_hand(w, &x, u))
                    {
                        return false;
                    }
                }
                if !flag2 {
                    return false;
                }
            }
            true
        }
    }
}

/// `ItemSlot.CanRemoveItem()` of each slot class (`Lock` and `IgnoreLock` aren't modelled).
fn can_remove_item(w: &mut World, u: EntityIdx, name: &str) -> bool {
    match slot_kind(name) {
        SlotKind::Armor | SlotKind::Equipment(_) if crate::combat::is_in_combat(w, u) => {
            return false;
        }
        SlotKind::Limb | SlotKind::Hand { .. } => {
            // `WeaponSlot.CanRemoveItem`: built-in limbs stay unless the owner is finally
            // dead; turn-based combat limits who may change weapons.
            if slot_kind(name) == SlotKind::Limb && !w.unit(u).is_some_and(|x| x.finally_dead) {
                return false;
            }
            if w.combat.turn_based && crate::combat::is_in_combat(w, u) {
                w.coverage.unsupported("WeaponSlot.CanRemoveItem: turn-based combat");
            }
        }
        _ => {}
    }
    !slot_item(w, u, name).and_then(|i| w.item(i)).is_some_and(|i| i.is_non_removable)
}

/// `ItemSlot.CanInsertItem(item, skipSlotAvailabilityCheck)`.
pub fn can_insert_item(
    w: &mut World,
    u: EntityIdx,
    name: &str,
    item: EntityIdx,
    skip: bool,
) -> bool {
    if !is_item_supported(w, u, name, item) || !can_be_equipped_by(w, item, u) {
        return false;
    }
    if skip {
        return true;
    }
    if slot_item(w, u, name).is_some() && !can_remove_item(w, u, name) {
        return false;
    }
    if let Some(wp) = Weapon::of(w, item) {
        let pair = pair_name(name);
        let flag = match &pair {
            Some(p) => slot_item(w, u, p).is_some() && !can_remove_item(w, u, p),
            None => false,
        };
        if flag && !can_take_one_hand(w, &wp, u) {
            return false;
        }
        if flag {
            let pw = maybe_weapon(w, u, pair.as_deref().unwrap_or_default());
            if pw.and_then(|p| Weapon::of(w, p)).is_none_or(|x| !can_take_one_hand(w, &x, u)) {
                return false;
            }
        }
    }
    true
}

/// `ItemEntity.CanBeEquippedBy(owner)`.
fn can_be_equipped_by(w: &mut World, item: EntityIdx, u: EntityIdx) -> bool {
    if !w.unit(u).is_some_and(|x| x.is_player_faction) {
        return true;
    }
    let Some(bp) = w.item(item).and_then(|i| i.blueprint).and_then(|g| w.content.get(g)) else {
        return false;
    };
    // `(Blueprint as BlueprintItemEquipment)?.CanBeEquippedBy(owner) ?? false`: every
    // `EquipmentRestriction` component.
    if !is_a(bp.obj.ty, "BlueprintItemEquipment") {
        return false;
    }
    for o in bp.components_array() {
        if !is_a(o.ty, "EquipmentRestriction") {
            continue;
        }
        let v = View(o);
        let ok = if v.is("EquipmentRestrictionClass") {
            let level = v
                .guid("m_Class")
                .and_then(|c| {
                    w.unit(u)?.progression.classes.iter().find(|x| x.class == c).map(|x| x.level)
                })
                .unwrap_or(0);
            if v.bool("Not") { level == 0 } else { level > 0 }
        } else if v.is("EquipmentRestrictionStat") {
            let s = w.stats(u).v(crate::stats::types::StatType(v.i32("Stat"))).permanent_value;
            s >= v.i32("MinValue")
        } else {
            w.coverage.unsupported(format!("ItemEntity.CanBeEquippedBy: {}", v.type_name()));
            true
        };
        if !ok {
            return false;
        }
    }
    // The subclass's `CanBeEquippedInternal`.
    let prof = |w: &World, armor: bool, k: i64| {
        let p = &w.unit(u).expect("unit").proficiencies;
        let l = if armor { &p.armor } else { &p.weapons };
        l.iter().any(|(x, n)| *x == k && *n > 0)
    };
    match w.entity(item).clr {
        "ItemEntityWeapon" => Weapon::of(w, item).is_some_and(|x| prof(w, false, x.category())),
        "ItemEntityArmor" => {
            let ty = bp.view().guid("m_Type").and_then(|g| w.content.get(g));
            let group = ty.as_ref().map_or(0, |t| t.view().int("m_ProficiencyGroup"));
            if prof(w, true, group) {
                return true;
            }
            let ignore_type = ty.as_ref().is_some_and(|t| {
                t.components_array().iter().any(|o| is_a(o.ty, "IgnoreArmorGroupComponent"))
            });
            if ignore_type {
                return true;
            }
            bp.components_array().iter().any(|o| is_a(o.ty, "IgnoreArmorGroupComponent"))
        }
        "ItemEntityShield" => {
            let group = bp
                .view()
                .guid("m_ArmorComponent")
                .and_then(|g| w.content.get(g))
                .and_then(|a| a.view().guid("m_Type"))
                .and_then(|g| w.content.get(g))
                .map_or(0, |t| t.view().int("m_ProficiencyGroup"));
            prof(w, true, group)
        }
        _ => true,
    }
}

/// `ItemSlot.InsertItem(item)`; false when nothing was inserted.
pub fn insert_item(
    w: &mut World,
    u: EntityIdx,
    name: &str,
    item: EntityIdx,
    suppress_events: bool,
) -> bool {
    if !can_insert_item(w, u, name, item, false) {
        w.log(
            "error",
            format!(
                "ItemSlot.InsertItem: can't insert item {} into slot {name}",
                w.entity(item).clr
            ),
        );
        return false;
    }
    let mut item = item;
    if w.item(item).and_then(|i| i.collection).is_none() {
        match inventory_add(w, u, item, false, suppress_events) {
            Some(i) => item = i,
            None => {
                w.log("warning", "Item dissipated during insertion.");
                return false;
            }
        }
    }
    if slot_item(w, u, name) == Some(item) && w.item(item).is_some_and(|i| i.count == 1) {
        return false;
    }
    let prev = slot_item(w, u, name);
    if prev.is_some() {
        remove_item_internal(w, u, name, false, true, suppress_events);
    }
    if w.item(item).is_some_and(|i| i.count > 1) {
        item = split(w, item, 1);
    }
    // `EntityService.Register(item)`.
    let id = w.entity(item).unique_id.clone();
    w.by_id.insert(id, item);
    w.unregistered_items.remove(&item);
    if let Some(s) = find_slot_mut(w, u, name) {
        s.item = Some(item);
    }
    if let Some(it) = w.item_mut(item) {
        it.holding_slot = Some((u, name.to_owned()));
    }
    if w.item(item).and_then(|i| i.wielder) == Some(u) {
        // The item is equipped elsewhere on this unit: `EquipmentSlots.First(s => s.HasItem
        // && s.Item == item)` loses it.
        let first = w.unit(u).and_then(|x| {
            x.body.slots.iter().find(|s| s.item == Some(item)).map(|s| s.name.clone())
        });
        if let Some(n) = first {
            if let Some(s) = find_slot_mut(w, u, &n) {
                s.item = None;
            }
            // `HandleEquipmentSlotUpdated(slot, pi)` for the slot that was vacated
            // (`ItemSlot.cs:274`), before the destination's own event below.
            raise_equipment_updated(w, u, &n);
        }
    } else if slot_active(w, u, name) {
        on_did_equipped(w, item, u);
    }
    // `Owner.Body.OnItemInserted` (weight cache), `OnItemInserted` (hand slots).
    on_slot_item_inserted(w, u, name);
    raise_equipment_updated(w, u, name);
    true
}

/// `EventBus.RaiseEvent<IUnitEquipmentHandler>(h => h.HandleEquipmentSlotUpdated(slot, item))`:
/// the slot (and so its owner) is visible to handlers in [`bus::EventBus::slot`].
fn raise_equipment_updated(w: &mut World, u: EntityIdx, name: &str) {
    w.bus.slot.push((u, name.to_owned()));
    bus::raise_global_call(w, "IUnitEquipmentHandler", "HandleEquipmentSlotUpdated", Some(u));
    w.bus.slot.pop();
}

/// The item in the current hands set's primary or secondary hand.
#[must_use]
pub fn current_hand_item(w: &World, u: EntityIdx, primary: bool) -> Option<EntityIdx> {
    let set = w.unit(u)?.body.current_hand_set;
    let hand = if primary { "PrimaryHand" } else { "SecondaryHand" };
    slot_item(w, u, &format!("HandsEquipmentSets[{set}].{hand}"))
}

/// `ItemSlot.Active`.
#[must_use]
pub fn slot_is_active(w: &World, u: EntityIdx, name: &str) -> bool {
    slot_active(w, u, name)
}

/// `HandSlot.OnItemInserted()`.
fn on_slot_item_inserted(w: &mut World, u: EntityIdx, name: &str) {
    let SlotKind::Hand { set, primary } = slot_kind(name) else { return };
    let primary_name = format!("HandsEquipmentSets[{set}].PrimaryHand");
    if !primary
        && let Some(p) = slot_item(w, u, &primary_name)
        && let Some(wp) = Weapon::of(w, p)
        && !can_take_one_hand(w, &wp, u)
    {
        remove_item(w, u, &primary_name, true);
    }
    if let Some(it) = slot_item(w, u, name)
        && let Some(wp) = Weapon::of(w, it)
        && !can_take_one_hand(w, &wp, u)
    {
        if primary {
            let pair = pair_name(name).unwrap_or_default();
            remove_item(w, u, &pair, true);
        } else {
            // `MoveWeaponToPairSlot()`.
            let mw = maybe_weapon(w, u, name);
            remove_item(w, u, name, true);
            if let Some(mw) = mw {
                let pair = pair_name(name).unwrap_or_default();
                insert_item(w, u, &pair, mw, false);
            }
        }
    }
    // `HandsEquipmentSet.AdjustGripAfterWeaponChange()`: a grip other than Auto isn't
    // modelled (a loaded one is reported by the weapon module).
    let grip = w.unit(u).and_then(|x| x.body.grip_types.get(set).copied()).unwrap_or(0);
    if grip != 0 {
        w.coverage.unsupported("HandsEquipmentSet.AdjustGripAfterWeaponChange: explicit grip");
    }
}

/// `ItemSlot.RemoveItem(autoMerge)`.
pub fn remove_item(w: &mut World, u: EntityIdx, name: &str, auto_merge: bool) -> bool {
    remove_item_internal(w, u, name, true, auto_merge, false)
}

fn remove_item_internal(
    w: &mut World,
    u: EntityIdx,
    name: &str,
    raise_update: bool,
    auto_merge: bool,
    _suppress_events: bool,
) -> bool {
    let Some(item) = slot_item(w, u, name) else { return false };
    if !can_remove_item(w, u, name) {
        return false;
    }
    if let Some(it) = w.item_mut(item) {
        it.remove_from_slot_in_progress = true;
    }
    if slot_active(w, u, name) {
        on_will_unequip(w, item);
    }
    if let Some(it) = w.item_mut(item) {
        it.holding_slot = None;
    }
    if let Some(s) = find_slot_mut(w, u, name) {
        s.item = None;
    }
    w.bus.slot.push((u, name.to_owned()));
    bus::raise_global_call(w, "IRemoveItemFromSlotHandler", "HandleItemRemovedFromSlot", Some(u));
    w.bus.slot.pop();
    if raise_update {
        raise_equipment_updated(w, u, name);
    }
    if auto_merge {
        try_merge_in_collection(w, item);
    }
    if let Some(it) = w.item_mut(item) {
        it.remove_from_slot_in_progress = false;
    }
    true
}

// ---- collections --------------------------------------------------------------------

/// Applies `f` to the item list of collection `col` held by every unit that holds it.
pub fn collection_edit(w: &mut World, col: usize, f: impl Fn(&mut Vec<EntityIdx>)) {
    for ent in &mut w.entities {
        if let EntityKind::Unit(u) = &mut ent.kind
            && u.inventory_collection == Some(col)
        {
            f(&mut u.inventory);
        }
    }
}

/// `ItemsCollection.Items`.
#[must_use]
pub fn collection_items(w: &World, col: usize) -> Vec<EntityIdx> {
    w.entities
        .iter()
        .find_map(|e| match &e.kind {
            EntityKind::Unit(u) if u.inventory_collection == Some(col) => Some(u.inventory.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// `ItemsCollection.IsPlayerInventory`: the shared inventory the main character holds.
pub(crate) fn is_player_inventory(w: &World, col: usize) -> bool {
    w.main_character.and_then(|m| w.unit(m)).and_then(|m| m.inventory_collection) == Some(col)
}

/// `IsMayBeStackable`/`IsStackable` of an item.
fn is_stackable(w: &World, item: EntityIdx) -> bool {
    let Some(it) = w.item(item) else { return false };
    if let Some(f) = it.force_stackable {
        return f;
    }
    let Some(bp) = it.blueprint.and_then(|g| w.content.get(g)) else { return false };
    let v = bp.view();
    let may = !v.bool("m_IsNotable") || v.bool("m_ForceStackable");
    // `m_HasExternalEnchantments` (false after a load, updated when an effect adds or removes
    // an enchantment); `HasOverridenVisual` is `VisualSourceItemBlueprint != null`
    // (`ItemEntity.cs:370`).
    may && !w.external_enchantments.contains(&item)
        && it.visual_source.is_none()
        && it.is_identified
}

/// `ItemEntity.CanBeMerged(other)`.
fn can_be_merged(w: &mut World, item: EntityIdx, other: EntityIdx) -> bool {
    let (Some(a), Some(b)) = (w.item(item), w.item(other)) else { return false };
    if !(is_stackable(w, item)
        && a.blueprint == b.blueprint
        && a.holding_slot.is_none()
        && a.charges == b.charges)
    {
        return false;
    }
    // `HasSameCraftParams`.
    let crafted =
        |w: &World, e: EntityIdx| w.entity(e).parts.iter().any(|p| p.clr == "CraftedItemPart");
    if crafted(w, item) || crafted(w, other) {
        w.coverage.unsupported("ItemEntity.CanBeMerged: CraftedItemPart");
    }
    true
}

/// `ItemEntity.TryMerge(other)`.
fn try_merge(w: &mut World, item: EntityIdx, other: EntityIdx) -> bool {
    let (c1, c2) =
        (w.item(item).and_then(|i| i.collection), w.item(other).and_then(|i| i.collection));
    if c2.is_some() && c2 != c1 {
        w.log("error", "Can't merge items from different collections");
        return false;
    }
    if !can_be_merged(w, item, other) {
        return false;
    }
    let n = w.item(other).map_or(0, |i| i.count);
    if let Some(i) = w.item_mut(item) {
        i.count = add(i.count, n);
    }
    if let Some(o) = w.item_mut(other) {
        o.count = sub(o.count, n).max(0);
    }
    if c2.is_some() {
        extract(w, other);
    }
    crate::entity::dispose(w, other);
    true
}

/// `ItemEntity.TryMergeInCollection()`.
fn try_merge_in_collection(w: &mut World, item: EntityIdx) -> EntityIdx {
    let Some(col) = w.item(item).and_then(|i| i.collection) else { return item };
    if !is_stackable(w, item) {
        return item;
    }
    for it in collection_items(w, col) {
        if it != item && try_merge(w, it, item) {
            return it;
        }
    }
    item
}

/// `ItemsCollection.TryMerge(item)`.
fn collection_try_merge(w: &mut World, col: usize, item: EntityIdx) -> EntityIdx {
    if !is_stackable(w, item) {
        return item;
    }
    let ic = w.item(item).and_then(|i| i.collection);
    if ic.is_some() && ic != Some(col) {
        w.log("error", "Can't merge items from different collection");
        return item;
    }
    for it in collection_items(w, col) {
        if item != it && try_merge(w, it, item) {
            return it;
        }
    }
    item
}

/// `ItemsCollection.Insert(item)`.
fn collection_insert(w: &mut World, col: usize, item: EntityIdx) {
    if let Some(c) = w.item(item).and_then(|i| i.collection) {
        w.log("error", "Item already in collection now");
        collection_remove(w, c, item);
    }
    if collection_items(w, col).contains(&item) {
        w.log("error", "Collection already contains item");
    } else {
        collection_edit(w, col, |v| v.push(item));
    }
    if let Some(i) = w.item_mut(item) {
        i.collection = Some(col);
        // `Insert`'s `UpdateSlotIndex()` isn't ported: the index is not established.
        i.inventory_slot_index = None;
    }
    // `!item.IsStackable && item.Count > 1 && !(IsMayBeStackable & isStackableIfNotIndefined)`
    // splits the extra count off; only items created with a count do.
    if !is_stackable(w, item) && w.item(item).is_some_and(|i| i.count > 1) {
        w.coverage.unsupported("ItemsCollection.Insert: splitting a non-stackable count");
    }
}

/// `ItemsCollection.Extract(item)`.
fn extract(w: &mut World, item: EntityIdx) {
    let Some(col) = w.item(item).and_then(|i| i.collection) else { return };
    collection_edit(w, col, |v| v.retain(|x| *x != item));
    if let Some(i) = w.item_mut(item) {
        i.collection = None;
        i.inventory_slot_index = None;
    }
    // Not under `DoNotRemoveFromSlot`: `item.HoldingSlot?.RemoveItem()`.
    if let Some((u, name)) = w.item(item).and_then(|i| i.holding_slot.clone()) {
        remove_item(w, u, &name, true);
    }
}

/// `ItemsCollection.Remove(item, count)` for the whole count.
pub fn collection_remove(w: &mut World, col: usize, item: EntityIdx) {
    let count = w.item(item).map_or(0, |i| i.count);
    let target = if !is_stackable(w, item) || w.item(item).is_some_and(|i| i.count == count) {
        item
    } else {
        split(w, item, count)
    };
    if w.item(target).is_some_and(|i| i.collection.is_some()) {
        extract(w, target);
    }
    on_items_removed(w, col, item, count, false);
}

/// `ItemEntity.IsLootable`: not `m_NotLootable`, and for a weapon not a natural one
/// (`ItemEntityWeapon.IsLootable`: `!Blueprint.IsNatural`, the weapon type's flag). `None`:
/// the weapon's type isn't available.
#[must_use]
pub fn is_lootable(w: &World, item: EntityIdx) -> Option<bool> {
    let it = w.item(item)?;
    if it.not_lootable {
        return Some(false);
    }
    if w.entity(item).clr == "ItemEntityWeapon" {
        return Weapon::of(w, item).map(|x| !x.is_natural());
    }
    Some(true)
}

/// `ItemEntity.UpdateSlotIndex(force)` (`ItemEntity.cs:1024–1101`): a non-lootable item's
/// index is -1; otherwise an index that is invalid (no collection, or the item sits in the
/// slot of a unit that is alive or of the player's faction) is reset, and a negative one
/// becomes the lowest index no item of the collection holds (or the collection's count).
/// Returns the index, `None` (and reports) where the collection's indices aren't established.
pub fn update_slot_index(w: &mut World, item: EntityIdx, force: bool) -> Option<i32> {
    if force && let Some(i) = w.item_mut(item) {
        i.inventory_slot_index = Some(-1);
    }
    let Some(lootable) = is_lootable(w, item) else {
        w.coverage.unsupported("ItemEntity.IsLootable: the weapon's type is not available");
        return None;
    };
    if !lootable {
        w.item_mut(item)?.inventory_slot_index = Some(-1);
        return Some(-1);
    }
    let it = w.item(item)?;
    // `HoldingSlot?.Owner != null && (!Owner.State.IsFinallyDead || Owner.IsPlayerFaction)`.
    let held_by_live_unit = it
        .holding_slot
        .as_ref()
        .is_some_and(|(u, _)| w.unit(*u).is_some_and(|x| !x.finally_dead || x.is_player_faction));
    let collection = it.collection;
    let Some(mut index) = it.inventory_slot_index else {
        w.coverage.unsupported(
            "ItemEntity.UpdateSlotIndex: slot index of an item added to the collection at run time is not established",
        );
        return None;
    };
    if index >= 0 && (collection.is_none() || held_by_live_unit) {
        index = -1;
    }
    if index < 0
        && let Some(col) = collection.filter(|_| !held_by_live_unit)
    {
        let items = collection_items(w, col);
        // The other items' indices (this item's own is negative here, so it counts as nothing).
        let others: Option<Vec<i32>> = items
            .iter()
            .map(|x| {
                if *x == item { Some(-1) } else { w.item(*x).and_then(|i| i.inventory_slot_index) }
            })
            .collect();
        let Some(others) = others else {
            w.coverage.unsupported(
                "ItemEntity.UpdateSlotIndex: slot index of an item added to the collection at run time is not established",
            );
            return None;
        };
        let max = others.iter().copied().fold(0, i32::max);
        let used: std::collections::HashSet<i32> =
            others.iter().copied().filter(|m| *m >= 0).collect();
        // The first free index below `max + 2`, else the collection's count.
        index = (0..add(max, 2))
            .find(|i| !used.contains(i))
            .unwrap_or_else(|| i32::try_from(items.len()).unwrap_or(i32::MAX));
    }
    w.item_mut(item)?.inventory_slot_index = Some(index);
    Some(index)
}

fn on_items_removed(w: &mut World, col: usize, item: EntityIdx, count: i32, suppress: bool) {
    if !suppress {
        w.bus.item.push((col, item));
        bus::raise_global_call(w, "IItemsCollectionHandler", "HandleItemsRemoved", None);
        // `GameLogEventItemsCollection.EventsHandler.HandleItemsRemoved`: a global subscriber of
        // the same raise.
        crate::game_log::items_collection(w, true, col, item, count);
        w.bus.item.pop();
    }
}

fn on_item_added(w: &mut World, col: usize, item: EntityIdx, count: i32, suppress: bool) {
    if !suppress {
        w.bus.item.push((col, item));
        bus::raise_global_call(w, "IItemsCollectionHandler", "HandleItemsAdded", None);
        crate::game_log::items_collection(w, false, col, item, count);
        w.bus.item.pop();
    }
    if is_player_inventory(w, col) {
        crate::rules::identify::try_identify_by_party(w, item);
    }
}

/// `unit.Inventory.Add(item)` (`ItemsCollection.Add(ItemEntity)`).
pub fn inventory_add(
    w: &mut World,
    u: EntityIdx,
    item: EntityIdx,
    stackable_if_not_indefined: bool,
    suppress_events: bool,
) -> Option<EntityIdx> {
    let col = w.unit(u).and_then(|x| x.inventory_collection)?;
    collection_add(w, col, item, stackable_if_not_indefined, suppress_events)
}

/// `unit.Inventory.Add(blueprint, count)` (`ItemsCollection.Add(BlueprintItem, int,
/// isStackableIfNotIndefined: false)`): one stack of a stackable item, else `count` items.
pub fn inventory_add_blueprint(w: &mut World, u: EntityIdx, bp: &Rc<Blueprint>, count: i32) {
    if count < 1 {
        return;
    }
    let Some(item) = create_entity(w, bp) else { return };
    if is_stackable(w, item) {
        if let Some(i) = w.item_mut(item) {
            i.count = add(i.count, sub(count, 1));
        }
        inventory_add(w, u, item, false, false);
        return;
    }
    inventory_add(w, u, item, false, false);
    for _ in 0..sub(count, 1) {
        if let Some(item) = create_entity(w, bp) {
            inventory_add(w, u, item, false, false);
        }
    }
}

/// `ItemsCollection.Add(newItem, isStackableIfNotIndefined)`.
pub fn collection_add(
    w: &mut World,
    col: usize,
    new_item: EntityIdx,
    _stackable_if_not_indefined: bool,
    suppress_events: bool,
) -> Option<EntityIdx> {
    // `SetOriginAreaIfNull`, `Time = GameTime`: not read by the rules.
    if is_player_inventory(w, col)
        && let Some(bp) = w.item(new_item).and_then(|i| i.blueprint).and_then(|g| w.content.get(g))
        && bp.components_array().iter().any(|o| {
            is_a(o.ty, "MoneyReplacement")
                || is_a(o.ty, "BuildPointsReplacement")
                || is_a(o.ty, "ConsumableEventBonusReplacement")
        })
    {
        return replacement_item(w, col, new_item, suppress_events);
    }
    let count = w.item(new_item).map_or(0, |i| i.count);
    let mut item = new_item;
    let flag = w.item(new_item).is_some_and(|i| i.holding_slot.is_none());
    if flag {
        item = collection_try_merge(w, col, item);
    }
    if item == new_item {
        collection_insert(w, col, item);
    }
    on_item_added(w, col, item, count, suppress_events);
    if item == new_item && flag {
        item = collection_try_merge(w, col, item);
    }
    Some(item)
}

/// `ItemsCollection.Add` of an item standing for money, build points or event bonus: the
/// player gains the amount, `OnItemAdded` runs, and the item is disposed.
fn replacement_item(
    w: &mut World,
    col: usize,
    item: EntityIdx,
    suppress: bool,
) -> Option<EntityIdx> {
    let bp = w.item(item).and_then(|i| i.blueprint).and_then(|g| w.content.get(g))?;
    let count = w.item(item).map_or(0, |i| i.count);
    let money = bp.components_array().into_iter().find(|o| is_a(o.ty, "MoneyReplacement"));
    if let Some(m) = money {
        // `GainMoney(component.Cost * newItem.Count)`.
        let cost = View(m).int("Cost");
        w.player.money = w.player.money.wrapping_add(cost.wrapping_mul(i64::from(count)));
    } else {
        w.coverage.unsupported("ItemsCollection.Add: BuildPointsReplacement or event bonus");
    }
    on_item_added(w, col, item, count, suppress);
    crate::entity::dispose(w, item);
    None
}

/// `AddItemToPlayer.RunAction()` (`AddItemToPlayer.cs`): the item (or `Quantity` of them) into
/// the player character's inventory; equipping isn't ported.
pub fn add_item_to_player(w: &mut World, a: View<'_>) -> crate::elements::Result<()> {
    let Some(bp) = a.guid("m_ItemToGive").and_then(|g| w.content.get(g)) else {
        return Err(crate::elements::ElementError::Other(
            "NullReferenceException: item to give".into(),
        ));
    };
    let Some(mc) = w.main_character else {
        return Err(crate::elements::ElementError::Other("no player character".into()));
    };
    let quantity = a.i32("Quantity");
    let (identify, equip) = (a.bool("Identify"), a.bool("Equip"));
    if equip {
        w.coverage.unsupported("AddItemToPlayer: Equip");
    }
    let give = |w: &mut World, item: EntityIdx| {
        if identify && let Some(i) = w.item_mut(item) {
            i.is_identified = true;
        }
        // `inventory.Add(item)`; a money item is gone afterwards.
        inventory_add(w, mc, item, false, false);
    };
    let Some(item) = create_entity(w, &bp) else { return Ok(()) };
    if is_stackable(w, item) {
        if quantity > 1
            && let Some(i) = w.item_mut(item)
        {
            i.count = add(i.count, sub(quantity, 1));
        }
        give(w, item);
        return Ok(());
    }
    give(w, item);
    for _ in 0..sub(quantity, 1).max(0) {
        if let Some(item) = create_entity(w, &bp) {
            give(w, item);
        }
    }
    Ok(())
}

/// `ItemEntity.Split(count)`.
fn split(w: &mut World, item: EntityIdx, count: i32) -> EntityIdx {
    let Some(it) = w.item(item).cloned() else { return item };
    if !is_stackable(w, item) {
        w.log("error", "Can't split item");
        return item;
    }
    if it.count < count {
        w.log("error", "Can't split item: required count more than the items count");
        return item;
    }
    if it.count == count {
        return item;
    }
    if let Some(i) = w.item_mut(item) {
        i.count = sub(i.count, count).max(0);
    }
    let Some(bp) = it.blueprint.and_then(|g| w.content.get(g)) else { return item };
    let Some(n) = create_entity(w, &bp) else { return item };
    if let Some(i) = w.item_mut(n) {
        i.charges = it.charges;
        i.is_identified = it.is_identified;
        i.count = add(i.count, sub(count, 1));
    }
    if w.entity(item).parts.iter().any(|p| p.clr == "CraftedItemPart") {
        w.coverage.unsupported("ItemEntity.Split: CraftedItemPart");
    }
    if let Some(col) = w.item(item).and_then(|i| i.collection) {
        collection_insert(w, col, n);
    }
    n
}

// ---- entities -----------------------------------------------------------------------

/// `ItemsEntityFactory.CreateEntity(bpItem)`.
pub fn create_entity(w: &mut World, bp: &Rc<Blueprint>) -> Option<EntityIdx> {
    if bp.components_array().iter().any(|o| is_a(o.ty, "ItemDlcRestriction")) {
        w.coverage.unsupported("ItemsEntityFactory.CreateEntity: ItemDlcRestriction");
    }
    let t = bp.obj.ty;
    Some(if is_a(t, "BlueprintItemWeapon") {
        new_weapon(w, bp, None)
    } else if is_a(t, "BlueprintItemArmor") {
        new_armor(w, bp, None)
    } else if is_a(t, "BlueprintItemShield") {
        new_shield(w, bp)
    } else if is_a(t, "BlueprintItemEquipmentUsable") {
        new_item_entity(w, bp, "ItemEntityUsable")
    } else {
        new_item_entity(w, bp, "ItemEntitySimple")
    })
}

/// `new ItemEntityWeapon(bpItem, shield)`.
fn new_weapon(w: &mut World, bp: &Rc<Blueprint>, shield: Option<EntityIdx>) -> EntityIdx {
    let e = new_item_entity(w, bp, "ItemEntityWeapon");
    if shield.is_some()
        && let Some(i) = w.item_mut(e)
    {
        i.shield = shield;
    }
    let v = bp.view();
    if v.bool("Double") && v.guid("m_SecondWeapon") != Some(bp.guid) {
        w.coverage.unsupported("ItemEntityWeapon..ctor: double weapon");
    }
    e
}

/// `new ItemEntityArmor(bpItem, shield)`.
fn new_armor(w: &mut World, bp: &Rc<Blueprint>, shield: Option<EntityIdx>) -> EntityIdx {
    let e = new_item_entity(w, bp, "ItemEntityArmor");
    if let Some(i) = w.item_mut(e) {
        i.shield = shield;
    }
    e
}

/// `new ItemEntityShield(bpItem)`: the shield, then its armor and weapon components.
fn new_shield(w: &mut World, bp: &Rc<Blueprint>) -> EntityIdx {
    let e = new_item_entity(w, bp, "ItemEntityShield");
    let v = bp.view();
    let armor = v.guid("m_ArmorComponent").and_then(|g| w.content.get(g));
    match armor {
        Some(a) => {
            let c = new_armor(w, &a, Some(e));
            if let Some(i) = w.item_mut(e) {
                i.armor_component = Some(c);
            }
        }
        None => w.coverage.unsupported("ItemEntityShield..ctor: no armor component"),
    }
    if let Some(wb) = v.guid("m_WeaponComponent").and_then(|g| w.content.get(g)) {
        let c = new_weapon(w, &wb, Some(e));
        if let Some(i) = w.item_mut(e) {
            i.weapon_component = Some(c);
        }
    }
    e
}

/// The `ItemEntity` constructor: a new entity with a fresh id, on and subscribed, the
/// enchantments of its blueprint, charges and identification.
fn new_item_entity(w: &mut World, bp: &Rc<Blueprint>, clr: &'static str) -> EntityIdx {
    let id = w.new_guid();
    let item =
        Item { blueprint: Some(bp.guid), count: 1, runtime_created: true, ..Item::default() };
    // `EntityDataBase(uniqueId, isInGame: true)`: on, post-loaded, facts manager on,
    // subscribed, registered.
    let e = w.add_entity(Entity {
        unique_id: id,
        clr,
        kind: EntityKind::Item(Box::new(item)),
        facts: FactsManager { list: Vec::new(), capacity: 0, is_turned_on: true },
        parts: Vec::new(),
        is_turned_on: true,
        post_load_executed: true,
        is_in_game: true,
        subscribed: true,
        disposing: false,
        is_disposed: false,
    });
    bus::subscribe(w, Sub::Entity(e));
    // `Facts.Add(new ItemFact(bpItem))` for a blueprint with runtime components.
    if bp
        .components_array()
        .iter()
        .any(|o| wr_content::schema::ty(o.ty).has(wr_content::schema::type_flags::RUNTIME_PROVIDER))
    {
        w.coverage.unsupported("ItemEntity..ctor: ItemFact for runtime components");
    }
    apply_enchantments_on_create(w, e);
    let v = bp.view();
    let equipment = is_a(bp.obj.ty, "BlueprintItemEquipment");
    let gain_ability =
        equipment && (v.guid("m_Ability").is_some() || v.guid("m_ActivatableAbility").is_some());
    let spend_charges = gain_ability && v.bool("SpendCharges");
    let identified = identify_dc(w, bp) == 0;
    let non_removable = equipment && v.bool("IsNonRemovable");
    if let Some(i) = w.item_mut(e) {
        if spend_charges {
            i.charges = v.i32("Charges");
        }
        i.is_non_removable = non_removable;
        i.is_identified = identified;
    }
    if bp.components_array().iter().any(|o| is_a(o.ty, "AddItemShowInfoCallback")) {
        w.coverage.unsupported("ItemEntity.SetupShowInfoCallback");
    }
    e
}

/// `BlueprintItem.IdentifyDC`: a usable item's own, else the highest of its enchantments'.
fn identify_dc(w: &World, bp: &Blueprint) -> i32 {
    if is_a(bp.obj.ty, "BlueprintItemEquipmentUsable") {
        return bp.view().i32("m_IdentifyDC");
    }
    blueprint_enchantments(w, bp)
        .into_iter()
        .filter_map(|g| w.content.get(g))
        .map(|e| e.view().i32("m_IdentifyDC"))
        .max()
        .unwrap_or(0)
}

/// `BlueprintItem.Enchantments` (`CollectEnchantments`): a weapon's or armor's type
/// enchantments then its own; a shield's two components'; a simple or usable item's own.
fn blueprint_enchantments(w: &World, bp: &Blueprint) -> Vec<Guid> {
    let v = bp.view();
    let t = bp.obj.ty;
    if is_a(t, "BlueprintItemWeapon") || is_a(t, "BlueprintItemArmor") {
        let mut out = v
            .guid("m_Type")
            .and_then(|g| w.content.get(g))
            .map(|ty| ty.view().guids("m_Enchantments"))
            .unwrap_or_default();
        out.extend(v.guids("m_Enchantments"));
        out
    } else if is_a(t, "BlueprintItemShield") {
        let mut out = Vec::new();
        for f in ["m_ArmorComponent", "m_WeaponComponent"] {
            if let Some(c) = v.guid(f).and_then(|g| w.content.get(g)) {
                out.extend(blueprint_enchantments(w, &c));
            }
        }
        out
    } else if is_a(t, "BlueprintItemEquipmentSimple") || is_a(t, "BlueprintItemEquipmentUsable") {
        // `BlueprintItemEquipmentSimple.cs:21`, `BlueprintItemEquipmentUsable.cs:132`.
        v.guids("m_Enchantments")
    } else {
        Vec::new()
    }
}

/// `ItemEntity.ApplyEnchantments(onInitializeOrEquip: false)` (`ItemEntity.cs:667`, from
/// `OnTurnOn`): enchantments without a `ParentContext` that the blueprint no longer lists are
/// removed, blueprint enchantments with no such fact are added (built in), in that order.
pub(crate) fn apply_enchantments_on_turn_on(w: &mut World, e: EntityIdx, bp: &Rc<Blueprint>) {
    // `InstantiateEnchantments` is false for shields (their components hold them).
    if w.entity(e).clr == "ItemEntityShield" {
        return;
    }
    let listed = blueprint_enchantments(w, bp);
    let facts: Vec<FactIdx> = w
        .entity(e)
        .facts
        .list
        .iter()
        .copied()
        .filter(|f| w.fact(*f).kind == FactKind::ItemEnchantment)
        .collect();
    let redundant: Vec<FactIdx> = facts
        .iter()
        .copied()
        .filter(|f| {
            !w.fact(*f).parent_context && !w.fact(*f).blueprint.is_some_and(|g| listed.contains(&g))
        })
        .collect();
    // `ContainsEnchantmentFromBlueprint`: a fact of the blueprint with no `ParentContext`.
    let missing: Vec<Guid> = listed
        .iter()
        .copied()
        .filter(|g| {
            !facts.iter().any(|f| w.fact(*f).blueprint == Some(*g) && !w.fact(*f).parent_context)
        })
        .collect();
    for f in redundant {
        fl::manager_remove(w, e, f, true);
    }
    for g in missing {
        if let Some(eb) = w.content.get(g) {
            add_enchantment(w, e, &eb);
        }
    }
}

/// `ItemEntity.ApplyEnchantments(onInitializeOrEquip: true)` on a new item: the blueprint's
/// enchantments are added as built-in ones (no armor recalculation).
fn apply_enchantments_on_create(w: &mut World, e: EntityIdx) {
    // `InstantiateEnchantments` is false for shields (their components hold them).
    if w.entity(e).clr == "ItemEntityShield" {
        return;
    }
    let Some(bp) = w.item(e).and_then(|i| i.blueprint).and_then(|g| w.content.get(g)) else {
        return;
    };
    for g in blueprint_enchantments(w, &bp) {
        if let Some(eb) = w.content.get(g) {
            add_enchantment(w, e, &eb);
        }
    }
}

/// `ItemEnchantmentCollection.AddEnchantment(blueprint)` through the manager: the fact
/// attaches, and activates at once when the item has a wielder (`PrepareFactForAttach`).
fn add_enchantment(w: &mut World, item: EntityIdx, bp: &Rc<Blueprint>) -> FactIdx {
    let f = fl::new_fact(w, FactKind::ItemEnchantment, "ItemEnchantment", bp.clone());
    let suppress = w.item(item).and_then(|i| i.wielder).is_none();
    fl::manager_add_attach_with(w, item, f, suppress, |w, _| {
        // `OnFactDidAttach`: `RuntimeVersion++`, `IFactCollectionUpdatedHandler`.
        bus::raise_global(w, "IFactCollectionUpdatedHandler", None);
    });
    // `OnEnchantmentAdded` under `BuiltInEnchantmentFlag`: no recalculation.
    f
}

/// `ItemEntity.AddEnchantment(blueprint, parentContext)` (`ItemEntity.cs:739`): through the
/// item's `ItemEnchantmentCollection` (none on a shield: `InstantiateEnchantments`), a fact
/// with the parent context, attached (activated at once when the item has a wielder); then
/// `UpdateHasExternalEnchantments` and `OnEnchantmentAdded` (armor recalculates its stats).
pub fn add_enchantment_with_parent(
    w: &mut World,
    item: EntityIdx,
    bp: &Rc<Blueprint>,
    parent: crate::context::MechanicsContext,
) -> Option<FactIdx> {
    if !has_enchantments_collection(w, item) {
        return None;
    }
    // A runtime item's constructor turns its facts on (`EntityDataBase` ctor:
    // `Facts.TurnOn()`); an item made on without them (`combat::weapon::create_weapon`'s limbs)
    // would leave the new enchantment off: reported.
    if w.entity(item).is_turned_on && !w.entity(item).facts.is_turned_on {
        w.coverage.unsupported(
            "ItemEntity.AddEnchantment: an item turned on without its facts turned on",
        );
    }
    let f = fl::new_fact(w, FactKind::ItemEnchantment, "ItemEnchantment", bp.clone());
    w.fact_mut(f).parent_context = true;
    w.fact_mut(f).enchant_parent = Some(Box::new(parent));
    let suppress = w.item(item).and_then(|i| i.wielder).is_none();
    fl::manager_add_attach_with(w, item, f, suppress, |w, _| {
        bus::raise_global(w, "IFactCollectionUpdatedHandler", None);
    });
    update_has_external_enchantments(w, item);
    if w.entity(item).clr == "ItemEntityArmor" {
        crate::armor::recalculate_stats(w, item);
    }
    Some(f)
}

/// `ItemEntity.RemoveEnchantment(enchantment)` (`ItemEntity.cs:746`): out of the collection
/// (disposed), `UpdateHasExternalEnchantments`, `OnEnchantmentRemoved` (armor recalculates).
pub fn remove_enchantment(w: &mut World, item: EntityIdx, f: FactIdx) {
    if has_enchantments_collection(w, item) {
        fl::manager_remove_with(w, item, f, true, |w, _| {
            bus::raise_global(w, "IFactCollectionUpdatedHandler", None);
        });
    }
    update_has_external_enchantments(w, item);
    if w.entity(item).clr == "ItemEntityArmor" {
        crate::armor::recalculate_stats(w, item);
    }
}

/// `ItemEntity.EnchantmentsCollection != null`: the collection is made by the constructor (an
/// item created in this run), `OnPostLoad` (a loaded item, once post-loaded) and `OnDidEquipped`
/// (`ApplyEnchantments`), never for a shield (`InstantiateEnchantments`).
#[must_use]
pub fn has_enchantments_collection(w: &World, item: EntityIdx) -> bool {
    w.entity(item).clr != "ItemEntityShield"
        && (w.entity(item).post_load_executed
            || w.item(item).is_some_and(|i| i.runtime_created)
            || w.equipped_enchantment_collections.contains(&item))
}

/// `UpdateHasExternalEnchantments`: whether any enchantment has a parent context.
fn update_has_external_enchantments(w: &mut World, item: EntityIdx) {
    let any = w
        .entity(item)
        .facts
        .list
        .iter()
        .any(|f| w.fact(*f).kind == FactKind::ItemEnchantment && w.fact(*f).parent_context);
    if any {
        w.external_enchantments.insert(item);
    } else {
        w.external_enchantments.remove(&item);
    }
}

/// `ItemEnchantment.OnActivate`: `m_CurrentContext = CreateContext(this, Owner.Wielder,
/// ParentContext)` (`parentContext.CloneFor(blueprint, wielder)`, or `new MechanicsContext(null,
/// wielder, blueprint)`; then `AddItemSource`).
pub fn enchantment_on_activate(w: &mut World, f: FactIdx) {
    let Some(item) = w.fact(f).owner else { return };
    let wielder = w.item(item).and_then(|i| i.wielder);
    let Some(bp) = w.fact(f).bp.clone() else { return };
    let parent = w.fact(f).enchant_parent.clone();
    let mut ctx = match (parent, wielder) {
        (Some(p), Some(u)) => crate::unit_facts::clone_for(w, &p, &bp, u),
        (Some(_), None) => {
            w.coverage.unsupported("ItemEnchantment.OnActivate: parent context without a wielder");
            crate::unit_facts::context(w, None, wielder, &bp)
        }
        (None, _) => crate::unit_facts::context(w, None, wielder, &bp),
    };
    ctx.source_item = Some(w.entity(item).unique_id.clone());
    w.fact_mut(f).context = Some(ctx);
    if bp.view().obj("WeaponFxPrefab").is_some() {
        w.coverage.stub("ItemEnchantment.OnActivate: weapon effect");
    }
}

// ---- equip and unequip --------------------------------------------------------------

/// `ItemEntity.OnDidEquipped(wielder)` and the subclasses' overrides.
pub fn on_did_equipped(w: &mut World, e: EntityIdx, wielder: EntityIdx) {
    match w.entity(e).clr {
        "ItemEntityWeapon" => base_did_equipped(w, e, wielder),
        "ItemEntityArmor" => {
            base_did_equipped(w, e, wielder);
            crate::armor::recalculate_stats(w, e);
        }
        "ItemEntityShield" => {
            base_did_equipped(w, e, wielder);
            let slot = w.item(e).and_then(|i| i.holding_slot.clone());
            let (armor, weapon) = {
                let i = w.item(e).expect("item");
                (i.armor_component, i.weapon_component)
            };
            if let Some(a) = armor {
                on_did_equipped(w, a, wielder);
                if let Some(i) = w.item_mut(a) {
                    i.holding_slot = slot.clone();
                }
            }
            if let Some(c) = weapon {
                on_did_equipped(w, c, wielder);
                if let Some(i) = w.item_mut(c) {
                    i.holding_slot = slot;
                }
            }
        }
        _ => base_did_equipped(w, e, wielder),
    }
}

/// `ItemEntity.OnDidEquipped`: the wielder, the enchantments (restored if missing, then
/// activated), the facts for the wielder, and the item's ability.
fn base_did_equipped(w: &mut World, e: EntityIdx, wielder: EntityIdx) {
    if let Some(it) = w.item_mut(e) {
        it.wielder = Some(wielder);
    }
    let Some(bp) = w.item(e).and_then(|i| i.blueprint).and_then(|g| w.content.get(g)) else {
        return;
    };
    // `ApplyEnchantments(onInitializeOrEquip: true)`: restores a missing blueprint
    // enchantment (built in, so without recalculation).
    if w.entity(e).clr != "ItemEntityShield" {
        w.equipped_enchantment_collections.insert(e);
        let present: Vec<Option<Guid>> = w
            .entity(e)
            .facts
            .list
            .iter()
            .filter(|f| w.fact(**f).kind == FactKind::ItemEnchantment)
            .map(|f| w.fact(*f).blueprint)
            .collect();
        let missing: Vec<Guid> = blueprint_enchantments(w, &bp)
            .into_iter()
            .filter(|g| !present.contains(&Some(*g)))
            .collect();
        for g in missing {
            if let Some(eb) = w.content.get(g) {
                add_enchantment(w, e, &eb);
            }
        }
        // `m_Enchantments?.SetActive(true)`.
        for f in w.entity(e).facts.list.clone() {
            if w.fact(f).kind == FactKind::ItemEnchantment && !w.fact(f).is_active {
                fl::activate(w, f);
            }
        }
    }
    // `ReapplyFactsForWielder()`.
    if bp.components_array().iter().any(|o| is_a(o.ty, "AddFactToEquipmentWielder")) {
        w.coverage.unsupported("ItemEntity.ReapplyFactsForWielder");
    }
    if !w.item(e).is_some_and(|i| i.is_identified) || !is_a(bp.obj.ty, "BlueprintItemEquipment") {
        return;
    }
    let v = bp.view();
    for (field, is_ability) in [("m_Ability", true), ("m_ActivatableAbility", false)] {
        let Some(abp) = v.guid(field).and_then(|g| w.content.get(g)) else { continue };
        let fact = crate::unit_facts::add_fact(w, wielder, &abp);
        if let Some(f) = fact {
            w.fact_mut(f).source_item = Some(e);
        }
        if let Some(it) = w.item_mut(e) {
            if is_ability {
                it.ability = fact;
            } else {
                it.activatable_ability = fact;
            }
        }
        // `if (Ability) … else if (ActivatableAbility)`.
        break;
    }
}

/// `ItemEntity.OnWillUnequip()` and the subclasses' overrides.
pub fn on_will_unequip(w: &mut World, e: EntityIdx) {
    match w.entity(e).clr {
        "ItemEntityArmor" => {
            crate::armor::on_will_unequip(w, e);
            base_will_unequip(w, e);
        }
        "ItemEntityShield" => {
            let (armor, weapon) = {
                let i = w.item(e).expect("item");
                (i.armor_component, i.weapon_component)
            };
            if let Some(a) = armor {
                on_will_unequip(w, a);
                if let Some(i) = w.item_mut(a) {
                    i.holding_slot = None;
                }
            }
            if let Some(c) = weapon {
                on_will_unequip(w, c);
                if let Some(i) = w.item_mut(c) {
                    i.holding_slot = None;
                }
            }
            base_will_unequip(w, e);
        }
        _ => base_will_unequip(w, e),
    }
}

fn base_will_unequip(w: &mut World, e: EntityIdx) {
    let Some(item) = w.item(e).cloned() else { return };
    let wielder = item.wielder;
    for f in [item.ability, item.activatable_ability].into_iter().flatten() {
        // `Wielder.RemoveFact(fact)`: `EntityFactsManager.Remove` does nothing for a fact
        // that isn't in the list.
        if let Some(u) = wielder
            && w.fact(f).owner == Some(u)
        {
            crate::unit_facts::remove(w, u, f);
        }
    }
    if item.facts_applied_to_wielder {
        w.coverage.unsupported("ItemEntity.OnWillUnequip: m_FactsAppliedToWielder");
    }
    // Enchantments that go with the item are removed (`RemoveOnUnequipItem`: temporary
    // ones, not ported), then deactivated: `SetActive(false, keepPolymorphEnchantments)`.
    // (`ItemEnchantment.RemoveOnUnequipItem` is set only on enchantments added by effects.)
    if w.entity(e).clr != "ItemEntityShield" {
        for f in w.entity(e).facts.list.clone() {
            if w.fact(f).kind == FactKind::ItemEnchantment && w.fact(f).is_active {
                fl::deactivate(w, f);
            }
        }
    }
    if let Some(it) = w.item_mut(e) {
        it.ability = None;
        it.activatable_ability = None;
        it.facts_applied_to_wielder = false;
        it.wielder = None;
    }
}

// ---- slot reads through `MaybeItem` ---------------------------------------------------

/// `ItemSlot.MaybeItem` resolves the slot's item through `EntityService` (`m_ItemRef.Entity`):
/// a loaded item only once its `PrePostLoad` registered it (a placeholder's saved body during
/// the new unit's `Initialize` isn't yet), and an item leaving its slot is hidden.
#[must_use]
pub fn slot_item_resolves(w: &World, item: EntityIdx) -> bool {
    // Registered: by its id (`EntityService.Unregister` at disposal drops it), and not a loaded
    // item still waiting for its `PrePostLoad`.
    w.by_id.get(&w.entity(item).unique_id) == Some(&item)
        && !w.entity(item).is_disposed
        && !w.unregistered_items.contains(&item)
        && !w.item(item).is_some_and(|i| i.remove_from_slot_in_progress)
}

/// `HandSlot.HasItem` of the current hands set's primary or secondary hand.
#[must_use]
pub fn hand_has_item(w: &World, u: EntityIdx, primary: bool) -> bool {
    current_hand_item(w, u, primary).is_some_and(|i| slot_item_resolves(w, i))
}

/// `HandSlot.MaybeWeapon` of the current hands set's hand: [`crate::combat::weapon::hand_weapon`],
/// except that a slot whose item doesn't resolve (`MaybeItem == null`) gives the empty-hand
/// weapon.
pub fn hand_maybe_weapon(w: &mut World, u: EntityIdx, primary: bool) -> Option<EntityIdx> {
    let set = w.unit(u).map_or(0, |x| x.body.current_hand_set);
    let hand = if primary { "PrimaryHand" } else { "SecondaryHand" };
    let raw = find_slot(w, u, &format!("HandsEquipmentSets[{set}].{hand}")).and_then(|s| s.item);
    if raw.is_some_and(|i| !slot_item_resolves(w, i)) {
        return w.unit(u).and_then(|x| x.body.empty_hand);
    }
    let h = if primary {
        crate::combat::weapon::Hand::Primary
    } else {
        crate::combat::weapon::Hand::Secondary
    };
    crate::combat::weapon::hand_weapon(w, u, h)
}

/// `HandSlot.HasWeapon` (the hand's override): `MaybeWeapon != null`, the empty-hand weapon
/// included.
pub fn hand_has_weapon(w: &mut World, u: EntityIdx, primary: bool) -> bool {
    hand_maybe_weapon(w, u, primary).is_some()
}

/// `UnitBody.AdditionalLimbs`, each slot's `HasItem` and `MaybeWeapon` (the empty-hand weapon
/// when the slot has no resolved item or is inactive; else its item if a weapon).
#[must_use]
pub fn limb_slots(w: &World, u: EntityIdx) -> Vec<(bool, Option<EntityIdx>)> {
    let Some(unit) = w.unit(u) else { return Vec::new() };
    unit.body
        .limbs
        .iter()
        .map(|s| {
            let item = s.item.filter(|i| slot_item_resolves(w, *i));
            let weapon = match item {
                Some(i) if s.active => Weapon::of(w, i).map(|_| i),
                _ => unit.body.empty_hand,
            };
            (item.is_some(), weapon)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::tests::obj;
    use wr_content::node::Node;

    /// A potion blueprint (a usable item) with the given identify DC.
    fn potion(w: &World, g: u128, identify_dc: i64) -> Rc<Blueprint> {
        let o = obj(
            "Kingmaker.Blueprints.Items.Equipment.BlueprintItemEquipmentUsable",
            &[("m_IdentifyDC", Node::Int(identify_dc))],
        );
        w.content.insert(Blueprint {
            guid: Guid(g),
            name: format!("Potion{g}"),
            obj: o,
            source: crate::content::Source::Bbp,
        });
        w.content.get(Guid(g)).expect("inserted")
    }

    /// A unit with one inventory and one quick slot.
    fn world() -> (World, EntityIdx) {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        let unit = w.unit_mut(u).expect("unit");
        unit.inventory_collection = Some(1);
        unit.body.slots.push(Slot { name: "QuickSlot0".into(), item: None, active: true });
        (w, u)
    }

    #[test]
    fn slot_names_say_what_the_slot_is() {
        assert_eq!(
            slot_kind("HandsEquipmentSets[2].SecondaryHand"),
            SlotKind::Hand { set: 2, primary: false }
        );
        assert_eq!(slot_kind("Armor"), SlotKind::Armor);
        assert_eq!(slot_kind("Ring2"), SlotKind::Equipment("BlueprintItemEquipmentRing"));
        assert_eq!(slot_kind("QuickSlot3"), SlotKind::Usable);
        assert_eq!(slot_kind("AdditionalLimbs[0]"), SlotKind::Limb);
        assert_eq!(
            pair_name("HandsEquipmentSets[1].PrimaryHand").as_deref(),
            Some("HandsEquipmentSets[1].SecondaryHand")
        );
    }

    /// `ItemsCollection.Add` merges an identified item into a stack of its blueprint in the
    /// collection; the merged-away entity is disposed.
    #[test]
    fn identical_identified_items_stack_in_a_collection() {
        let (mut w, u) = world();
        let bp = potion(&w, 100, 0);
        let a = create_entity(&mut w, &bp).expect("item");
        let b = create_entity(&mut w, &bp).expect("item");
        assert!(w.item(a).is_some_and(|i| i.is_identified && i.count == 1));
        assert_eq!(inventory_add(&mut w, u, a, false, true), Some(a));
        assert_eq!(inventory_add(&mut w, u, b, false, true), Some(a), "merged into the stack");
        assert_eq!(w.item(a).map(|i| i.count), Some(2));
        assert!(w.entity(b).is_disposed);
        assert_eq!(collection_items(&w, 1), vec![a]);
    }

    #[test]
    fn unidentified_items_do_not_stack() {
        let (mut w, u) = world();
        let bp = potion(&w, 101, 5);
        let a = create_entity(&mut w, &bp).expect("item");
        let b = create_entity(&mut w, &bp).expect("item");
        assert!(!w.item(a).expect("item").is_identified);
        inventory_add(&mut w, u, a, false, true);
        assert_eq!(inventory_add(&mut w, u, b, false, true), Some(b));
        assert_eq!(collection_items(&w, 1), vec![a, b]);
    }

    /// Equipping one of a stack splits one off into the slot (`ItemSlot.InsertItem`);
    /// disposing the equipped item unequips it and takes it out of the collection.
    #[test]
    fn a_stack_gives_one_to_the_slot_and_dispose_unequips() {
        let (mut w, u) = world();
        let bp = potion(&w, 102, 0);
        let first = create_entity(&mut w, &bp).expect("item");
        inventory_add(&mut w, u, first, false, true);
        let second = create_entity(&mut w, &bp).expect("item");
        inventory_add(&mut w, u, second, false, true);
        assert_eq!(w.item(first).map(|i| i.count), Some(2));
        let one = w.item(first).map(|_| first).expect("stack");
        assert!(insert_item(&mut w, u, "QuickSlot0", one, true));
        let held = slot_item(&w, u, "QuickSlot0").expect("equipped");
        assert_ne!(held, first, "a new item of count 1 was split off the stack");
        assert_eq!(w.item(held).map(|i| (i.count, i.wielder)), Some((1, Some(u))));
        assert_eq!(w.item(first).map(|i| i.count), Some(1));
        assert!(collection_items(&w, 1).contains(&held));
        crate::entity::dispose(&mut w, held);
        assert!(slot_item(&w, u, "QuickSlot0").is_none());
        assert_eq!(collection_items(&w, 1), vec![first]);
        assert!(w.entity(held).is_disposed);
    }

    /// A usable item fits quick slots only.
    #[test]
    fn a_potion_does_not_fit_the_armor_slot() {
        let (mut w, u) = world();
        w.unit_mut(u).expect("unit").body.slots.push(Slot {
            name: "Armor".into(),
            item: None,
            active: true,
        });
        let bp = potion(&w, 103, 0);
        let a = create_entity(&mut w, &bp).expect("item");
        assert!(!can_insert_item(&mut w, u, "Armor", a, false));
        assert!(can_insert_item(&mut w, u, "QuickSlot0", a, false));
    }

    /// A blueprint of an item family with the given enchantments.
    fn with_enchantments(w: &World, ty: &str, g: u128, enchantments: &[u128]) -> Rc<Blueprint> {
        let list = Node::Seq(enchantments.iter().map(|e| Node::BpRef(Guid(*e))).collect());
        let o = obj(ty, &[("m_Enchantments", list)]);
        w.content.insert(Blueprint {
            guid: Guid(g),
            name: format!("Item{g}"),
            obj: o,
            source: crate::content::Source::Bbp,
        });
        w.content.get(Guid(g)).expect("inserted")
    }

    fn enchantment(w: &World, g: u128, identify_dc: i64) {
        let o = obj(
            "Kingmaker.Blueprints.Items.Ecnchantments.BlueprintEquipmentEnchantment",
            &[("m_IdentifyDC", Node::Int(identify_dc))],
        );
        w.content.insert(Blueprint {
            guid: Guid(g),
            name: format!("Ench{g}"),
            obj: o,
            source: crate::content::Source::Bbp,
        });
    }

    /// `CollectEnchantments` of simple equipment and usable items returns their own
    /// enchantments: facts exist, and the identify DC is the highest one.
    #[test]
    fn simple_and_usable_items_collect_their_enchantments() {
        let (mut w, _) = world();
        enchantment(&w, 200, 10);
        let belt = with_enchantments(
            &w,
            "Kingmaker.Blueprints.Items.Equipment.BlueprintItemEquipmentBelt",
            201,
            &[200],
        );
        let a = create_entity(&mut w, &belt).expect("belt");
        assert_eq!(w.entity(a).facts.list.len(), 1, "the belt's enchantment fact");
        assert!(!w.item(a).expect("item").is_identified, "DC 10 needs identification");
        let usable = with_enchantments(
            &w,
            "Kingmaker.Blueprints.Items.Equipment.BlueprintItemEquipmentUsable",
            202,
            &[200],
        );
        let b = create_entity(&mut w, &usable).expect("usable");
        assert_eq!(w.entity(b).facts.list.len(), 1, "the usable item's enchantment fact");
    }

    /// `ItemEntity.IncrementCount` wraps like unchecked C# arithmetic.
    #[test]
    fn stack_counts_wrap_like_csharp() {
        let (mut w, u) = world();
        let bp = potion(&w, 110, 0);
        let a = create_entity(&mut w, &bp).expect("item");
        inventory_add(&mut w, u, a, false, true);
        w.item_mut(a).expect("item").count = i32::MAX;
        let b = create_entity(&mut w, &bp).expect("item");
        assert_eq!(inventory_add(&mut w, u, b, false, true), Some(a));
        assert_eq!(w.item(a).map(|i| i.count), Some(i32::MIN));
    }

    /// An item with an overridden visual (`VisualSourceItemBlueprint`) never stacks.
    #[test]
    fn an_item_with_an_overridden_visual_does_not_stack() {
        let (mut w, u) = world();
        let bp = potion(&w, 111, 0);
        let a = create_entity(&mut w, &bp).expect("item");
        w.item_mut(a).expect("item").visual_source = Some(Guid(1));
        inventory_add(&mut w, u, a, false, true);
        let b = create_entity(&mut w, &bp).expect("item");
        assert_eq!(inventory_add(&mut w, u, b, false, true), Some(b), "not merged");
        assert_eq!(collection_items(&w, 1), vec![a, b]);
    }

    struct SlotLog(std::cell::RefCell<Vec<String>>);

    impl crate::rulebook::bus::ExternalHandler for SlotLog {
        fn interfaces(&self) -> Vec<String> {
            vec!["IUnitEquipmentHandler".into()]
        }
        fn event(&self, w: &mut World, _iface: &str) {
            if let Some((_, n)) = w.bus.slot.last() {
                self.0.borrow_mut().push(n.clone());
            }
        }
        fn rule(
            &self,
            _w: &mut World,
            _rule: &mut dyn crate::rulebook::RuleEvent,
            _step: crate::rulebook::bus::RuleStep,
        ) {
        }
    }

    /// Moving an equipped item to another slot notifies the vacated slot first
    /// (`ItemSlot.InsertItem`, `ItemSlot.cs:274`), then the destination.
    #[test]
    fn moving_an_equipped_item_notifies_the_vacated_slot_first() {
        let (mut w, u) = world();
        w.unit_mut(u).expect("unit").body.slots.push(Slot {
            name: "QuickSlot1".into(),
            item: None,
            active: true,
        });
        let bp = potion(&w, 112, 0);
        let a = create_entity(&mut w, &bp).expect("item");
        inventory_add(&mut w, u, a, false, true);
        assert!(insert_item(&mut w, u, "QuickSlot0", a, true));
        let held = slot_item(&w, u, "QuickSlot0").expect("equipped");
        let log = Rc::new(SlotLog(std::cell::RefCell::new(Vec::new())));
        w.bus.external.insert(7, log.clone());
        bus::subscribe(&mut w, Sub::External(7));
        assert!(insert_item(&mut w, u, "QuickSlot1", held, true));
        assert_eq!(*log.0.borrow(), vec!["QuickSlot0".to_owned(), "QuickSlot1".to_owned()]);
    }
}
