//! The comparers' recorded identity-hash decisions (`super`).

use serde_json::{Value as J, json};

use super::*;
use crate::combat::turns::tests::{in_combat, reported, unit, world};
use crate::combat::turns::{TbEvent, sort_units};

/// [`super::load`] of records written as `serde_json::Value`s.
fn load(w: &mut World, recs: &[J], declared: bool, port: bool) {
    super::load(w, &recs.iter().map(Rv::from_json).collect::<Vec<_>>(), declared, port);
}

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

/// A frame's `tick` record.
fn frame(t: i64) -> J {
    json!({"k": "tick", "t": t})
}

/// A recorded `SortUnits` (its postfix's `sort`).
fn sort(t: i64) -> J {
    json!({"k": "tb", "t": t, "ev": "sort"})
}

fn last_order(w: &World) -> Vec<String> {
    match w.combat.turns.log.back() {
        Some(TbEvent::Sort { units, .. }) => {
            units.iter().map(|u| w.entity(u.0).unique_id.clone()).collect()
        }
        e => panic!("no sort: {e:?}"),
    }
}

/// The records loaded, frame 5 started.
fn loaded(w: &mut World, recs: &[J]) {
    load(w, recs, true, true);
    frame_start(w, 5);
}

fn clean(w: &World) {
    assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
}

/// `OrderBy` on two units compares them once (`Array.Sort`'s two-element case): the recorded
/// result decides, either way, and the decision is used up.
#[test]
fn the_recorded_decision_orders_a_tie() {
    for (ha, hb, order) in [(1, 2, ["b", "a"]), (2, 1, ["a", "b"])] {
        let (mut w, _, _) = tied();
        loaded(&mut w, &[frame(5), tie(5, "turn", 0, "a", "b", ha, hb), sort(5)]);
        sort_units(&mut w, "x");
        assert_eq!(last_order(&w), order);
        frame_end(&mut w);
        assert_eq!(finish(&mut w), 1);
        clean(&w);
    }
}

