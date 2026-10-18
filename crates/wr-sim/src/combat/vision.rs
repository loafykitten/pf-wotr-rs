//! Line of sight between units (decompiled, not copied): `UnitSightCache`,
//! `UnitLineOfSightCacheController`, `UnitEntityData.HasLOS` and `CheckLOS`, over
//! `LineOfSightGeometry` (`wr-los`). docs/poc5/vision.md.
//!
//! A unit's sight cache answers `HasLOS(other)` from its entry for `other` while `other` is within
//! 0.5 m (on the ground plane) of where it stood when the entry was made; a miss computes
//! `CheckLOS(other.Position + EyeShift, sample)` from the position the cache last sampled (the
//! owner's, at its last `Clear`). The cache controller clears a cache whose owner moved 0.5 m from
//! its sample, or whose vision radius changed (or the geometry was updated: not modelled, a recorded
//! `los_geometry` is refused). So an answer depends on the history of every cache, not only on
//! the positions now.
//!
//! What a recording that opens mid-session doesn't show is each cache's state at its first record:
//! an entry made before the window, its position, and the sample. WrathOracle records every miss
//! (`los`: owner, the target's eye point, the sample, the result) and every clear (`los_clear`).
//! So a cache the window hasn't cleared yet is *unknown*: its entries made before the window
//! (their values and positions) are, and a read it answers from one is a hit whose value the
//! recording gives (taken, counted apart; the fresh computation is compared with it). The first
//! clear makes a cache known: from then on every hit and miss is the sim's, and each miss must be
//! the next recorded one.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use wr_los::Geometry;
use wr_nav::cs::{d, r};

use crate::world::{EntityIdx, World};

pub mod range;

pub type V3 = [f32; 3];

/// `LineOfSightGeometry.EyeShift`: `Vector3.up`.
pub const EYE: V3 = [0.0, 1.0, 0.0];

/// `UnitSightCache.MaxPositionError2`.
const MAX_POSITION_ERROR2: f32 = 0.25;

/// What the model reads from the world.
pub trait VisionWorld {
    /// `UnitEntityData.Position`, as of the read.
    fn position(&self, u: EntityIdx) -> V3;
    /// `UnitEntityData.VisionRangeMeters`, as of the read.
    fn radius(&self, u: EntityIdx) -> f32;
    /// `UnitEntityData.Blueprint.IsFake`.
    fn is_fake(&self, u: EntityIdx) -> bool;
    /// Every unit (to tell whom a miss was about).
    fn units(&self) -> Vec<EntityIdx>;
}

/// One sight cache miss as WrathOracle records it (`los`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Miss {
    pub owner: EntityIdx,
    /// The target's eye point (`other.Position + EyeShift`).
    pub point: V3,
    /// The owner's sample plus nothing: `m_LastSamplePosition`.
    pub sample: V3,
    pub result: bool,
}

/// An entry of a sight cache.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Where the target stood; `None`: made before the window (not known).
    pub pos: Option<V3>,
    /// `HasLOS`; `None`: made before the window and not read since.
    pub has_los: Option<bool>,
    /// An entry made before the window: where the target stood at each hit on it. The cached
    /// position is within 0.5 m of every one, so no two are 1 m apart (a necessary condition).
    pub hits: Vec<V3>,
}

impl Entry {
    fn pre() -> Self {
        Self { pos: None, has_los: None, hits: Vec::new() }
    }
}

/// `UnitSightCache`.
#[derive(Debug, Clone, Default)]
pub struct SightCache {
    /// Cleared in the window: every entry and the sample are the sim's.
    pub known: bool,
    /// `m_LastSamplePosition` (`None`: not known yet).
    pub sample: Option<V3>,
    /// `m_LastVisionRadius`.
    pub radius: Option<f32>,
    /// A cache not known yet: its owner's last controller tick could tell it didn't clear (the
    /// sample and the radius were known then and neither moved), so a recorded clear is wrong.
    pub decided: bool,
    pub entries: HashMap<EntityIdx, Entry>,
}

/// What `HasLOS` answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Los {
    /// The sim's answer.
    Value(bool),
    /// A hit on an entry made before the window whose value isn't known: the recording gives it.
    Pre,
}

