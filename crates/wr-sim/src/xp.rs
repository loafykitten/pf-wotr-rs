//! Experience (decompiled, not copied: `Kingmaker.Blueprints.Classes.Experience/
//! ExperienceHelper.cs`, `Kingmaker.Designers/GameHelper.cs` `GainExperience`,
//! `GainExperienceForSkillCheck`, `Kingmaker/Player.cs` `GainPartyExperience`,
//! `Kingmaker.UnitLogic/UnitProgressionData.cs` `GainExperience`/`AdvanceExperienceTo`,
//! `Kingmaker.DialogSystem.Blueprints/BlueprintDialogExperienceModifierTable.cs`,
//! `Kingmaker.Blueprints.Classes/BlueprintStatProgression.cs`, and the `GainExp` action).
//!
//! The tables come from `BlueprintRoot.Progression`: `CRTable` (XP per CR),
//! `DCToCRTable` (a check's DC to a CR) and `DialogXPModifierTable`. Leveling up on XP is
//! not automatic in Wrath (the player levels up in the UI), so gaining XP only changes
//! `Experience`.

use serde_json::json;

use crate::csharp::arith;
use crate::world::{EntityIdx, World};

/// `EncounterType` (`Kingmaker.Blueprints.Classes.Experience`).
pub const ENCOUNTER_NAMES: [&str; 7] =
    ["Mob", "Boss", "QuestMain", "QuestNormal", "ChallengeMinor", "ChallengeMajor", "SkillCheck"];

/// `ExperienceHelper.s_EncounterModifiers`.
pub const ENCOUNTER_MODIFIERS: [f64; 7] = [0.75, 3.0, 10.0, 5.0, 1.5, 4.0, 0.375];

/// `ExperienceGainStatistic.GainType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GainType {
    None = -1,
    Mob = 0,
    Quest = 1,
    Check = 2,
    Trap = 3,
}

fn progression(w: &World) -> Option<std::rc::Rc<crate::content::Blueprint>> {
    w.content.get(crate::unit::BLUEPRINT_ROOT)
}

/// `BlueprintStatProgression.GetBonus(level)` of a `BlueprintRoot.Progression` table.
pub fn table_bonus(w: &mut World, table: &str, level: i32) -> i32 {
    let Some(root) = progression(w) else {
        w.coverage.unsupported("BlueprintRoot missing");
        return 0;
    };
    let Some(g) = root.view().obj("Progression").and_then(|p| p.guid(table)) else {
        w.coverage.unsupported(format!("BlueprintRoot.Progression.{table} missing"));
        return 0;
    };
    let Some(t) = w.content.get(g) else {
        w.coverage.unsupported(format!("{table} blueprint missing"));
        return 0;
    };
    let bonuses: Vec<i64> = t
        .view()
        .seq("Bonuses")
        .iter()
        .map(|n| match n {
            wr_content::node::Node::Int(i) => *i,
            _ => 0,
        })
        .collect();
    match usize::try_from(level).ok().and_then(|l| bonuses.get(l)) {
        #[allow(clippy::cast_possible_truncation)]
        Some(v) => *v as i32,
        None => {
            w.log("error", format!("Can't find value for level {level} in {table}"));
            0
        }
    }
}

/// `DialogXPModifierTable.GetModifier(experience)` (`DialogExperience`: 0 None, 1 Small,
/// 2 Normal, 3 Large).
pub fn dialog_xp_modifier(w: &mut World, experience: i32) -> f32 {
    let field = match experience {
        0 => return 0.0,
        1 => "MultiplierLow",
        2 => "MultiplierNormal",
        3 => "MultiplierHigh",
        _ => {
            w.coverage.unsupported("DialogXPModifierTable.GetModifier: experience out of range");
            return 0.0;
        }
    };
    let g = progression(w)
        .and_then(|r| r.view().obj("Progression").and_then(|p| p.guid("m_DialogXPModifierTable")));
    match g.and_then(|g| w.content.get(g)) {
        Some(t) => t.view().f32(field),
        None => {
            w.coverage.unsupported("DialogXPModifierTable missing");
            0.0
        }
    }
}

/// `ExperienceHelper.GetXp(type, cr, modifier, count)` (outside dungeons).
pub fn get_xp(w: &mut World, encounter: i32, cr: i32, modifier: f32, count: Option<i32>) -> i32 {
    if is_dungeon_campaign(w) {
        w.coverage.unsupported("ExperienceHelper.GetXp: dungeon campaign");
    }
    let mut bonus = table_bonus(w, "m_CRTable", cr);
    let m = usize::try_from(encounter).ok().and_then(|i| ENCOUNTER_MODIFIERS.get(i)).copied();
    let Some(m) = m else {
        w.coverage.unsupported("ExperienceHelper.GetXp: encounter type out of range");
        return 0;
    };
    // `(int)(s_EncounterModifiers[type] * (double)bonus)`.
    bonus = arith::to_int(m * f64::from(bonus));
    // `Mathf.Abs(modifier - 1f) > 1E-06` (float arithmetic, compared in double).
    if f64::from(arith::store_f32(f64::from(modifier) - 1.0)).abs() > 1e-6 {
        // `(int)((float)bonus * modifier)`.
        bonus = arith::to_int(f64::from(arith::store_f32(f64::from(bonus) * f64::from(modifier))));
    }
    if let Some(c) = count {
        bonus = arith::mul(bonus, c);
    }
    bonus
}

