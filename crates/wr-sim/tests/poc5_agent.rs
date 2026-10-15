//! POC-5's driven replay with the movement agent the sim's (`docs/poc5/movement.md`, "The
//! driven replay: hardening"): every way of losing, changing or adding an agent record is
//! reported. Ignored by default (needs the game's blueprint pack, Manual_18, the area's nav file
//! and the capture; skipped when the capture is absent).
//! `cargo nextest run -p wr-sim --test poc5_agent --run-ignored only`.

use std::path::PathBuf;

use serde_json::{Value as V, json};
use wr_sim::combat::{oracle::Trace, replay};

const SAVE: &str = "Manual_18_WR_FirstBattle";

/// The capture with WrathOracle's `agent_sync` the agent's records are damaged in.
const AGENT: (&str, &str) = ("poc5-tb-113", "tb-nearest");

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

fn leaf(r: &V, f: &str) -> bool {
    ev(r, "ag_v") && r["f"] == f
}

fn renumber(recs: &mut [V]) {
    for (n, r) in recs.iter_mut().enumerate() {
        r["seq"] = json!(n);
    }
}

fn pos(recs: &[V], f: impl Fn(&V) -> bool) -> usize {
    recs.iter().position(f).expect("a record to change")
}

/// The window's synchronization (where real time ends).
fn sync(recs: &[V]) -> usize {
    pos(recs, |r| ev(r, "ag_sync"))
}

/// The first record matching `f` after the synchronization (turn-based, the agent the sim's).
fn after(recs: &[V], f: impl Fn(&V) -> bool) -> usize {
    let s = sync(recs);
    s + recs[s..].iter().position(f).expect("a record to change")
}

/// The record matching `f` some frames into the fight (past its start's UI).
fn later(recs: &[V], f: impl Fn(&V) -> bool) -> usize {
    let t = recs[sync(recs)]["t"].as_i64().unwrap() + 30;
    after(recs, |r| r["t"].as_i64() > Some(t) && f(r))
}

/// A `TickUnit` of a unit standing (no `TickMovement`): its `ag_ctl`.
fn standing(recs: &[V]) -> usize {
    let mut at = 0;
    loop {
        at += 1 + recs[at + 1..].iter().position(|r| ev(r, "ag_ctl")).unwrap();
        let t = recs[sync(recs)]["t"].as_i64().unwrap() + 30;
        if recs[at]["t"].as_i64() > Some(t) && ev(&recs[at + 1], "ag_rot") {
            return at;
        }
    }
}

/// A `TickUnit` that ticks the agent (`ag_tick` inside): its `ag_ctl`.
fn moving(recs: &[V]) -> usize {
    let tick = later(recs, |r| ev(r, "ag_tick"));
    (0..tick).rev().find(|j| ev(&recs[*j], "ag_ctl")).unwrap()
}

/// The span `at` ..= its end record `end` of the same unit.
fn span_end(recs: &[V], at: usize, end: &str) -> usize {
    let u = recs[at]["u"].clone();
    at + recs[at..].iter().position(|r| ev(r, end) && r["u"] == u).unwrap()
}

/// A `PathTo` of the commands that made a request: its `ag_req`.
fn path_to_made(recs: &[V]) -> usize {
    let s = sync(recs);
    (s..recs.len())
        .find(|j| {
            ev(&recs[*j], "ag_req")
                && recs[*j]["fn"] == "pathto"
                && recs[*j + 1..]
                    .iter()
                    .find(|r| ev(r, "path_start") || ev(r, "ag_req_end"))
                    .is_some_and(|r| ev(r, "path_start"))
        })
        .expect("a request")
}

