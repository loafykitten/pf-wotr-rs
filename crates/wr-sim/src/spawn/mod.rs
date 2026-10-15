//! Units created from blueprints while loading: `UnitPlaceholderEntityData.BecomeUnit`
//! (decompiled, not copied: `Kingmaker.EntitySystem.Entities/UnitPlaceholderEntityData.cs:213–330`)
//! → `EntityCreationController.SpawnUnit` (`Kingmaker.Controllers/EntityCreationController.cs:79–260`)
//! → `new UnitEntityData(id, isInGame, blueprint)` (`Kingmaker.EntitySystem.Entities/UnitEntityData.cs:1019–1075`,
//! `Kingmaker.EntitySystem/EntityDataBase.cs:256`) → `new UnitDescriptor` and
//! `UnitDescriptor.Initialize` (`Kingmaker.UnitLogic/UnitDescriptor.cs:690–882`), with the
//! automatic class levels of the unit blueprint's `AddClassLevels` ([`levelup`]).
//!
//! The order matters for the facts' unique ids, which come from one global counter: the
//! entity is turned on in its constructor, so every fact added during construction is
//! attached, activated and turned on at once, depth first (a parent's `OnActivate` adds its
//! children before the parent's own `TurnOn`).

pub mod copy;
pub mod levelup;
#[cfg(test)]
mod tests;

use std::rc::Rc;

use wr_content::Guid;

use crate::content::{Blueprint, is_a};
use crate::registry::{Hook, Registry};
use crate::stats::ops;
use crate::stats::types::{Size, StatType};
use crate::stats::value::Stats;
use crate::unit::{Body, Progression, Unit};
use crate::world::{CompRef, EntityIdx, EntityKind, FactKind, World};

/// The `ContextData` flags unit construction reads: `UnitEntityData.DoNotCreateItems`
/// (no body or starting items) and `AddClassLevels.DoNotCreatePlan` (no level plan for the
/// levels a unit can't take yet).
#[derive(Debug, Clone, Copy, Default)]
pub struct BuildFlags {
    pub do_not_create_items: bool,
    pub do_not_create_plan: bool,
}

/// A saved roaming state whose next point is a `RoamingPoint` of its own with an idle cutscene:
/// `UnitRoamingController.TickOnUnit` plays it on arrival (`RoamingPoint.SelectCutscene` draws
/// nothing, so no recorded producer names it).
pub const ROAMING_NEXT_IDLE_CUTSCENE: &str =
    "UnitPartRoaming: a saved next point with an idle cutscene (RoamingPoint.IdleCutscene)";

/// A placeholder as saved, with its reserved unit entity.
#[derive(Debug, Clone)]
pub struct PlaceholderData {
    pub entity: EntityIdx,
    pub unit_id: String,
    pub blueprint: Option<Guid>,
    pub group_id: Option<String>,
    pub position: [f32; 3],
    pub orientation: f32,
    pub is_in_game: bool,
    pub is_revealed: bool,
    pub is_finally_dead: bool,
    pub is_extra: bool,
    pub race: Option<Guid>,
    pub faction: Option<Guid>,
    pub pools: Vec<Guid>,
    /// A saved `m_Roaming` (`UnitPartRoaming`): `BecomeUnit` gives the unit the part and
    /// `RestoreFrom` copies it.
    pub roaming: bool,
    /// The saved roaming state's `IdleCutscene` (a cutscene player mid-way) is set.
    pub roaming_idle_cutscene: bool,
    /// The saved roaming state's next point is a `RoamingPoint` of its own (`RollNextPoint`'s)
    /// with an idle cutscene.
    pub roaming_next_idle_cutscene: bool,
    /// `m_VendorTable` (`UnitPartVendor.SharedInventory`).
    pub vendor_table: Option<Guid>,
    /// `m_PrefabId` is set (`UnitSpawningData.PrefabGuid`): `EntityCreator.SpawnUnit` then
    /// selects no customization variation.
    pub has_prefab_id: bool,
    /// `m_PrefabId`, when set: the prefab of the unit's view.
    pub prefab_id: Option<String>,
    /// `m_IsOptimizedHide` (`UnitSpawningData.SimplifyWhenHidden`).
    pub optimized_hide: bool,
    pub body: Option<Body>,
    pub inventory: Vec<EntityIdx>,
    pub inventory_collection: Option<usize>,
    /// Saved members `BecomeUnit` passes on that aren't modelled (reported when present).
    pub unmodelled: Vec<String>,
}

/// An entry of a scene state's entity list, in order.
#[derive(Debug, Clone)]
pub enum AreaEntry {
    Unit(EntityIdx),
    Placeholder(Box<PlaceholderData>),
    /// A scene entity that is not a unit (script zone, map object, cutscene player, unit
    /// group, locator…).
    Static(EntityIdx),
}

/// The components this module ports.
pub fn register(r: &mut Registry) {
    use crate::components::{Port, bits};
    r.register(Box::new(Port::new(
        "AddClassLevels",
        bits(&[Hook::Activate, Hook::TurnOn, Hook::TurnOff]),
        add_class_levels,
    )));
}

/// `AddClassLevels`: `OnActivate` levels the unit up automatically (unless
/// `DoNotApplyAutomatically`, once: `Data.Applied`); `OnTurnOn`/`OnTurnOff` keep the
/// level-up plan provider list.
fn add_class_levels(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = crate::components::owner_unit(w, c) else { return };
    match h {
        Hook::Activate => {
            let Some(manual) =
                crate::components::with_settings(w, c, |v| v.bool("DoNotApplyAutomatically"))
            else {
                return;
            };
            if manual {
                return;
            }
            let applied = w.component(c).data.as_ref().is_some_and(|d| d["Applied"] == true);
            if applied {
                return;
            }
            let mut d = w.component(c).data.clone().unwrap_or_else(|| serde_json::json!({}));
            d["Applied"] = serde_json::Value::Bool(true);
            w.component_mut(c).data = Some(d);
            levelup::add_class_levels(w, c, u);
            // `ILevelUpCompleteUIHandler.HandleLevelUpComplete(unit, false)`: the UI, and the
            // components that learn spells (`AddKnownSpell`, `LearnSpellList`).
            crate::rulebook::bus::raise_global_call(
                w,
                "ILevelUpCompleteUIHandler",
                "HandleLevelUpComplete",
                Some(u),
            );
        }
        Hook::TurnOn => w.unit_mut(u).expect("unit").level_up_plan_providers.push(c),
        Hook::TurnOff => w.unit_mut(u).expect("unit").level_up_plan_providers.retain(|x| *x != c),
        _ => {}
    }
}

fn root(w: &World) -> Option<Rc<Blueprint>> {
    w.content.get(crate::unit::BLUEPRINT_ROOT)
}

/// `BlueprintRoot.Progression.<field>` / `SystemMechanics.<field>` references.
pub(crate) fn root_ref(w: &World, section: &str, field: &str) -> Option<Rc<Blueprint>> {
    let r = root(w)?;
    let g = r.view().obj(section)?.guid(field)?;
    w.content.get(g)
}

