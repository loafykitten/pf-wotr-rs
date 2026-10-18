//! `UnitVisionRangeController` over `EntityBoundsController`'s sphere colliders (decompiled, not
//! copied; docs/poc5/vision.md): the units an awake unit may see (`CanBeInVisionRange`).
//!
//! The game keeps one `SphereCollider` per entity with bounds in a private physics scene
//! (`EntityBoundsController`, registered just before the vision controller). The vision controller
//! asks that scene, for each awake unit that isn't an extra, which unit colliders (layer
//! `SphereBoundsUnit`, only enabled ones) overlap the vertical capsule from `pos - up * 2r` to
//! `pos + up * 2r` with radius `r = VisionRangeMeters`, and keeps their units.
//!
//! What the model holds, and what the recordings fixed (docs/poc5/vision.md). A pair the sim's
//! unknowns (the view's scale, a corpulence not read yet, a physics step that may not have run, a
//! fade) could change is left to the recording ([`Tri::Maybe`], counted by [`Cause`]). What the
//! sim *hypothesizes* about the state of the colliders at the window's start (the creation order
//! of the colliders made before it, no fade in progress, the positions PhysX has at the first
//! frame) is not an unknown in that sense: everything decided rests on them, and a wrong one is
//! found only when the recording disagrees with the sim (a wrong hypothesis matched by an equally
//! wrong recording would pass; docs/poc5/vision.md, "Hypotheses about the window's start").
//! - **Population.** `State.EntitiesWithBounds` in the order its entities joined their states; the
//!   first tick (`m_UpdateAll`) walks it through `EntityPoolEnumerator`, which skips entities that are
//!   not `ShouldBeEnumeratedByEntityPoolEnumerator` (in the game and not suppressed): a unit hidden
//!   at that tick may already own a disabled collider (made earlier: its place in the order is
//!   unknown, [`Collider::history_unknown`]) or get one when it comes into the game. A collider is
//!   destroyed with its view, which the sim doesn't know the time of: [`Tick::removed`].
//! - **Lost and re-established.** The update list takes an entity whose position, game state or
//!   size changed while it was in the game, and `TickEntity` runs for it at the next tick even if
//!   it has left the game since; the sim sees the unit's state at its ticks only. So once a unit is
//!   out of the game, its collider's transform, radius and PhysX copy are unknown (the collider is
//!   *lost*), and they are re-established only by a tick the sim models after it is back (its
//!   transform the unit's position, its radius by the rule from an unknown one) and, for PhysX's
//!   copy, the next physics step. The same holds after a frame the sim didn't follow
//!   ([`Bounds::lose_track`]): every collider is lost until its next modelled update.
//! - **Position.** The bounds controller sets a collider's transform to the entity's `Position` in
//!   its tick, but `Physics.autoSyncTransforms` is off: PhysX takes the transforms at a physics
//!   step (a fixed update, before the frame's controllers). Most frames run a step, so a query
//!   sees where the unit stood at the previous frame's tick; a frame that runs none sees the tick
//!   before. The capsule is at the querying unit's position of this frame. [`Bounds::tick`] takes
//!   whether the frame ran a step ([`Step`]); when that isn't known the collider has the
//!   candidates of both. A collider made in a tick is at the origin until the next step. An
//!   update sets the transform only when it isn't `Vector3 ==` the position (1e-5 m): after a
//!   tick, a unit in the game has its collider's transform within that tolerance of its position
//!   (any change of it while in the game puts it on the update list), exactly where the sim
//!   followed the updates; after a loss only to the tolerance, and pairs that close to deciding
//!   go to the recording.
//! - **Radius.** A collider is made with `Max(0.05, Corpulence * view scale)` and, in the same
//!   tick, set to `Max(0.05, Corpulence)` (its own transform has unit scale) if that is more than
//!   0.05 away; later ticks of the entity (position, game state or size changed) do the same
//!   against the radius it has. The sim has the corpulence from the recording, never the view's
//!   scale, so the radius is an interval: `[c - 0.05, c + 0.05]` until a size change of more than
//!   0.1 makes it exact, and unbounded above while the corpulence is unknown (nothing bounds a
//!   corpulence; at least 0.5 less the hysteresis below).
//! - **Enabled.** `ShouldBeEnumeratedByEntityPoolEnumerator` (in the game and not suppressed), set
//!   by `UpdateViewActive` the moment either changes, except while the view fade-hides
//!   (`m_IsFadeHide`): the collider then keeps its state until the fade ends, which the sim
//!   doesn't know ([`Enabled::Unknown`]).
//! - **Order.** The results come in the colliders' order, the order they were made in, so it is
//!   also the order `UnitGroup.ExtractNearUnits` unions the sets in (they are refilled in it each
//!   tick). The order of the colliders made before the window is assumed to be the pool's.
//!
//! The overlap itself is PhysX's sphere against capsule: the squared distance from the sphere's
//! centre to the capsule's segment against the squared sum of the radii, in `f32`
//! (`Gu::distancePointSegmentSquared`).

use std::collections::HashSet;

use crate::world::EntityIdx;

use super::V3;
use crate::combat::orientation;

/// `UnitEntityView.Corpulence` is at least this (`Mathf.Max(.., 0.5f)`).
pub const MIN_CORPULENCE: f32 = 0.5;

/// The hysteresis of `EntityBoundsController.TickEntity`: the radius changes only when it is more
/// than this away from `ScaleRadius(Corpulence, 1)`; a little more, so that a radius on the edge
/// stays inside the interval.
const BAND: f32 = 0.05 + 1e-6;

/// A collider's radius as far as the sim knows it: it is in `lo..=hi` (`hi` infinite: no bound).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RadiusIv {
    pub lo: f32,
    pub hi: f32,
}

impl RadiusIv {
    /// A corpulence the recording doesn't show: at least `0.5`, less the hysteresis.
    pub const UNKNOWN: Self = Self { lo: MIN_CORPULENCE - BAND, hi: f32::INFINITY };

    fn exact(r: f32) -> Self {
        Self { lo: r, hi: r }
    }

    /// Whether the interval is one point.
    #[must_use]
    pub fn is_exact(&self) -> bool {
        self.lo == self.hi
    }
}

/// What a tick of `EntityBoundsController` makes of a unit's collider radius: `prev` is what it
/// can have (`None`: not made yet, its view's scale unknown), `corpulence` the unit's now.
#[must_use]
pub fn next_radius(prev: Option<RadiusIv>, corpulence: Option<f32>) -> RadiusIv {
    let Some(c) = corpulence else { return RadiusIv::UNKNOWN };
    // `ScaleRadius(Corpulence, 1)`.
    let t = c.max(0.05);
    let band = RadiusIv { lo: (t - BAND).max(0.05), hi: t + BAND };
    let Some(iv) = prev else { return band };
    // A held radius within the band stays, any other becomes `t`.
    let (lo, hi) = (iv.lo.max(band.lo), iv.hi.min(band.hi));
    if lo > hi {
        return RadiusIv::exact(t);
    }
    if iv.lo < band.lo || iv.hi > band.hi {
        return RadiusIv { lo: lo.min(t), hi: hi.max(t) };
    }
    iv
}

/// Whether a collider is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enabled {
    Yes,
    No,
    /// The view is fade-hiding: `UpdateViewActive` returns early, the collider keeps its state
    /// until the fade ends (when isn't known).
    Unknown,
}

/// Whether the frame ran a physics step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Ran,
    NotRan,
    Unknown,
}

/// What the sim knows of a pair: it overlaps, it doesn't, or something unknown decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tri {
    In,
    Out,
    Maybe,
}

/// Why a pair is [`Tri::Maybe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    /// The collider's corpulence isn't known (its radius has no upper bound).
    Corpulence,
    /// Its position isn't known (a step that may or may not have run, or a history the sim
    /// doesn't have).
    Position,
    /// Its `enabled` isn't known (a fade-hiding view).
    Enabled,
    /// Its radius is known to within the view-scale band only.
    Scale,
    /// More colliders are in reach than the controller's result buffer holds (512): which ones
    /// the game keeps depends on PhysX's order.
    Capacity,
}

impl Cause {
    pub const ALL: [Self; 5] =
        [Self::Corpulence, Self::Position, Self::Enabled, Self::Scale, Self::Capacity];

    fn index(self) -> usize {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0)
    }
}

