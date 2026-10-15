//! The movement agent, `UnitMovementAgent` (`docs/poc5/movement.md`), in turn-based combat:
//! `TickMovement` (with `CanMoveInTurnBased` and `UnitMovementAgentBase.Move` without its
//! navmesh clamp), `SetWaypoint`, `CompleteMovement`, `InterruptMovement`, `Stop`,
//! `ResetStuck`, `UpdateVelocity` and the getters the commands read (`WantsToMove`,
//! `IsReallyMoving`, `PathFailed`, `NearTheEnd`).
//!
//! The agent is the view's, so it lives with the unit ([`super::UnitCombat::agent`]) and every
//! step reads and writes it through the world: its callbacks into the unit
//! (`OnMovementInterrupted`, `OnMovementComplete`) run the commands model, which can stop the
//! agent again before `TickMovement` goes on.
//!
//! Arithmetic is the game's Mono's ([`crate::csharp::arith`]): each `float` expression in
//! double, rounded where it is stored (a local, a field, an argument or a return). Unity's
//! managed `Vector2`/`Vector3` members are ported from `UnityEngine.CoreModule`
//! ([`m`]); `Vector3.RotateTowards` is the player's native code (`wr_nav::cs::rotate_towards`,
//! checked against it).

use super::commands::{Desync, R};
use crate::csharp::arith::store_f32;
use crate::world::{EntityIdx, World};

pub type V3 = [f32; 3];
pub type V2 = [f32; 2];

/// A path the agent holds: its key (WrathOracle's `PathSync` key, or the observer's id for a
/// path made without `AstarPath.StartPath`) and `vectorPath`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AgentPath {
    pub key: String,
    pub points: Vec<V3>,
    /// A `ForcedPath` (`ClaimPath`/`ReleasePath` skip it).
    pub forced: bool,
}

/// `m_DestinationAgent`: the agent the path leads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestAgent {
    /// Its view's `UniqueId`; `None` once the view is gone (`Unit == null`).
    pub view: Option<String>,
    /// The view still has its `EntityData`.
    pub attached: bool,
}

/// What the destination's proxy resolves to (`EntityService.GetProxy(id).Entity`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Proxy {
    /// Not a unit: the destination agent stays.
    NotUnit,
    /// A unit: the destination becomes its view's agent (`None` without a view).
    Unit(Option<String>),
}

/// `UnitMovementAgent`'s state (the fields WrathOracle's `agent` observer writes as `s`).
#[derive(Debug, Clone, PartialEq)]
pub struct Agent {
    /// `transform.position` (the view's).
    pub pos: V3,
    pub vel: V3,
    pub next_vel: V3,
    pub path: Option<AgentPath>,
    pub requested: Option<AgentPath>,
    pub requested_find: Option<AgentPath>,
    pub dest_agent: Option<DestAgent>,
    pub requested_new: bool,
    pub roaming: bool,
    pub force_roaming: bool,
    pub first_tick: bool,
    pub force: bool,
    pub follow: bool,
    pub no_more_tb: bool,
    pub repath: bool,
    pub stopping: bool,
    pub avoidance_off: bool,
    /// `ConnectedToObstacles` (the obstacle groups of real-time avoidance; their connect and
    /// disconnect also mark every agent for a repath).
    pub connected: bool,
    pub warmup: f32,
    pub remaining: f32,
    pub eta: f32,
    pub last_turn: f32,
    pub stuck_stop: f32,
    pub stuck_dir: f32,
    pub min_wp: f32,
    pub slow_down: f32,
    pub speed: f32,
    pub min_speed: f32,
    pub accel: f32,
    pub angular: f32,
    pub combat_angular: f32,
    pub approach: f32,
    pub max_approach: f32,
    pub saved_min_speed: Option<f32>,
    pub max_speed_override: Option<f32>,
    pub next_index: i32,
    pub path_frame: i32,
    pub repath_after: i32,
    pub charging: i32,
    pub next_wp: V2,
    pub next_next_wp: V2,
    pub next_dir: V2,
    pub move_dir: V2,
    pub dest: Option<V3>,
    pub no_path_dest: Option<V3>,
}

