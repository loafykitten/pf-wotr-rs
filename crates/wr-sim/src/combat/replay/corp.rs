//! The driven replay's check of `UnitEntityView.Corpulence` (docs/poc5/corpulence.md): the sim's
//! value ([`crate::combat::corpulence`], from the unit's prefab and size) against every `corp`
//! read the recording holds (`uc_v` and `ag_v`: the AI's, the engagement's, the commands' and
//! the agent's `Corpulence` taps).
//!
//! A read is compared after the top-level entry that contains it, with the state that entry left.
//! If an event that could change the unit's corpulence happened in that entry (a size write, a
//! fact added or removed, a view replaced, the size scale reset) the read could be from either
//! side of it: it is counted, not compared. A value the sim can't establish is counted by its
//! cause. Equal means the same bits; a difference is reported (the run is invalid).
//!
//! WrathOracle's `bounds_init` and `bounds_tick` records (the entity-bounds colliders) are a second
//! check where a recording has them: `EntityBoundsController.TickEntity` leaves a collider's radius
//! within 0.05 of `Max(0.05, Corpulence)` (the hysteresis; exactly it for a view of scale 1 at its
//! first tick), whatever the view's scale, so the sim's corpulence must put every recorded radius
//! inside that band. Counted apart: exact, in the band, outside it (reported).

use std::collections::{BTreeMap, HashSet};

use crate::combat::corpulence;
use crate::combat::oracle::{Rv, Trace};
use crate::world::{EntityIdx, World};

use super::Driver;

/// What the check made of the recording's reads.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CorpCounts {
    /// The recording's reads of a unit's corpulence.
    pub reads: usize,
    /// Reads with no unit (an `ag_v` read outside a request) or no finite value.
    pub unattributed: usize,
    /// Reads of a unit the sim hasn't made when the read's entry ended.
    pub no_unit: usize,
    /// Reads whose entry had an event for the unit.
    pub event: usize,
    /// Reads compared: equal bits, and differing (each reported).
    pub equal: usize,
    pub differ: usize,
    /// Reads of a value the sim can't establish, by cause.
    pub unknown: BTreeMap<&'static str, usize>,
    /// The colliders' radii of `bounds_init` and `bounds_tick` of units: with a sim corpulence, the
    /// radius equal to `Max(0.05, Corpulence)`, within the 0.05 band around it, outside it (each
    /// reported); without one (an event in the entry, or no value).
    pub bounds_reads: usize,
    pub bounds_exact: usize,
    pub bounds_band: usize,
    pub bounds_differ: usize,
    pub bounds_unchecked: usize,
}

impl CorpCounts {
    /// Reads the sim has no value for, of any cause.
    #[must_use]
    pub fn unknown_total(&self) -> usize {
        self.unknown.values().sum::<usize>() + self.no_unit
    }
}

/// One read: the record's index, the unit's id and the value.
#[derive(Debug, Clone)]
struct Read {
    at: usize,
    unit: String,
    value: f32,
    /// A collider's radius (`bounds_init`, `bounds_tick`) rather than a `Corpulence` read.
    radius: bool,
}

/// The driver's state for the check.
#[derive(Debug, Default)]
pub(super) struct CorpDriver {
    reads: Vec<Read>,
    /// The first read not yet checked.
    cursor: usize,
    pub counts: CorpCounts,
}

impl CorpDriver {
    /// The recording's corpulence reads, in order: the commands', the AI's and the engagement's
    /// (`uc_v`, whose unit is the record's `u`) and the agent's (`ag_v` without a context, of the
    /// record's unit or the request's); the movement simulation's (`ctx`) are the AI's own steps.
    pub(super) fn start(&mut self, trace: &Trace) {
        let value = |v: &Rv| match v {
            Rv::String(s) => s.parse::<f32>().ok(),
            #[allow(clippy::cast_possible_truncation)]
            _ => v.as_f64().map(|x| x as f32),
        };
        let mut req: Option<String> = None;
        for (i, r) in trace.records.iter().enumerate() {
            if r["k"] != "obs" {
                continue;
            }
            if matches!(r["ev"].as_str(), Some("bounds_init" | "bounds_tick")) {
                if let (Some(u), Some(v)) = (r["e"].as_str(), value(&r["r"]))
                    && r["x"].is_null()
                    && v.is_finite()
                {
                    self.reads.push(Read { at: i, unit: u.to_owned(), value: v, radius: true });
                }
                continue;
            }
            let unit = match r["ev"].as_str() {
                Some("ag_req") => {
                    req = r["u"].as_str().map(str::to_owned);
                    continue;
                }
                Some("ag_req_end") => {
                    req = None;
                    continue;
                }
                Some("uc_v") if r["f"] == "corp" => r["u"].as_str().map(str::to_owned),
                Some("ag_v") if r["f"] == "corp" && r.get("ctx").is_none() => {
                    r["u"].as_str().map(str::to_owned).or_else(|| req.clone())
                }
                _ => continue,
            };
            self.counts.reads += 1;
            match (unit, value(&r["v"]).filter(|v| v.is_finite())) {
                (Some(unit), Some(value)) => {
                    self.reads.push(Read { at: i, unit, value, radius: false });
                }
                _ => self.counts.unattributed += 1,
            }
        }
    }
}

