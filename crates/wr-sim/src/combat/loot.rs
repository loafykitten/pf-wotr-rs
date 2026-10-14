//! `UnitEntityData.DropLootToGround` (decompiled, not copied:
//! `Kingmaker.EntitySystem.Entities/UnitEntityData.cs`, with `Kingmaker.View.MapObjects/
//! DroppedLoot.cs` and `EntityCreationController.SpawnEntityView`).
//!
//! A dying non-player unit that holds lootable items spawns a dropped-loot map object: a new
//! entity (it takes the next unique id), whose loot collection *is* the unit's inventory (the
//! items move to their own collection only when the unit is destroyed,
//! `TransferInventoryToDroppedLoot`), with a random yaw drawn from Unity's generator. The unit
//! remembers it in `m_DroppedLootEntityDataRef`.
//!
//! Not ported, and reported where the drop happens: everything the dropped-loot *prefab*
//! decides (the view, its scale, the parts its components create: `InteractionLootPart`'s
//! settings and `Enabled`, `LocalMapMarkerPart`'s description, `InteractionLootPart.SetUnit`)
//! and the creation tick (`EntityCreationController.m_ToCreate`, the map object's handlers).

use crate::content::is_a;
use crate::rng::sites;
use crate::unit_state::{cond, has_condition};
use crate::world::{Entity, EntityIdx, EntityKind, FactsManager, Part, World};

/// `DroppedLoot.EntityData`: the members the simulation reads.
#[derive(Debug, Clone, Default)]
pub struct DroppedLoot {
    /// `DroppedBy` (`m_DroppedBy`).
    pub dropped_by: Option<EntityIdx>,
    /// `IsDroppedByPlayer`, set by the `DroppedBy` setter.
    pub is_dropped_by_player: bool,
    /// `IsDismember`.
    pub is_dismember: bool,
    /// `BloodType` (set for a dismember drop); `None`: not established.
    pub blood_type: Option<i32>,
    /// `Loot` (`InteractionLootPart.Loot`): identity of the `ItemsCollection`.
    pub loot: Option<usize>,
}

/// `BloodType.Stone`.
const BLOOD_STONE: i32 = 9;

/// `UnitEntityData.HasLoot`: a unit not of the player's faction, not `Unlootable`, with a
/// lootable item, and no dropped loot yet. `None` (reported) where an item's lootability
/// can't be told.
fn has_loot(w: &mut World, u: EntityIdx) -> Option<bool> {
    let Some(unit) = w.unit(u) else { return Some(false) };
    if unit.is_player_faction || has_condition(w, u, cond::UNLOOTABLE) {
        return Some(false);
    }
    // `Inventory.Any(i => i.IsLootable)` stops at the first lootable item.
    for item in unit.inventory.clone() {
        match crate::items::is_lootable(w, item) {
            Some(true) => return Some(w.unit(u).is_some_and(|x| !x.has_dropped_loot_ref)),
            Some(false) => {}
            None => {
                w.coverage.unsupported("ItemEntity.IsLootable: the weapon's type is not available");
                return None;
            }
        }
    }
    Some(false)
}

/// `UnitEntityData.BloodType`: the override (`ChangeHitFxType`, not saved), `Stone` while
/// petrified, else the blueprint's `Visual.BloodType`. `None` where it isn't established.
fn blood_type(w: &World, u: EntityIdx) -> Option<i32> {
    let overridden = w.entity(u).facts.list.iter().any(|f| {
        w.fact(*f)
            .bp
            .as_ref()
            .is_some_and(|b| b.components_array().iter().any(|o| is_a(o.ty, "ChangeHitFxType")))
    });
    if overridden {
        return None;
    }
    if has_condition(w, u, cond::PETRIFIED) {
        return Some(BLOOD_STONE);
    }
    let bp = w.content.get(w.unit(u)?.blueprint?)?;
    let visual = bp.view().obj("Visual")?;
    i32::try_from(visual.int("BloodType")).ok()
}

/// `ItemEntity.NeedSkinningForCollect` for an item of the unit's inventory: the blueprint
/// needs skinning, it hasn't succeeded, and the collection belongs to a unit
/// (`Collection.OwnerUnit != null`: the unit's own inventory).
fn need_skinning_for_collect(w: &World, u: EntityIdx, item: EntityIdx) -> bool {
    let Some(it) = w.item(item) else { return false };
    let needs = it
        .blueprint
        .and_then(|g| w.content.get(g))
        .is_some_and(|b| b.view().bool("NeedSkinningForCollect"));
    needs && !it.skinning_successful && w.unit(u).is_some_and(|x| x.has_own_inventory)
}

