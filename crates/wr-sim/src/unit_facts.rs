//! Creating, adding and removing a unit's facts at run time, with the unit's fact
//! processors (decompiled, not copied):
//!
//! - `BlueprintUnitFact.CreateFact` and its overrides (`BlueprintFeature` → `Feature`,
//!   `BlueprintAbility` → `Ability`, `BlueprintActivatableAbility` → `ActivatableAbility`,
//!   `BlueprintBuff` → `Buff`), `Kingmaker.UnitLogic/UnitHelper.cs:602–690` (`AddFact`,
//!   `AddBuff`);
//! - `EntityFactsManager.Add`/`Remove` (`Kingmaker.EntitySystem/EntityFactsManager.cs:160–215`)
//!   with the processors `FeatureCollection` (rank merge, `Kingmaker.UnitLogic/FeatureCollection.cs`),
//!   `BuffCollection` (`RuleCanApplyBuff`, stacking, end time:
//!   `Kingmaker.UnitLogic.Buffs/BuffCollection.cs:52–190`), `AbilityCollection`,
//!   `ActivatableAbilityCollection` (default-on, `ActivatableAbilityCollection.cs:64`);
//! - `ActivatableAbility` turning on, starting and stopping, and its applied buff
//!   (`Kingmaker.UnitLogic.ActivatableAbilities/ActivatableAbility.cs:309–620, 735–830`);
//! - `Buff` attach times (`Kingmaker.UnitLogic.Buffs/Buff.cs:247, 282`) and `Remove`.
//!
//! Every branch these paths take for the units ported so far is implemented; the others
//! (parametrized features, temporarily disabled abilities, targeted or command-activated
//! toggles, ability restrictions and resources, buff stacking modes other than replace and
//! ignore) report an unsupported execution.

use std::rc::Rc;

use crate::content::{Blueprint, is_a};
use crate::context::MechanicsContext;
use crate::csharp::TimeSpan;
use crate::facts::lifecycle as fl;
use crate::world::{EntityIdx, FactIdx, FactKind, World};

/// The fact class `CreateFact` makes for a blueprint.
#[must_use]
pub fn kind_for(bp: &Blueprint) -> (FactKind, &'static str) {
    let t = bp.obj.ty;
    if is_a(t, "BlueprintBuff") {
        (FactKind::Buff, "Buff")
    } else if is_a(t, "BlueprintFeature") {
        (FactKind::Feature, "Feature")
    } else if is_a(t, "BlueprintAbility") {
        (FactKind::Ability, "Ability")
    } else if is_a(t, "BlueprintActivatableAbility") {
        (FactKind::ActivatableAbility, "ActivatableAbility")
    } else if is_a(t, "BlueprintUnit") {
        (FactKind::MainUnitFact, "MainUnitFact")
    } else {
        (FactKind::UnitFact, "UnitFact")
    }
}

fn unit_id(w: &World, u: EntityIdx) -> String {
    w.entity(u).unique_id.clone()
}

/// `new MechanicsContext(caster, owner, blueprint)`.
#[must_use]
pub fn context(
    w: &World,
    caster: Option<EntityIdx>,
    owner: Option<EntityIdx>,
    bp: &Blueprint,
) -> MechanicsContext {
    // `m_CasterRef = caster ?? owner.Unit; m_OwnerRef = owner?.Unit ?? caster`; the arrays
    // are `CountOfRanks` / `CountOfValues` long (7 each).
    MechanicsContext {
        associated_blueprint: Some(bp.guid),
        owner: owner.or(caster).map(|o| unit_id(w, o)),
        caster: caster.or(owner).map(|c| unit_id(w, c)),
        ranks: vec![0; 7],
        shared_values: vec![0; 7],
        spell_descriptor: crate::combat::rules::spell_descriptor(bp),
        ..MechanicsContext::default()
    }
}

/// `BlueprintUnitFact.CreateFact(parentContext, owner, duration)` for the non-buff
/// classes: the new fact (with its runtime components set up), not yet attached.
/// `Feature`'s context is `parentContext?.CloneFor(blueprint, owner) ?? new
/// MechanicsContext(null, owner, blueprint)`.
pub fn create_fact(
    w: &mut World,
    bp: &Rc<Blueprint>,
    owner: EntityIdx,
    parent: Option<&MechanicsContext>,
) -> FactIdx {
    let (kind, clr) = kind_for(bp);
    let f = fl::new_fact(w, kind, clr, bp.clone());
    match kind {
        FactKind::Feature => {
            let ctx = match parent {
                Some(p) => clone_for(w, p, bp, owner),
                None => context(w, None, Some(owner), bp),
            };
            w.fact_mut(f).context = Some(ctx);
            // `Param = ContextData<ParamContextData>.Current?.Param ?? Param`.
            if let Some(p) = w.feature_param_ctx.clone() {
                w.fact_mut(f).param = crate::feature_param::FeatureParam::from_json(&p)
                    .map_or(Some(p), |p| p.to_json());
            }
        }
        FactKind::ActivatableAbility => w.fact_mut(f).activatable = Some((false, false, None)),
        // `Ability(blueprint, owner)`: an `AbilityData` (caster links; no sheet state).
        // `UnitFact`: no context. `MainUnitFact`: its context is made in `OnAttach`.
        _ => {}
    }
    if parent.is_some() && !matches!(kind, FactKind::Feature) {
        w.coverage.unsupported(format!("{clr}: CreateFact with a parent context"));
    }
    f
}

/// `MechanicsContext.CloneFor(blueprint, owner)`: the parent's caster, a new owner and
/// associated blueprint, the parent kept (`ParentContext`), shared values copied (ranks start at zero).
#[must_use]
pub fn clone_for(
    w: &World,
    p: &MechanicsContext,
    bp: &Blueprint,
    owner: EntityIdx,
) -> MechanicsContext {
    MechanicsContext {
        associated_blueprint: Some(bp.guid),
        parent_associated_blueprint: p.associated_blueprint,
        owner: Some(unit_id(w, owner)),
        caster: p.caster.clone(),
        // The constructor copies shared values and properties, not ranks.
        ranks: vec![0; p.ranks.len()],
        shared_values: p.shared_values.clone(),
        // `new MechanicsContext(…, parent: this)`: no `m_Params` of its own.
        params: None,
        parent_params: p.effective_params().cloned(),
        source_item: p.source_item.clone(),
        parent_source_item: p.source_item.clone(),
        has_ability_context: p.has_ability_context,
        spell_descriptor: p.spell_descriptor | crate::combat::rules::spell_descriptor(bp),
    }
}

/// `UnitHelper.AddFact(descriptor, blueprint)`: a buff through `AddBuff` (caster = the
/// owner), anything else through `CreateFact` and the manager.
pub fn add_fact(w: &mut World, u: EntityIdx, bp: &Rc<Blueprint>) -> Option<FactIdx> {
    if kind_for(bp).0 == FactKind::Buff {
        let ctx = context(w, Some(u), Some(u), bp);
        return add_buff(w, u, bp, ctx, None);
    }
    let f = create_fact(w, bp, u, None);
    add(w, u, f)
}

