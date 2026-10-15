//! The comparers' recorded identity-hash decisions (`super`).

use serde_json::{Value as J, json};

use super::*;
use crate::combat::turns::tests::{in_combat, reported, unit, world};
use crate::combat::turns::{TbEvent, sort_units};

/// Two units that tie in everything the comparers read before the hashes, in the turn order
/// `[a, b]`, the port on, no decisions loaded yet.
fn tied() -> (World, EntityIdx, EntityIdx) {
    let mut w = world();
    let a = unit(&mut w, "a", None, 20);
    let b = unit(&mut w, "b", None, 10);
    in_combat(&mut w, &[a, b]);
    w.unit_mut(b).expect("b").combat.initiative.initiative = 20;
    w.combat.turns.log.clear();
    w.coverage.unsupported.clear();
    (w, a, b)
}

fn tie(t: i64, cmp: &str, i: u64, a: &str, b: &str, ha: i64, hb: i64) -> J {
    let r = if ha >= hb { -1 } else { 1 };
    json!({"k": "obs", "t": t, "ev": "init_tie", "cmp": cmp, "i": i, "a": a, "b": b, "ha": ha, "hb": hb, "r": r})
}

fn last_order(w: &World) -> Vec<String> {
    match w.combat.turns.log.back() {
        Some(TbEvent::Sort { units, .. }) => {
            units.iter().map(|u| w.entity(u.0).unique_id.clone()).collect()
        }
        e => panic!("no sort: {e:?}"),
    }
}

fn loaded(w: &mut World, recs: &[J]) {
    load(w, recs, true, true);
    frame_start(w, 5);
}

/// `OrderBy` on two units compares them once (`Array.Sort`'s two-element case): the recorded
/// result decides, either way, and the decision is used up.
#[test]
fn the_recorded_decision_orders_a_tie() {
    for (ha, hb, order) in [(1, 2, ["b", "a"]), (2, 1, ["a", "b"])] {
        let (mut w, _, _) = tied();
        loaded(&mut w, &[tie(5, "turn", 0, "a", "b", ha, hb)]);
        sort_units(&mut w, "x");
        assert_eq!(last_order(&w), order);
        frame_end(&mut w);
        assert_eq!(finish(&mut w), 1);
        assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
    }
}

/// Without the observer the fallback is reported (older recordings), and its order is the
/// stable one.
#[test]
fn without_the_observer_a_fallback_is_reported() {
    let (mut w, _, _) = tied();
    sort_units(&mut w, "x");
    assert_eq!(last_order(&w), ["a", "b"]);
    assert!(reported(
        &w,
        "CombatController.SortUnits: the comparer's identity-hash fallback (object identity)"
    ));
}

/// A fallback whose decision isn't the frame's next record (another pair, another comparer, or
/// none at all) is reported.
#[test]
fn a_fallback_without_its_decision_is_reported() {
    for recs in [
        vec![],
        vec![tie(5, "turn", 0, "b", "a", 1, 2)],
        vec![tie(5, "prepare", 0, "a", "b", 1, 2)],
        vec![tie(6, "turn", 0, "a", "b", 1, 2)],
    ] {
        let (mut w, _, _) = tied();
        loaded(&mut w, &recs);
        sort_units(&mut w, "x");
        assert!(
            reported(
                &w,
                "CombatController.SortUnits: the comparer's identity-hash fallback without its recorded decision"
            ),
            "{recs:?}"
        );
    }
}

/// Decisions the sim didn't need are reported: one continuing a sort call (at the call's end),
/// one left in the frame (at its end), one of a tick never run (at the next frame or the end).
#[test]
fn decisions_the_sim_didnt_need_are_reported() {
    let (mut w, _, _) = tied();
    loaded(&mut w, &[tie(5, "turn", 0, "a", "b", 1, 2), tie(5, "turn", 1, "a", "b", 1, 2)]);
    sort_units(&mut w, "x");
    assert!(reported(
        &w,
        "CombatController.SortUnits: a recorded identity-hash decision the sort didn't need"
    ));

    let (mut w, _, _) = tied();
    loaded(&mut w, &[tie(5, "turn", 0, "a", "b", 1, 2)]);
    frame_end(&mut w);
    assert!(reported(&w, "driven replay: a recorded identity-hash decision the sim didn't need"));

    let (mut w, _, _) = tied();
    load(&mut w, &[tie(3, "turn", 0, "a", "b", 1, 2)], true, true);
    frame_start(&mut w, 5);
    assert!(reported(
        &w,
        "driven replay: a recorded identity-hash decision at a tick the sim didn't run"
    ));

    let (mut w, _, _) = tied();
    load(&mut w, &[tie(9, "turn", 0, "a", "b", 1, 2)], true, true);
    frame_start(&mut w, 5);
    frame_end(&mut w);
    assert!(w.coverage.unsupported.is_empty());
    finish(&mut w);
    assert!(reported(
        &w,
        "driven replay: a recorded identity-hash decision at a tick the sim didn't run"
    ));
}

