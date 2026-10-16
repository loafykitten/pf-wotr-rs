//! The driven replay's side of the engagement controller ([`crate::combat::engage`]): WrathOracle's
//! observer `engage`. An entry record (`uc_eng`: `UnitCombatEngagementController.Tick`) starts the
//! sim's controller, which pulls the leaves it reads (`uc_v`), the attacks of opportunity it runs
//! and its checks (`eng_tu`, `eng_e`, `eng_aoo`, `ai_nct`, `cd_write`) in order, as the commands
//! model does ([`super::commands`]). `eng_init` gives the window's initial state; `eng_prov`, a
//! provoked or forced attack of opportunity, is checked against the sim's queue.

use crate::combat::oracle::Rv;

use super::Driver;
use super::commands::{describe, id, is_unit, obs, opt_str, phase_of};
use crate::combat::commands::{self as uc, R};
use crate::combat::engage::{self, AooCheck, EngInputs, EngageCheck};
use crate::world::{EntityIdx, World};

/// A `float` written round-trip as a string (finite).
pub(super) fn float(v: &Rv) -> Option<f32> {
    v.as_str().and_then(|s| s.parse::<f32>().ok()).filter(|f| f.is_finite())
}

/// Exactly these fields besides the envelope (`seq`, `k`, `t`, `ev`).
pub(super) fn fields(r: &Rv, payload: &[&str]) -> bool {
    const BASE: [&str; 4] = ["seq", "k", "t", "ev"];
    r.as_object().is_some_and(|o| {
        o.len() == BASE.len() + payload.len()
            && BASE.iter().chain(payload).all(|k| o.contains_key(*k))
    })
}

/// A list of units the sim has: `None` when it isn't one (or names another entity).
fn unit_list(w: &World, v: &Rv) -> Option<Vec<String>> {
    v.as_array()?.iter().map(|x| x.as_str().filter(|s| is_unit(w, s)).map(str::to_owned)).collect()
}

/// `m_AttackedByAooUnits`: null until the unit's first attack of opportunity (no reader tells it
/// from an empty list).
fn attacked_list(w: &World, v: &Rv) -> Option<Vec<String>> {
    if v.is_null() { Some(Vec::new()) } else { unit_list(w, v) }
}

/// A frame of the engagement controller's `Tick` lambdas (`EngagedBy.ForEach(u => ...)`: a closure
/// class's `<Tick>b__` method).
fn tick_lambda(f: &str) -> bool {
    f.strip_prefix("Kingmaker.Controllers.Combat.UnitCombatEngagementController+<>c__DisplayClass")
        .and_then(|r| r.split_once('.'))
        .is_some_and(|(_, m)| m.starts_with("<Tick>b__") && !m.contains(' '))
}

/// The frames between `AttackOfOpportunity` and the engagement's tick: `Disengage` in `TickUnit`,
/// or (a provoked one) a `Tick` lambda through `ForEach`.
fn aoo_path(rest: &[&str], disengaging: bool) -> bool {
    if disengaging {
        rest == [
            "Kingmaker.Controllers.Combat.UnitCombatState.Disengage",
            "Kingmaker.Controllers.Combat.UnitCombatEngagementController.TickUnit",
        ]
    } else {
        matches!(rest, [l, "Kingmaker.Utility.LinqExtensions.ForEach"] if tick_lambda(l))
    }
}

/// A C# `int`.
fn int(v: &Rv) -> Option<i32> {
    v.as_i64().and_then(|x| i32::try_from(x).ok())
}

