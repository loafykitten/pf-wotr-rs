//! Synthetic tests of the unit-creation lifecycle: disposal, previews, the creation queue.

use std::cell::Cell;
use std::rc::Rc;

use wr_content::node::Node;

use crate::facts::tests::{feature, loaded_fact, obj, stat_bonus};
use crate::rulebook::RuleEvent;
use crate::rulebook::bus::{ExternalHandler, RuleStep, Sub};
use crate::stats::types::StatType as S;
use crate::world::{Entity, EntityIdx, EntityKind, FactsManager, World};

fn registry_world() -> (World, EntityIdx) {
    let (mut w, u) = crate::stats::tests::world_with_unit();
    w.registry = Rc::new(crate::components::registry());
    (w, u)
}

fn new_unit(w: &mut World, id: &str) -> EntityIdx {
    w.add_entity(Entity {
        unique_id: id.into(),
        clr: "UnitEntityData",
        kind: EntityKind::Unit(Box::default()),
        facts: FactsManager::default(),
        parts: Vec::new(),
        is_turned_on: false,
        post_load_executed: true,
        is_in_game: true,
        subscribed: false,
        disposing: false,
        is_disposed: false,
    })
}

/// Every subscriber on every list of the bus.
fn subscribers(w: &World) -> Vec<Sub> {
    let b = &w.bus;
    let mut out: Vec<Sub> = Vec::new();
    let lists = b
        .global
        .values()
        .chain(b.rule_global.values())
        .chain(b.unit.values().flat_map(|m| m.values()))
        .chain(b.rule_target.values().flat_map(|m| m.values()))
        .chain(b.rule_initiator.values().flat_map(|m| m.values()));
    for l in lists {
        out.extend(l.list.iter().flatten().copied());
    }
    out.extend(b.about_to_hooks.iter().copied());
    out.extend(b.did_hooks.iter().copied());
    out
}

fn refers_to(w: &World, s: Sub, e: EntityIdx) -> bool {
    match s {
        Sub::Entity(x) => x == e,
        Sub::Fact(f) => w.fact(f).owner == Some(e),
        Sub::Component(c) | Sub::Runtime(c) => w.fact(c.fact).owner == Some(e),
        Sub::External(_) => false,
    }
}

/// `EntityDataBase.Dispose`: the entity unregisters, its facts are disposed (without
/// `Detach`) and nothing of it stays subscribed (the finding on the level-up plan unit).
#[test]
fn dispose_unsubscribes_the_entity_and_its_facts() {
    let (mut w, u) = registry_world();
    w.by_id.insert("u".into(), u);
    // A component with unit-subscriber interfaces (`IUnitGainFactHandler`…).
    let comp = Node::Object(Box::new(obj(
        "Kingmaker.Designers.Mechanics.Buffs.AddStatBonusIfHasFact",
        &[
            ("name", Node::Str("$AddStatBonusIfHasFact$d".into())),
            ("Stat", Node::Enum(i64::from(S::Strength.0))),
            ("Value", Node::Int(2)),
            ("Descriptor", Node::Enum(9)),
        ],
    )));
    let bp = feature(&w, 10, vec![comp]);
    let f = loaded_fact(&mut w, u, &bp, true);
    crate::facts::lifecycle::manager_restore_owner_link(&mut w, u);
    crate::facts::lifecycle::manager_post_load(&mut w, u);
    crate::entity::turn_on(&mut w, u);
    assert!(w.fact(f).is_turned_on);
    assert!(subscribers(&w).iter().any(|s| refers_to(&w, *s, u)), "subscribed before");

    crate::entity::dispose(&mut w, u);
    assert!(w.entity(u).is_disposed);
    assert!(w.fact(f).is_disposed);
    assert!(w.entity(u).facts.list.is_empty());
    assert!(!w.by_id.contains_key("u"));
    let left: Vec<_> = subscribers(&w).into_iter().filter(|s| refers_to(&w, *s, u)).collect();
    assert!(left.is_empty(), "{left:?}");
    // Disposing again does nothing.
    crate::entity::dispose(&mut w, u);
}

