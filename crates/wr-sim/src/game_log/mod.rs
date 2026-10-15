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

use std::collections::HashMap;

use wr_text::{Color32, Env, LogIntField, LogRoll, LogRollField, LogStrField, LogUnit, LogUnitField};
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
    let mut v: Vec<&str> = THREADS
        .iter()
        .flat_map(|(_, h)| h.iter())
        .filter_map(|h| h.strip_prefix("R:"))
        .collect();
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
    THREADS.iter().enumerate().filter(|(_, (_, h))| h.contains(&k.as_str())).map(|(i, _)| i).collect()
}

/// A roll in `GameLogContext.D20`/`D100`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Roll {
    #[default]
    Null,
    Known(LogRoll),
    /// A rule's roll the sim keeps only as its result (its history isn't established).
    ResultOnly(i32),
}

/// `GameLogContext.SourceFact`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum SourceFact {
    #[default]
    Null,
    /// A fact the sim doesn't name (its `Name` is reported when read).
    Unknown(String),
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
        d20: i32,
        modifier: i32,
    },
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
    },
    CombatState {
        in_combat: bool,
    },
    PartyExperience {
        experience: i32,
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
}

/// A log event.
#[derive(Debug, Clone)]
pub struct Event {
    pub ty: EventType,
    /// The rule's sequence number (`RuleBase.seq`).
    pub rule_seq: Option<u64>,
    /// The rule's initiator and target (as the observer's rule records name them).
    pub units: [Option<EntityIdx>; 2],
    /// `m_IsEnabled`.
    pub enabled: bool,
    /// `OverrideMessage`'s key.
    pub override_message: Option<String>,
    pub payload: Payload,
    /// The frame it was queued in (`None` while open).
    pub queued: Option<i64>,
}

/// A message's tooltip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tip {
    None,
    /// A `TooltipTemplateSimple` (or subclass): its class, header and description.
    Simple { cls: &'static str, header: Option<String>, desc: Option<String> },
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
}

/// What the sim's log did, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// `AddReadyEvent` (an event index).
    Queued { frame: i64, event: usize },
    /// `Tick` with something queued or open: the queue (event index, `IsEnabled`), the open
    /// stack's size.
    Tick { frame: i64, queue: Vec<(usize, bool)>, stack: usize },
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
    pub out: Vec<Out>,
}

