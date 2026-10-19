//! POC-7's units timeline (`docs/poc7/fight.md`): the driven replay of a fight, collecting every
//! unit's presentation state at the end of each frame. Ignored by default (needs the game's
//! blueprint pack, Manual_18, the caves' nav and physics files and the capture; skipped when the
//! capture is absent). `cargo nextest run -p wr-sim --test poc7_timeline --run-ignored only`.
//!
//! - the replay's outputs are the same with and without collecting;
//! - the timeline's positions are the replay's own values: its last frame is the world's end
//!   state, and in frames the sim computed they equal the recording's observation rows (which the
//!   replay checks against the sim's, bit for bit);
//! - a run that is not valid has no timeline.

use std::path::PathBuf;

use serde_json::Value as V;
use wr_sim::combat::replay::{self, Row, timeline::Source};

const SAVE: &str = "Manual_18_WR_FirstBattle";

/// Four fights of the lizard encounter, with the player's walk-ups in real time between them.
const FIGHT: (&str, &str) = ("poc5-tb-140", "tb-lizard");

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

fn bits(p: [f32; 3]) -> [u32; 3] {
    p.map(f32::to_bits)
}

/// The observation rows `(record index, tick, unit, row)`.
fn rows(trace: &wr_sim::combat::oracle::Trace) -> Vec<(usize, i64, String, Row)> {
    trace
        .records
        .iter()
        .enumerate()
        .filter(|(_, r)| r["k"] == "obs" && r["ev"] == "u")
        .filter_map(|(i, r)| {
            Some((i, r["t"].as_i64()?, r["u"].as_str()?.to_owned(), Row::parse(r["s"].as_str()?)?))
        })
        .collect()
}

#[test]
#[ignore = "needs the game's blueprint pack, the unit prefab table, Manual_18 and the poc5-tb-140 capture"]
fn the_timeline_is_the_replays_own_and_changes_nothing() {
    let Some(dir) = fixture_in(FIGHT.0, FIGHT.1) else {
        eprintln!("{FIGHT:?} not present: skipped");
        return;
    };
    // Without collecting.
    let (plain, w_plain) = replay::run_fixture(SAVE, &dir, "record").expect("replay");
    // Collecting.
    let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
    let (out, tl) = replay::timeline::run_timeline(&mut w, &trace);
    let tl = tl.expect("poc5-tb-140's record run is valid");
    assert!(w.valid() && w_plain.valid());
    assert_eq!(
        (&plain.oracle, &plain.sim, format!("{:?}", plain.diff)),
        (&out.oracle, &out.sim, format!("{:?}", out.diff)),
        "the replay's streams differ when collecting"
    );
    assert!(out.diff.equal);
    assert_eq!(
        w_plain.coverage.unsupported.keys().collect::<Vec<_>>(),
        w.coverage.unsupported.keys().collect::<Vec<_>>()
    );
    assert_eq!(plain.roots, out.roots);

    // One frame per `tick` record, in order; every unit of the area that is in the game is tracked.
    let want: Vec<i64> =
        trace.records.iter().filter(|r| r["k"] == "tick").filter_map(|r| r["t"].as_i64()).collect();
    assert_eq!(tl.ticks, want);
    assert_eq!(tl.tracks.len(), 21);

    // The last frame is the world's end state.
    for t in &tl.tracks {
        let e = w.by_id[&t.id];
        let u = w.unit(e).expect("a unit");
        let last = t.samples.last().expect("a sample");
        assert_eq!(t.first + t.samples.len(), tl.ticks.len(), "{}", t.id);
        assert_eq!(bits(last.position), bits(u.position), "{} position", t.id);
        assert_eq!(last.orientation.to_bits(), u.orientation.to_bits(), "{} orientation", t.id);
        assert_eq!(last.life_state, u.life_state, "{} life state", t.id);
        assert_eq!(last.prone, u.prone_active, "{} prone", t.id);
    }

    // In frames the sim computed (the frame before a row's and the row's own), a unit's position
    // at the end of the previous frame is the row the recording wrote at the next frame's prefix.
    let mut checked = 0usize;
    let mut moved = 0usize;
    for (_, tick, unit, row) in rows(&trace) {
        let Some(frame) = tl.ticks.binary_search(&tick).ok().and_then(|f| f.checked_sub(1)) else {
            continue;
        };
        let (Some(before), Some(at)) = (tl.sample(&unit, frame), tl.sample(&unit, frame + 1))
        else {
            continue;
        };
        if before.source != Source::Sim || at.source != Source::Sim {
            continue;
        }
        assert_eq!(
            bits(before.position),
            bits(row.pos),
            "{unit}: the position at the end of frame {} is the row of frame {tick}",
            tl.ticks[frame]
        );
        checked += 1;
        moved += usize::from(before.speed > 0.0);
    }
    assert!(checked > 5_000 && moved > 100, "checked {checked} rows, {moved} of a moving unit");

    // The timeline sees the fight: the first lizard to die, standing prone, was alive earlier.
    let dead: Vec<_> = tl
        .tracks
        .iter()
        .filter(|t| t.samples.last().is_some_and(|s| s.life_state == 2 && s.prone))
        .collect();
    assert!(dead.len() >= 5, "{} units dead and prone", dead.len());
    for t in dead {
        assert_eq!(t.samples[0].life_state, 0, "{} starts alive", t.id);
    }
    // Sim-computed frames are most of the run, recorded ones the real-time walk-ups.
    let sim = tl.tracks[0].samples.iter().filter(|s| s.source == Source::Sim).count();
    assert!(sim * 5 > tl.ticks.len() * 3 && sim < tl.ticks.len());
}

#[test]
#[ignore = "needs the game's blueprint pack, the unit prefab table, Manual_18 and the poc5-tb-140 capture"]
fn a_run_that_is_not_valid_has_no_timeline() {
    let Some(dir) = fixture_in(FIGHT.0, FIGHT.1) else {
        eprintln!("{FIGHT:?} not present: skipped");
        return;
    };
    let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
    let (_, tl) = replay::timeline::run_timeline(&mut w, &trace);
    let tl = tl.expect("the undamaged run is valid");
    // A row in a frame the sim computed (before and after it), moved a metre: the replay reports
    // that the row's position differs from the sim's, and the run is invalid.
    let (i, ..) = rows(&trace)
        .into_iter()
        .find(|(_, tick, unit, _)| {
            let f = tl.ticks.binary_search(tick).unwrap_or(0);
            [f.saturating_sub(1), f]
                .iter()
                .all(|f| tl.sample(unit, *f).is_some_and(|s| s.source == Source::Sim))
                && tl.sample(unit, f).is_some_and(|s| s.speed > 0.0)
        })
        .expect("a row to change");
    let damaged = trace.edited(|recs: &mut Vec<V>| {
        let s = recs[i]["s"].as_str().expect("a row").to_owned();
        let (pos, rest) = s.split_once('|').expect("a row's tail");
        let mut p: Vec<f32> = pos.split(',').map(|x| x.parse().expect("a float")).collect();
        p[0] += 1.0;
        let pos: Vec<String> = p.iter().map(ToString::to_string).collect();
        recs[i]["s"] = V::from(format!("{}|{rest}", pos.join(",")));
    });
    let (mut w, _) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
    let (_, bad) = replay::timeline::run_timeline(&mut w, &damaged);
    let e = bad.expect_err("a damaged run has no timeline");
    assert!(!w.valid());
    assert!(e.unsupported.iter().any(|k| k.contains("observation row's position")), "{e}");
}
