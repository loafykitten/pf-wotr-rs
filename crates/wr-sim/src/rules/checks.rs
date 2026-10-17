//! Dice and checks (decompiled, not copied: `Kingmaker.RuleSystem/RulebookEvent.cs`
//! `Dice.D`, `Kingmaker.RuleSystem.Rules/{RuleRollDice,RuleRollD20,RuleStatCheck,
//! RuleSkillCheck,RulePartyStatCheck,RulePartySkillCheck}.cs`, `Kingmaker.UnitLogic/
//! UnitState.cs` `CanAct`).
//!
//! Every die is a separate `Random.Range(1, sides + 1)` at `Dice.D`'s call site; the
//! direct draw of a forced check (`EnsureSuccess`) is `RuleSkillCheck.OnTrigger`'s own
//! site. Rule subscribers (unit facts handling `RuleSkillCheck`, `RuleRollD20`…) run
//! through the rulebook bus, fail-closed; the oracle's other subscribers of these rules
//! (`GameHistoryLog`, `SoundState`) are presentation.

use std::any::Any;

use crate::csharp::arith;
use crate::rng;
use crate::rulebook::{RuleBase, RuleEvent, trigger};
use crate::stats::types::StatType;
use crate::world::{EntityIdx, World};

/// `RuleRollDice` (and `RuleRollD20` when `d20`).
#[derive(Debug, Clone)]
pub struct RuleRollDice {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub rolls: i32,
    pub sides: i32,
    pub d20: bool,
    m_result: i32,
    pub result_override: Option<i32>,
    pub pre_rolled: Option<i32>,
    pub reroll_amount: i32,
    pub replace_one_with_max: bool,
    pub roll_history: Option<Vec<i32>>,
}

impl RuleRollDice {
    #[must_use]
    pub fn new(initiator: EntityIdx, rolls: i32, sides: i32) -> Self {
        Self {
            base: RuleBase::default(),
            initiator,
            rolls,
            sides,
            d20: false,
            m_result: 0,
            result_override: None,
            pre_rolled: None,
            reroll_amount: 0,
            replace_one_with_max: false,
            roll_history: None,
        }
    }

    /// `new RuleRollD20(initiator)`.
    #[must_use]
    pub fn d20(initiator: EntityIdx) -> Self {
        let mut r = Self::new(initiator, 1, 20);
        r.d20 = true;
        r
    }

    /// `new RuleRollD20(initiator, resultOverride)`.
    #[must_use]
    pub fn d20_override(initiator: EntityIdx, v: i32) -> Self {
        let mut r = Self::d20(initiator);
        r.result_override = Some(v);
        r
    }

    /// `ReplacedOne`.
    #[must_use]
    pub fn replaced_one(&self) -> bool {
        self.replace_one_with_max && self.m_result == 1
    }

    /// `Result`.
    #[must_use]
    pub fn result(&self) -> i32 {
        if self.replaced_one() {
            // `DiceFormula.MaxValue(0)`: rolls × sides.
            arith::mul(self.rolls, self.sides)
        } else {
            self.result_override.unwrap_or(self.m_result)
        }
    }

    /// The rolled value before overrides (`m_Result`).
    #[must_use]
    pub fn raw(&self) -> i32 {
        self.m_result
    }

    fn reroll(&mut self, w: &mut World) {
        let num = rng::dice(&mut *w.rng, self.rolls, self.sides);
        self.roll_history.get_or_insert_with(Vec::new).push(num);
        let val = if num == 1 && self.replace_one_with_max {
            arith::mul(self.rolls, self.sides)
        } else {
            num
        };
        self.m_result =
            if self.reroll_amount > 0 { self.m_result.max(val) } else { self.m_result.min(val) };
    }
}

