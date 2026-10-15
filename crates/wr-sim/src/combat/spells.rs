//! Spellcasting (`docs/poc5/spells.md`): a spellbook spell cast by a `UnitUseAbility` command,
//! from the command's lifecycle checks to the buff it applies. Ported (decompiled, not copied):
//! `Kingmaker.UnitLogic.Abilities/AbilityData.cs` (`IsAvailableForCast`, `IsAvailable`,
//! `CanTarget`, `CalculateParams`, `CreateExecutionContext`, `Spend`, `SpellSource`),
//! `AbilityCastRateUtils.cs`, `AbilityExecutionContext.cs`, `Kingmaker.RuleSystem.Rules.Abilities/
//! {RuleCastSpell,RuleApplySpell}.cs`, `Kingmaker.RuleSystem.Rules/RuleSpellResistanceCheck.cs`,
//! `Kingmaker.Controllers/{AbilityExecutionController,AbilityExecutionProcess}.cs` (the immediate
//! path: no delivery), `Kingmaker.UnitLogic.Abilities.Components/AbilityEffectRunAction.cs`,
//! `Kingmaker.UnitLogic.Mechanics/{ContextDurationValue,ContextValueHelper}.cs`.
//!
//! Fail closed: what a branch reads that isn't modelled (deliveries, area selections, saving
//! throws, metamagic, items, touch spells, the magus' and kineticist's parts, restrictions of
//! kinds not ported, spell resistance parts) is reported where the C# would read it.

use std::any::Any;
use std::rc::Rc;

use serde_json::{Value as J, json};
use wr_content::Guid;

use super::rules::{rule_boilerplate, trigger_any};
use crate::content::{Blueprint, View, is_a};
use crate::context::{MechanicsContext, SourceAbility};
use crate::csharp::{TimeSpan, arith};
use crate::elements::MechRef;
use crate::rulebook::{RuleBase, RuleEvent};
use crate::spellbook::Spell;
use crate::unit_state::has_condition;
use crate::world::{EntityIdx, World};

/// `UnitCondition` values these checks read.
mod cond {
    pub const STAGGERED: i32 = 6;
    pub const USE_ABILITY_FORBIDDEN: i32 = 46;
    pub const SPELLCASTING_FORBIDDEN: i32 = 51;
    pub const EX_AND_SU_ABILITIES_FORBIDDEN: i32 = 53;
    pub const CANT_USE_STANDARD_ACTIONS: i32 = 55;
}

/// `SpellSource`.
pub mod source {
    pub const ARCANE: i64 = 0;
    pub const DIVINE: i64 = 1;
    pub const UNKNOWN: i64 = 2;
    pub const NONE: i64 = 100;
}

/// Component classes implementing `IAbilityRestriction` (`BlueprintAbility.Restrictions`).
const RESTRICTIONS: &[&str] = &[
    "AbilityAcceptBurnOnCast",
    "AbilityApplyFact",
    "AbilityRequirementCanMove",
    "AbilityRequirementHasCondition",
    "AbilityRequirementHasItemInCollection",
    "AbilityRequirementHasItemInHands",
    "AbilityRequirementMountHasAction",
    "AbilityRequirementOnMount",
    "AbilityResourceLogic",
    "AbilityRestoreSpellSlot",
    "AbilityRestrictionByArmor",
    "AbilityRestrictionByShield",
    "AbilityRestrictionByWeapon",
    "AbilitySwtichDualCompanionChecker",
];

/// Component classes implementing `IAbilityCasterRestriction` (`CasterRestrictions`).
const CASTER_RESTRICTIONS: &[&str] = &[
    "AbilityCasterAlignment",
    "AbilityCasterHasChosenWeapon",
    "AbilityCasterHasFacts",
    "AbilityCasterHasNoFacts",
    "AbilityCasterHasWeaponWithRangeType",
    "AbilityCasterInCombat",
    "AbilityCasterIsOnFavoredTerrain",
    "AbilityCasterMainWeaponCheck",
    "AbilityCasterMainWeaponIsMelee",
    "AbilityCasterMainWeaponIsTwoHanded",
    "AbilityCasterNotPolymorphed",
    "AbilityMaxSquadsRestriction",
];

/// Component classes implementing `IAbilityTargetRestriction` (`TargetRestrictions`) but the
/// ported `AbilityTargetHasFact`.
const TARGET_RESTRICTIONS: &[&str] = &[
    "AbilityCustomCharge",
    "AbilityCustomDimensionDoor",
    "AbilityCustomFly",
    "AbilityCustomOverrun",
    "AbilityCustomTeleportation",
    "AbilityCustomTongueGrab",
    "AbilityDemonCharge",
    "AbilityGriffinAttack",
    "AbilityTargetAlignment",
    "AbilityTargetBreathOfLife",
    "AbilityTargetCanSeeCaster",
    "AbilityTargetCellsRestriction",
    "AbilityTargetDivineTroth",
    "AbilityTargetHasCondition",
    "AbilityTargetHasConditionOrBuff",
    "AbilityTargetHasMeleeWeaponInPrimaryHand",
    "AbilityTargetHasNoFactUnless",
    "AbilityTargetHasOneOfConditionsOrHP",
    "AbilityTargetHPCondition",
    "AbilityTargetIsAlly",
    "AbilityTargetIsAnimalCompanion",
    "AbilityTargetIsAreaEffectFromCaster",
    "AbilityTargetIsDeadAnimalCompanion",
    "AbilityTargetIsDeadCompanion",
    "AbilityTargetIsFavoredEnemy",
    "AbilityTargetIsNotDevoured",
    "AbilityTargetIsPartyMember",
    "AbilityTargetIsSuitableMount",
    "AbilityTargetIsSuitableMountSize",
    "AbilityTargetMaximumHitDice",
    "AbilityTargetNotSelf",
    "AbilityTargetRangeRestriction",
    "AbilityTargetStatCondition",
    "AbilityTargetStoneToFlesh",
    "ArmyAbilityHook",
];

