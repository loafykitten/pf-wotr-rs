//! Companion spawners, the spawner activation at load, and the capital mode (decompiled, not
//! copied: `Kingmaker.View.Spawners/{CompanionSpawner,UnitSpawnerBase}.cs`,
//! `Kingmaker.EntitySystem.Persistence.Scenes/SceneLoader.cs` `ProcessSpawnerActivationCoroutine`,
//! `Kingmaker.EntitySystem/RuntimeAreaSettings.cs` `SetCapitalMode`, `Kingmaker.Utility/
//! CountingGuard.cs`, `Kingmaker.UnitLogic.Parts/UnitPartCompanion.cs`,
//! `Kingmaker.UnitLogic.Interaction/SpawnerInteractionPart.cs`, `Kingmaker.View.Spawners/
//! {SpawnerAdditionalActions,SpawnerCustomCutscene}.cs`).
//!
//! A `CompanionSpawner` is a scene view (its settings in the scene cache, [`crate::scene`])
//! with saved data, `CompanionSpawner.MyData` (`m_SpawnedUnit`, `HasSpawned`, `DidSpawnOnce`,
//! the parts its `IUnitInitializer` components made), kept with the unit spawners' in
//! [`crate::narrative::Player::spawners`]. A companion names its current spawner in
//! `UnitPartCompanion.m_Spawner` ([`crate::unit::Unit::companion_spawner`]).
//!
//! What runs, in the load's order:
//!
//! - **The view attaches** (`SceneLoader.MatchStateWithScene`, [`view_did_attach`]):
//!   `UnitSpawnerBase.MyData.OnViewDidAttach` initializes the spawned unit (the
//!   `IUnitInitializer` parts' `OnInitialize`) and, when the unit stands where it was spawned
//!   (`SpawnPosition` within 0.1 in x and z), moves it to the view's exact position, which
//!   wakes it when the saved, rounded position differs.
//! - **The views subscribe** to the event bus when their scene loads (`EntityViewBase.OnEnable`),
//!   before the area's states are post-loaded ([`subscribe_views`]): `IPartyHandler`,
//!   `ICompanionChangeHandler`.
//! - **`ProcessSpawnerActivationCoroutine`** ([`process_activation`]), once the states are on:
//!   every spawner data in `Game.State.Entities` (in game, not suppressed: no
//!   `SuppressEntitiesController` tick has run yet) whose `ShouldProcessActivation(false)` holds,
//!   sorted by its blueprint's prefab id (`List.Sort`, Mono's introsort), gets
//!   `HandleAreaSpawnerInit`. A companion spawner always processes: it places the companion it
//!   controls (out of the game when it shouldn't be shown), or takes control.
//! - **The capital mode** ([`set_capital_mode`], from `CapitalCompanionLogic`): the
//!   `CountingGuard` changes and `IPartyHandler.HandleCapitalModeChanged` lets each spawner
//!   that can control its companion place it.
//!
//! What isn't ported is reported where it would act: pet spawners (`m_UsePet`), pets of placed
//! companions, a unit spawner that spawns at load, a spawn's ground placement
//! (`UnitPlaceOnGroundController.ForcedTick`), custom cutscenes, settings components without a
//! port, `HandleCompanionRemoved` (its `stayInGame` argument) and `HandleUnrecruit`.

use std::rc::Rc;

use serde_json::Value;
use wr_content::Guid;

use crate::content::Blueprint;
use crate::rulebook::RuleEvent;
use crate::rulebook::bus::{ExternalHandler, RuleStep, Sub};
use crate::scene::ViewInfo;
use crate::world::{EntityIdx, World};

/// `CompanionState`.
const REMOTE: i32 = 1;
const IN_PARTY: i32 = 2;
const EX_COMPANION: i32 = 3;
const IN_PARTY_DETACHED: i32 = 4;

fn short(class: &str) -> &str {
    class.rsplit(['.', '+']).next().unwrap_or(class)
}

fn guid_field(v: &Value) -> Option<Guid> {
    v["guid"].as_str().filter(|s| !s.is_empty()).and_then(Guid::parse)
}

fn flag(v: &Value) -> bool {
    v.as_i64().map_or_else(|| v.as_bool().unwrap_or(false), |i| i != 0)
}

/// A `CompanionSpawner` component's serialized settings.
#[derive(Debug, Clone, Default)]
struct Settings {
    blueprint: Option<Guid>,
    spawn_on_scene_init: bool,
    respawn_if_dead: bool,
    spawn_conditions: Option<Guid>,
    use_pet: bool,
    pet_type: i32,
    spawn_when_remote: bool,
    spawn_when_in_capital: bool,
    spawn_when_ex: bool,
    show_condition: Option<Guid>,
}

/// The view of a loaded scene with this unique id: the spawner data's view (`Data.View`).
fn loaded_view(w: &World, id: &str) -> Option<ViewInfo> {
    let cache = w.scene.cache.as_ref()?;
    let v = cache.view(id)?;
    w.scene.loaded_scenes.contains(&v.scene).then(|| v.clone())
}

/// The companion spawner view of a loaded scene that holds the data `id` (`m_Spawner.Entity?.View
/// as CompanionSpawner`: the data must be loaded, its view a `CompanionSpawner`).
fn companion_view(w: &World, id: &str) -> Option<ViewInfo> {
    if !w.player.spawners.get(id).is_some_and(|s| s.companion) {
        return None;
    }
    loaded_view(w, id).filter(|v| v.class.ends_with(".CompanionSpawner"))
}

/// The settings of a companion spawner view; `None` (reported) when the cache doesn't hold
/// the component.
fn settings(w: &mut World, v: &ViewInfo) -> Option<Settings> {
    let Some(c) = v.components.iter().find(|c| c.class.ends_with(".CompanionSpawner")) else {
        w.coverage.unsupported(
            "CompanionSpawner: the view's settings aren't in the scene cache (rebuild it)",
        );
        return None;
    };
    let f = &c.fields;
    Some(Settings {
        blueprint: guid_field(&f["m_Blueprint"]),
        // `m_SpawnOnSceneInit` and `m_SpawnWhenEx` default to true.
        spawn_on_scene_init: f.get("m_SpawnOnSceneInit").is_none_or(flag),
        respawn_if_dead: flag(&f["m_RespawnIfDead"]),
        spawn_conditions: guid_field(&f["m_spawnConditions"]),
        use_pet: flag(&f["m_UsePet"]),
        #[allow(clippy::cast_possible_truncation)]
        pet_type: f["m_PetType"].as_i64().unwrap_or(0) as i32,
        spawn_when_remote: flag(&f["m_SpawnWhenRemote"]),
        spawn_when_in_capital: flag(&f["m_SpawnWhenInCapital"]),
        spawn_when_ex: f.get("m_SpawnWhenEx").is_none_or(flag),
        show_condition: guid_field(&f["ShowCondition"]),
    })
}

/// A live unit by id (`UnitReference.Value`).
fn unit_by_id(w: &World, id: Option<&str>) -> Option<EntityIdx> {
    let e = *w.by_id.get(id?)?;
    (w.unit(e).is_some() && !w.entity(e).is_disposed).then_some(e)
}

fn spawned_unit(w: &World, id: &str) -> Option<EntityIdx> {
    unit_by_id(w, w.player.spawners.get(id).and_then(|s| s.spawned_unit.as_deref()))
}

