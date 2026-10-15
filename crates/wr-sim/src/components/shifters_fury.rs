//! `ShiftersFury` and its unit part `ShiftersFuryPart` (decompiled, not copied:
//! `Kingmaker.UnitLogic.ActivatableAbilities/ShiftersFury.cs`, `ShiftersFuryPart.cs`,
//! `ShiftersFuryItemData.cs`).
//!
//! The fact keeps a list of toggles, one per natural weapon the owner wields (`AppliedFacts`,
//! each an `ActivatableAbility` of `m_ItemBlueprint` whose `ShiftersFuryItemAbility` data names
//! the weapon and hand). The list isn't saved; the part's `State` is (the count and the selected
//! toggle at the last turn-off), and turning the fact on rebuilds the list from it
//! (`RestoreState`). While the part is on it subscribes to the owner's equipment, empty-hand,
//! polymorph and level-up events and rebuilds the list when the weapons changed
//! (`RecreateListItems`).

use serde_json::json;
use wr_content::Guid;

use super::{Port, bits, owner_unit, with_settings};
use crate::combat::weapon::{Weapon, has_feature, hold_in_two_hands};
use crate::items::{hand_maybe_weapon, limb_slots};
use crate::registry::{Hook, Registry};
use crate::world::{CompRef, EntityIdx, FactIdx, World};

pub const PART: &str = "ShiftersFuryPart";

/// `ShiftersFury.HandType`.
const PRIMARY_HAND: i64 = 0;
const SECONDARY_HAND: i64 = 1;
const LIMB: i64 = 2;

/// `MechanicsFeatureType.MythicDemonNaturalAttacksNoSecondary`.
const MYTHIC_DEMON_NATURAL_ATTACKS_NO_SECONDARY: i64 = 90;

/// `ShiftersFuryPart.InternalState` (saved).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub weapons_count: i64,
    pub selected_weapon_index: i64,
    /// `ShiftersFury.HandType`.
    pub selected_hand: i64,
    pub selected_weapon_bp: Option<Guid>,
}

/// `ShiftersFuryPart`.
#[derive(Debug, Clone, Default)]
pub struct Part {
    /// `State` (saved; `null` until the first `SaveState`).
    pub state: Option<State>,
    /// `m_ItemBlueprint` (set by `Setup`).
    pub item_blueprint: Option<Guid>,
    /// `AppliedFacts`.
    pub applied: Vec<FactIdx>,
    /// `m_IsOn` (the part's own turn-on).
    pub is_on: bool,
}

/// `ShiftersFuryItemData`: a weapon and its hand.
type ItemData = (EntityIdx, i64);

pub fn register(r: &mut Registry) {
    r.register(Box::new(Port::new(
        "ShiftersFury",
        bits(&[Hook::TurnOn, Hook::TurnOff, Hook::Activate, Hook::Deactivate, Hook::PostLoad]),
        shifters_fury,
    )));
}

/// The saved part (`ShiftersFuryPart`'s `State`, its members' defaults when absent).
#[must_use]
pub fn part_from_save(o: crate::hydrate::graph::O<'_>) -> Part {
    let state = o.obj("State").map(|s| State {
        weapons_count: s.int("WeaponsCount").unwrap_or(0),
        selected_weapon_index: s.int("SelectedWeaponIndex").unwrap_or(-1),
        selected_hand: s.int("SelectedHand").unwrap_or(PRIMARY_HAND),
        selected_weapon_bp: s.guid("SelectedWeaponBP"),
    });
    Part { state, ..Part::default() }
}

fn part(w: &World, u: EntityIdx) -> Option<&Part> {
    w.unit(u)?.shifters_fury.as_ref()
}

fn part_mut(w: &mut World, u: EntityIdx) -> Option<&mut Part> {
    w.unit_mut(u)?.shifters_fury.as_mut()
}

/// `Owner.Ensure<ShiftersFuryPart>()`: a new part is appended and, the owner being on, turned on
/// (`EntityPart.AttachToEntity`).
fn ensure(w: &mut World, u: EntityIdx) {
    if part(w, u).is_some() {
        return;
    }
    w.entity_mut(u).parts.push(crate::world::Part { clr: PART });
    if let Some(x) = w.unit_mut(u) {
        x.shifters_fury = Some(Part::default());
    }
    if w.entity(u).is_turned_on {
        part_turn_on(w, u);
    }
}

