//! `wr-rules-data`: extracts rules data that lives outside blueprints into
//! `cache/sim/rules-data.json` (read-only on the game install).
//!
//! Today: `DifficultyPresetsList` (skill-check DC adjustments, stat and HP scaling for
//! `DifficultyStatAdvancement`, mob XP coefficients) and its `DifficultyPresetAsset`s,
//! from `Bundles/blueprint.assets`. The list is found the way the game finds it:
//! `BlueprintRoot.m_DifficultyList` (read from `blueprints-pack.bbp` with `wr-content`)
//! is an index into `BlueprintReferencedAssets.m_Entries`, whose entry points at the
//! object. Unity objects are decoded with `wr-import`'s type-tree reader.
//!
//! Also `cache/sim/shared-strings.json` ([`SharedStrings`]): the runtime pack holds a
//! `SharedStringAsset` field as its index into the same list, so for every entry that is a
//! `SharedStringAsset`, the key its string resolves to (`LocalizedString.GetActualKey`:
//! `m_Key`, or the end of its `Shared` chain).
//!
//! And `cache/sim/doll-prefabs.json`: the `UnitEntityView` settings (`m_Corpulence`, the size scale
//! override) of `BlueprintRoot.CharGen.MaleDoll` and `FemaleDoll`, the prefabs a unit with a
//! doll gets its view from (`DollData.CreateUnitView`).
//!
//! And `cache/sim/log-strings.json` ([`LogStrings`]): `BlueprintRoot.LocalizedTexts`' string
//! containers, plus `SettingsRoot` (`BlueprintRoot.SettingsRoot.Settings`' `DamageToParty` and
//! `EnemyCriticalHits` entities, whose texts the combat log's damage tooltip reads).

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use wr_content::node::Node;
use wr_import::unity::bundle::Bundle;
use wr_import::unity::serialized::{SerializedFile, class_id};
use wr_import::unity::value::Value;
use wr_sim::area_parts::{
    AREA_BOUNDS_SCHEMA, AreaBoundsTable, AreaPartBoundsAsset, Bounds, FogOfWarBoundsAsset,
};
use wr_sim::difficulty::{
    DifficultyPreset, DifficultyPresetAsset, DifficultyPresetsList, RULES_DATA_SCHEMA, RulesData,
    StatsAdjustmentPreset,
};
use wr_sim::game_log::strings::{LOG_STRINGS_SCHEMA, LogStrings};
use wr_sim::text::{SHARED_STRINGS_SCHEMA, SharedStrings};

#[derive(Parser, Debug)]
#[command(about = "Extract rules data kept outside blueprints (DifficultyPresetsList)")]
struct Cli {
    /// Output (default `cache/sim/rules-data.json`).
    #[arg(long)]
    out: Option<PathBuf>,
    /// The shared strings' output (default `cache/sim/shared-strings.json`).
    #[arg(long)]
    shared_strings_out: Option<PathBuf>,
    /// The log strings' output (default `cache/sim/log-strings.json`).
    #[arg(long)]
    log_strings_out: Option<PathBuf>,
    /// The area parts' bounds' output (default `cache/sim/area-bounds.json`).
    #[arg(long)]
    area_bounds_out: Option<PathBuf>,
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
    // `LocalizedTexts`' string containers (each an asset index).
    let containers = localized_texts_containers(&root)?;
    // `BlueprintRoot.SettingsRoot` (`UISettingsRoot`, another asset index): the settings' UI texts
    // the combat log's damage tooltip reads.
    let settings_root = match field_node(&root, "SettingsRoot") {
        Some(Node::AssetIndex(i)) if i >= 0 => Some(i),
        _ => None,
    };