/// Component classes implementing `IAbilityRequiredParameters`.
const REQUIRED_PARAMETERS: &[&str] = &[
    "AbilityApplyFact",
    "AbilityResourceOverride",
    "AbilityConvertSpellLevelToResource",
    "AbilityRestoreSpellSlot",
];

/// Component classes implementing `ICanTargetDeadUnits` (`CanCastToDeadTarget`).
const CAN_TARGET_DEAD: &[&str] = &[
    "AbilityTargetBreathOfLife",
    "AbilityTargetIsNotDevoured",
    "AbilityCanTargetDead",
    "AbilityTargetIsDeadAnimalCompanion",
    "AbilityTargetStoneToFlesh",
    "AbilityTargetIsDeadCompanion",
];

/// Components of the ability blueprint whose effect on the cast isn't ported.
const UNPORTED_CAST: &[&str] = &[
    "AbilityDeliverEffect",
    "AbilityTargetsAround",
    "AbilityEffectStickyTouch",
    "AbilityKineticist",
    "AbilityCustomLogic",
    "AbilityVariants",
    "AdditionalAbilityEffectRunActionOnClickedTarget",
    "AbilityAcceptBurnOnCast",
    "AbilityExecuteActionOnCast",
    "ContextAbilityParamsCalculator",
    "ContextSetAbilityParams",
    "ContextRankConfig",
    "ContextCalculateSharedValue",
    "PropertyCalculatorComponent",
    "SpellTypeOverride",
    "PretendSpellLevel",
    "AbilityShadowSpell",
];

fn has_any(bp: &Blueprint, names: &[&str]) -> Option<&'static str> {
    bp.components_array().iter().find_map(|o| {
        names.iter().find(|n| is_a(o.ty, n)).map(|_| wr_content::schema::ty(o.ty).short)
    })
}

/// An `AbilityData` of a memorizing spellbook (the spell its slot holds).
#[derive(Debug, Clone)]
pub struct Ability {
    pub caster: EntityIdx,
    pub spellbook: Guid,
    pub spell: Spell,
}

impl Ability {
    /// The spell of `caster`'s spellbook `sb` a memorized slot of `level` holds (the driver's
    /// `MechanicActionBarSlotMemorizedSpell`: the slot's `SpellShell`, equal by value to any
    /// other slot's of that spell).
    ///
    /// # Errors
    ///
    /// No such spellbook or slot.
    pub fn memorized(
        w: &World,
        caster: EntityIdx,
        sb: Guid,
        blueprint: Guid,
        level: i32,
    ) -> Result<Self, String> {
        let book = w
            .unit(caster)
            .and_then(|u| u.spellbooks.get(&sb))
            .ok_or_else(|| "the caster has no such spellbook".to_owned())?;
        let lv = usize::try_from(level).map_err(|_| "a negative spell level".to_owned())?;
        let spell = book
            .memorized
            .get(lv)
            .into_iter()
            .flatten()
            .filter_map(|s| s.spell.as_ref())
            .find(|s| s.blueprint == blueprint && book.spell_level_of(s) == level)
            .ok_or_else(|| "no memorized slot of the spell at the level".to_owned())?;
        Ok(Self { caster, spellbook: sb, spell: spell.clone() })
    }

    #[must_use]
    pub fn bp(&self, w: &World) -> Option<Rc<Blueprint>> {
        w.content.get(self.spell.blueprint)
    }

    fn sb_bp(&self, w: &World) -> Option<Rc<Blueprint>> {
        w.content.get(self.spellbook)
    }

    /// `SpellLevel`: the spellbook's level of it.
    #[must_use]
    pub fn spell_level(&self, w: &World) -> i32 {
        crate::spellbook::spell_level(w, self.caster, self.spellbook, &self.spell)
    }

    /// `SpellSchool` (`Blueprint.School`).
    #[must_use]
    pub fn school(&self, w: &World) -> i64 {
        self.bp(w).map_or(0, |b| {
            b.components_array()
                .iter()
                .find(|o| is_a(o.ty, "SpellComponent"))
                .map_or(0, |o| View(o).int("School"))
        })
    }

    /// `SpellSource` (`CalcSpellSource`): a spell or spell-like ability of a spellbook is
    /// arcane or divine by the spellbook's `IsArcane`; anything else `None`.
    #[must_use]
    pub fn spell_source(&self, w: &World) -> i64 {
        let ty = self.bp(w).map_or(-1, |b| b.view().int("Type"));
        if ty != 0 && ty != 1 {
            return source::NONE;
        }
        if self.sb_bp(w).is_some_and(|b| b.view().bool("IsArcane")) {
            source::ARCANE
        } else {
            source::DIVINE
        }
    }
}

