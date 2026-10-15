//! Weapons (decompiled, not copied): dice formulas (`Kingmaker.RuleSystem/DiceFormula.cs`,
//! `Kingmaker.RuleSystem.Rules/ModifiableDiceFormula.cs`), the damage scale table
//! (`Kingmaker.Blueprints.Items.Weapons/WeaponDamageScaleTable.cs`), weapon blueprint
//! properties (`BlueprintItemWeapon.cs`, `BlueprintWeaponType.cs`), `ItemEntityWeapon`
//! (`Kingmaker.Items/ItemEntityWeapon.cs`: `Size`, `HoldInTwoHands`), a unit's weapon
//! slots (`Kingmaker.Items/UnitBody.cs`, `Kingmaker.Items.Slots/{WeaponSlot,HandSlot}.cs`,
//! `Kingmaker.Items/HandsEquipmentSet.cs` grips) and `AddAdditionalLimb`
//! (`Kingmaker.UnitLogic.FactLogic/AddAdditionalLimb.cs`).

use std::rc::Rc;

use crate::components::{Port, bits, owner_unit, with_settings};
use crate::content::{Blueprint, View};
use crate::registry::{Hook, Registry};
use crate::stats::types::{ModifierDescriptor, Size, StatType};
use crate::world::{CompRef, EntityIdx, World};

/// `DiceFormula`: `m_Rolls` (clamped at 0 by the constructor) and `m_Dice` (`DiceType`,
/// whose value is its number of sides).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiceFormula {
    pub rolls: i32,
    pub dice: i32,
}

impl DiceFormula {
    pub const ZERO: Self = Self { rolls: 0, dice: 0 };
    pub const ONE: Self = Self { rolls: 1, dice: 1 };

    #[must_use]
    pub fn new(rolls: i32, dice: i32) -> Self {
        Self { rolls: rolls.max(0), dice }
    }

    /// A serialized `DiceFormula` (stored as saved: no clamping).
    #[must_use]
    pub fn from_view(v: Option<View<'_>>) -> Self {
        v.map_or(Self::ZERO, |v| Self { rolls: v.i32("m_Rolls"), dice: v.i32("m_Dice") })
    }

    /// `Rolls`: 0 for `DiceType.Zero`.
    #[must_use]
    pub fn rolls(self) -> i32 {
        if self.dice != 0 { self.rolls } else { 0 }
    }
}

/// `DiceFormulaModifier`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiceModification {
    pub formula: DiceFormula,
    pub descriptor: ModifierDescriptor,
    pub critical: bool,
}

/// `ModifiableDiceFormula`: the last modification wins.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModifiableDiceFormula {
    pub base: DiceFormula,
    pub modifications: Vec<DiceModification>,
}

impl ModifiableDiceFormula {
    #[must_use]
    pub fn new(base: DiceFormula) -> Self {
        Self { base, modifications: Vec::new() }
    }

    /// `ModifiedValue`.
    #[must_use]
    pub fn modified(&self) -> DiceFormula {
        self.modifications.last().map_or(self.base, |m| m.formula)
    }

    #[must_use]
    pub fn has_modifications(&self) -> bool {
        !self.modifications.is_empty()
    }

    /// `Modify(formula, descriptor)`.
    pub fn modify(&mut self, formula: DiceFormula, descriptor: ModifierDescriptor) {
        self.modifications.push(DiceModification { formula, descriptor, critical: false });
    }

    /// `ModifyCritical(formula)`.
    pub fn modify_critical(&mut self, formula: DiceFormula) {
        self.modifications.push(DiceModification {
            formula,
            descriptor: ModifierDescriptor::None,
            critical: true,
        });
    }
}

