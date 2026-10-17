//! The combat log (decompiled, not copied: `Kingmaker.UI.Models.Log/GameLogController.cs`,
//! `GameLogContext.cs`, `Kingmaker.UI.Models.Log.Events/*`,
//! `Kingmaker.UI.Models.Log.CombatLog_ThreadSystem/*` and its threads,
//! `Kingmaker.Blueprints.Root.Strings.GameLog/*`, `Kingmaker.TextTools/LogTemplate*.cs`);
//! the design and what is measured are in `docs/poc5/combat-log.md`.
//!
//! `GameLogController` turns rules and a few game events into log events: a rule of a *root*
//! type (one a thread handles) and every rule triggered inside one become events when they
//! start (its about-to-trigger hook) and are queued when they finish (its did-trigger hook), in
//! completion order; other events come from the event bus as they are raised. Its `Tick`, the
//! last controller of a frame, hands at most five queued events to their threads, in order
//! (the rest wait for the next frame); each thread makes its messages from the event and the
//! world as they are then, through `GameLogContext`, a set of static fields some of which
//! outlive every scope. The sim does the same with its own rules and state, and writes each
//! queued event, each tick's queue and each message to [`GameLog::out`], which
//! [`check`] compares with the recording's (WrathOracle's observer `combat_log`).

pub mod check;
pub mod strings;
pub mod tips;

use std::collections::HashMap;

use wr_text::{
    Color32, Env, LogIntField, LogRoll, LogRollField, LogStrField, LogUnit, LogUnitField,
};
use wr_text::{Gender, TextError};

use crate::rulebook::RuleEvent;
use crate::world::{EntityIdx, World};

/// `GameLogController.MaxLogsInFrame`.
pub const MAX_LOGS_IN_FRAME: usize = 5;

/// `LogThreadService.AllThreads` in order (`m_Logs`' channels in insertion order, each
/// channel's list in order, `Distinct` by instance: the threads shared with `GlobalMap` appear
/// once; `MessageLogThread` and `WarningNotificationLogThread` are two instances each), with
/// what each handles: `R:` a rule type (`IGameLogRuleHandler<T>`), `E:` an event type
/// (`IGameLogEventHandler<T>`), as `GetInterfaces` lists them (a base class's included).
pub const THREADS: &[(&str, &[&str])] = &[
    // Common
    ("UnitEvasionLogThread", &["E:GameLogEventEffectEvaded"]),
    ("RulebookRollChance", &["R:RuleRollChance"]),
    ("InteractionRestrictionLogThread", &["E:GameLogEventInteractionRestriction"]),
    ("PartyEncumbranceLogThread", &["E:GameLogEventPartyEncumbranceChanged"]),
    ("UnitEncumbranceLogThread", &["E:GameLogEventUnitEncumbrance"]),
    ("PerceptionLogThread", &["E:GameLogEventEntityNoticed"]),
    ("GameTimeAdvancedLogThread", &["E:GameLogEventTimeAdvanced"]),
    ("MessageLogThread", &["E:GameLogEventMessage"]),
    ("UnitFakeDeathMessageLogThread", &["E:GameLogEventUnitFakeDeathMessage"]),
    ("UnitDeflectArrowLogThread", &["E:GameLogEventUnitDeflectArrow"]),
    ("SpecialFeatureChangedLogThread", &["E:GameLogEventSpecialFeatureChanged"]),
    // AnyCombat
    ("DamageDealtLogThread", &["R:RuleDealDamage", "E:GameLogEventTacticalCombatFullAttack"]),
    ("UnitInitiativeLogThread", &["R:RuleInitiativeRoll"]),
    ("HealingLogThread", &["R:RuleHealDamage"]),
    ("RollSkillCheckLogThread", &["R:RuleStatCheck", "R:RuleSkillCheck", "R:RulePartyStatCheck"]),
    ("RulebookAttackWithWeaponLogThread", &["R:RuleAttackWithWeapon"]),
    ("RulebookCastSpellLogThread", &["R:RuleCastSpell"]),
    ("UseAbilityLogThread", &["E:GameLogEventUseAbility"]),
    ("RulebookSavingThrowLogThread", &["R:RuleSavingThrow"]),
    ("HitDiceRestrictionLogThread", &[]),
    // TacticalCombat
    ("TacticalCombatMoraleLogThread", &["E:GameLogEventTacticalCombatMorale"]),
    ("TacticalCombatTurnLogThread", &["E:GameLogEventTacticalCombatNextTurn"]),
    ("TacticalCombatPostponeTurnLogThread", &["E:GameLogEventTacticalCombatPostponeTurn"]),
    ("TacticalCombatSkipTurnLogThread", &["E:GameLogEventTacticalCombatSkipTurn"]),
    ("TacticalCombatResurrectLogThread", &["E:GameLogEventTacticalCombatResurrect"]),
    ("TacticalCombatUnitsDiedThread", &["E:GameLogEventTacticalCombatUnitsDied"]),
    ("WarningNotificationLogThread", &["E:GameLogEventWarningNotification"]),
    ("MessageLogThread", &["E:GameLogEventMessage"]),
    // InGameCombat
    ("AttackLogThread", &["R:RuleAttackRoll"]),
    ("ProjectileHitLogThread", &["E:GameLogEventProjectileHit"]),
    ("UnitLifeStateChangedLogThread", &["E:GameLogEventUnitLifeStateChanged"]),
    ("AttackOfOpportunityLogThread", &["E:GameLogEventAttackOfOpportunity"]),
    ("RulebookCombatManeuverLogThread", &["R:RuleCombatManeuver"]),
    ("RulebookCastingDefensivelyLogThread", &["R:RuleCheckCastingDefensively"]),
    ("RulebookCheckConcentration", &["R:RuleCheckConcentration"]),
    (
        "RulebookSpellResistanceCheckLogThread",
        &["R:RuleSpellResistanceCheck", "E:GameLogEventAbilityImmunity"],
    ),
    ("RulebookDispellMagicLogThread", &["R:RuleDispelMagic"]),
    ("RulebookDealStatDamageLogThread", &["R:RuleDealStatDamage"]),
    ("RulebookHealStatDamageLogThread", &["R:RuleHealStatDamage"]),
    ("RulebookDrainEnergyLogThread", &["R:RuleDrainEnergy"]),
    ("RulebookHealEnergyDrainLogThread", &["R:RuleHealEnergyDrain"]),
    ("UnitInStealthSpottedLogThread", &["E:GameLogEventUnitInStealthSpotted"]),
    ("PickLockLogThread", &["E:GameLogEventPickLock"]),
    ("DisarmTrapLogThread", &["E:GameLogEventDisarmTrap"]),
    ("RulebookCanApplyBuffLogThread", &["R:RuleCanApplyBuff"]),
    ("KineticistGlobalLogThread", &["E:GameLogEventKineticistAbility"]),
    ("SpellTurningLogThread", &["E:GameLogEventSpellTurned"]),
    // LifeEvents
    ("UnitEquipmentLogThread", &["E:GameLogEventUnitEquipment"]),
    ("ItemsCollectionLogThread", &["E:GameLogEventItemsCollection"]),
    (
        "SkinningRollSkillCheckLogThread",
        &["R:RuleStatCheck", "R:RuleSkillCheck", "R:RulePartyStatCheck"],
    ),
    ("LocationLogThread", &["E:GameLogEventLocationRevealed"]),
    ("PartyGainExperienceLogThread", &["E:GameLogEventPartyGainExperience"]),
    ("PartyCombatLogThread", &["E:GameLogEventCombatStateChanged"]),
    ("IdentifyLogThread", &["E:GameLogEventIdentify"]),
    ("RestFinishedLogThread", &["E:GameLogEventRestFinished"]),
    ("UnitGainExperienceLogThread", &["E:GameLogEventUnitGainExperience"]),
    ("KingdomMoraleChangeLogThread", &["E:GameLogEventKingdomMoraleChanged"]),
    ("WarningNotificationLogThread", &["E:GameLogEventWarningNotification"]),
    // Dialog
    ("BarkLogThread", &["E:GameLogEventBark"]),
    ("DialogHistoryLogThread", &["E:GameLogEventDialogHistory"]),
    ("DialogCueLogThread", &["E:GameLogEventDialogCue"]),
    ("DialogLogThread", &["E:GameLogEventDialog"]),
    // DialogAndLife
    ("AlignmentShiftLogThread", &["E:GameLogEventAlignmentShifted"]),
];