/// `UnitEntityData.OnTurnOn` skips the facts' `UpdateIsActive` pass for a preview.
#[test]
fn turning_a_preview_on_leaves_its_deactivated_facts_alone() {
    for preview in [false, true] {
        let (mut w, u) = registry_world();
        let bp = feature(&w, 11, vec![stat_bonus("$AddStatBonus$p", S::Wisdom, 2)]);
        // Enabled but not active, as `SetupPreview` leaves a buff.
        let f = loaded_fact(&mut w, u, &bp, false);
        w.fact_mut(f).kind = crate::world::FactKind::Buff;
        w.fact_mut(f).clr = "Buff";
        crate::facts::lifecycle::manager_restore_owner_link(&mut w, u);
        crate::facts::lifecycle::manager_post_load(&mut w, u);
        if preview {
            w.unit_mut(u).expect("unit").preview_of = Some(u);
        }
        crate::entity::turn_on(&mut w, u);
        assert_eq!(w.fact(f).is_active, !preview, "preview: {preview}");
    }
}

/// `EntityCreationController.Tick` iterates the live `m_ToCreate.Count`: a unit queued by a
/// spawn subscriber is created in the same tick.
struct Spawner {
    next: Cell<Option<EntityIdx>>,
}

impl ExternalHandler for Spawner {
    fn interfaces(&self) -> Vec<String> {
        vec!["IUnitSpawnHandler".into()]
    }
    fn event(&self, w: &mut World, _iface: &str) {
        if let Some(e) = self.next.take() {
            w.player.creation_queue.push((e, false));
        }
    }
    fn rule(&self, _w: &mut World, _rule: &mut dyn RuleEvent, _step: RuleStep) {}
}

#[test]
fn units_queued_by_a_spawn_handler_are_created_in_the_same_tick() {
    let (mut w, a) = registry_world();
    let b = new_unit(&mut w, "b");
    w.bus.external.insert(1, Rc::new(Spawner { next: Cell::new(Some(b)) }));
    crate::rulebook::bus::subscribe(&mut w, Sub::External(1));
    w.player.creation_queue.push((a, false));
    super::entity_creator_tick(&mut w);
    assert_eq!(w.player.area_units, vec![a, b]);
    assert!(w.player.creation_queue.is_empty());
}

// ---- the activatable-abilities controller (review findings) ---------------------------

/// The unit is a party member in the cross-scene state (`PartyAndPets`: awake).
fn in_party(w: &mut World, u: EntityIdx) {
    w.player.cross_scene_units.push(u);
    w.player.party_characters.push(u);
    w.unit_mut(u).expect("unit").companion_state = Some(2);
}

fn toggle_unit(
    w: &mut World,
    g: u128,
    fields: &[(&str, Node)],
    state: (bool, bool),
) -> crate::world::FactIdx {
    let o = obj("Kingmaker.UnitLogic.ActivatableAbilities.BlueprintActivatableAbility", fields);
    w.content.insert(crate::content::Blueprint {
        guid: wr_content::Guid(g),
        name: format!("Toggle{g}"),
        obj: o,
        source: crate::content::Source::Bbp,
    });
    let bp = w.content.get(wr_content::Guid(g)).expect("inserted");
    let mut f =
        crate::world::Fact::blank(crate::world::FactKind::ActivatableAbility, "ActivatableAbility");
    f.blueprint = Some(bp.guid);
    f.bp = Some(bp);
    f.is_active = true;
    f.activatable = Some((state.0, state.1, None));
    let fi = w.add_fact(f);
    let u = EntityIdx(0);
    w.entity_mut(u).facts.list.push(fi);
    w.fact_mut(fi).owner = Some(u);
    fi
}

/// A started toggle that is off stops at the next round (`ActivatableAbility.OnNewRound`),
/// not never: the saved `TimeToNextRound` counts down first.
#[test]
fn a_started_toggle_that_is_off_stops_when_its_round_comes() {
    let (mut w, u) = registry_world();
    in_party(&mut w, u);
    let f = toggle_unit(&mut w, 300, &[], (false, true));
    w.fact_mut(f).time_to_next_round = 3.0;
    super::activatable_abilities_tick(&mut w, false);
    assert_eq!(w.fact(f).activatable.map(|a| a.1), Some(true), "3 s remain (clock delta is 0)");
    w.fact_mut(f).time_to_next_round = 0.0;
    super::activatable_abilities_tick(&mut w, false);
    assert_eq!(w.fact(f).activatable.map(|a| a.1), Some(false));
    assert!((w.fact(f).time_to_next_round - 6.0).abs() < f32::EPSILON, "the timer restarts");
    assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
}

