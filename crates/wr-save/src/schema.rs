//! The shape of the generated save schema (`generated/schema.rs`, written by
//! `wr-codegen gen-save`) and lookups over it.
//!
//! Each [`TypeDef`] is one closed CLR type a save document can hold: its members follow
//! `OptInContractResolver` (fields and properties with `[JsonProperty]`, base classes
//! included; Newtonsoft's default contract for Unity core types), and each member's
//! [`Codec`] is the converter `DefaultJsonSettings` would pick for its declared type, or
//! the contract when no converter can read it.

use crate::generated::schema::{BY_NAME, ENUMS, FINGERPRINT, HOOKS, TYPES};

pub type TypeIdx = u32;
pub const NO_TYPE: TypeIdx = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SeqKind {
    Array,
    List,
    HashSet,
    /// `Stack<T>`: Newtonsoft rebuilds it from a list, reversing the order.
    Stack,
    Queue,
    Other,
}

/// `EntityRefConverter` targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKind {
    Unit,
    MapObject,
    Entity,
    Typed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Bool,
    Int {
        bits: u8,
        signed: bool,
    },
    F32,
    F64,
    Decimal,
    Char,
    Str,
    Guid,
    TimeSpan,
    DateTime,
    DateTimeOffset,
    Version,
    Bytes,
    /// `StringEnumConverter`; index into [`ENUMS`].
    Enum(u32),
    Nullable(&'static Codec),
    /// An object contract: index into [`TYPES`] (the declared type).
    Object(TypeIdx),
    /// `BlueprintConverter`: the declared blueprint type.
    Blueprint(TypeIdx),
    /// `BlueprintRefConverter`: the referenced blueprint type.
    BpRef(TypeIdx),
    BlueprintGuid,
    EntityRef(RefKind),
    Vector2,
    Vector3,
    Quaternion,
    BoneTransform,
    Bounds,
    CountableFlag,
    Sprite,
    StringEntry,
    Dict {
        key: &'static Codec,
        value: &'static Codec,
    },
    DictContract {
        key: &'static Codec,
        value: &'static Codec,
    },
    KeyValue {
        key: &'static Codec,
        value: &'static Codec,
    },
    Seq {
        kind: SeqKind,
        elem: &'static Codec,
    },
    UnityObject(&'static str),
    Any,
    JToken,
    Unsupported(&'static str),
}

impl Codec {
    /// Short name for reports.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Bool => "bool",
            Self::Int { .. } => "int",
            Self::F32 => "float",
            Self::F64 => "double",
            Self::Decimal => "decimal",
            Self::Char => "char",
            Self::Str => "string",
            Self::Guid => "Guid",
            Self::TimeSpan => "TimeSpan",
            Self::DateTime => "DateTime",
            Self::DateTimeOffset => "DateTimeOffset",
            Self::Version => "Version",
            Self::Bytes => "byte[]",
            Self::Enum(_) => "enum",
            Self::Nullable(_) => "Nullable",
            Self::Object(_) => "object",
            Self::Blueprint(_) => "BlueprintConverter",
            Self::BpRef(_) => "BlueprintRefConverter",
            Self::BlueprintGuid => "BlueprintGuidConverter",
            Self::EntityRef(_) => "EntityRefConverter",
            Self::Vector2 => "Vector2Converter",
            Self::Vector3 => "Vector3Converter",
            Self::Quaternion => "QuaternionConverter",
            Self::BoneTransform => "BoneTransformDataConverter",
            Self::Bounds => "BoundsConverter",
            Self::CountableFlag => "CountableFlagConverter",
            Self::Sprite => "SpriteConverter",
            Self::StringEntry => "EntryConverter",
            Self::Dict { .. } => "DictionaryConverter",
            Self::DictContract { .. } => "dictionary contract",
            Self::KeyValue { .. } => "KeyValuePair",
            Self::Seq { .. } => "array",
            Self::UnityObject(_) => "UnityEngine.Object",
            Self::Any => "System.Object",
            Self::JToken => "JToken",
            Self::Unsupported(_) => "unsupported",
        }
    }
}

