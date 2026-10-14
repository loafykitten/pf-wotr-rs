//! Difficulty: the `DifficultyPresetsList` ScriptableObject (rules data that lives in
//! `Bundles/blueprint.assets`, not in a blueprint) and the save's difficulty settings.
//!
//! C# (decompiled, not copied): `Kingmaker.Settings.Difficulty/DifficultyPresetsList.cs`
//! (`StatsAdjustmentPreset`, `GetAdjustmentPreset`, `GetExperienceBonus`),
//! `DifficultyPreset.cs`, `Kingmaker.Settings/DifficultySettings.cs` (the save-dependent
//! settings keys), `StatsAdjustmentsType.cs`, `EnemyDifficulty.cs`.
//!
//! The runtime never parses Unity formats (Decision 4): `tools/wr-rules-data` reads
//! `blueprint.assets` through `wr-import`'s type-tree reader and writes
//! `cache/sim/rules-data.json` in the shape of [`RulesData`], which the sim loads.
//! `BlueprintRoot.m_DifficultyList` is an index into `BlueprintReferencedAssets`; the
//! extractor follows that index, so the list read here is the one the game uses.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// `DifficultyPresetsList.StatsAdjustmentPreset`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StatsAdjustmentPreset {
    /// `StatsAdjustmentsType` (0 `ExtraDecline` … 6 `StrongImprovement`).
    pub preset: i32,
    pub basic_stat_bonus_multiplier: i32,
    pub basic_stat_coefficient_start: f32,
    pub basic_stat_coefficient_end: f32,
    #[serde(rename = "BasicStatCoefficientCRCap")]
    pub basic_stat_coefficient_cr_cap: i32,
    pub derivative_stat_bonus_multiplier: i32,
    pub derivative_stat_coefficient_start: f32,
    pub derivative_stat_coefficient_end: f32,
    #[serde(rename = "DerivativeStatCoefficientCRCap")]
    pub derivative_stat_coefficient_cr_cap: i32,
    #[serde(rename = "HPMultiplier")]
    pub hp_multiplier: f32,
    #[serde(rename = "ACBonusMultiplier")]
    pub ac_bonus_multiplier: i32,
    #[serde(rename = "AbilityDCBonus")]
    pub ability_dc_bonus: i32,
    #[serde(rename = "AbilityDCLimit")]
    pub ability_dc_limit: i32,
    #[serde(rename = "AbilityDCLimitCoeff")]
    pub ability_dc_limit_coeff: i32,
    #[serde(rename = "SkillCheckDCBonus")]
    pub skill_check_dc_bonus: i32,
}

/// `DifficultyPreset` (the settings a named difficulty sets).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DifficultyPreset {
    pub game_difficulty: i32,
    pub death_door: bool,
    pub dead_companions_rise_after_combat: bool,
    pub remove_negative_effects_on_rest: bool,
    pub remove_annoying_buffs_after_combat: bool,
    pub damage_to_party: f32,
    pub enemy_critical_hits: i32,
    pub auto_level_up: i32,
    pub stats_adjustments: i32,
    pub enemy_difficulty: i32,
    pub encumbrance_slowdown: bool,
    pub weather_effects: i32,
    pub respec_allowed: bool,
    #[serde(rename = "AdditionalAIBehaviors")]
    pub additional_ai_behaviors: bool,
    pub combat_encounters_capacity: i32,
}

/// A `DifficultyPresetAsset` (one entry of `m_Difficulties`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DifficultyPresetAsset {
    pub name: String,
    pub path_id: i64,
    pub is_custom_mode: bool,
    pub preset: DifficultyPreset,
}

/// `DifficultyPresetsList`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DifficultyPresetsList {
    pub path_id: i64,
    /// `m_Difficulties`, in order.
    pub difficulties: Vec<DifficultyPresetAsset>,
    /// `m_CoreDifficulty`, `m_HardDifficulty`, `m_UnfairDifficulty` (path ids).
    pub core_difficulty: i64,
    pub hard_difficulty: i64,
    pub unfair_difficulty: i64,
    /// `StatsAdjustmentPresets`, in order.
    pub stats_adjustment_presets: Vec<StatsAdjustmentPreset>,
    /// `MobExperienceCoeffs`.
    pub mob_experience_coeffs: Vec<f32>,
}