impl Default for Agent {
    /// A new agent: the serialized defaults (`m_Acceleration` 20, `m_MinSpeed` 0.2,
    /// `m_AngularSpeed` 360, `m_CombatAngularSpeed` 720) and `ResetStuck`'s minimum distance
    /// unset (`m_MinWaypointDistance` 0 until the first reset).
    fn default() -> Self {
        Self {
            pos: [0.0; 3],
            vel: [0.0; 3],
            next_vel: [0.0; 3],
            path: None,
            requested: None,
            requested_find: None,
            dest_agent: None,
            requested_new: false,
            roaming: false,
            force_roaming: false,
            first_tick: false,
            force: false,
            follow: false,
            no_more_tb: false,
            repath: false,
            stopping: false,
            avoidance_off: false,
            connected: false,
            warmup: 0.0,
            remaining: 0.0,
            eta: 0.0,
            last_turn: 0.0,
            stuck_stop: 0.0,
            stuck_dir: 0.0,
            min_wp: 0.0,
            slow_down: 0.0,
            speed: 0.0,
            min_speed: 0.2,
            accel: 20.0,
            angular: 360.0,
            combat_angular: 720.0,
            approach: 0.0,
            max_approach: 0.0,
            saved_min_speed: None,
            max_speed_override: None,
            next_index: 0,
            path_frame: 0,
            repath_after: -1,
            charging: 0,
            next_wp: [0.0; 2],
            next_next_wp: [0.0; 2],
            next_dir: [0.0; 2],
            move_dir: [0.0; 2],
            dest: None,
            no_path_dest: None,
        }
    }
}

