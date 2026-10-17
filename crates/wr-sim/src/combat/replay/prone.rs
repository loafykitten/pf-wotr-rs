//! The driven replay's side of `UnitProneController` (`combat/prone.rs`): the controller runs in
//! its place in every frame with the sim's posture states, unit by unit as the game does
//! (`BaseUnitController.Tick`), and the two things it does that the recording shows are checked:
//!
//! - **The stop of `UnitEntityView.EnterProneState`** (WrathOracle's `ag_call Stop` with that
//!   caller, run by the agent's driver) and **`Commands.InterruptAll()`** (the `uc_x` record by
//!   `UnitProneController.Tick`, run by the commands model) each name a unit the controller
//!   makes fall. The first record of either for a unit ticks the unit then (a unit's whole
//!   sequence is processed before the next unit's, as in the game, so the agent's read of the
//!   view's posture is the sim's); the unit must fall. [`Phase::Prone`] is the place of those
//!   records and runs nothing of its own.
//! - **[`Phase::ProneEnd`]** (before the next controller's first record, or at the frame's end):
//!   every other awake unit is ticked, and each fall needs the recorded `InterruptAll` (a fall of
//!   a unit without an agent has no stop), each recorded call a fall.
//!
//! On when the commands are the sim's (the recording has WrathOracle's command records with the
//! turn controller's tick). The awake list is complete once `Phase::Sleeping` has run, but for
//! the window's last frame (no recorded next awake set): there a unit the sim can't place is
//! taken from the frame's own recorded set, or, without one, ticked on a copy and reported if
//! that would change its posture.

use super::{Driver, Phase, mode};
use crate::combat::commands::{Desync, R};
use crate::combat::oracle::Rv;
use crate::combat::prone;
use crate::world::{EntityIdx, World};

/// The prone controller's frame in a caller chain.
const TICK: &str = "Kingmaker.Controllers.Units.UnitProneController.Tick";

/// What the replay computed of the controller (the coverage line).
#[derive(Debug, Default, Clone)]
pub struct ProneCounts {
    /// `TickOnUnit`s the sim ran, units falling prone and getting up.
    pub ticks: usize,
    pub falls: usize,
    pub get_ups: usize,
    /// Units of the window's last frame taken as asleep from its prefix's awake set.
    pub assumed_asleep: usize,
}

/// Whether a `uc_x` record is the controller's `InterruptAll`.
#[must_use]
pub(super) fn is_prone_call(r: &Rv) -> bool {
    r["fn"] == "InterruptAll"
        && r["origin"].as_str().is_some_and(|o| o.split(" < ").any(|f| f == TICK))
}

