//! Save-side schema: the types the game's save documents deserialize into, following
//! `DefaultJsonSettings` (decompiled `Kingmaker.EntitySystem.Persistence.JsonUtility`):
//!
//! - `OptInContractResolver`: members are fields and properties with `[JsonProperty]`
//!   (any visibility, base classes included), except for types in
//!   `UnityEngine.CoreModule`, which get Newtonsoft's default contract (public fields and
//!   public readable properties);
//! - the converter list in `DefaultJsonSettings.CommonConverters`, matched on the
//!   *declared* type in registration order, with `CanRead == false` converters falling
//!   back to the contract;
//! - Newtonsoft's own contract choice (`DefaultContractResolver.CreateContract`):
//!   `[JsonObject]`, dictionaries, `IEnumerable` → arrays, otherwise objects;
//! - `TypeNameHandling.Auto`: any `$type` assignable to the declared type, so every
//!   subclass (or implementor) of a declared class (or interface) joins the closure.
//!
//! Closed generic types carry all their CLR generic arguments, outer types' first (a
//! nested type of `Outer<T>` is `Outer`1+Nested[[T]]` in the CLR). The schema is keyed by
//! the CLR name `$type` uses, with assembly qualifications removed.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use serde::Serialize;

use crate::model::{Prim, Ty, Universe};
use crate::parse::{Attr, Kind, TypeExpr};
use crate::rules::{self, SEnum, UnityKind};

/// The documents' root types (`SaveManager`, `AreaDataStash`, `ThreadedGameLoader`).
pub const ROOTS: &[&str] = &[
    "Kingmaker.EntitySystem.Persistence.SaveInfo",
    "Kingmaker.Player",
    "Kingmaker.EntitySystem.SceneEntitiesState",
    "Kingmaker.EntitySystem.AreaPersistentState",
    "TurnBased.Controllers.CombatController",
    "Kingmaker.QA.Clockwork.PlayData",
];

const UNITY_CORE: &str = "UnityEngine.CoreModule";
const SIMPLE_BLUEPRINT: &str = "Kingmaker.Blueprints.SimpleBlueprint";
const BP_REF_BASE: &str = "Kingmaker.Blueprints.BlueprintReferenceBase";
const BP_REF_GENERIC: &str = "Kingmaker.Blueprints.BlueprintReference";

/// Sequence flavours (they differ in how the game rebuilds them, not in the JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
pub enum SeqKind {
    Array,
    List,
    HashSet,
    /// `Stack<T>`: Newtonsoft rebuilds it from a list, which reverses the order.
    Stack,
    Queue,
    /// Other `IEnumerable<T>` (interfaces deserialize as `List<T>`).
    Other,
}

/// Entity reference flavours of `EntityRefConverter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
pub enum RefKind {
    Unit,
    MapObject,
    Entity,
    /// `EntityRef<T>`: null for a null id (the others keep an empty reference).
    Typed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, PartialOrd, Ord)]
pub enum SCodec {
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
    /// `System.Version` (`VersionConverter`).
    Version,
    /// `byte[]` (base64).
    Bytes,
    /// `StringEnumConverter`.
    Enum(usize),
    /// `Nullable<T>`: null or `T`'s encoding.
    Nullable(Box<SCodec>),
    /// An object contract: `$id`/`$ref`, `$type` when the runtime type differs.
    Object(usize),
    /// `BlueprintConverter`: a GUID string, `"null"` or null. The declared type.
    Blueprint(usize),
    /// `BlueprintRefConverter`: a GUID string, `{"guid": …}` or null; target blueprint type.
    BpRef(usize),
    /// `BlueprintGuidConverter`.
    BlueprintGuid,
    /// `EntityRefConverter`.
    EntityRef(RefKind),
    /// `Vector2Converter`, `Vector3Converter`, `QuaternionConverter`: `"x|y(|z)"` in
    /// hundredths, or an object.
    Vector2,
    Vector3,
    Quaternion,
    /// `BoneTransformDataConverter`: `"x|y|z|ex|ey|ez"`.
    BoneTransform,
    /// `BoundsConverter`: `{"center", "size"}`.
    Bounds,
    /// `CountableFlagConverter`: an integer, null, or `{"m_Count": n}`.
    CountableFlag,
    /// `SpriteConverter`: always null.
    Sprite,
    /// `LocalizationPack.EntryConverter`: a string.
    StringEntry,
    /// `DictionaryConverter` (generic `Dictionary<,>` / `IDictionary<,>`): an object keyed
    /// by string, int or enum, or an array of `{Key, Value}`.
    Dict {
        key: Box<SCodec>,
        value: Box<SCodec>,
    },
    /// A dictionary contract without the converter (other `IDictionary` types): an object.
    DictContract {
        key: Box<SCodec>,
        value: Box<SCodec>,
    },
    /// Newtonsoft's `KeyValuePairConverter`.
    KeyValue {
        key: Box<SCodec>,
        value: Box<SCodec>,
    },
    Seq {
        kind: SeqKind,
        elem: Box<SCodec>,
    },
    /// A `UnityEngine.Object` (not a blueprint): Newtonsoft would walk the engine object's
    /// public members; saves only ever hold null here. The declared type.
    UnityObject(String),
    /// `System.Object`: whatever the JSON holds (`$type` objects resolve by name).
    Any,
    /// `JToken`/`JObject`/`JArray`: raw JSON.
    JToken,
    Unsupported(String),
}

pub const T_ABSTRACT: u32 = 1;
pub const T_STRUCT: u32 = 2;
pub const T_INTERFACE: u32 = 4;
/// Unity core type: Newtonsoft's default contract.
pub const T_UNITY: u32 = 8;
/// `[JsonObject(IsReference = false)]`.
pub const T_NOT_REFERENCE: u32 = 16;
/// Created through a constructor with parameters (`[JsonConstructor]`): Newtonsoft reads
/// every property first and registers the object's `$id` only after construction.
pub const T_CREATOR_PARAMS: u32 = 32;
/// No usable constructor: Newtonsoft can't create it.
pub const T_NO_CREATOR: u32 = 64;
/// `EntityDataBase` subclass.
pub const T_ENTITY: u32 = 128;
/// `EntityFact` subclass.
pub const T_FACT: u32 = 256;
/// `EntityPart` subclass.
pub const T_PART: u32 = 512;
/// `EntityFactComponent` subclass.
pub const T_COMPONENT: u32 = 1024;
/// A `sealed` class or a struct: no subclasses.
pub const T_SEALED: u32 = 2048;
/// Array contract (an `IEnumerable` without `[JsonObject]`).
pub const T_ARRAY_CONTRACT: u32 = 4096;
/// Dictionary contract.
pub const T_DICT_CONTRACT: u32 = 8192;

pub const M_PROPERTY: u16 = 1;
/// Newtonsoft can read the member (written to saves).
pub const M_READABLE: u16 = 2;
/// Newtonsoft can set the member when loading.
pub const M_WRITABLE: u16 = 4;
pub const M_DEFAULT_IGNORE: u16 = 8;
pub const M_NULL_IGNORE: u16 = 16;
/// `ObjectCreationHandling.Replace`.
pub const M_REPLACE: u16 = 32;
pub const M_STATIC: u16 = 64;
/// `[JsonProperty(TypeNameHandling = …)]` or item settings that we don't model.
pub const M_UNMODELLED_SETTINGS: u16 = 128;
/// The JSON property feeds a creator parameter (matched case-insensitively), so it is
/// read even when the member has no setter.
pub const M_CTOR_PARAM: u16 = 256;

