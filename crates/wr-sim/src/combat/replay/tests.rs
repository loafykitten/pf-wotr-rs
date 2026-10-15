use super::*;
use crate::combat::attack::RuleAttackWithWeaponResolve;
use crate::csharp::TimeSpan;
use crate::rulebook::RuleBase;
use crate::stats::tests::world_with_unit;

/// A world with one unit, which is in the loaded area's state (`State.Units`).
fn world_in_area() -> (World, EntityIdx) {
    let (mut w, u) = world_with_unit();
    w.player.area_units.push(u);
    (w, u)
}

fn rec(k: &str, extra: J) -> J {
    let mut v = json!({"k": k});
    if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
        o.extend(e.clone());
    }
    v
}

/// A trace of `records`, as WrathOracle writes one: a record's (`begin.mode`), numbered as
/// `Trace.Emit` numbers them (`seq`) where they aren't.
fn trace(mut records: Vec<J>) -> Trace {
    if let Some(o) = records.iter_mut().find(|r| r["k"] == "begin").and_then(J::as_object_mut) {
        o.entry("mode").or_insert(J::from("record"));
    }
    for (n, r) in records.iter_mut().enumerate() {
        if let Some(o) = r.as_object_mut() {
            o.entry("seq").or_insert(J::from(n));
        }
    }
    Trace { records, sites: HashMap::default() }
}

fn tick(t: i64, mode: i64) -> J {
    json!({"k": "tick", "t": t, "mode": mode})
}

/// A `time` record; `tb`: the turn-based state it saw.
fn time(t: i64, game: i64, tb: bool) -> J {
    json!({"k": "time", "t": t, "game_time": game, "real_time": game, "turn_based": tb})
}

fn tb(t: i64, ev: &str, unit: Option<&str>, game: i64) -> J {
    let mut r = json!({"k": "tb", "t": t, "ev": ev, "game_time": game});
    if let Some(u) = unit {
        r["unit"] = json!({"$unit": u});
    }
    r
}

fn input(t: i64, step: &str) -> J {
    json!({"k": "input", "t": t, "step": step})
}

fn has_report(w: &World, prefix: &str) -> bool {
    w.coverage.unsupported.keys().any(|k| k.starts_with(prefix))
}

fn timed_buff(w: &mut World, u: EntityIdx, g: u128, end: i64) -> crate::world::FactIdx {
    use crate::facts::tests::obj;
    use wr_content::node::Node;
    let o = obj(
        "Kingmaker.UnitLogic.Buffs.Blueprints.BlueprintBuff",
        &[("Components", Node::Seq(Vec::new()))],
    );
    w.content.insert(crate::content::Blueprint {
        guid: wr_content::Guid(g),
        name: format!("B{g}"),
        obj: o,
        source: crate::content::Source::Bbp,
    });
    let bp = w.content.get(wr_content::Guid(g)).expect("inserted");
    let ctx = crate::unit_facts::context(w, Some(u), Some(u), &bp);
    let start = super::super::buff_start_time(w).0;
    crate::unit_facts::add_buff(w, u, &bp, ctx, Some(TimeSpan(end - start))).expect("buff")
}

fn alive(w: &World, u: EntityIdx, f: crate::world::FactIdx) -> bool {
    w.entity(u).facts.list.contains(&f)
}

const S: i64 = 10_000_000;

/// The controllers run in their registration order: groups, the turn controller, movement,
/// sleeping units, cooldowns, commands, buffs, projectiles, effects, life, leaving combat, helpless, inspection, then what follows
/// `Game.Tick`. Each runs once per frame, also in a frame without any event; an event of an
/// earlier phase than the frame has reached can't be placed.
#[test]
fn phases_follow_the_registration_order() {
    use Phase::*;
    let mut f = Frame::default();
    assert_eq!(f.start(mode::DEFAULT), vec![]);
    assert_eq!(f.to(Commands), Ok(vec![Groups, Turn, TurnChoose, Move, Sleeping, Cooldowns]));
    assert_eq!(f.to(Projectiles), Ok(vec![Buffs]));
    assert_eq!(f.to(Projectiles), Ok(vec![]), "nothing between two resolves");
    assert_eq!(f.to(Fx), Ok(vec![]));
    assert_eq!(f.to(Commands), Err(Misplaced::Late));
    assert_eq!(f.to(LeavePrefix), Ok(vec![Life]), "the leave controller's inputs before its work");
    assert_eq!(f.to(Post), Ok(vec![Leave, Conscious, Helpless, Inspect, GameLog, Post]));
    assert_eq!(f.to(Post), Ok(vec![]), "once");
    assert_eq!(f.start(mode::DEFAULT), vec![]);
    // A frame without events: everything at its end.
    let all = vec![
        Groups, Turn, TurnChoose, Move, Sleeping, Cooldowns, Buffs, Life, Leave, Conscious,
        Helpless, Inspect, GameLog, Post,
    ];
    assert_eq!(f.start(mode::DEFAULT), all);
    assert_eq!(f.finish(), all);
    // Before the first frame only an input can happen.
    assert_eq!(f.to(Post), Ok(vec![]));
    assert_eq!(f.to(Commands), Err(Misplaced::NoModel));
}

/// The game mode decides which controllers run (`GameModesFactory.Initialize`): a paused frame
/// runs no groups, time, turns, movement, buffs or life (only sleeping units and helplessness), a
/// cutscene no buffs or leaving combat, combat preparation, cooldowns and leaving only in the
/// default mode; other
/// modes are not modelled.
#[test]
fn phases_follow_the_game_modes_registrations() {
    use Phase::*;
    let mut f = Frame::default();
    f.start(mode::PAUSE);
    assert_eq!(f.to(Time), Err(Misplaced::NotRegistered));
    assert_eq!(f.finish(), vec![Sleeping, Helpless, GameLog, Post]);
    f.start(mode::CUTSCENE);
    assert_eq!(f.to(Prepare), Err(Misplaced::NotRegistered));
    assert_eq!(f.to(LeavePrefix), Err(Misplaced::NotRegistered));
    assert_eq!(
        f.finish(),
        vec![
            Groups, Turn, TurnChoose, Move, Sleeping, Life, Conscious, Helpless, Inspect, GameLog,
            Post
        ]
    );
    f.start(mode::DIALOG);
    assert_eq!(
        f.finish(),
        vec![Groups, Move, Sleeping, Buffs, Life, Conscious, Helpless, Inspect, GameLog, Post]
    );
    f.start(2);
    assert_eq!(f.to(Time), Err(Misplaced::NoModel));
    assert_eq!(f.finish(), vec![]);
}

/// Replay-level: a buff past its end stays through cutscene and paused frames (no buffs
/// controller there) and goes in the first default-mode frame.
#[test]
fn buffs_tick_only_in_modes_that_register_them() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let f = timed_buff(&mut w, u, 950, 2 * S);
    let recs = vec![tick(0, mode::CUTSCENE), time(0, 10 * S, false), tick(1, mode::PAUSE)];
    let _ = run(&mut w, &trace(recs));
    assert!(alive(&w, u, f), "no UnitBuffsController in a cutscene or a pause");
    let _ = run(&mut w, &trace(vec![tick(2, mode::DEFAULT), time(2, 11 * S, false)]));
    assert!(!alive(&w, u, f), "gone in the default mode");
}

/// Replay-level: a dying unit isn't killed in a paused frame (no `UnitLifeController`), but its
/// helplessness is updated there.
#[test]
fn a_paused_frame_runs_no_life_controller() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    w.unit_mut(u).expect("unit").damage = 1_000;
    let _ = run(&mut w, &trace(vec![tick(0, mode::PAUSE), tick(1, mode::PAUSE)]));
    assert!(w.combat.awake.contains(&u), "awake in the paused frames");
    assert_eq!(w.unit(u).expect("unit").life_state, 0, "paused: still conscious");
    let _ = run(&mut w, &trace(vec![tick(2, mode::DEFAULT), time(2, 0, false)]));
    assert_eq!(w.unit(u).expect("unit").life_state, super::super::life::state::DEAD);
}

