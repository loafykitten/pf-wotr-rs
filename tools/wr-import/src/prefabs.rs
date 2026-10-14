//! Unit prefabs' `UnitEntityView.m_IsInGame` (`EntityViewBase.IsInGameBySettings`), the value a
//! unit built from the prefab's view starts with (`new UnitEntityData(view)` →
//! `EntityDataBase(view.UniqueId, view.IsInGameBySettings)`).
//!
//! `ResourcesLibrary` loads a unit prefab by its asset GUID from the bundle `locationlist.json`
//! names for it (`<guid>.unit`). Each bundle is opened on its own, read and dropped (no
//! [`crate::area::Universe`]: it keeps every bundle it opens). The view is recognised by its
//! own serialized members (`m_IsInGame`, `UniqueId` and `UnitEntityView`'s `m_Corpulence`,
//! a field no other class declares), so no script needs resolving. A bundle with no view, or
//! with views that disagree, gives no value: the sim then reports the spawn.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use rayon::prelude::*;
use serde::Serialize;

use crate::area::is_stream;
use crate::catalog::Locations;
use crate::unity::bundle::Bundle;
use crate::unity::serialized::{SerializedFile, class_id};
use crate::unity::value::Value;

/// Schema tag of `cache/sim/unit-prefabs.json`.
pub const SCHEMA: &str = "wr-sim-unit-prefabs-v1";

/// The table: asset GUID → `m_IsInGame` (`None`: no single value).
#[derive(Debug, Serialize)]
pub struct UnitPrefabs {
    pub schema: &'static str,
    pub source: String,
    pub prefabs: BTreeMap<String, Option<bool>>,
    /// Bundles that couldn't be read, with the error.
    pub errors: Vec<String>,
}

/// The `m_IsInGame` values of the `UnitEntityView`s in one bundle.
fn views_of(bundles_dir: &Path, bundle: &str) -> Result<Vec<bool>> {
    let b = Bundle::open(&bundles_dir.join(bundle))?;
    let mut out = Vec::new();
    for node in &b.header.nodes {
        if is_stream(node) {
            continue;
        }
        let data = b.node_data(node)?;
        let f = SerializedFile::parse(&node.path, data)?;
        for o in f.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
            let v = f.read(o)?;
            let is_view = v.get("UniqueId").is_some()
                && v.get("m_Corpulence").is_some()
                && v.get("m_SoftColliderRadius").is_some();
            if !is_view {
                continue;
            }
            match v.get("m_IsInGame").and_then(Value::as_bool) {
                Some(x) => out.push(x),
                None => anyhow::bail!("UnitEntityView without m_IsInGame"),
            }
        }
    }
    Ok(out)
}

/// Every `.unit` resource of the location list (a bundle that can't be read is recorded in
/// `errors`, its value unknown).
pub fn unit_prefabs(bundles_dir: &Path, locations: &Locations) -> UnitPrefabs {
    let mut units: Vec<(&String, &String)> =
        locations.guid_to_bundle.iter().filter(|(_, b)| b.ends_with(".unit")).collect();
    units.sort();
    let results: Vec<(String, Result<Vec<bool>>)> =
        units.par_iter().map(|(g, b)| ((*g).clone(), views_of(bundles_dir, b))).collect();
    let mut prefabs = BTreeMap::new();
    let mut errors = Vec::new();
    for (g, r) in results {
        match r {
            Ok(v) => {
                let value = v.first().copied().filter(|x| v.iter().all(|y| y == x));
                prefabs.insert(g, value);
            }
            Err(e) => {
                errors.push(format!("{g}: {e:#}"));
                prefabs.insert(g, None);
            }
        }
    }
    UnitPrefabs {
        schema: SCHEMA,
        source: format!("wr-import unit-prefabs {}", bundles_dir.display()),
        prefabs,
        errors,
    }
}
