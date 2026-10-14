//! The turn order, unit groups and the leave controller (`super`, `super::super::groups`).

use std::rc::Rc;

use wr_content::Guid;
use wr_content::node::Node;

use super::*;
use crate::combat::groups::{self, Command, Gate, Memory};
use crate::combat::order::Initiative;
use crate::combat::units::PLAYER_PARTY_GROUP;
use crate::content::{Blueprint, Source};
use crate::stats::types::StatType as S;
use crate::world::{Entity, EntityKind, FactsManager};

const ENEMY_FACTION: u128 = 0xe1;
const PARTY_FACTION: u128 = 0xa1;

fn faction(w: &World, g: u128, everyone: bool) {
    let o = crate::facts::tests::obj(
        "Kingmaker.Blueprints.BlueprintFaction",
        &[("EnemyForEveryone", Node::Bool(everyone))],
    );
    w.content.insert(Blueprint {
        guid: Guid(g),
        name: format!("F{g}"),
        obj: o,
        source: Source::Bbp,
    });
}

/// An empty world with the two factions: the enemies' is everyone's enemy.
pub(crate) fn world() -> World {
    let w = World::new(
        Rc::new(crate::content::Content::empty()),
        Rc::new(crate::registry::Registry::default()),
    );
    faction(&w, ENEMY_FACTION, true);
    faction(&w, PARTY_FACTION, false);
    w
}

/// A unit in the area's state, in the game: the party's (player faction) or an enemy of `group`.
pub(crate) fn unit(w: &mut World, id: &str, group: Option<&str>, initiative: i32) -> EntityIdx {
    let party = group.is_none();
    let mut x = crate::unit::Unit {
        stats: crate::stats::value::Stats::fresh(),
        is_player_faction: party,
        faction: Some(Guid(if party { PARTY_FACTION } else { ENEMY_FACTION })),
        group_id: group.map(str::to_owned),
        ..crate::unit::Unit::default()
    };
    x.combat.initiative = Initiative { initiative, random: 0 };
    let e = w.add_entity(Entity {
        unique_id: id.into(),
        clr: "UnitEntityData",
        kind: EntityKind::Unit(Box::new(x)),
        facts: FactsManager::default(),
        parts: Vec::new(),
        is_turned_on: false,
        post_load_executed: true,
        is_in_game: true,
        subscribed: false,
        disposing: false,
        is_disposed: false,
    });
    for t in S::ALL {
        crate::stats::ops::update_value(w, e, t);
    }
    w.player.area_units.push(e);
    e
}

/// Turn-based combat with the port on and every unit in combat and in the turn order, in the
/// given order.
pub(crate) fn in_combat(w: &mut World, units: &[EntityIdx]) {
    w.combat.turns.on = true;
    groups::start(w);
    w.combat.tb_mode_on = true;
    w.combat.combat_active = true;
    crate::combat::life::update_turn_based(w);
    for &u in units {
        crate::combat::life::join_combat(w, u);
    }
    w.combat.tb_units = Some(units.to_vec());
    combat_start(w, units.iter().map(|u| TbUnitInfo::new(*u)).collect(), false);
}

fn ids(w: &World, units: &[EntityIdx]) -> Vec<String> {
    units.iter().map(|u| w.entity(*u).unique_id.clone()).collect()
}

/// Every unit visible, none can carry a rider, flags as the sim has them.
fn recorded_choice(w: &World) -> ChooseInput {
    ChooseInput {
        current: w.combat.current_unit.map(|u| w.entity(u).unique_id.clone()),
        next: w.combat.turns.next.map(|u| w.entity(u).unique_id.clone()),
        units: w
            .combat
            .turns
            .units
            .iter()
            .map(|i| ChooseUnit {
                id: w.entity(i.unit).unique_id.clone(),
                surprised: i.surprised,
                acting: i.acting_in_surprise_round,
                marked: i.marked_as_leave,
                visible: Some(true),
                saddled: false,
            })
            .collect(),
    }
}

pub(crate) fn reported(w: &World, prefix: &str) -> bool {
    w.coverage.unsupported.keys().any(|k| k.starts_with(prefix))
}