/// Counters for the report.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counts {
    /// Misses computed and equal to the next recorded one.
    pub misses: usize,
    /// Hits on the sim's own entries (known caches).
    pub hits: usize,
    /// Hits on entries made before the window (the value is the recording's).
    pub pre_hits: usize,
    /// Of those, the fresh computation's value equal / different.
    pub pre_fresh_equal: usize,
    pub pre_fresh_different: usize,
    /// Reads not asked of a cache (the target out of vision range: `CanBeInVisionRange`).
    pub out_of_range: usize,
    /// Clears the sim predicted and the recording showed.
    pub clears: usize,
    /// Clears of a cache not known yet (the recording's: it becomes known).
    pub adopted_clears: usize,
    /// Misses by readers the sim doesn't run, verified and adopted.
    pub unmodelled_fills: usize,
    /// Answers of `HasObstacle` (the AI's `InLos`, the movement simulation's) computed and equal.
    pub obstacle_reads: usize,
}

/// The sim's line of sight.
#[derive(Debug, Default)]
pub struct Vision {
    /// The geometry, once loaded (`LineOfSightGeometry.Init`): `None` leaves every read an input.
    pub geometry: Option<Rc<Geometry>>,
    /// Each unit's sight cache, created on the first read or clear.
    pub caches: HashMap<EntityIdx, SightCache>,
    /// `UnitEntityData.CanBeInVisionRange`, per unit (`UnitVisionRangeController`'s sets).
    pub near: HashMap<EntityIdx, HashSet<EntityIdx>>,
    /// `CanBeInVisionRange`'s enumeration order (the set is refilled in the overlap's order each
    /// tick), for the units whose set the sim computed this way.
    pub near_order: HashMap<EntityIdx, Vec<EntityIdx>>,
    /// Units whose set holds a collider of unknown place in the order: `near_order` isn't exact.
    pub near_inexact: HashSet<EntityIdx>,
    /// `EntityBoundsController`'s sphere colliders (the overlap behind `near`).
    pub bounds: range::Bounds,
    /// Records (stream indices) of the events that could change a unit's `Corpulence` (a size
    /// write, a fact added or removed, a view replaced), per unit: a `corp` read is valid only
    /// after the last of them.
    pub corp_bumps: HashMap<EntityIdx, Vec<usize>>,
    /// Every event noted in `corp_bumps`, in order (the unit, its place in the unit's list).
    pub bump_log: Vec<(EntityIdx, usize)>,
    /// Per unit, how many size writes (`UnitState.Size`'s setter, which raises
    /// `HandleUnitSizeChanged` and puts the entity on the bounds controller's update list) or
    /// view replacements the sim has seen.
    pub size_gen: HashMap<EntityIdx, u64>,
    /// Per unit, how many changes of `IsInGame` the sim has seen (a return puts the entity on the
    /// bounds controller's update list, even when it left and came back between two ticks).
    pub in_game_gen: HashMap<EntityIdx, u64>,
    /// Per unit, how many writes of its position the sim made (its own movers: the agent,
    /// roaming, spawns, view replacement, companions placed); without the driven agent a write
    /// after the vision phase means the next prefix's row isn't where the unit stood at it.
    pub pos_writes: HashMap<EntityIdx, u64>,
    /// Units whose collider may be fade-hiding with its `enabled` kept: a `HideUnit` with `Fade`
    /// hid them, or they were not in the game at the window's first tick (a fade may have begun
    /// before). While the fade runs (`EntityViewBase.m_IsFadeHide`) `UpdateViewActive` returns
    /// early and the collider keeps its `enabled`; when the fade ends isn't modelled. Until the
    /// sim sees them in the game again their pairs in reach are the recording's.
    pub fade_hidden: HashSet<EntityIdx>,
    /// Each awake unit's vision radius at the frame's cache phase (a miss adopted later in the
    /// frame may have been made before a unit joined combat).
    pub frame_radius: HashMap<EntityIdx, f32>,
    /// The recorded misses not yet explained by a read, in order.
    pub pending: VecDeque<Miss>,
    /// The clears the sim predicted in this frame's cache phase and the recording hasn't shown.
    pub clears: VecDeque<(EntityIdx, V3)>,
    pub counts: Counts,
}

/// Why a read can't be answered (the replay reports it and takes the recording's answer).
pub type VisError = String;

