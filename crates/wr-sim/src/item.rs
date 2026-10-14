//! `ItemEntity` and its subclasses, as far as the character sheet and load need.

use wr_content::Guid;

use crate::world::{EntityIdx, ModIdx};

#[derive(Debug, Clone, Default)]
pub struct Item {
    pub blueprint: Option<Guid>,
    pub count: i32,
    pub charges: i32,
    pub is_identified: bool,
    /// `Wielder` (the unit whose slot holds it), as a unit.
    pub wielder: Option<EntityIdx>,
    /// `Collection.OwnerUnit`.
    pub collection_owner: Option<EntityIdx>,
    /// `Collection`: identity of the `ItemsCollection` object (compared by reference in
    /// the C#, e.g. `item.Collection != owner.Inventory`).
    pub collection: Option<usize>,
    /// `HoldingSlot`: the slot's owner and the slot's `UnitBody` member name.
    pub holding_slot: Option<(EntityIdx, String)>,
    /// `m_Modifiers` of armor and shields: the modifiers equipping it added.
    pub modifiers: Vec<ModIdx>,
    /// `Ability` / `ActivatableAbility`: the facts equipping it gave the wielder.
    pub ability: Option<crate::world::FactIdx>,
    pub activatable_ability: Option<crate::world::FactIdx>,
    /// `m_FactsAppliedToWielder` is saved non-empty (the facts themselves aren't tracked).
    pub facts_applied_to_wielder: bool,
    /// `ItemEntityShield.ArmorComponent` / `WeaponComponent` (entities of their own).
    pub armor_component: Option<EntityIdx>,
    pub weapon_component: Option<EntityIdx>,
    /// For a shield's component: `Shield`.
    pub shield: Option<EntityIdx>,
    /// `ItemEntityArmor.m_DexBonusLimeterAC`: identity of the limiter it put on the AC.
    pub dex_limiter: Option<u32>,
    /// `ItemEntityWeapon.ForceSecondary` (saved).
    pub force_secondary: bool,
    /// `IsNonRemovable` (saved).
    pub is_non_removable: bool,
    /// `ForceStackable` (saved).
    pub force_stackable: Option<bool>,
    /// Created during the run (`new ItemEntity`: a fresh `Guid.NewGuid()` id), not loaded.
    pub runtime_created: bool,
    /// `VisualSourceItemBlueprint` (saved): `HasOverridenVisual` when set.
    pub visual_source: Option<Guid>,
    /// `RemoveFromSlotInProgress`: the item is leaving its slot (`ItemSlot.MaybeItem` hides it).
    pub remove_from_slot_in_progress: bool,
    /// `m_NotLootable` (saved; `MakeNotLootable`).
    pub not_lootable: bool,
    /// `m_SkinningSuccessful` (saved).
    pub skinning_successful: bool,
    /// `m_InventorySlotIndex` (saved). `None`: not established. The sim doesn't port the
    /// `UpdateSlotIndex` calls of `ItemsCollection.Add`/`Insert`/`Transfer`, so an item that
    /// entered or left a collection at run time has no known index.
    pub inventory_slot_index: Option<i32>,
}
