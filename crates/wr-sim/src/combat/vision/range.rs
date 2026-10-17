//! `UnitVisionRangeController` over `EntityBoundsController`'s sphere colliders (decompiled, not
//! copied; docs/poc5/vision.md): the units an awake unit may see (`CanBeInVisionRange`).
//!
//! The game keeps one `SphereCollider` per entity with bounds in a private physics scene
//! (`EntityBoundsController`, registered just before the vision controller). The vision controller
//! asks that scene, for each awake unit that isn't an extra, which unit colliders (layer
//! `SphereBoundsUnit`, only enabled ones) overlap the vertical capsule from `pos - up * 2r` to
//! `pos + up * 2r` with radius `r = VisionRangeMeters`, and keeps their units.
//!
//! What the model holds, and what the recordings fixed (docs/poc5/vision.md):
//! - **Population.** `State.EntitiesWithBounds` in the order its entities joined their states; the
//!   first tick (`m_UpdateAll`) walks it through `EntityPoolEnumerator`, which skips entities that are
//!   not `ShouldBeEnumeratedByEntityPoolEnumerator` (in the game and not suppressed): a unit hidden
//!   at that tick gets its collider only when it later comes into the game, in the order of
//!   the update list's insertions (taken to be the pool's order), after the others. A collider is
//!   destroyed with its view, which the sim doesn't know the time of: [`Tick::removed`].
//! - **Position.** The bounds controller sets a collider's transform to the entity's `Position` in
//!   its tick, but `Physics.autoSyncTransforms` is off: PhysX takes the transforms at a physics
//!   step (a fixed update, before the frame's controllers). Most frames run a step, so a query
//!   sees where the unit stood at the previous frame's tick; a frame that runs none sees the tick
//!   before. The capsule is at the querying unit's position of this frame ([`Bounds::tick`] takes
//!   whether the frame ran a step). A collider made in a tick is at the origin until the next
//!   step.
//! - **Radius.** A collider is made with `Max(0.05, Corpulence * view scale)` and, in the same
//!   tick, set to `Max(0.05, Corpulence)` (its own transform has unit scale) if that is more than
//!   0.05 away; later ticks of the entity (position, game state or size changed) do the same
//!   against the radius it has. The sim has the corpulence from the recording and the view's scale
//!   only as an assumption (1: [`BoundsUnit::view_scale`]). A unit whose corpulence the recording
//!   doesn't show yet has an *unknown* radius, at least [`MIN_CORPULENCE`]: a pair that range
//!   decides is the sim's, any other pair is [`Tri::Maybe`] (the recording answers).
//! - **Enabled.** `ShouldBeEnumeratedByEntityPoolEnumerator` (in the game and not suppressed), set
//!   by `UpdateViewActive` the moment either changes, except while the view fade-hides
//!   (`m_IsFadeHide`): the collider then keeps its state until the fade ends, which the sim
//!   doesn't know ([`Enabled::Unknown`]: a pair in reach is [`Tri::Maybe`]).
//! - **Order.** The results come in the colliders' order, the order they were made in, so it is
//!   also the order `UnitGroup.ExtractNearUnits` unions the sets in (they are refilled in it each
//!   tick).
//!
//! The overlap itself is PhysX's sphere against capsule: the squared distance from the sphere's
//! centre to the capsule's segment against the squared sum of the radii, in `f32`
//! (`Gu::distancePointSegmentSquared`).

use std::collections::HashSet;

use crate::world::EntityIdx;

use super::V3;

/// `UnitEntityView.Corpulence` is at least this (`Mathf.Max(.., 0.5f)`).
pub const MIN_CORPULENCE: f32 = 0.5;

/// The largest corpulence a unit is assumed to have when the recording never shows it (an
/// inference: the biggest creatures of the game stay under it).
pub const MAX_CORPULENCE: f32 = 4.0;

/// A collider's radius as far as the sim knows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Radius {
    /// The radius the game's rules give, from the corpulence the recording shows.
    Known(f32),
    /// A corpulence the recording doesn't show.
    Unknown,
}

/// `ScaleRadius(radius, lossyScale)` with the scale's larger horizontal component `s`.
fn scale_radius(radius: f32, s: f32) -> f32 {
    (radius * s).max(0.05)
}

