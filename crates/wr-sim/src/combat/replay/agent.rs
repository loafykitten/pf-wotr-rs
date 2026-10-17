//! The driven replay's side of the movement agent ([`crate::combat::agent`],
//! [`crate::combat::movement`]): WrathOracle's observer `agent` with `agent_sync`
//! (`docs/poc5/movement.md`, "The driven replay"). `ag_init` gives every agent's state at the
//! window's first tick and `ag_sync` where real time ends (real-time agent code isn't ported:
//! its spans are taken); from there each agent is the sim's.
//!
//! The sim's code makes the agent's calls: `UnitMoveController.TickUnit` (an `ag_ctl` entry,
//! each frame's batch the awake units in order), the commands' `MoveTo`, `FollowPrecomputedPath`
//! and `StopMoving` (their `uc_call`s, [`Driver::ag_command_call`]) and the AI's `FindPath` (an
//! `ag_req` entry with the AI's arguments). Each pulls the records it makes and reads, in order,
//! through [`AgentInputs`]; a state the recording writes is checked against the sim's (a
//! difference is reported and the recorded state taken). The agent's getters the commands, the
//! turn controller and the observation rows read are the sim's, checked against their records.
//! What stays an input is counted (`Outcome::ag_inputs`): path deliveries and contents, physics,
//! animation and view gates, unit reads (corpulence, speed), calls from code the sim doesn't run
//! (by caller), positions written outside the agent (`ag_pos`, by caller).

use std::collections::{HashMap, HashSet};

use super::commands::describe;
use super::{Driver, Phase, Row};
use crate::combat::agent::{
    self as ag, Agent, AgentInputs, AgentPath, AgentPhysics, DestAgent, PathRequest, Proxy, V2, V3,
};
use crate::combat::commands::approach::Call;
use crate::combat::commands::{Desync, R};
use crate::combat::movement::{self, MoveInputs};
use crate::combat::oracle::Rv;
use crate::combat::orientation;
use crate::world::{EntityIdx, World};

pub(super) fn f(v: &Rv) -> Option<f32> {
    #[allow(clippy::cast_possible_truncation)]
    v.as_f64().map(|x| x as f32)
}

pub(super) fn v3(v: &Rv) -> Option<V3> {
    let a = v.as_array().filter(|a| a.len() == 3)?;
    Some([f(&a[0])?, f(&a[1])?, f(&a[2])?])
}

pub(super) fn v2(v: &Rv) -> Option<V2> {
    let a = v.as_array().filter(|a| a.len() == 2)?;
    Some([f(&a[0])?, f(&a[1])?])
}

fn opt_v3(v: &Rv) -> Option<Option<V3>> {
    if v.is_null() { Some(None) } else { v3(v).map(Some) }
}

fn opt_f(v: &Rv) -> Option<Option<f32>> {
    if v.is_null() { Some(None) } else { f(v).map(Some) }
}

pub(super) fn bits3(v: V3) -> [u32; 3] {
    v.map(f32::to_bits)
}

/// The record has exactly `keys` besides the trace's own (`seq`, `k`, `t`, `ev`).
pub(super) fn fields(r: &Rv, keys: &[&str]) -> bool {
    const BASE: [&str; 4] = ["seq", "k", "t", "ev"];
    r.as_object().is_some_and(|o| {
        o.len() == BASE.len() + keys.len() && BASE.iter().chain(keys).all(|k| o.contains_key(*k))
    })
}

/// A critter's ground query (`ag_phys`, ctx "critter"): exactly the record WrathOracle writes, with
/// no unit (`u` present and null), a complete identity of the critter (scene, hierarchy path,
/// sibling path) and the query `UnitMovementAgentBase.Move` makes: a ray straight down or a disc
/// cast of radius 0.4, 100 m, `WalkableMask`, from a finite point, with a finite result and the
/// collider when it hit. Anything less isn't identified.
fn critter_phys(r: &Rv) -> bool {
    let query = match r["fn"].as_str() {
        Some("ray") => v3(&r["d"]).map(bits3) == Some(bits3([0.0, -1.0, 0.0])),
        _ => f(&r["r"]).map(f32::to_bits) == Some(0.4f32.to_bits()),
    };
    let c = &r["critter"];
    r["ctx"] == "critter"
        && phys_record(r, "critter")
        && query
        && f(&r["max"]).map(f32::to_bits) == Some(100.0f32.to_bits())
        && r["mask"].as_i64() == Some(2_099_457)
        && c.as_object().is_some_and(|o| o.len() == 3)
        && placed(c)
}

/// Another caller's query (`ag_phys`, ctx "other": outside any unit agent's entry and not a
/// critter's own move): exactly the record WrathOracle writes, with no unit (`u` present and
/// null), the caller chain (`origin`, non-empty), no critter, a ray (finite direction) or a disc
/// cast (finite radius) from a finite point with a finite length and a mask, and a finite result
/// and the collider when it hit.
fn other_phys(r: &Rv) -> bool {
    let query = match r["fn"].as_str() {
        Some("ray") => finite3(&r["d"]),
        _ => f(&r["r"]).is_some_and(f32::is_finite),
    };
    r["ctx"] == "other"
        && phys_record(r, "origin")
        && query
        && r["origin"].as_str().is_some_and(|x| !x.is_empty())
        && f(&r["max"]).is_some_and(f32::is_finite)
        && r["mask"].is_i64()
}

/// A finite vector.
fn finite3(v: &Rv) -> bool {
    v3(v).is_some_and(|p| p.iter().all(|x| x.is_finite()))
}

/// An identified object (`PhysProbe.WritePath`/`WriteId`): scene, hierarchy path, sibling path.
fn placed(o: &Rv) -> bool {
    ["scene", "path"].iter().all(|k| o[*k].as_str().is_some_and(|x| !x.is_empty()))
        && o["sib"].as_array().is_some_and(|a| a.iter().all(Rv::is_u64))
}

/// An unattributed `ag_phys` record's shape: exactly its keys (the query's, `u` null, `who`: the
/// critter or the caller chain, the hit's when it hit), a finite origin, and a finite result with
/// the collider hit (`WriteId`: placed, its type and instance id) when it hit.
fn phys_record(r: &Rv, who: &str) -> bool {
    let mut keys = vec!["fn", "ctx", "u", who, "o", "max", "mask", "hit"];
    match r["fn"].as_str() {
        Some("ray") => keys.push("d"),
        Some("disc") => keys.push("r"),
        _ => return false,
    }
    match r["hit"].as_bool() {
        Some(true) => keys.extend(["dist", "pt", "col"]),
        Some(false) => {}
        None => return false,
    }
    let col = &r["col"];
    let collider = col.as_object().is_some_and(|o| o.len() == 5)
        && placed(col)
        && col["type"].as_str().is_some_and(|x| !x.is_empty())
        && col["iid"].is_i64();
    let result = r["hit"] == false
        || (f(&r["dist"]).is_some_and(f32::is_finite) && finite3(&r["pt"]) && collider);
    result && finite3(&r["o"]) && fields(r, &keys) && r["u"].is_null()
}

/// A required boolean (a missing, null or mistyped value isn't `false`).
pub(super) fn boolean(r: &Rv, k: &str, what: &str) -> R<bool> {
    r[k].as_bool().ok_or_else(|| Desync(format!("{what} without a boolean {k} ({})", describe(r))))
}

/// The caller of the hooked method in a recorded call chain (`origin`, innermost first): the
/// frame after `skip` frames.
pub(super) fn caller(origin: &str, skip: usize) -> String {
    let frame = origin.split(" < ").nth(skip).unwrap_or("?");
    let mut parts: Vec<&str> = frame.rsplit('.').take(2).collect();
    parts.reverse();
    parts.join(".")
}

