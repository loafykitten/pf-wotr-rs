//! The game's serialization rules, applied to resolved declarations:
//!
//! - `FieldsContractResolver.GetUnitySerializedFields`: instance fields, base classes first,
//!   that are public or `[SerializeField]`/`[SerializeReference]`, not `[NonSerialized]`,
//!   and whose type passes `IsSerializableType`.
//! - Newtonsoft on top: `[JsonProperty]` names, `[JsonIgnore]`, derived fields hiding base
//!   fields of the same JSON name, read-only fields not populated, the `name` property
//!   added for `UnityEngine.Object` subclasses, and the converters registered in `Json`.
//! - `ReflectionBasedSerializer` (the `.bbp` pack): the same fields minus `[JsonIgnore]` and
//!   `[ExcludeFieldFromBuild]`, under their C# names.
//!
//! The output is a closed set of schema entries (one per closed type) reachable from every
//! `[TypeId]` type in Assembly-CSharp, which is what `StartGameLoader.PrepareTypeCache`
//! registers with `GuidClassBinder`.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::model::{Prim, Ty, Universe};
use crate::parse::{Expr, Kind, RawField};

pub const UNITY_OBJECT: &str = "UnityEngine.Object";
const BP_REF_BASE: &str = "Kingmaker.Blueprints.BlueprintReferenceBase";
const BP_REF_GENERIC: &str = "Kingmaker.Blueprints.BlueprintReference";
const SIMPLE_BLUEPRINT: &str = "Kingmaker.Blueprints.SimpleBlueprint";
const BP_SCRIPTABLE: &str = "Kingmaker.Blueprints.BlueprintScriptableObject";
const BP_COMPONENT: &str = "Kingmaker.Blueprints.BlueprintComponent";
const ELEMENT: &str = "Kingmaker.ElementsSystem.Element";
const RUNTIME_PROVIDER: &str = "Kingmaker.EntitySystem.IRuntimeEntityFactComponentProvider";
const WEAK_LINK: &str = "Kingmaker.ResourceLinks.WeakResourceLink";
const SHARED_STRING: &str = "Kingmaker.Localization.SharedStringAsset";
const PROTOTYPEABLE: &str = "Kingmaker.Blueprints.PrototypeableObjectBase";
const ELEMENTS_SO: &str = "Kingmaker.ElementsSystem.ElementsScriptableObject";
const OVERRIDES: &str = "Kingmaker.Blueprints.OverridesManager";
const BP_GUID: &str = "Kingmaker.Blueprints.BlueprintGuid";
/// mscorlib enums that show up as field types, with their members in value order.
pub(crate) const SYSTEM_ENUMS: &[(&str, &[&str])] = &[(
    "System.DayOfWeek",
    &["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"],
)];
pub const WRAPPER: &str = "Kingmaker.Blueprints.JsonSystem.BlueprintJsonWrapper";

/// Unity value types with their own converter or binary case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
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

impl UnityKind {
    pub(crate) fn from_full(n: &str) -> Option<Self> {
        Some(match n {
            "UnityEngine.Vector2" => Self::Vector2,
            "UnityEngine.Vector3" => Self::Vector3,
            "UnityEngine.Vector4" => Self::Vector4,
            "UnityEngine.Vector2Int" => Self::Vector2Int,
            "UnityEngine.Vector3Int" => Self::Vector3Int,
            "UnityEngine.Quaternion" => Self::Quaternion,
            "UnityEngine.Color" => Self::Color,
            "UnityEngine.Color32" => Self::Color32,
            "UnityEngine.Rect" => Self::Rect,
            "UnityEngine.Bounds" => Self::Bounds,
            "UnityEngine.AnimationCurve" => Self::AnimationCurve,
            "UnityEngine.Gradient" => Self::Gradient,
            _ => return None,
        })
    }

    /// Accepted by `IsSerializableType` (Quaternion is not on its list).
    fn serializable(self) -> bool {
        self != Self::Quaternion
    }
}

/// How a field's value is encoded in the JSON (and, mostly, the pack).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
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
    /// `System.Decimal` is not primitive and lives in mscorlib, so it never passes
    /// `IsSerializableType`; kept for completeness.
    Decimal,
    Enum(usize),
    /// `[Serializable]` class or struct, or a polymorphic `[TypeId]` type. Index into
    /// the schema's types.
    Object(usize),
    /// `BlueprintReference<T>` subclass: `"!bp_<guid>"` or null. `target` is `T`'s
    /// schema entry, `class` the reference class's.
    BpRef {
        target: usize,
        class: usize,
    },
    /// `WeakResourceLink<T>` subclass: `{"AssetId": …}` or null.
    WeakLink(usize),
    /// `SharedStringAsset`: `{"stringkey": …}` or null.
    SharedString,
    /// A Unity asset (`UnityEngine.Object` subclass) as `{"guid", "fileid"}`.
    Asset(String),
    Unity(UnityKind),
    /// `OverridesManager`: written as a path list, read back as null.
    Overrides,
    /// `PrototypeableObjectBase.PrototypeLink` (`PrototypeLinkConverter`): read as null.
    PrototypeLink,
    BlueprintGuid,
    Seq {
        list: bool,
        elem: Box<Codec>,
    },
    /// The type can't be decoded through a schema (reason).
    Unsupported(String),
}

/// A field default from its C# initializer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Def {
    /// No initializer: `default(T)` (zero, false, null, or a zeroed struct).
    Zero,
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Only the empty string is carried; other string literals are code text we don't
    /// copy into generated files.
    EmptyStr,
    NonEmptyStr,
    /// `new T()` with no arguments.
    NewObject,
    /// `new Vector2/3/4(…)` or `new Color(…)` with constant arguments, completed to the
    /// type's arity (missing vector components 0, missing alpha 1).
    Floats(Vec<f64>),
    /// `new T[0]`, `Array.Empty<T>()`, `new List<T>()`.
    EmptySeq,
    /// Anything else, with the reason.
    Unknown(String),
}

pub const F_JSON_IGNORE: u16 = 1;
pub const F_EXCLUDE_FROM_BUILD: u16 = 2;
pub const F_HIDDEN: u16 = 8;
pub const F_SERIALIZE_REFERENCE: u16 = 16;
pub const F_JSON_ONLY: u16 = 32;
pub const F_DEFAULT_IGNORE: u16 = 64;
/// The initializer is a non-empty collection or object initializer.
pub const F_INIT_POPULATED: u16 = 128;

#[derive(Debug, Clone, Serialize)]
pub struct SField {
    /// C# field name (the `.bbp` writes this).
    pub name: String,
    /// JSON property name.
    pub json: String,
    pub codec: Codec,
    pub flags: u16,
    pub default: Def,
    /// Schema entry of the declaring type.
    pub declared_in: usize,
}

