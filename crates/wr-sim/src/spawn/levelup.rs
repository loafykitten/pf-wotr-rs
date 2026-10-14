//! Automatic class levels: `AddClassLevels.LevelUp` (decompiled, not copied:
//! `Kingmaker.Blueprints.Classes/AddClassLevels.cs:185–495`) through an instant-commit
//! `LevelUpController` on the unit itself (`Kingmaker.UnitLogic.Class.LevelUp/LevelUpController.cs:247`,
//! `LevelUpState.cs:123–175`) and its actions (`Kingmaker.UnitLogic.Class.LevelUp.Actions/`:
//! `SelectRace`, `SelectRaceStat`, `AddArchetype`, `SelectClass`, `ApplyClassMechanics`,
//! `LevelUpHelper.UpdateProgression`/`AddFeaturesFromProgression`, `SelectFeature`,
//! `ApplySkillPoints`, `SpendSkillPoint`), `UnitProgressionData` (`AddClassLevel`,
//! `SetRace`, `AddArchetype`, `ReapplyFeaturesOnLevelUp`, `Kingmaker.UnitLogic/
//! UnitProgressionData.cs:277, 365, 637, 743`) and `ProgressionData`/`FeatureSelectionData`.
//!
//! The whole path runs under `IgnorePrerequisites`: class prerequisites are skipped,
//! features granted by a progression still check their rank and the prerequisites flagged
//! `CheckInProgression`, and a selection's `CanSelect` skips the prerequisites. Parts not on
//! the paths of the units ported so far report unsupported: spellbooks (the sim keeps no
//! spellbook state), stat-distribution presets, attribute points, higher mythic classes,
//! parametrized features and spell selections.

use std::collections::HashSet;
use std::rc::Rc;

use wr_content::Guid;

use crate::content::{Blueprint, View, is_a};
use crate::feature_param::{self, FeatureParam};
use crate::stats::ops;
use crate::stats::types::StatType;
use crate::world::{CompRef, EntityIdx, FactIdx, FactKind, World};

/// `LevelUpActionPriority`.
mod prio {
    pub const HERITAGE: i32 = 6;
    pub const REPLACE_SPELLBOOK: i32 = 7;
    pub const APPLY_SKILL_POINTS: i32 = 11;
    pub const BACKGROUND: i32 = 13;
    pub const MYTHIC_ABILITY: i32 = 14;
    pub const DEITY: i32 = 15;
    pub const PATH: i32 = 16;
    pub const FEATURES: i32 = 17;
    pub const MYTHIC_ADDITIONAL_PROGRESSIONS: i32 = 20;
}

/// The kinds of `IFeatureSelection`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SelKind {
    /// `BlueprintFeatureSelection`.
    Plain,
    /// `BlueprintParametrizedFeature`.
    Parametrized,
    /// `BlueprintFeatureSelectMythicSpellbook`.
    MythicSpellbook,
}

/// `IFeatureSelectionItem`: a feature and its parameter.
#[derive(Debug, Clone, PartialEq)]
struct Item {
    feature: Guid,
    param: Option<FeatureParam>,
}

/// `FeatureSelectionState`.
#[derive(Debug, Clone)]
struct SelState {
    /// `Parent`: the state of the selection a nested selection came from.
    parent: Option<usize>,
    source: Option<Guid>,
    selection: Guid,
    level: i32,
    selected: Option<Item>,
}

/// `LevelUpState` (the members this path uses).
#[derive(Debug, Default)]
struct State {
    next_character_level: i32,
    next_class_level: i32,
    selected_class: Option<Guid>,
    selections: Vec<SelState>,
    total_skill_points: i32,
    spent_skill_points: i32,
    attribute_points: i32,
    can_select_race: bool,
    can_select_race_stat: bool,
    /// `ContextData<IgnorePrerequisites>` being requested around the level
    /// (`AddClassLevels.cs:210`; the plan unit's levels don't have it, only `SelectClass`
    /// does, `:287`).
    ignore_prerequisites: bool,
}

impl State {
    /// `LevelUpState.AddSelection(parent, source, selection, level)` (states are matched by
    /// position here, so the parent and the per-selection index aren't kept).
    fn add_selection(
        &mut self,
        parent: Option<usize>,
        source: Option<Guid>,
        selection: Guid,
        level: i32,
    ) {
        self.selections.push(SelState { parent, source, selection, level, selected: None });
    }
}

/// A `SelectionEntry` of an `AddClassLevels`.
#[derive(Debug, Clone)]
struct Entry {
    is_parametrized: bool,
    is_mythic_spellbook: bool,
    selection: Option<Guid>,
    features: Vec<Guid>,
    parametrized_feature: Option<Guid>,
    mythic_spellbook_selection: Option<Guid>,
    spellbook: Option<Guid>,
    param_object: Option<Guid>,
    param_spell_school: i64,
    param_weapon_category: i64,
    stat: i64,
}

impl Entry {
    fn read(v: View<'_>) -> Self {
        Self {
            is_parametrized: v.bool("IsParametrizedFeature"),
            is_mythic_spellbook: v.bool("IsFeatureSelectMythicSpellbook"),
            selection: v.guid("m_Selection"),
            features: v.guids("m_Features"),
            parametrized_feature: v.guid("m_ParametrizedFeature"),
            mythic_spellbook_selection: v.guid("m_FeatureSelectMythicSpellbook"),
            spellbook: v.guid("m_Spellbook"),
            param_object: v.guid("m_ParamObject"),
            param_spell_school: v.int("ParamSpellSchool"),
            param_weapon_category: v.int("ParamWeaponCategory"),
            stat: v.int("Stat"),
        }
    }

    /// `SelectionEntry.GetFeatureSelection()`.
    fn feature_selection(&self) -> Option<Guid> {
        if self.is_parametrized {
            self.parametrized_feature
        } else if self.is_mythic_spellbook {
            self.mythic_spellbook_selection
        } else {
            self.selection
        }
    }

    /// `SelectionEntry.IsAllFeaturesSelected(count)`.
    fn all_selected(&self, count: usize) -> bool {
        if !self.is_parametrized && !self.is_mythic_spellbook {
            self.features.len() == count
        } else {
            true
        }
    }
}

/// The `AddClassLevels` settings.
struct Cfg {
    class: Rc<Blueprint>,
    archetypes: Vec<Guid>,
    race_stat: StatType,
    levels_stat: StatType,
    skills: Vec<StatType>,
    entries: Vec<Entry>,
    select_spells: bool,
    memorize_spells: Vec<Guid>,
}

/// `Dictionary<SelectionEntry, HashSet<int>>` by entry position.
type History = Vec<Option<HashSet<usize>>>;

/// `SelectFeature.CalculatePriority(selection)` by the selection's `Group`.
fn priority(group: i64) -> i32 {
    match group {
        24 => prio::DEITY,
        42 | 45 | 80 | 81 | 82 | 91 => prio::HERITAGE,
        79 => prio::BACKGROUND,
        87 => prio::MYTHIC_ADDITIONAL_PROGRESSIONS,
        56 | 57 => prio::MYTHIC_ABILITY,
        // Domain, SpecialistSchool, BloodLine, ChannelEnergy, DruidDomain, BlightDruidDomain,
        // ClericSecondaryDomain, RangerBond, DruidBond, OracleCurse, ShamanSpirit,
        // OracleMystery, WarpriestBlessing, LichSkeletalUpgrade.
        4 | 13 | 15 | 25 | 39 | 40 | 41 | 43 | 44 | 53 | 55 | 58 | 72 | 86 => prio::PATH,
        // ThassilonianSpellbook, ArcaneTricksterSpellbook, EldritchKnightSpellbook,
        // DragonDiscipleSpellbook, MysticTheurgeArcaneSpellbook, SelectMythicSpellbook,
        // HellknightSigniferSpellbook, ReplaceSpellbook.
        30 | 31 | 32 | 33 | 35 | 69 | 75 | 90 => prio::REPLACE_SPELLBOOK,
        _ => prio::FEATURES,
    }
}

/// `FeatureGroup.SelectMythicSpellbook`.
const GROUP_SELECT_MYTHIC_SPELLBOOK: i64 = 69;

/// `StatTypeHelper.Skills`, in order.
const SKILLS: [StatType; 11] = [
    StatType::SkillAthletics,
    StatType::SkillMobility,
    StatType::SkillThievery,
    StatType::SkillStealth,
    StatType::SkillKnowledgeArcana,
    StatType::SkillKnowledgeWorld,
    StatType::SkillLoreNature,
    StatType::SkillLoreReligion,
    StatType::SkillPerception,
    StatType::SkillPersuasion,
    StatType::SkillUseMagicDevice,
];

fn bp(w: &World, g: Option<Guid>) -> Option<Rc<Blueprint>> {
    g.and_then(|g| w.content.get(g))
}

fn selection_kind(b: &Blueprint) -> Option<SelKind> {
    let t = b.obj.ty;
    if is_a(t, "BlueprintFeatureSelection") {
        Some(SelKind::Plain)
    } else if is_a(t, "BlueprintParametrizedFeature") {
        Some(SelKind::Parametrized)
    } else if is_a(t, "BlueprintFeatureSelectMythicSpellbook") {
        Some(SelKind::MythicSpellbook)
    } else {
        None
    }
}

/// `UnitProgressionData.SetRace(race)` (`UnitProgressionData.cs:743`): the old race feature
/// leaves, the new one is added.
pub fn set_race(w: &mut World, u: EntityIdx, race: Guid) {
    let current = w.unit(u).and_then(|x| x.progression.race);
    if current == Some(race) {
        return;
    }
    if let Some(old) = current {
        let f =
            w.entity(u).facts.list.iter().copied().find(|f| {
                w.fact(*f).kind == FactKind::Feature && w.fact(*f).blueprint == Some(old)
            });
        if let Some(f) = f {
            crate::unit_facts::remove(w, u, f);
        }
    }
    w.unit_mut(u).expect("unit").progression.race = Some(race);
    if let Some(b) = bp(w, Some(race)) {
        crate::unit_facts::add_fact(w, u, &b);
    }
    // `View?.ResetSizeScale()`: presentation.
}

