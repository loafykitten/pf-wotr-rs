//! `RuleCalculateAbilityParams` for a blueprint with no ability data (the path
//! `ContextCalculateAbilityParams.Calculate(context)` takes when the context has no ability
//! behind it). Sources (decompiled, not copied): `Kingmaker.RuleSystem.Rules.Abilities/
//! RuleCalculateAbilityParams.cs`, `Kingmaker.UnitLogic.Mechanics.Components/
//! ContextCalculateAbilityParams.cs`, `Kingmaker.UnitLogic.Abilities/AbilityParams.cs`.
//!
//! Not ported, reported: a rule for an `AbilityData` (a spell or ability being cast), a
//! spellbook, metamagic, a `ContextSetAbilityParams` on the blueprint, the kineticist main
//! stat and a custom property for the stat. The bonus members (`AddBonusCasterLevel` …) are
//! only raised by subscribers, and a subscriber that isn't ported reports itself.

use serde_json::{Value, json};

use crate::content::{Blueprint, View, is_a};
use crate::rulebook::{RuleBase, RuleEvent};
use crate::stats::types::StatType;
use crate::world::{EntityIdx, World};

/// `RuleCalculateAbilityParams`.
#[derive(Debug)]
pub struct RuleCalculateAbilityParams {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    /// `Blueprint is BlueprintAbility`: the `IsDomainAbility` check and the ability paths.
    pub blueprint_is_ability: bool,
    pub ignore_dc_bonuses: bool,
    /// `AbilityDifficultyLimitDC` on the blueprint.
    pub difficulty_limit_dc: bool,
    pub replace_caster_level: Option<i32>,
    pub replace_spell_level: Option<i32>,
    pub replace_dc: Option<i32>,
    pub replace_concentration: Option<i32>,
    pub replace_stat_bonus_modifier: Option<i32>,
    pub replace_stat: Option<StatType>,
    /// `m_BonusCasterLevel`, `m_BonusSpellLevel`, `m_BonusDC`, `m_BonusConcentration`
    /// (`ModifiableBonus` totals): raised by subscribers.
    pub bonus_caster_level: i32,
    pub bonus_spell_level: i32,
    pub bonus_dc: i32,
    pub bonus_concentration: i32,
    /// `Result`: the persisted members of `AbilityParams`.
    pub result: Option<Value>,
}

impl RuleCalculateAbilityParams {
    /// `new RuleCalculateAbilityParams(initiator, blueprint, null)`.
    #[must_use]
    pub fn new(initiator: EntityIdx, blueprint: &Blueprint) -> Self {
        Self {
            base: RuleBase::default(),
            initiator,
            blueprint_is_ability: is_a(blueprint.obj.ty, "BlueprintAbility"),
            ignore_dc_bonuses: false,
            difficulty_limit_dc: blueprint
                .components_array()
                .iter()
                .any(|o| is_a(o.ty, "AbilityDifficultyLimitDC")),
            replace_caster_level: None,
            replace_spell_level: None,
            replace_dc: None,
            replace_concentration: None,
            replace_stat_bonus_modifier: None,
            replace_stat: None,
            bonus_caster_level: 0,
            bonus_spell_level: 0,
            bonus_dc: 0,
            bonus_concentration: 0,
            result: None,
        }
    }
}

/// `RuleCalculateAbilityParams.LimitDC`.
fn limit_dc(dc: i32, limit: i32, coeff: i32) -> i32 {
    if limit <= 0 || dc <= limit {
        return dc;
    }
    if coeff <= 0 {
        return limit;
    }
    limit.wrapping_add(dc.wrapping_sub(limit) / coeff)
}

