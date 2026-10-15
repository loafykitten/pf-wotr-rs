//! `wr-import diff`: compares a WrathOracle capture (`scene-dump.json`,
//! `hierarchy/<scene>.json`, `nav-graph.json`, `nav-queries.json`) with what wr-import
//! computes from the install, field by field, and writes a JSON report.
//!
//! Everything on our side is recomputed in-process (scenes decoded, inventory built,
//! graphs decoded, queries run with the ported A*), so the diff reruns on any new dump.

pub mod hierarchy;
pub mod json;
pub mod nav;
pub mod scene;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use crate::area::Universe;
use crate::blueprints::BlueprintIndex;
use crate::catalog::{CabIndex, Dependencies, Locations};
use crate::nav::graph::{self, GraphCache};
use crate::scene::{Inventory, Resolver, SceneData};
use crate::unity::serialized::class_id;
use crate::unity::value::Value;
use wr_nav::world::World;

/// Counts for one compared field or record kind.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Tally {
    pub checked: usize,
    /// Bit-identical (or equal, for non-floats).
    pub exact: usize,
    /// Not bit-identical but close: within 1e-5, or at most 8 ULP apart (floats only).
    pub within_1e5: usize,
    pub mismatched: usize,
    /// Differences explained as runtime-only (listed, not counted as errors).
    pub expected: usize,
}

impl Tally {
    pub fn add(&mut self, o: Outcome) {
        self.checked += 1;
        match o {
            Outcome::Exact => self.exact += 1,
            Outcome::Close => self.within_1e5 += 1,
            Outcome::Mismatch => self.mismatched += 1,
            Outcome::Expected => self.expected += 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Exact,
    Close,
    Mismatch,
    Expected,
}

/// Field tallies plus example differences for one category.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Section {
    pub fields: BTreeMap<String, Tally>,
    /// Up to [`MAX_EXAMPLES`] differences per field.
    pub differences: BTreeMap<String, Vec<serde_json::Value>>,
    /// Close (not bit-identical) values, listed separately from mismatches.
    pub close: BTreeMap<String, Vec<serde_json::Value>>,
    /// Expected (runtime-only) differences, listed.
    pub expected: BTreeMap<String, Vec<serde_json::Value>>,
    pub notes: Vec<String>,
}

pub const MAX_EXAMPLES: usize = 400;

impl Section {
    pub fn tally(&mut self, field: &str, o: Outcome) {
        self.fields.entry(field.to_owned()).or_default().add(o);
    }

    /// Records an outcome, with an example when it isn't exact.
    pub fn record(&mut self, field: &str, o: Outcome, example: impl FnOnce() -> serde_json::Value) {
        self.tally(field, o);
        let list = match o {
            Outcome::Exact => return,
            Outcome::Expected => self.expected.entry(field.to_owned()).or_default(),
            Outcome::Close => self.close.entry(field.to_owned()).or_default(),
            Outcome::Mismatch => self.differences.entry(field.to_owned()).or_default(),
        };
        if list.len() < MAX_EXAMPLES {
            list.push(example());
        }
    }

    pub fn note(&mut self, s: impl Into<String>) {
        self.notes.push(s.into());
    }
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema: &'static str,
    pub capture: String,
    pub area: String,
    pub scene: Section,
    pub hierarchy: Section,
    pub nav_graph: Section,
    pub nav_queries: Section,
    pub summary: BTreeMap<String, BTreeMap<String, Tally>>,
}

/// Our side of the comparison, decoded from the install.
#[derive(Debug)]
pub struct Ours {
    pub scenes: Vec<SceneData>,
    pub inventory: Inventory,
    pub cache: GraphCache,
    pub world: World,
    /// The `.nav` bundle and its graph cache's length and checksum.
    pub nav_source: wr_nav::Source,
}

fn area_roots(deps: &Dependencies, area: &str) -> (Vec<String>, String) {
    let prefix = format!("{area}_");
    let mut scenes: Vec<String> = deps
        .map
        .keys()
        .filter(|k| k.starts_with(&prefix) && k.ends_with(".scenes"))
        .cloned()
        .collect();
    scenes.sort();
    (scenes, format!("{area}.nav"))
}

/// The `AstarPath` component's `maxNearestNodeDistance` and `fullGetNearestSearch`.
fn astar_settings(inv: &Inventory) -> (f32, bool) {
    for o in &inv.objects {
        for c in &o.components {
            if c.class.ends_with("AstarPath") {
                let max = c.fields.f32_at("maxNearestNodeDistance").unwrap_or(100.0);
                let full =
                    c.fields.get("fullGetNearestSearch").and_then(Value::as_bool).unwrap_or(false);
                return (max, full);
            }
        }
    }
    (100.0, false)
}

/// The area's scene files decoded, with the universe that resolves their references.
pub struct AreaScenes {
    pub universe: Universe,
    pub scenes: Vec<SceneData>,
    pub blueprints: BlueprintIndex,
    pub locations: Locations,
    pub deps: Dependencies,
}

impl std::fmt::Debug for AreaScenes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AreaScenes").field("scenes", &self.scenes.len()).finish_non_exhaustive()
    }
}