pub const T_ABSTRACT: u32 = 1;
pub const T_STRUCT: u32 = 2;
/// `GuidClassBinder.IsIdentifiedType`: the pack writes a type GUID before the object.
pub const T_IDENTIFIED: u32 = 4;
pub const T_BLUEPRINT: u32 = 8;
pub const T_COMPONENT: u32 = 16;
pub const T_ELEMENT: u32 = 32;
pub const T_SERIALIZABLE: u32 = 64;
pub const T_UNITY_OBJECT: u32 = 128;
/// Only here as a base class or reference target; never decoded directly.
pub const T_NOT_DECODABLE: u32 = 256;
pub const T_INTERFACE: u32 = 512;
/// Implements `IRuntimeEntityFactComponentProvider`: a fact creates a runtime component
/// (with per-fact state and a lifecycle) for this blueprint component.
pub const T_RUNTIME_PROVIDER: u32 = 1024;

/// Fact-component lifecycle overrides (`EntityFactComponentDelegate` virtuals with a body,
/// declared on the type or a base below the delegate classes).
pub const FH_INITIALIZE: u32 = 1;
pub const FH_FACT_ATTACHED: u32 = 2;
pub const FH_ACTIVATE: u32 = 4;
pub const FH_DEACTIVATE: u32 = 8;
pub const FH_TURN_ON: u32 = 16;
pub const FH_TURN_OFF: u32 = 32;
pub const FH_POST_LOAD: u32 = 64;
pub const FH_APPLY_POST_LOAD_FIXES: u32 = 128;
pub const FH_RECALCULATE: u32 = 256;
pub const FH_DISPOSE: u32 = 512;
pub const FH_VIEW_DID_ATTACH: u32 = 1024;
pub const FH_VIEW_WILL_DETACH: u32 = 2048;
pub const FH_PRE_SAVE: u32 = 4096;
/// Has its own runtime class (an `IRuntimeEntityFactComponentProvider` that isn't a
/// delegate): hooks live in that class and aren't listed.
pub const FH_CUSTOM_RUNTIME: u32 = 8192;

const FACT_HOOKS: &[(&str, u32)] = &[
    ("OnInitialize", FH_INITIALIZE),
    ("OnFactAttached", FH_FACT_ATTACHED),
    ("OnActivate", FH_ACTIVATE),
    ("OnDeactivate", FH_DEACTIVATE),
    ("OnTurnOn", FH_TURN_ON),
    ("OnTurnOff", FH_TURN_OFF),
    ("OnPostLoad", FH_POST_LOAD),
    ("OnApplyPostLoadFixes", FH_APPLY_POST_LOAD_FIXES),
    ("OnRecalculate", FH_RECALCULATE),
    ("OnDispose", FH_DISPOSE),
    ("OnViewDidAttach", FH_VIEW_DID_ATTACH),
    ("OnViewWillDetach", FH_VIEW_WILL_DETACH),
    ("OnPreSave", FH_PRE_SAVE),
];

const DELEGATE_BASE: &str = "Kingmaker.EntitySystem.EntityFactComponentDelegate";
const SUBSCRIBER: &str = "Kingmaker.PubSubSystem.ISubscriber";

/// `A.B.IFoo<C.D.Bar, E.Baz>` → `IFoo<Bar, Baz>`.
fn short_generic(full: &str) -> String {
    let mut out = String::new();
    let mut seg = String::new();
    for ch in full.chars() {
        match ch {
            '<' | '>' | ',' | ' ' => {
                out.push_str(seg.rsplit(['.', '+']).next().unwrap_or(&seg));
                seg.clear();
                out.push(ch);
            }
            _ => seg.push(ch),
        }
    }
    out.push_str(seg.rsplit(['.', '+']).next().unwrap_or(&seg));
    out
}

pub const H_ON_ENABLE: u32 = 1;
pub const H_ON_ENABLE_WITH_LIBRARY: u32 = 2;
pub const H_ON_DESERIALIZED: u32 = 4;
pub const H_ON_DESERIALIZING: u32 = 8;
pub const H_AFTER_DESERIALIZE: u32 = 16;
pub const H_CTOR_BODY: u32 = 32;
pub const H_ON_VALIDATE: u32 = 64;
pub const H_PACKING_CALLBACK: u32 = 128;

#[derive(Debug, Clone, Serialize)]
pub struct SType {
    pub name: String,
    pub short: String,
    pub guid: Option<String>,
    pub flags: u32,
    pub base: Option<usize>,
    pub fields: Vec<SField>,
    /// Lifecycle hooks with a body, declared on this type or inherited.
    pub hooks: u32,
    /// Where each hook comes from: hook name → declaring type.
    pub hook_sources: BTreeMap<String, String>,
    /// Instance fields that aren't serialized (runtime state a loader would have to compute).
    pub runtime_fields: Vec<String>,
    /// `FH_*` bits: fact-component lifecycle overrides.
    pub fact_hooks: u32,
    /// Event-handler and subscriber interfaces the type (or a base) lists, as written with
    /// generic arguments (e.g. `IInitiatorRulebookHandler<RuleCalculateAC>`).
    pub handlers: Vec<String>,
    pub file: String,
    #[serde(skip)]
    pub ty: Ty,
}

#[derive(Debug, Clone, Serialize)]
pub struct SEnum {
    pub name: String,
    pub bits: u8,
    pub signed: bool,
    pub flags: bool,
    pub members: Vec<(String, i64)>,
    pub unresolved: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Problems {
    /// Field types that didn't resolve (field → type text).
    pub unresolved_field_types: BTreeMap<String, Vec<String>>,
    /// Field types inside the closure that pass `IsSerializableType` but have no codec.
    pub unsupported: BTreeMap<String, Vec<String>>,
    /// Duplicate JSON property names Newtonsoft would reject.
    pub duplicate_json_names: Vec<String>,
    pub type_id_collisions: Vec<String>,
    pub non_literal_defaults: usize,
    /// Types whose parameterless constructor does more than assign fields (defaults of
    /// fields it doesn't visibly assign may be wrong).
    pub ctor_unmodelled: std::collections::BTreeSet<String>,
}

#[derive(Debug)]
pub struct Builder<'u> {
    pub u: &'u Universe,
    pub types: Vec<SType>,
    pub enums: Vec<SEnum>,
    index: HashMap<Ty, usize>,
    enum_index: HashMap<usize, usize>,
    pending: Vec<usize>,
    pub problems: Problems,
}

impl<'u> Builder<'u> {
    #[must_use]
    pub fn new(u: &'u Universe) -> Self {
        Self {
            u,
            types: Vec::new(),
            enums: Vec::new(),
            index: HashMap::new(),
            enum_index: HashMap::new(),
            pending: Vec::new(),
            problems: Problems::default(),
        }
    }

