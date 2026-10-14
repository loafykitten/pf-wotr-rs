//! `ModifiableValue.Modifier` (`ModifiableValue.cs:121`).

use super::types::{ModifierDescriptor, StackMode, StatType};
use crate::world::{EntityIdx, FactIdx, World};

/// A stat modifier. Shared by reference in the C# (a stat's list and a field such as the
/// AC's `m_BaseAttributeAppliedBonus` hold the same object), so it lives in
/// [`World::mods`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Modifier {
    /// `AppliedTo` (`[JsonIgnore]`): the stat holding it, as (unit, stat).
    pub applied_to: Option<(EntityIdx, StatType)>,
    pub desc: ModifierDescriptor,
    pub stack_mode: StackMode,
    pub value: i32,
    /// `Source`: the fact that added it.
    pub source: Option<FactIdx>,
    /// `SourceComponent`: the blueprint component name within the source fact.
    pub source_component: Option<String>,
    /// `ItemSource`.
    pub item_source: Option<EntityIdx>,
}

impl Modifier {
    #[must_use]
    pub fn new(value: i32, desc: ModifierDescriptor) -> Self {
        Self {
            applied_to: None,
            desc,
            stack_mode: StackMode::Default,
            value,
            source: None,
            source_component: None,
            item_source: None,
        }
    }

    /// `Stacks`.
    #[must_use]
    pub fn stacks(&self) -> bool {
        if self.stack_mode == StackMode::Default {
            self.desc.is_stackable()
        } else {
            self.stack_mode == StackMode::ForceStack
        }
    }

    /// `IsRacial`.
    #[must_use]
    pub fn is_racial(&self) -> bool {
        self.desc == ModifierDescriptor::Racial
    }

    /// `IsPermanent`: racial, or an enhancement from an item.
    #[must_use]
    pub fn is_permanent(&self) -> bool {
        self.is_racial()
            || (self.desc == ModifierDescriptor::Enhancement && self.item_source.is_some())
    }
}

/// `Modifier.Equals(Modifier)`: the C# `List<Modifier>.Remove` and `IndexOf` use this
/// value equality, not reference equality, so removing a modifier removes the first
/// *equal* one. `AppliedTo` compares by stat type and owner.
#[must_use]
pub fn mod_equals(w: &World, a: crate::world::ModIdx, b: crate::world::ModIdx) -> bool {
    if a == b {
        return true;
    }
    let (x, y) = (&w.mods[a.i()], &w.mods[b.i()]);
    let applied = match (x.applied_to, y.applied_to) {
        (None, None) => true,
        (Some((ou, ot)), Some((pu, pt))) => ot == pt && ou == pu,
        _ => false,
    };
    applied
        && x.desc == y.desc
        && x.stack_mode == y.stack_mode
        && x.value == y.value
        && x.source == y.source
        && x.source_component == y.source_component
        && x.item_source == y.item_source
}

/// `ModifiableValue.IsDuplicate` (used by `AddModifier`'s duplicate removal): same source,
/// source component, value and descriptor.
#[must_use]
pub fn is_duplicate(w: &World, a: crate::world::ModIdx, b: crate::world::ModIdx) -> bool {
    let (x, y) = (&w.mods[a.i()], &w.mods[b.i()]);
    x.source == y.source
        && x.source_component == y.source_component
        && x.value == y.value
        && x.desc == y.desc
}
