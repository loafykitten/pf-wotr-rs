//! Ported blueprint components. Each file ports one C# type (cited at the top), and each
//! port implements exactly the lifecycle hooks and event handlers its C# type overrides
//! (the dispatcher checks this against the decompiled declarations).

pub mod aura_loot;
pub mod books;
pub mod damage_resistance;
pub mod difficulty;
pub mod empty_hand;
pub mod enchant;
pub mod equipment;
pub mod etudes;
pub mod facts;
pub mod inventory;
pub mod items;
pub mod levelup;
pub mod logic;
pub mod mechanics;
pub mod npc_ports;
pub mod shifters_fury;
pub mod state;
pub mod stats;

use std::rc::Rc;

use crate::content::{Blueprint, View};
use crate::registry::{ComponentLogic, Hook, Registry};
use crate::world::{CompRef, EntityIdx, FactKind, World};

/// The registry with every ported component and the known-inert notes.
#[must_use]
pub fn registry() -> Registry {
    let mut r = Registry::default();
    stats::register(&mut r);
    books::register(&mut r);
    items::register(&mut r);
    equipment::register(&mut r);
    inventory::register(&mut r);
    levelup::register(&mut r);
    state::register(&mut r);
    difficulty::register(&mut r);
    etudes::register(&mut r);
    logic::register(&mut r);
    crate::unit_state::register(&mut r);
    facts::register(&mut r);
    mechanics::register(&mut r);
    damage_resistance::register(&mut r);
    empty_hand::register(&mut r);
    npc_ports::register(&mut r);
    aura_loot::register(&mut r);
    shifters_fury::register(&mut r);
    enchant::register(&mut r);
    crate::spawn::register(&mut r);
    crate::companion_spawner::register(&mut r);
    crate::combat::weapon::register(&mut r);
    crate::combat::components::register(&mut r);
    crate::combat::counts::register(&mut r);
    r
}

/// The blueprint holding a component and the component's index (cloned out so the
/// caller can mutate the world while reading settings).
pub(crate) fn settings(w: &World, c: CompRef) -> Option<(Rc<Blueprint>, usize)> {
    w.component(c).source.clone()
}

/// [`settings`] for other modules.
#[must_use]
pub fn settings_of(w: &World, c: CompRef) -> Option<(Rc<Blueprint>, usize)> {
    settings(w, c)
}

