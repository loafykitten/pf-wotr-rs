//! The driven replay's side of the roaming controller (`docs/poc5/orientation.md`, "Roaming"):
//! `UnitRoamingController` runs in its phase of every frame ([`Phase::Roaming`]) with the sim's
//! `UnitPartRoaming` states ([`crate::combat::roaming`]), and each thing it makes pulls the
//! record the game wrote and checks the sim's against it:
//!
//! - a roll (`RollNextPoint`): WrathOracle's `roam_roll` (the unit, its settings and their idle
//!   cutscenes), then its two draws, taken from the tape where the sim draws;
//! - a sleeping unit's step: its `ag_pos` (the position before and the sim's after);
//! - its `LookAt` toward the next point: its `ag_look` (observer `look`), through
//!   [`Driver::look_write`];
//! - an awake unit's `UnitMoveTo`: its `uc_new` (the point, approach and move fields) and its
//!   `uc_run` entry, which the commands model runs.
//!
//! A roaming record the sim's controller didn't pull (or one reached after its phase) is
//! reported, and taken as before. The states at the window's start are the save's (the spawner
//! gives the settings and origin, as at the view's attach). With observer `roam_init`, each is
//! checked against the game's at the window's first tick: what only the scene and the actions
//! set (the settings, the origin, `m_Disabled`, a follow part) must equal it, and what the
//! controller advances between the load and the window (the next point, the direction, the idle
//! end and cutscene) is taken from it. Without `roam_init`, a state is established only where the
//! recording shows the controller hasn't moved on since the load: the unit runs a roaming move to
//! its saved next point. Otherwise (and after a report) a unit's state is unknown, and the
//! controller reaching it is reported.
//!
//! On when the agent is driven and the recording declares the roaming producers' observer
//! (`cutscene_progress`: `roam_roll`).

use std::collections::{BTreeMap, HashSet};

use super::agent::{bits3, f, fields, v3};
use super::commands::{describe, id};
use super::{Driver, Phase, registered};
use crate::combat::commands::approach::Params;
use crate::combat::commands::move_to::Fields;
use crate::combat::commands::{Desync, LookSite, R, Target, class, ty};
use crate::combat::oracle::Rv;
use crate::combat::orientation::{LookArg, LookFn, V3};
use crate::combat::roaming::{self as roam, Point, RoamInputs, Settings};
use crate::csharp::TimeSpan;
use crate::world::{EntityIdx, World};

const TICK_ON_UNIT: &str = "Kingmaker.Controllers.Units.UnitRoamingController.TickOnUnit";

/// The replay's state for the roaming controller.
#[derive(Debug, Default)]
pub(super) struct RoamDriver {
    pub on: bool,
    /// The states at the window's start were established (the first roaming phase).
    started: bool,
    /// Units whose state isn't known (not established at the start, or after a report).
    unknown: HashSet<EntityIdx>,
    /// The area's navigation (`TraceAlongNavmesh`) when the paths aren't the sim's.
    nav: Option<Result<wr_nav::world::World, String>>,
    /// The roll's draws' numeric sites in the trace's table.
    sites: Vec<i64>,
    /// Observer `roam_init`'s block.
    init: Init,
}

/// Observer `roam_init`: declared; the states read so far by unit, in the block's order; the
/// count `roam_init_end` gave; the first thing wrong with the block.
#[derive(Debug, Default)]
struct Init {
    declared: bool,
    states: Vec<(String, InitState)>,
    end: Option<usize>,
    bad: Option<String>,
}

/// A `roam_init`: the unit's `UnitPartRoaming` at the window's first tick.
#[derive(Debug, Clone, PartialEq)]
struct InitState {
    disabled: bool,
    /// A `UnitPartFollowUnit` (`Disabled`'s other operand).
    follow: bool,
    /// `m_NextPoint`, or another type's name.
    next: Result<Option<Point>, String>,
    original: V3,
    reverse: bool,
    idle_cutscene: bool,
    idle_end: TimeSpan,
    settings: Option<Settings>,
}

/// What the replay computed of the roaming controller (the coverage line).
#[derive(Debug, Default, Clone)]
pub struct RoamCounts {
    /// `TickOnUnit`s the sim ran; of those, waiting at a point.
    pub ticks: usize,
    pub idle: usize,
    /// Rolls (`RollNextPoint`), sleeping steps, `LookAt`s, `UnitMoveTo` runs, arrivals.
    pub rolls: usize,
    pub steps: usize,
    pub looks: usize,
    pub moves: usize,
    pub arrivals: usize,
    /// The states the window's start established, and units left unknown.
    pub established: usize,
    pub unknown: usize,
    /// Of the established, with `roam_init`: equal to the save's, or advanced since the load
    /// (the next point, direction, idle end or cutscene taken from it).
    pub init_equal: usize,
    pub init_taken: usize,
}

