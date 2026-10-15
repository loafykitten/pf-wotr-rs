//! The driven replay's side of units' commands ([`crate::combat::commands`]): WrathOracle's `uc_*`
//! records. An entry record (`uc_ctl`, an outside `uc_run`, `uc_queue` or `uc_x`) starts the sim's
//! lifecycle code, which takes the records that follow as its inputs ([`Inputs`], pulled in order:
//! leaves, class code, the recorded `TickCommand` calls); everything between them (the attack
//! rules class code triggers, the cooldown records, ends made outside base code) is processed as
//! usual. A record the sim didn't ask for, or one other than it asked for, is reported, and the
//! entry's remaining records are skipped up to its end. The state rows (`uc_state`, `uc_cs`,
//! `uc_running`) are the sim's initial state at the window's first tick (`uc_snap`) and checks
//! around the controller's tick (a difference is reported and the recorded state taken).

use std::collections::{HashMap, HashSet};

use serde_json::{Value as J, json};

use super::{Driver, Phase};
use crate::combat::commands::{self as uc, Cmd, Desync, Fx, Inputs, Of, R, Set, State};
use crate::world::{EntityIdx, World};

/// The recorded state rows, as last written.
#[derive(Debug, Default)]
pub(super) struct Rows {
    /// By set: the unit holding it (`None` for a prediction set) and its row.
    sets: HashMap<u32, (Option<String>, String)>,
    cmds: HashMap<u32, String>,
    /// Commands whose row came since the last check (each is checked, held or not).
    fresh: Vec<u32>,
    /// A row came since the last check.
    pending: bool,
    running: Option<String>,
    /// The window's first rows have been taken (`uc_snap`).
    pub ready: bool,
}

pub(super) fn obs<'a>(r: &'a J, ev: &str) -> Option<&'a J> {
    (r["k"] == "obs" && r["ev"] == ev).then_some(r)
}

pub(super) fn id(v: &J) -> Option<u32> {
    v.as_u64().and_then(|x| u32::try_from(x).ok())
}

/// A field that must be there: `Some(None)` for an explicit null, `Some(Some(s))` for a string,
/// `None` when it's missing or of another type.
pub(super) fn opt_str<'a>(r: &'a J, k: &str) -> Option<Option<&'a str>> {
    match r.get(k)? {
        J::Null => Some(None),
        J::String(s) => Some(Some(s)),
        _ => None,
    }
}

/// A field that must be there: an explicit null or an id.
pub(super) fn opt_id(r: &J, k: &str) -> Option<Option<u32>> {
    match r.get(k)? {
        J::Null => Some(None),
        v => id(v).map(Some),
    }
}

/// An id of a unit the sim has (not another entity's).
pub(super) fn is_unit(w: &World, id: &str) -> bool {
    w.by_id.get(id).is_some_and(|e| w.unit(*e).is_some())
}

/// A lifecycle record of the entry being run: one the sim asks for, never processed on the way.
fn boundary(r: &J) -> bool {
    if r["k"] == "tick" {
        return true;
    }
    if r["k"] != "obs" {
        return false;
    }
    match r["ev"].as_str() {
        Some(
            "uc_v" | "uc_fx" | "uc_tc" | "uc_tick" | "uc_tick_end" | "uc_ctl" | "uc_ctl_end"
            | "uc_snap" | "uc_run_end" | "uc_queue_end" | "uc_x_end" | "uc_exc" | "ua_atk"
            | "uc_call" | "uc_ai" | "uc_ai_end" | "ai_sc" | "ai_best" | "ai_sel" | "uc_eng"
            | "uc_eng_end" | "eng_tu" | "eng_e" | "eng_aoo" | "uc_imp" | "uc_imp_end",
        ) => true,
        Some("imp_s") => matches!(r["at"].as_str(), Some("upd" | "setup")),
        Some("uc_end" | "uc_rc") => r["ctx"] != "other",
        Some("uc_run" | "uc_queue") => r["nested"] != false,
        Some("cd_spend") => from_controller(r),
        _ => false,
    }
}

/// A `cd_spend` from `UnitCommandController` (a command acted).
pub(super) fn from_controller(r: &J) -> bool {
    r["caller"]
        .as_str()
        .is_some_and(|c| c.contains("Kingmaker.Controllers.Units.UnitCommandController."))
}

/// Whether a call site (innermost first) is `UnitAttack`'s code down to its method `method`: every
/// frame before it is `UnitAttack`'s (its iterators included).
pub(super) fn in_unit_attack(origin: &str, method: &str) -> bool {
    const T: &str = "Kingmaker.UnitLogic.Commands.UnitAttack";
    for f in origin.split(" < ") {
        if f.strip_prefix(T).is_some_and(|m| m.strip_prefix('.') == Some(method)) {
            return true;
        }
        if !f.strip_prefix(T).is_some_and(|m| m.starts_with('.') || m.starts_with('+')) {
            return false;
        }
    }
    false
}

/// A target wrapper's id seen in a record (the highest so far: a new wrapper's is above it).
fn see_wrapper(w: &mut World, id: Option<u32>) {
    if let Some(x) = id {
        let m = w.combat.commands.max_wrapper;
        // A first sight is the next id `TargetStr` allocates (`++_nextTarget`, a C# `int`).
        if x > m && (x != m + 1 || i32::try_from(x).is_err()) {
            w.coverage.unsupported("driven replay: commands: a target wrapper id out of WrathOracle's allocation order");
        }
        w.combat.commands.max_wrapper = m.max(x);
    }
}

/// A record's short description, for reports.
pub(super) fn describe(r: &J) -> String {
    if r["k"] == "tick" {
        return format!("the next frame ({})", r["t"]);
    }
    let mut s = r["ev"].as_str().or(r["k"].as_str()).unwrap_or("?").to_owned();
    for k in ["f", "fn", "c", "u", "op", "ctx"] {
        if let Some(v) = r.get(k).filter(|v| !v.is_null()) {
            s.push_str(&format!(" {k}={v}"));
        }
    }
    s
}

/// The controller phase a call's origin (innermost first) places it in: the controller
/// `GameMode.Tick` runs it from, or after `Game.Tick` (a coroutine, the UI's `LateUpdate`).
pub(super) fn phase_of(origin: &str) -> Option<Phase> {
    if origin.ends_with("...") {
        return None;
    }
    let frames: Vec<&str> = origin.split(" < ").collect();
    let Some(g) = frames.iter().position(|f| *f == "Kingmaker.GameModes.GameMode.Tick") else {
        let after = frames.iter().any(|f| {
            f.starts_with("UnityEngine.SetupCoroutine.InvokeMoveNext")
                || f.starts_with("UniRx.MainThreadDispatcher.LateUpdate")
        });
        return (after && !frames.iter().any(|f| f.starts_with("Kingmaker.Game.Tick")))
            .then_some(Phase::Post);
    };
    // The controller GameMode.Tick called (BaseUnitController's frames are its base class's).
    let controller = frames[..g]
        .iter()
        .rev()
        .find(|f| !f.starts_with("Kingmaker.Controllers.Units.BaseUnitController."))?;
    let (ty, _) = controller.rsplit_once('.')?;
    let name = ty.rsplit('.').next()?;
    Some(match name {
        "TimeController" => Phase::Time,
        "CombatController" => Phase::Turn,
        "UnitFollowUnitController" => Phase::Follow,
        "UnitMoveController" => Phase::Move,
        "UnitCombatJoinController" => Phase::Join,
        "UnitCombatPrepareController" => Phase::Prepare,
        "UnitImpatienceController" => Phase::Impatience,
        "AiBrainController" => Phase::Ai,
        "UnitCombatEngagementController" => Phase::Engagement,
        "UnitCommandController" => Phase::Commands,
        "UnitRoamingController" => Phase::Roaming,
        "UnitLifeController" => Phase::Life,
        "UnitCombatLeaveController" => Phase::Leave,
        "UnitProneController" => Phase::Prone,
        _ => return None,
    })
}

/// Which entry an entry record starts.
#[derive(Clone, Copy)]
enum Entry {
    Run,
    Queue,
    Mutator,
}

