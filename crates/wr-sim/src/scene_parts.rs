//! Map objects of the area's scene state and their parts (decompiled, not copied:
//! `Kingmaker.EntitySystem.Entities/MapObjectEntityData.cs`,
//! `Kingmaker.EntitySystem/{StaticEntityData,EntityDataBase,EntityPartsManager,EntityPart}.cs`,
//! `Kingmaker.EntitySystem.Persistence.Scenes/SceneLoader.cs` (`MatchStateWithScene`,
//! `LoadSceneEntities`), `Kingmaker.View.MapObjects/{AbstractEntityPartComponent,
//! EntityPartComponent,ViewBasedPart,InteractionPart,InteractionLootPart,KingdomLootPart,
//! AreaTransitionPart,LocalMapMarkerPart,MapObjectFxPart,InteractionSkillCheckPart}.cs`).
//!
//! A scene state loads in this order: `PostLoad` (parts' `OnPostLoad`), then
//! `MatchStateWithScene` (each saved entity attaches to the scene view with its id:
//! `OnViewDidAttach`, which makes the view's part components `Ensure` and `SetSource` their
//! parts, drops view-based parts no component backs, and re-derives view-dependent state),
//! then `TurnOn`. The sim reads the views from the scene cache ([`crate::scene`]) and
//! **checks** that the attach step leaves the saved parts alone: a part set the view
//! disagrees with, a changed loot table, a part to create, a component without a port are
//! reported. State the attach re-derives from the view and the save (an interaction's
//! `Enabled`, the map marker's `Hidden`, the area transition's visibility) is computed and
//! applied.
//!
//! `InteractionSkillCheckPart.OnSettingsDidSet` recomputes the part's `DCOverride` (from the
//! id's string hash) and `SkillOverride` (an empty pool's `Unknown`; a pool's draw from the
//! global RNG is reported) at every load. Not ported, listed in `coverage.stubbed`:
//! presentation (map markers' UI list, FX).

use std::collections::HashMap;

use wr_content::Guid;

use crate::scene::{CompInfo, ViewInfo};
use crate::world::{EntityIdx, World};

/// `ItemsCollection` of an `InteractionLootPart`: the collection's identity and its items.
#[derive(Debug, Clone, Default)]
pub struct LootColl {
    pub id: usize,
    pub items: Vec<EntityIdx>,
}

/// `InteractionLootPart`'s saved state.
#[derive(Debug, Clone)]
pub struct LootPart {
    /// `InteractionPart.m_Enabled`.
    pub enabled: bool,
    /// `m_Loot`: the part's own collection; `None` when null (a kingdom stash's chest while
    /// off).
    pub loot: Option<LootColl>,
    /// `Loot` is the player's shared stash `n` (`KingdomLootPart.OnTurnOn`).
    pub stash: Option<usize>,
    /// `m_KnownItems`; `None` when null.
    pub known_items: Option<Vec<(Guid, i32)>>,
}

/// A `MapObjectEntityData` and what the sim keeps of its parts.
#[derive(Debug, Clone, Default)]
pub struct MapObj {
    pub loot: Option<LootPart>,
    /// `LocalMapMarkerPart.Hidden`, if the part exists.
    pub marker_hidden: Option<bool>,
    /// `MapObjectEntityData.WasHighlightedOnReveal`.
    pub was_highlighted_on_reveal: bool,
    /// `StaticEntityData.PerceptionCheckDC`.
    pub perception_check_dc: Option<i32>,
    /// `InteractionSkillCheckPart`'s members that `OnSettingsDidSet` recomputes at every
    /// load, if the part exists.
    pub skill_check: Option<SkillCheckPart>,
}

/// `InteractionSkillCheckPart.DCOverride` and `SkillOverride` (a `StatType`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SkillCheckPart {
    pub dc_override: i32,
    pub skill_override: i32,
}

/// Player-level shared stashes' loot (`Player.SharedStash`, `SecondSharedStash`,
/// `ThirdSharedStash`): whether each holds a lootable item (`ItemsCollection.HasLoot`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Stashes {
    pub has_loot: [bool; 3],
}

