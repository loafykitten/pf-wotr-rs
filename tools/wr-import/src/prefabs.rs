//! Unit prefabs' `UnitEntityView` settings the sim reads: `m_IsInGame`
//! (`EntityViewBase.IsInGameBySettings`, the value a unit built from the prefab's view starts with:
//! `new UnitEntityData(view)` → `EntityDataBase(view.UniqueId, view.IsInGameBySettings)`) and what
//! `UnitEntityView.Corpulence` is made of: `m_Corpulence` and the `OverrideSizeScale` component
//! on the view's game object (`ResultReduceSizeScaleStep`, `GetFinalScale`). `DisableSizeScaling`
//! and `m_RacialPetScaleCoeff` are not serialized (a runtime flag, and a field `Start` sets).
//!
//! `ResourcesLibrary` loads a unit prefab by its asset GUID from the bundle `locationlist.json`
//! names for it (`<guid>.unit`). Each bundle is opened on its own, read and dropped (no
//! [`crate::area::Universe`]: it keeps every bundle it opens). The view is recognised by its
//! own serialized members (`m_IsInGame`, `UniqueId` and `UnitEntityView`'s `m_Corpulence`,
//! a field no other class declares), so no script needs resolving; `OverrideSizeScale` by its
//! `m_ScaleStepCoefficient` and `m_MaxScale`. A bundle with no view, or with views that
//! disagree, gives no value for the field: the sim then reports the unit.

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

/// Schema tag of `cache/sim/unit-prefabs.json` (v2: each prefab an object with its view's
/// corpulence and size-scale override; v1 held `m_IsInGame` alone).
pub const SCHEMA: &str = "wr-sim-unit-prefabs-v2";

/// One prefab's settings; a field is `None` when the bundle has no view, the views disagree, or
/// the field is missing.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UnitPrefab {
    /// `UnitEntityView`s in the bundle.
    pub views: usize,
    /// `m_IsInGame`.
    pub in_game: Option<bool>,
    /// `m_Corpulence`.
    pub corpulence: Option<f32>,
    /// Whether the view's game object has an `OverrideSizeScale` (`m_OverrideSizeScale`).
    pub override_present: Option<bool>,
    /// `OverrideSizeScale.m_ScaleStepCoefficient` (`ResultReduceSizeScaleStep`; the game's
    /// default 0.66 is the sim's when there is no component).
    pub scale_step: Option<f32>,
    /// `OverrideSizeScale.m_MaxScale`.
    pub max_scale: Option<f32>,
}

/// The table: asset GUID → the prefab's settings.
#[derive(Debug, Serialize)]
pub struct UnitPrefabs {
    pub schema: &'static str,
    pub source: String,
    pub prefabs: BTreeMap<String, UnitPrefab>,
    /// Bundles that couldn't be read, with the error.
    pub errors: Vec<String>,
}

/// One `UnitEntityView` as found, with the `OverrideSizeScale`s on its game object.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewFound {
    pub in_game: Option<bool>,
    pub corpulence: Option<f32>,
    /// `(m_ScaleStepCoefficient, m_MaxScale)` of each `OverrideSizeScale` on the game object.
    pub overrides: Vec<(Option<f32>, Option<f32>)>,
}

/// The value all of `xs` agree on; `None` for no item, a missing value or a disagreement.
fn agreed<T: Copy + PartialEq>(xs: impl Iterator<Item = Option<T>>) -> Option<T> {
    let v: Vec<Option<T>> = xs.collect();
    let first = (*v.first()?)?;
    v.iter().all(|x| *x == Some(first)).then_some(first)
}