/// `WeaponDamageScaleTable.Data`: rows of (rolls, dice) from `Fine` to `Colossal`. `D1()`
/// is `DiceFormula.One` (1, `DiceType.One`).
const SCALE: [[(i32, i32); 9]; 18] = [
    [(1, 1), (1, 1), (1, 1), (1, 1), (1, 1), (1, 2), (1, 3), (1, 4), (1, 6)],
    [(1, 1), (1, 1), (1, 1), (1, 1), (1, 2), (1, 3), (1, 4), (1, 6), (1, 8)],
    [(1, 1), (1, 1), (1, 1), (1, 2), (1, 3), (1, 4), (1, 6), (1, 8), (2, 6)],
    [(1, 1), (1, 1), (1, 2), (1, 3), (1, 4), (1, 6), (1, 8), (2, 6), (3, 6)],
    [(1, 1), (1, 2), (1, 3), (1, 4), (1, 6), (1, 8), (2, 6), (3, 6), (4, 6)],
    [(1, 2), (1, 3), (1, 4), (1, 6), (1, 8), (2, 6), (3, 6), (4, 6), (6, 6)],
    [(1, 2), (1, 3), (1, 4), (1, 6), (2, 4), (2, 6), (3, 6), (4, 6), (6, 6)],
    [(1, 3), (1, 4), (1, 6), (1, 8), (1, 10), (2, 8), (3, 8), (4, 8), (6, 8)],
    [(1, 4), (1, 6), (1, 8), (1, 10), (1, 12), (3, 6), (4, 6), (6, 6), (8, 6)],
    [(1, 4), (1, 6), (1, 8), (1, 10), (2, 6), (3, 6), (4, 6), (6, 6), (8, 6)],
    [(1, 4), (1, 6), (1, 10), (1, 10), (2, 8), (3, 8), (4, 8), (6, 8), (8, 8)],
    [(1, 6), (1, 8), (1, 10), (2, 6), (3, 6), (4, 6), (6, 6), (8, 6), (12, 6)],
    [(1, 8), (1, 10), (2, 6), (2, 8), (2, 10), (4, 8), (6, 8), (8, 8), (12, 8)],
    [(1, 8), (1, 10), (2, 6), (3, 6), (4, 6), (6, 6), (8, 6), (12, 6), (16, 6)],
    [(1, 10), (1, 10), (3, 6), (2, 12), (3, 10), (4, 10), (6, 10), (8, 10), (12, 10)],
    [(1, 2), (1, 3), (1, 4), (1, 6), (1, 8), (1, 10), (2, 8), (3, 8), (4, 8)],
    [(1, 1), (1, 2), (1, 3), (1, 4), (1, 6), (1, 8), (1, 10), (2, 8), (3, 8)],
    // Padding never matched: the table has 17 rows.
    [(-1, -1); 9],
];

fn size_index(s: Size) -> usize {
    s as usize
}

/// `WeaponDamageScaleTable.Scale(baseDice, size, baseDiceSize)`: the first row with
/// `baseDice` at `baseDiceSize`, at `size`; unchanged (logged) when no row matches.
#[must_use]
pub fn scale(w: &mut World, base: DiceFormula, size: Size, base_size: Size) -> DiceFormula {
    if size == base_size {
        return base;
    }
    for row in &SCALE {
        let (r, d) = row[size_index(base_size)];
        if r == base.rolls && d == base.dice {
            let (r2, d2) = row[size_index(size)];
            return DiceFormula { rolls: r2, dice: d2 };
        }
    }
    w.log("error", format!("Can't scale damage: {base:?} {base_size:?}->{size:?}"));
    base
}

/// `SizeExtension.Shift(size, delta)`: clamped to `Fine`..`Colossal`.
#[must_use]
pub fn shift(s: Size, delta: i32) -> Size {
    let i = (s as i32 + delta).clamp(0, 8);
    Size::from_i64(i64::from(i)).unwrap_or(s)
}

/// `Kingmaker.RuleSystem.AttackType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackType {
    Melee = 0,
    Touch = 1,
    Ranged = 2,
    RangedTouch = 3,
}

impl AttackType {
    #[must_use]
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Touch,
            2 => Self::Ranged,
            3 => Self::RangedTouch,
            _ => Self::Melee,
        }
    }
    /// `AttackTypeExtensions.IsTouch`.
    #[must_use]
    pub fn is_touch(self) -> bool {
        matches!(self, Self::Touch | Self::RangedTouch)
    }
    /// `IsMelee`.
    #[must_use]
    pub fn is_melee(self) -> bool {
        matches!(self, Self::Melee | Self::Touch)
    }
}