/// A record the roaming controller makes (`TickOnUnit` is its caller): its `roam_roll`, a step's
/// `ag_pos`, a `LookAt`'s `ag_look`, a `Run`'s `uc_run`.
fn by_controller(r: &Rv) -> bool {
    if r["k"] != "obs" {
        return false;
    }
    let origin = r["origin"].as_str().unwrap_or("");
    let mut frames = origin.split(" < ");
    match r["ev"].as_str() {
        Some("roam_roll") => true,
        Some("ag_pos") => {
            frames.next() == Some("Kingmaker.EntitySystem.Entities.UnitEntityData.set_Position")
                && frames.next() == Some(TICK_ON_UNIT)
        }
        Some("ag_look") => frames.next() == Some(TICK_ON_UNIT),
        Some("uc_run") => run_by_controller(origin),
        _ => false,
    }
}

/// A `Run`'s caller chain: `UnitCommands.Run` (its overloads), then the roaming controller.
fn run_by_controller(origin: &str) -> bool {
    origin.split(" < ").find(|f| !f.starts_with("Kingmaker.UnitLogic.Commands.UnitCommands.Run"))
        == Some(TICK_ON_UNIT)
}

impl Driver<'_> {
    fn roam_unsupported(w: &mut World, what: impl std::fmt::Display) {
        w.coverage.unsupported(format!("driven replay: roaming: {what}"));
    }

    /// The window's start: on with the driven agent and the roaming producers' records;
    /// the settings and origin of each roaming unit from its spawner.
    pub(super) fn roam_start(&mut self, w: &mut World, declared: &[&str]) {
        let trace = self.trace;
        self.ags.roam.on = self.ag && declared.contains(&"cutscene_progress");
        self.ags.roam.init.declared = declared.contains(&"roam_init");
        if !self.ags.roam.on {
            if self.ags.roam.init.declared {
                // (Its states would go unused: a damaged begin record, or a capture whose agent
                // isn't driven.)
                Self::roam_unsupported(
                    w,
                    "roam_init without the roaming controller (the driven agent and cutscene_progress)",
                );
            }
            return;
        }
        self.ags.roam.sites = trace
            .sites
            .iter()
            .filter(|(_, v)| v.0 == roam::SITE_ROLL_CIRCLE.0 || v.0 == roam::SITE_ROLL_IDLE.0)
            .map(|(k, _)| *k)
            .collect();
        // (What the controller reads that the replay doesn't model shows in what it makes: a
        // cutscene controlling the unit, `Disable`, a disabled part, stealth or the weather's
        // slowdown would each leave out or change a roll, step or move the sim makes and checks
        // against its record.)
        for u in super::super::life::all_units(w) {
            if !w.entity(u).parts.iter().any(|p| p.clr == "UnitPartRoaming") {
                continue;
            }
            if w.unit(u).is_none_or(|x| x.combat.roaming.is_none()) {
                self.ags.roam.unknown.insert(u);
                continue;
            }
            match spawner_settings(w, u) {
                Ok((settings, original)) => {
                    let p = w.unit_mut(u).and_then(|x| x.combat.roaming.as_deref_mut());
                    let p = p.expect("a roaming state");
                    p.settings = settings;
                    if let Some(o) = original {
                        p.original = o;
                    }
                }
                Err(_) => {
                    self.ags.roam.unknown.insert(u);
                }
            }
        }
    }

    /// The first roaming phase: a unit's saved state is its state at the window's start where it
    /// runs a roaming move to its saved next point (the controller ran it after the load and
    /// hasn't reached the point since: arriving would have rolled another).
    fn roam_establish(&mut self, w: &mut World) {
        self.ags.roam.started = true;
        if self.ags.roam.init.declared {
            self.roam_establish_init(w);
            self.out.roam.unknown = self.ags.roam.unknown.len();
            return;
        }
        for u in super::super::life::all_units(w) {
            let Some(p) = w.unit(u).and_then(|x| x.combat.roaming.as_deref()) else { continue };
            if self.ags.roam.unknown.contains(&u) {
                continue;
            }
            let want = p.next.as_ref().and_then(Point::position);
            let id = &w.entity(u).unique_id;
            let c = &w.combat.commands;
            let running = c
                .set_of(id)
                .and_then(|s| s.slots[usize::from(ty::MOVE)])
                .and_then(|m| c.get(m))
                .filter(|m| m.class == class::MOVE_TO && m.mv.is_some_and(|f| f.roaming));
            let ok = match (running, want) {
                (Some(m), Some(p)) => {
                    matches!(m.s.target, Target::Point(t) if bits3(t) == bits3(p))
                }
                _ => false,
            };
            if ok {
                self.out.roam.established += 1;
            } else {
                self.ags.roam.unknown.insert(u);
            }
        }
        self.out.roam.unknown = self.ags.roam.unknown.len();
    }

    /// A `roam_init` or `roam_init_end` (the window's first tick, before the first roaming phase):
    /// read into the block, or the block marked bad and reported (every roaming unit then unknown).
    fn roam_init_record(&mut self, w: &mut World, r: &Rv) {
        let init = &mut self.ags.roam.init;
        if init.bad.is_some() {
            return;
        }
        let ev = r["ev"].as_str().unwrap_or("");
        // The window's first frame: its `tick` record reached (the window's first `t`).
        let first =
            self.trace.records.iter().find(|x| x["k"] == "tick").and_then(|x| x["t"].as_i64());
        let why = if !init.declared {
            Some(format!("a {ev} without its observer's declaration"))
        } else if !self.ags.roam.on {
            Some(format!("a {ev} without the roaming controller"))
        } else if self.frame.mode.is_none() || first != Some(self.tick) {
            Some(format!("a {ev} outside the window's first frame"))
        } else if self.ags.roam.started || r["t"].as_i64() != Some(self.tick) {
            Some(format!("a {ev} after the window's first roaming phase"))
        } else if init.end.is_some() {
            Some(format!("a {ev} after roam_init_end"))
        } else if ev == "roam_init_end" {
            match r["n"].as_i64().and_then(|n| usize::try_from(n).ok()) {
                Some(n) if fields(r, &["n"]) && n == init.states.len() => {
                    init.end = Some(n);
                    None
                }
                _ => Some(format!(
                    "roam_init_end {} after {} states",
                    describe(r),
                    init.states.len()
                )),
            }
        } else {
            match (r["u"].as_str(), init_state(r)) {
                (Some(u), Some(s)) if !init.states.iter().any(|(x, _)| x == u) => {
                    init.states.push((u.to_owned(), s));
                    None
                }
                _ => Some(format!("a roam_init it can't read: {}", describe(r))),
            }
        };
        if let Some(why) = &why {
            Self::roam_unsupported(w, format!("roam_init: {why}"));
        }
        init.bad = why;
    }

    /// The first roaming phase with `roam_init`: each roaming unit's state against the game's.
    /// The settings, origin, `m_Disabled` and follow part must equal it (only the scene and the
    /// actions set them); the next point, direction, idle end and idle cutscene, which the
    /// controller advances between the load and the window, are taken from it.
    fn roam_establish_init(&mut self, w: &mut World) {
        let init = std::mem::take(&mut self.ags.roam.init);
        let units: Vec<EntityIdx> = super::super::life::all_units(w)
            .into_iter()
            .filter(|&u| w.entity(u).parts.iter().any(|p| p.clr == "UnitPartRoaming"))
            .collect();
        if init.bad.is_some() || init.end.is_none() {
            // (A bad block was reported where it went wrong.)
            if init.bad.is_none() {
                Self::roam_unsupported(w, "roam_init: no roam_init_end at the window's first tick");
            }
            self.ags.roam.unknown.extend(units);
            self.ags.roam.init = init;
            return;
        }
        let mut states: BTreeMap<&str, &InitState> = BTreeMap::new();
        for (id, s) in &init.states {
            states.insert(id.as_str(), s);
        }
        // `StateDump.AllUnits` orders by `UniqueId` (ordinal).
        if !init.states.windows(2).all(|p| p[0].0.as_bytes() < p[1].0.as_bytes()) {
            Self::roam_unsupported(w, "roam_init: the states aren't in UniqueId order");
        }
        for &u in &units {
            let id = w.entity(u).unique_id.clone();
            let Some(s) = states.remove(id.as_str()) else {
                Self::roam_unsupported(w, format!("roam_init: no state of {id}, a roaming unit"));
                self.ags.roam.unknown.insert(u);
                continue;
            };
            if self.ags.roam.unknown.contains(&u) {
                // (No saved state, or the spawner's settings unknown: the record can't be checked,
                // whether or not the controller reaches the unit.)
                Self::roam_unsupported(
                    w,
                    format!("roam_init: {id}'s state, which the sim can't establish to check it"),
                );
                continue;
            }
            let follow = w.entity(u).parts.iter().any(|p| p.clr == "UnitPartFollowUnit");
            let p = w.unit_mut(u).and_then(|x| x.combat.roaming.as_deref_mut());
            let p = p.expect("a roaming state");
            let fixed = p.disabled == s.disabled
                && follow == s.follow
                && bits3(p.original) == bits3(s.original)
                && settings_bits(p.settings) == settings_bits(s.settings);
            if !fixed {
                Self::roam_unsupported(
                    w,
                    format!(
                        "roam_init: {id}'s settings, origin, m_Disabled or follow part differ from the sim's"
                    ),
                );
                self.ags.roam.unknown.insert(u);
                continue;
            }
            let saved = (p.next.clone(), p.unknown_next.clone());
            let (next, unknown_next) = match &s.next {
                Ok(n) => (n.clone(), None),
                Err(t) => (None, Some(t.clone())),
            };
            let equal = point_bits(saved.0.as_ref()) == point_bits(next.as_ref())
                && saved.1 == unknown_next
                && p.reverse == s.reverse
                && p.idle_cutscene == s.idle_cutscene
                && p.idle_end == s.idle_end;
            p.next = next;
            p.unknown_next = unknown_next;
            p.reverse = s.reverse;
            p.idle_cutscene = s.idle_cutscene;
            p.idle_end = s.idle_end;
            self.out.roam.established += 1;
            if equal {
                self.out.roam.init_equal += 1;
            } else {
                self.out.roam.init_taken += 1;
            }
        }
        for id in states.keys() {
            Self::roam_unsupported(
                w,
                format!("roam_init: a state of {id}, not a roaming unit of the sim's"),
            );
        }
        self.ags.roam.init = init;
    }

    /// `UnitRoamingController.Tick` (`TickSleeping`): `TickOnUnit` on every unit of the pool that
    /// `ShouldTickOnUnit` takes, in order.
    pub(super) fn roam_tick(&mut self, w: &mut World) {
        if !self.ags.roam.on {
            return;
        }
        if !self.ags.roam.started {
            self.roam_establish(w);
        }
        let dialog = self.draining.or(self.frame.mode) == Some(super::mode::DIALOG);
        let (game_time, delta) = (w.clock.game_time, w.clock.delta_time());
        for u in crate::scene::units_pool(w) {
            if !w.entity(u).parts.iter().any(|p| p.clr == "UnitPartRoaming")
                || !crate::scene::enumerated_by_pool(w, u)
            {
                continue;
            }
            let id = w.entity(u).unique_id.clone();
            match roam::should_tick_on_unit(w, u, dialog) {
                Ok(true) => {}
                Ok(false) => continue,
                Err(e) => {
                    Self::roam_unsupported(w, format!("ShouldTickOnUnit of {id}: {e}"));
                    continue;
                }
            }
            if self.ags.roam.unknown.contains(&u) {
                Self::roam_unsupported(
                    w,
                    format!(
                        "{id}'s state isn't known (the save's unconfirmed at the window's start: no roam_init; or after a report)"
                    ),
                );
                continue;
            }
            self.out.roam.ticks += 1;
            let mut io = Io { d: self, draws: None };
            if let Err(Desync(e)) = roam::tick_on_unit(w, &mut io, u, game_time, delta) {
                Self::roam_unsupported(w, format!("{id}: {e}"));
                // (Its state isn't the game's from here: its records are taken.)
                self.ags.roam.unknown.insert(u);
            }
        }
    }

    /// A roaming record the record loop reached: placed in the roaming phase (which pulls it)
    /// when the frame hasn't run it yet; otherwise the sim's controller didn't make it. `None`:
    /// not a roaming record (or roaming isn't on).
    pub(super) fn roam_record(&mut self, w: &mut World, i: usize) -> Option<usize> {
        let trace = self.trace;
        let recs = &trace.records;
        let r = &recs[i];
        // (Read whether the controller is on or not: without it, the block is reported.)
        if r["k"] == "obs" && matches!(r["ev"].as_str(), Some("roam_init" | "roam_init_end")) {
            self.roam_init_record(w, r);
            return Some(i + 1);
        }
        if !self.ags.roam.on {
            return None;
        }
        let draw = r["k"] == "draw" && self.roam_draw(r);
        // A `Run`'s `uc_new` comes right before its `uc_run`.
        let new = r["k"] == "obs"
            && r["ev"] == "uc_new"
            && recs.get(i + 1).is_some_and(|n| n["ev"] == "uc_run" && by_controller(n));
        if !(draw || new || by_controller(r)) {
            return None;
        }
        let before = self.frame.mode.is_some_and(|m| {
            registered(m, Phase::Roaming) == Some(true) && self.frame.at < Phase::Roaming
        });
        if before {
            self.i = i;
            self.place(w, Phase::Roaming, "the roaming controller");
            if self.i != i {
                return Some(self.i);
            }
        }
        // Not the sim's: reported, and taken as before (the caller processes it).
        let unit = r["u"].as_str().or(r["unit"].as_str()).unwrap_or("?");
        Self::roam_unsupported(
            w,
            format!(
                "{} of {unit} by the roaming controller, which the sim's didn't make",
                describe(r)
            ),
        );
        if draw {
            let _ = if self.trace.site_key(r).as_deref() == Some(roam::SITE_ROLL_CIRCLE.0) {
                let _ = w.rng.inside_unit_circle(roam::SITE_ROLL_CIRCLE);
                0.0
            } else {
                let (lo, hi) = range(r);
                w.rng.range_float(roam::SITE_ROLL_IDLE, lo, hi)
            };
            return Some(i + 1);
        }
        None
    }

    /// A draw of `RollNextPoint`'s (by its numeric site or its key).
    fn roam_draw(&self, r: &Rv) -> bool {
        r["site"].as_i64().is_some_and(|s| self.ags.roam.sites.contains(&s))
            || self
                .trace
                .site_key(r)
                .is_some_and(|k| k == roam::SITE_ROLL_CIRCLE.0 || k == roam::SITE_ROLL_IDLE.0)
    }

    /// The record at the cursor, which must be `want`'s (of this frame): taken.
    fn roam_take(&mut self, what: &str, want: impl Fn(&Rv) -> bool) -> R<usize> {
        let r = self.trace.records.get(self.i);
        match r {
            Some(r) if want(r) && r["t"].as_i64() == Some(self.tick) => {
                self.i += 1;
                Ok(self.i - 1)
            }
            Some(r) => Err(Desync(format!("{what}: the recording has {}", describe(r)))),
            None => Err(Desync(format!("{what}: the recording ends"))),
        }
    }
}

