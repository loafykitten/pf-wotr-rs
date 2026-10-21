//! The table of every record family and every field it carries (docs/poc5/vision.md, "The
//! fields"), as a test: a small synthetic trace in the oracle's order, valid; then each field of
//! each record perturbed alone (another value of its type, null, removed, an `x` added), and a
//! report required from the parse and the lifecycle machine, but for the fields that are *taken*
//! or checked elsewhere, each named below with its reason. A field that nothing constrains is
//! therefore either in the exemptions with a reason, or a failing test.

use serde_json::{Value, json};

use super::*;

const DECLARED: [&str; 6] =
    ["bounds_init", "bounds_tick", "bounds_set", "bounds_scene", "phys_sync", "vis_order"];

const P: [f64; 3] = [1.0, 2.0, 3.0];
const P2: [f64; 3] = [1.5, 2.0, 3.0];
const P3: [f64; 3] = [2.5, 2.0, 3.0];
const P4: [f64; 3] = [3.5, 2.0, 3.0];
const Q: [f64; 3] = [9.0, 8.0, 7.0];

fn tick_rec(t: i64) -> Value {
    json!({"k": "tick", "t": t, "mode": 1})
}

fn init(i: i64, id: i64, e: &str, kind: &str, layer: i64, r: f64, pos: [f64; 3]) -> Value {
    json!({"k": "obs", "t": 0, "ev": "bounds_init", "i": i, "id": id, "layer": layer, "e": e,
        "kind": kind, "r": r, "on": true, "pos": pos, "fade": false})
}

#[allow(clippy::too_many_arguments)]
fn call(
    t: i64,
    e: &str,
    kind: &str,
    made: bool,
    r0: Option<f64>,
    r: f64,
    pos: [f64; 3],
    on: bool,
    c: i64,
) -> Value {
    json!({"k": "obs", "t": t, "ev": "bounds_tick", "e": e, "kind": kind, "made": made,
        "r0": r0, "r": r, "pos": pos, "on": on, "c": c})
}

#[allow(clippy::too_many_arguments)]
fn set(
    t: i64,
    f: &str,
    v: Option<&str>,
    c: Option<i64>,
    on0: Option<bool>,
    on: Option<bool>,
    pos0: Option<[f64; 3]>,
    pos: Option<[f64; 3]>,
    e0: Option<&str>,
    e: Option<&str>,
) -> Value {
    json!({"k": "obs", "t": t, "ev": "bounds_set", "fn": f, "v": v, "c": c, "on0": on0, "on": on,
        "pos0": pos0, "pos": pos, "e0": e0, "e": e})
}

const UNIT: &str = "UnitEntityData";
const OBJ: &str = "MapObjectEntityData";

/// A valid trace: every record family, in the oracle's order.
fn valid() -> Vec<Value> {
    let a = Some("a");
    vec![
        tick_rec(0),
        init(0, -1, "a", UNIT, 23, 0.5, P),
        init(1, -50, "o", OBJ, 24, 0.05, Q),
        json!({"k": "obs", "t": 0, "ev": "bounds_init_end", "n": 2, "valid": true}),
        call(0, "a", UNIT, false, Some(0.5), 0.5, P2, true, -1),
        call(0, "o", OBJ, false, Some(0.05), 0.05, Q, true, -50),
        tick_rec(1),
        set(1, "active", a, Some(-1), Some(true), Some(false), Some(P2), Some(P2), a, a),
        set(1, "pos", a, Some(-1), Some(false), Some(false), Some(P2), Some(P3), a, a),
        set(1, "link", a, Some(-1), Some(false), Some(false), Some(P3), Some(P3), a, a),
        set(1, "entity", None, Some(-1), None, Some(false), None, Some(P3), a, a),
        set(1, "link", Some("ghost"), None, None, None, None, None, None, None),
        json!({"k": "obs", "t": 1, "ev": "phys_sync", "src": "Polymorph.RestoreView"}),
        call(1, "a", UNIT, false, Some(0.5), 0.5, P3, false, -1),
        json!({"k": "obs", "t": 1, "ev": "vis_order", "u": "a", "v": ["a"]}),
        json!({"k": "obs", "t": 1, "ev": "vis", "u": "a", "add": ["a"], "rem": []}),
        tick_rec(2),
        set(2, "gone", a, Some(-1), Some(false), None, Some(P3), None, a, None),
        set(2, "entity", None, Some(-2), None, Some(true), None, Some(P4), None, a),
        call(2, "a", UNIT, true, None, 0.5, P4, true, -2),
        tick_rec(3),
        call(3, "a", UNIT, false, Some(0.5), 0.5, P4, true, -2),
        set(3, "entity", None, Some(-51), None, Some(true), None, Some(Q), None, Some("o2")),
        call(3, "o2", OBJ, true, None, 0.05, Q, true, -51),
    ]
}

