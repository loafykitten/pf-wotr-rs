//! `AddDamageResistancePhysical` and the shared `AddDamageResistanceBase.ComponentRuntime`
//! (decompiled, not copied: `Kingmaker.UnitLogic.FactLogic/AddDamageResistanceBase.cs`,
//! `AddDamageResistancePhysical.cs`, `Kingmaker.UnitLogic.Parts/UnitPartDamageReduction.cs`).
//!
//! The runtime is custom, so every hook reaches the port; only three do anything:
//! - `OnActivate`: `m_RemainPool = Settings.Pool.Calculate(Fact.MaybeContext)` (a saved
//!   member of the runtime, kept in the component's `runtime` map);
//! - `OnTurnOn`: `Owner.Ensure<UnitPartDamageReduction>().Add(Fact)`;
//! - `OnTurnOff`: `Owner.Get<UnitPartDamageReduction>()?.Remove(Fact)`.
//!
//! The part keeps the saved `m_SourceFacts` and the chunks built from them (lazily, by
//! `TryInitialize`). Applying the reduction to damage (`ApplyDamageReduction`, the pool
//! spending, `Cleanup`) is combat's: `RuleCalculateDamage` reports a target that has the
//! part as unsupported (`combat::attack`).

use super::{Port, all_hooks, context_value, owner_unit};
use crate::content::{View, is_a};
use crate::registry::{Hook, Registry};
use crate::world::{CompRef, EntityIdx, FactIdx, Part, World};

const PART: &str = "UnitPartDamageReduction";

pub fn register(r: &mut Registry) {
    r.register(Box::new(Port::new("AddDamageResistancePhysical", all_hooks(), hook)));
}

fn hook(w: &mut World, c: CompRef, h: Hook) {
    match h {
        Hook::Activate => {
            let Some((bp, i)) = super::settings(w, c) else { return };
            let objs = bp.components_array();
            let pool = context_value(w, c, View(objs[i]).obj("Pool"));
            w.component_mut(c).runtime.insert("m_RemainPool".into(), pool.into());
        }
        Hook::TurnOn => {
            if let Some(u) = owner_unit(w, c) {
                add(w, u, c.fact);
            }
        }
        Hook::TurnOff => {
            if let Some(u) = owner_unit(w, c) {
                remove(w, u, c.fact);
            }
        }
        _ => {}
    }
}

/// `fact.SelectComponentsWithRuntime<AddDamageResistanceBase>()`: how many chunks the fact
/// makes.
fn runtime_count(w: &World, f: FactIdx) -> usize {
    w.fact(f)
        .components
        .iter()
        .filter(|c| c.ty().is_some_and(|t| is_a(t, "AddDamageResistanceBase")))
        .count()
}

/// `UnitPartDamageReduction.TryInitialize()`: the chunks of the loaded `m_SourceFacts`.
fn try_initialize(w: &mut World, u: EntityIdx) {
    let Some(part) = w.unit(u).and_then(|x| x.damage_reduction.as_ref()) else { return };
    if part.initialized {
        return;
    }
    let sources = part.sources.clone();
    let mut chunks = Vec::new();
    for f in sources {
        chunks.extend(std::iter::repeat_n(f, runtime_count(w, f)));
    }
    if let Some(part) = w.unit_mut(u).and_then(|x| x.damage_reduction.as_mut()) {
        part.initialized = true;
        part.chunks.extend(chunks);
    }
}

/// `Owner.Ensure<UnitPartDamageReduction>()`.
fn ensure(w: &mut World, u: EntityIdx) {
    if !w.entity(u).parts.iter().any(|p| p.clr == PART) {
        w.entity_mut(u).parts.push(Part { clr: PART });
    }
    let unit = w.unit_mut(u).expect("unit");
    if unit.damage_reduction.is_none() {
        // A new part: nothing to load, so it counts as initialized.
        unit.damage_reduction =
            Some(crate::unit::DamageReductionPart { initialized: true, ..Default::default() });
    }
}

/// `UnitPartDamageReduction.Add(fact)`.
fn add(w: &mut World, u: EntityIdx, f: FactIdx) {
    ensure(w, u);
    try_initialize(w, u);
    let n = runtime_count(w, f);
    let Some(part) = w.unit_mut(u).and_then(|x| x.damage_reduction.as_mut()) else { return };
    if part.sources.contains(&f) {
        return;
    }
    part.sources.push(f);
    part.chunks.extend(std::iter::repeat_n(f, n));
}

