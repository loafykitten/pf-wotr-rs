//! The driven replay's side of the impatience controller ([`crate::combat::impatience`]):
//! WrathOracle's observer `impatience`. `imp_init` gives the window's initial state (one block
//! right after `tgt_init`'s, over the same units in order). An entry record (`uc_imp`: the
//! controller's tick, or `TurnController.Prepare`'s `UpdatePriorityTarget`) runs the sim's
//! controller, which pulls its leaves, nested entries, draws and checks (`imp_s` `upd`,
//! `setup`) in order. A handler's `imp_s` (`start`, `bother`, `dmg`, `death`) applies the sim's
//! queued operation and checks the unit's state; inside a rule's span they're taken after the
//! sim fires the rule.

use std::collections::VecDeque;

use serde_json::Value as J;

use super::Driver;
use super::commands::{describe, id, is_unit, obs, opt_str, phase_of};
use super::engage::{fields, float};
use crate::combat::commands::{Desync, R};
use crate::combat::impatience::{self as imp, ImpInputs, Op, UnitImpatience};
use crate::rng::CallSite;
use crate::world::{EntityIdx, World};

/// A unit list: `None` when it isn't one (or names an entity the sim doesn't have as a unit).
fn units(w: &World, v: &J) -> Option<Vec<Option<EntityIdx>>> {
    v.as_array()?
        .iter()
        .map(|x| match x {
            J::Null => Some(None),
            J::String(s) if is_unit(w, s) => Some(w.by_id.get(s.as_str()).copied()),
            _ => None,
        })
        .collect()
}

fn unit_or_null(w: &World, v: &J) -> Option<Option<EntityIdx>> {
    match v {
        J::Null => Some(None),
        J::String(s) if is_unit(w, s) => Some(w.by_id.get(s.as_str()).copied()),
        _ => None,
    }
}

fn unit_of(w: &World, v: &J) -> Option<EntityIdx> {
    unit_or_null(w, v).flatten()
}

/// The state fields every `imp_s` and `imp_init` carries.
const STATE: [&str; 11] = [
    "u",
    "hard",
    "hard_slots",
    "dis",
    "dis_slots",
    "bother",
    "bother_slots",
    "cawe",
    "lost",
    "ipt",
    "cnt",
];

/// A recorded state: `None` when it can't be read.
fn parse_state(w: &World, r: &J) -> Option<UnitImpatience> {
    let set = |k: &str| {
        let mut s = crate::csharp::hashset::CsHashSet::default();
        for u in units(w, &r[k])? {
            if !s.add(u) {
                return None;
            }
        }
        // (The slots aren't rebuilt: a fresh set has as many as entries, compared apart.)
        Some(s)
    };
    let cawe: Option<Vec<u32>> = r["cawe"].as_array()?.iter().map(id).collect();
    Some(UnitImpatience {
        known: true,
        hard: set("hard")?,
        disliked: set("dis")?,
        bother: set("bother")?,
        attacks: cawe?,
        last_target: unit_or_null(w, &r["lost"])?,
        priority: unit_or_null(w, &r["ipt"])?,
        inefficient: r["cnt"].as_i64().and_then(|c| i32::try_from(c).ok())?,
    })
}

/// Whether recorded state `r` is the sim's `s` (each set in its order and with its used slots).
fn state_equal(w: &World, r: &J, s: &UnitImpatience) -> bool {
    let slots = |k: &str, n: usize| r[k].as_u64() == Some(n as u64);
    let list = |k: &str, set: &crate::csharp::hashset::CsHashSet<EntityIdx>| {
        units(w, &r[k]).is_some_and(|v| v == set.iter().collect::<Vec<_>>())
    };
    list("hard", &s.hard)
        && slots("hard_slots", s.hard.slots())
        && list("dis", &s.disliked)
        && slots("dis_slots", s.disliked.slots())
        && list("bother", &s.bother)
        && slots("bother_slots", s.bother.slots())
        && r["cawe"].as_array().is_some_and(|a| {
            a.len() == s.attacks.len() && a.iter().zip(&s.attacks).all(|(x, c)| id(x) == Some(*c))
        })
        && unit_or_null(w, &r["lost"]) == Some(s.last_target)
        && unit_or_null(w, &r["ipt"]) == Some(s.priority)
        && r["cnt"].as_i64() == Some(i64::from(s.inefficient))
}

/// An `imp_s`'s fields: the state's and `at`'s own.
fn state_fields(r: &J, extra: &[&str]) -> bool {
    let mut all: Vec<&str> = vec!["at"];
    all.extend(STATE);
    all.extend(extra);
    fields(r, &all)
}

/// The impatience sites' draws (`Range(0, 100)`, `Range(0f, sum)`).
fn site_of(key: &str) -> Option<CallSite> {
    [imp::SWITCH_DRAW, imp::SETUP_DRAW].into_iter().find(|s| s.0 == key)
}