/// `UnitEntityOrderComaprer` (`UnitCombatPrepareController`'s `List.Sort`) takes its decisions
/// too: the recorded one decides the cooldowns' order.
#[test]
fn the_prepare_sort_takes_its_decision() {
    let (mut w, a, b) = tied();
    for u in [a, b] {
        w.combat.turns.prepared.push((u, 20));
    }
    // The prepare sort's decision, then each handler's `SortUnits` (the second compares the
    // first's order, `[b, a]`).
    loaded(
        &mut w,
        &[
            tie(5, "prepare", 0, "a", "b", 1, 2),
            tie(5, "turn", 0, "a", "b", 1, 2),
            tie(5, "turn", 0, "b", "a", 2, 1),
        ],
    );
    crate::combat::initiative::prepare_flush(&mut w);
    let cd = |u| w.unit(u).expect("u").combat.cooldown.initiative;
    assert_eq!((cd(b), cd(a)), (0.0, 6.0), "b first: a's hash is the lower");
    frame_end(&mut w);
    assert_eq!(finish(&mut w), 3);
    assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
}

/// Damaged or inconsistent records are reported at load: a result the hashes don't give, a
/// missing or mistyped field, the same unit with two hashes, an index that doesn't follow its
/// call's previous decision, a decision the oracle couldn't record, records without the
/// observer declared, the observer without the turn port.
#[test]
fn damaged_records_are_reported() {
    let ok = tie(5, "turn", 0, "a", "b", 1, 2);
    let with = |k: &str, v: J| {
        let mut r = ok.clone();
        r[k] = v;
        r
    };
    let mut no_field = ok.clone();
    no_field.as_object_mut().expect("obj").remove("hb");
    for r in [
        with("r", json!(-1)),
        with("r", json!(0)),
        with("cmp", json!("other")),
        with("i", json!(-1)),
        with("a", json!(null)),
        with("b", json!("a")),
        with("ha", json!(1u64 << 40)),
        with("t", json!(-1)),
        no_field,
    ] {
        let mut w = world();
        load(&mut w, std::slice::from_ref(&r), true, true);
        assert!(reported(&w, "driven replay: an init_tie record it can't read"), "{r}");
    }
    let mut w = world();
    load(&mut w, &[ok.clone(), tie(5, "turn", 0, "a", "c", 3, 4)], true, true);
    assert!(reported(
        &w,
        "driven replay: a unit's identity hash differs between its init_tie records"
    ));
    for later in [
        tie(5, "turn", 2, "a", "b", 1, 2),
        tie(6, "turn", 1, "a", "b", 1, 2),
        tie(5, "prepare", 1, "a", "b", 1, 2),
    ] {
        let mut w = world();
        load(&mut w, &[ok.clone(), later.clone()], true, true);
        assert!(
            reported(
                &w,
                "driven replay: an init_tie record that doesn't follow its sort call's previous one"
            ),
            "{later}"
        );
    }
    let mut w = world();
    load(
        &mut w,
        &[json!({"k": "obs", "t": 5, "ev": "init_tie_bad", "cmp": "turn", "why": "x"})],
        true,
        true,
    );
    assert!(reported(&w, "driven replay: an identity-hash decision WrathOracle couldn't record"));
    let mut w = world();
    load(&mut w, std::slice::from_ref(&ok), false, true);
    assert!(reported(
        &w,
        "driven replay: init_tie records in a recording that doesn't declare init_ties"
    ));
    assert!(w.combat.turns.ties.is_none());
    let mut w = world();
    load(&mut w, &[ok], true, false);
    assert!(reported(&w, "driven replay: the recording declares its init_ties observer without"));
    assert!(w.combat.turns.ties.is_none());
}
