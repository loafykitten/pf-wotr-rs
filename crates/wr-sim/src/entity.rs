//! `EntityDataBase`'s lifecycle (`EntityDataBase.cs:319–430`) with the unit and item
//! overrides on the load path.
//!
//! `PostLoad`: `PrePostLoad` (register, restore owner links), `OnPostLoad`,
//! `Facts.PostLoad`, `Parts.PostLoad`, `OnComponentsDidPostLoad`. `ApplyPostLoadFixes`:
//! `Facts`, `Parts`, then `OnApplyPostLoadFixes`. `TurnOn`: flag, subscribe,
//! `Facts.TurnOn` (sorted by priority), `Parts.TurnOn`, `OnTurnOn`.

use std::rc::Rc;

use crate::content::Blueprint;
use crate::facts::lifecycle as fl;
use crate::world::{EntityIdx, EntityKind, World};

/// `PrePostLoad()`.
pub fn pre_post_load(w: &mut World, e: EntityIdx) {
    let id = w.entity(e).unique_id.clone();
    w.by_id.insert(id, e);
    w.unregistered_items.remove(&e);
    fl::manager_restore_owner_link(w, e);
}

/// `PostLoad()`.
pub fn post_load(w: &mut World, e: EntityIdx) {
    if w.entity(e).post_load_executed {
        w.log("error", "EntityDataBase.PostLoad: already executed");
        return;
    }
    w.entity_mut(e).post_load_executed = true;
    pre_post_load(w, e);
    w.trace("Entity.PostLoad", |w| w.entity(e).unique_id.clone());
    on_post_load(w, e);
    fl::manager_post_load(w, e);
    parts_post_load(w, e);
    // `OnComponentsDidPostLoad`: units → `Descriptor.OnComponentsDidPostLoad` →
    // `UISettings.PostLoad` (portrait and UI state only).
}

fn on_post_load(w: &mut World, e: EntityIdx) {
    match &w.entity(e).kind {
        EntityKind::Unit(_) => unit_on_post_load(w, e),
        // `ItemEntity.OnPostLoad`: `m_Enchantments = EnsureFactProcessor<…>()` (the
        // enchantment fact processor; its attach/detach logic isn't on the load path).
        // `ItemEntityShield.OnPostLoad` then post-loads its armor and weapon components
        // (`ArmorComponent.PostLoad(this)`) and replaces any whose blueprint changed.
        EntityKind::Item(_) => shield_on_post_load(w, e),
        // `CutscenePlayerData.OnPostLoad`.
        EntityKind::Other if w.entity(e).clr == "CutscenePlayerData" => {
            crate::scene::cutscene_post_load(w, e);
        }
        EntityKind::Other | EntityKind::DroppedLoot(_) => {}
    }
}

/// `UnitEntityData.OnPostLoad`: fact processors, the buff next-event cache, then
/// `Descriptor.PostLoad` (stats, alignment, resources, body, progression, spellbooks,
/// state, attack factions, brain), then combat state and commands. All before the unit's
/// facts are post-loaded.
fn unit_on_post_load(w: &mut World, e: EntityIdx) {
    w.trace("UnitEntityData.OnPostLoad", |w| w.entity(e).unique_id.clone());
    // `UnitDescriptor.PostLoad`: `OriginalBlueprint ??= Blueprint`.
    if let Some(u) = w.unit_mut(e)
        && u.original_blueprint.is_none()
    {
        u.original_blueprint = u.blueprint;
    }
    crate::stats::ops::character_stats_post_load(w, e);
    // `Alignment.PostLoad`, `Resources.PostLoad`: owner links. `Body.PostLoad`: each slot
    // in `AllSlots` order post-loads its item if not done yet (before the unit's facts).
    // `AllSlots` is the equipment slots then the additional limbs; the empty-hand
    // weapons follow (`UnitBody.PostLoad`).
    for it in crate::unit::body_items(w, e) {
        if !w.entity(it).post_load_executed {
            post_load(w, it);
        }
    }
    // Alignment, resources, body, progression, state, attack factions and brain `PostLoad`s
    // restore owner links and caches; the sheet's values don't depend on them beyond what
    // hydration copied in. The spellbooks' do (`Spellbook.PostLoad`).
    crate::spellbook::post_load_unit(w, e);
    // `State.PostLoad` (after spellbooks).
    crate::unit_state::post_load(w, e);
}

