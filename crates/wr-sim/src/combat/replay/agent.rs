//! The driven replay's side of the movement agent ([`crate::combat::agent`],
//! [`crate::combat::movement`]): WrathOracle's observer `agent` (`docs/poc5/movement.md`, "The
//! driven replay"). `ag_init` gives every agent's state at the window's first tick; from there
//! each agent is the sim's. Each entry record (`ag_ctl`: `UnitMoveController.TickUnit`;
//! `ag_req`: `PathTo`; `ag_done`: `OnPathComplete`; a top-level `ag_call`) runs the sim's code,
//! which pulls the records it makes and reads in order through [`AgentInputs`]; a state the
//! recording writes (an entry's, a call's end, a tick's) is checked against the sim's, and on a
//! difference reported and taken. What stays an input is counted (`Outcome::ag_inputs`).

use std::collections::HashMap;

use serde_json::Value as J;

use super::commands::describe;
use super::{Driver, Phase};
use crate::combat::agent::{
    self as ag, Agent, AgentInputs, AgentPath, DestAgent, PathRequest, Proxy, V2, V3,
};
use crate::combat::commands::{Desync, R};
use crate::combat::movement::{self, MoveInputs};
use crate::combat::orientation;
use crate::world::{EntityIdx, World};

fn f(v: &J) -> Option<f32> {
    #[allow(clippy::cast_possible_truncation)]
    v.as_f64().map(|x| x as f32)
}

fn v3(v: &J) -> Option<V3> {
    let a = v.as_array().filter(|a| a.len() == 3)?;
    Some([f(&a[0])?, f(&a[1])?, f(&a[2])?])
}

fn v2(v: &J) -> Option<V2> {
    let a = v.as_array().filter(|a| a.len() == 2)?;
    Some([f(&a[0])?, f(&a[1])?])
}

fn opt_v3(v: &J) -> Option<Option<V3>> {
    if v.is_null() { Some(None) } else { v3(v).map(Some) }
}

fn opt_f(v: &J) -> Option<Option<f32>> {
    if v.is_null() { Some(None) } else { f(v).map(Some) }
}

fn bits3(v: V3) -> [u32; 3] {
    v.map(f32::to_bits)
}

/// An agent from a recorded state `s` (WrathOracle's `WriteState`), with the paths it holds
/// from `paths`; `None` when a field is missing or of another type.
pub(crate) fn agent_from(s: &J, paths: &HashMap<String, Vec<V3>>) -> Option<Agent> {
    let b = |k: &str| s[k].as_bool();
    let n = |k: &str| f(&s[k]);
    let i = |k: &str| s[k].as_i64().and_then(|x| i32::try_from(x).ok());
    let path = |k: &str| -> Option<Option<AgentPath>> {
        match &s[k] {
            J::Null => Some(None),
            J::String(key) => Some(Some(AgentPath {
                key: key.clone(),
                points: paths.get(key).cloned().unwrap_or_default(),
                forced: key.starts_with("?ForcedPath"),
            })),
            _ => None,
        }
    };
    let dest_agent = if b("dhas")? {
        Some(DestAgent { view: s["dview"].as_str().map(str::to_owned), attached: b("dattached")? })
    } else {
        None
    };
    Some(Agent {
        pos: v3(&s["pos"])?,
        vel: v3(&s["vel"])?,
        next_vel: v3(&s["nvel"])?,
        path: path("path")?,
        requested: path("req")?,
        requested_find: path("find")?,
        dest_agent,
        requested_new: b("reqnew")?,
        roaming: b("roam")?,
        force_roaming: b("froam")?,
        first_tick: b("first")?,
        force: b("force")?,
        follow: b("follow")?,
        no_more_tb: b("nomoretb")?,
        repath: b("repath")?,
        stopping: b("stopping")?,
        avoidance_off: b("avoidoff")?,
        connected: b("conn")?,
        warmup: n("warmup")?,
        remaining: n("remain")?,
        eta: n("eta")?,
        last_turn: n("lastturn")?,
        stuck_stop: n("stuckstop")?,
        stuck_dir: n("stuckdir")?,
        min_wp: n("minwp")?,
        slow_down: n("slow")?,
        speed: n("speed")?,
        min_speed: n("minspeed")?,
        accel: n("accel")?,
        angular: n("ang")?,
        combat_angular: n("cang")?,
        approach: n("ar")?,
        max_approach: n("mar")?,
        saved_min_speed: opt_f(&s["savedmin"])?,
        max_speed_override: opt_f(&s["maxover"])?,
        next_index: i("idx")?,
        path_frame: i("pframe")?,
        repath_after: i("repathafter")?,
        charging: i("charging")?,
        next_wp: v2(&s["wp"])?,
        next_next_wp: v2(&s["wp2"])?,
        next_dir: v2(&s["ndir"])?,
        move_dir: v2(&s["mdir"])?,
        dest: opt_v3(&s["dest"])?,
        no_path_dest: opt_v3(&s["nopath"])?,
    })
}

/// The fields of two agents that differ (by bits; paths by key).
fn diff(ours: &Agent, want: &Agent) -> Vec<&'static str> {
    let mut out = Vec::new();
    let fb = |a: f32, b: f32| a.to_bits() == b.to_bits();
    let ob = |a: Option<f32>, b: Option<f32>| a.map(f32::to_bits) == b.map(f32::to_bits);
    let vb = |a: V3, b: V3| bits3(a) == bits3(b);
    let v2b = |a: V2, b: V2| a.map(f32::to_bits) == b.map(f32::to_bits);
    let ovb = |a: Option<V3>, b: Option<V3>| a.map(bits3) == b.map(bits3);
    let key = |p: &Option<AgentPath>| p.as_ref().map(|x| x.key.clone());
    let checks: [(&'static str, bool); 44] = [
        ("pos", vb(ours.pos, want.pos)),
        ("vel", vb(ours.vel, want.vel)),
        ("nvel", vb(ours.next_vel, want.next_vel)),
        ("path", key(&ours.path) == key(&want.path)),
        ("req", key(&ours.requested) == key(&want.requested)),
        ("find", key(&ours.requested_find) == key(&want.requested_find)),
        ("dest agent", ours.dest_agent == want.dest_agent),
        ("reqnew", ours.requested_new == want.requested_new),
        ("roam", ours.roaming == want.roaming),
        ("froam", ours.force_roaming == want.force_roaming),
        ("first", ours.first_tick == want.first_tick),
        ("force", ours.force == want.force),
        ("follow", ours.follow == want.follow),
        ("nomoretb", ours.no_more_tb == want.no_more_tb),
        ("repath", ours.repath == want.repath),
        ("stopping", ours.stopping == want.stopping),
        ("avoidoff", ours.avoidance_off == want.avoidance_off),
        ("conn", ours.connected == want.connected),
        ("warmup", fb(ours.warmup, want.warmup)),
        ("remain", fb(ours.remaining, want.remaining)),
        ("eta", fb(ours.eta, want.eta)),
        ("lastturn", fb(ours.last_turn, want.last_turn)),
        ("stuckstop", fb(ours.stuck_stop, want.stuck_stop)),
        ("stuckdir", fb(ours.stuck_dir, want.stuck_dir)),
        ("minwp", fb(ours.min_wp, want.min_wp)),
        ("slow", fb(ours.slow_down, want.slow_down)),
        ("speed", fb(ours.speed, want.speed)),
        ("minspeed", fb(ours.min_speed, want.min_speed)),
        ("accel", fb(ours.accel, want.accel)),
        ("ang", fb(ours.angular, want.angular)),
        ("cang", fb(ours.combat_angular, want.combat_angular)),
        ("ar", fb(ours.approach, want.approach)),
        ("mar", fb(ours.max_approach, want.max_approach)),
        ("savedmin", ob(ours.saved_min_speed, want.saved_min_speed)),
        ("maxover", ob(ours.max_speed_override, want.max_speed_override)),
        ("idx", ours.next_index == want.next_index),
        ("pframe", ours.path_frame == want.path_frame),
        ("repathafter", ours.repath_after == want.repath_after),
        ("charging", ours.charging == want.charging),
        ("wp", v2b(ours.next_wp, want.next_wp) && v2b(ours.next_next_wp, want.next_next_wp)),
        ("ndir", v2b(ours.next_dir, want.next_dir)),
        ("mdir", v2b(ours.move_dir, want.move_dir)),
        ("dest", ovb(ours.dest, want.dest)),
        ("nopath", ovb(ours.no_path_dest, want.no_path_dest)),
    ];
    for (k, same) in checks {
        if !same {
            out.push(k);
        }
    }
    out
}