    // 2. blueprint.assets: find BlueprintReferencedAssets and follow the index.
    let assets = bundles.join("blueprint.assets");
    let bundle = Bundle::open(&assets)?;
    let mut found: Option<(String, DifficultyPresetsList, i64)> = None;
    let mut shared: Option<SharedStrings> = None;
    let mut log: Option<LogStrings> = None;
    let mut area: Option<AreaBoundsTable> = None;
    let mut dolls: Option<serde_json::Value> = None;
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
        shared = Some(shared_strings(&f, entries, &node.path, &script)?);
        let mut l = log_strings(&f, entries, &containers, &node.path)?;
        if let Some(ix) = settings_root {
            settings_ui(&f, entries, ix, &mut l)?;
        }
        log = Some(l);
        area = Some(area_bounds(&f, entries, &node.path, &script)?);
        dolls = Some(doll_prefabs(&f, entries, &root, &node.path)?);
        let idx = usize::try_from(asset_index)?;
        let entry = entries.get(idx).context("asset index out of range")?;
        let p = entry.get("Asset").and_then(Value::as_pptr).context("entry Asset")?;
        ensure!(p.file_id == 0, "DifficultyList entry points into another file");
        ensure!(lists.contains(&p.path_id), "entry {idx} is not a DifficultyPresetsList");
        let o = f.object(p.path_id).context("list object")?;
        let v = f.read(o)?;
        let mut list = presets_list(&f, &v, p.path_id)?;
        // The entries that point at one of its presets (a blueprint's `DifficultyPresetAsset`
        // field holds such an index).
        for (i, e) in entries.iter().enumerate() {
            let Some(q) = e.get("Asset").and_then(Value::as_pptr) else { continue };
            if q.file_id == 0 && list.difficulties.iter().any(|d| d.path_id == q.path_id) {
                list.preset_asset_indices.insert(i32::try_from(i)?, q.path_id);
            }
        }
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
    let shared = shared.context("no shared strings")?;
    let spath = cli.shared_strings_out.unwrap_or_else(SharedStrings::default_path);
    if let Some(d) = spath.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&spath, serde_json::to_string_pretty(&shared)?)?;
    eprintln!(
        "shared strings: {} of {} referenced assets -> {}",
        shared.keys.len(),
        entries_count(&shared),
        spath.display()
    );
    let log = log.context("no log strings")?;
    let lpath = cli.log_strings_out.unwrap_or_else(LogStrings::default_path);
    if let Some(d) = lpath.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&lpath, serde_json::to_string_pretty(&log)?)?;
    eprintln!(
        "log strings: {} of {} LocalizedTexts containers{} -> {}",
        containers.iter().filter(|(n, _)| log.containers.contains_key(n)).count(),
        containers.len(),
        if log.containers.contains_key("SettingsRoot") { ", SettingsRoot" } else { "" },
        lpath.display()
    );
    let dolls = dolls.context("no doll prefabs")?;
    let dpath = wr_paths::cache_root().join("sim/doll-prefabs.json");
    std::fs::write(&dpath, serde_json::to_string_pretty(&dolls)?)?;
    eprintln!("doll prefabs -> {}", dpath.display());
    let area = area.context("no area bounds")?;
    let apath = cli.area_bounds_out.unwrap_or_else(AreaBoundsTable::default_path);
    if let Some(d) = apath.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&apath, serde_json::to_string_pretty(&area)?)?;
    eprintln!(
        "area bounds: {} AreaPartBounds ({} entries in other files) -> {}",
        area.bounds.len(),
        area.source.get("entries_in_other_files").and_then(serde_json::Value::as_u64).unwrap_or(0),
        apath.display()
    );
    eprintln!(
        "DifficultyPresetsList: {} presets, {} stats-adjustment presets -> {}",
        out.difficulty_presets_list.difficulties.len(),
        out.difficulty_presets_list.stats_adjustment_presets.len(),
        path.display()
    );
    Ok(())
}

fn entries_count(s: &SharedStrings) -> u64 {
    s.source.get("entries").and_then(serde_json::Value::as_u64).unwrap_or(0)
}

