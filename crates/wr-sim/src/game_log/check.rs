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
    let mut keys: Vec<&str> =
        g.as_object().map(|o| o.keys().map(String::as_str).collect()).unwrap_or_default();
    keys.sort_unstable();
    if keys != ["controller", "ctx", "ready", "roots", "scope", "stack", "threads"] {
        unsupported(w, format!("game_log's fields ({keys:?})"));
    }
    if g["controller"] != true {
        unsupported(w, "no GameLogController when the window opened");
        return;
    }
    // m_RootRulesTypes.
    let roots: Option<Vec<&str>> =
        g["roots"].as_array().and_then(|a| a.iter().map(J::as_str).collect::<Option<Vec<&str>>>());
    let mut ours = root_rules();
    ours.sort_unstable();
    if roots.as_deref() != Some(ours.as_slice()) {
        unsupported(w, format!("the root rule types differ from the game's ({roots:?})"));
    }
    // m_EventToThreads: each event type once, every one the sim dispatches, its threads by name in
    // order, each ordinal one thread instance.
    let Some(table) = g["threads"].as_array() else {
        unsupported(w, "game_log.threads missing");
        return;
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut by_ordinal: HashMap<i64, usize> = HashMap::new();
    let mut ordinal_of: HashMap<usize, i64> = HashMap::new();
    for t in table {
        let (Some(ev), Some(by)) = (t["event"].as_str(), t["by"].as_array()) else {
            unsupported(w, "a malformed game_log.threads entry");
            continue;
        };
        if !keys_are(t, &["event", "by"]) {
            unsupported(w, "a malformed game_log.threads entry");
            continue;
        };
        let Some(ty) = parse_type(ev) else {
            unsupported(w, format!("game_log.threads: an event type {ev}"));
            continue;
        };
        if !seen.insert(ty.key()) {
            unsupported(w, format!("game_log.threads: {ev} twice"));
        }
        let ours = threads_of(ty);
        let rec: Option<Vec<(&str, i64)>> = by
            .iter()
            .map(|x| match x.as_array().map(Vec::as_slice) {
                Some([n, o]) => Some((n.as_str()?, o.as_i64()?)),
                _ => None,
            })
            .collect();
        let Some(rec) = rec.filter(|r| r.len() == ours.len()) else {
            unsupported(w, format!("{ev}'s threads differ from the game's ({by:?})"));
            continue;
        };
        for ((name, ord), th) in rec.iter().zip(&ours) {
            // `LogObs.ThreadId`: ordinals from 0, in the order the table first names a thread.
            let fresh = !by_ordinal.contains_key(ord);
            if fresh && *ord != i64::try_from(by_ordinal.len()).unwrap_or(-1) {
                unsupported(w, format!("{ev}'s thread ordinal {ord} out of order"));
            }
            let same_name = *name == THREADS[*th].0;
            let a = *by_ordinal.entry(*ord).or_insert(*th) == *th;
            let b = *ordinal_of.entry(*th).or_insert(*ord) == *ord;
            if !(same_name && a && b) {
                unsupported(w, format!("{ev}'s threads differ from the game's ({by:?})"));
                break;
            }
        }
    }
    let our_keys: std::collections::BTreeSet<String> =
        THREADS.iter().flat_map(|(_, h)| h.iter().map(|k| (*k).to_owned())).collect();
    if seen != our_keys {
        unsupported(
            w,
            format!("the game dispatches {} event types, the sim {}", seen.len(), our_keys.len()),
        );
    }
    w.game_log.thread_ordinals = by_ordinal;
    if g["ready"].as_array().is_none_or(|a| !a.is_empty()) || g["stack"] != 0 {
        unsupported(w, "events queued or open when the window opened");
    }
    if g["scope"] != 0 {
        unsupported(w, "a GameLogContext scope open when the window opened");
    }
    context(w, &g["ctx"]);
}

