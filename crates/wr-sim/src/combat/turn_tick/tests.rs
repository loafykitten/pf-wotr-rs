use super::*;

const FRAME: Time = Time {
    game_delta: 1.0 / 60.0,
    delta: 1.0 / 60.0,
    player_scale: 1.0,
    debug_scale: 1.0,
    animation_divider: 1.0,
};

fn ai_input() -> ActingInput {
    ActingInput {
        running: false,
        get_up: false,
        panicked: false,
        able: Some(true),
        can_get_up_on_command: false,
        controllable: Some(false),
        extra_action: None,
        surprise_round: false,
        motion: false,
        wants_to_move: false,
        no_more_movement: false,
        time: FRAME,
    }
}

const IN_TURN: ActingState =
    ActingState { in_combat: true, remaining_time: 6.0, meters_five_foot: 0.0 };

#[test]
fn parses_the_records_fields() {
    assert_eq!(Status::parse("Ending"), Some(Status::Ending));
    assert_eq!(Status::parse("Nope"), None);
    assert_eq!(Status::Delayed.name(), "Delayed");
    let c = Counters::parse("0.0166666675|0|3|1").unwrap();
    assert_eq!((c.idle, c.end_wait, c.stuck, c.ai_forced), (1.0 / 60.0, 0.0, 3, 1));
    assert!(Counters::parse("0|0|0").is_none() && Counters::parse("0|0|0|0|0").is_none());
    assert_eq!(Time::parse("0.0166666675|0.0166666675|1|1|1"), Some(FRAME));
    assert_eq!(Time::parse("0.0166666675|0.0166666675|1|1|NaN"), None);
    assert_eq!(Counters::parse("inf|0|0|0"), None);
}

#[test]
fn prepare_sets_preparing_for_a_controllable_rider() {
    let mut t =
        TurnCtl { counters: Counters { stuck: 4, ..Counters::default() }, ..TurnCtl::default() };
    prepare(&mut t, true, true);
    assert_eq!((t.status, t.wait_for_get_up, t.counters.stuck), (Status::Preparing, true, 0));
    prepare(&mut t, false, false);
    assert_eq!((t.status, t.wait_for_get_up), (Status::Acting, false));
}

/// `continue_acting` with every input it read recorded.
fn acting(t: &mut TurnCtl, i: &ActingInput, s: &ActingState) -> bool {
    continue_acting(t, i, s).expect("recorded inputs")
}

#[test]
fn a_preparing_turn_acts_once_its_rider_acted_or_has_a_command() {
    let idle = StatusInput {
        commands_empty: true,
        able: Some(true),
        running: false,
        projectile: Some(false),
    };
    let mut t = TurnCtl { status: Status::Preparing, ..TurnCtl::default() };
    assert_eq!(status_change(&mut t, false, idle), Ok(false));
    assert_eq!(t.status, Status::Preparing);
    assert_eq!(
        status_change(&mut t, false, StatusInput { commands_empty: false, ..idle }),
        Ok(true)
    );
    assert_eq!(t.status, Status::Acting);
    // Acting, but waiting for a running command, a projectile or the rider getting up.
    assert_eq!(status_change(&mut t, false, StatusInput { running: true, ..idle }), Ok(false));
    let flying = StatusInput { projectile: Some(true), ..idle };
    assert_eq!(status_change(&mut t, false, flying), Ok(false));
    t.wait_for_get_up = true;
    assert_eq!(status_change(&mut t, false, idle), Ok(false));
    // What it reads must be recorded; what it doesn't read needn't be.
    let mut t = TurnCtl { status: Status::Preparing, ..TurnCtl::default() };
    assert!(status_change(&mut t, false, StatusInput { able: None, ..idle }).is_err());
    assert_eq!(t.status, Status::Preparing);
    assert_eq!(status_change(&mut t, true, StatusInput { able: None, ..idle }), Ok(true));
    let waiting = StatusInput { running: true, projectile: None, ..idle };
    assert_eq!(status_change(&mut t, false, waiting), Ok(false));
    assert!(status_change(&mut t, false, StatusInput { projectile: None, ..idle }).is_err());
}

#[test]
fn an_idle_ai_ends_after_the_idle_wait() {
    let mut t = TurnCtl { status: Status::Acting, ..TurnCtl::default() };
    let i = ai_input();
    // 0.2 s at 60 fps: the 13th idle frame is past it (12/60 = 0.2 isn't, in single precision).
    let frames = (1..100).find(|_| !acting(&mut t, &i, &IN_TURN)).unwrap();
    assert_eq!(frames, 13);
    // Motion, a running command or a wish to move resets the wait.
    let mut t = TurnCtl::default();
    assert!(acting(&mut t, &i, &IN_TURN));
    assert!(acting(&mut t, &ActingInput { motion: true, ..i }, &IN_TURN));
    assert_eq!(t.counters.idle, 0.0);
}

