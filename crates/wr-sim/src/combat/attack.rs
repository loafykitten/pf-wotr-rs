//! The weapon attack rule chain (decompiled, not copied; files under
//! `Kingmaker.RuleSystem.Rules/` unless noted): `RuleAttackWithWeapon`,
//! `RuleCalculateWeaponStats`, `RuleCalculateWeaponSizeBonus`, `RuleAttackRoll`,
//! `RuleConcealmentCheck`, `RuleCalculateAC`, `RuleCheckTargetFlatFooted`,
//! `RuleCalculateAttackBonus`, `RuleCalculateAttackBonusWithoutTarget`,
//! `RuleAttackWithWeaponResolve`, and `Kingmaker.RuleSystem.Rules.Damage/`
//! `RuleDealDamage`, `RulePrepareDamage`, `RuleCalculateDamage`, `BaseDamage`,
//! `DamageDescription`, `DamageValue`; `ModifiableBonus`/`Modifier` for rule bonuses;
//! `ModifiableValueArmorClass.SelectMissReason`.
//!
//! Branches the sim doesn't model report unsupported where they would run: projectiles
//! (ranged weapons), sneak attack, precise strike, fortification, mirror images, parry,
//! concealment and miss chances, damage reduction and immunities, temporary hit points,
//! army (tactical) combat is never active here.

use std::any::Any;

use serde_json::{Value as J, json};

use super::rules::{rule_boilerplate, trigger_any};
use super::units::{self, AttackEnv};
use super::weapon::{self, AttackType, DiceFormula, ModifiableDiceFormula, Weapon};
use crate::content::View;
use crate::csharp::arith;
use crate::rng::{self, sites};
use crate::rulebook::{RuleBase, RuleEvent};
use crate::rules::checks::RuleRollDice;
use crate::stats::ops::attribute_bonus;
use crate::stats::types::{ModifierDescriptor as MD, StatType};
use crate::world::{EntityIdx, World};

/// `AttackResult`.
pub mod result {
    pub const UNKNOWN: i32 = 0;
    pub const HIT: i32 = 1;
    pub const CRITICAL_HIT: i32 = 2;
    pub const MISS: i32 = 3;
    pub const DODGE_AC: i32 = 4;
    pub const ARMOR_AC: i32 = 5;
    pub const SHIELD_AC: i32 = 6;
    pub const MIRROR_IMAGE: i32 = 7;
    pub const CONCEALMENT: i32 = 8;
    pub const PARRIED: i32 = 9;
}

/// `AttackResultExtensions.IsHit`.
#[must_use]
pub fn is_hit_result(r: i32) -> bool {
    r == result::HIT || r == result::CRITICAL_HIT
}

/// `Kingmaker.RuleSystem.Rules.Modifier` (a rule bonus).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bonus {
    pub value: i32,
    pub descriptor: MD,
    pub stat: StatType,
    /// `BonusType` (`None` 0, `Flanking`, `Concealment`, …) as its name's tag.
    pub bonus_type: &'static str,
    /// `Fact`: the source fact a component's bonus names (`AddModifier(value, Fact, ...)`).
    pub fact: Option<crate::world::FactIdx>,
}

impl Bonus {
    #[must_use]
    pub fn desc(value: i32, descriptor: MD) -> Self {
        Self { value, descriptor, stat: StatType::Unknown, bonus_type: "None", fact: None }
    }
    /// `new Modifier(value, fact, descriptor)`: this bonus of `fact`.
    #[must_use]
    pub fn of(self, fact: crate::world::FactIdx) -> Self {
        Self { fact: Some(fact), ..self }
    }
    /// `new Modifier(value, stat)`: descriptor `None`.
    #[must_use]
    pub fn stat(value: i32, stat: StatType) -> Self {
        Self { value, descriptor: MD::None, stat, bonus_type: "None", fact: None }
    }
    /// `new Modifier(value, type, descriptor)`.
    #[must_use]
    pub fn typed(value: i32, bonus_type: &'static str, descriptor: MD) -> Self {
        Self { value, descriptor, stat: StatType::Unknown, bonus_type, fact: None }
    }
}

/// `ModifiableBonus`: stackable bonuses append; a non-stackable one replaces the first
/// with its descriptor only when larger. Every addition asked for is kept too, in order
/// (`RulebookEvent.AddModifier`'s calls, as WrathOracle's `rule_bonus` records them), with the
/// rulebook's next rule number when it was asked for (by [`Self::add_in`]).
#[derive(Debug, Clone, Default)]
pub struct BonusList(pub Vec<Bonus>, pub Vec<(Bonus, Option<u64>)>);

impl BonusList {
    /// A rule's `AddModifier(modifier)`: [`Self::add`], noting where among the rules it happened.
    pub fn add_in(&mut self, w: &World, m: Bonus) -> bool {
        let added = self.add(m);
        if let Some(last) = self.1.last_mut() {
            last.1 = Some(w.rulebook.next_seq);
        }
        added
    }

    /// `Add(modifier)`: whether it was added.
    pub fn add(&mut self, m: Bonus) -> bool {
        self.1.push((m, None));
        if m.descriptor.is_stackable() {
            self.0.push(m);
            return true;
        }
        for x in &mut self.0 {
            if x.descriptor == m.descriptor {
                if x.value < m.value {
                    *x = m;
                    return true;
                }
                return false;
            }
        }
        self.0.push(m);
        true
    }

    /// `TotalValue`.
    #[must_use]
    pub fn total(&self) -> i32 {
        self.0.iter().fold(0, |a, m| arith::add(a, m.value))
    }

    /// The modifiers and the additions asked for, as [`RuleEvent::all_bonuses`] lists them.
    #[must_use]
    pub fn as_t0(&self, w: &World, seq: u64) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
        let one = |m: &Bonus| {
            let fact = m.fact.map(|f| w.fact(f).unique_id.clone());
            serde_json::json!([
                m.value,
                m.descriptor.0,
                bonus_type_index(m.bonus_type),
                m.stat.0,
                fact
            ])
        };
        let list = self.0.iter().map(one).collect();
        // An addition's place: the rules pushed after its rule (`seq`) before it.
        let adds = self
            .1
            .iter()
            .map(|(m, at)| {
                let mut x = one(m);
                if let Some(a) = x.as_array_mut() {
                    a.push(serde_json::json!(at.map(|at| at.saturating_sub(seq + 1))));
                }
                x
            })
            .collect();
        (list, adds)
    }
}

/// `BonusType`'s value by name (`None` 0, `Concealment`, `Flanking`, `SecondaryWeapon`,
/// `ShootIntoCombat`, `Blindness`, `TargetStunned`).
fn bonus_type_index(name: &str) -> i32 {
    match name {
        "Concealment" => 1,
        "Flanking" => 2,
        "SecondaryWeapon" => 3,
        "ShootIntoCombat" => 4,
        "Blindness" => 5,
        "TargetStunned" => 6,
        _ => 0,
    }
}

/// `EnhancementValue` (`Kingmaker.RuleSystem.Rules/EnhancementValue.cs`): the weapon's
/// enhancement as the handlers add it, clamped by the upper limit.
#[derive(Debug, Clone)]
pub struct EnhancementValue {
    upper_limit: i32,
    bonus: BonusList,
    extra: BonusList,
}

impl Default for EnhancementValue {
    fn default() -> Self {
        Self { upper_limit: 5, bonus: BonusList::default(), extra: BonusList::default() }
    }
}

impl EnhancementValue {
    /// `AddModifier(modifier)`.
    pub fn add_modifier(&mut self, m: Bonus) -> bool {
        self.bonus.add(m)
    }
    /// `AddExtraModifier(modifier)`.
    pub fn add_extra_modifier(&mut self, m: Bonus) -> bool {
        self.extra.add(m)
    }
    /// `TotalValue` (`Calculate`).
    #[must_use]
    pub fn total(&self) -> i32 {
        let stack: Vec<i32> =
            self.bonus.0.iter().filter(|m| m.descriptor.is_stackable()).map(|m| m.value).collect();
        let single: Vec<i32> =
            self.bonus.0.iter().filter(|m| !m.descriptor.is_stackable()).map(|m| m.value).collect();
        let num2 = stack.iter().fold(0, |a, v| arith::add(a, *v));
        let num = single.iter().copied().max().unwrap_or(0);
        let clamped =
            if num >= self.upper_limit { num } else { arith::add(num, num2).min(self.upper_limit) };
        arith::add(clamped, self.extra.total())
    }
}

/// `DamageTypeDescription` (physical and energy members the sim reads).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DamageTypeDesc {
    /// `DamageType`: Physical 0, Energy 1, Force 2, Direct 3, Untyped 4.
    pub ty: i64,
    pub reality: i64,
    pub alignment: i64,
    pub precision: bool,
    pub material: i64,
    pub form: i64,
    pub enhancement: i32,
    pub enhancement_total: i32,
    pub energy: i64,
}

impl DamageTypeDesc {
    #[must_use]
    pub fn from_view(v: Option<View<'_>>) -> Self {
        let Some(v) = v else { return Self::default() };
        let common = v.obj("Common");
        let phys = v.obj("Physical");
        Self {
            ty: v.int("Type"),
            reality: common.map_or(0, |c| c.int("Reality")),
            alignment: common.map_or(0, |c| c.int("Alignment")),
            precision: common.is_some_and(|c| c.bool("Precision")),
            material: phys.map_or(0, |p| p.int("Material")),
            form: phys.map_or(0, |p| p.int("Form")),
            enhancement: phys.map_or(0, |p| p.i32("Enhancement")),
            enhancement_total: phys.map_or(0, |p| p.i32("EnhancementTotal")),
            energy: v.int("Energy"),
        }
    }
}

/// `DamageDescription`.
#[derive(Debug, Clone, Default)]
pub struct DamageDescription {
    pub dice: DiceFormula,
    /// `m_DiceModifiers` (`SetDice`): the weapon's modifiable formula.
    pub dice_modifiers: Option<ModifiableDiceFormula>,
    pub bonus: i32,
    pub type_desc: DamageTypeDesc,
    pub modifiers: BonusList,
}

impl DamageDescription {
    /// `AddModifier(bonus)`: `Bonus` always grows; the list may refuse.
    pub fn add_modifier(&mut self, m: Bonus) {
        self.bonus = arith::add(self.bonus, m.value);
        self.modifiers.add(m);
    }

    /// `CreateDamage()`.
    pub fn create_damage(&self, w: &mut World) -> BaseDamage {
        let num = arith::sub(self.bonus, self.modifiers.total());
        let dice =
            self.dice_modifiers.clone().unwrap_or_else(|| ModifiableDiceFormula::new(self.dice));
        if self.type_desc.ty != 0 {
            w.coverage.unsupported(format!(
                "DamageTypeDescription.CreateDamage: type {}",
                self.type_desc.ty
            ));
        }
        let mut d = BaseDamage::new(self.type_desc, dice, num);
        for m in &self.modifiers.0 {
            d.add_modifier(*m);
        }
        d
    }
}

