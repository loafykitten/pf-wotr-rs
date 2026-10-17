//! The driven replay's side of `UnitProneController` (`combat/prone.rs`): the controller runs in
//! its place in every frame ([`Phase::Prone`]) on the awake units, with the sim's posture
//! states, and the one thing it does that the recording shows, `Commands.InterruptAll()` on a
//! unit falling prone (WrathOracle's `uc_x` record, run by the commands model where it was
//! recorded), is checked in both directions:
//!
//! - a unit the sim's controller makes fall needs the recording's call from the controller
//!   (`UnitProneController.Tick` in its chain), in this frame, for that unit;
//! - a recorded call from the controller for a unit the sim didn't make fall is reported, and so
//!   is one left over at the frame's end.
//!
//! On when the commands are the sim's (the recording has WrathOracle's command records with the
//! turn controller's tick). A unit whose awake state the sim couldn't establish is ticked too
//! (the recording's awake sets settle it in the previous phase): the awake list is complete once
//! `Phase::Sleeping` has run.

use super::{Driver, mode};
use crate::combat::oracle::Rv;
use crate::combat::prone;
use crate::world::{EntityIdx, World};

/// The frame's recorded calls of `Commands.InterruptAll()` by the controller.
const TICK: &str = "Kingmaker.Controllers.Units.UnitProneController.Tick";

/// What the replay computed of the controller (the coverage line).
#[derive(Debug, Default, Clone)]
pub struct ProneCounts {
    /// `TickOnUnit`s the sim ran, units falling prone and getting up.
    pub ticks: usize,
    pub falls: usize,
    pub get_ups: usize,
}

/// Whether a `uc_x` record is the controller's `InterruptAll`.
#[must_use]
pub(super) fn is_prone_call(r: &Rv) -> bool {
    r["fn"] == "InterruptAll"
        && r["origin"].as_str().is_some_and(|o| o.split(" < ").any(|f| f == TICK))
}

impl Driver<'_> {
    /// A recorded `InterruptAll` by the controller, for `unit`: noted for the tick's check.
    pub(super) fn prone_call(&mut self, w: &mut World, unit: Option<EntityIdx>) {
        match unit {
            Some(u) if self.prone_on => self.prone_calls.push(u),
            Some(_) => {}
            None => w.coverage.unsupported(
                "driven replay: the prone controller's InterruptAll of an unknown unit",
            ),
        }
    }

    /// `UnitProneController.Tick` over the awake units, then the check of the recorded calls.
    pub(super) fn prone_tick(&mut self, w: &mut World) {
        if !self.prone_on {
            return;
        }
        let cutscene = self.draining.or(self.frame.mode) == Some(mode::CUTSCENE);
        let mut calls = std::mem::take(&mut self.prone_calls);
        for u in w.combat.awake.clone() {
            self.out.prone.ticks += 1;
            match prone::tick_unit(w, u, cutscene) {
                Ok(step) => {
                    if step.fall {
                        self.out.prone.falls += 1;
                        match calls.iter().position(|c| *c == u) {
                            Some(i) => {
                                calls.remove(i);
                            }
                            None => w.coverage.unsupported(
                                "driven replay: the prone controller made a unit fall prone, but the recording has no InterruptAll by it for the unit in this frame",
                            ),
                        }
                    }
                    if step.get_up {
                        self.out.prone.get_ups += 1;
                    }
                }
                Err(e) => {
                    w.coverage.unsupported(format!("driven replay: UnitProneController: {e}"));
                }
            }
        }
        for _ in calls {
            w.coverage.unsupported(
                "driven replay: the recording has an InterruptAll by the prone controller the sim's controller didn't make (a unit not falling prone, or not awake)",
            );
        }
    }
}
