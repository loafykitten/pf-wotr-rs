//! The quest book (decompiled, not copied: `Kingmaker.AreaLogic.QuestSystem/QuestBook.cs`,
//! `Quest.cs`, `QuestObjective.cs`). Quests and objectives are facts on the `QuestBook`
//! entity; an objective is active while started. States: `QuestState` (0 None, 1 Started,
//! 2 Completed, 3 Failed), `QuestObjectiveState` (0 None, 1 Started, 2 Completed,
//! 3 Failed).
//!
//! Ported: `GiveObjective`, `CompleteObjective`, `FailObjective`, `GetObjectiveState`,
//! `EnsureObjective` (creating the quest and its objectives), `QuestObjective.Start/
//! Complete/Fail`, `Quest.OnObjectiveStateChange` (quest start, objective order, next
//! objectives, finishing the parent or the quest). Objective components implementing
//! `IQuestObjectiveLogic` / `IQuestObjectiveCallback` and quest components implementing
//! `IQuestLogic` report unsupported when called (none on the meetings' objectives).
//! Kingdom-time objective starts report unsupported.

use std::rc::Rc;

use serde_json::json;
use wr_content::Guid;

use crate::content::Blueprint;
use crate::facts::lifecycle as fl;
use crate::world::{FactExt, FactIdx, FactKind, World};

fn bp(w: &World, g: Guid) -> Option<Rc<Blueprint>> {
    w.content.get(g)
}

/// The quest fact of a quest blueprint (`GetQuest`).
#[must_use]
pub fn get_quest(w: &World, quest: Guid) -> Option<FactIdx> {
    let book = w.player.quest_book?;
    w.entity(book)
        .facts
        .list
        .iter()
        .copied()
        .find(|f| w.fact(*f).clr == "Quest" && w.fact(*f).blueprint == Some(quest))
}

fn objectives(w: &World, q: FactIdx) -> Vec<(Guid, FactIdx)> {
    match &w.fact(q).ext {
        FactExt::Quest { objectives, .. } => objectives.clone(),
        _ => Vec::new(),
    }
}

/// `Quest.TryGetObjective`.
#[must_use]
pub fn try_get_objective(w: &World, q: FactIdx, obj: Guid) -> Option<FactIdx> {
    objectives(w, q).into_iter().find(|(g, _)| *g == obj).map(|(_, f)| f)
}

fn quest_of_objective_bp(w: &World, obj: Guid) -> Option<Guid> {
    bp(w, obj).and_then(|b| b.view().guid("m_Quest"))
}

#[must_use]
pub fn objective_state(w: &World, f: FactIdx) -> i32 {
    match &w.fact(f).ext {
        FactExt::Objective { state, .. } => *state,
        _ => 0,
    }
}

#[must_use]
pub fn quest_state(w: &World, q: FactIdx) -> i32 {
    match &w.fact(q).ext {
        FactExt::Quest { state, .. } => *state,
        _ => 0,
    }
}

fn quest_read_only(w: &World, q: FactIdx) -> bool {
    matches!(&w.fact(q).ext, FactExt::Quest { is_read_only: true, .. })
}

/// `QuestBook.GetObjectiveState(bp)`.
pub fn get_objective_state(w: &mut World, obj: Guid) -> i32 {
    let Some(quest) = quest_of_objective_bp(w, obj) else { return 0 };
    let Some(q) = get_quest(w, quest) else { return 0 };
    match try_get_objective(w, q, obj) {
        Some(f) => objective_state(w, f),
        None => {
            // `throw new Exception("Can't find objective in quest")`.
            w.coverage.unsupported("QuestBook.GetObjectiveState: objective not in quest");
            0
        }
    }
}

/// `QuestBook.EnsureObjective(bp)`.
fn ensure_objective(w: &mut World, obj: Guid) -> Option<FactIdx> {
    let quest = quest_of_objective_bp(w, obj)?;
    let q = match get_quest(w, quest) {
        Some(q) => q,
        None => new_quest(w, quest)?,
    };
    let o = try_get_objective(w, q, obj);
    if o.is_none() {
        w.log("error", "Can't find objective in quest");
    }
    o
}