/// `UnitHelper.AddFact<Feature>(blueprint, null, param)`: the param is the current
/// `Feature.ParamContextData` while the fact is created and attached (so features created while
/// it activates take it too).
pub fn add_fact_with_param(
    w: &mut World,
    u: EntityIdx,
    bp: &Rc<Blueprint>,
    param: &crate::feature_param::FeatureParam,
) -> Option<FactIdx> {
    // `(param != null) ? Request().Setup(param) : null`: `FeatureParam.operator !=` treats an
    // empty param as null, so no scope is made and an outer one stays in force.
    let Some(json) = param.to_json() else { return add_fact(w, u, bp) };
    let outer = w.feature_param_ctx.replace(json);
    let r = add_fact(w, u, bp);
    w.feature_param_ctx = outer;
    r
}

/// `BlueprintBuff.Flags` (`m_Flags`).
pub const BUFF_IS_FROM_SPELL: i64 = 1;
pub const BUFF_STAY_ON_DEATH: i64 = 8;
pub const BUFF_REMOVE_ON_RESURRECT: i64 = 0x20;

/// `BlueprintBuff.HasFlag(flag)`: `IsFromSpell`, `StayOnDeath`, `RemoveOnResurrect`...
#[must_use]
pub fn buff_flag(bp: &Blueprint, flag: i64) -> bool {
    bp.view().int("m_Flags") & flag != 0
}

/// `BuffCollection.AddBuff`: a `Buff` with `context` and `duration`, through the manager.
pub fn add_buff(
    w: &mut World,
    u: EntityIdx,
    bp: &Rc<Blueprint>,
    ctx: MechanicsContext,
    duration: Option<TimeSpan>,
) -> Option<FactIdx> {
    add_buff_with(w, u, bp, ctx, duration, |_, _| {})
}

/// [`add_buff`] with a step between the buff's creation and `Facts.Add` (`AddFacts` writes
/// the new fact's `Params.CasterLevel` there).
pub fn add_buff_with(
    w: &mut World,
    u: EntityIdx,
    bp: &Rc<Blueprint>,
    ctx: MechanicsContext,
    duration: Option<TimeSpan>,
    created: impl FnOnce(&mut World, FactIdx),
) -> Option<FactIdx> {
    let f = fl::new_fact(w, FactKind::Buff, "Buff", bp.clone());
    let fact = w.fact_mut(f);
    fact.context = Some(ctx);
    fact.rank = 1;
    fact.buff = Some(crate::world::BuffTimes::default());
    let parent = std::mem::take(&mut w.pending_buffs.parent_descriptor);
    w.pending_buffs.planned.push((f, duration, parent));
    created(w, f);
    add(w, u, f)
}

/// Buffs created but not yet attached.
#[derive(Debug, Clone, Default)]
pub struct PendingBuffs {
    /// `Buff.PlannedDuration`, with the parent context's spell descriptor.
    pub planned: Vec<(FactIdx, Option<TimeSpan>, i64)>,
    /// The parent context's spell descriptor for the next `add_buff`.
    pub parent_descriptor: i64,
}

fn planned(w: &mut World, f: FactIdx) -> (Option<TimeSpan>, i64) {
    let p = &mut w.pending_buffs.planned;
    match p.iter().position(|(x, _, _)| *x == f) {
        Some(i) => {
            let (_, d, s) = p.remove(i);
            (d, s)
        }
        None => (None, 0),
    }
}

/// `EntityFactsManager.Add(fact)` with the unit's processors.
pub fn add(w: &mut World, u: EntityIdx, f: FactIdx) -> Option<FactIdx> {
    let f = prepare_for_attach(w, u, f)?;
    if w.entity(u).facts.list.contains(&f) {
        return Some(f);
    }
    // `DelegateOnFactDidAttach` runs inside `Add`, before WrathOracle's `fact_attach`
    // record (a postfix on it): the processor's nested rules and records come first.
    fl::manager_add_attach_with(w, u, f, false, |w, f| on_fact_did_attach(w, u, f));
    Some(f)
}

/// The processors' `PrepareFactForAttach`.
fn prepare_for_attach(w: &mut World, u: EntityIdx, f: FactIdx) -> Option<FactIdx> {
    match w.fact(f).kind {
        FactKind::Feature => {
            // `FeatureCollection.PrepareFactForAttach`: a parametrized feature without a
            // feature of its blueprint and param is attached as it is; otherwise an existing
            // feature of the same blueprint takes a rank instead.
            let bp = w.fact(f).blueprint;
            let parametrized = w
                .fact(f)
                .bp
                .as_ref()
                .is_some_and(|b| is_a(b.obj.ty, "BlueprintParametrizedFeature"));
            let list: Vec<FactIdx> = w.entity(u).facts.list.clone();
            let same: Vec<FactIdx> = list
                .into_iter()
                .filter(|x| w.fact(*x).kind == FactKind::Feature && w.fact(*x).blueprint == bp)
                .collect();
            if parametrized {
                let mine = crate::feature_param::fact_param(w, f);
                let any_same_param =
                    same.iter().copied().any(|x| crate::feature_param::fact_param(w, x) == mine);
                if !any_same_param {
                    return Some(f);
                }
            }
            match same.first().copied() {
                Some(e) => {
                    feature_add_rank(w, e);
                    Some(e)
                }
                None => Some(f),
            }
        }
        FactKind::Buff => buff_prepare_for_attach(w, u, f),
        // `AbilityCollection`/`ActivatableAbilityCollection`: an existing temporarily
        // disabled fact would be turned on again; the sim's facts are never disabled
        // that way (that needs an action-bar slot), so the new fact is attached.
        _ => Some(f),
    }
}

/// `using (ContextData<ReapplyScope>.Request().Setup(fact))` (`EntityFact.cs:27–50`): the
/// fact reapplies for the body's duration. A fact that already is logs and leaves the flag
/// to its outer scope.
fn in_reapply_scope(w: &mut World, f: FactIdx, body: impl FnOnce(&mut World)) {
    if w.fact(f).reapplying {
        w.log("error", format!("EntityFact {} is already reapplying", w.fact_label(f)));
        body(w);
        return;
    }
    w.fact_mut(f).reapplying = true;
    body(w);
    w.fact_mut(f).reapplying = false;
}

/// The rank change of `Feature.AddRank`/`RemoveRank` (`Feature.cs:246–282`): deactivate,
/// change, activate, all inside the reapply scope, then `IFeatureRankHandler` on the owner.
fn change_rank(w: &mut World, f: FactIdx, gain: bool) {
    in_reapply_scope(w, f, |w| {
        let active = w.fact(f).is_active;
        if active {
            fl::deactivate(w, f);
        }
        w.fact_mut(f).rank += if gain { 1 } else { -1 };
        if active {
            fl::activate(w, f);
        }
    });
    if let Some(o) = w.fact(f).owner {
        let method = if gain { "HandleUnitGainFeatureRank" } else { "HandleUnitLostFeatureRank" };
        crate::rulebook::bus::raise_unit_fact(w, o, "IFeatureRankHandler", method, None, Some(f));
    }
}

/// `Feature.AddRank()`.
pub fn feature_add_rank(w: &mut World, f: FactIdx) {
    let max = w.fact(f).bp.as_ref().map_or(1, |b| b.view().i32("Ranks"));
    if w.fact(f).rank >= max {
        return;
    }
    change_rank(w, f, true);
}