/// A null Unity object reference (`PPtr` with no file and no object).
fn is_null_ref(v: &serde_json::Value) -> bool {
    v.is_null() || (v["m_PathID"].as_i64() == Some(0) && v["m_FileID"].as_i64() == Some(0))
}

fn short(class: &str) -> &str {
    class.rsplit('.').next().unwrap_or(class)
}

fn i(v: &serde_json::Value) -> i64 {
    v.as_i64().unwrap_or(0)
}

fn guid_of(v: &serde_json::Value) -> Option<Guid> {
    v["guid"].as_str().filter(|s| !s.is_empty()).and_then(Guid::parse)
}

/// The part type a view component creates (`EntityPartComponent<TPart>`), if the sim has a
/// port; `Err` for view components that make parts it doesn't know.
fn part_of_component(class: &str) -> Result<Option<&'static str>, ()> {
    Ok(match short(class) {
        "InteractionLoot" => Some("InteractionLootPart"),
        "InteractionSkillCheck" => Some("InteractionSkillCheckPart"),
        "KingdomLootComponent" => Some("KingdomLootPart"),
        "AreaTransition" => Some("AreaTransitionPart"),
        "MapObjectFxComponent" => Some("MapObjectFxPart"),
        // `LocalMapMarker` makes a `LocalMapMarkerPart` (settings from the view).
        "LocalMapMarker" => Some("LocalMapMarkerPart"),
        // Part components of other kinds (`AbstractEntityPartComponent` subclasses).
        "DestructionComponent"
        | "InteractionRestriction"
        | "MusicPlayerComponent"
        | "InvisibleKittenComponent"
        | "ConditionalRestriction"
        | "DungeonOverrideTrapSpellRandomization" => return Err(()),
        _ => None,
    })
}

/// Whether a component class is an `IInteractionComponent` (`MapObjectView.InteractionComponent`).
fn is_interaction(class: &str) -> bool {
    matches!(short(class), "InteractionLoot" | "InteractionSkillCheck")
}

fn settings(c: &CompInfo) -> &serde_json::Value {
    &c.fields["Settings"]
}

/// `InteractionLootPart.DisableWhenEmpty`: `DefaultLoot`, `Environment` or `Unit`.
fn disable_when_empty(c: &CompInfo) -> bool {
    matches!(i(&settings(c)["LootContainerType"]), 0 | 1 | 3)
}

/// `MapObjectLoot`: the loot tables' entries, counted by item (`CountItems`).
fn count_items(w: &mut World, c: &CompInfo) -> Option<Vec<(Guid, i32)>> {
    let mut out: Vec<(Guid, i32)> = Vec::new();
    for t in settings(c)["m_LootTables"].as_array().into_iter().flatten() {
        let Some(g) = guid_of(t) else { continue };
        let Some(bp) = w.content.get(g) else {
            w.coverage.unsupported("InteractionLootPart: loot table blueprint missing");
            return None;
        };
        for e in bp.view().objs("Items") {
            let v = crate::content::View(e);
            let Some(item) = v.guid("m_Item") else { continue };
            let n = v.i32("Count");
            match out.iter_mut().find(|(g, _)| *g == item) {
                Some((_, c)) => *c = c.wrapping_add(n),
                None => out.push((item, n)),
            }
        }
    }
    Some(out)
}

/// `ItemsCollection.PostLoad()` over a loot collection: items post-load in order; an item
/// the collection would remove (`Count < 1`, another collection) is reported.
fn loot_post_load(w: &mut World, items: &[EntityIdx], id: usize) {
    for &it in items {
        let bad = w.item(it).is_none_or(|x| x.count < 1 || x.collection != Some(id));
        if bad {
            w.coverage.unsupported("ItemsCollection.PostLoad: invalid loot item removed");
        }
        if !w.entity(it).post_load_executed {
            crate::entity::post_load(w, it);
        }
    }
}