/// `BaseDamage` (a `PhysicalDamage` here).
#[derive(Debug, Clone)]
pub struct BaseDamage {
    pub type_desc: DamageTypeDesc,
    pub dice: ModifiableDiceFormula,
    pub bonus: i32,
    pub bonus_target_related: i32,
    pub modifiers: BonusList,
    pub precision: bool,
    /// `Sneak`: a sneak attack's damage.
    pub sneak: bool,
    pub critical_modifier: Option<i32>,
    pub tactical_critical_modifier: f32,
    pub empower: f32,
    pub half: bool,
    pub already_halved: bool,
    pub pre_rolled: Option<i32>,
    pub bonus_percent: i32,
    pub vulnerability: f32,
    pub durability: f32,
    pub ignore_reduction: bool,
    pub ignore_immunities: bool,
    /// `m_ImmunityDeclineType` (`DamageDeclineType.Total` 3 by default).
    pub immunity_decline: i32,
    /// `Declines` types.
    pub declines: Vec<i32>,
    /// `CalculationType`: Normal 0, Maximized 1, Minimized 2.
    pub calculation_type: i32,
    pub maximum: Option<i32>,
    /// `CriticalApplied` bits.
    pub critical_applied: i32,
    pub reduction_because_resistance: i32,
    pub reduction_because_hardness: i32,
    pub reduction_penalty: BonusList,
}

impl BaseDamage {
    #[must_use]
    pub fn new(type_desc: DamageTypeDesc, dice: ModifiableDiceFormula, bonus: i32) -> Self {
        Self {
            type_desc,
            dice,
            bonus,
            bonus_target_related: 0,
            modifiers: BonusList::default(),
            precision: type_desc.precision,
            sneak: false,
            critical_modifier: None,
            tactical_critical_modifier: 1.0,
            empower: 1.0,
            half: false,
            already_halved: false,
            pre_rolled: None,
            bonus_percent: 0,
            vulnerability: 1.0,
            durability: 1.0,
            ignore_reduction: false,
            ignore_immunities: false,
            immunity_decline: 3,
            declines: Vec::new(),
            calculation_type: 0,
            maximum: None,
            critical_applied: 0,
            reduction_because_resistance: 0,
            reduction_because_hardness: 0,
            reduction_penalty: BonusList::default(),
        }
    }

    /// `AddModifier(bonus)`: `Bonus` grows only when the list takes it.
    pub fn add_modifier(&mut self, m: Bonus) {
        if self.modifiers.add(m) {
            self.bonus = arith::add(self.bonus, m.value);
        }
    }

    /// `AddModifierTargetRelated(bonus, fact, descriptor)`: the list takes it, and
    /// `BonusTargetRelated` grows instead of `Bonus`.
    pub fn add_modifier_target_related(&mut self, m: Bonus) {
        if self.modifiers.add(m) {
            self.bonus_target_related = arith::add(self.bonus_target_related, m.value);
        }
    }

    /// `Decline` (`GetDecline`): no physical immunities (`TargetImmunities`) or declines
    /// are added by ported handlers, so `None` unless a decline was recorded.
    #[must_use]
    pub fn decline(&self) -> i32 {
        self.declines.iter().copied().max().unwrap_or(0).max(0)
    }

    /// `Immune`.
    #[must_use]
    pub fn immune(&self) -> bool {
        self.decline() >= 3
    }

    /// `DecreaseDecline()`.
    pub fn decrease_decline(&mut self) {
        for d in &mut self.declines {
            // `DamageDecline.Decrease()`: one step towards `None` (not `Absolute`).
            if *d > 0 && *d < 4 {
                *d -= 1;
            }
        }
        if self.immunity_decline > 0 {
            self.immunity_decline -= 1;
        }
    }

    /// `ResetDecline()`.
    pub fn reset_decline(&mut self) {
        for d in &mut self.declines {
            if *d != 4 {
                *d = 0;
            }
        }
        self.immunity_decline = 0;
    }
}

/// `DamageValue`.
#[derive(Debug, Clone)]
pub struct DamageValue {
    pub source: BaseDamage,
    pub value_without_reduction: i32,
    pub roll_and_bonus: i32,
    pub roll_result: i32,
}

impl DamageValue {
    /// `Reduction` (no tactical DR modifier: 1).
    #[must_use]
    pub fn reduction(&self) -> i32 {
        if self.source.ignore_reduction {
            return 0;
        }
        let r = arith::sub(
            self.source.reduction_because_resistance,
            self.source.reduction_penalty.total(),
        );
        // `(float)r * TacticalCombatDRModifier + (float)hardness` is stored into
        // `Mathf.RoundToInt`'s float parameter.
        #[allow(clippy::cast_precision_loss)]
        let f = arith::store_f32(
            f64::from(r as f32) * 1.0 + f64::from(self.source.reduction_because_hardness as f32),
        );
        crate::csharp::arith::round_to_int(f).max(0)
    }
    /// `FinalValue`.
    #[must_use]
    pub fn final_value(&self) -> i32 {
        let max = self.source.maximum.unwrap_or(i32::MAX);
        max.min(arith::sub(self.value_without_reduction, self.reduction())).max(0)
    }
}

/// Fires a d20 (`Dice.D20` / `Dice.GenerateD20`): a `RuleRollD20` of the current rule's
/// initiator.
pub(crate) fn d20(w: &mut World, initiator: EntityIdx) -> RuleRollDice {
    let mut r = RuleRollDice::d20(initiator);
    trigger_any(w, &mut r, |r| Some(json!({"Result": r.result()})));
    r
}

// ---------------------------------------------------------------------------------------
// RuleCalculateWeaponStats, RuleCalculateWeaponSizeBonus
// ---------------------------------------------------------------------------------------

/// `RuleCalculateWeaponStats`.
#[derive(Debug)]
pub struct RuleCalculateWeaponStats {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub weapon: Weapon,
    /// The attack's `IsCharge` when created by `RuleAttackWithWeapon`.
    pub attack_is_charge: Option<bool>,
    pub damage_description: Vec<DamageDescription>,
    pub damage_modifiers: BonusList,
    pub size_shift: i32,
    pub damage_bonus_stat: Option<StatType>,
    pub damage_bonus_stat_multiplier: f32,
    pub is_charge_weapon: bool,
    pub is_secondary: bool,
    pub is_secondary_override: Option<bool>,
    pub override_damage_bonus: Option<f32>,
    pub additional_damage_bonus_stat_multiplier: f32,
    pub weapon_damage_dice: ModifiableDiceFormula,
    /// `Enhancement`.
    pub enhancement: EnhancementValue,
    pub enhancement_total: i32,
    pub additional_critical_multiplier: i32,
    pub critical_edge_bonus: i32,
    pub double_critical_edge: bool,
    pub two_handed_stat_replacement: bool,
    pub half_damage_bonus: bool,
    pub do_not_scale_damage: bool,
    pub alignment: i64,
    pub triggered: bool,
}

impl RuleCalculateWeaponStats {
    #[must_use]
    pub fn new(initiator: EntityIdx, weapon: Weapon, attack_is_charge: Option<bool>) -> Self {
        let melee = weapon.attack_type() == AttackType::Melee;
        let dice = ModifiableDiceFormula::new(weapon.base_damage());
        Self {
            base: RuleBase::default(),
            initiator,
            weapon,
            attack_is_charge,
            damage_description: vec![DamageDescription::default()],
            damage_modifiers: BonusList::default(),
            size_shift: 0,
            damage_bonus_stat: melee.then_some(StatType::Strength),
            damage_bonus_stat_multiplier: 1.0,
            is_charge_weapon: false,
            is_secondary: false,
            is_secondary_override: None,
            override_damage_bonus: None,
            additional_damage_bonus_stat_multiplier: 0.0,
            weapon_damage_dice: dice,
            enhancement: EnhancementValue::default(),
            enhancement_total: 0,
            additional_critical_multiplier: 0,
            critical_edge_bonus: 0,
            double_critical_edge: false,
            two_handed_stat_replacement: false,
            half_damage_bonus: false,
            do_not_scale_damage: false,
            alignment: 0,
            triggered: false,
        }
    }

    /// `CriticalMultiplier`.
    #[must_use]
    pub fn critical_multiplier(&self) -> i32 {
        arith::add(self.weapon.critical_modifier(), self.additional_critical_multiplier).max(2)
    }
    /// `CriticalRange`.
    #[must_use]
    pub fn critical_range(&self) -> i32 {
        let m = if self.double_critical_edge { 2 } else { 1 };
        arith::add(arith::mul(21 - self.weapon.critical_roll_edge(), m), self.critical_edge_bonus)
    }
    /// `CriticalEdge`.
    #[must_use]
    pub fn critical_edge(&self) -> i32 {
        21 - self.critical_range()
    }
}

/// `IsSecondaryWeapon(weapon)`.
fn is_secondary_weapon(w: &mut World, wp: &Weapon) -> bool {
    if wp.always_primary() {
        return false;
    }
    // `RuleCalculateWeaponStats.cs:197`: the saved `ForceSecondary` flag.
    if w.item(wp.item).is_some_and(|i| i.force_secondary) {
        return true;
    }
    let Some(wielder) = w.item(wp.item).and_then(|i| i.wielder) else { return false };
    if !wp.is_natural() || wp.is_unarmed() {
        return false;
    }
    let p = weapon::hand_weapon(w, wielder, weapon::Hand::Primary).and_then(|i| Weapon::of(w, i));
    let s = weapon::hand_weapon(w, wielder, weapon::Hand::Secondary).and_then(|i| Weapon::of(w, i));
    match p {
        None => s.is_some_and(|s| !s.is_natural()),
        Some(p) if p.is_natural() => s.is_some_and(|s| !s.is_natural()),
        Some(_) => true,
    }
}

