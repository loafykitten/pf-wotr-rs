//! `AiAction.ScorePath`'s movement simulation (`docs/poc5/ai-movement.md`):
//! `UnitMovementAgent.SimulateTickMovementForTurnBased`, `MovementSimulator` and
//! `UnitMovementAgent.SegmentCircleIntersection`, in turn-based combat.
//!
//! Arithmetic is the game's Mono's ([`crate::csharp::arith`]): each `float` expression in double,
//! rounded where it is stored (a local, a field, an argument or a return); Unity's managed vector
//! members are [`agent::m`]'s. A step moves the simulator with `UnitMovementAgentBase.Move`
//! ([`agent::move_tb`], without the navmesh clamp, as in turn-based combat); what it reads outside
//! itself ([`SimInputs`]: `MaxSpeed`, `FlyHeight`, the physics queries, the grid node's
//! `AllyBlocked`, line of sight) is an input.

use super::agent::{self, AgentPhysics, V2, V3, m};
use super::commands::{Desync, R};
use crate::csharp::arith::store_f32;
use crate::world::{EntityIdx, World};

fn d(x: f32) -> f64 {
    f64::from(x)
}

fn bad<T>(what: impl Into<String>) -> R<T> {
    Err(Desync(what.into()))
}

/// `LineOfSightGeometry.EyeShift`: a mutable static initialised to `Vector3.up`, which nothing in
/// the decompiled code assigns again.
pub const EYE_SHIFT: V3 = [0.0, 1.0, 0.0];

/// `MathfInternal.FloatMinNormal`: `Mathf.Epsilon` under flush-to-zero (else `float.Epsilon`, the
/// smallest denormal). Which one the player runs with isn't established, so a comparison where the
/// two differ is reported.
const FLOAT_MIN_NORMAL: f32 = f32::MIN_POSITIVE;

