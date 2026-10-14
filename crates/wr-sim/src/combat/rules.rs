//! Combat rules (decompiled, not copied; each rule cites its file under
//! `Kingmaker.RuleSystem.Rules/`).

use std::any::Any;
use std::rc::Rc;

use serde_json::{Value as J, json};

use crate::content::{Blueprint, is_a};
use crate::csharp::TimeSpan;
use crate::rulebook::{RuleBase, RuleEvent};
use crate::world::{EntityIdx, World};

macro_rules! rule_boilerplate {
    () => {
        fn base(&self) -> &RuleBase {
            &self.base
        }
        fn base_mut(&mut self) -> &mut RuleBase {
            &mut self.base
        }
        fn as_any(&mut self) -> &mut dyn Any {
            self
        }
    };
}
#[allow(unused_imports)]
pub(crate) use rule_boilerplate;

/// Triggers a rule: with T0 records when a combat run is recording ([`super::fire`]),
/// else plainly (hydration, spawning).
pub fn trigger_any<R: RuleEvent>(
    w: &mut World,
    rule: &mut R,
    result: impl FnOnce(&R) -> Option<J>,
) {
    match super::fire::current(w) {
        Some(cx) => super::fire::fire(w, &cx, rule, result),
        // A dialogue run records its rules (`rules::trigger_rule`; nothing while off).
        None => {
            crate::rules::trigger_rule(w, rule);
            w.rules_end_fields.remove(&rule.base().seq);
        }
    }
}

/// `MechanicsFeatureType.Immortality`.
const IMMORTALITY: i64 = 44;

/// `SpellDescriptorComponent.Descriptor` of a blueprint (0 if none).
#[must_use]
pub fn spell_descriptor(bp: &Blueprint) -> i64 {
    bp.components_array()
        .iter()
        .find(|o| is_a(o.ty, "SpellDescriptorComponent"))
        .map_or(0, |o| crate::content::View(o).obj("Descriptor").map_or(0, |d| d.int("m_IntValue")))
}

/// `SpellDescriptorOperations.NegativeEffects` (`Kingmaker.Blueprints.Classes.Spells/
/// SpellDescriptorOperations.cs:12`): mind-affecting, fear, compulsion, emotion, poison,
/// disease, charm, daze, sickened, shaken, fatigue, staggered, nauseated, exhausted, stun,
/// paralysis, confusion, blindness, curse, sleep, stat debuff, bleed, Vilderavn bleed,
/// petrified, negative emotion, undead control.
pub const NEGATIVE_EFFECTS: i64 = 0x1066111fbfc1f0;

/// `SpellDescriptor.IsNegativeEffect()`.
#[must_use]
pub fn is_negative_effect(d: i64) -> bool {
    d & NEGATIVE_EFFECTS != 0
}

/// `RuleCanApplyBuff` (`RuleCanApplyBuff.cs`).
#[derive(Debug)]
pub struct RuleCanApplyBuff {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub blueprint: Rc<Blueprint>,
    pub duration: Option<TimeSpan>,
    pub can_apply: bool,
    pub ignore_immunity: bool,
    pub immunity: bool,
    /// The buff context's caster is someone else.
    pub foreign_caster: bool,
    /// The buff context's caster (`Context.MaybeCaster`).
    pub caster: Option<EntityIdx>,
    /// `Context.SpellDescriptor`.
    pub descriptor: i64,
}

impl RuleCanApplyBuff {
    #[must_use]
    pub fn new(initiator: EntityIdx, blueprint: Rc<Blueprint>, duration: Option<TimeSpan>) -> Self {
        Self {
            base: RuleBase::default(),
            initiator,
            blueprint,
            duration,
            can_apply: true,
            ignore_immunity: false,
            immunity: false,
            foreign_caster: false,
            caster: None,
            descriptor: 0,
        }
    }

    /// The projected result.
    #[must_use]
    pub fn result(&self) -> Option<J> {
        Some(json!({"CanApply": self.can_apply, "Immunity": self.immunity}))
    }
}

impl RuleEvent for RuleCanApplyBuff {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCanApplyBuff"]
    }

    fn initiator(&self) -> EntityIdx {
        self.initiator
    }

    fn on_trigger(&mut self, w: &mut World) {
        if !self.can_apply {
            return;
        }
        let descriptor = self.descriptor;
        if self.foreign_caster && is_negative_effect(descriptor) {
            w.coverage
                .unsupported("RuleCanApplyBuff: IsAttackingGreenNPC with a negative descriptor");
        }
        // `SkipBecauseOfShadow`: no shadow-spell context on these paths.
        if self.immunity && !self.ignore_immunity {
            self.can_apply = false;
            return;
        }
        // `StatsAdjustments == ExtraDecline` (enum value 0) for a player-faction target with a
        // negative effect shortens the duration: reported.
        let player = w.unit(self.initiator).is_some_and(|u| u.is_player_faction);
        let extra_decline = w.settings.difficulty.settings.stats_adjustments == Some(0);
        if player && extra_decline && is_negative_effect(descriptor) {
            w.coverage.unsupported("RuleCanApplyBuff: ExtraDecline story-mode immunity");
        }
        let remove_on_resurrect = crate::unit_facts::buff_flag(
            &self.blueprint,
            crate::unit_facts::BUFF_REMOVE_ON_RESURRECT,
        );
        let immortal = w
            .unit(self.initiator)
            .is_some_and(|u| u.mechanics_features.get(&IMMORTALITY).copied().unwrap_or(0) > 0);
        // `ForceKill` is false outside a kill action.
        self.can_apply &= !remove_on_resurrect || !immortal;
    }

    rule_boilerplate!();
}