    /// Builds the closure from every `[TypeId]` type in Assembly-CSharp plus the wrapper.
    pub fn build_all(&mut self) {
        let mut roots = Vec::new();
        for (i, t) in self.u.types.iter().enumerate() {
            if &*t.assembly == "Assembly-CSharp" && t.has_attr("TypeId") && t.generics.is_empty() {
                roots.push(i);
            }
        }
        if let Some(w) = self.u.find(WRAPPER) {
            roots.push(w);
        }
        for r in roots {
            self.intern(&Ty::Decl(r, Vec::new()));
        }
        while let Some(i) = self.pending.pop() {
            self.fill(i);
        }
        // TypeId collisions.
        let mut seen: HashMap<String, String> = HashMap::new();
        for t in &self.types {
            if let Some(g) = &t.guid
                && let Some(prev) = seen.insert(g.clone(), t.name.clone())
            {
                self.problems.type_id_collisions.push(format!("{g}: {prev} / {}", t.name));
            }
        }
    }

    fn intern(&mut self, ty: &Ty) -> usize {
        if let Some(&i) = self.index.get(ty) {
            return i;
        }
        let Ty::Decl(d, _) = ty else { unreachable!("intern called with {ty:?}") };
        let raw = &self.u.types[*d];
        let mut flags = 0;
        if raw.has_mod("abstract") {
            flags |= T_ABSTRACT;
        }
        if raw.kind == Kind::Struct {
            flags |= T_STRUCT;
        }
        if raw.kind == Kind::Interface {
            flags |= T_INTERFACE;
        }
        if raw.has_attr("Serializable") {
            flags |= T_SERIALIZABLE;
        }
        let is = |n: &str| self.u.is_or_subclass_of(ty, n);
        if is(SIMPLE_BLUEPRINT) {
            flags |= T_BLUEPRINT | T_IDENTIFIED;
        }
        if is(BP_COMPONENT) {
            flags |= T_COMPONENT | T_IDENTIFIED;
        }
        if is(ELEMENT) {
            flags |= T_ELEMENT | T_IDENTIFIED;
        }
        if is(UNITY_OBJECT) {
            flags |= T_UNITY_OBJECT;
        }
        if flags & T_COMPONENT != 0 && self.u.implements(ty, RUNTIME_PROVIDER) {
            flags |= T_RUNTIME_PROVIDER;
        }
        let guid = raw
            .attrs
            .iter()
            .find(|a| a.name == "TypeId")
            .and_then(|a| a.first_positional_string())
            .map(|s| s.to_ascii_lowercase().replace('-', ""))
            // `PrepareTypeCache` registers Assembly-CSharp's types only. It registers open
            // generic definitions too, but those can't be instantiated, so a `$type` naming
            // one fails in the game as it does here (unknown type).
            .filter(|_| raw.generics.is_empty() && &*raw.assembly == "Assembly-CSharp");
        let idx = self.types.len();
        self.types.push(SType {
            name: self.u.display(ty),
            short: raw.name.clone(),
            guid,
            flags,
            base: None,
            fields: Vec::new(),
            hooks: 0,
            hook_sources: BTreeMap::new(),
            runtime_fields: Vec::new(),
            fact_hooks: 0,
            handlers: Vec::new(),
            file: format!("{}:{}", raw.file, raw.line),
            ty: ty.clone(),
        });
        self.index.insert(ty.clone(), idx);
        self.pending.push(idx);
        idx
    }

    fn intern_enum(&mut self, d: usize) -> usize {
        if let Some(&i) = self.enum_index.get(&d) {
            return i;
        }
        let i = self.enums.len();
        self.enums.push(enum_def(self.u, d));
        self.enum_index.insert(d, i);
        i
    }

    fn intern_system_enum(&mut self, name: &str) -> usize {
        if let Some(i) = self.enums.iter().position(|e| e.name == name) {
            return i;
        }
        let members = SYSTEM_ENUMS
            .iter()
            .find(|(e, _)| *e == name)
            .map(|(_, m)| m.iter().zip(0..).map(|(n, v)| ((*n).to_owned(), v)).collect())
            .unwrap_or_default();
        self.enums.push(SEnum {
            name: name.to_owned(),
            bits: 32,
            signed: true,
            flags: false,
            members,
            unresolved: Vec::new(),
        });
        self.enums.len() - 1
    }

    /// `IsSerializableType`.
    fn is_serializable(&self, t: &Ty, arrays_allowed: bool) -> bool {
        match t {
            Ty::Prim(p) => p.is_clr_primitive() || *p == Prim::String,
            Ty::Decl(d, _) => {
                let raw = &self.u.types[*d];
                if raw.kind == Kind::Enum {
                    return true;
                }
                if self.u.is_or_subclass_of(t, UNITY_OBJECT) {
                    return true;
                }
                if let Some(k) = UnityKind::from_full(self.u.full_name(*d)) {
                    return k.serializable();
                }
                // `[Serializable]` and not from mscorlib (no decompiled decl is).
                raw.has_attr("Serializable") && raw.kind != Kind::Interface
            }
            Ty::Array(e, _) | Ty::List(e) => arrays_allowed && self.is_serializable(e, false),
            // mscorlib enums pass `IsEnum`.
            Ty::External(n, _) => SYSTEM_ENUMS.iter().any(|(e, _)| e == n),
            // Nullable<T>, Guid, DateTime…: mscorlib.
            _ => false,
        }
    }

    /// Converter / binary case for a serializable field type.
    fn codec(&mut self, t: &Ty, ctx: &str) -> Codec {
        match t {
            Ty::Prim(p) => match p {
                Prim::Bool => Codec::Bool,
                Prim::F32 => Codec::F32,
                Prim::F64 => Codec::F64,
                Prim::Char => Codec::Char,
                Prim::String => Codec::Str,
                Prim::Decimal => Codec::Decimal,
                p => match prim_int(*p) {
                    Some((bits, signed)) => Codec::Int { bits, signed },
                    None => Codec::Unsupported(format!("{p:?}")),
                },
            },
            Ty::Array(e, rank) => {
                if *rank > 1 {
                    return Codec::Unsupported(format!("rank-{rank} array"));
                }
                Codec::Seq { list: false, elem: Box::new(self.codec(e, ctx)) }
            }
            Ty::List(e) => Codec::Seq { list: true, elem: Box::new(self.codec(e, ctx)) },
            Ty::External(n, _) if SYSTEM_ENUMS.iter().any(|(e, _)| e == n) => {
                Codec::Enum(self.intern_system_enum(n))
            }
            Ty::Nullable(_) | Ty::External(..) | Ty::GenericParam(_) | Ty::Unresolved(_) => {
                let why = self.u.display(t);
                self.problems.unsupported.entry(why.clone()).or_default().push(ctx.to_owned());
                Codec::Unsupported(why)
            }
            Ty::Decl(d, _) => {
                let d = *d;
                let full = self.u.full_name(d).to_owned();
                if self.u.types[d].kind == Kind::Enum {
                    return Codec::Enum(self.intern_enum(d));
                }
                if let Some(k) = UnityKind::from_full(&full) {
                    return Codec::Unity(k);
                }
                let strict = |u: &Universe, n: &str| full != n && u.is_or_subclass_of(t, n);
                if strict(self.u, BP_REF_BASE) {
                    let target = self
                        .u
                        .find_base(t, BP_REF_GENERIC)
                        .and_then(|b| match b {
                            Ty::Decl(_, mut a) if a.len() == 1 => Some(a.remove(0)),
                            _ => None,
                        })
                        .filter(|a| matches!(a, Ty::Decl(..)))
                        .unwrap_or_else(|| {
                            Ty::Decl(self.u.find(SIMPLE_BLUEPRINT).unwrap_or(0), Vec::new())
                        });
                    let target = self.intern(&target);
                    let class = self.intern(t);
                    return Codec::BpRef { target, class };
                }
                if strict(self.u, WEAK_LINK) {
                    return Codec::WeakLink(self.intern(t));
                }
                if full == SHARED_STRING {
                    return Codec::SharedString;
                }
                if full == OVERRIDES {
                    return Codec::Overrides;
                }
                if full == BP_GUID {
                    return Codec::BlueprintGuid;
                }
                let unity_obj = (strict(self.u, UNITY_OBJECT)
                    && !strict(self.u, PROTOTYPEABLE)
                    && !strict(self.u, ELEMENTS_SO)
                    && !strict(self.u, ELEMENT))
                    || full == UNITY_OBJECT;
                if unity_obj {
                    return Codec::Asset(self.u.display(t));
                }
                Codec::Object(self.intern(t))
            }
        }
    }

