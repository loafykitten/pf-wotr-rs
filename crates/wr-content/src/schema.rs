//! The shape of the generated schema tables (`generated/schema.rs`) and lookups over them.
//!
//! Each [`TypeDef`] is one closed C# type reachable from a `[TypeId]` type, with its
//! serialized fields flattened base-first (the order `ReflectionBasedSerializer` writes
//! them). Flags say which fields the JSON carries and which the runtime pack carries.

use crate::generated::schema::{ENUMS, TYPE_IDS, TYPES};
use crate::guid::Guid;

/// Index into [`TYPES`].
pub type TypeIdx = u32;
pub const NO_TYPE: TypeIdx = u32::MAX;

/// Unity value types with their own converter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnityKind {
    Vector2,
    Vector3,
    Vector4,
    Vector2Int,
    Vector3Int,
    Quaternion,
    Color,
    Color32,
    Rect,
    Bounds,
    AnimationCurve,
    Gradient,
}

/// How a field is encoded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Codec {
    Bool,
    Int {
        bits: u8,
        signed: bool,
    },
    F32,
    F64,
    Char,
    Str,
    Decimal,
    /// Index into [`ENUMS`].
    Enum(u32),
    /// A `[Serializable]` class or struct, or a polymorphic `[TypeId]` type.
    Object(TypeIdx),
    /// `BlueprintReference<T>`: `target` is `T`, `class` the reference class.
    BpRef {
        target: TypeIdx,
        class: TypeIdx,
    },
    /// `WeakResourceLink<T>` subclass (the link class).
    WeakLink(TypeIdx),
    SharedString,
    /// Unity asset reference, by C# type name.
    Asset(&'static str),
    Unity(UnityKind),
    Overrides,
    /// `PrototypeLinkConverter`: the game reads null whatever the JSON holds.
    PrototypeLink,
    BlueprintGuid,
    Seq {
        list: bool,
        elem: &'static Codec,
    },
    Unsupported(&'static str),
}

/// A field's C# initializer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Def {
    Zero,
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    EmptyStr,
    /// A non-empty string literal (not copied into the generated code).
    NonEmptyStr,
    NewObject,
    EmptySeq,
    /// `new Vector2/3/4(…)`, `new Color(…)`: the components.
    Floats(&'static [f64]),
    /// Not a literal (a call, a constructor with arguments…).
    Unknown,
}

/// Field flags.
pub mod field_flags {
    /// `[JsonIgnore]`: never in the JSON, never in the pack.
    pub const JSON_IGNORE: u16 = 1;
    /// `[ExcludeFieldFromBuild]`: in the JSON (authoring), not in the runtime pack.
    pub const EXCLUDE_FROM_BUILD: u16 = 2;
    /// A derived field with the same JSON name hides this one in the JSON.
    pub const HIDDEN: u16 = 8;
    pub const SERIALIZE_REFERENCE: u16 = 16;
    /// Unity's `name` property, added by `FieldsContractResolver` (JSON only).
    pub const JSON_ONLY: u16 = 32;
    /// `DefaultValueHandling.Ignore*` on `[JsonProperty]`.
    pub const DEFAULT_IGNORE: u16 = 64;
    /// The C# initializer is a non-empty collection or object initializer: Newtonsoft
    /// adds the JSON's items to it instead of replacing it.
    pub const INIT_POPULATED: u16 = 128;
}

/// Type flags.
pub mod type_flags {
    pub const ABSTRACT: u32 = 1;
    pub const STRUCT: u32 = 2;
    /// `GuidClassBinder.IsIdentifiedType`: blueprint, component or element. The pack
    /// writes a type GUID before each such object.
    pub const IDENTIFIED: u32 = 4;
    pub const BLUEPRINT: u32 = 8;
    pub const COMPONENT: u32 = 16;
    pub const ELEMENT: u32 = 32;
    pub const SERIALIZABLE: u32 = 64;
    pub const UNITY_OBJECT: u32 = 128;
    pub const INTERFACE: u32 = 512;
    /// Implements `IRuntimeEntityFactComponentProvider` (directly or through a base): a fact
    /// creates a runtime component, with per-fact state and a lifecycle, for it.
    pub const RUNTIME_PROVIDER: u32 = 1024;
}

/// Fact-component lifecycle overrides (`EntityFactComponentDelegate` virtuals with a body).
pub mod fact_hooks {
    pub const INITIALIZE: u32 = 1;
    pub const FACT_ATTACHED: u32 = 2;
    pub const ACTIVATE: u32 = 4;
    pub const DEACTIVATE: u32 = 8;
    pub const TURN_ON: u32 = 16;
    pub const TURN_OFF: u32 = 32;
    pub const POST_LOAD: u32 = 64;
    pub const APPLY_POST_LOAD_FIXES: u32 = 128;
    pub const RECALCULATE: u32 = 256;
    pub const DISPOSE: u32 = 512;
    pub const VIEW_DID_ATTACH: u32 = 1024;
    pub const VIEW_WILL_DETACH: u32 = 2048;
    pub const PRE_SAVE: u32 = 4096;
    /// A runtime provider with its own runtime class: its hooks live there, unlisted.
    pub const CUSTOM_RUNTIME: u32 = 8192;

    pub const NAMES: [(u32, &str); 14] = [
        (INITIALIZE, "OnInitialize"),
        (FACT_ATTACHED, "OnFactAttached"),
        (ACTIVATE, "OnActivate"),
        (DEACTIVATE, "OnDeactivate"),
        (TURN_ON, "OnTurnOn"),
        (TURN_OFF, "OnTurnOff"),
        (POST_LOAD, "OnPostLoad"),
        (APPLY_POST_LOAD_FIXES, "OnApplyPostLoadFixes"),
        (RECALCULATE, "OnRecalculate"),
        (DISPOSE, "OnDispose"),
        (VIEW_DID_ATTACH, "OnViewDidAttach"),
        (VIEW_WILL_DETACH, "OnViewWillDetach"),
        (PRE_SAVE, "OnPreSave"),
        (CUSTOM_RUNTIME, "custom runtime class"),
    ];
}

/// Lifecycle hooks (with a body) on a type or its bases.
pub mod hooks {
    /// Called by `BlueprintsCache.Load` on every blueprint after reading it.
    pub const ON_ENABLE: u32 = 1;
    /// Called from `BlueprintScriptableObject.OnEnable`.
    pub const ON_ENABLE_WITH_LIBRARY: u32 = 2;
    /// Newtonsoft callbacks: JSON path only, not the pack.
    pub const ON_DESERIALIZED: u32 = 4;
    pub const ON_DESERIALIZING: u32 = 8;
    /// Unity's `ISerializationCallbackReceiver`: neither path calls it for blueprints.
    pub const AFTER_DESERIALIZE: u32 = 16;
    /// A parameterless constructor with statements (runs before fields are read).
    pub const CTOR_BODY: u32 = 32;
    /// Editor validation.
    pub const ON_VALIDATE: u32 = 64;
    pub const PACKING_CALLBACK: u32 = 128;

    pub const NAMES: [(u32, &str); 8] = [
        (ON_ENABLE, "OnEnable"),
        (ON_ENABLE_WITH_LIBRARY, "OnEnableWithLibrary"),
        (ON_DESERIALIZED, "[OnDeserialized] (JSON only)"),
        (ON_DESERIALIZING, "[OnDeserializing] (JSON only)"),
        (AFTER_DESERIALIZE, "OnAfterDeserialize (not called)"),
        (CTOR_BODY, "constructor body"),
        (ON_VALIDATE, "OnValidate (editor)"),
        (PACKING_CALLBACK, "OnPrePacked/OnPostPacked (build time)"),
    ];
}

#[derive(Debug)]
pub struct Field {
    /// C# name; the pack writes this before each value.
    pub name: &'static str,
    /// JSON property name.
    pub json: &'static str,
    pub codec: Codec,
    pub flags: u16,
    pub default: Def,
    pub declared_in: TypeIdx,
}

impl Field {
    /// Whether the field is read from the JSON (not ignored, not hidden).
    #[must_use]
    pub fn in_json(&self) -> bool {
        self.flags & (field_flags::JSON_IGNORE | field_flags::HIDDEN) == 0
    }

    /// Whether the runtime pack carries the field.
    #[must_use]
    pub fn in_runtime(&self) -> bool {
        self.flags
            & (field_flags::JSON_IGNORE | field_flags::EXCLUDE_FROM_BUILD | field_flags::JSON_ONLY)
            == 0
    }
}

#[derive(Debug)]
pub struct TypeDef {
    /// Full C# name, with generic arguments.
    pub name: &'static str,
    pub short: &'static str,
    /// `[TypeId]`, or 0.
    pub guid: u128,
    pub flags: u32,
    pub base: TypeIdx,
    pub hooks: u32,
    /// Instance fields that aren't serialized.
    pub runtime_fields: u16,
    /// [`fact_hooks`] bits: fact-component lifecycle methods the type (or a base below the
    /// delegate classes) overrides with a body.
    pub fact_hooks: u32,
    /// Event-handler and subscriber interfaces the type or a base lists, with generic
    /// arguments, namespaces stripped (`IInitiatorRulebookHandler<RuleCalculateAC>`).
    pub handlers: &'static [&'static str],
    pub fields: &'static [Field],
}