/// `EntityPartsManager.PostLoad()`: each part's `PostLoad`, then the owner's callbacks.
/// Part types whose `OnPostLoad` isn't ported are reported unless known to be inert for
/// the sheet.
fn parts_post_load(w: &mut World, e: EntityIdx) {
    for p in w.entity(e).parts.clone() {
        if part_hook_overridden(p.clr, "OnPostLoad")
            && !INERT_PARTS.contains(&p.clr)
            && !crate::scene_parts::part_hook(w, e, p.clr, "OnPostLoad")
        {
            w.coverage.unsupported(format!("{}.OnPostLoad", p.clr));
        }
    }
}

/// Parts whose load-time hooks restore links or UI state only (read in the C#).
const INERT_PARTS: &[&str] = &[
    "UnitPartDollData",
    "UnitPartCompanion",
    "UnitPartNonStackBonuses",
    "UnitPartWeariness",
    "UnitPartPartyWeatherBuff",
    // `UnitPartActivatableAbility`, `UnitPartSpecialMetamagic` and `UnitPartExtraSpellsPerDay`
    // override no part hook (`OldStyleUnitPart` data only).
    "UnitPartActivatableAbility",
    "UnitPartSpecialMetamagic",
    "UnitPartExtraSpellsPerDay",
    // `UnitPartDamageReduction` overrides only `OnPreSave` (it drops the chunks of inactive
    // facts); it subscribes `IUnitApplyDamageReduction`, which combat reports for a target
    // that has the part.
    "UnitPartDamageReduction",
    // `UnitPartRoaming.OnPostLoad` resolves the next waypoint (a runtime link for the
    // roaming controller: movement is T1).
    "UnitPartRoaming",
    // `UnitPartWeaponTraining` is an `OldStyleUnitPart` list of facts (no hooks).
    "UnitPartWeaponTraining",
];

/// Whether a part's save-side type (or a base) has a body for `hook`, from the save
/// schema's hook bits.
fn part_hook_overridden(clr: &str, hook: &str) -> bool {
    let types = wr_save::schema::types();
    types.iter().find(|t| t.short() == clr).is_some_and(|t| t.hook_names().contains(&hook))
}

/// `ApplyPostLoadFixes()`.
pub fn apply_post_load_fixes(w: &mut World, e: EntityIdx) {
    w.trace("Entity.ApplyPostLoadFixes", |w| w.entity(e).unique_id.clone());
    fl::manager_apply_post_load_fixes(w, e);
    for p in w.entity(e).parts.clone() {
        if part_hook_overridden(p.clr, "OnApplyPostLoadFixes") && !INERT_PARTS.contains(&p.clr) {
            w.coverage.unsupported(format!("{}.OnApplyPostLoadFixes", p.clr));
        }
    }
    match w.entity(e).kind {
        EntityKind::Unit(_) => unit_on_apply_post_load_fixes(w, e),
        EntityKind::Item(_) => item_on_apply_post_load_fixes(w, e),
        EntityKind::Other | EntityKind::DroppedLoot(_) => {}
    }
}

/// `ItemEntity.OnApplyPostLoadFixes` (`Kingmaker.Items/ItemEntity.cs:909`): a usable item
/// that spends charges and has none is given one; an `Ability` the save held on the item
/// (a fact of no manager, so never post-loaded) is replaced by unequipping and equipping the
/// item again, which gives its wielder the blueprint's ability afresh (new unique ids).
fn item_on_apply_post_load_fixes(w: &mut World, e: EntityIdx) {
    let Some(item) = w.item(e).cloned() else { return };
    if let Some(bp) = item.blueprint.and_then(|g| w.content.get(g))
        && crate::content::is_a(bp.view().ty(), "BlueprintItemEquipmentUsable")
        && bp.view().bool("SpendCharges")
        && bp.view().int("Charges") > 0
        && !bp.view().bool("RestoreChargesOnRest")
        && item.charges < 1
    {
        w.log("error", "ItemEntity has 0 charges, fixed");
        if let Some(it) = w.item_mut(e) {
            it.charges = 1;
        }
    }
    if let Some(a) = item.ability
        && !w.fact(a).post_load_executed
    {
        match item.wielder {
            Some(wielder) => {
                w.log("error", "ItemEntity has Ability which isn't PosLoaded, fixed");
                crate::items::on_will_unequip(w, e);
                crate::items::on_did_equipped(w, e, wielder);
            }
            None => {
                w.log("error", "ItemEntity isn't active but has Ability, fixed");
                if let Some(it) = w.item_mut(e) {
                    it.ability = None;
                }
            }
        }
    }
    // `HoldingSlot != null && HoldingSlot.MaybeItem != this && Owner.IsPlayerFaction`: the
    // hydrated slot always holds its item.
}