/// A reference as `LogObs.Val` writes it, of the kind the field holds: a unit by id, a fact by id
/// and blueprint, a localized string by key, a roll by its rule's id and class, a tooltip's object.
fn reference_ok(field: &str, v: &J) -> bool {
    match field {
        "SourceUnit" | "Target" => keys_are(v, &["$unit"]) && v["$unit"].is_string(),
        "SourceFact" => {
            keys_are(v, &["$fact", "bp"])
                && v["$fact"].is_string()
                && (v["bp"].is_null() || v["bp"].is_string())
        }
        "Message" => keys_are(v, &["$key"]) && (v["$key"].is_null() || v["$key"].is_string()),
        "D20" | "D100" => {
            keys_are(v, &["$rule", "class"]) && v["$rule"].is_i64() && v["class"].is_string()
        }
        _ => {
            (keys_are(v, &["$type"]) && v["$type"].is_string())
                || (keys_are(v, &["$entity", "class"])
                    && v["$entity"].is_string()
                    && v["class"].is_string())
        }
    }
}

/// `GameLogContext`'s fields when the window opens: every one, of its type; the ones `Clear`
/// leaves taken, the others at their defaults (outside a scope).
fn context(w: &mut World, c: &J) {
    // (name, kind: 'i' an int, 's' a string, else an object or null)
    const FIELDS: [(&str, char); 22] = [
        ("SourceUnit", 'o'),
        ("SourceFact", 'o'),
        ("Target", 'o'),
        ("Message", 'o'),
        ("Formula", 's'),
        ("Text", 's'),
        ("TextWithTags", 's'),
        ("Description", 's'),
        ("Count", 'i'),
        ("Roll", 'i'),
        ("D20", 'o'),
        ("D100", 'o'),
        ("Modifier", 'i'),
        ("DC", 'i'),
        ("ChanceDC", 'i'),
        ("ConcealmentStatus", 'i'),
        ("Rations", 'i'),
        ("AttackNumber", 'i'),
        ("AttacksCount", 'i'),
        ("RoundNumber", 'i'),
        ("Tooltip", 'o'),
        ("RollChance", 'i'),
    ];
    let Some(o) = c.as_object() else {
        unsupported(w, "game_log.ctx malformed");
        return;
    };
    if o.len() != FIELDS.len() {
        unsupported(w, format!("game_log.ctx has {} fields", o.len()));
    }
    for (k, kind) in FIELDS {
        let Some(v) = o.get(k) else {
            unsupported(w, format!("game_log.ctx.{k} missing"));
            continue;
        };
        let typed = match kind {
            'i' => v.as_i64().is_some_and(|x| i32::try_from(x).is_ok()),
            's' => v.is_null() || v.is_string(),
            _ => v.is_null() || reference_ok(k, v),
        };
        if !typed {
            unsupported(w, format!("game_log.ctx.{k} malformed"));
            continue;
        }
        let int = || v.as_i64().and_then(|x| i32::try_from(x).ok()).unwrap_or(0);
        match k {
            "SourceFact" if !v.is_null() => {
                w.game_log.ctx.source_fact = super::SourceFact::Unknown(v.clone());
            }
            "SourceFact" => {}
            "Formula" => w.game_log.ctx.formula = v.as_str().map(str::to_owned),
            "Rations" => w.game_log.ctx.rations = int(),
            "RoundNumber" => w.game_log.ctx.round_number = int(),
            "RollChance" => w.game_log.ctx.roll_chance = int(),
            // The fields `Clear` resets are at their defaults outside a scope.
            _ if !(v.is_null() || *v == 0) => unsupported(
                w,
                format!("GameLogContext.{k} set outside a scope when the window opened"),
            ),
            _ => {}
        }
    }
}