fn numbered(mut recs: Vec<Value>) -> Vec<Value> {
    for (n, r) in recs.iter_mut().enumerate() {
        r["seq"] = json!(n);
    }
    recs
}

/// What the parse and the machine report for `recs`, run like the driver runs the records
/// (frames in stream order, each record's event applied at its place).
fn reports(recs: &[Value], declared: &[&str]) -> Vec<String> {
    let rv: Vec<Rv> = recs.iter().map(Rv::from_json).collect();
    let parsed = parse(&rv, declared);
    let mut out = parsed.problems.clone();
    let Some(rec) = parsed.rec else { return out };
    let mut life = Lifecycle::default();
    let mut roots = rec.roots.iter();
    let ticks: Vec<&TickRaw> = rec.ticks.values().flatten().collect();
    for r in &rv {
        if r["k"] == "tick" {
            life.frame(&mut out);
            continue;
        }
        let seq = r["seq"].as_i64();
        match r["ev"].as_str() {
            Some("bounds_init") => {
                if let Some(root) = roots.next() {
                    life.apply(root_event(root), &mut out);
                }
            }
            Some("bounds_tick") => {
                if let Some(t) = ticks.iter().find(|t| Some(t.seq) == seq) {
                    life.apply(tick_event(t), &mut out);
                }
            }
            Some("bounds_set" | "phys_sync") => {
                if let Some((_, s)) = seq.and_then(|s| rec.stream.get(&s)) {
                    life.apply(stream_event(s), &mut out);
                }
            }
            _ => {}
        }
    }
    life.frame(&mut out);
    out
}

#[test]
fn the_valid_trace_is_valid() {
    let out = reports(&numbered(valid()), &DECLARED);
    assert!(out.is_empty(), "{out:?}");
}

/// Why a field of a record is *not* reported when it changes alone: it is taken, or checked
/// where the sim's own state is (not in this trace).
fn exempt(rec: &Value, field: &str) -> Option<&'static str> {
    let ev = rec["ev"].as_str().unwrap_or("");
    let f = rec["fn"].as_str().unwrap_or("");
    let unit = rec["kind"] == UNIT;
    match (ev, field) {
        ("bounds_init", "fade") => Some("taken: the view's fade mark, marks the unit fade-hiding"),
        ("bounds_init", "pos") if unit => {
            Some("checked against the unit's position at the previous frame's end by the driver")
        }
        ("bounds_tick", "r" | "pos") if unit && rec["made"] == false => Some(
            "a unit's radius and transform after a call: the sim's rule (Bounds), then the next record's before-state",
        ),
        ("bounds_tick", "r") if rec["made"] == true && !unit => Some(
            "taken: a map object's radius is its bounds' extent, which no state of the sim has (the next call's radius before checks it)",
        ),
        ("bounds_tick", "kind") if rec["made"] == true => Some(
            "taken: the making call is the first record to name a new entity's type; later records are checked against it",
        ),
        ("bounds_tick", "pos") if rec["made"] == true && unit => Some(
            "a unit's making: the entity's position unless `==` the link's transform, the sim's rule (Bounds, with the link's)",
        ),
        ("bounds_set", "pos") if f == "entity" && rec["e0"].is_null() && rec["e"] == "a" => Some(
            "a unit's creation link: the view's transform, which the making call keeps or sets to the entity's position (Bounds)",
        ),
        ("bounds_set", "v") if f == "link" && rec["c"].is_null() => {
            Some("any entity without a view collider (a view attached or detached)")
        }
        ("vis_order", _) | ("vis", _) => {
            Some("checked against the vis set and the colliders by the driver")
        }
        _ => None,
    }
}