/// The object has exactly `keys`.
fn keys_exactly(v: &Rv, keys: &[&str]) -> bool {
    v.as_object().is_some_and(|o| o.len() == keys.len() && keys.iter().all(|k| o.contains_key(*k)))
}

fn finite(v: &Rv) -> Option<f32> {
    f(v).filter(|x| x.is_finite())
}

fn finite3(v: &Rv) -> Option<V3> {
    v3(v).filter(|x| x.iter().all(|c| c.is_finite()))
}

/// A `roam_init` record exactly as WrathOracle writes it (`CommandObsRoam.cs`), or `None`.
/// `prev` (`PreviousPosition`: the waypoints' `Advance` reads it, not the modelled controller) and
/// the cutscenes' guids are checked for their form only.
fn init_state(r: &Rv) -> Option<InitState> {
    let keys = [
        "u", "disabled", "follow", "next", "next_id", "orig", "prev", "reverse", "idle_cs",
        "idle_end", "settings",
    ];
    if !fields(r, &keys) {
        return None;
    }
    let guid = |v: &Rv| v.is_null() || v.as_str().is_some();
    let next_id = r["next_id"].as_str()?;
    let n = &r["next"];
    // The setter keeps `m_PersistentNextPointId` with the point: a waypoint's id, else empty (a
    // saved waypoint `OnPostLoad` didn't find leaves the point null and the id set).
    let next = if n.is_null() {
        Ok(None)
    } else {
        match n["kind"].as_str()? {
            "RoamingPoint" => {
                if !keys_exactly(n, &["kind", "pos", "idle", "cs"])
                    || !guid(&n["cs"])
                    || !next_id.is_empty()
                {
                    return None;
                }
                Ok(Some(Point::Own {
                    position: finite3(&n["pos"])?,
                    idle_time: finite(&n["idle"])?,
                    idle_cutscene: !n["cs"].is_null(),
                }))
            }
            "RoamingWaypointData" => {
                let id = n["id"].as_str()?;
                if !keys_exactly(n, &["kind", "id"]) || next_id != id {
                    return None;
                }
                Ok(Some(Point::Waypoint(id.to_owned())))
            }
            t => {
                if !keys_exactly(n, &["kind"]) {
                    return None;
                }
                Err(t.to_owned())
            }
        }
    };
    let cs = &r["idle_cs"];
    if !(cs.is_null()
        || keys_exactly(cs, &["id", "cs"]) && cs["id"].as_str().is_some() && guid(&cs["cs"]))
    {
        return None;
    }
    finite3(&r["prev"])?;
    let s = &r["settings"];
    let settings = if s.is_null() {
        None
    } else {
        let keys = ["radius", "min_idle", "max_idle", "move", "sleepless", "idle"];
        let idle = s["idle"].as_array()?;
        if !keys_exactly(s, &keys) || !idle.iter().all(guid) {
            return None;
        }
        Some(Settings {
            radius: finite(&s["radius"])?,
            min_idle: finite(&s["min_idle"])?,
            max_idle: finite(&s["max_idle"])?,
            idle_cutscenes: idle.len(),
            movement_type: s["move"].as_i64()?,
            sleepless: s["sleepless"].as_bool()?,
        })
    };
    Some(InitState {
        disabled: r["disabled"].as_bool()?,
        follow: r["follow"].as_bool()?,
        next,
        original: finite3(&r["orig"])?,
        reverse: r["reverse"].as_bool()?,
        idle_cutscene: !cs.is_null(),
        idle_end: TimeSpan(r["idle_end"].as_i64()?),
        settings,
    })
}

