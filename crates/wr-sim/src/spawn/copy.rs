//! A level-up preview of a unit: `UnitHelper.CreatePreview` → `Copy(createView: false,
//! preview: true)` (decompiled, not copied: `Kingmaker.UnitLogic/UnitHelper.cs:77–330`,
//! `Game.CreateUnitVacuum`, `UnitProgressionData.CopyFrom` `UnitProgressionData.cs:1082`,
//! `LevelUpPlanUnitHolder.RequestPlan`). The plan unit is a new unit built from the original
//! blueprint (`DoNotCreateItems`, `DoNotCreatePlan`), turned off, given the original's
//! progression, stats and facts, turned on; the levels the original can't take yet are added
//! to it.

use crate::stats::ops;
use crate::stats::value::StatKind;
use crate::world::{EntityIdx, EntityKind, FactIdx, FactKind, World};

use super::BuildFlags;

/// `LevelUpPlanUnitHolder.RequestPlan()`: the owner's plan unit, made on the first request
/// (`None` without the holder part, which exists only while `UnitDescriptor.Initialize` runs).
pub fn request_plan(w: &mut World, u: EntityIdx) -> Option<EntityIdx> {
    let unit = w.unit(u)?;
    if !unit.plan_holder {
        return None;
    }
    if let Some(p) = unit.plan_unit {
        return Some(p);
    }
    let plan = create_preview(w, u)?;
    w.unit_mut(u).expect("unit").plan_unit = Some(plan);
    Some(plan)
}

/// `UnitHelper.CreatePreview(unit, createView: false)` (`Copy`: the level-up controller is
/// stashed for the copy).
pub fn create_preview(w: &mut World, original: EntityIdx) -> Option<EntityIdx> {
    super::levelup::with_controller_stashed(w, |w| create_preview_inner(w, original))
}

fn create_preview_inner(w: &mut World, original: EntityIdx) -> Option<EntityIdx> {
    let bp = w.unit(original).and_then(|u| u.original_blueprint).and_then(|g| w.content.get(g))?;
    // `CopyInternal`: the vacuum unit under `DoNotCreateItems` and `DoNotCreatePlan`.
    let id = w.new_guid();
    let e = w.add_entity(crate::world::Entity {
        unique_id: id,
        clr: "UnitEntityData",
        kind: EntityKind::Unit(Box::default()),
        facts: crate::world::FactsManager::default(),
        parts: Vec::new(),
        is_turned_on: false,
        post_load_executed: false,
        is_in_game: true,
        subscribed: false,
        disposing: false,
        is_disposed: false,
    });
    let outer_ctx = w.coverage.context.clone();
    w.coverage.context = Some(w.entity(e).unique_id.clone());
    let outer = std::mem::replace(
        &mut w.build,
        BuildFlags { do_not_create_items: true, do_not_create_plan: true },
    );
    // `Game.CreateUnitVacuum`: `new UnitEntityData(id, isInGame: true, blueprint)`.
    super::construct(w, e, &bp, None, true);
    w.build = outer;
    w.unit_mut(e).expect("unit").preview_of = Some(original);
    // `unitEntityData.TurnOff()`.
    crate::entity::turn_off(w, e);
    // Name, gender, voice, portrait, doll: presentation. `Alignment.InitializeFrom`: the
    // alignment vector isn't read by the level-up.
    // `EnsureOwnInventory()`: a player unit's shared inventory becomes one of its own.
    {
        w.collection_seq += 1;
        let seq = w.collection_seq;
        let unit = w.unit_mut(e).expect("unit");
        if unit.is_player_faction {
            unit.has_own_inventory = true;
            unit.inventory = Vec::new();
            unit.inventory_collection = Some(seq);
        }
    }
    // `BuffCollection.SetupPreview`: no more buffs; those there are deactivated.
    w.unit_mut(e).expect("unit").buffs_disabled = true;
    for f in w.entity(e).facts.list.clone() {
        if w.fact(f).kind == FactKind::Buff {
            crate::facts::lifecycle::deactivate(w, f);
        }
    }
    copy_progression(w, original, e);
    copy_stats(w, original, e);
    copy_facts(w, original, e);
    crate::spellbook::copy_spellbooks(w, original, e);
    copy_proficiencies(w, original, e);
    // `CopyItems`: the original's equipment slots' items, created again for the copy.
    if w.unit(original).is_some_and(|u| u.body.slots.iter().any(|s| s.item.is_some())) {
        w.coverage.unsupported("UnitHelper.CopyItems: creating the equipment again");
    }
    // `UpdateScalePercent`: presentation. `unitEntityData.TurnOn()`.
    crate::entity::turn_on(w, e);
    w.entity_mut(e).is_in_game = false;
    w.coverage.context = outer_ctx;
    Some(e)
}

