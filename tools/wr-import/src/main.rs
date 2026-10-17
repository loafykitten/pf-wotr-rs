//! Command-line entry point for the importer.
//!
//! `wr-import import --area prologue_caves_1` reads the area's bundles from the install
//! (read-only) and writes `inventory.json`, `nav.json`, `report.json` and, given an
//! AssetRipper export, `assetripper_check.json` under `cache/import/<area>/`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use serde::Serialize;
use wr_import::area::Universe;
use wr_import::blueprints::{BlueprintIndex, BlueprintReader};
use wr_import::catalog::{CabIndex, Dependencies, Locations};
use wr_import::nav::{graph, path};
use wr_import::report::{self, DynamicDep, YieldReport};
use wr_import::scene::{self, Inventory, Resolver, SceneData};
use wr_import::unity::serialized::class_id;
use wr_import::unity::value::Value;
use wr_paths::GamePaths;

#[derive(Parser, Debug)]
#[command(about = "Offline importer for Wrath's Unity bundles (POC-0)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Imports an area: inventory, navigation graph and yield report.
    Import {
        #[arg(long, default_value = "prologue_caves_1")]
        area: String,
        /// AssetRipper export directory to cross-check against.
        #[arg(long)]
        assetripper: Option<PathBuf>,
        /// TSV of PNGs recorded and deleted during the export (size, path).
        #[arg(long)]
        pruned: Option<PathBuf>,
        /// Skip the dependency-closure census (faster).
        #[arg(long)]
        no_census: bool,
    },
    /// Writes the area's nav file for the sim, `cache/sim/nav/<guid>.wrnav`: the runtime
    /// graphs built from the area's `.nav` bundle, in wr-nav's format.
    NavPack {
        /// The area's bundle name, lower case (e.g. `prologue_caves_1`).
        #[arg(long)]
        area: String,
        /// The area blueprint's guid.
        #[arg(long)]
        guid: String,
    },
    /// Writes the area's physics file for the sim, `cache/sim/phys/<guid>.wrphys`: the
    /// colliders of the area's scenes in wr-phys's format.
    PhysPack {
        /// The area's bundle name, lower case (e.g. `prologue_caves_1`).
        #[arg(long)]
        area: String,
        /// The area blueprint's guid.
        #[arg(long)]
        guid: String,
        /// A recording of the area with observer `phys_probe` (`*.trace.jsonl.gz`): its
        /// `ph_col` records give the mesh and box colliders the transforms the game holds,
        /// without which a query that may touch one fails closed.
        #[arg(long)]
        colliders: Option<PathBuf>,
    },
    /// Writes the area's line-of-sight file for the sim, `cache/sim/los/<guid>.wrlos`: the
    /// scenes' `FogOfWarBlocker`s in wr-los's format.
    LosPack {
        /// The area's bundle name, lower case (e.g. `prologue_caves_1`).
        #[arg(long)]
        area: String,
        /// The area blueprint's guid.
        #[arg(long)]
        guid: String,
    },
    /// Runs a path query on an area's graph cache.
    Path {
        #[arg(long, default_value = "prologue_caves_1")]
        area: String,
        #[arg(long, value_delimiter = ',', num_args = 3)]
        from: Vec<f32>,
        #[arg(long, value_delimiter = ',', num_args = 3)]
        to: Vec<f32>,
    },
    /// Prints MonoBehaviours whose script class name matches, as JSON.
    Dump {
        #[arg(long)]
        class: String,
        #[arg(long, default_value_t = 2)]
        limit: usize,
        bundles: Vec<String>,
    },
    /// Decodes every `.scenes` bundle and counts MonoBehaviour script classes.
    SceneCensus,
    /// Looks up which bundle holds each named serialized file (`CAB-…`).
    Cab { cabs: Vec<String> },
    /// Diffs a WrathOracle capture against our own decoding of the area, field by field;
    /// writes `oracle_diff.json` under `cache/import/<area>/`.
    Diff {
        /// Capture directory (scene-dump.json, hierarchy/, nav-graph.json, nav-queries.json).
        #[arg(long, default_value = "fixtures/captures/poc0/check-1")]
        capture: PathBuf,
        #[arg(long, default_value = "prologue_caves_1")]
        area: String,
        /// Only these sections (comma-separated: scene, hierarchy, nav_graph, nav_queries).
        #[arg(long)]
        only: Option<String>,
        /// Run the navigation sections on the world read from this nav file.
        #[arg(long)]
        nav_file: Option<PathBuf>,
    },
    /// Writes `cache/sim/unit-prefabs.json`: each unit prefab's `UnitEntityView.m_IsInGame`
    /// (the `IsInGameBySettings` a unit built from it starts with), for the sim.
    UnitPrefabs,
    /// Prints a pack's metadata as JSON (blobs as offsets).
    PackInfo { pack: PathBuf },
    /// Builds an area's runtime asset pack (meshes, BC textures, materials, Owlcat
    /// shader variants as metallibs, lights, lightmaps) for the Bevy renderer.
    Pack {
        #[arg(long, default_value = "prologue_caves_1")]
        area: String,
        /// Output file (default `cache/poc6/packs/<area>.wrpack`).
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

struct Context0 {
    paths: GamePaths,
    deps: Dependencies,
    locations: Locations,
    cabs: CabIndex,
}

fn context() -> Result<Context0> {
    let paths = GamePaths::from_env();
    let bundles = paths.bundles();
    let cabs =
        CabIndex::load_or_build(&bundles, &wr_paths::cache_root().join("import/cab_index.json"))?;
    Ok(Context0 {
        deps: Dependencies::load(&bundles)?,
        locations: Locations::load(&bundles)?,
        cabs,
        paths,
    })
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Import { area, assetripper, pruned, no_census } => {
            import(&area, assetripper.as_deref(), pruned.as_deref(), no_census)
        }
        Cmd::NavPack { area, guid } => {
            let t = Instant::now();
            let ours = wr_import::oracle_diff::load_ours(&area)?;
            let file =
                wr_nav::NavFile { area: guid.clone(), source: ours.nav_source, world: ours.world };
            let path = wr_nav::path_for(&guid);
            file.write(&path)?;
            let back = wr_nav::NavFile::read(&path)?;
            anyhow::ensure!(
                postcard_eq(&back, &file),
                "{} does not read back as written",
                path.display()
            );
            eprintln!(
                "wrote {} ({} triangles, {} grid cells, {} bytes) in {:?}",
                path.display(),
                file.world.tris.len(),
                file.world.grid.as_ref().map_or(0, |g| g.positions.len()),
                std::fs::metadata(&path)?.len(),
                t.elapsed()
            );
            Ok(())
        }
        Cmd::PhysPack { area, guid, colliders } => {
            let t = Instant::now();
            let a = wr_import::oracle_diff::load_area_scenes(&area)?;
            let (mut world, mut rep) = wr_import::phys::build(&a.universe, &a.scenes)?;
            if let Some(trace) = &colliders {
                // The recording's name under the captures folder, for reports.
                let given = trace.to_string_lossy();
                let source = given.rsplit_once("captures/oracle/").map_or_else(
                    || trace.file_name().map_or_else(String::new, |n| n.to_string_lossy().into()),
                    |(_, rest)| rest.to_owned(),
                );
                let c = wr_import::phys::apply_recorded_colliders(&mut world, trace, &source)?;
                eprintln!(
                    "recorded transforms from {source}: {} meshes (the importer's own equal: position {}, rotation {}, lossy scale {}); left out, recorded disabled: {:?}; zero scale, left unrecorded: {:?}",
                    c.meshes,
                    c.position_equal,
                    c.rotation_equal,
                    c.lossy_equal,
                    c.disabled,
                    c.degenerate
                );
                eprintln!(
                    "  {} boxes; left out, recorded disabled: {:?}; the mapping not established, left unrecorded: {:#?}",
                    c.boxes, c.boxes_disabled, c.boxes_unestablished
                );
                rep.shapes = world.shapes.len();
                if let Some(n) = rep.by_kind.get_mut("mesh") {
                    *n -= c.disabled.len();
                }
                if let Some(n) = rep.by_kind.get_mut("box") {
                    *n -= c.boxes_disabled.len();
                }
            }
            let mut scenes: Vec<String> = a.scenes.iter().map(|s| s.bundle.clone()).collect();
            scenes.dedup();
            let file = wr_phys::PhysFile { area: guid.clone(), scenes, world };
            let path = wr_phys::path_for(&guid);
            file.write(&path)?;
            let back = wr_phys::PhysFile::read(&path)?;
            anyhow::ensure!(back == file, "{} does not read back as written", path.display());
            eprintln!(
                "wrote {} ({} shapes {:?}, {} inactive left out, {} bytes) in {:?}",
                path.display(),
                rep.shapes,
                rep.by_kind,
                rep.skipped_inactive,
                std::fs::metadata(&path)?.len(),
                t.elapsed()
            );
            for m in &rep.meshes {
                eprintln!(
                    "  cooked {} (format {}, midphase {}): {} vertices ({} also render vertices; render {}), {} triangles (render {})",
                    m.mesh,
                    m.version,
                    m.midphase,
                    m.cooked_vertices,
                    m.cooked_in_render,
                    m.render_vertices,
                    m.cooked_triangles,
                    m.render_triangles
                );
            }
            Ok(())
        }
        Cmd::LosPack { area, guid } => {
            let t = Instant::now();
            let a = wr_import::oracle_diff::load_area_scenes(&area)?;
            // The player's own files have no type trees (a blocker's `MeshFilter` may point at the
            // built-in cube): borrow the Mesh tree from the scenes (same Unity version).
            let donors: Vec<&_> = a.scenes.iter().map(|s| s.file).collect();
            let paths = wr_paths::GamePaths::from_env();
            a.universe.add_loose_file_with_donors(
                &paths.app.join("Contents/Resources/unity default resources"),
                &donors,
            )?;
            let (file, rep) = wr_import::los::build(&a.universe, &a.scenes, &guid)?;
            let path = wr_los::path_for(&guid);
            file.write(&path)?;
            let back = wr_los::LosFile::read(&path)?;
            anyhow::ensure!(back == file, "{} does not read back as written", path.display());
            eprintln!(
                "wrote {} ({} blockers: {} polygons, {} meshes; {} inactive left out, {} updatable; {} fog of war areas; {} extended vision spawners; {} transformed points whose f32 and double sums differ; {} bytes) in {:?}",
                path.display(),
                rep.blockers,
                rep.polygons,
                rep.meshes,
                rep.skipped_inactive,
                rep.updatable,
                rep.areas,
                file.extended_vision_spawners,
                rep.ambiguous_points,
                std::fs::metadata(&path)?.len(),
                t.elapsed()
            );
            for l in &rep.list {
                eprintln!("  {l}");
            }
            Ok(())
        }
        Cmd::Path { area, from, to } => {
            let ctx = context()?;
            let cache = load_nav(&ctx, &area)?;
            let (a, b) = ([from[0], from[1], from[2]], [to[0], to[1], to[2]]);
            let out = serde_json::json!({
                "recast": path::navmesh_path(&cache, a, b),
                "grid": path::grid_path(&cache, a, b),
            });
            println!("{}", serde_json::to_string_pretty(&out)?);
            Ok(())
        }
        Cmd::Dump { class, limit, bundles } => dump(&bundles, &class, limit),
        Cmd::UnitPrefabs => {
            let t = Instant::now();
            let ctx = context()?;
            let t1 = wr_import::prefabs::unit_prefabs(&ctx.paths.bundles(), &ctx.locations);
            let known = t1.prefabs.values().filter(|v| v.is_some()).count();
            let hidden = t1.prefabs.values().filter(|v| **v == Some(false)).count();
            eprintln!(
                "unit prefabs: {} ({known} with one view, {hidden} not in game by settings, {} errors) in {:?}",
                t1.prefabs.len(),
                t1.errors.len(),
                t.elapsed()
            );
            write_json(&wr_paths::cache_root().join("sim"), "unit-prefabs.json", &t1)
        }
        Cmd::SceneCensus => {
            let t = Instant::now();
            let paths = GamePaths::from_env();
            let c = report::scene_census(&paths.bundles())?;
            eprintln!("scene census: {} bundles in {:?}", c.bundles, t.elapsed());
            write_json(&wr_paths::cache_root().join("import"), "scene_census.json", &c)
        }
        Cmd::PackInfo { pack } => {
            // SAFETY: a pack written by `PackWriter` (new file, renamed into place); this
            // process only reads it.
            let p = unsafe { wr_assets::Pack::open(&pack) }?;
            println!("{}", serde_json::to_string_pretty(&p.meta)?);
            Ok(())
        }
        Cmd::Pack { area, out } => {
            let ctx = context()?;
            let universe = Universe::new(&ctx.paths.bundles(), ctx.cabs.clone());
            let cache = wr_paths::cache_root().join("poc6");
            let out = out.unwrap_or_else(|| cache.join("packs").join(format!("{area}.wrpack")));
            let rep = wr_import::pack::build(
                &ctx.paths.data(),
                &universe,
                &ctx.deps,
                &area,
                &out,
                &cache.join("metallib"),
            )?;
            eprintln!(
                "pack: {} renderers, {} meshes, {} materials, {} textures, {} programs ({} metallibs), {} lights; {:.1} MiB in {} ms",
                rep.renderers,
                rep.meshes,
                rep.materials,
                rep.textures,
                rep.programs,
                rep.metallibs,
                rep.lights,
                rep.pack_bytes as f64 / 1048576.0,
                rep.timings_ms.get("total").copied().unwrap_or(0)
            );
            let report_dir = out.parent().map_or_else(|| cache.join("packs"), Path::to_path_buf);
            write_json(&report_dir, &format!("{area}.report.json"), &rep)
        }
        Cmd::Cab { cabs } => {
            let ctx = context()?;
            for c in cabs {
                println!("{c} -> {:?}", ctx.cabs.bundle_of(&c));
            }
            Ok(())
        }
        Cmd::Diff { capture, area, only, nav_file } => {
            let out = wr_paths::cache_root().join("import").join(&area);
            let r = wr_import::oracle_diff::run(
                &capture,
                &area,
                &out,
                only.as_deref(),
                nav_file.as_deref(),
            )?;
            print!("{}", wr_import::oracle_diff::summary_text(&r));
            Ok(())
        }
    }
}

