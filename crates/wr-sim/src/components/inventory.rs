//! Components that watch an inventory (decompiled, not copied):
//! `PartyInventoryTrigger` (`Kingmaker.Designers.EventConditionActionSystem.Events/`) and the
//! `TutorialTriggerNewItem` family (`Kingmaker.Tutorial.Triggers/`), both
//! `IItemsCollectionHandler.HandleItemsAdded/HandleItemsRemoved(collection, item, count)`
//! (the collection and item of the event are in [`crate::rulebook::bus::EventBus::item`]).

use super::{Port, settings_of};
use crate::content::is_a;
use crate::registry::Registry;
use crate::world::{CompRef, World};

pub fn register(r: &mut Registry) {
    r.register(Box::new(Port {
        name: "PartyInventoryTrigger",
        bits: 0,
        f: |_, _, _| {},
        handlers: &["IItemsCollectionHandler"],
        ev: Some(party_inventory_trigger),
    }));
    for name in [
        "TutorialTriggerNewGripFlexibleWeapon",
        "TutorialTriggerNewRodItem",
        "TutorialTriggerNewWand",
        "TutorialTriggerNewItemWithEnchantment",
        "TutorialTriggerNewNotLearnedScroll",
        "TutorialTriggerHealingScroll",
        "TutorialTriggerNewRecipe",
    ] {
        let handlers = wr_content::schema::find(name)
            .map(|t| wr_content::schema::ty(t).handlers)
            .unwrap_or(&[]);
        r.register(Box::new(Port {
            name,
            bits: 0,
            f: |_, _, _| {},
            handlers,
            ev: Some(tutorial_trigger_new_item),
        }));
    }
}

/// `PartyInventoryTrigger.HandleItemsAdded`/`HandleItemsRemoved`: an event for the trigger's
/// item in a party member's inventory runs the matching action list.
fn party_inventory_trigger(w: &mut World, c: CompRef, iface: &str) {
    if iface != "IItemsCollectionHandler" {
        return;
    }
    let Some(&(method, _)) = w.bus.call.last() else { return };
    let Some(&(col, item)) = w.bus.item.last() else { return };
    let Some((owner, idx)) = settings_of(w, c) else { return };
    let objs = owner.components_array();
    let v = crate::content::View(objs[idx]);
    let want = v.guid("m_Item");
    if w.item(item).and_then(|i| i.blueprint) != want {
        return;
    }
    let list = match method {
        "HandleItemsAdded" => "OnAddActions",
        "HandleItemsRemoved" => "OnRemoveActions",
        _ => return,
    };
    // `foreach (partyAndPet in PartyAndPets) if (partyAndPet.Inventory == collection)`.
    let party = crate::narrative::party::party(w);
    let pets = w.entities.iter().any(|e| {
        matches!(&e.kind, crate::world::EntityKind::Unit(u)
            if u.is_pet && u.master.is_some_and(|m| party.contains(&m)))
    });
    if pets {
        w.coverage.unsupported("PartyInventoryTrigger: pets (PartyAndPets)");
    }
    if party.iter().any(|u| w.unit(*u).and_then(|x| x.inventory_collection) == Some(col)) {
        crate::elements::run_actions(w, &owner, v.obj(list));
    }
}

/// `TutorialTriggerNewItem.HandleItemsRemoved` does nothing; `HandleItemsAdded` calls the
/// subclass's `TryToTrigger(collection.IsPlayerInventory, item)`: an item none of them
/// looks at doesn't trigger; one a subclass would look at is reported (the tutorial system
/// is presentation, but its trigger counters are not ported).
fn tutorial_trigger_new_item(w: &mut World, c: CompRef, iface: &str) {
    if iface != "IItemsCollectionHandler" {
        return;
    }
    let Some(&(method, _)) = w.bus.call.last() else { return };
    if method != "HandleItemsAdded" {
        return;
    }
    let Some(&(_, item)) = w.bus.item.last() else { return };
    let Some(bp) = w.item(item).and_then(|i| i.blueprint).and_then(|g| w.content.get(g)) else {
        return;
    };
    let ty = w.component(c).ty().map(|t| wr_content::schema::ty(t).short).unwrap_or("");
    let t = bp.obj.ty;
    let looks = match ty {
        "TutorialTriggerNewGripFlexibleWeapon" => is_a(t, "BlueprintItemWeapon"),
        "TutorialTriggerNewRodItem"
        | "TutorialTriggerNewWand"
        | "TutorialTriggerNewNotLearnedScroll"
        | "TutorialTriggerHealingScroll" => is_a(t, "BlueprintItemEquipmentUsable"),
        "TutorialTriggerNewItemWithEnchantment" => {
            !w.entity(item).facts.list.is_empty() || is_a(t, "BlueprintItemShield")
        }
        "TutorialTriggerNewRecipe" => {
            bp.components_array().iter().any(|o| is_a(o.ty, "CopyRecipe"))
        }
        _ => true,
    };
    if looks {
        w.coverage.unsupported(format!("{ty}.TryToTrigger"));
    }
}
