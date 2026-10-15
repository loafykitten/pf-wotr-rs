//! The load lifecycle on synthetic facts built from real component types.

use std::rc::Rc;

use wr_content::node::{Node, Object};
use wr_content::{Guid, schema};

use super::lifecycle as fl;
use crate::content::Blueprint;
use crate::stats::types::StatType as S;
use crate::world::{Component, EntityIdx, Fact, FactIdx, FactKind, World};

/// An object of a content type with the given fields set (others absent).
pub(crate) fn obj(ty: &str, set: &[(&str, Node)]) -> Object {
    let t = schema::find(ty).unwrap_or_else(|| panic!("type {ty}"));
    let d = schema::ty(t);
    let mut fields = vec![Node::Absent; d.fields.len()];
    for (name, v) in set {
        let i =
            d.fields.iter().position(|f| f.name == *name).unwrap_or_else(|| panic!("{ty}.{name}"));
        fields[i] = v.clone();
    }
    Object { ty: t, tagged: true, fields, extra: Vec::new() }
}

pub(crate) fn stat_bonus(name: &str, stat: S, value: i64) -> Node {
    Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.FactLogic.AddStatBonus",
        &[
            ("name", Node::Str(name.into())),
            ("Stat", Node::Enum(i64::from(stat.0))),
            ("Value", Node::Int(value)),
            ("Descriptor", Node::Enum(9)),
        ],
    )))
}

/// A feature blueprint with the given components.
pub(crate) fn feature(w: &World, g: u128, comps: Vec<Node>) -> Rc<Blueprint> {
    let o =
        obj("Kingmaker.Blueprints.Classes.BlueprintFeature", &[("Components", Node::Seq(comps))]);
    w.content.insert(Blueprint {
        guid: Guid(g),
        name: format!("F{g}"),
        obj: o,
        source: crate::content::Source::Bbp,
    });
    w.content.get(Guid(g)).expect("inserted")
}

/// A loaded (deserialized) fact: saved `IsActive`, runtimes named as the blueprint's
/// components (null dictionary entries), nothing turned on.
pub(crate) fn loaded_fact(
    w: &mut World,
    u: EntityIdx,
    bp: &Rc<Blueprint>,
    active: bool,
) -> FactIdx {
    let mut f = Fact::blank(FactKind::Feature, "Feature");
    f.blueprint = Some(bp.guid);
    f.bp = Some(bp.clone());
    f.unique_id = format!("f{}", w.facts.len());
    f.is_active = active;
    for o in bp.components_array() {
        f.components.push(Component {
            name: crate::content::View(o).str("name").unwrap_or_default().to_owned(),
            source: None,
            saved_clr: None,
            data: None,
            is_turned_on: false,
            is_disposed: false,
            subscribed: false,
            fact_refs: std::collections::HashMap::new(),
            runtime: serde_json::Map::new(),
            need_resume: false,
        });
    }
    let fi = w.add_fact(f);
    w.entity_mut(u).facts.list.push(fi);
    fi
}

fn registry_world() -> (World, EntityIdx) {
    let (mut w, u) = crate::stats::tests::world_with_unit();
    w.registry = Rc::new(crate::components::registry());
    w.trace = Some(Vec::new());
    (w, u)
}

