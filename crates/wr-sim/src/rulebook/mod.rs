//! The rulebook: synchronous, nested `RulebookEvent`s (`Kingmaker.RuleSystem/Rulebook.cs`,
//! `RulebookEvent.cs`, `RulebookEventContext.cs`, `Kingmaker.PubSubSystem/RulebookEventBus.cs`).
//!
//! `Rulebook.TriggerEvent(evt)`: refuse a triggered rule; push it on the context stack
//! (`StackSize > 25` throws `RulesOverflowException` and resets the context); then
//! `TriggerEventInternal`, each step wrapped so an exception is logged and the next step
//! still runs (`SaveExecute`, which rethrows only the overflow):
//!
//! 1. `AssignSource` (the rule's reason);
//! 2. `IBeforeRulebookEventTriggerHandler<T>` (global `EventBus` subscribers, the mod hook);
//! 3. `RulebookEventBus.OnEventAboutToTrigger`: `IRulebookEventAboutToTriggerHook`s, then
//!    global, then the rule target's, then the initiator's `OnEventAboutToTrigger`
//!    subscribers, each manager walking the rule's type and its bases up to (excluding)
//!    `RulebookEvent`, each list in subscription order;
//! 4. the rule's own `OnTrigger`;
//! 5. `IAfterRulebookEventTriggerHandler<T>` (so the mod "after" hook runs *before* the
//!    ordinary did-trigger subscribers);
//! 6. `RulebookEventBus.OnEventDidTrigger` (global, target, initiator, then
//!    `IRulebookEventDidTriggerHook`s) followed by `OnDidTrigger` (mark triggered, remove the
//!    rule's temporary modifiers).
//!
//! Then pop, and if this was the outermost rule, start a fresh context.

pub mod bus;

use crate::world::{EntityIdx, World};

/// A rule. `chain` lists the rule's C# type and its bases (most derived first, up to but
/// excluding `RulebookEvent`): subscription managers dispatch on each in turn.
pub trait RuleEvent {
    fn chain(&self) -> &'static [&'static str];
    fn initiator(&self) -> EntityIdx;
    /// `GetRuleTarget()`.
    fn target(&self) -> Option<EntityIdx> {
        None
    }
    /// Whether WrathOracle records the rule's target: its `GetRuleTarget` passes the oracle's
    /// purity check (`RuleCastSpell`'s and `RuleApplySpell`'s don't: `$impure`, a null target).
    fn record_target(&self) -> bool {
        true
    }
    /// `OnTrigger(context)`.
    fn on_trigger(&mut self, w: &mut World);
    fn base(&self) -> &RuleBase;
    fn base_mut(&mut self) -> &mut RuleBase;
    fn as_any(&mut self) -> &mut dyn std::any::Any;
    /// `AllBonuses` (its `m_ModifiableBonus`'s modifiers) after it triggered, and every
    /// `AddModifier` call in order (rejected ones too), each as `[value, descriptor, type, stat,
    /// fact id]`: none for a rule whose port adds none.
    fn all_bonuses(&self, w: &World) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
        let _ = w;
        (Vec::new(), Vec::new())
    }
}

/// The members every `RulebookEvent` has.
#[derive(Debug, Default, Clone)]
pub struct RuleBase {
    pub triggered: bool,
    /// `m_TemporaryModifiers`, removed by `OnDidTrigger`.
    pub temporary_modifiers: Vec<crate::world::ModIdx>,
    /// `Reason`, as a description of what it was assigned from.
    pub reason: Option<String>,
    /// Sequence number within the run, for traces.
    pub seq: u64,
}

/// One entry of the rule trace (when tracing): what ran, for which rule, at what depth.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RuleTrace {
    pub seq: u64,
    pub depth: usize,
    pub rule: &'static str,
    pub step: String,
}