#[derive(Debug, Clone, Serialize)]
pub struct SMember {
    pub json: String,
    pub name: String,
    pub codec: SCodec,
    pub flags: u16,
    pub declared_in: usize,
}

/// Lifecycle hooks a hydrating loader must run, by method name.
pub const HOOK_NAMES: &[&str] = &[
    "OnDeserialized",
    "OnDeserializing",
    "OnPostLoad",
    "PostLoad",
    "OnPreSave",
    "PreSave",
    "OnApplyPostLoadFixes",
    "ApplyPostLoadFixes",
    "OnComponentsDidPostLoad",
    "OnTurnOn",
    "OnActivate",
    "OnAttach",
    "OnRecalculate",
];

#[derive(Debug, Clone, Serialize)]
pub struct SType {
    /// CLR name without assembly qualification (the `$type` key).
    pub clr: String,
    pub assembly: String,
    pub flags: u32,
    pub base: Option<usize>,
    /// Interfaces in the schema that the type implements (for `$type` assignability).
    pub ifaces: Vec<usize>,
    pub members: Vec<SMember>,
    /// Hooks declared on this type or a base (name → declaring type).
    pub hooks: BTreeMap<String, String>,
    /// Parameter names of the constructor Newtonsoft calls with arguments.
    pub creator_params: Vec<String>,
    #[serde(skip)]
    pub creator_types: Vec<TypeExpr>,
    pub file: String,
    #[serde(skip)]
    pub ty: Ty,
}

#[derive(Debug, Default, Serialize)]
pub struct Problems {
    pub unresolved: BTreeMap<String, Vec<String>>,
    pub unsupported: BTreeMap<String, Vec<String>>,
    pub duplicate_json_names: Vec<String>,
    pub unmodelled_member_settings: Vec<String>,
    pub custom_collections: Vec<String>,
    pub static_members: Vec<String>,
}

/// A type candidate for `$type`: a closed class or struct.
#[derive(Debug)]
struct PoolEntry {
    ty: Ty,
}

#[derive(Debug)]
pub struct SaveBuilder<'u> {
    pub u: &'u Universe,
    pub types: Vec<SType>,
    pub enums: Vec<SEnum>,
    /// Assembly of each enum (for `$type` generic-argument checks).
    pub enum_asm: Vec<String>,
    /// CLR name of each enum (`+` between nested types).
    pub enum_clr: Vec<String>,
    index: HashMap<Ty, usize>,
    enum_index: HashMap<String, usize>,
    pending: Vec<usize>,
    /// Ancestor → pool entries deriving from it.
    descendants: HashMap<Ty, Vec<usize>>,
    pool: Vec<PoolEntry>,
    pub problems: Problems,
}

impl<'u> SaveBuilder<'u> {
    #[must_use]
    pub fn new(u: &'u Universe) -> Self {
        let mut b = Self {
            u,
            types: Vec::new(),
            enums: Vec::new(),
            enum_asm: Vec::new(),
            enum_clr: Vec::new(),
            index: HashMap::new(),
            enum_index: HashMap::new(),
            pending: Vec::new(),
            descendants: HashMap::new(),
            pool: Vec::new(),
            problems: Problems::default(),
        };
        b.build_pool();
        b
    }

    // ---- generics ---------------------------------------------------------------

    /// CLR generic parameters of a declaration: enclosing types' first, then its own.
    fn clr_params(&self, d: usize) -> Vec<String> {
        let mut chain = vec![d];
        let mut cur = self.u.types[d].outer;
        while let Some(o) = cur {
            chain.push(o);
            cur = self.u.types[o].outer;
        }
        chain.reverse();
        chain.iter().flat_map(|&c| self.u.types[c].generics.iter().cloned()).collect()
    }

    /// Generic parameters contributed by enclosing types only.
    fn outer_params(&self, d: usize) -> Vec<String> {
        let mut p = self.clr_params(d);
        let own = self.u.types[d].generics.len();
        p.truncate(p.len() - own);
        p
    }

    fn env(&self, t: &Ty) -> Vec<(String, Ty)> {
        match t {
            Ty::Decl(d, args) => {
                self.clr_params(*d).into_iter().zip(args.iter().cloned()).collect()
            }
            _ => Vec::new(),
        }
    }

    /// Closes a type written inside the declaration of `ctx` (a closed type): substitutes
    /// generic parameters and gives nested types of generic types their enclosing types'
    /// arguments. A nested type inherited from a base class takes the arguments of the
    /// closed base found in the enclosing types' ancestry (and, for member types, in
    /// `ctx`'s own when `own_ancestry` is set; not for base clauses, which would recurse).
    fn close(&self, t: &Ty, ctx: &Ty, own_ancestry: bool) -> Ty {
        let env = self.env(ctx);
        self.close_env(t, ctx, &env, own_ancestry)
    }

    fn close_env(&self, t: &Ty, ctx: &Ty, env: &[(String, Ty)], own: bool) -> Ty {
        match t {
            Ty::GenericParam(n) => {
                env.iter().find(|(p, _)| p == n).map_or_else(|| t.clone(), |(_, v)| v.clone())
            }
            Ty::Decl(d, args) => {
                let own_args: Vec<Ty> =
                    args.iter().map(|a| self.close_env(a, ctx, env, own)).collect();
                let outer = self.outer_params(*d);
                if outer.is_empty() || args.len() == self.clr_params(*d).len() {
                    return Ty::Decl(*d, own_args);
                }
                if let Some(o) = self.u.types[*d].outer
                    && let Some(oargs) = self.enclosing_args(o, ctx, own)
                {
                    let mut full = oargs;
                    full.extend(own_args);
                    return Ty::Decl(*d, full);
                }
                let mut full: Vec<Ty> = outer
                    .iter()
                    .map(|p| {
                        env.iter()
                            .find(|(q, _)| q == p)
                            .map_or_else(|| Ty::GenericParam(p.clone()), |(_, v)| v.clone())
                    })
                    .collect();
                full.extend(own_args);
                Ty::Decl(*d, full)
            }
            Ty::Array(e, r) => Ty::Array(Box::new(self.close_env(e, ctx, env, own)), *r),
            Ty::List(e) => Ty::List(Box::new(self.close_env(e, ctx, env, own))),
            Ty::Nullable(e) => Ty::Nullable(Box::new(self.close_env(e, ctx, env, own))),
            Ty::External(n, a) => Ty::External(
                n.clone(),
                a.iter().map(|x| self.close_env(x, ctx, env, own)).collect(),
            ),
            _ => t.clone(),
        }
    }

    /// The closed arguments of declaration `o` as seen from `ctx`: `ctx` or one of its
    /// enclosing instances, or a closed base class of those.
    fn enclosing_args(&self, o: usize, ctx: &Ty, own: bool) -> Option<Vec<Ty>> {
        let mut scopes: Vec<Ty> = Vec::new();
        let mut cur = Some(ctx.clone());
        while let Some(c) = cur {
            let Ty::Decl(cd, cargs) = &c else { break };
            cur = self.u.types[*cd].outer.map(|od| {
                let k = self.clr_params(od).len().min(cargs.len());
                Ty::Decl(od, cargs[..k].to_vec())
            });
            scopes.push(c);
        }
        for (k, s) in scopes.iter().enumerate() {
            if matches!(s, Ty::Decl(sd, _) if *sd == o) {
                return match s {
                    Ty::Decl(_, a) => Some(a.clone()),
                    _ => None,
                };
            }
            if k == 0 && !own {
                continue;
            }
            for a in self.class_chain(s) {
                if let Ty::Decl(ad, aargs) = &a
                    && *ad == o
                {
                    return Some(aargs.clone());
                }
            }
        }
        None
    }