#[test]
#[ignore = "needs the game's blueprint pack, Manual_18, the caves' nav file and the poc5-tb-113 capture"]
fn missing_or_wrong_agent_records_are_reported() {
    let Some(dir) = fixture_in(AGENT.0, AGENT.1) else {
        eprintln!("{AGENT:?} not present: skipped");
        return;
    };
    type Change = fn(&mut Vec<V>);
    const A: &str = "driven replay: agent: ";
    let cases: Vec<(&str, Change)> = vec![
        // The capability and the initial states.
        ("driven replay: the recording declares agent_sync without", |recs| {
            let b = pos(recs, |r| r["k"] == "begin");
            let obs: Vec<V> = recs[b]["observers"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|x| *x != "agent")
                .cloned()
                .collect();
            recs[b]["observers"] = json!(obs);
        }),
        ("a recording declaring agent_sync without its initial states", |recs| {
            recs.retain(|r| !ev(r, "ag_init"));
            renumber(recs);
        }),
        ("ag_init: an agent's AllAgents index out of order", |recs| {
            let at = pos(recs, |r| ev(r, "ag_init"));
            recs[at]["i"] = json!(1);
        }),
        ("ag_init: an agent twice in its block", |recs| {
            let at = pos(recs, |r| ev(r, "ag_init"));
            let r = recs[at].clone();
            recs.insert(at + 1, r);
            renumber(recs);
        }),
        ("an ag_init record with fields its writer doesn't write", |recs| {
            let at = pos(recs, |r| ev(r, "ag_init"));
            recs[at].as_object_mut().unwrap().remove("prev");
        }),
        // The synchronization where real time ends.
        ("without a synchronization", |recs| {
            recs.retain(|r| !ev(r, "ag_sync"));
            renumber(recs);
        }),
        ("a synchronization without one of the agents", |recs| {
            let s = sync(recs);
            let last = s + recs[s..].iter().position(|r| !ev(r, "ag_sync")).unwrap() - 1;
            recs.remove(last);
            renumber(recs);
        }),
        ("AllAgents at the synchronization differ", |recs| {
            let s = sync(recs);
            recs.swap(s, s + 1);
            recs[s]["i"] = json!(0);
            recs[s + 1]["i"] = json!(1);
            renumber(recs);
        }),
        ("an ag_sync record it can't read", |recs| {
            let s = sync(recs);
            recs[s]["desired"] = json!(null);
        }),
        ("the sim's state differs from the recording's (accel", |recs| {
            let s = sync(recs);
            recs[s]["s"]["accel"] = json!(21.0);
        }),
        ("a synchronization (ag_sync) where real time didn't end", |recs| {
            // The block again, a frame into the fight.
            let s = sync(recs);
            let end = s + recs[s..].iter().position(|r| !ev(r, "ag_sync")).unwrap();
            let t = recs[s]["t"].as_i64().unwrap() + 40;
            let at = pos(recs, |r| r["t"].as_i64() == Some(t) && ev(r, "turn_tick"));
            let mut block: Vec<V> = recs[s..end].to_vec();
            for r in &mut block {
                r["t"] = json!(t);
            }
            for (k, r) in block.into_iter().enumerate() {
                recs.insert(at + 1 + k, r);
            }
            renumber(recs);
        }),
        ("OnPathComplete(", |recs| {
            // A constructed ForcedPath's completion with an error.
            let at = after(recs, |r| {
                ev(r, "ag_done") && r["path"].as_str().is_some_and(|p| p.starts_with("?ForcedPath"))
            });
            recs[at]["err"] = json!(true);
        }),
        ("a synchronization (ag_sync) outside turn-based combat", |recs| {
            // A second one, in real time (before the fight).
            let s = sync(recs);
            let mut r = recs[s].clone();
            let at = pos(recs, |r| ev(r, "ag_ctl") && r["t"].as_i64() > Some(5));
            r["t"] = recs[at]["t"].clone();
            recs.insert(at, r);
            renumber(recs);
        }),
        ("where real time didn't end", |recs| {
            // A synchronization row moved out of its block.
            let s = sync(recs);
            let last = s + recs[s..].iter().position(|r| !ev(r, "ag_sync")).unwrap() - 1;
            let r = recs.remove(last);
            recs.insert(last + 2, r);
            renumber(recs);
        }),
        ("OnPathComplete of ?ForcedPath", |recs| {
            let at = after(recs, |r| {
                ev(r, "ag_done") && r["path"].as_str().is_some_and(|p| p.starts_with("?ForcedPath"))
            });
            recs[at]["v"][0][0] = json!(999.0);
        }),
        ("OnPathComplete of ?ForcedPath", |recs| {
            let at = after(recs, |r| {
                ev(r, "ag_done") && r["path"].as_str().is_some_and(|p| p.starts_with("?ForcedPath"))
            });
            recs[at]["cs"] = json!(1);
        }),
        ("IsDone of the constructed", |recs| {
            let at = after(recs, |r| {
                leaf(r, "isdone")
                    && r["path"].as_str().is_some_and(|p| p.starts_with("?ForcedPath"))
            });
            recs[at]["v"] = json!(true);
        }),
        // UnitMoveController's batch.
        ("TickUnit: PreviousPosition", |recs| {
            let at = standing(recs);
            recs[at]["prev"][0] = json!(1.0);
        }),
        ("TickUnit: the unit's position", |recs| {
            let at = standing(recs);
            recs[at]["pos"][0] = json!(1.0);
        }),
        ("a TickUnit record it can't read", |recs| {
            let at = standing(recs);
            recs[at]["extra"] = json!(1);
        }),
        ("TickUnit's end: the sim's state differs from the recording's (vel", |recs| {
            let at = standing(recs);
            let e = span_end(recs, at, "ag_ctl_end");
            recs[e]["s"]["vel"][0] = json!(0.5);
        }),
        ("UnitMoveController", |recs| {
            // A standing unit's whole TickUnit gone.
            let at = standing(recs);
            let e = span_end(recs, at, "ag_ctl_end");
            recs.drain(at..=e);
            renumber(recs);
        }),
        ("UnitMoveController ticks", |recs| {
            // Two units' TickUnits swapped.
            let at = standing(recs);
            let e = span_end(recs, at, "ag_ctl_end");
            let b = span_end(recs, e + 1, "ag_ctl_end");
            assert!(ev(&recs[e + 1], "ag_ctl"));
            let first: Vec<V> = recs.drain(at..=e).collect();
            let at2 = b - first.len() + 1;
            for (k, r) in first.into_iter().enumerate() {
                recs.insert(at2 + k, r);
            }
            renumber(recs);
        }),
        ("UnitMoveController ticks", |recs| {
            // A TickUnit while WaitingForUI (the fight's start window: no controller tick).
            let s = sync(recs);
            let t = recs[s]["t"].as_i64().unwrap();
            let frame = (t..t + 10)
                .find(|f| {
                    recs.iter().any(|r| r["t"].as_i64() == Some(*f) && ev(r, "turn_tick"))
                        && !recs.iter().any(|r| r["t"].as_i64() == Some(*f) && ev(r, "ag_ctl"))
                })
                .unwrap();
            let at = (0..s)
                .rev()
                .find(|j| ev(&recs[*j], "ag_ctl") && ev(&recs[*j + 1], "ag_rot"))
                .unwrap();
            let e = span_end(recs, at, "ag_ctl_end");
            let mut span: Vec<V> = recs[at..=e].to_vec();
            for r in &mut span {
                r["t"] = json!(frame);
            }
            let ins = 1 + pos(recs, |r| r["t"].as_i64() == Some(frame) && ev(r, "turn_tick"));
            for (k, r) in span.into_iter().enumerate() {
                recs.insert(ins + k, r);
            }
            renumber(recs);
        }),
        ("canrotate without a boolean v", |recs| {
            let at = after(recs, |r| leaf(r, "canrotate"));
            recs[at].as_object_mut().unwrap().remove("v");
        }),
        // TickMovement and CanMoveInTurnBased.
        ("CanMoveInTurnBased returned", |recs| {
            let at = moving(recs);
            let c = at + recs[at..].iter().position(|r| leaf(r, "canmovetb")).unwrap();
            recs[c]["dt"] = json!(0.5);
        }),
        ("CanMoveInTurnBased returned", |recs| {
            let at = moving(recs);
            let c = at + recs[at..].iter().position(|r| leaf(r, "canmovetb")).unwrap();
            let v = recs[c]["v"].as_bool().unwrap();
            recs[c]["v"] = json!(!v);
        }),
        ("TickMovement's turn state", |recs| {
            let at = later(recs, |r| ev(r, "ag_tick"));
            let v = recs[at]["cur"].as_bool().unwrap();
            recs[at]["cur"] = json!(!v);
        }),
        ("TickMovement's turn state", |recs| {
            let at = later(recs, |r| ev(r, "ag_tick"));
            let v = recs[at]["acting"].as_bool().unwrap();
            recs[at]["acting"] = json!(!v);
        }),
        ("TickMovement's turn state", |recs| {
            let at = later(recs, |r| ev(r, "ag_tick"));
            recs[at]["dontint"] = json!(true);
        }),
        ("TickMovement's turn state", |recs| {
            let at = later(recs, |r| ev(r, "ag_tick"));
            recs[at]["tb"] = json!(1);
        }),
        ("isdone without a boolean v", |recs| {
            let at = moving(recs);
            let c = at + recs[at..].iter().position(|r| leaf(r, "isdone")).unwrap();
            recs[c]["v"] = json!("true");
        }),
        // SlowDown's draws.
        ("a SlowDown draw the sim's agent didn't make", |recs| {
            let d = pos(recs, |r| {
                r["k"] == "draw"
                    && r["m"] == "Range_ff"
                    && r["a"] == json!([0.2, 0.3])
                    && r["c"] == "sim"
            });
            let at = later(recs, |r| ev(r, "leave_tick"));
            let mut r = recs[d].clone();
            r["t"] = recs[at]["t"].clone();
            recs.insert(at + 1, r);
            renumber(recs);
        }),
        ("SlowDown's draw", |recs| {
            let d = pos(recs, |r| {
                r["k"] == "draw"
                    && r["m"] == "Range_ff"
                    && r["a"] == json!([0.2, 0.3])
                    && r["c"] == "sim"
                    && r["t"].as_i64()
                        > recs.iter().find(|x| ev(x, "ag_sync")).unwrap()["t"].as_i64()
            });
            recs.remove(d);
            renumber(recs);
        }),
        // AllAgents.
        ("AllAgents' index after an enable or disable", |recs| {
            let at = pos(recs, |r| ev(r, "ag_enable"));
            recs[at]["i"] = json!(99);
        }),
        // Requests.
        ("the request", |recs| {
            let at = path_to_made(recs);
            let p = at + recs[at..].iter().position(|r| ev(r, "path_start")).unwrap();
            recs[p]["path"] = json!("5052#9");
        }),
        ("the request", |recs| {
            let at = path_to_made(recs);
            let p = at + recs[at..].iter().position(|r| ev(r, "path_start")).unwrap();
            recs[p]["e"][0] = json!(1.0);
        }),
        ("the request", |recs| {
            let at = path_to_made(recs);
            let e = span_end(recs, at, "ag_req_end");
            recs[e]["rq"]["tags"] = json!(7);
        }),
        ("the request's dsame", |recs| {
            let at = later(recs, |r| ev(r, "ag_req") && r["fn"] == "pathto");
            let v = recs[at]["dsame"].as_bool().unwrap();
            recs[at]["dsame"] = json!(!v);
        }),
        ("the request", |recs| {
            let at =
                after(recs, |r| ev(r, "ag_req_end") && r["fn"] == "find" && !r["rq"].is_null());
            recs[at]["rq"]["maxd"] = json!(1.0);
        }),
        ("the request's start: the sim's state differs", |recs| {
            let at = after(recs, |r| ev(r, "ag_req") && r["fn"] == "find");
            recs[at]["s"]["remain"] = json!(9.0);
        }),
        ("FindPath's callback of a path the sim's FindPath didn't request", |recs| {
            let at = after(recs, |r| ev(r, "ag_findclr"));
            recs[at]["path"] = json!("5052#9");
        }),
        ("FindPath's callback of a path the sim's FindPath didn't request", |recs| {
            let at = after(recs, |r| ev(r, "ag_findclr"));
            let ret = (0..at).rev().find(|j| ev(&recs[*j], "path_ret")).unwrap();
            recs.remove(ret);
            renumber(recs);
        }),
        ("FindPath's callback of a path the sim's FindPath didn't request", |recs| {
            let at = after(recs, |r| ev(r, "ag_findclr"));
            let mut r = recs[at].clone();
            r["held"] = json!(null);
            recs.insert(at + 1, r);
            renumber(recs);
        }),
        ("FindPath's callback", |recs| {
            let at = after(recs, |r| ev(r, "ag_findclr"));
            recs[at]["held"] = json!("5052#9");
        }),
        ("without its delivery", |recs| {
            let at =
                after(recs, |r| ev(r, "ag_done") && !r["path"].as_str().unwrap().starts_with('?'));
            let ret = (0..at).rev().find(|j| ev(&recs[*j], "path_ret")).unwrap();
            recs.remove(ret);
            renumber(recs);
        }),
        // The commands' FollowPrecomputedPath of a new ForcedPath.
        ("FollowPrecomputedPath of the command's path", |recs| {
            let at = after(recs, |r| ev(r, "ag_call") && r["fn"] == "FollowPrecomputedPath");
            let key = recs[at]["rq"]["path"].clone();
            let d = (0..at)
                .rev()
                .find(|j| ev(&recs[*j], "ag_pathdef") && recs[*j]["path"] == key)
                .unwrap();
            recs[d]["err"] = json!(true);
        }),
        ("FollowPrecomputedPath of the command's path", |recs| {
            let at = after(recs, |r| ev(r, "ag_call") && r["fn"] == "FollowPrecomputedPath");
            recs[at]["args"][1] = json!("3.5");
        }),
        ("FollowPrecomputedPath of the command's path", |recs| {
            let at = after(recs, |r| ev(r, "ag_call") && r["fn"] == "FollowPrecomputedPath");
            recs[at]["rq"]["tags"] = json!(7);
        }),
        ("FollowPrecomputedPath of the command's path", |recs| {
            let at = after(recs, |r| ev(r, "ag_call") && r["fn"] == "FollowPrecomputedPath");
            recs[at]["rq"]["xz"] = json!(true);
        }),
        ("FollowPrecomputedPath of the command's path", |recs| {
            let at = after(recs, |r| ev(r, "ag_call") && r["fn"] == "FollowPrecomputedPath");
            let key = recs[at]["rq"]["path"].clone();
            let d = (0..at)
                .rev()
                .find(|j| ev(&recs[*j], "ag_pathdef") && recs[*j]["path"] == key)
                .unwrap();
            recs[d]["kind"] = json!("ABPathWithClearance");
        }),
        ("isn't a new ForcedPath's name", |recs| {
            let at = after(recs, |r| ev(r, "ag_call") && r["fn"] == "FollowPrecomputedPath");
            recs[at]["rq"]["path"] = json!("5052#0");
        }),
        // The rows and the getters.
        ("an observation row's position differs", |recs| {
            let at = later(recs, |r| ev(r, "u") && r["u"] != "~fx");
            let s = recs[at]["s"].as_str().unwrap().to_owned();
            let (_, rest) = s.split_once(',').unwrap();
            recs[at]["s"] = json!(format!("1.5,{rest}"));
        }),
        ("an observation row's IsMoving differs", |recs| {
            let at = later(recs, |r| {
                ev(r, "u") && r["s"].as_str().is_some_and(|s| s.contains("|mv=1|"))
            });
            let s = recs[at]["s"].as_str().unwrap().replace("|mv=1|", "|mv=0|");
            recs[at]["s"] = json!(s);
        }),
        ("the sim's agent has moving", |recs| {
            let at = later(recs, |r| ev(r, "uc_v") && r["f"] == "moving");
            let v = recs[at]["v"].as_bool().unwrap();
            recs[at]["v"] = json!(!v);
        }),
        ("the sim's agent has rmoving", |recs| {
            let at = later(recs, |r| ev(r, "uc_v") && r["f"] == "rmoving");
            let v = recs[at]["v"].as_bool().unwrap();
            recs[at]["v"] = json!(!v);
        }),
        ("ContinueActing: the agent's WantsToMove", |recs| {
            let at = later(recs, |r| ev(r, "tt_ca") && r["wants"].is_boolean());
            let v = recs[at]["wants"].as_bool().unwrap();
            recs[at]["wants"] = json!(!v);
        }),
        ("ContinueActing: the agent's WantsToMove", |recs| {
            let at = later(recs, |r| ev(r, "tt_ca") && r["motion"].is_boolean());
            let v = recs[at]["motion"].as_bool().unwrap();
            recs[at]["motion"] = json!(!v);
        }),
        // Positions written outside the agent.
        ("position", |recs| {
            let at = pos(recs, |r| ev(r, "ag_pos"));
            recs.remove(at);
            renumber(recs);
        }),
        ("the position before differs", |recs| {
            let at = pos(recs, |r| ev(r, "ag_pos"));
            recs[at]["pos"][0] = json!(1.0);
        }),
        ("the place-on-ground step starts from another position", |recs| {
            let at = after(recs, |r| ev(r, "ag_ground"));
            recs[at]["pos"][0] = json!(1.0);
        }),
        ("a position written inside the agent's controllers", |recs| {
            // Inside a standing unit's TickUnit, masking a changed end position.
            let at = standing(recs);
            let e = span_end(recs, at, "ag_ctl_end");
            let mut r = recs[pos(recs, |r| ev(r, "ag_pos"))].clone();
            r["t"] = recs[at]["t"].clone();
            r["u"] = recs[at]["u"].clone();
            r["pos"] = recs[at]["pos"].clone();
            r["v"] = json!([1.0, 2.0, 3.0]);
            recs.insert(e, r);
            renumber(recs);
        }),
        ("a delivery (path_ret)", |recs| {
            let at = after(recs, |r| ev(r, "path_ret"));
            recs[at]["req"] = json!(-1000);
        }),
        ("a delivery (path_ret)", |recs| {
            // The recording's last frame: a delivery without its callback.
            let at = after(recs, |r| ev(r, "path_ret"));
            let mut r = recs[at].clone();
            let last = recs.len() - 1;
            r["t"] = recs[last]["t"].clone();
            recs.push(r);
            renumber(recs);
        }),
        ("a delivery (path_ret)", |recs| {
            let at = after(recs, |r| ev(r, "path_ret"));
            let r = recs[at].clone();
            recs.insert(at + 1, r);
            renumber(recs);
        }),
        ("a delivery (path_ret)", |recs| {
            // A delivery without its callback, at a frame's end.
            let at = after(recs, |r| ev(r, "path_ret"));
            let mut r = recs[at].clone();
            r["path"] = json!("51DD#7");
            let t = recs[at]["t"].as_i64().unwrap() + 2;
            let ins = pos(recs, |x| x["k"] == "tick" && x["t"].as_i64() == Some(t));
            r["t"] = json!(t - 1);
            recs.insert(ins, r);
            renumber(recs);
        }),
        // The view's callbacks.
        ("the agent calls the view's", |recs| {
            let at = after(recs, |r| ev(r, "ag_cb"));
            recs[at]["i"] = json!(7);
        }),
        // A PathTo no code the sim runs made.
        ("a PathTo the sim's commands didn't make", |recs| {
            let at = path_to_made(recs);
            let e = span_end(recs, at, "ag_req_end");
            let span: Vec<V> = recs[at..=e].to_vec();
            let ins = 1 + at + recs[at..].iter().position(|r| ev(r, "leave_tick")).unwrap();
            for (k, r) in span.into_iter().enumerate() {
                recs.insert(ins + k, r);
            }
            renumber(recs);
        }),
        // A record of another frame.
        ("a record (ag_ctl) of another frame", |recs| {
            let at = standing(recs);
            recs[at]["t"] = json!(recs[at]["t"].as_i64().unwrap() + 1);
        }),
    ];
    let base = {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let _ = replay::run_with_tape(&mut w, &trace, &trace);
        w.coverage.unsupported.keys().cloned().collect::<Vec<_>>()
    };
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
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
                    let mut changed =
                        Trace { records: trace.records.clone(), sites: trace.sites.clone() };
                    change(&mut changed.records);
                    let _ = replay::run_with_tape(&mut w, &changed, &trace);
                    let new: Vec<&String> =
                        w.coverage.unsupported.keys().filter(|k| !base.contains(k)).collect();
                    let hit = new.iter().any(|k| {
                        k.contains(want)
                            && (want.starts_with("driven replay:")
                                || k.contains(A)
                                || k.starts_with("driven replay: commands:"))
                    });
                    if w.valid() || !hit {
                        missed.lock().unwrap().push(format!("case {i}: {want}: {new:?}"));
                    }
                }
            });
        }
    });
    let missed = missed.into_inner().unwrap();
    assert!(missed.is_empty(), "{missed:#?}");
}

