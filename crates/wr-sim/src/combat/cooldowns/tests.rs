//! Units' cooldowns and the turn's movement budget (`super`).

use super::*;
use crate::combat::turns::tests::{in_combat, reported, unit, world};

/// Two units in turn-based combat, awake; time passes between turns towards `b`'s turn.
fn passing() -> (World, EntityIdx, EntityIdx) {
    let mut w = world();
    let a = unit(&mut w, "a", None, 10);
    let b = unit(&mut w, "b", Some("g"), 5);
    in_combat(&mut w, &[a, b]);
    w.combat.awake = vec![a, b];
    w.combat.current_unit = None;
    w.combat.turns.next = Some(b);
    w.combat.turns.passing = true;
    w.clock.set_game_delta_time(0.5);
    (w, a, b)
}

fn set(w: &mut World, u: EntityIdx, c: Cooldowns) {
    w.unit_mut(u).unwrap().combat.cooldown = c;
}

fn cd(i: f32, s: f32, m: f32, sw: f32, aoo: f32) -> Cooldowns {
    Cooldowns {
        initiative: i,
        standard_action: s,
        move_action: m,
        swift_action: sw,
        attack_of_opportunity: aoo,
    }
}

#[test]
fn spending_an_action_in_turn_based_combat_adds_its_time() {
    let (mut w, a, _) = passing();
    assert!(spend_action(&mut w, a, command_type::MOVE, false, 0.2));
    assert!(spend_action(&mut w, a, command_type::STANDARD, true, 0.2));
    assert!(spend_action(&mut w, a, command_type::SWIFT, false, 0.2));
    assert!(spend_action(&mut w, a, command_type::FREE, false, 0.2));
    assert_eq!(cooldown(&w, a), cd(0.0, 6.0, 6.0, 6.0, 0.0));
    // Outside turn-based combat each sets the action's time less the time since the command
    // started.
    w.combat.tb_mode_on = false;
    crate::combat::life::update_turn_based(&mut w);
    assert!(spend_action(&mut w, a, command_type::STANDARD, false, 0.5));
    assert_eq!(cooldown(&w, a), cd(0.0, 5.5, 3.0, 6.0, 0.0));
    // An unknown type is the game's exception; a unit out of combat spends nothing.
    assert!(!spend_action(&mut w, a, 7, false, 0.0));
    w.unit_mut(a).unwrap().combat.in_combat = false;
    assert!(spend_action(&mut w, a, command_type::MOVE, false, 0.0));
    assert_eq!(cooldown(&w, a).move_action, 3.0);
}

#[test]
fn time_passing_between_turns_runs_the_initiative_down_first() {
    let (mut w, a, b) = passing();
    set(&mut w, a, cd(0.25, 1.0, 2.0, 0.1, 0.0));
    set(&mut w, b, cd(1.0, 6.0, 0.0, 0.0, 0.0));
    controller_tick(&mut w);
    // `a`: 0.25 s of initiative, then the other 0.25 s from each action (floored at zero).
    assert_eq!(cooldown(&w, a), cd(0.0, 0.75, 1.75, 0.0, 0.0));
    // `b`: the whole delta goes to its initiative.
    assert_eq!(cooldown(&w, b), cd(0.5, 6.0, 0.0, 0.0, 0.0));
    assert_eq!(time_to_next_turn(&w, b), 6.5);
    // During a turn nothing runs down.
    w.combat.current_unit = Some(a);
    controller_tick(&mut w);
    assert_eq!(cooldown(&w, b), cd(0.5, 6.0, 0.0, 0.0, 0.0));
}

#[test]
fn a_surprised_units_initiative_waits_and_the_dead_dont_tick() {
    let (mut w, a, b) = passing();
    w.combat.turns.surprise_round = true;
    w.combat.turns.round = 0;
    w.combat.turns.units[0].surprised = true;
    set(&mut w, a, cd(1.0, 1.0, 0.0, 0.0, 0.0));
    w.unit_mut(b).unwrap().life_state = 2;
    set(&mut w, b, cd(1.0, 0.0, 0.0, 0.0, 0.0));
    controller_tick(&mut w);
    assert_eq!(cooldown(&w, a), cd(1.0, 0.5, 0.0, 0.0, 0.0));
    assert_eq!(cooldown(&w, b), cd(1.0, 0.0, 0.0, 0.0, 0.0));
    // A unit in combat whose awake state isn't known is reported once it has a cooldown.
    w.combat.awake_unknown = vec![a];
    controller_tick(&mut w);
    assert!(reported(&w, "UnitCombatCooldownsController: a unit in combat whose awake"));
}

