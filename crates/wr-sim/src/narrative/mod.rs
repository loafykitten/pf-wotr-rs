//! Narrative and party state of `Player`: unlockable flags, the etude system, the quest
//! book, dialogue history, tutorials, the party, alignment, experience and unique ids.
//!
//! C# (decompiled, not copied): `Kingmaker/Player.cs` (party lists, `GetNewUniqueId`,
//! `GainPartyExperience`, `AddCompanion`), `Kingmaker.AreaLogic.QuestSystem/*`,
//! `Kingmaker.AreaLogic.Etudes/{EtudesSystem,Etude,EtudesTree}.cs`,
//! `Kingmaker.DialogSystem.State/DialogState.cs`, `Kingmaker.Tutorial/TutorialSystem.cs`,
//! `Kingmaker.UnitLogic.Alignments/UnitAlignment.cs`.
//!
//! The etude system, quest book and tutorial system are `EntityDataBase`s whose facts
//! (`Etude`, `Quest`/`QuestObjective`, `Tutorial`) live in [`World::facts`] like unit facts,
//! so their components run through the same fail-closed lifecycle and event bus.

pub mod alignment;
pub mod dump;
pub mod etudes;
pub mod flags;
pub mod party;
pub mod quests;
pub mod tutorial;

use std::collections::HashMap;

use serde::Serialize;
use wr_content::Guid;

use crate::csharp::dict::CsDict;
use crate::world::{EntityIdx, FactIdx, World};

/// `EtudesSystem.EtudeState` (private enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum EtudeState {
    Unknown = 0,
    Started = 1,
    Completed = 2,
    PreStarted = 3,
    PreCompleted = 4,
}

impl EtudeState {
    #[must_use]
    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "Unknown" => Self::Unknown,
            "Started" => Self::Started,
            "Completed" => Self::Completed,
            "PreStarted" => Self::PreStarted,
            "PreCompleted" => Self::PreCompleted,
            _ => return None,
        })
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Started => "Started",
            Self::Completed => "Completed",
            Self::PreStarted => "PreStarted",
            Self::PreCompleted => "PreCompleted",
        }
    }
}

/// `DialogState` (`Player.Dialog`): the global dialogue history. The sets are
/// `HashSet<T>`s; [`CsDict`] keeps Mono's slot order, which nothing observable depends
/// on (dumps sort them).
#[derive(Debug, Clone, Default)]
pub struct DialogState {
    pub selected_answers: CsDict<Guid, ()>,
    /// `AnswerChecks`: answer → `CheckResult` (0 Passed, 1 Failed).
    pub answer_checks: CsDict<Guid, i32>,
    pub shown_answer_lists: CsDict<Guid, ()>,
    pub shown_cues: CsDict<Guid, ()>,
    pub shown_dialogs: CsDict<Guid, ()>,
    /// `Scheduled` (`StartDialogData`): the dialogue and its unit ids.
    pub scheduled: Option<ScheduledDialog>,
}

/// `StartDialogData`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledDialog {
    pub dialog: Guid,
    pub initiator: Option<EntityIdx>,
    pub unit: Option<EntityIdx>,
    /// `CustomSpeakerName`: the `LocalizedString` the start passed, as a string (`"<null>"`
    /// for none).
    pub custom_speaker_name: String,
    pub is_scheduled: bool,
}

/// A unit spawner's saved state (`UnitSpawnerBase.MyData`): what `UnitFromSpawner` reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spawner {
    /// `m_SpawnedUnit` (a unit id; it may name a placeholder that hasn't become a unit).
    pub spawned_unit: Option<String>,
    pub has_spawned: bool,
}

/// `UnitPlaceholderEntityData`: a unit stored compressed, rebuilt from its blueprint by
/// `BecomeUnit` when revealed (not ported).
#[derive(Debug, Clone, PartialEq)]
pub struct Placeholder {
    pub blueprint: Option<Guid>,
    pub position: [f32; 3],
    /// `m_IsFinallyDead` (the built unit is dead).
    pub finally_dead: bool,
}

