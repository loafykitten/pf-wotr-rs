//! `EventBus` and `RulebookEventBus` subscriptions, in the original's order.
//!
//! `EventBus.Subscribe(obj)`: if `obj` is a subscription proxy (a delegate component's
//! runtime), subscribe its delegate's interfaces with the proxy as the listener; then
//! `obj`'s own interfaces; then `RulebookEventBus.Subscribe(obj)` (global, target,
//! initiator managers, then the hooks manager). Each (interface or rule type) has its own
//! `SubscribersList`: append on subscribe; on unsubscribe remove, or, while the list is
//! executing, null the slot and compact after (`SubscribersList.RemoveSubscriber` /
//! `Cleanup`); raising iterates the count captured at the start.
//!
//! Interfaces come from the decompiled declarations: components from the content
//! schema's `handlers`, facts and entities from the save schema's interfaces.

use std::collections::HashMap;

use super::RuleEvent;
use crate::world::{CompRef, EntityIdx, FactIdx, World};

/// A subscriber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sub {
    Component(CompRef),
    Fact(FactIdx),
    Entity(EntityIdx),
    /// A component runtime subscribed as itself, after its delegate's interfaces were
    /// subscribed through it as a proxy (`EventBus.Subscribe` on an `ISubscriptionProxy`):
    /// `EtudeBracketRuntime` and `EtudeInvokeActionDelayedData`. Delivery prefers the
    /// delegate when it implements the raised interface (`ExecuteAction`).
    Runtime(CompRef),
    /// A handler outside the game's own types (tests, the oracle's hooks), dispatched
    /// through [`EventBus::external`].
    External(u32),
    /// A unit part (`EntityPart.Subscribe` in its `TurnOn`): the unit and the part's class. Only
    /// the parts in [`PART_HANDLERS`] subscribe in the sim.
    Part(EntityIdx, &'static str),
}

/// `SubscribersList<T>`.
#[derive(Debug, Default, Clone)]
pub struct SubList {
    pub list: Vec<Option<Sub>>,
    executing: i32,
    needs_cleanup: bool,
}

impl SubList {
    fn add(&mut self, s: Sub) {
        self.list.push(Some(s));
    }

    fn remove(&mut self, s: Sub) {
        if let Some(p) = self.list.iter().position(|x| *x == Some(s)) {
            if self.executing > 0 {
                self.needs_cleanup = true;
                self.list[p] = None;
            } else {
                self.list.remove(p);
            }
        }
    }

    fn cleanup(&mut self) {
        if self.needs_cleanup {
            self.list.retain(Option::is_some);
            self.needs_cleanup = false;
        }
    }
}

/// A handler for [`Sub::External`].
pub trait ExternalHandler {
    fn interfaces(&self) -> Vec<String>;
    fn event(&self, w: &mut World, iface: &str);
    fn rule(&self, w: &mut World, rule: &mut dyn RuleEvent, step: RuleStep);
}

/// Which rulebook callback is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleStep {
    BeforeHook,
    AboutToTriggerHook,
    AboutToTrigger,
    AfterHook,
    DidTrigger,
    DidTriggerHook,
}

/// The two mod-facing hooks around a rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleHook {
    Before,
    After,
}