/// An input is made after `Game.Tick`: the frame's controllers have run by then, with the
/// turn-based semantics they had. Here the buff isn't the current unit's in turn-based combat,
/// so the frame's buffs tick leaves it; switching turn-based mode off afterwards doesn't
/// reach back into that frame.
#[test]
fn an_input_comes_after_its_frames_controllers() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let f = timed_buff(&mut w, u, 951, 6 * S);
    let recs = vec![
        input(-1, "tbm:on"),
        tick(0, mode::DEFAULT),
        time(0, 0, false),
        tb(0, "combat_start", None, 0),
        tick(1, mode::DEFAULT),
        time(1, 100 * S, true),
        input(1, "tbm:off"),
    ];
    let _ = run(&mut w, &trace(recs));
    assert!(alive(&w, u, f), "the frame's buffs ran in turn-based combat, before the input");
    assert!(!has_report(&w, "driven replay"), "{:?}", w.coverage.unsupported);
}

/// `StartTurn`'s sweep runs inside `TimeController.Tick`, before the frame's later controllers:
/// in one replay with the recorded order (tick, turn_start, time, then the frame's initiative),
/// the current unit's buff that is due at the previous `TurnStartTime` is removed before the
/// initiative rule. Only the sweep does that: the frame's own buffs tick comes after the
/// initiative (and the buff was not due for anyone before, as it wasn't anyone's turn).
#[test]
fn the_turn_start_sweep_runs_before_the_frames_controllers() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let f = timed_buff(&mut w, u, 952, S);
    let root = |t: i64| {
        json!({"k": "rule", "t": t, "id": 1, "depth": 0, "class": "Kingmaker.RuleSystem.Rules.RuleInitiativeRoll",
            "initiator": {"$unit": "u"}, "fields": {}})
    };
    let recs = vec![
        input(-1, "tbm:on"),
        tick(0, mode::DEFAULT),
        time(0, 2 * S, false),
        tb(0, "combat_start", None, 2 * S),
        tick(1, mode::DEFAULT),
        time(1, 3 * S, true),
        tick(2, mode::DEFAULT),
        tb(2, "turn_start", Some("u"), 4 * S),
        time(2, 4 * S, true),
        root(2),
        json!({"k": "rule_end", "t": 2, "id": 1}),
    ];
    let t = trace(recs);
    let mut d = Vec::new();
    let _ = run(&mut w, &t);
    assert!(!alive(&w, u, f));
    for (i, e) in w.events.records.iter().enumerate() {
        if e.k == "rule" || e.fields.get("what").and_then(J::as_str) == Some("fact_detach") {
            d.push((i, e.k));
        }
    }
    assert_eq!(d.first().map(|x| x.1), Some("mut"), "the sweep's removal comes first: {d:?}");
    assert_eq!(w.combat.turn_start_time, TimeSpan(4 * S), "the turn start's recorded time");
    assert!(!has_report(&w, "CombatController.StartTurn"), "{:?}", w.coverage.unsupported);
}

/// What lies between a turn's frame start and its `turn_start` record can't be ordered against
/// the sweep: a driven root, a mutation or a turn event there is reported; echoed inert roots,
/// observations and commands are not.
#[test]
fn records_before_a_turn_start_in_its_frame_are_checked() {
    let (mut w, _) = world_in_area();
    let base = vec![tick(0, mode::DEFAULT), rec("obs", json!({"t": 0, "ev": "u", "u": "~fx"}))];
    let mut with_mut = base.clone();
    with_mut.push(json!({"k": "mut", "t": 0, "what": "fact_detach", "owner": "u"}));
    with_mut.push(tb(0, "turn_start", Some("u"), 0));
    let _ = run(&mut w, &trace(with_mut));
    assert!(has_report(&w, "CombatController.StartTurn"));
    let (mut w, _) = world_in_area();
    let mut clean = base;
    clean.push(rec(
        "cmd",
        json!({"t": 0, "ev": "run", "class": "UnitAttack", "unit": {"$unit": "u"}}),
    ));
    clean.push(tb(0, "turn_start", Some("u"), 0));
    let _ = run(&mut w, &trace(clean));
    assert!(!has_report(&w, "CombatController.StartTurn"), "{:?}", w.coverage.unsupported);
}

/// Frames without any recorded rule still run the life and helpless controllers: a unit
/// that falls asleep becomes helpless, and wakes again, with no rule in either frame.
#[test]
fn the_controllers_run_in_frames_without_rules() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let helpless = |w: &World| crate::unit_state::has_condition(w, u, 24);
    crate::unit_state::add_condition(&mut w, u, 16, None);
    let recs = vec![tick(0, 1), time(0, 10, false), tick(1, 1), time(1, 20, false)];
    let _ = run(&mut w, &trace(recs));
    assert!(helpless(&w), "asleep: helpless");
    crate::unit_state::remove_condition(&mut w, u, 16, true);
    let recs = vec![tick(2, 1), time(2, 30, false), tick(3, 1), time(3, 40, false)];
    let _ = run(&mut w, &trace(recs));
    assert!(!helpless(&w), "awake again: not helpless");
}

/// Buffs expire by game time before combat and after it, and by the turn's start time (and
/// only on the current unit's turn) during it.
#[test]
fn buff_expiry_follows_turn_based_combat_only_while_it_lasts() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let early = timed_buff(&mut w, u, 940, 2 * S);
    let late = timed_buff(&mut w, u, 941, 6 * S);
    let recs = vec![
        input(-1, "tbm:on"),
        // Turn-based mode is on, combat is not: game time counts.
        tick(0, 1),
        time(0, 3 * S, false),
        tick(1, 1),
        time(1, 3 * S + 1, false),
        tb(1, "combat_start", None, 3 * S + 1),
        // In combat, no turn yet: the buff isn't the current unit's: nothing ticks.
        tick(2, 1),
        time(2, 10 * S, true),
        tick(3, 1),
        time(3, 10 * S + 1, true),
    ];
    let _ = run(&mut w, &trace(recs));
    assert!(!alive(&w, u, early), "expired before combat, by game time");
    assert!(alive(&w, u, late), "in turn-based combat nobody's turn: no tick");
}

/// After combat ends, buffs go by game time again.
#[test]
fn buff_expiry_returns_to_game_time_after_combat() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let f = timed_buff(&mut w, u, 942, 6 * S);
    let recs = vec![
        input(-1, "tbm:on"),
        tick(0, 1),
        time(0, 0, false),
        tb(0, "combat_start", None, 0),
        tick(1, 1),
        time(1, 10 * S, true),
        tick(2, 1),
        time(2, 10 * S + 1, true),
        tb(2, "combat_end", None, 10 * S + 1),
        tick(3, 1),
        time(3, 10 * S + 2, false),
        tick(4, 1),
        time(4, 10 * S + 3, false),
    ];
    let _ = run(&mut w, &trace(recs));
    assert!(!alive(&w, u, f), "gone by game time once combat ended");
    assert!(!has_report(&w, "driven replay"), "{:?}", w.coverage.unsupported);
}

/// A turn that has ended stays the current one until the turn controller clears it, which the
/// recording doesn't show: a due buff of its unit is reported, not ticked or skipped on a
/// guess; the next prefix's recorded turn status (`none`) settles it.
#[test]
fn an_ended_turn_is_settled_by_the_next_turn_status() {
    let gate =
        |t: i64, d: &str| rec("obs", json!({"t": t, "ev": "turn_status", "g": true, "d": d}));
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let f2 = timed_buff(&mut w, u, 954, S);
    let recs = vec![
        input(-1, "tbm:on"),
        tick(0, 1),
        time(0, 2 * S, false),
        tb(0, "combat_start", None, 2 * S),
        tick(1, 1),
        gate(1, "none->u Acting"),
        time(1, 3 * S, true),
        tb(1, "turn_end", Some("u"), 3 * S),
        tick(2, 1),
        time(2, 3 * S, true),
        tick(3, 1),
        gate(3, "u Ending->none"),
        time(3, 3 * S, true),
    ];
    let _ = run(&mut w, &trace(recs));
    assert!(alive(&w, u, f2), "not ticked on a guess, and settled as not current");
    assert_eq!(
        w.coverage
            .unsupported
            .get(
                "BuffCollection.Tick: whether an ended turn's unit is still current is not recorded"
            )
            .copied(),
        Some(2),
        "frames 1 and 2, not 3"
    );
}

