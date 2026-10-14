//! The etude system (decompiled, not copied: `Kingmaker.AreaLogic.Etudes/EtudesSystem.cs`,
//! `Etude.cs`, `EtudesTree.cs`, `BlueprintEtude.cs`, `Kingmaker.Controllers/
//! EtudeSystemController.cs`).
//!
//! An etude has a saved state (`m_EtudesData`: Started, Completed, PreStarted,
//! PreCompleted) and, while started, an `Etude` fact on the `EtudesSystem` entity. A fact
//! being attached ("started") is distinct from it being active ("playing"): `StartEtude`
//! attaches with activation suppressed, and only `UpdateEtudes` (run by
//! `EtudeSystemController` on a tick when conditions are dirty) activates the etudes whose
//! area, campaign and activation conditions allow it, then raises `IEtudesUpdateHandler`
//! to every subscriber (etude brackets enter there).
//!
//! Sets the C# keeps as `HashSet<Etude>` (`m_StartingSet`, `m_StoppingSet`) iterate in
//! Mono's slot order, kept here with [`CsDict`].

use std::rc::Rc;

use serde_json::json;
use wr_content::Guid;

use super::EtudeState;
use crate::content::Blueprint;
use crate::csharp::dict::CsDict;
use crate::facts::lifecycle as fl;
use crate::world::{FactExt, FactIdx, FactKind, World};

fn bp(w: &World, g: Guid) -> Option<Rc<Blueprint>> {
    w.content.get(g)
}

fn parent_of(w: &World, g: Guid) -> Option<Guid> {
    bp(w, g).and_then(|b| b.view().guid("m_Parent"))
}

fn saved_state(w: &World, g: Guid) -> EtudeState {
    w.player.etudes_data.get(&g).copied().unwrap_or(EtudeState::Unknown)
}

/// `Etudes.GetFact(bp)`: the etude fact of this blueprint on the etudes system, if any.
#[must_use]
pub fn get_fact(w: &World, g: Guid) -> Option<FactIdx> {
    let sys = w.player.etudes_system?;
    w.entity(sys).facts.list.iter().copied().find(|f| w.fact(*f).blueprint == Some(g))
}

fn etude_flags(w: &World, f: FactIdx) -> (bool, bool) {
    match &w.fact(f).ext {
        FactExt::Etude { is_completed, completion_in_progress, .. } => {
            (*is_completed, *completion_in_progress)
        }
        _ => (false, false),
    }
}

fn set_flags(w: &mut World, f: FactIdx, completed: Option<bool>, in_progress: Option<bool>) {
    if let FactExt::Etude { is_completed, completion_in_progress, .. } = &mut w.fact_mut(f).ext {
        if let Some(c) = completed {
            *is_completed = c;
        }
        if let Some(p) = in_progress {
            *completion_in_progress = p;
        }
    }
}

fn children(w: &World, f: FactIdx) -> Vec<FactIdx> {
    match &w.fact(f).ext {
        FactExt::Etude { children, .. } => children.clone(),
        _ => Vec::new(),
    }
}

/// `BlueprintEtude.IsReadOnly`: linked campaigns that don't include the player's, else
/// the parent's.
#[must_use]
pub fn is_read_only(w: &World, g: Guid) -> bool {
    let Some(b) = bp(w, g) else { return false };
    let linked = b.view().guids("m_LinkedCampaigns");
    if linked.is_empty() {
        return b.view().guid("m_Parent").is_some_and(|p| is_read_only(w, p));
    }
    let campaign = player_campaign(w);
    !linked.iter().any(|c| Some(*c) == campaign)
}

/// `Player.Campaign`: `StartPreset.Campaign`.
#[must_use]
pub fn player_campaign(w: &World) -> Option<Guid> {
    w.player.start_preset.and_then(|g| bp(w, g)).and_then(|b| b.view().guid("m_Campaign"))
}

pub fn etude_is_not_started(w: &World, g: Guid) -> bool {
    !w.player.etudes_data.contains_key(&g)
}