impl Driver<'_> {
    /// A `uc_*` record (the commands model runs).
    pub(super) fn uc_record(&mut self, w: &mut World, i: usize) -> usize {
        let trace = self.trace;
        let r = &trace.records[i];
        let ev = r["ev"].as_str().unwrap_or("");
        // Each record is of the frame it's made in.
        if r["t"].as_i64() != Some(self.tick) {
            w.coverage
                .unsupported(format!("driven replay: a command record ({ev}) of another frame"));
            return i + 1;
        }
        match ev {
            "uc_new" => self.uc_new(w, r),
            "uc_state" => self.uc_state_row(w, r),
            "uc_cs" => match (id(&r["c"]), r["s"].as_str()) {
                (Some(c), Some(s)) if State::parse(s).is_some() => {
                    see_wrapper(w, State::parse(s).and_then(|x| x.target_ref));
                    // A row is of a described command (`uc_new` comes first), of a known unit, and
                    // one per command between two checks.
                    if !w.combat.commands.cmds.contains_key(&c) {
                        w.coverage
                            .unsupported("driven replay: a uc_cs record of an undescribed command");
                    }
                    if State::parse(s).and_then(|x| x.executor).is_some_and(|e| !is_unit(w, &e)) {
                        w.coverage
                            .unsupported("driven replay: commands: a command of an unknown unit");
                    }
                    if self.uc_rows.fresh.contains(&c) {
                        w.coverage.unsupported(
                            "driven replay: two uc_cs rows of one command between checks",
                        );
                    }
                    self.uc_rows.pending = true;
                    self.uc_rows.cmds.insert(c, s.to_owned());
                    self.uc_rows.fresh.push(c);
                    // A row is the state at the moment it's written: checked as it comes.
                    if self.uc_rows.ready {
                        self.uc_check_row(w, c);
                    }
                }
                _ => w.coverage.unsupported("driven replay: a uc_cs record it can't read"),
            },
            "uc_running" => match r["s"].as_str().filter(|s| parse_running(s).is_some()) {
                Some(s) => {
                    self.uc_rows.running = Some(s.to_owned());
                    self.uc_rows.pending = true;
                    if self.uc_rows.ready {
                        self.uc_check_running(w);
                    }
                }
                None => w.coverage.unsupported("driven replay: a uc_running record it can't read"),
            },
            "uc_snap" => self.uc_snap(w),
            "uc_ctl" => return self.uc_ctl(w, i),
            "uc_ai" if r["nested"] == false => return self.ai_entry(w, i),
            "ai_nct" => self.ai_nct_input(w, r),
            "uc_eng" => return self.eng_entry(w, i),
            "eng_init" => {
                self.tgt_eng_init_place(w, i);
                self.eng_init(w, r);
            }
            "tgt_init" => self.tgt_init(w, i),
            "tgt_w" => self.tgt_w(w, i),
            "imp_init" => self.imp_init(w, i),
            "uc_imp" if r["nested"] == false => return self.imp_entry(w, i),
            "imp_s" | "imp_other" => self.imp_record(w, i),
            // A provoke outside an attack rule the sim fires: its handler's context isn't validated
            // (the rule handlers' provokes are taken inside their rule's span).
            "eng_prov" => w.coverage.unsupported(
                "driven replay: an attack of opportunity provoked or forced that the sim doesn't model",
            ),
            "uc_run" if r["nested"] == false => return self.uc_entry(w, i, Entry::Run),
            "uc_queue" if r["nested"] == false => return self.uc_entry(w, i, Entry::Queue),
            "uc_x" => return self.uc_entry(w, i, Entry::Mutator),
            "uc_set" => return self.uc_set(w, i),
            "uc_end" if r["ctx"] == "other" => self.uc_input_end(w, r),
            "uc_rc" if r["ctx"] == "other" => self.uc_input_rc(w, r),
            _ if self.uc_skipping => {}
            _ => w.coverage.unsupported(format!(
                "driven replay: a command lifecycle record the sim's commands didn't ask for ({})",
                describe(r)
            )),
        }
        i + 1
    }

    pub(super) fn uc_desync(&mut self, w: &mut World, d: &Desync) {
        if std::env::var_os("WR_DEBUG_COMMANDS").is_some() {
            eprintln!("DEBUG commands tick {}: {}", self.tick, d.0);
        }
        w.coverage.unsupported(format!("driven replay: commands: {}", d.0));
    }

    /// After a desync: the entry's remaining records, up to its end record (`end`), are skipped
    /// (its rows are still taken and everything else is processed as usual).
    pub(super) fn uc_skip(&mut self, w: &mut World, end: impl Fn(&J) -> bool) {
        let trace = self.trace;
        let recs = &trace.records;
        let was = std::mem::replace(&mut self.uc_skipping, true);
        // The sim's writes not yet matched: the skipped records' writes are taken instead.
        w.combat.targets.pending.clear();
        while let Some(r) = recs.get(self.i) {
            if end(r) {
                self.i += 1;
                break;
            }
            if r["k"] == "tick" {
                w.coverage.unsupported(
                    "driven replay: commands: an entry without its end record in its frame",
                );
                break;
            }
            let rows =
                matches!(r["ev"].as_str(), Some("uc_new" | "uc_state" | "uc_cs" | "uc_running"));
            if self.approach_draw_record(r) {
                // (The tape's draw too, to keep the site's queue in step.)
                let _ = w.rng.range_float(crate::rng::sites::TICK_APPROACHING, 0.2, 0.6);
                self.i += 1;
                continue;
            }
            if self.ai_draw_record(r) {
                let _ = w.rng.range_float(crate::rng::sites::AI_NEXT_COMMAND_TIME, 0.0, 0.1);
                self.i += 1;
                continue;
            }
            if self.imp_draw_record(r) {
                self.imp_skip_draw(w, r);
                self.i += 1;
                continue;
            }
            if r["k"] == "obs"
                && r["ev"] == "imp_s"
                && matches!(r["at"].as_str(), Some("upd" | "setup"))
            {
                self.i += 1;
                continue;
            }
            if r["k"] == "obs"
                && r["ev"].as_str().is_some_and(|e| {
                    (e.starts_with("uc_")
                        || e.starts_with("ua_")
                        || e.starts_with("ai_")
                        || e.starts_with("eng_"))
                        && e != "eng_init"
                        && e != "eng_prov"
                })
                && !rows
            {
                self.i += 1;
                continue;
            }
            let i = self.i;
            self.i = self.record(w, i);
        }
        self.uc_skipping = was;
        self.uc_dirty = true;
        // The skipped records may have written units' `NextCommandTime`: unknown again.
        w.combat.ai.nct.clear();
    }

    /// Records up to the next one `want` takes (returned); everything before it is processed as
    /// usual, but a lifecycle record the sim didn't ask for stops the pull.
    pub(super) fn uc_pull(
        &mut self,
        w: &mut World,
        what: &str,
        want: impl Fn(&J) -> bool,
    ) -> R<usize> {
        let trace = self.trace;
        let recs = &trace.records;
        loop {
            let Some(r) = recs.get(self.i) else {
                return Err(Desync(format!("{what}: the recording ends")));
            };
            if want(r) {
                // (An impatience handler's record comes before the next lifecycle record.)
                let own =
                    r["k"] == "obs" && r["ev"].as_str().is_some_and(|e| e.starts_with("imp_"));
                if !own {
                    self.imp_due(w)?;
                }
                // Each record is of the frame it's made in.
                if r["t"].as_i64() != Some(self.tick) {
                    let d = describe(r);
                    return Err(Desync(format!("a command record ({d}) of another frame")));
                }
                self.i += 1;
                return Ok(self.i - 1);
            }
            if boundary(r)
                || self.approach_draw_record(r)
                || self.ai_draw_record(r)
                || self.imp_draw_record(r)
            {
                return Err(Desync(format!("{what}: the recording has {}", describe(r))));
            }
            let i = self.i;
            self.i = self.record(w, i);
        }
    }

    /// A draw of `TickApproaching` (with the `approach` observer a lifecycle record: the sim's draw
    /// takes it).
    pub(super) fn approach_draw_record(&self, r: &J) -> bool {
        // By its numeric site (a contradictory `key` doesn't hide it) or its key.
        self.uc_approach
            && r["k"] == "draw"
            && (r["site"].as_i64().is_some_and(|s| self.uc_approach_sites.contains(&s))
                || self.trace.site_key(r).as_deref() == Some(crate::rng::sites::TICK_APPROACHING.0))
    }

    /// The next lifecycle record (a pull would stop there), without processing anything.
    fn uc_peek(&self) -> Option<&J> {
        let trace = self.trace;
        trace.records[self.i..].iter().find(|r| {
            boundary(r)
                || self.approach_draw_record(r)
                || self.ai_draw_record(r)
                || self.imp_draw_record(r)
                || (r["k"] == "obs"
                    && r["ev"].as_str().is_some_and(|e| {
                        (e.starts_with("uc_")
                            || e.starts_with("ua_")
                            || e.starts_with("ai_")
                            || e.starts_with("eng_"))
                            && e != "eng_init"
                            && e != "eng_prov"
                    })
                    && !matches!(
                        r["ev"].as_str(),
                        Some("uc_new" | "uc_state" | "uc_cs" | "uc_running")
                    ))
        })
    }

    /// A pulled record that isn't what the sim asked for: it stays unconsumed for the skip.
    pub(super) fn bind_limb(
        &mut self,
        w: &World,
        u: EntityIdx,
        i: usize,
        item: &str,
    ) -> Result<(), String> {
        bind_limb(&mut self.limb_ids, w, u, i, item)
    }

    pub(super) fn uc_reject<T>(&mut self, at: usize, what: impl Into<String>) -> R<T> {
        self.i = at;
        Err(Desync(what.into()))
    }

    fn uc_new(&mut self, w: &mut World, r: &J) {
        let (Some(c), Some(x)) = (id(&r["c"]), Cmd::parse(r)) else {
            w.coverage.unsupported("driven replay: a uc_new record it can't read");
            return;
        };
        see_wrapper(w, x.s.target_ref);
        if x.s.executor.as_ref().is_some_and(|e| !is_unit(w, e)) {
            w.coverage.unsupported("driven replay: commands: a command of an unknown unit");
        }
        // With the attacks observer a `UnitAttack` is described with its flags (and only it); a
        // subclass of it isn't ported.
        let attacks = w.combat.commands.attacks;
        if attacks && (x.class == "UnitAttack") != x.ua.is_some() {
            w.coverage.unsupported(
                "driven replay: a uc_new record whose UnitAttack flags (ua) don't fit its class",
            );
        }
        if !attacks && r.get("ua").is_some() {
            w.coverage
                .unsupported("driven replay: UnitAttack flags (ua) without the attacks observer");
        }
        // A `UnitAttack` initialized before its first sight (the window opened on it): its class state
        // isn't recorded.
        if attacks && x.ua.is_some() && x.s.executor.is_some() {
            w.coverage.unsupported(
                "driven replay: commands: a UnitAttack initialized before its first sight (its class state isn't recorded)",
            );
        }
        if attacks && x.bases.iter().any(|b| b == "UnitAttack") {
            w.coverage
                .unsupported("driven replay: commands: a subclass of UnitAttack (not modelled)");
        }
        // With the approach observer every command is described with its approach, a `UnitMoveTo`
        // with its fields and an attack of opportunity with its own (and only they); a subclass of
        // either isn't ported.
        let approach = w.combat.commands.approach;
        let fits = if approach {
            x.ap.is_some()
                && (x.class == uc::class::MOVE_TO) == x.mv.is_some()
                && (x.class == uc::class::AOO) == x.aoo.is_some()
        } else {
            x.ap.is_none() && x.mv.is_none() && x.aoo.is_none()
        };
        if !fits {
            w.coverage.unsupported(
                "driven replay: a uc_new record whose approach fields (ap, mv, aoo) don't fit its class or observers",
            );
        }
        if approach && x.bases.iter().any(|b| b == uc::class::MOVE_TO || b == uc::class::AOO) {
            w.coverage.unsupported(
                "driven replay: commands: a subclass of UnitMoveTo or UnitAttackOfOpportunity (not modelled)",
            );
        }
        // One initialized before its first sight: its class state isn't recorded.
        if approach && x.class == uc::class::AOO && x.s.executor.is_some() {
            w.coverage.unsupported(
                "driven replay: commands: an attack of opportunity initialized before its first sight",
            );
        }
        if w.combat.commands.cmds.insert(c, x).is_some() {
            w.coverage.unsupported("driven replay: a command described twice (uc_new)");
        }
    }

    fn uc_state_row(&mut self, w: &mut World, r: &J) {
        let (Some(s), Some(row)) = (id(&r["set"]), r["s"].as_str()) else {
            w.coverage.unsupported("driven replay: a uc_state record it can't read");
            return;
        };
        let Some(set) = Set::parse(row) else {
            w.coverage.unsupported("driven replay: a uc_state record it can't read");
            return;
        };
        let unit = match &r["u"] {
            J::Null if r.get("u").is_some() => None,
            J::String(u) => Some(u.clone()),
            _ => {
                w.coverage.unsupported("driven replay: a uc_state record it can't read");
                return;
            }
        };
        // A unit's own set is its: held by a unit the sim has, and owned by it.
        let known = |u: &Option<String>| u.as_ref().is_none_or(|u| is_unit(w, u));
        if !known(&unit) || !known(&set.owner) || (unit.is_some() && unit != set.owner) {
            w.coverage.unsupported("driven replay: a uc_state record of an unknown or other unit");
        }
        // Its commands are described ones.
        if set
            .slots
            .iter()
            .flatten()
            .chain(set.queue.iter())
            .chain(set.prev.iter())
            .any(|c| !w.combat.commands.cmds.contains_key(c))
        {
            w.coverage.unsupported("driven replay: a uc_state record of an undescribed command");
        }
        // A set the sim hasn't seen (a new prediction set, a new unit's) starts as recorded; a known
        // one's row is checked as it comes (the state at the moment it's written).
        let adopt = self.uc_rows.ready && !w.combat.commands.sets.contains_key(&s);
        if adopt {
            w.combat.commands.sets.insert(s, set.clone());
            if let Some(u) = &unit {
                w.combat.commands.unit_set.entry(u.clone()).or_insert(s);
            }
        }
        self.uc_rows.sets.insert(s, (unit.clone(), row.to_owned()));
        self.uc_rows.pending = true;
        if self.uc_rows.ready && !adopt {
            let m = &mut w.combat.commands;
            let mut differ = Vec::new();
            if m.sets.get(&s) != Some(&set) {
                differ.push(format!("set {s}: sim {:?}, recorded {set:?}", m.sets.get(&s)));
                m.sets.insert(s, set);
            }
            if let Some(u) = unit
                && m.unit_set.get(&u) != Some(&s)
            {
                differ.push(format!("unit {u}'s set: sim {:?}, recorded {s}", m.unit_set.get(&u)));
                m.unit_set.insert(u, s);
            }
            self.uc_row_differ(w, &differ);
        }
    }

    /// The window's first rows: the sim's initial commands (the save doesn't hold them).
    fn uc_snap(&mut self, w: &mut World) {
        if self.uc_rows.ready {
            w.coverage.unsupported("driven replay: a second commands snapshot (uc_snap)");
            return;
        }
        self.uc_rows.ready = true;
        let m = &mut w.combat.commands;
        m.on = true;
        for (s, (unit, row)) in &self.uc_rows.sets {
            if let Some(set) = Set::parse(row) {
                m.sets.insert(*s, set);
            }
            if let Some(u) = unit {
                m.unit_set.insert(u.clone(), *s);
            }
        }
        let mut missing = false;
        let mut differ = false;
        for (c, row) in &self.uc_rows.cmds {
            match (m.cmds.get_mut(c), State::parse(row)) {
                // Described in the same snapshot: the description is that state.
                (Some(x), Some(s)) => {
                    differ |= !x.s.same(&s);
                    x.s = s;
                }
                _ => missing = true,
            }
        }
        if differ {
            w.coverage.unsupported(
                "driven replay: the commands snapshot's rows differ from the commands' descriptions",
            );
        }
        // Every unit holds a set (the save doesn't hold commands: a missing row isn't "none").
        let units = crate::combat::life::all_units(w);
        if units.iter().any(|u| !w.combat.commands.unit_set.contains_key(&w.entity(*u).unique_id)) {
            w.coverage
                .unsupported("driven replay: the commands snapshot lacks a unit's command set");
        }
        let m = &mut w.combat.commands;
        m.running =
            self.uc_rows.running.as_deref().and_then(parse_running).flatten().unwrap_or_default();
        // The next rows are a new window.
        self.uc_rows.fresh.clear();
        self.uc_rows.pending = false;
        if missing || self.uc_rows.running.is_none() {
            w.coverage
                .unsupported("driven replay: the commands snapshot names an undescribed command");
        }
    }

    /// The sim's commands against the recorded rows (at the controller's tick, its start and end):
    /// a difference is reported and the recorded state taken.
    fn uc_check(&mut self, w: &mut World, what: &str) {
        if !self.uc_rows.ready {
            w.coverage.unsupported(
                "driven replay: the command controller ticks before the commands snapshot",
            );
            return;
        }
        let dirty = std::mem::take(&mut self.uc_dirty);
        self.uc_rows.pending = false;
        let mut differ = Vec::new();
        let m = &mut w.combat.commands;
        let mut live: Vec<u32> = Vec::new();
        for (s, (unit, row)) in &self.uc_rows.sets {
            let Some(rec) = Set::parse(row) else { continue };
            if m.sets.get(s) != Some(&rec) {
                differ.push(format!("set {s}: sim {:?}, recorded {rec:?}", m.sets.get(s)));
                m.sets.insert(*s, rec.clone());
            }
            if let Some(u) = unit
                && m.unit_set.get(u) != Some(s)
            {
                differ.push(format!("unit {u}'s set: sim {:?}, recorded {s}", m.unit_set.get(u)));
                m.unit_set.insert(u.clone(), *s);
            }
            live.extend(rec.slots.iter().flatten().chain(rec.queue.iter()).chain(rec.prev.iter()));
        }
        let rec_running = self.uc_rows.running.as_deref().and_then(parse_running).flatten();
        let turn = uc::current_turn(w).is_some();
        let m = &mut w.combat.commands;
        let sim_running = turn.then(|| m.running.clone());
        if rec_running != sim_running {
            differ.push(format!(
                "the running commands: sim {sim_running:?}, recorded {rec_running:?}"
            ));
            m.running = rec_running.unwrap_or_default();
        }
        if turn {
            live.extend(m.running.iter());
        }
        live.extend(std::mem::take(&mut self.uc_rows.fresh));
        live.sort_unstable();
        live.dedup();
        for c in live {
            let rec = self.uc_rows.cmds.get(&c).and_then(|r| State::parse(r));
            match (m.cmds.get_mut(&c), rec) {
                (Some(x), Some(rec)) => {
                    if !x.s.same(&rec) {
                        differ.push(format!("command {c}: sim {:?}, recorded {rec:?}", x.s));
                        x.s = rec;
                    }
                }
                _ => differ.push(format!("command {c} isn't described or has no state row")),
            }
        }
        if !differ.is_empty() {
            if std::env::var_os("WR_DEBUG_COMMANDS").is_some() {
                eprintln!("DEBUG commands tick {} {what}: {}", self.tick, differ.join("; "));
            }
            self.out.commands_recorded += differ.len();
            if !dirty {
                w.coverage.unsupported(format!(
                    "driven replay: the sim's commands differ from the recording's at {what}"
                ));
            }
        }
        self.out.commands_checked += 1;
    }

    /// A command's row as it comes, against the sim's state.
    fn uc_check_row(&mut self, w: &mut World, c: u32) {
        let rec = self.uc_rows.cmds.get(&c).and_then(|r| State::parse(r));
        let mut differ = Vec::new();
        match (w.combat.commands.cmds.get_mut(&c), rec) {
            (Some(x), Some(rec)) => {
                if !x.s.same(&rec) {
                    differ.push(format!("command {c}: sim {:?}, recorded {rec:?}", x.s));
                    x.s = rec;
                }
            }
            _ => differ.push(format!("command {c} isn't described or has no state row")),
        }
        self.uc_row_differ(w, &differ);
    }

    /// The running commands' row as it comes, against the sim's (its commands described).
    fn uc_check_running(&mut self, w: &mut World) {
        let rec = self.uc_rows.running.as_deref().and_then(parse_running).flatten();
        let turn = uc::current_turn(w).is_some();
        let m = &mut w.combat.commands;
        let mut differ = Vec::new();
        if rec.iter().flatten().any(|c| !m.cmds.contains_key(c)) {
            differ.push("the running commands name an undescribed command".to_owned());
        }
        let sim = turn.then(|| m.running.clone());
        if rec != sim {
            differ.push(format!("the running commands: sim {sim:?}, recorded {rec:?}"));
            m.running = rec.unwrap_or_default();
        }
        self.uc_row_differ(w, &differ);
    }

    /// A row's differences from the sim's state (already taken): reported, unless the state isn't
    /// established after a desync.
    fn uc_row_differ(&mut self, w: &mut World, differ: &[String]) {
        if differ.is_empty() {
            return;
        }
        if std::env::var_os("WR_DEBUG_COMMANDS").is_some() {
            eprintln!("DEBUG commands tick {} row: {}", self.tick, differ.join("; "));
        }
        self.out.commands_recorded += differ.len();
        if !self.uc_dirty {
            w.coverage.unsupported(
                "driven replay: the sim's commands differ from the recording's at a row",
            );
        }
    }

    /// The recording's end: rows that came after the last check are checked.
    pub(super) fn uc_finish(&mut self, w: &mut World) {
        if self.uc && self.uc_rows.ready && self.uc_rows.pending {
            self.uc_check(w, "the recording's end");
        }
    }

    /// `uc_ctl`: `UnitCommandController.Tick`, run by the sim.
    fn uc_ctl(&mut self, w: &mut World, i: usize) -> usize {
        self.tgt_runs.clear();
        self.i = i + 1;
        if self.uc_ctl_tick.replace(self.tick) == Some(self.tick) {
            w.coverage.unsupported("driven replay: two command controller ticks in a frame");
        }
        self.place(w, Phase::Commands, "the command controller's tick");
        self.uc_check(w, "UnitCommandController.Tick's start");
        self.uc_active += 1;
        let outer = std::mem::replace(&mut self.uc_origin, CONTROLLER.to_owned());
        let res = self.uc_controller(w);
        self.uc_origin = outer;
        self.uc_active -= 1;
        if let Err(d) = res {
            self.uc_desync(w, &d);
            self.uc_skip(w, |r| obs(r, "uc_ctl_end").is_some());
        }
        self.uc_check(w, "UnitCommandController.Tick's end");
        self.i
    }

    /// `BaseUnitController.Tick` for `UnitCommandController`: the awake units, alive, whose
    /// commands the tick works on, in `State.AwakeUnits`' order. A unit whose awake state the sim
    /// can't establish (the recording's last frame has no next snapshot) is ticked where the
    /// recording ticks it.
    fn uc_controller(&mut self, w: &mut World) -> R<()> {
        let unknown: Vec<EntityIdx> = w.combat.awake_unknown.clone();
        let units: Vec<EntityIdx> = crate::combat::life::all_units(w)
            .into_iter()
            .filter(|u| w.combat.awake.contains(u) || unknown.contains(u))
            .collect();
        for u in units {
            if crate::combat::units::is_dead(w, u) || !uc::busy(w, u) {
                continue;
            }
            let uid = w.entity(u).unique_id.clone();
            if unknown.contains(&u) {
                if self
                    .uc_peek()
                    .is_none_or(|r| obs(r, "uc_tick").is_none() || r["u"] != uid.as_str())
                {
                    continue;
                }
                self.out.awake_recorded += 1;
            }
            let at = self.uc_pull(w, "the controller's next unit", |r| {
                obs(r, "uc_tick").is_some() || obs(r, "uc_ctl_end").is_some()
            })?;
            let r = &self.trace.records[at];
            if r["ev"] != "uc_tick" || r["u"].as_str() != Some(uid.as_str()) {
                return self.uc_reject(
                    at,
                    format!("the sim ticks {uid}'s commands next, the recording {}", describe(r)),
                );
            }
            let (Some(view), Some(rb)) = (r["view"].as_bool(), r["rb"].as_bool()) else {
                return self.uc_reject(at, "a uc_tick record it can't read");
            };
            if id(&r["set"]) != w.combat.commands.unit_set.get(&uid).copied() {
                return self.uc_reject(at, format!("unit {uid}'s ticked set isn't the sim's"));
            }
            self.out.commands_ticked += 1;
            uc::tick_on_unit(w, self, u, view, rb)?;
            let at = self.uc_pull(w, "TickOnUnit's end", |r| obs(r, "uc_tick_end").is_some())?;
            if self.trace.records[at]["u"].as_str() != Some(uid.as_str()) {
                return self.uc_reject(at, "a TickOnUnit's end of another unit");
            }
        }
        let at =
            self.uc_pull(w, "the command controller's end", |r| obs(r, "uc_ctl_end").is_some())?;
        if self.trace.records[at]["exc"] != false {
            return Err(Desync("an exception out of the command controller's tick".into()));
        }
        Ok(())
    }

    /// An entry from outside the lifecycle (`uc_run`, `uc_queue`, `uc_x`), placed by its caller.
    fn uc_entry(&mut self, w: &mut World, i: usize, kind: Entry) -> usize {
        let trace = self.trace;
        let r = &trace.records[i];
        self.i = i + 1;
        let origin = r["origin"].as_str().unwrap_or("");
        let outer = std::mem::replace(&mut self.uc_origin, origin.to_owned());
        self.tgt_runs.clear();
        if obs(r, "uc_x").is_some() {
            self.imp_death_window(w, r);
        }
        let click = self.tgt_entry(w, i);
        let running_before = r["u"].as_str().and_then(|u| w.combat.commands.is_running(u));
        match phase_of(origin) {
            Some(p) => self.place(w, p, "a command call"),
            None => w.coverage.unsupported(
                "driven replay: commands: a call from a source the replay can't place",
            ),
        }
        let set = id(&r["set"]);
        let c = id(&r["c"]);
        self.uc_active += 1;
        let res = (|| -> R<()> {
            let Some(s) = set else { return Err(Desync("an entry record it can't read".into())) };
            let owner = w.combat.commands.sets.get(&s).map(|x| x.owner.clone());
            if owner.is_none() {
                return Err(Desync(format!("command set {s} is unknown")));
            }
            if owner.flatten().as_deref() != r["u"].as_str() {
                return Err(Desync(format!("command set {s}'s owner differs from the record's")));
            }
            match kind {
                Entry::Run => {
                    let (Some(c), Some(fq), Some(ncq), Some(prev), Some(group)) = (
                        c,
                        r["fq"].as_bool(),
                        r["ncq"].as_bool(),
                        opt_id(r, "prev"),
                        r["group"].as_bool(),
                    ) else {
                        return Err(Desync("a uc_run record it can't read".into()));
                    };
                    let st = &w.combat.commands.sets[&s];
                    if (st.prev, st.group) != (prev, group) {
                        return Err(Desync(format!(
                            "a Run's parked or group command differs from set {s}'s"
                        )));
                    }
                    self.out.commands_run += 1;
                    self.tgt_run_push(i);
                    uc::run(w, self, s, c, fq, ncq)?;
                    self.tgt_run_pop();
                    let at = self.uc_pull(w, "Run's end", |x| obs(x, "uc_run_end").is_some())?;
                    let e = &self.trace.records[at];
                    if id(&e["set"]) != Some(s) || id(&e["c"]) != Some(c) || e["exc"] != false {
                        return self
                            .uc_reject(at, "a Run's end of another command, or an exception");
                    }
                    let owner = r["u"].as_str().and_then(|u| w.by_id.get(u).copied());
                    let Some(owner) = owner else {
                        return Err(Desync("the Run's unit is unknown".into()));
                    };
                    self.tgt_after_entry(w, i, owner, Some(c), running_before)?;
                    // The driver's click: `unit.CombatState.ManualTarget = target` after its `Run`.
                    if let Some(input) = &click {
                        self.tgt_driver_after(w, owner, c, input)?;
                    }
                }
                Entry::Queue => {
                    let (Some(c), Some(first)) = (c, r["first"].as_bool()) else {
                        return Err(Desync("a uc_queue record it can't read".into()));
                    };
                    uc::add_to_queue_or_run(w, self, s, c, first)?;
                    let at =
                        self.uc_pull(w, "AddToQueue's end", |x| obs(x, "uc_queue_end").is_some())?;
                    if id(&self.trace.records[at]["set"]) != Some(s)
                        || self.trace.records[at]["exc"] != false
                    {
                        return self
                            .uc_reject(at, "an AddToQueue's end of another set, or an exception");
                    }
                }
                Entry::Mutator => {
                    // `raise`: InterruptAll's argument, an explicit null for the others.
                    let raise = match r.get("raise") {
                        Some(J::Null) => None,
                        Some(J::Bool(b)) => Some(*b),
                        _ => return Err(Desync("a uc_x record it can't read".into())),
                    };
                    match (r["fn"].as_str(), raise) {
                        (Some("InterruptAll"), Some(raise)) => {
                            uc::interrupt_all(w, self, s, raise)?
                        }
                        (Some("InterruptAiCommands"), None) => {
                            uc::interrupt_ai_commands(w, self, s)?
                        }
                        (Some("InterruptMove"), None) => uc::interrupt_move(w, self, s)?,
                        (Some("RemoveNoStartStadartCommand"), None) => {
                            uc::remove_unstarted_standard(w, self, s)?;
                        }
                        (Some("InterruptGroupCommand"), None) => {
                            uc::interrupt_group_command(w, self, s)?
                        }
                        // The predicate is its caller's: the AI's commands (`AiBrainController`'s
                        // `cmd.AiAction != null`); other callers' aren't modelled.
                        (Some("InterruptAllWhere"), None) => {
                            let caller = origin.split(" < ").nth(1).unwrap_or("");
                            if caller != "Kingmaker.AI.AiBrainController.TickBrain" {
                                return Err(Desync(format!(
                                    "UnitCommands.InterruptAll(predicate) from {caller} (not modelled)"
                                )));
                            }
                            uc::interrupt_all_where(w, self, s, |x| x.ai)?;
                        }
                        (Some("RemoveFinishedAndUpdateQueue"), None) => {
                            uc::remove_finished_and_update_queue(w, self, s)?;
                        }
                        (f, _) => {
                            return Err(Desync(format!(
                                "UnitCommands.{} (not modelled)",
                                f.unwrap_or("?")
                            )));
                        }
                    }
                    let at =
                        self.uc_pull(w, "the mutator's end", |x| obs(x, "uc_x_end").is_some())?;
                    if id(&self.trace.records[at]["set"]) != Some(s)
                        || self.trace.records[at]["exc"] != false
                    {
                        return self
                            .uc_reject(at, "a mutator's end of another set, or an exception");
                    }
                    if let Some(owner) = r["u"].as_str().and_then(|u| w.by_id.get(u).copied()) {
                        self.tgt_after_entry(w, i, owner, None, running_before)?;
                    }
                }
            }
            Ok(())
        })();
        self.uc_active -= 1;
        self.uc_origin = outer;
        if let Err(d) = res {
            self.uc_desync(w, &d);
            let (end, s) = match kind {
                Entry::Run => ("uc_run_end", set),
                Entry::Queue => ("uc_queue_end", set),
                Entry::Mutator => ("uc_x_end", set),
            };
            self.uc_skip(w, |x| {
                obs(x, end).is_some()
                    && id(&x["set"]) == s
                    && (end != "uc_run_end" || id(&x["c"]) == c)
            });
        }
        self.i
    }

    /// `ReplaceCommands` (`UnitCommands.Temporary`: a prediction set in, then the unit's own back).
    fn uc_set(&mut self, w: &mut World, i: usize) -> usize {
        self.tgt_runs.clear();
        let trace = self.trace;
        let r = &trace.records[i];
        let (Some(u), Some(to), Some(sim), Some(Some(origin)), Some(from)) = (
            r["u"].as_str().filter(|u| is_unit(w, u)),
            id(&r["to"]),
            r["sim"].as_bool(),
            opt_str(r, "origin"),
            opt_id(r, "from"),
        ) else {
            w.coverage.unsupported("driven replay: a uc_set record it can't read");
            return i + 1;
        };
        match phase_of(origin) {
            Some(p) => self.place(w, p, "a command set's replacement"),
            None => w.coverage.unsupported(
                "driven replay: commands: a call from a source the replay can't place",
            ),
        }
        if from != w.combat.commands.unit_set.get(u).copied() {
            w.coverage
                .unsupported("driven replay: commands: a unit's replaced set isn't the sim's");
        }
        // A set the sim hasn't seen comes with its first row, right after.
        let mut next = i + 1;
        if !w.combat.commands.sets.contains_key(&to) {
            match trace.records.get(i + 1) {
                Some(row)
                    if obs(row, "uc_state").is_some()
                        && id(&row["set"]) == Some(to)
                        && row["t"].as_i64() == Some(self.tick) =>
                {
                    self.uc_state_row(w, row);
                    next = i + 2;
                    // `RecreateForOwner` ran before `ReplaceCommands`: the new set is the unit's.
                    if sim
                        && w.combat.commands.sets.get(&to).and_then(|x| x.owner.as_deref())
                            != Some(u)
                    {
                        w.coverage.unsupported(
                            "driven replay: commands: a new prediction set's owner isn't the unit replacing its set",
                        );
                    }
                }
                _ => {
                    w.coverage.unsupported(
                        "driven replay: commands: a new command set without its first row",
                    );
                    return next;
                }
            }
        }
        let m = &mut w.combat.commands;
        m.unit_set.insert(u.to_owned(), to);
        let Some(set) = m.sets.get_mut(&to) else { return next };
        // `RecreateForOwner`: a prediction set is the unit's while it holds it.
        if sim {
            set.owner = Some(u.to_owned());
        }
        if set.simulating != sim {
            w.coverage
                .unsupported("driven replay: commands: a replacing set's simulating flag differs");
        }
        next
    }

    /// An end made outside base lifecycle code (class code, the movement agent, the AI): an input.
    fn uc_input_end(&mut self, w: &mut World, r: &J) {
        let (Some(c), Some(result), Some(_), Some(Some(origin))) = (
            id(&r["c"]),
            r["res"].as_u64().and_then(|x| u8::try_from(x).ok()).filter(|x| *x <= uc::res::SUCCESS),
            r["raise"].as_bool(),
            opt_str(r, "origin"),
        ) else {
            w.coverage.unsupported("driven replay: a uc_end record it can't read");
            return;
        };
        // Outside any entry, the end is placed by its caller (the movement agent's interrupt).
        if !self.uc_in_entry() {
            match phase_of(origin) {
                Some(p) => self.place(w, p, "a command's end"),
                None => w.coverage.unsupported(
                    "driven replay: commands: an end from a source the replay can't place",
                ),
            }
        }
        match w.combat.commands.cmds.get_mut(&c) {
            Some(x) if !x.s.finished => {
                x.s.finished = true;
                x.s.result = result;
                self.out.commands_input_ends += 1;
            }
            Some(_) => {
                w.coverage.unsupported("driven replay: commands: an end of a finished command")
            }
            None => w.coverage.unsupported("driven replay: commands: an end of an unknown command"),
        }
    }

    /// A running-list call outside base lifecycle code (in class code: an overridden `OnRun`; the
    /// movement agent's interrupt): an input.
    fn uc_input_rc(&mut self, w: &mut World, r: &J) {
        let to_len = |v: &J| v.as_u64().and_then(|x| usize::try_from(x).ok());
        let (Some(c), Some(op), Some(b), Some(a), Some(current)) = (
            id(&r["c"]),
            r["op"].as_str().filter(|o| *o == "add" || *o == "remove"),
            to_len(&r["b"]),
            to_len(&r["a"]),
            r["current"].as_bool(),
        ) else {
            w.coverage.unsupported("driven replay: a uc_rc record it can't read");
            return;
        };
        if !current {
            w.coverage.unsupported("driven replay: commands: a running-list call of another turn");
            return;
        }
        if uc::current_turn(w).is_none() {
            w.coverage.unsupported("driven replay: commands: a running-list call without a turn");
            return;
        }
        if !w.combat.commands.cmds.contains_key(&c) {
            w.coverage
                .unsupported("driven replay: commands: a running-list call of an unknown command");
            return;
        }
        let l = &mut w.combat.commands.running;
        if b != l.len() {
            w.coverage.unsupported(
                "driven replay: commands: the running list's length differs from the recording's",
            );
            return;
        }
        if op == "add" {
            // `AddRunningCommand` adds a command whose executor is in combat.
            let exec = w.combat.commands.cmds[&c].s.executor.clone();
            let Some(e) = exec.as_deref().and_then(|e| w.by_id.get(e).copied()) else {
                w.coverage.unsupported(
                    "driven replay: commands: a running-list add of an unknown executor",
                );
                return;
            };
            let added = crate::combat::is_in_combat(w, e);
            let l = &mut w.combat.commands.running;
            if added {
                l.push(c);
            }
            if a != l.len() {
                w.coverage.unsupported(
                    "driven replay: commands: the running list's length differs from the recording's",
                );
            }
        } else {
            if let Some(p) = l.iter().position(|x| *x == c) {
                l.remove(p);
            }
            if a != l.len() {
                w.coverage.unsupported(
                    "driven replay: commands: the running list's length differs from the recording's",
                );
            }
        }
    }

    /// Whether lifecycle code is running (an entry's records are being pulled).
    fn uc_in_entry(&self) -> bool {
        self.uc_active > 0
    }

    /// The next leaf record, of `f`, with exactly its writer's fields (`payload` besides the
    /// envelope `Emit` writes, on the main thread).
    pub(super) fn uc_leaf_record(&mut self, w: &mut World, f: &str, payload: &[&str]) -> R<usize> {
        let at = self.uc_pull(w, &format!("the leaf {f}"), |r| obs(r, "uc_v").is_some())?;
        let r = &self.trace.records[at];
        if r["f"] != f {
            let d = describe(r);
            return self.uc_reject(at, format!("the sim reads {f}, the recording has {d}"));
        }
        const BASE: [&str; 5] = ["seq", "k", "t", "ev", "f"];
        if !r.as_object().is_some_and(|o| {
            o.len() == BASE.len() + payload.len()
                && BASE.iter().chain(payload).all(|k| o.contains_key(*k))
        }) {
            return self.uc_reject(at, format!("a leaf {f} with fields its writer doesn't write"));
        }
        Ok(at)
    }
}

/// The rule whose engagement handler made a provoke (its subscribers list's type argument, as the
/// recorded caller has it): `RuleAttackRoll` or `RuleAttackWithWeapon`.
fn provoke_rule(r: &J) -> Option<&'static str> {
    let o = r["origin"].as_str()?;
    ["RuleAttackRoll", "RuleAttackWithWeapon"].into_iter().find(|c| {
        o.contains(&format!(
            "OnEventDidTrigger < Kingmaker.PubSubSystem.RulebookSubscribersList`1[[Kingmaker.RuleSystem.Rules.{c},"
        ))
    })
}

/// A path's points (`[[x, y, z], ...]`, finite `f32`s).
pub(super) fn points(v: &J) -> Option<Vec<[f32; 3]>> {
    v.as_array()?
        .iter()
        .map(|p| {
            let a = p.as_array()?;
            let f: Vec<f32> = a
                .iter()
                .map(|x| {
                    #[allow(clippy::cast_possible_truncation)]
                    let f = x.as_f64()? as f32;
                    f.is_finite().then_some(f)
                })
                .collect::<Option<_>>()?;
            let [x, y, z] = f[..] else { return None };
            Some([x, y, z])
        })
        .collect()
}

/// `uc_running`'s row: `-` (no turn: `Some(None)`) or the ids; `None` for anything else.
fn parse_running(s: &str) -> Option<Option<Vec<u32>>> {
    if s == "-" {
        return Some(None);
    }
    if s.is_empty() {
        return Some(Some(Vec::new()));
    }
    s.split(',').map(|x| x.parse().ok()).collect::<Option<Vec<u32>>>().map(Some)
}

