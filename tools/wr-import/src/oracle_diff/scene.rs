//! Scene and entity diffs: `scene-dump.json` (entity views with components and fields,
//! spawners, unit groups, script zones with shapes and probes, enter points, area
//! transitions) against our inventory.
//!
//! Expected runtime differences (listed, not errors): views of units spawned at runtime
//! (scene `BaseMechanics` or ids the scenes don't contain), components the game adds at
//! runtime, and fields of the live objects that serialization never sees.

use std::collections::{BTreeSet, HashMap};

use serde_json::{Value as J, json};

use super::json::{self as dj, cmp_f, cmp_fs, eq};
use super::{Ours, Outcome, Section};
use crate::scene::{GameObjectRecord, ShapeRecord};
use crate::unity::value::Value;

fn short_scene(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s).trim_end_matches(".unity")
}

fn norm(c: &str) -> &str {
    c.strip_prefix("UnityEngine.").unwrap_or(c)
}

/// Compares a dumped serialized value with ours, recursively. Blueprint references
/// (`{"$bp": g}` vs `{"guid": g}`) compare by GUID; object references (`$ref` vs PPtr)
/// are counted as references and not compared (their encodings differ); numbers compare
/// as `f32` bits, booleans as 0/1.
fn cmp_value(path: &str, live: &J, ours: &Value, sec: &mut Section, ctx: &J) {
    match (live, ours) {
        (J::Object(m), _) if m.contains_key("$bp") => {
            let g = ours.str_at("guid").unwrap_or_default();
            let o = eq(&g, &m["$bp"].as_str().unwrap_or_default());
            sec.record(
                "fields.blueprint_refs",
                o,
                || json!({"at": ctx, "field": path, "ours": g, "live": live}),
            );
        }
        (J::Object(m), _) if m.contains_key("$ref") || m.contains_key("$null") => {
            sec.tally("fields.object_refs_not_compared", Outcome::Expected);
        }
        (J::Object(m), Value::Object(_)) => {
            for (k, lv) in m {
                match ours.get(k) {
                    Some(ov) => cmp_value(&format!("{path}.{k}"), lv, ov, sec, ctx),
                    None => sec.record(
                        "fields.only_in_live",
                        Outcome::Expected,
                        || json!({"at": ctx, "field": format!("{path}.{k}")}),
                    ),
                }
            }
        }
        (J::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                sec.record(
                    "fields.values",
                    Outcome::Mismatch,
                    || json!({"at": ctx, "field": path, "live_len": a.len(), "ours_len": b.len()}),
                );
                return;
            }
            for (i, (lv, ov)) in a.iter().zip(b).enumerate() {
                cmp_value(&format!("{path}[{i}]"), lv, ov, sec, ctx);
            }
        }
        // Vectors: [x, y, z(, w)] live, {x, y, z(, w)} ours; colours r g b a.
        (J::Array(a), Value::Object(_)) => {
            let keys: &[&str] =
                if ours.get("r").is_some() { &["r", "g", "b", "a"] } else { &["x", "y", "z", "w"] };
            let o: Vec<f32> = keys.iter().filter_map(|k| ours.f32_at(k)).collect();
            let l: Vec<f32> = a.iter().filter_map(dj::f).collect();
            sec.record(
                "fields.values",
                cmp_fs(&o, &l),
                || json!({"at": ctx, "field": path, "ours": o, "live": l}),
            );
        }
        (J::Bool(b), _) => {
            let o = ours.as_bool();
            sec.record(
                "fields.values",
                eq(&o, &Some(*b)),
                || json!({"at": ctx, "field": path, "ours": o, "live": b}),
            );
        }
        (J::Number(_), _) => {
            let l = dj::f(live).unwrap_or(f32::NAN);
            let o = match ours {
                Value::Float(_) | Value::Double(_) => cmp_f(ours.as_f32().unwrap_or(f32::NAN), l),
                _ => match (ours.as_i64(), live.as_i64()) {
                    (Some(a), Some(b)) => eq(&a, &b),
                    _ => cmp_f(ours.as_f32().unwrap_or(f32::NAN), l),
                },
            };
            sec.record(
                "fields.values",
                o,
                || json!({"at": ctx, "field": path, "ours": ours, "live": live}),
            );
        }
        // Enums: the dump writes names, the scene file stores values.
        (J::String(_), Value::Int(_) | Value::UInt(_)) => {
            sec.tally("fields.enum_name_vs_value", Outcome::Expected)
        }
        (J::String(s), _) => {
            let o = ours.as_str().map(str::to_owned);
            sec.record(
                "fields.values",
                eq(&o.as_deref(), &Some(s.as_str())),
                || json!({"at": ctx, "field": path, "ours": o, "live": s}),
            );
        }
        (J::Null, _) => sec.tally("fields.null_live", Outcome::Expected),
        _ => sec.record(
            "fields.values",
            Outcome::Mismatch,
            || json!({"at": ctx, "field": path, "ours": ours, "live": live}),
        ),
    }
}

