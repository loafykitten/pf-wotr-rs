//! Units' orientation (`docs/poc5/orientation.md`): `UnitEntityData.Orientation` (its setter
//! sets `DesiredOrientation` too), `DesiredOrientation`, `OrientationDirection`,
//! `UpdateSlowRotation`, `HasMotionThisTick`, `UnitMoveController.TickUnit`'s write from the
//! agent's `MoveDirection` and `UnitMovementAgent.InitMoveDirection`, with the managed Unity
//! members they use (`Quaternion.eulerAngles`, `Quaternion.Euler`, `Mathf.MoveTowardsAngle`,
//! `DeltaAngle`, `Repeat`, `MoveTowards`), and the writes outside `TickUnit`:
//! `GetLookAtAngle`, `LookAt`, `ForceLookAt` and the setters ([`Orientation::write`]). Pure
//! functions: `combat::movement` runs `TickUnit`'s, the commands their `TurnToTarget`s and
//! `UnitMoveTo.OnAction`'s, and the driven replay checks each against WrathOracle's `ag_look`.
//!
//! Arithmetic is the game's Mono's ([`crate::csharp::arith`]): each `float` expression in
//! double, rounded where it is stored (a local, a field, an argument or a return).
//! `Quaternion.LookRotation`, `Internal_ToEulerRad` and `Internal_FromEulerRad` are the player's
//! native code (`wr_nav::cs`, checked against it).

use wr_nav::cs;

use super::agent::m;
use crate::csharp::arith::store_f32;

pub type V3 = [f32; 3];
pub type V2 = [f32; 2];

fn d(x: f32) -> f64 {
    f64::from(x)
}

/// `UnitEntityData.m_Orientation` and `DesiredOrientation` (degrees, never normalised: the
/// setter stores what it gets, and `UpdateSlowRotation` what `Mathf.MoveTowardsAngle`
/// returns).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Orientation {
    pub value: f32,
    pub desired: f32,
}

impl Orientation {
    /// The `Orientation` setter: `m_Orientation = value; DesiredOrientation = value`.
    pub fn set(&mut self, v: f32) {
        self.value = v;
        self.desired = v;
    }

    /// `UnitEntityData.UpdateSlowRotation(maxAngle)`, given `Descriptor.State.CanRotate`.
    pub fn update_slow_rotation(&mut self, max_angle: f32, can_rotate: bool) {
        if can_rotate {
            self.value = move_towards_angle(self.value, self.desired, max_angle);
        }
    }
}

/// `Mathf.Repeat(t, length)`: `Clamp(t - Floor(t / length) * length, 0f, length)`, the
/// quotient and the difference rounded as arguments.
#[must_use]
pub fn repeat(t: f32, length: f32) -> f32 {
    let q = store_f32(d(t) / d(length));
    let fl = store_f32(d(q).floor());
    let v = store_f32(d(t) - d(fl) * d(length));
    if v < 0.0 {
        0.0
    } else if v > length {
        length
    } else {
        v
    }
}

/// `Mathf.DeltaAngle(current, target)`.
#[must_use]
pub fn delta_angle(current: f32, target: f32) -> f32 {
    let mut num = repeat(store_f32(d(target) - d(current)), 360.0);
    if num > 180.0 {
        num = store_f32(d(num) - 360.0);
    }
    num
}

/// `Mathf.MoveTowards(current, target, maxDelta)`.
#[must_use]
pub fn move_towards(current: f32, target: f32, max_delta: f32) -> f32 {
    let diff = store_f32(d(target) - d(current));
    if diff.abs() <= max_delta {
        return target;
    }
    // `Mathf.Sign`: `f >= 0f ? 1f : -1f`.
    let sign = if diff >= 0.0 { 1.0 } else { -1.0 };
    store_f32(d(current) + sign * d(max_delta))
}

/// `Mathf.MoveTowardsAngle(current, target, maxDelta)`.
#[must_use]
pub fn move_towards_angle(current: f32, target: f32, max_delta: f32) -> f32 {
    let num = delta_angle(current, target);
    if -d(max_delta) < d(num) && num < max_delta {
        return target;
    }
    let target = store_f32(d(current) + d(num));
    move_towards(current, target, max_delta)
}

/// `Quaternion.Internal_MakePositive`: each angle below -0.005729578 gets 360 added, each above
/// 360 - 0.005729578 loses 360.
#[must_use]
pub fn make_positive(mut e: V3) -> V3 {
    let neg = -0.005_729_578_f32;
    let pos = store_f32(360.0 + d(neg));
    for c in &mut e {
        if *c < neg {
            *c = store_f32(d(*c) + 360.0);
        } else if *c > pos {
            *c = store_f32(d(*c) - 360.0);
        }
    }
    e
}

/// `Quaternion.eulerAngles`: `Internal_MakePositive(Internal_ToEulerRad(this) * 57.29578f)`.
#[must_use]
pub fn euler_angles(q: [f32; 4]) -> V3 {
    make_positive(cs::to_euler_rad(q).map(|c| c * 57.295_78_f32))
}

/// `Quaternion.LookRotation(forward).eulerAngles.y` (`upwards` is `Vector3.up`).
#[must_use]
pub fn look_angle(forward: V3) -> f32 {
    euler_angles(cs::look_rotation(forward, [0.0, 1.0, 0.0]))[1]
}

/// `Quaternion.Euler(0f, orientation, 0f) * Vector3.forward`: `UnitEntityData.
/// OrientationDirection`.
#[must_use]
pub fn orientation_direction(orientation: f32) -> V3 {
    cs::quat_mul_vec(cs::quat_euler([0.0, orientation, 0.0]), [0.0, 0.0, 1.0])
}