#[test]
fn a_loaded_active_fact_gets_post_load_and_turn_on_not_activate() {
    let (mut w, u) = registry_world();
    let bp = feature(&w, 1, vec![stat_bonus("$AddStatBonus$a", S::Strength, 2)]);
    let f = loaded_fact(&mut w, u, &bp, true);
    let before = w.stats(u).v(S::Strength).modified_value;
    assert_eq!(before, 10);
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    fl::manager_turn_on(&mut w, u);
    let steps: Vec<&str> = w.trace.as_ref().unwrap().iter().map(|t| t.what).collect();
    assert_eq!(
        steps,
        vec!["Fact.PostLoad", "FactsManager.TurnOn", "Fact.TurnOn", "Component.TurnOn"]
    );
    assert!(!steps.contains(&"Fact.Activate"));
    assert!(w.fact(f).is_turned_on);
    assert_eq!(w.stats(u).v(S::Strength).modified_value, before + 2);
    // The modifier carries its source fact and component name.
    let m = w.stats(u).v(S::Strength).modifiers().next().expect("modifier");
    assert_eq!(w.mods[m.i()].source, Some(f));
    assert_eq!(w.mods[m.i()].source_component.as_deref(), Some("$AddStatBonus$a"));
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

#[test]
fn turn_off_removes_and_turn_on_restores_without_duplicates() {
    let (mut w, u) = registry_world();
    let bp = feature(&w, 2, vec![stat_bonus("$AddStatBonus$b", S::Dexterity, 4)]);
    let f = loaded_fact(&mut w, u, &bp, true);
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    fl::manager_turn_on(&mut w, u);
    let on = w.stats(u).v(S::Dexterity).modified_value;
    fl::turn_off(&mut w, f);
    assert_eq!(w.stats(u).v(S::Dexterity).modified_value, on - 4);
    fl::turn_on(&mut w, f);
    fl::turn_off(&mut w, f);
    fl::turn_on(&mut w, f);
    assert_eq!(w.stats(u).v(S::Dexterity).modified_value, on);
    // Deactivate turns off first, then deactivates components; IsActive goes false last.
    fl::deactivate(&mut w, f);
    assert!(!w.fact(f).is_active && !w.fact(f).is_turned_on);
    assert_eq!(w.stats(u).v(S::Dexterity).modified_value, on - 4);
}

#[test]
fn inactive_facts_are_not_turned_on() {
    let (mut w, u) = registry_world();
    let bp = feature(&w, 3, vec![stat_bonus("$AddStatBonus$c", S::Wisdom, 2)]);
    let f = loaded_fact(&mut w, u, &bp, false);
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    fl::manager_turn_on(&mut w, u);
    assert!(!w.fact(f).is_turned_on);
}

#[test]
fn orphaned_runtime_is_disposed_and_new_blueprint_component_is_created_by_fixes() {
    let (mut w, u) = registry_world();
    let bp = feature(&w, 4, vec![stat_bonus("$AddStatBonus$new", S::Charisma, 1)]);
    let f = loaded_fact(&mut w, u, &bp, true);
    // The save had a runtime under a name the blueprint no longer has, and lacks the new one.
    w.fact_mut(f).components[0].name = "$Gone$x".into();
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    assert!(w.fact(f).components.is_empty(), "orphan disposed and dropped");
    fl::manager_apply_post_load_fixes(&mut w, u);
    // `Setup(Blueprint)` created the missing runtime: Initialize, and Activate (the fact is
    // active) — the exception to "no activation replay on load".
    assert_eq!(w.fact(f).components.len(), 1);
    assert!(w.trace.as_ref().unwrap().iter().any(|t| t.what == "Fact.AddComponent"));
    fl::manager_turn_on(&mut w, u);
    assert!(w.fact(f).components[0].is_turned_on);
}

#[test]
fn unported_overridden_hooks_make_the_run_invalid_but_empty_ones_dont() {
    let (mut w, u) = registry_world();
    // `AddDamageResistanceEnergy` has a custom runtime (unported): every hook counts.
    let dr = Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.FactLogic.AddDamageResistanceEnergy",
        &[("name", Node::Str("$DR$1".into()))],
    )));
    let bp = feature(&w, 5, vec![dr]);
    loaded_fact(&mut w, u, &bp, true);
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    assert!(!w.coverage.valid());
    assert!(
        w.coverage
            .unsupported
            .keys()
            .any(|k| k.starts_with("AddDamageResistanceEnergy.OnPostLoad"))
    );
    // `AddClassSkill` overrides only OnTurnOn/OnTurnOff: its (ported) PostLoad is a no-op.
    let (mut w, u) = registry_world();
    let cs = Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.FactLogic.AddClassSkill",
        &[
            ("name", Node::Str("$CS$1".into())),
            ("Skill", Node::Enum(i64::from(S::SkillAthletics.0))),
        ],
    )));
    let bp = feature(&w, 6, vec![cs]);
    loaded_fact(&mut w, u, &bp, true);
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    fl::manager_turn_on(&mut w, u);
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

#[test]
fn turn_on_order_follows_monos_unstable_priority_sort() {
    let (mut w, u) = registry_world();
    let mut ids = Vec::new();
    for i in 0..20u128 {
        let bp = feature(&w, 100 + i, vec![stat_bonus(&format!("$S${i}"), S::Strength, 0)]);
        ids.push(loaded_fact(&mut w, u, &bp, true));
    }
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    w.trace = Some(Vec::new());
    fl::manager_turn_on(&mut w, u);
    let order: Vec<String> = w
        .trace
        .as_ref()
        .unwrap()
        .iter()
        .filter(|t| t.what == "Fact.TurnOn")
        .map(|t| t.subject.clone())
        .collect();
    // All priorities are equal: the order is the introsort's, not the list's.
    let mut expect = ids.clone();
    let cap = w.entity(u).facts.capacity.max(crate::csharp::sort::grown_capacity(ids.len()));
    crate::csharp::introsort(&mut expect, cap, &mut |_, _| 0);
    let expect: Vec<String> = expect.iter().map(|f| w.fact_label(*f)).collect();
    assert_eq!(order, expect);
    assert_ne!(order, ids.iter().map(|f| w.fact_label(*f)).collect::<Vec<_>>());
}

#[test]
fn saved_runtimes_are_matched_by_their_closed_delegate_type() {
    let (mut w, u) = registry_world();
    let bp = feature(&w, 7, vec![stat_bonus("$AddStatBonus$t", S::Strength, 1)]);
    let ok = loaded_fact(&mut w, u, &bp, true);
    let bad = loaded_fact(&mut w, u, &bp, true);
    let rt = |data: &str| {
        format!(
            "Kingmaker.EntitySystem.EntityFactComponentDelegate`2+ComponentRuntime[[Kingmaker.EntitySystem.Entities.UnitEntityData, Assembly-CSharp],[{data}, Assembly-CSharp]], Assembly-CSharp"
        )
    };
    w.fact_mut(ok).components[0].saved_clr =
        Some(rt("Kingmaker.UnitLogic.FactLogic.AddStatBonus+ComponentData"));
    // Another delegate's data type with the same short name: `Setup` rejects it.
    w.fact_mut(bad).components[0].saved_clr =
        Some(rt("Kingmaker.UnitLogic.FactLogic.AddContextStatBonus+ComponentData"));
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    assert_eq!(w.fact(ok).components.len(), 1);
    assert!(w.fact(bad).components.is_empty(), "rejected runtime is disposed and dropped");
}