/// `UnitPlaceholderEntityData.BecomeUnit()`: the unit, or `None` when it isn't created.
pub fn become_unit(w: &mut World, p: &PlaceholderData) -> Option<EntityIdx> {
    for m in &p.unmodelled {
        w.coverage.unsupported(format!("UnitPlaceholderEntityData.BecomeUnit: saved {m}"));
    }
    let Some(bp) = p.blueprint.and_then(|g| w.content.get(g)) else {
        w.coverage.unsupported("BecomeUnit: unit blueprint missing");
        return None;
    };
    // `EntityCreator.SpawnUnit`: a customization preset selects a variation (random) unless
    // the spawning data names a prefab, as a placeholder's does.
    if bp.view().guid("m_CustomizationPreset").is_some() && !p.has_prefab_id {
        w.coverage.unsupported("SpawnUnit: customization preset");
    }
    let e = p.entity;
    // `BecomeUnit` requests `DoNotCreateItems`.
    let outer = w.build;
    let flags = std::mem::replace(&mut w.build, BuildFlags { do_not_create_items: true, ..outer });
    // `EntityCreator.SpawnUnit`: the view comes from the spawning data's prefab
    // (`m_PrefabId`), else the blueprint's, or from the optimization dummy when the unit spawns
    // hidden and simplified (`ShouldSpawnOptimizedDummy`; an area part's bounds aren't
    // modelled: reported).
    if crate::scene::area_has_parts(w) {
        w.coverage
            .unsupported("EntityCreationController.ShouldSpawnOptimizedDummy: area part bounds");
    }
    let prefab = if p.optimized_hide && !p.is_in_game {
        crate::scene_spawn::dummy_prefab(w)
    } else {
        p.prefab_id.clone().or_else(|| crate::scene_spawn::blueprint_prefab(&bp))
    };
    let in_game = crate::scene_spawn::prefab_in_game(w, prefab.as_deref());
    construct(w, e, &bp, Some(p), in_game);
    if let Some(u) = w.unit_mut(e) {
        u.view_prefab = prefab;
    }
    w.build = flags;
    // `SpawnEntityWithView`: `BeforeAttachView` → `SetupUnit`.
    setup_unit(w, e, p);
    // `AttachView` (presentation; `IUnitViewAttachedHandler` UI), `AddEntity` (queued),
    // `UnitPlaceOnGroundController.ForcedTick` (pose; T1). Then `SpawnUnit` sets
    // `IsInGame = !SpawnHidden` and `BecomeUnit` restores the placeholder's fields.
    // (`EntityCreationController.SpawnUnit`: `IsInGame = !SpawnHidden` → `OnIsInGameChanged`.)
    crate::entity::set_is_in_game(w, e, p.is_in_game);
    // `if (current.SimplifyWhenHidden) unitEntityData.Ensure<UnitPartOptimizeWhenHidden>()`
    // (a marker part `UpdateHasDummyView` reads).
    if p.optimized_hide && !w.entity(e).parts.iter().any(|x| x.clr == "UnitPartOptimizeWhenHidden")
    {
        w.entity_mut(e).parts.push(crate::world::Part { clr: "UnitPartOptimizeWhenHidden" });
    }
    if let Some(u) = w.unit_mut(e) {
        u.group_id = p.group_id.clone();
        u.position = p.position;
        u.orientation = p.orientation;
    }
    // `SetStealthData(m_Stealth)` replaces the unit's fresh `UnitStealth`: a saved state
    // that differs from it is reported with the placeholder's unmodelled members.
    // `MarkExtra()`: `IsExtra` (an extra sleeps out of the camera's frustum: its awake state is
    // unknown where it matters, `awake_state`).
    if p.is_extra
        && let Some(u) = w.unit_mut(e)
    {
        u.is_extra = true;
    }
    // `unit.Ensure<UnitPartRoaming>().RestoreFrom(m_Roaming)`: the roaming state (next point,
    // idle cutscene and time, direction) steers the unit's movement between waypoints
    // (`RoamingController`), which is T1; the part has no other hook.
    if p.roaming {
        if !w.entity(e).parts.iter().any(|x| x.clr == "UnitPartRoaming") {
            w.entity_mut(e).parts.push(crate::world::Part { clr: "UnitPartRoaming" });
        }
        w.coverage.stub("UnitPartRoaming.RestoreFrom (roaming movement along waypoints: T1)");
        if p.roaming_idle_cutscene {
            w.coverage.unsupported("UnitPartRoaming.RestoreFrom: an idle cutscene playing");
        }
        if p.roaming_next_idle_cutscene {
            w.coverage.unsupported(ROAMING_NEXT_IDLE_CUTSCENE);
        }
    }
    // `unit.Ensure<UnitPartVendor>().SetSharedInventory(m_VendorTable)`: the part points at
    // the shared table (nothing is generated; the table's stock is the player's).
    if let Some(t) = p.vendor_table {
        if !w.entity(e).parts.iter().any(|x| x.clr == "UnitPartVendor") {
            w.entity_mut(e).parts.push(crate::world::Part { clr: "UnitPartVendor" });
        }
        w.scene.vendors.insert(e, t);
    }
    // `SummonPools.Register(pool, unit)` per saved pool, in order.
    for &pool in &p.pools {
        crate::scene::summon_pool_register(w, pool, e);
    }
    if let Some(f) = p.faction
        && Some(f) != w.unit(e).and_then(|u| u.faction)
    {
        w.coverage.unsupported("BecomeUnit: faction override (SwitchFactions)");
    }
    // `LootViewed = IsFinallyDead && !LootNew`; `Body.SyncEquipmentWithBlueprint()` when
    // the loot wasn't viewed: equipment already matches a placeholder's saved body.
    // `IUnitPlaceholder.HandlePlaceholderBecomeUnit`: not subscribed by sim types.
    Some(e)
}

/// `new UnitEntityData(uniqueId, isInGame, blueprint)` with `UnitSpawningData` from the
/// placeholder, or without it (`ChangeUnitBlueprint`, `Game.CreateUnitVacuum`): the entity
/// `e` becomes the unit, built with the `World::build` flags. `is_in_game` is the
/// constructor's `isInGame`: the view's `IsInGameBySettings` for `new UnitEntityData(view)`
/// ([`crate::scene_spawn::prefab_in_game`]), `true` for `CreateUnitVacuum`.
pub(crate) fn construct(
    w: &mut World,
    e: EntityIdx,
    bp: &Rc<Blueprint>,
    p: Option<&PlaceholderData>,
    is_in_game: bool,
) {
    let v = bp.view();
    // `EntityDataBase(uniqueId, isInGame)`: on, post-loaded, facts manager on, subscribed.
    {
        let ent = w.entity_mut(e);
        ent.is_turned_on = true;
        ent.post_load_executed = true;
        ent.facts.is_turned_on = true;
        ent.is_in_game = is_in_game;
        ent.subscribed = true;
    }
    crate::rulebook::bus::subscribe(w, crate::rulebook::bus::Sub::Entity(e));
    // `new UnitDescriptor(blueprint, this)`.
    let faction = v.guid("m_Faction");
    let player_faction = root(w).and_then(|r| r.view().guid("m_PlayerFaction"));
    let race_size =
        p.and_then(|p| p.race).and_then(|g| w.content.get(g)).map(|b| b.view().int("Size"));
    let size = race_size.or(Some(v.int("Size"))).and_then(Size::from_i64);
    let is_player = faction.is_some() && faction == player_faction;
    // `current?.Inventory ?? (IsPlayerFaction ? Player.EnsureInventory() : new ItemsCollection(this))`:
    // the player's shared inventory, or a collection of its own.
    let (inventory, inventory_collection) = match p {
        Some(p) => (p.inventory.clone(), p.inventory_collection),
        None if is_player => {
            let mc = w.main_character.and_then(|m| w.unit(m));
            (
                mc.map(|m| m.inventory.clone()).unwrap_or_default(),
                mc.and_then(|m| m.inventory_collection),
            )
        }
        None => {
            w.collection_seq += 1;
            (Vec::new(), Some(w.collection_seq))
        }
    };
    let saved_body = match p {
        Some(_) => w.unit(e).map(|u| u.body.clone()).unwrap_or_default(),
        None => Body::default(),
    };
    let unit = Unit {
        blueprint: Some(bp.guid),
        original_blueprint: Some(bp.guid),
        stats: Stats::fresh(),
        progression: Progression {
            character_level: Some(0),
            mythic_level: Some(0),
            ..Progression::default()
        },
        body: saved_body,
        body_pending: p.is_some_and(|p| p.body.is_some()),
        inventory,
        inventory_collection,
        is_player_faction: is_player,
        faction,
        original_size: size,
        // `new UnitProficiency()`: armor `None`; weapons UnarmedStrike, Bite, Claw, Gore,
        // Touch and Ray.
        proficiencies: crate::unit::Proficiencies {
            armor: vec![(0, 1)],
            weapons: [0, 64, 65, 66, 61, 62].iter().map(|c| (*c, 1)).collect(),
            ..crate::unit::Proficiencies::default()
        },
        ..Unit::default()
    };
    w.entity_mut(e).kind = EntityKind::Unit(Box::new(unit));
    // `new UnitCombatState(unit)`: its engagement empty (the count, set from stats not yet
    // initialized, is established at the unit's first engagement tick).
    if w.combat.engage.on
        && let Some(x) = w.unit_mut(e)
    {
        x.combat.eng = crate::combat::engage::UnitEngage { known: true, ..Default::default() };
    }
    // (`Clear()`: neither target.)
    if w.combat.targets.on
        && let Some(x) = w.unit_mut(e)
    {
        x.combat.tgt = crate::combat::targets::UnitTargets { known: true, ..Default::default() };
    }
    // (`new CombatAiData()`: no impatience state.)
    if w.combat.impatience.on
        && let Some(x) = w.unit_mut(e)
    {
        x.combat.imp =
            crate::combat::impatience::UnitImpatience { known: true, ..Default::default() };
    }
    // `new CharacterStats(descriptor)`: every stat's initial value goes through its
    // `BaseValue` setter (AC 10, attacks of opportunity 1), in `CollectAllStats` order.
    let types: Vec<StatType> = w.stats(e).all.iter().map(|s| s.ty).collect();
    for t in types {
        ops::update_value(w, e, t);
    }
    if p.is_none_or(|p| p.body.is_none()) {
        // `new UnitBody(descriptor)`: the slots and the empty-hand weapon.
        crate::items::new_body(w, e);
    }
    for c in bp.components_array() {
        if is_a(c.ty, "FixUnitOnPostLoad") {
            w.coverage.unsupported("UnitDescriptor: FixUnitOnPostLoad components");
        }
    }
    // `UnitProgressionData(unit)`: the root `FeatsProgression` feature first.
    if let Some(feats) = root_ref(w, "Progression", "m_FeatsProgression") {
        crate::unit_facts::add_fact(w, e, &feats);
    }
    initialize(w, e, bp, p);
    // `UnitEntityData.Initialize` after the descriptor: ragdoll, interactions,
    // `BonusExperience`, upgraders marked applied, then `IUnitInitializedHandler`.
    w.coverage.executed("BecomeUnit.UnitEntityData.Initialize");
    crate::xp::init_bonus_experience(w, e);
    crate::rulebook::bus::raise_unit(w, e, "IUnitInitializedHandler");
}

