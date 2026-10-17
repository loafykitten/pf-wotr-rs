//! Area parts and the entities they suppress (decompiled, not copied:
//! `Kingmaker.Blueprints.Area/{AreaPartBounds,AreaService,BlueprintArea,BlueprintAreaPart}.cs`,
//! `Kingmaker.EntitySystem.Persistence.Scenes/SceneLoader.cs` (`LoadAreaCoroutine`),
//! `Kingmaker.Controllers/{SuppressEntitiesController,EntityCreationController,
//! SleepingUnitsController}.cs`, `Owlcat.Runtime.Core.Utils/GeometryUtils.cs`).
//!
//! An area with parts (`BlueprintArea.HasParts`) loads one of them as its current part
//! (`SceneLoader.CurrentlyLoadedAreaPart`, which `AreaService.CurrentAreaPart` answers). Each
//! part (the area itself is one) names an `AreaPartBounds` asset; its `MechanicBounds` decide
//! which entities are suppressed (`SuppressEntitiesController`: outside them in x and z, and
//! not in the cross-scene state) and which units spawn with the optimization dummy view
//! (`EntityCreationController.ShouldSpawnOptimizedDummy`: outside them in 3D). The assets
//! aren't blueprints: the runtime pack holds a part's `m_Bounds` as an index into
//! `BlueprintReferencedAssets`, and `tools/wr-rules-data` extracts every `AreaPartBounds` it
//! lists into `cache/sim/area-bounds.json` ([`AreaBoundsTable`]).
//!
//! `m_Suppressed` isn't saved, and only `SuppressEntitiesController` sets it: nothing is
//! suppressed during the load. Its first tick (the first controller of every game mode's
//! frame, with `m_UpdateAll` set by `Activate`) evaluates every suppressible entity, so at
//! the first tick after the load an entity is suppressed exactly when [`suppressed_at_load`]
//! says so. Later frames re-evaluate only entities that moved or joined a state, at the
//! frame's start; the sim doesn't follow that and reports it.

use std::collections::BTreeMap;
use std::rc::Rc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use wr_content::{Guid, Node};
use wr_nav::cs::{V3, d};

use crate::world::{EntityIdx, World};

pub const AREA_BOUNDS_SCHEMA: &str = "wr-area-bounds-v1";

/// A Unity `Bounds` as serialized (`m_Center`, `m_Extent`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub center: V3,
    pub extents: V3,
}

impl Bounds {
    /// `Bounds.Contains(point)` (native, `wr_nav::update::Bounds::contains`, checked against
    /// the player).
    #[must_use]
    pub fn contains(&self, p: V3) -> bool {
        wr_nav::update::Bounds { center: self.center, extents: self.extents }.contains(p)
    }

    /// `GeometryUtils.ContainsXZ(bounds, point)`: `vector = point - center` (stored), then
    /// `-e.x <= v.x + 0.1f && e.x >= v.x - 0.1f && -e.z <= v.z + 0.1f && e.z >= v.z - 0.1f`,
    /// each side in Mono's double evaluation.
    #[must_use]
    pub fn contains_xz(&self, p: V3) -> bool {
        let v = wr_nav::cs::sub(p, self.center);
        let e = self.extents;
        let tenth = d(0.1f32);
        (0.0 - d(e[0])) <= d(v[0]) + tenth
            && d(e[0]) >= d(v[0]) - tenth
            && (0.0 - d(e[2])) <= d(v[2]) + tenth
            && d(e[2]) >= d(v[2]) - tenth
    }

    /// `GeometryUtils.ManhattanDistance2D(point, bounds)`: `Mathf.Max(|p.x - c.x| - size.x / 2f,
    /// |p.z - c.z| - size.z / 2f)` (`size` is `m_Extents * 2f`, stored; `Mathf.Abs` takes and
    /// returns a `float`; each difference is stored in a local).
    #[must_use]
    pub fn manhattan_distance_2d(&self, p: V3) -> f32 {
        let axis = |i: usize| {
            let diff = (p[i] - self.center[i]).abs();
            let size = self.extents[i] * 2.0;
            wr_nav::cs::r(d(diff) - d(size) / 2.0)
        };
        let (a, b) = (axis(0), axis(2));
        // `Mathf.Max(a, b)`: `a > b ? a : b`.
        if a > b { a } else { b }
    }
}

/// An `AreaPartBounds` asset: the fields the mechanics read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AreaPartBoundsAsset {
    /// `m_Name`.
    pub name: String,
    /// Where it is: the serialized file's object.
    pub path_id: i64,
    pub default_bounds: Bounds,
    pub override_mechanic_bounds: bool,
    pub mechanic_bounds: Bounds,
    pub override_local_map_bounds: bool,
    pub local_map_bounds: Bounds,
    /// `m_OverrideFogOfWarBounds` and `m_FogOfWarBounds` (`None`: a table extracted before these
    /// were read: rebuild it with `wr-rules-data`).
    #[serde(default)]
    pub fog_of_war: Option<FogOfWarBoundsAsset>,
}

