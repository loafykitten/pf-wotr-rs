//! The fact and component lifecycle, ported from `EntityFact.cs`, `EntityFactComponent.cs`,
//! `EntityFactComponentDelegate.cs` and `EntityFactsManager.cs`. Exceptions the C#
//! catches and logs around each step don't exist here (a port doesn't throw); where the C#
//! would log an error and return, so do we ([`World::log`]).

use std::rc::Rc;

use crate::content::Blueprint;
use crate::csharp::sort::{grown_capacity, introsort};
use crate::registry::{Hook, dispatch_hook};
use crate::world::{CompRef, Component, EntityIdx, FactIdx, World};

fn label(w: &World, f: FactIdx) -> String {
    w.fact_label(f)
}

// ---- components ---------------------------------------------------------------------

/// `EntityFactComponent.Setup(component)` (the delegate runtime's override checks the
/// component's type; a mismatch logs and leaves it unset).
pub fn component_setup(w: &mut World, c: CompRef, source: (Rc<Blueprint>, usize)) {
    let ok = match w.component(c).saved_clr.as_deref() {
        // A saved runtime of a delegate for another component type: `Setup` rejects it.
        Some(saved) => saved_runtime_matches(saved, &source),
        None => true,
    };
    if !ok {
        w.log(
            "error",
            format!(
                "EntityFactComponentDelegate.Runtime.Setup: invalid component type ({})",
                label(w, c.fact)
            ),
        );
        return;
    }
    let comp = w.component_mut(c);
    comp.is_disposed = false;
    let name = crate::content::View(source.0.components_array()[source.1])
        .str("name")
        .unwrap_or_default()
        .to_owned();
    comp.source = Some(source);
    comp.name = name;
}

/// Whether a saved runtime's CLR type (e.g. `AddFacts+…ComponentRuntime` of the delegate
/// `EntityFactComponentDelegate<UnitEntityData, AddFactsData>`) fits the blueprint
/// component. Runtimes are saved as the delegate's nested `ComponentRuntime` closed over
/// `TData`, so the check compares the data type the runtime names with the component's.
fn saved_runtime_matches(saved: &str, source: &(Rc<Blueprint>, usize)) -> bool {
    let obj = source.0.components_array()[source.1];
    let ty = wr_content::schema::ty(obj.ty);
    if ty.fact_hooks & wr_content::schema::fact_hooks::CUSTOM_RUNTIME != 0 {
        // Custom runtimes are saved under their own nested class name.
        // `AddDamageResistanceBase.CreateRuntimeFactComponent` makes the base's own
        // `ComponentRuntime` for the subclasses that don't override it (Physical).
        return saved.contains(ty.short)
            || (crate::content::is_a(obj.ty, "AddDamageResistanceBase")
                && saved.contains("AddDamageResistanceBase+ComponentRuntime"));
    }
    // Find the delegate base's TData among the component's bases.
    let mut cur = obj.ty;
    for _ in 0..64 {
        if cur == wr_content::schema::NO_TYPE {
            break;
        }
        let d = wr_content::schema::ty(cur);
        if let Some(rest) =
            d.name.strip_prefix("Kingmaker.EntitySystem.EntityFactComponentDelegate<")
        {
            // `Setup` checks `component is EntityFactComponentDelegate<TEntity, TData>` for
            // the runtime's closed type: compare both generic arguments by full name.
            let inner = rest.strip_suffix('>').unwrap_or(rest);
            let mut parts = inner.splitn(2, ", ");
            let (entity, data) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            if data.is_empty() {
                // `EntityFactComponentDelegate<TData>`: the two-argument base is further up.
                cur = d.base;
                continue;
            }
            let norm = |x: &str| x.replace('+', ".");
            let Some(tn) = wr_save::clr::parse(saved) else { return false };
            let args: Vec<String> = tn.args.iter().map(|(k, _)| norm(k)).collect();
            return match args.as_slice() {
                // `EntityFactComponentDelegate<TEntity, TData>.ComponentRuntime`.
                [e, d] => *e == norm(entity) && *d == norm(data),
                // A runtime nested in a one-argument delegate subclass
                // (`UnitBuffComponentDelegate<TData>.UnitBuffComponentRuntime`).
                [d] => *d == norm(data),
                _ => false,
            };
        }
        cur = d.base;
    }
    true
}

