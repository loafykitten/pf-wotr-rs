//! The element system: actions, conditions and evaluators authored inside blueprints
//! (decompiled, not copied: `Kingmaker.ElementsSystem/{ActionList,ConditionsChecker,
//! Condition,GameAction,Evaluator,UnitEvaluator}.cs` and each element's own file under
//! `Kingmaker.Designers.EventConditionActionSystem.*`).
//!
//! Semantics kept from the C#:
//! - `ActionList.Run`: each non-null action in order; an exception is logged and the next
//!   action still runs (the list is not a transaction).
//! - `ConditionsChecker.Check`: empty is true; `And` stops at the first false, `Or` at the
//!   first true; an exception makes the whole check false; each condition applies `Not`.
//! - `Evaluator.GetValue` throws `FailToEvaluateException` on null; `TryGetValue` and
//!   `CanEvaluate` catch it.
//! - `ContextData<T>` scopes ([`Ctx`]) give evaluators like `RecruitedUnit` and
//!   `ScriptZoneUnit` their value.
//!
//! Fail closed (Decision 7): an element type without a port records an unsupported
//! execution (the run is invalid) and fails like an exception would. Presentation-only
//! actions (2D sounds, one-shot animations) run as recorded stubs; cutscenes are T1 and
//! are recorded with what they would have done (see [`cutscene`]).

pub mod actions;
pub mod conditions;
pub mod cutscene;
pub mod evaluators;

use std::rc::Rc;

use crate::content::{Blueprint, View};
use crate::world::{EntityIdx, World};

/// A `ContextData<T>` scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ctx {
    /// `RecruitedUnitData` (inside `Recruit`'s `OnRecruit`).
    RecruitedUnit(EntityIdx),
    /// `ScriptZoneTriggerData` (a script zone's enter actions).
    ScriptZoneUnit(EntityIdx),
    /// `ClickedUnitData` (dialogue start with a first speaker).
    ClickedUnit(EntityIdx),
    /// `PartyUnitData` (`PartyUnits` condition and action).
    PartyUnit(EntityIdx),
    /// `FactData` (`ContextData<FactData>.Request().Setup(Fact)`).
    Fact(crate::world::FactIdx),
    /// `SpawnedUnitData` (a spawner's `ActionsOnSpawn`, `SpawnActions`).
    SpawnedUnit(EntityIdx),
    /// `EvaluatedUnitCombatTrigger.UnitData` (the unit that joined or left combat).
    CombatUnit(EntityIdx),
    /// `Buff.Data` (`Buff.RunActionInContext`): the buff.
    BuffData(crate::world::FactIdx),
    /// `MechanicsContext.Data` (`MechanicsContext.GetDataScope(target)`): the context the actions
    /// run in (a fact's, or an ability's execution context), and its current target.
    Mechanics { ctx: MechRef, target: Option<EntityIdx> },
}

/// A `MechanicsContext` a scope names: a fact's (`MaybeContext`), or an ability's execution
/// context (`AbilityExecutionContext`, [`crate::combat::spells`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MechRef {
    Fact(crate::world::FactIdx),
    Ability(u32),
}

/// The context a [`MechRef`] names.
#[must_use]
pub fn mech_context(w: &World, r: MechRef) -> Option<&crate::context::MechanicsContext> {
    match r {
        MechRef::Fact(f) => w.fact(f).context.as_ref(),
        MechRef::Ability(i) => w.combat.spells.context(i).map(|c| &c.mech),
    }
}

/// `Context.MaybeCaster` of a [`MechRef`]'s context (a unit the sim has, or none).
#[must_use]
pub fn mech_caster(w: &World, r: MechRef) -> Option<EntityIdx> {
    mech_context(w, r)
        .and_then(|x| x.caster.clone())
        .and_then(|id| w.by_id.get(&id).copied())
        .filter(|u| w.unit(*u).is_some())
}

/// Why an element stopped (a C# exception).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElementError {
    /// `FailToEvaluateException`.
    FailToEvaluate(String),
    /// No port: an unsupported execution was recorded.
    Unsupported(String),
    /// Any other exception the C# would throw.
    Other(String),
}

pub type Result<T> = std::result::Result<T, ElementError>;