/// A unit as the bounds controller sees it at its tick.
#[derive(Debug, Clone, Copy)]
pub struct BoundsUnit {
    pub unit: EntityIdx,
    /// `Position` now.
    pub pos: V3,
    /// `pos` is the unit's (the driven agent's, or an observation row's); else a guess, and the
    /// collider is lost.
    pub pos_known: bool,
    /// `Position` at the frame's start: where the first tick of a window finds the collider (the
    /// previous frame's tick put it there); `None` when the recording doesn't show it.
    pub prefix: Option<V3>,
    /// `IsInGame` (the entity is added to the update list only while it is, and the first tick
    /// walks only the entities that are).
    pub in_game: bool,
    /// What `ShouldBeEnumeratedByEntityPoolEnumerator` and the fade make of the collider's `enabled`.
    pub enabled: Enabled,
    /// `Corpulence` now, when the recording shows it.
    pub corpulence: Option<f32>,
    /// How many size writes (`HandleUnitSizeChanged`) the sim has seen of the unit: the update
    /// list's reason to run `TickEntity` for it besides position and game state.
    pub size_gen: u64,
    /// How many changes of `IsInGame` the sim has seen of the unit (a hide and a return between
    /// two ticks is an update the states at the ticks don't show).
    pub in_game_gen: u64,
}

/// One `SphereCollider`.
#[derive(Debug, Clone)]
struct Collider {
    unit: EntityIdx,
    /// Where PhysX can have the collider (queries use it): one position, or the candidates when a
    /// step may or may not have run.
    phys: Vec<V3>,
    /// The position isn't known at all.
    phys_unknown: bool,
    /// The collider's transform (`transform.position`, set by the tick; PhysX takes it at the
    /// next step); `None` while the collider is lost (its unit out of the game, or a frame the sim
    /// didn't follow).
    transform: Option<V3>,
    /// The transform is only known to be within `Vector3 ==`'s tolerance of `transform` (an
    /// update from an unknown transform keeps it when it is that close to the position).
    fuzzy: bool,
    /// PhysX's copy is only known to that tolerance (taken from a fuzzy transform).
    phys_fuzzy: bool,
    /// What the radius can be: [`RadiusIv::UNKNOWN`] while lost.
    radius: RadiusIv,
    /// The corpulence the radius was last established with (at an update the sim modelled);
    /// `None`: the radius is unknown until the next one. A radius stays known only while the
    /// unit's corpulence is known and equal to it.
    radius_corp: Option<f32>,
    /// The point estimate of the radius (`Max(0.05, Corpulence)`), for the margins and the guess.
    centre: Option<f32>,
    enabled: Enabled,
    /// A unit that was hidden at the window's first tick: it may have owned a collider before
    /// (made earlier, at an unknown place in the order) or get one when it comes into the game.
    history_unknown: bool,
    /// Its unit's view was (possibly) replaced: the new view's collider is made at an update, at
    /// an unknown place in the order, and the old one, with its last transform, lives until the
    /// old view is destroyed (now or after a transition the sim doesn't follow). Whether either
    /// overlaps is the recording's for the rest of the run.
    replaced: bool,
    /// Made (possibly) while its unit was out of the game: enabled until `UpdateViewActive` runs
    /// at its return, so its `enabled` is unknown until then.
    made_hidden: bool,
    /// The unit's state at the last tick the sim modelled: what the update list reacts to; `None`
    /// after a frame the sim didn't follow (whether the next tick updates the collider isn't known).
    seen: Option<Seen>,
}

/// A unit's state at a bounds tick.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Seen {
    pos: V3,
    in_game: bool,
    size_gen: u64,
    in_game_gen: u64,
}

impl Collider {
    /// The unit is out of the game, or the sim lost track of the ticks: the transform, the radius
    /// and (from the next step that may take the transform) PhysX's copy are unknown.
    fn lose(&mut self) {
        self.transform = None;
        self.fuzzy = false;
        self.radius = RadiusIv::UNKNOWN;
        self.radius_corp = None;
        self.centre = None;
        self.phys_unknown = true;
    }
}

/// One pair of a query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub unit: EntityIdx,
    pub tri: Tri,
    /// Why a pair is [`Tri::Maybe`].
    pub cause: Option<Cause>,
    /// For a collider of one position and a corpulence the recording shows, with the radius the
    /// corpulence (the view's scale 1): the distance from the capsule's segment to the sphere's
    /// surface beyond the capsule's radius, in metres and `f64` (negative: overlapping).
    pub margin: Option<f64>,
    /// The `f32` decision differs from the exact one (only with a margin).
    pub rounding: bool,
    /// The same, PhysX's decision.
    pub guess: Option<bool>,
    /// The collider's place in the order is the pool's (not a unit of unknown history).
    pub order_known: bool,
    /// What the pair was before the result buffer's capacity made it [`Tri::Maybe`].
    pub capped: Option<Tri>,
    /// The collider's view was replaced: its old collider may still take a slot of the result
    /// buffer (dropped after, its entity link gone, or a duplicate of the unit).
    pub extra_slot: bool,
}

/// What a tick found beyond updating the colliders.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Tick {
    /// Units with a collider that are not among the entities any more: their views' destruction
    /// (the time of which the sim doesn't know) is taken to destroy the collider now.
    pub removed: Vec<EntityIdx>,
}