/// `AbilityData.ActionType`: `GetDefaultActionType`, the blueprint's for a spellbook spell
/// without metamagic data and no item (`HasMetamagic(Quicken)` also reads the caster's
/// `UnitPartSpecialMetamagic`: reported), without a `UnitPartAbilityModifiers` override
/// (reported), as `commands::ty`.
fn action_type(w: &World, a: &Ability) -> Result<u8, String> {
    let bp = a.bp(w).ok_or_else(|| "the spell's blueprint is missing".to_owned())?;
    if let Some(p) = part(w, a.caster, &["UnitPartSpecialMetamagic", "UnitPartAbilityModifiers"]) {
        return Err(format!("AbilityData.ActionType: {p}"));
    }
    u8::try_from(bp.view().int("ActionType")).map_err(|_| "an action type".to_owned())
}

/// `AbilityData.RuntimeActionType` (`ActionType`; no Quicken; no mythic abilities as move
/// actions), as `commands::ty`.
///
/// # Errors
///
/// A branch that reads what isn't modelled.
pub fn runtime_action_type(w: &World, a: &Ability) -> Result<u8, String> {
    let ty = action_type(w, a)?;
    if ty == super::commands::ty::STANDARD
        && crate::components::state::has_mechanics_feature(
            w,
            a.caster,
            &["MythicAbilitiesAsMoveAction"],
        )
    {
        return Err("AbilityData.RuntimeActionType: MythicAbilitiesAsMoveAction".into());
    }
    Ok(ty)
}

/// `AbilityData.RequireFullRoundAction`: a standard spell without metamagic whose blueprint is a
/// full-round action (`IsFullRoundAction`: `m_IsFullRoundAction` of a standard action; an
/// `AbilityIsFullRoundInTurnBased` is reported).
///
/// # Errors
///
/// A branch that reads what isn't modelled.
pub fn require_full_round_action(w: &World, a: &Ability) -> Result<bool, String> {
    if runtime_action_type(w, a)? != super::commands::ty::STANDARD {
        return Ok(false);
    }
    let bp = a.bp(w).ok_or_else(|| "the spell's blueprint is missing".to_owned())?;
    if let Some(c) = has_any(&bp, &["AbilityIsFullRoundInTurnBased"]) {
        return Err(format!("BlueprintAbility.IsFullRoundAction: {c}"));
    }
    // (`IsStandardAction`: the blueprint's `ActionType`, standard here.)
    let standard = bp.view().int("ActionType") == i64::from(super::commands::ty::STANDARD);
    Ok(standard && bp.view().bool("m_IsFullRoundAction"))
}

/// `UnitState.CanCast`: no `UseAbilityForbidden`.
#[must_use]
pub fn can_cast(w: &World, u: EntityIdx) -> bool {
    !has_condition(w, u, cond::USE_ABILITY_FORBIDDEN)
}

/// `AbilityData.IsAvailableForCast` (no conversion, no service caster: the spell has no source
/// item): `CanBeCastByCaster`.
///
/// # Errors
///
/// A branch that reads what isn't modelled.
pub fn is_available_for_cast(w: &mut World, a: &Ability) -> Result<bool, String> {
    let bp = a.bp(w).ok_or_else(|| "the spell's blueprint is missing".to_owned())?;
    let u = a.caster;
    if let Some(c) = has_any(&bp, &["AbilityEffectStickyTouch", "AbilityResourceLogic"]) {
        return Err(format!("AbilityData.GetAvailableForCastCount: {c}"));
    }
    if a.spell.save_spellbook_slot {
        return Err("AbilityData.GetAvailableForCastCount: SaveSpellbookSlot".into());
    }
    // `GetAvailableCastsCountFromCharges`: the spellbook's count / `SpellSlotCost`; no resources.
    let charges = crate::spellbook::available_for_cast_count(w, u, a.spellbook, &a.spell);
    let cost = arith::add(1, a.spell.extra_spell_slot_cost);
    let count = if charges >= 0 && cost > 0 { charges / cost } else { charges };
    if count == 0 {
        return Ok(false);
    }
    if let Some(c) = has_any(&bp, CASTER_RESTRICTIONS) {
        return Err(format!("AbilityData.CasterRestrictions: {c}"));
    }
    if has_condition(w, u, cond::STAGGERED) && require_full_round_action(w, a)? {
        return Ok(false);
    }
    // `ActionType == Standard && HasCondition(CantUseStandardActions)`.
    if action_type(w, a)? == super::commands::ty::STANDARD
        && has_condition(w, u, cond::CANT_USE_STANDARD_ACTIONS)
    {
        return Ok(false);
    }
    if !can_cast(w, u) {
        return Ok(false);
    }
    let ty = bp.view().int("Type");
    if ty == 0 || ty == 1 {
        // `SpellcastingForbidden && (SourceItem == null | ...)`.
        if has_condition(w, u, cond::SPELLCASTING_FORBIDDEN) {
            return Ok(false);
        }
        // `Body.IsPolymorphed` (Natural Spell).
        if super::inspect::polymorphed(w, u) {
            return Err("AbilityData.CanBeCastByCaster: a polymorphed caster".into());
        }
        if let Some(p) = part(w, u, &["UnitPartForbiddenSpellbooks"]) {
            return Err(format!("AbilityData.CanBeCastByCaster: {p}"));
        }
    }
    if (ty == 2 || ty == 3) && has_condition(w, u, cond::EX_AND_SU_ABILITIES_FORBIDDEN) {
        return Ok(false);
    }
    if bp.view().obj("MaterialComponent").and_then(|m| m.guid("m_Item")).is_some() {
        return Err("AbilityData.RequireMaterialComponent".into());
    }
    if let Some(c) = has_any(&bp, REQUIRED_PARAMETERS) {
        return Err(format!("AbilityData.HasRequiredParams: {c}"));
    }
    if let Some(c) = has_any(&bp, RESTRICTIONS) {
        return Err(format!("AbilityData.Restrictions: {c}"));
    }
    if let Some(p) = part(w, u, &["UnitPartSaddled", "UnitPartRider"]) {
        return Err(format!("AbilityData.ShouldDelegateToMount: {p}"));
    }
    if unit_has_component(w, u, "SuppressSpellSchool") {
        return Err("UnitState.IsSpellSchoolSuppressed".into());
    }
    Ok(true)
}

