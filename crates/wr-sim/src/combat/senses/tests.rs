use std::rc::Rc;

use wr_content::Guid;
use wr_content::node::Node;

use super::*;
use crate::content::{Blueprint, Content, Source};
use crate::facts::tests::obj;
use crate::registry::Registry;
use crate::stats::value::Stats;
use crate::unit::Unit;
use crate::world::{Entity, EntityKind, FactsManager};

fn world() -> World {
    let mut w = World::new(Rc::new(Content::empty()), Rc::new(Registry::default()));
    w.combat.groups.on = true;
    w
}

fn add_unit(w: &mut World, id: &str) -> EntityIdx {
    let unit = Unit { stats: Stats::fresh(), ..Unit::default() };
    let e = w.add_entity(Entity {
        unique_id: id.into(),
        clr: "UnitEntityData",
        kind: EntityKind::Unit(Box::new(unit)),
        facts: FactsManager::default(),
        parts: Vec::new(),
        is_turned_on: false,
        post_load_executed: true,
        is_in_game: true,
        subscribed: false,
        disposing: false,
        is_disposed: false,
    });
    w.player.area_units.push(e);
    e
}

/// A unit in the live group `id` (its cached group, as `Group` leaves it).
fn add_to_group(w: &mut World, u: EntityIdx, id: &str) -> GroupIdx {
    let g = groups::get_group(w, id);
    w.combat.groups.get_mut(g).units.push(u);
    w.combat.groups.cached.insert(u, g);
    g
}

fn faction(w: &mut World, u: EntityIdx, guid: u128, peaceful: bool) {
    let o = obj("Kingmaker.Blueprints.BlueprintFaction", &[("Peaceful", Node::Bool(peaceful))]);
    w.content.insert(Blueprint { guid: Guid(guid), name: "F".into(), obj: o, source: Source::Bbp });
    w.unit_mut(u).expect("unit").faction = Some(Guid(guid));
}

#[test]
fn the_areas_peaceful_flag_is_its_etude_zones_count() {
    let mut w = world();
    assert!(!area_peaceful(&w));
    w.player.counters.insert("area.Peaceful".into(), 2);
    assert!(area_peaceful(&w));
    w.player.counters.insert("area.Peaceful".into(), 0);
    assert!(!area_peaceful(&w), "released to zero");
}

#[test]
fn passive_is_the_units_flag_and_a_pets_is_its_masters() {
    let mut w = world();
    let (m, p, other) = (add_unit(&mut w, "m"), add_unit(&mut w, "p"), add_unit(&mut w, "o"));
    assert!(!passive(&w, m));
    w.player.counters.insert("unit:m.Passive".into(), 1);
    assert!(passive(&w, m));
    assert!(!passive(&w, other), "another unit's flag is its own");
    // A pet without the part, or without its master, keeps its own flag.
    assert!(!passive(&w, p));
    w.player.counters.insert("unit:p.Passive".into(), 1);
    assert!(passive(&w, p));
    // With the part and its master, it mirrors the master (its own sources are reverted).
    {
        let x = w.unit_mut(p).expect("unit");
        x.pet_type = Some(0);
        x.master = Some(m);
    }
    assert!(passive(&w, p));
    w.player.counters.insert("unit:m.Passive".into(), 0);
    assert!(!passive(&w, p), "the master's release releases the pet's");
}

#[test]
fn stealth_hides_a_unit_from_every_group_that_has_not_spotted_it() {
    let mut w = world();
    let (t, s, o) = (add_unit(&mut w, "t"), add_unit(&mut w, "s"), add_unit(&mut w, "o"));
    let (gs, go) = (add_to_group(&mut w, s, "A"), add_to_group(&mut w, o, "B"));
    add_to_group(&mut w, t, "T");
    assert!(!in_stealth_for(&mut w, t, gs), "not in stealth");
    w.unit_mut(t).expect("unit").in_stealth = true;
    assert!(in_stealth_for(&mut w, t, gs), "nobody spotted it");
    w.unit_mut(t).expect("unit").stealth_spotted_by = vec!["s".into(), "gone".into()];
    assert!(!in_stealth_for(&mut w, t, gs), "spotted by a member of the group");
    assert!(in_stealth_for(&mut w, t, go), "another group hasn't");
    w.unit_mut(s).expect("unit").life_state = 1;
    assert!(in_stealth_for(&mut w, t, gs), "an unconscious spotter doesn't count");
    w.unit_mut(s).expect("unit").life_state = 0;
    w.entity_mut(s).is_disposed = true;
    assert!(in_stealth_for(&mut w, t, gs), "a spotter that no longer resolves doesn't count");
}