/// Type flags (see `tools/wr-codegen/src/save.rs`).
pub mod type_flags {
    pub const ABSTRACT: u32 = 1;
    pub const STRUCT: u32 = 2;
    pub const INTERFACE: u32 = 4;
    pub const UNITY: u32 = 8;
    pub const NOT_REFERENCE: u32 = 16;
    /// Constructed with parameters: the object's `$id` is registered only after all its
    /// properties have been read.
    pub const CREATOR_PARAMS: u32 = 32;
    pub const NO_CREATOR: u32 = 64;
    pub const ENTITY: u32 = 128;
    pub const FACT: u32 = 256;
    pub const PART: u32 = 512;
    pub const COMPONENT: u32 = 1024;
    pub const SEALED: u32 = 2048;
    pub const ARRAY_CONTRACT: u32 = 4096;
    pub const DICT_CONTRACT: u32 = 8192;
}

/// Member flags.
pub mod member_flags {
    pub const PROPERTY: u16 = 1;
    pub const READABLE: u16 = 2;
    /// Newtonsoft sets the member on load; otherwise the JSON value is skipped (unless it
    /// populates an existing object or collection).
    pub const WRITABLE: u16 = 4;
    pub const DEFAULT_IGNORE: u16 = 8;
    pub const NULL_IGNORE: u16 = 16;
    pub const REPLACE: u16 = 32;
    pub const STATIC: u16 = 64;
    pub const UNMODELLED_SETTINGS: u16 = 128;
    /// The property feeds a constructor parameter, so it is read without a setter.
    pub const CTOR_PARAM: u16 = 256;
}

#[derive(Debug)]
pub struct Member {
    pub json: &'static str,
    /// C# member name.
    pub name: &'static str,
    pub codec: Codec,
    pub flags: u16,
    pub declared_in: TypeIdx,
}

#[derive(Debug)]
pub struct TypeDef {
    /// CLR name without assembly qualifications.
    pub clr: &'static str,
    pub assembly: &'static str,
    pub flags: u32,
    pub base: TypeIdx,
    /// Bit `i` set: the type (or a base) has a body for [`HOOKS`]`[i]`.
    pub hooks: u32,
    pub ifaces: &'static [TypeIdx],
    pub members: &'static [Member],
}

impl TypeDef {
    #[must_use]
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    /// Member by JSON name: exact first, then case-insensitive
    /// (`JsonPropertyCollection.GetClosestMatchProperty`).
    #[must_use]
    pub fn member(&self, json: &str) -> Option<usize> {
        self.members
            .iter()
            .position(|m| m.json == json)
            .or_else(|| self.members.iter().position(|m| m.json.eq_ignore_ascii_case(json)))
    }

    /// Short name (last segment, generic arguments dropped).
    #[must_use]
    pub fn short(&self) -> &'static str {
        let base = self.clr.split('[').next().unwrap_or(self.clr);
        base.rsplit(['.', '+']).next().unwrap_or(base)
    }

    /// Hook names.
    #[must_use]
    pub fn hook_names(&self) -> Vec<&'static str> {
        HOOKS
            .iter()
            .enumerate()
            .filter(|(i, _)| self.hooks & (1 << i) != 0)
            .map(|(_, h)| *h)
            .collect()
    }
}

#[derive(Debug)]
pub struct EnumDef {
    pub name: &'static str,
    /// CLR name (`+` between nested types), as a `$type` generic argument names it.
    pub clr: &'static str,
    pub assembly: &'static str,
    pub bits: u8,
    pub signed: bool,
    pub flags: bool,
    pub members: &'static [(&'static str, i64)],
}

impl EnumDef {
    /// Wraps a value to the underlying type (`Enum.ToObject`).
    #[must_use]
    pub fn wrap(&self, v: i64) -> i64 {
        match (self.bits, self.signed) {
            (8, true) => i64::from(v as i8),
            (8, false) => i64::from(v as u8),
            (16, true) => i64::from(v as i16),
            (16, false) => i64::from(v as u16),
            (32, true) => i64::from(v as i32),
            (32, false) => i64::from(v as u32),
            _ => v,
        }
    }