/// The area's root bundles: its `.scenes` bundles and its `.nav`, from the dependency
/// list's keys.
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

fn load_nav(ctx: &Context0, area: &str) -> Result<graph::GraphCache> {
    let universe = Universe::new(&ctx.paths.bundles(), ctx.cabs.clone());
    let (_, nav) = area_roots(&ctx.deps, area);
    for f in universe.files_of(&nav)? {
        if let Some(o) = f.objects.iter().find(|o| o.class_id == class_id::TEXT_ASSET) {
            let v = f.read(o)?;
            let bytes = v
                .get("m_Script")
                .and_then(Value::as_bytes)
                .context("TextAsset without m_Script")?;
            return graph::decode(bytes);
        }
    }
    bail!("no TextAsset in {nav}")
}

fn write_json<T: Serialize>(dir: &Path, name: &str, v: &T) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let p = dir.join(name);
    std::fs::write(&p, serde_json::to_vec_pretty(v)?)
        .with_context(|| format!("write {}", p.display()))?;
    eprintln!("wrote {}", p.display());
    Ok(())
}

/// A deterministic set of path queries between authored points, for comparison with the
/// live game.
fn query_set(inv: &Inventory) -> Vec<(String, [f32; 3], [f32; 3])> {
    let mut starts: Vec<(&str, [f32; 3])> = Vec::new();
    for o in &inv.objects {
        if o.components.iter().any(|c| c.class == "Kingmaker.View.AreaEnterPoint") {
            starts.push((o.name.as_str(), o.world.position));
        }
    }
    let mut ends: Vec<(String, [f32; 3])> = inv
        .spawners
        .iter()
        .filter(|s| s.active_in_hierarchy)
        .map(|s| (format!("spawner {}", s.unique_id), s.position))
        .collect();
    ends.extend(inv.zones.iter().filter_map(|z| {
        let c = z.shapes.first()?.world_center?;
        Some((format!("zone {}", z.unique_id.clone().unwrap_or_default()), c))
    }));
    let mut out = Vec::new();
    for (sn, sp) in &starts {
        for (en, ep) in &ends {
            out.push((format!("{sn} -> {en}"), *sp, *ep));
        }
    }
    out
}

