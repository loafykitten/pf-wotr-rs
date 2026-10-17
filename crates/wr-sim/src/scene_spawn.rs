//! Unit spawners and the `Spawn` and `HideUnit` actions (decompiled, not copied:
//! `Kingmaker.Designers.EventConditionActionSystem.Actions/{Spawn,HideUnit}.cs`,
//! `Kingmaker.View.Spawners/{UnitSpawnerBase,UnitSpawner,SpawnerAdditionalActions,
//! UnitSpawningData}.cs`, `Kingmaker.UnitLogic.Interaction/{SpawnerInteractionPart,
//! SpawnerInteractionDialog}.cs`, `Kingmaker.Controllers/EntityCreationController.cs`
//! `SpawnUnit`/`SpawnEntityWithView`, `Kingmaker.EntitySystem.Entities/UnitEntityData.cs`
//! `OnIsInGameChanged`).
//!
//! A spawner's settings (blueprint, respawn, group, settings components) come from the
//! scene cache ([`crate::scene`]); its saved state is `UnitSpawnerBase.MyData` (spawned unit,
//! `HasSpawned`), kept in `World::player.spawners`. Spawner settings components the sim has no
//! port for make the spawn unsupported.

use std::rc::Rc;

use crate::content::{Blueprint, View};
use crate::elements::{Ctx, Result, get_unit, unsupported, with_ctx};
use crate::scene::ViewInfo;
use crate::world::{EntityIdx, EntityKind, World};

/// Spawner settings components with a port (their `IUnitInitializer` part, if any).
const PORTED_SETTINGS: &[&str] =
    &["Transform", "UnitSpawner", "SpawnerInteractionDialog", "SpawnerAdditionalActions"];

fn short(class: &str) -> &str {
    class.rsplit('.').next().unwrap_or(class)
}

fn guid_of(v: &serde_json::Value) -> Option<wr_content::Guid> {
    v["guid"].as_str().filter(|s| !s.is_empty()).and_then(wr_content::Guid::parse)
}

/// `cache/sim/unit-prefabs.json` (`wr-import unit-prefabs`, schema `wr-sim-unit-prefabs-v1`):
/// each unit prefab's `UnitEntityView.m_IsInGame` by asset GUID.
#[must_use]
pub fn load_unit_prefabs() -> Option<Rc<std::collections::HashMap<String, Option<bool>>>> {
    let path = wr_paths::cache_root().join("sim").join("unit-prefabs.json");
    let v: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    if v["schema"] != "wr-sim-unit-prefabs-v1" {
        return None;
    }
    let map = v["prefabs"].as_object()?.iter().map(|(k, x)| (k.clone(), x.as_bool())).collect();
    Some(Rc::new(map))
}

/// `view.IsInGameBySettings` of a unit view instantiated from the prefab with asset GUID
/// `asset` (`Object.Instantiate` copies the prefab's `m_IsInGame`): the value
/// `new UnitEntityData(view)` starts the unit with. Without the prefab table, or for a prefab
/// it has no single value for (a missing view, `OverrideLegacyUnits`), reported; the unit is
/// then built in game.
pub fn prefab_in_game(w: &mut World, asset: Option<&str>) -> bool {
    let value = asset
        .filter(|a| !a.is_empty())
        .and_then(|a| w.scene.unit_prefabs.as_ref().and_then(|t| t.get(a).copied().flatten()));
    match value {
        Some(v) => v,
        None => {
            w.coverage.unsupported(
                "UnitEntityData(view): the prefab's IsInGameBySettings is unknown (cache/sim/unit-prefabs.json)",
            );
            true
        }
    }
}

/// A `WeakResourceLink`'s `AssetId` (`UnitViewLink`: the link object with its `AssetId`, or
/// the id itself), when set.
fn weak_link_asset(n: &wr_content::node::Node) -> Option<String> {
    use wr_content::node::Node;
    let id: &str = match n {
        Node::WeakLink(Some(s)) | Node::WeakLinkLegacy(s, _) | Node::Str(s) => s,
        Node::Object(o) => View(o).str("AssetId")?,
        _ => return None,
    };
    Some(id.to_owned()).filter(|s| !s.is_empty())
}

