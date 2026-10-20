//! The T0 projection a combat replay is compared on, and the diff.
//!
//! WrathOracle's trace has far more detail than the sim reproduces record for record (field
//! snapshots of every rule, ticks, presentation draws). Both sides are projected to the same
//! T0 vocabulary, in order:
//!
//! | record | from the oracle | fields compared |
//! |---|---|---|
//! | `rule` | `rule` | class (short name), initiator, target, depth |
//! | `result` | a rule's `about`/`trig`/`h_end`/`trig_end`/`did`/`rule_end` changes, at `rule_end` | the rule class's result fields ([`RESULT_FIELDS`]) |
//! | `bonuses` | a rule's `rule_end` bonuses and its `rule_bonus` additions, when it has any | class, each bonus's and addition's value, descriptor, type, stat and source fact |
//! | `draw` | `draw` made inside a rule (`rule != 0`), sim class | call-site key, value |
//! | `mut` | `mut` of kind `damage`, `xp`, `fact_attach`, `fact_detach` | kind, unit, values, fact blueprint |
//! | `mut` | `mut` of kind `stat_value`, `stat_mod_add`, `stat_mod_remove`, `stat_temp_scope_add`, `stat_temp_scope_remove` | [`stat_record`] |
//! | `tb` | `tb` (turn-based controller) | event, round, unit; for orders, the unit ids and initiatives |
//! | `cmd` | `cmd` `start`/`end` | event, command class, unit, target, result |
//! | `input` | `input` | the step text (positions left out) |
//!
//! Draws outside rules are timing-coupled (T1) and are compared per call site separately
//! (the tape check), not in this stream. Time values are left out (T1).

use serde_json::{Value as J, json};

use super::oracle::{Obj, Rv, Trace};

/// Result fields compared per rule class (the rest of a rule's snapshot is its inputs or
/// derived views of the same values).
pub const RESULT_FIELDS: &[(&str, &[&str])] = &[
    ("RuleRollD20", &["Result"]),
    ("RuleRollDice", &["Result"]),
    ("RuleInitiativeRoll", &["Result", "Modifier"]),
    (
        "RuleAttackRoll",
        &[
            "Result",
            "D20",
            "AttackBonus",
            "TargetAC",
            "IsTargetFlatFooted",
            "Roll",
            "IsCriticalRoll",
            "IsCriticalConfirmed",
            "IsHit",
        ],
    ),
    ("RuleCalculateAC", &["Result", "IsTargetFlatFooted"]),
    ("RuleCalculateAttackBonus", &["Result"]),
    ("RuleCalculateAttackBonusWithoutTarget", &["Result"]),
    ("RuleCheckTargetFlatFooted", &["IsFlatFooted"]),
    ("RuleConcealmentCheck", &["Concealment", "Success"]),
    ("RuleCalculateAttacksCount", &["Result"]),
    ("RuleCalculateAttacksCountLimbs", &["Slots", "AttacksCounts"]),
    ("RuleDealDamage", &["Result"]),
    ("RuleCalculateDamage", &[]),
    ("RuleCanApplyBuff", &["CanApply", "Immunity"]),
    ("RuleCalculateAbilityParams", &["Result"]),
    // Its inputs: the oracle writes its target as `$impure` (`GetRuleTarget`).
    ("RuleApplySpell", &["Spell", "Context", "SpellTarget", "Place"]),
    (
        "RuleCastSpell",
        &[
            "Success",
            "SpellFailureChance",
            "ArcaneSpellFailureChance",
            "IsArcaneSpellFailed",
            "IsSpellFailed",
        ],
    ),
    (
        "RuleSpellResistanceCheck",
        &["TargetIsImmune", "SpellResistance", "SpellPenetration", "Roll"],
    ),
];

fn short(class: &str) -> &str {
    class.rsplit(['.', '+']).next().unwrap_or(class)
}

fn unit_id(v: &Rv) -> J {
    v.get("$unit").map_or(J::Null, Rv::to_json)
}

fn result_fields(class: &str) -> Option<&'static [&'static str]> {
    RESULT_FIELDS.iter().find(|(c, _)| *c == class).map(|(_, f)| *f)
}