/// `new Quest(bp)` (objectives for `AllObjectives`) and `Facts.Add(quest)`, whose
/// `OnAttach` adds each objective to the book.
fn new_quest(w: &mut World, quest: Guid) -> Option<FactIdx> {
    let book = w.player.quest_book?;
    let qb = bp(w, quest)?;
    let q = fl::new_fact(w, FactKind::Other, "Quest", qb.clone());
    let mut objs = Vec::new();
    for og in all_objectives(w, &qb) {
        let Some(ob) = bp(w, og) else { continue };
        let is_addendum = ob.view().int("m_Type") != 0;
        let o = fl::new_fact(w, FactKind::Other, "QuestObjective", ob);
        w.fact_mut(o).ext = FactExt::Objective {
            state: 0,
            is_visible: !is_addendum,
            is_collapse: false,
            start_time: 0,
            need_to_attention: false,
            order: 0,
            quest: Some(q),
            parent_objective: None,
        };
        objs.push((og, o));
    }
    w.fact_mut(q).ext = FactExt::Quest {
        state: 0,
        next_objective_order: 1,
        is_read_only: false,
        is_in_ui_selected: false,
        objectives: objs,
    };
    fl::manager_add_attach(w, book, q, false);
    Some(q)
}

/// `BlueprintQuest.AllObjectives`: `m_Objectives` (addendums are listed there too).
fn all_objectives(_w: &World, qb: &Rc<Blueprint>) -> Vec<Guid> {
    qb.view().guids("m_Objectives")
}

/// `Quest.OnAttach`: each objective is added to the book's facts (objectives suppress
/// activation on attach).
pub fn quest_on_attach(w: &mut World, q: FactIdx) {
    let Some(book) = w.player.quest_book else { return };
    for (_, o) in objectives(w, q) {
        fl::manager_add_attach(w, book, o, true);
    }
}

/// `Quest.OnPostLoad`: objectives for blueprint objectives the save lacks are created
/// and added (not ported: reported).
pub fn quest_on_post_load(w: &mut World, q: FactIdx) {
    let Some(qb) = w.fact(q).bp.clone() else { return };
    let have: Vec<Guid> = objectives(w, q).into_iter().map(|(g, _)| g).collect();
    if all_objectives(w, &qb).iter().any(|g| !have.contains(g)) {
        w.coverage.unsupported("Quest.OnPostLoad: create a missing objective");
    }
}

/// `QuestObjective.OnPostLoad`: links its addendums back to it (`ParentObjective`).
pub fn objective_on_post_load(w: &mut World, o: FactIdx) {
    let Some(q) = objective_quest(w, o) else { return };
    let adds = w.fact(o).bp.as_ref().map(|b| b.view().guids("m_Addendums")).unwrap_or_default();
    for a in adds {
        match try_get_objective(w, q, a) {
            Some(af) => {
                if let FactExt::Objective { parent_objective, .. } = &mut w.fact_mut(af).ext {
                    *parent_objective = Some(o);
                }
            }
            None => w.log("error", "Quest Objective has a null addendum"),
        }
    }
}

/// `QuestBook.GiveObjective(bp)`.
pub fn give_objective(w: &mut World, obj: Guid) {
    let Some(o) = ensure_objective(w, obj) else { return };
    let s = objective_state(w, o);
    if s != 1 {
        if s != 0 {
            w.log("warning", "Quest objective has invalid state");
        } else {
            objective_start(w, o);
        }
    }
}

/// `QuestBook.CompleteObjective(bp)`.
pub fn complete_objective(w: &mut World, obj: Guid) {
    let Some(o) = ensure_objective(w, obj) else { return };
    if objective_state(w, o) == 0 {
        objective_start(w, o);
    }
    if objective_state(w, o) != 1 {
        w.log("warning", "Quest objective has invalid state");
    } else {
        objective_finish(w, o, true);
    }
}

