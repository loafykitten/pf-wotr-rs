//! Mechanics components on monsters' features and buffs (decompiled, not copied):
//! `BuffDescriptorImmunity`, `SpecificBuffImmunity`, `SpellImmunityToSpellDescriptor`
//! (`Kingmaker.UnitLogic.FactLogic/`), `SavesFixedRecalculateThievery`,
//! `ReplaceSourceBone`, `ReplaceCastSource` (`Kingmaker.Designers.Mechanics.Facts/`),
//! `AddFactContextActions` (`Kingmaker.UnitLogic.Mechanics.Components/`) and the context
//! actions their lists use (`Kingmaker.UnitLogic.Mechanics.Actions/ContextActionRemoveBuff.cs`).

use super::{Port, bits, owner_unit, with_settings};
use crate::combat::rules::RuleCanApplyBuff;
use crate::content::View;
use crate::registry::{ComponentLogic, Hook, Registry};
use crate::rulebook::RuleEvent;
use crate::world::{CompRef, EntityIdx, FactKind, World};

/// A component port with rulebook handlers.
pub(crate) struct RulePort {
    pub name: &'static str,
    pub bits: u32,
    pub hooks: fn(&mut World, CompRef, Hook),
    pub handlers: &'static [&'static str],
    pub rule: fn(&mut World, CompRef, &mut dyn RuleEvent, bool),
}

impl ComponentLogic for RulePort {
    fn type_name(&self) -> &'static str {
        self.name
    }
    fn implemented(&self) -> u32 {
        self.bits
    }
    fn handlers(&self) -> &'static [&'static str] {
        self.handlers
    }
    fn hook(&self, w: &mut World, c: CompRef, h: Hook) {
        (self.hooks)(w, c, h);
    }
    fn rule(&self, w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
        (self.rule)(w, c, rule, about);
    }
}

fn no_hooks(_: &mut World, _: CompRef, _: Hook) {}

pub fn register(r: &mut Registry) {
    r.register(Box::new(RulePort {
        name: "BuffDescriptorImmunity",
        bits: 0,
        hooks: no_hooks,
        handlers: &["IInitiatorRulebookHandler<RuleCanApplyBuff>"],
        rule: buff_descriptor_immunity,
    }));
    r.register(Box::new(RulePort {
        name: "SpecificBuffImmunity",
        bits: 0,
        hooks: no_hooks,
        handlers: &["IInitiatorRulebookHandler<RuleCanApplyBuff>"],
        rule: specific_buff_immunity,
    }));
    r.register(Box::new(Port::new(
        "SpellImmunityToSpellDescriptor",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        spell_immunity_to_spell_descriptor,
    )));
    r.register(Box::new(Port::new(
        "SavesFixedRecalculateThievery",
        bits(&[Hook::Activate]),
        saves_fixed_thievery,
    )));
    r.register(Box::new(Port::new(
        "ReplaceSourceBone",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        visual_changes,
    )));
    r.register(Box::new(Port::new(
        "ReplaceCastSource",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        visual_changes,
    )));
    r.register(Box::new(Port::new(
        "AddFactContextActions",
        bits(&[Hook::Activate, Hook::Deactivate, Hook::Dispose]),
        add_fact_context_actions,
    )));
}

fn has_fact(w: &World, u: EntityIdx, g: Option<wr_content::Guid>) -> bool {
    g.is_some_and(|g| w.entity(u).facts.list.iter().any(|f| w.fact(*f).blueprint == Some(g)))
}

/// `BuffDescriptorImmunity.OnEventAboutToTrigger(RuleCanApplyBuff)`: immune when the
/// context's spell descriptor shares a flag with `Descriptor`, unless the caster has the
/// ignore feature, and (with `CheckFact`) only when the caster has `FactToCheck`.
fn buff_descriptor_immunity(w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
    if !about {
        return;
    }
    let Some(r) = rule.as_any().downcast_mut::<RuleCanApplyBuff>() else { return };
    let Some((mask, ignore, check, to_check)) = with_settings(w, c, |v| {
        (
            v.obj("Descriptor").map_or(0, |d| d.int("m_IntValue")),
            v.guid("m_IgnoreFeature"),
            v.bool("CheckFact"),
            v.guid("m_FactToCheck"),
        )
    }) else {
        return;
    };
    if let Some(caster) = r.caster
        && w.unit(caster).is_some_and(|u| {
            u.mechanics_features.get(&MYTHIC_REDUCE_RESISTANCES).copied().unwrap_or(0) > 0
        })
    {
        return;
    }
    let flag = mask & r.descriptor != 0;
    let flag3 =
        ignore.is_none() || r.caster.is_none() || !r.caster.is_some_and(|x| has_fact(w, x, ignore));
    let flag4 = !check || r.caster.is_some_and(|x| has_fact(w, x, to_check));
    if flag && flag3 && flag4 {
        r.immunity = true;
    }
}

