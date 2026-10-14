//! Synthetic tests of the scene, spawner and cutscene ports.

use std::rc::Rc;

use wr_content::Guid;
use wr_content::node::Node;

use crate::content::Blueprint;
use crate::facts::lifecycle as fl;
use crate::facts::tests::{feature, loaded_fact, obj, stat_bonus};
use crate::scene::{
    SceneCache, SceneState, Shape, ZoneInfo, params_same, summon_pool_register, tick_unit_inside,
};
use crate::stats::types::StatType as S;
use crate::world::{Entity, EntityIdx, EntityKind, FactKind, FactsManager, World};

fn world() -> (World, EntityIdx) {
    let (mut w, u) = crate::stats::tests::world_with_unit();
    w.registry = Rc::new(crate::components::registry());
    w.events.on = true;
    w.player.mode = "Default";
    (w, u)
}

fn identity() -> [[f64; 4]; 3] {
    [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]]
}

fn cylinder(center: [f64; 3], height: f64, radius: f64) -> Shape {
    Shape {
        kind: "ScriptZoneCylinder".into(),
        world_to_local: identity(),
        center,
        height,
        radius,
        bounds_center: [0.0; 3],
        bounds_extent: [0.0; 3],
    }
}

fn bx(center: [f64; 3], extent: [f64; 3]) -> Shape {
    Shape {
        kind: "ScriptZoneBox".into(),
        world_to_local: identity(),
        center: [0.0; 3],
        height: 0.0,
        radius: 0.0,
        bounds_center: center,
        bounds_extent: extent,
    }
}

/// `ScriptZoneCylinder.Contains`: the height test uses the full `Height` as a half-height,
/// the radius grows with corpulence.
#[test]
fn cylinder_contains_follows_the_game() {
    let c = cylinder([0.0; 3], 3.0, 10.0);
    assert_eq!(c.contains([0.0, 2.9, 0.0], 0.0), Some(true));
    assert_eq!(c.contains([0.0, 3.1, 0.0], 0.0), Some(false), "|dy| > Height");
    assert_eq!(c.contains([-2.9, -2.9, 0.0], 0.0), Some(true));
    assert_eq!(c.contains([10.0, 0.0, 0.0], 0.0), Some(true), "on the radius");
    assert_eq!(c.contains([10.2, 0.0, 0.0], 0.0), Some(false));
    assert_eq!(c.contains([10.2, 0.0, 0.0], 0.5), Some(true), "corpulence widens the radius");
    // The 2D distance ignores y.
    assert_eq!(c.contains([0.0, 3.0, 10.0], 0.0), Some(true));
    assert_eq!(cylinder([0.0; 3], 3.0, 1.0).contains([1.0, 0.0, 1.0], 0.0), Some(false));
}

/// `ScriptZoneBox.Contains`: `Bounds.Contains` is inclusive; corpulence adds a squared
/// distance test against the unsquared corpulence, after a y test against the box's local centre.
#[test]
fn box_contains_keeps_the_games_quirks() {
    let b = bx([0.0; 3], [1.0, 1.0, 1.0]);
    assert_eq!(b.contains([1.0, 1.0, 1.0], 0.0), Some(true), "inclusive");
    assert_eq!(b.contains([1.1, 0.0, 0.0], 0.0), Some(false));
    // 0.3 outside: squared distance 0.09 <= corpulence 0.3.
    assert_eq!(b.contains([1.3, 0.0, 0.0], 0.3), Some(true));
    // 0.6 outside: 0.36 > 0.3.
    assert_eq!(b.contains([1.6, 0.0, 0.0], 0.3), Some(false));
    // The y test: |centre.y - point.y| > extent.y.
    assert_eq!(b.contains([1.1, 1.5, 0.0], 0.5), Some(false));
    let polygon = Shape { kind: "ScriptZonePolygon".into(), ..cylinder([0.0; 3], 1.0, 1.0) };
    assert_eq!(polygon.contains([0.0; 3], 0.0), None, "no port: reported by the caller");
}

