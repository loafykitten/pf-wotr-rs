//! The driven replay's side of units' orientation writes (`docs/poc5/orientation.md`):
//! WrathOracle's observer `look` records every call of `UnitEntityData.LookAt`, `ForceLookAt` and
//! the `Orientation` and `DesiredOrientation` setters outside `UnitMoveController.TickUnit`'s own
//! write (`ag_look`: the call, its argument, the unit's position, rider flag and orientation
//! before and after, the caller chain). With it (and the driven agent), the sim's
//! `DesiredOrientation` is its own: `UpdateSlowRotation`'s reads (`ag_rot`'s `desired`) are checks.
//!
//! - A write the sim's code makes (the commands' `TurnToTarget`s, the attack of opportunity's
//!   `OnRun`, `UnitMoveTo.OnAction`: [`LookSite`]) pulls its record: the call, the unit, the
//!   caller chain's innermost frames, the argument (the sim's point, or the record's where the
//!   sim doesn't know it: counted), the position and the orientation before, and the write's
//!   result, computed by [`Orientation::write`], are checked.
//! - A write by code the sim doesn't run (an `ag_look` the sim's code didn't pull) is an input
//!   counted by its call and caller: its position and orientation before must be the sim's, and
//!   its result is computed from the recorded argument (a point's angle is the sim's) and checked.
//!   One whose caller is a site the sim runs is reported (the sim didn't make it), as is a rider's
//!   call (its mount's write isn't recorded apart) and the rider's copy in `TickUnit`.
//! - Real time (the walk-up, before the agents' synchronization) isn't ported: its writes are taken.

use std::collections::BTreeMap;

use crate::combat::oracle::Rv as J;

use super::Driver;
use super::agent::{bits3, caller, f, fields, v3};
use super::commands::{boundary, describe};
use crate::combat::commands::{Desync, LookSite, R};
use crate::combat::orientation::{LookArg, LookFn, Orientation, V3};
use crate::world::{EntityIdx, World};

/// The replay's state for the observer.
#[derive(Debug, Default)]
pub(super) struct LookDriver {
    /// The recording declares `look` and the agent is driven: the sim's writes are checked and
    /// its `DesiredOrientation` reads are its own.
    pub on: bool,
    /// The `ag_look` records the replay processed (pulled by the sim's code or taken as
    /// outside writes): every one of the recording's must be.
    pub processed: usize,
}

/// What the replay computed of units' orientation writes (the coverage line).
#[derive(Debug, Default, Clone)]
pub struct LookCounts {
    /// Writes the sim's code made, checked against their records, by site.
    pub computed: BTreeMap<&'static str, usize>,
    /// Of those, points the sim doesn't know (the record's: a class's own `GetTargetPoint`).
    pub points_taken: usize,
    /// The attack of opportunity's `LookAt`: made (its animation handle, presentation, was made)
    /// and not.
    pub aoo_made: usize,
    pub aoo_not_made: usize,
    /// Writes by code the sim doesn't run, by call and caller (their argument an input); of
    /// those, angles computed from a recorded point.
    pub outside: BTreeMap<String, usize>,
    pub outside_angles: usize,
    /// Writes in real time, or while the commands skip a desynced entry (taken).
    pub real_time: usize,
    /// `UpdateSlowRotation`'s `DesiredOrientation` reads: checked (the sim's), taken (without
    /// `look`).
    pub reads_checked: usize,
    pub reads_taken: usize,
    /// Synchronizations (`ag_sync`, where real time ends) that changed a unit's desired
    /// orientation (the initial states, `ag_init`, give every unit's).
    pub snapshots_taken: usize,
}

/// An `ag_look` record.
struct Rec {
    f: LookFn,
    u: String,
    arg: LookArg,
    pos: V3,
    rider: bool,
    before: Orientation,
    after: Orientation,
    origin: String,
}

const KEYS: [&str; 11] = ["fn", "u", "p", "v", "pos", "rider", "o0", "d0", "o", "d", "origin"];