    /// Fills one schema entry: base, fields, hooks.
    fn fill(&mut self, idx: usize) {
        let ty = self.types[idx].ty.clone();
        let Ty::Decl(d, args) = &ty else { return };
        let (d, args) = (*d, args.clone());
        let base = self.u.closed_base(&ty);
        let mut fields: Vec<SField> = Vec::new();
        let mut hooks = 0;
        let mut hook_sources = BTreeMap::new();
        let mut runtime_fields = Vec::new();
        let mut fact_hooks = 0;
        let mut handlers: Vec<String> = Vec::new();
        if let Some(b) = &base {
            let bi = self.intern(b);
            self.types[idx].base = Some(bi);
            // The base may still be pending; fill it first so we can copy its fields.
            if self.pending.contains(&bi) {
                self.pending.retain(|&p| p != bi);
                self.fill(bi);
            }
            fields = self.types[bi].fields.clone();
            hooks = self.types[bi].hooks;
            hook_sources = self.types[bi].hook_sources.clone();
            runtime_fields = self.types[bi].runtime_fields.clone();
            fact_hooks = self.types[bi].fact_hooks;
            handlers = self.types[bi].handlers.clone();
        }
        let raw = &self.u.types[d];
        let own_name = self.types[idx].name.clone();
        let short = raw.name.clone();
        // Hooks declared here.
        for m in &raw.methods {
            let h = match (m.name.as_str(), m.params) {
                ("OnEnable", 0) => H_ON_ENABLE,
                ("OnEnableWithLibrary", 0) => H_ON_ENABLE_WITH_LIBRARY,
                ("OnAfterDeserialize", 0) => H_AFTER_DESERIALIZE,
                ("OnValidate", 0) => H_ON_VALIDATE,
                ("OnPrePacked" | "OnPostPacked", 0) => H_PACKING_CALLBACK,
                _ if m.attrs.iter().any(|a| a == "OnDeserialized") => H_ON_DESERIALIZED,
                _ if m.attrs.iter().any(|a| a == "OnDeserializing") => H_ON_DESERIALIZING,
                _ => 0,
            };
            if h != 0 && m.has_body {
                hooks |= h;
                hook_sources.insert(hook_name(h).to_owned(), short.clone());
            }
        }
        // Fact-component lifecycle overrides, below the delegate base classes (whose
        // virtuals are empty, except `OnRecalculate`'s default of off-then-on).
        if !self.u.display(&Ty::Decl(d, Vec::new())).starts_with(DELEGATE_BASE) {
            for m in &raw.methods {
                if m.params != 0 || !m.has_body {
                    continue;
                }
                if let Some((_, h)) = FACT_HOOKS.iter().find(|(n, _)| *n == m.name) {
                    fact_hooks |= h;
                }
                // A delegate that overrides the runtime factory has its own runtime
                // class, whose hooks aren't the delegate's. Except
                // `UnitBuffComponentDelegate`, whose runtime only adds `OnRemoved`
                // (buff removal) to the delegate's own forwarding.
                if m.name == "CreateRuntimeFactComponent"
                    && !self.u.display(&Ty::Decl(d, Vec::new())).starts_with(
                        "Kingmaker.UnitLogic.Buffs.Components.UnitBuffComponentDelegate",
                    )
                {
                    fact_hooks |= FH_CUSTOM_RUNTIME;
                }
            }
        }
        if self.types[idx].flags & T_RUNTIME_PROVIDER != 0
            && !self.u.is_or_subclass_of(&ty, DELEGATE_BASE)
        {
            fact_hooks |= FH_CUSTOM_RUNTIME;
        }
        // Handler and subscriber interfaces listed on this declaration.
        let skip = usize::from(self.u.base_class(d).is_some());
        for b in raw.bases.iter().skip(skip) {
            let r = self.u.resolve(b, Some(d));
            let r = self.u.subst(&r, d, &args);
            if let Ty::Decl(i, _) = &r
                && self.u.types[*i].kind == Kind::Interface
            {
                // Any interface deriving from `ISubscriber` (what `InterfaceFinder` picks
                // up by assignability), not only those named like handlers.
                let n = &self.u.types[*i].name;
                if n != "ISubscriber" && self.u.implements(&r, SUBSCRIBER) {
                    let disp = self.u.display(&r);
                    let short = short_generic(&disp);
                    if !handlers.contains(&short) {
                        handlers.push(short);
                    }
                }
            }
        }
        if raw.ctor_with_body {
            hooks |= H_CTOR_BODY;
            hook_sources.insert(hook_name(H_CTOR_BODY).to_owned(), short.clone());
        }
        let is_unity_object = self.types[idx].flags & T_UNITY_OBJECT != 0;
        let raw_fields: Vec<RawField> = raw.fields.clone();
        let raw_ctor = raw.ctor_assigns.clone();
        let raw_ctor_other = raw.ctor_other;
        for f in &raw_fields {
            if f.has_mod("static") || f.has_mod("const") {
                continue;
            }
            let has = |n: &str| f.attrs.iter().any(|a| a.name == n);
            let declared = self.u.resolve(&f.ty, Some(d));
            let fty = self.u.subst(&declared, d, &args);
            let ctx = format!("{own_name}.{}", f.name);
            let selected =
                (f.has_mod("public") || has("SerializeField") || has("SerializeReference"))
                    && !has("NonSerialized");
            if !selected {
                runtime_fields.push(format!("{short}.{}", f.name));
                continue;
            }
            if !self.is_serializable(&fty, true) {
                if contains_generic_param(&fty) && selected {
                    self.problems
                        .unresolved_field_types
                        .entry(format!("generic parameter in {}", self.u.display(&fty)))
                        .or_default()
                        .push(ctx.clone());
                }
                if let Ty::Unresolved(s) = &fty {
                    self.problems
                        .unresolved_field_types
                        .entry(s.clone())
                        .or_default()
                        .push(ctx.clone());
                }
                runtime_fields.push(format!("{short}.{}", f.name));
                continue;
            }
            let json_prop = f.attrs.iter().find(|a| a.name == "JsonProperty");
            let json = json_prop
                .and_then(|a| a.string_arg("PropertyName").or_else(|| a.first_positional_string()))
                .unwrap_or(&f.name)
                .to_owned();
            let mut flags = 0;
            if has("JsonIgnore") {
                flags |= F_JSON_IGNORE;
            }
            if has("ExcludeFieldFromBuild") {
                flags |= F_EXCLUDE_FROM_BUILD;
            }
            if has("SerializeReference") {
                flags |= F_SERIALIZE_REFERENCE;
            }
            // `CreateProperties(type, MemberSerialization.Fields)` makes every property
            // writable, readonly fields included.
            if json_prop.is_some_and(|a| {
                a.args.iter().any(|(n, e)| {
                    n.as_deref() == Some("DefaultValueHandling")
                        && matches!(e, Expr::Ident(p) if p.last().is_some_and(|x| x.starts_with("Ignore")))
                })
            }) {
                flags |= F_DEFAULT_IGNORE;
            }
            let codec = if f.attrs.iter().any(|a| {
                a.name == "JsonConverter"
                    && a.args.iter().any(|(_, e)| matches!(e, Expr::Other(s) if s.contains("PrototypeLinkConverter")))
            }) {
                Codec::PrototypeLink
            } else {
                self.codec(&fty, &ctx)
            };
            // A parameterless constructor's assignment runs after the initializer.
            let ctor = raw_ctor.iter().find(|(n, _)| n == &f.name).map(|(_, e)| e);
            let mut default = self.default_of(ctor.or(f.init.as_ref()), &fty);
            if raw_ctor_other && ctor.is_none() {
                // Other constructor statements may set this field too.
                self.problems.ctor_unmodelled.insert(own_name.clone());
            }
            if let (Some(Expr::NewEmpty(t)), Ty::Decl(..)) = (ctor.or(f.init.as_ref()), &fty)
                && self.u.resolve(t, Some(d)) != declared
            {
                default = Def::Unknown("initializer constructs another type".into());
            }
            // Newtonsoft fills an existing list or object instead of replacing it (arrays
            // are always replaced), so JSON items would be added to this initializer's.
            let populates = matches!(fty, Ty::List(_))
                || matches!(&fty, Ty::Decl(d2, _) if self.u.types[*d2].kind == Kind::Class);
            if populates
                && matches!(ctor.or(f.init.as_ref()), Some(Expr::Other(t)) if t.contains('{') && t.trim_start().starts_with("new"))
            {
                flags |= F_INIT_POPULATED;
            }
            if matches!(default, Def::Unknown(_)) {
                self.problems.non_literal_defaults += 1;
            }
            // Newtonsoft's duplicate-name handling: an ignored property never displaces
            // another; a derived declaration hides the base one.
            if flags & F_JSON_IGNORE == 0 {
                for other in &mut fields {
                    if other.json == json && other.flags & (F_JSON_IGNORE | F_HIDDEN) == 0 {
                        // `other` comes from a base class (fields are base-first).
                        if other.declared_in == idx {
                            self.problems.duplicate_json_names.push(format!("{own_name}.{json}"));
                        }
                        other.flags |= F_HIDDEN;
                    }
                }
            }
            fields.push(SField {
                name: f.name.clone(),
                json,
                codec,
                flags,
                default,
                declared_in: idx,
            });
        }
        // `FieldsContractResolver` adds Unity's `name` property for UnityEngine.Object
        // subclasses that aren't blueprints (JSON only).
        if is_unity_object && !self.u.is_or_subclass_of(&ty, BP_SCRIPTABLE) {
            fields.retain(|f| !(f.flags & F_JSON_ONLY != 0 && f.json == "name"));
            fields.push(SField {
                name: "name".into(),
                json: "name".into(),
                codec: Codec::Str,
                flags: F_JSON_ONLY,
                default: Def::Null,
                declared_in: idx,
            });
        }
        let t = &mut self.types[idx];
        t.fields = fields;
        t.hooks = hooks;
        t.hook_sources = hook_sources;
        t.runtime_fields = runtime_fields;
        t.fact_hooks = fact_hooks;
        t.handlers = handlers;
    }