/// `AddClassLevels.LevelUp(c, unit, levels, fact)` for the component `c` on unit `u`.
pub fn add_class_levels(w: &mut World, c: CompRef, u: EntityIdx) {
    let Some((class, levels, cfg)) = crate::components::with_settings(w, c, |v| {
        let skills: Vec<StatType> = v
            .seq("Skills")
            .iter()
            .filter_map(|n| match n {
                wr_content::Node::Enum(i) | wr_content::Node::Int(i) => {
                    i32::try_from(*i).ok().map(StatType)
                }
                _ => None,
            })
            .collect();
        (
            v.guid("m_CharacterClass"),
            v.i32("Levels"),
            (
                v.guids("m_Archetypes"),
                StatType(v.i32("RaceStat")),
                StatType(v.i32("LevelsStat")),
                skills,
                v.objs("Selections").into_iter().map(|o| Entry::read(View(o))).collect::<Vec<_>>(),
                !v.seq("m_SelectSpells").is_empty(),
                v.guids("m_MemorizeSpells"),
            ),
        )
    }) else {
        return;
    };
    let Some(class_bp) = bp(w, class) else {
        w.coverage.unsupported("AddClassLevels: class missing");
        return;
    };
    let (archetypes, race_stat, levels_stat, skills, entries, select_spells, memorize_spells) = cfg;
    let cfg = Cfg {
        class: class_bp,
        archetypes,
        race_stat,
        levels_stat,
        skills,
        entries,
        select_spells,
        memorize_spells,
    };
    // `ClassLevelLimit` / `MythicLevelLimit` on the unit blueprint.
    let mythic = cfg.class.view().bool("IsMythic");
    let unit_bp = bp(w, w.unit(u).and_then(|x| x.original_blueprint));
    let limit = unit_bp
        .as_ref()
        .and_then(|b| {
            let ty = if mythic { "MythicLevelLimit" } else { "ClassLevelLimit" };
            b.components_array()
                .into_iter()
                .find(|o| is_a(o.ty, ty))
                .map(|o| View(o).i32("LevelLimit"))
        })
        .unwrap_or(i32::MAX);
    if unit_bp
        .as_ref()
        .is_some_and(|b| b.components_array().iter().any(|o| is_a(o.ty, "ArmyUnitComponent")))
    {
        w.coverage.unsupported("AddClassLevels: army unit (fake levels)");
        return;
    }
    let mut history: History = vec![None; cfg.entries.len()];
    for j in 0..levels {
        // The C# getters store both level caches on their first read
        // (`SetupLevelsIfNecessary`), before the level changes any class data.
        crate::unit::setup_levels_if_necessary(w, u);
        let current = if mythic {
            crate::unit::mythic_level(w, u)
        } else {
            crate::unit::character_level(w, u)
        };
        if current < limit {
            add_level(w, u, &cfg, c, &mut history, true);
            if !mythic && j == 0 {
                configured_deity(w, u, &cfg);
            }
        } else if w.unit(u).is_some_and(|x| x.is_player_faction)
            && !w.build.do_not_create_plan
            && !mythic
        {
            // `unit.Get<LevelUpPlanUnitHolder>()?.RequestPlan()`: the level goes to the
            // plan unit (`AddLevel` on it), whose plan the unit keeps
            // (`Progression.AddLevelPlan`, `SetLevelPlanSkills`: read when the player levels
            // up, not by the sheet).
            if let Some(plan) = super::copy::request_plan(w, u) {
                add_level(w, plan, &cfg, c, &mut history, false);
            }
        }
    }
    reapply_features_on_level_up(w, u);
    prepare_spellbook(w, u, &cfg);
    // `view.UpdateClassEquipment()`: no view yet. `unit.Replenish()`.
    super::replenish(w, u);
}

/// The first-level deity of `AddClassLevels.LevelUp`: when the configured selections name a
/// deity that the selection pass didn't give, it is added to the unit directly.
fn configured_deity(w: &mut World, u: EntityIdx, cfg: &Cfg) {
    let Some(deity_sel) = super::root_ref(w, "Progression", "m_DeitySelection").map(|b| b.guid)
    else {
        return;
    };
    let Some(feature) = cfg
        .entries
        .iter()
        .find(|e| e.selection == Some(deity_sel) && !e.is_parametrized)
        .and_then(|e| e.features.first().copied())
    else {
        return;
    };
    let has_deity = w.entity(u).facts.list.iter().any(|f| {
        let fact = w.fact(*f);
        fact.kind == FactKind::Feature
            && fact.bp.as_ref().is_some_and(|b| {
                b.view()
                    .seq("Groups")
                    .iter()
                    .any(|n| matches!(n, wr_content::Node::Enum(24) | wr_content::Node::Int(24)))
            })
    });
    let has_fact = w.entity(u).facts.list.iter().any(|f| w.fact(*f).blueprint == Some(feature));
    if !has_deity
        && !has_fact
        && let Some(fb) = bp(w, Some(feature))
    {
        // `unit.AddFact<Feature>(feature).SetSource(FeatsProgression, 1)` and
        // `Progression.AddSelection(DeitySelection, FeatsProgression, 1, feature)`.
        let feats = super::root_ref(w, "Progression", "m_FeatsProgression").map(|b| b.guid);
        if let Some(f) = crate::unit_facts::add_fact(w, u, &fb)
            && let Some(s) = feats
        {
            set_source(w, f, s);
        }
        progression_add_selection(w, u, deity_sel, feats, 1, feature);
    }
}

/// `AddClassLevels.AddLevel` with an instant-commit controller on the unit.
fn add_level(
    w: &mut World,
    u: EntityIdx,
    cfg: &Cfg,
    c: CompRef,
    history: &mut History,
    ignore_prerequisites: bool,
) {
    let class = cfg.class.clone();
    // `new LevelUpState(unit)` reads `CharacterLevel` and `MythicLevel`, storing the caches.
    crate::unit::setup_levels_if_necessary(w, u);
    let mut st = State {
        next_character_level: crate::unit::character_level(w, u) + 1,
        ignore_prerequisites,
        ..State::default()
    };
    // `LevelUpState` ctor: the first level can select a race (`CharBuildMode.CharGen`).
    st.can_select_race = st.next_character_level == 1;
    // `SelectRace(Progression.Race)` (`LevelUpController.cs:998`): `SelectRace.Check`/`Apply`.
    if st.can_select_race
        && let Some(race) = w.unit(u).and_then(|x| x.progression.race)
    {
        select_race(w, u, &mut st, race);
    }
    if st.can_select_race_stat {
        select_race_stat(w, u, &mut st, cfg.race_stat);
    }
    // `ApplyStatsDistributionPreset(controller, fact)`: a preset on the fact's blueprint.
    let fact_bp = w.fact(c.fact).bp.clone();
    if fact_bp
        .is_some_and(|b| b.components_array().iter().any(|o| is_a(o.ty, "StatsDistributionPreset")))
    {
        w.coverage.unsupported("AddClassLevels: StatsDistributionPreset");
    }
    // `AddArchetype` for the first level of the class.
    let class_level = w
        .unit(u)
        .and_then(|x| x.progression.classes.iter().find(|cd| cd.class == class.guid))
        .map_or(0, |cd| cd.level);
    if class_level <= 0 {
        for &a in &cfg.archetypes {
            add_archetype(w, u, &class, a, st.ignore_prerequisites);
        }
    }
    // `SelectClass(class, applyMechanics: false)`.
    select_class(w, u, &class, &mut st);
    // `ApplyClassMechanics`.
    apply_class_mechanics(w, u, &class, &mut st);
    perform_selections(w, u, &mut st, cfg, history, Some(prio::REPLACE_SPELLBOOK));
    apply_spellbook(w, u, &class, &mut st, cfg);
    // `while (AttributePoints > 0 && LevelsStat.IsAttribute()) SpendAttributePoint(LevelsStat)`
    // (`AddClassLevels.cs:298`): `SpendAttributePoint.Check`/`Apply` (`SpendAttributePoint.cs:26`).
    while st.attribute_points > 0 && cfg.levels_stat.is_attribute() {
        st.attribute_points -= 1;
        let base = w.stats(u).v(cfg.levels_stat).base_value;
        ops::set_base_value(w, u, cfg.levels_stat, base.wrapping_add(1));
    }
    perform_selections(w, u, &mut st, cfg, history, Some(prio::APPLY_SKILL_POINTS));
    apply_skill_points(w, u, &mut st);
    while st.total_skill_points - st.spent_skill_points > 0 {
        let before = st.total_skill_points - st.spent_skill_points;
        for &s in &cfg.skills {
            if st.total_skill_points - st.spent_skill_points <= 0 {
                break;
            }
            spend_skill_point(w, u, &mut st, s);
        }
        if before == st.total_skill_points - st.spent_skill_points {
            break;
        }
    }
    perform_selections(w, u, &mut st, cfg, history, None);
    // `PerformSpellSelections`: the spell selections come from `ApplySpellbook`.
}

/// `SelectRace.Check` and `Apply` (`LevelUpController.SelectRace`).
fn select_race(w: &mut World, u: EntityIdx, st: &mut State, race: Guid) {
    let Some(r) = bp(w, Some(race)) else {
        w.coverage.unsupported("SelectRace: race blueprint missing");
        return;
    };
    // `Race.GetRestrictedByArchetype(unit)`: the first level has no class yet.
    if w.unit(u).is_some_and(|x| !x.progression.classes.is_empty()) {
        w.coverage.unsupported("SelectRace: GetRestrictedByArchetype with classes");
    }
    st.can_select_race = false;
    if r.view().bool("SelectableRaceStat") {
        st.can_select_race_stat = true;
    }
    set_race(w, u, race);
    // `State.Size = OriginalSize`: set at construction (`spawn::initialize`).
    let features = r.view().guids("m_Features");
    add_features_from_progression(w, u, st, &features, race, 0);
}

/// `SelectRaceStat.Check` and `Apply`: the racial `+2`.
fn select_race_stat(w: &mut World, u: EntityIdx, st: &mut State, stat: StatType) {
    if !stat.is_attribute() || !st.can_select_race_stat {
        return;
    }
    st.can_select_race_stat = false;
    let race = w.unit(u).and_then(|x| x.progression.race);
    let fact = race.and_then(|g| {
        w.entity(u)
            .facts
            .list
            .iter()
            .copied()
            .find(|f| w.fact(*f).kind == FactKind::Feature && w.fact(*f).blueprint == Some(g))
    });
    ops::add_modifier_from_fact(
        w,
        u,
        stat,
        2,
        fact,
        crate::stats::types::ModifierDescriptor::Racial,
    );
}