/// `UnitEntityData.DropLootToGround(dismember, overridePos: null, force)`.
pub fn drop_loot_to_ground(w: &mut World, u: EntityIdx, dismember: bool, force: bool) {
    // `if (!(HasLoot | force)) return;`
    let has = has_loot(w, u);
    if has.is_none() || !(has == Some(true) || force) {
        return;
    }
    // The prefab (`GetDismemberLoot(BloodType)` or `DroppedLootBag`) only chooses the view.
    // `SpawnEntityView`: `Player.GetNewUniqueId()` names the entity, `CreateEntityData`
    // builds `DroppedLoot.EntityData`.
    let id = crate::narrative::Player::new_unique_id(w);
    let dropped = w.add_entity(Entity {
        unique_id: id,
        clr: "DroppedLoot.EntityData",
        kind: EntityKind::DroppedLoot(Box::default()),
        facts: FactsManager::default(),
        parts: Vec::new(),
        is_turned_on: false,
        post_load_executed: false,
        is_in_game: true,
        subscribed: false,
        disposing: false,
        is_disposed: false,
    });
    // `localScale *= GetScaleMultiplierBySize` is the view's; the yaw is Unity's generator.
    w.rng.range_float(sites::DROP_LOOT_ROTATION, 0.0, 360.0);
    w.coverage.unsupported(
        "UnitEntityData.DropLootToGround: the dropped loot prefab's view, scale and parts (InteractionLootPart.SetUnit, map marker settings) and its creation tick",
    );
    if dismember {
        // `entityData.IsDismember = true; BloodType = BloodType;` then the items that need
        // skinning leave the inventory.
        let blood = blood_type(w, u);
        if blood.is_none() {
            w.coverage.unsupported(
                "UnitEntityData.BloodType: ChangeHitFxType override or no visual settings",
            );
        }
        if let EntityKind::DroppedLoot(d) = &mut w.entity_mut(dropped).kind {
            d.is_dismember = true;
            d.blood_type = blood;
        }
        // `Inventory.Items.Where(i => i.NeedSkinningForCollect).ToArray().ForEach(Inventory.Remove)`.
        let col = w.unit(u).and_then(|x| x.inventory_collection);
        for item in w.unit(u).map(|x| x.inventory.clone()).unwrap_or_default() {
            if need_skinning_for_collect(w, u, item)
                && let Some(col) = col
            {
                crate::items::collection_remove(w, col, item);
            }
        }
    }
    // `droppedLoot.Loot = Inventory` (`Ensure<InteractionLootPart>()`), `DroppedBy = unit`
    // (`IsDroppedByPlayer = unit.IsPlayerFaction`), `m_DroppedLootEntityDataRef`.
    let col = w.unit(u).and_then(|x| x.inventory_collection);
    let by_player = w.unit(u).is_some_and(|x| x.is_player_faction);
    w.entity_mut(dropped).parts.push(Part { clr: "InteractionLootPart" });
    if let EntityKind::DroppedLoot(d) = &mut w.entity_mut(dropped).kind {
        d.loot = col;
        d.dropped_by = Some(u);
        d.is_dropped_by_player = by_player;
    }
    if let Some(x) = w.unit_mut(u) {
        x.has_dropped_loot_ref = true;
        x.dropped_loot = Some(dropped);
    }
    // `Ensure<LocalMapMarkerPart>()`: `IsRuntimeCreated`, `Type = Loot`, the unit's name.
    w.entity_mut(dropped).parts.push(Part { clr: "LocalMapMarkerPart" });
}

#[cfg(test)]
mod tests {
    use wr_content::Guid;
    use wr_content::node::Node;

    use super::*;
    use crate::content::{Blueprint, Source};
    use crate::facts::tests::obj;
    use crate::item::Item;
    use crate::stats::tests::world_with_unit;

    fn add_item(w: &mut World, u: EntityIdx, id: &str, item: Item) -> EntityIdx {
        let item = Item { count: 1, collection: Some(1), ..item };
        let e = w.add_entity(Entity {
            unique_id: id.into(),
            clr: "ItemEntityUsable",
            kind: EntityKind::Item(Box::new(item)),
            facts: FactsManager::default(),
            parts: Vec::new(),
            is_turned_on: true,
            post_load_executed: true,
            is_in_game: true,
            subscribed: false,
            disposing: false,
            is_disposed: false,
        });
        let x = w.unit_mut(u).expect("unit");
        x.inventory.push(e);
        x.inventory_collection = Some(1);
        x.has_own_inventory = true;
        e
    }