    /// `t` and its closed base classes.
    fn class_chain(&self, t: &Ty) -> Vec<Ty> {
        let mut out = Vec::new();
        let mut cur = Some(t.clone());
        while let Some(c) = cur {
            if out.len() > 64 {
                break;
            }
            cur = self.base_of(&c);
            out.push(c);
        }
        out
    }

    /// The declaration's base class as written (resolved, unsubstituted). Falls back to a
    /// lookup that sees nested types inherited by the enclosing type, which the model
    /// skips when computing bases.
    fn raw_base(&self, d: usize) -> Option<Ty> {
        if let Some(b) = self.u.base_class(d) {
            return Some(b);
        }
        let raw = &self.u.types[d];
        if !matches!(raw.kind, Kind::Class | Kind::Record) {
            return None;
        }
        let first = raw.bases.first()?;
        let ctx = raw.outer?;
        match self.u.resolve(first, Some(ctx)) {
            r @ Ty::Decl(b, _)
                if matches!(self.u.types[b].kind, Kind::Class | Kind::Record) && b != d =>
            {
                Some(r)
            }
            _ => None,
        }
    }

    fn is_closed(t: &Ty) -> bool {
        match t {
            Ty::GenericParam(_) | Ty::Unresolved(_) => false,
            Ty::Decl(_, a) | Ty::External(_, a) => a.iter().all(Self::is_closed),
            Ty::Array(e, _) | Ty::List(e) | Ty::Nullable(e) => Self::is_closed(e),
            Ty::Prim(_) => true,
        }
    }

    /// The closed base class.
    fn base_of(&self, t: &Ty) -> Option<Ty> {
        let Ty::Decl(d, _) = t else { return None };
        let b = self.raw_base(*d)?;
        Some(self.close(&b, t, false))
    }

    /// Closed interfaces listed on the declaration (not inherited).
    fn own_interfaces(&self, t: &Ty) -> Vec<Ty> {
        let Ty::Decl(d, _) = t else { return Vec::new() };
        let raw = &self.u.types[*d];
        let skip = usize::from(self.raw_base(*d).is_some());
        raw.bases
            .iter()
            .skip(skip)
            .map(|b| self.close(&self.u.resolve(b, Some(*d)), t, false))
            .filter(|x| matches!(x, Ty::Decl(i, _) if self.u.types[*i].kind == Kind::Interface))
            .collect()
    }

    /// Class chain from `t` up, then every interface (transitively).
    fn ancestors(&self, t: &Ty) -> Vec<Ty> {
        let mut out = Vec::new();
        let mut cur = Some(t.clone());
        let mut n = 0;
        while let Some(c) = cur {
            n += 1;
            if n > 64 {
                break;
            }
            cur = self.base_of(&c);
            out.push(c);
        }
        let mut stack: Vec<Ty> = out.iter().flat_map(|c| self.own_interfaces(c)).collect();
        let mut seen: HashSet<Ty> = HashSet::new();
        while let Some(i) = stack.pop() {
            if seen.insert(i.clone()) {
                stack.extend(self.own_interfaces(&i));
                out.push(i);
            }
        }
        out
    }

