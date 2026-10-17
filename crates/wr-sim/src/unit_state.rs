//! `UnitState`'s condition counters, immunities and status effects
//! (`Kingmaker.UnitLogic/UnitState.cs:20–150, 393–560`), the status effects
//! (`Kingmaker.UnitLogic.StatusEffect/*.cs`: `StatusEffectFactory`, `UnitStatusEffectFatigued`,
//! `UnitStatusEffectDazzled`, `UnitStatusEffectHelpless`, `UnitStatusEffectProne`,
//! `UnitStatusBuff`) and the `AddCondition`/`AddConditionImmunity` components
//! (`Kingmaker.UnitLogic.FactLogic/AddCondition.cs`, `AddConditionImmunity.cs`).

use crate::csharp::CsDict;
use crate::registry::{Hook, Registry};
use crate::stats::ops;
use crate::stats::types::{ModifierDescriptor as D, StatType};
use crate::world::{CompRef, EntityIdx, ModIdx, World};

/// `EnumUtils.GetMaxValue<UnitCondition>() + 1`: the counter arrays' length.
pub const CONDITION_COUNT: usize = 58;

/// `UnitCondition` values the sim names.
pub mod cond {
    pub const NAUSEATED: i32 = 2;
    pub const FATIGUED: i32 = 3;
    pub const DEATH_DOOR: i32 = 5;
    pub const STAGGERED: i32 = 6;
    pub const PETRIFIED: i32 = 8;
    pub const ENTANGLED: i32 = 11;
    pub const DIFFICULT_TERRAIN: i32 = 12;
    pub const FRIGHTENED: i32 = 13;
    pub const PRONE: i32 = 14;
    pub const SICKENED: i32 = 15;
    pub const SHAKEN: i32 = 20;
    pub const DAZZLED: i32 = 22;
    pub const STUNNED: i32 = 23;
    pub const HELPLESS: i32 = 24;
    pub const UNCONSCIOUS: i32 = 31;
    pub const UNLOOTABLE: i32 = 33;
    pub const COWERING: i32 = 41;
    pub const DISABLE_MOUNT_RIDING: i32 = 48;
    pub const SPELLCASTING_FORBIDDEN: i32 = 51;
    pub const CANT_USE_STANDARD_ACTIONS: i32 = 55;
}

/// A `UnitStatusEffect` and the state it saves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusEffect {
    /// `UnitStatusEffectFatigued`: `m_ModifierStrength`, `m_ModifierDexterity`.
    Fatigued { strength: Option<ModIdx>, dexterity: Option<ModIdx> },
    /// `UnitStatusEffectDazzled`: `m_ModifierAttack`, `m_ModifierPerception`.
    Dazzled { attack: Option<ModIdx>, perception: Option<ModIdx> },
    /// `UnitStatusEffectProne`: wakes the unit (`Unit.Wake(1f)`, AI sleep state).
    Prone,
    /// `UnitStatusEffectHelpless`.
    Helpless,
    /// `UnitStatusBuff`: a buff from `BlueprintRoot.SystemMechanics`.
    Buff { condition: i32 },
    /// An effect read from the save, whose own state (buff, modifiers) isn't hydrated.
    Saved,
}

/// `UnitState` (the members the sim models).
#[derive(Debug, Clone)]
pub struct UnitStateData {
    /// `m_Conditions`.
    pub conditions: [i8; CONDITION_COUNT],
    /// `m_ConditionsImmunity`.
    pub immunities: [i8; CONDITION_COUNT],
    /// `m_StatusEffects` (a `Dictionary<UnitCondition, UnitStatusEffect>`, Mono order).
    pub status_effects: CsDict<i32, StatusEffect>,
    /// `State.IsPanicked` may be true: saved true, or the unit was (or was about to be) frightened
    /// since the load. `UnitFearController` (not run by the sim) sets it only while `Frightened`
    /// holds and clears it on its own tick, after the memory controller's, so a condition that
    /// ended can leave it true for a frame; once possible it stays so.
    pub panic_possible: bool,
}

