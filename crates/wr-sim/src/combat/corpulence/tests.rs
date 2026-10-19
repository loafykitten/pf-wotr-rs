use std::rc::Rc;

use super::*;
use crate::scene_spawn::UnitPrefab;
use crate::stats::types::Size;
use crate::world::Part;

const PREFAB: &str = "prefab";

fn inputs(original: Size, size: Size) -> ScaleInputs {
    ScaleInputs {
        original,
        size,
        percent: 0.0,
        step: DEFAULT_SCALE_STEP,
        max_scale: None,
        pet_coeff: 1.0,
    }
}

/// A world with a unit spawned from a prefab of `m_Corpulence` `c`, Medium.
fn world(c: f32) -> (World, EntityIdx) {
    let (mut w, u) = crate::stats::tests::world_with_unit();
    let p = UnitPrefab { corpulence: Some(c), override_present: Some(false), ..Default::default() };
    w.scene.unit_prefabs = Some(Rc::new([(PREFAB.to_owned(), p)].into_iter().collect()));
    let x = w.unit_mut(u).expect("unit");
    x.view_prefab = Some(PREFAB.to_owned());
    x.original_size = Some(Size::Medium);
    (w, u)
}

#[test]
fn the_size_scale_of_a_size_change_steps_by_the_reduce_coefficient() {
    // Enlarge Person (+1 step): 1 / 0.66 in f32, the corpulence 0.5 of the recorded 0.75757575.
    let up = calculate_size_scale(&inputs(Size::Medium, Size::Large));
    assert_eq!(up.to_bits(), (1.0_f32 / 0.66).to_bits());
    assert_eq!(view_corpulence(0.5, up).to_string(), "0.75757575");
    // Two steps down multiply twice, in order.
    let down = calculate_size_scale(&inputs(Size::Medium, Size::Tiny));
    assert_eq!(down.to_bits(), (1.0_f32 * 0.66 * 0.66).to_bits());
    // Equal sizes: 1 (no cap, no reduce step), times the pet coefficient.
    assert_eq!(calculate_size_scale(&inputs(Size::Medium, Size::Medium)), 1.0);
}

#[test]
fn the_scale_percent_adds_when_sizes_are_equal_and_multiplies_otherwise() {
    let mut i = inputs(Size::Medium, Size::Medium);
    i.percent = 0.25;
    assert_eq!(calculate_size_scale(&i), 1.25);
    i.size = Size::Large;
    let step = 1.0_f32 / 0.66;
    assert_eq!(calculate_size_scale(&i).to_bits(), (step * (1.0_f32 + 0.25)).to_bits());
    // A percent of 0.01 or less is ignored.
    i.percent = 0.01;
    assert_eq!(calculate_size_scale(&i).to_bits(), step.to_bits());
}

#[test]
fn the_override_caps_a_changed_size_only_and_sets_the_step() {
    let mut i = inputs(Size::Medium, Size::Huge);
    i.step = 0.5;
    i.max_scale = Some(3.0);
    // 1 / 0.5 / 0.5 = 4, capped at 3.
    assert_eq!(calculate_size_scale(&i), 3.0);
    // The equal-size branch returns before `GetFinalScale`.
    i.size = Size::Medium;
    i.percent = 5.0;
    assert_eq!(calculate_size_scale(&i), 6.0);
    i.pet_coeff = 0.5;
    assert_eq!(calculate_size_scale(&i), 3.0);
}

#[test]
fn the_corpulence_is_at_least_half_a_metre() {
    assert_eq!(view_corpulence(0.3, 1.0), 0.5);
    assert_eq!(view_corpulence(0.9, 1.0), 0.9);
    assert_eq!(view_corpulence(0.5, 0.66), 0.5);
    assert_eq!(view_corpulence(f32::NAN, 1.0), 0.5);
}