impl RuleEvent for RuleCalculateWeaponStats {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCalculateWeaponStats"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn on_trigger(&mut self, w: &mut World) {
        self.triggered = true;
        let wielder = w.item(self.weapon.item).and_then(|i| i.wielder);
        if wielder != Some(self.initiator) {
            w.coverage.unsupported("RuleCalculateWeaponStats: ApplyEnchantmentsManually");
        }
        if self.attack_is_charge == Some(true) {
            w.coverage.unsupported("RuleCalculateWeaponStats: charge weapon");
        }
        let sec = is_secondary_weapon(w, &self.weapon);
        self.is_secondary |= sec && !self.is_charge_weapon;
        if let Some(o) = self.is_secondary_override {
            self.is_secondary = o;
        }
        let flag = self.damage_bonus_stat.is_none_or(|s| s == StatType::Strength);
        let flag2 = self.damage_bonus_stat == Some(StatType::Dexterity);
        if let Some(wu) = wielder
            && self.weapon.is_natural()
            && !self.weapon.is_unarmed()
            && flag
        {
            let mw = weapon::hand_weapon(w, wu, weapon::Hand::Primary);
            let mw2 = weapon::hand_weapon(w, wu, weapon::Hand::Secondary);
            let iu = weapon::has_feature(w, wu, weapon::IMPROVED_UNARMED_STRIKE);
            let flag3 = mw.is_none_or(|i| Weapon::of(w, i).is_some_and(|x| x.is_unarmed()) && !iu);
            let flag4 = mw2.is_none_or(|i| Weapon::of(w, i).is_some_and(|x| x.is_unarmed()));
            let flag5 = flag4 & flag3;
            let num = (mw == Some(self.weapon.item)) & flag4;
            let flag6 = (mw2 == Some(self.weapon.item)) & flag3;
            let limbs = weapon::limb_weapons(w, wu);
            let flag7 = limbs.iter().all(Option::is_none);
            let flag8 = limbs.iter().filter(|l| **l == Some(self.weapon.item)).count() == 1;
            if (num & flag7) || (flag6 & flag7) || (flag5 & flag8) {
                self.damage_bonus_stat_multiplier = 1.5;
            }
        }
        if weapon::hold_in_two_hands(w, &self.weapon)
            && (flag || (flag2 && self.two_handed_stat_replacement))
        {
            self.damage_bonus_stat_multiplier = 1.5;
        }
        if self.damage_bonus_stat.is_some() && (self.is_secondary || self.half_damage_bonus) {
            self.damage_bonus_stat_multiplier = 0.5;
        }
        if let Some(o) = self.override_damage_bonus {
            self.damage_bonus_stat_multiplier = o;
        }
        let mut stat = self.damage_bonus_stat.unwrap_or(StatType::Unknown);
        let mut num2 = 0;
        if let Some(s) = self.damage_bonus_stat {
            let num3 =
                self.damage_bonus_stat_multiplier + self.additional_damage_bonus_stat_multiplier;
            let b = attribute_bonus(w, self.initiator, s);
            // Mono: the product stays in double up to the `(int)` cast.
            #[allow(clippy::cast_precision_loss)]
            let v =
                arith::to_int(f64::from(b as f32) * f64::from(if b < 0 { 1.0_f32 } else { num3 }));
            num2 = arith::add(num2, v);
        } else if matches!(
            self.weapon.category(),
            weapon::CATEGORY_SHORTBOW | weapon::CATEGORY_LONGBOW
        ) {
            stat = StatType::Strength;
            let b = attribute_bonus(w, self.initiator, StatType::Strength);
            if b < 0 {
                num2 = arith::add(num2, b);
            }
        }
        let mut sb = RuleCalculateWeaponSizeBonus {
            base: RuleBase::default(),
            initiator: self.initiator,
            weapon: self.weapon.clone(),
            dice: self.weapon_damage_dice.clone(),
            size_shift: self.size_shift,
            do_not_scale: self.do_not_scale_damage,
        };
        trigger_any(w, &mut sb, |_| None);
        self.weapon_damage_dice = sb.dice;
        // `DamageType.GetDamageDescriptor(WeaponDamageDice, AdditionalDamage.BaseValue)`.
        let add_base = w.stats(self.initiator).v(StatType::AdditionalDamage).base_value;
        let mut d = DamageDescription {
            dice: self.weapon_damage_dice.base,
            dice_modifiers: Some(self.weapon_damage_dice.clone()),
            bonus: add_base,
            type_desc: DamageTypeDesc::from_view(self.weapon.damage_type()),
            modifiers: BonusList::default(),
        };
        d.type_desc.enhancement = self.enhancement.total();
        // `Weapon.EnchantmentValue`: enchantment bonuses of the item.
        let ench_value = item_enchantment_value(w, self.weapon.item);
        d.type_desc.enhancement_total = arith::add(self.enhancement_total, ench_value);
        let add_mods: Vec<_> =
            w.stats(self.initiator).v(StatType::AdditionalDamage).modifiers().collect();
        for m in add_mods {
            let (v, desc) = (w.mods[m.i()].value, w.mods[m.i()].desc);
            d.add_modifier(Bonus::desc(v, desc));
        }
        d.add_modifier(Bonus::stat(num2, stat));
        d.add_modifier(Bonus::desc(self.enhancement.total(), MD::Enhancement));
        d.type_desc.alignment = self.alignment;
        for m in self.damage_modifiers.0.clone() {
            d.add_modifier(m);
        }
        self.damage_description[0] = d;
    }
    rule_boilerplate!();
}

/// `ItemEntityWeapon.EnchantmentValue` (`GameHelper.GetItemEnhancementBonus`): items with
/// enchantment facts report unsupported unless they carry no enhancement.
fn item_enchantment_value(w: &mut World, item: EntityIdx) -> i32 {
    let facts = w.entity(item).facts.list.clone();
    let mut v = 0;
    for f in facts {
        let Some(bp) = w.fact(f).bp.clone() else { continue };
        for o in bp.components_array() {
            let n = wr_content::schema::ty(o.ty).short;
            if n == "WeaponEnhancementBonus" || n == "ArmorEnhancementBonus" {
                v = arith::add(v, View(o).i32("EnhancementBonus"));
            }
        }
    }
    v
}

/// `RuleCalculateWeaponSizeBonus`.
#[derive(Debug)]
pub struct RuleCalculateWeaponSizeBonus {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub weapon: Weapon,
    pub dice: ModifiableDiceFormula,
    pub size_shift: i32,
    pub do_not_scale: bool,
}

impl RuleEvent for RuleCalculateWeaponSizeBonus {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCalculateWeaponSizeBonus"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn on_trigger(&mut self, w: &mut World) {
        // `IsDefaultUnit` (`Game.DefaultUnit`) never attacks here; polymorph reported.
        if w.entity(self.initiator).parts.iter().any(|p| p.clr == "UnitPartPolymorphed") {
            w.coverage.unsupported("RuleCalculateWeaponSizeBonus: polymorphed");
        }
        let player = units::is_player_faction(w, self.initiator);
        if !self.do_not_scale
            && (self.dice.has_modifications() || !self.weapon.damage_dice_overridden() || player)
        {
            let size = weapon::shift(weapon::weapon_size(w, &self.weapon), self.size_shift);
            let cur = self.dice.modified();
            let f = weapon::scale(w, cur, size, crate::stats::types::Size::Medium);
            if f != cur {
                self.dice.modify(f, MD::Size);
            }
        }
    }
    rule_boilerplate!();
}

// ---------------------------------------------------------------------------------------
// RuleAttackRoll and its children
// ---------------------------------------------------------------------------------------

/// `RuleConcealmentCheck` (concealment `None` only: a nonzero driven concealment is
/// reported).
#[derive(Debug)]
pub struct RuleConcealmentCheck {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub concealment: i32,
    pub env_concealment: i32,
}

impl RuleConcealmentCheck {
    /// `Success`.
    #[must_use]
    pub fn success(&self) -> bool {
        true
    }
}

impl RuleEvent for RuleConcealmentCheck {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleConcealmentCheck", "RulebookTargetEvent"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn target(&self) -> Option<EntityIdx> {
        Some(self.target)
    }
    fn on_trigger(&mut self, w: &mut World) {
        self.concealment = self.env_concealment;
        if self.concealment != 0 {
            w.coverage.unsupported("RuleConcealmentCheck: concealment (d100 miss chance)");
        }
    }
    rule_boilerplate!();
}

/// `RuleCheckTargetFlatFooted`.
#[derive(Debug)]
pub struct RuleCheckTargetFlatFooted {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub force_flat_footed: bool,
    pub env: AttackEnv,
    pub is_flat_footed: bool,
}

impl RuleEvent for RuleCheckTargetFlatFooted {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCheckTargetFlatFooted", "RulebookTargetEvent"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn target(&self) -> Option<EntityIdx> {
        Some(self.target)
    }
    fn on_trigger(&mut self, w: &mut World) {
        use crate::unit_state::{cond, has_condition};
        use units::condition as c;
        let t = self.target;
        // Shaken/frightened targets with the attacker's Shatter Defenses.
        let shatter = (has_condition(w, t, cond::SHAKEN) || has_condition(w, t, cond::FRIGHTENED))
            && weapon::has_feature(w, self.initiator, 36);
        self.is_flat_footed = self.force_flat_footed
            || !self.env.target_can_act_in_combat
            || units::is_helpless(w, t)
            || has_condition(w, t, cond::STUNNED)
            || !self.env.target_remembers_initiator
            || self.env.concealment_initiator == 2
            || has_condition(w, t, c::LOSE_DEXTERITY_TO_AC)
            || shatter;
    }
    rule_boilerplate!();
}

/// `RuleCalculateAC`.
#[derive(Debug)]
pub struct RuleCalculateAC {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub attack_type: AttackType,
    pub force_flat_footed: bool,
    pub is_critical: bool,
    pub env: AttackEnv,
    pub is_target_flat_footed: bool,
    pub result: i32,
    pub bonuses: BonusList,
}

impl RuleEvent for RuleCalculateAC {
    fn all_bonuses(&self, w: &World) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
        self.bonuses.as_t0(w, self.base.seq)
    }
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCalculateAC", "RulebookTargetEvent"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn target(&self) -> Option<EntityIdx> {
        Some(self.target)
    }
    fn on_trigger(&mut self, w: &mut World) {
        use crate::unit_state::has_condition;
        let t = self.target;
        // Rule bonuses (`AddModifier`): kept on the rule, applied to the AC in a temporary
        // scope below. Prone reads the posture (`State.Prone.Active`), then, only when that
        // is false, `View.IsGetUp` (not the condition: they differ while falling or standing
        // up). The view's get-up state comes from the environment; reported when missing.
        let prone_active = w.unit(t).is_some_and(|x| x.prone_active);
        let prone = prone_active
            || match self.env.target_is_get_up {
                Some(g) => g,
                None => {
                    w.coverage.unsupported("UnitEntityView.IsGetUp: not in the attack environment");
                    false
                }
            };
        if prone {
            self.bonuses.add_in(
                w,
                Bonus::desc(if self.attack_type.is_melee() { -4 } else { 4 }, MD::Prone),
            );
        }
        if units::is_helpless(w, t) && self.attack_type.is_melee() {
            self.bonuses.add_in(w, Bonus::desc(-4, MD::Helpless));
        }
        if has_condition(w, t, units::condition::BLINDNESS) {
            self.bonuses.add_in(w, Bonus::typed(-2, "Blindness", MD::Penalty));
        }
        // `using (AC.GetTemporaryModifiersScope(AllBonuses))` (RuleCalculateAC.cs:56–75):
        // the flat-footed check and the AC read happen with the rule's bonuses applied.
        let rule_mods: Vec<_> =
            self.bonuses.0.iter().map(|b| (b.value, b.descriptor, None)).collect();
        let scope = crate::stats::ops::open_temporary_scope(w, t, StatType::AC, &rule_mods);
        let mut ff = RuleCheckTargetFlatFooted {
            base: RuleBase::default(),
            initiator: self.initiator,
            target: t,
            force_flat_footed: self.force_flat_footed,
            env: self.env.clone(),
            is_flat_footed: false,
        };
        trigger_any(w, &mut ff, |r| Some(json!({"IsFlatFooted": r.is_flat_footed})));
        self.is_target_flat_footed = ff.is_flat_footed;
        let s = w.stats(t).v(StatType::AC);
        if let crate::stats::value::StatKind::ArmorClass(ac) = &s.kind {
            self.result = match (self.is_target_flat_footed, self.attack_type.is_touch()) {
                (true, true) => ac.flat_footed_touch,
                (true, false) => ac.flat_footed,
                (false, true) => ac.touch,
                (false, false) => s.modified_value,
            };
        } else {
            w.coverage.unsupported("RuleCalculateAC: AC stat kind");
        }
        crate::stats::ops::close_temporary_scope(w, t, StatType::AC, &scope);
    }
    rule_boilerplate!();
}