/// `m_RootRulesTypes`: the rule types a thread handles.
#[must_use]
pub fn root_rules() -> Vec<&'static str> {
    let mut v: Vec<&str> =
        THREADS.iter().flat_map(|(_, h)| h.iter()).filter_map(|h| h.strip_prefix("R:")).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// A log event's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    /// `GameLogRuleEvent<T>`.
    Rule(&'static str),
    /// A `GameLogEvent` subclass.
    Other(&'static str),
}

impl EventType {
    /// The type's name as the observer writes it.
    #[must_use]
    pub fn name(self) -> String {
        match self {
            Self::Rule(r) => format!("GameLogRuleEvent<{r}>"),
            Self::Other(e) => e.to_owned(),
        }
    }

    fn key(self) -> String {
        match self {
            Self::Rule(r) => format!("R:{r}"),
            Self::Other(e) => format!("E:{e}"),
        }
    }
}

/// `m_EventToThreads[type]`: the threads (indices into [`THREADS`]) an event type goes to,
/// in order.
#[must_use]
pub fn threads_of(ty: EventType) -> Vec<usize> {
    let k = ty.key();
    THREADS
        .iter()
        .enumerate()
        .filter(|(_, (_, h))| h.contains(&k.as_str()))
        .map(|(i, _)| i)
        .collect()
}

/// A roll in `GameLogContext.D20`/`D100`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Roll {
    #[default]
    Null,
    /// A `RuleRollD20` the sim rolled, with its rule's id (`GameLog::rules_seen`'s).
    Known(LogRoll, i64),
    /// A rule's roll the sim keeps only as its result (its history isn't established).
    ResultOnly(i32),
}

/// `GameLogContext.SourceFact`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SourceFact {
    #[default]
    Null,
    /// A fact the sim doesn't name (its `Name` is reported when read): the recording's
    /// reference to it.
    Unknown(serde_json::Value),
}

/// `GameLogContext`'s static fields.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Context {
    pub source_unit: Option<EntityIdx>,
    pub source_fact: SourceFact,
    pub target: Option<EntityIdx>,
    /// `Message`: an override's key.
    pub message: Option<String>,
    pub formula: Option<String>,
    pub text: Option<String>,
    pub text_with_tags: Option<String>,
    pub description: Option<String>,
    pub count: i32,
    pub roll: i32,
    pub d20: Roll,
    pub d100: Roll,
    pub modifier: i32,
    pub dc: i32,
    pub chance_dc: i32,
    /// `ConcealmentStatus`: None 0, Pass 1, Fail 2.
    pub concealment: i32,
    pub rations: i32,
    pub attack_number: i32,
    pub attacks_count: i32,
    pub round_number: i32,
    pub tooltip: bool,
    /// `Tooltip`'s object as the observer writes it (`{"$type": …}`), when the sim sets one.
    pub tooltip_type: Option<&'static str>,
    pub roll_chance: i32,
}

impl Context {
    /// `GameLogContext.Clear` (the last scope's release): `SourceFact`, `Formula`, `Rations`,
    /// `RoundNumber` and `RollChance` stay.
    fn clear(&mut self) {
        let keep = Self {
            source_fact: std::mem::take(&mut self.source_fact),
            formula: self.formula.take(),
            rations: self.rations,
            round_number: self.round_number,
            roll_chance: self.roll_chance,
            ..Self::default()
        };
        *self = keep;
    }
}

/// What an event carries for its threads.
#[derive(Debug, Clone)]
pub enum Payload {
    /// A rule the ported threads don't read (or no thread handles).
    None,
    AttackRoll(Box<AttackRollLog>),
    AttackWithWeapon {
        initiator: EntityIdx,
        /// `Reason` was set before the rule triggered (its context's blueprint isn't modelled).
        explicit_reason: bool,
    },
    DealDamage(Box<DealDamageLog>),
    Initiative {
        initiator: EntityIdx,
        result: i32,
        d20: Roll,
        modifier: i32,
    },
    /// A `RuleRollD20`.
    D20(Roll),
    CanApplyBuff {
        initiator: EntityIdx,
        blueprint: wr_content::Guid,
        immunity: bool,
    },
    AttackOfOpportunity {
        attacker: EntityIdx,
        target: EntityIdx,
    },
    LifeState {
        unit: EntityIdx,
        /// `PrevLifeState` (not read by its thread).
        prev: i32,
    },
    CombatState {
        in_combat: bool,
    },
    PartyExperience {
        experience: i32,
    },
    /// `IKnowledgeHandler` (no thread handles it).
    Knowledge,
    /// `IItemsCollectionHandler.HandleItemsAdded`/`HandleItemsRemoved`
    /// (`GameLogEventItemsCollection`): the collection (its identity), the item, the count.
    ItemsCollection {
        removed: bool,
        collection: usize,
        item: EntityIdx,
        count: i32,
    },
    /// A recorded input: the bark's speaker and text.
    Bark {
        unit: Option<EntityIdx>,
        text: String,
    },
    /// A recorded input: the projectile's attack roll (its event).
    ProjectileHit {
        attack: Option<usize>,
    },
    /// A `RuleCastSpell` (`BaseUseAbilityLogThread.HandleUseAbility(rule.Spell, rule)`): its
    /// initiator and spell, whether the spell is a spellbook's, its failures.
    CastSpell {
        initiator: EntityIdx,
        spell: wr_content::Guid,
        spellbook: bool,
        failed: bool,
    },
    /// A `RuleSpellResistanceCheck`: `HasResistanceRoll`, `TargetIsImmune`.
    SpellResistance {
        has_roll: bool,
        immune: bool,
    },
}

/// What `AttackLogMessage.GetData` and `AttackLogThread` read of a `RuleAttackRoll`.
#[derive(Debug, Clone)]
pub struct AttackRollLog {
    pub seq: u64,
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub weapon_item: EntityIdx,
    pub weapon_bp: wr_content::Guid,
    pub d20: i32,
    /// `D20` as a roll (its first d20).
    pub d20_roll: Roll,
    pub attack_bonus: i32,
    pub target_ac: i32,
    /// `AttackResult`.
    pub result: i32,
    pub is_hit: bool,
    pub auto_hit: bool,
    pub auto_miss: bool,
    pub is_critical_confirmed: bool,
    pub is_sneak_attack: bool,
    /// `SuspendCombatLog` as created (`RuleAttackWithWeapon`: the weapon is ranged).
    pub suspend: bool,
    /// `IsCriticalRoll`, `TargetCriticalAC` and `CriticalConfirmationD20` (the second d20 rolled
    /// inside it; an automatic confirmation's is an untriggered rule, id 0, of 20).
    pub is_critical_roll: bool,
    pub target_critical_ac: i32,
    pub confirmation: Roll,
    /// `AttackIndexForLog ?? RuleAttackWithWeapon?.AttackNumber ?? -1` and the count alike
    /// (`None`: made by a `RuleAttackWithWeapon` the log didn't see open).
    pub numbers: Option<(i32, i32)>,
    /// `AttackBonusRule` and `ACRule` as the tooltip reads them (`None`: not triggered).
    pub rules: Option<Box<(tips::AttackBonusRule, tips::AcRule)>>,
}

/// What `DamageLogMessage.GetData` reads of a `RuleDealDamage`.
#[derive(Debug, Clone)]
pub struct DealDamageLog {
    pub initiator: EntityIdx,
    pub target: EntityIdx,
    pub result: i32,
    pub damage_before_difficulty: i32,
    pub damage_without_reduction: i32,
    /// `ResultList`'s sources' `Half` (any).
    pub any_half: bool,
    /// A weapon attack's damage (`Reason.Fact` is null, nothing is redirected, no saving throw
    /// halves it); other damage isn't ported.
    pub weapon_attack: bool,
    /// What the tooltip reads.
    pub tip: tips::DamageRule,
}

/// A log event.
#[derive(Debug, Clone)]
pub struct Event {
    pub ty: EventType,
    /// The rule's sequence number (`RuleBase.seq`).
    pub rule_seq: Option<u64>,
    /// The rule's place among the rules triggered since the log was turned on, from 1 (the
    /// oracle's rule ids are allocated so in a window).
    pub rule_id: Option<i64>,
    /// The rule's initiator and target (as the observer's rule records name them).
    pub units: [Option<EntityIdx>; 2],
    /// `m_IsEnabled`.
    pub enabled: bool,
    /// `OverrideMessage`'s key.
    pub override_message: Option<String>,
    pub payload: Payload,
    /// The frame it was queued in (`None` while open).
    pub queued: Option<i64>,
    /// A `RuleAttackWithWeapon`'s `AttackNumber` and `AttacksCount`.
    pub attack_numbers: Option<(i32, i32)>,
}

/// A message's tooltip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tip {
    None,
    /// A `TooltipTemplateSimple` (or subclass): its class, header and description.
    Simple {
        cls: &'static str,
        header: Option<String>,
        desc: Option<String>,
    },
    /// One the sim doesn't make (its class): not compared.
    NotPorted(&'static str),
}