/// `Player` state the narrative systems read and write.
#[derive(Debug, Default)]
pub struct Player {
    /// `Player.Stalker` (a unit blueprint).
    pub stalker: Option<Guid>,
    /// `InspectUnitsManager.m_UnitInfos`, in order.
    pub inspect_infos: Vec<crate::combat::inspect::UnitInfo>,
    /// `UnlockableFlagsManager.m_UnlockedFlags`.
    pub flags: CsDict<Guid, i32>,
    /// `EtudesSystem.m_EtudesData`.
    pub etudes_data: CsDict<Guid, EtudeState>,
    /// The `EtudesSystem` entity (its facts are the `Etude`s).
    pub etudes_system: Option<EntityIdx>,
    /// `EtudesTree.m_Roots`.
    pub etude_roots: Vec<FactIdx>,
    /// `EtudesSystem.ConditionsDirty`.
    pub conditions_dirty: bool,
    /// `m_UpdateGuard`.
    pub etude_update_guard: bool,
    /// `EtudesTree.m_ForceStopOnUpdate`.
    pub etude_force_stop: CsDict<FactIdx, ()>,
    /// `m_HeldConflictingGroups`: group → etude.
    pub held_conflicting_groups: CsDict<Guid, Guid>,
    /// The `QuestBook` entity.
    pub quest_book: Option<EntityIdx>,
    /// `settings.game.tutorial.*` from the save's `SettingsList`.
    pub tutorial_settings: HashMap<String, bool>,
    /// The `TutorialSystem` entity, `m_ShowIndex`.
    pub tutorial: Option<EntityIdx>,
    pub tutorial_show_index: i32,
    /// `Player.Dialog`.
    pub dialog: DialogState,
    /// `PartyCharacters` (unit references, in order).
    pub party_characters: Vec<EntityIdx>,
    /// `CrossSceneState.AllEntityData` units, in order.
    pub cross_scene_units: Vec<EntityIdx>,
    /// The loaded area's main-state units, in order.
    pub area_units: Vec<EntityIdx>,
    /// `Units.All` appends a unit when it joins a state (`EntityPool.HandleAdded`): the units
    /// that joined one after the load, in that order (the load's are in their states' order).
    pub joined_after_load: Vec<EntityIdx>,
    /// `EntityCreationController.m_ToCreate`: units built but not yet in a state (the flag:
    /// the cross-scene state, else the loaded area's main state).
    pub creation_queue: Vec<(EntityIdx, bool)>,
    /// Entities marked for destroy (`EntityDataBase.MarkForDestroy`), in order.
    pub destroy_marks: Vec<EntityIdx>,
    /// `Player.MythicExperience`.
    pub mythic_experience: i32,
    /// `m_MaxGameUniqueId`.
    pub max_unique_id: u64,
    pub experience_rate_percent: i32,
    pub money: i64,
    /// `CapitalPartyMode` (the loaded area's setting; false in the prologue).
    pub capital_party_mode: bool,
    pub area: Option<Guid>,
    pub area_part: Option<Guid>,
    /// `StartPreset` (its `Campaign` is `Player.Campaign`).
    pub start_preset: Option<Guid>,
    /// Unit spawners of the loaded area by `UniqueId`.
    pub spawners: HashMap<String, Spawner>,
    /// Placeholders of the loaded area by unit id (`m_Id`).
    pub placeholders: HashMap<String, Placeholder>,
    /// `Player.GameTime` is on the clock; this is `Game.Instance.CurrentMode` as a name.
    pub mode: &'static str,
    /// The modes under the current one on `Game.m_GameModes`' stack, bottom first.
    pub modes_below: Vec<&'static str>,
    /// `Game.CutsceneLock`: `m_Lock`'s count (a `CountingGuard`) and `m_LockChangeScheduled`.
    pub cutscene_lock: (i32, bool),
    /// `CountableFlag`s that etude brackets and the like retain, by owner and name
    /// (`unit:<id>.Passive`, `area.Peaceful`, `player.CraftManager.Disabled`,
    /// `tutorial:<guid>.EnabledByEtude`…). Most aren't saved: brackets re-retain them on
    /// load through `OnResume`.
    pub counters: std::collections::BTreeMap<String, i32>,
    /// `UnitPartInteractions` overrides added by etude brackets: (unit, bracket runtime).
    pub interactions: Vec<(EntityIdx, crate::world::CompRef)>,
    /// The loaded area's `CampingOverride` / `SkipTimeOverride` (the bracket that set it).
    pub area_overrides: HashMap<&'static str, Option<crate::world::CompRef>>,
    /// Area mechanics scenes loaded with the area (`BlueprintAreaMechanics.IsSceneLoadedNow`).
    pub loaded_mechanics: Vec<Guid>,
    /// `Player.Kingdom` is set (`KingdomState.Instance != null`).
    pub kingdom: bool,
    /// `UnitPartFollowUnit`: follower → the leader it was initialized with (`m_Initialized`).
    pub follow_leaders: HashMap<EntityIdx, EntityIdx>,
    /// `UnitPartFollowUnit.m_LeaderRef` (saved): follower → the leader's unique id.
    pub follow_leader_refs: HashMap<EntityIdx, String>,
    /// `Player.ImportedCampaigns`.
    pub imported_campaigns: Vec<Guid>,
}

impl Player {
    /// `Player.GetNewUniqueId()`: `m_MaxGameUniqueId++.ToString("X")`, skipping ids in
    /// use.
    pub fn new_unique_id(w: &mut World) -> String {
        loop {
            let id = format!("{:X}", w.player.max_unique_id);
            w.player.max_unique_id += 1;
            if !w.by_id.contains_key(&id) && !fact_id_in_use(w, &id) {
                if std::env::var_os("WR_DEBUG_IDS").is_some() {
                    trace_id(&id);
                }
                return id;
            }
        }
    }
}

