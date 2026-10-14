//! `DifficultyStatAdvancement` (decompiled, not copied:
//! `Kingmaker.Designers.Mechanics.Buffs/DifficultyStatAdvancement.cs`): difficulty stat
//! scaling for NPCs, from the save's `stats-adjustments` and `enemy-difficulty` settings
//! and `DifficultyPresetsList` ([`crate::difficulty`]).
//!
//! Ported: `OnTurnOn` (`AddAllModifiers` unless the unit is an army unit) and `OnTurnOff`
//! (`RemoveAllModifiers`). Its handlers (`IDifficultyChangedClassHandler` when the player
//! changes difficulty, `IUnitInitializedHandler` when a unit is created) aren't on the
//! load or dialogue paths; they stay unported, so the manifest shows the type as partial
//! and a raised event reports unsupported.

use super::{Port, bits, owner_unit, with_settings};
use crate::csharp::arith;
use crate::difficulty::StatsAdjustmentPreset;
use crate::registry::{Hook, Registry};
use crate::stats::ops;
use crate::stats::types::{ModifierDescriptor as D, StatType};
use crate::world::{CompRef, EntityIdx, World};

pub fn register(r: &mut Registry) {
    let mut p = Port::new(
        "DifficultyStatAdvancement",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        difficulty_stat_advancement,
    );
    p.handlers = &["IUnitInitializedHandler"];
    p.ev = Some(on_event);
    r.register(Box::new(p));
}

/// `HandleUnitInitialized` (POC-5, units created from placeholders): unless an army unit,
/// `Recalculate()`: keep the health ratio, remove and re-add every modifier, then
/// `Damage = Math.Max(0, MaxHP - Mathf.RoundToInt((float)MaxHP * ratio))`.
fn on_event(w: &mut World, c: CompRef, iface: &str) {
    if iface != "IUnitInitializedHandler" {
        w.coverage.unsupported(format!("DifficultyStatAdvancement: {iface}"));
        return;
    }
    let Some(u) = owner_unit(w, c) else { return };
    if ignore_difficulty(w, u) {
        return;
    }
    let max = w.stats(u).v(StatType::HitPoints).modified_value;
    let left = arith::sub(max, w.unit(u).map_or(0, |x| x.damage));
    // `(float)HPLeft / (float)MaxHP`, stored as float.
    #[allow(clippy::cast_precision_loss)]
    let ratio = arith::store_f32(f64::from(left as f32) / f64::from(max as f32));
    for t in STATS {
        ops::remove_modifiers_from(w, u, t, c);
    }
    add_all_modifiers(w, c, u);
    let max = w.stats(u).v(StatType::HitPoints).modified_value;
    #[allow(clippy::cast_precision_loss)]
    let scaled = arith::round_to_int(arith::store_f32(f64::from(max as f32) * f64::from(ratio)));
    let dmg = 0.max(arith::sub(max, scaled));
    crate::combat::set_damage(w, u, dmg);
}

/// The stats `AddAllModifiers`/`RemoveAllModifiers` touch, in the C#'s order.
const STATS: [StatType; 10] = [
    StatType::Strength,
    StatType::Constitution,
    StatType::Dexterity,
    StatType::Intelligence,
    StatType::Wisdom,
    StatType::Charisma,
    StatType::SkillPerception,
    StatType::AC,
    StatType::AdditionalAttackBonus,
    StatType::HitPoints,
];

fn difficulty_stat_advancement(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    if ignore_difficulty(w, u) {
        return;
    }
    match h {
        Hook::TurnOn => add_all_modifiers(w, c, u),
        Hook::TurnOff => {
            for t in STATS {
                ops::remove_modifiers_from(w, u, t, c);
            }
        }
        _ => {}
    }
}

/// `Owner.Blueprint.GetArmyData() != null`: the unit blueprint has an `ArmyUnitComponent`.
fn ignore_difficulty(w: &World, u: EntityIdx) -> bool {
    w.unit(u).and_then(|x| x.blueprint).and_then(|g| w.content.get(g)).is_some_and(|b| {
        b.components_array().iter().any(|o| crate::content::is_a(o.ty, "ArmyUnitComponent"))
    })
}

/// `BlueprintUnit.CR`: the `Experience` component's CR, or 0 when there is none or it is
/// the default (`CR == 1 && |Modifier - 0.5| < 0.001`).
#[must_use]
pub fn blueprint_cr(w: &World, unit_bp: Option<wr_content::Guid>) -> i32 {
    let Some(b) = unit_bp.and_then(|g| w.content.get(g)) else { return 0 };
    let comps = b.components_array();
    let Some(e) = comps.iter().find(|o| crate::content::is_a(o.ty, "Experience")) else {
        return 0;
    };
    let v = crate::content::View(e);
    let cr = v.i32("CR");
    // `Math.Abs(component.Modifier - 0.5f) < 0.001f`: float arithmetic, compared as float.
    let m = arith::store_f32(f64::from(v.f32("Modifier")) - 0.5);
    if cr == 1 && m.abs() < 0.001 { 0 } else { cr }
}