#[test]
fn a_unit_gets_its_prefabs_corpulence_times_its_size_scale() {
    let (mut w, u) = world(0.9);
    assert_eq!(corpulence(&mut w, u), Ok(0.9));
    crate::spawn::set_state_size(&mut w, u, Size::Large);
    let want = view_corpulence(0.9, 1.0_f32 / 0.66);
    assert_eq!(corpulence(&mut w, u), Ok(want));
    // Back to the original size: 1 again.
    crate::spawn::set_state_size(&mut w, u, Size::Medium);
    assert_eq!(corpulence(&mut w, u), Ok(0.9));
}

#[test]
fn the_cached_size_scale_stays_until_a_reset_clears_it() {
    let (mut w, u) = world(0.9);
    assert_eq!(corpulence(&mut w, u), Ok(0.9));
    // A size changed without `UpdateSizeModifiers` (no `ResetSizeScale`) leaves the cache stale.
    w.unit_mut(u).expect("unit").state_size = Some(Size::Large);
    assert_eq!(corpulence(&mut w, u), Ok(0.9));
    reset_size_scale(&mut w, u);
    assert_eq!(corpulence(&mut w, u), Ok(view_corpulence(0.9, 1.0_f32 / 0.66)));
    // The cache is filled by the first call after the reset, with the inputs of that time.
    w.unit_mut(u).expect("unit").state_size = Some(Size::Small);
    assert_eq!(corpulence(&mut w, u), Ok(view_corpulence(0.9, 1.0_f32 / 0.66)));
    // A scale percent set without a reset is stale too; `AddUnitScale` resets right after.
    update_scale_percent(&mut w, u, 0.5);
    assert_eq!(corpulence(&mut w, u), Ok(view_corpulence(0.9, 1.0_f32 / 0.66)));
    crate::spawn::update_size_modifiers(&mut w, u);
    let small = 0.66_f32;
    assert_eq!(corpulence(&mut w, u), Ok(view_corpulence(0.9, small * (1.0 + 0.5))));
}

#[test]
fn the_scale_percent_update_ignores_a_change_within_half_a_percent() {
    let (mut w, u) = world(0.9);
    update_scale_percent(&mut w, u, 0.25);
    update_scale_percent(&mut w, u, 0.253);
    assert_eq!(w.combat.corpulence.scale_percent[&u], 0.25);
    update_scale_percent(&mut w, u, 0.26);
    assert_eq!(w.combat.corpulence.scale_percent[&u], 0.26);
    reset_scale_percent(&mut w, u);
    assert_eq!(w.combat.corpulence.scale_percent[&u], 0.0);
}

#[test]
fn a_rider_answers_with_its_mount_and_the_sim_does_not_know_it() {
    let (mut w, u) = world(0.9);
    w.entity_mut(u).parts.push(Part { clr: "UnitPartRider" });
    assert_eq!(corpulence(&mut w, u), Err(Cause::Rider));
    // The saddled unit (the mount) has its own view's corpulence.
    w.entity_mut(u).parts.clear();
    w.entity_mut(u).parts.push(Part { clr: "UnitPartSaddled" });
    assert_eq!(corpulence(&mut w, u), Ok(0.9));
}