/// `EntityPart.TurnOn` for the part (unless on): `Subscribe()`, then `OnTurnOn`
/// (`m_IsOn = true`). `IsTurnedOn` and `m_IsOn` change together, so one flag stands for both.
pub fn part_turn_on(w: &mut World, u: EntityIdx) {
    if part(w, u).is_none_or(|p| p.is_on) {
        return;
    }
    crate::rulebook::bus::subscribe(w, crate::rulebook::bus::Sub::Part(u, PART));
    if let Some(p) = part_mut(w, u) {
        p.is_on = true;
    }
}

/// The part's removal (`EntityPartsManager.Remove`/`Dispose`: `TurnOff` if the owner is on, then
/// `EntityPart.Dispose`, which unsubscribes unconditionally).
pub fn part_dispose(w: &mut World, u: EntityIdx) {
    if w.entity(u).is_turned_on {
        part_turn_off(w, u);
    }
    crate::rulebook::bus::unsubscribe(w, crate::rulebook::bus::Sub::Part(u, PART));
    if let Some(x) = w.unit_mut(u) {
        x.shifters_fury = None;
    }
}

/// `EntityPart.TurnOff` for the part (if on): `Unsubscribe()`, then `OnTurnOff`
/// (`m_IsOn = false`).
pub fn part_turn_off(w: &mut World, u: EntityIdx) {
    if !part(w, u).is_some_and(|p| p.is_on) {
        return;
    }
    crate::rulebook::bus::unsubscribe(w, crate::rulebook::bus::Sub::Part(u, PART));
    if let Some(p) = part_mut(w, u) {
        p.is_on = false;
    }
}

fn shifters_fury(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::PostLoad | Hook::Activate => setup(w, c, u),
        // `Owner.Ensure<ShiftersFuryPart>().RestoreState()` / `.SaveState()`.
        Hook::TurnOn => {
            ensure(w, u);
            restore_state(w, u);
        }
        Hook::TurnOff => {
            ensure(w, u);
            save_state(w, u);
        }
        // The part's list is cleared and the part removed (turned off first if the owner is on).
        // (`EntityPartsManager.Remove(part)`: off the list, `TurnOff` if the owner is on, then
        // `Dispose`, which the part doesn't override.)
        Hook::Deactivate if part(w, u).is_some() => {
            remove_existing_items(w, u);
            w.entity_mut(u).parts.retain(|p| p.clr != PART);
            part_dispose(w, u);
        }
        _ => {}
    }
}

/// `ShiftersFury.Setup()`: the fact turns on after the others and off before them, then the
/// part takes the item blueprint and rebuilds its list (`Setup` → `RecreateListItems`).
fn setup(w: &mut World, c: CompRef, u: EntityIdx) {
    w.fact_mut(c.fact).turn_off_priority = -1;
    w.fact_mut(c.fact).turn_on_priority = 1;
    ensure(w, u);
    let item = with_settings(w, c, |v| v.guid("m_ItemBlueprint")).flatten();
    if let Some(p) = part_mut(w, u) {
        p.item_blueprint = item;
    }
    recreate_list_items(w, u);
}

/// `RecreateListItems()`: while the part is on, the list is rebuilt if the weapons changed.
fn recreate_list_items(w: &mut World, u: EntityIdx) {
    if !part(w, u).is_some_and(|p| p.is_on) {
        return;
    }
    let weapons = collect_current_weapons(w, u);
    if are_weapons_changed(w, u, &weapons) {
        remove_existing_items(w, u);
        for d in weapons {
            try_create_list_item(w, u, d);
        }
    }
}

/// `SaveState()`: the count, and the last toggle that is on (its index, hand and weapon).
fn save_state(w: &mut World, u: EntityIdx) {
    let applied = part(w, u).map(|p| p.applied.clone()).unwrap_or_default();
    let mut s = State {
        weapons_count: i64::try_from(applied.len()).unwrap_or(i64::MAX),
        selected_weapon_index: -1,
        selected_hand: PRIMARY_HAND,
        selected_weapon_bp: None,
    };
    for (i, f) in applied.iter().enumerate() {
        if w.fact(*f).activatable.is_some_and(|a| a.0) {
            // `data.Weapon.Blueprint`: a toggle without data throws (not modelled: reported).
            let Some((weapon, hand)) = item_data(w, *f) else {
                w.coverage.unsupported("ShiftersFuryPart.SaveState: a toggle without its data");
                return;
            };
            s.selected_weapon_index = i64::try_from(i).unwrap_or(i64::MAX);
            s.selected_hand = hand;
            s.selected_weapon_bp = w.item(weapon).and_then(|x| x.blueprint);
        }
    }
    if let Some(p) = part_mut(w, u) {
        p.state = Some(s);
    }
}