/// `BuffCollection.PrepareFactForAttach`.
fn buff_prepare_for_attach(w: &mut World, u: EntityIdx, f: FactIdx) -> Option<FactIdx> {
    if w.unit(u).is_some_and(|x| x.buffs_disabled) {
        return None;
    }
    let bp = w.fact(f).bp.clone()?;
    let v = bp.view();
    let dead = w.unit(u).is_some_and(|x| x.life_state == 2);
    if !buff_flag(&bp, BUFF_STAY_ON_DEATH) && dead {
        return None;
    }
    let (mut duration, parent_descriptor) = planned(w, f);
    let mut rule = crate::combat::rules::RuleCanApplyBuff::new(u, bp.clone(), duration);
    // `Context.SpellDescriptor`: the parent's | the buff's `SpellDescriptorComponent`.
    rule.descriptor = parent_descriptor | crate::combat::rules::spell_descriptor(&bp);
    rule.caster = w
        .fact(f)
        .context
        .as_ref()
        .and_then(|c| c.caster.as_ref())
        .and_then(|id| w.by_id.get(id).copied());
    rule.foreign_caster = rule.caster.is_some_and(|c| c != u);
    crate::combat::rules::trigger_any(w, &mut rule, crate::combat::rules::RuleCanApplyBuff::result);
    if !rule.can_apply {
        return None;
    }
    duration = rule.duration;
    let start = crate::combat::buff_start_time(w);
    let end = duration.map(|d| {
        if d == TimeSpan::MAX { TimeSpan::MAX } else { TimeSpan(start.0.wrapping_add(d.0)) }
    });
    // `StackingType`: Replace 0? The enum: Replace, Prolong, Ignore, Stack, Poison, Summ, Rank.
    let stacking = v.int("Stacking");
    const STACK: i64 = 3;
    if stacking != STACK {
        let existing =
            w.entity(u).facts.list.iter().copied().find(|x| {
                w.fact(*x).kind == FactKind::Buff && w.fact(*x).blueprint == Some(bp.guid)
            });
        if let Some(e) = existing {
            match stacking {
                0 => remove(w, u, e),
                2 => return Some(e),
                s => {
                    w.coverage.unsupported(format!("BuffCollection: stacking type {s}"));
                    return Some(e);
                }
            }
        }
    }
    if let Some(end) = end
        && let Some(b) = &mut w.fact_mut(f).buff
    {
        b.end_time = if end == TimeSpan::MAX { None } else { Some(end.0) };
    }
    Some(f)
}

/// Whether a fact class has a fact processor on the unit (`FeatureCollection`,
/// `BuffCollection`, `AbilityCollection`, `ActivatableAbilityCollection`: the
/// `UnitLogicCollection`s); `UnitFact` and `MainUnitFact` have none.
fn has_processor(kind: FactKind) -> bool {
    matches!(
        kind,
        FactKind::Feature | FactKind::Buff | FactKind::Ability | FactKind::ActivatableAbility
    )
}

/// `UnitLogicCollection.OnFactDidAttach` / `OnFactWillDetach` (`UnitLogicCollection.cs:26–61`):
/// the owner's `IUnitGainFactHandler`/`IUnitLostFactHandler`, then, if the owner is in the
/// party, `IUnitInPartyGainFactHandler`/`IUnitInPartyLostFactHandler` on every party member.
fn unit_logic_fact_event(w: &mut World, u: EntityIdx, f: FactIdx, gain: bool) {
    let (iface, method) = if gain {
        ("IUnitGainFactHandler", "HandleUnitGainFact")
    } else {
        ("IUnitLostFactHandler", "HandleUnitLostFact")
    };
    crate::rulebook::bus::raise_unit_fact(w, u, iface, method, None, Some(f));
    let party = crate::narrative::party::party(w);
    if !party.contains(&u) {
        return;
    }
    let (iface, method) = if gain {
        ("IUnitInPartyGainFactHandler", "HandleUnitInPartyGainFact")
    } else {
        ("IUnitInPartyLostFactHandler", "HandleUnitInPartyLostFact")
    };
    for unit in party {
        crate::rulebook::bus::raise_unit_fact(w, unit, iface, method, Some(unit), Some(f));
    }
}

/// The base processor's closing `IFactCollectionUpdatedHandler.HandleFactCollectionUpdated`
/// (`EntityFactsProcessor.cs:96–118`), after the collection's own handling.
fn fact_collection_updated(w: &mut World) {
    crate::rulebook::bus::raise_global(w, "IFactCollectionUpdatedHandler", None);
}

/// `AbilityCollection.OnFactDidAttach`/`OnFactWillDetach` after the base
/// (`AbilityCollection.cs:92–130`): for a player-faction owner, the action-panel auto-cast
/// check (not ported: reported) or the UI slot removal (presentation), and
/// `IPlayerAbilitiesHandler`.
/// `GetComponent<ActionPanelLogic>()` with `AutoCastConditions.HasConditions`: the array's
/// length, null entries included.
fn has_auto_cast_conditions(bp: &Blueprint) -> bool {
    bp.components_array().iter().find(|o| is_a(o.ty, "ActionPanelLogic")).is_some_and(|o| {
        crate::content::View(o)
            .obj("AutoCastConditions")
            .is_some_and(|c| !c.seq("Conditions").is_empty())
    })
}

fn ability_collection_event(w: &mut World, u: EntityIdx, f: FactIdx, gain: bool) {
    if !w.unit(u).is_some_and(|x| x.is_player_faction) {
        return;
    }
    // `GetComponent<ActionPanelLogic>()` with `AutoCastConditions.HasConditions`: checks
    // them and, if they hold, sets `Brain.AutoUseAbility` (the AI brain isn't modelled).
    // Without conditions nothing happens.
    if gain
        && let Some(bp) = w.fact(f).bp.clone()
        && has_auto_cast_conditions(&bp)
    {
        w.coverage.unsupported("AbilityCollection.OnFactDidAttach: ActionPanelLogic auto-cast");
    }
    let method = if gain { "HandleAbilityAdded" } else { "HandleAbilityRemoved" };
    crate::rulebook::bus::raise_global_fact(w, "IPlayerAbilitiesHandler", method, None, Some(f));
}

/// The processors' `OnFactDidAttach` (`DelegateOnFactDidAttach`): `UnitLogicCollection`
/// raises the gain events; then the collection's own handling (abilities: player-faction
/// events; activatables go on by default; buffs update the next event and raise
/// `IUnitBuffHandler.HandleBuffDidAdded`); then `IFactCollectionUpdatedHandler`.
fn on_fact_did_attach(w: &mut World, u: EntityIdx, f: FactIdx) {
    let kind = w.fact(f).kind;
    if !has_processor(kind) {
        return;
    }
    unit_logic_fact_event(w, u, f, true);
    match kind {
        FactKind::Ability => ability_collection_event(w, u, f, true),
        FactKind::ActivatableAbility => {
            let by_default = w.fact(f).bp.as_ref().is_some_and(|b| b.view().bool("IsOnByDefault"));
            if by_default {
                set_is_on(w, f, true);
            }
        }
        FactKind::Buff => {
            // `UpdateNextEvent` (the buff tick queue isn't modelled: ticks are reported
            // where they would run).
            crate::rulebook::bus::raise_global_fact(
                w,
                "IUnitBuffHandler",
                "HandleBuffDidAdded",
                None,
                Some(f),
            );
        }
        _ => {}
    }
    fact_collection_updated(w);
}

