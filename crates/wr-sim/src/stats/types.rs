//! `StatType`, `ModifierDescriptor` and `ModifiableValue.StackMode`
//! (`Kingmaker.EntitySystem.Stats/StatType.cs`, `Kingmaker.Enums/ModifierDescriptor.cs`,
//! `ModifierDescriptorHelper.cs`), as integer newtypes so values from saves and blueprints
//! that aren't named members survive.

use serde::Serialize;

macro_rules! int_enum {
    ($(#[$m:meta])* $name:ident { $($v:ident = $n:expr),* $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
        pub struct $name(pub i32);
        #[allow(non_upper_case_globals)]
        impl $name {
            $(pub const $v: Self = Self($n);)*
            /// The member name, or the number.
            #[must_use]
            pub fn name(self) -> std::borrow::Cow<'static, str> {
                match self.0 {
                    $($n => std::borrow::Cow::Borrowed(stringify!($v)),)*
                    n => std::borrow::Cow::Owned(n.to_string()),
                }
            }
        }
    };
}

int_enum!(
    /// `Kingmaker.EntitySystem.Stats.StatType`.
    StatType {
        Unknown = 0, Strength = 1, Dexterity = 2, Constitution = 3, Intelligence = 4,
        Wisdom = 5, Charisma = 6, BaseAttackBonus = 7, AdditionalAttackBonus = 8,
        AdditionalDamage = 9, HitPoints = 10, AC = 11, AdditionalCMB = 12, AdditionalCMD = 13,
        SaveFortitude = 14, SaveWill = 15, SaveReflex = 16, SkillMobility = 17,
        AttackOfOpportunityCount = 18, SkillAthletics = 19, SkillPerception = 20,
        SneakAttack = 21, TemporaryHitPoints = 22, Reach = 23, SkillKnowledgeArcana = 24,
        Initiative = 26, SkillThievery = 27, Speed = 28, SkillPersuasion = 29,
        SkillLoreNature = 35, SkillStealth = 42, SkillUseMagicDevice = 43,
        SkillLoreReligion = 45, SkillKnowledgeWorld = 48, CheckBluff = 101,
        CheckDiplomacy = 102, CheckIntimidate = 103, DamageNonLethal = 104,
        BonusCasterLevel = 105,
    }
);

int_enum!(
    /// `Kingmaker.Enums.ModifierDescriptor` (`ShieldFocus` is an alias of `Focus`).
    ModifierDescriptor {
        None = 0, Racial = 1, Dodge = 2, Competence = 3, Armor = 4, Shield = 5,
        Alchemical = 6, Circumstance = 7, Deflection = 8, Enhancement = 9,
        ArmorEnhancement = 10, ShieldEnhancement = 11, Inherent = 12, Insight = 13, Luck = 14,
        Morale = 15, NaturalArmor = 16, NaturalArmorEnhancement = 17, Profane = 18,
        Sacred = 19, Size = 20, Trait = 21, Resistance = 22, FearPenalty = 23,
        NegativeEnergyPenalty = 24, UntypedStackable = 25, DexterityBonus = 26,
        ConstitutionBonus = 27, Fatigued = 28, Crippled = 29, Feat = 30, StatDamage = 31,
        StatDrain = 32, Focus = 33, Difficulty = 34, BaseStatBonus = 35, Penalty = 36,
        ArmorFocus = 37, Cooking = 38, Polymorph = 39, Helpless = 40, Encumbrance = 41,
        FavoredEnemy = 42, Other = 43, Prone = 44, Mythic = 45, DemonBonus = 46, Rage = 47,
        UniqueItem = 48, LockpickersKit = 49, FavouredClassBonus = 50, NaturalArmorForm = 51,
        Anomaly = 52, DLC3_Stackable = 53, WeaponTraining = 54, MasterShapeshifter = 55,
        SelfBonus = 56, Haste = 57,
    }
);

impl ModifierDescriptor {
    /// `ModifierDescriptorHelper.IsStackable`: membership of `DefaultStackingDescriptors`.
    #[must_use]
    pub fn is_stackable(self) -> bool {
        matches!(
            self,
            Self::None
                | Self::Racial
                | Self::Dodge
                | Self::UntypedStackable
                | Self::Feat
                | Self::Penalty
                | Self::ArmorFocus
                | Self::Focus
                | Self::NegativeEnergyPenalty
                | Self::NaturalArmor
                | Self::DLC3_Stackable
        )
    }
}

int_enum!(
    /// `ModifiableValue.StackMode`.
    StackMode { Default = 0, ForceStack = 1, ForceNonStack = 2 }
);

impl StatType {
    /// `StatTypeHelper.IsAttribute`.
    #[must_use]
    pub fn is_attribute(self) -> bool {
        (1..=6).contains(&self.0)
    }

