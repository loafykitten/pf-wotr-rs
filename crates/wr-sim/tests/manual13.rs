//! Hydrates Ember's Manual_13 (read-only, from the fixtures backup or the save folder) and
//! checks the result. Ignored by default: it needs the game's `.bbp` and the save.
//! `cargo nextest run -p wr-sim --run-ignored only`.

use std::rc::Rc;

use wr_sim::stats::types::StatType;
use wr_sim::world::EntityIdx;

fn hydrate() -> (wr_sim::world::World, Vec<EntityIdx>) {
    let path = wr_save::report::find_save("Manual_13").expect("Manual_13");
    let save = wr_sim::hydrate::Save::read(&path).expect("read");
    let content = Rc::new(wr_sim::content::Content::open_default().expect("content"));
    let registry = Rc::new(wr_sim::components::registry());
    let (w, _) =
        wr_sim::hydrate::hydrate(content, registry, &save, &wr_sim::hydrate::Options::default())
            .expect("hydrate");
    let units = (0..w.entities.len())
        .map(|i| EntityIdx(u32::try_from(i).unwrap()))
        .filter(|e| w.unit(*e).is_some())
        .collect();
    (w, units)
}

#[test]
#[ignore = "needs the game's blueprint pack and Manual_13"]
fn manual_13_hydrates() {
    let (w, units) = hydrate();
    assert!(w.content.errors.borrow().is_empty());
    let ids: Vec<&str> = units.iter().map(|u| w.entity(*u).unique_id.as_str()).collect();
    for id in ["35fffa0a-06ba-4cba-be40-7914924be190", "3708", "3759", "3830"] {
        assert!(ids.contains(&id), "{id}");
    }
    let mc = w.main_character.expect("main character");
    let s = |t| w.stats(mc).v(t).modified_value;
    // The values the hydrated sim computes (to be confirmed against the oracle).
    assert_eq!(s(StatType::Strength), 16);
    assert_eq!(s(StatType::Dexterity), 15);
    assert_eq!(s(StatType::AC), 18);
    assert_eq!(s(StatType::HitPoints), 13);
    assert_eq!(s(StatType::SaveFortitude), 5);
    assert_eq!(s(StatType::Speed), 20);
    // Every fact is post-loaded; active ones are turned on.
    for f in &w.entity(mc).facts.list {
        let fact = w.fact(*f);
        assert!(fact.post_load_executed);
        assert_eq!(fact.is_active, fact.is_turned_on, "{}", w.fact_label(*f));
    }
    // Everything the main character's facts do at load is ported (the kitsune bite's extra
    // limb was the last gap).
    let gaps = w.coverage.unsupported_by_unit.get(&w.entity(mc).unique_id);
    assert!(gaps.is_none_or(|g| g.is_empty()), "{gaps:?}");
}

#[test]
#[ignore = "needs the game's blueprint pack and Manual_13"]
fn manual_13_hydration_is_deterministic() {
    let (a, ua) = hydrate();
    let (b, ub) = hydrate();
    assert_eq!(wr_sim::sheet::dump(&a, &ua, "x"), wr_sim::sheet::dump(&b, &ub, "x"));
}