/// A message a thread adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Msg {
    pub frame: i64,
    /// The thread ([`THREADS`] index).
    pub thread: usize,
    /// The event handled ([`GameLog::events`] index).
    pub src: Option<usize>,
    pub text: String,
    /// `ColorUtility.ToHtmlStringRGBA(Color)` of the message's `Color32`.
    pub color: String,
    pub icon: i32,
    pub tip: Tip,
    /// `GameLogContext`'s scope count and fields when it is added (`None`: a field the sim can't
    /// write as the observer does).
    pub scope: u32,
    pub ctx: Option<serde_json::Value>,
}

/// What the sim's log did, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// `AddReadyEvent` (an event index), with the experience and damage mutations the sim had
    /// written by then (its place among the gameplay records).
    Queued {
        frame: i64,
        event: usize,
        muts: usize,
    },
    /// `Tick` with something queued or open: the queue (event index, `IsEnabled`), the open
    /// stack's size.
    Tick {
        frame: i64,
        queue: Vec<(usize, bool)>,
        stack: usize,
    },
    Msg(Msg),
}

/// `GameLogController` with `GameLogContext`.
#[derive(Debug, Default)]
pub struct GameLog {
    /// Modelled in this run (a recording with WrathOracle's `combat_log`).
    pub on: bool,
    /// The frame (the oracle's tick) the run is in.
    pub frame: i64,
    pub strings: Option<std::rc::Rc<strings::LogStrings>>,
    pub events: Vec<Event>,
    /// `m_ReadyEvents`.
    pub ready: Vec<usize>,
    /// `m_EventsStack`.
    pub stack: Vec<usize>,
    pub ctx: Context,
    /// `GameLogContext`'s scope count.
    pub scope: u32,
    /// Attack rolls' log state by rule sequence number: `AttackLogEntry` (a message made and
    /// kept while `SuspendCombatLog`), `SuspendCombatLog`, `AddedToCombatLog`.
    pub attack_state: HashMap<u64, AttackState>,
    /// Rules triggered since the log was turned on (every one, events or not).
    pub rules_seen: i64,
    /// The recording's thread ordinals (the observer's `ti`), as [`THREADS`] indices.
    pub thread_ordinals: HashMap<i64, usize>,
    /// The d20 rolls finished inside each open event, in order.
    pub d20s: HashMap<usize, Vec<Roll>>,
    pub out: Vec<Out>,
}

/// A `RuleAttackRoll`'s log members.
#[derive(Debug, Clone, Default)]
pub struct AttackState {
    pub entry: Option<Msg0>,
    pub suspend: bool,
    pub added: bool,
}

impl GameLog {
    fn retain(&mut self) {
        self.scope += 1;
    }

    fn release(&mut self) {
        self.scope = self.scope.saturating_sub(1);
        if self.scope == 0 {
            self.ctx.clear();
        }
    }
}

fn unsupported(w: &mut World, what: impl std::fmt::Display) {
    w.coverage.unsupported(format!("combat log: {what}"));
}

// ---- the rulebook hooks ----

/// `IRulebookEventAboutToTriggerHook.OnBeforeEventAboutToTrigger`: a root rule, or any rule
/// inside an open one, becomes an event and is pushed.
pub fn about_to_trigger(w: &mut World, rule: &mut dyn RuleEvent) {
    if !w.game_log.on {
        return;
    }
    w.game_log.rules_seen += 1;
    let class = rule.chain().first().copied().unwrap_or("?");
    let root = root_rules().contains(&class);
    if w.game_log.stack.is_empty() && !root {
        return;
    }
    let (seq, units) = (rule.base().seq, [Some(rule.initiator()), rule.target()]);
    // A `RuleAttackWithWeapon`'s `AttackNumber` and `AttacksCount`, set as it was made (its roll's
    // log reads them while it's still open).
    let attack_numbers = rule
        .as_any()
        .downcast_ref::<crate::combat::attack::RuleAttackWithWeapon>()
        .map(|r| (r.attack_number, r.attacks_count));
    let ev = Event {
        ty: EventType::Rule(class),
        rule_seq: Some(seq),
        rule_id: Some(w.game_log.rules_seen),
        units,
        enabled: true,
        override_message: None,
        payload: Payload::None,
        queued: None,
        attack_numbers,
    };
    w.game_log.events.push(ev);
    let i = w.game_log.events.len() - 1;
    w.game_log.stack.push(i);
}

/// `IRulebookEventDidTriggerHook.OnAfterEventDidTrigger`: the open event of this rule is
/// popped and queued, with what its threads read of the rule.
pub fn did_trigger(w: &mut World, rule: &mut dyn RuleEvent) {
    if !w.game_log.on {
        return;
    }
    let seq = rule.base().seq;
    let Some(&top) = w.game_log.stack.last() else { return };
    if w.game_log.events[top].rule_seq != Some(seq) {
        return;
    }
    w.game_log.stack.pop();
    let payload = payload_of(w, rule, top);
    // A d20 roll is its parent's `D20` (the first) or another of its rolls.
    if let Payload::D20(r) = &payload
        && let Some(&parent) = w.game_log.stack.last()
    {
        w.game_log.d20s.entry(parent).or_default().push(r.clone());
    }
    w.game_log.events[top].payload = payload;
    add_ready(w, top);
}

/// The payload of a finished rule.
fn payload_of(w: &mut World, rule: &mut dyn RuleEvent, ev: usize) -> Payload {
    use crate::combat::attack::{RuleAttackRoll, RuleAttackWithWeapon, RuleDealDamage};
    let explicit_reason =
        rule.base().reason.as_deref().is_some_and(|r| r != "self" && !r.starts_with("previous:"));
    let any = rule.as_any();
    if let Some(r) = any.downcast_ref::<RuleAttackRoll>() {
        let suspend = r.reason_full_attack.is_some() && r.weapon.is_ranged();
        // Its `RuleAttackWithWeapon` (its reason's rule) is the open event below it.
        let numbers = if r.reason_full_attack.is_some() {
            w.game_log.stack.last().and_then(|p| w.game_log.events[*p].attack_numbers)
        } else {
            Some((-1, -1))
        };
        return Payload::AttackRoll(Box::new(AttackRollLog {
            seq: r.base.seq,
            initiator: r.initiator,
            target: r.target,
            weapon_item: r.weapon.item,
            weapon_bp: r.weapon.bp.guid,
            d20: r.d20,
            // An auto miss rolls no `D20` (null).
            d20_roll: if r.auto_miss { Roll::Null } else { first_d20(w, ev, r.d20) },
            attack_bonus: r.attack_bonus,
            target_ac: r.target_ac,
            result: r.result,
            is_hit: r.is_hit(),
            auto_hit: r.auto_hit,
            auto_miss: r.auto_miss,
            is_critical_confirmed: r.is_critical_confirmed,
            is_sneak_attack: r.is_sneak_attack,
            suspend,
            numbers,
            rules: r.log_rules.clone(),
            is_critical_roll: r.is_critical_roll,
            target_critical_ac: r.target_critical_ac,
            confirmation: if !r.is_critical_roll {
                Roll::Null
            } else if r.auto_critical_confirmation {
                // `new RuleRollD20(Initiator, 20)`, never triggered: no history.
                Roll::Known(
                    LogRoll {
                        result: 20,
                        history: None,
                        replaced_one: false,
                        replace_one_with_max: false,
                        rerolls: None,
                    },
                    0,
                )
            } else {
                match w.game_log.d20s.get(&ev).and_then(|v| v.get(1)) {
                    Some(Roll::Known(x, id)) if x.result == r.critical_confirmation_d20 => {
                        Roll::Known(x.clone(), *id)
                    }
                    _ => Roll::ResultOnly(r.critical_confirmation_d20),
                }
            },
        }));
    }
    if let Some(r) = any.downcast_ref::<RuleAttackWithWeapon>() {
        return Payload::AttackWithWeapon { initiator: r.initiator, explicit_reason };
    }
    if let Some(r) = any.downcast_ref::<RuleDealDamage>() {
        return Payload::DealDamage(Box::new(DealDamageLog {
            initiator: r.initiator,
            target: r.target,
            result: r.result,
            damage_before_difficulty: r.damage_before_difficulty,
            damage_without_reduction: r.damage_without_reduction,
            any_half: r.bundle.iter().any(|d| d.half),
            weapon_attack: r.link.weapon_attack && !explicit_reason,
            tip: tips::DamageRule {
                results: r.result_list.clone(),
                // `DamageBundle.WeaponDamage` (the bundle's first damage when it has a weapon) is
                // the first value's source.
                first_is_weapon_damage: r.weapon_item.is_some() && !r.result_list.is_empty(),
                weapon: r.weapon_item,
                difficulty_modifier: r.difficulty_modifier,
                damage_before_difficulty: r.damage_before_difficulty,
                result: r.result,
            },
        }));
    }
    if let Some(r) = any.downcast_ref::<crate::combat::initiative::RuleInitiativeRoll>() {
        return Payload::Initiative {
            initiator: r.initiator,
            result: r.result(),
            d20: first_d20(w, ev, r.d20),
            modifier: r.modifier,
        };
    }
    if let Some(r) = any.downcast_ref::<crate::combat::spells::RuleCastSpell>() {
        let a = w.combat.spells.context(r.ctx).map(|c| c.ability.blueprint());
        if let Some(spell) = a {
            return Payload::CastSpell {
                initiator: r.initiator,
                spell,
                spellbook: true,
                failed: r.spell_failure_chance > 0,
            };
        }
    }
    if let Some(r) = any.downcast_ref::<crate::combat::spells::RuleSpellResistanceCheck>() {
        return Payload::SpellResistance {
            has_roll: r.spell_resistance > 0,
            immune: r.target_is_immune,
        };
    }
    if let Some(r) = any.downcast_ref::<crate::combat::rules::RuleCanApplyBuff>() {
        return Payload::CanApplyBuff {
            initiator: r.initiator,
            blueprint: r.blueprint.guid,
            immunity: r.immunity,
        };
    }
    if let Some(r) = any.downcast_ref::<crate::rules::checks::RuleRollDice>()
        && r.d20
    {
        // `Rerolls` (each reroll's source) isn't kept by the sim: a roll with rerolls is known by
        // its result only.
        let id = w.game_log.events[ev].rule_id.unwrap_or(0);
        return Payload::D20(if r.reroll_amount == 0 {
            Roll::Known(
                LogRoll {
                    result: r.result(),
                    history: r.roll_history.clone(),
                    replaced_one: r.replaced_one(),
                    replace_one_with_max: r.replace_one_with_max,
                    rerolls: None,
                },
                id,
            )
        } else {
            Roll::ResultOnly(r.result())
        });
    }
    Payload::None
}