/// `Vector3 - Vector3`: each component stored.
fn sub3(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `Vector3 + Vector3`.
fn add3(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `Vector3 * float`.
fn mul3(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}

/// The simulator (`Kingmaker.View.MovementSimulator`, a struct the step takes by reference).
#[derive(Debug, Clone, PartialEq)]
pub struct MovementSimulator {
    /// `m_Position` (assigning it directly skips the `Position` setter).
    pub m_position: V3,
    /// `Path.vectorPath` (the simulator holds the path; nothing changes it during the simulation).
    pub path: Vec<V3>,
    pub next_waypoint: V2,
    pub next_waypoint_3d: V3,
    pub waypoint_index: i32,
    pub last_move_length: f32,
    pub reached_max_distance: bool,
    pub approach_point: V3,
    pub approach_radius: f32,
    pub check_los: bool,
    pub ignore_blocker_radius: f32,
}

impl MovementSimulator {
    /// `new MovementSimulator(pos, p, approachPoint, approachRadius, checkLOS, ignoreBlockerRadius)`.
    #[must_use]
    pub fn new(
        pos: V3,
        path: Vec<V3>,
        approach_point: V3,
        approach_radius: f32,
        check_los: bool,
        ignore_blocker_radius: f32,
    ) -> Self {
        // An empty path: `NextWaypoint3D = Vector2.zero` (the implicit conversion: (0, 0, 0)).
        let (next_waypoint, next_waypoint_3d, waypoint_index) = match path.as_slice() {
            [] => ([0.0, 0.0], [0.0, 0.0, 0.0], 0),
            [p] => (m::to2d(*p), *p, 0),
            [_, p, ..] => (m::to2d(*p), *p, 1),
        };
        Self {
            m_position: pos,
            path,
            next_waypoint,
            next_waypoint_3d,
            waypoint_index,
            last_move_length: 0.0,
            reached_max_distance: false,
            approach_point,
            approach_radius,
            check_los,
            ignore_blocker_radius,
        }
    }

    /// `Position`'s getter.
    #[must_use]
    pub fn position(&self) -> V3 {
        self.m_position
    }

    /// `Position`'s setter: `LastMoveLength = (m_Position - value).To2D().magnitude`, then the
    /// position.
    pub fn set_position(&mut self, value: V3) {
        let diff = sub3(self.m_position, value);
        self.last_move_length = m::distance(m::to2d(diff), [0.0, 0.0]);
        self.m_position = value;
    }

    /// `vectorPath.Count` as C#'s `int`.
    fn count(&self) -> i32 {
        i32::try_from(self.path.len()).unwrap_or(i32::MAX)
    }

    /// `OnLastSegment`: `WaypointIndex == Path.vectorPath.Count - 2`.
    #[must_use]
    pub fn on_last_segment(&self) -> bool {
        self.waypoint_index == self.count() - 2
    }

    /// `AdvanceWaypoint()`: the next point, unless the simulator is at the last.
    ///
    /// # Errors
    ///
    /// The list's `ArgumentOutOfRangeException` (a negative index).
    pub fn advance_waypoint(&mut self) -> R<bool> {
        if self.waypoint_index >= self.count() - 1 {
            return Ok(false);
        }
        self.waypoint_index += 1;
        let Some(&p) = usize::try_from(self.waypoint_index).ok().and_then(|i| self.path.get(i))
        else {
            return bad(format!(
                "AdvanceWaypoint to {}: ArgumentOutOfRangeException",
                self.waypoint_index
            ));
        };
        self.next_waypoint = m::to2d(p);
        self.next_waypoint_3d = p;
        Ok(true)
    }
}

/// `UnitMovementAgent.SegmentCircleIntersection(a, b, c, radius, out intersectionT)`: `Some(t)`
/// when it returns true. Its local function's variables (`A`, `B`, `C`, `det`, `t`, and `dx`,
/// `dy`) are captured, so each assignment stores a `float`.
///
/// # Errors
///
/// `A` between the smallest denormal and `FloatMinNormal` with a determinant not negative, where
/// `Mathf.Epsilon`'s flush-to-zero setting decides.
pub fn segment_circle_intersection(a: V2, b: V2, c: V2, radius: f32) -> R<Option<f32>> {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let t = line_circle_intersection(a, c, radius, dx, dy)?;
    if t < 0.0 || t > 1.0 { Ok(None) } else { Ok(Some(t)) }
}

/// The local function `LineCircleIntersection()`.
fn line_circle_intersection(a: V2, c: V2, radius: f32, dx: f32, dy: f32) -> R<f32> {
    let (ax, ay, cx, cy) = (d(a[0]), d(a[1]), d(c[0]), d(c[1]));
    let big_a = store_f32(d(dx) * d(dx) + d(dy) * d(dy));
    let big_b = store_f32(2.0 * (d(dx) * (ax - cx) + d(dy) * (ay - cy)));
    let big_c = store_f32((ax - cx) * (ax - cx) + (ay - cy) * (ay - cy) - d(radius) * d(radius));
    let det = store_f32(d(big_b) * d(big_b) - 4.0 * d(big_a) * d(big_c));
    // `A <= Mathf.Epsilon || det < 0f`: a negative determinant, or an `A` at most the smallest
    // denormal, rejects under either epsilon; an `A` up to `FloatMinNormal` depends on it.
    if det < 0.0 || big_a <= f32::from_bits(1) {
        return Ok(-1.0);
    }
    if big_a <= FLOAT_MIN_NORMAL {
        return bad(format!(
            "SegmentCircleIntersection: A = {big_a:e} against Mathf.Epsilon (flush-to-zero not established)"
        ));
    }
    // `0f - B` (not a negation: `0 - +0` is +0).
    let minus_b = 0.0 - d(big_b);
    if det == 0.0 {
        return Ok(store_f32(minus_b / (2.0 * d(big_a))));
    }
    // `(float)(((double)(0f - B) + Math.Sqrt(det)) / (double)(2f * A))`.
    let root = d(det).sqrt();
    let num = store_f32((minus_b + root) / (2.0 * d(big_a)));
    let num2 = store_f32((minus_b - root) / (2.0 * d(big_a)));
    if num <= num2 && num > 0.0 { Ok(num) } else { Ok(num2) }
}

/// What a simulation step reads outside itself, in the order the game reads it.
pub trait SimInputs {
    /// `MaxSpeed` (`MaxSpeedOverride ?? CurrentSpeedMps ?? 30 ft / 3`).
    fn max_speed(&mut self, w: &mut World, u: EntityIdx) -> R<f32>;
    /// `(Unit?.EntityData?.FlyHeight).GetValueOrDefault()`.
    fn fly_height(&mut self, w: &mut World, u: EntityIdx) -> R<f32>;
    /// `Physics.Raycast(origin, direction, out hit, max, mask)` in `Move`: the hit's distance.
    fn raycast(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        o: V3,
        d: V3,
        max: f32,
        mask: i32,
    ) -> R<Option<f32>>;
    /// `PhysicUtils.HeuristicDiscCastDown(origin, radius, out hit, max, mask)` in `Move`.
    fn disc_cast(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        o: V3,
        r: f32,
        max: f32,
        mask: i32,
    ) -> R<Option<f32>>;
    /// `AstarPath.active.data.gridGraph.GetNearest(p).node`'s `AllyBlocked` (`None`: no node).
    fn ally_blocked(&mut self, w: &mut World, u: EntityIdx, p: V3) -> R<Option<i8>>;
    /// `LineOfSightGeometry.Instance.HasObstacle(from, to, fudgeRadius)`.
    fn has_obstacle(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        from: V3,
        to: V3,
        fudge: f32,
    ) -> R<bool>;
}

/// `agent::move_tb` reads only the two physics queries: this hands them to the step's inputs and
/// reports any other read (`Move` makes none).
struct MovePhysics<'a>(&'a mut dyn SimInputs);

impl AgentPhysics for MovePhysics<'_> {
    fn raycast(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        o: V3,
        d: V3,
        max: f32,
        mask: i32,
    ) -> R<Option<f32>> {
        self.0.raycast(w, u, o, d, max, mask)
    }
    fn disc_cast(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        o: V3,
        r: f32,
        max: f32,
        mask: i32,
    ) -> R<Option<f32>> {
        self.0.disc_cast(w, u, o, r, max, mask)
    }
}