impl RuleEvent for RuleRollDice {
    fn chain(&self) -> &'static [&'static str] {
        if self.d20 { &["RuleRollD20", "RuleRollDice"] } else { &["RuleRollDice"] }
    }

    fn initiator(&self) -> EntityIdx {
        self.initiator
    }

    /// `RuleRollDice.OnTrigger`: roll unless a result is already set.
    fn on_trigger(&mut self, w: &mut World) {
        if self.result() <= 0 {
            self.m_result = match self.pre_rolled {
                Some(v) => v,
                None => rng::dice(&mut *w.rng, self.rolls, self.sides),
            };
            self.roll_history = Some(vec![self.m_result]);
            let mut n = self.reroll_amount.abs();
            while n > 0 {
                self.reroll(w);
                n -= 1;
            }
        }
        let fields = serde_json::json!({ "m_Result": self.m_result, "Result": self.result(), "RollHistory": self.roll_history });
        crate::rules::record_trigger_end(w, self, fields);
    }

    fn base(&self) -> &RuleBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RuleBase {
        &mut self.base
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

/// `RuleRollD20.FromInt(initiator, roll)`: a D20 rule with an override, triggered.
pub fn d20_from_int(w: &mut World, initiator: EntityIdx, v: i32) -> RuleRollDice {
    let mut r = RuleRollDice::d20_override(initiator, v);
    crate::rules::trigger_rule(w, &mut r);
    r
}

/// `new RuleRollD20(initiator)` triggered (`RuleSkillCheck.RollD20`, `Dice.D20`).
pub fn roll_d20(w: &mut World, initiator: EntityIdx) -> RuleRollDice {
    let mut r = RuleRollDice::d20(initiator);
    crate::rules::trigger_rule(w, &mut r);
    r
}

/// The stat value a check reads: an attribute's `Bonus`, else `ModifiedValue`.
fn stat_value(w: &World, u: EntityIdx, t: StatType) -> Option<i32> {
    let s = w.unit(u)?.stats.get(t)?;
    Some(match s.kind {
        crate::stats::value::StatKind::Attribute { .. } => {
            crate::stats::ops::attribute_bonus(w, u, t)
        }
        _ => s.modified_value,
    })
}

/// `RuleSkillCheck`.
#[derive(Debug, Clone)]
pub struct RuleSkillCheck {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub stat_type: StatType,
    pub stat_value: i32,
    pub original_dc: i32,
    pub dc: i32,
    pub difficulty_dc_adjustment: i32,
    pub ignore_difficulty_bonus_to_dc: bool,
    pub success_bonus: i32,
    pub take10_for_success: bool,
    pub ensure_success: Option<bool>,
    /// `Bonus` (a rule-local `ModifiableValue`): its modified value. Rule subscribers
    /// would add modifiers; none are ported, and a subscriber reports unsupported.
    pub bonus: i32,
    pub d20: Option<RuleRollDice>,
    m_success: bool,
    pub disable_battle_log_self: bool,
    /// `UpdateValues` failed (no stat): `OnTrigger` logs and returns.
    pub invalid_stat: bool,
}

impl RuleSkillCheck {
    /// `new RuleSkillCheck(unit, statType, dc)` (constructor runs `UpdateValues`).
    pub fn new(w: &mut World, unit: EntityIdx, stat_type: StatType, dc: i32) -> Self {
        if stat_type == StatType::SkillUseMagicDevice {
            // `unit.State.Features.FoolMagicFeature` turns UMD into Persuasion.
            w.coverage.unsupported("RuleSkillCheck: UseMagicDevice (FoolMagicFeature)");
        }
        let mut r = Self {
            base: RuleBase::default(),
            initiator: unit,
            stat_type,
            stat_value: 0,
            original_dc: dc,
            dc: 0,
            difficulty_dc_adjustment: 0,
            ignore_difficulty_bonus_to_dc: false,
            success_bonus: 0,
            take10_for_success: false,
            ensure_success: None,
            bonus: 0,
            d20: None,
            m_success: false,
            disable_battle_log_self: false,
            invalid_stat: false,
        };
        r.update_values(w);
        r
    }

    /// `UpdateValues()`.
    fn update_values(&mut self, w: &mut World) -> bool {
        let (dc, adj) =
            adjust_dc_by_difficulty(w, self.original_dc, self.ignore_difficulty_bonus_to_dc);
        self.dc = dc;
        self.difficulty_dc_adjustment = adj;
        match stat_value(w, self.initiator, self.stat_type) {
            Some(v) => {
                self.stat_value = v;
                self.invalid_stat = false;
                true
            }
            None => {
                self.invalid_stat = true;
                false
            }
        }
    }

    /// `TotalBonus`.
    #[must_use]
    pub fn total_bonus(&self) -> i32 {
        arith::add(self.bonus, self.stat_value)
    }

    /// `D20` as an int (`(int)D20` is its `Result`, 0 if null).
    #[must_use]
    pub fn d20_value(&self) -> i32 {
        self.d20.as_ref().map_or(0, RuleRollDice::result)
    }

    #[must_use]
    pub fn base_roll_result(&self) -> i32 {
        arith::add(self.d20_value(), self.total_bonus())
    }

    #[must_use]
    pub fn requires_success_bonus(&self) -> bool {
        self.base_roll_result() < self.dc
            && self.success_bonus != 0
            && arith::add(self.base_roll_result(), self.success_bonus) >= self.dc
    }

    /// `RollResult`.
    #[must_use]
    pub fn roll_result(&self) -> i32 {
        if self.requires_success_bonus() {
            arith::add(self.base_roll_result(), self.success_bonus)
        } else {
            self.base_roll_result()
        }
    }

    /// `NonNegativeDC`.
    #[must_use]
    pub fn non_negative_dc(&self) -> i32 {
        if self.dc >= 0 || self.stat_value < 0 { self.dc } else { 0 }
    }

    #[must_use]
    pub fn success(&self) -> bool {
        self.m_success
    }

    /// `IsSuccessRoll(d20, successBonus)`.
    #[must_use]
    pub fn is_success_roll(&self, d20: i32, success_bonus: i32) -> bool {
        arith::add(arith::add(d20, self.total_bonus()), success_bonus) >= self.dc
    }

    /// `Calculate()` (the UI's preview: no rule for the check itself, but a triggered
    /// `RuleRollD20`).
    pub fn calculate(&mut self, w: &mut World) {
        if let Some(v) = stat_value(w, self.initiator, self.stat_type) {
            self.stat_value = v;
        } else {
            self.stat_value = 0;
        }
        self.d20 = Some(roll_d20(w, self.initiator));
    }

    /// `Chance` (`CalculateChances(TotalBonus, DC)`).
    #[must_use]
    pub fn chance(&self) -> i32 {
        calculate_chances(self.total_bonus(), self.dc)
    }
}

/// `RuleSkillCheck.CalculateChances(bonus, dc)`: `Clamp(FloorToInt((float)(100 * (21 -
/// (dc - bonus))) / 20f), 0, 100)`.
#[must_use]
pub fn calculate_chances(bonus: i32, dc: i32) -> i32 {
    let n = arith::mul(100, arith::sub(21, arith::sub(dc, bonus)));
    #[allow(clippy::cast_precision_loss)]
    let f = arith::store_f32(f64::from(n as f32) / 20.0);
    arith::floor_to_int(f).clamp(0, 100)
}

/// `RuleSkillCheck.AdjustDCByDifficulty(originalDC, out dcAdjustment, ignore)`.
pub fn adjust_dc_by_difficulty(w: &mut World, original: i32, ignore: bool) -> (i32, i32) {
    let mut adj = 0;
    if !ignore {
        adj = match w.settings.difficulty.adjustment_preset() {
            Ok(p) => p.skill_check_dc_bonus,
            Err(why) => {
                w.coverage.unsupported(format!("RuleSkillCheck.AdjustDCByDifficulty: {why}"));
                0
            }
        };
        if adj < 0 && original > 10 && arith::add(original, adj) < 10 {
            adj = arith::sub(10, original);
        } else if adj < 0 && original <= 10 {
            adj = 0;
        }
    }
    (arith::add(original, adj), adj)
}

impl RuleEvent for RuleSkillCheck {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleSkillCheck", "RuleStatCheck"]
    }

    fn initiator(&self) -> EntityIdx {
        self.initiator
    }

    /// `RuleSkillCheck.OnTrigger`.
    fn on_trigger(&mut self, w: &mut World) {
        if !self.update_values(w) {
            w.log("error", format!("Invalid stat {}", self.stat_type.name()));
            return;
        }
        match self.ensure_success {
            None => {
                if self.take10_for_success && arith::add(10, self.total_bonus()) >= self.dc {
                    self.d20 = Some(d20_from_int(w, self.initiator, 10));
                } else {
                    self.d20 = Some(roll_d20(w, self.initiator));
                }
            }
            Some(ensure) => {
                let mut min = 1;
                let mut max = 21;
                let val = arith::sub(self.dc, self.total_bonus());
                if ensure {
                    min = 1.max(val.min(20));
                } else {
                    max = 21.min(val.max(2));
                }
                let v = w.rng.range_int(rng::sites::RULE_SKILL_CHECK, min, max);
                self.d20 = Some(d20_from_int(w, self.initiator, v));
            }
        }
        let bonus = if self.requires_success_bonus() { self.success_bonus } else { 0 };
        self.m_success = self.is_success_roll(self.d20_value(), bonus);
        let fields = serde_json::json!({
            "D20": self.d20_value(),
            "RollResult": self.roll_result(),
            "BaseRollResult": self.base_roll_result(),
            "Success": self.m_success,
            "StatType": self.stat_type.0,
            "StatValue": self.stat_value,
            "DC": self.dc,
            "OriginalDC": self.original_dc,
            "DifficultyDCAdjustment": self.difficulty_dc_adjustment,
        });
        crate::rules::record_trigger_end(w, self, fields);
    }

    fn base(&self) -> &RuleBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RuleBase {
        &mut self.base
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

/// `RuleStatCheck.Create(initiator, statType, dc)`: saving throws become
/// `RuleSavingThrow` (not ported), everything else a `RuleSkillCheck`.
pub fn stat_check_create(
    w: &mut World,
    u: EntityIdx,
    t: StatType,
    dc: i32,
) -> Option<RuleSkillCheck> {
    if matches!(t, StatType::SaveFortitude | StatType::SaveReflex | StatType::SaveWill) {
        w.coverage.unsupported("RuleStatCheck.Create: RuleSavingThrow");
        return None;
    }
    Some(RuleSkillCheck::new(w, u, t, dc))
}

/// `UnitState.CanAct` outside cutscenes: not helpless, able, no `CantAct`. Conditions come
/// from facts (`AddCondition` and friends), which aren't ported: a unit with such a fact
/// active reports unsupported. The animation manager's exclusive state is presentation
/// (assumed not exclusive).
pub fn can_act(w: &mut World, u: EntityIdx) -> bool {
    use crate::unit_state::has_condition as has;
    if w.player.mode == "Cutscene" {
        return true;
    }
    // `UnitCondition`: Paralyzed 4, Petrified 8, Dazed 9, Sleeping 16, Stunned 23, CantAct
    // 28, Unconscious 31.
    let conscious = w.unit(u).is_some_and(|x| x.life_state == 0);
    if has(w, u, 4) || has(w, u, 8) {
        // `IsAnimating` then asks the cutscene-controlled unit.
        w.coverage.unsupported("UnitState.IsAnimating: paralyzed or petrified unit");
    }
    let helpless = !(conscious && !has(w, u, 16)) || has(w, u, 31);
    let prone = w.unit(u).is_some_and(|x| x.prone_active);
    let able = !has(w, u, 9) && !has(w, u, 23) && !prone;
    !helpless && able && !has(w, u, 28)
}

/// `RulePartyStatCheck`.
#[derive(Debug, Clone)]
pub struct RulePartyStatCheck {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub stat_type: StatType,
    pub difficulty_class: i32,
    pub apply_party_checks_in_capital: bool,
    pub allow_pets: bool,
    pub ensure_success: Option<bool>,
    pub stat_value: i32,
    pub roller: Option<EntityIdx>,
    pub stat_check: Option<RuleSkillCheck>,
    pub d20: Option<i32>,
}

impl RulePartyStatCheck {
    /// `new RulePartyStatCheck(statType, dc, applyPartyChecksInCapital, allowPets)`: the
    /// initiator is the main character.
    #[must_use]
    pub fn new(w: &World, stat_type: StatType, dc: i32, capital: bool) -> Self {
        Self {
            base: RuleBase::default(),
            initiator: w.main_character.expect("main character"),
            stat_type,
            difficulty_class: dc,
            apply_party_checks_in_capital: capital,
            allow_pets: false,
            ensure_success: None,
            stat_value: 0,
            roller: None,
            stat_check: None,
            d20: None,
        }
    }

    #[must_use]
    pub fn success(&self) -> bool {
        self.stat_check.as_ref().is_some_and(RuleSkillCheck::success)
    }

    /// `NonNegativeDC`.
    #[must_use]
    pub fn non_negative_dc(&self) -> i32 {
        self.stat_check
            .as_ref()
            .map_or(self.difficulty_class.max(0), RuleSkillCheck::non_negative_dc)
    }

    /// `Calculate(isTrigger)`.
    pub fn calculate(&mut self, w: &mut World, is_trigger: bool) {
        self.stat_value = i32::MIN;
        self.roller = None;
        for u in applicable_units(w, self.allow_pets, self.apply_party_checks_in_capital) {
            if can_act(w, u) {
                let v = stat_value(w, u, self.stat_type).unwrap_or(0);
                if self.stat_value < v {
                    self.stat_value = v;
                    self.roller = Some(u);
                }
            }
        }
        let Some(roller) = self.roller else {
            w.log("info", "Roller is null, in the party skillcheck");
            return;
        };
        let Some(mut check) = stat_check_create(w, roller, self.stat_type, self.difficulty_class)
        else {
            return;
        };
        check.ensure_success = self.ensure_success;
        if is_trigger {
            check.disable_battle_log_self = true;
            crate::rules::trigger_rule(w, &mut check);
        } else {
            check.calculate(w);
        }
        self.d20 = Some(check.d20_value());
        self.stat_check = Some(check);
    }
}

/// `GetApplicableUnits()`: the party (with pets if allowed), plus remote companions in
/// capital mode, minus units with party checks disabled.
fn applicable_units(w: &mut World, allow_pets: bool, capital: bool) -> Vec<EntityIdx> {
    let lists = crate::narrative::party::character_lists(w);
    let mut v = if allow_pets { lists.party_and_pets } else { lists.party };
    if capital && w.player.capital_party_mode() {
        v.extend(
            lists
                .remote_companions
                .into_iter()
                .filter(|u| allow_pets || !w.unit(*u).is_some_and(|x| x.is_pet)),
        );
    }
    v.into_iter().filter(|u| !crate::narrative::party::is_party_checks_disabled(w, *u)).collect()
}

impl RuleEvent for RulePartyStatCheck {
    fn chain(&self) -> &'static [&'static str] {
        &["RulePartyStatCheck"]
    }

    fn initiator(&self) -> EntityIdx {
        self.initiator
    }

    fn on_trigger(&mut self, w: &mut World) {
        self.calculate(w, true);
        let fields = serde_json::json!({
            "StatValue": self.stat_value,
            "Roller": self.roller.map(|u| crate::events::unit_ref(&w.entity(u).unique_id)),
            "D20": self.d20,
            "RollResult": arith::add(self.d20.unwrap_or(0), self.stat_value),
        });
        crate::rules::record_trigger_end(w, self, fields);
    }

    fn base(&self) -> &RuleBase {
        &self.base
    }

    fn base_mut(&mut self) -> &mut RuleBase {
        &mut self.base
    }

    fn as_any(&mut self) -> &mut dyn Any {
        self
    }
}

/// `Rulebook.Trigger(rule)` ignoring the overflow (it can't happen at these depths).
pub fn trigger_ignoring_overflow(w: &mut World, rule: &mut dyn RuleEvent) {
    let _ = trigger(w, rule);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chances_match_the_formula() {
        assert_eq!(calculate_chances(-1, 12), 40);
        assert_eq!(calculate_chances(0, 30), 0);
        assert_eq!(calculate_chances(25, 5), 100);
        assert_eq!(calculate_chances(3, 12), 60);
    }
}
