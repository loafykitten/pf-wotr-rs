//! `UnitProneController` (decompiled, not copied: `Kingmaker.Controllers.Units/
//! UnitProneController.cs`, with `UnitState.CanStandUp`, `ProneState` and the
//! `IUnitFallProneHandler`/`IUnitGetUpHandler` events).
//!
//! The controller runs in its place in every frame ([`crate::combat::replay::Phase::Prone`],
//! after `UnitLifeController` and `UnitReturnToConsciousController`) on every awake unit, dead
//! ones included. A unit goes prone when it should be (`ShouldBeProne`: unconscious, asleep, the
//! condition, or `ProneState.ShouldBeActive` set by a knockdown), which interrupts its commands;
//! it gets up after three seconds once it shouldn't be and can stand. In turn-based combat the
//! posture's `Duration` runs down on its own schedule (the current unit's turn, a passing
//! round); the turn controller's `WaitForGetUp` and every attack against the unit (`+4` ranged,
//! `-4` melee) read the posture.
//!
//! What the sim makes of it, and what stays outside:
//! - `ProneState` (`Active`, `ShouldBeActive`, `Duration` in 100 ns ticks, saved) is computed.
//! - `Commands.InterruptAll()` on falling prone is the commands model's, driven by the
//!   recording's `uc_x` record: [`tick_unit`] says a unit falls ([`Step::fall`]) and the driver
//!   checks it against the records (in both directions).
//! - `UnitEntityView.EnterProneState`/`LeaveProneState` (animation, `IsProne` for the movement
//!   agent, whose `prone` read is a recorded input) and `Unit.Wake(1f)` (`AwakeTimer`; the awake
//!   sets are recorded inputs) are presentation.
//! - Reported where reached, not made: a unit that should stand and is directly controllable
//!   (the controller keeps it down until it has a command: `IsDirectlyControllable` isn't
//!   established), a unit that gets up (`HandleUnitWillGetUp`'s subscribers: the engagement
//!   controller's attack of opportunity, the turn controller's move cooldown), and a unit
//!   falling with an `AddFallProneTrigger` fact on any unit.

use crate::combat::{commands as uc, cooldowns, life, turns};
use crate::csharp::timespan::{TICKS_PER_SECOND, TimeSpan};
use crate::unit_state::{cond, has_condition};
use crate::world::{EntityIdx, World};

/// `UnitCondition.Dazed`, `Sleeping`.
const DAZED: i32 = 9;
const SLEEPING: i32 = 16;

/// What one unit's tick did that its caller places in the frame.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// The unit fell prone: `Commands.InterruptAll()` ran (the commands model's, from the
    /// recording's record).
    pub fall: bool,
    /// The unit got up.
    pub get_up: bool,
}

/// `UnitProneController.ShouldBeProne(unit)`.
#[must_use]
pub fn should_be_prone(w: &World, u: EntityIdx) -> bool {
    let Some(x) = w.unit(u) else { return false };
    if !x.prone_should_be_active
        && x.life_state == life::state::CONSCIOUS
        && !has_condition(w, u, cond::PRONE)
        && !has_condition(w, u, SLEEPING)
    {
        return has_condition(w, u, cond::UNCONSCIOUS);
    }
    true
}

/// `TimeSpan.FromSeconds(float)` (the `Seconds()` extension), as a count of ticks.
fn secs(v: f32) -> Result<i64, String> {
    TimeSpan::from_seconds(f64::from(v)).map(|t| t.0).map_err(|e| format!("Seconds({v}): {e:?}"))
}

/// `UnitState.CanStandUp`: always in a cutscene, else not helpless, dazed or stunned.
fn can_stand_up(w: &mut World, u: EntityIdx, cutscene: bool) -> bool {
    cutscene
        || (!crate::combat::units::is_helpless(w, u)
            && !has_condition(w, u, DAZED)
            && !has_condition(w, u, cond::STUNNED))
}

/// A unit's `Commands.Standard`/`Commands.Move` is a cutscene command.
fn cutscene_command(w: &World, u: EntityIdx) -> bool {
    let c = &w.combat.commands;
    let Some(s) = c.set_of(&w.entity(u).unique_id) else { return false };
    [uc::ty::STANDARD, uc::ty::MOVE]
        .into_iter()
        .any(|t| s.slots[usize::from(t)].and_then(|i| c.get(i)).is_some_and(|x| x.cutscene))
}