/// A rule's `D20`: the first d20 rolled inside it (its event's child), when its result is the
/// rule's; else known by its result only.
fn first_d20(w: &World, ev: usize, result: i32) -> Roll {
    match w.game_log.d20s.get(&ev).and_then(|v| v.first()) {
        Some(Roll::Known(r, id)) if r.result == result => Roll::Known(r.clone(), *id),
        _ => Roll::ResultOnly(result),
    }
}

/// `GameLogController.AddReadyEvent`: `IsEnabled` (a rule event's reads its rule's
/// `DisableBattleLog`) and no `GameLogDisabled` scope; `GameLogContext.Message` as the
/// override; queued. The scope it opens clears the context when it is the outermost.
fn add_ready(w: &mut World, e: usize) {
    let muts = w.events.records.iter().filter(|r| r.k == "mut" && counted_mut(&r.fields)).count();
    let gl = &mut w.game_log;
    gl.retain();
    // `GameLogDisabled` and the rules' and units' `DisableBattleLog` aren't set where the sim runs
    // (`docs/poc5/combat-log.md`); the recording's `en` checks it.
    gl.events[e].enabled = true;
    if let Some(m) = gl.ctx.message.clone() {
        gl.events[e].override_message = Some(m);
    }
    gl.events[e].queued = Some(gl.frame);
    gl.ready.push(e);
    gl.out.push(Out::Queued { frame: gl.frame, event: e, muts });
    gl.release();
}

/// A mutation whose count places a non-rule event among the gameplay records: experience or
/// damage (written alike by the sim and the recording).
#[must_use]
pub fn counted_mut(fields: &serde_json::Map<String, serde_json::Value>) -> bool {
    matches!(fields.get("what").and_then(serde_json::Value::as_str), Some("xp" | "damage"))
}

/// A non-rule event raised through the event bus (its `GameEventsHandler.AddEvent`).
pub fn add_event(w: &mut World, ty: &'static str, payload: Payload, units: [Option<EntityIdx>; 2]) {
    if !w.game_log.on {
        return;
    }
    w.game_log.events.push(Event {
        ty: EventType::Other(ty),
        rule_seq: None,
        rule_id: None,
        attack_numbers: None,
        units,
        enabled: true,
        override_message: None,
        payload,
        queued: None,
    });
    let i = w.game_log.events.len() - 1;
    add_ready(w, i);
}

/// `IAttackOfOpportunityHandler.HandleAttackOfOpportunity` (`UnitCombatState.AttackOfOpportunity`).
pub fn attack_of_opportunity(w: &mut World, attacker: EntityIdx, target: EntityIdx) {
    add_event(
        w,
        "GameLogEventAttackOfOpportunity",
        Payload::AttackOfOpportunity { attacker, target },
        [Some(attacker), Some(target)],
    );
}

/// `IUnitLifeStateChanged.HandleUnitLifeStateChanged` (`UnitLifeController`).
pub fn life_state_changed(w: &mut World, unit: EntityIdx, prev: i32) {
    add_event(
        w,
        "GameLogEventUnitLifeStateChanged",
        Payload::LifeState { unit, prev },
        [Some(unit), None],
    );
}

/// `IPartyCombatHandler.HandlePartyCombatStateChanged`: two handlers subscribe, in
/// `GameLogController.GameEventsHandlerTypes`' order (the recording's queue checks it).
pub fn party_combat_changed(w: &mut World, in_combat: bool) {
    for ty in PARTY_COMBAT_HANDLERS {
        add_event(w, ty, Payload::CombatState { in_combat }, [None, None]);
    }
}

/// `IPartyGainExperienceHandler.HandlePartyGainExperience` (`Player.GainPartyExperience`).
pub fn party_gain_experience(w: &mut World, experience: i32) {
    add_event(
        w,
        "GameLogEventPartyGainExperience",
        Payload::PartyExperience { experience },
        [None, None],
    );
}

/// `IKnowledgeHandler.HandleKnowledgeUpdated` (`InspectUnitsManager.TryMakeKnowledgeCheck`).
pub fn knowledge_updated(w: &mut World) {
    add_event(w, "GameLogEventKnowledge", Payload::Knowledge, [None, None]);
}

/// `IItemsCollectionHandler.HandleItemsAdded`/`HandleItemsRemoved` (`ItemsCollection.OnItemAdded`,
/// `OnItemsRemoved`, unless the events are suppressed): a `GameLogEventItemsCollection`.
pub fn items_collection(
    w: &mut World,
    removed: bool,
    collection: usize,
    item: EntityIdx,
    count: i32,
) {
    add_event(
        w,
        "GameLogEventItemsCollection",
        Payload::ItemsCollection { removed, collection, item, count },
        [None, None],
    );
}

/// The `GameEventsHandler`s of `IPartyCombatHandler`, in subscription order.
pub const PARTY_COMBAT_HANDLERS: [&str; 2] =
    ["GameLogEventCombatStateChanged", "GameLogEventPartyCombatState"];

// ---- the tick ----

/// `IController.Tick`: up to [`MAX_LOGS_IN_FRAME`] queued events to their threads.
pub fn tick(w: &mut World) {
    if !w.game_log.on {
        return;
    }
    let frame = w.game_log.frame;
    let open = w.game_log.stack.len();
    if open > 0 {
        unsupported(w, "GameLogController.Tick: the events stack isn't empty");
        w.game_log.stack.clear();
    }
    w.game_log.retain();
    let list = w.game_log.ready.clone();
    if !list.is_empty() || open > 0 {
        let queue = list.iter().map(|&e| (e, is_enabled(w, e))).collect();
        w.game_log.out.push(Out::Tick { frame, queue, stack: open });
    }
    for &e in list.iter().take(MAX_LOGS_IN_FRAME) {
        if !is_enabled(w, e) {
            continue;
        }
        let ty = w.game_log.events[e].ty;
        // `GameLogEvent<TSelf>.Invoke` sets `GameLogContext.Message` before each thread.
        for th in threads_of(ty) {
            if matches!(ty, EventType::Other(_)) {
                w.game_log.ctx.message = w.game_log.events[e].override_message.clone();
            }
            handle(w, th, e);
        }
    }
    w.game_log.ready = list.iter().skip(MAX_LOGS_IN_FRAME).copied().collect();
    w.game_log.release();
}

/// `IsEnabled` as the tick reads it.
fn is_enabled(w: &World, e: usize) -> bool {
    w.game_log.events[e].enabled
}