/// `AddArchetype.Check` and `Apply` for the level's class (`UnitProgressionData.cs:458`).
fn add_archetype(
    w: &mut World,
    u: EntityIdx,
    class: &Rc<Blueprint>,
    archetype: Guid,
    ignore_prerequisites: bool,
) {
    let Some(a) = bp(w, Some(archetype)) else {
        w.coverage.unsupported("AddArchetype: archetype blueprint missing");
        return;
    };
    // `CharacterClass.Archetypes.Contains(Archetype)`.
    if !class.view().guids("m_Archetypes").contains(&archetype) {
        return;
    }
    // `CanAddArchetype`: `SureClassData(class)` creates the class's data.
    sure_class_data(w, u, class);
    let class_level = w
        .unit(u)
        .and_then(|x| x.progression.classes.iter().find(|cd| cd.class == class.guid))
        .map_or(0, |cd| cd.level);
    if class_level > min_feature_level(&a) {
        return;
    }
    let Some(prog) = bp(w, class.view().guid("m_Progression")) else {
        w.coverage.unsupported("AddArchetype: class progression missing");
        return;
    };
    if !progression_can_add_archetype(w, u, &prog, &a) {
        return;
    }
    // `unit.Progression.AddArchetype(class, archetype)`.
    {
        let cd = sure_class_data(w, u, class);
        let (add_to_class, replace) = (!cd.archetypes.contains(&archetype), &a);
        if add_to_class {
            cd.archetypes.push(archetype);
            if replace.view().bool("RemoveSpellbook") {
                cd.spellbook = None;
            } else if let Some(sb) = replace.view().guid("m_ReplaceSpellbook") {
                cd.spellbook = Some(sb);
            }
        }
    }
    let pd = sure_progression_data(w, u, prog.guid);
    if !pd.archetypes.contains(&archetype) {
        pd.archetypes.push(archetype);
    }
    // `Archetype.RestrictPrerequisites`: returns at once under `IgnorePrerequisites`;
    // otherwise only `PrerequisiteAlignment` restricts anything.
    if !ignore_prerequisites
        && a.components_array().iter().any(|o| is_a(o.ty, "PrerequisiteAlignment"))
    {
        w.coverage.unsupported("BlueprintArchetype.RestrictPrerequisites: PrerequisiteAlignment");
    }
}

/// `BlueprintArchetype.MinFeatureLevel`.
fn min_feature_level(a: &Blueprint) -> i32 {
    let v = a.view();
    if v.bool("RemoveSpellbook") || v.guid("m_ReplaceSpellbook").is_some() {
        return 1;
    }
    let levels = v.objs("AddFeatures").into_iter().chain(v.objs("RemoveFeatures"));
    levels.map(|e| View(e).i32("Level")).min().unwrap_or(i32::MAX)
}

/// `UnitProgressionData.SureClassData(class)`.
fn sure_class_data<'a>(
    w: &'a mut World,
    u: EntityIdx,
    class: &Blueprint,
) -> &'a mut crate::unit::ClassData {
    let spellbook = class.view().guid("m_Spellbook");
    let p = &mut w.unit_mut(u).expect("unit").progression;
    let i = match p.classes.iter().position(|c| c.class == class.guid) {
        Some(i) => i,
        None => {
            p.classes.push(crate::unit::ClassData {
                class: class.guid,
                level: 0,
                archetypes: Vec::new(),
                spellbook,
            });
            p.classes.len() - 1
        }
    };
    &mut p.classes[i]
}

/// `UnitProgressionData.SureProgressionData(progression)`.
fn sure_progression_data(
    w: &mut World,
    u: EntityIdx,
    progression: Guid,
) -> &mut crate::unit::ProgressionData {
    let p = &mut w.unit_mut(u).expect("unit").progression;
    let i = match p.progressions.iter().position(|d| d.progression == progression) {
        Some(i) => i,
        None => {
            p.progressions.push(crate::unit::ProgressionData {
                progression,
                level: 0,
                archetypes: Vec::new(),
            });
            p.progressions.len() - 1
        }
    };
    &mut p.progressions[i]
}

/// `ProgressionData.CanAddArchetype(archetype)`.
fn progression_can_add_archetype(
    w: &mut World,
    u: EntityIdx,
    prog: &Rc<Blueprint>,
    a: &Blueprint,
) -> bool {
    let archetypes = sure_progression_data(w, u, prog.guid).archetypes.clone();
    let replaces = |g: &Guid| {
        w.content.get(*g).is_some_and(|b| {
            b.view().guid("m_ReplaceSpellbook").is_some() || b.view().bool("RemoveSpellbook")
        })
    };
    let a_replaces =
        a.view().guid("m_ReplaceSpellbook").is_some() || a.view().bool("RemoveSpellbook");
    if a_replaces && archetypes.iter().any(replaces) {
        return false;
    }
    let entries = level_entries(w, prog, &archetypes);
    for removed in a.view().objs("RemoveFeatures") {
        let rv = View(removed);
        let level = rv.i32("Level");
        let have = entries.iter().find(|e| e.0 == level).map(|e| e.1.clone()).unwrap_or_default();
        let wanted = rv.guids("m_Features");
        for f in &wanted {
            let n2 = wanted.iter().filter(|x| *x == f).count();
            let n3 = have.iter().filter(|x| *x == f).count();
            if n2 > n3 {
                return false;
            }
        }
    }
    true
}

/// `ProgressionData.CalculateLevelEntries()`: the progression's entries with the archetypes'
/// removed and added features, as `(level, features)` in order.
fn level_entries(w: &World, prog: &Blueprint, archetypes: &[Guid]) -> Vec<(i32, Vec<Guid>)> {
    let own: Vec<(i32, Vec<Guid>)> = prog
        .view()
        .objs("LevelEntries")
        .into_iter()
        .map(|e| (View(e).i32("Level"), View(e).guids("m_Features")))
        .collect();
    if archetypes.is_empty() {
        return own;
    }
    let abps: Vec<Rc<Blueprint>> = archetypes.iter().filter_map(|g| w.content.get(*g)).collect();
    // `Archetype.GetAddEntry(level)` / `GetRemoveEntry(level)`: every entry of that level.
    let features_at = |a: &Blueprint, field: &str, level: i32| -> Vec<Guid> {
        a.view()
            .objs(field)
            .into_iter()
            .filter(|e| View(e).i32("Level") == level)
            .flat_map(|e| View(e).guids("m_Features"))
            .collect()
    };
    let mut list: Vec<(i32, Vec<Guid>)> = Vec::new();
    for (level, features) in &own {
        let mut f2 = features.clone();
        for a in &abps {
            for r in features_at(a, "RemoveFeatures", *level) {
                if let Some(i) = f2.iter().position(|x| *x == r) {
                    f2.remove(i);
                }
            }
            f2.extend(features_at(a, "AddFeatures", *level));
        }
        if !f2.is_empty() {
            list.push((*level, f2));
        }
    }
    let mut extra: Vec<(i32, Vec<Guid>)> = Vec::new();
    for a in &abps {
        for e in a.view().objs("AddFeatures") {
            let ev = View(e);
            let level = ev.i32("Level");
            let feats = ev.guids("m_Features");
            if !feats.is_empty() && !list.iter().any(|x| x.0 == level) {
                match extra.iter_mut().find(|x| x.0 == level) {
                    Some(x) => x.1.extend(feats),
                    None => extra.push((level, feats)),
                }
            }
        }
    }
    list.extend(extra);
    // `List<LevelEntry>.Sort` by level (Mono's unstable introsort): equal levels can't occur
    // (merged above), so any sort gives the same order.
    list.sort_by_key(|e| e.0);
    list
}

/// `SelectClass.Apply` → `UnitProgressionData.AddClassLevel(class)`.
fn select_class(w: &mut World, u: EntityIdx, class: &Rc<Blueprint>, st: &mut State) {
    let mythic = class.view().bool("IsMythic");
    if class.view().bool("IsHigherMythic") {
        w.coverage.unsupported("UnitProgressionData.AddClassLevel: higher mythic class");
    }
    let level = {
        let cd = sure_class_data(w, u, class);
        cd.level += 1;
        cd.level
    };
    // (`m_ClassesOrder.Add`: not modelled; no `ObligatoryMythicClassesQueue` on these units.)
    // `MythicLevel++` / `CharacterLevel++` read the levels first (`SetupLevelsIfNecessary`).
    crate::unit::setup_levels_if_necessary(w, u);
    if mythic {
        // `MythicLevel++; MythicExperience = Math.Max(MythicExperience, MythicLevel)`.
        let ml = crate::unit::mythic_level(w, u) + 1;
        let p = &mut w.unit_mut(u).expect("unit").progression;
        p.mythic_level = Some(ml);
        p.mythic_experience = p.mythic_experience.max(ml);
    } else {
        let cl = crate::unit::character_level(w, u) + 1;
        {
            let p = &mut w.unit_mut(u).expect("unit").progression;
            p.character_level = Some(cl);
        }
        // `Experience = Math.Max(Experience, XPTable.Bonuses[Math.Min(len - 1, level)])`.
        let xp = super::root_ref(w, "Progression", "m_XPTable")
            .map(|t| {
                let b: Vec<i64> = t
                    .view()
                    .seq("Bonuses")
                    .iter()
                    .filter_map(|n| if let wr_content::Node::Int(i) = n { Some(*i) } else { None })
                    .collect();
                let i = usize::try_from(cl).unwrap_or(0).min(b.len().saturating_sub(1));
                b.get(i).copied().unwrap_or(0)
            })
            .unwrap_or(0);
        let p = &mut w.unit_mut(u).expect("unit").progression;
        p.experience = p.experience.max(xp);
    }
    // `Classes.Sort()`: `List<ClassData>.Sort` with `ClassData.CompareTo` (higher levels first;
    // `other.Level.CompareTo(Level)`), the framework's introspective sort.
    {
        let classes = &mut w.unit_mut(u).expect("unit").progression.classes;
        let cap = crate::csharp::sort::grown_capacity(classes.len());
        crate::csharp::sort::introsort(classes, cap, &mut |a: &crate::unit::ClassData, b| {
            b.level.cmp(&a.level) as i32
        });
    }
    // `Features.AddFeature(Owner.GetProgression(characterClass)).SetSource(characterClass, 1)`.
    if let Some(prog) = bp(w, class.view().guid("m_Progression"))
        && let Some(f) = crate::unit_facts::add_fact(w, u, &prog)
    {
        set_source(w, f, class.guid);
    }
    // `OnGainClassLevel`: HP update, `IOwnerGainLevelHandler`.
    ops::update_value(w, u, StatType::HitPoints);
    crate::rulebook::bus::raise_unit(w, u, "IOwnerGainLevelHandler");
    st.next_class_level = level;
    st.selected_class = Some(class.guid);
    if !mythic && st.next_character_level % 4 == 0 {
        st.attribute_points = 1;
    }
    // `RestrictPrerequisites` (ignored under `IgnorePrerequisites`), `ILevelUpSelectClassHandler` (UI).
}

/// `Feature.SetSource(source, level)` (the source blueprint; ranks above 1 keep a list).
fn set_source(w: &mut World, f: FactIdx, source: Guid) {
    w.fact_mut(f).source_blueprint = Some(source);
}

/// `BlueprintStatProgression.GetBonus(level)`.
fn stat_bonus(w: &World, prog: Option<Guid>, level: i32) -> i32 {
    let Some(p) = bp(w, prog) else { return 0 };
    let b = p.view();
    let arr = b.seq("Bonuses");
    usize::try_from(level)
        .ok()
        .and_then(|i| arr.get(i))
        .and_then(|n| if let wr_content::Node::Int(i) = n { i32::try_from(*i).ok() } else { None })
        .unwrap_or(0)
}