/// `UnitPartDamageReduction.Remove(fact)` (no `TryInitialize`: a loaded part that was never
/// used has no chunks, so it goes away here) and `RemovePartIfNecessary` (immunities are
/// added by `AddDamageResistanceEnergy`, which isn't ported).
fn remove(w: &mut World, u: EntityIdx, f: FactIdx) {
    let Some(part) = w.unit_mut(u).and_then(|x| x.damage_reduction.as_mut()) else { return };
    part.sources.retain(|x| *x != f);
    part.chunks.retain(|x| *x != f);
    if part.chunks.is_empty() {
        w.unit_mut(u).expect("unit").damage_reduction = None;
        w.entity_mut(u).parts.retain(|p| p.clr != PART);
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use wr_content::node::Node;

    use crate::facts::lifecycle as fl;
    use crate::facts::tests::{feature, loaded_fact, obj};
    use crate::world::{EntityIdx, World};

    fn world() -> (World, EntityIdx) {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        w.registry = Rc::new(crate::components::registry());
        (w, u)
    }

    fn dr(name: &str, pool: i64) -> Node {
        Node::Object(Box::new(obj(
            "Kingmaker.UnitLogic.FactLogic.AddDamageResistancePhysical",
            &[("name", Node::Str(name.into())), ("Pool", simple(pool))],
        )))
    }

    /// A `ContextValue` of type `Simple`.
    fn simple(v: i64) -> Node {
        Node::Object(Box::new(obj(
            "Kingmaker.UnitLogic.Mechanics.ContextValue",
            &[("ValueType", Node::Enum(0)), ("Value", Node::Int(v))],
        )))
    }

    fn has_part(w: &World, u: EntityIdx) -> bool {
        w.entity(u).parts.iter().any(|p| p.clr == "UnitPartDamageReduction")
    }

    #[test]
    fn activation_sets_the_pool_and_turning_on_and_off_adds_and_removes_the_part() {
        let (mut w, u) = world();
        let bp = feature(&w, 40, vec![dr("$DR$a", 9)]);
        let f = loaded_fact(&mut w, u, &bp, false);
        fl::manager_restore_owner_link(&mut w, u);
        fl::manager_post_load(&mut w, u);
        fl::manager_turn_on(&mut w, u);
        assert!(!has_part(&w, u));
        fl::activate(&mut w, f);
        assert_eq!(w.fact(f).components[0].runtime.get("m_RemainPool"), Some(&9.into()));
        assert!(has_part(&w, u));
        let part = w.unit(u).unwrap().damage_reduction.clone().expect("part");
        assert_eq!((part.sources, part.chunks), (vec![f], vec![f]));
        fl::turn_off(&mut w, f);
        assert!(!has_part(&w, u) && w.unit(u).unwrap().damage_reduction.is_none());
        // Turning on again re-adds it, once.
        fl::turn_on(&mut w, f);
        fl::turn_on(&mut w, f);
        assert_eq!(w.unit(u).unwrap().damage_reduction.as_ref().unwrap().chunks.len(), 1);
        assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
    }

    #[test]
    fn two_components_of_a_fact_make_two_chunks_and_two_facts_share_the_part() {
        let (mut w, u) = world();
        let a = feature(&w, 41, vec![dr("$DR$1", 1), dr("$DR$2", 1)]);
        let b = feature(&w, 42, vec![dr("$DR$3", 1)]);
        let fa = loaded_fact(&mut w, u, &a, false);
        let fb = loaded_fact(&mut w, u, &b, false);
        fl::manager_restore_owner_link(&mut w, u);
        fl::manager_post_load(&mut w, u);
        fl::manager_turn_on(&mut w, u);
        fl::activate(&mut w, fa);
        fl::activate(&mut w, fb);
        let part = w.unit(u).unwrap().damage_reduction.clone().expect("part");
        assert_eq!(part.sources, vec![fa, fb]);
        assert_eq!(part.chunks, vec![fa, fa, fb]);
        // Turning the first fact off removes its chunks; the part stays for `fb`.
        fl::turn_off(&mut w, fa);
        let part = w.unit(u).unwrap().damage_reduction.clone().expect("part");
        assert_eq!((part.sources, part.chunks), (vec![fb], vec![fb]));
        fl::turn_off(&mut w, fb);
        assert!(!has_part(&w, u));
    }

    #[test]
    fn a_saved_base_runtime_survives_the_load_with_its_spent_pool() {
        let (mut w, u) = world();
        let bp = feature(&w, 44, vec![dr("$DR$5", 9)]);
        let f = loaded_fact(&mut w, u, &bp, true);
        let c = &mut w.fact_mut(f).components[0];
        c.saved_clr = Some(
            "Kingmaker.UnitLogic.FactLogic.AddDamageResistanceBase+ComponentRuntime, Assembly-CSharp"
                .into(),
        );
        c.runtime.insert("m_RemainPool".into(), 3.into());
        fl::manager_restore_owner_link(&mut w, u);
        fl::manager_post_load(&mut w, u);
        fl::manager_turn_on(&mut w, u);
        let c = &w.fact(f).components[0];
        assert!(!c.is_disposed && c.is_turned_on);
        assert_eq!(c.runtime.get("m_RemainPool"), Some(&3.into()));
        assert!(has_part(&w, u));
        assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
    }

    #[test]
    fn a_loaded_part_builds_its_chunks_on_the_first_add() {
        let (mut w, u) = world();
        let bp = feature(&w, 43, vec![dr("$DR$4", 1)]);
        let f = loaded_fact(&mut w, u, &bp, true);
        w.entity_mut(u).parts.push(crate::world::Part { clr: "UnitPartDamageReduction" });
        w.unit_mut(u).unwrap().damage_reduction = Some(crate::unit::DamageReductionPart {
            sources: vec![f],
            chunks: Vec::new(),
            initialized: false,
        });
        fl::manager_restore_owner_link(&mut w, u);
        fl::manager_post_load(&mut w, u);
        fl::manager_turn_on(&mut w, u);
        // `Add` initialized the part (a chunk from the saved source) and found the fact in it.
        let part = w.unit(u).unwrap().damage_reduction.clone().expect("part");
        assert!(part.initialized);
        assert_eq!((part.sources, part.chunks), (vec![f], vec![f]));
        assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
    }
}