/// `Buff.OnRemove` (`Buff.cs`): the stored modifiers (not modelled: reported) and the stored
/// facts are removed from the owner (`Owner.RemoveFact(fact)`, each), then the lists cleared.
/// The removal effect is presentation; `IBuffRemoved` components aren't called here.
fn buff_on_remove(w: &mut World, u: EntityIdx, f: FactIdx) {
    let Some(b) = w.fact_mut(f).buff.as_mut() else { return };
    let stored = std::mem::take(&mut b.stored_facts);
    let mods = std::mem::take(&mut b.stored_mods);
    if mods {
        w.coverage.unsupported("Buff.OnRemove: stored modifiers");
    }
    for s in stored {
        remove(w, u, s);
    }
}

/// The processors' `OnFactWillDetach` (`DelegateOnFactWillDetach`, after WrathOracle's
/// `fact_detach` record): the lost events; abilities' player-faction events; buffs run
/// `OnRemove` (`buff_on_remove`), update the next event and raise
/// `IUnitBuffHandler.HandleBuffDidRemoved` (activatables whose applied buff it is stop); then
/// `IFactCollectionUpdatedHandler`.
fn on_fact_will_detach(w: &mut World, u: EntityIdx, f: FactIdx) {
    let kind = w.fact(f).kind;
    if !has_processor(kind) {
        return;
    }
    unit_logic_fact_event(w, u, f, false);
    match kind {
        FactKind::Ability => ability_collection_event(w, u, f, false),
        FactKind::Buff => {
            buff_on_remove(w, u, f);
            crate::rulebook::bus::raise_global_fact(
                w,
                "IUnitBuffHandler",
                "HandleBuffDidRemoved",
                None,
                Some(f),
            );
        }
        _ => {}
    }
    fact_collection_updated(w);
}

/// `ActivatableAbility`'s `EventBus` handlers (`ActivatableAbility.cs:412–480`, `:731–745`,
/// `:899–913`) for one subscribed ability, the method and its arguments read from the bus;
/// whether `iface`'s method is one it implements.
pub fn activatable_event(w: &mut World, a: FactIdx, iface: &str) -> bool {
    let Some((method, unit)) = w.bus.call.last().copied() else { return false };
    let cmd = w.bus.command.last().copied();
    match (iface, method) {
        ("IUnitBuffHandler", "HandleBuffDidAdded")
        | ("IUnitCombatHandler", "HandleUnitLeaveCombat")
        | ("IApplyAbilityEffectHandler", "OnAbilityEffectApplied" | "OnTryToApplyAbilityEffect") => {
        }
        ("IUnitBuffHandler", "HandleBuffDidRemoved") => {
            let Some(Some(buff)) = w.bus.call_fact.last().copied() else { return false };
            if let Some((_, true, Some(b))) = w.fact(a).activatable
                && b == buff
            {
                set_is_on(w, a, false);
                if act(w, a).1 {
                    stop(w, a);
                }
                w.fact_mut(a).deactivate_next_round = false;
            }
        }
        ("IUnitCombatHandler", "HandleUnitJoinCombat") => {
            let Some(bp) = ability_bp(w, a) else { return true };
            let v = bp.view();
            // `ActivateOnCombatStarts && m_IsOn && !IsStarted && unit == Owner.Unit &&
            // ActivateImmediately`.
            let (on, started, _) = act(w, a);
            if v.bool("DeactivateIfCombatEnded")
                && v.int("ActivationType") == 0
                && on
                && !started
                && unit.is_some()
                && unit == w.fact(a).owner
            {
                try_start(w, a);
            }
        }
        ("IApplyAbilityEffectHandler", "OnAbilityEffectAppliedToTarget") => {
            // `IsWaitingForTarget && context.Ability.Fact == SelectTargetAbility`: targeted
            // toggles aren't modelled.
            if ability_bp(w, a).is_some_and(|b| b.view().bool("IsTargeted")) && act(w, a).0 {
                w.coverage.unsupported(
                    "ActivatableAbility.OnAbilityEffectAppliedToTarget: a targeted toggle",
                );
            }
        }
        ("IUnitRunCommandHandler", "HandleUnitRunCommand") => {
            let Some(c) = cmd else { return false };
            activatable_run_command(w, a, c);
        }
        ("IUnitCommandActHandler", "HandleUnitCommandDidAct") => {
            let Some(c) = cmd else { return false };
            activatable_command_did_act(w, a, c);
        }
        ("IUnitCommandEndHandler", "HandleUnitCommandDidEnd") => {
            let Some(c) = cmd else { return false };
            activatable_command_did_end(w, a, c);
        }
        _ => return false,
    }
    true
}

/// `ActivatableAbilityUnitCommand.Type` of the ability's blueprint (`GetComponent`), if it has
/// one.
fn unit_command_type(bp: &Blueprint) -> Option<i64> {
    bp.components_array()
        .into_iter()
        .find(|o| is_a(o.ty, "ActivatableAbilityUnitCommand"))
        .map(|o| crate::content::View(o).int("Type"))
}

/// `HandleUnitRunCommand(command)`: a command of its unit-command type, run by its owner,
/// turns it off (unless the command activates this ability).
fn activatable_run_command(w: &mut World, a: FactIdx, cmd: crate::rulebook::bus::CommandArg) {
    if !act(w, a).0 || cmd.executor != w.fact(a).owner {
        return;
    }
    let Some(bp) = ability_bp(w, a) else { return };
    if unit_command_type(&bp) != Some(i64::from(cmd.ty)) {
        return;
    }
    if cmd.activate_ability {
        w.coverage.unsupported("ActivatableAbility.HandleUnitRunCommand: UnitActivateAbility");
        return;
    }
    set_is_on(w, a, false);
}

/// `HandleUnitCommandDidAct(command)`: an ability activated with a unit command that is on and
/// not started turns off when its owner acts with another command of its type.
fn activatable_command_did_act(w: &mut World, a: FactIdx, cmd: crate::rulebook::bus::CommandArg) {
    let (on, started, _) = act(w, a);
    if cmd.executor != w.fact(a).owner || !on {
        return;
    }
    let Some(bp) = ability_bp(w, a) else { return };
    if unit_command_type(&bp).is_some_and(|t| t != i64::from(cmd.ty))
        || started
        || bp.view().int("ActivationType") != 1
    {
        return;
    }
    if cmd.activate_ability {
        w.coverage.unsupported("ActivatableAbility.HandleUnitCommandDidAct: UnitActivateAbility");
        return;
    }
    set_is_on(w, a, false);
}

/// `HandleUnitCommandDidEnd(command)`: acts only on a `UnitActivateAbility`.
fn activatable_command_did_end(w: &mut World, a: FactIdx, cmd: crate::rulebook::bus::CommandArg) {
    let (on, started, _) = act(w, a);
    if !on || started || cmd.executor != w.fact(a).owner || !cmd.activate_ability {
        return;
    }
    if ability_bp(w, a).is_some_and(|b| b.view().int("ActivationType") == 1) {
        w.coverage.unsupported("ActivatableAbility.HandleUnitCommandDidEnd: UnitActivateAbility");
    }
}