/// `UpdateGroup`: a held unit in the game joins its group (made on first use, in that order);
/// leaving the game drops it, and the controller's next tick puts it back at the end.
#[test]
fn units_join_their_groups_in_order_and_rejoin_after_leaving_the_game() {
    let mut w = world();
    let a = unit(&mut w, "a", None, 0);
    let e1 = unit(&mut w, "e1", Some("g"), 0);
    let e2 = unit(&mut w, "e2", Some("g"), 0);
    let away = unit(&mut w, "away", Some("h"), 0);
    w.player.area_units.retain(|u| *u != away);
    groups::start(&mut w);
    let list: Vec<_> =
        w.combat.groups.list.iter().map(|g| w.combat.groups.get(*g).id.clone()).collect();
    assert_eq!(list, [Some(PLAYER_PARTY_GROUP.to_owned()), Some("g".to_owned())]);
    let g = w.combat.groups.find("g").expect("g");
    assert_eq!(w.combat.groups.get(g).units, [e1, e2]);
    // Not held by a state: a fake group of its own.
    let fake = groups::group_of(&mut w, away).expect("group");
    assert!(w.combat.groups.get(fake).is_fake());
    crate::entity::set_is_in_game(&mut w, e1, false);
    assert_eq!(w.combat.groups.get(g).units, [e2]);
    crate::entity::set_is_in_game(&mut w, e1, true);
    groups::tick(&mut w);
    assert_eq!(w.combat.groups.get(g).units, [e2, e1]);
    let _ = a;
}

/// The group's combat count follows its members' joins and leaves, and a leave at zero doesn't
/// make it negative.
#[test]
fn the_combat_count_follows_joins_and_leaves() {
    let mut w = world();
    let e1 = unit(&mut w, "e1", Some("g"), 0);
    let e2 = unit(&mut w, "e2", Some("g"), 0);
    w.combat.turns.on = true;
    groups::start(&mut w);
    let g = w.combat.groups.find("g").expect("g");
    crate::combat::life::join_combat(&mut w, e1);
    crate::combat::life::join_combat(&mut w, e2);
    assert_eq!(w.combat.groups.get(g).in_combat, 2);
    crate::combat::life::leave_combat(&mut w, e1);
    assert_eq!(w.combat.groups.get(g).in_combat, 1);
    groups::left_combat(&mut w, e1);
    groups::left_combat(&mut w, e1);
    assert_eq!(w.combat.groups.get(g).in_combat, 0, "never negative");
}

/// The last enemy gone (`HasEnemyInCombat` false after having had one), the party's timer is set
/// to 3 s: the party leaves combat at once, in its group's order, the current unit first; its
/// `RemoveUnit` chooses the next unit (wrapping into a new round: a sort) and clears the turn.
#[test]
fn the_party_leaves_once_no_enemy_is_left() {
    let mut w = world();
    let a = unit(&mut w, "a", None, 20);
    let b = unit(&mut w, "b", None, 10);
    in_combat(&mut w, &[a, b]);
    w.combat.groups.commands = Some(std::collections::HashMap::new());
    let party = w.combat.groups.find(PLAYER_PARTY_GROUP).expect("party");
    w.combat.groups.get_mut(party).memory = Some(Memory::default());
    w.combat.current_unit = Some(a);
    // Two recorded choices: the current unit's removal (`b` chosen), then the next unit's (the
    // walk wraps: `StartRound` returns, everyone marked to leave, and `b` is chosen again).
    let mut first = recorded_choice(&w);
    first.units.iter_mut().for_each(|u| u.marked = true);
    let mut second = first.clone();
    second.current = None;
    second.next = Some("b".into());
    second.units.retain(|u| u.id == "b");
    w.combat.turns.choose.extend([first, second]);
    let gate = Gate { has_enemy: false, had_enemy: true, passing: false };
    groups::leave_tick(&mut w, gate);
    assert!(!crate::combat::is_in_combat(&w, a) && !crate::combat::is_in_combat(&w, b));
    assert_eq!(w.combat.leave_log, [a, b]);
    assert_eq!(w.combat.groups.get(party).in_combat, 0);
    assert_eq!(w.combat.current_unit, None);
    // Every unit marked to leave: `StartRound` returns without sorting.
    assert_eq!(w.combat.turns.log, [TbEvent::Clear(a)]);
    assert!(w.combat.turns.units.is_empty());
    assert!(w.combat.turns.choose.is_empty(), "both recorded choices used");
    assert_eq!(w.unit(a).expect("a").combat.initiative.initiative, 0, "the combat state cleared");
}