/// Who joins combat is recorded (`join_combat`, `UnitCombatJoinController`): the unit joins
/// there. An initiative for a unit whose join isn't recorded is reported; a join after a later
/// phase of its frame (here after an input) is reported.
#[test]
fn combat_joins_follow_the_recording() {
    let (mut w, u) = world_in_area();
    let join = |t: i64| rec("obs", json!({"t": t, "ev": "join_combat", "u": "u"}));
    let recs = vec![input(-1, "tbm:on"), tick(0, 1), time(0, 0, false), join(0)];
    let _ = run(&mut w, &trace(recs));
    assert!(w.unit(u).expect("unit").combat.in_combat);
    assert!(!has_report(&w, "driven replay"), "{:?}", w.coverage.unsupported);
    let (mut w, _) = world_in_area();
    let recs = vec![input(-1, "tbm:on"), tick(0, 1), time(0, 0, false), input(0, "x"), join(0)];
    let _ = run(&mut w, &trace(recs));
    assert!(has_report(&w, "driven replay: a combat join can't be placed"));
    let (mut w, _) = world_in_area();
    let recs = vec![
        tick(0, 1),
        time(0, 0, false),
        json!({"k": "rule", "t": 0, "id": 1, "depth": 0, "class": "Kingmaker.RuleSystem.Rules.RuleInitiativeRoll",
            "initiator": {"$unit": "u"}, "fields": {}}),
        json!({"k": "rule_end", "t": 0, "id": 1}),
    ];
    let _ = run(&mut w, &trace(recs));
    assert!(has_report(&w, "driven replay: initiative of a unit whose combat join"));
}

/// A root attack is placed in `UnitCommandController` only if the initiator runs an attack
/// command (`cmd start` without its `end`); otherwise its controller is not established.
#[test]
fn a_root_attack_needs_a_running_attack_command() {
    let attack = |t: i64| {
        vec![
            json!({"k": "rule", "t": t, "id": 1, "depth": 0, "class": "Kingmaker.RuleSystem.Rules.RuleAttackWithWeapon",
                "initiator": {"$unit": "u"}, "target": {"$unit": "u"}, "fields": {}}),
            json!({"k": "rule_end", "t": t, "id": 1}),
        ]
    };
    let cmd = |ev: &str| {
        rec("cmd", json!({"t": 0, "ev": ev, "class": "UnitAttack", "unit": {"$unit": "u"}}))
    };
    let report = "driven replay: a root attack without a running attack command";
    let (mut w, _) = world_in_area();
    let _ = run(&mut w, &trace([vec![tick(0, 1), time(0, 0, false)], attack(0)].concat()));
    assert!(has_report(&w, report));
    let (mut w, _) = world_in_area();
    let recs = [vec![tick(0, 1), time(0, 0, false), cmd("start")], attack(0)].concat();
    let _ = run(&mut w, &trace(recs));
    assert!(!has_report(&w, report), "{:?}", w.coverage.unsupported);
    let (mut w, _) = world_in_area();
    let recs = [vec![tick(0, 1), time(0, 0, false), cmd("start"), cmd("end")], attack(0)].concat();
    let _ = run(&mut w, &trace(recs));
    assert!(has_report(&w, report), "ended: no longer running");
}

/// A root `AddBuff` is driven only for `DismemberUnitFX`'s buff (the system mechanics'
/// `DismemberedUnitBuff`): any other blueprint has an origin the replay can't place.
#[test]
fn a_root_add_buff_of_another_blueprint_is_not_driven() {
    let (mut w, _) = world_in_area();
    let recs = vec![
        tick(0, 1),
        time(0, 0, false),
        json!({"k": "rule", "t": 0, "id": 1, "depth": 0, "class": "Kingmaker.RuleSystem.Rules.RuleCanApplyBuff",
            "initiator": {"$unit": "u"}, "fields": {"Blueprint": "0da0c194d6e1d43419eb8d990b28e0ab"}}),
        json!({"k": "rule_end", "t": 0, "id": 1}),
    ];
    let out = run(&mut w, &trace(recs));
    assert!(has_report(&w, "driven replay: a root AddBuff of an unestablished origin"));
    assert_eq!(out.skipped.get("RuleCanApplyBuff"), Some(&1));
}

/// The sim's turn-based state is checked against every `time` record's.
#[test]
fn the_turn_based_state_is_checked_against_the_time_records() {
    let (mut w, _) = world_in_area();
    let _ = run(&mut w, &trace(vec![tick(0, 1), time(0, 0, false)]));
    assert!(!has_report(&w, "driven replay: turn-based combat state differs"));
    let _ = run(&mut w, &trace(vec![tick(0, 1), time(0, 0, true)]));
    assert!(has_report(&w, "driven replay: turn-based combat state differs"));
}

/// A presentation root that holds a draw, a mutation or a nested rule is not echoed.
#[test]
fn an_echoed_root_must_be_inert() {
    let clean = [rec("h", json!({})), rec("rule_end", json!({}))];
    assert!(echo_span_is_inert(&clean));
    assert!(echo_span_is_inert(&[rec("draw", json!({"c": "pres"}))]));
    assert!(!echo_span_is_inert(&[rec("draw", json!({"c": "sim", "rule": 3}))]));
    assert!(!echo_span_is_inert(&[rec("mut", json!({"what": "damage"}))]));
    assert!(!echo_span_is_inert(&[rec("rule", json!({"depth": 1}))]));
}

/// The observation rows establish a position only where they were written for a unit that was
/// awake: at every prefix from the roll to the impact and the next one, the same place and not
/// moving. A unit without rows, a gap, a move or a missing next prefix leaves it unknown.
#[test]
fn a_position_at_impact_needs_rows_at_every_frame() {
    let at = [64.62313, 35.27422, 156.008_79];
    let still = Row { pos: at, moving: false };
    let mut p = Positions::default();
    p.known.insert("5198".into());
    let seen = p.seen.entry("5198".into()).or_default();
    for t in 11..=20 {
        seen.insert(t, still);
    }
    assert!(p.still("5198", at, 10, 20, Some(still)));
    assert!(!p.still("51DD", at, 10, 20, Some(still)), "no rows: unknown, not still");
    assert!(!p.still("5198", at, 10, 20, None), "the impact frame's own movement is unbounded");
    assert!(!p.still("5198", at, 9, 20, Some(still)), "a prefix without a row (not awake)");
    let moved = Row { pos: [64.7, 35.27422, 156.008_79], moving: false };
    assert!(!p.still("5198", at, 10, 20, Some(moved)));
    p.seen.get_mut("5198").expect("rows").insert(15, Row { pos: at, moving: true });
    assert!(!p.still("5198", at, 10, 20, Some(still)), "moving in place");
    assert_eq!(
        Row::parse("64.62313,35.27422,156.008789|mv=0|ic=1|cmd=UnitAttack*;|q=0"),
        Some(still)
    );
    assert_eq!(Row::parse("1,2|mv=0"), None);
}

/// Only units awake at the window's first observed tick get rows (`Observe.Tick`'s known set).
#[test]
fn rows_are_known_for_the_first_ticks_units_only() {
    let row = |t: i64, u: &str| rec("obs", json!({"t": t, "ev": "u", "u": u, "s": "0,0,0|mv=0"}));
    let p = Positions::from_trace(&trace(vec![row(3, "~fx"), row(3, "a"), row(4, "b")]));
    assert!(p.known.contains("a"));
    assert!(!p.known.contains("b"));
    assert!(!p.known.contains("~fx"));
}