/// `GeometryUtils.SqrDistance2D(a, b)`.
#[must_use]
pub fn sqr_distance_2d(a: V3, b: V3) -> f32 {
    let num = a[0] - b[0];
    let num2 = a[2] - b[2];
    r(d(num) * d(num) + d(num2) * d(num2))
}

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `UnitEntityData.CheckLOS(point, overridePosition, out hasLOS, out hasLOSWithoutDistance)` for
/// a unit without an `ExtendedVisionArea`: `(hasLOS, hasLOSWithoutDistance)`.
///
/// # Errors
///
/// A segment the geometry can't walk.
pub fn check_los(
    geometry: &Geometry,
    radius: f32,
    override_position: V3,
    point: V3,
) -> Result<(bool, bool), VisError> {
    let p1 = add(override_position, EYE);
    let flag = f64::from(sqr_distance_2d(p1, point)) <= d(radius) * d(radius);
    let flag2 = geometry.has_obstacle(p1, point, 0.0).map_err(|e| e.to_string())?;
    Ok((flag && !flag2, !flag2))
}

/// A hit on an entry made before the window, with the target at `there`: the cached position is
/// within 0.5 m of every position the target has had at a hit, so two 1 m apart can't both be.
fn note_hit(e: &mut Entry, there: V3) -> Result<(), VisError> {
    // (The game's squared distances are rounded `float`s: the bound has a margin of 0.001.)
    if let Some(far) = e.hits.iter().find(|h| sqr_distance_2d(**h, there) >= 1.001) {
        return Err(format!(
            "hits on one cache entry with the target at {far:?} and at {there:?}: the cache would have missed"
        ));
    }
    if !e.hits.iter().any(|h| bits(*h) == bits(there)) {
        e.hits.push(there);
    }
    Ok(())
}

fn bits(a: V3) -> [u32; 3] {
    a.map(f32::to_bits)
}

impl Vision {
    /// Whether the sim computes line of sight (the geometry is loaded).
    #[must_use]
    pub fn on(&self) -> bool {
        self.geometry.is_some()
    }

    /// `owner.CanBeInVisionRange.Contains(other)`.
    #[must_use]
    pub fn in_vision_range(&self, owner: EntityIdx, other: EntityIdx) -> bool {
        self.near.get(&owner).is_some_and(|s| s.contains(&other))
    }

    fn geometry(&self) -> Result<Rc<Geometry>, VisError> {
        self.geometry.clone().ok_or_else(|| "no line-of-sight geometry".to_owned())
    }

    /// The front recorded miss, when it is `owner`'s read of the eye point `point`.
    fn front_is(&self, owner: EntityIdx, point: V3) -> bool {
        self.pending.front().is_some_and(|m| m.owner == owner && bits(m.point) == bits(point))
    }

    /// `UnitSightCache.ClearIfNeeded()` (`UnitLineOfSightCacheController.Tick`) for a cache the
    /// window has cleared: the owner moved 0.5 m from the sample, or the vision radius changed
    /// (the geometry's `LastUpdateTime` doesn't move in a window: a recorded update is refused).
    /// Clears it and returns the position it clears at (the owner's, what WrathOracle's
    /// `los_clear` writes). A cache not known yet isn't decided here (the recording's clear is).
    pub fn clear_if_needed(&mut self, vw: &dyn VisionWorld, u: EntityIdx) -> Option<V3> {
        let c = self.caches.entry(u).or_default();
        let pos = vw.position(u);
        let radius = vw.radius(u);
        let moved = c.sample.is_some_and(|s| sqr_distance_2d(pos, s) >= MAX_POSITION_ERROR2);
        // A cache not known yet is decided only by the owner's movement from a sample a recorded
        // miss gave (its vision radius and its geometry stamp before the window aren't known).
        // `m_LastVisionRadius` of a cache not cleared in the window is unknown until a controller
        // tick leaves it: it then equals the owner's radius.
        let radius_changed = c.radius.is_some_and(|r| r != radius);
        c.decided = !c.known && c.sample.is_some() && c.radius.is_some();
        if moved || radius_changed {
            c.entries.clear();
            c.sample = Some(pos);
            c.radius = Some(radius);
            c.known = true;
            return Some(pos);
        }
        c.radius.get_or_insert(radius);
        None
    }