/// `UnitProgressionData.CopyFrom(other)`.
fn copy_progression(w: &mut World, from: EntityIdx, to: EntityIdx) {
    let (race, experience, mythic_experience, classes, progressions, selections) = {
        let p = &w.unit(from).expect("unit").progression;
        (
            p.race,
            p.experience,
            p.mythic_experience,
            p.classes.clone(),
            p.progressions.clone(),
            p.selections.clone(),
        )
    };
    if let Some(r) = race {
        super::levelup::set_race(w, to, r);
    }
    w.unit_mut(to).expect("unit").progression.experience = experience;
    w.unit_mut(to).expect("unit").progression.mythic_experience = mythic_experience;
    for cd in &classes {
        let have = w
            .unit(to)
            .and_then(|u| u.progression.classes.iter().find(|c| c.class == cd.class))
            .map_or(0, |c| c.level);
        if have < cd.level {
            w.coverage.unsupported("UnitProgressionData.CopyFrom: adding class levels");
        }
        let target = w.unit_mut(to).expect("unit");
        if let Some(t) = target.progression.classes.iter_mut().find(|c| c.class == cd.class) {
            for a in &cd.archetypes {
                if !t.archetypes.contains(a) {
                    t.archetypes.push(*a);
                }
            }
            t.spellbook = cd.spellbook;
        }
    }
    let target = &mut w.unit_mut(to).expect("unit").progression;
    for pd in progressions {
        match target.progressions.iter_mut().find(|d| d.progression == pd.progression) {
            Some(d) => d.level = pd.level,
            None => target.progressions.push(pd),
        }
    }
    for sd in selections {
        for (level, features) in &sd.by_level {
            for f in features {
                let have = target
                    .selections
                    .iter()
                    .find(|s| s.selection == sd.selection)
                    .and_then(|s| s.by_level.iter().find(|(l, _)| l == level))
                    .is_some_and(|(_, v)| v.contains(f));
                if !have {
                    super::levelup::add_selection_data(
                        &mut target.selections,
                        sd.selection,
                        sd.source,
                        *level,
                        *f,
                    );
                }
            }
        }
    }
    // `CopyLevelPlanFrom`, `m_ClassesOrder`: not read.
}

/// `UnitHelper.CopyStats`.
fn copy_stats(w: &mut World, from: EntityIdx, to: EntityIdx) {
    let types: Vec<_> = w.stats(from).all.iter().map(|s| s.ty).collect();
    for t in types {
        let (base, racial, class_flag) = {
            let s = w.stats(from).v(t);
            let racial: Vec<(i32, crate::stats::types::ModifierDescriptor)> = s
                .modifiers()
                .filter(|m| w.mods[m.i()].desc == crate::stats::types::ModifierDescriptor::Racial)
                .filter(|m| {
                    w.mods[m.i()].source.and_then(|f| w.fact(f).bp.clone()).is_some_and(|b| {
                        crate::content::is_a(b.obj.ty, "BlueprintRace")
                            && b.view().bool("SelectableRaceStat")
                    })
                })
                .map(|m| (w.mods[m.i()].value, w.mods[m.i()].desc))
                .collect();
            let flag = matches!(s.kind, StatKind::Skill { class_skill, .. } if class_skill > 0);
            (s.base_value, racial, flag)
        };
        ops::set_base_value(w, to, t, base);
        let target_has_racial = w
            .stats(to)
            .v(t)
            .modifiers()
            .any(|m| w.mods[m.i()].desc == crate::stats::types::ModifierDescriptor::Racial);
        if !target_has_racial {
            for (value, desc) in racial {
                ops::add_modifier_value(w, to, t, value, desc);
            }
        }
        // `ClassSkill` is `m_ClassSkill || BackgroundSkillSource != null`; `ClassSkillFlag`
        // is the counter alone.
        if class_flag {
            ops::class_skill_retain(w, to, t);
        }
    }
}