/// `UnitEntityData.OnApplyPostLoadFixes` → `Descriptor.ApplyPostLoadFixes` first.
fn unit_on_apply_post_load_fixes(w: &mut World, e: EntityIdx) {
    // `OnDifficultyChanged`: player-faction units return at once; others recalculate
    // ability params of facts whose context blueprint has a params calculator.
    if !w.unit(e).is_some_and(|u| u.is_player_faction) {
        // `Progression.Features`, then `Buffs`, each in the unit's fact order.
        for kind in [crate::world::FactKind::Feature, crate::world::FactKind::Buff] {
            for f in w.entity(e).facts.list.clone() {
                if w.fact(f).kind != kind {
                    continue;
                }
                let has_calc = w
                    .fact(f)
                    .context
                    .as_ref()
                    .and_then(|c| c.associated_blueprint)
                    .and_then(|g| w.content.get(g))
                    .is_some_and(|b| {
                        b.components_array()
                            .iter()
                            .any(|o| crate::content::is_a(o.ty, "ContextAbilityParamsCalculator"))
                    });
                if has_calc {
                    crate::context::recalculate_ability_params(w, f);
                }
            }
        }
    }
    // `EnsureNaturalProficiencies`: proficiency list only.
    crate::stats::ops::character_stats_apply_post_load_fixes(w, e);
    // `State.ApplyPostLoadFixes`: `SpellcastingForbidden` (51) once per
    // `Features.SpellCastingForbidden` (`MechanicsFeatureType` 49) count.
    let n = w.unit(e).and_then(|u| u.mechanics_features.get(&49).copied()).unwrap_or(0);
    for _ in 0..n {
        crate::unit_state::add_condition(
            w,
            e,
            crate::unit_state::cond::SPELLCASTING_FORBIDDEN,
            None,
        );
    }
    // The spellbooks' fixes (`Spellbook.ApplyPostLoadFixes`, dropping an empty one).
    crate::spellbook::apply_post_load_fixes_unit(w, e);
    // Not ported, and not checked: `Body.ApplyPostLoadFixes`, resurrection-incompatible
    // buffs, polymorph cancel, the class-feature restores of `Progression.ApplyPostLoadFixes`.
    // Checked below:
    // `AddPostLoadActions` and `FixUnitOnPostLoad` on the unit blueprint, unit upgraders,
    // and the replace-stacking buff de-duplication.
    if let Some(bp) = w.unit(e).and_then(|u| u.blueprint).and_then(|g| w.content.get(g)) {
        for o in bp.components_array() {
            for n in ["AddPostLoadActions", "FixUnitOnPostLoad"] {
                if crate::content::is_a(o.ty, n) {
                    w.coverage.unsupported(format!("UnitDescriptor.ApplyPostLoadFixes: {n}"));
                }
            }
        }
    }
    // `Progression.ApplyPostLoadFixes()`: the spellbooks of classes with a negative caster
    // level modifier catch up with the class level (its class-feature restores aren't ported).
    crate::spellbook::restore_class_spellbooks(w, e);
    // `UnitUpgraderHelper.ApplyUpgraders(OriginalBlueprint, fromPlaceholder: false)`: any
    // listed upgrader not yet applied would run.
    let applied = w.unit(e).map(|u| u.applied_upgraders.clone()).unwrap_or_default();
    if let Some(bp) = w.unit(e).and_then(|u| u.original_blueprint).and_then(|g| w.content.get(g)) {
        for o in bp.components_array() {
            if crate::content::is_a(o.ty, "UnitUpgraderComponent") {
                for g in crate::content::View(o).guids("m_Upgraders") {
                    let from_placeholder =
                        w.content.get(g).is_some_and(|u| u.view().bool("ApplyFromPlaceholder"));
                    if !from_placeholder && !applied.contains(&g) {
                        w.coverage.unsupported("UnitUpgraderHelper.ApplyUpgraders");
                    }
                }
            }
        }
    }
    // Replace-stacking buffs: the last of each blueprint stays, earlier duplicates go.
    let mut seen = std::collections::HashSet::new();
    for f in w.entity(e).facts.list.clone() {
        let fact = w.fact(f);
        if fact.kind == crate::world::FactKind::Buff
            && fact.bp.as_ref().is_some_and(|b| b.view().int("Stacking") == 0)
            && !seen.insert(fact.blueprint)
        {
            w.coverage.unsupported(
                "UnitEntityData.OnApplyPostLoadFixes: duplicate replace-stacking buff",
            );
        }
    }
}

