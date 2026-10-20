//! `ModifiableValue` and its subclasses' state (`Kingmaker.EntitySystem.Stats`). The
//! algorithms are in [`super::ops`].

use serde::Serialize;

use super::types::{BonusMod, HitPointsBaseStat, ModifierDescriptor, StatType};
use crate::csharp::CsDict;
use crate::world::{CompRef, FactIdx, ModIdx};

/// A `List<Modifier>` in `ModifierList`, with its capacity (for `Sort`'s depth limit).
#[derive(Debug, Clone, Default)]
pub struct ModList {
    pub items: Vec<ModIdx>,
    pub capacity: usize,
}

impl ModList {
    pub fn push(&mut self, m: ModIdx) {
        if self.items.len() == self.capacity {
            self.capacity = if self.capacity == 0 { 4 } else { self.capacity * 2 };
        }
        self.items.push(m);
    }
}

/// An `OnChanged` subscriber (`event Action<ModifiableValue, int>`), in subscription
/// order. The subscribers the stat classes install themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ChangedHandler {
    /// `ModifiableValueArmorClass.AttributeChangeHandler` of the unit's AC.
    AcAttribute,
    /// `ModifiableValueSkill.AttributeChangeHandler` of a skill.
    SkillAttribute(StatType),
    /// The `OnChanged +=` of `Spellbook.PostLoad` on the casting stat, by the spellbook's
    /// identity.
    Spellbook(u32),
    /// `UnitStatusEffectHelpless.UnitStatHandler` on the Dexterity.
    Helpless,
    /// The `OnChanged +=` of `MaxDexBonusIncrease.OnTurnOn`: the component's fact and index.
    MaxDexBonus(u32, u32),
}

/// `ModifiableValueArmorClass.DexBonusLimiter`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DexBonusLimiter {
    pub value: i32,
    /// `SourceType`: 0 Invalid, 1 Armor, 2 Shield, 3 Encumbrance.
    pub source: i32,
    /// Identity, for reference-equality removal.
    pub id: u32,
}

/// What subclass a stat is, with the subclass's own state.
#[derive(Debug, Clone)]
pub enum StatKind {
    Plain,
    Attribute {
        /// `Disabled` (`CountableFlag` count).
        disabled: i32,
        damage: Option<ModIdx>,
        drain: Option<ModIdx>,
    },
    Skill {
        /// `m_BaseAttribute` (saved).
        base_attribute: StatType,
        /// `BaseAttribute` (computed by `CheckAttribute`).
        current_base: StatType,
        replace: StatType,
        replace_if_higher: bool,
        background_modifier: Option<ModIdx>,
        /// `m_ClassSkill` count.
        class_skill: i32,
        background_source: Option<FactIdx>,
    },
    SavingThrow {
        base_stat: StatType,
    },
    Dependant {
        base_stat: StatType,
    },
    Initiative {
        dexterity: StatType,
        dex_bonus: Option<ModIdx>,
        min_value: i32,
    },
    HitPoints {
        base_stat: HitPointsBaseStat,
    },
    TemporaryHitPoints,
    Speed {
        racial: i32,
    },
    ArmorClass(Box<AcState>),
}

#[derive(Debug, Clone)]
pub struct AcState {
    pub limiters: Option<Vec<DexBonusLimiter>>,
    pub applied_bonus: Option<ModIdx>,
    pub replace: StatType,
    pub replace_if_higher: bool,
    pub replace_bonus_mod: BonusMod,
    /// `BaseAttribute` (computed).
    pub base_attribute: StatType,
    pub update_only_base: bool,
    pub flat_footed: i32,
    pub touch: i32,
    pub flat_footed_touch: i32,
    pub flat_footed_permanent: i32,
    pub touch_permanent: i32,
    pub flat_footed_touch_permanent: i32,
    pub armor_ac: i32,
    pub shield_ac: i32,
    pub dodge_ac: i32,
}

impl Default for AcState {
    fn default() -> Self {
        Self {
            limiters: None,
            applied_bonus: None,
            replace: StatType::Unknown,
            replace_if_higher: false,
            replace_bonus_mod: BonusMod::AsIs,
            base_attribute: StatType::Dexterity,
            update_only_base: false,
            flat_footed: 0,
            touch: 0,
            flat_footed_touch: 0,
            flat_footed_permanent: 0,
            touch_permanent: 0,
            flat_footed_touch_permanent: 0,
            armor_ac: 0,
            shield_ac: 0,
            dodge_ac: 0,
        }
    }
}