/// `UnitHelper.CopyFacts`.
fn copy_facts(w: &mut World, from: EntityIdx, to: EntityIdx) {
    for f in w.entity(from).facts.list.clone() {
        let fact = w.fact(f);
        // `item.MaybeContext?.Root.AssociatedBlueprint is BlueprintItemEnchantment`; `SourceItem`.
        if fact.source_item.is_some()
            || fact
                .context
                .as_ref()
                .and_then(|c| c.associated_blueprint)
                .and_then(|g| w.content.get(g))
                .is_some_and(|b| crate::content::is_a(b.obj.ty, "BlueprintItemEnchantment"))
        {
            continue;
        }
        let (kind, bp_guid, rank) = (fact.kind, fact.blueprint, fact.rank);
        let is_feature = kind == FactKind::Feature;
        let existing: Option<FactIdx> =
            if is_feature {
                w.entity(to).facts.list.iter().copied().find(|x| {
                    w.fact(*x).kind == FactKind::Feature && w.fact(*x).blueprint == bp_guid
                })
            } else {
                None
            };
        let has = w.entity(to).facts.list.iter().any(|x| w.fact(*x).blueprint == bp_guid);
        let lower = existing.is_some_and(|x| w.fact(x).rank < rank);
        if (is_feature && (existing.is_none() || lower)) || !has {
            if lower {
                w.coverage.unsupported("UnitHelper.CopyFacts: raising a feature's rank");
                continue;
            }
            let Some(bp) = bp_guid.and_then(|g| w.content.get(g)) else { continue };
            let parent = w.fact(f).context.as_ref().and_then(|c| c.parent_source_item.as_ref());
            if parent.is_some() {
                w.coverage.unsupported("UnitHelper.CopyFacts: a fact with a parent context");
            }
            let nf = match kind {
                FactKind::Feature | FactKind::Ability | FactKind::ActivatableAbility => {
                    crate::unit_facts::create_fact(w, &bp, to, None)
                }
                FactKind::Buff => {
                    // A preview takes no buffs (`BuffCollection.m_Disabled`).
                    let ctx = crate::unit_facts::context(w, None, Some(to), &bp);
                    crate::unit_facts::add_buff(w, to, &bp, ctx, None);
                    continue;
                }
                _ => {
                    w.coverage.unsupported(format!(
                        "UnitHelper.CopyFacts: {} not on the copy",
                        w.fact(f).clr
                    ));
                    continue;
                }
            };
            if is_feature {
                let src = w.fact(f).source_blueprint;
                w.fact_mut(nf).source_blueprint = src;
                w.fact_mut(nf).rank = rank;
            }
            w.fact_mut(nf).recalculate_context_before_turn_on = true;
            crate::unit_facts::add(w, to, nf);
        }
    }
}

/// `UnitHelper.CopyProficiencies`.
fn copy_proficiencies(w: &mut World, from: EntityIdx, to: EntityIdx) {
    let (armor, weapons) = {
        let p = &w.unit(from).expect("unit").proficiencies;
        (p.armor.clone(), p.weapons.clone())
    };
    let target = &mut w.unit_mut(to).expect("unit").proficiencies;
    for (k, _) in armor {
        if !target.armor.iter().any(|(x, _)| *x == k) {
            target.armor.push((k, 1));
        }
    }
    for (k, _) in weapons {
        if !target.weapons.iter().any(|(x, _)| *x == k) {
            target.weapons.push((k, 1));
        }
    }
}