/// `RestoreState()`: with a saved state, the list is rebuilt from the current weapons; when
/// the count and a selected weapon are as saved, the selected toggle is turned on again.
fn restore_state(w: &mut World, u: EntityIdx) {
    let Some(state) = part(w, u).and_then(|p| p.state.clone()) else { return };
    let weapons = collect_current_weapons(w, u);
    remove_existing_items(w, u);
    let count = i64::try_from(weapons.len()).unwrap_or(i64::MAX);
    if state.weapons_count != count || state.selected_weapon_bp.is_none() {
        for d in weapons {
            try_create_list_item(w, u, d);
        }
        return;
    }
    // `FillItemsListAndRestoreSelection`.
    for (i, d) in weapons.into_iter().enumerate() {
        let ability = try_create_list_item(w, u, d);
        if ability.is_some() && state.selected_weapon_index == i64::try_from(i).unwrap_or(-2) {
            let bp = w.item(d.0).and_then(|x| x.blueprint);
            if d.1 != state.selected_hand || bp != state.selected_weapon_bp {
                break;
            }
            if let Some(a) = ability {
                crate::unit_facts::set_is_on(w, a, true);
            }
        }
    }
}

/// `RemoveExistingItems()`: each toggle is removed from the owner, then the list cleared.
fn remove_existing_items(w: &mut World, u: EntityIdx) {
    let applied = part(w, u).map(|p| p.applied.clone()).unwrap_or_default();
    for f in applied {
        crate::unit_facts::remove(w, u, f);
    }
    if let Some(p) = part_mut(w, u) {
        p.applied.clear();
    }
}

/// `AreWeaponsChanged(weaponsData)`: another count, a toggle without data, or a toggle whose
/// weapon and hand aren't among the current ones.
fn are_weapons_changed(w: &World, u: EntityIdx, weapons: &[ItemData]) -> bool {
    let applied = part(w, u).map(|p| p.applied.clone()).unwrap_or_default();
    if applied.len() != weapons.len() {
        return true;
    }
    applied.iter().any(|f| item_data(w, *f).is_none_or(|d| !weapons.contains(&d)))
}

/// `CollectCurrentWeapons`: the current hands' natural weapons (the secondary hand only when the
/// primary weapon isn't held in two hands), then each additional limb's natural weapon unless it
/// is forced secondary (a natural one isn't when its owner has
/// `MythicDemonNaturalAttacksNoSecondary`). Hands disabled by a polymorph aren't modelled
/// (`PolymorphUsedPrimaryHandForFirstLimbWeapon`): reported.
fn collect_current_weapons(w: &mut World, u: EntityIdx) -> Vec<ItemData> {
    let mut out = Vec::new();
    if w.unit(u).is_some_and(|x| x.body.hands_disabled) {
        w.coverage
            .unsupported("ShiftersFuryPart.CollectCurrentWeapons: hands disabled (polymorph)");
    } else {
        let primary = hand_maybe_weapon(w, u, true);
        add_weapon_data(w, &mut out, primary, PRIMARY_HAND);
        let two_handed =
            primary.and_then(|p| Weapon::of(w, p)).is_some_and(|p| hold_in_two_hands(w, &p));
        if !two_handed {
            let secondary = hand_maybe_weapon(w, u, false);
            add_weapon_data(w, &mut out, secondary, SECONDARY_HAND);
        }
    }
    // `additionalLimb.HasWeapon`: then `additionalLimb.Weapon`.
    for limb in limb_slots(w, u).into_iter().filter(|l| l.0).filter_map(|l| l.1) {
        let Some(wp) = Weapon::of(w, limb) else { continue };
        let mut forced = w.item(limb).is_some_and(|x| x.force_secondary);
        let owner = w.item(limb).and_then(|x| x.wielder);
        if owner.is_some_and(|o| has_feature(w, o, MYTHIC_DEMON_NATURAL_ATTACKS_NO_SECONDARY))
            && wp.is_natural()
        {
            forced = false;
        }
        if !forced {
            add_weapon_data(w, &mut out, Some(limb), LIMB);
        }
    }
    out
}