impl Inputs for Driver<'_> {
    fn leaf_next(&mut self, w: &mut World, f: &'static str, of: Of) -> R<bool> {
        let r = self.trace.records.get(self.i);
        if !r.is_some_and(|r| obs(r, "uc_v").is_some() && r["f"] == f) {
            let d = r.map_or_else(|| "its end".to_owned(), describe);
            return self
                .uc_reject(self.i, format!("the sim reads {f} next, the recording has {d}"));
        }
        self.leaf(w, f, of)
    }

    fn tgt_write(&mut self, w: &mut World, x: &crate::combat::targets::Write) -> R<()> {
        self.tgt_pull(w, x)
    }

    fn nct_zero(&mut self, w: &mut World, u: &str, site: &'static str) -> R<()> {
        // `UpdateCombatTarget`'s reset follows its target writes (or reads) at once.
        if site == "UpdateCombatTarget"
            && !self
                .trace
                .records
                .get(self.i)
                .is_some_and(|r| obs(r, "ai_nct").is_some() && r["site"] == site)
        {
            let d = self.trace.records.get(self.i).map_or_else(|| "its end".to_owned(), describe);
            return self.uc_reject(
                self.i,
                format!("{site}'s NextCommandTime next, the recording has {d}"),
            );
        }
        let at = self.uc_pull(w, &format!("{site}'s NextCommandTime"), |r| {
            obs(r, "ai_nct").is_some() && r["site"] == site
        })?;
        let r = &self.trace.records[at];
        if !super::engage::fields(r, &["u", "site", "v"])
            || r["u"].as_str() != Some(u)
            || super::engage::float(&r["v"]).map(f32::to_bits) != Some(0f32.to_bits())
        {
            let d = describe(r);
            return self.uc_reject(
                at,
                format!("{site} sets {u}'s NextCommandTime to 0, the recording has {d}"),
            );
        }
        Ok(())
    }

    fn leaf(&mut self, w: &mut World, f: &'static str, of: Of) -> R<bool> {
        let payload: &[&str] = match (&of, f) {
            (Of::Cmd(_), _) => &["c", "v"],
            (Of::Unit(_), _) => &["u", "v"],
            (Of::Pair(..), "untgt") => &["src", "u", "v"],
            (Of::Pair(..), "enemy") => &["u", "o", "v"],
            (Of::Pair(..), _) => &[],
            (Of::CmdUnit(..), _) => &["c", "u", "v"],
        };
        let at = self.uc_leaf_record(w, f, payload)?;
        let r = &self.trace.records[at];
        let ok = match (&of, f) {
            (Of::Cmd(c), _) => id(&r["c"]) == Some(*c),
            (Of::Unit(u), _) => opt_str(r, "u") == Some(u.as_deref()),
            (Of::Pair(a, b), "untgt") => {
                opt_str(r, "src") == Some(a.as_deref()) && opt_str(r, "u") == Some(b.as_deref())
            }
            (Of::Pair(a, b), "enemy") => {
                opt_str(r, "u") == Some(a.as_deref()) && opt_str(r, "o") == Some(b.as_deref())
            }
            (Of::Pair(..), _) => false,
            (Of::CmdUnit(c, u), _) => {
                id(&r["c"]) == Some(*c) && opt_str(r, "u") == Some(u.as_deref())
            }
        };
        match (ok, r["v"].as_bool()) {
            (true, Some(v)) => Ok(v),
            _ => {
                let d = describe(r);
                self.uc_reject(at, format!("the sim reads {f} of {of:?}, the recording has {d}"))
            }
        }
    }

    fn merge(&mut self, w: &mut World, c: u32, into: u32) -> R<(bool, uc::Target, Option<u32>)> {
        let at = self.uc_leaf_record(w, "merge", &["c", "into", "v", "tgt"])?;
        let r = &self.trace.records[at];
        let tgt = r["tgt"].as_str().and_then(uc::Target::parse);
        see_wrapper(w, tgt.as_ref().and_then(|(_, x)| *x));
        match (id(&r["c"]) == Some(c) && id(&r["into"]) == Some(into), r["v"].as_bool(), tgt) {
            (true, Some(v), Some((t, wrapper))) => Ok((v, t, wrapper)),
            _ => self.uc_reject(at, "a TryMergeInto of other commands, or one it can't read"),
        }
    }

    fn saddled(&mut self, w: &mut World, target: Option<&str>) -> R<Option<String>> {
        let at = self.uc_leaf_record(w, "saddled", &["u", "v"])?;
        let r = &self.trace.records[at];
        if opt_str(r, "u") != Some(target) {
            return self.uc_reject(at, "GetSaddledUnit of another unit");
        }
        match opt_str(r, "v") {
            Some(v) => Ok(v.map(str::to_owned)),
            None => self.uc_reject(at, "a saddled record it can't read"),
        }
    }

    fn target_equals(&mut self, w: &mut World, a: &State, b: &State) -> R<bool> {
        let at = self.uc_leaf_record(w, "teq", &["a", "b", "v"])?;
        let r = &self.trace.records[at];
        let same = |k: &str, s: &State| {
            r[k].as_str().and_then(uc::Target::parse) == Some((s.target.clone(), s.target_ref))
        };
        match (same("a", a) && same("b", b), r["v"].as_bool()) {
            (true, Some(v)) => Ok(v),
            _ => {
                self.uc_reject(at, "a TargetWrapper.Equals of other targets, or one it can't read")
            }
        }
    }

    fn custom(&mut self, w: &mut World, c: u32) -> R<bool> {
        let at = self.uc_leaf_record(w, "custom", &["c", "v"])?;
        let r = &self.trace.records[at];
        match (id(&r["c"]) == Some(c), r["v"].as_bool()) {
            (true, Some(v)) => Ok(v),
            _ => self.uc_reject(at, "TryUseCustomHandler of another command"),
        }
    }

    fn fx(&mut self, w: &mut World, f: &'static str, c: u32) -> R<Fx> {
        let at = self.uc_pull(w, &format!("{f} of command {c}"), |r| obs(r, "uc_fx").is_some())?;
        let r = &self.trace.records[at];
        if r["fn"] != f || id(&r["c"]) != Some(c) {
            let d = describe(r);
            return self
                .uc_reject(at, format!("the sim calls {f} of command {c}, the recording has {d}"));
        }
        if r["exc"] != false {
            return self.uc_reject(at, format!("an exception out of {f}"));
        }
        let ret = match &r["ret"] {
            J::Null if r.get("ret").is_some() => None,
            v => match v
                .as_u64()
                .and_then(|x| u8::try_from(x).ok())
                .filter(|x| *x <= uc::res::SUCCESS)
            {
                Some(x) => Some(x),
                None => return self.uc_reject(at, "a uc_fx record it can't read"),
            },
        };
        if (f == "OnAction") != ret.is_some() {
            return self.uc_reject(at, "a uc_fx record's result where it doesn't belong");
        }
        match r["s"].as_str().and_then(State::parse) {
            Some(s) if s.executor.as_ref().is_some_and(|e| !is_unit(w, e)) => {
                self.uc_reject(at, "class code left a command of an unknown unit")
            }
            Some(s) => {
                // A UnitAttack's class state: with the attacks observer, and only then.
                let ua = match r.get("ua") {
                    None => None,
                    Some(J::String(x)) => Some(x.clone()),
                    Some(_) => return self.uc_reject(at, "a uc_fx record whose ua it can't read"),
                };
                let attack = w.combat.commands.attacks
                    && w.combat.commands.get(c).is_some_and(|x| x.class == "UnitAttack");
                if attack != ua.is_some() {
                    return self
                        .uc_reject(at, "a uc_fx record whose UnitAttack state (ua) doesn't fit");
                }
                let own = w
                    .combat
                    .commands
                    .get(c)
                    .is_some_and(|x| uc::move_to::ported(w, x) || uc::aoo::ported(w, x));
                if ua.is_some() {
                    self.out.attacks_calls += 1;
                } else if own {
                    self.out.approach_class += 1;
                } else {
                    self.out.commands_fx += 1;
                }
                // A target wrapper first seen here (WrathOracle numbers them as it first sees them).
                let fresh = s.target_ref.is_some_and(|x| x > w.combat.commands.max_wrapper);
                see_wrapper(w, s.target_ref);
                Ok(Fx { s, ret, ua, fresh })
            }
            None => self.uc_reject(at, "a uc_fx record it can't read"),
        }
    }

    fn ended(&mut self, w: &mut World, c: u32, result: u8, raise: bool) -> R<()> {
        let at =
            self.uc_pull(w, &format!("the end of command {c}"), |r| obs(r, "uc_end").is_some())?;
        let r = &self.trace.records[at];
        if id(&r["c"]) != Some(c)
            || r["res"].as_u64() != Some(u64::from(result))
            || r["raise"].as_bool() != Some(raise)
            || r["ctx"] != "base"
            || opt_str(r, "origin") != Some(None)
        {
            let d = describe(r);
            return self.uc_reject(
                at,
                format!("the sim ends command {c} ({result}), the recording has {d}"),
            );
        }
        self.out.commands_ends += 1;
        Ok(())
    }

    fn running_call(&mut self, w: &mut World, c: u32, add: bool, before: usize) -> R<()> {
        let op = if add { "add" } else { "remove" };
        let at = self.uc_pull(w, &format!("the running list's {op} of command {c}"), |r| {
            obs(r, "uc_rc").is_some()
        })?;
        let r = &self.trace.records[at];
        let len = w.combat.commands.running.len();
        if id(&r["c"]) != Some(c)
            || r["op"] != op
            || r["current"] != true
            || r["ctx"] != "base"
            || r["b"].as_u64().and_then(|b| usize::try_from(b).ok()) != Some(before)
            || r["a"].as_u64().and_then(|a| usize::try_from(a).ok()) != Some(len)
        {
            let d = describe(r);
            return self.uc_reject(
                at,
                format!("the sim's running list ({op} {c}, {len}), the recording has {d}"),
            );
        }
        Ok(())
    }

    fn tick_command(&mut self, w: &mut World, c: u32) -> R<bool> {
        let at = self.uc_pull(w, &format!("TickCommand({c})"), |r| obs(r, "uc_tc").is_some())?;
        let r = &self.trace.records[at];
        match (id(&r["c"]) == Some(c), r["canmove"].as_bool()) {
            (true, Some(v)) => Ok(v),
            _ => {
                let d = describe(r);
                self.uc_reject(at, format!("the sim ticks command {c}, the recording has {d}"))
            }
        }
    }

    fn spend(&mut self, w: &mut World, u: EntityIdx, ty: u8, full: bool, since: f32) -> R<()> {
        let at = self.uc_pull(w, "a command's SpendAction", |r| {
            obs(r, "cd_spend").is_some() && from_controller(r)
        })?;
        let trace = self.trace;
        let r = &trace.records[at];
        let tsc = r["tsc"].as_str().and_then(|x| x.parse::<f32>().ok());
        if r["u"].as_str() != Some(w.entity(u).unique_id.as_str())
            || r["type"].as_i64() != Some(i64::from(ty))
            || r["full"].as_bool() != Some(full)
            || tsc.map(f32::to_bits) != Some(since.to_bits())
        {
            let d = describe(r);
            return self.uc_reject(
                at,
                format!("the sim's SpendAction ({ty}, {full}, {since}), the recording has {d}"),
            );
        }
        self.uc_spend_ok = true;
        self.cd_record(w, "cd_spend", r);
        self.uc_spend_ok = false;
        self.out.commands_spends += 1;
        Ok(())
    }

    fn nested_run(&mut self, w: &mut World, set: u32, c: u32, from_queue: bool) -> R<()> {
        let at =
            self.uc_pull(w, "a nested Run", |r| obs(r, "uc_run").is_some() && r["nested"] == true)?;
        let r = &self.trace.records[at];
        let st = &w.combat.commands.sets[&set];
        if id(&r["set"]) != Some(set)
            || id(&r["c"]) != Some(c)
            || r["fq"].as_bool() != Some(from_queue)
            || r["ncq"].as_bool() != Some(false)
            || opt_id(r, "prev") != Some(st.prev)
            || r["group"].as_bool() != Some(st.group)
            || opt_str(r, "u") != Some(st.owner.as_deref())
            || !matches!(opt_str(r, "origin"), Some(Some(_)))
        {
            return self.uc_reject(at, "a nested Run of another command, or one it can't read");
        }
        self.tgt_run_push(at);
        Ok(())
    }

    fn nested_run_end(&mut self, w: &mut World, set: u32, c: u32) -> R<()> {
        self.tgt_run_pop();
        let at = self.uc_pull(w, "a nested Run's end", |r| obs(r, "uc_run_end").is_some())?;
        let r = &self.trace.records[at];
        if id(&r["set"]) != Some(set) || id(&r["c"]) != Some(c) || r["exc"] != false {
            return self.uc_reject(at, "a nested Run's end of another command, or an exception");
        }
        Ok(())
    }
    fn leaf_int(&mut self, w: &mut World, f: &'static str, c: u32) -> R<i64> {
        let at = self.uc_leaf_record(w, f, &["c", "v"])?;
        let r = &self.trace.records[at];
        // The animation style: a `WeaponAnimationStyle` (0 to 23).
        match (id(&r["c"]) == Some(c), r["v"].as_i64().filter(|v| (0..=23).contains(v))) {
            (true, Some(v)) => Ok(v),
            _ => {
                let d = describe(r);
                self.uc_reject(
                    at,
                    format!("the sim reads {f} of command {c}, the recording has {d}"),
                )
            }
        }
    }

    fn leaf_ids(&mut self, w: &mut World, f: &'static str, c: u32) -> R<Vec<Option<String>>> {
        let at = self.uc_leaf_record(w, f, &["c", "v"])?;
        let r = &self.trace.records[at];
        let v: Option<Vec<Option<String>>> = r["v"].as_array().and_then(|a| {
            a.iter()
                .map(|x| match x {
                    J::Null => Some(None),
                    J::String(s) => Some(Some(s.clone())),
                    _ => None,
                })
                .collect()
        });
        match (id(&r["c"]) == Some(c), v) {
            (true, Some(v)) => Ok(v),
            _ => {
                let d = describe(r);
                self.uc_reject(
                    at,
                    format!("the sim reads {f} of command {c}, the recording has {d}"),
                )
            }
        }
    }

    fn attack_list(&mut self, w: &mut World, c: u32) -> R<uc::attack::List> {
        let at = self.uc_pull(w, &format!("InitAttacks' list of command {c}"), |r| {
            obs(r, "ua_atk").is_some()
        })?;
        let r = &self.trace.records[at];
        if id(&r["c"]) != Some(c) {
            let d = describe(r);
            return self.uc_reject(
                at,
                format!("the sim's InitAttacks of command {c}, the recording has {d}"),
            );
        }
        // `InitAttacksFinalizer`'s fields, on the main thread.
        const KEYS: [&str; 11] =
            ["seq", "k", "t", "ev", "c", "flags", "full", "cfg", "dur", "list", "exc"];
        if !r
            .as_object()
            .is_some_and(|o| o.len() == KEYS.len() && KEYS.iter().all(|k| o.contains_key(*k)))
        {
            return self.uc_reject(at, "a ua_atk record with fields its writer doesn't write");
        }
        if r["exc"] != false {
            return self.uc_reject(at, "an exception out of InitAttacks");
        }
        let attacks = match r.get("list") {
            Some(J::Null) => Some(None),
            Some(J::Array(a)) => a
                .iter()
                .map(|x| x.as_str().and_then(uc::attack::Entry::parse))
                .collect::<Option<Vec<_>>>()
                .map(Some),
            _ => None,
        };
        let flags = r["flags"].as_str().and_then(uc::attack::Flags::parse);
        let duration =
            r["dur"].as_str().and_then(|d| d.parse::<f32>().ok()).filter(|d| d.is_finite());
        match (flags, r["full"].as_bool(), r["cfg"].as_bool(), duration, attacks) {
            (Some(flags), Some(full), Some(configured), Some(duration), Some(attacks)) => {
                // A limb's recorded item: the id the limbs rule (or a first list) showed for it.
                let u = w
                    .combat
                    .commands
                    .get(c)
                    .and_then(|x| x.s.executor.as_deref())
                    .and_then(|e| w.by_id.get(e))
                    .copied();
                for e in attacks.iter().flatten() {
                    if let (Some(uc::attack::Slot::Limb(i)), Some(u)) = (e.slot, u)
                        && let Err(e) = self.bind_limb(w, u, i, &e.item)
                    {
                        return self.uc_reject(at, e);
                    }
                }
                Ok(uc::attack::List { flags, full, configured, duration, attacks })
            }
            _ => self.uc_reject(at, "a ua_atk record it can't read"),
        }
    }

    fn attacks_count(
        &mut self,
        w: &mut World,
        c: u32,
        u: EntityIdx,
        limbs: bool,
    ) -> R<uc::attack::Counts> {
        use crate::combat::counts::{RuleCalculateAttacksCount, RuleCalculateAttacksCountLimbs};
        let class =
            if limbs { "RuleCalculateAttacksCountLimbs" } else { "RuleCalculateAttacksCount" };
        let at = self.attack_root(w, c, class, (ATTACK_T, "InitAttacks"), u, None)?;
        let r = &self.trace.records[at];
        // `new RuleCalculateAttacksCount(Executor)` (`...Limbs`) at its push: every field the
        // constructor's (no forced weapon, filter or iterative natural attacks; an empty result).
        let zero = json!({"AdditionalAttacks": 0, "PenalizedAttacks": 0, "HasteAttacks": 0,
            "FlawlessAttacks": false, "IsTwoHanded": false, "MainAttacks": 0});
        let mut want = base_fields();
        want.insert("ForcePrimaryAttackWeapon".into(), J::Null);
        want.insert("WeaponFilter".into(), J::Null);
        if limbs {
            want.insert("Slots".into(), json!([]));
            want.insert("AttacksCounts".into(), json!([]));
        } else {
            want.insert("ForceIterativeNaturalAttacks".into(), J::Bool(false));
            want.insert("Result".into(), json!({"PrimaryHand": zero, "SecondaryHand": zero}));
        }
        if r["fields"].as_object() != Some(&want) {
            return self
                .uc_reject(at, format!("a {class} of UnitAttack with fields it doesn't set"));
        }
        let end = self.attack_span(w, at, None, None)?;
        self.i = end + 1;
        self.out.roots += 1;
        self.out.attacks_rules += 1;
        *self.out.by_origin.entry("command (UnitAttack)").or_default() += 1;
        let cx = self.cx.clone();
        if limbs {
            // The counted limbs' slots: `Slots` only grows (`OnTrigger` adds each counted limb), so
            // every recorded value is a prefix of the sim's counted limbs, each a whole slot
            // snapshot; its last value names the game's ids of their items.
            let id = &self.trace.records[at]["id"];
            let counted = uc::attack::counted_limbs(w, u);
            let mut ids: Vec<String> = Vec::new();
            let mut damaged = false;
            for r in &self.trace.records[at..=end] {
                if &r["id"] != id {
                    continue;
                }
                for p in r.get("changes").and_then(J::as_array).into_iter().flatten() {
                    if p[0] != "Slots" {
                        continue;
                    }
                    let Some(a) = p[1].as_array().filter(|a| a.len() <= counted.len()) else {
                        damaged = true;
                        continue;
                    };
                    let mut now = Vec::new();
                    for (x, i) in a.iter().zip(&counted) {
                        let item = x["m_ItemRef"]["$entity"].as_str().unwrap_or_default();
                        let slot = uc::attack::Slot::Limb(*i);
                        if item.is_empty() || !slot_snapshot_ok(w, u, slot, item, x, None) {
                            damaged = true;
                        }
                        now.push(item.to_owned());
                    }
                    if !now.starts_with(&ids) {
                        damaged = true;
                    }
                    ids = now;
                }
            }
            if damaged || ids.len() != counted.len() {
                return self.uc_reject(
                    at,
                    "RuleCalculateAttacksCountLimbs' slots don't fit the sim's limbs",
                );
            }
            for (i, item) in counted.iter().zip(&ids) {
                if let Err(e) = self.bind_limb(w, u, *i, item) {
                    return self.uc_reject(at, e);
                }
            }
            let f = &self.trace.records[at]["fields"];
            let mut rule = RuleCalculateAttacksCountLimbs::from_snapshot(w, u, f);
            crate::combat::fire::fire(w, &cx, &mut rule, RuleCalculateAttacksCountLimbs::result);
            Ok(uc::attack::Counts::Limbs(rule.counts))
        } else {
            let mut rule = RuleCalculateAttacksCount::new(u);
            crate::combat::fire::fire(w, &cx, &mut rule, RuleCalculateAttacksCount::result);
            Ok(uc::attack::Counts::Hands(rule.primary, rule.secondary))
        }
    }

    fn path(&mut self, w: &mut World, f: &'static str, c: u32) -> R<Option<Vec<[f32; 3]>>> {
        let at = self.uc_leaf_record(w, f, &["c", "v"])?;
        let r = &self.trace.records[at];
        if id(&r["c"]) != Some(c) {
            let d = describe(r);
            return self
                .uc_reject(at, format!("the sim reads {f} of command {c}, the recording has {d}"));
        }
        match &r["v"] {
            J::Null => Ok(None),
            v => match points(v) {
                Some(p) => Ok(Some(p)),
                None => self.uc_reject(at, format!("a {f} record it can't read")),
            },
        }
    }

    fn threat(
        &mut self,
        w: &mut World,
        c: u32,
        u: EntityIdx,
        hand: Option<uc::attack::Slot>,
    ) -> R<Option<String>> {
        let at = self.uc_leaf_record(w, "threat", &["c", "v"])?;
        let r = &self.trace.records[at];
        if id(&r["c"]) != Some(c) {
            let d = describe(r);
            return self.uc_reject(
                at,
                format!("the sim reads threat of command {c}, the recording has {d}"),
            );
        }
        let recorded = match opt_str(r, "v") {
            Some(None) => None,
            Some(Some(v)) => match uc::attack::SlotFields::parse(v) {
                Some(x) => Some(x),
                None => return self.uc_reject(at, "a threat record it can't read"),
            },
            None => return self.uc_reject(at, "a threat record it can't read"),
        };
        match (hand, recorded) {
            (None, None) => Ok(None),
            (Some(h), Some(x)) if x.slot == Some(h) && uc::attack::slot_holds(w, u, h, &x) => {
                if let uc::attack::Slot::Limb(i) = h
                    && let Err(e) = self.bind_limb(w, u, i, &x.item)
                {
                    return self.uc_reject(at, e);
                }
                Ok(Some(x.item))
            }
            (h, x) => self.uc_reject(
                at,
                format!("the sim's threat hand of command {c} is {h:?}, the recording's {x:?}"),
            ),
        }
    }

    fn call(&mut self, w: &mut World, call: &uc::approach::Call) -> R<()> {
        use uc::approach::Call;
        let at = self.uc_pull(w, &format!("{call:?}"), |r| obs(r, "uc_call").is_some())?;
        let r = &self.trace.records[at];
        // Each call's fields, exactly as its writer has them.
        let keys: &[&str] = match r["fn"].as_str() {
            Some("moveto") => &["fn", "u", "c", "p", "r", "mr", "tv", "tu", "xz"],
            Some("follow") => &["fn", "u", "c", "v", "forced", "r"],
            Some("stop") => &["fn", "u", "c"],
            _ => &[],
        };
        const BASE: [&str; 4] = ["seq", "k", "t", "ev"];
        if keys.is_empty()
            || !r.as_object().is_some_and(|o| {
                o.len() == BASE.len() + keys.len()
                    && BASE.iter().chain(keys).all(|k| o.contains_key(*k))
            })
        {
            return self.uc_reject(at, "a uc_call record with fields its writer doesn't write");
        }
        let num =
            |k: &str| r[k].as_str().and_then(|v| v.parse::<f32>().ok()).filter(|v| v.is_finite());
        let same = |want: Option<f32>, got: Option<f32>| match (want, got) {
            (_, None) => false,
            (Some(a), Some(b)) => a.to_bits() == b.to_bits(),
            (None, Some(_)) => true,
        };
        let ok = match call {
            Call::MoveTo { c, unit, point, radius, max_radius, target, xz } => {
                let p = points(&json!([r["p"].clone()])).and_then(|v| v.first().copied());
                r["fn"] == "moveto"
                    && opt_str(r, "u") == Some(Some(unit.as_str()))
                    && opt_id(r, "c") == Some(Some(*c))
                    && p.is_some_and(|p| {
                        point.is_none_or(|q| {
                            q.iter().zip(&p).all(|(a, b)| a.to_bits() == b.to_bits())
                        })
                    })
                    && same(*radius, num("r"))
                    && same(*max_radius, num("mr"))
                    && r["tv"] == true
                    && opt_str(r, "tu") == Some(Some(target.as_str()))
                    && r["xz"].as_bool() == Some(*xz)
            }
            Call::Follow { c, unit, path, forced, radius } => {
                let p = points(&r["v"]);
                r["fn"] == "follow"
                    && opt_str(r, "u") == Some(Some(unit.as_str()))
                    && opt_id(r, "c") == Some(Some(*c))
                    && p.is_some_and(|p| {
                        p.len() == path.len()
                            && p.iter().zip(path).all(|(a, b)| {
                                a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits())
                            })
                    })
                    && r["forced"].as_bool() == Some(*forced)
                    && same(*radius, num("r"))
            }
            Call::Stop { unit, c } => {
                r["fn"] == "stop"
                    && opt_str(r, "u") == Some(Some(unit.as_str()))
                    && opt_id(r, "c") == Some(*c)
            }
        };
        if !ok {
            let d = describe(r);
            return self.uc_reject(at, format!("the sim calls {call:?}, the recording has {d}"));
        }
        self.out.approach_calls += 1;
        Ok(())
    }

    fn approach_draw(&mut self, w: &mut World, c: u32) -> R<f32> {
        let site = crate::rng::sites::TICK_APPROACHING;
        let ids = self.uc_approach_sites.clone();
        let at = self.uc_pull(w, &format!("TickApproaching's draw for command {c}"), |r| {
            r["k"] == "draw"
                && (r["site"].as_i64().is_some_and(|s| ids.contains(&s))
                    || r["key"].as_str() == Some(site.0))
        })?;
        let r = &self.trace.records[at];
        // A simulation draw outside rules, of `Range(0.2f, 0.6f)` at the site, as `Rng` writes it.
        // (A keyed draw is a global hook's, of site -1: never this site's.)
        let ok = self.trace.site_key(r).as_deref() == Some(site.0)
            && r.get("key").is_none()
            && r["c"] == "sim"
            && r["m"] == "Range_ff"
            && r["rule"] == 0
            && r["h"] == 0
            && r.get("ctx").is_none()
            && r.get("forced").is_some() == self.replay_mode
            && draw_ok(r, &self.trace.sites)
            && r["a"].as_array().is_some_and(|a| {
                #[allow(clippy::cast_possible_truncation)]
                let f = |x: &J| x.as_f64().map(|v| (v as f32).to_bits());
                a.len() == 2
                    && f(&a[0]) == Some(0.2f32.to_bits())
                    && f(&a[1]) == Some(0.6f32.to_bits())
            });
        if !ok {
            let d = describe(r);
            return self.uc_reject(
                at,
                format!("TickApproaching's draw for command {c}: the recording has {d}"),
            );
        }
        let v = w.rng.range_float(site, 0.2, 0.6);
        #[allow(clippy::cast_possible_truncation)]
        let rec = r["v"].as_f64().map(|x| x as f32);
        if rec.map(f32::to_bits) != Some(v.to_bits()) {
            return self.uc_reject(
                at,
                format!("TickApproaching's draw {v} differs from the recorded {rec:?}"),
            );
        }
        self.out.approach_draws += 1;
        Ok(v)
    }

    fn attack_rule(&mut self, w: &mut World, c: u32, spec: &uc::attack::Spec) -> R<bool> {
        // A `UnitAttack`'s from `TriggerAttackRule`, an attack of opportunity's from its `OnAction`.
        let (t_class, frame) = match spec.opportunity {
            None => (ATTACK_T, "TriggerAttackRule"),
            Some(_) => (AOO_T, "OnAction"),
        };
        let at = self.attack_root(
            w,
            c,
            "RuleAttackWithWeapon",
            (t_class, frame),
            spec.initiator,
            Some(spec.target),
        )?;
        let trace = self.trace;
        let r = &trace.records[at];
        let Some(f) = r["fields"].as_object() else {
            return self.uc_reject(at, "an attack root without its fields");
        };
        // `new RuleAttackWithWeapon(Executor, Target, attack.Weapon, attack.AttackBonusPenalty)` with
        // the fields `TriggerAttackRule` sets: every field at its push is the sim's.
        let mut want = base_fields();
        for (k, v) in [
            ("Target", json!({"$unit": w.entity(spec.target).unique_id})),
            ("AttackBonusPenalty", json!(spec.penalty)),
            ("WeaponStats", json!({"$rule": 0, "class": "RuleCalculateWeaponStats"})),
            ("LaunchedProjectiles", json!([])),
            ("MeleeDamage", J::Null),
            ("AttackRoll", J::Null),
            ("ResolveRules", json!([])),
            ("IsFirstAttack", json!(spec.is_first_attack)),
            ("IsRend", json!(spec.is_rend)),
            ("IsAttackOfOpportunity", json!(spec.opportunity.is_some())),
            ("IsFullAttack", json!(spec.is_full_attack)),
            ("IsCharge", json!(spec.is_charge)),
            ("AutoHit", json!(false)),
            ("AutoCriticalThreat", json!(false)),
            ("AutoCriticalConfirmation", json!(false)),
            (
                "Maximized",
                json!({"m_Value": false, "Source": null, "Metamagic": 0, "Reason": null, "Value": false}),
            ),
            ("ReplaceTarget", json!(false)),
            ("ForceFlatFooted", json!(spec.opportunity.unwrap_or(false))),
            ("ExtraAttack", json!(false)),
            ("NewTarget", J::Null),
            ("AttackNumber", json!(spec.attack_number)),
            ("AttacksCount", json!(spec.attacks_count)),
            ("SkipAnimation", json!(false)),
            ("SkipMainDamage", json!(false)),
            ("ReplaceAttackType", json!(false)),
            ("AttackTypeReplacement", json!(0)),
        ] {
            want.insert(k.into(), v);
        }
        let differ: Vec<&String> = want
            .keys()
            .chain(f.keys())
            .filter(|k| !matches!(k.as_str(), "Weapon" | "WeaponSlot") && want.get(*k) != f.get(*k))
            .collect();
        // The weapon: the slot's (by item and blueprint; a limb's items get fresh ids at load, so
        // by blueprint), and the slot: the attacker's, holding the sim's item, as active.
        let (weapon_id, limb) = attack_weapon_id(w, spec);
        let bp = w.item(spec.weapon).and_then(|x| x.blueprint).map(|g| g.to_string());
        let wp = &f.get("Weapon").cloned().unwrap_or(J::Null);
        let weapon_ok = wp.as_object().is_some_and(|o| o.len() == 2)
            && wp["blueprint"].as_str() == bp.as_deref()
            && wp["$item"].as_str() == Some(weapon_id);
        // (An attack of opportunity's rule has no slot.)
        let slot_ok = f.get("WeaponSlot").is_some_and(|x| {
            if spec.opportunity.is_some() {
                return x.is_null();
            }
            let weapon_item = f.get("Weapon").and_then(|v| v["$item"].as_str());
            slot_snapshot_ok(
                w,
                spec.initiator,
                spec.slot,
                &spec.item,
                x,
                Some((spec.weapon, weapon_item)),
            )
        });
        if !differ.is_empty() || !weapon_ok || !slot_ok {
            return self.uc_reject(
                at,
                format!(
                    "the sim's attack {spec:?} differs from the recorded one (fields {differ:?}, weapon {weapon_ok}, slot {slot_ok})"
                ),
            );
        }
        if spec.is_rend {
            return self.uc_reject(at, "a rend attack (RuleAttackWithWeapon.IsRend: not modelled)");
        }
        let Some(wp) = crate::combat::weapon::Weapon::of(w, spec.weapon) else {
            return self.uc_reject(at, "an attack whose weapon the sim can't resolve");
        };
        let limb = limb.then_some(spec.item.as_str());
        let item = f.get("Weapon").and_then(|v| v["$item"].as_str()).map(str::to_owned);
        let end = self.attack_span(w, at, limb, item.as_deref())?;
        self.i = end + 1;
        self.out.roots += 1;
        if spec.opportunity.is_some() {
            self.out.approach_rules += 1;
        } else {
            self.out.attacks_rules += 1;
        }
        let origin = if spec.opportunity.is_some() {
            "command (UnitAttackOfOpportunity)"
        } else {
            "command (UnitAttack)"
        };
        *self.out.by_origin.entry(origin).or_default() += 1;
        let (a, t) = (spec.initiator, spec.target);
        if let Some(env) = super::env_in(trace, at, end, a, t, w) {
            w.combat.env.push(((a, t), env));
        }
        let mut rule = crate::combat::attack::RuleAttackWithWeapon::new(a, t, wp, spec.penalty);
        rule.is_first_attack = spec.is_first_attack;
        rule.is_full_attack = spec.is_full_attack;
        rule.is_charge = spec.is_charge;
        rule.attack_number = spec.attack_number;
        rule.attacks_count = spec.attacks_count;
        rule.is_attack_of_opportunity = spec.opportunity.is_some();
        rule.force_flat_footed = spec.opportunity.unwrap_or(false);
        let before = w.combat.pending_resolves.len();
        let cx = self.cx.clone();
        crate::combat::fire::fire(w, &cx, &mut rule, |_| None);
        // The resolves this attack launched, by the recorded rule identity.
        let rid = trace.records[at]["id"].as_i64();
        for p in w.combat.pending_resolves.iter_mut().skip(before) {
            p.source_id = rid;
        }
        // (`AllAttacks[i].Rule.AttackRoll`, for the impatience controller's damage handler.)
        if let Some(seq) = rule.attack_roll_seq
            && w.combat.impatience.on
        {
            w.combat.impatience.rolls.entry(c).or_default().push(seq);
        }
        self.imp_span(w, at, end);
        match rule.attack_hit {
            Some(hit) => Ok(hit),
            None => Err(Desync(format!("command {c}'s attack rule made no roll"))),
        }
    }
}