#[derive(Default)]
pub struct EventBus {
    /// `EventBus.GlobalSubscribers`, by interface name.
    pub global: HashMap<String, SubList>,
    /// `EventBus.UnitSubscribers`, by unit and interface name.
    pub unit: HashMap<EntityIdx, HashMap<String, SubList>>,
    /// `RulebookEventBus.GlobalRulebookSubscribers`, by rule type.
    pub rule_global: HashMap<String, SubList>,
    pub rule_target: HashMap<EntityIdx, HashMap<String, SubList>>,
    pub rule_initiator: HashMap<EntityIdx, HashMap<String, SubList>>,
    /// `RulebookEventHooksManager` lists.
    pub about_to_hooks: Vec<Sub>,
    pub did_hooks: Vec<Sub>,
    pub external: HashMap<u32, std::rc::Rc<dyn ExternalHandler>>,
    /// The event being raised: its method and unit argument (innermost last), for
    /// handlers that need them. `raise_global` without arguments pushes `("", None)`.
    pub call: Vec<(&'static str, Option<EntityIdx>)>,
    /// The fact argument of the event being raised, in step with [`EventBus::call`]
    /// (`HandleUnitGainFact(fact)`, `HandleBuffDidAdded(buff)`).
    pub call_fact: Vec<Option<FactIdx>>,
    /// `HandleUnitCalculateSkillPointsOnLevelup(state, ref extraSkillPoints)` being raised:
    /// the state's `NextCharacterLevel` and the extra points handlers add (innermost last).
    pub skill_points: Vec<(i32, i32)>,
    /// `CalculateMaxResourceAmount(resource, ref bonus)` being raised: the resource and the
    /// bonus handlers add (innermost last).
    pub resource_bonus: Vec<(wr_content::Guid, i32)>,
    /// The slot (owner and name) of the equipment event being raised, innermost last.
    pub slot: Vec<(EntityIdx, String)>,
    /// The collection (its identity) and item of the `IItemsCollectionHandler` event being
    /// raised, innermost last.
    pub item: Vec<(usize, EntityIdx)>,
    /// The command of the `IUnitRunCommandHandler`, `IUnitCommandActHandler` or
    /// `IUnitCommandEndHandler` event being raised, innermost last.
    pub command: Vec<CommandArg>,
}

/// What a command event's handlers read of the command (`UnitCommand`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandArg {
    pub executor: Option<EntityIdx>,
    /// `Type` (`combat::commands::ty`).
    pub ty: u8,
    /// `command is UnitActivateAbility`.
    pub activate_ability: bool,
    /// `Result` (`combat::commands::res`).
    pub result: u8,
    pub acted: bool,
    pub finished: bool,
}

impl std::fmt::Debug for EventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventBus")
            .field("global", &self.global.len())
            .field("rule_global", &self.rule_global.len())
            .finish_non_exhaustive()
    }
}