/// The short type name of an element object.
#[must_use]
pub fn type_name(v: View<'_>) -> &'static str {
    v.type_name()
}

/// Records an unsupported element type and returns the error.
pub fn unsupported(w: &mut World, kind: &str, ty: &str) -> ElementError {
    let what = format!("{kind} {ty}");
    w.coverage.unsupported(what.clone());
    ElementError::Unsupported(what)
}

/// `ActionList.Run()`.
pub fn run_actions(w: &mut World, owner: &Rc<Blueprint>, list: Option<View<'_>>) {
    let Some(list) = list else { return };
    for a in list.objs("Actions") {
        let v = View(a);
        let ty = v.type_name();
        w.coverage.executed(format!("action {ty}"));
        if let Err(e) = actions::run(w, owner, v) {
            // `PFLog.Actions.ExceptionWithReport(ElementLogicException)` and continue.
            w.log("error", format!("action {ty}: {e:?}"));
            w.events.push(
                "element_exception",
                serde_json::json!({ "action": ty, "error": format!("{e:?}") }),
            );
        }
    }
}

/// `ConditionsChecker.Check()`.
pub fn check_conditions(w: &mut World, owner: &Rc<Blueprint>, checker: Option<View<'_>>) -> bool {
    let Some(checker) = checker else { return true };
    let conds = checker.objs("Conditions");
    if conds.is_empty() {
        return true;
    }
    let or = checker.int("Operation") == 1;
    for c in conds {
        let v = View(c);
        match check(w, owner, v) {
            Ok(flag) => {
                if !or && !flag {
                    return false;
                }
                if or && flag {
                    return true;
                }
            }
            Err(e) => {
                w.log("error", format!("condition {}: {e:?}", v.type_name()));
                return false;
            }
        }
    }
    !or
}

/// `Condition.Check()`: the condition, then `Not`.
pub fn check(w: &mut World, owner: &Rc<Blueprint>, c: View<'_>) -> Result<bool> {
    w.coverage.executed(format!("condition {}", c.type_name()));
    let r = conditions::check_condition(w, owner, c)?;
    Ok(if c.bool("Not") { !r } else { r })
}

/// `UnitEvaluator.GetValue()`: the value, or `FailToEvaluateException`.
pub fn get_unit(w: &mut World, owner: &Rc<Blueprint>, ev: Option<View<'_>>) -> Result<EntityIdx> {
    let Some(ev) = ev else {
        return Err(ElementError::Other("NullReferenceException: evaluator is null".into()));
    };
    match evaluators::unit_value(w, owner, ev)? {
        Some(u) => Ok(u),
        None => Err(ElementError::FailToEvaluate(ev.type_name().into())),
    }
}

/// `UnitEvaluator.TryGetValue(out value)`: catches only `FailToEvaluateException`.
pub fn try_get_unit(
    w: &mut World,
    owner: &Rc<Blueprint>,
    ev: Option<View<'_>>,
) -> Result<Option<EntityIdx>> {
    let Some(ev) = ev else {
        return Err(ElementError::Other("NullReferenceException: evaluator is null".into()));
    };
    match evaluators::unit_value(w, owner, ev) {
        Ok(v) => Ok(v),
        Err(ElementError::FailToEvaluate(_)) => Ok(None),
        Err(e) => Err(e),
    }
}

/// `IntEvaluator.GetValue()`.
pub fn get_int(w: &mut World, owner: &Rc<Blueprint>, ev: View<'_>) -> Result<i32> {
    evaluators::int_value(w, owner, ev)
}

/// The innermost `ContextData` of a kind.
#[must_use]
pub fn ctx_unit(w: &World, pick: fn(&Ctx) -> Option<EntityIdx>) -> Option<EntityIdx> {
    w.ctx.iter().rev().find_map(pick)
}

/// Runs `f` with a `ContextData` scope pushed.
pub fn with_ctx<R>(w: &mut World, c: Ctx, f: impl FnOnce(&mut World) -> R) -> R {
    w.ctx.push(c);
    let r = f(w);
    w.ctx.pop();
    r
}

/// Element types with a port, by kind (for the manifest).
pub const IMPLEMENTED_ACTIONS: &[&str] = actions::IMPLEMENTED;
pub const IMPLEMENTED_CONDITIONS: &[&str] = conditions::IMPLEMENTED;
pub const IMPLEMENTED_EVALUATORS: &[&str] = evaluators::IMPLEMENTED;