/// `MechanicsFeatureType.MythicReduceResistances`.
const MYTHIC_REDUCE_RESISTANCES: i64 = 74;

/// `SpecificBuffImmunity.OnEventAboutToTrigger(RuleCanApplyBuff)`: for its own buff only.
fn specific_buff_immunity(w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
    if !about {
        return;
    }
    let Some(r) = rule.as_any().downcast_mut::<RuleCanApplyBuff>() else { return };
    let Some(buff) = with_settings(w, c, |v| v.guid("m_Buff")) else { return };
    if Some(r.blueprint.guid) == buff {
        // The reason's caster's alignment and facts decide: `evt.Reason.Caster` isn't
        // modelled (the sim's `AssignSource` keeps no context).
        w.coverage.unsupported("SpecificBuffImmunity: matching buff (reason caster)");
    }
}

/// `SpellImmunityToSpellDescriptor.OnTurnOn`/`OnTurnOff`: an immunity entry in
/// `UnitPartSpellResistance` (`AddImmunity` returns its id, kept in `Data.AppliedId`;
/// `Remove(id)` drops the entry and the part once it is empty).
fn spell_immunity_to_spell_descriptor(w: &mut World, c: CompRef, h: Hook) {
    use crate::unit::{SpellImmunity, SpellResistancePart};
    let Some(u) = owner_unit(w, c) else { return };
    match h {
        Hook::TurnOn => {
            let Some((descriptor, ignore)) = with_settings(w, c, |v| {
                (
                    v.obj("Descriptor").map_or(0, |d| d.int("m_IntValue")),
                    v.guid("m_CasterIgnoreImmunityFact"),
                )
            }) else {
                return;
            };
            if !w.entity(u).parts.iter().any(|p| p.clr == "UnitPartSpellResistance") {
                w.entity_mut(u).parts.push(crate::world::Part { clr: "UnitPartSpellResistance" });
            }
            // `AddImmunity`: `new SpellImmunity(m_NextId++)` with type SpellDescriptor,
            // alignment None, no exceptions.
            let part = w
                .unit_mut(u)
                .expect("unit")
                .spell_resistance
                .get_or_insert_with(SpellResistancePart::default);
            let id = part.next_id;
            part.next_id = id.wrapping_add(1);
            part.immunities.push(SpellImmunity {
                id,
                kind: 3,
                alignment: 0,
                spell_descriptor: descriptor,
                caster_ignore_fact: ignore,
            });
            let mut d = w.component(c).data.clone().unwrap_or_else(|| serde_json::json!({}));
            d["AppliedId"] = serde_json::json!(id);
            w.component_mut(c).data = Some(d);
        }
        Hook::TurnOff => {
            let applied = w
                .component(c)
                .data
                .as_ref()
                .and_then(|d| d["AppliedId"].as_i64())
                .and_then(|i| i32::try_from(i).ok());
            if let Some(id) = applied {
                // `Owner.Get<UnitPartSpellResistance>()?.Remove(id)`, then
                // `RemovePartIfEmpty` (no resistance values or ignored descriptors are
                // ever added by the sim).
                let unit = w.unit_mut(u).expect("unit");
                if let Some(part) = &mut unit.spell_resistance {
                    part.immunities.retain(|i| i.id != id);
                    if part.immunities.is_empty() {
                        unit.spell_resistance = None;
                        w.entity_mut(u).parts.retain(|p| p.clr != "UnitPartSpellResistance");
                    }
                }
                if let Some(d) = &mut w.component_mut(c).data {
                    d["AppliedId"] = serde_json::Value::Null;
                }
            }
        }
        _ => {}
    }
}

/// `SavesFixedRecalculateThievery.OnActivate`: removes the encumbrance modifiers of the
/// Thievery skill.
fn saves_fixed_thievery(w: &mut World, c: CompRef, h: Hook) {
    if h != Hook::Activate {
        return;
    }
    let Some(u) = owner_unit(w, c) else { return };
    let mods: Vec<_> = w
        .stats(u)
        .v(crate::stats::types::StatType::SkillThievery)
        .modifiers()
        .filter(|m| w.mods[m.i()].desc == crate::stats::types::ModifierDescriptor::Encumbrance)
        .collect();
    for m in mods {
        crate::stats::ops::remove(w, m);
    }
}

