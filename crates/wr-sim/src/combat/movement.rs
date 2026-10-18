//! `UnitMoveController.TickUnit` in turn-based combat (`docs/poc5/movement.md`): the gate on
//! the agent's `IsReallyMoving` and the unit's `CanMove` (an interrupted move when it can't),
//! the agent's `TickMovement`, the unit's and the view's positions, the orientation the move
//! writes (`combat::orientation`), the place-on-ground step of a standing unit (physics: an
//! input), `UpdateSlowRotation` and `UpdateVelocity`. Presentation (the camera, IK, the view's
//! rotation) is left out.

use super::agent::{self, AgentInputs, V3};
use super::commands::{Desync, R};
use super::orientation::{self, Orientation};
use crate::world::{EntityIdx, World};

/// What `TickUnit` reads outside the agent, and its calls into the unit.
pub trait MoveInputs: AgentInputs {
    /// `unit.Descriptor.State.CanMove` (each evaluation).
    fn can_move(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// `unit.Descriptor.State.CanRotate` (each evaluation).
    fn can_rotate(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// `unit.Commands.InterruptMove()` (the commands model).
    fn interrupt_move(&mut self, w: &mut World, u: EntityIdx) -> R<()>;
    /// `UnitPlaceOnGroundController.ForcedTick(unit)` (physics): the unit's position after.
    /// `required`: `TickUnit` calls it (the view stood elsewhere); else it may (a unit going
    /// prone or on an elevator, which the sim doesn't model): `None` when it didn't.
    /// The unit's position after and the view's (the view takes the computed position even where
    /// the unit's setter keeps the old one).
    fn place_on_ground(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        required: bool,
    ) -> R<Option<(V3, V3)>>;
    /// `UpdateSlowRotation(maxAngle)`'s `DesiredOrientation` read: `desired` is the sim's (its
    /// writers: `docs/poc5/orientation.md`), `orientation` and `max_angle` too; the answer is the
    /// value used (the sim's, checked, or the recording's where the writes aren't recorded).
    fn desired_orientation(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        orientation: f32,
        desired: f32,
        max_angle: f32,
    ) -> R<f32>;
    /// `Game.Instance.IsPaused`.
    fn paused(&mut self, w: &mut World) -> R<bool>;
    /// The unit has a mount's or rider's part (`UnitPartSaddled`: not modelled).
    fn saddled_unit(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// `unit.View.Or(null)?.MovementAgent != null` (a `UnitMovementAgent`: no other kind is
    /// modelled).
    fn has_agent(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
}

fn bad<T>(what: impl Into<String>) -> R<T> {
    Err(Desync(what.into()))
}

/// `UnitMoveController.TickUnit(unit)` in turn-based combat.
pub fn tick_unit(w: &mut World, u: EntityIdx, inp: &mut dyn MoveInputs) -> R<()> {
    if !w.combat.turn_based {
        return bad(
            "UnitMoveController.TickUnit outside turn-based combat (real time isn't ported)",
        );
    }
    let dt = w.clock.delta_time();
    let Some(unit) = w.unit_mut(u) else { return bad("TickUnit of an entity that isn't a unit") };
    let previous = unit.position;
    unit.combat.prev_position = previous;
    if !inp.has_agent(w, u)? {
        return Ok(());
    }
    let moving = agent::is_really_moving(w, u, inp)?;
    let flag = moving && inp.can_move(w, u)?;
    if agent::is_really_moving(w, u, inp)? && !inp.can_move(w, u)? {
        inp.interrupt_move(w, u)?;
    }
    if flag {
        let v = agent::tick_movement(w, u, dt, inp)?;
        let unit = w.unit_mut(u).ok_or_else(|| Desync("no unit".into()))?;
        unit.combat.agent.pos = v;
        unit.position = orientation::set_position(unit.position, v);
        let has_motion = orientation::has_motion(unit.position, previous);
        let move_dir = unit.combat.agent.move_dir;
        let mut o =
            Orientation { value: unit.orientation, desired: unit.combat.desired_orientation };
        crate::combat::vision::note_position_write(w, u);
        // `!TacticalCombatHelper.IsActive || IsReallyMoving`: tactical combat isn't modelled
        // (and is never active in these fights), so the gate holds.
        orientation::tick_unit_write(
            &mut o,
            has_motion,
            || inp.can_rotate(w, u),
            || Ok(true),
            move_dir,
        )?;
        let unit = w.unit_mut(u).ok_or_else(|| Desync("no unit".into()))?;
        unit.orientation = o.value;
        unit.combat.desired_orientation = o.desired;
    } else {
        let unit = w.unit_mut(u).ok_or_else(|| Desync("no unit".into()))?;
        let (view, pos) = (unit.combat.agent.pos, unit.position);
        let sq = agent::m::sqr_magnitude3([view[0] - pos[0], view[1] - pos[1], view[2] - pos[2]]);
        if sq > 0.0 {
            unit.combat.agent.pos = pos;
        }
        if let Some((after, view)) = inp.place_on_ground(w, u, sq > 0.0)? {
            let unit = w.unit_mut(u).ok_or_else(|| Desync("no unit".into()))?;
            unit.position = after;
            unit.combat.agent.pos = view;
            crate::combat::vision::note_position_write(w, u);
        }
    }
    if inp.saddled_unit(w, u)? {
        return bad("TickUnit of a mount or rider (the rider's copy isn't modelled)");
    }
    let unit = w.unit(u).ok_or_else(|| Desync("no unit".into()))?;
    let max = orientation::max_angle(dt, unit.combat.agent.angular);
    let (value, desired) = (unit.orientation, unit.combat.desired_orientation);
    let desired = inp.desired_orientation(w, u, value, desired, max)?;
    let mut o = Orientation { value, desired };
    let can_rotate = inp.can_rotate(w, u)?;
    o.update_slow_rotation(max, can_rotate);
    let unit = w.unit_mut(u).ok_or_else(|| Desync("no unit".into()))?;
    unit.orientation = o.value;
    unit.combat.desired_orientation = o.desired;
    if !inp.paused(w)? {
        agent::update_velocity(w, u, inp)?;
    }
    Ok(())
}