impl DifficultyPresetsList {
    /// `GetAdjustmentPreset(preset)`: `FirstOrDefault(a => a.Preset == preset)`.
    #[must_use]
    pub fn adjustment_preset(&self, preset: i32) -> Option<&StatsAdjustmentPreset> {
        self.stats_adjustment_presets.iter().find(|p| p.preset == preset)
    }
}

/// What `tools/wr-rules-data` extracts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RulesData {
    pub schema: String,
    /// Where it came from: the bundle file, the serialized file, the asset index that
    /// `BlueprintRoot.m_DifficultyList` holds, and the resolved object.
    pub source: BTreeMap<String, serde_json::Value>,
    pub difficulty_presets_list: DifficultyPresetsList,
}

pub const RULES_DATA_SCHEMA: &str = "wr-rules-data-v1";

impl RulesData {
    /// `cache/sim/rules-data.json`.
    #[must_use]
    pub fn default_path() -> std::path::PathBuf {
        wr_paths::cache_root().join("sim/rules-data.json")
    }

    /// Loads the extracted rules data.
    ///
    /// # Errors
    ///
    /// Missing file (run `cargo run -p wr-rules-data`), bad JSON or another schema.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let b = std::fs::read(path).with_context(|| {
            format!("{} (run `cargo run --release -p wr-rules-data` first)", path.display())
        })?;
        let r: Self = serde_json::from_slice(&b).context("rules-data.json")?;
        anyhow::ensure!(r.schema == RULES_DATA_SCHEMA, "rules data schema {}", r.schema);
        Ok(r)
    }
}

/// `StatsAdjustmentsType` names, by value.
pub const STATS_ADJUSTMENTS: &[&str] = &[
    "ExtraDecline",
    "StrongDecline",
    "Decline",
    "SlightDecline",
    "None",
    "Improvement",
    "StrongImprovement",
];
/// `EnemyDifficulty` names, by value.
pub const ENEMY_DIFFICULTY: &[&str] = &["Weak", "Normal", "Strong", "Insane"];
/// `GameDifficultyOption` names, by value + 1 (`Custom` is -1).
pub const GAME_DIFFICULTY: &[&str] =
    &["Custom", "Story", "Casual", "Normal", "Daring", "Core", "Hard", "Unfair"];
/// `CombatEncountersCapacity` names.
pub const COMBAT_ENCOUNTERS_CAPACITY: &[&str] = &["Standard", "Enlarged"];

/// The save-dependent difficulty settings (`SettingsRoot.Difficulty`), read from the save's
/// `player.json` `SettingsList` (keys `settings.difficulty.<key>`). Enum settings are
/// written by name; bools and floats as JSON values.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct DifficultySettings {
    pub game_difficulty: Option<i32>,
    pub stats_adjustments: Option<i32>,
    pub enemy_difficulty: Option<i32>,
    pub combat_encounters_capacity: Option<i32>,
    pub only_active_companions_receive_experience: Option<bool>,
    pub only_initiator_receive_skill_check_experience: Option<bool>,
    pub dead_companions_rise_after_combat: Option<bool>,
    /// Every `settings.difficulty.*` entry as saved, for reports.
    pub raw: BTreeMap<String, serde_json::Value>,
}

fn enum_index(names: &[&str], v: &serde_json::Value) -> Option<i32> {
    match v {
        serde_json::Value::String(s) => {
            names.iter().position(|n| n == s).and_then(|i| i32::try_from(i).ok())
        }
        serde_json::Value::Number(n) => n.as_i64().and_then(|i| i32::try_from(i).ok()),
        _ => None,
    }
}