/// Binds (or checks) the game's id of the item in `u`'s limb `i`: an id the sim knows is that
/// item's (a limb item loaded with its id), any other a fresh one, never another item's, and
/// the same while the limb holds that item (a limb granted again holds a new item).
fn bind_limb(
    ids: &mut HashMap<EntityIdx, String>,
    w: &World,
    u: EntityIdx,
    i: usize,
    item: &str,
) -> Result<(), String> {
    let Some(held) = uc::attack::slot_fields(w, u, uc::attack::Slot::Limb(i)).and_then(|(x, _)| x)
    else {
        return Err(format!("limb {i}'s recorded item {item} in an empty limb"));
    };
    if w.by_id.get(item).is_some_and(|e| *e != held) {
        return Err(format!("limb {i}'s recorded item {item} is another entity"));
    }
    if ids.iter().any(|(k, v)| *k != held && v == item) {
        return Err(format!("limb {i}'s recorded item {item} is another item's"));
    }
    match ids.get(&held) {
        Some(id) if id != item => Err(format!("limb {i}'s recorded item {item} isn't {id}")),
        Some(_) => Ok(()),
        None => {
            ids.insert(held, item.to_owned());
            Ok(())
        }
    }
}

/// The recorded id an attack's weapon must have, and whether it's a limb's own item: a limb's
/// weapon is the item it holds (by the id `InitAttacks` recorded: limb items get fresh ids at load),
/// unless the limb is inactive and `WeaponSlot.MaybeWeapon` gives the owner's empty hand, which (like
/// a hand's weapon) has the sim's id.
fn attack_weapon_id<'a>(w: &'a World, spec: &'a uc::attack::Spec) -> (&'a str, bool) {
    let limb = matches!(spec.slot, uc::attack::Slot::Limb(_))
        && uc::attack::slot_fields(w, spec.initiator, spec.slot).and_then(|(x, _)| x)
            == Some(spec.weapon);
    if limb {
        (spec.item.as_str(), true)
    } else {
        (w.entity(spec.weapon).unique_id.as_str(), false)
    }
}

/// A `WeaponSlot` push snapshot: every field there and typed, and what the sim models its own:
/// the attacker's slot, holding the sim's item (a limb's items get fresh ids at load: an item,
/// then), as active, not a fake item; `Active`, `HideEquipmentEntity` and `IsItemFake` their
/// fields; a limb's parent fact the sim's. `slot` is `u`'s, `item` the recorded id of its item;
/// with `weapon`, the attack's weapon and its recorded `$item`.
fn slot_snapshot_ok(
    w: &World,
    u: EntityIdx,
    slot: uc::attack::Slot,
    item: &str,
    x: &J,
    weapon: Option<(EntityIdx, Option<&str>)>,
) -> bool {
    let Some(o) = x.as_object() else { return false };
    // `WeaponSlot`'s fields, and a hand's (`HandSlot`) `IsDirty`.
    const KEYS: [&str; 16] = [
        "m_ItemRef",
        "m_Active",
        "m_HideEquipmentEntity",
        "Owner",
        "m_DeactivateFlag",
        "Lock",
        "m_FakeItem",
        "m_IsItemFake",
        "BuiltIn",
        "ParentFact",
        "m_NaturalFxSnapMap",
        "m_FxSnapMap",
        "IsItemFake",
        "Active",
        "HideEquipmentEntity",
        "Disabled",
    ];
    let limb = match slot {
        uc::attack::Slot::Limb(i) => Some(i),
        _ => None,
    };
    if o.len() != KEYS.len() + usize::from(limb.is_none())
        || !KEYS.iter().all(|k| o.contains_key(*k))
        || limb.is_none() != o.get("IsDirty").is_some_and(J::is_boolean)
    {
        return false;
    }
    let Some((held, active)) = uc::attack::slot_fields(w, u, slot) else {
        return false;
    };
    let owner = &w.entity(u).unique_id;
    // The slot's item: the sim's (a limb's items get fresh ids at load: the one `InitAttacks`
    // recorded, then), an item reference, and the attack's weapon when the slot holds one.
    let item_ok = match (&o["m_ItemRef"], held) {
        (J::Null, None) => item.is_empty(),
        (J::Object(r), Some(i)) => {
            let id = r.get("$entity").and_then(J::as_str);
            r.len() == 2
                && r.get("class").and_then(J::as_str) == Some("ItemEntity")
                && id == Some(item)
                && (limb.is_some() || id == Some(w.entity(i).unique_id.as_str()))
                // The weapon is the active slot's item, or (shield bash) its shield's weapon component.
                && weapon.is_none_or(|(wpn, weapon_item)| {
                    o.get("m_Active") != Some(&J::Bool(true))
                        || weapon_item == id
                        || (w.item(i).and_then(|x| x.weapon_component) == Some(wpn)
                            && weapon_item == Some(w.entity(wpn).unique_id.as_str()))
                })
        }
        _ => false,
    };
    // `CountableFlag`: a `short` count, `Value` exactly `m_Count > 0`.
    let flag = |v: &J| {
        v.as_object().is_some_and(|f| {
            let count = f.get("m_Count").and_then(J::as_i64).filter(|c| i16::try_from(*c).is_ok());
            f.len() == 3
                && count.is_some()
                && f.get("Count") == f.get("m_Count")
                && f.get("Value") == count.map(|c| J::Bool(c > 0)).as_ref()
        })
    };
    let snap = |v: &J| match v {
        J::Null => true,
        J::Object(m) => {
            // `WeaponParticlesSnapMap` (no subclass), by `Snap.Value`'s runtime type name.
            m.len() == 2
                && m.get("$unity").and_then(J::as_str) == Some("WeaponParticlesSnapMap")
                && m.get("name").is_some_and(J::is_string)
        }
        _ => false,
    };
    let parent = o["ParentFact"].as_object().is_some_and(|p| {
        let parent = limb.and_then(|i| w.unit(u)?.body.limb_parents.get(&i).copied());
        p.len() == 4
            && match parent {
                None => {
                    p.get("IsNull") == Some(&J::Bool(true))
                        && p.get("m_Proxy") == Some(&J::Null)
                        && p.get("EntityId") == Some(&J::Null)
                        && p.get("FactId") == Some(&J::Null)
                }
                // The proxy, when resolved, is the owner's.
                Some(fact) => {
                    p.get("IsNull") == Some(&J::Bool(false))
                        && p.get("EntityId").and_then(J::as_str) == Some(owner.as_str())
                        && p.get("FactId").and_then(J::as_str)
                            == Some(w.fact(fact).unique_id.as_str())
                        && match p.get("m_Proxy") {
                            Some(J::Null) => true,
                            Some(J::Object(m)) => {
                                m.len() == 3
                                    && m.get("Entity") == Some(&json!({"$unit": owner}))
                                    && m.get("Id").and_then(J::as_str) == Some(owner.as_str())
                                    && m.get("IsDisposed") == Some(&J::Bool(false))
                            }
                            _ => false,
                        }
                }
            }
    });
    item_ok
        && o["m_Active"].as_bool() == Some(active)
        && o["Active"] == o["m_Active"]
        && o["m_HideEquipmentEntity"].is_boolean()
        && o["HideEquipmentEntity"] == o["m_HideEquipmentEntity"]
        && o["Owner"] == json!({"$unit": owner})
        && flag(&o["m_DeactivateFlag"])
        && o["Disabled"] == o["m_DeactivateFlag"]["Value"]
        && flag(&o["Lock"])
        && o["m_FakeItem"] == J::Null
        && o["m_IsItemFake"] == false
        && o["IsItemFake"] == false
        && o["BuiltIn"].is_boolean()
        && parent
        && snap(&o["m_NaturalFxSnapMap"])
        && snap(&o["m_FxSnapMap"])
}

/// The fields every rule has at its push (`RulebookEvent`'s, untriggered).
fn base_fields() -> serde_json::Map<String, J> {
    let mut m = serde_json::Map::new();
    for (k, v) in [
        ("m_Triggered", J::Bool(false)),
        ("m_TemporaryModifiers", J::Null),
        ("m_ModifiableBonus", J::Null),
        ("m_CustomData", J::Null),
        ("DisableBattleLogSelf", J::Bool(false)),
        ("DisableBattleLog", J::Bool(false)),
        ("IsTriggered", J::Bool(false)),
        ("AllBonuses", json!([])),
    ] {
        m.insert(k.into(), v);
    }
    m
}

