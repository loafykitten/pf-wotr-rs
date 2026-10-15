//! The combat log against WrathOracle's `combat_log` captures (local, gitignored): every queued
//! event, tick and message of the driven replay is the sim's, and a damaged log record, table or
//! context is reported. Ignored by default (needs the game's blueprint and localization packs,
//! `cache/sim/log-strings.json` from `wr-rules-data`, Manual_18 and the capture; skipped when the
//! capture is absent). `cargo nextest run -p wr-sim --test poc5_log --run-ignored only`.

use std::path::PathBuf;

use serde_json::Value as V;
use serde_json::json;
use wr_sim::combat::{oracle::Trace, replay};

const SAVE: &str = "Manual_18_WR_FirstBattle";

/// The captures with `combat_log` (each fight's record and two replays), with each fight's
/// recorded messages; `poc5-tb-117`'s fight is the damaged one.
const CAPTURES: [(&str, &str, usize); 4] = [
    ("poc5-tb-115", "tb-nearest", 25),
    ("poc5-tb-115", "tb-focus", 27),
    ("poc5-tb-116", "tb-hold", 27),
    ("poc5-tb-117", "tb-solo", 27),
];

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

/// Every run: every message equal, nothing in the log differing. (The runs' validity apart from
/// the log is the driven replay's: `poc5.rs`.)
#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-115..-117 captures"]
fn every_recorded_message_is_the_sims() {
    for (cap, fight, messages) in CAPTURES {
        let Some(dir) = fixture_in(cap, fight) else {
            eprintln!("{cap}/{fight} not present: skipped");
            continue;
        };
        for run in ["record", "replay-1", "replay-2"] {
            let (out, w) = replay::run_fixture(SAVE, &dir, run).expect("replay");
            let log: Vec<_> =
                w.coverage.unsupported.keys().filter(|k| k.contains("combat log")).collect();
            assert!(log.is_empty(), "{cap}/{fight}/{run}: {log:?}");
            assert!(out.diff.equal, "{cap}/{fight}/{run}: {:?}", out.diff.first_difference);
            let c = out.clog.expect("the combat log is modelled");
            assert_eq!(c.messages, [messages, messages], "{cap}/{fight}/{run}");
            assert_eq!((c.messages_equal, c.differences), (messages, 0), "{cap}/{fight}/{run}");
            assert_eq!(c.queued[0], c.queued[1], "{cap}/{fight}/{run}");
            assert_eq!(c.ticks[0], c.ticks[1], "{cap}/{fight}/{run}");
        }
    }
}

fn nth(recs: &[V], ev: &str, n: usize) -> usize {
    recs.iter()
        .enumerate()
        .filter(|(_, r)| r["k"] == "obs" && r["ev"] == ev)
        .nth(n)
        .map(|(i, _)| i)
        .expect("a record")
}

fn first_q(recs: &[V], e: &str) -> usize {
    recs.iter().position(|r| r["ev"] == "clog_q" && r["e"] == e).expect("a clog_q")
}

fn first_msg(recs: &[V], th: &str) -> usize {
    recs.iter().position(|r| r["ev"] == "clog_msg" && r["th"] == th).expect("a clog_msg")
}

fn renumber(recs: &mut [V]) {
    for (n, r) in recs.iter_mut().enumerate() {
        r["seq"] = json!(n);
    }
}

const L: &str = "combat log:";
const C: &str = "driven replay: commands:";

type Change = fn(&mut Vec<V>);