/// `ReplaceSourceBone`/`ReplaceCastSource` `OnTurnOn`/`OnTurnOff`: entries in
/// `UnitPartVisualChanges` (FX attachment bones and cast source; presentation data that is
/// saved with the unit, no rules effect).
fn visual_changes(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = owner_unit(w, c) else { return };
    if h == Hook::TurnOn && !w.entity(u).parts.iter().any(|p| p.clr == "UnitPartVisualChanges") {
        w.entity_mut(u).parts.push(crate::world::Part { clr: "UnitPartVisualChanges" });
    }
}

/// `AddFactContextActions`: `Activated` on activation, `Deactivated` on deactivation,
/// `Dispose` on disposal, run in the fact's context (`IFactContextOwner.RunActionInContext`:
/// the context's main target is its owner). `NewRound` runs on round ticks.
pub(super) fn add_fact_context_actions(w: &mut World, c: CompRef, h: Hook) {
    let list = match h {
        Hook::Activate => "Activated",
        Hook::Deactivate => "Deactivated",
        Hook::Dispose => "Dispose",
        _ => return,
    };
    run_fact_context_actions(w, c, list);
}

/// `AddFactContextActions`' `ITickEachRound.OnNewRound`: the `NewRound` list.
pub fn add_fact_context_actions_new_round(w: &mut World, c: CompRef) {
    run_fact_context_actions(w, c, "NewRound");
}

/// Whether an `AddFactContextActions` component's action list (`Activated`, `NewRound`, …) holds
/// any action.
#[must_use]
pub fn fact_context_actions_any(w: &World, c: CompRef, list: &str) -> bool {
    let Some((bp, i)) = crate::components::settings(w, c) else { return false };
    let objs = bp.components_array();
    View(objs[i]).obj(list).is_some_and(|l| !l.objs("Actions").is_empty())
}

fn run_fact_context_actions(w: &mut World, c: CompRef, list: &str) {
    let Some((bp, i)) = crate::components::settings(w, c) else { return };
    let objs = bp.components_array();
    let actions: Vec<_> = View(objs[i]).obj(list).map(|l| l.objs("Actions")).unwrap_or_default();
    if actions.is_empty() {
        return;
    }
    let owner = w.fact(c.fact).owner;
    let caster = w
        .fact(c.fact)
        .context
        .as_ref()
        .and_then(|x| x.caster.clone())
        .and_then(|id| w.by_id.get(&id).copied());
    // `(Fact as IFactContextOwner)?.RunActionInContext(list)`: the context's main target.
    in_fact_context(w, c.fact, owner, |w| {
        for a in actions {
            let v = View(a);
            // `Conditional` is a game action: its conditions and branches run in the fact's
            // scope (`ActionList.Run` logs an exception and goes on).
            if v.type_name() == "Conditional" {
                w.coverage.executed("action Conditional");
                if let Err(e) = crate::elements::actions::run(w, &bp, v) {
                    w.log("error", format!("action Conditional: {e:?}"));
                }
                continue;
            }
            run_context_action(w, v, owner, caster);
        }
    });
}

/// The scopes `IFactContextOwner.RunActionInContext` opens around its actions: `Buff.Data` for a
/// buff, then the fact's `MechanicsContext.Data` on `target`. Only `Feature`, `MainUnitFact`,
/// `Buff` and `ItemEnchantment` implement it (callers use `(Fact as IFactContextOwner)?`, so any
/// other fact runs nothing). `Feature`'s own `Feature.Data` scope isn't opened: no ported element
/// reads it.
fn in_fact_context(
    w: &mut World,
    fact: crate::world::FactIdx,
    target: Option<EntityIdx>,
    f: impl FnOnce(&mut World),
) {
    let kind = w.fact(fact).kind;
    if !matches!(
        kind,
        FactKind::Feature | FactKind::Buff | FactKind::MainUnitFact | FactKind::ItemEnchantment
    ) {
        return;
    }
    if w.fact(fact).context.is_none() {
        w.coverage.unsupported("IFactContextOwner.RunActionInContext: the fact has no context");
        return;
    }
    let mechanics = crate::elements::Ctx::Mechanics { fact, target };
    if kind == FactKind::Buff {
        crate::elements::with_ctx(w, crate::elements::Ctx::BuffData(fact), |w| {
            crate::elements::with_ctx(w, mechanics, f);
        });
    } else {
        crate::elements::with_ctx(w, mechanics, f);
    }
}