    /// `Enum.Parse(type, s)`: a member name, a comma-separated list of names, or a
    /// number in range of the underlying type. Case-sensitive.
    #[must_use]
    pub fn parse(&self, s: &str) -> Option<i64> {
        let t = s.trim();
        if t.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+') {
            let v: i128 = t.parse().ok()?;
            let (lo, hi) = match (self.bits, self.signed) {
                (8, true) => (i128::from(i8::MIN), i128::from(i8::MAX)),
                (8, false) => (0, i128::from(u8::MAX)),
                (16, true) => (i128::from(i16::MIN), i128::from(i16::MAX)),
                (16, false) => (0, i128::from(u16::MAX)),
                (32, true) => (i128::from(i32::MIN), i128::from(i32::MAX)),
                (32, false) => (0, i128::from(u32::MAX)),
                (64, false) => (0, i128::from(u64::MAX)),
                _ => (i128::from(i64::MIN), i128::from(i64::MAX)),
            };
            return (lo..=hi).contains(&v).then_some(v as i64);
        }
        let mut v = 0i64;
        for part in t.split(',') {
            let p = part.trim();
            v |= self.members.iter().find(|(n, _)| *n == p)?.1;
        }
        Some(v)
    }

    /// The member whose name is exactly `s`.
    #[must_use]
    pub fn member_index(&self, s: &str) -> Option<usize> {
        self.members.iter().position(|(n, _)| *n == s)
    }
}

#[must_use]
pub fn ty(i: TypeIdx) -> &'static TypeDef {
    &TYPES[i as usize]
}

#[must_use]
pub fn types() -> &'static [TypeDef] {
    &TYPES
}

/// The assembly declaring a type or enum with this CLR name, if the schema has it.
#[must_use]
pub fn assembly_of(key: &str) -> Option<&'static str> {
    by_name(key)
        .map(|t| ty(t).assembly)
        .or_else(|| ENUMS.iter().find(|e| e.clr == key).map(|e| e.assembly))
}

/// Number of enums in the schema.
#[must_use]
pub fn enum_count() -> usize {
    ENUMS.len()
}

#[must_use]
pub fn enum_def(i: u32) -> &'static EnumDef {
    &ENUMS[i as usize]
}

/// The type a `$type` key (CLR name without assemblies) names.
#[must_use]
pub fn by_name(key: &str) -> Option<TypeIdx> {
    BY_NAME.binary_search_by(|(n, _)| (*n).cmp(key)).ok().map(|i| BY_NAME[i].1)
}

/// `declared.IsAssignableFrom(t)`.
#[must_use]
pub fn is_assignable(t: TypeIdx, declared: TypeIdx) -> bool {
    let mut cur = t;
    for _ in 0..64 {
        if cur == declared {
            return true;
        }
        if cur == NO_TYPE {
            return false;
        }
        let d = ty(cur);
        if d.ifaces.contains(&declared) {
            return true;
        }
        cur = d.base;
    }
    false
}

/// The schema's fingerprint, recorded in our save format.
#[must_use]
pub fn fingerprint() -> u64 {
    FINGERPRINT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_sorted_and_roots_present() {
        assert!(BY_NAME.windows(2).all(|w| w[0].0 < w[1].0));
        for r in [
            "Kingmaker.Player",
            "Kingmaker.EntitySystem.SceneEntitiesState",
            "Kingmaker.EntitySystem.AreaPersistentState",
            "Kingmaker.EntitySystem.Persistence.SaveInfo",
        ] {
            assert!(by_name(r).is_some(), "{r}");
        }
        let unit = by_name("Kingmaker.EntitySystem.Entities.UnitEntityData").unwrap();
        let base = by_name("Kingmaker.EntitySystem.EntityDataBase").unwrap();
        assert!(is_assignable(unit, base));
        assert!(!is_assignable(base, unit));
        assert!(ty(unit).has(type_flags::ENTITY));
    }

    #[test]
    fn enum_parse() {
        let e = EnumDef {
            name: "E",
            clr: "E",
            assembly: "A",
            bits: 8,
            signed: false,
            flags: true,
            members: &[("A", 1), ("B", 2)],
        };
        assert_eq!(e.parse("A, B"), Some(3));
        assert_eq!(e.parse("255"), Some(255));
        assert_eq!(e.parse("256"), None);
        assert_eq!(e.parse("a"), None);
        assert_eq!(e.wrap(257), 1);
    }
}