/// `UnitMovementAgent.InitMoveDirection` for an agent with a unit:
/// `MoveDirection = Unit.EntityData.OrientationDirection.To2D()`.
#[must_use]
pub fn init_move_direction(orientation: f32) -> V2 {
    let v = orientation_direction(orientation);
    [v[0], v[2]]
}

/// `Vector3 ==`: the differences and the squared distance stored as `float` locals, below
/// `9.9999994E-11f`.
#[must_use]
pub fn vec3_eq(a: V3, b: V3) -> bool {
    let x = a[0] - b[0];
    let y = a[1] - b[1];
    let z = a[2] - b[2];
    let sq = store_f32(d(x) * d(x) + d(y) * d(y) + d(z) * d(z));
    sq < 9.999_999_4e-11_f32
}

/// The `UnitEntityData.Position` setter: `m_Position` changes only when `m_Position != value`
/// (a step shorter than 1e-5 leaves it where it was). Returns the position after.
#[must_use]
pub fn set_position(current: V3, value: V3) -> V3 {
    if vec3_eq(current, value) { current } else { value }
}

/// `UnitEntityData.HasMotionThisTick`: `Position != PreviousPosition`.
#[must_use]
pub fn has_motion(position: V3, previous: V3) -> bool {
    !vec3_eq(position, previous)
}

/// `UnitMoveController.TickUnit`'s angle argument to `UpdateSlowRotation`:
/// `deltaTime * m_AngularSpeed`.
#[must_use]
pub fn max_angle(delta_time: f32, angular_speed: f32) -> f32 {
    store_f32(d(delta_time) * d(angular_speed))
}

/// `UnitEntityData.GetLookAtAngle(point)`: `point.y = Position.y`; when `point == Position`
/// (`Vector3 ==`) the unit's `DesiredOrientation`, else `Quaternion.LookRotation(point -
/// Position).eulerAngles.y` (the difference's components each rounded at the constructor).
#[must_use]
pub fn look_at_angle(position: V3, desired: f32, mut point: V3) -> f32 {
    point[1] = position[1];
    if vec3_eq(point, position) {
        return desired;
    }
    let diff = [0, 1, 2].map(|i| store_f32(d(point[i]) - d(position[i])));
    look_angle(diff)
}

/// What writes a unit's orientation outside `UnitMoveController.TickUnit` (WrathOracle's
/// `ag_look` `fn`): `UnitEntityData.LookAt(point)`, `ForceLookAt(point)`, the `Orientation`
/// setter, the `DesiredOrientation` setter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookFn {
    LookAt,
    ForceLookAt,
    Orientation,
    Desired,
}

impl LookFn {
    /// WrathOracle's name (`ag_look`'s `fn`).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::LookAt => "LookAt",
            Self::ForceLookAt => "ForceLookAt",
            Self::Orientation => "Orientation",
            Self::Desired => "DesiredOrientation",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        [Self::LookAt, Self::ForceLookAt, Self::Orientation, Self::Desired]
            .into_iter()
            .find(|f| f.name() == s)
    }

    /// Whether its argument is a point (`LookAt`, `ForceLookAt`), else a value (the setters).
    #[must_use]
    pub fn takes_point(self) -> bool {
        matches!(self, Self::LookAt | Self::ForceLookAt)
    }
}

/// A call's argument: the point looked at, or the value set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LookArg {
    Point(V3),
    Value(f32),
}

impl Orientation {
    /// `f(arg)` of a unit at `position` without a saddled unit (a rider's `LookAt` and
    /// `ForceLookAt` turn its mount too: not modelled). `None`: the argument isn't `f`'s kind.
    #[must_use]
    pub fn write(self, f: LookFn, arg: LookArg, position: V3) -> Option<Self> {
        let mut o = self;
        match (f, arg) {
            // `DesiredOrientation = GetLookAtAngle(point)`.
            (LookFn::LookAt, LookArg::Point(p)) => {
                o.desired = look_at_angle(position, o.desired, p);
            }
            // `Orientation = GetLookAtAngle(point)` (the view's rotation when paused:
            // presentation).
            (LookFn::ForceLookAt, LookArg::Point(p)) => {
                o.set(look_at_angle(position, o.desired, p))
            }
            (LookFn::Orientation, LookArg::Value(v)) => o.set(v),
            (LookFn::Desired, LookArg::Value(v)) => o.desired = v,
            _ => return None,
        }
        Some(o)
    }
}

/// `UnitMoveController.TickUnit`'s write once the agent moved the unit (`flag`): when
/// `HasMotionThisTick && CanRotate && (!TacticalCombatHelper.IsActive || IsReallyMoving)`,
/// `forward = MoveDirection.To3D()`, and if `forward.sqrMagnitude > 0.01f`,
/// `unit.Orientation = Quaternion.LookRotation(forward).eulerAngles.y`. The gates are read
/// in that order, each only when the ones before held: `can_rotate` and `tactical_gate` (the
/// third conjunct) are called only then. Returns the angle written, if any.
///
/// # Errors
///
/// What the gates return.
pub fn tick_unit_write<E>(
    o: &mut Orientation,
    has_motion: bool,
    can_rotate: impl FnOnce() -> Result<bool, E>,
    tactical_gate: impl FnOnce() -> Result<bool, E>,
    move_direction: V2,
) -> Result<Option<f32>, E> {
    if !has_motion || !can_rotate()? || !tactical_gate()? {
        return Ok(None);
    }
    let forward = [move_direction[0], 0.0, move_direction[1]];
    if m::sqr_magnitude3(forward) > 0.01 {
        let y = look_angle(forward);
        o.set(y);
        return Ok(Some(y));
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