/// A resolve is matched to its attack by the recorded rule identity: two shots of one pair
/// of units in one fight don't swap.
#[test]
fn resolves_match_by_the_attacks_identity() {
    let mk = |source| RuleAttackWithWeaponResolve {
        base: RuleBase::default(),
        initiator: EntityIdx(1),
        target: EntityIdx(2),
        damage: None,
        attack_hit: true,
        weapon_melee: false,
        source_id: Some(source),
    };
    let pending = vec![mk(91), mk(114)];
    assert_eq!(matching_resolves(&pending, EntityIdx(1), EntityIdx(2), Some(114)), vec![1]);
    assert_eq!(matching_resolves(&pending, EntityIdx(1), EntityIdx(2), Some(91)), vec![0]);
    assert!(matching_resolves(&pending, EntityIdx(1), EntityIdx(2), Some(7)).is_empty());
    assert_eq!(matching_resolves(&pending, EntityIdx(1), EntityIdx(2), None).len(), 2);
}

/// A recorded `DistanceDamageBonus` outcome is its about-to-trigger handler's: whether that
/// changed the rule. The did-trigger call of the same fact changes nothing and doesn't count.
#[test]
fn a_recorded_distance_outcome_is_the_about_handlers() {
    let h = |id: i64, phase: &str| {
        rec(
            "h",
            json!({"id": id, "phase": phase, "class": "Kingmaker.Designers.Mechanics.Facts.DistanceDamageBonus",
            "fact": {"$fact": "51BB"}}),
        )
    };
    let end = |id: i64, changed: bool| {
        rec(
            "h_end",
            json!({"id": id, "changes": if changed { json!([["m_DamageBundle", []]]) } else { json!([]) }}),
        )
    };
    let span = [h(1, "about"), end(1, true), h(2, "did"), end(2, false)];
    assert_eq!(recorded_distance(&span).get("51BB"), Some(&true));
    let span = [h(1, "about"), end(1, false)];
    assert_eq!(recorded_distance(&span).get("51BB"), Some(&false));
}

/// An attack environment with a value the oracle couldn't read (flanking outside turn-based
/// combat is written as null) is not taken as false: no environment, reported.
#[test]
fn an_unrecorded_environment_value_is_not_false() {
    let (mut w, u) = world_in_area();
    let env = |flanked: J| {
        json!({"k": "env", "rule": "RuleAttackRoll", "initiator": {"$unit": "u"}, "target": {"$unit": "u"},
            "target_flanked": flanked, "target_remembers_initiator": true, "target_can_act_in_combat": true,
            "concealment_target_from_initiator": 0, "concealment_initiator_from_target": 0})
    };
    let t = trace(vec![env(json!(false))]);
    assert!(env_in(&t, 0, 0, u, u, &mut w).is_some());
    assert!(!has_report(&w, "driven replay"));
    let t = trace(vec![env(J::Null)]);
    assert!(env_in(&t, 0, 0, u, u, &mut w).is_none());
    assert!(has_report(&w, "driven replay: an attack environment value"));
}

/// With `StartTurn`'s entry recorded (`turn_start_begin`), a mutation recorded before it in the
/// frame is ordered (no report), the turn is installed and `TurnStartTime` is the postfix's
/// (`turn_start`) game time. (The sweep's timing: the next test.)
#[test]
fn the_sweep_runs_at_the_recorded_turn_start_entry() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let f = timed_buff(&mut w, u, 960, S);
    let recs = vec![
        input(-1, "tbm:on"),
        tick(0, mode::DEFAULT),
        time(0, 2 * S, false),
        tb(0, "combat_start", None, 2 * S),
        tick(1, mode::DEFAULT),
        time(1, 3 * S, true),
        tick(2, mode::DEFAULT),
        json!({"k": "mut", "t": 2, "what": "fact_detach", "owner": "u"}),
        tb(2, "turn_start_begin", Some("u"), 4 * S),
        tb(2, "turn_start", Some("u"), 4 * S),
        time(2, 4 * S, true),
    ];
    let _ = run(&mut w, &trace(recs));
    assert!(!alive(&w, u, f), "removed by the sweep");
    assert_eq!(w.combat.current_unit, Some(u));
    assert_eq!(w.combat.turn_start_time, TimeSpan(4 * S));
    assert!(!has_report(&w, "CombatController.StartTurn"), "{:?}", w.coverage.unsupported);
    assert!(!has_report(&w, "driven replay"), "{:?}", w.coverage.unsupported);
}

/// `StartTurn` returns early for a unit not in combat: no turn is installed and `TurnStartTime`
/// stays.
#[test]
fn a_turn_start_of_a_unit_out_of_combat_installs_nothing() {
    let (mut w, _) = world_in_area();
    let recs = vec![
        input(-1, "tbm:on"),
        tick(0, mode::DEFAULT),
        time(0, 2 * S, false),
        tb(0, "turn_start_begin", Some("u"), 2 * S),
        tb(0, "turn_start", Some("u"), 2 * S),
    ];
    let before = w.combat.turn_start_time;
    let _ = run(&mut w, &trace(recs));
    assert_eq!(w.combat.current_unit, None);
    assert_eq!(w.combat.turn_start_time, before);
}

/// A root rule inside `StartTurn` (between its entry and its postfix) is reported.
#[test]
fn a_root_inside_start_turn_is_reported() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let recs = vec![
        input(-1, "tbm:on"),
        tick(0, mode::DEFAULT),
        tb(0, "turn_start_begin", Some("u"), 0),
        json!({"k": "rule", "t": 0, "id": 1, "depth": 0, "class": "Kingmaker.RuleSystem.Rules.RuleInitiativeRoll",
            "initiator": {"$unit": "u"}, "fields": {}}),
        json!({"k": "rule_end", "t": 0, "id": 1}),
        tb(0, "turn_start", Some("u"), 0),
    ];
    let _ = run(&mut w, &trace(recs));
    assert!(has_report(&w, "CombatController.StartTurn: a root rule"));
}

/// With `turn_clear` recorded, an ended turn stays current until the clear and is gone after it:
/// a due buff of its unit is ticked in the frame before the clear and not after; the turn status
/// gates agree (no report). A gate that disagrees is reported.
#[test]
fn a_recorded_turn_clear_ends_the_current_turn() {
    let gate =
        |t: i64, d: &str| rec("obs", json!({"t": t, "ev": "turn_status", "g": true, "d": d}));
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let f = timed_buff(&mut w, u, 962, 5 * S);
    let recs = vec![
        input(-1, "tbm:on"),
        tick(0, 1),
        time(0, 2 * S, false),
        tb(0, "combat_start", None, 2 * S),
        tick(1, 1),
        tb(1, "turn_start_begin", Some("u"), 3 * S),
        tb(1, "turn_start", Some("u"), 3 * S),
        time(1, 3 * S, true),
        tick(2, 1),
        gate(2, "none->u Acting"),
        time(2, 6 * S, true),
        tb(2, "turn_end", Some("u"), 6 * S),
        tb(2, "turn_clear", Some("u"), 6 * S),
        tick(3, 1),
        gate(3, "u Ending->none"),
        time(3, 6 * S, true),
    ];
    let _ = run(&mut w, &trace(recs));
    assert_eq!(w.combat.current_unit, None);
    assert!(alive(&w, u, f), "TurnStartTime (3 s) is before the buff's end: nothing due");
    assert!(!has_report(&w, "driven replay"), "{:?}", w.coverage.unsupported);
    assert!(!has_report(&w, "BuffCollection.Tick"), "{:?}", w.coverage.unsupported);
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let recs = vec![
        input(-1, "tbm:on"),
        tick(0, 1),
        tb(0, "combat_start", None, 0),
        tick(1, 1),
        tb(1, "turn_start_begin", Some("u"), 0),
        tb(1, "turn_start", Some("u"), 0),
        tick(2, 1),
        gate(2, "u Acting->none"),
    ];
    let _ = run(&mut w, &trace(recs));
    assert!(has_report(&w, "driven replay: the current turn differs"));
}

