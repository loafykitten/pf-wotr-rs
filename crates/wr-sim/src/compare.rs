//! Compares our sheet dump with a WrathOracle state dump (`wrath-oracle-state-v1`), field
//! by field, per `docs/oracle/state-dump-format.md` § Comparing: units by `id`, stats by
//! `type`, facts by `id` (order checked separately), modifiers per stat and descriptor in
//! list order. Keys we don't produce are listed as not compared, never counted as matches.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value as J;

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub units_compared: usize,
    pub units_missing_ours: Vec<String>,
    pub units_missing_oracle: Vec<String>,
    /// Field class → (equal, total).
    pub fields: BTreeMap<String, (u64, u64)>,
    /// Per unit: (equal, total).
    pub per_unit: BTreeMap<String, (u64, u64)>,
    /// First differences (path, ours, oracle).
    pub diffs: Vec<(String, J, J)>,
    pub match_rate: f64,
}

impl Report {
    fn check(&mut self, class: &str, path: String, ours: &J, theirs: &J) {
        let unit = path.split('.').next().unwrap_or("").to_owned();
        let pu = self.per_unit.entry(unit).or_default();
        pu.1 += 1;
        pu.0 += u64::from(ours == theirs);
        let e = self.fields.entry(class.to_owned()).or_default();
        e.1 += 1;
        if ours == theirs {
            e.0 += 1;
        } else if self.diffs.len() < 500 {
            self.diffs.push((path, ours.clone(), theirs.clone()));
        }
    }
}

fn by<'a>(v: &'a J, key: &str) -> BTreeMap<String, &'a J> {
    v.as_array().map(|a| a.iter().map(|x| (x[key].to_string(), x)).collect()).unwrap_or_default()
}

/// Facts by blueprint and occurrence: `<blueprint>#<n>` for the n-th fact of that blueprint in
/// list order.
fn by_occurrence(v: &J) -> BTreeMap<String, &J> {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = BTreeMap::new();
    for x in v.as_array().map(Vec::as_slice).unwrap_or_default() {
        let bp = x["blueprint"].as_str().map_or_else(|| x["blueprint"].to_string(), str::to_owned);
        let n = seen.entry(bp.clone()).or_default();
        out.insert(format!("{bp}#{n}"), x);
        *n += 1;
    }
    out
}

/// The comparable part of a modifier.
fn modifier_key(m: &J) -> J {
    serde_json::json!([
        m["descriptor"],
        m["value"],
        m["source"]["$fact"],
        m["source_component"],
        m["item"]["$item"]
    ])
}

/// Renames the item ids created during the run by order of first appearance on each side
/// (`Guid.NewGuid()` is random in each run), so the two dumps compare on structure. Only ids
/// positively known as run-time are renamed: those our dump says it did not load
/// (`sim.saved_item_ids` lists every loaded item), on either side. A saved id stays as it is,
/// so a modifier from the wrong saved item still differs. Without that list nothing is renamed.
fn canonical_runtime_items(ours: &J, oracle: &J) -> (J, J) {
    let Some(saved) = ours["sim"]["saved_item_ids"].as_array() else {
        return (ours.clone(), oracle.clone());
    };
    let saved: std::collections::HashSet<&str> = saved.iter().filter_map(J::as_str).collect();
    let rename = |d: &J| -> J {
        let mut map: BTreeMap<String, String> = BTreeMap::new();
        let mut d = d.clone();
        for u in d["units"].as_array_mut().into_iter().flatten() {
            for s in u["stats"].as_array_mut().into_iter().flatten() {
                for m in s["modifiers"].as_array_mut().into_iter().flatten() {
                    if let Some(i) = m["item"]["$item"].as_str()
                        && !saved.contains(i)
                    {
                        let n = map.len();
                        let name =
                            map.entry(i.to_owned()).or_insert_with(|| format!("runtime-item-{n}"));
                        m["item"]["$item"] = J::String(name.clone());
                    }
                }
            }
        }
        d
    };
    (rename(ours), rename(oracle))
}