/// While time passes between turns, a group an enemy in combat remembers stays in combat and its
/// timer stays at zero; without the enemy's memory recorded, the decision is reported.
#[test]
fn a_remembered_group_stays_in_combat_and_unrecorded_memory_is_reported() {
    let mut w = world();
    let a = unit(&mut w, "a", None, 20);
    let e = unit(&mut w, "e", Some("g"), 10);
    in_combat(&mut w, &[a, e]);
    w.combat.groups.commands = Some(std::collections::HashMap::new());
    let g = w.combat.groups.find("g").expect("g");
    let party = w.combat.groups.find(PLAYER_PARTY_GROUP).expect("party");
    w.combat.groups.get_mut(party).memory = Some(Memory { dict: vec!["e".into()], list: None });
    w.combat.groups.get_mut(g).memory = Some(Memory { dict: vec!["a".into()], list: None });
    w.clock.set_game_delta_time(0.5);
    let gate = Gate { has_enemy: true, had_enemy: true, passing: true };
    groups::leave_tick(&mut w, gate);
    assert!(crate::combat::is_in_combat(&w, a) && crate::combat::is_in_combat(&w, e));
    assert_eq!(w.combat.groups.get(party).leave_timer, 0.0);
    assert!(!reported(&w, "UnitCombatLeaveController"));
    w.combat.groups.get_mut(g).memory = None;
    w.combat.groups.get_mut(party).memory = None;
    groups::leave_tick(&mut w, gate);
    assert!(reported(&w, "UnitCombatLeaveController: a group's memory is not recorded"));
}

/// An offensive command (an unfinished attack on a living enemy) keeps the group in combat; a
/// finished one doesn't, and the group, with no one remembering it, starts its leave timer.
#[test]
fn an_offensive_command_keeps_a_group_in_combat() {
    let mut w = world();
    let a = unit(&mut w, "a", None, 20);
    let e = unit(&mut w, "e", Some("g"), 10);
    in_combat(&mut w, &[a, e]);
    let party = w.combat.groups.find(PLAYER_PARTY_GROUP).expect("party");
    let g = w.combat.groups.find("g").expect("g");
    w.combat.groups.get_mut(party).memory = Some(Memory::default());
    w.combat.groups.get_mut(g).memory = Some(Memory::default());
    let attack = |finished| Command {
        class: "UnitAttack".into(),
        target: Some("e".into()),
        started: true,
        finished,
    };
    let mut cmds = std::collections::HashMap::new();
    cmds.insert("a".to_owned(), vec![attack(false)]);
    w.combat.groups.commands = Some(cmds);
    w.clock.set_game_delta_time(0.5);
    let gate = Gate { has_enemy: true, had_enemy: true, passing: true };
    groups::leave_tick(&mut w, gate);
    assert_eq!(w.combat.groups.get(party).leave_timer, 0.0);
    w.combat.groups.commands.as_mut().expect("cmds").insert("a".to_owned(), vec![attack(true)]);
    groups::leave_tick(&mut w, gate);
    assert_eq!(w.combat.groups.get(party).leave_timer, 0.5);
    assert!(crate::combat::is_in_combat(&w, a), "under 3 s");
}

