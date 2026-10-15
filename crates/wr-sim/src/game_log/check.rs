//! The combat log against a recording (WrathOracle's observer `combat_log`, `LogObs.cs`): the
//! begin record's `game_log` (the dispatch tables, the queue and `GameLogContext` when the
//! window opens), each `clog_q` (an event queued), `clog_tick` (a tick's queue) and `clog_msg`
//! (a message added). The sim's own [`super::Out`] stream is compared with them in order:
//! every queued event (frame, type, its rule's class and units, `IsEnabled`, override), every
//! tick's queue, every message (frame, thread, event, text, colour, icon and, where the sim
//! makes it, tooltip). A difference, or a record the sim can't read, is reported.
//!
//! Events the sim doesn't raise itself are the recording's inputs, queued where their records
//! are: a unit's bark (`GameLogEventBark`: the UI's) and a projectile's hit
//! (`GameLogEventProjectileHit`: projectiles aren't modelled).

use std::collections::HashMap;

use serde_json::Value as J;

use super::{EventType, Out, Payload, THREADS, Tip, root_rules, threads_of, unsupported};
use crate::world::World;

/// What the comparison found.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    /// Events queued, ticks with a queue, messages: the recording's and the sim's.
    pub queued: [usize; 2],
    pub ticks: [usize; 2],
    pub messages: [usize; 2],
    /// Messages whose text, colour, icon, thread and event are equal.
    pub messages_equal: usize,
    /// Tooltips compared and equal; tooltips the sim doesn't make (not compared), by class.
    pub tips_equal: usize,
    pub tips_not_ported: usize,
    /// Events taken from the recording (barks, projectile hits), by type.
    pub inputs: std::collections::BTreeMap<String, usize>,
    /// Differences found (each also reported).
    pub differences: usize,
}

/// The rule records' classes and units, by the recording's rule id.
fn rule_table(recs: &[J]) -> HashMap<i64, (String, Option<String>, Option<String>)> {
    let unit = |v: &J| v.get("$unit").and_then(J::as_str).map(str::to_owned);
    recs.iter()
        .filter(|r| r["k"] == "rule")
        .filter_map(|r| {
            let class = r["class"].as_str()?.rsplit(['.', '+']).next()?.to_owned();
            Some((r["id"].as_i64()?, (class, unit(&r["initiator"]), unit(&r["target"]))))
        })
        .collect()
}

/// The event type a record names.
fn parse_type(s: &str) -> Option<EventType> {
    if let Some(r) = s.strip_prefix("GameLogRuleEvent<").and_then(|x| x.strip_suffix('>')) {
        return root_rule_name(r).map(EventType::Rule).or(Some(EventType::Rule(leak(r))));
    }
    Some(EventType::Other(leak(s)))
}

fn root_rule_name(r: &str) -> Option<&'static str> {
    root_rules().into_iter().find(|x| *x == r)
}

/// Type names are few and live for the run.
fn leak(s: &str) -> &'static str {
    static NAMES: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
    let mut n = NAMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(x) = n.iter().find(|x| **x == s) {
        return x;
    }
    let x: &'static str = Box::leak(s.to_owned().into_boxed_str());
    n.push(x);
    x
}