/// The damaged recordings, each with the report it must make.
const CASES: [(&str, Change); 41] = [
    // Messages.
    (L, |recs| {
        let at = nth(recs, "clog_msg", 3);
        recs[at]["text"] = json!(format!("{} ", recs[at]["text"].as_str().unwrap_or("")));
    }),
    (L, |recs| {
        let at = nth(recs, "clog_msg", 3);
        recs[at]["color"] = json!("000000FF");
    }),
    (L, |recs| {
        let at = nth(recs, "clog_msg", 3);
        recs[at]["icon"] = json!(2);
    }),
    (L, |recs| {
        let at = nth(recs, "clog_msg", 3);
        recs[at]["th"] = json!("DamageDealtLogThread");
    }),
    (L, |recs| {
        let at = nth(recs, "clog_msg", 3);
        recs[at]["src"] = json!(recs[at]["src"].as_i64().unwrap_or(0) + 1);
    }),
    (L, |recs| {
        let at = nth(recs, "clog_msg", 3);
        recs[at]["t"] = json!(recs[at]["t"].as_i64().unwrap_or(0) + 1);
    }),
    (L, |recs| {
        let at = nth(recs, "clog_msg", 3);
        recs.remove(at);
        renumber(recs);
    }),
    (L, |recs| {
        let at = nth(recs, "clog_msg", 3);
        let r = recs[at].clone();
        recs.insert(at, r);
        renumber(recs);
    }),
    // An initiative's tooltip (the sim's), and an attack's dropped.
    (L, |recs| {
        let at = first_msg(recs, "UnitInitiativeLogThread");
        recs[at]["tip"]["d"] = json!("x");
    }),
    (L, |recs| {
        let at = first_msg(recs, "AttackLogThread");
        recs[at]["tip"] = V::Null;
    }),
    // Ticks.
    (L, |recs| {
        let at = nth(recs, "clog_tick", 2);
        recs.remove(at);
        renumber(recs);
    }),
    (L, |recs| {
        let at = nth(recs, "clog_tick", 2);
        recs[at]["q"][0][1] = json!(false);
    }),
    (L, |recs| {
        let at = nth(recs, "clog_tick", 2);
        recs[at]["stack"] = json!(1);
    }),
    (L, |recs| {
        let at = nth(recs, "clog_tick", 2);
        if let Some(q) = recs[at]["q"].as_array_mut() {
            q.pop();
        }
    }),
    // Events outside rules: dropped, other fields, retyped, an override.
    (L, |recs| {
        let at = first_q(recs, "GameLogEventPartyGainExperience");
        recs.remove(at);
        renumber(recs);
    }),
    (L, |recs| {
        let at = first_q(recs, "GameLogEventPartyGainExperience");
        recs[at]["f"]["Experience"] = json!(5);
    }),
    (L, |recs| {
        let at = first_q(recs, "GameLogEventAttackOfOpportunity");
        recs[at]["f"]["Target"] = json!({"$unit": "5671"});
    }),
    (L, |recs| {
        let at = first_q(recs, "GameLogEventUnitLifeStateChanged");
        recs[at]["e"] = json!("GameLogEventUnitFakeDeathMessage");
    }),
    (L, |recs| {
        let at = first_q(recs, "GameLogEventCombatStateChanged");
        recs[at]["ov"] = json!("a-key");
    }),
    // A projectile's hit naming another rule.
    (L, |recs| {
        let at = first_q(recs, "GameLogEventProjectileHit");
        recs[at]["f"]["Projectile"]["$rule"] = json!(1);
    }),
    // A rule's own event inside its span: disabled, of another rule, before its `did`, twice.
    (C, |recs| {
        let at = first_q(recs, "GameLogRuleEvent<RuleAttackRoll>");
        recs[at]["en"] = json!(false);
    }),
    (C, |recs| {
        let at = first_q(recs, "GameLogRuleEvent<RuleAttackRoll>");
        recs[at]["rule"] = json!(recs[at]["rule"].as_i64().unwrap_or(0) + 1);
    }),
    (C, |recs| {
        let at = first_q(recs, "GameLogRuleEvent<RuleAttackRoll>");
        let did = (0..at).rev().find(|&i| recs[i]["k"] == "did").unwrap_or(at);
        let r = recs.remove(at);
        recs.insert(did, r);
        renumber(recs);
    }),
    (C, |recs| {
        let at = first_q(recs, "GameLogRuleEvent<RuleAttackRoll>");
        let r = recs[at].clone();
        recs.insert(at, r);
        renumber(recs);
    }),
    // The begin record's tables and context.
    (L, |recs| {
        if let Some(b) = recs[0].as_object_mut() {
            b.remove("game_log");
        }
    }),
    (L, |recs| {
        if let Some(r) = recs[0]["game_log"]["roots"].as_array_mut() {
            r.pop();
        }
    }),
    (L, |recs| recs[0]["game_log"]["threads"][0]["by"][0][0] = json!("MessageLogThread")),
    (L, |recs| recs[0]["game_log"]["ctx"]["SourceUnit"] = json!({"$unit": "5198"})),
    (L, |recs| recs[0]["game_log"]["ready"] = json!([{"id": 1, "e": "GameLogEventMessage"}])),
    // The declaration dropped while the records stay.
    (L, |recs| {
        if let Some(o) = recs[0]["observers"].as_array_mut() {
            o.retain(|x| x != "combat_log");
        }
    }),
    // Another rule of the same class and units (an initiative's d20, outside the checked spans).
    (L, |recs| {
        let at = first_q(recs, "GameLogRuleEvent<RuleRollD20>");
        recs[at]["rule"] = json!(recs[at]["rule"].as_i64().unwrap_or(0) + 1);
    }),
    // A message before its tick.
    (L, |recs| {
        let at = nth(recs, "clog_msg", 0);
        let tick = (0..at).rev().find(|&i| recs[i]["ev"] == "clog_tick").unwrap_or(at);
        let r = recs.remove(at);
        recs.insert(tick, r);
        renumber(recs);
    }),
    // Another thread instance, ordinal, scope, handler; an override of another type; a gate flag;
    // an unknown log record.
    (L, |recs| {
        let at = first_msg(recs, "UnitInitiativeLogThread");
        let other =
            recs.iter().find(|r| r["ev"] == "clog_msg" && r["th"] != "UnitInitiativeLogThread");
        recs[at]["ti"] = other.map_or(json!(-1), |r| r["ti"].clone());
    }),
    (L, |recs| {
        let at = nth(recs, "clog_msg", 2);
        recs[at]["n"] = json!(7);
    }),
    (L, |recs| {
        let at = nth(recs, "clog_msg", 2);
        recs[at]["scope"] = json!(0);
    }),
    (L, |recs| {
        let at = nth(recs, "clog_msg", 2);
        recs[at]["in_handler"] = json!(false);
    }),
    (L, |recs| {
        let at = first_q(recs, "GameLogEventCombatStateChanged");
        recs[at]["ov"] = json!(5);
    }),
    (L, |recs| {
        let at = nth(recs, "clog_tick", 1);
        recs[at]["g"] = json!(true);
    }),
    (L, |recs| {
        let at = nth(recs, "clog_tick", 1);
        let mut r = recs[at].clone();
        r["ev"] = json!("clog_x");
        recs.insert(at + 1, r);
        renumber(recs);
    }),
    // A dispatch row replaced by a copy of another; a context field missing.
    (L, |recs| {
        let row = recs[0]["game_log"]["threads"][1].clone();
        recs[0]["game_log"]["threads"][0] = row;
    }),
    (L, |recs| {
        if let Some(c) = recs[0]["game_log"]["ctx"].as_object_mut() {
            c.remove("Target");
        }
    }),
];

#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-117 capture"]
fn missing_or_wrong_combat_log_records_are_reported() {
    let (cap, fight, _) = CAPTURES[3];
    let Some(dir) = fixture_in(cap, fight) else {
        eprintln!("{cap}/{fight} not present: skipped");
        return;
    };
    let (out, w) = replay::run_fixture(SAVE, &dir, "record").expect("replay");
    assert!(w.valid(), "{:?}", w.coverage.unsupported.keys().collect::<Vec<_>>());
    assert!(out.clog.is_some_and(|c| c.differences == 0 && c.tips_equal == 7));
    let mut missed = Vec::new();
    for (i, (want, change)) in CASES.into_iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
        change(&mut changed.records);
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