impl Default for UnitStateData {
    fn default() -> Self {
        Self {
            conditions: [0; CONDITION_COUNT],
            immunities: [0; CONDITION_COUNT],
            status_effects: CsDict::new(),
            panic_possible: false,
        }
    }
}

fn idx(c: i32) -> usize {
    usize::try_from(c).unwrap_or(0).min(CONDITION_COUNT - 1)
}

fn st(w: &World, u: EntityIdx) -> &UnitStateData {
    &w.unit(u).expect("unit").state
}

fn st_mut(w: &mut World, u: EntityIdx) -> &mut UnitStateData {
    &mut w.unit_mut(u).expect("unit").state
}

/// `HasCondition`: counted and not immune.
#[must_use]
pub fn has_condition(w: &World, u: EntityIdx, c: i32) -> bool {
    let s = st(w, u);
    s.conditions[idx(c)] > 0 && s.immunities[idx(c)] <= 0
}

/// `UnitState.PostLoad`: `FreezedConditionsForRemove` (Fatigued, DifficultTerrain) are
/// removed entirely; the facts that keep them re-add them when turned on.
pub fn post_load(w: &mut World, u: EntityIdx) {
    for c in [cond::FATIGUED, cond::DIFFICULT_TERRAIN] {
        remove_condition(w, u, c, true);
    }
}

/// `AddCondition(condition, source, exceptions)`. Condition exceptions only matter to
/// `HasConditionAndItWorksOnTarget` (not ported); the component reports non-null ones.
pub fn add_condition(
    w: &mut World,
    u: EntityIdx,
    c: i32,
    source_buff: Option<crate::world::FactIdx>,
) {
    let had = has_condition(w, u, c);
    if c == cond::FRIGHTENED {
        st_mut(w, u).panic_possible = true;
    }
    st_mut(w, u).conditions[idx(c)] = st(w, u).conditions[idx(c)].wrapping_add(1);
    update_status_effect(w, u, c, source_buff);
    let success = has_condition(w, u, c);
    raise(w, u, "IUnitConditionChangeAttemptHandler", "HandleUnitConditionAddAttempt");
    if !had && success {
        raise(w, u, "IUnitConditionsChanged", "HandleUnitConditionsChanged");
    }
}

/// `RemoveCondition` / `RemoveConditionAll`: nothing while the owner is being disposed.
pub fn remove_condition(w: &mut World, u: EntityIdx, c: i32, all: bool) {
    if w.entity(u).disposing {
        return;
    }
    let had = has_condition(w, u, c);
    if all {
        st_mut(w, u).conditions[idx(c)] = 0;
    } else if st(w, u).conditions[idx(c)] > 0 {
        st_mut(w, u).conditions[idx(c)] -= 1;
    }
    update_status_effect(w, u, c, None);
    if had && !has_condition(w, u, c) {
        raise(w, u, "IUnitConditionsChanged", "HandleUnitConditionsChanged");
    }
}

/// `AddConditionImmunity` (raises whenever the unit had the condition) /
/// `RemoveConditionImmunity` (nothing while disposing; raises when the condition appears).
pub fn change_immunity(w: &mut World, u: EntityIdx, c: i32, add: bool) {
    if !add && w.entity(u).disposing {
        return;
    }
    let had = has_condition(w, u, c);
    let i = &mut st_mut(w, u).immunities[idx(c)];
    *i = if add { i.wrapping_add(1) } else { i.wrapping_sub(1) };
    update_status_effect(w, u, c, None);
    let raised = if add { had } else { !had && has_condition(w, u, c) };
    if raised {
        raise(w, u, "IUnitConditionsChanged", "HandleUnitConditionsChanged");
    }
}

fn raise(w: &mut World, u: EntityIdx, iface: &'static str, method: &'static str) {
    crate::rulebook::bus::raise_global_call(w, iface, method, Some(u));
}