impl DifficultySettings {
    /// From the save's `SettingsList` (a JSON object of key → value).
    #[must_use]
    pub fn from_settings_list(list: &serde_json::Map<String, serde_json::Value>) -> Self {
        let get = |k: &str| list.get(&format!("settings.difficulty.{k}"));
        let raw = list
            .iter()
            .filter(|(k, _)| k.starts_with("settings.difficulty."))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        Self {
            game_difficulty: get("game-difficulty")
                .and_then(|v| enum_index(GAME_DIFFICULTY, v))
                .map(|i| i - 1),
            stats_adjustments: get("stats-adjustments")
                .and_then(|v| enum_index(STATS_ADJUSTMENTS, v)),
            enemy_difficulty: get("enemy-difficulty").and_then(|v| enum_index(ENEMY_DIFFICULTY, v)),
            combat_encounters_capacity: get("combat-encounters-capacity")
                .and_then(|v| enum_index(COMBAT_ENCOUNTERS_CAPACITY, v)),
            only_active_companions_receive_experience: get(
                "only-active-companions-receive-experience",
            )
            .and_then(serde_json::Value::as_bool),
            // The game's key really is spelled "chick".
            only_initiator_receive_skill_check_experience: get(
                "only-initiator-receive-skill-chick-experience",
            )
            .and_then(serde_json::Value::as_bool),
            dead_companions_rise_after_combat: get("dead-companion-rise-after-combat")
                .and_then(serde_json::Value::as_bool),
            raw,
        }
    }
}

/// The difficulty state the rules read: the presets list (if extracted) and the save's
/// settings.
#[derive(Debug, Clone, Default)]
pub struct Difficulty {
    pub list: Option<DifficultyPresetsList>,
    pub settings: DifficultySettings,
}

impl Difficulty {
    /// `BlueprintRoot.Instance.DifficultyList.GetAdjustmentPreset(SettingsRoot.Difficulty
    /// .StatsAdjustments)`, or `None` with the reason (no extracted list, no setting, or
    /// no preset of that type: the C# would throw a `NullReferenceException`).
    ///
    /// # Errors
    ///
    /// The reason the preset isn't available.
    pub fn adjustment_preset(&self) -> Result<&StatsAdjustmentPreset, &'static str> {
        let list = self.list.as_ref().ok_or("DifficultyPresetsList not extracted")?;
        let s = self.settings.stats_adjustments.ok_or("stats-adjustments setting missing")?;
        list.adjustment_preset(s).ok_or("no StatsAdjustmentPreset for the setting")
    }

    /// `DifficultyPresetsController.CurrentDifficultyStatAdjustmentCompareTo(option)`: the
    /// current `StatsAdjustments` setting against that of the preset at the option's ordinal
    /// among the enum's values (`Custom` = -1 first), as -1, 0 or 1.
    ///
    /// # Errors
    ///
    /// The reason the comparison isn't available.
    pub fn stat_adjustment_compare_to(&self, option: i32) -> Result<i32, &'static str> {
        let list = self.list.as_ref().ok_or("DifficultyPresetsList not extracted")?;
        let cur = self.settings.stats_adjustments.ok_or("stats-adjustments setting missing")?;
        let ord = usize::try_from(option + 1).map_err(|_| "difficulty option out of range")?;
        let preset = list.difficulties.get(ord).ok_or("no preset at the option's ordinal")?;
        Ok(match cur.cmp(&preset.preset.stats_adjustments) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_parse_names_as_the_enums() {
        let j: serde_json::Value = serde_json::json!({
            "settings.difficulty.game-difficulty": "Normal",
            "settings.difficulty.stats-adjustments": "Decline",
            "settings.difficulty.enemy-difficulty": "Normal",
            "settings.difficulty.combat-encounters-capacity": "Standard",
            "settings.difficulty.only-initiator-receive-skill-chick-experience": false,
            "settings.difficulty.only-active-companions-receive-experience": false,
            "settings.game.other": true
        });
        let s = DifficultySettings::from_settings_list(j.as_object().unwrap());
        assert_eq!(s.game_difficulty, Some(2));
        assert_eq!(s.stats_adjustments, Some(2));
        assert_eq!(s.enemy_difficulty, Some(1));
        assert_eq!(s.combat_encounters_capacity, Some(0));
        assert_eq!(s.only_initiator_receive_skill_check_experience, Some(false));
        assert_eq!(s.raw.len(), 6);
    }
}