#[test]
fn an_invisible_target_is_seen_only_by_members_that_could() {
    let mut w = world();
    let (a, b) = (add_unit(&mut w, "a"), add_unit(&mut w, "b"));
    assert_eq!(invisible_target_seen(&w, &[a, b]), Some(false));
    assert_eq!(invisible_target_seen(&w, &[]), Some(false), "an empty group");
    w.unit_mut(b).expect("unit").state.conditions[usize::try_from(TRUE_SEEING).unwrap()] = 1;
    assert!(sees_invisibility(&w, b) && !sees_invisibility(&w, a));
    assert_eq!(invisible_target_seen(&w, &[a, b]), None, "true seeing needs line of sight");
    w.unit_mut(b).expect("unit").state.conditions[usize::try_from(TRUE_SEEING).unwrap()] = 0;
    w.unit_mut(a).expect("unit").state.conditions[usize::try_from(SEE_INVISIBILITY).unwrap()] = 1;
    assert_eq!(invisible_target_seen(&w, &[a, b]), None, "see invisibility too");
    // Immunity to the condition (it isn't "had").
    w.unit_mut(a).expect("unit").state.immunities[usize::try_from(SEE_INVISIBILITY).unwrap()] = 1;
    assert_eq!(invisible_target_seen(&w, &[a, b]), Some(false));
    w.unit_mut(b).expect("unit").has_blindsense = true;
    assert_eq!(invisible_target_seen(&w, &[a, b]), None, "blindsense needs its range and sight");
}

#[test]
fn a_group_is_ready_when_a_member_is_awake_unpeaceful_and_out_of_fog_or_in_combat() {
    let mut w = world();
    let (a, b) = (add_unit(&mut w, "a"), add_unit(&mut w, "b"));
    let g = add_to_group(&mut w, a, "A");
    w.combat.groups.get_mut(g).units.push(b);
    w.combat.groups.cached.insert(b, g);
    faction(&mut w, a, 1, false);
    faction(&mut w, b, 2, true);
    // Asleep: not ready, for certain.
    assert_eq!(ready_for_combat(&w, g, &[]), Tri::No);
    // Awake, out of combat, not of the party: fog of war decides (not modelled).
    w.combat.awake = vec![a, b];
    assert_eq!(ready_for_combat(&w, g, &[]), Tri::Unknown);
    // In combat: ready whatever the fog; the peaceful member doesn't count alone.
    w.unit_mut(a).expect("unit").combat.in_combat = true;
    assert_eq!(ready_for_combat(&w, g, &[]), Tri::Yes);
    w.unit_mut(a).expect("unit").combat.in_combat = false;
    // A party member is never in fog of war.
    assert_eq!(ready_for_combat(&w, g, &[a]), Tri::Yes);
    assert_eq!(ready_for_combat(&w, g, &[b]), Tri::Unknown, "a peaceful party member is no use");
    // Only the peaceful one awake: certainly not ready.
    w.combat.awake = vec![b];
    assert_eq!(ready_for_combat(&w, g, &[b]), Tri::No);
    // A member whose awake state is unknown keeps the answer open.
    w.combat.awake_unknown = vec![a];
    assert_eq!(ready_for_combat(&w, g, &[]), Tri::Unknown);
    // …unless another member is ready.
    w.combat.awake = vec![b];
    w.unit_mut(b).expect("unit").combat.in_combat = true;
    faction(&mut w, b, 3, false);
    assert_eq!(ready_for_combat(&w, g, &[]), Tri::Yes);
    // A faction the sim doesn't know is unknown, not peaceful.
    w.unit_mut(b).expect("unit").faction = None;
    assert_eq!(ready_for_combat(&w, g, &[]), Tri::Unknown);
}
