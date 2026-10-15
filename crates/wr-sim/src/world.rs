//! The simulation's state: entities (units, items, the rest) with durable game IDs
//! (`UniqueId` strings, not Bevy entities), facts with their components, and modifiers.
//!
//! The C# object graph is full of shared references (a modifier sits in a stat's list and
//! in a field of the stat or an item; a fact is referenced from modifiers, from
//! `UnitProgressionData.Features`, from `AddFacts` data…). Here every such object lives in
//! an arena on [`World`] and is referenced by index, and every C# method that mutates the
//! graph is a function taking `&mut World`, so re-entrant call chains (a stat update
//! triggering a dependent stat's update, which fires `OnChanged`, which adds a modifier to
//! a third stat) run in the same order as in the original.

use std::collections::HashMap;
use std::rc::Rc;

use wr_content::Guid;

use crate::content::{Blueprint, Content};
use crate::csharp::TimeSpan;
use crate::registry::{Coverage, Registry};
use crate::rng::{PlayRng, Rng};
use crate::rulebook::bus::EventBus;
use crate::stats::modifier::Modifier;
use crate::stats::value::Stats;
use crate::time::SimClock;

macro_rules! idx {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u32);
        impl $name {
            #[must_use]
            pub fn i(self) -> usize {
                self.0 as usize
            }
        }
    };
}

idx!(
    /// An entity in [`World::entities`].
    EntityIdx
);
idx!(
    /// A fact in [`World::facts`].
    FactIdx
);
idx!(
    /// A modifier in [`World::mods`].
    ModIdx
);

/// A component of a fact: `(fact, index into Fact::components)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CompRef {
    pub fact: FactIdx,
    pub comp: usize,
}

/// `EntityDataBase` and its subclasses, as far as the sim models them.
#[derive(Debug)]
pub struct Entity {
    /// `UniqueId`: the durable game ID.
    pub unique_id: String,
    /// CLR short name of the saved type (`UnitEntityData`, `ItemEntityArmor`…).
    pub clr: &'static str,
    pub kind: EntityKind,
    pub facts: FactsManager,
    /// Parts by CLR short name, in saved order (`EntityPartsManager.m_Parts`).
    pub parts: Vec<Part>,
    pub is_turned_on: bool,
    pub post_load_executed: bool,
    pub is_in_game: bool,
    pub subscribed: bool,
    pub disposing: bool,
    /// `EntityDataBase.IsDisposed`.
    pub is_disposed: bool,
}

#[derive(Debug)]
pub enum EntityKind {
    Unit(Box<crate::unit::Unit>),
    Item(Box<crate::item::Item>),
    /// `DroppedLoot.EntityData` (`UnitEntityData.DropLootToGround`).
    DroppedLoot(Box<crate::combat::loot::DroppedLoot>),
    Other,
}

/// `EntityFactsManager`.
#[derive(Debug, Default)]
pub struct FactsManager {
    /// `m_Facts`, in order.
    pub list: Vec<FactIdx>,
    /// The list's capacity (`List<T>` growth), which bounds `Sort`'s depth limit.
    pub capacity: usize,
    pub is_turned_on: bool,
}

/// An entity part (`EntityPart`/`UnitPart` subclasses) the sim keeps by type name. Parts
/// whose behaviour isn't ported are kept so the coverage manifest can list them.
#[derive(Debug, Clone)]
pub struct Part {
    pub clr: &'static str,
}

/// What kind of `EntityFact` a fact is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub enum FactKind {
    Feature,
    Buff,
    Ability,
    ActivatableAbility,
    ItemEnchantment,
    MainUnitFact,
    /// A plain `UnitFact` (e.g. a racial or blueprint fact).
    UnitFact,
    Etude,
    /// Any other `EntityFact` subclass (area facts, kingdom facts…).
    Other,
}