    /// `StatTypeHelper.IsSkill`.
    #[must_use]
    pub fn is_skill(self) -> bool {
        matches!(
            self,
            Self::SkillMobility
                | Self::SkillAthletics
                | Self::SkillPersuasion
                | Self::SkillThievery
                | Self::SkillLoreNature
                | Self::SkillPerception
                | Self::SkillStealth
                | Self::SkillUseMagicDevice
                | Self::SkillLoreReligion
                | Self::SkillKnowledgeWorld
                | Self::SkillKnowledgeArcana
        )
    }

    /// `CharacterStats.CollectAllStats` order: also the order `PostLoad` binds and then
    /// updates stats.
    pub const ALL: [Self; 38] = [
        Self::Strength,
        Self::Dexterity,
        Self::Constitution,
        Self::Intelligence,
        Self::Wisdom,
        Self::Charisma,
        Self::BaseAttackBonus,
        Self::AdditionalAttackBonus,
        Self::AC,
        Self::AdditionalCMB,
        Self::AdditionalCMD,
        Self::AdditionalDamage,
        Self::SaveFortitude,
        Self::SaveWill,
        Self::SaveReflex,
        Self::SkillMobility,
        Self::SkillAthletics,
        Self::SkillPerception,
        Self::SkillThievery,
        Self::SkillLoreNature,
        Self::SkillKnowledgeArcana,
        Self::SkillPersuasion,
        Self::SkillStealth,
        Self::SkillUseMagicDevice,
        Self::SkillLoreReligion,
        Self::SkillKnowledgeWorld,
        Self::CheckBluff,
        Self::CheckDiplomacy,
        Self::CheckIntimidate,
        Self::Initiative,
        Self::AttackOfOpportunityCount,
        Self::Speed,
        Self::SneakAttack,
        Self::Reach,
        Self::BonusCasterLevel,
        Self::HitPoints,
        Self::TemporaryHitPoints,
        Self::DamageNonLethal,
    ];

    /// The JSON member of `CharacterStats` holding this stat (its C# field name).
    #[must_use]
    pub fn member(self) -> std::borrow::Cow<'static, str> {
        self.name()
    }
}

/// `ModifierDescriptorComparer.SortedValues`: the display order of descriptors.
pub const DESCRIPTOR_DISPLAY_ORDER: [ModifierDescriptor; 45] = [
    ModifierDescriptor::None,
    ModifierDescriptor::Racial,
    ModifierDescriptor::Inherent,
    ModifierDescriptor::Trait,
    ModifierDescriptor::Size,
    ModifierDescriptor::Insight,
    ModifierDescriptor::Profane,
    ModifierDescriptor::Sacred,
    ModifierDescriptor::Luck,
    ModifierDescriptor::Haste,
    ModifierDescriptor::Circumstance,
    ModifierDescriptor::Enhancement,
    ModifierDescriptor::Morale,
    ModifierDescriptor::Competence,
    ModifierDescriptor::Resistance,
    ModifierDescriptor::Cooking,
    ModifierDescriptor::Feat,
    ModifierDescriptor::FavoredEnemy,
    ModifierDescriptor::UntypedStackable,
    ModifierDescriptor::DLC3_Stackable,
    ModifierDescriptor::ConstitutionBonus,
    ModifierDescriptor::Difficulty,
    ModifierDescriptor::Polymorph,
    ModifierDescriptor::Other,
    ModifierDescriptor::DexterityBonus,
    ModifierDescriptor::Deflection,
    ModifierDescriptor::Armor,
    ModifierDescriptor::ArmorEnhancement,
    ModifierDescriptor::ArmorFocus,
    ModifierDescriptor::Shield,
    ModifierDescriptor::ShieldEnhancement,
    ModifierDescriptor::Focus,
    ModifierDescriptor::NaturalArmor,
    ModifierDescriptor::NaturalArmorEnhancement,
    ModifierDescriptor::Dodge,
    ModifierDescriptor::Encumbrance,
    ModifierDescriptor::Penalty,
    ModifierDescriptor::StatDamage,
    ModifierDescriptor::StatDrain,
    ModifierDescriptor::Fatigued,
    ModifierDescriptor::Crippled,
    ModifierDescriptor::FearPenalty,
    ModifierDescriptor::NegativeEnergyPenalty,
    ModifierDescriptor::Helpless,
    ModifierDescriptor::Prone,
];

/// `HitPointsBaseStat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum HitPointsBaseStat {
    Constitution,
    Charisma,
    Size,
}

/// `Kingmaker.Enums.Size`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Size {
    Fine,
    Diminutive,
    Tiny,
    Small,
    Medium,
    Large,
    Huge,
    Gargantuan,
    Colossal,
}

impl Size {
    #[must_use]
    pub fn from_i64(v: i64) -> Option<Self> {
        Some(match v {
            0 => Self::Fine,
            1 => Self::Diminutive,
            2 => Self::Tiny,
            3 => Self::Small,
            4 => Self::Medium,
            5 => Self::Large,
            6 => Self::Huge,
            7 => Self::Gargantuan,
            8 => Self::Colossal,
            _ => return None,
        })
    }
}

/// `BonusMod` (AC and skill base-attribute replacement).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BonusMod {
    AsIs,
    Div2,
    OnePlusDiv2,
    Custom,
}