/// A `SharedStringAsset` (its `MonoScript` is usually in another file, so also by shape: a
/// `MonoBehaviour` whose only own field is the `LocalizedString` `String`).
fn is_shared_string_asset(v: &Value, script: &dyn Fn(&Value) -> Option<String>) -> bool {
    if let Some(s) = script(v) {
        return s == "SharedStringAsset";
    }
    let Value::Object(m) = v else { return false };
    let own: Vec<&str> = m
        .iter()
        .map(|(k, _)| k.as_str())
        .filter(|k| !matches!(*k, "m_GameObject" | "m_Enabled" | "m_Script" | "m_Name"))
        .collect();
    own == ["String"] && v.get("String").and_then(|s| s.get("m_Key")).is_some()
}

/// Every `BlueprintReferencedAssets` entry that is a `SharedStringAsset` in this file, with
/// its string's actual key: `m_Key`, or, while the string has a `Shared` asset, that
/// asset's string's (at most 50 steps, as `GetActualKey`).
fn shared_strings(
    f: &SerializedFile<'_>,
    entries: &[Value],
    file: &str,
    script: &dyn Fn(&Value) -> Option<String>,
) -> Result<SharedStrings> {
    let mut keys = BTreeMap::new();
    let mut elsewhere = 0u64;
    for (i, e) in entries.iter().enumerate() {
        let Some(p) = e.get("Asset").and_then(Value::as_pptr) else { continue };
        if p.path_id == 0 {
            continue;
        }
        if p.file_id != 0 {
            elsewhere += 1;
            continue;
        }
        let Some(o) = f.object(p.path_id) else { continue };
        if o.class_id != class_id::MONO_BEHAVIOUR {
            continue;
        }
        let v = f.read(o)?;
        if !is_shared_string_asset(&v, script) {
            continue;
        }
        let mut s = v.get("String").context("SharedStringAsset.String")?.clone();
        let mut steps = 0;
        loop {
            let next = s.get("Shared").and_then(Value::as_pptr).filter(|q| q.path_id != 0);
            let Some(q) = next.filter(|_| steps < 50) else { break };
            ensure!(q.file_id == 0, "entry {i}: a shared string in another file");
            let o = f.object(q.path_id).with_context(|| format!("entry {i}: shared asset"))?;
            s = f.read(o)?.get("String").context("SharedStringAsset.String")?.clone();
            steps += 1;
        }
        let key = s.str_at("m_Key").unwrap_or_default().to_owned();
        keys.insert(i32::try_from(i)?, key);
    }
    let mut source = BTreeMap::new();
    source.insert("bundle".into(), serde_json::json!("Bundles/blueprint.assets"));
    source.insert("serialized_file".into(), serde_json::json!(file));
    source.insert("entries".into(), serde_json::json!(entries.len()));
    source.insert("entries_in_other_files".into(), serde_json::json!(elsewhere));
    Ok(SharedStrings { schema: SHARED_STRINGS_SCHEMA.into(), source, keys })
}