/// `EntityFact` and its subclasses.
#[derive(Debug)]
pub struct Fact {
    pub kind: FactKind,
    /// CLR short name of the saved type.
    pub clr: &'static str,
    pub blueprint: Option<Guid>,
    pub bp: Option<Rc<Blueprint>>,
    pub unique_id: String,
    pub attach_time: TimeSpan,
    /// `IsActive`: saved.
    pub is_active: bool,
    /// `IsTurnedOn`: not saved; rebuilt by `TurnOn`.
    pub is_turned_on: bool,
    pub post_load_executed: bool,
    pub is_disposed: bool,
    /// `Manager.Owner`, once attached (`RestoreOwnerLink` on load).
    pub owner: Option<EntityIdx>,
    pub components: Vec<Component>,
    pub turn_on_priority: i32,
    pub turn_off_priority: i32,
    pub activating: bool,
    pub deactivating: bool,
    pub reapplying: bool,
    pub subscribed: bool,
    pub remove_when_activation_ended: bool,
    pub reapply_on_apply_post_load_fixes: bool,
    pub recalculate_context_before_turn_on: bool,
    pub source_item: Option<EntityIdx>,
    pub source_fact: Option<FactIdx>,
    /// `Feature.Rank` / buff rank.
    pub rank: i32,
    /// `MechanicsContext`, for `IFactContextOwner` facts.
    pub context: Option<crate::context::MechanicsContext>,
    /// `m_BlueprintComponents`: `BlueprintFact.CollectComponents()`, computed lazily.
    pub blueprint_components: Option<Vec<(Rc<Blueprint>, usize)>>,
    /// `Feature.DisabledBecauseOfPrerequisites`.
    pub disabled_because_of_prerequisites: bool,
    /// `Feature.IgnorePrerequisites` (`[DefaultValue(true)]`: absent in a save means true).
    pub ignore_prerequisites: bool,
    /// `Feature.m_Source` (the progression, race or class that granted it).
    pub source_blueprint: Option<Guid>,
    /// `Buff.IsSuppressed`.
    pub is_suppressed: bool,
    /// Buff timing (saved absolute game times).
    pub buff: Option<BuffTimes>,
    /// `ActivatableAbility` saved state: `m_IsOn`, `IsStarted`, `m_AppliedBuff`.
    pub activatable: Option<(bool, bool, Option<FactIdx>)>,
    /// `ActivatableAbility.SelectTargetAbility` (saved): a targeted toggle's target-selection
    /// ability while active.
    pub select_target: Option<FactIdx>,
    /// `ActivatableAbility.ReadyToStart` (saved).
    pub ready_to_start: bool,
    /// `ActivatableAbility.TimeToNextRound` (saved).
    pub time_to_next_round: f32,
    /// `ActivatableAbility.m_WasInCombat` (saved).
    pub was_in_combat: bool,
    /// `ActivatableAbility.m_ShouldBeDeactivatedInNextRound` (not saved).
    pub deactivate_next_round: bool,
    /// `ItemEnchantment.ParentContext != null` (added by an effect, not by the blueprint).
    pub parent_context: bool,
    /// `ItemEnchantment.ParentContext` (saved, or given by `AddEnchantment`).
    pub enchant_parent: Option<Box<crate::context::MechanicsContext>>,
    /// `Feature.Param` as saved (`FeatureParam` persisted members), if not empty.
    pub param: Option<serde_json::Value>,
    /// State of narrative fact subclasses (etudes, quests, objectives, tutorials).
    pub ext: FactExt,
    /// An `Ability` fact's saved `UsagesPerDayResource` or `StickyTouch` is set (the sim keeps
    /// neither: an `AbilityData` of it reports them).
    pub ability_saved_extras: bool,
}