/// Which entry an `ag_*` record starts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AgEntry {
    Ctl,
    Req,
    Done,
    Call,
}

impl Driver<'_> {
    fn ag_input(&mut self, what: &'static str) {
        *self.out.ag_inputs.entry(what).or_default() += 1;
    }

    /// A draw at `UnitMovementAgent.SlowDown`'s site.
    pub(super) fn ag_slow_draw(&self, r: &J) -> bool {
        r["k"] == "draw"
            && (r["site"].as_i64().is_some_and(|s| self.ag_slow_sites.contains(&s))
                || self.trace.site_key(r).as_deref() == Some(crate::rng::sites::AGENT_SLOW_DOWN.0))
    }

    /// A record an agent entry pulls (one it didn't ask for stops the pull).
    fn ag_stop(&self, r: &J) -> bool {
        if r["k"] == "tick" || self.ag_slow_draw(r) {
            return true;
        }
        r["k"] == "obs"
            && r["ev"].as_str().is_some_and(|e| {
                (e.starts_with("ag_") && e != "ag_pathdef" && e != "ag_findclr") || e == "cd_move"
            })
    }

    /// Records up to the next one `want` takes (returned); everything before it is processed as
    /// usual, but an agent record the sim didn't ask for stops the pull.
    fn ag_pull(&mut self, w: &mut World, what: &str, want: impl Fn(&J) -> bool) -> R<usize> {
        let trace = self.trace;
        loop {
            let Some(r) = trace.records.get(self.i) else {
                return Err(Desync(format!("{what}: the recording ends")));
            };
            if want(r) {
                if r["t"].as_i64() != Some(self.tick) {
                    return Err(Desync(format!(
                        "{what}: a record of another frame ({})",
                        describe(r)
                    )));
                }
                self.i += 1;
                return Ok(self.i - 1);
            }
            // `CanMoveInTurnBased`'s postfix (its answer and delta): the sim's own are its
            // `TickMovement`'s, checked by the state after.
            if r["k"] == "obs" && r["ev"] == "ag_v" && r["f"] == "canmovetb" {
                self.i += 1;
                continue;
            }
            if self.ag_stop(r) {
                return Err(Desync(format!("{what}: the recording has {}", describe(r))));
            }
            let i = self.i;
            self.i = self.record(w, i);
        }
    }

    /// The next record a pull would stop at (`canmovetb` postfixes aside), not consumed.
    fn ag_peek(&self) -> Option<J> {
        self.trace.records[self.i..]
            .iter()
            .find(|r| self.ag_stop(r) && !(r["ev"] == "ag_v" && r["f"] == "canmovetb"))
            .cloned()
    }

    fn ag_unit_id(&self, w: &World, u: EntityIdx) -> String {
        w.entity(u).unique_id.clone()
    }

    /// The next agent record `ev` of the entry's unit (or with no unit).
    fn ag_take(&mut self, w: &mut World, u: EntityIdx, ev: &str, what: &str) -> R<J> {
        let id = self.ag_unit_id(w, u);
        let at = self.ag_pull(w, what, |r| {
            r["k"] == "obs"
                && r["ev"] == ev
                && (r["u"].is_null() || r["u"].as_str() == Some(id.as_str()))
        })?;
        Ok(self.trace.records[at].clone())
    }

    fn ag_leaf(&mut self, w: &mut World, u: EntityIdx, leaf: &str) -> R<J> {
        let id = self.ag_unit_id(w, u);
        let at = self.ag_pull(w, &format!("the agent's {leaf}"), |r| {
            r["k"] == "obs"
                && r["ev"] == "ag_v"
                && r["f"] == leaf
                && (r["u"].is_null() || r["u"].as_str() == Some(id.as_str()))
        })?;
        self.ag_input(match leaf {
            "isdone" => "isdone",
            "animprev" => "animprev",
            "cmdprev" => "cmdprev",
            "maxspeed" => "maxspeed",
            "fly" => "fly",
            "canmove" => "canmove",
            "canrotate" => "canrotate",
            "csctl" | "csactive" => "cutscene",
            "corp" => "corpulence",
            "cells" => "cells",
            "fudge" => "fudge",
            "speedmps" => "speedmps",
            "navnode" => "navnode",
            "move" => "move command",
            "gmem" => "group memory",
            "softcol" | "closest" => "soft collider",
            "proxy" => "proxy",
            "gamepad" => "gamepad",
            _ => "other leaf",
        });
        Ok(self.trace.records[at].clone())
    }

    /// The sim's agent against a recorded state: a difference is reported and the recorded
    /// state taken.
    fn ag_check_state(&mut self, w: &mut World, u: EntityIdx, s: &J, what: &str) {
        let Some(want) = agent_from(s, &self.ag_paths) else {
            w.coverage.unsupported(format!("driven replay: agent: {what}: a state it can't read"));
            return;
        };
        // After a real-time span taken from the recording (its effects on other agents, e.g. a
        // repath, aren't modelled), each agent's next recorded state is taken as it comes.
        if self.ag_after_rt.remove(&u) {
            if let Some(unit) = w.unit_mut(u) {
                unit.combat.agent = want;
            }
            self.ag_input("state after real time");
            self.ag_last_state.insert(u, s.clone());
            return;
        }
        let Some(unit) = w.unit_mut(u) else { return };
        let fields = diff(&unit.combat.agent, &want);
        self.out.ag_states += 1;
        if !fields.is_empty() {
            if std::env::var_os("WR_DEBUG_AGENT").is_some() {
                eprintln!(
                    "DEBUG agent tick {}: {what}: {fields:?}\n  ours {:?}\n  game {:?}",
                    self.tick, unit.combat.agent, want
                );
            }
            unit.combat.agent = want;
            w.coverage.unsupported(format!(
                "driven replay: agent: {what}: the sim's state differs from the recording's ({})",
                fields.join(", ")
            ));
        }
        self.ag_last_state.insert(u, s.clone());
    }

    /// An `ag_*` record.
    pub(super) fn ag_record(&mut self, w: &mut World, i: usize) -> usize {
        let trace = self.trace;
        let r = &trace.records[i];
        let ev = r["ev"].as_str().unwrap_or("");
        if r["t"].as_i64() != Some(self.tick) {
            w.coverage
                .unsupported(format!("driven replay: an agent record ({ev}) of another frame"));
            return i + 1;
        }
        match ev {
            "ag_init" => self.ag_init(w, r),
            "ag_pathdef" => {
                if r["pending"] != true
                    && let (Some(k), Some(v)) = (r["path"].as_str(), r["v"].as_array())
                {
                    let pts: Option<Vec<V3>> = v.iter().map(v3).collect();
                    match pts {
                        Some(p) => {
                            self.ag_paths.insert(k.to_owned(), p);
                        }
                        None => w
                            .coverage
                            .unsupported("driven replay: agent: a path definition it can't read"),
                    }
                }
            }
            // An agent joins or leaves `AllAgents` (`OnEnable`/`OnDisable`, the view enabled or
            // disabled: not modelled, the recording's); `OnDisable` also leaves its obstacle group.
            "ag_enable" => {
                self.ag_input("an agent enabled or disabled");
                match (r["u"].as_str().and_then(|x| w.by_id.get(x).copied()), r["on"].as_bool()) {
                    (Some(u), Some(true)) => {
                        if !self.ag_all.contains(&u) {
                            self.ag_all.push(u);
                        }
                    }
                    (Some(u), Some(false)) => {
                        self.ag_all.retain(|x| *x != u);
                        if let Err(d) = ag::remove_from_group(w, u, self) {
                            w.coverage
                                .unsupported(format!("driven replay: agent: OnDisable: {}", d.0));
                        }
                    }
                    _ => w
                        .coverage
                        .unsupported("driven replay: agent: an enable record it can't read"),
                }
            }
            // FindPath's callback ran (the AI's path delivered): `m_RequestedFindPath = null`.
            "ag_findclr" => {
                self.ag_input("FindPath's delivery");
                if let Some(u) = r["u"].as_str().and_then(|x| w.by_id.get(x).copied())
                    && let Some(unit) = w.unit_mut(u)
                {
                    let held = unit.combat.agent.requested_find.as_ref().map(|p| p.key.clone());
                    unit.combat.agent.requested_find = None;
                    if held.as_deref() != r["held"].as_str() {
                        w.coverage.unsupported("driven replay: agent: FindPath's callback: the sim's requested path differs from the recording's");
                    }
                }
            }
            "ag_ctl" => return self.ag_entry(w, i, AgEntry::Ctl),
            "ag_req" => return self.ag_entry(w, i, AgEntry::Req),
            "ag_done" => return self.ag_entry(w, i, AgEntry::Done),
            "ag_call" => return self.ag_entry(w, i, AgEntry::Call),
            // The place-on-ground step outside a `TickUnit` (its own controller): physics, an input.
            "ag_ground" => {
                self.ag_input("ground (outside TickUnit)");
                if let (Some(u), Some(after)) =
                    (r["u"].as_str().and_then(|x| w.by_id.get(x).copied()), v3(&r["after"]))
                {
                    if let Some(unit) = w.unit_mut(u) {
                        unit.position = after;
                        unit.combat.agent.pos = after;
                    }
                } else {
                    w.coverage.unsupported(
                        "driven replay: agent: a place-on-ground record it can't read",
                    );
                }
            }
            // `IsDone()` read by the commands (their `rmoving` leaf: still recorded), the AI's
            // movement simulation's physics (its `msim` leaf: still recorded).
            "ag_v" if r["f"] == "isdone" => self.ag_input("isdone (outside the agent)"),
            "ag_phys" if r["ctx"] == "sim" => self.ag_input("physics (the AI's simulation)"),
            "ag_v" if r["f"] == "corp" || r["f"] == "speedmps" || r["f"] == "cells" => {
                // FindPath's reads (not ported: the AI's requests are inputs).
                self.ag_input("FindPath's reads");
            }
            _ => w.coverage.unsupported(format!(
                "driven replay: agent: a record outside the sim's agent entries ({})",
                describe(r)
            )),
        }
        i + 1
    }

    /// `ag_init`: an agent's state at the window's first tick (taken), with the unit's position
    /// and orientation (taken: the sim doesn't move or turn units before the window).
    fn ag_init(&mut self, w: &mut World, r: &J) {
        if r["kind"] != "UnitMovementAgent" {
            w.coverage.unsupported(
                "driven replay: agent: an agent of another kind (UnitMovementAgentContinuous?)",
            );
            return;
        }
        let Some(u) = r["u"].as_str().and_then(|x| w.by_id.get(x).copied()) else {
            w.coverage.unsupported("driven replay: agent: an agent of a unit the sim doesn't have");
            return;
        };
        let Some(agent) = agent_from(&r["s"], &self.ag_paths) else {
            w.coverage.unsupported("driven replay: agent: an initial state it can't read");
            return;
        };
        let (upos, orient) = (v3(&r["upos"]), f(&r["orient"]));
        // The sim doesn't move units before the window (a roaming unit walks there): the unit's
        // position at the window's start is the recording's.
        let Some(upos) = upos else {
            w.coverage
                .unsupported("driven replay: agent: an initial state without the unit's position");
            return;
        };
        if Some(bits3(upos)) != w.unit(u).map(|x| bits3(x.position)) {
            self.ag_input("position at the window's start (moved before it)");
        }
        let Some(unit) = w.unit_mut(u) else { return };
        unit.position = upos;
        unit.combat.agent = agent;
        if let Some(o) = orient {
            unit.orientation = o;
        }
        // `UnitMovementAgentBase.AllAgents` holds the enabled agents (a disabled view's unit is
        // still ticked by UnitMoveController).
        if r["enabled"] != false {
            self.ag_all.push(u);
        }
        self.ag_last_state.insert(u, r["s"].clone());
        self.ag_input("initial state");
    }

    /// An entry: the sim's code runs, pulling its records; on a desync the entry's remaining
    /// records are skipped up to its end.
    fn ag_entry(&mut self, w: &mut World, i: usize, kind: AgEntry) -> usize {
        let trace = self.trace;
        let r = &trace.records[i];
        let Some(u) = r["u"].as_str().and_then(|x| w.by_id.get(x).copied()) else {
            w.coverage.unsupported(format!(
                "driven replay: agent: an entry of a unit the sim doesn't have ({})",
                describe(r)
            ));
            return i + 1;
        };
        self.out.ag_entries += 1;
        self.i = i;
        let id = self.ag_unit_id(w, u);
        let (end_ev, f) = match kind {
            AgEntry::Ctl => ("ag_ctl_end", None),
            AgEntry::Req => ("ag_req_end", None),
            AgEntry::Done => ("ag_done_end", None),
            AgEntry::Call => ("ag_call_end", r["fn"].as_str().map(str::to_owned)),
        };
        // Real time (the walk up before combat) isn't ported: the entry is taken from the
        // recording (its states, the unit's position and orientation after).
        if !w.combat.turn_based {
            self.ag_input("a real-time entry (taken)");
            self.ag_after_rt.extend(self.ag_all.iter().copied());
            self.ag_after_rt.extend(self.ag_last_state.keys().copied());
            if kind == AgEntry::Ctl {
                self.place(w, Phase::Move, "UnitMoveController.TickUnit");
            }
            if let Err(d) = self.ag_take_span(w, u, end_ev, f.as_deref()) {
                w.coverage.unsupported(format!("driven replay: agent: a real-time entry: {}", d.0));
            }
            return self.i;
        }
        let res = match kind {
            AgEntry::Ctl => self.ag_tick_unit(w, u),
            AgEntry::Req => self.ag_path_to(w, u, r),
            AgEntry::Done => self.ag_done(w, u, r),
            AgEntry::Call => self.ag_call_entry(w, u, r),
        };
        if let Err(d) = res {
            if std::env::var_os("WR_DEBUG_AGENT").is_some() {
                eprintln!("DEBUG agent tick {}: {}", self.tick, d.0);
            }
            w.coverage.unsupported(format!("driven replay: agent: {}", d.0));
            // The rest of the entry, up to its end record, is skipped (its agent records too).
            while let Some(x) = trace.records.get(self.i) {
                if x["ev"] == end_ev
                    && x["u"].as_str() == Some(id.as_str())
                    && f.as_deref().is_none_or(|f| x["fn"] == f)
                {
                    self.i += 1;
                    if let Some(s) = x.get("s") {
                        let s = s.clone();
                        self.ag_check_state(w, u, &s, "after a desynced entry");
                    }
                    break;
                }
                if x["k"] == "tick" {
                    break;
                }
                if self.ag_stop(x) {
                    self.ag_skip_draw(w, x);
                    self.i += 1;
                    continue;
                }
                let at = self.i;
                self.i = self.record(w, at);
            }
        }
        self.i
    }

    /// `ag_ctl` .. `ag_ctl_end`: `UnitMoveController.TickUnit`.
    fn ag_tick_unit(&mut self, w: &mut World, u: EntityIdx) -> R<()> {
        self.place(w, Phase::Move, "UnitMoveController.TickUnit");
        let r = self.ag_take(w, u, "ag_ctl", "TickUnit")?;
        let pos = w.unit(u).map(|x| x.position).unwrap_or_default();
        if v3(&r["pos"]).map(bits3) != Some(bits3(pos)) {
            // A unit coming back to the controller after frames without it (asleep) may have
            // been moved meanwhile by code the sim doesn't run (roaming while asleep): its
            // position is taken. Ticked in the previous frame, it's a desync.
            let back = self.ag_last_ctl.get(&u).is_none_or(|t| *t + 1 < self.tick);
            match (back, v3(&r["pos"])) {
                (true, Some(p)) => {
                    self.ag_input("position moved while not ticked (roaming asleep)");
                    if let Some(unit) = w.unit_mut(u) {
                        unit.position = p;
                    }
                }
                _ => {
                    return Err(Desync(format!(
                        "TickUnit: the unit's position {pos:?} differs from the recording's {}",
                        r["pos"]
                    )));
                }
            }
        }
        self.ag_last_ctl.insert(u, self.tick);
        movement::tick_unit(w, u, self)?;
        let e = self.ag_take(w, u, "ag_ctl_end", "TickUnit's end")?;
        let unit = w.unit(u).ok_or_else(|| Desync("no unit".into()))?;
        let (pos, view, orient) = (unit.position, unit.combat.agent.pos, unit.orientation);
        if v3(&e["pos"]).map(bits3) != Some(bits3(pos))
            || v3(&e["vpos"]).map(bits3) != Some(bits3(view))
            || f(&e["orient"]).map(f32::to_bits) != Some(orient.to_bits())
        {
            return Err(Desync(format!(
                "TickUnit's end: position {pos:?}, view {view:?}, orientation {orient}; the recording's {}, {}, {}",
                e["pos"], e["vpos"], e["orient"]
            )));
        }
        self.out.ag_ticks += 1;
        Ok(())
    }

    /// `ag_req` .. `ag_req_end`: `PathTo` (the sim's) or `FindPath` (the AI's: not ported, its
    /// state after taken).
    fn ag_path_to(&mut self, w: &mut World, u: EntityIdx, r: &J) -> R<()> {
        if r["fn"] == "find" {
            let id = self.ag_unit_id(w, u);
            self.i += 1;
            let at = self.ag_skip_to(w, |x| {
                x["ev"] == "ag_req_end" && x["u"].as_str() == Some(id.as_str())
            })?;
            let s = self.trace.records[at]["s"].clone();
            let agent = agent_from(&s, &self.ag_paths)
                .ok_or_else(|| Desync("FindPath's state after: unreadable".into()))?;
            if let Some(unit) = w.unit_mut(u) {
                unit.combat.agent = agent;
            }
            self.ag_last_state.insert(u, s);
            self.ag_input("FindPath (the AI's request)");
            return Ok(());
        }
        let num = |k: &str| f(&r[k]).ok_or_else(|| Desync(format!("PathTo: no {k}")));
        let dest = if r["dhas"] == true {
            Some(DestAgent {
                view: r["dview"].as_str().map(str::to_owned),
                attached: r["dattached"] == true,
            })
        } else {
            None
        };
        let point = v3(&r["p"]).ok_or_else(|| Desync("PathTo: no destination".into()))?;
        let command = r["c"].as_u64().and_then(|c| u32::try_from(c).ok());
        let (radius, max_radius) = (num("r")?, num("mr")?);
        ag::path_to(
            w,
            u,
            command,
            point,
            radius,
            max_radius,
            r["xz"] == true,
            dest,
            r["dself"] == true,
            self,
        )?;
        Ok(())
    }

    /// `ag_done` .. `ag_done_end`: `OnPathComplete` with the delivered path (its contents from
    /// the recording until paths are computed: docs/poc5/paths.md).
    fn ag_done(&mut self, w: &mut World, u: EntityIdx, r: &J) -> R<()> {
        let key =
            r["path"].as_str().ok_or_else(|| Desync("OnPathComplete: no path".into()))?.to_owned();
        let pts: Vec<V3> = r["v"]
            .as_array()
            .ok_or_else(|| Desync("OnPathComplete: no points".into()))?
            .iter()
            .map(v3)
            .collect::<Option<_>>()
            .ok_or_else(|| Desync("OnPathComplete: unreadable points".into()))?;
        self.ag_paths.insert(key.clone(), pts.clone());
        self.ag_input("path contents");
        let p = AgentPath { forced: key.starts_with("?ForcedPath"), key, points: pts };
        ag::on_path_complete(w, u, &p, r["err"] == true, r["forced"] == true, self)
    }

    /// A top-level `ag_call`: the agent's method as the game called it (from code the sim
    /// doesn't run yet: an input, counted by its caller).
    fn ag_call_entry(&mut self, w: &mut World, u: EntityIdx, r: &J) -> R<()> {
        let origin = r["origin"].as_str().unwrap_or("");
        let caller = origin.split(" < ").nth(1).unwrap_or("?");
        self.ag_input(match caller {
            "Kingmaker.View.UnitEntityView.StopMoving" => "Stop (StopMoving)",
            "Kingmaker.UnitLogic.Commands.Base.UnitCommand.TickApproaching" => {
                "FollowPrecomputedPath (TickApproaching)"
            }
            _ => "a call from elsewhere",
        });
        let fname = r["fn"].as_str().unwrap_or("");
        match fname {
            "Stop" => ag::stop(w, u, self),
            "CompleteMovement" => ag::complete_movement(w, u, self),
            "InterruptMovement" => ag::interrupt_movement(w, u, self),
            "SetWaypoint" => {
                let i = r["i"]
                    .as_i64()
                    .and_then(|x| i32::try_from(x).ok())
                    .ok_or_else(|| Desync("SetWaypoint: no index".into()))?;
                ag::set_waypoint(w, u, i, self)
            }
            "FollowPrecomputedPath" | "ForcePath" => {
                if let Some(s) = r.get("s") {
                    let s = s.clone();
                    self.ag_check_state(w, u, &s, &format!("{fname}'s start"));
                }
                let key = r["rq"]["path"]
                    .as_str()
                    .ok_or_else(|| Desync(format!("{fname}: no path")))?
                    .to_owned();
                let pts: Vec<V3> = r["v"]
                    .as_array()
                    .ok_or_else(|| Desync(format!("{fname}: no points")))?
                    .iter()
                    .map(v3)
                    .collect::<Option<_>>()
                    .ok_or_else(|| Desync(format!("{fname}: unreadable points")))?;
                self.ag_paths.insert(key.clone(), pts.clone());
                let p = AgentPath { forced: key.starts_with("?ForcedPath"), key, points: pts };
                let arg = |k: usize| r["args"][k].as_str().unwrap_or("").to_owned();
                // The nested completion's error flag (the call doesn't record one).
                let err = self.trace.records[self.i..]
                    .iter()
                    .take_while(|x| x["k"] != "tick")
                    .find(|x| x["ev"] == "ag_done")
                    .is_some_and(|x| x["err"] == true);
                if fname == "FollowPrecomputedPath" {
                    let max = arg(1)
                        .parse::<f32>()
                        .map_err(|_| Desync("FollowPrecomputedPath: no radius".into()))?;
                    ag::follow_precomputed_path(w, u, &p, err, arg(0) == "1", max, self)
                } else {
                    ag::force_path(w, u, &p, err, arg(0) == "1", self)
                }
            }
            _ => Err(Desync(format!("an agent call the sim doesn't model ({fname})"))),
        }
    }

    /// A span taken from the recording, up to the unit's end record `end_ev` (`f`: a call's): the
    /// unit's agent becomes the last state the span records, and a `TickUnit`'s end gives its
    /// position, view and orientation.
    fn ag_take_span(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        end_ev: &str,
        f: Option<&str>,
    ) -> R<()> {
        let id = self.ag_unit_id(w, u);
        let trace = self.trace;
        let mut last: Option<J> = None;
        loop {
            let Some(x) = trace.records.get(self.i) else {
                return Err(Desync("the recording ends".into()));
            };
            let mine = x["k"] == "obs" && x["u"].as_str() == Some(id.as_str());
            if mine
                && x.get("s").is_some_and(J::is_object)
                && x["ev"].as_str().is_some_and(|e| e.starts_with("ag_"))
            {
                last = Some(x["s"].clone());
            }
            if mine && x["ev"] == end_ev && f.is_none_or(|f| x["fn"] == f) {
                self.i += 1;
                if end_ev == "ag_ctl_end" {
                    let (pos, view, orient) =
                        (v3(&x["pos"]), v3(&x["vpos"]), self::f(&x["orient"]));
                    let unit = w.unit_mut(u).ok_or_else(|| Desync("no unit".into()))?;
                    if let (Some(p), Some(v), Some(o)) = (pos, view, orient) {
                        unit.position = p;
                        unit.orientation = o;
                        unit.combat.agent.pos = v;
                    } else {
                        return Err(Desync("a TickUnit end it can't read".into()));
                    }
                }
                break;
            }
            if x["k"] == "tick" {
                return Err(Desync("an agent entry without its end in its frame".into()));
            }
            if self.ag_stop(x) {
                self.ag_skip_draw(w, x);
                self.i += 1;
                continue;
            }
            let i = self.i;
            self.i = self.record(w, i);
        }
        if let Some(s) = last {
            let a = agent_from(&s, &self.ag_paths)
                .ok_or_else(|| Desync("a recorded state it can't read".into()))?;
            let view = w.unit(u).map(|x| x.combat.agent.pos);
            let unit = w.unit_mut(u).ok_or_else(|| Desync("no unit".into()))?;
            unit.combat.agent = a;
            // (A TickUnit's view position is its end's; the agent's state was recorded before.)
            if end_ev == "ag_ctl_end"
                && let Some(v) = view
            {
                unit.combat.agent.pos = v;
            }
            self.ag_last_state.insert(u, s);
        }
        Ok(())
    }

    /// Records up to `want`'s (returned), the agent's skipped (an input's span).
    fn ag_skip_to(&mut self, w: &mut World, want: impl Fn(&J) -> bool) -> R<usize> {
        let trace = self.trace;
        loop {
            let Some(r) = trace.records.get(self.i) else {
                return Err(Desync("the recording ends".into()));
            };
            if want(r) {
                self.i += 1;
                return Ok(self.i - 1);
            }
            if r["k"] == "tick" {
                return Err(Desync("an agent span without its end in its frame".into()));
            }
            if self.ag_stop(r) {
                self.ag_skip_draw(w, r);
                self.i += 1;
                continue;
            }
            let i = self.i;
            self.i = self.record(w, i);
        }
    }

    /// A skipped `SlowDown` draw keeps the tape's queue for its site in step.
    fn ag_skip_draw(&mut self, w: &mut World, r: &J) {
        if self.ag_slow_draw(r) {
            let _ = w.rng.range_float(crate::rng::sites::AGENT_SLOW_DOWN, 0.2, 0.3);
        }
    }

    /// A path delivered (`PathSync`'s `path_ret`): an agent's `FindPath` request (the AI's)
    /// clears `m_RequestedFindPath` in its callback.
    pub(super) fn ag_path_ret(&mut self, w: &mut World, r: &J) {
        // A capture with FindPath's own callback record (`ag_findclr`, since poc5-tb-109) takes
        // that instead.
        if self.ag_has_findclr {
            return;
        }
        let Some(key) = r["path"].as_str() else { return };
        for u in self.ag_last_state.keys().copied().collect::<Vec<_>>() {
            if let Some(unit) = w.unit_mut(u)
                && unit.combat.agent.requested_find.as_ref().is_some_and(|p| p.key == key)
            {
                unit.combat.agent.requested_find = None;
            }
        }
    }

    /// The draw `SlowDown` makes (`Random.Range(0.2f, 0.3f)`), from the tape.
    fn ag_draw(&mut self, w: &mut World, min: f32, max: f32) -> R<f32> {
        let at = self.ag_pull(w, "SlowDown's draw", |r| r["k"] == "draw")?;
        let r = &self.trace.records[at];
        let site = crate::rng::sites::AGENT_SLOW_DOWN;
        let args_ok = r["a"].as_array().is_some_and(|a| {
            a.len() == 2
                && f(&a[0]).map(f32::to_bits) == Some(min.to_bits())
                && f(&a[1]).map(f32::to_bits) == Some(max.to_bits())
        });
        if !(self.ag_slow_draw(r) && r["m"] == "Range_ff" && r["c"] == "sim" && args_ok) {
            return Err(Desync(format!("SlowDown's draw: the recording has {}", describe(r))));
        }
        let v = w.rng.range_float(site, min, max);
        if f(&r["v"]).map(f32::to_bits) != Some(v.to_bits()) {
            return Err(Desync(format!(
                "SlowDown's draw {v} differs from the recorded {}",
                r["v"]
            )));
        }
        self.out.ag_draws += 1;
        Ok(v)
    }

    fn ag_state_of(&self, u: EntityIdx, k: &str) -> Option<bool> {
        self.ag_last_state.get(&u).and_then(|s| s[k].as_bool())
    }
}