/// `EntityFactComponent.Initialize(fact)`.
pub fn component_initialize(w: &mut World, c: CompRef) {
    dispatch_hook(w, c, Hook::Initialize);
}

/// `EntityFactComponent.Activate()`.
pub fn component_activate(w: &mut World, c: CompRef) {
    if w.component(c).is_disposed {
        return;
    }
    dispatch_hook(w, c, Hook::Activate);
    if w.fact(c.fact).is_turned_on {
        component_turn_on(w, c);
    }
}

/// `EntityFactComponent.Deactivate()`.
pub fn component_deactivate(w: &mut World, c: CompRef) {
    if w.component(c).is_disposed {
        return;
    }
    if w.component(c).is_turned_on {
        component_turn_off(w, c);
    }
    dispatch_hook(w, c, Hook::Deactivate);
}

/// `EntityFactComponent.TurnOn()`.
pub fn component_turn_on(w: &mut World, c: CompRef) {
    let comp = w.component(c);
    if comp.is_turned_on || comp.is_disposed {
        return;
    }
    w.component_mut(c).is_turned_on = true;
    crate::rulebook::bus::subscribe_component(w, c);
    w.trace("Component.TurnOn", |w| format!("{}/{}", label(w, c.fact), w.component(c).name));
    dispatch_hook(w, c, Hook::TurnOn);
}

/// `EntityFactComponent.TurnOff()`.
pub fn component_turn_off(w: &mut World, c: CompRef) {
    let comp = w.component(c);
    if !comp.is_turned_on || comp.is_disposed {
        return;
    }
    w.component_mut(c).is_turned_on = false;
    crate::rulebook::bus::unsubscribe_component(w, c);
    w.trace("Component.TurnOff", |w| format!("{}/{}", label(w, c.fact), w.component(c).name));
    dispatch_hook(w, c, Hook::TurnOff);
}

/// `EntityFactComponent.PostLoad()`.
pub fn component_post_load(w: &mut World, c: CompRef) {
    dispatch_hook(w, c, Hook::PostLoad);
}

/// `EntityFactComponent.OnRecalculate()` (default: off then on).
pub fn component_recalculate(w: &mut World, c: CompRef) {
    dispatch_hook(w, c, Hook::Recalculate);
}

/// `EntityFactComponent.Dispose()`.
pub fn component_dispose(w: &mut World, c: CompRef) {
    crate::rulebook::bus::unsubscribe_component(w, c);
    dispatch_hook(w, c, Hook::Dispose);
    let comp = w.component_mut(c);
    // The delegate runtime's `OnDispose` clears `m_Data`.
    comp.data = None;
    comp.source = None;
    comp.is_disposed = true;
}

// ---- facts --------------------------------------------------------------------------

fn subscribe_fact(w: &mut World, f: FactIdx) {
    if !w.fact(f).subscribed {
        crate::rulebook::bus::subscribe_fact(w, f);
        w.fact_mut(f).subscribed = true;
    }
}

fn unsubscribe_fact(w: &mut World, f: FactIdx) {
    if w.fact(f).subscribed {
        crate::rulebook::bus::unsubscribe_fact(w, f);
        w.fact_mut(f).subscribed = false;
    }
}

/// `EntityFact.Activate()`.
pub fn activate(w: &mut World, f: FactIdx) {
    if !super::is_enabled(w, f) || w.owner_disposing(f) {
        return;
    }
    let fact = w.fact(f);
    if fact.is_active {
        w.log("error", format!("EntityFact.Activate: already active ({})", label(w, f)));
        return;
    }
    if fact.activating || fact.deactivating {
        w.log("error", format!("EntityFact.Activate: re-entered ({})", label(w, f)));
        return;
    }
    w.fact_mut(f).is_active = true;
    w.fact_mut(f).activating = true;
    crate::combat::vision::note_fact_starting(w, f);
    w.trace("Fact.Activate", |w| label(w, f));
    on_activate(w, f);
    crate::context::recalculate(w, f);
    for c in super::comps(w, f) {
        component_activate(w, c);
    }
    on_components_did_activated(w, f);
    let owner = w.fact(f).owner;
    if owner.is_some_and(|o| w.entity(o).facts.is_turned_on) {
        turn_on(w, f);
    }
    w.fact_mut(f).activating = false;
    if w.fact(f).remove_when_activation_ended {
        w.fact_mut(f).remove_when_activation_ended = false;
        if let Some(o) = owner {
            manager_remove(w, o, f, true);
        }
    }
}