impl RuleEvent for RuleCalculateAbilityParams {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCalculateAbilityParams"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn on_trigger(&mut self, w: &mut World) {
        let u = self.initiator;
        let mut num = self.replace_caster_level.unwrap_or(-1);
        let mut num2 = self.replace_spell_level.unwrap_or(-1);
        let mut num3 = self.replace_dc.unwrap_or(-1);
        let mut num4 = self.replace_concentration.unwrap_or(-1);
        if num < 0 {
            // `Spellbook?.EffectiveCasterLevel ?? Progression.CharacterLevel`: no spellbook.
            num = crate::unit::character_level(w, u);
        }
        num = num.max(1).wrapping_add(self.bonus_caster_level);
        if num2 < 0 {
            // `m_SpellLevel ?? num / 2` (no metamagic data).
            num2 = (num / 2).wrapping_add(self.bonus_spell_level);
        }
        let num5 = if let Some(v) = self.replace_stat_bonus_modifier {
            v
        } else {
            // `ReplaceStat ?? Spellbook?.Blueprint.CastingAttribute ?? Charisma`.
            let stat = self.replace_stat.unwrap_or(StatType::Charisma);
            if self.replace_stat.is_some() && self.blueprint_is_ability {
                // `Features.DomainsByIntelligenceFeature` with `IsDomainAbility`.
                w.coverage.unsupported("RuleCalculateAbilityParams: ability blueprint");
            }
            if stat == StatType::BaseAttackBonus {
                w.stats(u).v(StatType::BaseAttackBonus).modified_value
            } else if stat.is_attribute() {
                crate::stats::ops::attribute_bonus(w, u, stat)
            } else {
                // `PFLog.Default.Error(…)`, then `Stats.Charisma`.
                w.log("error", format!("Can't use stat for casting: '{stat:?}'"));
                crate::stats::ops::attribute_bonus(w, u, StatType::Charisma)
            }
        };
        if num3 < 0 {
            num3 = 10i32.wrapping_add(num2).wrapping_add(num5);
        }
        if !self.ignore_dc_bonuses {
            num3 = num3.wrapping_add(self.bonus_dc);
        }
        // `GetAdjustmentPreset(SettingsRoot.Difficulty.StatsAdjustments)`, read for every
        // rule; only an enemy of the player uses it.
        let preset = match w.settings.difficulty.adjustment_preset() {
            Ok(p) => Some((p.ability_dc_bonus, p.ability_dc_limit, p.ability_dc_limit_coeff)),
            Err(why) => {
                w.coverage.unsupported(format!("RuleCalculateAbilityParams: {why}"));
                None
            }
        };
        if crate::combat::units::is_players_enemy(w, u)
            && let Some((bonus, limit, coeff)) = preset
        {
            num3 = num3.wrapping_add(bonus);
            if self.difficulty_limit_dc {
                num3 = limit_dc(num3, limit, coeff);
            }
        }
        if num4 < 0 {
            num4 = num.wrapping_add(num5);
        }
        num4 = num4.wrapping_add(self.bonus_concentration);
        // `AbilityParams`: `SpellSource` Unknown (2), no metamagic, `RankBonus` the caster
        // level bonus.
        self.result = Some(json!({
            "SpellSource": 2,
            "CasterLevel": num,
            "SpellLevel": num2,
            "DC": num3,
            "Concentration": num4,
            "Metamagic": 0,
            "RankBonus": self.bonus_caster_level,
        }));
        let fields = json!({ "Result": self.result });
        crate::rules::record_trigger_end(w, self, fields);
    }
    fn base(&self) -> &RuleBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut RuleBase {
        &mut self.base
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// `ContextCalculateAbilityParams.Calculate(context)` for a context without an ability
/// behind it: the rule triggered through `MechanicsContext.TriggerRule`. `cfg` is the
/// component, `caster` the context's caster. The returned value is the new `m_Params`.
pub fn calculate_context_params(
    w: &mut World,
    ctx: &crate::context::MechanicsContext,
    bp: &Blueprint,
    caster: EntityIdx,
    cfg: View<'_>,
) -> Option<Value> {
    if ctx.has_ability_context {
        w.coverage.unsupported("ContextCalculateAbilityParams: ability context");
        return None;
    }
    if cfg.bool("UseKineticistMainStat") || cfg.bool("StatTypeFromCustomProperty") {
        w.coverage.unsupported("ContextCalculateAbilityParams: kineticist or custom property stat");
        return None;
    }
    if bp.components_array().iter().any(|o| is_a(o.ty, "ContextSetAbilityParams")) {
        // The rule's constructor calls `ContextSetAbilityParams.Calculate(initiator)`.
        w.coverage.unsupported("RuleCalculateAbilityParams: ContextSetAbilityParams");
        return None;
    }
    let mut rule = RuleCalculateAbilityParams::new(caster, bp);
    rule.replace_stat = Some(StatType(cfg.i32("StatType")));
    // `ContextValue.Calculate(context)` for the value types the context holds.
    let value = |w: &mut World, cv: Option<View<'_>>| -> i32 {
        let Some(cv) = cv else { return 0 };
        match cv.int("ValueType") {
            0 => cv.i32("Value"),
            1 => ctx.rank(cv.int("ValueRank")),
            2 => ctx.shared(cv.int("ValueShared")),
            t => {
                w.coverage.unsupported(format!("ContextValue.Calculate: ValueType {t}"));
                0
            }
        }
    };
    if cfg.bool("ReplaceCasterLevel") {
        rule.replace_caster_level = Some(value(w, cfg.obj("CasterLevel")));
    }
    if cfg.bool("ReplaceSpellLevel") {
        rule.replace_spell_level = Some(value(w, cfg.obj("SpellLevel")));
    }
    // `context.TriggerRule(rule)`: `Reason` = the context.
    rule.base.reason = Some("MechanicsContext".into());
    crate::rules::trigger_rule(w, &mut rule);
    rule.result
}

#[cfg(test)]
mod tests {
    use wr_content::node::Node;

    use super::*;
    use crate::facts::tests::{feature, obj};

    /// `RuleCalculateAbilityParams` without ability data leaves `SpellSource` at Unknown (2),
    /// and its result is the one recorded in `rule_end`, which the oracle comparison reads.
    #[test]
    fn context_params_have_an_unknown_source_and_are_recorded() {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        w.events.on = true;
        let bp = feature(&w, 30, vec![]);
        let cfg = obj(
            "Kingmaker.UnitLogic.Mechanics.Components.ContextCalculateAbilityParams",
            &[("StatType", Node::Enum(6))],
        );
        let ctx = crate::context::MechanicsContext::default();
        let p = calculate_context_params(&mut w, &ctx, &bp, u, View(&cfg)).expect("params");
        assert_eq!(p["SpellSource"], 2);
        let end = w.events.records.iter().rev().find(|e| e.k == "rule_end").expect("rule_end");
        assert_eq!(end.fields["result"]["Result"], p);
    }
}