/// `UpdateStatusEffect(condition, sourceBuff)`.
fn update_status_effect(
    w: &mut World,
    u: EntityIdx,
    c: i32,
    source_buff: Option<crate::world::FactIdx>,
) {
    let counted = st(w, u).conditions[idx(c)] > 0;
    let immune = st(w, u).immunities[idx(c)] > 0;
    let existing = st(w, u).status_effects.get(&c).cloned();
    if let Some(e) = existing
        && (!counted || immune)
    {
        deactivate(w, u, &e);
        st_mut(w, u).status_effects.remove(&c);
        return;
    }
    if st(w, u).status_effects.get(&c).is_some() || !counted || immune {
        return;
    }
    let Some(mut e) = create(c) else { return };
    if let StatusEffect::Buff { .. } = e {
        // `sourceBuff == null || buffBlueprint != sourceBuff.Blueprint`: a condition coming
        // from the status buff itself doesn't create it again.
        if source_buff.is_some() {
            w.coverage.unsupported("UnitStatusBuff: source-buff comparison");
        }
    }
    activate(w, u, &mut e);
    st_mut(w, u).status_effects.insert(c, e);
}

/// `StatusEffectFactory.CreateStatusEffect`.
fn create(c: i32) -> Option<StatusEffect> {
    match c {
        cond::FATIGUED => Some(StatusEffect::Fatigued { strength: None, dexterity: None }),
        cond::DAZZLED => Some(StatusEffect::Dazzled { attack: None, perception: None }),
        cond::PRONE => Some(StatusEffect::Prone),
        cond::HELPLESS => Some(StatusEffect::Helpless),
        cond::DEATH_DOOR
        | cond::PETRIFIED
        | cond::ENTANGLED
        | cond::SHAKEN
        | cond::FRIGHTENED
        | cond::SICKENED
        | cond::STUNNED
        | cond::COWERING => Some(StatusEffect::Buff { condition: c }),
        _ => None,
    }
}

fn activate(w: &mut World, u: EntityIdx, e: &mut StatusEffect) {
    match e {
        StatusEffect::Fatigued { strength, dexterity } => {
            *strength = Some(ops::add_modifier_value(w, u, StatType::Strength, -2, D::Fatigued));
            *dexterity = Some(ops::add_modifier_value(w, u, StatType::Dexterity, -2, D::Fatigued));
        }
        StatusEffect::Dazzled { attack, perception } => {
            *attack = Some(ops::add_modifier_value(
                w,
                u,
                StatType::AdditionalAttackBonus,
                -1,
                D::UntypedStackable,
            ));
            *perception = Some(ops::add_modifier_value(
                w,
                u,
                StatType::SkillPerception,
                -1,
                D::UntypedStackable,
            ));
        }
        // `Unit.Wake(1f)` (`UnitEntityData.cs:1871`): `AwakeTimer = max(1, AwakeTimer)`,
        // which `SleepingUnitsController` counts down and the awake-unit set follows. Neither
        // is modelled, so the effect is reported.
        StatusEffect::Prone => {
            w.coverage.unsupported("UnitStatusEffectProne.OnActivate: Unit.Wake");
        }
        // `Dexterity.OnChanged += UnitStatHandler; UnitStatHandler(Dexterity, Dexterity)`.
        StatusEffect::Helpless => {
            ops::subscribe_changed_handler(
                w,
                u,
                StatType::Dexterity,
                crate::stats::value::ChangedHandler::Helpless,
            );
            helpless_changed(w, u);
        }
        StatusEffect::Buff { condition } => {
            w.coverage.unsupported(format!("UnitStatusBuff.OnActivate (condition {condition})"));
        }
        StatusEffect::Saved => unreachable!("saved effects are never created"),
    }
}

