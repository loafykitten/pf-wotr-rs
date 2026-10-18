//! Hydrating `Player`'s narrative state (`player.json`) and the narrative members of
//! units and the area: flags, the etude system, the quest book, the tutorial system,
//! dialogue history, party, unique-id counter, unit positions, companion states,
//! alignment, spawners and placeholders. Each saved member maps to what the C# class
//! deserializes into (`[JsonProperty]` members of `Player`, `EtudesSystem`, `Etude`,
//! `QuestBook`, `Quest`, `QuestObjective`, `TutorialSystem`, `Tutorial`, `DialogState`,
//! `UnlockableFlagsManager`, `UnitAlignment`, `UnitPartCompanion`, `UnitSpawnerBase.MyData`,
//! `UnitPlaceholderEntityData`).

use wr_save::value::Value;

use super::Loader;
use super::graph::{O, guid_of, int_of};
use crate::csharp::arith::store_f32;
use crate::narrative::alignment::{HistoryRecord, UnitAlignment};
use crate::narrative::{EtudeState, Placeholder, Spawner};
use crate::world::{Entity, EntityIdx, EntityKind, FactExt, FactIdx, FactsManager};

/// `x|y|z` hundredths → floats (`Vector3Converter`: `int / 100f`).
pub(super) fn vec_of(v: Option<&Value>) -> Option<Vec<f32>> {
    match v? {
        Value::Cm(c) => Some(c.iter().map(|x| store_f32(f64::from(*x) / 100.0)).collect()),
        Value::Floats(f) => Some(f.to_vec()),
        _ => None,
    }
}

