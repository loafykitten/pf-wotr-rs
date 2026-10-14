//! Rule nesting, step order and subscriber order.

use std::cell::RefCell;
use std::rc::Rc;

use super::bus::{ExternalHandler, RuleStep, Sub};
use super::{RuleBase, RuleEvent, trigger};
use crate::world::{EntityIdx, World};

type Log = Rc<RefCell<Vec<String>>>;

struct TestRule {
    base: RuleBase,
    name: &'static str,
    chain: &'static [&'static str],
    initiator: EntityIdx,
    target: Option<EntityIdx>,
    /// Nested rules to trigger from `OnTrigger`.
    nested: usize,
    log: Log,
}

impl RuleEvent for TestRule {
    fn chain(&self) -> &'static [&'static str] {
        self.chain
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn target(&self) -> Option<EntityIdx> {
        self.target
    }
    fn on_trigger(&mut self, w: &mut World) {
        self.log.borrow_mut().push(format!("{}.OnTrigger", self.name));
        for _ in 0..self.nested {
            let mut inner = TestRule {
                base: RuleBase::default(),
                name: "inner",
                chain: &["RuleInner"],
                initiator: self.initiator,
                target: None,
                nested: 0,
                log: self.log.clone(),
            };
            trigger(w, &mut inner).expect("no overflow");
        }
    }
    fn base(&self) -> &RuleBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut RuleBase {
        &mut self.base
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

struct Handler {
    name: &'static str,
    ifaces: Vec<String>,
    log: Log,
}

impl ExternalHandler for Handler {
    fn interfaces(&self) -> Vec<String> {
        self.ifaces.clone()
    }
    fn event(&self, _w: &mut World, iface: &str) {
        self.log.borrow_mut().push(format!("{}:{iface}", self.name));
    }
    fn rule(&self, _w: &mut World, rule: &mut dyn RuleEvent, step: RuleStep) {
        self.log.borrow_mut().push(format!("{}:{step:?}:{}", self.name, rule.chain()[0]));
    }
}

fn add(w: &mut World, id: u32, name: &'static str, ifaces: &[&str], log: &Log) {
    w.bus.external.insert(
        id,
        Rc::new(Handler {
            name,
            ifaces: ifaces.iter().map(|s| (*s).to_owned()).collect(),
            log: log.clone(),
        }),
    );
    super::bus::subscribe(w, Sub::External(id));
}

fn world() -> (World, EntityIdx, EntityIdx) {
    let (mut w, a) = crate::stats::tests::world_with_unit();
    let b = w.add_entity(crate::world::Entity {
        unique_id: "b".into(),
        clr: "UnitEntityData",
        kind: crate::world::EntityKind::Unit(Box::default()),
        facts: crate::world::FactsManager::default(),
        parts: Vec::new(),
        is_turned_on: false,
        post_load_executed: true,
        is_in_game: true,
        subscribed: false,
        disposing: false,
        is_disposed: false,
    });
    (w, a, b)
}

#[test]
fn steps_and_managers_run_in_the_originals_order() {
    let (mut w, a, b) = world();
    let log: Log = Rc::default();
    // External handlers subscribe globally; to place them as target or initiator
    // subscribers we put them in those managers directly, as `Subscribe` would for a
    // unit-owned subscriber.
    add(&mut w, 1, "before", &["IBeforeRulebookEventTriggerHandler<RuleTest>"], &log);
    add(&mut w, 2, "after", &["IAfterRulebookEventTriggerHandler<RuleTest>"], &log);
    add(&mut w, 3, "hookA", &["IRulebookEventAboutToTriggerHook"], &log);
    add(&mut w, 4, "hookD", &["IRulebookEventDidTriggerHook"], &log);
    add(&mut w, 5, "global", &["IGlobalRulebookHandler<RuleBase_>"], &log);
    w.bus
        .rule_target
        .entry(b)
        .or_default()
        .entry("RuleTest".into())
        .or_default()
        .list
        .push(Some(Sub::External(5)));
    w.bus
        .rule_initiator
        .entry(a)
        .or_default()
        .entry("RuleTest".into())
        .or_default()
        .list
        .push(Some(Sub::External(5)));
    let mut r = TestRule {
        base: RuleBase::default(),
        name: "outer",
        chain: &["RuleTest", "RuleBase_"],
        initiator: a,
        target: Some(b),
        nested: 0,
        log: log.clone(),
    };
    trigger(&mut w, &mut r).unwrap();
    assert_eq!(
        log.borrow().clone(),
        vec![
            "before:BeforeHook:RuleTest",
            "hookA:AboutToTriggerHook:RuleTest",
            // Global manager: the rule's base type `RuleBase_` (the chain is walked).
            "global:AboutToTrigger:RuleTest",
            // Target, then initiator.
            "global:AboutToTrigger:RuleTest",
            "global:AboutToTrigger:RuleTest",
            "outer.OnTrigger",
            // The mod "after" hook runs before the did-trigger subscribers.
            "after:AfterHook:RuleTest",
            "global:DidTrigger:RuleTest",
            "global:DidTrigger:RuleTest",
            "global:DidTrigger:RuleTest",
            "hookD:DidTriggerHook:RuleTest",
        ]
    );
    assert!(r.base.triggered);
    assert!(w.rulebook.stack.is_empty());
}

#[test]
fn subscribers_run_in_subscription_order_and_unsubscribe_during_execution_is_deferred() {
    let (mut w, a, _) = world();
    let log: Log = Rc::default();
    add(&mut w, 10, "first", &["IGlobalRulebookHandler<RuleTest>"], &log);
    add(&mut w, 11, "second", &["IGlobalRulebookHandler<RuleTest>"], &log);
    add(&mut w, 12, "third", &["IGlobalRulebookHandler<RuleTest>"], &log);
    super::bus::unsubscribe(&mut w, Sub::External(11));
    let mut r = TestRule {
        base: RuleBase::default(),
        name: "r",
        chain: &["RuleTest"],
        initiator: a,
        target: None,
        nested: 0,
        log: log.clone(),
    };
    trigger(&mut w, &mut r).unwrap();
    let l = log.borrow().clone();
    assert_eq!(l[0], "first:AboutToTrigger:RuleTest");
    assert_eq!(l[1], "third:AboutToTrigger:RuleTest");
    assert_eq!(w.bus.rule_global["RuleTest"].list.len(), 2);
}

#[test]
fn rules_nest_synchronously_and_a_triggered_rule_cannot_rerun() {
    let (mut w, a, _) = world();
    let log: Log = Rc::default();
    w.rulebook.trace = Some(Vec::new());
    let mut r = TestRule {
        base: RuleBase::default(),
        name: "outer",
        chain: &["RuleOuter"],
        initiator: a,
        target: None,
        nested: 2,
        log: log.clone(),
    };
    trigger(&mut w, &mut r).unwrap();
    assert_eq!(log.borrow().clone(), vec!["outer.OnTrigger", "inner.OnTrigger", "inner.OnTrigger"]);
    let t = w.rulebook.trace.as_ref().unwrap();
    // Inner rules run at depth 2 inside the outer's OnTrigger step.
    let depths: Vec<(String, usize)> =
        t.iter().filter(|x| x.step == "OnTrigger").map(|x| (x.rule.to_owned(), x.depth)).collect();
    assert_eq!(
        depths,
        vec![("RuleOuter".into(), 1), ("RuleInner".into(), 2), ("RuleInner".into(), 2)]
    );
    assert_eq!(t.iter().find(|x| x.rule == "RuleInner").unwrap().seq, 1);
    let n = log.borrow().len();
    trigger(&mut w, &mut r).unwrap();
    assert_eq!(log.borrow().len(), n, "Can't trigger rule twice");
}

struct Recursive {
    base: RuleBase,
    initiator: EntityIdx,
    after: Rc<RefCell<u32>>,
}

impl RuleEvent for Recursive {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleRecursive"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn on_trigger(&mut self, w: &mut World) {
        let mut inner = Recursive {
            base: RuleBase::default(),
            initiator: self.initiator,
            after: self.after.clone(),
        };
        let _ = trigger(w, &mut inner);
        // Code after a nested trigger that overflowed still runs in this frame (the C#
        // exception unwinds instead; the rulebook stops at this rule's next step).
        *self.after.borrow_mut() += 1;
    }
    fn base(&self) -> &RuleBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut RuleBase {
        &mut self.base
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[test]
fn nesting_past_25_overflows_and_resets_the_context() {
    let (mut w, a, _) = world();
    let after = Rc::new(RefCell::new(0));
    let mut r = Recursive { base: RuleBase::default(), initiator: a, after: after.clone() };
    let e = trigger(&mut w, &mut r).expect_err("overflow");
    assert_eq!(e.0.len(), 26);
    assert!(w.rulebook.stack.is_empty());
    assert!(w.rulebook.pending_overflow.is_none());
    assert_eq!(w.rulebook.overflows, 1);
    // No rule reached its did-trigger step.
    assert!(!r.base.triggered);
}