/// `AddWeaponData`: a natural, non-unarmed weapon with a blueprint.
fn add_weapon_data(w: &World, out: &mut Vec<ItemData>, weapon: Option<EntityIdx>, hand: i64) {
    let Some(i) = weapon else { return };
    if Weapon::of(w, i).is_some_and(|x| x.is_natural() && !x.is_unarmed()) {
        out.push((i, hand));
    }
}

/// `TryCreateListItem(data)`: a toggle of the item blueprint is added to the owner, listed, and
/// given the weapon and hand (`ShiftersFuryItemAbility`'s data; the UI override is presentation).
fn try_create_list_item(w: &mut World, u: EntityIdx, d: ItemData) -> Option<FactIdx> {
    if !Weapon::of(w, d.0).is_some_and(|x| x.is_natural() && !x.is_unarmed()) {
        return None;
    }
    // `Owner.AddFact(m_ItemBlueprint) as ActivatableAbility`: a missing blueprint gives null.
    let bp = part(w, u).and_then(|p| p.item_blueprint).and_then(|g| w.content.get(g))?;
    let f = crate::unit_facts::add_fact(w, u, &bp)?;
    if w.fact(f).kind != crate::world::FactKind::ActivatableAbility {
        return None;
    }
    if let Some(p) = part_mut(w, u) {
        p.applied.push(f);
    }
    // `ability.GetData<ShiftersFuryItemAbility, ShiftersFuryItemData>().CopyFrom(data)`.
    match data_component(w, f) {
        Some(c) => {
            let id = w.entity(d.0).unique_id.clone();
            w.component_mut(c).data = Some(json!({ "Hand": d.1, "Weapon": id }));
        }
        None => w.coverage.unsupported("ShiftersFuryPart: item ability without its data component"),
    }
    Some(f)
}

/// The toggle's `ShiftersFuryItemAbility` runtime.
fn data_component(w: &World, f: FactIdx) -> Option<CompRef> {
    (0..w.fact(f).components.len()).map(|comp| CompRef { fact: f, comp }).find(|c| {
        w.component(*c)
            .ty()
            .is_some_and(|t| wr_content::schema::ty(t).short == "ShiftersFuryItemAbility")
    })
}

/// The toggle's `ShiftersFuryItemData` (`null` without the component).
fn item_data(w: &World, f: FactIdx) -> Option<ItemData> {
    let c = data_component(w, f)?;
    let d = w.component(c).data.as_ref()?;
    let weapon = w.by_id.get(d.get("Weapon")?.as_str()?).copied()?;
    Some((weapon, d.get("Hand")?.as_i64()?))
}

/// The part's event handlers (`ShiftersFuryPart`'s interfaces), `iface` raised: each rebuilds the
/// list when the event is the owner's (`RecreateListIfOwner`); `HandleUnitInitialized` and
/// `HandleUnitEmptyHandWeaponUpdated` are the owner's own events. Whether the part handled it.
pub fn part_event(w: &mut World, u: EntityIdx, iface: &str) -> bool {
    let (method, unit) = w.bus.call.last().copied().unwrap_or(("", None));
    match iface {
        "IUnitInitializedHandler" | "IUnitEmptyHandWeaponHandler" => recreate_list_items(w, u),
        // `slot.Owner`.
        "IUnitEquipmentHandler" | "IRemoveItemFromSlotHandler" => {
            if w.bus.slot.last().is_some_and(|s| s.0 == u) {
                recreate_list_items(w, u);
            }
        }
        "IUnitLevelUpHandler" if method == "HandleUnitBeforeLevelUp" => {}
        "IUnitActiveEquipmentSetHandler"
        | "IPolymorphActivatedHandler"
        | "IPolymorphDeactivatedHandler"
        | "IUnitLevelUpHandler" => match unit {
            Some(x) if x == u => recreate_list_items(w, u),
            Some(_) => {}
            None => return false,
        },
        _ => return false,
    }
    true
}

/// `ShiftersFuryPart`'s handler interfaces (its declaration).
pub const PART_HANDLERS: &[&str] = &[
    "IUnitActiveEquipmentSetHandler",
    "IUnitInitializedHandler",
    "IPolymorphDeactivatedHandler",
    "IPolymorphActivatedHandler",
    "IUnitEquipmentHandler",
    "IRemoveItemFromSlotHandler",
    "IUnitEmptyHandWeaponHandler",
    "IUnitLevelUpHandler",
];
