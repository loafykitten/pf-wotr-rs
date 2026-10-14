//! `FeatureParam` (`Kingmaker.Blueprints.Classes.Selection/FeatureParam.cs`, decompiled, not
//! copied): what a parametrized feature is parametrized by, its persisted form (`Feature.Param`
//! as `ParamConverter`, which skips an empty value and null members), equality, and
//! `UnitDescriptor.GetFeature(blueprint, param)`. Also `WeaponCategoryExtension.HasSubCategory`
//! (the per-category sub-category data), which the parametrized feature's items use.

use serde_json::{Map, Value};
use wr_content::Guid;

use crate::world::{EntityIdx, FactIdx, FactKind, World};

/// A `FeatureParam`: at most one of its members is normally set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FeatureParam {
    pub blueprint: Option<Guid>,
    pub weapon_category: Option<i64>,
    pub spell_school: Option<i64>,
    pub stat_type: Option<i64>,
}

const WEAPON_CATEGORY: &str = "Kingmaker.Enums.WeaponCategory";
const SPELL_SCHOOL: &str = "Kingmaker.Blueprints.Classes.Spells.SpellSchool";
const STAT_TYPE: &str = "Kingmaker.EntitySystem.Stats.StatType";

fn enum_members(name: &str) -> &'static [(&'static str, i64)] {
    wr_content::schema::enums().iter().find(|e| e.name == name).map_or(&[], |e| e.members)
}

fn enum_json(name: &str, v: i64) -> Value {
    match enum_members(name).iter().find(|(_, x)| *x == v) {
        Some((n, _)) => Value::String((*n).to_owned()),
        None => Value::from(v),
    }
}

fn enum_from_json(name: &str, v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => enum_members(name).iter().find(|(n, _)| n == s).map(|(_, x)| *x),
        _ => None,
    }
}

impl FeatureParam {
    #[must_use]
    pub fn of_blueprint(g: Option<Guid>) -> Self {
        Self { blueprint: g, ..Self::default() }
    }

    #[must_use]
    pub fn of_weapon_category(c: i64) -> Self {
        Self { weapon_category: Some(c), ..Self::default() }
    }

    #[must_use]
    pub fn of_spell_school(s: i64) -> Self {
        Self { spell_school: Some(s), ..Self::default() }
    }

    #[must_use]
    pub fn of_stat(s: i64) -> Self {
        Self { stat_type: Some(s), ..Self::default() }
    }

    /// `FeatureParam.IsEmpty`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The persisted form (`Feature.ParamConverter`): `None` for an empty param, else the
    /// members that are set (`NullValueHandling.Ignore`).
    #[must_use]
    pub fn to_json(&self) -> Option<Value> {
        if self.is_empty() {
            return None;
        }
        let mut m = Map::new();
        if let Some(g) = self.blueprint {
            m.insert("Blueprint".into(), Value::String(g.to_string()));
        }
        if let Some(c) = self.weapon_category {
            m.insert("WeaponCategory".into(), enum_json(WEAPON_CATEGORY, c));
        }
        if let Some(s) = self.spell_school {
            m.insert("SpellSchool".into(), enum_json(SPELL_SCHOOL, s));
        }
        if let Some(s) = self.stat_type {
            m.insert("StatType".into(), enum_json(STAT_TYPE, s));
        }
        Some(Value::Object(m))
    }

    /// Reads a persisted param; `None` when its form isn't recognized.
    #[must_use]
    pub fn from_json(v: &Value) -> Option<Self> {
        let o = v.as_object()?;
        let mut p = Self::default();
        for (k, x) in o {
            if x.is_null() {
                continue;
            }
            match k.as_str() {
                "Blueprint" => p.blueprint = Some(Guid::parse(x.as_str()?)?),
                "WeaponCategory" => p.weapon_category = Some(enum_from_json(WEAPON_CATEGORY, x)?),
                "SpellSchool" => p.spell_school = Some(enum_from_json(SPELL_SCHOOL, x)?),
                "StatType" => p.stat_type = Some(enum_from_json(STAT_TYPE, x)?),
                _ => return None,
            }
        }
        Some(p)
    }
}

/// `Feature.Param` of a fact (an unset one is the empty param). A saved form that isn't
/// recognized is reported and read as empty.
pub fn fact_param(w: &mut World, f: FactIdx) -> FeatureParam {
    let Some(v) = w.fact(f).param.clone() else { return FeatureParam::default() };
    FeatureParam::from_json(&v).unwrap_or_else(|| {
        w.coverage.unsupported("FeatureParam: unrecognized persisted form");
        FeatureParam::default()
    })
}

/// `UnitDescriptor.GetFeature(blueprint, param)`: the first feature of the blueprint with an
/// equal param.
pub fn get_feature(
    w: &mut World,
    u: EntityIdx,
    blueprint: Guid,
    param: &FeatureParam,
) -> Option<FactIdx> {
    let facts: Vec<FactIdx> = w.entity(u).facts.list.clone();
    facts.into_iter().find(|f| {
        w.fact(*f).kind == FactKind::Feature
            && w.fact(*f).blueprint == Some(blueprint)
            && fact_param(w, *f) == *param
    })
}