impl Driver<'_> {
    /// `imp_init` (record `i`): a unit's state at the window's first tick, right after `tgt_init`'s
    /// block over the same units in the same order. Adopted only fresh (empty, no slot used).
    pub(super) fn imp_init(&mut self, w: &mut World, i: usize) {
        let r = &self.trace.records[i];
        if !self.imp {
            w.coverage
                .unsupported("driven replay: an imp_init record without the impatience observer");
            return;
        }
        let u = unit_of(w, &r["u"]);
        let (Some(u), true, Some(s)) = (u, fields(r, &STATE), parse_state(w, r)) else {
            w.coverage.unsupported("driven replay: an imp_init record it can't read");
            return;
        };
        let prev = i.checked_sub(1).map(|j| &self.trace.records[j]);
        let n = self.imp_init_n;
        self.imp_init_n += 1;
        let before = if n == 0 { "tgt_init" } else { "imp_init" };
        if self.tgt_init_closed
            || !prev.is_some_and(|p| obs(p, before).is_some())
            || self.tgt_init_units.get(n) != Some(&u)
        {
            w.coverage.unsupported(
                "driven replay: an imp_init record out of its block (after tgt_init, in its units' order)",
            );
            return;
        }
        let fresh = s.hard.is_empty()
            && s.disliked.is_empty()
            && s.bother.is_empty()
            && s.attacks.is_empty()
            && s.last_target.is_none()
            && s.priority.is_none()
            && s.inefficient == 0
            && ["hard_slots", "dis_slots", "bother_slots"].iter().all(|k| r[*k] == 0);
        if !fresh {
            w.coverage.unsupported(
                "driven replay: impatience state at the window's start (its sets' layout isn't recorded)",
            );
            return;
        }
        let x = &mut w.unit_mut(u).expect("a unit").combat.imp;
        if x.known {
            w.coverage.unsupported("driven replay: a unit's impatience given twice");
            return;
        }
        *x = UnitImpatience { known: true, ..UnitImpatience::default() };
    }

    /// Applies the queue's silent operations at its head (a damage's list update, a death's
    /// removals, leaving combat's clear).
    fn imp_drain(&mut self, w: &mut World) -> R<()> {
        while let Some(op) = w.combat.impatience.queue.front().cloned() {
            if op.recorded() {
                break;
            }
            w.combat.impatience.queue.pop_front();
            imp::apply(w, &op, None)?;
        }
        Ok(())
    }

    /// An `imp_s` of a handler (`start`, `bother`, `dmg`, `death`) or an `imp_other` (record `i`):
    /// the sim's next queued operation, applied and checked.
    pub(super) fn imp_record(&mut self, w: &mut World, i: usize) {
        self.imp_record_in(w, i, None);
    }

    /// `imp_record`; `handler` when a rule's span holds it (a rule handler's records only there):
    /// whether that's the damage handler.
    fn imp_record_in(&mut self, w: &mut World, i: usize, handler: Option<bool>) {
        let in_span = handler.is_some();
        let r = &self.trace.records[i];
        if self.imp
            && !in_span
            && obs(r, "imp_s").is_some()
            && matches!(r["at"].as_str(), Some("bother" | "dmg"))
        {
            w.coverage.unsupported(format!(
                "driven replay: impatience: a rule handler's record outside its rule's span ({})",
                describe(r)
            ));
            w.combat.impatience.queue.clear();
            if let Some(u) = unit_of(w, &r["u"])
                && let Some(x) = w.unit_mut(u)
            {
                x.combat.imp.known = false;
            }
            return;
        }
        if !self.imp {
            w.coverage
                .unsupported("driven replay: an impatience record without the impatience observer");
            return;
        }
        if obs(r, "imp_other").is_some() {
            w.coverage.unsupported(format!(
                "driven replay: impatience: a heal or a summon's handler (not modelled): {}",
                r["fn"]
            ));
            return;
        }
        if self.uc_skipping {
            // After a desync the queue isn't the recording's: the state isn't established again.
            w.combat.impatience.queue.clear();
            if let Some(u) = unit_of(w, &r["u"])
                && let Some(x) = w.unit_mut(u)
            {
                x.combat.imp.known = false;
            }
            return;
        }
        // A death's records are one batch: once started, the next record is its next. Its first
        // comes where the notification is: after the mutation records the sim made before it.
        let batch = match w.combat.impatience.queue.iter().find(|op| op.recorded()) {
            Some(Op::DeathCheck { dead, .. }) => Some(*dead),
            _ => None,
        };
        // (Its silent operation before its first record, after any other silent one.)
        let death_muts = w.combat.impatience.queue.iter().take_while(|op| !op.recorded()).find_map(
            |op| match op {
                Op::Death { muts, .. } => Some(*muts),
                _ => None,
            },
        );
        if let Some(muts) = death_muts.as_ref()
            && !self.uc_skipping
        {
            let before = self.trace.records[..i]
                .iter()
                .filter(|x| {
                    x.as_object()
                        .is_some_and(|o| imp::compared_mutation(x["k"].as_str().unwrap_or(""), o))
                })
                .count();
            if before != *muts {
                w.coverage.unsupported(format!(
                    "driven replay: impatience: a death's records away from its notification ({before} mutations before them, the sim's {muts})"
                ));
                w.combat.impatience.queue.clear();
                return;
            }
        }
        if let Err(Desync(d)) = self.imp_match(w, r, handler) {
            w.coverage.unsupported(format!("driven replay: impatience: {d}"));
            w.combat.impatience.queue.clear();
            if let Some(u) = unit_of(w, &r["u"])
                && let Some(x) = w.unit_mut(u)
            {
                x.combat.imp.known = false;
            }
            return;
        }
        if let Some(dead) = batch
            && matches!(w.combat.impatience.queue.front(), Some(Op::DeathCheck { dead: d, .. }) if *d == dead)
        {
            let next = self.trace.records.get(i + 1);
            if !next.is_some_and(|x| obs(x, "imp_s").is_some() && x["at"] == "death") {
                w.coverage.unsupported(
                    "driven replay: impatience: a death's records not together (HandleUnitDeath's batch)",
                );
                w.combat.impatience.queue.clear();
            }
        }
    }

    /// A unit's `SetLifeState` goes on after its death's notification: none of that death's
    /// impatience records may still be due (`uc_x` record `r`).
    pub(super) fn imp_death_window(&mut self, w: &mut World, r: &J) {
        if !self.imp || self.uc_skipping {
            return;
        }
        let from_life = r["origin"].as_str().is_some_and(|o| {
            o.split(" < ").nth(1)
                == Some("Kingmaker.Controllers.Units.UnitLifeController.SetLifeState")
        });
        let u = unit_of(w, &r["u"]);
        let due = w.combat.impatience.queue.iter().any(|op| {
            matches!(op, Op::Death { dead, .. } | Op::DeathCheck { dead, .. } if Some(*dead) == u)
        });
        if from_life && due {
            w.coverage.unsupported(
                "driven replay: impatience: a death's records after its SetLifeState went on",
            );
            w.combat.impatience.queue.clear();
        }
    }

    fn imp_match(&mut self, w: &mut World, r: &J, handler: Option<bool>) -> R<()> {
        // (`HandleUnitDestroyed` calls `HandleUnitDeath`: the replay doesn't model destruction.)
        if r["fn"] == "HandleUnitDestroyed" {
            return Err(Desync("a destruction's handler (not modelled)".into()));
        }
        self.imp_drain(w)?;
        let Some(op) = w.combat.impatience.queue.pop_front() else {
            return Err(Desync(format!(
                "a handler's record the sim didn't make ({})",
                describe(r)
            )));
        };
        let u = unit_of(w, &r["u"]);
        let mismatch = || Desync(format!("the sim's {op:?}, the recording has {}", describe(r)));
        let at = r["at"].as_str().unwrap_or("");
        let mut added = None;
        let ok = match &op {
            Op::Start { u: x, c, .. } => {
                at == "start" && u == Some(*x) && state_fields(r, &["c"]) && id(&r["c"]) == Some(*c)
            }
            Op::DamageEnd { initiator } => {
                at == "dmg"
                    && u == Some(*initiator)
                    && state_fields(r, &["rule"])
                    && r["rule"].as_i64().is_some_and(|x| x > 0)
            }
            Op::Bother { u: x, b, cond, damage } => {
                let vis = match &r["vis"] {
                    J::Null => None,
                    J::Bool(v) => Some(*v),
                    _ => return Err(mismatch()),
                };
                // The visibility is read only when the unit is in combat and the other its enemy.
                // (In the handler that queued it.)
                let ok = at == "bother"
                    && handler == Some(*damage)
                    && u == Some(*x)
                    && state_fields(r, &["b", "added", "vis"])
                    && unit_of(w, &r["b"]) == Some(*b)
                    && vis.is_some() == *cond
                    && r["added"].is_boolean();
                if ok {
                    added = Some(imp::apply(w, &op, vis)?);
                }
                ok
            }
            Op::DeathCheck { dead, u: x } => {
                at == "death"
                    && u == Some(*x)
                    && state_fields(r, &["fn", "dead"])
                    && r["fn"] == "HandleUnitDeath"
                    && unit_of(w, &r["dead"]) == Some(*dead)
            }
            Op::DamageList { .. } | Op::Death { .. } | Op::Clear { .. } => false,
        };
        if !ok {
            return Err(mismatch());
        }
        if !matches!(op, Op::Bother { .. }) {
            imp::apply(w, &op, None)?;
        }
        if added.is_some_and(|a| r["added"] != a) {
            return Err(Desync(format!(
                "TryAddBotherUnit's Add: the sim's {added:?}, the recording's {}",
                r["added"]
            )));
        }
        let Some(u) = u else { return Err(mismatch()) };
        let s = imp::state(w, u)?.clone();
        if !state_equal(w, r, &s) {
            return Err(Desync(format!(
                "{}'s impatience after {at}: the sim's {s:?}, the recording's {}",
                w.entity(u).unique_id,
                describe(r)
            )));
        }
        self.out.imp_handlers += 1;
        Ok(())
    }

    /// The impatience records inside a rule's span (`start..=end`), after the sim fired it. Each
    /// must be of its frame and written inside a did handler of its rule: the impatience
    /// controller's for a `RuleDealDamage` (the damaged unit's bother unit, then the handler's end,
    /// the initiator's, with the rule's id), `OffensiveActionsController`'s for a `RuleAttackRoll`
    /// (the target's bother unit).
    pub(super) fn imp_span(&mut self, w: &mut World, start: usize, end: usize) {
        if !self.imp {
            let any = self.trace.records[start..=end].iter().any(|r| self.imp_family(r));
            if any {
                w.coverage.unsupported(
                    "driven replay: an impatience record without the impatience observer",
                );
            }
            if self.trace.records[start..=end].iter().any(|r| r["k"] == "obs" && r["ev"] == "mem_q")
            {
                w.coverage.unsupported(
                    "driven replay: a memory addition (mem_q) in a rule without the impatience observer",
                );
            }
            return;
        }
        // The rules seen (id: class, initiator, target) and the open handlers (class, rule, phase).
        let mut rules: std::collections::HashMap<i64, (String, J, J)> =
            std::collections::HashMap::new();
        let mut handlers: Vec<(String, i64, String)> = Vec::new();
        for k in start..=end {
            let r = &self.trace.records[k];
            match r["k"].as_str() {
                Some("rule") => {
                    if let Some(id) = r["id"].as_i64() {
                        let class = r["class"].as_str().unwrap_or("").to_owned();
                        rules.insert(id, (class, r["initiator"].clone(), r["target"].clone()));
                    }
                }
                Some("h") => handlers.push((
                    r["class"].as_str().unwrap_or("").to_owned(),
                    r["rule"].as_i64().unwrap_or(0),
                    r["phase"].as_str().unwrap_or("").to_owned(),
                )),
                Some("h_end") => {
                    handlers.pop();
                }
                // The controller's entries, leaves and draws never run inside a rule.
                Some("obs")
                    if matches!(r["ev"].as_str(), Some("uc_imp" | "uc_imp_end"))
                        || (r["ev"] == "uc_v"
                            && matches!(
                                r["f"].as_str(),
                                Some("ipic" | "ivis" | "idist" | "iotw" | "ilvl")
                            )) =>
                {
                    w.coverage.unsupported(format!(
                        "driven replay: impatience: the controller's record inside a rule ({})",
                        describe(r)
                    ));
                }
                Some("draw") if self.imp_draw_record(r) => {
                    w.coverage.unsupported(
                        "driven replay: impatience: the controller's draw inside a rule",
                    );
                }
                // The memory controller's `AddToMemory` in an offensive action's handler
                // (`OffensiveActionsController`'s did handler of a `RuleAttackRoll`): the sim's
                // next call, of its writer's shape. Whether it queued is the memory queue's,
                // which isn't wired: taken as recorded.
                Some("obs") if r["ev"] == "mem_q" => {
                    let rule = handlers.last().and_then(|(class, rule, phase)| {
                        (class == "Kingmaker.Controllers.OffensiveActionsController"
                            && phase == "did")
                            .then(|| rules.get(rule))
                            .flatten()
                            .filter(|(rc, ..)| rc.rsplit('.').next() == Some("RuleAttackRoll"))
                    });
                    let unit = |v: &J| v.get("$unit").cloned().unwrap_or(J::Null);
                    let pair = (r["o"].clone(), r["tgt"].clone());
                    let shape = super::engage::fields(
                        r,
                        &["o", "tgt", "peaceful", "queued", "exc", "by"],
                    ) && r["t"].as_i64() == Some(self.tick)
                        && r["exc"] == false
                        && r["peaceful"].is_boolean()
                        && r["queued"].is_boolean()
                        && r["by"].as_str().is_some_and(|b| {
                            b.starts_with(
                                "UnitMemoryController.AddToMemory<UnitMemoryController.HandleUnitMakeOffensiveAction<",
                            )
                        });
                    let placed = rule.is_some_and(|(_, a, t)| {
                        pair == (unit(a), unit(t)) || pair == (unit(t), unit(a))
                    });
                    let want =
                        w.combat.impatience.memory_adds.as_mut().and_then(VecDeque::pop_front);
                    let sim = want.map(|(o, t)| {
                        (
                            J::from(w.entity(o).unique_id.clone()),
                            J::from(w.entity(t).unique_id.clone()),
                        )
                    });
                    if shape && placed && sim.as_ref() == Some(&pair) {
                        self.out.mem_adds += 1;
                    } else {
                        w.coverage.unsupported(format!(
                            "driven replay: memory: an AddToMemory the sim didn't make, or out of its handler ({})",
                            describe(r)
                        ));
                        if let Some(q) = w.combat.impatience.memory_adds.as_mut() {
                            q.clear();
                        }
                    }
                }
                Some("obs") if r["ev"].as_str().is_some_and(|e| e.starts_with("imp_")) => {
                    let placed = handlers.last().is_some_and(|(class, rule, phase)| {
                        let Some((rc, a, t)) = rules.get(rule) else { return false };
                        let rc = rc.rsplit('.').next().unwrap_or("");
                        let unit = |v: &J| v.get("$unit").cloned().unwrap_or(J::Null);
                        phase == "did"
                            && r["t"].as_i64() == Some(self.tick)
                            && match (class.as_str(), r["ev"].as_str(), r["at"].as_str(), rc) {
                                (
                                    "Kingmaker.Controllers.UnitImpatienceController",
                                    Some("imp_s"),
                                    Some("bother"),
                                    "RuleDealDamage",
                                )
                                | (
                                    "Kingmaker.Controllers.OffensiveActionsController",
                                    Some("imp_s"),
                                    Some("bother"),
                                    "RuleAttackRoll",
                                ) => r["u"] == unit(t) && r["b"] == unit(a),
                                (
                                    "Kingmaker.Controllers.UnitImpatienceController",
                                    Some("imp_s"),
                                    Some("dmg"),
                                    "RuleDealDamage",
                                ) => r["u"] == unit(a) && r["rule"].as_i64() == Some(*rule),
                                _ => false,
                            }
                    });
                    if placed {
                        let damage = handlers.last().is_some_and(|(class, ..)| {
                            class == "Kingmaker.Controllers.UnitImpatienceController"
                        });
                        self.imp_record_in(w, k, Some(damage));
                    } else {
                        w.coverage.unsupported(format!(
                            "driven replay: impatience: a record out of its rule's handler ({})",
                            describe(r)
                        ));
                        w.combat.impatience.queue.clear();
                        if let Some(u) = unit_of(w, &r["u"])
                            && let Some(x) = w.unit_mut(u)
                        {
                            x.combat.imp.known = false;
                        }
                    }
                }
                _ => {}
            }
        }
        // The rule's handlers' records all came inside it.
        let due = w.combat.impatience.queue.iter().any(|op| {
            matches!(op, Op::Bother { .. } | Op::DamageEnd { .. } | Op::DamageList { .. })
        });
        if due && !self.uc_skipping {
            w.coverage.unsupported(
                "driven replay: impatience: a rule handler's record missing from its rule's span",
            );
            w.combat.impatience.queue.clear();
        }
        if w.combat.impatience.memory_adds.as_ref().is_some_and(|q| !q.is_empty()) {
            if !self.uc_skipping {
                w.coverage.unsupported(
                    "driven replay: memory: an AddToMemory missing from its rule's span",
                );
            }
            if let Some(q) = w.combat.impatience.memory_adds.as_mut() {
                q.clear();
            }
        }
    }

    /// `uc_imp` from outside the controller: its tick (from `GameMode.Tick`) or `UpdatePriorityTarget`
    /// (from `TurnController.Prepare`), placed by its caller.
    pub(super) fn imp_entry(&mut self, w: &mut World, i: usize) -> usize {
        self.tgt_runs.clear();
        let trace = self.trace;
        let r = &trace.records[i];
        self.i = i + 1;
        let f = r["fn"].as_str().filter(|f| *f == "Tick" || *f == "UpdatePriorityTarget");
        let origin = r["origin"].as_str().unwrap_or("");
        let tick = f == Some("Tick");
        let unit = unit_of(w, &r["u"]);
        let awake = units(w, &r["awake"]);
        let ok = self.imp
            && fields(r, &["fn", "nested", "origin", "u", "awake"])
            && r["nested"] == false
            && matches!(opt_str(r, "origin"), Some(Some(_)))
            && if tick {
                r["u"].is_null() && awake.as_ref().is_some_and(|a| a.iter().all(Option::is_some))
            } else {
                unit.is_some() && r["awake"].is_null()
            };
        let Some(f) = f.filter(|_| ok) else {
            w.coverage.unsupported(if self.imp {
                "driven replay: a uc_imp record it can't read"
            } else {
                "driven replay: an impatience entry without the impatience observer"
            });
            return self.i;
        };
        // The tick's whole chain is `GameMode.Tick`'s; `Prepare`'s is checked against its
        // `ForceTick`'s, which follows (`imp_prepare_tail`).
        let tail = origin
            .strip_prefix(&format!("Kingmaker.Controllers.UnitImpatienceController.{f} < "))
            .unwrap_or("");
        let own = !tail.is_empty();
        let caller_ok = if tick {
            tail == "Kingmaker.GameModes.GameMode.Tick < Kingmaker.Game.Tick < Kingmaker.Runner.Update"
        } else {
            tail.starts_with("TurnBased.Controllers.TurnController.Prepare < ")
        };
        let placed = phase_of(origin)
            .filter(|p| tick == (*p == super::Phase::Impatience))
            .filter(|_| own && caller_ok);
        match placed {
            Some(p) => self.place(w, p, "the impatience controller"),
            None => w.coverage.unsupported(
                "driven replay: the impatience controller called from a source the replay can't place",
            ),
        }
        if tick {
            if self.imp_tick_frame.replace(self.tick) == Some(self.tick) {
                w.coverage.unsupported("driven replay: two impatience controller ticks in a frame");
            }
            // The awake units: the sim's, as far as it knows them (each unit it knows awake is
            // listed, each other listed unit one whose awake state it can't establish), once each.
            let rec: Vec<EntityIdx> =
                awake.clone().unwrap_or_default().into_iter().flatten().collect();
            let sim = &w.combat.awake;
            let unknown = &w.combat.awake_unknown;
            let mut d = rec.clone();
            d.sort_unstable();
            d.dedup();
            // (In the state's units' order, as `SleepingUnitsController` lists them.)
            let order = crate::combat::life::all_units(w);
            let at = |u: &EntityIdx| order.iter().position(|x| x == u);
            let ordered = rec
                .windows(2)
                .all(|p| matches!((at(&p[0]), at(&p[1])), (Some(a), Some(b)) if a < b));
            let consistent = d.len() == rec.len()
                && ordered
                && sim.iter().all(|u| rec.contains(u))
                && rec.iter().all(|u| sim.contains(u) || unknown.contains(u));
            if !consistent {
                if std::env::var_os("WR_DEBUG_COMMANDS").is_some() {
                    eprintln!(
                        "DEBUG impatience tick {}: awake recorded {rec:?}, sim {sim:?}, unknown {unknown:?}",
                        self.tick
                    );
                }
                w.coverage.unsupported(
                    "driven replay: the impatience controller's awake units aren't the sim's",
                );
            }
        } else {
            if self.imp_prepare_ready.take() != unit {
                w.coverage.unsupported(
                    "driven replay: UpdatePriorityTarget before its TurnController.Prepare's interrupts and cooldowns' clear",
                );
            }
            self.imp_prepared.push(unit.expect("checked"));
            self.imp_prepare_tail = Some(tail.to_owned());
        }
        let outer = std::mem::replace(&mut self.uc_origin, origin.to_owned());
        self.uc_active += 1;
        self.out.imp_entries += 1;
        let res = (|| -> R<()> {
            self.imp_drain(w)?;
            if w.combat.impatience.queue.iter().any(Op::recorded) {
                return Err(Desync("an impatience entry while a handler's record is due".into()));
            }
            if tick {
                let a: Vec<EntityIdx> = awake.unwrap_or_default().into_iter().flatten().collect();
                imp::tick(w, self, &a)?;
            } else {
                imp::update_priority_target(w, self, unit.expect("checked"))?;
            }
            let at = self.imp_next(w, "the impatience controller's end", |x| {
                obs(x, "uc_imp_end").is_some()
            })?;
            let e = &self.trace.records[at];
            if !fields(e, &["fn", "exc"]) || e["fn"] != f || e["exc"] != false {
                return self
                    .uc_reject(at, "an impatience entry's end of another call, or an exception");
            }
            Ok(())
        })();
        self.uc_active -= 1;
        self.uc_origin = outer;
        if let Err(d) = res {
            self.uc_desync(w, &d);
            self.uc_skip(w, |x| obs(x, "uc_imp_end").is_some() && x["fn"] == f);
            // What the skipped records did is unknown: no unit's state is established again.
            w.combat.impatience.queue.clear();
            for u in crate::scene::units_pool(w) {
                if let Some(x) = w.unit_mut(u) {
                    x.combat.imp.known = false;
                }
            }
        }
        self.i
    }

    /// The end of a frame: the handlers' records all came, the controller ticked where its mode
    /// runs it, and after the window's first frame every unit's state is established.
    pub(super) fn imp_frame_end(&mut self, w: &mut World) {
        if !self.imp {
            return;
        }
        if let Err(Desync(d)) = self.imp_drain(w) {
            w.coverage.unsupported(format!("driven replay: impatience: {d}"));
        }
        if !w.combat.impatience.queue.is_empty() {
            w.combat.impatience.queue.clear();
            w.coverage.unsupported(
                "driven replay: an impatience handler record the recording doesn't have",
            );
        }
        if let Some(q) = w.combat.impatience.memory_adds.as_mut().filter(|q| !q.is_empty()) {
            q.clear();
            w.coverage
                .unsupported("driven replay: memory: an AddToMemory the recording doesn't have");
        }
        self.imp_prepare_ready = None;
        if !self.imp_prepared.is_empty() || self.imp_prepare_tail.take().is_some() {
            self.imp_prepared.clear();
            w.coverage.unsupported(
                "driven replay: UpdatePriorityTarget from a TurnController.Prepare the sim didn't run",
            );
        }
        if let Some(m) = self.frame.mode
            && super::registered(m, super::Phase::Impatience) == Some(true)
            && self.imp_tick_frame != Some(self.tick)
        {
            w.coverage.unsupported(
                "driven replay: a frame without the impatience controller's tick (uc_imp)",
            );
        }
        if !self.imp_init_closed && self.tgt_init_closed {
            self.imp_init_closed = true;
            if self.imp_init_n != self.tgt_init_units.len() {
                w.coverage.unsupported(
                    "driven replay: the initial impatience (imp_init) doesn't cover eng_init's units",
                );
            }
            for u in crate::scene::units_pool(w) {
                if !w.unit(u).is_some_and(|x| x.combat.imp.known) {
                    w.coverage.unsupported(
                        "driven replay: a unit without its initial impatience (imp_init)",
                    );
                }
            }
        }
    }

    /// Whatever the impatience observer writes (its observations, entries, leaves, draws), with
    /// the observer or not.
    pub(super) fn imp_family(&self, r: &J) -> bool {
        match r["k"].as_str() {
            Some("obs") => {
                r["ev"].as_str().is_some_and(|e| e.starts_with("imp_") || e.starts_with("uc_imp"))
                    || (r["ev"] == "uc_v"
                        && matches!(
                            r["f"].as_str(),
                            Some("ipic" | "ivis" | "idist" | "iotw" | "ilvl")
                        ))
            }
            Some("draw") => self.trace.site_key(r).as_deref().and_then(site_of).is_some(),
            _ => false,
        }
    }

    /// The controller's next record, taken where its code makes it (nothing else is recorded in
    /// its code): it must be the very next one.
    fn imp_next(&mut self, w: &mut World, what: &str, want: impl Fn(&J) -> bool) -> R<usize> {
        let r = self.trace.records.get(self.i);
        if !r.is_some_and(&want) {
            let d = r.map_or_else(|| "its end".to_owned(), describe);
            return self.uc_reject(self.i, format!("{what} next, the recording has {d}"));
        }
        self.uc_pull(w, what, want)
    }

    /// A leaf of the controller's, the very next record.
    fn imp_leaf(&mut self, w: &mut World, f: &'static str, payload: &[&str]) -> R<usize> {
        let r = self.trace.records.get(self.i);
        if !r.is_some_and(|r| obs(r, "uc_v").is_some() && r["f"] == f) {
            let d = r.map_or_else(|| "its end".to_owned(), describe);
            return self.uc_reject(
                self.i,
                format!("the impatience controller reads {f} next, the recording has {d}"),
            );
        }
        self.uc_leaf_record(w, f, payload)
    }

    /// A command start's record still due when a lifecycle record comes: `UnitCommand.Start` raised
    /// it before anything after it, so it's missing. (A death's records may come after records the
    /// sim's life phase got ahead of: `LeaveCombat`'s own commands.)
    pub(super) fn imp_due(&self, w: &World) -> R<()> {
        let due = w.combat.impatience.queue.iter().any(|op| matches!(op, Op::Start { .. }));
        if self.imp && !self.uc_skipping && due {
            if std::env::var_os("WR_DEBUG_COMMANDS").is_some() {
                eprintln!(
                    "DEBUG impatience tick {}: due {:?} at record {}",
                    self.tick,
                    w.combat.impatience.queue,
                    self.trace.records.get(self.i).map_or_else(String::new, describe)
                );
            }
            return Err(Desync("an impatience handler's record is missing".into()));
        }
        Ok(())
    }

    /// The controller's draw record at `site` (`Range(0, 100)`, or `Range(0f, max)`), the very
    /// next record, checked as `Rng` writes it.
    fn imp_draw_record_at(&mut self, w: &mut World, site: CallSite, max: Option<f32>) -> R<usize> {
        let ids = self.imp_sites.clone();
        let at = self.imp_next(w, "the impatience controller's draw", |r| {
            r["k"] == "draw"
                && (r["site"].as_i64().is_some_and(|s| ids.contains(&s))
                    || r["key"].as_str().and_then(site_of).is_some())
        })?;
        let r = &self.trace.records[at];
        let args_ok = match max {
            None => r["m"] == "Range_ii" && r["a"] == serde_json::json!([0, 100]),
            Some(m) => {
                #[allow(clippy::cast_possible_truncation)]
                let f = |x: &J| x.as_f64().map(|v| (v as f32).to_bits());
                r["m"] == "Range_ff"
                    && r["a"].as_array().is_some_and(|a| {
                        a.len() == 2
                            && f(&a[0]) == Some(0f32.to_bits())
                            && f(&a[1]) == Some(m.to_bits())
                    })
            }
        };
        let ok = self.trace.site_key(r).as_deref() == Some(site.0)
            && r.get("key").is_none()
            && r["c"] == "sim"
            && r["rule"] == 0
            && r["h"] == 0
            && r.get("ctx").is_none()
            && r.get("forced").is_some() == self.replay_mode
            && super::commands::draw_ok(r, &self.trace.sites)
            && args_ok;
        if !ok {
            let d = describe(r);
            return self.uc_reject(
                at,
                format!("the impatience controller's draw at {}, the recording has {d}", site.0),
            );
        }
        self.out.imp_draws += 1;
        Ok(at)
    }

    /// A draw of the impatience controller's (a lifecycle record with the observer).
    pub(super) fn imp_draw_record(&self, r: &J) -> bool {
        self.imp
            && r["k"] == "draw"
            && (r["site"].as_i64().is_some_and(|s| self.imp_sites.contains(&s))
                || self.trace.site_key(r).as_deref().and_then(site_of).is_some())
    }

    /// After a desync, a skipped impatience draw: the tape's too, to keep the site's queue in step.
    pub(super) fn imp_skip_draw(&self, w: &mut World, r: &J) {
        let Some(site) = self.trace.site_key(r).as_deref().and_then(site_of) else { return };
        if r["m"] == "Range_ii" {
            let _ = w.rng.range_int(site, 0, 100);
        } else {
            #[allow(clippy::cast_possible_truncation)]
            let max = r["a"][1].as_f64().unwrap_or(0.0) as f32;
            let _ = w.rng.range_float(site, 0.0, max);
        }
    }
}

