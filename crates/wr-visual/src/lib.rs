//! `wr-visual`: the unit views of a hydrated save, as a [`wr_view::UnitsSpec`].
//!
//! POC-7 stage 1. The hydrated sim says which units stand in the area, where, and what
//! they carry; this crate turns that into the vocabulary of the assets (unit prefab,
//! equipment entities and their colour ramps) so the importer can bake the units into an
//! area pack. It reads the sim, never writes it.
//!
//! What the game decides when it builds a unit's view (`UnitEntityData` -> `UnitEntityView`):
//!
//! - the view is the blueprint's prefab (`BlueprintUnit.Prefab`); a unit made in the
//!   character generator (`UnitPartDollData`) is built from the doll prefab instead,
//!   emptied of its equipment and dressed from `DollData` (`DollData.CreateUnitView`);
//! - body items add their equipment entities (`UnitEntityView.UpdateBodyEquipmentModel`);
//! - a unit with `UseClassEquipment` (the main character, custom companions) then gets its
//!   equipment class' clothes (`UpdateClassEquipment`).
//!
//! Left out, and listed in `docs/poc7/characters.md`: per-item custom ramps
//! (`CustomPrimaryRampIndex`), slots that hide their entity (`HideEquipmentEntity`), the
//! UI visibility switches (helmet, cloak, backpack: all on by default), polymorphs.

pub mod spec;