/// `WeaponCategory.Shortbow`, `Longbow` (the enum's declaration order: 42 and 43).
pub const CATEGORY_SHORTBOW: i64 = 42;
pub const CATEGORY_LONGBOW: i64 = 43;

/// `WeaponSubCategory.Finessable`.
pub const SUB_FINESSABLE: i64 = 3;

/// A weapon item with its blueprint and weapon type.
#[derive(Debug, Clone)]
pub struct Weapon {
    pub item: EntityIdx,
    pub bp: Rc<Blueprint>,
    pub ty: Rc<Blueprint>,
}

impl Weapon {
    /// The item as an `ItemEntityWeapon` (its blueprint is a `BlueprintItemWeapon` with a
    /// weapon type).
    #[must_use]
    pub fn of(w: &World, item: EntityIdx) -> Option<Self> {
        let bp = w.content.get(w.item(item)?.blueprint?)?;
        if !bp.view().is("BlueprintItemWeapon") {
            return None;
        }
        let ty = w.content.get(bp.view().guid("m_Type")?)?;
        Some(Self { item, bp, ty })
    }

    fn b(&self) -> View<'_> {
        self.bp.view()
    }
    fn t(&self) -> View<'_> {
        self.ty.view()
    }

    #[must_use]
    pub fn attack_type(&self) -> AttackType {
        AttackType::from_i64(self.t().int("m_AttackType"))
    }
    /// `IsRanged`: `Ranged` or `RangedTouch`.
    #[must_use]
    pub fn is_ranged(&self) -> bool {
        matches!(self.attack_type(), AttackType::Ranged | AttackType::RangedTouch)
    }
    #[must_use]
    pub fn is_melee(&self) -> bool {
        !self.is_ranged()
    }
    #[must_use]
    pub fn is_natural(&self) -> bool {
        self.t().bool("m_IsNatural")
    }
    #[must_use]
    pub fn is_unarmed(&self) -> bool {
        self.t().bool("m_IsUnarmed")
    }
    #[must_use]
    pub fn is_two_handed(&self) -> bool {
        self.t().bool("m_IsTwoHanded")
    }
    #[must_use]
    pub fn is_double(&self) -> bool {
        self.b().bool("Double")
    }
    /// `BlueprintItemWeapon.CountAsDouble`.
    #[must_use]
    pub fn count_as_double(&self) -> bool {
        self.b().bool("CountAsDouble")
    }
    #[must_use]
    pub fn category(&self) -> i64 {
        self.t().int("Category")
    }
    /// `AlwaysPrimary`: natural weapons only.
    #[must_use]
    pub fn always_primary(&self) -> bool {
        self.is_natural() && self.b().bool("m_AlwaysPrimary")
    }
    /// `IsDamageDiceOverridden`.
    #[must_use]
    pub fn damage_dice_overridden(&self) -> bool {
        self.b().bool("m_OverrideDamageDice")
    }
    /// `BaseDamage`.
    #[must_use]
    pub fn base_damage(&self) -> DiceFormula {
        if self.damage_dice_overridden() {
            DiceFormula::from_view(self.b().obj("m_DamageDice"))
        } else {
            DiceFormula::from_view(self.t().obj("m_BaseDamage"))
        }
    }
    /// `HasNoDamage`: `BaseDamage == DiceFormula.Zero`.
    #[must_use]
    pub fn has_no_damage(&self) -> bool {
        self.base_damage() == DiceFormula::ZERO
    }
    /// `DamageType` (a `DamageTypeDescription`).
    #[must_use]
    pub fn damage_type(&self) -> Option<View<'_>> {
        if self.b().bool("m_OverrideDamageType") {
            self.b().obj("m_DamageType")
        } else {
            self.t().obj("m_DamageType")
        }
    }
    #[must_use]
    pub fn critical_roll_edge(&self) -> i32 {
        self.t().i32("m_CriticalRollEdge")
    }
    /// `CriticalModifier.IntValue()`.
    #[must_use]
    pub fn critical_modifier(&self) -> i32 {
        self.t().i32("m_CriticalModifier")
    }
    /// `AttackBonusStat`.
    #[must_use]
    pub fn attack_bonus_stat(&self) -> StatType {
        if self.t().bool("m_OverrideAttackBonusStat") {
            StatType(self.t().i32("m_AttackBonusStatOverride"))
        } else if self.is_ranged() {
            StatType::Dexterity
        } else {
            StatType::Strength
        }
    }
    /// `IsOneHandedWhichCanBeUsedWithTwoHands`.
    #[must_use]
    pub fn one_handed_usable_with_two(&self) -> bool {
        let t = self.t();
        !t.bool("m_IsTwoHanded")
            && !t.bool("m_IsOneHanded")
            && self.attack_type() == AttackType::Melee
            && !t.bool("m_IsLight")
            && !self.is_natural()
            && !self.is_unarmed()
    }
    /// `CanTakeTwoHands()`.
    #[must_use]
    pub fn can_take_two_hands(&self) -> bool {
        if !self.is_two_handed() && !self.one_handed_usable_with_two() {
            return self.is_double();
        }
        true
    }
    /// `CanTakeOneHand(unit)` (`FighterOnHandedGrip` reported by the caller).
    #[must_use]
    pub fn can_take_one_hand(&self) -> bool {
        if !self.is_two_handed() { !self.is_double() } else { false }
    }
    /// `WeaponVisualParameters.Projectiles` count: the weapon's own list, else its type's.
    #[must_use]
    pub fn projectiles(&self) -> usize {
        self.projectile_refs().len()
    }
    /// `WeaponVisualParameters.Projectiles`: the weapon's own list, else its type's (an empty
    /// reference is `None`).
    #[must_use]
    pub fn projectile_refs(&self) -> Vec<Option<wr_content::Guid>> {
        let refs = |v: View<'_>| {
            v.obj("m_VisualParameters")
                .map_or_else(Vec::new, |v| v.guids_with_nulls("m_Projectiles"))
        };
        let own = refs(self.b());
        if own.is_empty() { refs(self.t()) } else { own }
    }
    /// The blueprint's `m_Size`.
    #[must_use]
    pub fn blueprint_size(&self) -> Size {
        Size::from_i64(self.b().int("m_Size")).unwrap_or(Size::Medium)
    }
}