fn walk(dt: f32) -> MoveInput {
    MoveInput {
        dt,
        force: false,
        near_the_end: false,
        limit: limit::TWO_ACTIONS,
        modify_current: false,
        modify_request: false,
        moving: true,
        ai_used_five_foot_step: false,
        auto_stop: false,
        speed: Some(3.048),
        panicked: false,
    }
}

#[test]
fn moving_spends_the_move_actions_up_to_the_turns_budget() {
    let (mut w, a, _) = passing();
    w.combat.current_unit = Some(a);
    assert_eq!(tick_movement(&mut w, a, &walk(0.25)), Some(0.25));
    assert_eq!(cooldown(&w, a).move_action, 0.25);
    assert_eq!(w.combat.turns.movement.time_moved, 0.25);
    // Two move actions are 6 s: the last step is clamped to what is left.
    set(&mut w, a, cd(0.0, 0.0, 5.9, 0.0, 0.0));
    let left = tick_movement(&mut w, a, &walk(0.25)).unwrap();
    assert_eq!(left, 6.0 - 5.9_f32);
    assert_eq!(cooldown(&w, a).move_action, 5.9 + left);
    // Nothing left: no movement, nothing counted.
    let before = w.combat.turns.movement;
    assert_eq!(tick_movement(&mut w, a, &walk(0.25)), Some(0.0));
    assert_eq!(w.combat.turns.movement, before);
    // Near its path's end the agent moves on borrowed time: not clamped.
    let far = MoveInput { near_the_end: true, ..walk(0.25) };
    assert_eq!(tick_movement(&mut w, a, &far), Some(0.25));
    // After a standard action one move action is left; a single-action move only that.
    set(&mut w, a, cd(0.0, 6.0, 2.9, 0.0, 0.0));
    let one = MoveInput { limit: limit::ONE_ACTION, ..walk(0.25) };
    assert_eq!(tick_movement(&mut w, a, &one), Some(0.0));
    assert_eq!(tick_movement(&mut w, a, &walk(0.25)).unwrap(), 3.0 - 2.9_f32);
}

#[test]
fn a_five_foot_step_counts_meters_and_needs_the_speed() {
    let (mut w, a, _) = passing();
    w.combat.current_unit = Some(a);
    let step = MoveInput { limit: limit::FIVE_FOOT_STEP, ..walk(0.25) };
    assert_eq!(tick_movement(&mut w, a, &step), Some(0.25));
    let m = w.combat.turns.movement;
    assert_eq!(m.time_moved_by_five_foot_step, 0.25);
    #[allow(clippy::cast_possible_truncation)]
    let want = (0.25_f64 * f64::from(3.048_f32)) as f32;
    assert_eq!(m.meters_moved_by_five_foot_step, want);
    assert_eq!(cooldown(&w, a).move_action, 0.0);
    // The step's range is METERS_OF_FIVE_FOOT_STEP: the rest of it, at the unit's speed.
    let rest = tick_movement(&mut w, a, &MoveInput { dt: 5.0, ..step }).unwrap();
    assert_eq!(rest, (METERS_OF_FIVE_FOOT_STEP - want) / 3.048);
    // Without the speed nothing is computed or changed.
    let before = w.combat.turns.movement;
    assert_eq!(tick_movement(&mut w, a, &MoveInput { speed: None, ..step }), None);
    assert_eq!(w.combat.turns.movement, before);
    // Force mode counts time only.
    let forced = MoveInput { force: true, ..walk(0.5) };
    assert_eq!(tick_movement(&mut w, a, &forced), Some(0.5));
    assert_eq!(w.combat.turns.movement.time_moved_in_force_mode, 0.5);
}

#[test]
fn ending_a_turn_spends_its_actions() {
    let (mut w, a, _) = passing();
    set(&mut w, a, cd(0.0, 0.0, 7.0, 0.0, 0.0));
    end(&mut w, a);
    assert_eq!(cooldown(&w, a), cd(0.0, 6.0, 6.0, 0.0, 0.0));
    set(&mut w, a, cd(0.0, 0.0, 1.0, 0.0, 0.0));
    force_to_end(&mut w, a, false);
    assert_eq!(cooldown(&w, a), cd(0.0, 0.0, 1.0, 0.0, 0.0));
    force_to_end(&mut w, a, true);
    assert_eq!(cooldown(&w, a), cd(0.0, 6.0, 3.0, 6.0, 0.0));
    assert_eq!(w.combat.turns.movement.meters_moved_by_five_foot_step, METERS_OF_FIVE_FOOT_STEP);
}

