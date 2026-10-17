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
//! - **Position.** The bounds controller sets a collider's transform to the entity's `Position` in
//!   its tick, but `Physics.autoSyncTransforms` is off: PhysX takes the transforms at a physics
//!   step (a fixed update, before the frame's controllers). Most frames run a step, so a query
//!   sees where the unit stood at the previous frame's tick; a frame that runs none sees the tick
//!   before. The capsule is at the querying unit's position of this frame ([`Bounds::tick`] takes
//!   whether the frame ran a step). A collider made in a tick is at the origin until the next
//!   step.
//! - **Radius.** The collider's radius is `Max(0.05, Corpulence)`. A unit whose corpulence the
//!   recording never shows has an *unknown* radius, at least [`MIN_CORPULENCE`]: a pair that range
//!   decides is the sim's, any other pair is [`Tri::Maybe`] (the recording answers).
//! - **Enabled.** `ShouldBeEnumeratedByEntityPoolEnumerator` (in the game and not suppressed), set
//!   by `UpdateViewActive` the moment either changes.
//! - **Order.** The results come in the colliders' order, the order they were made in (the pool's
//!   for the units of a loaded area, a later unit after them): the sets are refilled in that order
//!   each tick, so it is also the order `UnitGroup.ExtractNearUnits` unions them in.
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
    /// `Max(0.05, Corpulence)` with the corpulence the recording gives.
    Known(f32),
    /// A corpulence the recording never shows.
    Unknown,
}

impl Radius {
    /// The collider radius for a unit's `Corpulence` (`EntityBoundsController.ScaleRadius` with
    /// the collider's unit scale).
    #[must_use]
    pub fn of_corpulence(c: Option<f32>) -> Self {
        c.map_or(Self::Unknown, |c| Self::Known(c.max(0.05)))
    }
}

/// What the sim knows of a pair: it overlaps, it doesn't, or the radius left unknown decides.
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
    /// `IsInGame` (the entity is added to the update list only while it is).
    pub in_game: bool,
    /// `ShouldBeEnumeratedByEntityPoolEnumerator`: the collider's `enabled`.
    pub enabled: bool,
    pub radius: Radius,
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
    enabled: bool,
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
    /// set reach PhysX; then the units whose position changed (all of them on the first tick of a
    /// run, and those without a collider) move their collider's transform; `units` are the pool's,
    /// in order. The first tick of a window finds the colliders where the unit stood at the
    /// frame's start.
    pub fn tick(&mut self, units: &[BoundsUnit], step: bool) {
        if step {
            for c in &mut self.colliders {
                c.phys = c.transform;
            }
        }
        let first = !self.started;
        self.started = true;
        for u in units {
            match self.colliders.iter_mut().find(|c| c.unit == u.unit) {
                Some(c) => {
                    c.enabled = u.enabled;
                    if first || u.in_game {
                        c.transform = u.pos;
                        c.radius = u.radius;
                    }
                }
                None if first => {
                    self.colliders.push(Collider {
                        unit: u.unit,
                        phys: u.prefix,
                        transform: u.pos,
                        radius: u.radius,
                        enabled: u.enabled,
                    });
                }
                None if u.in_game => {
                    // (`AddComponent<SphereCollider>` makes the shape where the new object is,
                    // the origin; the position set after it waits for a step.)
                    self.made_later += 1;
                    self.colliders.push(Collider {
                        unit: u.unit,
                        phys: [0.0; 3],
                        transform: u.pos,
                        radius: u.radius,
                        enabled: u.enabled,
                    });
                }
                None => {}
            }
        }
    }

    /// `OverlapCapsule` for a unit at `pos` with vision radius `r`: every enabled collider, in
    /// the order they were made.
    #[must_use]
    pub fn overlap(&self, pos: V3, r: f32) -> Vec<Hit> {
        let mut out = Vec::new();
        for c in self.colliders.iter().filter(|c| c.enabled) {
            out.push(match c.radius {
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
            });
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

    fn unit(u: EntityIdx, pos: V3, radius: Radius) -> BoundsUnit {
        BoundsUnit { unit: u, pos, prefix: pos, in_game: true, enabled: true, radius }
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
        let k = Radius::Known(0.5);
        // The window's first tick: the unit stood at 0 and is at 5 now; PhysX has 0.
        let mut a = unit(A, [5.0, 0.0, 0.0], k);
        a.prefix = [0.0; 3];
        b.tick(&[a, unit(B, [0.0; 3], k)], true);
        let at = |b: &Bounds| tris(b, [0.0; 3], 1.0);
        assert_eq!(at(&b), vec![(A, Tri::In), (B, Tri::In)]);
        // Next tick it is at 9: PhysX now has 5, out of reach of a capsule of radius 1 (+0.5).
        b.tick(&[unit(A, [9.0, 0.0, 0.0], k), unit(B, [0.0; 3], k)], true);
        assert_eq!(at(&b), vec![(A, Tri::Out), (B, Tri::In)]);
        // A frame without a step: the next one still sees 5 (two ticks old).
        b.tick(&[unit(A, [1.0, 0.0, 0.0], k), unit(B, [0.0; 3], k)], false);
        assert_eq!(at(&b), vec![(A, Tri::Out), (B, Tri::In)]);
        // A unit appearing later sits at the origin until a step.
        let both = [unit(A, [1.0, 0.0, 0.0], k), unit(B, [0.0; 3], k)];
        b.tick(&[both[0], both[1], unit(C, [40.0, 0.0, 0.0], k)], true);
        assert_eq!(at(&b), vec![(A, Tri::In), (B, Tri::In), (C, Tri::In)]);
        b.tick(&[both[0], both[1], unit(C, [40.0, 0.0, 0.0], k)], true);
        assert_eq!(at(&b)[2], (C, Tri::Out));
    }

    #[test]
    fn disabled_and_not_in_game_colliders() {
        let mut b = Bounds::default();
        let k = Radius::Known(0.5);
        let mut a = unit(A, [0.0; 3], k);
        let mut c = unit(C, [0.0; 3], k);
        c.in_game = false;
        c.enabled = false;
        b.tick(&[a, c], true);
        // At the first tick every unit gets a collider, a disabled one isn't asked.
        assert!(b.has(C));
        assert_eq!(b.overlap([0.0; 3], 1.0).len(), 1);
        // A later unit that isn't in the game gets none.
        let mut d = unit(B, [0.0; 3], k);
        d.in_game = false;
        d.enabled = false;
        b.tick(&[a, c, d], true);
        assert!(!b.has(B));
        // Disabling a collider at once removes it from the query.
        a.enabled = false;
        b.tick(&[a, c], true);
        assert!(b.overlap([0.0; 3], 1.0).is_empty());
    }

    #[test]
    fn results_come_in_the_order_the_colliders_were_made() {
        let mut b = Bounds::default();
        let k = Radius::Known(0.5);
        let ca = [unit(C, [1.0, 0.0, 0.0], k), unit(A, [2.0, 0.0, 0.0], k)];
        b.tick(&ca, true);
        let cab = [ca[0], ca[1], unit(B, [3.0, 0.0, 0.0], k)];
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
                unit(A, [10.2, 0.0, 0.0], Radius::Unknown),
                unit(B, [10.8, 0.0, 0.0], Radius::Unknown),
                unit(C, [30.0, 0.0, 0.0], Radius::Unknown),
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
