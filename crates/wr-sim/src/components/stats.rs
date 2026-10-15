//! Components that put modifiers on stats. Sources (decompiled, not copied):
//! `Kingmaker.UnitLogic.FactLogic/AddStatBonus.cs`, `AddContextStatBonus.cs`,
//! `AddClassSkill.cs`, `AddBackgroundClassSkill.cs`,
//! `Kingmaker.Designers.Mechanics.EquipmentEnchants/AddStatBonusEquipment.cs`,
//! `AllSavesBonusEquipment.cs`, `Kingmaker.Designers.Mechanics.Buffs/AddStatBonusIfHasFact.cs`,
//! `ShieldFocus.cs`, `Kingmaker.Designers.Mechanics.Facts/DerivativeStatBonus.cs`,
//! `RecalculateOnStatChange.cs`.

use super::{Port, bits, context_value, fact_rank, owner_unit, wielder, with_settings};
use crate::csharp::arith;
use crate::registry::{Hook, Registry};
use crate::stats::ops;
use crate::stats::types::{ModifierDescriptor as D, StatType};
use crate::world::{CompRef, World};

pub fn register(r: &mut Registry) {
    use Hook::{Activate, Deactivate, PostLoad, PreSave, TurnOff, TurnOn};
    let ports: Vec<Port> = vec![
        Port::new("AddStatBonus", bits(&[Activate, TurnOn, TurnOff]), add_stat_bonus),
        Port::new("ChangeUnitSize", bits(&[Activate, Deactivate]), change_unit_size),
        Port::new(
            "AddGenericStatBonus",
            bits(&[Activate, TurnOn, TurnOff]),
            add_generic_stat_bonus,
        ),
        Port::new(
            "AddContextStatBonus",
            bits(&[Activate, TurnOn, TurnOff]),
            add_context_stat_bonus,
        ),
        Port::new("AddClassSkill", bits(&[TurnOn, TurnOff]), add_class_skill),
        Port::new("AddBackgroundClassSkill", bits(&[TurnOn, TurnOff]), add_background_class_skill),
        Port::new("AddStatBonusEquipment", bits(&[TurnOn, TurnOff]), add_stat_bonus_equipment),
        Port::new("AllSavesBonusEquipment", bits(&[TurnOn, TurnOff]), all_saves_bonus_equipment),
        Port::new("DerivativeStatBonus", bits(&[TurnOn, TurnOff]), derivative_stat_bonus),
        Port::new(
            "RecalculateOnStatChange",
            bits(&[TurnOn, TurnOff, PostLoad, PreSave]),
            recalculate_on_stat_change,
        ),
    ];
    let mut p =
        Port::new("AddStatBonusIfHasFact", bits(&[TurnOn, TurnOff]), add_stat_bonus_if_has_fact);
    p.handlers = &["IUnitGainFactHandler", "IUnitLostFactHandler"];
    p.ev = Some(add_stat_bonus_if_has_fact_event);
    let all = bits(&[TurnOn, TurnOff]);
    let skills = Port::new("BuffAllSkillsBonus", all, buff_all_skills_bonus);
    let saves = Port::new("BuffAllSavesBonus", all, buff_all_saves_bonus);
    for p in ports.into_iter().chain([p, skills, saves]) {
        r.register(Box::new(p));
    }
}

/// `UnitDescriptor.HasFact(BlueprintFact)`.
fn has_fact(w: &World, u: crate::world::EntityIdx, g: wr_content::Guid) -> bool {
    w.entity(u).facts.list.iter().any(|f| w.fact(*f).blueprint == Some(g))
}

/// `AddStatBonusIfHasFact.ShouldApplyBonus`.
fn should_apply_bonus(
    w: &World,
    u: crate::world::EntityIdx,
    facts: &[wr_content::Guid],
    require_all: bool,
    invert: bool,
) -> bool {
    let all = require_all || facts.len() == 1;
    for g in facts {
        let has = has_fact(w, u, *g);
        if has && !all {
            return !invert;
        }
        if !has && all {
            return invert;
        }
    }
    !invert
}