/// A recording with `loot_timer` records fires each timer at its record, not by the sim's clock:
/// a timer long due stays while the recording fires another (here, of an unknown owner: reported),
/// and goes at its own record.
#[test]
fn a_loot_timer_fires_at_its_record() {
    let timer = |t: i64, owner: &str, fact: &str| {
        let mut r = tb(t, "loot_timer", None, 0);
        r["owner"] = json!({"$unit": owner});
        r["fact"] = json!(fact);
        r
    };
    let (mut w, u) = world_in_area();
    let f = timed_buff(&mut w, u, 963, 100 * S);
    w.combat.timers.push(super::super::Timer { due: 0, fact: f });
    let id = w.fact(f).unique_id.clone();
    let recs =
        vec![tick(0, 1), time(0, S, false), timer(0, "x", "0"), tick(1, 1), time(1, 2 * S, false)];
    let _ = run(&mut w, &trace(recs));
    assert!(alive(&w, u, f), "not fired by the clock");
    assert!(has_report(&w, "driven replay: a loot timer the sim didn't start"));
    let recs = vec![tick(2, 1), time(2, 3 * S, false), timer(2, "u", "not-the-buff")];
    let _ = run(&mut w, &trace(recs));
    assert!(alive(&w, u, f), "another buff's timer: the owner's only timer isn't taken for it");
    let recs = vec![tick(3, 1), time(3, 3 * S, false), timer(3, "u", &id)];
    let _ = run(&mut w, &trace(recs));
    assert!(!alive(&w, u, f), "fired at its record");
    assert!(w.combat.timers.is_empty());
}

/// The recorded `State.AwakeUnits` (the next prefix's) settles the units whose state the sim
/// can't establish, and a unit whose state it establishes must agree.
#[test]
fn the_recorded_awake_set_settles_unknown_units() {
    let awake_set =
        |t: i64, units: &[&str]| rec("obs", json!({"t": t, "ev": "awake_set", "units": units}));
    let awake =
        |t: i64, u: &str, on: bool| rec("obs", json!({"t": t, "ev": "awake", "u": u, "on": on}));
    // Not in combat and not in the party: unknown to the sim.
    let (mut w, u) = world_in_area();
    let recs = vec![tick(0, 1), awake_set(0, &["u"]), tick(1, 1), awake(1, "u", false), tick(2, 1)];
    let out = run(&mut w, &trace(recs));
    assert!(out.awake_recorded >= 2, "{}", out.awake_recorded);
    assert!(w.combat.awake.is_empty(), "frame 1's snapshot (prefix 2) has u asleep");
    assert!(!has_report(&w, "SleepingUnitsController"), "{:?}", w.coverage.unsupported);
    // In combat: awake for the sim; a recording that has it asleep is reported.
    let (mut w, u2) = world_in_area();
    w.unit_mut(u2).expect("unit").combat.in_combat = true;
    let recs = vec![tick(0, 1), awake_set(0, &[]), tick(1, 1)];
    let _ = run(&mut w, &trace(recs));
    assert!(has_report(&w, "SleepingUnitsController: the sim's awake state differs"));
    let _ = u;
}

/// A ranged resolve's positions at impact come from its own `env` record.
#[test]
fn a_resolve_takes_its_positions_from_its_env_record() {
    let env = json!({"k": "env", "t": 5, "rule": "RuleAttackWithWeaponResolve",
        "initiator": {"$unit": "a"}, "target": {"$unit": "b"},
        "at": {"initiator_pos": [1.0, 2.0, 3.0], "target_pos": [4.0, 5.0, 6.0]}});
    let t = trace(vec![
        json!({"k": "rule", "t": 5, "id": 9, "depth": 0}),
        env,
        json!({"k": "rule_end", "id": 9}),
    ]);
    assert_eq!(resolve_env(&t, 0, 2, "a", "b"), Some(([1.0, 2.0, 3.0], [4.0, 5.0, 6.0])));
    assert_eq!(resolve_env(&t, 0, 2, "b", "a"), None);
}

/// The sweep runs at `StartTurn`'s entry, with the previous `TurnStartTime`, before the frame's
/// later controllers: a buff due before the previous turn start goes before the frame's
/// initiative root, one due between the two turn starts only in the frame's buffs controller,
/// after that root. A sweep at the postfix (with the new time) or none at all fails this.
#[test]
fn the_sweep_uses_the_previous_turn_start_time_at_the_entry() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let early = timed_buff(&mut w, u, 970, 2 * S);
    let late = timed_buff(&mut w, u, 971, 4 * S);
    let (early_id, late_id) = (w.fact(early).unique_id.clone(), w.fact(late).unique_id.clone());
    let recs = vec![
        input(-1, "tbm:on"),
        tick(0, 1),
        time(0, 3 * S, false),
        tb(0, "combat_start", None, 3 * S),
        tick(1, 1),
        time(1, 3 * S, true),
        tick(2, 1),
        tb(2, "turn_start_begin", Some("u"), 5 * S),
        tb(2, "turn_start", Some("u"), 5 * S),
        time(2, 5 * S, true),
        json!({"k": "rule", "t": 2, "id": 1, "depth": 0, "class": "Kingmaker.RuleSystem.Rules.RuleInitiativeRoll",
            "initiator": {"$unit": "u"}, "fields": {}}),
        json!({"k": "rule_end", "t": 2, "id": 1}),
    ];
    let _ = run(&mut w, &trace(recs));
    let at = |id: &str| {
        w.events.records.iter().position(|e| {
            e.fields.get("what").and_then(J::as_str) == Some("fact_detach")
                && e.fields["fact"]["id"].as_str().or(e.fields["fact"]["$fact"].as_str())
                    == Some(id)
        })
    };
    let root = w.events.records.iter().position(|e| e.k == "rule").expect("the root");
    let (e, l) = (at(&early_id).expect("early removed"), at(&late_id).expect("late removed"));
    assert!(e < root, "the sweep (previous TurnStartTime 3 s) removes the early buff first");
    assert!(root < l, "the late buff (due 4 s) goes in the frame's buffs controller");
}

/// `StartTurn` returns early for a unit in combat that the turn controller doesn't hold
/// (`FindUnitInfo` is null): no turn is installed.
#[test]
fn a_turn_start_of_a_unit_outside_the_controllers_units_installs_nothing() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    w.combat.tb_units = Some(Vec::new());
    let recs = vec![
        tick(0, 1),
        tb(0, "turn_start_begin", Some("u"), 0),
        tb(0, "turn_start", Some("u"), 0),
    ];
    let _ = run(&mut w, &trace(recs));
    assert_eq!(w.combat.current_unit, None);
    w.combat.tb_units = Some(vec![u]);
    let recs = vec![
        tick(1, 1),
        tb(1, "turn_start_begin", Some("u"), 0),
        tb(1, "turn_start", Some("u"), 0),
    ];
    let _ = run(&mut w, &trace(recs));
    assert_eq!(w.combat.current_unit, Some(u), "a member's turn is installed");
}

/// A sort whose units differ from the controller's is reported; a recorded join adds the unit.
#[test]
fn sorts_are_checked_against_the_controllers_units() {
    let sort = |t: i64, units: &[&str]| {
        let list: Vec<J> = units.iter().map(|u| json!({"unit": u, "initiative": 0})).collect();
        json!({"k": "tb", "t": t, "ev": "sort", "reason": "x", "units": list})
    };
    let (mut w, u) = world_in_area();
    let recs = vec![
        input(-1, "tbm:on"),
        tick(0, 1),
        tb(0, "combat_start", None, 0),
        sort(0, &["u"]),
        rec("obs", json!({"t": 0, "ev": "join_combat", "u": "u"})),
        sort(0, &["u"]),
    ];
    let out = run(&mut w, &trace(recs));
    assert_eq!(out.sorts.members_mismatch, 1, "before the join only");
    assert!(has_report(&w, "driven replay: the turn controller's units differ"));
    assert_eq!(w.combat.tb_units.as_deref(), Some(&[u][..]));
}