/// `BlueprintUnit.Prefab.AssetId`.
#[must_use]
pub fn blueprint_prefab(bp: &Blueprint) -> Option<String> {
    weak_link_asset(bp.view().node("Prefab")?)
}

/// `BlueprintRoot.Instance.OptimizationDummyUnit.AssetId`.
#[must_use]
pub fn dummy_prefab(w: &World) -> Option<String> {
    let root = w.content.get(crate::unit::BLUEPRINT_ROOT)?;
    weak_link_asset(root.view().node("OptimizationDummyUnit")?)
}

/// `Spawn.RunAction()`.
pub fn spawn_action(w: &mut World, owner: &Rc<Blueprint>, a: View<'_>) -> Result<()> {
    let respawn = a.bool("RespawnIfDead");
    for s in a.objs("Spawners") {
        let sv = View(s);
        // `EntityReference.FindView()`: the view with that `UniqueId`.
        let Some(id) = sv.str("UniqueId").or_else(|| sv.str("_entity_id")) else { continue };
        let id = id.to_owned();
        let view = w.scene.cache.as_ref().and_then(|c| c.view(&id)).cloned();
        let Some(view) = view else {
            if w.scene.cache.is_none() {
                return Err(unsupported(
                    w,
                    "action",
                    "Spawn (no scene cache: spawner views unknown)",
                ));
            }
            continue;
        };
        if !view.class.ends_with(".UnitSpawner") {
            return Err(unsupported(w, "action", "Spawn (spawner class other than UnitSpawner)"));
        }
        // `TrySpawnOrRespawn()` / `Spawn()`.
        let unit = if respawn {
            let st = w.player.spawners.get(&id).cloned();
            let has = st.as_ref().is_some_and(|s| s.has_spawned);
            if has {
                // `SpawnedUnitHasDied`, then clearing and respawning.
                return Err(unsupported(w, "action", "Spawn (respawn of a spawned unit)"));
            }
            spawn(w, &id, &view)
        } else {
            spawn(w, &id, &view)
        };
        if let Some(u) = unit {
            // `ContextData<SpawnedUnitData>` around `ActionsOnSpawn.Run()`.
            with_ctx(w, Ctx::SpawnedUnit(u), |w| {
                crate::elements::run_actions(w, owner, a.obj("ActionsOnSpawn"));
            });
        }
    }
    Ok(())
}