/// `ActivatableAbility`'s initiator rule handlers (`ActivatableAbility.cs:614–672`) for one
/// subscribed ability; whether it handles `rule` at `step`.
pub fn activatable_rule(
    w: &mut World,
    a: FactIdx,
    rule: &mut dyn crate::rulebook::RuleEvent,
    step: crate::rulebook::bus::RuleStep,
) -> bool {
    use crate::combat::attack::{
        RuleAttackWithWeapon, RuleAttackWithWeaponResolve, RuleDealDamage,
    };
    use crate::rulebook::bus::RuleStep;
    let about = match step {
        RuleStep::AboutToTrigger => true,
        RuleStep::DidTrigger => false,
        _ => return false,
    };
    // `m_IsOn && ActivateOnUnitAction && ActivateOnUnitActionType == action`.
    let on_unit_action = |w: &World, action: i64| {
        act(w, a).0
            && ability_bp(w, a).is_some_and(|b| {
                b.view().int("ActivationType") == 2
                    && b.view().int("m_ActivateOnUnitAction") == action
            })
    };
    match rule.chain()[0] {
        "RuleAttackWithWeapon" => {
            let Some(r) = rule.as_any().downcast_mut::<RuleAttackWithWeapon>() else {
                return false;
            };
            if about {
                if on_unit_action(w, 0) {
                    try_start(w, a);
                    w.fact_mut(a).deactivate_next_round = false;
                    w.fact_mut(a).time_to_next_round = 0.0;
                }
            } else if !r.weapon.is_ranged() {
                activatable_on_attack(w, a, Spend::Attack);
            }
        }
        "RuleDealDamage" => {
            let Some(r) = rule.as_any().downcast_mut::<RuleDealDamage>() else { return false };
            // `Reason.Rule is RuleAttackWithWeapon`: a weapon attack's damage
            // (`CreateRuleDealDamage` sets `Reason = this`).
            if !about && r.link.weapon_attack {
                activatable_on_attack(w, a, Spend::Hit);
                // `AttackRoll.IsCriticalConfirmed && !FortificationNegatesCriticalHit`
                // (fortification is reported by the roll).
                if r.link.roll_seq.is_some() && r.link.is_critical_confirmed {
                    activatable_on_attack(w, a, Spend::Crit);
                }
            }
        }
        "RuleAttackWithWeaponResolve" => {
            let Some(r) = rule.as_any().downcast_mut::<RuleAttackWithWeaponResolve>() else {
                return false;
            };
            if !about && !r.weapon_melee {
                activatable_on_attack(w, a, Spend::Attack);
            }
        }
        "RuleCastSpell" => {
            if about && on_unit_action(w, 1) {
                try_start(w, a);
            }
        }
        _ => return false,
    }
    true
}

/// Which of `OnAttack`, `OnHit` and `OnCrit`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Spend {
    Attack,
    Hit,
    Crit,
}

/// `OnAttack`/`OnHit`/`OnCrit`: a started ability's resource logic
/// (`IActivatableAbilitySpendResourceLogic`, not ported: reported), then a stop when it can't
/// go on and deactivates immediately (`OnAttack` also when it isn't active).
fn activatable_on_attack(w: &mut World, a: FactIdx, spend: Spend) {
    let (on, started, _) = act(w, a);
    if !on || !started {
        return;
    }
    let Some(bp) = ability_bp(w, a) else { return };
    let available = is_available(w, a);
    if available && has_component(&bp, "IActivatableAbilitySpendResourceLogic") {
        w.coverage
            .unsupported("ActivatableAbility: IActivatableAbilitySpendResourceLogic on an attack");
    }
    let inactive = spend == Spend::Attack && !w.fact(a).is_active;
    if (inactive || !available) && bp.view().bool("DeactivateImmediately") {
        stop(w, a);
    }
}

/// `EntityFactsManager.Remove(fact)` with the unit's processors.
pub fn remove(w: &mut World, u: EntityIdx, f: FactIdx) {
    if w.fact(f).activating {
        w.fact_mut(f).remove_when_activation_ended = true;
        return;
    }
    // `PrepareFactForDetach`: features above rank 1 lose a rank instead.
    if w.fact(f).kind == FactKind::Feature && w.fact(f).rank >= 2 {
        change_rank(w, f, false);
        return;
    }
    // Out of the list, the `fact_detach` record, `OnFactWillDetach`, `Detach`, `Dispose`.
    fl::manager_remove_with(w, u, f, true, |w, f| on_fact_will_detach(w, u, f));
}

// ---- activatable abilities ----------------------------------------------------------

fn act(w: &World, f: FactIdx) -> (bool, bool, Option<FactIdx>) {
    w.fact(f).activatable.unwrap_or((false, false, None))
}

fn set_act(w: &mut World, f: FactIdx, v: (bool, bool, Option<FactIdx>)) {
    w.fact_mut(f).activatable = Some(v);
}

fn ability_bp(w: &World, f: FactIdx) -> Option<Rc<Blueprint>> {
    w.fact(f).bp.clone()
}

/// `ActivatableAbility.IsOn` setter (`SetIsOn(value, null)`); turn-on conditions aren't
/// modelled (a fact with restriction components reports).
pub fn set_is_on(w: &mut World, f: FactIdx, value: bool) {
    let (on, started, buff) = act(w, f);
    if on == value {
        return;
    }
    set_act(w, f, (value, started, buff));
    if value {
        on_did_turn_on(w, f);
    } else {
        // `OnDidTurnOff`: `m_Target = null` (targets aren't modelled), `ReadyToStart = false`;
        // a started ability stops now if it deactivates immediately, else its next round does
        // (`OnNewRound`, as it is off), and in turn-based combat its
        // `DeactivateImmediatelyIfNoAttacksThisRound` components act (not ported: reported).
        w.fact_mut(f).ready_to_start = false;
        if !started {
            return;
        }
        let Some(bp) = ability_bp(w, f) else { return };
        if bp.view().bool("DeactivateImmediately") {
            stop(w, f);
        } else if w.combat.turn_based
            && has_component(&bp, "DeactivateImmediatelyIfNoAttacksThisRound")
        {
            w.coverage.unsupported(
                "ActivatableAbility.OnDidTurnOff: DeactivateImmediatelyIfNoAttacksThisRound",
            );
        }
    }
}