/// The captures with `agent_sync`: every run of every fight valid with the agent driven, its
/// entries run, its states and getters checked, one synchronization per run, every physics query
/// the sim's and each movement simulation step run. `poc5-tb-114` ends its fight while the UI
/// guard is up: the guard drops inside the party's combat-state change, after
/// `UnitMoveController` (which ticks no unit that frame). With WrathOracle's `paths`
/// (`poc5-tb-114` onward) the grid's graph updates and the turn-based paths are the sim's too.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18, the caves' nav and physics files and the poc5-tb-113 to -117 captures"]
fn driven_agent_replays_are_valid() {
    let mut runs = Vec::new();
    for (capture, fight) in [
        ("poc5-tb-113", "tb-nearest"),
        ("poc5-tb-113", "tb-focus"),
        ("poc5-tb-114", "tb-nearest"),
        ("poc5-tb-114", "tb-focus"),
        ("poc5-tb-115", "tb-nearest"),
        ("poc5-tb-115", "tb-focus"),
        ("poc5-tb-116", "tb-hold"),
        ("poc5-tb-117", "tb-solo"),
    ] {
        let Some(dir) = fixture_in(capture, fight) else {
            eprintln!("{capture}/{fight} not present: skipped");
            continue;
        };
        for run in ["record", "replay-1", "replay-2"] {
            runs.push((
                format!("{capture}/{fight} {run}"),
                dir.clone(),
                run,
                capture != "poc5-tb-113",
            ));
        }
    }
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failed = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((name, dir, run, paths)) = runs.get(i) else { break };
                    let (out, w) = replay::run_fixture(SAVE, dir, run).expect("replay");
                    let n = &out.nav;
                    let ok = w.valid()
                        && out.diff.equal
                        && out.ag_entries > 10_000
                        && out.ag_ticks > 5_000
                        && out.ag_states > 10_000
                        && out.ag_command_calls > 100
                        && out.ag_reads > 1_000
                        && out.ag_syncs == 1
                        && n.phys_tick > 400
                        && n.phys_span > 300
                        && n.phys_taken == 0
                        && n.sim_steps > 40
                        && !out.ag_inputs.contains_key("physics")
                        && (!paths
                            || (n.paths >= 19
                                && n.los_answers > 10
                                && n.deliveries > 60
                                && n.delivered_to_agent >= 2
                                && n.graph_events > 100
                                && n.turn_starts >= 7
                                && n.removals >= 6
                                && n.sim_grid_checked > 90
                                && n.sim_grid_taken == 0
                                && !out.ag_inputs.contains_key("path contents")));
                    if !ok {
                        failed.lock().unwrap().push(format!(
                            "{name}: valid {}, {n:?}, {:?}",
                            w.valid(),
                            w.coverage.unsupported.keys().collect::<Vec<_>>()
                        ));
                    }
                }
            });
        }
    });
    let failed = failed.into_inner().unwrap();
    assert!(failed.is_empty(), "{failed:#?}");
}