/// `DeactivateAfterFirstRound`: the first round only arms the deactivation.
#[test]
fn deactivate_after_first_round_stops_at_the_second_round() {
    let (mut w, u) = registry_world();
    in_party(&mut w, u);
    let f =
        toggle_unit(&mut w, 301, &[("DeactivateAfterFirstRound", Node::Bool(true))], (true, true));
    super::activatable_abilities_tick(&mut w, false);
    assert_eq!(w.fact(f).activatable.map(|a| a.1), Some(true));
    assert!(w.fact(f).deactivate_next_round);
    w.fact_mut(f).time_to_next_round = 0.0;
    super::activatable_abilities_tick(&mut w, false);
    assert_eq!(w.fact(f).activatable.map(|a| a.1), Some(false));
}

/// `ShouldBeDisabled` applies the Rest mode on its own, with or without the owner-state flags.
#[test]
fn rest_stops_a_started_toggle_unless_it_survives_rest() {
    for (survives, started_after) in [(false, false), (true, true)] {
        let (mut w, u) = registry_world();
        in_party(&mut w, u);
        w.player.mode = "Rest";
        let f = toggle_unit(
            &mut w,
            302 + u128::from(survives),
            &[("DoNotTurnOffOnRest", Node::Bool(survives))],
            (true, true),
        );
        super::activatable_abilities_tick(&mut w, false);
        assert_eq!(w.fact(f).activatable.map(|a| a.1), Some(started_after), "{survives}");
    }
    // An unstarted toggle that is on doesn't start during Rest.
    let (mut w, u) = registry_world();
    in_party(&mut w, u);
    w.player.mode = "Rest";
    let f = toggle_unit(&mut w, 310, &[], (true, false));
    super::activatable_abilities_tick(&mut w, false);
    assert_eq!(w.fact(f).activatable.map(|a| a.1), Some(false));
}

/// An awake area unit's toggles aren't ticked by this port: that is reported, not skipped.
#[test]
fn an_area_unit_with_a_toggle_that_is_on_is_reported() {
    let (mut w, u) = registry_world();
    w.player.area_units.push(u);
    toggle_unit(&mut w, 311, &[], (true, false));
    super::activatable_abilities_tick(&mut w, false);
    assert!(
        w.coverage.unsupported.keys().any(|k| k.contains("area unit")),
        "{:?}",
        w.coverage.unsupported
    );
}

/// Destroying a unit that has dropped loot would hand its inventory over first: reported.
#[test]
fn destroying_a_unit_with_dropped_loot_is_reported() {
    let (mut w, u) = registry_world();
    w.unit_mut(u).expect("unit").has_dropped_loot_ref = true;
    w.player.destroy_marks.push(u);
    super::entity_destroyer_tick(&mut w);
    assert!(
        w.coverage.unsupported.keys().any(|k| k.contains("TransferInventoryToDroppedLoot")),
        "{:?}",
        w.coverage.unsupported
    );
}

/// The load pass ticks the units the first tick keeps awake: an extra sleeps out of the camera,
/// which the sim doesn't know, so its toggle isn't started (reported); a sleepless extra is
/// awake and its toggle starts.
#[test]
fn the_load_pass_leaves_an_extras_toggle_alone_unless_it_is_sleepless() {
    for sleepless in [false, true] {
        let (mut w, u) = registry_world();
        w.player.area_units.push(u);
        w.unit_mut(u).expect("unit").is_extra = true;
        if sleepless {
            let id = w.entity(u).unique_id.clone();
            w.player.counters.insert(format!("unit:{id}.Sleepless"), 1);
        }
        let f = toggle_unit(&mut w, 320 + u128::from(sleepless), &[], (true, false));
        super::after_area_load(&mut w, &[], &[u]);
        assert_eq!(w.fact(f).activatable.map(|a| a.1), Some(sleepless), "{sleepless}");
        let reported = w.coverage.unsupported.keys().any(|k| k.contains("area unit"));
        assert_eq!(reported, !sleepless, "{:?}", w.coverage.unsupported);
    }
}