impl TypeDef {
    #[must_use]
    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

#[derive(Debug)]
pub struct EnumDef {
    pub name: &'static str,
    pub bits: u8,
    pub signed: bool,
    pub flags: bool,
    pub members: &'static [(&'static str, i64)],
}

impl EnumDef {
    /// `Enum.Parse`: a member name, a comma-separated list of names (OR-ed, for any enum),
    /// or a decimal number. Names are case-sensitive, as `Enum.Parse(type, s)` is.
    #[must_use]
    pub fn parse(&self, s: &str) -> Option<i64> {
        let s = s.trim();
        if s.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '+') {
            return s.parse().ok();
        }
        let mut v = 0i64;
        for part in s.split(',') {
            let p = part.trim();
            v |= self.members.iter().find(|(n, _)| *n == p)?.1;
        }
        Some(v)
    }
}

#[must_use]
pub fn ty(i: TypeIdx) -> &'static TypeDef {
    &TYPES[i as usize]
}

#[must_use]
pub fn enum_def(i: u32) -> &'static EnumDef {
    &ENUMS[i as usize]
}

#[must_use]
pub fn types() -> &'static [TypeDef] {
    &TYPES
}

#[must_use]
pub fn enums() -> &'static [EnumDef] {
    &ENUMS
}

/// The type registered for a `$type` GUID (`GuidClassBinder.BindToType`).
#[must_use]
pub fn by_type_id(g: Guid) -> Option<TypeIdx> {
    TYPE_IDS.binary_search_by_key(&g.0, |(k, _)| *k).ok().map(|i| TYPE_IDS[i].1)
}