/// A bundle's views summarised into one prefab's settings.
#[must_use]
pub fn summarize(views: &[ViewFound]) -> UnitPrefab {
    // (Floats agree bitwise.)
    let float = |xs: Vec<Option<f32>>| {
        agreed(xs.into_iter().map(|x| x.map(f32::to_bits))).map(f32::from_bits)
    };
    // (`GetComponentNonAlloc` takes the first: a game object with two is left unknown.)
    let override_present = agreed(views.iter().map(|v| match v.overrides.len() {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }));
    let one = |v: &ViewFound| (v.overrides.len() == 1).then(|| v.overrides[0]);
    UnitPrefab {
        views: views.len(),
        in_game: agreed(views.iter().map(|v| v.in_game)),
        corpulence: float(views.iter().map(|v| v.corpulence).collect()),
        override_present,
        scale_step: float(views.iter().map(|v| one(v).and_then(|o| o.0)).collect()),
        max_scale: float(views.iter().map(|v| one(v).and_then(|o| o.1)).collect()),
    }
}

/// The `UnitEntityView`s of one serialized file, each with its game object's path id.
///
/// # Errors
///
/// An object that can't be read, or a view without `m_IsInGame`.
pub fn views_in(f: &SerializedFile) -> Result<Vec<(i64, ViewFound)>> {
    let mut monos = Vec::new();
    for o in f.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
        monos.push(f.read(o)?);
    }
    views_among(&monos)
}

/// The `UnitEntityView`s among a file's `MonoBehaviour`s, each with its game object's path id
/// and the `OverrideSizeScale`s on that same game object (`GetComponentNonAlloc` looks at the
/// view's own game object only).
///
/// # Errors
///
/// A view without `m_IsInGame`.
pub fn views_among(monos: &[Value]) -> Result<Vec<(i64, ViewFound)>> {
    let game_object =
        |v: &Value| v.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
    let overrides: Vec<(i64, &Value)> = monos
        .iter()
        .filter(|v| v.get("m_ScaleStepCoefficient").is_some() && v.get("m_MaxScale").is_some())
        .map(|v| (game_object(v), v))
        .collect();
    let mut out = Vec::new();
    for v in monos {
        let is_view = v.get("UniqueId").is_some()
            && v.get("m_Corpulence").is_some()
            && v.get("m_SoftColliderRadius").is_some();
        if !is_view {
            continue;
        }
        let Some(in_game) = v.get("m_IsInGame").and_then(Value::as_bool) else {
            anyhow::bail!("UnitEntityView without m_IsInGame");
        };
        let go = game_object(v);
        out.push((
            go,
            ViewFound {
                in_game: Some(in_game),
                corpulence: v.get("m_Corpulence").and_then(Value::as_f32),
                overrides: overrides
                    .iter()
                    .filter(|(g, _)| *g == go)
                    .map(|(_, o)| {
                        (
                            o.get("m_ScaleStepCoefficient").and_then(Value::as_f32),
                            o.get("m_MaxScale").and_then(Value::as_f32),
                        )
                    })
                    .collect(),
            },
        ));
    }
    Ok(out)
}

/// The `UnitEntityView`s of one bundle.
fn views_of(bundles_dir: &Path, bundle: &str) -> Result<Vec<ViewFound>> {
    let b = Bundle::open(&bundles_dir.join(bundle))?;
    let mut out = Vec::new();
    for node in &b.header.nodes {
        if is_stream(node) {
            continue;
        }
        let data = b.node_data(node)?;
        let f = SerializedFile::parse(&node.path, data)?;
        out.extend(views_in(&f)?.into_iter().map(|(_, v)| v));
    }
    Ok(out)
}