/// `ApplyBonusProgression(preset, cr, value, isBaseStat)`.
#[must_use]
pub fn apply_bonus_progression(p: &StatsAdjustmentPreset, cr: i32, value: i32, base: bool) -> i32 {
    let (start, end, cap) = if base {
        (
            p.basic_stat_coefficient_start,
            p.basic_stat_coefficient_end,
            p.basic_stat_coefficient_cr_cap,
        )
    } else {
        (
            p.derivative_stat_coefficient_start,
            p.derivative_stat_coefficient_end,
            p.derivative_stat_coefficient_cr_cap,
        )
    };
    // `float num3 = cap` (int to float on store); `Math.Min(1f, (float)cr / num3)`: the
    // quotient is rounded to float when passed.
    #[allow(clippy::cast_precision_loss)]
    let num3 = cap as f32;
    let q = arith::store_f32(f64::from(cr) / f64::from(num3));
    let num4 = if q.is_nan() { q } else { 1f32.min(q) };
    // `Mathf.FloorToInt((float)value * (num + (num2 - num) * num4))`: evaluated in double,
    // rounded to float at the call.
    let x = arith::store_f32(
        f64::from(value)
            * (f64::from(start) + (f64::from(end) - f64::from(start)) * f64::from(num4)),
    );
    let num5 = arith::floor_to_int(x);
    if value <= 0 { value.min(num5) } else { num5 }
}

fn add_all_modifiers(w: &mut World, c: CompRef, u: EntityIdx) {
    let Some((basic, derivative)) =
        with_settings(w, c, |v| (v.i32("BasicStatBonus"), v.i32("DerivativeStatBonus")))
    else {
        return;
    };
    let preset = match w.settings.difficulty.adjustment_preset() {
        Ok(p) => p.clone(),
        Err(why) => {
            // The C# would throw (no list, or no preset): report and stop.
            w.coverage.unsupported(format!("DifficultyStatAdvancement: {why}"));
            return;
        }
    };
    let Some(enemy) = w.settings.difficulty.settings.enemy_difficulty else {
        w.coverage.unsupported("DifficultyStatAdvancement: enemy-difficulty setting missing");
        return;
    };
    let num = arith::sub(enemy, 1);
    let mut value = arith::mul(basic, arith::add(preset.basic_stat_bonus_multiplier, num));
    let mut value2 =
        arith::mul(derivative, arith::add(preset.derivative_stat_bonus_multiplier, num));
    let mut value3 = arith::mul(derivative, preset.ac_bonus_multiplier);
    // `Owner.Get<UnitPartSummonedMonster>()?.Summoner?.CR ?? Owner.CR`.
    if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartSummonedMonster") {
        w.coverage.unsupported("DifficultyStatAdvancement: summoned monster's summoner CR");
    }
    let cr = blueprint_cr(w, w.unit(u).and_then(|x| x.blueprint));
    value = apply_bonus_progression(&preset, cr, value, true);
    value2 = apply_bonus_progression(&preset, cr, value2, false);
    value3 = apply_bonus_progression(&preset, cr, value3, false);
    for t in &STATS[..6] {
        ops::add_modifier_unique(w, u, *t, value, c, D::Difficulty);
    }
    ops::add_modifier_unique(w, u, StatType::SkillPerception, value2, c, D::Difficulty);
    ops::add_modifier_unique(w, u, StatType::AC, arith::add(value2, value3), c, D::Difficulty);
    ops::add_modifier_unique(w, u, StatType::AdditionalAttackBonus, value2, c, D::Difficulty);
    let modified = w.stats(u).v(StatType::HitPoints).modified_value;
    let scaled = arith::floor_to_int(arith::store_f32(
        f64::from(modified) * f64::from(preset.hp_multiplier),
    ));
    let value4 = arith::sub(1.max(scaled), modified);
    ops::add_modifier_unique(w, u, StatType::HitPoints, value4, c, D::Difficulty);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decline() -> StatsAdjustmentPreset {
        // The shape of the shipped "Decline" preset (values as extracted).
        StatsAdjustmentPreset {
            preset: 2,
            basic_stat_bonus_multiplier: 1,
            basic_stat_coefficient_start: 0.0,
            basic_stat_coefficient_end: 0.0,
            basic_stat_coefficient_cr_cap: 13,
            derivative_stat_bonus_multiplier: 0,
            derivative_stat_coefficient_start: 0.5,
            derivative_stat_coefficient_end: 2.0,
            derivative_stat_coefficient_cr_cap: 13,
            hp_multiplier: 0.8,
            ac_bonus_multiplier: -1,
            ability_dc_bonus: -2,
            ability_dc_limit: 22,
            ability_dc_limit_coeff: 2,
            skill_check_dc_bonus: 0,
        }
    }

    #[test]
    fn bonus_progression_matches_the_formula() {
        let p = decline();
        // Basic stats on Decline: coefficient 0 → floor(0) = 0, and a positive value
        // returns the floored product.
        assert_eq!(apply_bonus_progression(&p, 5, 2, true), 0);
        // Negative values keep the smaller of the value and the scaled value.
        // cr 0: 0.5 → floor(-2 * 0.5) = -1 → min(-2, -1) = -2.
        assert_eq!(apply_bonus_progression(&p, 0, -2, false), -2);
        // cr 13 (at cap): 2.0 → floor(-2 * 2) = -4.
        assert_eq!(apply_bonus_progression(&p, 13, -2, false), -4);
        // cr 6: 0.5 + 1.5 * (6/13) = 1.1923… → floor(3 * 1.1923) = 3.
        assert_eq!(apply_bonus_progression(&p, 6, 3, false), 3);
        // cr above the cap is clamped to 1.
        assert_eq!(apply_bonus_progression(&p, 40, 1, false), 2);
        assert_eq!(apply_bonus_progression(&p, 0, 0, false), 0);
    }
}