#[test]
fn a_command_that_empties_movement_spends_the_move_action_begun() {
    let (mut w, a, _) = passing();
    set(&mut w, a, cd(0.0, 0.0, 1.2, 0.0, 0.0));
    assert!(command_did_end(&mut w, a, false, "Preparing", None));
    assert!(command_did_end(&mut w, a, true, "Acting", None));
    assert!(command_did_end(&mut w, a, false, "Acting", Some(false)));
    assert_eq!(cooldown(&w, a).move_action, 1.2);
    // Past the early returns the handler evaluates the command: unknown, nothing changes.
    assert!(!command_did_end(&mut w, a, false, "Acting", None));
    assert_eq!(cooldown(&w, a).move_action, 1.2);
    assert!(command_did_end(&mut w, a, false, "Acting", Some(true)));
    assert_eq!(cooldown(&w, a).move_action, 3.0);
    // A move action not begun, or one spent, is left alone.
    set(&mut w, a, cd(0.0, 0.0, 3.5, 0.0, 0.0));
    assert!(command_did_end(&mut w, a, false, "Ended", Some(true)));
    assert_eq!(cooldown(&w, a).move_action, 3.5);
}

#[test]
fn a_staggered_unit_has_one_move_action() {
    let (mut w, a, _) = passing();
    w.combat.current_unit = Some(a);
    w.unit_mut(a).unwrap().state.conditions[cond::STAGGERED as usize] = 1;
    set(&mut w, a, cd(0.0, 0.0, 2.9, 0.0, 0.0));
    assert_eq!(tick_movement(&mut w, a, &walk(0.25)).unwrap(), 3.0 - 2.9_f32);
}

#[test]
fn a_unit_that_cant_use_standard_actions_has_used_its_standard_action() {
    let (mut w, a, _) = passing();
    w.combat.current_unit = Some(a);
    // Frightened (13) restricts nothing: the second move action goes on.
    w.unit_mut(a).unwrap().state.conditions[13] = 1;
    set(&mut w, a, cd(0.0, 0.0, 3.5, 0.0, 0.0));
    assert_eq!(tick_movement(&mut w, a, &walk(0.25)), Some(0.25));
    // `CantUseStandardActions` (55): the standard action counts as used, one move action is left,
    // and it is spent.
    w.unit_mut(a).unwrap().state.conditions[cond::CANT_USE_STANDARD_ACTIONS as usize] = 1;
    set(&mut w, a, cd(0.0, 0.0, 3.5, 0.0, 0.0));
    assert_eq!(tick_movement(&mut w, a, &walk(0.25)), Some(0.0));
}

#[test]
fn the_five_foot_steps_speed_check_is_in_double_and_read_only_when_needed() {
    let (mut w, a, _) = passing();
    w.combat.current_unit = Some(a);
    // 0.762 * 3 rounds to 2.286 in f32 (at most the step: restricted), but is above it in
    // double: the step is allowed.
    let slow = MoveInput { limit: limit::FIVE_FOOT_STEP, speed: Some(0.762), ..walk(0.25) };
    const { assert!(0.762_f32 * 3.0 <= METERS_OF_FIVE_FOOT_STEP) };
    assert_eq!(tick_movement(&mut w, a, &slow), Some(0.25));
    // After normal movement the step is restricted without reading the speed.
    w.combat.turns.movement = MovementStats { time_moved: 1.0, ..MovementStats::default() };
    let unknown = MoveInput { speed: None, ..slow };
    assert_eq!(tick_movement(&mut w, a, &unknown), Some(0.0));
}

#[test]
fn the_requested_movement_modifier_applies_once_the_units_stop() {
    let (mut w, a, _) = passing();
    w.combat.current_unit = Some(a);
    let req = MoveInput { modify_request: true, ..walk(0.25) };
    // Moving: the current value (off), normal movement.
    assert_eq!(tick_movement(&mut w, a, &req), Some(0.25));
    assert_eq!(cooldown(&w, a).move_action, 0.25);
    // Not moving: the request (a five-foot step, in a turn without normal movement yet).
    w.combat.turns.movement = MovementStats::default();
    let still = MoveInput { moving: false, ..req };
    assert_eq!(tick_movement(&mut w, a, &still), Some(0.25));
    assert_eq!(cooldown(&w, a).move_action, 0.25);
    assert_eq!(w.combat.turns.movement.time_moved_by_five_foot_step, 0.25);
}