/// Outside turn-based combat line of sight is read only when an enemy in combat remembers a
/// member: then it is reported; with no such enemy the decision stands without it.
#[test]
fn line_of_sight_is_reported_only_when_it_decides() {
    let mut w = world();
    let a = unit(&mut w, "a", None, 20);
    let e = unit(&mut w, "e", Some("g"), 10);
    in_combat(&mut w, &[a, e]);
    w.combat.tb_mode_on = false;
    crate::combat::life::update_turn_based(&mut w);
    let g = w.combat.groups.find("g").expect("g");
    let party = w.combat.groups.find(PLAYER_PARTY_GROUP).expect("party");
    w.combat.groups.get_mut(g).memory = Some(Memory::default());
    w.combat.groups.get_mut(party).memory = Some(Memory::default());
    groups::tick_groups(&mut w, false);
    assert!(!reported(&w, "UnitCombatLeaveController: ShouldLeaveCombat reads line of sight"));
    w.combat.groups.get_mut(g).memory = Some(Memory { dict: vec!["a".into()], list: None });
    groups::tick_groups(&mut w, false);
    assert!(reported(&w, "UnitCombatLeaveController: ShouldLeaveCombat reads line of sight"));
}

/// `ChooseNextUnit` after the last unit wraps into a new round: `StartRound` resets the round's
/// overrides and sorts (logged with each unit's initiative), the round advances, and the first
/// unit in the new order is next.
#[test]
fn choosing_past_the_last_unit_starts_a_round() {
    let mut w = world();
    let a = unit(&mut w, "a", None, 10);
    let b = unit(&mut w, "b", Some("g"), 20);
    in_combat(&mut w, &[a, b]);
    w.combat.turns.units[0].initiative_override = Some(30);
    w.combat.current_unit = Some(b);
    let input = recorded_choice(&w);
    w.combat.turns.choose.push_back(input);
    choose_next_unit(&mut w);
    assert_eq!(
        w.combat.turns.log,
        [
            TbEvent::Sort {
                reason: "StartRound".into(),
                units: vec![(b, 20, None, 0), (a, 10, None, 0)]
            },
            TbEvent::RoundStart(2)
        ]
    );
    assert_eq!(w.combat.turns.next, Some(b));
    assert_eq!(ids(&w, w.combat.tb_units.as_ref().expect("units")), ["b", "a"]);
}

/// A unit that isn't the party's enemy is skipped when the recorded `IsVisibleForPlayer` says it
/// isn't visible; a choice without a recorded input is reported.
#[test]
fn an_invisible_ally_is_skipped_and_an_unrecorded_choice_reported() {
    let mut w = world();
    let a = unit(&mut w, "a", None, 30);
    let b = unit(&mut w, "b", None, 20);
    let e = unit(&mut w, "e", Some("g"), 10);
    in_combat(&mut w, &[a, b, e]);
    w.combat.current_unit = Some(a);
    let mut input = recorded_choice(&w);
    input.units[1].visible = Some(false);
    w.combat.turns.choose.push_back(input);
    choose_next_unit(&mut w);
    assert_eq!(w.combat.turns.next, Some(e), "b skipped");
    assert!(!reported(&w, "CombatController.ChooseNextUnit"));
    w.combat.turns.next = None;
    choose_next_unit(&mut w);
    assert!(reported(&w, "CombatController.ChooseNextUnit: a call the recording doesn't have"));
}

/// A unit joining between turns at or above the next unit's initiative takes its place: the next
/// unit's initiative as override, its sequence, the following ties pushed back, and it becomes
/// the next unit.
#[test]
fn a_late_joiner_takes_the_next_units_place() {
    let mut w = world();
    let a = unit(&mut w, "a", None, 16);
    let b = unit(&mut w, "b", None, 16);
    let j = unit(&mut w, "j", None, 17);
    in_combat(&mut w, &[a, b]);
    w.combat.turns.next = Some(a);
    w.combat.turns.time_since_start = 1.0;
    w.unit_mut(b).expect("b").combat.cooldown.initiative = 2.0;
    w.unit_mut(a).expect("a").combat.cooldown.initiative = 1.0;
    for i in &mut w.combat.turns.units {
        i.initiative_processed = true;
    }
    crate::combat::life::join_combat(&mut w, j);
    unit_rolls_initiative(&mut w, j);
    let info = |u| w.combat.turns.units.iter().find(|i| i.unit == u).cloned().expect("info");
    assert_eq!(info(j).initiative_override, Some(16));
    assert_eq!(info(j).initiative_sequence, 0);
    assert_eq!(info(a).initiative_sequence, 1);
    assert_eq!(info(b).initiative_sequence, 2);
    assert_eq!(w.combat.turns.next, Some(j));
    assert_eq!(ids(&w, w.combat.tb_units.as_ref().expect("units")), ["j", "a", "b"]);
    // The next unit: half the time to the following processed unit's turn.
    assert_eq!(w.unit(j).expect("j").combat.cooldown.initiative, 0.5);
    assert!(info(j).initiative_processed);
}

