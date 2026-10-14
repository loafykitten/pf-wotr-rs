//! `wr-rules-data`: extracts rules data that lives outside blueprints into
//! `cache/sim/rules-data.json` (read-only on the game install).
//!
//! Today: `DifficultyPresetsList` (skill-check DC adjustments, stat and HP scaling for
//! `DifficultyStatAdvancement`, mob XP coefficients) and its `DifficultyPresetAsset`s,
//! from `Bundles/blueprint.assets`. The list is found the way the game finds it:
//! `BlueprintRoot.m_DifficultyList` (read from `blueprints-pack.bbp` with `wr-content`)
//! is an index into `BlueprintReferencedAssets.m_Entries`, whose entry points at the
//! object. Unity objects are decoded with `wr-import`'s type-tree reader.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use wr_content::node::Node;
use wr_import::unity::bundle::Bundle;
use wr_import::unity::serialized::{SerializedFile, class_id};
use wr_import::unity::value::Value;
use wr_sim::difficulty::{
    DifficultyPreset, DifficultyPresetAsset, DifficultyPresetsList, RULES_DATA_SCHEMA, RulesData,
    StatsAdjustmentPreset,
};

#[derive(Parser, Debug)]
#[command(about = "Extract rules data kept outside blueprints (DifficultyPresetsList)")]
struct Cli {
    /// Output (default `cache/sim/rules-data.json`).
    #[arg(long)]
    out: Option<PathBuf>,
}

/// `BlueprintRoot`.
const BLUEPRINT_ROOT: &str = "2d77316c72b9ed44f888ceefc2a131f6";

fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = wr_paths::GamePaths::from_env();
    let bundles = paths.bundles();

    // 1. The asset index BlueprintRoot holds.
    let bbp_path = bundles.join("blueprints-pack.bbp");
    let bbp = wr_content::bbp::Bbp::open(&bbp_path).context("open .bbp")?;
    let root_guid = wr_content::Guid::parse(BLUEPRINT_ROOT).context("guid")?;
    let off = bbp
        .toc
        .iter()
        .find(|(g, _)| *g == root_guid)
        .map(|(_, o)| *o)
        .context("BlueprintRoot not in the .bbp")?;
    let rec = bbp.decode(off)?;
    let root = rec.data.context("BlueprintRoot record empty")?;
    let t = wr_content::schema::ty(root.ty);
    let fi = t
        .fields
        .iter()
        .position(|f| f.name == "m_DifficultyList")
        .context("BlueprintRoot.m_DifficultyList")?;
    let asset_index = match &root.fields[fi] {
        Node::AssetIndex(i) => *i,
        other => bail!("m_DifficultyList is {other:?}, expected an asset index"),
    };
    ensure!(asset_index >= 0, "m_DifficultyList is null");

    // 2. blueprint.assets: find BlueprintReferencedAssets and follow the index.
    let assets = bundles.join("blueprint.assets");
    let bundle = Bundle::open(&assets)?;
    let mut found: Option<(String, DifficultyPresetsList, i64)> = None;
    for node in &bundle.header.nodes {
        if node.path.ends_with(".resS") || node.path.ends_with(".resource") {
            continue;
        }
        let data = bundle.node_data(node)?;
        let Ok(f) = SerializedFile::parse(&node.path, data) else { continue };
        let script = |v: &Value| -> Option<String> {
            let p = v.get("m_Script")?.as_pptr()?;
            if p.file_id != 0 {
                return None;
            }
            let o = f.object(p.path_id)?;
            f.read(o).ok()?.str_at("m_ClassName").map(str::to_owned)
        };
        let mut referenced = None;
        let mut lists = Vec::new();
        for o in f.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
            let Ok(v) = f.read(o) else { continue };
            match script(&v).as_deref() {
                Some("BlueprintReferencedAssets") => referenced = Some(v),
                Some("DifficultyPresetsList") => lists.push(o.path_id),
                _ => {}
            }
        }
        let Some(refs) = referenced else { continue };
        let entries = refs.get("m_Entries").and_then(Value::as_array).context("m_Entries")?;
        let idx = usize::try_from(asset_index)?;
        let entry = entries.get(idx).context("asset index out of range")?;
        let p = entry.get("Asset").and_then(Value::as_pptr).context("entry Asset")?;
        ensure!(p.file_id == 0, "DifficultyList entry points into another file");
        ensure!(lists.contains(&p.path_id), "entry {idx} is not a DifficultyPresetsList");
        let o = f.object(p.path_id).context("list object")?;
        let v = f.read(o)?;
        let list = presets_list(&f, &v, p.path_id)?;
        found = Some((node.path.clone(), list, p.path_id));
        break;
    }
    let (file, list, path_id) =
        found.context("no BlueprintReferencedAssets in blueprint.assets")?;
    let mut source = BTreeMap::new();
    source.insert("bundle".into(), serde_json::json!("Bundles/blueprint.assets"));
    source.insert("serialized_file".into(), serde_json::json!(file));
    source.insert("blueprint_root_asset_index".into(), serde_json::json!(asset_index));
    source.insert("path_id".into(), serde_json::json!(path_id));
    let out = RulesData { schema: RULES_DATA_SCHEMA.into(), source, difficulty_presets_list: list };
    let path = cli.out.unwrap_or_else(RulesData::default_path);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&out)?)?;
    eprintln!(
        "DifficultyPresetsList: {} presets, {} stats-adjustment presets -> {}",
        out.difficulty_presets_list.difficulties.len(),
        out.difficulty_presets_list.stats_adjustment_presets.len(),
        path.display()
    );
    Ok(())
}