/// Renames the `AbilityData.UniqueId`s a spellbook holds that were created during the run
/// (`Guid.NewGuid()`), by order of first appearance in each spellbook on each side: the ids our
/// dump lists in `sim.saved_spell_ids` (every spell loaded from the save) stay strict, and
/// without that list nothing is renamed.
fn canonical_runtime_spells(ours: &J, oracle: &J) -> (J, J) {
    let Some(saved) = ours["sim"]["saved_spell_ids"].as_array() else {
        return (ours.clone(), oracle.clone());
    };
    let saved: std::collections::HashSet<&str> = saved.iter().filter_map(J::as_str).collect();
    fn walk(
        v: &mut J,
        saved: &std::collections::HashSet<&str>,
        map: &mut BTreeMap<String, String>,
    ) {
        match v {
            J::Object(o) => {
                for (k, x) in o.iter_mut() {
                    if k == "UniqueId"
                        && let Some(s) = x.as_str()
                        && !saved.contains(s)
                    {
                        let n = map.len();
                        let name =
                            map.entry(s.to_owned()).or_insert_with(|| format!("runtime-spell-{n}"));
                        *x = J::String(name.clone());
                    } else {
                        walk(x, saved, map);
                    }
                }
            }
            J::Array(a) => a.iter_mut().for_each(|x| walk(x, saved, map)),
            _ => {}
        }
    }
    let rename = |d: &J| -> J {
        let mut d = d.clone();
        for u in d["units"].as_array_mut().into_iter().flatten() {
            // One numbering per spellbook, so a spellbook that differs doesn't shift the names
            // in the others.
            for sb in u.get_mut("spellbooks").and_then(J::as_array_mut).into_iter().flatten() {
                walk(sb, &saved, &mut BTreeMap::new());
            }
        }
        d
    };
    (rename(ours), rename(oracle))
}

/// Run-time spell ids held by more than one spellbook (of one unit or several), as sorted
/// `unit/spellbook` location lists. `AbilityData` gets a fresh id each time it's constructed,
/// so the per-spellbook renaming above can't see an id shared across spellbooks; both sides
/// must share ids between the same spellbooks.
fn spell_aliases(d: &J, saved: &std::collections::HashSet<&str>) -> Vec<Vec<String>> {
    fn ids<'a>(v: &'a J, out: &mut Vec<&'a str>) {
        match v {
            J::Object(o) => {
                for (k, x) in o {
                    match x.as_str() {
                        Some(s) if k == "UniqueId" => out.push(s),
                        _ => ids(x, out),
                    }
                }
            }
            J::Array(a) => a.iter().for_each(|x| ids(x, out)),
            _ => {}
        }
    }
    let mut at: BTreeMap<&str, std::collections::BTreeSet<String>> = BTreeMap::new();
    for u in d["units"].as_array().into_iter().flatten() {
        for sb in u["spellbooks"].as_array().into_iter().flatten() {
            let loc = format!("{}/{}", u["id"].as_str().unwrap_or("?"), sb["blueprint"]);
            let mut found = Vec::new();
            ids(sb, &mut found);
            for id in found.into_iter().filter(|i| !saved.contains(i)) {
                at.entry(id).or_default().insert(loc.clone());
            }
        }
    }
    let mut v: Vec<Vec<String>> =
        at.into_values().filter(|l| l.len() > 1).map(|l| l.into_iter().collect()).collect();
    v.sort();
    v
}

