//! Extracting an area's gameplay objects from its scene bundles.
//!
//! For each scene file: read every GameObject, Transform and component; rebuild the
//! hierarchy and world transforms; classify components; keep the GameObjects that carry
//! gameplay components, with their decoded fields and resolved references; and derive
//! the views later phases need directly (entities, spawners, zones with world shapes).

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::{Context, Result};
use glam::{Mat4, Quat, Vec3};
use serde::Serialize;

use crate::area::{ScriptName, Target, Universe};
use crate::blueprints::BlueprintIndex;
use crate::catalog::Locations;
use crate::families::{Family, Relevance, classify_builtin, classify_script};
use crate::refs::{self, BASE_FIELDS, RawRef};
use crate::transform::{self, Node, Trs, World};
use crate::unity::serialized::{SerializedFile, class_id, class_name};
use crate::unity::value::{PPtr, Value};

/// Identity of a scene object: serialized file and path id. Stable across runs (it is
/// the shipped data's own identity).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct ObjKey {
    pub file: String,
    pub path_id: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalTrs {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

/// A reference after resolution.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedRef {
    pub field: String,
    pub kind: &'static str,
    /// Human-readable target: `file:path_id`, a GUID or a unique id.
    pub target: String,
    pub resolved: bool,
    /// What it resolved to (class and name, blueprint type, bundle), or why not.
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComponentRecord {
    pub path_id: i64,
    /// Namespace-qualified script class, or the Unity class name for built-ins.
    pub class: String,
    pub assembly: String,
    pub family: &'static str,
    pub relevance: Relevance,
    pub enabled: Option<bool>,
    pub unique_id: Option<String>,
    /// Decoded fields, minus the base fields.
    pub fields: Value,
    pub refs: Vec<ResolvedRef>,
    pub persistent_calls: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GameObjectRecord {
    pub key: ObjKey,
    pub bundle: String,
    pub scene: String,
    pub name: String,
    /// `Root/Child/Name` through the transform hierarchy.
    pub path: String,
    /// `Transform.GetSiblingIndex()` of each GameObject below the root, root first
    /// excluded: with the root's name, this identifies the object at runtime even when
    /// names repeat (the oracle's join key).
    pub sibling_path: Vec<u32>,
    pub active_self: bool,
    pub active_in_hierarchy: bool,
    pub layer: i64,
    pub local: LocalTrs,
    pub world: World,
    pub components: Vec<ComponentRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntityRecord {
    pub unique_id: String,
    pub class: String,
    pub is_in_game: Option<bool>,
    pub game_object: ObjKey,
    pub name: String,
    pub path: String,
    pub scene: String,
    pub active_in_hierarchy: bool,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
}

#[derive(Debug, Clone, Serialize)]
pub struct SpawnerRecord {
    pub unique_id: String,
    pub game_object: ObjKey,
    pub name: String,
    pub blueprint: String,
    pub blueprint_type: Option<String>,
    pub blueprint_path: Option<String>,
    pub spawn_on_scene_init: Option<bool>,
    pub active_in_hierarchy: bool,
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    /// Other gameplay components on the same GameObject (spawner settings).
    pub settings: Vec<String>,
}

/// One containment probe.
#[derive(Debug, Clone, Serialize)]
pub struct Probe {
    pub point: [f32; 3],
    pub corpulence: f32,
    pub inside: Option<bool>,
}

/// Probe points for a shape: its centre, then just inside and just outside its extent
/// along local ±X, ±Z and ±Y, each at corpulence 0 and 0.5.
#[must_use]
pub fn probes(kind: &str, local: &Value, m: Mat4) -> Vec<Probe> {
    let (center, ext) = match kind {
        "ScriptZoneBox" => {
            let Some(b) = local.get("Bounds") else { return Vec::new() };
            (
                Vec3::from(b.get("m_Center").and_then(Value::vec3).unwrap_or([0.0; 3])),
                Vec3::from(b.get("m_Extent").and_then(Value::vec3).unwrap_or([0.0; 3])),
            )
        }
        "ScriptZoneCylinder" => {
            let c = Vec3::from(local.get("Center").and_then(Value::vec3).unwrap_or([0.0; 3]));
            let r = local.f32_at("Radius").unwrap_or(0.0);
            let h = local.f32_at("Height").unwrap_or(0.0);
            (c, Vec3::new(r, h, r))
        }
        _ => return Vec::new(),
    };
    let mut pts = vec![center];
    for axis in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z, Vec3::Y, Vec3::NEG_Y] {
        for f in [0.99, 1.01, 1.2] {
            pts.push(center + axis * ext * f);
        }
    }
    let mut out = Vec::new();
    for lp in pts {
        let wp = m.transform_point3(lp).to_array();
        for corpulence in [0.0, 0.5] {
            out.push(Probe {
                point: wp,
                corpulence,
                inside: shape_contains(kind, local, m, wp, corpulence),
            });
        }
    }
    out
}

/// An axis-aligned box as `(center, size)`, like Unity's `Bounds`.
pub type CenterSize = ([f32; 3], [f32; 3]);

/// A zone shape in world space.
#[derive(Debug, Clone, Serialize)]
pub struct ShapeRecord {
    pub kind: String,
    pub game_object: ObjKey,
    pub active_in_hierarchy: bool,
    pub enabled: Option<bool>,
    /// The shape's own fields (local space).
    pub local: Value,
    /// Shape transform `localToWorldMatrix`, column-major.
    pub matrix: [f32; 16],
    /// `IScriptZoneShape.Center()` in world space, where computable.
    pub world_center: Option<[f32; 3]>,
    /// `IScriptZoneShape.GetBounds()` as the game computes it: center and size.
    pub world_bounds: Option<CenterSize>,
    /// Containment probes: points around the shape and our `Contains` result, for the
    /// oracle to evaluate with the game's own `IScriptZoneShape.Contains`.
    pub probes: Vec<Probe>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ZoneRecord {
    pub unique_id: Option<String>,
    pub game_object: ObjKey,
    pub name: String,
    pub blueprint: Option<String>,
    pub active_in_hierarchy: bool,
    /// Shapes the zone collects (`GetComponentsInChildren<IScriptZoneShape>()`: active
    /// GameObjects only, depth-first) in order; inactive ones are listed with
    /// `active_in_hierarchy = false` and are not collected by the game.
    pub shapes: Vec<ShapeRecord>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct SceneSummary {
    pub bundle: String,
    pub scene: String,
    pub file: String,
    pub objects: usize,
    pub game_objects: usize,
    pub components_by_class: BTreeMap<String, usize>,
    pub hierarchy_errors: Vec<String>,
}

/// The area inventory written to `cache/import/<area>/inventory.json`.
#[derive(Debug, Clone, Serialize)]
pub struct Inventory {
    pub schema: &'static str,
    pub area: String,
    pub scenes: Vec<SceneSummary>,
    pub entities: Vec<EntityRecord>,
    pub spawners: Vec<SpawnerRecord>,
    pub zones: Vec<ZoneRecord>,
    pub objects: Vec<GameObjectRecord>,
}

/// Per-component data gathered while scanning, before records are built.
#[derive(Debug, Clone)]
struct Comp {
    path_id: i64,
    class: String,
    assembly: String,
    family: Family,
    value: Value,
}

#[derive(Debug, Clone)]
struct Go {
    name: String,
    active: bool,
    layer: i64,
    components: Vec<i64>,
}

/// One GameObject as [`SceneData::game_objects`] reports it.
#[derive(Debug, Clone)]
pub struct GoSnapshot {
    pub path_id: i64,
    pub name: String,
    pub path: String,
    pub sibling_path: Vec<u32>,
    pub active_self: bool,
    pub active_in_hierarchy: bool,
    pub layer: i64,
    pub local: Option<Trs>,
    pub world: World,
    /// Component classes in `m_Component` order (built-ins by Unity class name).
    pub components: Vec<String>,
}

/// All objects of one scene file, decoded.
#[derive(Debug)]
pub struct SceneData {
    pub bundle: String,
    pub scene: String,
    pub file: &'static SerializedFile<'static>,
    gos: BTreeMap<i64, Go>,
    /// GameObject → its Transform.
    go_transform: HashMap<i64, i64>,
    /// Transform → (GameObject, parent Transform, children).
    transforms: HashMap<i64, (i64, Option<i64>, Vec<i64>)>,
    comps: HashMap<i64, Comp>,
    worlds: HashMap<i64, World>,
    locals: HashMap<i64, Trs>,
    pub hierarchy_errors: Vec<String>,
}

fn trs_of(v: &Value) -> Trs {
    let p = v.get("m_LocalPosition").and_then(Value::vec3).unwrap_or([0.0; 3]);
    let r = v.get("m_LocalRotation").and_then(Value::vec4).unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let s = v.get("m_LocalScale").and_then(Value::vec3).unwrap_or([1.0; 3]);
    Trs { position: Vec3::from(p), rotation: Quat::from_array(r), scale: Vec3::from(s) }
}

fn strip_base(v: &Value) -> Value {
    Value::Object(
        v.fields().iter().filter(|(k, _)| !BASE_FIELDS.contains(&k.as_str())).cloned().collect(),
    )
}

impl SceneData {
    /// Decodes a scene file. MonoBehaviours are classified through their MonoScript.
    pub fn load(
        universe: &Universe,
        bundle: &str,
        scene: &str,
        file: &'static SerializedFile<'static>,
    ) -> Result<Self> {
        let mut gos = BTreeMap::new();
        let mut transforms = HashMap::new();
        let mut go_transform = HashMap::new();
        let mut comps = HashMap::new();
        let mut locals = HashMap::new();
        let mut script_cache: HashMap<PPtr, ScriptName> = HashMap::new();
        for o in &file.objects {
            match o.class_id {
                class_id::GAME_OBJECT => {
                    let v = file.read(o)?;
                    let components = v
                        .get("m_Component")
                        .and_then(Value::as_array)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|c| c.get("component").and_then(Value::as_pptr))
                        .filter(|p| p.file_id == 0)
                        .map(|p| p.path_id)
                        .collect();
                    gos.insert(
                        o.path_id,
                        Go {
                            name: v.str_at("m_Name").unwrap_or_default().to_owned(),
                            active: v.get("m_IsActive").and_then(Value::as_bool).unwrap_or(true),
                            layer: v.i64_at("m_Layer").unwrap_or(0),
                            components,
                        },
                    );
                }
                class_id::TRANSFORM | class_id::RECT_TRANSFORM => {
                    let v = file.read(o)?;
                    let go =
                        v.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
                    let parent = v
                        .get("m_Father")
                        .and_then(Value::as_pptr)
                        .filter(|p| !p.is_null())
                        .map(|p| p.path_id);
                    let children = v
                        .get("m_Children")
                        .and_then(Value::as_array)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(Value::as_pptr)
                        .map(|p| p.path_id)
                        .collect();
                    locals.insert(o.path_id, trs_of(&v));
                    transforms.insert(o.path_id, (go, parent, children));
                    go_transform.insert(go, o.path_id);
                    comps.insert(
                        o.path_id,
                        Comp {
                            path_id: o.path_id,
                            class: class_name(o.class_id),
                            assembly: String::new(),
                            family: classify_builtin(&class_name(o.class_id)),
                            value: v,
                        },
                    );
                }
                class_id::MONO_BEHAVIOUR => {
                    let v = file.read(o)?;
                    let sp = v.get("m_Script").and_then(Value::as_pptr);
                    let script = match sp {
                        Some(p) => {
                            if let Some(s) = script_cache.get(&p) {
                                Some(s.clone())
                            } else {
                                let s = universe.script_of(file, &v)?;
                                if let Some(s) = &s {
                                    script_cache.insert(p, s.clone());
                                }
                                s
                            }
                        }
                        None => None,
                    };
                    let (class, assembly) = script.map_or_else(
                        || ("<missing script>".to_owned(), String::new()),
                        |s| (s.full(), s.assembly),
                    );
                    let family = classify_script(&class);
                    comps.insert(
                        o.path_id,
                        Comp { path_id: o.path_id, class, assembly, family, value: v },
                    );
                }
                class_id::ASSET_BUNDLE => {}
                _ => {
                    let cname = class_name(o.class_id);
                    let value = file.read(o).unwrap_or(Value::Object(Vec::new()));
                    comps.insert(
                        o.path_id,
                        Comp {
                            path_id: o.path_id,
                            family: classify_builtin(&cname),
                            class: cname,
                            assembly: String::new(),
                            value,
                        },
                    );
                }
            }
        }
        let nodes: HashMap<i64, Node<i64>> = transforms
            .iter()
            .map(|(&t, (_, parent, _))| (t, Node { local: locals[&t], parent: *parent }))
            .collect();
        let (worlds, errs) = transform::world_transforms(&nodes);
        let hierarchy_errors = errs.iter().map(|e| format!("{e:?}")).collect();
        Ok(Self {
            bundle: bundle.to_owned(),
            scene: scene.to_owned(),
            file,
            gos,
            go_transform,
            transforms,
            comps,
            worlds,
            locals,
            hierarchy_errors,
        })
    }

    fn key(&self, path_id: i64) -> ObjKey {
        ObjKey { file: self.file.name.clone(), path_id }
    }

    /// `Root/…/Name` for a GameObject.
    fn go_path(&self, go: i64) -> String {
        let mut names = Vec::new();
        let mut t = self.go_transform.get(&go).copied();
        let mut visited = HashSet::new();
        while let Some(tr) = t {
            if !visited.insert(tr) {
                break;
            }
            let Some((g, parent, _)) = self.transforms.get(&tr) else { break };
            names.push(self.gos.get(g).map_or("?", |x| x.name.as_str()));
            t = *parent;
        }
        names.reverse();
        names.join("/")
    }

    /// Sibling indices from just below the root down to `go`.
    fn sibling_path(&self, go: i64) -> Vec<u32> {
        let mut out = Vec::new();
        let mut t = self.go_transform.get(&go).copied();
        let mut visited = HashSet::new();
        while let Some(tr) = t {
            if !visited.insert(tr) {
                break;
            }
            let Some((_, parent, _)) = self.transforms.get(&tr) else { break };
            let Some(p) = parent else { break };
            let idx = self
                .transforms
                .get(p)
                .and_then(|(_, _, children)| children.iter().position(|c| *c == tr))
                .map_or(u32::MAX, |i| u32::try_from(i).unwrap_or(u32::MAX));
            out.push(idx);
            t = Some(*p);
        }
        out.reverse();
        out
    }

    fn active_in_hierarchy(&self, go: i64) -> bool {
        let mut t = self.go_transform.get(&go).copied();
        let mut visited = HashSet::new();
        while let Some(tr) = t {
            if !visited.insert(tr) {
                break;
            }
            let Some((g, parent, _)) = self.transforms.get(&tr) else { break };
            if !self.gos.get(g).is_some_and(|x| x.active) {
                return false;
            }
            t = *parent;
        }
        true
    }

    fn world_of(&self, go: i64) -> World {
        self.go_transform.get(&go).and_then(|t| self.worlds.get(t)).copied().unwrap_or(World {
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            lossy_scale: [1.0; 3],
            matrix: Mat4::IDENTITY.to_cols_array(),
            depth: 0,
        })
    }

    /// Self and descendants of a GameObject, depth-first (as `GetComponentsInChildren`).
    fn descendants(&self, go: i64) -> Vec<i64> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![go];
        while let Some(g) = stack.pop() {
            // A cyclic hierarchy (reported in `hierarchy_errors`) must not loop forever.
            if !seen.insert(g) {
                continue;
            }
            out.push(g);
            if let Some(t) = self.go_transform.get(&g)
                && let Some((_, _, children)) = self.transforms.get(t)
            {
                for c in children.iter().rev() {
                    if let Some((cg, _, _)) = self.transforms.get(c) {
                        stack.push(*cg);
                    }
                }
            }
        }
        out
    }

    #[must_use]
    pub fn game_object_count(&self) -> usize {
        self.gos.len()
    }

    #[must_use]
    pub fn game_object_name(&self, path_id: i64) -> Option<&str> {
        self.gos.get(&path_id).map(|g| g.name.as_str())
    }

    /// The local transform of a Transform component, by path id.
    #[must_use]
    pub fn local_trs(&self, transform: i64) -> Option<Trs> {
        self.locals.get(&transform).copied()
    }

    /// Every GameObject with its hierarchy data, for the oracle diff.
    #[must_use]
    pub fn game_objects(&self) -> Vec<GoSnapshot> {
        let mut out: Vec<GoSnapshot> = self
            .gos
            .iter()
            .map(|(&g, go)| GoSnapshot {
                path_id: g,
                name: go.name.clone(),
                path: self.go_path(g),
                sibling_path: self.sibling_path(g),
                active_self: go.active,
                active_in_hierarchy: self.active_in_hierarchy(g),
                layer: go.layer,
                local: self.go_transform.get(&g).and_then(|t| self.locals.get(t)).copied(),
                world: self.world_of(g),
                components: go
                    .components
                    .iter()
                    .filter_map(|c| self.comps.get(c))
                    .map(|c| c.class.clone())
                    .collect(),
            })
            .collect();
        out.sort_by_key(|g| g.path_id);
        out
    }

    /// A GameObject's components in `m_Component` order: (class, decoded value).
    #[must_use]
    pub fn components_of(&self, go: i64) -> Vec<(&str, &Value)> {
        self.gos.get(&go).map_or_else(Vec::new, |g| {
            g.components
                .iter()
                .filter_map(|c| self.comps.get(c))
                .map(|c| (c.class.as_str(), &c.value))
                .collect()
        })
    }

    /// The local transforms from the root down to `go` (the chain Unity composes).
    #[must_use]
    pub fn local_chain(&self, go: i64) -> Vec<Trs> {
        let mut out = Vec::new();
        let mut t = self.go_transform.get(&go).copied();
        let mut visited = HashSet::new();
        while let Some(tr) = t {
            if !visited.insert(tr) {
                break;
            }
            let Some((_, parent, _)) = self.transforms.get(&tr) else { break };
            out.push(self.locals.get(&tr).copied().unwrap_or(Trs::IDENTITY));
            t = *parent;
        }
        out.reverse();
        out
    }

    /// Counts components by class name.
    #[must_use]
    pub fn class_counts(&self) -> BTreeMap<String, usize> {
        let mut m = BTreeMap::new();
        for c in self.comps.values() {
            *m.entry(c.class.clone()).or_default() += 1;
        }
        m
    }

    /// Every component: (class, family, value, owning GameObject active in hierarchy),
    /// for whole-scene statistics. Scene-level objects without a GameObject (settings,
    /// embedded assets) count as active.
    pub fn components(&self) -> impl Iterator<Item = (&str, Family, &Value, bool)> {
        self.comps.values().map(|c| {
            let active = c
                .value
                .get("m_GameObject")
                .and_then(Value::as_pptr)
                .is_none_or(|p| self.active_in_hierarchy(p.path_id));
            (c.class.as_str(), c.family, &c.value, active)
        })
    }
}

/// Resolves raw references against the universe, blueprint index, resource locations and
/// the area's entity IDs.
#[derive(Debug)]
pub struct Resolver<'a> {
    pub universe: &'a Universe,
    pub blueprints: &'a BlueprintIndex,
    pub locations: &'a Locations,
    pub entity_ids: HashSet<String>,
}

impl Resolver<'_> {
    pub fn resolve(&self, from: &SerializedFile<'_>, r: &RawRef) -> Result<ResolvedRef> {
        Ok(match r {
            RawRef::Object { field, pptr } => {
                let target = self.universe.target(from, *pptr)?;
                let (kind, resolved, tgt, detail) = match &target {
                    Target::Found { file, bundle, path_id } => {
                        let o =
                            self.universe.get(file, *path_id)?.context("found target vanished")?;
                        let cname = class_name(o.info.class_id);
                        let name = o
                            .file
                            .read_prefix(o.info, "m_Name")
                            .ok()
                            .and_then(|v| v.str_at("m_Name").map(str::to_owned))
                            .unwrap_or_default();
                        let kind = if file == &from.name { "scene_object" } else { "asset" };
                        (
                            kind,
                            true,
                            format!("{file}:{path_id}"),
                            format!("{cname} {name:?} in {bundle}"),
                        )
                    }
                    Target::MissingObject { file, bundle, path_id } => (
                        "asset",
                        false,
                        format!("{file}:{path_id}"),
                        format!("no such object in {bundle}"),
                    ),
                    Target::MissingFile { file, path_id } => (
                        "asset",
                        false,
                        format!("{file}:{path_id}"),
                        "file not in any bundle".into(),
                    ),
                    Target::BadFileId(id) => {
                        ("asset", false, format!("fileID {id}"), "bad file id".into())
                    }
                    Target::Null => ("asset", true, "null".into(), String::new()),
                };
                ResolvedRef { field: field.clone(), kind, target: tgt, resolved, detail }
            }
            RawRef::Blueprint { field, guid } => {
                let b = self.blueprints.get(guid);
                ResolvedRef {
                    field: field.clone(),
                    kind: "blueprint",
                    target: guid.clone(),
                    resolved: b.is_some(),
                    detail: b.map_or_else(
                        || "not in blueprints.zip".into(),
                        |b| format!("{} {}", b.type_name, b.path),
                    ),
                }
            }
            RawRef::Entity { field, unique_id } => {
                let ok = self.entity_ids.contains(unique_id);
                ResolvedRef {
                    field: field.clone(),
                    kind: "entity",
                    target: unique_id.clone(),
                    resolved: ok,
                    detail: if ok {
                        "entity in this area".into()
                    } else {
                        "not an entity of this area".into()
                    },
                }
            }
            RawRef::Resource { field, asset_id } => {
                let b = self.locations.guid_to_bundle.get(asset_id);
                ResolvedRef {
                    field: field.clone(),
                    kind: "resource",
                    target: asset_id.clone(),
                    resolved: b.is_some(),
                    detail: b.map_or_else(
                        || "not in locationlist.json".into(),
                        |b| format!("bundle {b}"),
                    ),
                }
            }
        })
    }
}