/// A blueprint object's field by name.
/// `CharGen.MaleDoll` and `FemaleDoll` (each an index into `BlueprintReferencedAssets`, an entry
/// whose object is the `Character` of the doll's prefab, in this file): the `UnitEntityView`s on
/// that game object, as [`wr_import::prefabs::summarize`] reads a unit prefab's.
fn doll_prefabs(
    f: &SerializedFile<'_>,
    entries: &[Value],
    root: &wr_content::node::Object,
    file: &str,
) -> Result<serde_json::Value> {
    let Some(Node::Object(chargen)) = field_node(root, "CharGen") else {
        bail!("BlueprintRoot.CharGen")
    };
    let views = wr_import::prefabs::views_in(f)?;
    let mut out = serde_json::Map::new();
    out.insert("schema".into(), wr_sim::scene_spawn::DOLL_PREFABS_SCHEMA.into());
    out.insert("source".into(), format!("wr-rules-data {file}").into());
    for (key, field) in [("male", "MaleDoll"), ("female", "FemaleDoll")] {
        let Some(Node::AssetIndex(ix)) = field_node(&chargen, field) else {
            bail!("CharGen.{field} isn't an asset index")
        };
        let entry = entries
            .get(usize::try_from(ix).with_context(|| format!("CharGen.{field}"))?)
            .with_context(|| format!("CharGen.{field}: index {ix} out of range"))?;
        let p = entry.get("Asset").and_then(Value::as_pptr).context("doll entry Asset")?;
        ensure!(p.file_id == 0, "CharGen.{field}: the prefab is in another file");
        let o = f.object(p.path_id).with_context(|| format!("CharGen.{field}: object"))?;
        // (The `Character` component, or the game object itself.)
        let go = if o.class_id == class_id::GAME_OBJECT {
            p.path_id
        } else {
            f.read(o)?.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |g| g.path_id)
        };
        let found: Vec<wr_import::prefabs::ViewFound> =
            views.iter().filter(|(g, _)| *g == go).map(|(_, v)| v.clone()).collect();
        ensure!(
            found.len() == 1,
            "CharGen.{field}: {} UnitEntityViews on its game object",
            found.len()
        );
        out.insert(key.into(), serde_json::to_value(wr_import::prefabs::summarize(&found))?);
    }
    Ok(out.into())
}

fn field_node(o: &wr_content::node::Object, name: &str) -> Option<Node> {
    let t = wr_content::schema::ty(o.ty);
    t.fields.iter().position(|f| f.name == name).map(|i| o.fields[i].clone())
}

/// `UISettingsRoot.Settings`' entities the combat log reads (`DamageToParty`,
/// `EnemyCriticalHits`), each followed to its object, as the container `SettingsRoot`
/// (`{"Settings": {name: entity}}`). Skipped when the root or an entity is in another file.
fn settings_ui(
    f: &SerializedFile<'_>,
    entries: &[Value],
    ix: i32,
    log: &mut LogStrings,
) -> Result<()> {
    let Some(p) = entries
        .get(usize::try_from(ix)?)
        .and_then(|e| e.get("Asset"))
        .and_then(Value::as_pptr)
        .filter(|p| p.file_id == 0 && p.path_id != 0)
    else {
        eprintln!("log strings: BlueprintRoot.SettingsRoot isn't in this file: skipped");
        return Ok(());
    };
    let root = f.read(f.object(p.path_id).context("SettingsRoot object")?)?;
    let settings = root.get("Settings").context("UISettingsRoot.Settings")?;
    let mut out = serde_json::Map::new();
    for name in ["DamageToParty", "EnemyCriticalHits"] {
        let Some(q) = settings
            .get(name)
            .and_then(Value::as_pptr)
            .filter(|q| q.file_id == 0 && q.path_id != 0)
        else {
            eprintln!("log strings: UISettingsRoot.Settings.{name} isn't in this file: skipped");
            continue;
        };
        let v = f.read(f.object(q.path_id).with_context(|| format!("Settings.{name}"))?)?;
        out.insert(name.to_owned(), container_json(f, &v)?);
    }
    log.containers.insert(
        "SettingsRoot".into(),
        serde_json::json!({ "Settings": serde_json::Value::Object(out) }),
    );
    log.source.insert("settings_root_asset_index".into(), serde_json::json!(ix));
    Ok(())
}

/// `BlueprintRoot.LocalizedTexts`' fields that are single Unity objects (the string
/// containers), with their `BlueprintReferencedAssets` index.
fn localized_texts_containers(root: &wr_content::node::Object) -> Result<Vec<(String, i32)>> {
    let field = field_node;
    let Some(Node::Object(lt)) = field(root, "LocalizedTexts") else {
        bail!("BlueprintRoot.LocalizedTexts missing")
    };
    let t = wr_content::schema::ty(lt.ty);
    let mut out = Vec::new();
    for (i, f) in t.fields.iter().enumerate() {
        if let Node::AssetIndex(ix) = &lt.fields[i]
            && *ix >= 0
        {
            out.push((f.name.to_owned(), *ix));
        }
    }
    ensure!(out.iter().any(|(n, _)| n == "GameLog"), "LocalizedTexts.GameLog isn't an asset index");
    Ok(out)
}