/// A point by its floats' bits: an own point's position, idle time and cutscene, or a
/// waypoint's id.
type PointBits<'a> = Result<([u32; 3], u32, bool), &'a str>;

fn point_bits(p: Option<&Point>) -> Option<PointBits<'_>> {
    p.map(|p| match p {
        Point::Own { position, idle_time, idle_cutscene } => {
            Ok((bits3(*position), idle_time.to_bits(), *idle_cutscene))
        }
        Point::Waypoint(id) => Err(id.as_str()),
    })
}

/// Settings, their floats by their bits.
fn settings_bits(s: Option<Settings>) -> Option<([u32; 3], usize, i64, bool)> {
    s.map(|s| {
        let f = [s.radius, s.min_idle, s.max_idle].map(f32::to_bits);
        (f, s.idle_cutscenes, s.movement_type, s.sleepless)
    })
}

/// A `Range_ff` draw's arguments.
fn range(r: &Rv) -> (f32, f32) {
    let a = r["a"].as_array();
    let g = |i: usize| a.and_then(|a| a.get(i)).and_then(f).unwrap_or(0.0);
    (g(0), g(1))
}

/// The spawner's `SpawnerRoamingSettings.Part.OnInitialize` (when its view attaches, at the
/// load): `Settings` (a new one: `Radius` 1, idle times 0, no idle cutscenes; the movement type;
/// `Radius` mode's radius and idle times, `Cutscene` mode's cutscenes and idle times) and
/// `OriginalPoint` (the spawner's position); `Sleepless` too (`SleepingUnitsController` reads
/// the unit's own retained flag, not this copy). `Ok((None, None))`: no spawner of the unit has
/// roaming settings (the saved state stands). `Err`: the spawner's view isn't known.
fn spawner_settings(w: &World, u: EntityIdx) -> Result<(Option<Settings>, Option<V3>), String> {
    let id = &w.entity(u).unique_id;
    let spawners: BTreeMap<&String, _> = w
        .player
        .spawners
        .iter()
        .filter(|(_, s)| s.spawned_unit.as_deref() == Some(id.as_str()))
        .collect();
    let [(sid, _)] = spawners.into_iter().collect::<Vec<_>>()[..] else {
        return Err("the unit's spawner isn't known".into());
    };
    let Some(view) = w.scene.cache.as_ref().and_then(|c| c.view(sid)) else {
        return Err("the spawner's view isn't in the scene cache".into());
    };
    let Some(c) = view.components.iter().find(|c| c.class.ends_with(".SpawnerRoamingSettings"))
    else {
        return Ok((None, None));
    };
    let fl = |k: &str| -> Result<f32, String> {
        #[allow(clippy::cast_possible_truncation)]
        c.fields[k].as_f64().map(|x| x as f32).ok_or_else(|| format!("{k} missing"))
    };
    let int = |k: &str| c.fields[k].as_i64().ok_or_else(|| format!("{k} missing"));
    let mode = int("Mode")?;
    let mut s = Settings {
        radius: 1.0,
        min_idle: 0.0,
        max_idle: 0.0,
        idle_cutscenes: 0,
        movement_type: int("MovementType")?,
        sleepless: int("Sleepless")? != 0,
    };
    // `ModeType`: Patrol, Radius, Cutscene.
    if mode == 1 {
        s.radius = fl("Radius")?;
        s.min_idle = fl("MinIdleTime")?;
        s.max_idle = fl("MaxIdleTime")?;
    }
    if mode == 2 {
        s.idle_cutscenes =
            c.fields["IdleCutscenes"].as_array().ok_or("IdleCutscenes missing")?.len();
        s.min_idle = fl("MinIdleTime")?;
        s.max_idle = fl("MaxIdleTime")?;
    }
    Ok((Some(s), Some(view.position)))
}

