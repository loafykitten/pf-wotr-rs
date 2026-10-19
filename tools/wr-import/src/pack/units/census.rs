//! `wr-import ee-census`: how the equipment entities' renderer prefabs sit with regard to
//! `GameObject.activeSelf`, for deciding what `BodyPart.SkinnedRenderer`
//! (`GetComponentInChildren<SkinnedMeshRenderer>()`) finds. Reads every `.ee` bundle of the location list.

use anyhow::Result;
use serde::Serialize;

use crate::area::Universe;
use crate::catalog::Locations;
use crate::unity::serialized::{SerializedFile, class_id};
use crate::unity::value::Value;

/// One body part whose renderer prefab's root is inactive.
#[derive(Debug, Serialize)]
pub struct InactiveRoot {
    pub guid: String,
    pub ee: String,
    pub part: usize,
    pub body_part_type: i64,
    /// A `SkinnedMeshRenderer` sits on the inactive root itself.
    pub renderer_on_root: bool,
}

#[derive(Debug, Default, Serialize)]
pub struct Census {
    pub bundles: usize,
    pub equipment_entities: usize,
    pub body_parts: usize,
    pub with_renderer_prefab: usize,
    /// Parts whose renderer prefab root is active and carries the `SkinnedMeshRenderer` itself (the
    /// detection's control: it must be most of them).
    pub active_root_with_renderer: usize,
    /// Parts where a renderer is found with `includeInactive` and not without.
    pub hidden_by_inactive_descendant: usize,
    /// Parts where the lookup (the root always searched) finds a renderer.
    pub found: usize,
    /// Parts whose renderer prefab root has `m_IsActive` false.
    pub inactive_roots: Vec<InactiveRoot>,
    pub unreadable: Vec<String>,
}

fn has_smr(f: &SerializedFile<'_>, go: &Value) -> bool {
    go.get("m_Component")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|c| c.get("component").and_then(Value::as_pptr))
        .filter(|p| p.file_id == 0)
        .any(|p| f.object(p.path_id).is_some_and(|o| o.class_id == class_id::SKINNED_MESH_RENDERER))
}

/// `GetComponentInChildren<SkinnedMeshRenderer>`: the GameObject itself first whatever its state, then
/// descendants depth first, an inactive descendant hiding its subtree unless `include_inactive`.
fn find(f: &SerializedFile<'_>, go: &Value, include_inactive: bool, root: bool) -> bool {
    if (root || include_inactive || go.get("m_IsActive").and_then(Value::as_bool).unwrap_or(true))
        && has_smr(f, go)
    {
        return true;
    }
    if !root && !include_inactive && !go.get("m_IsActive").and_then(Value::as_bool).unwrap_or(true)
    {
        return false;
    }
    let tr = go
        .get("m_Component")
        .and_then(Value::as_array)
        .unwrap_or_default()
        .iter()
        .filter_map(|c| c.get("component").and_then(Value::as_pptr))
        .filter(|p| p.file_id == 0)
        .find_map(|p| f.object(p.path_id).filter(|o| o.class_id == class_id::TRANSFORM));
    let Some(tr) = tr.and_then(|o| f.read(o).ok()) else { return false };
    tr.get("m_Children").and_then(Value::as_array).unwrap_or_default().iter().any(|c| {
        c.as_pptr()
            .filter(|p| p.file_id == 0)
            .and_then(|p| f.object(p.path_id))
            .and_then(|o| f.read(o).ok())
            .and_then(|t| t.get("m_GameObject").and_then(Value::as_pptr))
            .and_then(|p| f.object(p.path_id))
            .and_then(|o| f.read(o).ok())
            .is_some_and(|child| find(f, &child, include_inactive, false))
    })
}

/// Scans the `.ee` bundles (`only`: just these GUIDs).
pub fn run(universe: &Universe, locations: &Locations, only: Option<&[String]>) -> Result<Census> {
    let mut c = Census::default();
    let mut guids: Vec<&String> = locations
        .guid_to_bundle
        .iter()
        .filter(|(g, b)| b.ends_with(".ee") && only.is_none_or(|o| o.contains(g)))
        .map(|(g, _)| g)
        .collect();
    guids.sort();
    for g in guids {
        let bundle = &locations.guid_to_bundle[g];
        c.bundles += 1;
        let files = match universe.files_of(bundle) {
            Ok(f) => f,
            Err(e) => {
                c.unreadable.push(format!("{g}: {e:#}"));
                continue;
            }
        };
        for f in files {
            for o in f.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
                // A behaviour that can't be decoded may be an equipment entity: the census isn't
                // exhaustive without it.
                let v = match f.read(o) {
                    Ok(v) => v,
                    Err(e) => {
                        c.unreadable.push(format!("{g}: object {}: {e:#}", o.path_id));
                        continue;
                    }
                };
                if !universe.script_of(f, &v)?.is_some_and(|s| s.class == "EquipmentEntity") {
                    continue;
                }
                c.equipment_entities += 1;
                for (i, p) in v
                    .get("BodyParts")
                    .and_then(Value::as_array)
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                {
                    c.body_parts += 1;
                    let Some(ptr) =
                        p.get("RendererPrefab").and_then(Value::as_pptr).filter(|p| !p.is_null())
                    else {
                        continue;
                    };
                    c.with_renderer_prefab += 1;
                    let Some(go) = universe.resolve(f, ptr)? else {
                        c.unreadable.push(format!("{g}: part {i}: renderer prefab not found"));
                        continue;
                    };
                    let gv = go.read()?;
                    let (with, without) =
                        (find(go.file, &gv, true, true), find(go.file, &gv, false, true));
                    c.found += usize::from(without);
                    c.hidden_by_inactive_descendant += usize::from(with && !without);
                    if gv.get("m_IsActive").and_then(Value::as_bool).unwrap_or(true) {
                        c.active_root_with_renderer += usize::from(has_smr(go.file, &gv));
                        continue;
                    }
                    c.inactive_roots.push(InactiveRoot {
                        guid: g.clone(),
                        ee: v.str_at("m_Name").unwrap_or_default().to_owned(),
                        part: i,
                        body_part_type: p.i64_at("m_Type").unwrap_or(0),
                        renderer_on_root: has_smr(go.file, &gv),
                    });
                }
            }
        }
    }
    Ok(c)
}