/// A root rule's span (`span[0]` the root), as WrathOracle's `Rules.cs` writes it: every record of
/// frame `tick`, in each rule's order (its push with its snapshot, `mod_before` handlers, `about`,
/// `about` handlers, `trig`, its own computation with nested rules, `trig_end`, `mod_after` then `did`
/// handlers, `did`, `rule_end` with its bonuses), each step once; handlers on one global stack (an
/// ancestor's stays open across a rule it triggers), each of the innermost rule with its identity
/// (a fact's: the sim's, of that blueprint and owner), closed in order; nested rules pushed by the
/// innermost rule during its computation or a handler of it (`parent`, `depth`, `handler`: the open
/// handler, `context_parent`: an open rule); draws and mutations of the innermost rule in the open
/// handler, a draw's context its rule's class, units (stable ids) and ordinal; environments of the
/// innermost rule, of a kind the replay reads, with every field typed; diffs of the rule's own
/// fields, each value of its field's kind. No lifecycle record, other observation or exception.
fn span_ok(span: &[J], cx: &SpanCx<'_>) -> bool {
    let SpanCx { tick, w, replay, sites, limb, rules, root_reason, achievements, impatience } = *cx;
    struct Frame {
        id: i64,
        class: String,
        initiator: J,
        target: J,
        fields: serde_json::Map<String, J>,
        /// 0 pushed, 1 about, 2 trig, 3 trig_end, 4 did.
        stage: u8,
        /// Its environment records.
        envs: usize,
        /// The bonuses its `rule_bonus` mutations added (value, descriptor, type, stat, fact).
        bonuses: Vec<J>,
        /// Its reason at its push, and whether a `mod_before` handler ran before `about` (which may
        /// assign another).
        push_reason: J,
        before_handlers: bool,
        /// Its reason at `about`.
        reason: J,
        did_handlers: bool,
        draws: i64,
        /// Its `ActivatableAbility` handlers at about and did (identity-free records: counted).
        activatable: [usize; 2],
        /// Its component handlers at about and did (fact id, component name).
        components: [Vec<(String, String)>; 2],
        /// Its global singleton handlers at about and did, by class (achievements apart).
        globals: [HashMap<String, usize>; 2],
        achievements: [HashMap<String, usize>; 2],
        /// Its engagement controller's provokes (one call at most per rule).
        provokes: u8,
    }
    // `Rules.StableId`: the id, or for an id longer than 8 (a unit created in the window, or a GUID
    // id) `new:` and its blueprint; `-` for none.
    let stable = |v: &J| -> Option<String> {
        match v {
            J::Null => Some("-".into()),
            J::Object(o) => {
                let id = o.get("$unit")?.as_str()?;
                if id.len() <= 8 {
                    return Some(id.to_owned());
                }
                let e = *w.by_id.get(id)?;
                Some(format!("new:{}", w.unit(e)?.blueprint?))
            }
            _ => None,
        }
    };
    let frame = |r: &J| -> Option<Frame> {
        let reason = match r.get("reason_at_push") {
            Some(J::Null) => true,
            Some(x) => reason_ok(x, r["id"].as_i64().unwrap_or(0), w, limb, rules),
            None => false,
        };
        // A target event's snapshot has its `Target`, the envelope's (`GetRuleTarget` returns it); no
        // other rule has one.
        let target_event = TARGET_EVENTS.contains(&short_class(r["class"].as_str().unwrap_or("")));
        match r["fields"].get("Target") {
            Some(t) if target_event && Some(t) == r.get("target") => {}
            None if !target_event => {}
            _ => return None,
        }
        let f = Frame {
            id: r["id"].as_i64().filter(|i| *i > 0)?,
            // A rule type seen inside attacks (another is reported: its own computation isn't checked).
            class: r["class"].as_str().filter(|c| RULES.contains(c))?.to_owned(),
            initiator: r.get("initiator").filter(|v| sim_unit_or_null(w, v))?.clone(),
            target: r.get("target").filter(|v| sim_unit_or_null(w, v))?.clone(),
            fields: r["fields"]
                .as_object()
                .filter(|f| {
                    snapshot_ok(f, r["class"].as_str().unwrap_or(""))
                        && bonus_state_ok(f, w)
                        && f.values().all(observed)
                })?
                .clone(),
            stage: 0,
            envs: 0,
            bonuses: Vec::new(),
            push_reason: r.get("reason_at_push").cloned().unwrap_or(J::Null),
            before_handlers: false,
            reason: J::Null,
            did_handlers: false,
            draws: 0,
            activatable: [0, 0],
            components: [Vec::new(), Vec::new()],
            globals: [HashMap::new(), HashMap::new()],
            achievements: [HashMap::new(), HashMap::new()],
            provokes: 0,
        };
        reason.then_some(f)
    };
    let Some(root) = frame(&span[0]).filter(|_| envelope_ok(&span[0])) else { return false };
    // Ids are allocated in order through the window: each new rule's and handler's is above the last.
    let (mut last_rule, mut last_handler) = (root.id, 0_i64);
    let mut frames = vec![root];
    // The global handler stack: (handler, its rule).
    let mut handlers: Vec<(i64, i64)> = Vec::new();
    // The open handlers' classes (with `handlers`).
    let mut handler_classes: Vec<String> = Vec::new();
    let mut seq = span[0]["seq"].as_i64();
    for (n, x) in span.iter().enumerate().skip(1) {
        // Every record carries its trace sequence number, in order.
        let s = x["seq"].as_i64();
        if x["t"].as_i64() != Some(tick) || seq.is_none() || s <= seq {
            return false;
        }
        seq = s;
        let Some(top) = frames.last() else { return false };
        let (rule, stage) = (top.id, top.stage);
        let handler = handlers.last().map_or(0, |h| h.0);
        let own_handler = handlers.last().is_some_and(|h| h.1 == rule);
        let id = x["id"].as_i64();
        let attributed = x["rule"].as_i64() == Some(rule) && x["h"].as_i64() == Some(handler);
        // A step's diff, applied to the innermost rule's fields.
        let step = |frames: &mut Vec<Frame>, want: u8, next: u8| -> bool {
            let ok = id == Some(rule) && stage == want && !own_handler;
            ok && frames.last_mut().is_some_and(|f| {
                f.stage = next;
                apply_diff(&mut f.fields, &x["changes"])
                    && bonus_state_ok(&f.fields, w)
                    && added_agree(&f.fields, &f.bonuses)
                    && (next == 5 || untriggered(&f.fields))
                    && f.fields.get("Target").is_none_or(|t| *t == f.target)
            })
        };
        if !envelope_ok(x) {
            return false;
        }
        let ok = match x["k"].as_str() {
            Some("rule") => {
                let ok = x["parent"].as_i64() == Some(rule)
                    && x["depth"].as_u64().and_then(|d| usize::try_from(d).ok())
                        == Some(frames.len())
                    && x["handler"].as_i64() == Some(handler)
                    // Pushed by an ordinary `Trigger`: its context is its parent (`PreviousEvent`).
                    && x["context_parent"] == x["parent"]
                    && (stage == 2 || own_handler)
                    && id.is_some_and(|i| i > last_rule);
                match frame(x) {
                    Some(f) if ok => {
                        last_rule = f.id;
                        frames.push(f);
                        true
                    }
                    _ => false,
                }
            }
            // The reason `Rulebook.AssignSource` gave (the root's: its own, of the attacker and weapon).
            Some("about") => {
                // A nested rule's: `AssignSource` keeps a reason set at its push (unless a `mod_before`
                // handler ran).
                x.get("reason").is_some_and(|r| {
                    reason_ok(r, rule, w, limb, rules)
                        && if frames.len() > 1 {
                            // (`AssignSource` gives one when there's none.)
                            // (`AssignSource` copies its context's reason when there's no fact in context:
                            // a fact isn't seen, and is reported.)
                            let parent = frames.len().checked_sub(2).map(|i| &frames[i].reason);
                            top.before_handlers
                                || if top.push_reason.is_null() {
                                    !r.is_null() && Some(r) == parent
                                } else {
                                    *r == top.push_reason
                                }
                        } else {
                            r == root_reason
                        }
                }) && {
                    if let Some(f) = frames.last_mut() {
                        f.reason = x["reason"].clone();
                    }
                    step(&mut frames, 0, 1)
                }
            }
            Some("trig") => step(&mut frames, 1, 2),
            Some("trig_end") => step(&mut frames, 2, 3),
            Some("did") => step(&mut frames, 3, 4),
            Some("rule_end") => {
                // An attack roll's and a resolve's environment once, a flat-footed check's at most once.
                let envs = match short_class(&top.class) {
                    "RuleAttackRoll" | "RuleAttackWithWeaponResolve" => top.envs == 1,
                    _ => top.envs <= 1,
                };
                // `ActivatableAbility` handlers (records without the ability): one per subscribed
                // ability of the initiator (`IInitiatorRulebookHandler`; a fact subscribes as it turns
                // on), at about and at did, for the rules it handles.
                let subscribers = activatable_subscribers(w, &top.initiator, &top.class);
                // Its component handlers: the subscribed components' dispatch, at about and at did
                // (as a multiset: the subscribers' order isn't checked).
                let components = component_subscribers(w, &top.class, &top.initiator, &top.target);
                let sorted = |v: &Vec<(String, String)>| {
                    let mut v = v.clone();
                    v.sort();
                    v
                };
                // Its global singletons (controllers and achievements, subscribed for the session): each
                // once at about and at did for a rule it handles, never for another.
                let globals = singleton_handlers(&top.class);
                // An achievement's: at about and at did alike, once or never, and the same for every
                // rule of the run it handles (its subscription changes only as it unlocks).
                let achievements_ok = GLOBAL_HANDLERS
                    .iter()
                    .filter(|(h, r)| {
                        is_achievement(h) && r.is_none_or(|r| r.contains(&short_class(&top.class)))
                    })
                    .all(|(h, _)| {
                        let n = top.achievements[0].get(*h).copied().unwrap_or(0);
                        n <= 1
                            && top.achievements[1].get(*h).copied().unwrap_or(0) == n
                            && *achievements.borrow_mut().entry((*h).to_owned()).or_insert(n) == n
                    });
                let ok = top.activatable == [subscribers, subscribers]
                                        && top.globals.iter().all(|g| *g == globals)
                    && achievements_ok
                    && top.components.iter().all(|c| sorted(c) == components)
                    && envs
                    && x["bonuses"].as_array().is_some_and(|b| b.iter().all(|b| bonus_ok(b, w)))
                    && step(&mut frames, 4, 5)
                    && frames.last().is_some_and(|f| {
                        f.fields.get("m_Triggered") == Some(&J::Bool(true))
                            && f.fields.get("IsTriggered") == Some(&J::Bool(true))
                                                                                    && bonuses_agree(&x["bonuses"], f.fields.get("AllBonuses").or_else(|| {
                                // (Without `AllBonuses`, the container's list: none without one.)
                                match f.fields.get("m_ModifiableBonus") {
                                    Some(J::Object(o)) => o.get("m_Modifiers"),
                                    _ => Some(&EMPTY),
                                }
                            }))
                                                        // The bonuses are the ones its mutations added, in order (without dice).
                            && x["bonuses"].as_array().is_some_and(|b| {
                                b.len() == f.bonuses.len()
                                    && b.iter().all(|b| b.get("dice") == Some(&J::Null))
                                    && b.iter().zip(&f.bonuses).all(|(b, m)| {
                                        ["value", "descriptor", "type", "stat", "fact"].iter().all(|k| b.get(*k) == m.get(*k))
                                    })
                            })
                    });
                frames.pop();
                ok && (frames.is_empty() == (n == span.len() - 1))
            }
            Some("h") => {
                // Its phase is the rule's stage's: before `about`, after it, or after `trig_end`
                // (`mod_after` handlers before `did` ones).
                let phase_ok = match (x["phase"].as_str(), stage) {
                    (Some("mod_before"), 0) | (Some("about"), 1) => true,
                    (Some("mod_after"), 3) => !top.did_handlers,
                    (Some("did"), 3) => true,
                    _ => false,
                };
                let ok = x["rule"].as_i64() == Some(rule)
                    && !own_handler
                    && phase_ok
                    && handler_identity_ok(w, x, &top.class, &top.initiator, &top.target)
                    && id.is_some_and(|i| i > last_handler);
                if ok && let Some(f) = frames.last_mut() {
                    f.did_handlers |= x["phase"] == "did";
                    f.before_handlers |= x["phase"] == "mod_before";
                    let did = usize::from(x["phase"] == "did");
                    if x["class"] == ACTIVATABLE {
                        f.activatable[did] += 1;
                    } else if x.get("component").is_none()
                        && let Some(c) = x["class"].as_str()
                    {
                        let by =
                            if is_achievement(c) { &mut f.achievements } else { &mut f.globals };
                        *by[did].entry(c.to_owned()).or_default() += 1;
                    }
                    if let (Some(fact), Some(c), "about" | "did") = (
                        x["fact"]["$fact"].as_str(),
                        x["component"].as_str(),
                        x["phase"].as_str().unwrap_or(""),
                    ) {
                        f.components[did].push((fact.to_owned(), c.to_owned()));
                    }
                }
                if let Some(i) = id.filter(|_| ok) {
                    handlers.push((i, rule));
                    handler_classes.push(x["class"].as_str().unwrap_or("").to_owned());
                    last_handler = i;
                }
                ok
            }
            Some("h_end") => {
                let ok = own_handler
                    && id == Some(handler)
                    && x["rule"].as_i64() == Some(rule)
                    && frames.last_mut().is_some_and(|f| {
                        apply_diff(&mut f.fields, &x["changes"])
                            && bonus_state_ok(&f.fields, w)
                            && added_agree(&f.fields, &f.bonuses)
                            && untriggered(&f.fields)
                            // `RulebookTargetEvent.Target` is readonly.
                            && f.fields.get("Target").is_none_or(|t| *t == f.target)
                    });
                handlers.pop();
                handler_classes.pop();
                ok
            }
            Some("draw") => {
                // A simulation draw carries its rule's context (class, units, ordinal) and advances
                // the ordinal; a presentation draw has neither (`Rng`).
                let sim = x["c"] == "sim";
                let want = (|| {
                    let f = frames.last()?;
                    Some(format!(
                        "{}|{}|{}|{}",
                        f.class,
                        stable(&f.initiator)?,
                        stable(&f.target)?,
                        f.draws
                    ))
                })();
                let ctx_ok = if sim {
                    want.is_some() && x["ctx"].as_str() == want.as_deref()
                } else {
                    x.get("ctx").is_none()
                };
                if sim && let Some(f) = frames.last_mut() {
                    f.draws += 1;
                }
                // A simulation draw is the rule's computation's (`OnTrigger`) or a handler's of it.
                attributed
                    && ctx_ok
                    && draw_ok(x, sites)
                    && x.get("forced").is_some() == (replay && sim)
                    && (!sim || stage == 2 || own_handler)
            }
            // A mutation in the rule's computation or a handler of it; HP changes across the rule after
            // `did` (`OnDidTrigger`).
            Some("mut") => {
                let damage = x["what"] == "damage";
                // A rule bonus (reconciled with the rule's bonuses) or damage (with the sim's, in T0);
                // other mutations (stat values and modifiers, facts, experience) aren't reconciled
                // inside attacks: reported.
                let ok = matches!(x["what"].as_str(), Some("rule_bonus" | "damage"))
                    && attributed
                    && mutation_ok(x, rule, w)
                    && if damage { stage == 4 && !own_handler } else { stage == 2 || own_handler };
                if ok
                    && x["what"] == "rule_bonus"
                    && let Some(f) = frames.last_mut()
                {
                    add_bonus(
                        &mut f.bonuses,
                        json!({"value": x["value"], "descriptor": x["descriptor"], "type": x["type"],
                            "stat": x["stat_type"], "fact": x["fact"]}),
                    );
                }
                ok
            }
            // The engagement controller's provoke (`OnEventDidTrigger` of an attack roll or an attack
            // with a weapon), in its handler: the initiator's, checked against the sim's queue at
            // the next engagement tick.
            Some("obs") if x["ev"] == "imp_s" => {
                // Written in a did handler of this very rule: the impatience controller's for a
                // `RuleDealDamage` (its bother unit, then its end), `OffensiveActionsController`'s
                // for a `RuleAttackRoll` (the target's bother unit). Matched with the sim's
                // operations after the sim fires the rule (`imp_span`).
                let handler = handler_classes.last().map(String::as_str);
                // Its units: the bother unit is the rule's target, bothered by its initiator; the
                // damage handler's end is the initiator's, with the rule's id.
                let units_ok = match x["at"].as_str() {
                    Some("bother") => {
                        x.get("u") == top.target.get("$unit")
                            && x.get("b") == top.initiator.get("$unit")
                    }
                    Some("dmg") => {
                        x.get("u") == top.initiator.get("$unit") && x["rule"].as_i64() == Some(rule)
                    }
                    _ => false,
                };
                impatience
                    && units_ok
                    && own_handler
                    && stage == 3
                    && matches!(
                        (handler, x["at"].as_str(), short_class(&top.class)),
                        (
                            Some("Kingmaker.Controllers.UnitImpatienceController"),
                            Some("bother" | "dmg"),
                            "RuleDealDamage",
                        ) | (
                            Some("Kingmaker.Controllers.OffensiveActionsController"),
                            Some("bother"),
                            "RuleAttackRoll",
                        )
                    )
            }
            // The memory controller's `AddToMemory` (`HandleUnitMakeOffensiveAction`), raised by
            // `OffensiveActionsController`'s did handler of this very attack roll: the rule's two
            // units, either way. Matched with the sim's calls after it fires the rule (`imp_span`).
            Some("obs") if x["ev"] == "mem_q" => {
                let pair = (x.get("o"), x.get("tgt"));
                let (a, t) = (top.initiator.get("$unit"), top.target.get("$unit"));
                impatience
                    && own_handler
                    && stage == 3
                    && short_class(&top.class) == "RuleAttackRoll"
                    && handler_classes.last().map(String::as_str)
                        == Some("Kingmaker.Controllers.OffensiveActionsController")
                    && a.is_some()
                    && t.is_some()
                    && (pair == (a, t) || pair == (t, a))
            }
            Some("obs") if x["ev"] == "eng_prov" => {
                // Called by the handler of this very rule (its subscribers list's type argument).
                let list = format!(
                    "Kingmaker.Controllers.Combat.UnitCombatEngagementController.OnEventDidTrigger < Kingmaker.PubSubSystem.RulebookSubscribersList`1[[{},",
                    top.class
                );
                let ok = own_handler
                    && stage == 3
                    && top.provokes == 0
                    && handler_classes.last().map(String::as_str)
                        == Some("Kingmaker.Controllers.Combat.UnitCombatEngagementController")
                    && x["fn"] == "ProvokeAttackOfOpportunity"
                    && x["u"].as_str().is_some()
                    && x["u"].as_str() == top.initiator["$unit"].as_str()
                    && x["origin"].as_str().is_some_and(|o| o.contains(&list));
                if let Some(f) = frames.last_mut() {
                    f.provokes += 1;
                }
                ok
            }
            Some("env") => {
                // Written in the rule's `OnTrigger` prefix.
                // (A flat-footed check's only at the root: `FlatFootedPrefix` writes it when one rule
                // is open; inside an attack it's nested.)
                let ok = stage == 2
                    && !own_handler
                    && (short_class(&top.class) != "RuleCheckTargetFlatFooted" || frames.len() == 1)
                    && env_ok(x, &top.class, &top.initiator, &top.target)
                    // `EngagedBy`'s keys, ordinal-sorted and unique, each the sim's unit.
                    && x.get("target_engaged_by").and_then(J::as_array).is_none_or(|a| {
                        a.iter().all(|u| {
                            u.as_str().and_then(|id| w.by_id.get(id)).is_some_and(|e| w.unit(*e).is_some())
                        }) && a.windows(2).all(|p| p[0].as_str() < p[1].as_str())
                    });
                if let Some(f) = frames.last_mut() {
                    f.envs += 1;
                }
                ok
            }
            _ => false,
        };
        if !ok {
            if std::env::var_os("WR_DEBUG_COMMANDS").is_some() {
                eprintln!(
                    "DEBUG span: record {n} rejected: {}",
                    x.to_string().chars().take(400).collect::<String>()
                );
            }
            return false;
        }
    }
    frames.is_empty() && handlers.is_empty()
}

/// A draw's payload (`Rng`): its class, member with its arguments and value (finite numbers), its
/// site (a site of no id carries its key), the generator's state and steps.
pub(super) fn draw_ok(x: &J, sites: &HashMap<i64, (String, String)>) -> bool {
    let Some(o) = x.as_object() else { return false };
    let int = |v: &J| v.as_i64().is_some_and(|i| i32::try_from(i).is_ok());
    let float = |v: &J| v.as_f64().is_some_and(|f| f.is_finite() && f.abs() <= f64::from(f32::MAX));
    let (arity, ints) = match o.get("m").and_then(J::as_str) {
        Some("value" | "insideUnitCircle") => (0, false),
        Some("Range_ii") => (2, true),
        Some("Range_ff") => (2, false),
        _ => return false,
    };
    let num = |v: &J| if ints { int(v) } else { float(v) };
    // The member's result: `value` in [0, 1], `Range` within its arguments (an `int` range excludes
    // its maximum unless empty), a point of the unit circle.
    let a: Vec<f64> = o
        .get("a")
        .and_then(J::as_array)
        .map(|a| a.iter().filter_map(J::as_f64).collect())
        .unwrap_or_default();
    let value = match (o.get("v"), o.get("m").and_then(J::as_str)) {
        (Some(J::Array(v)), Some("insideUnitCircle")) => {
            v.len() == 2
                && v.iter().all(float)
                && v.iter().filter_map(J::as_f64).map(|c| c * c).sum::<f64>() <= 1.0 + 1e-6
        }
        (Some(v), Some("value")) => {
            float(v) && v.as_f64().is_some_and(|f| (0.0..=1.0).contains(&f))
        }
        (Some(v), Some("Range_ff")) => {
            float(v)
                && a.len() == 2
                && v.as_f64().is_some_and(|f| f >= a[0].min(a[1]) && f <= a[0].max(a[1]))
        }
        (Some(v), Some("Range_ii")) => {
            int(v)
                && a.len() == 2
                && v.as_f64()
                    .is_some_and(|f| if a[1] > a[0] { f >= a[0] && f < a[1] } else { f == a[0] })
        }
        _ => false,
    };

    let sim = o.get("c").and_then(J::as_str) == Some("sim");
    let site = o.get("site").and_then(J::as_i64);
    let keyed = site.is_some_and(|s| s < 0);
    // A replay's simulation draw says whether the tape forced it and matched (and a fallback).
    let replayed = o.contains_key("forced");
    let fallback = o.contains_key("fallback");
    let base = ["k", "t", "seq", "site", "c", "m", "a", "v", "rule", "h", "gap", "steps", "st"];
    o.len()
        == base.iter().filter(|k| o.contains_key(**k)).count()
            + usize::from(keyed)
            + usize::from(o.contains_key("ctx"))
            + 2 * usize::from(replayed)
            + usize::from(fallback)
        && base.iter().all(|k| o.contains_key(*k))
        && matches!(o.get("c").and_then(J::as_str), Some("sim" | "pres"))
        && o.get("a").and_then(J::as_array).is_some_and(|a| a.len() == arity && a.iter().all(num))
        && value
        // An instrumented site of the trace's table (of the draw's member and class: the bark
        // scheduler's weighted pick is presentation in a turn-based window), or a keyed one.
        && site.is_some_and(|s| {
            i32::try_from(s).is_ok()
                && ((s < 0
                                        // A global hook's draw (`Rng.CallerSite`): site -1, its key naming the caller and
                    // the member, its class the caller's.
                    && s == -1
                    && o.get("key").and_then(J::as_str).is_some_and(|k| {
                        keyed_ok(
                            k,
                            o.get("m").and_then(J::as_str).unwrap_or(""),
                            o.get("c").and_then(J::as_str).unwrap_or(""),
                        )
                    }))
                    || sites.get(&s).is_some_and(|(key, class)| {
                        let member = match key.rsplit('|').next() {
                            Some("Range(int,int)") => "Range_ii",
                            Some("Range(float,float)") => "Range_ff",
                            Some("get_value") => "value",
                            Some("get_insideUnitCircle") => "insideUnitCircle",
                            _ => "",
                        };
                        let c = o.get("c").and_then(J::as_str).unwrap_or("");
                        o.get("m").and_then(J::as_str) == Some(member)
                            && (c == class
                                || (class == "sim"
                                    && c == "pres"
                                    && key.contains("Kingmaker.Utility.LinqExtensions|WeightedRandom")))
                    }))
        })
        && (!keyed || o.get("key").is_some_and(J::is_string))
        // A replay's draw from the tape: forced and matched, no fallback (a fallback or a mismatch
        // is a failed replay), no generator steps.
        && (!replayed
            || (sim
                && o.get("forced") == Some(&J::Bool(true))
                && o.get("match") == Some(&J::Bool(true))
                && o.get("steps") == Some(&J::from(0))))
        && !fallback
        // `Gap` searches 4096 steps, `StepsBetween` for a draw 64 (-1: not found).
        && o.get("gap").and_then(J::as_i64).is_some_and(|g| (-1..=4096).contains(&g))
        && o.get("steps").and_then(J::as_i64).is_some_and(|n| (-1..=64).contains(&n))
        && o.get("st").and_then(J::as_str).is_some_and(|st| {
            st.split(':').count() == 4
                && st.split(':').all(|p| p.len() == 8 && p.chars().all(|c| c.is_ascii_hexdigit()))
        })
}

/// Whether a type's handler interfaces (`TypeDef::handlers`) handle `rule_class` in `phase`:
/// `about`/`did` by a rulebook handler of it, `mod_before`/`mod_after` by a before/after-trigger
/// handler of it.
fn handles(handlers: &[&str], rule_class: &str, phase: &str) -> bool {
    let arg = format!("<{}>", short_class(rule_class));
    handlers.iter().any(|h| {
        h.ends_with(&arg)
            && match phase {
                "about" | "did" => {
                    h.strip_suffix(&arg).is_some_and(|i| i.ends_with("RulebookHandler"))
                }
                "mod_before" => h.starts_with("IBeforeRulebookEventTriggerHandler<"),
                "mod_after" => h.starts_with("IAfterRulebookEventTriggerHandler<"),
                _ => false,
            }
    })
}

/// A rule not yet triggered: `OnDidTrigger`, right before `rule_end`, sets both flags.
fn untriggered(f: &serde_json::Map<String, J>) -> bool {
    f.get("m_Triggered") == Some(&J::Bool(false)) && f.get("IsTriggered") == Some(&J::Bool(false))
}

/// What a span is checked against: its frame, the sim, the trace's mode and site table, the limb item
/// `InitAttacks` recorded, the rules the trace allocated, the root's assigned reason.
#[derive(Clone, Copy)]
struct SpanCx<'a> {
    tick: i64,
    w: &'a World,
    replay: bool,
    sites: &'a HashMap<i64, (String, String)>,
    limb: Option<&'a str>,
    rules: &'a HashSet<i64>,
    root_reason: &'a J,
    /// Each achievement handler's multiplicity, as the run's first rule it handles showed it.
    achievements: &'a std::cell::RefCell<HashMap<String, usize>>,
    /// The impatience controller is observed (its handlers' records may be in a span).
    impatience: bool,
}

/// The `RulebookTargetEvent`s among the rules seen inside attacks.
const TARGET_EVENTS: &[&str] = &[
    "RuleAttackRoll",
    "RuleAttackWithWeapon",
    "RuleAttackWithWeaponResolve",
    "RuleCalculateAC",
    "RuleCalculateAttackBonus",
    "RuleCalculateDamage",
    "RuleCheckTargetFlatFooted",
    "RuleConcealmentCheck",
    "RuleDealDamage",
    "RulePrepareDamage",
];

/// The rule types seen inside attacks, by their full names.
const RULES: &[&str] = &[
    "Kingmaker.RuleSystem.Rules.Damage.RuleCalculateDamage",
    "Kingmaker.RuleSystem.Rules.Damage.RuleDealDamage",
    "Kingmaker.RuleSystem.Rules.Damage.RulePrepareDamage",
    "Kingmaker.RuleSystem.Rules.RuleAttackRoll",
    "Kingmaker.RuleSystem.Rules.RuleAttackWithWeapon",
    "Kingmaker.RuleSystem.Rules.RuleAttackWithWeaponResolve",
    "Kingmaker.RuleSystem.Rules.RuleCalculateAC",
    "Kingmaker.RuleSystem.Rules.RuleCalculateAttackBonus",
    "Kingmaker.RuleSystem.Rules.RuleCalculateAttackBonusWithoutTarget",
    "Kingmaker.RuleSystem.Rules.RuleCalculateAttacksCount",
    "Kingmaker.RuleSystem.Rules.RuleCalculateAttacksCountLimbs",
    "Kingmaker.RuleSystem.Rules.RuleCalculateWeaponSizeBonus",
    "Kingmaker.RuleSystem.Rules.RuleCalculateWeaponStats",
    "Kingmaker.RuleSystem.Rules.RuleCanApplyBuff",
    "Kingmaker.RuleSystem.Rules.RuleCheckTargetFlatFooted",
    "Kingmaker.RuleSystem.Rules.RuleConcealmentCheck",
    "Kingmaker.RuleSystem.Rules.RuleInitiativeRoll",
    "Kingmaker.RuleSystem.Rules.RuleRollD20",
];

/// A unit reference of the sim's, or null.
fn sim_unit_or_null(w: &World, v: &J) -> bool {
    v.is_null()
        || (unit_or_null(v)
            && v["$unit"]
                .as_str()
                .and_then(|id| w.by_id.get(id))
                .is_some_and(|e| w.unit(*e).is_some()))
}

/// A unit reference or null.
fn unit_or_null(v: &J) -> bool {
    match v {
        J::Null => true,
        J::Object(o) => o.len() == 1 && o.get("$unit").is_some_and(J::is_string),
        _ => false,
    }
}

