//! Character-sheet dump in WrathOracle's canonical format (`docs/oracle/state-dump-format.md`,
//! `wrath-oracle-state-v1`), for the parts the sim models: `units[]` with `id`,
//! `blueprint`, `class`, `flags` (subset), `hp`, `progression` (levels), `stats[]` with
//! modifiers grouped by descriptor ascending and in list order within a descriptor,
//! `facts[]` in `Facts.List` order with components and saved data, `resources[]`, `body`
//! and `inventory`. Fields the sim doesn't model are left out rather than guessed, and the
//! comparer skips keys missing on our side (listing them).

use serde_json::{Value as J, json};

use crate::stats::ops;
use crate::stats::types::StatType;
use crate::stats::value::StatKind;
use crate::world::{EntityIdx, FactIdx, World};

fn guid(g: Option<wr_content::Guid>) -> J {
    g.map_or(J::Null, |g| J::String(g.to_string()))
}

fn fact_ref(w: &World, f: Option<FactIdx>) -> J {
    match f {
        Some(f) => json!({"$fact": w.fact(f).unique_id, "blueprint": guid(w.fact(f).blueprint)}),
        None => J::Null,
    }
}

fn item_ref(w: &World, e: Option<EntityIdx>) -> J {
    match e {
        Some(e) => {
            json!({"$item": w.entity(e).unique_id, "blueprint": guid(w.item(e).and_then(|i| i.blueprint))})
        }
        None => J::Null,
    }
}

fn stat_class(k: &StatKind) -> &'static str {
    match k {
        StatKind::Plain => "ModifiableValue",
        StatKind::Attribute { .. } => "ModifiableValueAttributeStat",
        StatKind::Skill { .. } => "ModifiableValueSkill",
        StatKind::SavingThrow { .. } => "ModifiableValueSavingThrow",
        StatKind::Dependant { .. } => "ModifiableValueDependant",
        StatKind::Initiative { .. } => "ModifiableValueInitiative",
        StatKind::HitPoints { .. } => "ModifiableValueHitPoints",
        StatKind::TemporaryHitPoints => "ModifiableValueTemporaryHitPoints",
        StatKind::Speed { .. } => "ModifiableValueSpeed",
        StatKind::ArmorClass(_) => "ModifiableValueArmorClass",
    }
}

fn stat_json(w: &World, u: EntityIdx, t: StatType) -> J {
    let s = w.stats(u).v(t);
    let mut keys: Vec<_> = s.modifier_list.keys().collect();
    keys.sort();
    let mut mods = Vec::new();
    for d in keys {
        for &m in &s.modifier_list.get(&d).expect("key").items {
            let md = &w.mods[m.i()];
            mods.push(json!({
                "descriptor": md.desc.0, "descriptor_name": md.desc.name(), "value": md.value,
                "stack_mode": md.stack_mode.0, "stacks": md.stacks(),
                "source": fact_ref(w, md.source), "source_component": md.source_component,
                "item": item_ref(w, md.item_source),
            }));
        }
    }
    let mut extra = serde_json::Map::new();
    extra.insert("NoMorale".into(), s.no_morale.into());
    extra.insert("NoMoraleNoInsight".into(), s.no_morale_no_insight.into());
    match &s.kind {
        StatKind::Attribute { .. } => {
            extra.insert("Bonus".into(), ops::attribute_bonus(w, u, t).into());
            extra.insert(
                "PermanentBonus".into(),
                crate::csharp::arith::sub(s.permanent_value / 2, 5).into(),
            );
        }
        StatKind::Skill { current_base, class_skill, .. } => {
            extra.insert(
                "BaseAttribute".into(),
                json!({"$stat": current_base.0, "owner": w.entity(u).unique_id}),
            );
            extra.insert("ClassSkillFlag".into(), (*class_skill > 0).into());
        }
        StatKind::ArmorClass(ac) => {
            for (k, v) in [
                ("Touch", ac.touch),
                ("FlatFooted", ac.flat_footed),
                ("FlatFootedTouch", ac.flat_footed_touch),
                ("TouchPermanent", ac.touch_permanent),
                ("FlatFootedPermanent", ac.flat_footed_permanent),
                ("FlatFootedTouchPermanent", ac.flat_footed_touch_permanent),
            ] {
                extra.insert(k.into(), v.into());
            }
            extra.insert(
                "BaseAttributeAppliedBonus".into(),
                ac.applied_bonus.map_or(0, |m| w.mods[m.i()].value).into(),
            );
        }
        StatKind::Speed { racial } => {
            extra.insert("Racial".into(), (*racial).into());
        }
        _ => {}
    }
    json!({
        "type": t.0, "type_name": t.name(), "class": stat_class(&s.kind),
        "base": s.base_value, "permanent": s.permanent_value, "modified": s.modified_value,
        "raw": s.modified_value_raw, "extra": extra, "modifiers": mods,
    })
}