/// The begin record's `game_log`: checks the dispatch tables against [`THREADS`], takes the
/// context's fields that outlive scopes, and turns the model on. A recording without it is
/// reported (a run that declares `combat_log` must have it).
pub fn activate(w: &mut World, begin: &J) {
    let g = &begin["game_log"];
    if !g.is_object() {
        unsupported(w, "the begin record declares combat_log without its game_log");
        return;
    }
    w.game_log.on = true;
    match super::strings::LogStrings::load(&super::strings::LogStrings::default_path()) {
        Ok(s) => w.game_log.strings = Some(std::rc::Rc::new(s)),
        Err(e) => unsupported(w, format!("the log strings: {e:#}")),
    }
    if w.text.is_none() {
        match crate::text::TextData::load_default() {
            Ok(t) => w.text = Some(std::rc::Rc::new(t)),
            Err(e) => unsupported(w, format!("the localization pack: {e:#}")),
        }
    }
    if g["controller"] != true {
        unsupported(w, "no GameLogController when the window opened");
        return;
    }
    // m_RootRulesTypes.
    let roots: Option<Vec<&str>> = g["roots"].as_array().map(|a| a.iter().filter_map(J::as_str).collect());
    let mut ours = root_rules();
    ours.sort_unstable();
    if roots.as_deref() != Some(ours.as_slice()) {
        unsupported(w, format!("the root rule types differ from the game's ({roots:?})"));
    }
    // m_EventToThreads: each event type's threads, by name, in order.
    let Some(table) = g["threads"].as_array() else {
        unsupported(w, "game_log.threads missing");
        return;
    };
    let mut keys = 0;
    for t in table {
        let (Some(ev), Some(by)) = (t["event"].as_str(), t["by"].as_array()) else {
            unsupported(w, "a malformed game_log.threads entry");
            continue;
        };
        let names: Vec<&str> = by.iter().filter_map(|x| x[0].as_str()).collect();
        let ours: Vec<&str> = parse_type(ev).map(|ty| threads_of(ty).into_iter().map(|i| THREADS[i].0).collect()).unwrap_or_default();
        if names != ours || names.len() != by.len() {
            unsupported(w, format!("{ev}'s threads differ from the game's ({names:?})"));
        }
        keys += 1;
    }
    let our_keys: std::collections::BTreeSet<&str> =
        THREADS.iter().flat_map(|(_, h)| h.iter().copied()).collect();
    if keys != our_keys.len() {
        unsupported(w, format!("the game dispatches {keys} event types, the sim {}", our_keys.len()));
    }
    if g["ready"].as_array().is_none_or(|a| !a.is_empty()) || g["stack"] != 0 {
        unsupported(w, "events queued or open when the window opened");
    }
    if g["scope"] != 0 {
        unsupported(w, "a GameLogContext scope open when the window opened");
    }
    // The fields `Clear` leaves.
    let c = &g["ctx"];
    let int = |k: &str| c[k].as_i64().and_then(|v| i32::try_from(v).ok());
    match (&c["SourceFact"], int("Rations"), int("RoundNumber"), int("RollChance")) {
        (f, Some(rations), Some(round), Some(chance)) => {
            if !f.is_null() {
                w.game_log.ctx.source_fact = super::SourceFact::Unknown(format!("from before the window: {f}"));
            }
            w.game_log.ctx.rations = rations;
            w.game_log.ctx.round_number = round;
            w.game_log.ctx.roll_chance = chance;
            match &c["Formula"] {
                J::Null => {}
                J::String(s) => w.game_log.ctx.formula = Some(s.clone()),
                _ => unsupported(w, "game_log.ctx.Formula malformed"),
            }
        }
        _ => unsupported(w, "game_log.ctx malformed"),
    }
    // The fields `Clear` resets are at their defaults outside a scope.
    for (k, v) in c.as_object().into_iter().flatten() {
        let sticky = matches!(k.as_str(), "SourceFact" | "Formula" | "Rations" | "RoundNumber" | "RollChance");
        if !sticky && !(v.is_null() || *v == 0) {
            unsupported(w, format!("GameLogContext.{k} set outside a scope when the window opened"));
        }
    }
}

/// The recording's inputs: a `clog_q` of an event the sim doesn't raise, queued at its record.
/// `rec_q` counts the recording's `clog_q`s seen so far, `rule_q` maps a recorded rule id to its
/// event's place in the queue order.
pub fn input(w: &mut World, r: &J, inputs: &mut std::collections::BTreeMap<String, usize>, rule_q: &HashMap<i64, usize>) {
    let ty = r["e"].as_str().unwrap_or("");
    match ty {
        "GameLogEventBark" => {
            let f = &r["f"];
            let unit = f["Entity"]["$unit"].as_str().and_then(|id| w.by_id.get(id).copied());
            if f["Entity"].get("$unit").is_some() && unit.is_none() {
                unsupported(w, "a bark's unit is unknown");
                return;
            }
            let Some(text) = f["Text"].as_str() else {
                unsupported(w, "a bark without its text");
                return;
            };
            super::add_event(w, "GameLogEventBark", Payload::Bark { unit, text: text.to_owned() }, [unit, None]);
        }
        "GameLogEventProjectileHit" => {
            let rule = &r["f"]["Projectile"];
            let attack = if rule.is_null() {
                None
            } else {
                let Some(q) = rule["$rule"].as_i64().and_then(|id| rule_q.get(&id)) else {
                    unsupported(w, "a projectile hit whose attack roll wasn't queued");
                    return;
                };
                let ev = w.game_log.out.iter().filter_map(|o| match o {
                    Out::Queued { event, .. } => Some(*event),
                    _ => None,
                }).nth(*q);
                let Some(ev) = ev else {
                    unsupported(w, "a projectile hit whose attack roll the sim didn't queue");
                    return;
                };
                Some(ev)
            };
            super::add_event(w, "GameLogEventProjectileHit", Payload::ProjectileHit { attack }, [None, None]);
        }
        _ => return,
    }
    *inputs.entry(ty.to_owned()).or_default() += 1;
}