/// `UnitDescriptor.Initialize(unit, blueprint)`.
fn initialize(w: &mut World, e: EntityIdx, bp: &Rc<Blueprint>, p: Option<&PlaceholderData>) {
    let v = bp.view();
    // `unit.Ensure<LevelUpPlanUnitHolder>()`.
    w.unit_mut(e).expect("unit").plan_holder = true;
    ops::set_base_value(w, e, StatType::HitPoints, v.i32("MaxHP"));
    for (t, f) in [
        (StatType::Strength, "Strength"),
        (StatType::Dexterity, "Dexterity"),
        (StatType::Constitution, "Constitution"),
        (StatType::Intelligence, "Intelligence"),
        (StatType::Wisdom, "Wisdom"),
        (StatType::Charisma, "Charisma"),
    ] {
        let value = v.i32(f);
        if value < 1 {
            w.coverage.unsupported("UnitDescriptor.InitializeStat: RetainDisabled");
        } else {
            ops::set_base_value(w, e, t, value);
        }
    }
    // `Speed.Value`: `(int)m_Value` (a float, truncated: `conv.i4`, saturating on arm64).
    let speed =
        v.obj("Speed").map_or(0, |s| crate::csharp::arith::to_int(f64::from(s.f32("m_Value"))));
    ops::set_base_value(w, e, StatType::Speed, speed);
    if let Some(s) = v.obj("Skills") {
        for (t, f) in [
            (StatType::SkillMobility, "Acrobatics"),
            (StatType::SkillAthletics, "Physique"),
            (StatType::SkillPersuasion, "Diplomacy"),
            (StatType::SkillThievery, "Thievery"),
            (StatType::SkillPerception, "Perception"),
            (StatType::SkillStealth, "Stealth"),
            (StatType::SkillUseMagicDevice, "UseMagicDevice"),
            (StatType::SkillLoreNature, "LoreNature"),
            (StatType::SkillKnowledgeArcana, "KnowledgeArcana"),
            (StatType::SkillLoreReligion, "LoreReligion"),
            (StatType::SkillKnowledgeWorld, "KnowledgeWorld"),
        ] {
            ops::set_base_value(w, e, t, s.i32(f));
        }
    }
    let player = w.unit(e).is_some_and(|u| u.is_player_faction);
    if !player {
        w.unit_mut(e).expect("unit").has_own_inventory = true;
    }
    ensure_natural_proficiencies(w, e);
    // `Alignment.Initialize(blueprint.Alignment)` (`UnitDescriptor.cs:772`), before the
    // main fact and its features activate. `BlueprintUnit.Alignment` defaults to
    // TrueNeutral.
    // (0 is no `Alignment` value: the field's unresolved default.)
    let al = match v.int("Alignment") {
        0 => 1,
        x => x,
    };
    match crate::narrative::alignment::index_of_enum_value(al) {
        Some(i) => {
            w.unit_mut(e).expect("unit").alignment =
                Some(crate::narrative::alignment::UnitAlignment::initialized(i));
        }
        None => w.coverage.unsupported("UnitAlignment.Initialize: unknown alignment value"),
    }
    // `UnitSpawningData`: race, voice, gender, handedness, prefab; the placeholder's saved
    // body and inventory (owner links, `Body.PrePostLoad`).
    if let Some(race) = p.and_then(|p| p.race) {
        levelup::set_race(w, e, race);
    }
    if w.unit(e).is_some_and(|u| u.progression.race.is_none())
        && let Some(race) = v.guid("m_Race")
    {
        levelup::set_race(w, e, race);
    }
    // `State.Size = OriginalSize` (the race's, else the blueprint's) → `UpdateSizeModifiers`.
    let race_size = w
        .unit(e)
        .and_then(|u| u.progression.race)
        .and_then(|g| w.content.get(g))
        .map(|b| b.view().int("Size"));
    let size = race_size.unwrap_or(v.int("Size"));
    let u = w.unit_mut(e).expect("unit");
    u.original_size = Size::from_i64(size);
    u.state_size = u.original_size;
    update_size_modifiers(w, e);
    // `AttackFactions.UnionWith(blueprint.AttackFactions)`.
    let attack = crate::combat::units::blueprint_attack_factions(w, bp);
    w.unit_mut(e).expect("unit").attack_factions = attack;
    // `MainFact = Facts.Add(new MainUnitFact(blueprint))`.
    let main =
        crate::facts::lifecycle::new_fact(w, FactKind::MainUnitFact, "MainUnitFact", bp.clone());
    let main = crate::unit_facts::add(w, e, main);
    w.unit_mut(e).expect("unit").main_fact = main;
    if let Some(charge) = root_ref(w, "SystemMechanics", "m_ChargeAbility") {
        crate::unit_facts::add_fact(w, e, &charge);
    }
    for c in bp.components_array() {
        if is_a(c.ty, "AddAbilityToCharacterComponent")
            || is_a(c.ty, "AddActivatableAbilityComponent")
        {
            w.coverage
                .unsupported("UnitDescriptor.Initialize: ability components on the unit blueprint");
        }
        if c.ty != wr_content::schema::NO_TYPE
            && wr_content::schema::ty(c.ty)
                .handlers
                .iter()
                .any(|h| h.contains("IHandleInventoryInitialized"))
        {
            w.coverage.unsupported("UnitDescriptor.Initialize: IHandleInventoryInitialized");
        }
    }
    for g in v.guids("m_AddFacts") {
        if let Some(f) = w.content.get(g) {
            crate::unit_facts::add_fact(w, e, &f);
        }
    }
    if !v.guids("m_AdditionalTemplates").is_empty() {
        w.coverage.unsupported("UnitDescriptor.ApplyTemplate");
    }
    // `Body.Initialize()` unless a saved body came with the spawning data;
    // `AddStartingInventory` unless the spawning data carries an inventory or comes from a
    // placeholder (`ChargenUnit` units skip both).
    if p.is_none_or(|p| p.body.is_none()) {
        crate::items::body_initialize(w, e);
    }
    if p.is_none() {
        crate::items::add_starting_inventory(w, e, bp);
    }
    // `new UnitBrain(descriptor)` (`SetBrain(Blueprint.DefaultBrain)`), then
    // `Brain.RestoreAvailableActions()`: the AI's action list (`crate::combat::ai`).
    let brain = crate::combat::ai::new_brain(w, e, bp.view().guid("m_Brain"));
    w.unit_mut(e).expect("unit").brain = Some(brain);
    // `unit.Remove<LevelUpPlanUnitHolder>()`: `OnDispose` disposes the plan unit
    // (`LevelUpPlanUnitHolder.cs:30`): its subscriptions, facts and registry entry go.
    if let Some(plan) = w.unit_mut(e).and_then(|u| u.plan_unit.take()) {
        crate::entity::dispose(w, plan);
    }
    w.unit_mut(e).expect("unit").plan_holder = false;
    w.unit_mut(e).expect("unit").descriptor_turned_on = true;
    // `foreach (Spellbook spellbook in unit.Spellbooks) spellbook.UpdateAllSlotsSize(true)`.
    for sb in crate::spellbook::spellbooks_of(w, e) {
        crate::spellbook::update_all_slots_size(w, e, sb, true);
    }
    replenish(w, e);
}