/// Number of registered `[TypeId]`s.
#[must_use]
pub fn type_id_count() -> usize {
    TYPE_IDS.len()
}

/// `t == of || t.IsSubclassOf(of)`.
#[must_use]
pub fn is_assignable(t: TypeIdx, of: TypeIdx) -> bool {
    let mut cur = t;
    for _ in 0..64 {
        if cur == of {
            return true;
        }
        if cur == NO_TYPE {
            return false;
        }
        cur = ty(cur).base;
    }
    false
}

/// Index of the first type with this full or short name.
#[must_use]
pub fn find(name: &str) -> Option<TypeIdx> {
    TYPES
        .iter()
        .position(|t| t.name == name || t.short == name)
        .and_then(|i| TypeIdx::try_from(i).ok())
}

/// Schema indices of types the serializers treat specially.
#[derive(Debug, Clone, Copy)]
pub struct Special {
    /// `BlueprintReferenceBase` itself (not a subclass): the JSON converter skips it, the
    /// pack writes a bare GUID.
    pub bp_ref_base: TypeIdx,
    /// `UnityEngine.UI.ColorBlock`: its own pack case.
    pub color_block: TypeIdx,
    /// `LocalizedString`: rewritten at pack time (see `canon`).
    pub localized_string: TypeIdx,
}

/// The [`Special`] indices (computed once).
#[must_use]
pub fn special() -> &'static Special {
    static S: std::sync::OnceLock<Special> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        let f = |n: &str| find(n).unwrap_or(NO_TYPE);
        Special {
            bp_ref_base: f("Kingmaker.Blueprints.BlueprintReferenceBase"),
            color_block: f("UnityEngine.UI.ColorBlock"),
            localized_string: f("Kingmaker.Localization.LocalizedString"),
        }
    })
}

/// A stable hash of the schema (computed by the generator), stored in packs so a stale
/// pack is rejected.
#[must_use]
pub fn fingerprint() -> u64 {
    crate::generated::schema::FINGERPRINT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_sorted_and_resolves() {
        assert!(TYPE_IDS.windows(2).all(|w| w[0].0 < w[1].0));
        let g = Guid::parse("d79ad4ed064aa4f43ace0e9c238fb9b9").unwrap();
        let t = by_type_id(g).expect("AddFacts is registered");
        assert_eq!(ty(t).short, "AddFacts");
        assert!(ty(t).has(type_flags::COMPONENT));
        let comp = find("Kingmaker.Blueprints.BlueprintComponent").unwrap();
        assert!(is_assignable(t, comp));
        assert!(!is_assignable(comp, t));
    }

    #[test]
    fn enum_parse_follows_enum_parse() {
        let e = EnumDef {
            name: "E",
            bits: 32,
            signed: true,
            flags: true,
            members: &[("A", 1), ("B", 2), ("C", 4)],
        };
        assert_eq!(e.parse("B"), Some(2));
        assert_eq!(e.parse("A, C"), Some(5));
        assert_eq!(e.parse("7"), Some(7));
        assert_eq!(e.parse("a"), None);
        assert_eq!(e.parse("D"), None);
    }
}