/// An `AreaPartBounds`' fog-of-war bounds (`SceneLoader.SetFogOfWarParameters` gives them to the
/// area's `FogOfWarArea`, whose bounds `LineOfSightGeometry.Init` takes).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FogOfWarBoundsAsset {
    pub override_bounds: bool,
    pub bounds: Bounds,
}

impl AreaPartBoundsAsset {
    /// `MechanicBounds`: the override, else `m_DefaultBounds`.
    #[must_use]
    pub fn mechanic(&self) -> Bounds {
        if self.override_mechanic_bounds { self.mechanic_bounds } else { self.default_bounds }
    }

    /// `LocalMapBounds`: the override, else `m_DefaultBounds`.
    #[must_use]
    pub fn local_map(&self) -> Bounds {
        if self.override_local_map_bounds { self.local_map_bounds } else { self.default_bounds }
    }

    /// `FogOfWarBounds`: the override, else `m_DefaultBounds` (`None`: not extracted).
    #[must_use]
    pub fn fog_of_war_bounds(&self) -> Option<Bounds> {
        let f = self.fog_of_war?;
        Some(if f.override_bounds { f.bounds } else { self.default_bounds })
    }
}

/// What `tools/wr-rules-data` extracts: every `AreaPartBounds` that
/// `BlueprintReferencedAssets.m_Entries` lists, by its index there (a part's `m_Bounds` in the
/// runtime pack).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AreaBoundsTable {
    pub schema: String,
    pub source: BTreeMap<String, serde_json::Value>,
    pub bounds: BTreeMap<i32, AreaPartBoundsAsset>,
}

impl AreaBoundsTable {
    /// `cache/sim/area-bounds.json`.
    #[must_use]
    pub fn default_path() -> std::path::PathBuf {
        wr_paths::cache_root().join("sim/area-bounds.json")
    }

    /// Loads the extracted table.
    ///
    /// # Errors
    ///
    /// If it's missing (run `wr-rules-data`), unreadable or another schema.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let b = std::fs::read(path)
            .with_context(|| format!("{} (run wr-rules-data)", path.display()))?;
        let t: Self = serde_json::from_slice(&b).context("area-bounds.json")?;
        anyhow::ensure!(t.schema == AREA_BOUNDS_SCHEMA, "area-bounds.json: schema {}", t.schema);
        Ok(t)
    }

    /// The default table, `None` when it wasn't extracted.
    #[must_use]
    pub fn load_default() -> Option<Rc<Self>> {
        Self::load(&Self::default_path()).ok().map(Rc::new)
    }
}

/// The loaded area's part, as the load establishes it ([`establish`]).
#[derive(Debug, Clone, Default)]
pub enum LoadedPart {
    /// Not established yet (no area loaded through [`establish`]).
    #[default]
    Unset,
    /// `CurrentlyLoadedAreaPart`, and its `Bounds?.MechanicBounds` (`None`: no bounds asset).
    Known { part: Guid, mechanic: Option<Bounds> },
    /// The sim can't establish it (why).
    Unknown(String),
}

/// `BlueprintAreaPart.Bounds` (`m_Bounds`): `Ok(None)` for no asset, the extracted asset, or
/// why it can't be read (a zip-sourced reference, an index the table doesn't hold, no table).
pub fn part_bounds(w: &World, part: Guid) -> Result<Option<AreaPartBoundsAsset>, String> {
    let bp = w.content.get(part).ok_or_else(|| format!("area part {part} missing"))?;
    match bp.view().node("m_Bounds") {
        None | Some(Node::Null | Node::Absent | Node::Asset(None)) => Ok(None),
        Some(Node::AssetIndex(i)) if *i < 0 => Ok(None),
        Some(Node::AssetIndex(i)) => {
            let t =
                w.scene.area_bounds.as_ref().ok_or("no area bounds table (run wr-rules-data)")?;
            t.bounds
                .get(i)
                .cloned()
                .map(Some)
                .ok_or_else(|| format!("referenced asset {i} isn't an extracted AreaPartBounds"))
        }
        Some(other) => Err(format!("m_Bounds of {part} is {other:?}")),
    }
}