fn counter(w: &World, u: EntityIdx, name: &str) -> bool {
    w.player.counters.get(&format!("unit:{}.{name}", w.entity(u).unique_id)).is_some_and(|n| *n > 0)
}

fn has_part(w: &World, u: EntityIdx, part: &str) -> bool {
    w.entity(u).parts.iter().any(|p| p.clr == part)
}

fn is_main_character(w: &World, u: EntityIdx) -> bool {
    w.main_character == Some(u)
}

/// `UnitPartCompanion.IsControllableInParty()` (the part's state; the caller checked the part).
fn is_controllable_in_party(w: &World, u: EntityIdx) -> bool {
    if is_main_character(w, u) {
        return true;
    }
    if w.unit(u).and_then(|x| x.master).is_some_and(|m| is_main_character(w, m)) {
        return true;
    }
    if w.unit(u).and_then(|x| x.companion_state) != Some(IN_PARTY) {
        return false;
    }
    !w.player.capital_party_mode()
}

/// `UnitPartCompanion.GetCurrentSpawner()`: the data id of the companion's spawner, if it is a
/// loaded companion spawner.
pub fn current_spawner(w: &World, u: EntityIdx) -> Option<String> {
    if !has_part(w, u, "UnitPartCompanion") {
        return None;
    }
    let id = w.unit(u)?.companion_spawner.clone()?;
    companion_view(w, &id).map(|_| id)
}

/// `Player.AddCharacterToLists`' `IsControlledBySpawner(unit)`.
pub fn is_controlled_by_spawner(w: &World, u: EntityIdx) -> bool {
    current_spawner(w, u).is_some_and(|s| spawned_unit(w, &s) == Some(u))
}

/// `CompanionSpawner.GetMyCompanion()`: the one character of `AllCharacters` with the
/// spawner's blueprint (`SingleItem`: the first, an error logged for a second).
/// With `m_UsePet`, that character's first pet (`Pets`: `UnitPartPetMaster.m_Pets` in order)
/// whose `UnitPartPet.Type` is the spawner's `m_PetType`.
fn my_companion(w: &mut World, s: &Settings) -> Option<EntityIdx> {
    let bp = s.blueprint?;
    let all = crate::narrative::party::character_lists(w).all_characters;
    let mut found = all.into_iter().filter(|u| w.unit(*u).and_then(|x| x.blueprint) == Some(bp));
    let first = found.next();
    if first.is_some() && found.next().is_some() {
        w.log("error", "Sequence contains more than one suitable items");
    }
    if !s.use_pet {
        return first;
    }
    let refs = w.unit(first?)?.pet_refs.clone();
    refs.iter()
        .filter_map(|id| unit_by_id(w, Some(id)))
        .find(|p| w.unit(*p).and_then(|x| x.pet_type) == Some(s.pet_type))
}

/// `CompanionSpawner.ShouldShowUnit(unit)`.
fn should_show_unit(w: &mut World, s: &Settings, u: Option<EntityIdx>) -> bool {
    let Some(u) = u else { return false };
    let companion = has_part(w, u, "UnitPartCompanion");
    if companion && counter(w, u, "ShouldBeHidden") {
        return false;
    }
    if w.unit(u).is_some_and(|x| x.is_pet) {
        // `UnitPartPet.ShouldBeHidden` isn't modelled.
        w.coverage.unsupported("CompanionSpawner.ShouldShowUnit: a pet");
    }
    let state = if companion { w.unit(u).and_then(|x| x.companion_state).unwrap_or(0) } else { 0 };
    let remote = state == REMOTE;
    let capital =
        w.player.capital_party_mode() && (state == REMOTE || state == IN_PARTY || s.use_pet);
    let ex = state == EX_COMPANION;
    let mut show = (s.spawn_when_remote && remote)
        || (s.spawn_when_ex && ex)
        || (s.spawn_when_in_capital && capital);
    if show && let Some(holder) = s.show_condition.and_then(|g| w.content.get(g)) {
        show = crate::elements::check_conditions(w, &holder, holder.view().obj("Conditions"));
    }
    show
}

fn settings_of(w: &mut World, id: &str) -> Option<Settings> {
    let v = companion_view(w, id)?;
    settings(w, &v)
}

/// `CompanionSpawner.IsControllingCompanion`.
fn is_controlling(w: &mut World, id: &str, s: &Settings) -> bool {
    let Some(my) = my_companion(w, s) else { return false };
    my_is_controlled_by(w, id, my)
}

fn my_is_controlled_by(w: &World, id: &str, my: EntityIdx) -> bool {
    spawned_unit(w, id) == Some(my) && current_spawner(w, my).as_deref() == Some(id)
}

/// `CompanionSpawner.CanControlCompanion`.
fn can_control(w: &mut World, id: &str, s: &Settings) -> bool {
    let Some(data) = w.player.spawners.get(id) else { return false };
    if !data.did_spawn_once && !s.spawn_on_scene_init {
        return false;
    }
    let my = my_companion(w, s);
    if my.is_some_and(|u| has_part(w, u, "UnitPartCompanion") && counter(w, u, "IgnoresSpawners")) {
        return false;
    }
    let current = my.and_then(|u| current_spawner(w, u));
    if let Some(u) = my
        && spawned_unit(w, id) == Some(u)
        && current.as_deref() == Some(id)
    {
        return true;
    }
    if let Some(c) = current.as_deref()
        && c != id
        && let Some(cs) = settings_of(w, c)
        && should_show_unit(w, &cs, my)
    {
        return false;
    }
    my.is_some() && should_show_unit(w, s, my)
}

// ---- the spawner data's initializer parts ------------------------------------------------

/// An `IUnitInitializer` part of a spawner's data, made from its view's components.
#[derive(Debug, Clone)]
enum Init {
    /// `SpawnerInteractionPart` (`SpawnerInteractionDialog`, `SpawnerInteractionBark`): one
    /// wrapper per component, the dialogue of each (`None` for a bark).
    Interaction(Vec<Option<Guid>>),
    /// `SpawnerAdditionalActions.Part`: `SpawnActions` at `OnSpawn`.
    AdditionalActions(Option<Guid>),
    /// `SpawnerCustomCutscene.Part`: plays its cutscene at `OnSpawn` (its `m_Cutscene` isn't
    /// saved, so `OnInitialize`/`OnDispose` find none after a load).
    CustomCutscene(Option<Guid>),
}

impl Init {
    fn clr(&self) -> &'static str {
        match self {
            Self::Interaction(_) => "Kingmaker.UnitLogic.Interaction.SpawnerInteractionPart",
            Self::AdditionalActions(_) => "Kingmaker.View.Spawners.SpawnerAdditionalActions+Part",
            Self::CustomCutscene(_) => "Kingmaker.View.Spawners.SpawnerCustomCutscene+Part",
        }
    }
}