/// `QuestBook.FailObjective(bp)`.
pub fn fail_objective(w: &mut World, obj: Guid) {
    let Some(o) = ensure_objective(w, obj) else { return };
    if objective_state(w, o) == 0 {
        objective_start(w, o);
    }
    if objective_state(w, o) != 1 {
        w.log("warning", "Quest objective has invalid state");
    } else {
        objective_finish(w, o, false);
    }
}

fn objective_quest(w: &World, o: FactIdx) -> Option<FactIdx> {
    match &w.fact(o).ext {
        FactExt::Objective { quest, .. } => *quest,
        _ => None,
    }
}

fn set_objective_state(w: &mut World, o: FactIdx, s: i32) {
    if let FactExt::Objective { state, need_to_attention, .. } = &mut w.fact_mut(o).ext {
        *state = s;
        *need_to_attention = true;
    }
    let g = w.fact(o).blueprint.map(|g| g.to_string());
    w.events.mutation("objective", json!({ "objective": g, "state": s }));
}

/// Components of a fact implementing `iface` (`CallComponents<T>`): report unsupported
/// unless ported.
fn call_components(w: &mut World, f: FactIdx, iface: &'static str, method: &'static str) {
    for c in crate::facts::comps(w, f) {
        let Some(ty) = w.component(c).ty() else { continue };
        let d = wr_content::schema::ty(ty);
        if !super::implements(d.short, iface) {
            continue;
        }
        match (d.short, iface, method) {
            // `Kingmaker.Blueprints.Classes.Experience/Experience.cs`: only `OnCompleted`
            // (of an objective or a quest) does anything (`Apply()`).
            ("Experience", "IQuestObjectiveLogic" | "IQuestLogic", "OnCompleted") => {
                w.coverage.executed(format!("Experience.{iface}.{method}"));
                experience_apply(w, c);
            }
            ("Experience", _, _) => w.coverage.executed(format!("Experience.{iface}.{method}")),
            _ => w.coverage.unsupported(format!("{}: {iface}.{method}", d.short)),
        }
    }
}

/// `Experience.Apply(null, 0)`: unless `PlayerGainsNoExp`,
/// `GameHelper.GainExperience(RoundToInt(GetXp(Encounter, CR, Modifier, Count) * 1f))`
/// (`GainType.Quest` for objectives and quests), i.e. the party gains it.
fn experience_apply(w: &mut World, c: crate::world::CompRef) {
    let Some((b, i)) = crate::components::settings_of(w, c) else { return };
    let v = crate::content::View(b.components_array()[i]);
    if v.bool("PlayerGainsNoExp") {
        return;
    }
    #[allow(clippy::cast_possible_truncation)]
    let enc = v.int("Encounter") as i32;
    let count = match v.obj("Count") {
        Some(e) => match crate::elements::get_int(w, &b, e) {
            Ok(n) => Some(n),
            Err(e) => {
                // `CallComponents` doesn't catch: the exception propagates to the caller.
                w.coverage.unsupported(format!("Experience.Apply: Count evaluator failed ({e:?})"));
                return;
            }
        },
        None => None,
    };
    // `Mathf.RoundToInt((float)xp * (1f + extraMod))` with `extraMod` 0: the int goes
    // through a float (exact below 2^24).
    let xp = crate::xp::get_xp(w, enc, v.i32("CR"), v.f32("Modifier"), count);
    let f = crate::csharp::arith::store_f32(
        f64::from(crate::csharp::arith::store_f32(f64::from(xp))) * 1.0,
    );
    crate::xp::gain_experience(w, crate::csharp::arith::round_to_int(f));
}

/// Blueprint components of a blueprint implementing `iface`
/// (`Blueprint.CallComponents<T>`).
fn call_blueprint_components(w: &mut World, b: &Rc<Blueprint>, iface: &'static str) {
    for o in b.components_array() {
        let d = wr_content::schema::ty(o.ty);
        if super::implements(d.short, iface) {
            w.coverage.unsupported(format!("{}: {iface}", d.short));
        }
    }
}