    fn default_of(&mut self, init: Option<&Expr>, ty: &Ty) -> Def {
        let Some(e) = init else { return Def::Zero };
        match e {
            Expr::Null => Def::Null,
            Expr::Bool(b) => Def::Bool(*b),
            Expr::Str(s) if s.is_empty() => Def::EmptyStr,
            Expr::Str(_) => Def::NonEmptyStr,
            Expr::Default => Def::Zero,
            Expr::NewEmpty(_) => match ty {
                Ty::List(_) | Ty::Array(..) => Def::EmptySeq,
                _ => Def::NewObject,
            },
            Expr::Other(s) if is_empty_seq_text(s) => Def::EmptySeq,
            Expr::New(_, args) => {
                let kind = match ty {
                    Ty::Decl(d, _) => UnityKind::from_full(self.u.full_name(*d)),
                    _ => None,
                };
                let nums: Option<Vec<f64>> = args
                    .iter()
                    .map(|a| match const_num(a)? {
                        #[allow(clippy::cast_precision_loss)]
                        Num::I(v) => Some(v as f64),
                        Num::F(v) => Some(v),
                    })
                    .collect();
                match (kind, nums) {
                    (Some(UnityKind::Vector2), Some(n)) if n.len() == 2 => Def::Floats(n),
                    (Some(UnityKind::Vector3), Some(mut n)) if (2..=3).contains(&n.len()) => {
                        n.resize(3, 0.0);
                        Def::Floats(n)
                    }
                    (Some(UnityKind::Vector4), Some(mut n)) if (2..=4).contains(&n.len()) => {
                        n.resize(4, 0.0);
                        Def::Floats(n)
                    }
                    (Some(UnityKind::Color), Some(mut n)) if (3..=4).contains(&n.len()) => {
                        n.resize(4, 1.0);
                        Def::Floats(n)
                    }
                    _ => Def::Unknown("constructor call".into()),
                }
            }
            _ => {
                // Numbers and enum members.
                if let Ty::Decl(d, _) = ty
                    && self.u.types[*d].kind != Kind::Enum
                {
                    // A literal assigned to a class or struct goes through an implicit
                    // conversion operator (e.g. `ContextValue Bonus = 0`).
                    return Def::Unknown("implicit conversion".into());
                }
                if let Ty::Decl(d, _) = ty
                    && self.u.types[*d].kind == Kind::Enum
                {
                    let ei = self.intern_enum(*d);
                    let members = self.enums[ei].members.clone();
                    let (bits, signed) = (self.enums[ei].bits, self.enums[ei].signed);
                    return eval_enum(e, &members, self.u, *d, bits, signed)
                        .map_or_else(|| Def::Unknown(expr_kind(e)), Def::Int);
                }
                match const_num(e) {
                    Some(Num::I(v)) => match ty {
                        Ty::Prim(Prim::F32 | Prim::F64) => {
                            #[allow(clippy::cast_precision_loss)]
                            let f = v as f64;
                            Def::Float(f)
                        }
                        _ => i64::try_from(v)
                            .map_or_else(|_| Def::Unknown("big int".into()), Def::Int),
                    },
                    Some(Num::F(v)) => Def::Float(v),
                    None => Def::Unknown(expr_kind(e)),
                }
            }
        }
    }
}