fn add_stat_bonus_if_has_fact_update(w: &mut World, c: CompRef, cancel: bool) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((stat, desc, facts, all, invert)) = with_settings(w, c, |v| {
        (
            StatType(v.i32("Stat")),
            D(v.i32("Descriptor")),
            v.guids("m_CheckedFacts"),
            v.bool("RequireAllFacts"),
            v.bool("InvertCondition"),
        )
    }) else {
        return;
    };
    if !stat_exists(w, u, stat) {
        return;
    }
    if !cancel && should_apply_bonus(w, u, &facts, all, invert) {
        let (bp, i) = super::settings(w, c).expect("set up");
        let objs = bp.components_array();
        let value = context_value(w, c, crate::content::View(objs[i]).obj("Value"));
        ops::add_modifier_unique(w, u, stat, value, c, desc);
    } else {
        ops::remove_modifiers_from(w, u, stat, c);
    }
}

/// `HandleUnitGainFact` (when the condition is inverted) / `HandleUnitLostFact` (when not):
/// an update if the fact is one of the checked ones.
fn add_stat_bonus_if_has_fact_event(w: &mut World, c: CompRef, iface: &str) {
    let Some(Some(fact)) = w.bus.call_fact.last().copied() else { return };
    let Some((checked, invert)) =
        with_settings(w, c, |v| (v.guids("m_CheckedFacts"), v.bool("InvertCondition")))
    else {
        return;
    };
    let matches = w.fact(fact).blueprint.is_some_and(|g| checked.contains(&g));
    let wanted = match iface {
        "IUnitGainFactHandler" => invert,
        _ => !invert,
    };
    if wanted && matches {
        add_stat_bonus_if_has_fact_update(w, c, false);
    }
}

fn add_stat_bonus_if_has_fact(w: &mut World, c: CompRef, h: Hook) {
    match h {
        Hook::TurnOn => add_stat_bonus_if_has_fact_update(w, c, false),
        Hook::TurnOff => add_stat_bonus_if_has_fact_update(w, c, true),
        _ => {}
    }
}

fn stat_exists(w: &World, u: crate::world::EntityIdx, t: StatType) -> bool {
    w.stats(u).get(t).is_some()
}

/// `PowerfulChangeType` from saved data (`None` 0, `Simple` 1, `Improved` 2).
fn powerful_change(data: Option<&serde_json::Value>) -> Option<i64> {
    let v = data?.get("PowerfulChange")?;
    match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => Some(match s.as_str() {
            "None" => 0,
            "Simple" => 1,
            _ => 2,
        }),
        _ => None,
    }
}

/// `AddStatBonus.FixLegacySaveData` + `TryApplyArcanistPowerfulChange`.
fn powerful_change_bonus(w: &mut World, c: CompRef, num: i32) -> i32 {
    let legacy = match powerful_change(w.component(c).data.as_ref()) {
        None | Some(0) => 0,
        Some(1) => 2,
        Some(_) => 4,
    };
    if legacy != 0 {
        let d = w.component_mut(c).data.get_or_insert_with(|| serde_json::json!({}));
        d["PowerfulChangeBonus"] = legacy.into();
        d["PowerfulChange"] = "None".into();
    }
    let bonus = w
        .component(c)
        .data
        .as_ref()
        .map(|d| d.get("PowerfulChangeBonus").and_then(serde_json::Value::as_i64).unwrap_or(0));
    match bonus {
        Some(b) if num >= 0 => arith::add(num, i32::try_from(b).unwrap_or(0)),
        _ => num,
    }
}