fn insert_bp(w: &World, g: u128, ty: &str, set: &[(&str, Node)]) -> Rc<Blueprint> {
    w.content.insert(Blueprint {
        guid: Guid(g),
        name: format!("B{g}"),
        obj: obj(ty, set),
        source: crate::content::Source::Bbp,
    });
    w.content.get(Guid(g)).expect("inserted")
}

fn add_facts_node(name: &str, children: &[u128]) -> Node {
    Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.FactLogic.AddFacts",
        &[
            ("name", Node::Str(name.into())),
            ("m_Facts", Node::Seq(children.iter().map(|g| Node::BpRef(Guid(*g))).collect())),
        ],
    )))
}

const FEATURE: &str = "Kingmaker.Blueprints.Classes.BlueprintFeature";

fn find_fact(w: &World, u: EntityIdx, g: Guid) -> Option<FactIdx> {
    w.entity(u).facts.list.iter().copied().find(|f| w.fact(*f).blueprint == Some(g))
}

#[test]
fn rank_changes_reapply_the_children_instead_of_recreating_them() {
    // `Feature.AddRank`/`RemoveRank` run inside `ReapplyScope` (`Feature.cs:246–282`), so
    // `AddFacts` reapplies its facts: the child keeps its identity.
    let (mut w, u) = registry_world();
    let child = insert_bp(&w, 301, FEATURE, &[]);
    let parent = insert_bp(
        &w,
        300,
        FEATURE,
        &[
            ("Ranks", Node::Int(3)),
            ("Components", Node::Seq(vec![add_facts_node("$AF$1", &[301])])),
        ],
    );
    let p = crate::unit_facts::add_fact(&mut w, u, &parent).expect("parent");
    let c = find_fact(&w, u, child.guid).expect("child added");
    let id = w.fact(c).unique_id.clone();

    crate::unit_facts::add_fact(&mut w, u, &parent);
    assert_eq!(w.fact(p).rank, 2);
    assert!(!w.fact(p).reapplying);
    crate::unit_facts::remove(&mut w, u, p);
    assert_eq!(w.fact(p).rank, 1);

    assert_eq!(find_fact(&w, u, child.guid), Some(c), "the same child fact survives");
    assert_eq!(w.fact(c).unique_id, id);
    assert!(!w.fact(c).is_disposed);
    assert_eq!(w.facts.iter().filter(|f| f.blueprint == Some(child.guid)).count(), 1);
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

#[test]
fn gain_and_loss_events_reach_unit_subscribers() {
    // `UnitLogicCollection.OnFactDidAttach`/`OnFactWillDetach` raise their handlers; a
    // subscribed component whose handler isn't ported is reported (fail closed).
    let (mut w, u) = registry_world();
    let watched = insert_bp(&w, 401, FEATURE, &[]);
    let rof = Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.FactLogic.RecalculateOnOwnerFactUpdated",
        &[("name", Node::Str("$R$1".into())), ("m_Fact", Node::BpRef(Guid(401)))],
    )));
    let watcher = insert_bp(&w, 400, FEATURE, &[("Components", Node::Seq(vec![rof]))]);
    let wf = crate::unit_facts::add_fact(&mut w, u, &watcher).expect("watcher");
    fl::turn_on(&mut w, wf);
    w.coverage.unsupported.clear();
    crate::unit_facts::add_fact(&mut w, u, &watched);
    assert!(
        w.coverage
            .unsupported
            .keys()
            .any(|k| k == "RecalculateOnOwnerFactUpdated: IUnitGainFactHandler"),
        "{:?}",
        w.coverage.unsupported
    );
    w.coverage.unsupported.clear();
    let f = find_fact(&w, u, watched.guid).expect("watched");
    crate::unit_facts::remove(&mut w, u, f);
    assert!(
        w.coverage
            .unsupported
            .keys()
            .any(|k| k == "RecalculateOnOwnerFactUpdated: IUnitLostFactHandler"),
        "{:?}",
        w.coverage.unsupported
    );
}