fn contains_generic_param(t: &Ty) -> bool {
    match t {
        Ty::GenericParam(_) => true,
        Ty::Decl(_, a) | Ty::External(_, a) => a.iter().any(contains_generic_param),
        Ty::Array(e, _) | Ty::List(e) | Ty::Nullable(e) => contains_generic_param(e),
        _ => false,
    }
}

fn expr_kind(e: &Expr) -> String {
    match e {
        Expr::Other(_) => "expression".into(),
        Expr::Ident(p) => format!("name {}", p.join(".")),
        Expr::Cast(..) => "cast".into(),
        _ => "constant".into(),
    }
}

fn is_empty_seq_text(s: &str) -> bool {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    (t.starts_with("new") && t.ends_with("[0]"))
        || t.starts_with("Array.Empty<")
        || (t.starts_with("new") && t.ends_with("[]{}"))
}

#[must_use]
pub fn hook_name(h: u32) -> &'static str {
    match h {
        H_ON_ENABLE => "OnEnable",
        H_ON_ENABLE_WITH_LIBRARY => "OnEnableWithLibrary",
        H_ON_DESERIALIZED => "[OnDeserialized]",
        H_ON_DESERIALIZING => "[OnDeserializing]",
        H_AFTER_DESERIALIZE => "OnAfterDeserialize",
        H_CTOR_BODY => "constructor body",
        H_ON_VALIDATE => "OnValidate",
        H_PACKING_CALLBACK => "OnPrePacked/OnPostPacked",
        _ => "?",
    }
}

/// An enum declaration's underlying type and members, evaluated with C# typing.
pub(crate) fn enum_def(u: &Universe, d: usize) -> SEnum {
    let raw = &u.types[d];
    let (bits, signed) = match raw.bases.first().map(|b| u.resolve(b, Some(d))) {
        Some(Ty::Prim(p)) => prim_int(p).unwrap_or((32, true)),
        _ => (32, true),
    };
    let mut members: Vec<(String, i64)> = Vec::new();
    let mut unresolved = Vec::new();
    let mut next: i64 = 0;
    for (name, e) in &raw.enum_members {
        let v = match e {
            None => Some(next),
            Some(e) => eval_enum(e, &members, u, d, bits, signed),
        };
        match v {
            Some(v) => {
                members.push((name.clone(), v));
                next = v.wrapping_add(1);
            }
            None => unresolved.push(name.clone()),
        }
    }
    SEnum {
        name: u.full_name(d).to_owned(),
        bits,
        signed,
        flags: raw.has_attr("Flags"),
        members,
        unresolved,
    }
}

pub(crate) fn prim_int(p: Prim) -> Option<(u8, bool)> {
    Some(match p {
        Prim::I8 => (8, true),
        Prim::U8 => (8, false),
        Prim::I16 => (16, true),
        Prim::U16 => (16, false),
        Prim::I32 => (32, true),
        Prim::U32 => (32, false),
        Prim::I64 => (64, true),
        Prim::U64 => (64, false),
        Prim::IntPtr => (64, true),
        _ => return None,
    })
}

enum Num {
    I(i128),
    F(f64),
}

/// The integral types of C# constant expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CT {
    I32,
    U32,
    I64,
    U64,
}

impl CT {
    fn bits(self) -> u32 {
        match self {
            Self::I32 | Self::U32 => 32,
            Self::I64 | Self::U64 => 64,
        }
    }

    fn signed(self) -> bool {
        matches!(self, Self::I32 | Self::I64)
    }

    /// Unchecked conversion of `v` to this type.
    fn wrap(self, v: i128) -> i128 {
        let bits = self.bits();
        let u = v & ((1i128 << bits) - 1);
        if self.signed() && (u >> (bits - 1)) & 1 == 1 { u - (1i128 << bits) } else { u }
    }

    /// The type of an integer literal (C# §6.4.5.3).
    fn literal(v: i128, (u, l): (bool, bool)) -> Self {
        let fits = |t: Self| t.wrap(v) == v;
        let order: &[Self] = match (u, l) {
            (false, false) => &[Self::I32, Self::U32, Self::I64, Self::U64],
            (true, false) => &[Self::U32, Self::U64],
            (false, true) => &[Self::I64, Self::U64],
            (true, true) => &[Self::U64],
        };
        order.iter().copied().find(|&t| fits(t)).unwrap_or(Self::U64)
    }

    /// Binary numeric promotion.
    fn promote(a: Self, b: Self) -> Self {
        use CT::{I32, I64, U32, U64};
        match (a, b) {
            (U64, _) | (_, U64) => U64,
            (I64, _) | (_, I64) | (U32, I32) | (I32, U32) => I64,
            (U32, U32) => U32,
            _ => I32,
        }
    }

    fn from_type_name(n: &str) -> Option<Self> {
        Some(match n {
            "int" | "Int32" | "short" | "Int16" | "sbyte" | "SByte" | "byte" | "Byte"
            | "ushort" | "UInt16" | "char" => Self::I32,
            "uint" | "UInt32" => Self::U32,
            "long" | "Int64" => Self::I64,
            "ulong" | "UInt64" => Self::U64,
            _ => return None,
        })
    }
}

/// Evaluates an integral constant expression with C# typing: literal types, binary
/// promotion, shift counts masked to the operand width, wrapping casts. `name` resolves
/// identifiers (enum members), giving a value already in the right type.
/// A typed constant value.
type Const = (i128, CT);