/// Whether a GameObject with these component families goes into the inventory.
fn is_gameplay(f: Family) -> bool {
    matches!(f.relevance, Relevance::Gameplay | Relevance::Supporting)
        && f.name != "transform"
        && f.name != "physics"
}

fn shape_world(kind: &str, local: &Value, m: Mat4) -> (Option<[f32; 3]>, Option<CenterSize>) {
    match kind {
        "ScriptZoneBox" => {
            let Some(b) = local.get("Bounds") else { return (None, None) };
            let c = Vec3::from(b.get("m_Center").and_then(Value::vec3).unwrap_or([0.0; 3]));
            let e = Vec3::from(b.get("m_Extent").and_then(Value::vec3).unwrap_or([0.0; 3]));
            let wc = m.transform_point3(c);
            // GetBounds(): center transformed, size NOT transformed (as the game does).
            (Some(wc.to_array()), Some((wc.to_array(), (e * 2.0).to_array())))
        }
        "ScriptZoneCylinder" => {
            let c = Vec3::from(local.get("Center").and_then(Value::vec3).unwrap_or([0.0; 3]));
            let h = local.f32_at("Height").unwrap_or(0.0);
            let r = local.f32_at("Radius").unwrap_or(0.0);
            let wc = m.transform_point3(c);
            (Some(wc.to_array()), Some((wc.to_array(), [2.0 * r, h, 2.0 * r])))
        }
        _ => (None, None),
    }
}