/// What a tick of `EntityBoundsController` does to a unit's collider radius: `radius` is what it
/// has (`None`: not made yet), `corpulence` the unit's now and `view_scale` the view's
/// `lossyScale` (larger horizontal component).
fn next_radius(radius: Option<Radius>, corpulence: Option<f32>, view_scale: f32) -> Radius {
    let Some(c) = corpulence else { return Radius::Unknown };
    // The collider's own transform has unit scale.
    let target = scale_radius(c, 1.0);
    let have = match radius {
        Some(Radius::Known(r)) => r,
        // Made now: from the view's scale.
        None => scale_radius(c, view_scale),
        // The earlier radius is lost with the corpulence: the rule's target.
        Some(Radius::Unknown) => return Radius::Known(target),
    };
    Radius::Known(if (have - target).abs() > 0.05 { target } else { have })
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

/// What the sim knows of a pair: it overlaps, it doesn't, or the radius left unknown (or the
/// collider's enabled state) decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tri {
    In,
    Out,
    Maybe,
}

/// A unit as the bounds controller sees it at its tick.
#[derive(Debug, Clone, Copy)]
pub struct BoundsUnit {
    pub unit: EntityIdx,
    /// `Position` now.
    pub pos: V3,
    /// `Position` at the frame's start: where the first tick of a window finds the collider (the
    /// previous frame's tick put it there).
    pub prefix: V3,
    /// `IsInGame` (the entity is added to the update list only while it is, and the first tick
    /// walks only the entities that are).
    pub in_game: bool,
    /// What `ShouldBeEnumeratedByEntityPoolEnumerator` and the fade make of the collider's `enabled`.
    pub enabled: Enabled,
    /// `Corpulence` now, when the recording shows it.
    pub corpulence: Option<f32>,
    /// The view's `lossyScale` (larger horizontal component) a collider is made with: not known to
    /// the sim, assumed 1.
    pub view_scale: f32,
}

/// One `SphereCollider`.
#[derive(Debug, Clone)]
struct Collider {
    unit: EntityIdx,
    /// Where PhysX has the collider (queries use it).
    phys: V3,
    /// The collider's transform (`transform.position`, set by the tick; PhysX takes it at the
    /// next step).
    transform: V3,
    radius: Radius,
    enabled: Enabled,
    /// The unit's state at the collider's last tick: what the update list reacts to.
    seen_in_game: bool,
    seen_corpulence: Option<f32>,
}

/// One pair of a query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub unit: EntityIdx,
    pub tri: Tri,
    /// For a known radius: the distance from the capsule's segment to the sphere's surface beyond
    /// the capsule's radius, in metres and `f64` (negative: overlapping).
    pub margin: Option<f64>,
    /// The `f32` decision differs from the exact one (only for a known radius).
    pub rounding: bool,
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
    let margin = dist - f64::from(sum);
    (hit, margin, hit != (margin <= 0.0))
}