/// `ActivatableAbility.OnDidTurnOn`.
fn on_did_turn_on(w: &mut World, f: FactIdx) {
    let Some(bp) = ability_bp(w, f) else { return };
    let v = bp.view();
    if v.bool("IsTargeted") {
        w.coverage.unsupported("ActivatableAbility: targeted (IsWaitingForTarget)");
        return;
    }
    let group = v.int("Group");
    if group != 0 {
        // Groups over capacity turn the earliest-on abilities off; with one ability on
        // per group (all the units here) nothing changes. More than one is reported.
        let owner = w.fact(f).owner;
        let on = owner.map_or(0, |o| {
            w.entity(o)
                .facts
                .list
                .iter()
                .filter(|x| {
                    w.fact(**x).kind == FactKind::ActivatableAbility
                        && w.fact(**x).bp.as_ref().is_some_and(|b| b.view().int("Group") == group)
                        && act(w, **x).0
                })
                .count()
        });
        if on > 1 {
            w.coverage.unsupported("ActivatableAbility.OnDidTurnOn: group over capacity");
        }
    }
    if act(w, f).1 {
        return;
    }
    // `IActivatableAbilitySpendResourceLogic.OnAbilityTurnOn`: empty for its one implementer on
    // toggles (`ActivatableAbilityResourceLogic`); a unit command's logic isn't ported.
    if has_component(&bp, "ActivatableAbilityUnitCommand") {
        w.coverage.unsupported("ActivatableAbility: resource or unit-command components");
    }
    // `ActivateImmediately`: `ActivationType == Immediately` (0).
    if v.int("ActivationType") == 0 {
        try_start(w, f);
    } else {
        w.coverage.unsupported("ActivatableAbility: activation with a unit command");
    }
}

pub fn has_component(bp: &Blueprint, ty: &str) -> bool {
    // `IActivatableAbilitySpendResourceLogic` isn't in the content schema's handler lists: its
    // implementer on toggles is `ActivatableAbilityResourceLogic`.
    if ty == "IActivatableAbilitySpendResourceLogic"
        && bp.components_array().iter().any(|o| is_a(o.ty, "ActivatableAbilityResourceLogic"))
    {
        return true;
    }
    bp.components_array().iter().any(|o| {
        is_a(o.ty, ty) || wr_content::schema::ty(o.ty).handlers.iter().any(|h| h.contains(ty))
    })
}

/// `ActivatableAbility.IsAvailable`: available by resources and by restrictions (and not
/// `TemporarilyDisabled`, which the sim never sets).
pub fn is_available(w: &mut World, f: FactIdx) -> bool {
    is_available_by_resources(w, f) && is_available_by_restrictions(w, f)
}

/// The fact's blueprint components of a type (`SelectComponentsWithRuntime<T>`: the enabled
/// ones), in order.
fn components_of(w: &mut World, f: FactIdx, ty: &str) -> Vec<(Rc<Blueprint>, usize)> {
    // `BlueprintFact.CollectComponents()` (`ComponentsList`s expanded), the enabled ones.
    w.blueprint_components(f)
        .into_iter()
        .filter(|(b, i)| {
            let o = b.components_array()[*i];
            is_a(o.ty, ty) && crate::content::View(o).int("m_Flags") & 1 == 0
        })
        .collect()
}

/// `Owner.HasFact(blueprint)`.
fn owner_has_fact(w: &World, u: EntityIdx, g: wr_content::Guid) -> bool {
    w.entity(u).facts.list.iter().any(|f| w.fact(*f).blueprint == Some(g))
}

/// `IsAvailableByResources`: every `ActivatableAbilityResourceLogic` allows it.
pub fn is_available_by_resources(w: &mut World, f: FactIdx) -> bool {
    let Some(u) = w.fact(f).owner.filter(|o| w.unit(*o).is_some()) else { return true };
    for (b, i) in components_of(w, f, "ActivatableAbilityResourceLogic") {
        if !resource_logic_available(w, f, u, crate::content::View(b.components_array()[i])) {
            return false;
        }
    }
    true
}

/// `ActivatableAbilityResourceLogic.IsFreeBecauseOfFact`.
fn free_because_of_fact(w: &World, u: EntityIdx, v: crate::content::View<'_>) -> bool {
    if v.bool("m_UseListForFreeBlueprints") {
        v.guids_with_nulls("m_FreeBlueprints")
            .into_iter()
            .flatten()
            .any(|g| w.content.get(g).is_some() && owner_has_fact(w, u, g))
    } else {
        v.guid("m_FreeBlueprint")
            .filter(|g| w.content.get(*g).is_some())
            .is_some_and(|g| owner_has_fact(w, u, g))
    }
}

/// `ActivatableAbilityResourceLogic.SpendType`: `None`, `Judgment`.
const SPEND_NONE: i64 = 0;
const SPEND_JUDGMENT: i64 = 5;
/// `MechanicsFeatureType.JudgmentSwitch`.
const JUDGMENT_SWITCH: i64 = 7;

/// `ActivatableAbilityResourceLogic.IsAvailable(runtime)`: free because of a fact; else enough
/// of the required resource (one unless the spend type is `None`, or `Judgment` with the
/// owner's `JudgmentSwitch`); else the source item's charges (not modelled: reported).
fn resource_logic_available(
    w: &mut World,
    f: FactIdx,
    u: EntityIdx,
    v: crate::content::View<'_>,
) -> bool {
    if free_because_of_fact(w, u, v) {
        return true;
    }
    if let Some(r) = v.guid("m_RequiredResource").filter(|g| w.content.get(*g).is_some()) {
        let spend = v.int("SpendType");
        let amount = if spend == SPEND_JUDGMENT {
            i32::from(
                spend != SPEND_NONE && !crate::combat::weapon::has_feature(w, u, JUDGMENT_SWITCH),
            )
        } else {
            i32::from(spend != SPEND_NONE)
        };
        // `Resources.HasEnoughResource(resource, amount)`: no resource is false.
        return w.unit(u).and_then(|x| x.resources.get(&r)).is_some_and(|x| x.amount >= amount);
    }
    if w.fact(f).source_item.is_some() {
        w.coverage.unsupported("ActivatableAbilityResourceLogic.IsAvailable: source item charges");
        return true;
    }
    false
}

/// `IsAvailableByRestrictions`: not an `OnlyInCombat` toggle out of combat, every restriction
/// allows it (those with `IgnoreIfStarted` don't count once started), and no polymorph restricts
/// it. Restriction types without a port and a toggle whose buff polymorphs are reported.
pub fn is_available_by_restrictions(w: &mut World, f: FactIdx) -> bool {
    let Some(bp) = ability_bp(w, f) else { return true };
    let Some(u) = w.fact(f).owner.filter(|o| w.unit(*o).is_some()) else { return true };
    if bp.view().bool("OnlyInCombat") && !crate::combat::is_in_combat(w, u) {
        return false;
    }
    let started = act(w, f).1;
    for (b, i) in components_of(w, f, "ActivatableAbilityRestriction") {
        let v = crate::content::View(b.components_array()[i]);
        if started && v.bool("IgnoreIfStarted") {
            continue;
        }
        if !restriction_available(w, u, v) {
            return false;
        }
    }
    // `IsPolymorphRestricted()`: the toggle's buff polymorphs and another polymorph is active.
    if bp
        .view()
        .guid("m_Buff")
        .and_then(|g| w.content.get(g))
        .is_some_and(|b| b.components_array().iter().any(|o| is_a(o.ty, "Polymorph")))
    {
        w.coverage.unsupported("ActivatableAbility.IsPolymorphRestricted: a polymorph buff");
    }
    true
}

/// `ActivatableAbilityRestriction.IsAvailable(runtime)` of one restriction.
fn restriction_available(w: &mut World, u: EntityIdx, v: crate::content::View<'_>) -> bool {
    match v.type_name() {
        // `KensaiChosenWeapon.CheckWeaponEquipped(Owner, ChosenWeaponBlueprint)`.
        "RestrictionKensaiWeapon" => {
            kensai_weapon_equipped(w, u, v.guid("m_ChosenWeaponBlueprint"))
        }
        other => {
            w.coverage.unsupported(format!("ActivatableAbility restriction {other} (IsAvailable)"));
            true
        }
    }
}

