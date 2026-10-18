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
//!   or an attack with a weapon, matched with the sim's provoke calls at the next engagement tick
//!   like the commands' ([`Driver::eng_prov`]); a provoke the sim didn't make is reported there.
//! - `proj_launch`: the projectiles a rule attack launched, checked against the sim's by
//!   [`Driver::proj_launches`] where the sim fires an attack.
//!
//! Any other record of a span (an observation of another family, a record of another kind) is
//! reported: the replay doesn't take what it can't name. A span the commands pull
//! (`attack_rule`) is checked record by record by `span_ok` instead.

use super::commands::provoke_rule;
use super::{Driver, Rv};
use crate::world::World;

/// The observation records found inside root rules' spans (the commands' own spans apart), each
/// checked or taken as described in the module's documentation.
#[derive(Debug, Default, Clone)]
pub struct SpanCounts {
    /// Rule events queued (`clog_q`), compared with the sim's queue at the run's end.
    pub clog_q: usize,
    /// Impatience (`imp_s`) and memory (`mem_q`) handlers' records, matched with the sim's.
    pub imp_s: usize,
    pub mem_q: usize,
    /// Provokes (`eng_prov`) queued to be matched with the sim's calls.
    pub eng_prov: usize,
    /// Projectile launches (`proj_launch`) equal to the sim's.
    pub proj_launch: usize,
    /// Records reported: of another family or kind, or one its checker rejects.
    pub reported: usize,
}

impl Driver<'_> {
    /// The records of the root rule at `start`, up to its `rule_end` at `end`, after the sim
    /// fired it (or didn't).
    pub(super) fn span_records(&mut self, w: &mut World, start: usize, end: usize) {
        let trace = self.trace;
        for r in &trace.records[start + 1..end] {
            match r["k"].as_str() {
                // The rule's own: its steps, handlers, results and environment (read by the code
                // that fires it) and the draws and mutations (compared in the T0 streams).
                Some(
                    "rule" | "rule_end" | "about" | "trig" | "trig_end" | "did" | "h" | "h_end"
                    | "result" | "bonuses" | "env" | "draw" | "mut",
                ) => {}
                Some("obs") => self.span_obs(w, start, r),
                other => self.span_reported(
                    w,
                    format!("a {} record", other.unwrap_or("?")),
                    "a record of a kind the replay doesn't take",
                ),
            }
        }
    }

    fn span_obs(&mut self, w: &mut World, root: usize, r: &Rv) {
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
                if self.eng_prov(w, r, provoke_rule(r)) {
                    self.out.spans.eng_prov += 1;
                } else {
                    self.span_reported(
                        w,
                        "eng_prov".into(),
                        "a provoke the engagement controller's handlers can't have made",
                    );
                }
            }
            Some("proj_launch") => {
                if self.launches_checked == Some(root) {
                    self.out.spans.proj_launch += 1;
                } else {
                    self.span_reported(
                        w,
                        "proj_launch".into(),
                        "a projectile launched by a rule the sim didn't fire as an attack",
                    );
                }
            }
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