/// `TurnOn()`.
pub fn turn_on(w: &mut World, e: EntityIdx) {
    if w.entity(e).is_turned_on {
        w.log("error", "EntityDataBase.TurnOn: is already turned on");
        return;
    }
    w.entity_mut(e).is_turned_on = true;
    w.trace("Entity.TurnOn", |w| w.entity(e).unique_id.clone());
    if !w.entity(e).subscribed {
        w.entity_mut(e).subscribed = true;
        crate::rulebook::bus::subscribe(w, crate::rulebook::bus::Sub::Entity(e));
    }
    fl::manager_turn_on(w, e);
    for p in w.entity(e).parts.clone() {
        if part_hook_overridden(p.clr, "OnTurnOn")
            && !INERT_PARTS.contains(&p.clr)
            && !crate::scene_parts::part_hook(w, e, p.clr, "OnTurnOn")
        {
            w.coverage.unsupported(format!("{}.OnTurnOn", p.clr));
        }
    }
    match &w.entity(e).kind {
        EntityKind::Unit(_) => unit_on_turn_on(w, e),
        EntityKind::Item(_) => item_on_turn_on(w, e),
        EntityKind::Other | EntityKind::DroppedLoot(_) => {}
    }
}

/// `TurnOff()` (`EntityDataBase.cs:428`): facts, parts, `OnTurnOff`, unsubscribe.
pub fn turn_off(w: &mut World, e: EntityIdx) {
    if !w.entity(e).is_turned_on {
        w.log("error", "EntityDataBase.TurnOff: isn't turned on");
        return;
    }
    w.entity_mut(e).is_turned_on = false;
    w.trace("Entity.TurnOff", |w| w.entity(e).unique_id.clone());
    fl::manager_turn_off(w, e);
    for p in w.entity(e).parts.clone() {
        if part_hook_overridden(p.clr, "OnTurnOff")
            && !INERT_PARTS.contains(&p.clr)
            && !crate::scene_parts::part_hook(w, e, p.clr, "OnTurnOff")
        {
            w.coverage.unsupported(format!("{}.OnTurnOff", p.clr));
        }
    }
    // `UnitEntityData.OnTurnOff` → `Descriptor.TurnOff()`: the unit's own inventory, the
    // body's items, the brain.
    if w.unit(e).is_some() {
        w.unit_mut(e).expect("unit").descriptor_turned_on = false;
        let (own, inv) = {
            let unit = w.unit(e).expect("unit");
            (unit.has_own_inventory, unit.inventory.clone())
        };
        let body_items = crate::unit::body_items(w, e);
        if own {
            for it in inv {
                if w.entity(it).is_turned_on {
                    turn_off(w, it);
                }
            }
        }
        for it in body_items {
            if w.entity(it).is_turned_on {
                turn_off(w, it);
            }
        }
    }
    if w.entity(e).subscribed {
        w.entity_mut(e).subscribed = false;
        crate::rulebook::bus::unsubscribe(w, crate::rulebook::bus::Sub::Entity(e));
    }
}