/// `IsAvailable`: `IsAvailableInSpellbook` (`Spellbook.CanSpend`) and `IsAvailableForCast`, not
/// `TemporarilyDisabled`.
///
/// # Errors
///
/// As [`is_available_for_cast`].
pub fn is_available(w: &mut World, a: &Ability) -> Result<bool, String> {
    if !crate::spellbook::spend_spell(w, a.caster, a.spellbook, &a.spell, false) {
        return Ok(false);
    }
    Ok(is_available_for_cast(w, a)? && !a.spell.temporarily_disabled)
}

/// `AbilityData.CanTarget(target)` for a unit target (no mount, no magic hack).
///
/// # Errors
///
/// A branch that reads what isn't modelled.
pub fn can_target(w: &mut World, a: &Ability, target: EntityIdx) -> Result<bool, String> {
    let bp = a.bp(w).ok_or_else(|| "the spell's blueprint is missing".to_owned())?;
    let v = bp.view();
    if let Some(p) = part(w, a.caster, &["UnitPartSaddled", "UnitPartRider"]) {
        return Err(format!("AbilityData.CanTarget: {p}"));
    }
    for o in bp.components_array() {
        if TARGET_RESTRICTIONS.iter().any(|n| is_a(o.ty, n)) {
            return Err(format!(
                "AbilityData.TargetRestrictions: {}",
                wr_content::schema::ty(o.ty).short
            ));
        }
        if is_a(o.ty, "AbilityTargetHasFact") && !target_has_fact(w, View(o), a.caster, target)? {
            return Ok(false);
        }
    }
    if !v.bool("CanTargetSelf") && target == a.caster {
        return Ok(false);
    }
    let dead = super::units::is_dead(w, target);
    if dead && has_any(&bp, CAN_TARGET_DEAD).is_none() {
        return Ok(false);
    }
    // `TargetAnchor`.
    if a.spell.force_target_owner {
        return Ok(target == a.caster);
    }
    if a.sb_bp(w).is_some_and(|b| has_any(&b, &["IsAlchemistSpellbook"]).is_some()) {
        return Err("AbilityData.TargetAnchor: an alchemist's spell".into());
    }
    const PERSONAL: i64 = 6;
    let range = v.int("Range");
    if range == PERSONAL {
        return Err("AbilityData.TargetAnchor: a personal spell".into());
    }
    let (friends, enemies, point) =
        (v.bool("CanTargetFriends"), v.bool("CanTargetEnemies"), v.bool("CanTargetPoint"));
    if !friends && !enemies && !point {
        return Ok(target == a.caster);
    }
    if point {
        // `AbilityTargetAnchor.Point`.
        return Ok(true);
    }
    // `AbilityTargetAnchor.Unit`.
    let player = w.unit(target).is_some_and(|x| x.is_player_faction);
    if !friends && !player && !super::groups::can_attack(w, a.caster, target) {
        return Ok(false);
    }
    Ok(true)
}

/// `AbilityTargetHasFact.IsTargetRestrictionPassed`.
fn target_has_fact(
    w: &World,
    v: View<'_>,
    caster: EntityIdx,
    target: EntityIdx,
) -> Result<bool, String> {
    let from_caster = v.bool("FromCaster");
    let caster_id = &w.entity(caster).unique_id;
    let mut flag = false;
    for g in v.guids("m_CheckedFacts") {
        flag = w.entity(target).facts.list.iter().any(|f| {
            let fact = w.fact(*f);
            fact.blueprint == Some(g)
                && (!from_caster
                    || fact.context.as_ref().and_then(|c| c.caster.as_ref()) == Some(caster_id))
        });
        if flag {
            break;
        }
    }
    Ok(flag != v.bool("Inverted"))
}