/// A rule's reason (`Rules.Reason`): null, or its fact (a reference), ability (a blueprint),
/// item (an id), caster (a unit), context blueprint and rule (an id).
fn reason_ok(v: &J, below: i64, w: &World, limb: Option<&str>, rules: &HashSet<i64>) -> bool {
    let Some(o) = v.as_object() else { return v.is_null() };
    o.len() == 6
        // Its fact and item: the sim's (a fact of that blueprint; detached ones stay listed).
        && o.get("fact").is_some_and(|f| {
            f.is_null()
                || (fact_ref_ok(f)
                    && w.facts.iter().any(|x| {
                        f["$fact"].as_str() == Some(x.unique_id.as_str())
                            && f["blueprint"].as_str() == x.blueprint.map(|g| g.to_string()).as_deref()
                    }))
        })
                // Its ability (`AbilityData.Blueprint`, a `BlueprintAbility`) and context: blueprints of the
        // game's.
        && ["ability", "context_blueprint"].iter().all(|k| {
            o.get(*k).is_some_and(|b| {
                b.is_null()
                    || b.as_str().and_then(wr_content::Guid::parse).and_then(|g| w.content.get(g)).is_some_and(
                        |bp| *k != "ability" || crate::content::is_a(bp.obj.ty, "BlueprintAbility"),
                    )
            })
        })
        // (A limb's item has a fresh id at load: the one `InitAttacks` recorded.)
        && o.get("item").is_some_and(|i| {
            i.is_null()
                || i.as_str().is_some_and(|id| {
                    Some(id) == limb || w.by_id.get(id).is_some_and(|e| w.item(*e).is_some())
                })
        })
        // The caster: a unit of the sim's.
        && o.get("caster").is_some_and(|c| {
            c.is_null()
                || (unit_or_null(c)
                    && c["$unit"].as_str().and_then(|id| w.by_id.get(id)).is_some_and(|e| w.unit(*e).is_some()))
        })
        // A rule allocated before (`Rules.IdOf`: an `int`; 0 when untracked).
        && o.get("rule").is_some_and(|r| {
            r.is_null()
                || r.as_i64().is_some_and(|i| i == 0 || (i <= below && rules.contains(&i)))
        })
}

/// A rule's push snapshot: `RulebookEvent`'s own fields as at a push (untriggered, no temporary
/// modifiers, bonuses or custom data yet; the battle log flags), the class's own beside them.
fn snapshot_ok(f: &serde_json::Map<String, J>, class: &str) -> bool {
    f.get("m_Triggered") == Some(&J::Bool(false))
        && f.get("IsTriggered") == Some(&J::Bool(false))
        && f.get("m_TemporaryModifiers") == Some(&J::Null)
        && f.get("m_ModifiableBonus") == Some(&J::Null)
        && f.get("m_CustomData") == Some(&J::Null)
        // (`RuleCalculateAttackBonus`' snapshot has no `AllBonuses`.)
        && match f.get("AllBonuses") {
            None => class == "Kingmaker.RuleSystem.Rules.RuleCalculateAttackBonus",
            Some(b) => b.as_array().is_some_and(Vec::is_empty),
        }
        && f.get("DisableBattleLog").is_some_and(J::is_boolean)
        && f.get("DisableBattleLogSelf").is_some_and(J::is_boolean)
}

/// A mutation (`Hooks.Mut`) of a kind seen inside attacks, with its payload; another kind is
/// reported.
fn mutation_ok(x: &J, rule: i64, w: &World) -> bool {
    let Some(o) = x.as_object() else { return false };
    // C# `int`s (an end time is a `long`).
    let int = |k: &str| o.get(k).and_then(J::as_i64).is_some_and(|i| i32::try_from(i).is_ok());
    let string = |k: &str| o.get(k).is_some_and(J::is_string);
    // A unit (by id) of the sim's.
    let unit = |k: &str| {
        o.get(k)
            .and_then(J::as_str)
            .and_then(|id| w.by_id.get(id))
            .is_some_and(|e| w.unit(*e).is_some())
    };
    let base = ["k", "t", "seq", "what", "rule", "h"];
    let n = |extra: usize| o.len() == extra + base.iter().filter(|k| o.contains_key(**k)).count();
    // `StatId`: the owner's stat of that type (an `int`).
    let stat = || {
        let t = o.get("stat").and_then(J::as_i64).and_then(|i| i32::try_from(i).ok());
        let owner = o
            .get("owner")
            .and_then(J::as_str)
            .and_then(|id| w.by_id.get(id))
            .and_then(|e| w.unit(*e));
        t.zip(owner).is_some_and(|(t, u)| u.stats.get(crate::stats::types::StatType(t)).is_some())
    };
    match o.get("what").and_then(J::as_str) {
        Some("damage") => n(3) && unit("unit") && int("old") && int("new"),
        Some("xp") => n(3) && unit("unit") && int("gained") && int("total"),
        // `UpdateValuePostfix` writes a change only (old != new), never its re-entry sentinel.
        Some("stat_value") => {
            n(5) && unit("owner")
                && stat()
                && int("old")
                && int("new")
                && int("permanent")
                && o["old"] != o["new"]
                && o["old"].as_i64() != Some(i64::from(i32::MIN))
        }
        Some("rule_bonus") => {
            // Its target is the rule it bonuses: the innermost one (`Rules.IdOf`'s 0, an untracked
            // rule, isn't seen inside attacks: one is reported).
            n(6) && ["descriptor", "stat_type", "type", "value"].iter().all(|k| int(k))
                && o.get("target_rule").and_then(J::as_i64) == Some(rule)
                && o.get("fact").is_some_and(|f| f.is_null() || sim_fact(w, f))
                && modifier_ctor_ok(
                    &o["type"],
                    &o["stat_type"],
                    &o["descriptor"],
                    &J::Null,
                    &o["fact"],
                )
        }
        // A buff carries its timing (`Hooks.FactBody`); a fact detached is the sim's (one attached is
        // new).
        Some(what @ ("fact_attach" | "fact_detach")) => {
            let timed = o.get("class").and_then(J::as_str) == Some("Buff");
            n(if timed { 6 } else { 4 })
                && string("class")
                && unit("owner")
                && (what == "fact_attach" || o.get("fact").is_some_and(|f| sim_fact(w, f)))
                && int("rank")
                && o.get("fact").is_some_and(fact_ref_ok)
                && (!timed
                    || (o.get("end_time").and_then(J::as_i64).is_some()
                        && o.get("is_permanent").is_some_and(J::is_boolean)))
        }
        Some("stat_mod_add") => {
            let m = o.get("mod").and_then(J::as_object);
            n(4) && unit("owner")
                && stat()
                && int("modified")
                && m.is_some_and(|m| {
                    // The source item: the sim's, of that blueprint.
                    let item = m.get("item").is_some_and(|v| {
                        v.is_null()
                            || v.as_object().is_some_and(|i| {
                                i.len() == 2
                                    && i.get("$item")
                                        .and_then(J::as_str)
                                        .and_then(|id| w.by_id.get(id))
                                        .is_some_and(|e| {
                                            w.item(*e).is_some_and(|x| {
                                                i.get("blueprint").and_then(J::as_str)
                                                    == x.blueprint.map(|g| g.to_string()).as_deref()
                                            })
                                        })
                            })
                    });
                    m.len() == 8
                        && ["descriptor", "value", "stack_mode"].iter().all(|k| {
                            m.get(*k).and_then(J::as_i64).is_some_and(|i| i32::try_from(i).is_ok())
                        })
                        // `Stacks`: the stack mode's, or by default the descriptor's; its name the enum's.
                        && m.get("descriptor").and_then(J::as_i64).and_then(|d| i32::try_from(d).ok()).is_some_and(|d| {
                            let d = crate::stats::types::ModifierDescriptor(d);
                            let stacks = match m.get("stack_mode").and_then(J::as_i64) {
                                Some(0) => Some(d.is_stackable()),
                                Some(1) => Some(true),
                                Some(2) => Some(false),
                                _ => None,
                            };
                            stacks.is_some_and(|x| m.get("stacks") == Some(&J::Bool(x)))
                                && m.get("descriptor_name").and_then(J::as_str) == Some(d.name().as_ref())
                        })
                        && m.get("source").is_some_and(|f| f.is_null() || sim_fact(w, f))
                                                // `SourceComponent`: the source component's name (`SourceBlueprintComponentName`),
                        // a component of the source fact; none without a source.
                        && m.get("source_component").is_some_and(|c| {
                            c.is_null()
                                || c.as_str().is_some_and(|name| {
                                    let id = m.get("source").and_then(|f| f["$fact"].as_str());
                                    w.facts.iter().any(|x| {
                                        Some(x.unique_id.as_str()) == id
                                            && x.components.iter().any(|k| k.name == name)
                                    })
                                })
                        })
                        && item
                })
        }
        _ => false,
    }
}

/// The game's global rule subscribers seen inside attacks (`Rules`' handlers without a component),
/// with the rules each handles (its `IRulebookHandler<T>`s: `ShadowSpellController`'s
/// `RulebookTargetEvent` as the target events seen inside attacks); another class or rule is reported
/// (the sim models none of them: an unknown one may act).
const GLOBAL_HANDLERS: &[(&str, Option<&[&str]>)] = &[
    ("Kingmaker.Controllers.ShadowSpellController", Some(TARGET_EVENTS)),
    (
        "Kingmaker.UnitLogic.ActivatableAbilities.ActivatableAbility",
        Some(&["RuleAttackWithWeapon", "RuleAttackWithWeaponResolve", "RuleDealDamage"]),
    ),
    (
        "Kingmaker.Controllers.Combat.UnitCombatEngagementController",
        Some(&["RuleAttackRoll", "RuleAttackWithWeapon"]),
    ),
    (
        "Kingmaker.Achievements.Logic.AchievementLogicTacticalCombatDemonVsDemon",
        Some(&["RuleAttackWithWeapon"]),
    ),
    ("Kingmaker.Controllers.TouchSpellsController", Some(&["RuleAttackWithWeapon"])),
    ("Kingmaker.Controllers.MagusController", Some(&["RuleAttackWithWeapon"])),
    ("Kingmaker.UnitLogic.Class.Kineticist.KineticistController", Some(&["RuleAttackWithWeapon"])),
    ("Kingmaker.Controllers.OffensiveActionsController", Some(&["RuleAttackRoll"])),
    (
        "Kingmaker.Achievements.Logic.AchievementLogicMassiveDamageMountedChargeKill",
        Some(&["RuleDealDamage"]),
    ),
    ("Kingmaker.Controllers.UnitImpatienceController", Some(&["RuleDealDamage"])),
    ("Kingmaker.Controllers.UnitConcentrationController", Some(&["RuleDealDamage"])),
    ("Kingmaker.Dungeon.DungeonController", Some(&["RuleDealDamage"])),
    ("Kingmaker.Dungeon.Leaderboard.DungeonLeaderboardController", Some(&["RuleDealDamage"])),
    ("Kingmaker.Controllers.AutoPauseController", Some(&["RuleDealDamage"])),
];

/// A diff (`changes`: `[field, value]` pairs) of a rule's own fields, each value of its field's
/// kind (or null, or a container for a container), applied.
/// A value the oracle observed: no `$failed` marker inside (`Snap` writes one where reading a value
/// threw: missing evidence, not the rule's data).
fn observed(v: &J) -> bool {
    match v {
        J::Object(o) => !o.contains_key("$failed") && o.values().all(observed),
        J::Array(a) => a.iter().all(observed),
        _ => true,
    }
}

fn apply_diff(fields: &mut serde_json::Map<String, J>, changes: &J) -> bool {
    let Some(a) = changes.as_array() else { return false };
    let mut seen = std::collections::HashSet::new();
    for p in a {
        let Some([k, v]) = p.as_array().map(Vec::as_slice) else { return false };
        let Some(k) = k.as_str() else { return false };
        // Each field once (`Rules.WriteDiff`).
        if !seen.insert(k.to_owned()) {
            return false;
        }
        let Some(old) = fields.get(k) else { return false };
        if !observed(v) {
            return false;
        }
        // A field that changed (`WriteDiff` and the handlers' `Diff` leave out an unchanged one; the
        // running fields are both's baseline).
        if old == v {
            return false;
        }
        let kind = |x: &J| match x {
            J::Null => 0,
            J::Bool(_) => 1,
            J::Number(_) => 2,
            J::String(_) => 3,
            J::Array(_) | J::Object(_) => 4,
        };
        // A value stays of its kind, or moves between null and a value: the replay has no schema of
        // the classes' fields (`float?` and references are nullable), but `RulebookEvent`'s own flags
        // and `AllBonuses` (an enumerable) never are.
        let (a, b) = (kind(old), kind(v));
        let flag = [
            "m_Triggered",
            "IsTriggered",
            "DisableBattleLog",
            "DisableBattleLogSelf",
            "AllBonuses",
        ]
        .contains(&k);
        if !(a == b || (!flag && (a == 0 || b == 0))) {
            return false;
        }
        fields.insert(k.to_owned(), v.clone());
    }
    true
}

/// A bonus of `rule_end` (`Rules.OnDidTriggerPostfix`): value, descriptor, type, stat, dice, fact.
fn bonus_ok(b: &J, w: &World) -> bool {
    let Some(o) = b.as_object() else { return false };
    o.len() == 6
        && ["value", "descriptor", "type", "stat"]
            .iter()
            .all(|k| o.get(*k).and_then(J::as_i64).is_some_and(|i| i32::try_from(i).is_ok()))
        && o.get("dice").is_some_and(|d| d.is_null() || d.is_string())
        && o.get("fact").is_some_and(|f| f.is_null() || sim_fact(w, f))
        && modifier_ctor_ok(&o["type"], &o["stat"], &o["descriptor"], &o["dice"], &o["fact"])
}

/// A `Modifier` one of its constructors makes (each sets the fields the others leave at their
/// defaults): `(value, stat)` a stat's (no type, descriptor, dice or fact); `(value, type,
/// descriptor)` a type's (no stat, dice or fact); `(value, [dice,] fact, descriptor)` a fact's and
/// its dice (no type or stat); `(value, descriptor)` and `Copy` none new.
fn modifier_ctor_ok(ty: &J, stat: &J, descriptor: &J, dice: &J, fact: &J) -> bool {
    let (ty, stat, descriptor) = (ty.as_i64(), stat.as_i64(), descriptor.as_i64());
    let (dice, fact) = (!dice.is_null(), !fact.is_null());
    (stat == Some(0) || (ty == Some(0) && descriptor == Some(0) && !dice && !fact))
        && (ty == Some(0) || (stat == Some(0) && !dice && !fact))
        && (!(dice || fact) || (ty == Some(0) && stat == Some(0)))
}

/// `RulebookEvent`'s state: its containers' kinds, the battle-log invariant, and its bonus container:
/// `m_ModifiableBonus` null or its modifiers (`m_Modifiers`,
/// mirrored by `Modifiers`, counted by `ModifiersCount`), each a `Modifier`, and `AllBonuses` (where the
/// snapshot has it) the same list (empty without a container).
fn bonus_state_ok(f: &serde_json::Map<String, J>, w: &World) -> bool {
    // `RulebookEvent`'s other fields: no temporary modifiers or custom data, and its battle-log getter
    // true with the self flag.
    // (A rule's temporary modifiers and custom data aren't seen inside attacks: one is reported.)
    let base_ok = f.get("m_TemporaryModifiers") == Some(&J::Null)
        && f.get("m_CustomData") == Some(&J::Null)
        && f.get("DisableBattleLog").is_some_and(J::is_boolean)
        && (f.get("DisableBattleLogSelf") != Some(&J::Bool(true))
            || f.get("DisableBattleLog") == Some(&J::Bool(true)));
    if !base_ok {
        return false;
    }
    let modifier = |m: &J| {
        m.as_object().is_some_and(|o| {
            o.len() == 6
                && ["Value", "Type", "Stat", "Descriptor"].iter().all(|k| {
                    o.get(*k).and_then(J::as_i64).is_some_and(|i| i32::try_from(i).is_ok())
                })
                && o.get("DiceFormula").is_some_and(|d| d.is_null() || d.is_string())
                && o.get("Fact").is_some_and(|x| x.is_null() || sim_fact(w, x))
                && modifier_ctor_ok(
                    &o["Type"],
                    &o["Stat"],
                    &o["Descriptor"],
                    &o["DiceFormula"],
                    &o["Fact"],
                )
        })
    };
    let list: Option<&Vec<J>> = match f.get("m_ModifiableBonus") {
        Some(J::Null) => None,
        Some(J::Object(o)) => {
            let Some(m) = o.get("m_Modifiers").and_then(J::as_array) else { return false };
            if o.len() != 3
                || o.get("Modifiers") != o.get("m_Modifiers")
                || o.get("ModifiersCount").and_then(J::as_u64) != u64::try_from(m.len()).ok()
                || !m.iter().all(modifier)
            {
                return false;
            }
            Some(m)
        }
        _ => return false,
    };
    match f.get("AllBonuses") {
        None => true,
        Some(J::Array(a)) => list.map_or(a.is_empty(), |m| a == m),
        Some(_) => false,
    }
}

/// `ModifiableBonus.Add`: a stacking descriptor's bonus joins the list; another replaces the first of
/// its descriptor if larger, is dropped if not, or joins when it's the first.
fn add_bonus(list: &mut Vec<J>, b: J) {
    let d = b["descriptor"].as_i64().and_then(|d| i32::try_from(d).ok()).unwrap_or(0);
    if crate::stats::types::ModifierDescriptor(d).is_stackable() {
        list.push(b);
        return;
    }
    match list.iter().position(|x| x["descriptor"] == b["descriptor"]) {
        Some(i) => {
            if list[i]["value"].as_i64() < b["value"].as_i64() {
                list[i] = b;
            }
        }
        None => list.push(b),
    }
}

/// An empty list (a rule without a bonus container).
static EMPTY: J = J::Array(Vec::new());

/// `rule_end`'s bonuses are the rule's `AllBonuses` as its last diff left them (the same list,
/// written twice); a snapshot without the field (`RuleCalculateAttackBonus`') can't be compared.
/// A rule's bonuses (`AllBonuses`, else its container's: none without one) are the ones its
/// `rule_bonus` mutations added so far, in order (`RuleBonusPostfix` writes each as it's added).
fn added_agree(fields: &serde_json::Map<String, J>, added: &[J]) -> bool {
    // (`AddModifier` allocates the container as it adds the first: one without an addition isn't
    // the rule's.)
    if added.is_empty() && matches!(fields.get("m_ModifiableBonus"), Some(J::Object(_))) {
        return false;
    }
    let list = fields.get("AllBonuses").or_else(|| match fields.get("m_ModifiableBonus") {
        Some(J::Object(o)) => o.get("m_Modifiers"),
        _ => Some(&EMPTY),
    });
    // (`AddModifier`'s callers build their bonuses without dice: `Modifier`'s dice constructor
    // serves damage descriptions, not rules.)
    list.and_then(J::as_array).is_some_and(|l| {
        l.len() == added.len()
            && l.iter().all(|a| a.get("DiceFormula") == Some(&J::Null))
            && l.iter().zip(added).all(|(a, b)| {
                [
                    ("Value", "value"),
                    ("Descriptor", "descriptor"),
                    ("Type", "type"),
                    ("Stat", "stat"),
                    ("Fact", "fact"),
                ]
                .iter()
                .all(|(k, m)| a.get(*k).is_some() && a.get(*k) == b.get(*m))
            })
    })
}

fn bonuses_agree(bonuses: &J, all: Option<&J>) -> bool {
    let Some(b) = bonuses.as_array() else { return false };
    let Some(all) = all else { return true };
    let J::Array(a) = all else { return false };
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            [
                ("Value", "value"),
                ("Descriptor", "descriptor"),
                ("Type", "type"),
                ("Stat", "stat"),
                ("DiceFormula", "dice"),
                ("Fact", "fact"),
            ]
            .iter()
            .all(|(k, l)| a.get(*k).is_some() && a.get(*k) == b.get(*l))
                && a.as_object().is_some_and(|o| o.len() == 6)
        })
}

/// A fact reference to the sim's fact of that id and blueprint (detached ones stay listed).
fn sim_fact(w: &World, f: &J) -> bool {
    fact_ref_ok(f)
        && w.facts.iter().any(|x| {
            f["$fact"].as_str() == Some(x.unique_id.as_str())
                && f["blueprint"].as_str() == x.blueprint.map(|g| g.to_string()).as_deref()
        })
}

fn fact_ref_ok(f: &J) -> bool {
    f.as_object().is_some_and(|o| {
        o.len() == 2
            && o.get("$fact").is_some_and(J::is_string)
            && o.get("blueprint").is_some_and(J::is_string)
    })
}

/// A rule step's record has exactly the fields `Rules.cs` writes for its kind (on the main thread:
/// no `thread`); a handler's identity is a component's (`component`, `blueprint`, and its runtime's
/// `fact` and `owner` when found) or a fact component's (`fact`, `owner`). Other kinds have their
/// own checks.
fn envelope_ok(x: &J) -> bool {
    let Some(o) = x.as_object() else { return false };
    let root = x["depth"] == 0;
    let own: &[&str] = match x["k"].as_str() {
        Some("rule") if root => &[
            "id",
            "parent",
            "context_parent",
            "depth",
            "class",
            "handler",
            "initiator",
            "target",
            "reason_at_push",
            "origin",
            "fields",
        ],
        Some("rule") => &[
            "id",
            "parent",
            "context_parent",
            "depth",
            "class",
            "handler",
            "initiator",
            "target",
            "reason_at_push",
            "fields",
        ],
        Some("about") => &["id", "reason", "changes"],
        Some("trig" | "trig_end" | "did") => &["id", "changes"],
        Some("rule_end") => &["id", "changes", "bonuses"],
        Some("h_end") => &["id", "rule", "changes"],
        Some("h") => {
            let has = |k: &str| o.contains_key(k);
            let identity = match (has("component"), has("blueprint"), has("fact"), has("owner")) {
                (false, false, false, false) | (true, true, false, false) => true,
                (c, b, true, true) => c == b,
                _ => false,
            };
            let n = ["component", "blueprint", "fact", "owner"].iter().filter(|k| has(k)).count();
            return identity
                && o.len() == 7 + n
                && ["seq", "k", "t", "id", "rule", "phase", "class"].iter().all(|k| has(k));
        }
        _ => return true,
    };
    o.len() == 3 + own.len() && ["seq", "k", "t"].iter().chain(own).all(|k| o.contains_key(*k))
}

/// `Rng.PresentationPrefixes`: a caller type of the presentation (its draws don't feed the rules).
const PRESENTATION_PREFIXES: [&str; 22] = [
    "Owlcat.Runtime.Visual",
    "Kingmaker.Visual.Critters",
    "Kingmaker.Visual.Particles",
    "Kingmaker.Visual.Sound",
    "Kingmaker.Visual.Trails",
    "Kingmaker.Visual.RandomRotationOnStart",
    "Kingmaker.Visual.RandomScaleOnStart",
    "Kingmaker.Visual.CameraShakeFx",
    "Kingmaker.Visual.CharactersRigidbody",
    "Kingmaker.Visual.HitSystem",
    "Kingmaker.View.ShakeSettings",
    "Kingmaker.UI.",
    "Kingmaker.Assets.Visual",
    "_2dxFX",
    "Dreamteck.",
    "RootMotion.",
    "DG.Tweening",
    "AkObstructionOcclusion",
    "AnimatorControllerSpeedMultiplyer",
    "LanternKingExpressionsRandom",
    "Kingmaker.Utility.ParticleSystemDestroyer",
    "Rewired.",
];

/// A global hook draw's key (`Rng.CallerSite`): `global|<assembly>|<type>|<method(args)>[|via
/// <helper>]|<member>`, or `global|?|<member>[|via <helper>]` when no caller was found; its member
/// the draw's, its class the caller type's (`IsPresentation`; no caller: simulation).
fn keyed_ok(key: &str, member: &str, class: &str) -> bool {
    let parts: Vec<&str> = key.split('|').collect();
    let via = |p: &str| p.strip_prefix("via ").is_some_and(|h| !h.is_empty());
    let caller = match parts.as_slice() {
        ["global", "?", m] => (*m == member).then_some(None),
        ["global", "?", m, v] => (*m == member && via(v)).then_some(None),
        ["global", asm, ty, sig, rest @ ..] => {
            let tail_ok = match rest {
                [m] => *m == member,
                [v, m] => via(v) && *m == member,
                _ => false,
            };
            (tail_ok
                && !asm.is_empty()
                && *asm != "?"
                && !ty.is_empty()
                && sig.contains('(')
                && sig.ends_with(')'))
            .then_some(Some(*ty))
        }
        _ => None,
    };
    caller.is_some_and(|ty| {
        let pres = ty.is_some_and(|t| PRESENTATION_PREFIXES.iter().any(|p| t.starts_with(p)));
        class == if pres { "pres" } else { "sim" }
    })
}