/// The controller's inputs: the records it pulls.
struct Io<'a, 't> {
    d: &'a mut Driver<'t>,
    /// The roll's draw records, pulled with its `roam_roll`, checked once drawn.
    draws: Option<[usize; 2]>,
}

impl RoamInputs for Io<'_, '_> {
    fn roll(&mut self, w: &mut World, u: EntityIdx, settings: Option<Settings>) -> R<()> {
        let id = w.entity(u).unique_id.clone();
        let at = self.d.roam_take("RollNextPoint's roam_roll", |r| {
            r["k"] == "obs" && r["ev"] == "roam_roll"
        })?;
        let r = &self.d.trace.records[at];
        let idle = r["idle"].as_array().map(<[Rv]>::len);
        let ok = fields(r, &["unit", "settings", "idle"])
            && r["unit"].as_str() == Some(id.as_str())
            && r["settings"].as_bool() == Some(settings.is_some())
            && idle == Some(settings.map_or(0, |s| s.idle_cutscenes));
        if !ok {
            return Err(Desync(format!("RollNextPoint of {id}: the recording has {r:?}")));
        }
        // Its two draws follow (the circle, then the idle time), each the sim's from the tape.
        let trace = self.d.trace;
        let mut at = [0; 2];
        for (n, site) in [roam::SITE_ROLL_CIRCLE, roam::SITE_ROLL_IDLE].into_iter().enumerate() {
            at[n] = self.d.roam_take(&format!("RollNextPoint's draw at {}", site.0), |r| {
                r["k"] == "draw" && r["c"] == "sim" && trace.site_key(r).as_deref() == Some(site.0)
            })?;
        }
        self.draws = Some(at);
        Ok(())
    }

