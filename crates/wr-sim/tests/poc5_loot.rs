//! The leader's fall and the lizards' loot against WrathOracle's `poc5-tb-131` and `-134`
//! (`tb-lizard`, local and gitignored): in both, the main character is struck unconscious (the
//! prone controller makes her fall and the turn controller waits for her to get up), the dying
//! lizard's skinning items leave its inventory as its loot drops (the combat log's
//! `GameLogEventItemsCollection`), and the log names her unidentified ring. Every run is valid,
//! every unit's posture equals the recording's end state, and a damaged record of the new
//! families is reported. Ignored by default (needs the game's packs, `cache/sim/log-strings.json`,
//! Manual_18 and the captures; skipped when a capture is absent).
//! `cargo nextest run -p wr-sim --test poc5_loot --run-ignored only`.

use std::path::PathBuf;

use serde_json::Value as V;
use serde_json::json;
use wr_sim::combat::replay;

const SAVE: &str = "Manual_18_WR_FirstBattle";
const CAPTURES: [&str; 2] = ["poc5-tb-131", "poc5-tb-134"];
const FIGHT: &str = "tb-lizard";

fn fixture_in(capture: &str, name: &str) -> Option<PathBuf> {
    let rel = format!("oracle/{capture}/fixtures/{name}");
    let mut roots: Vec<PathBuf> =
        std::env::var_os("WR_CAPTURES").map(PathBuf::from).into_iter().collect();
    roots.extend(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().map(|a| a.join("fixtures/captures")),
    );
    roots.into_iter().map(|r| r.join(&rel)).find(|d| d.join("record.trace.jsonl.gz").exists())
}

fn read_json(path: &std::path::Path) -> V {
    serde_json::from_slice(&std::fs::read(path).expect("file")).expect("json")
}

/// Every run is valid; the log's events, ticks and messages equal the recording's, the posture
/// of every unit equals its recorded end state, and the new records were all used: one
/// collection event (the lizard's skinning item), the leader's fall, each dead creature's.
#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-131/-134 captures"]
fn the_leaders_fall_and_the_lizards_loot_are_the_sims() {
    for cap in CAPTURES {
        let Some(dir) = fixture_in(cap, FIGHT) else {
            eprintln!("{cap}/{FIGHT} not present: skipped");
            continue;
        };
        for run in ["record", "replay-1", "replay-2"] {
            let (out, w) = replay::run_fixture(SAVE, &dir, run).expect("replay");
            let name = format!("{cap}/{run}");
            assert!(w.valid(), "{name}: {:?}", w.coverage.unsupported.keys().collect::<Vec<_>>());
            assert!(out.diff.equal, "{name}: {:?}", out.diff.first_difference);
            let c = out.clog.expect("the combat log is modelled");
            assert_eq!(c.differences, 0, "{name}");
            assert_eq!(c.queued[0], c.queued[1], "{name}");
            assert_eq!(c.messages[0], c.messages[1], "{name}");
            assert!(out.prone.falls >= 8, "{name}: {:?}", out.prone);
            assert_eq!(out.prone.get_ups, 0, "{name}");
            // The postures against the end state.
            let units: Vec<_> = (0..w.entities.len())
                .map(|i| wr_sim::world::EntityIdx(u32::try_from(i).unwrap()))
                .filter(|e| w.unit(*e).is_some())
                .collect();
            let sheet = wr_sim::sheet::dump(&w, &units, "end");
            let end = read_json(&dir.join(format!("state-end-{run}.json")));
            let (n, diffs) = wr_sim::compare::compare_prone(&sheet, &end);
            assert!(n >= 20 && diffs.is_empty(), "{name}: {n} units, {diffs:?}");
            // The dropped loot joined the state at the creation tick, with its unit remembering it.
            assert_eq!(w.player.created_entities.len(), 1, "{name}");
        }
    }
}

fn nth_q(recs: &[V], e: &str, n: usize) -> usize {
    recs.iter()
        .enumerate()
        .filter(|(_, r)| r["ev"] == "clog_q" && r["e"] == e)
        .nth(n)
        .map(|(i, _)| i)
        .expect("a clog_q")
}

fn items_q(recs: &[V]) -> usize {
    nth_q(recs, "GameLogEventItemsCollection", 0)
}

fn renumber(recs: &mut [V]) {
    for (n, r) in recs.iter_mut().enumerate() {
        r["seq"] = json!(n);
    }
}

/// The prone controller's `InterruptAll` records.
fn prone_calls(recs: &[V]) -> Vec<usize> {
    (0..recs.len())
        .filter(|&i| {
            recs[i]["ev"] == "uc_x"
                && recs[i]["origin"]
                    .as_str()
                    .is_some_and(|o| o.contains("UnitProneController.Tick"))
        })
        .collect()
}

/// A call's records, from its `uc_x` to its `uc_x_end`.
fn call_span(recs: &[V], at: usize) -> (usize, usize) {
    let end = (at..recs.len()).find(|&i| recs[i]["ev"] == "uc_x_end").expect("the call's end");
    (at, end)
}

const L: &str = "combat log:";
const P: &str = "prone controller";

type Change = fn(&mut Vec<V>);