/// A field value reduced to what is compared: a nested rule reference becomes its result, a
/// list of weapon slots (`RuleCalculateAttacksCountLimbs.Slots`) their `ParentFact`s' ids (a
/// limb's weapon is created again at each load, with a fresh id).
fn reduce(v: &J) -> J {
    match v {
        J::Object(o) if o.contains_key("$rule") => o.get("result").cloned().unwrap_or(J::Null),
        J::Array(a) if a.iter().any(|x| x.get("m_ItemRef").is_some()) => {
            J::Array(a.iter().map(|x| x["ParentFact"]["FactId"].clone()).collect())
        }
        _ => v.clone(),
    }
}

/// Whether `what` is one of a stat's mutation records (`stats::trace`).
#[must_use]
pub fn is_stat_mut(what: &str) -> bool {
    matches!(
        what,
        "stat_value"
            | "stat_mod_add"
            | "stat_mod_remove"
            | "stat_temp_scope_add"
            | "stat_temp_scope_remove"
    )
}

/// A stat modifier as compared: its descriptor, value and stack mode, the fact that added it (its
/// unique id and blueprint), the component, the item's id and blueprint.
fn stat_mod(m: &J) -> J {
    if m.is_null() {
        return J::Null;
    }
    json!([
        m["descriptor"],
        m["value"],
        m["stack_mode"],
        m["stacks"],
        m["source"],
        m["source_component"],
        m["item"]
    ])
}

/// A stat mutation record (`r`: a recorded or a simulated one) as compared: its frame, the stat and its
/// owner, then the value change (`old`, `new`, `permanent`), or the modifier and the stat's value
/// after it (`modified`), or a scope's modifiers. `None` for a record that is none of them.
#[must_use]
pub fn stat_record(r: &J) -> Option<J> {
    let what = r["what"].as_str().filter(|w| is_stat_mut(w))?;
    let mut o =
        json!({"k": "mut", "what": what, "t": r["t"], "stat": r["stat"], "owner": r["owner"]});
    match what {
        "stat_value" => {
            for f in ["old", "new", "permanent"] {
                o[f] = r[f].clone();
            }
        }
        "stat_mod_add" | "stat_mod_remove" => {
            o["mod"] = stat_mod(&r["mod"]);
            o["modified"] = r["modified"].clone();
        }
        _ => {
            o["mods"] =
                J::Array(r["mods"].as_array().into_iter().flatten().map(stat_mod).collect());
        }
    }
    Some(o)
}

/// Projects an oracle trace.
#[must_use]
pub fn project_oracle(t: &Trace) -> Vec<J> {
    project_records(t, &t.records)
}

