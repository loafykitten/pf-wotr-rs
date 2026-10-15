//! Triggering rules from combat code and writing the sim's side of the T0 stream in the
//! projected vocabulary of [`super::project`]: `rule` when a rule is pushed, draws made
//! inside rules where they happen, `mut damage` for HP changes across a rule (attributed to
//! the innermost rule, as WrathOracle's `Rules.OnDidTriggerPostfix` does), then the rule's
//! `result`.

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::{Value as J, json};

use crate::rng::{CallSite, Draw, Rng, RngFailure};
use crate::rulebook::RuleEvent;
use crate::world::{EntityIdx, World};

/// Draws seen while rules run, waiting to be written before the next record.
#[derive(Debug, Default)]
pub struct DrawSink {
    /// Rules on the stack that were fired through [`fire`] (or any rule: see
    /// [`Recording`]).
    pub depth: usize,
    pub pending: Vec<J>,
    /// Per open rule: its initiator and target with their `Damage` baselines (an inner
    /// rule's report moves the enclosing rules' baselines).
    pub frames: Vec<[(Option<EntityIdx>, i32); 2]>,
}

/// An [`Rng`] that reports draws made inside rules to a [`DrawSink`].
#[derive(Debug)]
pub struct Recording<R: Rng> {
    pub inner: R,
    pub sink: Rc<RefCell<DrawSink>>,
}

/// A draw value as the projection writes it (integers as integers, floats as `f32` text).
#[must_use]
pub fn draw_value(v: f64, int: bool) -> J {
    if int {
        #[allow(clippy::cast_possible_truncation)]
        let i = v as i64;
        json!(i)
    } else {
        #[allow(clippy::cast_possible_truncation)]
        let f = v as f32;
        json!(format!("{f:?}"))
    }
}

impl<R: Rng> Recording<R> {
    fn note(&self, site: CallSite, v: J) {
        let mut s = self.sink.borrow_mut();
        if s.depth > 0 {
            s.pending.push(json!({"k": "draw", "site": site.0, "v": v}));
        }
    }
}

impl<R: Rng> Rng for Recording<R> {
    fn range_int(&mut self, site: CallSite, min: i32, max: i32) -> i32 {
        let v = self.inner.range_int(site, min, max);
        self.note(site, json!(v));
        v
    }

    fn range_float(&mut self, site: CallSite, min: f32, max: f32) -> f32 {
        let v = self.inner.range_float(site, min, max);
        self.note(site, draw_value(f64::from(v), false));
        v
    }

    fn value(&mut self, site: CallSite) -> f32 {
        let v = self.inner.value(site);
        self.note(site, draw_value(f64::from(v), false));
        v
    }

    fn log(&self) -> &[Draw] {
        self.inner.log()
    }

    fn failures(&self) -> &[RngFailure] {
        self.inner.failures()
    }

    fn finish(&mut self) {
        self.inner.finish();
    }
}

/// The recording run's [`Cx`], if a combat run is recording: rules triggered by shared
/// code (fact processors, components) record through it.
#[must_use]
pub fn current(w: &World) -> Option<Cx> {
    w.combat.recording.clone()
}

/// Sets (or clears) the recording run.
pub fn set_current(w: &mut World, cx: Option<Cx>) {
    w.combat.recording = cx;
}

/// The combat run's link to its draw sink.
#[derive(Debug, Clone, Default)]
pub struct Cx {
    pub sink: Rc<RefCell<DrawSink>>,
}

impl Cx {
    /// Writes pending draws, then `rec` (`{"k": …, …}`), into the run's event log.
    pub fn emit(&self, w: &mut World, rec: J) {
        let pending = std::mem::take(&mut self.sink.borrow_mut().pending);
        for d in pending {
            push(w, d);
        }
        push(w, rec);
    }
}

fn push(w: &mut World, rec: J) {
    let J::Object(mut m) = rec else { return };
    let k = match m.remove("k").as_ref().and_then(J::as_str) {
        Some("rule") => "rule",
        Some("result") => "result",
        Some("bonuses") => "bonuses",
        Some("draw") => "draw",
        Some("mut") => "mut",
        Some("tb") => "tb",
        Some("cmd") => "cmd",
        Some("input") => "input",
        _ => "other",
    };
    w.events.push(k, J::Object(m));
}

/// A unit reference as the projection writes it (the id, or null).
#[must_use]
pub fn uid(w: &World, u: Option<EntityIdx>) -> J {
    u.map_or(J::Null, |u| J::String(w.entity(u).unique_id.clone()))
}

/// Triggers `rule` (`Rulebook.Trigger`) and records it: `rule` at the push, draws and
/// nested rules as they happen, HP changes across the rule, then `result` (when `result`
/// gives one). `target_pure` says whether WrathOracle can read the rule's target (its
/// `GetRuleTarget` passes the purity check); otherwise the record's target is null, as the
/// projection writes `$impure`.
pub fn fire<R: RuleEvent>(
    w: &mut World,
    cx: &Cx,
    rule: &mut R,
    result: impl FnOnce(&R) -> Option<J>,
) {
    let class = rule.chain()[0];
    let a = rule.initiator();
    let b = rule.target();
    let depth = w.rulebook.stack.len();
    let recorded = if rule.record_target() { uid(w, b) } else { J::Null };
    cx.emit(
        w,
        json!({"k": "rule", "class": class, "initiator": uid(w, Some(a)), "target": recorded, "depth": depth}),
    );
    let dmg = |w: &World, u: EntityIdx| w.unit(u).map_or(0, |x| x.damage);
    let b2 = b.filter(|b| *b != a);
    let frame = [(Some(a), dmg(w, a)), (b2, b2.map_or(0, |b| dmg(w, b)))];
    {
        let mut s = cx.sink.borrow_mut();
        s.depth += 1;
        s.frames.push(frame);
    }
    if let Err(e) = crate::rulebook::trigger(w, rule) {
        w.coverage.unsupported(format!("RulesOverflowException: {:?}", e.0));
    }
    let frame = {
        let mut s = cx.sink.borrow_mut();
        s.depth -= 1;
        s.frames.pop().unwrap_or(frame)
    };
    // A rule ported in `rules/` keeps its fields for a `rule_end` record: not used here.
    w.rules_end_fields.remove(&rule.base().seq);
    for (u, before) in frame {
        let Some(u) = u else { continue };
        let now = dmg(w, u);
        if now != before {
            cx.emit(
                w,
                json!({"k": "mut", "what": "damage", "unit": w.entity(u).unique_id, "old": before, "new": now}),
            );
            for f in &mut cx.sink.borrow_mut().frames {
                for slot in f.iter_mut() {
                    if slot.0 == Some(u) {
                        slot.1 = now;
                    }
                }
            }
        }
    }
    // Only the classes the projection compares results for get a `result` record, with
    // exactly the compared fields.
    if let Some(fields) =
        super::project::RESULT_FIELDS.iter().find(|(c, _)| *c == class).map(|(_, f)| *f)
        && let Some(r) = result(rule)
    {
        let mut res = serde_json::Map::new();
        for f in fields {
            res.insert((*f).to_owned(), r.get(*f).cloned().unwrap_or(J::Null));
        }
        cx.emit(w, json!({"k": "result", "class": class, "result": res}));
    }
    // Its bonuses and the additions asked for, when it has any (the projection's `bonuses`).
    let (bonuses, adds) = rule.all_bonuses(w);
    if !bonuses.is_empty() || !adds.is_empty() {
        cx.emit(w, json!({"k": "bonuses", "class": class, "list": bonuses, "adds": adds}));
    }
}