/// `UnitCombatPrepareController`'s handlers run once every unit has rolled, by result
/// (descending), after the cooldowns are spread over the round in initiative order.
#[test]
fn the_prepare_controllers_handlers_run_by_result_after_every_roll() {
    let mut w = world();
    let a = unit(&mut w, "a", None, 0);
    let b = unit(&mut w, "b", None, 0);
    let c = unit(&mut w, "c", None, 0);
    in_combat(&mut w, &[a, b, c]);
    for (u, v) in [(a, 5), (b, 15), (c, 10)] {
        w.unit_mut(u).expect("u").combat.initiative.initiative = v;
        w.combat.turns.prepared.push((u, v));
    }
    crate::combat::initiative::prepare_flush(&mut w);
    let reasons: Vec<Vec<EntityIdx>> = w
        .combat
        .turns
        .log
        .iter()
        .map(|e| match e {
            TbEvent::Sort { units, .. } => units.iter().map(|x| x.0).collect(),
            _ => Vec::new(),
        })
        .collect();
    assert_eq!(reasons.len(), 3, "one sort per handler");
    assert_eq!(reasons[0], [b, c, a]);
    let cd = |u| w.unit(u).expect("u").combat.cooldown.initiative;
    assert_eq!((cd(b), cd(c), cd(a)), (0.0, 3.0, 6.0));
    assert!(w.combat.turns.units.iter().all(|i| i.initiative_processed));
}

/// `HasOffensiveCommand` stops at the first offensive command: a later command whose target the
/// sim doesn't know isn't read (and doesn't make the decision unknown).
#[test]
fn the_first_offensive_command_decides() {
    let mut w = world();
    let a = unit(&mut w, "a", None, 20);
    let e = unit(&mut w, "e", Some("g"), 10);
    in_combat(&mut w, &[a, e]);
    let party = w.combat.groups.find(PLAYER_PARTY_GROUP).expect("party");
    let g = w.combat.groups.find("g").expect("g");
    w.combat.groups.get_mut(party).memory = Some(Memory::default());
    w.combat.groups.get_mut(g).memory = Some(Memory::default());
    let attack = |t: &str| Command {
        class: "UnitAttack".into(),
        target: Some(t.into()),
        started: true,
        finished: false,
    };
    let mut cmds = std::collections::HashMap::new();
    cmds.insert("a".to_owned(), vec![attack("e"), attack("nobody")]);
    w.combat.groups.commands = Some(cmds);
    w.clock.set_game_delta_time(0.5);
    groups::leave_tick(&mut w, Gate { has_enemy: true, had_enemy: true, passing: true });
    assert!(!reported(&w, "UnitCombatLeaveController"), "{:?}", w.coverage.unsupported);
    assert_eq!(w.combat.groups.get(party).leave_timer, 0.0);
}

/// A remembered unit the sim doesn't have is reported: the game's reference could resolve to a
/// living enemy.
#[test]
fn an_unknown_remembered_unit_is_reported() {
    let mut w = world();
    let a = unit(&mut w, "a", None, 20);
    let e = unit(&mut w, "e", Some("g"), 10);
    in_combat(&mut w, &[a, e]);
    w.combat.groups.commands = Some(std::collections::HashMap::new());
    let party = w.combat.groups.find(PLAYER_PARTY_GROUP).expect("party");
    let g = w.combat.groups.find("g").expect("g");
    w.combat.groups.get_mut(g).memory = Some(Memory::default());
    w.combat.groups.get_mut(party).memory = Some(Memory { dict: vec!["ghost".into()], list: None });
    w.clock.set_game_delta_time(0.5);
    groups::tick_groups(&mut w, false);
    assert!(reported(&w, "UnitCombatLeaveController: a remembered unit the sim doesn't have"));
    assert_eq!(w.combat.groups.get(party).leave_timer, 0.0, "no decision, no timer");
}