#[must_use]
pub fn compare(ours: &J, oracle: &J) -> Report {
    let mut r = Report::default();
    if let Some(saved) = ours["sim"]["saved_spell_ids"].as_array() {
        let saved: std::collections::HashSet<&str> = saved.iter().filter_map(J::as_str).collect();
        let (a, b) = (spell_aliases(ours, &saved), spell_aliases(oracle, &saved));
        if !a.is_empty() || !b.is_empty() {
            r.check(
                "spellbook.identity",
                "_global.spell_aliases".into(),
                &serde_json::json!(a),
                &serde_json::json!(b),
            );
        }
    }
    // `AbilityParams.Empty` at dump time, when the oracle's dump records it.
    if oracle["abilityParamsEmptyNow"].is_object() && ours["abilityParamsEmptyNow"].is_object() {
        for k in ["CasterLevel", "RankBonus"] {
            r.check(
                "ability_params_empty",
                format!("_global.abilityParamsEmptyNow.{k}"),
                &ours["abilityParamsEmptyNow"][k],
                &oracle["abilityParamsEmptyNow"][k],
            );
        }
    }
    let (ours, oracle) = &canonical_runtime_items(ours, oracle);
    let (ours, oracle) = &canonical_runtime_spells(ours, oracle);
    let ou = by(&ours["units"], "id");
    let tu = by(&oracle["units"], "id");
    for id in tu.keys() {
        if !ou.contains_key(id) {
            r.units_missing_ours.push(id.clone());
        }
    }
    for id in ou.keys() {
        if !tu.contains_key(id) {
            r.units_missing_oracle.push(id.clone());
        }
    }
    for (id, o) in &ou {
        let Some(t) = tu.get(id) else { continue };
        r.units_compared += 1;
        for k in ["max", "damage", "temporary", "left"] {
            r.check("hp", format!("{id}.hp.{k}"), &o["hp"][k], &t["hp"][k]);
        }
        for k in ["conditions", "condition_immunities"] {
            r.check("conditions", format!("{id}.{k}"), &o[k], &t[k]);
        }
        for k in ["character_level", "mythic_level"] {
            r.check(
                "progression",
                format!("{id}.progression.{k}"),
                &o["progression"][k],
                &t["progression"][k],
            );
        }
        let os = by(&o["stats"], "type");
        let ts = by(&t["stats"], "type");
        for (ty, s) in &os {
            let Some(ts) = ts.get(ty) else { continue };
            let name = s["type_name"].as_str().unwrap_or("?");
            for k in ["base", "permanent", "modified", "raw"] {
                r.check(&format!("stat.{k}"), format!("{id}.{name}.{k}"), &s[k], &ts[k]);
            }
            if let Some(ex) = s["extra"].as_object() {
                for (k, v) in ex {
                    if let Some(tv) = ts["extra"].get(k) {
                        // A `$stat` reference compares by stat and owner (the oracle adds
                        // the referenced stat's value for convenience).
                        let tv = if tv.get("$stat").is_some() {
                            serde_json::json!({"$stat": tv["$stat"], "owner": tv["owner"]})
                        } else {
                            tv.clone()
                        };
                        r.check("stat.extra", format!("{id}.{name}.extra.{k}"), v, &tv);
                    }
                }
            }
            let om: Vec<J> = s["modifiers"]
                .as_array()
                .map(|a| a.iter().map(modifier_key).collect())
                .unwrap_or_default();
            let tm: Vec<J> = ts["modifiers"]
                .as_array()
                .map(|a| a.iter().map(modifier_key).collect())
                .unwrap_or_default();
            r.check("stat.modifiers", format!("{id}.{name}.modifiers"), &J::from(om), &J::from(tm));
        }
        for (class, key) in [("resource", "resources"), ("spellbook", "spellbooks")] {
            let or = by(&o[key], "blueprint");
            let tr = by(&t[key], "blueprint");
            for bp in tr.keys() {
                if !or.contains_key(bp) {
                    r.check(
                        &format!("{key}.present"),
                        format!("{id}.{key}.{bp}"),
                        &J::Bool(false),
                        &J::Bool(true),
                    );
                }
            }
            for (bp, x) in &or {
                let Some(y) = tr.get(bp) else {
                    r.check(
                        &format!("{key}.present"),
                        format!("{id}.{key}.{bp}"),
                        &J::Bool(true),
                        &J::Bool(false),
                    );
                    continue;
                };
                r.check(
                    &format!("{key}.present"),
                    format!("{id}.{key}.{bp}"),
                    &J::Bool(true),
                    &J::Bool(true),
                );
                let fields: &[&str] = if class == "resource" {
                    &["amount", "persisted"]
                } else {
                    &["caster_level", "persisted"]
                };
                for k in fields {
                    r.check(
                        &format!("{class}.{k}"),
                        format!("{id}.{key}.{bp}.{k}"),
                        &x[*k],
                        &y[*k],
                    );
                }
            }
        }
        // Facts are paired by blueprint and occurrence (the n-th fact of a blueprint in list
        // order), so an offset in the run's id allocation doesn't pair different facts; whether
        // a pair's ids agree is its own field (`fact.id`), the order is the blueprints'.
        let of = by_occurrence(&o["facts"]);
        let tf = by_occurrence(&t["facts"]);
        let order = |v: &J| -> J {
            J::from(
                v.as_array()
                    .map(|a| a.iter().map(|x| x["blueprint"].clone()).collect::<Vec<_>>())
                    .unwrap_or_default(),
            )
        };
        r.check("facts.order", format!("{id}.facts"), &order(&o["facts"]), &order(&t["facts"]));
        for key in tf.keys() {
            if !of.contains_key(key) {
                r.check(
                    "facts.present",
                    format!("{id}.facts.{key}"),
                    &J::Bool(false),
                    &J::Bool(true),
                );
            }
        }
        for (key, f) in &of {
            let Some(tfa) = tf.get(key) else {
                r.check(
                    "facts.present",
                    format!("{id}.facts.{key}"),
                    &J::Bool(true),
                    &J::Bool(false),
                );
                continue;
            };
            for k in ["id", "rank", "active", "class"] {
                r.check(&format!("fact.{k}"), format!("{id}.facts.{key}.{k}"), &f[k], &tfa[k]);
            }
        }
    }
    let (eq, total) = r.fields.values().fold((0, 0), |a, (e, t)| (a.0 + e, a.1 + t));
    #[allow(clippy::cast_precision_loss)]
    {
        r.match_rate = if total == 0 { 0.0 } else { eq as f64 / total as f64 };
    }
    r
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn facts_pair_by_blueprint_and_occurrence_and_ids_are_their_own_field() {
        let unit = |facts: J| json!({"units": [{"id": "u", "stats": [], "facts": facts}]});
        // The oracle's ids run one ahead (another fact made earlier in its run).
        let ours = unit(json!([
            {"id": "f1", "blueprint": "a", "rank": 1, "active": true, "class": "Feature"},
            {"id": "f2", "blueprint": "b", "rank": 1, "active": true, "class": "Buff"},
            {"id": "f3", "blueprint": "a", "rank": 2, "active": true, "class": "Feature"},
        ]));
        let theirs = unit(json!([
            {"id": "f2", "blueprint": "a", "rank": 1, "active": true, "class": "Feature"},
            {"id": "f3", "blueprint": "b", "rank": 1, "active": true, "class": "Buff"},
            {"id": "f4", "blueprint": "a", "rank": 2, "active": true, "class": "Feature"},
        ]));
        let r = compare(&ours, &theirs);
        assert_eq!(r.fields["fact.rank"], (3, 3), "fields compare the paired facts");
        assert_eq!(r.fields["fact.id"], (0, 3), "every id differs, reported apart");
        assert_eq!(r.fields["facts.order"], (1, 1));
        assert!(!r.fields.contains_key("facts.present"));
    }

    /// A dump with one unit whose stat has one modifier from `item`.
    fn dump(item: &str, saved: &[&str]) -> J {
        json!({
            "units": [{"id": "u", "stats": [{
                "type": 1, "type_name": "Strength", "base": 10, "permanent": 10, "modified": 10,
                "raw": 10,
                "modifiers": [{"descriptor": 1, "value": 2, "source": null,
                               "source_component": null, "item": {"$item": item}}],
            }]}],
            "sim": {"saved_item_ids": saved},
        })
    }

    /// Items created during the run have random ids on each side: they compare by structure.
    #[test]
    fn run_time_item_ids_compare_by_structure() {
        let r = compare(&dump("new-a", &["s1"]), &dump("new-b", &["s1"]));
        assert!((r.match_rate - 1.0).abs() < f64::EPSILON, "{:?}", r.diffs);
    }

    /// A modifier from the wrong saved item still differs; so does a saved item against a
    /// run-time one.
    #[test]
    fn saved_item_ids_stay_strict() {
        let saved = ["s1", "s2"];
        let r = compare(&dump("s1", &saved), &dump("s2", &saved));
        assert!(r.match_rate < 1.0, "wrong saved item must differ");
        let r = compare(&dump("s1", &saved), &dump("new-b", &saved));
        assert!(r.match_rate < 1.0, "saved against run-time must differ");
    }

    /// Without the list of saved ids nothing is renamed.
    #[test]
    fn nothing_is_renamed_without_the_saved_list() {
        let mut a = dump("x", &[]);
        a["sim"] = J::Null;
        let r = compare(&a, &dump("y", &[]));
        assert!(r.match_rate < 1.0);
    }

    /// A dump with one unit holding one spellbook whose first known spell has `id`.
    fn books(id: &str, saved: &[&str], extra: Option<&str>) -> J {
        let spell = |i: &str| json!({"Blueprint": "s", "UniqueId": i});
        let mut known = vec![spell(id)];
        known.extend(extra.map(spell));
        json!({
            "units": [{"id": "u", "spellbooks": [{
                "blueprint": "b", "caster_level": 1, "persisted": {"m_KnownSpells": [known]},
            }]}],
            "sim": {"saved_spell_ids": saved},
        })
    }

    /// Spells created during the run have random ids: they compare by structure, in each
    /// spellbook; a spell from the save keeps its id.
    #[test]
    fn run_time_spell_ids_compare_by_structure_and_saved_ones_stay_strict() {
        let r = compare(&books("a", &["s1"], None), &books("b", &["s1"], None));
        assert_eq!(r.fields["spellbook.persisted"], (1, 1), "{:?}", r.diffs);
        let r = compare(&books("s1", &["s1", "s2"], None), &books("s2", &["s1", "s2"], None));
        assert_eq!(r.fields["spellbook.persisted"], (0, 1), "a different saved spell differs");
        let r = compare(&books("s1", &["s1"], None), &books("zz", &["s1"], None));
        assert_eq!(r.fields["spellbook.persisted"], (0, 1), "saved against run-time differs");
        let r = compare(&books("a", &["s1"], Some("b")), &books("a", &["s1"], None));
        assert_eq!(r.fields["spellbook.persisted"], (0, 1), "an extra spell differs");
    }

    /// An id shared by two spellbooks on one side only differs, though each spellbook alone
    /// compares equal by structure.
    #[test]
    fn a_spell_id_shared_across_spellbooks_differs() {
        let two = |x: &str, y: &str| {
            let book = |bp: &str, i: &str| {
                json!({"blueprint": bp, "caster_level": 1,
                       "persisted": {"m_KnownSpells": [[{"Blueprint": "s", "UniqueId": i}]]}})
            };
            json!({"units": [{"id": "u", "spellbooks": [book("b1", x), book("b2", y)]}],
                   "sim": {"saved_spell_ids": []}})
        };
        let r = compare(&two("a", "a"), &two("p", "q"));
        assert_eq!(r.fields["spellbook.persisted"], (2, 2), "{:?}", r.diffs);
        assert_eq!(r.fields["spellbook.identity"], (0, 1));
        let r = compare(&two("a", "b"), &two("p", "q"));
        assert!(!r.fields.contains_key("spellbook.identity"));
    }

    /// Resources and spellbooks present on one side only are counted as differences.
    #[test]
    fn missing_spellbooks_and_resources_count() {
        let ours = json!({"units": [{"id": "u", "resources": [], "spellbooks": []}]});
        let oracle = json!({"units": [{"id": "u",
            "resources": [{"blueprint": "r", "amount": 1, "persisted": {}}],
            "spellbooks": [{"blueprint": "b", "caster_level": 1, "persisted": {}}]}]});
        let r = compare(&ours, &oracle);
        assert_eq!(r.fields["resources.present"], (0, 1));
        assert_eq!(r.fields["spellbooks.present"], (0, 1));
    }
}

