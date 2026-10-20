//! The colliders as WrathOracle's `bounds_*` records show them (docs/poc5/vision.md, "The window's
//! start, from the records").
//!
//! With `bounds_init` (the entity-bounds scene's roots before the window's first tick),
//! `bounds_tick` (every `TickEntity` call: the radius before and after, the transform after, `enabled`,
//! whether the call made the collider), `bounds_set` (the collider writes outside `TickEntity`)
//! and `phys_sync` (every explicit `Physics.SyncTransforms()`), a collider's radius, transform,
//! `enabled`, creation order and history are the game's own, not the sim's hypotheses: nothing
//! about them is left to the recording but PhysX's copy of the transform, which follows the
//! physics steps (the clock, still inferred) and the recorded syncs, each at its place in the stream. What the sim models of the
//! bounds controller (the radius rule, the update list, the transform rule, `UpdateViewActive`)
//! is then a check on the records, counted and reported where it contradicts them.
//!
//! The order of the events is the stream's: a write before the frame's vision phase ran is
//! `early` (it is in the query), one after it `late` (the next query sees it). A destruction
//! (`Object.Destroy`) takes effect at the frame's end, so an early one still shows in this
//! frame's query.

use super::Bounds;
use std::collections::HashSet;

use super::{
    BoundsUnit, Collider, Enabled, EntityIdx, RadiusIv, Seen, Step, V3, next_radius, orientation,
};

/// A collider as `bounds_init` shows it, or a `bounds_tick` that made it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecCollider {
    pub unit: EntityIdx,
    /// The GameObject's instance id (`bounds_init`'s `id`, `bounds_tick`'s `c`): what
    /// `bounds_set` names.
    pub id: i32,
    pub radius: f32,
    pub enabled: bool,
    /// The transform's position (Unity's, not PhysX's copy).
    pub pos: V3,
}

/// One `TickEntity` call of a unit (`bounds_tick`, without an exception).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecTick {
    /// The record's place in the stream (orders it among the writes of its frame).
    pub seq: i64,
    pub unit: EntityIdx,
    /// The call created the collider.
    pub made: bool,
    /// The radius before (`None`: no collider) and after.
    pub r0: Option<f32>,
    pub r: Option<f32>,
    pub pos: Option<V3>,
    pub on: Option<bool>,
    pub id: Option<i32>,
}

/// A write outside `TickEntity` (`bounds_set`, `phys_sync`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Write {
    /// `UpdateViewActive` changed `enabled`.
    Active { id: i32, on0: bool, on: bool },
    /// `UpdateSphereColliderPositionIfNeeded` moved the transform.
    Pos { id: i32, pos0: V3, pos: V3 },
    /// `DestroySphereBoundsCollider`: the collider leaves the scene at the frame's end.
    Gone { id: i32 },
    /// `Physics.SyncTransforms()` (or DollRoom's `Simulate`): every transform reaches PhysX.
    Sync,
}

/// A write and whether it came after the frame's vision phase.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    pub seq: i64,
    pub write: Write,
    pub late: bool,
}

/// What a recorded tick checked and applied.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecCounts {
    /// `TickEntity` calls of units applied, of which made a collider.
    pub ticks: usize,
    pub made: usize,
    /// Recorded radii after a tick, against the sim's rule: equal to `Max(0.05, Corpulence)`,
    /// within the hysteresis band of it, not checked (the corpulence unknown).
    pub radius_exact: usize,
    pub radius_band: usize,
    pub radius_unchecked: usize,
    /// Recorded transforms after a tick equal to the sim's rule (`Vector3 ==` against the unit's
    /// position), not checked (the unit's position not known).
    pub pos_equal: usize,
    pub pos_unchecked: usize,
    /// Unit-frames: the sim's update list (position, game state or size changed) against the
    /// units that have a `bounds_tick`: agree, the sim's only, the recording's only.
    pub listed_agree: usize,
    pub listed_sim_only: usize,
    pub listed_rec_only: usize,
    pub listed_unchecked: usize,
    /// Unit-frames: the collider's `enabled` against the sim's (`IsInGame`), of which the sim
    /// could not tell (a fade-hiding view, or a collider made while its unit was hidden).
    pub enabled_checked: usize,
    pub enabled_unknown: usize,
    /// Writes applied: `UpdateViewActive`, the position updates, destructions, syncs.
    pub active: usize,
    pub moved: usize,
    pub gone: usize,
    pub syncs: usize,
}

impl RecCounts {
    pub fn add(&mut self, o: &Self) {
        self.ticks += o.ticks;
        self.made += o.made;
        self.radius_exact += o.radius_exact;
        self.radius_band += o.radius_band;
        self.radius_unchecked += o.radius_unchecked;
        self.pos_equal += o.pos_equal;
        self.pos_unchecked += o.pos_unchecked;
        self.listed_agree += o.listed_agree;
        self.listed_sim_only += o.listed_sim_only;
        self.listed_rec_only += o.listed_rec_only;
        self.listed_unchecked += o.listed_unchecked;
        self.enabled_checked += o.enabled_checked;
        self.enabled_unknown += o.enabled_unknown;
        self.active += o.active;
        self.moved += o.moved;
        self.gone += o.gone;
        self.syncs += o.syncs;
    }
}

