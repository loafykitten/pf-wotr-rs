//! POC-5's driven replay with units' `DesiredOrientation` the sim's (`docs/poc5/orientation.md`,
//! "The writers"): WrathOracle's observer `look` records every orientation write outside
//! `TickUnit`'s own (`ag_look`); the sim makes the commands' and checks them, takes the others'
//! arguments by cause, and every `UpdateSlowRotation` read is its own. Ignored by default (needs
//! the game's blueprint pack, Manual_18, the caves' nav and physics files and the captures;
//! skipped when a capture is absent).
//! `cargo nextest run -p wr-sim --test poc5_look --run-ignored only`.

use std::path::PathBuf;

use serde_json::{Value as V, json};
use wr_sim::combat::replay;

const SAVE: &str = "Manual_18_WR_FirstBattle";

/// The capture the records are damaged in (tb-focus has an attack of opportunity).
const LOOK: (&str, &str) = ("poc5-tb-132", "tb-focus");

const TURN: &str = "Kingmaker.UnitLogic.Commands.Base.UnitCommand.TurnToTarget";

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

/// The first record matching `f` some frames after the synchronization (turn-based, the agent
/// the sim's).
fn later(recs: &[V], f: impl Fn(&V) -> bool) -> usize {
    let s = pos(recs, |r| ev(r, "ag_sync"));
    let t = recs[s]["t"].as_i64().unwrap() + 30;
    s + recs[s..]
        .iter()
        .position(|r| r["t"].as_i64() > Some(t) && f(r))
        .expect("a record to change")
}

fn origin(r: &V) -> Vec<&str> {
    r["origin"].as_str().unwrap_or("").split(" < ").collect()
}

/// A write by `TurnToTarget` from `caller` (`TickCommand`, `TickApproaching`).
fn turn(recs: &[V], caller: &'static str) -> usize {
    later(recs, |r| {
        let o = origin(r);
        ev(r, "ag_look") && o[0] == TURN && o.get(1).is_some_and(|f| f.ends_with(caller))
    })
}

fn tick_command(recs: &[V]) -> usize {
    turn(recs, ".TickCommand")
}

/// A write the first frame of whose caller chain ends with `frame`.
fn by(recs: &[V], frame: &'static str) -> usize {
    later(recs, |r| ev(r, "ag_look") && origin(r)[0].ends_with(frame))
}

fn roaming(recs: &[V]) -> usize {
    by(recs, "UnitRoamingController.TickOnUnit")
}

fn observers_without(recs: &mut [V], name: &str) {
    let b = pos(recs, |r| r["k"] == "begin");
    let obs: Vec<V> =
        recs[b]["observers"].as_array().unwrap().iter().filter(|x| *x != name).cloned().collect();
    recs[b]["observers"] = json!(obs);
}

fn bump(r: &mut V, k: &str) {
    r[k] = json!(r[k].as_f64().unwrap() + 1.0);
}

type Change = fn(&mut Vec<V>);