/// A part hook of a scene entity's part. Returns whether the sim handled it.
pub fn part_hook(w: &mut World, e: EntityIdx, clr: &str, hook: &str) -> bool {
    match (clr, hook) {
        // `InteractionLootPart.OnPostLoad` → `Loot?.PostLoad()`; `OnTurnOn` → `Loot?.TurnOn()`.
        ("InteractionLootPart", "OnPostLoad") => {
            let loot = w.scene.map_objects.get(&e).and_then(|m| m.loot.as_ref()?.loot.clone());
            if let Some(l) = loot {
                loot_post_load(w, &l.items, l.id);
            }
            true
        }
        ("InteractionLootPart", "OnTurnOn") => {
            let loot = w.scene.map_objects.get(&e).and_then(|m| m.loot.as_ref()?.loot.clone());
            if let Some(l) = loot {
                for it in l.items {
                    if !w.entity(it).is_turned_on {
                        crate::entity::turn_on(w, it);
                    }
                }
            }
            true
        }
        // `UnitPartFollowUnit.OnTurnOn`: unless initialized, the following starts from the saved
        // `m_LeaderRef` (nothing when the leader isn't loaded).
        ("UnitPartFollowUnit", "OnTurnOn") => {
            let leader =
                w.player.follow_leader_refs.get(&e).and_then(|id| w.by_id.get(id).copied());
            if let (false, Some(l)) = (w.player.follow_leaders.contains_key(&e), leader) {
                crate::components::etudes::follow_init(w, e, l);
            }
            true
        }
        // `ShiftersFuryPart.OnTurnOn`/`OnTurnOff`: `m_IsOn`, with the part's subscription.
        (crate::components::shifters_fury::PART, "OnTurnOn") => {
            crate::components::shifters_fury::part_turn_on(w, e);
            true
        }
        (crate::components::shifters_fury::PART, "OnTurnOff") => {
            crate::components::shifters_fury::part_turn_off(w, e);
            true
        }
        ("UnitPartFollowUnit", "OnTurnOff") => {
            crate::components::etudes::follow_release(w, e);
            true
        }
        // `LocalMapMarkerPart.OnTurnOn`: `LocalMapModel.Markers.Add(this)`, the UI's list.
        ("LocalMapMarkerPart", "OnTurnOn") => {
            w.coverage.stub("LocalMapMarkerPart.OnTurnOn (UI marker list)");
            true
        }
        // `MapObjectFxPart.OnPostLoad`: `m_IsLoaded = true` (read by `OnSettingsDidSet`, FX).
        ("MapObjectFxPart", "OnPostLoad") => {
            w.coverage.stub("MapObjectFxPart.OnPostLoad (FX state)");
            true
        }
        // `UnitPartVendor` with a shared table: `OnTurnOn`/`OnPostLoad`/`OnDispose` act on its
        // own inventory only, which it doesn't have.
        ("UnitPartVendor", "OnTurnOn" | "OnPostLoad") => w.scene.vendors.contains_key(&e),
        // `UnitPartCorruption.OnPostLoad` → `RefreshAppliedBuffs`: the unit's buffs that are
        // corruption levels' (`BlueprintRoot.Corruption.Progression`).
        ("UnitPartCorruption", "OnPostLoad") => {
            let levels: Vec<Guid> = w
                .content
                .get(crate::unit::BLUEPRINT_ROOT)
                .and_then(|r| r.view().guid("m_CorruptionRoot"))
                .and_then(|g| w.content.get(g))
                .and_then(|c| {
                    c.view().obj("m_Progression").map(|p| {
                        p.objs("m_Levels")
                            .into_iter()
                            .filter_map(|l| crate::content::View(l).guid("m_Buff"))
                            .collect()
                    })
                })
                .unwrap_or_default();
            let buffs: Vec<crate::world::FactIdx> = w
                .entity(e)
                .facts
                .list
                .iter()
                .copied()
                .filter(|f| {
                    w.fact(*f).kind == crate::world::FactKind::Buff
                        && w.fact(*f).blueprint.is_some_and(|b| levels.contains(&b))
                })
                .collect();
            w.scene.corruption.insert(e, buffs);
            true
        }
        // `KingdomLootPart.OnTurnOn`: the chest's `Loot` becomes the shared stash.
        ("KingdomLootPart", "OnTurnOn") => {
            kingdom_loot_turn_on(w, e);
            true
        }
        _ => false,
    }
}