impl Driver<'_> {
    /// `eng_init`: a unit's engagement at the window's first tick. Adopted only without history
    /// (both dictionaries empty: a removal's free slot would decide the next one's place).
    pub(super) fn eng_init(&mut self, w: &mut World, r: &Rv) {
        if !self.eng {
            w.coverage.unsupported("driven replay: an eng_init record without the engage observer");
            return;
        }
        let u =
            r["u"].as_str().and_then(|u| w.by_id.get(u).copied()).filter(|e| w.unit(*e).is_some());
        let lists = ["eu", "eb", "dat", "abu"]
            .map(|k| if k == "abu" { attacked_list(w, &r[k]) } else { unit_list(w, &r[k]) });
        let ok =
            fields(r, &["u", "cnt", "eu", "eb", "dat", "abu", "prevent", "eu_slots", "eb_slots"])
                && r["prevent"].is_boolean()
                && lists.iter().all(Option::is_some);
        let (Some(u), Some(cnt), true) = (u, int(&r["cnt"]), ok) else {
            w.coverage.unsupported("driven replay: an eng_init record it can't read");
            return;
        };
        self.tgt_eng_init(w, u);
        if w.unit(u).is_some_and(|x| x.combat.eng.known) {
            w.coverage.unsupported("driven replay: a unit's engagement given twice");
            return;
        }
        if self.eng_init_closed {
            w.coverage.unsupported(
                "driven replay: an eng_init record after the window's first engagement tick",
            );
            return;
        }
        let [eu, eb, dat, abu] = lists.map(Option::unwrap_or_default);
        // A dictionary is fresh (or cleared) only when no slot was ever used: a freed slot keeps
        // its place and is reused first, so keys alone don't give the layout.
        if !eu.is_empty() || !eb.is_empty() || r["eu_slots"] != 0 || r["eb_slots"] != 0 {
            w.coverage.unsupported(
                "driven replay: engagement at the window's start (its dictionaries' layout isn't recorded)",
            );
            return;
        }
        let to_idx = |w: &World, v: &[String]| -> Vec<EntityIdx> {
            v.iter().filter_map(|s| w.by_id.get(s).copied()).collect()
        };
        let dat = to_idx(w, &dat);
        let abu = to_idx(w, &abu);
        let e = &mut w.unit_mut(u).expect("a unit").combat.eng;
        e.known = true;
        e.count = Some(cnt);
        e.disengage_targets = dat;
        e.attacked_by_aoo = abu;
    }

    /// `eng_prov`: `ProvokeAttackOfOpportunity`, `...AfterDefensivelyFail` or
    /// `ForceAttackOfOpportunity`, from anywhere: only the rule handlers' provoke is the sim's, the
    /// units provoking compared with its queue at the next engagement tick. False when it isn't
    /// one the sim models.
    pub(super) fn eng_prov(&mut self, w: &World, r: &Rv, rule: Option<&'static str>) -> bool {
        const FROM: &str = "Kingmaker.Controllers.Combat.UnitCombatEngagementController.ProvokeAttackOfOpportunity < Kingmaker.Controllers.Combat.UnitCombatEngagementController.OnEventDidTrigger < ";
        let unit =
            r["u"].as_str().and_then(|u| w.by_id.get(u).copied()).filter(|e| w.unit(*e).is_some());
        let ok = self.eng
            && fields(r, &["fn", "u", "tgt", "trick", "origin"])
            && r["fn"] == "ProvokeAttackOfOpportunity"
            && r["tgt"].is_null()
            && r["trick"] == false
            && r["origin"].as_str().is_some_and(|o| o.starts_with(FROM));
        match (unit.filter(|_| ok), rule) {
            (Some(u), Some(rule)) => {
                self.eng_prov_seen.push((u, rule));
                true
            }
            _ => false,
        }
    }

    /// `uc_eng`: the engagement controller's tick, placed by its caller.
    pub(super) fn eng_entry(&mut self, w: &mut World, i: usize) -> usize {
        self.tgt_runs.clear();
        let trace = self.trace;
        let r = &trace.records[i];
        self.i = i + 1;
        let origin = r["origin"].as_str().unwrap_or("");
        let ok = self.eng
            && fields(r, &["nested", "origin"])
            && r["nested"] == false
            && origin.starts_with(
                "Kingmaker.Controllers.Combat.UnitCombatEngagementController.Tick < Kingmaker.GameModes.GameMode.Tick",
            );
        if !ok {
            w.coverage.unsupported(if self.eng {
                "driven replay: a uc_eng record it can't read"
            } else {
                "driven replay: an engagement entry without the engage observer"
            });
            return self.i;
        }
        match phase_of(origin).filter(|p| *p == super::Phase::Engagement) {
            Some(p) => self.place(w, p, "the engagement controller"),
            None => w.coverage.unsupported(
                "driven replay: the engagement controller called from a source the replay can't place",
            ),
        }
        if self.eng_tick_frame.replace(self.tick) == Some(self.tick) {
            w.coverage.unsupported("driven replay: two engagement controller ticks in a frame");
        }
        // The window's initial state is complete before the first tick: every unit of the pool
        // has its own (or was constructed since).
        if !self.eng_init_closed {
            self.eng_init_closed = true;
            for u in crate::scene::units_pool(w) {
                if !w.unit(u).is_some_and(|x| x.combat.eng.known) {
                    w.coverage.unsupported(
                        "driven replay: a unit without its initial engagement (eng_init)",
                    );
                }
            }
        }
        self.eng_provokes_match(w);
        let outer = std::mem::replace(&mut self.uc_origin, origin.to_owned());
        self.uc_active += 1;
        self.out.eng_entries += 1;
        let res = (|| -> R<()> {
            engage::tick(w, self)?;
            let at = self.uc_pull(w, "the engagement's end", |x| obs(x, "uc_eng_end").is_some())?;
            if !fields(&self.trace.records[at], &["exc"]) || self.trace.records[at]["exc"] != false
            {
                return self.uc_reject(at, "an engagement's end with an exception");
            }
            Ok(())
        })();
        self.uc_active -= 1;
        self.uc_origin = outer;
        if let Err(d) = res {
            self.uc_desync(w, &d);
            self.uc_skip(w, |x| obs(x, "uc_eng_end").is_some());
            // What the skipped records did is unknown: the engagement isn't established again.
            for u in crate::combat::life::all_units(w) {
                if let Some(x) = w.unit_mut(u) {
                    x.combat.eng = engage::UnitEngage::default();
                }
            }
            w.combat.engage.provoke.clear();
            w.combat.engage.provoke_adf.clear();
            w.combat.engage.provoke_calls.clear();
        }
        self.i
    }

    /// Every provoke call since the last tick (or before the run's end), in order, is the game's.
    fn eng_provokes_match(&mut self, w: &mut World) {
        let seen = std::mem::take(&mut self.eng_prov_seen);
        let calls = std::mem::take(&mut w.combat.engage.provoke_calls);
        if seen == calls {
            self.out.eng_provokes += calls.len();
        } else {
            w.coverage.unsupported(
                "driven replay: the attacks of opportunity provoked differ from the sim's queue",
            );
        }
    }

    /// The run's end: the provoke calls after the last engagement tick.
    pub(super) fn eng_finish(&mut self, w: &mut World) {
        self.eng_provokes_match(w);
    }

    /// The next leaf `f` with exactly these fields, its unit (`u`) and other unit (`k`) checked.
    fn eng_leaf(
        &mut self,
        w: &mut World,
        f: &'static str,
        payload: &[&str],
        units: &[(&str, Option<&str>)],
    ) -> R<usize> {
        let at = self.uc_leaf_record(w, f, payload)?;
        let r = &self.trace.records[at];
        for (k, want) in units {
            if r[*k].as_str() != *want {
                let d = describe(r);
                return self.uc_reject(
                    at,
                    format!("the sim reads {f} of {k}={want:?}, the recording has {d}"),
                );
            }
        }
        Ok(at)
    }

    fn eng_check(&mut self, w: &mut World, ev: &str, payload: &[&str]) -> R<usize> {
        let at = self.uc_pull(w, &format!("the engagement's {ev}"), |r| obs(r, ev).is_some())?;
        if !fields(&self.trace.records[at], payload) {
            return self
                .uc_reject(at, format!("an {ev} record with fields its writer doesn't write"));
        }
        Ok(at)
    }
}