fn deactivate(w: &mut World, u: EntityIdx, e: &StatusEffect) {
    match e {
        StatusEffect::Fatigued { strength, dexterity } => {
            for m in [strength, dexterity].into_iter().flatten() {
                ops::remove(w, *m);
            }
        }
        StatusEffect::Dazzled { attack, perception } => {
            for m in [attack, perception].into_iter().flatten() {
                ops::remove(w, *m);
            }
        }
        StatusEffect::Prone => {
            w.coverage.unsupported("UnitStatusEffectProne.OnDeactivate: Unit.Wake");
        }
        // `Dexterity.OnChanged -= UnitStatHandler; ClearPenalty(unit)`.
        StatusEffect::Helpless => {
            ops::unsubscribe_changed_handler(
                w,
                u,
                StatType::Dexterity,
                crate::stats::value::ChangedHandler::Helpless,
            );
            helpless_clear(w, u);
        }
        StatusEffect::Buff { condition } => {
            w.coverage.unsupported(format!("UnitStatusBuff.OnDeactivate (condition {condition})"));
        }
        StatusEffect::Saved => {
            w.coverage.unsupported("UnitState.UpdateStatusEffect: deactivating a saved effect");
        }
    }
}

/// `UnitStatusEffectHelpless.ClearPenalty`.
fn helpless_clear(w: &mut World, u: EntityIdx) {
    for t in [
        StatType::AC,
        StatType::SaveReflex,
        StatType::AdditionalCMD,
        StatType::SkillThievery,
        StatType::SkillMobility,
        StatType::SkillStealth,
    ] {
        ops::remove_modifiers(w, u, t, D::Helpless);
    }
}

/// `UnitStatusEffectHelpless.UnitStatHandler` (`ClearPenalty` then `AddPenalty`):
/// `-5 - Dexterity.Bonus` on the reflex save and the skills, less the AC's dodge bonus on the
/// AC and the CMD.
pub fn helpless_changed(w: &mut World, u: EntityIdx) {
    helpless_clear(w, u);
    let num = crate::csharp::arith::sub(-5, ops::attribute_bonus(w, u, StatType::Dexterity));
    let dodge =
        ops::apply_modifiers_filtered(w, u, StatType::AC, 0, ops::Filter::Descriptor(D::Dodge));
    let with_dodge = crate::csharp::arith::sub(num, dodge);
    ops::add_modifier_value(w, u, StatType::AC, with_dodge, D::Helpless);
    ops::add_modifier_value(w, u, StatType::SaveReflex, num, D::Helpless);
    ops::add_modifier_value(w, u, StatType::AdditionalCMD, with_dodge, D::Helpless);
    ops::add_modifier_value(w, u, StatType::SkillThievery, num, D::Helpless);
    ops::add_modifier_value(w, u, StatType::SkillMobility, num, D::Helpless);
    ops::add_modifier_value(w, u, StatType::SkillStealth, num, D::Helpless);
}

/// The save's `Descriptor.State` counters: `m_Conditions`, `m_ConditionsImmunity` (missing
/// = all zero, `DefaultValueHandling.Ignore`). Saved status effects are read with the rest
/// of the unit's narrative state (`hydrate::narrative`).
pub fn hydrate(w: &mut World, u: EntityIdx, conditions: &[i64], immunities: &[i64]) {
    let s = st_mut(w, u);
    for (i, v) in conditions.iter().enumerate().take(CONDITION_COUNT) {
        s.conditions[i] = i8::try_from(*v).unwrap_or(0);
    }
    if s.conditions[idx(cond::FRIGHTENED)] > 0 {
        s.panic_possible = true;
    }
    for (i, v) in immunities.iter().enumerate().take(CONDITION_COUNT) {
        s.immunities[i] = i8::try_from(*v).unwrap_or(0);
    }
}

/// The dump's `conditions` (`[[UnitCondition, name, count], …]` for non-zero counts).
#[must_use]
pub fn dump_counts(w: &World, u: EntityIdx, immunities: bool) -> serde_json::Value {
    let s = st(w, u);
    let arr = if immunities { &s.immunities } else { &s.conditions };
    serde_json::Value::Array(
        arr.iter()
            .enumerate()
            .filter(|(_, v)| **v != 0)
            .map(|(i, v)| serde_json::json!([i, condition_name(i32::try_from(i).unwrap_or(0)), v]))
            .collect(),
    )
}

