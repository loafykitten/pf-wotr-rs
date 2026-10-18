//! Saved `UnitPlaceholderEntityData` (`Kingmaker.EntitySystem.Entities/UnitPlaceholderEntityData.cs`):
//! a unit stored compressed. Deserialization keeps what `BecomeUnit` passes on (blueprint,
//! unit id, group, pose, flags, saved body and inventory, pools, faction); the unit itself
//! is built from its blueprint during the scene's post-load pass ([`crate::spawn`]). Its
//! entity is reserved here (empty, not turned on) so the saved items' slots and wielder
//! links resolve to it as they do for full units.

use super::Loader;
use super::graph::{O, guid_of};
use super::narrative::vec_of;
use crate::spawn::PlaceholderData;
use crate::world::{Entity, EntityIdx, EntityKind, FactsManager};

impl Loader<'_> {
    /// A placeholder's data and its reserved unit entity.
    pub(super) fn placeholder(&mut self, o: O<'_>) -> PlaceholderData {
        let id = o.str("m_Id").unwrap_or_default().to_owned();
        let e = self.w.add_entity(Entity {
            unique_id: id.clone(),
            clr: "UnitEntityData",
            kind: EntityKind::Unit(Box::default()),
            facts: FactsManager::default(),
            parts: Vec::new(),
            is_turned_on: false,
            post_load_executed: false,
            is_in_game: false,
            subscribed: false,
            disposing: false,
            is_disposed: false,
        });
        let inv = o.obj("m_Inventory");
        let inventory: Vec<EntityIdx> = inv
            .map(|c| c.objs("m_Items"))
            .unwrap_or_default()
            .into_iter()
            .map(|i| self.item_for(i))
            .collect();
        let body = o.obj("m_Body").map(|b| self.body(b, e));
        // The saved body lives on the reserved unit, so slots resolve to it.
        if let Some(u) = self.w.unit_mut(e) {
            u.body = body.clone().unwrap_or_default();
            u.inventory = inventory.clone();
        }
        let pos = vec_of(o.get("m_Position")).unwrap_or_default();
        let unknown: Vec<&str> =
            ["m_DollData", "m_Ragdoll", "m_Dismemberment", "m_Conditions", "m_AppliedUpgraders"]
                .into_iter()
                .filter(|k| o.get(k).is_some_and(|v| !matches!(v, wr_save::value::Value::Null)))
                .chain(stealth_differs(o).then_some("m_Stealth"))
                .collect();
        PlaceholderData {
            entity: e,
            unit_id: id,
            blueprint: o.guid("m_Blueprint"),
            group_id: o.str("m_GroupId").map(str::to_owned),
            position: [
                pos.first().copied().unwrap_or(0.0),
                pos.get(1).copied().unwrap_or(0.0),
                pos.get(2).copied().unwrap_or(0.0),
            ],
            orientation: match o.get("m_Orientation") {
                Some(wr_save::value::Value::F32(f)) => *f,
                #[allow(clippy::cast_possible_truncation)]
                Some(wr_save::value::Value::F64(f)) => *f as f32,
                _ => 0.0,
            },
            is_in_game: o.bool_or("m_IsInGame", true),
            is_revealed: o.bool_or("m_IsRevealed", true),
            is_finally_dead: o.bool_or("m_IsFinallyDead", false),
            is_extra: o.bool_or("m_IsExtra", false),
            race: o.guid("m_Race"),
            faction: o.guid("m_Faction"),
            roaming: o.obj("m_Roaming").is_some(),
            roaming_idle_cutscene: o
                .obj("m_Roaming")
                .is_some_and(|r| r.obj("IdleCutscene").is_some()),
            roaming_next_idle_cutscene: o
                .obj("m_Roaming")
                .is_some_and(super::graph::roaming_next_idle_cutscene),
            roaming_part: o.obj("m_Roaming").map(super::graph::roaming_part),
            vendor_table: o.guid("m_VendorTable"),
            has_prefab_id: o.str("m_PrefabId").is_some_and(|s| !s.is_empty()),
            prefab_id: o.str("m_PrefabId").filter(|s| !s.is_empty()).map(str::to_owned),
            doll: o.get("m_DollData").is_some_and(|v| !matches!(v, wr_save::value::Value::Null)),
            optimized_hide: o.bool_or("m_IsOptimizedHide", false),
            pools: match o.get("m_Pools") {
                Some(wr_save::value::Value::Seq(v)) => v.iter().filter_map(guid_of).collect(),
                _ => Vec::new(),
            },
            body,
            inventory,
            inventory_collection: inv.map(|c| c.id()),
            unmodelled: unknown.into_iter().map(str::to_owned).collect(),
        }
    }
}

/// Whether the saved `m_Stealth` (`UnitStealth`, written with `DefaultValueHandling.Ignore`)
/// holds anything but the defaults of a fresh `UnitStealth`, which is what the unit has
/// before `SetStealthData` replaces it (`UnitEntityData.cs:164`).
fn stealth_differs(o: O<'_>) -> bool {
    use wr_save::value::Value;
    let Some(s) = o.obj("m_Stealth") else { return false };
    [
        "m_SpottedBy",
        "CachedRoll",
        "FullSpeed",
        "ShouldExitStealth",
        "WantEnterStealth",
        "LastNearEnemyTime",
        "NearEnemyPenalty",
        "ForceEnterStealth",
        "BecameInvisibleThisFrame",
        "InAmbush",
        "AmbushJoinCombatDistance",
        "AmbushTake20",
    ]
    .into_iter()
    .any(|k| match s.get(k) {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Int(i)) => *i != 0,
        Some(Value::UInt(u)) => *u != 0,
        Some(Value::F32(f)) => *f != 0.0,
        Some(Value::F64(f)) => *f != 0.0,
        Some(Value::TimeSpan(t)) => *t != 0,
        Some(Value::Seq(v)) => !v.is_empty(),
        Some(_) => true,
    })
}