fn fact_json(w: &World, index: usize, f: FactIdx) -> J {
    let fact = w.fact(f);
    let comps: Vec<J> = fact
        .components
        .iter()
        .map(|c| json!({"name": c.name, "class": c.saved_clr, "data": c.data}))
        .collect();
    let mut o = json!({
        "index": index, "id": fact.unique_id, "class": fact.clr,
        "blueprint": guid(fact.blueprint), "blueprint_name": fact.bp.as_ref().map(|b| b.name.clone()),
        "rank": crate::components::fact_rank(w, f), "active": fact.is_active,
        "source_fact": fact_ref(w, fact.source_fact), "source_item": item_ref(w, fact.source_item),
        "caster": fact.context.as_ref().and_then(|c| c.caster.clone()).map(|c| json!({"$unit": c})),
        "components": comps,
    });
    if let Some(b) = &fact.buff {
        o["buff"] = json!({
            "start_time": fact.attach_time.0, "end_time": b.end_time, "next_tick_time": b.next_tick_time,
            "tick_time": b.tick_time, "round_number": b.round_number, "is_permanent": b.is_permanent(),
            "is_suppressed": fact.is_suppressed, "is_from_spell": b.is_from_spell,
        });
    }
    o
}

fn item_json(w: &World, e: EntityIdx) -> J {
    let it = w.item(e).cloned().unwrap_or_default();
    let ench: Vec<J> =
        w.entity(e).facts.list.iter().enumerate().map(|(i, f)| fact_json(w, i, *f)).collect();
    json!({
        "id": w.entity(e).unique_id, "blueprint": guid(it.blueprint),
        "blueprint_name": it.blueprint.and_then(|g| w.content.get(g)).map(|b| b.name.clone()),
        "class": w.entity(e).clr, "count": it.count, "is_identified": it.is_identified,
        "charges": it.charges, "enchantments": ench,
    })
}