/// The members of narrative `EntityFact` subclasses (`Etude`, `Quest`, `QuestObjective`,
/// `Tutorial`), as saved, plus their runtime links.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum FactExt {
    #[default]
    None,
    /// `Etude`: `IsCompleted`, `CompletionInProgress` (saved); `Children` (rebuilt by
    /// `EtudesTree.RestoreTreeStructure`); the parent is `source_fact`.
    Etude { is_completed: bool, completion_in_progress: bool, children: Vec<FactIdx> },
    /// `Quest`: `m_State`, `m_NextObjectiveOrder`, `m_IsReadOnly`, `m_IsInUiSelected`, and
    /// `m_Objectives` in its dictionary order (blueprint → objective fact).
    Quest {
        state: i32,
        next_objective_order: i32,
        is_read_only: bool,
        is_in_ui_selected: bool,
        objectives: Vec<(Guid, FactIdx)>,
    },
    /// `QuestObjective`.
    Objective {
        state: i32,
        is_visible: bool,
        is_collapse: bool,
        start_time: i64,
        need_to_attention: bool,
        order: i32,
        quest: Option<FactIdx>,
        parent_objective: Option<FactIdx>,
    },
    /// `Tutorial`.
    Tutorial {
        triggered_times: i32,
        showed_times: i32,
        last_show_index: i32,
        trigger_logic_counter: i32,
        banned: bool,
    },
}

/// `Buff` timing members, absolute game time in ticks (not rebased on load).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct BuffTimes {
    /// `m_EndTime`; `None` is permanent (`TimeSpan.MaxValue`).
    pub end_time: Option<i64>,
    pub tick_time: i64,
    pub next_tick_time: i64,
    pub round_number: i64,
    pub is_from_spell: bool,
    /// `SourceAreaEffectId` (saved), when not empty.
    pub source_area_effect: Option<String>,
    /// `m_StoredFacts` (saved; `Buff.StoreFact`): the owner's facts `Buff.OnRemove` removes.
    #[serde(skip)]
    pub stored_facts: Vec<FactIdx>,
    /// `m_StoredMods` was saved non-empty: `OnRemove` would remove them (not modelled).
    #[serde(skip)]
    pub stored_mods: bool,
}

impl BuffTimes {
    /// `Buff.IsPermanent`: no end time, or one at `TimeSpan.MaxValue` (a save keeps the
    /// sentinel; buffs created in the sim store `None`).
    #[must_use]
    pub fn is_permanent(&self) -> bool {
        self.end_time.is_none_or(|t| t == i64::MAX)
    }
}

impl Fact {
    /// A fact with every runtime flag cleared, as Newtonsoft constructs it before
    /// populating saved members.
    #[must_use]
    pub fn blank(kind: FactKind, clr: &'static str) -> Self {
        Self {
            kind,
            clr,
            blueprint: None,
            bp: None,
            unique_id: String::new(),
            attach_time: TimeSpan::ZERO,
            is_active: false,
            is_turned_on: false,
            post_load_executed: false,
            is_disposed: false,
            owner: None,
            components: Vec::new(),
            turn_on_priority: 0,
            turn_off_priority: 0,
            activating: false,
            deactivating: false,
            reapplying: false,
            subscribed: false,
            remove_when_activation_ended: false,
            reapply_on_apply_post_load_fixes: false,
            recalculate_context_before_turn_on: false,
            source_item: None,
            source_fact: None,
            rank: 1,
            context: None,
            blueprint_components: None,
            disabled_because_of_prerequisites: false,
            ignore_prerequisites: true,
            source_blueprint: None,
            is_suppressed: false,
            buff: None,
            activatable: None,
            select_target: None,
            ready_to_start: false,
            time_to_next_round: 0.0,
            was_in_combat: false,
            deactivate_next_round: false,
            parent_context: false,
            enchant_parent: None,
            param: None,
            ext: FactExt::None,
            ability_saved_extras: false,
        }
    }
}