impl ImpInputs for Driver<'_> {
    fn imp_in_combat(&mut self, w: &mut World) -> R<bool> {
        let at = self.imp_leaf(w, "ipic", &["v"])?;
        let v = self.trace.records[at]["v"].as_bool();
        match v {
            // `Player.IsInCombat`: the sim's.
            Some(v) if v == w.combat.combat_active => Ok(v),
            _ => {
                let d = describe(&self.trace.records[at]);
                self.uc_reject(
                    at,
                    format!(
                        "Player.IsInCombat is {} in the sim, the recording has {d}",
                        w.combat.combat_active
                    ),
                )
            }
        }
    }

    fn imp_visible(&mut self, w: &mut World, o: EntityIdx) -> R<bool> {
        let at = self.imp_leaf(w, "ivis", &["o", "v"])?;
        let r = &self.trace.records[at];
        match (unit_of(w, &r["o"]) == Some(o), r["v"].as_bool()) {
            (true, Some(v)) => Ok(v),
            _ => {
                let d = describe(r);
                self.uc_reject(
                    at,
                    format!(
                        "the sim reads the visibility of {}, the recording has {d}",
                        w.entity(o).unique_id
                    ),
                )
            }
        }
    }

    fn imp_distance(&mut self, w: &mut World, u: EntityIdx, o: EntityIdx) -> R<f32> {
        let at = self.imp_leaf(w, "idist", &["u", "o", "v"])?;
        let r = &self.trace.records[at];
        match (unit_of(w, &r["u"]) == Some(u) && unit_of(w, &r["o"]) == Some(o), float(&r["v"])) {
            (true, Some(v)) if v >= 0.0 => Ok(v),
            _ => self.uc_reject(at, "a distance of other units, or one it can't read"),
        }
    }

    fn imp_on_the_way(&mut self, w: &mut World, c: u32) -> R<Vec<bool>> {
        let at = self.imp_leaf(w, "iotw", &["c", "v"])?;
        let r = &self.trace.records[at];
        let v: Option<Vec<bool>> =
            r["v"].as_array().and_then(|a| a.iter().map(J::as_bool).collect());
        match (id(&r["c"]) == Some(c), v) {
            (true, Some(v)) => Ok(v),
            _ => self.uc_reject(
                at,
                format!("the projectiles of command {c}, or a record it can't read"),
            ),
        }
    }

    fn imp_level(&mut self, w: &mut World, u: EntityIdx) -> R<i32> {
        let at = self.imp_leaf(w, "ilvl", &["u", "v"])?;
        let r = &self.trace.records[at];
        match (unit_of(w, &r["u"]) == Some(u), r["v"].as_i64().filter(|v| (-1..=1).contains(v))) {
            (true, Some(v)) => Ok(i32::try_from(v).expect("in range")),
            _ => self.uc_reject(at, "an impatience level of another unit, or one it can't read"),
        }
    }

    fn imp_draw_switch(&mut self, w: &mut World) -> R<i32> {
        let at = self.imp_draw_record_at(w, imp::SWITCH_DRAW, None)?;
        let v = w.rng.range_int(imp::SWITCH_DRAW, 0, 100);
        if self.trace.records[at]["v"].as_i64() != Some(i64::from(v)) {
            let d = describe(&self.trace.records[at]);
            return self.uc_reject(
                at,
                format!("UpdatePriorityTarget's draw {v} differs from the recorded {d}"),
            );
        }
        Ok(v)
    }

    fn imp_draw_setup(&mut self, w: &mut World, max: f32) -> R<f32> {
        let at = self.imp_draw_record_at(w, imp::SETUP_DRAW, Some(max))?;
        let v = w.rng.range_float(imp::SETUP_DRAW, 0.0, max);
        #[allow(clippy::cast_possible_truncation)]
        let rec = self.trace.records[at]["v"].as_f64().map(|x| x as f32);
        if rec.map(f32::to_bits) != Some(v.to_bits()) {
            return self.uc_reject(
                at,
                format!("SetupPriorityTarget's draw {v} differs from the recorded {rec:?}"),
            );
        }
        Ok(v)
    }

    fn imp_state(&mut self, w: &mut World, at_name: &'static str, u: EntityIdx) -> R<()> {
        let at = self.imp_next(w, &format!("the impatience state after {at_name}"), |r| {
            obs(r, "imp_s").is_some()
        })?;
        let r = &self.trace.records[at];
        let s = imp::state(w, u)?.clone();
        if r["at"] != at_name
            || !state_fields(r, &[])
            || unit_of(w, &r["u"]) != Some(u)
            || !state_equal(w, r, &s)
        {
            let d = describe(r);
            return self.uc_reject(
                at,
                format!(
                    "{}'s impatience after {at_name}: the sim's {s:?}, the recording has {d}",
                    w.entity(u).unique_id
                ),
            );
        }
        self.out.imp_states += 1;
        Ok(())
    }

    fn imp_nested(&mut self, w: &mut World, u: EntityIdx) -> R<()> {
        let at =
            self.imp_next(w, "the tick's UpdatePriorityTarget", |r| obs(r, "uc_imp").is_some())?;
        let r = &self.trace.records[at];
        let ok = fields(r, &["fn", "nested", "origin", "u", "awake"])
            && r["fn"] == "UpdatePriorityTarget"
            && r["nested"] == true
            && unit_of(w, &r["u"]) == Some(u)
            && r["awake"].is_null()
            && r["origin"].as_str().is_some_and(|o| {
                o.strip_prefix(
                    "Kingmaker.Controllers.UnitImpatienceController.UpdatePriorityTarget < ",
                ) == Some(self.uc_origin.as_str())
            });
        if !ok {
            return self.uc_reject(
                at,
                "the tick's UpdatePriorityTarget of another unit, or one it can't read",
            );
        }
        Ok(())
    }

    fn imp_nested_end(&mut self, w: &mut World) -> R<()> {
        let at =
            self.imp_next(w, "UpdatePriorityTarget's end", |r| obs(r, "uc_imp_end").is_some())?;
        let r = &self.trace.records[at];
        if !fields(r, &["fn", "exc"]) || r["fn"] != "UpdatePriorityTarget" || r["exc"] != false {
            return self
                .uc_reject(at, "UpdatePriorityTarget's end of another call, or an exception");
        }
        Ok(())
    }
}