/// `EntityDataBase.IsInGame` setter (`EntityDataBase.cs:147`): on a change, `OnIsInGameChanged`.
/// For a unit (`UnitEntityData.OnIsInGameChanged`, `UnitEntityData.cs:1123`) that is
/// `Descriptor.Buffs`, `ActivatableAbilities` and `Abilities` `.SetActive(IsInGame)`
/// (`EntityFactsProcessor.SetActive`: each fact of the kind, in `Facts.List` order, is
/// activated when it isn't active, or deactivated when it is), then the group is left. The
/// setter raises `IInGameHandler.HandleObjectInGameChanged` first, for any entity. The pets'
/// own `IsInGame`, the companion character lists, the dummy view and the group's
/// `Remove` (the cached `m_Group`, which the save doesn't say) aren't modelled: reported.
pub fn set_is_in_game(w: &mut World, e: EntityIdx, value: bool) {
    if w.entity(e).is_in_game == value {
        return;
    }
    w.entity_mut(e).is_in_game = value;
    // `IInGameHandler.HandleObjectInGameChanged(entity)` to the global subscribers, for every
    // entity; then `OnIsInGameChanged`.
    let unit = w.unit(e).is_some();
    if unit {
        // (`EntityBoundsController` among them: back in the game, on its update list.)
        *w.combat.vision.in_game_gen.entry(e).or_default() += 1;
        // `UnitEntityData.OnIsInGameChanged` → `UpdateHasDummyView`.
        let sleeping = if w.combat.awake.contains(&e) {
            Some(false)
        } else if w.combat.awake_unknown.contains(&e) {
            None
        } else {
            Some(true)
        };
        crate::combat::vision::update_has_dummy_view(w, e, sleeping, true);
    }
    crate::rulebook::bus::raise_global_call(
        w,
        "IInGameHandler",
        "HandleObjectInGameChanged",
        unit.then_some(e),
    );
    // `UnitPartCompanion.HandleObjectInGameChanged` (the part is a global `IInGameHandler`,
    // subscribed while its owner is turned on; only its owner's change matters): back in the
    // game, `IsIgnoreFixInGame` clears.
    if value
        && w.entity(e).is_turned_on
        && w.entity(e).parts.iter().any(|p| p.clr == "UnitPartCompanion")
        && let Some(x) = w.unit_mut(e)
    {
        x.ignore_fix_in_game = false;
    }
    if !unit {
        match w.entity(e).clr {
            "MapObjectEntityData" | "ScriptZoneData" => {
                crate::scene_parts::map_object_in_game_changed(w, e);
            }
            // `UnitSpawnerBase.MyData`, `TrapObjectData` and `AreaEffectEntityData` override
            // it; the other entities don't.
            "MyData" | "TrapObjectData" | "AreaEffectEntityData" => {
                w.coverage.unsupported("EntityDataBase.OnIsInGameChanged: entity kind not ported");
            }
            _ => {}
        }
        return;
    }
    let has = |w: &World, clr: &str| w.entity(e).parts.iter().any(|p| p.clr == clr);
    if has(w, "UnitPartCompanion") {
        // `Player.InvalidateCharacterLists()`: cached lists (UI and party queries).
        w.coverage.stub("UnitEntityData.OnIsInGameChanged: companion list cache");
    }
    if has(w, "UnitPartPetMaster") || has(w, "UnitPartPet") {
        w.coverage.unsupported("UnitEntityData.OnIsInGameChanged: pets");
    }
    use crate::world::FactKind;
    for kind in [FactKind::Buff, FactKind::ActivatableAbility, FactKind::Ability] {
        for f in w.entity(e).facts.list.clone() {
            if w.fact(f).kind != kind {
                continue;
            }
            let active = w.fact(f).is_active;
            if value && !active {
                fl::activate(w, f);
            } else if !value && active {
                fl::deactivate(w, f);
            }
        }
    }
    w.events.push(
        "t1",
        serde_json::json!({ "what": "unit_in_game", "unit": crate::events::unit_ref(&w.entity(e).unique_id), "value": value }),
    );
    // `m_Group?.Remove(this); m_Group = null`: modelled while groups are (a driven replay with
    // the oracle's group records), else reported.
    if w.combat.groups.on {
        crate::combat::groups::drop_cached(w, e);
    } else {
        w.coverage.unsupported("UnitEntityData.OnIsInGameChanged: group membership");
    }
}

