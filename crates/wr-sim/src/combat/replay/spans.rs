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
//!   resolve's, the flat-footed check's: [`Driver::env_reads`]); any other is reported. A resolve
//!   nested in a phase-driven attack has its own, whose positions must equal the attack roll's.
//! - `invalid`: the oracle's own statement that the game's replay left its recording. The sim is
//!   checked against the game's actual trace, so it is not the sim's matter: accepted everywhere
//!   (counted in [`SpanCounts::oracle_invalid`] inside spans).
//!
//! The rest of a span is the rule's own: its steps (`about`, `trig`, `trig_end`, `did`), handlers
//! (`h`, `h_end`) and nested rules, whose structure is `Life`'s (shared with `span_ok`), and its
//! draws and mutations, compared in the T0 streams (a `mut` the projection drops is reported; a
//! `rule_bonus` is projected only to a rule open at the mutation). The oracle never writes raw `result` or `bonuses`
//! records (the projection derives them from `rule` and `rule_end`), so a stray one is reported,
//! like any other observation or record of another kind: the replay doesn't take what it can't
//! name. A span the commands pull (`attack_rule`) is checked record by record by `span_ok` instead.

use std::collections::{HashMap, HashSet};

use super::commands::provoke_rule;
use super::life::Life;
use super::{Driver, Rv};
use crate::rulebook::{HandlerKind, HandlerRun};
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
    /// The recorded handlers compared one to one with the sim's deliveries.
    pub handlers: usize,
    /// Records reported: of another family or kind, or one its checker rejects.
    pub reported: usize,
}

/// Whether the compared T0 stream keeps a mutation: what `project::project_records` projects (damage,
/// experience, facts attached and detached, and a bonus added to a rule it tracks, which it buffers
/// and emits at that rule's `rule_end`: so the rule must be open on `open`'s stack at the mutation),
/// less what `oracle_stream` then drops (the tutorial's facts: [`super::stream_drops_fact`]).
/// The stat mutations (`project::is_stat_mut`) are in the stream too, but a span doesn't accept
/// one: the sweep's captures have none inside a root span, and the sim writes none there, so one
/// would be a record nothing explains (`docs/poc5/stats.md`).
pub(super) fn projected(open: &Open, r: &Rv) -> bool {
    match r["what"].as_str() {
        Some("damage" | "xp") => true,
        Some("fact_attach" | "fact_detach") => !super::stream_drops_fact(r["class"].as_str()),
        Some("rule_bonus") => {
            r["target_rule"].as_i64().is_some_and(|t| t > 0 && open.life.is_open(t))
        }
        _ => false,
    }
}

/// The recording's handlers that the sim's bus dispatches, as the sim's log has them: each with
/// the rule it was delivered to, identified by class, initiator and ordinal among that class and
/// initiator's rules (`skip(i)`: records out of the comparison).
pub(super) fn recorded_handlers(records: &[Rv], skip: impl Fn(usize) -> bool) -> Vec<HandlerRun> {
    let mut rules: HashMap<i64, (String, (String, u64))> = HashMap::new();
    let mut counts: HashMap<(String, String), u64> = HashMap::new();
    let mut recorded = Vec::new();
    for (i, r) in records.iter().enumerate() {
        match r["k"].as_str() {
            Some("rule") if !skip(i) => {
                if let Some(id) = r["id"].as_i64() {
                    let class = r["class"].as_str().unwrap_or("");
                    let short = class.rsplit(['.', '+']).next().unwrap_or("").to_owned();
                    // The initiator's id: a unit's, or an item's or other entity's.
                    let initiator = r["initiator"]
                        .as_object()
                        .and_then(|o| o.values().find_map(|v| v.as_str()))
                        .unwrap_or("")
                        .to_owned();
                    let n = counts.entry((short.clone(), initiator.clone())).or_default();
                    rules.insert(id, (short, (initiator, *n)));
                    *n += 1;
                }
            }
            Some("h") if dispatched(r) && !skip(i) => {
                let phase = match r["phase"].as_str() {
                    Some("mod_before") => "mod_before",
                    Some("about") => "about",
                    Some("mod_after") => "mod_after",
                    Some("did") => "did",
                    _ => continue,
                };
                let kind = if r.get("component").is_some() {
                    let owner = &r["owner"];
                    let owner = match (owner["$unit"].as_str(), owner["$item"].as_str()) {
                        (Some(id), _) => format!("unit:{id}"),
                        (_, Some(id)) => {
                            format!("item:{id}:{}", owner["blueprint"].as_str().unwrap_or_default())
                        }
                        _ => String::new(),
                    };
                    HandlerKind::Component {
                        class: r["class"].as_str().unwrap_or("").to_owned(),
                        name: r["component"].as_str().unwrap_or("").to_owned(),
                        fact: r["fact"]["$fact"].as_str().unwrap_or("").to_owned(),
                        blueprint: r["blueprint"].as_str().map(str::to_owned),
                        fact_blueprint: r["fact"]["blueprint"].as_str().map(str::to_owned),
                        owner,
                    }
                } else {
                    HandlerKind::Fact("ActivatableAbility")
                };
                let (rule, instance) =
                    r["rule"].as_i64().and_then(|id| rules.get(&id)).cloned().unwrap_or_default();
                recorded.push(HandlerRun { phase, rule, instance, kind });
            }
            _ => {}
        }
    }
    recorded
}

