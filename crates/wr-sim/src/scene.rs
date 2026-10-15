//! The area's scene: script zones and the scene data they need (decompiled, not copied:
//! `Kingmaker.View.MapObjects.SriptZones/{ScriptZone,ScriptZoneCylinder,ScriptZoneBox}.cs`,
//! `Kingmaker.Controllers.MapObjects/ScriptZoneController.cs`).
//!
//! A save holds a script zone's `IsActive` and the entity's parts and facts. What the zone
//! *is* (its blueprint-independent settings `m_OnceOnly`, `m_PlayersOnly`, `m_StartInactive`,
//! the `OnUnitEntered`/`OnUnitExited` Unity events and its shapes) lives in the Unity scene.
//! The sim reads it from a local cache, `cache/sim/scenes/<area guid>.json`, built from the
//! oracle's scene dump (`wr-sim scene-import`); an area without a cache file reports the
//! scene entities it cannot place (fail closed).
//!
//! Runtime state per zone is `ScriptZoneData`: `IsActive` (saved) and `InsideUnits` (not
//! saved).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wr_content::Guid;

use crate::elements::{self, Ctx};
use crate::world::{EntityIdx, World};

/// Cache file schema tag.
pub const SCHEMA: &str = "wr-sim-scene-v2";

/// One shape of a zone (`IScriptZoneShape`), with the world-to-local transform of its
/// `Transform` (the importer's `localToWorldMatrix`, inverted at import).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shape {
    /// `ScriptZoneCylinder`, `ScriptZoneBox`, or another kind (no port: reported on use).
    pub kind: String,
    /// Row-major 3x4 of `Transform.worldToLocalMatrix` (the affine part).
    pub world_to_local: [[f64; 4]; 3],
    /// Cylinder: `Center`, `Height`, `Radius`.
    #[serde(default)]
    pub center: [f64; 3],
    #[serde(default)]
    pub height: f64,
    #[serde(default)]
    pub radius: f64,
    /// Box: `Bounds.center` and `Bounds.extents`.
    #[serde(default)]
    pub bounds_center: [f64; 3],
    #[serde(default)]
    pub bounds_extent: [f64; 3],
}

/// A `ScriptZone` view's scene settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneInfo {
    pub unique_id: String,
    pub blueprint: Option<String>,
    pub once_only: bool,
    pub players_only: bool,
    pub start_inactive: bool,
    /// Persistent calls of `OnUnitEntered` and `OnUnitExited` (UnityEvents).
    pub unit_entered_calls: usize,
    pub unit_exited_calls: usize,
    /// `GetComponentsInChildren<IScriptZoneShape>()`: the shapes on active objects, in
    /// hierarchy order. Empty when the zone's own object is inactive (`OnEnable` doesn't run).
    pub shapes: Vec<Shape>,
}

/// A serialized component of a view (`class` is the CLR name; `fields` are the importer's
/// typetree values).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompInfo {
    pub class: String,
    pub fields: Value,
}

/// An entity view of the scene, by unique id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewInfo {
    pub unique_id: String,
    pub class: String,
    pub scene: String,
    pub active_in_hierarchy: bool,
    /// `IsInGameBySettings` (`m_IsInGame`).
    pub is_in_game: bool,
    /// The view's gameplay components (map objects, zones, unit spawners and roaming
    /// waypoints).
    #[serde(default)]
    pub components: Vec<CompInfo>,
    /// World position and rotation (quaternion x, y, z, w) of the view's object.
    #[serde(default)]
    pub position: [f32; 3],
    #[serde(default)]
    pub rotation: [f32; 4],
    /// A spawner's `UnitGroupView` (its parent object): the group's id and `DisableOnSimplified`.
    #[serde(default)]
    pub group: Option<(String, bool)>,
}

/// `cache/sim/scenes/<area>.json`, built by `wr-sim scene-import` from the importer's
/// `cache/import/<area name>/inventory.json` (`wr-import import --area <name>`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneCache {
    pub schema: String,
    /// The area blueprint's GUID.
    pub area: String,
    /// The importer's area name.
    pub name: String,
    pub source: String,
    pub zones: Vec<ZoneInfo>,
    pub views: Vec<ViewInfo>,
}

impl SceneCache {
    #[must_use]
    pub fn path(area: &str) -> PathBuf {
        wr_paths::cache_root().join("sim").join("scenes").join(format!("{area}.json"))
    }

    /// The cache of an area, if one was built.
    #[must_use]
    pub fn load(area: &str) -> Option<Rc<Self>> {
        let text = std::fs::read_to_string(Self::path(area)).ok()?;
        let c: Self = serde_json::from_str(&text).ok()?;
        (c.schema == SCHEMA && c.area == area).then(|| Rc::new(c))
    }