/// `UnitCondition` member names (`Kingmaker.UnitLogic/UnitCondition.cs`).
#[must_use]
pub fn condition_name(c: i32) -> &'static str {
    const NAMES: [&str; CONDITION_COUNT] = [
        "0",
        "Blindness",
        "Nauseated",
        "Fatigued",
        "Paralyzed",
        "DeathDoor",
        "Staggered",
        "Invisible",
        "Petrified",
        "Dazed",
        "Slowed",
        "Entangled",
        "DifficultTerrain",
        "Frightened",
        "Prone",
        "Sickened",
        "Sleeping",
        "SeeInvisibility",
        "TrueSeeing",
        "CantMove",
        "Shaken",
        "LoseDexterityToAC",
        "Dazzled",
        "Stunned",
        "Helpless",
        "Confusion",
        "SpellCastingIsDifficult",
        "ForceMove",
        "CantAct",
        "DisableAttacksOfOpportunity",
        "AttackNearest",
        "Unconscious",
        "CanNotAttack",
        "Unlootable",
        "PartialConcealmentOnAttacks",
        "ImmuneToAttackOfOpportunity",
        "UseMobilityToNegateAttackOfOpportunity",
        "ImmuneToCombatManeuvers",
        "MovementBan",
        "AttackOfOpportunityBeforeInitiative",
        "StealthForbidden",
        "Cowering",
        "Exhausted",
        "BarbarianRage",
        "SuppressedEnergyResistance",
        "SuppressedEnergyImmunity",
        "UseAbilityForbidden",
        "WalkingDead",
        "DisableMountRiding",
        "SpellCastingIsVeryDifficult",
        "DevouredBySwarm",
        "SpellcastingForbidden",
        "MagicItemsForbidden",
        "ExAndSuAbilitiesForbidden",
        "GreaterInvisibility",
        "CantUseStandardActions",
        "UseMobilityToNegateDefensivelyFail",
        "NegatedSpellblight",
    ];
    NAMES.get(idx(c)).copied().unwrap_or("?")
}

// ---- components ---------------------------------------------------------------------

pub fn register(r: &mut Registry) {
    use crate::components::{Port, bits};
    r.register(Box::new(Port::new(
        "AddCondition",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        add_condition_component,
    )));
    r.register(Box::new(Port::new(
        "AddConditionImmunity",
        bits(&[Hook::TurnOn, Hook::TurnOff]),
        add_condition_immunity_component,
    )));
}

/// The component's `Exceptions` (`UnitConditionExceptions`, `[SerializeReference]`):
/// non-null exceptions aren't modelled.
fn exceptions_present(w: &World, c: CompRef) -> bool {
    crate::components::with_settings(w, c, |v| v.obj("Exceptions").is_some()).unwrap_or(false)
}

/// `AddCondition.OnTurnOn`/`OnTurnOff`: `Owner.State.AddCondition(Condition, Fact as Buff,
/// Exceptions)` / `RemoveCondition(Condition, Exceptions)`.
fn add_condition_component(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = crate::components::owner_unit(w, c) else { return };
    let Some(condition) = crate::components::with_settings(w, c, |v| v.i32("Condition")) else {
        return;
    };
    if exceptions_present(w, c) {
        w.coverage.unsupported("AddCondition: condition exceptions");
    }
    let buff = (w.fact(c.fact).kind == crate::world::FactKind::Buff).then_some(c.fact);
    match h {
        Hook::TurnOn => add_condition(w, u, condition, buff),
        Hook::TurnOff => remove_condition(w, u, condition, false),
        _ => {}
    }
}

/// `AddConditionImmunity.OnTurnOn`/`OnTurnOff`.
fn add_condition_immunity_component(w: &mut World, c: CompRef, h: Hook) {
    let Some(u) = crate::components::owner_unit(w, c) else { return };
    let Some(condition) = crate::components::with_settings(w, c, |v| v.i32("Condition")) else {
        return;
    };
    match h {
        Hook::TurnOn => change_immunity(w, u, condition, true),
        Hook::TurnOff => change_immunity(w, u, condition, false),
        _ => {}
    }
}