/// The data's `Parts.GetAll<IUnitInitializer>()`: the saved parts in order, then those the
/// view's components `Ensure`d (`EntityDataBase.OnViewDidAttach`), in component order. A
/// component without a port, or a saved part no component backs (removed), is reported.
fn initializers(w: &mut World, id: &str, v: &ViewInfo) -> Vec<Init> {
    let mut made: Vec<Init> = Vec::new();
    for c in &v.components {
        let n = short(&c.class);
        match n {
            "Transform" | "CompanionSpawner" | "ValidNavmeshArea" => {}
            "SpawnerInteractionDialog" | "SpawnerInteractionBark" => {
                let dialog = if n == "SpawnerInteractionDialog" {
                    guid_field(&c.fields["m_Dialog"])
                } else {
                    None
                };
                match made.iter_mut().find_map(|i| match i {
                    Init::Interaction(l) => Some(l),
                    _ => None,
                }) {
                    Some(l) => l.push(dialog),
                    None => made.push(Init::Interaction(vec![dialog])),
                }
            }
            "SpawnerAdditionalActions" => {
                made.push(Init::AdditionalActions(guid_field(&c.fields["SpawnActions"])));
            }
            "SpawnerCustomCutscene" => {
                made.push(Init::CustomCutscene(guid_field(&c.fields["Cutscene"])));
            }
            _ => w
                .coverage
                .unsupported(format!("CompanionSpawner: settings component {n} not ported")),
        }
    }
    let saved = w.player.spawners.get(id).map(|s| s.parts.clone()).unwrap_or_default();
    let mut out: Vec<Init> = Vec::new();
    for p in &saved {
        match made.iter().position(|i| i.clr() == p) {
            Some(k) => out.push(made.remove(k)),
            None => {
                w.coverage
                    .unsupported(format!("EntityDataBase.OnViewDidAttach: {} removed", short(p)));
            }
        }
    }
    out.extend(made);
    out
}

fn ensure_interactions(w: &mut World, u: EntityIdx) {
    if !has_part(w, u, "UnitPartInteractions") {
        w.entity_mut(u).parts.push(crate::world::Part { clr: "UnitPartInteractions" });
    }
}

/// `MyData.ApplyOnSpawn()`: each part's `OnSpawn`, then its `OnInitialize`.
fn apply_on_spawn(w: &mut World, id: &str, inits: &[Init], u: EntityIdx) {
    for i in inits {
        on_spawn(w, std::slice::from_ref(i), u);
        on_initialize(w, id, std::slice::from_ref(i), u);
    }
}

/// `IUnitInitializer.OnSpawn` of each part.
fn on_spawn(w: &mut World, inits: &[Init], u: EntityIdx) {
    for i in inits {
        match i {
            Init::Interaction(_) => {}
            Init::AdditionalActions(a) => {
                if let Some(a) = a.and_then(|g| w.content.get(g)) {
                    crate::elements::with_ctx(w, crate::elements::Ctx::SpawnedUnit(u), |w| {
                        crate::elements::run_actions(w, &a, a.view().obj("Actions"));
                    });
                }
            }
            Init::CustomCutscene(c) => {
                if c.is_some_and(|g| w.content.get(g).is_some()) {
                    w.coverage.unsupported("SpawnerCustomCutscene.Part.OnSpawn: plays a cutscene");
                }
            }
        }
    }
}

/// `IUnitInitializer.OnInitialize` of each part.
fn on_initialize(w: &mut World, id: &str, inits: &[Init], u: EntityIdx) {
    for i in inits {
        if let Init::Interaction(dialogs) = i {
            ensure_interactions(w, u);
            for d in dialogs {
                w.scene.interactions.push((u, id.to_owned(), *d));
            }
        }
    }
}

/// `IUnitInitializer.OnDispose` of each part.
fn on_dispose(w: &mut World, id: &str, inits: &[Init], u: EntityIdx) {
    for i in inits {
        if let Init::Interaction(dialogs) = i {
            ensure_interactions(w, u);
            for d in dialogs {
                if let Some(p) =
                    w.scene.interactions.iter().position(|x| x.0 == u && x.1 == id && x.2 == *d)
                {
                    w.scene.interactions.remove(p);
                }
            }
        }
    }
}

fn inits_of(w: &mut World, id: &str) -> Vec<Init> {
    match companion_view(w, id) {
        Some(v) => initializers(w, id, &v),
        None => Vec::new(),
    }
}

/// `MyData.Clear()`: `ApplyOnDispose` (a live unit's initializers' `OnDispose`), then no unit
/// and `HasSpawned = false`.
fn clear(w: &mut World, id: &str) {
    if let Some(u) = spawned_unit(w, id)
        && !w.player.destroy_marks.contains(&u)
    {
        let inits = inits_of(w, id);
        on_dispose(w, id, &inits, u);
    }
    if let Some(s) = w.player.spawners.get_mut(id) {
        s.spawned_unit = None;
        s.has_spawned = false;
    }
}

/// `MyData.SpawnedUnit = unit`: on a change, the old unit's `Clear()`, then `OnSpawned`
/// (`CompanionSpawner.MyData`: `ApplyOnSpawn` and `HasSpawned` for a live unit, then
/// `m_NeedInitOnPlace = false`, `DidSpawnOnce = true`).
fn set_spawned_unit(w: &mut World, id: &str, u: EntityIdx) {
    let uid = w.entity(u).unique_id.clone();
    let old = w.player.spawners.get(id).and_then(|s| s.spawned_unit.clone());
    if old.as_deref() == Some(uid.as_str()) {
        return;
    }
    if old.is_some() {
        clear(w, id);
    }
    if let Some(s) = w.player.spawners.get_mut(id) {
        s.spawned_unit = Some(uid);
    }
    let inits = inits_of(w, id);
    apply_on_spawn(w, id, &inits, u);
    if let Some(s) = w.player.spawners.get_mut(id) {
        s.has_spawned = true;
        s.need_init_on_place = false;
        s.did_spawn_once = true;
    }
}

/// `UnitPartCompanion.SetSpawner(spawner)`: the current spawner's `StopControllingCompanion`
/// (`Clear`), then `m_Spawner`.
fn set_spawner(w: &mut World, u: EntityIdx, id: &str) {
    if !has_part(w, u, "UnitPartCompanion") {
        return;
    }
    if let Some(cur) = current_spawner(w, u) {
        clear(w, &cur);
    }
    if let Some(x) = w.unit_mut(u) {
        x.companion_spawner = Some(id.to_owned());
    }
}

/// `UnitEntityData.Position = value`: on a change (`Vector3 !=`) the position and `Wake()`
/// (`AwakeTimer = Max(0, AwakeTimer)`). `OnPositionChanged` raises
/// `IEntityPositionChangedHandler`, whose subscribers are controllers (`EntityBoundsController`,
/// `SuppressEntitiesController`), not active before the first tick after the load [C].
fn set_position(w: &mut World, u: EntityIdx, p: [f32; 3]) {
    let Some(cur) = w.unit(u).map(|x| x.position) else { return };
    if crate::combat::orientation::vec3_eq(cur, p) {
        return;
    }
    if let Some(x) = w.unit_mut(u) {
        x.position = p;
    }
    if !w.woken_at_load.contains(&u) {
        w.woken_at_load.push(u);
    }
}

/// `unit.IsInGame = value`, the unit recorded when it changes (its toggles stop and activate
/// again: the first tick's start pass looks at it, `spawn::after_area_load`).
fn set_in_game(w: &mut World, u: EntityIdx, value: bool) {
    if w.entity(u).is_in_game != value && !w.in_game_changed_at_load.contains(&u) {
        w.in_game_changed_at_load.push(u);
    }
    crate::entity::set_is_in_game(w, u, value);
}