/// Decodes the area's scene bundles from the install (read-only).
pub fn load_area_scenes(area: &str) -> Result<AreaScenes> {
    let paths = wr_paths::GamePaths::from_env();
    let bundles = paths.bundles();
    let cabs =
        CabIndex::load_or_build(&bundles, &wr_paths::cache_root().join("import/cab_index.json"))?;
    let deps = Dependencies::load(&bundles)?;
    let locations = Locations::load(&bundles)?;
    let blueprints = BlueprintIndex::load_or_build(
        &paths.blueprints_zip(),
        &wr_paths::cache_root().join("import/blueprint_index.json"),
    )?;
    let universe = Universe::new(&bundles, cabs);
    universe.add_loose_file(&paths.app.join("Contents/Resources/unity default resources"))?;
    universe.add_loose_file(&paths.data().join("Resources/unity_builtin_extra"))?;
    let (scene_bundles, _) = area_roots(&deps, area);
    if scene_bundles.is_empty() {
        bail!("no scene bundles for area {area}");
    }
    let mut scenes = Vec::new();
    for b in &scene_bundles {
        let files = universe.files_of(b)?;
        let mut hashes = Vec::new();
        for f in &files {
            for o in f.objects.iter().filter(|o| o.class_id == class_id::ASSET_BUNDLE) {
                let v = f.read(o)?;
                for pair in v.get("m_SceneHashes").and_then(Value::as_array).unwrap_or_default() {
                    hashes.push((
                        pair.str_at("first").unwrap_or_default().to_owned(),
                        pair.str_at("second").unwrap_or_default().to_owned(),
                    ));
                }
            }
        }
        for (scene_name, cab) in hashes {
            let file = files
                .iter()
                .find(|f| f.name == cab)
                .with_context(|| format!("scene file {cab} not in {b}"))?;
            scenes.push(SceneData::load(&universe, b, &scene_name, file)?);
        }
    }
    Ok(AreaScenes { universe, scenes, blueprints, locations, deps })
}

/// Decodes the area's scenes and graph cache from the install (read-only).
pub fn load_ours(area: &str) -> Result<Ours> {
    let AreaScenes { universe, scenes, blueprints, locations, deps } = load_area_scenes(area)?;
    let (_, nav_bundle) = area_roots(&deps, area);
    let mut resolver = Resolver {
        universe: &universe,
        blueprints: &blueprints,
        locations: &locations,
        entity_ids: Default::default(),
    };
    let inventory = crate::scene::build_inventory(area, &scenes, &mut resolver)?;
    let mut cache = None;
    let mut nav_source = None;
    for f in universe.files_of(&nav_bundle)? {
        if let Some(o) = f.objects.iter().find(|o| o.class_id == class_id::TEXT_ASSET) {
            let v = f.read(o)?;
            let bytes = v
                .get("m_Script")
                .and_then(Value::as_bytes)
                .context("TextAsset without m_Script")?;
            cache = Some(graph::decode(bytes)?);
            nav_source = Some(wr_nav::Source {
                bundle: nav_bundle.clone(),
                cache_len: bytes.len() as u64,
                cache_fnv: wr_nav::fnv1a64(bytes),
            });
            break;
        }
    }
    let cache = cache.with_context(|| format!("no TextAsset in {nav_bundle}"))?;
    let (max, full) = astar_settings(&inventory);
    let world = crate::nav::build::build_world(&cache, max, full)?;
    let nav_source = nav_source.context("nav source")?;
    Ok(Ours { scenes, inventory, cache, world, nav_source })
}