    /// A recorded `los_clear` of a cache the window hasn't cleared before: it clears at `at` (the
    /// owner's position, checked) and is known from now on.
    ///
    /// # Errors
    ///
    /// The position isn't the owner's.
    pub fn adopt_clear(
        &mut self,
        vw: &dyn VisionWorld,
        u: EntityIdx,
        at: V3,
    ) -> Result<(), VisError> {
        if bits(at) != bits(vw.position(u)) {
            return Err(format!(
                "a recorded clear at {at:?}, the owner stands at {:?}",
                vw.position(u)
            ));
        }
        let radius = vw.radius(u);
        let c = self.caches.entry(u).or_default();
        if c.decided {
            return Err(format!(
                "a recorded clear of {u:?}, whose sample and radius say it doesn't clear"
            ));
        }
        c.entries.clear();
        c.sample = Some(at);
        c.radius = Some(radius);
        c.known = true;
        self.counts.adopted_clears += 1;
        Ok(())
    }

    /// `owner.HasLOS(other)`: `(Blueprint.IsFake || CanBeInVisionRange.Contains(other)) &&
    /// SightCache.GetLOS(other)`. The recorded misses explain the misses.
    ///
    /// # Errors
    ///
    /// The read disagrees with the recorded misses, or the geometry can't answer.
    pub fn has_los(
        &mut self,
        vw: &dyn VisionWorld,
        owner: EntityIdx,
        other: EntityIdx,
    ) -> Result<Los, VisError> {
        if !(vw.is_fake(owner) || self.in_vision_range(owner, other)) {
            // (No cache is asked: a miss recorded for this pair would be unexplained.)
            self.counts.out_of_range += 1;
            return Ok(Los::Value(false));
        }
        self.get_los(vw, owner, other)
    }

    /// `UnitSightCache.GetLOS(other)`.
    fn get_los(
        &mut self,
        vw: &dyn VisionWorld,
        owner: EntityIdx,
        other: EntityIdx,
    ) -> Result<Los, VisError> {
        let geometry = self.geometry()?;
        let there = vw.position(other);
        let eye = add(there, EYE);
        // Misses recorded before this read's, of readers the sim doesn't run, were made (and
        // filled their entries) first.
        self.settle(vw, owner, eye)?;
        let front = self.front_is(owner, eye);
        let c = self.caches.entry(owner).or_default();
        // A cached answer: an entry within 0.5 m of where the target stands.
        match c.entries.get_mut(&other) {
            Some(Entry { pos: Some(p), has_los, .. })
                if sqr_distance_2d(*p, there) < MAX_POSITION_ERROR2 =>
            {
                if front {
                    return Err(format!(
                        "the sim answers {owner:?}'s read of {other:?} from its cache, the game missed"
                    ));
                }
                let has_los = *has_los;
                self.counts.hits += 1;
                return Ok(match has_los {
                    Some(v) => Los::Value(v),
                    None => Los::Pre,
                });
            }
            Some(e @ Entry { pos: None, .. }) if !front => {
                // An entry made before the window, and no miss recorded: the game hit it.
                note_hit(e, there)?;
                let has_los = e.has_los;
                self.counts.pre_hits += 1;
                return Ok(match has_los {
                    Some(v) => Los::Value(v),
                    None => Los::Pre,
                });
            }
            None if !c.known && !front => {
                // No entry the sim made, no miss recorded: the game hit an entry made before the
                // window, which the sim doesn't have.
                let mut e = Entry::pre();
                note_hit(&mut e, there)?;
                c.entries.insert(other, e);
                self.counts.pre_hits += 1;
                return Ok(Los::Pre);
            }
            _ => {}
        }
        // A miss: `m_LastSamplePosition`, then `CheckLOS`. An entry made before the window that a
        // hit with the target exactly here answered can't have missed here.
        if let Some(e) = self.caches.get(&owner).and_then(|c| c.entries.get(&other))
            && e.pos.is_none()
            && e.hits.iter().any(|h| sqr_distance_2d(*h, there) == 0.0)
        {
            return Err(format!("{owner:?} missed on {other:?} at a position its cache hit at"));
        }
        let rec = self.pending.front().copied().filter(|_| front);
        let c = self.caches.entry(owner).or_default();
        if !c.known && rec.is_none() {
            return Err(format!("{owner:?}'s read of {other:?} misses, no miss recorded"));
        }
        let sample = match (c.sample, rec) {
            (Some(s), _) => s,
            // `if (!m_HasPosition) { m_LastSamplePosition = m_Owner.Position }` for a cache the
            // window made; a cache from before it has its sample in the record.
            (None, Some(m)) if !c.known => m.sample,
            (None, _) => vw.position(owner),
        };
        let radius = vw.radius(owner);
        let (has_los, _) = check_los(&geometry, radius, sample, eye)?;
        let Some(m) = rec else {
            return Err(format!("the sim misses {owner:?}'s read of {other:?}, none was recorded"));
        };
        if bits(m.sample) != bits(sample) || m.result != has_los {
            return Err(format!(
                "the sim's miss of {owner:?} on {other:?} (sample {sample:?}, {has_los}) differs from the recorded one ({:?}, {})",
                m.sample, m.result
            ));
        }
        self.pending.pop_front();
        let c = self.caches.entry(owner).or_default();
        if c.sample.is_none() {
            c.sample = Some(sample);
        }
        c.entries
            .insert(other, Entry { pos: Some(there), has_los: Some(has_los), hits: Vec::new() });
        self.counts.misses += 1;
        Ok(Los::Value(has_los))
    }