fn part(w: &World, u: EntityIdx, names: &[&'static str]) -> Option<&'static str> {
    w.entity(u).parts.iter().find(|p| names.contains(&p.clr)).map(|p| p.clr)
}

fn unit_has_component(w: &World, u: EntityIdx, name: &str) -> bool {
    w.entity(u).facts.list.iter().any(|f| {
        w.fact(*f)
            .components
            .iter()
            .any(|c| c.ty().is_some_and(|t| wr_content::schema::ty(t).short == name))
    })
}

// ---------------------------------------------------------------------------------------
// The execution context
// ---------------------------------------------------------------------------------------

/// An `AbilityExecutionContext`.
#[derive(Debug, Clone)]
pub struct AbilityCtx {
    pub ability: Ability,
    /// `ClickedTarget` (a unit).
    pub target: EntityIdx,
    /// Its `MechanicsContext` members: caster and owner the caster, `m_Params`, every rank the
    /// caster level, the blueprint's spell descriptor.
    pub mech: MechanicsContext,
    /// `IsForced`.
    pub is_forced: bool,
}

/// `AbilityExecutionController` and the contexts its processes run in.
#[derive(Debug, Default)]
pub struct Spells {
    pub contexts: Vec<AbilityCtx>,
    /// `m_Abilities`: each process by its context.
    pub processes: Vec<u32>,
    /// A cast whose context was made (`new RuleCastSpell`'s `CreateExecutionContext`) and whose
    /// rule hasn't triggered yet.
    pub pending: Option<u32>,
    /// The frame (a replay's tick) in which processes last ticked.
    pub ran_at: Option<i64>,
}

impl Spells {
    #[must_use]
    pub fn context(&self, i: u32) -> Option<&AbilityCtx> {
        usize::try_from(i).ok().and_then(|i| self.contexts.get(i))
    }
}

fn param(p: &J, k: &str) -> i32 {
    p[k].as_i64().and_then(|x| i32::try_from(x).ok()).unwrap_or(0)
}

/// `new RuleCastSpell(ability, target)`'s `spell.CreateExecutionContext(target)`:
/// `CalculateParams()` (`RuleCalculateAbilityParams` for a spellbook spell without a params
/// calculator, fired), then the context. Returns its index (the cast pending its rule).
pub fn begin_cast(w: &mut World, a: &Ability, target: EntityIdx) -> Option<u32> {
    let bp = a.bp(w)?;
    if let Some(c) = has_any(&bp, UNPORTED_CAST) {
        w.coverage.unsupported(format!("AbilityExecutionContext: {c} on the spell"));
        return None;
    }
    let mut rule = crate::rules::ability_params::RuleCalculateAbilityParams::new(a.caster, &bp);
    rule.spellbook = Some(a.spellbook);
    rule.spell_level = Some(a.spell_level(w));
    rule.spell_source = a.spell_source(w);
    rule.ability_data = true;
    trigger_any(w, &mut rule, |r| {
        // The oracle's snapshot of `Result` has its computed `IsChanged` too.
        let mut p = r.result.clone()?;
        let i = |k: &str| p[k].as_i64().unwrap_or(0);
        let changed = !(i("SpellSource") == source::UNKNOWN
            && i("CasterLevel") == 1
            && i("SpellLevel") == 1
            && i("DC") == 11
            && i("Concentration") == 0
            && i("Metamagic") == 0)
            || i("RankBonus") != 0;
        p["IsChanged"] = J::Bool(changed);
        Some(json!({ "Result": p }))
    });
    let p = rule.result?;
    let cl = param(&p, "CasterLevel");
    let caster = w.entity(a.caster).unique_id.clone();
    let mech = MechanicsContext {
        associated_blueprint: Some(bp.guid),
        owner: Some(caster.clone()),
        caster: Some(caster),
        ranks: vec![cl; 7],
        shared_values: vec![0; 7],
        params: Some(p),
        has_ability_context: true,
        spell_descriptor: super::rules::spell_descriptor(&bp),
        source_ability: Some(SourceAbility {
            blueprint: bp.guid,
            spellbook: Some(a.spellbook),
            school: a.school(w),
        }),
        ..MechanicsContext::default()
    };
    let i = u32::try_from(w.combat.spells.contexts.len()).ok()?;
    w.combat.spells.contexts.push(AbilityCtx {
        ability: a.clone(),
        target,
        mech,
        is_forced: false,
    });
    w.combat.spells.pending = Some(i);
    Some(i)
}

/// What `UnitUseAbility.OnAction` gets from its `RuleCastSpell`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CastOutcome {
    pub success: bool,
    pub spent: bool,
}

/// `Rulebook.Trigger(ruleCastSpell)` for the pending context (`IsCutscene` false), then, as
/// `UnitUseAbility.OnAction` goes on, `ShouldSpendResource`'s `Ability.Spend()`.
pub fn cast(w: &mut World, i: u32, cutscene: bool) -> Option<CastOutcome> {
    let c = w.combat.spells.context(i)?.clone();
    let mut rule = RuleCastSpell {
        base: RuleBase::default(),
        ctx: i,
        initiator: c.ability.caster,
        target: c.target,
        is_cutscene: cutscene,
        success: false,
        spell_failure_chance: 0,
    };
    trigger_any(w, &mut rule, RuleCastSpell::result);
    // `ShouldSpendResource`: not `ForceFail`, no failed device check (neither modelled).
    spend(w, &c.ability);
    Some(CastOutcome { success: rule.success, spent: true })
}

/// `AbilityData.Spend()` of a spellbook spell: no material component, no item, the spellbook's
/// slot (`SpendFromSpellbook`: `SpellSlotCost` charges), no resources.
fn spend(w: &mut World, a: &Ability) {
    if a.spell.save_spellbook_slot {
        return;
    }
    let cost = arith::add(1, a.spell.extra_spell_slot_cost);
    let mut n = cost;
    while n > 0 {
        n -= 1;
        crate::spellbook::spend_spell(w, a.caster, a.spellbook, &a.spell, true);
    }
}

/// `RuleCastSpell`.
#[derive(Debug)]
pub struct RuleCastSpell {
    pub base: RuleBase,
    pub ctx: u32,
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub is_cutscene: bool,
    pub success: bool,
    /// `SpellFailureChance` (raised by handlers).
    pub spell_failure_chance: i32,
}

impl RuleCastSpell {
    fn result(&self) -> Option<J> {
        Some(json!({
            "Success": self.success,
            "SpellFailureChance": self.spell_failure_chance,
            "ArcaneSpellFailureChance": 0,
            "IsArcaneSpellFailed": false,
            "IsSpellFailed": false,
        }))
    }
}

impl RuleEvent for RuleCastSpell {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCastSpell"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    /// `GetRuleTarget()`: `SpellTarget.Unit`.
    fn target(&self) -> Option<EntityIdx> {
        Some(self.target)
    }
    fn record_target(&self) -> bool {
        false
    }
    fn on_trigger(&mut self, w: &mut World) {
        let Some(c) = w.combat.spells.context(self.ctx).cloned() else { return };
        // The controllers' global handlers of this rule (`TouchSpellsController`,
        // `MagusController`, `KineticistController`) act only for a held touch charge, a magus or
        // a kineticist's blast (the spell has no `AbilityKineticist`: checked as the cast began).
        if let Some(p) =
            part(w, self.initiator, &["UnitPartTouch", "UnitPartMagus", "UnitPartKineticist"])
        {
            w.coverage.unsupported(format!("RuleCastSpell's controller handlers: {p}"));
        }
        let mut before =
            RuleApplySpell::new(w, self.ctx, c.ability.caster, Some(self.target), false);
        trigger_any(w, &mut before, RuleApplySpell::result);
        // `Context.IsForced = IsCutscene || Initiator.Blueprint.IsCheater`.
        let cheater = w
            .unit(self.initiator)
            .and_then(|u| u.blueprint)
            .and_then(|g| w.content.get(g))
            .is_some_and(|b| b.view().bool("IsCheater"));
        let forced = self.is_cutscene || cheater;
        if let Some(x) = w.combat.spells.contexts.get_mut(self.ctx as usize) {
            x.is_forced = forced;
        }
        match can_target(w, &c.ability, self.target) {
            Ok(true) => {}
            Ok(false) if !forced => {
                w.log("error", "Invalid target for spell");
                return;
            }
            Ok(false) => {}
            Err(e) => {
                w.coverage.unsupported(format!("RuleCastSpell: {e}"));
                return;
            }
        }
        // `UseMagicDeviceDC`: no item. `SpellFailureChance` (handlers): reported.
        if self.spell_failure_chance > 0 && !forced {
            w.coverage.unsupported("RuleCastSpell: a spell failure chance");
            return;
        }
        // `IsAffectedByArcaneSpellFailure`: an arcane spellbook spell without touch delivery.
        let standalone =
            crate::spellbook::standalone_mythic(w, c.ability.caster, c.ability.spellbook);
        if c.ability.spell_source(w) == source::ARCANE && !standalone && !forced {
            w.coverage.unsupported("RuleCastSpell: arcane spell failure");
            return;
        }
        // `ShadowSpellSettings` (a converted spell's): none. `Spell.Cast(Context)`.
        self.success = true;
        w.combat.spells.processes.push(self.ctx);
    }
    rule_boilerplate!();
}

/// A cast's `Spell`, `Context` and `SpellTarget` (`target`) as WrathOracle writes them in a rule's
/// snapshot (`RuleCastSpell`, `RuleApplySpell`); `None` without the context.
#[must_use]
pub fn spell_inputs(w: &World, ctx: u32, target: Option<EntityIdx>) -> Option<J> {
    let c = w.combat.spells.context(ctx)?;
    let caster = json!({ "$unit": w.entity(c.ability.caster).unique_id });
    let context = c.mech.associated_blueprint.map(|g| g.to_string());
    // `TargetWrapper(unit)`: `m_Point` left zero, no orientation.
    let spell_target = target.map_or(J::Null, |t| {
        json!({
            "UnitRef": { "$unit": w.entity(t).unique_id },
            "m_Point": [0.0, 0.0, 0.0],
            "m_Orientation": null,
            "IsUnit": true,
            "IsOrientationSpecified": false,
        })
    });
    Some(json!({
        "Spell": { "$ability": c.ability.spell.blueprint.to_string(), "caster": caster },
        "Context": { "$context": context, "caster": caster },
        "SpellTarget": spell_target,
    }))
}

/// `RuleApplySpell` (`OnTrigger` empty).
#[derive(Debug)]
pub struct RuleApplySpell {
    pub base: RuleBase,
    pub ctx: u32,
    pub initiator: EntityIdx,
    pub target: Option<EntityIdx>,
    /// `Place`: `After`.
    pub after: bool,
    /// Its inputs as the oracle writes them (`Spell`, `Context`, `SpellTarget`, `Place`): its
    /// result fields in T0 (it changes none of them), as its target is the oracle's `$impure`.
    inputs: Option<J>,
}

impl RuleApplySpell {
    #[must_use]
    pub fn new(
        w: &World,
        ctx: u32,
        initiator: EntityIdx,
        target: Option<EntityIdx>,
        after: bool,
    ) -> Self {
        let inputs = spell_inputs(w, ctx, target).map(|mut o| {
            o["Place"] = json!(i32::from(after));
            o
        });
        Self { base: RuleBase::default(), ctx, initiator, target, after, inputs }
    }

    fn result(&self) -> Option<J> {
        self.inputs.clone()
    }
}

impl RuleEvent for RuleApplySpell {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleApplySpell"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn target(&self) -> Option<EntityIdx> {
        self.target
    }
    fn record_target(&self) -> bool {
        false
    }
    fn on_trigger(&mut self, _w: &mut World) {}
    rule_boilerplate!();
}