/// `UnitEntityData.Orientation = value` (also `DesiredOrientation`).
fn set_orientation(w: &mut World, u: EntityIdx, yaw: f32) {
    if let Some(x) = w.unit_mut(u) {
        x.orientation = yaw;
        x.combat.desired_orientation = yaw;
    }
}

fn view_yaw(v: &ViewInfo) -> f32 {
    crate::combat::orientation::euler_angles(v.rotation)[1]
}

/// `CompanionSpawner.PlaceCompanion(unit, stayInGame)`.
fn place_companion(w: &mut World, id: &str, s: &Settings, u: EntityIdx, stay_in_game: bool) {
    let Some(v) = companion_view(w, id) else { return };
    let mut did_place = false;
    if should_show_unit(w, s, Some(u)) {
        set_position(w, u, v.position);
        set_orientation(w, u, view_yaw(&v));
        set_in_game(w, u, true);
        did_place = true;
    } else if !stay_in_game
        && (!has_part(w, u, "UnitPartCompanion") || !is_controllable_in_party(w, u))
    {
        set_in_game(w, u, false);
    }
    on_place_companion(w, id, did_place);
}

/// `CompanionSpawner.MyData.OnPlaceCompanion(didPlace)`.
fn on_place_companion(w: &mut World, id: &str, did_place: bool) {
    let Some(u) = spawned_unit(w, id) else { return };
    if w.player.destroy_marks.contains(&u) {
        return;
    }
    let need = w.player.spawners.get(id).is_some_and(|s| s.need_init_on_place);
    if !did_place && has_part(w, u, "UnitPartCompanion") && is_controllable_in_party(w, u) && !need
    {
        let inits = inits_of(w, id);
        on_dispose(w, id, &inits, u);
        if let Some(s) = w.player.spawners.get_mut(id) {
            s.need_init_on_place = true;
        }
    }
    let need = w.player.spawners.get(id).is_some_and(|s| s.need_init_on_place);
    if did_place && need {
        let inits = inits_of(w, id);
        on_initialize(w, id, &inits, u);
        if let Some(s) = w.player.spawners.get_mut(id) {
            s.need_init_on_place = false;
        }
    }
}

/// `CompanionSpawner.BeginControllingCompanion(stayInGame)`.
fn begin_controlling(w: &mut World, id: &str, s: &Settings, stay_in_game: bool) {
    let Some(my) = my_companion(w, s) else {
        // `myCompanion.Get<…>()` on null: a `NullReferenceException`, caught by the caller.
        w.log("error", "CompanionSpawner.BeginControllingCompanion: no companion");
        return;
    };
    if !my_is_controlled_by(w, id, my) {
        set_spawner(w, my, id);
        set_spawned_unit(w, id, my);
    }
    place_companion(w, id, s, my, stay_in_game);
    // `myCompanion.Pets.Count > 0`: `UnitPartPetMaster.m_Pets`.
    let has_pets = w.unit(my).is_some_and(|x| !x.pet_refs.is_empty());
    let pet_settings = companion_view(w, id)
        .is_some_and(|v| v.components.iter().any(|c| short(&c.class) == "SpawnerPetSettings"));
    if has_pets && !pet_settings {
        w.coverage.unsupported("CompanionSpawner.PlacePets: a placed companion's pets");
    }
}

/// `UnitSpawnerBase.CheckConditions()`.
fn check_spawn_conditions(w: &mut World, g: Option<Guid>) -> bool {
    let Some(holder) = g.and_then(|g| w.content.get(g)) else { return true };
    let checker = holder.view().obj("Conditions");
    if checker.is_none_or(|c| c.objs("Conditions").is_empty()) {
        return true;
    }
    crate::elements::check_conditions(w, &holder, checker)
}

/// `Game.Instance.CurrentlyLoadedArea.GetComponent<CombatRandomEncounterAreaSettings>() != null`.
fn random_encounter_area(w: &World) -> bool {
    w.player.area.and_then(|g| w.content.get(g)).is_some_and(|a| {
        a.components_array()
            .iter()
            .any(|o| crate::content::is_a(o.ty, "CombatRandomEncounterAreaSettings"))
    })
}

/// `UnitSpawnerBase.HandleAreaSpawnerInit()` for a companion spawner (`base.` in its override).
fn base_handle_area_spawner_init(w: &mut World, id: &str, s: &Settings) {
    if random_encounter_area(w) {
        return;
    }
    let has_spawned = w.player.spawners.get(id).is_some_and(|d| d.has_spawned);
    if has_spawned && s.respawn_if_dead {
        // `SpawnedUnitHasDied` (`HasDied` isn't hydrated).
        w.coverage.unsupported("CompanionSpawner: respawn of a dead companion");
        return;
    }
    if has_spawned || !s.spawn_on_scene_init || !check_spawn_conditions(w, s.spawn_conditions) {
        return;
    }
    if let Some(u) = spawned_unit(w, id)
        && !has_part(w, u, "UnitPartCompanion")
    {
        w.coverage
            .unsupported("UnitSpawnerBase.HandleAreaSpawnerInit: MarkForDestroy of a spawned unit");
        return;
    }
    clear(w, id);
    spawn(w, id, s);
}

/// `UnitSpawnerBase.Spawn()` of a companion spawner.
fn spawn(w: &mut World, id: &str, s: &Settings) {
    if w.player.spawners.get(id).is_some_and(|d| d.has_spawned) {
        w.log("warning", format!("Trying to use spawner {id} twice."));
        return;
    }
    if s.blueprint.is_none() {
        w.log("error", "UnitSpawnerBase.Spawn: unit blueprint is null!");
        return;
    }
    // `IUnitSpawnRestriction` components (`SpawnerDifficultySettings`) aren't among the ported
    // ones: `initializers` reports them.
    let Some(v) = companion_view(w, id) else { return };
    // `CompanionSpawner.SpawnUnit`: no companion, no unit.
    let Some(u) = my_companion(w, s) else { return };
    begin_controlling(w, id, s, false);
    // `Data.SpawnedUnit = unit` (set by `BeginControllingCompanion`), `SpawnPosition`,
    // `GiveExperienceOnDeath &&= !HasDied` (`HasDied` isn't hydrated: reported with the
    // respawn).
    set_spawned_unit(w, id, u);
    if let Some(x) = w.unit_mut(u) {
        x.spawn_position = v.position;
    }
    w.coverage.unsupported("UnitSpawnerBase.Spawn: UnitPlaceOnGroundController.ForcedTick");
}

/// `CompanionSpawner.HandleAreaSpawnerInit()`.
fn handle_area_spawner_init(w: &mut World, id: &str) {
    let Some(s) = settings_of(w, id) else { return };
    w.coverage.executed("CompanionSpawner.HandleAreaSpawnerInit");
    if w.player.spawners.get(id).is_some_and(|d| d.has_spawned) {
        if is_controlling(w, id, &s) {
            if let Some(u) = spawned_unit(w, id) {
                place_companion(w, id, &s, u, false);
            }
        } else {
            clear(w, id);
        }
    }
    if can_control(w, id, &s) {
        base_handle_area_spawner_init(w, id, &s);
    }
}

// ---- the load ------------------------------------------------------------------------------