/// Recorded awake states (`aa_awake`, per oracle tick): a saved unit's id is taken as is; a
/// unit created in the run is matched by its blueprint among live units (a disposed one doesn't
/// count); a tick the recording's controller didn't run is reported.
#[test]
fn recorded_awake_states_match_units_and_fill_unknown_ones() {
    use serde_json::json;
    let (mut w, saved) = registry_world();
    let bp = wr_content::Guid(0xB0B);
    // A disposed unit of the same blueprint: not a candidate.
    let gone = new_unit(&mut w, "1EFF");
    w.unit_mut(gone).expect("unit").blueprint = Some(bp);
    w.entity_mut(gone).is_disposed = true;
    let spawned = new_unit(&mut w, "1F00");
    w.unit_mut(spawned).expect("unit").blueprint = Some(bp);
    w.by_id.insert("1F00".into(), spawned);
    w.by_id.insert("u".into(), saved);
    w.player.area_units.extend([saved, gone, spawned]);
    let records = vec![
        json!({"k": "tick", "t": 0}),
        json!({"k": "obs", "ev": "aa_awake", "set": ["u", "9999"], "new": [["9999", bp.to_string()]]}),
        json!({"k": "tick", "t": 1}),
        json!({"k": "obs", "ev": "aa_awake", "on": [], "off": ["u"], "new": []}),
        json!({"k": "tick", "t": 2}),
    ];
    let saved_ids: std::collections::HashSet<String> = ["u".to_owned()].into();
    w.recorded_awake =
        crate::spawn::RecordedAwake::of(&records, saved_ids).expect("well formed").map(Box::new);
    let now = |w: &World| w.recorded_awake.as_ref().and_then(|r| r.now.clone()).expect("set");
    crate::spawn::recorded_awake_for_tick(&mut w, 0);
    assert!(now(&w).contains(&saved) && now(&w).contains(&spawned) && !now(&w).contains(&gone));
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
    crate::spawn::recorded_awake_for_tick(&mut w, 1);
    assert!(!now(&w).contains(&saved) && now(&w).contains(&spawned));
    // Tick 2: the recording's controller didn't run.
    crate::spawn::recorded_awake_for_tick(&mut w, 2);
    assert!(w.coverage.unsupported.keys().any(|k| k.contains("no recorded awake state")));
}

/// A unit indexed but not yet in the state's pool (queued for creation) can't be matched:
/// the controller wouldn't visit it, so a recording that has it awake is reported.
#[test]
fn a_queued_unit_isnt_matched_to_a_recorded_awake_one() {
    use serde_json::json;
    let (mut w, _) = registry_world();
    let bp = wr_content::Guid(0xB0C);
    let queued = new_unit(&mut w, "1F01");
    w.unit_mut(queued).expect("unit").blueprint = Some(bp);
    w.by_id.insert("1F01".into(), queued);
    let records = vec![
        json!({"k": "tick", "t": 0}),
        json!({"k": "obs", "ev": "aa_awake", "set": ["n"], "new": [["n", bp.to_string()]]}),
    ];
    w.recorded_awake = crate::spawn::RecordedAwake::of(&records, std::collections::HashSet::new())
        .expect("well formed")
        .map(Box::new);
    crate::spawn::recorded_awake_for_tick(&mut w, 0);
    assert!(w.coverage.unsupported.keys().any(|k| k.contains("can't match")));
}