/// `InteractionPart.Enabled = value` (the setter): `m_Enabled` takes the value and then the
/// getter's (`AlwaysDisabled`, `ForceDisable`), and a change runs `OnEnabledChanged`
/// (`InteractionLootPart.UpdateMarkerName`).
fn set_enabled(w: &mut World, e: EntityIdx, view: Option<&ViewInfo>, value: bool) {
    let always_disabled = view
        .and_then(|v| v.components.iter().find(|c| short(&c.class) == "InteractionLoot"))
        .is_some_and(|c| i(&settings(c)["AlwaysDisabled"]) != 0);
    let Some(lp) = w.scene.map_objects.get_mut(&e).and_then(|m| m.loot.as_mut()) else { return };
    if lp.enabled == value {
        return;
    }
    // `m_Enabled = Enabled`: a disabled-by-settings interaction stays off.
    lp.enabled = value && !always_disabled;
    update_marker_name(w, e, view);
}

/// `InteractionLootPart.UpdateMarkerName`: with `AddMapMarker`, the marker is hidden when the
/// interaction is off or (unless shown when empty) there is nothing lootable.
fn update_marker_name(w: &mut World, e: EntityIdx, view: Option<&ViewInfo>) {
    let Some(c) =
        view.and_then(|v| v.components.iter().find(|c| short(&c.class) == "InteractionLoot"))
    else {
        return;
    };
    if i(&settings(c)["AddMapMarker"]) == 0 {
        return;
    }
    let has_marker = w.scene.map_objects.get(&e).is_some_and(|m| m.marker_hidden.is_some());
    if !has_marker {
        w.coverage.unsupported("InteractionLootPart.UpdateMarkerName: LocalMapMarkerPart created");
        return;
    }
    let (enabled, lootable) = {
        let m = &w.scene.map_objects[&e];
        let lp = m.loot.as_ref();
        let enabled = lp.is_some_and(|l| l.enabled);
        let lootable = match lp {
            Some(l) if l.stash.is_some() => {
                l.stash.is_some_and(|s| w.scene.stashes.has_loot[s.min(2)])
            }
            Some(l) => {
                l.loot.as_ref().is_some_and(|c| c.items.iter().any(|it| w.item(*it).is_some()))
            }
            None => false,
        };
        (enabled, lootable)
    };
    let show_when_empty = i(&settings(c)["ShowOnMapWhenEmpty"]) != 0;
    let hidden = !enabled || (!show_when_empty && !lootable);
    if let Some(m) = w.scene.map_objects.get_mut(&e) {
        m.marker_hidden = Some(hidden);
    }
}

fn kingdom_loot_turn_on(w: &mut World, e: EntityIdx) {
    let view = view_of(w, e);
    // `Source` is the view's `KingdomLootComponent`: `type` picks the stash.
    let stash = view
        .as_ref()
        .and_then(|v| v.components.iter().find(|c| short(&c.class) == "KingdomLootComponent"))
        .map(|c| usize::try_from(i(&c.fields["type"])).unwrap_or(0));
    let Some(stash) = stash else { return };
    let dwe = view
        .as_ref()
        .and_then(|v| v.components.iter().find(|c| short(&c.class) == "InteractionLoot"))
        .is_some_and(disable_when_empty);
    let Some(lp) = w.scene.map_objects.get_mut(&e).and_then(|m| m.loot.as_mut()) else { return };
    // `InteractionLootPart.Loot = stash`: `Enabled = Loot != null && (HasLoot || !DisableWhenEmpty)`.
    lp.loot = None;
    lp.stash = Some(stash);
    let enabled = w.scene.stashes.has_loot[stash.min(2)] || !dwe;
    set_enabled(w, e, view.as_ref(), enabled);
    // The setter ran `OnEnabledChanged` only on a change; a changed `Loot` with the same
    // `Enabled` leaves the marker as it was.
}

fn view_of(w: &World, e: EntityIdx) -> Option<ViewInfo> {
    let id = &w.entity(e).unique_id;
    w.scene.cache.as_ref()?.view(id).cloned()
}