/// `UnitState.Size` (`spawn::state_size`; Medium for a unit with no size).
#[must_use]
pub fn unit_size(w: &mut World, u: EntityIdx) -> Size {
    crate::spawn::state_size(w, u).unwrap_or(Size::Medium)
}

/// `ItemEntityWeapon.Size`: Medium unwielded; a non-player wielder uses the blueprint's
/// non-Medium size; else the wielder's size.
#[must_use]
pub fn weapon_size(w: &mut World, wp: &Weapon) -> Size {
    let Some(wielder) = w.item(wp.item).and_then(|i| i.wielder) else { return Size::Medium };
    let player = w.unit(wielder).is_some_and(|u| u.is_player_faction);
    if wp.blueprint_size() != Size::Medium && !player {
        return wp.blueprint_size();
    }
    unit_size(w, wielder)
}

/// A hand of the current hand set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hand {
    Primary,
    Secondary,
}

fn hand_slot_name(w: &World, u: EntityIdx, h: Hand) -> String {
    let set = w.unit(u).map_or(0, |x| x.body.current_hand_set);
    let n = match h {
        Hand::Primary => "PrimaryHand",
        Hand::Secondary => "SecondaryHand",
    };
    format!("HandsEquipmentSets[{set}].{n}")
}

/// The slot's item and `Active` flag.
fn hand_slot(w: &World, u: EntityIdx, h: Hand) -> Option<(Option<EntityIdx>, bool)> {
    let name = hand_slot_name(w, u, h);
    let unit = w.unit(u)?;
    unit.body.slots.iter().find(|s| s.name == name).map(|s| (s.item, s.active))
}

/// `HandSlot.HasItem`.
#[must_use]
pub fn hand_has_item(w: &World, u: EntityIdx, h: Hand) -> bool {
    hand_slot(w, u, h).is_some_and(|(i, _)| i.is_some())
}

