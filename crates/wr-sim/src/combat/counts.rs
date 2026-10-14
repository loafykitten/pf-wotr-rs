//! Attack counts (decompiled, not copied): `RuleCalculateAttacksCount` and
//! `RuleCalculateAttacksCountLimbs` (`Kingmaker.RuleSystem.Rules/`), and their handlers
//! `TwoWeaponFightingAttacks` (`Kingmaker.Designers.Mechanics.Facts/`),
//! `BuffExtraAttackWeaponSpecific` and `BuffExtraAttackForFastBombs`
//! (`Kingmaker.Designers.Mechanics.Buffs/`).
//!
//! The rules' optional inputs (`ForcePrimaryAttackWeapon`, `WeaponFilter`) are read from a
//! recorded snapshot only to report them: the roots the replay drives (the UI's weapon
//! tooltips and `UnitAttack.EnumerateFullAttack`) leave them unset.

use std::any::Any;

use serde_json::{Value as J, json};

use super::rules::rule_boilerplate;
use super::weapon::{self, Hand, Weapon};
use crate::components::mechanics::RulePort;
use crate::components::{fact_rank, owner_unit, with_settings};
use crate::registry::{Hook, Registry};
use crate::rulebook::{RuleBase, RuleEvent};
use crate::stats::types::StatType;
use crate::world::{CompRef, EntityIdx, World};

/// `MechanicsFeatureType` values read here.
const ITERATIVE_NATURAL_ATTACKS: i64 = 22;
const FLAWLESS_ATTACKS: i64 = 88;

/// `RuleCalculateAttacksCount.AttacksCount`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AttacksCount {
    pub additional: i32,
    pub penalized: i32,
    pub haste: i32,
    pub flawless: bool,
    pub two_handed: bool,
}

impl AttacksCount {
    /// As WrathOracle's field dumper writes it (`MainAttacks` is a property).
    #[must_use]
    pub fn to_json(self) -> J {
        json!({
            "AdditionalAttacks": self.additional,
            "PenalizedAttacks": self.penalized,
            "HasteAttacks": self.haste,
            "FlawlessAttacks": self.flawless,
            "IsTwoHanded": self.two_handed,
            "MainAttacks": self.additional + self.haste,
        })
    }
}

/// `CalculatePenalizedAttacksCount`: one iterative attack per 5 BAB past the first, at most 3
/// for a unit that isn't a companion.
fn penalized_attacks(w: &World, u: EntityIdx) -> i32 {
    let bab = w.stats(u).v(StatType::BaseAttackBonus).modified_value;
    let n = 0.max(bab / 5 - i32::from(bab % 5 == 0));
    let companion = w.entity(u).parts.iter().any(|p| p.clr == "UnitPartCompanion");
    if n > 3 && !companion { 3 } else { n }
}

/// `RuleCalculateAttacksCount`.
#[derive(Debug)]
pub struct RuleCalculateAttacksCount {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub force_iterative_natural: bool,
    pub primary: AttacksCount,
    pub secondary: AttacksCount,
}

impl RuleCalculateAttacksCount {
    #[must_use]
    pub fn new(initiator: EntityIdx) -> Self {
        Self {
            base: RuleBase::default(),
            initiator,
            force_iterative_natural: false,
            primary: AttacksCount::default(),
            secondary: AttacksCount::default(),
        }
    }

    /// From a recorded push snapshot; `ForcePrimaryAttackWeapon` and `WeaponFilter` are
    /// reported when set.
    pub fn from_snapshot(w: &mut World, initiator: EntityIdx, f: &J) -> Self {
        if !f["ForcePrimaryAttackWeapon"].is_null() || !f["WeaponFilter"].is_null() {
            w.coverage.unsupported(
                "RuleCalculateAttacksCount: ForcePrimaryAttackWeapon or WeaponFilter set",
            );
        }
        let mut r = Self::new(initiator);
        r.force_iterative_natural = f["ForceIterativeNaturalAttacks"].as_bool().unwrap_or(false);
        r
    }

    /// The projected result (`Result`).
    #[must_use]
    pub fn result(&self) -> Option<J> {
        Some(json!({"Result": {
            "PrimaryHand": self.primary.to_json(),
            "SecondaryHand": self.secondary.to_json(),
        }}))
    }
}