/// `ApplyClassMechanics.Apply`: base stats, hit points, class skills, progressions.
fn apply_class_mechanics(w: &mut World, u: EntityIdx, class: &Rc<Blueprint>, st: &mut State) {
    let v = class.view();
    let old = st.next_class_level - 1;
    let new = st.next_class_level;
    // `ClassData.BaseAttackBonus` etc.: the first archetype that sets one, else the class's.
    let archetypes: Vec<Rc<Blueprint>> = w
        .unit(u)
        .and_then(|x| x.progression.classes.iter().find(|cd| cd.class == class.guid))
        .map(|cd| cd.archetypes.clone())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|g| w.content.get(g))
        .collect();
    for (stat, field, afield) in [
        (StatType::BaseAttackBonus, "m_BaseAttackBonus", "m_BaseAttackBonus"),
        (StatType::SaveFortitude, "m_FortitudeSave", "m_FortitudeSave"),
        (StatType::SaveReflex, "m_ReflexSave", "m_ReflexSave"),
        (StatType::SaveWill, "m_WillSave", "m_WillSave"),
    ] {
        let g = archetypes.iter().find_map(|a| a.view().guid(afield)).or_else(|| v.guid(field));
        let d = stat_bonus(w, g, new) - stat_bonus(w, g, old);
        let base = w.stats(u).v(stat).base_value;
        ops::set_base_value(w, u, stat, base.wrapping_add(d));
    }
    // `ApplyHitPoints(state.NextCharacterLevel, class, unit)`: a mythic level adds 5.
    if v.bool("IsMythic") {
        let base = w.stats(u).v(StatType::HitPoints).base_value;
        ops::set_base_value(w, u, StatType::HitPoints, base.wrapping_add(5));
    } else {
        let hit_die = i32::try_from(v.int("HitDie")).unwrap_or(0);
        if w.entity(u).parts.iter().any(|p| p.clr == "ChangeHitDie+HitDiePart") {
            w.coverage.unsupported("ApplyHitPoints: ChangeHitDie");
        }
        let available = super::root(w)
            .and_then(|r| r.view().obj("Progression").map(|p| p.guids("m_CharacterClasses")))
            .is_some_and(|l| l.contains(&class.guid));
        let player = w.unit(u).is_some_and(|x| x.is_player_faction);
        let level = st.next_character_level;
        let add = if !available || !player {
            hit_die / 2 + level % 2
        } else if level > 1 {
            hit_die / 2 + 1
        } else {
            hit_die
        };
        let base = w.stats(u).v(StatType::HitPoints).base_value;
        ops::set_base_value(w, u, StatType::HitPoints, base.wrapping_add(add));
        ops::update_value(w, u, StatType::HitPoints);
    }
    // `ApplyClassSkills`: the first archetype that replaces them, else the class's.
    let replace = archetypes.iter().find(|a| a.view().bool("ReplaceClassSkills"));
    let class_skills: Vec<StatType> = replace
        .map_or_else(|| v.seq("ClassSkills"), |a| a.view().seq("ClassSkills"))
        .iter()
        .filter_map(|n| match n {
            wr_content::Node::Enum(i) | wr_content::Node::Int(i) => {
                i32::try_from(*i).ok().map(StatType)
            }
            _ => None,
        })
        .collect();
    for s in class_skills {
        ops::class_skill_retain(w, u, s);
    }
    // `ApplyProgressions`: every progression feature, in the feature list's order; on the
    // first class level the class's progression takes the class as its source.
    let first = st.next_class_level <= 1;
    let progs: Vec<(FactIdx, Rc<Blueprint>)> = w
        .entity(u)
        .facts
        .list
        .iter()
        .copied()
        .filter(|f| w.fact(*f).kind == FactKind::Feature)
        .filter_map(|f| {
            w.fact(f).bp.clone().filter(|b| is_a(b.obj.ty, "BlueprintProgression")).map(|b| (f, b))
        })
        .collect();
    for (f, p) in progs {
        if first && progression_classes(&p).contains(&class.guid) {
            set_source(w, f, class.guid);
        }
        update_progression(w, u, st, &p);
    }
}

fn progression_classes(p: &Blueprint) -> Vec<Guid> {
    p.view().objs("m_Classes").into_iter().filter_map(|o| View(o).guid("m_Class")).collect()
}

/// `BlueprintProgression.CalcLevel(unit)`.
fn calc_level(w: &mut World, u: EntityIdx, p: &Blueprint) -> i32 {
    let v = p.view();
    let classes = v.objs("m_Classes");
    let prog_archetypes = v.objs("m_Archetypes");
    if classes.is_empty() && prog_archetypes.is_empty() {
        return crate::unit::character_level(w, u);
    }
    let unit_classes = w.unit(u).map(|x| x.progression.classes.clone()).unwrap_or_default();
    let archetype_guids: Vec<Guid> =
        prog_archetypes.iter().filter_map(|o| View(o).guid("m_Archetype")).collect();
    let mut n = 0i32;
    // `m_FeaturesRankIncrease`: each feature the unit has adds its rank.
    for g in v.guids("m_FeaturesRankIncrease") {
        let fact = w.entity(u).facts.list.iter().copied().find(|f| w.fact(*f).blueprint == Some(g));
        if let Some(f) = fact {
            n = n.wrapping_add(crate::components::fact_rank(w, f));
        }
    }
    let class_archetypes = |w: &World, class: Guid| -> Vec<Guid> {
        w.content.get(class).map(|b| b.view().guids("m_Archetypes")).unwrap_or_default()
    };
    for cv in &classes {
        let cv = View(cv);
        let Some(g) = cv.guid("m_Class") else { continue };
        if class_archetypes(w, g).iter().any(|a| archetype_guids.contains(a)) {
            continue;
        }
        let level = unit_classes.iter().find(|cd| cd.class == g).map_or(0, |cd| cd.level);
        n = n.wrapping_add(0.max(level.wrapping_add(cv.i32("AdditionalLevel"))));
    }
    for av in &prog_archetypes {
        let av = View(av);
        let Some(a) = av.guid("m_Archetype") else { continue };
        for cd in &unit_classes {
            if cd.archetypes.contains(&a) {
                n = n.wrapping_add(0.max(cd.level.wrapping_add(av.i32("AdditionalLevel"))));
            }
        }
    }
    let mut n2 = 0i32;
    if v.bool("ForAllOtherClasses") {
        for cd in &unit_classes {
            let listed = classes.iter().any(|c| View(c).guid("m_Class") == Some(cd.class));
            let by_archetype = cd.archetypes.iter().any(|a| archetype_guids.contains(a));
            if !listed && !by_archetype {
                n2 = n2.wrapping_add(cd.level);
            }
        }
    } else {
        for cv in v.objs("m_AlternateProgressionClasses") {
            let cv = View(cv);
            let Some(g) = cv.guid("m_Class") else { continue };
            if class_archetypes(w, g).iter().any(|a| archetype_guids.contains(a)) {
                continue;
            }
            let level = unit_classes.iter().find(|cd| cd.class == g).map_or(0, |cd| cd.level);
            n2 = n2.wrapping_add(0.max(level.wrapping_add(cv.i32("AdditionalLevel"))));
        }
    }
    // `AlternateProgressionType.Div2` (1).
    if v.int("AlternateProgressionType") == 1 {
        n = n.wrapping_add(n2 / 2);
    }
    n
}

/// `LevelUpHelper.UpdateProgression(state, unit, progression)`.
fn update_progression(w: &mut World, u: EntityIdx, st: &mut State, p: &Rc<Blueprint>) {
    let data = sure_progression_data(w, u, p.guid);
    let mut old = data.level;
    let archetypes = data.archetypes.clone();
    let new = calc_level(w, u, p);
    sure_progression_data(w, u, p.guid).level = new;
    let exclusive = p.view().guid("m_ExclusiveProgression");
    if old < new && (exclusive.is_none() || st.selected_class == exclusive) {
        if !p.view().bool("GiveFeaturesForPreviousLevels") {
            old = new - 1;
        }
        let entries = level_entries(w, p, &archetypes);
        for i in (old + 1)..=new {
            // `ProgressionData.GetLevelEntry(i)`: the first entry of that level (levels are
            // unique after the archetypes merged them; a progression's own entries may repeat
            // a level, which `FirstOrDefault` handles).
            let features: Vec<Guid> =
                entries.iter().find(|e| e.0 == i).map(|e| e.1.clone()).unwrap_or_default();
            add_features_from_progression(w, u, st, &features, p.guid, i);
        }
    }
}

/// `LevelUpHelper.AddFeaturesFromProgression`.
fn add_features_from_progression(
    w: &mut World,
    u: EntityIdx,
    st: &mut State,
    features: &[Guid],
    source: Guid,
    level: i32,
) {
    for g in features {
        let Some(f) = w.content.get(*g) else { continue };
        if !is_a(f.obj.ty, "BlueprintFeature") {
            continue;
        }
        if !meets_prerequisites_from_progression(w, u, &f) {
            continue;
        }
        if let Some(kind) = selection_kind(&f)
            && (!selection_prohibited(w, u, &f, kind) || selection_obligatory(&f, kind))
        {
            st.add_selection(None, Some(source), f.guid, level);
        }
        let Some(fact) = crate::unit_facts::add_fact(w, u, &f) else { continue };
        if is_a(f.obj.ty, "BlueprintProgression") {
            update_progression(w, u, st, &f);
        }
        set_source(w, fact, source);
    }
}

/// `IFeatureSelection.IsObligatory()`.
fn selection_obligatory(sel: &Blueprint, kind: SelKind) -> bool {
    match kind {
        SelKind::Plain => sel.view().bool("Obligatory"),
        // `BlueprintFeatureSelectMythicSpellbook.IsObligatory()`.
        SelKind::MythicSpellbook => true,
        SelKind::Parametrized => false,
    }
}

/// `IFeatureSelection.IsSelectionProhibited(unit)` (`NoSelectionIfAlreadyHasFeature`).
fn selection_prohibited(w: &mut World, u: EntityIdx, sel: &Blueprint, kind: SelKind) -> bool {
    match kind {
        // `IsSelectionProhibited(unit)` is `false` for both.
        SelKind::Parametrized | SelKind::MythicSpellbook => return false,
        SelKind::Plain => {}
    }
    let Some(c) =
        sel.components_array().into_iter().find(|o| is_a(o.ty, "NoSelectionIfAlreadyHasFeature"))
    else {
        return false;
    };
    let c = View(c);
    let has = |w: &World, g: Guid| feature_rank(w, u, g) > 0;
    if c.bool("AnyFeatureFromSelection")
        && sel.view().guids("m_AllFeatures").into_iter().any(|g| has(w, g))
    {
        return true;
    }
    c.guids("m_Features").into_iter().any(|g| has(w, g))
}