    /// The value of a hit on an entry made before the window, from the recording (a unit's own
    /// read says it): kept in the entry, and compared with the fresh computation.
    ///
    /// # Errors
    ///
    /// A value contradicting one already known.
    pub fn resolve_pre(
        &mut self,
        vw: &dyn VisionWorld,
        owner: EntityIdx,
        other: EntityIdx,
        v: bool,
    ) -> Result<(), VisError> {
        let geometry = self.geometry()?;
        let eye = add(vw.position(other), EYE);
        let c = self.caches.entry(owner).or_default();
        let sample = c.sample.unwrap_or_else(|| vw.position(owner));
        let fresh = check_los(&geometry, vw.radius(owner), sample, eye)?.0;
        let c = self.caches.entry(owner).or_default();
        let e = c.entries.entry(other).or_insert_with(Entry::pre);
        if e.has_los.is_some_and(|old| old != v) {
            return Err(format!(
                "{owner:?}'s entry for {other:?} was {:?}, the recording has {v}",
                e.has_los
            ));
        }
        if e.has_los.is_none() {
            if fresh == v {
                self.counts.pre_fresh_equal += 1;
            } else {
                self.counts.pre_fresh_different += 1;
            }
        }
        e.has_los = Some(v);
        Ok(())
    }

    /// Adopts the recorded misses ahead of the miss the read by `owner` of the eye point `eye`
    /// made (they were made first, by readers the sim doesn't run). A read that made none leaves
    /// them: they may be a later read's of the same aggregate, or a frame end's.
    fn settle(&mut self, vw: &dyn VisionWorld, owner: EntityIdx, eye: V3) -> Result<(), VisError> {
        let ahead = self
            .pending
            .iter()
            .position(|m| m.owner == owner && bits(m.point) == bits(eye))
            .unwrap_or(0);
        for _ in 0..ahead {
            let Some(m) = self.pending.pop_front() else { break };
            self.adopt_miss(vw, m)?;
        }
        Ok(())
    }

    /// What a miss would answer now: `CheckLOS` from the cache's sample (the owner's position if
    /// the cache's isn't known) to `other`. For the comparison with an answer taken from the
    /// recording (an entry made before the window); not an answer.
    ///
    /// # Errors
    ///
    /// No geometry, or a segment it can't walk.
    pub fn fresh(
        &self,
        vw: &dyn VisionWorld,
        owner: EntityIdx,
        other: EntityIdx,
    ) -> Result<bool, VisError> {
        let geometry = self.geometry()?;
        let eye = add(vw.position(other), EYE);
        let sample =
            self.caches.get(&owner).and_then(|c| c.sample).unwrap_or_else(|| vw.position(owner));
        Ok(check_los(&geometry, vw.radius(owner), sample, eye)?.0)
    }

    /// Notes whether a fresh computation agreed with an answer the recording gave.
    pub fn note_fresh(&mut self, equal: bool) {
        if equal {
            self.counts.pre_fresh_equal += 1;
        } else {
            self.counts.pre_fresh_different += 1;
        }
    }