/// `UnitMovementAgentBase.Move(p, shift, out noMove, flyHeight)` in turn-based combat (`noMove`
/// stays false there).
fn move_sim(
    w: &mut World,
    u: EntityIdx,
    p: V3,
    shift: V3,
    fly: f32,
    inp: &mut dyn SimInputs,
) -> R<V3> {
    agent::move_tb(w, u, p, shift, fly, &mut MovePhysics(inp))
}

/// `SimulateTickMovementForTurnBased(ref sim, deltaTime, maxMoveLength)` of unit `u`'s agent: the
/// simulator moved by one step; true while the simulation goes on (false at the path's end or on
/// reaching the approach radius).
///
/// # Errors
///
/// A desync from the inputs, outside turn-based combat (`Move`'s navmesh clamp isn't ported), or
/// where the game throws (`Last()` of an empty path, no grid node under the step's end).
pub fn simulate_tick_movement_for_turn_based(
    w: &mut World,
    u: EntityIdx,
    sim: &mut MovementSimulator,
    delta_time: f32,
    max_move_length: f32,
    inp: &mut dyn SimInputs,
) -> R<bool> {
    if !w.combat.turn_based {
        return bad(
            "a movement simulation step outside turn-based combat (Move's navmesh clamp isn't ported)",
        );
    }
    sim.reached_max_distance = false;
    let max_speed = inp.max_speed(w, u)?;
    let fly = inp.fly_height(w, u)?;
    let vector = m::to2d(sim.position());
    let num = m::distance(sim.next_waypoint, vector);
    // (`sim.NextWaypoint.To3D()`, discarded.)
    let next_3d = sim.next_waypoint_3d;
    // `num <= maxSpeed * deltaTime`: the product in double.
    if d(num) <= d(max_speed) * d(delta_time) {
        if !sim.advance_waypoint()? {
            // `sim.Path.vectorPath.Last()`.
            let Some(&last) = sim.path.last() else {
                return bad(
                    "Last() of an empty path in the movement simulation: InvalidOperationException",
                );
            };
            let p = move_sim(w, u, last, [0.0; 3], fly, inp)?;
            sim.set_position(p);
            return Ok(false);
        }
        let p = move_sim(w, u, next_3d, [0.0; 3], fly, inp)?;
        sim.set_position(p);
        sim.reached_max_distance = max_move_length <= 0.0;
        // (`maxMoveLength -= num` changes only the argument; `noMove` is false.)
        return Ok(true);
    }
    // `((NextWaypoint - vector) / Mathf.Max(num, 0.0001f)).To3D() * maxSpeed * deltaTime`.
    let den = m::max(num, 0.0001);
    let dir = [(sim.next_waypoint[0] - vector[0]) / den, (sim.next_waypoint[1] - vector[1]) / den];
    let mut shift = mul3(mul3(m::to3d(dir), max_speed), delta_time);
    if m::magnitude3(shift) >= max_move_length {
        // `shift *= maxMoveLength / shift.magnitude`: the quotient an argument (stored).
        let k = store_f32(d(max_move_length) / d(m::magnitude3(shift)));
        shift = mul3(shift, k);
        sim.reached_max_distance = true;
    } else {
        sim.reached_max_distance = false;
    }
    let mut vector2 = move_sim(w, u, sim.position(), shift, fly, inp)?;
    // (`noMove` is false in turn-based combat.)
    let mut flag = false;
    // `GetNearest(vector2).node`, read before the radius test; `AllyBlocked` only after it.
    let node = inp.ally_blocked(w, u, vector2)?;
    if sim.approach_radius > 0.0 {
        let Some(blocked) = node else {
            return bad(
                "the movement simulation's GetNearest found no node: NullReferenceException",
            );
        };
        if blocked == 0 {
            let mut vector3 = vector2;
            let r = d(sim.approach_radius);
            let mut flag2 =
                d(m::sqr_mechanics_distance(sim.approach_point, sim.position())) <= r * r;
            if !flag2
                && let Some(t) = segment_circle_intersection(
                    vector,
                    m::to2d(vector2),
                    m::to2d(sim.approach_point),
                    sim.approach_radius,
                )?
            {
                flag2 = true;
                let t = m::clamp(store_f32(d(t) + d(0.0001_f32)), 0.0, 1.0);
                let p = sim.position();
                vector3 = add3(p, mul3(sub3(vector2, p), t));
            }
            if flag2
                && (!sim.check_los
                    || !inp.has_obstacle(
                        w,
                        u,
                        add3(vector3, EYE_SHIFT),
                        sim.approach_point,
                        sim.ignore_blocker_radius,
                    )?)
            {
                vector2 = vector3;
                flag = true;
            }
        }
    }
    sim.set_position(vector2);
    sim.reached_max_distance = sim.last_move_length >= max_move_length;
    Ok(!flag)
}

#[cfg(test)]
mod tests;