    /// Builds the cache from the importer's inventory of the area's scenes.
    ///
    /// # Errors
    ///
    /// A malformed inventory.
    pub fn import(inventory: &Path, area_guid: &str) -> Result<Self> {
        let text =
            std::fs::read_to_string(inventory).with_context(|| inventory.display().to_string())?;
        let d: Value = serde_json::from_str(&text)?;
        let name = d["area"].as_str().context("inventory without area")?.to_owned();
        let f3 = |v: &Value| -> [f64; 3] {
            let g = |k: &str| v[k].as_f64().unwrap_or(0.0);
            [g("x"), g("y"), g("z")]
        };
        let flag = |v: &Value| v.as_i64().map_or_else(|| v.as_bool().unwrap_or(false), |i| i != 0);
        // Components by owner unique id.
        let mut comps_of: std::collections::HashMap<String, Vec<(String, Value)>> =
            std::collections::HashMap::new();
        for o in d["objects"].as_array().context("inventory without objects")? {
            let cs = o["components"].as_array().cloned().unwrap_or_default();
            let Some(uid) = cs.iter().find_map(|c| c["unique_id"].as_str()).map(str::to_owned)
            else {
                continue;
            };
            let list = comps_of.entry(uid).or_default();
            for c in cs {
                list.push((
                    c["class"].as_str().unwrap_or_default().to_owned(),
                    c["fields"].clone(),
                ));
            }
        }
        let mut zones = Vec::new();
        for z in d["zones"].as_array().context("inventory without zones")? {
            let Some(uid) = z["unique_id"].as_str() else { continue };
            let fields = comps_of
                .get(uid)
                .and_then(|cs| cs.iter().find(|(c, _)| c.ends_with("SriptZones.ScriptZone")))
                .map_or(Value::Null, |(_, f)| f.clone());
            let calls = |n: &str| {
                fields[n]["m_PersistentCalls"]["m_Calls"].as_array().map_or(0, std::vec::Vec::len)
            };
            let mut shapes = Vec::new();
            if z["active_in_hierarchy"].as_bool() == Some(true) {
                for s in z["shapes"].as_array().into_iter().flatten() {
                    if s["active_in_hierarchy"].as_bool() != Some(true) {
                        continue;
                    }
                    let kind = s["kind"].as_str().unwrap_or_default().to_owned();
                    let m = s["matrix"].as_array().context("shape without matrix")?;
                    let mut l2w = [[0.0f64; 4]; 4];
                    for (i, x) in m.iter().enumerate().take(16) {
                        // `to_cols_array`: column-major.
                        l2w[i % 4][i / 4] = x.as_f64().unwrap_or(0.0);
                    }
                    let w2l = invert_affine(&l2w).context("singular shape transform")?;
                    let l = &s["local"];
                    let mut sh = Shape {
                        kind: kind.clone(),
                        world_to_local: w2l,
                        center: [0.0; 3],
                        height: 0.0,
                        radius: 0.0,
                        bounds_center: [0.0; 3],
                        bounds_extent: [0.0; 3],
                    };
                    match kind.as_str() {
                        "ScriptZoneCylinder" => {
                            sh.center = f3(&l["Center"]);
                            sh.height = l["Height"].as_f64().unwrap_or(0.0);
                            sh.radius = l["Radius"].as_f64().unwrap_or(0.0);
                        }
                        "ScriptZoneBox" => {
                            sh.bounds_center = f3(&l["Bounds"]["m_Center"]);
                            sh.bounds_extent = f3(&l["Bounds"]["m_Extent"]);
                        }
                        _ => {}
                    }
                    shapes.push(sh);
                }
            }
            zones.push(ZoneInfo {
                unique_id: uid.to_owned(),
                blueprint: z["blueprint"].as_str().map(str::to_owned),
                once_only: flag(&fields["m_OnceOnly"]),
                players_only: flag(&fields["m_PlayersOnly"]),
                start_inactive: flag(&fields["m_StartInactive"]),
                unit_entered_calls: calls("OnUnitEntered"),
                unit_exited_calls: calls("OnUnitExited"),
                shapes,
            });
        }
        // Objects by scene and path, for a spawner's group (`UnitGroupView.OnEnable`: the
        // direct children of the group object that hold a `UnitSpawner`).
        let mut by_path: std::collections::HashMap<(String, String), &Value> =
            std::collections::HashMap::new();
        for o in d["objects"].as_array().into_iter().flatten() {
            if let (Some(s), Some(p)) = (o["scene"].as_str(), o["path"].as_str()) {
                by_path.insert((s.to_owned(), p.to_owned()), o);
            }
        }
        let group_of = |v: &Value| -> Option<(String, bool)> {
            let scene = v["scene"].as_str()?;
            let path = v["path"].as_str()?;
            let parent = path.rsplit_once('/')?.0;
            let o = by_path.get(&(scene.to_owned(), parent.to_owned()))?;
            let c = o["components"]
                .as_array()?
                .iter()
                .find(|c| c["class"].as_str().is_some_and(|k| k.ends_with(".UnitGroupView")))?;
            Some((c["unique_id"].as_str()?.to_owned(), flag(&c["fields"]["DisableOnSimplified"])))
        };
        let f32s = |v: &Value, n: usize| -> Vec<f32> {
            (0..n).map(|i| v[i].as_f64().unwrap_or(0.0) as f32).collect()
        };
        let views = d["entities"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| {
                let uid = v["unique_id"].as_str()?.to_owned();
                let class = v["class"].as_str()?.to_owned();
                let components = if class.ends_with(".MapObjectView")
                    || class.ends_with("SriptZones.ScriptZone")
                    || class.ends_with(".UnitSpawner")
                    || class.ends_with(".RoamingWaypointView")
                {
                    comps_of
                        .get(&uid)
                        .into_iter()
                        .flatten()
                        .map(|(c, f)| CompInfo { class: c.clone(), fields: f.clone() })
                        .collect()
                } else {
                    Vec::new()
                };
                let group = if class.ends_with(".UnitSpawner") { group_of(v) } else { None };
                Some(ViewInfo {
                    unique_id: uid,
                    class,
                    scene: v["scene"].as_str().unwrap_or_default().to_owned(),
                    active_in_hierarchy: v["active_in_hierarchy"].as_bool().unwrap_or(true),
                    is_in_game: v["is_in_game"].as_bool().unwrap_or(true),
                    components,
                    position: f32s(&v["position"], 3).try_into().unwrap_or_default(),
                    rotation: f32s(&v["rotation"], 4).try_into().unwrap_or_default(),
                    group,
                })
            })
            .collect();
        Ok(Self {
            schema: SCHEMA.into(),
            area: area_guid.to_owned(),
            name,
            source: format!("wr-import inventory {}", inventory.display()),
            zones,
            views,
        })
    }