/// `IFactContextOwner.RunActionInContext(actions, target)` on a component's fact, in its scopes
/// (`in_fact_context`). `ActionList.Run` semantics, as [`crate::elements::run_actions`].
pub fn run_action_in_context(
    w: &mut World,
    fact: crate::world::FactIdx,
    owner: &std::rc::Rc<crate::content::Blueprint>,
    list: Option<View<'_>>,
    target: Option<EntityIdx>,
) {
    if list.is_none_or(|l| l.objs("Actions").is_empty()) {
        return;
    }
    in_fact_context(w, fact, target, |w| crate::elements::run_actions(w, owner, list));
}

/// `ContextAction.RunAction()` for the context actions on these paths.
pub fn run_context_action(
    w: &mut World,
    a: View<'_>,
    target: Option<EntityIdx>,
    caster: Option<EntityIdx>,
) {
    match a.type_name() {
        // `ContextActionRemoveBuff.RunAction`: the first buff of the blueprint on the target
        // (or the caster), optionally only one the caster applied.
        "ContextActionRemoveBuff" => {
            if a.bool("RemoveRank") {
                w.coverage.unsupported("ContextActionRemoveBuff: RemoveRank");
            }
            let who = if a.bool("ToCaster") { caster } else { target };
            let Some(u) = who else { return };
            let buff = a.guid("m_Buff");
            let only_from_caster = a.bool("OnlyFromCaster");
            let caster_id = caster.map(|c| w.entity(c).unique_id.clone());
            let found = w.entity(u).facts.list.iter().copied().find(|f| {
                let fact = w.fact(*f);
                fact.kind == FactKind::Buff
                    && fact.blueprint == buff
                    && (!only_from_caster
                        || fact.context.as_ref().and_then(|x| x.caster.clone()) == caster_id)
            });
            if let Some(f) = found {
                crate::unit_facts::remove(w, u, f);
            }
        }
        // `ContextActionReduceDebilitatingBuffsDuration.RunAction`: in a mechanics context, unless
        // the difficulty's `StatsAdjustments` is above the action's, every buff of the target
        // with a debilitating descriptor lasting under 5 minutes ends a round from now.
        "ContextActionReduceDebilitatingBuffsDuration" => {
            if !w.ctx.iter().any(|c| matches!(c, crate::elements::Ctx::Mechanics { .. })) {
                w.log("error", "Unable to apply buff: no context found");
                return;
            }
            let Some(u) = target else { return };
            let Some(setting) = w.settings.difficulty.settings.stats_adjustments else {
                w.coverage.unsupported("SettingsRoot.Difficulty.StatsAdjustments missing");
                return;
            };
            if i64::from(setting) > a.int("StatsAdjustmentsType") {
                return;
            }
            // `SpellDescriptorOperations.IsDebilitatingEffect`: Shaken, Nauseated, Stun,
            // Paralysis, Confusion, Sleep, Petrified, UndeadControl.
            const DEBILITATING: i64 = 0x40000
                | 0x20_0000
                | 0x100_0000
                | 0x200_0000
                | 0x400_0000
                | 0x1_0000_0000
                | 0x2000_0000_0000
                | 0x10_0000_0000_0000;
            const FIVE_MINUTES: i64 = 5 * 60 * 10_000_000;
            const ROUND: i64 = 6 * 10_000_000;
            let now = w.clock.game_time.0;
            for f in w.entity(u).facts.list.clone() {
                let fact = w.fact(f);
                let Some(times) = &fact.buff else { continue };
                let debilitating =
                    fact.context.as_ref().is_some_and(|c| c.spell_descriptor & DEBILITATING != 0);
                let end = times.end_time.unwrap_or(i64::MAX);
                if fact.kind == FactKind::Buff
                    && debilitating
                    && end.saturating_sub(fact.attach_time.0) < FIVE_MINUTES
                {
                    // `SetDuration(1.Rounds().Seconds)`: `SetEndTime(GameTime + 6 s)`; the next
                    // event follows the end time when the buffs tick.
                    if let Some(t) = w.fact_mut(f).buff.as_mut() {
                        t.end_time = Some(now + ROUND);
                    }
                }
            }
        }
        "ContextActionApplyBuff" => apply_buff(w, a, target, caster),
        t => w.coverage.unsupported(format!("context action {t}")),
    }
}