pub(crate) fn is_dungeon_campaign(w: &World) -> bool {
    crate::narrative::etudes::player_campaign(w)
        .and_then(|g| w.content.get(g))
        .is_some_and(|b| b.view().bool("IsDungeon"))
}

/// `ExperienceHelper.GetDialogXp(experience, dc, crOverride)`; the area CR comes from the
/// loaded area (`BlueprintArea.CR`).
pub fn get_dialog_xp(w: &mut World, experience: i32, dc: Option<i32>, cr_override: bool) -> i32 {
    let modifier = dialog_xp_modifier(w, experience);
    if cr_override {
        w.coverage.unsupported("ExperienceHelper.GetDialogXp: OverrideAreaCR");
    }
    let cr = match dc {
        Some(dc) => table_bonus(w, "m_DCToCRTable", dc),
        None => area_cr(w),
    };
    get_xp(w, 6, cr, modifier, None)
}

/// `CurrentlyLoadedArea.AreaCR` (not a dungeon area).
pub fn area_cr(w: &mut World) -> i32 {
    match w.player.area.and_then(|g| w.content.get(g)) {
        Some(a) => a.view().i32("CR"),
        None => {
            w.coverage.unsupported("AreaCR: area blueprint missing");
            0
        }
    }
}

fn campaign_allows(w: &World, gain: GainType) -> bool {
    let Some(c) = crate::narrative::etudes::player_campaign(w).and_then(|g| w.content.get(g))
    else {
        return true;
    };
    let comps = c.components_array();
    let Some(e) = comps.iter().find(|o| crate::content::is_a(o.ty, "BlueprintCampaignExperience"))
    else {
        return true;
    };
    let v = crate::content::View(e);
    match gain {
        GainType::Mob => v.bool("AllowMobs"),
        GainType::Quest => v.bool("AllowQuests"),
        GainType::Check => v.bool("AllowChecks"),
        GainType::Trap => v.bool("AllowTraps"),
        GainType::None => true,
    }
}

/// `UnitProgressionData.GainExperience(exp, log, allowForPet, statType)`. The trace record
/// is WrathOracle's postfix: written on every return, with the `exp` argument and the
/// experience after.
pub fn unit_gain_experience(
    w: &mut World,
    u: EntityIdx,
    exp: i32,
    allow_for_pet: bool,
    gain: GainType,
) {
    gain_experience_body(w, u, exp, allow_for_pet, gain);
    let id = w.entity(u).unique_id.clone();
    let total = w.unit(u).map_or(0, |x| x.progression.experience);
    w.events.mutation("xp", json!({ "unit": id, "gained": exp, "total": total }));
}

fn gain_experience_body(
    w: &mut World,
    u: EntityIdx,
    exp: i32,
    allow_for_pet: bool,
    gain: GainType,
) {
    if w.unit(u).is_some_and(|x| x.is_pet) && !allow_for_pet {
        return;
    }
    let rate = w.player.experience_rate_percent;
    let r = arith::div(arith::mul(exp, rate), 100).unwrap_or(0);
    if r < 1 {
        return;
    }
    if !campaign_allows(w, gain) {
        return;
    }
    let Some(unit) = w.unit_mut(u) else { return };
    #[allow(clippy::cast_possible_truncation)]
    let total = arith::add(unit.progression.experience as i32, r);
    unit.progression.experience = i64::from(total);
    if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartDualCompanion") {
        w.coverage.unsupported("UnitProgressionData.GainExperience: dual companion pair");
    }
    let _ = total;
    crate::rulebook::bus::raise_global(w, "IUnitGainExperienceHandler", None);
}

/// `AdvanceExperienceTo(target)`: `GainExperience(target - Experience, …)`.
pub fn advance_experience_to(w: &mut World, u: EntityIdx, target: i32) {
    #[allow(clippy::cast_possible_truncation)]
    let cur = w.unit(u).map_or(0, |x| x.progression.experience as i32);
    unit_gain_experience(w, u, arith::sub(target, cur), false, GainType::None);
}

