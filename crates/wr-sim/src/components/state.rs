//! Components that change unit state other than stat modifiers. Sources (decompiled, not
//! copied): `Kingmaker.UnitLogic.FactLogic/AddProficiencies.cs`,
//! `AddBackgroundWeaponProficiency.cs`, `AddMechanicsFeature.cs` (its `Runtime`),
//! `AddFacts.cs`, `Kingmaker.Blueprints.Classes/AddClassLevels.cs`,
//! `Kingmaker.Designers.Mechanics.Facts/AddFeatureOnClassLevel.cs`,
//! `Kingmaker.Designers.Mechanics.Buffs/OverrideLocoMotion.cs`.

use super::{Port, bits, owner_unit, with_settings};
use crate::registry::{Hook, Registry};
use crate::world::{CompRef, EntityIdx, World};

pub fn register(r: &mut Registry) {
    use Hook::{PostLoad, TurnOff, TurnOn};
    let ports: Vec<Port> = vec![
        Port::new("AddProficiencies", bits(&[TurnOn, TurnOff]), add_proficiencies),
        // `AddBackgroundWeaponProficiency` (with its attack bonus handler) is in
        // `combat::components`.
        // A custom runtime: every hook reaches the port; only `OnTurnOn`/`OnTurnOff` do
        // anything in the C#.
        Port::new("AddMechanicsFeature", super::all_hooks(), add_mechanics_feature),
        Port::new("AllowDyingCondition", super::all_hooks(), allow_dying_condition),
        Port::new("AddImmortality", super::all_hooks(), add_immortality),
        Port::new("AddLocalMapMarker", super::all_hooks(), add_local_map_marker),
        Port::new("OverrideLocoMotion", bits(&[PostLoad]), override_loco_motion),
    ];
    for p in ports {
        r.register(Box::new(p));
    }
    // `OnActivate`/`OnDeactivate`/`HandleUnitGainLevel` are in `levelup`.
    let mut p = Port::new(
        "AddFeatureOnClassLevel",
        bits(&[Hook::PostLoad, Hook::Activate, Hook::Deactivate]),
        add_feature_on_class_level,
    );
    p.handlers = &["IOwnerGainLevelHandler"];
    p.ev = Some(super::levelup::add_feature_on_class_level_event);
    r.register(Box::new(p));
    r.note_inert("PrerequisiteFeature", "level-up prerequisite; read by progression, no runtime");
}

/// `MultiSet<T>.Add` / `Remove` on a proficiency set.
pub(super) fn multiset(set: &mut Vec<(i64, i32)>, v: i64, add: bool) -> bool {
    match set.iter().position(|(k, _)| *k == v) {
        Some(p) if add => {
            set[p].1 += 1;
            true
        }
        Some(p) => {
            set[p].1 -= 1;
            if set[p].1 == 0 {
                set.remove(p);
            }
            true
        }
        None if add => {
            set.push((v, 1));
            true
        }
        None => false,
    }
}

fn add_proficiencies(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((race, armor, weapons)) = with_settings(w, c, |v| {
        let ints = |n: &str| -> Vec<i64> {
            v.seq(n)
                .iter()
                .filter_map(|x| {
                    if let wr_content::Node::Enum(i) | wr_content::Node::Int(i) = x {
                        Some(*i)
                    } else {
                        None
                    }
                })
                .collect()
        };
        (v.guid("m_RaceRestriction"), ints("ArmorProficiencies"), ints("WeaponProficiencies"))
    }) else {
        return;
    };
    let add = match h {
        Hook::TurnOn => true,
        Hook::TurnOff => false,
        _ => return,
    };
    let unit_race = w.unit(u).and_then(|x| x.progression.race);
    if race.is_some() && race != unit_race {
        return;
    }
    let unit = w.unit_mut(u).expect("unit");
    let mut errors = 0;
    for a in armor {
        errors += usize::from(!multiset(&mut unit.proficiencies.armor, a, add));
    }
    for wc in weapons {
        errors += usize::from(!multiset(&mut unit.proficiencies.weapons, wc, add));
    }
    for _ in 0..errors {
        w.log("error", "Has no item in set");
    }
}

/// `AddBackgroundWeaponProficiency.OnTurnOn`/`OnTurnOff`: `Proficiencies.AddBackground` /
/// `RemoveBackground`.
pub(crate) fn add_background_weapon_proficiency(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some(p) = with_settings(w, c, |v| v.int("Proficiency")) else { return };
    let add = matches!(h, Hook::TurnOn);
    if !matches!(h, Hook::TurnOn | Hook::TurnOff) {
        return;
    }
    let unit = w.unit_mut(u).expect("unit");
    multiset(&mut unit.proficiencies.weapons, p, add);
    multiset(&mut unit.proficiencies.background_weapons, p, add);
}