    fn drew(&mut self, circle: [f32; 2], idle: f32, min: f32, max: f32) -> R<()> {
        let Some([c, i]) = self.draws.take() else {
            return Err(Desync("RollNextPoint's draws without their records".into()));
        };
        let recs = &self.d.trace.records;
        let (rc, ri) = (&recs[c], &recs[i]);
        let vec2 = |v: &Rv| -> Option<[u32; 2]> {
            let a = v.as_array().filter(|a| a.len() == 2)?;
            Some([f(&a[0])?.to_bits(), f(&a[1])?.to_bits()])
        };
        let args = |v: &Rv| -> Option<Vec<u32>> {
            v.as_array()?.iter().map(|x| f(x).map(f32::to_bits)).collect()
        };
        // (`Random.Range(float, float)` returns within its inclusive bounds; the tape doesn't
        // check them.)
        let (lo, hi) = if min <= max { (min, max) } else { (max, min) };
        let ok = (lo..=hi).contains(&idle)
            && rc["m"] == "insideUnitCircle"
            && args(&rc["a"]).is_some_and(|a| a.is_empty())
            && vec2(&rc["v"]) == Some(circle.map(f32::to_bits))
            && ri["m"] == "Range_ff"
            && args(&ri["a"]) == Some(vec![min.to_bits(), max.to_bits()])
            && f(&ri["v"]).map(f32::to_bits) == Some(idle.to_bits());
        if !ok {
            return Err(Desync(format!(
                "RollNextPoint's draws: the sim's insideUnitCircle {circle:?} and Range({min}, {max}) {idle}, the recording's {} and {}",
                describe(rc),
                describe(ri)
            )));
        }
        self.d.out.roam.rolls += 1;
        Ok(())
    }