/// `SceneLoader.MatchStateWithScene` → `LoadSceneEntities` → `AttachToViewOnLoad(view)` for a
/// `MapObjectEntityData` (script zones are one): `OnViewDidAttach` and the parts'
/// `ViewDidAttach`.
pub fn view_attach(w: &mut World, e: EntityIdx) {
    let Some(view) = view_of(w, e) else { return };
    let clr = w.entity(e).clr;
    if clr != "MapObjectEntityData" && clr != "ScriptZoneData" {
        return;
    }
    // `EntityDataBase.OnViewDidAttach`: each part component of the view `Ensure`s its part and
    // sets its source.
    let mut expected: Vec<&'static str> = Vec::new();
    for c in &view.components {
        match part_of_component(&c.class) {
            Err(()) => {
                w.coverage.unsupported(format!(
                    "EntityPartComponent {}: part not ported",
                    short(&c.class)
                ));
            }
            Ok(None) => {}
            Ok(Some(part)) => {
                expected.push(part);
                if !w.entity(e).parts.iter().any(|p| p.clr == part) {
                    w.coverage.unsupported(format!("EntityPartsManager.Ensure: {part} created"));
                    continue;
                }
                set_source(w, e, &view, c, part);
            }
        }
    }
    // `LocalMapMarkerPart` is created by `UpdateMarkerName` / the transition's settings (it is
    // `IsRuntimeCreated`, so the removal below spares it); every other part must be backed.
    let marker_made = view.components.iter().any(|c| {
        matches!(short(&c.class), "InteractionLoot" | "AreaTransition")
            && i(&settings(c)["AddMapMarker"]) != 0
    });
    for p in w.entity(e).parts.clone() {
        let backed = expected.contains(&p.clr) || (p.clr == "LocalMapMarkerPart" && marker_made);
        if !backed {
            w.coverage.unsupported(format!("EntityDataBase.OnViewDidAttach: {} removed", p.clr));
        }
    }
    // `StaticEntityData.OnViewDidAttach`: the perception check.
    let perception = view
        .components
        .iter()
        .find(|c| short(&c.class) == "PerceptionCheckComponent")
        .map(|c| i(&c.fields["DC"]) as i32);
    if let Some(m) = w.scene.map_objects.get_mut(&e) {
        if m.perception_check_dc.is_none() {
            m.perception_check_dc = perception;
        }
        // `MapObjectEntityData.OnViewDidAttach`: `WasHighlightedOnReveal = View.InteractionComponent == null`.
        m.was_highlighted_on_reveal = !view.components.iter().any(|c| is_interaction(&c.class));
    }
    // ... then the view's `FactHolder` against the saved `FactHolder.Fact`.
    let holder = view
        .components
        .iter()
        .find(|c| short(&c.class) == "FactHolder")
        .and_then(|c| guid_of(&c.fields["m_Blueprint"]));
    let fact = w.entity(e).facts.list.first().and_then(|f| w.fact(*f).blueprint);
    if w.entity(e).facts.list.len() > 1 || fact != holder {
        w.coverage.unsupported("MapObjectEntityData.OnViewDidAttach: FactHolder fact replaced");
    }
}