/// What the agent reads outside itself and the calls it makes into the unit, in the order the
/// game makes them.
pub trait AgentInputs {
    /// `Path.IsDone()` of a path the agent holds.
    fn is_done(&mut self, w: &mut World, u: EntityIdx, path: &str) -> R<bool>;
    /// `Game.Instance.TurnBasedCombatController.CurrentTurn != null`.
    fn has_current_turn(&mut self, w: &mut World) -> R<bool>;
    /// `unit.IsCurrentUnit() && currentTurn.IsActing`.
    fn current_and_acting(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// `currentTurn.TickMovement(unit, ref deltaTime, isInForceMode)`: the delta it leaves.
    fn turn_tick_movement(&mut self, w: &mut World, u: EntityIdx, dt: f32, force: bool) -> R<f32>;
    /// `unit.CutsceneControlledUnit?.GetCurrentlyActive() != null`.
    fn cutscene_active(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// `unit.Commands.Move?.DontInterruptInTurnBasedMode == true`.
    fn move_dont_interrupt(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// `ObstacleAnalyzer.HasNavmeshObstacles` (a static).
    fn nav_obstacles(&mut self, w: &mut World) -> R<bool>;
    /// `SlowDown`'s gamepad gate: `Game.Instance.IsControllerGamepad`.
    fn gamepad(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// `UnityEngine.Random.Range(min, max)` (the tape's draw).
    fn random_range(&mut self, w: &mut World, u: EntityIdx, min: f32, max: f32) -> R<f32>;
    /// The agent called its own method `f` (`Stop`, `CompleteMovement`, `InterruptMovement`,
    /// `SetWaypoint` with its index): the recording's call must be it.
    fn call(&mut self, w: &mut World, u: EntityIdx, f: &'static str, index: Option<i32>) -> R<()>;
    /// `unit.EntityData.Group?.Memory != null` (`ConnectToGroups` returns before marking the
    /// agents without it).
    fn group_memory(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// `ObstaclesGroup != null` (`RemoveFromGroup` returns before marking the agents without it).
    fn obstacles_group(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// `unit.SaddledPart != null`: a mount (its rider's half of the ends isn't modelled).
    fn saddled(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// The destination's proxy, for a destination view that lost its data.
    fn dest_proxy(&mut self, w: &mut World, u: EntityIdx, view: &str) -> R<Proxy>;
    /// `UnitEntityView.IsProne`.
    fn prone(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// `RepathNeeded = true` on every agent of `UnitMovementAgentBase.AllAgents` (the enabled
    /// views'), as `ObstaclesHelper` does after changing the obstacle groups.
    fn repath_all(&mut self, w: &mut World) -> R<()>;
    /// `unit.SaddledPart?.Rider.IsInCombat == true` (a mount whose rider fights).
    fn rider_in_combat(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// `UnitAnimationManager.IsPreventingMovement` (`None`: no animation manager).
    fn anim_prevents(&mut self, w: &mut World, u: EntityIdx) -> R<Option<bool>>;
    /// `UnitEntityView.IsCommandsPreventMovement`.
    fn commands_prevent(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// `MaxSpeed` (`MaxSpeedOverride ?? CurrentSpeedMps ?? 30 ft / 3`).
    fn max_speed(&mut self, w: &mut World, u: EntityIdx) -> R<f32>;
    /// `UnitEntityData.FlyHeight`.
    fn fly_height(&mut self, w: &mut World, u: EntityIdx) -> R<f32>;
    /// `Physics.Raycast(origin, direction, out hit, max, mask)`: the hit's distance.
    fn raycast(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        o: V3,
        d: V3,
        max: f32,
        mask: i32,
    ) -> R<Option<f32>>;
    /// `PhysicUtils.HeuristicDiscCastDown(origin, radius, out hit, max, mask)`: the distance.
    fn disc_cast(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        o: V3,
        r: f32,
        max: f32,
        mask: i32,
    ) -> R<Option<f32>>;
    /// `UnitEntityData.IsInCombat`.
    fn in_combat(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// `Unit.OnMovementInterrupted(destination)` (the move command's and group command's
    /// interrupts, the interruption event).
    fn movement_interrupted(&mut self, w: &mut World, u: EntityIdx, dest: V3) -> R<()>;
    /// `Unit.OnMovementComplete()` (the movement event).
    fn movement_complete(&mut self, w: &mut World, u: EntityIdx) -> R<()>;
    /// `Unit.OnMovementWaypointUpdate(index)` (the waypoint event).
    fn waypoint_update(&mut self, w: &mut World, u: EntityIdx, index: i32) -> R<()>;
}

/// Unity's managed vector math as Mono runs it (`UnityEngine.CoreModule`): an expression in
/// double, rounded once where its value is stored or returned.
pub mod m {
    use super::{V2, V3};
    use crate::csharp::arith::store_f32;

    fn d(x: f32) -> f64 {
        f64::from(x)
    }

    /// `Vector2.Distance(a, b)`: the two differences stored, the root of their squares' sum.
    #[must_use]
    pub fn distance(a: V2, b: V2) -> f32 {
        let x = a[0] - b[0];
        let y = a[1] - b[1];
        store_f32((d(x) * d(x) + d(y) * d(y)).sqrt())
    }

    /// `Vector2.sqrMagnitude` / `Vector2.SqrMagnitude(a)`.
    #[must_use]
    pub fn sqr_magnitude(a: V2) -> f32 {
        store_f32(d(a[0]) * d(a[0]) + d(a[1]) * d(a[1]))
    }

    /// `Vector2.Dot`.
    #[must_use]
    pub fn dot(a: V2, b: V2) -> f32 {
        store_f32(d(a[0]) * d(b[0]) + d(a[1]) * d(b[1]))
    }

    /// `Vector3.sqrMagnitude`.
    #[must_use]
    pub fn sqr_magnitude3(a: V3) -> f32 {
        store_f32(d(a[0]) * d(a[0]) + d(a[1]) * d(a[1]) + d(a[2]) * d(a[2]))
    }

    /// `Vector3.magnitude`: `(float)Math.Sqrt(x * x + y * y + z * z)`.
    #[must_use]
    pub fn magnitude3(a: V3) -> f32 {
        store_f32((d(a[0]) * d(a[0]) + d(a[1]) * d(a[1]) + d(a[2]) * d(a[2])).sqrt())
    }

    /// `Vector3.Dot`.
    #[must_use]
    pub fn dot3(a: V3, b: V3) -> f32 {
        store_f32(d(a[0]) * d(b[0]) + d(a[1]) * d(b[1]) + d(a[2]) * d(b[2]))
    }

    /// `Mathf.Clamp(value, min, max)`.
    #[must_use]
    pub fn clamp(v: f32, min: f32, max: f32) -> f32 {
        if v < min {
            min
        } else if v > max {
            max
        } else {
            v
        }
    }

    /// `Mathf.Max(a, b)`: `(a > b) ? a : b`.
    #[must_use]
    pub fn max(a: f32, b: f32) -> f32 {
        if a > b { a } else { b }
    }

    /// `Vector3.Angle(from, to)` (also `Vector2.Angle`, the same code with z = 0): the
    /// magnitudes' product's root stored, then `acos` of the clamped quotient in degrees.
    #[must_use]
    pub fn angle3(from: V3, to: V3) -> f32 {
        let num = store_f32((d(sqr_magnitude3(from)) * d(sqr_magnitude3(to))).sqrt());
        if num < 1e-15 {
            return 0.0;
        }
        let num2 = clamp(store_f32(d(dot3(from, to)) / d(num)), -1.0, 1.0);
        store_f32(d(store_f32(d(num2).acos())) * d(57.295_78_f32))
    }

    /// `Vector2.Angle(from, to)`.
    #[must_use]
    pub fn angle(from: V2, to: V2) -> f32 {
        let num = store_f32((d(sqr_magnitude(from)) * d(sqr_magnitude(to))).sqrt());
        if num < 1e-15 {
            return 0.0;
        }
        let num2 = clamp(store_f32(d(dot(from, to)) / d(num)), -1.0, 1.0);
        store_f32(d(store_f32(d(num2).acos())) * d(57.295_78_f32))
    }

    /// `Vector2.Lerp(a, b, t)` with `t` clamped to [0, 1].
    #[must_use]
    pub fn lerp(a: V2, b: V2, t: f32) -> V2 {
        let t = crate::csharp::arith::clamp01(t);
        [
            store_f32(d(a[0]) + (d(b[0]) - d(a[0])) * d(t)),
            store_f32(d(a[1]) + (d(b[1]) - d(a[1])) * d(t)),
        ]
    }

    /// `Vector2.Normalize()`: divided by its magnitude above 1e-5, else zero.
    #[must_use]
    pub fn normalize(a: V2) -> V2 {
        let n = store_f32((d(a[0]) * d(a[0]) + d(a[1]) * d(a[1])).sqrt());
        if n > 1e-5 { [a[0] / n, a[1] / n] } else { [0.0, 0.0] }
    }

    /// `VectorMath.ClosestPointOnSegment(a, b, p)` (A* Pathfinding Project).
    #[must_use]
    pub fn closest_point_on_segment(a: V3, b: V3, p: V3) -> V3 {
        let v = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let sq = sqr_magnitude3(v);
        if f64::from(sq) <= 1e-6 {
            return a;
        }
        let value = store_f32(d(dot3([p[0] - a[0], p[1] - a[1], p[2] - a[2]], v)) / d(sq));
        let t = crate::csharp::arith::clamp01(value);
        [0, 1, 2].map(|i| a[i] + t * v[i])
    }

    /// `VectorMath.SqrDistancePointSegment(a, b, p)`.
    #[must_use]
    pub fn sqr_distance_point_segment(a: V3, b: V3, p: V3) -> f32 {
        let c = closest_point_on_segment(a, b, p);
        sqr_magnitude3([c[0] - p[0], c[1] - p[1], c[2] - p[2]])
    }

    /// `GeometryUtils.SqrMechanicsDistance(a, b)` (Owlcat): the XZ distance squared, or the
    /// height difference's square over 4 when that's larger and the difference above 1.5.
    #[must_use]
    pub fn sqr_mechanics_distance(a: V3, b: V3) -> f32 {
        let num = (a[1] - b[1]).abs();
        let sq = sqr_magnitude([a[0] - b[0], a[2] - b[2]]);
        if num <= 1.5 {
            return sq;
        }
        max(sq, store_f32(d(num) * d(num) / 4.0))
    }

    /// `Vector2.To3D()`: `(x, 0, y)`.
    #[must_use]
    pub fn to3d(v: V2) -> V3 {
        [v[0], 0.0, v[1]]
    }

    /// `Vector3.To2D()`: `(x, z)`.
    #[must_use]
    pub fn to2d(v: V3) -> V2 {
        [v[0], v[2]]
    }
}

fn agent(w: &World, u: EntityIdx) -> R<&Agent> {
    w.unit(u).map(|x| &x.combat.agent).ok_or_else(|| Desync(format!("agent of {u:?}: not a unit")))
}

fn agent_mut(w: &mut World, u: EntityIdx) -> R<&mut Agent> {
    w.unit_mut(u)
        .map(|x| &mut x.combat.agent)
        .ok_or_else(|| Desync(format!("agent of {u:?}: not a unit")))
}

fn d(x: f32) -> f64 {
    f64::from(x)
}

fn bad<T>(what: impl Into<String>) -> R<T> {
    Err(Desync(what.into()))
}

/// `OnLastSegment`.
fn on_last_segment(a: &Agent) -> bool {
    a.path.as_ref().is_some_and(|p| a.next_index as usize + 1 >= p.points.len())
}

/// `IsReallyMoving`: a path done (or forced, or precomputed) and past its first point.
pub fn is_really_moving(w: &mut World, u: EntityIdx, inp: &mut dyn AgentInputs) -> R<bool> {
    let a = agent(w, u)?;
    let Some(key) = a.path.as_ref().map(|p| p.key.clone()) else { return Ok(false) };
    let (force, follow, idx) = (a.force, a.follow, a.next_index);
    Ok((inp.is_done(w, u, &key)? || force || follow) && idx > 0)
}

/// `WantsToMove`: a path, or a requested one.
pub fn wants_to_move(w: &World, u: EntityIdx) -> R<bool> {
    let a = agent(w, u)?;
    Ok(a.path.is_some() || a.requested.is_some())
}

/// `ResetStuck`.
fn reset_stuck(a: &mut Agent) {
    a.stuck_stop = 0.0;
    a.stuck_dir = 0.0;
    a.min_wp = 1_000_000.0;
}

/// `Stop()`: everything about the path cleared (its log of a bad turn-based state aside), then
/// `ObstaclesHelper.ConnectToGroups`.
pub fn stop(w: &mut World, u: EntityIdx, inp: &mut dyn AgentInputs) -> R<()> {
    inp.call(w, u, "Stop", None)?;
    let a = agent_mut(w, u)?;
    a.stopping = false;
    a.path = None;
    a.requested = None;
    a.speed = 0.0;
    a.dest = None;
    a.next_index = 0;
    a.vel = [0.0; 3];
    connect_to_groups(w, u, inp)
}

/// `ObstaclesHelper.GroupingForbidden`: a unit that wants to move, or is prone.
fn grouping_forbidden(w: &mut World, u: EntityIdx, inp: &mut dyn AgentInputs) -> R<bool> {
    Ok(wants_to_move(w, u)? || inp.prone(w, u)?)
}

/// `ObstaclesHelper.ConnectToGroups(unit)` as far as the agent sees it: connected, and, for a
/// unit whose group has a memory, every agent marked for a repath. The navmesh contacts and the
/// groups themselves serve real-time avoidance only (not ported).
pub fn connect_to_groups(w: &mut World, u: EntityIdx, inp: &mut dyn AgentInputs) -> R<()> {
    if grouping_forbidden(w, u, inp)? || agent(w, u)?.connected {
        return Ok(());
    }
    agent_mut(w, u)?.connected = true;
    if !inp.group_memory(w, u)? {
        return Ok(());
    }
    inp.repath_all(w)
}

/// `ObstaclesHelper.RemoveFromGroup(unit)` as far as the agent sees it.
pub fn remove_from_group(w: &mut World, u: EntityIdx, inp: &mut dyn AgentInputs) -> R<()> {
    if !agent(w, u)?.connected {
        return Ok(());
    }
    agent_mut(w, u)?.connected = false;
    if !inp.obstacles_group(w, u)? {
        return Ok(());
    }
    inp.repath_all(w)
}

/// `InterruptMovement()`.
pub fn interrupt_movement(w: &mut World, u: EntityIdx, inp: &mut dyn AgentInputs) -> R<()> {
    inp.call(w, u, "InterruptMovement", None)?;
    if inp.saddled(w, u)? {
        return bad("InterruptMovement of a mount (its rider's interruption isn't modelled)");
    }
    if let Some(dest) = agent(w, u)?.dest {
        inp.movement_interrupted(w, u, dest)?;
    }
    stop(w, u, inp)
}

/// `CompleteMovement()`: stopped within the approach radius of the destination, else
/// interrupted; then the unit's completion.
pub fn complete_movement(w: &mut World, u: EntityIdx, inp: &mut dyn AgentInputs) -> R<()> {
    inp.call(w, u, "CompleteMovement", None)?;
    if inp.saddled(w, u)? {
        return bad(
            "CompleteMovement of a mount (its rider's stop and completion aren't modelled)",
        );
    }
    let a = agent(w, u)?;
    let reached = a.dest.is_some_and(|dest| {
        d(m::sqr_mechanics_distance(a.pos, dest)) <= d(a.approach) * d(a.approach)
    });
    if reached {
        stop(w, u, inp)?;
    } else {
        interrupt_movement(w, u, inp)?;
    }
    inp.movement_complete(w, u)
}

/// `SetWaypoint(index)` (an invalid index only logs).
pub fn set_waypoint(w: &mut World, u: EntityIdx, index: i32, inp: &mut dyn AgentInputs) -> R<()> {
    inp.call(w, u, "SetWaypoint", Some(index))?;
    let a = agent_mut(w, u)?;
    let Some(path) = a.path.as_ref() else { return Ok(()) };
    let n = path.points.len() as i32;
    if index <= 0 || index >= n {
        return Ok(());
    }
    let pts = path.points.clone();
    a.next_index = index;
    a.next_wp = m::to2d(pts[index as usize]);
    if index == n - 1 {
        a.next_next_wp = a.next_wp;
        a.next_dir = [0.0; 2];
    } else {
        a.next_next_wp = m::to2d(pts[index as usize + 1]);
        a.next_dir =
            m::normalize([a.next_next_wp[0] - a.next_wp[0], a.next_next_wp[1] - a.next_wp[1]]);
    }
    if a.repath_after >= 0 && index > a.repath_after {
        a.repath = true;
    }
    a.remaining = 0.0;
    for i in index as usize..pts.len() - 1 {
        a.remaining =
            store_f32(d(a.remaining) + d(m::distance(m::to2d(pts[i]), m::to2d(pts[i + 1]))));
    }
    if let Some(dest) = a.dest {
        let last = pts[pts.len() - 1];
        a.remaining = store_f32(d(a.remaining) + d(m::distance(m::to2d(last), m::to2d(dest))));
    }
    a.remaining -= a.approach;
    inp.waypoint_update(w, u, index)
}

/// `CanMoveInTurnBased(ref deltaTime)` (turn-based combat): the current unit moves while its
/// turn acts, its animation and commands allow it and it really moves, within the turn's
/// budget; any other unit stops, unless a cutscene controls it, its force mode is on or its
/// move command may not be interrupted.
fn can_move_in_turn_based(
    w: &mut World,
    u: EntityIdx,
    dt: &mut f32,
    inp: &mut dyn AgentInputs,
) -> R<bool> {
    let force = agent(w, u)?.force;
    let mut flag = if inp.has_current_turn(w)? {
        let moving = inp.current_and_acting(w, u)?
            && inp.anim_prevents(w, u)? != Some(true)
            && !inp.commands_prevent(w, u)?
            && is_really_moving(w, u, inp)?;
        if moving {
            *dt = inp.turn_tick_movement(w, u, *dt, force)?;
            if *dt <= 0.0 {
                false
            } else {
                if !force {
                    let a = agent_mut(w, u)?;
                    a.saved_min_speed = Some(a.min_speed);
                    a.min_speed = 1.0;
                    a.warmup = 0.0;
                    a.slow_down = 0.0;
                }
                true
            }
        } else {
            false
        }
    } else {
        !inp.in_combat(w, u)?
    };
    if !flag {
        if !force && !inp.cutscene_active(w, u)? {
            if inp.move_dont_interrupt(w, u)? {
                agent_mut(w, u)?.no_more_tb = true;
            } else {
                complete_movement(w, u, inp)?;
            }
        }
        flag = false;
    }
    Ok(flag)
}

/// `SlowDown(ref desiredSpeed)`: in turn-based combat only for a unit not in combat (and not
/// charging, and not under the gamepad's selection rule): a slow-down of 0.2 to 0.3 s drawn
/// when none runs, else kept at least 0.1 s.
fn slow_down(w: &mut World, u: EntityIdx, desired: &mut f32, inp: &mut dyn AgentInputs) -> R<()> {
    if inp.in_combat(w, u)? || agent(w, u)?.charging > 0 {
        return Ok(());
    }
    if inp.gamepad(w, u)? {
        return bad("SlowDown under a gamepad (its selection rule isn't ported)");
    }
    if agent(w, u)?.slow_down <= 0.0 {
        let t = inp.random_range(w, u, 0.2, 0.3)?;
        agent_mut(w, u)?.slow_down = t;
        *desired *= 0.7;
    } else {
        let a = agent_mut(w, u)?;
        a.slow_down = m::max(a.slow_down, 0.1);
    }
    Ok(())
}

/// `UnitMovementAgentBase.Move(p, shift, out noMove, flyHeight, hintNode)` in turn-based
/// combat (no navmesh clamp): `p + shift`, raised onto the ground by the physics queries.
pub fn move_tb(
    w: &mut World,
    u: EntityIdx,
    p: V3,
    shift: V3,
    fly: f32,
    inp: &mut dyn AgentInputs,
) -> R<V3> {
    const MASK: i32 = 2_099_457;
    let mut v = [p[0] + shift[0], p[1] + shift[1], p[2] + shift[2]];
    let v6 = v;
    let mut num2 = 0.0f32;
    let mut grounded = false;
    // `vector6 + Vector3.up * 2f`: `Vector3.up * 2f` is (0, 2, 0), added component by component.
    let origin = [v6[0] + 0.0, v6[1] + 2.0, v6[2] + 0.0];
    if let Some(dist) = inp.raycast(w, u, origin, [0.0, -1.0, 0.0], 100.0, MASK)? {
        num2 = 2.0 - dist;
        grounded = true;
    }
    if d(p[1]) - (d(v[1]) + d(num2)) > d(m::magnitude3(shift)) {
        let origin = [v6[0] + 0.0, v6[1] + 0.65, v6[2] + 0.0];
        if let Some(dist) = inp.disc_cast(w, u, origin, 0.4, 100.0, MASK)? {
            let num3 = 0.65 - dist;
            if num3 > num2 {
                num2 = num3;
            }
        }
    }
    v[1] += num2;
    if grounded {
        // `vector += Vector3.up * flyHeight`.
        let up = [0.0 * fly, 1.0 * fly, 0.0 * fly];
        v = [v[0] + up[0], v[1] + up[1], v[2] + up[2]];
    }
    Ok(v)
}

/// `MoveUnit(originalPos, shift)`: `Move` with the unit's fly height.
fn move_unit(w: &mut World, u: EntityIdx, p: V3, shift: V3, inp: &mut dyn AgentInputs) -> R<V3> {
    let fly = inp.fly_height(w, u)?;
    move_tb(w, u, p, shift, fly, inp)
}

/// `TickMovement(deltaTime)` in turn-based combat: the agent's new position (the caller stores
/// it in the view and the unit).
pub fn tick_movement(w: &mut World, u: EntityIdx, dt: f32, inp: &mut dyn AgentInputs) -> R<V3> {
    if !w.combat.turn_based {
        return bad("TickMovement outside turn-based combat (real time isn't ported)");
    }
    let position = agent(w, u)?.pos;
    {
        let a = agent_mut(w, u)?;
        if let Some(s) = a.saved_min_speed.take() {
            a.min_speed = s;
        }
    }
    let mut dt = dt;
    if !can_move_in_turn_based(w, u, &mut dt, inp)? {
        return Ok(position);
    }
    {
        let a = agent_mut(w, u)?;
        if a.warmup > 0.0 {
            a.warmup -= dt;
            return Ok(position);
        }
    }
    if inp.anim_prevents(w, u)? == Some(true) || inp.commands_prevent(w, u)? {
        return Ok(position);
    }
    if !is_really_moving(w, u, inp)? {
        return Ok(position);
    }
    // The destination agent: dropped once its view is gone; re-resolved through its proxy when
    // the view lost its data.
    if let Some(dst) = agent(w, u)?.dest_agent.clone() {
        match dst.view {
            None => agent_mut(w, u)?.dest_agent = None,
            Some(view) if !dst.attached => {
                if let Proxy::Unit(to) = inp.dest_proxy(w, u, &view)? {
                    agent_mut(w, u)?.dest_agent =
                        to.map(|v| DestAgent { view: Some(v), attached: true });
                }
            }
            Some(_) => {}
        }
    }
    let a = agent_mut(w, u)?;
    let first_tick = a.first_tick;
    let flag = a.first_tick && d(m::sqr_magnitude3(a.vel)) < 0.1;
    a.first_tick = false;
    let vector = m::to2d(position);
    let mut num = m::distance(a.next_wp, vector);
    // `(m_IsInForceMode || m_FollowPrecomputedPath) && !OnLastSegment && num < m_Speed * dt`.
    loop {
        let a = agent(w, u)?;
        if !((a.force || a.follow) && !on_last_segment(a) && d(num) < d(a.speed) * d(dt)) {
            break;
        }
        let next = a.next_index + 1;
        set_waypoint(w, u, next, inp)?;
        let a = agent_mut(w, u)?;
        num = m::distance(a.next_wp, vector);
        a.stuck_stop = 0.0;
    }
    let max_speed = inp.max_speed(w, u)?;
    if first_tick {
        agent_mut(w, u)?.speed = max_speed;
    }
    let max_speed2 = inp.max_speed(w, u)?;
    let a = agent_mut(w, u)?;
    a.eta = m::max(0.0, store_f32((d(a.remaining) + d(num)) / d(max_speed2)));
    let num2 = num / m::max(max_speed, 0.01);
    let div = m::max(num, 0.0001);
    let mut desired_dir = [(a.next_wp[0] - vector[0]) / div, (a.next_wp[1] - vector[1]) / div];
    a.stopping = false;
    let mut desired_speed = max_speed;
    let last = on_last_segment(a);
    if last && (d(max_speed) * d(max_speed)) / (2.0 * d(a.accel)) > d(num) {
        a.stopping = true;
        // `Mathf.Sqrt(float)`: the argument is rounded to float first.
        desired_speed = store_f32(d(store_f32(2.0 * d(a.accel) * d(num))).sqrt());
        if d(desired_speed) < d(max_speed) * d(a.min_speed) {
            desired_speed = max_speed * a.min_speed;
        }
    }
    if a.slow_down > 0.0 {
        a.slow_down -= dt;
        desired_speed *= 0.7;
    }
    let mut move_dir = a.move_dir;
    // No avoidance in turn-based combat.
    if d(m::angle(move_dir, desired_dir)) > d(dt) * d(a.angular) {
        slow_down(w, u, &mut desired_speed, inp)?;
    }
    let obstacles = first_tick && inp.nav_obstacles(w)?;
    let combat_mode = !obstacles && flag && inp.in_combat(w, u)?;
    let a = agent_mut(w, u)?;
    if obstacles || combat_mode || a.force || a.follow {
        move_dir = desired_dir;
    } else {
        desired_dir = m::lerp(move_dir, desired_dir, 0.5);
        let combat = inp.in_combat(w, u)?;
        let a = agent_mut(w, u)?;
        let mut num3 = if combat { a.combat_angular } else { a.angular };
        if last {
            let num4 = store_f32(
                d(m::angle3(
                    [move_dir[0], move_dir[1], 0.0],
                    [desired_dir[0], desired_dir[1], 0.0],
                )) / d(num3),
            );
            if d(num4) * 2.0 > d(num2) {
                num3 = store_f32(d(num3) * (2.0 * (d(num4) / d(num2))));
            }
        }
        let max_rad = store_f32(d(num3) * d(dt) * d(std::f32::consts::PI / 180.0));
        let r = wr_nav::cs::rotate_towards(
            [move_dir[0], move_dir[1], 0.0],
            [desired_dir[0], desired_dir[1], 0.0],
            max_rad,
            1.0,
        );
        move_dir = [r[0], r[1]];
    }
    let a = agent_mut(w, u)?;
    a.speed = desired_speed;
    let v3 = m::to3d(move_dir);
    a.next_vel = [v3[0] * a.speed, v3[1] * a.speed, v3[2] * a.speed];
    let shift = [a.next_vel[0] * dt, a.next_vel[1] * dt, a.next_vel[2] * dt];
    let mut vector3 = move_unit(w, u, position, shift, inp)?;
    let vector4 = m::to2d(vector3);
    let a = agent_mut(w, u)?;
    a.move_dir = move_dir;
    let Some(path) = a.path.as_ref() else { return bad("TickMovement: the path went away") };
    let Some(&prev) = path.points.get(a.next_index as usize - 1) else {
        return bad("TickMovement: no point before the next waypoint");
    };
    let path_last = *path.points.last().unwrap_or(&prev);
    let vector5 = m::to2d(prev);
    let vector6 = [a.next_wp[0] - vector5[0], a.next_wp[1] - vector5[1]];
    let rhs = [a.next_wp[0] - vector4[0], a.next_wp[1] - vector4[1]];
    let mut flag2 = num <= 0.0001 || m::dot(vector6, rhs) < 0.0;
    if last {
        let num5 = m::distance(vector4, a.next_wp);
        flag2 |= num5 < 0.05;
        // `(m_Roaming & intersectsObstacles) && ...`: no obstacles without avoidance.
    }
    let seg = m::sqr_distance_point_segment(
        [vector5[0], vector5[1], 0.0],
        [a.next_wp[0], a.next_wp[1], 0.0],
        [vector4[0], vector4[1], 0.0],
    );
    if seg > 4.0 {
        a.repath = true;
    }
    if flag2 {
        if last {
            // The final snap onto the path's last point, for a unit in combat (or a mount whose
            // rider is).
            if inp.in_combat(w, u)? || inp.rider_in_combat(w, u)? {
                let pos = agent(w, u)?.pos;
                let shift = [path_last[0] - pos[0], path_last[1] - pos[1], path_last[2] - pos[2]];
                vector3 = move_unit(w, u, pos, shift, inp)?;
            }
            let a = agent_mut(w, u)?;
            a.pos = vector3;
            a.move_dir = vector6;
            complete_movement(w, u, inp)?;
        } else {
            let next = agent(w, u)?.next_index + 1;
            set_waypoint(w, u, next, inp)?;
        }
    }
    let a = agent_mut(w, u)?;
    if num < a.min_wp {
        a.min_wp = num;
        a.stuck_stop = 0.0;
        a.stuck_dir = 0.0;
    }
    a.stuck_stop += dt;
    a.stuck_dir += dt;
    if a.stuck_dir > 4.0 {
        a.last_turn = 0.0;
        a.stuck_dir = 0.0;
    }
    if a.stuck_stop > 7.0 {
        interrupt_movement(w, u, inp)?;
    }
    if flag2 {
        reset_stuck(agent_mut(w, u)?);
    }
    Ok(vector3)
}

/// `UpdateVelocity()`: the next velocity while really moving or warming up, else zero.
pub fn update_velocity(w: &mut World, u: EntityIdx, inp: &mut dyn AgentInputs) -> R<()> {
    let moving = is_really_moving(w, u, inp)?;
    let a = agent_mut(w, u)?;
    a.vel = if moving || a.warmup > 0.0 { a.next_vel } else { [0.0; 3] };
    Ok(())
}

#[cfg(test)]
mod tests;