    /// Every closed class or struct a `$type` could name: non-generic declarations, closed
    /// generic types found in their ancestry, and nested types of those.
    fn build_pool(&mut self) {
        let mut seeds: Vec<Ty> = Vec::new();
        for (i, t) in self.u.types.iter().enumerate() {
            if matches!(t.kind, Kind::Class | Kind::Struct | Kind::Record)
                && self.clr_params(i).is_empty()
            {
                seeds.push(Ty::Decl(i, Vec::new()));
            }
        }
        let mut seen: HashSet<Ty> = seeds.iter().cloned().collect();
        let mut queue = seeds;
        // Nested types per declaration, for instantiating nested types of generic types.
        let mut nested: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, t) in self.u.types.iter().enumerate() {
            if let Some(o) = t.outer
                && matches!(t.kind, Kind::Class | Kind::Struct | Kind::Record)
                && t.generics.is_empty()
            {
                nested.entry(o).or_default().push(i);
            }
        }
        while let Some(t) = queue.pop() {
            let anc = self.ancestors(&t);
            for a in &anc {
                if let Ty::Decl(d, args) = a
                    && !args.is_empty()
                    && Self::is_closed(a)
                {
                    let mut new = Vec::new();
                    if matches!(self.u.types[*d].kind, Kind::Class | Kind::Struct | Kind::Record)
                        && seen.insert(a.clone())
                    {
                        new.push(a.clone());
                    }
                    if let Some(ns) = nested.get(d) {
                        for &n in ns {
                            let inst = Ty::Decl(n, args.clone());
                            if seen.insert(inst.clone()) {
                                new.push(inst);
                            }
                        }
                    }
                    queue.extend(new);
                }
            }
            let idx = self.pool.len();
            for a in anc.iter().skip(1) {
                self.descendants.entry(a.clone()).or_default().push(idx);
            }
            self.pool.push(PoolEntry { ty: t });
        }
    }

    // ---- closure ----------------------------------------------------------------

    pub fn build_all(&mut self) {
        for r in ROOTS {
            match self.u.find(r) {
                Some(d) => {
                    self.intern(&Ty::Decl(d, Vec::new()));
                }
                None => {
                    self.problems
                        .unresolved
                        .entry((*r).to_owned())
                        .or_default()
                        .push("root".into());
                }
            }
        }
        while let Some(i) = self.pending.pop() {
            self.fill(i);
        }
        for i in 0..self.types.len() {
            let ty = self.types[i].ty.clone();
            let mut ifaces: Vec<usize> = self
                .ancestors(&ty)
                .iter()
                .filter(|a| matches!(a, Ty::Decl(d, _) if self.u.types[*d].kind == Kind::Interface))
                .filter_map(|a| self.index.get(a).copied())
                .collect();
            ifaces.sort_unstable();
            ifaces.dedup();
            self.types[i].ifaces = ifaces;
        }
    }

    fn assembly_of(&self, t: &Ty) -> String {
        match t {
            Ty::Decl(d, _) => self.u.types[*d].assembly.to_string(),
            _ => "mscorlib".into(),
        }
    }

    fn intern(&mut self, ty: &Ty) -> usize {
        if let Some(&i) = self.index.get(ty) {
            return i;
        }
        let Ty::Decl(d, _) = ty else { unreachable!("intern({ty:?})") };
        let raw = &self.u.types[*d];
        let mut flags = 0;
        if raw.has_mod("abstract") || raw.has_mod("static") {
            flags |= T_ABSTRACT;
        }
        if raw.kind == Kind::Struct {
            flags |= T_STRUCT | T_SEALED;
        }
        if raw.has_mod("sealed") || raw.has_mod("static") {
            flags |= T_SEALED;
        }
        if raw.kind == Kind::Interface {
            flags |= T_INTERFACE | T_ABSTRACT;
        }
        if &*raw.assembly == UNITY_CORE {
            flags |= T_UNITY;
        }
        if raw.attrs.iter().any(|a| {
            a.name == "JsonObject"
                && a.args.iter().any(|(n, e)| {
                    n.as_deref() == Some("IsReference")
                        && matches!(e, crate::parse::Expr::Bool(false))
                })
        }) {
            flags |= T_NOT_REFERENCE;
        }
        let anc_names: Vec<String> = self
            .ancestors(ty)
            .iter()
            .filter_map(|a| match a {
                Ty::Decl(i, _) => Some(self.u.full_name(*i).to_owned()),
                _ => None,
            })
            .collect();
        let has = |n: &str| anc_names.iter().any(|a| a == n);
        if has("Kingmaker.EntitySystem.EntityDataBase") {
            flags |= T_ENTITY;
        }
        if has("Kingmaker.EntitySystem.EntityFact") {
            flags |= T_FACT;
        }
        if has("Kingmaker.EntitySystem.EntityPart") {
            flags |= T_PART;
        }
        if has("Kingmaker.EntitySystem.EntityFactComponent") {
            flags |= T_COMPONENT;
        }
        let (cf, creator) = self.creator(*d);
        let creator_params: Vec<String> = creator.iter().map(|(n, _)| n.clone()).collect();
        let creator_types: Vec<TypeExpr> = creator.into_iter().map(|(_, t)| t).collect();
        flags |= cf;
        let idx = self.types.len();
        self.types.push(SType {
            clr: self.clr_name(ty),
            assembly: self.assembly_of(ty),
            flags,
            base: None,
            ifaces: Vec::new(),
            members: Vec::new(),
            hooks: BTreeMap::new(),
            creator_params,
            creator_types,
            file: format!("{}:{}", raw.file, raw.line),
            ty: ty.clone(),
        });
        self.index.insert(ty.clone(), idx);
        self.pending.push(idx);
        idx
    }

    /// A class outside the decompiled sources whose opt-in contract has no members.
    fn intern_opaque_class(&mut self, name: &str, assembly: &str) -> usize {
        let ty = Ty::Unresolved(name.to_owned());
        if let Some(&i) = self.index.get(&ty) {
            return i;
        }
        let idx = self.types.len();
        self.types.push(SType {
            clr: format!("UniRx.{name}"),
            assembly: assembly.to_owned(),
            flags: T_SEALED,
            base: None,
            ifaces: Vec::new(),
            members: Vec::new(),
            hooks: BTreeMap::new(),
            creator_params: Vec::new(),
            creator_types: Vec::new(),
            file: String::new(),
            ty: ty.clone(),
        });
        self.index.insert(ty, idx);
        idx
    }

    /// `OptInContractResolver.CreateObjectContract` and Newtonsoft's creator choice, with
    /// the creator's parameter names when it has parameters.
    fn creator(&self, d: usize) -> (u32, Vec<(String, TypeExpr)>) {
        let raw = &self.u.types[d];
        if matches!(raw.kind, Kind::Interface | Kind::Struct) || raw.has_mod("abstract") {
            return (0, Vec::new());
        }
        let inst: Vec<_> = raw.ctors.iter().filter(|c| !c.is_static).collect();
        let ours = &*raw.assembly == "Assembly-CSharp";
        let mark = inst.iter().any(|c| {
            c.params.len() == 1
                && c.params[0].text().rsplit('.').next() == Some("JsonConstructorMark")
        });
        if ours && mark {
            return (0, Vec::new());
        }
        if let Some(c) = inst.iter().find(|c| c.attrs.iter().any(|a| a == "JsonConstructor")) {
            return if c.params.is_empty() {
                (0, Vec::new())
            } else {
                (
                    T_CREATOR_PARAMS,
                    c.param_names.iter().cloned().zip(c.params.iter().cloned()).collect(),
                )
            };
        }
        // No constructor declared: the implicit public parameterless one. Otherwise any
        // parameterless constructor (the resolver uses non-public ones; so does Newtonsoft's
        // own contract when there is no single public parameterized constructor).
        if inst.is_empty() || inst.iter().any(|c| c.params.is_empty()) {
            return (0, Vec::new());
        }
        // `DefaultContractResolver.GetParameterizedConstructor`: exactly one public one.
        let public: Vec<_> = inst.iter().filter(|c| c.is_public).collect();
        if public.len() == 1 {
            let c = public[0];
            (
                T_CREATOR_PARAMS,
                c.param_names.iter().cloned().zip(c.params.iter().cloned()).collect(),
            )
        } else {
            (T_NO_CREATOR, Vec::new())
        }
    }

    /// CLR name as `$type` writes it, minus assembly qualifications.
    #[must_use]
    pub fn clr_name(&self, t: &Ty) -> String {
        match t {
            Ty::Prim(p) => prim_clr(*p).to_owned(),
            Ty::Decl(d, args) => {
                let mut chain = vec![*d];
                let mut cur = self.u.types[*d].outer;
                while let Some(o) = cur {
                    chain.push(o);
                    cur = self.u.types[o].outer;
                }
                chain.reverse();
                let top = &self.u.types[chain[0]];
                let mut s = if top.namespace.is_empty() {
                    String::new()
                } else {
                    format!("{}.", top.namespace)
                };
                for (k, &c) in chain.iter().enumerate() {
                    if k > 0 {
                        s.push('+');
                    }
                    let r = &self.u.types[c];
                    s.push_str(&r.name);
                    if !r.generics.is_empty() {
                        let _ = write!(s, "`{}", r.generics.len());
                    }
                }
                if !args.is_empty() {
                    s.push('[');
                    for (k, a) in args.iter().enumerate() {
                        if k > 0 {
                            s.push(',');
                        }
                        let _ = write!(s, "[{}]", self.clr_name(a));
                    }
                    s.push(']');
                }
                s
            }
            Ty::Array(e, r) => {
                format!("{}[{}]", self.clr_name(e), ",".repeat(usize::from(r.saturating_sub(1))))
            }
            Ty::List(e) => format!("System.Collections.Generic.List`1[[{}]]", self.clr_name(e)),
            Ty::Nullable(e) => format!("System.Nullable`1[[{}]]", self.clr_name(e)),
            Ty::External(n, a) => {
                if a.is_empty() {
                    n.clone()
                } else {
                    let args: Vec<String> =
                        a.iter().map(|x| format!("[{}]", self.clr_name(x))).collect();
                    format!("{n}`{}[{}]", a.len(), args.join(","))
                }
            }
            Ty::GenericParam(n) => format!("!{n}"),
            Ty::Unresolved(s) => format!("?{s}"),
        }
    }

    fn hooks_of(&self, t: &Ty) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for a in self.ancestors(t) {
            let Ty::Decl(d, _) = a else { continue };
            let raw = &self.u.types[d];
            if raw.kind == Kind::Interface {
                continue;
            }
            for m in &raw.methods {
                let hook = HOOK_NAMES.iter().find(|h| **h == m.name).copied().or_else(|| {
                    m.attrs
                        .iter()
                        .find(|a| *a == "OnDeserialized" || *a == "OnDeserializing")
                        .map(String::as_str)
                        .and_then(|a| HOOK_NAMES.iter().find(|h| **h == a).copied())
                });
                if let Some(h) = hook
                    && m.has_body
                {
                    out.entry(h.to_owned()).or_insert_with(|| self.u.full_name(d).to_owned());
                }
            }
        }
        out
    }

    fn fill(&mut self, idx: usize) {
        let ty = self.types[idx].ty.clone();
        let Ty::Decl(d, _) = &ty else { return };
        let d = *d;
        if let Some(b) = self.base_of(&ty)
            && Self::is_closed(&b)
            && matches!(b, Ty::Decl(..))
        {
            let bi = self.intern(&b);
            self.types[idx].base = Some(bi);
        }
        self.types[idx].hooks = self.hooks_of(&ty);
        // Contract kind (`CreateContract`).
        let raw = &self.u.types[d];
        let json_object = raw.has_attr("JsonObject");
        if !json_object && raw.kind != Kind::Interface {
            if let Some((k, v)) = self.dictionary_item(&ty) {
                self.types[idx].flags |= T_DICT_CONTRACT;
                let ctx = self.types[idx].clr.clone();
                let kc = self.codec(&k, &ctx);
                let vc = self.codec(&v, &ctx);
                self.types[idx].members.push(SMember {
                    json: "*".into(),
                    name: "<entries>".into(),
                    codec: SCodec::DictContract { key: Box::new(kc), value: Box::new(vc) },
                    flags: M_WRITABLE | M_READABLE,
                    declared_in: idx,
                });
                return;
            }
            if let Some(e) = self.enumerable_item(&ty) {
                self.types[idx].flags |= T_ARRAY_CONTRACT;
                let ctx = self.types[idx].clr.clone();
                self.problems.custom_collections.push(ctx.clone());
                let ec = self.codec(&e, &ctx);
                self.types[idx].members.push(SMember {
                    json: "*".into(),
                    name: "<items>".into(),
                    codec: SCodec::Seq { kind: SeqKind::Other, elem: Box::new(ec) },
                    flags: M_WRITABLE | M_READABLE,
                    declared_in: idx,
                });
                return;
            }
        }
        let unity = self.types[idx].flags & T_UNITY != 0;
        let members = self.members_of(&ty, unity);
        let mut out: Vec<SMember> = Vec::new();
        // Creator parameters: a JSON property matching one (case-insensitively) is read
        // through the parameter's own contract (`CreateConstructorParameters`), even when
        // the member of that name has another type, no setter, or doesn't exist.
        let params = self.types[idx].creator_params.clone();
        let ptypes = self.types[idx].creator_types.clone();
        let mut used = vec![false; params.len()];
        for mut m in members {
            let ctx = format!("{}.{}", self.types[idx].clr, m.name);
            let mut codec = self.codec(&m.ty, &ctx);
            if let Some(pi) = params.iter().position(|p| p.eq_ignore_ascii_case(&m.json)) {
                m.flags |= M_CTOR_PARAM;
                used[pi] = true;
                let pt = self.close(&self.u.resolve(&ptypes[pi], Some(d)), &ty, true);
                codec = self.codec(&pt, &ctx);
            }
            let declared_in = self.index.get(&m.declared_in).copied().unwrap_or(idx);
            out.push(SMember { json: m.json, name: m.name, codec, flags: m.flags, declared_in });
        }
        for (pi, p) in params.iter().enumerate() {
            if used[pi] {
                continue;
            }
            let ctx = format!("{}.<ctor {p}>", self.types[idx].clr);
            let pt = self.close(&self.u.resolve(&ptypes[pi], Some(d)), &ty, true);
            let codec = self.codec(&pt, &ctx);
            out.push(SMember {
                json: p.clone(),
                name: format!("<ctor {p}>"),
                codec,
                flags: M_CTOR_PARAM,
                declared_in: idx,
            });
        }
        self.types[idx].members = out;
        // Polymorphism: any subclass may appear as `$type`.
        if self.types[idx].flags & T_SEALED == 0 {
            let subs: Vec<Ty> = self
                .descendants
                .get(&ty)
                .map(|v| v.iter().map(|&p| self.pool[p].ty.clone()).collect())
                .unwrap_or_default();
            for s in subs {
                self.intern(&s);
            }
        }
    }

    /// `IDictionary<K, V>` / `IDictionary` implemented by the type or a base.
    fn dictionary_item(&self, t: &Ty) -> Option<(Ty, Ty)> {
        for a in self.ancestors(t).iter().skip(1) {
            match a {
                Ty::External(n, args)
                    if (n.ends_with(".IDictionary")
                        || n.ends_with(".Dictionary")
                        || n.ends_with(".IReadOnlyDictionary"))
                        && args.len() == 2 =>
                {
                    return Some((args[0].clone(), args[1].clone()));
                }
                _ => {}
            }
        }
        let Ty::Decl(d, _) = t else { return None };
        // Unresolved interface names on any ancestor declaration.
        for a in self.ancestors(t) {
            let Ty::Decl(ad, _) = a else { continue };
            for b in &self.u.types[ad].bases {
                let last = match b {
                    TypeExpr::Name { segments, .. } => segments.last().map(|s| s.0.as_str()),
                    _ => None,
                };
                if last == Some("IDictionary") || last == Some("Dictionary") {
                    return Some((
                        Ty::Unresolved(format!("key of {}", self.u.full_name(*d))),
                        Ty::Unresolved(format!("value of {}", self.u.full_name(*d))),
                    ));
                }
            }
        }
        None
    }

    /// The element type of an `IEnumerable<T>` the type implements (or `object`).
    fn enumerable_item(&self, t: &Ty) -> Option<Ty> {
        for a in self.ancestors(t) {
            let Ty::Decl(ad, _) = &a else {
                if let Ty::External(n, args) = &a
                    && n.starts_with("System.Collections.Generic.")
                    && args.len() == 1
                {
                    return Some(args[0].clone());
                }
                continue;
            };
            for b in &self.u.types[*ad].bases {
                let r = self.close(&self.u.resolve(b, Some(*ad)), &a, false);
                match &r {
                    Ty::External(n, args)
                        if args.len() == 1
                            && matches!(
                                n.rsplit('.').next(),
                                Some(
                                    "IEnumerable"
                                        | "ICollection"
                                        | "IList"
                                        | "IReadOnlyList"
                                        | "List"
                                        | "HashSet"
                                )
                            ) =>
                    {
                        return Some(args[0].clone());
                    }
                    Ty::List(e) => return Some((**e).clone()),
                    Ty::Unresolved(s)
                        if matches!(
                            s.as_str(),
                            "IEnumerable"
                                | "ICollection"
                                | "IList"
                                | "System.Collections.IEnumerable"
                        ) =>
                    {
                        return Some(Ty::Prim(Prim::Object));
                    }
                    _ => {}
                }
            }
        }
        None
    }

    fn members_of(&mut self, t: &Ty, unity: bool) -> Vec<RawMember> {
        // Class chain, base first.
        let mut chain: Vec<Ty> = Vec::new();
        let mut cur = Some(t.clone());
        while let Some(c) = cur {
            if chain.len() > 64 {
                break;
            }
            cur = self.base_of(&c);
            if matches!(c, Ty::Decl(..)) {
                chain.push(c);
            }
        }
        chain.reverse();
        let mut out: Vec<RawMember> = Vec::new();
        for c in &chain {
            let Ty::Decl(d, _) = c else { continue };
            let raw = &self.u.types[*d];
            let owner = self.u.full_name(*d).to_owned();
            let mut local: Vec<RawMember> = Vec::new();
            for f in &raw.fields {
                if f.has_mod("const") {
                    continue;
                }
                let jp = json_property(&f.attrs);
                let is_static = f.has_mod("static");
                let take = if unity {
                    f.has_mod("public") && !is_static && !has(&f.attrs, "NonSerialized")
                } else {
                    jp.is_some()
                };
                if !take || has(&f.attrs, "JsonIgnore") {
                    continue;
                }
                if is_static {
                    self.problems.static_members.push(format!("{owner}.{}", f.name));
                }
                let mut flags = M_READABLE;
                // Readonly fields are settable only with a member attribute.
                if !f.has_mod("readonly") || jp.is_some() {
                    flags |= M_WRITABLE;
                }
                if is_static {
                    flags |= M_STATIC;
                }
                let ty = self.close(&self.u.resolve(&f.ty, Some(*d)), c, true);
                let (json, extra) = jp.map_or((f.name.clone(), 0), |a| member_settings(a, &f.name));
                if extra & M_UNMODELLED_SETTINGS != 0 {
                    self.problems.unmodelled_member_settings.push(format!("{owner}.{}", f.name));
                }
                local.push(RawMember {
                    json,
                    name: f.name.clone(),
                    ty,
                    flags: flags | extra,
                    declared_in: c.clone(),
                });
            }
            for p in &raw.props {
                if p.explicit_interface {
                    continue;
                }
                let jp = json_property(&p.attrs);
                let is_static = p.has_mod("static");
                let take = if unity {
                    p.has_mod("public") && p.getter && !is_static
                } else {
                    jp.is_some()
                };
                if !take || has(&p.attrs, "JsonIgnore") {
                    continue;
                }
                if is_static {
                    self.problems.static_members.push(format!("{owner}.{}", p.name));
                }
                let mut flags = M_PROPERTY;
                if p.getter {
                    flags |= M_READABLE;
                }
                if p.setter {
                    flags |= M_WRITABLE;
                }
                if is_static {
                    flags |= M_STATIC;
                }
                let ty = self.close(&self.u.resolve(&p.ty, Some(*d)), c, true);
                let (json, extra) = jp.map_or((p.name.clone(), 0), |a| member_settings(a, &p.name));
                if extra & M_UNMODELLED_SETTINGS != 0 {
                    self.problems.unmodelled_member_settings.push(format!("{owner}.{}", p.name));
                }
                local.push(RawMember {
                    json,
                    name: p.name.clone(),
                    ty,
                    flags: flags | extra,
                    declared_in: c.clone(),
                });
            }
            // Same JSON name twice in one declaration: Newtonsoft throws.
            let mut names = HashSet::new();
            for m in &local {
                if !names.insert(m.json.clone()) {
                    self.problems.duplicate_json_names.push(format!("{owner}.{}", m.json));
                }
            }
            for m in local {
                // A derived declaration replaces a base one with the same JSON name.
                if let Some(pos) = out.iter().position(|x| x.json == m.json) {
                    out.remove(pos);
                }
                out.push(m);
            }
        }
        out
    }

    fn intern_enum(&mut self, d: usize) -> usize {
        let name = self.u.full_name(d).to_owned();
        if let Some(&i) = self.enum_index.get(&name) {
            return i;
        }
        let i = self.enums.len();
        self.enums.push(rules::enum_def(self.u, d));
        self.enum_asm.push(self.u.types[d].assembly.to_string());
        self.enum_clr.push(self.clr_name(&Ty::Decl(d, Vec::new())));
        self.enum_index.insert(name, i);
        i
    }

    fn intern_system_enum(&mut self, name: &str) -> usize {
        if let Some(&i) = self.enum_index.get(name) {
            return i;
        }
        let members = rules::SYSTEM_ENUMS
            .iter()
            .find(|(e, _)| *e == name)
            .map(|(_, m)| m.iter().zip(0..).map(|(n, v)| ((*n).to_owned(), v)).collect())
            .unwrap_or_default();
        let i = self.enums.len();
        self.enum_asm.push("mscorlib".to_owned());
        self.enum_clr.push(name.to_owned());
        self.enums.push(SEnum {
            name: name.to_owned(),
            bits: 32,
            signed: true,
            flags: false,
            members,
            unresolved: Vec::new(),
        });
        self.enum_index.insert(name.to_owned(), i);
        i
    }

    fn unsupported(&mut self, t: &Ty, ctx: &str) -> SCodec {
        let why = self.u.display(t);
        self.problems.unsupported.entry(why.clone()).or_default().push(ctx.to_owned());
        SCodec::Unsupported(why)
    }

    fn is_or_sub(&self, t: &Ty, full: &str, strict: bool) -> bool {
        let anc = self.ancestors(t);
        anc.iter().enumerate().any(|(k, a)| {
            (!strict || k > 0) && matches!(a, Ty::Decl(i, _) if self.u.full_name(*i) == full)
        })
    }

    /// Declared type → codec: the converter `GetMatchingConverter` would pick (when it can
    /// read), else the contract.
    #[allow(clippy::too_many_lines)]
    fn codec(&mut self, t: &Ty, ctx: &str) -> SCodec {
        match t {
            Ty::Prim(p) => match p {
                Prim::Bool => SCodec::Bool,
                Prim::F32 => SCodec::F32,
                Prim::F64 => SCodec::F64,
                Prim::Char => SCodec::Char,
                Prim::String => SCodec::Str,
                Prim::Decimal => SCodec::Decimal,
                Prim::Object => SCodec::Any,
                p => match rules::prim_int(*p) {
                    Some((bits, signed)) => SCodec::Int { bits, signed },
                    None => self.unsupported(t, ctx),
                },
            },
            Ty::Array(e, rank) => {
                if *rank > 1 {
                    return self.unsupported(t, ctx);
                }
                if matches!(**e, Ty::Prim(Prim::U8)) {
                    return SCodec::Bytes;
                }
                SCodec::Seq { kind: SeqKind::Array, elem: Box::new(self.codec(e, ctx)) }
            }
            Ty::List(e) => SCodec::Seq { kind: SeqKind::List, elem: Box::new(self.codec(e, ctx)) },
            Ty::Nullable(e) => {
                // `StringEnumConverter` handles `Nullable<enum>` itself; other converters
                // match only the exact type, so `Nullable<Vector3>` uses Vector3's
                // contract.
                if let Ty::Decl(d, _) = &**e
                    && self.u.types[*d].kind == Kind::Enum
                {
                    return SCodec::Nullable(Box::new(SCodec::Enum(self.intern_enum(*d))));
                }
                let inner = match &**e {
                    Ty::Decl(d, _) if UnityKind::from_full(self.u.full_name(*d)).is_some() => {
                        SCodec::Object(self.intern(e))
                    }
                    _ => self.codec(e, ctx),
                };
                SCodec::Nullable(Box::new(inner))
            }
            Ty::External(n, args) => {
                if rules::SYSTEM_ENUMS.iter().any(|(e, _)| e == n) {
                    return SCodec::Enum(self.intern_system_enum(n));
                }
                let short = n.rsplit('.').next().unwrap_or(n);
                match (short, args.len()) {
                    ("Guid", 0) => SCodec::Guid,
                    ("TimeSpan", 0) => SCodec::TimeSpan,
                    ("DateTime", 0) => SCodec::DateTime,
                    ("DateTimeOffset", 0) => SCodec::DateTimeOffset,
                    ("Version", 0) => SCodec::Version,
                    ("JToken" | "JObject" | "JArray", 0) => SCodec::JToken,
                    ("Dictionary" | "IDictionary", 2) => SCodec::Dict {
                        key: Box::new(self.codec(&args[0], ctx)),
                        value: Box::new(self.codec(&args[1], ctx)),
                    },
                    ("SortedDictionary" | "SortedList" | "IReadOnlyDictionary", 2) => {
                        SCodec::DictContract {
                            key: Box::new(self.codec(&args[0], ctx)),
                            value: Box::new(self.codec(&args[1], ctx)),
                        }
                    }
                    ("KeyValuePair", 2) => SCodec::KeyValue {
                        key: Box::new(self.codec(&args[0], ctx)),
                        value: Box::new(self.codec(&args[1], ctx)),
                    },
                    ("HashSet" | "ISet" | "SortedSet", 1) => SCodec::Seq {
                        kind: SeqKind::HashSet,
                        elem: Box::new(self.codec(&args[0], ctx)),
                    },
                    ("Stack", 1) => SCodec::Seq {
                        kind: SeqKind::Stack,
                        elem: Box::new(self.codec(&args[0], ctx)),
                    },
                    ("Queue", 1) => SCodec::Seq {
                        kind: SeqKind::Queue,
                        elem: Box::new(self.codec(&args[0], ctx)),
                    },
                    (
                        "IList"
                        | "ICollection"
                        | "IEnumerable"
                        | "IReadOnlyList"
                        | "IReadOnlyCollection"
                        | "LinkedList",
                        1,
                    ) => SCodec::Seq {
                        kind: SeqKind::Other,
                        elem: Box::new(self.codec(&args[0], ctx)),
                    },
                    _ => self.unsupported(t, ctx),
                }
            }
            Ty::GenericParam(_) => self.unsupported(t, ctx),
            Ty::Unresolved(s) => {
                let last = s.rsplit('.').next().unwrap_or(s);
                match last {
                    "Version" => SCodec::Version,
                    "JToken" | "JObject" | "JArray" => SCodec::JToken,
                    // UniRx (not decompiled): no `[JsonProperty]` members, so the opt-in
                    // contract is empty and only `$id` is written.
                    _ if last.starts_with("ReactiveProperty<") => {
                        SCodec::Object(self.intern_opaque_class(s, "UniRx"))
                    }
                    _ => {
                        self.problems.unresolved.entry(s.clone()).or_default().push(ctx.to_owned());
                        SCodec::Unsupported(format!("?{s}"))
                    }
                }
            }
            Ty::Decl(d, args) => {
                let d = *d;
                if !Self::is_closed(t) {
                    return self.unsupported(t, ctx);
                }
                let full = self.u.full_name(d).to_owned();
                let raw_kind = self.u.types[d].kind;
                if raw_kind == Kind::Enum {
                    return SCodec::Enum(self.intern_enum(d));
                }
                // Converters, in `CommonConverters` order.
                if self.is_or_sub(t, SIMPLE_BLUEPRINT, true)
                    || full == "Kingmaker.Blueprints.Classes.Selection.IFeatureSelection"
                {
                    return SCodec::Blueprint(self.intern(t));
                }
                if full == "Kingmaker.Blueprints.BlueprintGuid" {
                    return SCodec::BlueprintGuid;
                }
                if self.is_or_sub(t, BP_REF_BASE, true) {
                    let target = self
                        .ancestors(t)
                        .into_iter()
                        .find_map(|a| match a {
                            Ty::Decl(i, mut a)
                                if self.u.full_name(i) == BP_REF_GENERIC && a.len() == 1 =>
                            {
                                Some(a.remove(0))
                            }
                            _ => None,
                        })
                        .filter(|a| matches!(a, Ty::Decl(..)))
                        .or_else(|| self.u.find(SIMPLE_BLUEPRINT).map(|i| Ty::Decl(i, Vec::new())));
                    return match target {
                        Some(tt) => SCodec::BpRef(self.intern(&tt)),
                        None => self.unsupported(t, ctx),
                    };
                }
                match full.as_str() {
                    "Kingmaker.Visual.CharacterSystem.BoneTransformData"
                    | "Kingmaker.EntitySystem.Persistence.JsonUtility.BoneTransformData" => {
                        return SCodec::BoneTransform;
                    }
                    "UnityEngine.Bounds" => return SCodec::Bounds,
                    "Kingmaker.Utility.CountableFlag" | "Kingmaker.EntitySystem.CountableFlag" => {
                        return SCodec::CountableFlag;
                    }
                    "Kingmaker.EntitySystem.Entities.UnitReference" => {
                        return SCodec::EntityRef(RefKind::Unit);
                    }
                    "Kingmaker.EntitySystem.Entities.MapObjReference"
                    | "Kingmaker.View.MapObjects.MapObjReference" => {
                        return SCodec::EntityRef(RefKind::MapObject);
                    }
                    "Kingmaker.EntitySystem.EntityRef" if args.is_empty() => {
                        return SCodec::EntityRef(RefKind::Entity);
                    }
                    "Kingmaker.EntitySystem.EntityRef" => return SCodec::EntityRef(RefKind::Typed),
                    "UnityEngine.Sprite" => return SCodec::Sprite,
                    "UnityEngine.Vector2" => return SCodec::Vector2,
                    "UnityEngine.Vector3" => return SCodec::Vector3,
                    "UnityEngine.Quaternion" => return SCodec::Quaternion,
                    "Kingmaker.Localization.LocalizationPack.StringEntry" => {
                        return SCodec::StringEntry;
                    }
                    _ => {}
                }
                if self.is_or_sub(t, rules::UNITY_OBJECT, false) {
                    return SCodec::UnityObject(self.clr_name(t));
                }
                if self.is_named_type(d, "BoneTransformData") {
                    return SCodec::BoneTransform;
                }
                if self.is_named_type(d, "CountableFlag") {
                    return SCodec::CountableFlag;
                }
                SCodec::Object(self.intern(t))
            }
        }
    }

    fn is_named_type(&self, d: usize, short: &str) -> bool {
        let r = &self.u.types[d];
        r.name == short && r.generics.is_empty() && &*r.assembly == "Assembly-CSharp"
    }
}