/// Projects a slice of an oracle trace's records (`t` supplies the call-site table).
#[must_use]
pub fn project_records<'r>(t: &Trace, records: impl IntoIterator<Item = &'r Rv>) -> Vec<J> {
    let mut out = Vec::new();
    // Per open rule id: (class, accumulated changes).
    let mut open: std::collections::HashMap<i64, (String, serde_json::Map<String, J>)> =
        std::collections::HashMap::new();
    // Per open rule id: the additions its `rule_bonus` mutations asked for, in order.
    let mut adds: std::collections::HashMap<i64, Vec<J>> = std::collections::HashMap::new();
    let fact_id = |f: &Rv| f.get("$fact").map_or(J::Null, Rv::to_json);
    // The last rule id allocated (WrathOracle numbers rules in push order): an addition's place is
    // the rules pushed after its rule before it.
    let mut last_rule = 0;
    for r in records {
        let k = r["k"].as_str().unwrap_or("");
        match k {
            "rule" => {
                let class = short(r["class"].as_str().unwrap_or("")).to_owned();
                out.push(
                    json!({"k": "rule", "class": class, "initiator": unit_id(&r["initiator"]),
                                "target": unit_id(&r["target"]), "depth": r["depth"]}),
                );
                // Results start from the pushed snapshot: fields a rule never changes keep
                // their initial values.
                let init = r["fields"].as_object().map(Obj::to_json).unwrap_or_default();
                last_rule = last_rule.max(r["id"].as_i64().unwrap_or(0));
                open.insert(r["id"].as_i64().unwrap_or(-1), (class, init));
            }
            // The oracle advances a rule's running snapshot after each of these (`about`,
            // `trig`, `trig_end`, `did`, `rule_end`: `WriteDiff`; `h_end`: `Rules.cs`
            // `HandlerPostfix` sets `Last`), so a change is reported once, by the record that
            // saw it, under the rule's id (`rule` for a handler).
            "about" | "trig" | "h_end" | "trig_end" | "did" | "rule_end" => {
                let id = r[if k == "h_end" { "rule" } else { "id" }].as_i64().unwrap_or(-1);
                if let Some((_, acc)) = open.get_mut(&id) {
                    for c in r["changes"].as_array().into_iter().flatten() {
                        if let (Some(name), Some(v)) = (c[0].as_str(), c.get(1)) {
                            acc.insert(name.to_owned(), v.to_json());
                        }
                    }
                }
                if k == "rule_end"
                    && let Some((class, acc)) = open.remove(&id)
                {
                    if let Some(fields) = result_fields(&class) {
                        let mut res = serde_json::Map::new();
                        for f in fields {
                            res.insert((*f).to_owned(), acc.get(*f).map_or(J::Null, reduce));
                        }
                        out.push(json!({"k": "result", "class": class, "result": res}));
                    }
                    // Its bonuses (`AllBonuses` at its end) and the additions asked for, when it
                    // has any: value, descriptor, type, stat, source fact.
                    let bonuses: Vec<J> = r["bonuses"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|b| {
                            json!([
                                b["value"],
                                b["descriptor"],
                                b["type"],
                                b["stat"],
                                fact_id(&b["fact"])
                            ])
                        })
                        .collect();
                    let asked = adds.remove(&id).unwrap_or_default();
                    if !bonuses.is_empty() || !asked.is_empty() {
                        out.push(
                            json!({"k": "bonuses", "class": class, "list": bonuses, "adds": asked}),
                        );
                    }
                }
            }
            "draw" => {
                if r["c"] == "pres" || r["rule"].as_i64().unwrap_or(0) == 0 {
                    continue;
                }
                let int = r["m"] == "Range_ii";
                let v = r["v"].as_f64().map_or(J::Null, |v| super::fire::draw_value(v, int));
                out.push(json!({"k": "draw", "site": t.site_key(r), "v": v}));
            }
            "mut" => {
                let what = r["what"].as_str().unwrap_or("");
                match what {
                    "damage" => out.push(json!({"k": "mut", "what": what, "unit": r["unit"], "old": r["old"], "new": r["new"]})),
                    "xp" => out.push(json!({"k": "mut", "what": what, "unit": r["unit"], "gained": r["gained"], "total": r["total"]})),
                                        "fact_attach" | "fact_detach" => out.push(json!({"k": "mut", "what": what, "owner": r["owner"],
                        "blueprint": r["fact"]["blueprint"], "class": r["class"]})),
                    // An addition to a rule's bonuses (`RuleBonusPostfix`), for its rule's
                    // `bonuses` record; one to an untracked rule (`target_rule` 0) is left out.
                    w if is_stat_mut(w) => out.extend(stat_record(&r.to_json())),
                    "rule_bonus" => {
                        if let Some(t) = r["target_rule"].as_i64().filter(|t| *t > 0) {
                                                        adds.entry(t).or_default().push(json!([r["value"], r["descriptor"], r["type"],
                                r["stat_type"], fact_id(&r["fact"]), last_rule - t]));
                        }
                    }
                    _ => {}
                }
            }
            "tb" => {
                let units: Option<Vec<J>> = r["units"].as_array().map(|a| {
                    a.iter()
                        .map(|u| json!([u["unit"], u["initiative"], u["override"], u["sequence"]]))
                        .collect()
                });
                let mut o = json!({"k": "tb", "ev": r["ev"], "round": r["round"]});
                if let Some(u) = r.get("unit") {
                    o["unit"] = unit_id(u);
                }
                if let Some(u) = units {
                    o["units"] = J::Array(u);
                }
                out.push(o);
            }
            "cmd" => {
                let ev = r["ev"].as_str().unwrap_or("");
                if ev == "run" {
                    continue;
                }
                let mut o = json!({"k": "cmd", "ev": ev, "class": r["class"], "unit": unit_id(&r["unit"]), "target": unit_id(&r["target"])});
                if ev == "end" {
                    o["result"] = r["result"].to_json();
                }
                out.push(o);
            }
            "input" => {
                let step = r["step"].as_str().unwrap_or("");
                let mut o = json!({"k": "input", "step": step});
                for key in ["unit", "target"] {
                    if let Some(u) = r.get(key) {
                        o[key] = unit_id(u);
                    }
                }
                out.push(o);
            }
            _ => {}
        }
    }
    out
}

/// The diff of two projected streams.
#[derive(Debug, Default, serde::Serialize)]
pub struct Diff {
    pub equal: bool,
    pub records: [usize; 2],
    /// First differing position, with context from both sides.
    pub first_difference: Option<usize>,
    pub context: Vec<(Option<J>, Option<J>)>,
    /// Records per kind on each side.
    pub kinds: std::collections::BTreeMap<String, [usize; 2]>,
}