/// `EntityFact.Deactivate()`.
pub fn deactivate(w: &mut World, f: FactIdx) {
    let fact = w.fact(f);
    if !fact.is_active {
        w.log("error", format!("EntityFact.Deactivate: is not active ({})", label(w, f)));
        return;
    }
    if fact.deactivating {
        w.log("error", format!("EntityFact.Deactivate: already deactivating ({})", label(w, f)));
        return;
    }
    if fact.activating {
        w.log("error", "EntityFact.Deactivate: invoked from Activate");
    }
    w.fact_mut(f).deactivating = true;
    crate::combat::vision::note_fact_ending(w, f);
    w.trace("Fact.Deactivate", |w| label(w, f));
    if w.fact(f).is_turned_on {
        turn_off(w, f);
    }
    for c in super::comps(w, f) {
        component_deactivate(w, c);
    }
    on_deactivate(w, f);
    w.fact_mut(f).is_active = false;
    w.fact_mut(f).deactivating = false;
}

/// `EntityFact.UpdateIsActive()`.
pub fn update_is_active(w: &mut World, f: FactIdx) {
    let enabled = super::is_enabled(w, f);
    if w.fact(f).is_active && !enabled {
        deactivate(w, f);
    }
    if !w.fact(f).is_active && super::is_enabled(w, f) {
        activate(w, f);
    }
}

/// `EntityFact.TurnOn()`.
pub fn turn_on(w: &mut World, f: FactIdx) {
    if w.owner_disposing(f) {
        return;
    }
    if !w.fact(f).is_active {
        w.log("error", format!("EntityFact.TurnOn: invalid state ({})", label(w, f)));
        return;
    }
    if w.fact(f).is_turned_on {
        w.log("error", format!("EntityFact.TurnOn: already turned on ({})", label(w, f)));
        return;
    }
    if w.fact(f).recalculate_context_before_turn_on {
        w.fact_mut(f).recalculate_context_before_turn_on = false;
        crate::context::recalculate(w, f);
    }
    w.fact_mut(f).is_turned_on = true;
    subscribe_fact(w, f);
    w.trace("Fact.TurnOn", |w| label(w, f));
    // `OnTurnOn`: no fact class on the units' path overrides it (checked in the C#).
    for c in super::comps(w, f) {
        component_turn_on(w, c);
    }
}

/// `EntityFact.TurnOff()`.
pub fn turn_off(w: &mut World, f: FactIdx) {
    if !w.fact(f).is_turned_on {
        w.log("error", format!("EntityFact.TurnOff: is not turned on ({})", label(w, f)));
        return;
    }
    unsubscribe_fact(w, f);
    w.fact_mut(f).is_turned_on = false;
    w.trace("Fact.TurnOff", |w| label(w, f));
    for c in super::comps(w, f) {
        component_turn_off(w, c);
    }
}

/// `EntityFact.Reapply()`.
pub fn reapply(w: &mut World, f: FactIdx) {
    if w.owner_disposing(f) {
        return;
    }
    if w.fact(f).reapplying {
        w.log("error", format!("EntityFact {} is already reapplying", label(w, f)));
        return;
    }
    w.fact_mut(f).reapplying = true;
    crate::context::recalculate(w, f);
    if w.fact(f).is_active && !w.fact(f).deactivating {
        deactivate(w, f);
        activate(w, f);
    }
    w.fact_mut(f).reapplying = false;
}