#[test]
fn a_default_on_ability_records_its_buff_before_its_own_attachment() {
    // `ActivatableAbilityCollection.OnFactDidAttach` starts the ability inside
    // `DelegateOnFactDidAttach`; WrathOracle records `fact_attach` after it returns, so the
    // nested buff's attachment comes first.
    let (mut w, u) = registry_world();
    w.events.on = true;
    insert_bp(&w, 501, "Kingmaker.UnitLogic.Buffs.Blueprints.BlueprintBuff", &[]);
    let ability = insert_bp(
        &w,
        500,
        "Kingmaker.UnitLogic.ActivatableAbilities.BlueprintActivatableAbility",
        &[("IsOnByDefault", Node::Bool(true)), ("m_Buff", Node::BpRef(Guid(501)))],
    );
    crate::unit_facts::add_fact(&mut w, u, &ability).expect("ability");
    let attached: Vec<String> = w
        .events
        .records
        .iter()
        .filter(|e| e.fields.get("what").and_then(|v| v.as_str()) == Some("fact_attach"))
        .map(|e| e.fields["class"].as_str().unwrap_or("").to_owned())
        .collect();
    assert_eq!(attached, vec!["Buff", "ActivatableAbility"]);
}

const BUFF: &str = "Kingmaker.UnitLogic.Buffs.Blueprints.BlueprintBuff";

fn caster_level_rank_config() -> Node {
    Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.Mechanics.Components.ContextRankConfig",
        &[
            ("name", Node::Str("$rank$".into())),
            ("m_BaseValueType", Node::Enum(0)),
            ("m_Type", Node::Enum(0)),
        ],
    )))
}

/// `AddFacts.UpdateFacts` writes `unitFact.MaybeContext.Params.CasterLevel` on a fresh fact
/// whose context has no params, which is the shared static `AbilityParams.Empty`: the added
/// buff's caster-level rank sees it, and so does every later fact without params.
#[test]
fn add_facts_caster_level_is_written_into_the_shared_empty_params() {
    let (mut w, u) = registry_world();
    w.by_id.insert("u".into(), u);
    let buff =
        insert_bp(&w, 601, BUFF, &[("Components", Node::Seq(vec![caster_level_rank_config()]))]);
    let add = Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.FactLogic.AddFacts",
        &[
            ("name", Node::Str("$AF$cl".into())),
            ("m_Facts", Node::Seq(vec![Node::BpRef(buff.guid)])),
            ("CasterLevel", Node::Int(11)),
        ],
    )));
    let parent = insert_bp(&w, 600, FEATURE, &[("Components", Node::Seq(vec![add]))]);
    assert_eq!(w.empty_params.caster_level, 1);
    crate::unit_facts::add_fact(&mut w, u, &parent).expect("parent");
    let b = find_fact(&w, u, buff.guid).expect("buff added");
    assert_eq!(w.empty_params.caster_level, 11);
    assert_eq!(w.fact(b).context.as_ref().map(|c| c.ranks[0]), Some(11));
    // A later fact whose context has no params reads the changed value too.
    let other =
        insert_bp(&w, 602, BUFF, &[("Components", Node::Seq(vec![caster_level_rank_config()]))]);
    let o = crate::unit_facts::add_fact(&mut w, u, &other).expect("other");
    assert_eq!(w.fact(o).context.as_ref().map(|c| c.ranks[0]), Some(11));
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

/// `EntityDataBase.IsInGame` → `UnitEntityData.OnIsInGameChanged`: buffs (and activatable
/// abilities and abilities) are deactivated when the unit leaves the game, activated when it
/// comes back.
#[test]
fn leaving_the_game_deactivates_buffs_and_returning_activates_them() {
    let (mut w, u) = registry_world();
    let buff = insert_bp(&w, 611, BUFF, &[]);
    let feat = insert_bp(&w, 612, FEATURE, &[]);
    let b = crate::unit_facts::add_fact(&mut w, u, &buff).expect("buff");
    let ft = crate::unit_facts::add_fact(&mut w, u, &feat).expect("feature");
    assert!(w.fact(b).is_active && w.fact(ft).is_active);
    crate::entity::set_is_in_game(&mut w, u, false);
    assert!(!w.fact(b).is_active, "the buff is deactivated");
    assert!(w.fact(ft).is_active, "features are not touched");
    crate::entity::set_is_in_game(&mut w, u, true);
    assert!(w.fact(b).is_active);
    // The group's `Remove` (the cached `m_Group`) isn't modelled: each change reports it.
    let reported: Vec<_> = w.coverage.unsupported.keys().cloned().collect();
    assert_eq!(reported, ["UnitEntityData.OnIsInGameChanged: group membership"], "{reported:?}");
}