/// `RuleCalculateAttackBonusWithoutTarget`.
#[derive(Debug)]
pub struct RuleCalculateAttackBonusWithoutTarget {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub weapon: Weapon,
    pub is_secondary: bool,
    pub weapon_enhancement: i32,
    pub penalty: i32,
    /// `AttackBonusStat` (handlers may replace it).
    pub attack_bonus_stat: StatType,
    pub bonuses: BonusList,
    pub result: i32,
    /// `Reason.Rule as RuleAttackWithWeapon`: `Some(IsFullAttack)` when the attack bonus is
    /// part of a weapon attack, `None` for another reason.
    pub reason_full_attack: Option<bool>,
}

impl RuleEvent for RuleCalculateAttackBonusWithoutTarget {
    fn all_bonuses(&self, w: &World) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
        self.bonuses.as_t0(w, self.base.seq)
    }
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCalculateAttackBonusWithoutTarget"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn on_trigger(&mut self, w: &mut World) {
        let u = self.initiator;
        let bab = w.stats(u).v(StatType::BaseAttackBonus).modified_value;
        self.bonuses
            .add_in(w, Bonus::stat(arith::sub(bab, self.penalty), StatType::BaseAttackBonus));
        // `ContextData<AttackBonusStatReplacement>.Current`: no such scope on these paths.
        let bonus = if self.attack_bonus_stat != StatType::Unknown {
            if self.attack_bonus_stat.is_attribute() {
                attribute_bonus(w, u, self.attack_bonus_stat)
            } else {
                0
            }
        } else if self.weapon.is_ranged() {
            self.attack_bonus_stat = StatType::Dexterity;
            attribute_bonus(w, u, StatType::Dexterity)
        } else {
            self.attack_bonus_stat = StatType::Strength;
            attribute_bonus(w, u, StatType::Strength)
        };
        self.bonuses.add_in(w, Bonus::stat(bonus, self.attack_bonus_stat));
        let no_secondary = self.weapon.is_natural() && weapon::has_feature(w, u, 90);
        if self.is_secondary && !no_secondary {
            let b = if self.weapon.is_natural() && weapon::has_feature(w, u, 93) { -2 } else { -5 };
            self.bonuses.add_in(w, Bonus::typed(b, "SecondaryWeapon", MD::Penalty));
        }
        // `AdditionalAttackBonus.ModifiedValueWithDisabledBonuses()`.
        let s = w.stats(u).v(StatType::AdditionalAttackBonus);
        let disabled = s.morale_disabled > 0 || s.insight_disabled > 0;
        let num = s.modified_value;
        if disabled {
            w.coverage.unsupported("AdditionalAttackBonus with disabled bonuses");
        }
        // `(int)WeaponStats.Enhancement > 0`: the enhancement is added unless the stat's own
        // display list already has a larger or equal non-stacking enhancement modifier (which
        // it removes). The display list's choice among several enhancement modifiers is not
        // modelled: any nonzero one is reported.
        if self.weapon_enhancement > 0 {
            let adds: Vec<_> = w.stats(u).v(StatType::AdditionalAttackBonus).modifiers().collect();
            if adds
                .iter()
                .any(|m| w.mods[m.i()].desc == MD::Enhancement && w.mods[m.i()].value != 0)
            {
                w.coverage.unsupported(
                    "RuleCalculateAttackBonusWithoutTarget: AdditionalAttackBonus enhancement modifier",
                );
            } else {
                self.bonuses.add_in(w, Bonus::desc(self.weapon_enhancement, MD::Enhancement));
            }
        }
        self.result = arith::add(num, self.bonuses.total());
    }
    rule_boilerplate!();
}

/// `RuleCalculateAttackBonus`.
#[derive(Debug)]
pub struct RuleCalculateAttackBonus {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub inner: RuleCalculateAttackBonusWithoutTarget,
    pub target_is_flanked: bool,
    pub ignore_ranged_penalty: bool,
    pub env: AttackEnv,
    pub bonuses: BonusList,
    pub result: i32,
}

impl RuleEvent for RuleCalculateAttackBonus {
    fn all_bonuses(&self, w: &World) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
        self.bonuses.as_t0(w, self.base.seq)
    }
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCalculateAttackBonus", "RulebookTargetEvent"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn target(&self) -> Option<EntityIdx> {
        Some(self.target)
    }
    fn on_trigger(&mut self, w: &mut World) {
        let blinking = weapon::has_feature(w, self.initiator, 59);
        if blinking {
            w.coverage.unsupported("RuleCalculateAttackBonus: Blinking attacker");
        }
        if self.env.concealment_initiator == 2 {
            self.bonuses.add_in(w, Bonus::typed(2, "Concealment", MD::UntypedStackable));
        }
        if (self.env.target_flanked || self.target_is_flanked) && self.inner.weapon.is_melee() {
            self.bonuses.add_in(w, Bonus::typed(2, "Flanking", MD::UntypedStackable));
        }
        if self.inner.weapon.is_ranged() && !self.ignore_ranged_penalty {
            // `foreach unit in Game.Instance.State.Units`: an ally of the attacker, not the
            // attacker or the target, that the target is engaged with either way. The
            // recording has the units engaging the target (`EngagedBy`); the other direction
            // (`Target.CombatState.EngagedUnits`) is not recorded.
            let mut found = false;
            match self.env.target_engaged_by.clone() {
                Some(list) => {
                    for u in list {
                        if u != self.initiator
                            && u != self.target
                            && !units::is_enemy(w, u, self.initiator)
                        {
                            found = true;
                            break;
                        }
                    }
                }
                None => {
                    w.coverage
                        .unsupported("RuleCalculateAttackBonus: shoot into combat (engagement)");
                }
            }
            if found {
                self.bonuses.add_in(w, Bonus::typed(-4, "ShootIntoCombat", MD::Penalty));
            } else if self.env.target_engaged_by.is_some() {
                w.coverage.unsupported(
                    "RuleCalculateAttackBonus: shoot into combat (Target.EngagedUnits not recorded)",
                );
            }
        }
        trigger_any(w, &mut self.inner, |r| Some(json!({"Result": r.result})));
        self.result = arith::add(self.inner.result, self.bonuses.total());
        let player = units::is_player_faction(w, self.initiator);
        if player && w.settings.dead_companions_rise_after_combat {
            let bab = w.stats(self.initiator).v(StatType::BaseAttackBonus).modified_value;
            let num = (bab - self.inner.penalty - self.result - 2).max(0);
            self.result = arith::add(self.result, num);
            if num > 0 {
                self.bonuses.add_in(w, Bonus::desc(num, MD::Difficulty));
            }
        }
    }
    rule_boilerplate!();
}

/// `RuleAttackRoll`.
#[derive(Debug)]
pub struct RuleAttackRoll {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub weapon: Weapon,
    pub is_secondary: bool,
    pub enhancement: i32,
    pub critical_edge: i32,
    pub penalty: i32,
    pub attack_type: AttackType,
    pub auto_hit: bool,
    pub auto_miss: bool,
    pub auto_critical_threat: bool,
    pub auto_critical_confirmation: bool,
    pub force_flat_footed: bool,
    pub target_is_flanked: bool,
    pub do_not_provoke: bool,
    pub immune_to_critical_hit: bool,
    pub ignore_crit_immunity: bool,
    pub immune_to_sneak_attack: bool,
    pub precise_strike: i32,
    /// `Reason.Rule as RuleAttackWithWeapon`: `Some(IsFullAttack)`.
    pub reason_full_attack: Option<bool>,
    /// An explicit environment; `None` takes the driven queue's as `OnTrigger` starts.
    pub env: Option<AttackEnv>,
    // results
    pub result: i32,
    pub d20: i32,
    pub attack_bonus: i32,
    pub target_ac: i32,
    pub target_critical_ac: i32,
    pub is_target_flat_footed: bool,
    pub is_critical_roll: bool,
    pub is_critical_confirmed: bool,
    pub critical_confirmation_d20: i32,
    pub is_sneak_attack: bool,
}

impl RuleAttackRoll {
    /// `IsHit`.
    #[must_use]
    pub fn is_hit(&self) -> bool {
        !self.auto_miss && is_hit_result(self.result)
    }
    /// `Roll`.
    #[must_use]
    pub fn roll(&self) -> i32 {
        arith::add(self.d20, self.attack_bonus)
    }
    /// `IsSuccessRoll(d20)`.
    fn is_success_roll(&self, w: &World, d20: i32) -> bool {
        if d20 > 1 || weapon::has_feature(w, self.initiator, 61) {
            if d20 != 20 {
                return arith::add(d20, self.attack_bonus) >= self.target_ac;
            }
            return true;
        }
        false
    }
    /// The projected result.
    #[must_use]
    pub fn result_json(&self) -> J {
        json!({"Result": self.result, "D20": self.d20, "AttackBonus": self.attack_bonus,
               "TargetAC": self.target_ac, "IsTargetFlatFooted": self.is_target_flat_footed,
               "Roll": self.roll(), "IsCriticalRoll": self.is_critical_roll,
               "IsCriticalConfirmed": self.is_critical_confirmed, "IsHit": self.is_hit()})
    }
}