/// `UnitDescriptor.EnsureNaturalProficiencies()`: every natural weapon category
/// (`WeaponSubCategory.Natural.GetAllCategories()`, in `WeaponCategoryExtension.Data`
/// order), added to the proficiency multiset if missing.
fn ensure_natural_proficiencies(w: &mut World, e: EntityIdx) {
    // Bite, Claw, Gore, Tail, Hoof, Slam, Spike, Sting, Talon, Tentacle, Wing,
    // OtherNaturalWeapons.
    const NATURAL: [i64; 12] = [64, 65, 66, 75, 76, 80, 81, 79, 77, 78, 82, 67];
    let set = &mut w.unit_mut(e).expect("unit").proficiencies.weapons;
    for c in NATURAL {
        if !set.iter().any(|(k, _)| *k == c) {
            set.push((c, 1));
        }
    }
}

/// `UnitState.Size`'s getter: `m_Size`, set to `OriginalSize` on first read (`None` for a unit
/// without either). A size part loaded from a save (its facts and `OnPostLoad` aren't hydrated)
/// is reported.
pub fn state_size(w: &mut World, e: EntityIdx) -> Option<Size> {
    if w.unit(e)?.size_part_from_save {
        w.coverage.unsupported("UnitPartSizeModifier from a save (not hydrated)");
    }
    let u = w.unit_mut(e)?;
    if u.state_size.is_none() {
        u.state_size = u.original_size;
    }
    u.state_size
}

/// `UnitState.Size`'s setter: `m_SizeBeforeDeath` kept for a dead unit, then
/// `UpdateSizeModifiers` and `IUnitSizeHandler.HandleUnitSizeChanged` (the view's scale, the UI).
pub fn set_state_size(w: &mut World, e: EntityIdx, size: Size) {
    let dead = w.unit(e).is_some_and(|u| u.life_state == 2);
    let Some(u) = w.unit_mut(e) else { return };
    if dead && u.size_before_death.is_none() {
        u.size_before_death = u.state_size;
    }
    u.state_size = Some(size);
    update_size_modifiers(w, e);
    crate::rulebook::bus::raise_global_call(
        w,
        "IUnitSizeHandler",
        "HandleUnitSizeChanged",
        Some(e),
    );
}

/// `UnitPartSizeModifier.UpdateSize()` (`Kingmaker.UnitLogic.Parts/UnitPartSizeModifier.cs`): the
/// last size-changing fact's first `Polymorph` or `ChangeUnitSize` component sets the size; with
/// none left the size returns to `OriginalSize` and the part goes; a fact without such a
/// component is dropped (an error) and the next one tried.
pub fn update_size_part(w: &mut World, e: EntityIdx) {
    loop {
        let Some(f) = w.unit(e).and_then(|u| u.size_change_facts.last().copied()) else {
            let original = w.unit(e).and_then(|u| u.original_size);
            if let Some(s) = original {
                set_state_size(w, e, s);
            }
            w.entity_mut(e).parts.retain(|p| p.clr != "UnitPartSizeModifier");
            return;
        };
        let mut size = None;
        for k in 0..w.fact(f).components.len() {
            let c = crate::world::CompRef { fact: f, comp: k };
            let ty = w.component(c).ty().map(|t| wr_content::schema::ty(t).short);
            match ty {
                Some("Polymorph") => {
                    w.coverage.unsupported("UnitPartSizeModifier: a Polymorph's size");
                    return;
                }
                Some("ChangeUnitSize") => {
                    let Some((value_type, delta, value)) =
                        crate::components::with_settings(w, c, |v| {
                            (v.int("m_Type"), v.i32("SizeDelta"), v.int("Size"))
                        })
                    else {
                        continue;
                    };
                    // `GetUnitSize`: `Delta` (0) `OriginalSize.Shift(SizeDelta, Fine)`, `Value` (1)
                    // the size.
                    size = match value_type {
                        0 => w.unit(e).and_then(|u| u.original_size).map(|o| shift_size(o, delta)),
                        1 => Size::from_i64(value),
                        _ => {
                            w.coverage.unsupported("ChangeUnitSize: an unknown change type");
                            return;
                        }
                    };
                }
                _ => size = None,
            }
            if let Some(s) = size {
                set_state_size(w, e, s);
                break;
            }
        }
        if size.is_some() {
            return;
        }
        w.log("error", "Invalid fact (has no ChangeUnitSize component)");
        if let Some(u) = w.unit_mut(e) {
            u.size_change_facts.pop();
        }
    }
}

/// `Size.Shift(shift, Size.Fine, Size.Colossal)`.
fn shift_size(s: Size, delta: i32) -> Size {
    let i = (s as i32).wrapping_add(delta).clamp(Size::Fine as i32, Size::Colossal as i32);
    Size::from_i64(i64::from(i)).unwrap_or(s)
}

/// `UnitDescriptor.UpdateSizeModifiers()` (`WeaponSizeExtension.GetModifiers` of `State.Size`).
pub fn update_size_modifiers(w: &mut World, e: EntityIdx) {
    use crate::stats::types::ModifierDescriptor as D;
    for t in
        [StatType::AdditionalAttackBonus, StatType::AC, StatType::SkillStealth, StatType::Reach]
    {
        ops::remove_modifiers(w, e, t, D::Size);
    }
    let size = state_size(w, e);
    // (AttackAndAC, Stealth, Reach) per size, Fine … Colossal.
    let m = match size {
        Some(Size::Fine) => (8, 16, 0),
        Some(Size::Diminutive) => (4, 12, 0),
        Some(Size::Tiny) => (2, 8, 0),
        Some(Size::Small) => (1, 4, 5),
        Some(Size::Medium) => (0, 0, 5),
        Some(Size::Large) => (-1, -4, 9),
        Some(Size::Huge) => (-2, -8, 12),
        Some(Size::Gargantuan) => (-4, -12, 15),
        Some(Size::Colossal) => (-8, -16, 18),
        None => return,
    };
    ops::add_modifier_value(w, e, StatType::AdditionalAttackBonus, m.0, D::Size);
    ops::add_modifier_value(w, e, StatType::AC, m.0, D::Size);
    ops::add_modifier_value(w, e, StatType::SkillStealth, m.1, D::Size);
    ops::add_modifier_value(w, e, StatType::Reach, m.2, D::Size);
    // `IUnitSizeHandler.HandleUnitSizeChanged`: view scale and UI.
}