impl AgentInputs for Driver<'_> {
    fn is_done(&mut self, w: &mut World, u: EntityIdx, path: &str) -> R<bool> {
        let r = self.ag_leaf(w, u, "isdone")?;
        if r["path"].as_str() != Some(path) {
            return Err(Desync(format!("IsDone of {path}: the recording's is of {}", r["path"])));
        }
        let v = r["v"].as_bool().ok_or_else(|| Desync("isdone without a value".into()))?;
        // A path the agent holds was delivered: done (a forced path is never computed).
        if w.unit(u).is_some_and(|x| {
            x.combat.agent.path.as_ref().is_some_and(|p| p.key == path && !p.forced)
        }) && !v
        {
            return Err(Desync(format!("the delivered path {path} isn't done in the recording")));
        }
        Ok(v)
    }
    fn has_current_turn(&mut self, w: &mut World) -> R<bool> {
        Ok(w.combat.current_unit.is_some())
    }
    fn current_and_acting(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        Ok(w.combat.current_unit == Some(u)
            && w.combat.turns.ctl.status == crate::combat::turn_tick::Status::Acting)
    }
    fn turn_tick_movement(&mut self, w: &mut World, u: EntityIdx, dt: f32, force: bool) -> R<f32> {
        let id = self.ag_unit_id(w, u);
        let near = ag::near_the_end(w, u)?;
        let at = self.ag_pull(w, "the turn's TickMovement (cd_move)", |r| {
            r["k"] == "obs" && r["ev"] == "cd_move" && r["u"].as_str() == Some(id.as_str())
        })?;
        let r = self.trace.records[at].clone();
        let num = |k: &str| r[k].as_str().and_then(|x| x.parse::<f32>().ok());
        if num("dt").map(f32::to_bits) != Some(dt.to_bits())
            || r["force"].as_bool() != Some(force)
            || r["near"].as_bool() != Some(near)
        {
            return Err(Desync(format!(
                "TickMovement({dt}, force {force}, near {near}): the turn's call was {}",
                describe(&r)
            )));
        }
        // The cooldowns' own handler runs the turn's budget (its other fields stay inputs).
        self.i = at;
        let i = self.i;
        self.i = self.record(w, i);
        num("dt_out").ok_or_else(|| Desync("cd_move without dt_out".into()))
    }
    fn cutscene_active(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        let held = self.ag_leaf(w, u, "csctl")?["v"] == true;
        Ok(held && self.ag_leaf(w, u, "csactive")?["v"] == true)
    }
    fn move_dont_interrupt(&mut self, _: &mut World, u: EntityIdx) -> R<bool> {
        // The move command's flag (`ag_tick.dontint`: an input until the commands model has it).
        self.ag_input("DontInterruptInTurnBasedMode");
        let _ = u;
        Ok(self.ag_tick_rec.as_ref().is_some_and(|t| t["dontint"] == true))
    }
    fn nav_obstacles(&mut self, _: &mut World) -> R<bool> {
        Ok(self.ag_tick_rec.as_ref().is_some_and(|t| t["s"]["navobst"] == true))
    }
    fn gamepad(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        Ok(self.ag_leaf(w, u, "gamepad")?["v"] == true)
    }
    fn random_range(&mut self, w: &mut World, _: EntityIdx, min: f32, max: f32) -> R<f32> {
        self.ag_draw(w, min, max)
    }
    fn rider_in_combat(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(false)
    }
    fn anim_prevents(&mut self, w: &mut World, u: EntityIdx) -> R<Option<bool>> {
        let id = self.ag_unit_id(w, u);
        let next = self.ag_peek();
        if next.is_some_and(|r| {
            r["ev"] == "ag_v"
                && r["f"] == "animprev"
                && (r["u"].is_null() || r["u"].as_str() == Some(id.as_str()))
        }) {
            return Ok(Some(self.ag_leaf(w, u, "animprev")?["v"] == true));
        }
        Ok(None)
    }
    fn commands_prevent(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        Ok(self.ag_leaf(w, u, "cmdprev")?["v"] == true)
    }
    fn max_speed(&mut self, w: &mut World, u: EntityIdx) -> R<f32> {
        f(&self.ag_leaf(w, u, "maxspeed")?["v"])
            .ok_or_else(|| Desync("maxspeed without a value".into()))
    }
    fn fly_height(&mut self, w: &mut World, u: EntityIdx) -> R<f32> {
        f(&self.ag_leaf(w, u, "fly")?["v"]).ok_or_else(|| Desync("fly without a value".into()))
    }
    fn raycast(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        o: V3,
        d: V3,
        max: f32,
        mask: i32,
    ) -> R<Option<f32>> {
        self.ag_phys(w, u, "ray", o, &d, max, mask)
    }
    fn disc_cast(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        o: V3,
        r: f32,
        max: f32,
        mask: i32,
    ) -> R<Option<f32>> {
        self.ag_phys(w, u, "disc", o, &[r], max, mask)
    }
    fn in_combat(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        Ok(w.unit(u).is_some_and(|x| x.combat.in_combat))
    }
    fn movement_interrupted(&mut self, w: &mut World, u: EntityIdx, dest: V3) -> R<()> {
        self.ag_callback(w, u, "OnMovementInterrupted", Some(dest), None)
    }
    fn movement_complete(&mut self, w: &mut World, u: EntityIdx) -> R<()> {
        self.ag_callback(w, u, "OnMovementComplete", None, None)
    }
    fn waypoint_update(&mut self, w: &mut World, u: EntityIdx, index: i32) -> R<()> {
        self.ag_callback(w, u, "OnMovementWaypointUpdate", None, Some(index))
    }
    fn call(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        fname: &'static str,
        index: Option<i32>,
    ) -> R<()> {
        let r = self.ag_take(w, u, "ag_call", &format!("the agent's {fname}"))?;
        if r["fn"] != fname || r["i"].as_i64().and_then(|x| i32::try_from(x).ok()) != index {
            return Err(Desync(format!(
                "the agent calls {fname}({index:?}), the recording has {}",
                describe(&r)
            )));
        }
        Ok(())
    }
    fn call_end(&mut self, w: &mut World, u: EntityIdx, fname: &'static str) -> R<()> {
        let r = self.ag_take(w, u, "ag_call_end", &format!("{fname}'s end"))?;
        if r["fn"] != fname {
            return Err(Desync(format!("{fname} returned; the recording has {}", describe(&r))));
        }
        self.ag_check_state(w, u, &r["s"], &format!("{fname}'s end"));
        Ok(())
    }
    fn tick_start(&mut self, w: &mut World, u: EntityIdx, dt: f32) -> R<()> {
        let r = self.ag_take(w, u, "ag_tick", "TickMovement")?;
        if f(&r["dt"]).map(f32::to_bits) != Some(dt.to_bits()) {
            return Err(Desync(format!("TickMovement({dt}): the recording's is {}", r["dt"])));
        }
        // The turn state `CanMoveInTurnBased` reads, the sim's against the recording's.
        let turn = w.combat.current_unit.is_some();
        let cur_acting = self.current_and_acting(w, u)?;
        let combat = w.unit(u).is_some_and(|x| x.combat.in_combat);
        if r["tb"] != true
            || r["turn"].as_bool() != Some(turn)
            || (r["cur"] == true && r["acting"] == true) != cur_acting
            || r["combat"].as_bool() != Some(combat)
        {
            return Err(Desync(format!(
                "TickMovement's turn state (turn {turn}, current and acting {cur_acting}, in combat {combat}) differs from the recording's ({})",
                describe(&r)
            )));
        }
        self.ag_check_state(w, u, &r["s"], "TickMovement's start");
        self.ag_tick_rec = Some(r);
        Ok(())
    }
    fn tick_end(&mut self, w: &mut World, u: EntityIdx, ret: V3) -> R<()> {
        let r = self.ag_take(w, u, "ag_tick_end", "TickMovement's end")?;
        if v3(&r["ret"]).map(bits3) != Some(bits3(ret)) {
            return Err(Desync(format!("TickMovement returned {ret:?}, the game {}", r["ret"])));
        }
        self.ag_check_state(w, u, &r["s"], "TickMovement's end");
        self.ag_tick_rec = None;
        Ok(())
    }
    fn done_start(&mut self, w: &mut World, u: EntityIdx, path: &str, forced: bool) -> R<()> {
        let r = self.ag_take(w, u, "ag_done", "OnPathComplete")?;
        if r["path"].as_str() != Some(path) || r["forced"].as_bool() != Some(forced) {
            return Err(Desync(format!(
                "OnPathComplete({path}, {forced}): the recording's is {}",
                describe(&r)
            )));
        }
        if r["combat"].as_bool() != w.unit(u).map(|x| x.combat.in_combat) {
            return Err(Desync(
                "OnPathComplete: the unit's combat state differs from the recording's".into(),
            ));
        }
        self.ag_check_state(w, u, &r["s"], "OnPathComplete's start");
        Ok(())
    }
    fn done_end(&mut self, w: &mut World, u: EntityIdx) -> R<()> {
        let r = self.ag_take(w, u, "ag_done_end", "OnPathComplete's end")?;
        self.ag_check_state(w, u, &r["s"], "OnPathComplete's end");
        Ok(())
    }
    fn req_start(&mut self, w: &mut World, u: EntityIdx) -> R<()> {
        let r = self.ag_take(w, u, "ag_req", "PathTo")?;
        if r["tb"] != true {
            return Err(Desync("PathTo outside turn-based combat (real time isn't ported)".into()));
        }
        self.ag_check_state(w, u, &r["s"], "PathTo's start");
        self.ag_req_rec = Some(r);
        Ok(())
    }
    fn req_end(&mut self, w: &mut World, u: EntityIdx, request: Option<&PathRequest>) -> R<()> {
        let r = self.ag_take(w, u, "ag_req_end", "PathTo's end")?;
        let made = r["s"]["req"]
            != self.ag_req_rec.as_ref().map_or(J::Null, |x| x["s"]["req"].clone())
            && !r["rq"].is_null();
        match (request, made) {
            (Some(q), true) => {
                let rq = &r["rq"];
                let same = rq["kind"] == "ABPathWithClearance"
                    && rq["ec"] == "LosPathConditionForGrid"
                    && rq["path"].as_str() == Some(q.key.as_str())
                    && v3(&rq["start"]).map(bits3) == Some(bits3(q.start))
                    && v3(&rq["end"]).map(bits3) == Some(bits3(q.end))
                    && v3(&rq["clear"]).map(bits3) == Some(bits3(q.clearance))
                    && rq["tags"].as_i64() == Some(i64::from(q.tags))
                    && rq["gmask"].as_i64() == Some(i64::from(q.graph_mask))
                    && f(&rq["ar"]).map(f32::to_bits) == Some(q.approach.to_bits())
                    && rq["los"].as_bool() == Some(q.los)
                    && f(&rq["ibr"]).map(f32::to_bits) == Some(q.ignore_blocker.to_bits())
                    && f(&rq["maxd"]).map(f32::to_bits) == Some(q.max_distance.to_bits())
                    && rq["xz"].as_bool() == Some(q.distance_xz);
                if !same {
                    return Err(Desync(format!("PathTo's request {q:?}: the recording's is {rq}")));
                }
                self.out.ag_requests += 1;
            }
            (None, false) => {}
            (Some(_), false) => return Err(Desync("PathTo made a request the game didn't".into())),
            (None, true) => {
                return Err(Desync("the game's PathTo made a request the sim's didn't".into()));
            }
        }
        self.ag_check_state(w, u, &r["s"], "PathTo's end");
        self.ag_req_rec = None;
        Ok(())
    }
    fn group_memory(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        Ok(self.ag_leaf(w, u, "gmem")?["v"] == true)
    }
    fn obstacles_group(&mut self, _: &mut World, u: EntityIdx) -> R<bool> {
        // `ObstaclesGroup != null`: the groups serve real-time avoidance (not ported); the
        // agent's last recorded state has it.
        self.ag_input("obstacle group");
        self.ag_state_of(u, "ogroup")
            .ok_or_else(|| Desync("no recorded state with the obstacle group".into()))
    }
    fn saddled(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(false)
    }
    fn dest_proxy(&mut self, w: &mut World, u: EntityIdx, _: &str) -> R<Proxy> {
        let r = self.ag_leaf(w, u, "proxy")?;
        if r["unit"] != true {
            return Ok(Proxy::NotUnit);
        }
        Ok(Proxy::Unit(if r["agent"] == true {
            r["aview"].as_str().map(str::to_owned)
        } else {
            None
        }))
    }
    fn accelerate_if_long(&mut self, _: &mut World, _: EntityIdx, _: &AgentPath) -> R<()> {
        // The commands' acceleration (no records; its effect reaches the agent as MaxSpeed).
        Ok(())
    }
    fn unreachable(&mut self, w: &mut World, u: EntityIdx, dest_view: &str) -> R<()> {
        let r = self.ag_leaf(w, u, "unreach")?;
        if r["v"].as_str() != Some(dest_view) {
            return Err(Desync(format!(
                "UnreachableUnits.Add({dest_view}): the recording's is {}",
                describe(&r)
            )));
        }
        // (The AI reads `UnreachableUnits` as its own recorded leaf.)
        Ok(())
    }
    fn movement_started(&mut self, _: &mut World, _: EntityIdx, _: V3) -> R<()> {
        Ok(())
    }
    fn has_commands(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(true)
    }
    fn soft_collider_closest(&mut self, w: &mut World, u: EntityIdx, point: V3) -> R<Option<V3>> {
        if self.ag_leaf(w, u, "softcol")?["v"] != true {
            return Ok(None);
        }
        let r = self.ag_leaf(w, u, "closest")?;
        if v3(&r["p"]).map(bits3) != Some(bits3(point)) {
            return Err(Desync(format!(
                "ClosestPoint({point:?}): the recording's is of {}",
                r["p"]
            )));
        }
        v3(&r["v"]).map(Some).ok_or_else(|| Desync("closest without a value".into()))
    }
    fn prone(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        // Read by `ConnectToGroups` inside `Stop` (its only caller), which can't change it: the
        // Stop's end state has it (a unit going prone is stopped by `EnterProneState`).
        self.ag_input("prone");
        let id = self.ag_unit_id(w, u);
        self.trace.records[self.i..]
            .iter()
            .take_while(|x| x["k"] != "tick")
            .find(|x| {
                x["ev"] == "ag_call_end"
                    && x["fn"] == "Stop"
                    && x["u"].as_str() == Some(id.as_str())
            })
            .and_then(|x| x["s"]["prone"].as_bool())
            .ok_or_else(|| Desync("IsProne: no end of the Stop that reads it".into()))
    }
    fn repath_all(&mut self, w: &mut World) -> R<()> {
        for u in self.ag_all.clone() {
            if let Some(x) = w.unit_mut(u) {
                x.combat.agent.repath = true;
            }
        }
        Ok(())
    }
    fn on_navmesh(&mut self, w: &mut World, u: EntityIdx, pos: V3) -> R<bool> {
        let r = self.ag_leaf(w, u, "navnode")?;
        if v3(&r["p"]).map(bits3) != Some(bits3(pos)) {
            return Err(Desync(format!(
                "GetNearestNode({pos:?}): the recording's is of {}",
                r["p"]
            )));
        }
        Ok(r["v"] == true)
    }
    fn frames(&mut self, _: &mut World) -> R<(i32, i32)> {
        self.ag_input("frames");
        let r = self
            .ag_req_rec
            .as_ref()
            .ok_or_else(|| Desync("PathTo's frames outside PathTo".into()))?;
        let n = |k: &str| r[k].as_i64().and_then(|x| i32::try_from(x).ok());
        n("frame").zip(n("tile")).ok_or_else(|| Desync("ag_req without its frames".into()))
    }
    fn init_move_direction(&mut self, w: &mut World, u: EntityIdx) -> R<V2> {
        let o = w.unit(u).ok_or_else(|| Desync("no unit".into()))?.orientation;
        Ok(orientation::init_move_direction(o))
    }
    fn corpulence(&mut self, w: &mut World, u: EntityIdx) -> R<f32> {
        f(&self.ag_leaf(w, u, "corp")?["v"]).ok_or_else(|| Desync("corp without a value".into()))
    }
    fn size_in_cells(&mut self, w: &mut World, u: EntityIdx) -> R<i32> {
        self.ag_leaf(w, u, "cells")?["v"]
            .as_i64()
            .and_then(|x| i32::try_from(x).ok())
            .ok_or_else(|| Desync("cells without a value".into()))
    }
    fn grid_erosion(&mut self, _: &mut World) -> R<Option<(i32, i32)>> {
        self.ag_erosion.clone().map_err(Desync)
    }
    fn fudge_radius(&mut self, w: &mut World, u: EntityIdx, command: Option<u32>) -> R<f32> {
        if command.is_none() {
            return Ok(0.0);
        }
        f(&self.ag_leaf(w, u, "fudge")?["v"]).ok_or_else(|| Desync("fudge without a value".into()))
    }
    fn speed_mps(&mut self, w: &mut World, u: EntityIdx) -> R<f32> {
        f(&self.ag_leaf(w, u, "speedmps")?["v"])
            .ok_or_else(|| Desync("speedmps without a value".into()))
    }
    fn move_command(&mut self, w: &mut World, u: EntityIdx) -> R<Option<(f32, bool)>> {
        let r = self.ag_leaf(w, u, "move")?;
        if r["v"] != true {
            return Ok(None);
        }
        Ok(Some((
            f(&r["delay"]).ok_or_else(|| Desync("move without its delay".into()))?,
            r["roaming"] == true,
        )))
    }
    fn request_key(&mut self, w: &mut World, u: EntityIdx) -> R<String> {
        // `PathSync`'s key (`<unit>#<n>`): the next `ag_req_end`'s request (an input).
        let id = self.ag_unit_id(w, u);
        let key = self.trace.records[self.i..]
            .iter()
            .take_while(|x| x["k"] != "tick")
            .find(|x| x["ev"] == "ag_req_end" && x["u"].as_str() == Some(id.as_str()))
            .and_then(|x| x["rq"]["path"].as_str().map(str::to_owned));
        self.ag_input("request key");
        key.ok_or_else(|| Desync("PathTo made a request; the recording has none".into()))
    }
}

