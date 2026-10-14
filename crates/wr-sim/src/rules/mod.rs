//! Ported rules, and their records in the T0 trace (`rule` when a rule is pushed, with
//! its parent and depth; `rule_end` after `OnDidTrigger`, with the rule's results), so a
//! sim trace lines up with WrathOracle's `rule`/`rule_end` records.

pub mod ability_params;
pub mod checks;
pub mod identify;

use serde_json::{Value, json};

use crate::rulebook::{RuleEvent, trigger};
use crate::world::World;

/// Triggers a rule through the rulebook and records it.
pub fn trigger_rule(w: &mut World, rule: &mut dyn RuleEvent) {
    let id = w.rulebook.next_seq;
    let parent = w.rulebook.stack.last().map_or(0, |s| s.1 + 1);
    let depth = w.rulebook.stack.len();
    let class = rule.chain()[0];
    let initiator = crate::events::unit_ref(&w.entity(rule.initiator()).unique_id);
    w.events.push(
        "rule",
        json!({ "id": id + 1, "parent": parent, "depth": depth, "class": class, "initiator": initiator }),
    );
    if let Err(e) = trigger(w, rule) {
        w.coverage.unsupported(format!("RulesOverflowException: {:?}", e.0));
    }
    let end = w.rules_end_fields.remove(&id).unwrap_or(Value::Null);
    w.events.push("rule_end", json!({ "id": id + 1, "class": class, "result": end }));
}

/// Called at the end of a rule's `OnTrigger` with its result fields (kept for its
/// `rule_end` record).
pub fn record_trigger_end(w: &mut World, rule: &dyn RuleEvent, fields: Value) {
    let seq = rule.base().seq;
    w.rules_end_fields.insert(seq, fields);
}