/// The idle wait is rounded once, as Mono stores it: with the time scale averaging 1.5 and the
/// player's at 3 it's 0.4f (0.400000036 if each step were rounded), so eight idle frames of 0.05 s
/// (0.400000036 in single precision) are past it.
#[test]
fn the_idle_wait_rounds_once() {
    let time =
        Time { game_delta: 0.05, delta: 0.05, player_scale: 3.0, animation_divider: 1.5, ..FRAME };
    let i = ActingInput { time, ..ai_input() };
    let mut t = TurnCtl::default();
    let frames = (1..100).find(|_| !acting(&mut t, &i, &IN_TURN)).unwrap();
    assert_eq!((frames, t.counters.idle), (8, 0.400_000_04));
}

#[test]
fn a_stuck_ai_ends_after_fifteen_frames() {
    let mut t = TurnCtl::default();
    let i = ActingInput { wants_to_move: true, ..ai_input() };
    let frames = (1..100).find(|_| !acting(&mut t, &i, &IN_TURN)).unwrap();
    assert_eq!((frames, t.counters.stuck), (16, 16));
    // An uninterruptable command's flag makes it idle instead.
    let mut t = TurnCtl::default();
    let i = ActingInput { no_more_movement: true, motion: true, ..i };
    assert!(acting(&mut t, &i, &IN_TURN));
    assert_eq!(t.counters.stuck, 0);
}

#[test]
fn a_controllable_rider_acts_while_it_has_time_or_an_extra_action() {
    let mut t = TurnCtl::default();
    let i = ActingInput { controllable: Some(true), ..ai_input() };
    let spent = ActingState { remaining_time: 0.0, ..IN_TURN };
    assert!(acting(&mut t, &i, &IN_TURN));
    // Out of move actions: HasExtraAction decides (it must be recorded then).
    assert!(continue_acting(&mut t, &i, &spent).is_err());
    assert!(!acting(&mut t, &ActingInput { extra_action: Some(false), ..i }, &spent));
    assert!(acting(&mut t, &ActingInput { extra_action: Some(true), ..i }, &spent));
    assert!(acting(&mut t, &ActingInput { running: true, ..i }, &spent));
    // Out of combat, or unable to act and to get up: over.
    assert!(!acting(&mut t, &i, &ActingState { in_combat: false, ..IN_TURN }));
    assert!(!acting(&mut t, &ActingInput { able: Some(false), ..i }, &IN_TURN));
    let i = ActingInput { able: Some(false), can_get_up_on_command: true, ..i };
    assert!(acting(&mut t, &i, &IN_TURN));
    assert!(continue_acting(&mut t, &ActingInput { controllable: None, ..i }, &IN_TURN).is_err());
    // A panicked unit whose five-foot step is used up.
    let stepped = ActingState { meters_five_foot: METERS_OF_FIVE_FOOT_STEP, ..IN_TURN };
    assert!(!acting(&mut t, &ActingInput { panicked: true, ..ai_input() }, &stepped));
}

#[test]
fn an_ending_turn_waits_for_its_commands_then_two_frames() {
    let mut t = TurnCtl { status: Status::Ending, ..TurnCtl::default() };
    let i = WaitingInput { running: true, get_up: false, time: FRAME };
    assert!(continue_waiting(&mut t, &i));
    assert_eq!(t.counters.end_wait, 0.0);
    let i = WaitingInput { running: false, ..i };
    assert!(continue_waiting(&mut t, &i));
    assert!(!continue_waiting(&mut t, &i));
}

#[test]
fn a_forced_ai_tick_that_decides_is_counted() {
    let mut t = TurnCtl { status: Status::Acting, ..TurnCtl::default() };
    t.counters.idle = 0.1;
    after_forced_ai_tick(&mut t, true);
    assert_eq!((t.counters.ai_forced, t.counters.idle), (0, 0.1));
    after_forced_ai_tick(&mut t, false);
    assert_eq!((t.counters.ai_forced, t.counters.idle), (1, 0.0));
    t.counters.ai_forced = 50;
    after_forced_ai_tick(&mut t, false);
    assert_eq!(t.status, Status::Ending);
}
