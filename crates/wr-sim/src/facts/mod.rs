//! Facts (`EntityFact`, `EntityFactsManager`) and their components.
//!
//! [`lifecycle`] ports the activation lifecycle (`Attach`, `Activate`, `TurnOn`, `TurnOff`,
//! `Deactivate`, `Detach`, `PostLoad`, `ApplyPostLoadFixes`, `Reapply`, `Dispose`) and the
//! manager's passes. The rule from the lifecycle trace holds throughout: `IsActive` is
//! saved, `IsTurnedOn` isn't, so a loaded active fact gets `PostLoad` and `TurnOn`, never
//! another `Activate`.

pub mod lifecycle;

use wr_content::schema::{self, type_flags};

use crate::world::{CompRef, FactIdx, FactKind, World};

/// `EntityFact.IsEnabled` (virtual): `Feature` adds `!DisabledBecauseOfPrerequisites`,
/// `Buff` adds `!IsSuppressed`.
#[must_use]
pub fn is_enabled(w: &World, f: FactIdx) -> bool {
    let fact = w.fact(f);
    match fact.kind {
        FactKind::Feature => !fact.disabled_because_of_prerequisites,
        FactKind::Buff => !fact.is_suppressed,
        _ => true,
    }
}

/// `EntityFact.SelectComponents<BlueprintComponent>(c => c.name == name).Any()`.
///
/// The C# applies the predicate to the runtime components only: its second loop yields
/// every non-runtime component of `Blueprint.ComponentsArray` without testing it. So the
/// answer is also true whenever the blueprint has any non-runtime component. Ported as is.
#[must_use]
pub fn select_components_named_any(w: &World, f: FactIdx, name: &str) -> bool {
    let fact = w.fact(f);
    let runtime_match = fact.components.iter().any(|c| {
        c.bp_component().is_some_and(|o| crate::content::View(o).str("name") == Some(name))
    });
    if runtime_match {
        return true;
    }
    fact.bp.as_ref().is_some_and(|bp| {
        bp.components_array().iter().any(|o| !schema::ty(o.ty).has(type_flags::RUNTIME_PROVIDER))
    })
}

/// The facts of an entity, in `Facts.List` order.
#[must_use]
pub fn facts_of(w: &World, e: crate::world::EntityIdx) -> Vec<FactIdx> {
    w.entity(e).facts.list.clone()
}

/// Every component of a fact as a [`CompRef`].
#[must_use]
pub fn comps(w: &World, f: FactIdx) -> Vec<CompRef> {
    (0..w.fact(f).components.len()).map(|comp| CompRef { fact: f, comp }).collect()
}

#[cfg(test)]
pub(crate) mod tests;