/// The entity-bounds scene.
#[derive(Debug, Default)]
pub struct Bounds {
    colliders: Vec<Collider>,
    started: bool,
    /// [`Bounds::lose_track`] ran since the last tick: the next one doesn't follow the previous.
    gap: bool,
    /// Colliders made after the first tick (units that joined later).
    pub made_later: usize,
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// `Gu::distancePointSegmentSquared(p0, p1, point)` in `f32`.
#[must_use]
pub fn point_segment_squared(p0: V3, p1: V3, point: V3) -> f32 {
    let mut diff = sub(point, p0);
    let seg = sub(p1, p0);
    let mut t = dot(diff, seg);
    if t > 0.0 {
        let dd = dot(seg, seg);
        if t >= dd {
            diff = sub(diff, seg);
        } else {
            t /= dd;
            diff = [diff[0] - seg[0] * t, diff[1] - seg[1] * t, diff[2] - seg[2] * t];
        }
    }
    dot(diff, diff)
}

/// A sphere at `centre` of radius `rs` against `OverlapCapsule(pos - up * 2r, pos + up * 2r, r)`:
/// PhysX's test. Also the exact (`f64`) margin to the surface and whether the exact decision
/// differs.
#[must_use]
pub fn capsule_sphere(pos: V3, r: f32, centre: V3, rs: f32) -> (bool, f64, bool) {
    // `Vector3.up * visionRangeMeters * 2f`.
    let v = [0.0, r * 2.0, 0.0];
    let bottom = sub(pos, v);
    let top = [pos[0] + v[0], pos[1] + v[1], pos[2] + v[2]];
    let sum = r + rs;
    let hit = point_segment_squared(bottom, top, centre) <= sum * sum;
    // Exact: the vertical segment through `pos`, half length 2r.
    let (dx, dz) =
        (f64::from(centre[0]) - f64::from(pos[0]), f64::from(centre[2]) - f64::from(pos[2]));
    let y0 = f64::from(pos[1]);
    let h = 2.0 * f64::from(r);
    let cy = f64::from(centre[1]).clamp(y0 - h, y0 + h);
    let dy = f64::from(centre[1]) - cy;
    let dist = (dx * dx + dz * dz + dy * dy).sqrt();
    // (The sum in `f64` from the two `f32`s: `sum` is the rounded one.)
    let margin = dist - (f64::from(r) + f64::from(rs));
    (hit, margin, hit != (margin <= 0.0))
}

/// `UnitVisionRangeController.OverlapResults.Length`.
const RESULT_BUFFER: usize = 512;

/// `Vector3 ==` holds below a squared distance of 9.9999994e-11 (1e-5 m); a little more.
const FUZZ: f32 = 2e-5;

/// More candidate positions than this make a collider's position unknown.
const MAX_CANDIDATES: usize = 6;

impl Bounds {
    /// `EntityBoundsController.Tick` (in the game's order: before the vision controller): when the
    /// frame ran a physics step (`step`, before its controllers), the transforms the previous ticks
    /// set reach PhysX; then the entities in the update list move their collider's transform and
    /// update its radius: on the first tick of a run all of them that are in the game (a collider
    /// is made for those without one), later those whose position, game state or corpulence
    /// changed (a collider is made for an entity that is in the game). `units` are the pool's, in
    /// order. The first tick of a window finds the colliders where the unit stood at the
    /// frame's start.
    pub fn tick(&mut self, units: &[BoundsUnit], step: Step) -> Tick {
        let first = !self.started;
        self.started = true;
        // A collider first met after a gap may have been made, and synced, in a frame the sim
        // didn't follow.
        let gap = std::mem::take(&mut self.gap);
        for c in &mut self.colliders {
            match (step, c.transform) {
                (Step::NotRan, _) => {}
                // A step may have taken a transform the sim doesn't know.
                (_, None) => c.phys_unknown = true,
                (Step::Ran, Some(t)) => {
                    c.phys = vec![t];
                    c.phys_unknown = false;
                    c.phys_fuzzy = c.fuzzy;
                }
                (Step::Unknown, Some(t)) => {
                    if !c.phys.iter().any(|p| p.map(f32::to_bits) == t.map(f32::to_bits)) {
                        c.phys.push(t);
                    }
                    c.phys_unknown |= c.phys.len() > MAX_CANDIDATES;
                    c.phys_fuzzy |= c.fuzzy;
                }
            }
        }
        let present: HashSet<EntityIdx> = units.iter().map(|u| u.unit).collect();
        let mut out = Tick::default();
        self.colliders.retain(|c| {
            let keep = present.contains(&c.unit);
            if !keep {
                out.removed.push(c.unit);
            }
            keep
        });
        for u in units {
            let seen = Seen {
                pos: u.pos,
                in_game: u.in_game,
                size_gen: u.size_gen,
                in_game_gen: u.in_game_gen,
            };
            let Some(c) = self.colliders.iter_mut().find(|c| c.unit == u.unit) else {
                // A unit without a collider: on the first tick, one made before the window for
                // what is in the game, and what is hidden may own one from before (its place in
                // the order unknown). Later, one is made now for a unit in the game; one hidden
                // may still have an update queued from when it was in the game (hiding doesn't
                // cancel it), which makes its collider enabled (`UpdateViewActive` ran before the
                // collider existed) at an unknown place in the order: unknown, not none.
                let made_hidden = !first && !u.in_game;
                // Made in an unseen frame (after a gap): its place, radius, order unknown.
                let unseen = !first && gap;
                self.made_later += usize::from(!first && u.in_game);
                let mut c = Collider {
                    unit: u.unit,
                    // The first tick finds the collider where the unit stood at the previous
                    // tick, if that frame synced it; a collider made now is at the origin (the
                    // new object's place) until a step takes the transform set after it.
                    phys: if first { u.prefix.into_iter().collect() } else { vec![[0.0; 3]] },
                    phys_unknown: first && (u.prefix.is_none() || step != Step::Ran),
                    // The first tick's update sets the transform only when it isn't already
                    // `==` the position (the collider is taken to be at the prefix, as PhysX's
                    // copy is); a collider made now starts at the view's transform, which the
                    // sim doesn't hold (`UnitMoveController` writes the view directly and the
                    // entity through the tolerant setter), and keeps it when it is `==` the
                    // position: only known to the tolerance around the position.
                    transform: Some(match (first, u.prefix) {
                        (true, Some(p)) => orientation::set_position(p, u.pos),
                        _ => u.pos,
                    }),
                    fuzzy: !first || u.prefix.is_none(),
                    phys_fuzzy: false,
                    radius: next_radius(None, u.corpulence),
                    radius_corp: u.corpulence,
                    centre: u.corpulence.map(|x| x.max(0.05)),
                    enabled: if made_hidden { Enabled::Unknown } else { u.enabled },
                    history_unknown: !u.in_game || unseen,
                    made_hidden,
                    replaced: false,
                    seen: u.pos_known.then_some(seen),
                };
                if !u.in_game || !u.pos_known || unseen {
                    c.lose();
                    if unseen {
                        c.seen = None;
                    }
                }
                self.colliders.push(c);
                continue;
            };
            if u.in_game {
                c.made_hidden = false;
            }
            c.enabled = if c.made_hidden { Enabled::Unknown } else { u.enabled };
            if !u.in_game {
                // An update queued while it was in the game may still run (or have run).
                c.lose();
                c.seen = Some(seen);
                continue;
            }
            if !u.pos_known {
                // Where it is, and whether it moved, aren't known.
                c.lose();
                c.seen = None;
                continue;
            }
            // On the update list: moved, back in the game or resized since the last tick (after
            // a frame the sim didn't follow: not known, so the radius stays as unknown as it was).
            let listed = c.seen.is_some_and(|s| {
                !s.in_game
                    || s.pos != u.pos
                    || s.size_gen != u.size_gen
                    || s.in_game_gen != u.in_game_gen
            });
            // `TickEntity` sets the transform only when it isn't `Vector3 ==` the position. Not
            // updated, it is where the last update left it (no change in the game since); after
            // a loss, within the tolerance of the position (whichever update ran last).
            match (listed, c.transform, c.fuzzy) {
                (true, Some(t), false) => c.transform = Some(orientation::set_position(t, u.pos)),
                (true, _, _) | (false, None, _) => {
                    c.transform = Some(u.pos);
                    c.fuzzy = true;
                }
                (false, Some(_), _) => {}
            }
            if listed {
                // The rule, from what the radius was, with the corpulence now.
                c.radius = next_radius(Some(c.radius), u.corpulence);
                c.radius_corp = u.corpulence;
                c.centre = u.corpulence.map(|x| x.max(0.05));
            } else if c.radius_corp.map(f32::to_bits) != u.corpulence.map(f32::to_bits) {
                // The corpulence isn't the one the radius was established with (or isn't known):
                // an update the sim can't see (a move away and back between two ticks) would
                // have applied it, so the radius is unknown until an update the sim models.
                c.radius = RadiusIv::UNKNOWN;
                c.radius_corp = None;
            }
            c.seen = Some(seen);
        }
        out
    }

    /// A frame the sim didn't follow (the bounds controller may have ticked, the units may have
    /// moved or changed size, a step may have run): every collider is lost until its next
    /// modelled update, and the next tick can't tell which units it updates.
    pub fn lose_track(&mut self) {
        for c in &mut self.colliders {
            c.lose();
            c.seen = None;
        }
        self.gap = true;
    }

    /// Physics settings the step and sync model isn't for: no collider's PhysX position is known.
    pub fn forget_positions(&mut self) {
        for c in &mut self.colliders {
            c.phys_unknown = true;
        }
    }

    /// A unit's view replaced (a polymorph's start or end): its collider is lost, its place in
    /// the order unknown, and its pairs the recording's from now on ([`Collider::replaced`]).
    pub fn replace_unit(&mut self, u: EntityIdx) {
        for c in self.colliders.iter_mut().filter(|c| c.unit == u) {
            c.lose();
            c.seen = None;
            c.history_unknown = true;
            c.replaced = true;
        }
    }

    /// A unit's collider whose transform the sim set from a wrong position: lost until its next
    /// modelled update.
    pub fn lose_unit(&mut self, u: EntityIdx) {
        for c in self.colliders.iter_mut().filter(|c| c.unit == u) {
            c.lose();
            c.seen = None;
        }
    }