/// A thread handling an event; an exception the game would throw stops the handler (the
/// tick logs it), state the sim doesn't model is reported.
fn handle(w: &mut World, th: usize, e: usize) {
    let name = THREADS[th].0;
    let r = match (name, w.game_log.events[e].payload.clone()) {
        ("AttackLogThread", Payload::AttackRoll(a)) => attack_thread(w, th, e, &a),
        ("DamageDealtLogThread", Payload::DealDamage(d)) => damage_thread(w, th, e, &d),
        ("UnitInitiativeLogThread", Payload::Initiative { initiator, result, d20, modifier }) => {
            initiative_thread(w, th, e, initiator, result, d20, modifier)
        }
        (
            "RulebookAttackWithWeaponLogThread",
            Payload::AttackWithWeapon { explicit_reason, .. },
        ) => {
            if explicit_reason {
                Err(TextError::Unsupported(
                    "RuleAttackWithWeapon with an explicit reason (its context's blueprint)".into(),
                ))
            } else {
                // `Reason` is the rule itself: no context, no message.
                Ok(())
            }
        }
        ("RulebookCanApplyBuffLogThread", Payload::CanApplyBuff { immunity, .. }) => {
            if immunity {
                Err(TextError::Unsupported("RuleCanApplyBuff's immunity message".into()))
            } else {
                // Not immune (`StoryModeImmunity` is reported where the sim would set it): no message.
                Ok(())
            }
        }
        ("UnitLifeStateChangedLogThread", Payload::LifeState { unit, .. }) => {
            death_thread(w, th, e, unit)
        }
        ("PartyCombatLogThread", Payload::CombatState { in_combat }) => {
            party_combat_thread(w, th, e, in_combat)
        }
        ("AttackOfOpportunityLogThread", Payload::AttackOfOpportunity { attacker, target }) => {
            aoo_thread(w, th, e, attacker, target)
        }
        ("BarkLogThread", Payload::Bark { unit, text }) => bark_thread(w, th, e, unit, text),
        ("PartyGainExperienceLogThread", Payload::PartyExperience { experience }) => {
            party_xp_thread(w, th, e, experience)
        }
        (
            "ItemsCollectionLogThread",
            Payload::ItemsCollection { removed, collection, item, .. },
        ) => items_collection_thread(w, removed, collection, item),
        ("ProjectileHitLogThread", Payload::ProjectileHit { attack }) => {
            projectile_thread(w, th, e, attack)
        }
        (
            "RulebookCastSpellLogThread",
            Payload::CastSpell { initiator, spell, spellbook, failed },
        ) => cast_spell_thread(w, th, e, initiator, spell, spellbook, failed),
        // `RulebookSpellResistanceCheckLogThread`: no message without a resistance roll or an
        // immunity.
        (
            "RulebookSpellResistanceCheckLogThread",
            Payload::SpellResistance { has_roll, immune },
        ) => {
            if has_roll || immune {
                Err(TextError::Unsupported("a spell resistance check's message".into()))
            } else {
                Ok(())
            }
        }
        (n, p) => Err(TextError::Unsupported(format!(
            "{n} handling {} ({}) isn't ported",
            w.game_log.events[e].ty.name(),
            match p {
                Payload::None => "the rule's fields aren't captured",
                _ => "unexpected payload",
            }
        ))),
    };
    match r {
        Ok(()) => {}
        // The game logs the exception and goes on with the next thread.
        Err(TextError::Throws(_)) => {}
        Err(err) => unsupported(w, err),
    }
}

fn add_message(w: &mut World, th: usize, e: Option<usize>, m: Msg0) {
    let frame = w.game_log.frame;
    let (scope, ctx) = (w.game_log.scope, context_json(w));
    w.game_log.out.push(Out::Msg(Msg {
        frame,
        thread: th,
        src: e,
        text: m.text,
        color: m.color,
        icon: m.icon,
        tip: m.tip,
        scope,
        ctx,
    }));
}

/// `GameLogContext`'s fields as the observer writes them (`LogObs.WriteContext`); `None` for a
/// field the sim holds without what the observer writes (a roll known by its result only).
fn context_json(w: &World) -> Option<serde_json::Value> {
    use serde_json::{Value as J, json};
    let c = &w.game_log.ctx;
    let unit =
        |u: Option<EntityIdx>| u.map_or(J::Null, |u| json!({"$unit": w.entity(u).unique_id}));
    let roll = |r: &Roll| match r {
        Roll::Null => Some(J::Null),
        Roll::Known(_, id) => Some(json!({"$rule": id, "class": "RuleRollD20"})),
        Roll::ResultOnly(_) => None,
    };
    let fact = match &c.source_fact {
        SourceFact::Null => J::Null,
        SourceFact::Unknown(v) => v.clone(),
    };
    Some(json!({
        "SourceUnit": unit(c.source_unit),
        "SourceFact": fact,
        "Target": unit(c.target),
        "Message": c.message.as_ref().map_or(J::Null, |k| json!({"$key": k})),
        "Formula": c.formula,
        "Text": c.text,
        "TextWithTags": c.text_with_tags,
        "Description": c.description,
        "Count": c.count,
        "Roll": c.roll,
        "D20": roll(&c.d20)?,
        "D100": roll(&c.d100)?,
        "Modifier": c.modifier,
        "DC": c.dc,
        "ChanceDC": c.chance_dc,
        "ConcealmentStatus": c.concealment,
        "Rations": c.rations,
        "AttackNumber": c.attack_number,
        "AttacksCount": c.attacks_count,
        "RoundNumber": c.round_number,
        "Tooltip": c.tooltip_type.map_or(J::Null, |t| json!({"$type": t})),
        "RollChance": c.roll_chance,
    }))
}

/// A `CombatLogMessage` before a thread adds it (or kept on its attack roll).
#[derive(Debug, Clone)]
pub struct Msg0 {
    pub text: String,
    pub color: String,
    pub icon: i32,
    pub tip: Tip,
}

impl Msg0 {
    /// `new CombatLogMessage(message, null, hasTooltip: false)`: the text, colour and icon;
    /// no tooltip (`IsControllerMouse`: the oracle's input is the mouse).
    fn copy_without_tip(self) -> Self {
        Self { tip: Tip::None, ..self }
    }
}

type R = Result<(), TextError>;

fn strings(w: &World) -> Result<std::rc::Rc<strings::LogStrings>, TextError> {
    w.game_log.strings.clone().ok_or_else(|| TextError::Unsupported("no log strings loaded".into()))
}

fn rgba(c: Color32) -> String {
    format!("{:02X}{:02X}{:02X}{:02X}", c.r, c.g, c.b, c.a)
}

/// `GameLogMessage.GetColor` / `AttackLogMessage.GetColor`: the message's colour, or
/// `GameLogStrings.DefaultColor` when every channel is 0.
fn color_of(w: &World, path: &str) -> Result<String, TextError> {
    let s = strings(w)?;
    let c = s.color(&format!("{path}/Color")).map_err(TextError::Unsupported)?;
    let c = if c.r == 0 && c.g == 0 && c.b == 0 && c.a == 0 {
        s.color("GameLog/DefaultColor").map_err(TextError::Unsupported)?
    } else {
        c
    };
    Ok(rgba(c))
}

/// A `LocalizedString` of the log strings, converted with the current context.
fn text_of(w: &World, path: &str) -> Result<String, TextError> {
    let s = strings(w)?;
    let key = s.key(path).map_err(TextError::Unsupported)?.to_owned();
    localized(w, &key)
}

/// A `LocalizedString` with actual key `key`, converted inside the log's scope (`mf` reads its
/// source unit first).
pub(crate) fn localized(w: &World, key: &str) -> Result<String, TextError> {
    if key.is_empty() {
        return Ok(String::new());
    }
    let pack = &w
        .text
        .as_deref()
        .ok_or_else(|| TextError::Unsupported("no localization pack loaded".into()))?
        .pack;
    wr_text::display(Some(pack), key, &mut LogEnv { w })
}

/// `GameLogContext.GetIcon`.
fn icon(w: &World) -> i32 {
    let c = &w.game_log.ctx;
    let Some(t) = c.target else { return -1 };
    if crate::combat::units::is_player_faction(w, t) {
        return 0;
    }
    if c.source_unit.is_some_and(|s| crate::combat::units::is_player_faction(w, s)) {
        return 1;
    }
    2
}

/// `GameLogMessage.GetData` for the message at `path` (`GameLog/InitiativeRoll`…).
fn game_log_message(w: &mut World, path: &str) -> Result<Option<Msg0>, TextError> {
    w.game_log.retain();
    let r = game_log_message_in_scope(w, path);
    w.game_log.release();
    r
}

fn game_log_message_in_scope(w: &mut World, path: &str) -> Result<Option<Msg0>, TextError> {
    let text = match w.game_log.ctx.message.clone() {
        Some(k) => localized(w, &k)?,
        None => text_of(w, &format!("{path}/Message"))?,
    };
    // The tooltip's text (`TooltipTemplateCombatLogMessage(text, Tooltip)`, after the
    // concealment's); one the sim can't make is reported.
    let tooltip = |w: &World| -> Result<String, TextError> {
        let t = text_of(w, &format!("{path}/Tooltip"))?;
        Ok(match (w.game_log.ctx.tooltip, w.game_log.ctx.concealment) {
            (true, _) => t,
            (false, 2) => text_of(w, "GameLog/ConcealmentFail")?,
            (false, 1) => format!("{}\n{t}", text_of(w, "GameLog/ConcealmentPass")?),
            _ => t,
        })
    };
    let tip = Tip::Simple {
        cls: "TooltipTemplateCombatLogMessage",
        header: Some(text.clone()),
        desc: Some(tooltip(w)?),
    };
    if text.is_empty() {
        return Ok(None);
    }
    let s = strings(w)?;
    let own = s.get(&format!("{path}/Icon")).and_then(serde_json::Value::as_i64).unwrap_or(-1);
    let icon = if own != -1 { i32::try_from(own).unwrap_or(-1) } else { icon(w) };
    let color = color_of(w, path)?;
    Ok(Some(Msg0 { text, color, icon, tip }))
}