/// `AddStatBonus.CalculatePowerfulChangeBonus` in `OnActivate` (`Data.PowerfulChangeBonus` set
/// when positive): non-zero only for an arcanist's transmutation spell, with the caster's
/// `PowerfulChange` and `ImprovedPowerfulChange` features. A loaded context under an ability
/// (its root's ability not hydrated) is reported.
pub(crate) fn powerful_change_on_activate(w: &mut World, c: CompRef, stat: StatType) {
    let Some(ctx) = w.fact(c.fact).context.clone() else { return };
    let caster = ctx.caster.as_ref().and_then(|id| w.by_id.get(id).copied());
    let Some(caster) = caster else {
        w.log("error", "Caster is missing");
        return;
    };
    if !stat.is_attribute() || !ctx.has_ability_context {
        return;
    }
    let Some(src) = ctx.source_ability else {
        w.coverage.unsupported("AddStatBonus.CalculatePowerfulChangeBonus (ability context)");
        return;
    };
    // `abilityData?.Spellbook != null && Blueprint.IsSpell && Spellbook.Blueprint.IsArcanist &&
    // SpellSchool == Transmutation` (8).
    // (`IsSpell`: `Type` Spell 0 or SpellLike 1.)
    let is_spell =
        w.content.get(src.blueprint).is_some_and(|b| matches!(b.view().int("Type"), 0 | 1));
    let arcanist =
        src.spellbook.and_then(|g| w.content.get(g)).is_some_and(|b| b.view().bool("IsArcanist"));
    if !(is_spell && arcanist && src.school == 8) {
        return;
    }
    let mut num = 0;
    for f in ["PowerfulChange", "ImprovedPowerfulChange"] {
        if crate::components::state::has_mechanics_feature(w, caster, &[f]) {
            num += 2;
        }
    }
    if num > 0 {
        let d = w.component_mut(c).data.get_or_insert_with(|| serde_json::json!({}));
        d["PowerfulChangeBonus"] = num.into();
    }
}

fn add_stat_bonus(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((stat, value, desc, scale)) = with_settings(w, c, |v| {
        (
            StatType(v.i32("Stat")),
            v.i32("Value"),
            D(v.i32("Descriptor")),
            v.bool("ScaleByBasicAttackBonus"),
        )
    }) else {
        return;
    };
    match h {
        Hook::Activate => powerful_change_on_activate(w, c, stat),
        Hook::TurnOn => {
            let mut num = arith::mul(value, fact_rank(w, c.fact));
            if scale {
                let bab = w.stats(u).v(StatType::BaseAttackBonus).modified_value;
                num = arith::mul(num, arith::add(1, bab / 4));
            }
            let num = powerful_change_bonus(w, c, num);
            if stat_exists(w, u, stat) {
                ops::add_modifier_unique(w, u, stat, num, c, desc);
            }
        }
        Hook::TurnOff if stat_exists(w, u, stat) => ops::remove_modifiers_from(w, u, stat, c),
        _ => {}
    }
}

/// `AddGenericStatBonus` (`Kingmaker.UnitLogic.Buffs.Components/AddGenericStatBonus.cs`, a buff's):
/// `OnActivate` as `AddStatBonus`'s; `OnTurnOn` `Value` (not scaled by rank) with the arcanist's
/// bonus, `AddModifierUnique` on the stat when it has one; `OnTurnOff` the runtime's modifiers off.
fn add_generic_stat_bonus(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((stat, value, desc)) =
        with_settings(w, c, |v| (StatType(v.i32("Stat")), v.i32("Value"), D(v.i32("Descriptor"))))
    else {
        return;
    };
    match h {
        Hook::Activate => powerful_change_on_activate(w, c, stat),
        Hook::TurnOn => {
            let num = powerful_change_bonus(w, c, value);
            if stat_exists(w, u, stat) {
                ops::add_modifier_unique(w, u, stat, num, c, desc);
            }
        }
        // `GetStat(Stat)?.RemoveModifiersFrom(Runtime)`.
        Hook::TurnOff if stat_exists(w, u, stat) => ops::remove_modifiers_from(w, u, stat, c),
        _ => {}
    }
}