/// Keeps up to `reachable` found queries and `unreachable` unfound ones, in order.
fn pick_queries(all: Vec<QueryOut>, reachable: usize, unreachable: usize) -> Vec<QueryOut> {
    let (mut r, mut u) = (0, 0);
    all.into_iter()
        .filter(|q| {
            let found = q.recast.as_ref().is_some_and(|p| p.found);
            if found && r < reachable {
                r += 1;
                true
            } else if !found && u < unreachable {
                u += 1;
                true
            } else {
                false
            }
        })
        .collect()
}

#[derive(Serialize)]
struct NavOut {
    version: String,
    entries: Vec<(String, u64)>,
    max_node_index: i32,
    node_link2_count: i32,
    graphs: Vec<graph::GraphSummary>,
    settings: Vec<serde_json::Value>,
    queries: Vec<QueryOut>,
}

#[derive(Serialize)]
struct QueryOut {
    label: String,
    start: [f32; 3],
    end: [f32; 3],
    recast: Option<path::PathResult>,
    grid: Option<path::PathResult>,
}

fn import(area: &str, ar: Option<&Path>, pruned: Option<&Path>, no_census: bool) -> Result<()> {
    let t_all = Instant::now();
    let mut timings = BTreeMap::new();
    let mut t = Instant::now();
    let ctx = context()?;
    let blueprints = BlueprintIndex::load_or_build(
        &ctx.paths.blueprints_zip(),
        &wr_paths::cache_root().join("import/blueprint_index.json"),
    )?;
    timings.insert("indexes".to_owned(), t.elapsed().as_millis());
    let out_dir = wr_paths::cache_root().join("import").join(area);
    let (scene_bundles, nav_bundle) = area_roots(&ctx.deps, area);
    if scene_bundles.is_empty() {
        bail!("no scene bundles for area {area}");
    }
    let mut roots = scene_bundles.clone();
    roots.push(nav_bundle.clone());
    let closure = ctx.deps.closure(&roots);

    // Scenes.
    t = Instant::now();
    let universe = Universe::new(&ctx.paths.bundles(), ctx.cabs.clone());
    universe.add_loose_file(&ctx.paths.app.join("Contents/Resources/unity default resources"))?;
    universe.add_loose_file(&ctx.paths.data().join("Resources/unity_builtin_extra"))?;
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
    timings.insert("scenes_decode".to_owned(), t.elapsed().as_millis());

    t = Instant::now();
    let mut resolver = Resolver {
        universe: &universe,
        blueprints: &blueprints,
        locations: &ctx.locations,
        entity_ids: Default::default(),
    };
    let inventory = scene::build_inventory(area, &scenes, &mut resolver)?;
    timings.insert("inventory".to_owned(), t.elapsed().as_millis());
    write_json(&out_dir, "inventory.json", &inventory)?;

    // Navigation.
    t = Instant::now();
    let cache = load_nav(&ctx, area)?;
    timings.insert("nav_decode".to_owned(), t.elapsed().as_millis());
    t = Instant::now();
    let queries = query_set(&inventory)
        .into_iter()
        .map(|(label, a, b)| QueryOut {
            label,
            start: a,
            end: b,
            recast: path::navmesh_path(&cache, a, b),
            grid: path::grid_path(&cache, a, b),
        })
        .collect();
    let queries = pick_queries(queries, 24, 6);
    timings.insert("nav_queries".to_owned(), t.elapsed().as_millis());
    let settings = cache
        .graphs
        .iter()
        .map(|g| match g {
            graph::Graph::Recast(r) => r.settings.clone(),
            graph::Graph::Grid(gg) => gg.settings.clone(),
            graph::Graph::Unsupported { type_name } => {
                serde_json::json!({ "unsupported": type_name })
            }
        })
        .collect();
    let nav = NavOut {
        version: cache.version.clone(),
        entries: cache.entries.clone(),
        max_node_index: cache.max_node_index,
        node_link2_count: cache.node_link2_count,
        graphs: graph::summarize(&cache),
        settings,
        queries,
    };
    write_json(&out_dir, "nav.json", &nav)?;

    // Report.
    let mut rep = YieldReport { area: area.to_owned(), ..Default::default() };
    t = Instant::now();
    report::scene_stats(&mut rep, &scenes, &inventory, &resolver)?;
    timings.insert("scene_stats_and_refs".to_owned(), t.elapsed().as_millis());

    // Unit bundles the spawners need at runtime (loaded on spawn via locationlist, not
    // through the dependency list).
    let static_set: BTreeSet<&String> = closure.iter().collect();
    let mut reader = BlueprintReader::open(&ctx.paths.blueprints_zip())?;
    for s in &inventory.spawners {
        let prefab = match &s.blueprint_path {
            Some(p) => reader.json(p).ok().and_then(|j| {
                j.pointer("/Data/Prefab/AssetId").and_then(|v| v.as_str()).map(str::to_owned)
            }),
            None => None,
        };
        let bundle = prefab.as_ref().and_then(|a| ctx.locations.guid_to_bundle.get(a).cloned());
        rep.dynamic_unit_bundles.push(DynamicDep {
            spawner: s.unique_id.clone(),
            blueprint: s.blueprint.clone(),
            in_static_closure: bundle.as_ref().is_some_and(|b| static_set.contains(b)),
            prefab_asset_id: prefab,
            bundle,
        });
    }

    if !no_census {
        t = Instant::now();
        rep.closure = report::closure_census(&ctx.paths.bundles(), &closure, &universe)?;
        timings.insert("closure_census".to_owned(), t.elapsed().as_millis());
    }
    if let Some(dir) = ar {
        t = Instant::now();
        let check = wr_import::archeck::check(
            dir,
            pruned,
            &rep.closure.meshes,
            &scenes,
            &rep.closure.per_bundle,
        )?;
        timings.insert("assetripper_check".to_owned(), t.elapsed().as_millis());
        write_json(&out_dir, "assetripper_check.json", &check)?;
    }
    timings.insert("total".to_owned(), t_all.elapsed().as_millis());
    rep.timings_ms = timings;
    write_json(&out_dir, "report.json", &rep)?;
    Ok(())
}

fn dump(names: &[String], class: &str, limit: usize) -> Result<()> {
    let ctx = context()?;
    let universe = Universe::new(&ctx.paths.bundles(), ctx.cabs.clone());
    let mut n = 0;
    for b in names {
        for f in universe.files_of(b)? {
            for o in f.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
                let v = f.read(o)?;
                let cname = universe.script_of(f, &v)?.map(|s| s.class).unwrap_or_default();
                if cname == class && n < limit {
                    println!("{} {} {}", f.name, o.path_id, serde_json::to_string_pretty(&v)?);
                    n += 1;
                }
            }
        }
    }
    Ok(())
}

/// Two nav files encode to the same bytes.
fn postcard_eq(a: &wr_nav::NavFile, b: &wr_nav::NavFile) -> bool {
    matches!((postcard::to_allocvec(a), postcard::to_allocvec(b)), (Ok(x), Ok(y)) if x == y)
}
