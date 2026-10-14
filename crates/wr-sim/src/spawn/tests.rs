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