/// `HandSlot.MaybeWeapon` (double weapons reported): with `ShieldBash`, the hand's shield's
/// weapon component (`MaybeShield`, active slot or not); else the item if it is a weapon and the
/// slot is active (a shield is not a weapon: `null`), or the empty-hand weapon.
#[must_use]
pub fn hand_weapon(w: &mut World, u: EntityIdx, h: Hand) -> Option<EntityIdx> {
    let (item, active) = hand_slot(w, u, h)?;
    if h == Hand::Secondary
        && let Some(p) = hand_slot(w, u, Hand::Primary).and_then(|(i, _)| i)
        && Weapon::of(w, p).is_some_and(|x| x.is_double())
    {
        w.coverage.unsupported("HandSlot.MaybeWeapon: double weapon");
    }
    let shield_weapon = item
        .filter(|i| w.entity(*i).clr == "ItemEntityShield")
        .and_then(|i| w.item(i).and_then(|x| x.weapon_component));
    if has_feature(w, u, SHIELD_BASH)
        && let Some(sw) = shield_weapon
    {
        return Some(sw);
    }
    match item {
        Some(i) if active => Weapon::of(w, i).map(|_| i),
        _ => w.unit(u).and_then(|x| x.body.empty_hand),
    }
}

/// `UnitBody.AdditionalLimbs`: per limb slot, its weapon when `HasWeapon` (`HasItem` and
/// `MaybeWeapon != null`; an inactive slot with an item answers the empty-hand weapon).
#[must_use]
pub fn limb_weapons(w: &World, u: EntityIdx) -> Vec<Option<EntityIdx>> {
    let Some(unit) = w.unit(u) else { return Vec::new() };
    unit.body
        .limbs
        .iter()
        .map(|s| match s.item {
            Some(i) if s.active => Weapon::of(w, i).map(|_| i),
            Some(_) => unit.body.empty_hand,
            None => None,
        })
        .collect()
}

/// `HandsEquipmentSet.GripType` of the current set (auto grips resolved by
/// `GetPreferredAutoGripType`; `GripType.TwoHanded` is 2).
pub fn current_grip_two_handed(w: &mut World, u: EntityIdx) -> bool {
    let set = w.unit(u).map_or(0, |x| x.body.current_hand_set);
    let saved = w
        .unit(u)
        .and_then(|x| x.body.grip_types.get(usize::try_from(set).unwrap_or(0)).copied())
        .unwrap_or(0);
    match saved {
        0 => preferred_auto_two_handed(w, u),
        g => g == 2,
    }
}

/// `IsGripAllowed(TwoHanded)` / `(OneHanded)` for the current set.
fn grip_allowed(w: &mut World, u: EntityIdx, two: bool) -> bool {
    let p_item = hand_has_item(w, u, Hand::Primary);
    let s_item = hand_has_item(w, u, Hand::Secondary);
    if !p_item && !s_item {
        return !two;
    }
    if p_item {
        let pw = hand_weapon(w, u, Hand::Primary).and_then(|i| Weapon::of(w, i));
        if s_item || pw.as_ref().is_some_and(Weapon::is_double) {
            return !two;
        }
    }
    let item = if p_item {
        hand_slot(w, u, Hand::Primary).and_then(|x| x.0)
    } else {
        // A locked secondary hand allows only one-handed grips: slot locks aren't modelled.
        hand_slot(w, u, Hand::Secondary).and_then(|x| x.0)
    };
    let wp = item.and_then(|i| Weapon::of(w, i));
    if two && wp.as_ref().is_some_and(|x| !x.can_take_two_hands()) {
        return false;
    }
    if !two && wp.as_ref().is_none_or(|x| !x.can_take_one_hand()) {
        return false;
    }
    true
}

/// `GetPreferredAutoGripType() == TwoHanded`.
fn preferred_auto_two_handed(w: &mut World, u: EntityIdx) -> bool {
    let two = grip_allowed(w, u, true);
    if two && grip_allowed(w, u, false) {
        if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartMagus") {
            w.coverage.unsupported("grip: UnitPartMagus spell combat");
        }
        if has_feature(w, u, FIGHTER_ONE_HANDED_GRIP) {
            return false;
        }
        // Empty-hand primary with a shield and Improved Unarmed Strike: shields reported
        // by `hand_weapon`.
    }
    if !hand_has_item(w, u, Hand::Primary) && hand_has_item(w, u, Hand::Secondary) {
        return false;
    }
    two
}