fn feature_rank(w: &World, u: EntityIdx, g: Guid) -> i32 {
    w.entity(u)
        .facts
        .list
        .iter()
        .find(|f| w.fact(**f).kind == FactKind::Feature && w.fact(**f).blueprint == Some(g))
        .map_or(0, |f| w.fact(*f).rank)
}

/// `BlueprintFeature.MeetsPrerequisites(null, unit, state, fromProgression: true)`: the
/// rank limit, then each prerequisite flagged `CheckInProgression` (groups "all" and
/// "any").
fn meets_prerequisites_from_progression(w: &mut World, u: EntityIdx, f: &Blueprint) -> bool {
    let is_selection = selection_kind(f).is_some();
    if feature_rank(w, u, f.guid) >= f.view().i32("Ranks") && !is_selection {
        return false;
    }
    let mut all: Option<bool> = None;
    let mut any: Option<bool> = None;
    for o in f.components_array() {
        if !is_a(o.ty, "Prerequisite") {
            continue;
        }
        let v = View(o);
        if !v.bool("CheckInProgression") {
            continue;
        }
        let ok = check_prerequisite(w, u, v);
        if v.int("Group") == 0 {
            all = Some(all.unwrap_or(true) & ok);
        } else {
            any = Some(any.unwrap_or(false) | ok);
        }
    }
    all.unwrap_or(true) && any.unwrap_or(true)
}

/// `IFeatureSelection.IsIgnorePrerequisites() || selectionState.IgnorePrerequisites` (the
/// state and its parents' selections).
fn selection_ignores_prerequisites(w: &mut World, st: &State, idx: usize, sel: &Blueprint) -> bool {
    let own = |w: &mut World, sel: &Blueprint| {
        if sel.view().bool("IgnorePrerequisites") {
            return true;
        }
        // `TricksterIgnoreFeatPrerequisites` on the main character, for the basic feat
        // selection.
        let basic = super::root_ref(w, "SystemMechanics", "m_BasicFeatSelection").map(|b| b.guid);
        let trickster = w
            .main_character
            .and_then(|m| w.unit(m))
            .is_some_and(|m| m.mechanics_features.get(&97).copied().unwrap_or(0) > 0);
        trickster && Some(sel.guid) == basic
    };
    if own(w, sel) {
        return true;
    }
    let mut cur = Some(idx);
    while let Some(i) = cur {
        if let Some(s) = w.content.get(st.selections[i].selection)
            && own(w, &s)
        {
            return true;
        }
        cur = st.selections[i].parent;
    }
    false
}

/// `BlueprintFeature.MeetsPrerequisites(selectionState, unit, state, fromProgression:
/// false, onlyWhiteListed)` outside `IgnorePrerequisites`: the rank limit, then every
/// prerequisite (the white-listed ones only for `only_white_listed`), "all" and "any" groups.
fn meets_prerequisites(
    w: &mut World,
    u: EntityIdx,
    f: &Blueprint,
    only_white_listed: bool,
) -> bool {
    let is_selection = selection_kind(f).is_some();
    if feature_rank(w, u, f.guid) >= f.view().i32("Ranks") && !is_selection {
        return false;
    }
    let mut all: Option<bool> = None;
    let mut any: Option<bool> = None;
    for o in f.components_array() {
        if !is_a(o.ty, "Prerequisite") {
            continue;
        }
        let v = View(o);
        if only_white_listed && !v.bool("IsFeatureSelectionWhiteList") {
            continue;
        }
        let ok = check_prerequisite(w, u, v);
        if v.int("Group") == 0 {
            all = Some(all.unwrap_or(true) & ok);
        } else {
            any = Some(any.unwrap_or(false) | ok);
        }
    }
    all.unwrap_or(true) && any.unwrap_or(true)
}

/// `Prerequisite.Check` for the prerequisite types on these paths.
fn check_prerequisite(w: &mut World, u: EntityIdx, v: View<'_>) -> bool {
    if v.is("PrerequisiteIsPet") {
        let pet = w.unit(u).is_some_and(|x| x.is_pet);
        let not = v.bool("Not");
        return if !pet || not { if !pet { not } else { false } } else { true };
    }
    if v.is("PrerequisiteStatValue") {
        let stat = StatType(v.i32("Stat"));
        let s = w.stats(u).v(stat);
        let value = if stat.is_skill() { s.base_value } else { s.permanent_value };
        return value >= v.i32("Value");
    }
    if v.is("PrerequisiteFeature") {
        let g = v.guid("m_Feature");
        let has = g.is_some_and(|g| {
            w.entity(u).facts.list.iter().any(|f| w.fact(*f).blueprint == Some(g))
        });
        return has;
    }
    if v.is("PrerequisiteNoFeature") {
        let g = v.guid("m_Feature");
        let has = g.is_some_and(|g| {
            w.entity(u).facts.list.iter().any(|f| w.fact(*f).blueprint == Some(g))
        });
        return !has;
    }
    if v.is("PrerequisiteClassLevel") {
        // `GetClassLevel`: the class level plus the fake levels of `ClassLevelsForPrerequisites`
        // components on the unit's features (not ported).
        let has_fake = w.entity(u).facts.list.iter().any(|f| {
            w.fact(*f).components.iter().any(|c| {
                c.bp_component().is_some_and(|o| is_a(o.ty, "ClassLevelsForPrerequisites"))
            })
        });
        if has_fake {
            w.coverage.unsupported("PrerequisiteClassLevel: ClassLevelsForPrerequisites");
        }
        let level = v
            .guid("m_CharacterClass")
            .and_then(|c| {
                w.unit(u)?.progression.classes.iter().find(|x| x.class == c).map(|x| x.level)
            })
            .unwrap_or(0);
        let need = v.i32("Level");
        return if v.bool("Not") { level < need } else { level >= need };
    }
    if v.is("PrerequisiteProficiency") {
        // Any listed armor group or weapon category in the unit's proficiencies.
        let p = &w.unit(u).expect("unit").proficiencies;
        let has = |list: &[(i64, i32)], seq: &[wr_content::Node]| {
            seq.iter().any(|n| match n {
                wr_content::Node::Enum(k) | wr_content::Node::Int(k) => {
                    list.iter().any(|(x, c)| x == k && *c > 0)
                }
                _ => false,
            })
        };
        return has(&p.armor, v.seq("ArmorProficiencies"))
            | has(&p.weapons, v.seq("WeaponProficiencies"));
    }
    w.coverage.unsupported(format!("Prerequisite.Check: {}", v.type_name()));
    true
}

// ---- feature selections ------------------------------------------------------------

/// `IFeatureSelection.Items`.
fn selection_items(w: &mut World, sel: &Blueprint, kind: SelKind) -> Vec<Item> {
    match kind {
        SelKind::Plain => {
            let mut out = Vec::new();
            for g in sel.view().guids("m_AllFeatures") {
                let Some(f) = w.content.get(g) else { continue };
                // `!f.IsDlcRestricted()` (`BlueprintFeatureSelection.cs:49`).
                if crate::elements::conditions::dlc_restricted_or_report(
                    w,
                    &f,
                    "IFeatureSelection.Items",
                ) {
                    continue;
                }
                out.push(Item { feature: g, param: None });
            }
            out
        }
        SelKind::Parametrized => parametrized_items(w, sel),
        // `BlueprintFeatureSelectMythicSpellbook.CollectItems`: the empty choice, then each
        // allowed spellbook (`new FeatureUIData(this, (BlueprintScriptableObject)null)` is a
        // non-null, empty param).
        SelKind::MythicSpellbook => {
            let feature = sel.guid;
            let mut out = vec![Item { feature, param: Some(FeatureParam::of_blueprint(None)) }];
            for sb in sel.view().guids("m_AllowedSpellbooks") {
                if w.content.get(sb).is_some() {
                    out.push(Item { feature, param: Some(FeatureParam::of_blueprint(Some(sb))) });
                }
            }
            out
        }
    }
}

/// `BlueprintParametrizedFeature.GetFullSelectionItems()` (`Items`) by `ParameterType`:
/// Custom 0, SpellSpecialization 1, WeaponCategory 2, SpellSchool 3, LearnSpell 4, Skill 5,
/// FeatureSelection 6.
fn parametrized_items(w: &mut World, sel: &Blueprint) -> Vec<Item> {
    let feature = sel.guid;
    let v = sel.view();
    let item = |p: FeatureParam| Item { feature, param: Some(p) };
    match v.int("ParameterType") {
        // `ExtractItemsFromBlueprints(BlueprintParameterVariants.Dereference())`.
        0 | 1 | 6 => v
            .guids("BlueprintParameterVariants")
            .into_iter()
            .filter(|g| w.content.get(*g).is_some())
            .map(|g| item(FeatureParam::of_blueprint(Some(g))))
            .collect(),
        // `ExtractItemsFromSpellList()` without a unit: no spellbook, so nothing is known and
        // the level penalty doesn't apply; a specific spell level keeps only that level.
        4 => {
            let Some(list) = v.guid("m_SpellList").and_then(|g| w.content.get(g)) else {
                w.coverage.unsupported("BlueprintParametrizedFeature.Items: no spell list");
                return Vec::new();
            };
            let specific = v.bool("SpecificSpellLevel");
            let mut out = Vec::new();
            for level_list in list.view().objs("SpellsByLevel") {
                let lv = View(level_list);
                if specific && lv.i32("SpellLevel") != v.i32("SpellLevel") {
                    continue;
                }
                // `SpellLevelList.SpellsFiltered`: not DLC-restricted.
                for spell in lv.guids("m_Spells") {
                    let Some(b) = w.content.get(spell) else { continue };
                    if crate::elements::conditions::dlc_restricted_or_report(
                        w,
                        &b,
                        "SpellLevelList.SpellsFiltered",
                    ) {
                        continue;
                    }
                    out.push(item(FeatureParam::of_blueprint(Some(spell))));
                }
            }
            out
        }
        // Every weapon category (enum order) of the sub-category, unless disabled.
        2 => {
            let sub = v.int("WeaponSubCategory");
            (0..84)
                .filter(|c| {
                    feature_param::has_sub_category(*c, sub)
                        && !feature_param::has_sub_category(*c, feature_param::SUB_DISABLED)
                })
                .map(|c| item(FeatureParam::of_weapon_category(c)))
                .collect()
        }
        // Every school but `None` and `Universalist`.
        3 => (1..=8).map(|s| item(FeatureParam::of_spell_school(s))).collect(),
        5 => SKILLS.iter().map(|s| item(FeatureParam::of_stat(i64::from(s.0)))).collect(),
        _ => {
            w.coverage.unsupported("BlueprintParametrizedFeature.Items: unknown ParameterType");
            Vec::new()
        }
    }
}