/// `EntityFactComponent` (a delegate's `ComponentRuntime`, or a custom runtime).
#[derive(Debug)]
pub struct Component {
    /// `SourceBlueprintComponentName`.
    pub name: String,
    /// `SourceBlueprintComponent`: the blueprint holding it and the index into its
    /// `ComponentsArray`. `None` until `Setup` (or after `Dispose`).
    pub source: Option<(Rc<Blueprint>, usize)>,
    /// The runtime's CLR type as saved (e.g. `ComponentRuntime` of `AddFacts`), if any.
    pub saved_clr: Option<String>,
    /// `m_Data` (the delegate's `TData`), as JSON; `None` is a null `m_Data`.
    pub data: Option<serde_json::Value>,
    pub is_turned_on: bool,
    pub is_disposed: bool,
    pub subscribed: bool,
    /// Facts the saved `m_Data` references, by member (`AddFactsData.AppliedFacts`…).
    pub fact_refs: std::collections::HashMap<String, Vec<Option<FactIdx>>>,
    /// The runtime's own saved members besides `m_Data` (e.g. `EtudeBracketRuntime
    /// .m_AlreadyProcessedActivation`), as JSON.
    pub runtime: serde_json::Map<String, serde_json::Value>,
    /// `EtudeBracketRuntime.m_NeedResume` (not saved).
    pub need_resume: bool,
}

impl Component {
    /// The blueprint component object, if set up.
    #[must_use]
    pub fn bp_component(&self) -> Option<&wr_content::node::Object> {
        let (bp, i) = self.source.as_ref()?;
        bp.components_array().get(*i).copied()
    }

    /// The content type of the blueprint component.
    #[must_use]
    pub fn ty(&self) -> Option<wr_content::schema::TypeIdx> {
        self.bp_component().map(|o| o.ty)
    }
}

/// An action queued with `Game.ScheduleAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduledAction {
    /// `DialogController.StartScheduledDialogImmediately`.
    StartScheduledDialog,
    /// `StopDialog`'s `IDialogInteractionHandler.StopDialogInteraction` event.
    StopDialogInteraction,
}

/// A before-tick task (`GameTaskAction`, `GameTaskStartMode`, `GameTaskStopMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeforeTick {
    Action(ScheduledAction),
    StartMode(&'static str),
    StopMode(&'static str),
    /// `CutsceneLock.SetCutsceneLock`'s scheduled `UpdateModeForCutsceneLock(wasLocked)`.
    CutsceneLockMode {
        was_locked: bool,
    },
}

/// Settings that rules read (`SettingsRoot`, difficulty).
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// `SettingsRoot.Difficulty.DeadCompanionsRiseAfterCombat`.
    pub dead_companions_rise_after_combat: bool,
    /// `SettingsRoot.Difficulty` (the save's settings) and `DifficultyPresetsList`.
    pub difficulty: crate::difficulty::Difficulty,
    /// `BlueprintDlcReward.IsAvailable` per reward: platform state (the store's purchase
    /// and mount state), not in the save, read from `cache/sim/dlc-rewards.json`
    /// (`{"<reward guid>": bool}`). A reward not listed makes the condition unsupported.
    pub dlc_rewards: Option<std::collections::HashMap<wr_content::Guid, bool>>,
}

/// A diagnostic the original would log (`PFLog`), kept for the run report.
#[derive(Debug, Clone, serde::Serialize)]
pub struct LogEntry {
    pub level: &'static str,
    pub message: String,
}

/// One step of the lifecycle, recorded when tracing is on (tests, and the hydration
/// report's order check).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TraceEntry {
    pub what: &'static str,
    pub subject: String,
}

/// `BlueprintSpellList.GetSpells(level)` results by (list, level).
pub type SpellCache = std::cell::RefCell<HashMap<(Guid, i32), Rc<Vec<Guid>>>>;

