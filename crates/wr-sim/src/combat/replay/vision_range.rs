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
use crate::combat::vision::range::{self, BoundsUnit, Enabled, Hit, Step};
use crate::world::{EntityIdx, World};

use super::vision::vision_radius;
use super::{Driver, Phase};

/// What sets a unit's corpulence as the sim samples it at a phase, and how many of the hooked
/// events that explain a change of it the sim had seen by then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SizeWatch {
    /// The unit's size and original size.
    pub key: Option<(Option<i32>, Option<i32>)>,
    /// An `AddUnitScale` or `Polymorph` fact is on it (a producer the sim can't follow).
    pub fact_producer: bool,
    /// It rides (`UnitPartRider`, a part: no hook sees it come or go).
    pub rider: bool,
    /// Size writes and view replacements seen ([`crate::combat::vision::Vision::size_gen`]): what
    /// explains a change of `key`.
    pub size_hooks: u64,
    /// Hooked events of any kind seen (`corp_bumps`: facts added or removed among them): what
    /// explains a change of `fact_producer`.
    pub fact_hooks: usize,
}

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
    /// What sets each unit's corpulence as of the last phase (sizes, a producer the sim can't
    /// follow), sampled: a change no hooked event explains is an event too.
    size_watch: HashMap<EntityIdx, SizeWatch>,
    /// Per unit, the phase (stream index) of the last change only the sampling saw: a read
    /// counts only after it. Kept apart from the hooked events, so it never explains a change.
    sampled: HashMap<EntityIdx, usize>,
    /// WrathOracle's awake units at its observation of each frame (`awake_set` once, then
    /// `awake` changes): it writes a unit's `u` row, when it changed, only while the unit is
    /// awake, so a row carries to a later frame only through frames the unit was awake in.
    awake_set: Option<(i64, HashSet<String>)>,
    awake_ev: HashMap<String, Vec<(i64, bool)>>,
    /// Without the driven agent, the frame's comparisons, made at its end (the unit, its overlap,
    /// the recording's set, whether a record gave it).
    pending: Vec<(EntityIdx, Vec<Hit>, HashSet<EntityIdx>, bool)>,
    /// The sim's position writes per unit ([`crate::combat::vision::Vision::pos_writes`]) at the
    /// frame's vision phase (`None`: it didn't run in this frame).
    writes_at_phase: Option<HashMap<EntityIdx, u64>>,
    /// The recording's physics settings aren't the ones the step and sync model is for.
    physics_unmodelled: bool,
    /// The Unity frame of the last tick the bounds controller was followed in.
    last_frame: Option<i64>,
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
                (Some("obs"), Some("awake_set")) if self.awake_set.is_none() => {
                    if let (Some(t), Some(us)) = (r["t"].as_i64(), r["units"].as_array()) {
                        let us = us.iter().filter_map(|x| x.as_str().map(str::to_owned));
                        self.awake_set = Some((t, us.collect()));
                    }
                }
                (Some("obs"), Some("awake")) => {
                    if let (Some(t), Some(u), Some(on)) =
                        (r["t"].as_i64(), r["u"].as_str(), r["on"].as_bool())
                    {
                        self.awake_ev.entry(u.to_owned()).or_default().push((t, on));
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
        // The step and sync model holds for `Physics.autoSyncTransforms` off and
        // `autoSimulation` on, as recorded; otherwise (or unrecorded) no collider's PhysX
        // position is known.
        self.physics_unmodelled = head["physics_auto_sync"].as_bool() != Some(false)
            || head["physics_auto_simulation"].as_bool() != Some(true);
        if let (Some(phase), Some(step), Some(frame)) = (
            head["fixed_phase0"].as_f64(),
            head["fixed_delta_time"].as_f64(),
            head["frame"].as_i64(),
        ) {
            self.clock = Some((phase, frame, step));
        }
        self.on = true;
    }

    /// Whether the observer saw unit `id` awake at its observation of frame `t` (`false` before
    /// the first `awake_set`).
    fn observed_awake(&self, id: &str, t: i64) -> bool {
        let Some((t0, set)) = &self.awake_set else { return false };
        if t < *t0 {
            return false;
        }
        let ev = self.awake_ev.get(id).map_or(&[][..], Vec::as_slice);
        ev.iter().rev().find(|(e, _)| *e <= t).map_or(set.contains(id), |(_, on)| *on)
    }

    /// Unit `id`'s position at frame `want` from its observation rows: a row of that frame, or
    /// the last one before it when the observer saw the unit awake at `want` (it compares the
    /// unit's row with the last it wrote and writes it when it differs). A sleeping unit can be
    /// moved (roaming) with no row: the last row doesn't tell where it is.
    pub(super) fn row_at(&self, id: &str, rows: &[(i64, V3)], want: i64) -> Option<V3> {
        let i = rows.partition_point(|(t, _)| *t <= want);
        let (t, p) = *rows.get(i.checked_sub(1)?)?;
        (t == want || self.observed_awake(id, want)).then_some(p)
    }

    /// A unit's `Corpulence` at the phase placed at stream index `at` (see
    /// [`Driver::vis_corpulence`]), from what sets it now (`now`) and the last hooked event
    /// (`hooked`, its stream index): the last `corp` read before `at` that comes after the last
    /// event, unless a producer the sim can't follow is on the unit. Also whether there was a read
    /// before `at` at all.
    ///
    /// A change of `now` since the last sample that no hooked event of its kind explains (a size
    /// change without a size write, a producer fact without a fact event, a mount) could have
    /// happened anywhere since that sample: it is stamped at `at`, which no earlier read passes.
    pub(super) fn corpulence(
        &mut self,
        u: EntityIdx,
        id: &str,
        now: SizeWatch,
        hooked: Option<usize>,
        at: usize,
    ) -> (Option<f32>, bool) {
        if let Some(was) = self.size_watch.insert(u, now) {
            let unexplained = (was.key != now.key && was.size_hooks == now.size_hooks)
                || (was.fact_producer != now.fact_producer && was.fact_hooks == now.fact_hooks)
                || was.rider != now.rider;
            if unexplained {
                self.sampled.insert(u, at);
            }
        }
        let from = hooked.max(self.sampled.get(&u).copied());
        let latest = self.corp.get(id).and_then(|series| {
            let i = series.partition_point(|(i, _)| *i < at);
            (i > 0).then(|| series[i - 1])
        });
        // A read at the stream index of an event may be from before it: it doesn't count.
        let producer = now.fact_producer || now.rider;
        let read = latest.filter(|(i, _)| from.is_none_or(|f| *i > f) && !producer).map(|(_, c)| c);
        (read, latest.is_some())
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
        // A set holding a collider of unknown place in the order has an unknown order.
        if hits.iter().filter(|h| set.contains(&h.unit)).all(|h| h.order_known) {
            w.combat.vision.near_inexact.remove(&u);
        } else {
            w.combat.vision.near_inexact.insert(u);
        }
        if self.vis.ag {
            self.vis_range_judge(w, u, &hits, set, rec);
        } else {
            // Without the driven agent the positions are the next prefix's rows: whether a
            // position write after this phase moved a unit is known at the frame's end.
            self.vis.range.pending.push((u, hits, set.clone(), rec));
        }
    }

    /// The comparison of a unit's overlap with the recording's set, counted and reported.
    fn vis_range_judge(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        hits: &[Hit],
        set: &HashSet<EntityIdx>,
        rec: bool,
    ) {
        let d = range::differences(hits, set);
        let diffs = d.diffs;
        let c = &mut self.vis.counts;
        c.range_sets_compared += 1;
        c.range_changes += usize::from(rec);
        let open: usize = d.open.iter().sum();
        c.range_sets_open += usize::from(open > 0);
        c.range_changes_open += usize::from(open > 0 && rec);
        for (k, n) in d.open.iter().enumerate() {
            c.range_open[k] += n;
        }
        c.range_decided_given_start += d.decided;
        c.range_guess_agree += d.guess_agree;
        c.range_guess_differ += d.guess_differ;
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
                range::Difference::Count(n) => {
                    format!("(a set of {n}: not a size the overlap's result buffer of 512 gives)")
                }
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

    /// A unit's `Corpulence` as the recording shows it: the last `corp` read before the record the
    /// phase is placed at (a read in the frame's later phases comes after it) and after the last
    /// event the sim saw that could change it ([`crate::combat::vision::note_corpulence_change`]:
    /// a size write, a fact or buff added or removed, a view replaced; the stream index the
    /// event was made at). `None` until the unit's first valid read (counted).
    ///
    /// What sets it (`View.Corpulence` = the prefab's `m_Corpulence` times `GetSizeScale()`): the
    /// unit's size against its original size, the progression's scale percent (`AddUnitScale`),
    /// a polymorph (`DisableSizeScaling`), a mount's size for its rider, the view. The events
    /// are what the sim runs; for a unit that has one of the producers it can't follow (the
    /// three named) the corpulence is unknown while it has one, and a change of what the unit has
    /// seen between two phases (sampled) counts as an event too. An event the sim doesn't run
    /// (an unported action) isn't seen: a limit of the inference.
    fn vis_corpulence(&mut self, w: &World, u: EntityIdx) -> Option<f32> {
        let at = w.combat.nav.at;
        let key = w.unit(u).map(|x| {
            (x.state_size.or(x.original_size).map(|s| s as i32), x.original_size.map(|s| s as i32))
        });
        let bumps = w.combat.vision.corp_bumps.get(&u);
        let now = SizeWatch {
            key,
            fact_producer: ["AddUnitScale", "Polymorph"]
                .iter()
                .any(|c| super::has_component(w, u, c)),
            rider: w.entity(u).parts.iter().any(|p| p.clr == "UnitPartRider"),
            size_hooks: w.combat.vision.size_gen.get(&u).copied().unwrap_or(0),
            fact_hooks: bumps.map_or(0, Vec::len),
        };
        let hooked = bumps.and_then(|b| b.iter().max().copied());
        let r = &mut self.vis.range;
        let (read, latest) = r.corpulence(u, &w.entity(u).unique_id, now, hooked, at);
        if read.is_none() {
            self.vis.counts.range_corp_unknown += 1;
            if latest {
                self.vis.counts.range_corp_stale += 1;
            }
        }
        read
    }

    /// A unit's position after the movement when the sim has it: the driven agent's, or without
    /// it the next frame's observation row ([`RangeDriver::row_at`]: none in the recording's last
    /// frame, for a unit without rows, or for one that slept since its last row). `None`: [`Driver::vis_pos`]'s answer is a guess.
    fn vis_pos_known(&self, w: &World, u: EntityIdx) -> Option<V3> {
        if self.vis.ag {
            return Some(self.vis_pos(w, u, true));
        }
        if self.tick >= self.vis.last_tick {
            return None;
        }
        let id = &w.entity(u).unique_id;
        self.vis.range.row_at(id, self.vis.rows.get(id)?, self.tick + 1)
    }

    /// A unit's position at the frame's start, from the observation rows (the previous frame's
    /// end); `None` without a row.
    fn vis_prefix_pos(&self, w: &World, u: EntityIdx) -> Option<V3> {
        let id = &w.entity(u).unique_id;
        self.vis.range.row_at(id, self.vis.rows.get(id)?, self.tick)
    }

    /// Whether a physics step ran since the last tick (a fixed update before this frame's
    /// controllers): the clock `Time.time - Time.fixedTime` gains the frame's scaled delta in each
    /// frame and loses a fixed step for each fixed update. When the clock or a frame's delta is
    /// missing, or a frame is skipped, the phase isn't known any more, for good (nothing in the
    /// recording re-establishes it): every later step is unknown.
    fn vis_steps(&mut self) -> Step {
        let r = &mut self.vis.range;
        let Some((mut phase, at, step)) = r.clock else { return Step::Unknown };
        match r.frames.get(&self.tick) {
            Some(&(frame, dt)) if frame == at + 1 => {
                phase += dt;
                let mut ran = false;
                while phase >= step {
                    phase -= step;
                    ran = true;
                }
                r.clock = Some((phase, frame, step));
                if ran { Step::Ran } else { Step::NotRan }
            }
            _ => {
                r.clock = None;
                Step::Unknown
            }
        }
    }

    /// `EntityBoundsController.Tick` and `UnitVisionRangeController.Tick` (the vision controller
    /// follows the movement and isn't registered in the pause mode): the sim's overlap of each
    /// awake unit, checked against the unit's `vis` record when the frame has one (a record
    /// follows at once), else now against the set the last record left.
    pub(super) fn vis_range_phase(&mut self, w: &mut World) {
        if !self.vis.on || !self.vis.range.on {
            return;
        }
        // (The clock follows every frame the phase is reached in.)
        let step = self.vis_steps();
        let Some(m) = self.draining.or(self.frame.mode) else { return };
        if super::registered(m, Phase::Visibility) != Some(true) {
            return;
        }
        let first = !self.vis.range.started;
        self.vis.range.started = true;
        // A frame the bounds controller ran in that the sim didn't follow (a skipped frame, a
        // mode it doesn't model) leaves every collider unknown until its next update.
        let frame = self.vis.range.frames.get(&self.tick).map(|x| x.0);
        let followed = frame.zip(self.vis.range.last_frame).is_some_and(|(f, l)| f == l + 1);
        self.vis.range.last_frame = frame;
        if !first && !followed {
            w.combat.vision.bounds.lose_track();
            self.vis.counts.range_frames_lost += 1;
        }
        let pool = crate::scene::units_with_bounds(w);
        let mut list = Vec::with_capacity(pool.len());
        for &u in &pool {
            let known = self.vis_pos_known(w, u);
            let pos = known.unwrap_or_else(|| self.vis_pos(w, u, true));
            let prefix = if first { self.vis_prefix_pos(w, u) } else { Some(pos) };
            // `ShouldBeEnumeratedByEntityPoolEnumerator` is `IsInGame && !Suppressed`; `m_Suppressed`
            // only exists in an area with parts, which the vision run refuses.
            let in_game = w.entity(u).is_in_game;
            // A unit not in the game at the window's first tick may be fade-hiding with its
            // collider enabled: unknown until the sim sees it in the game.
            if first && !in_game {
                w.combat.vision.fade_hidden.insert(u);
            }
            // A view that fade-hides keeps its collider's `enabled` until the fade ends, which
            // isn't modelled (a fade may outlast a return to the game: the mark stays).
            let enabled = match (in_game, w.combat.vision.fade_hidden.contains(&u)) {
                (true, _) => Enabled::Yes,
                (false, true) => Enabled::Unknown,
                (false, false) => Enabled::No,
            };
            self.vis.counts.range_fade_unit_frames += usize::from(enabled == Enabled::Unknown);
            let corpulence = self.vis_corpulence(w, u);
            let size_gen = w.combat.vision.size_gen.get(&u).copied().unwrap_or(0);
            let in_game_gen = w.combat.vision.in_game_gen.get(&u).copied().unwrap_or(0);
            list.push(BoundsUnit {
                unit: u,
                pos,
                prefix,
                in_game,
                enabled,
                corpulence,
                size_gen,
                in_game_gen,
                pos_known: known.is_some(),
            });
        }
        for gone in w.combat.vision.bounds.tick(&list, step).removed {
            let id = w.entity(gone).unique_id.clone();
            Self::vis_report(
                w,
                &format!(
                    "the entity {id} left the entities with bounds: when its view and sphere collider are destroyed isn't modelled"
                ),
            );
        }
        if self.vis.range.physics_unmodelled {
            w.combat.vision.bounds.forget_positions();
            self.vis.counts.range_physics_unknown += 1;
        }
        // A `Physics.SyncTransforms()` since the last phase may have given PhysX transforms a
        // step wouldn't have (this frame's, or the next's when it came after this phase):
        // unknown until a step known to run.
        if std::mem::take(&mut w.combat.vision.explicit_sync) {
            w.combat.vision.bounds.forget_positions();
            self.vis.counts.range_sync_frames += 1;
        }
        self.vis.range.expect.clear();
        self.vis.range.writes_at_phase = Some(w.combat.vision.pos_writes.clone());
        if m == super::mode::PAUSE {
            return;
        }
        for u in w.combat.awake.clone() {
            let hits = if w.unit(u).is_some_and(|x| x.is_extra) {
                Vec::new()
            } else {
                let r = match vision_radius(w, u) {
                    Ok(r) => r,
                    Err(e) => {
                        // (Refused at the start; a fact added since.)
                        let id = w.entity(u).unique_id.clone();
                        Self::vis_report(w, &format!("the vision range: {e} ({id})"));
                        continue;
                    }
                };
                let known = self.vis_pos_known(w, u);
                let pos = known.unwrap_or_else(|| self.vis_pos(w, u, true));
                let mut hits = w.combat.vision.bounds.overlap(pos, r);
                if known.is_none() {
                    // The capsule's place is a guess: nothing is decided, and nothing is evidence
                    // of the result buffer's capacity.
                    for h in &mut hits {
                        (h.tri, h.cause, h.margin, h.guess, h.capped) =
                            (range::Tri::Maybe, Some(range::Cause::Position), None, None, None);
                    }
                }
                hits
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
            if w.combat.vision.near_inexact.contains(m) {
                return None;
            }
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

    /// A frame's end, without the driven agent: a unit whose position the sim wrote after the
    /// vision phase (an ability, a command, a spawn, roaming) stood elsewhere at the phase than the
    /// next prefix's row says, so its queries and its collider's pairs of the frame are the
    /// recording's, and its collider is lost (the transform the tick set from the row is wrong).
    /// Then the frame's comparisons are made.
    pub(super) fn vis_range_frame_end(&mut self, w: &mut World) {
        // (Only a frame whose vision phase ran has comparisons and colliders set from rows.)
        let Some(at_phase) = self.vis.range.writes_at_phase.take() else { return };
        if self.vis.ag {
            return;
        }
        let moved: HashSet<EntityIdx> = w
            .combat
            .vision
            .pos_writes
            .iter()
            .filter(|(u, n)| at_phase.get(u).copied().unwrap_or(0) != **n)
            .map(|(u, _)| *u)
            .collect();
        for u in &moved {
            w.combat.vision.bounds.lose_unit(*u);
        }
        self.vis.counts.range_moved_after += moved.len();
        for (u, mut hits, set, rec) in std::mem::take(&mut self.vis.range.pending) {
            for h in &mut hits {
                if moved.contains(&u) || moved.contains(&h.unit) {
                    (h.tri, h.cause, h.margin, h.guess, h.capped) =
                        (range::Tri::Maybe, Some(range::Cause::Position), None, None, None);
                }
            }
            self.vis_range_judge(w, u, &hits, &set, rec);
        }
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
        w.combat.vision.near_inexact.retain(|u| awake.contains(u));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const U: EntityIdx = EntityIdx(1);

    fn watch(size: i32) -> SizeWatch {
        SizeWatch {
            key: Some((Some(size), Some(size))),
            fact_producer: false,
            rider: false,
            size_hooks: 0,
            fact_hooks: 0,
        }
    }

    #[test]
    fn a_row_carries_only_through_frames_the_unit_was_awake_in() {
        let mut r = RangeDriver {
            awake_set: Some((0, ["a".to_owned()].into_iter().collect())),
            ..Default::default()
        };
        r.awake_ev.insert("a".into(), vec![(10, false), (20, true)]);
        let rows = [(0, [1.0, 0.0, 0.0]), (25, [2.0, 0.0, 0.0])];
        assert_eq!(r.row_at("a", &rows, 0), Some([1.0, 0.0, 0.0]));
        assert_eq!(r.row_at("a", &rows, 9), Some([1.0, 0.0, 0.0]));
        // Asleep from 10: a roamer may have moved without a row.
        assert_eq!(r.row_at("a", &rows, 10), None);
        assert_eq!(r.row_at("a", &rows, 19), None);
        // Awake again at 20 with no row: the observer found the row unchanged.
        assert_eq!(r.row_at("a", &rows, 22), Some([1.0, 0.0, 0.0]));
        assert_eq!(r.row_at("a", &rows, 25), Some([2.0, 0.0, 0.0]));
        assert_eq!(r.row_at("a", &rows, 30), Some([2.0, 0.0, 0.0]));
        // A unit never awake has only the rows of their own frames.
        let b = [(0, [5.0, 0.0, 0.0])];
        assert_eq!(r.row_at("b", &b, 0), Some([5.0, 0.0, 0.0]));
        assert_eq!(r.row_at("b", &b, 1), None);
    }

    #[test]
    fn a_change_and_a_read_in_one_driven_entry_count_as_read_before_change() {
        // Sol's round 11: an entry at 100 whose readers consume records up to 110; a corpulence
        // read at 105 and a size change made while it ran (stamped 100) are in one entry.
        let mut v = crate::combat::vision::Vision::default();
        v.corp_bumps.insert(U, vec![100]);
        v.bump_log.push((U, 0));
        v.restamp_bumps(0, 110);
        assert_eq!(v.corp_bumps[&U], vec![110]);
        let mut r = RangeDriver::default();
        r.corp.insert("u".into(), vec![(105, 0.5), (115, 0.6)]);
        assert_eq!(r.corpulence(U, "u", watch(3), Some(110), 112), (None, true));
        assert_eq!(r.corpulence(U, "u", watch(3), Some(110), 120), (Some(0.6), true));
    }

    #[test]
    fn two_unexplained_size_changes_each_invalidate_the_reads_before_them() {
        // Sol's round 6: an unexplained change sampled at phase 20 must not make the next one,
        // at phase 30, look explained (a sampler's stamp is not a hooked event): the read at 25
        // predates the second change.
        let mut r = RangeDriver::default();
        r.corp.insert("u".into(), vec![(5, 0.5), (25, 0.6)]);
        assert_eq!(r.corpulence(U, "u", watch(3), None, 10), (Some(0.5), true));
        assert_eq!(r.corpulence(U, "u", watch(4), None, 20), (None, true));
        assert_eq!(r.corpulence(U, "u", watch(4), None, 26), (Some(0.6), true));
        assert_eq!(r.corpulence(U, "u", watch(5), None, 30), (None, true));
        assert_eq!(r.corpulence(U, "u", watch(5), None, 40), (None, true));
    }

    #[test]
    fn a_change_explained_by_its_hooked_event_counts_from_that_event() {
        let mut r = RangeDriver::default();
        r.corp.insert("u".into(), vec![(5, 0.5), (25, 0.6)]);
        assert_eq!(r.corpulence(U, "u", watch(3), None, 10), (Some(0.5), true));
        // A size write at 22 changed the size: reads after it count.
        let w = SizeWatch { size_hooks: 1, fact_hooks: 1, ..watch(4) };
        assert_eq!(r.corpulence(U, "u", w, Some(22), 30), (Some(0.6), true));
        // A size change without a size write, after a fact event, is not explained by it.
        let w2 = SizeWatch { fact_hooks: 2, ..SizeWatch { key: watch(5).key, ..w } };
        assert_eq!(r.corpulence(U, "u", w2, Some(28), 32), (None, true));
        // A mount is never explained by a hooked event.
        let mut r = RangeDriver::default();
        r.corp.insert("u".into(), vec![(25, 0.6)]);
        r.corpulence(U, "u", watch(3), None, 10);
        let rider = SizeWatch { rider: true, fact_hooks: 1, ..watch(3) };
        assert_eq!(r.corpulence(U, "u", rider, Some(22), 30), (None, true));
        let off = SizeWatch { rider: false, fact_hooks: 2, ..watch(3) };
        assert_eq!(r.corpulence(U, "u", off, Some(23), 40), (None, true));
    }
}