/// `IFeatureSelection.CanSelect(unit, state, selectionState, item)` for `sel`, with the
/// selection state `idx` (the outer one when this is a nested selection's check).
fn can_select(
    w: &mut World,
    u: EntityIdx,
    st: &State,
    idx: usize,
    sel: &Rc<Blueprint>,
    item: &Item,
) -> bool {
    let Some(kind) = selection_kind(sel) else { return false };
    match kind {
        SelKind::Plain => {}
        SelKind::Parametrized => return can_select_parametrized(w, u, st, idx, sel, item),
        // `BlueprintFeatureSelectMythicSpellbook.CanSelect`: a spellbook must be one the unit
        // has and the selection allows; the empty choice is always possible.
        SelKind::MythicSpellbook => {
            let Some(param) = item.param else { return false };
            return match param.blueprint {
                Some(sb) => {
                    crate::spellbook::get_spellbook(w, u, sb)
                        && sel.view().guids("m_AllowedSpellbooks").contains(&sb)
                }
                None => true,
            };
        }
    }
    let Some(feature) = w.content.get(item.feature) else { return false };
    if is_a(feature.obj.ty, "BlueprintLevelUpPlanFeaturesList") {
        w.coverage.unsupported("BlueprintFeatureSelection.CanSelect: level-up plan features list");
        return true;
    }
    if !sel.view().guids("m_AllFeatures").contains(&item.feature) {
        return false;
    }
    // The prerequisites: `MeetsPrerequisites(…, fromProgression: false)` returns true at
    // once under `IgnorePrerequisites.Ignore`, which `AddClassLevels.LevelUp` requests
    // around the unit's own levels but not around a plan unit's.
    if !st.ignore_prerequisites {
        let ignored = selection_ignores_prerequisites(w, st, idx, sel);
        let white = sel.view().bool("ExceptWhiteListed") && ignored;
        if (white || !ignored) && !meets_prerequisites(w, u, &feature, white) {
            return false;
        }
    }
    let rank = feature_rank(w, u, item.feature);
    let nested = selection_kind(&feature).is_some();
    if !nested && rank >= feature.view().i32("Ranks") {
        return false;
    }
    // `SelectionMode`: Default 0, OnlyNew 1, OnlyRankUp 2.
    let mode = sel.view().int("Mode");
    if mode == 1 && rank > 0 {
        return false;
    }
    if mode == 2 && rank <= 0 {
        return false;
    }
    // `NestedFeatureSelectionUtils.AllNestedFeaturesUnavailable`.
    if nested && !can_select_any(w, u, st, idx, &feature) {
        return false;
    }
    true
}

/// `BlueprintParametrizedFeature.CanSelect`.
fn can_select_parametrized(
    w: &mut World,
    u: EntityIdx,
    st: &State,
    idx: usize,
    sel: &Rc<Blueprint>,
    item: &Item,
) -> bool {
    let v = sel.view();
    if !parametrized_items(w, sel)
        .iter()
        .any(|i| i.feature == item.feature && i.param == item.param)
    {
        return false;
    }
    let Some(param) = item.param else { return false };
    let ptype = v.int("ParameterType");
    // `ParameterType == SpellSchool` needs a school other than `None`.
    if ptype == 3 && param.spell_school.is_none_or(|s| s == 0) {
        return false;
    }
    if feature_param::get_feature(w, u, sel.guid, &param).is_some() {
        return false;
    }
    let state_ignores = selection_ignores_prerequisites(w, st, idx, sel);
    if let Some(prereq) = v.guid("m_Prerequisite")
        && !st.ignore_prerequisites
    {
        if ptype == 1 {
            // `item.Param.Value.Blueprint as BlueprintAbility`: the feature of its school.
            let ability = param.blueprint.and_then(|g| w.content.get(g));
            if let Some(a) = ability
                && is_a(a.obj.ty, "BlueprintAbility")
            {
                let school = FeatureParam::of_spell_school(a.view().int("School"));
                if feature_param::get_feature(w, u, prereq, &school).is_none() {
                    return false;
                }
            }
        } else if feature_param::get_feature(w, u, prereq, &param).is_none() && !state_ignores {
            return false;
        }
    }
    if !st.ignore_prerequisites
        && let Some(c) = param.weapon_category
    {
        let proficient = w
            .unit(u)
            .is_some_and(|x| x.proficiencies.weapons.iter().any(|(k, n)| *k == c && *n > 0));
        if (v.bool("RequireProficiency") && !proficient)
            || feature_param::has_sub_category(c, feature_param::SUB_DISABLED)
        {
            return false;
        }
    }
    if !st.ignore_prerequisites {
        // `IParamPrerequisite.CanBeSelected` (`PrerequisiteConditionForWeaponCategory` is the
        // only implementer): a listed weapon category must satisfy the condition, checked in a
        // context whose caster and target are the unit.
        for o in sel.components_array() {
            if !is_a(o.ty, "PrerequisiteConditionForWeaponCategory") {
                continue;
            }
            let lists = param.weapon_category.is_some_and(|c| {
                View(o).seq("m_WeaponCategories").iter().any(|n| match n {
                    wr_content::Node::Enum(k) | wr_content::Node::Int(k) => *k == c,
                    _ => false,
                })
            });
            if ptype != 2 || !lists {
                continue;
            }
            match View(o).obj("m_Condition") {
                // `ContextConditionCharacterClass.CheckCondition`, then `Condition.Check`'s `Not`.
                Some(c) if c.is("ContextConditionCharacterClass") => {
                    let level = c.guid("m_Class").and_then(|g| {
                        w.unit(u)?
                            .progression
                            .classes
                            .iter()
                            .find(|x| x.class == g)
                            .map(|x| x.level)
                    });
                    let ok = level.is_some_and(|l| l >= c.i32("MinLevel"));
                    if ok == c.bool("Not") {
                        return false;
                    }
                }
                _ => {
                    w.coverage.unsupported("PrerequisiteConditionForWeaponCategory.CanBeSelected");
                }
            }
        }
    }
    let feature = param
        .blueprint
        .and_then(|g| w.content.get(g))
        .filter(|b| is_a(b.obj.ty, "BlueprintFeature"));
    if let Some(f) = &feature
        && v.bool("HasNoSuchFeature")
        && w.entity(u).facts.list.iter().any(|x| w.fact(*x).blueprint == Some(f.guid))
    {
        return false;
    }
    // `NestedFeatureSelectionUtils.AllNestedFeaturesUnavailable(…, blueprintFeature)`.
    if let Some(f) = &feature
        && selection_kind(f).is_some()
        && !can_select_any(w, u, st, idx, f)
    {
        return false;
    }
    if let Some(f) = &feature
        && !state_ignores
        && !v.bool("IgnoreParameterFeaturePrerequisites")
        && !st.ignore_prerequisites
        && !meets_prerequisites(w, u, f, false)
    {
        return false;
    }
    true
}

/// `FeatureSelectionExtensions.CanSelectAny`.
fn can_select_any(
    w: &mut World,
    u: EntityIdx,
    st: &State,
    idx: usize,
    sel: &Rc<Blueprint>,
) -> bool {
    let Some(kind) = selection_kind(sel) else { return false };
    let items = selection_items(w, sel, kind);
    for item in &items {
        if can_select(w, u, st, idx, sel, item) {
            return true;
        }
    }
    false
}

/// `LevelUpHelper`/`UnitProgressionData.AddSelection`.
fn progression_add_selection(
    w: &mut World,
    u: EntityIdx,
    selection: Guid,
    source: Option<Guid>,
    level: i32,
    feature: Guid,
) {
    let p = &mut w.unit_mut(u).expect("unit").progression;
    add_selection_data(&mut p.selections, selection, source, level, feature);
}

/// `UnitProgressionData.AddSelection` on the selections dictionary.
pub(super) fn add_selection_data(
    list: &mut Vec<crate::unit::SelectionData>,
    selection: Guid,
    source: Option<Guid>,
    level: i32,
    feature: Guid,
) {
    let i = match list.iter().position(|s| s.selection == selection) {
        Some(i) => i,
        None => {
            list.push(crate::unit::SelectionData { selection, source, by_level: Vec::new() });
            list.len() - 1
        }
    };
    let data = &mut list[i];
    match data.by_level.iter_mut().find(|(l, _)| *l == level) {
        Some((_, v)) => v.push(feature),
        None => data.by_level.push((level, vec![feature])),
    }
}

/// `UnitProgressionData.IsAlreadyApplyInProgression(index, entry)`.
fn already_applied_in_progression(w: &World, u: EntityIdx, index: usize, entry: &Entry) -> bool {
    let feature = entry.features[index];
    let num = entry.features[..index].iter().filter(|g| **g == feature).count();
    let Some(sel) = entry.selection else { return false };
    let p = &w.unit(u).expect("unit").progression;
    let Some(data) = p.selections.iter().find(|s| s.selection == sel) else { return false };
    let level = crate::unit::character_level(w, u);
    let mut num2 = 0usize;
    for j in 0..level {
        if let Some((_, v)) = data.by_level.iter().find(|(l, _)| *l == j) {
            num2 += v.iter().filter(|g| **g == feature).count();
        }
    }
    num2 > num
}

/// `AddClassLevels.GetSelectionItem`.
fn get_selection_item(
    w: &mut World,
    u: EntityIdx,
    st: &State,
    idx: usize,
    entry: &Entry,
    hist: &mut HashSet<usize>,
) -> Option<Item> {
    if entry.is_parametrized {
        let pf = entry.parametrized_feature?;
        let pbp = w.content.get(pf)?;
        // `FeatureParameterType`: Custom 0, SpellSpecialization 1, WeaponCategory 2,
        // SpellSchool 3, LearnSpell 4, Skill 5, FeatureSelection 6.
        let param = match pbp.view().int("ParameterType") {
            0 | 1 | 4 | 6 => {
                // `ParamObject.ToString()` throws for a missing object.
                let Some(o) = entry.param_object.filter(|g| w.content.get(*g).is_some()) else {
                    w.coverage.unsupported("AddClassLevels.GetSelectionItem: missing ParamObject");
                    return None;
                };
                FeatureParam::of_blueprint(Some(o))
            }
            2 => FeatureParam::of_weapon_category(entry.param_weapon_category),
            3 => FeatureParam::of_spell_school(entry.param_spell_school),
            5 => FeatureParam::of_stat(entry.stat),
            _ => return None,
        };
        return Some(Item { feature: pf, param: Some(param) });
    }
    if entry.is_mythic_spellbook {
        // `new FeatureUIData(FeatureSelectMythicSpellbook, Spellbook)`: a missing spellbook
        // is the empty param.
        let feature = entry.mythic_spellbook_selection.filter(|g| w.content.get(*g).is_some())?;
        let spellbook = entry.spellbook.filter(|g| w.content.get(*g).is_some());
        return Some(Item { feature, param: Some(FeatureParam::of_blueprint(spellbook)) });
    }
    let sel = w.content.get(st.selections[idx].selection)?;
    for (i, g) in entry.features.iter().enumerate() {
        let is_selection = w.content.get(*g).is_some_and(|b| selection_kind(&b).is_some());
        if (!is_selection || !already_applied_in_progression(w, u, i, entry))
            && !hist.contains(&i)
            && can_select(w, u, st, idx, &sel, &Item { feature: *g, param: None })
        {
            hist.insert(i);
            return Some(Item { feature: *g, param: None });
        }
    }
    None
}