struct RawMember {
    json: String,
    name: String,
    ty: Ty,
    flags: u16,
    declared_in: Ty,
}

fn has(attrs: &[Attr], name: &str) -> bool {
    attrs.iter().any(|a| a.name == name)
}

fn json_property(attrs: &[Attr]) -> Option<&Attr> {
    attrs.iter().find(|a| a.name == "JsonProperty" || a.name == "JsonRequired")
}

/// JSON name and flags from `[JsonProperty(…)]`.
fn member_settings(a: &Attr, name: &str) -> (String, u16) {
    let json = a
        .string_arg("PropertyName")
        .or_else(|| a.first_positional_string())
        .unwrap_or(name)
        .to_owned();
    let mut flags = 0;
    for (n, e) in &a.args {
        let text = match e {
            crate::parse::Expr::Ident(p) => p.join("."),
            crate::parse::Expr::Bool(b) => b.to_string(),
            _ => String::new(),
        };
        match n.as_deref() {
            Some("DefaultValueHandling") if text.contains("Ignore") => flags |= M_DEFAULT_IGNORE,
            Some("NullValueHandling") if text.contains("Ignore") => flags |= M_NULL_IGNORE,
            Some("ObjectCreationHandling") if text.contains("Replace") => flags |= M_REPLACE,
            Some(
                "TypeNameHandling"
                | "ItemTypeNameHandling"
                | "ItemConverterType"
                | "ItemIsReference"
                | "Required",
            ) => flags |= M_UNMODELLED_SETTINGS,
            _ => {}
        }
    }
    (json, flags)
}