/// A `LocalizedString`'s actual key: `m_Key`, or the end of its `Shared` chain (at most 50 steps).
fn actual_key(f: &SerializedFile<'_>, s: &Value) -> Result<String> {
    let mut s = s.clone();
    let mut steps = 0;
    loop {
        let next = s.get("Shared").and_then(Value::as_pptr).filter(|q| q.path_id != 0);
        let Some(q) = next.filter(|_| steps < 50) else { break };
        ensure!(q.file_id == 0, "a shared string in another file");
        let o = f.object(q.path_id).context("shared asset")?;
        s = f.read(o)?.get("String").context("SharedStringAsset.String")?.clone();
        steps += 1;
    }
    Ok(s.str_at("m_Key").unwrap_or_default().to_owned())
}

/// A decoded Unity value as JSON, each `LocalizedString` (an object with `m_Key` and `Shared`)
/// reduced to `{"$key": actual key}`, references as `{"$pptr": [file, path]}`.
fn container_json(f: &SerializedFile<'_>, v: &Value) -> Result<serde_json::Value> {
    use serde_json::{Value as J, json};
    Ok(match v {
        Value::Bool(b) => J::Bool(*b),
        Value::Int(i) => json!(i),
        Value::UInt(u) => json!(u),
        Value::Float(x) => json!(x),
        Value::Double(x) => json!(x),
        Value::Str(s) => J::String(s.clone()),
        Value::Bytes(_) => J::Null,
        Value::Array(a) => J::Array(a.iter().map(|x| container_json(f, x)).collect::<Result<_>>()?),
        Value::Object(_) if v.get("m_Key").is_some() && v.get("Shared").is_some() => {
            json!({"$key": actual_key(f, v)?})
        }
        Value::Object(fields) => {
            if let Some(p) = v.as_pptr()
                && fields.len() == 2
            {
                return Ok(json!({"$pptr": [p.file_id, p.path_id]}));
            }
            let mut m = serde_json::Map::new();
            for (k, x) in fields {
                if matches!(k.as_str(), "m_GameObject" | "m_Enabled" | "m_Script") {
                    continue;
                }
                m.insert(k.clone(), container_json(f, x)?);
            }
            J::Object(m)
        }
    })
}

/// Every `LocalizedTexts` container in this file, as JSON.
fn log_strings(
    f: &SerializedFile<'_>,
    entries: &[Value],
    containers: &[(String, i32)],
    file: &str,
) -> Result<LogStrings> {
    let mut out = BTreeMap::new();
    for (name, ix) in containers {
        let entry = entries.get(usize::try_from(*ix)?).with_context(|| format!("{name}: index"))?;
        let p = entry
            .get("Asset")
            .and_then(Value::as_pptr)
            .with_context(|| format!("{name}: Asset"))?;
        if p.file_id != 0 || p.path_id == 0 {
            continue;
        }
        let o = f.object(p.path_id).with_context(|| format!("{name}: object"))?;
        let v = f.read(o)?;
        out.insert(name.clone(), container_json(f, &v)?);
    }
    ensure!(out.contains_key("GameLog"), "GameLogStrings isn't in {file}");
    let mut source = BTreeMap::new();
    source.insert("bundle".into(), serde_json::json!("Bundles/blueprint.assets"));
    source.insert("serialized_file".into(), serde_json::json!(file));
    source.insert(
        "asset_indices".into(),
        serde_json::json!(containers.iter().cloned().collect::<BTreeMap<_, _>>()),
    );
    Ok(LogStrings { schema: LOG_STRINGS_SCHEMA.into(), source, containers: out })
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
        preset_asset_indices: BTreeMap::new(),
    })
}