/// A malformed `aa_awake` record isn't taken as an input: units, changes and new units must
/// be well formed, changes need a whole set first, one record per tick.
#[test]
fn malformed_awake_records_are_rejected() {
    use serde_json::json;
    let of = |recs: Vec<serde_json::Value>| {
        crate::spawn::RecordedAwake::of(&recs, std::collections::HashSet::new())
    };
    let tick = |t: i64| json!({"k": "tick", "t": t});
    assert!(
        of(vec![tick(0), json!({"k": "obs", "ev": "aa_awake", "set": "u", "new": []})]).is_err()
    );
    assert!(
        of(vec![tick(0), json!({"k": "obs", "ev": "aa_awake", "on": [], "off": [], "new": []})])
            .is_err()
    );
    assert!(
        of(vec![tick(0), json!({"k": "obs", "ev": "aa_awake", "set": ["u"], "new": [["x", 1]]})])
            .is_err()
    );
    assert!(of(vec![json!({"k": "obs", "ev": "aa_awake", "set": [], "new": []})]).is_err());
    let set = json!({"k": "obs", "ev": "aa_awake", "set": ["u"], "new": []});
    assert!(of(vec![tick(0), set.clone(), set.clone()]).is_err(), "two in one tick");
    let off = json!({"k": "obs", "ev": "aa_awake", "on": [], "off": ["v"], "new": []});
    assert!(of(vec![tick(0), set.clone(), tick(1), off]).is_err(), "off but not awake");
    let twice = json!({"k": "obs", "ev": "aa_awake", "on": [], "off": [], "new": [["n", wr_content::Guid(1).to_string()]]});
    let first = json!({"k": "obs", "ev": "aa_awake", "set": ["u"], "new": [["n", wr_content::Guid(2).to_string()]]});
    assert!(of(vec![tick(0), first, tick(1), twice]).is_err(), "a new unit announced twice");
    assert!(matches!(of(vec![tick(0)]), Ok(None)), "no records: no input");
    assert!(matches!(of(vec![tick(0), set]), Ok(Some(_))));
}

/// An `ActivatableAbility` subscribes to the handler interfaces it declares (the save schema
/// has none of them): its event and initiator rule lists.
#[test]
fn a_toggle_subscribes_to_its_handler_interfaces() {
    let (mut w, u) = registry_world();
    let f = toggle_unit(&mut w, 320, &[], (true, true));
    crate::rulebook::bus::subscribe_fact(&mut w, f);
    for iface in ["IUnitBuffHandler", "IUnitCombatHandler", "IUnitRunCommandHandler"] {
        assert!(w.bus.global[iface].list.contains(&Some(Sub::Fact(f))), "{iface}");
    }
    for rule in ["RuleAttackWithWeapon", "RuleDealDamage", "RuleAttackWithWeaponResolve"] {
        assert!(w.bus.rule_initiator[&u][rule].list.contains(&Some(Sub::Fact(f))), "{rule}");
    }
    crate::rulebook::bus::unsubscribe_fact(&mut w, f);
    assert!(!subscribers(&w).contains(&Sub::Fact(f)));
}

fn unit_command(ty: i64) -> Node {
    Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.ActivatableAbilities.ActivatableAbilityUnitCommand",
        &[("Type", Node::Enum(ty))],
    )))
}

/// `HandleUnitRunCommand`: its owner's command of its unit-command type turns the toggle off;
/// a started one that doesn't deactivate immediately stops at its round (`OnDidTurnOff`).
#[test]
fn a_command_of_its_type_turns_a_toggle_off() {
    use crate::rulebook::bus::{CommandArg, raise_global_command};
    let arg = |executor, ty| CommandArg {
        executor: Some(executor),
        ty,
        activate_ability: false,
        result: 0,
        acted: false,
        finished: false,
    };
    let (mut w, u) = registry_world();
    let other = new_unit(&mut w, "other");
    let f =
        toggle_unit(&mut w, 321, &[("Components", Node::Seq(vec![unit_command(1)]))], (true, true));
    w.fact_mut(f).ready_to_start = true;
    crate::rulebook::bus::subscribe_fact(&mut w, f);
    // A move command, and another unit's standard one: nothing.
    raise_global_command(&mut w, "IUnitRunCommandHandler", "HandleUnitRunCommand", arg(u, 3));
    raise_global_command(&mut w, "IUnitRunCommandHandler", "HandleUnitRunCommand", arg(other, 1));
    assert_eq!(w.fact(f).activatable.map(|a| (a.0, a.1)), Some((true, true)));
    raise_global_command(&mut w, "IUnitRunCommandHandler", "HandleUnitRunCommand", arg(u, 1));
    assert_eq!(w.fact(f).activatable.map(|a| (a.0, a.1)), Some((false, true)));
    assert!(!w.fact(f).ready_to_start);
    assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
}