/// `IScriptZoneShape.Contains(point, corpulence)` as the game implements it, for the
/// shapes we decode. Reproduces two quirks of the original: the box's corpulence test
/// compares the *world* point's y with the *local* bounds centre, and compares a squared
/// distance with the unsquared corpulence; the cylinder's height test uses the full
/// `Height` as a half-height. `None` for unsupported shapes.
#[must_use]
pub fn shape_contains(
    kind: &str,
    local: &Value,
    matrix: Mat4,
    point: [f32; 3],
    corpulence: f32,
) -> Option<bool> {
    let p = Vec3::from(point);
    let lp = matrix.inverse().transform_point3(p);
    match kind {
        "ScriptZoneBox" => {
            let b = local.get("Bounds")?;
            let c = Vec3::from(b.get("m_Center").and_then(Value::vec3)?);
            let e = Vec3::from(b.get("m_Extent").and_then(Value::vec3)?);
            let (mn, mx) = (c - e, c + e);
            if lp.cmpge(mn).all() && lp.cmple(mx).all() {
                return Some(true);
            }
            if corpulence > 0.001 {
                if (c.y - p.y).abs() > e.y {
                    return Some(false);
                }
                let closest = lp.clamp(mn, mx);
                return Some((closest - lp).length_squared() <= corpulence);
            }
            Some(false)
        }
        "ScriptZoneCylinder" => {
            let c = Vec3::from(local.get("Center").and_then(Value::vec3)?);
            let h = local.f32_at("Height")?;
            let r = local.f32_at("Radius")?;
            if (lp.y - c.y).abs() > h {
                return Some(false);
            }
            let d2 = (c.x - lp.x).powi(2) + (c.z - lp.z).powi(2);
            Some(d2 <= (r + corpulence) * (r + corpulence))
        }
        _ => None,
    }
}

