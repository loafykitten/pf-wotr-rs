//! The records inside a root rule's span that nothing in the rule's own code reads.
//!
//! A root rule (`Rulebook.Trigger` outside any rule) is replayed as a unit: the driver fires the
//! rule (or the sim's command, cast or process already did) and the main loop moves past its
//! `rule_end`, so every record in between is skipped unless something takes it. The rule's own
//! steps, handlers, draws, mutations, results and environment are compared through the T0
//! streams; the observation records WrathOracle writes inside a span are the families below.
//! Each is either checked by its family's checker or reported, and none is skipped silently
//! (docs/poc5/tb-replay.md, "Records inside root rule spans"):
//!
//! - `clog_q`: a rule event queued by `GameLogController`'s did hook. Compared with the sim's
//!   queue at the run's end ([`crate::game_log::check::compare`]: its place in the queue, frame,
//!   type, rule, units, enabled flag, override, and its position right after the rule's `did`).
//!   The sim raises it when it fires the rule, so a span the sim doesn't fire leaves the queue short.
//! - `imp_s` and `mem_q`: the impatience controller's and the memory controller's handlers of a
//!   rule's did event, matched with the sim's operations by [`Driver::imp_span`] (after the sim
//!   fired the rule), which reports them without the impatience observer.
//! - `eng_prov`: `UnitCombatEngagementController`'s provoke in its did handler of an attack roll
//!   or an attack with a weapon. Placed like a command's (`span_ok`): in the root's frame, inside
//!   that handler of the rule its caller names, once per rule, of the rule's initiator; then
//!   matched with the sim's provoke calls at the next engagement tick like the commands'
//!   ([`Driver::eng_prov`]), which reports a provoke the sim didn't make.
//! - `proj_launch`: the projectiles a rule attack launched, checked against the sim's by
//!   [`Driver::proj_launches`] where the sim fires an attack: each launch placed as `span_ok`
//!   places a command's (the root's frame and rule, at its `OnTrigger` outside any handler, right
//!   after its attack roll's end, its fields, its target), then equal to the sim's.
//! - `env`: only the environment the code that fires the root looked up (the attack roll's, the
//!   resolve's, the flat-footed check's: [`Driver::env_reads`]); any other is reported.
//! - `invalid`: the oracle's own statement that the game's replay left its recording. The sim is
//!   checked against the game's actual trace, so it is not the sim's matter: accepted everywhere
//!   (counted in [`SpanCounts::oracle_invalid`] inside spans).
//!
//! The rest of a span is the rule's own: its steps (`about`, `trig`, `trig_end`, `did`), handlers
//! (`h`, `h_end`) and nested rules, which the code that fires it reads, and its draws and
//! mutations, compared in the T0 streams. The oracle never writes raw `result` or `bonuses`
//! records (the projection derives them from `rule` and `rule_end`), so a stray one is reported,
//! like any other observation or record of another kind: the replay doesn't take what it can't
//! name. A span the commands pull (`attack_rule`) is checked record by record by `span_ok` instead.

use std::collections::{HashMap, HashSet};

use super::commands::provoke_rule;
use super::{Driver, Rv};
use crate::world::World;

/// `UnitCombatEngagementController`, whose did handler of an attack roll or an attack with a
/// weapon makes the provoke.
const ENGAGEMENT: &str = "Kingmaker.Controllers.Combat.UnitCombatEngagementController";

/// The observation records found inside root rules' spans (the commands' own spans apart), each
/// checked or taken as described in the module's documentation.
#[derive(Debug, Default, Clone)]
pub struct SpanCounts {
    /// Rule events queued (`clog_q`), compared with the sim's queue at the run's end.
    pub clog_q: usize,
    /// Impatience (`imp_s`) and memory (`mem_q`) handlers' records, matched with the sim's.
    pub imp_s: usize,
    pub mem_q: usize,
    /// Provokes (`eng_prov`) placed and queued to be matched with the sim's calls.
    pub eng_prov: usize,
    /// Projectile launches (`proj_launch`) placed and equal to the sim's.
    pub proj_launch: usize,
    /// The oracle's own `invalid` records (the game's replay left its recording): accepted.
    pub oracle_invalid: usize,
    /// Records reported: of another family or kind, or one its checker rejects.
    pub reported: usize,
}