/// The fact class's `OnActivate` (virtual). `Feature.OnActivate` logs a parametrized feature
/// whose `Param == null`; `FeatureParam.operator ==` makes an empty param equal to null, so an
/// absent or empty param logs. The other unit fact classes don't override it.
fn on_activate(w: &mut World, f: FactIdx) {
    if w.fact(f).kind == crate::world::FactKind::Feature
        && w.fact(f)
            .bp
            .as_ref()
            .is_some_and(|b| crate::content::is_a(b.obj.ty, "BlueprintParametrizedFeature"))
        && crate::feature_param::fact_param(w, f).is_empty()
    {
        w.log("error", format!("Param is not specified for {}", label(w, f)));
    }
    match w.fact(f).clr {
        // `Etude.OnActivate`: conditions dirty, conflicting groups, the playing record.
        "Etude" => crate::narrative::etudes::etude_on_activate(w, f),
        // `ItemEnchantment.OnActivate`: the context for the wielder.
        "ItemEnchantment" => crate::items::enchantment_on_activate(w, f),
        // `Quest`, `QuestObjective` and `Tutorial` don't override `OnActivate`.
        "Quest" | "QuestObjective" | "Tutorial" => {}
        // `ActivatableAbility.OnActivate`: a targeted toggle adds its target-selection
        // ability.
        "ActivatableAbility" => activatable_on_activate(w, f),
        _ => {
            if !matches!(
                w.fact(f).kind,
                crate::world::FactKind::Feature
                    | crate::world::FactKind::UnitFact
                    | crate::world::FactKind::Buff
                    | crate::world::FactKind::Ability
                    | crate::world::FactKind::MainUnitFact
            ) {
                w.coverage.unsupported(format!("{}.OnActivate", w.fact(f).clr));
            }
        }
    }
}

/// `Buff.OnComponentsDidActivated` spawns the buff's particle effect (presentation only).
fn on_components_did_activated(_w: &mut World, _f: FactIdx) {}

/// `Buff.OnDeactivate` clears the particle effect (presentation only); `Etude.OnDeactivate`
/// marks the etude system dirty and releases conflicting groups.
fn on_deactivate(w: &mut World, f: FactIdx) {
    match w.fact(f).clr {
        "Etude" => crate::narrative::etudes::etude_on_deactivate(w, f),
        // `ItemEnchantment.OnDeactivate`: `m_CurrentContext = null`.
        "ItemEnchantment" => w.fact_mut(f).context = None,
        // `ActivatableAbility.OnDeactivate`: remove the target-selection ability (targeted
        // toggles only), then `Stop()`.
        "ActivatableAbility" => {
            if let Some(a) = w.fact_mut(f).select_target.take()
                && let Some(o) = w.fact(f).owner
            {
                crate::unit_facts::remove(w, o, a);
            }
            crate::unit_facts::stop(w, f);
        }
        _ => {}
    }
}

/// `ActivatableAbility.OnActivate`: a targeted toggle adds its target-selection ability
/// (`Owner.Unit.Facts.Add(new Ability(Blueprint.SelectTargetAbility, Owner))`); a missing
/// blueprint is reported.
fn activatable_on_activate(w: &mut World, f: FactIdx) {
    let Some(bp) = w.fact(f).bp.clone() else { return };
    if !bp.view().bool("IsTargeted") {
        return;
    }
    let Some(owner) = w.fact(f).owner.filter(|o| w.unit(*o).is_some()) else { return };
    let Some(ab) = bp.view().guid("m_SelectTargetAbility").and_then(|g| w.content.get(g)) else {
        w.coverage.unsupported("ActivatableAbility.OnActivate: no target-selection ability");
        return;
    };
    let a = crate::unit_facts::create_fact(w, &ab, owner, None);
    let added = crate::unit_facts::add(w, owner, a);
    w.fact_mut(f).select_target = added;
}

/// `EntityFact.PostLoad()`.
pub fn post_load(w: &mut World, f: FactIdx) {
    if w.fact(f).post_load_executed {
        return;
    }
    if w.fact(f).owner.is_none() {
        w.log("error", "Manager is null");
        return;
    }
    w.trace("Fact.PostLoad", |w| label(w, f));
    on_post_load(w, f);
    post_load_components(w, f);
    // `OnComponentsDidPostLoad`: no unit fact class overrides it.
    w.fact_mut(f).post_load_executed = true;
}

fn on_post_load(w: &mut World, f: FactIdx) {
    match w.fact(f).clr {
        // `Ability.OnPostLoad` → `AbilityData.PostLoad(this)`: restores the ability's
        // caster and fact back-links (caches; no state a character sheet reads).
        "Ability" => w.coverage.executed("Ability.OnPostLoad (back-links)"),
        "ActivatableAbility" => activatable_on_post_load(w, f),
        "Quest" => crate::narrative::quests::quest_on_post_load(w, f),
        "QuestObjective" => crate::narrative::quests::objective_on_post_load(w, f),
        _ => {}
    }
}