impl RuleCalculateAttacksCount {
    /// `AddExtraAttacks(amount, haste, penalized, weapon)`: both hands for one weapon in both
    /// that counts as double (reported: double weapons aren't modelled), the off hand for its own
    /// weapon, else the main hand. Two empty hands share the empty-hand weapon without being
    /// double.
    pub fn add_extra_attacks(
        &mut self,
        w: &mut World,
        amount: i32,
        haste: bool,
        penalized: bool,
        weapon: Option<EntityIdx>,
    ) {
        let u = self.initiator;
        let main_item = weapon::hand_weapon(w, u, Hand::Primary);
        let hands = w.unit(u).is_some_and(|x| !x.body.hands_disabled);
        let off_item = if hands { weapon::hand_weapon(w, u, Hand::Secondary) } else { None };
        let (main, off, _) = hand_weapons(w, u);
        let double = main_item.is_some()
            && main_item == off_item
            && main.as_ref().is_some_and(Weapon::count_as_double);
        if double {
            w.coverage.unsupported("RuleCalculateAttacksCount.AddExtraAttacks: a double weapon");
            add_for_hand(&mut self.primary, amount, haste, penalized);
            add_for_hand(&mut self.secondary, amount, haste, penalized);
            return;
        }
        let hand = if let Some(o) = &off
            && weapon.is_some()
            && weapon == off_item
            && (!o.is_unarmed() || main.as_ref().is_none_or(Weapon::is_unarmed))
        {
            &mut self.secondary
        } else {
            &mut self.primary
        };
        add_for_hand(hand, amount, haste, penalized);
    }
}

/// `AddExtraAttackForHand`.
fn add_for_hand(hand: &mut AttacksCount, amount: i32, haste: bool, penalized: bool) {
    if haste {
        hand.haste = hand.haste.max(amount);
    } else if penalized {
        hand.penalized += amount;
    } else {
        hand.additional += amount;
    }
}

/// `PrepareHandVariables`' weapons: the main hand's `MaybeWeapon`, and the off hand's (none when
/// the hands are disabled; a shield's weapon component only with Shield Bash, which
/// [`weapon::hand_weapon`] already answers).
fn hand_weapons(w: &mut World, u: EntityIdx) -> (Option<Weapon>, Option<Weapon>, bool) {
    let main = weapon::hand_weapon(w, u, Hand::Primary).and_then(|i| Weapon::of(w, i));
    let hands = w.unit(u).is_some_and(|x| !x.body.hands_disabled);
    let off = if hands {
        weapon::hand_weapon(w, u, Hand::Secondary).and_then(|i| Weapon::of(w, i))
    } else {
        None
    };
    (main, off, hands)
}

/// `HandSlot.HasShield` of the off hand.
fn off_hand_has_shield(w: &World, u: EntityIdx) -> bool {
    let Some(unit) = w.unit(u) else { return false };
    let name = format!("HandsEquipmentSets[{}].SecondaryHand", unit.body.current_hand_set);
    unit.body
        .slots
        .iter()
        .find(|s| s.name == name)
        .and_then(|s| s.item)
        .is_some_and(|i| w.entity(i).clr == "ItemEntityShield")
}

impl RuleEvent for RuleCalculateAttacksCount {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCalculateAttacksCount"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn on_trigger(&mut self, w: &mut World) {
        let u = self.initiator;
        let penalized = penalized_attacks(w, u);
        let (main, off, hands) = hand_weapons(w, u);
        let primary_two = match &main {
            Some(m) => weapon::hold_in_two_hands(w, m),
            None => false,
        };
        let secondary_two = match &off {
            Some(o) => weapon::hold_in_two_hands(w, o),
            None => false,
        };
        let flawless = weapon::has_feature(w, u, FLAWLESS_ATTACKS);
        let iterative_natural = weapon::has_feature(w, u, ITERATIVE_NATURAL_ATTACKS);
        // `CalculateMainHand` (no `ForcePrimaryAttackWeapon`: `num` and `flag` are false).
        self.primary.two_handed = primary_two;
        self.primary.flawless = flawless;
        if let Some(m) = &main {
            let shield_bash = hands
                && off_hand_has_shield(w, u)
                && weapon::has_feature(w, u, weapon::SHIELD_BASH);
            let off_unarmed = off.as_ref().is_none_or(Weapon::is_unarmed);
            if !secondary_two && (!m.is_unarmed() || off_unarmed || shield_bash) {
                self.primary.additional += 1;
                let flag2 = !m.is_natural()
                    || iterative_natural
                    || self.force_iterative_natural
                    || m.is_unarmed();
                if flag2 {
                    self.primary.penalized += penalized.max(0);
                }
            }
        }
        // `CalculateSecondaryHand`.
        self.secondary.two_handed = secondary_two;
        self.secondary.flawless = flawless;
        if let Some(o) = &off
            && !primary_two
            && (!o.is_unarmed() || main.is_none())
        {
            self.secondary.additional += 1;
            let flag = match &main {
                None => true,
                Some(m) => {
                    (m.is_unarmed() && !weapon::has_feature(w, u, weapon::IMPROVED_UNARMED_STRIKE))
                        || (m.is_natural() && (iterative_natural || self.force_iterative_natural))
                }
            };
            if flag {
                self.secondary.penalized += penalized.max(0);
            }
        }
    }
    rule_boilerplate!();
}