fn cmp_object(view: &J, rec: &GameObjectRecord, sec: &mut Section, ctx: &J) {
    let t = &view["transform"];
    let rec_local =
        [rec.local.position.as_slice(), rec.local.rotation.as_slice(), rec.local.scale.as_slice()];
    for (k, ours) in ["local_position", "local_rotation", "local_scale"].iter().zip(rec_local) {
        let l: Vec<f32> = dj::arr(t, k).iter().filter_map(dj::f).collect();
        sec.record(
            &format!("views.{k}"),
            cmp_fs(ours, &l),
            || json!({"at": ctx, "ours": ours, "live": l}),
        );
    }
    let wm = dj::matrix_cols(&t["local_to_world"]).unwrap_or([f32::NAN; 16]);
    sec.record(
        "views.local_to_world",
        cmp_fs(&rec.world.matrix, &wm),
        || json!({"at": ctx, "ours": rec.world.matrix, "live": wm}),
    );
    let ls: Vec<f32> = dj::arr(t, "lossy_scale").iter().filter_map(dj::f).collect();
    sec.record(
        "views.lossy_scale",
        cmp_fs(&rec.world.lossy_scale, &ls),
        || json!({"at": ctx, "ours": rec.world.lossy_scale, "live": ls}),
    );
    let sib: Vec<u32> =
        dj::arr(view, "sibling_path").iter().filter_map(|x| x.as_u64().map(|u| u as u32)).collect();
    sec.record(
        "views.sibling_path",
        eq(&rec.sibling_path, &sib),
        || json!({"at": ctx, "ours": rec.sibling_path, "live": sib}),
    );
    sec.record(
        "views.layer",
        eq(&rec.layer, &view["layer"].as_i64().unwrap_or(-1)),
        || json!({"at": ctx, "ours": rec.layer, "live": view["layer"]}),
    );
    let a_self = view["active_self"].as_bool().unwrap_or(false);
    sec.record(
        "views.active_self",
        eq(&rec.active_self, &a_self),
        || json!({"at": ctx, "ours": rec.active_self, "live": a_self}),
    );
    // Components: types in order; enabled flags and fields for the ones we decoded.
    let lc: Vec<&J> = dj::arr(view, "components").iter().collect();
    let lt: Vec<&str> = lc.iter().map(|c| norm(dj::s(c, "type").unwrap_or_default())).collect();
    let ot: Vec<&str> = rec.components.iter().map(|c| norm(&c.class)).collect();
    // Expected only if ours is the live list with runtime additions: our components in
    // the same order, as a subsequence.
    let mut it = lt.iter();
    let subsequence = ot.iter().all(|c| it.any(|l| l == c));
    let o = if lt == ot {
        Outcome::Exact
    } else if subsequence {
        Outcome::Expected
    } else {
        Outcome::Mismatch
    };
    sec.record("views.component_types", o, || json!({"at": ctx, "ours": ot, "live": lt}));
    // Join components by (type, occurrence): the n-th live component of a type with our
    // n-th of that type, each used once.
    let mut seen_types: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for c in &lc {
        let ty = norm(dj::s(c, "type").unwrap_or_default());
        let n = seen_types.entry(ty).or_default();
        let occurrence = *n;
        *n += 1;
        let Some(oc) = rec.components.iter().filter(|x| norm(&x.class) == ty).nth(occurrence)
        else {
            sec.record(
                "views.component_only_in_live",
                Outcome::Expected,
                || json!({"at": ctx, "component": ty}),
            );
            continue;
        };
        if let (Some(le), Some(oe)) = (c.get("enabled").and_then(J::as_bool), oc.enabled) {
            sec.record(
                "views.component_enabled",
                eq(&oe, &le),
                || json!({"at": ctx, "component": ty, "ours": oe, "live": le}),
            );
        }
        if let Some(f) = c.get("fields") {
            let cctx = json!({"view": ctx, "component": ty});
            cmp_value("", f, &oc.fields, sec, &cctx);
        }
    }
}

