//! `EmptyHandWeaponOverride` and the empty-hand part of `UnitBody` (decompiled, not copied:
//! `Kingmaker.Designers.Mechanics.Buffs/EmptyHandWeaponOverride.cs`,
//! `Kingmaker.Items/UnitBody.cs` `SetEmptyHandWeapon`, `SetNewerWeapon`,
//! `SinkEmptyHandedWeapon`, `EquipEmptyHandedWeapon`, `RemoveEmptyHandWeapon`,
//! `Kingmaker.RuleSystem/DiceFormula.cs` `IsBetterThan`).
//!
//! Turning the component on creates a new empty-hand weapon from its blueprint (a fresh
//! entity each time, also after a load: the component's `Data.WeaponEntity` isn't saved, and
//! neither are the weapon's `IsPermanentEmptyHand` and `IsMonkUnarmedStrike`). A non-permanent
//! weapon replaces the current one, which goes to the stack; a permanent one replaces it only
//! if its damage dice are better, else it sinks into the stack by rank. Turning it off
//! brings the stack's last weapon back.

use serde_json::{Value, json};

use super::{Port, bits, owner_unit, with_settings};
use crate::combat::weapon::{DiceFormula, Weapon, scale, weapon_size};
use crate::registry::{Hook, Registry};
use crate::world::{CompRef, EntityIdx, World};

pub fn register(r: &mut Registry) {
    let mut p = Port::new("EmptyHandWeaponOverride", bits(&[Hook::TurnOn, Hook::TurnOff]), hook);
    p.handlers = &["IUnitBodyHandsAreEnabledHandler"];
    p.ev = Some(hands_enabled_event);
    r.register(Box::new(p));
}

fn hook(w: &mut World, c: CompRef, h: Hook) {
    match h {
        Hook::TurnOn => set_weapon(w, c),
        Hook::TurnOff => remove_weapon(w, c),
        _ => {}
    }
}

/// `OnUnitBodyHandsAreEnabledUpdated`: raised by `UnitBody.HandsAreEnabled`'s setter, which
/// nothing in the sim changes; reported if it ever arrives.
fn hands_enabled_event(w: &mut World, _c: CompRef, _iface: &str) {
    w.coverage.unsupported("EmptyHandWeaponOverride.OnUnitBodyHandsAreEnabledUpdated");
}

fn weapon_entity(w: &World, c: CompRef) -> Option<EntityIdx> {
    let id = w.component(c).data.as_ref()?.get("WeaponEntity")?.as_str()?.to_owned();
    w.by_id.get(&id).copied()
}

fn store_weapon(w: &mut World, c: CompRef, e: Option<EntityIdx>) {
    let v = e.map_or(Value::Null, |e| json!(w.entity(e).unique_id));
    w.component_mut(c).data = Some(json!({ "WeaponEntity": v }));
}

/// `SetWeapon()`: `Data.WeaponEntity = Owner.Body.SetEmptyHandWeapon(Weapon, IsMonkUnarmedStrike,
/// IsPermanent)`.
fn set_weapon(w: &mut World, c: CompRef) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((bp, monk, permanent)) = with_settings(w, c, |v| {
        (v.guid("m_Weapon"), v.bool("IsMonkUnarmedStrike"), v.bool("IsPermanent"))
    }) else {
        return;
    };
    let item = set_empty_hand_weapon(w, u, bp, monk, permanent);
    store_weapon(w, c, item);
}

/// `RemoveWeapon()`.
fn remove_weapon(w: &mut World, c: CompRef) {
    let Some(u) = owner_unit(w, c) else { return };
    if let Some(item) = weapon_entity(w, c) {
        remove_empty_hand_weapon(w, u, Some(item));
    } else {
        remove_empty_hand_weapon(w, u, None);
    }
    store_weapon(w, c, None);
}

fn flags(w: &World, u: EntityIdx, item: EntityIdx) -> (bool, bool) {
    w.unit(u)
        .and_then(|x| x.body.empty_hand_flags.iter().find(|f| f.0 == item))
        .map_or((false, false), |f| (f.1, f.2))
}

fn set_flags(w: &mut World, u: EntityIdx, item: EntityIdx, permanent: bool, monk: bool) {
    if let Some(unit) = w.unit_mut(u) {
        unit.body.empty_hand_flags.retain(|f| f.0 != item);
        unit.body.empty_hand_flags.push((item, permanent, monk));
    }
}

/// `MaxValue(0)` of a `DiceFormula` (`Zero` and `One` count their rolls only).
fn max_value(d: DiceFormula) -> i32 {
    if d == DiceFormula::ZERO || d == DiceFormula::ONE {
        d.rolls
    } else {
        d.dice.wrapping_mul(d.rolls()).max(1)
    }
}

/// `ItemEntityWeapon.DamageDice` = `Blueprint.ScaleDamage(Size)`.
fn damage_dice(w: &mut World, item: EntityIdx) -> Option<DiceFormula> {
    let wp = Weapon::of(w, item)?;
    let size = weapon_size(w, &wp);
    let base_size = if wp.damage_dice_overridden() {
        wp.blueprint_size()
    } else {
        crate::stats::types::Size::Medium
    };
    Some(scale(w, wp.base_damage(), size, base_size))
}