/// One `ModifiableValue`.
#[derive(Debug, Clone)]
pub struct StatValue {
    pub ty: StatType,
    pub kind: StatKind,
    /// `m_BaseValue` (saved).
    pub base_value: i32,
    /// `PermanentValue` (saved, then recomputed).
    pub permanent_value: i32,
    pub modified_value: i32,
    pub modified_value_raw: i32,
    pub no_morale: i32,
    pub no_morale_no_insight: i32,
    /// `ModifierList`: `Dictionary<ModifierDescriptor, List<Modifier>>`, in Mono order.
    pub modifier_list: CsDict<ModifierDescriptor, ModList>,
    /// `m_Dependents` (saved): stats updated after this one.
    pub dependents: Option<Vec<StatType>>,
    pub dependent_facts: Vec<FactIdx>,
    pub dependent_components: Vec<CompRef>,
    pub on_changed: Vec<ChangedHandler>,
    pub updating_internal: bool,
    /// Inside the outermost `UpdateValue` (WrathOracle's `Hooks.Updating`).
    pub in_update: bool,
    pub updating_dependent_facts: bool,
    pub temporary_scopes: i32,
    /// `MoraleBonusesDisabled`, `InsightBonusesDisabled` counts.
    pub morale_disabled: i32,
    pub insight_disabled: i32,
    /// Bound by `PostLoad` (`m_Stats`).
    pub bound: bool,
}

impl StatValue {
    #[must_use]
    pub fn new(ty: StatType, kind: StatKind) -> Self {
        Self {
            ty,
            kind,
            base_value: 0,
            permanent_value: 0,
            modified_value: 0,
            modified_value_raw: 0,
            no_morale: 0,
            no_morale_no_insight: 0,
            modifier_list: CsDict::new(),
            dependents: None,
            dependent_facts: Vec::new(),
            dependent_components: Vec::new(),
            on_changed: Vec::new(),
            updating_internal: false,
            in_update: false,
            updating_dependent_facts: false,
            temporary_scopes: 0,
            morale_disabled: 0,
            insight_disabled: 0,
            bound: false,
        }
    }

    /// `MinValue`.
    #[must_use]
    pub fn min_value(&self) -> i32 {
        match &self.kind {
            StatKind::Attribute { .. } | StatKind::HitPoints { .. } | StatKind::ArmorClass(_) => 1,
            StatKind::Speed { .. } => 5,
            StatKind::Initiative { min_value, .. } => *min_value,
            _ => i32::MIN,
        }
    }

    /// Modifiers in `ModifierList` order (`Modifiers`).
    pub fn modifiers(&self) -> impl Iterator<Item = ModIdx> + '_ {
        self.modifier_list.iter().flat_map(|(_, l)| l.items.iter().copied())
    }
}

/// `CharacterStats`: the unit's stats in `CollectAllStats` order.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub all: Vec<StatValue>,
}

impl Stats {
    #[must_use]
    pub fn get(&self, t: StatType) -> Option<&StatValue> {
        self.all.iter().find(|s| s.ty == t)
    }

    pub fn get_mut(&mut self, t: StatType) -> Option<&mut StatValue> {
        self.all.iter_mut().find(|s| s.ty == t)
    }

    /// # Panics
    ///
    /// If the unit lacks the stat (every `CharacterStats` has all 38).
    #[must_use]
    pub fn v(&self, t: StatType) -> &StatValue {
        self.get(t).unwrap_or_else(|| panic!("stat {t:?}"))
    }

    /// # Panics
    ///
    /// As [`Self::v`].
    pub fn m(&mut self, t: StatType) -> &mut StatValue {
        self.get_mut(t).unwrap_or_else(|| panic!("stat {t:?}"))
    }