impl Driver<'_> {
    /// Ticks `u` once in this frame's controller phase (the unit's first record, or the phase's
    /// end); returns whether it fell.
    fn prone_unit(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        self.place(w, Phase::Prone, "the prone controller");
        // `BaseUnitController.Tick` visits `AwakeUnits` in order and finishes a unit's visit (its
        // stop, then its `InterruptAll`) before the next: a named unit must be awake and neither
        // before the unit last named, nor after it while that unit's records are still coming.
        let at = self.prone_order(w).iter().position(|x| *x == u);
        let Some(at) = at else {
            return Err(Desync("a prone controller record of a unit that isn't awake".into()));
        };
        match self.prone_cursor {
            Some(c) if at < c => {
                return Err(Desync(
                    "a prone controller record out of the awake list's order".into(),
                ));
            }
            Some(c) if at > c && self.prone_open => {
                return Err(Desync(
                    "a prone controller record of the next unit before the last one's calls came"
                        .into(),
                ));
            }
            _ => {}
        }
        self.prone_cursor = Some(at);
        self.prone_tick_unit(w, u)
    }

    /// The units the controller visits, in order: the awake list, and in the window's last frame
    /// (no recorded next awake set) the units of the frame's prefix set the sim couldn't place.
    fn prone_order(&self, w: &World) -> Vec<EntityIdx> {
        let mut units = w.combat.awake.clone();
        if !self.awake_sets.contains_key(&(self.tick + 1))
            && let Some(set) = self.awake_sets.get(&self.tick)
        {
            units.extend(
                w.combat
                    .awake_unknown
                    .iter()
                    .copied()
                    .filter(|u| set.contains(&w.entity(*u).unique_id)),
            );
        }
        units
    }

    /// [`Self::prone_unit`] in the phase (no placing: the phase's end runs inside the frame's).
    fn prone_tick_unit(&mut self, w: &mut World, u: EntityIdx) -> R<bool> {
        if !self.prone_done.insert(u) {
            return Ok(self.prone_falls.contains(&u));
        }
        let cutscene = self.draining.or(self.frame.mode) == Some(mode::CUTSCENE);
        self.out.prone.ticks += 1;
        match prone::tick_unit(w, u, cutscene) {
            Ok(step) => {
                if step.fall {
                    self.out.prone.falls += 1;
                    self.prone_falls.push(u);
                }
                if step.get_up {
                    self.out.prone.get_ups += 1;
                }
                Ok(step.fall)
            }
            Err(e) => Err(Desync(format!("UnitProneController: {e}"))),
        }
    }

    /// The recorded stop of `EnterProneState` for `u`: the controller makes it fall now.
    pub(super) fn prone_stop(&mut self, w: &mut World, u: EntityIdx) -> R<()> {
        if !self.prone_on {
            return Ok(());
        }
        if self.prone_unit(w, u)? {
            self.prone_open = true;
            Ok(())
        } else {
            Err(Desync(
                "EnterProneState of a unit the sim's prone controller doesn't make fall".into(),
            ))
        }
    }

    /// A recorded `InterruptAll` by the controller, for `unit`: the unit falls now, and the call
    /// is noted for the phase's end.
    pub(super) fn prone_call(&mut self, w: &mut World, unit: Option<EntityIdx>) {
        if !self.prone_on {
            return;
        }
        let Some(u) = unit else {
            w.coverage.unsupported(
                "driven replay: the prone controller's InterruptAll of an unknown unit",
            );
            return;
        };
        match self.prone_unit(w, u) {
            Ok(true) => {
                self.prone_calls.push(u);
                self.prone_open = false;
            }
            Ok(false) => w.coverage.unsupported(
                "driven replay: the recording has an InterruptAll by the prone controller for a unit the sim's controller doesn't make fall (or not awake)",
            ),
            Err(d) => w.coverage.unsupported(format!("driven replay: {}", d.0)),
        }
    }

    /// The controller's remaining units, then the check of the falls against the calls.
    ///
    /// A unit whose awake state the sleeping controller couldn't settle (the recording's next
    /// awake set is missing: the window's last frame) is taken from the frame's own recorded set
    /// [I: the frame's own sleeping-controller change is what that leaves out]; without a set it
    /// is ticked on a copy and reported if that would change its posture.
    pub(super) fn prone_end(&mut self, w: &mut World) {
        if !self.prone_on {
            return;
        }
        let cutscene = self.draining.or(self.frame.mode) == Some(mode::CUTSCENE);
        let mut units = w.combat.awake.clone();
        let last = !self.awake_sets.contains_key(&(self.tick + 1));
        for u in w.combat.awake_unknown.clone() {
            let id = w.entity(u).unique_id.clone();
            match last.then(|| self.awake_sets.get(&self.tick)).flatten().map(|s| s.contains(&id)) {
                Some(true) => units.push(u),
                Some(false) => self.out.prone.assumed_asleep += 1,
                None => {
                    if prone::would_change(w, u, cutscene) {
                        w.coverage.unsupported(
                            "driven replay: UnitProneController would change a unit whose awake state isn't established",
                        );
                    }
                }
            }
        }
        for u in units {
            if let Err(d) = self.prone_tick_unit(w, u) {
                w.coverage.unsupported(format!("driven replay: {}", d.0));
            }
        }
        // Each fall needs the recorded `InterruptAll` of the controller for the unit (a recorded
        // call made its unit fall when it came).
        let mut calls = std::mem::take(&mut self.prone_calls);
        for u in std::mem::take(&mut self.prone_falls) {
            match calls.iter().position(|c| *c == u) {
                Some(i) => {
                    calls.remove(i);
                }
                None => w.coverage.unsupported(
                    "driven replay: the prone controller made a unit fall prone, but the recording has no InterruptAll by it for the unit in this frame",
                ),
            }
        }
        for _ in calls {
            w.coverage.unsupported(
                "driven replay: the recording has more InterruptAll calls by the prone controller than the sim's controller made falls",
            );
        }
        self.prone_done.clear();
        self.prone_cursor = None;
        self.prone_open = false;
    }
}