    /// A recorded miss nobody read in the sim (a reader it doesn't run): it must be the cache's
    /// own computation, and the target is the unit standing at the eye point; the entry is kept.
    ///
    /// # Errors
    ///
    /// The miss isn't `CheckLOS` of its sample, or its target can't be told.
    pub fn adopt_miss(&mut self, vw: &dyn VisionWorld, m: Miss) -> Result<(), VisError> {
        let units = vw.units();
        let geometry = self.geometry()?;
        // The radius at the miss: now, or at the frame's start (a unit that joined combat since
        // changed it; the miss was made before the frame's end).
        let radius = vw.radius(m.owner);
        let mut has_los = check_los(&geometry, radius, m.sample, m.point)?.0;
        if has_los != m.result
            && let Some(r0) = self.frame_radius.get(&m.owner).copied().filter(|r0| *r0 != radius)
        {
            has_los = check_los(&geometry, r0, m.sample, m.point)?.0;
        }
        if has_los != m.result {
            return Err(format!(
                "an unexplained miss of {:?}: the sim computes {has_los}",
                m.owner
            ));
        }
        // The target is the unit standing at the eye point; of several, those in vision range
        // (`HasLOS` asks the cache only about those; `GetLOS` called directly doesn't).
        let at_point: Vec<EntityIdx> = units
            .iter()
            .copied()
            .filter(|t| bits(add(vw.position(*t), EYE)) == bits(m.point))
            .collect();
        let in_range: Vec<EntityIdx> = at_point
            .iter()
            .copied()
            .filter(|t| vw.is_fake(m.owner) || self.in_vision_range(m.owner, *t))
            .collect();
        let targets = if in_range.is_empty() { at_point } else { in_range };
        let [target] = targets[..] else {
            return Err(format!(
                "an unexplained miss of {:?} at {:?}: {} units stand there",
                m.owner,
                m.point,
                targets.len()
            ));
        };
        let c = self.caches.entry(m.owner).or_default();
        if let Some(e) = c.entries.get(&target) {
            let there = vw.position(target);
            let answered = match e.pos {
                Some(p) => sqr_distance_2d(p, there) < MAX_POSITION_ERROR2,
                None => e.hits.iter().any(|h| sqr_distance_2d(*h, there) == 0.0),
            };
            if answered {
                return Err(format!(
                    "an unexplained miss of {:?} on {target:?}, which its cache answers",
                    m.owner
                ));
            }
        }
        match c.sample {
            Some(s) if bits(s) != bits(m.sample) => {
                return Err(format!(
                    "an unexplained miss from sample {:?}, the cache's is {s:?}",
                    m.sample
                ));
            }
            None => c.sample = Some(m.sample),
            _ => {}
        }
        c.entries.insert(
            target,
            Entry { pos: Some(vw.position(target)), has_los: Some(has_los), hits: Vec::new() },
        );
        self.counts.unmodelled_fills += 1;
        Ok(())
    }

    /// `LineOfSightGeometry.HasObstacle(from, to, fudge)`, checked against a recorded answer.
    ///
    /// # Errors
    ///
    /// No geometry, or an answer that differs.
    pub fn obstacle(
        &mut self,
        from: V3,
        to: V3,
        fudge: f32,
        recorded: bool,
    ) -> Result<bool, VisError> {
        let g = self.geometry()?;
        let v = g.has_obstacle(from, to, fudge).map_err(|e| e.to_string())?;
        if v != recorded {
            return Err(format!(
                "HasObstacle({from:?}, {to:?}, {fudge}): the sim computes {v}, the recording {recorded}"
            ));
        }
        self.counts.obstacle_reads += 1;
        Ok(v)
    }
}

/// A segment of a cell: `(cell x, cell z, A, B, Height)`.
pub type SegDump = (i32, i32, [f32; 2], [f32; 2], f32);

/// The game's geometry as WrathOracle's `los_geom` dumps it (`LineOfSightGeometry`'s private
/// `m_Bounds` and `m_Cells`), for the check against the sim's.
#[derive(Debug, Clone, PartialEq)]
pub struct GeomDump {
    pub nx: i32,
    pub nz: i32,
    /// `m_Bounds.min.x` and `.z`.
    pub min: [f32; 2],
    /// `(cell x, cell z, A, B, Height)` of every segment of every cell.
    pub segs: Vec<SegDump>,
}

/// A `HasObstacle(from, to, fudge)` probe and the game's answer (`los_probe`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probe {
    pub from: V3,
    pub to: V3,
    pub fudge: f32,
    pub result: bool,
}