/// Whether the T0 projection keeps a mutation (`project::project_records`): damage, experience,
/// facts attached and detached, and a bonus added to a rule it tracks.
fn projected(r: &Rv) -> bool {
    match r["what"].as_str() {
        Some("damage" | "xp" | "fact_attach" | "fact_detach") => true,
        Some("rule_bonus") => r["target_rule"].as_i64().is_some_and(|t| t > 0),
        _ => false,
    }
}

/// What `span_records` knows of the span so far: the rules and handlers open and where each
/// rule is in its life (`span_ok`'s frames), fed the span's records in order.
pub(super) struct Open {
    /// The root's frame.
    pub(super) t: i64,
    /// Its rules by id: short class and initiator.
    pub(super) rules: HashMap<i64, (String, Rv)>,
    /// The open rules, outermost first, with their stage: 0 pushed, 1 `about`, 2 `trig`, 3
    /// `trig_end`, 4 `did`.
    pub(super) stack: Vec<(i64, u8)>,
    /// The open handlers: class, rule, phase.
    pub(super) handlers: Vec<(String, i64, String)>,
    /// The rules that made their provoke.
    pub(super) provoked: HashSet<i64>,
}

impl Open {
    /// The state at the root's record.
    pub(super) fn new(root: &Rv) -> Self {
        let mut open = Self {
            t: root["t"].as_i64().unwrap_or(-1),
            rules: HashMap::new(),
            stack: Vec::new(),
            handlers: Vec::new(),
            provoked: HashSet::new(),
        };
        let _ = open.feed(root);
        open
    }

    /// The next record of the span: a rule pushed or ended, one of its steps, a handler opened or
    /// closed. False for one out of its place, as `span_ok` places them: a nested rule pushed
    /// in its parent's `OnTrigger` or one of its handlers, a step of the innermost rule (not
    /// while its own handler is open) in the order `about`, `trig`, `trig_end`, `did`, `rule_end`,
    /// a handler of the innermost rule in the phase its stage allows.
    pub(super) fn feed(&mut self, r: &Rv) -> bool {
        let id = r["id"].as_i64();
        let own =
            self.stack.last().is_some_and(|t| self.handlers.last().is_some_and(|h| h.1 == t.0));
        let mut step = |want: u8, next: u8| match self.stack.last_mut() {
            Some(top) if Some(top.0) == id && top.1 == want && !own => {
                top.1 = next;
                true
            }
            _ => false,
        };
        match r["k"].as_str() {
            Some("rule") => {
                let ok = self
                    .stack
                    .last()
                    .is_none_or(|t| Some(t.0) == r["parent"].as_i64() && (t.1 == 2 || own));
                let class =
                    r["class"].as_str().unwrap_or("").rsplit(['.', '+']).next().unwrap_or("");
                if let Some(id) = id {
                    self.rules.insert(id, (class.to_owned(), r["initiator"].clone()));
                    self.stack.push((id, 0));
                }
                ok
            }
            Some("about") => step(0, 1),
            Some("trig") => step(1, 2),
            Some("trig_end") => step(2, 3),
            Some("did") => step(3, 4),
            Some("rule_end") => {
                let ok = step(4, 5);
                if self.stack.last().is_some_and(|t| Some(t.0) == id) {
                    self.stack.pop();
                }
                ok
            }
            Some("h") => {
                let phase = r["phase"].as_str().unwrap_or("");
                let ok = self.stack.last().is_some_and(|t| {
                    Some(t.0) == r["rule"].as_i64()
                        && !own
                        && matches!(
                            (phase, t.1),
                            ("mod_before", 0) | ("about", 1) | ("mod_after" | "did", 3)
                        )
                });
                self.handlers.push((
                    r["class"].as_str().unwrap_or("").to_owned(),
                    r["rule"].as_i64().unwrap_or(0),
                    phase.to_owned(),
                ));
                ok
            }
            Some("h_end") => self.handlers.pop().is_some(),
            _ => true,
        }
    }
}

/// A provoke (`r`, whose caller names `rule`) placed like `span_ok` places a command's: in the
/// root's frame, in the engagement controller's did handler of that rule, of the rule's
/// initiator, once per rule, while the rule is the innermost open one and between `trig_end` and
/// `did`.
pub(super) fn provoke_placed(open: &mut Open, r: &Rv, rule: Option<&str>) -> bool {
    let Some((class, id, phase)) = open.handlers.last() else { return false };
    let placed = r["t"].as_i64() == Some(open.t)
        && class == ENGAGEMENT
        && phase == "did"
        && open.stack.last() == Some(&(*id, 3))
        && open.rules.get(id).is_some_and(|(rc, initiator)| {
            Some(rc.as_str()) == rule && r["u"] == initiator["$unit"]
        });
    let id = *id;
    placed && open.provoked.insert(id)
}