/// The rule's component handlers (`RulebookEventBus`): each subscribed component of a turned-on
/// fact once for each manager it's subscribed to that dispatches the rule: the global one
/// (`IGlobalRulebookHandler<R>`), its unit's initiator subscribers when that unit is the rule's
/// initiator (`IInitiatorRulebookHandler<R>`), its target subscribers when it's the target
/// (`ITargetRulebookHandler<R>`); its unit the fact's owner or an item's wielder. Sorted (fact id,
/// component name).
fn component_subscribers(
    w: &World,
    rule_class: &str,
    initiator: &J,
    target: &J,
) -> Vec<(String, String)> {
    let arg = format!("<{}>", short_class(rule_class));
    let mut out = Vec::new();
    for f in w.facts.iter().filter(|f| !f.is_disposed) {
        let unit = f.owner.and_then(|e| match w.item(e) {
            None => w.unit(e).map(|_| e),
            Some(item) => item.wielder,
        });
        let is = |u: &J| unit.is_some_and(|e| *u == json!({"$unit": w.entity(e).unique_id}));
        for c in f.components.iter().filter(|c| c.subscribed && !c.is_disposed) {
            let Some(t) = c.source.as_ref().and_then(|(bp, i)| {
                bp.components_array().get(*i).map(|o| wr_content::schema::ty(o.ty))
            }) else {
                continue;
            };
            let has = |i: &str| t.handlers.iter().any(|h| h.strip_suffix(&arg) == Some(i));
            let n = usize::from(has("IGlobalRulebookHandler"))
                + usize::from(has("IInitiatorRulebookHandler") && is(initiator))
                + usize::from(has("ITargetRulebookHandler") && is(target));
            for _ in 0..n {
                out.push((f.unique_id.clone(), c.name.clone()));
            }
        }
    }
    out.sort();
    out
}

/// An achievement's logic: subscribed only while the achievement is locked
/// (`AchievementsManager.Activate` skips unlocked ones, `OnAchievementUnlocked` deactivates them),
/// which the save doesn't hold.
fn is_achievement(class: &str) -> bool {
    class.starts_with("Kingmaker.Achievements.")
}

/// The global singleton handlers (`GLOBAL_HANDLERS` but `ActivatableAbility`, a fact, and the
/// achievements, subscribed or not by state outside the save) that handle the rule, each once.
fn singleton_handlers(rule_class: &str) -> HashMap<String, usize> {
    GLOBAL_HANDLERS
        .iter()
        .filter(|(h, rules)| {
            *h != ACTIVATABLE
                && !is_achievement(h)
                && rules.is_none_or(|r| r.contains(&short_class(rule_class)))
        })
        .map(|(h, _)| ((*h).to_owned(), 1))
        .collect()
}

const ACTIVATABLE: &str = "Kingmaker.UnitLogic.ActivatableAbilities.ActivatableAbility";

/// The rule's initiator's `ActivatableAbility` facts subscribed to the event bus, for a rule the
/// class handles (`IInitiatorRulebookHandler<...>`); 0 for another rule.
fn activatable_subscribers(w: &World, initiator: &J, rule_class: &str) -> usize {
    let handled = GLOBAL_HANDLERS
        .iter()
        .find(|(h, _)| *h == ACTIVATABLE)
        .and_then(|(_, r)| *r)
        .is_some_and(|r| r.contains(&short_class(rule_class)));
    let Some(u) = initiator["$unit"].as_str().and_then(|id| w.by_id.get(id)).copied() else {
        return 0;
    };
    if !handled {
        return 0;
    }
    w.facts
        .iter()
        .filter(|f| {
            f.clr == "ActivatableAbility" && f.subscribed && !f.is_disposed && f.owner == Some(u)
        })
        .count()
}

/// A handler's identity (`Rules.Identity`): its class, and for a fact's component the component,
/// blueprint, fact and owner: the sim's fact of that id, blueprint and owner (a unit or an item).
fn handler_identity_ok(w: &World, x: &J, rule_class: &str, initiator: &J, target: &J) -> bool {
    let Some(o) = x.as_object() else { return false };
    let class_ok = o
        .get("class")
        .and_then(J::as_str)
        .is_some_and(|c| !c.is_empty() && c.split(['.', '+', '`']).all(|p| !p.is_empty()));
    let base = ["k", "t", "seq", "id", "rule", "phase", "class"];
    if o.keys().all(|k| base.contains(&k.as_str())) {
        return class_ok
            && o.get("class").and_then(J::as_str).is_some_and(|c| {
                GLOBAL_HANDLERS.iter().any(|(h, rules)| {
                    *h == c && rules.is_none_or(|r| r.contains(&short_class(rule_class)))
                })
            })
            && matches!(o.get("phase").and_then(J::as_str), Some("about" | "did"))
            && o.len() == base.iter().filter(|k| o.contains_key(**k)).count();
    }
    let Some(bp) = o.get("blueprint").and_then(J::as_str) else { return false };
    let fact = o.get("fact").cloned().unwrap_or(J::Null);
    let owner = o.get("owner").cloned().unwrap_or(J::Null);
    let resolved = fact_ref_ok(&fact).then(|| {
        let id = fact["$fact"].as_str().unwrap_or("");
        w.facts.iter().find(|f| f.unique_id == id && !f.is_disposed)
    });
    // The component is the fact's (by its source name), of the handler's class.
    let component = o.get("component").and_then(J::as_str).unwrap_or("");
    let phase = o.get("phase").and_then(J::as_str).unwrap_or("");
    let class = o.get("class").and_then(J::as_str).unwrap_or("");
    // `RulebookEventBus`: an about/did handler is subscribed by its interfaces (`InterfaceFinder`):
    // to the rule's initiator's subscribers, its target's or the global ones; its subscribing unit
    // is its owner, or an item's wielder (`EntityFactComponent.GetSubscribingUnit`).
    let subscribed = |handlers: &[&str], f: &crate::world::Fact| {
        let arg = format!("<{}>", short_class(rule_class));
        let has = |i: &str| handlers.iter().any(|h| h.strip_suffix(&arg) == Some(i));
        let unit = f.owner.and_then(|e| match w.item(e) {
            None => w.unit(e).map(|_| e),
            Some(item) => item.wielder,
        });
        let is = |u: &J| unit.is_some_and(|e| *u == json!({"$unit": w.entity(e).unique_id}));
        !matches!(phase, "about" | "did")
            || has("IGlobalRulebookHandler")
            || (has("IInitiatorRulebookHandler") && is(initiator))
            || (has("ITargetRulebookHandler") && is(target))
    };
    let component_ok = resolved.flatten().is_some_and(|f| {
        f.components.iter().any(|c| {
            c.name == component
                && !c.is_disposed
                && c.source.as_ref().is_some_and(|(bp, i)| {
                    bp.components_array().get(*i).is_some_and(|obj| {
                        let t = wr_content::schema::ty(obj.ty);
                        t.name == class
                            && handles(t.handlers, rule_class, phase)
                            && subscribed(t.handlers, f)
                    })
                })
        })
    });
    let fact_ok = component_ok
        && resolved.flatten().is_some_and(|f| {
            f.blueprint.map(|g| g.to_string()).as_deref() == Some(bp)
                && fact["blueprint"].as_str() == Some(bp)
                // The owner by its kind: a unit, or an item with its blueprint.
                && f.owner.is_some_and(|e| {
                    let eid = &w.entity(e).unique_id;
                    match w.item(e) {
                        None => w.unit(e).is_some() && owner == json!({"$unit": eid}),
                        Some(item) => {
                            owner == json!({"$item": eid, "blueprint": item.blueprint.map(|g| g.to_string())})
                        }
                    }
                })
        });
    class_ok
        && o.len() == base.iter().filter(|k| o.contains_key(**k)).count() + 4
        // `$<component type>$<name>`: the handler's class is the component's type.
        && o.get("component").and_then(J::as_str).is_some_and(|c| {
            c.strip_prefix('$').and_then(|c| c.split_once('$')).map(|(t, _)| t)
                == o.get("class").and_then(J::as_str).map(short_class)
        })
        && fact_ok
}

/// An environment record (`Hooks`' `env`) of the innermost rule: a kind the replay reads, its units
/// the rule's, every field there and typed, positions of two finite `float` triples.
fn env_ok(x: &J, class: &str, initiator: &J, target: &J) -> bool {
    let Some(o) = x.as_object() else { return false };
    let kind = short_class(class);
    let fields: &[(&str, &str)] = match kind {
        "RuleCheckTargetFlatFooted" => &[
            ("concealment_initiator_from_target", "int"),
            ("target_can_act_in_combat", "bool"),
            ("target_remembers_initiator", "bool"),
        ],
        "RuleAttackRoll" => &[
            ("at", "at"),
            ("concealment_initiator_from_target", "int"),
            ("concealment_target_from_initiator", "int"),
            ("target_can_act_in_combat", "bool"),
            ("target_engaged_by", "ids"),
            ("target_flanked", "bool?"),
            ("target_is_get_up", "bool"),
            ("target_remembers_initiator", "bool"),
        ],
        "RuleAttackWithWeaponResolve" => &[("at", "at")],
        _ => return false,
    };
    let pos = |v: &J| {
        v.as_array().is_some_and(|a| {
            a.len() == 3
                && a.iter().all(|c| {
                    c.as_f64().is_some_and(|f| f.is_finite() && f.abs() <= f64::from(f32::MAX))
                })
        })
    };
    let typed = |v: &J, t: &str| match t {
        // `Concealment`: None, Partial, Total.
        "int" => v.as_i64().is_some_and(|c| (0..=2).contains(&c)),
        "bool" => v.is_boolean(),
        "bool?" => v.is_boolean() || v.is_null(),
        "ids" => v.as_array().is_some_and(|a| a.iter().all(J::is_string)),
        "at" => v.as_object().is_some_and(|m| {
            m.len() == 2
                && m.get("initiator_pos").is_some_and(pos)
                && m.get("target_pos").is_some_and(pos)
        }),
        _ => false,
    };
    let base = ["k", "t", "seq", "rule", "initiator", "target"];
    o.len() == fields.len() + base.iter().filter(|k| o.contains_key(**k)).count()
        && ["k", "t", "rule", "initiator", "target"].iter().all(|k| o.contains_key(*k))
        && o["rule"].as_str() == Some(kind)
        && o.get("initiator") == Some(initiator)
        && o.get("target") == Some(target)
        && fields.iter().all(|(k, t)| o.get(*k).is_some_and(|v| typed(v, t)))
}

fn short_class(c: &str) -> &str {
    c.rsplit(['.', '+']).next().unwrap_or(c)
}

/// Whether `caller` calls `callee` in the lifecycle (the edges from an entry to `UnitAttack`'s class
/// code).
fn caller_ok(callee: &str, caller: &str) -> bool {
    const UC: &str = "Kingmaker.UnitLogic.Commands.UnitCommands.";
    const C: &str = "Kingmaker.UnitLogic.Commands.Base.UnitCommand.";
    const CTL: &str = "Kingmaker.Controllers.Units.UnitCommandController.";
    const UA: &str = "Kingmaker.UnitLogic.Commands.UnitAttack.";
    const AO: &str = "Kingmaker.UnitLogic.Commands.UnitAttackOfOpportunity.";
    const AI: &str = "Kingmaker.AI.AiBrainController.";
    let (Some(callee), Some(caller)) = (
        callee.rsplit_once('.').map(|(t, m)| (format!("{t}."), m)),
        caller.rsplit_once('.').map(|(t, m)| (format!("{t}."), m)),
    ) else {
        return false;
    };
    let is = |x: &(String, &str), t: &str, m: &str| x.0 == t && x.1 == m;
    match (callee.0.as_str(), callee.1) {
        (UA, "Init") => is(&caller, UC, "Run") || is(&caller, UC, "AddToQueueInternal"),
        (UA, "OnStart") => is(&caller, C, "Start"),
        (UA, "OnTick" | "OnAction") => is(&caller, C, "Tick"),
        // An attack of opportunity's rule (slice C).
        (AO, "OnAction") => is(&caller, C, "Tick"),
        (C, "Start" | "Tick") => is(&caller, CTL, "TickCommand"),
        (CTL, "TickCommand") => is(&caller, CTL, "TickOnUnit"),
        // `Run(cmd)` calls the private overload; the queue's head runs from
        // `RemoveFinishedAndUpdateQueue`, and `AddToQueueOrRun` runs a command whose slot is free.
        (UC, "Run") => {
            is(&caller, UC, "Run")
                || is(&caller, UC, "RemoveFinishedAndUpdateQueue")
                || is(&caller, UC, "AddToQueueOrRun")
                || is(&caller, AI, "SelectAction")
        }
        // The AI (slice D): `Tick` calls `ForceTick`, which ticks each brain.
        (AI, "SelectAction") => is(&caller, AI, "TickBrain"),
        (AI, "TickBrain") => is(&caller, AI, "ForceTick"),
        (AI, "ForceTick") => is(&caller, AI, "Tick"),
        (UC, "AddToQueueInternal") => {
            is(&caller, UC, "AddToQueue")
                || is(&caller, UC, "AddToQueueFirst")
                || is(&caller, UC, "AddToQueueOrRun")
        }
        (UC, "AddToQueue" | "AddToQueueFirst") => is(&caller, UC, "AddToQueueOrRun"),
        (UC, "RemoveFinishedAndUpdateQueue") => is(&caller, CTL, "TickOnUnit"),
        _ => false,
    }
}

/// `UnitAttack`'s frames of a root's call site (innermost first): the paths from its class code to
/// the rule's trigger in `method` (the full attack's iterator, or the single attack, from
/// `InitAttacks`; `ProvideNextAttack`'s `TriggerAttackRule` from `OnAction`).
fn path_ok(frames: &[&str], t: &str, method: &str) -> bool {
    if t == AOO_T {
        return method == "OnAction" && frames == [format!("{AOO_T}.OnAction")];
    }
    const T: &str = ATTACK_T;
    let names: Vec<&str> = frames
        .iter()
        .map(|f| {
            let m = f.strip_prefix(T).unwrap_or("");
            let iterator = m.strip_prefix("+<EnumerateFullAttack>d__").is_some_and(|rest| {
                rest.split_once('.').is_some_and(|(n, m)| {
                    !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && m == "MoveNext"
                })
            });
            if iterator { "<EnumerateFullAttack>" } else { m.strip_prefix('.').unwrap_or("?") }
        })
        .collect();
    match method {
        "InitAttacks" => matches!(
            names[..],
            ["<EnumerateFullAttack>", "CreateFullAttack", "InitAttacks", "Init" | "OnStart"]
                | ["CreateSingleAttack", "InitAttacks", "Init" | "OnStart"]
        ),
        "TriggerAttackRule" => names[..] == ["TriggerAttackRule", "ProvideNextAttack", "OnAction"],
        _ => false,
    }
}

/// The class code whose rules the sim fires: `UnitAttack`'s, an attack of opportunity's.
pub(super) const ATTACK_T: &str = "Kingmaker.UnitLogic.Commands.UnitAttack";
pub(super) const AOO_T: &str = "Kingmaker.UnitLogic.Commands.UnitAttackOfOpportunity";

/// The controller's chain below `TickOnUnit` (`UnitCommandController.Tick`'s entry).
pub(super) const CONTROLLER: &str = "Kingmaker.Controllers.Units.UnitCommandController.TickOnUnit < Kingmaker.Controllers.Units.BaseUnitController.TickUnit < Kingmaker.Controllers.Units.BaseUnitController.Tick < Kingmaker.GameModes.GameMode.Tick < Kingmaker.Game.Tick < Kingmaker.Runner.Update";

/// Whether a root rule's call site (innermost first) is `UnitAttack`'s method `method` called within
/// its class code `call` (every frame from the rule's trigger to it `UnitAttack`'s, its iterators
/// included), which the lifecycle called (`UnitCommands.Run` or `AddToQueueInternal` calls `Init`,
/// `UnitCommand.Start` calls `OnStart`, `UnitCommand.Tick` calls `OnTick` and `OnAction`), all of it
/// within the active entry: the frames after the class code are lifecycle code down to the entry's
/// own recorded call site (`entry`: a `uc_run`'s, `uc_queue`'s or `uc_x`'s origin, or
/// [`CONTROLLER`]).
fn attack_chain(origin: &str, t: &str, method: &str, call: &str, entry: &str) -> bool {
    let frames: Vec<&str> = origin.split(" < ").collect();
    let own = frames
        .iter()
        .take_while(|f| f.strip_prefix(t).is_some_and(|m| m.starts_with('.') || m.starts_with('+')))
        .count();
    if own == 0 || own == frames.len() || entry.is_empty() {
        return false;
    }
    let caller: &[&str] = match call {
        "Init" => &[
            "Kingmaker.UnitLogic.Commands.UnitCommands.Run",
            "Kingmaker.UnitLogic.Commands.UnitCommands.AddToQueueInternal",
        ],
        "OnStart" => &["Kingmaker.UnitLogic.Commands.Base.UnitCommand.Start"],
        "OnTick" | "OnAction" => &["Kingmaker.UnitLogic.Commands.Base.UnitCommand.Tick"],
        _ => return false,
    };
    let rest = frames[own..].join(" < ");
    let Some(between) = rest.strip_suffix(entry) else { return false };
    let between: Vec<&str> =
        between.trim_end_matches(" < ").split(" < ").filter(|f| !f.is_empty()).collect();
    // Each frame from the class code down to the entry's first is called by the next: an edge of
    // the lifecycle's call graph.
    let first = entry.split(" < ").next().unwrap_or("");
    let mut chain = vec![frames[own - 1]];
    chain.extend(between.iter().copied());
    chain.push(first);
    // (The public `Run(cmd)` calls the private overload, which doesn't call `Run`: two in a row at
    // most, the entry's own frames included.)
    let run = |f: &str| f == "Kingmaker.UnitLogic.Commands.UnitCommands.Run";
    let edges_ok = chain.windows(2).all(|p| caller_ok(p[0], p[1]))
        && !frames.windows(3).any(|p| p.iter().all(|f| run(f)));
    // The lifecycle methods that call class code or lead to it from an entry.
    let lifecycle = |f: &&str| {
        [
            "Kingmaker.UnitLogic.Commands.UnitCommands.Run",
            "Kingmaker.UnitLogic.Commands.UnitCommands.AddToQueueInternal",
            "Kingmaker.UnitLogic.Commands.UnitCommands.AddToQueue",
            "Kingmaker.UnitLogic.Commands.UnitCommands.AddToQueueFirst",
            "Kingmaker.UnitLogic.Commands.UnitCommands.RemoveFinishedAndUpdateQueue",
            "Kingmaker.UnitLogic.Commands.Base.UnitCommand.Start",
            "Kingmaker.UnitLogic.Commands.Base.UnitCommand.Tick",
            "Kingmaker.Controllers.Units.UnitCommandController.TickCommand",
            "Kingmaker.AI.AiBrainController.SelectAction",
            "Kingmaker.AI.AiBrainController.TickBrain",
            "Kingmaker.AI.AiBrainController.ForceTick",
        ]
        .contains(f)
    };
    path_ok(&frames[..own], t, method)
        && frames[own - 1].strip_prefix(t).and_then(|m| m.strip_prefix('.')) == Some(call)
        && caller.contains(&frames[own])
        && between.iter().all(lifecycle)
        && edges_ok
}