    /// A fresh `CharacterStats(UnitDescriptor)`: the constructor's kinds and wiring
    /// (dependents and `OnChanged` subscriptions), without computing values.
    #[must_use]
    pub fn fresh() -> Self {
        use StatType as S;
        let mut all: Vec<StatValue> = S::ALL
            .iter()
            .map(|&t| {
                let kind = match t {
                    S::Strength
                    | S::Dexterity
                    | S::Constitution
                    | S::Intelligence
                    | S::Wisdom
                    | S::Charisma => StatKind::Attribute { disabled: 0, damage: None, drain: None },
                    S::HitPoints => {
                        StatKind::HitPoints { base_stat: HitPointsBaseStat::Constitution }
                    }
                    S::TemporaryHitPoints => StatKind::TemporaryHitPoints,
                    S::AC => StatKind::ArmorClass(Box::default()),
                    S::SaveFortitude => StatKind::SavingThrow { base_stat: S::Constitution },
                    S::SaveReflex => StatKind::SavingThrow { base_stat: S::Dexterity },
                    S::SaveWill => StatKind::SavingThrow { base_stat: S::Wisdom },
                    S::Speed => StatKind::Speed { racial: 0 },
                    S::Initiative => StatKind::Initiative {
                        dexterity: S::Dexterity,
                        dex_bonus: None,
                        min_value: i32::MIN,
                    },
                    S::CheckBluff | S::CheckDiplomacy | S::CheckIntimidate => {
                        StatKind::Dependant { base_stat: S::SkillPersuasion }
                    }
                    t if t.is_skill() => {
                        let a = default_skill_attribute(t);
                        StatKind::Skill {
                            base_attribute: a,
                            current_base: a,
                            replace: S::Unknown,
                            replace_if_higher: false,
                            background_modifier: None,
                            class_skill: 0,
                            background_source: None,
                        }
                    }
                    _ => StatKind::Plain,
                };
                StatValue::new(t, kind)
            })
            .collect();
        // Constructor order of `AddDependentValue` and `OnChanged +=` calls.
        let order: [(S, S); 21] = [
            (S::Constitution, S::HitPoints),
            (S::Dexterity, S::AC),
            (S::Constitution, S::SaveFortitude),
            (S::Dexterity, S::SaveReflex),
            (S::Wisdom, S::SaveWill),
            (S::Dexterity, S::SkillMobility),
            (S::Wisdom, S::SkillPerception),
            (S::Dexterity, S::SkillThievery),
            (S::Strength, S::SkillAthletics),
            (S::Charisma, S::SkillPersuasion),
            (S::Wisdom, S::SkillLoreNature),
            (S::Intelligence, S::SkillKnowledgeArcana),
            (S::Wisdom, S::SkillLoreReligion),
            (S::Intelligence, S::SkillKnowledgeWorld),
            (S::Dexterity, S::SkillStealth),
            (S::Charisma, S::SkillUseMagicDevice),
            (S::SkillPersuasion, S::CheckBluff),
            (S::SkillPersuasion, S::CheckDiplomacy),
            (S::SkillPersuasion, S::CheckIntimidate),
            (S::Dexterity, S::Initiative),
            (S::Unknown, S::Unknown),
        ];
        for (base, dep) in order {
            if base == S::Unknown {
                continue;
            }
            let b = all.iter_mut().find(|s| s.ty == base).expect("stat");
            b.dependents.get_or_insert_with(Vec::new).push(dep);
            if dep == S::AC {
                b.on_changed.push(ChangedHandler::AcAttribute);
            } else if dep.is_skill() {
                b.on_changed.push(ChangedHandler::SkillAttribute(dep));
            }
        }
        for s in &mut all {
            s.bound = true;
            if s.ty == S::AC {
                s.base_value = 10;
            }
            if s.ty == S::AttackOfOpportunityCount {
                s.base_value = 1;
            }
        }
        Self { all }
    }
}

/// The attribute `CharacterStats`' constructor gives each skill.
#[must_use]
pub fn default_skill_attribute(t: StatType) -> StatType {
    use StatType as S;
    match t {
        S::SkillMobility | S::SkillThievery | S::SkillStealth => S::Dexterity,
        S::SkillAthletics => S::Strength,
        S::SkillPerception | S::SkillLoreNature | S::SkillLoreReligion => S::Wisdom,
        S::SkillKnowledgeArcana | S::SkillKnowledgeWorld => S::Intelligence,
        _ => S::Charisma,
    }
}