/// Whether the recording's `h` record is of a handler the sim's bus dispatches: a fact's
/// component, or an `ActivatableAbility` (a fact that is its own handler).
fn dispatched(r: &Rv) -> bool {
    r.get("component").is_some()
        || r["class"].as_str().is_some_and(|c| c.rsplit('.').next() == Some("ActivatableAbility"))
}

/// A component handler's record shape, world-free: its component, blueprint, fact and owner, the
/// component named `$<type>$<name>` of the handler's class, the about and did phases.
fn component_shape_ok(r: &Rv) -> bool {
    if r.get("component").is_none() {
        return true;
    }
    let class = r["class"].as_str().unwrap_or("").rsplit(['.', '+']).next().unwrap_or("");
    r["component"]
        .as_str()
        .and_then(|c| c.strip_prefix('$'))
        .and_then(|c| c.split_once('$'))
        .is_some_and(|(t, _)| t == class)
        && r["blueprint"].is_string()
        && r["fact"]["$fact"].is_string()
        && (r["owner"]["$unit"].is_string() || r["owner"]["$item"].is_string())
        && matches!(r["phase"].as_str(), Some("about" | "did" | "mod_before" | "mod_after"))
}

/// What `span_records` knows of the span so far: the structure of its rules and handlers
/// (`Life`, shared with `span_ok`), the rules' classes and initiators, the provokes made.
pub(super) struct Open {
    /// The root's frame.
    pub(super) t: i64,
    pub(super) life: Life,
    /// Its rules by id: short class and initiator.
    pub(super) rules: HashMap<i64, (String, Rv)>,
    /// The rules that made their provoke.
    pub(super) provoked: HashSet<i64>,
    /// The environment records each open rule has written.
    pub(super) envs: HashMap<i64, usize>,
    /// Each rule's full class, initiator and target, and the simulation draws it has made.
    pub(super) shapes: HashMap<i64, (String, Rv, Rv)>,
    pub(super) draws: HashMap<i64, i64>,
}

impl Open {
    /// The state at the root's record.
    pub(super) fn new(root: &Rv) -> Self {
        let mut open = Self {
            t: root["t"].as_i64().unwrap_or(-1),
            life: Life::new(root["id"].as_i64().unwrap_or(0)),
            rules: HashMap::new(),
            provoked: HashSet::new(),
            envs: HashMap::new(),
            shapes: HashMap::new(),
            draws: HashMap::new(),
        };
        open.note(root);
        open
    }

    fn note(&mut self, r: &Rv) {
        let class = r["class"].as_str().unwrap_or("").rsplit(['.', '+']).next().unwrap_or("");
        if let Some(id) = r["id"].as_i64() {
            self.rules.insert(id, (class.to_owned(), r["initiator"].clone()));
            self.shapes.insert(
                id,
                (
                    r["class"].as_str().unwrap_or("").to_owned(),
                    r["initiator"].clone(),
                    r["target"].clone(),
                ),
            );
        }
    }