/// An `AreaPartBounds` (its `MonoScript` may be in another file, so also by shape: the
/// default bounds with the mechanic and local-map overrides).
fn is_area_part_bounds(v: &Value, script: &dyn Fn(&Value) -> Option<String>) -> bool {
    if let Some(s) = script(v) {
        return s == "AreaPartBounds";
    }
    [
        "m_DefaultBounds",
        "m_OverrideMechanicBounds",
        "m_MechanicBounds",
        "m_OverrideLocalMapBounds",
        "m_LocalMapBounds",
        "m_OverrideFogOfWarBounds",
    ]
    .iter()
    .all(|k| v.get(k).is_some())
}

/// A serialized `Bounds` (`m_Center`, `m_Extent`).
fn bounds(v: &Value, k: &str) -> Result<Bounds> {
    let b = v.get(k).with_context(|| format!("field {k}"))?;
    Ok(Bounds {
        center: b.get("m_Center").and_then(Value::vec3).with_context(|| format!("{k}.m_Center"))?,
        extents: b
            .get("m_Extent")
            .and_then(Value::vec3)
            .with_context(|| format!("{k}.m_Extent"))?,
    })
}

/// Every `BlueprintReferencedAssets` entry that is an `AreaPartBounds` in this file (a
/// `BlueprintAreaPart.m_Bounds` in the runtime pack is such an index), with the bounds the
/// mechanics read; entries in other files are counted.
fn area_bounds(
    f: &SerializedFile<'_>,
    entries: &[Value],
    file: &str,
    script: &dyn Fn(&Value) -> Option<String>,
) -> Result<AreaBoundsTable> {
    let mut out = BTreeMap::new();
    let mut elsewhere = 0u64;
    for (i, e) in entries.iter().enumerate() {
        let Some(p) = e.get("Asset").and_then(Value::as_pptr) else { continue };
        if p.path_id == 0 {
            continue;
        }
        if p.file_id != 0 {
            elsewhere += 1;
            continue;
        }
        let Some(o) = f.object(p.path_id) else { continue };
        if o.class_id != class_id::MONO_BEHAVIOUR {
            continue;
        }
        let v = f.read(o)?;
        if !is_area_part_bounds(&v, script) {
            continue;
        }
        let ctx = || format!("entry {i}: AreaPartBounds");
        out.insert(
            i32::try_from(i)?,
            AreaPartBoundsAsset {
                name: v.str_at("m_Name").unwrap_or_default().to_owned(),
                path_id: p.path_id,
                default_bounds: bounds(&v, "m_DefaultBounds").with_context(ctx)?,
                override_mechanic_bounds: boolf(&v, "m_OverrideMechanicBounds")
                    .with_context(ctx)?,
                mechanic_bounds: bounds(&v, "m_MechanicBounds").with_context(ctx)?,
                override_local_map_bounds: boolf(&v, "m_OverrideLocalMapBounds")
                    .with_context(ctx)?,
                local_map_bounds: bounds(&v, "m_LocalMapBounds").with_context(ctx)?,
                fog_of_war: Some(FogOfWarBoundsAsset {
                    override_bounds: boolf(&v, "m_OverrideFogOfWarBounds").with_context(ctx)?,
                    bounds: bounds(&v, "m_FogOfWarBounds").with_context(ctx)?,
                }),
            },
        );
    }
    let mut source = BTreeMap::new();
    source.insert("bundle".into(), serde_json::json!("Bundles/blueprint.assets"));
    source.insert("serialized_file".into(), serde_json::json!(file));
    source.insert("entries".into(), serde_json::json!(entries.len()));
    source.insert("entries_in_other_files".into(), serde_json::json!(elsewhere));
    Ok(AreaBoundsTable { schema: AREA_BOUNDS_SCHEMA.into(), source, bounds: out })
}