/// An agent from a recorded state `s` (WrathOracle's `WriteState`), with the paths it holds
/// from `paths`; `None` when a field is missing or of another type.
pub(crate) fn agent_from(s: &Rv, paths: &HashMap<String, Vec<V3>>) -> Option<Agent> {
    let b = |k: &str| s[k].as_bool();
    let n = |k: &str| f(&s[k]);
    let i = |k: &str| s[k].as_i64().and_then(|x| i32::try_from(x).ok());
    let path = |k: &str| -> Option<Option<AgentPath>> {
        match &s[k] {
            Rv::Null => Some(None),
            Rv::String(key) => Some(Some(AgentPath {
                key: key.as_str().to_owned(),
                points: paths.get(key.as_str()).cloned().unwrap_or_default(),
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

/// A value an `ag_req` record must have (the caller's arguments, the sim's destination agent).
#[derive(Debug, Clone, PartialEq)]
enum Want {
    F(f32),
    B(bool),
    S(Option<String>),
    Id(Option<u32>),
    V(V3),
}

impl Want {
    fn matches(&self, v: &Rv) -> bool {
        match self {
            Self::F(x) => f(v).map(f32::to_bits) == Some(x.to_bits()),
            Self::B(x) => v.as_bool() == Some(*x),
            Self::S(x) => match x {
                Some(s) => v.as_str() == Some(s.as_str()),
                None => v.is_null(),
            },
            Self::Id(x) => match x {
                Some(c) => v.as_u64() == Some(u64::from(*c)),
                None => v.is_null(),
            },
            Self::V(x) => v3(v).map(bits3) == Some(bits3(*x)),
        }
    }
}

/// A block of snapshot records (`ag_init` or `ag_sync`): `AllAgents` in order with their
/// indices, then the other units' agents (index -1).
#[derive(Debug)]
pub(super) struct AgBlock {
    ev: String,
    tick: i64,
    next: i64,
    disabled: bool,
    units: HashSet<EntityIdx>,
    enabled: Vec<EntityIdx>,
}

/// A frame's `UnitMoveController.Tick`: the units it ticks, in order (`State.AwakeUnits`), those
/// whose awake state the sim can't establish, and how far the recording's `TickUnit`s got.
#[derive(Debug)]
pub(super) struct AgBatch {
    tick: i64,
    expected: Vec<EntityIdx>,
    unknown: HashSet<EntityIdx>,
    next: usize,
}

/// The driven agent's state across the recording.
#[derive(Debug)]
pub(super) struct AgDriver {
    /// Path contents by key, and each definition's `error`, `CompleteState`, kind and `pathID`
    /// (`ag_pathdef`).
    pub paths: HashMap<String, Vec<V3>>,
    pub defs: HashMap<String, (bool, i64, String, i64)>,
    /// `UnitMovementAgentBase.AllAgents` (the enabled agents, in order).
    pub all: Vec<EntityIdx>,
    /// The units with an agent (a `UnitMovementAgent`).
    pub has: HashSet<EntityIdx>,
    /// Each unit's last recorded agent state.
    pub last_state: HashMap<EntityIdx, Rv>,
    /// The `TickMovement`s running, innermost last (their unit and `ag_tick` record).
    pub ticks: Vec<(EntityIdx, Rv)>,
    /// The `PathTo`/`FindPath` running (its `ag_req`), and what its caller passes.
    pub req_rec: Option<Rv>,
    req_want: Option<Vec<(&'static str, Want)>>,
    /// `SlowDown`'s draw sites; the grid's erosion settings (the area's nav file).
    pub slow_sites: Vec<i64>,
    pub erosion: Result<Option<(i32, i32)>, String>,
    /// The capture records FindPath's callback (`ag_findclr`, since poc5-tb-109).
    pub has_findclr: bool,
    /// The capture declares `agent_sync` (else a diagnostic run: `WR_AGENT_DRIVEN`).
    pub strict: bool,
    /// Every agent's state is the sim's (the initial states or a synchronization came after the
    /// last real-time span).
    pub synced: bool,
    pub init_seen: bool,
    /// Real time since the last snapshot (a real-time entry or frame, a combat start): a
    /// synchronization may come.
    pub boundary: bool,
    pub block: Option<AgBlock>,
    /// The units whose `PreviousPosition` the sim knows.
    pub prev_known: HashSet<EntityIdx>,
    pub batch: Option<AgBatch>,
    /// `PathSync`'s requests per requester so far (its keys are `<requester>#<n>`).
    pub req_counts: HashMap<String, u32>,
    /// The turn's `TickMovement` delta the cooldowns computed (`cd_move`).
    pub cd_out: Option<f32>,
    /// The last delivered path (`path_ret`, a recording's) not yet completed; the requests
    /// started (`path_start`) and those completed (a replay's trace has no `path_ret`).
    pub last_ret: Option<String>,
    pub started: HashSet<String>,
    /// Each started request's frame (`path_ret`'s `req`).
    pub started_at: HashMap<String, i64>,
    pub delivered: HashSet<String>,
    /// The paths the sim constructed (the commands' `ForcedPath`s); the `FindPath` requests the
    /// sim made, not yet delivered.
    pub constructed: HashSet<String>,
    pub finding: HashSet<String>,
    /// The requests the snapshots and real-time spans showed in flight (delivered without a
    /// `path_ret` when the window didn't start them); the deliveries (`path_ret`) seen.
    pub carried: HashSet<String>,
    pub returned: HashSet<String>,
    /// The unit whose `TickUnit` runs.
    pub ctl_unit: Option<EntityIdx>,
    /// The last `FollowPrecomputedPath`/`ForcePath` call the sim took (its record).
    pub follow_rec: Option<usize>,
    /// The frame's observation rows so far (frame, units), and the frame whose unwritten rows
    /// were checked.
    pub rows: (i64, HashSet<String>),
    pub rows_checked: Option<i64>,
    /// The recording ended (its last frame's controllers may not have run).
    pub ending: bool,
    /// The physics and paths the sim computes (`replay/nav.rs`).
    pub nav: super::nav::NavDriver,
    /// Units' orientation writes (`replay/look.rs`).
    pub look: super::look::LookDriver,
    /// The roaming controller (`replay/roaming.rs`).
    pub roam: super::roaming::RoamDriver,
}

impl Default for AgDriver {
    fn default() -> Self {
        Self {
            paths: HashMap::new(),
            defs: HashMap::new(),
            all: Vec::new(),
            has: HashSet::new(),
            last_state: HashMap::new(),
            ticks: Vec::new(),
            req_rec: None,
            req_want: None,
            slow_sites: Vec::new(),
            erosion: Ok(None),
            has_findclr: false,
            strict: false,
            synced: false,
            init_seen: false,
            boundary: false,
            block: None,
            prev_known: HashSet::new(),
            batch: None,
            req_counts: HashMap::new(),
            cd_out: None,
            last_ret: None,
            started: HashSet::new(),
            started_at: HashMap::new(),
            delivered: HashSet::new(),
            constructed: HashSet::new(),
            finding: HashSet::new(),
            carried: HashSet::new(),
            returned: HashSet::new(),
            ctl_unit: None,
            follow_rec: None,
            rows: (-1, HashSet::new()),
            rows_checked: None,
            ending: false,
            nav: super::nav::NavDriver::default(),
            look: super::look::LookDriver::default(),
            roam: super::roaming::RoamDriver::default(),
        }
    }
}

impl Driver<'_> {
    pub(super) fn ag_input(&mut self, what: impl Into<String>) {
        *self.out.ag_inputs.entry(what.into()).or_default() += 1;
    }

    fn ag_unsupported(w: &mut World, what: impl std::fmt::Display) {
        w.coverage.unsupported(format!("driven replay: agent: {what}"));
    }

    /// A draw at `UnitMovementAgent.SlowDown`'s site.
    pub(super) fn ag_slow_draw(&self, r: &Rv) -> bool {
        r["k"] == "draw"
            && (r["site"].as_i64().is_some_and(|s| self.ags.slow_sites.contains(&s))
                || self.trace.site_key(r).as_deref() == Some(crate::rng::sites::AGENT_SLOW_DOWN.0))
    }

    /// A record an agent entry pulls (one it didn't ask for stops the pull): the agent's records
    /// but path definitions, FindPath's callback, enable changes and outside position writes
    /// (processed as they come), the turn's `cd_move`, `PathSync`'s request starts and
    /// `SlowDown`'s draws.
    fn ag_stop(&self, r: &Rv) -> bool {
        if r["k"] == "tick" || self.ag_slow_draw(r) {
            return true;
        }
        r["k"] == "obs"
            && r["ev"].as_str().is_some_and(|e| {
                (e.starts_with("ag_")
                    && !matches!(
                        e,
                        "ag_pathdef" | "ag_findclr" | "ag_enable" | "ag_pos" | "ag_look"
                    ))
                    || e == "cd_move"
                    || e == "path_start"
            })
    }

    /// A stop record skipped (an input's span, a desynced entry): a `SlowDown` draw takes the
    /// tape's to keep its site in step, a request start counts for its requester.
    fn ag_skip_stop(&mut self, w: &mut World, r: &Rv) {
        // A physics query of the span: the sim's answer to its recorded arguments, checked.
        if r["k"] == "obs" && r["ev"] == "ag_phys" {
            // (A critter's or another caller's, identified: the sim never uses the answer.)
            let ctx = if critter_phys(r) || other_phys(r) {
                super::nav::PhysCtx::Other
            } else if matches!(r["ctx"].as_str(), Some("critter" | "other")) {
                Self::ag_unsupported(
                    w,
                    format!("an unattributed physics query it can't identify ({})", describe(r)),
                );
                return;
            } else {
                super::nav::PhysCtx::Span
            };
            self.nav_phys_alone(w, r, ctx);
        }
        if self.ag_slow_draw(r) {
            let _ = w.rng.range_float(crate::rng::sites::AGENT_SLOW_DOWN, 0.2, 0.3);
        }
        // `UpdateSlowRotation`'s read in a span taken from the recording: the `DesiredOrientation`
        // the span's `TickUnit` left (its `Orientation` setter's), taken like the span's
        // orientation.
        if r["k"] == "obs"
            && r["ev"] == "ag_rot"
            && let (Some(u), Some(d)) =
                (r["u"].as_str().and_then(|x| w.by_id.get(x).copied()), f(&r["desired"]))
            && let Some(unit) = w.unit_mut(u)
        {
            unit.combat.desired_orientation = d;
        }
        if r["k"] == "obs" && r["ev"] == "path_start" {
            self.ag_count_request(r);
            if let Some(k) = r["path"].as_str() {
                self.nav_taken_request(k);
            }
        }
    }

    /// A delivered request's callback (`OnPathComplete`, FindPath's): once, of a request the
    /// window started or the snapshots and real-time spans carried; in a recording a window
    /// request's right after its `path_ret` (a replay's trace has none).
    fn ag_deliver(&mut self, key: &str) -> bool {
        let pending = self.ags.last_ret.take();
        let window = self.ags.started.contains(key);
        let known = window || self.ags.carried.contains(key);
        let ret = self.replay_mode
            || if window { pending.as_deref() == Some(key) } else { pending.is_none() };
        known && ret && self.ags.delivered.insert(key.to_owned())
    }

    /// The requests an agent's state shows in flight (a snapshot's, a taken span's).
    fn ag_carry(&mut self, a: &Agent) {
        if let Some(p) = &a.requested {
            self.ags.carried.insert(p.key.clone());
        }
        if let Some(p) = &a.requested_find {
            self.ags.carried.insert(p.key.clone());
            self.ags.finding.insert(p.key.clone());
        }
    }

    /// A frame ends: a delivery's callback comes in its frame.
    pub(super) fn ag_frame_end(&mut self, w: &mut World) {
        if self.ags.strict && self.ags.last_ret.take().is_some() {
            Self::ag_unsupported(w, "a delivery (path_ret) without its callback");
        }
    }

    fn ag_count_request(&mut self, r: &Rv) {
        if let Some(key) = r["path"].as_str() {
            self.ags.started.insert(key.to_owned());
            if let Some(t) = r["t"].as_i64() {
                self.ags.started_at.insert(key.to_owned(), t);
            }
            if let Some((who, _)) = key.rsplit_once('#') {
                *self.ags.req_counts.entry(who.to_owned()).or_default() += 1;
            }
            self.nav_seeker_start(key);
        }
    }

    /// Records up to the next one `want` takes (returned); everything before it is processed as
    /// usual, but an agent record the sim didn't ask for stops the pull.
    pub(super) fn ag_pull(
        &mut self,
        w: &mut World,
        what: &str,
        want: impl Fn(&Rv) -> bool,
    ) -> R<usize> {
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
            if self.ag_stop(r) {
                return Err(Desync(format!("{what}: the recording has {}", describe(r))));
            }
            let i = self.i;
            self.i = self.record(w, i);
        }
    }

    /// The next record a pull would stop at, not consumed.
    pub(super) fn ag_peek(&self) -> Option<Rv> {
        self.trace.records[self.i..].iter().find(|r| self.ag_stop(r)).cloned()
    }

    fn ag_unit_id(&self, w: &World, u: EntityIdx) -> String {
        w.entity(u).unique_id.clone()
    }

    /// The next agent record `ev` of the entry's unit.
    fn ag_take(&mut self, w: &mut World, u: EntityIdx, ev: &str, what: &str) -> R<Rv> {
        let id = self.ag_unit_id(w, u);
        let at = self.ag_pull(w, what, |r| r["k"] == "obs" && r["ev"] == ev)?;
        let r = self.trace.records[at].clone();
        if r["u"].as_str() != Some(id.as_str()) {
            return Err(Desync(format!("{what} of {id}: the recording's is {}", describe(&r))));
        }
        Ok(r)
    }

    /// The agent's leaf `leaf`: its unit is the innermost `TickMovement`'s (`null` outside one),
    /// or `TickUnit`'s and `ConnectToGroups`' own.
    fn ag_leaf(&mut self, w: &mut World, u: EntityIdx, leaf: &str) -> R<Rv> {
        let want = match leaf {
            "canmove" | "canrotate" | "gmem" => Some(self.ag_unit_id(w, u)),
            _ => self.ags.ticks.last().map(|(t, _)| self.ag_unit_id(w, *t)),
        };
        let at = self.ag_pull(w, &format!("the agent's {leaf}"), |r| {
            r["k"] == "obs" && r["ev"] == "ag_v" && r["f"] == leaf
        })?;
        let r = self.trace.records[at].clone();
        if r["u"].as_str() != want.as_deref() || r.get("ctx").is_some() {
            return Err(Desync(format!(
                "the agent's {leaf} (of {want:?}): the recording's is {}",
                describe(&r)
            )));
        }
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
            "gmem" => "group memory",
            "softcol" | "closest" => "soft collider",
            "proxy" => "proxy",
            "gamepad" => "gamepad",
            "move" => "move command (checked)",
            "unreach" => "UnreachableUnits.Add (checked)",
            _ => "other leaf",
        });
        Ok(r)
    }

    /// The sim's agent against a recorded state: a difference is reported and the recorded
    /// state taken.
    fn ag_check_state(&mut self, w: &mut World, u: EntityIdx, s: &Rv, what: &str) {
        let Some(want) = agent_from(s, &self.ags.paths) else {
            Self::ag_unsupported(w, format!("{what}: a state it can't read"));
            return;
        };
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
            Self::ag_unsupported(
                w,
                format!(
                    "{what}: the sim's state differs from the recording's ({})",
                    fields.join(", ")
                ),
            );
        }
        self.ags.last_state.insert(u, s.clone());
    }

    fn ag_has(&self, u: EntityIdx) -> bool {
        self.ags.has.contains(&u)
    }

    /// An `ag_*` record.
    pub(super) fn ag_record(&mut self, w: &mut World, i: usize) -> usize {
        let trace = self.trace;
        let r = &trace.records[i];
        let ev = r["ev"].as_str().unwrap_or("");
        if r["t"].as_i64() != Some(self.tick) {
            Self::ag_unsupported(w, format!("a record ({ev}) of another frame"));
            return i + 1;
        }
        if !matches!(ev, "ag_init" | "ag_sync" | "ag_pathdef") {
            self.ag_block_close(w);
        }
        match ev {
            "ag_init" | "ag_sync" => self.ag_snapshot(w, r, ev),
            "ag_pathdef" => self.ag_pathdef(w, r),
            "ag_enable" => self.ag_enable(w, r),
            "ag_findclr" => self.ag_findclr(w, r),
            "ag_pos" => self.ag_pos(w, r),
            "ag_look" => self.look_outside(w, r),
            "ag_ctl" => return self.ag_entry(w, i, AgEntry::Ctl),
            "ag_req" => return self.ag_entry(w, i, AgEntry::Req),
            "ag_done" => return self.ag_entry(w, i, AgEntry::Done),
            "ag_call" => return self.ag_entry(w, i, AgEntry::Call),
            // The place-on-ground step outside a `TickUnit` (its own controller): physics, an input.
            // The place-on-ground controller's own step (`ground_view`: its rays come first).
            "ag_phys" if r["ctx"] == "ground" && self.ags.nav.ground_view => {
                return self.nav_ground_entry(w, i);
            }
            "ag_ground" if self.ags.nav.ground_view => {
                Self::ag_unsupported(w, "a place-on-ground step without its rays");
            }
            "ag_ground" => {
                let u = r["u"].as_str().and_then(|x| w.by_id.get(x).copied());
                let (pos, after) = (v3(&r["pos"]), v3(&r["after"]));
                match (u, pos, after, r["ctl"].as_bool()) {
                    (Some(u), Some(pos), Some(after), Some(false))
                        if fields(r, &["u", "ctl", "pos", "after"]) =>
                    {
                        self.ag_input("ground (outside TickUnit)");
                        let unit = w.unit_mut(u).expect("a unit");
                        if bits3(unit.position) != bits3(pos) {
                            Self::ag_unsupported(
                                w,
                                "the place-on-ground step starts from another position than the sim's",
                            );
                        }
                        let unit = w.unit_mut(u).expect("a unit");
                        unit.position = after;
                        unit.combat.agent.pos = after;
                    }
                    _ => Self::ag_unsupported(w, "a place-on-ground record it can't read"),
                }
            }
            // `IsDone()` read outside the sim's agent code (the AI's), the AI's movement
            // simulation (`SimulateTickMovementForTurnBased`, ported in `combat::ai_sim`, not yet
            // run by the AI): the AI's inputs.
            "ag_v" if r["f"] == "isdone" && r.get("ctx").is_none() => {
                // (A `ForcedPath` the sim constructed is never done.)
                let path = r["path"].as_str().unwrap_or("");
                if self.ags.constructed.contains(path) {
                    if r["v"] != false {
                        Self::ag_unsupported(
                            w,
                            format!(
                                "IsDone of the constructed {path}: the recording's isn't false"
                            ),
                        );
                    }
                    self.out.ag_reads += 1;
                } else {
                    self.ag_input("isdone (read outside the sim's agent code)");
                }
            }
            // A critter's ground ray (`CritterMoveAgent.TickMovement`, the static `Move`; WrathOracle
            // names the critter): presentation, no unit's; the sim's answer checked when it has one.
            "ag_phys" if critter_phys(r) => {
                self.nav_phys_alone(w, r, super::nav::PhysCtx::Other);
            }
            // The movement simulation (`SimulateTickMovementForTurnBased`: the AI's `ScorePath`,
            // the path visualizer): each step the sim's from its recorded simulator
            // (`replay/nav.rs`); in real time its physics the sim's, its reads inputs.
            "ag_sim" => return self.nav_sim_step(w, i),
            // A query of another caller (a critter's move): the sim's answer checked when it has one.
            "ag_phys" if other_phys(r) => {
                self.nav_phys_alone(w, r, super::nav::PhysCtx::Other);
            }
            "ag_phys" if r["ctx"] == "sim" && !w.combat.turn_based => {
                self.nav_phys_alone(w, r, super::nav::PhysCtx::Span);
            }
            "ag_sim_end" | "ag_v"
                if !w.combat.turn_based && (ev == "ag_sim_end" || r["ctx"] == "sim") =>
            {
                self.ag_input("the AI's simulation (real time)");
            }
            "ag_phys" | "ag_sim_end" | "ag_v" if ev == "ag_sim_end" || r["ctx"] == "sim" => {
                Self::ag_unsupported(
                    w,
                    format!("a movement simulation record outside its step ({})", describe(r)),
                );
            }
            _ => Self::ag_unsupported(
                w,
                format!("a record outside the sim's agent entries ({})", describe(r)),
            ),
        }
        i + 1
    }

    fn ag_pathdef(&mut self, w: &mut World, r: &Rv) {
        let Some(k) = r["path"].as_str() else {
            Self::ag_unsupported(w, "a path definition it can't read");
            return;
        };
        if r["pending"] == true {
            // (The path's kind is known before its points.)
            match r["kind"].as_str() {
                Some(kind) if fields(r, &["path", "kind", "pending"]) => {
                    self.nav_def(w, k, kind, None);
                }
                _ => Self::ag_unsupported(w, "a path definition it can't read"),
            }
            return;
        }
        let pts: Option<Vec<V3>> = r["v"].as_array().and_then(|v| v.iter().map(v3).collect());
        match (pts, r["err"].as_bool(), r["cs"].as_i64(), r["kind"].as_str(), r["pid"].as_i64()) {
            (Some(p), Some(err), Some(cs), Some(kind), Some(pid)) => {
                // A path the sim computes: the definition is a check.
                self.nav_def(w, k, kind, Some((&p, err, cs)));
                self.ags.paths.insert(k.to_owned(), p);
                self.ags.defs.insert(k.to_owned(), (err, cs, kind.to_owned(), pid));
            }
            _ => Self::ag_unsupported(w, "a path definition it can't read"),
        }
    }

    /// An agent joins or leaves `AllAgents` (`OnEnable`/`OnDisable`: the view enabled or
    /// disabled, not modelled: an input); the record's index is `IndexOf` after it. `OnDisable`
    /// also leaves its obstacle group.
    fn ag_enable(&mut self, w: &mut World, r: &Rv) {
        self.ag_input("an agent enabled or disabled");
        let u = r["u"].as_str().and_then(|x| w.by_id.get(x).copied());
        let (Some(u), Some(on), Some(i), true) =
            (u, r["on"].as_bool(), r["i"].as_i64(), fields(r, &["u", "view", "on", "i"]))
        else {
            Self::ag_unsupported(w, "an enable record it can't read");
            return;
        };
        if !self.ag_has(u) {
            Self::ag_unsupported(w, "an agent enabled or disabled that the sim doesn't have");
            return;
        }
        if on {
            self.ags.all.push(u);
        } else if let Some(at) = self.ags.all.iter().position(|x| *x == u) {
            self.ags.all.remove(at);
        }
        let index = self.ags.all.iter().position(|x| *x == u).map_or(-1, |x| x as i64);
        if index != i {
            Self::ag_unsupported(
                w,
                "AllAgents' index after an enable or disable differs from the sim's",
            );
        }
        if !on && let Err(d) = ag::remove_from_group(w, u, self) {
            Self::ag_unsupported(w, format!("OnDisable: {}", d.0));
        }
    }

    /// FindPath's callback ran (the AI's path delivered: when is an input):
    /// `m_RequestedFindPath = null`. The path is a request the sim's `FindPath` made, delivered
    /// once (in a recording right after its `path_ret`).
    fn ag_findclr(&mut self, w: &mut World, r: &Rv) {
        self.ag_input("FindPath's delivery");
        let u = r["u"].as_str().and_then(|x| w.by_id.get(x).copied());
        let (Some(u), Some(path), true) =
            (u, r["path"].as_str(), fields(r, &["u", "path", "held"]))
        else {
            Self::ag_unsupported(w, "a FindPath callback record it can't read");
            return;
        };
        let requested = self.ags.finding.remove(path);
        // The callback the seeker makes for a path the sim computed.
        if let Err(d) = self.nav_delivered(path, None) {
            Self::ag_unsupported(w, format!("FindPath's callback: {}", d.0));
        }
        if self.ags.strict && !(self.ag_deliver(path) && requested) {
            Self::ag_unsupported(
                w,
                "FindPath's callback of a path the sim's FindPath didn't request, or without its delivery",
            );
        }
        let Some(unit) = w.unit_mut(u) else { return };
        let held = unit.combat.agent.requested_find.take().map(|p| p.key);
        if held.as_deref() != r["held"].as_str() {
            Self::ag_unsupported(
                w,
                "FindPath's callback: the sim's requested path differs from the recording's",
            );
        }
    }

    /// A unit's position written outside the agent's controllers (roaming while asleep, a
    /// teleport): an input, by its caller.
    fn ag_pos(&mut self, w: &mut World, r: &Rv) {
        let u = r["u"].as_str().and_then(|x| w.by_id.get(x).copied());
        let (Some(u), Some(pos), Some(v), Some(origin), true) = (
            u,
            v3(&r["pos"]),
            v3(&r["v"]),
            r["origin"].as_str(),
            fields(r, &["u", "pos", "v", "origin"]),
        ) else {
            Self::ag_unsupported(w, "a position record it can't read");
            return;
        };
        // (The observer leaves out the controllers' own writes: one there didn't come from it.)
        let inside = origin.split(" < ").any(|f| {
            f == "Kingmaker.Controllers.Units.UnitMoveController.TickUnit"
                || f == "Kingmaker.Controllers.Units.UnitPlaceOnGroundController.TickUnit"
        });
        if inside || self.ags.ctl_unit == Some(u) {
            Self::ag_unsupported(w, "a position written inside the agent's controllers");
            return;
        }
        // (With the roaming controller the sim's, one it didn't make was reported: taken.)
        self.ag_input(format!("a position written by {}", caller(origin, 1)));
        let Some(unit) = w.unit_mut(u) else { return };
        if bits3(unit.position) != bits3(pos) {
            Self::ag_unsupported(
                w,
                "a position written outside the agent: the position before differs from the sim's",
            );
        }
        let unit = w.unit_mut(u).expect("a unit");
        unit.position = orientation::set_position(unit.position, v);
    }

    /// `ag_init` (the window's first tick) and `ag_sync` (where real time ends): every agent's
    /// state (taken: the sim doesn't run the code before), the unit's position, orientation,
    /// `PreviousPosition` and `DesiredOrientation`, in `AllAgents`' order with its indices.
    fn ag_snapshot(&mut self, w: &mut World, r: &Rv, ev: &str) {
        let sync = ev == "ag_sync";
        let open = !self.ags.block.as_ref().is_some_and(|b| b.ev == ev && b.tick == self.tick);
        if open {
            self.ag_block_close(w);
            if !sync && self.ags.init_seen {
                Self::ag_unsupported(w, "a second block of initial states (ag_init)");
            }
            if sync && !self.ags.init_seen {
                Self::ag_unsupported(w, "a synchronization (ag_sync) before the initial states");
            }
            if sync && !w.combat.turn_based {
                Self::ag_unsupported(w, "a synchronization (ag_sync) outside turn-based combat");
            }
            // Only where real time ended (a real-time entry or frame, a combat start, since the
            // last snapshot): elsewhere it would replace the sim's own outputs.
            if sync && !std::mem::take(&mut self.ags.boundary) {
                Self::ag_unsupported(w, "a synchronization (ag_sync) where real time didn't end");
            }
            self.ags.block = Some(AgBlock {
                ev: ev.to_owned(),
                tick: self.tick,
                next: 0,
                disabled: false,
                units: HashSet::new(),
                enabled: Vec::new(),
            });
            self.ags.synced = true;
            if sync {
                self.out.ag_syncs += 1;
            }
        }
        let (Some(i), Some(enabled)) = (r["i"].as_i64(), r["enabled"].as_bool()) else {
            Self::ag_unsupported(w, format!("an {ev} record it can't read"));
            return;
        };
        {
            let b = self.ags.block.as_mut().expect("a block");
            let ok = if enabled { !b.disabled && i == b.next } else { i == -1 };
            if enabled {
                b.next += 1;
            } else {
                b.disabled = true;
            }
            if !ok {
                Self::ag_unsupported(w, format!("{ev}: an agent's AllAgents index out of order"));
            }
        }
        if r["kind"] != "UnitMovementAgent" {
            Self::ag_unsupported(w, "an agent of another kind (UnitMovementAgentContinuous?)");
            return;
        }
        // The fields (`prev` and `desired` since `agent_sync`).
        let base = ["i", "enabled", "kind", "u", "view", "upos", "orient", "s"];
        let all = ["i", "enabled", "kind", "u", "view", "upos", "orient", "prev", "desired", "s"];
        let newer = fields(r, &all);
        if !(newer || (!self.ags.strict && fields(r, &base))) {
            Self::ag_unsupported(w, format!("an {ev} record with fields its writer doesn't write"));
            return;
        }
        let Some(u) = r["u"].as_str().and_then(|x| w.by_id.get(x).copied()) else {
            Self::ag_unsupported(w, "an agent of a unit the sim doesn't have");
            return;
        };
        if !self.ags.block.as_mut().expect("a block").units.insert(u) {
            Self::ag_unsupported(w, format!("{ev}: an agent twice in its block"));
            return;
        }
        let state = agent_from(&r["s"], &self.ags.paths);
        let (upos, orient) = (v3(&r["upos"]), f(&r["orient"]));
        let (prev, desired) = (v3(&r["prev"]), f(&r["desired"]));
        let (Some(agent), Some(upos), Some(orient)) = (state, upos, orient) else {
            Self::ag_unsupported(w, format!("an {ev} record it can't read"));
            return;
        };
        if newer && (prev.is_none() || desired.is_none()) {
            Self::ag_unsupported(w, format!("an {ev} record it can't read"));
            return;
        }
        let what =
            if sync { "at the real-time/turn-based boundary" } else { "at the window's start" };
        let unit = w.unit(u).expect("a unit");
        let moved = bits3(unit.position) != bits3(upos);
        let turned = unit.orientation.to_bits() != orient.to_bits();
        let changed = !diff(&unit.combat.agent, &agent).is_empty();
        if moved {
            self.ag_input(format!("position {what}"));
        }
        if turned {
            self.ag_input(format!("orientation {what}"));
        }
        if sync && changed {
            self.ag_input(format!("state {what}"));
        }
        if !sync {
            self.ag_input("initial state");
        }
        self.ag_carry(&agent);
        let unit = w.unit_mut(u).expect("a unit");
        unit.position = upos;
        unit.orientation = orient;
        unit.combat.agent = agent;
        if let Some(p) = prev {
            unit.combat.prev_position = p;
            self.ags.prev_known.insert(u);
        }
        if let Some(d) = desired {
            if sync && self.ags.look.on && unit.combat.desired_orientation.to_bits() != d.to_bits()
            {
                self.out.look.snapshots_taken += 1;
            }
            let unit = w.unit_mut(u).expect("a unit");
            unit.combat.desired_orientation = d;
        }
        self.ags.has.insert(u);
        self.ags.last_state.insert(u, r["s"].clone());
        if enabled {
            self.ags.block.as_mut().expect("a block").enabled.push(u);
            if !sync {
                self.ags.all.push(u);
            }
        }
    }

    /// The end of a snapshot block: a synchronization covers every agent, and its `AllAgents`
    /// is the sim's.
    pub(super) fn ag_block_close(&mut self, w: &mut World) {
        let Some(b) = self.ags.block.take() else { return };
        if b.ev == "ag_init" {
            self.ags.init_seen = true;
            return;
        }
        if b.enabled != self.ags.all {
            Self::ag_unsupported(w, "AllAgents at the synchronization differ from the sim's");
            self.ags.all = b.enabled;
        }
        if self.ags.has.iter().any(|u| !b.units.contains(u)) {
            Self::ag_unsupported(w, "a synchronization without one of the agents");
        }
    }

    /// `UnitMoveController.Tick`'s batch, as the frame reaches its phase: every awake unit in
    /// `State.AwakeUnits`' order (dead ones too: it isn't a `BaseUnitController`), none while
    /// `WaitingForUI`. Units the director controls aren't modelled (no cutscene in the window).
    /// A frame in real time (the controllers' phases run): a synchronization may follow.
    pub(super) fn ag_frame(&mut self, w: &World) {
        if !w.combat.turn_based {
            self.ags.boundary = true;
            self.ags.synced = false;
        }
    }

    pub(super) fn ag_batch_open(&mut self, w: &mut World) {
        self.ag_block_close(w);
        self.ag_batch_close(w);
        if !self.ags.init_seen {
            return;
        }
        if w.combat.cutscene {
            Self::ag_unsupported(
                w,
                "UnitMoveController in a cutscene (ControlledByDirector isn't modelled)",
            );
        }
        let unknown: HashSet<EntityIdx> = w.combat.awake_unknown.iter().copied().collect();
        let expected = if w.combat.turns.waiting_for_ui > 0 {
            Vec::new()
        } else {
            crate::combat::life::all_units(w)
                .into_iter()
                .filter(|u| w.combat.awake.contains(u) || unknown.contains(u))
                .collect()
        };
        self.ags.batch = Some(AgBatch { tick: self.tick, expected, unknown, next: 0 });
    }

    /// The batch's next unit must be `u`.
    fn ag_batch_take(&mut self, w: &mut World, u: EntityIdx) -> R<()> {
        let tick = self.tick;
        let Some(b) = self.ags.batch.as_mut().filter(|b| b.tick == tick) else {
            return Err(Desync("a TickUnit outside UnitMoveController's tick".into()));
        };
        let mut skipped = 0;
        while b.expected.get(b.next).is_some_and(|x| *x != u && b.unknown.contains(x)) {
            b.next += 1;
            skipped += 1;
        }
        let next = b.expected.get(b.next).copied();
        if next == Some(u) {
            b.next += 1;
        }
        for _ in 0..skipped {
            self.ag_input("UnitMoveController: an awake state the sim can't establish");
        }
        if next != Some(u) {
            let want = next.map(|x| self.ag_unit_id(w, x));
            let got = self.ag_unit_id(w, u);
            return Err(Desync(format!(
                "UnitMoveController ticks {want:?} next, the recording {got}"
            )));
        }
        Ok(())
    }

    /// The batch's end: a unit the controller ticks that the recording doesn't have.
    pub(super) fn ag_batch_close(&mut self, w: &mut World) {
        let Some(b) = self.ags.batch.take() else { return };
        if self.ags.ending {
            return;
        }
        let left: Vec<EntityIdx> = b.expected[b.next.min(b.expected.len())..]
            .iter()
            .copied()
            .filter(|u| !b.unknown.contains(u))
            .collect();
        if !left.is_empty() {
            if std::env::var_os("WR_DEBUG_AGENT").is_some() {
                let ids: Vec<String> = left.iter().map(|u| self.ag_unit_id(w, *u)).collect();
                eprintln!("DEBUG agent tick {}: UnitMoveController didn't tick {ids:?}", b.tick);
            }
            Self::ag_unsupported(
                w,
                "UnitMoveController's tick of a unit the recording doesn't have",
            );
        }
    }

    /// The end of the recording.
    pub(super) fn ag_finish(&mut self, w: &mut World) {
        self.ag_frame_end(w);
        self.ag_block_close(w);
        self.ag_batch_close(w);
        self.nav_finish(w);
        self.look_finish(w);
    }

    /// An entry: the sim's code runs, pulling its records; on a desync the entry's remaining
    /// records are skipped up to its end.
    fn ag_entry(&mut self, w: &mut World, i: usize, kind: AgEntry) -> usize {
        let trace = self.trace;
        let r = &trace.records[i];
        let Some(u) = r["u"].as_str().and_then(|x| w.by_id.get(x).copied()) else {
            Self::ag_unsupported(
                w,
                format!("an entry of a unit the sim doesn't have ({})", describe(r)),
            );
            return i + 1;
        };
        self.out.ag_entries += 1;
        self.i = i;
        let id = self.ag_unit_id(w, u);
        let (end_ev, fname) = match kind {
            AgEntry::Ctl => ("ag_ctl_end", None),
            AgEntry::Req => ("ag_req_end", None),
            AgEntry::Done => ("ag_done_end", None),
            AgEntry::Call => ("ag_call_end", r["fn"].as_str().map(str::to_owned)),
        };
        // Real time (the walk up before combat) isn't ported: the entry is taken from the
        // recording (its states, the unit's position and orientation after); the agents are
        // the sim's again only from the next synchronization.
        if !w.combat.turn_based || self.uc_skipping || !self.ags.synced {
            if self.uc_skipping {
                self.ag_input("an entry while the commands skip a desynced entry (taken)");
            } else if w.combat.turn_based {
                Self::ag_unsupported(
                    w,
                    "turn-based agent code after real time without a synchronization (ag_sync)",
                );
            } else {
                self.ag_input("a real-time entry (taken)");
                self.ags.synced = false;
                self.ags.boundary = true;
            }
            if kind == AgEntry::Ctl {
                self.place(w, Phase::Move, "UnitMoveController.TickUnit");
                if let Err(d) = self.ag_ctl_start(w, u, r) {
                    Self::ag_unsupported(w, format!("a real-time TickUnit: {}", d.0));
                }
                if let Some(x) = w.unit_mut(u) {
                    x.combat.prev_position = x.position;
                }
                self.ags.prev_known.insert(u);
            }
            // A delivery's callback (its `path_ret` right before in a recording).
            if kind == AgEntry::Done
                && self.ags.strict
                && !r["path"].as_str().is_some_and(|k| self.ag_deliver(k))
            {
                Self::ag_unsupported(w, "a taken OnPathComplete without its delivery");
            }
            if let Err(d) = self.ag_take_span(w, u, end_ev, fname.as_deref()) {
                Self::ag_unsupported(w, format!("a taken entry: {}", d.0));
            }
            return self.i;
        }
        let depth = self.ags.ticks.len();
        let res = match kind {
            AgEntry::Ctl => self.ag_tick_unit(w, u),
            AgEntry::Req => self.ag_find_entry(w, u, r),
            AgEntry::Done => self.ag_done(w, u, r),
            AgEntry::Call => self.ag_call_entry(w, u, r),
        };
        if let Err(d) = res {
            self.ags.ticks.truncate(depth);
            self.ags.req_want = None;
            self.ags.ctl_unit = None;
            if std::env::var_os("WR_DEBUG_AGENT").is_some() {
                eprintln!("DEBUG agent tick {}: {}", self.tick, d.0);
            }
            Self::ag_unsupported(w, &d.0);
            // The rest of the entry, up to its end record, is skipped (its agent records too).
            while let Some(x) = trace.records.get(self.i) {
                if x["ev"] == end_ev
                    && x["u"].as_str() == Some(id.as_str())
                    && fname.as_deref().is_none_or(|f| x["fn"] == f)
                {
                    self.i += 1;
                    if let Some(s) = x.get("s").filter(|s| s.is_object()) {
                        let s = s.clone();
                        self.ag_check_state(w, u, &s, "after a desynced entry");
                    }
                    break;
                }
                if x["k"] == "tick" {
                    break;
                }
                if self.ag_stop(x) {
                    self.ag_skip_stop(w, x);
                    self.i += 1;
                    continue;
                }
                let at = self.i;
                self.i = self.record(w, at);
            }
        }
        self.i
    }

    /// `TickUnit`'s start (`ag_ctl`): the batch's next unit, its position and `PreviousPosition`
    /// the sim's (the first one of a unit the window's start didn't give is taken).
    fn ag_ctl_start(&mut self, w: &mut World, u: EntityIdx, r: &Rv) -> R<()> {
        self.ag_batch_take(w, u)?;
        let (Some(pos), Some(prev), true) =
            (v3(&r["pos"]), v3(&r["prev"]), fields(r, &["u", "pos", "prev"]))
        else {
            return Err(Desync(format!("a TickUnit record it can't read ({})", describe(r))));
        };
        let unit = w.unit(u).ok_or_else(|| Desync("no unit".into()))?;
        if bits3(pos) != bits3(unit.position) {
            return Err(Desync(format!(
                "TickUnit: the unit's position {:?} differs from the recording's {pos:?}",
                unit.position
            )));
        }
        if self.ags.prev_known.contains(&u) {
            if bits3(prev) != bits3(unit.combat.prev_position) {
                return Err(Desync(format!(
                    "TickUnit: PreviousPosition {:?} differs from the recording's {prev:?}",
                    unit.combat.prev_position
                )));
            }
        } else {
            self.ag_input("PreviousPosition at the window's start");
        }
        Ok(())
    }

    /// `ag_ctl` .. `ag_ctl_end`: `UnitMoveController.TickUnit`.
    fn ag_tick_unit(&mut self, w: &mut World, u: EntityIdx) -> R<()> {
        self.place(w, Phase::Move, "UnitMoveController.TickUnit");
        let r = self.ag_take(w, u, "ag_ctl", "TickUnit")?;
        self.ag_ctl_start(w, u, &r)?;
        self.ags.prev_known.insert(u);
        // (Until its end record: a position write recorded inside it isn't the observer's.)
        self.ags.ctl_unit = Some(u);
        movement::tick_unit(w, u, self)?;
        let e = self.ag_take(w, u, "ag_ctl_end", "TickUnit's end")?;
        self.ags.ctl_unit = None;
        let has = self.ag_has(u);
        let shape = if has {
            fields(&e, &["u", "pos", "orient", "vpos", "s"])
        } else {
            fields(&e, &["u", "pos", "orient", "vpos"])
        };
        if !shape {
            return Err(Desync(format!("a TickUnit end it can't read ({})", describe(&e))));
        }
        let unit = w.unit(u).ok_or_else(|| Desync("no unit".into()))?;
        let (pos, view, orient) = (unit.position, unit.combat.agent.pos, unit.orientation);
        if v3(&e["pos"]).map(bits3) != Some(bits3(pos))
            || (has && v3(&e["vpos"]).map(bits3) != Some(bits3(view)))
            || f(&e["orient"]).map(f32::to_bits) != Some(orient.to_bits())
        {
            return Err(Desync(format!(
                "TickUnit's end: position {pos:?}, view {view:?}, orientation {orient}; the recording's {}, {}, {}",
                e["pos"], e["vpos"], e["orient"]
            )));
        }
        if has {
            // The agent's state after the tick (`UpdateVelocity` included).
            self.ag_check_state(w, u, &e["s"], "TickUnit's end");
        } else {
            self.ag_input("a unit without an agent (its view's position)");
        }
        self.out.ag_ticks += 1;
        Ok(())
    }

    /// `ag_req` .. `ag_req_end` from outside the commands: the AI's `FindPath` (its arguments the
    /// AI's, inputs; the method the sim's). A `PathTo` here has no caller the sim runs.
    fn ag_find_entry(&mut self, w: &mut World, u: EntityIdx, r: &Rv) -> R<()> {
        if r["fn"] != "find" {
            return Err(Desync(
                "a PathTo the sim's commands didn't make (its caller isn't recorded)".into(),
            ));
        }
        let (Some(p), Some(approach), Some(los), Some(ibr)) =
            (v3(&r["p"]), opt_f(&r["r"]), r["los"].as_bool(), f(&r["ibr"]))
        else {
            return Err(Desync(format!("a FindPath record it can't read ({})", describe(r))));
        };
        self.ag_input("FindPath's arguments (the AI's ScorePath)");
        self.nav_find_start();
        self.ags.req_want = Some(vec![("fn", Want::S(Some("find".into())))]);
        ag::find_path(w, u, p, approach, los, ibr, self)?;
        Ok(())
    }

    /// `ag_done` .. `ag_done_end`: `OnPathComplete` of a delivered path (when and its contents
    /// are inputs until paths are computed): a request of the window delivered once, in a
    /// recording right after `PathSync`'s `path_ret` (a replay's trace has none).
    fn ag_done(&mut self, w: &mut World, u: EntityIdx, r: &Rv) -> R<()> {
        let key =
            r["path"].as_str().ok_or_else(|| Desync("OnPathComplete: no path".into()))?.to_owned();
        if !self.ag_deliver(&key) {
            return Err(Desync(format!("OnPathComplete of {key} without its delivery")));
        }
        let pts: Vec<V3> = r["v"]
            .as_array()
            .ok_or_else(|| Desync("OnPathComplete: no points".into()))?
            .iter()
            .map(v3)
            .collect::<Option<_>>()
            .ok_or_else(|| Desync("OnPathComplete: unreadable points".into()))?;
        let err = boolean(r, "err", "OnPathComplete")?;
        if r["forced"] != false {
            return Err(Desync("a delivered path completed as forced".into()));
        }
        // The sim's path when it computes it (the recorded one a check), else the recording's.
        let cs = r["cs"].as_i64().ok_or_else(|| Desync("OnPathComplete: no state".into()))?;
        let kind = r["kind"].as_str().unwrap_or("");
        let pts = match self.nav_delivered(&key, Some((&pts, err, cs, kind)))? {
            Some(v) => {
                self.out.nav.delivered_to_agent += 1;
                v
            }
            None => {
                self.ag_input("path contents");
                pts
            }
        };
        self.ags.paths.insert(key.clone(), pts.clone());
        let p = AgentPath { forced: key.starts_with("?ForcedPath"), key, points: pts };
        ag::on_path_complete(w, u, &p, err, false, self)
    }

    /// A top-level `ag_call`: the agent's method as code the sim doesn't run called it (an
    /// input, counted by its caller); a call of code the sim runs is a desync.
    fn ag_call_entry(&mut self, w: &mut World, u: EntityIdx, r: &Rv) -> R<()> {
        let origin = r["origin"]
            .as_str()
            .ok_or_else(|| Desync("an agent call without its caller".into()))?;
        let fname = r["fn"].as_str().unwrap_or("");
        let frames: Vec<&str> = origin.split(" < ").collect();
        let via_stop_moving = frames.get(1) == Some(&"Kingmaker.View.UnitEntityView.StopMoving");
        let by = frames.get(if via_stop_moving { 2 } else { 1 }).copied().unwrap_or("?");
        let modelled = [
            "Kingmaker.UnitLogic.Commands.Base.UnitCommand.TickApproaching",
            "Kingmaker.UnitLogic.Commands.UnitMoveTo.OnAction",
        ];
        if modelled.contains(&by) {
            return Err(Desync(format!("{fname} from {by}: the sim's commands didn't make it")));
        }
        if via_stop_moving && by == "Kingmaker.Controllers.Units.UnitCommandController.TickOnUnit" {
            // A unit without commands: its view's gates (view, rigidbody, getting up) aren't
            // recorded for it, so whether it's stopped is an input.
            if crate::combat::commands::busy(w, u) {
                return Err(Desync(
                    "StopMoving of a unit the sim's commands tick: the sim's commands didn't make it".into(),
                ));
            }
            self.ag_input("Stop (TickOnUnit: a unit without commands)");
        } else {
            self.ag_input(format!(
                "{fname} (from {})",
                caller(origin, if via_stop_moving { 2 } else { 1 })
            ));
        }
        match fname {
            "Stop" => {
                ag::stop(w, u, self)?;
                // `UnitEntityView.EnterProneState`: `IsProne = true; StopMoving();
                // ObstaclesHelper.RemoveFromGroup(AgentASP); Wake(6f)`.
                if by == "Kingmaker.View.UnitEntityView.EnterProneState" {
                    ag::remove_from_group(w, u, self)?;
                }
                Ok(())
            }
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
                // The path's own error flag: its definition at its first reference.
                let Some(&(err, ..)) = self.ags.defs.get(&key) else {
                    return Err(Desync(format!("{fname}: the path {key} isn't defined")));
                };
                self.ags.paths.insert(key.clone(), pts.clone());
                let p = AgentPath { forced: key.starts_with("?ForcedPath"), key, points: pts };
                let arg = |k: usize| r["args"][k].as_str().unwrap_or("").to_owned();
                let flag = match arg(0).as_str() {
                    "1" => true,
                    "0" => false,
                    _ => return Err(Desync(format!("{fname}: its flag isn't a boolean"))),
                };
                if fname == "FollowPrecomputedPath" {
                    let max = arg(1)
                        .parse::<f32>()
                        .map_err(|_| Desync("FollowPrecomputedPath: no radius".into()))?;
                    ag::follow_precomputed_path(w, u, &p, err, flag, max, self)
                } else {
                    ag::force_path(w, u, &p, err, flag, self)
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
        let mut last: Option<Rv> = None;
        loop {
            let Some(x) = trace.records.get(self.i) else {
                return Err(Desync("the recording ends".into()));
            };
            let mine = x["k"] == "obs" && x["u"].as_str() == Some(id.as_str());
            if mine
                && x.get("s").is_some_and(Rv::is_object)
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
                    match (pos, orient) {
                        (Some(p), Some(o)) => {
                            unit.position = p;
                            unit.orientation = o;
                            if let Some(v) = view {
                                unit.combat.agent.pos = v;
                            }
                        }
                        _ => return Err(Desync("a TickUnit end it can't read".into())),
                    }
                }
                break;
            }
            if x["k"] == "tick" {
                return Err(Desync("an agent entry without its end in its frame".into()));
            }
            if self.ag_stop(x) {
                self.ag_skip_stop(w, x);
                self.i += 1;
                continue;
            }
            let i = self.i;
            self.i = self.record(w, i);
        }
        if let Some(s) = last {
            let a = agent_from(&s, &self.ags.paths)
                .ok_or_else(|| Desync("a recorded state it can't read".into()))?;
            self.ag_carry(&a);
            let view = w.unit(u).map(|x| x.combat.agent.pos);
            let unit = w.unit_mut(u).ok_or_else(|| Desync("no unit".into()))?;
            unit.combat.agent = a;
            // (A TickUnit's view position is its end's; the agent's state was recorded before.)
            if end_ev == "ag_ctl_end"
                && let Some(v) = view
            {
                unit.combat.agent.pos = v;
            }
            self.ags.last_state.insert(u, s);
        }
        Ok(())
    }

    /// A path delivered (`PathSync`'s `path_ret`): the next `OnPathComplete` is its; before
    /// FindPath's own callback record (`ag_findclr`, since poc5-tb-109) an agent's `FindPath`
    /// request is cleared here.
    pub(super) fn ag_path_ret(&mut self, w: &mut World, r: &Rv) {
        let Some(key) = r["path"].as_str() else {
            Self::ag_unsupported(w, "a path delivery it can't read");
            return;
        };
        if self.ags.strict {
            if self.ags.last_ret.take().is_some() {
                Self::ag_unsupported(w, "a delivery (path_ret) without its callback");
            }
            if !fields(r, &["path", "c", "req"])
                || !self.ags.started.contains(key)
                || r["req"].as_i64() != self.ags.started_at.get(key).copied()
                || !self.ags.returned.insert(key.to_owned())
            {
                Self::ag_unsupported(w, "a delivery (path_ret) of no request in flight");
            }
        }
        self.ags.last_ret = Some(key.to_owned());
        if self.ags.has_findclr {
            return;
        }
        for u in self.ags.has.iter().copied().collect::<Vec<_>>() {
            if let Some(unit) = w.unit_mut(u)
                && unit.combat.agent.requested_find.as_ref().is_some_and(|p| p.key == key)
            {
                unit.combat.agent.requested_find = None;
            }
        }
    }

    /// A request started outside the sim's agent code (`PathSync`'s `path_start`): counted for
    /// its requester; in turn-based combat an agent's is a desync (its requests are the sim's).
    pub(super) fn ag_path_start(&mut self, w: &mut World, r: &Rv) {
        self.ag_count_request(r);
        let who = r["path"].as_str().and_then(|k| k.rsplit_once('#')).map(|(w, _)| w.to_owned());
        let agent = who.and_then(|x| w.by_id.get(&x).copied()).is_some_and(|u| self.ag_has(u));
        if agent && w.combat.turn_based && self.ags.synced && !self.uc_skipping {
            Self::ag_unsupported(w, "an agent's path request outside the sim's agent code");
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
        self.ags.last_state.get(&u).and_then(|s| s[k].as_bool())
    }

    /// `unit.Commands.Move` in the commands model: the move slot's command, unless finished, if
    /// it is a `UnitMoveTo`.
    fn ag_move_command(&self, w: &World, u: EntityIdx) -> R<Option<u32>> {
        let id = &w.entity(u).unique_id;
        let m = &w.combat.commands;
        let Some(s) = m.set_of(id) else {
            return Err(Desync(format!("Commands.Move of {id}: its commands aren't known")));
        };
        let Some(c) = s.slots[3] else { return Ok(None) };
        let x = m.get(c).ok_or_else(|| Desync(format!("command {c} is unknown")))?;
        Ok((!x.s.finished && x.is("UnitMoveTo")).then_some(c))
    }

    /// The commands' calls into the agent (`uc_call`, the record `rec`) in turn-based combat:
    /// `View.MoveTo` runs `PathTo` (with an agent: `AstarPath` is active), `FollowPrecomputedPath`
    /// a new `ForcedPath` of the command's path, `View.StopMoving` `Stop`. In real time the
    /// agent's span comes as its own entry (taken).
    pub(super) fn ag_command_call(&mut self, w: &mut World, call: &Call, rec: &Rv) -> R<()> {
        if !self.ag || !w.combat.turn_based {
            return Ok(());
        }
        if !self.ags.synced {
            return Err(Desync(
                "the commands call the agent after real time without a synchronization (ag_sync)"
                    .into(),
            ));
        }
        let depth = self.ags.ticks.len();
        let res = self.ag_command_call_body(w, call, rec);
        if res.is_err() {
            self.ags.ticks.truncate(depth);
            self.ags.req_want = None;
        }
        self.out.ag_command_calls += 1;
        res
    }

    fn ag_command_call_body(&mut self, w: &mut World, call: &Call, rec: &Rv) -> R<()> {
        let unit_of = |w: &World, id: &str| {
            w.by_id.get(id).copied().ok_or_else(|| Desync(format!("unit {id} is unknown")))
        };
        let num = |k: &str| {
            rec[k]
                .as_str()
                .and_then(|v| v.parse::<f32>().ok())
                .ok_or_else(|| Desync(format!("a uc_call without {k}")))
        };
        match call {
            Call::MoveTo { c, unit, point, radius, max_radius, target, xz } => {
                let u = unit_of(w, unit)?;
                if !self.ag_has(u) {
                    return Ok(());
                }
                let recorded = v3(&rec["p"]).ok_or_else(|| Desync("a uc_call without p".into()))?;
                let point = match point {
                    Some(p) => *p,
                    None => self.ag_target_point(w, *c, recorded)?,
                };
                let radius = radius.map_or_else(|| num("r"), Ok)?;
                let max_radius = max_radius.map_or_else(|| num("mr"), Ok)?;
                let tu = unit_of(w, target)?;
                // `targetUnit.AgentASP` (the view's: a live unit's view has its data).
                let dest = self
                    .ag_has(tu)
                    .then(|| DestAgent { view: Some(target.clone()), attached: true });
                let dest_is_self = dest.is_some() && target == unit;
                let same =
                    w.unit(u).map(|x| x.combat.agent.dest_agent.clone()) == Some(dest.clone());
                let has = dest.is_some();
                self.ags.req_want = Some(vec![
                    ("fn", Want::S(Some("pathto".into()))),
                    ("c", Want::Id(Some(*c))),
                    ("p", Want::V(point)),
                    ("r", Want::F(radius)),
                    ("mr", Want::F(max_radius)),
                    ("xz", Want::B(*xz)),
                    ("da", Want::S(has.then(|| target.clone()))),
                    ("dself", Want::B(dest_is_self)),
                    ("dhas", Want::B(has)),
                    ("dview", Want::S(has.then(|| target.clone()))),
                    ("dattached", Want::B(has)),
                    ("dsame", Want::B(same)),
                ]);
                ag::path_to(
                    w,
                    u,
                    Some(*c),
                    point,
                    radius,
                    max_radius,
                    *xz,
                    dest,
                    dest_is_self,
                    self,
                )?;
                Ok(())
            }
            Call::Follow { unit, path, forced, radius, .. } => {
                let u = unit_of(w, unit)?;
                if !self.ag_has(u) {
                    return Err(Desync("FollowPrecomputedPath of a unit without an agent".into()));
                }
                // The new `ForcedPath`'s name: the observer's for the next path it sees.
                let id = unit.clone();
                let key = self.trace.records[self.i..]
                    .iter()
                    .take_while(|x| x["k"] != "tick")
                    .find(|x| x["ev"] == "ag_call" && x["u"].as_str() == Some(id.as_str()))
                    .and_then(|x| x["rq"]["path"].as_str().map(str::to_owned))
                    .ok_or_else(|| {
                        Desync("FollowPrecomputedPath: the recording has no call".into())
                    })?;
                if !key.starts_with("?ForcedPath#")
                    || self.ags.paths.contains_key(&key)
                    || self.ags.defs.contains_key(&key)
                {
                    return Err(Desync(format!(
                        "FollowPrecomputedPath: {key} isn't a new ForcedPath's name"
                    )));
                }
                let max = radius.map_or_else(|| num("r"), Ok)?;
                let p = AgentPath { key: key.clone(), points: path.clone(), forced: true };
                self.ags.constructed.insert(key.clone());
                self.ags.follow_rec = None;
                // `new ForcedPath(points)`: `CompleteState` Complete, no error.
                ag::follow_precomputed_path(w, u, &p, false, *forced, max, self)?;
                let at = self
                    .ags
                    .follow_rec
                    .take()
                    .ok_or_else(|| Desync("FollowPrecomputedPath: no call".into()))?;
                let r = &self.trace.records[at];
                let rq = &r["rq"];
                let pts: Option<Vec<V3>> =
                    r["v"].as_array().and_then(|v| v.iter().map(v3).collect());
                let args_ok = r["args"].as_array().is_some_and(|a| {
                    a.len() == 2
                        && a[0].as_str() == Some(if *forced { "1" } else { "0" })
                        && a[1].as_str().and_then(|x| x.parse::<f32>().ok()).map(f32::to_bits)
                            == Some(max.to_bits())
                });
                let same = |a: &[V3], b: &[V3]| {
                    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| bits3(*x) == bits3(*y))
                };
                // `new ForcedPath(points)`: `Path`'s defaults (`enabledTags` -1,
                // `PathNNConstraint.Default`: every graph, not XZ), never started (`pathID` 0).
                let ok = rq["path"].as_str() == Some(key.as_str())
                    && rq["kind"] == "ForcedPath"
                    && rq["tags"] == -1
                    && rq["gmask"] == -1
                    && rq["xz"] == false
                    && ["start", "end", "clear", "ec", "ar", "los", "ibr", "maxd"]
                        .iter()
                        .all(|k| rq.get(*k).is_some_and(Rv::is_null))
                    && pts.as_deref().is_some_and(|v| same(v, path))
                    && args_ok
                    && self.ags.defs.get(&key) == Some(&(false, 2, "ForcedPath".to_owned(), 0))
                    && self.ags.paths.get(&key).is_some_and(|v| same(v, path));
                if !ok {
                    return Err(Desync(format!(
                        "FollowPrecomputedPath of the command's path: the recording's call is {}",
                        describe(r)
                    )));
                }
                Ok(())
            }
            Call::Stop { unit, .. } => {
                let u = unit_of(w, unit)?;
                if !self.ag_has(u) {
                    return Ok(());
                }
                ag::stop(w, u, self)
            }
        }
    }

    /// `ApproachPoint` (`GetTargetPoint()`) of a command whose point the commands model leaves
    /// open: a unit target's position (`Target.Point`) is the sim's for the classes that don't
    /// override it; another class's is its record's (`uc_call.p`, an input).
    fn ag_target_point(&mut self, w: &World, c: u32, recorded: V3) -> R<V3> {
        let x =
            w.combat.commands.get(c).ok_or_else(|| Desync(format!("command {c} is unknown")))?;
        let base = matches!(x.class.as_str(), "UnitAttack" | "UnitAttackOfOpportunity")
            || (x.class == "UnitMoveTo" && x.mv.is_some_and(|m| !m.rider_target));
        let unit = x.s.target.unit().and_then(|t| w.by_id.get(t).copied());
        match (base, unit) {
            (true, Some(t)) => {
                let p =
                    w.unit(t).ok_or_else(|| Desync("a target that isn't a unit".into()))?.position;
                if bits3(p) != bits3(recorded) {
                    return Err(Desync(format!(
                        "MoveTo's point: the target's position {p:?}, the recording's {recorded:?}"
                    )));
                }
                self.out.ag_reads += 1;
                Ok(p)
            }
            _ => {
                self.ag_input(format!("MoveTo's point ({})", x.class));
                Ok(recorded)
            }
        }
    }

    /// The commands' agent leaf `f` of `unit` (`moving`: the view's `IsMoving()`; `rmoving`:
    /// `MovementAgent.IsReallyMoving`; `wants`: `AgentASP.WantsToMove`) as the sim's agent has
    /// it, in turn-based combat; `None` where it's still the recording's (real time).
    pub(super) fn ag_command_leaf(
        &mut self,
        w: &mut World,
        f: &str,
        unit: Option<&str>,
    ) -> R<Option<bool>> {
        if !self.ag || !w.combat.turn_based || !matches!(f, "moving" | "rmoving" | "wants") {
            return Ok(None);
        }
        if !self.ags.synced {
            return Err(Desync(format!(
                "the commands read the agent's {f} after real time without a synchronization (ag_sync)"
            )));
        }
        let Some(u) = unit.and_then(|x| w.by_id.get(x).copied()) else {
            return Err(Desync(format!("the commands read {f} of an unknown unit")));
        };
        let has = self.ag_has(u);
        let v = match f {
            "moving" => has && ag::wants_to_move(w, u)?,
            _ if !has => {
                return Err(Desync(format!("the commands read {f} of a unit without an agent")));
            }
            "wants" => ag::wants_to_move(w, u)?,
            _ => {
                let depth = self.ags.ticks.len();
                let v = ag::is_really_moving(w, u, self);
                self.ags.ticks.truncate(depth);
                v?
            }
        };
        self.out.ag_reads += 1;
        Ok(Some(v))
    }

    /// `ContinueActing`'s reads of the agent (`tt_ca`: `WantsToMove`, the uninterruptable
    /// command's flag, `HasMotionThisTick`) against the sim's.
    pub(super) fn ag_turn_reads(&mut self, w: &mut World, u: EntityIdx, r: &Rv) {
        if !self.ag || !w.combat.turn_based {
            return;
        }
        if !self.ags.synced {
            Self::ag_unsupported(
                w,
                "ContinueActing reads the agent after real time without a synchronization (ag_sync)",
            );
            return;
        }
        let has = self.ag_has(u);
        let Some(x) = w.unit(u) else { return };
        let a = &x.combat.agent;
        let wants = has.then(|| a.path.is_some() || a.requested.is_some());
        let nomore = has.then_some(a.no_more_tb);
        let motion = orientation::has_motion(x.position, x.combat.prev_position);
        let rec = |k: &str| if r[k].is_null() { Some(None) } else { r[k].as_bool().map(Some) };
        if rec("wants") != Some(wants)
            || rec("nomore") != Some(nomore)
            || r["motion"].as_bool() != Some(motion)
        {
            Self::ag_unsupported(
                w,
                "ContinueActing: the agent's WantsToMove, its uninterruptable flag or the unit's motion differs from the sim's",
            );
        }
        self.out.ag_reads += 3;
    }

    /// `CheckAndClearWantsToMoveInTurnBased()` cleared the flag (the turn port says so).
    pub(super) fn ag_turn_cleared(&mut self, w: &mut World, u: EntityIdx) {
        if self.ag
            && self.ag_has(u)
            && let Some(x) = w.unit_mut(u)
        {
            x.combat.agent.no_more_tb = false;
        }
    }

    /// An observation row (`Observe.Tick`, a frame's prefix): the unit's position (and, in
    /// turn-based combat, `View.IsMoving()`) are the sim's.
    pub(super) fn ag_row(&mut self, w: &mut World, unit: &str, row: &Row) {
        if self.ags.rows.0 != self.tick {
            self.ags.rows = (self.tick, HashSet::new());
        }
        self.ags.rows.1.insert(unit.to_owned());
        // (The window's first rows come before the initial states.)
        if self.ags.init_seen {
            self.ag_row_check(w, unit, row);
        }
    }

    fn ag_row_check(&mut self, w: &mut World, unit: &str, row: &Row) {
        let Some(u) = w.by_id.get(unit).copied() else { return };
        let Some(x) = w.unit(u) else { return };
        if bits3(x.position) != bits3(row.pos) {
            if std::env::var_os("WR_DEBUG_AGENT").is_some() {
                eprintln!(
                    "DEBUG agent tick {}: row of {unit}: sim {:?}, row {:?}",
                    self.tick, x.position, row.pos
                );
            }
            Self::ag_unsupported(w, "an observation row's position differs from the sim's");
            if let Some(x) = w.unit_mut(u) {
                x.position = row.pos;
            }
        }
        if w.combat.turn_based && self.ags.synced {
            let moving = self.ag_has(u) && ag::wants_to_move(w, u).unwrap_or(false);
            if moving != row.moving {
                Self::ag_unsupported(
                    w,
                    "an observation row's IsMoving differs from the sim's agent",
                );
            }
        }
        self.out.ag_rows += 1;
    }

    /// The frame's rows not written (the row didn't change): the awake units' are still the
    /// sim's. At the frame's first controller phase.
    pub(super) fn ag_rows_check(&mut self, w: &mut World) {
        if self.ags.rows_checked == Some(self.tick) || !self.ags.init_seen {
            return;
        }
        self.ags.rows_checked = Some(self.tick);
        let written =
            if self.ags.rows.0 == self.tick { self.ags.rows.1.clone() } else { HashSet::new() };
        for u in w.combat.awake.clone() {
            let id = w.entity(u).unique_id.clone();
            if !self.pos.known.contains(&id) || written.contains(&id) {
                continue;
            }
            if let Some(row) = self.pos.last.get(&id).copied() {
                self.ag_row_check(w, &id, &row);
            }
        }
    }

    /// Combat starts. A diagnostic run of an older capture (`WR_AGENT_DRIVEN`, no `agent_sync`)
    /// synthesizes the synchronization from each agent's last recorded state (reported: such a
    /// run is never valid).
    pub(super) fn ag_combat_start(&mut self, w: &mut World) {
        // `HandleCombatStart`'s graph part (`Clear`), before `m_Units` is filled (paths computed).
        crate::combat::paths::combat_start(w);
        if self.ag && self.ags.strict {
            // The observer writes its snapshot right after: until then the agents aren't the
            // sim's.
            self.ags.boundary = true;
            self.ags.synced = false;
        }
        if !self.ag || self.ags.strict || self.ags.synced {
            return;
        }
        Self::ag_unsupported(
            w,
            "a synchronization synthesized from the last recorded states (WR_AGENT_DRIVEN, diagnostic)",
        );
        for (u, s) in self.ags.last_state.clone() {
            if let (Some(a), Some(unit)) = (agent_from(&s, &self.ags.paths), w.unit_mut(u)) {
                let pos = unit.combat.agent.pos;
                unit.combat.agent = a;
                unit.combat.agent.pos = pos;
            }
        }
        self.ags.synced = true;
    }
}

impl AgentPhysics for Driver<'_> {
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
}

impl AgentInputs for Driver<'_> {
    fn is_done(&mut self, w: &mut World, u: EntityIdx, path: &str) -> R<bool> {
        let r = self.ag_leaf(w, u, "isdone")?;
        if r["path"].as_str() != Some(path) {
            return Err(Desync(format!("IsDone of {path}: the recording's is of {}", r["path"])));
        }
        let v = boolean(&r, "v", "isdone")?;
        // A `ForcedPath` the sim constructed is never queued: its `PipelineState` stays `Created`.
        if self.ags.constructed.contains(path) {
            if v {
                return Err(Desync(format!(
                    "IsDone of the constructed {path}: the recording's is true"
                )));
            }
            self.out.ag_reads += 1;
            return Ok(false);
        }
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
    fn can_move_end(&mut self, w: &mut World, u: EntityIdx, result: bool, dt: f32) -> R<()> {
        let r = self.ag_take(w, u, "ag_v", "CanMoveInTurnBased's end")?;
        if r["f"] != "canmovetb"
            || !fields(&r, &["f", "u", "v", "dt"])
            || r["v"].as_bool() != Some(result)
            || f(&r["dt"]).map(f32::to_bits) != Some(dt.to_bits())
        {
            return Err(Desync(format!(
                "CanMoveInTurnBased returned {result} with {dt}; the recording has {}",
                describe(&r)
            )));
        }
        Ok(())
    }
    fn turn_tick_movement(&mut self, w: &mut World, u: EntityIdx, dt: f32, force: bool) -> R<f32> {
        let id = self.ag_unit_id(w, u);
        let near = ag::near_the_end(w, u)?;
        let at = self.ag_pull(w, "the turn's TickMovement (cd_move)", |r| {
            r["k"] == "obs" && r["ev"] == "cd_move"
        })?;
        let r = self.trace.records[at].clone();
        let num = |k: &str| r[k].as_str().and_then(|x| x.parse::<f32>().ok());
        if r["u"].as_str() != Some(id.as_str())
            || num("dt").map(f32::to_bits) != Some(dt.to_bits())
            || r["force"].as_bool() != Some(force)
            || r["near"].as_bool() != Some(near)
        {
            return Err(Desync(format!(
                "TickMovement({dt}, force {force}, near {near}): the turn's call was {}",
                describe(&r)
            )));
        }
        // The cooldowns' own handler runs the turn's budget (its other fields stay inputs) and
        // leaves the delta it computed.
        self.ags.cd_out = None;
        self.i = at;
        let i = self.i;
        self.i = self.record(w, i);
        self.ags.cd_out.take().ok_or_else(|| {
            Desync("the turn's TickMovement left no delta (its record unread)".into())
        })
    }
    fn cutscene_active(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        let held = boolean(&self.ag_leaf(w, u, "csctl")?, "v", "csctl")?;
        Ok(held && boolean(&self.ag_leaf(w, u, "csactive")?, "v", "csactive")?)
    }
    fn move_dont_interrupt(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        // `Commands.Move?.DontInterruptInTurnBasedMode`: the commands model's.
        Ok(match self.ag_move_command(w, u)? {
            Some(c) => w.combat.commands.get(c).is_some_and(|x| x.dont_interrupt_tb),
            None => false,
        })
    }
    fn nav_obstacles(&mut self, _: &mut World) -> R<bool> {
        Ok(self.ags.ticks.last().is_some_and(|(_, t)| t["s"]["navobst"] == true))
    }
    fn gamepad(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        boolean(&self.ag_leaf(w, u, "gamepad")?, "v", "gamepad")
    }
    fn random_range(&mut self, w: &mut World, _: EntityIdx, min: f32, max: f32) -> R<f32> {
        self.ag_draw(w, min, max)
    }
    fn rider_in_combat(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(false)
    }
    fn anim_prevents(&mut self, w: &mut World, u: EntityIdx) -> R<Option<bool>> {
        let next = self.ag_peek();
        if next.is_some_and(|r| r["ev"] == "ag_v" && r["f"] == "animprev") {
            return Ok(Some(boolean(&self.ag_leaf(w, u, "animprev")?, "v", "animprev")?));
        }
        self.ag_input("no animation manager");
        Ok(None)
    }
    fn commands_prevent(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        boolean(&self.ag_leaf(w, u, "cmdprev")?, "v", "cmdprev")
    }
    fn max_speed(&mut self, w: &mut World, u: EntityIdx) -> R<f32> {
        f(&self.ag_leaf(w, u, "maxspeed")?["v"])
            .ok_or_else(|| Desync("maxspeed without a value".into()))
    }
    fn fly_height(&mut self, w: &mut World, u: EntityIdx) -> R<f32> {
        f(&self.ag_leaf(w, u, "fly")?["v"]).ok_or_else(|| Desync("fly without a value".into()))
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
        let id = self.ag_unit_id(w, u);
        let at = self.ag_pull(w, &format!("the agent's {fname}"), |r| {
            r["k"] == "obs" && r["ev"] == "ag_call"
        })?;
        let r = self.trace.records[at].clone();
        let path = matches!(fname, "FollowPrecomputedPath" | "ForcePath");
        let shape = if path {
            fields(&r, &["fn", "u", "origin", "i", "rq", "v", "args", "s"])
        } else {
            fields(&r, &["fn", "u", "origin", "i"])
        };
        if r["u"].as_str() != Some(id.as_str())
            || r["fn"] != fname
            || !shape
            || !r["origin"].is_string()
            || r["i"].as_i64().and_then(|x| i32::try_from(x).ok()) != index
            || (index.is_none() && !r["i"].is_null())
        {
            return Err(Desync(format!(
                "the agent calls {fname}({index:?}) of {id}, the recording has {}",
                describe(&r)
            )));
        }
        if path {
            // The state at its start (the AI's FindPath callback may run before, unobserved).
            self.ag_check_state(w, u, &r["s"], &format!("{fname}'s start"));
            self.ags.follow_rec = Some(at);
        }
        Ok(())
    }
    fn call_end(&mut self, w: &mut World, u: EntityIdx, fname: &'static str) -> R<()> {
        let r = self.ag_take(w, u, "ag_call_end", &format!("{fname}'s end"))?;
        if r["fn"] != fname || !fields(&r, &["fn", "u", "s"]) {
            return Err(Desync(format!("{fname} returned; the recording has {}", describe(&r))));
        }
        self.ag_check_state(w, u, &r["s"], &format!("{fname}'s end"));
        Ok(())
    }
    fn tick_start(&mut self, w: &mut World, u: EntityIdx, dt: f32) -> R<()> {
        let r = self.ag_take(w, u, "ag_tick", "TickMovement")?;
        if !fields(&r, &["u", "dt", "combat", "tb", "turn", "cur", "acting", "dontint", "s"])
            || f(&r["dt"]).map(f32::to_bits) != Some(dt.to_bits())
        {
            return Err(Desync(format!("TickMovement({dt}): the recording's is {}", describe(&r))));
        }
        // The turn and commands state `CanMoveInTurnBased` reads, the sim's against the
        // recording's (each strictly a boolean; the move command's flag null without one).
        let turn = w.combat.current_unit.is_some();
        let cur = w.combat.current_unit == Some(u);
        let acting = turn && w.combat.turns.ctl.status == crate::combat::turn_tick::Status::Acting;
        let combat = w.unit(u).is_some_and(|x| x.combat.in_combat);
        let dontint = match self.ag_move_command(w, u)? {
            Some(c) => Rv::Bool(w.combat.commands.get(c).is_some_and(|x| x.dont_interrupt_tb)),
            None => Rv::Null,
        };
        let b = |k: &str| r[k].as_bool();
        if b("tb") != Some(true)
            || b("turn") != Some(turn)
            || b("cur") != Some(cur)
            || b("acting") != Some(acting)
            || b("combat") != Some(combat)
            || r["dontint"] != dontint
        {
            return Err(Desync(format!(
                "TickMovement's turn state (turn {turn}, current {cur}, acting {acting}, in combat {combat}, the move's flag {dontint}) differs from the recording's ({})",
                describe(&r)
            )));
        }
        self.ag_check_state(w, u, &r["s"], "TickMovement's start");
        self.ags.ticks.push((u, r));
        Ok(())
    }
    fn tick_end(&mut self, w: &mut World, u: EntityIdx, ret: V3) -> R<()> {
        let r = self.ag_take(w, u, "ag_tick_end", "TickMovement's end")?;
        if !fields(&r, &["u", "ret", "s"]) || v3(&r["ret"]).map(bits3) != Some(bits3(ret)) {
            return Err(Desync(format!("TickMovement returned {ret:?}, the game {}", r["ret"])));
        }
        self.ags.ticks.pop();
        self.ag_check_state(w, u, &r["s"], "TickMovement's end");
        Ok(())
    }
    fn done_start(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        p: &AgentPath,
        forced: bool,
        error: bool,
    ) -> R<()> {
        let path = p.key.as_str();
        let r = self.ag_take(w, u, "ag_done", "OnPathComplete")?;
        // `OnPathComplete(p, forcedPath)`'s prefix (`path_done`, right before `ag_done`): its
        // path's.
        self.nav_path_done(w, u, &p.key, error, &p.points)?;
        // The path's contents (a delivered one's are the record's own; a `ForcedPath` is
        // constructed `Complete`).
        let pts: Option<Vec<V3>> = r["v"].as_array().and_then(|v| v.iter().map(v3).collect());
        let same = pts.as_deref().is_some_and(|v| {
            v.len() == p.points.len()
                && v.iter().zip(&p.points).all(|(a, b)| bits3(*a) == bits3(*b))
        });
        if !same || (p.forced && (r["kind"] != "ForcedPath" || r["cs"] != 2)) {
            return Err(Desync(format!(
                "OnPathComplete of {path}: its contents differ from the path's ({})",
                describe(&r)
            )));
        }
        let current = w
            .unit(u)
            .and_then(|x| x.combat.agent.requested.as_ref())
            .is_some_and(|p| p.key == path);
        if r["path"].as_str() != Some(path)
            || r["forced"].as_bool() != Some(forced)
            || r["err"].as_bool() != Some(error)
            || r["cur"].as_bool() != Some(current)
            || !fields(&r, &["u", "path", "kind", "forced", "cur", "err", "cs", "combat", "v", "s"])
        {
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
        if !fields(&r, &["u", "s"]) {
            return Err(Desync(format!("OnPathComplete's end: {}", describe(&r))));
        }
        self.ag_check_state(w, u, &r["s"], "OnPathComplete's end");
        Ok(())
    }
    fn req_start(&mut self, w: &mut World, u: EntityIdx) -> R<()> {
        let r = self.ag_take(w, u, "ag_req", "PathTo/FindPath")?;
        if r["fn"] == "pathto" {
            let (p, radius) = (v3(&r["p"]), f(&r["r"]));
            let (Some(p), Some(radius)) = (p, radius) else {
                return Err(Desync(format!("a request record it can't read ({})", describe(&r))));
            };
            self.nav_path_to(w, u, p, radius)?;
        }
        let want = self.ags.req_want.take();
        let shape = match r["fn"].as_str() {
            Some("pathto") => fields(
                &r,
                &[
                    "fn",
                    "u",
                    "c",
                    "p",
                    "r",
                    "mr",
                    "xz",
                    "da",
                    "dself",
                    "dhas",
                    "dview",
                    "dattached",
                    "dsame",
                    "tile",
                    "frame",
                    "tb",
                    "s",
                ],
            ),
            Some("find") => fields(&r, &["fn", "tb", "u", "p", "r", "los", "ibr", "s"]),
            _ => false,
        };
        if !shape || r["tb"] != true {
            return Err(Desync(format!("a request record it can't read ({})", describe(&r))));
        }
        let Some(want) = want else {
            return Err(Desync("a request its caller didn't make".into()));
        };
        if let Some((k, v)) = want.iter().find(|(k, v)| !v.matches(&r[*k])) {
            return Err(Desync(format!(
                "the request's {k}: the sim's is {v:?}, the recording's {}",
                r[*k]
            )));
        }
        self.ag_check_state(w, u, &r["s"], "the request's start");
        self.ags.req_rec = Some(r);
        Ok(())
    }
    fn req_end(&mut self, w: &mut World, u: EntityIdx, request: Option<&PathRequest>) -> R<()> {
        let r = self.ag_take(w, u, "ag_req_end", "the request's end")?;
        let start = self.ags.req_rec.take().unwrap_or(Rv::Null);
        if r["fn"] != start["fn"] || !fields(&r, &["fn", "u", "rq", "s"]) {
            return Err(Desync(format!("a request's end it can't read ({})", describe(&r))));
        }
        // `PathTo` leaves `RequestedPath` (the old one when it made none); `FindPath` returns its
        // request or null.
        let made =
            !r["rq"].is_null() && (start["fn"] == "find" || r["s"]["req"] != start["s"]["req"]);
        match (request, made) {
            (Some(q), true) => {
                if start["fn"] == "find" {
                    self.ags.finding.insert(q.key.clone());
                }
                self.nav_request(w, u, q, start["fn"] == "find");
                let rq = &r["rq"];
                let ending = match q.approach {
                    Some(a) => {
                        rq["ec"] == "LosPathConditionForGrid"
                            && f(&rq["ar"]).map(f32::to_bits) == Some(a.to_bits())
                            && rq["los"].as_bool() == Some(q.los)
                            && f(&rq["ibr"]).map(f32::to_bits) == Some(q.ignore_blocker.to_bits())
                    }
                    None => {
                        rq["ec"] == "MaxDistanceEndingCondition"
                            && rq["ar"].is_null()
                            && rq["los"].is_null()
                            && rq["ibr"].is_null()
                    }
                };
                let same = rq["kind"] == "ABPathWithClearance"
                    && ending
                    && rq["path"].as_str() == Some(q.key.as_str())
                    && v3(&rq["start"]).map(bits3) == Some(bits3(q.start))
                    && v3(&rq["end"]).map(bits3) == Some(bits3(q.end))
                    && v3(&rq["clear"]).map(bits3) == Some(bits3(q.clearance))
                    && rq["tags"].as_i64() == Some(i64::from(q.tags))
                    && rq["gmask"].as_i64() == Some(i64::from(q.graph_mask))
                    && f(&rq["maxd"]).map(f32::to_bits) == Some(q.max_distance.to_bits())
                    && rq["xz"].as_bool() == Some(q.distance_xz);
                if !same {
                    return Err(Desync(format!("the request {q:?}: the recording's is {rq}")));
                }
                self.out.ag_requests += 1;
            }
            (None, false) => {}
            (Some(_), false) => {
                return Err(Desync("the sim made a request the game didn't".into()));
            }
            (None, true) => {
                return Err(Desync("the game made a request the sim's didn't".into()));
            }
        }
        self.ag_check_state(w, u, &r["s"], "the request's end");
        Ok(())
    }
    fn group_memory(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        boolean(&self.ag_leaf(w, u, "gmem")?, "v", "gmem")
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
        if !boolean(&r, "unit", "proxy")? {
            return Ok(Proxy::NotUnit);
        }
        Ok(Proxy::Unit(if boolean(&r, "agent", "proxy")? {
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
        if !boolean(&self.ag_leaf(w, u, "softcol")?, "v", "softcol")? {
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
        for u in self.ags.all.clone() {
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
        boolean(&r, "v", "navnode")
    }
    fn frames(&mut self, _: &mut World) -> R<(i32, i32)> {
        self.ag_input("frames");
        let r = self
            .ags
            .req_rec
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
    fn grid_erosion(&mut self, w: &mut World) -> R<Option<(i32, i32)>> {
        // The grid's current settings when the sim keeps it (combat changes `erodeIterations`),
        // else the area's nav file's.
        if let Some(e) = self.nav_erosion(w) {
            return Ok(e);
        }
        self.ags.erosion.clone().map_err(Desync)
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
        // `Unit.EntityData.Commands.Move`: the commands model's, its read checked.
        let r = self.ag_leaf(w, u, "move")?;
        let sim =
            match self.ag_move_command(w, u)? {
                Some(c) => {
                    let mv =
                        w.combat.commands.get(c).and_then(|x| x.mv).ok_or_else(|| {
                            Desync(format!("command {c} has no UnitMoveTo fields"))
                        })?;
                    Some((mv.movement_delay, mv.roaming))
                }
                None => None,
            };
        let rec = match boolean(&r, "v", "move")? {
            true => Some((
                f(&r["delay"]).ok_or_else(|| Desync("move without its delay".into()))?,
                boolean(&r, "roaming", "move")?,
            )),
            false => None,
        };
        if sim.map(|(d, x)| (d.to_bits(), x)) != rec.map(|(d, x)| (d.to_bits(), x)) {
            return Err(Desync(format!(
                "Commands.Move: the sim's is {sim:?}, the recording's {rec:?}"
            )));
        }
        Ok(sim)
    }
    fn request_key(&mut self, w: &mut World, u: EntityIdx, end: V3) -> R<String> {
        // `PathSync`'s key: the requester's own count of requests in the window.
        let id = self.ag_unit_id(w, u);
        let n = self.ags.req_counts.get(&id).copied().unwrap_or(0);
        let key = format!("{id}#{n}");
        let at = self.ag_pull(w, "the request's start (path_start)", |r| {
            r["k"] == "obs" && r["ev"] == "path_start"
        })?;
        let r = &self.trace.records[at];
        let strict = w.unit(u).is_some_and(|x| x.combat.in_combat)
            || crate::combat::units::is_player_faction(w, u);
        if r["path"].as_str() != Some(key.as_str())
            || v3(&r["e"]).map(bits3) != Some(bits3(end))
            || r["strict"].as_bool() != Some(strict)
            || !fields(r, &["path", "strict", "e"])
        {
            return Err(Desync(format!(
                "the request {key} to {end:?} (strict {strict}): the recording's is {}",
                describe(r)
            )));
        }
        self.ag_count_request(&r.clone());
        Ok(key)
    }
}

impl MoveInputs for Driver<'_> {
    fn can_move(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        boolean(&self.ag_leaf(w, u, "canmove")?, "v", "canmove")
    }
    fn can_rotate(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        boolean(&self.ag_leaf(w, u, "canrotate")?, "v", "canrotate")
    }
    fn interrupt_move(&mut self, _: &mut World, _: EntityIdx) -> R<()> {
        Err(Desync("TickUnit interrupts a move it can't make (UnitCommands.InterruptMove from the controller isn't wired)".into()))
    }
    fn place_on_ground(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        required: bool,
    ) -> R<Option<(V3, V3)>> {
        let id = self.ag_unit_id(w, u);
        let next = self.ag_peek();
        // With `ground_view` the step is the sim's, from its first ray.
        if self.ags.nav.ground_view {
            let ray = next.as_ref().is_some_and(|r| {
                r["ev"] == "ag_phys" && r["ctx"] == "ground" && r["u"].as_str() == Some(id.as_str())
            });
            if ray {
                return self.nav_ground(w, u, true).map(Some);
            }
            if required {
                return Err(Desync(
                    "TickUnit places a unit on the ground; the recording has no ray".into(),
                ));
            }
            if next.is_some_and(|r| r["ev"] == "ag_ground" && r["u"].as_str() == Some(id.as_str()))
            {
                return Err(Desync("a place-on-ground step without its rays".into()));
            }
            return Ok(None);
        }
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
        if r["ctl"] != true || !fields(&r, &["u", "ctl", "pos", "after"]) {
            return Err(Desync(format!(
                "a place-on-ground record it can't read ({})",
                describe(&r)
            )));
        }
        self.ag_input(if required { "ground" } else { "ground (prone or elevator)" });
        let pos = w.unit(u).map(|x| x.position).unwrap_or_default();
        if v3(&r["pos"]).map(bits3) != Some(bits3(pos)) {
            return Err(Desync("the place-on-ground step starts from another position".into()));
        }
        v3(&r["after"])
            .map(|a| Some((a, a)))
            .ok_or_else(|| Desync("ag_ground without its position after".into()))
    }
    fn desired_orientation(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        value: f32,
        desired: f32,
        max_angle: f32,
    ) -> R<f32> {
        let r = self.ag_take(w, u, "ag_rot", "UpdateSlowRotation")?;
        if f(&r["o"]).map(f32::to_bits) != Some(value.to_bits())
            || f(&r["max"]).map(f32::to_bits) != Some(max_angle.to_bits())
            || r["ctl"] != true
        {
            return Err(Desync(format!(
                "UpdateSlowRotation({value}, {max_angle}): the recording's is {}",
                describe(&r)
            )));
        }
        let recorded = f(&r["desired"])
            .ok_or_else(|| Desync("ag_rot without its desired orientation".into()))?;
        self.look_read(w, desired, recorded)
    }
    fn paused(&mut self, w: &mut World) -> R<bool> {
        Ok(w.combat.commands_paused)
    }
    fn saddled_unit(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(false)
    }
    fn has_agent(&mut self, _: &mut World, u: EntityIdx) -> R<bool> {
        Ok(self.ag_has(u))
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
        let at = self
            .ag_pull(w, &format!("a {kind} query"), |r| r["k"] == "obs" && r["ev"] == "ag_phys")?;
        let trace = self.trace;
        let r = &trace.records[at];
        let recorded: Option<Vec<f32>> = if kind == "ray" {
            v3(&r["d"]).map(|d| d.to_vec())
        } else {
            f(&r["r"]).map(|x| vec![x])
        };
        let same_shape = recorded
            .is_some_and(|x| x.iter().map(|v| v.to_bits()).eq(shape.iter().map(|v| v.to_bits())));
        if r["u"].as_str() != Some(id.as_str())
            || r["fn"] != kind
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
        // The sim's answer (`wr-phys`), the recorded one its check.
        self.nav_phys(w, r, kind, o, shape, max, mask, super::nav::PhysCtx::Agent)
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
            || (index.is_none() && !r["i"].is_null())
            || !fields(&r, &["fn", "u", "dest", "i"])
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

#[cfg(test)]
mod critter_phys_tests {
    use serde_json::{Value, json};

    /// [`super::critter_phys`] of a record written as a `serde_json::Value`.
    fn critter_phys(r: &Value) -> bool {
        super::critter_phys(&super::Rv::from(r))
    }

    /// [`super::other_phys`] of a record written as a `serde_json::Value`.
    fn other_phys(r: &Value) -> bool {
        super::other_phys(&super::Rv::from(r))
    }

    fn ray() -> Value {
        json!({"seq": 1, "k": "obs", "t": 5, "ev": "ag_phys", "fn": "ray", "ctx": "critter", "u": null,
            "critter": {"scene": "S", "path": "Critters/Tarantula", "sib": [0]},
            "o": [0.0, 2.0, 0.0], "d": [0.0, -1.0, 0.0], "max": 100.0, "mask": 2099457, "hit": true,
            "dist": 2.0, "pt": [0.0, 0.0, 0.0],
            "col": {"scene": "S", "path": "TERRAIN", "sib": [], "type": "TerrainCollider", "iid": 1672364}})
    }

    #[test]
    fn accepts_the_record_written() {
        assert!(critter_phys(&ray()));
        let mut miss = ray();
        let o = miss.as_object_mut().unwrap();
        o.insert("hit".into(), json!(false));
        for k in ["dist", "pt", "col"] {
            o.remove(k);
        }
        assert!(critter_phys(&miss));
    }

    #[test]
    fn rejects_damaged_identity_or_shape() {
        let damage: Vec<fn(&mut Value)> = vec![
            |r| {
                r.as_object_mut().unwrap().remove("u");
            },
            |r| r["u"] = json!("51FC"),
            |r| r["critter"]["path"] = json!(""),
            |r| {
                r["critter"].as_object_mut().unwrap().remove("scene");
            },
            |r| r["critter"]["sib"] = json!(["x"]),
            |r| r["critter"]["extra"] = json!(1),
            |r| r["ctx"] = json!("other"),
            |r| r["fn"] = json!("box"),
            |r| {
                r.as_object_mut().unwrap().remove("dist");
            },
            |r| r["d"] = json!(null),
            |r| r["d"] = json!([0.0, 1.0, 0.0]),
            |r| r["mask"] = json!(512),
            |r| r["max"] = json!(50.0),
            |r| r["o"] = json!([0.0, "x", 0.0]),
            |r| r["col"] = json!(null),
            |r| r["pt"] = json!([0.0, 0.0]),
            |r| r["dist"] = json!("2"),
            |r| r["col"] = json!({"type": null}),
            |r| r["col"] = json!({"arbitrary": true}),
            |r| r["col"]["type"] = json!(null),
            |r| r["col"]["iid"] = json!("1"),
            |r| {
                r["col"].as_object_mut().unwrap().remove("path");
            },
            |r| r["col"]["sib"] = json!(null),
        ];
        for (i, d) in damage.iter().enumerate() {
            let mut r = ray();
            d(&mut r);
            assert!(!critter_phys(&r), "damage {i} accepted");
        }
    }

    fn other() -> Value {
        json!({"seq": 1, "k": "obs", "t": 5, "ev": "ag_phys", "fn": "disc", "ctx": "other", "u": null,
            "origin": "UnitMovementAgentBase.Move < CritterMoveAgent.TickMovement",
            "o": [0.0, 2.0, 0.0], "r": 0.4, "max": 100.0, "mask": 2099457, "hit": true,
            "dist": 2.0, "pt": [0.0, 0.0, 0.0],
            "col": {"scene": "S", "path": "TERRAIN", "sib": [], "type": "TerrainCollider", "iid": 1672364}})
    }

    #[test]
    fn accepts_another_callers_record() {
        assert!(other_phys(&other()));
        assert!(!critter_phys(&other()));
        let mut miss = other();
        let o = miss.as_object_mut().unwrap();
        o.insert("hit".into(), json!(false));
        for k in ["dist", "pt", "col"] {
            o.remove(k);
        }
        assert!(other_phys(&miss));
        let mut ray = other();
        let o = ray.as_object_mut().unwrap();
        o.remove("r");
        o.insert("fn".into(), json!("ray"));
        o.insert("d".into(), json!([0.0, -1.0, 0.0]));
        assert!(other_phys(&ray));
    }

    #[test]
    fn rejects_another_callers_damaged_record() {
        let damage: Vec<fn(&mut Value)> = vec![
            |r| {
                r.as_object_mut().unwrap().remove("u");
            },
            |r| r["u"] = json!("51FC"),
            |r| {
                r.as_object_mut().unwrap().remove("origin");
            },
            |r| r["origin"] = json!(""),
            |r| r["origin"] = json!(null),
            |r| r["critter"] = json!({"scene": "S", "path": "Critters/Tarantula", "sib": [0]}),
            |r| r["ctx"] = json!("critter"),
            |r| r["ctx"] = json!("span"),
            |r| r["fn"] = json!("box"),
            |r| r["r"] = json!(null),
            |r| r["d"] = json!([0.0, -1.0, 0.0]),
            |r| r["mask"] = json!("x"),
            |r| r["max"] = json!(null),
            |r| r["o"] = json!([0.0, "x", 0.0]),
            |r| {
                r.as_object_mut().unwrap().remove("dist");
            },
            |r| r["hit"] = json!(false),
            |r| r["hit"] = json!(null),
            |r| r["col"] = json!(null),
            |r| r["col"]["iid"] = json!("1"),
            |r| r["extra"] = json!(1),
        ];
        for (i, d) in damage.iter().enumerate() {
            let mut r = other();
            d(&mut r);
            assert!(!other_phys(&r), "damage {i} accepted");
        }
    }
}