/// `RuleCalculateAttacksCountLimbs`.
#[derive(Debug)]
pub struct RuleCalculateAttacksCountLimbs {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    /// `Slots`: each counted limb slot, by its `ParentFact`'s id (`None`: a limb no
    /// `AddAdditionalLimb` filled).
    pub slots: Vec<Option<String>>,
    pub counts: Vec<AttacksCount>,
}

impl RuleCalculateAttacksCountLimbs {
    pub fn from_snapshot(w: &mut World, initiator: EntityIdx, f: &J) -> Self {
        if !f["ForcePrimaryAttackWeapon"].is_null() || !f["WeaponFilter"].is_null() {
            w.coverage.unsupported(
                "RuleCalculateAttacksCountLimbs: ForcePrimaryAttackWeapon or WeaponFilter set",
            );
        }
        Self { base: RuleBase::default(), initiator, slots: Vec::new(), counts: Vec::new() }
    }

    /// The projected result: the slots' parent facts (as [`super::project`] reduces `Slots`)
    /// and the counts.
    #[must_use]
    pub fn result(&self) -> Option<J> {
        Some(json!({
            "Slots": self.slots,
            "AttacksCounts": self.counts.iter().map(|c| c.to_json()).collect::<Vec<_>>(),
        }))
    }
}

impl RuleEvent for RuleCalculateAttacksCountLimbs {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCalculateAttacksCountLimbs"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn on_trigger(&mut self, w: &mut World) {
        let u = self.initiator;
        let weapons = weapon::limb_weapons(w, u);
        let items: Vec<Option<EntityIdx>> =
            w.unit(u).map(|x| x.body.limbs.iter().map(|s| s.item).collect()).unwrap_or_default();
        // `HasWeapon` (`HasItem` and a `MaybeWeapon`) and `HasItem`; without
        // `ForcePrimaryAttackWeapon` a limb counts one attack.
        for (i, (wp, item)) in weapons.into_iter().zip(items).enumerate() {
            let (Some(_), Some(_)) = (wp, item) else { continue };
            let parent = w.unit(u).and_then(|x| x.body.limb_parents.get(&i).copied());
            if parent.is_none() {
                w.coverage.unsupported("RuleCalculateAttacksCountLimbs: a limb's ParentFact");
            }
            self.slots.push(parent.map(|f| w.fact(f).unique_id.clone()));
            self.counts.push(AttacksCount { additional: 1, ..AttacksCount::default() });
        }
    }
    rule_boilerplate!();
}

fn no_hooks(_: &mut World, _: CompRef, _: Hook) {}

type RuleFn = fn(&mut World, CompRef, &mut dyn RuleEvent, bool);

pub fn register(r: &mut Registry) {
    for (name, rule) in [
        ("TwoWeaponFightingAttacks", two_weapon_fighting_attacks as RuleFn),
        ("BuffExtraAttackWeaponSpecific", buff_extra_attack_weapon_specific),
        ("BuffExtraAttackForFastBombs", buff_extra_attack_for_fast_bombs),
    ] {
        r.register(Box::new(RulePort {
            name,
            bits: 0,
            hooks: no_hooks,
            handlers: &["IInitiatorRulebookHandler<RuleCalculateAttacksCount>"],
            rule,
        }));
    }
}

/// `WeaponRangeTypeHelper.IsSuitableAttackType` (`Kingmaker.Enums/`); `None` for a value out
/// of the enum (the C# throws).
fn is_suitable_attack_type(range: i64, t: weapon::AttackType) -> Option<bool> {
    use weapon::AttackType as A;
    Some(match range {
        0 => matches!(t, A::Melee | A::Touch),
        1 => matches!(t, A::Ranged | A::RangedTouch),
        2 => matches!(t, A::Touch | A::RangedTouch),
        3 => matches!(t, A::Melee | A::Ranged),
        4 => t == A::Melee,
        5 => t == A::Ranged,
        6 => t == A::Touch,
        7 => t == A::RangedTouch,
        _ => return None,
    })
}