// ---- the threads ----

fn is_in_game(w: &World, u: EntityIdx) -> bool {
    w.entity(u).is_in_game
}

/// `AttackLogThread.HandleEvent(RuleAttackRoll)` (and `ProjectileHitLogThread`'s part).
fn attack_thread(w: &mut World, th: usize, e: usize, a: &AttackRollLog) -> R {
    // `IsFake` (no fake roll is fired by the sim), the units in game, `AutoHit`.
    if !is_in_game(w, a.initiator) || !is_in_game(w, a.target) || a.auto_hit {
        return Ok(());
    }
    let st = w
        .game_log
        .attack_state
        .entry(a.seq)
        .or_insert_with(|| AttackState { suspend: a.suspend, ..AttackState::default() });
    let kept = st.entry.clone();
    let msg = match kept {
        Some(m) => Some(m),
        None => attack_data(w, a)?,
    };
    // `GetParryData`: the sim has no parry (`Parry` null).
    let st = w.game_log.attack_state.entry(a.seq).or_default();
    if st.suspend {
        st.entry = msg;
        return Ok(());
    }
    st.added = true;
    w.game_log.ctx.source_unit = Some(a.initiator);
    w.game_log.ctx.target = Some(a.target);
    // `Result != AttackResult.Parried` (9).
    if let Some(m) = msg
        && a.result != 9
    {
        add_message(
            w,
            th,
            Some(e),
            Msg0 { text: m.text, color: m.color, icon: m.icon, tip: m.tip },
        );
    }
    Ok(())
}

/// `AttackLogMessage.GetData(RuleAttackRoll)`: the message's text; its tooltip (the attack's
/// breakdowns) isn't made.
fn attack_data(w: &mut World, a: &AttackRollLog) -> Result<Option<Msg0>, TextError> {
    w.game_log.retain();
    let r = attack_data_in_scope(w, a);
    w.game_log.release();
    r
}

fn attack_data_in_scope(w: &mut World, a: &AttackRollLog) -> Result<Option<Msg0>, TextError> {
    if !is_in_game(w, a.initiator) || !is_in_game(w, a.target) {
        return Ok(None);
    }
    {
        let c = &mut w.game_log.ctx;
        c.source_unit = Some(a.initiator);
        c.target = Some(a.target);
        c.roll = crate::csharp::arith::add(a.d20, a.attack_bonus);
        c.d20 = a.d20_roll.clone();
        c.modifier = a.attack_bonus;
        c.dc = a.target_ac;
    }
    let Some((number, count)) = a.numbers else {
        return Err(TextError::Unsupported(
            "an attack roll's RuleAttackWithWeapon (its attack number)".into(),
        ));
    };
    w.game_log.ctx.attack_number = number;
    w.game_log.ctx.attacks_count = count;
    // `SetConcealment`: the sim's concealment checks don't roll (a concealment is reported);
    // `SetMissChance`: the sim's rolls have no miss chance.
    // `AttackNumber`/`AttacksCount` (the tooltip's), the concealment and miss chance (the
    // tooltip's) aren't read by the message.
    let weapon_name = weapon_text(w, a)?;
    w.game_log.ctx.text = Some(weapon_name);
    let mut s = text_of(w, "GameLog/Attack/Message")?;
    let suffix = |w: &World, k: &str| text_of(w, &format!("GameLog/Attack/{k}"));
    if a.is_critical_confirmed {
        s.push(' ');
        s.push_str(&suffix(w, "CritSuffix")?);
    } else if a.is_hit {
        s.push(' ');
        s.push_str(&suffix(w, "HitSuffix")?);
    } else if a.auto_miss {
        s.push(' ');
        s.push_str(&suffix(w, "AutoMissSuffix")?);
    } else if a.d20 == 1 {
        s.push(' ');
        let v = suffix(w, "CriticalMissSuffix")?;
        if v.trim().is_empty() {
            s.push_str(&suffix(w, "MissSuffix")?);
        } else {
            s.push_str(&v);
        }
    } else {
        s.push(' ');
        s.push_str(&suffix(w, "MissSuffix")?);
        // `HitMirrorImageIndex > 0`: the sim has no mirror images.
    }
    if a.is_hit && a.is_sneak_attack {
        s.push(' ');
        s.push_str(&suffix(w, "SneakSuffix")?);
    }
    let tooltip = attack_tooltip(w, a)?;
    let icon = icon(w);
    let color = color_of(w, "GameLog/Attack")?;
    // An empty tooltip (an auto miss or hit: no breakdown) makes no template: `CombatLogMessage`
    // makes `TooltipTemplateSimple(null, message)`.
    let tip = if tooltip.is_empty() {
        Tip::Simple { cls: "TooltipTemplateSimple", header: None, desc: Some(s.clone()) }
    } else {
        Tip::Simple {
            cls: "TooltipTemplateCombatLogMessage",
            header: Some(s.clone()),
            desc: Some(tooltip),
        }
    };
    Ok(Some(Msg0 { text: s, color, icon, tip }))
}

/// `AttackLogMessage.GetData`'s tooltip, with the context changes it makes as it goes (a
/// critical roll's confirmation: `Roll`, `D20`, `Modifier`, `DC`, then `DC` back to the
/// target's AC). The sim's rolls have no mirror images (`HitMirrorImageIndex` 0), parry or
/// fortification roll (reported by the rule); `CriticalConfirmationBonus` is 0.
fn attack_tooltip(w: &mut World, a: &AttackRollLog) -> Result<String, TextError> {
    let mut sb = String::new();
    if a.auto_miss || a.auto_hit {
        return Ok(sb);
    }
    let t = |w: &World, k: &str| text_of(w, &format!("GameLog/Attack/{k}"));
    // `ConcealmentStatus`: Pass 1, Fail 2 (the context's, which can outlive one message).
    let concealment = w.game_log.ctx.concealment;
    if concealment == 2 {
        sb.push_str(&t(w, "TooltipConcealmentFail")?);
        sb.push('\n');
        sb.push_str(&t(w, "TooltipResultMiss")?);
        return Ok(sb);
    }
    if w.game_log.ctx.attack_number >= 0 && w.game_log.ctx.attacks_count > 1 {
        sb.push_str(&t(w, "TooltipAttackNumber")?);
        sb.push('\n');
    }
    if concealment == 1 {
        sb.push_str(&t(w, "TooltipConcealmentPass")?);
        sb.push('\n');
    }
    let crit_miss_empty = strings(w)?
        .key("GameLog/Attack/TooltipAttackRollCriticalMiss")
        .map_err(TextError::Unsupported)?
        .is_empty();
    if a.d20 == 20 {
        sb.push_str(&t(w, "TooltipAttackRollNatural20")?);
    } else if a.d20 == 1 && !crit_miss_empty {
        sb.push_str(&t(w, "TooltipAttackRollCriticalMiss")?);
    } else {
        sb.push_str(&t(w, "TooltipAttackRoll")?);
    }
    sb.push('\n');
    if a.is_hit {
        sb.push_str(&t(w, "TooltipResultHit")?);
    } else if a.d20 == 1 {
        sb.push_str(&t(w, "TooltipResultCriticalMiss")?);
    } else {
        sb.push_str(&t(w, "TooltipResultMiss")?);
    }
    if a.is_critical_roll {
        let conf = match &a.confirmation {
            Roll::Known(r, _) => r.result,
            Roll::ResultOnly(v) => *v,
            Roll::Null => 0,
        };
        {
            let c = &mut w.game_log.ctx;
            c.roll = crate::csharp::arith::add(conf, a.attack_bonus);
            c.d20 = a.confirmation.clone();
            c.modifier = a.attack_bonus;
            c.dc = a.target_critical_ac;
        }
        sb.push_str("\n\n");
        sb.push_str(&t(
            w,
            if conf == 20 { "TooltipCritRollNatural20" } else { "TooltipCritRoll" },
        )?);
        w.game_log.ctx.dc = a.target_ac;
        sb.push('\n');
        sb.push_str(&t(
            w,
            if a.is_critical_confirmed {
                "TooltipResultCritConfirmed"
            } else {
                "TooltipResultCritUnconfirmed"
            },
        )?);
    }
    sb.push_str("\n\n");
    let Some(rules) = a.rules.as_deref() else {
        return Err(TextError::Unsupported(
            "an attack roll's AttackBonusRule and ACRule (not kept)".into(),
        ));
    };
    tips::append_attack_bonus(w, &mut sb, &rules.0)?;
    sb.push('\n');
    tips::append_armor_class(w, &mut sb, &rules.1)?;
    Ok(sb)
}