    /// `OverlapCapsule` for a unit at `pos` with vision radius `r`: every collider that is or may
    /// be enabled, in the order they were made.
    #[must_use]
    pub fn overlap(&self, pos: V3, r: f32) -> Vec<Hit> {
        let mut out = Vec::new();
        for c in self.colliders.iter().filter(|c| c.enabled != Enabled::No || c.replaced) {
            let (mut tri, mut cause) = (Tri::Maybe, Some(Cause::Position));
            if !c.phys_unknown {
                // A copy known to the tolerance only: the sphere's centre within it.
                let sure_in =
                    |e: f32| c.phys.iter().all(|p| capsule_sphere(pos, r, *p, c.radius.lo - e).0);
                let sure_out = |e: f32| {
                    c.radius.hi.is_finite()
                        && c.phys.iter().all(|p| !capsule_sphere(pos, r, *p, c.radius.hi + e).0)
                };
                let e = if c.phys_fuzzy { FUZZ } else { 0.0 };
                (tri, cause) = if sure_in(e) {
                    (Tri::In, None)
                } else if sure_out(e) {
                    (Tri::Out, None)
                } else if !c.radius.hi.is_finite() {
                    (Tri::Maybe, Some(Cause::Corpulence))
                } else if c.phys.len() > 1 || sure_in(0.0) || sure_out(0.0) {
                    (Tri::Maybe, Some(Cause::Position))
                } else {
                    (Tri::Maybe, Some(Cause::Scale))
                };
            } else if !c.radius.hi.is_finite() {
                cause = Some(Cause::Corpulence);
            }
            if c.enabled == Enabled::Unknown && tri != Tri::Out {
                (tri, cause) = (Tri::Maybe, Some(Cause::Enabled));
            }
            if c.replaced {
                // Which collider exists, where, and enabled or not.
                (tri, cause) = (Tri::Maybe, Some(Cause::Enabled));
            }
            let (mut margin, mut rounding, mut guess) = (None, false, None);
            if let (Some(centre), [p], false, false) =
                (c.centre, c.phys.as_slice(), c.phys_unknown, c.replaced)
            {
                let (hit, m, rd) = capsule_sphere(pos, r, *p, centre);
                (margin, rounding, guess) = (Some(m), rd, Some(hit));
            }
            out.push(Hit {
                unit: c.unit,
                tri,
                cause,
                margin,
                rounding,
                guess,
                order_known: !c.history_unknown,
                capped: None,
                extra_slot: c.replaced,
            });
        }
        // `OverlapCapsule` fills a buffer of 512: with more colliders in reach, which ones it keeps
        // is PhysX's order.
        // (A replaced view's old collider may take a slot too.)
        let possible = out.iter().filter(|h| h.tri != Tri::Out).count()
            + out.iter().filter(|h| h.extra_slot).count();
        if possible > RESULT_BUFFER {
            for h in out.iter_mut().filter(|h| h.tri != Tri::Out) {
                h.capped = Some(h.tri);
                (h.tri, h.cause) = (Tri::Maybe, Some(Cause::Capacity));
            }
        }
        out
    }

    /// Whether a unit has a collider.
    #[must_use]
    pub fn has(&self, u: EntityIdx) -> bool {
        self.colliders.iter().any(|c| c.unit == u)
    }
}

/// How a recorded set differs from the sim's overlap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difference {
    /// The sim's overlap holds a unit the recorded set doesn't.
    Missing(EntityIdx),
    /// The recorded set holds a unit the sim's overlap doesn't (or has no enabled collider).
    Extra(EntityIdx),
    /// The recorded set has a size the overlap can't give: more than the result buffer holds
    /// (512), or fewer than the buffer fills when more colliders than that are surely in reach.
    Count(usize),
}

/// What [`differences`] found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Differences {
    pub diffs: Vec<Difference>,
    /// Pairs the sim left to the recording, by [`Cause`] (in [`Cause::ALL`]'s order).
    pub open: [usize; 5],
    /// Pairs the sim decided that equal the recording's.
    pub decided: usize,
    /// Of the pairs left open only by the view-scale band, those whose answer with scale 1 and the
    /// corpulence as the radius equals / differs from the recording's.
    pub guess_agree: usize,
    pub guess_differ: usize,
}