    fn trace_along_navmesh(&mut self, w: &mut World, start: V3, end: V3) -> R<V3> {
        if w.combat.nav.on
            && !w.combat.nav.broken
            && let Some(n) = &w.combat.nav.nav
        {
            return Ok(n.world.trace_along_navmesh(start, end));
        }
        let nav = self.d.ags.roam.nav.get_or_insert_with(|| match w.player.area {
            None => Err("the save names no current area (its nav file)".to_owned()),
            Some(g) => wr_nav::NavFile::read(&wr_nav::path_for(&g.to_string()))
                .map(|f| f.world)
                .map_err(|e| format!("the area's nav file: {e:#}")),
        });
        match nav {
            Ok(n) => Ok(n.trace_along_navmesh(start, end)),
            Err(e) => Err(Desync(format!("TraceAlongNavmesh: {e}"))),
        }
    }

    fn sleeping(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        if w.combat.awake_unknown.contains(&u) {
            return Err(Desync("IsSleeping: the unit's awake state isn't known".into()));
        }
        Ok(!w.combat.awake.contains(&u))
    }

    fn step(&mut self, w: &mut World, u: EntityIdx, value: Result<V3, String>) -> R<()> {
        let id = w.entity(u).unique_id.clone();
        // The setter keeps a position its value equals (`m_Position != value`): no write, and
        // WrathOracle writes no `ag_pos`.
        let before = w.unit(u).map_or([0.0; 3], |x| x.position);
        if let Ok(x) = value
            && crate::combat::orientation::vec3_eq(before, x)
        {
            let r = self.d.trace.records.get(self.d.i);
            // (Another unit's step may come next: only this unit's, in this frame, is wrong.)
            if r.is_some_and(|r| {
                r["k"] == "obs"
                    && r["ev"] == "ag_pos"
                    && by_controller(r)
                    && r["u"].as_str() == Some(id.as_str())
                    && r["t"].as_i64() == Some(self.d.tick)
            }) {
                return Err(Desync(format!(
                    "the sleeping step of {id}: a position written where the setter keeps it"
                )));
            }
            self.d.out.roam.steps += 1;
            return Ok(());
        }
        let at = self.d.roam_take("the sleeping step's ag_pos", |r| {
            r["k"] == "obs" && r["ev"] == "ag_pos" && by_controller(r)
        })?;
        let r = &self.d.trace.records[at];
        let (Some(pos), Some(v), true) =
            (v3(&r["pos"]), v3(&r["v"]), fields(r, &["u", "pos", "v", "origin"]))
        else {
            return Err(Desync(format!("the sleeping step of {id}: a record it can't read")));
        };
        if r["u"].as_str() != Some(id.as_str()) {
            return Err(Desync(format!(
                "the sleeping step of {id}: the recording's is {}'s",
                r["u"].as_str().unwrap_or("?")
            )));
        }
        // The recorded position carries on.
        let after = roam::stepped(before, v);
        if let Some(x) = w.unit_mut(u) {
            x.position = after;
        }
        crate::combat::vision::note_position_write(w, u);
        if bits3(pos) != bits3(before) {
            return Err(Desync(format!(
                "the sleeping step of {id}: the sim's position {before:?}, the recording's {}",
                describe(r)
            )));
        }
        match value {
            Ok(x) if bits3(x) == bits3(v) => {
                self.d.out.roam.steps += 1;
                Ok(())
            }
            Ok(x) => Err(Desync(format!(
                "the sleeping step of {id}: the sim's {x:?}, the recording's {v:?}"
            ))),
            Err(e) => Err(Desync(format!("the sleeping step of {id}: {e}"))),
        }
    }