    /// Writes the cache file.
    ///
    /// # Errors
    ///
    /// I/O.
    pub fn save(&self) -> Result<PathBuf> {
        let p = Self::path(&self.area);
        std::fs::create_dir_all(p.parent().context("no parent")?)?;
        std::fs::write(&p, serde_json::to_string_pretty(self)?)?;
        Ok(p)
    }

    #[must_use]
    pub fn zone(&self, unique_id: &str) -> Option<&ZoneInfo> {
        self.zones.iter().find(|z| z.unique_id == unique_id)
    }

    #[must_use]
    pub fn view(&self, unique_id: &str) -> Option<&ViewInfo> {
        self.views.iter().find(|v| v.unique_id == unique_id)
    }
}

/// [`invert_affine`] for the tests.
#[cfg(test)]
pub(crate) fn invert_affine_for_tests(m: &[[f64; 4]; 4]) -> Option<[[f64; 4]; 3]> {
    invert_affine(m)
}

/// Inverse of an affine 4x4 (last row 0 0 0 1), as the 3x4 part.
fn invert_affine(m: &[[f64; 4]; 4]) -> Option<[[f64; 4]; 3]> {
    let a = [[m[0][0], m[0][1], m[0][2]], [m[1][0], m[1][1], m[1][2]], [m[2][0], m[2][1], m[2][2]]];
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = [
        [
            (a[1][1] * a[2][2] - a[1][2] * a[2][1]) / det,
            (a[0][2] * a[2][1] - a[0][1] * a[2][2]) / det,
            (a[0][1] * a[1][2] - a[0][2] * a[1][1]) / det,
        ],
        [
            (a[1][2] * a[2][0] - a[1][0] * a[2][2]) / det,
            (a[0][0] * a[2][2] - a[0][2] * a[2][0]) / det,
            (a[0][2] * a[1][0] - a[0][0] * a[1][2]) / det,
        ],
        [
            (a[1][0] * a[2][1] - a[1][1] * a[2][0]) / det,
            (a[0][1] * a[2][0] - a[0][0] * a[2][1]) / det,
            (a[0][0] * a[1][1] - a[0][1] * a[1][0]) / det,
        ],
    ];
    let t = [m[0][3], m[1][3], m[2][3]];
    let mut out = [[0.0; 4]; 3];
    for r in 0..3 {
        for c in 0..3 {
            out[r][c] = inv[r][c];
        }
        out[r][3] = -(inv[r][0] * t[0] + inv[r][1] * t[1] + inv[r][2] * t[2]);
    }
    Some(out)
}