/// Every `.unit` resource of the location list (a bundle that can't be read is recorded in
/// `errors`, its values unknown).
pub fn unit_prefabs(bundles_dir: &Path, locations: &Locations) -> UnitPrefabs {
    let mut units: Vec<(&String, &String)> =
        locations.guid_to_bundle.iter().filter(|(_, b)| b.ends_with(".unit")).collect();
    units.sort();
    let results: Vec<(String, Result<Vec<ViewFound>>)> =
        units.par_iter().map(|(g, b)| ((*g).clone(), views_of(bundles_dir, b))).collect();
    let mut prefabs = BTreeMap::new();
    let mut errors = Vec::new();
    for (g, r) in results {
        match r {
            Ok(v) => {
                prefabs.insert(g, summarize(&v));
            }
            Err(e) => {
                errors.push(format!("{g}: {e:#}"));
                prefabs.insert(g, UnitPrefab::default());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn view(corp: f32, over: &[(f32, f32)]) -> ViewFound {
        ViewFound {
            in_game: Some(true),
            corpulence: Some(corp),
            overrides: over.iter().map(|(a, b)| (Some(*a), Some(*b))).collect(),
        }
    }

    fn pptr(path_id: i64) -> Value {
        Value::Object(vec![
            ("m_FileID".into(), Value::Int(0)),
            ("m_PathID".into(), Value::Int(path_id)),
        ])
    }

    /// A `UnitEntityView` `MonoBehaviour` on game object `go`.
    fn view_mono(go: i64, corp: f32) -> Value {
        Value::Object(vec![
            ("m_GameObject".into(), pptr(go)),
            ("UniqueId".into(), Value::Str(String::new())),
            ("m_Corpulence".into(), Value::Float(corp)),
            ("m_SoftColliderRadius".into(), Value::Float(0.3)),
            ("m_IsInGame".into(), Value::Bool(true)),
        ])
    }

    /// An `OverrideSizeScale` `MonoBehaviour` on game object `go`.
    fn override_mono(go: i64, step: f32, max: f32) -> Value {
        Value::Object(vec![
            ("m_GameObject".into(), pptr(go)),
            ("m_ScaleStepCoefficient".into(), Value::Float(step)),
            ("m_MaxScale".into(), Value::Float(max)),
        ])
    }

    #[test]
    fn an_override_belongs_to_the_view_on_its_own_game_object() {
        let monos = [
            view_mono(1, 0.9),
            override_mono(2, 0.1, 2.0),
            view_mono(2, 0.5),
            override_mono(2, 0.5, 4.0),
            override_mono(3, 0.3, 3.0),
        ];
        let found = views_among(&monos).unwrap();
        assert_eq!(found.len(), 2);
        let (go, first) = &found[0];
        assert_eq!((*go, first.corpulence), (1, Some(0.9)));
        assert!(first.overrides.is_empty(), "another object's override isn't the view's");
        let (go, second) = &found[1];
        assert_eq!((*go, second.corpulence), (2, Some(0.5)));
        assert_eq!(second.overrides, vec![(Some(0.1), Some(2.0)), (Some(0.5), Some(4.0))]);
        // Two on one game object: the summary leaves the override unknown.
        assert_eq!(summarize(std::slice::from_ref(second)).override_present, None);
        assert_eq!(summarize(std::slice::from_ref(first)).override_present, Some(false));
    }

    #[test]
    fn an_override_alone_or_a_view_without_in_game_is_not_a_view() {
        assert!(views_among(&[override_mono(1, 0.5, 4.0)]).unwrap().is_empty());
        let mut v = view_mono(1, 0.5);
        if let Value::Object(f) = &mut v {
            f.retain(|(k, _)| k != "m_IsInGame");
        }
        assert!(views_among(&[v]).is_err());
    }

    #[test]
    fn a_single_view_gives_its_settings() {
        let p = summarize(&[view(0.75, &[])]);
        assert_eq!(p.views, 1);
        assert_eq!(p.in_game, Some(true));
        assert_eq!(p.corpulence, Some(0.75));
        assert_eq!(p.override_present, Some(false));
        assert_eq!(p.scale_step, None);
    }

    #[test]
    fn an_override_on_the_views_game_object_gives_its_step_and_cap() {
        let p = summarize(&[view(2.0, &[(0.5, 4.0)])]);
        assert_eq!(
            (p.override_present, p.scale_step, p.max_scale),
            (Some(true), Some(0.5), Some(4.0))
        );
    }

    #[test]
    fn views_that_disagree_or_two_overrides_give_no_value() {
        let p = summarize(&[view(0.5, &[]), view(0.75, &[])]);
        assert_eq!((p.views, p.corpulence, p.override_present), (2, None, Some(false)));
        let p = summarize(&[view(0.5, &[(0.5, 4.0), (0.6, 4.0)])]);
        assert_eq!((p.override_present, p.scale_step), (None, None));
        let p = summarize(&[]);
        assert_eq!((p.views, p.in_game, p.corpulence, p.override_present), (0, None, None, None));
    }
}