/// What [`Bounds::tick_recorded`] found: the counts, and the records the sim contradicts (the
/// unit when the finding is about one, and what).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RecOut {
    pub counts: RecCounts,
    pub problems: Vec<(Option<EntityIdx>, String)>,
}

impl RecOut {
    fn problem(&mut self, unit: Option<EntityIdx>, what: impl Into<String>) {
        self.problems.push((unit, what.into()));
    }
}

impl Collider {
    /// A collider as the records show it. `made_now`: created in this frame's tick (PhysX has it
    /// at the origin until a step or a sync).
    fn recorded(r: &RecCollider, made_now: bool) -> Self {
        Self {
            unit: r.unit,
            phys: if made_now { vec![[0.0; 3]] } else { Vec::new() },
            phys_unknown: !made_now,
            transform: Some(r.pos),
            fuzzy: false,
            phys_fuzzy: false,
            radius: RadiusIv::exact(r.radius),
            radius_corp: None,
            centre: Some(r.radius),
            enabled: if r.enabled { Enabled::Yes } else { Enabled::No },
            history_unknown: false,
            replaced: 0,
            made_hidden: false,
            seen: None,
            rec_id: Some(r.id),
        }
    }
}

fn bits(v: V3) -> [u32; 3] {
    v.map(f32::to_bits)
}

impl Bounds {
    /// The window's start from `bounds_init`: the unit colliders in the order the scene lists
    /// them (the creation order). `units` are the sim's, for the update list's start state.
    pub fn start_recorded(&mut self, roots: &[RecCollider], units: &[BoundsUnit]) {
        assert!(!self.started, "the colliders are recorded at the window's start");
        self.started = true;
        self.recorded = true;
        for r in roots {
            let mut c = Collider::recorded(r, false);
            // The update list at the start: where the previous tick left the unit.
            c.seen = units.iter().find(|u| u.unit == r.unit).map(|u| Seen {
                pos: r.pos,
                in_game: u.in_game,
                // (What the sim counted before the first tick is its own set-up, not the game's
                // update list.)
                size_gen: u.size_gen,
                in_game_gen: u.in_game_gen,
            });
            self.colliders.push(c);
        }
    }

    /// The instance ids of a unit's colliders now (a destroyed one counts until the frame's end).
    #[must_use]
    pub fn collider_ids_of(&self, unit: EntityIdx) -> Vec<i32> {
        self.colliders.iter().filter(|c| c.unit == unit).filter_map(|c| c.rec_id).collect()
    }

    /// The units that have more than one collider now (a destroyed one counts until the frame's
    /// end): their colliders' places in the result order differ, which a unit-level order can't
    /// tell.
    #[must_use]
    pub fn units_with_several_colliders(&self) -> HashSet<EntityIdx> {
        let mut seen = HashSet::new();
        let mut out = HashSet::new();
        for c in self.colliders.iter().filter(|c| c.rec_id.is_some()) {
            if !seen.insert(c.unit) {
                out.insert(c.unit);
            }
        }
        out
    }

    /// Whether the colliders are the recording's ([`Bounds::start_recorded`]).
    #[must_use]
    pub fn is_recorded(&self) -> bool {
        self.recorded
    }

    fn index_of(&self, id: i32) -> Option<usize> {
        self.colliders.iter().position(|c| c.rec_id == Some(id))
    }

    /// One frame of `EntityBoundsController.Tick`, from the records, in the game's order: what
    /// the last frame's end destroyed is gone; the writes that came after the last query
    /// happened; the physics step the frame ran flushed the transforms; the writes before the
    /// bounds controller; its `TickEntity` calls (`ticks`, in order); a recorded sync flushes the
    /// transforms as they stand at its place among the calls (the stream's order). `units` are the sim's
    /// (empty for a frame it didn't follow): the update model is checked against the records.
    pub fn tick_recorded(
        &mut self,
        step: Step,
        events: &[Event],
        ticks: &[RecTick],
        units: &[BoundsUnit],
    ) -> RecOut {
        assert!(self.recorded, "tick_recorded needs start_recorded");
        let mut out = RecOut::default();
        for id in std::mem::take(&mut self.dying) {
            self.colliders.retain(|c| c.rec_id != Some(id));
        }
        for e in events.iter().filter(|e| e.late) {
            self.apply_write(e, &mut out);
        }
        self.take_step(step);
        // The writes before the frame's query and the bounds controller's calls, in the stream's
        // order (a sync flushes the transforms as they stand at its place in it).
        let mut items: Vec<(i64, Result<&Event, &RecTick>)> = events
            .iter()
            .filter(|e| !e.late)
            .map(|e| (e.seq, Ok(e)))
            .chain(ticks.iter().map(|t| (t.seq, Err(t))))
            .collect();
        items.sort_by_key(|(seq, _)| *seq);
        for (_, item) in items {
            match item {
                Ok(e) => self.apply_write(e, &mut out),
                Err(t) => self.apply_tick(t, units, &mut out),
            }
        }
        let ticked = |u: EntityIdx| ticks.iter().any(|t| t.unit == u);
        for u in units {
            self.check_unit(u, ticked(u.unit), &mut out);
        }
        out
    }