fn parse(r: &J) -> Option<Rec> {
    if !fields(r, &KEYS) {
        return None;
    }
    let f_ = LookFn::parse(r["fn"].as_str()?)?;
    let arg = if f_.takes_point() {
        r["v"].is_null().then_some(())?;
        LookArg::Point(v3(&r["p"])?)
    } else {
        r["p"].is_null().then_some(())?;
        LookArg::Value(f(&r["v"])?)
    };
    Some(Rec {
        f: f_,
        u: r["u"].as_str()?.to_owned(),
        arg,
        pos: v3(&r["pos"])?,
        rider: r["rider"].as_bool()?,
        before: Orientation { value: f(&r["o0"])?, desired: f(&r["d0"])? },
        after: Orientation { value: f(&r["o"])?, desired: f(&r["d"])? },
        // (WrathOracle's `Origin()` writes "?" when it can't walk the stack: no caller known.)
        origin: r["origin"].as_str().filter(|o| !o.is_empty() && *o != "?")?.to_owned(),
    })
}

/// What a record it can't read still says of its unit's orientation after (its `u`, `o`, `d`),
/// so a reported record's write carries on.
fn readable_after(r: &J) -> Option<(String, Orientation)> {
    Some((r["u"].as_str()?.to_owned(), Orientation { value: f(&r["o"])?, desired: f(&r["d"])? }))
}

fn take_readable(w: &mut World, r: &J) {
    if let Some((id, after)) = readable_after(r)
        && let Some(unit) = w.by_id.get(&id).copied().and_then(|u| w.unit_mut(u))
    {
        unit.orientation = after.value;
        unit.combat.desired_orientation = after.desired;
    }
}

fn same_o(a: Orientation, b: Orientation) -> bool {
    a.value.to_bits() == b.value.to_bits() && a.desired.to_bits() == b.desired.to_bits()
}

fn same_arg(a: LookArg, b: LookArg) -> bool {
    match (a, b) {
        (LookArg::Point(p), LookArg::Point(q)) => bits3(p) == bits3(q),
        (LookArg::Value(x), LookArg::Value(y)) => x.to_bits() == y.to_bits(),
        _ => false,
    }
}

/// The caller chain's innermost frames are `site`'s.
fn from_site(origin: &str, site: LookSite) -> bool {
    let frames: Vec<&str> = origin.split(" < ").collect();
    let want = site.frames();
    frames.len() >= want.len() && frames.iter().zip(want).all(|(a, b)| a == b)
}

/// The site the sim runs whose write this is, by its caller chain's innermost frame.
fn sim_site(origin: &str) -> Option<&'static str> {
    let first = origin.split(" < ").next().unwrap_or("");
    match first {
        "Kingmaker.UnitLogic.Commands.Base.UnitCommand.TurnToTarget" => Some("TurnToTarget"),
        "Kingmaker.UnitLogic.Commands.UnitAttackOfOpportunity.OnRun" => {
            Some("UnitAttackOfOpportunity.OnRun")
        }
        "Kingmaker.UnitLogic.Commands.UnitMoveTo.OnAction" => Some("UnitMoveTo.OnAction"),
        // The merge into a parked command (paused: the sim reports it).
        "Kingmaker.UnitLogic.Commands.UnitCommands.Run" => Some("UnitCommands.Run"),
        // The rider's copy (mounts aren't modelled).
        "Kingmaker.Controllers.Units.UnitMoveController.TickUnit" => {
            Some("UnitMoveController.TickUnit")
        }
        _ => None,
    }
}