/// The whole simulation state.
pub struct World {
    pub content: Rc<Content>,
    /// The localization pack text is shown from (`LocalizationManager.CurrentPack`), when
    /// loaded.
    pub text: Option<Rc<crate::text::TextData>>,
    pub registry: Rc<Registry>,
    pub entities: Vec<Entity>,
    pub facts: Vec<Fact>,
    pub mods: Vec<Modifier>,
    pub by_id: HashMap<String, EntityIdx>,
    pub coverage: Coverage,
    /// Awake states taken from a recording (a dialogue fixture's `awake_set`/`awake`), when the
    /// run has one: [`crate::spawn::RecordedAwake`].
    pub recorded_awake: Option<Box<crate::spawn::RecordedAwake>>,
    /// Cutscene progress taken from a recording: [`crate::elements::cutscene::RecordedCutscene`].
    pub recorded_cs: Option<Box<crate::elements::cutscene::RecordedCutscene>>,
    /// Unit groups' states recorded in a dialogue fixture, checked after each groups tick:
    /// [`crate::combat::groups::RecordedGroups`].
    pub recorded_groups: Option<Box<crate::combat::groups::RecordedGroups>>,
    /// Roaming's recorded producers by oracle tick, audited at their ticks
    /// ([`crate::runner::roaming_tick`]).
    pub recorded_roaming: Option<Box<std::collections::BTreeMap<i64, Vec<serde_json::Value>>>>,
    /// A recording's dialogue inputs (oracle tick, step), applied at their ticks.
    pub recorded_inputs: Option<std::collections::VecDeque<(i64, String)>>,
    pub bus: EventBus,
    pub rulebook: crate::rulebook::Rulebook,
    pub rng: Box<dyn Rng>,
    pub clock: SimClock,
    pub settings: Settings,
    pub log: Vec<LogEntry>,
    pub trace: Option<Vec<TraceEntry>>,
    /// `Player.MainCharacter`.
    pub main_character: Option<EntityIdx>,
    /// Identities for limiters created at run time (counting down, away from the saved
    /// ones).
    pub limiter_seq: u32,
    /// `Player`'s narrative and party state.
    pub player: crate::narrative::Player,
    /// `ContextData<T>` scopes, innermost last.
    pub ctx: Vec<crate::elements::Ctx>,
    /// The T0 event trace of the run (dialogue, narrative mutations, rules).
    pub events: crate::events::EventLog,
    /// `Game.Instance.DialogController`.
    pub dialog: crate::dialog::DialogController,
    /// `Game.m_BeforeTickActions`: `ScheduleAction`, `StartMode` and `StopMode` requests in
    /// one FIFO queue, run at the start of the next `Game.Tick` (`ExecuteBeforeTickActions`).
    pub before_tick: Vec<BeforeTick>,
    /// Result fields of rules whose `OnTrigger` finished, by rule sequence number, until
    /// their `rule_end` record is written.
    pub rules_end_fields: HashMap<u64, serde_json::Value>,
    /// Buffs created but not yet attached (`unit_facts::add_buff`).
    pub pending_buffs: crate::unit_facts::PendingBuffs,
    /// The process-wide mutable `AbilityParams.Empty` (see [`EmptyAbilityParams`]).
    pub empty_params: EmptyAbilityParams,
    /// `ContextData<Feature.ParamContextData>.Current`: the param of the `AddFact` in progress.
    pub feature_param_ctx: Option<serde_json::Value>,
    /// Combat controllers' state (POC-5, `crate::combat`).
    pub combat: crate::combat::CombatWorld,
    /// `ContextData` flags of unit construction.
    pub build: crate::spawn::BuildFlags,
    /// Identities for `ItemsCollection`s created at run time.
    pub collection_seq: usize,
    /// Collections being disposed (`ItemsCollection.IsDisposingNow`).
    pub disposing_collections: Vec<usize>,
    /// Stand-ins for `Guid.NewGuid()` ids (a run's random GUIDs are renamed by first appearance
    /// when streams are compared).
    pub guid_seq: u64,
    /// Identities for `Spellbook` objects (a casting stat's `OnChanged` handler outlives a
    /// deleted spellbook).
    pub spellbook_serial: u32,
    /// `BlueprintSpellList.GetSpells(level)` after the DLC filter, by (list, level).
    pub spell_cache: SpellCache,
    /// The `AbilityData.UniqueId`s that came from the save (everything else a spellbook holds
    /// was created during the run).
    pub saved_spell_ids: std::collections::HashSet<String>,
    /// The loaded area's scene entities (script zones…) and scene cache.
    pub scene: crate::scene::SceneState,
    /// Items whose `m_HasExternalEnchantments` is set (`ItemEntity.UpdateHasExternalEnchantments`,
    /// run when an effect adds or removes an enchantment; not saved, so false after a load).
    pub external_enchantments: std::collections::HashSet<EntityIdx>,
    /// Items given their `ItemEnchantmentCollection` by `OnDidEquipped` in this run (see
    /// `items::has_enchantments_collection`).
    pub equipped_enchantment_collections: std::collections::HashSet<EntityIdx>,
    /// `Game.Instance.LevelUpController` (its state while lent; see `spawn::levelup::Slot`).
    pub(crate) level_up: Vec<crate::spawn::levelup::Slot>,
    /// Loaded items not yet registered with `EntityService` (their `PrePostLoad` hasn't run):
    /// slot references to them don't resolve (`items::slot_item_resolves`).
    pub unregistered_items: std::collections::HashSet<EntityIdx>,
    /// Units the load's start pass took as awake whose toggles acted (see
    /// `spawn::settle_assumed_awake`), until settled.
    pub assumed_awake_at_load: Vec<EntityIdx>,
    /// `GameLogController` and `GameLogContext` (the combat log), when a run models it.
    pub game_log: crate::game_log::GameLog,
}