impl EngInputs for Driver<'_> {
    fn eng_bool(
        &mut self,
        w: &mut World,
        f: &'static str,
        u: Option<&str>,
        to: Option<&str>,
    ) -> R<bool> {
        let at = match f {
            "los" => self.eng_leaf(w, f, &["u", "to", "v"], &[("u", u), ("to", to)])?,
            "memc" => self.eng_leaf(w, f, &["of", "v"], &[("of", to)])?,
            "motion" | "prevent" | "fmove" | "canatk" => {
                self.eng_leaf(w, f, &["u", "v"], &[("u", u)])?
            }
            _ => self.eng_leaf(w, f, &["v"], &[])?,
        };
        match self.trace.records[at]["v"].as_bool() {
            Some(v) => Ok(v),
            None => self.uc_reject(at, format!("a leaf {f} it can't read")),
        }
    }

    fn eng_float(&mut self, w: &mut World, f: &'static str, u: &str, to: Option<&str>) -> R<f32> {
        let at = match to {
            Some(_) => self.eng_leaf(w, f, &["u", "to", "v"], &[("u", Some(u)), ("to", to)])?,
            None => self.eng_leaf(w, f, &["u", "v"], &[("u", Some(u))])?,
        };
        // A corpulence is at least 0.5 (`Mathf.Max(..., 0.5f)`), a squared distance never negative.
        let min = if f == "corp" { 0.5 } else { 0.0 };
        match float(&self.trace.records[at]["v"]).filter(|v| *v >= min) {
            Some(v) => Ok(v),
            None => self.uc_reject(at, format!("a leaf {f} it can't read")),
        }
    }

    fn eng_threat_range(&mut self, w: &mut World, u: &str) -> R<Option<f32>> {
        let at = self.eng_leaf(w, "trange", &["u", "v"], &[("u", Some(u))])?;
        let v = &self.trace.records[at]["v"];
        match v {
            Rv::Null => Ok(None),
            v => match float(v).filter(|x| *x >= 0.0) {
                Some(x) => Ok(Some(x)),
                None => self.uc_reject(at, "a leaf trange it can't read"),
            },
        }
    }

    fn eng_unit(&mut self, w: &mut World, f: &'static str, u: &str) -> R<Option<String>> {
        let at = self.eng_leaf(w, f, &["u", "v"], &[("u", Some(u))])?;
        let v = match &self.trace.records[at]["v"] {
            Rv::Null => None,
            Rv::String(s) if is_unit(w, s) => Some(s.as_str().to_owned()),
            _ => return self.uc_reject(at, format!("a leaf {f} it can't read")),
        };
        self.tgt_read(w, at, f, u, v.as_deref())?;
        Ok(v)
    }

    fn eng_conc(&mut self, w: &mut World, u: &str, to: &str) -> R<i64> {
        let at = self.eng_leaf(
            w,
            "conc",
            &["u", "to", "attack", "v"],
            &[("u", Some(u)), ("to", Some(to))],
        )?;
        let r = &self.trace.records[at];
        match (r["attack"].as_bool(), r["v"].as_i64().filter(|v| (0..=2).contains(v))) {
            (Some(false), Some(v)) => Ok(v),
            _ => self.uc_reject(at, "a leaf conc it can't read"),
        }
    }

    fn eng_enemies(&mut self, w: &mut World) -> R<Vec<String>> {
        let at = self.eng_leaf(w, "enemies", &["v"], &[])?;
        match unit_list(w, &self.trace.records[at]["v"]) {
            Some(v) => Ok(v),
            None => {
                self.uc_reject(at, "a leaf enemies it can't read (a unit the sim doesn't have)")
            }
        }
    }

    fn eng_threat(&mut self, w: &mut World, u: EntityIdx, hand: Option<uc::attack::Slot>) -> R<()> {
        let id = w.entity(u).unique_id.clone();
        let at = self.eng_leaf(w, "threat", &["u", "v"], &[("u", Some(&id))])?;
        let r = &self.trace.records[at];
        let recorded = match opt_str(r, "v") {
            Some(None) => None,
            Some(Some(v)) => match uc::attack::SlotFields::parse(v) {
                Some(x) => Some(x),
                None => return self.uc_reject(at, "a threat record it can't read"),
            },
            None => return self.uc_reject(at, "a threat record it can't read"),
        };
        match (hand, recorded) {
            (None, None) => Ok(()),
            (Some(h), Some(x)) if x.slot == Some(h) && uc::attack::slot_holds(w, u, h, &x) => {
                // A limb's item is bound to its recorded id (as the command's reads are).
                if let uc::attack::Slot::Limb(i) = h
                    && let Err(e) = self.bind_limb(w, u, i, &x.item)
                {
                    return self.uc_reject(at, e);
                }
                Ok(())
            }
            (h, x) => self.uc_reject(
                at,
                format!("the sim's threat hand of {id} is {h:?}, the recording's {x:?}"),
            ),
        }
    }

    fn eng_tick_unit(&mut self, w: &mut World, u: EntityIdx, count: Option<i32>) -> R<i32> {
        let at = self.eng_check(w, "eng_tu", &["u", "cnt"])?;
        let r = &self.trace.records[at];
        let id = w.entity(u).unique_id.clone();
        match (r["u"].as_str() == Some(id.as_str()), int(&r["cnt"])) {
            (true, Some(c)) if count.is_none_or(|x| x == c) => {
                if count.is_none() {
                    self.out.eng_counts_adopted += 1;
                }
                Ok(c)
            }
            _ => {
                let d = describe(r);
                self.uc_reject(
                    at,
                    format!(
                        "the sim ticks {id} (count {count:?}), the recording has {d} cnt={}",
                        r["cnt"]
                    ),
                )
            }
        }
    }

    fn eng_done(&mut self, w: &mut World, c: &EngageCheck) -> R<()> {
        let at = self.eng_check(w, "eng_e", &["fn", "u", "tgt", "eu", "eb", "dat", "exc"])?;
        let r = &self.trace.records[at];
        let list = |k: &str| unit_list(w, &r[k]);
        let ok = r["fn"] == c.f
            && r["u"].as_str() == Some(c.unit.as_str())
            && r["tgt"].as_str() == Some(c.target.as_str())
            && list("eu").as_ref() == Some(&c.engaged_units)
            && list("eb").as_ref() == Some(&c.engaged_by)
            && list("dat").as_ref() == Some(&c.disengage_targets)
            && r["exc"] == false;
        if !ok {
            let d = describe(r);
            return self.uc_reject(
                at,
                format!(
                    "the sim's {c:?}, the recording has {d} eu={} eb={} dat={}",
                    r["eu"], r["eb"], r["dat"]
                ),
            );
        }
        self.out.eng_changes += 1;
        Ok(())
    }

    fn eng_aoo(&mut self, w: &mut World, c: &AooCheck) -> R<()> {
        let at = self.eng_check(
            w,
            "eng_aoo",
            &["u", "tgt", "trick", "sim", "dis", "adf", "ret", "cnt", "abu", "exc"],
        )?;
        let r = &self.trace.records[at];
        let ok = r["u"].as_str() == Some(c.unit.as_str())
            && r["tgt"].as_str() == Some(c.target.as_str())
            && r["trick"].as_bool() == Some(c.trick)
            && r["sim"].as_bool() == Some(c.simulate)
            && r["dis"].as_bool() == Some(c.disengaging)
            && r["adf"].as_bool() == Some(c.after_defensively_fail)
            && r["ret"].as_bool() == Some(c.ret)
            && int(&r["cnt"]) == Some(c.count)
            && attacked_list(w, &r["abu"]).as_ref() == Some(&c.attacked_by_aoo)
            && r["exc"] == false;
        if !ok {
            let d = describe(r);
            return self
                .uc_reject(at, format!("the sim's {c:?}, the recording has {d} ret={}", r["ret"]));
        }
        self.out.eng_aoos += 1;
        Ok(())
    }

    fn eng_count_write(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        before: crate::combat::Cooldowns,
        after: crate::combat::Cooldowns,
        disengaging: bool,
    ) -> R<()> {
        // The setter called by `AttackOfOpportunity`, from `Disengage` in `TickUnit` or from the
        // tick's provoked attacks (a lambda through `ForEach`): `CooldownObs`' four frames.
        let at = self.uc_pull(w, "the count's cooldown write", |r| {
            obs(r, "cd_write").is_some()
                && r["writer"] == "UnitCombatState.set_AttackOfOpportunityCount"
        })?;
        let r = &self.trace.records[at];
        let id = w.entity(u).unique_id.clone();
        let frames: Vec<&str> =
            r["caller"].as_str().map_or_else(Vec::new, |c| c.split(" < ").collect());
        let caller_ok = matches!(
            &frames[..],
            [
                "Kingmaker.Controllers.Combat.UnitCombatState.set_AttackOfOpportunityCount",
                "Kingmaker.Controllers.Combat.UnitCombatState.AttackOfOpportunity",
                rest @ ..
            ] if aoo_path(rest, disengaging)
        );
        let ok = fields(r, &["u", "writer", "caller", "b", "a"])
            && caller_ok
            && r["u"].as_str() == Some(id.as_str())
            && r["b"].as_str().and_then(super::parse_cooldowns) == Some(before)
            && r["a"].as_str().and_then(super::parse_cooldowns) == Some(after);
        if !ok {
            let d = describe(r);
            return self.uc_reject(
                at,
                format!("the count's write of {id}: {before:?} to {after:?}, the recording has {d} b={} a={}", r["b"], r["a"]),
            );
        }
        self.out.eng_count_writes += 1;
        Ok(())
    }

    fn eng_new_run(
        &mut self,
        w: &mut World,
        set: u32,
        target: &str,
        trick: bool,
        disengaging: bool,
    ) -> R<u32> {
        let at = self.uc_pull(w, "the attack of opportunity's Run", |r| {
            obs(r, "uc_run").is_some() && r["nested"] == true
        })?;
        let r = &self.trace.records[at];
        let Some(c) = id(&r["c"]) else { return self.uc_reject(at, "a Run it can't read") };
        let described = at > 0
            && obs(&self.trace.records[at - 1], "uc_new").is_some()
            && id(&self.trace.records[at - 1]["c"]) == Some(c);
        // `RunPrefix`'s fields; called by `AttackOfOpportunity` within the engagement's tick.
        let origin = r["origin"].as_str().unwrap_or("");
        let head = "Kingmaker.UnitLogic.Commands.UnitCommands.Run < Kingmaker.UnitLogic.Commands.UnitCommands.Run < Kingmaker.Controllers.Combat.UnitCombatState.AttackOfOpportunity < ";
        // Every frame between `AttackOfOpportunity` and the engagement's tick (its entry's origin).
        let path_ok = origin
            .strip_prefix(head)
            .and_then(|o| o.strip_suffix(self.uc_origin.as_str()))
            .and_then(|o| o.strip_suffix(" < "))
            .is_some_and(|o| aoo_path(&o.split(" < ").collect::<Vec<_>>(), disengaging));
        let st = &w.combat.commands.sets[&set];
        if !described
            || !fields(r, &["set", "u", "c", "fq", "ncq", "prev", "group", "nested", "origin"])
            || !path_ok
            || id(&r["set"]) != Some(set)
            || r["fq"] != false
            || r["ncq"] != false
            || super::commands::opt_id(r, "prev") != Some(st.prev)
            || r["group"].as_bool() != Some(st.group)
            || opt_str(r, "u") != Some(st.owner.as_deref())
        {
            return self.uc_reject(
                at,
                "the attack of opportunity's Run of another command, or one it can't read",
            );
        }
        // `new UnitAttackOfOpportunity(target, forceFlatFooted, shouldTurnToTarget)`: type Free,
        // `ApproachRadius` 1000000, `IgnoreCooldown()`; nothing else set.
        let Some(x) = w.combat.commands.cmds.get(&c) else {
            return self.uc_reject(at, "the attack of opportunity isn't described");
        };
        let fresh = !x.s.started && !x.s.acted && !x.s.finished && x.s.executor.is_none();
        let flags = !x.reaction
            && x.ignore_cooldown
            && x.ignore_before.is_none()
            && !x.cutscene
            && !x.dont_interrupt_tb
            && !x.player
            && !x.rider
            && !x.mount
            && !x.ai
            && x.bases.is_empty();
        let approach = x.ap.as_ref().is_some_and(|a| {
            a.radius.to_bits() == 1_000_000f32.to_bits() && a.max_radius == 0.0 && !a.xz && !a.los
        });
        let own = x.aoo.as_ref().is_some_and(|a| a.force_flat_footed == trick && a.turn);
        if x.class != uc::class::AOO
            || x.s.ty != uc::ty::FREE
            || x.s.target.unit() != Some(target)
            || !fresh
            || !flags
            || !approach
            || !own
        {
            return self.uc_reject(
                at,
                format!("the attack of opportunity {c} isn't the one the sim made"),
            );
        }
        self.out.eng_runs += 1;
        self.tgt_run_push(at);
        Ok(c)
    }
}