pub fn etude_is_completed(w: &World, g: Guid) -> bool {
    matches!(saved_state(w, g), EtudeState::Completed | EtudeState::PreCompleted)
        && w.player.etudes_data.contains_key(&g)
}

fn set_state(w: &mut World, g: Guid, s: EtudeState) {
    w.player.etudes_data.insert(g, s);
}

/// `EtudesSystem.StartEtude(bp, startParent, force)`. The trace record is written after
/// the call, as WrathOracle's postfix does (nested starts come first).
pub fn start_etude(w: &mut World, g: Guid, start_parent: bool, force: bool) {
    start_etude_body(w, g, start_parent, force);
    w.events.mutation("etude_StartEtude", json!({ "etude": g.to_string() }));
}

fn start_etude_body(w: &mut World, g: Guid, start_parent: bool, force: bool) {
    if is_read_only(w, g) && !force {
        return;
    }
    match saved_state(w, g) {
        EtudeState::Started | EtudeState::Completed => return,
        _ => {}
    }
    if let Some(p) = parent_of(w, g) {
        match saved_state(w, p) {
            EtudeState::Unknown | EtudeState::PreStarted => {
                let starts_parent = bp(w, g).is_some_and(|b| b.view().bool("m_StartsParent"));
                if starts_parent || start_parent {
                    start_etude(w, p, start_parent, force);
                    if saved_state(w, g) == EtudeState::Started {
                        return;
                    }
                }
                if saved_state(w, p) != EtudeState::Started {
                    set_state(w, g, EtudeState::PreStarted);
                    w.events.mutation(
                        "etude_state",
                        json!({ "etude": g.to_string(), "state": "PreStarted" }),
                    );
                    return;
                }
            }
            EtudeState::Completed | EtudeState::PreCompleted => return,
            EtudeState::Started => {}
        }
    }
    let mut starting: Vec<Guid> = Vec::new();
    start_etude_internal(w, g, &mut starting);
}

fn start_etude_internal(w: &mut World, g: Guid, starting: &mut Vec<Guid>) {
    let saved = saved_state(w, g);
    if saved == EtudeState::Completed {
        return;
    }
    if starting.contains(&g) {
        w.log("error", "Starting etude: loop in StartsWith detected");
        return;
    }
    starting.push(g);
    if get_fact(w, g).is_some() {
        w.log("error", "Cannot start etude: already started");
        return;
    }
    let parent = parent_of(w, g).and_then(|p| get_fact(w, p));
    if let Some(pf) = parent
        && etude_flags(w, pf).1
    {
        return;
    }
    let Some(b) = bp(w, g) else {
        w.coverage.unsupported("StartEtude: etude blueprint missing");
        return;
    };
    let Some(sys) = w.player.etudes_system else {
        w.coverage.unsupported("StartEtude: no EtudesSystem");
        return;
    };
    // `Facts.Add(new Etude(bp, parent))`: `EtudesTree.PrepareFactForAttach` suppresses
    // activation; `OnFactDidAttach` links it into the tree.
    let f = fl::new_fact(w, FactKind::Etude, "Etude", b);
    w.fact_mut(f).ext =
        FactExt::Etude { is_completed: false, completion_in_progress: false, children: Vec::new() };
    w.fact_mut(f).source_fact = parent;
    // `Facts.Add`: attach, then `DelegateOnFactDidAttach` (`EtudesTree.OnFactDidAttach`
    // links it), then WrathOracle's `fact_attach` record.
    fl::manager_add_attach_with(w, sys, f, true, link_to_tree);
    if saved == EtudeState::PreCompleted {
        mark_etude_completed(w, g, false, true);
        return;
    }
    set_state(w, g, EtudeState::Started);
    let starts_with = bp(w, g).map(|b| b.view().guids("m_StartsWith")).unwrap_or_default();
    for s in starts_with {
        start_etude_internal(w, s, starting);
    }
    let pre: Vec<Guid> = w
        .player
        .etudes_data
        .iter()
        .filter(|(k, v)| **v == EtudeState::PreStarted && parent_of(w, **k) == Some(g))
        .map(|(k, _)| *k)
        .collect();
    for k in pre {
        start_etude_internal(w, k, starting);
    }
    w.player.conditions_dirty = true;
}

