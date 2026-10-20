//! POC-5's driven replay with the roaming states at the window's start the game's
//! (`docs/poc5/orientation.md`, "Roaming"): WrathOracle's observer `roam_init` records each
//! `UnitPartRoaming` at the window's first tick; the sim checks what only the scene and the actions
//! set and takes what the controller advanced since the load. Ignored by default (needs the game's
//! blueprint pack, Manual_18, the caves' nav and physics files and the captures; skipped when a
//! capture is absent).
//! `cargo nextest run -p wr-sim --test poc5_roaming --run-ignored only`.

use std::path::PathBuf;

use serde_json::{Value as V, json};
use wr_sim::combat::replay;

const SAVE: &str = "Manual_18_WR_FirstBattle";

/// The capture with `roam_init` (tb-lizard: the lizard roams before the fight).
const INIT: (&str, &str) = ("poc5-tb-140", "tb-lizard");

fn fixture_in(capture: &str, name: &str) -> Option<PathBuf> {
    // The captures are gitignored and live in the main checkout (see `poc5.rs`).
    let rel = format!("oracle/{capture}/fixtures/{name}");
    let mut roots: Vec<PathBuf> =
        std::env::var_os("WR_CAPTURES").map(PathBuf::from).into_iter().collect();
    roots.extend(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().map(|a| a.join("fixtures/captures")),
    );
    roots.into_iter().map(|r| r.join(&rel)).find(|d| d.join("record.trace.jsonl.gz").exists())
}

fn ev(r: &V, e: &str) -> bool {
    r["k"] == "obs" && r["ev"] == e
}

fn pos(recs: &[V], f: impl Fn(&V) -> bool) -> usize {
    recs.iter().position(f).expect("a record to change")
}

fn init(recs: &[V]) -> usize {
    pos(recs, |r| ev(r, "roam_init"))
}

fn observers_without(recs: &mut [V], name: &str) {
    let b = pos(recs, |r| r["k"] == "begin");
    let obs: Vec<V> =
        recs[b]["observers"].as_array().unwrap().iter().filter(|x| *x != name).cloned().collect();
    recs[b]["observers"] = json!(obs);
}

type Change = fn(&mut Vec<V>);