fn int(v: &Value, k: &str) -> Result<i64> {
    v.get(k).and_then(Value::as_i64).with_context(|| format!("field {k}"))
}

fn i32f(v: &Value, k: &str) -> Result<i32> {
    Ok(i32::try_from(int(v, k)?)?)
}

fn f32f(v: &Value, k: &str) -> Result<f32> {
    v.get(k).and_then(Value::as_f32).with_context(|| format!("field {k}"))
}

fn boolf(v: &Value, k: &str) -> Result<bool> {
    Ok(int(v, k)? != 0)
}

fn pptr_id(v: &Value, k: &str) -> Result<i64> {
    let p = v.get(k).and_then(Value::as_pptr).with_context(|| format!("field {k}"))?;
    ensure!(p.file_id == 0, "{k} points into another file");
    Ok(p.path_id)
}

fn presets_list(f: &SerializedFile<'_>, v: &Value, path_id: i64) -> Result<DifficultyPresetsList> {
    let mut difficulties = Vec::new();
    for d in v.get("m_Difficulties").and_then(Value::as_array).context("m_Difficulties")? {
        let p = d.as_pptr().context("difficulty pptr")?;
        ensure!(p.file_id == 0, "difficulty in another file");
        let o = f.object(p.path_id).context("difficulty object")?;
        let a = f.read(o)?;
        let pr = a.get("Preset").context("Preset")?;
        difficulties.push(DifficultyPresetAsset {
            name: a.str_at("m_Name").unwrap_or_default().to_owned(),
            path_id: p.path_id,
            is_custom_mode: boolf(&a, "IsCustomMode")?,
            preset: DifficultyPreset {
                game_difficulty: i32f(pr, "GameDifficulty")?,
                death_door: boolf(pr, "DeathDoor")?,
                dead_companions_rise_after_combat: boolf(pr, "DeadCompanionsRiseAfterCombat")?,
                remove_negative_effects_on_rest: boolf(pr, "RemoveNegativeEffectsOnRest")?,
                remove_annoying_buffs_after_combat: boolf(pr, "RemoveAnnoyingBuffsAfterCombat")?,
                damage_to_party: f32f(pr, "DamageToParty")?,
                enemy_critical_hits: i32f(pr, "EnemyCriticalHits")?,
                auto_level_up: i32f(pr, "AutoLevelUp")?,
                stats_adjustments: i32f(pr, "StatsAdjustments")?,
                enemy_difficulty: i32f(pr, "EnemyDifficulty")?,
                encumbrance_slowdown: boolf(pr, "EncumbranceSlowdown")?,
                weather_effects: i32f(pr, "WeatherEffects")?,
                respec_allowed: boolf(pr, "RespecAllowed")?,
                additional_ai_behaviors: boolf(pr, "AdditionalAIBehaviors")?,
                combat_encounters_capacity: i32f(pr, "CombatEncountersCapacity")?,
            },
        });
    }
    let mut stats = Vec::new();
    for s in v.get("StatsAdjustmentPresets").and_then(Value::as_array).context("presets")? {
        stats.push(StatsAdjustmentPreset {
            preset: i32f(s, "Preset")?,
            basic_stat_bonus_multiplier: i32f(s, "BasicStatBonusMultiplier")?,
            basic_stat_coefficient_start: f32f(s, "BasicStatCoefficientStart")?,
            basic_stat_coefficient_end: f32f(s, "BasicStatCoefficientEnd")?,
            basic_stat_coefficient_cr_cap: i32f(s, "BasicStatCoefficientCRCap")?,
            derivative_stat_bonus_multiplier: i32f(s, "DerivativeStatBonusMultiplier")?,
            derivative_stat_coefficient_start: f32f(s, "DerivativeStatCoefficientStart")?,
            derivative_stat_coefficient_end: f32f(s, "DerivativeStatCoefficientEnd")?,
            derivative_stat_coefficient_cr_cap: i32f(s, "DerivativeStatCoefficientCRCap")?,
            hp_multiplier: f32f(s, "HPMultiplier")?,
            ac_bonus_multiplier: i32f(s, "ACBonusMultiplier")?,
            ability_dc_bonus: i32f(s, "AbilityDCBonus")?,
            ability_dc_limit: i32f(s, "AbilityDCLimit")?,
            ability_dc_limit_coeff: i32f(s, "AbilityDCLimitCoeff")?,
            skill_check_dc_bonus: i32f(s, "SkillCheckDCBonus")?,
        });
    }
    let mob = v
        .get("MobExperienceCoeffs")
        .and_then(Value::as_array)
        .context("MobExperienceCoeffs")?
        .iter()
        .map(|x| x.as_f32().context("coeff"))
        .collect::<Result<Vec<f32>>>()?;
    Ok(DifficultyPresetsList {
        path_id,
        difficulties,
        core_difficulty: pptr_id(v, "m_CoreDifficulty")?,
        hard_difficulty: pptr_id(v, "m_HardDifficulty")?,
        unfair_difficulty: pptr_id(v, "m_UnfairDifficulty")?,
        stats_adjustment_presets: stats,
        mob_experience_coeffs: mob,
    })
}
