//! `AuraFeatureComponent` and `AddLoot` (decompiled, not copied:
//! `Kingmaker.Designers.Mechanics.Facts/AuraFeatureComponent.cs`, `AuraFeatureComponentData.cs`,
//! `Kingmaker.UnitLogic.FactLogic/AddLoot.cs`, `Kingmaker.Blueprints.Loot/BlueprintUnitLoot.cs`,
//! `LootItemsPackFixed.cs`, `LootItem.cs`, `Kingmaker.Items/ItemsCollection.cs`).
//!
//! **Aura:** `OnActivate` is `Data.AppliedBuff = Owner.AddBuff(Buff, Fact.MaybeContext)` and
//! `OnDeactivate` removes it. The aura's area effect belongs to the buff's own components
//! (`AddAreaEffect` and the like), which report themselves where they run. The applied buff
//! is the component data's `AppliedBuff` (kept in `fact_refs`). `HandleUnitLifeStateChanged`
//! (re-applies or removes the buff as the owner falls or rises) is not ported: it reports
//! when raised.
//!
//! **Loot:** `HandleUnitSpawned(unit)` for the owner, unless `DoNotCreateItems`: the unit
//! loot's fixed packs (nested loots included) into the inventory. A random pack
//! (`LootItemsPackVariable` uses `UnityEngine.Random`) reports unsupported.

use std::rc::Rc;

use wr_content::Guid;

use super::{Port, bits, owner_unit, with_settings};
use crate::content::{Blueprint, View, is_a};
use crate::registry::{Hook, Registry};
use crate::world::{CompRef, FactIdx, World};

pub fn register(r: &mut Registry) {
    r.register(Box::new(Port::new(
        "AuraFeatureComponent",
        bits(&[Hook::Activate, Hook::Deactivate]),
        aura_hook,
    )));
    let mut loot = Port::new("AddLoot", 0, |_, _, _| {});
    loot.handlers = &["IUnitSpawnHandler"];
    loot.ev = Some(add_loot_event);
    r.register(Box::new(loot));
}

const APPLIED: &str = "AppliedBuff";

fn applied(w: &World, c: CompRef) -> Option<FactIdx> {
    w.component(c).fact_refs.get(APPLIED).and_then(|v| v.first().copied().flatten())
}

fn aura_hook(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::Activate => {
            let buff = with_settings(w, c, |v| v.guid("m_Buff")).flatten();
            // `UnitHelper.AddBuff` answers null for a missing blueprint.
            let added = buff.and_then(|g| w.content.get(g)).and_then(|bp| {
                let parent = w.fact(c.fact).context.clone();
                let ctx = match &parent {
                    // `Buffs.AddBuff(blueprint, parentContext, null)`: `parent.CloneFor(..)`.
                    Some(p) => {
                        w.pending_buffs.parent_descriptor = p.spell_descriptor;
                        crate::unit_facts::clone_for(w, p, &bp, u)
                    }
                    // No context: `AddBuff(blueprint, Unit)`, the unit its own caster.
                    None => crate::unit_facts::context(w, Some(u), Some(u), &bp),
                };
                crate::unit_facts::add_buff(w, u, &bp, ctx, None)
            });
            w.component_mut(c).fact_refs.insert(APPLIED.to_owned(), vec![added]);
        }
        Hook::Deactivate => {
            // `Data.AppliedBuff?.Remove()`: a buff that no longer has a manager only has
            // its end time set, which nothing reads once it is off the unit.
            if let Some(b) = applied(w, c)
                && let Some(o) = w.fact(b).owner
            {
                crate::unit_facts::remove(w, o, b);
            }
            w.component_mut(c).fact_refs.insert(APPLIED.to_owned(), vec![None]);
        }
        _ => {}
    }
}

/// `AddLoot.HandleUnitSpawned(unit)`.
fn add_loot_event(w: &mut World, c: CompRef, _iface: &str) {
    let Some(("HandleUnitSpawned", Some(unit))) = w.bus.call.last().copied() else {
        w.coverage.unsupported("AddLoot: IUnitSpawnHandler (other callback)");
        return;
    };
    if owner_unit(w, c) != Some(unit) || w.build.do_not_create_items {
        return;
    }
    let Some(loot) = with_settings(w, c, |v| v.guid("m_Loot")).flatten() else { return };
    let Some(loot_bp) = w.content.get(loot) else {
        w.coverage.unsupported("AddLoot: loot blueprint missing");
        return;
    };
    let mut entries: Vec<(Guid, i32)> = Vec::new();
    if !add_items_to(w, &loot_bp, &mut entries) {
        return;
    }
    for (item, count) in entries {
        let Some(bp) = w.content.get(item) else {
            w.coverage.unsupported("AddLoot: item blueprint missing");
            continue;
        };
        crate::items::inventory_add_blueprint(w, unit, &bp, count);
    }
}

/// `BlueprintUnitLoot.AddItemsTo(list)`; `false` when a component isn't ported.
fn add_items_to(w: &mut World, loot: &Rc<Blueprint>, out: &mut Vec<(Guid, i32)>) -> bool {
    for o in loot.components_array() {
        if !is_a(o.ty, "BlueprintLootComponent") {
            continue;
        }
        let v = View(o);
        if !is_a(o.ty, "LootItemsPackFixed") {
            w.coverage.unsupported(format!("AddLoot: {}", v.type_name()));
            return false;
        }
        let Some(item) = v.obj("m_Item") else { continue };
        if !item_add_to(w, item, v.i32("m_Count"), out) {
            return false;
        }
    }
    true
}

/// `LootItem.AddItemTo(list, count)`.
fn item_add_to(w: &mut World, item: View<'_>, count: i32, out: &mut Vec<(Guid, i32)>) -> bool {
    match item.int("m_Type") {
        0 => {
            let Some(g) = item.guid("m_Item") else {
                w.coverage.unsupported("AddLoot: loot item without a blueprint");
                return false;
            };
            match out.iter_mut().find(|(i, _)| *i == g) {
                Some(e) => e.1 = e.1.wrapping_add(count),
                None => out.push((g, count)),
            }
            true
        }
        _ => {
            let Some(g) = item.guid("m_Loot") else { return true };
            let Some(nested) = w.content.get(g) else {
                w.coverage.unsupported("AddLoot: nested loot blueprint missing");
                return false;
            };
            for _ in 0..count.max(0) {
                if !add_items_to(w, &nested, out) {
                    return false;
                }
            }
            true
        }
    }
}