/// `ModifiableValueArmorClass.SelectMissReason(flatfooted, touch)`.
fn select_miss_reason(w: &mut World, t: EntityIdx, flat: bool, touch: bool) -> i32 {
    let s = w.stats(t).v(StatType::AC);
    let (base, dodge, armor, shield) = match &s.kind {
        crate::stats::value::StatKind::ArmorClass(ac) => {
            (s.base_value, ac.dodge_ac, ac.armor_ac, ac.shield_ac)
        }
        _ => (s.base_value, 0, 0, 0),
    };
    let mut num = base;
    if !flat {
        num = arith::add(num, dodge);
    }
    if !touch {
        num = arith::add(num, arith::add(armor, shield));
    }
    let mut n2 = w.rng.range_int(sites::AC_SELECT_MISS_REASON, 0, num);
    if !flat {
        if n2 < dodge {
            return result::DODGE_AC;
        }
        n2 -= dodge;
    }
    if !touch {
        if n2 < armor {
            return result::ARMOR_AC;
        }
        n2 -= armor;
        if n2 < shield {
            return result::SHIELD_AC;
        }
    }
    result::MISS
}

impl RuleEvent for RuleAttackRoll {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleAttackRoll", "RulebookTargetEvent"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn target(&self) -> Option<EntityIdx> {
        Some(self.target)
    }
    fn on_trigger(&mut self, w: &mut World) {
        use crate::unit_state::has_condition;
        let (a, t) = (self.initiator, self.target);
        // The environment as `OnTrigger` starts: WrathOracle records it there, after the
        // about-to-trigger subscribers (whose nested attacks take theirs first), once per roll.
        let env = match self.env.take() {
            Some(e) => e,
            None => units::take_env(w, a, t),
        };
        self.env = Some(env.clone());
        let confused = has_condition(w, t, units::condition::CONFUSION);
        let flag2 = !units::is_enemy(w, t, a) && !units::faction_neutral(w, t) && !confused;
        if a == t || (self.attack_type.is_touch() & flag2) {
            self.auto_hit = true;
        }
        if self.auto_hit {
            self.result = result::HIT;
            self.is_critical_confirmed =
                self.auto_critical_threat && self.auto_critical_confirmation;
            if w.stats(a).v(StatType::SneakAttack).modified_value > 0
                && !self.immune_to_sneak_attack
            {
                w.coverage.unsupported("RuleAttackRoll: auto-hit sneak attack");
            }
            self.is_sneak_attack = false;
        } else if self.auto_miss {
            self.result = result::MISS;
        } else {
            // `TryOvercomeTargetConcealmentAndMissChance`: no miss chance modifiers on these
            // paths; the concealment check runs.
            let mut cc = RuleConcealmentCheck {
                base: RuleBase::default(),
                initiator: a,
                target: t,
                concealment: 0,
                env_concealment: env.concealment_target,
            };
            trigger_any(w, &mut cc, |r| {
                Some(json!({"Concealment": r.concealment, "Success": r.success()}))
            });
            if !cc.success() {
                self.result = result::CONCEALMENT;
            } else {
                let mut ac = RuleCalculateAC {
                    base: RuleBase::default(),
                    initiator: a,
                    target: t,
                    attack_type: self.attack_type,
                    force_flat_footed: self.force_flat_footed,
                    is_critical: false,
                    env: env.clone(),
                    is_target_flat_footed: false,
                    result: 0,
                    bonuses: BonusList::default(),
                };
                trigger_any(w, &mut ac, |r| {
                    Some(json!({"Result": r.result, "IsTargetFlatFooted": r.is_target_flat_footed}))
                });
                self.is_target_flat_footed = ac.is_target_flat_footed;
                self.target_ac = ac.result;
                let mut ab = RuleCalculateAttackBonus {
                    base: RuleBase::default(),
                    initiator: a,
                    target: t,
                    inner: RuleCalculateAttackBonusWithoutTarget {
                        base: RuleBase::default(),
                        initiator: a,
                        weapon: self.weapon.clone(),
                        is_secondary: self.is_secondary,
                        weapon_enhancement: self.enhancement,
                        penalty: self.penalty,
                        attack_bonus_stat: self.weapon.attack_bonus_stat(),
                        bonuses: BonusList::default(),
                        result: 0,
                        reason_full_attack: self.reason_full_attack,
                    },
                    target_is_flanked: self.target_is_flanked,
                    ignore_ranged_penalty: false,
                    env: env.clone(),
                    bonuses: BonusList::default(),
                    result: 0,
                };
                trigger_any(w, &mut ab, |r| Some(json!({"Result": r.result})));
                self.attack_bonus = ab.result;
                let d = d20(w, a);
                self.d20 = d.result();
                let success = self.is_success_roll(w, self.d20);
                self.result = if success {
                    result::HIT
                } else {
                    select_miss_reason(
                        w,
                        t,
                        self.is_target_flat_footed,
                        self.attack_type.is_touch(),
                    )
                };
                // `IsSneakAttack = IsHit && !ImmuneToSneakAttack && IsAttackRollSuitableForSneakAttack()
                // && (IsTargetFlatFooted || Target.CombatState.IsFlanked) && SneakAttack > 0`;
                // a weapon attack's reason has no mechanics context (suitable).
                let sneak_stat = w.stats(a).v(StatType::SneakAttack).modified_value;
                self.is_sneak_attack = self.is_hit()
                    && !self.immune_to_sneak_attack
                    && (self.is_target_flat_footed || env.target_flanked)
                    && sneak_stat > 0;
                let crits = units::enemy_critical_hits(w);
                let target_player = units::is_player_faction(w, t);
                self.is_critical_roll = success
                    && (!self.immune_to_critical_hit || self.ignore_crit_immunity)
                    && (self.d20 >= self.critical_edge || self.auto_critical_threat)
                    && (!target_player || crits == units::CRIT_WEAK || crits == units::CRIT_NORMAL);
                if self.is_critical_roll {
                    let mut cac = RuleCalculateAC {
                        base: RuleBase::default(),
                        initiator: a,
                        target: t,
                        attack_type: self.attack_type,
                        force_flat_footed: false,
                        is_critical: true,
                        env: env.clone(),
                        is_target_flat_footed: false,
                        result: 0,
                        bonuses: BonusList::default(),
                    };
                    trigger_any(w, &mut cac, |r| {
                        Some(
                            json!({"Result": r.result, "IsTargetFlatFooted": r.is_target_flat_footed}),
                        )
                    });
                    self.target_critical_ac = cac.result;
                    if self.auto_critical_confirmation {
                        // `new RuleRollD20(Initiator, 20)`: constructed, not triggered.
                        self.critical_confirmation_d20 = 20;
                    } else {
                        self.critical_confirmation_d20 = d20(w, a).result();
                    }
                    // `CriticalConfirmationRoll` = d20 + AttackBonus + CriticalConfirmationBonus.
                    let roll = arith::add(self.critical_confirmation_d20, self.attack_bonus);
                    self.is_critical_confirmed =
                        self.auto_critical_confirmation || roll >= self.target_critical_ac;
                }
                if (self.is_sneak_attack || self.is_critical_confirmed || self.precise_strike > 0)
                    && w.entity(t).parts.iter().any(|p| p.clr == "UnitPartFortification")
                {
                    w.coverage.unsupported("RuleAttackRoll: fortification");
                }
                if self.is_critical_confirmed {
                    self.result = result::CRITICAL_HIT;
                }
                let near_miss = !success && self.target_ac - self.d20 - self.attack_bonus <= 5;
                if (success | near_miss)
                    && w.entity(t).parts.iter().any(|p| p.clr == "UnitPartMirrorImage")
                {
                    w.coverage.unsupported("RuleAttackRoll: mirror image");
                }
            }
        }
        // Parry (`Parry != null`) is set only by parry abilities: not on these paths.
        self.is_sneak_attack &= self.is_hit();
        crate::rulebook::bus::raise_global(w, "IAttackHandler", Some(a));
    }
    rule_boilerplate!();
}

// ---------------------------------------------------------------------------------------
// Damage
// ---------------------------------------------------------------------------------------

/// What a damage rule knows of the attack that caused it (`Reason.Rule`, `AttackRoll`).
#[derive(Debug, Clone, Default)]
pub struct AttackLink {
    pub is_critical_confirmed: bool,
    pub is_sneak_attack: bool,
    /// `RuleAttackRoll.IsSneakAttackUsed`, shared by every damage rule of the attack roll.
    pub sneak_used: std::rc::Rc<std::cell::Cell<bool>>,
    pub precise_strike: i32,
    pub weapon_attack: bool,
}

/// `RulePrepareDamage`.
#[derive(Debug)]
pub struct RulePrepareDamage {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub link: AttackLink,
    pub disable_sneak: bool,
    /// The parent rule's damage bundle (`DamageBundle`), lent for the rule; `Add(damage)`
    /// appends to it.
    pub bundle: Vec<BaseDamage>,
    /// `DamageBundle.Weapon`.
    pub weapon_item: Option<EntityIdx>,
    /// The weapon's category (`DamageBundle.Weapon.Blueprint.Category`).
    pub weapon_category: Option<i64>,
}