fn summarize(s: &Section) -> BTreeMap<String, Tally> {
    s.fields.clone()
}

/// Runs the whole diff and writes `oracle_diff.json` under `out_dir`.
/// Diffs a capture against our decoding of `area`; with `nav_file`, the navigation
/// sections use the world read from that nav file (what the sim loads) instead.
pub fn run(
    capture: &Path,
    area: &str,
    out_dir: &Path,
    only: Option<&str>,
    nav_file: Option<&Path>,
) -> Result<Report> {
    let t = std::time::Instant::now();
    let mut ours = load_ours(area)?;
    eprintln!("decoded area in {:?}", t.elapsed());
    if let Some(p) = nav_file {
        let file = wr_nav::NavFile::read(p)?;
        ensure!(
            file.source == ours.nav_source,
            "{} was built from another graph cache ({:?}; the install has {:?})",
            p.display(),
            file.source,
            ours.nav_source
        );
        // The settings come from the scenes' `AstarPath`, not the graph cache.
        ensure!(
            file.world.max_nearest_node_distance.to_bits()
                == ours.world.max_nearest_node_distance.to_bits()
                && file.world.full_get_nearest_search == ours.world.full_get_nearest_search,
            "{} has other AstarPath settings than the install's scenes: rebuild it",
            p.display()
        );
        ours.world = file.world;
        eprintln!("navigation from {}", p.display());
    }
    let want = |k: &str| only.is_none_or(|o| o.split(',').any(|x| x == k));
    let mut report = Report {
        schema: "wr-import/oracle-diff/0",
        capture: capture.display().to_string(),
        area: area.to_owned(),
        scene: Section::default(),
        hierarchy: Section::default(),
        nav_graph: Section::default(),
        nav_queries: Section::default(),
        summary: BTreeMap::new(),
    };
    if want("scene") {
        let dump = json::load(&capture.join("scene-dump.json"))?;
        scene::diff(&dump, &ours, &mut report.scene);
    }
    if want("hierarchy") {
        hierarchy::diff(&capture.join("hierarchy"), &ours, &mut report.hierarchy)?;
    }
    if want("nav_graph") {
        let dump = json::load(&capture.join("nav-graph.json"))?;
        nav::diff_graph(&dump, &ours, &mut report.nav_graph);
    }
    if want("nav_queries") {
        let dump = json::load(&capture.join("nav-queries.json"))?;
        nav::diff_queries(&dump, &ours, &mut report.nav_queries);
    }
    for (k, s) in [
        ("scene", &report.scene),
        ("hierarchy", &report.hierarchy),
        ("nav_graph", &report.nav_graph),
        ("nav_queries", &report.nav_queries),
    ] {
        report.summary.insert(k.to_owned(), summarize(s));
    }
    std::fs::create_dir_all(out_dir)?;
    let p = out_dir.join("oracle_diff.json");
    std::fs::write(&p, serde_json::to_vec_pretty(&report)?)
        .with_context(|| format!("write {}", p.display()))?;
    eprintln!("wrote {}", p.display());
    Ok(report)
}

/// A concise text summary: per category, fields with anything other than exact.
#[must_use]
pub fn summary_text(r: &Report) -> String {
    let mut out = String::new();
    for (cat, fields) in &r.summary {
        let total: usize = fields.values().map(|t| t.checked).sum();
        let exact: usize = fields.values().map(|t| t.exact).sum();
        let mism: usize = fields.values().map(|t| t.mismatched).sum();
        let close: usize = fields.values().map(|t| t.within_1e5).sum();
        let exp: usize = fields.values().map(|t| t.expected).sum();
        out.push_str(&format!(
            "{cat}: {total} checks, {exact} exact, {close} within 1e-5, {mism} mismatched, {exp} expected runtime differences\n"
        ));
        for (f, t) in fields {
            if t.exact != t.checked {
                out.push_str(&format!(
                    "  {f}: {}/{} exact, {} close, {} mismatched, {} expected\n",
                    t.exact, t.checked, t.within_1e5, t.mismatched, t.expected
                ));
            }
        }
    }
    out
}
