//! Synthetic tests of `ModifiableValue` semantics against the C#.

use std::rc::Rc;

use super::modifier::Modifier;
use super::ops::{self, Filter};
use super::types::{ModifierDescriptor as D, StackMode, StatType as S};
use super::value::Stats;
use crate::content::Content;
use crate::registry::Registry;
use crate::unit::Unit;
use crate::world::{Entity, EntityIdx, EntityKind, FactsManager, World};

pub(crate) fn world_with_unit() -> (World, EntityIdx) {
    let mut w = World::new(Rc::new(Content::empty()), Rc::new(Registry::default()));
    let unit = Unit { stats: Stats::fresh(), ..Unit::default() };
    let e = w.add_entity(Entity {
        unique_id: "u".into(),
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
    for t in S::ALL {
        ops::update_value(&mut w, e, t);
    }
    for a in [S::Strength, S::Dexterity, S::Constitution, S::Intelligence, S::Wisdom, S::Charisma] {
        ops::set_base_value(&mut w, e, a, 10);
    }
    (w, e)
}

fn val(w: &World, u: EntityIdx, t: S) -> i32 {
    w.stats(u).v(t).modified_value
}

#[test]
fn same_descriptor_takes_the_best_bonus_and_worst_penalty() {
    let (mut w, u) = world_with_unit();
    let t = S::AdditionalAttackBonus;
    ops::add_modifier_value(&mut w, u, t, 2, D::Enhancement);
    ops::add_modifier_value(&mut w, u, t, 3, D::Enhancement);
    assert_eq!(val(&w, u, t), 3);
    ops::add_modifier_value(&mut w, u, t, -1, D::Enhancement);
    ops::add_modifier_value(&mut w, u, t, -4, D::Enhancement);
    assert_eq!(val(&w, u, t), -1, "best +3 and worst -4 both count");
    // Stackable descriptors sum.
    ops::add_modifier_value(&mut w, u, t, 1, D::Dodge);
    ops::add_modifier_value(&mut w, u, t, 2, D::Dodge);
    assert_eq!(val(&w, u, t), 2);
    // `AddModifier` first removes duplicates (same source, component, value, descriptor):
    // a second unsourced +2 dodge replaces the first instead of stacking.
    ops::add_modifier_value(&mut w, u, t, 2, D::Dodge);
    assert_eq!(val(&w, u, t), 2);
    // ForceStack makes an enhancement stack; ForceNonStack makes a dodge not stack.
    let m = w.add_mod(Modifier {
        stack_mode: StackMode::ForceStack,
        ..Modifier::new(2, D::Enhancement)
    });
    ops::add_modifier(&mut w, u, t, m);
    assert_eq!(val(&w, u, t), 4);
}

#[test]
fn armor_and_encumbrance_penalties_pool_and_only_the_worse_counts() {
    let (mut w, u) = world_with_unit();
    let t = S::SkillStealth;
    let before = val(&w, u, t);
    ops::add_modifier_value(&mut w, u, t, -2, D::Armor);
    ops::add_modifier_value(&mut w, u, t, -3, D::Encumbrance);
    ops::add_modifier_value(&mut w, u, t, -1, D::Armor);
    assert_eq!(val(&w, u, t), before - 3);
    // A positive armor modifier is an ordinary non-stacking bonus.
    ops::add_modifier_value(&mut w, u, t, 4, D::Armor);
    assert_eq!(val(&w, u, t), before - 3 + 4);
}

#[test]
fn helpless_zeroes_an_attribute_and_bonus_truncates() {
    let (mut w, u) = world_with_unit();
    ops::set_base_value(&mut w, u, S::Strength, 7);
    assert_eq!(ops::attribute_bonus(&w, u, S::Strength), -2, "7 / 2 - 5 truncates");
    ops::add_modifier_value(&mut w, u, S::Strength, 0, D::Helpless);
    // `ApplyModifiersFiltered` returns 0, then `MinValue` (1) clamps.
    assert_eq!(w.stats(u).v(S::Strength).modified_value_raw, 0);
    assert_eq!(val(&w, u, S::Strength), 1);
}

#[test]
fn filters_follow_the_csharp_predicates() {
    let (mut w, u) = world_with_unit();
    let t = S::SaveWill;
    ops::add_modifier_value(&mut w, u, t, 2, D::Morale);
    ops::add_modifier_value(&mut w, u, t, 1, D::Insight);
    ops::add_modifier_value(&mut w, u, t, -1, D::Morale);
    let base = w.stats(u).v(t).base_value;
    assert_eq!(ops::apply_modifiers_filtered(&w, u, t, base, Filter::NoMorale), base + 1 - 1);
    assert_eq!(ops::apply_modifiers_filtered(&w, u, t, base, Filter::NoMoraleNoInsight), base - 1);
    // Saves add the attribute bonus to the morale-free values too.
    let s = w.stats(u).v(t);
    assert_eq!(
        s.no_morale,
        (base + 1 - 1 + ops::attribute_bonus(&w, u, S::Wisdom)).min(s.modified_value)
    );
}

#[test]
fn dexterity_changes_flow_to_ac_initiative_saves_and_skills() {
    let (mut w, u) = world_with_unit();
    ops::set_base_value(&mut w, u, S::Dexterity, 16);
    assert_eq!(val(&w, u, S::AC), 13, "10 + Dex bonus +3");
    assert_eq!(val(&w, u, S::Initiative), 3);
    assert_eq!(val(&w, u, S::SaveReflex), 3);
    assert_eq!(val(&w, u, S::SkillMobility), 3);
    // Flat-footed drops the dexterity bonus; touch keeps it.
    if let super::value::StatKind::ArmorClass(ac) = &w.stats(u).v(S::AC).kind {
        assert_eq!(ac.flat_footed, 10);
        assert_eq!(ac.touch, 13);
    } else {
        panic!("AC kind");
    }
    // A max-Dex limiter caps the applied bonus.
    if let super::value::StatKind::ArmorClass(ac) = &mut w.stats_mut(u).m(S::AC).kind {
        ac.limiters = Some(vec![super::value::DexBonusLimiter { value: 1, source: 1, id: 1 }]);
    }
    ops::update_value(&mut w, u, S::AC);
    assert_eq!(val(&w, u, S::AC), 11);
}

#[test]
fn class_skill_adds_three_only_with_ranks() {
    let (mut w, u) = world_with_unit();
    let t = S::SkillAthletics;
    let v0 = val(&w, u, t);
    ops::class_skill_retain(&mut w, u, t);
    assert_eq!(val(&w, u, t), v0, "no ranks, no class bonus");
    ops::set_base_value(&mut w, u, t, 1);
    assert_eq!(val(&w, u, t), v0 + 1 + 3);
    ops::class_skill_retain(&mut w, u, t);
    ops::class_skill_release(&mut w, u, t);
    assert_eq!(val(&w, u, t), v0 + 4, "a counted flag");
}

#[test]
fn persuasion_feeds_the_dependent_checks() {
    let (mut w, u) = world_with_unit();
    ops::set_base_value(&mut w, u, S::SkillPersuasion, 2);
    let p = val(&w, u, S::SkillPersuasion);
    assert_eq!(val(&w, u, S::CheckDiplomacy), p);
    ops::add_modifier_value(&mut w, u, S::CheckDiplomacy, 2, D::Competence);
    assert_eq!(val(&w, u, S::CheckDiplomacy), p + 2);
}

#[test]
fn removing_a_descriptor_reuses_its_dictionary_slot() {
    let (mut w, u) = world_with_unit();
    let t = S::AdditionalDamage;
    ops::add_modifier_value(&mut w, u, t, 1, D::Luck);
    ops::add_modifier_value(&mut w, u, t, 1, D::Sacred);
    ops::add_modifier_value(&mut w, u, t, 1, D::Profane);
    ops::remove_modifiers(&mut w, u, t, D::Luck);
    ops::add_modifier_value(&mut w, u, t, 1, D::Morale);
    let keys: Vec<D> = w.stats(u).v(t).modifier_list.keys().collect();
    assert_eq!(
        keys,
        vec![D::Morale, D::Sacred, D::Profane],
        "Mono's Dictionary reuses the freed slot"
    );
}

#[test]
fn remove_takes_the_first_equal_modifier() {
    let (mut w, u) = world_with_unit();
    let t = S::AdditionalCMB;
    let a = ops::add_modifier_value(&mut w, u, t, 2, D::Trait);
    let b = w.add_mod(Modifier::new(2, D::Trait));
    // A value-equal but distinct modifier (not yet applied: AppliedTo differs) doesn't
    // match; once its AppliedTo matches, it does and removes `a`'s entry.
    assert!(!ops::remove_modifier(&mut w, u, t, b));
    w.mods[b.i()].applied_to = Some((u, t));
    assert!(ops::remove_modifier(&mut w, u, t, b));
    assert!(w.stats(u).v(t).modifier_list.get(&D::Trait).is_some_and(|l| l.items.is_empty()));
    // `a` still believes it is applied: the C# only clears the argument's AppliedTo.
    assert!(w.mods[a.i()].applied_to.is_some());
    assert!(w.mods[b.i()].applied_to.is_none());
}

#[test]
fn integer_overflow_wraps() {
    let (mut w, u) = world_with_unit();
    let t = S::Reach;
    ops::add_modifier_value(&mut w, u, t, i32::MAX, D::None);
    ops::add_modifier_value(&mut w, u, t, 1, D::UntypedStackable);
    assert_eq!(val(&w, u, t), i32::MIN);
}

#[test]
fn modifier_lists_keep_monos_sort_order() {
    let (mut w, u) = world_with_unit();
    let t = S::AdditionalDamage;
    // Three equal values: the third insert sorts a 3-element list, which Mono's
    // ArraySortHelper reorders ([a, b, c] all equal stay; then a smaller one moves).
    // Distinct sources, so `RemoveDuplicates` keeps both equal values.
    let fa = w.add_fact(crate::world::Fact::blank(crate::world::FactKind::UnitFact, "UnitFact"));
    let fb = w.add_fact(crate::world::Fact::blank(crate::world::FactKind::UnitFact, "UnitFact"));
    let a = ops::add_modifier_from_fact(&mut w, u, t, 1, Some(fa), D::None);
    let b = ops::add_modifier_from_fact(&mut w, u, t, 1, Some(fb), D::None);
    let c = ops::add_modifier_value(&mut w, u, t, 0, D::None);
    let items = w.stats(u).v(t).modifier_list.get(&D::None).unwrap().items.clone();
    // [a, b, c] with values [1, 1, 0]: swap(0,1) no; swap(0,2) yes → [c, b, a]; swap(1,2) no.
    assert_eq!(items, vec![c, b, a]);
}

#[test]
fn cleanup_removes_every_entry_of_a_sourced_duplicate_group() {
    let (mut w, u) = world_with_unit();
    let f = w.add_fact(crate::world::Fact::blank(crate::world::FactKind::UnitFact, "UnitFact"));
    w.fact_mut(f).is_active = true;
    w.entity_mut(u).facts.list.push(f);
    let t = S::AdditionalDamage;
    // Three equal sourced modifiers, as a save could hold them (bypassing AddModifier).
    for _ in 0..3 {
        let m = w.add_mod(Modifier {
            source: Some(f),
            applied_to: Some((u, t)),
            ..Modifier::new(2, D::Luck)
        });
        w.stats_mut(u).m(t).modifier_list.get_or_insert_with(D::Luck, Default::default).push(m);
    }
    ops::cleanup_modifiers(&mut w, u, t);
    assert!(w.stats(u).v(t).modifier_list.get(&D::Luck).unwrap().items.is_empty());
}