/// Whether the sim raises events of this type itself (else they are inputs or unported).
#[must_use]
pub fn raised_by_sim(ty: &str) -> bool {
    ty.starts_with("GameLogRuleEvent<")
        || matches!(
            ty,
            "GameLogEventAttackOfOpportunity"
                | "GameLogEventUnitLifeStateChanged"
                | "GameLogEventCombatStateChanged"
                | "GameLogEventPartyCombatState"
                | "GameLogEventPartyGainExperience"
                | "GameLogEventKnowledge"
        )
}

/// Whether the recording's inputs include this type.
#[must_use]
pub fn is_input(ty: &str) -> bool {
    matches!(ty, "GameLogEventBark" | "GameLogEventProjectileHit")
}

fn diff(w: &mut World, s: &mut Summary, what: String) {
    s.differences += 1;
    if s.differences <= 20 {
        unsupported(w, what);
    } else if s.differences == 21 {
        unsupported(w, "more differences (not listed)");
    }
}

/// Compares the sim's stream with the recording's records.
pub fn compare(w: &mut World, recs: &[J], inputs: std::collections::BTreeMap<String, usize>) -> Summary {
    let mut s = Summary { inputs, ..Summary::default() };
    let obs = |ev: &str| -> Vec<&J> { recs.iter().filter(|r| r["k"] == "obs" && r["ev"] == ev).collect() };
    let (rq, rt, rm) = (obs("clog_q"), obs("clog_tick"), obs("clog_msg"));
    let out = w.game_log.out.clone();
    let sq: Vec<(i64, usize)> = out.iter().filter_map(|o| match o { Out::Queued { frame, event } => Some((*frame, *event)), _ => None }).collect();
    let st: Vec<&Out> = out.iter().filter(|o| matches!(o, Out::Tick { .. })).collect();
    let sm: Vec<&super::Msg> = out.iter().filter_map(|o| match o { Out::Msg(m) => Some(m), _ => None }).collect();
    s.queued = [rq.len(), sq.len()];
    s.ticks = [rt.len(), st.len()];
    s.messages = [rm.len(), sm.len()];
    let rules = rule_table(recs);
    // Queue positions: the recording's event ids and the sim's event indices.
    let mut rec_pos: HashMap<i64, usize> = HashMap::new();
    let mut sim_pos: HashMap<usize, usize> = HashMap::new();
    for (k, r) in rq.iter().enumerate() {
        if let Some(id) = r["id"].as_i64() {
            rec_pos.insert(id, k);
        }
    }
    for (k, (_, e)) in sq.iter().enumerate() {
        sim_pos.insert(*e, k);
    }
    if rq.len() != sq.len() {
        diff(w, &mut s, format!("{} events queued, the sim queued {}", rq.len(), sq.len()));
    }
    let uid = |w: &World, u: Option<crate::world::EntityIdx>| u.map(|u| w.entity(u).unique_id.clone());
    for (k, (r, (frame, e))) in rq.iter().zip(sq.iter()).enumerate() {
        let ev = w.game_log.events[*e].clone();
        let mut bad = Vec::new();
        if r["t"].as_i64() != Some(*frame) {
            bad.push(format!("frame {} vs {frame}", r["t"]));
        }
        if r["e"].as_str() != Some(ev.ty.name().as_str()) {
            bad.push(format!("type {} vs {}", r["e"], ev.ty.name()));
        }
        if let EventType::Rule(class) = ev.ty {
            match r["rule"].as_i64().and_then(|id| rules.get(&id)) {
                Some((c, a, b)) => {
                    let ours = (uid(w, ev.units[0]), uid(w, ev.units[1]));
                    if c != class || *a != ours.0 || (b.is_some() && *b != ours.1) {
                        bad.push(format!("rule {c} {a:?}->{b:?} vs {class} {:?}->{:?}", ours.0, ours.1));
                    }
                }
                None => bad.push("its rule isn't recorded".into()),
            }
        }
        if r["en"].as_bool() != Some(ev.enabled) {
            bad.push(format!("enabled {} vs {}", r["en"], ev.enabled));
        }
        if r["ov"].as_str() != ev.override_message.as_deref() {
            bad.push(format!("override {} vs {:?}", r["ov"], ev.override_message));
        }
        if !bad.is_empty() {
            diff(w, &mut s, format!("queued event #{k}: {}", bad.join("; ")));
        }
    }
    // Ticks.
    if rt.len() != st.len() {
        diff(w, &mut s, format!("{} ticks with a queue, the sim's {}", rt.len(), st.len()));
    }
    for (k, (r, o)) in rt.iter().zip(st.iter()).enumerate() {
        let Out::Tick { frame, queue, stack } = o else { continue };
        let rec: Option<Vec<(usize, bool)>> = r["q"].as_array().map(|a| {
            a.iter()
                .map(|x| (x[0].as_i64().and_then(|id| rec_pos.get(&id).copied()).unwrap_or(usize::MAX), x[1] == true))
                .collect()
        });
        let ours: Vec<(usize, bool)> = queue.iter().map(|(e, en)| (sim_pos.get(e).copied().unwrap_or(usize::MAX - 1), *en)).collect();
        if r["t"].as_i64() != Some(*frame) || rec.as_ref() != Some(&ours) || r["stack"].as_u64() != u64::try_from(*stack).ok() {
            diff(w, &mut s, format!("tick #{k} at {}: queue {:?} stack {} vs {frame}: {ours:?} stack {stack}", r["t"], rec, r["stack"]));
        }
    }
    // Messages.
    if rm.len() != sm.len() {
        diff(w, &mut s, format!("{} messages, the sim made {}", rm.len(), sm.len()));
    }
    let mut ordinal: HashMap<i64, usize> = HashMap::new();
    for (k, (r, m)) in rm.iter().zip(sm.iter()).enumerate() {
        let mut bad = Vec::new();
        if r["t"].as_i64() != Some(m.frame) {
            bad.push(format!("frame {} vs {}", r["t"], m.frame));
        }
        if r["th"].as_str() != Some(THREADS[m.thread].0) {
            bad.push(format!("thread {} vs {}", r["th"], THREADS[m.thread].0));
        }
        if let Some(ti) = r["ti"].as_i64()
            && *ordinal.entry(ti).or_insert(m.thread) != m.thread
        {
            bad.push(format!("thread instance {ti}"));
        }
        let src_rec = r["src"].as_i64().and_then(|id| rec_pos.get(&id).copied());
        let src_sim = m.src.and_then(|e| sim_pos.get(&e).copied());
        if src_rec != src_sim {
            bad.push(format!("event {src_rec:?} vs {src_sim:?}"));
        }
        if r["text"].as_str() != Some(m.text.as_str()) {
            bad.push("text".into());
        }
        if r["color"].as_str() != Some(m.color.as_str()) {
            bad.push(format!("colour {} vs {}", r["color"], m.color));
        }
        if r["icon"].as_i64() != Some(i64::from(m.icon)) {
            bad.push(format!("icon {} vs {}", r["icon"], m.icon));
        }
        let text_ok = bad.is_empty();
        match (&m.tip, &r["tip"]) {
            (Tip::None, J::Null) => {}
            (Tip::None, _) | (_, J::Null) => bad.push("tooltip presence".into()),
            (Tip::NotPorted(cls), t) => {
                if t["cls"].as_str() != Some(cls) {
                    bad.push(format!("tooltip class {} vs {cls}", t["cls"]));
                } else {
                    s.tips_not_ported += 1;
                }
            }
            (Tip::Simple { cls, header, desc }, t) => {
                if t["cls"].as_str() != Some(cls) || t["h"].as_str() != header.as_deref() || t["d"].as_str() != desc.as_deref() {
                    bad.push("tooltip".into());
                } else {
                    s.tips_equal += 1;
                }
            }
        }
        if text_ok {
            s.messages_equal += 1;
        }
        if !bad.is_empty() {
            if std::env::var_os("WR_DEBUG_CLOG").is_some() {
                eprintln!("DEBUG clog #{k}: rec {r}\n  sim {m:?}");
            }
            diff(w, &mut s, format!("message #{k} ({}): {}", THREADS[m.thread].0, bad.join("; ")));
        }
    }
    s
}