/// Compares two projected streams position by position.
#[must_use]
pub fn diff(oracle: &[J], sim: &[J]) -> Diff {
    let mut d = Diff { records: [oracle.len(), sim.len()], ..Diff::default() };
    for (i, side) in [oracle, sim].iter().enumerate() {
        for r in *side {
            d.kinds.entry(r["k"].as_str().unwrap_or("?").to_owned()).or_default()[i] += 1;
        }
    }
    let n = oracle.len().min(sim.len());
    let first = (0..n).find(|&i| oracle[i] != sim[i]).or((oracle.len() != sim.len()).then_some(n));
    d.equal = first.is_none();
    if let Some(f) = first {
        d.first_difference = Some(f);
        let lo = f.saturating_sub(5);
        for i in lo..(f + 5) {
            let a = oracle.get(i).cloned();
            let b = sim.get(i).cloned();
            if a.is_none() && b.is_none() {
                break;
            }
            d.context.push((a, b));
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stat mutation projects to the same record from the oracle's form and the sim's: the
    /// descriptor's name and the fields the oracle adds are not compared; a changed value, modifier,
    /// source fact (its id too), frame or the other kind is.
    #[test]
    fn stat_mutations_project_alike_from_both_sides() {
        let rec = json!({"seq": 9, "k": "mut", "t": 4, "what": "stat_mod_add", "rule": 0, "h": 0,
            "stat": 11, "owner": "51DD", "modified": 4,
            "mod": {"descriptor": 40, "descriptor_name": "Helpless", "value": -9, "stack_mode": 0,
                "stacks": false, "source": {"$fact": "1", "blueprint": "ab"},
                "source_component": "$c", "item": null}});
        let t = Trace {
            records: [
                rec.clone(),
                json!({"k": "mut", "t": 4, "what": "stat_value", "stat": 11,
                "owner": "51DD", "old": 13, "new": 4, "permanent": 10, "rule": 0, "h": 0}),
            ]
            .iter()
            .map(Rv::from_json)
            .collect(),
            sites: Default::default(),
        };
        let out = project_oracle(&t);
        assert_eq!(out.len(), 2);
        let mut sim = rec.clone();
        sim["mod"]["descriptor_name"] = json!("x");
        sim["seq"] = json!(0);
        assert_eq!(stat_record(&sim), Some(out[0].clone()));
        for (path, v) in
            [("t", json!(5)), ("modified", json!(5)), ("stat", json!(12)), ("owner", json!("x"))]
        {
            let mut other = rec.clone();
            other[path] = v;
            assert_ne!(stat_record(&other), Some(out[0].clone()), "{path}");
        }
        let mut other = rec.clone();
        other["mod"]["source"]["$fact"] = json!("2");
        assert_ne!(stat_record(&other), Some(out[0].clone()), "the source fact's id");
        other = rec.clone();
        other["mod"]["value"] = json!(-8);
        assert_ne!(stat_record(&other), Some(out[0].clone()));
        other = rec.clone();
        other["what"] = json!("stat_mod_remove");
        assert_ne!(stat_record(&other), Some(out[0].clone()));
        assert_eq!(stat_record(&json!({"what": "damage"})), None);
    }

    /// A handler's `h_end` changes (`rule` = the rule id) reach the rule's result, as do the
    /// `about` and `trig` changes: the oracle's running snapshot already includes them, so
    /// no later record repeats them.
    #[test]
    fn handler_about_and_trig_changes_reach_the_result() {
        let t = Trace {
            records: [
                json!({"k": "rule", "id": 7, "class": "Kingmaker.RuleSystem.Rules.RuleCanApplyBuff",
                    "initiator": null, "target": null, "depth": 0,
                    "fields": {"CanApply": true, "Immunity": false}}),
                json!({"k": "about", "id": 7, "changes": [["CanApply", false]]}),
                json!({"k": "h_end", "id": 1, "rule": 7, "changes": [["Immunity", true]]}),
                json!({"k": "trig", "id": 7, "changes": [["CanApply", true]]}),
                json!({"k": "rule_end", "id": 7, "changes": []}),
            ]
            .iter()
            .map(Rv::from_json)
            .collect(),
            sites: Default::default(),
        };
        let out = project_oracle(&t);
        assert_eq!(out[1]["result"], json!({"CanApply": true, "Immunity": true}));
    }
}