/// `EtudesTree.LinkEtudeToTree`.
fn link_to_tree(w: &mut World, f: FactIdx) {
    match w.fact(f).source_fact {
        Some(p) => {
            if let FactExt::Etude { children, .. } = &mut w.fact_mut(p).ext
                && !children.contains(&f)
            {
                children.push(f);
            }
        }
        None => {
            if !w.player.etude_roots.contains(&f) {
                w.player.etude_roots.push(f);
            }
        }
    }
}

/// `EtudesTree.RestoreTreeStructure()` (on load).
pub fn restore_tree_structure(w: &mut World) {
    let Some(sys) = w.player.etudes_system else { return };
    for f in w.entity(sys).facts.list.clone() {
        link_to_tree(w, f);
    }
}

/// `EtudesSystem.OnApplyPostLoadFixes`: `FixupActorChanges`, `EtudesTree.FixupEtudesTree`,
/// `FixupCompletesParent`.
pub fn apply_post_load_fixes(w: &mut World) {
    let Some(sys) = w.player.etudes_system else { return };
    // `FixupActorChanges`: held groups aren't saved, so each playing etude with
    // conflicting groups takes its free groups (higher-priority conflicts would schedule
    // force stops: reported).
    for f in w.entity(sys).facts.list.clone() {
        if !w.fact(f).is_active {
            continue;
        }
        let g = w.fact(f).blueprint.expect("etude");
        for grp in conflicting_groups(w, f) {
            match w.player.held_conflicting_groups.get(&grp).copied() {
                None => {
                    w.player.held_conflicting_groups.insert(grp, g);
                }
                Some(h) if h == g => {}
                Some(_) => {
                    w.coverage.unsupported("EtudesSystem.FixupActorChanges: priority conflict")
                }
            }
        }
    }
    // `FixupEtudesTree`: facts whose blueprint parent changed (reported), then StartsWith
    // etudes of the tree that aren't started.
    for f in w.entity(sys).facts.list.clone() {
        let g = w.fact(f).blueprint.expect("etude");
        let want = parent_of(w, g);
        let have = w.fact(f).source_fact.and_then(|p| w.fact(p).blueprint);
        if want != have {
            w.coverage.unsupported("EtudesTree.FixupEtudeParents");
        }
    }
    let mut to_start = Vec::new();
    for r in w.player.etude_roots.clone() {
        fixup_starts_with(w, r, &mut to_start);
    }
    for g in to_start {
        if etude_is_not_started(w, g) {
            start_etude(w, g, false, false);
        }
    }
    // `FixupCompletesParent`.
    let completed: Vec<Guid> = w
        .player
        .etudes_data
        .iter()
        .filter(|(_, v)| **v == EtudeState::Completed)
        .map(|(k, _)| *k)
        .collect();
    for g in completed {
        if bp(w, g).is_some_and(|b| b.view().bool("m_CompletesParent"))
            && let Some(p) = parent_of(w, g).and_then(|p| get_fact(w, p))
            && !etude_flags(w, p).1
        {
            let pg = w.fact(p).blueprint.expect("etude");
            mark_etude_completed(w, pg, false, true);
        }
    }
}

fn fixup_starts_with(w: &mut World, f: FactIdx, out: &mut Vec<Guid>) {
    let sw = w.fact(f).bp.as_ref().map(|b| b.view().guids("m_StartsWith")).unwrap_or_default();
    for g in sw {
        if !etude_is_completed(w, g) && get_fact(w, g).is_none() {
            out.push(g);
        }
    }
    for c in children(w, f) {
        fixup_starts_with(w, c, out);
    }
}

/// `EtudesSystem.MarkEtudeCompleted(bp, ignoreParent, force)` (trace record after the call).
pub fn mark_etude_completed(w: &mut World, g: Guid, ignore_parent: bool, force: bool) {
    mark_etude_completed_body(w, g, ignore_parent, force);
    w.events.mutation("etude_MarkEtudeCompleted", json!({ "etude": g.to_string() }));
}

