//! Combat-time component ports (decompiled, not copied). Files under
//! `Kingmaker.Designers.Mechanics.Facts/` unless noted:
//! `AddBuffOnCombatStart`; the attack bonus handlers `AttackStatReplacement`
//! (`Kingmaker.UnitLogic.FactLogic/`), `WeaponFocusParametrized`, `WeaponParametersAttackBonus`,
//! `TwoWeaponFightingAttackPenalty`, `DistanceAttackBonus`, `PreciseShot`; the weapon stats
//! handlers `WeaponTypeDamageStatReplacement`, `TwoWeaponFightingDamagePenalty`,
//! `ACBonusAgainstFactOwner` (target handler of `RuleAttackRoll` and `RuleCalculateAC`, with
//! `ACBonusAgainstAttacks.RuntimeData`); `WeaponEnhancementBonus`,
//! `WeaponDamageMultiplierStatReplacement`
//! (`Kingmaker.Designers.Mechanics.WeaponEnchants/`); `ImpatienceCalmingPart`
//! (`Kingmaker.Designers.Mechanics.Facts.Behavior/`, whose handlers are empty).
//!
//! Each port implements exactly the hooks and rule handlers its C# type overrides.
//! `OnEventDidTrigger` is empty in all of them. A handler that reads state the sim doesn't
//! model (`UnitPartDamageGrace` entries, `UnitPartWeaponTraining`, the unit state's
//! additional features) reports it where the C# would read it.

use super::attack::{
    Bonus, RuleAttackRoll, RuleCalculateAC, RuleCalculateAttackBonus,
    RuleCalculateAttackBonusWithoutTarget, RuleCalculateWeaponStats,
};
use super::weapon::{self, Hand, Weapon};
use crate::components::mechanics::RulePort;
use crate::components::{Port, bits, context_value, owner_unit, with_settings};
use crate::registry::{Hook, Registry};
use crate::rulebook::RuleEvent;
use crate::stats::ops::attribute_bonus;
use crate::stats::types::{ModifierDescriptor as MD, StatType};
use crate::world::{CompRef, EntityIdx, FactKind, World};

fn no_hooks(_: &mut World, _: CompRef, _: Hook) {}

pub fn register(r: &mut Registry) {
    r.register(Box::new(RulePort {
        name: "AddBuffOnCombatStart",
        bits: bits(&[Hook::TurnOff]),
        hooks: add_buff_on_combat_start_hooks,
        handlers: &["IInitiatorRulebookHandler<RuleInitiativeRoll>"],
        rule: add_buff_on_combat_start,
    }));
    for (name, rule) in [
        ("AttackStatReplacement", attack_stat_replacement as RuleFn),
        ("WeaponFocusParametrized", weapon_focus_parametrized),
        ("WeaponParametersAttackBonus", weapon_parameters_attack_bonus),
        ("TwoWeaponFightingAttackPenalty", two_weapon_fighting_attack_penalty),
    ] {
        r.register(Box::new(RulePort {
            name,
            bits: 0,
            hooks: no_hooks,
            handlers: ATTACK_BONUS,
            rule,
        }));
    }
    r.register(Box::new(RulePort {
        name: "AddBackgroundWeaponProficiency",
        bits: bits(&[Hook::TurnOn, Hook::TurnOff]),
        hooks: crate::components::state::add_background_weapon_proficiency,
        handlers: ATTACK_BONUS,
        rule: add_background_weapon_proficiency,
    }));
    for (name, rule) in
        [("DistanceAttackBonus", distance_attack_bonus as RuleFn), ("PreciseShot", precise_shot)]
    {
        r.register(Box::new(RulePort {
            name,
            bits: 0,
            hooks: no_hooks,
            handlers: &["IInitiatorRulebookHandler<RuleCalculateAttackBonus>"],
            rule,
        }));
    }
    for (name, rule) in [
        ("WeaponTypeDamageStatReplacement", weapon_type_damage_stat_replacement as RuleFn),
        ("TwoWeaponFightingDamagePenalty", two_weapon_fighting_damage_penalty),
        ("WeaponEnhancementBonus", weapon_enhancement_bonus),
        ("WeaponDamageMultiplierStatReplacement", weapon_damage_multiplier_stat_replacement),
    ] {
        r.register(Box::new(RulePort {
            name,
            bits: 0,
            hooks: no_hooks,
            handlers: &["IInitiatorRulebookHandler<RuleCalculateWeaponStats>"],
            rule,
        }));
    }
    r.register(Box::new(RulePort {
        name: "DistanceDamageBonus",
        bits: 0,
        hooks: no_hooks,
        handlers: &["IInitiatorRulebookHandler<RuleDealDamage>"],
        rule: distance_damage_bonus,
    }));
    r.register(Box::new(Port {
        name: "DeathActions",
        bits: 0,
        f: no_hooks,
        handlers: &["IUnitHandler", "IUnitSpawnHandler", "IUnitLifeStateChanged"],
        ev: Some(death_actions_event),
    }));
    r.register(Box::new(Port {
        name: "EvaluatedUnitDeathTrigger",
        bits: 0,
        f: no_hooks,
        handlers: &["IUnitHandler", "IUnitSpawnHandler"],
        ev: Some(evaluated_unit_death_trigger_event),
    }));
    r.register(Box::new(Port::new(
        "DropLootAndDestroyOnDeactivate",
        bits(&[Hook::Activate, Hook::Deactivate]),
        drop_loot_and_destroy,
    )));
    r.register(Box::new(Port {
        name: "CombatStateTrigger",
        bits: 0,
        f: no_hooks,
        handlers: &["IUnitCombatHandler"],
        ev: Some(combat_state_trigger_event),
    }));
    r.register(Box::new(Port {
        name: "EvaluatedUnitCombatTrigger",
        bits: 0,
        f: no_hooks,
        handlers: &["IUnitCombatHandler"],
        ev: Some(evaluated_unit_combat_trigger_event),
    }));
    r.register(Box::new(RulePort {
        name: "WeaponMaterial",
        bits: 0,
        hooks: no_hooks,
        handlers: &["IInitiatorRulebookHandler<RulePrepareDamage>"],
        rule: weapon_material,
    }));
    r.register(Box::new(RulePort {
        name: "ACBonusAgainstFactOwner",
        bits: 0,
        hooks: no_hooks,
        handlers: &[
            "ITargetRulebookHandler<RuleAttackRoll>",
            "ITargetRulebookHandler<RuleCalculateAC>",
        ],
        rule: ac_bonus_against_fact_owner,
    }));
    r.register(Box::new(RulePort {
        name: "ImpatienceCalmingPart",
        bits: 0,
        hooks: no_hooks,
        handlers: &["IInitiatorRulebookHandler<RuleAttackRoll>"],
        rule: |_, _, _, _| {},
    }));
}

// ---------------------------------------------------------------------------------------
// ACBonusAgainstFactOwner
// ---------------------------------------------------------------------------------------

/// `AlignmentExtension.HasComponent(alignment, component)`: `AlignmentComponent.None` (0)
/// always matches, otherwise any shared bit (`Neutral` 1, `Good` 2, `Evil` 4, `Lawful` 8,
/// `Chaotic` 0x10 against the `Alignment` flags).
pub(crate) fn alignment_has_component(alignment: i32, component: i32) -> bool {
    component == 0 || (alignment & component) > 0
}