/// A recorded tutorial fact takes the sim's next unique id; a different recorded id is reported.
#[test]
fn a_tutorial_fact_takes_the_next_unique_id() {
    let tut = |id: &str| json!({"k": "mut", "t": 0, "what": "fact_attach", "class": "Tutorial", "fact": {"$fact": id}});
    let (mut w, _) = world_in_area();
    let next = format!("{:X}", w.player.max_unique_id);
    let _ = run(&mut w, &trace(vec![tick(0, 1), tut(&next)]));
    assert!(!has_report(&w, "driven replay: a tutorial"), "{:?}", w.coverage.unsupported);
    assert_ne!(format!("{:X}", w.player.max_unique_id), next, "the id is taken");
    let _ = run(&mut w, &trace(vec![tick(1, 1), tut("0")]));
    assert!(has_report(&w, "driven replay: a tutorial fact's unique id differs"));
}

/// `ShouldBeSleeping`: a sleepless unit (here a follower) is awake, so a recording that has it
/// asleep is reported, not taken.
#[test]
fn a_sleepless_unit_is_awake_for_the_sim() {
    let awake_set =
        |t: i64, units: &[&str]| rec("obs", json!({"t": t, "ev": "awake_set", "units": units}));
    let (mut w, u) = world_in_area();
    crate::components::etudes::retain(&mut w, "unit:u.Sleepless");
    let out = run(&mut w, &trace(vec![tick(0, 1), awake_set(0, &[]), tick(1, 1)]));
    assert_eq!(out.awake_recorded, 0);
    assert!(has_report(&w, "SleepingUnitsController: the sim's awake state differs"));
    assert_eq!(w.combat.awake, [u]);
}

/// A clear right after its turn's `TurnController.End` is the turn controller's
/// (`CombatController.Tick`), before the frame's buffs controller: the unit's buff, due at the
/// new `TurnStartTime`, isn't ticked in that frame (it would be with the clear after the buffs
/// controller), and the frame's later initiative root isn't late.
#[test]
fn a_clear_after_its_turn_end_comes_before_the_buffs_controller() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let f = timed_buff(&mut w, u, 975, 4 * S);
    let root = json!({"k": "rule", "t": 1, "id": 1, "depth": 0, "class": "Kingmaker.RuleSystem.Rules.RuleInitiativeRoll",
        "initiator": {"$unit": "u"}, "fields": {}});
    let recs = vec![
        input(-1, "tbm:on"),
        tick(0, 1),
        time(0, 3 * S, false),
        tb(0, "combat_start", None, 3 * S),
        tick(1, 1),
        // The sweep (previous TurnStartTime 3 s) leaves the buff; TurnStartTime becomes 5 s.
        tb(1, "turn_start_begin", Some("u"), 5 * S),
        tb(1, "turn_start", Some("u"), 5 * S),
        time(1, 5 * S, true),
        tb(1, "turn_end", Some("u"), 5 * S),
        tb(1, "turn_clear", Some("u"), 5 * S),
        root,
        json!({"k": "rule_end", "t": 1, "id": 1}),
    ];
    let _ = run(&mut w, &trace(recs));
    assert!(alive(&w, u, f), "no current turn when the buffs controller ran");
    assert!(!has_report(&w, "driven replay"), "{:?}", w.coverage.unsupported);
}

/// A current unit dying in the frame's life controller is removed from the turn controller
/// (`LeaveCombat` → `RemoveUnit`), which clears its turn: the recorded clear is that one, after
/// the buffs controller (the frame's later initiative root is late, so reported), and without a
/// report of its own. A clear of a living unit with no established source is reported.
#[test]
fn a_dying_current_units_clear_is_the_life_controllers() {
    let start = |recs: &mut Vec<J>| {
        recs.extend([
            input(-1, "tbm:on"),
            tick(0, 1),
            time(0, 3 * S, false),
            tb(0, "combat_start", None, 3 * S),
            tick(1, 1),
            tb(1, "turn_start_begin", Some("u"), 5 * S),
            tb(1, "turn_start", Some("u"), 5 * S),
            time(1, 5 * S, true),
            tick(2, 1),
            time(2, 5 * S, true),
        ]);
    };
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let mut recs = Vec::new();
    start(&mut recs);
    recs.push(tb(2, "turn_clear", Some("u"), 5 * S));
    let _ = run(&mut w, &trace(recs.clone()));
    assert!(has_report(&w, "driven replay: a turn clear whose source is not established"));
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let mut recs = Vec::new();
    start(&mut recs);
    // Marked for death after the turn started (the frame's life controller kills it).
    let mut t = trace(recs.clone());
    let _ = run(&mut w, &t);
    w.unit_mut(u).expect("unit").marked_for_death = true;
    t.records = vec![
        tick(3, 1),
        tb(3, "turn_clear", Some("u"), 5 * S),
        json!({"k": "rule", "t": 3, "id": 1, "depth": 0, "class": "Kingmaker.RuleSystem.Rules.RuleInitiativeRoll",
            "initiator": {"$unit": "u"}, "fields": {}}),
        json!({"k": "rule_end", "t": 3, "id": 1}),
    ];
    // `run` starts with combat off: carry the turn controller's state over.
    let (units, cur) = (w.combat.tb_units.clone(), w.combat.current_unit);
    w.combat.tb_units = units;
    w.combat.current_unit = cur;
    let _ = run(&mut w, &t);
    assert_eq!(w.unit(u).expect("unit").life_state, super::super::life::state::DEAD);
    assert!(!has_report(&w, "driven replay: a turn clear"), "{:?}", w.coverage.unsupported);
    assert!(has_report(&w, "driven replay: a root RuleInitiativeRoll can't be placed"));
}

/// A turn status naming a unit the sim doesn't have is reported, not taken as no turn.
#[test]
fn a_turn_status_of_an_unknown_unit_is_reported() {
    let gate =
        |t: i64, d: &str| rec("obs", json!({"t": t, "ev": "turn_status", "g": true, "d": d}));
    let (mut w, _) = world_in_area();
    let recs =
        vec![tick(0, 1), tb(0, "turn_clear", None, 0), tick(1, 1), gate(1, "none->missing Acting")];
    let _ = run(&mut w, &trace(recs));
    assert!(has_report(&w, "driven replay: the current turn's unit is unknown"));
}

/// Stepping the records: the sweep has run once `turn_start_begin` is processed, before
/// `turn_start` (the postfix), with the previous `TurnStartTime`.
#[test]
fn the_sweep_has_run_when_the_entry_is_processed() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let early = timed_buff(&mut w, u, 980, 2 * S);
    let late = timed_buff(&mut w, u, 981, 4 * S);
    let t = trace(vec![
        input(-1, "tbm:on"),
        tick(0, 1),
        time(0, 3 * S, false),
        tb(0, "combat_start", None, 3 * S),
        tick(1, 1),
        tb(1, "turn_start_begin", Some("u"), 5 * S),
        tb(1, "turn_start", Some("u"), 5 * S),
    ]);
    let mut d = Driver::start(&mut w, &t, &t);
    let mut i = 0;
    while i < t.records.len() && t.records[i]["ev"] != "turn_start_begin" {
        i = d.record(&mut w, i);
    }
    assert!(alive(&w, u, early), "before the entry");
    i = d.record(&mut w, i);
    assert!(!alive(&w, u, early), "the sweep ran at the entry");
    assert!(alive(&w, u, late), "with the previous TurnStartTime (3 s)");
    assert_eq!(w.combat.turn_start_time, TimeSpan(3 * S), "not yet the postfix's");
    let _ = d.record(&mut w, i);
    assert_eq!(w.combat.turn_start_time, TimeSpan(5 * S));
    let _ = d.finish(&mut w);
}