/// One unit's `UnitProneController.Tick`, but for `Commands.InterruptAll()` (see [`Step`]).
/// `cutscene`: the game mode is a cutscene.
///
/// # Errors
///
/// What the sim doesn't establish (see the module's list); the unit's state is left as it was
/// before the failing step.
pub fn tick_unit(w: &mut World, u: EntityIdx, cutscene: bool) -> Result<Step, String> {
    let tb = w.combat.turn_based;
    let dt = secs(w.clock.delta_time())?;
    if tb && w.unit(u).is_some_and(|x| x.prone_active) {
        let mut d = w.unit(u).map_or(0, |x| x.prone_duration);
        if turns::is_passing(w) {
            if crate::combat::is_in_combat(w, u) {
                d = secs(3.0)?
                    .checked_sub(secs(cooldowns::time_to_next_turn(w, u))?)
                    .and_then(|v| v.checked_sub(1))
                    .ok_or("Duration overflows")?;
                if d < 0 {
                    d = 0;
                }
                d -= dt;
            }
        } else {
            let current = life::is_current_unit(w, u)
                .ok_or("IsCurrentUnit: the current unit isn't established")?;
            if current {
                d = if w.combat.turns.ctl.status == crate::combat::turn_tick::Status::Acting
                    && cooldowns::has_move_action(w, u)
                {
                    secs(3.0)?
                } else {
                    0
                };
            }
            d -= dt;
        }
        if let Some(x) = w.unit_mut(u) {
            x.prone_duration = d;
        }
    }
    let active = w.unit(u).is_some_and(|x| x.prone_active);
    let mut want = should_be_prone(w, u);
    if active && !want && tb {
        // `unit.IsDirectlyControllable && unit.Commands.Empty` keeps it down.
        return Err(
            "a prone unit that should stand, in turn-based combat: IsDirectlyControllable isn't established"
                .into(),
        );
    }
    if cutscene_command(w, u) {
        want = false;
    }
    let mut step = Step::default();
    let mut err = None;
    if active {
        let d = {
            let x = w.unit_mut(u).ok_or("not a unit")?;
            x.prone_duration =
                x.prone_duration.checked_add(dt).ok_or_else(|| "Duration overflows".to_owned())?;
            x.prone_duration
        };
        if !want
            && d >= TICKS_PER_SECOND
            && can_stand_up(w, u, cutscene)
            && d >= 3 * TICKS_PER_SECOND
        {
            // `HandleUnitWillGetUp`: the engagement controller provokes an attack of
            // opportunity (unless the unit gets up without one) and the turn controller adds
            // three seconds to the rider's move cooldown and cancels its commands.
            err = Some(
                "a prone unit gets up: HandleUnitWillGetUp's subscribers aren't ported".to_owned(),
            );
            if let Some(x) = w.unit_mut(u) {
                x.prone_active = false;
                x.prone_duration = 0;
            }
            step.get_up = true;
        }
    } else if want && !has_condition(w, u, cond::PETRIFIED) {
        if let Some(x) = w.unit_mut(u) {
            x.prone_active = true;
        }
        step.fall = true;
        // `HandleUnitFallProne`: `AddFallProneTrigger`'s action runs for its owner.
        let trigger = crate::combat::life::all_units(w).into_iter().any(|o| {
            w.entity(o).facts.list.iter().any(|f| {
                w.fact(*f).bp.as_ref().is_some_and(|b| {
                    b.components_array()
                        .iter()
                        .any(|c| crate::content::is_a(c.ty, "AddFallProneTrigger"))
                })
            })
        });
        if trigger {
            err = Some("a unit falls prone with an AddFallProneTrigger fact on a unit".to_owned());
        }
    }
    if let Some(x) = w.unit_mut(u) {
        x.prone_should_be_active = false;
    }
    err.map_or(Ok(step), Err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::tests::world_with_unit;

    fn world() -> (World, EntityIdx) {
        let (mut w, u) = world_with_unit();
        w.clock.set_delta_time(1.0 / 60.0);
        (w, u)
    }

    /// An unconscious unit falls prone once; the request flag is consumed either way.
    #[test]
    fn an_unconscious_unit_falls_prone_and_the_request_is_consumed() {
        let (mut w, u) = world();
        assert_eq!(tick_unit(&mut w, u, false), Ok(Step::default()));
        assert!(!w.unit(u).expect("unit").prone_active);
        w.unit_mut(u).expect("unit").life_state = life::state::UNCONSCIOUS;
        assert_eq!(tick_unit(&mut w, u, false), Ok(Step { fall: true, get_up: false }));
        assert!(w.unit(u).expect("unit").prone_active);
        // Already prone and still unconscious: it stays, the duration runs (one frame, 17 ms).
        assert_eq!(tick_unit(&mut w, u, false), Ok(Step::default()));
        assert_eq!(w.unit(u).expect("unit").prone_duration, 17 * 10_000);
        w.unit_mut(u).expect("unit").prone_should_be_active = true;
        w.unit_mut(u).expect("unit").life_state = life::state::CONSCIOUS;
        assert!(should_be_prone(&w, u));
        let _ = tick_unit(&mut w, u, false);
        assert!(!w.unit(u).expect("unit").prone_should_be_active);
    }

    /// A conscious, awake unit without the condition isn't prone; each cause alone makes it so.
    #[test]
    fn each_cause_alone_makes_a_unit_prone() {
        let (mut w, u) = world();
        assert!(!should_be_prone(&w, u));
        w.unit_mut(u).expect("unit").prone_should_be_active = true;
        assert!(should_be_prone(&w, u));
        w.unit_mut(u).expect("unit").prone_should_be_active = false;
        w.unit_mut(u).expect("unit").life_state = life::state::DEAD;
        assert!(should_be_prone(&w, u));
    }

    /// A unit that should stand gets up only after three seconds, and the get-up is reported
    /// (its handlers aren't ported); in a cutscene it stands regardless of being helpless.
    #[test]
    fn a_standing_unit_waits_three_seconds_and_the_get_up_is_reported() {
        let (mut w, u) = world();
        {
            let x = w.unit_mut(u).expect("unit");
            x.prone_active = true;
            x.prone_duration = 3 * TICKS_PER_SECOND - 17 * 10_000;
        }
        let r = tick_unit(&mut w, u, false);
        assert!(r.as_ref().is_err_and(|e| e.contains("gets up")), "{r:?}");
        let x = w.unit(u).expect("unit");
        assert!(!x.prone_active);
        assert_eq!(x.prone_duration, 0);
    }
}