/// `ACBonusAgainstAttacks.RuntimeData`: `ACModifier` and `Initiator` (`UnitReference`, by
/// unique id). `Data` creates it on first access.
fn ac_data(w: &mut World, c: CompRef) -> (i32, Option<String>) {
    let d = w.component_mut(c).data.get_or_insert_with(
        || serde_json::json!({ "ACModifier": 0, "Initiator": serde_json::Value::Null }),
    );
    let m = d.get("ACModifier").and_then(serde_json::Value::as_i64).unwrap_or(0);
    let i = d.get("Initiator").and_then(serde_json::Value::as_str).map(str::to_owned);
    (i32::try_from(m).unwrap_or(0), i)
}

fn set_ac_data(w: &mut World, c: CompRef, modifier: i32, initiator: Option<&str>) {
    w.component_mut(c).data = Some(serde_json::json!({
        "ACModifier": modifier,
        "Initiator": initiator,
    }));
}

/// `Data.Clear()`.
fn ac_data_clear(w: &mut World, c: CompRef) {
    ac_data(w, c);
    set_ac_data(w, c, 0, None);
}

/// `ACBonusAgainstFactOwner.OnEventAboutToTrigger(RuleAttackRoll)`.
fn ac_bonus_attack_roll_about(w: &mut World, c: CompRef, initiator: EntityIdx) {
    let Some((bonus, checked, no_fact, alignment)) = with_settings(w, c, |v| {
        (v.i32("Bonus"), v.guid("m_CheckedFact"), v.bool("NoFact"), v.i32("Alignment"))
    }) else {
        return;
    };
    let has = has_fact(w, initiator, checked);
    if !((!no_fact && has) || (no_fact && !has)) {
        return;
    }
    // `Descriptor.Alignment.ValueRaw`: read only when the component isn't `None`.
    if alignment != 0 {
        let Some(a) = w.unit(initiator).and_then(|u| u.alignment.as_ref()) else {
            w.coverage.unsupported("ACBonusAgainstFactOwner: initiator's alignment not modelled");
            return;
        };
        // (`ValueRaw` is `GetAlignment(VectorRaw, m_LockedAlignmentMask)`: a lock's mask isn't
        // ported.)
        if a.locked_mask != 0 {
            w.coverage.unsupported("ACBonusAgainstFactOwner: a locked alignment mask");
            return;
        }
        let raw = crate::narrative::alignment::enum_value(a.value());
        if !alignment_has_component(raw, alignment) {
            return;
        }
    }
    let rank = w.fact(c.fact).rank;
    let id = w.entity(initiator).unique_id.clone();
    ac_data(w, c);
    set_ac_data(w, c, crate::csharp::arith::mul(bonus, rank), Some(&id));
}

/// `ACBonusAgainstFactOwner.OnEventAboutToTrigger(RuleCalculateAC)`: the stored initiator's
/// bonus, `AddModifier(Data.ACModifier, Fact, Descriptor)`.
fn ac_bonus_calculate_ac_about(w: &mut World, c: CompRef, evt: &mut RuleCalculateAC) {
    let (modifier, initiator) = ac_data(w, c);
    if initiator.as_deref() == Some(w.entity(evt.initiator).unique_id.as_str()) {
        let Some(d) = with_settings(w, c, |v| MD(v.i32("Descriptor"))) else { return };
        evt.bonuses.add_in(w, Bonus::desc(modifier, d).of(c.fact));
    }
}

/// `ACBonusAgainstFactOwner` (`ITargetRulebookHandler<RuleAttackRoll>` and
/// `<RuleCalculateAC>`). `OnEventDidTrigger(RuleCalculateAC)` is empty;
/// `OnEventDidTrigger(RuleAttackRoll)` is `Data.Clear()`.
fn ac_bonus_against_fact_owner(w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
    if let Some(evt) = rule.as_any().downcast_mut::<RuleAttackRoll>() {
        if about {
            let initiator = evt.initiator;
            ac_bonus_attack_roll_about(w, c, initiator);
        } else {
            ac_data_clear(w, c);
        }
    } else if about && let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateAC>() {
        ac_bonus_calculate_ac_about(w, c, evt);
    }
}

const ATTACK_BONUS: &[&str] = &["IInitiatorRulebookHandler<RuleCalculateAttackBonusWithoutTarget>"];

type RuleFn = fn(&mut World, CompRef, &mut dyn RuleEvent, bool);

// ---------------------------------------------------------------------------------------
// AddBuffOnCombatStart
// ---------------------------------------------------------------------------------------

/// `AddBuffOnCombatStart.OnTurnOff`: `if (!TacticalCombatHelper.IsActive)
/// Data.AppliedBuff?.Remove()` (tactical combat is never active here).
fn add_buff_on_combat_start_hooks(w: &mut World, c: CompRef, h: Hook) {
    if h != Hook::TurnOff {
        return;
    }
    let Some(u) = owner_unit(w, c) else { return };
    let buff =
        w.component(c).fact_refs.get("AppliedBuff").and_then(|v| v.first().copied()).flatten();
    if let Some(b) = buff
        && w.fact(b).owner == Some(u)
    {
        crate::unit_facts::remove(w, u, b);
    }
}

/// `AddBuffOnCombatStart.OnEventAboutToTrigger(RuleInitiativeRoll)`: for the party
/// (`CheckParty`) or for everyone else, `Data.AppliedBuff = Owner.AddBuff(Feature, Owner)`.
fn add_buff_on_combat_start(w: &mut World, c: CompRef, _rule: &mut dyn RuleEvent, about: bool) {
    if !about {
        return;
    }
    let Some(u) = owner_unit(w, c) else { return };
    let Some((check_party, feature)) =
        with_settings(w, c, |v| (v.bool("CheckParty"), v.guid("m_Feature")))
    else {
        return;
    };
    let player = w.unit(u).is_some_and(|x| x.is_player_faction);
    if (check_party && player) || (!check_party && !player) {
        let Some(bp) = feature.and_then(|g| w.content.get(g)) else { return };
        let ctx = crate::unit_facts::context(w, Some(u), Some(u), &bp);
        let buff = crate::unit_facts::add_buff(w, u, &bp, ctx, None);
        w.component_mut(c).fact_refs.insert("AppliedBuff".to_owned(), vec![buff]);
    }
}

// ---------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------

fn has_fact(w: &World, u: EntityIdx, g: Option<wr_content::Guid>) -> bool {
    g.is_some_and(|g| w.entity(u).facts.list.iter().any(|f| w.fact(*f).blueprint == Some(g)))
}

/// `Descriptor.Ensure<UnitPartDamageGrace>().HasEntry(category)`: `Ensure` adds an empty part
/// to a unit without one.
fn damage_grace_has_entry(w: &mut World, u: EntityIdx, category: i64) -> bool {
    if w.unit(u).is_some_and(|x| x.damage_grace.is_none()) {
        if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartDamageGrace") {
            w.coverage.unsupported("UnitPartDamageGrace.HasEntry: entries not hydrated");
            return false;
        }
        w.entity_mut(u).parts.push(crate::world::Part { clr: "UnitPartDamageGrace" });
        if let Some(x) = w.unit_mut(u) {
            x.damage_grace = Some(Vec::new());
        }
    }
    w.unit(u).and_then(|x| x.damage_grace.as_ref()).is_some_and(|v| v.contains(&category))
}

fn weapon_stat_bonus_is_attribute(stat: StatType) -> bool {
    stat.is_attribute()
}

// ---------------------------------------------------------------------------------------
// RuleCalculateAttackBonusWithoutTarget handlers
// ---------------------------------------------------------------------------------------