/// `ChangeUnitSize` (`Kingmaker.Designers.Mechanics.Buffs/ChangeUnitSize.cs`): `OnActivate`
/// `Owner.Ensure<UnitPartSizeModifier>().Add(Fact)`, `OnDeactivate` its `Remove(Fact)`; each
/// updates the unit's size (`spawn::update_size_part`).
fn change_unit_size(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::Activate => {
            // A loaded part keeps facts the sim doesn't hydrate.
            if w.unit(u).is_some_and(|x| x.size_part_from_save) {
                w.coverage.unsupported("UnitPartSizeModifier from a save (not hydrated)");
            }
            if !w.entity(u).parts.iter().any(|p| p.clr == "UnitPartSizeModifier") {
                w.entity_mut(u).parts.push(crate::world::Part { clr: "UnitPartSizeModifier" });
            }
            if let Some(x) = w.unit_mut(u) {
                x.size_change_facts.push(c.fact);
            }
            crate::spawn::update_size_part(w, u);
        }
        Hook::Deactivate => {
            if let Some(x) = w.unit_mut(u)
                && let Some(i) = x.size_change_facts.iter().position(|f| *f == c.fact)
            {
                x.size_change_facts.remove(i);
            }
            if !w.entity(u).parts.iter().any(|p| p.clr == "UnitPartSizeModifier") {
                w.entity_mut(u).parts.push(crate::world::Part { clr: "UnitPartSizeModifier" });
            }
            crate::spawn::update_size_part(w, u);
        }
        _ => {}
    }
}

/// `RestrictionCalculator.IsPassed`: true without a property or with an empty one.
fn restrictions_pass(w: &mut World, c: CompRef) -> bool {
    let has_getters = with_settings(w, c, |v| {
        v.obj("Restrictions")
            .and_then(|r| r.obj("Property"))
            .is_some_and(|p| !p.seq("Getters").is_empty())
    })
    .unwrap_or(false);
    if has_getters {
        w.coverage.unsupported("RestrictionCalculator with a property");
    }
    true
}

fn add_context_stat_bonus(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((stat, desc, mult, has_min, min)) = with_settings(w, c, |v| {
        (
            StatType(v.i32("Stat")),
            D(v.i32("Descriptor")),
            v.i32("Multiplier"),
            v.bool("HasMinimal"),
            v.i32("Minimal"),
        )
    }) else {
        return;
    };
    match h {
        Hook::Activate => {
            if restrictions_pass(w, c) {
                powerful_change_on_activate(w, c, stat);
            }
        }
        Hook::TurnOn => {
            if !restrictions_pass(w, c) {
                return;
            }
            let base = if w.fact(c.fact).context.is_none() {
                w.log("error", "Context is missing");
                0
            } else {
                let (bp, i) = super::settings(w, c).expect("set up");
                let objs = bp.components_array();
                let cv = crate::content::View(objs[i]).obj("Value");
                arith::mul(context_value(w, c, cv), mult)
            };
            let num = powerful_change_bonus(w, c, base);
            let v = if has_min { num.max(min) } else { num };
            if stat_exists(w, u, stat) {
                ops::add_modifier_from_component(w, u, stat, v, c, desc);
            }
        }
        Hook::TurnOff if stat_exists(w, u, stat) => ops::remove_modifiers_from(w, u, stat, c),
        _ => {}
    }
}

fn skill_of(w: &World, c: CompRef) -> Option<StatType> {
    with_settings(w, c, |v| StatType(v.i32("Skill"))).filter(|s| s.is_skill())
}

fn add_class_skill(w: &mut World, c: CompRef, h: Hook) {
    let (Some(u), Some(s)) = (owner_unit(w, c), skill_of(w, c)) else { return };
    match h {
        Hook::TurnOn => ops::class_skill_retain(w, u, s),
        Hook::TurnOff => ops::class_skill_release(w, u, s),
        _ => {}
    }
}