impl MoveInputs for Driver<'_> {
    fn can_move(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        Ok(self.ag_leaf(w, u, "canmove")?["v"] == true)
    }
    fn can_rotate(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        Ok(self.ag_leaf(w, u, "canrotate")?["v"] == true)
    }
    fn interrupt_move(&mut self, _: &mut World, _: EntityIdx) -> R<()> {
        Err(Desync("TickUnit interrupts a move it can't make (UnitCommands.InterruptMove from the controller isn't wired)".into()))
    }
    fn place_on_ground(&mut self, w: &mut World, u: EntityIdx, required: bool) -> R<Option<V3>> {
        let id = self.ag_unit_id(w, u);
        let next = self.ag_peek();
        let here =
            next.is_some_and(|r| r["ev"] == "ag_ground" && r["u"].as_str() == Some(id.as_str()));
        if !here {
            return if required {
                Err(Desync("TickUnit places a unit on the ground; the recording doesn't".into()))
            } else {
                Ok(None)
            };
        }
        let r = self.ag_take(w, u, "ag_ground", "the place-on-ground step")?;
        self.ag_input(if required { "ground" } else { "ground (prone or elevator)" });
        let pos = w.unit(u).map(|x| x.position).unwrap_or_default();
        if v3(&r["pos"]).map(bits3) != Some(bits3(pos)) {
            return Err(Desync("the place-on-ground step starts from another position".into()));
        }
        v3(&r["after"])
            .map(Some)
            .ok_or_else(|| Desync("ag_ground without its position after".into()))
    }
    fn desired_orientation(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        value: f32,
        max_angle: f32,
    ) -> R<f32> {
        let r = self.ag_take(w, u, "ag_rot", "UpdateSlowRotation")?;
        if f(&r["o"]).map(f32::to_bits) != Some(value.to_bits())
            || f(&r["max"]).map(f32::to_bits) != Some(max_angle.to_bits())
        {
            return Err(Desync(format!(
                "UpdateSlowRotation({value}, {max_angle}): the recording's is {}",
                describe(&r)
            )));
        }
        self.ag_input("DesiredOrientation");
        f(&r["desired"]).ok_or_else(|| Desync("ag_rot without its desired orientation".into()))
    }
    fn paused(&mut self, w: &mut World) -> R<bool> {
        Ok(w.combat.commands_paused)
    }
    fn saddled_unit(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(false)
    }
}