/// How an interface subscribes.
enum Kind<'a> {
    Global(&'a str),
    RuleGlobal(&'a str),
    RuleTarget(&'a str),
    RuleInitiator(&'a str),
    AboutToHook,
    DidHook,
    Ignore,
}

fn classify(iface: &str) -> Kind<'_> {
    let generic = |prefix: &str| iface.strip_prefix(prefix).and_then(|r| r.strip_suffix('>'));
    if let Some(t) = generic("IInitiatorRulebookHandler<") {
        return Kind::RuleInitiator(t);
    }
    if let Some(t) = generic("ITargetRulebookHandler<") {
        return Kind::RuleTarget(t);
    }
    if let Some(t) = generic("IGlobalRulebookHandler<") {
        return Kind::RuleGlobal(t);
    }
    if iface == "IRulebookEventAboutToTriggerHook" {
        return Kind::AboutToHook;
    }
    if iface == "IRulebookEventDidTriggerHook" {
        return Kind::DidHook;
    }
    if iface.starts_with("IRulebookHandler<")
        || iface.ends_with("RulebookSubscriber")
        || iface == "IGlobalSubscriber"
        || iface == "IUnitSubscriber"
        || iface == "ISubscriber"
    {
        return Kind::Ignore;
    }
    Kind::Global(iface)
}

fn interfaces(w: &World, s: Sub) -> Vec<String> {
    match s {
        Sub::Component(c) => w
            .component(c)
            .ty()
            .map(|t| wr_content::schema::ty(t).handlers.iter().map(|h| (*h).to_owned()).collect())
            .unwrap_or_default(),
        Sub::Fact(f) => {
            let mut out = save_ifaces(w.fact(f).clr);
            for (clr, ifaces) in FACT_HANDLERS {
                if *clr == w.fact(f).clr {
                    for i in *ifaces {
                        if !out.iter().any(|o| o == i) {
                            out.push((*i).to_owned());
                        }
                    }
                }
            }
            out
        }
        Sub::Entity(e) => {
            let mut out = save_ifaces(w.entity(e).clr);
            for (clr, ifaces) in ENTITY_HANDLERS {
                if *clr == w.entity(e).clr {
                    for i in *ifaces {
                        if !out.iter().any(|o| o == i) {
                            out.push((*i).to_owned());
                        }
                    }
                }
            }
            out
        }
        Sub::External(id) => w.bus.external.get(&id).map(|h| h.interfaces()).unwrap_or_default(),
        Sub::Part(_, clr) => PART_HANDLERS
            .iter()
            .find(|(c, _)| *c == clr)
            .map(|(_, i)| i.iter().map(|x| (*x).to_owned()).collect())
            .unwrap_or_default(),
        Sub::Runtime(c) => w
            .component(c)
            .ty()
            .map(|t| {
                crate::components::etudes::runtime_interfaces(t)
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// The handler interfaces of the `EntityDataBase` classes that implement any, with the inherited
/// ones (from the decompiled declarations): the save schema keeps only the interfaces that are
/// serialized types, so it has none of these.
const ENTITY_HANDLERS: &[(&str, &[&str])] = &[
    ("AreaEffectEntityData", &["IAreaHandler", "IAreaPartHandler"]),
    ("CutscenePlayerData", &["IAreaHandler"]),
    ("DetailedTrapObjectData", &["IAreaHandler"]),
    (
        "EtudesSystem",
        &[
            "IUnlockHandler",
            "IUnlockValueHandler",
            "ICompanionChangeHandler",
            "IPartyHandler",
            "IAreaHandler",
            "IQuestObjectiveHandler",
            "ITimeOfDayChangedHandler",
            "ISummonPoolHandler",
            "IAreaPartHandler",
        ],
    ),
    ("KingdomState", &["IChapterChangeHandler"]),
    ("SimpleTrapObjectData", &["IAreaHandler"]),
    ("TrapObjectData", &["IAreaHandler"]),
    ("UnitEntityData", &["IAreaHandler"]),
];

/// The handler interfaces of the unit parts the sim subscribes (from their declarations).
const PART_HANDLERS: &[(&str, &[&str])] =
    &[(crate::components::shifters_fury::PART, crate::components::shifters_fury::PART_HANDLERS)];

/// The handler interfaces of the `EntityFact` classes that implement any (from the decompiled
/// declarations; `UnitFact` and `EntityFact` implement none): the save schema has none of these
/// either.
const FACT_HANDLERS: &[(&str, &[&str])] = &[
    (
        "ActivatableAbility",
        &[
            "IUnitCommandActHandler",
            "IUnitCommandEndHandler",
            "IUnitRunCommandHandler",
            "IInitiatorRulebookHandler<RuleAttackWithWeapon>",
            "IInitiatorRulebookHandler<RuleCastSpell>",
            "IInitiatorRulebookHandler<RuleDealDamage>",
            "IInitiatorRulebookHandler<RuleAttackWithWeaponResolve>",
            "IUnitCombatHandler",
            "IUnitBuffHandler",
            "IApplyAbilityEffectHandler",
        ],
    ),
    ("KingdomMoraleFlag", &["IKingdomDayHandler"]),
];

/// Interfaces of a save-side CLR type (short name), from the save schema.
fn save_ifaces(clr: &str) -> Vec<String> {
    let types = wr_save::schema::types();
    let Some(t) = types.iter().find(|t| t.short() == clr) else { return Vec::new() };
    let mut out = Vec::new();
    let mut cur = Some(t);
    let mut n = 0;
    while let Some(d) = cur {
        for &i in d.ifaces {
            let name = wr_save::schema::ty(i).short().to_owned();
            if (name.contains("Handler") || name.contains("Subscriber")) && !out.contains(&name) {
                out.push(name);
            }
        }
        n += 1;
        cur = (d.base != wr_save::schema::NO_TYPE && n < 64).then(|| wr_save::schema::ty(d.base));
    }
    out
}

/// The unit a subscriber subscribes for (`GetSubscribingUnit`): the owner if a unit, the
/// wielder if an item.
fn subscribing_unit(w: &World, s: Sub) -> Option<EntityIdx> {
    let owner = match s {
        Sub::Component(c) => w.fact(c.fact).owner,
        // The runtime's own interfaces are global ones.
        Sub::Runtime(_) => None,
        Sub::Fact(f) => w.fact(f).owner,
        Sub::Entity(e) | Sub::Part(e, _) => Some(e),
        Sub::External(_) => None,
    }?;
    if w.unit(owner).is_some() {
        return Some(owner);
    }
    w.item(owner).and_then(|i| i.wielder)
}

/// `EventBus.Subscribe(subscriber)`.
pub fn subscribe(w: &mut World, s: Sub) {
    let unit = subscribing_unit(w, s);
    for iface in interfaces(w, s) {
        match classify(&iface) {
            Kind::Global(n) => {
                w.bus.global.entry(n.to_owned()).or_default().add(s);
                if let Some(u) = unit {
                    w.bus.unit.entry(u).or_default().entry(n.to_owned()).or_default().add(s);
                }
            }
            Kind::RuleGlobal(t) => w.bus.rule_global.entry(t.to_owned()).or_default().add(s),
            Kind::RuleTarget(t) => match unit {
                Some(u) => {
                    w.bus.rule_target.entry(u).or_default().entry(t.to_owned()).or_default().add(s)
                }
                None => w.log("error", "Could not subscribe, it didnt provide a unit"),
            },
            Kind::RuleInitiator(t) => match unit {
                Some(u) => w
                    .bus
                    .rule_initiator
                    .entry(u)
                    .or_default()
                    .entry(t.to_owned())
                    .or_default()
                    .add(s),
                None => w.log("error", "Could not subscribe, it didnt provide a unit"),
            },
            Kind::AboutToHook => w.bus.about_to_hooks.push(s),
            Kind::DidHook => w.bus.did_hooks.push(s),
            Kind::Ignore => {}
        }
    }
}

/// `EventBus.Unsubscribe(subscriber)`.
pub fn unsubscribe(w: &mut World, s: Sub) {
    let unit = subscribing_unit(w, s);
    for iface in interfaces(w, s) {
        match classify(&iface) {
            Kind::Global(n) => {
                if let Some(l) = w.bus.global.get_mut(n) {
                    l.remove(s);
                }
                if let Some(u) = unit
                    && let Some(m) = w.bus.unit.get_mut(&u)
                    && let Some(l) = m.get_mut(n)
                {
                    l.remove(s);
                }
            }
            Kind::RuleGlobal(t) => {
                if let Some(l) = w.bus.rule_global.get_mut(t) {
                    l.remove(s);
                }
            }
            Kind::RuleTarget(t) | Kind::RuleInitiator(t) => {
                let map = if matches!(classify(&iface), Kind::RuleTarget(_)) {
                    &mut w.bus.rule_target
                } else {
                    &mut w.bus.rule_initiator
                };
                if let Some(u) = unit
                    && let Some(m) = map.get_mut(&u)
                {
                    if let Some(l) = m.get_mut(t) {
                        l.remove(s);
                    }
                    if m.values().all(|l| l.list.is_empty()) {
                        map.remove(&u);
                    }
                }
            }
            Kind::AboutToHook => w.bus.about_to_hooks.retain(|x| *x != s),
            Kind::DidHook => w.bus.did_hooks.retain(|x| *x != s),
            Kind::Ignore => {}
        }
    }
}

pub fn subscribe_component(w: &mut World, c: CompRef) {
    if !w.component(c).subscribed {
        w.component_mut(c).subscribed = true;
        subscribe(w, Sub::Component(c));
        subscribe(w, Sub::Runtime(c));
    }
}

pub fn unsubscribe_component(w: &mut World, c: CompRef) {
    if w.component(c).subscribed {
        w.component_mut(c).subscribed = false;
        unsubscribe(w, Sub::Component(c));
        unsubscribe(w, Sub::Runtime(c));
    }
}

pub fn subscribe_fact(w: &mut World, f: FactIdx) {
    subscribe(w, Sub::Fact(f));
}

pub fn unsubscribe_fact(w: &mut World, f: FactIdx) {
    unsubscribe(w, Sub::Fact(f));
}

/// Subscriber dispatch catches every exception (`SubscribersList` / `SubscriptionManager`
/// wrap each call in `try`/`catch (Exception)`), including `RulesOverflowException`: an
/// overflow raised inside one subscriber stops there.
fn catch_overflow(w: &mut World, was_pending: bool) {
    if !was_pending {
        w.rulebook.pending_overflow = None;
    }
}

fn deliver_event(w: &mut World, s: Sub, iface: &str) {
    let was = w.rulebook.pending_overflow.is_some();
    deliver_event_inner(w, s, iface);
    catch_overflow(w, was);
}

fn deliver_event_inner(w: &mut World, s: Sub, iface: &str) {
    if let Sub::Runtime(c) = s {
        // `ExecuteAction`: the delegate if it implements the interface, else the runtime.
        let Some(ty) = w.component(c).ty() else { return };
        if wr_content::schema::ty(ty).handlers.contains(&iface) {
            return deliver_event_inner(w, Sub::Component(c), iface);
        }
        crate::components::etudes::runtime_event(w, c, iface);
        return;
    }
    match s {
        Sub::Component(c) => {
            let Some(ty) = w.component(c).ty() else { return };
            let name = wr_content::schema::ty(ty).short;
            let reg = w.registry.clone();
            match reg.component(name) {
                Some(l) if l.handlers().contains(&iface) => {
                    w.coverage.executed(format!("{name}.{iface}"));
                    l.event(w, c, iface);
                }
                _ => w.coverage.unsupported(format!("{name}: {iface}")),
            }
        }
        Sub::Runtime(_) => {}
        Sub::Fact(f) => {
            if w.fact(f).clr == "ActivatableAbility"
                && crate::unit_facts::activatable_event(w, f, iface)
            {
                w.coverage.executed(format!("ActivatableAbility.{iface}"));
            } else {
                w.coverage.unsupported(format!("{}: {iface}", w.fact(f).clr));
            }
        }
        Sub::Entity(e) => {
            if crate::narrative::entity_event(w, e, iface) {
                w.coverage.executed(format!("{}.{iface}", w.entity(e).clr));
            } else {
                w.coverage.unsupported(format!("{}: {iface}", w.entity(e).clr));
            }
        }
        Sub::External(id) => {
            if let Some(h) = w.bus.external.get(&id).cloned() {
                h.event(w, iface);
            }
        }
        Sub::Part(e, clr) => {
            if clr == crate::components::shifters_fury::PART
                && crate::components::shifters_fury::part_event(w, e, iface)
            {
                w.coverage.executed(format!("{clr}.{iface}"));
            } else {
                w.coverage.unsupported(format!("{clr}: {iface}"));
            }
        }
    }
}

/// Pins a list accessor's signature to the higher-ranked form `run_list` takes.
fn hr<F: for<'a> Fn(&'a mut World) -> Option<&'a mut SubList>>(f: F) -> F {
    f
}

/// Iterates a `SubscribersList` the way `RaiseEvent` does: count captured first, nulled
/// slots skipped, cleanup afterwards.
fn run_list(
    w: &mut World,
    get: &dyn Fn(&mut World) -> Option<&mut SubList>,
    f: &mut dyn FnMut(&mut World, Sub),
) {
    let Some(l) = get(w) else { return };
    l.executing += 1;
    let count = l.list.len();
    for i in 0..count {
        let Some(l) = get(w) else { break };
        match l.list.get(i).copied() {
            Some(Some(s)) => f(w, s),
            Some(None) => {}
            None => {
                // A nested raise compacted the list under this loop (`Cleanup` runs in
                // every frame's `finally`, even while an outer frame still iterates).
                // `EventBus.RaiseEvent` logs and stops; a rulebook list's `List[i]`
                // throws, which aborts the whole rulebook step (not modelled: reported).
                w.coverage.unsupported("subscriber list compacted during dispatch");
                break;
            }
        }
    }
    if let Some(l) = get(w) {
        l.executing -= 1;
        // `Executing.Release(); Cleanup();` in each frame's `finally`.
        l.cleanup();
    }
}

/// `EventBus.RaiseEvent<iface>(…)` to global subscribers.
pub fn raise_global(w: &mut World, iface: &'static str, _about: Option<EntityIdx>) {
    raise_global_call(w, iface, "", None);
}

/// `EventBus.RaiseEvent<iface>(h => h.method(unit))`: the method and unit argument are
/// visible to handlers in [`EventBus::call`].
pub fn raise_global_call(
    w: &mut World,
    iface: &'static str,
    method: &'static str,
    unit: Option<EntityIdx>,
) {
    raise_global_fact(w, iface, method, unit, None);
}

/// `EventBus.RaiseEvent<iface>(h => h.method(unit, fact))`: as [`raise_global_call`] with a
/// fact argument, read through [`EventBus::call_fact`].
pub fn raise_global_fact(
    w: &mut World,
    iface: &'static str,
    method: &'static str,
    unit: Option<EntityIdx>,
    fact: Option<FactIdx>,
) {
    w.bus.call.push((method, unit));
    w.bus.call_fact.push(fact);
    run_list(w, &hr(move |w: &mut World| w.bus.global.get_mut(iface)), &mut |w, s| {
        deliver_event(w, s, iface)
    });
    w.bus.call_fact.pop();
    w.bus.call.pop();
}

/// `EventBus.RaiseEvent((iface h) => h.method(command))` to global subscribers, the command
/// visible to handlers in [`EventBus::command`].
pub fn raise_global_command(
    w: &mut World,
    iface: &'static str,
    method: &'static str,
    cmd: CommandArg,
) {
    w.bus.command.push(cmd);
    raise_global_call(w, iface, method, None);
    w.bus.command.pop();
}

/// `EventBus.RaiseEvent<iface>(unit, …)` to a unit's subscribers.
pub fn raise_unit(w: &mut World, unit: EntityIdx, iface: &'static str) {
    run_list(
        w,
        &hr(move |w: &mut World| w.bus.unit.get_mut(&unit).and_then(|m| m.get_mut(iface))),
        &mut |w, s| deliver_event(w, s, iface),
    );
}

/// `EventBus.RaiseEvent(unit, h => h.method(arg_unit, fact))` to a unit's subscribers, the
/// method and arguments visible through [`EventBus::call`] and [`EventBus::call_fact`].
pub fn raise_unit_fact(
    w: &mut World,
    unit: EntityIdx,
    iface: &'static str,
    method: &'static str,
    arg_unit: Option<EntityIdx>,
    fact: Option<FactIdx>,
) {
    w.bus.call.push((method, arg_unit));
    w.bus.call_fact.push(fact);
    raise_unit(w, unit, iface);
    w.bus.call_fact.pop();
    w.bus.call.pop();
}

/// The delivery in the handler log (the replay's one-to-one check with the recording's `h`
/// records), if one is kept: the four steps whose handlers the oracle records.
fn log_delivery(w: &mut World, s: Sub, rule: &dyn RuleEvent, step: RuleStep) {
    use super::{HandlerKind, HandlerRun};
    if w.rulebook.handler_log.is_none() {
        return;
    }
    let phase = match step {
        RuleStep::BeforeHook => "mod_before",
        RuleStep::AboutToTrigger => "about",
        RuleStep::AfterHook => "mod_after",
        RuleStep::DidTrigger => "did",
        RuleStep::AboutToTriggerHook | RuleStep::DidTriggerHook => return,
    };
    let kind = match s {
        Sub::Component(c) => {
            let f = w.fact(c.fact);
            let Some(ty) = w.component(c).ty() else { return };
            HandlerKind::Component {
                class: wr_content::schema::ty(ty).short.to_owned(),
                name: w.component(c).name.clone(),
                fact: f.unique_id.clone(),
                blueprint: f.blueprint.map(|g| g.to_string()),
                owner: f.owner.map(|e| w.entity(e).unique_id.clone()),
            }
        }
        Sub::Fact(f) => HandlerKind::Fact(w.fact(f).clr),
        // (A part or an entity the sim subscribes is a singleton by the recording's records: its
        // class is checked against the game's code, not compared with a delivery.)
        Sub::Part(..) | Sub::Entity(_) | Sub::Runtime(_) | Sub::External(_) => return,
    };
    let run = HandlerRun { phase, rule: rule.chain()[0].to_owned(), kind };
    if let Some(log) = &mut w.rulebook.handler_log {
        log.push(run);
    }
}

fn deliver_rule(w: &mut World, s: Sub, rule: &mut dyn RuleEvent, step: RuleStep) {
    let was = w.rulebook.pending_overflow.is_some();
    deliver_rule_inner(w, s, rule, step);
    catch_overflow(w, was);
}

fn deliver_rule_inner(w: &mut World, s: Sub, rule: &mut dyn RuleEvent, step: RuleStep) {
    log_delivery(w, s, rule, step);
    match s {
        Sub::Component(c) => {
            let Some(ty) = w.component(c).ty() else { return };
            let name = wr_content::schema::ty(ty).short;
            let reg = w.registry.clone();
            match reg.component(name) {
                Some(l)
                    if l.handlers().iter().any(|h| {
                        h.ends_with(&format!("<{}>", rule.chain()[0]))
                            || rule.chain().iter().any(|t| h.ends_with(&format!("<{t}>")))
                    }) =>
                {
                    w.coverage.executed(format!("{name}.{}", rule.chain()[0]));
                    l.rule(w, c, rule, matches!(step, RuleStep::AboutToTrigger));
                }
                _ => w.coverage.unsupported(format!("{name}: {} ({step:?})", rule.chain()[0])),
            }
        }
        Sub::Runtime(_) => {}
        Sub::Fact(f) => {
            if w.fact(f).clr == "ActivatableAbility"
                && crate::unit_facts::activatable_rule(w, f, rule, step)
            {
                w.coverage.executed(format!("ActivatableAbility.{}", rule.chain()[0]));
            } else {
                w.coverage.unsupported(format!("{}: {}", w.fact(f).clr, rule.chain()[0]));
            }
        }
        Sub::Entity(e) => {
            w.coverage.unsupported(format!("{}: {}", w.entity(e).clr, rule.chain()[0]))
        }
        Sub::Part(_, clr) => w.coverage.unsupported(format!("{clr}: {}", rule.chain()[0])),
        Sub::External(id) => {
            if let Some(h) = w.bus.external.get(&id).cloned() {
                h.rule(w, rule, step);
            }
        }
    }
}

/// `IBeforeRulebookEventTriggerHandler<T>` / `IAfterRulebookEventTriggerHandler<T>`:
/// global `EventBus` subscribers of those interfaces for the rule's exact type.
pub fn raise_rule_hooks(w: &mut World, rule: &mut dyn RuleEvent, which: RuleHook) {
    let t = rule.chain()[0];
    let iface = match which {
        RuleHook::Before => format!("IBeforeRulebookEventTriggerHandler<{t}>"),
        RuleHook::After => format!("IAfterRulebookEventTriggerHandler<{t}>"),
    };
    let step = match which {
        RuleHook::Before => RuleStep::BeforeHook,
        RuleHook::After => RuleStep::AfterHook,
    };
    let key = iface.clone();
    run_list(w, &hr(|w: &mut World| w.bus.global.get_mut(&key)), &mut |w, s| {
        deliver_rule(w, s, rule, step)
    });
}

/// One manager's `OnEventAboutToTrigger` / `OnEventDidTrigger`: the rule's type chain,
/// each type's list in order.
fn manager(w: &mut World, rule: &mut dyn RuleEvent, which: Mgr, step: RuleStep) {
    for &t in rule.chain() {
        let get = hr(move |w: &mut World| match which {
            Mgr::Global => w.bus.rule_global.get_mut(t),
            Mgr::Target(u) => w.bus.rule_target.get_mut(&u).and_then(|m| m.get_mut(t)),
            Mgr::Initiator(u) => w.bus.rule_initiator.get_mut(&u).and_then(|m| m.get_mut(t)),
        });
        run_list(w, &get, &mut |w, s| deliver_rule(w, s, rule, step));
    }
}

#[derive(Clone, Copy)]
enum Mgr {
    Global,
    Target(EntityIdx),
    Initiator(EntityIdx),
}

/// `RulebookEventBus.OnEventAboutToTrigger(evt)`.
pub fn rulebook_about_to_trigger(w: &mut World, rule: &mut dyn RuleEvent) {
    for s in w.bus.about_to_hooks.clone() {
        deliver_rule(w, s, rule, RuleStep::AboutToTriggerHook);
    }
    manager(w, rule, Mgr::Global, RuleStep::AboutToTrigger);
    if let Some(t) = rule.target() {
        manager(w, rule, Mgr::Target(t), RuleStep::AboutToTrigger);
    }
    let i = rule.initiator();
    manager(w, rule, Mgr::Initiator(i), RuleStep::AboutToTrigger);
}

/// `RulebookEventBus.OnEventDidTrigger(evt)`.
pub fn rulebook_did_trigger(w: &mut World, rule: &mut dyn RuleEvent) {
    // (The engagement controller's handlers, when it's modelled: a global subscriber whose only
    // effect is its provoke queue, so its place among the global subscribers doesn't show.)
    crate::combat::engage::rule_did_trigger(w, rule);
    // (The impatience controller's and `OffensiveActionsController`'s, when modelled: they queue
    // their effects, applied in order as their records come.)
    crate::combat::impatience::rule_did_trigger(w, rule);
    manager(w, rule, Mgr::Global, RuleStep::DidTrigger);
    if let Some(t) = rule.target() {
        manager(w, rule, Mgr::Target(t), RuleStep::DidTrigger);
    }
    let i = rule.initiator();
    manager(w, rule, Mgr::Initiator(i), RuleStep::DidTrigger);
    for s in w.bus.did_hooks.clone() {
        deliver_rule(w, s, rule, RuleStep::DidTriggerHook);
    }
}