/// `AbilityParams.Empty` (`AbilityParams.cs:7`): a `static readonly` instance with mutable
/// members, shared by every `MechanicsContext` whose `Params` falls through to it
/// (`m_Params ?? ParentContext?.Params ?? Empty`). `AddFacts`, `AddFactsToPet` and
/// `AddFactsToMount` write `Params.CasterLevel` on a fresh fact's context, which lands here.
/// Static in the game (so one value per process; a load starts from the constructor
/// defaults), kept on the [`World`] here. Only the members that are read or written are
/// modelled: the others stay at their constructor defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyAbilityParams {
    /// `CasterLevel` (constructor: 1).
    pub caster_level: i32,
    /// `RankBonus` (constructor: 0, never written on a path the sim runs).
    pub rank_bonus: i32,
    /// The value is the process's actual one: it was supplied (the oracle's
    /// `abilityParamsEmpty`) or a write in this World set it. A game process loads saves
    /// repeatedly and `Empty` is never reset, so otherwise the starting value depends on
    /// loads the save can't show and the constructor defaults are a guess.
    pub known: bool,
    /// The unknown read was reported already.
    reported: bool,
}

impl Default for EmptyAbilityParams {
    fn default() -> Self {
        Self { caster_level: 1, rank_bonus: 0, known: false, reported: false }
    }
}

impl EmptyAbilityParams {
    /// A supplied starting value (known).
    #[must_use]
    pub fn supplied(caster_level: i32, rank_bonus: i32) -> Self {
        Self { caster_level, rank_bonus, known: true, reported: false }
    }
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World")
            .field("entities", &self.entities.len())
            .field("facts", &self.facts.len())
            .field("mods", &self.mods.len())
            .finish_non_exhaustive()
    }
}

impl World {
    /// A read of `AbilityParams.Empty`. Before any write in this World, and without a
    /// supplied starting value, reports (once) that the value is unknown and uses the
    /// constructor defaults.
    pub fn read_empty_params(&mut self) -> EmptyAbilityParams {
        if !self.empty_params.known && !self.empty_params.reported {
            self.empty_params.reported = true;
            self.coverage.unsupported(
                "AbilityParams.Empty read before any write in this load: its value depends on the process's earlier loads",
            );
        }
        self.empty_params
    }

    /// A stand-in for `Guid.NewGuid().ToString()`: unique, 36 characters, deterministic.
    pub fn new_guid(&mut self) -> String {
        self.guid_seq += 1;
        format!("00000000-0000-4000-8000-{:012x}", self.guid_seq)
    }