fn mark_etude_completed_body(w: &mut World, g: Guid, ignore_parent: bool, force: bool) {
    let fact = get_fact(w, g);
    set_state(w, g, if fact.is_none() { EtudeState::PreCompleted } else { EtudeState::Completed });
    let completes_parent = bp(w, g).is_some_and(|b| b.view().bool("m_CompletesParent"));
    if !ignore_parent
        && completes_parent
        && let Some(p) = parent_of(w, g)
    {
        mark_etude_completed(w, p, false, force);
        return;
    }
    if let Some(f) = fact {
        etude_mark_completed(w, f, force);
    }
    w.player.conditions_dirty = true;
}

/// `Etude.MarkCompleted(force)`.
fn etude_mark_completed(w: &mut World, f: FactIdx, force: bool) {
    let g = w.fact(f).blueprint.expect("etude blueprint");
    if is_read_only(w, g) && !force {
        return;
    }
    if etude_flags(w, f).1 {
        return;
    }
    set_flags(w, f, None, Some(true));
    for c in children(w, f) {
        etude_mark_completed(w, c, force);
    }
}

/// `Etude.FinishCompletion(force)`.
fn finish_completion(w: &mut World, f: FactIdx) {
    let (completed, in_progress) = etude_flags(w, f);
    if !in_progress || w.fact(f).is_active || completed {
        w.log("error", "Etude.FinishCompletion: invalid state");
        return;
    }
    set_flags(w, f, Some(true), None);
}

/// `Etude.OnActivate` (called from the fact lifecycle).
pub fn etude_on_activate(w: &mut World, f: FactIdx) {
    w.player.conditions_dirty = true;
    add_to_playing(w, f);
    let (c, p) = etude_flags(w, f);
    if c || p {
        w.log("error", "Cannot activate etude: already completed");
        fl::deactivate(w, f);
    }
    let g = w.fact(f).blueprint.map(|g| g.to_string());
    w.events.mutation("etude_playing", json!({ "etude": g }));
}

/// `Etude.OnDeactivate`.
pub fn etude_on_deactivate(w: &mut World, f: FactIdx) {
    w.player.conditions_dirty = true;
    remove_from_playing(w, f);
    let g = w.fact(f).blueprint.map(|g| g.to_string());
    w.events.mutation("etude_stopping", json!({ "etude": g }));
}

fn conflicting_groups(w: &World, f: FactIdx) -> Vec<Guid> {
    w.fact(f).bp.as_ref().map(|b| b.view().guids("m_ConflictingGroups")).unwrap_or_default()
}

/// `EtudesTree.AddToPlaying`.
fn add_to_playing(w: &mut World, f: FactIdx) {
    let g = w.fact(f).blueprint.expect("etude");
    for grp in conflicting_groups(w, f) {
        w.player.held_conflicting_groups.insert(grp, g);
    }
}

/// `EtudesTree.RemoveFromPlaying`.
fn remove_from_playing(w: &mut World, f: FactIdx) {
    let g = w.fact(f).blueprint.expect("etude");
    for grp in conflicting_groups(w, f) {
        if w.player.held_conflicting_groups.get(&grp) == Some(&g) {
            w.player.held_conflicting_groups.remove(&grp);
        }
    }
}

/// `EtudeSystemController.Tick`: `UpdateEtudes` when conditions are dirty.
pub fn controller_tick(w: &mut World) {
    if w.player.conditions_dirty {
        update_etudes(w);
    }
}

/// `EtudesSystem.UpdateEtudes()`.
pub fn update_etudes(w: &mut World) {
    if w.player.etude_update_guard {
        return;
    }
    w.player.etude_update_guard = true;
    w.player.conditions_dirty = false;
    let before = active_additional_mechanics(w);
    maybe_deactivate_completed_etudes(w);
    select_playing_etudes(w);
    let after = active_additional_mechanics(w);
    if after.iter().any(|m| !before.contains(m)) {
        // `Game.Instance.ReloadAreaMechanic` then the update event after loading.
        w.coverage.unsupported("EtudesSystem.UpdateEtudes: area mechanics reload");
    }
    w.events.push("etudes_update", json!({}));
    crate::rulebook::bus::raise_global(w, "IEtudesUpdateHandler", None);
    crate::scene_parts::update_all_visibilities(w);
    w.player.etude_update_guard = false;
}