/// `ContextActionApplyBuff.RunAction` (`Kingmaker.UnitLogic.Mechanics.Actions/
/// ContextActionApplyBuff.cs`): in the innermost mechanics context, the buff goes on the
/// context's target (or its caster) through `UnitDescriptor.AddBuff(blueprint, parentContext,
/// duration)`: the context cloned for the buff (`IgnoreParentContext`: none, so a context of
/// its own with the target as its caster). `IsFromSpell` is the action's or the blueprint's;
/// `AsChild` stores the buff in the enclosing `Buff.Data` scope's buff when both have one owner.
/// Durations other than permanent or `DurationSeconds` (`SameDuration`, `DurationValue`'s
/// dice and context values), `IsNotDispelable` (not modelled on buffs) are reported.
fn apply_buff(w: &mut World, a: View<'_>, target: Option<EntityIdx>, caster: Option<EntityIdx>) {
    let Some((fact, _)) = crate::elements::actions::mechanics_scope(w) else {
        w.log("error", "Unable to apply buff: no context found");
        return;
    };
    let Some(ctx) = w.fact(fact).context.clone() else {
        w.log("error", "Unable to apply buff: no context found");
        return;
    };
    let Some(u) = (if a.bool("ToCaster") { caster } else { target }) else {
        w.log("error", "Can't apply buff: target is null");
        return;
    };
    // `CalculateDuration`.
    let duration = if a.bool("Permanent") {
        None
    } else if a.bool("SameDuration") {
        w.coverage.unsupported("ContextActionApplyBuff: SameDuration");
        return;
    } else if a.bool("UseDurationSeconds") {
        match crate::csharp::TimeSpan::from_seconds(f64::from(a.f32("DurationSeconds"))) {
            Ok(d) => Some(d),
            Err(_) => {
                w.coverage.unsupported("ContextActionApplyBuff: DurationSeconds out of range");
                return;
            }
        }
    } else {
        w.coverage.unsupported("ContextActionApplyBuff: DurationValue (ContextDurationValue)");
        return;
    };
    if a.bool("IsNotDispelable") {
        w.coverage.unsupported("ContextActionApplyBuff: IsNotDispelable");
        return;
    }
    // `buffTarget.Descriptor.AddBuff(Buff, parentContext, duration)`: a missing blueprint
    // answers null.
    let Some(bp) = a.guid("m_Buff").and_then(|g| w.content.get(g)) else { return };
    let buff_ctx = if a.bool("IgnoreParentContext") {
        // `AddBuff(blueprint, Unit, duration)`: the target its own caster.
        crate::unit_facts::context(w, Some(u), Some(u), &bp)
    } else {
        // `Buffs.AddBuff(blueprint, parentContext, duration)`: `parentContext.CloneFor`.
        w.pending_buffs.parent_descriptor = ctx.spell_descriptor;
        crate::unit_facts::clone_for(w, &ctx, &bp, u)
    };
    let Some(buff) = crate::unit_facts::add_buff(w, u, &bp, buff_ctx, duration) else { return };
    // `buff.IsFromSpell = IsFromSpell || Buff.IsFromSpell`.
    let from_spell = a.bool("IsFromSpell")
        || crate::unit_facts::buff_flag(&bp, crate::unit_facts::BUFF_IS_FROM_SPELL);
    if let Some(b) = w.fact_mut(buff).buff.as_mut() {
        b.is_from_spell = from_spell;
    }
    if !a.bool("AsChild") {
        return;
    }
    // `ContextData<Buff.Data>.Current?.Buff`: the parent stores the child (`StoreFact`, which
    // logs and skips a fact its owner doesn't have).
    let parent = w.ctx.iter().rev().find_map(|c| match c {
        crate::elements::Ctx::BuffData(f) => Some(*f),
        _ => None,
    });
    let Some(p) = parent else { return };
    let owner = w.fact(p).owner;
    if owner != w.fact(buff).owner {
        w.log("error", "Parent and child buff must have one owner");
        return;
    }
    let has = owner.is_some_and(|o| w.entity(o).facts.list.contains(&buff));
    if !has {
        w.log("error", "Can't store fact from different owner");
        return;
    }
    if let Some(b) = w.fact_mut(p).buff.as_mut() {
        b.stored_facts.push(buff);
    }
}