/// `QuestObjective.Start()`.
fn objective_start(w: &mut World, o: FactIdx) {
    let Some(q) = objective_quest(w, o) else { return };
    if quest_read_only(w, q) {
        return;
    }
    if matches!(quest_state(w, q), 2 | 3) {
        return;
    }
    set_objective_state(w, o, 1);
    fl::activate(w, o);
    call_components(w, o, "IQuestObjectiveLogic", "OnStarted");
    crate::rulebook::bus::raise_global(w, "IQuestObjectiveHandler", None);
    on_objective_state_change(w, q, o);
    try_became_visible(w, o);
    let ob = w.fact(o).bp.clone();
    for a in ob.map(|b| b.view().guids("m_Addendums")).unwrap_or_default() {
        if let Some(af) = try_get_objective(w, q, a) {
            if let FactExt::Objective { parent_objective, .. } = &mut w.fact_mut(af).ext {
                *parent_objective = Some(o);
            }
            let auto = bp(w, a).is_some_and(|b| b.view().int("m_Type") == 2);
            if objective_state(w, af) == 0 && auto {
                objective_start(w, af);
            }
            try_became_visible(w, af);
        }
    }
    let kingdom_time = w.fact(o).bp.as_ref().is_some_and(|b| b.view().bool("StartOnKingdomTime"));
    if kingdom_time {
        w.coverage.unsupported("QuestObjective.Start: StartOnKingdomTime");
    }
    let now = w.clock.game_time.ticks();
    if let FactExt::Objective { start_time, .. } = &mut w.fact_mut(o).ext {
        *start_time = now;
    }
}

/// `QuestObjective.Complete()` / `Fail()`.
fn objective_finish(w: &mut World, o: FactIdx, completed: bool) {
    let Some(q) = objective_quest(w, o) else { return };
    if quest_read_only(w, q) || matches!(quest_state(w, q), 2 | 3) {
        return;
    }
    set_objective_state(w, o, if completed { 2 } else { 3 });
    call_components(
        w,
        o,
        "IQuestObjectiveLogic",
        if completed { "OnCompleted" } else { "OnFailed" },
    );
    if w.fact(o).is_active {
        fl::deactivate(w, o);
    } else {
        w.log("error", "EntityFact.Deactivate: is not active");
    }
    crate::rulebook::bus::raise_global(w, "IQuestObjectiveHandler", None);
    on_objective_state_change(w, q, o);
    if let Some(b) = w.fact(o).bp.clone() {
        call_blueprint_components(w, &b, "IQuestObjectiveCallback");
    }
}

/// `QuestObjective.TryBecameVisible()`.
fn try_became_visible(w: &mut World, o: FactIdx) {
    let hidden = w.fact(o).bp.as_ref().is_some_and(|b| b.view().bool("m_Hidden"));
    let (visible, state, parent) = match &w.fact(o).ext {
        FactExt::Objective { is_visible, state, parent_objective, .. } => {
            (*is_visible, *state, *parent_objective)
        }
        _ => return,
    };
    let ready = state != 0 && parent.is_none_or(|p| objective_state(w, p) != 0);
    if !visible && !hidden && ready {
        if let FactExt::Objective { is_visible, .. } = &mut w.fact_mut(o).ext {
            *is_visible = true;
        }
        call_components(w, o, "IQuestObjectiveLogic", "OnBecameVisible");
        crate::rulebook::bus::raise_global(w, "IQuestObjectiveHandler", None);
    }
}