fn eval_int(e: &Expr, name: &dyn Fn(&[String]) -> Option<Const>) -> Option<Const> {
    Some(match e {
        Expr::Int(v, suffix) => (*v, CT::literal(*v, *suffix)),
        Expr::Char(c) => (i128::from(*c), CT::I32),
        Expr::Ident(p) => match p.as_slice() {
            [t, m] if m == "MaxValue" || m == "MinValue" => {
                let ct = CT::from_type_name(t)?;
                let (lo, hi) = match t.as_str() {
                    "int" | "Int32" => (i128::from(i32::MIN), i128::from(i32::MAX)),
                    "uint" | "UInt32" => (0, i128::from(u32::MAX)),
                    "long" | "Int64" => (i128::from(i64::MIN), i128::from(i64::MAX)),
                    "ulong" | "UInt64" => (0, i128::from(u64::MAX)),
                    _ => return None,
                };
                (if m == "MaxValue" { hi } else { lo }, ct)
            }
            _ => name(p)?,
        },
        Expr::Unary(op, x) => {
            let (v, t) = eval_int(x, name)?;
            match op.as_str() {
                "+" => (v, t),
                // `-uint` is long in C#.
                "-" => {
                    let t = if t == CT::U32 { CT::I64 } else { t };
                    (t.wrap(-v), t)
                }
                "~" => (t.wrap(!v), t),
                _ => return None,
            }
        }
        Expr::Cast(ty, x) => {
            let (v, _) = eval_int(x, name)?;
            let n = match ty {
                crate::parse::TypeExpr::Predefined(n) => n.clone(),
                crate::parse::TypeExpr::Name { segments, .. } => segments.last()?.0.clone(),
                _ => return None,
            };
            match CT::from_type_name(&n) {
                Some(t) => {
                    // Narrow types first wrap to their own width.
                    let w = match n.as_str() {
                        "byte" | "Byte" => v & 0xff,
                        "char" => v & 0xffff,
                        "sbyte" | "SByte" => ((v & 0xff) ^ 0x80) - 0x80,
                        "short" | "Int16" => ((v & 0xffff) ^ 0x8000) - 0x8000,
                        "ushort" | "UInt16" => v & 0xffff,
                        _ => t.wrap(v),
                    };
                    (w, t)
                }
                // A cast to an enum type: keep the value; the caller converts at the end.
                None => (v, CT::I64),
            }
        }
        Expr::Binary(op, l, r) => {
            let (a, ta) = eval_int(l, name)?;
            let (b, tb) = eval_int(r, name)?;
            if op == "<<" || op == ">>" {
                let t = CT::promote(ta, CT::I32);
                let n = u32::try_from(b & i128::from(t.bits() - 1)).ok()?;
                let v = if op == "<<" { a << n } else { t.wrap(a) >> n };
                return Some((t.wrap(v), t));
            }
            let t = CT::promote(ta, tb);
            let v = match op.as_str() {
                "|" => a | b,
                "&" => a & b,
                "^" => a ^ b,
                "+" => a + b,
                "-" => a - b,
                "*" => a * b,
                _ => return None,
            };
            (t.wrap(v), t)
        }
        _ => return None,
    })
}

fn const_num(e: &Expr) -> Option<Num> {
    if let Some((v, _)) = eval_int(e, &|_| None) {
        return Some(Num::I(v));
    }
    Some(match e {
        Expr::Float(v) => Num::F(*v),
        Expr::Unary(op, x) if op == "-" => match const_num(x)? {
            Num::I(v) => Num::I(-v),
            Num::F(v) => Num::F(-v),
        },
        Expr::Unary(op, x) if op == "+" => const_num(x)?,
        Expr::Cast(_, x) => const_num(x)?,
        Expr::Ident(p)
            if p.len() == 2
                && p[1] == "MaxValue"
                && matches!(p[0].as_str(), "float" | "Single") =>
        {
            Num::F(f64::from(f32::MAX))
        }
        Expr::Ident(p)
            if p.len() == 2
                && p[1] == "MinValue"
                && matches!(p[0].as_str(), "float" | "Single") =>
        {
            Num::F(f64::from(f32::MIN))
        }
        Expr::Ident(p) if p.len() == 2 && p[1] == "PositiveInfinity" => Num::F(f64::INFINITY),
        Expr::Ident(p) if p.len() == 2 && p[1] == "NegativeInfinity" => Num::F(f64::NEG_INFINITY),
        _ => return None,
    })
}