    #[must_use]
    pub fn new(content: Rc<Content>, registry: Rc<Registry>) -> Self {
        Self {
            content,
            text: None,
            registry,
            entities: Vec::new(),
            facts: Vec::new(),
            mods: Vec::new(),
            by_id: HashMap::new(),
            coverage: Coverage::default(),
            recorded_awake: None,
            recorded_cs: None,
            recorded_groups: None,
            recorded_roaming: None,
            recorded_inputs: None,
            bus: EventBus::default(),
            rulebook: crate::rulebook::Rulebook::default(),
            rng: Box::new(PlayRng::new(0)),
            clock: SimClock::new(TimeSpan::ZERO, TimeSpan::ZERO),
            settings: Settings::default(),
            log: Vec::new(),
            trace: None,
            main_character: None,
            limiter_seq: u32::MAX,
            player: crate::narrative::Player::default(),
            ctx: Vec::new(),
            events: crate::events::EventLog::default(),
            dialog: crate::dialog::DialogController::default(),
            rules_end_fields: HashMap::new(),
            pending_buffs: crate::unit_facts::PendingBuffs::default(),
            empty_params: EmptyAbilityParams::default(),
            feature_param_ctx: None,
            before_tick: Vec::new(),
            combat: crate::combat::CombatWorld::default(),
            build: crate::spawn::BuildFlags::default(),
            collection_seq: usize::MAX / 2,
            disposing_collections: Vec::new(),
            guid_seq: 0,
            spellbook_serial: 0,
            spell_cache: std::cell::RefCell::new(HashMap::new()),
            saved_spell_ids: std::collections::HashSet::new(),
            scene: crate::scene::SceneState::default(),
            external_enchantments: std::collections::HashSet::new(),
            equipped_enchantment_collections: std::collections::HashSet::new(),
            level_up: Vec::new(),
            unregistered_items: std::collections::HashSet::new(),
            assumed_awake_at_load: Vec::new(),
            game_log: crate::game_log::GameLog::default(),
        }
    }

    pub fn trace(&mut self, what: &'static str, subject: impl FnOnce(&Self) -> String) {
        if self.trace.is_some() {
            let s = subject(self);
            if let Some(t) = &mut self.trace {
                t.push(TraceEntry { what, subject: s });
            }
        }
    }

    pub fn log(&mut self, level: &'static str, message: impl Into<String>) {
        self.log.push(LogEntry { level, message: message.into() });
    }

    /// `TimeController.Tick` on the sim clock. A turn-based tick needs
    /// `CombatController.TickTime`, which isn't ported: reported unsupported.
    ///
    /// # Errors
    ///
    /// Game time overflow.
    pub fn tick_clock(
        &mut self,
        f: crate::time::Frame,
    ) -> Result<crate::time::Tick, crate::csharp::arith::CsError> {
        let t = self.clock.tick(f)?;
        if t.needs_combat_tick_time {
            self.coverage.unsupported("CombatController.TickTime (turn-based time)");
        }
        Ok(t)
    }

    /// Ends a run: finishes the RNG (a replay tape with draws left fails).
    pub fn finish_run(&mut self) {
        self.rng.finish();
    }

    /// Whether the run so far is valid: nothing unsupported executed and no RNG failure.
    #[must_use]
    pub fn valid(&self) -> bool {
        self.coverage.valid() && self.rng.failures().is_empty()
    }

    /// A fresh identity for a `DexBonusLimiter`.
    pub fn next_limiter_id(&mut self) -> u32 {
        self.limiter_seq -= 1;
        self.limiter_seq
    }

    pub fn add_entity(&mut self, e: Entity) -> EntityIdx {
        let i = EntityIdx(u32::try_from(self.entities.len()).expect("< 2^32 entities"));
        self.by_id.insert(e.unique_id.clone(), i);
        self.entities.push(e);
        i
    }

    pub fn add_fact(&mut self, f: Fact) -> FactIdx {
        let i = FactIdx(u32::try_from(self.facts.len()).expect("< 2^32 facts"));
        self.facts.push(f);
        i
    }

    pub fn add_mod(&mut self, m: Modifier) -> ModIdx {
        let i = ModIdx(u32::try_from(self.mods.len()).expect("< 2^32 modifiers"));
        self.mods.push(m);
        i
    }

