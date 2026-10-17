//! The area's other scene entities (`SceneEntitiesState.m_EntityData` besides units,
//! placeholders and spawners): script zones (`ScriptZone+ScriptZoneData`), map objects
//! (`MapObjectEntityData`), cutscene players (`CutscenePlayerData`), unit groups
//! (`UnitGroupView+UnitGroupData`), locators (`LocatorView+EntityData`) and reveal triggers
//! (`FowRevealerTrigger+EntityData`). Each is an `EntityDataBase`: saved id, `m_IsInGame`,
//! parts and facts (the map object's `FactHolder.Fact`), restored by `PrePostLoad`,
//! post-loaded and turned on by the scene state as for units.

use super::Loader;
use super::graph::{O, guid_of, int_of};
use super::parts_of;
use crate::world::{Entity, EntityIdx, EntityKind, FactsManager};
use wr_save::value::Value;

/// The entity kinds hydrated here, by the end of their CLR name.
const KINDS: &[&str] = &[
    "ScriptZone+ScriptZoneData",
    "Entities.MapObjectEntityData",
    "Cutscenes.CutscenePlayerData",
    "UnitGroupView+UnitGroupData",
    "LocatorView+EntityData",
    "FowRevealerTrigger+EntityData",
    "Roaming.RoamingWaypointData",
];

impl Loader<'_> {
    /// An `EntityDataBase` of one of [`KINDS`] as an entity of kind `Other`; `None` for a
    /// type this module doesn't model.
    pub(super) fn static_entity(&mut self, o: O<'_>) -> Option<EntityIdx> {
        let clr = o.clr();
        let kind = KINDS.iter().find(|k| clr.ends_with(*k))?;
        let e = self.w.add_entity(Entity {
            unique_id: o.str("UniqueId").unwrap_or_default().to_owned(),
            clr: o.ty(),
            kind: EntityKind::Other,
            facts: FactsManager::default(),
            parts: parts_of(o),
            is_turned_on: false,
            post_load_executed: false,
            is_in_game: o.bool_or("m_IsInGame", true),
            subscribed: false,
            disposing: false,
            is_disposed: false,
        });
        let facts = o.obj("Facts").map(|f| f.objs("m_Facts")).unwrap_or_default();
        let list: Vec<_> = facts.into_iter().map(|f| self.fact_for(f)).collect();
        let cap = crate::csharp::sort::grown_capacity(list.len());
        let blueprint = list.first().and_then(|f| self.w.fact(*f).blueprint);
        self.w.entity_mut(e).facts = FactsManager { list, capacity: cap, is_turned_on: false };
        if kind.ends_with("MapObjectEntityData") || kind.ends_with("ScriptZoneData") {
            let m = self.map_object(o);
            self.w.scene.map_objects.insert(e, m);
        }
        if kind.ends_with("CutscenePlayerData") {
            let params = o
                .obj("Parameters")
                .map(|p| {
                    p.dict("Params")
                        .into_iter()
                        .map(|(k, v)| {
                            let key = match k {
                                Value::Str(s) => s.to_string(),
                                _ => String::new(),
                            };
                            let val = match v {
                                Value::Str(s) => s.to_string(),
                                Value::Null => String::new(),
                                other => format!("{other:?}"),
                            };
                            (key, val)
                        })
                        .collect()
                })
                .unwrap_or_default();
            self.w.scene.cutscenes.push(crate::scene::CutscenePlayer {
                entity: e,
                cutscene: o.guid("m_Cutscene"),
                params,
                is_queued: o.bool_or("m_IsQueued", false),
                queued_after: o.obj("m_QueuedAfter").is_some(),
                restart: o.bool_or("m_Restart", false),
                remove: o.bool_or("m_Remove", false),
                activated_gates: o
                    .get("m_ActivatedGates")
                    .map_or(0, |v| if let Value::Seq(s) = v { s.len() } else { 0 }),
            });
        }
        if kind.ends_with("ScriptZoneData") {
            self.w.scene.zones.push(crate::scene::Zone {
                entity: e,
                id: self.w.entity(e).unique_id.clone(),
                blueprint,
                is_active: o.bool_or("IsActive", true),
                inside: Vec::new(),
            });
        }
        Some(e)
    }
}

