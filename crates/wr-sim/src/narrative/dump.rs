//! The narrative state in WrathOracle's state-dump form (`docs/oracle/state-dump-format.md`,
//! schema `wrath-oracle-state-v1`: the `game` and `narrative` objects), and its comparison
//! with an oracle dump.
//!
//! What the sim writes, per key, is what it models: `game` (game time, mode, area, party
//! lists, money), `narrative.etudes` (saved states), `narrative.etude_facts` (one per
//! `Etude` fact: id, active, parent, component names and data, `IsCompleted`,
//! `CompletionInProgress`), `flags`, `quests` (state, objectives' states), `dialog` (the
//! history sets, `AnswerChecks`), `alignment` (the main character's vector and value),
//! `player` (money). Plus `xp`: each party unit's `Progression.Experience`, which the
//! oracle dump has in `units[].progression`.
//!
//! The comparison is by blueprint (etudes, etude facts, flags, quests, objectives), as a set
//! (dialogue history), and by unit blueprint for the party (units created during a run get
//! fresh ids on both sides). Fact ids of etudes are compared only when both come from the
//! save (ids below the save's id counter); ids allocated during a run are left out, as
//! `oracle-state-diff` renames them.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::EtudeState;
use crate::world::{EntityIdx, FactExt, World};

fn guid(g: wr_content::Guid) -> Value {
    Value::String(g.to_string())
}

fn unit_ref(w: &World, u: EntityIdx) -> Value {
    json!({ "$unit": w.entity(u).unique_id })
}

/// `EtudeState` as the dump writes it (the private enum: 0 Unknown, 1 Started, 2 Completed,
/// 3 PreStarted, 4 PreCompleted).
fn etude_state(s: EtudeState) -> (i64, &'static str) {
    match s {
        EtudeState::Unknown => (0, "Unknown"),
        EtudeState::Started => (1, "Started"),
        EtudeState::Completed => (2, "Completed"),
        EtudeState::PreStarted => (3, "PreStarted"),
        EtudeState::PreCompleted => (4, "PreCompleted"),
    }
}