    /// The collider a view-owned operation names: one the records made and the view still holds.
    /// `DestroySphereBoundsCollider` clears the view's reference at once, so a collider destroyed
    /// earlier in the frame (still in the physics scene until the frame's end) is not one.
    fn view_collider(&self, id: i32, what: &str, out: &mut RecOut) -> Option<usize> {
        let Some(i) = self.index_of(id) else {
            out.problem(
                None,
                format!("{what} names collider {id}, which the sim has not seen made"),
            );
            return None;
        };
        if self.dying.contains(&id) {
            out.problem(
                Some(self.colliders[i].unit),
                format!("{what} names collider {id}, which was destroyed earlier in this frame (its view's reference is cleared)"),
            );
            return None;
        }
        Some(i)
    }

    /// The writes that came after the last frame's query, with no controller tick after them to
    /// check them: applied to the colliders as they stand, so one that contradicts them is reported.
    pub fn finish_recorded(&mut self, events: &[Event]) -> RecOut {
        let mut out = RecOut::default();
        for e in events {
            self.apply_write(e, &mut out);
        }
        out
    }

    fn apply_write(&mut self, e: &Event, out: &mut RecOut) {
        match e.write {
            Write::Sync => {
                out.counts.syncs += 1;
                // Every transform, as it stands here in the stream, reaches PhysX.
                for c in &mut self.colliders {
                    match c.transform {
                        Some(t) => {
                            c.phys = vec![t];
                            c.phys_unknown = false;
                            c.phys_fuzzy = false;
                        }
                        None => c.phys_unknown = true,
                    }
                }
            }
            Write::Active { id, on0, on } => {
                let Some(i) = self.view_collider(id, "bounds_set active", out) else { return };
                let c = &mut self.colliders[i];
                if (c.enabled == Enabled::Yes) != on0 {
                    out.problem(
                        Some(c.unit),
                        format!("bounds_set active: the collider was enabled {on0} before in the record, {:?} in the sim", c.enabled),
                    );
                }
                c.enabled = if on { Enabled::Yes } else { Enabled::No };
                out.counts.active += 1;
            }
            Write::Pos { id, pos0, pos } => {
                let Some(i) = self.view_collider(id, "bounds_set pos", out) else { return };
                let c = &mut self.colliders[i];
                if c.transform.map(bits) != Some(bits(pos0)) {
                    out.problem(
                        Some(c.unit),
                        format!("bounds_set pos: the transform was {pos0:?} before in the record, {:?} in the sim", c.transform),
                    );
                }
                c.transform = Some(pos);
                out.counts.moved += 1;
            }
            Write::Gone { id } => {
                let Some(i) = self.index_of(id) else {
                    out.problem(
                        None,
                        format!(
                            "bounds_set gone names collider {id}, which the sim has not seen made"
                        ),
                    );
                    return;
                };
                out.counts.gone += 1;
                if self.dying.contains(&id) {
                    out.problem(
                        Some(self.colliders[i].unit),
                        format!(
                            "bounds_set gone: collider {id} was already destroyed in this frame"
                        ),
                    );
                    return;
                }
                if e.late {
                    self.colliders.remove(i);
                } else {
                    // (The collider is in this frame's query; the frame's end destroys it.)
                    self.dying.push(id);
                }
            }
        }
    }

