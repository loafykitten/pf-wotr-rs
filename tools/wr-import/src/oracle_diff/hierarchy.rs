//! Hierarchy diffs: every GameObject of the area's scenes in `hierarchy/<scene>.json`
//! against our decoded scenes, joined on (scene, path, sibling path).
//!
//! Expected runtime differences, listed but not counted as errors: GameObjects the game
//! creates at runtime (no counterpart in the scene file), transforms under an `Animator`
//! (animated bones and props move), and components added at runtime.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use anyhow::Result;
use serde_json::{Value as J, json};

use super::json::{self as dj, cmp_fs, eq};
use super::{Ours, Outcome, Section};
use crate::scene::GoSnapshot;

fn short_scene(s: &str) -> &str {
    s.rsplit('/').next().unwrap_or(s).trim_end_matches(".unity")
}

/// Unity class names as the dump writes them (`UnityEngine.` prefix for built-ins).
fn norm(c: &str) -> &str {
    c.strip_prefix("UnityEngine.").unwrap_or(c)
}

fn sib(v: &J) -> Vec<u32> {
    v.as_array()
        .map(|a| a.iter().filter_map(|x| x.as_u64().and_then(|u| u32::try_from(u).ok())).collect())
        .unwrap_or_default()
}

pub fn diff(dir: &Path, ours: &Ours, sec: &mut Section) -> Result<()> {
    for sd in &ours.scenes {
        let name = short_scene(&sd.scene).to_owned();
        let p = dir.join(format!("{name}.json"));
        if !p.exists() {
            sec.record("coverage.missing_scene_dump", Outcome::Mismatch, || json!({"scene": name}));
            continue;
        }
        let dump = dj::load(&p)?;
        let live = dj::arr(&dump, "objects");
        let mine = sd.game_objects();
        let mut by_key: HashMap<(String, Vec<u32>), Vec<&GoSnapshot>> = HashMap::new();
        for g in &mine {
            by_key.entry((g.path.clone(), g.sibling_path.clone())).or_default().push(g);
        }
        // Paths of live objects carrying an Animator: their descendants animate.
        let animated: Vec<String> = live
            .iter()
            .filter(|o| {
                dj::arr(o, "components")
                    .iter()
                    .any(|c| c.get(0).and_then(J::as_str) == Some("UnityEngine.Animator"))
            })
            .filter_map(|o| dj::s(o, "path").map(|p| format!("{p}/")))
            .collect();
        let mut matched = BTreeSet::new();
        let mut used: HashMap<(String, Vec<u32>), usize> = HashMap::new();
        for o in live {
            let path = dj::s(o, "path").unwrap_or_default().to_owned();
            let key = (path.clone(), sib(&o["sibling_path"]));
            let n = used.entry(key.clone()).or_default();
            let Some(g) = by_key.get(&key).and_then(|v| v.get(*n)).copied() else {
                sec.record(
                    "presence.only_in_live",
                    Outcome::Expected,
                    || json!({"scene": name, "path": path}),
                );
                continue;
            };
            *n += 1;
            matched.insert(g.path_id);
            sec.tally("presence.both", Outcome::Exact);
            let ex = |f: &str, ours: J, live: J| json!({"scene": name, "path": path, "field": f, "ours": ours, "live": live});
            sec.record("name", eq(&g.name.as_str(), &dj::s(o, "name").unwrap_or_default()), || {
                ex("name", json!(g.name), o["name"].clone())
            });
            let a_self = o["active_self"].as_bool().unwrap_or(false);
            sec.record("active_self", eq(&g.active_self, &a_self), || {
                ex("active_self", json!(g.active_self), json!(a_self))
            });
            let a_h = o["active_in_hierarchy"].as_bool().unwrap_or(false);
            sec.record("active_in_hierarchy", eq(&g.active_in_hierarchy, &a_h), || {
                ex("active_in_hierarchy", json!(g.active_in_hierarchy), json!(a_h))
            });
            let layer = o["layer"].as_i64().unwrap_or(-1);
            sec.record("layer", eq(&g.layer, &layer), || ex("layer", json!(g.layer), json!(layer)));
            let anim = animated.iter().any(|a| path.starts_with(a.as_str()));
            let downgrade =
                |o: Outcome| if anim && o == Outcome::Mismatch { Outcome::Expected } else { o };
            if let Some(l) = g.local {
                let lp: [f32; 3] = dj::fa(&o["local_position"]).unwrap_or([f32::NAN; 3]);
                let lr: [f32; 4] = dj::fa(&o["local_rotation"]).unwrap_or([f32::NAN; 4]);
                let ls: [f32; 3] = dj::fa(&o["local_scale"]).unwrap_or([f32::NAN; 3]);
                let op = l.position.to_array();
                let or = l.rotation.to_array();
                let os = l.scale.to_array();
                sec.record("local_position", downgrade(cmp_fs(&op, &lp)), || {
                    ex("local_position", json!(op), json!(lp))
                });
                sec.record("local_rotation", downgrade(cmp_fs(&or, &lr)), || {
                    ex("local_rotation", json!(or), json!(lr))
                });
                sec.record("local_scale", downgrade(cmp_fs(&os, &ls)), || {
                    ex("local_scale", json!(os), json!(ls))
                });
            }
            let wp: [f32; 3] = dj::fa(&o["position"]).unwrap_or([f32::NAN; 3]);
            let wr: [f32; 4] = dj::fa(&o["rotation"]).unwrap_or([f32::NAN; 4]);
            let wl: [f32; 3] = dj::fa(&o["lossy_scale"]).unwrap_or([f32::NAN; 3]);
            let wm = dj::matrix_cols(&o["local_to_world"]).unwrap_or([f32::NAN; 16]);
            let w = &g.world;
            sec.record("position", downgrade(cmp_fs(&w.position, &wp)), || {
                ex("position", json!(w.position), json!(wp))
            });
            // q and -q are the same rotation; the game's chain product keeps one sign.
            let rot = {
                let a = cmp_fs(&w.rotation, &wr);
                if a == Outcome::Mismatch {
                    cmp_fs(&w.rotation.map(|x| -x), &wr).max_bad(a)
                } else {
                    a
                }
            };
            sec.record("rotation", downgrade(rot), || ex("rotation", json!(w.rotation), json!(wr)));
            sec.record("lossy_scale", downgrade(cmp_fs(&w.lossy_scale, &wl)), || {
                ex("lossy_scale", json!(w.lossy_scale), json!(wl))
            });
            sec.record("local_to_world", downgrade(cmp_fs(&w.matrix, &wm)), || {
                ex("local_to_world", json!(w.matrix), json!(wm))
            });
            // Components: ours in m_Component order vs live `GetComponents` order.
            let lc: Vec<String> = dj::arr(o, "components")
                .iter()
                .filter_map(|c| c.get(0).and_then(J::as_str).map(|s| norm(s).to_owned()))
                .collect();
            let oc: Vec<String> = g.components.iter().map(|c| norm(c).to_owned()).collect();
            let o_comp = if oc == lc {
                Outcome::Exact
            } else if lc.len() > oc.len() && oc.iter().all(|c| lc.contains(c)) {
                Outcome::Expected // added at runtime
            } else {
                Outcome::Mismatch
            };
            sec.record("components", o_comp, || ex("components", json!(oc), json!(lc)));
        }
        for g in &mine {
            if !matched.contains(&g.path_id) {
                sec.record(
                    "presence.only_in_ours",
                    Outcome::Mismatch,
                    || json!({"scene": name, "path": g.path, "sibling_path": g.sibling_path}),
                );
            }
        }
    }
    Ok(())
}

trait MaxBad {
    fn max_bad(self, other: Outcome) -> Outcome;
}

impl MaxBad for Outcome {
    /// The better of a sign-flipped comparison and the original.
    fn max_bad(self, other: Outcome) -> Outcome {
        match (self, other) {
            (Outcome::Exact, _) | (_, Outcome::Exact) => Outcome::Exact,
            (Outcome::Close, _) | (_, Outcome::Close) => Outcome::Close,
            _ => Outcome::Mismatch,
        }
    }
}