    fn look_at(&mut self, w: &mut World, u: EntityIdx, point: V3) -> R<()> {
        // (With `look`, its record comes next: nothing in between.)
        if self.d.ags.look.on {
            let r = self.d.trace.records.get(self.d.i);
            if !r.is_some_and(|r| r["k"] == "obs" && r["ev"] == "ag_look") {
                return Err(Desync(format!(
                    "LookAt: the recording has {}",
                    r.map_or("nothing".into(), describe)
                )));
            }
        }
        self.d.look_write(
            w,
            u,
            LookSite::Roaming,
            LookFn::LookAt,
            Some(LookArg::Point(point)),
            false,
        )?;
        self.d.out.roam.looks += 1;
        Ok(())
    }

    fn run_move(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        point: V3,
        orientation: Option<f32>,
        movement_type: i64,
    ) -> R<()> {
        let uid = w.entity(u).unique_id.clone();
        let at = self
            .d
            .roam_take("the roaming move's uc_new", |r| r["k"] == "obs" && r["ev"] == "uc_new")?;
        let next = self.d.uc_record(w, at);
        self.d.i = next;
        let c = id(&self.d.trace.records[at]["c"]);
        let m = c.and_then(|c| w.combat.commands.get(c));
        // `new UnitMoveTo(point, 1f, distanceXZ: true)`: `MaxApproachRadius` 1000000, no line of
        // sight; `Roaming`, the point's orientation, the rest the constructor's.
        let ap = Params { radius: 1.0, max_radius: 1_000_000.0, xz: true, los: false };
        let mv = Fields {
            roaming: true,
            prevent_interrupt: false,
            rider_target: false,
            orientation,
            await_override: None,
            movement_delay: 0.0,
        };
        // A fresh `UnitMoveTo` (`CommandType.Move`, `IgnoreCooldown()`: no time limit), not a
        // subclass, nobody's yet, not started.
        let ok = m.is_some_and(|m| {
            m.class == class::MOVE_TO
                && m.bases.is_empty()
                && matches!(m.s.target, Target::Point(t) if bits3(t) == bits3(point))
                && m.ap == Some(ap)
                && m.mv == Some(mv)
                && m.ua.is_none()
                && m.aoo.is_none()
                && m.s.ty == ty::MOVE
                && m.ignore_cooldown
                && m.ignore_before.is_none()
                && !(m.ai || m.reaction || m.cutscene || m.dont_interrupt_tb || m.player)
                && !(m.rider || m.mount)
                && m.s.executor.is_none()
                && !(m.s.started || m.s.acted || m.s.finished || m.s.did_run)
                && m.s.result == 0
                && m.s.time_since_start.to_bits() == 0
        });
        if !ok {
            return Err(Desync(format!(
                "the roaming move of {uid}: the sim's UnitMoveTo to {point:?}, the recording's {}",
                describe(&self.d.trace.records[at])
            )));
        }
        // (`MovementType` isn't in the description: the agent's speed reads are its leaves.)
        let _ = movement_type;
        let run = self.d.roam_take("the roaming move's uc_run", |r| {
            r["k"] == "obs" && r["ev"] == "uc_run" && r["nested"] == false && by_controller(r)
        })?;
        let r = &self.d.trace.records[run];
        // `unit.Commands.Run(cmd)`: not from the queue, the queue cleared.
        if r["u"].as_str() != Some(uid.as_str())
            || id(&r["c"]) != c
            || r["fq"] != false
            || r["ncq"] != false
        {
            return Err(Desync(format!(
                "the roaming move of {uid}: the recording runs {}",
                describe(r)
            )));
        }
        self.d.i = self.d.uc_run_entry(w, run);
        self.d.out.roam.moves += 1;
        Ok(())
    }

    fn idle(&mut self) {
        self.d.out.roam.idle += 1;
    }

    fn arrived(&mut self) {
        self.d.out.roam.arrivals += 1;
    }
}