impl Driver<'_> {
    fn look_unsupported(w: &mut World, what: impl std::fmt::Display) {
        w.coverage.unsupported(format!("driven replay: look: {what}"));
    }

    /// The window's start: the writes are checked when the recording declares `look` and the
    /// agent is driven.
    pub(super) fn look_start(&mut self, w: &mut World) {
        let trace = self.trace;
        let declared = trace
            .records
            .iter()
            .find(|r| r["k"] == "begin")
            .and_then(|b| b["observers"].as_array())
            .is_some_and(|a| a.iter().any(|o| o == "look"));
        let records = trace.records.iter().any(|r| r["k"] == "obs" && r["ev"] == "ag_look");
        if records && !declared {
            Self::look_unsupported(w, "ag_look records without the observer's declaration");
        }
        if declared && !self.ag {
            Self::look_unsupported(
                w,
                "observer look without the driven agent (agent_sync and its chain)",
            );
        }
        self.ags.look.on = declared && self.ag;
    }

    /// The agents are the sim's here (turn-based, synchronized, the commands not skipping a
    /// desynced entry): a write's state before is the sim's.
    fn look_driven(&self, w: &World) -> bool {
        self.ags.look.on && w.combat.turn_based && self.ags.synced && !self.uc_skipping
    }

    /// An `ag_look` comes before the commands' next lifecycle record: no other code runs in
    /// between, so it is the gated write's (the pull then checks it is).
    fn look_next(&self) -> bool {
        for r in &self.trace.records[self.i..] {
            if r["k"] == "obs" && r["ev"] == "ag_look" {
                return true;
            }
            if boundary(r) {
                return false;
            }
        }
        false
    }

    /// Takes a consumed record's state after (its write), where it can't be checked.
    fn look_take(w: &mut World, rec: &Rec) {
        if let Some(unit) = w.by_id.get(&rec.u).copied().and_then(|u| w.unit_mut(u)) {
            unit.orientation = rec.after.value;
            unit.combat.desired_orientation = rec.after.desired;
        }
    }

    /// The end of the recording: every `ag_look` was reached.
    pub(super) fn look_finish(&mut self, w: &mut World) {
        if !self.ags.look.on {
            return;
        }
        let total =
            self.trace.records.iter().filter(|r| r["k"] == "obs" && r["ev"] == "ag_look").count();
        if self.ags.look.processed != total {
            Self::look_unsupported(
                w,
                format!(
                    "{} of {total} ag_look records never reached (inside a span the replay skips)",
                    total - self.ags.look.processed
                ),
            );
        }
    }

    /// Turn-based code after real time without the agents' synchronization (WrathOracle writes it
    /// before any write): reported.
    fn look_unsynced(&self, w: &World) -> bool {
        self.ags.look.on && w.combat.turn_based && !self.ags.synced && !self.uc_skipping
    }

    /// A write of `u`'s orientation by the commands' code ([`crate::combat::commands::Inputs::
    /// look`]).
    pub(super) fn look_write(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        site: LookSite,
        f_: LookFn,
        arg: Option<LookArg>,
        optional: bool,
    ) -> R<()> {
        if !self.ags.look.on {
            // Without the observer: the sim's write where it knows the argument and that the
            // game made it (its reads stay the recording's).
            if let Some(a) = arg
                && !optional
            {
                crate::combat::commands::look_apply(w, u, f_, a)?;
            }
            return Ok(());
        }
        let id = w.entity(u).unique_id.clone();
        if optional {
            if !self.look_next() {
                self.out.look.aoo_not_made += 1;
                return Ok(());
            }
            self.out.look.aoo_made += 1;
        }
        let what = format!("{} of {id} ({})", f_.name(), site.name());
        let at = self.uc_pull(w, &what, |r| r["k"] == "obs" && r["ev"] == "ag_look")?;
        self.ags.look.processed += 1;
        let trace = self.trace;
        let r = &trace.records[at];
        let Some(rec) = parse(r) else {
            take_readable(w, r);
            return Err(Desync(format!("{what}: an ag_look it can't read ({})", describe(r))));
        };
        if self.look_unsynced(w) {
            Self::look_take(w, &rec);
            return Err(Desync(format!(
                "{what}: a turn-based write after real time without the agents' synchronization"
            )));
        }
        if rec.f != f_ || rec.u != id || !from_site(&rec.origin, site) {
            Self::look_take(w, &rec);
            return Err(Desync(format!("{what}: the recording has {}", describe(r))));
        }
        if rec.rider {
            Self::look_take(w, &rec);
            return Err(Desync(format!("{what}: a rider's (its mount's write: not modelled)")));
        }
        let arg = match arg {
            Some(a) if same_arg(a, rec.arg) => a,
            Some(a) => {
                Self::look_take(w, &rec);
                return Err(Desync(format!(
                    "{what}: the sim's argument {a:?}, the recording's {:?}",
                    rec.arg
                )));
            }
            None => {
                self.out.look.points_taken += 1;
                rec.arg
            }
        };
        if w.unit(u).is_none() {
            return Err(Desync(format!("{what}: not a unit")));
        }
        if !self.look_driven(w) {
            // Real time: the positions and orientations aren't the sim's; the write is taken.
            let unit = w.unit_mut(u).expect("a unit");
            unit.orientation = rec.after.value;
            unit.combat.desired_orientation = rec.after.desired;
            self.out.look.real_time += 1;
            return Ok(());
        }
        let unit = w.unit(u).expect("a unit");
        let before =
            Orientation { value: unit.orientation, desired: unit.combat.desired_orientation };
        let pos = unit.position;
        let after = before.write(f_, arg, pos);
        let ok = bits3(pos) == bits3(rec.pos)
            && same_o(before, rec.before)
            && after.is_some_and(|a| same_o(a, rec.after));
        // (The recorded state carries on either way.)
        let unit = w.unit_mut(u).expect("a unit");
        unit.orientation = rec.after.value;
        unit.combat.desired_orientation = rec.after.desired;
        if !ok {
            return Err(Desync(format!(
                "{what}: the sim's position {pos:?}, orientation {before:?} -> {after:?}; the recording's {}",
                describe(r)
            )));
        }
        *self.out.look.computed.entry(site.name()).or_default() += 1;
        Ok(())
    }

    /// An `ag_look` the sim's code didn't pull: a write by code the sim doesn't run.
    pub(super) fn look_outside(&mut self, w: &mut World, r: &J) {
        self.ags.look.processed += 1;
        let Some(rec) = parse(r) else {
            take_readable(w, r);
            Self::look_unsupported(w, format!("an ag_look it can't read ({})", describe(r)));
            return;
        };
        let Some(u) = w.by_id.get(&rec.u).copied().filter(|u| w.unit(*u).is_some()) else {
            Self::look_unsupported(
                w,
                format!("a write of a unit the sim doesn't have ({})", rec.u),
            );
            return;
        };
        let take = |w: &mut World| {
            let unit = w.unit_mut(u).expect("a unit");
            unit.orientation = rec.after.value;
            unit.combat.desired_orientation = rec.after.desired;
        };
        if !self.ags.look.on {
            take(w);
            return;
        }
        // With the roaming controller the sim's, a write of its the sim's didn't make was
        // reported where it was reached: taken.
        if self.ags.roam.on
            && rec.origin.split(" < ").next()
                == Some("Kingmaker.Controllers.Units.UnitRoamingController.TickOnUnit")
        {
            take(w);
            return;
        }
        if self.look_unsynced(w) {
            Self::look_unsupported(
                w,
                "a turn-based write after real time without the agents' synchronization",
            );
            take(w);
            return;
        }
        if !self.look_driven(w) {
            take(w);
            self.out.look.real_time += 1;
            return;
        }
        if let Some(site) = sim_site(&rec.origin) {
            Self::look_unsupported(
                w,
                format!("{} by {site}, code the sim runs, which didn't make it", rec.f.name()),
            );
            take(w);
            return;
        }
        if rec.rider {
            Self::look_unsupported(w, format!("{} of a rider (its mount's write)", rec.f.name()));
            take(w);
            return;
        }
        let unit = w.unit(u).expect("a unit");
        let before =
            Orientation { value: unit.orientation, desired: unit.combat.desired_orientation };
        let pos = unit.position;
        let by = caller(&rec.origin, 0);
        if bits3(pos) != bits3(rec.pos) || !same_o(before, rec.before) {
            Self::look_unsupported(
                w,
                format!(
                    "{} by {by}: the unit's position or orientation before differs from the sim's",
                    rec.f.name()
                ),
            );
            take(w);
            return;
        }
        if !before.write(rec.f, rec.arg, pos).is_some_and(|a| same_o(a, rec.after)) {
            Self::look_unsupported(
                w,
                format!("{} by {by}: the result differs from the sim's", rec.f.name()),
            );
            take(w);
            return;
        }
        take(w);
        if rec.f.takes_point() {
            self.out.look.outside_angles += 1;
        }
        *self.out.look.outside.entry(format!("{} by {by}", rec.f.name())).or_default() += 1;
    }

    /// `UpdateSlowRotation`'s `DesiredOrientation` read: the sim's (`desired`) when the writes
    /// are, checked against the recording's; else the recording's.
    pub(super) fn look_read(&mut self, w: &mut World, desired: f32, recorded: f32) -> R<f32> {
        if self.ags.look.on {
            if desired.to_bits() != recorded.to_bits() {
                // Reported; the recorded value carries on (the rotation completes with it).
                Self::look_unsupported(
                    w,
                    format!(
                        "UpdateSlowRotation's DesiredOrientation: the sim's {desired}, the recording's {recorded}"
                    ),
                );
                return Ok(recorded);
            }
            self.out.look.reads_checked += 1;
            return Ok(desired);
        }
        self.out.look.reads_taken += 1;
        self.ag_input("DesiredOrientation");
        Ok(recorded)
    }
}