fn cmp_shape(ls: &J, os: &ShapeRecord, sec: &mut Section, ctx: &J) {
    let lt = dj::s(ls, "type").unwrap_or_default();
    sec.record(
        "zones.shape_type",
        eq(&lt.rsplit('.').next().unwrap_or_default(), &os.kind.as_str()),
        || json!({"at": ctx, "ours": os.kind, "live": lt}),
    );
    let m = dj::matrix_cols(&ls["local_to_world"]).unwrap_or([f32::NAN; 16]);
    sec.record(
        "zones.shape_local_to_world",
        cmp_fs(&os.matrix, &m),
        || json!({"at": ctx, "ours": os.matrix, "live": m}),
    );
    if let Some(c) = os.world_center {
        let l: [f32; 3] = dj::fa(&ls["world_center"]).unwrap_or([f32::NAN; 3]);
        sec.record(
            "zones.shape_world_center",
            cmp_fs(&c, &l),
            || json!({"at": ctx, "ours": c, "live": l}),
        );
    }
    if let Some((c, s)) = os.world_bounds {
        let lc: [f32; 3] = dj::fa(&ls["world_bounds"]["center"]).unwrap_or([f32::NAN; 3]);
        let lsz: [f32; 3] = dj::fa(&ls["world_bounds"]["size"]).unwrap_or([f32::NAN; 3]);
        let o = if cmp_fs(&c, &lc) == Outcome::Mismatch || cmp_fs(&s, &lsz) == Outcome::Mismatch {
            Outcome::Mismatch
        } else if cmp_fs(&c, &lc) == Outcome::Exact && cmp_fs(&s, &lsz) == Outcome::Exact {
            Outcome::Exact
        } else {
            Outcome::Close
        };
        sec.record(
            "zones.shape_world_bounds",
            o,
            || json!({"at": ctx, "ours": [c, s], "live": [lc, lsz]}),
        );
    }
    if let Some(local) = ls.get("local") {
        cmp_value("local", local, &os.local, sec, ctx);
    }
    // Probes: the game's `Contains` at each captured point and corpulence vs our
    // evaluation of that same point (not of our own regenerated probe list).
    let m = glam::Mat4::from_cols_array(&os.matrix);
    let probes = dj::arr(ls, "inventory_probes");
    if probes.is_empty() {
        sec.record("zones.coverage.no_probes", Outcome::Mismatch, || ctx.clone());
    }
    for p in probes {
        let a = p.as_array().map_or(&[][..], Vec::as_slice);
        let (Some(x), Some(y), Some(z), Some(c)) = (
            a.first().and_then(dj::f),
            a.get(1).and_then(dj::f),
            a.get(2).and_then(dj::f),
            a.get(3).and_then(dj::f),
        ) else {
            sec.record(
                "zones.coverage.malformed_probe",
                Outcome::Mismatch,
                || json!({"at": ctx, "probe": p}),
            );
            continue;
        };
        let game = a.get(4).and_then(J::as_bool);
        let ours = crate::scene::shape_contains(&os.kind, &os.local, m, [x, y, z], c);
        sec.record(
            "zones.probe_contains",
            eq(&ours, &game),
            || json!({"at": ctx, "probe": p, "ours": ours}),
        );
    }
}