    #[must_use]
    pub fn entity(&self, e: EntityIdx) -> &Entity {
        &self.entities[e.i()]
    }

    pub fn entity_mut(&mut self, e: EntityIdx) -> &mut Entity {
        &mut self.entities[e.i()]
    }

    #[must_use]
    pub fn fact(&self, f: FactIdx) -> &Fact {
        &self.facts[f.i()]
    }

    pub fn fact_mut(&mut self, f: FactIdx) -> &mut Fact {
        &mut self.facts[f.i()]
    }

    #[must_use]
    pub fn unit(&self, e: EntityIdx) -> Option<&crate::unit::Unit> {
        match &self.entities[e.i()].kind {
            EntityKind::Unit(u) => Some(u),
            _ => None,
        }
    }

    pub fn unit_mut(&mut self, e: EntityIdx) -> Option<&mut crate::unit::Unit> {
        match &mut self.entities[e.i()].kind {
            EntityKind::Unit(u) => Some(u),
            _ => None,
        }
    }

    #[must_use]
    pub fn item(&self, e: EntityIdx) -> Option<&crate::item::Item> {
        match &self.entities[e.i()].kind {
            EntityKind::Item(i) => Some(i),
            _ => None,
        }
    }

    pub fn item_mut(&mut self, e: EntityIdx) -> Option<&mut crate::item::Item> {
        match &mut self.entities[e.i()].kind {
            EntityKind::Item(i) => Some(i),
            _ => None,
        }
    }

    /// A unit's stats.
    ///
    /// # Panics
    ///
    /// If `e` isn't a unit.
    #[must_use]
    pub fn stats(&self, e: EntityIdx) -> &Stats {
        &self.unit(e).expect("unit").stats
    }

    /// # Panics
    ///
    /// If `e` isn't a unit.
    pub fn stats_mut(&mut self, e: EntityIdx) -> &mut Stats {
        &mut self.unit_mut(e).expect("unit").stats
    }

    #[must_use]
    pub fn component(&self, c: CompRef) -> &Component {
        &self.facts[c.fact.i()].components[c.comp]
    }

    pub fn component_mut(&mut self, c: CompRef) -> &mut Component {
        &mut self.facts[c.fact.i()].components[c.comp]
    }

    /// Short description of a fact for logs and traces.
    #[must_use]
    pub fn fact_label(&self, f: FactIdx) -> String {
        let fact = self.fact(f);
        match &fact.bp {
            Some(bp) => format!("{}[{}]", fact.clr, bp.name),
            None => format!(
                "{}[{}]",
                fact.clr,
                fact.blueprint.map(|g| g.to_string()).unwrap_or_default()
            ),
        }
    }

    /// `EntityFact.BlueprintComponents` (`m_BlueprintComponents ??= CollectComponents()`).
    pub fn blueprint_components(&mut self, f: FactIdx) -> Vec<(Rc<Blueprint>, usize)> {
        if let Some(v) = &self.facts[f.i()].blueprint_components {
            return v.clone();
        }
        let v = match self.facts[f.i()].bp.clone() {
            Some(bp) => crate::content::collect_components(&self.content, &bp),
            None => Vec::new(),
        };
        self.facts[f.i()].blueprint_components = Some(v.clone());
        v
    }

    /// `EntityFactsManager.Owner` of a fact's manager is disposing (`Owner.IsDisposingNow`).
    #[must_use]
    pub fn owner_disposing(&self, f: FactIdx) -> bool {
        self.facts[f.i()].owner.is_none_or(|o| self.entities[o.i()].disposing)
    }
}

#[cfg(test)]
mod buff_time_tests {
    use super::BuffTimes;

    #[test]
    fn a_buff_is_permanent_without_an_end_time_or_at_the_max_value() {
        let at = |end_time| BuffTimes { end_time, ..BuffTimes::default() }.is_permanent();
        assert!(at(None));
        assert!(at(Some(i64::MAX)));
        assert!(!at(Some(434_213_070_000)));
    }
}