/// The `game` and `narrative` objects (plus `xp`) of the current world.
pub fn dump(w: &mut World, label: &str) -> Value {
    let lists = super::party::character_lists(w);
    let refs = |w: &World, v: &[EntityIdx]| v.iter().map(|u| unit_ref(w, *u)).collect::<Vec<_>>();
    let bps = |w: &World, v: &[EntityIdx]| {
        v.iter()
            .map(|u| w.unit(*u).and_then(|x| x.blueprint).map_or(Value::Null, guid))
            .collect::<Vec<_>>()
    };
    let game = json!({
        "game_time": w.clock.game_time.ticks(),
        "mode_name": w.player.mode,
        "area": w.player.area.map(guid),
        "area_part": w.player.area_part.map(guid),
        "main_character": w.main_character.map(|u| unit_ref(w, u)),
        "party": refs(w, &lists.party),
        "party_and_pets": refs(w, &lists.party_and_pets),
        "all_characters": refs(w, &lists.all_characters),
        "party_blueprints": bps(w, &lists.party),
        "all_characters_blueprints": bps(w, &lists.all_characters),
        "money": w.player.money,
    });
    let mut etudes: Vec<(String, Value)> = w
        .player
        .etudes_data
        .iter()
        .map(|(g, s)| {
            let (n, name) = etude_state(*s);
            (g.to_string(), json!({ "blueprint": g.to_string(), "state": n, "state_name": name }))
        })
        .collect();
    etudes.sort_by(|a, b| a.0.cmp(&b.0));
    let mut etude_facts: Vec<(String, Value)> = Vec::new();
    if let Some(sys) = w.player.etudes_system {
        for f in w.entity(sys).facts.list.clone() {
            let fact = w.fact(f);
            let Some(g) = fact.blueprint else { continue };
            let (is_completed, in_progress) = match &fact.ext {
                FactExt::Etude { is_completed, completion_in_progress, .. } => {
                    (*is_completed, *completion_in_progress)
                }
                _ => (false, false),
            };
            let comps: Vec<Value> = fact
                .components
                .iter()
                .map(|c| {
                    let class = c.ty().map(|t| {
                        if crate::components::etudes::is_bracket(t) {
                            "EtudeBracketRuntime"
                        } else {
                            wr_content::schema::ty(t).short
                        }
                    });
                    json!({ "name": c.name, "class": class, "data": strip_ids(c.data.clone()) })
                })
                .collect();
            let parent = fact.source_fact.map(|p| {
                json!({ "$fact": w.fact(p).unique_id, "blueprint": w.fact(p).blueprint.map(guid) })
            });
            etude_facts.push((
                g.to_string(),
                json!({
                    "id": fact.unique_id, "class": "Etude", "blueprint": g.to_string(),
                    "active": fact.is_active, "source_fact": parent, "components": comps,
                    "persisted": { "IsCompleted": is_completed, "CompletionInProgress": in_progress },
                }),
            ));
        }
    }
    etude_facts.sort_by(|a, b| a.0.cmp(&b.0));
    let mut flags: Vec<(String, Value)> = w
        .player
        .flags
        .iter()
        .map(|(g, v)| (g.to_string(), json!({ "blueprint": g.to_string(), "value": v })))
        .collect();
    flags.sort_by(|a, b| a.0.cmp(&b.0));
    let mut quests: Vec<(String, Value)> = Vec::new();
    if let Some(qb) = w.player.quest_book {
        for q in w.entity(qb).facts.list.clone() {
            let fact = w.fact(q);
            let Some(g) = fact.blueprint else { continue };
            let FactExt::Quest { state, objectives, .. } = &fact.ext else { continue };
            let objs: Vec<Value> = objectives
                .iter()
                .map(|(og, of)| {
                    json!({ "blueprint": og.to_string(), "state": super::quests::objective_state(w, *of) })
                })
                .collect();
            quests.push((
                g.to_string(),
                json!({ "blueprint": g.to_string(), "state": state, "objectives": objs }),
            ));
        }
    }
    quests.sort_by(|a, b| a.0.cmp(&b.0));
    let set = |d: &crate::csharp::dict::CsDict<wr_content::Guid, ()>| {
        let mut v: Vec<String> = d.keys().map(|g| g.to_string()).collect();
        v.sort();
        v
    };
    let mut checks: Vec<(String, i32)> =
        w.player.dialog.answer_checks.iter().map(|(g, r)| (g.to_string(), *r)).collect();
    checks.sort();
    let dialog = json!({
        "SelectedAnswers": set(&w.player.dialog.selected_answers),
        "ShownAnswerLists": set(&w.player.dialog.shown_answer_lists),
        "ShownCues": set(&w.player.dialog.shown_cues),
        "ShownDialogs": set(&w.player.dialog.shown_dialogs),
        "AnswerChecks": checks.iter().map(|(g, r)| json!([g, {"Result": r}])).collect::<Vec<_>>(),
    });
    let alignment = w
        .main_character
        .and_then(|mc| w.unit(mc))
        .and_then(|u| u.alignment.as_ref())
        .map(|a| json!({ "m_Vector": a.vector, "value_index": a.value() }));
    let xp: Vec<Value> = lists
        .all_characters
        .iter()
        .map(|u| {
            json!({
                "unit": unit_ref(w, *u),
                "blueprint": w.unit(*u).and_then(|x| x.blueprint).map(guid),
                "experience": w.unit(*u).map(|x| x.progression.experience),
            })
        })
        .collect();
    json!({
        "schema": "wrath-oracle-state-v1",
        "label": label,
        "game": game,
        "narrative": {
            "etudes": etudes.into_iter().map(|x| x.1).collect::<Vec<_>>(),
            "etude_facts": etude_facts.into_iter().map(|x| x.1).collect::<Vec<_>>(),
            "flags": flags.into_iter().map(|x| x.1).collect::<Vec<_>>(),
            "quests": quests.into_iter().map(|x| x.1).collect::<Vec<_>>(),
            "dialog": dialog,
            "alignment": alignment,
            "player": { "money": w.player.money },
        },
        "xp": xp,
    })
}

/// One category's comparison.
#[derive(Debug, Default, Clone, Serialize)]
pub struct Category {
    pub equal: usize,
    pub total: usize,
    pub diffs: Vec<String>,
}

impl Category {
    fn check(&mut self, ok: bool, what: impl FnOnce() -> String) {
        self.total += 1;
        if ok {
            self.equal += 1;
        } else if self.diffs.len() < 200 {
            self.diffs.push(what());
        }
    }
}

