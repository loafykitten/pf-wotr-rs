//! The initiative comparers' identity-hash tie-break against WrathOracle's capture (local,
//! gitignored) `poc5-tb-127/tb-tie`: `poc5-tb`'s `tb-nearest` with the plan step `seedinit` making
//! the two centipedes 51DD and 51FC tie at combat start, so both comparers reach the hash (observer
//! `init_ties`). See `docs/poc5/tb-replay.md`, "The initiative tie-break recorded". Ignored by
//! default (needs the game's packs, Manual_18 and the capture; skipped when it is absent).
//! `cargo nextest run -p wr-sim --test poc5_ties --run-ignored only`.

use std::path::PathBuf;

use serde_json::Value as V;
use serde_json::json;
use wr_sim::combat::replay;

const SAVE: &str = "Manual_18_WR_FirstBattle";
const TIE: (&str, &str) = ("poc5-tb-127", "tb-tie");

fn fixture_in(capture: &str, name: &str) -> Option<PathBuf> {
    // The captures live in the main checkout: look in this checkout and every directory above it
    // (a worktree under `.claude/worktrees/` finds the main one), or where `WR_CAPTURES` says.
    let rel = format!("oracle/{capture}/fixtures/{name}");
    let mut roots: Vec<PathBuf> =
        std::env::var_os("WR_CAPTURES").map(PathBuf::from).into_iter().collect();
    roots.extend(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().map(|a| a.join("fixtures/captures")),
    );
    roots.into_iter().map(|r| r.join(&rel)).find(|d| d.join("record.trace.jsonl.gz").exists())
}

fn is_tie(r: &V) -> bool {
    r["k"] == "obs" && r["ev"] == "init_tie"
}

fn renumber(recs: &mut [V]) {
    for (k, r) in recs.iter_mut().enumerate() {
        r["seq"] = json!(k);
    }
}

/// The k-th `init_tie` record of comparer `cmp`.
fn nth_tie(recs: &[V], cmp: &str, k: usize) -> usize {
    recs.iter()
        .enumerate()
        .filter(|(_, r)| is_tie(r) && r["cmp"] == cmp)
        .nth(k)
        .map(|(i, _)| i)
        .expect("tie record")
}

/// Every run of `tb-tie` is valid with the tie decided by the recorded decisions: the prepare
/// sort's one and seven `SortUnits` calls', each taken where the sim's sort reached the fallback,
/// the end record's counts of the comparers' calls and decisions the sim's.
#[test]
#[ignore = "needs the game's packs, Manual_18 and the poc5-tb-127 capture"]
fn the_tie_is_decided_by_the_recorded_decisions() {
    let Some(dir) = fixture_in(TIE.0, TIE.1) else {
        eprintln!("{}/{} not present: skipped", TIE.0, TIE.1);
        return;
    };
    for run in ["record", "replay-1", "replay-2"] {
        let (out, w) = replay::run_fixture(SAVE, &dir, run).expect("replay");
        let reports: Vec<_> = w.coverage.unsupported.keys().collect();
        assert!(w.valid(), "{run}: {reports:?}");
        assert!(out.diff.equal, "{run}: {:?}", out.diff.first_difference);
        assert_eq!(out.sorts.hash_decisions, 8, "{run}");
        assert_eq!(out.sorts.checked, out.sorts.equal, "{run}");
    }
}