impl Shape {
    /// `Transform.InverseTransformPoint`.
    fn to_local(&self, p: [f32; 3]) -> [f32; 3] {
        let w = &self.world_to_local;
        let p = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
        let mut out = [0.0f32; 3];
        for (r, o) in out.iter_mut().enumerate() {
            *o = (w[r][0] * p[0] + w[r][1] * p[1] + w[r][2] * p[2] + w[r][3]) as f32;
        }
        out
    }

    /// `IScriptZoneShape.Contains(point, corpulence)`. `None` for a shape kind without a
    /// port.
    #[must_use]
    pub fn contains(&self, point: [f32; 3], corpulence: f32) -> Option<bool> {
        let b = self.to_local(point);
        match self.kind.as_str() {
            // `ScriptZoneCylinder.Contains`.
            "ScriptZoneCylinder" => {
                let c = [self.center[0] as f32, self.center[1] as f32, self.center[2] as f32];
                if (b[1] - c[1]).abs() > self.height as f32 {
                    return Some(false);
                }
                // `GeometryUtils.SqrDistance2D`: x and z.
                let dx = c[0] - b[0];
                let dz = c[2] - b[2];
                let r = self.radius as f32 + corpulence;
                Some(dx * dx + dz * dz <= r * r)
            }
            // `ScriptZoneBox.Contains`.
            "ScriptZoneBox" => {
                let ctr = [
                    self.bounds_center[0] as f32,
                    self.bounds_center[1] as f32,
                    self.bounds_center[2] as f32,
                ];
                let ext = [
                    self.bounds_extent[0] as f32,
                    self.bounds_extent[1] as f32,
                    self.bounds_extent[2] as f32,
                ];
                // `Bounds.Contains`: inclusive on each axis.
                let inside = (0..3).all(|i| b[i] >= ctr[i] - ext[i] && b[i] <= ctr[i] + ext[i]);
                if inside {
                    return Some(true);
                }
                if corpulence > 0.001 {
                    if (ctr[1] - point[1]).abs() > ext[1] {
                        return Some(false);
                    }
                    // `Bounds.ClosestPoint(vector) - vector`, squared length vs corpulence.
                    let mut s = 0.0f32;
                    for i in 0..3 {
                        let cp = b[i].clamp(ctr[i] - ext[i], ctr[i] + ext[i]);
                        s += (cp - b[i]) * (cp - b[i]);
                    }
                    return Some(s <= corpulence);
                }
                Some(false)
            }
            _ => None,
        }
    }
}

/// A unit inside a zone (`ScriptZone.UnitInfo`).
#[derive(Debug, Clone)]
pub struct Inside {
    pub unit: EntityIdx,
    pub inside_this_tick: bool,
}

/// `ScriptZone.ScriptZoneData` with its view's scene settings.
#[derive(Debug, Clone)]
pub struct Zone {
    pub entity: EntityIdx,
    pub id: String,
    /// The blueprint of the entity's `FactHolder.Fact` (the zone's `m_Blueprint`).
    pub blueprint: Option<Guid>,
    /// `ScriptZoneData.IsActive` (saved).
    pub is_active: bool,
    /// `ScriptZoneData.InsideUnits`.
    pub inside: Vec<Inside>,
}