/// One handler the bus delivered a rule to, as the replay compares it with the recording's `h`
/// records (`combat/replay/spans.rs`): kept only while a replay turns the log on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerRun {
    /// `mod_before`, `about`, `mod_after` or `did`: the recorded `h` record's phase.
    pub phase: &'static str,
    /// The rule's class (short).
    pub rule: String,
    pub kind: HandlerKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandlerKind {
    /// A fact's component: its type (short class), name (`$Type$name`), the fact's id and
    /// blueprint, and its owner's id.
    Component {
        class: String,
        name: String,
        fact: String,
        blueprint: Option<String>,
        owner: Option<String>,
    },
    /// A fact that is its own handler (`ActivatableAbility`): its class.
    Fact(&'static str),
}

/// `Rulebook` state: the current `RulebookEventContext`'s stack.
#[derive(Debug, Default)]
pub struct Rulebook {
    /// Names of the rules on the stack (the events themselves live on the Rust stack).
    pub stack: Vec<(&'static str, u64)>,
    pub next_seq: u64,
    pub trace: Option<Vec<RuleTrace>>,
    /// Count of rules dropped by the overflow guard.
    pub overflows: u64,
    /// A `RulesOverflowException` in flight: it unwinds through every enclosing rule
    /// (`SaveExecute` rethrows it), so each level stops at its next step.
    pub pending_overflow: Option<RulesOverflow>,
    /// The handlers rules were delivered to, in order, while a replay keeps the log.
    pub handler_log: Option<Vec<HandlerRun>>,
}

/// `RulesOverflowException`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RulesOverflow(pub Vec<&'static str>);

fn record(w: &mut World, rule: &dyn RuleEvent, step: impl Into<String>) {
    let depth = w.rulebook.stack.len();
    let seq = rule.base().seq;
    let name = rule.chain().first().copied().unwrap_or("?");
    if let Some(t) = &mut w.rulebook.trace {
        t.push(RuleTrace { seq, depth, rule: name, step: step.into() });
    }
}

/// `Rulebook.TriggerEvent(evt)`.
///
/// # Errors
///
/// [`RulesOverflow`] when the nesting passes 25, as the C# throws (the context is reset).
pub fn trigger(w: &mut World, rule: &mut dyn RuleEvent) -> Result<(), RulesOverflow> {
    if rule.base().triggered {
        w.log("error", "Can't trigger rule twice");
        return Ok(());
    }
    let outermost = w.rulebook.stack.is_empty();
    let seq = w.rulebook.next_seq;
    w.rulebook.next_seq += 1;
    rule.base_mut().seq = seq;
    let name = rule.chain().first().copied().unwrap_or("?");
    w.rulebook.stack.push((name, seq));
    if w.rulebook.stack.len() > 25 {
        let names: Vec<&'static str> = w.rulebook.stack.iter().map(|s| s.0).collect();
        w.rulebook.stack.clear();
        w.rulebook.overflows += 1;
        w.rulebook.pending_overflow = Some(RulesOverflow(names.clone()));
        return Err(RulesOverflow(names));
    }
    if let Err(e) = trigger_internal(w, rule) {
        if outermost {
            w.rulebook.pending_overflow = None;
        }
        return Err(e);
    }
    if !w.rulebook.stack.is_empty() {
        w.rulebook.stack.pop();
    }
    if outermost {
        w.rulebook.stack.clear();
    }
    Ok(())
}

fn trigger_internal(w: &mut World, rule: &mut dyn RuleEvent) -> Result<(), RulesOverflow> {
    let check = |w: &World| match &w.rulebook.pending_overflow {
        Some(e) => Err(e.clone()),
        None => Ok(()),
    };
    assign_source(w, rule);
    record(w, rule, "BeforeEvent");
    bus::raise_rule_hooks(w, rule, bus::RuleHook::Before);
    check(w)?;
    record(w, rule, "AboutToTrigger");
    // `RulebookEventHooksManager.OnBeforeEventAboutToTrigger` (the hooks run first): the game log.
    crate::game_log::about_to_trigger(w, rule);
    bus::rulebook_about_to_trigger(w, rule);
    check(w)?;
    record(w, rule, "OnTrigger");
    rule.on_trigger(w);
    check(w)?;
    record(w, rule, "AfterEvent");
    bus::raise_rule_hooks(w, rule, bus::RuleHook::After);
    check(w)?;
    record(w, rule, "DidTrigger");
    bus::rulebook_did_trigger(w, rule);
    // `RulebookEventHooksManager.OnAfterEventDidTrigger` (after the subscribers): the game log.
    crate::game_log::did_trigger(w, rule);
    on_did_trigger(w, rule);
    check(w)?;
    Ok(())
}

/// `Rulebook.AssignSource`: keep an existing reason; attacks and casts are their own
/// reason; else the fact in the current `ContextData`, the previous rule's reason, the
/// mechanics context, or the rule itself. The sim has no `ContextData` stack yet, so only
/// the first, the previous-rule and the self cases apply.
fn assign_source(w: &mut World, rule: &mut dyn RuleEvent) {
    if rule.base().reason.is_some() {
        return;
    }
    let n = w.rulebook.stack.len();
    let reason = if n >= 2 {
        let (prev, seq) = w.rulebook.stack[n - 2];
        format!("previous:{prev}#{seq}")
    } else {
        "self".to_owned()
    };
    rule.base_mut().reason = Some(reason);
}

/// `RulebookEvent.OnDidTrigger()`.
fn on_did_trigger(w: &mut World, rule: &mut dyn RuleEvent) {
    rule.base_mut().triggered = true;
    let mods = std::mem::take(&mut rule.base_mut().temporary_modifiers);
    for m in mods {
        crate::stats::ops::remove(w, m);
    }
}

#[cfg(test)]
mod tests;