fn prim_clr(p: Prim) -> &'static str {
    match p {
        Prim::Bool => "System.Boolean",
        Prim::I8 => "System.SByte",
        Prim::U8 => "System.Byte",
        Prim::I16 => "System.Int16",
        Prim::U16 => "System.UInt16",
        Prim::I32 => "System.Int32",
        Prim::U32 => "System.UInt32",
        Prim::I64 => "System.Int64",
        Prim::U64 => "System.UInt64",
        Prim::F32 => "System.Single",
        Prim::F64 => "System.Double",
        Prim::Char => "System.Char",
        Prim::String => "System.String",
        Prim::Decimal => "System.Decimal",
        Prim::Object => "System.Object",
        Prim::IntPtr => "System.IntPtr",
    }
}

// ---- emission -----------------------------------------------------------------------

fn codec_src(c: &SCodec) -> String {
    match c {
        SCodec::Bool => "C::Bool".into(),
        SCodec::Int { bits, signed } => format!("C::Int {{ bits: {bits}, signed: {signed} }}"),
        SCodec::F32 => "C::F32".into(),
        SCodec::F64 => "C::F64".into(),
        SCodec::Decimal => "C::Decimal".into(),
        SCodec::Char => "C::Char".into(),
        SCodec::Str => "C::Str".into(),
        SCodec::Guid => "C::Guid".into(),
        SCodec::TimeSpan => "C::TimeSpan".into(),
        SCodec::DateTime => "C::DateTime".into(),
        SCodec::DateTimeOffset => "C::DateTimeOffset".into(),
        SCodec::Version => "C::Version".into(),
        SCodec::Bytes => "C::Bytes".into(),
        SCodec::Enum(i) => format!("C::Enum({i})"),
        SCodec::Nullable(e) => format!("C::Nullable(&{})", codec_src(e)),
        SCodec::Object(i) => format!("C::Object({i})"),
        SCodec::Blueprint(i) => format!("C::Blueprint({i})"),
        SCodec::BpRef(i) => format!("C::BpRef({i})"),
        SCodec::BlueprintGuid => "C::BlueprintGuid".into(),
        SCodec::EntityRef(k) => format!("C::EntityRef(R::{k:?})"),
        SCodec::Vector2 => "C::Vector2".into(),
        SCodec::Vector3 => "C::Vector3".into(),
        SCodec::Quaternion => "C::Quaternion".into(),
        SCodec::BoneTransform => "C::BoneTransform".into(),
        SCodec::Bounds => "C::Bounds".into(),
        SCodec::CountableFlag => "C::CountableFlag".into(),
        SCodec::Sprite => "C::Sprite".into(),
        SCodec::StringEntry => "C::StringEntry".into(),
        SCodec::Dict { key, value } => {
            format!("C::Dict {{ key: &{}, value: &{} }}", codec_src(key), codec_src(value))
        }
        SCodec::DictContract { key, value } => {
            format!("C::DictContract {{ key: &{}, value: &{} }}", codec_src(key), codec_src(value))
        }
        SCodec::KeyValue { key, value } => {
            format!("C::KeyValue {{ key: &{}, value: &{} }}", codec_src(key), codec_src(value))
        }
        SCodec::Seq { kind, elem } => {
            format!("C::Seq {{ kind: S::{kind:?}, elem: &{} }}", codec_src(elem))
        }
        SCodec::UnityObject(n) => format!("C::UnityObject({n:?})"),
        SCodec::Any => "C::Any".into(),
        SCodec::JToken => "C::JToken".into(),
        SCodec::Unsupported(w) => format!("C::Unsupported({w:?})"),
    }
}