/// The loaded area's scene state.
#[derive(Debug, Default)]
pub struct SceneState {
    pub cache: Option<Rc<SceneCache>>,
    pub zones: Vec<Zone>,
    /// The zones whose `TryAddUnit` is raising its events (`IScriptZoneHandler`), innermost
    /// last: the `zone` argument of the handlers.
    pub current: Vec<usize>,
    /// Map objects (`MapObjectEntityData`, script zones included) and their parts' state.
    pub map_objects: HashMap<EntityIdx, crate::scene_parts::MapObj>,
    /// The player's shared stashes.
    pub stashes: crate::scene_parts::Stashes,
    /// `CutscenePlayerData` entities of the area's scene states, in load order.
    pub cutscenes: Vec<CutscenePlayer>,
    /// `Game.SummonPools` (`SummonPoolsManager.m_Pools`): pools in creation order, with the
    /// units registered in each. Not saved: rebuilt by `PooledPart.OnPostLoad` and
    /// `BecomeUnit`.
    pub summon_pools: Vec<(Guid, Vec<EntityIdx>)>,
    /// Each pooled unit's `PooledPart.Pools`, in order.
    pub pooled: HashMap<EntityIdx, Vec<Guid>>,
    /// The interactions spawners gave their units (`UnitPartInteractions`): unit, spawner id,
    /// dialogue.
    pub interactions: Vec<(EntityIdx, String, Option<Guid>)>,
    /// `UnitPartVendor.SharedInventory` of units whose vendor part points at a shared table.
    pub vendors: HashMap<EntityIdx, Guid>,
    /// `UnitPartCorruption.m_AppliedBuffs` of each unit with the part.
    pub corruption: HashMap<EntityIdx, Vec<crate::world::FactIdx>>,
    /// A cutscene's T0 action is running ([`crate::elements::cutscene::run_pending`]).
    pub in_cutscene_step: bool,
    /// Started cutscenes' T0 actions, run by the cutscene controller's next tick.
    pub cutscene_pending: Vec<crate::elements::cutscene::Pending>,
    /// The run takes cutscene progress from a recording (set before the load).
    pub recorded_cutscenes: bool,
    /// The run also takes the cutscene lock's starts and stops from the recording
    /// (`CommandLockControls`: `UnitBuffsController.TickAll`/`PostponeAll`, Cutscene mode).
    pub recorded_locks: bool,
    /// The run also takes cutscene commands' dialogue starts and entry checks from the
    /// recording (`CommandStartDialog`, `EntryCondition`s that skip their command).
    pub recorded_commands: bool,
    /// Random gates accepted for recorded draws: (the player's cutscene, the gate).
    pub random_gates_t1: std::collections::HashSet<(Guid, Guid)>,
    /// Cutscenes the sim classified (started, restored, or in their closures): a recorded
    /// command of another one is reported.
    pub classified_cutscenes: std::collections::HashSet<Guid>,
    /// Unit prefabs' `IsInGameBySettings` by asset GUID (`cache/sim/unit-prefabs.json`,
    /// [`crate::scene_spawn::load_unit_prefabs`]); `None` when the table wasn't built.
    pub unit_prefabs: Option<Rc<HashMap<String, Option<bool>>>>,
}

/// A saved `CutscenePlayerData` (`Kingmaker.AreaLogic.Cutscenes/CutscenePlayerData.cs`).
#[derive(Debug, Clone)]
pub struct CutscenePlayer {
    pub entity: EntityIdx,
    /// `m_Cutscene`.
    pub cutscene: Option<Guid>,
    /// `Parameters.Params`, values as `NamedParametersContext.IsTheSame` normalizes them.
    pub params: Vec<(String, String)>,
    pub is_queued: bool,
    pub queued_after: bool,
    pub restart: bool,
    pub remove: bool,
    /// Number of `m_ActivatedGates`.
    pub activated_gates: usize,
}

/// `SummonPoolsManager.Register(pool, unit)` → `SummonPool.Register`: the unit joins the
/// pool's list (once), its `PooledPart.Pools` gains the pool, and `ISummonPoolHandler`
/// subscribers hear `HandleUnitAdded`.
pub fn summon_pool_register(w: &mut World, pool: Guid, unit: EntityIdx) {
    let i = match w.scene.summon_pools.iter().position(|(g, _)| *g == pool) {
        Some(i) => i,
        None => {
            w.scene.summon_pools.push((pool, Vec::new()));
            w.scene.summon_pools.len() - 1
        }
    };
    if w.scene.summon_pools[i].1.contains(&unit) {
        return;
    }
    w.scene.summon_pools[i].1.push(unit);
    // `unit.Ensure<PooledPart>().Pools`.
    if !w.entity(unit).parts.iter().any(|p| p.clr == "PooledPart") {
        w.entity_mut(unit).parts.push(crate::world::Part { clr: "PooledPart" });
    }
    let pools = w.scene.pooled.entry(unit).or_default();
    if !pools.contains(&pool) {
        pools.push(pool);
    }
    crate::rulebook::bus::raise_global_call(w, "ISummonPoolHandler", "HandleUnitAdded", Some(unit));
}

/// `SummonPool.Unregister(unit)`: the unit leaves the pool and its `PooledPart` forgets it; unless
/// the unit is being disposed, `ISummonPoolHandler.HandleUnitRemoved` (and `HandleLastUnitRemoved`
/// for an emptied pool) are raised.
pub fn summon_pool_unregister(w: &mut World, pool: Guid, unit: EntityIdx) {
    let Some(i) = w.scene.summon_pools.iter().position(|(g, _)| *g == pool) else { return };
    let Some(p) = w.scene.summon_pools[i].1.iter().position(|u| *u == unit) else { return };
    w.scene.summon_pools[i].1.remove(p);
    if let Some(pools) = w.scene.pooled.get_mut(&unit) {
        pools.retain(|g| *g != pool);
    }
    if w.entity(unit).disposing {
        return;
    }
    crate::rulebook::bus::raise_global_call(
        w,
        "ISummonPoolHandler",
        "HandleUnitRemoved",
        Some(unit),
    );
    if w.scene.summon_pools[i].1.is_empty() {
        crate::rulebook::bus::raise_global_call(
            w,
            "ISummonPoolHandler",
            "HandleLastUnitRemoved",
            Some(unit),
        );
    }
}