impl RuleEvent for RulePrepareDamage {
    fn chain(&self) -> &'static [&'static str] {
        &["RulePrepareDamage", "RulebookTargetEvent"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn target(&self) -> Option<EntityIdx> {
        Some(self.target)
    }
    fn on_trigger(&mut self, w: &mut World) {
        let u = self.initiator;
        if has_part_named(w, u, "UnitPartDisableBonusForDamage") {
            w.coverage.unsupported("RulePrepareDamage: UnitPartDisableBonusForDamage");
        }
        let sneak = w.stats(u).v(StatType::SneakAttack).modified_value;
        // `IsSurpriseSpell` is false on weapon attacks.
        if sneak > 0
            && !self.disable_sneak
            && self.link.weapon_attack
            && self.link.is_sneak_attack
            && !self.link.sneak_used.get()
        {
            // The sneak attack die: d6; Knife Master: d8 with a knife, else d4.
            let mut dice = 6;
            if weapon::has_feature(w, u, 15) {
                let knife = self
                    .weapon_category
                    .is_some_and(|c| crate::feature_param::has_sub_category(c, 6));
                dice = if knife { 8 } else { 4 };
            }
            if has_part_named(w, u, "UnitPartSneakAttackModifications") {
                w.coverage.unsupported("RulePrepareDamage: UnitPartSneakAttackModifications");
            }
            let type_desc =
                self.bundle.first().map_or_else(DamageTypeDesc::default, |f| f.type_desc);
            let d = DamageDescription {
                dice: DiceFormula::new(sneak, dice),
                dice_modifiers: None,
                bonus: 0,
                type_desc,
                modifiers: BonusList::default(),
            };
            let mut dmg = d.create_damage(w);
            dmg.precision = true;
            dmg.sneak = true;
            self.bundle.push(dmg);
            self.link.sneak_used.set(true);
        }
        if self.link.precise_strike > 0 {
            w.coverage.unsupported("RulePrepareDamage: precise strike");
        }
    }
    rule_boilerplate!();
}

fn has_part_named(w: &World, u: EntityIdx, clr: &str) -> bool {
    w.entity(u).parts.iter().any(|p| p.clr == clr)
}

/// `RuleCalculateDamage`.
#[derive(Debug)]
pub struct RuleCalculateDamage {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub bundle: Vec<BaseDamage>,
    pub disable_precision: bool,
    pub half: bool,
    pub already_halved: bool,
    pub half_because_saving_throw: bool,
    pub modifier_bonus: Option<f32>,
    pub calculated: Vec<DamageValue>,
}

impl RuleCalculateDamage {
    fn calculate_value(&mut self, w: &mut World, mut damage: BaseDamage) -> DamageValue {
        if damage.precision {
            // `ResetBonus()`.
            damage.bonus = 0;
            damage.bonus_target_related = 0;
        }
        let crits = units::enemy_critical_hits(w);
        let mut num = damage.critical_modifier.unwrap_or(1);
        if damage.ignore_immunities {
            damage.reset_decline();
        }
        // `CritImmunity` comes from handlers (none ported).
        let _ = &mut num;
        let target_player = units::is_player_faction(w, self.target);
        let num2 = if target_player && crits == units::CRIT_OFF { 1 } else { num };
        let num3 = if target_player
            && crits != units::CRIT_NORMAL
            && arith::add(damage.bonus, damage.bonus_target_related) >= 0
        {
            1
        } else {
            num
        };
        damage.critical_applied = i32::from(num2 > 1) | (i32::from(num3 > 1) << 1);
        let num4 = damage.empower;
        let num5;
        let mut num6: f32;
        if let Some(p) = damage.pre_rolled {
            num5 = p;
            #[allow(clippy::cast_precision_loss)]
            {
                num6 = num5 as f32;
            }
        } else {
            if num2 > 1 {
                let m = damage.dice.modified();
                damage.dice.modify_critical(DiceFormula::new(arith::mul(m.rolls(), num2), m.dice));
            }
            // `AddPostCritDiceModifications`: no post-crit increments on these paths.
            let f = damage.dice.modified();
            num5 = roll_damage(w, f, 1, damage.calculation_type);
            // One float store for the whole expression (Mono evaluates it in double).
            #[allow(clippy::cast_precision_loss)]
            {
                let roll = arith::add(num5, arith::mul(arith::mul(damage.bonus, 1), num3)) as f32;
                num6 = arith::store_f32(
                    f64::from(roll)
                        * f64::from(num4)
                        * f64::from(damage.tactical_critical_modifier),
                );
            }
        }
        let roll_and_bonus = arith::to_int(f64::from(num6));
        #[allow(clippy::cast_precision_loss)]
        {
            let target_related = arith::mul(damage.bonus_target_related, 1) as f32;
            num6 = arith::store_f32(
                f64::from(num6)
                    + f64::from(target_related) * f64::from(num4) * f64::from(num3 as f32),
            );
        }
        if damage.half && !damage.already_halved {
            num6 /= 2.0;
        }
        if self.half && !self.already_halved {
            num6 /= 2.0;
        }
        if self.half_because_saving_throw {
            w.coverage.unsupported("RuleCalculateDamage: half because of a saving throw");
            num6 *= 0.5;
        }
        match damage.decline() {
            1 => num6 *= 0.75,
            2 => num6 *= 0.5,
            _ => {}
        }
        num6 = num6.max(1.0);
        // `num8` and `num9` are float locals (one store each); the `Math.Floor` argument
        // `num8 * num9` is not stored, so it stays a double product.
        #[allow(clippy::cast_precision_loss)]
        let num8 = arith::store_f32(
            f64::from(num6)
                * (1.0 + f64::from(damage.bonus_percent as f32) / 100.0)
                * f64::from(damage.vulnerability)
                * f64::from(damage.durability)
                * 1.0,
        );
        let num9 = arith::store_f32(1.0 + f64::from(self.modifier_bonus.unwrap_or(0.0)));
        let mut value = arith::to_int((f64::from(num8) * f64::from(num9)).floor()).max(1);
        // `TryApplyShadowDamageFactor`: no shadow context on these paths.
        if damage.immune() {
            value = 0;
        }
        DamageValue {
            source: damage,
            value_without_reduction: value,
            roll_and_bonus,
            roll_result: num5,
        }
    }
}

/// `RuleCalculateDamage.Roll(formula, unitsCount, calculationType)`.
fn roll_damage(w: &mut World, f: DiceFormula, units_count: i32, ct: i32) -> i32 {
    if f == DiceFormula::ZERO {
        return 0;
    }
    match ct {
        1 => arith::mul(arith::mul(f.rolls(), f.dice), units_count),
        2 => arith::mul(f.rolls(), units_count),
        _ => rng::dice(&mut *w.rng, f.rolls(), f.dice),
    }
}

impl RuleEvent for RuleCalculateDamage {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCalculateDamage", "RulebookTargetEvent"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn target(&self) -> Option<EntityIdx> {
        Some(self.target)
    }
    fn on_trigger(&mut self, w: &mut World) {
        if self.disable_precision {
            self.bundle.retain(|d| !d.precision);
        }
        // `ApplyDifficultyBeforeCalculated`.
        let sa = units::stats_adjustments(w);
        let decline_setting = matches!(sa, 0..=2);
        if units::is_players_enemy(w, self.target) && decline_setting {
            for d in &mut self.bundle {
                if d.type_desc.ty != 3 {
                    if sa == 0 { d.reset_decline() } else { d.decrease_decline() }
                }
            }
        }
        let bundle = std::mem::take(&mut self.bundle);
        let mut out = Vec::new();
        for d in bundle {
            let v = self.calculate_value(w, d);
            out.push(v);
        }
        self.calculated.splice(0..0, out);
        // `IUnitApplyDamageReduction` to the target: damage reduction parts.
        if w.entity(self.target).parts.iter().any(|p| p.clr == "UnitPartDamageReduction") {
            w.coverage.unsupported("RuleCalculateDamage: damage reduction");
        }
        crate::rulebook::bus::raise_unit(w, self.target, "IUnitApplyDamageReduction");
        // `ApplyDifficultyForDamageReduction`: no resistance or hardness here.
        if units::is_players_enemy(w, self.target) && decline_setting {
            for v in &mut self.calculated {
                let s = &mut v.source;
                if s.type_desc.ty == 3 {
                    continue;
                }
                let num = arith::add(s.reduction_because_resistance, s.reduction_because_hardness);
                if sa == 0 {
                    s.reduction_penalty.add(Bonus::desc(num, MD::Difficulty));
                } else if s.reduction_because_resistance > 0 {
                    #[allow(clippy::cast_precision_loss)]
                    let p = crate::csharp::arith::ceil_to_int(num as f32 / 2.0);
                    s.reduction_penalty.add(Bonus::desc(p, MD::Difficulty));
                }
            }
        }
    }
    rule_boilerplate!();
}

/// `RuleDealDamage`.
#[derive(Debug)]
pub struct RuleDealDamage {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub bundle: Vec<BaseDamage>,
    pub link: AttackLink,
    pub disable_precision: bool,
    pub is_fake: bool,
    pub result: i32,
    pub raw_result: i32,
    pub damage_without_reduction: i32,
    pub damage_before_difficulty: i32,
    pub difficulty_modifier: f32,
    /// `DamageBundle.Weapon.Blueprint.IsRanged` (`None`: no weapon).
    pub weapon_ranged: Option<bool>,
    /// `DamageBundle.Weapon.Blueprint.Category`.
    pub weapon_category: Option<i64>,
    /// `DamageBundle.Weapon`.
    pub weapon_item: Option<EntityIdx>,
    /// The attacker's and the target's positions when the attack was rolled.
    pub positions: Option<([f32; 3], [f32; 3])>,
}

impl RuleDealDamage {
    #[must_use]
    pub fn new(
        initiator: EntityIdx,
        target: EntityIdx,
        bundle: Vec<BaseDamage>,
        link: AttackLink,
    ) -> Self {
        Self {
            base: RuleBase::default(),
            initiator,
            target,
            bundle,
            link,
            disable_precision: false,
            is_fake: false,
            result: 0,
            raw_result: 0,
            damage_without_reduction: 0,
            damage_before_difficulty: 0,
            difficulty_modifier: 1.0,
            weapon_ranged: None,
            weapon_category: None,
            weapon_item: None,
            positions: None,
        }
    }

    /// `ApplyDifficultyModifiers(damage)`.
    fn apply_difficulty(&mut self, w: &mut World, damage: i32) -> i32 {
        if units::is_player_faction(w, self.initiator) {
            self.difficulty_modifier = 1.0;
        } else if units::is_players_enemy(w, self.initiator) {
            self.difficulty_modifier = units::damage_to_party(w);
        }
        if damage > 0 {
            // `(int)((float)damage * DifficultyModifier)`: the product has no float store
            // before the cast (Mono: 10 * 0.9f truncates to 8, not 9).
            #[allow(clippy::cast_precision_loss)]
            let v = arith::to_int(f64::from(damage as f32) * f64::from(self.difficulty_modifier));
            return v.max(1);
        }
        0
    }
}

impl RuleEvent for RuleDealDamage {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleDealDamage", "RulebookTargetEvent"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn target(&self) -> Option<EntityIdx> {
        Some(self.target)
    }
    fn on_trigger(&mut self, w: &mut World) {
        // `UnitCommand.CommandTargetUntargetable`: untargetable parts reported.
        if w.entity(self.target).parts.iter().any(|p| p.clr == "UnitPartUntargetable") {
            w.coverage.unsupported("RuleDealDamage: untargetable target");
        }
        if !self.is_fake && self.bundle.is_empty() {
            return;
        }
        // `GameHelper.IsAttackingGreenNPC`: a player attacking a non-enemy, non-player NPC.
        if !self.is_fake
            && units::is_player_faction(w, self.initiator)
            && !units::is_player_faction(w, self.target)
            && !units::is_enemy(w, self.initiator, self.target)
        {
            w.coverage.unsupported("RuleDealDamage: attacking a green NPC");
            return;
        }
        let mut prep = RulePrepareDamage {
            base: RuleBase::default(),
            initiator: self.initiator,
            target: self.target,
            link: self.link.clone(),
            disable_sneak: false,
            bundle: std::mem::take(&mut self.bundle),
            weapon_item: self.weapon_item,
            weapon_category: self.weapon_category,
        };
        trigger_any(w, &mut prep, |_| None);
        self.bundle = prep.bundle;
        let mut calc = RuleCalculateDamage {
            base: RuleBase::default(),
            initiator: self.initiator,
            target: self.target,
            bundle: self.bundle.clone(),
            disable_precision: self.disable_precision,
            half: false,
            already_halved: false,
            half_because_saving_throw: false,
            modifier_bonus: None,
            calculated: Vec::new(),
        };
        trigger_any(w, &mut calc, |_| Some(json!({})));
        let mut direct = 0;
        for v in &calc.calculated {
            self.raw_result = arith::add(self.raw_result, v.roll_and_bonus);
            let f = v.final_value();
            self.result = arith::add(self.result, f);
            self.damage_without_reduction =
                arith::add(self.damage_without_reduction, v.value_without_reduction);
            if v.source.type_desc.ty == 3 {
                direct = arith::add(direct, f);
            }
        }
        // `MinHPAfterDamage` is unset on these paths.
        self.damage_before_difficulty = self.result;
        let nd = self.apply_difficulty(w, self.result - direct);
        self.result = arith::add(nd, direct);
        if !self.is_fake {
            let thp = w.stats(self.target).v(StatType::TemporaryHitPoints).modified_value;
            if thp > 0 {
                w.coverage.unsupported("RuleDealDamage: temporary hit points");
            }
            let cur = w.unit(self.target).map_or(0, |u| u.damage);
            super::set_damage(w, self.target, arith::add(cur, self.result));
        }
        crate::rulebook::bus::raise_global(w, "IDamageHandler", Some(self.target));
    }
    rule_boilerplate!();
}