/// `AreaService.FindLocalMapBoundsContainsPoint(area, point, fallbackToClosest: true)`: the
/// area if its own bounds' `LocalMapBounds` contain the point in x and z, else the first part
/// whose do, else the closest of the area and the parts (`ManhattanDistance2D`, the first of
/// equals). The area's bounds are dereferenced unconditionally on the fallback, as the C#
/// does: a missing asset there is a null reference, reported.
fn find_local_map_bounds_contains_point(w: &World, area: Guid, p: V3) -> Result<Guid, String> {
    let own = part_bounds(w, area)?;
    if let Some(b) = &own
        && b.local_map().contains_xz(p)
    {
        return Ok(area);
    }
    let own =
        own.ok_or("FindLocalMapBoundsContainsPoint: the area has no bounds (null reference)")?;
    let (mut best, mut dist) = (area, own.local_map().manhattan_distance_2d(p));
    let parts = w.content.get(area).map(|a| a.view().guids("m_Parts")).unwrap_or_default();
    for g in parts {
        // `part?.Get()?.Bounds`: a missing blueprint or asset is skipped.
        if w.content.get(g).is_none() {
            continue;
        }
        let Some(b) = part_bounds(w, g)? else { continue };
        if b.local_map().contains_xz(p) {
            return Ok(g);
        }
        let d2 = b.local_map().manhattan_distance_2d(p);
        if d2 < dist {
            dist = d2;
            best = g;
        }
    }
    Ok(best)
}

/// `SceneLoader.LoadAreaCoroutine` loading a save (no area loaded before it, no enter point):
/// the save's area part (`SaveInfo.AreaPart`, the player's `CurrentAreaPart`), or the area,
/// then corrected to the part whose local map holds the main character
/// ([`find_local_map_bounds_contains_point`]; the first party character's position when the
/// main character is detached from the party). Only in an area with parts; the result is kept
/// in `w.scene.loaded_part`. A correction away from the saved part is reported: the sim's other
/// readers of the current part (etudes) use the saved one.
pub fn establish(w: &mut World) {
    w.scene.loaded_part = LoadedPart::Unset;
    let Some(area) = w.player.area else { return };
    if !crate::scene::area_has_parts(w) {
        return;
    }
    let result = loaded_part(w, area);
    w.scene.loaded_part = match result {
        Ok(part) => {
            if Some(part) != w.player.area_part.or(Some(area)) {
                w.coverage.unsupported(
                    "SceneLoader.LoadAreaCoroutine: the loaded area part differs from the saved one",
                );
            }
            match part_bounds(w, part) {
                Ok(b) => LoadedPart::Known { part, mechanic: b.map(|b| b.mechanic()) },
                Err(e) => LoadedPart::Unknown(e),
            }
        }
        Err(e) => LoadedPart::Unknown(e),
    };
}

fn loaded_part(w: &World, area: Guid) -> Result<Guid, String> {
    let mc = w.main_character.ok_or("no main character")?;
    // `UnitPartCompanion.State == CompanionState.InPartyDetached` (4).
    if w.unit(mc).and_then(|x| x.companion_state) == Some(4) {
        return Err(
            "the main character is detached: PartyCharacters.FirstItem() not modelled".into()
        );
    }
    let pos = w.unit(mc).ok_or("main character")?.position;
    find_local_map_bounds_contains_point(w, area, pos)
}

/// The loaded part's `MechanicBounds`: `Ok(None)` when the area has no parts or the part no
/// bounds asset (nothing is suppressed), or why it isn't known.
pub fn mechanic_bounds(w: &World) -> Result<Option<Bounds>, String> {
    if !crate::scene::area_has_parts(w) {
        return Ok(None);
    }
    match &w.scene.loaded_part {
        LoadedPart::Known { mechanic, .. } => Ok(*mechanic),
        LoadedPart::Unknown(e) => Err(e.clone()),
        LoadedPart::Unset => Err("the loaded area part wasn't established".into()),
    }
}

/// `SuppressEntitiesController.UpdateEntity` at the first tick after the load (every
/// suppressible entity, `m_UpdateAll`): `HoldingState != CrossSceneState && bounds.HasValue &&
/// !bounds.ContainsXZ(Position)`. A unit outside any state isn't updated (it stays
/// unsuppressed). `Err`: the bounds aren't known.
pub fn suppressed_at_load(w: &World, u: EntityIdx) -> Result<bool, String> {
    let Some(b) = mechanic_bounds(w)? else { return Ok(false) };
    if w.player.cross_scene_units.contains(&u) || !w.player.area_units.contains(&u) {
        return Ok(false);
    }
    let x = w.unit(u).ok_or("not a unit")?;
    Ok(!b.contains_xz(x.position))
}

/// `EntityCreationController.ShouldSpawnOptimizedDummy`'s area-part clause for a unit spawned
/// into the area's state at `position`: `HasParts` and the current part's `MechanicBounds`
/// don't contain it (in 3D). `Err`: the bounds aren't known.
pub fn outside_mechanic_bounds(w: &World, position: V3) -> Result<bool, String> {
    Ok(mechanic_bounds(w)?.is_some_and(|b| !b.contains(position)))
}