/// `AbilityParams.Empty` is process-wide: a second load starts from whatever the first left,
/// which a World can't know. A read before any write in a World reports that (once) and uses
/// the constructor defaults; a write makes the value known; a supplied start is used as is.
#[test]
fn empty_params_start_unknown_in_every_world_until_written_or_supplied() {
    let read_rank = |w: &mut World, u: EntityIdx, g: u128| {
        let buff =
            insert_bp(w, g, BUFF, &[("Components", Node::Seq(vec![caster_level_rank_config()]))]);
        let f = crate::unit_facts::add_fact(w, u, &buff).expect("buff");
        w.fact(f).context.as_ref().map(|c| c.ranks[0])
    };
    let kind = "AbilityParams.Empty read before any write in this load";
    let reports = |w: &World| {
        w.coverage
            .unsupported
            .iter()
            .filter(|(k, _)| k.starts_with(kind))
            .map(|(_, n)| *n)
            .sum::<u64>()
    };

    // First load: an `AddFacts` write (caster level 11), then a read: known, no report.
    let (mut w1, u1) = registry_world();
    w1.by_id.insert("u".into(), u1);
    w1.empty_params.caster_level = 11;
    w1.empty_params.known = true;
    assert_eq!(read_rank(&mut w1, u1, 700), Some(11));
    assert_eq!(reports(&w1), 0);

    // Second load: a fresh World does not inherit it. A read before any write reports once
    // and uses the constructor defaults.
    let (mut w2, u2) = registry_world();
    w2.by_id.insert("u".into(), u2);
    assert!(!w2.empty_params.known);
    assert_eq!(read_rank(&mut w2, u2, 701), Some(1));
    assert_eq!(read_rank(&mut w2, u2, 702), Some(1));
    assert_eq!(reports(&w2), 1, "reported once per load");

    // A supplied start is used and known.
    let (mut w3, u3) = registry_world();
    w3.by_id.insert("u".into(), u3);
    w3.empty_params = crate::world::EmptyAbilityParams::supplied(7, 0);
    assert_eq!(read_rank(&mut w3, u3, 703), Some(7));
    assert_eq!(reports(&w3), 0);
    assert_eq!(
        crate::hydrate::ability_params_empty_of(
            &serde_json::json!({ "abilityParamsEmpty": { "CasterLevel": 7, "RankBonus": 2 } })
        ),
        Some((7, 2))
    );
    assert_eq!(crate::hydrate::ability_params_empty_of(&serde_json::json!({ "units": [] })), None);
}

/// `AddMythicEnemyHitPointsBonus`: on, the owner's hit points get `CR * 2 * MythicLevel`
/// (`Mythic`); the level-up reapply event recomputes it; off, it goes.
#[test]
fn mythic_enemy_hit_points_follow_cr_and_mythic_level() {
    let (mut w, u) = registry_world();
    let exp = Node::Object(Box::new(obj(
        "Kingmaker.Blueprints.Classes.Experience.Experience",
        &[("CR", Node::Int(3)), ("Modifier", Node::F32(1.0))],
    )));
    let o = obj("Kingmaker.Blueprints.BlueprintUnit", &[("Components", Node::Seq(vec![exp]))]);
    w.content.insert(Blueprint {
        guid: Guid(810),
        name: "U810".into(),
        obj: o,
        source: crate::content::Source::Bbp,
    });
    {
        let x = w.unit_mut(u).expect("unit");
        x.blueprint = Some(Guid(810));
        x.progression.character_level = Some(5);
        x.progression.mythic_level = Some(2);
    }
    let comp = Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.FactLogic.AddMythicEnemyHitPointsBonus",
        &[("name", Node::Str("$AddMythicEnemyHitPointsBonus$a".into()))],
    )));
    let bp = feature(&w, 811, vec![comp]);
    let f = loaded_fact(&mut w, u, &bp, true);
    crate::stats::ops::set_base_value(&mut w, u, S::HitPoints, 20);
    let hp = w.stats(u).v(S::HitPoints).modified_value;
    fl::manager_restore_owner_link(&mut w, u);
    fl::manager_post_load(&mut w, u);
    fl::manager_turn_on(&mut w, u);
    assert_eq!(w.stats(u).v(S::HitPoints).modified_value, hp + 12);
    let from = |w: &World| {
        w.stats(u)
            .v(S::HitPoints)
            .modifiers()
            .filter(|m| w.mods[m.i()].source == Some(f))
            .map(|m| (w.mods[m.i()].desc, w.mods[m.i()].value))
            .collect::<Vec<_>>()
    };
    let mythic = crate::stats::types::ModifierDescriptor::Mythic;
    assert_eq!(from(&w), [(mythic, 12)]);
    w.unit_mut(u).expect("unit").progression.mythic_level = Some(3);
    crate::rulebook::bus::raise_unit(&mut w, u, "IUnitReapplyFeaturesOnLevelUpHandler");
    assert_eq!(w.stats(u).v(S::HitPoints).modified_value, hp + 18);
    assert_eq!(from(&w), [(mythic, 18)]);
    fl::manager_turn_off(&mut w, u);
    assert_eq!(w.stats(u).v(S::HitPoints).modified_value, hp);
}