/// `MechanicsFeatureType` values read here.
pub const SHIELD_BASH: i64 = 4;
pub const IMPROVED_UNARMED_STRIKE: i64 = 21;
pub const FIGHTER_ONE_HANDED_GRIP: i64 = 112;

/// `UnitMechanicFeatures.<feature>` is on.
#[must_use]
pub fn has_feature(w: &World, u: EntityIdx, f: i64) -> bool {
    w.unit(u).is_some_and(|x| x.mechanics_features.get(&f).copied().unwrap_or(0) > 0)
}

/// `ItemEntityWeapon.HoldInTwoHands`: held in a hand slot whose set grips two-handed.
pub fn hold_in_two_hands(w: &mut World, wp: &Weapon) -> bool {
    let Some((u, slot)) = w.item(wp.item).and_then(|i| i.holding_slot.clone()) else {
        return false;
    };
    let set = w.unit(u).map_or(0, |x| x.body.current_hand_set);
    if !slot.starts_with(&format!("HandsEquipmentSets[{set}].")) {
        return false;
    }
    current_grip_two_handed(w, u)
}

pub fn register(r: &mut Registry) {
    r.register(Box::new(Port::new(
        "AddAdditionalLimb",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        add_additional_limb,
    )));
}

/// `AddAdditionalLimb.OnTurnOn`/`OnTurnOff`: `UnitBody.AddAdditionalLimb(Weapon, Fact)`
/// creates the limb's weapon item in a new limb slot (`Data.LimbIndex`); per-category
/// slot limits (`BlueprintPersonageLimits`) are reported.
fn add_additional_limb(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::TurnOn => {
            let Some(g) = with_settings(w, c, |v| v.guid("m_Weapon")).flatten() else { return };
            let item = create_weapon(w, g, u);
            let Some(unit) = w.unit_mut(u) else { return };
            // `AddAdditionalLimbItem`: the first limb slot without an item (unused-limb
            // bookkeeping isn't modelled), else a new slot.
            let idx = match unit.body.limbs.iter().position(|s| s.item.is_none()) {
                Some(i) => i,
                None => {
                    unit.body.limbs.push(crate::unit::Slot {
                        name: format!("AdditionalLimbs[{}]", unit.body.limbs.len()),
                        item: None,
                        active: true,
                    });
                    unit.body.limbs.len() - 1
                }
            };
            unit.body.limbs[idx].item = Some(item);
            // `AdditionalLimbs[index].ParentFact = fact`.
            unit.body.limb_parents.insert(idx, c.fact);
            let name = unit.body.limbs[idx].name.clone();
            if let Some(it) = w.item_mut(item) {
                it.holding_slot = Some((u, name));
            }
            let mut d = w.component(c).data.clone().unwrap_or_else(|| serde_json::json!({}));
            d["LimbIndex"] = serde_json::json!(idx);
            w.component_mut(c).data = Some(d);
            // `BlueprintPersonageLimits.GetWeaponSlotCount(category)`: more limbs of the
            // category than allowed marks the worst unused (not modelled).
            let category = Weapon::of(w, item).map_or(-1, |x| x.category());
            let limit = w
                .content
                .get(crate::unit::BLUEPRINT_ROOT)
                .and_then(|r| r.view().guid("m_PersonageLimits"))
                .and_then(|g| w.content.get(g))
                .and_then(|l| {
                    l.view()
                        .objs("m_DopLimbMaxCount")
                        .iter()
                        .map(|o| View(o))
                        .find(|o| o.int("Slot") == category)
                        .map(|o| o.i32("Count"))
                })
                .unwrap_or(-1);
            if limit >= 0 {
                let n = limb_weapons(w, u)
                    .into_iter()
                    .flatten()
                    .filter(|i| Weapon::of(w, *i).is_some_and(|x| x.category() == category))
                    .count();
                if i32::try_from(n).unwrap_or(i32::MAX) > limit {
                    w.coverage.unsupported("UnitBody.AddAdditionalLimb: limb limit exceeded");
                }
            }
        }
        Hook::TurnOff => {
            let idx = w
                .component(c)
                .data
                .as_ref()
                .and_then(|d| d["LimbIndex"].as_u64())
                .and_then(|x| usize::try_from(x).ok());
            if let Some(i) = idx
                && let Some(unit) = w.unit_mut(u)
                && i < unit.body.limbs.len()
            {
                unit.body.limbs[i].item = None;
            }
        }
        _ => {}
    }
}

