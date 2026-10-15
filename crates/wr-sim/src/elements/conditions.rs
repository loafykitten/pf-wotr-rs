//! Conditions (each cites its decompiled file under
//! `Kingmaker.Designers.EventConditionActionSystem.Conditions/`).

use std::rc::Rc;

use super::{ElementError, Result, get_int, try_get_unit, unsupported};
use crate::content::{Blueprint, View};
use crate::narrative::etudes::{self, EtudeStatus as S};
use crate::narrative::flags;
use crate::world::World;

pub const IMPLEMENTED: &[&str] = &[
    "EtudeStatus",
    "PcRace",
    "PlayerSignificantClassIs",
    "AnswerSelected",
    "UnitEqual",
    "CompanionInParty",
    "CueSeen",
    "DialogSeen",
    "False",
    "FlagInRange",
    "FlagUnlocked",
    "IsDLCEnabled",
    "LessThan",
    "ObjectiveStatus",
    "OrAndLogic",
    "PcFemale",
    "QuestStatus",
    "UnitArchetypes",
    "UnitClass",
    "KingdomStatCheck",
    "CompanionIsDead",
    "HasFact",
    "IsCampaign",
    "IsEqual",
    "PartyUnits",
    "UnitIsDead",
    "IsCampaignImported",
    "PlayerTopClassIs",
];