/// `AddFeatureOnAlignment`: activated, its facts are added when the owner's alignment has
/// the component (a lawful good unit: `Lawful`), and removed with the fact; an alignment
/// without it adds nothing.
#[test]
fn alignment_features_follow_the_owners_alignment() {
    let (mut w, u) = registry_world();
    w.unit_mut(u).expect("unit").alignment =
        Some(crate::narrative::alignment::UnitAlignment::initialized(0));
    feature(&w, 820, Vec::new());
    let comp = |mask: i64| {
        Node::Object(Box::new(obj(
            "Kingmaker.Designers.Mechanics.Facts.AddFeatureOnAlignment",
            &[
                ("name", Node::Str("$AddFeatureOnAlignment$a".into())),
                ("Alignment", Node::Enum(mask)),
                ("m_Facts", Node::Seq(vec![Node::BpRef(Guid(820))])),
            ],
        )))
    };
    let has = |w: &World| {
        w.entity(u).facts.list.iter().filter(|f| w.fact(**f).blueprint == Some(Guid(820))).count()
    };
    let lawful = feature(&w, 821, vec![comp(8)]);
    let chaotic = feature(&w, 822, vec![comp(16)]);
    let f = crate::unit_facts::add_fact(&mut w, u, &lawful).expect("added");
    assert_eq!(has(&w), 1);
    crate::unit_facts::add_fact(&mut w, u, &chaotic).expect("added");
    assert_eq!(has(&w), 1);
    crate::unit_facts::remove(&mut w, u, f);
    assert_eq!(has(&w), 0);
    // An empty reference adds nothing but keeps its null in `AppliedFacts`.
    let with_null = Node::Object(Box::new(obj(
        "Kingmaker.Designers.Mechanics.Facts.AddFeatureOnAlignment",
        &[
            ("name", Node::Str("$AddFeatureOnAlignment$b".into())),
            ("Alignment", Node::Enum(8)),
            ("m_Facts", Node::Seq(vec![Node::BpRef(Guid::EMPTY), Node::BpRef(Guid(820))])),
        ],
    )));
    let bp = feature(&w, 823, vec![with_null]);
    let f = crate::unit_facts::add_fact(&mut w, u, &bp).expect("added");
    assert_eq!(has(&w), 1);
    let c = crate::world::CompRef { fact: f, comp: 0 };
    let list = w.component(c).fact_refs.get("AppliedFacts").cloned().expect("applied");
    assert_eq!(list.len(), 2);
    assert!(list[0].is_none() && list[1].is_some());
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

/// `HasArmorFeatureUnlock`: without filters its fact is added on activation and removed with
/// the owner fact; a filter by armor type on a unit without armor keeps it off.
#[test]
fn armor_unlocks_add_their_fact_when_every_filter_passes() {
    let (mut w, u) = registry_world();
    feature(&w, 830, Vec::new());
    let comp = |by_type: bool| {
        Node::Object(Box::new(obj(
            "Kingmaker.Designers.Mechanics.Buffs.HasArmorFeatureUnlock",
            &[
                ("name", Node::Str("$HasArmorFeatureUnlock$a".into())),
                ("m_NewFact", Node::BpRef(Guid(830))),
                ("FilterByBlueprintArmorTypes", Node::Bool(by_type)),
            ],
        )))
    };
    let has = |w: &World| {
        w.entity(u).facts.list.iter().filter(|f| w.fact(**f).blueprint == Some(Guid(830))).count()
    };
    let plain = feature(&w, 831, vec![comp(false)]);
    let filtered = feature(&w, 832, vec![comp(true)]);
    crate::unit_facts::add_fact(&mut w, u, &filtered).expect("added");
    assert_eq!(has(&w), 0);
    let f = crate::unit_facts::add_fact(&mut w, u, &plain).expect("added");
    assert_eq!(has(&w), 1);
    crate::unit_facts::remove(&mut w, u, f);
    assert_eq!(has(&w), 0);
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

fn action_list(actions: Vec<Node>) -> Node {
    Node::Object(Box::new(obj(
        "Kingmaker.ElementsSystem.ActionList",
        &[("Actions", Node::Seq(actions))],
    )))
}

fn apply_buff_action(buff: u128, as_child: bool, ignore_parent: bool) -> Node {
    Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.Mechanics.Actions.ContextActionApplyBuff",
        &[
            ("name", Node::Str("$ContextActionApplyBuff$t".into())),
            ("m_Buff", Node::BpRef(Guid(buff))),
            ("Permanent", Node::Bool(true)),
            ("AsChild", Node::Bool(as_child)),
            ("IgnoreParentContext", Node::Bool(ignore_parent)),
        ],
    )))
}

fn context_actions(activated: Vec<Node>) -> Node {
    Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.Mechanics.Components.AddFactContextActions",
        &[
            ("name", Node::Str("$AddFactContextActions$t".into())),
            ("Activated", action_list(activated)),
            ("Deactivated", action_list(Vec::new())),
            ("NewRound", action_list(Vec::new())),
            ("Dispose", action_list(Vec::new())),
        ],
    )))
}