/// `RuleSpellResistanceCheck`.
#[derive(Debug)]
pub struct RuleSpellResistanceCheck {
    pub base: RuleBase,
    pub ctx: u32,
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub target_is_immune: bool,
    pub spell_resistance: i32,
    pub spell_penetration: i32,
    pub roll: Option<i32>,
}

impl RuleSpellResistanceCheck {
    fn result(&self) -> Option<J> {
        Some(json!({
            "TargetIsImmune": self.target_is_immune,
            "SpellResistance": self.spell_resistance,
            "SpellPenetration": self.spell_penetration,
            "Roll": self.roll,
        }))
    }

    /// `IsSpellResisted`: with no spell resistance (none modelled) it rolls but never resists,
    /// unless the target is immune.
    fn is_spell_resisted(&self, w: &World) -> bool {
        let primal = crate::components::state::has_mechanics_feature(
            w,
            self.initiator,
            &["PrimalMagicEssence"],
        );
        if primal {
            return false;
        }
        if self.target_is_immune {
            return true;
        }
        // `HasResistanceRoll` needs `SpellResistance > 0`.
        false
    }
}

impl RuleEvent for RuleSpellResistanceCheck {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleSpellResistanceCheck", "RulebookTargetEvent"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn target(&self) -> Option<EntityIdx> {
        Some(self.target)
    }
    fn on_trigger(&mut self, w: &mut World) {
        if part(w, self.target, &["UnitPartSpellResistance"]).is_some() {
            w.coverage.unsupported("RuleSpellResistanceCheck: UnitPartSpellResistance");
            return;
        }
        if part(w, self.initiator, &["UnitPartSpellResistanceCheckReplace"]).is_some() {
            w.coverage.unsupported("RuleSpellResistanceCheck: UnitPartSpellResistanceCheckReplace");
            return;
        }
        let cl = w
            .combat
            .spells
            .context(self.ctx)
            .and_then(|c| c.mech.params.as_ref())
            .map_or(0, |p| param(p, "CasterLevel"));
        self.spell_penetration = cl;
        // `Roll = Dice.D20`.
        let mut d20 = crate::rules::checks::RuleRollDice::d20(self.initiator);
        trigger_any(w, &mut d20, |r| Some(json!({ "Result": r.result() })));
        self.roll = Some(d20.result());
        crate::rulebook::bus::raise_global_call(w, "IWasRoll", "WasRoll", Some(self.target));
    }
    rule_boilerplate!();
}