/// `UnitEntityData.OnTurnOn`: `Descriptor.TurnOn`, then for in-game units every fact's
/// `UpdateIsActive` (correcting active/enabled mismatches).
fn unit_on_turn_on(w: &mut World, e: EntityIdx) {
    crate::unit::descriptor_turn_on(w, e);
    // `IsInGame && PreviewOf == null` (`UnitEntityData.cs:1689`): a preview's facts aren't
    // run through `UpdateIsActive` (`SetupPreview` deactivated its buffs).
    if w.entity(e).is_in_game && w.unit(e).is_some_and(|u| u.preview_of.is_none()) {
        for f in w.entity(e).facts.list.clone() {
            if w.fact(f).is_active != crate::facts::is_enabled(w, f) {
                w.log("error", format!("{} enabled but not active, fixed", w.fact_label(f)));
            }
            fl::update_is_active(w, f);
        }
    }
}

fn shield_on_post_load(w: &mut World, e: EntityIdx) {
    let Some(it) = w.item(e).cloned() else { return };
    let bp = it.blueprint.and_then(|g| w.content.get(g));
    for (c, field) in
        [(it.armor_component, "m_ArmorComponent"), (it.weapon_component, "m_WeaponComponent")]
    {
        let want = bp.as_ref().and_then(|b| b.view().guid(field));
        match c {
            Some(c) => {
                if !w.entity(c).post_load_executed {
                    post_load(w, c);
                }
                if w.item(c).and_then(|x| x.blueprint) != want {
                    w.coverage.unsupported("ItemEntityShield.OnPostLoad: replace component");
                }
            }
            None if want.is_some() && w.entity(e).clr == "ItemEntityShield" => {
                w.coverage.unsupported("ItemEntityShield.OnPostLoad: create component");
            }
            None => {}
        }
    }
}

/// `ItemEntity.OnTurnOn`: `ApplyEnchantments(false)` (drop enchantments the blueprint no
/// longer lists, restore missing ones), `ReapplyAbilities`, `ReapplyFactsForWielder`; then
/// the subclass's: a shield turns on its armor and weapon components, armor recalculates
/// its stats.
/// `ItemEntity.ReapplyAbilities` (`Kingmaker.Items/ItemEntity.cs:954`) when the item's
/// ability or activatable ability isn't the blueprint's: the old fact leaves the wielder,
/// the blueprint's is added (`Wielder.AddFact<Ability>`), with the item as its source
/// (POC-5).
fn reapply_abilities(w: &mut World, e: EntityIdx) {
    let Some(item) = w.item(e).cloned() else { return };
    let Some(wielder) = item.wielder else { return };
    let Some(bp) = item.blueprint.and_then(|g| w.content.get(g)) else { return };
    for (field, current) in
        [("m_Ability", item.ability), ("m_ActivatableAbility", item.activatable_ability)]
    {
        let want = bp.view().guid(field);
        let mut cur = current;
        if let Some(f) = cur
            && w.fact(f).blueprint != want
        {
            if w.fact(f).owner == Some(wielder) {
                crate::unit_facts::remove(w, wielder, f);
            }
            cur = None;
        }
        if cur.is_none()
            && let Some(abp) = want.and_then(|g| w.content.get(g))
        {
            cur = crate::unit_facts::add_fact(w, wielder, &abp);
            if let Some(f) = cur {
                w.fact_mut(f).source_item = Some(e);
            }
        }
        if let Some(it) = w.item_mut(e) {
            if field == "m_Ability" {
                it.ability = cur;
            } else {
                it.activatable_ability = cur;
            }
        }
    }
}

/// `ItemEntity.ApplyEnchantments(onInitializeOrEquip: false)` (from `OnTurnOn`).
fn check_item_enchantments(w: &mut World, e: EntityIdx, bp: &Rc<Blueprint>) {
    crate::items::apply_enchantments_on_turn_on(w, e, bp);
}