pub fn diff(dump: &J, ours: &Ours, sec: &mut Section) {
    let inv = &ours.inventory;
    let objects: HashMap<(String, i64), &GameObjectRecord> =
        inv.objects.iter().map(|o| ((o.key.file.clone(), o.key.path_id), o)).collect();
    let scenes: BTreeSet<String> =
        inv.scenes.iter().map(|s| short_scene(&s.scene).to_owned()).collect();

    // Entity views.
    let mut seen = BTreeSet::new();
    for v in dj::arr(dump, "views") {
        let id = dj::s(v, "unique_id").unwrap_or_default().to_owned();
        let scene = dj::s(v, "scene").unwrap_or_default();
        let ctx = json!({"unique_id": id, "path": v["path"]});
        let Some(e) = inv.entities.iter().find(|e| e.unique_id == id) else {
            let why = if scenes.contains(scene) { Outcome::Mismatch } else { Outcome::Expected };
            sec.record(
                "views.presence.only_in_live",
                why,
                || json!({"at": ctx, "scene": scene, "class": v["class"]}),
            );
            continue;
        };
        seen.insert(id.clone());
        sec.tally("views.presence.both", Outcome::Exact);
        let class = dj::s(v, "class").unwrap_or_default();
        sec.record(
            "views.class",
            eq(&e.class.as_str(), &class),
            || json!({"at": ctx, "ours": e.class, "live": class}),
        );
        sec.record(
            "views.scene",
            eq(&short_scene(&e.scene), &scene),
            || json!({"at": ctx, "ours": e.scene, "live": scene}),
        );
        let path = dj::s(v, "path").unwrap_or_default();
        sec.record(
            "views.path",
            eq(&e.path.as_str(), &path),
            || json!({"at": ctx, "ours": e.path, "live": path}),
        );
        let ah = v["active_in_hierarchy"].as_bool().unwrap_or(false);
        sec.record(
            "views.active_in_hierarchy",
            eq(&e.active_in_hierarchy, &ah),
            || json!({"at": ctx, "ours": e.active_in_hierarchy, "live": ah}),
        );
        let p: [f32; 3] = dj::fa(&v["position"]).unwrap_or([f32::NAN; 3]);
        let r: [f32; 4] = dj::fa(&v["rotation"]).unwrap_or([f32::NAN; 4]);
        sec.record(
            "views.position",
            cmp_fs(&e.position, &p),
            || json!({"at": ctx, "ours": e.position, "live": p}),
        );
        sec.record(
            "views.rotation",
            cmp_fs(&e.rotation, &r),
            || json!({"at": ctx, "ours": e.rotation, "live": r}),
        );
        if let Some(b) = v.get("is_in_game_by_settings").and_then(J::as_bool) {
            sec.record(
                "views.is_in_game",
                eq(&e.is_in_game, &Some(b)),
                || json!({"at": ctx, "ours": e.is_in_game, "live": b}),
            );
        }
        if let Some(rec) = objects.get(&(e.game_object.file.clone(), e.game_object.path_id)) {
            cmp_object(v, rec, sec, &ctx);
        }
    }
    for e in &inv.entities {
        if !seen.contains(&e.unique_id) {
            sec.record(
                "views.presence.only_in_ours",
                Outcome::Mismatch,
                || json!({"unique_id": e.unique_id, "path": e.path}),
            );
        }
    }

    // Spawners.
    for s in dj::arr(dump, "spawners") {
        let id = dj::s(s, "unique_id").unwrap_or_default();
        let ctx = json!({"unique_id": id, "path": s["path"]});
        let Some(o) = inv.spawners.iter().find(|x| x.unique_id == id) else {
            sec.record("spawners.presence.only_in_live", Outcome::Mismatch, || ctx.clone());
            continue;
        };
        sec.tally("spawners.presence.both", Outcome::Exact);
        let bp = dj::s(s, "blueprint").unwrap_or_default();
        sec.record(
            "spawners.blueprint",
            eq(&o.blueprint.as_str(), &bp),
            || json!({"at": ctx, "ours": o.blueprint, "live": bp}),
        );
        let soi = s["spawn_on_scene_init"].as_bool();
        sec.record(
            "spawners.spawn_on_scene_init",
            eq(&o.spawn_on_scene_init, &soi),
            || json!({"at": ctx, "ours": o.spawn_on_scene_init, "live": soi}),
        );
        let p: [f32; 3] = dj::fa(&s["position"]).unwrap_or([f32::NAN; 3]);
        let r: [f32; 4] = dj::fa(&s["rotation"]).unwrap_or([f32::NAN; 4]);
        sec.record(
            "spawners.position",
            cmp_fs(&o.position, &p),
            || json!({"at": ctx, "ours": o.position, "live": p}),
        );
        sec.record(
            "spawners.rotation",
            cmp_fs(&o.rotation, &r),
            || json!({"at": ctx, "ours": o.rotation, "live": r}),
        );
        let mut lt: Vec<String> = dj::arr(s, "settings_components")
            .iter()
            .filter_map(|c| dj::s(c, "type").map(str::to_owned))
            .collect();
        let mut ot = o.settings.clone();
        lt.sort();
        ot.sort();
        sec.record(
            "spawners.settings_components",
            eq(&ot, &lt),
            || json!({"at": ctx, "ours": ot, "live": lt}),
        );
        // Runtime state, for the record (scene data can't predict it).
        sec.tally("spawners.runtime_state_not_compared", Outcome::Expected);
    }

    // Unit groups: members derived from the hierarchy (spawners below the group).
    for g in dj::arr(dump, "unit_groups") {
        let gpath = format!("{}/", dj::s(g, "path").unwrap_or_default());
        let mut live: Vec<String> =
            dj::arr(g, "spawners").iter().filter_map(|x| x.as_str().map(str::to_owned)).collect();
        let mut mine: Vec<String> = inv
            .spawners
            .iter()
            .filter(|s| {
                objects
                    .get(&(s.game_object.file.clone(), s.game_object.path_id))
                    .is_some_and(|o| o.path.starts_with(&gpath))
            })
            .map(|s| s.unique_id.clone())
            .collect();
        live.sort();
        mine.sort();
        sec.record(
            "unit_groups.members",
            eq(&mine, &live),
            || json!({"group": g["unique_id"], "ours": mine, "live": live}),
        );
    }

    // Script zones.
    for z in dj::arr(dump, "script_zones") {
        let id = dj::s(z, "unique_id").unwrap_or_default();
        let ctx = json!({"unique_id": id, "path": z["path"]});
        let Some(o) = inv.zones.iter().find(|x| x.unique_id.as_deref() == Some(id)) else {
            sec.record("zones.presence.only_in_live", Outcome::Mismatch, || ctx.clone());
            continue;
        };
        sec.tally("zones.presence.both", Outcome::Exact);
        let bp = dj::s(z, "blueprint").map(str::to_owned);
        sec.record(
            "zones.blueprint",
            eq(&o.blueprint, &bp),
            || json!({"at": ctx, "ours": o.blueprint, "live": bp}),
        );
        let shapes = dj::arr(z, "shapes");
        sec.record(
            "zones.shape_count",
            eq(&o.shapes.len(), &shapes.len()),
            || json!({"at": ctx, "ours": o.shapes.len(), "live": shapes.len()}),
        );
        for (ls, os) in shapes.iter().zip(&o.shapes) {
            cmp_shape(ls, os, sec, &ctx);
        }
    }

    // Enter points and area transitions, joined on path.
    for (kind, class) in [
        ("area_enter_points", "Kingmaker.View.AreaEnterPoint"),
        ("area_transitions", "Kingmaker.View.MapObjects.AreaTransition"),
    ] {
        for x in dj::arr(dump, kind) {
            let path = dj::s(x, "path").unwrap_or_default();
            let ctx = json!({"kind": kind, "path": path});
            let Some(o) = inv
                .objects
                .iter()
                .find(|o| o.path == path && o.components.iter().any(|c| c.class == class))
            else {
                sec.record(&format!("{kind}.presence.only_in_live"), Outcome::Mismatch, || {
                    ctx.clone()
                });
                continue;
            };
            sec.tally(&format!("{kind}.presence.both"), Outcome::Exact);
            let p: [f32; 3] = dj::fa(&x["position"]).unwrap_or([f32::NAN; 3]);
            let r: [f32; 4] = dj::fa(&x["rotation"]).unwrap_or([f32::NAN; 4]);
            sec.record(
                &format!("{kind}.position"),
                cmp_fs(&o.world.position, &p),
                || json!({"at": ctx, "ours": o.world.position, "live": p}),
            );
            sec.record(
                &format!("{kind}.rotation"),
                cmp_fs(&o.world.rotation, &r),
                || json!({"at": ctx, "ours": o.world.rotation, "live": r}),
            );
            let comp = o.components.iter().find(|c| c.class == class);
            if let Some(bp) = dj::s(x, "blueprint")
                && let Some(c) = comp
            {
                let g = c.fields.str_at("m_Blueprint.guid").unwrap_or_default();
                sec.record(
                    &format!("{kind}.blueprint"),
                    eq(&g, &bp),
                    || json!({"at": ctx, "ours": g, "live": bp}),
                );
            }
            if let (Some(lc), Some(c)) = (x.get("component"), comp) {
                cmp_value("", lc, &c.fields, sec, &ctx);
            }
        }
    }
    sec.note(format!(
        "scenes in our inventory: {}",
        scenes.iter().cloned().collect::<Vec<_>>().join(", ")
    ));
}