impl Driver<'_> {
    #[allow(clippy::too_many_arguments)]
    fn ag_phys(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        kind: &str,
        o: V3,
        shape: &[f32],
        max: f32,
        mask: i32,
    ) -> R<Option<f32>> {
        let id = self.ag_unit_id(w, u);
        let at = self.ag_pull(w, &format!("a {kind} query"), |r| {
            r["k"] == "obs" && r["ev"] == "ag_phys" && r["u"].as_str() == Some(id.as_str())
        })?;
        let r = &self.trace.records[at];
        let recorded: Option<Vec<f32>> = if kind == "ray" {
            v3(&r["d"]).map(|d| d.to_vec())
        } else {
            f(&r["r"]).map(|x| vec![x])
        };
        let same_shape = recorded
            .is_some_and(|x| x.iter().map(|v| v.to_bits()).eq(shape.iter().map(|v| v.to_bits())));
        if r["fn"] != kind
            || r["ctx"] != "tick"
            || v3(&r["o"]).map(bits3) != Some(bits3(o))
            || !same_shape
            || f(&r["max"]).map(f32::to_bits) != Some(max.to_bits())
            || r["mask"].as_i64() != Some(i64::from(mask))
        {
            return Err(Desync(format!(
                "a {kind} query from {o:?}: the recording has {}",
                describe(r)
            )));
        }
        let hit = r["hit"] == true;
        let dist = if hit {
            Some(f(&r["dist"]).ok_or_else(|| Desync("a hit without its distance".into()))?)
        } else {
            None
        };
        *self.out.ag_inputs.entry("physics").or_default() += 1;
        Ok(dist)
    }

    /// A call into the view (`OnMovementInterrupted`, `OnMovementComplete`,
    /// `OnMovementWaypointUpdate`): the recording's, then the unit's reaction inside it as
    /// recorded (the commands' records processed as usual; a `Stop` re-entering the agent runs
    /// the sim's).
    fn ag_callback(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        fname: &'static str,
        dest: Option<V3>,
        index: Option<i32>,
    ) -> R<()> {
        let r = self.ag_take(w, u, "ag_cb", &format!("the view's {fname}"))?;
        let same_dest = match (opt_v3(&r["dest"]), dest) {
            (Some(a), b) => a.map(bits3) == b.map(bits3),
            (None, _) => false,
        };
        if r["fn"] != fname
            || !same_dest
            || r["i"].as_i64().and_then(|x| i32::try_from(x).ok()) != index
        {
            return Err(Desync(format!(
                "the agent calls the view's {fname}({dest:?}, {index:?}); the recording has {}",
                describe(&r)
            )));
        }
        let id = self.ag_unit_id(w, u);
        loop {
            let trace = self.trace;
            let Some(x) = trace.records.get(self.i) else {
                return Err(Desync(format!("{fname} without its end")));
            };
            if x["ev"] == "ag_cb_end" && x["fn"] == fname && x["u"].as_str() == Some(id.as_str()) {
                self.i += 1;
                return Ok(());
            }
            if x["ev"] == "ag_call" && x["fn"] == "Stop" && x["u"].as_str() == Some(id.as_str()) {
                ag::stop(w, u, self)?;
                continue;
            }
            if self.ag_stop(x) {
                return Err(Desync(format!(
                    "inside {fname}: {} (only a re-entering Stop is modelled)",
                    describe(x)
                )));
            }
            let i = self.i;
            self.i = self.record(w, i);
        }
    }
}