impl Driver<'_> {
    /// The next root rule, which `c`'s class code triggers: its class, metadata, call site (the
    /// method `frame` of class `t_class`, within the class code running), initiator and target.
    fn attack_root(
        &mut self,
        w: &mut World,
        c: u32,
        class: &str,
        (t_class, frame): (&str, &str),
        a: EntityIdx,
        t: Option<EntityIdx>,
    ) -> R<usize> {
        let at = self.uc_pull(w, &format!("command {c}'s {class}"), super::is_root)?;
        let r = &self.trace.records[at];
        let call = w.combat.commands.class_call.last().copied().unwrap_or("");
        let target_ok = match t {
            Some(t) => r.get("target") == Some(&json!({"$unit": w.entity(t).unique_id})),
            None => r.get("target") == Some(&J::Null),
        };
        let ok = r["class"].as_str() == Some(&format!("Kingmaker.RuleSystem.Rules.{class}"))
            && r["parent"] == 0
            && r["context_parent"] == 0
            && r["handler"] == 0
            && r.get("reason_at_push") == Some(&J::Null)
            && r["id"].as_i64().is_some_and(|x| x > 0)
            && r["origin"]
                .as_str()
                .is_some_and(|o| attack_chain(o, t_class, frame, call, &self.uc_origin))
            && r.get("initiator") == Some(&json!({"$unit": w.entity(a).unique_id}))
            && target_ok;
        if !ok {
            return self.uc_reject(
                at,
                format!(
                    "the sim's command {c} triggers {class} in {call}, the recording has root {} from {}",
                    r["class"], r["origin"]
                ),
            );
        }
        Ok(at)
    }

    /// The end of the root rule at `at`, every record of its span of this frame and a rule's own
    /// (its steps, handlers, nested rules, draws, mutations and environment; no lifecycle or other
    /// observation).
    fn attack_span(
        &mut self,
        w: &World,
        at: usize,
        limb: Option<&str>,
        item: Option<&str>,
    ) -> R<usize> {
        let recs = &self.trace.records;
        let end = super::root_end(recs, at);
        // A replay's trace (`begin.mode`) writes its simulation draws' tape status.
        let replay = self.replay_mode;
        // The root's reason: itself, of its initiator and (an attack's) weapon.
        let root_reason = json!({"fact": null, "ability": null, "item": item, "caster": recs[at]["initiator"],
            "context_blueprint": null, "rule": recs[at]["id"]});
        let ok = end > at
            && recs[end]["k"] == "rule_end"
            && span_ok(
                &recs[at..=end],
                &SpanCx {
                    tick: self.tick,
                    w,
                    replay,
                    sites: &self.trace.sites,
                    limb,
                    rules: &self.rule_ids,
                    root_reason: &root_reason,
                    achievements: &self.achievements,
                    impatience: self.imp,
                },
            );
        if !ok {
            return self.uc_reject(
                at,
                "a UnitAttack rule's span with records of another frame, kind or rule",
            );
        }
        // Its provokes (accepted in the engagement controller's handler).
        for i in at..=end {
            let r = &self.trace.records[i];
            if r["k"] == "obs" && r["ev"] == "eng_prov" {
                let r = r.clone();
                if !self.eng_prov(w, &r, provoke_rule(&r)) {
                    return self.uc_reject(at, "a provoke in an attack rule it can't read");
                }
            }
        }
        Ok(end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Payload checks: a modifier's references by their own schemas, a draw's member and arguments,
    /// `UnitAttack`'s frames down from the rule's trigger.
    #[test]
    fn payloads_and_paths_follow_their_writers() {
        use serde_json::json;
        let (w, _) = crate::stats::tests::world_with_unit();
        let m = |source: J| {
            json!({"k": "mut", "t": 1, "what": "stat_mod_add", "rule": 1, "h": 0, "owner": "u", "stat": 3,
                "modified": 2, "mod": {"descriptor": 40, "descriptor_name": "Helpless", "value": 2, "stack_mode": 0,
                "stacks": false, "source": source, "source_component": null, "item": null}})
        };
        assert!(mutation_ok(&m(J::Null), 1, &w));
        assert!(
            !mutation_ok(&m(json!({"$fact": "1", "blueprint": "b"})), 1, &w),
            "a source fact the sim hasn't"
        );
        assert!(!mutation_ok(&m(json!({"$unit": "u"})), 1, &w), "a unit as the modifier's source");
        // A buff's attachment carries its timing.
        let buff = |timed: bool| {
            let mut b = json!({"k": "mut", "t": 1, "what": "fact_attach", "rule": 1, "h": 0, "class": "Buff",
                "owner": "u", "rank": 1, "fact": {"$fact": "9", "blueprint": "b"}});
            if timed {
                b["end_time"] = json!(5);
                b["is_permanent"] = json!(false);
            }
            b
        };
        assert!(mutation_ok(&buff(true), 1, &w));
        assert!(!mutation_ok(&buff(false), 1, &w), "a buff without its timing");
        // `Stacks` follows the stack mode, or by default the descriptor (Helpless doesn't stack).
        let mut forced = m(J::Null);
        forced["mod"]["stack_mode"] = json!(1);
        assert!(!mutation_ok(&forced, 1, &w), "ForceStack without stacking");
        forced["mod"]["stacks"] = json!(true);
        assert!(mutation_ok(&forced, 1, &w));
        let mut named = m(J::Null);
        named["mod"]["descriptor_name"] = json!("Wrong");
        assert!(!mutation_ok(&named, 1, &w), "a descriptor's name");
        // Bonuses fold as `ModifiableBonus.Add` adds them: Enhancement (9) doesn't stack.
        let mut list = Vec::new();
        let b = |v: i64, d: i64| json!({"value": v, "descriptor": d, "type": 0, "stat": 0, "fact": null});
        add_bonus(&mut list, b(2, 9));
        add_bonus(&mut list, b(1, 9));
        assert_eq!(list, vec![b(2, 9)], "a lower non-stacking bonus is dropped");
        add_bonus(&mut list, b(3, 9));
        assert_eq!(list, vec![b(3, 9)], "a higher one replaces it in place");
        add_bonus(&mut list, b(1, 0));
        add_bonus(&mut list, b(1, 0));
        assert_eq!(list.len(), 3, "an untyped (stacking) bonus always joins");
        let sites: HashMap<i64, (String, String)> = [
            (3, ("A|T|M()|#0@3|Range(int,int)".to_owned(), "sim".to_owned())),
            (4, ("A|T|M()|#0@4|get_value".to_owned(), "sim".to_owned())),
        ]
        .into_iter()
        .collect();
        let d = json!({"k": "draw", "t": 1, "seq": 9, "site": 3, "c": "sim", "m": "Range_ii", "a": [1, 21], "v": 7,
            "rule": 1, "h": 0, "gap": 0, "steps": 1, "st": "4745e744:c34f5260:bb0a432a:ff07a905", "ctx": "x"});
        assert!(draw_ok(&d, &sites));
        let mut bad = d.clone();
        bad["a"] = json!([1, 21, 3]);
        assert!(!draw_ok(&bad, &sites), "an argument too many");
        let mut bad = d.clone();
        bad["c"] = json!("garbage");
        assert!(!draw_ok(&bad, &sites), "a class");
        let mut bad = d.clone();
        bad["site"] = json!(-1);
        assert!(!draw_ok(&bad, &sites), "a site without an id needs its key");
        let mut global = d.clone();
        global["site"] = json!(-1);
        global["key"] = json!("global|Assembly-CSharp|T|M()|Range_ii");
        assert!(draw_ok(&global, &sites), "a global hook's draw");
        global["key"] = json!("global|Assembly-CSharp|T|M()|value");
        assert!(!draw_ok(&global, &sites), "a global hook's key naming another member");
        global["site"] = json!(-2);
        assert!(!draw_ok(&global, &sites), "a keyed site other than -1");
        let mut bad = d.clone();
        bad["site"] = json!(i64::from(i32::MAX));
        assert!(!draw_ok(&bad, &sites), "a site the trace's table hasn't");
        let mut bad = d.clone();
        bad["c"] = json!("pres");
        assert!(!draw_ok(&bad, &sites), "a presentation draw of a simulation site");
        let mut bad = d.clone();
        bad["site"] = json!(4);
        assert!(!draw_ok(&bad, &sites), "a Range draw of a value site");
        let mut bad = d.clone();
        bad["v"] = json!(21);
        assert!(!draw_ok(&bad, &sites), "an int range's maximum");
        let mut value = d.clone();
        value["site"] = json!(4);
        value["m"] = json!("value");
        value["a"] = json!([]);
        value["v"] = json!(0.25);
        assert!(draw_ok(&value, &sites));
        value["v"] = json!(2.0);
        assert!(!draw_ok(&value, &sites), "a value above 1");
        let mut replayed = d.clone();
        replayed["forced"] = json!(true);
        replayed["match"] = json!(true);
        assert!(!draw_ok(&replayed, &sites), "a forced draw with generator steps");
        replayed["steps"] = json!(0);
        assert!(draw_ok(&replayed, &sites), "forced, no generator steps");
        replayed["fallback"] = json!(true);
        assert!(!draw_ok(&replayed, &sites), "a fallback: a failed replay");
        let mut bad = d.clone();
        bad["forced"] = json!(true);
        assert!(!draw_ok(&bad, &sites), "forced without match");
        let mut bad = d.clone();
        bad["v"] = json!(4.5);
        assert!(!draw_ok(&bad, &sites), "an int draw's value");
        const U: &str = "Kingmaker.UnitLogic.Commands.UnitAttack";
        let f = |names: &[&str]| names.iter().map(|n| format!("{U}{n}")).collect::<Vec<_>>();
        let full = f(&[
            "+<EnumerateFullAttack>d__91.MoveNext",
            ".CreateFullAttack",
            ".InitAttacks",
            ".Init",
        ]);
        assert!(path_ok(
            &full.iter().map(String::as_str).collect::<Vec<_>>(),
            ATTACK_T,
            "InitAttacks"
        ));
        // A command queued into a free slot runs from `AddToQueueOrRun`.
        assert!(caller_ok(
            "Kingmaker.UnitLogic.Commands.UnitCommands.Run",
            "Kingmaker.UnitLogic.Commands.UnitCommands.AddToQueueOrRun"
        ));
        assert!(!caller_ok(
            "Kingmaker.UnitLogic.Commands.Base.UnitCommand.Tick",
            "Kingmaker.UnitLogic.Commands.UnitCommands.AddToQueueInternal"
        ));
        let bad = f(&[".TriggerAttackRule", ".DoesNotExist", ".ProvideNextAttack", ".OnAction"]);
        assert!(!path_ok(
            &bad.iter().map(String::as_str).collect::<Vec<_>>(),
            ATTACK_T,
            "TriggerAttackRule"
        ));
    }

    /// A global hook draw's key: the writer's format, the draw's member, the caller type's class.
    #[test]
    fn keyed_draws_name_their_caller_and_class() {
        let bark = "global|Assembly-CSharp|Kingmaker.Visual.Sound.UnitAsksComponent|SelectAnimationBark(MappedAnimationEventType)|via LinqExtensions.Random|Range_ii";
        assert!(keyed_ok(bark, "Range_ii", "pres"));
        assert!(!keyed_ok(bark, "Range_ii", "sim"), "a presentation caller");
        assert!(!keyed_ok(bark, "value", "pres"), "another member");
        let d20 = "global|Assembly-CSharp|Kingmaker.RuleSystem.Rules.RuleRollD20|OnTrigger(RulebookEventContext)|Range_ii";
        assert!(keyed_ok(d20, "Range_ii", "sim"));
        assert!(!keyed_ok(d20, "Range_ii", "pres"), "a simulation caller");
        assert!(keyed_ok("global|?|Range_ii", "Range_ii", "sim"));
        assert!(keyed_ok("global|?|Range_ii|via LinqExtensions.Random", "Range_ii", "sim"));
        assert!(!keyed_ok("global|?|Range_ii", "Range_ii", "pres"), "no caller: simulation");
        assert!(!keyed_ok("global||Range_ii", "Range_ii", "sim"), "an empty caller");
        assert!(
            !keyed_ok("global|A|T|M|Range_ii", "Range_ii", "sim"),
            "a method without its signature"
        );
    }

    /// An attack's weapon id: a limb's own item by its recorded id; an inactive limb's empty hand
    /// (`WeaponSlot.MaybeWeapon`) and a hand's weapon by the sim's.
    #[test]
    fn an_inactive_limb_attacks_with_the_empty_hand() {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        let bite = crate::combat::weapon::create_weapon(&mut w, wr_content::Guid(0xB1), u);
        let fist = crate::combat::weapon::create_weapon(&mut w, wr_content::Guid(0xB2), u);
        w.unit_mut(u).expect("unit").body.limbs.push(crate::unit::Slot {
            name: "AdditionalLimbs[0]".into(),
            item: Some(bite),
            active: false,
        });
        let spec = |weapon| uc::attack::Spec {
            opportunity: None,
            initiator: u,
            target: u,
            weapon,
            slot: uc::attack::Slot::Limb(0),
            item: "game-bite".into(),
            penalty: 0,
            is_rend: false,
            is_first_attack: true,
            is_full_attack: false,
            is_charge: false,
            attack_number: 0,
            attacks_count: 1,
        };
        let held = spec(bite);
        assert_eq!(attack_weapon_id(&w, &held), ("game-bite", true));
        let empty = spec(fist);
        let fist_id = w.entity(fist).unique_id.clone();
        assert_eq!(attack_weapon_id(&w, &empty), (fist_id.as_str(), false));
    }

    /// A limb's game id is bound to the sim's item it holds: a limb granted again (a new item)
    /// binds a new id, an id another entity has or another item's is rejected.
    #[test]
    fn limb_ids_follow_the_limbs_items() {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        let first = crate::combat::weapon::create_weapon(&mut w, wr_content::Guid(0xB1), u);
        w.unit_mut(u).expect("unit").body.limbs.push(crate::unit::Slot {
            name: "AdditionalLimbs[0]".into(),
            item: Some(first),
            active: true,
        });
        let mut ids = HashMap::new();
        assert!(bind_limb(&mut ids, &w, u, 0, "game-a").is_ok());
        assert!(bind_limb(&mut ids, &w, u, 0, "game-a").is_ok());
        assert!(bind_limb(&mut ids, &w, u, 0, "game-b").is_err(), "another id for the same item");
        let unit_id = w.entity(u).unique_id.clone();
        let second = crate::combat::weapon::create_weapon(&mut w, wr_content::Guid(0xB1), u);
        w.unit_mut(u).expect("unit").body.limbs[0].item = Some(second);
        assert!(bind_limb(&mut ids, &w, u, 0, &unit_id).is_err(), "the unit's id");
        assert!(bind_limb(&mut ids, &w, u, 0, "game-a").is_err(), "the old item's id");
        assert!(bind_limb(&mut ids, &w, u, 0, "game-b").is_ok(), "the new item's id");
        let own = w.entity(second).unique_id.clone();
        let mut fresh = HashMap::new();
        assert!(bind_limb(&mut fresh, &w, u, 0, &own).is_ok(), "the sim's id for the item");
        w.unit_mut(u).expect("unit").body.limbs[0].item = None;
        assert!(bind_limb(&mut ids, &w, u, 0, "game-c").is_err(), "an empty limb");
    }

    /// A shield bash: the active off hand holds a shield, the attack's weapon is its weapon component.
    #[test]
    fn a_shield_bash_slot_holds_the_shield() {
        use serde_json::json;
        let (mut w, u) = crate::stats::tests::world_with_unit();
        let bash = crate::combat::weapon::create_weapon(&mut w, wr_content::Guid(0xB1), u);
        let shield = crate::combat::weapon::create_weapon(&mut w, wr_content::Guid(0xB2), u);
        w.entities[shield.0 as usize].clr = "ItemEntityShield";
        if let crate::world::EntityKind::Item(x) = &mut w.entities[shield.0 as usize].kind {
            x.weapon_component = Some(bash);
        }
        w.unit_mut(u).expect("unit").body.slots.push(crate::unit::Slot {
            name: "HandsEquipmentSets[0].SecondaryHand".into(),
            item: Some(shield),
            active: true,
        });
        let (sid, bid) = (w.entity(shield).unique_id.clone(), w.entity(bash).unique_id.clone());
        let spec = uc::attack::Spec {
            opportunity: None,
            initiator: u,
            target: u,
            weapon: bash,
            slot: uc::attack::Slot::Secondary,
            item: sid.clone(),
            penalty: 0,
            is_rend: false,
            is_first_attack: true,
            is_full_attack: false,
            is_charge: false,
            attack_number: 0,
            attacks_count: 1,
        };
        let flag = json!({"m_Count": 0, "Count": 0, "Value": false});
        let slot = json!({"m_ItemRef": {"$entity": sid, "class": "ItemEntity"}, "m_Active": true,
            "m_HideEquipmentEntity": false, "Owner": {"$unit": w.entity(u).unique_id}, "m_DeactivateFlag": flag,
            "Lock": flag, "m_FakeItem": null, "m_IsItemFake": false, "BuiltIn": false,
            "ParentFact": {"m_Proxy": null, "EntityId": null, "FactId": null, "IsNull": true},
            "m_NaturalFxSnapMap": null, "m_FxSnapMap": null, "IsDirty": false, "IsItemFake": false,
            "Active": true, "HideEquipmentEntity": false, "Disabled": false});
        assert!(slot_snapshot_ok(
            &w,
            u,
            spec.slot,
            &spec.item,
            &slot,
            Some((bash, Some(bid.as_str())))
        ));
        assert!(
            !slot_snapshot_ok(
                &w,
                u,
                spec.slot,
                &spec.item,
                &slot,
                Some((bash, Some("another-item")))
            ),
            "another weapon"
        );
        let mut other = slot.clone();
        other["m_ItemRef"]["$entity"] = json!(bid);
        assert!(
            !slot_snapshot_ok(
                &w,
                u,
                spec.slot,
                &spec.item,
                &other,
                Some((bash, Some(bid.as_str())))
            ),
            "the slot holding the component"
        );
    }

    /// A span with a rule triggered inside a handler (`FlankedAttackBonus` triggering a flat-footed
    /// check from its attack bonus handler) nests, its draw attributed to the open (ancestor's)
    /// handler; a step missing, a handler misattributed, misplaced or ended without one open, a
    /// draw's context or a diff damaged doesn't.
    #[test]
    fn spans_follow_the_rules_lifecycle() {
        use serde_json::json;
        let (w, _) = crate::stats::tests::world_with_unit();
        let sites: HashMap<i64, (String, String)> = [
            (1, ("A|T|M()|#0@1|get_value".to_owned(), "pres".to_owned())),
            (2, ("A|T|M()|#0@2|Range(int,int)".to_owned(), "sim".to_owned())),
        ]
        .into_iter()
        .collect();
        let rules: HashSet<i64> = [1, 2].into_iter().collect();
        let ach = std::cell::RefCell::new(HashMap::new());
        let root_reason = json!({"fact": null, "ability": null, "item": null, "caster": null,
            "context_blueprint": null, "rule": 1});
        let rule = |id: i64, parent: i64, depth: i64, handler: i64| {
            let mut r = json!({"k": "rule", "t": 3, "id": id, "parent": parent, "context_parent": parent,
                "depth": depth, "handler": handler, "class": "Kingmaker.RuleSystem.Rules.RuleCalculateAC",
                "initiator": null, "target": null, "reason_at_push": null, "fields": {"Value": 0, "Target": null, "m_Triggered": false, "IsTriggered": false,
                "m_TemporaryModifiers": null, "m_ModifiableBonus": null, "m_CustomData": null, "AllBonuses": [],
                "DisableBattleLog": false, "DisableBattleLogSelf": false}});
            // (`Rules.cs` writes a root's call site.)
            if depth == 0 {
                r["origin"] = json!("Kingmaker.UnitLogic.Commands.UnitAttack.TriggerAttackRule");
            }
            r
        };
        let step = |k: &str, id: i64| json!({"k": k, "t": 3, "id": id, "changes": []});
        let about = |id: i64| {
            json!({"k": "about", "t": 3, "id": id, "changes": [["Value", 1]], "reason": {"fact": null,
                "ability": null, "item": null, "caster": null, "context_blueprint": null, "rule": 1}})
        };
        let end = |id: i64| {
            json!({"k": "rule_end", "t": 3, "id": id, "bonuses": [],
                "changes": [["m_Triggered", true], ["IsTriggered", true]]})
        };
        let h = |id: i64, r: i64, phase: &str| json!({"k": "h", "t": 3, "id": id, "rule": r, "phase": phase, "class": "Kingmaker.Controllers.ShadowSpellController"});
        let h_end =
            |id: i64, r: i64| json!({"k": "h_end", "t": 3, "id": id, "rule": r, "changes": []});
        let span = vec![
            rule(1, 0, 0, 0),
            about(1),
            h(5, 1, "about"),
            rule(2, 1, 1, 5),
            about(2),
            // (The global singletons handle every rule of theirs: `ShadowSpellController` the
            // nested one too.)
            h(6, 2, "about"),
            h_end(6, 2),
            step("trig", 2),
            json!({"k": "draw", "t": 3, "site": 1, "c": "pres", "m": "value", "a": [], "v": 0.5, "rule": 2,
                "h": 5, "gap": 0, "steps": 1, "st": "00000000:00000000:00000000:00000001"}),
            json!({"k": "draw", "t": 3, "site": 2, "c": "sim", "m": "Range_ii", "a": [1, 21], "v": 3, "rule": 2,
                "h": 5, "gap": 0, "steps": 1, "st": "00000000:00000000:00000000:00000002",
                "ctx": "Kingmaker.RuleSystem.Rules.RuleCalculateAC|-|-|0"}),
            step("trig_end", 2),
            h(7, 2, "did"),
            h_end(7, 2),
            step("did", 2),
            end(2),
            h_end(5, 1),
            step("trig", 1),
            step("trig_end", 1),
            h(8, 1, "did"),
            h_end(8, 1),
            step("did", 1),
            json!({"k": "mut", "t": 3, "rule": 1, "h": 0, "what": "damage", "unit": "u", "old": 0, "new": 1}),
            end(1),
        ];
        // Trace sequence numbers, spaced to leave room for inserted records.
        let span: Vec<J> = span
            .into_iter()
            .enumerate()
            .map(|(n, mut r)| {
                r["seq"] = json!(n * 10);
                r
            })
            .collect();
        assert!(span_ok(
            &span,
            &SpanCx {
                tick: 3,
                w: &w,
                replay: false,
                sites: &sites,
                limb: None,
                rules: &rules,
                root_reason: &root_reason,
                achievements: &ach,
                impatience: false
            }
        ));
        // The root's reason is the one `AssignSource` gave.
        let other = json!({"fact": null, "ability": null, "item": null, "caster": null,
            "context_blueprint": null, "rule": 0});
        assert!(
            !span_ok(
                &span,
                &SpanCx {
                    tick: 3,
                    w: &w,
                    replay: false,
                    sites: &sites,
                    limb: None,
                    rules: &rules,
                    root_reason: &other,
                    achievements: &ach,
                    impatience: false
                }
            ),
            "another reason for the root"
        );
        // A reason's rule the trace didn't allocate.
        let only_two: HashSet<i64> = [2].into_iter().collect();
        assert!(
            !span_ok(
                &span,
                &SpanCx {
                    tick: 3,
                    w: &w,
                    replay: false,
                    sites: &sites,
                    limb: None,
                    rules: &only_two,
                    root_reason: &root_reason,
                    achievements: &ach,
                    impatience: false
                }
            ),
            "a reason's unallocated rule"
        );
        let bad = |f: &dyn Fn(&mut Vec<J>)| {
            let mut x = span.clone();
            f(&mut x);
            span_ok(
                &x,
                &SpanCx {
                    tick: 3,
                    w: &w,
                    replay: false,
                    sites: &sites,
                    limb: None,
                    rules: &rules,
                    root_reason: &root_reason,
                    achievements: &ach,
                    impatience: false,
                },
            )
        };
        assert!(!bad(&|x| x[3]["handler"] = json!(0)), "the nested rule's handler");
        assert!(!bad(&|x| x[9]["h"] = json!(0)), "a nested draw outside the open handler");
        assert!(
            !bad(&|x| x[9]["ctx"] = json!("Kingmaker.RuleSystem.Rules.RuleCalculateAC|-|-|1")),
            "a draw's ordinal"
        );
        assert!(!bad(&|x| x[21]["h"] = json!(8)), "a mutation in a closed handler");
        assert!(
            !bad(&|x| {
                x.remove(15);
            }),
            "a handler left open"
        );
        assert!(
            !bad(&|x| {
                x.remove(1);
            }),
            "a missing about"
        );
        assert!(
            !bad(&|x| {
                x.remove(16);
            }),
            "a missing trig"
        );
        assert!(
            !bad(&|x| x.insert(17, {
                let mut e = h_end(0, 1);
                e["seq"] = json!(165);
                e
            })),
            "an h_end without an open handler"
        );
        assert!(!bad(&|x| x[18]["phase"] = json!("nonsense")), "a phase");
        assert!(!bad(&|x| x[18]["phase"] = json!("mod_before")), "a phase out of its stage");
        assert!(
            !bad(&|x| {
                x[1].as_object_mut().unwrap().remove("changes");
            }),
            "a diff"
        );
        assert!(!bad(&|x| x[2]["t"] = json!(4)), "another frame");
        assert!(
            !bad(&|x| {
                x.drain(5..7);
            }),
            "a global singleton's handler left out"
        );
        assert!(!bad(&|x| x[22]["changes"] = json!([])), "a rule closed untriggered");
        assert!(
            !bad(&|x| x[8]["ctx"] = json!("Kingmaker.RuleSystem.Rules.RuleCalculateAC|-|-|0")),
            "a presentation draw's context"
        );
        assert!(!bad(&|x| x[1]["changes"] = json!([["Value", true]])), "a value of another kind");
        assert!(!bad(&|x| x[1]["changes"] = json!([["Missing", 1]])), "a field the rule hasn't");
        assert!(
            !bad(&|x| {
                x[3].as_object_mut().unwrap().remove("fields");
            }),
            "a nested rule without its snapshot"
        );
    }

    #[test]
    fn origins_place_their_calls() {
        let follow = "Kingmaker.UnitLogic.Commands.UnitCommands.Run < Kingmaker.UnitLogic.Commands.UnitCommands.Run < Kingmaker.Controllers.Units.UnitFollowUnitController.Move < Kingmaker.Controllers.Units.UnitFollowUnitController.Tick < Kingmaker.Controllers.Units.UnitFollowUnitController.TickOnUnit < Kingmaker.Controllers.Units.BaseUnitController.TickUnit < Kingmaker.Controllers.Units.BaseUnitController.Tick < Kingmaker.GameModes.GameMode.Tick < Kingmaker.Game.Tick < Kingmaker.Runner.Update";
        assert_eq!(phase_of(follow), Some(Phase::Follow));
        let end = "Kingmaker.UnitLogic.Commands.UnitCommands.InterruptAll < TurnBased.Controllers.TurnController.InterruptCommands < TurnBased.Controllers.TurnController.End < TurnBased.Controllers.TurnController.Tick < TurnBased.Controllers.CombatController.Tick < Kingmaker.GameModes.GameMode.Tick < Kingmaker.Game.Tick < Kingmaker.Runner.Update";
        assert_eq!(phase_of(end), Some(Phase::Turn));
        let ui = "Kingmaker.UnitLogic.Commands.UnitCommands.Run < TurnBased.Controllers.TurnController.UpdateActionPredictions < TurnBased.Controllers.TurnController.HandlePortraitHover < UnityEngine.SetupCoroutine.InvokeMoveNext";
        assert_eq!(phase_of(ui), Some(Phase::Post));
        assert_eq!(phase_of(&format!("{ui} < ...")), None);
        let unknown =
            "X.Y < Kingmaker.Controllers.SomeController.Tick < Kingmaker.GameModes.GameMode.Tick";
        assert_eq!(phase_of(unknown), None);
    }

    #[test]
    fn lifecycle_records_stop_a_pull() {
        let rec = |ev: &str, extra: J| {
            let mut o = serde_json::json!({"k": "obs", "ev": ev});
            if let (J::Object(o), J::Object(e)) = (&mut o, extra) {
                o.extend(e);
            }
            o
        };
        assert!(boundary(&rec("uc_v", serde_json::json!({}))));
        assert!(boundary(&rec("uc_end", serde_json::json!({"ctx": "base"}))));
        assert!(!boundary(&rec("uc_end", serde_json::json!({"ctx": "other"}))));
        assert!(boundary(&rec("uc_run", serde_json::json!({"nested": true}))));
        assert!(!boundary(&rec("uc_run", serde_json::json!({"nested": false}))));
        assert!(boundary(&rec(
            "cd_spend",
            serde_json::json!({"caller": "Kingmaker.EntitySystem.Entities.UnitEntityData.SpendAction < Kingmaker.Controllers.Units.UnitCommandController.TickCommand"})
        )));
        assert!(!boundary(&rec(
            "cd_spend",
            serde_json::json!({"caller": "x < TurnBased.Controllers.TurnController.Prepare"})
        )));
        assert!(boundary(&serde_json::json!({"k": "tick"})));
        assert!(!boundary(&serde_json::json!({"k": "rule"})));
    }
}