#[test]
fn what_the_sim_cannot_establish_is_unknown_with_its_cause() {
    let (mut w, u) = world(0.9);
    w.entity_mut(u).parts.push(Part { clr: "UnitPartDollData" });
    w.unit_mut(u).expect("unit").view_prefab = None;
    assert_eq!(corpulence(&mut w, u), Err(Cause::Doll));
    w.unit_mut(u).expect("unit").view_prefab = Some(PREFAB.to_owned());
    w.entity_mut(u).parts.clear();
    w.combat.vision.dummy.insert(u, None);
    assert_eq!(corpulence(&mut w, u), Err(Cause::DummyView));
    w.combat.vision.dummy.remove(&u);
    // A prefab the table doesn't hold, one without a single corpulence, no table.
    w.unit_mut(u).expect("unit").view_prefab = Some("other".into());
    assert_eq!(corpulence(&mut w, u), Err(Cause::PrefabUnlisted));
    w.unit_mut(u).expect("unit").view_prefab = None;
    assert_eq!(corpulence(&mut w, u), Err(Cause::NoPrefab));
    w.unit_mut(u).expect("unit").custom_prefab_guid = Some(PREFAB.into());
    assert_eq!(corpulence(&mut w, u), Ok(0.9));
    w.unit_mut(u).expect("unit").size_part_from_save = true;
    reset_size_scale(&mut w, u);
    assert_eq!(corpulence(&mut w, u), Err(Cause::Size));
    w.unit_mut(u).expect("unit").size_part_from_save = false;
    w.scene.unit_prefabs = None;
    assert_eq!(corpulence(&mut w, u), Err(Cause::NoTable));
    // An `OverrideSizeScale` the importer found no values for.
    let p =
        UnitPrefab { corpulence: Some(0.9), override_present: Some(true), ..Default::default() };
    w.scene.unit_prefabs = Some(Rc::new([(PREFAB.to_owned(), p)].into_iter().collect()));
    reset_size_scale(&mut w, u);
    assert_eq!(corpulence(&mut w, u), Err(Cause::Override));
}

#[test]
fn a_replaced_view_is_made_again_from_the_units_own_prefab() {
    let (mut w, u) = world(0.9);
    let other =
        UnitPrefab { corpulence: Some(2.0), override_present: Some(false), ..Default::default() };
    let mut table = (**w.scene.unit_prefabs.as_ref().expect("table")).clone();
    table.insert("custom".to_owned(), other);
    w.scene.unit_prefabs = Some(Rc::new(table));
    w.unit_mut(u).expect("unit").custom_prefab_guid = Some("custom".into());
    // Spawned with the first prefab: its view stays until something replaces it.
    assert_eq!(corpulence(&mut w, u), Ok(0.9));
    view_replaced(&mut w, u);
    assert_eq!(corpulence(&mut w, u), Ok(2.0));
}

#[test]
fn a_doll_unit_takes_the_doll_prefab_of_its_gender() {
    use crate::scene_spawn::DollPrefabs;
    let (mut w, u) = world(0.9);
    let doll =
        |c| UnitPrefab { corpulence: Some(c), override_present: Some(false), ..Default::default() };
    w.scene.doll_prefabs = Some(Rc::new(DollPrefabs { male: doll(0.4), female: doll(0.6) }));
    w.entity_mut(u).parts.push(Part { clr: "UnitPartDollData" });
    let x = w.unit_mut(u).expect("unit");
    x.view_prefab = None;
    x.blueprint = None;
    // Not read from a save: which doll isn't known.
    assert_eq!(corpulence(&mut w, u), Err(Cause::Doll));
    let x = w.unit_mut(u).expect("unit");
    x.doll_hydrated = true;
    x.doll_gender_female = Some(true);
    assert_eq!(corpulence(&mut w, u), Ok(0.6));
    reset_size_scale(&mut w, u);
    w.unit_mut(u).expect("unit").doll_gender_female = Some(false);
    // (0.4 is below the minimum.)
    assert_eq!(corpulence(&mut w, u), Ok(0.5));
    // A doll without a race preset: the unit's own prefab.
    w.unit_mut(u).expect("unit").doll_gender_female = None;
    w.unit_mut(u).expect("unit").custom_prefab_guid = Some(PREFAB.into());
    assert_eq!(corpulence(&mut w, u), Ok(0.9));
}

#[test]
fn a_dummy_view_has_the_dummys_corpulence_until_the_real_view_is_made_again() {
    let (mut w, u) = world(0.9);
    let dummy =
        UnitPrefab { corpulence: Some(0.3), override_present: Some(false), ..Default::default() };
    let mut table = (**w.scene.unit_prefabs.as_ref().expect("table")).clone();
    table.insert("dummy".to_owned(), dummy);
    w.scene.unit_prefabs = Some(Rc::new(table));
    assert!(crate::scene_spawn::dummy_prefab(&w).is_none(), "no root in the empty content");
    w.unit_mut(u).expect("unit").custom_prefab_guid = Some(PREFAB.into());
    // The saved flag: the unit's view is the dummy, whose prefab the content can't name here.
    w.combat.vision.dummy.insert(u, Some(true));
    assert_eq!(corpulence(&mut w, u), Ok(0.9), "(a view spawned with its prefab, unless replaced)");
    view_replaced(&mut w, u);
    assert_eq!(corpulence(&mut w, u), Err(Cause::NoPrefab));
    w.combat.vision.dummy.insert(u, Some(false));
    view_replaced(&mut w, u);
    assert_eq!(corpulence(&mut w, u), Ok(0.9));
}