/// One unit's sheet.
#[must_use]
pub fn unit_json(w: &World, u: EntityIdx) -> J {
    let unit = w.unit(u).expect("unit");
    let max = w.stats(u).v(StatType::HitPoints).modified_value;
    let temp = w.stats(u).v(StatType::TemporaryHitPoints).modified_value;
    let nonlethal = w.stats(u).v(StatType::DamageNonLethal).modified_value;
    let mut types: Vec<StatType> = StatType::ALL.to_vec();
    types.sort();
    let stats: Vec<J> = types.into_iter().map(|t| stat_json(w, u, t)).collect();
    let facts: Vec<J> =
        w.entity(u).facts.list.iter().enumerate().map(|(i, f)| fact_json(w, i, *f)).collect();
    let mut slots = serde_json::Map::new();
    for s in &unit.body.slots {
        slots.insert(
            s.name.clone(),
            json!({"item": s.item.map(|e| item_json(w, e)), "active": s.active}),
        );
    }
    json!({
        "id": w.entity(u).unique_id,
        "blueprint": guid(unit.blueprint),
        "blueprint_name": unit.blueprint.and_then(|g| w.content.get(g)).map(|b| b.name.clone()),
        "original_blueprint": guid(unit.original_blueprint),
        "class": w.entity(u).clr,
        "flags": {"in_game": w.entity(u).is_in_game, "is_main_character": unit.is_main_character,
                  "is_dead": unit.life_state == 2, "is_conscious": unit.life_state == 0,
                  "is_player_faction": unit.is_player_faction, "is_turned_on": w.entity(u).is_turned_on},
        // `UnitState.Prone` (`ProneState`, `Duration` in 100 ns ticks).
        "prone": {"Active": unit.prone_active, "ShouldBeActive": unit.prone_should_be_active,
                  "Duration": unit.prone_duration},
        "faction": guid(unit.faction),
        "original_size": unit.original_size.map(|s| s as i32),
        "hp": {"max": max, "damage": unit.damage, "nonlethal": nonlethal, "temporary": temp,
               "left": crate::csharp::arith::sub(max, unit.damage)},
        "conditions": crate::unit_state::dump_counts(w, u, false),
        "condition_immunities": crate::unit_state::dump_counts(w, u, true),
        "progression": {"character_level": crate::unit::character_level(w, u), "mythic_level": crate::unit::mythic_level(w, u),
                        "Classes": unit.progression.classes.iter().map(|c| json!({"CharacterClass": c.class.to_string(), "Level": c.level})).collect::<Vec<_>>()},
        "stats": stats,
        "facts": facts,
        "resources": crate::resources::dump(w, u),
        "spellbooks": crate::spellbook::dump(w, u),
        "body": {"current_hand_set": unit.body.current_hand_set, "slots": slots},
        "inventory": if unit.has_own_inventory { J::Array(unit.inventory.iter().map(|e| item_json(w, *e)).collect()) } else { J::Null },
    })
}

/// The ids of the items that came from the save (not created during the run), sorted: what
/// [`crate::compare`] keeps strict; any other item id is a fresh `Guid.NewGuid()` on each side.
fn saved_item_ids(w: &World) -> Vec<String> {
    let mut ids: Vec<String> = w
        .entities
        .iter()
        .filter(|e| matches!(&e.kind, crate::world::EntityKind::Item(i) if !i.runtime_created))
        .map(|e| e.unique_id.clone())
        .collect();
    ids.sort();
    ids
}

/// The `AbilityData.UniqueId`s loaded from the save, sorted: what [`crate::compare`] keeps
/// strict in the spellbooks; any other id is a fresh `Guid.NewGuid()` on each side.
fn saved_spell_ids(w: &World) -> Vec<String> {
    let mut ids: Vec<String> = w.saved_spell_ids.iter().cloned().collect();
    ids.sort();
    ids
}

/// The dump: `schema`, `label`, `game` (time, main character) and `units` sorted by id.
#[must_use]
pub fn dump(w: &World, units: &[EntityIdx], label: &str) -> J {
    let mut us: Vec<EntityIdx> = units.to_vec();
    us.sort_by(|a, b| w.entity(*a).unique_id.cmp(&w.entity(*b).unique_id));
    json!({
        "schema": "wrath-oracle-state-v1",
        "producer": "wr-sim",
        "label": label,
        "game": {"game_time": w.clock.game_time.0, "real_time": w.clock.real_time.0,
                 "main_character": w.main_character.map(|e| json!({"$unit": w.entity(e).unique_id}))},
        "units": us.iter().map(|u| unit_json(w, *u)).collect::<Vec<_>>(),
        // `AbilityParams.Empty` now (the oracle's `abilityParamsEmptyNow`); `known` is the sim's.
        "abilityParamsEmptyNow": {"CasterLevel": w.empty_params.caster_level,
                                  "RankBonus": w.empty_params.rank_bonus,
                                  "known": w.empty_params.known},
        "sim": {"saved_item_ids": saved_item_ids(w), "saved_spell_ids": saved_spell_ids(w), "valid": w.valid(), "unsupported": w.coverage.unsupported, "unsupported_by_unit": w.coverage.unsupported_by_unit, "rng_failures": w.rng.failures()},
    })
}