/// `UnitSpawnerBase.MyData.OnViewDidAttach()` of the companion spawners' data in `ids` (a
/// state's, in `m_EntityData` order): the spawned unit's initializers (`OnSpawn` only for a
/// unit restored from a placeholder whose part asks for it: none of the ported ones does),
/// then, when the unit stands where it was spawned (`(SpawnPosition.To2D() -
/// Position.To2D()).magnitude < 0.1f`), the view's position, orientation and spawn position.
pub fn view_did_attach(w: &mut World, ids: &[String]) {
    for id in ids {
        let Some(v) = companion_view(w, id) else { continue };
        let inits = initializers(w, id, &v);
        let Some(u) = spawned_unit(w, id) else { continue };
        on_initialize(w, id, &inits, u);
        let Some(x) = w.unit(u) else { continue };
        let dx = x.spawn_position[0] - x.position[0];
        let dz = x.spawn_position[2] - x.position[2];
        #[allow(clippy::cast_possible_truncation)]
        let m = (f64::from(dx) * f64::from(dx) + f64::from(dz) * f64::from(dz)).sqrt() as f32;
        if m < 0.1 {
            set_position(w, u, v.position);
            if let Some(x) = w.unit_mut(u) {
                x.spawn_position = v.position;
            }
            set_orientation(w, u, view_yaw(&v));
        }
    }
}

/// Before a subscribed spawner takes its companion in an event: if another subscribed spawner
/// could take the same companion now, the outcome depends on the views' subscription order,
/// which is inferred (the cache's object order): reported.
fn report_order_dependence(w: &mut World, id: &str, s: &Settings) {
    let Some(mine) = my_companion(w, s) else { return };
    for other in w.scene.companion_views.clone() {
        if other == id || !w.player.spawners.contains_key(&other) {
            continue;
        }
        let Some(os) = settings_of(w, &other) else { continue };
        if os.blueprint != s.blueprint || os.use_pet != s.use_pet {
            continue;
        }
        if my_companion(w, &os) == Some(mine) && can_control(w, &other, &os) {
            w.coverage.unsupported(
                "CompanionSpawner: two spawners can take a companion in one event (their subscription order is inferred)",
            );
            return;
        }
    }
}

/// The external-handler id base of the companion spawner views.
const VIEW_HANDLER_BASE: u32 = 0x4353_0000;

/// A companion spawner view as an event-bus subscriber (`IPartyHandler`,
/// `ICompanionChangeHandler`; `IResetHandler` is raised by nothing the sim runs).
struct SpawnerView(String);

impl ExternalHandler for SpawnerView {
    fn interfaces(&self) -> Vec<String> {
        vec!["IPartyHandler".into(), "ICompanionChangeHandler".into()]
    }

    fn event(&self, w: &mut World, iface: &str) {
        let (method, unit) = w.bus.call.last().copied().unwrap_or(("", None));
        let id = self.0.as_str();
        if !w.player.spawners.contains_key(id) {
            // No data: `CanControlCompanion` and `SpawnedUnit` are false and null.
            return;
        }
        let Some(s) = settings_of(w, id) else { return };
        let same_bp = |w: &World, u: EntityIdx| {
            s.blueprint.is_some() && w.unit(u).and_then(|x| x.blueprint) == s.blueprint
        };
        match (iface, method) {
            ("IPartyHandler", "HandleCapitalModeChanged") => {
                w.coverage.executed("CompanionSpawner.HandleCapitalModeChanged");
                if can_control(w, id, &s) {
                    report_order_dependence(w, id, &s);
                    begin_controlling(w, id, &s, false);
                }
            }
            ("IPartyHandler", "HandleAddCompanion" | "HandleCompanionActivated") => {
                let Some(u) = unit else { return };
                let pet_of =
                    s.use_pet && w.unit(u).and_then(|x| x.master).is_some_and(|m| same_bp(w, m));
                let matches = same_bp(w, u) || (method == "HandleAddCompanion" && pet_of);
                if matches && can_control(w, id, &s) {
                    report_order_dependence(w, id, &s);
                    begin_controlling(w, id, &s, false);
                }
            }
            ("ICompanionChangeHandler", "HandleRecruit") => {
                let Some(u) = unit else { return };
                if spawned_unit(w, id).is_none() && same_bp(w, u) && can_control(w, id, &s) {
                    report_order_dependence(w, id, &s);
                    begin_controlling(w, id, &s, false);
                }
            }
            ("IPartyHandler", "HandleCompanionRemoved") => {
                // `BeginControllingCompanion(stayInGame)`: the argument isn't in the sim's call.
                let Some(u) = unit else { return };
                if same_bp(w, u) && can_control(w, id, &s) {
                    w.coverage.unsupported("CompanionSpawner.HandleCompanionRemoved");
                }
            }
            ("ICompanionChangeHandler", "HandleUnrecruit") => {
                let Some(u) = unit else { return };
                if spawned_unit(w, id) == Some(u) && !should_show_unit(w, &s, Some(u)) {
                    w.coverage.unsupported("CompanionSpawner.HandleUnrecruit");
                }
            }
            _ => w.coverage.unsupported(format!("CompanionSpawner: {iface}.{method}")),
        }
    }

    fn rule(&self, _w: &mut World, _rule: &mut dyn RuleEvent, _step: RuleStep) {}
}

/// `EntityViewBase.OnEnable` → `EventBus.Subscribe(this)` for every active companion spawner view
/// of the loaded scenes, as their scenes load (before the area's states are post-loaded), in
/// the scene cache's view order. [I] Unity's enable order within a scene is taken to be the
/// cache's object order: on Manual_10 the order the capital mode reached seven spawners
/// (WrathOracle's `in_game` events) is that order [M].
pub fn subscribe_views(w: &mut World) {
    let Some(cache) = w.scene.cache.clone() else { return };
    let mut n = 0u32;
    for v in &cache.views {
        if !v.class.ends_with(".CompanionSpawner")
            || !v.active_in_hierarchy
            || !w.scene.loaded_scenes.contains(&v.scene)
        {
            continue;
        }
        let key = VIEW_HANDLER_BASE + n;
        n += 1;
        w.bus.external.insert(key, Rc::new(SpawnerView(v.unique_id.clone())));
        w.scene.companion_views.push(v.unique_id.clone());
        crate::rulebook::bus::subscribe(w, Sub::External(key));
    }
}

/// `UnitSpawnerBase.MyData.ShouldProcessActivation(alsoRaiseDead: false)`; `None` when the
/// spawner's kind isn't known.
fn should_process(w: &mut World, id: &str, v: &ViewInfo) -> Option<bool> {
    let d = w.player.spawners.get(id)?;
    if d.companion {
        return Some(true);
    }
    if !v.class.ends_with(".UnitSpawner") {
        w.coverage.unsupported(format!(
            "SceneLoader.ProcessSpawnerActivationCoroutine: spawner view {}",
            short(&v.class)
        ));
        return None;
    }
    let has_spawned = d.has_spawned;
    let c = v.components.iter().find(|c| c.class.ends_with(".UnitSpawner"))?;
    let on_init = c.fields.get("m_SpawnOnSceneInit").is_none_or(flag);
    Some(on_init && !has_spawned)
}