/// Runs `f` with a view of the component's settings.
pub(crate) fn with_settings<R>(w: &World, c: CompRef, f: impl FnOnce(View<'_>) -> R) -> Option<R> {
    let (bp, i) = settings(w, c)?;
    let objs = bp.components_array();
    Some(f(View(objs[i])))
}

/// `Owner` of a unit-fact delegate (the fact's owner as a unit).
pub(crate) fn owner_unit(w: &World, c: CompRef) -> Option<EntityIdx> {
    let o = w.fact(c.fact).owner?;
    w.unit(o).map(|_| o)
}

/// `Owner.Wielder` of an item-enchantment delegate.
pub(crate) fn wielder(w: &World, c: CompRef) -> Option<EntityIdx> {
    let o = w.fact(c.fact).owner?;
    w.item(o).and_then(|i| i.wielder)
}

/// `EntityFact.GetRank()`: features and buffs have ranks; other facts are 1.
pub fn fact_rank(w: &World, f: crate::world::FactIdx) -> i32 {
    match w.fact(f).kind {
        FactKind::Feature | FactKind::Buff => w.fact(f).rank,
        _ => 1,
    }
}

/// `ContextValue.Calculate(context)` for the value types whose inputs the context holds:
/// `Simple`, `Rank`, `Shared`, and `CasterProperty` for [`unit_property`]'s properties. Other
/// types report unsupported.
pub(crate) fn context_value(w: &mut World, c: CompRef, cv: Option<View<'_>>) -> i32 {
    let Some(cv) = cv else { return 0 };
    let ctx = w.fact(c.fact).context.clone();
    match cv.int("ValueType") {
        0 => cv.i32("Value"),
        1 => ctx.map_or(0, |x| x.rank(cv.int("ValueRank"))),
        2 => ctx.map_or(0, |x| x.shared(cv.int("ValueShared"))),
        3 => {
            // `caster = context?.MaybeCaster`; a null caster logs and gives 0.
            let caster = ctx.and_then(|x| x.caster).and_then(|id| w.by_id.get(&id).copied());
            let Some(u) = caster.filter(|u| w.unit(*u).is_some()) else {
                w.log("error", "Can't evaluate ContextValue (type: CasterProperty)");
                return 0;
            };
            unit_property(w, u, cv.int("Property"))
        }
        t => {
            w.coverage.unsupported(format!("ContextValue.Calculate: ValueType {t}"));
            0
        }
    }
}

/// `UnitPropertyExtension.GetInt(property, unit)` for the attributes (`StatStrength`..`StatCharisma`:
/// the modified value; `StatBonusStrength`..`StatBonusCharisma`: the bonus) and `None`; the
/// other properties are reported.
pub(crate) fn unit_property(w: &mut World, u: EntityIdx, property: i64) -> i32 {
    use crate::stats::types::StatType;
    const ATTRIBUTES: [StatType; 6] = [
        StatType::Strength,
        StatType::Dexterity,
        StatType::Constitution,
        StatType::Intelligence,
        StatType::Wisdom,
        StatType::Charisma,
    ];
    match property {
        0 => 0,
        4..=9 => {
            w.stats(u).v(ATTRIBUTES[usize::try_from(property - 4).unwrap_or(0)]).modified_value
        }
        10..=15 => crate::stats::ops::attribute_bonus(
            w,
            u,
            ATTRIBUTES[usize::try_from(property - 10).unwrap_or(0)],
        ),
        p => {
            w.coverage.unsupported(format!("UnitProperty.GetInt: property {p}"));
            0
        }
    }
}

/// A component port from a type name, the hooks it implements, and a hook function.
pub(crate) struct Port {
    pub name: &'static str,
    pub bits: u32,
    pub f: fn(&mut World, CompRef, Hook),
    /// Event-handler interfaces ported, and their handler.
    pub handlers: &'static [&'static str],
    pub ev: Option<fn(&mut World, CompRef, &str)>,
}

impl Port {
    pub(crate) fn new(name: &'static str, bits: u32, f: fn(&mut World, CompRef, Hook)) -> Self {
        Self { name, bits, f, handlers: &[], ev: None }
    }
}

impl ComponentLogic for Port {
    fn type_name(&self) -> &'static str {
        self.name
    }

    fn implemented(&self) -> u32 {
        self.bits
    }

    fn hook(&self, w: &mut World, c: CompRef, h: Hook) {
        (self.f)(w, c, h);
    }

    fn handlers(&self) -> &'static [&'static str] {
        self.handlers
    }

    fn event(&self, w: &mut World, c: CompRef, iface: &str) {
        match self.ev {
            Some(f) => f(w, c, iface),
            None => w.coverage.unsupported(format!("{}: {iface}", self.name)),
        }
    }
}

/// Every hook bit but `Recalculate` (for custom runtimes, whose unlisted hooks are the
/// base no-ops; `Recalculate`'s base is off-then-on, which the dispatcher runs).
pub(crate) fn all_hooks() -> u32 {
    use Hook::*;
    bits(&[
        Initialize,
        FactAttached,
        Activate,
        Deactivate,
        TurnOn,
        TurnOff,
        PostLoad,
        ApplyPostLoadFixes,
        Dispose,
        ViewDidAttach,
        ViewWillDetach,
        PreSave,
    ])
}

pub(crate) fn bits(hooks: &[Hook]) -> u32 {
    hooks.iter().fold(0, |a, h| a | h.bit())
}