fn item_on_turn_on(w: &mut World, e: EntityIdx) {
    let Some(item) = w.item(e).cloned() else { return };
    let Some(bp) = item.blueprint.and_then(|g| w.content.get(g)) else { return };
    check_item_enchantments(w, e, &bp);
    if item.wielder.is_some() {
        let v = bp.view();
        // `ReapplyAbilities`: nothing to do when the saved facts already match the
        // blueprint's (the C# only removes or adds on a mismatch).
        if item.is_identified {
            let matches =
                |fact: Option<crate::world::FactIdx>, want: Option<wr_content::Guid>| match (
                    fact, want,
                ) {
                    (Some(f), Some(g)) => w.fact(f).blueprint == Some(g),
                    (None, None) => true,
                    (Some(f), None) => w.fact(f).blueprint.is_none(),
                    (None, Some(_)) => false,
                };
            if !matches(item.ability, v.guid("m_Ability"))
                || !matches(item.activatable_ability, v.guid("m_ActivatableAbility"))
            {
                reapply_abilities(w, e);
            }
        }
        if bp
            .components_array()
            .iter()
            .any(|o| crate::content::is_a(o.ty, "AddFactToEquipmentWielder"))
        {
            w.coverage.unsupported("ItemEntity.ReapplyFactsForWielder");
        }
    }
    match w.entity(e).clr {
        // `ItemEntityShield.OnTurnOn`: then its components.
        "ItemEntityShield" => {
            for c in [item.armor_component, item.weapon_component].into_iter().flatten() {
                if !w.entity(c).is_turned_on {
                    turn_on(w, c);
                }
            }
        }
        // `ItemEntityArmor.OnTurnOn`: `RecalculateStats()`.
        "ItemEntityArmor" => {
            w.coverage.executed("ItemEntityArmor.RecalculateStats");
            crate::armor::recalculate_stats(w, e);
        }
        // `ItemEntityWeapon.OnTurnOn`: a double weapon's second half (not modelled; none
        // in the fixtures is double), then visual effects.
        "ItemEntityWeapon" | "ItemEntitySimple" | "ItemEntityUsable" => {}
        other => w.coverage.unsupported(format!("{other}.OnTurnOn")),
    }
}

/// `EntityDataBase.Dispose()` (`Kingmaker.EntitySystem/EntityDataBase.cs:515`): unsubscribe,
/// `OnDispose` (the unit's: its unit subscriptions go, then `UnitDescriptor.Dispose` disposes
/// the body's items and a unit-owned inventory's; an item's leaves its collection), the
/// parts, the facts (the owner is disposing, so each only unsubscribes and disposes its
/// components: no `Detach`), then the entity leaves the registry.
pub fn dispose(w: &mut World, e: EntityIdx) {
    if w.entity(e).is_disposed {
        return;
    }
    if w.entity(e).subscribed {
        w.entity_mut(e).subscribed = false;
        crate::rulebook::bus::unsubscribe(w, crate::rulebook::bus::Sub::Entity(e));
    }
    w.entity_mut(e).disposing = true;
    w.trace("Entity.Dispose", |w| w.entity(e).unique_id.clone());
    match &w.entity(e).kind {
        EntityKind::Unit(_) => unit_on_dispose(w, e),
        EntityKind::Item(_) => item_on_dispose(w, e),
        EntityKind::Other | EntityKind::DroppedLoot(_) => {}
    }
    // `View` (presentation). `Parts.Dispose()`: each part leaves (`OnPartRemoved`, `TurnOff`
    // if the owner is on, `Dispose`); the save schema's hook table has no dispose hooks, so
    // only the inert parts are known to do nothing there.
    for p in w.entity(e).parts.clone() {
        if p.clr == crate::components::shifters_fury::PART {
            crate::components::shifters_fury::part_dispose(w, e);
            continue;
        }
        // `UnitPartSpellResistance` overrides nothing (`EntityPart.OnTurnOff`/`OnDispose` are
        // empty for it).
        if !INERT_PARTS.contains(&p.clr) && p.clr != "UnitPartSpellResistance" {
            w.coverage.unsupported(format!("{}: part removal on Dispose", p.clr));
        }
    }
    w.entity_mut(e).parts.clear();
    // `Facts.Dispose()`: over a snapshot, the facts this manager still owns.
    for f in w.entity(e).facts.list.clone() {
        if w.fact(f).owner == Some(e) {
            fl::dispose(w, f);
        }
    }
    w.entity_mut(e).facts.list.clear();
    crate::combat::vision::note_corpulence_change(w, e);
    // `EntityService.Unregister`.
    let id = w.entity(e).unique_id.clone();
    if w.by_id.get(&id) == Some(&e) {
        w.by_id.remove(&id);
    }
    let ent = w.entity_mut(e);
    ent.is_disposed = true;
    ent.disposing = false;
}