/// Builds the inventory for an area from its loaded scenes.
pub fn build_inventory(
    area: &str,
    scenes: &[SceneData],
    resolver: &mut Resolver<'_>,
) -> Result<Inventory> {
    // Entity IDs first, so entity references can be resolved.
    let mut entities = Vec::new();
    for s in scenes {
        for (go_id, go) in &s.gos {
            for c in go.components.iter().filter_map(|c| s.comps.get(c)) {
                let Some(uid) = c.value.str_at("UniqueId") else { continue };
                if c.value.get("m_IsInGame").is_none() {
                    continue;
                }
                let w = s.world_of(*go_id);
                entities.push(EntityRecord {
                    unique_id: uid.to_owned(),
                    class: c.class.clone(),
                    is_in_game: c.value.get("m_IsInGame").and_then(Value::as_bool),
                    game_object: s.key(*go_id),
                    name: go.name.clone(),
                    path: s.go_path(*go_id),
                    scene: s.scene.clone(),
                    active_in_hierarchy: s.active_in_hierarchy(*go_id),
                    position: w.position,
                    rotation: w.rotation,
                });
            }
        }
    }
    resolver.entity_ids = entities.iter().map(|e| e.unique_id.clone()).collect();

    let mut objects = Vec::new();
    let mut spawners = Vec::new();
    let mut zones = Vec::new();
    let mut summaries = Vec::new();
    for s in scenes {
        summaries.push(SceneSummary {
            bundle: s.bundle.clone(),
            scene: s.scene.clone(),
            file: s.file.name.clone(),
            objects: s.file.objects.len(),
            game_objects: s.gos.len(),
            components_by_class: s.class_counts(),
            hierarchy_errors: s.hierarchy_errors.clone(),
        });
        for (&go_id, go) in &s.gos {
            let comps: Vec<&Comp> = go.components.iter().filter_map(|c| s.comps.get(c)).collect();
            if !comps.iter().any(|c| is_gameplay(c.family)) {
                continue;
            }
            let mut records = Vec::new();
            for c in &comps {
                if !is_gameplay(c.family) {
                    // Keep built-ins and presentation components as names only.
                    records.push(ComponentRecord {
                        path_id: c.path_id,
                        class: c.class.clone(),
                        assembly: c.assembly.clone(),
                        family: c.family.name,
                        relevance: c.family.relevance,
                        enabled: c.value.get("m_Enabled").and_then(Value::as_bool),
                        unique_id: None,
                        fields: if c.class == "BoxCollider"
                            || c.class == "CapsuleCollider"
                            || c.class == "SphereCollider"
                        {
                            strip_base(&c.value)
                        } else {
                            Value::Object(Vec::new())
                        },
                        refs: Vec::new(),
                        persistent_calls: Vec::new(),
                    });
                    continue;
                }
                let mut rrefs = Vec::new();
                for r in refs::collect(&c.value) {
                    rrefs.push(resolver.resolve(s.file, &r)?);
                }
                records.push(ComponentRecord {
                    path_id: c.path_id,
                    class: c.class.clone(),
                    assembly: c.assembly.clone(),
                    family: c.family.name,
                    relevance: c.family.relevance,
                    enabled: c.value.get("m_Enabled").and_then(Value::as_bool),
                    unique_id: c.value.str_at("UniqueId").map(str::to_owned),
                    fields: strip_base(&c.value),
                    refs: rrefs,
                    persistent_calls: refs::persistent_calls(&c.value),
                });
            }
            let world = s.world_of(go_id);
            let local = s
                .go_transform
                .get(&go_id)
                .and_then(|t| s.locals.get(t))
                .copied()
                .unwrap_or(Trs::IDENTITY);
            let active_in_hierarchy = s.active_in_hierarchy(go_id);
            // Spawners.
            for c in comps.iter().filter(|c| c.class == "Kingmaker.View.Spawners.UnitSpawner") {
                let guid = c.value.str_at("m_Blueprint.guid").unwrap_or_default().to_owned();
                let bp = resolver.blueprints.get(&guid);
                spawners.push(SpawnerRecord {
                    unique_id: c.value.str_at("UniqueId").unwrap_or_default().to_owned(),
                    game_object: s.key(go_id),
                    name: go.name.clone(),
                    blueprint_type: bp.map(|b| b.type_name.clone()),
                    blueprint_path: bp.map(|b| b.path.clone()),
                    blueprint: guid,
                    spawn_on_scene_init: c.value.get("m_SpawnOnSceneInit").and_then(Value::as_bool),
                    active_in_hierarchy,
                    position: world.position,
                    rotation: world.rotation,
                    settings: comps
                        .iter()
                        .filter(|o| {
                            o.path_id != c.path_id
                                && o.family.relevance == Relevance::Gameplay
                                && o.family.name != "transform"
                        })
                        .map(|o| o.class.clone())
                        .collect(),
                });
            }
            // Zones.
            for c in comps
                .iter()
                .filter(|c| c.class == "Kingmaker.View.MapObjects.SriptZones.ScriptZone")
            {
                let mut shapes = Vec::new();
                for d in s.descendants(go_id) {
                    let Some(dgo) = s.gos.get(&d) else { continue };
                    for sc in dgo.components.iter().filter_map(|x| s.comps.get(x)) {
                        if sc.family.name != "zone_shape" {
                            continue;
                        }
                        let kind = sc.class.rsplit('.').next().unwrap_or_default().to_owned();
                        let m = Mat4::from_cols_array(&s.world_of(d).matrix);
                        let local = strip_base(&sc.value);
                        let (world_center, world_bounds) = shape_world(&kind, &local, m);
                        let probes = probes(&kind, &local, m);
                        shapes.push(ShapeRecord {
                            kind,
                            game_object: s.key(d),
                            active_in_hierarchy: s.active_in_hierarchy(d),
                            enabled: sc.value.get("m_Enabled").and_then(Value::as_bool),
                            local,
                            matrix: m.to_cols_array(),
                            world_center,
                            world_bounds,
                            probes,
                        });
                    }
                }
                zones.push(ZoneRecord {
                    unique_id: c.value.str_at("UniqueId").map(str::to_owned),
                    game_object: s.key(go_id),
                    name: go.name.clone(),
                    blueprint: c.value.str_at("m_Blueprint.guid").map(str::to_owned),
                    active_in_hierarchy,
                    shapes,
                });
            }
            objects.push(GameObjectRecord {
                key: s.key(go_id),
                bundle: s.bundle.clone(),
                scene: s.scene.clone(),
                name: go.name.clone(),
                path: s.go_path(go_id),
                sibling_path: s.sibling_path(go_id),
                active_self: go.active,
                active_in_hierarchy,
                layer: go.layer,
                local: LocalTrs {
                    position: local.position.to_array(),
                    rotation: local.rotation.to_array(),
                    scale: local.scale.to_array(),
                },
                world,
                components: records,
            });
        }
    }
    Ok(Inventory {
        schema: "wr-import/area-inventory/0",
        area: area.to_owned(),
        scenes: summaries,
        entities,
        spawners,
        zones,
        objects,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(f: Vec<(&str, Value)>) -> Value {
        Value::Object(f.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }
    fn v3(x: f32, y: f32, z: f32) -> Value {
        obj(vec![("x", Value::Float(x)), ("y", Value::Float(y)), ("z", Value::Float(z))])
    }

    #[test]
    fn zone_shapes_match_the_games_get_bounds() {
        // Shape transform: translate (10, 0, 5), rotate 90° about Y, scale 2.
        let m = Mat4::from_scale_rotation_translation(
            Vec3::splat(2.0),
            Quat::from_rotation_y(std::f32::consts::FRAC_PI_2),
            Vec3::new(10.0, 0.0, 5.0),
        );
        let bx = obj(vec![(
            "Bounds",
            obj(vec![("m_Center", v3(1.0, 0.0, 0.0)), ("m_Extent", v3(5.0, 1.5, 15.0))]),
        )]);
        let (c, b) = shape_world("ScriptZoneBox", &bx, m);
        let c = c.unwrap();
        // Center is transformed (local +X → world -Z, scaled by 2)...
        assert!((c[0] - 10.0).abs() < 1e-5 && (c[2] - 3.0).abs() < 1e-5);
        // ...but GetBounds() keeps the local size, unrotated and unscaled.
        assert_eq!(b.unwrap().1, [10.0, 3.0, 30.0]);
        let cyl = obj(vec![
            ("Center", v3(0.0, 0.0, 0.0)),
            ("Height", Value::Float(3.0)),
            ("Radius", Value::Float(8.0)),
        ]);
        let (c, b) = shape_world("ScriptZoneCylinder", &cyl, m);
        assert_eq!(c.unwrap(), [10.0, 0.0, 5.0]);
        assert_eq!(b.unwrap().1, [16.0, 3.0, 16.0]);
        assert_eq!(shape_world("ScriptZonePolygon", &cyl, m), (None, None));
    }

    #[test]
    fn zone_containment_reproduces_the_games_quirks() {
        let m = Mat4::from_translation(Vec3::new(0.0, 10.0, 0.0));
        let bx = obj(vec![(
            "Bounds",
            obj(vec![("m_Center", v3(0.0, 0.0, 0.0)), ("m_Extent", v3(1.0, 1.0, 1.0))]),
        )]);
        assert_eq!(shape_contains("ScriptZoneBox", &bx, m, [0.5, 10.5, 0.0], 0.0), Some(true));
        assert_eq!(shape_contains("ScriptZoneBox", &bx, m, [1.5, 10.0, 0.0], 0.0), Some(false));
        // The corpulence branch compares world y (10) with local centre y (0): always
        // false for this zone, however close the point.
        assert_eq!(shape_contains("ScriptZoneBox", &bx, m, [1.1, 10.0, 0.0], 0.5), Some(false));
        let m0 = Mat4::IDENTITY;
        // Squared distance 0.25 vs unsquared corpulence 0.3: inside.
        assert_eq!(shape_contains("ScriptZoneBox", &bx, m0, [1.5, 0.0, 0.0], 0.3), Some(true));
        // Squared distance 0.36 > 0.3 although the distance 0.6 is within 0.6 + margin.
        assert_eq!(shape_contains("ScriptZoneBox", &bx, m0, [1.6, 0.0, 0.0], 0.3), Some(false));
        let cyl = obj(vec![
            ("Center", v3(0.0, 0.0, 0.0)),
            ("Height", Value::Float(3.0)),
            ("Radius", Value::Float(2.0)),
        ]);
        // Height acts as a half-height: y = 2.9 is inside a "3 m" cylinder.
        assert_eq!(
            shape_contains("ScriptZoneCylinder", &cyl, m0, [0.0, 2.9, 0.0], 0.0),
            Some(true)
        );
        assert_eq!(
            shape_contains("ScriptZoneCylinder", &cyl, m0, [2.4, 0.0, 0.0], 0.5),
            Some(true)
        );
        assert_eq!(
            shape_contains("ScriptZoneCylinder", &cyl, m0, [2.6, 0.0, 0.0], 0.5),
            Some(false)
        );
    }
}