/// `Player.GainPartyExperience(gained, statType)`.
pub fn gain_party_experience(w: &mut World, gained: i32, gain: GainType) {
    if !campaign_allows(w, gain) {
        return;
    }
    let only_active =
        w.settings.difficulty.settings.only_active_companions_receive_experience.unwrap_or(false);
    let flag = !only_active || w.player.capital_party_mode;
    let lists = crate::narrative::party::character_lists(w);
    let units: Vec<EntityIdx> = if flag {
        lists
            .all_characters
            .into_iter()
            .filter(|u| w.unit(*u).is_some_and(|x| x.master.is_none()))
            .collect()
    } else {
        lists.party
    };
    let mut each = gained;
    if !flag {
        let alive = units.iter().filter(|u| !is_finally_dead(w, **u)).count();
        if alive < 1 {
            return;
        }
        each = arith::div(arith::mul(gained, 6), i32::try_from(alive).unwrap_or(i32::MAX))
            .unwrap_or(0);
    }
    let mut done: Vec<EntityIdx> = Vec::new();
    for u in units {
        if (flag || !is_finally_dead(w, u)) && !done.contains(&u) {
            done.push(u);
            unit_gain_experience(w, u, each, false, gain);
        }
    }
    crate::rulebook::bus::raise_global(w, "IPartyGainExperienceHandler", None);
    // The game log's handler (`GameLogEventPartyGainExperience`), raised when the event's
    // experience (`flag ? gained : each`) is positive.
    let event_exp = if flag { gained } else { each };
    if event_exp > 0 {
        crate::game_log::party_gain_experience(w, event_exp);
    }
}

fn is_finally_dead(w: &World, u: EntityIdx) -> bool {
    // Hit points at or below minus Constitution with the dying condition, or a dead
    // flag; the meetings' units are alive. Dead units would need `UnitState`: report.
    let _ = (w, u);
    false
}

/// `GameHelper.GainExperience(gained, unit = null, Quest)`.
pub fn gain_experience(w: &mut World, gained: i32) {
    gain_party_experience(w, gained, GainType::Quest);
}

/// `UnitEntityData.BonusExperience`: `DifficultyList.GetExperienceBonus(CombatEncountersCapacity)`
/// = `MobExperienceCoeffs.GetClamped(capacity + 1)`.
pub fn bonus_experience(w: &mut World) -> f32 {
    let Some(list) = w.settings.difficulty.list.as_ref() else {
        w.coverage.unsupported("DifficultyPresetsList not extracted (BonusExperience)");
        return 0.0;
    };
    let Some(cap) = w.settings.difficulty.settings.combat_encounters_capacity else {
        w.coverage.unsupported("combat-encounters-capacity setting missing (BonusExperience)");
        return 0.0;
    };
    let coeffs = &list.mob_experience_coeffs;
    if coeffs.is_empty() {
        return 0.0;
    }
    let i = usize::try_from((cap + 1).max(0)).unwrap_or(0).min(coeffs.len() - 1);
    coeffs[i]
}

/// `UnitEntityData.Initialize`'s `BonusExperience = GetExperienceBonus(…)` for a unit.
pub fn init_bonus_experience(w: &mut World, u: EntityIdx) {
    let b = bonus_experience(w);
    if let Some(x) = w.unit_mut(u) {
        x.bonus_experience = Some(b);
    }
}

/// `Experience.Apply(component, null, extraMod)` for each `Experience` component of the unit's
/// blueprint (`UnitLifeController.GiveExperience`, outside dungeons): `GameHelper.GainExperience(
/// Mathf.RoundToInt(GetXp(Encounter, CR, Modifier, Count) * (1 + extraMod)), null, Mob)`.
pub fn apply_blueprint_experience(w: &mut World, u: EntityIdx, extra_mod: f32) {
    let Some(bp) = w.unit(u).and_then(|x| x.blueprint).and_then(|g| w.content.get(g)) else {
        return;
    };
    let comps: Vec<_> = bp
        .components_array()
        .into_iter()
        .filter(|o| wr_content::schema::ty(o.ty).short == "Experience")
        .map(crate::content::View)
        .collect();
    for v in comps {
        if v.obj("Count").is_some() {
            w.coverage.unsupported("Experience.Count (IntEvaluator)");
        }
        if v.bool("PlayerGainsNoExp") {
            continue;
        }
        let xp = get_xp(w, v.i32("Encounter"), v.i32("CR"), v.f32("Modifier"), None);
        // `(float)xp * (1f + extraMod)` rounded to even (`Mathf.RoundToInt`).
        #[allow(clippy::cast_precision_loss)]
        let scaled = arith::store_f32(f64::from(xp as f32) * f64::from(1.0_f32 + extra_mod));
        let gained = arith::round_to_int(scaled);
        // `GameHelper.GainExperience(gained, null, Mob)`: no unit, the party shares it.
        gain_party_experience(w, gained, GainType::Mob);
    }
}

/// `GameHelper.GainExperienceForSkillCheck(gained, unit)`.
pub fn gain_experience_for_skill_check(w: &mut World, gained: i32, unit: EntityIdx) {
    let u = w.unit(unit).and_then(|x| x.master).unwrap_or(unit);
    let only_initiator = w
        .settings
        .difficulty
        .settings
        .only_initiator_receive_skill_check_experience
        .unwrap_or(false);
    if only_initiator {
        unit_gain_experience(w, u, arith::mul(gained, 6), false, GainType::Check);
    } else {
        gain_party_experience(w, gained, GainType::Check);
    }
}