/// `WR_DEBUG_IDS`: each allocated id with the sim's call chain (`IDTRACE <id> <fn@file:line> <
/// …`, on stderr; needs debug line tables), to compare with WrathOracle's `IdTrace`.
fn trace_id(id: &str) {
    let bt = std::backtrace::Backtrace::force_capture().to_string();
    let lines: Vec<&str> = bt.lines().map(str::trim).collect();
    let mut frames: Vec<String> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(at) = l.strip_prefix("at ./crates/")
            && i > 0
        {
            let f = lines[i - 1].split_once(": ").map_or(lines[i - 1], |x| x.1);
            if f != "new_unique_id" && f != "trace_id" {
                frames.push(format!("{f}@{}", at.rsplit('/').next().unwrap_or(at)));
            }
        }
    }
    frames.truncate(16);
    eprintln!("IDTRACE {id} {}", frames.join(" < "));
}

fn fact_id_in_use(w: &World, id: &str) -> bool {
    // `EntityService.GetEntity` looks up entities only; facts have their own ids drawn
    // from the same counter, so a clash with a fact id can't happen in the game either.
    let _ = (w, id);
    false
}

/// An event delivered to an entity subscribed on the bus (its class's handlers): `true` when the
/// sim ran it. `EtudesSystem` (`EtudesSystem.cs`): its handlers for flags, companions, the party,
/// quest objectives and time of day mark its conditions dirty, except
/// `HandleCapitalModeChanged`; of the summon pools' only `HandleLastUnitRemoved` does;
/// `OnAreaDidLoad` does nothing. `UnitEntityData.OnAreaDidLoad` spawns the buffs' particle
/// effects (presentation). `CutscenePlayerData.OnAreaDidLoad` stops or removes a cutscene with
/// `StopCutsceneWhenExitingArea` (not ported). Anything else (an area unloading, an area part
/// changing) isn't ported.
pub fn entity_event(w: &mut World, e: EntityIdx, iface: &str) -> bool {
    let method = w.bus.call.last().map_or("", |c| c.0);
    match (w.entity(e).clr, iface, method) {
        ("EtudesSystem", "ICompanionChangeHandler", "HandleCapitalModeChanged")
        | ("EtudesSystem", "ISummonPoolHandler", "HandleUnitAdded" | "HandleUnitRemoved")
        | ("EtudesSystem", "IAreaHandler", "OnAreaDidLoad") => true,
        (
            "EtudesSystem",
            "IUnlockHandler"
            | "IUnlockValueHandler"
            | "ICompanionChangeHandler"
            | "IPartyHandler"
            | "IQuestObjectiveHandler"
            | "ITimeOfDayChangedHandler",
            _,
        )
        | ("EtudesSystem", "ISummonPoolHandler", "HandleLastUnitRemoved") => {
            w.player.conditions_dirty = true;
            true
        }
        ("UnitEntityData", "IAreaHandler", "OnAreaDidLoad") => {
            w.coverage.stub("UnitEntityData.OnAreaDidLoad: the buffs' particle effects");
            true
        }
        ("CutscenePlayerData", "IAreaHandler", "OnAreaDidLoad") => {
            let stops = w
                .scene
                .cutscenes
                .iter()
                .find(|p| p.entity == e)
                .and_then(|p| p.cutscene)
                .and_then(|g| w.content.get(g))
                .is_some_and(|b| {
                    b.components_array()
                        .iter()
                        .any(|o| crate::content::is_a(o.ty, "StopCutsceneWhenExitingArea"))
                });
            !stops
        }
        _ => false,
    }
}

/// Whether a component type implements a non-subscriber logic interface that the
/// narrative systems call through `CallComponents<T>` (implementers found in the
/// decompiled code: every class declaring the interface).
#[must_use]
pub fn implements(ty: &str, iface: &str) -> bool {
    let list: &[&str] = match iface {
        "IQuestObjectiveLogic" => &[
            "GiveUnlockOnObjectiveTrigger",
            "ChangeObjectiveOnUnlockTrigger",
            "Experience",
            "ObjectiveStatusTrigger",
        ],
        "IQuestLogic" => &["Experience"],
        "IQuestObjectiveCallback" => &["QuestObjectiveCallback"],
        "IEtudeCompleteTrigger" => &["EtudeCompleteTrigger"],
        _ => &[],
    };
    list.iter().any(|t| crate::content::is_a_name(ty, t))
}

/// Hex ids the sim allocated during a run, renamed in order of first appearance when
/// comparing with an oracle run (`docs/oracle/state-dump-format.md`: created ids differ
/// between runs).
#[must_use]
pub fn is_runtime_id(id: &str, first_new: u64) -> bool {
    u64::from_str_radix(id, 16).is_ok_and(|v| v >= first_new)
}

#[cfg(test)]
mod tests;
