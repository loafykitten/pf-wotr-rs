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
        ("bounds_tick", "pos") if rec["made"] == true => Some(
            "taken: the making call's transform (the view's, then set to the entity's position)",
        ),
        ("bounds_set", "pos") if f == "entity" && rec["e0"].is_null() => {
            Some("taken: the view's transform when the link is written, before the call sets it")
        }
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

#[test]
fn every_field_of_every_family_is_constrained_or_named_as_taken() {
    let base = numbered(valid());
    let mut perturbed = 0;
    let mut silent: Vec<String> = Vec::new();
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
                if reports(&recs, &DECLARED).is_empty() && exempt(rec, field).is_none() {
                    silent.push(format!("#{i} {} {:?} {field}: {how}", rec["ev"], rec["fn"]));
                }
            }
        }
        // A call or write that threw.
        if matches!(rec["ev"].as_str(), Some("bounds_tick" | "bounds_set")) {
            let mut recs = base.clone();
            recs[i]["x"] = json!("NullReferenceException");
            perturbed += 1;
            if reports(&recs, &DECLARED).is_empty() {
                silent.push(format!("{} {:?} x", rec["ev"], rec["fn"]));
            }
        }
    }
    assert!(perturbed > 400, "{perturbed}");
    assert!(silent.is_empty(), "{} fields no check reports:\n{}", silent.len(), silent.join("\n"));
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
        }
    }
}