    /// The next record of the span (a rule's push, step or end, a handler's open or close);
    /// false for one out of its place in the structure `Life` describes.
    pub(super) fn feed(&mut self, r: &Rv) -> bool {
        let id = r["id"].as_i64();
        let step = |life: &mut Life, want: u8, next: u8| {
            let ok = life.can_step(id, want);
            if ok {
                life.advance(next);
            }
            ok
        };
        match r["k"].as_str() {
            Some("rule") => {
                let ok = self.life.can_push(
                    id,
                    r["parent"].as_i64(),
                    r["depth"].as_u64(),
                    r["handler"].as_i64(),
                    r["context_parent"] == r["parent"],
                );
                if let Some(id) = id {
                    self.note(r);
                    self.life.push(id);
                }
                ok
            }
            Some("about") => step(&mut self.life, 0, 1),
            Some("trig") => step(&mut self.life, 1, 2),
            Some("trig_end") => step(&mut self.life, 2, 3),
            Some("did") => step(&mut self.life, 3, 4),
            Some("rule_end") => {
                let ok = step(&mut self.life, 4, 5);
                if id == Some(self.life.rule()) {
                    self.life.end_rule();
                }
                ok
            }
            Some("h") => {
                let phase = r["phase"].as_str().unwrap_or("");
                let ok = self.life.can_open(id, r["rule"].as_i64(), phase);
                if let (true, Some(id)) = (ok, id) {
                    self.life.open(id, r["class"].as_str().unwrap_or(""), phase);
                }
                ok
            }
            Some("h_end") => {
                let ok = self.life.can_close(id, r["rule"].as_i64());
                self.life.close();
                ok
            }
            _ => true,
        }
    }
}

/// A root rule's own stack fields: depth 0, no parent, no handler open, a context (an event in
/// context when it was triggered, or none), its reason at the push (null, or an ability's: a spell's
/// rules carry theirs), an id.
pub(super) fn root_ok(r: &Rv) -> bool {
    r["depth"] == 0
        && r["parent"] == 0
        && r["handler"] == 0
        && r["context_parent"].as_i64().is_some_and(|c| c >= 0)
        && r.get("reason_at_push").is_some()
        && r["id"].as_i64().is_some_and(|i| i > 0)
}

impl Open {
    /// A record of the innermost rule, in the open handler (its `rule` and `h`).
    fn attributed(&self, r: &Rv) -> bool {
        self.life.attributed(r["rule"].as_i64(), r["h"].as_i64())
    }

    /// A draw: attributed, and a simulation draw in the rule's `OnTrigger` or one of its handlers.
    pub(super) fn draw_placed(&self, r: &Rv) -> bool {
        self.attributed(r) && (r["c"] != "sim" || self.life.stage() == 2 || self.life.own_handler())
    }

    /// A draw's context: a simulation draw carries its rule's class, units (stable ids) and
    /// ordinal, which advances; a presentation draw carries none (`Rng`, `span_ok`'s check).
    pub(super) fn draw_ctx_ok(&mut self, w: &World, r: &Rv) -> bool {
        let rule = self.life.rule();
        if r["c"] != "sim" {
            return r.get("ctx").is_none();
        }
        let n = self.draws.entry(rule).or_default();
        let ordinal = *n;
        *n += 1;
        let want = self.shapes.get(&rule).and_then(|(class, initiator, target)| {
            Some(format!(
                "{class}|{}|{}|{ordinal}",
                super::commands::stable_id(w, &initiator.to_json())?,
                super::commands::stable_id(w, &target.to_json())?,
            ))
        });
        want.is_some() && r["ctx"].as_str() == want.as_deref()
    }

    /// A mutation: attributed; damage after `did` (`OnDidTrigger`) outside its handlers, a rule
    /// bonus in the rule's `OnTrigger` or one of its handlers.
    pub(super) fn mut_placed(&self, r: &Rv) -> bool {
        self.attributed(r)
            && match r["what"].as_str() {
                Some("damage") => self.life.stage() == 4 && !self.life.own_handler(),
                Some("rule_bonus") => self.life.stage() == 2 || self.life.own_handler(),
                _ => true,
            }
    }

    /// A rule's end: an attack roll's and a resolve's environment was written once (`span_ok`'s
    /// count), so a nested resolve (a weapon without a projectile resolves at once) with its
    /// environment removed is not skipped, whatever the sim reuses.
    pub(super) fn envs_ok(&self, rule_end: &Rv) -> bool {
        let Some(id) = rule_end["id"].as_i64() else { return false };
        match self.rules.get(&id).map(|(class, _)| class.as_str()) {
            Some("RuleAttackRoll" | "RuleAttackWithWeaponResolve") => {
                self.envs.get(&id).copied().unwrap_or(0) == 1
            }
            _ => true,
        }
    }