/// `RuleAttackWithWeaponResolve`.
#[derive(Debug)]
pub struct RuleAttackWithWeaponResolve {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub damage: Option<RuleDealDamage>,
    pub attack_hit: bool,
    pub weapon_melee: bool,
    /// The recorded id of the `RuleAttackWithWeapon` that launched it (a replay's identity).
    pub source_id: Option<i64>,
}

impl RuleEvent for RuleAttackWithWeaponResolve {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleAttackWithWeaponResolve", "RulebookTargetEvent"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn target(&self) -> Option<EntityIdx> {
        Some(self.target)
    }
    fn on_trigger(&mut self, w: &mut World) {
        // `ConsumeMirrorImageIfNecessary`: mirror images reported by the roll.
        if self.attack_hit
            && let Some(d) = &mut self.damage
        {
            trigger_any(w, d, |r| Some(json!({"Result": r.result})));
        }
    }
    rule_boilerplate!();
}

/// `RuleAttackWithWeapon`.
#[derive(Debug)]
pub struct RuleAttackWithWeapon {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub weapon: Weapon,
    pub penalty: i32,
    pub is_first_attack: bool,
    pub is_full_attack: bool,
    pub is_attack_of_opportunity: bool,
    pub is_charge: bool,
    pub attack_number: i32,
    pub attacks_count: i32,
    pub auto_hit: bool,
    pub auto_critical_threat: bool,
    pub auto_critical_confirmation: bool,
    pub force_flat_footed: bool,
    pub skip_animation: bool,
    /// What the roll reads from controllers the sim doesn't run (flanking, memory, can-act,
    /// concealment). `None` takes the driven input queued for (initiator, target) when the
    /// rule triggers, and reports unsupported if there is none.
    pub env: Option<AttackEnv>,
    // results
    pub attack_hit: Option<bool>,
    pub attack_result: i32,
    pub damage_result: Option<i32>,
}

impl RuleAttackWithWeapon {
    /// `LaunchProjectiles(BlueprintProjectileReference[])` / `LaunchProjectile(blueprint, first)`:
    /// per projectile, a `RuleAttackWithWeaponResolve` (with the damage rule when the roll hit)
    /// that the projectile triggers when it arrives (`ProjectileController`, not modelled:
    /// the resolves wait in [`super::CombatWorld::pending_resolves`] for the replay to fire
    /// them). A miss calls `Projectile.CalculateMissTarget` at launch: its draws are made
    /// here, the miss point they decide is projectile geometry (T1).
    fn launch_projectiles(
        &mut self,
        w: &mut World,
        stats: &RuleCalculateWeaponStats,
        roll: &RuleAttackRoll,
        link: AttackLink,
    ) {
        let (a, t) = (self.initiator, self.target);
        // `blueprintProjectileReference.Get() != null`.
        let projectiles: Vec<_> = self
            .weapon
            .projectile_refs()
            .into_iter()
            .filter_map(|g| g.and_then(|g| w.content.get(g)))
            .collect();
        for projectile in projectiles {
            // Manyshot: another projectile for a bow's first full-attack shot.
            if weapon::has_feature(w, a, weapon::feature::MANYSHOT)
                && self.is_first_attack
                && self.is_full_attack
                && !weapon::has_feature(w, a, weapon::feature::SUPPRESSED_MANYSHOT)
            {
                w.coverage.unsupported("RuleAttackWithWeapon: Manyshot");
            }
            let damage = if roll.is_hit() && !self.weapon.has_no_damage() {
                Some(create_rule_deal_damage(w, stats, roll, a, t, true, link.clone()))
            } else {
                None
            };
            let resolve = RuleAttackWithWeaponResolve {
                base: RuleBase::default(),
                initiator: a,
                target: t,
                damage,
                attack_hit: roll.is_hit(),
                weapon_melee: false,
                source_id: None,
            };
            w.combat.pending_resolves.push(resolve);
            if !roll.is_hit() {
                calculate_miss_target(w, &projectile);
            }
        }
        if w.entity(a).parts.iter().any(|p| p.clr == "UnitPartMagus") {
            w.coverage.unsupported("RuleAttackWithWeapon: eldritch archer spell");
        }
    }
}

/// `Projectile.CalculateMissTarget`'s draws for a weapon's projectile (`ProjectileController.Launch`
/// leaves `MaxRange` at 0). The point they decide (raycasts against the scene) isn't computed.
fn calculate_miss_target(w: &mut World, projectile: &crate::content::Blueprint) {
    let v = projectile.view();
    if v.bool("FallsOnMiss") {
        // `Random.insideUnitCircle` and a radius that can read the target bone's camera offset.
        w.coverage.unsupported("Projectile.CalculateMissTarget: FallsOnMiss");
        return;
    }
    // `MaxRange > 0f && MaxRange < MissRaycastDistance` is false (`MaxRange` is 0): no
    // `MISS_TARGET_RANGE` draw.
    let root = w.content.get(crate::unit::BLUEPRINT_ROOT);
    let Some(root) = root else {
        w.coverage.unsupported("BlueprintRoot missing");
        return;
    };
    let (min, max) = (
        root.view().f32("MinProjectileMissDeviation"),
        root.view().f32("MaxProjectileMissDeviation"),
    );
    w.rng.range_float(sites::MISS_TARGET_DEVIATION, min, max);
    w.rng.value(sites::MISS_TARGET_SIDE);
}

impl RuleAttackWithWeapon {
    #[must_use]
    pub fn new(initiator: EntityIdx, target: EntityIdx, weapon: Weapon, penalty: i32) -> Self {
        Self {
            base: RuleBase::default(),
            initiator,
            target,
            weapon,
            penalty,
            is_first_attack: false,
            is_full_attack: false,
            is_attack_of_opportunity: false,
            is_charge: false,
            attack_number: 0,
            attacks_count: 0,
            auto_hit: false,
            auto_critical_threat: false,
            auto_critical_confirmation: false,
            force_flat_footed: false,
            skip_animation: false,
            env: None,
            attack_hit: None,
            attack_result: 0,
            damage_result: None,
        }
    }
}

impl RuleEvent for RuleAttackWithWeapon {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleAttackWithWeapon", "RulebookTargetEvent"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn target(&self) -> Option<EntityIdx> {
        Some(self.target)
    }
    fn on_trigger(&mut self, w: &mut World) {
        let (a, t) = (self.initiator, self.target);
        let mut stats = RuleCalculateWeaponStats::new(a, self.weapon.clone(), Some(self.is_charge));
        trigger_any(w, &mut stats, |_| None);
        let env = self.env.take();
        let mut roll = RuleAttackRoll {
            base: RuleBase::default(),
            initiator: a,
            target: t,
            weapon: self.weapon.clone(),
            is_secondary: stats.is_secondary,
            enhancement: stats.enhancement.total(),
            critical_edge: stats.critical_edge(),
            penalty: self.penalty,
            attack_type: self.weapon.attack_type(),
            auto_hit: self.auto_hit,
            auto_miss: false,
            auto_critical_threat: self.auto_critical_threat,
            auto_critical_confirmation: self.auto_critical_confirmation,
            force_flat_footed: self.force_flat_footed,
            target_is_flanked: false,
            do_not_provoke: self.is_attack_of_opportunity,
            immune_to_critical_hit: false,
            ignore_crit_immunity: false,
            immune_to_sneak_attack: false,
            precise_strike: 0,
            reason_full_attack: Some(self.is_full_attack),
            env,
            result: result::UNKNOWN,
            d20: 0,
            attack_bonus: 0,
            target_ac: 0,
            target_critical_ac: 0,
            is_target_flat_footed: false,
            is_critical_roll: false,
            is_critical_confirmed: false,
            critical_confirmation_d20: 0,
            is_sneak_attack: false,
        };
        trigger_any(w, &mut roll, |r| Some(r.result_json()));
        self.attack_hit = Some(roll.is_hit());
        self.attack_result = roll.result;
        let link = AttackLink {
            is_critical_confirmed: roll.is_critical_confirmed,
            is_sneak_attack: roll.is_sneak_attack,
            sneak_used: std::rc::Rc::default(),
            precise_strike: roll.precise_strike,
            weapon_attack: true,
        };
        if self.skip_animation {
            w.coverage.unsupported("RuleAttackWithWeapon: SkipAnimation");
            return;
        }
        if self.weapon.projectiles() != 0 {
            self.launch_projectiles(w, &stats, &roll, link);
            return;
        }
        let damage = if self.weapon.has_no_damage() {
            None
        } else {
            Some(create_rule_deal_damage(w, &stats, &roll, a, t, true, link))
        };
        let mut resolve = RuleAttackWithWeaponResolve {
            base: RuleBase::default(),
            initiator: a,
            target: t,
            damage,
            attack_hit: roll.is_hit(),
            weapon_melee: self.weapon.is_melee(),
            source_id: None,
        };
        trigger_any(w, &mut resolve, |_| None);
        self.damage_result = resolve.damage.as_ref().filter(|_| roll.is_hit()).map(|d| d.result);
    }
    rule_boilerplate!();
}

/// `RuleAttackWithWeapon.CreateRuleDealDamage(enablePrecisionAndCritical)` with
/// `CreateDamage`: the first description is created twice (the first copy is discarded).
fn create_rule_deal_damage(
    w: &mut World,
    stats: &RuleCalculateWeaponStats,
    roll: &RuleAttackRoll,
    a: EntityIdx,
    t: EntityIdx,
    enable: bool,
    link: AttackLink,
) -> RuleDealDamage {
    let mut bundle: Vec<BaseDamage> = Vec::new();
    for (i, d) in stats.damage_description.iter().enumerate() {
        let damage = d.create_damage(w);
        if i == 0 {
            let mut b = d.create_damage(w);
            if enable && roll.is_critical_confirmed {
                b.critical_modifier = Some(stats.critical_multiplier());
                b.tactical_critical_modifier = 1.0;
            }
            bundle.push(b);
        } else {
            bundle.push(damage);
        }
    }
    let mut r = RuleDealDamage::new(a, t, bundle, link);
    r.disable_precision = !enable;
    r.weapon_ranged = Some(stats.weapon.is_ranged());
    r.weapon_category = Some(stats.weapon.category());
    r.weapon_item = Some(stats.weapon.item);
    r.positions = roll.env.as_ref().and_then(|e| e.positions);
    r
}