/// `AddMechanicsFeature.MechanicsFeatureType` values with side effects in `OnTurnOn`.
const IMMUNITY_TO_ARMOR_SPEED_PENALTY: i64 = 14;
const IMMUNITY_TO_MEDIUM_ARMOR_SPEED_PENALTY: i64 = 30;

fn add_mechanics_feature(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some(f) = with_settings(w, c, |v| v.int("m_Feature")) else { return };
    let delta = match h {
        Hook::TurnOn => 1,
        Hook::TurnOff => -1,
        _ => return,
    };
    let name = feature_name(f);
    let unit = w.unit_mut(u).expect("unit");
    let e = unit.mechanics_features.entry(f).or_insert(0);
    // `CountableFlag.Retain` / `Release` (release doesn't go below zero).
    *e = (*e + delta).max(0);
    let has_armor = w
        .unit(u)
        .and_then(|x| x.body.slots.iter().find(|s| s.name == "Armor").and_then(|s| s.item))
        .is_some();
    if (f == IMMUNITY_TO_ARMOR_SPEED_PENALTY || f == IMMUNITY_TO_MEDIUM_ARMOR_SPEED_PENALTY)
        && has_armor
    {
        w.coverage.unsupported("AddMechanicsFeature: armor RecalculateStats");
    }
    if matches!(name.as_deref(), Some("FakeEnhancement" | "AdvancedFakeEnhancement")) {
        w.coverage.unsupported("AddMechanicsFeature: body turn off/on for fake enhancement");
    }
    if name.as_deref() == Some("LegendaryHero") && delta > 0 {
        w.coverage.unsupported("AddMechanicsFeature: LegendaryHero experience event");
    }
}

fn retain_feature(w: &mut World, c: CompRef, h: Hook, feature: i64) {
    let Some(u) = owner_unit(w, c) else { return };
    let delta = match h {
        Hook::TurnOn => 1,
        Hook::TurnOff => -1,
        _ => return,
    };
    let e = w.unit_mut(u).expect("unit").mechanics_features.entry(feature).or_insert(0);
    *e = (*e + delta).max(0);
}

/// `AllowDyingCondition.Runtime`: `State.Features.AllowDyingCondition` retain/release.
fn allow_dying_condition(w: &mut World, c: CompRef, h: Hook) {
    retain_feature(w, c, h, 45);
}

/// `AddImmortality.Runtime`: `State.Features.Immortality` retain/release.
fn add_immortality(w: &mut World, c: CompRef, h: Hook) {
    retain_feature(w, c, h, 44);
}

/// `AddLocalMapMarker.Runtime`: adds itself to the UI's local-map marker list (UI model
/// only; nothing in the rules reads it).
fn add_local_map_marker(_w: &mut World, _c: CompRef, _h: Hook) {}

/// The `MechanicsFeatureType` member name for a value, from the content schema's enums.
fn feature_name(v: i64) -> Option<String> {
    wr_content::schema::enums()
        .iter()
        .find(|e| e.name.ends_with("MechanicsFeatureType"))
        .and_then(|e| e.members.iter().find(|(_, x)| *x == v).map(|(n, _)| (*n).to_owned()))
}

/// Whether a unit has any of the named mechanics features (`UnitMechanicFeatures`).
#[must_use]
pub fn has_mechanics_feature(w: &World, u: EntityIdx, names: &[&str]) -> bool {
    w.unit(u).is_some_and(|x| {
        x.mechanics_features
            .iter()
            .any(|(k, n)| *n > 0 && feature_name(*k).is_some_and(|f| names.contains(&f.as_str())))
    })
}

/// `AddFeatureOnClassLevel.OnPostLoad`: if the applied fact is no longer on the owner,
/// dispose it and maybe re-apply (not ported); otherwise nothing.
fn add_feature_on_class_level(w: &mut World, c: CompRef, h: Hook) {
    match h {
        Hook::Activate => return super::levelup::add_feature_on_class_level_apply(w, c),
        Hook::Deactivate => return super::levelup::remove_applied_fact(w, c),
        Hook::PostLoad => {}
        _ => return,
    }
    let Some(u) = owner_unit(w, c) else { return };
    let applied =
        w.component(c).fact_refs.get("AppliedFact").and_then(|v| v.first().copied().flatten());
    if let Some(f) = applied
        && !w.entity(u).facts.list.contains(&f)
    {
        w.coverage.unsupported("AddFeatureOnClassLevel.OnPostLoad: applied fact lost");
    }
}

/// `OverrideLocoMotion.OnPostLoad` → `TryOverride`: swaps the view's locomotion
/// animation set (presentation; the sim's posture state isn't affected).
fn override_loco_motion(_w: &mut World, _c: CompRef, _h: Hook) {}