/// The spawner's blueprint's `Prefab?.AssetId` (`GetPrefabId`): `None` for no blueprint, no
/// link or a null `AssetId`; an empty one stays `""` (`string.Compare` sorts null before it).
fn prefab_id(w: &World, v: &ViewInfo) -> Option<String> {
    use wr_content::node::Node;
    let c = v
        .components
        .iter()
        .find(|c| c.class.ends_with(".UnitSpawner") || c.class.ends_with(".CompanionSpawner"))?;
    let bp: Rc<Blueprint> = guid_field(&c.fields["m_Blueprint"]).and_then(|g| w.content.get(g))?;
    let view = bp.view();
    match view.node("Prefab")? {
        Node::WeakLink(Some(s)) | Node::WeakLinkLegacy(s, _) | Node::Str(s) => Some(s.to_string()),
        Node::Object(o) => crate::content::View(o).str("AssetId").map(str::to_owned),
        _ => None,
    }
}

/// `string.Compare(a, b, StringComparison.Ordinal)` with null first.
fn ordinal(a: Option<&str>, b: Option<&str>) -> i32 {
    match (a, b) {
        (None, None) => 0,
        (None, Some(_)) => -1,
        (Some(_), None) => 1,
        (Some(a), Some(b)) => {
            let (a, b): (Vec<u16>, Vec<u16>) =
                (a.encode_utf16().collect(), b.encode_utf16().collect());
            match a.cmp(&b) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            }
        }
    }
}

/// `SceneLoader.ProcessSpawnerActivationCoroutine(alsoRaiseDead: false)` after a save's area
/// states are on (`LoadAreaCoroutine`, no enter point).
pub fn process_activation(w: &mut World) {
    if w.player.spawner_order.is_empty() {
        return;
    }
    if w.scene.cache.is_none() {
        w.coverage.unsupported(
            "SceneLoader.ProcessSpawnerActivationCoroutine: no scene cache (spawner settings unknown)",
        );
        return;
    }
    // `Game.Instance.State.Entities.OfType<UnitSpawnerBase.MyData>()`: the pool's enumerator
    // skips entities out of the game or suppressed (`m_Suppressed` isn't saved and only
    // `SuppressEntitiesController` sets it, which hasn't ticked yet).
    let mut list: Vec<(String, Option<String>)> = Vec::new();
    for id in w.player.spawner_order.clone() {
        if !w.player.spawners.get(&id).is_some_and(|s| s.in_game) {
            continue;
        }
        // A data without a view of its scene is removed from its state at load (reported by
        // `check_scene_views`); `(UnitSpawnerBase)View` is then null.
        let Some(v) = loaded_view(w, &id) else { continue };
        if should_process(w, &id, &v) == Some(true) {
            let p = prefab_id(w, &v);
            list.push((id, p));
        }
    }
    // `Enumerable.ToList()` grows the list from empty; `List.Sort(Comparison)`.
    let cap = crate::csharp::sort::grown_capacity(list.len());
    let r = crate::csharp::sort::introsort(&mut list, cap, &mut |a, b| {
        ordinal(a.1.as_deref(), b.1.as_deref())
    });
    if r.heapsort {
        w.coverage.unsupported(
            "SceneLoader.ProcessSpawnerActivationCoroutine: sort fell back to heapsort",
        );
    }
    for (id, prefab) in list {
        if prefab.as_deref().is_none_or(str::is_empty) {
            w.log("warning", format!("Spawner {id} has no prefab Id"));
            continue;
        }
        let companion = w.player.spawners.get(&id).is_some_and(|s| s.companion);
        if companion {
            handle_area_spawner_init(w, &id);
        } else {
            unit_spawner_init(w, &id);
        }
    }
}

/// `UnitSpawnerBase.HandleAreaSpawnerInit()` of a unit spawner that processes at load (it hasn't
/// spawned and spawns on scene init): its spawn isn't ported, so a spawn is reported.
fn unit_spawner_init(w: &mut World, id: &str) {
    if random_encounter_area(w) {
        return;
    }
    let Some(v) = loaded_view(w, id) else { return };
    let conds = v
        .components
        .iter()
        .find(|c| c.class.ends_with(".UnitSpawner"))
        .and_then(|c| guid_field(&c.fields["m_spawnConditions"]));
    if check_spawn_conditions(w, conds) {
        w.coverage.unsupported(
            "SceneLoader.ProcessSpawnerActivationCoroutine: a unit spawner spawns at load",
        );
    }
}

// ---- the capital mode ------------------------------------------------------------------------

/// `RuntimeAreaSettings.SetCapitalMode(value)`: `CapitalPartyMode.SetValue(value)` (a
/// `CountingGuard` that can go negative); on a change of its value, `InvalidateCharacterLists`
/// (the sim computes the lists on each read) and `IPartyHandler.HandleCapitalModeChanged`.
pub fn set_capital_mode(w: &mut World, value: bool) {
    let before = w.player.capital_party_mode();
    if value {
        w.player.capital_party_guard += 1;
    } else {
        w.player.capital_party_guard -= 1;
    }
    if w.player.capital_party_mode() != before {
        crate::rulebook::bus::raise_global_call(
            w,
            "IPartyHandler",
            "HandleCapitalModeChanged",
            None,
        );
    }
}

/// `CapitalCompanionLogic.PlaceAllCompanions()`: capital mode on, then every remote companion
/// (not the main character) without a master that keeps it, whose part has no current spawner
/// and doesn't ignore spawners, leaves the game. The selection of the main character's view is
/// UI.
pub fn place_all_companions(w: &mut World) {
    set_capital_mode(w, true);
    let remote = crate::narrative::party::character_lists(w).remote_companions;
    for u in remote {
        if is_main_character(w, u) {
            continue;
        }
        let master = w.unit(u).and_then(|x| x.master);
        let kept =
            master.is_some_and(|m| is_main_character(w, m) || current_spawner(w, m).is_some());
        if kept {
            continue;
        }
        let companion = has_part(w, u, "UnitPartCompanion");
        if !companion || (current_spawner(w, u).is_none() && !counter(w, u, "IgnoresSpawners")) {
            set_in_game(w, u, false);
        }
    }
    w.coverage.stub("CapitalCompanionLogic.PlaceAllCompanions: select the main character (UI)");
}

/// `CapitalCompanionLogic.OnEnter()`.
pub fn capital_on_enter(w: &mut World, rest_all_remote: bool) {
    // Another playing etude with a `CapitalCompanionLogic` only logs an error.
    // `InPartyDetached` cross-scene companions are attached to the party first.
    let detached = w
        .player
        .cross_scene_units
        .iter()
        .any(|u| w.unit(*u).and_then(|x| x.companion_state) == Some(IN_PARTY_DETACHED));
    if detached {
        w.coverage.unsupported("CapitalCompanionLogic.OnEnter: AttachPartyMember");
    }
    place_all_companions(w);
    // `item.TryIdentify()` for each item of the player's inventory.
    let items =
        w.main_character.and_then(|m| w.unit(m)).map(|m| m.inventory.clone()).unwrap_or_default();
    for it in items {
        crate::rules::identify::try_identify_by_party(w, it);
    }
    if rest_all_remote {
        rest_remote_companions(w);
    }
    crate::rulebook::bus::raise_global_call(w, "ISafeZoneHandler", "OnEnterSafeZone", None);
}

