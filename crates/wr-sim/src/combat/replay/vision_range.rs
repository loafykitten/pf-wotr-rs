//! The driven replay's side of the vision range (docs/poc5/vision.md): `CanBeInVisionRange` is
//! the sim's overlap (`combat/vision/range.rs`), computed in [`Phase::Visibility`] for every awake
//! unit of every frame the vision controller runs, and checked against WrathOracle's `vis` records
//! (a unit's set after the controller's tick, written when it differs from the last record's) or,
//! in a frame without a record, against the set the last record left. A difference is reported and
//! the recording's set is the one the run goes on with.
//!
//! The collider's inputs the sim doesn't have are the recording's: each unit's `Corpulence` (its
//! `corp` reads before the record the phase is placed at; none yet: unknown) and the physics clock
//! (`fixed_phase0` of the begin record and each tick's scaled delta: which frames run a physics
//! step, the frame a collider's transform reaches PhysX in).

use std::collections::{HashMap, HashSet};

use crate::combat::oracle::{Rv, Trace};
use crate::combat::vision::V3;
use crate::combat::vision::range::{self, BoundsUnit, Enabled, Hit};
use crate::world::{EntityIdx, World};

use super::vision::vision_radius;
use super::{Driver, Phase};

/// The view's `lossyScale` (larger horizontal component) a collider is made with: the sim doesn't
/// have it (the unit prefab's root scale times the size scale), and 1 is assumed. The
/// recordings agree with the assumption: pairs within 5 cm of the capsule's surface, the ones another
/// scale could decide, are counted (`range_within_5cm`) and every one of them equals the recording's.
const ASSUMED_VIEW_SCALE: f32 = 1.0;

/// The vision controller's side: the sets the sim computes, checked against the `vis` records.
#[derive(Debug, Default)]
pub(super) struct RangeDriver {
    /// The sets are the sim's, checked against the records; else they stay the records'.
    on: bool,
    /// The recording's reads of each unit's `Corpulence` (`uc_v`/`ag_v` `corp`), by unit id: the
    /// record's index and the value, in order.
    corp: HashMap<String, Vec<(usize, f32)>>,
    /// The (frame, observer) pairs the recording has a `vis` record for.
    recs: HashSet<(i64, String)>,
    /// WrathOracle's `VisionLast`: each unit's set as of its last `vis` record (a record is the
    /// difference from it; a sleeping unit's set is cleared, its last stays).
    pub last: HashMap<EntityIdx, HashSet<EntityIdx>>,
    /// The frame's overlaps of the units whose `vis` record is still to come.
    pub expect: HashMap<EntityIdx, Vec<Hit>>,
    /// The bounds controller has ticked in this run.
    started: bool,
    /// The physics clock after the last frame it was advanced to: `Time.time - Time.fixedTime`,
    /// the frame, and the fixed step; `None` without the begin record's `fixed_phase0` (every
    /// frame is then assumed to run a step).
    clock: Option<(f64, i64, f64)>,
    /// The Unity frame of each recorded tick and its scaled delta (`Time.deltaTime`).
    frames: HashMap<i64, (i64, f64)>,
}

impl RangeDriver {
    /// Reads what the recording gives: the corpulences, the `vis` records' frames and units, the
    /// physics clock.
    pub(super) fn start(&mut self, trace: &Trace) {
        let value = |v: &Rv| match v {
            Rv::String(s) => s.parse::<f32>().ok(),
            #[allow(clippy::cast_possible_truncation)]
            _ => v.as_f64().map(|x| x as f32),
        };
        for (i, r) in trace.records.iter().enumerate() {
            match (r["k"].as_str(), r["ev"].as_str()) {
                (Some("obs"), Some("uc_v" | "ag_v")) if r["f"] == "corp" => {
                    if let (Some(u), Some(c)) = (r["u"].as_str(), value(&r["v"])) {
                        self.corp.entry(u.to_owned()).or_default().push((i, c));
                    }
                }
                (Some("obs"), Some("vis")) => {
                    if let (Some(u), Some(t)) = (r["u"].as_str(), r["t"].as_i64()) {
                        self.recs.insert((t, u.to_owned()));
                    }
                }
                (Some("tick"), _) => {
                    if let (Some(t), Some(f), Some(d)) =
                        (r["t"].as_i64(), r["frame"].as_i64(), r["delta"].as_f64())
                    {
                        #[allow(clippy::cast_possible_truncation)]
                        self.frames.insert(t, (f, f64::from(d as f32)));
                    }
                }
                _ => {}
            }
        }
        let head = &trace.records[0];
        if let (Some(phase), Some(step), Some(frame)) = (
            head["fixed_phase0"].as_f64(),
            head["fixed_delta_time"].as_f64(),
            head["frame"].as_i64(),
        ) {
            self.clock = Some((phase, frame, step));
        }
        self.on = true;
    }
}