    fn apply_tick(&mut self, t: &RecTick, units: &[BoundsUnit], out: &mut RecOut) {
        let u = units.iter().find(|u| u.unit == t.unit);
        let (Some(r), Some(pos), Some(on), Some(id)) = (t.r, t.pos, t.on, t.id) else {
            // `TickEntity` returned early (the entity has no view) or the view has no collider.
            if t.made || t.r.is_some() || t.pos.is_some() || t.on.is_some() || t.id.is_some() {
                out.problem(Some(t.unit), "a bounds_tick with a collider only in part");
            } else if self
                .colliders
                .iter()
                .any(|c| c.unit == t.unit && c.rec_id.is_some_and(|id| !self.dying.contains(&id)))
            {
                out.problem(
                    Some(t.unit),
                    "a bounds_tick with no collider, but the unit's collider is made",
                );
            }
            return;
        };
        out.counts.ticks += 1;
        let existing = self.index_of(id);
        let prev = if t.made {
            // A unit has one view collider: the previous one must have been destroyed before the
            // call makes another. (A view replaced is a detach and an attach whose records the
            // sim doesn't tell apart from a unit's view being cleared: ambiguous, so reported.)
            let live = self
                .colliders
                .iter()
                .any(|c| c.unit == t.unit && c.rec_id.is_some_and(|id| !self.dying.contains(&id)));
            if live {
                out.problem(Some(t.unit), format!("a bounds_tick that made collider {id} while the unit's previous collider is live: no destruction before it"));
            }
            if existing.is_some() || t.r0.is_some() {
                out.problem(Some(t.unit), format!("a bounds_tick that made collider {id} when the sim has it or the record has a radius before"));
            }
            None
        } else {
            let Some(i) = existing else {
                out.problem(
                    Some(t.unit),
                    format!("a bounds_tick of collider {id}, which the sim has not seen made"),
                );
                self.colliders.push(Collider::recorded(
                    &RecCollider { unit: t.unit, id, radius: r, enabled: on, pos },
                    false,
                ));
                return;
            };
            if self.dying.contains(&id) {
                out.problem(
                    Some(t.unit),
                    format!("a bounds_tick of collider {id}, which was destroyed earlier in this frame (its view's reference is cleared)"),
                );
                return;
            }
            // `TickEntity` works on the unit's view collider: its newest live one.
            let current =
                self.colliders.iter().rev().find(|c| {
                    c.unit == t.unit && c.rec_id.is_some_and(|id| !self.dying.contains(&id))
                });
            if current.and_then(|c| c.rec_id) != Some(id) {
                out.problem(
                    Some(t.unit),
                    format!("a bounds_tick of collider {id}, which isn't the unit's current view collider"),
                );
            }
            let c = &self.colliders[i];
            if c.unit != t.unit {
                out.problem(
                    Some(t.unit),
                    format!(
                        "a bounds_tick names collider {id}, which the sim has as another unit's"
                    ),
                );
            }
            if t.r0.map(f32::to_bits) != Some(c.radius.lo.to_bits()) {
                out.problem(Some(t.unit), format!("a bounds_tick whose radius before ({:?}) isn't the one the last record left ({})", t.r0, c.radius.lo));
            }
            if (c.enabled == Enabled::Yes) != on {
                out.problem(Some(t.unit), format!("a bounds_tick whose `on` ({on}) isn't the collider's state in the sim ({:?})", c.enabled));
            }
            Some(c.radius.lo)
        };
        // The sim's rule against what the game left.
        if let Some(u) = u {
            let exp = next_radius(prev.map(RadiusIv::exact), u.corpulence);
            let slack = 1e-6;
            if r < exp.lo - slack || r > exp.hi + slack {
                out.problem(Some(t.unit), format!("a collider radius of {r} after a tick, outside the sim's {:?} to {:?} (corpulence {:?})", exp.lo, exp.hi, u.corpulence));
            } else if u.corpulence.is_none() {
                out.counts.radius_unchecked += 1;
            } else if u.corpulence.is_some_and(|c| r.to_bits() == c.max(0.05).to_bits()) {
                out.counts.radius_exact += 1;
            } else {
                out.counts.radius_band += 1;
            }
            if u.pos_known {
                let expected = match existing {
                    Some(i) => {
                        self.colliders[i].transform.map(|tr| orientation::set_position(tr, u.pos))
                    }
                    None => None,
                };
                let ok = expected
                    .map_or_else(|| orientation::vec3_eq(pos, u.pos), |e| bits(e) == bits(pos));
                if ok {
                    out.counts.pos_equal += 1;
                } else {
                    out.problem(Some(t.unit), format!("a collider transform of {pos:?} after a tick, the sim's rule gives {expected:?} for the unit at {:?}", u.pos));
                }
            } else {
                out.counts.pos_unchecked += 1;
            }
        }
        let rec = RecCollider { unit: t.unit, id, radius: r, enabled: on, pos };
        if t.made {
            out.counts.made += 1;
            let mut c = Collider::recorded(&rec, true);
            c.made_hidden = u.is_some_and(|u| !u.in_game);
            self.colliders.push(c);
            self.made_later += 1;
        } else if let Some(i) = existing {
            let c = &mut self.colliders[i];
            c.radius = RadiusIv::exact(r);
            c.centre = Some(r);
            c.transform = Some(pos);
            c.fuzzy = false;
        }
    }