/// A new `ItemEntityWeapon` of `bp` wielded by `wielder` (`BlueprintItem.CreateEntity`):
/// its id is a fresh GUID in the original, here a stable `limb:` id.
pub fn create_weapon(w: &mut World, bp: wr_content::Guid, wielder: EntityIdx) -> EntityIdx {
    let item = crate::item::Item {
        blueprint: Some(bp),
        count: 1,
        charges: 0,
        is_identified: true,
        wielder: Some(wielder),
        collection_owner: None,
        holding_slot: None,
        collection: None,
        modifiers: Vec::new(),
        ability: None,
        activatable_ability: None,
        facts_applied_to_wielder: false,
        armor_component: None,
        weapon_component: None,
        shield: None,
        dex_limiter: None,
        force_secondary: false,
        is_non_removable: false,
        force_stackable: None,
        runtime_created: true,
        visual_source: None,
        remove_from_slot_in_progress: false,
        not_lootable: false,
        skinning_successful: false,
        inventory_slot_index: None,
    };
    let id = format!("limb:{}:{}", w.entity(wielder).unique_id, w.entities.len());
    let e = w.add_entity(crate::world::Entity {
        unique_id: id,
        clr: "ItemEntityWeapon",
        kind: crate::world::EntityKind::Item(Box::new(item)),
        facts: crate::world::FactsManager::default(),
        parts: Vec::new(),
        is_turned_on: true,
        post_load_executed: true,
        is_in_game: true,
        subscribed: false,
        disposing: false,
        is_disposed: false,
    });
    if w.content.get(bp).is_some_and(|b| !b.view().seq("m_Enchantments").is_empty()) {
        w.coverage.unsupported("AddAdditionalLimb: weapon with enchantments");
    }
    e
}

/// `RestrictionsHelper.CheckHasTwoWeapon(unit)` (`Kingmaker.UnitLogic.ActivatableAbilities.
/// Restrictions/RestrictionsHelper.cs`): both hands hold weapons and neither is natural or
/// the empty-hand weapon, or the primary hand is empty-handed with a shield, Improved
/// Unarmed Strike and Shield Bash.
pub fn check_has_two_weapon(w: &mut World, u: EntityIdx) -> bool {
    let primary = hand_weapon(w, u, Hand::Primary);
    let secondary = hand_weapon(w, u, Hand::Secondary);
    let empty = w.unit(u).and_then(|x| x.body.empty_hand);
    let natural = |w: &World, i: Option<EntityIdx>| {
        i.and_then(|i| Weapon::of(w, i)).is_some_and(|x| x.is_natural())
    };
    let flag =
        !natural(w, primary) && !natural(w, secondary) && primary != empty && secondary != empty;
    let secondary_shield = hand_slot(w, u, Hand::Secondary)
        .and_then(|(i, _)| i)
        .is_some_and(|i| w.entity(i).clr == "ItemEntityShield");
    let flag2 = primary == empty
        && secondary_shield
        && has_feature(w, u, IMPROVED_UNARMED_STRIKE)
        && has_feature(w, u, SHIELD_BASH);
    if primary.is_some() && secondary.is_some() { flag | flag2 } else { false }
}

/// `MechanicsFeatureType` values the combat components read.
pub mod feature {
    pub const MANYSHOT: i64 = 3;
    pub const SUPPRESSED_MANYSHOT: i64 = 99;
    pub const DOUBLE_SLICE: i64 = 13;
    pub const EFFORTLESS_DUAL_WIELDING: i64 = 38;
    pub const FAKE_ENHANCEMENT: i64 = 110;
    pub const ADVANCED_FAKE_ENHANCEMENT: i64 = 111;
}