impl Bounds {
    /// `EntityBoundsController.Tick` (in the game's order: before the vision controller): when the
    /// frame ran a physics step (`step`, before its controllers), the transforms the previous ticks
    /// set reach PhysX; then the entities in the update list move their collider's transform and
    /// update its radius: on the first tick of a run all of them that are in the game (a collider
    /// is made for those without one), later those whose position, game state or corpulence
    /// changed (a collider is made for an entity that is in the game). `units` are the pool's, in
    /// order. The first tick of a window finds the colliders where the unit stood at the
    /// frame's start.
    pub fn tick(&mut self, units: &[BoundsUnit], step: bool) -> Tick {
        if step {
            for c in &mut self.colliders {
                c.phys = c.transform;
            }
        }
        let first = !self.started;
        self.started = true;
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
            match self.colliders.iter_mut().find(|c| c.unit == u.unit) {
                Some(c) => {
                    c.enabled = u.enabled;
                    let listed = u.in_game
                        && (first
                            || c.transform != u.pos
                            || !c.seen_in_game
                            || c.seen_corpulence.map(f32::to_bits)
                                != u.corpulence.map(f32::to_bits));
                    if listed {
                        c.transform = u.pos;
                        c.radius = next_radius(Some(c.radius), u.corpulence, u.view_scale);
                        c.seen_corpulence = u.corpulence;
                    }
                    c.seen_in_game = u.in_game;
                }
                // (`EntityPoolEnumerator` skips what isn't `ShouldBeEnumerated` at the first tick;
                // the entity gets its collider when it comes into the game.)
                None if u.in_game => {
                    if !first {
                        self.made_later += 1;
                    }
                    self.colliders.push(Collider {
                        unit: u.unit,
                        // At the first tick the collider exists since before; later
                        // `AddComponent<SphereCollider>` makes the shape where the new object is,
                        // the origin, and the position set after it waits for a step.
                        phys: if first { u.prefix } else { [0.0; 3] },
                        transform: u.pos,
                        radius: next_radius(None, u.corpulence, u.view_scale),
                        enabled: u.enabled,
                        seen_in_game: true,
                        seen_corpulence: u.corpulence,
                    });
                }
                None => {}
            }
        }
        out
    }

    /// `OverlapCapsule` for a unit at `pos` with vision radius `r`: every collider that is or may
    /// be enabled, in the order they were made.
    #[must_use]
    pub fn overlap(&self, pos: V3, r: f32) -> Vec<Hit> {
        let mut out = Vec::new();
        for c in self.colliders.iter().filter(|c| c.enabled != Enabled::No) {
            let mut hit = match c.radius {
                Radius::Known(rs) => {
                    let (hit, margin, rounding) = capsule_sphere(pos, r, c.phys, rs);
                    Hit {
                        unit: c.unit,
                        tri: if hit { Tri::In } else { Tri::Out },
                        margin: Some(margin),
                        rounding,
                    }
                }
                Radius::Unknown => {
                    let small = capsule_sphere(pos, r, c.phys, MIN_CORPULENCE).0;
                    let big = capsule_sphere(pos, r, c.phys, MAX_CORPULENCE).0;
                    let tri = match (small, big) {
                        (true, _) => Tri::In,
                        (false, false) => Tri::Out,
                        (false, true) => Tri::Maybe,
                    };
                    Hit { unit: c.unit, tri, margin: None, rounding: false }
                }
            };
            if c.enabled == Enabled::Unknown && hit.tri != Tri::Out {
                hit.tri = Tri::Maybe;
            }
            out.push(hit);
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
}

/// The differences between a query's hits and a recorded set, and how many pairs the recording
/// decided ([`Tri::Maybe`]).
#[must_use]
pub fn differences(hits: &[Hit], recorded: &HashSet<EntityIdx>) -> (Vec<Difference>, usize) {
    let mut out = Vec::new();
    let mut maybe = 0;
    let mut seen = HashSet::new();
    for h in hits {
        seen.insert(h.unit);
        match (h.tri, recorded.contains(&h.unit)) {
            (Tri::In, false) => out.push(Difference::Missing(h.unit)),
            (Tri::Out, true) => out.push(Difference::Extra(h.unit)),
            (Tri::Maybe, _) => maybe += 1,
            _ => {}
        }
    }
    let mut rest: Vec<EntityIdx> = recorded.iter().copied().filter(|u| !seen.contains(u)).collect();
    rest.sort();
    out.extend(rest.into_iter().map(Difference::Extra));
    (out, maybe)
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: EntityIdx = EntityIdx(1);
    const B: EntityIdx = EntityIdx(2);
    const C: EntityIdx = EntityIdx(3);

    /// A unit in the game with a corpulence (`None`: unknown), its view's scale 1.
    fn unit(u: EntityIdx, pos: V3, corpulence: Option<f32>) -> BoundsUnit {
        BoundsUnit {
            unit: u,
            pos,
            prefix: pos,
            in_game: true,
            enabled: Enabled::Yes,
            corpulence,
            view_scale: 1.0,
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
    fn a_collider_lags_the_unit_by_one_step() {
        let mut b = Bounds::default();
        // The window's first tick: the unit stood at 0 and is at 5 now; PhysX has 0.
        let mut a = k(A, [5.0, 0.0, 0.0]);
        a.prefix = [0.0; 3];
        b.tick(&[a, k(B, [0.0; 3])], true);
        let at = |b: &Bounds| tris(b, [0.0; 3], 1.0);
        assert_eq!(at(&b), vec![(A, Tri::In), (B, Tri::In)]);
        // Next tick it is at 9: PhysX now has 5, out of reach of a capsule of radius 1 (+0.5).
        b.tick(&[k(A, [9.0, 0.0, 0.0]), k(B, [0.0; 3])], true);
        assert_eq!(at(&b), vec![(A, Tri::Out), (B, Tri::In)]);
        // A frame without a step: the next one still sees 5 (two ticks old).
        b.tick(&[k(A, [1.0, 0.0, 0.0]), k(B, [0.0; 3])], false);
        assert_eq!(at(&b), vec![(A, Tri::Out), (B, Tri::In)]);
        // A unit appearing later sits at the origin until a step.
        let both = [k(A, [1.0, 0.0, 0.0]), k(B, [0.0; 3])];
        b.tick(&[both[0], both[1], k(C, [40.0, 0.0, 0.0])], true);
        assert_eq!(at(&b), vec![(A, Tri::In), (B, Tri::In), (C, Tri::In)]);
        assert_eq!(b.made_later, 1);
        b.tick(&[both[0], both[1], k(C, [40.0, 0.0, 0.0])], true);
        assert_eq!(at(&b)[2], (C, Tri::Out));
    }

    #[test]
    fn the_first_tick_makes_colliders_only_for_what_is_in_the_game() {
        let mut b = Bounds::default();
        // B is hidden since the load: it gets no collider, so C's is made before the one B gets
        // when it comes into the game.
        b.tick(&[k(A, [0.0; 3]), hidden(k(B, [0.0; 3])), k(C, [0.5, 0.0, 0.0])], true);
        assert!(!b.has(B));
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::In), (C, Tri::In)]);
        // A hidden unit stays without one, and comes into the game later.
        b.tick(&[k(A, [0.0; 3]), hidden(k(B, [0.0; 3])), k(C, [0.5, 0.0, 0.0])], true);
        assert!(!b.has(B));
        b.tick(&[k(A, [0.0; 3]), k(B, [0.1, 0.0, 0.0]), k(C, [0.5, 0.0, 0.0])], true);
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::In), (C, Tri::In), (B, Tri::In)]);
        // (At the origin until a step: the unit stands at the origin here too, so make it far.)
        let far = k(B, [40.0, 0.0, 0.0]);
        let mut b = Bounds::default();
        b.tick(&[k(A, [0.0; 3]), hidden(far)], true);
        b.tick(&[k(A, [0.0; 3]), far], false);
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::In), (B, Tri::In)]);
        b.tick(&[k(A, [0.0; 3]), far], true);
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::In), (B, Tri::Out)]);
    }

    #[test]
    fn disabled_colliders_and_those_that_left() {
        let mut b = Bounds::default();
        let mut a = k(A, [0.0; 3]);
        // Out of the game after having a collider: it stays, disabled at once.
        b.tick(&[a, k(C, [0.0; 3])], true);
        let mut c = k(C, [0.0; 3]);
        c.in_game = false;
        c.enabled = Enabled::No;
        b.tick(&[a, c], true);
        assert!(b.has(C));
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::In)]);
        a.enabled = Enabled::No;
        b.tick(&[a, c], true);
        assert!(b.overlap([0.0; 3], 1.0).is_empty());
        // An entity gone from the pool takes its collider with it, and is reported.
        let t = b.tick(&[a], true);
        assert_eq!(t.removed, vec![C]);
        assert!(!b.has(C));
    }

    #[test]
    fn a_collider_of_a_fade_hidden_view_is_unknown_when_in_reach() {
        let mut b = Bounds::default();
        let mut a = k(A, [0.0; 3]);
        b.tick(&[a, k(B, [30.0, 0.0, 0.0])], true);
        a.in_game = false;
        a.enabled = Enabled::Unknown;
        b.tick(&[a, k(B, [30.0, 0.0, 0.0])], true);
        // In reach: the recording decides; out of reach: out whatever the fade does.
        assert_eq!(tris(&b, [0.0; 3], 1.0), vec![(A, Tri::Maybe), (B, Tri::Out)]);
        assert_eq!(tris(&b, [5.0, 0.0, 0.0], 1.0), vec![(A, Tri::Out), (B, Tri::Out)]);
    }

    #[test]
    fn the_radius_keeps_the_view_scale_within_a_twentieth_and_follows_later_changes() {
        let at = |c: f32, scale: f32| next_radius(None, Some(c), scale);
        // Made from the corpulence times the view's scale, corrected only when more than 0.05 off.
        assert_eq!(at(0.5, 1.0), Radius::Known(0.5));
        assert_eq!(at(0.5, 1.05), Radius::Known(0.525));
        assert_eq!(at(0.5, 1.2), Radius::Known(0.5));
        assert_eq!(at(0.5, 0.92), Radius::Known(0.5 * 0.92));
        // The minimum is 0.05.
        assert_eq!(at(0.01, 1.0), Radius::Known(0.05));
        // Later ticks compare with the radius it has.
        assert_eq!(next_radius(Some(Radius::Known(0.525)), Some(0.5), 1.0), Radius::Known(0.525));
        assert_eq!(next_radius(Some(Radius::Known(0.525)), Some(0.9), 1.0), Radius::Known(0.9));
        assert_eq!(next_radius(Some(Radius::Known(0.525)), None, 1.0), Radius::Unknown);
        assert_eq!(next_radius(Some(Radius::Unknown), Some(0.7), 1.2), Radius::Known(0.7));
        // In a bounds tick: made at 1.05, kept while the position changes, corrected by a size change.
        let mut b = Bounds::default();
        let mut a = k(A, [10.52, 0.0, 0.0]);
        a.view_scale = 1.05;
        b.tick(&[a], true);
        // Sphere radius 0.525: the margin at 10 + 0.525 = 10.525 is exactly the sphere's reach.
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::In)]);
        a.pos = [10.53, 0.0, 0.0];
        b.tick(&[a], true);
        b.tick(&[a], true);
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::Out)]);
        a.corpulence = Some(0.9);
        b.tick(&[a], true);
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::In)]);
    }

    #[test]
    fn results_come_in_the_order_the_colliders_were_made() {
        let mut b = Bounds::default();
        let ca = [k(C, [1.0, 0.0, 0.0]), k(A, [2.0, 0.0, 0.0])];
        b.tick(&ca, true);
        let cab = [ca[0], ca[1], k(B, [3.0, 0.0, 0.0])];
        b.tick(&cab, true);
        b.tick(&cab, true);
        let order: Vec<EntityIdx> = tris(&b, [0.0; 3], 5.0).into_iter().map(|x| x.0).collect();
        assert_eq!(order, vec![C, A, B]);
    }

    #[test]
    fn an_unknown_radius_decides_only_what_it_can() {
        let mut b = Bounds::default();
        b.tick(
            &[
                unit(A, [10.2, 0.0, 0.0], None),
                unit(B, [10.8, 0.0, 0.0], None),
                unit(C, [30.0, 0.0, 0.0], None),
            ],
            true,
        );
        // Capsule radius 10: reaches 10.5 with the smallest sphere, 14 with the biggest.
        assert_eq!(tris(&b, [0.0; 3], 10.0), vec![(A, Tri::In), (B, Tri::Maybe), (C, Tri::Out)]);
    }

    #[test]
    fn differences_name_each_side_and_pass_the_recording_for_unknown_radii() {
        let hit = |unit, tri| Hit { unit, tri, margin: None, rounding: false };
        let hits = [hit(A, Tri::In), hit(B, Tri::Out), hit(C, Tri::Maybe)];
        let rec: HashSet<EntityIdx> = [A, C].into_iter().collect();
        assert_eq!(differences(&hits, &rec), (Vec::new(), 1));
        let rec: HashSet<EntityIdx> = [B, EntityIdx(9)].into_iter().collect();
        assert_eq!(
            differences(&hits, &rec).0,
            vec![Difference::Missing(A), Difference::Extra(B), Difference::Extra(EntityIdx(9))]
        );
    }
}