/// Reads an attack's inputs from a WrathOracle `rule` snapshot of `RuleAttackWithWeapon`.
#[must_use]
pub fn attack_from_snapshot(
    w: &World,
    a: EntityIdx,
    t: EntityIdx,
    fields: &J,
) -> Option<RuleAttackWithWeapon> {
    let item_id = fields["Weapon"]["$item"].as_str()?;
    let item = w.by_id.get(item_id).copied().or_else(|| {
        // Limb items are created at load with fresh ids: match the wielder's limb by
        // blueprint.
        let bp = wr_content::Guid::parse(fields["Weapon"]["blueprint"].as_str()?)?;
        weapon::limb_weapons(w, a)
            .into_iter()
            .flatten()
            .chain(w.unit(a).and_then(|u| u.body.empty_hand))
            .find(|i| w.item(*i).and_then(|x| x.blueprint) == Some(bp))
    })?;
    let wp = Weapon::of(w, item)?;
    let b = |k: &str| fields[k].as_bool().unwrap_or(false);
    let i = |k: &str| fields[k].as_i64().and_then(|v| i32::try_from(v).ok()).unwrap_or(0);
    let mut r = RuleAttackWithWeapon::new(a, t, wp, i("AttackBonusPenalty"));
    r.is_first_attack = b("IsFirstAttack");
    r.is_full_attack = b("IsFullAttack");
    r.is_attack_of_opportunity = b("IsAttackOfOpportunity");
    r.is_charge = b("IsCharge");
    r.attack_number = i("AttackNumber");
    r.attacks_count = i("AttacksCount");
    r.auto_hit = b("AutoHit");
    r.auto_critical_threat = b("AutoCriticalThreat");
    r.auto_critical_confirmation = b("AutoCriticalConfirmation");
    r.force_flat_footed = b("ForceFlatFooted");
    r.skip_animation = b("SkipAnimation");
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::tests::world_with_unit;
    use crate::unit_state::cond;

    fn ac_rule(w: &mut World, u: EntityIdx, ty: AttackType, env: AttackEnv) -> RuleCalculateAC {
        let mut r = RuleCalculateAC {
            base: RuleBase::default(),
            initiator: u,
            target: u,
            attack_type: ty,
            force_flat_footed: false,
            is_critical: false,
            env,
            is_target_flat_footed: false,
            result: 0,
            bonuses: BonusList::default(),
        };
        trigger_any(w, &mut r, |_| None);
        r
    }

    fn set_cond(w: &mut World, u: EntityIdx, c: i32) {
        w.unit_mut(u).expect("unit").state.conditions[usize::try_from(c).expect("c")] = 1;
    }

    #[test]
    fn ac_rule_bonuses_apply_inside_the_temporary_scope_and_are_restored() {
        let (mut w, u) = world_with_unit();
        let base = w.stats(u).v(StatType::AC).modified_value;
        set_cond(&mut w, u, units::condition::BLINDNESS);
        let r = ac_rule(&mut w, u, AttackType::Melee, AttackEnv::neutral());
        assert_eq!(r.result, base - 2, "the blindness penalty is part of the AC read");
        assert_eq!(w.stats(u).v(StatType::AC).modified_value, base, "scope restored");
        assert_eq!(w.stats(u).v(StatType::AC).temporary_scopes, 0);
        // Helpless in melee makes the target flat-footed and adds -4 on top.
        set_cond(&mut w, u, cond::UNCONSCIOUS);
        let r = ac_rule(&mut w, u, AttackType::Melee, AttackEnv::neutral());
        assert!(r.is_target_flat_footed);
        assert_eq!(w.stats(u).v(StatType::AC).modified_value, base);
    }

    #[test]
    fn prone_reads_the_posture_not_the_condition() {
        let (mut w, u) = world_with_unit();
        let base = w.stats(u).v(StatType::AC).modified_value;
        w.unit_mut(u).expect("unit").prone_active = true;
        assert!(!crate::unit_state::has_condition(&w, u, cond::PRONE));
        let env = AttackEnv::neutral;
        assert_eq!(ac_rule(&mut w, u, AttackType::Melee, env()).result, base - 4);
        assert_eq!(ac_rule(&mut w, u, AttackType::Ranged, env()).result, base + 4);
        // Condition without posture: no modifier (the posture is what the rule reads).
        w.unit_mut(u).expect("unit").prone_active = false;
        set_cond(&mut w, u, cond::PRONE);
        assert_eq!(ac_rule(&mut w, u, AttackType::Melee, env()).result, base);
        // A get-up in progress (the view's state, supplied by the environment) counts.
        let mut e = env();
        e.target_is_get_up = Some(true);
        assert_eq!(ac_rule(&mut w, u, AttackType::Melee, e).result, base - 4);
        assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
        // A retained posture short-circuits: a missing get-up state isn't read, so it's fine.
        w.unit_mut(u).expect("unit").prone_active = true;
        let mut e = env();
        e.target_is_get_up = None;
        assert_eq!(ac_rule(&mut w, u, AttackType::Melee, e.clone()).result, base - 4);
        assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
        // Without the posture the rule reads the get-up state: missing, it's reported.
        w.unit_mut(u).expect("unit").prone_active = false;
        assert_eq!(ac_rule(&mut w, u, AttackType::Melee, e).result, base);
        assert!(!w.coverage.valid());
    }

    #[test]
    fn paralysis_makes_a_conscious_unit_helpless_and_reports_the_cutscene_branch() {
        let (mut w, u) = world_with_unit();
        assert!(!units::is_helpless(&mut w, u));
        assert!(w.coverage.valid());
        set_cond(&mut w, u, units::condition::PARALYZED);
        assert!(units::is_helpless(&mut w, u));
        assert!(!w.coverage.valid(), "cutscene control of the unit isn't modelled");
    }

    #[test]
    fn missing_attack_environment_is_reported_not_defaulted_to_flat_footed() {
        let (mut w, u) = world_with_unit();
        let e = units::take_env(&mut w, u, u);
        assert!(e.target_can_act_in_combat && e.target_remembers_initiator);
        assert!(!w.coverage.valid());
        let (mut w, u) = world_with_unit();
        let mut driven = AttackEnv::neutral();
        driven.target_flanked = true;
        w.combat.env.push(((u, u), driven.clone()));
        assert_eq!(units::take_env(&mut w, u, u), driven);
        assert!(w.coverage.valid());
        assert_eq!(w.combat.driven_inputs, 1);
    }

    /// `EnhancementValue.Calculate`: the best non-stacking modifier plus the stacking ones,
    /// capped at 5 unless the best alone is already past it, plus the extra modifiers.
    #[test]
    fn enhancement_value_clamps_stacking_and_adds_extra() {
        let mut e = EnhancementValue::default();
        assert_eq!(e.total(), 0);
        e.add_modifier(Bonus::desc(2, MD::Enhancement));
        assert!(!e.add_modifier(Bonus::desc(1, MD::Enhancement)), "a lower one is refused");
        e.add_modifier(Bonus::desc(1, MD::UntypedStackable));
        assert_eq!(e.total(), 3);
        e.add_modifier(Bonus::desc(4, MD::UntypedStackable));
        assert_eq!(e.total(), 5, "capped");
        e.add_extra_modifier(Bonus::desc(1, MD::None));
        assert_eq!(e.total(), 6, "extra modifiers sit outside the cap");
        let mut big = EnhancementValue::default();
        big.add_modifier(Bonus::desc(6, MD::Enhancement));
        big.add_modifier(Bonus::desc(1, MD::UntypedStackable));
        assert_eq!(big.total(), 6, "not capped when the best is already past the limit");
    }

    /// `GeometryUtils.MechanicsDistance`: the horizontal distance, unless the heights differ
    /// by more than 1.5.
    #[test]
    fn mechanics_distance_is_horizontal_until_the_height_gap_dominates() {
        let d = units::mechanics_distance;
        assert!((d([0.0, 0.0, 0.0], [3.0, 1.0, 4.0]) - 5.0).abs() < 1e-6);
        assert!((d([0.0, 0.0, 0.0], [3.0, 2.0, 4.0]) - 5.0).abs() < 1e-6, "half the gap is 1");
        assert!((d([0.0, 0.0, 0.0], [0.0, 20.0, 1.0]) - 10.0).abs() < 1e-6);
    }

    /// `UnitStatusEffectHelpless`: -5 minus the Dexterity bonus on the reflex save and the
    /// skills, also less the dodge bonus on the AC, removed with the effect.
    #[test]
    fn the_helpless_condition_penalizes_and_is_undone() {
        let (mut w, u) = world_with_unit();
        let ac = |w: &World| w.stats(u).v(StatType::AC).modified_value;
        let (ac0, reflex0) = (ac(&w), w.stats(u).v(StatType::SaveReflex).modified_value);
        crate::unit_state::add_condition(&mut w, u, crate::unit_state::cond::HELPLESS, None);
        // Dexterity 10: bonus 0, penalty -5.
        assert_eq!(w.stats(u).v(StatType::SaveReflex).modified_value, reflex0 - 5);
        assert_eq!(ac(&w), ac0 - 5);
        crate::unit_state::remove_condition(&mut w, u, crate::unit_state::cond::HELPLESS, true);
        assert_eq!((ac(&w), w.stats(u).v(StatType::SaveReflex).modified_value), (ac0, reflex0));
    }

    #[test]
    fn damage_float_expressions_stay_in_double_until_the_cast() {
        let (mut w, u) = world_with_unit();
        // `(int)((float)10 * 0.9f)`: the product is 8.99999976, not 9.0f.
        let mut d = RuleDealDamage::new(u, u, Vec::new(), AttackLink::default());
        d.difficulty_modifier = 0.9;
        assert_eq!(d.apply_difficulty(&mut w, 10), 8);
        // `(int)Math.Floor(num8 * num9)` with num9 = 1f + -0.1f = 0.9f.
        let mut c = RuleCalculateDamage {
            base: RuleBase::default(),
            initiator: u,
            target: u,
            bundle: Vec::new(),
            disable_precision: false,
            half: false,
            already_halved: false,
            half_because_saving_throw: false,
            modifier_bonus: Some(-0.1),
            calculated: Vec::new(),
        };
        let mut dmg = BaseDamage::new(
            DamageTypeDesc::default(),
            ModifiableDiceFormula::new(DiceFormula::ZERO),
            0,
        );
        dmg.pre_rolled = Some(10);
        assert_eq!(c.calculate_value(&mut w, dmg).value_without_reduction, 8);
    }
}
