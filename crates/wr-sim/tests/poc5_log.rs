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
/// recorded messages and tooltips (initiative, attack and damage: every one compared);
/// `poc5-tb-117`'s fight is the damaged one.
const CAPTURES: [(&str, &str, usize, usize); 4] = [
    ("poc5-tb-115", "tb-nearest", 25, 17),
    ("poc5-tb-115", "tb-focus", 27, 19),
    ("poc5-tb-116", "tb-hold", 27, 19),
    ("poc5-tb-117", "tb-solo", 27, 18),
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
    for (cap, fight, messages, tips) in CAPTURES {
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
            assert_eq!((c.tips_equal, c.tips_not_ported), (tips, 0), "{cap}/{fight}/{run}");
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

/// The first message of a thread with its tooltip's description changed (the change must
/// change it).
fn edit_tip(recs: &mut [V], th: &str, f: impl Fn(&str) -> String) {
    let at = first_msg(recs, th);
    let d = recs[at]["tip"]["d"].as_str().expect("a tooltip").to_owned();
    let e = f(&d);
    assert_ne!(d, e, "the damage changes the tooltip");
    recs[at]["tip"]["d"] = json!(e);
}

/// The first line after the first line containing `after` that has the colour markup removed.
fn drop_line(d: &str, after: &str) -> String {
    let lines: Vec<&str> = d.split('\n').collect();
    let start = lines.iter().position(|l| l.contains(after)).expect("the header");
    let at = (start + 1..lines.len()).find(|&i| lines[i].contains("<color")).expect("a line");
    lines
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != at)
        .map(|(_, l)| *l)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The two first nonempty lines after the first line containing `after`, swapped.
fn swap_after(d: &str, after: &str) -> String {
    let mut lines: Vec<&str> = d.split('\n').collect();
    let start = lines.iter().position(|l| l.contains(after)).expect("the header");
    let next: Vec<usize> =
        (start + 1..lines.len()).filter(|&i| !lines[i].trim().is_empty()).take(2).collect();
    assert_eq!(next.len(), 2, "two lines to swap");
    lines.swap(next[0], next[1]);
    lines.join("\n")
}

fn renumber(recs: &mut [V]) {
    for (n, r) in recs.iter_mut().enumerate() {
        r["seq"] = json!(n);
    }
}

const L: &str = "combat log:";
const C: &str = "driven replay: commands:";
/// A record out of its frame's controller order (`Frame::to`).
const P: &str = "can't be placed";

type Change = fn(&mut Vec<V>);

/// The damaged recordings, each with the report it must make.
const CASES: [(&str, Change); 62] = [
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
    // Attack and damage tooltips (the sim's): a wrong bonus, a missing line, a reordered
    // breakdown, another source's name, another header; a damage's dice modification, its roll
    // result, its bonus's breakdown reordered.
    (L, |recs| edit_tip(recs, "AttackLogThread", |d| d.replacen(">+", ">+1", 1))),
    (L, |recs| edit_tip(recs, "AttackLogThread", |d| drop_line(d, "Armor Class"))),
    (L, |recs| edit_tip(recs, "AttackLogThread", |d| swap_after(d, "<b>Attack Bonus"))),
    (L, |recs| edit_tip(recs, "AttackLogThread", |d| d.replacen("Dexterity", "Strength", 1))),
    (L, |recs| {
        let at = first_msg(recs, "AttackLogThread");
        recs[at]["tip"]["h"] = json!("x");
    }),
    (L, |recs| edit_tip(recs, "DamageDealtLogThread", |d| d.replacen("-> 1d6", "-> 1d8", 1))),
    (L, |recs| {
        edit_tip(recs, "DamageDealtLogThread", |d| d.replacen("Roll result: ", "Roll result: 1", 1))
    }),
    (L, |recs| edit_tip(recs, "DamageDealtLogThread", |d| swap_after(d, "Damage result"))),
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
    // An initiative's event (outside the checked spans) queued before its rule's `did`.
    (L, |recs| {
        let at = first_q(recs, "GameLogRuleEvent<RuleInitiativeRoll>");
        let did = (0..at).rev().find(|&i| recs[i]["k"] == "did").unwrap_or(at);
        let r = recs.remove(at);
        recs.insert(did, r);
        renumber(recs);
    }),
    // A message's context or scope.
    (L, |recs| {
        let at = first_msg(recs, "UnitInitiativeLogThread");
        recs[at]["ctx"] = json!({});
    }),
    (L, |recs| {
        let at = first_msg(recs, "UnitInitiativeLogThread");
        recs[at]["scope"] = json!(999);
    }),
    // A thread's ordinal out of the observer's allocation; a malformed sticky reference.
    (L, |recs| recs[0]["game_log"]["threads"][0]["by"][0][1] = json!(-1)),
    (L, |recs| recs[0]["game_log"]["ctx"]["SourceFact"] = json!({})),
    // A projectile's rule reference with another field.
    (L, |recs| {
        let at = first_q(recs, "GameLogEventProjectileHit");
        recs[at]["f"]["Projectile"]["extra"] = json!(1);
    }),
    // A party's experience event before the last experience the party gained, and raised by
    // another caller.
    (L, |recs| {
        let at = first_q(recs, "GameLogEventPartyGainExperience");
        let xp = (0..at).rev().find(|&i| recs[i]["k"] == "mut" && recs[i]["what"] == "xp");
        let r = recs.remove(at);
        recs.insert(xp.unwrap_or(at), r);
        renumber(recs);
    }),
    (L, |recs| {
        let at = first_q(recs, "GameLogEventPartyGainExperience");
        let o = recs[at]["origin"].as_str().unwrap_or("").replace("GainPartyExperience", "Other");
        recs[at]["origin"] = json!(o);
    }),
    // An attack of opportunity's event after the count's decrement; the party's combat state's
    // before the turn controller's handler of the same raise.
    (L, |recs| {
        let at = first_q(recs, "GameLogEventAttackOfOpportunity");
        let r = recs.remove(at);
        recs.insert(at + 1, r);
        renumber(recs);
    }),
    (L, |recs| {
        let at = first_q(recs, "GameLogEventCombatStateChanged");
        let tb = (0..at).rev().find(|&i| recs[i]["k"] == "tb" && recs[i]["ev"] == "combat_start");
        let pair = [recs.remove(at), recs.remove(at)];
        let to = tb.unwrap_or(at);
        for (n, r) in pair.into_iter().enumerate() {
            recs.insert(to + n, r);
        }
        renumber(recs);
    }),
    // A log tick with its messages before the leave controller's record of the frame (the log's
    // controller is the frame's last); the party's combat state after the cooldowns controller's
    // prefix (the join controller raises it).
    (P, |recs| {
        let at = (0..recs.len())
            .find(|&i| {
                recs[i]["ev"] == "clog_tick"
                    && recs[i.saturating_sub(4)..i].iter().any(|x| x["ev"] == "leave_tick")
            })
            .expect("a tick after a leave_tick");
        let leave = (0..at).rev().find(|&i| recs[i]["ev"] == "leave_tick").unwrap_or(at);
        let mut moved = vec![recs.remove(at)];
        while recs[at]["ev"] == "clog_msg" {
            moved.push(recs.remove(at));
        }
        for (n, r) in moved.into_iter().enumerate() {
            recs.insert(leave + n, r);
        }
        renumber(recs);
    }),
    (P, |recs| {
        let at = first_q(recs, "GameLogEventCombatStateChanged");
        let pair = [recs.remove(at), recs.remove(at)];
        let cd = at + recs[at..].iter().position(|r| r["ev"] == "cd_prefix").unwrap_or(0);
        for (n, r) in pair.into_iter().enumerate() {
            recs.insert(cd + 1 + n, r);
        }
        renumber(recs);
    }),
    // The party's combat end before the turn controller's handler of it has finished.
    (L, |recs| {
        let at = recs
            .iter()
            .position(|r| {
                r["ev"] == "clog_q"
                    && r["e"] == "GameLogEventCombatStateChanged"
                    && r["f"]["InCombat"] == false
            })
            .expect("a combat end");
        let end = (0..at).rev().find(|&i| recs[i]["k"] == "tb" && recs[i]["ev"] == "combat_end");
        let pair = [recs.remove(at), recs.remove(at)];
        let to = end.map_or(at, |e| e + 1);
        for (n, r) in pair.into_iter().enumerate() {
            recs.insert(to + n, r);
        }
        renumber(recs);
    }),
];

#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-117 capture"]
fn missing_or_wrong_combat_log_records_are_reported() {
    let (cap, fight, _, tips) = CAPTURES[3];
    let Some(dir) = fixture_in(cap, fight) else {
        eprintln!("{cap}/{fight} not present: skipped");
        return;
    };
    let (out, w) = replay::run_fixture(SAVE, &dir, "record").expect("replay");
    assert!(w.valid(), "{:?}", w.coverage.unsupported.keys().collect::<Vec<_>>());
    assert!(out.clog.is_some_and(|c| c.differences == 0 && c.tips_equal == tips));
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

/// The combat end's events moved before `CombatController.Clear`'s navigation cleanup, on a fight
/// whose cleanup starts with graph updates (`nav_guo`, `nav_update`) rather than its tags.
#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-115 capture"]
fn combat_end_before_the_navigation_cleanup_is_reported() {
    let (cap, fight, _, _) = CAPTURES[0];
    let Some(dir) = fixture_in(cap, fight) else {
        eprintln!("{cap}/{fight} not present: skipped");
        return;
    };
    let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
    let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
    let recs = &mut changed.records;
    let at = recs
        .iter()
        .position(|r| {
            r["ev"] == "clog_q"
                && r["e"] == "GameLogEventCombatStateChanged"
                && r["f"]["InCombat"] == false
        })
        .expect("a combat end");
    let end = (0..at)
        .rev()
        .find(|&i| recs[i]["k"] == "tb" && recs[i]["ev"] == "combat_end")
        .expect("its combat_end");
    assert!(
        recs[end + 1..at].iter().any(|r| r["ev"] == "nav_guo"),
        "the fight's cleanup has graph updates before the log events"
    );
    let pair = [recs.remove(at), recs.remove(at)];
    for (n, r) in pair.into_iter().enumerate() {
        recs.insert(end + 1 + n, r);
    }
    renumber(recs);
    let _ = replay::run_with_tape(&mut w, &changed, &trace);
    assert!(
        !w.valid() && w.coverage.unsupported.keys().any(|k| k.contains(L)),
        "{:?}",
        w.coverage.unsupported.keys().collect::<Vec<_>>()
    );
}