// ---------------------------------------------------------------------------------------
// AbilityExecutionController
// ---------------------------------------------------------------------------------------

/// `AbilityExecutionController.Tick`: each process ticks; ended ones go. How many ticked.
pub fn tick(w: &mut World) -> usize {
    let list = std::mem::take(&mut w.combat.spells.processes);
    let n = list.len();
    let mut keep = Vec::new();
    for i in list {
        if !process_tick(w, i) {
            keep.push(i);
        }
    }
    // (A process started during the tick is appended after the list.)
    let added = std::mem::take(&mut w.combat.spells.processes);
    keep.extend(added);
    w.combat.spells.processes = keep;
    n
}

/// `AbilityExecutionProcess.Tick` of a spell without a delivery: its routine runs to its end in
/// the first tick (`PrepareCast`, `ApplyEffect`, no approaching effects). Whether it ended.
fn process_tick(w: &mut World, i: u32) -> bool {
    let Some(c) = w.combat.spells.context(i).cloned() else { return true };
    let Some(bp) = c.ability.bp(w) else { return true };
    crate::rulebook::bus::raise_global_call(
        w,
        "IAbilityExecutionProcessHandler",
        "HandleExecutionProcessStart",
        None,
    );
    // `PrepareCast`: `context.Recalculate()` (no params calculator, rank configs, shared values
    // or properties on the spell: checked as the cast began) and `IAbilityOnCastLogic`s (none).
    // `SpawnFxs` are presentation.
    let effect = bp.components_array().into_iter().position(|o| is_a(o.ty, "AbilityApplyEffect"));
    if let Some(e) = effect {
        apply_effect(w, i, &c, &bp, e);
    }
    crate::rulebook::bus::raise_global_call(
        w,
        "IAbilityExecutionProcessHandler",
        "HandleExecutionProcessEnd",
        None,
    );
    true
}

/// `AbilityExecutionProcess.ApplyEffect(context, ClickedTarget, effect, null)`: `DoApplyEffect`,
/// the effect's handlers, then `RuleApplySpell(After)` without a target.
fn apply_effect(w: &mut World, i: u32, c: &AbilityCtx, bp: &Rc<Blueprint>, effect: usize) {
    do_apply_effect(w, i, c, bp, effect);
    crate::rulebook::bus::raise_global_call(
        w,
        "IApplyAbilityEffectHandler",
        "OnAbilityEffectApplied",
        None,
    );
    let mut after = RuleApplySpell::new(w, i, c.ability.caster, None, true);
    trigger_any(w, &mut after, RuleApplySpell::result);
}

/// `DoApplyEffect(context, target, effect)`.
fn do_apply_effect(w: &mut World, i: u32, c: &AbilityCtx, bp: &Rc<Blueprint>, effect: usize) {
    let target = c.target;
    crate::rulebook::bus::raise_global_call(
        w,
        "IApplyAbilityEffectHandler",
        "OnTryToApplyAbilityEffect",
        Some(target),
    );
    let mut resisted = false;
    if target != c.ability.caster {
        let mut sr = RuleSpellResistanceCheck {
            base: RuleBase::default(),
            ctx: i,
            initiator: c.ability.caster,
            target,
            target_is_immune: false,
            spell_resistance: 0,
            spell_penetration: 0,
            roll: None,
        };
        trigger_any(w, &mut sr, RuleSpellResistanceCheck::result);
        resisted = sr.is_spell_resisted(w);
    }
    if !resisted {
        let objs = bp.components_array();
        let o = View(objs[effect]);
        if o.type_name() == "AbilityEffectRunAction" {
            effect_run_action(w, i, c, bp, o);
        } else {
            w.coverage.unsupported(format!("AbilityApplyEffect: {}", o.type_name()));
        }
        crate::rulebook::bus::raise_global_call(
            w,
            "IApplyAbilityEffectHandler",
            "OnAbilityEffectAppliedToTarget",
            Some(target),
        );
        let mut after = RuleApplySpell::new(w, i, c.ability.caster, Some(target), true);
        trigger_any(w, &mut after, RuleApplySpell::result);
    }
    // A personal spell on its caster is shared with the caster's pets in reach
    // (`SharePersonalSpellsWithPets`): not modelled.
    if target == c.ability.caster
        && crate::components::state::has_mechanics_feature(
            w,
            target,
            &["SharePersonalSpellsWithPets"],
        )
    {
        w.coverage.unsupported("AbilityExecutionProcess: SharePersonalSpellsWithPets");
    }
}