/// `ViewBasedPart.SetSource` for a part and its `OnSettingsDidSet`.
fn set_source(w: &mut World, e: EntityIdx, view: &ViewInfo, c: &CompInfo, part: &str) {
    match part {
        "InteractionLootPart" => {
            let Some(lp) = w.scene.map_objects.get(&e).and_then(|m| m.loot.clone()) else {
                return;
            };
            // `m_KnownItems == null`: the tables' items are added to the loot; otherwise
            // `UpdateLoot` brings the loot to the tables' counts.
            let tables = count_items(w, c);
            match (&lp.known_items, tables) {
                (None, Some(t)) if t.is_empty() => {
                    if let Some(m) = w.scene.map_objects.get_mut(&e)
                        && let Some(l) = m.loot.as_mut()
                    {
                        l.known_items = Some(Vec::new());
                    }
                }
                (None, _) => {
                    w.coverage.unsupported("InteractionLootPart.SetSource: loot added from tables");
                }
                (Some(known), Some(t)) => {
                    let same = t.len() == known.len()
                        && t.iter().all(|(g, n)| known.iter().any(|(kg, kn)| kg == g && kn == n));
                    if !same {
                        w.coverage
                            .unsupported("InteractionLootPart.UpdateLoot: loot tables changed");
                    }
                }
                (Some(_), None) => {}
            }
            // `if (DisableWhenEmpty && Loot.Empty()) Enabled = false; else UpdateMarkerName();`
            let empty = lp.loot.as_ref().is_none_or(|l| l.items.is_empty());
            if disable_when_empty(c) && empty {
                set_enabled(w, e, Some(view), false);
            } else {
                update_marker_name(w, e, Some(view));
            }
        }
        "InteractionSkillCheckPart" => skill_check_settings_did_set(w, e, c),
        "AreaTransitionPart" => {
            // `OnSettingsDidSet`: the exit marker (`AddMapMarker`), then `UpdateVisibility`.
            if i(&settings(c)["AddMapMarker"]) != 0
                && !w.scene.map_objects.get(&e).is_some_and(|m| m.marker_hidden.is_some())
            {
                w.coverage
                    .unsupported("AreaTransitionPart.OnSettingsDidSet: LocalMapMarkerPart created");
            }
            update_visibility(w, e, view);
        }
        // `KingdomLootPart` and `MapObjectFxPart` only record their source here (`FxActive`
        // is kept: `m_IsLoaded` was set by the part's `OnPostLoad`).
        _ => {}
    }
    // `InteractionPart.OnViewDidAttach`: a trap's back-reference.
    if matches!(part, "InteractionLootPart" | "InteractionSkillCheckPart")
        && !is_null_ref(&settings(c)["Trap"])
    {
        w.coverage.unsupported("InteractionPart.OnViewDidAttach: trap");
    }
}

/// `BlueprintRoot.Instance.Interaction.InteractionDCVariation`.
fn interaction_dc_variation(w: &World) -> Option<i32> {
    let root = w.content.get(crate::unit::BLUEPRINT_ROOT)?;
    let ir = w.content.get(root.view().guid("m_InteractionRoot")?)?;
    let v = ir.view();
    v.has("m_InteractionDCVariation").then(|| v.i32("m_InteractionDCVariation"))
}