/// The sim's geometry against the game's dump: the same grid and, in each cell, the same
/// segments (the order within a cell is the blockers' registration order, which the game takes
/// from a `HashSet`: compared as sets).
///
/// # Errors
///
/// The first difference.
pub fn check_geometry(g: &Geometry, d: &GeomDump) -> Result<usize, VisError> {
    let (nx, nz, min_x, min_z) = g.grid();
    if (nx, nz) != (d.nx, d.nz)
        || min_x.to_bits() != d.min[0].to_bits()
        || min_z.to_bits() != d.min[1].to_bits()
    {
        return Err(format!(
            "the grid is {nx} x {nz} from ({min_x}, {min_z}), the game's {} x {} from {:?}",
            d.nx, d.nz, d.min
        ));
    }
    type Key = (i32, i32, [u32; 2], [u32; 2], u32);
    let key = |s: &SegDump| -> Key {
        (s.0, s.1, s.2.map(f32::to_bits), s.3.map(f32::to_bits), s.4.to_bits())
    };
    let mut ours: Vec<Key> = g.segments().iter().map(key).collect();
    let mut theirs: Vec<Key> = d.segs.iter().map(key).collect();
    ours.sort_unstable();
    theirs.sort_unstable();
    if let Some(i) = (0..ours.len().max(theirs.len())).find(|i| ours.get(*i) != theirs.get(*i)) {
        return Err(format!(
            "the segments differ at {i} of {} (the game's {}): the sim's {:?}, the game's {:?}",
            ours.len(),
            theirs.len(),
            ours.get(i),
            theirs.get(i)
        ));
    }
    Ok(ours.len())
}

/// Every probe's answer from the sim's geometry against the game's.
///
/// # Errors
///
/// The first difference.
pub fn check_probes(g: &Geometry, probes: &[Probe]) -> Result<usize, VisError> {
    for (i, p) in probes.iter().enumerate() {
        let v = g.has_obstacle(p.from, p.to, p.fudge).map_err(|e| e.to_string())?;
        if v != p.result {
            return Err(format!(
                "probe {i}: HasObstacle({:?}, {:?}, {}) is {v} in the sim, {} in the game",
                p.from, p.to, p.fudge, p.result
            ));
        }
    }
    Ok(probes.len())
}

#[cfg(test)]
mod tests;

/// An event the sim sees that could change a unit's `Corpulence` (a fact or buff added or removed,
/// a view replaced, a size write): a `corp` read made before it, or while it was being processed,
/// doesn't tell the corpulence after it ([`Vision::corp_bumps`]). Events the sim doesn't run
/// (an unported action) aren't seen: that is a limit of the inference, noted in docs/poc5/vision.md.
pub fn note_corpulence_change(w: &mut World, e: EntityIdx) {
    if w.unit(e).is_some() {
        let at = w.combat.nav.at;
        let v = &mut w.combat.vision;
        let list = v.corp_bumps.entry(e).or_default();
        v.bump_log.push((e, list.len()));
        list.push(at);
    }
}

impl Vision {
    /// The events noted since `mark` (a length of [`Vision::bump_log`]) are stamped no earlier
    /// than `stamp`: the end of the driven entry they were made in. A driven entry's readers
    /// consume records past the entry's own index, so a read and a change in one entry are taken
    /// as read before change (the read doesn't count).
    pub fn restamp_bumps(&mut self, mark: usize, stamp: usize) {
        for &(e, k) in self.bump_log.get(mark..).unwrap_or_default() {
            if let Some(s) = self.corp_bumps.get_mut(&e).and_then(|l| l.get_mut(k)) {
                *s = (*s).max(stamp);
            }
        }
    }
}

/// A size write or a view replacement: [`note_corpulence_change`], and the entity goes on the
/// bounds controller's update list (`HandleUnitSizeChanged`).
pub fn note_size_change(w: &mut World, e: EntityIdx) {
    note_corpulence_change(w, e);
    if w.unit(e).is_some() {
        *w.combat.vision.size_gen.entry(e).or_default() += 1;
    }
}

/// The sim wrote a unit's position ([`Vision::pos_writes`]).
pub fn note_position_write(w: &mut World, e: EntityIdx) {
    *w.combat.vision.pos_writes.entry(e).or_default() += 1;
}