/// `AddClassLevels.PerformSelections(c, controller, history, maxPriority)`.
fn perform_selections(
    w: &mut World,
    u: EntityIdx,
    st: &mut State,
    cfg: &Cfg,
    history: &mut History,
    max_priority: Option<i32>,
) {
    let basic = super::root_ref(w, "Progression", "m_BasicFeatSelection").map(|b| b.guid);
    let deity = super::root_ref(w, "Progression", "m_DeitySelection").map(|b| b.guid);
    let mut i = 0;
    while i < st.selections.len() {
        let idx = i;
        i += 1;
        if st.selections[idx].selected.is_some() {
            continue;
        }
        let sel_guid = st.selections[idx].selection;
        let Some(sel) = w.content.get(sel_guid) else { continue };
        let kind = selection_kind(&sel);
        let group = match kind {
            Some(SelKind::Plain) => sel.view().int("Group"),
            // `BlueprintFeatureSelectMythicSpellbook.GetGroup()` is `SelectMythicSpellbook`;
            // `BlueprintParametrizedFeature.GetGroup()` is `None`.
            Some(SelKind::MythicSpellbook) => GROUP_SELECT_MYTHIC_SPELLBOOK,
            _ => 0,
        };
        if let Some(maxp) = max_priority
            && priority(group) > maxp
        {
            continue;
        }
        // The first entry for this selection that still has features to give.
        let mut found: Option<usize> = None;
        for (ei, entry) in cfg.entries.iter().enumerate() {
            let used = history[ei].as_ref().map_or(0, HashSet::len);
            let unfinished = history[ei].is_none() || !entry.all_selected(used);
            if unfinished && entry.feature_selection() == Some(sel_guid) {
                found = Some(ei);
                break;
            }
        }
        let mut item: Option<Item> = None;
        if let Some(ei) = found {
            let hs = history[ei].get_or_insert_with(HashSet::new);
            item = get_selection_item(w, u, st, idx, &cfg.entries[ei], hs);
        }
        let flag = Some(sel_guid) != basic && Some(sel_guid) != deity;
        if item.is_none() && flag {
            // `selection.Items.FirstOrDefault(item => selection.CanSelect(...))`.
            let items = selection_items(w, &sel, kind.unwrap_or(SelKind::Plain));
            for cand in items {
                if can_select(w, u, st, idx, &sel, &cand) {
                    item = Some(cand);
                    break;
                }
            }
        }
        match item {
            Some(it) => select_feature(w, u, st, idx, &it),
            None if flag => {
                w.log("error", format!("Can't find suitable item for selection {}", sel.name));
            }
            None => {}
        }
    }
}

/// `LevelUpController.SelectFeature` → `AddAction(SelectFeature)`: `Check`, then `Apply`.
fn select_feature(w: &mut World, u: EntityIdx, st: &mut State, idx: usize, item: &Item) {
    // `SelectFeature.Check`: not selected, and the selection allows the item.
    if st.selections[idx].selected.is_some() {
        return;
    }
    let Some(sel) = w.content.get(st.selections[idx].selection) else { return };
    if !can_select(w, u, st, idx, &sel, item) {
        return;
    }
    let Some(f) = w.content.get(item.feature) else { return };
    let kind = selection_kind(&f);
    let sel_state = st.selections[idx].clone();
    // `item.Feature is BlueprintParametrizedFeature && item.Param != null` is not a nested
    // selection.
    let nested = match kind {
        Some(SelKind::Parametrized) if item.param.is_some() => None,
        Some(SelKind::MythicSpellbook) => None,
        other => other,
    };
    let level = sel_state.level;
    if nested.is_some() {
        st.add_selection(Some(idx), sel_state.source, f.guid, level);
    }
    st.selections[idx].selected = Some(item.clone());
    // `Progression.AddSelection(selection as BlueprintFeatureSelection, source, level, item.Feature)`.
    if selection_kind(&sel) == Some(SelKind::Plain) {
        progression_add_selection(w, u, sel.guid, sel_state.source, level, item.feature);
    }
    let mut feature_fact: Option<FactIdx> = None;
    if nested.is_none() || nested == Some(SelKind::Plain) {
        if is_a(f.obj.ty, "BlueprintFeatureReplaceSpellbook") {
            w.coverage.unsupported("SelectFeature.Apply: replace spellbook");
        }
        if kind == Some(SelKind::MythicSpellbook) {
            // `GetReplaceSpellbook`: the item's spellbook replaces the selected class's, and
            // the mythic spell list is added to it.
            let sb = item.param.and_then(|p| p.blueprint);
            let class_data = st
                .selected_class
                .and_then(|c| w.unit(u)?.progression.classes.iter().position(|x| x.class == c));
            if let (Some(sb), Some(ci)) = (sb, class_data) {
                w.unit_mut(u).expect("unit").progression.classes[ci].spellbook = Some(sb);
                if crate::spellbook::get_spellbook(w, u, sb)
                    && let Some(list) = f.view().guid("m_MythicSpellList")
                {
                    crate::spellbook::add_special_list(w, u, sb, list);
                }
            }
        }
        if is_a(f.obj.ty, "BlueprintLevelUpPlanFeaturesList") {
            w.coverage.unsupported("SelectFeature.Apply: level-up plan features list");
        }
        // `unit.AddFact<Feature>(feature, null, item.Param)`.
        feature_fact = match &item.param {
            Some(p) => crate::unit_facts::add_fact_with_param(w, u, &f, p),
            None => crate::unit_facts::add_fact(w, u, &f),
        };
        if let Some(fact) = feature_fact {
            if let Some(s) = sel_state.source {
                set_source(w, fact, s);
            }
            // `feature.IgnorePrerequisites = IgnorePrerequisites.Ignore ||
            // selectionState.IgnorePrerequisites || Selection.IsIgnorePrerequisites()`.
            let ignore =
                st.ignore_prerequisites || selection_ignores_prerequisites(w, st, idx, &sel);
            w.fact_mut(fact).ignore_prerequisites = ignore;
        }
    }
    if is_a(f.obj.ty, "BlueprintProgression") {
        update_progression(w, u, st, &f);
    }
    // `item.Param?.Blueprint is BlueprintProgression && Feature.GetComponent<AddFeatureParametrized>()`.
    if let Some(pb) = item.param.and_then(|p| p.blueprint).and_then(|g| w.content.get(g))
        && is_a(pb.obj.ty, "BlueprintProgression")
        && f.components_array().iter().any(|o| is_a(o.ty, "AddFeatureParametrized"))
    {
        update_progression(w, u, st, &pb);
    }
    // `item.Feature.RestrictPrerequisites(...)`: returns at once under `IgnorePrerequisites`;
    // otherwise only `PrerequisiteAlignment` restricts anything.
    if !st.ignore_prerequisites
        && f.components_array().iter().any(|o| is_a(o.ty, "PrerequisiteAlignment"))
    {
        w.coverage.unsupported("BlueprintFeature.RestrictPrerequisites: PrerequisiteAlignment");
    }
    if feature_fact.is_none() {
        return;
    }
    // `ForEachAddFeatureOnApply`: recursive over `AddFeatureOnApply` components.
    add_feature_on_apply(w, u, st, &f, sel_state.source);
}

/// The tail of `SelectFeature.Apply`: progressions added by `AddFeatureOnApply` components
/// get the selection's source and their levels updated.
fn add_feature_on_apply(
    w: &mut World,
    u: EntityIdx,
    st: &mut State,
    f: &Blueprint,
    source: Option<Guid>,
) {
    for o in f.components_array() {
        if !is_a(o.ty, "AddFeatureOnApply") {
            continue;
        }
        let Some(g) = View(o).guid("m_Feature") else { continue };
        let Some(fb) = w.content.get(g) else { continue };
        if is_a(fb.obj.ty, "BlueprintProgression") {
            let fact =
                w.entity(u).facts.list.iter().copied().find(|x| {
                    w.fact(*x).kind == FactKind::Feature && w.fact(*x).blueprint == Some(g)
                });
            if let Some(fact) = fact
                && w.fact(fact).source_blueprint.is_none()
            {
                if let Some(s) = source {
                    set_source(w, fact, s);
                }
                update_progression(w, u, st, &fb);
            }
        }
        add_feature_on_apply(w, u, st, &fb, source);
    }
}

// ---- spellbooks --------------------------------------------------------------------

/// `ApplySpellbook` (`Actions/ApplySpellbook.cs`): the class's spellbook gets a level,
/// spell selections open. The sim keeps no spellbook state.
fn apply_spellbook(w: &mut World, u: EntityIdx, class: &Rc<Blueprint>, st: &mut State, cfg: &Cfg) {
    let Some(cd) = w
        .unit(u)
        .and_then(|x| x.progression.classes.iter().find(|cd| cd.class == class.guid).cloned())
    else {
        return;
    };
    if cd.spellbook.is_none() {
        return;
    }
    if let Some(skip) =
        class.components_array().into_iter().find(|o| is_a(o.ty, "SkipLevelsForSpellProgression"))
    {
        let levels: Vec<i64> = View(skip)
            .seq("Levels")
            .iter()
            .filter_map(|n| match n {
                wr_content::Node::Int(i) | wr_content::Node::Enum(i) => Some(*i),
                _ => None,
            })
            .collect();
        if levels.contains(&i64::from(st.next_class_level)) {
            return;
        }
    }
    let Some(sb) = cd.spellbook else { return };
    crate::spellbook::demand_spellbook(w, u, sb);
    // A class whose own spellbook isn't the one the unit's class data holds (an archetype
    // replaced it): the old spellbook's spells move over and it goes.
    if let Some(class_sb) = class.view().guid("m_Spellbook")
        && class_sb != sb
        && crate::spellbook::get_spellbook(w, u, class_sb)
    {
        for (level, spell) in crate::spellbook::all_known_spells(w, u, class_sb) {
            crate::spellbook::add_known(w, u, sb, level, spell, true);
        }
        crate::spellbook::delete_spellbook(w, u, class_sb);
    }
    crate::spellbook::add_level_from_class(w, u, sb, class.guid);
    // The spell selections (`DemandSpellSelection`, `SetLevelSpells`, custom spells) feed
    // `SelectSpell`, which only `AddClassLevels.SelectSpells` makes.
    if cfg.select_spells {
        w.coverage.unsupported("AddClassLevels: spell selections");
    }
}