/// `KensaiChosenWeapon.CheckWeaponEquipped(unit, feature)`: the chosen category
/// (`GetChosenWeapon`: the unit's first fact of the feature, if it has a `KensaiChosenWeapon`
/// component, and its param's weapon category) is a hand's or an additional limb's weapon's.
fn kensai_weapon_equipped(w: &mut World, u: EntityIdx, feature: Option<wr_content::Guid>) -> bool {
    let fact = feature.and_then(|g| {
        w.entity(u)
            .facts
            .list
            .iter()
            .copied()
            .find(|f| w.fact(*f).kind == FactKind::Feature && w.fact(*f).blueprint == Some(g))
    });
    let chosen = match fact {
        Some(f)
            if w.fact(f).bp.as_ref().is_some_and(|b| {
                b.components_array().iter().any(|o| is_a(o.ty, "KensaiChosenWeapon"))
            }) =>
        {
            crate::feature_param::fact_param(w, f).weapon_category
        }
        _ => None,
    };
    let Some(chosen) = chosen else { return false };
    let category = |w: &World, i: Option<EntityIdx>| {
        i.and_then(|i| crate::combat::weapon::Weapon::of(w, i)).map(|x| x.category())
    };
    // `PrimaryHand.HasWeapon && PrimaryHand.Weapon…`, the secondary hand, each limb.
    let hand = |w: &mut World, primary: bool| {
        if crate::items::hand_has_weapon(w, u, primary) {
            crate::items::hand_maybe_weapon(w, u, primary)
        } else {
            None
        }
    };
    let primary = hand(w, true);
    let secondary = hand(w, false);
    category(w, primary) == Some(chosen)
        || category(w, secondary) == Some(chosen)
        || crate::items::limb_slots(w, u)
            .into_iter()
            .any(|(has, l)| has && l.is_some() && category(w, l) == Some(chosen))
}

/// `ActivatableAbility.TryStart()` (`ActivatableAbility.cs:492`).
pub fn try_start(w: &mut World, f: FactIdx) {
    let Some(bp) = ability_bp(w, f) else { return };
    let (on, started, buff) = act(w, f);
    // `IsStarted || IsWaitingForTarget || !IsAvailable`.
    let waiting = bp.view().bool("IsTargeted") && on;
    if started || waiting || !is_available(w, f) {
        return;
    }
    w.fact_mut(f).ready_to_start = true;
    let owner = w.fact(f).owner;
    let v = bp.view();
    // `ActivateOnCombatStarts`: `DeactivateIfCombatEnded && !ActivateWithUnitCommand &&
    // !ActivateOnUnitAction`.
    let activation = v.int("ActivationType");
    let on_combat_starts = v.bool("DeactivateIfCombatEnded") && activation == 0;
    if on_combat_starts && !owner.is_some_and(|o| crate::combat::is_in_combat(w, o)) {
        return;
    }
    if has_component(&bp, "ActivatableAbilityUnitCommand") {
        w.coverage.unsupported("ActivatableAbility.TryStart: ActivatableAbilityUnitCommand");
    }
    // The group's capacity: the weights of the started or on members must not exceed the
    // group size (`UnitPartActivatableAbility.GetGroupSize`, 1 without the part).
    let group = v.int("Group");
    if group != 0
        && let Some(o) = owner
    {
        let mut num = 0i32;
        for x in w.entity(o).facts.list.clone() {
            if w.fact(x).kind != FactKind::ActivatableAbility {
                continue;
            }
            let Some(xb) = w.fact(x).bp.clone() else { continue };
            let (xon, xstarted, _) = act(w, x);
            if xb.view().int("Group") == group && (xstarted || xon) {
                num = num.wrapping_add(xb.view().i32("WeightInGroup"));
            }
        }
        // `UnitPartActivatableAbility.GetGroupSize(group)`: `m_GroupsSizeIncreases[group] + 1`,
        // 1 without the part.
        let mut size = 1;
        if w.entity(o).parts.iter().any(|p| p.clr == "UnitPartActivatableAbility") {
            match usize::try_from(group)
                .ok()
                .and_then(|g| w.unit(o).and_then(|x| x.group_size_increases.get(g)))
            {
                Some(n) => size = n + 1,
                None => w.coverage.unsupported("UnitPartActivatableAbility.GetGroupSize"),
            }
        }
        if num > size {
            return;
        }
    }
    // `m_WasInCombat = Owner.Unit.IsInCombat` before `IsStarted = true`.
    w.fact_mut(f).was_in_combat = owner.is_some_and(|o| crate::combat::is_in_combat(w, o));
    set_act(w, f, (on, true, buff));
    reapply_buff(w, f);
    // `IActivatableAbilitySpendResourceLogic.OnStart`: `ActivatableAbilityResourceLogic` spends
    // for `TurnOn`, `Start` and the once-per spend types (not ported: reported).
    // (`SpendResource` returns at once when free because of a fact.)
    if let Some(u) = owner
        && components_of(w, f, "ActivatableAbilityResourceLogic").iter().any(|(b, i)| {
            let v = crate::content::View(b.components_array()[*i]);
            matches!(v.int("SpendType"), 1 | 2 | 10..=12) && !free_because_of_fact(w, u, v)
        })
    {
        w.coverage.unsupported("ActivatableAbilityResourceLogic.OnStart: spending a resource");
    }
    // `IActivatableAbilityStartHandler` (UI and action bar).
    w.fact_mut(f).time_to_next_round = 0.0;
    w.fact_mut(f).ready_to_start = false;
}

/// `ActivatableAbility.Stop()` (no lingering performance or command-delayed removal).
pub fn stop(w: &mut World, f: FactIdx) {
    let (on, started, buff) = act(w, f);
    if !started {
        return;
    }
    set_act(w, f, (on, false, None));
    // `m_WasInCombat = false`, `TimeToNextRound = 0f`.
    w.fact_mut(f).was_in_combat = false;
    w.fact_mut(f).time_to_next_round = 0.0;
    if let Some(b) = buff
        && let Some(o) = w.fact(b).owner
    {
        remove(w, o, b);
    }
    let Some(bp) = ability_bp(w, f) else { return };
    // `if (m_IsOn && (ActivateWithUnitCommand || !IsAvailableByRestrictions)) m_IsOn = false`
    // (`IsStarted` is already false).
    let with_command = bp.view().int("ActivationType") == 1;
    if on && (with_command || !is_available_by_restrictions(w, f)) {
        set_act(w, f, (false, false, None));
    }
}