/// `ActivatableAbility.OnPostLoad`: a command-activated ability that isn't active is
/// switched off (`IsOn = false`, whose setter can stop the ability); an on, started
/// ability whose buff is missing, inactive or of another blueprint gets it reapplied.
fn activatable_on_post_load(w: &mut World, f: FactIdx) {
    let Some((is_on, started, applied)) = w.fact(f).activatable else { return };
    let with_command = w.fact(f).bp.as_ref().is_some_and(|b| b.view().int("ActivationType") == 1);
    if with_command && !w.fact(f).is_active && is_on {
        w.coverage.unsupported("ActivatableAbility.OnPostLoad: IsOn = false");
    }
    if is_on && started {
        let want = w.fact(f).bp.as_ref().and_then(|b| b.view().guid("m_Buff"));
        let ok = applied.is_some_and(|b| {
            let bf = w.fact(b);
            (bf.is_active || bf.is_suppressed) && bf.blueprint == want
        });
        if !ok {
            w.coverage.unsupported("ActivatableAbility.ReapplyBuff");
        }
    }
    w.coverage.executed("ActivatableAbility.OnPostLoad");
}

/// `EntityFact.PostLoadComponents()`: match each saved runtime to the blueprint
/// component of the same name (`Setup`), dispose orphans, run `PostLoad` on the rest,
/// then drop the disposed.
fn post_load_components(w: &mut World, f: FactIdx) {
    let bcs = w.blueprint_components(f);
    for c in super::comps(w, f) {
        let name = w.component(c).name.clone();
        if !name.is_empty() {
            let found = bcs.iter().find(|(bp, i)| {
                crate::content::View(bp.components_array()[*i]).str("name") == Some(name.as_str())
            });
            match found {
                Some(src) => {
                    component_setup(w, c, src.clone());
                    if w.component(c).source.is_none() {
                        component_dispose(w, c);
                    }
                }
                None => {
                    w.log(
                        "warning",
                        format!(
                            "EntityFact.PostLoad: can't find source component {name} ({})",
                            label(w, f)
                        ),
                    );
                    component_dispose(w, c);
                }
            }
        }
        if !w.component(c).is_disposed {
            component_post_load(w, c);
        }
    }
    w.fact_mut(f).components.retain(|c| !c.is_disposed);
}

/// `EntityFact.Setup(blueprint)`: create runtime components the fact lacks (by name),
/// for enabled runtime-provider components of `BlueprintComponents`.
pub fn setup(w: &mut World, f: FactIdx) {
    w.fact_mut(f).is_disposed = false;
    let bcs = w.blueprint_components(f);
    for (bp, i) in bcs {
        let obj = bp.components_array()[i];
        let t = wr_content::schema::ty(obj.ty);
        if !t.has(wr_content::schema::type_flags::RUNTIME_PROVIDER) {
            continue;
        }
        let v = crate::content::View(obj);
        let name = v.str("name").unwrap_or_default().to_owned();
        let disabled = v.int("m_Flags") & 1 != 0;
        if disabled || w.fact(f).components.iter().any(|c| c.name == name) {
            continue;
        }
        let comp = Component {
            name,
            source: None,
            saved_clr: None,
            data: None,
            is_turned_on: false,
            is_disposed: false,
            subscribed: false,
            fact_refs: std::collections::HashMap::new(),
            runtime: serde_json::Map::new(),
            need_resume: false,
        };
        let idx = w.fact(f).components.len();
        w.fact_mut(f).components.push(comp);
        let c = CompRef { fact: f, comp: idx };
        component_setup(w, c, (bp.clone(), i));
        add_component_tail(w, c);
    }
}

/// The tail of `EntityFact.AddComponent` after the list insert: `Initialize`, and
/// `Activate` if the fact is active.
fn add_component_tail(w: &mut World, c: CompRef) {
    w.trace("Fact.AddComponent", |w| format!("{}/{}", label(w, c.fact), w.component(c).name));
    component_initialize(w, c);
    if w.fact(c.fact).is_active {
        component_activate(w, c);
    }
}