/// `AttackStatReplacement.OnEventAboutToTrigger`.
fn attack_stat_replacement(w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateAttackBonusWithoutTarget>() else {
        return;
    };
    let Some(u) = owner_unit(w, c) else { return };
    let Some((replacement, sub, check_types, types)) = with_settings(w, c, |v| {
        (
            StatType(v.i32("ReplacementStat")),
            v.int("SubCategory"),
            v.bool("CheckWeaponTypes"),
            v.guids("m_WeaponTypes"),
        )
    }) else {
        return;
    };
    let cur = evt.attack_bonus_stat;
    let flag = weapon_stat_bonus_is_attribute(replacement)
        && weapon_stat_bonus_is_attribute(cur)
        && attribute_bonus(w, u, replacement) >= attribute_bonus(w, u, cur);
    let cond = if check_types {
        types.contains(&evt.weapon.ty.guid)
    } else {
        crate::feature_param::has_sub_category(evt.weapon.category(), sub)
            || (sub == weapon::SUB_FINESSABLE
                && damage_grace_has_entry(w, u, evt.weapon.category()))
    };
    if cond & flag {
        evt.attack_bonus_stat = replacement;
    }
}

/// `AddBackgroundWeaponProficiency.OnEventAboutToTrigger`: the stack bonus for a weapon of the
/// proficiency's category when the owner has that proficiency from another source too
/// (`UnitProficiency.IsBackgroundStacks`: a background entry, and the weapon set counts it
/// more than once).
fn add_background_weapon_proficiency(
    w: &mut World,
    c: CompRef,
    rule: &mut dyn RuleEvent,
    about: bool,
) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateAttackBonusWithoutTarget>() else {
        return;
    };
    let Some(u) = owner_unit(w, c) else { return };
    let Some((proficiency, descriptor)) =
        with_settings(w, c, |v| (v.int("Proficiency"), MD(v.i32("StackBonusType"))))
    else {
        return;
    };
    let category = evt.weapon.category();
    let count = |set: &[(i64, i32)]| set.iter().find(|(k, _)| *k == category).map_or(0, |e| e.1);
    let stacks = w.unit(u).is_some_and(|x| {
        count(&x.proficiencies.background_weapons) > 0 && count(&x.proficiencies.weapons) > 1
    });
    if category == proficiency && stacks {
        let v = context_value_of(w, c, "StackBonus");
        evt.bonuses.add_in(w, Bonus::desc(v, descriptor).of(c.fact));
    }
}

/// `WeaponFocusParametrized.OnEventAboutToTrigger`: +1 (+2 with the mythic focus of the
/// same param) for weapons of the feature's category.
fn weapon_focus_parametrized(w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateAttackBonusWithoutTarget>() else {
        return;
    };
    let Some(u) = owner_unit(w, c) else { return };
    let Some((mythic, descriptor)) =
        with_settings(w, c, |v| (v.guid("m_MythicFocus"), MD(v.i32("Descriptor"))))
    else {
        return;
    };
    let param = crate::feature_param::fact_param(w, c.fact);
    let facts = w.entity(u).facts.list.clone();
    let mythic_focus = facts.into_iter().any(|f| {
        w.fact(f).kind == FactKind::Feature
            && mythic.is_some()
            && w.fact(f).blueprint == mythic
            && crate::feature_param::fact_param(w, f) == param
    });
    let bonus = if mythic_focus { 2 } else { 1 };
    if param == crate::feature_param::FeatureParam::of_weapon_category(evt.weapon.category()) {
        evt.bonuses.add_in(w, Bonus::desc(bonus, descriptor).of(c.fact));
    }
}

/// `WeaponParametersAttackBonus.OnEventAboutToTrigger`.
fn weapon_parameters_attack_bonus(
    w: &mut World,
    c: CompRef,
    rule: &mut dyn RuleEvent,
    about: bool,
) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateAttackBonusWithoutTarget>() else {
        return;
    };
    let Some(u) = owner_unit(w, c) else { return };
    struct S {
        specific: bool,
        weapon_bp: Option<wr_content::Guid>,
        only_finessable: bool,
        fighters_finesse_ok: bool,
        fighters_finesse: Option<wr_content::Guid>,
        ranged: bool,
        only_two_handed: bool,
        check_grip: bool,
        grip: i64,
        use_context: bool,
        attack_bonus: i32,
        scale_by_bab: bool,
        only_full: bool,
        multiplier_fact: Option<wr_content::Guid>,
        multiplier: i32,
        descriptor: MD,
    }
    let Some(s) = with_settings(w, c, |v| S {
        specific: v.bool("SpecificWeaponBlueprint"),
        weapon_bp: v.guid("m_WeaponBlueprint"),
        only_finessable: v.bool("OnlyFinessable"),
        fighters_finesse_ok: v.bool("CanBeUsedWithFightersFinesse"),
        fighters_finesse: v.guid("m_FightersFinesse"),
        ranged: v.bool("Ranged"),
        only_two_handed: v.bool("OnlyTwoHanded"),
        check_grip: v.bool("m_checkGripStyle"),
        grip: v.int("m_gripStyle"),
        use_context: v.bool("UseContextIstead"),
        attack_bonus: v.i32("AttackBonus"),
        scale_by_bab: v.bool("ScaleByBasicAttackBonus"),
        only_full: v.bool("OnlyForFullAttack"),
        multiplier_fact: v.guid("m_MultiplierFact"),
        multiplier: v.i32("Multiplier"),
        descriptor: MD(v.i32("Descriptor")),
    }) else {
        return;
    };
    if s.only_full {
        // `FastBombsFeature` (set by the bomb throwers' component) is not modelled.
        match evt.reason_full_attack {
            Some(false) | None => return,
            Some(true) => {}
        }
    }
    let mut num = s.attack_bonus;
    if s.use_context {
        num = context_value_of(w, c, "ContextAttackBonus");
    }
    if s.scale_by_bab {
        let bab = w.stats(u).v(StatType::BaseAttackBonus).modified_value;
        let scale = crate::csharp::arith::add(1, bab / 4);
        num = crate::csharp::arith::mul(num, scale);
    }
    let category = evt.weapon.category();
    let finessable = crate::feature_param::has_sub_category(category, weapon::SUB_FINESSABLE);
    let flag = if s.only_finessable && !finessable && !damage_grace_has_entry(w, u, category) {
        if s.fighters_finesse_ok && has_fact(w, u, s.fighters_finesse) {
            if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartWeaponTraining") {
                w.coverage.unsupported("UnitPartWeaponTraining.GetWeaponRank");
            }
            false
        } else {
            false
        }
    } else {
        true
    };
    let is_ranged = evt.weapon.is_ranged();
    let flag2 = (s.ranged && is_ranged) || (!s.ranged && !is_ranged);
    let flag3 = !s.only_two_handed || evt.weapon.is_two_handed();
    let flag4 = s.specific && s.weapon_bp == Some(evt.weapon.bp.guid);
    let two = weapon::hold_in_two_hands(w, &evt.weapon);
    let grip_ok = !s.check_grip || (s.grip == 2 && two) || (s.grip == 1 && !two);
    let mult = if has_fact(w, u, s.multiplier_fact) { s.multiplier } else { 1 };
    if (grip_ok & flag & flag2 & flag3 && !s.specific) | flag4 {
        let rank = crate::components::fact_rank(w, c.fact);
        let v = crate::csharp::arith::mul(crate::csharp::arith::mul(num, rank), mult);
        evt.bonuses.add_in(w, Bonus::desc(v, s.descriptor).of(c.fact));
    }
}

fn context_value_of(w: &mut World, c: CompRef, field: &str) -> i32 {
    let Some((bp, i)) = crate::components::settings_of(w, c) else { return 0 };
    let objs = bp.components_array();
    let cv = crate::content::View(objs[i]).obj(field);
    context_value(w, c, cv)
}