/// `GetActiveAdditionalMechanics(CurrentlyLoadedArea)`.
#[must_use]
pub fn active_additional_mechanics(w: &World) -> Vec<Guid> {
    let Some(sys) = w.player.etudes_system else { return Vec::new() };
    let mut out = Vec::new();
    for f in &w.entity(sys).facts.list {
        let fact = w.fact(*f);
        if !fact.is_active {
            continue;
        }
        let Some(b) = &fact.bp else { continue };
        for m in b.view().guids("m_AddedAreaMechanics") {
            let area = bp(w, m).and_then(|mb| mb.view().guid("Area"));
            if area.is_some() && area == w.player.area {
                out.push(m);
            }
        }
    }
    out
}

/// `EtudesTree.MaybeDeactivateCompletedEtudes()`.
fn maybe_deactivate_completed_etudes(w: &mut World) {
    let mut need_cleanup = false;
    for r in w.player.etude_roots.clone() {
        maybe_deactivate_completed(w, r, &mut need_cleanup);
    }
    if !need_cleanup {
        return;
    }
    let Some(sys) = w.player.etudes_system else { return };
    for f in w.entity(sys).facts.list.clone() {
        if etude_flags(w, f).0 {
            remove_etude_fact(w, f);
            let g = w.fact(f).blueprint.expect("etude");
            set_state(w, g, EtudeState::Completed);
        }
    }
}

fn maybe_deactivate_completed(w: &mut World, f: FactIdx, need_cleanup: &mut bool) {
    if etude_flags(w, f).0 {
        return;
    }
    let mut flag = etude_flags(w, f).1;
    for c in children(w, f) {
        maybe_deactivate_completed(w, c, need_cleanup);
        flag &= etude_flags(w, c).0;
    }
    let flag2 = !w.fact(f).is_active || check_condition_field(w, f, "CompletionCondition");
    if !(flag && flag2) {
        return;
    }
    // `CallComponents<IEtudeCompleteTrigger>(OnComplete)`.
    call_complete_triggers(w, f);
    if w.fact(f).is_active {
        fl::deactivate(w, f);
    }
    finish_completion(w, f);
    *need_cleanup = true;
    let g = w.fact(f).blueprint.expect("etude");
    w.events.mutation("etude_completed", json!({ "etude": g.to_string() }));
    if let Some(p) = w.fact(f).source_fact
        && etude_flags(w, p).1
    {
        return;
    }
    let next = bp(w, g).map(|b| b.view().guids("m_StartsOnComplete")).unwrap_or_default();
    for n in next {
        start_etude(w, n, false, false);
    }
}

/// `CallComponents<IEtudeCompleteTrigger>`: components implementing the interface.
fn call_complete_triggers(w: &mut World, f: FactIdx) {
    for c in crate::facts::comps(w, f) {
        let Some(ty) = w.component(c).ty() else { continue };
        let d = wr_content::schema::ty(ty);
        if super::implements(d.short, "IEtudeCompleteTrigger") {
            let name = d.short;
            match w.registry.clone().component(name) {
                Some(l) if l.handlers().contains(&"IEtudeCompleteTrigger") => {
                    w.coverage.executed(format!("{name}.IEtudeCompleteTrigger"));
                    l.event(w, c, "IEtudeCompleteTrigger");
                }
                _ => w.coverage.unsupported(format!("{name}: IEtudeCompleteTrigger")),
            }
        }
    }
}