impl Loader<'_> {
    /// A narrative system entity (`EtudesSystem`, `QuestBook`, `TutorialSystem`) with its
    /// facts.
    pub(super) fn system_entity(&mut self, o: O<'_>, clr: &'static str) -> EntityIdx {
        let e = self.w.add_entity(Entity {
            unique_id: o.str("UniqueId").unwrap_or_default().to_owned(),
            clr,
            kind: EntityKind::Other,
            facts: FactsManager::default(),
            parts: Vec::new(),
            is_turned_on: false,
            post_load_executed: false,
            is_in_game: true,
            subscribed: false,
            disposing: false,
            is_disposed: false,
        });
        let facts = o.obj("Facts").map(|f| f.objs("m_Facts")).unwrap_or_default();
        let mut list = Vec::new();
        for fo in facts {
            let f = self.fact_for(fo);
            self.narrative_fact_ext(f, fo);
            list.push(f);
        }
        let cap = crate::csharp::sort::grown_capacity(list.len());
        self.w.entity_mut(e).facts = FactsManager { list, capacity: cap, is_turned_on: false };
        e
    }

    /// The members of `Etude`, `Quest`, `QuestObjective`, `Tutorial`.
    fn narrative_fact_ext(&mut self, f: FactIdx, o: O<'_>) {
        let ext = match o.ty() {
            "Etude" => FactExt::Etude {
                is_completed: o.bool_or("IsCompleted", false),
                completion_in_progress: o.bool_or("CompletionInProgress", false),
                children: Vec::new(),
            },
            "Quest" => {
                let mut objectives = Vec::new();
                for oo in o.objs("PersistentObjectives") {
                    let of = self.fact_for(oo);
                    self.objective_ext(of, oo, f);
                    if let Some(g) = self.w.fact(of).blueprint {
                        objectives.push((g, of));
                    }
                }
                FactExt::Quest {
                    state: o.i32_or("m_State", 0),
                    next_objective_order: o.i32_or("m_NextObjectiveOrder", 1),
                    is_read_only: o.bool_or("m_IsReadOnly", false),
                    is_in_ui_selected: o.bool_or("m_IsInUiSelected", false),
                    objectives,
                }
            }
            "QuestObjective" => return,
            "Tutorial" => FactExt::Tutorial {
                triggered_times: o.i32_or("TriggeredTimes", 0),
                showed_times: o.i32_or("ShowedTimes", 0),
                last_show_index: o.i32_or("LastShowIndex", 0),
                trigger_logic_counter: o.i32_or("TriggerLogicCounter", 0),
                banned: o.bool_or("Banned", false),
            },
            _ => return,
        };
        self.w.fact_mut(f).ext = ext;
        self.w.fact_mut(f).kind = match o.ty() {
            "Etude" => crate::world::FactKind::Etude,
            _ => crate::world::FactKind::Other,
        };
    }

    fn objective_ext(&mut self, f: FactIdx, o: O<'_>, quest: FactIdx) {
        self.w.fact_mut(f).ext = FactExt::Objective {
            state: o.i32_or("m_State", 0),
            is_visible: o.bool_or("m_IsVisible", false),
            is_collapse: o.bool_or("m_IsCollapse", false),
            start_time: o.timespan("m_ObjectiveStartTime").unwrap_or(0),
            need_to_attention: o.bool_or("m_NeedToAttention", false),
            order: o.i32_or("Order", 0),
            quest: Some(quest),
            parent_objective: None,
        };
    }

    /// `player.json`'s narrative members.
    pub(super) fn player_narrative(&mut self, p: O<'_>) {
        let pl = &mut self.w.player;
        pl.max_unique_id =
            p.int("m_MaxGameUniqueId").and_then(|v| u64::try_from(v).ok()).unwrap_or(0);
        pl.experience_rate_percent = p.i32_or("ExperienceRatePercent", 100);
        pl.money = p.int("Money").unwrap_or(0);
        pl.mythic_experience = p.i32_or("MythicExperience", 0);
        pl.start_preset = p.guid("StartPreset");
        pl.area = p.guid("CurrentArea");
        pl.area_part = p.guid("CurrentAreaPart");
        pl.kingdom = p.obj("Kingdom").is_some();
        // `ItemsCollection.HasLoot` of the three shared stashes (items not hydrated).
        for (k, name) in ["SharedStash", "SecondSharedStash", "ThirdSharedStash"].iter().enumerate()
        {
            let has = p.obj(name).is_some_and(|s| {
                s.objs("m_Items").iter().any(|it| !it.bool_or("m_NotLootable", false))
            });
            self.w.scene.stashes.has_loot[k] = has;
        }
        pl.imported_campaigns = match p.get("ImportedCampaigns") {
            Some(Value::Seq(v)) => v.iter().filter_map(guid_of).collect(),
            _ => Vec::new(),
        };
        pl.stalker = p.guid("Stalker");
        if let Some(im) = p.obj("InspectUnitsManager") {
            for o in im.objs("m_UnitInfos") {
                let Some(blueprint) = o.guid("Blueprint") else { continue };
                pl.inspect_infos.push(crate::combat::inspect::UnitInfo {
                    blueprint,
                    parts: o.str("m_Parts").unwrap_or("None").to_owned(),
                    check_value: o
                        .get("m_CheckValue")
                        .and_then(int_of)
                        .and_then(|n| i32::try_from(n).ok())
                        .unwrap_or(0),
                });
            }
        }
        if let Some(fl) = p.obj("m_UnlockableFlags") {
            for (k, v) in fl.dict("m_UnlockedFlags") {
                if let (Some(g), Some(n)) = (guid_of(k), int_of(v)) {
                    self.w.player.flags.insert(g, i32::try_from(n).unwrap_or(0));
                }
            }
        }
        if let Some(d) = p.obj("m_Dialog") {
            let set = |name: &str| -> Vec<wr_content::Guid> {
                match d.get(name) {
                    Some(Value::Seq(v)) => v.iter().filter_map(guid_of).collect(),
                    _ => Vec::new(),
                }
            };
            let ds = &mut self.w.player.dialog;
            for g in set("SelectedAnswers") {
                ds.selected_answers.insert(g, ());
            }
            for g in set("ShownAnswerLists") {
                ds.shown_answer_lists.insert(g, ());
            }
            for g in set("ShownCues") {
                ds.shown_cues.insert(g, ());
            }
            for g in set("ShownDialogs") {
                ds.shown_dialogs.insert(g, ());
            }
            for (k, v) in d.dict("AnswerChecks") {
                if let (Some(g), Some(n)) = (guid_of(k), int_of(v)) {
                    ds.answer_checks.insert(g, i32::try_from(n).unwrap_or(0));
                }
            }
            if d.obj("Scheduled").is_some() {
                self.w
                    .coverage
                    .unsupported("DialogState.Scheduled: a dialogue scheduled in the save");
            }
        }
        if let Some(es) = p.obj("EtudesSystem") {
            for (k, v) in es.dict("m_EtudesData") {
                let state = match v {
                    Value::Enum { value, .. } => i32::try_from(*value).ok(),
                    other => int_of(other).and_then(|x| i32::try_from(x).ok()),
                };
                let st = match state {
                    Some(1) => EtudeState::Started,
                    Some(2) => EtudeState::Completed,
                    Some(3) => EtudeState::PreStarted,
                    Some(4) => EtudeState::PreCompleted,
                    _ => EtudeState::Unknown,
                };
                if let Some(g) = guid_of(k) {
                    self.w.player.etudes_data.insert(g, st);
                }
            }
            let e = self.system_entity(es, "EtudesSystem");
            self.w.player.etudes_system = Some(e);
        }
        if let Some(qb) = p.obj("m_QuestBook") {
            let e = self.system_entity(qb, "QuestBook");
            self.w.player.quest_book = Some(e);
        }
        if let Some(t) = p.obj("Tutorial") {
            self.w.player.tutorial_show_index = t.i32_or("m_ShowIndex", 0);
            let e = self.system_entity(t, "TutorialSystem");
            self.w.player.tutorial = Some(e);
        }
    }

    /// A unit's narrative members (position, orientation, companion state, alignment,
    /// life state, group).
    pub(super) fn unit_narrative(&mut self, e: EntityIdx, o: O<'_>) {
        let pos = vec_of(o.get("Position")).unwrap_or_default();
        let orientation = match o.get("m_Orientation") {
            Some(Value::F32(f)) => *f,
            #[allow(clippy::cast_possible_truncation)]
            Some(Value::F64(f)) => *f as f32,
            _ => 0.0,
        };
        let companion_part = o
            .obj("Parts")
            .map(|p| p.objs("m_Parts"))
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.ty() == "UnitPartCompanion");
        let companion = companion_part.map(|p| p.i32_or("State", 0));
        // `UnitPartCompanion.m_Spawner` (an `EntityRef`: the spawner data's id) and
        // `IsIgnoreFixInGame`.
        let companion_spawner = companion_part
            .and_then(|p| p.str("m_Spawner"))
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        let ignore_fix_in_game =
            companion_part.is_some_and(|p| p.bool_or("IsIgnoreFixInGame", false));
        let spawn_pos = vec_of(o.get("SpawnPosition")).unwrap_or_default();
        let parts = o.obj("Parts").map(|p| p.objs("m_Parts")).unwrap_or_default();
        let pet_type = parts.iter().find(|p| p.ty() == "UnitPartPet").map(|p| p.i32_or("Type", 0));
        let pet_refs: Vec<String> = parts
            .iter()
            .find(|p| p.ty() == "UnitPartPetMaster")
            .map(|p| {
                p.objs("m_Pets")
                    .into_iter()
                    .filter_map(|r| r.str("m_Ref").map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        // `UnitPartFollowUnit.m_LeaderRef`: `OnTurnOn` initializes the following from it.
        let leader_ref = o
            .obj("Parts")
            .map(|p| p.objs("m_Parts"))
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.ty() == "UnitPartFollowUnit")
            .and_then(|p| p.str("m_LeaderRef"))
            .map(str::to_owned);
        if let Some(l) = leader_ref {
            self.w.player.follow_leader_refs.insert(e, l);
        }
        // `UnitPartDamageGrace.Weapons` (entry categories), if the unit has the part.
        let damage_grace: Option<Vec<i64>> = o
            .obj("Parts")
            .map(|p| p.objs("m_Parts"))
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.ty() == "UnitPartDamageGrace")
            .map(|p| p.objs("Weapons").iter().filter_map(|e| e.int("Category")).collect());
        let d = o.obj("Descriptor");
        let alignment = d.and_then(|d| d.obj("Alignment")).map(|a| {
            let v = vec_of(a.get("m_Vector")).unwrap_or_else(|| vec![0.0, 0.0]);
            let history = a.get("m_History").map(|_| {
                a.objs("m_History")
                    .into_iter()
                    .map(|h| {
                        let p = vec_of(h.get("Position")).unwrap_or_else(|| vec![0.0, 0.0]);
                        HistoryRecord {
                            position: [
                                p.first().copied().unwrap_or(0.0),
                                p.get(1).copied().unwrap_or(0.0),
                            ],
                            direction: h.i32_or("Direction", 4),
                            provider: h.guid("Provider"),
                        }
                    })
                    .collect()
            });
            UnitAlignment {
                vector: [v.first().copied().unwrap_or(0.0), v.get(1).copied().unwrap_or(0.0)],
                locked_mask: a.i32_or("m_LockedAlignmentMask", 0),
                target_alignment: a.int("m_TargetAlignment").and_then(|x| i32::try_from(x).ok()),
                history,
                undetectable: a.i32_or("Undetectable", 0),
            }
        });
        let life = d.and_then(|d| d.obj("State")).map_or(0, |s| s.i32_or("LifeState", 0));
        let finally_dead =
            d.and_then(|d| d.obj("State")).is_some_and(|s| s.bool_or("IsFinallyDead", false));
        // `UnitState`'s saved status effects (by condition) and `Prone.Active`; the
        // condition counters are read with the unit (`unit_state::hydrate`).
        let state = d.and_then(|d| d.obj("State"));
        let cond_enum =
            wr_content::schema::enums().iter().find(|e| e.name.ends_with(".UnitCondition"));
        let mut status_effects = Vec::new();
        let mut unknown_effect = false;
        for (k, _) in state.map(|s| s.dict("m_StatusEffects")).unwrap_or_default() {
            let c = int_of(k).or_else(|| match k {
                Value::Str(n) => cond_enum.and_then(|e| e.parse(n)),
                _ => None,
            });
            match c {
                Some(c) => {
                    status_effects.push(i32::try_from(c).unwrap_or(0));
                }
                None => unknown_effect = true,
            }
        }
        if unknown_effect {
            self.w.coverage.unsupported("UnitState.m_StatusEffects: unknown condition key");
        }
        // A saved status effect's own state (its buff, modifiers, lifecycle) isn't hydrated.
        for c in &status_effects {
            self.w
                .coverage
                .unsupported(format!("UnitState: saved status effect ({c}) not hydrated"));
        }
        // `ProneState` (`Active`, `ShouldBeActive`, `Duration`: the defaults are not written).
        let prone_state = state.and_then(|s| s.obj("Prone"));
        let prone = prone_state.is_some_and(|p| p.bool_or("Active", false));
        let prone_should_be_active =
            prone_state.is_some_and(|p| p.bool_or("ShouldBeActive", false));
        let prone_duration = prone_state.and_then(|p| p.timespan("Duration")).unwrap_or(0);
        // The controller runs only in the driven combat replay, so a saved posture is still
        // reported for any other run.
        if prone_state.is_some_and(|p| {
            p.bool_or("Active", false)
                || p.bool_or("ShouldBeActive", false)
                || p.get("Duration").is_some()
        }) {
            self.w.coverage.unsupported(
                "UnitState.Prone: saved posture (the prone controller runs only in the driven combat replay)",
            );
        }
        let group = o.str("GroupId").map(str::to_owned);
        // `UnitState.IsInStealth`, `UnitEntityData.Stealth` (`InStealthFor`, the stealth controller's
        // entry flags) and the blindsense part (`UnitPartBlindsense`, which `UnitMemoryController`
        // asks `Reach`).
        let stealth = saved_stealth(o);
        if stealth.bad_spotters > 0 {
            self.w.coverage.unsupported("UnitStealth.m_SpottedBy: an entry that isn't a unit id");
        }
        let has_blindsense = parts.iter().any(|p| p.ty() == "UnitPartBlindsense");
        let marked_for_death = state.is_some_and(|s| s.bool_or("MarkedForDeath", false));
        let force_kill = state.is_some_and(|s| s.bool_or("ForceKill", false));
        let no_xp_on_death = !o.bool_or("GiveExperienceOnDeath", true);
        let is_extra = o.bool_or("IsExtra", false);
        if o.bool_or("HasDummyView", false) {
            self.w.combat.vision.dummy.insert(e, Some(true));
        }
        let custom_prefab =
            d.and_then(|d| d.str("CustomPrefabGuid")).filter(|s| !s.is_empty()).map(str::to_owned);
        if let Some(u) = self.w.unit_mut(e) {
            u.is_extra = is_extra;
            u.custom_prefab_guid = custom_prefab;
            if pos.len() == 3 {
                u.position = [pos[0], pos[1], pos[2]];
            }
            if spawn_pos.len() == 3 {
                u.spawn_position = [spawn_pos[0], spawn_pos[1], spawn_pos[2]];
            }
            u.companion_spawner = companion_spawner;
            u.pet_type = pet_type;
            u.pet_refs = pet_refs;
            u.ignore_fix_in_game = ignore_fix_in_game;
            u.last_rest_time = d.and_then(|d| d.timespan("LastRestTime"));
            u.orientation = orientation;
            u.companion_state = companion;
            u.alignment = alignment;
            u.life_state = life;
            u.finally_dead = finally_dead;
            u.state.panic_possible |= state.is_some_and(|s| s.bool_or("IsPanicked", false));
            u.in_stealth = stealth.in_stealth;
            u.stealth_spotted_by = stealth.spotted_by;
            u.has_blindsense = has_blindsense;
            u.stealth_pending = stealth.pending;
            u.marked_for_death = marked_for_death;
            u.force_kill = force_kill;
            u.no_xp_on_death = no_xp_on_death;
            u.damage_grace = damage_grace;
            u.is_extra = is_extra;
            for c in status_effects {
                u.state.status_effects.insert(c, crate::unit_state::StatusEffect::Saved);
            }
            u.prone_active = prone;
            u.prone_should_be_active = prone_should_be_active;
            u.prone_duration = prone_duration;
            u.group_id = group;
        }
    }

    /// The area's spawners and placeholders.
    pub(super) fn area_narrative(&mut self, main: O<'_>) {
        for o in main.objs("m_EntityData") {
            let companion = o.clr().contains("CompanionSpawner+MyData");
            if o.clr().contains("UnitSpawnerBase+MyData") || companion {
                let id = o.str("UniqueId").unwrap_or_default().to_owned();
                let parts = o
                    .obj("Parts")
                    .map(|p| p.objs("m_Parts"))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| p.clr().to_owned())
                    .collect();
                self.w.player.spawners.insert(
                    id,
                    Spawner {
                        spawned_unit: o.str("m_SpawnedUnit").map(str::to_owned),
                        has_spawned: o.bool_or("HasSpawned", false),
                        companion,
                        did_spawn_once: o.bool_or("DidSpawnOnce", false),
                        need_init_on_place: false,
                        in_game: o.bool_or("m_IsInGame", true),
                        parts,
                    },
                );
            } else if o.ty() == "UnitPlaceholderEntityData" {
                let id = o.str("m_Id").unwrap_or_default().to_owned();
                let p = vec_of(o.get("m_Position")).unwrap_or_default();
                let in_game = o.bool_or("m_IsInGame", true);
                // `BecomeUnit` (at the placeholder's `OnPostLoad`) spawns the unit under the
                // same id; the sim doesn't build it, but keeps an entity under that id so
                // dialogue speakers can name it (`DialogSpeaker` reads the blueprint,
                // position and state only).
                if !self.w.by_id.contains_key(&id) {
                    self.w.add_entity(Entity {
                        unique_id: id.clone(),
                        clr: "UnitPlaceholderEntityData",
                        kind: EntityKind::Other,
                        facts: FactsManager::default(),
                        parts: Vec::new(),
                        is_turned_on: false,
                        post_load_executed: false,
                        is_in_game: in_game,
                        subscribed: false,
                        disposing: false,
                        is_disposed: false,
                    });
                }
                self.w.player.placeholders.insert(
                    id,
                    Placeholder {
                        blueprint: o.guid("m_Blueprint"),
                        finally_dead: o.bool_or("m_IsFinallyDead", false),
                        position: [
                            p.first().copied().unwrap_or(0.0),
                            p.get(1).copied().unwrap_or(0.0),
                            p.get(2).copied().unwrap_or(0.0),
                        ],
                    },
                );
            }
        }
    }
}

