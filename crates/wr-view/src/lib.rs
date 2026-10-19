//! The unit views of an area: what the hydrated sim says each visible unit is, where it
//! stands and what it wears, in the vocabulary of the assets (asset GUIDs of unit
//! prefabs and equipment entities). `wr-visual` writes the spec from a save; `wr-import
//! pack --units` reads it to bake the units into an area pack. The sim's own state never
//! reaches the importer or the renderer except through this file.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Schema tag of the spec file.
pub const SCHEMA: &str = "wr-view/units/1";

/// `Kingmaker.Blueprints.Gender` as the character system takes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Gender {
    Male,
    Female,
}

/// One equipment entity a unit wears on top of what its prefab carries, as
/// `Character.AddEquipmentEntity` receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EquipmentView {
    /// The `EquipmentEntity` asset's GUID (an `EquipmentEntityLink.AssetId`).
    pub ee: String,
    /// Where it came from, for reports: `item:<blueprint name>`, `class:<name>`, `doll`, `skin`.
    pub source: String,
    /// Primary and secondary colour ramp indices (-1: none given).
    pub primary: i32,
    pub secondary: i32,
    /// `ItemEntity.ForcedRampColorPresetIndex` for item equipment (the EE's `ColorPresets`
    /// entry that gives the ramps when none were given), else -1.
    pub preset: i32,
}

/// A unit that has a view in the area.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitView {
    /// `UniqueId`.
    pub id: String,
    pub blueprint: String,
    pub name: String,
    /// `BlueprintUnit.Prefab.AssetId`: the unit prefab (a `.unit` bundle).
    pub prefab: String,
    /// `Position` (world, metres) and `Orientation` (degrees about +Y, Unity's convention).
    pub position: [f32; 3],
    pub orientation: f32,
    pub gender: Gender,
    /// `Kingmaker.Blueprints.Race` of the unit's race (`BlueprintRace.RaceId`).
    pub race: i32,
    /// The prefab's own equipment entities are dropped first (`Character.RemoveAllEquipmentEntities`:
    /// a unit made in the character generator is built on the bare doll prefab).
    pub replace_base_equipment: bool,
    /// Equipment entities added after the prefab's own, in the order the game adds them
    /// (the doll's skin, clothes and chosen features; the body items' entities; the class
    /// clothes).
    pub equipment: Vec<EquipmentView>,
}

/// The spec file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitsSpec {
    pub schema: String,
    /// Save the units come from.
    pub save: String,
    /// The area's blueprint GUID.
    pub area: String,
    pub units: Vec<UnitView>,
}

impl UnitsSpec {
    /// Reads and checks a spec file.
    ///
    /// # Errors
    ///
    /// I/O, malformed JSON or another schema.
    pub fn read(path: &Path) -> Result<Self> {
        let s: Self = serde_json::from_slice(
            &std::fs::read(path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("parse {}", path.display()))?;
        anyhow::ensure!(
            s.schema == SCHEMA,
            "{}: schema {} (want {SCHEMA})",
            path.display(),
            s.schema
        );
        Ok(s)
    }

    /// Writes the spec as pretty JSON.
    ///
    /// # Errors
    ///
    /// I/O.
    pub fn write(&self, path: &Path) -> Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("write {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_round_trips_and_rejects_other_schemas() {
        let spec = UnitsSpec {
            schema: SCHEMA.to_owned(),
            save: "Manual_13".to_owned(),
            area: "bad465e5404604346bdf71ef50b3299a".to_owned(),
            units: vec![UnitView {
                id: "3708".to_owned(),
                blueprint: "54be53f0b35bf3c4592a97ae335fe765".to_owned(),
                name: "Seelah_Companion".to_owned(),
                prefab: "4ab4d902d50a301479fe999d23058afe".to_owned(),
                position: [110.5, 39.24, 132.76],
                orientation: 126.38,
                gender: Gender::Female,
                race: 19,
                replace_base_equipment: false,
                equipment: vec![EquipmentView {
                    ee: "e".to_owned(),
                    source: "item:ScalemailStandard".to_owned(),
                    primary: -1,
                    secondary: -1,
                    preset: 0,
                }],
            }],
        };
        let dir = std::env::temp_dir().join(format!("wr-view-test-{}", std::process::id()));
        let path = dir.join("spec.json");
        spec.write(&path).unwrap();
        assert_eq!(UnitsSpec::read(&path).unwrap(), spec);
        let mut other = spec.clone();
        other.schema = "wr-view/units/0".to_owned();
        other.write(&path).unwrap();
        assert!(UnitsSpec::read(&path).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