impl Driver<'_> {
    /// The records of the root rule at `start`, up to its `rule_end` at `end`, after the sim
    /// fired it (or didn't).
    pub(super) fn span_records(&mut self, w: &mut World, start: usize, end: usize) {
        let trace = self.trace;
        let mut open = Open::new(&trace.records[start]);
        for k in start + 1..=end {
            let r = &trace.records[k];
            // A root rule fires within one frame: every record of its span is of it.
            if r["t"].as_i64() != Some(open.t) {
                self.span_reported(
                    w,
                    format!("a {} record", r["k"].as_str().unwrap_or("?")),
                    "a record of another frame than its root's",
                );
            }
            match r["k"].as_str() {
                // The rule's own: its steps and nested rules, and its handlers (read by the code
                // that fires it), and the draws and mutations (compared in the T0 streams).
                Some(
                    "rule" | "h" | "h_end" | "rule_end" | "about" | "trig" | "trig_end" | "did",
                ) => {
                    if !open.feed(r) {
                        self.span_reported(
                            w,
                            format!("a {} record", r["k"].as_str().unwrap_or("?")),
                            "a record out of its place in the rule's life",
                        );
                    }
                }
                // A draw is compared in the T0 stream (a presentation draw is none).
                Some("draw") => {}
                // So is a mutation of the kinds the projection keeps; the others it drops.
                Some("mut") if projected(r) => {}
                // The environment the root's code looked up.
                Some("env") => {
                    let read = self.env_reads.iter().any(|(rule, a, t)| {
                        super::find_env(trace, start, end, rule, a, t) == Some(k)
                    });
                    if !read {
                        self.span_reported(
                            w,
                            "an env record".into(),
                            "an environment the root's code didn't read",
                        );
                    }
                }
                Some("mut") => self.span_reported(
                    w,
                    format!("a {} mutation", r["what"].as_str().unwrap_or("?")),
                    "a mutation the T0 projection drops, so nothing compares it",
                ),
                Some("obs") => self.span_obs(w, start, r, &mut open),
                // (The oracle writes no raw `result` or `bonuses`: the projection derives them.)
                other => self.span_reported(
                    w,
                    format!("a {} record", other.unwrap_or("?")),
                    "a record of a kind the replay doesn't take",
                ),
            }
        }
    }

    fn span_obs(&mut self, w: &mut World, root: usize, r: &Rv, open: &mut Open) {
        match r["ev"].as_str() {
            Some("clog_q") if self.clog => self.out.spans.clog_q += 1,
            Some("clog_q") => self.span_reported(
                w,
                "clog_q".into(),
                "a combat log record in a recording that doesn't declare combat_log",
            ),
            // (Without the impatience observer `imp_span` reported them.)
            Some("imp_s") => self.out.spans.imp_s += usize::from(self.imp),
            Some("mem_q") => self.out.spans.mem_q += usize::from(self.imp),
            Some("eng_prov") => {
                // In the engagement controller's did handler of the rule its caller names, of the
                // rule's initiator, the root's frame, once per rule (`span_ok`'s placing).
                let rule = provoke_rule(r);
                let placed = provoke_placed(open, r, rule);
                if placed && self.eng_prov(w, r, rule) {
                    self.out.spans.eng_prov += 1;
                } else {
                    self.span_reported(
                        w,
                        "eng_prov".into(),
                        "a provoke the engagement controller's handlers can't have made",
                    );
                }
            }
            // Counted where `proj_launches` checked it (the attack the sim fired).
            Some("proj_launch") if self.launches_checked == Some(root) => {}
            Some("proj_launch") => self.span_reported(
                w,
                "proj_launch".into(),
                "a projectile launched by a rule the sim didn't fire as an attack",
            ),
            Some("invalid") => self.out.spans.oracle_invalid += 1,
            other => self.span_reported(
                w,
                format!("an {} observation", other.unwrap_or("?")),
                "an observation no checker of the replay takes",
            ),
        }
    }

    fn span_reported(&mut self, w: &mut World, what: String, why: &str) {
        self.out.spans.reported += 1;
        w.coverage.unsupported(format!("driven replay: {what} inside a root rule's span: {why}"));
    }
}