/// `EntityFact.ApplyPostLoadFixes()`.
pub fn apply_post_load_fixes(w: &mut World, f: FactIdx) {
    w.trace("Fact.ApplyPostLoadFixes", |w| label(w, f));
    for c in super::comps(w, f) {
        let disabled = w
            .component(c)
            .bp_component()
            .is_some_and(|o| crate::content::View(o).int("m_Flags") & 1 != 0);
        if disabled {
            component_deactivate(w, c);
            component_dispose(w, c);
        } else {
            dispatch_hook(w, c, Hook::ApplyPostLoadFixes);
        }
        if w.fact(f).is_disposed {
            return;
        }
    }
    w.fact_mut(f).components.retain(|c| !c.is_disposed);
    setup(w, f);
    if w.fact(f).reapply_on_apply_post_load_fixes {
        reapply(w, f);
        w.fact_mut(f).reapply_on_apply_post_load_fixes = false;
    }
    if w.fact(f).clr == "ActivatableAbility" {
        // `if (!IsStarted && m_AppliedBuff != null) { m_AppliedBuff.Remove(); … }`
        if let Some((_, started, Some(_))) = w.fact(f).activatable
            && !started
        {
            w.coverage.unsupported("ActivatableAbility.OnApplyPostLoadFixes: remove applied buff");
        }
    }
}

/// `EntityFact.Dispose()`.
pub fn dispose(w: &mut World, f: FactIdx) {
    if w.fact(f).is_disposed {
        return;
    }
    let disposing = w.fact(f).owner.is_some_and(|o| w.entity(o).disposing);
    if w.fact(f).owner.is_some() && !disposing {
        detach(w, f);
    } else {
        unsubscribe_fact(w, f);
    }
    for c in super::comps(w, f) {
        component_dispose(w, c);
    }
    w.fact_mut(f).is_disposed = true;
}

/// `EntityFact.Detach()`.
pub fn detach(w: &mut World, f: FactIdx) {
    if w.fact(f).owner.is_none() {
        w.log("error", "EntityFact.Detach: isn't attached");
        return;
    }
    if w.fact(f).is_active {
        deactivate(w, f);
    }
    // `OnDetach`: `Buff.OnDetach` drops its particle-effect references (presentation);
    // `MainUnitFact` doesn't override it.
    w.fact_mut(f).owner = None;
}

// ---- the manager --------------------------------------------------------------------

/// `EntityFactsManager.RestoreOwnerLink(owner)` → each fact's `RestoreOwnerLink`.
pub fn manager_restore_owner_link(w: &mut World, e: EntityIdx) {
    for f in w.entity(e).facts.list.clone() {
        w.fact_mut(f).owner = Some(e);
    }
}

/// `EntityFactsManager.PostLoad()`: post-load facts in saved order (facts without a
/// blueprint are removed under `SafeSaves`).
pub fn manager_post_load(w: &mut World, e: EntityIdx) {
    for f in w.entity(e).facts.list.clone() {
        if w.fact(f).bp.is_none() {
            w.log("error", format!("Fact {} had no blueprint and was removed!", w.fact(f).clr));
            w.entity_mut(e).facts.list.retain(|x| *x != f);
            crate::combat::vision::note_corpulence_change(w, e);
        } else {
            post_load(w, f);
        }
    }
}

/// `EntityFactsManager.ApplyPostLoadFixes()` over a snapshot.
pub fn manager_apply_post_load_fixes(w: &mut World, e: EntityIdx) {
    for f in w.entity(e).facts.list.clone() {
        if !w.fact(f).is_disposed {
            apply_post_load_fixes(w, f);
        }
    }
}