/// `EtudesTree.RemoveFact` → `Manager.Remove(fact)`: the manager's `OnFactWillDetach`
/// delegate (`EtudesTree.OnFactWillDetach`) removes the children first, then unlinks the
/// fact from its parent or the roots; then the fact detaches and is disposed.
fn remove_etude_fact(w: &mut World, f: FactIdx) {
    let Some(sys) = w.player.etudes_system else { return };
    fl::manager_remove_with(w, sys, f, true, |w, f| {
        for c in children(w, f) {
            remove_etude_fact(w, c);
        }
        match w.fact(f).source_fact {
            Some(p) => {
                if let FactExt::Etude { children, .. } = &mut w.fact_mut(p).ext {
                    children.retain(|x| *x != f);
                }
            }
            None => w.player.etude_roots.retain(|x| *x != f),
        }
    });
}

fn check_condition_field(w: &mut World, f: FactIdx, field: &str) -> bool {
    let Some(b) = w.fact(f).bp.clone() else { return true };
    crate::elements::check_conditions(w, &b, b.view().obj(field))
}

/// `EtudesTree.SelectPlayingEtudes()`.
fn select_playing_etudes(w: &mut World) {
    let mut stopping: CsDict<FactIdx, ()> = CsDict::new();
    for (f, ()) in w.player.etude_force_stop.iter() {
        stopping.insert(*f, ());
    }
    w.player.etude_force_stop.clear();
    let mut starting: CsDict<FactIdx, ()> = CsDict::new();
    for r in w.player.etude_roots.clone() {
        process_activation(w, r, &mut starting, &mut stopping);
    }
    remove_where_parent_not_playing(w, &mut starting, &stopping);
    let has_actors = starting.keys().any(|f| !conflicting_groups(w, f).is_empty());
    if has_actors {
        // `FilterOnActors`: priorities and conflicting groups (none on the meetings'
        // etudes).
        w.coverage.unsupported("EtudesTree.FilterOnActors (conflicting groups)");
    }
    if filter_on_synchronization(w, &mut starting, &mut stopping) {
        remove_where_parent_not_playing(w, &mut starting, &stopping);
    }
    for f in stopping.keys().collect::<Vec<_>>() {
        stop(w, f);
    }
    for f in starting.keys().collect::<Vec<_>>() {
        fl::activate(w, f);
    }
}

fn is_playing_or_starting(
    w: &World,
    mut e: Option<FactIdx>,
    starting: &CsDict<FactIdx, ()>,
    stopping: &CsDict<FactIdx, ()>,
) -> bool {
    while let Some(f) = e {
        if (!w.fact(f).is_active && !starting.contains_key(&f)) || stopping.contains_key(&f) {
            return false;
        }
        e = w.fact(f).source_fact;
    }
    true
}

fn remove_where_parent_not_playing(
    w: &World,
    starting: &mut CsDict<FactIdx, ()>,
    stopping: &CsDict<FactIdx, ()>,
) {
    // `RemoveWhere(e => !IsPlayingOrStarting(e.Parent))`: the predicate is evaluated for
    // every element against the set as it was before removal (HashSet.RemoveWhere tests
    // each slot and removes it at once, so later tests see earlier removals).
    for f in starting.keys().collect::<Vec<_>>() {
        if !is_playing_or_starting(w, w.fact(f).source_fact, starting, stopping) {
            starting.remove(&f);
        }
    }
}

fn process_activation(
    w: &mut World,
    f: FactIdx,
    starting: &mut CsDict<FactIdx, ()>,
    stopping: &mut CsDict<FactIdx, ()>,
) {
    if etude_flags(w, f).0 {
        return;
    }
    let can = etude_can_play(w, f);
    if w.fact(f).is_active {
        if !can {
            stopping.insert(f, ());
        }
    } else if can {
        starting.insert(f, ());
    }
    if !can {
        return;
    }
    for c in children(w, f) {
        process_activation(w, c, starting, stopping);
    }
}