/// The capture with WrathOracle's `paths` the physics and path records are damaged in.
const PATHS: (&str, &str) = ("poc5-tb-114", "tb-nearest");

type Change = fn(&mut Vec<V>);

/// Runs each damaged copy of `dir`'s record run; every case must make the run invalid with a new
/// report containing its text.
fn damage_cases(dir: &std::path::Path, cases: &[(&str, Change)]) {
    damage_cases_of(dir, "record", cases);
}

/// [`damage_cases`] on one run's trace.
fn damage_cases_of(dir: &std::path::Path, run: &str, cases: &[(&str, Change)]) {
    let base = {
        let (mut w, trace) = replay::prepare_world(SAVE, dir, run).expect("prepare");
        let _ = replay::run_with_tape(&mut w, &trace, &trace);
        assert!(w.valid(), "the undamaged run: {:?}", w.coverage.unsupported.keys());
        w.coverage.unsupported.keys().cloned().collect::<Vec<_>>()
    };
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let missed = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some((want, change)) = cases.get(i) else { break };
                    let (mut w, trace) = replay::prepare_world(SAVE, dir, run).expect("prepare");
                    let mut changed =
                        Trace { records: trace.records.clone(), sites: trace.sites.clone() };
                    change(&mut changed.records);
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

/// A turn-based record matching `f` (after the synchronization).
fn tb(recs: &[V], f: impl Fn(&V) -> bool) -> usize {
    after(recs, f)
}

/// The turn-based `path_fin` of a request the sim makes whose search asked line of sight.
fn fin_with_los(recs: &[V]) -> usize {
    tb(recs, |r| {
        ev(r, "path_fin")
            && r["los"].as_array().is_some_and(|a| !a.is_empty())
            && r["searched"].as_i64() > Some(0)
    })
}

/// The `path_fin` of the path delivered to the agent (`ag_done`) after the synchronization.
fn fin_of_done(recs: &[V]) -> usize {
    let done =
        tb(recs, |r| ev(r, "ag_done") && r["path"].as_str().is_some_and(|p| !p.starts_with('?')));
    let key = recs[done]["path"].clone();
    (0..done).rev().find(|j| ev(&recs[*j], "path_fin") && recs[*j]["path"] == key).unwrap()
}

/// The path's records `ev` after its `path_fin` at `fin`.
fn of_path(recs: &[V], fin: usize, e: &str) -> usize {
    let key = recs[fin]["path"].clone();
    fin + recs[fin..].iter().position(|r| ev(r, e) && r["path"] == key).unwrap()
}

/// A movement simulation step's records (`ag_sim` .. `ag_sim_end`) a while into the fight.
fn sim_step(recs: &[V]) -> (usize, usize) {
    let at = later(recs, |r| ev(r, "ag_sim"));
    (at, span_end(recs, at, "ag_sim_end"))
}

#[test]
#[ignore = "needs the game's blueprint pack, Manual_18, the caves' nav and physics files and the poc5-tb-114 capture"]
fn missing_or_wrong_physics_and_path_records_are_reported() {
    let Some(dir) = fixture_in(PATHS.0, PATHS.1) else {
        eprintln!("{PATHS:?} not present: skipped");
        return;
    };
    let cases: Vec<(&str, Change)> = vec![
        // Physics: the agent's queries, the simulation's, those of spans taken.
        ("driven replay: agent: a ray query from", |recs| {
            let at = later(recs, |r| ev(r, "ag_phys") && r["ctx"] == "tick");
            recs[at]["dist"] = json!(recs[at]["dist"].as_f64().unwrap() + 0.001);
        }),
        ("driven replay: agent: a ray query from", |recs| {
            let at = later(recs, |r| ev(r, "ag_phys") && r["ctx"] == "tick");
            recs[at]["pt"][1] = json!(recs[at]["pt"][1].as_f64().unwrap() + 0.001);
        }),
        ("driven replay: agent: a ray query from", |recs| {
            let at = later(recs, |r| ev(r, "ag_phys") && r["ctx"] == "tick");
            recs[at]["col"]["path"] = json!("Cube");
        }),
        ("driven replay: agent: a ray query from", |recs| {
            let at = later(recs, |r| ev(r, "ag_phys") && r["ctx"] == "tick");
            recs[at]["hit"] = json!(false);
        }),
        ("driven replay: physics: a ray query from", |recs| {
            // A query of a span taken from the recording (the walk-up in real time).
            let at = pos(recs, |r| ev(r, "ag_phys") && r["ctx"] == "tick");
            recs[at]["dist"] = json!(recs[at]["dist"].as_f64().unwrap() + 0.001);
        }),
        ("driven replay: agent: a ray query from", |recs| {
            let (at, end) = sim_step(recs);
            let q = at + recs[at..end].iter().position(|r| ev(r, "ag_phys")).unwrap();
            recs[q]["dist"] = json!(recs[q]["dist"].as_f64().unwrap() + 0.001);
        }),
        // The movement simulation's steps.
        ("driven replay: agent: the simulation step returned", |recs| {
            let (_, end) = sim_step(recs);
            recs[end]["sim"]["p"][0] = json!(recs[end]["sim"]["p"][0].as_f64().unwrap() + 0.001);
        }),
        ("driven replay: agent: the simulation step returned", |recs| {
            let (_, end) = sim_step(recs);
            recs[end]["ret"] = json!(!recs[end]["ret"].as_bool().unwrap());
        }),
        ("driven replay: agent: the simulation step returned", |recs| {
            let (_, end) = sim_step(recs);
            recs[end]["sim"]["len"] = json!(9.0);
        }),
        ("the simulation's AllyBlocked under", |recs| {
            let (at, end) = sim_step(recs);
            let q = at + recs[at..end].iter().position(|r| leaf(r, "near")).unwrap();
            recs[q]["v"] = json!(1);
        }),
        ("driven replay: agent: the simulation's maxspeed", |recs| {
            let (at, _) = sim_step(recs);
            recs.remove(at + 1);
            renumber(recs);
        }),
        ("the simulation step's end", |recs| {
            let (_, end) = sim_step(recs);
            recs.remove(end);
            renumber(recs);
        }),
        ("a movement simulation record outside its step", |recs| {
            let (at, end) = sim_step(recs);
            let q = at + recs[at..end].iter().position(|r| leaf(r, "near")).unwrap();
            let r = recs[q].clone();
            recs.insert(end + 1, r);
            renumber(recs);
        }),
        ("the simulation's HasObstacle", |recs| {
            let q = tb(recs, |r| leaf(r, "los") && r["ctx"] == "sim");
            recs[q]["r"] = json!(0.5);
        }),
        // Paths: the search, its line of sight, the modifiers, the deliveries.
        ("driven replay: paths: ", |recs| {
            let at = fin_with_los(recs);
            let n = recs[at]["raw"].as_array().unwrap().len();
            recs[at]["raw"][n - 1][0] = json!(recs[at]["raw"][n - 1][0].as_f64().unwrap() + 0.5);
        }),
        ("the sim's path (error", |recs| {
            let at = fin_with_los(recs);
            recs[at]["searched"] = json!(recs[at]["searched"].as_i64().unwrap() + 1);
        }),
        ("the sim's path (error", |recs| {
            let at = fin_of_done(recs);
            recs[at]["cs"] = json!(3);
        }),
        ("driven replay: paths: ", |recs| {
            // The line-of-sight answer the other way.
            let at = fin_with_los(recs);
            let b = recs[at]["los"][0][3].as_bool().unwrap();
            recs[at]["los"][0][3] = json!(!b);
        }),
        ("the game asked", |recs| {
            let at = fin_with_los(recs);
            recs[at]["los"][0][2] = json!(0.25);
        }),
        ("line of sight asked past the recorded answers", |recs| {
            let at = fin_with_los(recs);
            recs[at]["los"] = json!([]);
        }),
        ("isn't [from, to, fudge, result]", |recs| {
            let at = fin_with_los(recs);
            recs[at]["los"][0][3] = json!(null);
        }),
        ("delivered before its return (path_fin)", |recs| {
            let at = fin_of_done(recs);
            recs.remove(at);
            renumber(recs);
        }),
        ("the seeker's modifiers", |recs| {
            let at = of_path(recs, fin_of_done(recs), "path_mod");
            recs[at]["v"][1][2] = json!(recs[at]["v"][1][2].as_f64().unwrap() + 0.5);
        }),
        ("delivered before its modifiers (path_mod)", |recs| {
            let at = of_path(recs, fin_of_done(recs), "path_mod");
            recs.remove(at);
            renumber(recs);
        }),
        ("the seeker's decision", |recs| {
            let at = of_path(recs, fin_of_done(recs), "path_seek");
            recs[at]["cb"] = json!(false);
        }),
        ("the seeker's decision", |recs| {
            let at = of_path(recs, fin_of_done(recs), "path_seek");
            recs[at]["mods"] = json!(false);
        }),
        ("the delivered path differs from the sim's", |recs| {
            let at = of_path(recs, fin_of_done(recs), "ag_done");
            recs[at]["v"][1][0] = json!(recs[at]["v"][1][0].as_f64().unwrap() + 0.5);
        }),
        ("a definition before its return differs from the sim's", |recs| {
            let fin = fin_of_done(recs);
            let key = recs[fin]["path"].clone();
            let at = tb(recs, |r| ev(r, "ag_pathdef") && r["path"] == key && r["pending"] != true);
            recs[at]["v"][0][0] = json!(recs[at]["v"][0][0].as_f64().unwrap() + 0.5);
        }),
        ("ScorePath's path", |recs| {
            let at = tb(recs, |r| {
                ev(r, "uc_v")
                    && r["f"] == "path"
                    && r["fake"] == false
                    && r["v"].as_array().is_some_and(|a| a.len() > 2)
            });
            recs[at]["v"][1][0] = json!(recs[at]["v"][1][0].as_f64().unwrap() + 0.5);
        }),
        // The grid: the turn controller's graph updates and the navigation timer.
        ("a graph event differs from the sim's", |recs| {
            let at = tb(recs, |r| ev(r, "nav_guo"));
            recs[at]["e"][0] = json!(recs[at]["e"][0].as_f64().unwrap() + 0.25);
        }),
        ("a graph event differs from the sim's", |recs| {
            let at = tb(recs, |r| ev(r, "nav_guo"));
            recs[at]["c"][2] = json!(recs[at]["c"][2].as_f64().unwrap() + 0.25);
        }),
        ("a graph event differs from the sim's", |recs| {
            let at = tb(recs, |r| ev(r, "nav_guo"));
            recs[at]["f"] = json!("010000110");
        }),
        ("a graph event differs from the sim's", |recs| {
            let at = tb(recs, |r| ev(r, "nav_update"));
            recs[at]["erosion"] = json!(2);
        }),
        ("a graph event differs from the sim's", |recs| {
            let at = tb(recs, |r| ev(r, "nav_tags") && r["erode"] == 1);
            recs[at]["hash"] = json!("0000000000000000");
        }),
        ("driven replay: paths: ", |recs| {
            let at = tb(recs, |r| ev(r, "nav_guo"));
            recs.remove(at);
            renumber(recs);
        }),
        ("driven replay: paths: ", |recs| {
            let at = tb(recs, |r| ev(r, "nav_tags"));
            let r = recs[at].clone();
            recs.insert(at + 1, r);
            renumber(recs);
        }),
        ("the recording has no read", |recs| {
            recs.retain(|r| !ev(r, "nav_clock"));
            renumber(recs);
        }),
        ("a navigation clock read the sim's UpdateNavigationGrid doesn't make", |recs| {
            let b = pos(recs, |r| r["k"] == "begin");
            recs[b]["nav_timer0"] = json!(-1.0);
        }),
        ("driven replay: paths: ", |recs| {
            // A clock read that leaves the timer positive: no refresh, the recorded one left.
            let at = pos(recs, |r| ev(r, "nav_clock"));
            recs[at]["v"] = json!(-1.0);
        }),
        ("driven replay: paths: observer paths' records without its declaration", |recs| {
            let b = pos(recs, |r| r["k"] == "begin");
            let obs: Vec<V> = recs[b]["observers"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|x| *x != "paths")
                .cloned()
                .collect();
            recs[b]["observers"] = json!(obs);
        }),
        ("corpulence changes in the window", |recs| {
            let at = tb(recs, |r| ev(r, "uc_v") && r["f"] == "corp");
            recs[at]["v"] = json!("0.75");
        }),
        ("the seeker's decision", |recs| {
            let at = of_path(recs, fin_of_done(recs), "path_seek");
            recs[at]["accepted"] = json!(false);
        }),
        ("the seeker's decision", |recs| {
            // A FindPath's (the path visualizer's): no modifiers, none recorded.
            let fin = tb(recs, |r| {
                ev(r, "path_fin") && r["path"].as_str().is_some_and(|p| p.starts_with("5671#"))
            });
            let seek = of_path(recs, fin, "path_seek");
            recs[seek]["mods"] = json!(false);
            let m = of_path(recs, fin, "path_mod");
            recs.remove(m);
            renumber(recs);
        }),
        ("a fake path where FixPathIfTooShort keeps", |recs| {
            let at = tb(recs, |r| {
                ev(r, "uc_v")
                    && r["f"] == "path"
                    && r["fake"] == false
                    && r["v"].as_array().is_some_and(|a| a.len() == 2)
            });
            recs[at]["fake"] = json!(true);
        }),
        ("the end node", |recs| {
            let at = fin_of_done(recs);
            recs[at]["end"] = json!(-999);
        }),
        ("a path_seek of a path no request started", |recs| {
            let at = of_path(recs, fin_of_done(recs), "path_seek");
            let mut r = recs[at].clone();
            r["path"] = json!("nonexistent#0");
            recs.insert(at + 1, r);
            renumber(recs);
        }),
        ("a record of another frame", |recs| {
            let at = fin_of_done(recs);
            recs[at]["t"] = json!(1);
        }),
        ("driven replay: agent: a ray query from", |recs| {
            let at = later(recs, |r| ev(r, "ag_phys") && r["ctx"] == "tick");
            recs[at]["col"]["sib"] = json!([999]);
        }),
        ("observer paths without the driven agent", |recs| {
            let b = pos(recs, |r| r["k"] == "begin");
            let obs: Vec<V> = recs[b]["observers"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|x| *x != "agent_sync")
                .cloned()
                .collect();
            recs[b]["observers"] = json!(obs);
        }),
        ("graph events left at a frame's end", |recs| {
            // A removal's refresh moved into the next frame.
            let s = sync(recs);
            let at = (s..recs.len())
                .find(|j| {
                    ev(&recs[*j], "nav_tags")
                        && ev(&recs[*j - 1], "nav_update")
                        && recs[*j]["erode"].as_i64().is_some_and(|e| e > 0)
                })
                .unwrap();
            let t = recs[at]["t"].as_i64().unwrap();
            let mut r = recs.remove(at);
            r["t"] = json!(t + 1);
            let next = at + recs[at..].iter().position(|x| x["k"] == "tick").unwrap();
            recs.insert(next + 1, r);
            renumber(recs);
        }),
        ("a definition of a XPath path", |recs| {
            let fin = fin_of_done(recs);
            let key = recs[fin]["path"].clone();
            let at = tb(recs, |r| ev(r, "ag_pathdef") && r["path"] == key && r["pending"] != true);
            recs[at]["kind"] = json!("XPath");
        }),
        ("a graph update without its id", |recs| {
            let at = tb(recs, |r| ev(r, "nav_guo"));
            let id = recs[at]["id"].clone();
            for r in recs.iter_mut() {
                if (ev(r, "nav_guo") || ev(r, "nav_update")) && r["id"] == id {
                    r["id"] = json!(null);
                }
            }
        }),
        ("a path no request started", |recs| {
            let at = fin_of_done(recs);
            recs[at]["path"] = json!("5198#99");
        }),
        ("the query settings (ph_settings) aren't wr-phys's", |recs| {
            let at = pos(recs, |r| ev(r, "ph_settings"));
            recs[at]["hit_backfaces"] = json!(true);
        }),
        ("a return (path_fin) without its delivery (path_ret)", |recs| {
            // A path_fin of a request moved before its request: the request is a second.
            let fin = fin_of_done(recs);
            let key = recs[fin]["path"].clone();
            let req = (0..fin)
                .rev()
                .find(|j| ev(&recs[*j], "ag_req_end") && recs[*j]["rq"]["path"] == key)
                .unwrap();
            let r = recs[fin].clone();
            recs.remove(fin);
            recs.insert(req, r);
            renumber(recs);
        }),
    ];
    damage_cases(&dir, &cases);
    // A replay's navigation timer runs on its recording's clock reads: its refresh is the sim's.
    let replay: Vec<(&str, Change)> = vec![("driven replay: paths: ", |recs| {
        let at = (1..recs.len())
            .find(|j| ev(&recs[*j], "nav_tags") && ev(&recs[*j - 1], "turn_tick"))
            .unwrap();
        recs.remove(at);
        renumber(recs);
    })];
    damage_cases_of(&dir, "replay-1", &replay);
}