impl Driver<'_> {
    /// The sim's overlap of `u` against the set the recording gives it (a `vis` record's, or the
    /// last one's in a frame without a record): the set's enumeration order is recorded for the
    /// memory's near units, and a difference is reported.
    pub(super) fn vis_range_check(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        set: &HashSet<EntityIdx>,
        rec: bool,
    ) {
        let Some(hits) = self.vis.range.expect.remove(&u) else { return };
        // The set's enumeration order: the overlap's (the colliders' order), the set's units that
        // have no collider after it.
        let mut order: Vec<EntityIdx> =
            hits.iter().map(|h| h.unit).filter(|x| set.contains(x)).collect();
        let mut rest: Vec<EntityIdx> = set.iter().copied().filter(|x| !order.contains(x)).collect();
        rest.sort();
        order.extend(rest);
        w.combat.vision.near_order.insert(u, order);
        let (diffs, maybe) = range::differences(&hits, set);
        let c = &mut self.vis.counts;
        c.range_sets += 1;
        c.range_changes += usize::from(rec);
        c.range_maybe_pairs += maybe;
        if diffs.is_empty() {
            return;
        }
        c.range_differences += diffs.len();
        let name = |x: &EntityIdx| w.entity(*x).unique_id.clone();
        let list: Vec<String> = diffs
            .iter()
            .map(|d| match d {
                range::Difference::Missing(x) => format!("+{}", name(x)),
                range::Difference::Extra(x) => format!("-{}", name(x)),
            })
            .collect();
        Self::vis_report(
            w,
            &format!(
                "the vision range of {} differs from the recording's (+: the sim's only, -: the recording's only): {}",
                name(&u),
                list.join(" ")
            ),
        );
    }

    /// A unit's `Corpulence` as the recording shows it before the record the phase is placed at
    /// (the record being processed: a read in the frame's later phases comes after it); `None`
    /// until the unit's first read (counted).
    fn vis_corpulence(&mut self, w: &World, u: EntityIdx) -> Option<f32> {
        let at = w.combat.nav.at;
        let read = self.vis.range.corp.get(&w.entity(u).unique_id).and_then(|series| {
            let i = series.partition_point(|(i, _)| *i < at);
            (i > 0).then(|| series[i - 1].1)
        });
        if read.is_none() {
            self.vis.counts.range_corp_unknown += 1;
        }
        read
    }

    /// A unit's position at the frame's start, from the observation rows (the previous frame's
    /// end), else the sim's.
    fn vis_prefix_pos(&self, w: &World, u: EntityIdx) -> V3 {
        if let Some(rows) = self.vis.rows.get(&w.entity(u).unique_id) {
            let i = rows.partition_point(|(t, _)| *t <= self.tick);
            if i > 0 {
                return rows[i - 1].1;
            }
        }
        w.unit(u).map_or([0.0; 3], |x| x.position)
    }

    /// Whether a physics step ran since the last tick (a fixed update before this frame's
    /// controllers): the clock `Time.time - Time.fixedTime` gains the frame's scaled delta in each
    /// frame and loses a fixed step for each fixed update. Without the clock, every frame is
    /// taken to run one.
    fn vis_steps(&mut self) -> bool {
        let Some(&(frame, dt)) = self.vis.range.frames.get(&self.tick) else { return true };
        let Some((mut phase, mut at, step)) = self.vis.range.clock else { return true };
        if frame <= at || frame - at > 100_000 {
            return true;
        }
        let mut ran = false;
        while at < frame {
            at += 1;
            phase += dt;
            while phase >= step {
                phase -= step;
                ran = true;
            }
        }
        self.vis.range.clock = Some((phase, at, step));
        ran
    }

    /// `EntityBoundsController.Tick` and `UnitVisionRangeController.Tick` (the vision controller
    /// follows the movement and isn't registered in the pause mode): the sim's overlap of each
    /// awake unit, checked against the unit's `vis` record when the frame has one (a record
    /// follows at once), else now against the set the last record left.
    pub(super) fn vis_range_phase(&mut self, w: &mut World) {
        if !self.vis.on || !self.vis.range.on {
            return;
        }
        let Some(m) = self.draining.or(self.frame.mode) else { return };
        if super::registered(m, Phase::Visibility) != Some(true) {
            return;
        }
        let first = !self.vis.range.started;
        self.vis.range.started = true;
        let pool = crate::scene::units_with_bounds(w);
        let mut list = Vec::with_capacity(pool.len());
        for &u in &pool {
            let pos = self.vis_pos(w, u, true);
            let prefix = if first { self.vis_prefix_pos(w, u) } else { pos };
            // `ShouldBeEnumeratedByEntityPoolEnumerator` is `IsInGame && !Suppressed`; `m_Suppressed`
            // only exists in an area with parts, which the vision run refuses.
            let in_game = w.entity(u).is_in_game;
            if in_game {
                w.combat.vision.fade_hidden.remove(&u);
            }
            // A view that fade-hides keeps its collider's `enabled` until the fade ends.
            let enabled = match (in_game, w.combat.vision.fade_hidden.contains(&u)) {
                (true, _) => Enabled::Yes,
                (false, true) => Enabled::Unknown,
                (false, false) => Enabled::No,
            };
            self.vis.counts.range_fade_unit_frames += usize::from(enabled == Enabled::Unknown);
            let corpulence = self.vis_corpulence(w, u);
            list.push(BoundsUnit {
                unit: u,
                pos,
                prefix,
                in_game,
                enabled,
                corpulence,
                view_scale: ASSUMED_VIEW_SCALE,
            });
        }
        let step = self.vis_steps();
        for gone in w.combat.vision.bounds.tick(&list, step).removed {
            let id = w.entity(gone).unique_id.clone();
            Self::vis_report(
                w,
                &format!(
                    "the entity {id} left the entities with bounds: when its view and sphere collider are destroyed isn't modelled"
                ),
            );
        }
        self.vis.range.expect.clear();
        if m == super::mode::PAUSE {
            return;
        }
        for u in w.combat.awake.clone() {
            let hits = if w.unit(u).is_some_and(|x| x.is_extra) {
                Vec::new()
            } else {
                let r = vision_radius(w, u).unwrap_or(0.0);
                let pos = self.vis_pos(w, u, true);
                w.combat.vision.bounds.overlap(pos, r)
            };
            self.vis_tally(&hits);
            self.vis.range.expect.insert(u, hits);
            let id = w.entity(u).unique_id.clone();
            let last = self.vis.range.last.get(&u).cloned();
            if self.vis.range.recs.contains(&(self.tick, id)) {
                // The record follows at once; until then the unit keeps its last set.
                w.combat.vision.near.insert(u, last.unwrap_or_default());
                continue;
            }
            let Some(set) = last else {
                // The controller's first set of a unit is always recorded.
                self.vis.range.expect.remove(&u);
                Self::vis_report(w, "the vision range of a unit the recording never shows");
                continue;
            };
            w.combat.vision.near.insert(u, set.clone());
            self.vis_range_check(w, u, &set, false);
        }
    }

    /// `UnitGroup.ExtractNearUnits` into a cleared `HashSet`: the members' sets in order, a unit
    /// once at its first place. `None` when a member's set order isn't the sim's.
    pub(super) fn vis_near_union(
        &self,
        w: &World,
        members: &[EntityIdx],
    ) -> Option<Vec<EntityIdx>> {
        if !self.vis.on || !self.vis.range.on {
            return None;
        }
        let mut out: Vec<EntityIdx> = Vec::new();
        for m in members {
            match w.combat.vision.near_order.get(m) {
                Some(l) => {
                    for x in l {
                        if !out.contains(x) {
                            out.push(*x);
                        }
                    }
                }
                None if w.combat.vision.near.get(m).is_none_or(HashSet::is_empty) => {}
                None => return None,
            }
        }
        Some(out)
    }

    /// The margins of a query's pairs.
    fn vis_tally(&mut self, hits: &[Hit]) {
        let c = &mut self.vis.counts;
        for h in hits {
            let Some(margin) = h.margin else { continue };
            c.range_pairs += 1;
            c.range_rounding += usize::from(h.rounding);
            let a = margin.abs();
            c.range_within_5cm += usize::from(a < 0.05);
            c.range_within_1cm += usize::from(a < 0.01);
            c.range_within_1mm += usize::from(a < 0.001);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let um = (a * 1e6).min(1e18) as u64;
            c.range_closest_um = c.range_closest_um.min(um);
        }
    }

    /// `SleepingUnitsController` put units to sleep: `IsSleeping`'s setter cleared their sets.
    pub(super) fn vis_slept(&mut self, w: &mut World) {
        if !self.vis.on {
            return;
        }
        let awake: HashSet<EntityIdx> =
            w.combat.awake.iter().chain(&w.combat.awake_unknown).copied().collect();
        w.combat.vision.near.retain(|u, _| awake.contains(u));
        w.combat.vision.near_order.retain(|u, _| awake.contains(u));
    }
}