/// `AddClassLevels.PrepareSpellbook`: the class's spellbook resizes its slots and memorizes
/// `MemorizeSpells`.
fn prepare_spellbook(w: &mut World, u: EntityIdx, cfg: &Cfg) {
    let Some(sb) = crate::spellbook::class_spellbook(w, u, cfg.class.guid) else { return };
    crate::spellbook::update_all_slots_size(w, u, sb, true);
    for &spell in &cfg.memorize_spells {
        let min = crate::spellbook::min_spell_level(w, u, sb, spell);
        let data = crate::spellbook::Spell::new(w, spell, min);
        crate::spellbook::memorize(w, u, sb, &data);
    }
}

// ---- skill points ------------------------------------------------------------------

/// `ApplySkillPoints.Apply`.
fn apply_skill_points(w: &mut World, u: EntityIdx, st: &mut State) {
    // `state.IsMythicClassSelected`: no skill points.
    if st.selected_class.and_then(|c| w.content.get(c)).is_some_and(|c| c.view().bool("IsMythic")) {
        st.total_skill_points = 0;
        return;
    }
    let total = total_skill_points(w, u, st.next_character_level);
    let spent: i32 = SKILLS.iter().fold(0i32, |a, s| a.wrapping_add(w.stats(u).v(*s).base_value));
    // `IUnitCalculateSkillPointsOnLevelupHandler` (unit event): extra points.
    w.bus.skill_points.push((st.next_character_level, 0));
    crate::rulebook::bus::raise_unit(w, u, "IUnitCalculateSkillPointsOnLevelupHandler");
    let extra = w.bus.skill_points.pop().map_or(0, |(_, e)| e);
    st.total_skill_points = 1.max(total.wrapping_add(extra).wrapping_sub(spent));
}

/// `LevelUpHelper.GetTotalSkillPoints(unit, nextLevel)`.
fn total_skill_points(w: &mut World, u: EntityIdx, next_level: i32) -> i32 {
    // `Intelligence.CalculatePermanentValueWithoutEnhancement()`: the permanent value
    // minus enhancement modifiers (none on these units: an enhancement is reported).
    let int = w.stats(u).v(StatType::Intelligence);
    let enhanced = int
        .modifiers()
        .any(|m| w.mods[m.i()].desc == crate::stats::types::ModifierDescriptor::Enhancement);
    let permanent = int.permanent_value;
    if enhanced {
        w.coverage.unsupported("GetIntelligenceSkillPoints: enhancement on Intelligence");
    }
    let value = permanent / 2 - 5;
    let (abs, sign) = (value.abs(), value.signum());
    let mut sum = 0i32;
    for level in 1..=next_level {
        sum += if level % 2 == 1 { (abs + 1) / 2 * sign } else { abs / 2 * sign };
    }
    let mut classes = 0i32;
    let cds = w.unit(u).map(|x| x.progression.classes.clone()).unwrap_or_default();
    for cd in cds {
        let Some(c) = w.content.get(cd.class) else { continue };
        if !c.view().bool("IsMythic") {
            // `ClassData.CalcSkillPoints()`: the class's plus the archetypes'.
            let extra: i32 = cd
                .archetypes
                .iter()
                .filter_map(|a| w.content.get(*a))
                .fold(0i32, |a, b| a.wrapping_add(b.view().i32("AddSkillPoints")));
            let per_level = c.view().i32("SkillPoints").wrapping_add(extra);
            classes = classes.wrapping_add(per_level.wrapping_mul(cd.level));
        }
    }
    (sum + classes).max(next_level)
}

/// `SpendSkillPoint.Check` + `Apply`.
fn spend_skill_point(w: &mut World, u: EntityIdx, st: &mut State, s: StatType) {
    if !SKILLS.contains(&s) {
        return;
    }
    let base = w.stats(u).v(s).base_value;
    if base >= st.next_character_level || st.total_skill_points - st.spent_skill_points <= 0 {
        return;
    }
    st.spent_skill_points += 1;
    ops::set_base_value(w, u, s, base + 1);
}

/// `UnitProgressionData.ReapplyFeaturesOnLevelUp()`.
pub fn reapply_features_on_level_up(w: &mut World, u: EntityIdx) {
    let features: Vec<FactIdx> = w
        .entity(u)
        .facts
        .list
        .iter()
        .copied()
        .filter(|f| w.fact(*f).kind == FactKind::Feature)
        .collect();
    for f in features {
        if w.fact(f).is_disposed {
            continue;
        }
        let reapply = w.fact(f).bp.as_ref().is_some_and(|b| b.view().bool("ReapplyOnLevelUp"));
        if reapply {
            crate::facts::lifecycle::reapply(w, f);
        } else {
            crate::context::recalculate(w, f);
        }
    }
    let buffs: Vec<FactIdx> = w
        .entity(u)
        .facts
        .list
        .iter()
        .copied()
        .filter(|f| w.fact(*f).kind == FactKind::Buff)
        .collect();
    for b in buffs {
        crate::context::recalculate(w, b);
    }
    let acts: Vec<FactIdx> = w
        .entity(u)
        .facts
        .list
        .iter()
        .copied()
        .filter(|f| w.fact(*f).kind == FactKind::ActivatableAbility)
        .collect();
    for a in acts {
        crate::unit_facts::reapply_buff(w, a);
    }
    crate::rulebook::bus::raise_unit(w, u, "IUnitReapplyFeaturesOnLevelUpHandler");
}

#[cfg(test)]
mod param_tests {
    use wr_content::Guid;
    use wr_content::node::Node;

    use super::*;
    use crate::facts::tests::{feature, obj};

    fn put(w: &World, g: u128, ty: &str, set: &[(&str, Node)]) -> Rc<Blueprint> {
        w.content.insert(Blueprint {
            guid: Guid(g),
            name: format!("B{g}"),
            obj: obj(ty, set),
            source: crate::content::Source::Bbp,
        });
        w.content.get(Guid(g)).expect("inserted")
    }

    /// LearnSpell items read the list's spells through `SpellLevelList.SpellsFiltered`, whose
    /// `DlcCondition.IsFullfilled` throws for a null reward: the run is reported.
    #[test]
    fn learn_spell_items_report_a_null_dlc_reward() {
        let (mut w, _) = crate::stats::tests::world_with_unit();
        let cond = Node::Object(Box::new(obj(
            "Kingmaker.DLC.DlcCondition",
            &[("name", Node::Str("$dlc$".into()))],
        )));
        feature(&w, 71, vec![cond]);
        let level = Node::Object(Box::new(obj(
            "Kingmaker.Blueprints.Classes.Spells.SpellLevelList",
            &[("SpellLevel", Node::Int(1)), ("m_Spells", Node::Seq(vec![Node::BpRef(Guid(71))]))],
        )));
        put(
            &w,
            70,
            "Kingmaker.Blueprints.Classes.Spells.BlueprintSpellList",
            &[("SpellsByLevel", Node::Seq(vec![level]))],
        );
        let sel = put(
            &w,
            72,
            "Kingmaker.Blueprints.Classes.Selection.BlueprintParametrizedFeature",
            &[("ParameterType", Node::Enum(4)), ("m_SpellList", Node::BpRef(Guid(70)))],
        );
        let items = parametrized_items(&mut w, &sel);
        // Reported, and treated as unrestricted.
        assert!(!w.coverage.valid());
        assert_eq!(items.len(), 1);
    }

    /// A mythic class level (`SelectClass`, `ApplyClassMechanics`, `ApplySkillPoints`): the
    /// mythic level and the unit's mythic experience go up, not the character level or the
    /// experience; hit points get 5; no skill points; `Classes.Sort` puts higher levels first.
    #[test]
    fn a_mythic_level_raises_the_mythic_level_and_gives_five_hit_points() {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        let class = put(&w, 80, "Kingmaker.Blueprints.Classes.BlueprintCharacterClass", &[]);
        let mythic = put(
            &w,
            81,
            "Kingmaker.Blueprints.Classes.BlueprintCharacterClass",
            &[("IsMythic", Node::Bool(true))],
        );
        {
            let p = &mut w.unit_mut(u).expect("unit").progression;
            p.classes.push(crate::unit::ClassData {
                class: class.guid,
                level: 1,
                archetypes: Vec::new(),
                spellbook: None,
            });
            p.experience = 7;
            // The caches as a constructed unit has them (`SetupLevelsIfNecessary` done).
            p.character_level = Some(1);
            p.mythic_level = Some(0);
        }
        let hp = w.stats(u).v(StatType::HitPoints).base_value;
        let mut levels = Vec::new();
        for _ in 0..2 {
            let mut st = State { next_character_level: 2, ..State::default() };
            select_class(&mut w, u, &mythic, &mut st);
            apply_class_mechanics(&mut w, u, &mythic, &mut st);
            apply_skill_points(&mut w, u, &mut st);
            assert_eq!(st.total_skill_points, 0);
            assert_eq!(st.attribute_points, 0);
            levels.push(st.next_class_level);
        }
        assert_eq!(levels, [1, 2]);
        assert_eq!(crate::unit::mythic_level(&w, u), 2);
        assert_eq!(crate::unit::character_level(&w, u), 1);
        let p = &w.unit(u).expect("unit").progression;
        assert_eq!((p.mythic_experience, p.experience), (2, 7));
        // Sorted after each level: the mythic class (2) before the other (1).
        let order: Vec<(Guid, i32)> = p.classes.iter().map(|c| (c.class, c.level)).collect();
        assert_eq!(order, [(mythic.guid, 2), (class.guid, 1)]);
        assert_eq!(w.stats(u).v(StatType::HitPoints).base_value, hp + 10);
        // A hydrated unit's caches are unset: read before the class data changes
        // (`AddClassLevels.LevelUp`'s limit check), they count the levels once.
        let (mut w2, u2) = crate::stats::tests::world_with_unit();
        let mythic2 = put(
            &w2,
            81,
            "Kingmaker.Blueprints.Classes.BlueprintCharacterClass",
            &[("IsMythic", Node::Bool(true))],
        );
        assert_eq!(w2.unit(u2).expect("unit").progression.mythic_level, None);
        crate::unit::setup_levels_if_necessary(&mut w2, u2);
        let mut st = State { next_character_level: 1, ..State::default() };
        select_class(&mut w2, u2, &mythic2, &mut st);
        assert_eq!(crate::unit::mythic_level(&w2, u2), 1);
        assert_eq!(w2.unit(u2).expect("unit").progression.mythic_experience, 1);
        // (The hit points' size bonus reads the root, which the test world doesn't have.)
        let left: Vec<&String> = w.coverage.unsupported.keys().collect();
        assert_eq!(left, ["BlueprintRoot not found"]);
    }
}