/// `Quest.OnObjectiveStateChange(objective)`.
fn on_objective_state_change(w: &mut World, q: FactIdx, o: FactIdx) {
    if quest_read_only(w, q) {
        return;
    }
    let ostate = objective_state(w, o);
    let is_addendum = w.fact(o).bp.as_ref().is_some_and(|b| b.view().int("m_Type") != 0);
    if quest_state(w, q) == 0 && ostate != 0 && !is_addendum {
        if let FactExt::Quest { state, .. } = &mut w.fact_mut(q).ext {
            *state = 1;
        }
        let g = w.fact(q).blueprint.map(|g| g.to_string());
        w.events.mutation("quest", json!({ "quest": g, "state": 1 }));
        call_components(w, q, "IQuestLogic", "OnStarted");
        crate::rulebook::bus::raise_global(w, "IQuestHandler", None);
    }
    match ostate {
        1 => {
            let order = match &mut w.fact_mut(q).ext {
                FactExt::Quest { next_objective_order, .. } => {
                    let v = *next_objective_order;
                    *next_objective_order = crate::csharp::arith::add(v, 1);
                    v
                }
                _ => 0,
            };
            if let FactExt::Objective { order: ord, .. } = &mut w.fact_mut(o).ext {
                *ord = order;
            }
        }
        2 | 3 => on_objective_finished(w, q, o),
        _ => w.log("error", "Objective can't change state to None"),
    }
}

/// `Quest.OnObjectiveFinished(objective)`.
fn on_objective_finished(w: &mut World, q: FactIdx, o: FactIdx) {
    let completed = objective_state(w, o) == 2;
    let ob = w.fact(o).bp.clone();
    let next = ob.as_ref().map(|b| b.view().guids("m_NextObjectives")).unwrap_or_default();
    let finish_parent = ob.as_ref().is_some_and(|b| b.view().bool("m_FinishParent"));
    if finish_parent && (!completed || next.is_empty()) {
        let parent = match &w.fact(o).ext {
            FactExt::Objective { parent_objective, .. } => *parent_objective,
            _ => None,
        };
        let is_addendum = ob.as_ref().is_some_and(|b| b.view().int("m_Type") != 0);
        if is_addendum && let Some(p) = parent {
            objective_finish(w, p, completed);
        } else {
            on_quest_finished(w, q, completed);
        }
    } else if completed {
        for n in next {
            if let Some(nf) = try_get_objective(w, q, n) {
                // `OnObjectiveOpened`: start if `None` and `CanStart()` (start conditions
                // are components; none on the meetings' objectives).
                if objective_state(w, nf) == 0 {
                    let has_cond = w.fact(nf).bp.as_ref().is_some_and(|b| {
                        b.components_array().iter().any(|c| {
                            super::implements(
                                wr_content::schema::ty(c.ty).short,
                                "IQuestObjectiveStartCondition",
                            )
                        })
                    });
                    if has_cond {
                        w.coverage.unsupported("QuestObjective.CanStart: start condition");
                    }
                    objective_start(w, nf);
                }
            } else {
                w.coverage.unsupported("Quest.GetObjective: next objective not in quest");
            }
        }
    }
}

/// `Quest.OnQuestFinished(completed)`.
fn on_quest_finished(w: &mut World, q: FactIdx, completed: bool) {
    if quest_read_only(w, q) {
        return;
    }
    if let FactExt::Quest { state, .. } = &mut w.fact_mut(q).ext {
        *state = if completed { 2 } else { 3 };
    }
    let g = w.fact(q).blueprint.map(|g| g.to_string());
    w.events.mutation("quest", json!({ "quest": g, "state": if completed { 2 } else { 3 } }));
    call_components(w, q, "IQuestLogic", if completed { "OnCompleted" } else { "OnFailed" });
    crate::rulebook::bus::raise_global(w, "IQuestHandler", None);
    for (_, o) in objectives(w, q) {
        // `TryFailOnQuestFinished`.
        let is_addendum = w.fact(o).bp.as_ref().is_some_and(|b| b.view().int("m_Type") != 0);
        if !is_addendum && objective_state(w, o) == 1 {
            if let FactExt::Objective { state, need_to_attention, .. } = &mut w.fact_mut(o).ext {
                *state = 3;
                *need_to_attention = true;
            }
            call_components(w, o, "IQuestObjectiveLogic", "OnFailed");
            if w.fact(o).is_active {
                fl::deactivate(w, o);
            }
            crate::rulebook::bus::raise_global(w, "IQuestObjectiveHandler", None);
            if let Some(b) = w.fact(o).bp.clone() {
                call_blueprint_components(w, &b, "IQuestObjectiveCallback");
            }
        }
    }
}