/// `EntityCreationController.ChangeUnitBlueprint(unit, newBlueprint, toCrossState)`
/// (`Kingmaker.Controllers/EntityCreationController.cs:284`): the old unit is marked for
/// destroy, the view gets a new unique id, and a new unit is built from the new blueprint
/// (no spawning data: `Body.Initialize` and the starting inventory run), takes the old
/// unit's place and joins its state at the next `EntityCreationController.Tick`.
pub fn change_unit_blueprint(
    w: &mut World,
    old: EntityIdx,
    new_blueprint: wr_content::Guid,
    to_cross_state: bool,
) -> Option<EntityIdx> {
    let Some(bp) = w.content.get(new_blueprint) else {
        w.coverage.unsupported("ChangeUnitBlueprint: blueprint missing");
        return None;
    };
    // `unit.MarkForDestroy()`; `unit.Pets.ForEach(MarkForDestroy)`.
    w.player.destroy_marks.push(old);
    if w.entity(old).parts.iter().any(|p| p.clr == "UnitPartPetMaster") {
        w.coverage.unsupported("ChangeUnitBlueprint: the replaced unit's pets");
    }
    // `unit.DetachView()` (presentation); `view.UniqueId = GetNewUniqueId()`.
    let id = crate::narrative::Player::new_unique_id(w);
    let e = w.add_entity(crate::world::Entity {
        unique_id: id,
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
    w.coverage.context = Some(w.entity(e).unique_id.clone());
    // `new UnitEntityData(view)` with the old unit's view: no `UnitSpawningData`, no
    // `DoNotCreateItems`; the unit starts with the view's `IsInGameBySettings` and nothing sets
    // `IsInGame` after.
    let prefab = current_view_prefab(w, old);
    let in_game = crate::scene_spawn::prefab_in_game(w, prefab.as_deref());
    let outer = std::mem::take(&mut w.build);
    construct(w, e, &bp, None, in_game);
    w.build = outer;
    if let Some(u) = w.unit_mut(e) {
        u.view_prefab = prefab;
    }
    // `unitEntityData.Position = unit.Position; Orientation = unit.Orientation`.
    let (position, orientation) =
        w.unit(old).map(|u| (u.position, u.orientation)).unwrap_or_default();
    if let Some(u) = w.unit_mut(e) {
        u.position = position;
        u.orientation = orientation;
    }
    w.player.creation_queue.push((e, to_cross_state));
    w.coverage.context = None;
    Some(e)
}

/// `EntityCreationController.Tick()`: the queued units join their states, then
/// `IUnitHandler`/`IUnitSpawnHandler.HandleUnitSpawned` (not for a placeholder's unit).
pub fn entity_creator_tick(w: &mut World) {
    // `for (i = 0; i < m_ToCreate.Count; i++)` then `m_ToCreate.Clear()` (`:366–408`): entries
    // a spawn handler appends are created in this same tick.
    let mut i = 0;
    while i < w.player.creation_queue.len() {
        let (e, cross) = w.player.creation_queue[i];
        i += 1;
        if cross {
            w.player.cross_scene_units.push(e);
        } else {
            w.player.area_units.push(e);
        }
        w.player.joined_after_load.retain(|u| *u != e);
        w.player.joined_after_load.push(e);
        // `UnitGroupsController` (subscribed when the controllers start, before the area's
        // facts) is the first `IUnitHandler`.
        crate::combat::groups::spawned(w, e);
        crate::rulebook::bus::raise_global_call(w, "IUnitHandler", "HandleUnitSpawned", Some(e));
        crate::rulebook::bus::raise_global_call(
            w,
            "IUnitSpawnHandler",
            "HandleUnitSpawned",
            Some(e),
        );
    }
    w.player.creation_queue.clear();
}

/// `EntityDestructionController.Tick()` (`Kingmaker.Controllers/EntityDestructionController.cs`):
/// the entities marked for destroy, in their states' order (the loaded area's, then the
/// cross-scene state), are destroyed and disposed, leave their state, and
/// `IEntityDestructionHandler` is raised.
pub fn entity_destroyer_tick(w: &mut World) {
    let marks = std::mem::take(&mut w.player.destroy_marks);
    if marks.is_empty() {
        return;
    }
    let mut ordered: Vec<EntityIdx> = Vec::new();
    for u in w.player.area_units.iter().chain(w.player.cross_scene_units.iter()) {
        if marks.contains(u) && !ordered.contains(u) {
            ordered.push(*u);
        }
    }
    for m in &marks {
        if !ordered.contains(m) {
            ordered.push(*m);
        }
    }
    for e in ordered {
        if w.unit(e).is_some_and(|u| u.companion_state.is_some()) {
            // `TryUnrecruit`: a companion's destruction is cancelled.
            w.coverage.unsupported("EntityDestructionController: a companion marked for destroy");
            continue;
        }
        let outer = w.coverage.context.take();
        w.coverage.context = Some(w.entity(e).unique_id.clone());
        // `entity.Destroy()`: `OnDestroy` hands the inventory to dropped loot if asked
        // (`UnitEntityData.cs:1718`), then raises `HandleUnitDestroyed`.
        if w.unit(e).is_some_and(|u| u.has_dropped_loot_ref) {
            w.coverage.unsupported("UnitEntityData.OnDestroy: TransferInventoryToDroppedLoot");
        }
        crate::combat::groups::destroyed(w, e);
        crate::rulebook::bus::raise_global_call(w, "IUnitHandler", "HandleUnitDestroyed", Some(e));
        // `entity.Dispose()`.
        crate::entity::dispose(w, e);
        // `HoldingState.RemoveEntityData(entity)`, `AwakeUnits.Remove`.
        w.player.area_units.retain(|u| *u != e);
        w.player.cross_scene_units.retain(|u| *u != e);
        w.entity_mut(e).is_in_game = false;
        crate::rulebook::bus::raise_global_call(
            w,
            "IEntityDestructionHandler",
            "HandleEntityDestruction",
            Some(e),
        );
        w.coverage.context = outer;
    }
}

/// `UnitHelper.Replenish(unit)`: weariness, negative effects (player units only),
/// resources, spellbooks (`ISpellBookRest`), the brain's available actions. These units
/// have no resources or spellbooks.
pub fn replenish(w: &mut World, e: EntityIdx) {
    // `CanHealNegativeEffects`: a player-faction non-pet unit; the callers have no
    // negative effects to remove (recruiting reports its own).
    crate::resources::restore_all(w, e);
    crate::spellbook::rest_unit(w, e);
}

/// `UnitPlaceholderEntityData.SetupUnit(unit)` (`BeforeAttachView`): life state from the
/// placeholder, prone, then the saved body's `PostLoad`, the inventory's `PostLoad`, the
/// body's fixes, and both turned on.
fn setup_unit(w: &mut World, e: EntityIdx, p: &PlaceholderData) {
    if p.is_finally_dead {
        w.coverage.unsupported("SetupUnit: finally dead placeholder");
    }
    // `State.Prone.Active = UnitProneController.ShouldBeProne(unit)`: not prone.
    if let Some(u) = w.unit_mut(e) {
        u.body_pending = false;
    }
    let mut items: Vec<EntityIdx> = crate::unit::body_items(w, e);
    for it in &items {
        if !w.entity(*it).post_load_executed {
            crate::entity::post_load(w, *it);
        }
    }
    for it in w.unit(e).map(|u| u.inventory.clone()).unwrap_or_default() {
        if !w.entity(it).post_load_executed {
            crate::entity::post_load(w, it);
        }
        if !items.contains(&it) {
            items.push(it);
        }
    }
    // `UnitBody.ApplyPostLoadFixes` (empty-hand weapon, additional limbs): not the items'
    // own `ApplyPostLoadFixes`, which the scene's fix pass never reaches for a new unit.
    for it in &items {
        if !w.entity(*it).is_turned_on {
            crate::entity::turn_on(w, *it);
        }
    }
}

/// After the area loaded: placeholders' fixes (unit upgraders), the creation queue's
/// units joining the scene state, then the activatable-abilities controller's pass over the
/// spawned units the first tick keeps awake (`UnitActivatableAbilitiesController.cs:76`: a
/// toggle that is on but not started, activated immediately and not waiting for combat, is
/// started).
pub fn after_area_load(w: &mut World, order: &[AreaEntry], spawned: &[EntityIdx]) {
    // `UnitUpgraderHelper.ApplyUpgraders(unit, m_Blueprint, fromPlaceholder: true,
    // ref m_AppliedUpgraders)`: the blueprint's upgraders meant for placeholders that the
    // placeholder hasn't applied would run (the saved list is empty or absent for these).
    for entry in order {
        if let AreaEntry::Placeholder(p) = entry
            && let Some(bp) = p.blueprint.and_then(|g| w.content.get(g))
        {
            for o in bp.components_array() {
                if !is_a(o.ty, "UnitUpgraderComponent") {
                    continue;
                }
                for g in crate::content::View(o).guids("m_Upgraders") {
                    if w.content.get(g).is_some_and(|u| u.view().bool("ApplyFromPlaceholder")) {
                        w.coverage.unsupported(
                            "UnitPlaceholderEntityData.OnApplyPostLoadFixes: unit upgraders",
                        );
                    }
                }
            }
        }
    }
    // (The first tick's `SleepingUnitsController` runs before the controller: only the units it
    // keeps awake tick. An extra sleeps out of the camera's frustum, which the sim doesn't
    // know: it acts on nothing and is reported if it would (as in the controller's tick; the
    // extras of Manual_10's crowds sleep). Another unit in the game is taken as awake, as the
    // recordings have it (its fog of war isn't modelled).)
    let party = crate::narrative::party::character_lists(w).party_and_pets;
    for &e in spawned {
        let extra = w.unit(e).is_some_and(|x| x.is_extra);
        let mode = match awake_after_spawn(w, e, &party) {
            Awake::No => continue,
            Awake::Unknown if extra => TickMode::AreaUnit,
            Awake::Yes => TickMode::StartPass,
            Awake::Unknown => TickMode::StartPassAssumed,
        };
        w.coverage.context = Some(w.entity(e).unique_id.clone());
        activatable_abilities_unit_tick(w, e, false, mode);
    }
    w.coverage.context = None;
}

/// The prefab a unit's current view was instantiated from: the one it was spawned with in
/// this run, else `UnitEntityData.CreateView` for a loaded unit (`Descriptor.CustomPrefabGuid`,
/// else `Blueprint.Prefab`). A polymorph's or a doll's view, and a view that may have been
/// swapped for the optimization dummy (`UpdateHasDummyView`: suppression, or
/// `UnitPartOptimizeWhenHidden`), aren't followed: `None`, reported.
fn current_view_prefab(w: &mut World, u: EntityIdx) -> Option<String> {
    let has = |w: &World, p: &str| w.entity(u).parts.iter().any(|x| x.clr == p);
    if has(w, "UnitPartOptimizeWhenHidden") || crate::scene::area_has_parts(w) {
        w.coverage.unsupported("UnitEntityData.View: optimization dummy view (UpdateHasDummyView)");
        return None;
    }
    if let Some(p) = w.unit(u).and_then(|x| x.view_prefab.clone()) {
        return Some(p);
    }
    let polymorph = w.entity(u).facts.list.iter().any(|f| {
        let f = w.fact(*f);
        f.kind == FactKind::Buff
            && f.is_active
            && f.bp.as_ref().is_some_and(|b| crate::unit_facts::has_component(b, "Polymorph"))
    });
    if polymorph || has(w, "UnitPartDollData") {
        w.coverage.unsupported("UnitEntityData.CreateView: polymorph or doll view");
        return None;
    }
    let x = w.unit(u)?;
    if let Some(c) = x.custom_prefab_guid.clone() {
        return Some(c);
    }
    x.blueprint
        .and_then(|g| w.content.get(g))
        .and_then(|b| crate::scene_spawn::blueprint_prefab(&b))
}

/// What `SleepingUnitsController.ShouldBeSleeping` says of a unit, as far as the sim can
/// establish it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Awake {
    Yes,
    No,
    Unknown,
}

/// `CurrentlyLoadedArea.HideParty` (`BlueprintArea.HideParty => IsGlobalMap`, overridden by
/// the tactical-combat and settlement areas).
fn area_hides_party(w: &World) -> bool {
    w.player.area.and_then(|g| w.content.get(g)).is_some_and(|a| {
        a.view().bool("IsGlobalMap")
            || is_a(a.obj.ty, "BlueprintTacticalCombatArea")
            || is_a(a.obj.ty, "SettlementBlueprintArea")
    })
}

/// `!SleepingUnitsController.ShouldBeSleeping(unit)` (`Kingmaker.Controllers/
/// SleepingUnitsController.cs`), in its order: a unit out of the game, of a fake blueprint or
/// suppressed sleeps (suppression, only in an area with parts, is reported); a companion sleeps
/// while the area hides the party and is awake in a pit; then `AwakeTimer`/`FadeAwakeTimer`
/// keep a unit awake. Those are set by `Unit.Wake`, which view fades (`EntityFader`), proning
/// and death call and the sim doesn't follow, so a branch that would put the unit to sleep
/// after them is unknown: a finally dead unit; a sleepless unit or one in combat is awake; an extra unit depends
/// on the camera; a party member (`PartyAndPets`) isn't in the fog of war (the party reveals it),
/// so it is awake whatever the timers; anything else depends on the fog of war, cutscene control
/// and the unit's commands: unknown.
/// `State.AwakeUnits` as `UnitActivatableAbilitiesController` read it in a recording
/// (WrathOracle's `aa_awake`, one per run of the controller: the whole set first, then the
/// changes; a unit created in the run listed under `new` with its blueprint), taken as an input
/// where the sim can't establish a unit's state (fog of war, the camera).
#[derive(Debug, Default)]
pub struct RecordedAwake {
    /// The awake unit ids per oracle tick in which the controller ran.
    pub sets: std::collections::BTreeMap<i64, std::collections::BTreeSet<String>>,
    /// A run-created unit's recorded id and blueprint.
    pub new_bp: std::collections::HashMap<String, Guid>,
    /// Recorded run-created ids matched to the sim's units.
    pub mapped: std::collections::HashMap<String, EntityIdx>,
    /// Ids the save holds (not created in the run).
    pub saved: std::collections::HashSet<String>,
    /// The tick being run: the units the controller ticks.
    pub now: Option<std::collections::HashSet<EntityIdx>>,
}

impl RecordedAwake {
    /// From a recording's records: `Ok(None)` without `aa_awake` records.
    ///
    /// # Errors
    ///
    /// A malformed record (fields of the wrong type, changes before the first set, two in one
    /// tick, one before any tick).
    pub fn of(
        records: &[serde_json::Value],
        saved: std::collections::HashSet<String>,
    ) -> std::result::Result<Option<Self>, String> {
        let strs =
            |v: &serde_json::Value, what: &str| -> std::result::Result<Vec<String>, String> {
                v.as_array()
                    .ok_or_else(|| format!("aa_awake: {what} isn't an array"))?
                    .iter()
                    .map(|x| {
                        x.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| format!("aa_awake: {what} entry {x}"))
                    })
                    .collect()
            };
        let mut out = Self { saved, ..Self::default() };
        let mut tick: Option<i64> = None;
        let mut cur: Option<std::collections::BTreeSet<String>> = None;
        let mut any = false;
        for r in records {
            if r["k"] == "tick" {
                tick = Some(r["t"].as_i64().ok_or("a tick record without t")?);
                continue;
            }
            if r["k"] != "obs" || r["ev"] != "aa_awake" {
                continue;
            }
            any = true;
            let t = tick.ok_or("aa_awake before any tick")?;
            if out.sets.contains_key(&t) {
                return Err(format!("two aa_awake records in tick {t}"));
            }
            if r.get("set").is_some() {
                if cur.is_some() {
                    return Err(format!("aa_awake: a second whole set in tick {t}"));
                }
                cur = Some(strs(&r["set"], "set")?.into_iter().collect());
            } else {
                let c = cur.as_mut().ok_or("aa_awake: changes before the whole set")?;
                for id in strs(&r["on"], "on")? {
                    c.insert(id);
                }
                for id in strs(&r["off"], "off")? {
                    if !c.remove(&id) {
                        return Err(format!("aa_awake: {id} off but not awake"));
                    }
                }
            }
            for n in r["new"].as_array().ok_or("aa_awake: new isn't an array")? {
                let (Some(id), Some(bp)) =
                    (n[0].as_str(), n[1].as_str().and_then(wr_content::Guid::parse))
                else {
                    return Err(format!("aa_awake: new entry {n}"));
                };
                if out.new_bp.insert(id.to_owned(), bp).is_some() {
                    return Err(format!("aa_awake: {id} announced twice"));
                }
            }
            out.sets.insert(t, cur.clone().unwrap_or_default());
        }
        Ok(any.then_some(out))
    }
}

