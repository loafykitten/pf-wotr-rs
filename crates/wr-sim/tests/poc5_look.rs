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
        // The roaming controller's `LookAt` (the sim's since its port): its state before, its
        // result, its point, missing, a rider's, of another unit.
        ("(UnitRoamingController.TickOnUnit): the sim's position", |recs| {
            let at = roaming(recs);
            bump(&mut recs[at], "d0");
        }),
        ("(UnitRoamingController.TickOnUnit): the sim's position", |recs| {
            let at = roaming(recs);
            bump(&mut recs[at], "d");
        }),
        ("(UnitRoamingController.TickOnUnit): the sim's argument", |recs| {
            let at = roaming(recs);
            recs[at]["p"][0] = json!(recs[at]["p"][0].as_f64().unwrap() + 3.0);
        }),
        ("LookAt: the recording has", |recs| {
            let at = roaming(recs);
            recs.remove(at);
        }),
        ("(UnitRoamingController.TickOnUnit): a rider's", |recs| {
            let at = roaming(recs);
            recs[at]["rider"] = json!(true);
        }),
        ("(UnitRoamingController.TickOnUnit): the recording has", |recs| {
            let at = roaming(recs);
            recs[at]["u"] = json!("no-such-unit");
        }),
        // A write by code the sim doesn't run, of a unit it doesn't have.
        ("a write of a unit the sim doesn't have", |recs| {
            let at = roaming(recs);
            let mut w = recs[at].clone();
            w["u"] = json!("no-such-unit");
            w["origin"] = json!("Ns.X.Y < Ns.Z.W");
            recs.insert(at + 1, w);
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
            w["origin"] = json!("Ns.X.Y < Ns.Z.W");
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

/// The roaming controller's record at or after `t` (`roam_roll`, or by its caller chain).
fn roam_at(recs: &[V], t: i64, e: &str) -> usize {
    pos(recs, |r| {
        r["t"].as_i64().is_some_and(|x| x >= t)
            && ev(r, e)
            && (e == "roam_roll"
                || r["origin"].as_str().is_some_and(|o| {
                    o.split(" < ")
                        .find(|f| {
                            !f.starts_with("Kingmaker.UnitLogic.Commands.UnitCommands.Run")
                                && !f.ends_with("UnitEntityData.set_Position")
                        })
                        .is_some_and(|f| f.ends_with("UnitRoamingController.TickOnUnit"))
                }))
    })
}

/// The roaming move's `uc_run` in turn-based combat (after the synchronization).
fn roam_run(recs: &[V]) -> usize {
    let s = pos(recs, |r| ev(r, "ag_sync"));
    s + roam_at(&recs[s..], 0, "uc_run")
}

/// A roll in turn-based combat (after the synchronization).
fn roam_roll(recs: &[V]) -> usize {
    let s = pos(recs, |r| ev(r, "ag_sync"));
    s + roam_at(&recs[s..], 0, "roam_roll")
}

/// The sleeping step's `ag_pos` (turn-based).
fn roam_step(recs: &[V]) -> usize {
    let s = pos(recs, |r| ev(r, "ag_sync"));
    s + roam_at(&recs[s..], 0, "ag_pos")
}

/// The window's first roll (real time): its circle draw.
fn roll_draw(recs: &[V]) -> usize {
    pos(recs, |r| ev(r, "roam_roll")) + 1
}

/// Every roaming record the sim's controller makes, damaged: each reported. (The tape is the
/// damaged trace's where the case says so: a draw's value.)
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18, the caves' nav and physics files and the poc5-tb-132 capture"]
fn missing_or_wrong_roaming_records_are_reported() {
    let Some(dir) = fixture_in(LOOK.0, LOOK.1) else {
        eprintln!("{LOOK:?} not present: skipped");
        return;
    };
    let cases: Vec<(&str, bool, Change)> = vec![
        // A roll: missing, of another unit or settings, idle cutscenes, a draw missing or
        // elsewhere, a draw's value (the point the step and the move go to).
        ("RollNextPoint's roam_roll: the recording has", false, |recs| {
            let at = roam_roll(recs);
            recs.remove(at);
        }),
        ("RollNextPoint of 5052: the recording has", false, |recs| {
            let at = roam_roll(recs);
            recs[at]["unit"] = json!("5198");
        }),
        ("RollNextPoint of 5052: the recording has", false, |recs| {
            let at = roam_roll(recs);
            recs[at]["settings"] = json!(false);
        }),
        ("RollNextPoint of 5052: the recording has", false, |recs| {
            let at = roam_roll(recs);
            recs[at]["idle"] = json!(["0123456789abcdef0123456789abcdef"]);
        }),
        ("RollNextPoint of 5052: the recording has", false, |recs| {
            let at = roam_roll(recs);
            recs[at]["extra"] = json!(1);
        }),
        ("RollNextPoint's draw at", false, |recs| {
            let at = roam_roll(recs);
            recs.remove(at + 1);
        }),
        ("RollNextPoint's draw at", false, |recs| {
            let at = roam_roll(recs);
            recs.swap(at + 1, at + 2);
        }),
        ("the sleeping step of 5052: the sim's", true, |recs| {
            let at = roll_draw(recs);
            recs[at]["v"][0] = json!(recs[at]["v"][0].as_f64().unwrap() * 0.5);
        }),
        // A draw's record unlike the sim's draw: its value, its arguments, its kind.
        ("RollNextPoint's draws: the sim's", false, |recs| {
            let at = roam_roll(recs) + 2;
            recs[at]["v"] = json!(recs[at]["v"].as_f64().unwrap() + 0.25);
        }),
        ("RollNextPoint's draws: the sim's", false, |recs| {
            let at = roam_roll(recs) + 2;
            recs[at]["a"] = json!([3.0, 8.0, 999.0]);
        }),
        ("RollNextPoint's draws: the sim's", false, |recs| {
            let at = roam_roll(recs) + 1;
            recs[at]["v"][1] = json!(recs[at]["v"][1].as_f64().unwrap() * 0.5);
        }),
        ("RollNextPoint's draw at", false, |recs| {
            let at = roam_roll(recs) + 1;
            recs[at]["c"] = json!("pres");
        }),
        // An idle time outside `Range`'s bounds, on the tape too.
        ("RollNextPoint's draws: the sim's", true, |recs| {
            let at = roam_roll(recs) + 2;
            recs[at]["v"] = json!(1000.0);
        }),
        // A roll the sim's controller didn't make (another frame, or an extra one).
        ("which the sim's didn't make", false, |recs| {
            let at = roam_roll(recs);
            let mut r = recs[at].clone();
            r["t"] = json!(r["t"].as_i64().unwrap() + 1);
            let next = pos(recs, |x| x["k"] == "tick" && x["t"] == r["t"]);
            recs.insert(next + 1, r);
        }),
        // The sleeping step: missing, from or to another position, of another unit, unreadable,
        // a frame early (the idle isn't over), duplicated.
        ("the sleeping step's ag_pos: the recording has", false, |recs| {
            let at = roam_step(recs);
            recs.remove(at);
        }),
        ("the sleeping step of 5052: the sim's", false, |recs| {
            let at = roam_step(recs);
            recs[at]["v"][0] = json!(recs[at]["v"][0].as_f64().unwrap() + 0.001);
        }),
        ("the sleeping step of 5052: the sim's position", false, |recs| {
            let at = roam_step(recs);
            recs[at]["pos"][2] = json!(recs[at]["pos"][2].as_f64().unwrap() + 0.25);
        }),
        ("the sleeping step of 5052: the recording's is 5198's", false, |recs| {
            let at = roam_step(recs);
            recs[at]["u"] = json!("5198");
        }),
        ("the sleeping step of 5052: a record it can't read", false, |recs| {
            let at = roam_step(recs);
            recs[at]["v"] = json!(null);
        }),
        ("which the sim's didn't make", false, |recs| {
            let at = roam_step(recs);
            let t = recs[at]["t"].as_i64().unwrap();
            // (Its `ag_pos` and `ag_look`, moved to the end of the frame before.)
            let moved: Vec<V> = recs.drain(at..at + 2).collect();
            let next = pos(recs, |x| x["k"] == "tick" && x["t"] == json!(t));
            for (n, mut r) in moved.into_iter().enumerate() {
                r["t"] = json!(t - 1);
                recs.insert(next + n, r);
            }
        }),
        ("which the sim's didn't make", false, |recs| {
            let at = roam_step(recs);
            let copy = recs[at].clone();
            recs.insert(at + 2, copy);
        }),
        // The roaming move: to another point, not roaming, another approach radius, missing,
        // of another unit.
        ("the roaming move of 5052: the sim's UnitMoveTo", false, |recs| {
            let at = roam_run(recs) - 1;
            recs[at]["pt"][0] = json!(recs[at]["pt"][0].as_f64().unwrap() + 0.5);
        }),
        ("the roaming move of 5052: the sim's UnitMoveTo", false, |recs| {
            let at = roam_run(recs) - 1;
            recs[at]["mv"] = json!("000|-|-|0");
        }),
        ("the roaming move of 5052: the sim's UnitMoveTo", false, |recs| {
            let at = roam_run(recs) - 1;
            recs[at]["ap"] = json!("2|1000000|10");
        }),
        // Not a fresh `UnitMoveTo` (a cooldown time limit, a player's), or not `Run(cmd)`'s
        // flags.
        ("the roaming move of 5052: the sim's UnitMoveTo", false, |recs| {
            let at = roam_run(recs) - 1;
            recs[at]["ign_before"] = json!(1000);
        }),
        ("the roaming move of 5052: the sim's UnitMoveTo", false, |recs| {
            let at = roam_run(recs) - 1;
            recs[at]["player"] = json!(true);
        }),
        ("the roaming move of 5052: the recording runs", false, |recs| {
            let at = roam_run(recs);
            recs[at]["ncq"] = json!(true);
        }),
        ("the roaming move of 5052: the recording runs", false, |recs| {
            let at = roam_run(recs);
            recs[at]["fq"] = json!(true);
        }),
        ("the roaming move's uc_new: the recording has", false, |recs| {
            let at = roam_run(recs);
            recs.remove(at - 1);
        }),
        ("the roaming move of 5052: the recording runs", false, |recs| {
            let at = roam_run(recs);
            recs[at]["c"] = json!(recs[at]["c"].as_u64().unwrap() + 1000);
        }),
        // The window's start: the saved state not confirmed by the running roaming move.
        ("state isn't known (the save's unconfirmed", false, |recs| {
            let at = pos(recs, |r| ev(r, "uc_new") && r["mv"] == "100|-|-|0");
            recs[at]["pt"][0] = json!(recs[at]["pt"][0].as_f64().unwrap() + 0.01);
        }),
    ];
    let base = {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let out = replay::run_with_tape(&mut w, &trace, &trace);
        assert!(w.valid(), "the undamaged run: {:?}", w.coverage.unsupported.keys());
        let r = &out.roam;
        assert!(r.rolls == 2 && r.steps == 1 && r.looks == 1 && r.moves == 1, "{r:?}");
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
                    let Some((want, own_tape, change)) = cases.get(i) else { break };
                    let (mut w, trace) =
                        replay::prepare_world(SAVE, &dir, "record").expect("prepare");
                    let changed = trace.edited(|recs| {
                        change(recs);
                        for (n, r) in recs.iter_mut().enumerate() {
                            r["seq"] = json!(n);
                        }
                    });
                    let tape = if *own_tape { &changed } else { &trace };
                    let _ = replay::run_with_tape(&mut w, &changed, tape);
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