/// The damaged recordings, each with the report it must make.
const CASES: [(&str, Change); 14] = [
    // The collection event: dropped, another count, kind, item; duplicated; its raise elsewhere.
    (L, |recs| {
        let at = items_q(recs);
        recs.remove(at);
        renumber(recs);
    }),
    (L, |recs| {
        let at = items_q(recs);
        recs[at]["f"]["Count"] = json!(2);
    }),
    (L, |recs| {
        let at = items_q(recs);
        recs[at]["f"]["Event"] = json!(0);
    }),
    (L, |recs| {
        let at = items_q(recs);
        recs[at]["f"]["Item"]["$entity"] = json!("00000000-0000-0000-0000-000000000000");
    }),
    (L, |recs| {
        let at = items_q(recs);
        let r = recs[at].clone();
        recs.insert(at, r);
        renumber(recs);
    }),
    (L, |recs| {
        let at = items_q(recs);
        let o = recs[at]["origin"].as_str().unwrap_or("").replace("OnItemsRemoved", "Other");
        recs[at]["origin"] = json!(o);
    }),
    // Before the bag's rotation draw (the removal follows it).
    (L, |recs| {
        let at = items_q(recs);
        let r = recs.remove(at);
        recs.insert(at - 1, r);
        renumber(recs);
    }),
    // The enabled flag.
    (L, |recs| {
        let at = items_q(recs);
        recs[at]["en"] = json!(false);
    }),
    // The prone controller's call: dropped, repeated, for another unit, retyped.
    (P, |recs| {
        let (a, b) = call_span(recs, prone_calls(recs)[0]);
        recs.drain(a..=b);
        renumber(recs);
    }),
    (P, |recs| {
        let (a, b) = call_span(recs, prone_calls(recs)[1]);
        let span: Vec<V> = recs[a..=b].to_vec();
        for (n, r) in span.into_iter().enumerate() {
            recs.insert(b + 1 + n, r);
        }
        renumber(recs);
    }),
    (P, |recs| {
        let at = prone_calls(recs)[2];
        let other = recs[prone_calls(recs)[3]]["u"].clone();
        recs[at]["u"] = other;
    }),
    (P, |recs| {
        let at = prone_calls(recs)[0];
        recs[at]["fn"] = json!("InterruptMove");
    }),
    // The turn controller's view of the leader's posture at `Prepare`.
    ("rider's prone state", |recs| {
        let at = recs
            .iter()
            .position(|r| r["ev"] == "tt_prep" && r["prone"] == true)
            .expect("the leader's prepare");
        recs[at]["prone"] = json!(false);
    }),
    ("rider's prone state", |recs| {
        let at = recs
            .iter()
            .position(|r| r["ev"] == "tt_prep" && r["prone"] == false && r["ctrl"] == true)
            .expect("a party prepare");
        recs[at]["prone"] = json!(true);
    }),
];

#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-131 capture"]
fn missing_or_wrong_loot_and_prone_records_are_reported() {
    let Some(dir) = fixture_in(CAPTURES[0], FIGHT) else {
        eprintln!("{}/{FIGHT} not present: skipped", CAPTURES[0]);
        return;
    };
    let mut missed = Vec::new();
    for (i, (want, change)) in CASES.into_iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let changed = trace.edited(change);
        let _ = replay::run_with_tape(&mut w, &changed, &trace);
        if w.valid() || !w.coverage.unsupported.keys().any(|k| k.contains(want)) {
            missed.push(format!(
                "case {i}: {want}: {:?}",
                w.coverage.unsupported.keys().collect::<Vec<_>>()
            ));
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
}

/// A posture of the end state that differs is found by the comparison.
#[test]
#[ignore = "needs the poc5-tb-131 capture"]
fn a_damaged_end_posture_is_found() {
    let Some(dir) = fixture_in(CAPTURES[0], FIGHT) else {
        eprintln!("{}/{FIGHT} not present: skipped", CAPTURES[0]);
        return;
    };
    let (_, w) = replay::run_fixture(SAVE, &dir, "record").expect("replay");
    let units: Vec<_> = (0..w.entities.len())
        .map(|i| wr_sim::world::EntityIdx(u32::try_from(i).unwrap()))
        .filter(|e| w.unit(*e).is_some())
        .collect();
    let sheet = wr_sim::sheet::dump(&w, &units, "end");
    let end = read_json(&dir.join("state-end-record.json"));
    let (_, diffs) = wr_sim::compare::compare_prone(&sheet, &end);
    assert!(diffs.is_empty(), "{diffs:?}");
    for (field, value) in
        [("Active", json!(false)), ("Duration", json!(1)), ("ShouldBeActive", json!(true))]
    {
        let mut bad = end.clone();
        let at = bad["units"]
            .as_array()
            .and_then(|u| u.iter().position(|x| x["state"]["Prone"]["Active"] == true))
            .expect("a prone unit");
        bad["units"][at]["state"]["Prone"][field] = value;
        let (_, diffs) = wr_sim::compare::compare_prone(&sheet, &bad);
        assert_eq!(diffs.len(), 1, "{field}: {diffs:?}");
    }
}