impl Driver<'_> {
    /// A recorded collider radius against the sim's corpulence (see the module comment).
    fn corp_radius(&mut self, w: &mut World, u: EntityIdx, id: &str, at: usize, read: f32) {
        let Ok(c) = corpulence::corpulence(w, u) else {
            self.corp.counts.bounds_unchecked += 1;
            return;
        };
        // `ScaleRadius(Corpulence, 1)`, the band of the hysteresis (a little more, for rounding).
        let t = c.max(0.05);
        if read.to_bits() == t.to_bits() {
            self.corp.counts.bounds_exact += 1;
        } else if (read - t).abs() <= 0.05 + 1e-6 {
            self.corp.counts.bounds_band += 1;
        } else {
            self.corp.counts.bounds_differ += 1;
            w.coverage.unsupported(format!(
                "driven replay: corpulence: {id}'s collider radius is {read} in the recording (record {at}), outside 0.05 of the sim's corpulence {c}"
            ));
        }
    }

    /// The entry that processed records `from..to` is done: its reads are checked, but for the
    /// units with an event in it (`mark` and `events`: where the corpulence events of the vision
    /// and of [`crate::combat::corpulence`] stood when it began).
    pub(super) fn corp_entry_done(
        &mut self,
        w: &mut World,
        from: usize,
        to: usize,
        mark: usize,
        events: usize,
    ) {
        if self.corp.reads.len() == self.corp.cursor {
            return;
        }
        let mut units: HashSet<EntityIdx> =
            w.combat.vision.bump_log[mark..].iter().map(|(e, _)| *e).collect();
        units.extend(w.combat.corpulence.events[events..].iter().copied());
        self.corp_check(w, from, to, &units);
    }

    /// Checks the reads in records `from..to` (the entry just processed) against the sim's
    /// corpulence now. `events`: the units with a corpulence event in the entry.
    pub(super) fn corp_check(
        &mut self,
        w: &mut World,
        from: usize,
        to: usize,
        events: &HashSet<EntityIdx>,
    ) {
        while let Some(r) = self.corp.reads.get(self.corp.cursor) {
            if r.at >= to {
                break;
            }
            self.corp.cursor += 1;
            if r.at < from {
                // (Before the entry: not reached, e.g. before the first record the driver ran.)
                continue;
            }
            let radius = r.radius;
            let Some(&u) = w.by_id.get(&r.unit) else {
                // (Colliders of map objects and anchors: not units.)
                if !radius {
                    self.corp.counts.no_unit += 1;
                }
                continue;
            };
            if radius && w.unit(u).is_none() {
                continue;
            }
            if radius {
                self.corp.counts.bounds_reads += 1;
            }
            if events.contains(&u) {
                if radius {
                    self.corp.counts.bounds_unchecked += 1;
                } else {
                    self.corp.counts.event += 1;
                }
                continue;
            }
            if radius {
                let (id, at, read) = (r.unit.clone(), r.at, r.value);
                self.corp_radius(w, u, &id, at, read);
                continue;
            }
            let (id, at, read) = (r.unit.clone(), r.at, r.value);
            match corpulence::corpulence(w, u) {
                Err(cause) => *self.corp.counts.unknown.entry(cause.name()).or_default() += 1,
                Ok(c) if c.to_bits() == read.to_bits() => self.corp.counts.equal += 1,
                Ok(c) => {
                    self.corp.counts.differ += 1;
                    w.coverage.unsupported(format!(
                        "driven replay: corpulence: {id}'s Corpulence is {read} in the recording (record {at}), {c} in the sim"
                    ));
                }
            }
        }
    }
}