/// `ContextActionApplyBuff` from a buff's `AddFactContextActions`: the child buff goes on the
/// owner in the parent's cloned context, the parent stores it (`AsChild`), and removing the
/// parent removes it (`Buff.OnRemove`).
#[test]
fn an_applied_child_buff_is_stored_and_leaves_with_its_parent() {
    let (mut w, u) = registry_world();
    insert_bp(&w, 901, BUFF, &[]);
    let parent = insert_bp(
        &w,
        900,
        BUFF,
        &[(
            "Components",
            Node::Seq(vec![context_actions(vec![apply_buff_action(901, true, false)])]),
        )],
    );
    let p = crate::unit_facts::add_fact(&mut w, u, &parent).expect("parent");
    let c = find_fact(&w, u, Guid(901)).expect("child applied");
    assert_eq!(w.fact(p).buff.as_ref().map(|b| b.stored_facts.clone()), Some(vec![c]));
    let ctx = w.fact(c).context.clone().expect("context");
    assert_eq!(ctx.parent_associated_blueprint, Some(Guid(900)), "cloned from the parent's");
    crate::unit_facts::remove(&mut w, u, p);
    assert!(find_fact(&w, u, Guid(901)).is_none(), "the stored child leaves with the parent");
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

/// A feature's `AddFactContextActions` with `IgnoreParentContext`: a context of the owner's own
/// (no parent), and no `Buff.Data` scope to store the buff in.
#[test]
fn a_buff_applied_without_its_parent_context_is_its_own() {
    let (mut w, u) = registry_world();
    insert_bp(&w, 911, BUFF, &[]);
    let f = feature(&w, 910, vec![context_actions(vec![apply_buff_action(911, true, true)])]);
    crate::unit_facts::add_fact(&mut w, u, &f).expect("feature");
    let c = find_fact(&w, u, Guid(911)).expect("applied");
    let ctx = w.fact(c).context.clone().expect("context");
    assert_eq!(ctx.parent_associated_blueprint, None);
    assert_eq!(ctx.associated_blueprint, Some(Guid(911)));
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

/// A duration the action computes from context values isn't modelled: reported, no buff.
#[test]
fn an_apply_buff_with_a_context_duration_is_reported() {
    let (mut w, u) = registry_world();
    insert_bp(&w, 921, BUFF, &[]);
    let timed = Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.Mechanics.Actions.ContextActionApplyBuff",
        &[("name", Node::Str("$A$t".into())), ("m_Buff", Node::BpRef(Guid(921)))],
    )));
    let f = feature(&w, 920, vec![context_actions(vec![timed])]);
    crate::unit_facts::add_fact(&mut w, u, &f).expect("feature");
    assert!(find_fact(&w, u, Guid(921)).is_none());
    assert!(
        w.coverage.unsupported.keys().any(|k| k.contains("ContextActionApplyBuff: DurationValue")),
        "{:?}",
        w.coverage.unsupported
    );
}