/// Before the controllers of the oracle tick `tick`: the recorded awake units for it, each id
/// matched to a live sim unit (a saved id as is, a run-created one by its blueprint, the match
/// unique). A tick in which the recording's controller didn't run, or an id the sim can't
/// match, is reported.
/// A unit the recording's controllers can visit: in the state's pool (not queued for creation),
/// live, indexed, not a preview.
fn recorded_live(w: &World, pool: &std::collections::HashSet<EntityIdx>, e: EntityIdx) -> bool {
    pool.contains(&e)
        && !w.entity(e).is_disposed
        && w.by_id.get(&w.entity(e).unique_id) == Some(&e)
        && w.unit(e).is_some_and(|x| x.preview_of.is_none())
}

impl RecordedAwake {
    /// The sim's unit for a recorded id: a saved id as is, a run-created one (announced with its
    /// blueprint) through its match, made here when it isn't yet (the one live unit the run
    /// created with that blueprint, not matched to another id); `None` when there is none.
    pub fn resolve(&mut self, w: &World, id: &str) -> Option<EntityIdx> {
        let pool: std::collections::HashSet<EntityIdx> =
            crate::scene::units_pool(w).into_iter().collect();
        if let Some(bp) = self.new_bp.get(id).copied() {
            return match self.mapped.get(id).copied() {
                Some(e) if recorded_live(w, &pool, e) => Some(e),
                Some(_) => None,
                None => {
                    let taken: Vec<EntityIdx> = self.mapped.values().copied().collect();
                    let candidates: Vec<EntityIdx> = (0..w.entities.len())
                        .map(|i| EntityIdx(u32::try_from(i).unwrap_or(u32::MAX)))
                        .filter(|e| {
                            recorded_live(w, &pool, *e)
                                && w.unit(*e).is_some_and(|x| x.blueprint == Some(bp))
                                && !self.saved.contains(&w.entity(*e).unique_id)
                                && !taken.contains(e)
                        })
                        .collect();
                    if let [e] = candidates[..] {
                        self.mapped.insert(id.to_owned(), e);
                        Some(e)
                    } else {
                        None
                    }
                }
            };
        }
        if self.saved.contains(id) {
            w.by_id.get(id).copied().filter(|e| recorded_live(w, &pool, *e))
        } else {
            None
        }
    }
}