    /// The update list and `enabled` against the sim's model, and the model's start state for the
    /// next frame.
    fn check_unit(&mut self, u: &BoundsUnit, ticked: bool, out: &mut RecOut) {
        let Some(c) =
            self.colliders.iter_mut().rev().find(|c| c.unit == u.unit && c.rec_id.is_some())
        else {
            return;
        };
        if u.in_game {
            c.made_hidden = false;
        }
        match (u.enabled, c.enabled) {
            (Enabled::Yes, Enabled::Yes) | (Enabled::No, Enabled::No) => {
                out.counts.enabled_checked += 1;
            }
            (Enabled::Unknown, _) => out.counts.enabled_unknown += 1,
            _ if c.made_hidden => out.counts.enabled_unknown += 1,
            (sim, rec) => out.problem(
                Some(u.unit),
                format!("the collider's `enabled` is {rec:?} in the record, the sim's unit is {sim:?} (in the game: {})", u.in_game),
            ),
        }
        let seen = Seen {
            pos: u.pos,
            in_game: u.in_game,
            size_gen: u.size_gen,
            in_game_gen: u.in_game_gen,
        };
        match (u.pos_known && u.in_game, c.seen) {
            (true, Some(s)) => {
                let listed = !s.in_game
                    || !orientation::vec3_eq(s.pos, u.pos)
                    || s.size_gen != u.size_gen
                    || s.in_game_gen != u.in_game_gen;
                match (listed, ticked) {
                    (true, true) | (false, false) => out.counts.listed_agree += 1,
                    (true, false) => {
                        out.counts.listed_sim_only += 1;
                        out.problem(Some(u.unit), "the sim's update list has the unit (it moved, changed size or came back) and the recording has no bounds_tick of it");
                    }
                    (false, true) => {
                        out.counts.listed_rec_only += 1;
                        out.problem(Some(u.unit), "a bounds_tick of a unit the sim's update list doesn't have (it didn't move, change size or come back)");
                    }
                }
            }
            // (Out of the game the update list takes nobody, and one queued before may still run.)
            _ => out.counts.listed_unchecked += 1,
        }
        c.seen = u.pos_known.then_some(seen);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::super::{Cause, Difference, Hit, Tri, by_unit, cap, differences};
    use super::*;

    const A: EntityIdx = EntityIdx(1);
    const B: EntityIdx = EntityIdx(2);
    const RAN: Step = Step::Ran;

    fn root(unit: EntityIdx, id: i32, radius: f32, pos: V3) -> RecCollider {
        RecCollider { unit, id, radius, enabled: true, pos }
    }

    fn su(unit: EntityIdx, pos: V3, corpulence: Option<f32>) -> BoundsUnit {
        BoundsUnit {
            unit,
            pos,
            pos_known: true,
            prefix: Some(pos),
            in_game: true,
            enabled: Enabled::Yes,
            corpulence,
            size_gen: 0,
            in_game_gen: 0,
        }
    }

    fn tick(unit: EntityIdx, id: i32, r0: f32, r: f32, pos: V3) -> RecTick {
        RecTick {
            seq: 0,
            unit,
            made: false,
            r0: Some(r0),
            r: Some(r),
            pos: Some(pos),
            on: Some(true),
            id: Some(id),
        }
    }

    /// The problems that mention `word`.
    fn about(out: &RecOut, word: &str) -> usize {
        out.problems.iter().filter(|(_, p)| p.contains(word)).count()
    }

    fn started(roots: &[RecCollider]) -> Bounds {
        let mut b = Bounds::default();
        b.start_recorded(roots, &[]);
        b
    }

    fn tris(b: &Bounds, at: V3, r: f32) -> Vec<(EntityIdx, Tri, Option<Cause>)> {
        b.overlap(at, r).iter().map(|h| (h.unit, h.tri, h.cause)).collect()
    }

    #[test]
    fn a_recorded_radius_is_exact_with_no_corpulence_and_no_scale_band() {
        // A radius 0.5 at 10.52: a capsule of 10 reaches 10.5, so it is out; the same collider
        // with the corpulence unknown and the view's scale unknown would be the recording's.
        let mut b = started(&[root(A, 7, 0.5, [10.52, 0.0, 0.0])]);
        b.tick_recorded(RAN, &[], &[], &[]);
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::Out, None)]);
        assert_eq!(tris(&b, [0.0; 3], 10.03), vec![(A, Tri::In, None)]);
        let h = b.overlap([0.0; 3], 10.0)[0];
        assert!(h.order_known && !h.lost);
        // The same unit as the sim makes it at its first tick, the corpulence unknown.
        let mut m = Bounds::default();
        m.tick(&[su(A, [10.52, 0.0, 0.0], None)], RAN);
        assert_eq!(m.overlap([0.0; 3], 10.0)[0].cause, Some(Cause::Corpulence));
    }

    #[test]
    fn the_first_step_flushes_the_start_transform_and_a_frame_without_one_leaves_it_unknown() {
        let mut b = started(&[root(A, 7, 0.5, [10.4, 0.0, 0.0])]);
        b.tick_recorded(Step::NotRan, &[], &[], &[]);
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::Maybe, Some(Cause::Position))]);
        let mut b = started(&[root(A, 7, 0.5, [10.4, 0.0, 0.0])]);
        // The unit moves in this frame's tick; PhysX has the transform of the frame's start.
        b.tick_recorded(RAN, &[], &[tick(A, 7, 0.5, 0.5, [20.0, 0.0, 0.0])], &[]);
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::In, None)]);
        // Next step: the new transform.
        b.tick_recorded(RAN, &[], &[], &[]);
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::Out, None)]);
        // No step: still the old one.
        let mut b = started(&[root(A, 7, 0.5, [10.4, 0.0, 0.0])]);
        b.tick_recorded(RAN, &[], &[tick(A, 7, 0.5, 0.5, [20.0, 0.0, 0.0])], &[]);
        b.tick_recorded(Step::NotRan, &[], &[], &[]);
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::In, None)]);
    }

    #[test]
    fn a_collider_made_by_a_tick_is_appended_and_at_the_origin_until_a_step() {
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        b.tick_recorded(RAN, &[], &[], &[]);
        let made = RecTick {
            seq: 0,
            unit: B,
            made: true,
            r0: None,
            r: Some(0.9),
            pos: Some([30.0, 0.0, 0.0]),
            on: Some(true),
            id: Some(8),
        };
        let out = b.tick_recorded(RAN, &[], &[made], &[]);
        assert!(out.problems.is_empty(), "{:?}", out.problems);
        assert_eq!(out.counts.made, 1);
        // At the origin for PhysX: in reach of a capsule at the origin.
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::In, None), (B, Tri::In, None)]);
        b.tick_recorded(RAN, &[], &[], &[]);
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::In, None), (B, Tri::Out, None)]);
        assert_eq!(b.made_later, 1);
        // A second making of the same collider contradicts the record.
        let again = b.tick_recorded(RAN, &[], &[made], &[]);
        assert!(!again.problems.is_empty());
    }

    #[test]
    fn writes_disable_move_and_destroy_at_the_right_time() {
        let off =
            Event { seq: 0, write: Write::Active { id: 7, on0: true, on: false }, late: false };
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        b.tick_recorded(RAN, &[], &[], &[]);
        // A late disable is seen by the next query only, an early one by this frame's.
        let out = b.tick_recorded(RAN, &[off], &[], &[]);
        assert!(out.problems.is_empty());
        assert!(tris(&b, [0.0; 3], 1.0).is_empty());
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        b.tick_recorded(RAN, &[], &[], &[]);
        assert_eq!(tris(&b, [0.0; 3], 1.0).len(), 1);
        b.tick_recorded(RAN, &[Event { late: true, ..off }], &[], &[]);
        assert!(tris(&b, [0.0; 3], 1.0).is_empty());
        // A disable of a collider that was already off contradicts the record.
        let again = b.tick_recorded(RAN, &[off], &[], &[]);
        assert_eq!(again.problems.len(), 1, "{:?}", again.problems);
        // A destruction before the query leaves the collider in it, and the frame's end takes it.
        let gone = Event { seq: 0, write: Write::Gone { id: 7 }, late: false };
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        b.tick_recorded(RAN, &[], &[], &[]);
        b.tick_recorded(RAN, &[gone], &[], &[]);
        assert_eq!(tris(&b, [0.0; 3], 1.0).len(), 1);
        b.tick_recorded(RAN, &[], &[], &[]);
        assert!(tris(&b, [0.0; 3], 1.0).is_empty());
        // A write to a collider nobody made is reported.
        let out = b.tick_recorded(RAN, &[off], &[], &[]);
        assert_eq!(out.problems.len(), 1);
        // A move outside the tick shows at the next step.
        let mv = Event {
            seq: 0,
            write: Write::Pos { id: 7, pos0: [0.0; 3], pos: [50.0, 0.0, 0.0] },
            late: false,
        };
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        b.tick_recorded(RAN, &[], &[], &[]);
        assert!(b.tick_recorded(RAN, &[mv], &[], &[]).problems.is_empty());
        b.tick_recorded(RAN, &[], &[], &[]);
        assert!(tris(&b, [0.0; 3], 1.0)[0].1 == Tri::Out);
    }

    #[test]
    fn a_recorded_sync_flushes_the_transforms_as_they_stand_at_its_place_in_the_stream() {
        let ev = |seq: i64, write: Write, late: bool| Event { seq, write, late };
        let at = |seq: i64, x: f32| RecTick { seq, ..tick(A, 7, 0.5, 0.5, [x, 0.0, 0.0]) };
        let moved = |b: &mut Bounds, events: &[Event], ticks: &[RecTick]| {
            let mut b0 = started(&[root(A, 7, 0.5, [10.4, 0.0, 0.0])]);
            b0.tick_recorded(RAN, &[], &[], &[]);
            std::mem::swap(b, &mut b0);
            b.tick_recorded(Step::NotRan, events, ticks, &[])
        };
        // No step this frame. A sync before the frame's tick flushes the old transform, one
        // after it the new.
        let mut b = Bounds::default();
        let out = moved(&mut b, &[ev(1, Write::Sync, false)], &[at(2, 20.0)]);
        assert_eq!(out.counts.syncs, 1);
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::In, None)]);
        let mut b = Bounds::default();
        moved(&mut b, &[ev(3, Write::Sync, false)], &[at(2, 20.0)]);
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::Out, None)]);
        // Without the sync the frame with no step keeps the older copy.
        let mut b = Bounds::default();
        moved(&mut b, &[], &[at(2, 20.0)]);
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::In, None)]);
        // Sol's round 1: a write A to B, a sync, a write B to C: PhysX has B, and nothing is
        // decided as though it had A or C.
        let mut b = Bounds::default();
        let pos = |seq, pos0: f32, pos: f32| {
            ev(seq, Write::Pos { id: 7, pos0: [pos0, 0.0, 0.0], pos: [pos, 0.0, 0.0] }, false)
        };
        let out = moved(
            &mut b,
            &[pos(1, 10.4, 30.0), ev(2, Write::Sync, false), pos(3, 30.0, 50.0)],
            &[],
        );
        assert!(out.problems.is_empty(), "{:?}", out.problems);
        assert_eq!(tris(&b, [30.0, 0.0, 0.0], 1.0), vec![(A, Tri::In, None)]);
        assert_eq!(tris(&b, [10.0, 0.0, 0.0], 1.0), vec![(A, Tri::Out, None)]);
        assert_eq!(tris(&b, [50.0, 0.0, 0.0], 1.0), vec![(A, Tri::Out, None)]);
        // After the query (late) it is the transform as it stands.
        let mut b = Bounds::default();
        moved(&mut b, &[], &[at(2, 20.0)]);
        b.tick_recorded(Step::NotRan, &[ev(4, Write::Sync, true)], &[], &[]);
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::Out, None)]);
    }

    #[test]
    fn a_collider_is_destroyed_once() {
        let gone = |seq, late| Event { seq, write: Write::Gone { id: 7 }, late };
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        b.tick_recorded(RAN, &[], &[], &[]);
        let out = b.tick_recorded(RAN, &[gone(1, false), gone(2, false)], &[], &[]);
        assert_eq!(about(&out, "already destroyed"), 1, "{:?}", out.problems);
        // The frame's end took it: a destruction in the next frame names nothing.
        let out = b.tick_recorded(RAN, &[gone(3, true)], &[], &[]);
        assert_eq!(out.problems.len(), 1, "{:?}", out.problems);
    }

    #[test]
    fn writes_after_the_last_query_are_checked_at_the_end() {
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        b.tick_recorded(RAN, &[], &[], &[]);
        let off =
            |id| Event { seq: 1, write: Write::Active { id, on0: true, on: false }, late: true };
        assert!(b.finish_recorded(&[off(7)]).problems.is_empty());
        assert_eq!(b.finish_recorded(&[off(9)]).problems.len(), 1);
    }

    #[test]
    fn the_sims_rules_are_checked_against_the_records() {
        // The radius: the sim's rule from 0.5 with the corpulence 0.9 gives 0.9 (not 0.5).
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        let ok = b.tick_recorded(
            RAN,
            &[],
            &[tick(A, 7, 0.5, 0.9, [0.0; 3])],
            &[su(A, [0.0; 3], Some(0.9))],
        );
        assert!(ok.problems.is_empty(), "{:?}", ok.problems);
        assert_eq!((ok.counts.radius_exact, ok.counts.pos_equal), (1, 1));
        let bad = b.tick_recorded(
            RAN,
            &[],
            &[tick(A, 7, 0.9, 0.5, [0.0; 3])],
            &[su(A, [0.0; 3], Some(0.9))],
        );
        assert_eq!(about(&bad, "radius"), 1, "{:?}", bad.problems);
        // A radius before that isn't the one the last record left.
        let bad = b.tick_recorded(RAN, &[], &[tick(A, 7, 0.4, 0.5, [0.0; 3])], &[]);
        assert_eq!(about(&bad, "radius before"), 1, "{:?}", bad.problems);
        // The transform: the unit is at 5, the record says it stayed at 0.
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        let bad = b.tick_recorded(
            RAN,
            &[],
            &[tick(A, 7, 0.5, 0.5, [0.0; 3])],
            &[su(A, [5.0, 0.0, 0.0], Some(0.5))],
        );
        assert_eq!(about(&bad, "transform"), 1, "{:?}", bad.problems);
        // The update list: a unit that moved without a tick is the sim's only; the collider's
        // enabled flag against the unit's state.
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        let mut a = su(A, [0.0; 3], Some(0.5));
        b.start_check(&[a]);
        a.pos = [3.0, 0.0, 0.0];
        let out = b.tick_recorded(RAN, &[], &[], &[a]);
        assert_eq!((out.counts.listed_sim_only, out.counts.listed_agree), (1, 0));
        assert_eq!(out.problems.len(), 1, "{:?}", out.problems);
        // The same unit with a tick for its move agrees; a tick of a unit that stood still doesn't.
        let mut b2 = started(&[root(A, 7, 0.5, [0.0; 3])]);
        b2.start_check(&[su(A, [0.0; 3], Some(0.5))]);
        let out = b2.tick_recorded(
            RAN,
            &[],
            &[tick(A, 7, 0.5, 0.5, [3.0, 0.0, 0.0])],
            &[su(A, [3.0, 0.0, 0.0], Some(0.5))],
        );
        assert_eq!((out.counts.listed_agree, out.problems.len()), (1, 0), "{:?}", out.problems);
        let out = b2.tick_recorded(
            RAN,
            &[],
            &[tick(A, 7, 0.5, 0.5, [3.0, 0.0, 0.0])],
            &[su(A, [3.0, 0.0, 0.0], Some(0.5))],
        );
        assert_eq!((out.counts.listed_rec_only, out.problems.len()), (1, 1));
        a.in_game = false;
        a.enabled = Enabled::No;
        let out = b.tick_recorded(RAN, &[], &[], &[a]);
        assert_eq!(out.problems.len(), 1, "{:?}", out.problems);
        // A fade the sim saw begin explains a collider that stays on.
        a.enabled = Enabled::Unknown;
        let out = b.tick_recorded(RAN, &[], &[], &[a]);
        assert!(out.problems.is_empty());
        assert_eq!(out.counts.enabled_unknown, 1);
    }

    fn gone(seq: i64, id: i32, late: bool) -> Event {
        Event { seq, write: Write::Gone { id }, late }
    }

    #[test]
    fn a_destroyed_collider_takes_no_view_operations_for_the_rest_of_the_frame() {
        // Sol's round 2: the view's reference is cleared at once; the physics collider stays
        // queryable until the frame's end.
        let pos = Event {
            seq: 2,
            write: Write::Pos { id: 7, pos0: [0.0; 3], pos: [5.0, 0.0, 0.0] },
            late: false,
        };
        let off =
            Event { seq: 2, write: Write::Active { id: 7, on0: true, on: false }, late: false };
        for write in [pos, off] {
            let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
            b.tick_recorded(RAN, &[], &[], &[]);
            let out = b.tick_recorded(RAN, &[gone(1, 7, false), write], &[], &[]);
            assert_eq!(about(&out, "destroyed earlier in this frame"), 1, "{:?}", out.problems);
            assert_eq!(tris(&b, [0.0; 3], 1.0).len(), 1);
        }
        // A call of it (not making) too, and the writes queued after the last query.
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        b.tick_recorded(RAN, &[], &[], &[]);
        let call = RecTick { seq: 2, ..tick(A, 7, 0.5, 0.5, [0.0; 3]) };
        let out = b.tick_recorded(RAN, &[gone(1, 7, false)], &[call], &[]);
        assert_eq!(about(&out, "destroyed earlier in this frame"), 1, "{:?}", out.problems);
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        b.tick_recorded(RAN, &[gone(1, 7, false)], &[], &[]);
        let out = b.finish_recorded(&[Event { late: true, ..pos }]);
        assert_eq!(about(&out, "destroyed earlier in this frame"), 1, "{:?}", out.problems);
    }

    #[test]
    fn a_unit_has_one_view_collider_until_it_is_destroyed() {
        let make = |seq: i64, id: i32| RecTick {
            seq,
            unit: A,
            made: true,
            r0: None,
            r: Some(0.5),
            pos: Some([0.0; 3]),
            on: Some(true),
            id: Some(id),
        };
        // A call that makes a second collider while the first is live: reported (a view
        // replaced is ambiguous in the records, and no permission is taken from it).
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        b.tick_recorded(RAN, &[], &[], &[]);
        let out = b.tick_recorded(RAN, &[], &[make(1, 8)], &[]);
        assert_eq!(about(&out, "previous collider is live"), 1, "{:?}", out.problems);
        // Sol's round 3: a later call on the obsolete collider isn't the view's.
        let old = RecTick { seq: 2, ..tick(A, 7, 0.5, 0.5, [0.0; 3]) };
        let out = b.tick_recorded(RAN, &[], &[old], &[]);
        assert_eq!(about(&out, "current view collider"), 1, "{:?}", out.problems);
        assert_eq!(b.collider_ids_of(A), vec![7, 8]);
        assert_eq!(b.units_with_several_colliders(), HashSet::from([A]));
        // After a destruction (the same frame or an earlier one) it is the unit's next collider.
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        b.tick_recorded(RAN, &[], &[], &[]);
        let out = b.tick_recorded(RAN, &[gone(1, 7, false)], &[make(2, 8)], &[]);
        assert!(out.problems.is_empty(), "{:?}", out.problems);
        // (The destroyed one counts until the frame's end.)
        assert_eq!(b.units_with_several_colliders(), HashSet::from([A]));
        b.tick_recorded(RAN, &[], &[], &[]);
        assert!(b.units_with_several_colliders().is_empty());
        let mut b = started(&[root(A, 7, 0.5, [0.0; 3])]);
        b.tick_recorded(RAN, &[], &[], &[]);
        b.tick_recorded(RAN, &[gone(1, 7, true)], &[], &[]);
        assert!(b.tick_recorded(RAN, &[], &[make(2, 8)], &[]).problems.is_empty());
    }

    #[test]
    fn the_buffer_allowance_counts_only_duplicates_that_may_be_hit() {
        // Sol's round 3: 513 surely-in units, so the buffer is full and holds 512 distinct
        // units. A recorded set of 511 is impossible, and a duplicate collider that is surely
        // out (or of a unit not in the set) can't change that.
        let hit = |u: u32, tri: Tri| Hit {
            unit: EntityIdx(u),
            tri,
            cause: None,
            margin: None,
            rounding: false,
            guess: None,
            order_known: true,
            capped: None,
            extra_slots: 0,
            lost: false,
        };
        let mut hits: Vec<Hit> = (0..513).map(|i| hit(i, Tri::In)).collect();
        hits.push(hit(0, Tri::Out));
        cap(&mut hits);
        let set: HashSet<EntityIdx> = (1..512).map(EntityIdx).collect();
        let d = differences(&hits, &set);
        assert!(d.diffs.contains(&Difference::Missing(EntityIdx(512))), "{:?}", d.diffs);
        // A duplicate that may be hit (and whose unit is in the set) can take a slot: 511 units
        // and one more collider fill the buffer.
        let mut hits: Vec<Hit> = (0..513).map(|i| hit(i, Tri::In)).collect();
        hits.push(hit(1, Tri::In));
        cap(&mut hits);
        let d = differences(&hits, &set);
        assert!(d.diffs.is_empty(), "{:?}", d.diffs);
    }

    #[test]
    fn a_unit_with_two_colliders_is_in_the_set_if_either_is_hit() {
        // Sol's round 2: the old collider (destroyed this frame) is out of reach, the new one in.
        let mut b = started(&[root(A, 7, 0.5, [50.0, 0.0, 0.0])]);
        b.tick_recorded(RAN, &[], &[], &[]);
        let made = RecTick {
            seq: 2,
            unit: A,
            made: true,
            r0: None,
            r: Some(0.5),
            pos: Some([0.0; 3]),
            on: Some(true),
            id: Some(8),
        };
        b.tick_recorded(RAN, &[gone(1, 7, false)], &[made], &[]);
        // (Both are in this frame's query; the new one is made at the origin.)
        let hits = b.overlap([0.0; 3], 1.0);
        assert_eq!(hits.len(), 2);
        let set: HashSet<EntityIdx> = [A].into_iter().collect();
        let d = differences(&hits, &set);
        assert!(d.diffs.is_empty() && d.decided == 1, "{d:?}");
        let d = differences(&hits, &HashSet::new());
        assert_eq!(d.diffs, vec![Difference::Missing(A)]);
        // Far from both: out, and a set holding it is an extra.
        let d = differences(&b.overlap([200.0, 0.0, 0.0], 1.0), &set);
        assert_eq!(d.diffs, vec![Difference::Extra(A)]);
        // The enumeration holds the unit once, at its first hit.
        assert_eq!(by_unit(&hits).len(), 1);
    }

    impl Bounds {
        /// (Test helper: the update list's start state from units, as `start_recorded` takes it.)
        fn start_check(&mut self, units: &[BoundsUnit]) {
            for c in &mut self.colliders {
                c.seen = units.iter().find(|u| u.unit == c.unit).map(|u| Seen {
                    pos: c.transform.unwrap_or_default(),
                    in_game: u.in_game,
                    size_gen: 0,
                    in_game_gen: 0,
                });
            }
        }
    }
}