/// Another value of the same type.
fn other(v: &Value) -> Value {
    match v {
        Value::Null => json!("zz"),
        Value::Bool(b) => json!(!b),
        Value::Number(n) if n.is_i64() => json!(n.as_i64().unwrap_or(0) + 1),
        Value::Number(n) => json!(n.as_f64().unwrap_or(0.0) + 1.0),
        Value::String(s) => json!(format!("{s}x")),
        Value::Array(a) => {
            let mut a = a.clone();
            if let Some(x) = a.first_mut() {
                *x = other(x);
            }
            Value::Array(a)
        }
        Value::Object(_) => Value::Null,
    }
}

/// What the first report of a perturbed field must be about: a substring of the check that owns
/// the field (or of the earliest downstream check, for a field only later records can contradict),
/// so a perturbation caught by some other check fails the test. The type-breaking variants are the
/// parse's ("can't read").
fn reasons(rec: &Value, field: &str) -> Vec<String> {
    let ev = rec["ev"].as_str().unwrap_or("");
    let f = rec["fn"].as_str().unwrap_or("");
    let unit = rec["kind"] == UNIT;
    let made = rec["made"] == true;
    let create = f == "entity" && rec["e0"].is_null();
    let mut r: Vec<&str> = vec!["can't read"];
    match (ev, field) {
        (_, "t") => r.extend(["claims tick"]),
        ("bounds_init", "i") => r.extend(["out of order"]),
        ("bounds_init", "id") => r.extend(["no record made", "two bounds_init roots"]),
        ("bounds_init", "layer") => r.extend(["isn't a collider the game makes"]),
        ("bounds_init", "e") => r.extend(["isn't a collider the game makes", "bound to"]),
        ("bounds_init", "kind") => r.extend(["isn't a collider the game makes", "is a "]),
        ("bounds_init", "r") => r.extend(["isn't a collider the game makes", "radius before"]),
        ("bounds_init", "on") => {
            r.extend(["isn't a collider the game makes", "shows enabled", "`on0`"])
        }
        ("bounds_init", "pos") => r.extend(["not a unit's"]),
        ("bounds_init_end", "n") => r.extend(["counts"]),
        ("bounds_init_end", "valid") => r.extend(["not valid"]),
        ("bounds_tick", "e") => r.extend(["bound to", "no entity record linked", "is a "]),
        ("bounds_tick", "kind") => r.extend(["is a "]),
        ("bounds_tick", "made") => r.extend([
            "made collider",
            "no record made",
            "no entity record linked",
            "inside the making",
        ]),
        ("bounds_tick", "c") => r.extend([
            "no record made",
            "no entity record linked",
            "in part",
            "inside the making",
            "names collider",
        ]),
        ("bounds_tick", "r0") => r.extend(["radius before"]),
        ("bounds_tick", "r") => {
            r.extend(["changed its radius", "radius before", "under the 0.05", "in part"])
        }
        ("bounds_tick", "pos") if made && !unit => r.extend(["was linked at", "in part"]),
        ("bounds_tick", "pos") => r.extend(["changed its radius", "in part"]),
        ("bounds_tick", "on") => {
            r.extend(["shows enabled", "disabled", "was linked enabled", "in part"])
        }
        ("bounds_set", "c") if create => r.extend([
            "no entity record linked",
            "no call made it",
            "not modelled",
            "names no collider",
        ]),
        ("bounds_set", "e" | "e0") if f == "entity" && !create => r.extend(["not modelled"]),
        // A moved position is any other position: the next record's `pos0` is what shows it.
        ("bounds_set", "pos") if f == "pos" => r.extend(["`pos0`", "`pos`"]),
        ("bounds_set", "c") => r.extend([
            "no record made",
            "names no collider",
            "view collider",
            "made it",
            "with no collider",
            "no entity record linked",
            "no call made it",
        ]),
        ("bounds_set", "e") if create => {
            r.extend(["bound to", "no record made it", "no entity record linked"])
        }
        ("bounds_set", "e0") if create => r.extend(["no record made it"]),
        ("bounds_set", "on") if create => {
            r.extend(["linked disabled", "was linked enabled", "lacks"])
        }
        ("bounds_set", "pos") if create && !unit => r.extend(["was linked at", "lacks"]),
        ("bounds_set", "pos") if create => r.extend(["lacks"]),
        ("bounds_set", "v" | "e0" | "e" | "on0" | "on" | "pos0" | "pos") => {
            return [
                format!("`{field}`"),
                "can't read".into(),
                "lacks".into(),
                "no record made it".into(),
            ]
            .into();
        }
        ("phys_sync", "src") => {}
        _ => r.push("\u{0}unlisted"),
    }
    r.into_iter().map(str::to_owned).collect()
}