/// `rule.Weapon.Wielder == null ? rule.Reason.Name : rule.Weapon.Name`.
fn weapon_text(w: &World, a: &AttackRollLog) -> Result<String, TextError> {
    let item = w
        .item(a.weapon_item)
        .ok_or_else(|| TextError::Unsupported("the weapon isn't an item".into()))?;
    if item.wielder.is_none() {
        return Err(TextError::Unsupported(
            "an attack with a weapon without a wielder (Reason.Name)".into(),
        ));
    }
    crate::game_log::names::item_name(w, a.weapon_item)
}

/// `DamageDealtLogThread.HandleEvent(RuleDealDamage)` (`DisableBattleLog` unset; not tactical
/// combat) with `DamageLogMessage.GetData`; the tooltip isn't made.
fn damage_thread(w: &mut World, th: usize, e: usize, d: &DealDamageLog) -> R {
    if !d.weapon_attack {
        return Err(TextError::Unsupported("damage that isn't a weapon attack's".into()));
    }
    w.game_log.retain();
    let r = damage_data(w, d);
    if let Ok(Some(m)) = &r {
        add_message(w, th, Some(e), m.clone());
    }
    w.game_log.release();
    r.map(|_| ())
}

fn damage_data(w: &mut World, d: &DealDamageLog) -> Result<Option<Msg0>, TextError> {
    w.game_log.retain();
    let r = (|| {
        let reduced = d.damage_before_difficulty < d.damage_without_reduction || d.any_half;
        {
            let c = &mut w.game_log.ctx;
            c.source_unit = (d.target != d.initiator).then_some(d.initiator);
            // A weapon attack's `Reason.Fact` is null (its reason is a rule's).
            c.source_fact = SourceFact::Null;
            c.target = Some(d.target);
            c.count = d.result;
        }
        let has_source = w.game_log.ctx.source_unit.is_some();
        let text = if has_source {
            text_of(
                w,
                if reduced { "GameLog/Damage/MessageReduced" } else { "GameLog/Damage/Message" },
            )?
        } else {
            // `DamageBundle.First.CausedByCheckFail`: a weapon attack's damage isn't.
            text_of(w, "GameLog/Damage/MessageUnknownSource")?
        };
        if text.is_empty() {
            return Ok(None);
        }
        // `Count = Result` again, then the tooltip (no redirection: `Target` stays).
        w.game_log.ctx.count = d.result;
        let tooltip = tips::damage_tooltip(w, &d.tip)?;
        let icon = icon(w);
        let color = color_of(w, "GameLog/Damage")?;
        let tip = if tooltip.is_empty() {
            Tip::Simple { cls: "TooltipTemplateSimple", header: None, desc: Some(text.clone()) }
        } else {
            Tip::Simple {
                cls: "TooltipTemplateCombatLogMessage",
                header: Some(text.clone()),
                desc: Some(tooltip),
            }
        };
        Ok(Some(Msg0 { text, color, icon, tip }))
    })();
    w.game_log.release();
    r
}

/// `UnitInitiativeLogThread.HandleEvent(RuleInitiativeRoll)`.
fn initiative_thread(
    w: &mut World,
    th: usize,
    e: usize,
    initiator: EntityIdx,
    result: i32,
    d20: Roll,
    modifier: i32,
) -> R {
    w.game_log.retain();
    {
        let c = &mut w.game_log.ctx;
        c.source_unit = Some(initiator);
        c.roll = result;
        c.d20 = d20;
        c.modifier = modifier;
    }
    let r = game_log_message(w, "GameLog/InitiativeRoll");
    if let Ok(Some(m)) = &r {
        add_message(w, th, Some(e), m.clone());
    }
    w.game_log.release();
    r.map(|_| ())
}

/// `UnitLifeStateChangedLogThread`: a unit dead (as the tick finds it) and not immortal.
fn death_thread(w: &mut World, th: usize, e: usize, unit: EntityIdx) -> R {
    let Some(u) = w.unit(unit) else { return Err(TextError::Unsupported("not a unit".into())) };
    let immortal = u.mechanics_features.get(&IMMORTALITY).copied().unwrap_or(0) > 0;
    if u.life_state != crate::combat::life::state::DEAD || immortal {
        return Ok(());
    }
    w.game_log.retain();
    w.game_log.ctx.source_unit = Some(unit);
    let r = game_log_message(w, "GameLog/UnitDeath");
    // `new CombatLogMessage(null, …)` throws: no message.
    if let Ok(Some(m)) = &r {
        add_message(w, th, Some(e), m.clone().copy_without_tip());
    }
    w.game_log.release();
    r.map(|_| ())
}

/// `MechanicsFeatureType.Immortality`.
const IMMORTALITY: i64 = 44;

/// `PartyCombatLogThread`.
fn party_combat_thread(w: &mut World, th: usize, e: usize, in_combat: bool) -> R {
    let path = if in_combat { "GameLog/CombatStarted" } else { "GameLog/CombatEnded" };
    if let Some(m) = game_log_message(w, path)? {
        add_message(w, th, Some(e), m.copy_without_tip());
    }
    Ok(())
}

/// `AttackOfOpportunityLogThread` (no scope of its own: the tick's).
fn aoo_thread(w: &mut World, th: usize, e: usize, attacker: EntityIdx, target: EntityIdx) -> R {
    w.game_log.ctx.source_unit = Some(attacker);
    w.game_log.ctx.target = Some(target);
    if let Some(m) = game_log_message(w, "GameLog/AttackOfOpportunity")? {
        add_message(w, th, Some(e), m.copy_without_tip());
    }
    Ok(())
}

/// `ItemsCollectionLogThread.HandleItemsAdded`/`HandleItemsRemoved`: a message only for the
/// player's inventory (the collection the main character holds) and a lootable item that isn't in
/// one of the unit's additional limbs' slots. A unit's own inventory (a dead creature's, whose
/// skinning items leave it as its loot drops) makes none, but the event still takes its place in
/// the queue, so every later message of the frame shifts.
fn items_collection_thread(w: &mut World, removed: bool, collection: usize, item: EntityIdx) -> R {
    if !crate::items::is_player_inventory(w, collection) {
        return Ok(());
    }
    match crate::items::is_lootable(w, item) {
        Some(false) => return Ok(()),
        Some(true) => {}
        None => {
            return Err(TextError::Unsupported(
                "ItemsCollectionLogThread: IsLootable of a weapon whose type isn't available"
                    .into(),
            ));
        }
    }
    Err(TextError::Unsupported(format!(
        "ItemsCollectionLogThread's {} message for the player's inventory",
        if removed { "lost" } else { "gained" }
    )))
}

/// `PartyGainExperienceLogThread`: `Count = Experience * ExperienceRatePercent / 100`.
fn party_xp_thread(w: &mut World, th: usize, e: usize, experience: i32) -> R {
    w.game_log.retain();
    let rate = w.player.experience_rate_percent;
    w.game_log.ctx.count =
        crate::csharp::arith::div(crate::csharp::arith::mul(experience, rate), 100).unwrap_or(0);
    let r = game_log_message(w, "GameLog/XpGain");
    if let Ok(Some(m)) = &r {
        add_message(w, th, Some(e), m.clone().copy_without_tip());
    }
    w.game_log.release();
    r.map(|_| ())
}

/// `RulebookCastSpellLogThread`: `BaseUseAbilityLogThread.HandleUseAbility(rule.Spell, rule)` for
/// a spellbook spell that didn't fail: `CastSpell`'s message, its tooltip a
/// `TooltipTemplateAbility` (not made).
fn cast_spell_thread(
    w: &mut World,
    th: usize,
    e: usize,
    initiator: EntityIdx,
    spell: wr_content::Guid,
    spellbook: bool,
    failed: bool,
) -> R {
    w.game_log.retain();
    let r = (|| {
        let bp = w
            .content
            .get(spell)
            .ok_or_else(|| TextError::Unsupported("the spell's blueprint is missing".into()))?;
        if bp.view().bool("DisableLog") {
            return Ok(());
        }
        if failed || !spellbook {
            return Err(TextError::Unsupported("a failed or non-spellbook cast's message".into()));
        }
        w.game_log.ctx.source_unit = Some(initiator);
        // `ability.Name`, `ability.Description`: the blueprint's (no item, param fact or
        // conversion).
        let name = names::blueprint_text(w, spell, "m_DisplayName")?;
        let desc = names::blueprint_text(w, spell, "m_Description")?;
        w.game_log.ctx.text = Some(name);
        w.game_log.ctx.description = Some(desc);
        w.game_log.ctx.tooltip = true;
        w.game_log.ctx.tooltip_type = Some("AbilityData");
        if let Some(m) = game_log_message(w, "GameLog/CastSpell")? {
            add_message(
                w,
                th,
                Some(e),
                Msg0 { tip: Tip::NotPorted("TooltipTemplateAbility"), ..m },
            );
        }
        Ok(())
    })();
    w.game_log.release();
    r
}