/// `ActivatableAbility.ReapplyBuff()`.
pub fn reapply_buff(w: &mut World, f: FactIdx) {
    let (on, started, applied) = act(w, f);
    if !on || !started {
        return;
    }
    let Some(owner) = w.fact(f).owner else { return };
    if let Some(b) = applied {
        set_act(w, f, (on, started, None));
        if w.fact(b).owner.is_some() {
            remove(w, owner, b);
        }
    }
    let Some(bp) = ability_bp(w, f) else { return };
    let Some(buff_bp) = bp.view().guid("m_Buff").and_then(|g| w.content.get(g)) else { return };
    if bp.view().int("Group") == 5 {
        w.coverage.unsupported("ActivatableAbility.ReapplyBuff: bardic performance");
    }
    // `new MechanicsContext(Owner.Unit, null, Blueprint)` then `CloneFor(buff, owner)`.
    let parent = context(w, Some(owner), None, &bp);
    let ctx = clone_for(w, &parent, &buff_bp, owner);
    w.pending_buffs.parent_descriptor = crate::combat::rules::spell_descriptor(&bp);
    let b = add_buff(w, owner, &buff_bp, ctx, None);
    let (on, started, _) = act(w, f);
    set_act(w, f, (on, started, b));
}

/// Component types implementing `ITickEachRound` (every implementer in the decompiled
/// code).
const TICK_EACH_ROUND: &[&str] = &[
    "AddFactContextActions",
    "ShifterGrabInitiatorBuff",
    "AddRunwayLogic",
    "AddEffectContextFastHealing",
    "AddEffectRegeneration",
    "SetBuffOnsetDelay",
    "AddEffectFastHealing",
    "ImpatienceWatcherTickingResolve",
    "TargetChangedDuringRound",
    "BuffSaveEachRound",
    "AfterbuffIfHasFact",
    "BuffPoisonStatDamageContext",
    "BuffSleeping",
    "Afterbuff",
    "DamageOverTime",
    "BuffStatusCondition",
    "HealOverTimeIfHasFact",
    "BuffSaveOrDieEachRound",
    "BuffPoisonStatDamage",
    "HealOverTime",
    "BuffParticleEffectPlay",
    "BuffDamageEachRound",
];

/// Whether a component type implements `ITickEachRound`.
#[must_use]
pub fn ticks_each_round(name: &str) -> bool {
    TICK_EACH_ROUND.contains(&name)
}

/// `Buff.OnAttach`'s timing: `TickTime` from the blueprint (rounds: 6 s), the next tick
/// one tick after the attach (or turn start in turn-based combat), `IsFromSpell`.
pub fn buff_on_attach(w: &mut World, f: FactIdx) {
    let Some(bp) = w.fact(f).bp.clone() else { return };
    if bp.components_array().iter().any(|o| is_a(o.ty, "SetBuffOnsetDelay")) {
        w.coverage.unsupported("Buff.OnAttach: SetBuffOnsetDelay");
    }
    let start = crate::combat::buff_start_time(w);
    // `BlueprintBuff.TickTime`: `TimeSpan.MaxValue` without an `ITickEachRound` component;
    // else one second (`TickEachSecond`) or `Frequency.ToRounds()` rounds of 6 s.
    let ticks = bp.components_array().iter().any(|o| TICK_EACH_ROUND.iter().any(|t| is_a(o.ty, t)));
    let v = bp.view();
    let tick = if !ticks {
        i64::MAX
    } else if v.bool("TickEachSecond") {
        10_000_000
    } else {
        let rounds: i64 = match v.int("Frequency") {
            0 => 1,
            1 => 10,
            2 => 100,
            3 => 600,
            _ => 14_400,
        };
        rounds * 60_000_000
    };
    let from_spell = buff_flag(&bp, BUFF_IS_FROM_SPELL);
    if let Some(b) = &mut w.fact_mut(f).buff {
        b.tick_time = tick;
        b.next_tick_time = if tick == i64::MAX { i64::MAX } else { start.0.wrapping_add(tick) };
        b.is_from_spell = from_spell;
    }
}

#[cfg(test)]
mod gate_tests {
    use std::rc::Rc;

    use wr_content::node::Node;

    use crate::content::Blueprint;
    use crate::facts::tests::{feature, obj};
    use crate::feature_param::FeatureParam;

    #[test]
    fn auto_cast_gate_counts_null_conditions() {
        let (w, _u) = crate::stats::tests::world_with_unit();
        let panel = |conds: Vec<Node>| {
            let checker = Node::Object(Box::new(obj(
                "Kingmaker.ElementsSystem.ConditionsChecker",
                &[("Conditions", Node::Seq(conds))],
            )));
            Node::Object(Box::new(obj(
                "Kingmaker.UI.UnitSettings.Blueprints.ActionPanelLogic",
                &[("name", Node::Str("$P$".into())), ("AutoCastConditions", checker)],
            )))
        };
        let none = feature(&w, 60, vec![panel(vec![])]);
        let null = feature(&w, 61, vec![panel(vec![Node::Null])]);
        assert!(!super::has_auto_cast_conditions(&none));
        assert!(super::has_auto_cast_conditions(&null));
    }

    fn parametrized(w: &crate::world::World, g: u128) -> Rc<Blueprint> {
        w.content.insert(Blueprint {
            guid: wr_content::Guid(g),
            name: format!("P{g}"),
            obj: obj(
                "Kingmaker.Blueprints.Classes.Selection.BlueprintParametrizedFeature",
                &[("ParameterType", Node::Enum(2))],
            ),
            source: crate::content::Source::Bbp,
        });
        w.content.get(wr_content::Guid(g)).expect("inserted")
    }

    /// `AddFact<Feature>(bp, null, param)` makes a scope only for a param that isn't equal to
    /// null; an empty one leaves the outer scope's param in force.
    #[test]
    fn an_empty_param_keeps_the_outer_scope() {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        let bp = parametrized(&w, 80);
        let outer = serde_json::json!({"WeaponCategory": "Longsword"});
        w.feature_param_ctx = Some(outer.clone());
        let f = super::add_fact_with_param(&mut w, u, &bp, &FeatureParam::default()).unwrap();
        assert_eq!(w.fact(f).param, Some(outer.clone()));
        assert_eq!(w.feature_param_ctx, Some(outer));
        // A non-empty param takes over inside its own scope and the outer one comes back.
        let bp2 = parametrized(&w, 81);
        let f2 = super::add_fact_with_param(&mut w, u, &bp2, &FeatureParam::of_weapon_category(1))
            .unwrap();
        assert_eq!(w.fact(f2).param, Some(serde_json::json!({"WeaponCategory": "Dagger"})));
        assert_eq!(w.feature_param_ctx, Some(serde_json::json!({"WeaponCategory": "Longsword"})));
    }

    /// `Feature.OnActivate`'s `Param == null` holds for an absent or empty param.
    #[test]
    fn activating_a_parametrized_feature_without_a_param_logs() {
        let msg = |w: &crate::world::World| {
            w.log.iter().filter(|l| l.message.starts_with("Param is not specified")).count()
        };
        let (mut w, u) = crate::stats::tests::world_with_unit();
        let bp = parametrized(&w, 82);
        super::add_fact(&mut w, u, &bp).unwrap();
        assert_eq!(msg(&w), 1);
        let bp2 = parametrized(&w, 83);
        super::add_fact_with_param(&mut w, u, &bp2, &FeatureParam::default()).unwrap();
        assert_eq!(msg(&w), 2);
        let bp3 = parametrized(&w, 84);
        super::add_fact_with_param(&mut w, u, &bp3, &FeatureParam::of_weapon_category(1)).unwrap();
        assert_eq!(msg(&w), 2);
    }
}