fn add_background_class_skill(w: &mut World, c: CompRef, h: Hook) {
    let (Some(u), Some(s)) = (owner_unit(w, c), skill_of(w, c)) else { return };
    let source = match h {
        Hook::TurnOn => {
            if let crate::stats::value::StatKind::Skill { background_source, .. } =
                &w.stats(u).v(s).kind
                && background_source.is_some()
            {
                w.log("error", "BackgroundSkillSource already set");
            }
            Some(c.fact)
        }
        Hook::TurnOff => None,
        _ => return,
    };
    if let crate::stats::value::StatKind::Skill { background_source, .. } =
        &mut w.stats_mut(u).m(s).kind
    {
        *background_source = source;
    }
    ops::update_value(w, u, s);
}

fn add_stat_bonus_equipment(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = wielder(w, c) else { return };
    let item = w.fact(c.fact).owner;
    let Some((stat, value, desc)) =
        with_settings(w, c, |v| (StatType(v.i32("Stat")), v.i32("Value"), D(v.i32("Descriptor"))))
    else {
        return;
    };
    match h {
        Hook::TurnOn => {
            if matches!(
                desc,
                D::Enhancement
                    | D::ArmorEnhancement
                    | D::ShieldEnhancement
                    | D::NaturalArmorEnhancement
            ) && crate::components::state::has_mechanics_feature(
                w,
                u,
                &["FakeEnhancement", "AdvancedFakeEnhancement"],
            ) {
                w.coverage.unsupported("AddStatBonusEquipment: fake enhancement");
            }
            if !stat_exists(w, u, stat) {
                return;
            }
            // `AddItemModifierUnique(num, Runtime, Owner, Descriptor)`.
            let name = w.component(c).name.clone();
            let exists = w.stats(u).v(stat).modifier_list.get(&desc).is_some_and(|l| {
                l.items.iter().any(|m| {
                    let m = &w.mods[m.i()];
                    m.source == Some(c.fact)
                        && m.source_component.as_deref() == Some(name.as_str())
                        && m.item_source == item
                })
            });
            if !exists {
                ops::add_item_modifier(w, u, stat, value, Some(c), item, desc);
            }
        }
        Hook::TurnOff if stat_exists(w, u, stat) => ops::remove_modifiers_from(w, u, stat, c),
        _ => {}
    }
}

fn all_saves_bonus_equipment(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = wielder(w, c) else {
        // `Wielder.Stats…` on a null wielder throws (caught by the runtime).
        return;
    };
    let Some((value, desc)) = with_settings(w, c, |v| (v.i32("Value"), D(v.i32("Descriptor"))))
    else {
        return;
    };
    for s in [StatType::SaveFortitude, StatType::SaveReflex, StatType::SaveWill] {
        match h {
            Hook::TurnOn => {
                ops::add_modifier_from_component(w, u, s, value, c, desc);
            }
            Hook::TurnOff => ops::remove_modifiers_from(w, u, s, c),
            _ => {}
        }
    }
}

fn derivative_stat_bonus(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((base, der, desc)) = with_settings(w, c, |v| {
        (StatType(v.i32("BaseStat")), StatType(v.i32("DerivativeStat")), D(v.i32("Descriptor")))
    }) else {
        return;
    };
    match h {
        Hook::TurnOn => {
            if base.is_attribute() && stat_exists(w, u, der) {
                let bonus = ops::attribute_bonus(w, u, base);
                if bonus > 0 {
                    ops::add_modifier_unique(w, u, der, bonus, c, desc);
                }
            }
        }
        Hook::TurnOff if stat_exists(w, u, der) => ops::remove_modifiers_from(w, u, der, c),
        _ => {}
    }
}