/// `BarkLogThread` for a unit's bark (a map object's isn't ported).
fn bark_thread(w: &mut World, th: usize, e: usize, unit: Option<EntityIdx>, text: String) -> R {
    let Some(u) = unit else {
        return Err(TextError::Unsupported("a map object's bark (BarkSpeaker)".into()));
    };
    w.game_log.retain();
    w.game_log.ctx.source_unit = Some(u);
    w.game_log.ctx.text = Some(text);
    let r = game_log_message(w, "GameLog/UnitBark");
    if let Ok(Some(m)) = &r {
        add_message(w, th, Some(e), m.clone().copy_without_tip());
    }
    w.game_log.release();
    r.map(|_| ())
}

/// `ProjectileHitLogThread`: the attack roll's message, made now unless kept.
fn projectile_thread(w: &mut World, th: usize, e: usize, attack: Option<usize>) -> R {
    let Some(ae) = attack else {
        // `AttackRoll == null`: nothing.
        return Ok(());
    };
    let Payload::AttackRoll(a) = w.game_log.events[ae].payload.clone() else {
        return Err(TextError::Unsupported(
            "a projectile's attack roll the sim doesn't have".into(),
        ));
    };
    w.game_log
        .attack_state
        .entry(a.seq)
        .or_insert_with(|| AttackState { suspend: a.suspend, ..AttackState::default() })
        .suspend = false;
    if !is_in_game(w, a.initiator) || !is_in_game(w, a.target) || a.auto_hit {
        return Ok(());
    }
    let kept = w.game_log.attack_state.get(&a.seq).and_then(|s| s.entry.clone());
    let msg = match kept {
        Some(m) => Some(m),
        None => attack_data(w, &a)?,
    };
    let st = w.game_log.attack_state.entry(a.seq).or_default();
    if st.suspend {
        st.entry = msg;
        return Ok(());
    }
    st.added = true;
    // `Result != Parried`: `AddMessage(null)` adds nothing.
    if a.result != 9
        && let Some(m) = msg
    {
        add_message(w, th, Some(e), m);
    }
    Ok(())
}

pub mod names;

// ---- the template environment ----

/// Templates evaluated inside a log scope: `GameLogContext`'s fields, and the world for the
/// rest (`mf` reads `SourceUnit` first).
struct LogEnv<'a> {
    w: &'a World,
}

fn log_unit(w: &World, u: EntityIdx) -> Result<LogUnit, TextError> {
    let unit = w.unit(u).ok_or_else(|| TextError::Unsupported("not a unit".into()))?;
    let bp =
        unit.blueprint.ok_or_else(|| TextError::Unsupported("unit without blueprint".into()))?;
    Ok(LogUnit {
        name: names::character_name(w, u)?,
        color_rgb: crate::text::blueprint_color_html(w, bp)?,
    })
}

impl Env for LogEnv<'_> {
    fn mf_gender(&mut self) -> Result<Gender, TextError> {
        let w = self.w;
        let u = w.game_log.ctx.source_unit.or(w.dialog.acting_unit).or(w.main_character);
        Ok(u.map_or(Gender::Male, |u| crate::text::gender(w, u)))
    }

    fn player_is_king(&mut self) -> Result<bool, TextError> {
        crate::text::WorldEnv { w: self.w }.player_is_king()
    }

    // `NameTemplate` and `RaceTemplate` (`text::WorldEnv`'s), their names converted in the log's
    // scope too.
    fn name(&mut self) -> Result<String, TextError> {
        let w = self.w;
        let u = w
            .dialog
            .acting_unit
            .or(w.main_character)
            .ok_or_else(|| TextError::Throws("{name}: no main character".into()))?;
        names::character_name(w, u)
    }

    fn race_name(&mut self) -> Result<String, TextError> {
        let w = self.w;
        let mc = w
            .main_character
            .ok_or_else(|| TextError::Throws("{race}: no main character".into()))?;
        let race = w
            .unit(mc)
            .and_then(|u| u.progression.race)
            .ok_or_else(|| TextError::Throws("{race}: no race".into()))?;
        names::blueprint_text(w, race, "m_DisplayName")
    }

    fn narrator_color(&mut self) -> Result<Color32, TextError> {
        crate::text::WorldEnv { w: self.w }.narrator_color()
    }

    fn tooltip_color(&mut self, ty: wr_text::TooltipType) -> Result<Color32, TextError> {
        crate::text::WorldEnv { w: self.w }.tooltip_color(ty)
    }

    fn log_unit(&mut self, field: LogUnitField) -> Result<Option<LogUnit>, TextError> {
        let c = &self.w.game_log.ctx;
        let u = match field {
            LogUnitField::SourceUnit => c.source_unit,
            LogUnitField::Target => c.target,
        };
        u.map(|u| log_unit(self.w, u)).transpose()
    }

    fn log_source_fact_name(&mut self) -> Result<Option<String>, TextError> {
        match &self.w.game_log.ctx.source_fact {
            SourceFact::Null => Ok(None),
            SourceFact::Unknown(d) => Err(TextError::Unsupported(format!(
                "GameLogContext.SourceFact's name (from before the window: {d})"
            ))),
        }
    }

    fn log_str(&mut self, field: LogStrField) -> Result<Option<String>, TextError> {
        let c = &self.w.game_log.ctx;
        Ok(match field {
            LogStrField::Text => c.text.clone(),
            LogStrField::TextWithTags => c.text_with_tags.clone(),
            LogStrField::Description => c.description.clone(),
            LogStrField::Formula => c.formula.clone(),
        })
    }

    fn log_int(&mut self, field: LogIntField) -> Result<i32, TextError> {
        let c = &self.w.game_log.ctx;
        Ok(match field {
            LogIntField::Count => c.count,
            LogIntField::Roll => c.roll,
            LogIntField::Modifier => c.modifier,
            LogIntField::Dc => c.dc,
            LogIntField::ChanceDc => c.chance_dc,
            LogIntField::RollChance => c.roll_chance,
            LogIntField::AttackNumber => c.attack_number,
            LogIntField::AttacksCount => c.attacks_count,
            LogIntField::RoundNumber => c.round_number,
        })
    }

    fn log_roll(&mut self, field: LogRollField) -> Result<Option<LogRoll>, TextError> {
        let c = &self.w.game_log.ctx;
        let r = match field {
            LogRollField::D20 => &c.d20,
            LogRollField::D100 => &c.d100,
        };
        match r {
            Roll::Null => Ok(None),
            Roll::Known(r, _) => Ok(Some(r.clone())),
            Roll::ResultOnly(_) => {
                Err(TextError::Unsupported("a roll's history (RuleRollDice.RollHistory)".into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::Item;
    use crate::stats::tests::world_with_unit;
    use crate::world::{Entity, EntityKind, FactsManager};

    fn item(w: &mut World) -> EntityIdx {
        w.add_entity(Entity {
            unique_id: "i1".into(),
            clr: "ItemEntitySimple",
            kind: EntityKind::Item(Box::new(Item { count: 1, ..Item::default() })),
            facts: FactsManager::default(),
            parts: Vec::new(),
            is_turned_on: true,
            post_load_executed: true,
            is_in_game: true,
            subscribed: false,
            disposing: false,
            is_disposed: false,
        })
    }

    /// A collection's event is queued like any other (it takes a place among the frame's five),
    /// but only the player's inventory makes a message: a unit's own collection (a dead
    /// creature's, as its skinning items leave) makes none, the player's reports (not ported).
    #[test]
    fn an_items_collection_event_is_queued_and_only_the_players_inventory_speaks() {
        let (mut w, u) = world_with_unit();
        let it = item(&mut w);
        items_collection(&mut w, true, 7, it, 1);
        assert!(w.game_log.events.is_empty(), "the log is off");
        w.game_log.on = true;
        items_collection(&mut w, true, 7, it, 1);
        assert_eq!(w.game_log.events.len(), 1);
        assert_eq!(w.game_log.events[0].ty.name(), "GameLogEventItemsCollection");
        assert_eq!(w.game_log.ready, vec![0]);
        // Not the player's inventory: no message, no report.
        assert!(items_collection_thread(&mut w, true, 7, it).is_ok());
        // The main character's collection: a message the sim doesn't make yet.
        w.main_character = Some(u);
        w.unit_mut(u).expect("unit").inventory_collection = Some(7);
        assert!(items_collection_thread(&mut w, true, 7, it).is_err());
        assert!(items_collection_thread(&mut w, false, 7, it).is_err());
        // An item that isn't lootable (a natural weapon, `m_NotLootable`) makes none.
        w.item_mut(it).expect("item").not_lootable = true;
        assert!(items_collection_thread(&mut w, true, 7, it).is_ok());
    }
}