/// `HandleUnitJoinCombat`: a toggle that activates on combat start, on and not started, starts
/// when its owner joins combat (not another unit).
#[test]
fn joining_combat_starts_a_toggle_that_activates_on_combat_start() {
    use crate::rulebook::bus::raise_global_call;
    let (mut w, u) = registry_world();
    let other = new_unit(&mut w, "other");
    let f =
        toggle_unit(&mut w, 322, &[("DeactivateIfCombatEnded", Node::Bool(true))], (true, false));
    crate::rulebook::bus::subscribe_fact(&mut w, f);
    w.unit_mut(u).expect("unit").combat.in_combat = true;
    raise_global_call(&mut w, "IUnitCombatHandler", "HandleUnitJoinCombat", Some(other));
    assert_eq!(w.fact(f).activatable.map(|a| a.1), Some(false));
    raise_global_call(&mut w, "IUnitCombatHandler", "HandleUnitJoinCombat", Some(u));
    assert_eq!(w.fact(f).activatable.map(|a| a.1), Some(true));
    assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
}

/// `OnEventDidTrigger(RuleAttackWithWeaponResolve)`: a ranged attack's resolve is the toggle's
/// `OnAttack`, which stops a started toggle that isn't active and deactivates immediately; a
/// melee one's isn't.
#[test]
fn a_ranged_resolve_stops_an_inactive_toggle_that_deactivates_immediately() {
    use crate::combat::attack::RuleAttackWithWeaponResolve;
    for (melee, started_after) in [(true, true), (false, false)] {
        let (mut w, u) = registry_world();
        let f =
            toggle_unit(&mut w, 323, &[("DeactivateImmediately", Node::Bool(true))], (true, true));
        w.fact_mut(f).is_active = false;
        let mut r = RuleAttackWithWeaponResolve {
            base: crate::rulebook::RuleBase::default(),
            initiator: u,
            target: u,
            damage: None,
            attack_hit: false,
            weapon_melee: melee,
            source_id: None,
        };
        assert!(crate::unit_facts::activatable_rule(&mut w, f, &mut r, RuleStep::AboutToTrigger));
        assert_eq!(w.fact(f).activatable.map(|a| a.1), Some(true));
        assert!(crate::unit_facts::activatable_rule(&mut w, f, &mut r, RuleStep::DidTrigger));
        assert_eq!(w.fact(f).activatable.map(|a| a.1), Some(started_after), "melee {melee}");
        assert!(w.coverage.unsupported.is_empty(), "{:?}", w.coverage.unsupported);
    }
}

/// `HandleUnitCommandDidAct`: a toggle activated with a unit command, on and not started, turns
/// off when its owner acts with a command that doesn't activate it; a `UnitActivateAbility`
/// command (not run by the sim) reports instead of acting.
#[test]
fn acting_with_another_command_turns_a_with_command_toggle_off() {
    use crate::rulebook::bus::{CommandArg, raise_global_command};
    for (activate_ability, on_after, reported) in [(false, false, false), (true, true, true)] {
        let (mut w, u) = registry_world();
        let f = toggle_unit(&mut w, 324, &[("ActivationType", Node::Enum(1))], (true, false));
        crate::rulebook::bus::subscribe_fact(&mut w, f);
        let arg = CommandArg {
            executor: Some(u),
            ty: 1,
            activate_ability,
            result: 3,
            acted: true,
            finished: false,
        };
        raise_global_command(&mut w, "IUnitCommandActHandler", "HandleUnitCommandDidAct", arg);
        assert_eq!(w.fact(f).activatable.map(|a| a.0), Some(on_after), "{activate_ability}");
        assert_eq!(!w.coverage.unsupported.is_empty(), reported, "{:?}", w.coverage.unsupported);
    }
}