fn filter_on_synchronization(
    w: &mut World,
    starting: &mut CsDict<FactIdx, ()>,
    stopping: &mut CsDict<FactIdx, ()>,
) -> bool {
    let Some(sys) = w.player.etudes_system else { return false };
    let mut result = false;
    for f in w.entity(sys).facts.list.clone() {
        let Some(b) = w.fact(f).bp.clone() else { continue };
        let synced = b.view().guids("m_Synchronized");
        if synced.is_empty()
            || !is_playing_or_starting(w, Some(f), starting, stopping)
            || !conflicting_groups(w, f).is_empty()
        {
            continue;
        }
        for s in synced {
            if bp(w, s).is_some_and(|sb| !sb.view().guids("m_Synchronized").is_empty()) {
                continue;
            }
            let fact = get_fact(w, s);
            if fact.is_none_or(|sf| !is_playing_or_starting(w, Some(sf), starting, stopping)) {
                if w.fact(f).is_active {
                    stopping.insert(f, ());
                } else {
                    starting.remove(&f);
                }
                result = true;
            }
        }
    }
    result
}

/// `EtudesTree.EtudeCanPlay`.
fn etude_can_play(w: &mut World, f: FactIdx) -> bool {
    let Some(b) = w.fact(f).bp.clone() else { return false };
    let v = b.view();
    let linked = v.guid("m_LinkedAreaPart");
    if let Some(part) = linked
        && !is_linked_area_part(w, &b, part)
    {
        return false;
    }
    if !is_linked_campaign(w, f) {
        return false;
    }
    crate::elements::check_conditions(w, &b, v.obj("ActivationCondition"))
}

/// `BlueprintEtude.IsLinkedAreaPart(LoadEtudesForAreaPart)` with the current area part.
fn is_linked_area_part(w: &World, etude: &Rc<Blueprint>, linked: Guid) -> bool {
    let Some(target) = w.player.area_part else { return false };
    is_linked_area_part_of(w, etude, linked, target)
}

/// `BlueprintEtude.IsLinkedAreaPart(area)` for an explicit area part.
#[must_use]
pub fn is_linked_area_part_of(
    w: &World,
    etude: &Rc<Blueprint>,
    linked: Guid,
    target: Guid,
) -> bool {
    if linked == target {
        return true;
    }
    let include = etude.view().bool("m_IncludeAreaParts");
    let Some(lb) = bp(w, linked) else { return false };
    if include && crate::content::is_a(lb.obj.ty, "BlueprintArea") {
        // `BlueprintArea.GetParts()`: `m_Parts`.
        return lb.view().guids("m_Parts").contains(&target);
    }
    false
}

/// `Etude.IsLinkedCampaign(Player.Campaign)`.
fn is_linked_campaign(w: &World, f: FactIdx) -> bool {
    let mut cur = Some(f);
    while let Some(c) = cur {
        let linked =
            w.fact(c).bp.as_ref().map(|b| b.view().guids("m_LinkedCampaigns")).unwrap_or_default();
        if !linked.is_empty() {
            let campaign = player_campaign(w);
            return linked.iter().any(|l| Some(*l) == campaign);
        }
        cur = w.fact(c).source_fact;
    }
    true
}

/// `EtudesTree.Stop`.
fn stop(w: &mut World, f: FactIdx) {
    let g = w.fact(f).blueprint.expect("etude");
    if is_read_only(w, g) {
        return;
    }
    for c in children(w, f) {
        stop(w, c);
    }
    if w.fact(f).is_active {
        fl::deactivate(w, f);
    }
}

/// `EtudeStatus` condition semantics.
#[must_use]
pub fn status(w: &World, g: Guid) -> EtudeStatus {
    if etude_is_not_started(w, g) {
        return EtudeStatus::NotStarted;
    }
    if etude_is_completed(w, g) {
        return EtudeStatus::Completed;
    }
    match get_fact(w, g) {
        None => EtudeStatus::Started,
        Some(f) => {
            let (_, in_progress) = etude_flags(w, f);
            if in_progress {
                EtudeStatus::CompletionInProgress
            } else if w.fact(f).is_active {
                EtudeStatus::Playing
            } else {
                EtudeStatus::Started
            }
        }
    }
}

/// What `EtudeStatus.CheckCondition` distinguishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtudeStatus {
    NotStarted,
    Started,
    Playing,
    CompletionInProgress,
    Completed,
}