/// Evaluates an enum member value or an enum-typed default: other members of the same
/// enum (bare or `Enum.Member`) have the underlying type; the result is converted to it.
fn eval_enum(
    e: &Expr,
    members: &[(String, i64)],
    u: &Universe,
    d: usize,
    bits: u8,
    signed: bool,
) -> Option<i64> {
    let ename = &u.types[d].name;
    let under = match (bits, signed) {
        (64, true) => CT::I64,
        (64, false) => CT::U64,
        (_, false) if bits == 32 => CT::U32,
        _ => CT::I32,
    };
    let lookup = |p: &[String]| -> Option<(i128, CT)> {
        let last = p.last()?;
        if p.len() > 1 && p[p.len() - 2] != *ename {
            return None;
        }
        members.iter().find(|(n, _)| n == last).map(|(_, v)| (i128::from(*v), under))
    };
    let (v, _) = eval_int(e, &lookup)?;
    // Convert to the underlying type (narrow enums wrap to their own width).
    let w = if bits < 32 {
        let m = (1i128 << bits) - 1;
        let x = v & m;
        if signed && (x >> (bits - 1)) & 1 == 1 { x - (1i128 << bits) } else { x }
    } else {
        under.wrap(v)
    };
    #[allow(clippy::cast_possible_truncation)]
    Some(w as i64)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::parse::parse_file;

    const STUBS: &str = r#"
namespace UnityEngine { public class Object {} public class ScriptableObject : Object {}
  public class Sprite : Object {} public struct Vector3 { public float x; public float y; public float z; }
  public sealed class SerializeField : System.Attribute {} }
namespace Kingmaker.Blueprints {
  [Serializable] [TypeId("0b3cc43201601904bb7eb333c9b646ff")] public class SimpleBlueprint { [JsonIgnore] public string name; }
  [Serializable] public class BlueprintReferenceBase { [SerializeField][JsonProperty] private string guid; }
  [Serializable] public class BlueprintReference<T> : BlueprintReferenceBase where T : SimpleBlueprint {}
  [Serializable] [TypeId("0956112276c2f8340b645b372472e09a")] public class BlueprintComponent { public string name; }
}
"#;

    const SRC: &str = r#"
using System;
using System.Collections.Generic;
using Kingmaker.Blueprints;
using UnityEngine;
namespace Game;
[Flags] public enum Kind { None = 0, A = 1, B = 1 << 1, AB = A | B }
public enum Wide { Top = 1 << 31, Wrap = 1 << 32, Neg = ~0, U = (int)0x80000000u }
[TypeId("11111111111111111111111111111111")]
public class BlueprintThing : SimpleBlueprint {
  public int Count = 3;
  [SerializeField] private float m_Speed = 1.5f;
  private int m_Cache;
  [NonSerialized] public int Runtime;
  public static int Static;
  public const int Const = 1;
  [JsonProperty(PropertyName = "Renamed")] [SerializeField] private string m_Old;
  public Dictionary<int, int> Map;
  public List<List<int>> Nested;
  public IDisposable Iface;
  public Kind Flags = Kind.AB;
  public BlueprintThingReference Other;
  public Vector3 Where;
  public Sprite Icon;
  public Inner Data;
  public Wide W;
  public bool Ctor;
  public BlueprintThing() { Ctor = true; }
  public List<BlueprintComponent> Parts;
  [Serializable] public class Inner { public bool On; }
  public override void OnEnable() { Count++; }
}
[Serializable] public class BlueprintThingReference : BlueprintReference<BlueprintThing> {}
[TypeId("22222222222222222222222222222222")]
public class Derived : BlueprintThing { public new int Count; }
"#;

    fn build() -> (Universe, Vec<String>) {
        let a: Arc<str> = Arc::from("Assembly-CSharp");
        let mut types = parse_file(&a, "stubs.cs", STUBS).unwrap();
        let base = types.len();
        let mut more = parse_file(&a, "src.cs", SRC).unwrap();
        for t in &mut more {
            if let Some(o) = t.outer.as_mut() {
                *o += base;
            }
        }
        types.extend(more);
        let u = Universe::new(types);
        (u, Vec::new())
    }

    #[test]
    fn serialization_rules() {
        let (u, _) = build();
        let mut b = Builder::new(&u);
        b.build_all();
        let thing = b.types.iter().find(|t| t.short == "BlueprintThing").unwrap();
        assert_eq!(thing.guid.as_deref(), Some("11111111111111111111111111111111"));
        assert!(thing.flags & T_BLUEPRINT != 0 && thing.flags & T_IDENTIFIED != 0);
        let names: Vec<&str> = thing.fields.iter().map(|f| f.name.as_str()).collect();
        // Base first; private without [SerializeField], [NonSerialized], static, const,
        // dictionaries, nested lists and interfaces are all left out.
        assert_eq!(
            names,
            [
                "name", "Count", "m_Speed", "m_Old", "Flags", "Other", "Where", "Icon", "Data",
                "W", "Ctor", "Parts"
            ]
        );
        let f = |n: &str| thing.fields.iter().find(|f| f.name == n).unwrap();
        assert_eq!(f("name").flags & F_JSON_IGNORE, F_JSON_IGNORE);
        assert_eq!(f("m_Old").json, "Renamed");
        assert_eq!(f("Count").default, Def::Int(3));
        assert_eq!(f("m_Speed").default, Def::Float(1.5));
        assert!(matches!(f("Where").codec, Codec::Unity(UnityKind::Vector3)));
        assert!(matches!(&f("Icon").codec, Codec::Asset(n) if n == "UnityEngine.Sprite"));
        let Codec::BpRef { target, .. } = f("Other").codec else { panic!("reference") };
        assert_eq!(b.types[target].short, "BlueprintThing");
        let Codec::Enum(e) = f("Flags").codec else { panic!("enum") };
        assert!(b.enums[e].flags);
        assert_eq!(
            b.enums[e].members,
            [("None".into(), 0), ("A".into(), 1), ("B".into(), 2), ("AB".into(), 3)]
        );
        assert_eq!(f("Flags").default, Def::Int(3));
        // C# constant typing: int shifts wrap and mask the count; casts wrap.
        let Codec::Enum(w) = f("W").codec else { panic!("enum") };
        assert_eq!(
            b.enums[w].members,
            [
                ("Top".into(), -2_147_483_648),
                ("Wrap".into(), 1),
                ("Neg".into(), -1),
                ("U".into(), -2_147_483_648)
            ]
        );
        // A parameterless constructor's assignment is the default.
        assert_eq!(f("Ctor").default, Def::Bool(true));
        assert!(matches!(f("Parts").codec, Codec::Seq { list: true, .. }));
        assert!(thing.hooks & H_ON_ENABLE != 0);
        assert!(thing.runtime_fields.iter().any(|r| r.ends_with("m_Cache")));
        // `new` hides the base field in the JSON; the pack keeps both.
        let derived = b.types.iter().find(|t| t.short == "Derived").unwrap();
        let counts: Vec<&SField> = derived.fields.iter().filter(|f| f.name == "Count").collect();
        assert_eq!(counts.len(), 2);
        assert_eq!(counts[0].flags & F_HIDDEN, F_HIDDEN);
        assert_eq!(counts[1].flags & F_HIDDEN, 0);
    }

    const FACT_SRC: &str = r#"
namespace Kingmaker.Blueprints {
    [Serializable] [TypeId("00000000000000000000000000000001")]
    public class BlueprintComponent { public string name; }
}
namespace Kingmaker.PubSubSystem {
    public interface ISubscriber {}
    public interface IThingHandler : ISubscriber { void Handle(); }
    public interface IOddlyNamed : IThingHandler { }
}
namespace Kingmaker.EntitySystem {
    using Kingmaker.Blueprints;
    using Kingmaker.PubSubSystem;
    public interface IRuntimeEntityFactComponentProvider {}
    public abstract class EntityFactComponentDelegate<TEntity, TData> : BlueprintComponent, IRuntimeEntityFactComponentProvider, ISubscriber {
        protected virtual void OnTurnOn() { }
        protected virtual void OnRecalculate() { OnTurnOff(); OnTurnOn(); }
    }
    [TypeId("00000000000000000000000000000002")]
    public class Bonus : EntityFactComponentDelegate<object, object>, IThingHandler, IOddlyNamed {
        protected override void OnTurnOn() { Apply(); }
        protected override void OnTurnOff() { Remove(); }
        public void Handle() { }
    }
    [TypeId("00000000000000000000000000000003")]
    public class Custom : BlueprintComponent, IRuntimeEntityFactComponentProvider { }
    [TypeId("00000000000000000000000000000004")]
    public class Plain : BlueprintComponent { protected void OnTurnOn() { X(); } }
    [TypeId("00000000000000000000000000000005")]
    public class OwnRuntime : EntityFactComponentDelegate<object, object> {
        public override EntityFactComponent CreateRuntimeFactComponent() { return new R(); }
    }
}
"#;

    #[test]
    fn runtime_providers_hooks_and_handlers() {
        let a: Arc<str> = Arc::from("Assembly-CSharp");
        let u = Universe::new(parse_file(&a, "f.cs", FACT_SRC).unwrap());
        let mut b = Builder::new(&u);
        b.build_all();
        let t = |n: &str| b.types.iter().find(|t| t.short == n).unwrap();
        let bonus = t("Bonus");
        assert!(bonus.flags & T_RUNTIME_PROVIDER != 0);
        // The delegate base's virtuals (empty, or `OnRecalculate`'s default) don't count.
        assert_eq!(bonus.fact_hooks, FH_TURN_ON | FH_TURN_OFF);
        assert_eq!(bonus.handlers, vec!["IThingHandler".to_owned(), "IOddlyNamed".to_owned()]);
        let custom = t("Custom");
        assert!(custom.flags & T_RUNTIME_PROVIDER != 0);
        assert_eq!(custom.fact_hooks, FH_CUSTOM_RUNTIME);
        let plain = t("Plain");
        assert_eq!(plain.flags & T_RUNTIME_PROVIDER, 0);
        // Not a runtime provider: a method named like a hook is still listed, but nothing
        // ever calls it (the sim only dispatches to runtime components).
        assert_eq!(plain.fact_hooks, FH_TURN_ON);
        assert_eq!(t("OwnRuntime").fact_hooks, FH_CUSTOM_RUNTIME);
    }
}