pub fn recorded_awake_for_tick(w: &mut World, tick: i64) {
    let Some(mut rec) = w.recorded_awake.take() else { return };
    rec.now = None;
    match rec.sets.get(&tick).cloned() {
        None => w.coverage.unsupported("dialogue replay: no recorded awake state for a tick"),
        Some(set) => {
            let mut now = std::collections::HashSet::new();
            for id in set {
                let e = rec.resolve(w, &id);
                match e {
                    Some(e) => {
                        now.insert(e);
                    }
                    None => w
                        .coverage
                        .unsupported("dialogue replay: a recorded awake unit the sim can't match"),
                }
            }
            rec.now = Some(now);
        }
    }
    w.recorded_awake = Some(rec);
}

/// [`awake_state`], with the recording's state for the tick when the run has one: an unknown
/// state takes it; a known one must agree (else reported; the sim keeps its own, which its
/// world is consistent with).
fn awake_or_recorded(w: &mut World, u: EntityIdx, party: &[EntityIdx]) -> Awake {
    let sim = awake_state(w, u, party);
    let Some(on) = w.recorded_awake.as_ref().and_then(|r| r.now.as_ref()).map(|n| n.contains(&u))
    else {
        return sim;
    };
    match sim {
        Awake::Unknown => {
            if on {
                Awake::Yes
            } else {
                Awake::No
            }
        }
        s => {
            if (s == Awake::Yes) != on {
                w.coverage.unsupported(
                    "SleepingUnitsController: the sim's awake state differs from the recording's",
                );
            }
            s
        }
    }
}

pub fn awake_state(w: &mut World, u: EntityIdx, party: &[EntityIdx]) -> Awake {
    let Some(x) = w.unit(u) else { return Awake::No };
    if !w.entity(u).is_in_game || w.entity(u).is_disposed {
        return Awake::No;
    }
    if x.blueprint.and_then(|g| w.content.get(g)).is_some_and(|b| b.view().bool("IsFake")) {
        return Awake::No;
    }
    let (finally_dead, in_combat) = (x.finally_dead, x.combat.in_combat);
    let has = |p: &str| w.entity(u).parts.iter().any(|x| x.clr == p);
    let (companion, in_pit, summoned) =
        (has("UnitPartCompanion"), has("UnitPartInPit"), has("UnitPartSummonedMonster"));
    if crate::scene::area_has_parts(w) {
        w.coverage.unsupported("SleepingUnitsController: suppressed units (area parts)");
        return Awake::Unknown;
    }
    if companion {
        if area_hides_party(w) {
            return Awake::No;
        }
        if in_pit {
            return Awake::Yes;
        }
    }
    if finally_dead || summoned {
        return Awake::Unknown;
    }
    // `unit.Sleepless` (a `CountableFlag`: etudes, `UnitPartFollowUnit`) or combat: awake, before
    // an extra's camera check.
    let sleepless = w
        .player
        .counters
        .get(&format!("unit:{}.Sleepless", w.entity(u).unique_id))
        .is_some_and(|n| *n > 0);
    if sleepless || in_combat {
        return Awake::Yes;
    }
    if w.unit(u).is_some_and(|x| x.is_extra) {
        return Awake::Unknown;
    }
    if party.contains(&u) { Awake::Yes } else { Awake::Unknown }
}

/// `UnitActivatableAbilitiesController.Tick()` (`Kingmaker.Controllers.Units/
/// UnitActivatableAbilitiesController.cs`, registered before the script zones and the
/// dialogue in `GameModesFactory`): every awake unit's toggles. `Game.State.AwakeUnits` is the
/// units of `State.Units` that `SleepingUnitsController` keeps awake ([`awake_state`]): an awake
/// unit ticks, a sleeping one doesn't, and one whose state is unknown is reported when it has a
/// toggle the tick would act on (`TickMode::AreaUnit`).
pub fn activatable_abilities_tick(w: &mut World, dialog_or_cutscene: bool) {
    let party = crate::narrative::party::character_lists(w).party_and_pets;
    let pool = crate::scene::units_pool(w);
    for u in pool {
        let mode = match awake_or_recorded(w, u, &party) {
            Awake::No => continue,
            Awake::Yes => TickMode::Controller,
            Awake::Unknown => TickMode::AreaUnit,
        };
        let outer = w.coverage.context.take();
        w.coverage.context = Some(w.entity(u).unique_id.clone());
        activatable_abilities_unit_tick(w, u, dialog_or_cutscene, mode);
        w.coverage.context = outer;
    }
}

/// `UnitActivatableAbilitiesController.ShouldBeDisabled`. The owner-state clause
/// (`CanAct`, `IsAble`, `IsConscious`, `UnitPartPreventInterruption`) isn't ported and is
/// reported when the blueprint asks for it; then the Rest mode applies on its own.
fn activatable_should_be_disabled(w: &mut World, v: crate::content::View<'_>) -> bool {
    if v.bool("DeactivateIfOwnerDisabled") || v.bool("DeactivateIfOwnerUnconscious") {
        w.coverage.unsupported("UnitActivatableAbilitiesController: ShouldBeDisabled (unit state)");
    }
    // `Game.Instance.CurrentMode == GameModeType.Rest`.
    w.player.mode == "Rest" && !v.bool("DoNotTurnOffOnRest")
}