/// A `RuleAttackRoll`'s log members.
#[derive(Debug, Clone, Default)]
pub struct AttackState {
    pub entry: Option<Msg>,
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
pub fn about_to_trigger(w: &mut World, rule: &dyn RuleEvent) {
    if !w.game_log.on {
        return;
    }
    let class = rule.chain().first().copied().unwrap_or("?");
    let root = root_rules().contains(&class);
    if w.game_log.stack.is_empty() && !root {
        return;
    }
    let ev = Event {
        ty: EventType::Rule(class),
        rule_seq: Some(rule.base().seq),
        units: [Some(rule.initiator()), rule.target()],
        enabled: true,
        override_message: None,
        payload: Payload::None,
        queued: None,
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
    let payload = payload_of(w, rule);
    w.game_log.events[top].payload = payload;
    add_ready(w, top);
}

/// The payload of a finished rule.
fn payload_of(w: &mut World, rule: &mut dyn RuleEvent) -> Payload {
    use crate::combat::attack::{RuleAttackRoll, RuleAttackWithWeapon, RuleDealDamage};
    let explicit_reason = rule.base().reason.as_deref().is_some_and(|r| r != "self" && !r.starts_with("previous:"));
    let any = rule.as_any();
    if let Some(r) = any.downcast_ref::<RuleAttackRoll>() {
        let suspend = r.reason_full_attack.is_some() && r.weapon.is_ranged();
        return Payload::AttackRoll(Box::new(AttackRollLog {
            seq: r.base.seq,
            initiator: r.initiator,
            target: r.target,
            weapon_item: r.weapon.item,
            weapon_bp: r.weapon.bp.guid,
            d20: r.d20,
            attack_bonus: r.attack_bonus,
            target_ac: r.target_ac,
            result: r.result,
            is_hit: r.is_hit(),
            auto_hit: r.auto_hit,
            auto_miss: r.auto_miss,
            is_critical_confirmed: r.is_critical_confirmed,
            is_sneak_attack: r.is_sneak_attack,
            suspend,
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
        }));
    }
    if let Some(r) = any.downcast_ref::<crate::combat::initiative::RuleInitiativeRoll>() {
        return Payload::Initiative {
            initiator: r.initiator,
            result: r.result(),
            d20: r.d20,
            modifier: r.modifier,
        };
    }
    if let Some(r) = any.downcast_ref::<crate::combat::rules::RuleCanApplyBuff>() {
        return Payload::CanApplyBuff {
            initiator: r.initiator,
            blueprint: r.blueprint.guid,
            immunity: r.immunity,
        };
    }
    let _ = w;
    Payload::None
}

/// `GameLogController.AddReadyEvent`: `IsEnabled` (a rule event's reads its rule's
/// `DisableBattleLog`) and no `GameLogDisabled` scope; `GameLogContext.Message` as the
/// override; queued. The scope it opens clears the context when it is the outermost.
fn add_ready(w: &mut World, e: usize) {
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
    gl.out.push(Out::Queued { frame: gl.frame, event: e });
    gl.release();
}

/// A non-rule event raised through the event bus (its `GameEventsHandler.AddEvent`).
pub fn add_event(w: &mut World, ty: &'static str, payload: Payload, units: [Option<EntityIdx>; 2]) {
    if !w.game_log.on {
        return;
    }
    w.game_log.events.push(Event {
        ty: EventType::Other(ty),
        rule_seq: None,
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
pub fn life_state_changed(w: &mut World, unit: EntityIdx) {
    add_event(w, "GameLogEventUnitLifeStateChanged", Payload::LifeState { unit }, [Some(unit), None]);
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
    add_event(w, "GameLogEventPartyGainExperience", Payload::PartyExperience { experience }, [None, None]);
}

/// `IKnowledgeHandler.HandleKnowledgeUpdated` (`InspectUnitsManager.TryMakeKnowledgeCheck`).
pub fn knowledge_updated(w: &mut World) {
    add_event(w, "GameLogEventKnowledge", Payload::None, [None, None]);
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
        ("RulebookAttackWithWeaponLogThread", Payload::AttackWithWeapon { explicit_reason, .. }) => {
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
        ("UnitLifeStateChangedLogThread", Payload::LifeState { unit }) => death_thread(w, th, e, unit),
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
        ("ProjectileHitLogThread", Payload::ProjectileHit { attack }) => projectile_thread(w, th, e, attack),
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
    w.game_log.out.push(Out::Msg(Msg {
        frame,
        thread: th,
        src: e,
        text: m.text,
        color: m.color,
        icon: m.icon,
        tip: m.tip,
    }));
}

/// A `CombatLogMessage` before a thread adds it.
#[derive(Debug, Clone)]
struct Msg0 {
    text: String,
    color: String,
    icon: i32,
    tip: Tip,
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

fn localized(w: &World, key: &str) -> Result<String, TextError> {
    if key.is_empty() {
        return Ok(String::new());
    }
    let pack = &w.text.as_deref().ok_or_else(|| TextError::Unsupported("no localization pack loaded".into()))?.pack;
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
    // concealment's); one the sim can't make is left out of the comparison.
    let tooltip = |w: &World| -> Result<String, TextError> {
        let t = text_of(w, &format!("{path}/Tooltip"))?;
        Ok(match (w.game_log.ctx.tooltip, w.game_log.ctx.concealment) {
            (true, _) => t,
            (false, 2) => text_of(w, "GameLog/ConcealmentFail")?,
            (false, 1) => format!("{}\n{t}", text_of(w, "GameLog/ConcealmentPass")?),
            _ => t,
        })
    };
    let tip = match tooltip(w) {
        Ok(d) => Tip::Simple {
            cls: "TooltipTemplateCombatLogMessage",
            header: Some(text.clone()),
            desc: Some(d),
        },
        Err(TextError::Unsupported(_)) => Tip::NotPorted("TooltipTemplateCombatLogMessage"),
        Err(e) => return Err(e),
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
    let st = w.game_log.attack_state.entry(a.seq).or_insert_with(|| AttackState {
        suspend: a.suspend,
        ..AttackState::default()
    });
    let kept = st.entry.clone();
    let msg = match kept {
        Some(m) => Some(m),
        None => attack_data(w, a)?.map(|m| Msg { frame: 0, thread: th, src: Some(e), text: m.text, color: m.color, icon: m.icon, tip: m.tip }),
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
        add_message(w, th, Some(e), Msg0 { text: m.text, color: m.color, icon: m.icon, tip: m.tip });
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
        c.d20 = Roll::ResultOnly(a.d20);
        c.modifier = a.attack_bonus;
        c.dc = a.target_ac;
    }
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
    let icon = icon(w);
    let color = color_of(w, "GameLog/Attack")?;
    Ok(Some(Msg0 { text: s, color, icon, tip: Tip::NotPorted("TooltipTemplateCombatLogMessage") }))
}

/// `rule.Weapon.Wielder == null ? rule.Reason.Name : rule.Weapon.Name`.
fn weapon_text(w: &World, a: &AttackRollLog) -> Result<String, TextError> {
    let item = w.item(a.weapon_item).ok_or_else(|| TextError::Unsupported("the weapon isn't an item".into()))?;
    if item.wielder.is_none() {
        return Err(TextError::Unsupported("an attack with a weapon without a wielder (Reason.Name)".into()));
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
    w.game_log.release();
    if let Some(m) = r? {
        add_message(w, th, Some(e), m);
    }
    Ok(())
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
            text_of(w, if reduced { "GameLog/Damage/MessageReduced" } else { "GameLog/Damage/Message" })?
        } else {
            // `DamageBundle.First.CausedByCheckFail`: a weapon attack's damage isn't.
            text_of(w, "GameLog/Damage/MessageUnknownSource")?
        };
        if text.is_empty() {
            return Ok(None);
        }
        let icon = icon(w);
        let color = color_of(w, "GameLog/Damage")?;
        Ok(Some(Msg0 { text, color, icon, tip: Tip::NotPorted("TooltipTemplateCombatLogMessage") }))
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
    d20: i32,
    modifier: i32,
) -> R {
    w.game_log.retain();
    {
        let c = &mut w.game_log.ctx;
        c.source_unit = Some(initiator);
        c.roll = result;
        c.d20 = Roll::ResultOnly(d20);
        c.modifier = modifier;
    }
    let r = game_log_message(w, "GameLog/InitiativeRoll");
    w.game_log.release();
    if let Some(m) = r? {
        add_message(w, th, Some(e), m);
    }
    Ok(())
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
    w.game_log.release();
    // `new CombatLogMessage(null, …)` throws: no message.
    if let Some(m) = r? {
        add_message(w, th, Some(e), m.copy_without_tip());
    }
    Ok(())
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

/// `PartyGainExperienceLogThread`: `Count = Experience * ExperienceRatePercent / 100`.
fn party_xp_thread(w: &mut World, th: usize, e: usize, experience: i32) -> R {
    w.game_log.retain();
    let rate = w.player.experience_rate_percent;
    w.game_log.ctx.count = crate::csharp::arith::div(crate::csharp::arith::mul(experience, rate), 100).unwrap_or(0);
    let r = game_log_message(w, "GameLog/XpGain");
    w.game_log.release();
    if let Some(m) = r? {
        add_message(w, th, Some(e), m.copy_without_tip());
    }
    Ok(())
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
    w.game_log.release();
    if let Some(m) = r? {
        add_message(w, th, Some(e), m.copy_without_tip());
    }
    Ok(())
}

/// `ProjectileHitLogThread`: the attack roll's message, made now unless kept.
fn projectile_thread(w: &mut World, th: usize, e: usize, attack: Option<usize>) -> R {
    let Some(ae) = attack else {
        // `AttackRoll == null`: nothing.
        return Ok(());
    };
    let Payload::AttackRoll(a) = w.game_log.events[ae].payload.clone() else {
        return Err(TextError::Unsupported("a projectile's attack roll the sim doesn't have".into()));
    };
    w.game_log.attack_state.entry(a.seq).or_insert_with(|| AttackState { suspend: a.suspend, ..AttackState::default() }).suspend = false;
    if !is_in_game(w, a.initiator) || !is_in_game(w, a.target) || a.auto_hit {
        return Ok(());
    }
    let kept = w.game_log.attack_state.get(&a.seq).and_then(|s| s.entry.clone());
    let msg = match kept {
        Some(m) => Some(Msg0 { text: m.text, color: m.color, icon: m.icon, tip: m.tip }),
        None => attack_data(w, &a)?,
    };
    let st = w.game_log.attack_state.entry(a.seq).or_default();
    if st.suspend {
        st.entry = msg.map(|m| Msg { frame: 0, thread: th, src: Some(e), text: m.text, color: m.color, icon: m.icon, tip: m.tip });
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
    let bp = unit.blueprint.ok_or_else(|| TextError::Unsupported("unit without blueprint".into()))?;
    Ok(LogUnit {
        name: crate::text::character_name(w, u)?,
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

    fn name(&mut self) -> Result<String, TextError> {
        crate::text::WorldEnv { w: self.w }.name()
    }

    fn race_name(&mut self) -> Result<String, TextError> {
        crate::text::WorldEnv { w: self.w }.race_name()
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
            SourceFact::Unknown(d) => {
                Err(TextError::Unsupported(format!("GameLogContext.SourceFact's name ({d})")))
            }
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
            Roll::Known(r) => Ok(Some(r.clone())),
            Roll::ResultOnly(_) => {
                Err(TextError::Unsupported("a roll's history (RuleRollDice.RollHistory)".into()))
            }
        }
    }
}