/// The comparison of two narrative dumps, by category.
#[derive(Debug, Default, Clone, Serialize)]
pub struct NarrativeDiff {
    pub categories: BTreeMap<&'static str, Category>,
    /// Timing-coupled values, reported side by side and not compared (game time).
    pub t1: BTreeMap<String, Value>,
}

impl NarrativeDiff {
    #[must_use]
    pub fn equal(&self) -> bool {
        self.categories.values().all(|c| c.equal == c.total)
    }

    fn cat(&mut self, k: &'static str) -> &mut Category {
        self.categories.entry(k).or_default()
    }
}

fn by_key<'a>(v: &'a Value, path: &[&str], key: &str) -> BTreeMap<String, &'a Value> {
    let mut cur = v;
    for p in path {
        cur = &cur[*p];
    }
    cur.as_array()
        .map(|a| a.iter().filter_map(|x| Some((x.get(key)?.as_str()?.to_owned(), x))).collect())
        .unwrap_or_default()
}

fn keys2<'a>(a: &BTreeMap<String, &'a Value>, b: &BTreeMap<String, &'a Value>) -> BTreeSet<String> {
    a.keys().chain(b.keys()).cloned().collect()
}

/// Components' saved data by component name, with an entity reference as the oracle
/// writes it (`{"$unit": id}`) and as the save stores it (the id) made equal.
fn comp_data(f: &Value) -> BTreeMap<String, Value> {
    fn norm(v: &mut Value) {
        if let Value::Object(o) = v
            && o.len() == 1
            && let Some(id) = o.get("$unit").or_else(|| o.get("$entity")).cloned()
        {
            *v = id;
        }
        match v {
            Value::Array(a) => a.iter_mut().for_each(norm),
            Value::Object(o) => o.values_mut().for_each(norm),
            _ => {}
        }
    }
    f["components"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let mut d = c["data"].clone();
                    norm(&mut d);
                    Some((c["name"].as_str()?.to_owned(), d))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A dump with its run-time ids renamed by first appearance (game lists, then units, then
/// the narrative), one correspondence for the whole dump, so aliasing is kept.
fn renamed(d: &Value, ids: &crate::runner::RuntimeIds) -> Value {
    let mut map = std::collections::HashMap::new();
    let mut out = d.clone();
    for key in ["game", "xp", "units", "narrative"] {
        if let Some(v) = out.get_mut(key) {
            crate::runner::rename_in(v, ids, &mut map);
        }
    }
    out
}

/// Experience by unit id (our `xp[]`, the oracle's `units[].progression`).
fn experience(d: &Value) -> BTreeMap<String, i64> {
    let mut m = BTreeMap::new();
    for u in d["units"].as_array().into_iter().flatten() {
        if let (Some(id), Some(x)) = (u["id"].as_str(), u["progression"]["Experience"].as_i64()) {
            m.insert(id.to_owned(), x);
        }
    }
    for u in d["xp"].as_array().into_iter().flatten() {
        if let (Some(id), Some(x)) = (u["unit"]["$unit"].as_str(), u["experience"].as_i64()) {
            m.insert(id.to_owned(), x);
        }
    }
    m
}

fn unit_list(d: &Value, key: &str) -> Vec<String> {
    d["game"][key]
        .as_array()
        .map(|a| a.iter().map(|u| u["$unit"].as_str().unwrap_or("?").to_owned()).collect())
        .unwrap_or_default()
}

/// Compares our dump with an oracle dump (or two of either). Ids allocated during the run
/// (`ids`) are renamed by first appearance on each side; fact ids that come from the save
/// are compared as they are.
#[must_use]
pub fn compare(ours: &Value, oracle: &Value, ids: &crate::runner::RuntimeIds) -> NarrativeDiff {
    let (ours, oracle) = (&renamed(ours, ids), &renamed(oracle, ids));
    let mut d = NarrativeDiff::default();
    // The structure both dumps must have, with types: missing or mistyped collections
    // mustn't pass as empty ones.
    let arrays: &[&[&str]] = &[
        &["narrative", "etudes"],
        &["narrative", "etude_facts"],
        &["narrative", "flags"],
        &["narrative", "quests"],
        &["narrative", "dialog", "SelectedAnswers"],
        &["narrative", "dialog", "ShownAnswerLists"],
        &["narrative", "dialog", "ShownCues"],
        &["narrative", "dialog", "ShownDialogs"],
        &["narrative", "dialog", "AnswerChecks"],
        &["narrative", "alignment", "m_Vector"],
        &["game", "party"],
        &["game", "party_and_pets"],
        &["game", "all_characters"],
    ];
    for (side, v) in [("our", ours), ("the oracle's", oracle)] {
        for path in arrays {
            let mut cur = v;
            for p in *path {
                cur = &cur[*p];
            }
            d.cat("structure").check(cur.is_array(), || {
                format!("{side} dump: {} is not an array", path.join("."))
            });
        }
        for k in ["area", "area_part", "mode_name", "money"] {
            d.cat("structure")
                .check(v["game"].get(k).is_some(), || format!("{side} dump lacks game.{k}"));
        }
    }
    // Where and in which mode; game time is timing-coupled (T1): reported, not compared.
    for k in ["area", "area_part", "mode_name"] {
        let (x, y) = (&ours["game"][k], &oracle["game"][k]);
        d.cat("game").check(x == y, || format!("{k}: ours {x}, oracle {y}"));
    }
    d.t1.insert(
        "game_time".into(),
        json!({ "ours": ours["game"]["game_time"], "oracle": oracle["game"]["game_time"] }),
    );
    // Etude states.
    let (a, b) = (
        by_key(ours, &["narrative", "etudes"], "blueprint"),
        by_key(oracle, &["narrative", "etudes"], "blueprint"),
    );
    for k in keys2(&a, &b) {
        let (x, y) = (a.get(&k).map(|v| &v["state"]), b.get(&k).map(|v| &v["state"]));
        d.cat("etude_state").check(x == y, || format!("etude {k}: ours {x:?}, oracle {y:?}"));
    }
    // Etude facts: present, active, completion flags, parent, component data.
    let (a, b) = (
        by_key(ours, &["narrative", "etude_facts"], "blueprint"),
        by_key(oracle, &["narrative", "etude_facts"], "blueprint"),
    );
    for k in keys2(&a, &b) {
        let (Some(x), Some(y)) = (a.get(&k), b.get(&k)) else {
            d.cat("etude_fact").check(false, || {
                format!(
                    "etude fact {k}: only on {} side",
                    if a.contains_key(&k) { "our" } else { "the oracle's" }
                )
            });
            continue;
        };
        d.cat("etude_fact").check(true, String::new);
        d.cat("etude_active").check(x["active"] == y["active"], || {
            format!("etude {k} active: ours {}, oracle {}", x["active"], y["active"])
        });
        for f in ["IsCompleted", "CompletionInProgress"] {
            d.cat("etude_completion").check(x["persisted"][f] == y["persisted"][f], || {
                format!("etude {k} {f}: ours {}, oracle {}", x["persisted"][f], y["persisted"][f])
            });
        }
        d.cat("etude_parent").check(
            x["source_fact"]["blueprint"] == y["source_fact"]["blueprint"],
            || {
                format!(
                    "etude {k} parent: ours {}, oracle {}",
                    x["source_fact"]["blueprint"], y["source_fact"]["blueprint"]
                )
            },
        );
        // Ids allocated during the run were renamed (`new#…`) and are compared by
        // correspondence; save-stable ids must be equal.
        let saved = |v: &Value| v["id"].as_str().is_some_and(|s| !s.starts_with("new#"));
        if saved(x) && saved(y) {
            d.cat("etude_fact_id").check(x["id"] == y["id"], || {
                format!("etude {k} id: ours {}, oracle {}", x["id"], y["id"])
            });
        }
        let (ca, cb) = (comp_data(x), comp_data(y));
        for name in ca.keys().chain(cb.keys()).collect::<BTreeSet<_>>() {
            let (p, q) = (ca.get(name), cb.get(name));
            // A null `m_Data` and an all-default one aren't distinguishable in a dump
            // made after `Data` was read; compare as written.
            d.cat("etude_component_data")
                .check(p == q, || format!("etude {k} component {name}: ours {p:?}, oracle {q:?}"));
        }
    }
    // Flags.
    let (a, b) = (
        by_key(ours, &["narrative", "flags"], "blueprint"),
        by_key(oracle, &["narrative", "flags"], "blueprint"),
    );
    for k in keys2(&a, &b) {
        let (x, y) = (a.get(&k).map(|v| &v["value"]), b.get(&k).map(|v| &v["value"]));
        d.cat("flags").check(x == y, || format!("flag {k}: ours {x:?}, oracle {y:?}"));
    }
    // Quests and objectives.
    let (a, b) = (
        by_key(ours, &["narrative", "quests"], "blueprint"),
        by_key(oracle, &["narrative", "quests"], "blueprint"),
    );
    for k in keys2(&a, &b) {
        let (x, y) = (a.get(&k), b.get(&k));
        d.cat("quests").check(x.map(|v| &v["state"]) == y.map(|v| &v["state"]), || {
            format!(
                "quest {k}: ours {:?}, oracle {:?}",
                x.map(|v| &v["state"]),
                y.map(|v| &v["state"])
            )
        });
        let objs = |v: Option<&&Value>| -> BTreeMap<String, Value> {
            v.and_then(|v| v["objectives"].as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|o| {
                            Some((o["blueprint"].as_str()?.to_owned(), o["state"].clone()))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let (oa, ob) = (objs(x), objs(y));
        for o in oa.keys().chain(ob.keys()).collect::<BTreeSet<_>>() {
            d.cat("objectives").check(oa.get(o) == ob.get(o), || {
                format!("objective {o}: ours {:?}, oracle {:?}", oa.get(o), ob.get(o))
            });
        }
    }
    // Dialogue history (sets).
    for k in ["SelectedAnswers", "ShownAnswerLists", "ShownCues", "ShownDialogs", "AnswerChecks"] {
        let (x, y) = (&ours["narrative"]["dialog"][k], &oracle["narrative"]["dialog"][k]);
        let norm = |v: &Value| -> BTreeSet<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .map(|e| match e {
                            // `[guid, CheckResult persisted]`: the oracle writes the persisted members.
                            Value::Array(p) => format!(
                                "{}={}",
                                p[0],
                                p[1].get("Result")
                                    .or_else(|| p[1].get("m_Result"))
                                    .unwrap_or(&p[1])
                            ),
                            other => other.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let (sx, sy) = (norm(x), norm(y));
        for e in sx.symmetric_difference(&sy) {
            d.cat("dialog_history").check(false, || {
                format!(
                    "{k}: {e} only on {} side",
                    if sx.contains(e) { "our" } else { "the oracle's" }
                )
            });
        }
        for _ in sx.intersection(&sy) {
            d.cat("dialog_history").check(true, String::new);
        }
    }
    // Alignment of the main character (vector bits).
    let va = &ours["narrative"]["alignment"]["m_Vector"];
    let vb = &oracle["narrative"]["alignment"]["m_Vector"];
    let bits = |v: &Value| -> Vec<u32> {
        v.as_array()
            .map(|a| a.iter().filter_map(|x| x.as_f64()).map(|f| (f as f32).to_bits()).collect())
            .unwrap_or_default()
    };
    d.cat("alignment")
        .check(bits(va) == bits(vb), || format!("alignment vector: ours {va}, oracle {vb}"));
    // Party lists, by unit identity (run-time ids by correspondence); money.
    for k in ["party", "party_and_pets", "all_characters"] {
        let (x, y) = (unit_list(ours, k), unit_list(oracle, k));
        d.cat("party").check(x == y, || format!("{k}: ours {x:?}, oracle {y:?}"));
    }
    d.cat("money").check(ours["game"]["money"] == oracle["game"]["money"], || {
        format!("money: ours {}, oracle {}", ours["game"]["money"], oracle["game"]["money"])
    });
    // Experience of every character on either side, by unit.
    let (xa, xb) = (experience(ours), experience(oracle));
    let mut chars: BTreeSet<String> = unit_list(ours, "all_characters").into_iter().collect();
    chars.extend(unit_list(oracle, "all_characters"));
    for u in chars {
        d.cat("experience").check(xa.get(&u) == xb.get(&u), || {
            format!("experience of {u}: ours {:?}, oracle {:?}", xa.get(&u), xb.get(&u))
        });
    }
    d
}

/// A summary line per category.
#[must_use]
pub fn summary(d: &NarrativeDiff) -> Map<String, Value> {
    d.categories
        .iter()
        .map(|(k, c)| ((*k).to_owned(), json!(format!("{}/{}", c.equal, c.total))))
        .collect()
}

/// The save's `$id` markers aren't data (the oracle's dump has none).
fn strip_ids(v: Option<Value>) -> Option<Value> {
    fn go(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.remove("$id");
                m.values_mut().for_each(go);
            }
            Value::Array(a) => a.iter_mut().for_each(go),
            _ => {}
        }
    }
    v.map(|mut v| {
        go(&mut v);
        v
    })
}