/// `HandleCombatStart` fills `m_Units` from the entity pool: a unit out of the game, even in
/// combat, isn't in it, so its turn start returns early.
#[test]
fn an_out_of_game_combatant_is_not_in_the_controllers_units() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    w.entity_mut(u).is_in_game = false;
    let recs = vec![
        input(-1, "tbm:on"),
        tick(0, 1),
        tb(0, "combat_start", None, 0),
        tick(1, 1),
        tb(1, "turn_start_begin", Some("u"), 0),
        tb(1, "turn_start", Some("u"), 0),
    ];
    let _ = run(&mut w, &trace(recs));
    assert_eq!(w.combat.tb_units.as_deref(), Some(&[][..]));
    assert_eq!(w.combat.current_unit, None);
}

/// `State.Units` is the states' units without fakes (`NotFake`): a fake unit, or one in no
/// state, isn't in `m_Units` even in combat and in the game.
#[test]
fn fakes_and_units_outside_the_states_are_not_in_the_controllers_units() {
    let recs = || {
        trace(vec![
            input(-1, "tbm:on"),
            tick(0, 1),
            tb(0, "combat_start", None, 0),
            tick(1, 1),
            tb(1, "turn_start_begin", Some("u"), 0),
            tb(1, "turn_start", Some("u"), 0),
        ])
    };
    let (mut w, u) = world_with_unit();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let _ = run(&mut w, &recs());
    assert_eq!(w.combat.tb_units.as_deref(), Some(&[][..]), "in no state");
    assert_eq!(w.combat.current_unit, None);
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    use crate::facts::tests::obj;
    w.content.insert(crate::content::Blueprint {
        guid: wr_content::Guid(990),
        name: "Fake".into(),
        obj: obj(
            "Kingmaker.Blueprints.BlueprintUnit",
            &[("IsFake", wr_content::node::Node::Bool(true))],
        ),
        source: crate::content::Source::Bbp,
    });
    w.unit_mut(u).expect("unit").blueprint = Some(wr_content::Guid(990));
    let _ = run(&mut w, &recs());
    assert_eq!(w.combat.tb_units.as_deref(), Some(&[][..]), "fake");
    assert_eq!(w.combat.current_unit, None);
}

/// With the turn port, `cmds` rows are changes from empty: the first `leave_tick` establishes
/// that units without a row have no commands.
#[test]
fn the_first_leave_tick_establishes_empty_commands() {
    let (mut w, _) = world_in_area();
    let gate = "tb=0|has_enemy=0|had_enemy=0|passing=0|current=|next=|player_in_combat=0";
    let recs = vec![
        tick(0, mode::DEFAULT),
        rec("obs", json!({"t": 0, "ev": "turn_tick", "s": "guard=0|changed=0"})),
        rec("obs", json!({"t": 0, "ev": "leave_tick", "via": "Tick", "gate": gate, "groups": []})),
        rec(
            "obs",
            json!({"t": 99, "ev": "choose_next", "current": null, "next": null, "units": []}),
        ),
    ];
    let _ = run(&mut w, &trace(recs));
    assert!(w.combat.turns.on);
    assert_eq!(w.combat.groups.commands.as_ref().map(HashMap::len), Some(0));
}

/// Faction rows compare the counts as a mapping: the multiset's key order follows its history.
#[test]
fn faction_rows_compare_counts_not_key_order() {
    assert!(same_factions("f=a:2,b:1|af=x|efe=0", "f=b:1,a:2|af=x|efe=0"));
    assert!(!same_factions("f=a:2,b:1|af=x|efe=0", "f=a:1,b:1|af=x|efe=0"));
    assert!(!same_factions("f=a:2|af=x|efe=0", "f=a:2|af=y|efe=0"));
}

/// A cutscene frame's pending turn phase runs as the next (default-mode) frame starts, with the
/// cutscene's mode: `CombatController.Tick` doesn't choose a unit outside the default mode.
#[test]
fn a_drained_turn_phase_keeps_its_frames_mode() {
    let (mut w, u) = world_in_area();
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    let tt = |t: i64| rec("obs", json!({"t": t, "ev": "turn_tick", "s": "guard=0|changed=0"}));
    let mut start = tb(0, "combat_start", None, 0);
    start["units"] = json!([{"unit": "u"}]);
    start["party_combat_state_changed"] = json!(true);
    let recs = vec![
        tick(0, mode::DEFAULT),
        tt(0),
        start,
        // The cutscene frame: its turn controller ticks, its choice phase is left pending.
        tick(1, mode::CUTSCENE),
        rec("obs", json!({"t": 1, "ev": "turn_tick", "s": "guard=0|changed=1"})),
        // The default frame chooses (its own recorded input).
        tick(2, mode::DEFAULT),
        rec(
            "obs",
            json!({"t": 2, "ev": "choose_next", "current": null, "next": null, "units": ["u|000|vis=1|saddled=0"]}),
        ),
    ];
    let _ = run(&mut w, &trace(recs));
    assert!(w.combat.turns.on);
    assert_eq!(w.combat.turns.next, Some(u), "the default frame chose");
    assert!(
        !has_report(&w, "CombatController.ChooseNextUnit: a call the recording doesn't have"),
        "{:?}",
        w.coverage.unsupported
    );
}

/// A root's recorded call site places it: the UI after `Game.Tick`, coroutines (inputs), the
/// command controller, the AI in the turn controller's tick or in `StartTurn`; anything else
/// in `Game.Tick` (or nowhere known) isn't placed.
#[test]
fn a_root_call_site_places_it() {
    let tick = "Kingmaker.GameModes.GameMode.Tick < Kingmaker.Game.Tick < Kingmaker.Runner.Update";
    let ui = "Kingmaker.UI._ConsoleUI.TurnBasedMode.InitiativeTrackerUnitVM.UpdateData < UniRx.Subject`1[[UniRx.Unit]].OnNext < UniRx.MainThreadDispatcher.LateUpdate";
    assert_eq!(Caller::of(ui), Some(Caller::Ui));
    let hover = "Kingmaker.UnitLogic.Commands.UnitAttack+<EnumerateFullAttack>d__91.MoveNext < TurnBased.Controllers.TurnController.HandlePortraitHover < UnityEngine.SetupCoroutine.InvokeMoveNext";
    assert_eq!(Caller::of(hover), Some(Caller::Input));
    let cmd = format!(
        "Kingmaker.UnitLogic.Commands.UnitAttack.CreateSingleAttack < Kingmaker.Controllers.Units.UnitCommandController.TickCommand < Kingmaker.Controllers.Units.BaseUnitController.Tick < {tick}"
    );
    assert_eq!(Caller::of(&cmd), Some(Caller::Command));
    let ai = format!(
        "Kingmaker.AI.AiBrainController.SelectAction < Kingmaker.AI.AiBrainController.ForceTick < TurnBased.Controllers.TurnController.Tick < TurnBased.Controllers.CombatController.Tick < {tick}"
    );
    assert_eq!(Caller::of(&ai), Some(Caller::Ai));
    let start = format!(
        "Kingmaker.AI.AiBrainController.SelectAction < Kingmaker.AI.AiBrainController.ForceTick < TurnBased.Controllers.TurnController.Prepare < TurnBased.Controllers.TurnController.Start < TurnBased.Controllers.CombatController.StartTurn < TurnBased.Controllers.CombatController.TickTime < Kingmaker.Controllers.TimeController.Tick < {tick}"
    );
    assert_eq!(Caller::of(&start), Some(Caller::AiStartTurn));
    assert_eq!(Caller::AiStartTurn.phase(), Phase::Time);
    // The AI from another controller, or the turn controller's prepare from `Tick` while
    // `StartTurn` is also on the stack, isn't placed; nor is a call site without a known root.
    let other = format!(
        "Kingmaker.AI.AiBrainController.SelectAction < Kingmaker.AI.AiBrainController.Tick < {tick}"
    );
    assert_eq!(Caller::of(&other), None);
    assert_eq!(Caller::of("Kingmaker.UI.Common.UIUtilityItem.GetWeaponData"), None);
    // The old separator (`<`, which compiler-generated names also hold) isn't read.
    assert_eq!(Caller::of(&ui.replace(" < ", "<")), None);
}