/// `InteractionSkillCheckPart.OnSettingsDidSet(isNewSettings: true)`: after a load `Source`
/// is null, so `SetSource` always passes `true`, and the part's saved `DCOverride` and
/// `SkillOverride` are recomputed from the settings:
/// - `DCOverride = (dc == 0 || Exact) ? dc : dc + RandomDCMod(Owner.UniqueId)`, `dc` being
///   `Settings.GetDC()` (`DC`, unless `HasCalculatedDC`, plus each `DCModifiers` entry whose
///   conditions hold) and `RandomDCMod` `Math.Abs(id.GetHashCode()) % (2v + 1) - v` with
///   `v = InteractionDCVariation` (`string.GetHashCode` of the runtime,
///   [`crate::csharp::string_hash`]);
/// - `SkillOverride = Settings.SkillPool.Random()` (`LinqExtensions.Random(IList)`: `default`
///   (`Unknown`) for an empty pool, else a draw from the global RNG).
///
/// In a dungeon campaign both come from a `PersistentRandom` generator instead. The calculated
/// DC, the DC modifiers' conditions, the pool's draw and the dungeon generator aren't ported:
/// reported.
fn skill_check_settings_did_set(w: &mut World, e: EntityIdx, c: &CompInfo) {
    let s = settings(c);
    if crate::xp::is_dungeon_campaign(w) {
        w.coverage
            .unsupported("InteractionSkillCheckPart.OnSettingsDidSet: dungeon campaign generator");
        return;
    }
    if i(&s["HasCalculatedDC"]) != 0 {
        w.coverage.unsupported("InteractionSkillCheckPart.OnSettingsDidSet: CalculatedDC");
        return;
    }
    if s["DCModifiers"].as_array().is_some_and(|m| m.iter().any(|x| !x.is_null())) {
        w.coverage.unsupported("InteractionSkillCheckPart.OnSettingsDidSet: DCModifiers");
        return;
    }
    let Ok(dc) = i32::try_from(i(&s["DC"])) else {
        w.coverage.unsupported("InteractionSkillCheckPart: DC out of range");
        return;
    };
    let dc_override = if dc == 0 || i(&s["Exact"]) != 0 {
        dc
    } else {
        let Some(v) = interaction_dc_variation(w) else {
            w.coverage
                .unsupported("InteractionSkillCheckPart.RandomDCMod: interaction root missing");
            return;
        };
        let h = crate::csharp::string_hash::get_hash_code(&w.entity(e).unique_id);
        // `Math.Abs(int.MinValue)` throws `OverflowException`.
        let Some(abs) = h.checked_abs() else {
            w.coverage.unsupported("InteractionSkillCheckPart.RandomDCMod: Math.Abs overflow");
            return;
        };
        let n = v.wrapping_mul(2).wrapping_add(1);
        if n == 0 {
            w.coverage.unsupported("InteractionSkillCheckPart.RandomDCMod: division by zero");
            return;
        }
        dc.wrapping_add((abs % n).wrapping_sub(v))
    };
    let pool = s["SkillPool"].as_array().map_or(0, Vec::len);
    if pool > 0 {
        w.coverage.unsupported(
            "InteractionSkillCheckPart.OnSettingsDidSet: SkillPool.Random() (global RNG at load)",
        );
        return;
    }
    let Some(part) = w.scene.map_objects.get_mut(&e).and_then(|m| m.skill_check.as_mut()) else {
        w.coverage.unsupported("InteractionSkillCheckPart: saved part not read");
        return;
    };
    *part = SkillCheckPart { dc_override, skill_override: 0 };
}

/// `AreaTransitionPart.UpdateVisibility`: the object is in game while the visibility flag is
/// unlocked, or the visibility etude is playing.
fn update_visibility(w: &mut World, e: EntityIdx, view: &ViewInfo) {
    let Some(c) = view.components.iter().find(|c| short(&c.class) == "AreaTransition") else {
        return;
    };
    let s = settings(c);
    let value = if let Some(f) = guid_of(&s["m_VisibilityFlag"]) {
        crate::narrative::flags::is_unlocked(w, f)
    } else if let Some(et) = guid_of(&s["m_VisibilityEtude"]) {
        crate::narrative::etudes::status(w, et) == crate::narrative::etudes::EtudeStatus::Playing
    } else {
        return;
    };
    // `ChangeVisibility`: `Owner.IsInGame = value` (the setter).
    crate::entity::set_is_in_game(w, e, value);
}

/// `MapObjectEntityData.OnIsInGameChanged`: `Get<AreaTransitionPart>()?.UpdateVisibility()`.
pub fn map_object_in_game_changed(w: &mut World, e: EntityIdx) {
    if !w.entity(e).parts.iter().any(|p| p.clr == "AreaTransitionPart") {
        return;
    }
    if let Some(v) = view_of(w, e) {
        update_visibility(w, e, &v);
    }
}

/// `AreaTransitionPart`'s `OnAreaDidLoad`, `OnEtudesUpdate`, `HandleUnlock` and `HandleLock`.
pub fn update_all_visibilities(w: &mut World) {
    let objs: Vec<EntityIdx> = w
        .scene
        .map_objects
        .keys()
        .copied()
        .filter(|e| {
            w.entity(*e).parts.iter().any(|p| p.clr == "AreaTransitionPart")
                && w.entity(*e).is_turned_on
        })
        .collect();
    for e in objs {
        if let Some(v) = view_of(w, e) {
            update_visibility(w, e, &v);
        }
    }
}

/// For the tests: a map of entity ids to objects.
#[must_use]
pub fn ids(w: &World) -> HashMap<String, EntityIdx> {
    w.scene.map_objects.keys().map(|e| (w.entity(*e).unique_id.clone(), *e)).collect()
}