/// The differences between a query's hits and a recorded set, and the pairs the recording decided.
#[must_use]
pub fn differences(hits: &[Hit], recorded: &HashSet<EntityIdx>) -> Differences {
    let mut out = Differences::default();
    let mut seen = HashSet::new();
    for h in hits {
        seen.insert(h.unit);
        let rec = recorded.contains(&h.unit);
        match (h.tri, rec) {
            (Tri::In, false) => out.diffs.push(Difference::Missing(h.unit)),
            (Tri::Out, true) => out.diffs.push(Difference::Extra(h.unit)),
            (Tri::Maybe, _) => {
                let cause = h.cause.unwrap_or(Cause::Position);
                out.open[cause.index()] += 1;
                if cause == Cause::Scale
                    && let Some(g) = h.guess
                {
                    if g == rec {
                        out.guess_agree += 1;
                    } else {
                        out.guess_differ += 1;
                    }
                }
            }
            _ => out.decided += 1,
        }
    }
    let mut rest: Vec<EntityIdx> = recorded.iter().copied().filter(|u| !seen.contains(u)).collect();
    rest.sort();
    out.diffs.extend(rest.into_iter().map(Difference::Extra));
    // The buffer holds 512 hits: never more. A result of fewer than 512 wasn't truncated, so it
    // holds every hit that surely overlaps; only a full buffer may leave some out.
    if recorded.len() > RESULT_BUFFER {
        out.diffs.push(Difference::Count(recorded.len()));
    } else if recorded.len() + hits.iter().filter(|h| h.extra_slot).count() < RESULT_BUFFER {
        // (Old colliders of replaced views may have taken slots and been dropped after.)
        out.diffs.extend(
            hits.iter()
                .filter(|h| h.capped == Some(Tri::In) && !recorded.contains(&h.unit))
                .map(|h| Difference::Missing(h.unit)),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: EntityIdx = EntityIdx(1);
    const B: EntityIdx = EntityIdx(2);
    const C: EntityIdx = EntityIdx(3);
    const RAN: Step = Step::Ran;

    /// A unit in the game with a corpulence (`None`: unknown).
    fn unit(u: EntityIdx, pos: V3, corpulence: Option<f32>) -> BoundsUnit {
        BoundsUnit {
            unit: u,
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

    fn k(u: EntityIdx, pos: V3) -> BoundsUnit {
        unit(u, pos, Some(0.5))
    }

    fn hidden(mut u: BoundsUnit) -> BoundsUnit {
        u.in_game = false;
        u.enabled = Enabled::No;
        u
    }

    fn tris(b: &Bounds, pos: V3, r: f32) -> Vec<(EntityIdx, Tri)> {
        b.overlap(pos, r).iter().map(|h| (h.unit, h.tri)).collect()
    }

    /// A bounds scene after the first tick (all steps known, so nothing is open for the position).
    fn settled(units: &[BoundsUnit]) -> Bounds {
        let mut b = Bounds::default();
        b.tick(units, RAN);
        b
    }

    #[test]
    fn capsule_against_sphere_is_the_distance_to_the_segment() {
        // Radius 10: the segment runs 20 m above and below the capsule's centre.
        let pos = [0.0, 0.0, 0.0];
        // Beside the centre: 10 + 0.5 reaches 10.5 m.
        assert!(capsule_sphere(pos, 10.0, [10.4, 0.0, 0.0], 0.5).0);
        assert!(!capsule_sphere(pos, 10.0, [10.6, 0.0, 0.0], 0.5).0);
        // Above the segment's top (20 m): the distance is to the end point.
        assert!(capsule_sphere(pos, 10.0, [0.0, 30.4, 0.0], 0.5).0);
        assert!(!capsule_sphere(pos, 10.0, [0.0, 30.6, 0.0], 0.5).0);
        // The corner: past the top and aside, the distance is Euclidean to the end point.
        assert!(capsule_sphere(pos, 10.0, [6.0, 28.0, 0.0], 0.5).0);
        assert!(!capsule_sphere(pos, 10.0, [8.0, 28.0, 0.0], 0.5).0);
        // The margin is metres, negative inside, and the exact test agrees with PhysX's.
        let (_, m, rounding) = capsule_sphere(pos, 10.0, [10.0, 0.0, 0.0], 0.5);
        assert!((m + 0.5).abs() < 1e-9, "{m}");
        assert!(!rounding);
    }

    #[test]
    fn learning_the_corpulence_doesnt_change_a_collider_only_an_update_does() {
        // Made at the first tick with the corpulence unknown: the radius has no bound.
        let mut b = settled(&[unit(A, [10.2, 0.0, 0.0], None)]);
        // The sim learns the corpulence (0.5) while the unit stands still: no update, so the
        // collider's radius is still unbounded and a pair out of reach at 0.45 is the recording's.
        let still = unit(A, [10.2, 0.0, 0.0], Some(0.5));
        b.tick(&[still], RAN);
        b.tick(&[still], RAN);
        let hits = b.overlap([0.0; 3], 9.0);
        assert_eq!((hits[0].tri, hits[0].cause), (Tri::Maybe, Some(Cause::Corpulence)));
        // A move is an update: the rule runs with the corpulence now.
        let moved = unit(A, [20.0, 0.0, 0.0], Some(0.5));
        b.tick(&[moved], RAN);
        b.tick(&[moved], RAN);
        assert_eq!(tris(&b, [0.0; 3], 9.0), vec![(A, Tri::Out)]);
        // A size write the sim saw is an update too.
        let mut b = settled(&[unit(A, [20.0, 0.0, 0.0], None)]);
        let mut grown = unit(A, [20.0, 0.0, 0.0], Some(0.9));
        b.tick(&[grown], RAN);
        assert_eq!(b.overlap([0.0; 3], 9.0)[0].tri, Tri::Maybe);
        grown.size_gen = 1;
        b.tick(&[grown], RAN);
        assert_eq!(b.overlap([0.0; 3], 9.0)[0].tri, Tri::Out);
        // So is a hide and a return between two ticks (the states at the ticks are the same).
        let mut b = settled(&[unit(A, [20.0, 0.0, 0.0], None)]);
        let mut back = unit(A, [20.0, 0.0, 0.0], Some(0.9));
        b.tick(&[back], RAN);
        assert_eq!(b.overlap([0.0; 3], 9.0)[0].tri, Tri::Maybe);
        back.in_game_gen = 2;
        b.tick(&[back], RAN);
        assert_eq!(b.overlap([0.0; 3], 9.0)[0].tri, Tri::Out);
    }

    #[test]
    fn an_update_queued_while_in_the_game_still_moves_the_collider_of_a_unit_hidden_since() {
        let mut b = settled(&[k(A, [0.0; 3])]);
        // It moves to 9 and is hidden before the bounds tick: the game may have queued the
        // move (and ticks it without asking again); the sim sees the state at the tick only.
        b.tick(&[hidden(k(A, [9.0, 0.0, 0.0]))], RAN);
        // Back at 9 in the next frame: the tick puts the transform there, PhysX has it at the
        // next step; until then where it is isn't known.
        b.tick(&[k(A, [9.0, 0.0, 0.0])], RAN);
        let hits = b.overlap([0.0; 3], 1.0);
        assert_eq!((hits[0].tri, hits[0].cause), (Tri::Maybe, Some(Cause::Position)));
        b.tick(&[k(A, [9.0, 0.0, 0.0])], RAN);
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::Out)]);
    }

    #[test]
    fn a_hidden_unit_s_collider_is_lost_until_an_update_reaches_physics() {
        // Sol's round 6: at 0, a move to 9 queued while in the game, hidden before the tick, moved
        // to 18 while hidden, a frame hidden, then back at 18. The collider may be at 9 until the
        // update of the return reaches PhysX: an observer at 9 can't be answered.
        let mut b = settled(&[k(A, [0.0; 3])]);
        b.tick(&[hidden(k(A, [9.0, 0.0, 0.0]))], RAN);
        b.tick(&[hidden(k(A, [18.0, 0.0, 0.0]))], RAN);
        b.tick(&[hidden(k(A, [18.0, 0.0, 0.0]))], RAN);
        b.tick(&[k(A, [18.0, 0.0, 0.0])], RAN);
        for at in [[0.0; 3], [9.0, 0.0, 0.0], [18.0, 0.0, 0.0]] {
            assert_eq!(b.overlap(at, 1.0)[0].tri, Tri::Maybe, "{at:?}");
        }
        // The next step takes the transform the return set.
        b.tick(&[k(A, [18.0, 0.0, 0.0])], RAN);
        assert_eq!(tris(&b, [9.0, 0.0, 0.0], 1.0), vec![(A, Tri::Out)]);
        assert_eq!(tris(&b, [18.0, 0.0, 0.0], 1.0), vec![(A, Tri::In)]);
        // A step that may not have run doesn't take it.
        let mut b = settled(&[k(A, [0.0; 3])]);
        b.tick(&[hidden(k(A, [0.0; 3]))], RAN);
        b.tick(&[k(A, [0.0; 3])], RAN);
        b.tick(&[k(A, [0.0; 3])], Step::Unknown);
        assert_eq!(b.overlap([0.0; 3], 1.0)[0].tri, Tri::Maybe);
    }

    #[test]
    fn a_size_change_queued_before_a_fade_leaves_the_radius_unknown() {
        // Sol's round 6: a size write queued while in the game still runs `TickEntity` after a
        // `HideUnit` with a fade (the collider stays enabled): the radius may be the new one.
        let mut b = settled(&[k(A, [10.4, 0.0, 0.0])]);
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::In)]);
        let mut a = unit(A, [10.4, 0.0, 0.0], Some(0.2));
        a.size_gen = 1;
        a.in_game = false;
        a.enabled = Enabled::Unknown;
        b.tick(&[a], RAN);
        let hits = b.overlap([0.0; 3], 10.0);
        assert_eq!((hits[0].tri, hits[0].cause), (Tri::Maybe, Some(Cause::Enabled)));
        // Back in the game, the update re-establishes the radius by the rule from an unknown one.
        a.in_game = true;
        a.enabled = Enabled::Yes;
        b.tick(&[a], RAN);
        b.tick(&[a], RAN);
        let r = next_radius(Some(RadiusIv::UNKNOWN), Some(0.2));
        assert!(r.hi < 0.26, "{r:?}");
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::Out)]);
    }

    #[test]
    fn an_update_moves_the_transform_only_when_it_isnt_vector3_equal() {
        // Sol's round 12: a collider at 9.000004; the unit moves to 10 and back to 9 between
        // two ticks: `transform.position != position` is false, the collider stays.
        let mut b = settled(&[k(A, [9.000_004, 0.0, 0.0])]);
        b.tick(&[k(A, [9.0, 0.0, 0.0])], RAN);
        assert_eq!(b.colliders[0].transform, Some([9.000_004, 0.0, 0.0]));
        b.tick(&[k(A, [9.5, 0.0, 0.0])], RAN);
        assert_eq!(b.colliders[0].transform, Some([9.5, 0.0, 0.0]));
        // From an unknown transform an update leaves it within the tolerance of the position:
        // a pair that close to the boundary is the recording's.
        let mut b = settled(&[k(A, [0.0; 3])]);
        b.tick(&[hidden(k(A, [0.0; 3]))], RAN);
        b.tick(&[k(A, [10.5, 0.0, 0.0])], RAN);
        b.tick(&[k(A, [10.5, 0.0, 0.0])], RAN);
        b.tick(&[k(A, [10.5, 0.0, 0.0])], RAN);
        // Radius 0.5 +- 0.05: the reach of a capsule of 10 is 10.45 to 10.55.
        assert_eq!(b.overlap([0.0; 3], 10.0)[0].cause, Some(Cause::Scale));
        let hits = b.overlap([0.0; 3], 10.050_005);
        assert_eq!((hits[0].tri, hits[0].cause), (Tri::Maybe, Some(Cause::Position)));
        assert_eq!(tris(&b, [0.0; 3], 10.1), vec![(A, Tri::In)]);
        assert_eq!(tris(&b, [0.0; 3], 9.9), vec![(A, Tri::Out)]);
    }

    #[test]
    fn a_collider_made_later_is_only_known_to_the_tolerance() {
        // Sol's round 13: a collider made after the first tick starts at the view's transform,
        // which may differ from the entity's position within `Vector3 ==`, and keeps it.
        let mut b = settled(&[k(A, [0.0; 3])]);
        b.tick(&[k(A, [0.0; 3]), k(B, [10.5, 0.0, 0.0])], RAN);
        b.tick(&[k(A, [0.0; 3]), k(B, [10.5, 0.0, 0.0])], RAN);
        let at = |b: &Bounds, r: f32| {
            b.overlap([20.0, 0.0, 0.0], r)
                .into_iter()
                .find(|h| h.unit == B)
                .map(|h| (h.tri, h.cause))
        };
        // Radius 0.5 +- 0.05, 9.5 away: a capsule of 9.050005 reaches within the tolerance.
        assert_eq!(at(&b, 9.050_005), Some((Tri::Maybe, Some(Cause::Position))));
        assert_eq!(at(&b, 9.1), Some((Tri::In, None)));
    }

    #[test]
    fn a_replaced_view_leaves_its_unit_s_pairs_to_the_recording() {
        // Sol's round 15: a polymorph's view swap makes a new collider (at the view's position
        // within `==`, a radius from its scale, a new place in the order) and keeps the old one
        // until its view is destroyed.
        let mut b = settled(&[k(A, [0.0; 3]), k(B, [0.0; 3])]);
        b.replace_unit(A);
        for _ in 0..3 {
            b.tick(&[k(A, [0.0; 3]), k(B, [0.0; 3])], RAN);
        }
        b.tick(&[k(A, [5.0, 0.0, 0.0]), k(B, [0.0; 3])], RAN);
        b.tick(&[k(A, [5.0, 0.0, 0.0]), k(B, [0.0; 3])], RAN);
        for at in [[0.0; 3], [-12.0, 0.0, 0.0], [300.0, 0.0, 0.0]] {
            let hits = b.overlap(at, 11.7);
            assert_eq!(
                (hits[0].unit, hits[0].tri, hits[0].cause, hits[0].order_known),
                (A, Tri::Maybe, Some(Cause::Enabled), false),
                "{at:?}"
            );
        }
        // Hidden, the old collider may still be enabled.
        b.tick(&[hidden(k(A, [5.0, 0.0, 0.0])), k(B, [0.0; 3])], RAN);
        assert_eq!(b.overlap([0.0; 3], 1.0)[0].tri, Tri::Maybe);
    }

    #[test]
    fn a_replaced_view_s_old_collider_may_take_a_buffer_slot() {
        // Sol's round 16: 512 sure colliders and a replaced one: its old collider may take a slot
        // (dropped after), so the capacity is in doubt and a set of 511 may be a truncated one.
        let units: Vec<BoundsUnit> =
            (0..513).map(|i| k(EntityIdx(i), [f32::from(i as u16) * 0.001, 0.0, 0.0])).collect();
        let mut b = Bounds::default();
        b.tick(&units, RAN);
        b.replace_unit(EntityIdx(512));
        let mut far = units.clone();
        far[512].pos = [300.0, 0.0, 0.0];
        b.tick(&far, RAN);
        b.tick(&far, RAN);
        let hits = b.overlap([0.0; 3], 5.0);
        assert!(hits.iter().all(|h| h.tri == Tri::Maybe), "the buffer may overflow");
        let set: HashSet<EntityIdx> = (1..512).map(EntityIdx).collect();
        assert!(differences(&hits, &set).diffs.is_empty());
        // Without the replaced view, 512 sure ones fit and all must be there.
        let mut b = Bounds::default();
        b.tick(&units[..512], RAN);
        let hits = b.overlap([0.0; 3], 5.0);
        assert!(hits.iter().all(|h| h.tri == Tri::In));
        assert_eq!(differences(&hits, &set).diffs, vec![Difference::Missing(EntityIdx(0))]);
        // A replaced collider is left out of the margins.
        let mut b = settled(&[k(A, [1.0, 0.0, 0.0])]);
        b.replace_unit(A);
        b.tick(&[k(A, [2.0, 0.0, 0.0])], RAN);
        b.tick(&[k(A, [2.0, 0.0, 0.0])], RAN);
        let h = b.overlap([0.0; 3], 1.0)[0];
        assert_eq!((h.margin, h.guess, h.extra_slot), (None, None, true));
    }

    #[test]
    fn forgotten_positions_stay_unknown_until_a_step_known_to_run() {
        // A possible `Physics.SyncTransforms()`: PhysX's copies are unknown until a step.
        let mut b = settled(&[k(A, [0.0; 3])]);
        b.forget_positions();
        assert_eq!(b.overlap([0.0; 3], 1.0)[0].tri, Tri::Maybe);
        b.tick(&[k(A, [0.0; 3])], Step::NotRan);
        assert_eq!(b.overlap([0.0; 3], 1.0)[0].tri, Tri::Maybe);
        b.tick(&[k(A, [0.0; 3])], Step::Unknown);
        assert_eq!(b.overlap([0.0; 3], 1.0)[0].tri, Tri::Maybe);
        b.tick(&[k(A, [0.0; 3])], RAN);
        assert_eq!(b.overlap([0.0; 3], 1.0)[0].tri, Tri::In);
    }

    #[test]
    fn a_radius_is_known_only_while_its_corpulence_is() {
        // Sol's round 7: a known radius, then a producer the sim can't follow makes the
        // corpulence unknown, and a move away and back between two ticks (which the sim can't
        // see) would update the radius: unknown until an update the sim models.
        let mut b = settled(&[k(A, [10.6, 0.0, 0.0])]);
        assert_eq!(tris(&b, [0.0; 3], 9.0), vec![(A, Tri::Out)]);
        b.tick(&[unit(A, [10.6, 0.0, 0.0], None)], RAN);
        let hits = b.overlap([0.0; 3], 9.0);
        assert_eq!((hits[0].tri, hits[0].cause), (Tri::Maybe, Some(Cause::Corpulence)));
        // Known again (the same value): still unknown, it may have changed in between.
        b.tick(&[k(A, [10.6, 0.0, 0.0])], RAN);
        assert_eq!(b.overlap([0.0; 3], 9.0)[0].tri, Tri::Maybe);
        // A different known value without an update: unknown as well.
        let mut b = settled(&[k(A, [10.6, 0.0, 0.0])]);
        b.tick(&[unit(A, [10.6, 0.0, 0.0], Some(3.0))], RAN);
        assert_eq!(b.overlap([0.0; 3], 9.0)[0].tri, Tri::Maybe);
        // An update re-establishes it.
        b.tick(&[unit(A, [10.7, 0.0, 0.0], Some(0.5))], RAN);
        b.tick(&[unit(A, [10.7, 0.0, 0.0], Some(0.5))], RAN);
        assert_eq!(tris(&b, [0.0; 3], 9.0), vec![(A, Tri::Out)]);
    }

    #[test]
    fn a_unit_that_joins_hidden_may_own_an_enabled_collider() {
        // Sol's round 7: a unit that joins in the game is queued, and hiding it before the tick
        // doesn't cancel the update, which makes its collider (enabled) while it is hidden.
        let mut b = settled(&[k(A, [0.0; 3])]);
        b.tick(&[k(A, [0.0; 3]), hidden(k(B, [0.5, 0.0, 0.0]))], RAN);
        let hits = b.overlap([0.0; 3], 1.0);
        assert_eq!(
            hits.iter().map(|h| (h.unit, h.tri, h.cause, h.order_known)).collect::<Vec<_>>(),
            vec![(A, Tri::In, None, true), (B, Tri::Maybe, Some(Cause::Enabled), false)]
        );
        assert_eq!(b.made_later, 0);
        // Back in the game: `UpdateViewActive` enables it, the update moves it, a step syncs it.
        b.tick(&[k(A, [0.0; 3]), k(B, [0.5, 0.0, 0.0])], RAN);
        assert_eq!(b.overlap([0.0; 3], 1.0)[1].tri, Tri::Maybe);
        b.tick(&[k(A, [0.0; 3]), k(B, [0.5, 0.0, 0.0])], RAN);
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::In), (B, Tri::In)]);
    }

    #[test]
    fn a_guessed_position_is_never_known() {
        // Sol's round 7: a position not from a row nor the driven agent stays unknown at every
        // step; the collider is lost until a known position and an update.
        let mut guess = k(A, [0.0; 3]);
        guess.pos_known = false;
        let mut b = settled(&[guess, k(B, [30.0, 0.0, 0.0])]);
        for _ in 0..3 {
            b.tick(&[guess, k(B, [30.0, 0.0, 0.0])], RAN);
            assert_eq!(b.overlap([0.0; 3], 1.0)[0].tri, Tri::Maybe);
            assert_eq!(b.overlap([60.0, 0.0, 0.0], 1.0)[0].tri, Tri::Maybe);
        }
        // Known: the position after a step; the radius after an update the sim sees.
        b.tick(&[k(A, [0.0; 3]), k(B, [30.0, 0.0, 0.0])], RAN);
        b.tick(&[k(A, [0.0; 3]), k(B, [30.0, 0.0, 0.0])], RAN);
        assert_eq!(b.overlap([0.0; 3], 1.0)[0].tri, Tri::In);
        assert_eq!(b.overlap([2.0, 0.0, 0.0], 1.0)[0].cause, Some(Cause::Corpulence));
    }

    #[test]
    fn a_frame_the_sim_didn_t_follow_loses_every_collider_until_its_next_update() {
        let mut b = settled(&[k(A, [10.4, 0.0, 0.0]), k(B, [0.0; 3])]);
        b.lose_track();
        // The next tick can't tell which units it updates: A stands still, so its radius stays
        // unknown; PhysX's copy is known after a step.
        b.tick(&[k(A, [10.4, 0.0, 0.0]), k(B, [0.0; 3])], RAN);
        assert!(b.overlap([0.0; 3], 10.0).iter().all(|h| h.tri == Tri::Maybe));
        b.tick(&[k(A, [10.4, 0.0, 0.0]), k(B, [0.0; 3])], RAN);
        let hits = b.overlap([0.0; 3], 10.0);
        assert_eq!(
            hits.iter().map(|h| (h.tri, h.cause)).collect::<Vec<_>>(),
            vec![(Tri::In, None), (Tri::In, None)]
        );
        assert_eq!(b.overlap([0.0; 3], 9.0)[0].cause, Some(Cause::Corpulence));
        // A collider first met after a gap may have been made and synced unseen: lost.
        let mut b2 = settled(&[k(A, [0.0; 3])]);
        b2.lose_track();
        b2.tick(&[k(A, [0.0; 3]), k(C, [40.0, 0.0, 0.0])], Step::Unknown);
        let hits = b2.overlap([0.0; 3], 1.0);
        assert_eq!((hits[1].tri, hits[1].order_known), (Tri::Maybe, false));
        assert_eq!(b2.made_later, 1);
        // A move is an update the sim sees: the radius is the band again.
        b.tick(&[k(A, [10.6, 0.0, 0.0]), k(B, [0.0; 3])], RAN);
        b.tick(&[k(A, [10.6, 0.0, 0.0]), k(B, [0.0; 3])], RAN);
        assert_eq!(tris(&b, [0.0; 3], 9.0), vec![(A, Tri::Out), (B, Tri::In)]);
    }

    #[test]
    fn more_colliders_in_reach_than_the_result_buffer_holds_go_to_the_recording() {
        let units: Vec<BoundsUnit> =
            (0..600).map(|i| k(EntityIdx(i), [f32::from(i as u16) * 0.001, 0.0, 0.0])).collect();
        let mut b = Bounds::default();
        b.tick(&units, RAN);
        let hits = b.overlap([0.0; 3], 5.0);
        assert!(hits.iter().all(|h| h.tri == Tri::Maybe && h.cause == Some(Cause::Capacity)));
        // Which 512 are kept is the recording's; any other size is impossible.
        let set = |n: u32| (0..n).map(EntityIdx).collect::<HashSet<EntityIdx>>();
        let d = differences(&hits, &set(512));
        assert!(d.diffs.is_empty() && d.open[Cause::Capacity.index()] == 600, "{d:?}");
        assert_eq!(differences(&hits, &set(600)).diffs, vec![Difference::Count(600)]);
        // Fewer than 512 means no truncation: every sure hit is in it.
        let d = differences(&hits, &set(511));
        assert_eq!(
            d.diffs,
            (511..600).map(|i| Difference::Missing(EntityIdx(i))).collect::<Vec<_>>()
        );
        // At 512 in reach the buffer holds them all.
        let mut b = Bounds::default();
        b.tick(&units[..512], RAN);
        let hits = b.overlap([0.0; 3], 5.0);
        assert!(hits.iter().all(|h| h.tri == Tri::In));
        assert!(differences(&hits, &set(512)).diffs.is_empty());
        // Sol's round 8: 500 sure and 20 uncertain colliders in reach; a set of 480 sure ones and
        // the 20 uncertain ones leaves out sure hits without being full.
        let mut mixed: Vec<BoundsUnit> = units[..500].to_vec();
        mixed.extend((500..520).map(|i| unit(EntityIdx(i), [0.0; 3], None)));
        mixed.iter_mut().skip(500).for_each(|u| u.pos = [9.0, 0.0, 0.0]);
        mixed.iter_mut().skip(500).for_each(|u| u.prefix = Some([9.0, 0.0, 0.0]));
        let mut b = Bounds::default();
        b.tick(&mixed, RAN);
        let hits = b.overlap([0.0; 3], 5.0);
        assert!(hits.iter().all(|h| h.cause == Some(Cause::Capacity)));
        let rec: HashSet<EntityIdx> = (20..520).map(EntityIdx).collect();
        assert_eq!(differences(&hits, &rec).diffs.len(), 20);
        // A full buffer may leave sure hits out.
        let full: HashSet<EntityIdx> = (8..520).map(EntityIdx).collect();
        assert!(differences(&hits, &full).diffs.is_empty());
        // A recorded set of more than 512 is impossible whatever the sim decided.
        let mut b = Bounds::default();
        b.tick(&units[..2], RAN);
        let hits = b.overlap([0.0; 3], 5.0);
        let d = differences(&hits, &set(513));
        assert_eq!(d.diffs.last(), Some(&Difference::Count(513)));
    }

    #[test]
    fn the_exact_margin_sums_the_radii_in_f64() {
        // Sol's reproducer: the f32 sum 12.4 rounds, the exact one is 11.7f32 + 0.7f32.
        let (hit, margin, rounding) =
            capsule_sphere([0.0; 3], 11.7, [11.468_463, 0.0, 4.715_331_6], 0.7);
        assert!(margin < 0.0 && margin > -1e-7, "{margin:e}");
        assert!((margin + 4.062e-8).abs() < 1e-9, "{margin:e}");
        assert_eq!(rounding, hit != (margin <= 0.0));
    }

    #[test]
    fn the_radius_is_an_interval_until_a_size_change_makes_it_exact() {
        let band = next_radius(None, Some(0.5));
        assert!((band.lo - 0.45).abs() < 1e-5 && (band.hi - 0.55).abs() < 1e-5, "{band:?}");
        // Held radii within the band stay as they are: still the interval.
        assert_eq!(next_radius(Some(band), Some(0.5)), band);
        // A small change moves the interval's hull to include the new target.
        let moved = next_radius(Some(band), Some(0.52));
        assert!(moved.lo >= 0.469 && moved.hi <= 0.571 && !moved.is_exact(), "{moved:?}");
        // A change of more than 0.1 leaves no held value within 0.05 of the new one: exact.
        let big = next_radius(Some(band), Some(0.9));
        assert!(big.is_exact() && big.lo == 0.9, "{big:?}");
        // The minimum is 0.05.
        assert_eq!(next_radius(None, Some(0.01)).lo, 0.05);
        // An unknown history (no corpulence yet) is unbounded above, and the first corpulence
        // bounds it to the band only (a radius kept from before may be 0.05 off).
        let u = next_radius(Some(RadiusIv::UNKNOWN), None);
        assert!(u.hi.is_infinite());
        let known = next_radius(Some(u), Some(0.5));
        assert!(known.hi < 0.56 && known.lo < 0.46 && !known.is_exact(), "{known:?}");
        // Unknown corpulence stays unknown.
        assert_eq!(next_radius(Some(band), None), RadiusIv::UNKNOWN);
    }

    #[test]
    fn a_pair_inside_the_scale_band_goes_to_the_recording_with_the_guess_counted() {
        // Capsule radius 10; the collider's radius is 0.5 +- 0.05: the reach is 10.45 to 10.55.
        let b =
            settled(&[k(A, [10.40, 0.0, 0.0]), k(B, [10.50, 0.0, 0.0]), k(C, [10.60, 0.0, 0.0])]);
        let hits = b.overlap([0.0; 3], 10.0);
        let t: Vec<_> = hits.iter().map(|h| (h.tri, h.cause)).collect();
        assert_eq!(t, vec![(Tri::In, None), (Tri::Maybe, Some(Cause::Scale)), (Tri::Out, None)]);
        // Its guess (scale 1) is In, and counted against the recording's answer.
        assert_eq!(hits[1].guess, Some(true));
        let yes: HashSet<EntityIdx> = [A, B].into_iter().collect();
        let d = differences(&hits, &yes);
        assert!(d.diffs.is_empty());
        assert_eq!((d.open, d.guess_agree, d.guess_differ), ([0, 0, 0, 1, 0], 1, 0));
        let no: HashSet<EntityIdx> = [A].into_iter().collect();
        let d = differences(&hits, &no);
        assert!(d.diffs.is_empty());
        assert_eq!((d.guess_agree, d.guess_differ), (0, 1));
    }

    #[test]
    fn an_unknown_corpulence_decides_only_what_the_smallest_radius_does() {
        let b = settled(&[
            unit(A, [10.2, 0.0, 0.0], None),
            unit(B, [10.8, 0.0, 0.0], None),
            unit(C, [300.0, 0.0, 0.0], None),
        ]);
        // Nothing bounds a corpulence: only a pair that overlaps with the smallest radius is decided.
        let hits = b.overlap([0.0; 3], 10.0);
        let t: Vec<_> = hits.iter().map(|h| (h.tri, h.cause)).collect();
        assert_eq!(
            t,
            vec![
                (Tri::In, None),
                (Tri::Maybe, Some(Cause::Corpulence)),
                (Tri::Maybe, Some(Cause::Corpulence))
            ]
        );
    }

    #[test]
    fn a_collider_lags_the_unit_by_one_step() {
        let mut b = Bounds::default();
        // The window's first tick: the unit stood at 0 and is at 5 now; PhysX has 0.
        let mut a = k(A, [5.0, 0.0, 0.0]);
        a.prefix = Some([0.0; 3]);
        b.tick(&[a, k(B, [0.0; 3])], RAN);
        let at = |b: &Bounds| tris(b, [0.0; 3], 1.0);
        assert_eq!(at(&b), vec![(A, Tri::In), (B, Tri::In)]);
        // Next tick it is at 9: PhysX now has 5, out of reach of a capsule of radius 1 (+0.5).
        b.tick(&[k(A, [9.0, 0.0, 0.0]), k(B, [0.0; 3])], RAN);
        assert_eq!(at(&b), vec![(A, Tri::Out), (B, Tri::In)]);
        // A frame without a step: the next one still sees 5 (two ticks old).
        b.tick(&[k(A, [1.0, 0.0, 0.0]), k(B, [0.0; 3])], Step::NotRan);
        assert_eq!(at(&b), vec![(A, Tri::Out), (B, Tri::In)]);
        // A unit appearing later sits at the origin until a step.
        let both = [k(A, [1.0, 0.0, 0.0]), k(B, [0.0; 3])];
        b.tick(&[both[0], both[1], k(C, [40.0, 0.0, 0.0])], RAN);
        assert_eq!(at(&b), vec![(A, Tri::In), (B, Tri::In), (C, Tri::In)]);
        assert_eq!(b.made_later, 1);
        b.tick(&[both[0], both[1], k(C, [40.0, 0.0, 0.0])], RAN);
        assert_eq!(at(&b)[2], (C, Tri::Out));
    }

    #[test]
    fn a_step_that_may_not_have_run_leaves_the_pairs_it_changes_to_the_recording() {
        let mut b = settled(&[k(A, [0.0; 3]), k(B, [0.0; 3])]);
        // A moves from 0 to 9; a frame whose step is unknown still sees the old tick (0) either
        // way, the next one with an unknown step has 0 or 9.
        b.tick(&[k(A, [9.0, 0.0, 0.0]), k(B, [0.0; 3])], Step::Unknown);
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::In), (B, Tri::In)]);
        b.tick(&[k(A, [9.0, 0.0, 0.0]), k(B, [0.0; 3])], Step::Unknown);
        let hits = b.overlap([0.0; 3], 1.0);
        assert_eq!(
            hits.iter().map(|h| (h.tri, h.cause)).collect::<Vec<_>>(),
            vec![(Tri::Maybe, Some(Cause::Position)), (Tri::In, None)]
        );
        // A capsule near neither: out whatever the step did.
        assert_eq!(tris(&b, [30.0, 0.0, 0.0], 1.0), vec![(A, Tri::Out), (B, Tri::Out)]);
        // The next known step settles it.
        b.tick(&[k(A, [9.0, 0.0, 0.0]), k(B, [0.0; 3])], RAN);
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::Out), (B, Tri::In)]);
        // A first tick without a known step doesn't know where the colliders were.
        let b = {
            let mut b = Bounds::default();
            b.tick(&[k(A, [0.0; 3])], Step::NotRan);
            b
        };
        assert_eq!(b.overlap([0.0; 3], 1.0)[0].tri, Tri::Maybe);
    }

    #[test]
    fn the_first_tick_makes_colliders_only_for_what_is_in_the_game() {
        // B is hidden at the window's start: it may own a disabled collider from before, so when it
        // comes into the game its position (until a step) and its place in the order are unknown.
        let mut b = Bounds::default();
        b.tick(&[k(A, [0.0; 3]), hidden(k(B, [0.0; 3])), k(C, [0.5, 0.0, 0.0])], RAN);
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::In), (C, Tri::In)]);
        b.tick(&[k(A, [0.0; 3]), hidden(k(B, [0.0; 3])), k(C, [0.5, 0.0, 0.0])], RAN);
        b.tick(&[k(A, [0.0; 3]), k(B, [0.1, 0.0, 0.0]), k(C, [0.5, 0.0, 0.0])], RAN);
        let hits = b.overlap([0.0; 3], 1.0);
        assert_eq!(
            hits.iter().map(|h| (h.unit, h.tri, h.order_known)).collect::<Vec<_>>(),
            vec![(A, Tri::In, true), (B, Tri::Maybe, false), (C, Tri::In, true)]
        );
        assert_eq!(hits[1].cause, Some(Cause::Position));
        // The next step makes the position known; the order stays unknown.
        b.tick(&[k(A, [0.0; 3]), k(B, [0.1, 0.0, 0.0]), k(C, [0.5, 0.0, 0.0])], RAN);
        let hits = b.overlap([0.0; 3], 1.0);
        assert_eq!((hits[1].tri, hits[1].order_known), (Tri::In, false));
        assert_eq!(b.made_later, 0);
    }

    #[test]
    fn disabled_colliders_and_those_that_left() {
        let mut b = Bounds::default();
        let mut a = k(A, [0.0; 3]);
        // Out of the game after having a collider: it stays, disabled at once.
        b.tick(&[a, k(C, [0.0; 3])], RAN);
        let mut c = k(C, [0.0; 3]);
        c.in_game = false;
        c.enabled = Enabled::No;
        b.tick(&[a, c], RAN);
        assert!(b.has(C));
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::In)]);
        a.enabled = Enabled::No;
        b.tick(&[a, c], RAN);
        assert!(b.overlap([0.0; 3], 1.0).is_empty());
        // An entity gone from the pool takes its collider with it, and is reported.
        let t = b.tick(&[a], RAN);
        assert_eq!(t.removed, vec![C]);
        assert!(!b.has(C));
    }

    #[test]
    fn a_collider_of_a_fade_hidden_view_goes_to_the_recording() {
        let mut b = settled(&[k(A, [0.0; 3]), k(B, [30.0, 0.0, 0.0])]);
        let mut a = k(A, [0.0; 3]);
        a.in_game = false;
        a.enabled = Enabled::Unknown;
        b.tick(&[a, k(B, [30.0, 0.0, 0.0])], RAN);
        // Its `enabled`, and where an update queued before the hide put it, are unknown.
        for at in [[0.0; 3], [5.0, 0.0, 0.0]] {
            let hits = b.overlap(at, 1.0);
            assert_eq!((hits[0].tri, hits[0].cause), (Tri::Maybe, Some(Cause::Enabled)));
            assert_eq!(hits[1].tri, Tri::Out);
        }
    }

    #[test]
    fn results_come_in_the_order_the_colliders_were_made() {
        let mut b = Bounds::default();
        let ca = [k(C, [1.0, 0.0, 0.0]), k(A, [2.0, 0.0, 0.0])];
        b.tick(&ca, RAN);
        let cab = [ca[0], ca[1], k(B, [3.0, 0.0, 0.0])];
        b.tick(&cab, RAN);
        b.tick(&cab, RAN);
        let order: Vec<EntityIdx> = tris(&b, [0.0; 3], 5.0).into_iter().map(|x| x.0).collect();
        assert_eq!(order, vec![C, A, B]);
    }

    #[test]
    fn differences_name_each_side_and_count_what_the_recording_decided() {
        let hit = |unit, tri, cause| Hit {
            unit,
            tri,
            cause,
            margin: None,
            rounding: false,
            guess: None,
            order_known: true,
            capped: None,
            extra_slot: false,
        };
        let hits = [
            hit(A, Tri::In, None),
            hit(B, Tri::Out, None),
            hit(C, Tri::Maybe, Some(Cause::Corpulence)),
        ];
        let rec: HashSet<EntityIdx> = [A, C].into_iter().collect();
        let d = differences(&hits, &rec);
        assert_eq!((d.diffs.len(), d.open), (0, [1, 0, 0, 0, 0]));
        let rec: HashSet<EntityIdx> = [B, EntityIdx(9)].into_iter().collect();
        assert_eq!(
            differences(&hits, &rec).diffs,
            vec![Difference::Missing(A), Difference::Extra(B), Difference::Extra(EntityIdx(9))]
        );
    }
}