/// `UnitEntityData.OnDispose` (`UnitEntityData.cs:1731`) and `UnitDescriptor.Dispose`
/// (`UnitDescriptor.cs:560`).
fn unit_on_dispose(w: &mut World, e: EntityIdx) {
    // `EventBus.ClearUnitSubscriptions(this)`: the unit's lists, whoever subscribed there.
    w.bus.unit.remove(&e);
    w.bus.rule_target.remove(&e);
    w.bus.rule_initiator.remove(&e);
    if crate::combat::is_in_combat(w, e) {
        w.coverage.unsupported("UnitEntityData.OnDispose: LeaveCombat");
    }
    // `m_Group?.Remove(this)`: modelled while groups are, else reported.
    if w.combat.groups.on {
        crate::combat::groups::drop_cached(w, e);
    } else if w.unit(e).is_some_and(|u| u.group_id.is_some()) {
        w.coverage.unsupported("UnitEntityData.OnDispose: leaving the unit group");
    }
    // `Descriptor.Dispose()`: `Body.Dispose()` (every slot's item, then the empty hands),
    // `Progression.Dispose()` (its feature processor: no facts), spellbooks, and the unit's
    // own inventory (`!IsPlayerFaction && Inventory.OwnerUnit == this`).
    if let Some(unit) = w.unit(e) {
        let slots: Vec<String> =
            unit.body.slots.iter().chain(unit.body.limbs.iter()).map(|s| s.name.clone()).collect();
        for name in slots {
            // `AllSlots.Where(s => s.HasItem).ForEach(s => s.Item.Dispose())`.
            if let Some(it) = crate::items::slot_item(w, e, &name) {
                dispose(w, it);
            }
        }
    }
    // `AllEmptyHands`: the empty-hand weapon, then the stack.
    let empties: Vec<EntityIdx> = w
        .unit(e)
        .map(|u| {
            u.body.empty_hand.into_iter().chain(u.body.empty_hand_stack.iter().copied()).collect()
        })
        .unwrap_or_default();
    for empty in empties {
        dispose(w, empty);
    }
    // `Spellbook.Dispose()` disposes the slots (`SpellSlot.Dispose()` does nothing).
    if let Some(u) = w.unit(e)
        && !u.is_player_faction
        && u.has_own_inventory
        && let Some(col) = u.inventory_collection
    {
        // `ItemsCollection.Dispose()`: `IsDisposingNow`, each item disposed.
        let items = u.inventory.clone();
        w.disposing_collections.push(col);
        for it in items {
            dispose(w, it);
        }
        w.disposing_collections.retain(|c| *c != col);
    }
}

/// `ItemEntity.OnDispose` (`ItemEntity.cs:946`): the item leaves its collection unless the
/// collection is being disposed (`Collection.Remove(this)`: out of the list, out of its slot
/// with the unequip hooks); a shield or a double weapon disposes its parts.
fn item_on_dispose(w: &mut World, e: EntityIdx) {
    if let Some(col) = w.item(e).and_then(|i| i.collection)
        && !w.disposing_collections.contains(&col)
    {
        crate::items::collection_remove(w, col, e);
    }
    let (armor, weapon) = match w.item(e) {
        Some(i) if w.entity(e).clr == "ItemEntityShield" => (i.armor_component, i.weapon_component),
        _ => (None, None),
    };
    for c in [armor, weapon].into_iter().flatten() {
        dispose(w, c);
    }
}