impl Loader<'_> {
    /// `MapObjectEntityData`/`StaticEntityData`'s saved members and the parts the sim keeps
    /// state of.
    fn map_object(&mut self, o: O<'_>) -> crate::scene_parts::MapObj {
        let mut m = crate::scene_parts::MapObj {
            was_highlighted_on_reveal: o.bool_or("WasHighlightedOnReveal", false),
            perception_check_dc: o.int("PerceptionCheckDC").and_then(|v| i32::try_from(v).ok()),
            ..Default::default()
        };
        for p in o.obj("Parts").map(|p| p.objs("m_Parts")).unwrap_or_default() {
            match p.ty() {
                "InteractionLootPart" => {
                    let loot = p.obj("m_Loot").map(|c| crate::scene_parts::LootColl {
                        id: c.id(),
                        items: c.objs("m_Items").into_iter().map(|i| self.item_for(i)).collect(),
                    });
                    let known = match p.get("m_KnownItems") {
                        Some(Value::Null) | None => None,
                        _ => Some(
                            p.dict("m_KnownItems")
                                .into_iter()
                                .filter_map(|(k, v)| {
                                    Some((guid_of(k)?, i32::try_from(int_of(v)?).ok()?))
                                })
                                .collect(),
                        ),
                    };
                    m.loot = Some(crate::scene_parts::LootPart {
                        enabled: p.bool_or("m_Enabled", true),
                        loot,
                        stash: None,
                        known_items: known,
                    });
                }
                "LocalMapMarkerPart" => m.marker_hidden = Some(p.bool_or("Hidden", false)),
                "InteractionSkillCheckPart" => {
                    m.skill_check = Some(crate::scene_parts::SkillCheckPart {
                        dc_override: p.i32_or("DCOverride", 0),
                        skill_override: p.i32_or("SkillOverride", 0),
                    });
                }
                _ => {}
            }
        }
        m
    }

    /// `SceneLoader.LoadSceneEntities(state, views of the state's scene)`, both directions:
    /// - each saved entity attaches to the view of its scene with its `UniqueId`
    ///   (`AttachToViewOnLoad`); a scene entity without one gets no view (`CreateViewForData`
    ///   is null for them), leaves the game and is removed from the state (`NeedsView`);
    /// - each active view of the scene whose class `CreatesDataOnLoad` and that no saved
    ///   entity matched gets new data (`CreateDataForView(state, view, load: true)`).
    ///
    /// Units, placeholders and cutscene players make their own views. The sim has the
    /// scene's views only from the scene cache: without it, with a saved entity the scene
    /// doesn't hold, or with a view that would get new data, the step is reported.
    pub(super) fn check_scene_views(&mut self, states: &[super::SceneState]) {
        check_scene_views(&mut self.w, states);
    }
}

/// [`Loader::check_scene_views`] on a world.
pub(super) fn check_scene_views(w: &mut crate::world::World, states: &[super::SceneState]) {
    let cache = w.scene.cache.clone();
    let Some(cache) = cache else {
        let any = states.iter().any(|st| st.saved.iter().any(|(_, clr)| needs_scene_view(clr)));
        if any {
            w.coverage.unsupported(
                "SceneLoader: no scene cache for the area (views, script zone settings and shapes)",
            );
        }
        return;
    };
    for st in states {
        for (id, clr) in &st.saved {
            if !needs_scene_view(clr) {
                continue;
            }
            let found = cache.view(id).is_some_and(|v| v.scene == st.scene);
            if !found {
                let short = clr.rsplit(['.', '+']).next().unwrap_or(clr);
                w.coverage.unsupported(format!(
                    "SceneLoader.LoadSceneEntities: saved {short} has no view in its scene (removed)"
                ));
            }
        }
        for v in &cache.views {
            if v.scene != st.scene || !v.active_in_hierarchy {
                continue;
            }
            let class = v.class.rsplit('.').next().unwrap_or(&v.class);
            if creates_data_on_load(class) && !st.saved.iter().any(|(id, _)| *id == v.unique_id) {
                w.coverage.unsupported(format!(
                    "SceneLoader.LoadSceneEntities: CreateDataForView for a view without saved data ({class})"
                ));
            }
        }
    }
}

/// A saved scene entity that must find its view in the scene (the kinds this module hydrates
/// but cutscene players, and spawners' data); units, placeholders and cutscene players make
/// their own views or need none.
fn needs_scene_view(clr: &str) -> bool {
    (KINDS.iter().any(|k| clr.ends_with(k)) && !clr.ends_with("Cutscenes.CutscenePlayerData"))
        || clr.contains("UnitSpawnerBase+MyData")
        || clr.contains("CompanionSpawner+MyData")
}

/// `EntityViewBase.CreatesDataOnLoad` by view class (`false` in the base; `true` in
/// `MapObjectView` but `DynamicMapObjectView`, `UnitSpawnerBase`, `UnitGroupView`,
/// `LocatorView`, `DungeonLocatorView`, `RoamingWaypointView`, `RegionEventLocation`,
/// `DirectorAdapter`, `CutscenePath` and their subclasses).
fn creates_data_on_load(class: &str) -> bool {
    matches!(
        class,
        "MapObjectView"
            | "ScriptZone"
            | "BuildingSlot"
            | "CampPlaceView"
            | "DroppedLoot"
            | "TrapObjectView"
            | "SimpleTrapObjectView"
            | "DetailedTrapObjectView"
            | "UnitSpawner"
            | "CompanionSpawner"
            | "UnitGroupView"
            | "LocatorView"
            | "RoamingWaypointView"
            | "RegionEventLocation"
            | "DirectorAdapter"
            | "CutscenePath"
            | "DungeonLocatorView"
            | "DungeonLocatorRewardView"
            | "DungeonLocatorTrapView"
            | "DungeonLocatorObjectView"
            | "DungeonLocatorLootView"
            | "DungeonLocatorEnterView"
            | "DungeonLocatorUnitView"
            | "DungeonLocatorMindControlView"
            | "DungeonLocatorExitView"
    )
}