/// A root of a presentation class with a call site is computed there, not echoed; one that
/// changes state or that its call site doesn't place is reported and skipped.
#[test]
fn a_root_with_a_call_site_is_planned_not_echoed() {
    let ui = "X.UpdateData < UniRx.MainThreadDispatcher.LateUpdate";
    let root = |id: i64, origin: &str| {
        rec(
            "rule",
            json!({"id": id, "depth": 0, "class": "Kingmaker.RuleSystem.Rules.RuleCheckTargetFlatFooted", "origin": origin}),
        )
    };
    let t = trace(vec![
        root(1, ui),
        rec("env", json!({"rule": "RuleCheckTargetFlatFooted"})),
        rec("rule_end", json!({"id": 1})),
        root(2, ui),
        rec("mut", json!({"what": "damage"})),
        rec("rule_end", json!({"id": 2})),
        root(3, "Nowhere.Known"),
        rec("rule_end", json!({"id": 3})),
    ]);
    assert!(plan_echoes(&t).0.is_empty(), "roots with call sites are never echoed");
    let o = plan_origins(&t);
    assert_eq!(o.get(&0), Some(&(2, Some(Caller::Ui))), "its own env is allowed");
    assert_eq!(o.get(&3), Some(&(5, None)), "a mutation: not inert");
    assert_eq!(o.get(&6), Some(&(7, None)));
}

/// A root computed at its call site holds only its own steps, handlers and its own `env`: a
/// combat join, a mutation, another unit's env or a second env makes it unplaced (reported).
#[test]
fn a_call_site_root_holds_nothing_but_itself() {
    let root = json!({"k": "rule", "class": "Kingmaker.RuleSystem.Rules.RuleCheckTargetFlatFooted",
        "initiator": {"$unit": "a"}, "target": {"$unit": "b"}, "depth": 0});
    let env = |a: &str, b: &str| {
        rec(
            "env",
            json!({"rule": "RuleCheckTargetFlatFooted", "initiator": {"$unit": a}, "target": {"$unit": b}}),
        )
    };
    let steps = [rec("about", json!({})), rec("h", json!({})), rec("h_end", json!({}))];
    let ok = [steps.to_vec(), vec![env("a", "b"), rec("rule_end", json!({}))]].concat();
    assert!(origin_span_is_inert(&root, &ok));
    for bad in [
        rec("obs", json!({"ev": "join_combat"})),
        rec("obs", json!({"ev": "u"})),
        rec("mut", json!({"what": "damage"})),
        rec("h_exc", json!({})),
        rec("draw", json!({"c": "sim"})),
        env("a", "c"),
    ] {
        let span = [steps.to_vec(), vec![bad.clone()]].concat();
        assert!(!origin_span_is_inert(&root, &span), "{bad}");
    }
    assert!(!origin_span_is_inert(&root, &[env("a", "b"), env("a", "b")]), "two envs");
    // Only a flat-footed check has an env of its own.
    let count = json!({"k": "rule", "class": "Kingmaker.RuleSystem.Rules.RuleCalculateAttacksCount",
        "initiator": {"$unit": "a"}, "target": null, "depth": 0});
    assert!(!origin_span_is_inert(&count, &[env("a", "b")]));
}

/// `BuffCollection.Postpone` moves every finite buff time by the time since the last tick that
/// ran to its end (`m_PreviousTime`); with none since the load it moves nothing. A tick that
/// returns early (no buff left) leaves `m_PreviousTime` alone.
#[test]
fn postponing_moves_buffs_by_the_time_since_the_last_full_tick() {
    use crate::combat::life::{buff_collection_postpone, buff_collection_tick};
    let (mut w, u) = world_in_area();
    w.clock.game_time = TimeSpan(100 * S);
    let f = timed_buff(&mut w, u, 952, 110 * S);
    let end = |w: &World| w.fact(f).buff.as_ref().and_then(|b| b.end_time);
    let next_tick = |w: &World| w.fact(f).buff.as_ref().map(|b| b.next_tick_time);
    let tick0 = next_tick(&w);
    buff_collection_postpone(&mut w, u);
    assert_eq!(end(&w), Some(110 * S), "no tick since the load: nothing moves");
    w.clock.game_time = TimeSpan(103 * S);
    buff_collection_tick(&mut w, u);
    assert_eq!(w.unit(u).expect("unit").buffs_previous_time, 103 * S);
    w.clock.game_time = TimeSpan(107 * S);
    buff_collection_postpone(&mut w, u);
    assert_eq!(end(&w), Some(114 * S), "moved by 4 s");
    assert_eq!(next_tick(&w), tick0, "a time at TimeSpan.MaxValue stays");
    assert_eq!(w.unit(u).expect("unit").buffs_previous_time, 107 * S);
    // The buff ends: the tick that removes it returns early and keeps 107 s.
    w.clock.game_time = TimeSpan(120 * S);
    buff_collection_tick(&mut w, u);
    assert!(!alive(&w, u, f));
    assert_eq!(w.unit(u).expect("unit").buffs_previous_time, 107 * S);
}

/// `UnitBuffsController.Activate` runs when a mode that has it becomes current again (Cutscene
/// mode popped): a buff whose parent context's activatable ability the owner no longer has
/// ends now (`RemoveAfterDelayIfInvalid`); one with an ability the owner has stays. Units
/// outside the pool (`Units.All`) aren't swept.
#[test]
fn resuming_a_mode_with_the_buffs_controller_ends_orphaned_toggle_buffs() {
    use crate::facts::tests::obj;
    use wr_content::node::Node;
    let (mut w, u) = world_in_area();
    w.clock.game_time = TimeSpan(100 * S);
    let ability = wr_content::Guid(960);
    w.content.insert(crate::content::Blueprint {
        guid: ability,
        name: "Toggle".into(),
        obj: obj("Kingmaker.UnitLogic.ActivatableAbilities.BlueprintActivatableAbility", &[]),
        source: crate::content::Source::Bbp,
    });
    let f = timed_buff(&mut w, u, 953, 200 * S);
    let g = timed_buff(&mut w, u, 954, 200 * S);
    for x in [f, g] {
        if let Some(c) = w.fact_mut(x).context.as_mut() {
            c.parent_associated_blueprint = Some(ability);
        }
    }
    // The owner has the ability only while `g` is checked: give it a fact of that blueprint
    // and check that nothing ends.
    let held = crate::world::Fact {
        blueprint: Some(ability),
        ..crate::world::Fact::blank(crate::world::FactKind::Feature, "ActivatableAbility")
    };
    let h = w.add_fact(held);
    w.entity_mut(u).facts.list.push(h);
    assert!(crate::combat::life::buffs_invalid_on_activate(&w).is_empty());
    w.entity_mut(u).facts.list.retain(|x| *x != h);
    w.player.mode = "Cutscene";
    w.player.modes_below = vec!["Default"];
    w.before_tick.push(crate::world::BeforeTick::StopMode("Cutscene"));
    crate::runner::execute_before_tick(&mut w);
    assert_eq!(w.player.mode, "Default");
    let end = |x| w.fact(x).buff.as_ref().and_then(|b| b.end_time);
    assert_eq!(end(f), Some(100 * S), "ends now");
    assert_eq!(end(g), Some(100 * S));
    let _ = Node::Null;
    // Out of the pool: not swept by `TickAll`.
    w.player.area_units.clear();
    crate::combat::life::buffs_tick_all(&mut w);
    assert_eq!(w.unit(u).expect("unit").buffs_previous_time, 0);
}
