//! Event-driven fact components met on the load path of the POC-4 saves. Sources
//! (decompiled, not copied): `Kingmaker.UnitLogic.FactLogic/
//! ForbidSpellbookOnAlignmentDeviation.cs`, `Kingmaker.Enums/AlignmentExtension.cs`
//! (`ToMask`), `Kingmaker.UnitLogic.Alignments/AlignmentMaskType.cs`,
//! `Kingmaker.UnitLogic.Mechanics.Components/DeathActions.cs`,
//! `Kingmaker.Designers.EventConditionActionSystem.Events/EvaluatedUnitDeathTrigger.cs`,
//! `Kingmaker.Designers.EventConditionActionSystem.Events/AreaDidLoadTrigger.cs`.

use serde_json::{Value, json};

use super::{Port, bits, owner_unit, settings};
use crate::content::View;
use crate::registry::{Hook, Registry};
use crate::world::{CompRef, World};

pub fn register(r: &mut Registry) {
    let mut forbid = Port::new(
        "ForbidSpellbookOnAlignmentDeviation",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        forbid_hook,
    );
    forbid.handlers = &["IAlignmentChangeHandler", "IAreaActivationHandler"];
    forbid.ev = Some(forbid_event);
    r.register(Box::new(forbid));
    // `DeathActions` / `EvaluatedUnitDeathTrigger` are ported in `combat::components`.
    r.register(Box::new(Port::new("ActivateTrigger", bits(&[Hook::Activate]), activate_trigger)));
    let mut area = Port::new("AreaDidLoadTrigger", 0, |_, _, _| {});
    area.handlers = &["IAreaActivationHandler"];
    area.ev = Some(area_did_load);
    r.register(Box::new(area));
}

fn applied(w: &World, c: CompRef) -> bool {
    w.component(c).data.as_ref().and_then(|d| d.get("Applied")).and_then(Value::as_bool)
        == Some(true)
}

fn set_applied(w: &mut World, c: CompRef, v: bool) {
    let d = w.component_mut(c).data.get_or_insert_with(|| json!({}));
    if let Some(o) = d.as_object_mut() {
        o.insert("Applied".into(), Value::Bool(v));
    }
}

/// `OnTurnOn` → `CheckAlignment`; `OnTurnOff` → `Unapply`.
fn forbid_hook(w: &mut World, c: CompRef, h: Hook) {
    match h {
        Hook::TurnOn => check_alignment(w, c),
        Hook::TurnOff => unapply(w, c),
        _ => {}
    }
}

fn forbid_event(w: &mut World, c: CompRef, iface: &str) {
    match iface {
        "IAreaActivationHandler" => check_alignment(w, c),
        "IAlignmentChangeHandler" => {
            let was = applied(w, c);
            check_alignment(w, c);
            if !was && applied(w, c) {
                // Raises `IAlignmentFeaturesChangeHandler` if the unit has one of the
                // spellbooks: spellbooks aren't modelled.
                w.coverage.unsupported(
                    "ForbidSpellbookOnAlignmentDeviation.HandleAlignmentChange: features lost",
                );
            }
        }
        other => w.coverage.unsupported(format!("ForbidSpellbookOnAlignmentDeviation: {other}")),
    }
}

/// `CheckAlignment()`: apply when the owner is of the player's faction, its alignment is
/// outside the mask, and it lacks the ignore fact (which must be set).
fn check_alignment(w: &mut World, c: CompRef) {
    let Some(u) = owner_unit(w, c) else { return };
    let Some((bp, i)) = settings(w, c) else { return };
    let v = View(bp.components_array()[i]);
    let mask = v.int("Alignment");
    let ignore = v.guid("m_IgnoreFact");
    let Some(unit) = w.unit(u) else { return };
    let player_faction = unit.is_player_faction;
    // `ValueRaw.ToMask()`: the alignment index (LG, NG, CG, LN, TN, CN, LE, NE, CE) is the
    // mask bit.
    let al = unit.alignment.as_ref().map_or(4, crate::narrative::alignment::UnitAlignment::value);
    let has_ignore = ignore
        .is_some_and(|g| w.entity(u).facts.list.iter().any(|f| w.fact(*f).blueprint == Some(g)));
    if player_faction && (1i64 << al) & mask == 0 && ignore.is_some() && !has_ignore {
        apply(w, c, v.guids("m_Spellbooks"));
    } else {
        unapply(w, c);
    }
}

fn spellbook_key(w: &World, c: CompRef, sb: wr_content::Guid) -> Option<String> {
    let u = owner_unit(w, c)?;
    Some(format!("unit:{}.ForbiddenSpellbook:{sb}:Alignment", w.entity(u).unique_id))
}

/// `Apply()`: `UnitPartForbiddenSpellbooks.Add(spellbook, Alignment)` for each.
fn apply(w: &mut World, c: CompRef, books: Vec<wr_content::Guid>) {
    if applied(w, c) {
        return;
    }
    for sb in books {
        if let Some(k) = spellbook_key(w, c, sb) {
            super::etudes::retain(w, &k);
        }
    }
    set_applied(w, c, true);
}

/// `Unapply()`.
fn unapply(w: &mut World, c: CompRef) {
    if !applied(w, c) {
        return;
    }
    let books = settings(w, c)
        .map(|(bp, i)| View(bp.components_array()[i]).guids("m_Spellbooks"))
        .unwrap_or_default();
    for sb in books {
        if let Some(k) = spellbook_key(w, c, sb) {
            super::etudes::release(w, &k);
        }
    }
    set_applied(w, c, false);
}

/// `AreaDidLoadTrigger.OnAreaActivated`: in the fact's data scope, `Conditions.Check()`
/// then `Actions.Run()` (`FactData` and the context's data scope only feed evaluators that
/// read them, which report themselves if unported).
fn area_did_load(w: &mut World, c: CompRef, _iface: &str) {
    let Some((bp, i)) = settings(w, c) else { return };
    let v = View(bp.components_array()[i]);
    if crate::elements::check_conditions(w, &bp, v.obj("Conditions")) {
        crate::elements::run_actions(w, &bp, v.obj("Actions"));
    }
}

/// `ActivateTrigger.OnActivate` → `Trigger()`: unless `m_Once` and already triggered,
/// `Conditions.Check()` (outside the fact scope), then `Actions.Run()` inside a
/// `ContextData<FactData>` scope of the component's fact, and `Data.AlreadyTriggered = true`
/// (`ActivateTriggerData`). `ITriggerOnLoad.TriggerOnLoad` is called only by the area's main
/// fact, which has no components in the areas met so far (`area_activated` reports one).
fn activate_trigger(w: &mut World, c: CompRef, h: Hook) {
    if h != Hook::Activate {
        return;
    }
    let Some((bp, i)) = settings(w, c) else { return };
    let v = View(bp.components_array()[i]);
    let already = w
        .component(c)
        .data
        .as_ref()
        .and_then(|d| d.get("AlreadyTriggered"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    w.component_mut(c).data.get_or_insert_with(|| json!({ "AlreadyTriggered": false }));
    if (v.bool("m_Once") && already)
        || !crate::elements::check_conditions(w, &bp, v.obj("Conditions"))
    {
        return;
    }
    crate::elements::with_ctx(w, crate::elements::Ctx::Fact(c.fact), |w| {
        crate::elements::run_actions(w, &bp, v.obj("Actions"));
    });
    if let Some(d) = w.component_mut(c).data.as_mut().and_then(Value::as_object_mut) {
        d.insert("AlreadyTriggered".into(), Value::Bool(true));
    }
}