/// `Condition.CheckCondition()` (before `Not`).
pub fn check_condition(w: &mut World, owner: &Rc<Blueprint>, c: View<'_>) -> Result<bool> {
    match c.type_name() {
        // `ContextConditionCasterHasFact.cs`: the context's caster (`MaybeCaster`) has the fact.
        // A `ContextCondition` reads the innermost `MechanicsContext.Data` scope.
        "ContextConditionCasterHasFact" => {
            let Some((fact, _)) = super::actions::mechanics_scope(w) else {
                return Err(unsupported(
                    w,
                    "condition",
                    "ContextConditionCasterHasFact (no context)",
                ));
            };
            let caster = w
                .fact(fact)
                .context
                .as_ref()
                .and_then(|x| x.caster.clone())
                .and_then(|id| w.by_id.get(&id).copied());
            let Some(u) = caster else {
                w.log("error", "Caster is missing");
                return Ok(false);
            };
            let g = c.guid("m_Fact");
            Ok(g.is_some_and(|g| {
                w.entity(u).facts.list.iter().any(|f| w.fact(*f).blueprint == Some(g))
            }))
        }
        // `EtudeStatus.cs`.
        "EtudeStatus" => {
            let Some(g) = c.guid("m_Etude") else {
                return Err(ElementError::Other("NullReferenceException: etude".into()));
            };
            Ok(match etudes::status(w, g) {
                S::NotStarted => c.bool("NotStarted"),
                S::Completed => c.bool("Completed"),
                S::Started => c.bool("Started"),
                S::CompletionInProgress => c.bool("CompletionInProgress") || c.bool("Started"),
                S::Playing => c.bool("Playing") || c.bool("Started"),
            })
        }
        // `PcRace.cs`: `Race == MainCharacter?.Progression.Race?.RaceId`.
        "PcRace" => {
            let race = w
                .main_character
                .and_then(|mc| w.unit(mc).and_then(|u| u.progression.race))
                .and_then(|g| w.content.get(g))
                .map(|b| b.view().int("RaceId"));
            Ok(race == Some(c.int("Race")))
        }
        // `PlayerSignificantClassIs.cs` → `UnitProgressionData.GetSignificantClasses()`:
        // classes with level ≥ (max + 1) / 2, or prestige classes.
        "PlayerSignificantClassIs" => {
            if c.bool("CheckGroup") {
                return Err(unsupported(w, "condition", "PlayerSignificantClassIs (group)"));
            }
            let Some(class) = c.guid("m_CharacterClass") else {
                w.log("error", "Character class is not set.");
                return Ok(false);
            };
            let Some(mc) = w.main_character else {
                return Err(ElementError::Other("NullReferenceException: main character".into()));
            };
            let classes = w.unit(mc).map(|u| u.progression.classes.clone()).unwrap_or_default();
            // `Classes.Max(...)` throws on an empty list.
            let Some(max) = classes.iter().map(|c| c.level).max() else {
                return Err(ElementError::Other("InvalidOperationException: no classes".into()));
            };
            let threshold = crate::csharp::arith::add(max, 1) / 2;
            Ok(classes.iter().any(|cd| {
                let prestige =
                    w.content.get(cd.class).is_some_and(|b| b.view().bool("PrestigeClass"));
                (cd.level >= threshold || prestige) && cd.class == class
            }))
        }
        // `AnswerSelected.cs`.
        "AnswerSelected" => {
            let Some(a) = c.guid("m_Answer") else { return Ok(false) };
            Ok(if c.bool("CurrentDialog") {
                w.dialog.local_selected_answers.contains_key(&a)
            } else {
                w.player.dialog.selected_answers.contains_key(&a)
            })
        }
        // `UnitEqual.cs`: both `TryGetValue`s must succeed.
        "UnitEqual" => {
            let a = try_get_unit(w, owner, c.obj("FirstUnit"))?;
            let b = try_get_unit(w, owner, c.obj("SecondUnit"))?;
            Ok(match (a, b) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            })
        }
        // `CompanionInParty.cs`: some character of `AllCharacters` with that blueprint (or
        // whose blueprint's `PrototypeLink` is it) in an allowed state.
        "CompanionInParty" => {
            let Some(comp) = c.guid("m_companion") else { return Ok(false) };
            let lists = crate::narrative::party::character_lists(w);
            for u in lists.all_characters {
                let Some(unit) = w.unit(u) else { continue };
                let bp = unit.blueprint;
                let proto =
                    bp.and_then(|g| w.content.get(g)).and_then(|b| b.view().guid("PrototypeLink"));
                if bp != Some(comp) && proto != Some(comp) {
                    continue;
                }
                let state = unit.companion_state.unwrap_or(0);
                let dead = unit.finally_dead;
                if (c.bool("MatchWhenActive") || state != 2)
                    && (c.bool("MatchWhenRemote") || state != 1)
                    && (c.bool("MatchWhenDetached") || state != 4)
                    && (c.bool("MatchWhenDead") || !dead)
                    && (c.bool("MatchWhenEx") || state != 3)
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        // `CueSeen.cs`.
        "CueSeen" => {
            let Some(cue) = c.guid("m_Cue") else { return Ok(false) };
            Ok(if c.bool("CurrentDialog") {
                w.dialog.local_shown_cues.contains_key(&cue)
            } else {
                w.player.dialog.shown_cues.contains_key(&cue)
            })
        }
        // `DialogSeen.cs` (`Kingmaker.Assets.Designers…`).
        "DialogSeen" => {
            let Some(d) = c.guid("m_Dialog") else { return Ok(false) };
            Ok(w.player.dialog.shown_dialogs.contains_key(&d))
        }
        // `False.cs`.
        "False" => Ok(false),
        // `FlagInRange.cs`.
        "FlagInRange" => {
            let Some(f) = c.guid("m_Flag") else {
                w.log("error", "Flag in FlagInRange is NULL");
                return Ok(false);
            };
            if !flags::is_unlocked(w, f) {
                return Ok(false);
            }
            let v = flags::value(w, f);
            Ok(v >= c.i32("MinValue") && v <= c.i32("MaxValue"))
        }
        // `FlagUnlocked.cs`.
        "FlagUnlocked" => {
            let Some(f) = c.guid("m_ConditionFlag") else {
                w.log("error", "Flag in FlagUnlocked is NULL");
                return Ok(false);
            };
            if !flags::is_unlocked(w, f) {
                return Ok(false);
            }
            let values: Vec<i64> = c.seq("SpecifiedValues").iter().filter_map(node_int).collect();
            if values.is_empty() {
                return Ok(true);
            }
            let except = c.bool("ExceptSpecifiedValues");
            let v = i64::from(flags::value(w, f));
            Ok(if values.contains(&v) { !except } else { except })
        }
        // `IsDLCEnabled.cs` → `BlueprintDlcReward.IsAvailable` (any of its DLCs purchased
        // and mounted): platform state, given per reward in `Settings::dlc_rewards`.
        "IsDLCEnabled" => {
            let Some(reward) = c.guid("m_BlueprintDlcReward") else { return Ok(false) };
            match w.settings.dlc_rewards.as_ref().and_then(|m| m.get(&reward)).copied() {
                Some(v) => Ok(v),
                None => {
                    Err(unsupported(w, "condition", "IsDLCEnabled (DLC reward status unknown)"))
                }
            }
        }
        // `LessThan.cs`.
        "LessThan" => {
            if c.bool("FloatValues") {
                return Err(unsupported(w, "condition", "LessThan (float evaluators)"));
            }
            let (Some(a), Some(b)) = (c.obj("Value"), c.obj("MaxValue")) else {
                return Err(ElementError::Other("NullReferenceException: evaluator".into()));
            };
            let a = get_int(w, owner, a)?;
            let b = get_int(w, owner, b)?;
            Ok(a < b)
        }
        // `ObjectiveStatus.cs`.
        "ObjectiveStatus" => {
            let Some(o) = c.guid("m_QuestObjective") else {
                return Err(ElementError::Other("NullReferenceException: objective".into()));
            };
            Ok(i64::from(crate::narrative::quests::get_objective_state(w, o)) == c.int("State"))
        }
        // `OrAndLogic.cs`.
        "OrAndLogic" => Ok(super::check_conditions(w, owner, c.obj("ConditionsChecker"))),
        // `PcFemale.cs`: `MainCharacter.Gender` = `CustomGender ?? Blueprint.Gender`.
        "PcFemale" => {
            let Some(mc) = w.main_character else {
                return Err(ElementError::Other("NullReferenceException: main character".into()));
            };
            let unit = w.unit(mc);
            let custom = unit.and_then(|u| u.custom_gender);
            let gender = match custom {
                Some(g) => g,
                None => unit
                    .and_then(|u| u.blueprint)
                    .and_then(|g| w.content.get(g))
                    .map_or(0, |b| b.view().int("Gender")),
            };
            Ok(gender == 1)
        }
        // `QuestStatus.cs` → `QuestBook.GetQuestState` (`None` without the quest).
        "QuestStatus" => {
            let Some(q) = c.guid("m_Quest") else { return Ok(c.int("State") == 0) };
            let state = crate::narrative::quests::get_quest(w, q)
                .map_or(0, |f| crate::narrative::quests::quest_state(w, f));
            Ok(i64::from(state) == c.int("State"))
        }
        // `UnitClass.cs` / `UnitArchetypes.cs`.
        "UnitClass" | "UnitArchetypes" => {
            let ev = c.obj("Unit");
            if ev.is_none() {
                return Ok(false);
            }
            let Some(u) = try_get_unit(w, owner, ev)? else { return Ok(false) };
            let classes = w.unit(u).map(|x| x.progression.classes.clone()).unwrap_or_default();
            let cd = if c.type_name() == "UnitClass" {
                let Some(class) = c.guid("m_Class") else { return Ok(false) };
                classes.into_iter().find(|cd| cd.class == class)
            } else {
                let Some(arch) = c.guid("m_Archetype") else { return Ok(false) };
                // `GetParentClass()`: the available class listing the archetype.
                let parent = parent_class(w, arch)?;
                let cd = parent.and_then(|p| classes.into_iter().find(|cd| cd.class == p));
                match cd {
                    Some(cd) if cd.archetypes.contains(&arch) => Some(cd),
                    _ => return Ok(false),
                }
            };
            let Some(cd) = cd else { return Ok(false) };
            // `LimitMinLevel` defaults to true.
            let lim_min = if c.has("LimitMinLevel") { c.bool("LimitMinLevel") } else { true };
            if lim_min && let Some(e) = c.obj("MinLevel") {
                match try_int(w, owner, e)? {
                    Some(v) if cd.level >= v => {}
                    _ => return Ok(false),
                }
            }
            if c.bool("LimitMaxLevel")
                && let Some(e) = c.obj("MaxLevel")
            {
                match try_int(w, owner, e)? {
                    Some(v) if cd.level <= v => {}
                    _ => return Ok(false),
                }
            }
            Ok(true)
        }
        // `Kingmaker.Kingdom.Conditions/KingdomStatCheck.cs`: false without a kingdom.
        "KingdomStatCheck" => {
            if w.player.kingdom {
                return Err(unsupported(w, "condition", "KingdomStatCheck (kingdom state)"));
            }
            Ok(false)
        }
        // `CompanionIsDead.cs`: over `AllCharacters` (with remote) or `PartyAndPets`.
        "CompanionIsDead" => {
            let lists = crate::narrative::party::character_lists(w);
            let units =
                if c.bool("includeRemote") { lists.all_characters } else { lists.party_and_pets };
            let any = c.bool("anyCompanion");
            let comp = c.guid("m_companion");
            for u in units {
                let Some(unit) = w.unit(u) else { continue };
                let bp = unit.blueprint;
                let proto =
                    bp.and_then(|g| w.content.get(g)).and_then(|b| b.view().guid("PrototypeLink"));
                let matches = any || (comp.is_some() && (bp == comp || proto == comp));
                if matches && unit.finally_dead {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        // `HasFact.cs`: `Unit.TryGetValue` then `Descriptor.HasFact(Fact)`.
        "HasFact" => {
            let ev = c.obj("Unit");
            if ev.is_none() {
                return Ok(false);
            }
            let Some(u) = try_get_unit(w, owner, ev)? else { return Ok(false) };
            let Some(f) = c.guid("m_Fact") else { return Ok(false) };
            Ok(w.entity(u).facts.list.iter().any(|x| w.fact(*x).blueprint == Some(f)))
        }
        // `IsCampaign.cs`: `Player.Campaign == BlueprintCampaign`.
        "IsCampaign" => Ok(etudes::player_campaign(w) == c.guid("m_BlueprintCampaign")),
        // `PlayerTopClassIs.cs`: the main character's classes at the top level (`Max`
        // throws on none), against the class or the group's classes.
        "PlayerTopClassIs" => {
            let Some(mc) = w.main_character else {
                return Err(ElementError::Other("NullReferenceException: main character".into()));
            };
            let classes = w.unit(mc).map(|u| u.progression.classes.clone()).unwrap_or_default();
            let Some(max) = classes.iter().map(|c| c.level).max() else {
                return Err(ElementError::Other("InvalidOperationException: no classes".into()));
            };
            let top: Vec<wr_content::Guid> =
                classes.iter().filter(|c| c.level >= max).map(|c| c.class).collect();
            if c.bool("CheckGroup") {
                let Some(g) = c.guid("m_CharacterClassGroup") else {
                    w.log("error", "Character class group is not set.");
                    return Ok(false);
                };
                let group = w
                    .content
                    .get(g)
                    .map(|b| b.view().guids("m_CharacterClasses"))
                    .unwrap_or_default();
                return Ok(top.iter().any(|t| group.contains(t)));
            }
            let Some(class) = c.guid("m_CharacterClass") else {
                w.log("error", "Character class is not set.");
                return Ok(false);
            };
            Ok(top.contains(&class))
        }
        // `IsCampaignImported.cs`.
        "IsCampaignImported" => Ok(c
            .guid("m_BlueprintCampaign")
            .is_some_and(|g| w.player.imported_campaigns.contains(&g))),
        // `IsEqual.cs`.
        "IsEqual" => {
            let (Some(a), Some(b)) = (c.obj("FirstValue"), c.obj("SecondValue")) else {
                return Err(ElementError::Other("NullReferenceException: evaluator".into()));
            };
            let a = get_int(w, owner, a)?;
            let b = get_int(w, owner, b)?;
            Ok(a == b)
        }
        // `PartyUnits.cs` (the condition): each of `PartyAndPets` in a `PartyUnitData` scope.
        "PartyUnits" => {
            let any = c.bool("Any");
            for u in crate::narrative::party::character_lists(w).party_and_pets {
                let ok = super::with_ctx(w, super::Ctx::PartyUnit(u), |w| {
                    super::check_conditions(w, owner, c.obj("Conditions"))
                });
                if any && ok {
                    return Ok(true);
                }
                if !any && !ok {
                    return Ok(false);
                }
            }
            Ok(!any)
        }
        // `UnitIsDead.cs`: `TryGetValue` (a null value counts as dead), `State.IsDead`.
        "UnitIsDead" => {
            let Some(ev) = c.obj("Target") else {
                return Err(ElementError::Other("NullReferenceException: evaluator".into()));
            };
            match try_get_unit(w, owner, Some(ev))? {
                Some(u) => Ok(w.unit(u).is_some_and(|x| x.life_state == 2)),
                None => Ok(false),
            }
        }
        other => Err(unsupported(w, "condition", other)),
    }
}

fn node_int(n: &wr_content::node::Node) -> Option<i64> {
    match n {
        wr_content::node::Node::Int(i) => Some(*i),
        _ => None,
    }
}

/// `IntEvaluator.CanEvaluate() && TryGetValue(out v)`.
fn try_int(w: &mut World, owner: &Rc<Blueprint>, e: View<'_>) -> Result<Option<i32>> {
    match get_int(w, owner, e) {
        Ok(v) => Ok(Some(v)),
        Err(ElementError::FailToEvaluate(_)) => Ok(None),
        Err(x) => Err(x),
    }
}

/// `BlueprintArchetype.GetParentClass()`: the single class of
/// `BlueprintRoot.Progression.AvailableCharacterClasses` (not DLC-restricted) listing it;
/// `SingleOrDefault` throws when there are several.
fn parent_class(w: &mut World, arch: wr_content::Guid) -> Result<Option<wr_content::Guid>> {
    let Some(root) = w.content.get(crate::unit::BLUEPRINT_ROOT) else { return Ok(None) };
    let classes =
        root.view().obj("Progression").map(|p| p.guids("m_CharacterClasses")).unwrap_or_default();
    let mut found = Vec::new();
    for c in classes {
        let Some(b) = w.content.get(c) else { continue };
        if dlc_restricted(w, &b)? {
            continue;
        }
        if b.view().guids("m_Archetypes").contains(&arch) {
            found.push(c);
        }
    }
    match found.len() {
        0 => Ok(None),
        1 => Ok(Some(found[0])),
        _ => Err(ElementError::Other("InvalidOperationException: several parent classes".into())),
    }
}

/// `IsDlcRestricted()` where its exception has no handler of its own (`IFeatureSelection.Items`,
/// spell lists): an unknown reward status is reported by `dlc_restricted`, a null reward (which
/// throws in `DlcCondition.IsFullfilled`) is reported as `what`; either counts as unrestricted.
pub(crate) fn dlc_restricted_or_report(w: &mut World, b: &Rc<Blueprint>, what: &str) -> bool {
    match dlc_restricted(w, b) {
        Ok(r) => r,
        Err(ElementError::Unsupported(_)) => false,
        Err(e) => {
            w.coverage.unsupported(format!("{what}: {e:?}"));
            false
        }
    }
}

/// `DlcConditionExtension.IsDlcRestricted`: a `DlcCondition` component not fulfilled
/// (`m_HideInstead ^ DlcReward.IsAvailable`). The reward's availability is platform state
/// (`Settings::dlc_rewards`); unknown is unsupported.
pub(crate) fn dlc_restricted(w: &mut World, b: &Rc<Blueprint>) -> Result<bool> {
    for o in b.components_array() {
        if !crate::content::is_a(o.ty, "DlcCondition") {
            continue;
        }
        let v = View(o);
        let Some(reward) = v.guid("m_DlcReward") else {
            return Err(ElementError::Other("NullReferenceException: DLC reward".into()));
        };
        let Some(avail) = w.settings.dlc_rewards.as_ref().and_then(|m| m.get(&reward)).copied()
        else {
            return Err(unsupported(w, "condition", "DlcCondition (DLC reward status unknown)"));
        };
        return Ok(!(v.bool("m_HideInstead") ^ avail));
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use wr_content::node::Node;

    use super::*;
    use crate::facts::tests::{feature, obj};

    /// A `DlcCondition` with no reward throws in `DlcCondition.IsFullfilled`; where nothing
    /// handles it (a selection's items, a spell list) the run is reported, not silently
    /// treated as unrestricted.
    #[test]
    fn a_null_dlc_reward_is_reported() {
        let (mut w, _) = crate::stats::tests::world_with_unit();
        let cond = Node::Object(Box::new(obj(
            "Kingmaker.DLC.DlcCondition",
            &[("name", Node::Str("$dlc$".into()))],
        )));
        let bp = feature(&w, 40, vec![cond]);
        assert!(!dlc_restricted_or_report(&mut w, &bp, "IFeatureSelection.Items"));
        assert!(!w.coverage.valid());
    }
}