/// A damaged recording is reported: each case changes the record run's trace, keeps its tape, and
/// must make the run invalid with the named report.
#[test]
#[ignore = "needs the game's packs, Manual_18 and the poc5-tb-127 capture"]
fn missing_or_wrong_tie_records_are_reported() {
    let Some(dir) = fixture_in(TIE.0, TIE.1) else {
        eprintln!("{}/{} not present: skipped", TIE.0, TIE.1);
        return;
    };
    type Change = fn(&mut Vec<V>);
    let cases: Vec<(&str, Change)> = vec![
        ("without its recorded decision", |recs| {
            let at = nth_tie(recs, "turn", 2);
            recs.remove(at);
            renumber(recs);
        }),
        ("UnitCombatPrepareController: the comparer's identity-hash fallback without", |recs| {
            let at = nth_tie(recs, "prepare", 0);
            recs.remove(at);
            renumber(recs);
        }),
        ("the sim didn't need", |recs| {
            let at = nth_tie(recs, "turn", 0);
            let mut extra = recs[at].clone();
            extra["i"] = json!(1);
            recs.insert(at + 1, extra);
            renumber(recs);
        }),
        ("an init_tie record it can't read", |recs| {
            let at = nth_tie(recs, "turn", 3);
            let r = recs[at]["r"].as_i64().unwrap();
            recs[at]["r"] = json!(-r);
        }),
        ("without its recorded decision", |recs| {
            // The units swapped, hashes and result kept consistent.
            let at = nth_tie(recs, "turn", 1);
            let r = recs[at].clone();
            recs[at]["a"] = r["b"].clone();
            recs[at]["b"] = r["a"].clone();
            recs[at]["ha"] = r["hb"].clone();
            recs[at]["hb"] = r["ha"].clone();
            recs[at]["r"] = json!(-r["r"].as_i64().unwrap());
        }),
        ("the sim didn't need", |recs| {
            // A decision moved to the next SortUnits call.
            let at = nth_tie(recs, "turn", 0);
            let r = recs.remove(at);
            let next = at
                + 1
                + recs[at..]
                    .iter()
                    .position(|x| x["k"] == "tb" && x["ev"] == "sort")
                    .expect("its sort");
            recs.insert(next, r);
            renumber(recs);
        }),
        ("outside its tick's frame", |recs| {
            let at = nth_tie(recs, "prepare", 0);
            let r = recs.remove(at);
            let next = at + recs[at..].iter().position(|x| x["k"] == "tick").expect("next frame");
            recs.insert(next + 1, r);
            renumber(recs);
        }),
        ("a unit's identity hash differs", |recs| {
            let at = nth_tie(recs, "turn", 4);
            let (ha, hb) = (recs[at]["ha"].as_i64().unwrap(), recs[at]["hb"].as_i64().unwrap());
            // Another value for the first unit, the result unchanged (moved away from the other).
            recs[at]["ha"] = json!(if ha >= hb { ha + 1 } else { ha - 1 });
        }),
        ("init_tie records in a recording that doesn't declare init_ties", |recs| {
            let b = recs.iter_mut().find(|r| r["k"] == "begin").expect("begin");
            let obs: Vec<V> = b["observers"]
                .as_array()
                .expect("observers")
                .iter()
                .filter(|o| *o != "init_ties")
                .cloned()
                .collect();
            b["observers"] = json!(obs);
        }),
        ("differ from the end record's counts", |recs| {
            let end = recs.iter_mut().rev().find(|r| r["k"] == "end").expect("end");
            let n = end["init_tie_compares_turn"].as_u64().expect("count");
            end["init_tie_compares_turn"] = json!(n + 1);
        }),
        ("an identity-hash decision WrathOracle couldn't record", |recs| {
            let at = nth_tie(recs, "turn", 5);
            recs[at] = json!({"seq": 0, "k": "obs", "t": recs[at]["t"], "ev": "init_tie_bad",
                              "cmp": "turn", "why": "1 hash calls"});
            renumber(recs);
        }),
    ];
    let mut missed = Vec::new();
    // One at a time: each run holds the whole recording (the capture tests' memory rule).
    for (i, (want, change)) in cases.iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let changed = trace.edited(change);
        let _ = replay::run_with_tape(&mut w, &changed, &trace);
        let hit = w.coverage.unsupported.keys().any(|k| k.contains(want));
        if w.valid() || !hit {
            missed.push(format!("case {i}: {want}: {:?}", w.coverage.unsupported.keys()));
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
}