    fn dropped(w: &World) -> Vec<EntityIdx> {
        (0..w.entities.len())
            .map(|i| EntityIdx(u32::try_from(i).expect("small")))
            .filter(|e| matches!(w.entity(*e).kind, EntityKind::DroppedLoot(_)))
            .collect()
    }

    /// `HasLoot` gates the drop: nothing to loot, a player-faction owner, `Unlootable`, or loot
    /// already dropped each leave the world exactly as it was (no id, no draw).
    #[test]
    fn nothing_drops_without_lootable_items() {
        let (mut w, u) = world_with_unit();
        // A natural attack or `m_NotLootable` item isn't lootable.
        add_item(&mut w, u, "i1", Item { not_lootable: true, ..Item::default() });
        let next = w.player.max_unique_id;
        drop_loot_to_ground(&mut w, u, true, false);
        assert!(dropped(&w).is_empty());
        assert_eq!(w.player.max_unique_id, next, "no id taken");
        assert!(w.rng.log().is_empty(), "no draw");
        assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
        // A lootable item, but the owner is in the player's faction / already dropped.
        add_item(&mut w, u, "i2", Item::default());
        w.unit_mut(u).expect("unit").is_player_faction = true;
        drop_loot_to_ground(&mut w, u, true, false);
        w.unit_mut(u).expect("unit").is_player_faction = false;
        w.unit_mut(u).expect("unit").has_dropped_loot_ref = true;
        drop_loot_to_ground(&mut w, u, true, false);
        assert!(dropped(&w).is_empty());
        assert_eq!(w.player.max_unique_id, next);
    }

    /// A lootable item drops: the next unique id names a new `DroppedLoot` entity holding the
    /// unit's own collection and remembering who dropped it, the rotation is drawn, and the unit
    /// remembers the entity; the view and parts the prefab decides are reported.
    #[test]
    fn a_unit_with_loot_drops_a_loot_entity() {
        let (mut w, u) = world_with_unit();
        add_item(&mut w, u, "i1", Item { not_lootable: true, ..Item::default() });
        add_item(&mut w, u, "i2", Item::default());
        let next = format!("{:X}", w.player.max_unique_id);
        drop_loot_to_ground(&mut w, u, true, false);
        let d = dropped(&w);
        assert_eq!(d.len(), 1);
        assert_eq!(w.entity(d[0]).unique_id, next);
        let EntityKind::DroppedLoot(l) = &w.entity(d[0]).kind else { unreachable!() };
        assert_eq!((l.dropped_by, l.loot, l.is_dismember), (Some(u), Some(1), true));
        assert!(!l.is_dropped_by_player);
        let unit = w.unit(u).expect("unit");
        assert!(unit.has_dropped_loot_ref);
        assert_eq!(unit.dropped_loot, Some(d[0]));
        let draws = w.rng.log();
        assert_eq!(draws.len(), 1);
        assert_eq!(draws[0].site, sites::DROP_LOOT_ROTATION.0);
        assert!(
            w.coverage
                .unsupported
                .keys()
                .any(|k| k.starts_with("UnitEntityData.DropLootToGround:"))
        );
        // Dropped once: `m_DroppedLootEntityDataRef` is set, so `HasLoot` is false.
        drop_loot_to_ground(&mut w, u, true, false);
        assert_eq!(dropped(&w).len(), 1);
    }

    /// A dismember drop takes the items that need skinning out of the inventory first.
    #[test]
    fn a_dismember_drop_removes_items_that_need_skinning() {
        let (mut w, u) = world_with_unit();
        let pelt = obj(
            "Kingmaker.Blueprints.Items.BlueprintItem",
            &[("NeedSkinningForCollect", Node::Bool(true)), ("Components", Node::Seq(Vec::new()))],
        );
        w.content.insert(Blueprint {
            guid: Guid(950),
            name: "Pelt".into(),
            obj: pelt,
            source: Source::Bbp,
        });
        add_item(&mut w, u, "i1", Item::default());
        let skin =
            add_item(&mut w, u, "i2", Item { blueprint: Some(Guid(950)), ..Item::default() });
        drop_loot_to_ground(&mut w, u, true, false);
        let inventory = &w.unit(u).expect("unit").inventory;
        assert!(!inventory.contains(&skin), "the pelt left the inventory");
        assert_eq!(inventory.len(), 1);
        // Not dismembered: it stays.
        let (mut w, u) = world_with_unit();
        add_item(&mut w, u, "i1", Item::default());
        let skin =
            add_item(&mut w, u, "i2", Item { blueprint: Some(Guid(950)), ..Item::default() });
        drop_loot_to_ground(&mut w, u, false, false);
        assert!(w.unit(u).expect("unit").inventory.contains(&skin));
    }
}