    /// An environment record's fields as `span_ok` types them (`env_record_ok`: the rule's units,
    /// every field of its kind, `EngagedBy` the sim's units in order), and a flat-footed check's
    /// only at the root (inside an attack it's nested: `FlatFootedPrefix` writes it when one
    /// rule is open).
    pub(super) fn env_valid(&self, w: &World, r: &Rv) -> bool {
        let rule = self.life.rule();
        self.shapes.get(&rule).is_some_and(|(class, initiator, target)| {
            (r["rule"] != "RuleCheckTargetFlatFooted" || self.life.depth() == 1)
                && super::commands::env_record_ok(
                    w,
                    &r.to_json(),
                    class,
                    &initiator.to_json(),
                    &target.to_json(),
                )
        })
    }

    /// An environment: of the innermost rule (by its class), in its `OnTrigger` outside its handlers.
    pub(super) fn env_placed(&self, r: &Rv) -> bool {
        self.rules.get(&self.life.rule()).is_some_and(|(class, _)| r["rule"] == class.as_str())
            && self.life.stage() == 2
            && !self.life.own_handler()
    }
}

/// A provoke (`r`, whose caller names `rule`) placed like `span_ok` places a command's: in the
/// root's frame, in the engagement controller's did handler of that rule, of the rule's
/// initiator, once per rule, while the rule is the innermost open one and between `trig_end` and
/// `did`.
pub(super) fn provoke_placed(open: &mut Open, r: &Rv, rule: Option<&str>) -> bool {
    let Some(h) = open.life.handler() else { return false };
    let id = h.rule;
    let placed = r["t"].as_i64() == Some(open.t)
        && h.class == ENGAGEMENT
        && h.phase == "did"
        && open.life.rule() == id
        && open.life.stage() == 3
        && open.rules.get(&id).is_some_and(|(rc, initiator)| {
            Some(rc.as_str()) == rule && r["u"] == initiator["$unit"]
        });
    placed && open.provoked.insert(id)
}

