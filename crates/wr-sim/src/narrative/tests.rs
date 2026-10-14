//! POC-4 elements on synthetic data built from real content types: the etude bracket
//! runtime and its subscriptions, conditions, flags, unit conditions, and the runner's
//! comparison helpers.

use std::rc::Rc;

use serde_json::json;
use wr_content::node::{Node, Object};
use wr_content::{Guid, schema};

use crate::content::{Blueprint, View};
use crate::facts::lifecycle as fl;
use crate::facts::tests::obj;
use crate::rulebook::bus;
use crate::world::{
    Component, Entity, EntityIdx, EntityKind, Fact, FactExt, FactIdx, FactKind, FactsManager, World,
};

fn node(o: Object) -> Node {
    Node::Object(Box::new(o))
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

fn world() -> (World, EntityIdx) {
    let (mut w, u) = crate::stats::tests::world_with_unit();
    w.registry = Rc::new(crate::components::registry());
    w.events.on = true;
    let sys = w.add_entity(Entity {
        unique_id: "etudes".into(),
        clr: "EtudesSystem",
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
    w.player.etudes_system = Some(sys);
    w.player.mode = "Default";
    (w, u)
}

fn actions(list: Vec<Node>) -> Node {
    node(obj("Kingmaker.ElementsSystem.ActionList", &[("Actions", Node::Seq(list))]))
}

fn unlock_flag(flag: u128, value: i64) -> Node {
    node(obj(
        "Kingmaker.Designers.EventConditionActionSystem.Actions.UnlockFlag",
        &[("m_flag", Node::BpRef(Guid(flag))), ("flagValue", Node::Int(value))],
    ))
}

/// A loaded etude fact (saved `IsActive`, runtimes named as the components, the runtime's
/// saved `m_AlreadyProcessedActivation`), on the etudes system.
fn loaded_etude(w: &mut World, bp: &Rc<Blueprint>, active: bool, processed: bool) -> FactIdx {
    let sys = w.player.etudes_system.expect("system");
    let mut f = Fact::blank(FactKind::Etude, "Etude");
    f.blueprint = Some(bp.guid);
    f.bp = Some(bp.clone());
    f.unique_id = format!("e{}", w.facts.len());
    f.is_active = active;
    f.ext =
        FactExt::Etude { is_completed: false, completion_in_progress: false, children: Vec::new() };
    for o in bp.components_array() {
        let mut runtime = serde_json::Map::new();
        runtime.insert("m_AlreadyProcessedActivation".into(), json!(processed));
        f.components.push(Component {
            name: View(o).str("name").unwrap_or_default().to_owned(),
            source: None,
            saved_clr: None,
            data: None,
            is_turned_on: false,
            is_disposed: false,
            subscribed: false,
            fact_refs: std::collections::HashMap::new(),
            runtime,
            need_resume: false,
        });
    }
    let fi = w.add_fact(f);
    w.entity_mut(sys).facts.list.push(fi);
    w.player.etude_roots.push(fi);
    w.player.etudes_data.insert(bp.guid, crate::narrative::EtudeState::Started);
    fi
}

fn load(w: &mut World) {
    let sys = w.player.etudes_system.expect("system");
    fl::manager_restore_owner_link(w, sys);
    fl::manager_post_load(w, sys);
    for f in w.entity(sys).facts.list.clone() {
        fl::apply_post_load_fixes(w, f);
    }
    fl::manager_turn_on(w, sys);
}

const FLAG: u128 = 0xF1A6;

fn play_trigger(w: &World, g: u128, once: bool, on_load: bool) -> Rc<Blueprint> {
    let trigger = obj(
        "Kingmaker.Designers.EventConditionActionSystem.Events.EtudePlayTrigger",
        &[
            ("name", Node::Str("$EtudePlayTrigger$t".into())),
            ("m_Once", Node::Bool(once)),
            ("IsActivateOnLoadArea", Node::Bool(on_load)),
            ("Actions", actions(vec![unlock_flag(FLAG, 7)])),
        ],
    );
    insert(
        w,
        g,
        "Kingmaker.AreaLogic.Etudes.BlueprintEtude",
        &[("Components", Node::Seq(vec![node(trigger)]))],
    )
}

fn flag_writes(w: &World) -> usize {
    w.events
        .records
        .iter()
        .filter(|r| r.fields.get("what").and_then(|v| v.as_str()) == Some("flag_SetFlagValue"))
        .count()
}

#[test]
fn play_trigger_runs_once_per_activation_although_delivered_twice() {
    let (mut w, _) = world();
    let bp = play_trigger(&w, 0xE1, false, false);
    let f = loaded_etude(&mut w, &bp, true, false);
    load(&mut w);
    let c = crate::world::CompRef { fact: f, comp: 0 };
    // The delegate's interfaces through the runtime (proxy), then the runtime's own.
    let l = &w.bus.global["IEtudesUpdateHandler"].list;
    assert_eq!(l.as_slice(), &[Some(bus::Sub::Component(c)), Some(bus::Sub::Runtime(c))]);
    bus::raise_global(&mut w, "IEtudesUpdateHandler", None);
    // Both entries call the delegate (it implements the interface); the second sees
    // `AlreadyProcessedActivation` and does nothing.
    assert_eq!(flag_writes(&w), 1);
    assert_eq!(crate::narrative::flags::value(&w, Guid(FLAG)), 7);
    bus::raise_global(&mut w, "IEtudesUpdateHandler", None);
    assert_eq!(flag_writes(&w), 1);
    // Unlock comes before SetFlagValue, each recorded after its call.
    let whats: Vec<&str> = w
        .events
        .records
        .iter()
        .filter_map(|r| r.fields.get("what").and_then(|v| v.as_str()))
        .filter(|w| w.starts_with("flag_"))
        .collect();
    assert_eq!(whats, vec!["flag_Unlock", "flag_SetFlagValue"]);
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

#[test]
fn play_trigger_on_load_area_resets_and_once_holds() {
    let (mut w, _) = world();
    let bp = play_trigger(&w, 0xE2, true, true);
    loaded_etude(&mut w, &bp, true, false);
    load(&mut w);
    bus::raise_global(&mut w, "IAreaActivationHandler", None);
    assert_eq!(flag_writes(&w), 1);
    // `IsActivateOnLoadArea` clears the processed flag, but `m_Once` with
    // `AlreadyTriggered` stops a second run.
    bus::raise_global(&mut w, "IAreaActivationHandler", None);
    assert_eq!(flag_writes(&w), 1);
}

fn bracket(w: &World, g: u128, ty: &str) -> Rc<Blueprint> {
    let comp = obj(ty, &[("name", Node::Str(format!("$B${g}").into()))]);
    insert(
        w,
        g,
        "Kingmaker.AreaLogic.Etudes.BlueprintEtude",
        &[("Components", Node::Seq(vec![node(comp)]))],
    )
}

#[test]
fn bracket_enters_once_resumes_on_load_and_exits_on_deactivate() {
    let (mut w, _) = world();
    let ty = "Kingmaker.AreaLogic.Etudes.EtudePeacefulZone";
    let fresh = bracket(&w, 0xB1, ty);
    let f = loaded_etude(&mut w, &fresh, true, false);
    let resumed = bracket(&w, 0xB2, ty);
    loaded_etude(&mut w, &resumed, true, true);
    load(&mut w);
    // Post-load: only the etude whose activation was processed resumes.
    assert_eq!(w.player.counters.get("area.Peaceful"), Some(&1));
    // `OnEtudesUpdate` → `MaybeEnter`: the other enters; the resumed one doesn't again.
    bus::raise_global(&mut w, "IEtudesUpdateHandler", None);
    assert_eq!(w.player.counters.get("area.Peaceful"), Some(&2));
    bus::raise_global(&mut w, "IEtudesUpdateHandler", None);
    assert_eq!(w.player.counters.get("area.Peaceful"), Some(&2));
    fl::deactivate(&mut w, f);
    assert_eq!(w.player.counters.get("area.Peaceful"), Some(&1));
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

#[test]
fn unported_bracket_phases_fail_closed_and_presentation_is_stubbed() {
    let (mut w, _) = world();
    let music = bracket(&w, 0xB3, "Kingmaker.AreaLogic.Etudes.EtudeBracketMusic");
    loaded_etude(&mut w, &music, true, false);
    let capital = bracket(&w, 0xB4, "Kingmaker.AreaLogic.Capital.CapitalCompanionLogic");
    loaded_etude(&mut w, &capital, true, false);
    load(&mut w);
    bus::raise_global(&mut w, "IEtudesUpdateHandler", None);
    assert!(w.coverage.stubbed.keys().any(|k| k.starts_with("EtudeBracketMusic.OnEnter")));
    assert!(w.coverage.unsupported.contains_key("CapitalCompanionLogic.OnEnter"));
}

fn check(w: &mut World, c: &Object) -> bool {
    let owner = insert(w, 0xC0, "Kingmaker.AreaLogic.Etudes.BlueprintEtude", &[]);
    crate::elements::check(w, &owner, View(c)).expect("condition")
}

#[test]
fn flag_conditions_follow_unlock_state_and_values() {
    let (mut w, _) = world();
    let f = Guid(FLAG);
    let unlocked = obj(
        "Kingmaker.Designers.EventConditionActionSystem.Conditions.FlagUnlocked",
        &[
            ("m_ConditionFlag", Node::BpRef(f)),
            ("SpecifiedValues", Node::Seq(vec![Node::Int(2), Node::Int(3)])),
        ],
    );
    let range = obj(
        "Kingmaker.Designers.EventConditionActionSystem.Conditions.FlagInRange",
        &[("m_Flag", Node::BpRef(f)), ("MinValue", Node::Int(1)), ("MaxValue", Node::Int(2))],
    );
    assert!(!check(&mut w, &unlocked) && !check(&mut w, &range));
    crate::narrative::flags::unlock(&mut w, f);
    // Unlocked at 0: not one of the specified values, below the range.
    assert!(!check(&mut w, &unlocked) && !check(&mut w, &range));
    crate::narrative::flags::set_value(&mut w, f, 2);
    assert!(check(&mut w, &unlocked) && check(&mut w, &range));
    crate::narrative::flags::set_value(&mut w, f, 3);
    assert!(check(&mut w, &unlocked) && !check(&mut w, &range));
}

#[test]
fn conditions_counters_drive_can_act() {
    let (mut w, u) = world();
    assert!(crate::rules::checks::can_act(&mut w, u));
    // Dazed (9) makes a unit unable; immunity cancels it.
    crate::unit_state::add_condition(&mut w, u, 9, None);
    assert!(!crate::rules::checks::can_act(&mut w, u));
    crate::unit_state::change_immunity(&mut w, u, 9, true);
    assert!(crate::rules::checks::can_act(&mut w, u));
    crate::unit_state::change_immunity(&mut w, u, 9, false);
    crate::unit_state::remove_condition(&mut w, u, 9, false);
    assert!(crate::rules::checks::can_act(&mut w, u));
    // CantAct (28) and Unconscious (31).
    crate::unit_state::add_condition(&mut w, u, 28, None);
    assert!(!crate::rules::checks::can_act(&mut w, u));
    crate::unit_state::remove_condition(&mut w, u, 28, false);
    crate::unit_state::add_condition(&mut w, u, 31, None);
    assert!(!crate::rules::checks::can_act(&mut w, u));
    // In Cutscene mode a unit can always act.
    w.player.mode = "Cutscene";
    assert!(crate::rules::checks::can_act(&mut w, u));
    assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
}

#[test]
fn fatigue_status_effect_adds_and_removes_its_modifiers() {
    use crate::stats::types::StatType as S;
    let (mut w, u) = world();
    let str0 = w.stats(u).v(S::Strength).modified_value;
    crate::unit_state::add_condition(&mut w, u, 3, None);
    crate::unit_state::add_condition(&mut w, u, 3, None);
    // One effect for the condition however often it is added.
    assert_eq!(w.stats(u).v(S::Strength).modified_value, str0 - 2);
    crate::unit_state::remove_condition(&mut w, u, 3, false);
    assert_eq!(w.stats(u).v(S::Strength).modified_value, str0 - 2);
    crate::unit_state::remove_condition(&mut w, u, 3, false);
    assert_eq!(w.stats(u).v(S::Strength).modified_value, str0);
    // Other status-effect conditions are reported.
    crate::unit_state::add_condition(&mut w, u, 23, None);
    assert!(!w.coverage.valid());
}

#[test]
fn alignment_indices_map_to_the_enum_values() {
    use crate::narrative::alignment::enum_value;
    assert_eq!((0..9).map(enum_value).collect::<Vec<_>>(), vec![10, 3, 18, 9, 1, 17, 12, 5, 20]);
    assert_eq!(enum_value(9), -1);
}

#[test]
fn runner_takes_the_tape_from_modelled_sites_and_recorded_answers() {
    let d20 = crate::rng::sites::DICE_D.0;
    let records = vec![
        json!({"k": "begin", "rng_sites": [[1, d20, "sim"], [2, "Assembly-CSharp|X|Y()|#0@1|Range(float,float)", "sim"]]}),
        json!({"k": "draw", "c": "sim", "site": 1, "m": "Range_ii", "a": [1, 21], "v": 9}),
        json!({"k": "draw", "c": "sim", "site": 2, "m": "Range_ff", "a": [0.5, 1.5], "v": 1.2}),
        json!({"k": "draw", "c": "pres", "site": 1, "m": "Range_ii", "a": [1, 21], "v": 3}),
        json!({"k": "input", "step": "move"}),
        json!({"k": "input", "step": "answer:0", "answer": "aa"}),
        json!({"k": "input", "step": "answer:2", "answer": "bb"}),
    ];
    let tape = crate::runner::tape_from_trace(&records);
    assert_eq!(tape.len(), 1);
    assert_eq!(tape[0].value, crate::rng::DrawValue::Int(9));
    let steps: Vec<String> = ["zone:z", "waitdialog", "answer:0", "answer?:0", "wait:30"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(
        crate::runner::steps_with_oracle_answers(&steps, &records).expect("steps"),
        vec!["zone:z", "waitdialog", "answerbp:aa", "answerbp:bb", "wait:30"]
    );
    // `dialogrun` steps take the answers recorded up to their `dialogrun_end`; waits
    // between stay.
    let runs = vec![
        json!({"k": "input", "step": "answer:0", "answer": "aa", "via": "dialogrun"}),
        json!({"k": "input", "step": "dialogrun_end"}),
        json!({"k": "input", "step": "dialogrun_end"}),
    ];
    let steps: Vec<String> = ["dialogrun:x", "wait:120", "dialogrun", "wait:60"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(
        crate::runner::steps_with_oracle_answers(&steps, &runs).expect("steps"),
        vec!["answerbp:aa", "waitnodialog", "wait:120", "waitnodialog", "wait:60"]
    );
}

#[test]
fn narrative_compare_counts_by_category() {
    let a = json!({
        "narrative": {"etudes": [{"blueprint": "e1", "state": 1}], "flags": [{"blueprint": "f", "value": 2}],
                      "etude_facts": [], "quests": [],
                      "dialog": {"ShownCues": ["c1", "c2"], "SelectedAnswers": [], "ShownAnswerLists": [], "ShownDialogs": [], "AnswerChecks": []},
                      "alignment": {"m_Vector": [0.0, -0.66]}},
        "game": {"money": 5, "area": "a", "area_part": "a", "mode_name": "Default", "party": [{"$unit": "u1"}],
                 "party_and_pets": [], "all_characters": []}
    });
    let mut b = a.clone();
    b["narrative"]["flags"][0]["value"] = json!(3);
    b["narrative"]["dialog"]["ShownCues"] = json!(["c1"]);
    let same = crate::narrative::dump::compare(&a, &a, &no_runtime_ids());
    assert!(same.equal(), "{:?}", same.categories);
    // A collection of the wrong type isn't an empty one.
    let mut broken = a.clone();
    broken["narrative"]["etudes"] = json!({});
    assert!(!crate::narrative::dump::compare(&broken, &a, &no_runtime_ids()).equal());
    let d = crate::narrative::dump::compare(&a, &b, &no_runtime_ids());
    assert!(!d.equal());
    assert_eq!(d.categories["flags"].equal, 0);
    assert_eq!(d.categories["dialog_history"].total, 2);
    assert_eq!(d.categories["etude_state"].equal, 1);
}

#[test]
fn schema_has_the_types_the_ports_name() {
    for t in [
        "Kingmaker.Designers.EventConditionActionSystem.Events.EtudePlayTrigger",
        "Kingmaker.AreaLogic.Etudes.EtudePeacefulZone",
        "Kingmaker.AreaLogic.Etudes.EtudeBracketMusic",
    ] {
        let i = schema::find(t).expect(t);
        assert!(crate::components::etudes::is_bracket(i), "{t}");
    }
}

/// No id counts as allocated during a run.
fn no_runtime_ids() -> crate::runner::RuntimeIds {
    crate::runner::RuntimeIds { first_hex: u64::MAX, saved: std::collections::HashSet::new() }
}

/// `UnitAlignment.Initialize(alignment)`: the circle point scaled by 0.66f, with a history
/// holding the initial record and the alignment's own (`OnChanged` without events).
#[test]
fn initialized_alignment_sets_vector_and_history() {
    use crate::narrative::alignment::{UnitAlignment, circle, index_of_enum_value};
    // LawfulGood (10), TrueNeutral (1), ChaoticEvil (20), and no alignment with value 2.
    assert_eq!([10, 1, 20, 2].map(index_of_enum_value), [Some(0), Some(4), Some(8), None]);
    let a = UnitAlignment::initialized(0);
    let c = circle(0);
    #[allow(clippy::cast_possible_truncation)]
    let scaled = |x: f32| (f64::from(x) * f64::from(0.66f32)) as f32;
    assert_eq!(a.vector, [scaled(c[0]), scaled(c[1])]);
    assert_eq!(a.value(), 0);
    let h = a.history.as_ref().unwrap();
    assert_eq!(h.len(), 2);
    assert_eq!((h[0].position, h[0].direction), ([0.0, 0.0], 4));
    assert_eq!((h[1].position, h[1].direction), (a.vector, 0));
    assert_eq!(UnitAlignment::initialized(4).value(), 4);
}