#[test]
fn a_race_set_after_the_size_was_read_moves_the_original_size_and_not_the_state_size() {
    use wr_content::Guid;
    use wr_content::node::Node;
    // `State.Size`'s getter keeps `OriginalSize` as first read; `SetRace` resets the cache and
    // moves `OriginalSize` (`Race.Size`): the scale then follows the difference.
    let (mut w, u) = world(0.9);
    let race = Guid(0x77);
    w.content.insert(crate::content::Blueprint {
        guid: race,
        name: "Small".into(),
        obj: crate::facts::tests::obj(
            "Kingmaker.Blueprints.Classes.BlueprintRace",
            &[("Size", Node::Int(3))],
        ),
        source: crate::content::Source::Bbp,
    });
    assert_eq!(w.unit(u).unwrap().state_size, None);
    assert_eq!(corpulence(&mut w, u), Ok(0.9));
    assert_eq!(w.unit(u).unwrap().state_size, Some(Size::Medium), "latched by the first read");
    crate::spawn::levelup::set_race(&mut w, u, race);
    assert_eq!(crate::spawn::original_size(&w, u), Some(Size::Small));
    assert_eq!(w.unit(u).unwrap().state_size, Some(Size::Medium));
    // Medium against Small: one step up, 1 / 0.66.
    assert_eq!(corpulence(&mut w, u), Ok(view_corpulence(0.9, 1.0_f32 / 0.66)));
}

#[test]
fn original_size_follows_the_current_blueprint_when_it_is_swapped() {
    use wr_content::Guid;
    use wr_content::node::Node;
    // `PretendUnit` swaps `unit.blueprint`; `OriginalSize` is `Race?.Size ?? Blueprint.Size`.
    let (mut w, u) = world(0.9);
    let unit_bp = |g: u128, size: Size| {
        w.content.insert(crate::content::Blueprint {
            guid: Guid(g),
            name: format!("B{g}"),
            obj: crate::facts::tests::obj(
                "Kingmaker.Blueprints.BlueprintUnit",
                &[("Size", Node::Int(size as i64))],
            ),
            source: crate::content::Source::Bbp,
        });
        Guid(g)
    };
    let (medium, large) = (unit_bp(0x61, Size::Medium), unit_bp(0x62, Size::Large));
    w.unit_mut(u).unwrap().blueprint = Some(medium);
    w.unit_mut(u).unwrap().original_size = Some(Size::Medium);
    assert_eq!(corpulence(&mut w, u), Ok(0.9));
    assert_eq!(w.unit(u).unwrap().state_size, Some(Size::Medium));
    w.unit_mut(u).unwrap().blueprint = Some(large);
    assert_eq!(crate::spawn::original_size(&w, u), Some(Size::Large));
    reset_size_scale(&mut w, u);
    // Medium against a Large original: one step down, times 0.66.
    assert_eq!(corpulence(&mut w, u), Ok(view_corpulence(0.9, 0.66)));
    // A race overrides the blueprint's size, and the blueprint comes back when it goes.
    w.unit_mut(u).unwrap().blueprint = Some(medium);
    assert_eq!(crate::spawn::original_size(&w, u), Some(Size::Medium));
}

#[test]
fn a_body_saved_polymorphed_has_the_polymorphs_view() {
    let (mut w, u) = world(0.9);
    w.unit_mut(u).unwrap().body.polymorphed = true;
    assert_eq!(corpulence(&mut w, u), Err(Cause::Polymorph));
}