/// `EntityFactsManager.TurnOn()`: a temp copy sorted by `TurnOnPriority` with Mono's
/// unstable sort (ties are *not* kept in list order), then turn on each fact still owned,
/// active and off.
pub fn manager_turn_on(w: &mut World, e: EntityIdx) {
    if w.entity(e).facts.is_turned_on {
        return;
    }
    w.entity_mut(e).facts.is_turned_on = true;
    let mut list = w.entity(e).facts.list.clone();
    // `ToTempList` gives capacity max(pooled, source); we know the source's.
    let cap = w.entity(e).facts.capacity.max(grown_capacity(list.len()));
    let facts = &w.facts;
    let rep = introsort(&mut list, cap, &mut |a: &FactIdx, b: &FactIdx| {
        facts[a.i()].turn_on_priority.wrapping_sub(facts[b.i()].turn_on_priority)
    });
    if rep.heapsort {
        w.coverage.unsupported(
            "EntityFactsManager.TurnOn sort reached heapsort (pooled capacity unknown)",
        );
    }
    w.trace("FactsManager.TurnOn", |w| w.entity(e).unique_id.clone());
    for f in list {
        let fact = w.fact(f);
        if fact.owner == Some(e) && fact.is_active && !fact.is_turned_on {
            turn_on(w, f);
        }
    }
}

/// `EntityFactsManager.TurnOff()`.
pub fn manager_turn_off(w: &mut World, e: EntityIdx) {
    if !w.entity(e).facts.is_turned_on {
        return;
    }
    w.entity_mut(e).facts.is_turned_on = false;
    let mut list = w.entity(e).facts.list.clone();
    let cap = w.entity(e).facts.capacity.max(grown_capacity(list.len()));
    let facts = &w.facts;
    introsort(&mut list, cap, &mut |a: &FactIdx, b: &FactIdx| {
        facts[a.i()].turn_off_priority.wrapping_sub(facts[b.i()].turn_off_priority)
    });
    for f in list {
        let fact = w.fact(f);
        if fact.owner == Some(e) && fact.is_active && fact.is_turned_on {
            turn_off(w, f);
        }
    }
}

/// `EntityFactsManager.Remove(fact, dispose)` (no fact processors modelled yet).
pub fn manager_remove(w: &mut World, e: EntityIdx, f: FactIdx, dispose_fact: bool) {
    // Unit facts have fact processors (`FeatureCollection` decrements ranks above one
    // instead of detaching, buffs and abilities have their own): not ported.
    if w.fact(f).kind != crate::world::FactKind::Other && w.unit(e).is_some() {
        w.coverage
            .unsupported(format!("fact processor for {} (PrepareFactForDetach)", w.fact(f).clr));
    }
    manager_remove_with(w, e, f, dispose_fact, |_, _| {});
}

/// `EntityFactsManager.Remove(fact, dispose)` after the fact processor: out of the list,
/// WrathOracle's `fact_detach` record (a prefix on `DelegateOnFactWillDetach`), the
/// manager's `OnFactWillDetach` delegate, then `Detach` and `Dispose`.
pub fn manager_remove_with(
    w: &mut World,
    e: EntityIdx,
    f: FactIdx,
    dispose_fact: bool,
    will_detach: impl FnOnce(&mut World, FactIdx),
) {
    if w.fact(f).activating {
        w.fact_mut(f).remove_when_activation_ended = true;
        return;
    }
    let list = &mut w.entity_mut(e).facts.list;
    if let Some(p) = list.iter().position(|x| *x == f) {
        list.remove(p);
        crate::combat::vision::note_corpulence_change(w, e);
        crate::combat::vision::note_fact_ending(w, f);
        let rank = crate::components::fact_rank(w, f);
        w.events.mutation(
            "fact_detach",
            serde_json::json!({ "owner": w.entity(e).unique_id, "fact": {"$fact": w.fact(f).unique_id, "blueprint": w.fact(f).blueprint.map(|g| g.to_string())}, "class": w.fact(f).clr, "rank": rank }),
        );
        will_detach(w, f);
        detach(w, f);
        if dispose_fact {
            dispose(w, f);
        }
    }
}

/// `EntityFactsManager.Add(fact)` → `fact.Attach(manager)` for a new fact: unique id,
/// attach time, components' `OnFactAttached`, `OnAttach`, then `Activate` unless
/// suppressed. Fact processors (`PrepareFactForAttach`, e.g. buff stacking) are not
/// modelled: adding a fact whose kind has one is reported.
pub fn manager_add(w: &mut World, e: EntityIdx, f: FactIdx) {
    if matches!(w.fact(f).kind, crate::world::FactKind::Buff | crate::world::FactKind::Feature) {
        w.coverage
            .unsupported(format!("fact processor for {} (PrepareFactForAttach)", w.fact(f).clr));
    }
    manager_add_attach(w, e, f, false);
}