/// `NamedParametersContext.IsTheSame`: every parameter of `a` is in `b` with the same
/// normalized value.
#[must_use]
pub fn params_same(a: &[(String, String)], b: &[(String, String)]) -> bool {
    a.iter().all(|(k, v)| b.iter().any(|(k2, v2)| k == k2 && v == v2))
}

/// `CutscenePlayerData.OnPostLoad` (`DestroyCutsceneOnLoad`, the queue fix-up,
/// `StopCopies`), and what keeps the player playing (the background cutscene controller
/// ticks it). Cutscene playback is T1 ([`crate::elements::cutscene`]): the cutscene's closure
/// is classified, T1 commands are stubbed and anything with a T0 effect is reported.
pub fn cutscene_post_load(w: &mut World, e: EntityIdx) {
    let Some(p) = w.scene.cutscenes.iter().find(|c| c.entity == e).cloned() else { return };
    let Some(g) = p.cutscene else {
        w.coverage.unsupported("CutscenePlayerData: no cutscene blueprint");
        return;
    };
    let Some(bp) = w.content.get(g) else {
        w.coverage.unsupported("CutscenePlayerData: cutscene blueprint missing");
        return;
    };
    for o in bp.components_array() {
        for n in ["DestroyCutsceneOnLoad", "StopCutsceneWhenExitingArea"] {
            if crate::content::is_a(o.ty, n) {
                w.coverage.unsupported(format!("CutscenePlayerData: cutscene component {n}"));
            }
        }
    }
    if p.is_queued || p.queued_after || p.restart || p.remove {
        w.coverage.unsupported("CutscenePlayerData: saved queue, restart or remove state");
    }
    // `StopCopies()`: a copy with the same parameters is stopped (its `OnStopped` actions run).
    if !bp.view().bool("AllowCopies")
        && w.scene
            .cutscenes
            .iter()
            .any(|o| o.entity != e && o.cutscene == p.cutscene && params_same(&p.params, &o.params))
    {
        w.coverage.unsupported("CutscenePlayerData.StopCopies: a copy is stopped");
    }
    // `Cutscene.ForbidDialogs` blocks a dialogue whose first speaker the cutscene controls.
    if bp.view().bool("ForbidDialogs") {
        w.coverage.unsupported("Cutscene.ForbidDialogs (control of dialogue speakers)");
    }
    // Playback (`TickScene`, `Restore`, commands' `OnTurnOn`) is T1; commands with T0 effects
    // are reported.
    if p.activated_gates > 0 {
        let cls = crate::elements::cutscene::classify(w, g);
        for t in &cls.t1_commands {
            w.coverage.stub(format!("cutscene command {t} (T1)"));
        }
        for t in &cls.t1_actions {
            w.coverage.stub(format!("cutscene action {t} (T1: positions, cutscene control)"));
        }
        crate::elements::cutscene::settle_recorded(w, &cls);
        for u in &cls.unsupported {
            w.coverage.unsupported(format!("cutscene: {u}"));
        }
        if !cls.dialogs.is_empty() {
            w.coverage.unsupported("cutscene: CommandStartDialog in a running background cutscene");
        }
        // `Restore` runs each started track's current command again: a lock there would lock
        // and tick the buffs at load, and the saved positions aren't read.
        if cls.t1_commands.contains(&"CommandLockControls") {
            w.coverage.unsupported(
                "CutscenePlayerData.Restore: a running cutscene with CommandLockControls",
            );
        }
        // `Restore` resumes each started track at its saved `CommandIndex`; the saved track
        // positions aren't read, so T0 actions still ahead of them can't be run in place.
        if !cls.t0_steps.is_empty() {
            w.coverage.unsupported(
                "CutscenePlayerData.Restore: a running cutscene with T0 actions (saved track positions not resumed)",
            );
        }
    }
}

impl SceneState {
    #[must_use]
    pub fn zone_by_blueprint(&self, bp: Guid) -> Option<usize> {
        self.zones.iter().position(|z| z.blueprint == Some(bp))
    }
}

/// The scene settings of zone `i`, if the cache has them.
fn info(w: &World, i: usize) -> Option<ZoneInfo> {
    let id = &w.scene.zones[i].id;
    w.scene.cache.as_ref()?.zone(id).cloned()
}

/// `ScriptZone.IsInterestedInUnit`.
fn interested(w: &World, info: &ZoneInfo, u: EntityIdx) -> bool {
    if crate::combat::units::is_dead(w, u) {
        return false;
    }
    if info.players_only && !w.unit(u).is_some_and(|x| x.is_player_faction) {
        return false;
    }
    true
}