/// A rotated and moved shape: the inverse of the scene's `localToWorldMatrix`.
#[test]
fn shapes_test_in_their_local_space() {
    // Rotated 90 degrees about y and moved to (100, 0, 50): local x is world -z.
    let l2w = [
        [0.0, 0.0, 1.0, 100.0],
        [0.0, 1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0, 50.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let inv = crate::scene::invert_affine_for_tests(&l2w).expect("invertible");
    let b = Shape { world_to_local: inv, ..bx([0.0; 3], [5.0, 1.0, 1.0]) };
    // World (100, 0, 46) is local x = +4 (inside), world (104, 0, 50) is local z = +4 (outside).
    assert_eq!(b.contains([100.0, 0.0, 46.0], 0.0), Some(true));
    assert_eq!(b.contains([104.0, 0.0, 50.0], 0.0), Some(false));
}

#[test]
fn identical_parameters_and_the_empty_set() {
    let a = vec![("Unit".to_owned(), "5B0F".to_owned())];
    let b = vec![("Unit".to_owned(), "5B48".to_owned())];
    assert!(params_same(&a, &a));
    assert!(!params_same(&a, &b), "different unit");
    // `IsTheSame` walks this side's parameters: an empty set is the same as anything.
    assert!(params_same(&[], &b));
    assert!(!params_same(&a, &[]));
}

fn zone_world(once_only: bool, players_only: bool) -> (World, EntityIdx, usize) {
    let (mut w, u) = world();
    let ent = w.add_entity(Entity {
        unique_id: "zone".into(),
        clr: "ScriptZoneData",
        kind: EntityKind::Other,
        facts: FactsManager::default(),
        parts: Vec::new(),
        is_turned_on: true,
        post_load_executed: true,
        is_in_game: true,
        subscribed: false,
        disposing: false,
        is_disposed: false,
    });
    w.scene = SceneState::default();
    w.scene.zones.push(crate::scene::Zone {
        entity: ent,
        id: "zone".into(),
        blueprint: None,
        is_active: true,
        inside: Vec::new(),
    });
    w.scene.cache = Some(Rc::new(SceneCache {
        schema: crate::scene::SCHEMA.into(),
        area: "a".into(),
        name: "a".into(),
        source: String::new(),
        zones: vec![ZoneInfo {
            unique_id: "zone".into(),
            blueprint: None,
            once_only,
            players_only,
            start_inactive: false,
            unit_entered_calls: 0,
            unit_exited_calls: 0,
            shapes: Vec::new(),
        }],
        views: Vec::new(),
    }));
    (w, u, 0)
}

/// `ScriptZone.TryAddUnit`: the unit joins `InsideUnits`, a once-only zone switches itself
/// off, and a unit already inside doesn't enter again.
#[test]
fn a_once_only_zone_fires_once() {
    let (mut w, u, z) = zone_world(true, false);
    tick_unit_inside(&mut w, z, u);
    assert!(!w.scene.zones[z].is_active, "once-only: IsActive = false");
    assert_eq!(w.scene.zones[z].inside.len(), 1);
    assert_eq!(w.events.records.iter().filter(|r| r.k == "t1").count(), 1);
    // An inactive zone ignores the unit.
    tick_unit_inside(&mut w, z, u);
    assert_eq!(w.scene.zones[z].inside.len(), 1);
}

#[test]
fn a_repeating_zone_counts_a_unit_once() {
    let (mut w, u, z) = zone_world(false, false);
    tick_unit_inside(&mut w, z, u);
    assert!(w.scene.zones[z].is_active);
    // `InsideThisTick` is already true (the zone ticks it off at the start of its own tick).
    tick_unit_inside(&mut w, z, u);
    assert_eq!(w.scene.zones[z].inside.len(), 1);
    assert_eq!(w.events.records.iter().filter(|r| r.k == "t1").count(), 1);
}

/// `IsInterestedInUnit`: a players-only zone ignores other factions and the dead.
#[test]
fn a_players_only_zone_skips_other_units() {
    let (mut w, u, z) = zone_world(false, true);
    tick_unit_inside(&mut w, z, u);
    assert!(w.scene.zones[z].inside.is_empty(), "not a player-faction unit");
    w.unit_mut(u).expect("unit").is_player_faction = true;
    tick_unit_inside(&mut w, z, u);
    assert_eq!(w.scene.zones[z].inside.len(), 1);
    // A dead unit isn't interesting either.
    let (mut w, u, z) = zone_world(false, false);
    w.unit_mut(u).expect("unit").life_state = 2;
    tick_unit_inside(&mut w, z, u);
    assert!(w.scene.zones[z].inside.is_empty());
}

/// Without scene settings the zone can't be evaluated: reported.
#[test]
fn a_zone_without_scene_settings_is_reported() {
    let (mut w, u, z) = zone_world(false, false);
    w.scene.cache = None;
    tick_unit_inside(&mut w, z, u);
    assert!(!w.coverage.valid());
}

/// `SummonPool.Register`: once per unit, the unit's `PooledPart` lists the pool.
#[test]
fn summon_pool_registration_is_idempotent() {
    let (mut w, u) = world();
    summon_pool_register(&mut w, Guid(7), u);
    summon_pool_register(&mut w, Guid(7), u);
    summon_pool_register(&mut w, Guid(8), u);
    assert_eq!(w.scene.summon_pools.len(), 2);
    assert_eq!(w.scene.summon_pools[0].1, vec![u]);
    assert_eq!(w.scene.pooled[&u], vec![Guid(7), Guid(8)]);
    assert_eq!(w.entity(u).parts.iter().filter(|p| p.clr == "PooledPart").count(), 1);
}

/// `ActivateTrigger`: conditions outside the fact scope, actions inside, `m_Once` with
/// `AlreadyTriggered`.
#[test]
fn activate_trigger_runs_its_actions_once() {
    let (mut w, u) = world();
    let flag: u128 = 0xAC71;
    let unlock = Node::Object(Box::new(obj(
        "Kingmaker.Designers.EventConditionActionSystem.Actions.UnlockFlag",
        &[("m_flag", Node::BpRef(Guid(flag))), ("flagValue", Node::Int(3))],
    )));
    let trigger = Node::Object(Box::new(obj(
        "Kingmaker.Designers.EventConditionActionSystem.Events.ActivateTrigger",
        &[
            ("name", Node::Str("$ActivateTrigger$t".into())),
            ("m_Once", Node::Bool(true)),
            (
                "Actions",
                Node::Object(Box::new(obj(
                    "Kingmaker.ElementsSystem.ActionList",
                    &[("Actions", Node::Seq(vec![unlock]))],
                ))),
            ),
        ],
    )));
    let bp = feature(&w, 0xAC, vec![trigger]);
    let f = loaded_fact(&mut w, u, &bp, false);
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    fl::activate(&mut w, f);
    assert_eq!(w.player.flags.get(&Guid(flag)).copied(), Some(3));
    assert_eq!(
        w.component(crate::world::CompRef { fact: f, comp: 0 }).data,
        Some(serde_json::json!({ "AlreadyTriggered": true }))
    );
    // Active again (a fresh activation): `m_Once` and `AlreadyTriggered` stop it.
    w.player.flags.clear();
    fl::deactivate(&mut w, f);
    fl::activate(&mut w, f);
    assert!(w.player.flags.is_empty(), "already triggered");
}

/// `UnitEntityData.OnIsInGameChanged`: the buffs, activatable abilities and abilities are
/// de- and re-activated with the unit.
#[test]
fn hiding_a_unit_deactivates_its_buffs() {
    let (mut w, u) = world();
    let bp = feature(&w, 0xB1, vec![stat_bonus("$AddStatBonus$b", S::Wisdom, 4)]);
    let f = loaded_fact(&mut w, u, &bp, true);
    w.fact_mut(f).kind = FactKind::Buff;
    w.fact_mut(f).clr = "Buff";
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    crate::entity::turn_on(&mut w, u);
    let base = w.stats(u).v(S::Wisdom).modified_value;
    assert!(w.fact(f).is_active);
    crate::entity::set_is_in_game(&mut w, u, false);
    assert!(!w.entity(u).is_in_game);
    assert!(!w.fact(f).is_active);
    assert_eq!(w.stats(u).v(S::Wisdom).modified_value, base - 4);
    // Unchanged: nothing happens.
    crate::entity::set_is_in_game(&mut w, u, false);
    crate::entity::set_is_in_game(&mut w, u, true);
    assert!(w.fact(f).is_active);
    assert_eq!(w.stats(u).v(S::Wisdom).modified_value, base);
}

fn insert(w: &World, g: u128, ty: &str, set: &[(&str, Node)]) -> Rc<Blueprint> {
    w.content.insert(Blueprint {
        guid: Guid(g),
        name: format!("B{g}"),
        obj: obj(ty, set),
        source: crate::content::Source::Bbp,
    });
    w.content.get(Guid(g)).expect("inserted")
}

fn command_action(w: &World, g: u128, actions: Vec<Node>) -> Guid {
    let list = Node::Object(Box::new(obj(
        "Kingmaker.ElementsSystem.ActionList",
        &[("Actions", Node::Seq(actions))],
    )));
    insert(w, g, "Kingmaker.AreaLogic.Cutscenes.CommandAction", &[("Action", list)]);
    Guid(g)
}

fn action(ty: &str) -> Node {
    Node::Object(Box::new(obj(
        &format!("Kingmaker.Designers.EventConditionActionSystem.Actions.{ty}"),
        &[],
    )))
}

fn track(commands: &[Guid], end: Option<Guid>) -> Node {
    Node::Object(Box::new(obj(
        "Kingmaker.AreaLogic.Cutscenes.Track",
        &[
            ("m_Commands", Node::Seq(commands.iter().map(|g| Node::BpRef(*g)).collect())),
            ("m_EndGate", end.map_or(Node::Null, Node::BpRef)),
        ],
    )))
}

/// A cutscene's T0 actions come in the order it reaches them: a track's commands in order,
/// then the end gate's. Parallel tracks with T0 actions are timing-dependent: reported.
#[test]
fn cutscene_t0_actions_follow_the_tracks_and_gates() {
    let (w, _) = world();
    let hide = command_action(&w, 0xC1, vec![action("HideUnit")]);
    let spawn = command_action(&w, 0xC2, vec![action("Spawn")]);
    insert(
        &w,
        0x6A,
        "Kingmaker.AreaLogic.Cutscenes.Gate",
        &[("m_Tracks", Node::Seq(vec![track(&[spawn], None)]))],
    );
    insert(
        &w,
        0xC0,
        "Kingmaker.AreaLogic.Cutscenes.Cutscene",
        &[("m_Tracks", Node::Seq(vec![track(&[hide], Some(Guid(0x6A)))]))],
    );
    let cls = crate::elements::cutscene::classify(&w, Guid(0xC0));
    let names: Vec<_> = cls
        .t0_steps
        .iter()
        .map(|s| {
            crate::content::View(
                s.command.view().obj("Action").expect("list").objs("Actions")[s.action],
            )
            .type_name()
        })
        .collect();
    assert_eq!(names, vec!["HideUnit", "Spawn"]);
    assert!(cls.unsupported.is_empty(), "{:?}", cls.unsupported);

    insert(
        &w,
        0xC3,
        "Kingmaker.AreaLogic.Cutscenes.Cutscene",
        &[("m_Tracks", Node::Seq(vec![track(&[hide], None), track(&[spawn], None)]))],
    );
    let cls = crate::elements::cutscene::classify(&w, Guid(0xC3));
    assert!(cls.unsupported.iter().any(|u| u.contains("parallel")), "{:?}", cls.unsupported);
}

fn step_names(cls: &crate::elements::cutscene::Classified) -> Vec<&'static str> {
    cls.t0_steps
        .iter()
        .map(|s| {
            crate::content::View(
                s.command.view().obj("Action").expect("list").objs("Actions")[s.action],
            )
            .type_name()
        })
        .collect()
}

fn gate(w: &World, g: u128, ty: &str, tracks: Vec<Node>, extra: &[(&str, Node)]) -> Guid {
    let mut set = vec![("m_Tracks", Node::Seq(tracks))];
    set.extend(extra.iter().cloned());
    insert(w, g, &format!("Kingmaker.AreaLogic.Cutscenes.{ty}"), &set);
    Guid(g)
}

/// The T0 actions follow the gates, not the walk: a track ends in a gate that the root's
/// other track also ends in, so that gate's actions come after the first track's, whichever
/// gate the walk meets first.
#[test]
fn cutscene_t0_actions_follow_the_chain_of_gates() {
    let (w, _) = world();
    let hide = command_action(&w, 0xD1, vec![action("HideUnit")]);
    let spawn = command_action(&w, 0xD2, vec![action("Spawn")]);
    let x = gate(&w, 0xD3, "Gate", vec![track(&[spawn], None)], &[]);
    let y = gate(&w, 0xD4, "Gate", vec![track(&[hide], Some(x))], &[]);
    gate(&w, 0xD5, "Cutscene", vec![track(&[], Some(x)), track(&[], Some(y))], &[]);
    let cls = crate::elements::cutscene::classify(&w, Guid(0xD5));
    assert!(cls.unsupported.is_empty(), "{:?}", cls.unsupported);
    assert_eq!(step_names(&cls), vec!["HideUnit", "Spawn"], "Y's track finishes before X starts");
}

/// What the classification can't order or evaluate is reported: a command's entry condition,
/// a random-track gate (always: it draws), a T0 action on a continuous track, an `Or` gate, a
/// gate reactivation, and a fade's in-fade T0 action.
#[test]
fn cutscene_paths_that_depend_on_state_or_timing_are_reported() {
    let (w, _) = world();
    let spawn = command_action(&w, 0xE1, vec![action("Spawn")]);
    let classify = |g: Guid| crate::elements::cutscene::classify(&w, g).unsupported;
    // A conditional command.
    let cond = Node::Object(Box::new(obj(
        "Kingmaker.ElementsSystem.ConditionsChecker",
        &[(
            "Conditions",
            Node::Seq(vec![Node::Object(Box::new(obj(
                "Kingmaker.Designers.EventConditionActionSystem.Conditions.FlagUnlocked",
                &[],
            )))]),
        )],
    )));
    insert(&w, 0xE2, "Kingmaker.AreaLogic.Cutscenes.CommandAction", &[("EntryCondition", cond)]);
    let c = gate(&w, 0xE3, "Cutscene", vec![track(&[Guid(0xE2)], None)], &[]);
    assert!(classify(c).iter().any(|u| u.contains("EntryCondition")));
    // A random-track gate with nothing T0 in it.
    let c =
        gate(&w, 0xE4, "Cutscene", vec![track(&[], None)], &[("m_ActivationMode", Node::Enum(2))]);
    assert!(classify(c).iter().any(|u| u.contains("RandomTrack")));
    // T0 before a continuous command on the same track.
    insert(&w, 0xE5, "Kingmaker.AreaLogic.Cutscenes.Commands.CommandLockControls", &[]);
    let c = gate(&w, 0xE6, "Cutscene", vec![track(&[spawn, Guid(0xE5)], None)], &[]);
    assert!(classify(c).iter().any(|u| u.contains("continuous")));
    // An `Or` gate, only when there is something T0.
    let c = gate(&w, 0xE7, "Cutscene", vec![track(&[spawn], None)], &[("m_Op", Node::Enum(1))]);
    assert!(classify(c).iter().any(|u| u.contains("Gate Or")));
    let c = gate(&w, 0xE8, "Cutscene", vec![track(&[], None)], &[("m_Op", Node::Enum(1))]);
    assert!(classify(c).is_empty());
    // A reactivated gate reruns its tracks.
    let signal = Node::Object(Box::new(obj(
        "Kingmaker.AreaLogic.Cutscenes.CommandBase.CommandSignalData",
        &[("m_Gate", Node::BpRef(Guid(0xEA)))],
    )));
    insert(
        &w,
        0xE9,
        "Kingmaker.AreaLogic.Cutscenes.CommandActivateGate",
        &[("SignalData", signal)],
    );
    let c = gate(&w, 0xEA, "Cutscene", vec![track(&[spawn, Guid(0xE9)], None)], &[]);
    assert!(classify(c).iter().any(|u| u.contains("CommandActivateGate")));
    // A fade's in-fade actions run at half its lifetime.
    let list = Node::Object(Box::new(obj(
        "Kingmaker.ElementsSystem.ActionList",
        &[("Actions", Node::Seq(vec![action("Spawn")]))],
    )));
    insert(
        &w,
        0xEB,
        "Kingmaker.AreaLogic.Cutscenes.Commands.CutsceneCommandFadeout",
        &[("m_InFadeActions", list)],
    );
    let c = gate(&w, 0xEC, "Cutscene", vec![track(&[Guid(0xEB)], None)], &[]);
    assert!(classify(c).iter().any(|u| u.contains("in-fade action Spawn")));
}

/// `CommandStartDialog` blocks its track until the dialogue ends, so a `Spawn` after it waits
/// for the dialogue: the sim doesn't start a cutscene's dialogue, so the cutscene is reported
/// and its T0 actions aren't queued ahead of it.
#[test]
fn a_cutscene_dialogue_holds_back_the_actions_after_it() {
    let (mut w, _) = world();
    let spawn = command_action(&w, 0xF1, vec![action("Spawn")]);
    insert(
        &w,
        0xF2,
        "Kingmaker.AreaLogic.Cutscenes.Commands.CommandStartDialog",
        &[("m_Dialog", Node::BpRef(Guid(0xF3)))],
    );
    let c = gate(&w, 0xF4, "Cutscene", vec![track(&[Guid(0xF2), spawn], None)], &[]);
    let play = obj(
        "Kingmaker.Designers.EventConditionActionSystem.Actions.PlayCutscene",
        &[("m_Cutscene", Node::BpRef(c))],
    );
    let owner = w.content.get(c).expect("cutscene");
    crate::elements::cutscene::play(&mut w, &owner, crate::content::View(&play)).expect("play");
    assert!(!w.coverage.valid(), "the dialogue isn't started");
    assert!(w.scene.cutscene_pending.is_empty(), "Spawn doesn't run ahead of the dialogue");
}

/// `CutscenePlayerData.Restore` resumes the saved track positions, which aren't read: a
/// restored running cutscene with T0 actions is reported, not dropped.
#[test]
fn a_restored_cutscene_with_t0_actions_is_reported() {
    let (mut w, u) = world();
    let spawn = command_action(&w, 0xF5, vec![action("Spawn")]);
    let c = gate(&w, 0xF6, "Cutscene", vec![track(&[spawn], None)], &[]);
    w.scene.cutscenes.push(crate::scene::CutscenePlayer {
        entity: u,
        cutscene: Some(c),
        params: Vec::new(),
        is_queued: false,
        queued_after: false,
        restart: false,
        remove: false,
        activated_gates: 1,
    });
    crate::scene::cutscene_post_load(&mut w, u);
    assert!(w.coverage.unsupported.keys().any(|k| k.contains("Restore")));
}

/// The shapes built from the importer's inventory agree with the importer's own containment
/// probes (which the oracle's `Contains` verified, `wr-import`'s `oracle_diff`): needs
/// `cache/import/<area>/inventory.json` (`wr-import import --area prologue_caves_1`).
#[test]
#[ignore = "needs the importer's inventory of Prologue_Caves_1"]
fn real_zone_shapes_reproduce_the_importers_probes() {
    let path = wr_paths::cache_root().join("import/prologue_caves_1/inventory.json");
    let cache = SceneCache::import(&path, "bad465e5404604346bdf71ef50b3299a").expect("import");
    let inv: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    let mut probes = 0;
    for z in inv["zones"].as_array().expect("zones") {
        let uid = z["unique_id"].as_str().expect("uid");
        let info = cache.zone(uid).expect("zone");
        let raw: Vec<&serde_json::Value> = z["shapes"]
            .as_array()
            .expect("shapes")
            .iter()
            .filter(|s| s["active_in_hierarchy"].as_bool() == Some(true))
            .collect();
        assert_eq!(info.shapes.len(), raw.len(), "{uid}");
        for (shape, r) in info.shapes.iter().zip(raw) {
            for p in r["probes"].as_array().expect("probes") {
                let pt = &p["point"];
                let point = [
                    pt[0].as_f64().expect("x") as f32,
                    pt[1].as_f64().expect("y") as f32,
                    pt[2].as_f64().expect("z") as f32,
                ];
                let c = p["corpulence"].as_f64().expect("c") as f32;
                let want = p["inside"].as_bool();
                assert_eq!(shape.contains(point, c), want, "{uid} {point:?} {c}");
                probes += 1;
            }
        }
    }
    assert!(probes > 500, "{probes}");
}

/// `CurrentDifficultyStatAdjustmentCompareTo`: the preset at the option's ordinal
/// (`Custom` = -1 first).
#[test]
fn difficulty_comparison_uses_the_ordinal_of_the_option() {
    use crate::difficulty::{
        Difficulty, DifficultyPreset, DifficultyPresetAsset, DifficultyPresetsList,
    };
    let preset = |g: i32, stats: i32| DifficultyPresetAsset {
        name: format!("P{g}"),
        path_id: i64::from(g),
        is_custom_mode: g < 0,
        preset: DifficultyPreset {
            game_difficulty: g,
            death_door: false,
            dead_companions_rise_after_combat: false,
            remove_negative_effects_on_rest: false,
            remove_annoying_buffs_after_combat: false,
            damage_to_party: 1.0,
            enemy_critical_hits: 0,
            auto_level_up: 0,
            stats_adjustments: stats,
            enemy_difficulty: 1,
            encumbrance_slowdown: false,
            weather_effects: 0,
            respec_allowed: false,
            additional_ai_behaviors: false,
            combat_encounters_capacity: 0,
        },
    };
    let list = DifficultyPresetsList {
        path_id: 0,
        difficulties: vec![preset(-1, 0), preset(0, 0), preset(1, 1), preset(2, 2), preset(3, 3)],
        core_difficulty: 0,
        hard_difficulty: 0,
        unfair_difficulty: 0,
        stats_adjustment_presets: Vec::new(),
        mob_experience_coeffs: Vec::new(),
    };
    let mut d = Difficulty { list: Some(list), ..Difficulty::default() };
    d.settings.stats_adjustments = Some(2);
    assert_eq!(d.stat_adjustment_compare_to(2), Ok(0), "Normal");
    assert_eq!(d.stat_adjustment_compare_to(3), Ok(-1), "Daring needs more");
    assert_eq!(d.stat_adjustment_compare_to(1), Ok(1), "Casual needs less");
    assert!(d.stat_adjustment_compare_to(9).is_err());
    d.settings.stats_adjustments = None;
    assert!(d.stat_adjustment_compare_to(2).is_err());
}

// ---- RuleIdentifyItem and the player inventory's handlers ------------------------------

fn unidentified_potion(w: &World, g: u128) -> Rc<Blueprint> {
    insert(
        w,
        g,
        "Kingmaker.Blueprints.Items.Equipment.BlueprintItemEquipmentUsable",
        &[("m_IdentifyDC", Node::Int(15))],
    )
}

/// A unit without ranks in Knowledge (Arcana) rolls nothing; the rule still runs and is
/// recorded. With a rank the skill check isn't ported and is reported.
#[test]
fn identify_item_needs_a_rank_and_reports_the_check() {
    let (mut w, u) = world();
    let bp = unidentified_potion(&w, 0x1D);
    let item = crate::items::create_entity(&mut w, &bp).expect("item");
    assert!(!w.item(item).expect("item").is_identified, "DC 15: not identified");
    crate::rules::identify::try_identify(&mut w, u, item);
    assert!(w.coverage.valid(), "no rank: nothing rolled");
    assert_eq!(w.events.records.iter().filter(|r| r.k == "rule").count(), 1);
    assert!(!w.item(item).expect("item").is_identified);
    crate::stats::ops::set_base_value(&mut w, u, S::SkillKnowledgeArcana, 3);
    crate::rules::identify::try_identify(&mut w, u, item);
    assert!(!w.coverage.valid(), "a rank would roll: reported");
}

/// The player's inventory is a global subscriber: a level-up (or recruit) of any unit makes
/// it try to identify each of its items for that unit.
#[test]
fn the_player_inventory_identifies_for_a_unit_that_levels_up() {
    let (mut w, u) = world();
    let bp = unidentified_potion(&w, 0x1E);
    let item = crate::items::create_entity(&mut w, &bp).expect("item");
    w.main_character = Some(u);
    w.unit_mut(u).expect("unit").inventory = vec![item];
    crate::rules::identify::subscribe_player_inventory(&mut w);
    crate::rulebook::bus::raise_global_call(
        &mut w,
        "ILevelUpCompleteUIHandler",
        "HandleLevelUpComplete",
        Some(u),
    );
    let rules = w.events.records.iter().filter(|r| r.k == "rule").count();
    assert_eq!(rules, 1, "one RuleIdentifyItem for the one unidentified item");
}

// ---- component ports of the Neathholm NPCs --------------------------------------------

/// `ArmorSpeedPenaltyRemoval`: the medium-armor immunity, and at rank 2 the full one, as
/// counted flags.
#[test]
fn armor_speed_penalty_removal_retains_its_flags_by_rank() {
    for (rank, full) in [(1, false), (2, true)] {
        let (mut w, u) = world();
        let comp = Node::Object(Box::new(obj(
            "Kingmaker.Designers.Mechanics.Facts.ArmorSpeedPenaltyRemoval",
            &[("name", Node::Str("$ArmorSpeedPenaltyRemoval$a".into()))],
        )));
        let bp = feature(&w, 0xA5, vec![comp]);
        let f = loaded_fact(&mut w, u, &bp, true);
        w.fact_mut(f).rank = rank;
        fl::manager_restore_owner_link(&mut w, u);
        fl::manager_post_load(&mut w, u);
        crate::entity::turn_on(&mut w, u);
        let feat = |w: &World, k: i64| w.unit(u).expect("unit").mechanics_features.get(&k).copied();
        assert_eq!(feat(&w, 30), Some(1), "ImmunityToMediumArmorSpeedPenalty");
        assert_eq!(feat(&w, 14).is_some(), full, "ImmunityToArmorSpeedPenalty at rank {rank}");
        crate::entity::turn_off(&mut w, u);
        assert_eq!(feat(&w, 30), Some(0));
    }
}

/// `UnitPartWeaponTraining.GetMaxWeaponRank` reads `HandSlot.MaybeWeapon`: with empty hands
/// that is the empty-hand weapon, so a training of its fighter group counts.
#[test]
fn weapon_training_counts_the_empty_hand_weapon() {
    let (mut w, u) = world();
    let group = 3;
    let ty = insert(
        &w,
        0x7E1,
        "Kingmaker.Blueprints.Items.Weapons.BlueprintWeaponType",
        &[("m_FighterGroupFlags", Node::Enum(1 << group))],
    );
    let wbp = insert(
        &w,
        0x7E2,
        "Kingmaker.Blueprints.Items.Weapons.BlueprintItemWeapon",
        &[("m_Type", Node::BpRef(ty.guid))],
    );
    let fist = crate::combat::weapon::create_weapon(&mut w, wbp.guid, u);
    {
        let body = &mut w.unit_mut(u).expect("unit").body;
        body.empty_hand = Some(fist);
        for hand in ["PrimaryHand", "SecondaryHand"] {
            body.slots.push(crate::unit::Slot {
                name: format!("HandsEquipmentSets[0].{hand}"),
                item: None,
                active: true,
            });
        }
    }
    let training = |name: &str, ty: &str, set: &[(&str, Node)]| {
        let mut fields = vec![("name", Node::Str(format!("${name}$t").into()))];
        fields.extend(set.iter().cloned());
        Node::Object(Box::new(obj(ty, &fields)))
    };
    let bp = feature(
        &w,
        0x7E3,
        vec![
            training("WeaponTraining", "Kingmaker.UnitLogic.FactLogic.WeaponTraining", &[]),
            training(
                "WeaponGroupAttackBonus",
                "Kingmaker.Designers.Mechanics.Facts.WeaponGroupAttackBonus",
                &[("WeaponGroup", Node::Enum(group))],
            ),
            training(
                "WeaponTrainingBonuses",
                "Kingmaker.Designers.Mechanics.Buffs.WeaponTrainingBonuses",
                &[
                    ("Stat", Node::Enum(i64::from(S::AdditionalAttackBonus.0))),
                    ("Descriptor", Node::Enum(0)),
                ],
            ),
        ],
    );
    let f = loaded_fact(&mut w, u, &bp, true);
    w.fact_mut(f).rank = 2;
    let before = w.stats(u).v(S::AdditionalAttackBonus).modified_value;
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    crate::entity::turn_on(&mut w, u);
    assert_eq!(w.stats(u).v(S::AdditionalAttackBonus).modified_value, before + 2);
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

/// `AddUndetectableAlignment`: the alignment's `Undetectable` flag while the fact is active.
#[test]
fn undetectable_alignment_is_a_counted_flag() {
    let (mut w, u) = world();
    let comp = Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.FactLogic.AddUndetectableAlignment",
        &[("name", Node::Str("$AddUndetectableAlignment$a".into()))],
    )));
    let bp = feature(&w, 0xA6, vec![comp]);
    w.unit_mut(u).expect("unit").alignment =
        Some(crate::narrative::alignment::UnitAlignment::initialized(4));
    let f = loaded_fact(&mut w, u, &bp, false);
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    let undetectable =
        |w: &World| w.unit(u).and_then(|x| x.alignment.as_ref()).map(|a| a.undetectable);
    fl::activate(&mut w, f);
    assert_eq!(undetectable(&w), Some(1), "the unit's own UnitAlignment.Undetectable");
    assert!(w.player.counters.is_empty(), "no side counter");
    fl::deactivate(&mut w, f);
    assert_eq!(undetectable(&w), Some(0));
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
    // A unit without an alignment can't hold the flag: reported.
    let (mut w, u) = world();
    w.unit_mut(u).expect("unit").alignment = None;
    let bp = feature(
        &w,
        0xA7,
        vec![Node::Object(Box::new(obj(
            "Kingmaker.UnitLogic.FactLogic.AddUndetectableAlignment",
            &[("name", Node::Str("$AddUndetectableAlignment$a".into()))],
        )))],
    );
    let f = loaded_fact(&mut w, u, &bp, false);
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    fl::activate(&mut w, f);
    assert!(!w.coverage.valid());
}