/// `TwoWeaponFightingAttackPenalty.OnEventAboutToTrigger`.
fn two_weapon_fighting_attack_penalty(
    w: &mut World,
    c: CompRef,
    rule: &mut dyn RuleEvent,
    about: bool,
) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateAttackBonusWithoutTarget>() else {
        return;
    };
    let Some(u) = owner_unit(w, c) else { return };
    let Some(mythic) = with_settings(w, c, |v| v.guid("m_MythicBlueprint")) else { return };
    if !weapon::check_has_two_weapon(w, u) {
        return;
    }
    let primary = weapon::hand_weapon(w, u, Hand::Primary);
    let secondary = weapon::hand_weapon(w, u, Hand::Secondary);
    if primary != Some(evt.weapon.item) && secondary != Some(evt.weapon.item) {
        return;
    }
    // `flag2 & flag3`: ShieldMaster (a unit state feature the sim doesn't model) with a
    // shield in a hand.
    let shield = |w: &World, i: Option<EntityIdx>| {
        i.is_some_and(|i| w.item(i).is_some_and(|x| x.shield.is_some()))
    };
    if shield(w, primary) || shield(w, secondary) {
        w.coverage.unsupported("TwoWeaponFightingAttackPenalty: ShieldMaster with a shield");
    }
    if evt.reason_full_attack == Some(false) {
        return;
    }
    let rank = crate::components::fact_rank(w, c.fact);
    let mythic_has = has_fact(w, u, mythic);
    let num = if rank <= 1 {
        -4
    } else if mythic_has {
        0
    } else {
        -2
    };
    let num2 = if rank <= 1 {
        -8
    } else if mythic_has {
        0
    } else {
        -2
    };
    let mut num3 = if Some(evt.weapon.item) == primary { num } else { num2 };
    let effortless = weapon::has_feature(w, u, weapon::feature::EFFORTLESS_DUAL_WIELDING);
    let mut flag4 = false;
    if effortless {
        if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartWeaponTraining") {
            w.coverage.unsupported("UnitPartWeaponTraining.IsSuitableWeapon");
        }
        flag4 = false;
    }
    let sec = secondary.and_then(|i| Weapon::of(w, i));
    let pri = primary.and_then(|i| Weapon::of(w, i));
    let light = sec.as_ref().is_some_and(|x| x.ty.view().bool("m_IsLight"));
    let double = pri.as_ref().is_some_and(Weapon::is_double);
    if !light && !double && !flag4 {
        num3 = crate::csharp::arith::add(num3, -2);
    }
    evt.bonuses.add_in(w, Bonus::desc(num3, MD::UntypedStackable).of(c.fact));
}

// ---------------------------------------------------------------------------------------
// RuleCalculateAttackBonus handlers
// ---------------------------------------------------------------------------------------

/// `DistanceAttackBonus.OnEventAboutToTrigger(RuleCalculateAttackBonus)`.
fn distance_attack_bonus(w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateAttackBonus>() else { return };
    let Some((range, bonus, descriptor)) = with_settings(w, c, |v| {
        (super::units::feet_meters(v, "Range"), v.i32("AttackBonus"), MD(v.i32("Descriptor")))
    }) else {
        return;
    };
    if !evt.inner.weapon.is_ranged() {
        return;
    }
    // `Target.DistanceTo(Initiator) <= Range.Meters + Target.Corpulence + Initiator.Corpulence`.
    // Corpulence (the views' radii) is not modelled: the distance decides only when it is
    // within the range alone (corpulence is not negative).
    let Some((a, t)) = evt.env.positions else {
        w.coverage.unsupported("DistanceAttackBonus: unit positions not recorded");
        return;
    };
    // Both corpulences are at least 0.5 (`UnitEntityView.Corpulence`).
    if super::units::mechanics_distance(a, t) <= range + 1.0 {
        evt.bonuses.add_in(w, Bonus::desc(bonus, descriptor).of(c.fact));
    } else {
        w.coverage.unsupported("DistanceAttackBonus: range beyond the units' corpulence");
    }
}

/// `PreciseShot.OnEventAboutToTrigger`: `evt.IgnoreRangedPenalty = true`.
fn precise_shot(_w: &mut World, _c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
    if !about {
        return;
    }
    if let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateAttackBonus>() {
        evt.ignore_ranged_penalty = true;
    }
}

// ---------------------------------------------------------------------------------------
// RuleCalculateWeaponStats handlers
// ---------------------------------------------------------------------------------------

/// `WeaponTypeDamageStatReplacement.OnEventAboutToTrigger`.
fn weapon_type_damage_stat_replacement(
    w: &mut World,
    c: CompRef,
    rule: &mut dyn RuleEvent,
    about: bool,
) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateWeaponStats>() else { return };
    let Some(u) = owner_unit(w, c) else { return };
    let Some((stat, category, only_one_handed, two_handed_bonus)) = with_settings(w, c, |v| {
        (
            StatType(v.i32("Stat")),
            v.int("Category"),
            v.bool("OnlyOneHanded"),
            v.bool("TwoHandedBonus"),
        )
    }) else {
        return;
    };
    if !only_one_handed || !weapon::hand_has_item(w, u, Hand::Secondary) {
        // `modifiableValueAttributeStat2 == null` for no damage bonus stat or a non-attribute one.
        let current = evt.damage_bonus_stat.filter(|s| s.is_attribute());
        let better = stat.is_attribute()
            && current.is_none_or(|cur| attribute_bonus(w, u, stat) > attribute_bonus(w, u, cur));
        if better && evt.weapon.category() == category {
            evt.damage_bonus_stat = Some(stat);
        }
        if two_handed_bonus {
            evt.two_handed_stat_replacement = true;
        }
    }
}

/// `TwoWeaponFightingDamagePenalty.OnEventAboutToTrigger`.
fn two_weapon_fighting_damage_penalty(
    w: &mut World,
    c: CompRef,
    rule: &mut dyn RuleEvent,
    about: bool,
) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateWeaponStats>() else { return };
    let Some(u) = owner_unit(w, c) else { return };
    let secondary = weapon::hand_weapon(w, u, Hand::Secondary);
    let two = weapon::check_has_two_weapon(w, u);
    if two
        && secondary == Some(evt.weapon.item)
        && !weapon::has_feature(w, u, weapon::feature::DOUBLE_SLICE)
    {
        evt.half_damage_bonus = true;
    }
}

/// `WeaponEnhancementBonus.OnEventAboutToTrigger`: an enhancement enchantment of the weapon
/// being used.
fn weapon_enhancement_bonus(w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateWeaponStats>() else { return };
    let Some((bonus, stack)) =
        with_settings(w, c, |v| (v.i32("EnhancementBonus"), v.bool("Stack")))
    else {
        return;
    };
    let item = w.fact(c.fact).owner;
    if item != Some(evt.weapon.item) {
        return;
    }
    let descriptor = if stack { MD::UntypedStackable } else { MD::Enhancement };
    evt.enhancement.add_modifier(Bonus::desc(bonus, descriptor));
    evt.enhancement_total = crate::csharp::arith::add(evt.enhancement_total, bonus);
    let wielder = w.item(evt.weapon.item).and_then(|i| i.wielder);
    if wielder
        .is_some_and(|x| weapon::has_feature(w, x, weapon::feature::ADVANCED_FAKE_ENHANCEMENT))
    {
        evt.enhancement.add_extra_modifier(Bonus::desc(2, MD::None));
        evt.enhancement_total = crate::csharp::arith::add(evt.enhancement_total, 2);
    } else if wielder.is_some_and(|x| weapon::has_feature(w, x, weapon::feature::FAKE_ENHANCEMENT))
    {
        evt.enhancement.add_extra_modifier(Bonus::desc(1, MD::None));
        evt.enhancement_total = crate::csharp::arith::add(evt.enhancement_total, 1);
    }
}