/// `ItemEntityWeapon.IsBetterThan`.
fn is_better_than(w: &mut World, a: EntityIdx, b: EntityIdx) -> bool {
    match (damage_dice(w, a), damage_dice(w, b)) {
        (Some(x), Some(y)) => max_value(x) > max_value(y),
        _ => false,
    }
}

/// `UnitBody.SetEmptyHandWeapon`.
pub fn set_empty_hand_weapon(
    w: &mut World,
    u: EntityIdx,
    weapon: Option<wr_content::Guid>,
    monk: bool,
    permanent: bool,
) -> Option<EntityIdx> {
    if w.unit(u).is_none_or(|x| x.body.hands_disabled) {
        w.log("error", "Hands are not enabled, can't set empty hand weapon");
        return None;
    }
    let Some(bp) = weapon.and_then(|g| w.content.get(g)) else {
        w.log("error", "Weapon blueprint is null");
        return None;
    };
    let item = crate::items::create_entity(w, &bp)?;
    set_flags(w, u, item, permanent, monk);
    let current = w.unit(u).and_then(|x| x.body.empty_hand);
    match current {
        Some(cur) if permanent => {
            if flags(w, u, cur).0 && is_better_than(w, item, cur) {
                set_newer_weapon(w, u, item);
            } else {
                sink_empty_handed_weapon(w, u, Some(item));
            }
        }
        _ => set_newer_weapon(w, u, item),
    }
    Some(item)
}

/// `SetNewerWeapon`: the current weapon sinks, the new one is equipped.
fn set_newer_weapon(w: &mut World, u: EntityIdx, new: EntityIdx) {
    let cur = w.unit(u).and_then(|x| x.body.empty_hand);
    sink_empty_handed_weapon(w, u, cur);
    equip_empty_handed_weapon(w, u, new);
}

/// `SinkEmptyHandedWeapon`.
fn sink_empty_handed_weapon(w: &mut World, u: EntityIdx, new: Option<EntityIdx>) {
    let Some(new) = new else { return };
    let (permanent, _) = flags(w, u, new);
    if !permanent {
        if let Some(x) = w.unit_mut(u) {
            x.body.empty_hand_stack.push(new);
        }
        return;
    }
    let stack = w.unit(u).map(|x| x.body.empty_hand_stack.clone()).unwrap_or_default();
    let mut i = stack.len();
    while i > 0 {
        let cand = stack[i - 1];
        let cand_permanent = flags(w, u, cand).0;
        let better = is_better_than(w, new, cand);
        if cand_permanent && better {
            break;
        }
        i -= 1;
    }
    if let Some(x) = w.unit_mut(u) {
        x.body.empty_hand_stack.insert(i, new);
    }
}

/// `EquipEmptyHandedWeapon`.
fn equip_empty_handed_weapon(w: &mut World, u: EntityIdx, new: EntityIdx) {
    if let Some(old) = w.unit(u).and_then(|x| x.body.empty_hand) {
        crate::items::on_will_unequip(w, old);
    }
    if let Some(x) = w.unit_mut(u) {
        x.body.empty_hand = Some(new);
    }
    crate::items::on_did_equipped(w, new, u);
    raise_updated(w, u);
}

/// `IUnitEmptyHandWeaponHandler.HandleUnitEmptyHandWeaponUpdated` to the unit's subscribers.
fn raise_updated(w: &mut World, u: EntityIdx) {
    crate::rulebook::bus::raise_unit_fact(
        w,
        u,
        "IUnitEmptyHandWeaponHandler",
        "HandleUnitEmptyHandWeaponUpdated",
        None,
        None,
    );
}

/// `UnitBody.RemoveEmptyHandWeapon`.
pub fn remove_empty_hand_weapon(w: &mut World, u: EntityIdx, weapon: Option<EntityIdx>) -> bool {
    let cur = w.unit(u).and_then(|x| x.body.empty_hand);
    if cur == weapon {
        if w.unit(u).is_none_or(|x| x.body.empty_hand_stack.is_empty()) {
            w.log("error", "m_EmptyHandWeaponsStack is empty, can't remove empty hand weapon");
            return false;
        }
        if let Some(c) = cur {
            crate::items::on_will_unequip(w, c);
        }
        let last = w.unit(u).and_then(|x| x.body.empty_hand_stack.last().copied());
        if let Some(x) = w.unit_mut(u) {
            x.body.empty_hand = last;
        }
        if let Some(l) = last {
            crate::items::on_did_equipped(w, l, u);
        }
        if let Some(x) = w.unit_mut(u) {
            x.body.empty_hand_stack.pop();
        }
        raise_updated(w, u);
    }
    if let Some(wp) = weapon
        && let Some(x) = w.unit_mut(u)
    {
        // `List.Remove(weapon)`: the first match.
        if let Some(p) = x.body.empty_hand_stack.iter().position(|e| *e == wp) {
            x.body.empty_hand_stack.remove(p);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `DiceFormula.MaxValue(0)`: `Zero` and `One` count their rolls; otherwise at least 1.
    #[test]
    fn max_value_of_dice() {
        assert_eq!(max_value(DiceFormula::ZERO), 0);
        assert_eq!(max_value(DiceFormula::ONE), 1);
        assert_eq!(max_value(DiceFormula::new(1, 8)), 8);
        assert_eq!(max_value(DiceFormula::new(2, 6)), 12);
        assert_eq!(max_value(DiceFormula::new(0, 6)), 1, "no rolls: at least 1");
    }
}