/// `m_RestAllRemoteCompanions`: each remote companion (state `Remote`) rests once per day
/// since its last rest beyond the camp time (`(int)(GameTime - LastRestTime).TotalHours`, the
/// last rest a day ago when unknown; `1 + (hours - 9) / 24` rests from 9 hours on, at most
/// 100). `RestController.HealAndApplyRest` isn't ported: a rest is reported.
fn rest_remote_companions(w: &mut World) {
    const HOUR: i64 = 36_000_000_000;
    let now = w.clock.game_time.0;
    for u in crate::narrative::party::character_lists(w).remote_companions {
        if w.unit(u).and_then(|x| x.companion_state) != Some(REMOTE) {
            continue;
        }
        let last = w.unit(u).and_then(|x| x.last_rest_time).unwrap_or(now - 24 * HOUR);
        // `TimeSpan.TotalHours` (`ticks * (1 / 36e9)` as a double), truncated.
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let hours = ((now - last) as f64 * (1.0 / 36e9)) as i32;
        let camp = 9;
        let rests = if hours >= camp { 1 + (hours - camp) / 24 } else { 0 };
        if rests.min(100) > 0 {
            w.coverage
                .unsupported("CapitalCompanionLogic.OnEnter: RestController.HealAndApplyRest");
        }
    }
}

/// `CapitalCompanionLogic.OnExit()`: the remote companions lose the logic's own interaction
/// overrides (`OverrideDialogInteraction`, `OverrideBarkInteraction`, which nothing the sim
/// runs adds), each remote companion in the `Remote` state leaves the game, and the capital
/// mode turns off.
pub fn capital_on_exit(w: &mut World) {
    let remote = crate::narrative::party::character_lists(w).remote_companions;
    for u in remote {
        if has_part(w, u, "UnitPartCompanion")
            && w.unit(u).and_then(|x| x.companion_state) == Some(REMOTE)
        {
            set_in_game(w, u, false);
        }
    }
    set_capital_mode(w, false);
}

/// The `IPartyHandler` methods of `AddFeatureToPet` (`Kingmaker.Designers.Mechanics.Facts/
/// AddFeatureToPet.cs`) and `HideAllPets` (`Kingmaker.AreaLogic.Etudes/HideAllPets.cs`), whose
/// lifecycle hooks aren't ported (reported): `HandleCompanionActivated`,
/// `HandleCompanionRemoved` and `HandleCapitalModeChanged` are empty; `HandleAddCompanion`
/// (a pet's features, hiding a pet) is reported.
fn pet_party_event(w: &mut World, c: crate::world::CompRef, iface: &str) {
    let name = w.component(c).ty().map_or("?", |t| wr_content::schema::ty(t).short);
    match (iface, w.bus.call.last().map(|x| x.0)) {
        (
            "IPartyHandler",
            Some(
                "HandleCompanionActivated" | "HandleCompanionRemoved" | "HandleCapitalModeChanged",
            ),
        ) => {}
        (_, m) => w.coverage.unsupported(format!("{name}: {iface}.{}", m.unwrap_or("?"))),
    }
}