/// `BuffExtraEffects`: the checked buff arriving puts the extra buff on; its leaving (no checked
/// buff left) takes the extra one off.
#[test]
fn extra_effects_follow_the_checked_buff() {
    let (mut w, u) = registry_world();
    insert_bp(&w, 931, BUFF, &[]);
    insert_bp(&w, 932, BUFF, &[]);
    let comp = Node::Object(Box::new(obj(
        "Kingmaker.Designers.Mechanics.Facts.BuffExtraEffects",
        &[
            ("name", Node::Str("$BuffExtraEffects$t".into())),
            ("m_CheckedBuff", Node::BpRef(Guid(931))),
            ("m_ExtraEffectBuff", Node::BpRef(Guid(932))),
        ],
    )));
    let f = feature(&w, 930, vec![comp]);
    let ff = crate::unit_facts::add_fact(&mut w, u, &f).expect("feature");
    fl::turn_on(&mut w, ff);
    assert!(find_fact(&w, u, Guid(932)).is_none(), "nothing to extend yet");
    let checked = w.content.get(Guid(931)).expect("bp");
    let b = crate::unit_facts::add_fact(&mut w, u, &checked).expect("checked buff");
    assert!(find_fact(&w, u, Guid(932)).is_some(), "the extra buff follows");
    crate::unit_facts::remove(&mut w, u, b);
    assert!(find_fact(&w, u, Guid(932)).is_none(), "and leaves with it");
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

/// `AddFeatureOnSkill`: its facts are added when the stat's permanent value reaches the minimum.
#[test]
fn skill_features_need_the_minimal_stat() {
    let (mut w, u) = registry_world();
    feature(&w, 941, Vec::new());
    let comp = |min: i64| {
        Node::Object(Box::new(obj(
            "Kingmaker.Designers.Mechanics.Facts.AddFeatureOnSkill",
            &[
                ("name", Node::Str("$AddFeatureOnSkill$t".into())),
                ("StatType", Node::Enum(i64::from(S::Strength.0))),
                ("MinimalStat", Node::Int(min)),
                ("m_Facts", Node::Seq(vec![Node::BpRef(Guid(941))])),
            ],
        )))
    };
    let strength = w.stats(u).v(S::Strength).permanent_value;
    let high = feature(&w, 942, vec![comp(i64::from(strength) + 1)]);
    crate::unit_facts::add_fact(&mut w, u, &high).expect("added");
    assert!(find_fact(&w, u, Guid(941)).is_none());
    let low = feature(&w, 943, vec![comp(i64::from(strength))]);
    let f = crate::unit_facts::add_fact(&mut w, u, &low).expect("added");
    assert!(find_fact(&w, u, Guid(941)).is_some());
    crate::unit_facts::remove(&mut w, u, f);
    assert!(find_fact(&w, u, Guid(941)).is_none());
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

/// `BuffOnEntityCreated`: the buff goes on with activation, its id kept; deactivation removes the
/// fact of that id.
#[test]
fn an_entity_created_buff_is_removed_by_its_id() {
    let (mut w, u) = registry_world();
    insert_bp(&w, 951, BUFF, &[]);
    let comp = Node::Object(Box::new(obj(
        "Kingmaker.Designers.TempMapCode.Ambush.BuffOnEntityCreated",
        &[("name", Node::Str("$BuffOnEntityCreated$t".into())), ("m_Buff", Node::BpRef(Guid(951)))],
    )));
    let f = feature(&w, 950, vec![comp]);
    let ff = crate::unit_facts::add_fact(&mut w, u, &f).expect("added");
    let b = find_fact(&w, u, Guid(951)).expect("buff");
    let c = crate::world::CompRef { fact: ff, comp: 0 };
    assert_eq!(
        w.component(c).data.as_ref().and_then(|d| d["BuffId"].as_str()),
        Some(w.fact(b).unique_id.as_str())
    );
    crate::unit_facts::remove(&mut w, u, ff);
    assert!(find_fact(&w, u, Guid(951)).is_none());
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

/// `AddFeatureOnApply` outside a level-up adds its feature and updates a progression on a new
/// state; inside one whose state its code holds (not lent to the world), it is reported.
#[test]
fn feature_on_apply_needs_the_level_ups_state_when_one_runs() {
    let (mut w, u) = registry_world();
    insert_bp(&w, 961, "Kingmaker.Blueprints.Classes.BlueprintProgression", &[]);
    let comp = Node::Object(Box::new(obj(
        "Kingmaker.Designers.Mechanics.Facts.AddFeatureOnApply",
        &[
            ("name", Node::Str("$AddFeatureOnApply$t".into())),
            ("m_Feature", Node::BpRef(Guid(961))),
        ],
    )));
    let f = feature(&w, 960, vec![comp.clone()]);
    crate::unit_facts::add_fact(&mut w, u, &f).expect("added");
    assert!(find_fact(&w, u, Guid(961)).is_some());
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
    let g = feature(&w, 962, vec![comp]);
    w.level_up.push(crate::spawn::levelup::Slot::Live(None));
    crate::unit_facts::add_fact(&mut w, u, &g).expect("added");
    w.level_up.pop();
    assert!(
        w.coverage.unsupported.keys().any(|k| k.contains("level-up's state isn't reachable")),
        "{:?}",
        w.coverage.unsupported
    );
}

const TOGGLE: &str = "Kingmaker.UnitLogic.ActivatableAbilities.BlueprintActivatableAbility";

/// An `OnlyInCombat` toggle out of combat isn't available, and stopping it turns it off
/// (`Stop`: `!IsAvailableByRestrictions`).
#[test]
fn a_toggle_stopped_while_restricted_turns_off() {
    let (mut w, u) = registry_world();
    insert_bp(&w, 971, BUFF, &[]);
    let t = insert_bp(
        &w,
        970,
        TOGGLE,
        &[("OnlyInCombat", Node::Bool(true)), ("m_Buff", Node::BpRef(Guid(971)))],
    );
    let f = crate::unit_facts::add_fact(&mut w, u, &t).expect("toggle");
    assert!(!crate::unit_facts::is_available(&mut w, f));
    crate::unit_facts::set_is_on(&mut w, f, true);
    assert_eq!(w.fact(f).activatable.map(|a| (a.0, a.1)), Some((true, false)), "on, not started");
    w.fact_mut(f).activatable = Some((true, true, None));
    crate::unit_facts::stop(&mut w, f);
    assert_eq!(w.fact(f).activatable.map(|a| (a.0, a.1)), Some((false, false)));
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

/// A toggle whose resource logic spends on start (not ported) is reported when it starts, unless
/// a fact makes it free.
#[test]
fn a_toggle_spending_a_resource_on_start_is_reported() {
    let (mut w, u) = registry_world();
    insert_bp(&w, 981, BUFF, &[]);
    let free = feature(&w, 982, Vec::new());
    let logic = Node::Object(Box::new(obj(
        "Kingmaker.UnitLogic.ActivatableAbilities.ActivatableAbilityResourceLogic",
        &[
            ("name", Node::Str("$ActivatableAbilityResourceLogic$t".into())),
            ("SpendType", Node::Enum(2)),
            ("m_FreeBlueprint", Node::BpRef(Guid(982))),
        ],
    )));
    let t = insert_bp(
        &w,
        980,
        TOGGLE,
        &[("m_Buff", Node::BpRef(Guid(981))), ("Components", Node::Seq(vec![logic]))],
    );
    crate::unit_facts::add_fact(&mut w, u, &free).expect("free fact");
    let f = crate::unit_facts::add_fact(&mut w, u, &t).expect("toggle");
    w.coverage.unsupported.clear();
    crate::unit_facts::try_start(&mut w, f);
    assert_eq!(w.fact(f).activatable.map(|a| a.1), Some(true), "free: started");
    assert!(
        !w.coverage.unsupported.keys().any(|k| k.contains("OnStart")),
        "{:?}",
        w.coverage.unsupported
    );
}