/// `UnitSpawnerBase.Spawn()` for a `UnitSpawner`.
fn spawn(w: &mut World, id: &str, view: &ViewInfo) -> Option<EntityIdx> {
    let has_spawned = w.player.spawners.get(id).is_some_and(|s| s.has_spawned);
    if has_spawned {
        w.log("warning", format!("Trying to use spawner {id} twice."));
        return None;
    }
    let comp = |n: &str| view.components.iter().find(|c| short(&c.class) == n);
    let Some(spawner) = comp("UnitSpawner") else {
        w.coverage.unsupported("Spawn: spawner view without a UnitSpawner component");
        return None;
    };
    let Some(bp) = guid_of(&spawner.fields["m_Blueprint"]).and_then(|g| w.content.get(g)) else {
        w.log("error", "UnitSpawnerBase.Spawn: unit blueprint is null!");
        return None;
    };
    // The settings components: restrictions and initializers.
    for c in &view.components {
        let n = short(&c.class);
        if !PORTED_SETTINGS.contains(&n) {
            w.coverage.unsupported(format!("UnitSpawner: settings component {n} not ported"));
            return None;
        }
    }
    // `UnitSpawner.SpawnUnit`: a group disabled on simplified marks the spawner as done.
    if view.group.as_ref().is_some_and(|g| g.1) {
        set_spawner(w, id, None, true);
        return None;
    }
    // `EntityCreator.SpawnUnit`.
    if bp.view().guid("m_CustomizationPreset").is_some() {
        w.coverage.unsupported("SpawnUnit: customization preset");
    }
    let uid = crate::narrative::Player::new_unique_id(w);
    let e = w.add_entity(crate::world::Entity {
        unique_id: uid,
        clr: "UnitEntityData",
        kind: EntityKind::Unit(Box::default()),
        facts: crate::world::FactsManager::default(),
        parts: Vec::new(),
        is_turned_on: false,
        post_load_executed: false,
        is_in_game: true,
        subscribed: false,
        disposing: false,
        is_disposed: false,
    });
    let outer_ctx = w.coverage.context.take();
    w.coverage.context = Some(w.entity(e).unique_id.clone());
    // `new UnitEntityData(view)` with a default `UnitSpawningData`: the view is the
    // blueprint's prefab, or the optimization dummy when the spawner (`transform.position`, in
    // the area's state) is outside the loaded area part's mechanic bounds
    // (`ShouldSpawnOptimizedDummy`, `area_parts`; reported when they aren't known).
    let outside = match crate::area_parts::outside_mechanic_bounds(w, view.position) {
        Ok(o) => o,
        Err(_) => {
            w.coverage.unsupported(
                "EntityCreationController.ShouldSpawnOptimizedDummy: area part bounds",
            );
            false
        }
    };
    let prefab = if outside { dummy_prefab(w) } else { blueprint_prefab(&bp) };
    let in_game = prefab_in_game(w, prefab.as_deref());
    let outer = std::mem::take(&mut w.build);
    crate::spawn::construct(w, e, &bp, None, in_game);
    w.build = outer;
    if let Some(u) = w.unit_mut(e) {
        u.view_prefab = prefab;
    }
    let yaw = quat_yaw_degrees(view.rotation);
    if let Some(u) = w.unit_mut(e) {
        u.position = view.position;
        u.orientation = yaw;
    }
    // `EntityCreator.SpawnUnit`: `unitEntityData.IsInGame = !current.SpawnHidden` (the
    // setter: a change raises `IInGameHandler` and `OnIsInGameChanged`).
    crate::entity::set_is_in_game(w, e, true);
    // `unitEntityData.GroupId = m_GroupId`.
    if let Some((g, _)) = &view.group
        && let Some(u) = w.unit_mut(e)
    {
        u.group_id = Some(g.clone());
    }
    // `AddEntity(entity, state)`: the creation queue (the spawner's scene state: the area's).
    w.player.creation_queue.push((e, false));
    w.coverage.context = outer_ctx;
    // `Data.SpawnedUnit = unit`: `OnSpawned` → `ApplyOnSpawn` (the spawner's initializer parts,
    // in saved order), then `HasSpawned`.
    set_spawner(w, id, Some(e), false);
    apply_on_spawn(w, id, view, e);
    set_spawner(w, id, Some(e), true);
    Some(e)
}

fn set_spawner(w: &mut World, id: &str, unit: Option<EntityIdx>, has_spawned: bool) {
    let uid = unit.map(|u| w.entity(u).unique_id.clone());
    let s = w
        .player
        .spawners
        .entry(id.to_owned())
        .or_insert(crate::narrative::Spawner { in_game: true, ..Default::default() });
    if uid.is_some() {
        s.spawned_unit = uid;
    }
    s.has_spawned = has_spawned;
}