/// A unit's saved stealth state (`UnitState.IsInStealth` under `Descriptor.State`, and the unit's
/// `Stealth` property: `UnitEntityData.Stealth`, not a placeholder's `m_Stealth`).
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct SavedStealth {
    pub in_stealth: bool,
    /// `UnitStealth.m_SpottedBy`: the spotters' unique ids, in order.
    pub spotted_by: Vec<String>,
    /// Entries of the list that aren't ids.
    pub bad_spotters: usize,
    /// `WantEnterStealth`, `InAmbush`, `ForceEnterStealth` or `ShouldExitStealth` is set.
    pub pending: bool,
}

/// The stealth state of a saved `UnitEntityData`.
pub(super) fn saved_stealth(o: O<'_>) -> SavedStealth {
    let mut out = SavedStealth {
        in_stealth: o
            .obj("Descriptor")
            .and_then(|d| d.obj("State"))
            .is_some_and(|s| s.bool_or("IsInStealth", false)),
        ..SavedStealth::default()
    };
    let Some(stealth) = o.obj("Stealth") else { return out };
    if let Some(Value::Seq(items)) = stealth.get("m_SpottedBy") {
        for v in items {
            match super::graph::str_of(v) {
                Some(id) => out.spotted_by.push(id.to_owned()),
                None => out.bad_spotters += 1,
            }
        }
    }
    out.pending = ["WantEnterStealth", "InAmbush", "ForceEnterStealth", "ShouldExitStealth"]
        .iter()
        .any(|k| stealth.bool_or(k, false));
    out
}

/// `settings.game.tutorial.*` from the save's settings list.
pub(super) fn tutorial_settings(
    list: &serde_json::Map<String, serde_json::Value>,
) -> std::collections::HashMap<String, bool> {
    list.iter()
        .filter_map(|(k, v)| {
            let key = k.strip_prefix("settings.game.tutorial.")?;
            Some((key.to_owned(), v.as_bool()?))
        })
        .collect()
}