/// Three tied units: `Array.Sort`'s three-element case compares map positions (0, 1), (0, 2),
/// (1, 2), swapping as it goes, so a call takes three decisions, indices 0 to 2, on the pairs the
/// map holds then. A call without a fallback before it takes none of its decisions.
#[test]
fn a_call_takes_its_decisions_in_order() {
    let mut w = world();
    let a = unit(&mut w, "a", None, 30);
    let b = unit(&mut w, "b", None, 20);
    let c = unit(&mut w, "c", None, 10);
    in_combat(&mut w, &[a, b, c]);
    w.combat.turns.log.clear();
    w.coverage.unsupported.clear();
    loaded(
        &mut w,
        &[
            frame(5),
            sort(5),
            // [a, b, c]: (a, b) → [b, a, c]; (b, c) → [c, a, b]; (a, b) → [c, b, a].
            tie(5, "turn", 0, "a", "b", 1, 2),
            tie(5, "turn", 1, "b", "c", 2, 3),
            tie(5, "turn", 2, "a", "b", 1, 2),
            sort(5),
        ],
    );
    // The first call: no tie, no decision.
    sort_units(&mut w, "x");
    assert_eq!(last_order(&w), ["a", "b", "c"]);
    for u in [b, c] {
        w.unit_mut(u).expect("u").combat.initiative.initiative = 30;
    }
    sort_units(&mut w, "x");
    assert_eq!(last_order(&w), ["c", "b", "a"]);
    frame_end(&mut w);
    assert_eq!(finish(&mut w), 3);
    clean(&w);
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

/// A fallback whose decision isn't its call's next record (another pair, another comparer, a
/// later call's, or none at all) is reported.
#[test]
fn a_fallback_without_its_decision_is_reported() {
    for recs in [
        vec![frame(5), sort(5)],
        vec![frame(5), tie(5, "turn", 0, "b", "a", 1, 2), sort(5)],
        vec![frame(5), tie(5, "prepare", 0, "a", "b", 1, 2), sort(5)],
        // The decision is the second recorded sort's.
        vec![frame(5), sort(5), tie(5, "turn", 0, "a", "b", 1, 2), sort(5)],
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

/// Decisions the sim didn't need are reported: a call's left at its end, a frame's prepare
/// decisions left at its end, a prepare decision of a tick never run, a call the sim never made,
/// a call whose recorded sort is of another tick.
#[test]
fn decisions_the_sim_didnt_need_are_reported() {
    let (mut w, _, _) = tied();
    loaded(
        &mut w,
        &[frame(5), tie(5, "turn", 0, "a", "b", 1, 2), tie(5, "turn", 1, "a", "b", 1, 2), sort(5)],
    );
    sort_units(&mut w, "x");
    assert!(reported(
        &w,
        "CombatController.SortUnits: a recorded identity-hash decision the sim didn't need"
    ));

    let (mut w, _, _) = tied();
    loaded(&mut w, &[frame(5), tie(5, "prepare", 0, "a", "b", 1, 2)]);
    frame_end(&mut w);
    assert!(reported(
        &w,
        "UnitCombatPrepareController: a recorded identity-hash decision the sim didn't need"
    ));

    let (mut w, _, _) = tied();
    load(&mut w, &[frame(3), tie(3, "prepare", 0, "a", "b", 1, 2)], true, true);
    frame_start(&mut w, 5);
    assert!(reported(
        &w,
        "UnitCombatPrepareController: a recorded identity-hash decision the sim didn't need"
    ));

    let (mut w, _, _) = tied();
    loaded(&mut w, &[frame(5), tie(5, "turn", 0, "a", "b", 1, 2), sort(5)]);
    frame_end(&mut w);
    clean(&w);
    finish(&mut w);
    assert!(reported(
        &w,
        "CombatController.SortUnits: a recorded identity-hash decision the sim didn't need"
    ));

    let (mut w, _, _) = tied();
    load(&mut w, &[frame(4), tie(4, "turn", 0, "a", "b", 1, 2), sort(4)], true, true);
    frame_start(&mut w, 5);
    sort_units(&mut w, "x");
    assert!(reported(&w, "CombatController.SortUnits: a call whose recorded sort"));
    assert!(reported(
        &w,
        "CombatController.SortUnits: the comparer's identity-hash fallback without its recorded decision"
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
            frame(5),
            tie(5, "prepare", 0, "a", "b", 1, 2),
            tie(5, "turn", 0, "a", "b", 1, 2),
            sort(5),
            tie(5, "turn", 0, "b", "a", 2, 1),
            sort(5),
        ],
    );
    crate::combat::initiative::prepare_flush(&mut w);
    let cd = |u| w.unit(u).expect("u").combat.cooldown.initiative;
    assert_eq!((cd(b), cd(a)), (0.0, 6.0), "b first: a's hash is the lower");
    frame_end(&mut w);
    assert_eq!(finish(&mut w), 3);
    clean(&w);
}

/// Damaged, misplaced or inconsistent records are reported at load: a result the hashes don't
/// give, a missing or mistyped field, a record outside its tick's frame, a `SortUnits` decision
/// without its sort record in its frame, the same unit with two hashes, an index that doesn't
/// follow its call's previous decision, a decision the oracle couldn't record, records without the
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
        load(&mut w, &[frame(5), r.clone(), sort(5)], true, true);
        assert!(reported(&w, "driven replay: an init_tie record it can't read"), "{r}");
    }
    for recs in [
        vec![ok.clone(), frame(5), sort(5)],
        vec![frame(4), ok.clone(), frame(5), sort(5)],
        vec![frame(5), sort(5), frame(6), ok.clone()],
    ] {
        let mut w = world();
        load(&mut w, &recs, true, true);
        assert!(
            reported(&w, "driven replay: an init_tie record outside its tick's frame"),
            "{recs:?}"
        );
    }
    for recs in [vec![frame(5), ok.clone()], vec![frame(5), ok.clone(), frame(6), sort(6)]] {
        let mut w = world();
        load(&mut w, &recs, true, true);
        assert!(
            reported(
                &w,
                "driven replay: a SortUnits init_tie record without its sort record in its frame"
            ),
            "{recs:?}"
        );
    }
    let mut w = world();
    load(&mut w, &[frame(5), ok.clone(), tie(5, "turn", 0, "a", "c", 3, 4), sort(5)], true, true);
    assert!(reported(
        &w,
        "driven replay: a unit's identity hash differs between its init_tie records"
    ));
    for later in [
        vec![tie(5, "turn", 2, "a", "b", 1, 2)],
        vec![frame(6), tie(6, "turn", 1, "a", "b", 1, 2)],
        vec![tie(5, "prepare", 1, "a", "b", 1, 2)],
    ] {
        let mut w = world();
        let mut recs = vec![frame(5), ok.clone()];
        recs.extend(later.iter().cloned());
        recs.push(sort(later.last().expect("one")["t"].as_i64().expect("t")));
        load(&mut w, &recs, true, true);
        assert!(
            reported(
                &w,
                "driven replay: an init_tie record that doesn't follow its sort call's previous one"
            ),
            "{later:?}"
        );
    }
    let mut w = world();
    load(
        &mut w,
        &[frame(5), json!({"k": "obs", "t": 5, "ev": "init_tie_bad", "cmp": "turn", "why": "x"})],
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

/// The end record's counts of each comparer's calls and decisions must be the sim's; a capture
/// without them isn't checked, one with only some of them is reported.
#[test]
fn the_end_records_counts_are_checked() {
    let end = |tc: u64, tt: u64| {
        json!({"k": "end", "t": 5, "init_tie_compares_turn": tc, "init_ties_turn": tt,
               "init_tie_compares_prepare": 0, "init_ties_prepare": 0})
    };
    // Two units: `Array.Sort` compares once.
    for (tc, tt, ok) in [(1, 1, true), (2, 1, false), (1, 0, false)] {
        let (mut w, _, _) = tied();
        loaded(&mut w, &[frame(5), tie(5, "turn", 0, "a", "b", 1, 2), sort(5), end(tc, tt)]);
        sort_units(&mut w, "x");
        frame_end(&mut w);
        finish(&mut w);
        assert_eq!(
            reported(
                &w,
                "CombatController.SortUnits: the sim's comparer calls or decisions differ"
            ),
            !ok,
            "{tc} {tt}: {:?}",
            w.coverage.unsupported
        );
    }
    let mut w = world();
    load(&mut w, &[frame(5), json!({"k": "end", "t": 5, "init_ties_turn": 0})], true, true);
    assert!(reported(&w, "driven replay: an end record whose init_tie counts it can't read"));
}
