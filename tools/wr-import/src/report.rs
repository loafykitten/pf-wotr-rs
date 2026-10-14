//! The import yield report: what the area's scenes contain, how much of it we read and
//! resolve, which shader and component families are unsupported, and a census of the
//! dependency closure's assets.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use anyhow::Result;
use rayon::prelude::*;
use serde::Serialize;

use crate::area::{Universe, is_stream};
use crate::families::Relevance;
use crate::refs::{self, RawRef};
use crate::scene::{Inventory, Resolver, SceneData};
use crate::texture::{self, TextureCheck};
use crate::unity::bundle::Bundle;
use crate::unity::serialized::{SerializedFile, class_id, class_name};
use crate::unity::value::Value;

/// One row of a ranked count table.
#[derive(Debug, Clone, Serialize)]
pub struct Count {
    pub name: String,
    pub count: usize,
}

fn ranked(m: BTreeMap<String, usize>) -> Vec<Count> {
    let mut v: Vec<Count> = m.into_iter().map(|(name, count)| Count { name, count }).collect();
    v.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    v
}

#[derive(Debug, Clone, Serialize)]
pub struct ComponentFamilyRow {
    pub class: String,
    pub assembly: String,
    pub family: &'static str,
    pub relevance: Relevance,
    pub count: usize,
    /// Instances whose GameObject is active in the hierarchy.
    pub active: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct RefStats {
    pub total: usize,
    pub resolved: usize,
    pub unresolved: usize,
    /// Target bundle family (suffix, or bundle name for singletons) → count.
    pub by_target: BTreeMap<String, usize>,
    pub unresolved_samples: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShaderRow {
    pub shader: String,
    pub materials: usize,
    /// Non-null entries of renderers' `m_Materials` arrays using this shader.
    pub renderer_slots: usize,
    /// Other references to its materials (skybox, terrain template, script fields).
    pub other_refs: usize,
    pub bundle: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct AssetReach {
    pub materials: usize,
    pub materials_unresolved: usize,
    pub shaders: Vec<ShaderRow>,
    pub meshes: usize,
    pub mesh_vertices: u64,
    /// Meshes from the player's built-in resources (no type trees; e.g. Cube, Quad).
    pub builtin_meshes: usize,
    pub textures: usize,
    pub textures_by_format: BTreeMap<String, usize>,
    pub textures_payload_ok: usize,
    pub texture_failures: Vec<TextureCheck>,
    pub lights_by_type: BTreeMap<String, usize>,
    pub lights_by_bake: BTreeMap<String, usize>,
    pub animator_controllers: usize,
    pub bundles_opened: usize,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ClosureCensus {
    pub bundles: usize,
    pub bytes: u64,
    pub by_family: BTreeMap<String, (usize, u64)>,
    pub objects_by_class: Vec<Count>,
    pub textures_by_format: BTreeMap<String, usize>,
    pub textures_payload_ok: usize,
    pub textures_checked: usize,
    pub texture_failures: Vec<TextureCheck>,
    pub read_failures: Vec<String>,
    pub objects_total: usize,
    /// Objects fully decoded through their type tree (all of them must be).
    pub objects_decoded: usize,
    /// Per-mesh fingerprints for the AssetRipper comparison: (bundle, name) → hashes.
    #[serde(skip)]
    pub meshes: Vec<MeshPrint>,
    /// Object counts per (bundle, class), for the AssetRipper comparison.
    #[serde(skip)]
    pub per_bundle: BTreeMap<(String, String), usize>,
}

#[derive(Debug, Clone)]
pub struct MeshPrint {
    pub bundle: String,
    pub name: String,
    pub vertex_count: u64,
    pub index_hash: u64,
    pub vertex_hash: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DynamicDep {
    pub spawner: String,
    pub blueprint: String,
    pub prefab_asset_id: Option<String>,
    pub bundle: Option<String>,
    pub in_static_closure: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct YieldReport {
    pub area: String,
    pub timings_ms: BTreeMap<String, u128>,
    pub closure: ClosureCensus,
    pub dynamic_unit_bundles: Vec<DynamicDep>,
    pub scenes: Vec<(String, usize, usize)>,
    pub objects_by_class: Vec<Count>,
    pub components: Vec<ComponentFamilyRow>,
    pub components_by_relevance: BTreeMap<String, (usize, usize)>,
    pub references: BTreeMap<String, RefStats>,
    pub persistent_calls: Vec<Count>,
    pub inventory: BTreeMap<String, usize>,
    pub duplicate_unique_ids: Vec<String>,
    pub assets: AssetReach,
    pub hierarchy_errors: usize,
}

/// FNV-1a 64: a stable fingerprint for byte comparisons.
#[must_use]
pub fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Family label for a bundle name: its suffix, or the name for suffix-less singletons.
#[must_use]
pub fn bundle_family(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((_, ext)) => format!(".{ext}"),
        None => name.to_owned(),
    }
}

/// Raw bytes of a mesh's vertex data: inline, or from its `.resS` stream.
fn mesh_vertex_bytes<'a>(v: &'a Value, streams: &HashMap<String, &'a [u8]>) -> Option<&'a [u8]> {
    if let Some(Value::Bytes(b)) = v.path("m_VertexData.m_DataSize")
        && !b.is_empty()
    {
        return Some(b);
    }
    let size = usize::try_from(v.i64_at("m_StreamData.size")?).ok()?;
    if size == 0 {
        return Some(&[]);
    }
    let off = match v.path("m_StreamData.offset")? {
        Value::UInt(u) => usize::try_from(*u).ok()?,
        Value::Int(i) => usize::try_from(*i).ok()?,
        _ => return None,
    };
    let name = v.str_at("m_StreamData.path")?.rsplit('/').next()?;
    streams.get(name)?.get(off..off + size)
}

/// Census of every bundle in the closure: object classes, full-decode check, texture
/// payload checks and mesh fingerprints. Bundles are processed in parallel and dropped
/// after use.
pub fn closure_census(
    bundles_dir: &Path,
    closure: &[String],
    universe: &Universe,
) -> Result<ClosureCensus> {
    struct Part {
        bytes: u64,
        classes: BTreeMap<String, usize>,
        formats: BTreeMap<String, usize>,
        tex_ok: usize,
        tex_checked: usize,
        tex_fail: Vec<TextureCheck>,
        failures: Vec<String>,
        total: usize,
        decoded: usize,
        meshes: Vec<MeshPrint>,
    }
    let _ = universe;
    let parts: Vec<Result<Part>> = closure
        .par_iter()
        .map(|name| {
            let path = bundles_dir.join(name);
            let bytes = std::fs::metadata(&path)?.len();
            let bundle = Bundle::open(&path)?;
            let mut streams = HashMap::new();
            let mut files = Vec::new();
            for n in &bundle.header.nodes {
                let d = bundle.node_data(n)?;
                if is_stream(n) {
                    streams.insert(n.path.clone(), d);
                } else {
                    files.push(SerializedFile::parse(&n.path, d)?);
                }
            }
            let mut p = Part {
                bytes,
                classes: BTreeMap::new(),
                formats: BTreeMap::new(),
                tex_ok: 0,
                tex_checked: 0,
                tex_fail: Vec::new(),
                failures: Vec::new(),
                total: 0,
                decoded: 0,
                meshes: Vec::new(),
            };
            for f in &files {
                for o in &f.objects {
                    p.total += 1;
                    let cname = class_name(o.class_id);
                    *p.classes.entry(cname.clone()).or_default() += 1;
                    let v = match f.read(o) {
                        Ok(v) => {
                            p.decoded += 1;
                            v
                        }
                        Err(e) => {
                            if p.failures.len() < 20 {
                                p.failures.push(format!("{name}: {e:#}"));
                            }
                            continue;
                        }
                    };
                    match o.class_id {
                        class_id::TEXTURE_2D | class_id::CUBEMAP | 187 => {
                            let c = texture_check_local(&cname, &v, &streams);
                            p.tex_checked += 1;
                            *p.formats.entry(c.format.clone()).or_default() += 1;
                            if c.ok {
                                p.tex_ok += 1;
                            } else if p.tex_fail.len() < 20 {
                                p.tex_fail.push(c);
                            }
                        }
                        class_id::MESH => {
                            let index = match v.get("m_IndexBuffer") {
                                Some(Value::Bytes(b)) => fnv1a(b),
                                _ => 0,
                            };
                            let vert = mesh_vertex_bytes(&v, &streams).map_or(0, fnv1a);
                            p.meshes.push(MeshPrint {
                                bundle: name.clone(),
                                name: v.str_at("m_Name").unwrap_or_default().to_owned(),
                                vertex_count: u64::try_from(
                                    v.i64_at("m_VertexData.m_VertexCount").unwrap_or(0),
                                )
                                .unwrap_or(0),
                                index_hash: index,
                                vertex_hash: vert,
                            });
                        }
                        _ => {}
                    }
                }
            }
            Ok(p)
        })
        .collect();
    let mut c = ClosureCensus { bundles: closure.len(), ..Default::default() };
    let mut classes = BTreeMap::new();
    for (name, part) in closure.iter().zip(parts) {
        let p = part?;
        c.bytes += p.bytes;
        let fam = c.by_family.entry(bundle_family(name)).or_default();
        fam.0 += 1;
        fam.1 += p.bytes;
        for (k, v) in p.classes {
            *classes.entry(k.clone()).or_default() += v;
            // Scene-embedded assets live in the scene bundles; AssetRipper writes those
            // under the bundle directory too.
            *c.per_bundle.entry((name.clone(), k)).or_default() += v;
        }
        for (k, v) in p.formats {
            *c.textures_by_format.entry(k).or_default() += v;
        }
        c.textures_payload_ok += p.tex_ok;
        c.textures_checked += p.tex_checked;
        c.texture_failures.extend(p.tex_fail);
        c.read_failures.extend(p.failures);
        c.objects_total += p.total;
        c.objects_decoded += p.decoded;
        c.meshes.extend(p.meshes);
    }
    c.objects_by_class = ranked(classes);
    c.texture_failures.truncate(50);
    Ok(c)
}

/// Texture check against a bundle's own streams (no universe needed).
fn texture_check_local(class: &str, v: &Value, streams: &HashMap<String, &[u8]>) -> TextureCheck {
    texture::check_with(class, v, |name| streams.get(name).map(|s| s.len() as u64))
}

/// Collects whole-scene statistics, resolving every reference and following materials
/// to shaders and textures.
pub fn scene_stats(
    report: &mut YieldReport,
    scenes: &[SceneData],
    inventory: &Inventory,
    resolver: &Resolver<'_>,
) -> Result<()> {
    let universe = resolver.universe;
    let mut objects: BTreeMap<String, usize> = BTreeMap::new();
    let mut rows: BTreeMap<String, ComponentFamilyRow> = BTreeMap::new();
    let mut by_rel: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut refstats: BTreeMap<String, RefStats> = BTreeMap::new();
    let mut calls: BTreeMap<String, usize> = BTreeMap::new();
    // Material → (renderer `m_Materials` slots, other references) pointing at it.
    let mut material_slots: HashMap<(String, i64), (usize, usize)> = HashMap::new();
    let mut meshes: HashSet<(String, i64)> = HashSet::new();
    let mut controllers: HashSet<(String, i64)> = HashSet::new();
    let mut lights_type = BTreeMap::new();
    let mut lights_bake = BTreeMap::new();
    for s in scenes {
        report.scenes.push((s.scene.clone(), s.file.objects.len(), s.game_object_count()));
        report.hierarchy_errors += s.hierarchy_errors.len();
        for o in &s.file.objects {
            *objects.entry(class_name(o.class_id)).or_default() += 1;
        }
        for (class, fam, v, active) in s.components() {
            let row = rows.entry(class.to_owned()).or_insert_with(|| ComponentFamilyRow {
                class: class.to_owned(),
                assembly: String::new(),
                family: fam.name,
                relevance: fam.relevance,
                count: 0,
                active: 0,
            });
            row.count += 1;
            if active {
                row.active += 1;
            }
            let rel = format!("{:?}", fam.relevance).to_lowercase();
            by_rel.entry(rel).or_default().0 += 1;
            for m in refs::persistent_calls(v) {
                *calls.entry(format!("{class}.{m}")).or_default() += 1;
            }
            if class == "Light" {
                let t = match v.i64_at("m_Type") {
                    Some(0) => "spot",
                    Some(1) => "directional",
                    Some(2) => "point",
                    Some(3) => "area",
                    _ => "other",
                };
                *lights_type.entry(t.to_owned()).or_default() += 1;
                let b = match v.i64_at("m_Lightmapping") {
                    Some(1) => "mixed",
                    Some(2) => "baked",
                    Some(4) => "realtime",
                    _ => "other",
                };
                *lights_bake.entry(b.to_owned()).or_default() += 1;
            }
            for r in refs::collect(v) {
                let rr = resolver.resolve(s.file, &r)?;
                let st = refstats.entry(rr.kind.to_owned()).or_default();
                st.total += 1;
                if rr.resolved {
                    st.resolved += 1;
                } else {
                    st.unresolved += 1;
                    if st.unresolved_samples.len() < 25 {
                        st.unresolved_samples
                            .push(format!("{class}.{} -> {} ({})", rr.field, rr.target, rr.detail));
                    }
                }
                if let RawRef::Object { pptr, .. } = &r
                    && let Some(t) = universe.resolve(s.file, *pptr)?
                {
                    *st.by_target.entry(bundle_family(t.bundle)).or_default() += 1;
                    let key = (t.file.name.clone(), t.info.path_id);
                    match t.info.class_id {
                        class_id::MATERIAL => {
                            let e = material_slots.entry(key).or_default();
                            if class.ends_with("Renderer") && r.field().starts_with("m_Materials[")
                            {
                                e.0 += 1;
                            } else {
                                e.1 += 1;
                            }
                        }
                        class_id::MESH => {
                            meshes.insert(key);
                        }
                        class_id::ANIMATOR_CONTROLLER => {
                            controllers.insert(key);
                        }
                        _ => {}
                    }
                }
            }
        }
        // Assembly names, from the inventory's records.
        for rec in inventory.objects.iter().filter(|r| r.scene == s.scene) {
            for c in &rec.components {
                if let Some(r) = rows.get_mut(&c.class)
                    && r.assembly.is_empty()
                {
                    r.assembly.clone_from(&c.assembly);
                }
            }
        }
    }
    report.objects_by_class = ranked(objects);
    let mut comps: Vec<ComponentFamilyRow> = rows.into_values().collect();
    comps.sort_by(|a, b| {
        a.relevance
            .cmp(&b.relevance)
            .then(b.count.cmp(&a.count))
            .then_with(|| a.class.cmp(&b.class))
    });
    report.components = comps;
    report.components_by_relevance = by_rel;
    report.references = refstats;
    report.persistent_calls = ranked(calls);

    // Materials → shaders and textures.
    let mut reach = AssetReach {
        meshes: meshes.len(),
        animator_controllers: controllers.len(),
        ..Default::default()
    };
    for (file, pid) in &meshes {
        if let Some(m) = universe.get(file, *pid)? {
            // Player built-ins (`unity default resources`) ship without type trees.
            match m.file.read_prefix(m.info, "m_VertexData") {
                Ok(v) => {
                    reach.mesh_vertices +=
                        u64::try_from(v.i64_at("m_VertexData.m_VertexCount").unwrap_or(0))
                            .unwrap_or(0);
                }
                Err(_) => reach.builtin_meshes += 1,
            }
        }
    }
    let mut shader_rows: BTreeMap<String, ShaderRow> = BTreeMap::new();
    let mut textures: BTreeSet<(String, i64)> = BTreeSet::new();
    for ((file, pid), (slots, other)) in &material_slots {
        let Some(m) = universe.get(file, *pid)? else { continue };
        reach.materials += 1;
        let mv = m.read()?;
        let shader = mv.get("m_Shader").and_then(Value::as_pptr);
        let (sname, sbundle) =
            match shader.map(|p| universe.resolve(m.file, p)).transpose()?.flatten() {
                Some(s) => {
                    let name = s.file.read_prefix(s.info, "m_ParsedForm").ok().map_or_else(
                        || format!("<no type tree: {}:{}>", s.file.name, s.info.path_id),
                        |sv| sv.str_at("m_ParsedForm.m_Name").unwrap_or("<unnamed>").to_owned(),
                    );
                    (name, s.bundle.to_owned())
                }
                None => {
                    reach.materials_unresolved += 1;
                    ("<unresolved shader>".to_owned(), String::new())
                }
            };
        let row = shader_rows.entry(sname.clone()).or_insert(ShaderRow {
            shader: sname,
            materials: 0,
            renderer_slots: 0,
            other_refs: 0,
            bundle: sbundle,
        });
        row.materials += 1;
        row.renderer_slots += slots;
        row.other_refs += other;
        for tex in
            mv.path("m_SavedProperties.m_TexEnvs").and_then(Value::as_array).unwrap_or_default()
        {
            let Some(p) = tex.path("second.m_Texture").and_then(Value::as_pptr) else { continue };
            if let Some(t) = universe.resolve(m.file, p)? {
                textures.insert((t.file.name.clone(), t.info.path_id));
            }
        }
    }
    let mut shaders: Vec<ShaderRow> = shader_rows.into_values().collect();
    shaders.sort_by(|a, b| {
        b.renderer_slots.cmp(&a.renderer_slots).then_with(|| a.shader.cmp(&b.shader))
    });
    reach.shaders = shaders;
    for (file, pid) in &textures {
        let Some(t) = universe.get(file, *pid)? else { continue };
        let cname = class_name(t.info.class_id);
        if !matches!(t.info.class_id, class_id::TEXTURE_2D | class_id::CUBEMAP | 187) {
            *reach.textures_by_format.entry(format!("({cname})")).or_default() += 1;
            continue;
        }
        let c = texture::check(universe, &cname, &t.read()?)?;
        reach.textures += 1;
        *reach.textures_by_format.entry(c.format.clone()).or_default() += 1;
        if c.ok {
            reach.textures_payload_ok += 1;
        } else if reach.texture_failures.len() < 30 {
            reach.texture_failures.push(c);
        }
    }
    reach.lights_by_type = lights_type;
    reach.lights_by_bake = lights_bake;
    reach.bundles_opened = universe.opened_bundles().len();
    report.assets = reach;

    // Inventory counts.
    let mut inv = BTreeMap::new();
    inv.insert("gameplay_game_objects".to_owned(), inventory.objects.len());
    inv.insert("entities".to_owned(), inventory.entities.len());
    inv.insert("spawners".to_owned(), inventory.spawners.len());
    inv.insert("zones".to_owned(), inventory.zones.len());
    inv.insert("zone_shapes".to_owned(), inventory.zones.iter().map(|z| z.shapes.len()).sum());
    for rec in &inventory.objects {
        for c in &rec.components {
            if c.relevance == Relevance::Gameplay || c.relevance == Relevance::Supporting {
                *inv.entry(format!("family:{}", c.family)).or_default() += 1;
            }
        }
    }
    report.inventory = inv;
    let mut seen = HashSet::new();
    report.duplicate_unique_ids = inventory
        .entities
        .iter()
        .filter(|e| !seen.insert(e.unique_id.clone()))
        .map(|e| e.unique_id.clone())
        .collect();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv_and_families() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_ne!(fnv1a(b"a"), fnv1a(b"b"));
        assert_eq!(bundle_family("settings_caves.worldtex"), ".worldtex");
        assert_eq!(bundle_family("fx"), "fx");
    }
}

/// One script class across all scene bundles.
#[derive(Debug, Clone, Serialize)]
pub struct ScriptUse {
    pub class: String,
    pub family: &'static str,
    pub relevance: Relevance,
    pub instances: usize,
    pub bundles: usize,
}

/// Census of every `.scenes` bundle in the install: full decode check and MonoBehaviour
/// script classes, to size the scene-component long tail beyond one area.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SceneCensus {
    pub bundles: usize,
    pub bytes: u64,
    pub objects: usize,
    pub objects_decoded: usize,
    pub game_objects: usize,
    pub mono_behaviours: usize,
    pub scripts_unresolved: usize,
    pub failures: Vec<String>,
    pub classes: Vec<ScriptUse>,
    pub by_relevance: BTreeMap<String, (usize, usize)>,
}

/// Runs the scene census over every bundle whose name ends in `.scenes`.
pub fn scene_census(bundles_dir: &Path) -> Result<SceneCensus> {
    let mut names: Vec<String> = std::fs::read_dir(bundles_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".scenes"))
        .collect();
    names.sort();
    struct Part {
        bytes: u64,
        objects: usize,
        decoded: usize,
        gos: usize,
        mbs: usize,
        unresolved: usize,
        failures: Vec<String>,
        classes: BTreeMap<String, usize>,
    }
    let parts: Vec<Result<Part>> = names
        .par_iter()
        .map(|name| {
            let path = bundles_dir.join(name);
            let bundle = Bundle::open(&path)?;
            let mut files = Vec::new();
            for n in &bundle.header.nodes {
                if !is_stream(n) {
                    files.push(SerializedFile::parse(&n.path, bundle.node_data(n)?)?);
                }
            }
            let mut p = Part {
                bytes: std::fs::metadata(&path)?.len(),
                objects: 0,
                decoded: 0,
                gos: 0,
                mbs: 0,
                unresolved: 0,
                failures: Vec::new(),
                classes: BTreeMap::new(),
            };
            let by_name: HashMap<&str, &SerializedFile<'_>> =
                files.iter().map(|f| (f.name.as_str(), f)).collect();
            for f in &files {
                for o in &f.objects {
                    p.objects += 1;
                    let v = match f.read(o) {
                        Ok(v) => v,
                        Err(e) => {
                            if p.failures.len() < 5 {
                                p.failures.push(format!("{name}: {e:#}"));
                            }
                            continue;
                        }
                    };
                    p.decoded += 1;
                    match o.class_id {
                        class_id::GAME_OBJECT => p.gos += 1,
                        class_id::MONO_BEHAVIOUR => {
                            p.mbs += 1;
                            let script =
                                v.get("m_Script").and_then(Value::as_pptr).and_then(|sp| {
                                    let target: &SerializedFile<'_> = if sp.file_id == 0 {
                                        Some(f)
                                    } else {
                                        usize::try_from(sp.file_id - 1)
                                            .ok()
                                            .and_then(|i| f.externals.get(i))
                                            .and_then(|e| by_name.get(e.file_name()).copied())
                                    }?;
                                    let so = target.object(sp.path_id)?;
                                    let sv = target.read(so).ok()?;
                                    let ns = sv.str_at("m_Namespace").unwrap_or_default();
                                    let cl = sv.str_at("m_ClassName").unwrap_or_default();
                                    Some(if ns.is_empty() {
                                        cl.to_owned()
                                    } else {
                                        format!("{ns}.{cl}")
                                    })
                                });
                            match script {
                                Some(c) => *p.classes.entry(c).or_default() += 1,
                                None => p.unresolved += 1,
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(p)
        })
        .collect();
    let mut c = SceneCensus { bundles: names.len(), ..Default::default() };
    let mut uses: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for part in parts {
        let p = part?;
        c.bytes += p.bytes;
        c.objects += p.objects;
        c.objects_decoded += p.decoded;
        c.game_objects += p.gos;
        c.mono_behaviours += p.mbs;
        c.scripts_unresolved += p.unresolved;
        c.failures.extend(p.failures);
        for (k, v) in p.classes {
            let u = uses.entry(k).or_default();
            u.0 += v;
            u.1 += 1;
        }
    }
    let mut classes: Vec<ScriptUse> = uses
        .into_iter()
        .map(|(class, (instances, bundles))| {
            let f = crate::families::classify_script(&class);
            ScriptUse { class, family: f.name, relevance: f.relevance, instances, bundles }
        })
        .collect();
    classes.sort_by(|a, b| a.relevance.cmp(&b.relevance).then(b.instances.cmp(&a.instances)));
    for u in &classes {
        let e = c.by_relevance.entry(format!("{:?}", u.relevance).to_lowercase()).or_default();
        e.0 += 1;
        e.1 += u.instances;
    }
    c.classes = classes;
    c.failures.truncate(50);
    Ok(c)
}