fn recalculate_on_stat_change(w: &mut World, c: CompRef, h: Hook) {
    match h {
        Hook::PostLoad | Hook::PreSave => {
            w.fact_mut(c.fact).recalculate_context_before_turn_on = true
        }
        Hook::TurnOn => {
            let Some(u) = owner_unit(w, c) else { return };
            let Some((kin, stat)) =
                with_settings(w, c, |v| (v.bool("UseKineticistMainStat"), StatType(v.i32("Stat"))))
            else {
                return;
            };
            if kin {
                w.coverage.unsupported("RecalculateOnStatChange: kineticist main stat");
            }
            if stat_exists(w, u, stat) {
                ops::add_dependent_fact(w, u, stat, c.fact);
                let d = w.component_mut(c).data.get_or_insert_with(|| serde_json::json!({}));
                d["AppliedToStat"] = stat.0.into();
            }
        }
        Hook::TurnOff => {
            let Some(u) = owner_unit(w, c) else { return };
            let applied = w
                .component(c)
                .data
                .as_ref()
                .and_then(|d| d.get("AppliedToStat"))
                .and_then(serde_json::Value::as_i64)
                .and_then(|x| i32::try_from(x).ok());
            if let Some(s) = applied
                && let Some(sv) = w.stats_mut(u).get_mut(StatType(s))
            {
                sv.dependent_facts.retain(|f| *f != c.fact);
            }
            if let Some(d) = &mut w.component_mut(c).data {
                d["AppliedToStat"] = serde_json::Value::Null;
            }
        }
        _ => {}
    }
}

/// `StatTypeHelper.Skills`, in its order.
const SKILLS: [StatType; 11] = [
    StatType::SkillMobility,
    StatType::SkillAthletics,
    StatType::SkillPersuasion,
    StatType::SkillThievery,
    StatType::SkillLoreNature,
    StatType::SkillPerception,
    StatType::SkillStealth,
    StatType::SkillUseMagicDevice,
    StatType::SkillLoreReligion,
    StatType::SkillKnowledgeWorld,
    StatType::SkillKnowledgeArcana,
];

/// `Kingmaker.Designers.Mechanics.Buffs/BuffAllSkillsBonus.cs`: on turn-on,
/// `Multiplier.Calculate(Context) * Value` to every skill (`AddModifier`, not unique); on
/// turn-off, `RemoveModifiersFrom(Runtime)`.
fn buff_all_skills_bonus(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::TurnOn => {
            let Some((bp, i)) = super::settings(w, c) else { return };
            let v = crate::content::View(bp.components_array()[i]);
            let mult = super::context_value(w, c, v.obj("Multiplier"));
            let value = arith::mul(mult, v.i32("Value"));
            let desc = D(v.i32("Descriptor"));
            for t in SKILLS {
                if stat_exists(w, u, t) {
                    ops::add_modifier_from_component(w, u, t, value, c, desc);
                }
            }
        }
        Hook::TurnOff => {
            for t in SKILLS {
                if stat_exists(w, u, t) {
                    ops::remove_modifiers_from(w, u, t, c);
                }
            }
        }
        _ => {}
    }
}

/// `Kingmaker.Designers.Mechanics.Buffs/BuffAllSavesBonus.cs`: `Value * Fact.GetRank()` to
/// Fortitude, Reflex and Will (`AddModifierUnique`); removed on turn-off.
fn buff_all_saves_bonus(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let saves = [StatType::SaveFortitude, StatType::SaveReflex, StatType::SaveWill];
    match h {
        Hook::TurnOn => {
            let Some((value, desc)) =
                with_settings(w, c, |v| (v.i32("Value"), D(v.i32("Descriptor"))))
            else {
                return;
            };
            let value = arith::mul(value, fact_rank(w, c.fact));
            for t in saves {
                ops::add_modifier_unique(w, u, t, value, c, desc);
            }
        }
        Hook::TurnOff => {
            for t in saves {
                ops::remove_modifiers_from(w, u, t, c);
            }
        }
        _ => {}
    }
}