/// `UnitCommand.CommandTargetUntargetable(null, unit)`: the parts it reads that the sim has
/// no port for are reported.
fn untargetable(w: &mut World, u: EntityIdx) -> bool {
    for p in [
        "UnitPartInPit",
        "UnitPartSwallowed",
        "UnitPartInsideAnotherCreature",
        "UnitPartPersonalEnemy",
    ] {
        if w.entity(u).parts.iter().any(|x| x.clr == p) {
            w.coverage.unsupported(format!("UnitCommand.CommandTargetUntargetable: {p}"));
        }
    }
    // `Features.IsUntargetable` (`MechanicsFeatureType.IsUntargetable` = 43).
    w.unit(u).is_some_and(|x| x.mechanics_features.get(&43).copied().unwrap_or(0) > 0)
}

/// `ScriptZone.TryAddUnit`.
fn try_add_unit(w: &mut World, zi: usize, unit: EntityIdx, info: &ZoneInfo) {
    let zone_bp = w.scene.zones[zi].blueprint;
    let bp = zone_bp.and_then(|g| w.content.get(g));
    let ok = elements::with_ctx(w, Ctx::ScriptZoneUnit(unit), |w| {
        if let Some(bp) = &bp
            && !elements::check_conditions(w, bp, bp.view().obj("TriggerConditions"))
        {
            return false;
        }
        w.scene.zones[zi].inside.push(Inside { unit, inside_this_tick: true });
        w.events.push(
            "t1",
            json!({ "what": "script_zone_enter", "zone": zone_bp.map(|g| g.to_string()), "unit": crate::events::unit_ref(&w.entity(unit).unique_id) }),
        );
        // `OnUnitEnter`: the blueprint's `EnterActions`.
        if let Some(bp) = &bp {
            elements::run_actions(w, bp, bp.view().obj("EnterActions"));
        }
        // `OnUnitEntered.Invoke(unit, this)`: the scene's persistent calls.
        if info.unit_entered_calls > 0 {
            w.coverage.unsupported("ScriptZone.OnUnitEntered: scene UnityEvent calls");
        }
        w.scene.current.push(zi);
        crate::rulebook::bus::raise_global_call(
            w,
            "IScriptZoneHandler",
            "OnUnitEnteredScriptZone",
            Some(unit),
        );
        w.scene.current.pop();
        true
    });
    if ok && info.once_only {
        w.scene.zones[zi].is_active = false;
    }
}

/// `ScriptZone.TickUnit` for a unit that stands in the zone's shape (the oracle's T1 walk;
/// the sim places the unit by a `zone:` step, which asserts the position).
pub fn tick_unit_inside(w: &mut World, zi: usize, unit: EntityIdx) {
    let Some(info) = info(w, zi) else {
        w.coverage.unsupported("ScriptZone: scene settings unavailable (no scene cache)");
        return;
    };
    if !w.scene.zones[zi].is_active || !interested(w, &info, unit) {
        return;
    }
    let existing = w.scene.zones[zi].inside.iter().position(|i| i.unit == unit);
    if existing.is_some_and(|k| w.scene.zones[zi].inside[k].inside_this_tick) {
        return;
    }
    if untargetable(w, unit) {
        return;
    }
    match existing {
        Some(k) => w.scene.zones[zi].inside[k].inside_this_tick = true,
        None => try_add_unit(w, zi, unit, &info),
    }
}

/// The loaded area has parts (`BlueprintArea.HasParts`: a part reference that resolves).
#[must_use]
pub fn area_has_parts(w: &World) -> bool {
    w.player
        .area
        .and_then(|g| w.content.get(g))
        .is_some_and(|a| a.view().guids("m_Parts").into_iter().any(|p| w.content.get(p).is_some()))
}

/// `Game.Instance.State.Units.All`: the `EntityPool<UnitEntityData>` with the `NotFake`
/// filter (`PersistentState.Units`: a unit whose blueprint `IsFake` never joins it), in the
/// order the units joined their states (at the load the cross-scene state's, then the area's;
/// a unit that joins one later is appended); a unit removed from its state (disposed) leaves it.
#[must_use]
pub fn units_pool(w: &World) -> Vec<EntityIdx> {
    let mut out: Vec<EntityIdx> = Vec::new();
    let held =
        |u: &EntityIdx| w.player.cross_scene_units.contains(u) || w.player.area_units.contains(u);
    let loaded = w
        .player
        .cross_scene_units
        .iter()
        .chain(&w.player.area_units)
        .filter(|u| !w.player.joined_after_load.contains(u));
    let joined = w.player.joined_after_load.iter().filter(|u| held(u));
    for &u in loaded.chain(joined) {
        let Some(x) = w.unit(u) else { continue };
        let fake =
            x.blueprint.and_then(|g| w.content.get(g)).is_some_and(|b| b.view().bool("IsFake"));
        if fake || w.entity(u).is_disposed || out.contains(&u) {
            continue;
        }
        out.push(u);
    }
    out
}