#[test]
#[ignore = "needs the game's blueprint pack, Manual_18, the caves' nav and physics files and the poc5-tb-132 capture"]
fn missing_or_wrong_look_records_are_reported() {
    let Some(dir) = fixture_in(LOOK.0, LOOK.1) else {
        eprintln!("{LOOK:?} not present: skipped");
        return;
    };
    let cases: Vec<(&str, Change)> = vec![
        // The capability.
        ("ag_look records without the observer's declaration", |recs| {
            observers_without(recs, "look");
        }),
        ("observer look without the driven agent", |recs| observers_without(recs, "agent_sync")),
        // The sim's writes: missing, of another call, unit, caller, argument, state or result.
        ("(TickCommand's TurnToTarget): the recording has", |recs| {
            let at = tick_command(recs);
            recs.remove(at);
        }),
        ("(TickApproaching's TurnToTarget): the recording has", |recs| {
            let at = turn(recs, ".TickApproaching");
            recs.remove(at);
        }),
        ("(TickCommand's TurnToTarget): the recording has", |recs| {
            let at = tick_command(recs);
            recs[at]["fn"] = json!("ForceLookAt");
        }),
        ("(TickCommand's TurnToTarget): the recording has", |recs| {
            let at = tick_command(recs);
            let other = recs.iter().find(|r| ev(r, "ag_look") && r["u"] != recs[at]["u"]).unwrap()
                ["u"]
                .clone();
            recs[at]["u"] = other;
        }),
        ("(TickCommand's TurnToTarget): the recording has", |recs| {
            let at = tick_command(recs);
            let o = recs[at]["origin"].as_str().unwrap().replacen(
                ".UnitCommandController.TickCommand",
                ".UnitCommandController.TickOnUnit",
                1,
            );
            recs[at]["origin"] = json!(o);
        }),
        ("(TickCommand's TurnToTarget): the sim's argument", |recs| {
            let at = tick_command(recs);
            recs[at]["p"][0] = json!(recs[at]["p"][0].as_f64().unwrap() + 0.5);
        }),
        ("(TickCommand's TurnToTarget): the sim's position", |recs| {
            let at = tick_command(recs);
            bump(&mut recs[at], "d");
        }),
        ("(TickCommand's TurnToTarget): the sim's position", |recs| {
            let at = tick_command(recs);
            bump(&mut recs[at], "o");
        }),
        ("(TickCommand's TurnToTarget): the sim's position", |recs| {
            let at = tick_command(recs);
            bump(&mut recs[at], "d0");
        }),
        ("(TickCommand's TurnToTarget): the sim's position", |recs| {
            let at = tick_command(recs);
            recs[at]["pos"][2] = json!(recs[at]["pos"][2].as_f64().unwrap() + 0.25);
        }),
        ("(TickCommand's TurnToTarget): a rider's", |recs| {
            let at = tick_command(recs);
            recs[at]["rider"] = json!(true);
        }),
        ("(TickCommand's TurnToTarget): an ag_look it can't read", |recs| {
            let at = tick_command(recs);
            recs[at]["extra"] = json!(1);
        }),
        ("(TickCommand's TurnToTarget): an ag_look it can't read", |recs| {
            let at = tick_command(recs);
            recs[at]["v"] = json!(3.0);
        }),
        ("of another frame", |recs| {
            let at = tick_command(recs);
            recs[at]["t"] = json!(recs[at]["t"].as_i64().unwrap() + 1);
        }),
        // A write of code the sim runs that it didn't make.
        ("TurnToTarget, code the sim runs, which didn't make it", |recs| {
            let at = tick_command(recs);
            let copy = recs[at].clone();
            recs.insert(at + 1, copy);
        }),
        // The attack of opportunity's `LookAt` (made past a presentation gate): missing, so the
        // next write starts from another orientation.
        ("the sim's position", |recs| {
            let at = by(recs, "UnitAttackOfOpportunity.OnRun");
            recs.remove(at);
        }),
        ("(UnitAttackOfOpportunity.OnRun): the sim's position", |recs| {
            let at = by(recs, "UnitAttackOfOpportunity.OnRun");
            bump(&mut recs[at], "d");
        }),
        // A write by code the sim doesn't run (roaming): its state before, its result, its unit,
        // missing (the next read differs), or a rider's.
        (
            "LookAt by UnitRoamingController.TickOnUnit: the unit's position or orientation before",
            |recs| {
                let at = roaming(recs);
                bump(&mut recs[at], "d0");
            },
        ),
        ("LookAt by UnitRoamingController.TickOnUnit: the result differs", |recs| {
            let at = roaming(recs);
            bump(&mut recs[at], "d");
        }),
        ("LookAt by UnitRoamingController.TickOnUnit: the result differs", |recs| {
            let at = roaming(recs);
            recs[at]["p"][0] = json!(recs[at]["p"][0].as_f64().unwrap() + 3.0);
        }),
        ("UpdateSlowRotation's DesiredOrientation", |recs| {
            let at = roaming(recs);
            recs.remove(at);
        }),
        ("LookAt of a rider", |recs| {
            let at = roaming(recs);
            recs[at]["rider"] = json!(true);
        }),
        ("a write of a unit the sim doesn't have", |recs| {
            let at = roaming(recs);
            recs[at]["u"] = json!("no-such-unit");
        }),
        ("an ag_look it can't read", |recs| {
            let at = roaming(recs);
            recs[at]["fn"] = json!("Desired");
        }),
        // A caller not identified (empty, or WrathOracle's "?" when it can't walk the stack).
        ("an ag_look it can't read", |recs| {
            let at = roaming(recs);
            recs[at]["origin"] = json!("");
        }),
        ("an ag_look it can't read", |recs| {
            let at = roaming(recs);
            recs[at]["origin"] = json!("?");
        }),
        // The attack of opportunity's write by another caller: still the gated write, checked.
        ("(UnitAttackOfOpportunity.OnRun): the recording has", |recs| {
            let at = by(recs, "UnitAttackOfOpportunity.OnRun");
            recs[at]["origin"] = json!("Ns.X.Y < Ns.Z.W");
        }),
        // A turn-based write before the agents' synchronization (WrathOracle writes it first).
        ("without the agents' synchronization", |recs| {
            let s = pos(recs, |r| ev(r, "ag_sync"));
            let mut w = recs[roaming(recs)].clone();
            w["t"] = recs[s]["t"].clone();
            recs.insert(s, w);
        }),
        // A setter of code the sim doesn't run, its result not the value set.
        ("DesiredOrientation by X.Y: the result differs", |recs| {
            let at = roaming(recs);
            let mut w = recs[at].clone();
            w["fn"] = json!("DesiredOrientation");
            w["p"] = V::Null;
            w["v"] = json!(12.5);
            w["d0"] = w["d"].clone();
            w["o0"] = w["o"].clone();
            w["origin"] = json!("Ns.X.Y < Ns.Z.W");
            recs.insert(at + 1, w);
        }),
        // A read the sim's writes don't explain.
        ("UpdateSlowRotation's DesiredOrientation", |recs| {
            let at = later(recs, |r| ev(r, "ag_rot"));
            bump(&mut recs[at], "desired");
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

/// Every run of the captures with `look`: valid (tb-lizard: no report of the agent, the commands
/// or orientation), every `DesiredOrientation` read the sim's,
/// the commands' writes computed, nothing taken in turn-based combat but the outside callers'
/// arguments.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18, the caves' nav and physics files and the poc5-tb-131 and -132 captures"]
fn every_desired_orientation_read_is_the_sims() {
    let mut failed = Vec::new();
    for (capture, fight) in
        [("poc5-tb-132", "tb-nearest"), ("poc5-tb-132", "tb-focus"), ("poc5-tb-131", "tb-lizard")]
    {
        let Some(dir) = fixture_in(capture, fight) else {
            eprintln!("{capture}/{fight} not present: skipped");
            continue;
        };
        for run in ["record", "replay-1", "replay-2"] {
            let (out, w) = replay::run_fixture(SAVE, &dir, run).expect("replay");
            let l = &out.look;
            // (tb-lizard's other reports are other ports': its walls' and crates' colliders,
            // `Unverified` until the mesh and box raycasts land, the dropped loot, a prone
            // rider's turn.)
            let other: Vec<&String> = w
                .coverage
                .unsupported
                .keys()
                .filter(|k| {
                    fight != "tb-lizard"
                        || ["look", "rientation", "agent:", "commands:"]
                            .iter()
                            .any(|x| k.contains(x))
                })
                .collect();
            // (tb-lizard's end differs on the dropped loot: another port's.)
            let ok = other.is_empty()
                && (fight == "tb-lizard" || (w.valid() && out.diff.equal))
                && l.reads_checked > 5_000
                && l.reads_taken == 0
                && l.computed.values().sum::<usize>() > 500
                && !out.ag_inputs.contains_key("DesiredOrientation");
            if !ok {
                failed.push(format!("{capture}/{fight} {run}: {other:?} {l:?}"));
            }
        }
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