/// `MyData.ApplyOnSpawn()`: `OnSpawn` then `OnInitialize` of each `IUnitInitializer` part.
fn apply_on_spawn(w: &mut World, id: &str, view: &ViewInfo, unit: EntityIdx) {
    for c in &view.components {
        match short(&c.class) {
            // `SpawnerInteractionPart.OnInitialize`: `unit.Ensure<UnitPartInteractions>()
            // .AddInteraction(wrapper)`; the interaction fires when someone talks to the unit.
            "SpawnerInteractionDialog" => {
                if !w.entity(unit).parts.iter().any(|p| p.clr == "UnitPartInteractions") {
                    w.entity_mut(unit)
                        .parts
                        .push(crate::world::Part { clr: "UnitPartInteractions" });
                }
                w.scene.interactions.push((unit, id.to_owned(), guid_of(&c.fields["m_Dialog"])));
            }
            // `SpawnerAdditionalActions.Part.OnSpawn`: `SpawnActions` in a `SpawnedUnitData`
            // scope.
            "SpawnerAdditionalActions" => {
                if let Some(a) = guid_of(&c.fields["SpawnActions"]).and_then(|g| w.content.get(g)) {
                    with_ctx(w, Ctx::SpawnedUnit(unit), |w| {
                        crate::elements::run_actions(w, &a, a.view().obj("Actions"));
                    });
                }
            }
            _ => {}
        }
    }
}

/// `Quaternion.eulerAngles.y` of a rotation about the vertical axis, in degrees.
fn quat_yaw_degrees(q: [f32; 4]) -> f32 {
    let [x, y, z, wq] = q;
    let yaw = (2.0 * (wq * y + x * z)).atan2(1.0 - 2.0 * (y * y + z * z)).to_degrees();
    if yaw < 0.0 { yaw + 360.0 } else { yaw }
}

/// `HideUnit.RunAction()`.
/// `TranslocateUnit.RunAction()`: with a position (a locator or an evaluator) the unit stops
/// being an extra (`MarkNoExtra`, saved; it changes the unit's sleeping), then stops and moves
/// there (positions and movement are T1).
pub fn translocate_unit(w: &mut World, owner: &Rc<Blueprint>, a: View<'_>) -> Result<()> {
    let u = get_unit(w, owner, a.obj("Unit"))?;
    if a.obj("translocatePosition").is_none() && a.obj("translocatePositionEvaluator").is_none() {
        return Ok(());
    }
    if let Some(x) = w.unit_mut(u) {
        x.is_extra = false;
    }
    w.coverage.stub("TranslocateUnit: stop, interrupt the move, the new position (T1)");
    Ok(())
}

pub fn hide_unit(w: &mut World, owner: &Rc<Blueprint>, a: View<'_>) -> Result<()> {
    // Run from a cutscene's `CommandAction` (the closing cutscene of the Horgus meeting) the
    // action is checked against the oracle. Run by an etude trigger at load it is not yet: on
    // Manual_10 the sim's 25 etude `HideUnit`s hide units that the oracle shows in game (the
    // etudes that would unhide them, `AnotherEtudeOfGroupIsPlaying` and the other conditions
    // and actions they use, are not ported), and 17 units' sheets get worse. It stays
    // reported there.
    if !w.scene.in_cutscene_step {
        return Err(unsupported(w, "action", "HideUnit"));
    }
    let u = get_unit(w, owner, a.obj("Target"))?;
    if a.bool("Unhide") {
        if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartPet") {
            return Err(unsupported(w, "action", "HideUnit (unhide a pet)"));
        }
        crate::entity::set_is_in_game(w, u, true);
        return Ok(());
    }
    if a.bool("Fade") {
        // `value.Commands.InterruptAll(); view.FadeHide();` (commands and fading are T1).
        w.coverage.stub("HideUnit: interrupt commands, fade out (T1)");
        // (The view's `m_IsFadeHide` keeps its sphere collider as it was until the fade ends.)
        w.combat.vision.fade_hidden.insert(u);
    }
    crate::entity::set_is_in_game(w, u, false);
    if a.bool("SetHideInSaves") && w.entity(u).parts.iter().any(|p| p.clr == "UnitPartCompanion") {
        return Err(unsupported(w, "action", "HideUnit (IsIgnoreFixInGame of a companion)"));
    }
    Ok(())
}