/// `ActivatableAbility.OnNewRound` (`ActivatableAbility.cs:390`): stops an ability that is
/// off, unavailable, due to deactivate, or whose combat ended; otherwise its resource and
/// unit-command logic runs, which isn't ported and is reported (`is_available` reports
/// restriction and resource components).
fn activatable_on_new_round(w: &mut World, f: crate::world::FactIdx, owner: EntityIdx) {
    let Some(abp) = w.fact(f).bp.clone() else { return };
    let v = abp.view();
    let in_combat = crate::combat::is_in_combat(w, owner);
    w.fact_mut(f).was_in_combat |= in_combat;
    let (on, _, _) = w.fact(f).activatable.unwrap_or((false, false, None));
    // `!IsAvailableByRestrictions`.
    let available = crate::unit_facts::is_available_by_restrictions(w, f);
    // `ActivateOnCombatStarts`: `DeactivateIfCombatEnded && !ActivateWithUnitCommand &&
    // !ActivateOnUnitAction`.
    let on_combat_starts = v.bool("DeactivateIfCombatEnded") && v.int("ActivationType") == 0;
    if w.fact(f).deactivate_next_round
        || !on
        || !available
        || (v.bool("DeactivateIfCombatEnded")
            && !in_combat
            && (on_combat_starts || w.fact(f).was_in_combat))
    {
        crate::unit_facts::stop(w, f);
    } else if crate::unit_facts::spends_resources(w, f)
        || crate::unit_facts::has_component(&abp, "ActivatableAbilityUnitCommand")
    {
        w.coverage.unsupported("ActivatableAbility.OnNewRound: resource or unit-command logic");
    }
    w.fact_mut(f).deactivate_next_round = v.bool("DeactivateAfterFirstRound");
}

/// Whether `ActivatableAbility.OnNewRound` can change nothing for this started toggle: it is
/// on, doesn't end with combat or after its first round, and has no restriction, resource
/// or unit-command logic.
fn activatable_round_is_plain(
    w: &mut World,
    f: crate::world::FactIdx,
    bp: &crate::content::Blueprint,
    on: bool,
) -> bool {
    let v = bp.view();
    on && !v.bool("DeactivateIfCombatEnded")
        && !v.bool("DeactivateAfterFirstRound")
        && v.guid("m_Resource").is_none()
        && !crate::unit_facts::has_restrictions(w, f)
        && !crate::unit_facts::spends_resources(w, f)
        && !crate::unit_facts::has_component(bp, "ActivatableAbilityUnitCommand")
}

/// How [`activatable_abilities_unit_tick`] runs.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TickMode {
    /// The controller's tick on a party unit.
    Controller,
    /// The start pass after an area loads, which isn't a controller tick: no rounds.
    StartPass,
    /// The start pass on a unit taken as awake whose state the sim can't establish (fog of war,
    /// cutscene control, commands): it acts as `StartPass`, and an action is reported.
    StartPassAssumed,
    /// An area unit, which may or may not be awake: acts on nothing, and reports the first
    /// toggle the tick would have acted on.
    AreaUnit,
}

/// `TickOnUnit`/`TickAbility` for one unit. Waiting for a target and turn-based round
/// timing aren't ported: they are reported when a toggle needs them.
fn activatable_abilities_unit_tick(
    w: &mut World,
    e: EntityIdx,
    dialog_or_cutscene: bool,
    mode: TickMode,
) {
    // `AreaUnit`: reports instead of acting.
    let area = mode == TickMode::AreaUnit;
    let assumed = |w: &mut World| {
        if mode == TickMode::StartPassAssumed && !w.assumed_awake_at_load.contains(&e) {
            w.assumed_awake_at_load.push(e);
        }
    };
    let mode = if mode == TickMode::StartPassAssumed { TickMode::StartPass } else { mode };
    let would_act = |w: &mut World| {
        w.coverage.unsupported(
            "UnitActivatableAbilitiesController: area unit with a toggle to tick (AwakeUnits)",
        );
    };
    let acts: Vec<_> = w
        .entity(e)
        .facts
        .list
        .iter()
        .copied()
        .filter(|f| w.fact(*f).kind == FactKind::ActivatableAbility)
        .collect();
    for f in acts {
        let (on, started, _) = w.fact(f).activatable.unwrap_or((false, false, None));
        let Some(abp) = w.fact(f).bp.clone() else { continue };
        let v = abp.view();
        if v.bool("IsTargeted") && on {
            w.coverage.unsupported("UnitActivatableAbilitiesController: targeted toggle");
            continue;
        }
        if area && !on && !started {
            continue;
        }
        let in_combat = crate::combat::is_in_combat(w, e);
        // `StopOutOfCombat`.
        if !dialog_or_cutscene
            && v.bool("DeactivateIfCombatEnded")
            && !in_combat
            && started
            && v.bool("DeactivateImmediately")
        {
            if area {
                would_act(w);
                return;
            }
            assumed(w);
            crate::unit_facts::stop(w, f);
            continue;
        }
        if started {
            if mode == TickMode::StartPass {
                continue;
            }
            if activatable_should_be_disabled(w, v) {
                if area {
                    would_act(w);
                    return;
                }
                crate::unit_facts::stop(w, f);
            } else if !dialog_or_cutscene {
                if area {
                    // A round of a plain toggle that is on changes nothing but its timer,
                    // which isn't tracked for a unit that may be asleep [I].
                    if !activatable_round_is_plain(w, f, &abp, on) {
                        would_act(w);
                        return;
                    }
                    continue;
                }
                if w.combat.turn_based {
                    // `GetTimeToNextRound`, `CanTickNewRound`, `IsCurrentUnit`.
                    w.coverage.unsupported(
                        "UnitActivatableAbilitiesController: round timer in turn-based combat",
                    );
                    continue;
                }
                // `TimeToNextRound -= GameDeltaTime`; outside turn-based combat nobody is
                // the current unit, and a new round can always tick.
                let t = w.fact(f).time_to_next_round - w.clock.game_delta_time();
                w.fact_mut(f).time_to_next_round = t;
                if t <= 0.0 {
                    activatable_on_new_round(w, f, e);
                    w.fact_mut(f).time_to_next_round = 6.0;
                }
            }
            continue;
        }
        let activation = v.int("ActivationType");
        let on_combat = v.bool("DeactivateIfCombatEnded") && activation == 0;
        let ready = w.fact(f).ready_to_start;
        if on
            && (activation == 0 || ready)
            && (!on_combat || in_combat)
            && !activatable_should_be_disabled(w, v)
        {
            if area {
                would_act(w);
                return;
            }
            assumed(w);
            crate::unit_facts::try_start(w, f);
        }
    }
}

/// Reports the start pass's assumption ([`World::assumed_awake_at_load`]: units taken as awake
/// whose sleep the sim can't establish, and whose toggles then acted), one report per unit.
pub fn report_assumed_awake(w: &mut World) {
    let outer = w.coverage.context.take();
    for e in std::mem::take(&mut w.assumed_awake_at_load) {
        w.coverage.context = Some(w.entity(e).unique_id.clone());
        w.coverage.unsupported(
            "SleepingUnitsController: a toggle acts on a unit taken as awake at load (suppression or fog of war unknown)",
        );
    }
    w.coverage.context = outer;
}

/// `ShouldBeSleeping` at the first tick after a unit was spawned at load. `CreateEntityData`
/// sets the new unit's `Position` (`new UnitEntityData(view) { Position = … }`), and the setter
/// wakes it on a change from the default origin (`Wake()`: `AwakeTimer = 0`, which the
/// controller counts down only after this tick), so the checks after the timer (death, combat,
/// an extra's camera, cutscene control, the fog of war, commands) don't apply: such a unit is
/// awake unless one before it puts it to sleep (out of the game, a fake blueprint, suppressed:
/// [`awake_state`]'s, which report suppression in an area with parts as unknown; a companion
/// while the party is hidden). A unit at the origin isn't woken: [`awake_state`].
fn awake_after_spawn(w: &mut World, u: EntityIdx, party: &[EntityIdx]) -> Awake {
    let woken = w.unit(u).is_some_and(|x| {
        // `Vector3 !=`: a squared distance from the origin of at least 1e-10.
        let [a, b, c] = x.position.map(f64::from);
        a * a + b * b + c * c >= 9.999_999_44e-11
    });
    let state = awake_state(w, u, party);
    if !woken || state == Awake::No || crate::scene::area_has_parts(w) {
        return state;
    }
    Awake::Yes
}