/// `EntityDataBase.ShouldBeEnumeratedByEntityPoolEnumerator` (`m_IsInGame && !m_Suppressed`),
/// which `EntityPoolEnumerator.MoveNext` asks of each entity as it reaches it. `m_Suppressed`
/// isn't saved and only `SuppressEntitiesController` sets it, in an area with parts
/// (`HasParts`), which isn't modelled: there it is reported.
pub fn enumerated_by_pool(w: &mut World, u: EntityIdx) -> bool {
    if !w.entity(u).is_in_game {
        return false;
    }
    if area_has_parts(w) {
        w.coverage
            .unsupported("EntityPoolEnumerator: suppressed entities (area parts) not modelled");
    }
    true
}

/// `ScriptZone.OnAreaDidLoad`: for each active zone, `foreach unit in Game.State.Units`
/// (the live pool, [`units_pool`], each unit tested by [`enumerated_by_pool`] when the
/// enumerator reaches it), a unit that interests the zone, stands in one of its shapes and isn't
/// inside yet enters (`TryAddUnit`); a zone that turns itself off stops there. Without the
/// scene cache an active zone cannot be tested and is reported.
pub fn on_area_did_load(w: &mut World) {
    for zi in 0..w.scene.zones.len() {
        if !w.scene.zones[zi].is_active {
            continue;
        }
        let Some(info) = info(w, zi) else {
            w.coverage.unsupported(
                "ScriptZone.OnAreaDidLoad: scene settings unavailable (no scene cache)",
            );
            continue;
        };
        // `EntityPoolEnumerator` walks the pool's live list by index.
        let mut next = 0;
        loop {
            let pool = units_pool(w);
            let Some(&u) = pool.get(next) else { break };
            next += 1;
            if !enumerated_by_pool(w, u) {
                continue;
            }
            let pos = w.unit(u).map_or([0.0; 3], |x| x.position);
            // `IsInterestedInUnit(unit) && ContainsPosition(unit.Position) && !Data.IsInside(unit)`.
            if interested(w, &info, u) {
                let mut inside = false;
                for s in &info.shapes {
                    match s.contains(pos, 0.0) {
                        Some(true) => {
                            inside = true;
                            break;
                        }
                        Some(false) => {}
                        None => {
                            w.coverage.unsupported(format!("ScriptZone shape {}", s.kind));
                        }
                    }
                }
                if inside && !w.scene.zones[zi].inside.iter().any(|i| i.unit == u) {
                    // The zone runs its conditions, actions and `IScriptZoneHandler` here, among
                    // the other `IAreaHandler.OnAreaDidLoad` subscribers, whose order isn't
                    // modelled.
                    w.coverage.unsupported(
                        "ScriptZone.OnAreaDidLoad: a unit enters at load (order among the IAreaHandler subscribers)",
                    );
                    try_add_unit(w, zi, u, &info);
                }
            }
            if !w.scene.zones[zi].is_active {
                break;
            }
        }
    }
}

/// The `zone:` runner step: the main character enters the zone of blueprint `bp`.
pub fn enter_by_blueprint(w: &mut World, bp: Guid) {
    let Some(mc) = w.main_character else { return };
    enter_by_blueprint_unit(w, bp, mc);
}

/// [`enter_by_blueprint`] for any unit (one a cutscene moved, say). The entry is a recorded
/// `TryAddUnit`, which the game calls only for a unit not inside: one the sim has inside left
/// the zone in the game, and exits aren't modelled (reported).
pub fn enter_by_blueprint_unit(w: &mut World, bp: Guid, u: EntityIdx) {
    let Some(zi) = w.scene.zone_by_blueprint(bp) else {
        w.coverage.unsupported("ScriptZone: no zone of that blueprint in the area state");
        return;
    };
    if w.scene.zones[zi].inside.iter().any(|i| i.unit == u) {
        w.coverage
            .unsupported("ScriptZone: an entry of a unit already inside (exits aren't modelled)");
        return;
    }
    tick_unit_inside(w, zi, u);
}

/// A map of zone entities by unique id, for tests.
#[must_use]
pub fn zone_ids(w: &World) -> HashMap<String, usize> {
    w.scene.zones.iter().enumerate().map(|(i, z)| (z.id.clone(), i)).collect()
}