/// A flag that differs: (unit, flag, ours, oracle).
pub type FlagDiff = (String, String, J, J);

/// The unit flags the field comparison doesn't cover: `in_game`, `is_dead`, `is_conscious`
/// (`UnitState.LifeState`, `EntityDataBase.IsInGame`). Additive: a separate report, so the
/// match rates of [`compare`] stay what they were. Returns (equal, total) per flag and the
/// differences as (unit, flag, ours, oracle).
#[must_use]
pub fn compare_flags(ours: &J, oracle: &J) -> (BTreeMap<String, (u64, u64)>, Vec<FlagDiff>) {
    let (a, b) = (by(&ours["units"], "id"), by(&oracle["units"], "id"));
    let mut counts: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut diffs = Vec::new();
    for (id, u) in &a {
        let Some(o) = b.get(id) else { continue };
        for f in ["in_game", "is_dead", "is_conscious"] {
            let (x, y) = (&u["flags"][f], &o["flags"][f]);
            if x.is_null() || y.is_null() {
                continue;
            }
            let e = counts.entry(f.to_owned()).or_default();
            e.1 += 1;
            if x == y {
                e.0 += 1;
            } else {
                diffs.push((id.clone(), f.to_owned(), x.clone(), y.clone()));
            }
        }
    }
    (counts, diffs)
}

#[cfg(test)]
mod flag_tests {
    use super::compare_flags;
    use serde_json::json;

    /// A unit that is dead in one dump and not in the other, or out of the game, shows.
    #[test]
    fn flag_differences_show() {
        let unit = |dead: bool, conscious: bool, in_game: bool| json!({"units": [{"id": "u", "flags": {"is_dead": dead, "is_conscious": conscious, "in_game": in_game}}]});
        let oracle = unit(true, false, false);
        let (c, d) = compare_flags(&unit(true, false, false), &oracle);
        assert!(d.is_empty());
        assert_eq!(c["is_dead"], (1, 1));
        let (c, d) = compare_flags(&unit(false, true, true), &oracle);
        assert_eq!(d.len(), 3);
        assert_eq!((c["in_game"].0, c["in_game"].1), (0, 1));
        // A dump without the flag (an older one) isn't counted.
        let (c, _) = compare_flags(&json!({"units": [{"id": "u", "flags": {}}]}), &oracle);
        assert!(c.is_empty());
    }
}