/// `AddFacts.CasterLevel`: `Params` is `m_Params ?? ParentContext?.Params ??
/// AbilityParams.Empty`; a context with neither writes the one shared `Empty`.
#[test]
fn caster_level_goes_to_the_params_the_context_reads() {
    let (mut w, u) = world();
    let bp = feature(&w, 0xA7, Vec::new());
    let f = loaded_fact(&mut w, u, &bp, false);
    w.fact_mut(f).context = Some(crate::context::MechanicsContext::default());
    crate::context::set_params_caster_level(&mut w, f, 9);
    assert_eq!(w.empty_params.caster_level, 9, "AbilityParams.Empty is shared");
    assert!(w.empty_params.known);
    assert!(w.coverage.valid());
    // A context with params of its own is not reached by a fresh fact: reported.
    w.fact_mut(f).context = Some(crate::context::MechanicsContext {
        params: Some(serde_json::json!({ "SpellLevel": 1 })),
        ..Default::default()
    });
    crate::context::set_params_caster_level(&mut w, f, 7);
    assert!(!w.coverage.valid());
}

// ---- map objects ----------------------------------------------------------------------

fn map_object_world(
    components: Vec<crate::scene::CompInfo>,
    parts: &[&'static str],
) -> (World, EntityIdx) {
    let (mut w, _) = world();
    let e = w.add_entity(Entity {
        unique_id: "obj".into(),
        clr: "MapObjectEntityData",
        kind: EntityKind::Other,
        facts: FactsManager::default(),
        parts: parts.iter().map(|p| crate::world::Part { clr: p }).collect(),
        is_turned_on: true,
        post_load_executed: true,
        is_in_game: true,
        subscribed: false,
        disposing: false,
        is_disposed: false,
    });
    w.scene.map_objects.insert(e, crate::scene_parts::MapObj::default());
    w.scene.cache = Some(Rc::new(SceneCache {
        schema: crate::scene::SCHEMA.into(),
        area: "a".into(),
        name: "a".into(),
        source: String::new(),
        zones: Vec::new(),
        views: vec![crate::scene::ViewInfo {
            unique_id: "obj".into(),
            class: "Kingmaker.View.MapObjects.MapObjectView".into(),
            scene: "s".into(),
            active_in_hierarchy: true,
            is_in_game: true,
            components,
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            group: None,
        }],
    }));
    (w, e)
}

fn comp(class: &str, fields: serde_json::Value) -> crate::scene::CompInfo {
    crate::scene::CompInfo { class: class.into(), fields }
}

/// `AreaTransitionPart.UpdateVisibility`: the object is in game while the visibility flag is
/// unlocked; the flag's changes reach it.
#[test]
fn an_area_transition_follows_its_visibility_flag() {
    let flag: u128 = 0xF1A9;
    let (mut w, e) = map_object_world(
        vec![comp(
            "Kingmaker.View.MapObjects.AreaTransition",
            serde_json::json!({ "Settings": {
                "m_VisibilityFlag": { "guid": format!("{flag:032x}") },
                "m_VisibilityEtude": { "guid": "" },
                "AddMapMarker": 0 } }),
        )],
        &["AreaTransitionPart"],
    );
    crate::scene_parts::update_all_visibilities(&mut w);
    assert!(!w.entity(e).is_in_game, "the flag is locked");
    crate::narrative::flags::unlock(&mut w, Guid(flag));
    assert!(w.entity(e).is_in_game, "unlocking shows it");
    crate::narrative::flags::lock(&mut w, Guid(flag));
    assert!(!w.entity(e).is_in_game);
}

/// `KingdomLootPart.OnTurnOn`: the chest's loot becomes the shared stash and `Enabled`
/// follows `HasLoot || !DisableWhenEmpty`; a change shows the map marker.
#[test]
fn a_kingdom_chest_turns_on_with_the_stash() {
    let (mut w, e) = map_object_world(
        vec![
            comp(
                "Kingmaker.View.MapObjects.InteractionLoot",
                serde_json::json!({ "Settings": { "LootContainerType": 0, "AddMapMarker": 1,
                    "ShowOnMapWhenEmpty": 0, "AlwaysDisabled": 0, "m_LootTables": [] } }),
            ),
            comp(
                "Kingmaker.View.MapObjects.KingdomLootComponent",
                serde_json::json!({ "type": 0 }),
            ),
        ],
        &["InteractionLootPart", "LocalMapMarkerPart", "KingdomLootPart"],
    );
    {
        let m = w.scene.map_objects.get_mut(&e).expect("object");
        m.loot = Some(crate::scene_parts::LootPart {
            enabled: false,
            loot: None,
            stash: None,
            known_items: Some(Vec::new()),
        });
        m.marker_hidden = Some(true);
    }
    // An empty stash leaves the (default-loot) chest off.
    assert!(crate::scene_parts::part_hook(&mut w, e, "KingdomLootPart", "OnTurnOn"));
    let m = &w.scene.map_objects[&e];
    assert!(!m.loot.as_ref().expect("loot").enabled);
    assert_eq!(m.loot.as_ref().expect("loot").stash, Some(0));
    // A stash with something lootable enables it and shows its marker.
    w.scene.stashes.has_loot[0] = true;
    assert!(crate::scene_parts::part_hook(&mut w, e, "KingdomLootPart", "OnTurnOn"));
    let m = &w.scene.map_objects[&e];
    assert!(m.loot.as_ref().expect("loot").enabled);
    assert_eq!(m.marker_hidden, Some(false));
}

/// The attach step leaves the saved parts alone only when the view backs them: a part no
/// view component backs, or one the view would create, is reported.
#[test]
fn a_view_that_disagrees_with_the_saved_parts_is_reported() {
    let (mut w, e) = map_object_world(Vec::new(), &["InteractionSkillCheckPart"]);
    crate::scene_parts::view_attach(&mut w, e);
    assert!(!w.coverage.valid(), "a saved part no component backs would be removed");
    let (mut w, e) = map_object_world(
        vec![comp(
            "Kingmaker.View.MapObjects.InteractionSkillCheck",
            serde_json::json!({ "Settings": { "Trap": { "m_FileID": 0, "m_PathID": 0 },
                "DC": 0, "Exact": 0, "HasCalculatedDC": 0, "DCModifiers": [], "SkillPool": [] } }),
        )],
        &["InteractionSkillCheckPart"],
    );
    w.scene.map_objects.get_mut(&e).expect("object").skill_check =
        Some(crate::scene_parts::SkillCheckPart { dc_override: 9, skill_override: 3 });
    crate::scene_parts::view_attach(&mut w, e);
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
    // `OnSettingsDidSet` at load: a zero DC stays zero, an empty pool gives `Unknown`.
    assert_eq!(
        w.scene.map_objects[&e].skill_check,
        Some(crate::scene_parts::SkillCheckPart { dc_override: 0, skill_override: 0 })
    );
    let (mut w, e) = map_object_world(
        vec![comp(
            "Kingmaker.View.MapObjects.AreaTransition",
            serde_json::json!({ "Settings": {
                "m_VisibilityFlag": { "guid": "" }, "m_VisibilityEtude": { "guid": "" } } }),
        )],
        &[],
    );
    crate::scene_parts::view_attach(&mut w, e);
    assert!(!w.coverage.valid(), "the part would be created");
}

// ---- review round: the units pool, skill checks, cutscenes, load order ----------------

/// A second unit at `pos`, in the area's state.
fn area_unit(w: &mut World, id: &str, pos: [f32; 3], in_game: bool) -> EntityIdx {
    let unit = crate::unit::Unit {
        stats: crate::stats::value::Stats::fresh(),
        position: pos,
        ..crate::unit::Unit::default()
    };
    let e = w.add_entity(Entity {
        unique_id: id.into(),
        clr: "UnitEntityData",
        kind: EntityKind::Unit(Box::new(unit)),
        facts: FactsManager::default(),
        parts: Vec::new(),
        is_turned_on: true,
        post_load_executed: true,
        is_in_game: in_game,
        subscribed: false,
        disposing: false,
        is_disposed: false,
    });
    w.player.area_units.push(e);
    e
}

/// `ScriptZone.OnAreaDidLoad` enumerates `Game.State.Units`: a unit out of the game (or of a
/// fake blueprint) isn't enumerated, so it can't use up a once-only zone before the unit
/// after it.
#[test]
fn a_hidden_unit_does_not_use_up_a_once_only_zone_at_load() {
    let (mut w, u, z) = zone_world(true, false);
    // The test world's first unit isn't in the area's state.
    w.entity_mut(u).is_in_game = false;
    {
        let cache = w.scene.cache.as_mut().expect("cache");
        Rc::get_mut(cache).expect("unique").zones[0].shapes = vec![cylinder([0.0; 3], 3.0, 5.0)];
    }
    let hidden = area_unit(&mut w, "hidden", [0.0; 3], false);
    let fake_bp =
        insert(&w, 0xFA4E, "Kingmaker.Blueprints.BlueprintUnit", &[("IsFake", Node::Bool(true))]);
    let fake = area_unit(&mut w, "fake", [0.0; 3], true);
    w.unit_mut(fake).expect("unit").blueprint = Some(fake_bp.guid);
    let eligible = area_unit(&mut w, "eligible", [1.0, 0.0, 0.0], true);
    crate::scene::on_area_did_load(&mut w);
    let inside: Vec<EntityIdx> = w.scene.zones[z].inside.iter().map(|i| i.unit).collect();
    assert_eq!(inside, vec![eligible], "hidden {hidden:?} and fake {fake:?} aren't enumerated");
    assert!(!w.scene.zones[z].is_active, "the eligible unit used the zone up");
    // A zone firing at load runs among the other `OnAreaDidLoad` subscribers: reported.
    let kinds: Vec<&String> = w.coverage.unsupported.keys().collect();
    assert_eq!(
        kinds,
        vec![
            "ScriptZone.OnAreaDidLoad: a unit enters at load (order among the IAreaHandler subscribers)"
        ]
    );
}

/// A skill-check object whose view settings are `settings`, with id `id`, after the attach.
fn skill_check_after_attach(id: &str, settings: serde_json::Value) -> World {
    let (mut w, e) = map_object_world(
        vec![comp("Kingmaker.View.MapObjects.InteractionSkillCheck", settings)],
        &["InteractionSkillCheckPart"],
    );
    let root = insert(
        &w,
        0x1A14,
        "Kingmaker.Interaction.BlueprintInteractionRoot",
        &[("m_InteractionDCVariation", Node::Int(2))],
    );
    insert(
        &w,
        crate::unit::BLUEPRINT_ROOT.0,
        "Kingmaker.Blueprints.Root.BlueprintRoot",
        &[("m_InteractionRoot", Node::BpRef(root.guid))],
    );
    w.entity_mut(e).unique_id = id.into();
    Rc::get_mut(w.scene.cache.as_mut().expect("cache")).expect("unique").views[0].unique_id =
        id.into();
    w.scene.map_objects.get_mut(&e).expect("object").skill_check =
        Some(crate::scene_parts::SkillCheckPart { dc_override: 0, skill_override: 7 });
    crate::scene_parts::view_attach(&mut w, e);
    w
}

fn skill_settings(dc: i64, exact: i64, calculated: i64, pool: &[i64]) -> serde_json::Value {
    serde_json::json!({ "Settings": { "Trap": { "m_FileID": 0, "m_PathID": 0 }, "DC": dc,
        "Exact": exact, "HasCalculatedDC": calculated, "DCModifiers": [], "SkillPool": pool } })
}

/// `InteractionSkillCheckPart.OnSettingsDidSet(true)` at load: `DCOverride` is the DC plus
/// `Math.Abs(UniqueId.GetHashCode()) % 5 - 2` (one of the saved parts: DC 12, saved 11), the
/// DC itself when `Exact` or zero, and `SkillOverride` the empty pool's `Unknown`. A pool's
/// draw and a calculated DC are reported.
#[test]
fn a_skill_check_recomputes_its_dc_and_skill_at_load() {
    let id = "6fdbec9e-a0ee-4f73-9042-e2ad802c053b";
    let state = |w: &World| w.scene.map_objects.values().next().and_then(|m| m.skill_check);
    let w = skill_check_after_attach(id, skill_settings(12, 0, 0, &[]));
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
    assert_eq!(
        state(&w),
        Some(crate::scene_parts::SkillCheckPart { dc_override: 11, skill_override: 0 })
    );
    let w = skill_check_after_attach(id, skill_settings(12, 1, 0, &[]));
    assert_eq!(state(&w).map(|s| s.dc_override), Some(12), "Exact");
    let w = skill_check_after_attach(id, skill_settings(12, 0, 0, &[17, 45]));
    assert!(!w.coverage.valid(), "the pool draws from the global RNG");
    let w = skill_check_after_attach(id, skill_settings(12, 0, 1, &[]));
    assert!(!w.coverage.valid(), "CalculatedDC");
}

/// `new UnitEntityData(view)` starts the unit with the view's `IsInGameBySettings`, the
/// prefab's `m_IsInGame`: read from the prefab table, and reported when the table or the
/// prefab's value is missing.
#[test]
fn a_unit_starts_with_its_prefabs_in_game_setting() {
    use crate::scene_spawn::{blueprint_prefab, prefab_in_game};
    let (mut w, _) = world();
    let bp = insert(
        &w,
        0x9A1,
        "Kingmaker.Blueprints.BlueprintUnit",
        &[("Prefab", Node::WeakLink(Some("hiddenprefab".into())))],
    );
    assert_eq!(blueprint_prefab(&bp).as_deref(), Some("hiddenprefab"));
    assert!(prefab_in_game(&mut w, Some("hiddenprefab")), "no table: built in game");
    assert!(!w.coverage.valid(), "and reported");
    let (mut w, _) = world();
    w.scene.unit_prefabs = Some(Rc::new(
        [("hiddenprefab".to_owned(), Some(false)), ("ambiguous".to_owned(), None)]
            .into_iter()
            .collect(),
    ));
    assert!(!prefab_in_game(&mut w, Some("hiddenprefab")), "the prefab's setting");
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
    prefab_in_game(&mut w, Some("ambiguous"));
    assert!(!w.coverage.valid(), "no single value");
}

/// `SleepingUnitsController.ShouldBeSleeping` from what the sim knows: out of the game
/// sleeps, a party member or a unit in combat is awake, and a unit a spawner just made (in
/// game, not in the party) depends on the fog of war and its timers: unknown, not awake.
#[test]
fn a_spawned_unit_is_not_assumed_awake() {
    use crate::spawn::{Awake, awake_state};
    let (mut w, _) = world();
    let spawned = area_unit(&mut w, "spawned", [0.0; 3], true);
    assert_eq!(awake_state(&mut w, spawned, &[]), Awake::Unknown);
    assert_eq!(awake_state(&mut w, spawned, &[spawned]), Awake::Yes, "a party member");
    w.unit_mut(spawned).expect("unit").combat.in_combat = true;
    assert_eq!(awake_state(&mut w, spawned, &[]), Awake::Yes, "in combat");
    w.unit_mut(spawned).expect("unit").finally_dead = true;
    assert_eq!(awake_state(&mut w, spawned, &[spawned]), Awake::Unknown, "dead: the timers");
    w.entity_mut(spawned).is_in_game = false;
    assert_eq!(awake_state(&mut w, spawned, &[spawned]), Awake::No, "out of the game");
    assert!(w.coverage.valid());
}