#[test]
#[ignore = "needs the game's blueprint pack, Manual_18, the caves' nav and physics files and the poc5-tb-140 capture"]
fn missing_or_wrong_roam_init_records_are_reported() {
    let Some(dir) = fixture_in(INIT.0, INIT.1) else {
        eprintln!("{INIT:?} not present: skipped");
        return;
    };
    let cases: Vec<(&str, Change)> = vec![
        // The block.
        ("roam_init: a roam_init without its observer's declaration", |recs| {
            observers_without(recs, "roam_init");
        }),
        ("roam_init without the roaming controller", |recs| {
            observers_without(recs, "cutscene_progress");
        }),
        ("roam_init: a roam_init outside the window's first frame", |recs| {
            // The block moved before the first frame's `tick` record (`t` -1, as pre-window records).
            let first = pos(recs, |r| r["k"] == "tick");
            let at = init(recs);
            let mut block: Vec<V> = recs.drain(at..=at + 1).collect();
            for r in &mut block {
                r["t"] = json!(-1);
            }
            recs.splice(first..first, block);
        }),
        ("roam_init: no roam_init_end", |recs| {
            let at = pos(recs, |r| ev(r, "roam_init_end"));
            recs.remove(at);
        }),
        ("roam_init: roam_init_end", |recs| {
            let at = pos(recs, |r| ev(r, "roam_init_end"));
            recs[at]["n"] = json!(2);
        }),
        ("roam_init: no state of", |recs| {
            let at = init(recs);
            recs.remove(at);
            let end = pos(recs, |r| ev(r, "roam_init_end"));
            recs[end]["n"] = json!(0);
        }),
        ("roam_init: a roam_init it can't read", |recs| {
            let at = init(recs);
            let r = recs[at].clone();
            recs.insert(at + 1, r);
            let end = pos(recs, |r| ev(r, "roam_init_end"));
            recs[end]["n"] = json!(2);
        }),
        ("roam_init: a state of X, not a roaming unit", |recs| {
            let at = init(recs);
            let mut r = recs[at].clone();
            r["u"] = json!("X");
            recs.insert(at, r);
            let end = pos(recs, |r| ev(r, "roam_init_end"));
            recs[end]["n"] = json!(2);
        }),
        ("roam_init: a roam_init outside the window's first frame", |recs| {
            let at = init(recs);
            let mut r = recs[at].clone();
            r["t"] = json!(5);
            let t = pos(recs, |r| r["k"] == "tick" && r["t"] == 5);
            recs.insert(t + 1, r);
        }),
        ("roam_init: a roam_init after the window's first roaming phase", |recs| {
            // At the first frame's end (its roaming phase has run).
            let at = init(recs);
            let r = recs[at].clone();
            let t = pos(recs, |r| r["k"] == "tick" && r["t"] == 1);
            recs.insert(t, r);
        }),
        // A record's form.
        ("roam_init: a roam_init it can't read", |recs| {
            let at = init(recs);
            recs[at].as_object_mut().unwrap().remove("prev");
        }),
        ("roam_init: a roam_init it can't read", |recs| {
            let at = init(recs);
            recs[at]["extra"] = json!(1);
        }),
        ("roam_init: a roam_init it can't read", |recs| {
            let at = init(recs);
            recs[at]["next"]["pos"] = json!([1.0, 2.0]);
        }),
        ("roam_init: a roam_init it can't read", |recs| {
            let at = init(recs);
            recs[at]["next_id"] = json!("abc");
        }),
        ("roam_init: a roam_init it can't read", |recs| {
            let at = init(recs);
            recs[at]["settings"]["move"] = json!("fast");
        }),
        ("roam_init: a roam_init it can't read", |recs| {
            let at = init(recs);
            recs[at]["idle_end"] = json!(1.5);
        }),
        // What only the scene and the actions set.
        ("settings, origin, m_Disabled or follow part differ", |recs| {
            let at = init(recs);
            recs[at]["settings"]["radius"] = json!(3.5);
        }),
        ("settings, origin, m_Disabled or follow part differ", |recs| {
            let at = init(recs);
            recs[at]["settings"]["sleepless"] = json!(true);
        }),
        ("settings, origin, m_Disabled or follow part differ", |recs| {
            let at = init(recs);
            recs[at]["orig"][0] = json!(recs[at]["orig"][0].as_f64().unwrap() + 0.01);
        }),
        ("settings, origin, m_Disabled or follow part differ", |recs| {
            let at = init(recs);
            recs[at]["disabled"] = json!(true);
        }),
        ("settings, origin, m_Disabled or follow part differ", |recs| {
            let at = init(recs);
            recs[at]["follow"] = json!(true);
        }),
        // What the controller advances, taken: a wrong one shows in what it makes next.
        ("roaming", |recs| {
            let at = init(recs);
            recs[at]["next"]["pos"][0] = json!(recs[at]["next"]["pos"][0].as_f64().unwrap() + 1.0);
        }),
        ("roaming", |recs| {
            let at = init(recs);
            recs[at]["idle_end"] = json!(i64::MAX / 2);
        }),
    ];
    let base = {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let _ = replay::run_with_tape(&mut w, &trace, &trace);
        assert!(w.valid(), "the undamaged run: {:?}", w.coverage.unsupported.keys());
        w.coverage.unsupported.keys().cloned().collect::<Vec<_>>()
    };
    let threads =
        std::env::var("WR_TEST_REPLAYS").ok().and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let missed = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((want, change)) = cases.get(i) else { break };
                    let (mut w, trace) =
                        replay::prepare_world(SAVE, &dir, "record").expect("prepare");
                    let changed = trace.edited(|recs| {
                        change(recs);
                        for (n, r) in recs.iter_mut().enumerate() {
                            r["seq"] = json!(n);
                        }
                    });
                    let _ = replay::run_with_tape(&mut w, &changed, &trace);
                    let new: Vec<&String> =
                        w.coverage.unsupported.keys().filter(|k| !base.contains(k)).collect();
                    if w.valid() || !new.iter().any(|k| k.contains(want)) {
                        missed.lock().unwrap().push(format!("case {i}: {want}: {new:?}"));
                    }
                }
            });
        }
    });
    let missed = missed.into_inner().unwrap();
    assert!(missed.is_empty(), "{missed:#?}");
}

/// Every run of the capture with `roam_init`: valid, its one roaming unit's state the game's at
/// the window's start (advanced since the load: `IsPlayingIdle` cleared the saved idle end).
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18, the caves' nav and physics files and the poc5-tb-140 capture"]
fn every_roaming_state_at_the_start_is_the_games() {
    let Some(dir) = fixture_in(INIT.0, INIT.1) else {
        eprintln!("{INIT:?} not present: skipped");
        return;
    };
    let mut failed = Vec::new();
    for run in ["record", "replay-1", "replay-2", "replay-3", "replay-4"] {
        let (out, w) = replay::run_fixture(SAVE, &dir, run).expect("replay");
        let r = &out.roam;
        if !w.valid() || r.established != 1 || r.init_taken != 1 || r.unknown != 0 || r.rolls == 0 {
            failed.push(format!("{run}: {r:?} {:?}", w.coverage.unsupported.keys()));
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
