//! POC-5's driven replay with `UnitEntityView.Corpulence` computed (`docs/poc5/corpulence.md`):
//! every `corp` read of the recording is checked against the sim's value from the unit's prefab and
//! size, and a read that differs by a bit is reported. Ignored by default (needs the game's
//! blueprint pack, the unit prefab table, Manual_18 and the capture; skipped when the capture is
//! absent). `cargo nextest run -p wr-sim --test poc5_corpulence --run-ignored only`.

use std::path::PathBuf;

use serde_json::{Value as V, json};
use wr_sim::combat::replay;

const SAVE: &str = "Manual_18_WR_FirstBattle";

/// Camelia casts Enlarge Person on Seelah (`558A`): her reads go from 0.5 to 0.75757575.
const CAST: (&str, &str) = ("poc5-tb-120", "tb-cast");

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

fn corp(r: &V) -> bool {
    r["k"] == "obs" && r["ev"] == "uc_v" && r["f"] == "corp"
}

/// The read value as a float.
fn value(r: &V) -> f32 {
    r["v"].as_str().and_then(|s| s.parse().ok()).expect("a uc_v corp value")
}

type Change = fn(&mut Vec<V>);

#[test]
#[ignore = "needs the game's blueprint pack, the unit prefab table, Manual_18 and the poc5-tb-120 capture"]
fn a_corpulence_read_that_differs_from_the_sims_is_reported() {
    let Some(dir) = fixture_in(CAST.0, CAST.1) else {
        eprintln!("{CAST:?} not present: skipped");
        return;
    };
    let run = |change: Option<Change>| {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let changed = change.map(|c| trace.edited(c));
        let out = replay::run_with_tape(&mut w, changed.as_ref().unwrap_or(&trace), &trace);
        let reports: Vec<String> =
            w.coverage.unsupported.keys().filter(|k| k.contains("corpulence")).cloned().collect();
        (out.corpulence.expect("a recording with corp reads"), reports, w.valid())
    };
    // The undamaged recording: every read compared is equal, the enlarged unit's among them.
    let (c, reports, valid) = run(None);
    assert!(valid && reports.is_empty() && c.differ == 0, "{c:?} {reports:?}");
    assert!(c.equal > 0 && c.equal + c.unknown_total() + c.event == c.reads, "{c:?}");
    let cases: Vec<(&str, Change)> = vec![
        // The enlarged unit's reads after the cast still show the size it had.
        ("558A's Corpulence is 0.5 in the recording", |recs| {
            for r in recs.iter_mut().filter(|r| corp(r) && r["u"] == "558A") {
                if value(r) > 0.6 {
                    r["v"] = json!("0.5");
                }
            }
        }),
        // One bit off (the next float up).
        ("558A's Corpulence is 0.7575758 in the recording", |recs| {
            for r in recs.iter_mut().filter(|r| corp(r) && r["u"] == "558A") {
                let v = value(r);
                if v > 0.6 {
                    r["v"] = json!(f32::from_bits(v.to_bits() + 1).to_string());
                }
            }
        }),
        // A unit that never changes size, one read of it.
        ("in the recording", |recs| {
            let at = recs.iter().position(|r| corp(r) && r["u"] != "558A").unwrap();
            let bits = value(&recs[at]).to_bits();
            recs[at]["v"] = json!(f32::from_bits(bits + 1).to_string());
        }),
        // An agent request's read (its unit is the request's).
        ("in the recording", |recs| {
            let at = recs
                .iter()
                .position(|r| {
                    r["k"] == "obs" && r["ev"] == "ag_v" && r["f"] == "corp" && r["u"].is_null()
                })
                .unwrap();
            recs[at]["v"] = json!(0.6);
        }),
    ];
    for (i, (want, change)) in cases.iter().enumerate() {
        let (c, reports, valid) = run(Some(*change));
        assert!(
            !valid && c.differ > 0 && reports.iter().any(|k| k.contains(want)),
            "case {i}: {want}: {c:?} {reports:?}"
        );
    }
}