#[test]
fn every_field_of_every_family_is_constrained_for_its_reason_or_named_as_taken() {
    let base = numbered(valid());
    let mut perturbed = 0;
    let mut wrong: Vec<String> = Vec::new();
    for (i, rec) in base.iter().enumerate() {
        if rec["k"] != "obs" {
            continue;
        }
        let Some(obj) = rec.as_object() else { continue };
        for field in obj.keys().filter(|k| !["seq", "k", "ev"].contains(&k.as_str())) {
            let mut variants: Vec<(&str, Option<Value>)> =
                vec![("another value", Some(other(&rec[field]))), ("removed", None)];
            if !rec[field].is_null() {
                variants.push(("null", Some(Value::Null)));
            }
            for (how, v) in variants {
                let mut recs = base.clone();
                match v {
                    Some(v) => recs[i][field] = v,
                    None => {
                        recs[i].as_object_mut().map(|o| o.remove(field));
                    }
                }
                perturbed += 1;
                if exempt(rec, field).is_some() {
                    continue;
                }
                let out = reports(&recs, &DECLARED);
                let ok = out.first().is_some_and(|m| {
                    // (A removed field is the parse's.)
                    how == "removed" && m.contains("can't read")
                        || reasons(rec, field).iter().any(|r| m.contains(r.as_str()))
                });
                if !ok {
                    wrong.push(format!(
                        "#{i} {} {} {field} [{how}]: first report {:?}",
                        rec["ev"],
                        rec["fn"],
                        out.first().map(|m| m.chars().take(140).collect::<String>())
                    ));
                }
            }
        }
        // A call or write that threw.
        if matches!(rec["ev"].as_str(), Some("bounds_tick" | "bounds_set")) {
            let mut recs = base.clone();
            recs[i]["x"] = json!("NullReferenceException");
            perturbed += 1;
            let out = reports(&recs, &DECLARED);
            if !out.first().is_some_and(|m| m.contains("threw")) {
                wrong.push(format!(
                    "#{i} {} {} x: first report {:?}",
                    rec["ev"],
                    rec["fn"],
                    out.first()
                ));
            }
        }
    }
    assert!(perturbed > 400, "{perturbed}");
    assert!(
        wrong.is_empty(),
        "{} perturbations caught by another check or by none:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn the_phase_order_of_a_frame_is_checked_whichever_observers_are_declared() {
    // The bounds controller precedes the vision controller; the init block is written at the
    // frame's prefix, before the controllers; a making's link is inside its call.
    let base = numbered(valid());
    fn at(recs: &[Value], pred: &dyn Fn(&Value) -> bool) -> usize {
        recs.iter().position(pred).unwrap()
    }
    type Change = Box<dyn Fn(&mut Vec<Value>)>;
    let cases: Vec<(&str, Change)> = vec![
        (
            "a call after the frame's vis",
            Box::new(|recs| {
                let call = at(recs, &|r| r["ev"] == "bounds_tick" && r["t"] == 1);
                let vis = at(recs, &|r| r["ev"] == "vis");
                let r = recs.remove(call);
                recs.insert(vis, r);
            }),
        ),
        (
            "a vis before the init block",
            Box::new(|recs| {
                recs.insert(
                    1,
                    json!({"k": "obs", "t": 0, "ev": "vis", "u": "a", "add": [], "rem": []}),
                );
            }),
        ),
        (
            "the init block after a call",
            Box::new(|recs| {
                let end = at(recs, &|r| r["ev"] == "bounds_init_end");
                let call = recs.remove(at(recs, &|r| r["ev"] == "bounds_tick" && r["t"] == 0));
                recs.insert(end - 1, call);
            }),
        ),
        (
            "a call before the end of the init block",
            Box::new(|recs| {
                let call = recs.remove(at(recs, &|r| r["ev"] == "bounds_tick" && r["t"] == 0));
                let end = at(recs, &|r| r["ev"] == "bounds_init_end");
                recs.insert(end, call);
            }),
        ),
        (
            "a setter inside the init block (otherwise valid)",
            Box::new(|recs| {
                let end = at(recs, &|r| r["ev"] == "bounds_init_end");
                recs.insert(
                    end,
                    set(
                        0,
                        "link",
                        Some("a"),
                        Some(-1),
                        Some(true),
                        Some(true),
                        Some(P),
                        Some(P),
                        Some("a"),
                        Some("a"),
                    ),
                );
            }),
        ),
        (
            "a sync inside the init block",
            Box::new(|recs| {
                let end = at(recs, &|r| r["ev"] == "bounds_init_end");
                recs.insert(
                    end,
                    json!({"k": "obs", "t": 0, "ev": "phys_sync", "src": "DollRoom.Update"}),
                );
            }),
        ),
        (
            "another call inside a making",
            Box::new(|recs| {
                let link = at(recs, &|r| r["fn"] == "entity" && r["c"] == -51);
                recs.insert(link + 1, call(3, "b", UNIT, false, None, 0.5, P, true, -9));
            }),
        ),
        (
            "a sync inside a making",
            Box::new(|recs| {
                let link = at(recs, &|r| r["fn"] == "entity" && r["c"] == -51);
                recs.insert(
                    link + 1,
                    json!({"k": "obs", "t": 3, "ev": "phys_sync", "src": "DollRoom.Update"}),
                );
            }),
        ),
        (
            "two makings interleaved",
            Box::new(|recs| {
                let link = at(recs, &|r| r["fn"] == "entity" && r["c"] == -51);
                recs.insert(
                    link + 1,
                    set(
                        3,
                        "entity",
                        None,
                        Some(-52),
                        None,
                        Some(true),
                        None,
                        Some(Q),
                        None,
                        Some("o3"),
                    ),
                );
            }),
        ),
        (
            "a call across a frame boundary with its t kept",
            Box::new(|recs| {
                let call = recs.remove(at(recs, &|r| r["ev"] == "bounds_tick" && r["t"] == 2));
                let next = at(recs, &|r| r["k"] == "tick" && r["t"] == 3);
                recs.insert(next + 1, call);
            }),
        ),
    ];
    for declared in [&DECLARED[..5], &DECLARED[..]] {
        // (Without `vis_order` declared there are none of its records, so the cases are
        // reported for their placement alone.)
        let mut base = base.clone();
        if declared.len() == 5 {
            base.retain(|r| r["ev"] != "vis_order");
        }
        let ok = reports(&numbered(base.clone()), declared);
        assert!(ok.is_empty(), "the valid trace ({} declared): {ok:?}", declared.len());
        for (name, change) in &cases {
            let mut recs = base.clone();
            change(&mut recs);
            let recs = numbered(recs);
            let out = reports(&recs, declared);
            assert!(!out.is_empty(), "{name} (declared {}): not reported", declared.len());
            // (Each case is otherwise valid: what is reported is its place.)
            let placed = [
                "claims tick",
                "before bounds_init_end",
                "bounds_init block",
                "after the frame's vis",
                "inside the making",
                "before the end of",
            ];
            assert!(
                out.iter().any(|m| placed.iter().any(|p| m.contains(p))),
                "{name} (declared {}): reported for another reason: {out:?}",
                declared.len()
            );
        }
    }
}