impl Driver<'_> {
    /// The handlers of the root rule at `start` (to its `rule_end` at `end`). A fact's component
    /// and `ActivatableAbility`, which the sim's bus dispatches, are compared one to one with its
    /// deliveries at the run's end ([`Driver::handlers_finish`]); here only their shape. Any
    /// other (a global singleton: the sim has none) is a class the game's code has handle the
    /// rule ([`super::handler_table`], via `span_ok`'s `handler_identity_ok`).
    pub(super) fn span_handlers(&mut self, w: &mut World, start: usize, end: usize) {
        let trace = self.trace;
        let mut rules: HashMap<i64, (String, Rv, Rv)> = HashMap::new();
        for r in &trace.records[start..=end] {
            match r["k"].as_str() {
                Some("rule") => {
                    if let Some(id) = r["id"].as_i64() {
                        let class = r["class"].as_str().unwrap_or("").to_owned();
                        rules.insert(id, (class, r["initiator"].clone(), r["target"].clone()));
                    }
                }
                Some("h") => {
                    let ok = if dispatched(r) {
                        component_shape_ok(r)
                    } else {
                        r["rule"].as_i64().and_then(|id| rules.get(&id)).is_some_and(
                            |(class, initiator, target)| {
                                super::commands::handler_identity_ok(
                                    w,
                                    &r.to_json(),
                                    class,
                                    &initiator.to_json(),
                                    &target.to_json(),
                                    &super::handler_table::handles,
                                )
                            },
                        )
                    };
                    if !ok {
                        self.span_reported(
                            w,
                            "a h record".into(),
                            "a handler the rule's subscribers don't have (class, fact, component or owner)",
                        );
                    }
                }
                _ => {}
            }
        }
    }

    /// The run's end: the handlers the recording dispatched to a fact's component or an
    /// `ActivatableAbility` are the sim's deliveries (`Rulebook::handler_log`, from the bus's
    /// subscription lists and the rule's type chain), one to one, in order: class, component,
    /// fact, blueprint and owner, phase and rule.
    pub(super) fn handlers_finish(&mut self, w: &mut World) {
        let Some(sim) = w.rulebook.handler_log.take() else { return };
        w.rulebook.handler_instances.clear();
        w.rulebook.handler_counts.clear();
        let trace = self.trace;
        // (An echoed root, a presentation root without a call site in an old recording, is the
        // recording's by definition, not fired, and reported as such.)
        let recorded = recorded_handlers(&trace.records, |i| {
            self.echoed.iter().any(|(s, e)| *s <= i && i <= *e)
        });
        self.out.spans.handlers = recorded.len();
        if recorded != sim {
            let at = recorded.iter().zip(&sim).position(|(a, b)| a != b);
            let at = at.unwrap_or(recorded.len().min(sim.len()));
            self.out.spans.reported += 1;
            w.coverage.unsupported(format!(
                "driven replay: handlers: the dispatched handlers differ from the sim's at #{at} (recorded {:?}, sim {:?}; {} recorded, {} sim)",
                recorded.get(at),
                sim.get(at),
                recorded.len(),
                sim.len()
            ));
        }
    }

    /// The records of the root rule at `start`, up to its `rule_end` at `end`, after the sim
    /// fired it (or didn't).
    pub(super) fn span_records(&mut self, w: &mut World, start: usize, end: usize) {
        let trace = self.trace;
        let root = &trace.records[start];
        let mut open = Open::new(root);
        // The root's own stack fields (what `attack_root` requires of a command's, but for its
        // context: a root may be triggered while another event is in context, a cast's).
        if !root_ok(root) {
            self.span_reported(
                w,
                "a root rule record".into(),
                "a root whose stack fields (depth, parent, handler, context, reason, id) aren't a root's",
            );
        }
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
                    if r["k"] == "rule_end" && !open.envs_ok(r) {
                        self.span_reported(
                            w,
                            "a rule_end record".into(),
                            "an attack roll or resolve without exactly one environment",
                        );
                    }
                    if !open.feed(r) {
                        self.span_reported(
                            w,
                            format!("a {} record", r["k"].as_str().unwrap_or("?")),
                            "a record out of its place in the rule's life",
                        );
                    }
                }
                // A draw is compared in the T0 stream (a presentation draw is none), and is the
                // innermost rule's, in the open handler: a simulation draw in its computation
                // (`OnTrigger`) or a handler of it (`span_ok`'s attribution).
                Some("draw") => {
                    if !open.draw_placed(r) {
                        self.span_reported(
                            w,
                            "a draw record".into(),
                            "a draw not of the innermost rule, its handler or its stage",
                        );
                    }
                    // Its payload and site, a presentation draw's too (`span_ok`'s `draw_ok`), its
                    // context, and the tape's status in a replay's trace (forced: simulation
                    // draws only).
                    if !super::commands::draw_ok(&r.to_json(), &trace.sites) {
                        self.span_reported(
                            w,
                            "a draw record".into(),
                            "a draw with a payload or site the recording's table doesn't give",
                        );
                    }
                    let sim = r["c"] == "sim";
                    if !open.draw_ctx_ok(w, r) || r.get("forced").is_some() != (self.replay_mode && sim)
                    {
                        self.span_reported(
                            w,
                            "a draw record".into(),
                            "a draw whose context or tape status isn't its rule's",
                        );
                    }
                }
                // So is a mutation of the kinds the projection keeps (the others it drops), made
                // by the innermost rule in the open handler.
                Some("mut") if projected(&open, r) => {
                    if !open.mut_placed(r) {
                        self.span_reported(
                            w,
                            format!("a {} mutation", r["what"].as_str().unwrap_or("?")),
                            "a mutation not of the innermost rule, its handler or its stage",
                        );
                    }
                }
                // The environment the root's code looked up, written by the rule it names at its
                // `OnTrigger` (`span_ok`'s placing), outside its handlers.
                Some("env") => {
                    let read = self.env_reads.iter().any(|(rule, a, t)| {
                        super::find_env(trace, start, end, rule, a, t) == Some(k)
                    });
                    if open.env_placed(r) {
                        *open.envs.entry(open.life.rule()).or_default() += 1;
                    }
                    if !read || !open.env_placed(r) || !open.env_valid(w, r) {
                        self.span_reported(
                            w,
                            "an env record".into(),
                            "an environment the root's code didn't read, or one out of its rule's computation",
                        );
                    }
                }
                Some("mut") => self.span_reported(
                    w,
                    format!("a {} mutation", r["what"].as_str().unwrap_or("?")),
                    "a mutation the compared T0 stream drops (or a tutorial fact), so nothing compares it",
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
            // A collider write or a sync inside the span: queued for the frame's vision phase,
            // where the sim's own writes of the rule stand (`bounds_rec.rs`).
            Some("bounds_set" | "phys_sync") if self.bounds_rec_on() => self.bounds_stream(w, r),
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