/// The generated module for `crates/wr-save`. Deterministic.
#[must_use]
pub fn emit(b: &SaveBuilder<'_>) -> String {
    let mut s = String::new();
    s.push_str(
        "// @generated by tools/wr-codegen (`gen-save`) from the game's decompiled declarations.\n\
         // Save-side types, members, enums and CLR names only: no game data. Don't edit by\n\
         // hand; rerun `cargo run -p wr-codegen -- gen-save`.\n\n\
         use crate::schema::{Codec as C, EnumDef, Member, RefKind as R, SeqKind as S, TypeDef};\n\n",
    );
    let _ = writeln!(s, "pub static TYPES: [TypeDef; {}] = [", b.types.len());
    for (i, t) in b.types.iter().enumerate() {
        let base = t.base.map_or_else(|| "u32::MAX".to_owned(), |x| x.to_string());
        let hooks: u32 = HOOK_NAMES
            .iter()
            .enumerate()
            .filter(|(_, h)| t.hooks.contains_key(**h))
            .map(|(k, _)| 1u32 << k)
            .sum();
        let _ = writeln!(s, "    // {i}");
        let _ = writeln!(
            s,
            "    TypeDef {{ clr: {:?}, assembly: {:?}, flags: {:#x}, base: {base}, hooks: {hooks:#x}, ifaces: &{:?}, members: &[",
            t.clr, t.assembly, t.flags, t.ifaces
        );
        for m in &t.members {
            let _ = writeln!(
                s,
                "        Member {{ json: {:?}, name: {:?}, codec: {}, flags: {:#x}, declared_in: {} }},",
                m.json,
                m.name,
                codec_src(&m.codec),
                m.flags,
                m.declared_in
            );
        }
        s.push_str("    ] },\n");
    }
    s.push_str("];\n\n");
    let _ = writeln!(s, "pub static ENUMS: [EnumDef; {}] = [", b.enums.len());
    for (k, e) in b.enums.iter().enumerate() {
        let members: Vec<String> = e.members.iter().map(|(n, v)| format!("({n:?}, {v})")).collect();
        let _ = writeln!(
            s,
            "    EnumDef {{ name: {:?}, clr: {:?}, assembly: {:?}, bits: {}, signed: {}, flags: {}, members: &[{}] }},",
            e.name,
            b.enum_clr[k],
            b.enum_asm[k],
            e.bits,
            e.signed,
            e.flags,
            members.join(", ")
        );
    }
    s.push_str("];\n\n");
    let mut names: Vec<(&str, usize)> =
        b.types.iter().enumerate().map(|(i, t)| (t.clr.as_str(), i)).collect();
    names.sort_unstable();
    names.dedup_by_key(|x| x.0);
    s.push_str("/// CLR name (no assembly qualification) → type, sorted.\n");
    let _ = writeln!(s, "pub static BY_NAME: [(&str, u32); {}] = [", names.len());
    for (n, i) in names {
        let _ = writeln!(s, "    ({n:?}, {i}),");
    }
    s.push_str("];\n");
    let _ = writeln!(
        s,
        "\n/// Hook names, by bit in `TypeDef::hooks`.\npub static HOOKS: [&str; {}] = {:?};",
        HOOK_NAMES.len(),
        HOOK_NAMES
    );
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in s.bytes() {
        h = (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
    }
    let _ = writeln!(
        s,
        "\n/// Hash of this file's tables; our save format records it.\npub const FINGERPRINT: u64 = {h:#018x};"
    );
    s
}

#[derive(Serialize)]
pub struct Summary<'a> {
    pub types: usize,
    pub enums: usize,
    pub pool: usize,
    pub by_flag: BTreeMap<&'static str, usize>,
    pub problems: &'a Problems,
    /// Types with lifecycle hooks: type → hook → declaring type.
    pub hooks: BTreeMap<&'a str, &'a BTreeMap<String, String>>,
}

#[must_use]
pub fn summary<'a>(b: &'a SaveBuilder<'_>) -> Summary<'a> {
    let mut by_flag = BTreeMap::new();
    for (name, f) in [
        ("abstract", T_ABSTRACT),
        ("unity", T_UNITY),
        ("creator_params", T_CREATOR_PARAMS),
        ("no_creator", T_NO_CREATOR),
        ("entity", T_ENTITY),
        ("fact", T_FACT),
        ("part", T_PART),
        ("component", T_COMPONENT),
        ("array_contract", T_ARRAY_CONTRACT),
        ("dict_contract", T_DICT_CONTRACT),
    ] {
        by_flag.insert(name, b.types.iter().filter(|t| t.flags & f != 0).count());
    }
    Summary {
        types: b.types.len(),
        enums: b.enums.len(),
        pool: b.pool.len(),
        by_flag,
        problems: &b.problems,
        hooks: b
            .types
            .iter()
            .filter(|t| !t.hooks.is_empty())
            .map(|t| (t.clr.as_str(), &t.hooks))
            .collect(),
    }
}