/// `BuffExtraAttackWeaponSpecific.OnEventAboutToTrigger`: one extra attack with the primary
/// hand's weapon when it suits `RangeType`.
fn buff_extra_attack_weapon_specific(
    w: &mut World,
    c: CompRef,
    rule: &mut dyn RuleEvent,
    about: bool,
) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateAttacksCount>() else { return };
    let Some(u) = owner_unit(w, c) else { return };
    let Some(range) = with_settings(w, c, |v| v.int("RangeType")) else { return };
    let Some(item) = weapon::hand_weapon(w, u, Hand::Primary) else { return };
    let Some(wp) = Weapon::of(w, item) else { return };
    match is_suitable_attack_type(range, wp.attack_type()) {
        Some(true) => evt.add_extra_attacks(w, 1, false, false, Some(item)),
        Some(false) => {}
        None => w.coverage.unsupported("BuffExtraAttackWeaponSpecific: RangeType out of range"),
    }
}

/// `BuffExtraAttackForFastBombs.OnEventAboutToTrigger`: extra attacks only inside
/// `ContextData<FastBombs.FastBombsContext>`, which only `FastBombs`' own ability execution
/// requests (not ported): outside it, nothing.
fn buff_extra_attack_for_fast_bombs(_: &mut World, _: CompRef, _: &mut dyn RuleEvent, _: bool) {}

/// `TwoWeaponFightingAttacks.OnEventAboutToTrigger`: with two weapons, ranks 3 and 4 each add a
/// penalized off-hand attack. `OnEventDidTrigger` is empty.
fn two_weapon_fighting_attacks(w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateAttacksCount>() else { return };
    let Some(u) = owner_unit(w, c) else { return };
    if !weapon::check_has_two_weapon(w, u) {
        return;
    }
    let rank = fact_rank(w, c.fact);
    if rank > 2 {
        evt.secondary.penalized += 1;
    }
    if rank > 3 {
        evt.secondary.penalized += 1;
    }
}

#[cfg(test)]
mod tests {
    use wr_content::Guid;
    use wr_content::node::Node;

    use super::*;
    use crate::content::{Blueprint, Source};
    use crate::facts::tests::obj;

    fn insert(w: &World, g: u128, ty: &str, set: &[(&str, Node)]) {
        w.content.insert(Blueprint {
            guid: Guid(g),
            name: format!("B{g}"),
            obj: obj(ty, set),
            source: Source::Bbp,
        });
    }

    /// Both hands empty: both `MaybeWeapon`s are the one empty-hand weapon, which doesn't count as
    /// double, so an extra attack with it goes to the off hand (`!mainWeapon || mainWeapon.IsUnarmed`).
    #[test]
    fn an_extra_attack_with_two_empty_hands_goes_to_the_off_hand() {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        insert(
            &w,
            0xC01,
            "Kingmaker.Blueprints.Items.Weapons.BlueprintWeaponType",
            &[("m_IsUnarmed", Node::Bool(true))],
        );
        insert(
            &w,
            0xC02,
            "Kingmaker.Blueprints.Items.Weapons.BlueprintItemWeapon",
            &[("m_Type", Node::BpRef(Guid(0xC01)))],
        );
        let fist = weapon::create_weapon(&mut w, Guid(0xC02), u);
        {
            let body = &mut w.unit_mut(u).expect("unit").body;
            body.empty_hand = Some(fist);
            for hand in ["PrimaryHand", "SecondaryHand"] {
                body.slots.push(crate::unit::Slot {
                    name: format!("HandsEquipmentSets[0].{hand}"),
                    item: None,
                    active: true,
                });
            }
        }
        let mut r = RuleCalculateAttacksCount::new(u);
        r.add_extra_attacks(&mut w, 1, false, false, Some(fist));
        assert_eq!((r.primary.additional, r.secondary.additional), (0, 1));
        assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
        // A haste attack keeps the larger count; a penalized one adds.
        r.add_extra_attacks(&mut w, 1, true, false, None);
        r.add_extra_attacks(&mut w, 2, true, false, None);
        r.add_extra_attacks(&mut w, 1, false, true, None);
        assert_eq!((r.primary.haste, r.primary.penalized), (2, 1));
    }
}