/// `WeaponDamageMultiplierStatReplacement.OnEventAboutToTrigger`.
fn weapon_damage_multiplier_stat_replacement(
    w: &mut World,
    c: CompRef,
    rule: &mut dyn RuleEvent,
    about: bool,
) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<RuleCalculateWeaponStats>() else { return };
    let Some((stat, multiplier)) =
        with_settings(w, c, |v| (StatType(v.i32("Stat")), v.f32("Multiplier")))
    else {
        return;
    };
    let item = w.fact(c.fact).owner;
    if item == Some(evt.weapon.item) && stat.is_attribute() {
        evt.damage_bonus_stat = Some(stat);
        evt.override_damage_bonus = Some(multiplier);
    }
}

// ---------------------------------------------------------------------------------------
// Death
// ---------------------------------------------------------------------------------------

/// `DeathActions.HandleUnitDeath` / `HandleUnitLifeStateChanged`: for the owner's own death,
/// the actions run in the fact's context. `HandleUnitSpawned`/`Destroyed` are empty.
fn death_actions_event(w: &mut World, c: CompRef, _iface: &str) {
    let Some((method, Some(unit))) = w.bus.call.last().copied() else {
        w.coverage.unsupported("DeathActions: event without a unit argument");
        return;
    };
    let Some((trigger, with_unconscious, only_party, check_resource, only_natural)) =
        with_settings(w, c, |v| {
            (
                v.int("m_DeathTrigger"),
                v.bool("m_WithUnconsciousLifeState"),
                v.bool("OnlyOnParty"),
                v.bool("CheckResource"),
                v.bool("OnlyNaturalDeath"),
            )
        })
    else {
        return;
    };
    let fire = match method {
        "HandleUnitDeath" => trigger == 0,
        "HandleUnitLifeStateChanged" => {
            trigger == 1
                && w.unit(unit).is_some_and(|x| {
                    x.life_state == super::life::state::DEAD
                        || (with_unconscious && x.life_state == super::life::state::UNCONSCIOUS)
                })
        }
        _ => false,
    };
    if !fire || owner_unit(w, c) != Some(unit) {
        return;
    }
    if only_party && !crate::narrative::party::character_lists(w).party_and_pets.contains(&unit) {
        return;
    }
    if check_resource {
        w.coverage.unsupported("DeathActions: CheckResource");
        return;
    }
    if only_natural && w.unit(unit).is_some_and(|x| x.force_kill) {
        return;
    }
    let Some((bp, i)) = crate::components::settings_of(w, c) else { return };
    let objs = bp.components_array();
    let actions = crate::content::View(objs[i]).obj("Actions");
    if actions.is_some_and(|a| !a.objs("Actions").is_empty()) {
        w.coverage.unsupported("DeathActions: actions run in the fact's context");
    }
}

/// `EvaluatedUnitDeathTrigger.HandleUnitDeath`: the actions run for any unit or for the unit
/// the evaluator names.
fn evaluated_unit_death_trigger_event(w: &mut World, c: CompRef, _iface: &str) {
    let Some((method, Some(unit))) = w.bus.call.last().copied() else { return };
    if method != "HandleUnitDeath" {
        return;
    }
    let Some((bp, i)) = crate::components::settings_of(w, c) else { return };
    let objs = bp.components_array();
    let v = crate::content::View(objs[i]);
    let any = v.bool("AnyUnit");
    let evaluator = v.obj("Unit");
    let run = if any || evaluator.is_none() {
        true
    } else {
        // `Unit.Is(entityData)`: the evaluator's value is the dead unit; a failing
        // evaluator is false.
        match crate::elements::try_get_unit(w, &bp, evaluator) {
            Ok(Some(u)) => u == unit,
            _ => false,
        }
    };
    if run && v.obj("Actions").is_some_and(|a| !a.objs("Actions").is_empty()) {
        w.coverage.unsupported("EvaluatedUnitDeathTrigger: actions run with FactData/DeadUnitData");
    }
}

/// `DropLootAndDestroyOnDeactivate.OnActivate`/`OnDeactivate` (a buff component): a timer
/// for `Buff.TimeLeft` removes the buff (see [`super::Timer`]); non-player owners drop their loot; on deactivation
/// the timer is disposed and the owner leaves the game (a summoned monster is destroyed).
fn drop_loot_and_destroy(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::Activate => {
            let end = w.fact(c.fact).buff.as_ref().and_then(|b| b.end_time);
            let left = match end {
                None => i64::MAX,
                Some(e) if e == i64::MAX => i64::MAX,
                Some(e) => {
                    let now = w.clock.game_time.0;
                    if e >= now { e - now } else { 0 }
                }
            };
            let due = w.clock.real_time.0.saturating_add(left);
            w.combat.timers.push(super::Timer { due, fact: c.fact });
            if left != i64::MAX && !w.combat.timer_frames_recorded {
                // UniRx's main-thread timer waits on Unity's `Time.time` in a coroutine (after
                // `Game.Tick`); neither clock is the sim's, so the frame it fires in isn't
                // established. The sim fires it after the frame's controllers once
                // `Player.RealTime` has passed it.
                w.coverage.unsupported(
                    "DropLootAndDestroyOnDeactivate: the UniRx timer's frame (Unity Time.time) is not established",
                );
            }
            if !w.unit(u).is_some_and(|x| x.is_player_faction) {
                super::loot::drop_loot_to_ground(w, u, true, false);
            }
        }
        Hook::Deactivate => {
            w.combat.timers.retain(|t| t.fact != c.fact);
            if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartSummonedMonster") {
                w.coverage.unsupported("DropLootAndDestroyOnDeactivate: MarkForDestroy");
            } else if !w.unit(u).is_some_and(|x| x.is_player_faction) {
                super::life::set_in_game(w, u, false);
            }
        }
        _ => {}
    }
}

/// `WeaponMaterial.OnEventAboutToTrigger(RulePrepareDamage)`: the weapon's physical damage
/// gains the enchantment's material.
fn weapon_material(w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<super::attack::RulePrepareDamage>() else {
        return;
    };
    let Some(material) = with_settings(w, c, |v| v.int("Material")) else { return };
    let owner = w.fact(c.fact).owner;
    if evt.weapon_item.is_some() && evt.weapon_item == owner {
        // `WeaponDamage` is the bundle's first damage; the weapon's damage is physical.
        if let Some(first) = evt.bundle.first_mut()
            && first.type_desc.ty == 0
        {
            first.type_desc.material |= material;
        }
    }
}

/// `DistanceDamageBonus.OnEventAboutToTrigger(RuleDealDamage)`.
fn distance_damage_bonus(w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
    if !about {
        return;
    }
    let Some(evt) = rule.as_any().downcast_mut::<super::attack::RuleDealDamage>() else { return };
    let Some((range, bonus)) =
        with_settings(w, c, |v| (super::units::feet_meters(v, "Range"), v.i32("DamageBonus")))
    else {
        return;
    };
    let caster = w
        .fact(c.fact)
        .context
        .as_ref()
        .and_then(|x| x.caster.clone())
        .and_then(|id| w.by_id.get(&id).copied());
    if caster
        .is_some_and(|k| w.entity(k).parts.iter().any(|p| p.clr == "UnitPartDisableBonusForDamage"))
    {
        w.coverage.unsupported("DistanceDamageBonus: UnitPartDisableBonusForDamage");
        return;
    }
    // `Reason.Ability` is null for weapon attacks: only a ranged weapon qualifies.
    if evt.weapon_ranged != Some(true) {
        return;
    }
    let applies = match evt.positions {
        // Both corpulences are at least 0.5 (`UnitEntityView.Corpulence`).
        Some((a, t)) if super::units::mechanics_distance(a, t) <= range + 1.0 => true,
        Some(_) => {
            w.coverage.unsupported("DistanceDamageBonus: range beyond the units' corpulence");
            false
        }
        // The positions aren't established: a driven replay may supply the recorded outcome
        // of this handler (and then counts the rule as unverified).
        None => {
            let id = w.fact(c.fact).unique_id.clone();
            if let Some(applied) = w.combat.recorded_distance.get(&id).copied() {
                w.combat.recorded_distance_used += 1;
                applied
            } else {
                w.coverage.unsupported("DistanceDamageBonus: unit positions not recorded");
                false
            }
        }
    };
    if applies && let Some(first) = evt.bundle.first_mut() {
        first.add_modifier_target_related(Bonus::desc(bonus, MD::UntypedStackable));
    }
}