/// `WeaponSubCategory`: `None` 0, `Ranged` 1, `Melee` 2, `Finessable` 3, `Thrown` 4,
/// `Natural` 5, `Knives` 6, `Monk` 7, `TwoHanded` 8, `Light` 9, `Simple` 10, `Martial` 11,
/// `Exotic` 12, `OneHandedPiercing` 13, `Disabled` 14, `OneHandedSlashing` 15, `Metal` 16.
pub const SUB_DISABLED: i64 = 14;

/// The sub-categories of each weapon category, by `WeaponCategory` value, as letters: r Ranged,
/// m Melee, f Finessable, t Thrown, n Natural, k Knives, o Monk, h TwoHanded, l Light,
/// s Simple, a Martial, e Exotic, p OneHandedPiercing, d Disabled, c OneHandedSlashing,
/// x Metal.
const SUB_CATEGORIES: [&str; 84] = [
    "mfo", "smfklpx", "smflx", "smfklpx", "smflcx", "sm", "smx", "smpx", "smh", "smhx", "smoh",
    "smhx", "amhpx", "srx", "srx", "srx", "srx", "dr", "amflcx", "amfklcx", "amflx", "amflpx",
    "amflcx", "amfklpx", "amflx", "amflpx", "amcx", "amx", "ampx", "amcx", "amfpx", "amcx", "amx",
    "amx", "ampx", "amhx", "amhx", "amhx", "amhx", "amhx", "amhx", "amhx", "arx", "arx", "emfolcx",
    "emfol", "emfkolpx", "dmfolpx", "emcx", "emfcx", "emcx", "emfpx", "emcx", "empx", "emfhx",
    "emhx", "drx", "drx", "drx", "drox", "er", "mf", "r", "rt", "mnfp", "mnfp", "mnf", "mnfd",
    "amhx", "emx", "emx", "emx", "emx", "f", "arx", "mnf", "mnf", "mnf", "mnf", "mnf", "mn", "rn",
    "mnf", "emflcx",
];

fn sub_letter(sub: i64) -> Option<char> {
    Some(match sub {
        1 => 'r',
        2 => 'm',
        3 => 'f',
        4 => 't',
        5 => 'n',
        6 => 'k',
        7 => 'o',
        8 => 'h',
        9 => 'l',
        10 => 's',
        11 => 'a',
        12 => 'e',
        13 => 'p',
        14 => 'd',
        15 => 'c',
        16 => 'x',
        _ => return None,
    })
}

/// `WeaponCategoryExtension.HasSubCategory(category, sub)`: every category has `None`.
#[must_use]
pub fn has_sub_category(category: i64, sub: i64) -> bool {
    if sub == 0 {
        return true;
    }
    let Some(letter) = sub_letter(sub) else { return false };
    usize::try_from(category)
        .ok()
        .and_then(|c| SUB_CATEGORIES.get(c))
        .is_some_and(|s| s.contains(letter))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_weapons_match_the_natural_proficiencies() {
        // The 12 natural categories `EnsureNaturalProficiencies` adds.
        let natural: Vec<i64> = (0..84).filter(|c| has_sub_category(*c, 5)).collect();
        assert_eq!(natural, vec![64, 65, 66, 67, 75, 76, 77, 78, 79, 80, 81, 82]);
    }

    #[test]
    fn disabled_categories() {
        let d: Vec<i64> = (0..84).filter(|c| has_sub_category(*c, SUB_DISABLED)).collect();
        assert_eq!(d, vec![17, 47, 56, 57, 58, 59, 67]);
        assert!(has_sub_category(29, 0));
        assert!(has_sub_category(29, 11) && !has_sub_category(29, 10));
    }

    #[test]
    fn absent_and_empty_persisted_params_are_empty() {
        let read = |v: Option<serde_json::Value>| {
            v.as_ref().and_then(FeatureParam::from_json).unwrap_or_default().is_empty()
        };
        assert!(read(None));
        assert!(read(Some(serde_json::json!({}))));
        assert!(read(Some(serde_json::json!({"Blueprint": null}))));
        assert!(!read(Some(serde_json::json!({"WeaponCategory": "Dagger"}))));
    }

    #[test]
    fn an_empty_param_has_no_scope_json() {
        // `add_fact_with_param` installs a scope only for a param with persisted members.
        assert!(FeatureParam::default().to_json().is_none());
        assert!(FeatureParam::of_blueprint(None).to_json().is_none());
    }

    #[test]
    fn param_json_round_trip() {
        let p = FeatureParam::of_weapon_category(29);
        let j = p.to_json().unwrap();
        assert_eq!(j, serde_json::json!({"WeaponCategory": "Longsword"}));
        assert_eq!(FeatureParam::from_json(&j), Some(p));
        assert!(FeatureParam::default().to_json().is_none());
        let s = FeatureParam::of_spell_school(5);
        assert_eq!(s.to_json().unwrap(), serde_json::json!({"SpellSchool": "Evocation"}));
        assert_eq!(
            FeatureParam::from_json(&serde_json::json!({"Blueprint": null})),
            Some(FeatureParam::default())
        );
        assert_eq!(FeatureParam::from_json(&serde_json::json!({"Other": 1})), None);
    }
}