/// `AbilityEffectRunAction.Apply(context, target)`: nothing on the caster with `IgnoreCaster`; no
/// saving throw (`SavingThrowType` Unknown, else reported); the actions in the context's data scope
/// on the target.
fn effect_run_action(w: &mut World, i: u32, c: &AbilityCtx, bp: &Rc<Blueprint>, o: View<'_>) {
    if c.target == c.ability.caster && o.bool("IgnoreCaster") {
        return;
    }
    // `SavingThrowType.Unknown` is 0.
    if o.int("SavingThrowType") != 0 {
        w.coverage.unsupported("AbilityEffectRunAction: a saving throw");
        return;
    }
    let scope =
        crate::elements::Ctx::Mechanics { ctx: MechRef::Ability(i), target: Some(c.target) };
    crate::elements::with_ctx(w, scope, |w| {
        crate::elements::run_actions(w, bp, o.obj("Actions"));
    });
}

// ---------------------------------------------------------------------------------------
// Context values
// ---------------------------------------------------------------------------------------

/// `ContextValue.Calculate(context)` for the value types a context holds.
pub fn context_value(w: &mut World, ctx: &MechanicsContext, cv: Option<View<'_>>) -> i32 {
    let Some(cv) = cv else { return 0 };
    match cv.int("ValueType") {
        0 => cv.i32("Value"),
        1 => ctx.rank(cv.int("ValueRank")),
        2 => ctx.shared(cv.int("ValueShared")),
        t => {
            w.coverage.unsupported(format!("ContextValue.Calculate: ValueType {t}"));
            0
        }
    }
}

/// `ContextDurationValue.Calculate(context).Seconds` in the scope's context `r`: the dice value
/// (`ContextValueHelper.CalculateDiceValue`: a `RuleRollDice` through `context.TriggerRule`, no
/// maximize or empower), doubled by `Extend` when extendable, times the rate's rounds.
#[must_use]
pub fn duration(w: &mut World, r: MechRef, v: View<'_>) -> Option<TimeSpan> {
    let ctx = crate::elements::mech_context(w, r)?.clone();
    let dice = v.int("DiceType");
    let count = context_value(w, &ctx, v.obj("DiceCountValue"));
    let bonus = context_value(w, &ctx, v.obj("BonusValue"));
    let metamagic = ctx.effective_params().map_or(0, |p| p["Metamagic"].as_i64().unwrap_or(0));
    // `Metamagic.Maximize` 2, `Empower` 1, `Extend` 8 (`HasMetamagic`).
    if dice > 1 && metamagic & (1 | 2) != 0 {
        w.coverage.unsupported("ContextDurationValue: maximize or empower");
        return None;
    }
    let caster = crate::elements::mech_caster(w, r)
        .or_else(|| ctx.owner.as_ref().and_then(|id| w.by_id.get(id).copied()))?;
    // `context.TriggerRule(rule)`: an ability context triggers through its cloned rulebook
    // context (a root when no rule is open, as here).
    if matches!(r, MechRef::Ability(_)) && !w.rulebook.stack.is_empty() {
        w.coverage.unsupported("AbilityExecutionContext.TriggerRule inside another rule");
    }
    let sides = i32::try_from(dice).unwrap_or(0);
    let mut roll = crate::rules::checks::RuleRollDice::new(caster, count, sides);
    trigger_any(w, &mut roll, |x| Some(json!({ "Result": x.result() })));
    let mut num = arith::add(roll.result(), bonus);
    let dice_count_zero = is_zero(v.obj("DiceCountValue"));
    let extendable = v.bool("m_IsExtendable")
        && ((dice > 0 && !dice_count_zero) || !is_zero(v.obj("BonusValue")));
    if extendable && metamagic & 8 != 0 {
        num = arith::add(num, num);
    }
    // `DurationRate.ToRounds()`: Rounds 1, Minutes 10, TenMinutes 100, Hours 600, Days 14400.
    let rate = match v.int("Rate") {
        0 => 1,
        1 => 10,
        2 => 100,
        3 => 600,
        4 => 14400,
        _ => {
            w.coverage.unsupported("ContextDurationValue: an unknown rate");
            return None;
        }
    };
    let rounds = arith::mul(rate, num);
    // `Rounds.Seconds`: `((float)m_Value * 6f).Seconds()`.
    #[allow(clippy::cast_precision_loss)]
    let secs = rounds as f32 * 6.0f32;
    TimeSpan::from_seconds(f64::from(secs)).ok()
}

/// `ContextValue.IsZero`: a simple zero.
fn is_zero(cv: Option<View<'_>>) -> bool {
    cv.is_none_or(|c| c.int("ValueType") == 0 && c.i32("Value") == 0)
}