/// `CombatStateTrigger.HandleUnitJoinCombat`/`HandleUnitLeaveCombat`: for the owner itself, the
/// start or end actions run in the fact's context (`(Fact as IFactContextOwner)?.
/// RunActionInContext(list, Owner)`, inside `Fact.MaybeContext?.GetDataScope(Owner)`: the same
/// scope twice).
fn combat_state_trigger_event(w: &mut World, c: CompRef, _iface: &str) {
    let Some((method, Some(unit))) = w.bus.call.last().copied() else { return };
    let list = match method {
        "HandleUnitJoinCombat" => "CombatStartActions",
        "HandleUnitLeaveCombat" => "CombatEndActions",
        _ => return,
    };
    if owner_unit(w, c) != Some(unit) {
        return;
    }
    let Some((bp, i)) = crate::components::settings_of(w, c) else { return };
    let objs = bp.components_array();
    let actions = crate::content::View(objs[i]).obj(list);
    crate::components::mechanics::run_action_in_context(w, c.fact, &bp, actions, Some(unit));
}

/// `EvaluatedUnitCombatTrigger.HandleUnitJoinCombat`/`HandleUnitLeaveCombat`: in the fact's
/// `FactData` scope, when the evaluator can evaluate and names the unit, the actions run
/// (on a join without `TriggerOnExit`, on a leave with it) in a `UnitData` scope of the unit.
fn evaluated_unit_combat_trigger_event(w: &mut World, c: CompRef, _iface: &str) {
    let Some((method, Some(unit))) = w.bus.call.last().copied() else { return };
    let on_exit = match method {
        "HandleUnitJoinCombat" => false,
        "HandleUnitLeaveCombat" => true,
        _ => return,
    };
    let Some((bp, i)) = crate::components::settings_of(w, c) else { return };
    let objs = bp.components_array();
    let v = crate::content::View(objs[i]);
    let evaluator = v.obj("Unit");
    if evaluator.is_none() {
        return;
    }
    crate::elements::with_ctx(w, crate::elements::Ctx::Fact(c.fact), |w| {
        // `Unit.CanEvaluate()` (else a warning), then `Unit.Is(unit)`.
        let is_unit =
            matches!(crate::elements::try_get_unit(w, &bp, evaluator), Ok(Some(u)) if u == unit);
        if is_unit && v.bool("TriggerOnExit") == on_exit {
            crate::elements::with_ctx(w, crate::elements::Ctx::CombatUnit(unit), |w| {
                crate::elements::run_actions(w, &bp, v.obj("Actions"));
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use wr_content::Guid;
    use wr_content::node::Node;

    use crate::content::{Blueprint, Source};
    use crate::facts::tests::{feature, obj};
    use crate::rulebook::bus;
    use crate::world::{EntityIdx, World};

    fn node(ty: &str, set: &[(&str, Node)]) -> Node {
        Node::Object(Box::new(obj(ty, set)))
    }

    fn list(actions: Vec<Node>) -> Node {
        node("Kingmaker.ElementsSystem.ActionList", &[("Actions", Node::Seq(actions))])
    }

    fn remove_buff(buff: u128) -> Node {
        node(
            "Kingmaker.UnitLogic.Mechanics.Actions.ContextActionRemoveBuff",
            &[("m_Buff", Node::BpRef(Guid(buff)))],
        )
    }

    /// A world whose unit has `bp` as a fact and a buff `buff` (900) it can lose.
    fn world_with_fact(comp: Node) -> (World, EntityIdx, Rc<Blueprint>, crate::world::FactIdx) {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        w.registry = Rc::new(crate::components::registry());
        w.events.on = true;
        let buff = obj(
            "Kingmaker.UnitLogic.Buffs.Blueprints.BlueprintBuff",
            &[("Components", Node::Seq(Vec::new()))],
        );
        w.content.insert(Blueprint {
            guid: Guid(900),
            name: "Buff".into(),
            obj: buff,
            source: Source::Bbp,
        });
        let bp = feature(&w, 901, vec![comp]);
        let f = crate::unit_facts::add_fact(&mut w, u, &bp).expect("feature");
        // The unit isn't turned on in this world: subscribe the component by hand.
        bus::subscribe_component(&mut w, crate::world::CompRef { fact: f, comp: 0 });
        let b = w.content.get(Guid(900)).expect("buff");
        let ctx = crate::unit_facts::context(&w, Some(u), Some(u), &b);
        let buff = crate::unit_facts::add_buff(&mut w, u, &b, ctx, None).expect("buff");
        (w, u, bp, buff)
    }

    fn add_unit(w: &mut World) -> EntityIdx {
        w.add_entity(crate::world::Entity {
            unique_id: "u2".into(),
            clr: "UnitEntityData",
            kind: crate::world::EntityKind::Unit(Box::new(crate::unit::Unit {
                stats: crate::stats::value::Stats::fresh(),
                ..crate::unit::Unit::default()
            })),
            facts: crate::world::FactsManager::default(),
            parts: Vec::new(),
            is_turned_on: false,
            post_load_executed: true,
            is_in_game: true,
            subscribed: false,
            disposing: false,
            is_disposed: false,
        })
    }

    fn has(w: &World, u: EntityIdx, f: crate::world::FactIdx) -> bool {
        w.entity(u).facts.list.contains(&f)
    }

    /// `CombatStateTrigger`: the owner's join runs the start actions, its leave the end actions,
    /// in the fact's context (here a `ContextActionRemoveBuff` on the owner); another unit's
    /// event runs nothing.
    #[test]
    fn combat_state_trigger_runs_its_actions_in_the_facts_context() {
        let comp = node(
            "Kingmaker.Designers.Mechanics.Facts.CombatStateTrigger",
            &[
                ("CombatStartActions", list(vec![])),
                ("CombatEndActions", list(vec![remove_buff(900)])),
            ],
        );
        let (mut w, u, _, buff) = world_with_fact(comp);
        bus::raise_global_call(&mut w, "IUnitCombatHandler", "HandleUnitJoinCombat", Some(u));
        assert!(has(&w, u, buff), "the start list is empty");
        let other = add_unit(&mut w);
        bus::raise_global_call(&mut w, "IUnitCombatHandler", "HandleUnitLeaveCombat", Some(other));
        assert!(has(&w, u, buff), "another unit's event");
        bus::raise_global_call(&mut w, "IUnitCombatHandler", "HandleUnitLeaveCombat", Some(u));
        assert!(!has(&w, u, buff), "the end actions ran");
        assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
    }

    /// An action the sim has no port for is reported, not skipped.
    #[test]
    fn combat_state_trigger_reports_an_unported_action() {
        let comp = node(
            "Kingmaker.Designers.Mechanics.Facts.CombatStateTrigger",
            &[
                ("CombatStartActions", list(vec![])),
                (
                    "CombatEndActions",
                    list(vec![node(
                        "Kingmaker.UnitLogic.Mechanics.Actions.ContextActionHealTarget",
                        &[],
                    )]),
                ),
            ],
        );
        let (mut w, u, _, _) = world_with_fact(comp);
        bus::raise_global_call(&mut w, "IUnitCombatHandler", "HandleUnitLeaveCombat", Some(u));
        assert!(
            w.coverage.unsupported.keys().any(|k| k == "context action ContextActionHealTarget"),
            "{:?}",
            w.coverage.unsupported
        );
    }

    /// `ContextActionReduceDebilitatingBuffsDuration` at combat end: with the difficulty's stats
    /// adjustments at or below the action's, a debilitating buff lasting under 5 minutes ends a
    /// round from now; above it, nothing changes.
    #[test]
    fn debilitating_buffs_are_cut_to_a_round_at_combat_end() {
        let comp = |level: i64| {
            node(
                "Kingmaker.Designers.Mechanics.Facts.CombatStateTrigger",
                &[
                    ("CombatStartActions", list(vec![])),
                    (
                        "CombatEndActions",
                        list(vec![node(
                            "Kingmaker.UnitLogic.Mechanics.Actions.ContextActionReduceDebilitatingBuffsDuration",
                            &[("StatsAdjustmentsType", Node::Enum(level))],
                        )]),
                    ),
                ],
            )
        };
        const S: i64 = 10_000_000;
        for (setting, cut) in [(1, true), (2, false)] {
            let (mut w, u, _, buff) = world_with_fact(comp(1));
            w.settings.difficulty.settings.stats_adjustments = Some(setting);
            w.clock.game_time = crate::csharp::TimeSpan(1_000 * S);
            let f = w.fact_mut(buff);
            f.attach_time = crate::csharp::TimeSpan(990 * S);
            f.buff.as_mut().expect("times").end_time = Some(1_060 * S);
            // `SpellDescriptor.Shaken`.
            f.context.as_mut().expect("context").spell_descriptor = 0x40000;
            bus::raise_global_call(&mut w, "IUnitCombatHandler", "HandleUnitLeaveCombat", Some(u));
            let end = w.fact(buff).buff.as_ref().and_then(|t| t.end_time);
            assert_eq!(end, Some(if cut { 1_006 * S } else { 1_060 * S }), "setting {setting}");
            assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
        }
    }

    /// `AddFactContextActions` runs its lists in the fact's context (`RunActionInContext`), so a
    /// context action that needs the mechanics scope runs: a debilitating buff is cut on
    /// activation.
    #[test]
    fn add_fact_context_actions_run_in_the_facts_context() {
        const S: i64 = 10_000_000;
        let (mut w, u, _, buff) = world_with_fact(node(
            "Kingmaker.Designers.Mechanics.Facts.CombatStateTrigger",
            &[("CombatStartActions", list(vec![])), ("CombatEndActions", list(vec![]))],
        ));
        w.settings.difficulty.settings.stats_adjustments = Some(1);
        w.clock.game_time = crate::csharp::TimeSpan(1_000 * S);
        let f = w.fact_mut(buff);
        f.attach_time = crate::csharp::TimeSpan(990 * S);
        f.buff.as_mut().expect("times").end_time = Some(1_060 * S);
        f.context.as_mut().expect("context").spell_descriptor = 0x40000;
        let actions = node(
            "Kingmaker.UnitLogic.Mechanics.Components.AddFactContextActions",
            &[(
                "Activated",
                list(vec![node(
                    "Kingmaker.UnitLogic.Mechanics.Actions.ContextActionReduceDebilitatingBuffsDuration",
                    &[("StatsAdjustmentsType", Node::Enum(1))],
                )]),
            )],
        );
        let bp = feature(&w, 902, vec![actions]);
        crate::unit_facts::add_fact(&mut w, u, &bp).expect("feature");
        let end = w.fact(buff).buff.as_ref().and_then(|t| t.end_time);
        assert_eq!(end, Some(1_006 * S));
        assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
    }

    /// `FactOwner` resolves through `Buff.Data` when there is no `FactData`: a buff's
    /// `RunActionInContext` opens it.
    #[test]
    fn fact_owner_falls_back_to_the_buffs_owner() {
        let (mut w, u, bp, buff) = world_with_fact(node(
            "Kingmaker.Designers.Mechanics.Facts.CombatStateTrigger",
            &[("CombatStartActions", list(vec![])), ("CombatEndActions", list(vec![]))],
        ));
        let ev = obj("Kingmaker.Designers.EventConditionActionSystem.Evaluators.FactOwner", &[]);
        let view = crate::content::View(&ev);
        assert_eq!(crate::elements::try_get_unit(&mut w, &bp, Some(view)), Ok(None));
        let got = crate::elements::with_ctx(&mut w, crate::elements::Ctx::BuffData(buff), |w| {
            crate::elements::try_get_unit(w, &bp, Some(view))
        });
        assert_eq!(got, Ok(Some(u)));
    }

    /// `EvaluatedUnitCombatTrigger`: the evaluator runs in the fact's `FactData` scope (here
    /// `FactOwner`), and the actions in a `UnitData` scope of the unit that joined (here a
    /// `PlayAnimationOneShot` on `CombatUnit`). `TriggerOnExit` picks leave over join.
    #[test]
    fn evaluated_unit_combat_trigger_runs_with_fact_and_unit_data() {
        let anim = node(
            "Kingmaker.Designers.EventConditionActionSystem.Actions.PlayAnimationOneShot",
            &[(
                "Unit",
                node("Kingmaker.Designers.EventConditionActionSystem.Evaluators.CombatUnit", &[]),
            )],
        );
        let comp = |exit: bool| {
            node(
                "Kingmaker.Designers.EventConditionActionSystem.Events.EvaluatedUnitCombatTrigger",
                &[
                    (
                        "Unit",
                        node(
                            "Kingmaker.Designers.EventConditionActionSystem.Evaluators.FactOwner",
                            &[],
                        ),
                    ),
                    ("Actions", list(vec![anim.clone()])),
                    ("TriggerOnExit", Node::Bool(exit)),
                ],
            )
        };
        let animations = |w: &World| {
            w.events
                .records
                .iter()
                .filter(|r| r.fields.get("what") == Some(&"animation_one_shot".into()))
                .count()
        };
        let (mut w, u, _, _) = world_with_fact(comp(false));
        bus::raise_global_call(&mut w, "IUnitCombatHandler", "HandleUnitLeaveCombat", Some(u));
        assert_eq!(animations(&w), 0, "a leave without TriggerOnExit");
        let other = add_unit(&mut w);
        bus::raise_global_call(&mut w, "IUnitCombatHandler", "HandleUnitJoinCombat", Some(other));
        assert_eq!(animations(&w), 0, "the evaluator names the owner, not the other unit");
        bus::raise_global_call(&mut w, "IUnitCombatHandler", "HandleUnitJoinCombat", Some(u));
        assert_eq!(animations(&w), 1, "the actions ran with the unit in scope");
        let (mut w, u, _, _) = world_with_fact(comp(true));
        bus::raise_global_call(&mut w, "IUnitCombatHandler", "HandleUnitJoinCombat", Some(u));
        assert_eq!(animations(&w), 0);
        bus::raise_global_call(&mut w, "IUnitCombatHandler", "HandleUnitLeaveCombat", Some(u));
        assert_eq!(animations(&w), 1);
        assert!(w.ctx.is_empty(), "scopes closed");
    }

    // ---- ACBonusAgainstFactOwner ----

    use super::{
        ac_bonus_attack_roll_about, ac_bonus_calculate_ac_about, ac_data, ac_data_clear,
        alignment_has_component,
    };
    use crate::combat::attack::{BonusList, RuleCalculateAC};
    use crate::combat::units::AttackEnv;
    use crate::combat::weapon::AttackType;
    use crate::rulebook::RuleBase;
    use crate::stats::types::ModifierDescriptor as MD;
    use crate::world::{CompRef, Entity, EntityKind, FactsManager};

    const CHECKED: u128 = 950;

    fn ac_component(bonus: i64, no_fact: bool, alignment: i64) -> Node {
        node(
            "Kingmaker.Designers.Mechanics.Facts.ACBonusAgainstFactOwner",
            &[
                ("name", Node::Str("$ACBonusAgainstFactOwner$t".into())),
                ("m_CheckedFact", Node::BpRef(Guid(CHECKED))),
                ("Bonus", Node::Int(bonus)),
                ("Descriptor", Node::Enum(9)),
                ("Alignment", Node::Enum(alignment)),
                ("NoFact", Node::Bool(no_fact)),
            ],
        )
    }

    /// The owner `u`, an enemy initiator `e` and the checked fact's blueprint; the owner has
    /// the component's fact `c` (rank 3).
    fn ac_world(
        bonus: i64,
        no_fact: bool,
        alignment: i64,
    ) -> (World, EntityIdx, EntityIdx, CompRef, Rc<Blueprint>) {
        let (mut w, owner) = crate::stats::tests::world_with_unit();
        w.registry = Rc::new(crate::components::registry());
        let bp = feature(&w, 951, vec![ac_component(bonus, no_fact, alignment)]);
        let f = crate::unit_facts::add_fact(&mut w, owner, &bp).expect("feature");
        let c = CompRef { fact: f, comp: 0 };
        let e = w.add_entity(Entity {
            unique_id: "enemy".into(),
            clr: "UnitEntityData",
            kind: EntityKind::Unit(Box::new(crate::unit::Unit {
                stats: crate::stats::value::Stats::fresh(),
                ..crate::unit::Unit::default()
            })),
            facts: FactsManager::default(),
            parts: Vec::new(),
            is_turned_on: false,
            post_load_executed: true,
            is_in_game: true,
            subscribed: false,
            disposing: false,
            is_disposed: false,
        });
        w.fact_mut(c.fact).rank = 3;
        let checked = feature(&w, CHECKED, Vec::new());
        (w, owner, e, c, checked)
    }

    fn set_alignment(w: &mut World, u: EntityIdx, index: i32) {
        w.unit_mut(u).expect("unit").alignment =
            Some(crate::narrative::alignment::UnitAlignment::initialized(index));
    }

    fn ac_for(
        w: &mut World,
        c: CompRef,
        initiator: EntityIdx,
        target: EntityIdx,
    ) -> Vec<(i32, MD)> {
        let mut r = RuleCalculateAC {
            base: RuleBase::default(),
            initiator,
            target,
            attack_type: AttackType::Melee,
            force_flat_footed: false,
            is_critical: false,
            env: AttackEnv::neutral(),
            is_target_flat_footed: false,
            result: 0,
            bonuses: BonusList::default(),
        };
        ac_bonus_calculate_ac_about(w, c, &mut r);
        // (`AddModifier(.., Fact, ..)`: the component's fact is the bonus's source.)
        assert!(r.bonuses.0.iter().all(|b| b.fact == Some(c.fact)));
        r.bonuses.0.iter().map(|b| (b.value, b.descriptor)).collect()
    }

    #[test]
    fn alignment_has_component_matches_none_and_shared_bits() {
        // Lawful good is Lawful (8) | Good (2); `None` always matches.
        assert!(alignment_has_component(10, 0));
        assert!(alignment_has_component(10, 2));
        assert!(alignment_has_component(10, 8));
        assert!(!alignment_has_component(10, 4));
        assert!(!alignment_has_component(10, 0x10));
        assert!(alignment_has_component(1, 1), "true neutral is Neutral");
    }

    #[test]
    fn ac_bonus_is_added_against_the_stored_initiator_only_with_the_rank_multiplier() {
        let (mut w, owner, e, c, checked) = ac_world(2, false, 0);
        crate::unit_facts::add_fact(&mut w, e, &checked).expect("checked fact");
        ac_bonus_attack_roll_about(&mut w, c, e);
        assert_eq!(ac_data(&mut w, c), (6, Some("enemy".to_owned())), "Bonus * rank 3");
        assert_eq!(ac_for(&mut w, c, e, owner), vec![(6, MD(9))]);
        assert!(ac_for(&mut w, c, owner, owner).is_empty(), "another initiator gets nothing");
        // `Data.Clear()` after the attack roll.
        ac_data_clear(&mut w, c);
        assert_eq!(ac_data(&mut w, c), (0, None));
        assert!(ac_for(&mut w, c, e, owner).is_empty());
        assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
    }

    #[test]
    fn ac_bonus_needs_the_fact_or_with_no_fact_its_absence() {
        let (mut w, _, e, c, checked) = ac_world(2, false, 0);
        ac_bonus_attack_roll_about(&mut w, c, e);
        assert_eq!(ac_data(&mut w, c).1, None, "the initiator lacks the fact");
        crate::unit_facts::add_fact(&mut w, e, &checked).expect("checked fact");
        ac_bonus_attack_roll_about(&mut w, c, e);
        assert_eq!(ac_data(&mut w, c).1.as_deref(), Some("enemy"));

        let (mut w, _, e, c, checked) = ac_world(2, true, 0);
        ac_bonus_attack_roll_about(&mut w, c, e);
        assert_eq!(ac_data(&mut w, c), (6, Some("enemy".to_owned())), "NoFact: lacks it");
        ac_data_clear(&mut w, c);
        crate::unit_facts::add_fact(&mut w, e, &checked).expect("checked fact");
        ac_bonus_attack_roll_about(&mut w, c, e);
        assert_eq!(ac_data(&mut w, c).1, None, "NoFact: has it");
    }

    #[test]
    fn ac_bonus_checks_the_initiators_alignment_component() {
        // Alignment `Evil` (4): chaotic evil is Chaotic | Evil, lawful good isn't.
        let (mut w, _, e, c, checked) = ac_world(1, false, 4);
        crate::unit_facts::add_fact(&mut w, e, &checked).expect("checked fact");
        set_alignment(&mut w, e, 0);
        ac_bonus_attack_roll_about(&mut w, c, e);
        assert_eq!(ac_data(&mut w, c).1, None, "lawful good has no Evil");
        set_alignment(&mut w, e, 8);
        ac_bonus_attack_roll_about(&mut w, c, e);
        assert_eq!(ac_data(&mut w, c), (3, Some("enemy".to_owned())));
        assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
        // An alignment the sim doesn't hold is reported where the C# reads it.
        ac_data_clear(&mut w, c);
        w.unit_mut(e).expect("unit").alignment = None;
        ac_bonus_attack_roll_about(&mut w, c, e);
        assert_eq!(ac_data(&mut w, c).1, None);
        assert!(!w.coverage.valid());
        // A locked alignment (`ValueRaw` honours the mask): reported, not read unmasked.
        let (mut w, _, e, c, checked) = ac_world(1, false, 4);
        crate::unit_facts::add_fact(&mut w, e, &checked).expect("checked fact");
        set_alignment(&mut w, e, 8);
        w.unit_mut(e).expect("unit").alignment.as_mut().expect("alignment").locked_mask = 1;
        ac_bonus_attack_roll_about(&mut w, c, e);
        assert_eq!(ac_data(&mut w, c).1, None);
        assert!(w.coverage.unsupported.keys().any(|k| k.contains("locked alignment")));
    }
}