/// `new TFact(blueprint)`: a fact with its blueprint, `IsPostLoadExecuted = true`, and
/// `Setup(blueprint)` (runtime components for the blueprint's runtime providers).
pub fn new_fact(
    w: &mut World,
    kind: crate::world::FactKind,
    clr: &'static str,
    bp: Rc<Blueprint>,
) -> FactIdx {
    let mut fact = crate::world::Fact::blank(kind, clr);
    fact.blueprint = Some(bp.guid);
    fact.bp = Some(bp);
    fact.post_load_executed = true;
    let f = w.add_fact(fact);
    setup(w, f);
    f
}

/// The part of `EntityFactsManager.Add(fact)` after the fact processors ran
/// (`PrepareFactForAttach` returns the fact): list insert, then `fact.Attach(manager)`:
/// `UniqueId ??= Player.GetNewUniqueId()`, `AttachTime`, components' `OnFactAttached`,
/// `OnAttach`, and `Activate()` unless `SuppressActivationOnAttach`.
pub fn manager_add_attach(w: &mut World, e: EntityIdx, f: FactIdx, suppress_activation: bool) {
    manager_add_attach_with(w, e, f, suppress_activation, |_, _| {});
}

/// `EntityFactsManager.Add(fact)` with the manager's `OnFactDidAttach` delegate, then the
/// `fact_attach` trace record (WrathOracle's postfix on `DelegateOnFactDidAttach`).
pub fn manager_add_attach_with(
    w: &mut World,
    e: EntityIdx,
    f: FactIdx,
    suppress_activation: bool,
    did_attach: impl FnOnce(&mut World, FactIdx),
) {
    if w.fact(f).owner.is_some() {
        w.log("error", "EntityFactsManager.Add: invalid fact state");
        return;
    }
    if w.fact(f).unique_id.is_empty() {
        let id = crate::narrative::Player::new_unique_id(w);
        w.fact_mut(f).unique_id = id;
    }
    let list = &mut w.entity_mut(e).facts;
    if list.list.len() == list.capacity {
        list.capacity = if list.capacity == 0 { 4 } else { list.capacity * 2 };
    }
    list.list.push(f);
    crate::combat::vision::note_corpulence_change(w, e);
    w.fact_mut(f).owner = Some(e);
    w.fact_mut(f).attach_time = w.clock.game_time;
    w.trace("Fact.Attach", |w| label(w, f));
    for c in super::comps(w, f) {
        dispatch_hook(w, c, Hook::FactAttached);
    }
    on_attach(w, f);
    if !suppress_activation {
        activate(w, f);
    }
    // `if (entityFact.IsAttached) DelegateOnFactDidAttach(entityFact)`: a fact removed at
    // the end of its own activation isn't attached any more.
    if w.fact(f).owner != Some(e) || !w.entity(e).facts.list.contains(&f) {
        return;
    }
    did_attach(w, f);
    let rank = crate::components::fact_rank(w, f);
    w.events.mutation(
        "fact_attach",
        serde_json::json!({ "owner": w.entity(e).unique_id, "fact": {"$fact": w.fact(f).unique_id, "blueprint": w.fact(f).blueprint.map(|g| g.to_string())}, "class": w.fact(f).clr, "rank": rank }),
    );
}

/// The fact class's `OnAttach` (virtual): `Etude` logs; `Quest` adds its objectives to
/// the quest book; unit fact classes on the dialogue path don't override it.
fn on_attach(w: &mut World, f: FactIdx) {
    match w.fact(f).clr {
        "Quest" => crate::narrative::quests::quest_on_attach(w, f),
        "Buff" => crate::unit_facts::buff_on_attach(w, f),
        // `MainUnitFact.OnAttach`: `new MechanicsContext(Owner, null, Blueprint, null,
        // Owner.Unit)`.
        "MainUnitFact" => {
            let owner = w.fact(f).owner;
            if let Some(bp) = w.fact(f).bp.clone() {
                let ctx = crate::unit_facts::context(w, owner, owner, &bp);
                w.fact_mut(f).context = Some(ctx);
            }
        }
        _ => {}
    }
}