/// Registers the pets' `IPartyHandler` handlers ([`pet_party_event`]).
pub fn register(r: &mut crate::registry::Registry) {
    for name in ["AddFeatureToPet", "HideAllPets"] {
        let mut p = crate::components::Port::new(name, 0, |_, _, _| {});
        p.handlers = &["IPartyHandler"];
        p.ev = Some(pet_party_event);
        r.register(Box::new(p));
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use serde_json::json;
    use wr_content::Guid;
    use wr_content::node::Node;

    use super::*;
    use crate::content::Blueprint;
    use crate::facts::tests::obj;
    use crate::scene::{CompInfo, SceneCache};

    const COMPANION_BP: u128 = 0xC0;
    const SPAWNER_PART: &str = "Kingmaker.UnitLogic.Interaction.SpawnerInteractionPart";

    fn companion_bp(w: &World, g: u128, prefab: &str) {
        w.content.insert(Blueprint {
            guid: Guid(g),
            name: format!("B{g}"),
            obj: obj(
                "Kingmaker.Blueprints.BlueprintUnit",
                &[("Prefab", Node::WeakLink(Some(prefab.into())))],
            ),
            source: crate::content::Source::Bbp,
        });
    }

    fn spawner_view(
        id: &str,
        bp: u128,
        capital: bool,
        remote: bool,
        position: [f32; 3],
    ) -> ViewInfo {
        ViewInfo {
            unique_id: id.into(),
            class: "Kingmaker.View.Spawners.CompanionSpawner".into(),
            scene: "S".into(),
            active_in_hierarchy: true,
            is_in_game: true,
            components: vec![
                CompInfo { class: "Transform".into(), fields: json!({}) },
                CompInfo {
                    class: "Kingmaker.View.Spawners.CompanionSpawner".into(),
                    fields: json!({
                        "m_Blueprint": { "guid": Guid(bp).to_string() },
                        "m_SpawnOnSceneInit": 0, "m_RespawnIfDead": 0, "m_spawnConditions": { "guid": "" },
                        "m_UsePet": 0, "m_PetType": 0, "m_SpawnWhenRemote": i32::from(remote),
                        "m_SpawnWhenInCapital": i32::from(capital), "m_SpawnWhenEx": 0,
                        "ShowCondition": { "guid": "" },
                    }),
                },
                CompInfo {
                    class: "Kingmaker.UnitLogic.Interaction.SpawnerInteractionDialog".into(),
                    fields: json!({ "m_Dialog": { "guid": "" } }),
                },
            ],
            position,
            // 90 degrees about y.
            rotation: [0.0, std::f32::consts::FRAC_1_SQRT_2, 0.0, std::f32::consts::FRAC_1_SQRT_2],
            group: None,
        }
    }

    /// A remote companion `u` (blueprint `COMPANION_BP`, saved at a rounded position, its spawn
    /// position the same), controlled by the spawner `sp`, which shows it only in capital
    /// mode; the scene `S` is loaded.
    fn setup() -> (World, EntityIdx) {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        w.registry = Rc::new(crate::components::registry());
        companion_bp(&w, COMPANION_BP, "prefab-a");
        w.entity_mut(u).parts.push(crate::world::Part { clr: "UnitPartCompanion" });
        let x = w.unit_mut(u).expect("unit");
        x.blueprint = Some(Guid(COMPANION_BP));
        x.companion_state = Some(REMOTE);
        x.companion_spawner = Some("sp".into());
        x.position = [-13.46, 40.13, 60.64];
        x.spawn_position = [-13.46, 40.13, 60.64];
        w.player.cross_scene_units = vec![u];
        w.scene.cache = Some(Rc::new(SceneCache {
            schema: crate::scene::SCHEMA.into(),
            area: String::new(),
            name: String::new(),
            source: String::new(),
            zones: Vec::new(),
            views: vec![spawner_view("sp", COMPANION_BP, true, false, [-13.467, 40.136, 60.642])],
        }));
        w.scene.loaded_scenes = vec!["S".into()];
        w.player.spawners.insert(
            "sp".into(),
            crate::narrative::Spawner {
                spawned_unit: Some("u".into()),
                has_spawned: true,
                companion: true,
                did_spawn_once: true,
                need_init_on_place: false,
                in_game: true,
                parts: vec![SPAWNER_PART.into()],
            },
        );
        w.player.spawner_order = vec!["sp".into()];
        (w, u)
    }

    /// The load's sequence for a remote companion of a capital: the view attaches (the
    /// interaction, the exact position: a wake), the spawner activation takes the companion
    /// out of the game (capital mode is off), and turning capital mode on puts it back where its
    /// spawner stands.
    #[test]
    fn a_remote_companion_leaves_the_game_at_load_and_returns_in_capital_mode() {
        let (mut w, u) = setup();
        view_did_attach(&mut w, &["sp".to_owned()]);
        assert_eq!(w.unit(u).expect("unit").position, [-13.467, 40.136, 60.642]);
        assert_eq!(w.unit(u).expect("unit").spawn_position, [-13.467, 40.136, 60.642]);
        assert!((w.unit(u).expect("unit").orientation - 90.0).abs() < 1e-3);
        assert_eq!(w.woken_at_load, vec![u], "the rounded saved position differs: a wake");
        assert_eq!(w.scene.interactions, vec![(u, "sp".to_owned(), None)]);
        assert!(has_part(&w, u, "UnitPartInteractions"));
        subscribe_views(&mut w);
        process_activation(&mut w);
        assert!(!w.entity(u).is_in_game, "not shown outside capital mode");
        assert_eq!(w.player.spawners["sp"].spawned_unit.as_deref(), Some("u"), "still controlled");
        set_capital_mode(&mut w, true);
        assert!(w.player.capital_party_mode());
        assert!(w.entity(u).is_in_game, "placed by its spawner");
        // The party in capital mode: the companion is remote.
        let lists = crate::narrative::party::character_lists(&mut w);
        assert_eq!(lists.remote_companions, vec![u]);
        assert!(lists.party.is_empty());
        // A second retain doesn't change the value: no event.
        set_capital_mode(&mut w, true);
        assert_eq!(w.player.capital_party_guard, 2);
        set_capital_mode(&mut w, false);
        assert!(w.player.capital_party_mode());
    }

    /// A spawner that stands where the unit already is doesn't wake it (`Vector3 !=`), and one
    /// whose unit has moved from its spawn position keeps the unit where it is.
    #[test]
    fn attaching_moves_and_wakes_only_on_a_change() {
        let (mut w, u) = setup();
        w.unit_mut(u).expect("unit").position = [-13.467, 40.136, 60.642];
        w.unit_mut(u).expect("unit").spawn_position = [-13.467, 40.136, 60.642];
        view_did_attach(&mut w, &["sp".to_owned()]);
        assert!(w.woken_at_load.is_empty());
        let (mut w, u) = setup();
        w.unit_mut(u).expect("unit").position = [0.0, 0.0, 0.0];
        view_did_attach(&mut w, &["sp".to_owned()]);
        assert_eq!(w.unit(u).expect("unit").position, [0.0, 0.0, 0.0]);
        assert!(w.woken_at_load.is_empty());
    }

    /// Another spawner of the same companion that would show it (and spawns on scene init)
    /// takes control when the current one doesn't show it; the old one's data is cleared
    /// (`SetSpawner`'s `StopControllingCompanion`), its interaction disposed.
    #[test]
    fn a_spawner_that_shows_the_companion_takes_control() {
        let (mut w, u) = setup();
        let mut cache = (**w.scene.cache.as_ref().expect("cache")).clone();
        let mut v2 = spawner_view("sp2", COMPANION_BP, false, true, [5.0, 0.0, 5.0]);
        v2.components[1].fields["m_SpawnOnSceneInit"] = json!(1);
        cache.views.push(v2);
        w.scene.cache = Some(Rc::new(cache));
        w.player.spawners.insert(
            "sp2".into(),
            crate::narrative::Spawner {
                did_spawn_once: true,
                companion: true,
                in_game: true,
                ..Default::default()
            },
        );
        w.player.spawner_order.push("sp2".into());
        view_did_attach(&mut w, &["sp".to_owned(), "sp2".to_owned()]);
        process_activation(&mut w);
        // `sp` (capital only) hides the companion; `sp2` (remote) shows it and takes it.
        assert!(w.entity(u).is_in_game);
        assert_eq!(w.unit(u).expect("unit").position, [5.0, 0.0, 5.0]);
        assert_eq!(w.unit(u).expect("unit").companion_spawner.as_deref(), Some("sp2"));
        assert_eq!(w.player.spawners["sp"].spawned_unit, None);
        assert!(!w.player.spawners["sp"].has_spawned);
        assert_eq!(w.player.spawners["sp2"].spawned_unit.as_deref(), Some("u"));
        assert!(w.player.spawners["sp2"].has_spawned);
        assert_eq!(w.scene.interactions, vec![(u, "sp2".to_owned(), None)]);
        // The spawn's ground placement isn't ported.
        assert!(
            w.coverage
                .unsupported
                .contains_key("UnitSpawnerBase.Spawn: UnitPlaceOnGroundController.ForcedTick")
        );
    }

    /// The activation list is sorted by prefab id (ordinal), and a unit spawner that would
    /// spawn at load is reported.
    #[test]
    fn unit_spawners_that_spawn_at_load_are_reported() {
        let (mut w, _) = setup();
        companion_bp(&w, 0xC1, "prefab-b");
        let mut cache = (**w.scene.cache.as_ref().expect("cache")).clone();
        let mut v = spawner_view("us", 0xC1, false, false, [0.0; 3]);
        v.class = "Kingmaker.View.Spawners.UnitSpawner".into();
        v.components = vec![CompInfo {
            class: "Kingmaker.View.Spawners.UnitSpawner".into(),
            fields: json!({ "m_Blueprint": { "guid": Guid(0xC1).to_string() }, "m_SpawnOnSceneInit": 1, "m_spawnConditions": { "guid": "" } }),
        }];
        cache.views.push(v);
        w.scene.cache = Some(Rc::new(cache));
        w.player
            .spawners
            .insert("us".into(), crate::narrative::Spawner { in_game: true, ..Default::default() });
        w.player.spawner_order.push("us".into());
        process_activation(&mut w);
        assert!(w.coverage.unsupported.contains_key(
            "SceneLoader.ProcessSpawnerActivationCoroutine: a unit spawner spawns at load"
        ));
        assert_eq!(ordinal(Some("a"), Some("b")), -1);
        assert_eq!(ordinal(None, Some("")), -1);
        assert_eq!(ordinal(Some("b"), Some("b")), 0);
    }

    /// Two spawners that could both take a companion in one event: the views' subscription
    /// order (inferred) decides, which is reported; the first in that order takes it.
    #[test]
    fn competing_spawners_report_their_inferred_order() {
        let (mut w, u) = setup();
        w.unit_mut(u).expect("unit").companion_spawner = None;
        let d = w.player.spawners.get_mut("sp").expect("sp");
        d.spawned_unit = None;
        d.has_spawned = false;
        d.parts.clear();
        let mut cache = (**w.scene.cache.as_ref().expect("cache")).clone();
        cache.views.push(spawner_view("sp2", COMPANION_BP, true, false, [5.0, 0.0, 5.0]));
        w.scene.cache = Some(Rc::new(cache));
        w.player.spawners.insert(
            "sp2".into(),
            crate::narrative::Spawner {
                did_spawn_once: true,
                companion: true,
                in_game: true,
                ..Default::default()
            },
        );
        subscribe_views(&mut w);
        set_capital_mode(&mut w, true);
        assert!(
            w.coverage.unsupported.keys().any(|k| k.contains("subscription order is inferred"))
        );
        assert_eq!(w.unit(u).expect("unit").companion_spawner.as_deref(), Some("sp"));
        assert_eq!(w.unit(u).expect("unit").position, [-13.467, 40.136, 60.642]);
    }
}