/// The recording's inputs: a `clog_q` of an event the sim doesn't raise, queued at its record.
/// `rec_q` counts the recording's `clog_q`s seen so far, `rule_q` maps a recorded rule id to its
/// event's place in the queue order.
pub fn input(
    w: &mut World,
    r: &J,
    inputs: &mut std::collections::BTreeMap<String, usize>,
    rule_q: &HashMap<i64, usize>,
) {
    let ty = r["e"].as_str().unwrap_or("");
    match ty {
        "GameLogEventBark" => {
            let f = &r["f"];
            let entity = &f["Entity"];
            let entity_ok = entity.is_null()
                || (keys_are(entity, &["$unit"]) && entity["$unit"].is_string())
                || (keys_are(entity, &["$entity", "class"]) && entity["$entity"].is_string());
            if !keys_are(f, &["Entity", "Text"]) || !entity_ok {
                unsupported(w, "a bark's fields malformed");
                return;
            }
            let unit = f["Entity"]["$unit"].as_str().and_then(|id| w.by_id.get(id).copied());
            if f["Entity"].get("$unit").is_some() && unit.is_none() {
                unsupported(w, "a bark's unit is unknown");
                return;
            }
            let Some(text) = f["Text"].as_str() else {
                unsupported(w, "a bark without its text");
                return;
            };
            super::add_event(
                w,
                "GameLogEventBark",
                Payload::Bark { unit, text: text.to_owned() },
                [unit, None],
            );
        }
        "GameLogEventProjectileHit" => {
            let rule = &r["f"]["Projectile"];
            if !keys_are(&r["f"], &["Projectile"])
                || !(rule.is_null()
                    || (keys_are(rule, &["$rule", "class"])
                        && rule["$rule"].is_i64()
                        && rule["class"] == "RuleAttackRoll"))
            {
                unsupported(w, "a projectile hit's fields malformed");
                return;
            }
            let attack = if rule.is_null() {
                None
            } else {
                let Some(q) = rule["$rule"].as_i64().and_then(|id| rule_q.get(&id)) else {
                    unsupported(w, "a projectile hit whose attack roll wasn't queued");
                    return;
                };
                let ev = w
                    .game_log
                    .out
                    .iter()
                    .filter_map(|o| match o {
                        Out::Queued { event, .. } => Some(*event),
                        _ => None,
                    })
                    .nth(*q);
                let Some(ev) = ev else {
                    unsupported(w, "a projectile hit whose attack roll the sim didn't queue");
                    return;
                };
                Some(ev)
            };
            super::add_event(
                w,
                "GameLogEventProjectileHit",
                Payload::ProjectileHit { attack },
                [None, None],
            );
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

/// A non-rule event's `f` as the observer writes the sim's event (`None` for a rule event).
fn event_fields(w: &World, p: &Payload) -> Option<J> {
    let unit = |u: crate::world::EntityIdx| serde_json::json!({"$unit": w.entity(u).unique_id});
    Some(match p {
        Payload::AttackOfOpportunity { attacker, target } => {
            serde_json::json!({"Attacker": unit(*attacker), "Target": unit(*target)})
        }
        Payload::LifeState { unit: u, prev } => {
            serde_json::json!({"Unit": unit(*u), "PrevLifeState": prev})
        }
        Payload::CombatState { in_combat } => serde_json::json!({"InCombat": in_combat}),
        Payload::PartyExperience { experience } => serde_json::json!({"Experience": experience}),
        // `InspectUnitsManager`'s `UnitInfo` (an object the observer names by type).
        Payload::Knowledge => serde_json::json!({"UnitInfo": {"$type": "UnitInfo"}}),
        _ => return None,
    })
}

/// Where the game raises a non-rule event, against the records around it (`recs[i]`, a `clog_q`),
/// for the kinds whose raise point the trace shows:
/// - an attack of opportunity's (`UnitCombatState.AttackOfOpportunity`) right after the command's
///   `Run` (its `cmd` run of `UnitAttackOfOpportunity`, the same units) and before the count's
///   decrement (`cd_write` from its setter) or, without one, the observer's `eng_aoo` of it;
/// - a party's experience (`Player.GainPartyExperience`) right after its last member's experience
///   (`mut` xp);
/// - `GameLogEventPartyCombatState` right after `GameLogEventCombatStateChanged` of the same
///   state (the next subscriber of the same raise), which comes after the turn controller's
///   handler of that raise (its `tb` `combat_start`/`combat_end`, the last of the frame).
fn raise_point_ok(recs: &[J], i: usize) -> bool {
    let r = &recs[i];
    let prev = i.checked_sub(1).map(|j| &recs[j]);
    let not_log =
        |x: &&J| !(x["k"] == "obs" && x["ev"].as_str().is_some_and(|e| e.starts_with("clog_")));
    let prev_other = recs[..i].iter().rev().find(not_log);
    let next_other = recs[i + 1..].iter().find(not_log);
    let f = &r["f"];
    match r["e"].as_str() {
        Some("GameLogEventAttackOfOpportunity") => {
            let (a, t) = (&f["Attacker"]["$unit"], &f["Target"]["$unit"]);
            prev.is_some_and(|p| {
                p["k"] == "cmd"
                    && p["ev"] == "run"
                    && p["class"] == "UnitAttackOfOpportunity"
                    && p["unit"]["$unit"] == *a
                    && p["target"]["$unit"] == *t
            }) && next_other.is_some_and(|n| {
                (n["ev"] == "cd_write"
                    && n["u"] == *a
                    && n["writer"] == "UnitCombatState.set_AttackOfOpportunityCount")
                    || (n["ev"] == "eng_aoo" && n["u"] == *a && n["tgt"] == *t)
            })
        }
        Some("GameLogEventPartyGainExperience") => {
            prev_other.is_some_and(|p| p["k"] == "mut" && p["what"] == "xp")
        }
        Some("GameLogEventPartyCombatState") => prev.is_some_and(|p| {
            p["ev"] == "clog_q" && p["e"] == "GameLogEventCombatStateChanged" && p["f"] == *f
        }),
        Some("GameLogEventCombatStateChanged") => {
            let want = if f["InCombat"] == true { "combat_start" } else { "combat_end" };
            // After the turn controller's handler's work (its record is written at the handler's
            // start: `Clear`'s turn clear and navigation tags, the commands it cancels, the agents it\n            // stops follow).
            let handler_work = next_other.is_some_and(|n| {
                (n["k"] == "tb" && n["ev"] == "turn_clear")
                    || n["k"] == "cmd"
                    || n["ev"].as_str().is_some_and(|e| {
                        e.starts_with("uc_")
                            || e.starts_with("ag_call")
                            || e == "ag_sync"
                            || e == "nav_tags"
                    })
            });
            if handler_work {
                return false;
            }
            recs[..i]
                .iter()
                .rev()
                .take_while(|x| x["k"] != "tick")
                .find(|x| {
                    x["k"] == "tb"
                        && matches!(x["ev"].as_str(), Some("combat_start" | "combat_end"))
                })
                .is_some_and(|x| x["ev"] == want)
        }
        _ => true,
    }
}

/// A record's fields, exactly.
fn keys_are(r: &J, want: &[&str]) -> bool {
    r.as_object().is_some_and(|o| o.len() == want.len() && want.iter().all(|k| o.contains_key(*k)))
}

fn int_or_null(v: &J) -> bool {
    v.is_null() || v.is_i64()
}

/// A `clog_q` as `LogObs.QueuedPostfix` writes it.
fn q_shape(r: &J) -> bool {
    const BASE: [&str; 9] = ["seq", "k", "t", "ev", "id", "e", "rule", "en", "ov"];
    let fields = if r["rule"].is_null() {
        let mut v = BASE.to_vec();
        v.extend(["f", "origin"]);
        keys_are(r, &v) && r["f"].is_object() && r["origin"].is_string()
    } else {
        keys_are(r, &BASE) && r["rule"].as_i64().is_some_and(|x| x > 0)
    };
    fields
        && r["t"].is_i64()
        && r["e"].is_string()
        && r["en"].is_boolean()
        && (r["ov"].is_null() || r["ov"].is_string())
}

/// A `clog_tick` as `LogObs.TickPrefix` writes it (before the tick's own scope).
fn tick_shape(r: &J) -> bool {
    keys_are(r, &["seq", "k", "t", "ev", "q", "stack", "scope"])
        && r["t"].is_i64()
        && r["stack"].as_u64().is_some()
        && r["scope"] == 0
        && r["q"].as_array().is_some_and(|q| {
            q.iter().all(|x| {
                x.as_array()
                    .is_some_and(|p| p.len() == 2 && int_or_null(&p[0]) && p[1].is_boolean())
            })
        })
}

/// A `clog_msg` as `LogObs.AddPostfix` writes it: in a thread's handler, inside the tick's scope.
fn msg_shape(r: &J) -> bool {
    let keys = [
        "seq",
        "k",
        "t",
        "ev",
        "n",
        "th",
        "ti",
        "src",
        "in_handler",
        "text",
        "color",
        "icon",
        "tip",
        "scope",
        "ctx",
    ];
    let tip_ok = match &r["tip"] {
        J::Null => true,
        t => {
            keys_are(t, &["cls", "h", "d"])
                && t["cls"].is_string()
                && (t["h"].is_null() || t["h"].is_string())
                && (t["d"].is_null() || t["d"].is_string())
        }
    };
    keys_are(r, &keys)
        && r["t"].is_i64()
        && r["n"].is_i64()
        && r["th"].is_string()
        && r["ti"].is_i64()
        && r["src"].as_i64().is_some_and(|x| x > 0)
        && r["in_handler"] == true
        && r["text"].is_string()
        && r["color"]
            .as_str()
            .is_some_and(|c| c.len() == 8 && c.bytes().all(|b| b.is_ascii_hexdigit()))
        && r["icon"].is_i64()
        && tip_ok
        && r["scope"].as_i64().is_some_and(|x| x >= 1)
        && r["ctx"].is_object()
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
pub fn compare(
    w: &mut World,
    recs: &[J],
    inputs: std::collections::BTreeMap<String, usize>,
) -> Summary {
    let mut s = Summary { inputs, ..Summary::default() };
    // Every log record well formed, the events numbered from 1 and the messages from 0 in order;
    // the three kinds in the sim's order.
    let mut rec_kinds = Vec::new();
    let (mut next_id, mut next_n) = (1_i64, 0_i64);
    for r in recs
        .iter()
        .filter(|r| r["k"] == "obs" && r["ev"].as_str().is_some_and(|e| e.starts_with("clog_")))
    {
        let ok = match r["ev"].as_str() {
            Some("clog_q") => {
                rec_kinds.push('q');
                next_id += 1;
                q_shape(r) && r["id"].as_i64() == Some(next_id - 1)
            }
            Some("clog_tick") => {
                rec_kinds.push('t');
                tick_shape(r)
            }
            Some("clog_msg") => {
                rec_kinds.push('m');
                next_n += 1;
                msg_shape(r) && r["n"].as_i64() == Some(next_n - 1)
            }
            _ => false,
        };
        if !ok {
            diff(w, &mut s, format!("a malformed or unknown log record (seq {})", r["seq"]));
        }
    }
    for (i, r) in recs.iter().enumerate() {
        if r["k"] != "obs" || r["ev"] != "clog_q" {
            continue;
        }
        let Some(rule) = r["rule"].as_i64() else { continue };
        let did = i.checked_sub(1).map(|j| &recs[j]);
        let end = recs[i + 1..].iter().find(|x| x["k"] != "mut");
        let ok = did.is_some_and(|d| d["k"] == "did" && d["id"] == rule)
            && end.is_some_and(|x| x["k"] == "rule_end" && x["id"] == rule)
            && recs[i + 1..]
                .iter()
                .take_while(|x| x["k"] == "mut")
                .all(|x| x["rule"].as_i64() == Some(rule));
        if !ok {
            diff(
                w,
                &mut s,
                format!("a rule's event queued outside its did hook (seq {})", r["seq"]),
            );
        }
    }
    // A non-rule event: raised where the game raises it (its handler and raiser in its caller
    // chain) and, for one the sim raises, after as many experience and damage mutations as the
    // sim had written.
    let mut rec_muts = Vec::new();
    let mut n_muts = 0_usize;
    for (i, r) in recs.iter().enumerate() {
        if r["k"] == "mut" && r.as_object().is_some_and(super::counted_mut) {
            n_muts += 1;
        }
        if r["k"] == "obs" && r["ev"] == "clog_q" && r["rule"].is_null() {
            let origin = r["origin"].as_str().unwrap_or("");
            let frames: Vec<&str> = origin.split('<').collect();
            let raiser = match r["e"].as_str() {
                Some("GameLogEventPartyGainExperience") => {
                    Some(("HandlePartyGainExperience", "Player.GainPartyExperience"))
                }
                Some("GameLogEventUnitLifeStateChanged") => {
                    Some(("HandleUnitLifeStateChanged", "UnitLifeController.RaiseLifestateChanged"))
                }
                Some("GameLogEventKnowledge") => {
                    Some(("HandleKnowledgeUpdated", "InspectUnitsManager.TryMakeKnowledgeCheck"))
                }
                Some("GameLogEventCombatStateChanged" | "GameLogEventPartyCombatState") => {
                    Some(("HandlePartyCombatStateChanged", "Player.UpdateIsInCombat"))
                }
                Some("GameLogEventAttackOfOpportunity") => {
                    Some(("HandleAttackOfOpportunity", "UnitCombatState.AttackOfOpportunity"))
                }
                Some("GameLogEventProjectileHit") => {
                    Some(("HandleProjectileHit", "Projectile.OnHit"))
                }
                Some("GameLogEventBark") => Some(("HandleOnShowBark", "")),
                _ => None,
            };
            let placed = raiser.is_some_and(|(h, by)| {
                frames.first() == Some(&"GameLogController.AddReadyEvent")
                    && frames.get(1) == Some(&"GameEventsHandler.AddEvent")
                    && frames.get(2) == Some(&format!("EventsHandler.{h}").as_str())
                    && (by.is_empty() || frames.iter().any(|f| f.contains(by)))
            });
            if !placed {
                diff(w, &mut s, format!("a {} event raised elsewhere ({origin})", r["e"]));
            }
            if !raise_point_ok(recs, i) {
                diff(w, &mut s, format!("a {} event out of its raise (seq {})", r["e"], r["seq"]));
            }
            rec_muts.push(Some(n_muts));
        } else if r["k"] == "obs" && r["ev"] == "clog_q" {
            rec_muts.push(None);
        }
    }
    let sim_muts: Vec<Option<usize>> = w
        .game_log
        .out
        .iter()
        .filter_map(|o| match o {
            Out::Queued { event, muts, .. } => Some(
                (w.game_log.events[*event].rule_id.is_none()
                    && super::check::raised_by_sim(&w.game_log.events[*event].ty.name()))
                .then_some(*muts),
            ),
            _ => None,
        })
        .collect();
    for (k, (a, b)) in rec_muts.iter().zip(&sim_muts).enumerate() {
        if let (Some(a), Some(b)) = (a, b)
            && a != b
        {
            diff(w, &mut s, format!("queued event #{k} after {a} mutations, the sim's after {b}"));
        }
    }
    let sim_kinds: Vec<char> = w
        .game_log
        .out
        .iter()
        .map(|o| match o {
            Out::Queued { .. } => 'q',
            Out::Tick { .. } => 't',
            Out::Msg(_) => 'm',
        })
        .collect();
    if rec_kinds != sim_kinds {
        let at = rec_kinds.iter().zip(&sim_kinds).position(|(a, b)| a != b);
        diff(w, &mut s, format!("the log's records in another order (from record {at:?})"));
    }
    let obs = |ev: &str| -> Vec<&J> {
        recs.iter().filter(|r| r["k"] == "obs" && r["ev"] == ev).collect()
    };
    let (rq, rt, rm) = (obs("clog_q"), obs("clog_tick"), obs("clog_msg"));
    let out = w.game_log.out.clone();
    let sq: Vec<(i64, usize)> = out
        .iter()
        .filter_map(|o| match o {
            Out::Queued { frame, event, .. } => Some((*frame, *event)),
            _ => None,
        })
        .collect();
    let st: Vec<&Out> = out.iter().filter(|o| matches!(o, Out::Tick { .. })).collect();
    let sm: Vec<&super::Msg> = out
        .iter()
        .filter_map(|o| match o {
            Out::Msg(m) => Some(m),
            _ => None,
        })
        .collect();
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
    let uid =
        |w: &World, u: Option<crate::world::EntityIdx>| u.map(|u| w.entity(u).unique_id.clone());
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
            // Its rule: the same one (the window's rules numbered alike), of its class and units.
            if r["rule"].as_i64() != ev.rule_id {
                bad.push(format!("rule {} vs {:?}", r["rule"], ev.rule_id));
            }
            match r["rule"].as_i64().and_then(|id| rules.get(&id)) {
                Some((c, a, b)) => {
                    let ours = (uid(w, ev.units[0]), uid(w, ev.units[1]));
                    if c != class || *a != ours.0 || (b.is_some() && *b != ours.1) {
                        bad.push(format!(
                            "rule {c} {a:?}->{b:?} vs {class} {:?}->{:?}",
                            ours.0, ours.1
                        ));
                    }
                }
                None => bad.push("its rule isn't recorded".into()),
            }
        }
        // A non-rule event's fields, as the observer writes them.
        if let Some(f) = event_fields(w, &ev.payload)
            && r["f"] != f
        {
            bad.push(format!("fields {} vs {f}", r["f"]));
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
                .map(|x| {
                    (
                        x[0].as_i64()
                            .and_then(|id| rec_pos.get(&id).copied())
                            .unwrap_or(usize::MAX),
                        x[1] == true,
                    )
                })
                .collect()
        });
        let ours: Vec<(usize, bool)> = queue
            .iter()
            .map(|(e, en)| (sim_pos.get(e).copied().unwrap_or(usize::MAX - 1), *en))
            .collect();
        if r["t"].as_i64() != Some(*frame)
            || rec.as_ref() != Some(&ours)
            || r["stack"].as_u64() != u64::try_from(*stack).ok()
        {
            diff(
                w,
                &mut s,
                format!(
                    "tick #{k} at {}: queue {:?} stack {} vs {frame}: {ours:?} stack {stack}",
                    r["t"], rec, r["stack"]
                ),
            );
        }
    }
    // Messages.
    if rm.len() != sm.len() {
        diff(w, &mut s, format!("{} messages, the sim made {}", rm.len(), sm.len()));
    }
    for (k, (r, m)) in rm.iter().zip(sm.iter()).enumerate() {
        let mut bad = Vec::new();
        if r["t"].as_i64() != Some(m.frame) {
            bad.push(format!("frame {} vs {}", r["t"], m.frame));
        }
        if r["th"].as_str() != Some(THREADS[m.thread].0) {
            bad.push(format!("thread {} vs {}", r["th"], THREADS[m.thread].0));
        }
        // Its thread instance: the begin record's ordinal of this thread.
        if r["ti"].as_i64().and_then(|ti| w.game_log.thread_ordinals.get(&ti)) != Some(&m.thread) {
            bad.push(format!("thread instance {}", r["ti"]));
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
        if r["scope"].as_u64() != Some(u64::from(m.scope)) {
            bad.push(format!("scope {} vs {}", r["scope"], m.scope));
        }
        match &m.ctx {
            Some(c) if *c == r["ctx"] => {}
            Some(c) => bad.push(format!("context {} vs {c}", r["ctx"])),
            None => bad.push("context (a roll the sim knows by its result only)".into()),
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
                if t["cls"].as_str() != Some(cls)
                    || t["h"].as_str() != header.as_deref()
                    || t["d"].as_str() != desc.as_deref()
                {
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
