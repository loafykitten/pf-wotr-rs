//! Schema-driven decoding of `.jbp` JSON, streamed through serde.
//!
//! [`NodeSeed`] is a `DeserializeSeed` parameterised by a field's [`Codec`]: it reads the
//! token stream once and builds a [`Node`], following the game's Newtonsoft setup:
//!
//! - `$type` is honoured only as an object's first property (Newtonsoft's default
//!   metadata handling); the GUID before the comma selects the type (`GuidClassBinder`),
//!   and the tagged type must be assignable to the declared one.
//! - Properties match fields by JSON name, exactly first and then ignoring case
//!   (`GetClosestMatchProperty`); unknown properties are kept in [`Object::extra`].
//! - Converters: `!bp_` references, `{guid, fileid}` assets, `{AssetId}` weak links,
//!   `{stringkey}` shared strings, enums as names, numbers or comma lists, `x|y|z`
//!   vectors, Color32 as a packed int, curves and gradients, prototype links read as null.
//!
//! A value that doesn't fit becomes [`Node::Opaque`] with a [`Reason`]; decoding goes on.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

use serde::Deserialize;
use serde::de::value::{MapAccessDeserializer, SeqAccessDeserializer};
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::Value;

use crate::guid::Guid;
use crate::node::{
    AssetRef, Curve, Gradient, Keyframe, Node, Object, Opaque, Reason, SharedRef, json_kind,
};
use crate::schema::{self, Codec, Def, TypeIdx, UnityKind, type_flags};

/// Counters for one type's objects.
#[derive(Debug, Clone, Default)]
pub struct TypeTally {
    pub objects: u64,
    /// Objects created from a `$type` tag naming this type.
    pub tagged: u64,
    /// Objects whose whole subtree decoded without an opaque value.
    pub clean: u64,
    pub fields_typed: u64,
    pub fields_opaque: u64,
    pub fields_absent: u64,
    pub unknown_fields: u64,
}

/// Decode statistics, mergeable across threads.
#[derive(Debug, Default)]
pub struct Tally {
    pub per_type: Vec<TypeTally>,
    pub opaque: HashMap<Reason, u64>,
    pub opaque_nodes: u64,
    /// `(type, JSON name)` → count of properties the schema doesn't have.
    pub unknown_fields: HashMap<(TypeIdx, Box<str>), u64>,
    /// `(type, field slot)` → count of fields missing from the JSON.
    pub absent: HashMap<(TypeIdx, u32), u64>,
    /// Absent fields whose C# default isn't a literal we carry.
    pub absent_unknown_default: u64,
    /// Things decoded the way the game would, but worth knowing about.
    pub notes: HashMap<&'static str, u64>,
    /// `$type` GUIDs with no registered type.
    pub unknown_tags: HashMap<Box<str>, u64>,
}

impl Tally {
    fn ty(&mut self, t: TypeIdx) -> &mut TypeTally {
        let n = schema::types().len();
        if self.per_type.len() < n {
            self.per_type.resize(n, TypeTally::default());
        }
        &mut self.per_type[t as usize]
    }

    fn note(&mut self, what: &'static str) {
        *self.notes.entry(what).or_default() += 1;
    }

    pub fn merge(&mut self, o: Self) {
        if self.per_type.len() < o.per_type.len() {
            self.per_type.resize(o.per_type.len(), TypeTally::default());
        }
        for (a, b) in self.per_type.iter_mut().zip(o.per_type) {
            a.objects += b.objects;
            a.tagged += b.tagged;
            a.clean += b.clean;
            a.fields_typed += b.fields_typed;
            a.fields_opaque += b.fields_opaque;
            a.fields_absent += b.fields_absent;
            a.unknown_fields += b.unknown_fields;
        }
        for (k, v) in o.opaque {
            *self.opaque.entry(k).or_default() += v;
        }
        self.opaque_nodes += o.opaque_nodes;
        for (k, v) in o.unknown_fields {
            *self.unknown_fields.entry(k).or_default() += v;
        }
        for (k, v) in o.absent {
            *self.absent.entry(k).or_default() += v;
        }
        self.absent_unknown_default += o.absent_unknown_default;
        for (k, v) in o.notes {
            *self.notes.entry(k).or_default() += v;
        }
        for (k, v) in o.unknown_tags {
            *self.unknown_tags.entry(k).or_default() += v;
        }
    }
}

/// A typed blueprint reference found while decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefUse {
    pub guid: Guid,
    /// `T` of the field's `BlueprintReference<T>`.
    pub target: TypeIdx,
}

/// Per-decode context.
#[derive(Debug)]
pub struct Cx<'t> {
    tally: RefCell<&'t mut Tally>,
    refs: RefCell<Vec<RefUse>>,
}

impl<'t> Cx<'t> {
    pub fn new(tally: &'t mut Tally) -> Self {
        Self { tally: RefCell::new(tally), refs: RefCell::new(Vec::new()) }
    }

    #[must_use]
    pub fn take_refs(&self) -> Vec<RefUse> {
        std::mem::take(&mut self.refs.borrow_mut())
    }

    fn opaque(&self, value: Value, reason: Reason) -> Node {
        let mut t = self.tally.borrow_mut();
        t.opaque_nodes += 1;
        if let Reason::UnknownTypeId(tag) = &reason {
            *t.unknown_tags.entry(tag.clone()).or_default() += 1;
        }
        *t.opaque.entry(reason.clone()).or_default() += 1;
        Node::Opaque(Box::new(Opaque { value, reason }))
    }

    fn note(&self, what: &'static str) {
        self.tally.borrow_mut().note(what);
    }

    fn opaque_count(&self) -> u64 {
        self.tally.borrow().opaque_nodes
    }
}

/// Decodes one value with a codec.
#[derive(Debug, Clone, Copy)]
pub struct NodeSeed<'c, 't> {
    pub cx: &'c Cx<'t>,
    pub codec: Codec,
}

impl<'de> DeserializeSeed<'de> for NodeSeed<'_, '_> {
    type Value = Node;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Node, D::Error> {
        d.deserialize_any(self)
    }
}

fn codec_name(c: &Codec) -> &'static str {
    match c {
        Codec::Bool => "bool",
        Codec::Int { .. } => "int",
        Codec::F32 => "float",
        Codec::F64 => "double",
        Codec::Char => "char",
        Codec::Str => "string",
        Codec::Decimal => "decimal",
        Codec::Enum(_) => "enum",
        Codec::Object(_) => "object",
        Codec::BpRef { .. } => "blueprint reference",
        Codec::WeakLink(_) => "weak link",
        Codec::SharedString => "shared string",
        Codec::Asset(_) => "asset",
        Codec::Unity(k) => match k {
            UnityKind::Vector2 => "Vector2",
            UnityKind::Vector3 => "Vector3",
            UnityKind::Vector4 => "Vector4",
            UnityKind::Vector2Int => "Vector2Int",
            UnityKind::Vector3Int => "Vector3Int",
            UnityKind::Quaternion => "Quaternion",
            UnityKind::Color => "Color",
            UnityKind::Color32 => "Color32",
            UnityKind::Rect => "Rect",
            UnityKind::Bounds => "Bounds",
            UnityKind::AnimationCurve => "AnimationCurve",
            UnityKind::Gradient => "Gradient",
        },
        Codec::Overrides => "overrides",
        Codec::PrototypeLink => "prototype link",
        Codec::BlueprintGuid => "BlueprintGuid",
        Codec::Seq { .. } => "array",
        Codec::Unsupported(_) => "unsupported",
    }
}

impl NodeSeed<'_, '_> {
    fn mismatch(&self, v: Value) -> Node {
        if let Codec::Unsupported(why) = self.codec {
            return self.cx.opaque(v, Reason::Unsupported(why));
        }
        let json = json_kind(&v);
        self.cx.opaque(v, Reason::Mismatch { codec: codec_name(&self.codec), json })
    }

    fn int(&self, v: i128, raw: Value) -> Node {
        match self.codec {
            Codec::Int { bits, signed } => {
                let (lo, hi): (i128, i128) = if signed {
                    (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
                } else {
                    (0, (1i128 << bits) - 1)
                };
                if v < lo || v > hi {
                    return self.cx.opaque(raw, Reason::ConverterError("integer out of range"));
                }
                match i64::try_from(v) {
                    Ok(x) => Node::Int(x),
                    Err(_) => Node::UInt(u64::try_from(v).unwrap_or(u64::MAX)),
                }
            }
            #[allow(clippy::cast_precision_loss)]
            Codec::F32 => Node::F32(v as f32),
            #[allow(clippy::cast_precision_loss)]
            Codec::F64 => Node::F64(v as f64),
            // `Enum.ToObject(type, long)`: an unchecked conversion to the underlying type.
            Codec::Enum(e) => {
                let w = wrap_to(v, schema::enum_def(e).bits, schema::enum_def(e).signed);
                if i128::from(w) != v {
                    self.cx.note("enum number wrapped to the enum's width, as Enum.ToObject does");
                    return Node::Coerced(Box::new(Node::Enum(w)), Box::new(raw));
                }
                Node::Enum(w)
            }
            // `Convert.ToInt32` then unpacked into four bytes.
            Codec::Unity(UnityKind::Color32) => match i32::try_from(v) {
                Ok(x) => Node::Int(i64::from(x)),
                Err(_) => self.cx.opaque(raw, Reason::ConverterError("Color32 out of range")),
            },
            Codec::PrototypeLink | Codec::Overrides => Node::Discarded(Box::new(raw)),
            // `ReadAsString` turns a primitive token into its invariant text.
            Codec::Str => {
                self.cx.note("number read into a string field (Newtonsoft converts it)");
                Node::Str(v.to_string().into())
            }
            _ => self.mismatch(raw),
        }
    }

    fn string(&self, s: &str) -> Node {
        match self.codec {
            Codec::Str => Node::Str(s.into()),
            // `ReadAsInt32`/`ReadAsInt64` parse a numeric string.
            Codec::Int { bits, signed } => match s.trim().parse::<i128>() {
                Ok(v) if fits(v, bits, signed) => {
                    self.cx.note("numeric string read into an integer field");
                    Node::Coerced(
                        Box::new(self.int(v, Value::Null)),
                        Box::new(Value::String(s.into())),
                    )
                }
                _ => self.mismatch(Value::String(s.into())),
            },
            // `Convert.ToInt32(string)`.
            Codec::Unity(UnityKind::Color32) => match s.trim().parse::<i32>() {
                Ok(v) => {
                    self.cx.note("Color32 given as a numeric string");
                    Node::Coerced(
                        Box::new(Node::Int(i64::from(v))),
                        Box::new(Value::String(s.into())),
                    )
                }
                Err(_) => self.mismatch(Value::String(s.into())),
            },
            Codec::Char => {
                let mut it = s.chars();
                match (it.next(), it.next()) {
                    (Some(c), None) => Node::Int(i64::from(u32::from(c))),
                    _ => self.mismatch(Value::String(s.into())),
                }
            }
            Codec::Enum(e) => {
                let def = schema::enum_def(e);
                if let Some(v) = def.parse(s)
                    && fits(i128::from(v), def.bits, def.signed)
                {
                    Node::Enum(v)
                } else {
                    // `StringEnumConverter` logs and returns `default(T)`.
                    self.cx.note("enum name not found: read as 0, as the game does");
                    coerced(Node::Enum(0), s)
                }
            }
            Codec::F32 | Codec::F64 => {
                let v = match s {
                    "NaN" => Some(f64::NAN),
                    "Infinity" => Some(f64::INFINITY),
                    "-Infinity" => Some(f64::NEG_INFINITY),
                    // `ReadAsDouble` parses other strings with the invariant culture.
                    t => t.trim().parse::<f64>().ok().inspect(|_| {
                        self.cx.note("numeric string read into a float field");
                    }),
                };
                #[allow(clippy::cast_possible_truncation)]
                match (v, self.codec) {
                    (Some(v), Codec::F32) => Node::F32(v as f32),
                    (Some(v), _) => Node::F64(v),
                    (None, _) => self.mismatch(Value::String(s.into())),
                }
            }
            Codec::BpRef { target, .. } => {
                if let Some(g) = s.strip_prefix("!bp_") {
                    // `BlueprintGuid.Parse` logs a bad GUID and keeps it empty.
                    let Some(guid) = Guid::parse(g) else {
                        if g.is_empty() {
                            return Node::BpRef(Guid::EMPTY);
                        }
                        self.cx.note("unparseable !bp_ GUID: read as empty");
                        return coerced(Node::BpRef(Guid::EMPTY), s);
                    };
                    if !guid.is_empty() {
                        self.cx.refs.borrow_mut().push(RefUse { guid, target });
                    }
                    Node::BpRef(guid)
                } else if s.is_empty() {
                    Node::BpRef(Guid::EMPTY)
                } else {
                    // The converter ignores text without the prefix.
                    self.cx.note("reference without !bp_ prefix: read as empty");
                    coerced(Node::BpRef(Guid::EMPTY), s)
                }
            }
            Codec::BlueprintGuid => Node::BpRef(Guid::parse(s).unwrap_or(Guid::EMPTY)),
            Codec::Unity(k @ (UnityKind::Vector2 | UnityKind::Vector3 | UnityKind::Quaternion)) => {
                // Save-format shorthand: integers / 100, separated by `|`.
                let parts: Vec<Option<i32>> = s.split('|').map(|p| p.parse().ok()).collect();
                let want = if k == UnityKind::Vector2 { 2 } else { 3 };
                if parts.len() != want || parts.iter().any(Option::is_none) {
                    return self
                        .cx
                        .opaque(Value::String(s.into()), Reason::ConverterError("bad x|y|z"));
                }
                #[allow(clippy::cast_precision_loss)]
                let f: Vec<f32> =
                    parts.iter().take(want).map(|p| p.unwrap_or(0) as f32 / 100.0).collect();
                if k == UnityKind::Quaternion {
                    // `Quaternion.Euler` of the angles; we keep the angles and note it.
                    self.cx.note("quaternion from euler shorthand (kept as angles)");
                }
                Node::Floats(f.into())
            }
            Codec::PrototypeLink | Codec::Overrides => {
                Node::Discarded(Box::new(Value::String(s.into())))
            }
            _ => self.mismatch(Value::String(s.into())),
        }
    }

    fn null(&self) -> Node {
        match self.codec {
            Codec::Str => Node::Null,
            Codec::Object(t) => {
                if schema::ty(t).has(type_flags::STRUCT) {
                    self.cx.opaque(Value::Null, Reason::ConverterError("null for a struct"))
                } else {
                    Node::Null
                }
            }
            Codec::Seq { .. } => Node::Null,
            Codec::BpRef { .. } => Node::BpRef(Guid::EMPTY),
            Codec::WeakLink(_) => Node::WeakLink(None),
            Codec::SharedString => Node::SharedString(None),
            Codec::Asset(_) => Node::Asset(None),
            // `StringEnumConverter`: `Activator.CreateInstance(enumType)`.
            Codec::Enum(_) => Node::Enum(0),
            // `Convert.ToInt32(null)` is 0.
            Codec::Unity(UnityKind::Color32) => {
                self.cx.note("null Color32 read as 0");
                Node::Coerced(Box::new(Node::Int(0)), Box::new(Value::Null))
            }
            // `BoundsConverter`: anything but an object gives `default(Bounds)`.
            Codec::Unity(UnityKind::Bounds) => Node::Floats(vec![0.0; 6].into()),
            Codec::PrototypeLink | Codec::Overrides => Node::Discarded(Box::new(Value::Null)),
            _ => self.mismatch(Value::Null),
        }
    }

    /// Converters that read a whole object (small; read into a `Value` first).
    fn value_map(&self, v: Value) -> Node {
        let Value::Object(m) = &v else { return self.mismatch(v) };
        // Keys each converter reads; anything else in the object is ignored by the game.
        let read: &[&str] = match self.codec {
            Codec::WeakLink(_) if m.contains_key("AssetId") => &["AssetId"],
            Codec::WeakLink(_) => &["guid", "fileid"],
            Codec::SharedString => &["stringkey", "assetguid"],
            Codec::Asset(_) => &["guid", "fileid"],
            Codec::Unity(UnityKind::Vector2 | UnityKind::Vector2Int) => &["x", "y"],
            Codec::Unity(UnityKind::Vector3 | UnityKind::Vector3Int) => &["x", "y", "z"],
            Codec::Unity(UnityKind::Vector4 | UnityKind::Quaternion) => &["x", "y", "z", "w"],
            Codec::Unity(UnityKind::Color) => &["r", "g", "b", "a"],
            Codec::Unity(UnityKind::Rect) => &["x", "y", "width", "height"],
            Codec::Unity(UnityKind::Bounds) => &["center", "size"],
            Codec::Unity(UnityKind::AnimationCurve) => &["preWrapMode", "postWrapMode", "keys"],
            Codec::Unity(UnityKind::Gradient) => &["colors", "alpha"],
            _ => &[],
        };
        if !read.is_empty() && m.keys().any(|k| !read.contains(&k.as_str())) {
            // Not representable in the node; the value is mismatched instead of trimmed.
            return self
                .cx
                .opaque(v, Reason::ConverterError("object has keys the converter ignores"));
        }
        let f32_of = |k: &str| m.get(k).and_then(json_f32);
        match self.codec {
            Codec::WeakLink(_) => {
                // `AssetId`, else the legacy `guid` key.
                match (m.get("AssetId"), m.get("guid")) {
                    (Some(Value::String(s)), _) => Node::WeakLink(Some(s.as_str().into())),
                    (Some(Value::Null), _) | (None, None | Some(Value::Null)) => {
                        Node::WeakLink(None)
                    }
                    (None, Some(Value::String(g))) => {
                        self.cx
                            .note("weak link written as an asset reference (guid read as AssetId)");
                        Node::WeakLinkLegacy(
                            g.as_str().into(),
                            m.get("fileid").and_then(Value::as_i64),
                        )
                    }
                    _ => self.cx.opaque(v, Reason::ConverterError("weak link AssetId")),
                }
            }
            Codec::SharedString => {
                let asset = match m.get("assetguid") {
                    Some(Value::String(a)) => Some(a.as_str().into()),
                    None | Some(Value::Null) => None,
                    Some(_) => return self.cx.opaque(v, Reason::ConverterError("assetguid")),
                };
                match m.get("stringkey") {
                    Some(Value::String(s)) => Node::SharedString(Some(Box::new(SharedRef {
                        key: s.as_str().into(),
                        asset,
                    }))),
                    // `(string)jObject["stringkey"]`: a missing key gives a null key, and the
                    // asset still exists.
                    Some(Value::Null) | None => {
                        Node::SharedString(Some(Box::new(SharedRef { key: "".into(), asset })))
                    }
                    Some(_) => self.cx.opaque(v, Reason::ConverterError("stringkey")),
                }
            }
            Codec::Asset(_) => {
                let guid = m.get("guid").and_then(Value::as_str);
                let file_id = m.get("fileid").and_then(Value::as_i64);
                let present = |k: &str| m.get(k).is_some_and(|x| !x.is_null());
                match (guid, file_id) {
                    (Some(g), Some(f)) => {
                        Node::Asset(Some(Box::new(AssetRef { guid: g.into(), file_id: f })))
                    }
                    (None, None) if !present("guid") && !present("fileid") => Node::Asset(None),
                    // The converter returns null when either is missing or unreadable.
                    _ => {
                        self.cx.note("incomplete asset reference read as null");
                        Node::Coerced(Box::new(Node::Asset(None)), Box::new(v))
                    }
                }
            }
            Codec::Unity(k) => {
                let keys: &[&str] = match k {
                    UnityKind::Vector2 => &["x", "y"],
                    UnityKind::Vector3 => &["x", "y", "z"],
                    UnityKind::Vector4 | UnityKind::Quaternion => &["x", "y", "z", "w"],
                    UnityKind::Color => &["r", "g", "b", "a"],
                    UnityKind::Rect => &["x", "y", "width", "height"],
                    UnityKind::Vector2Int | UnityKind::Vector3Int => {
                        let names: &[&str] =
                            if k == UnityKind::Vector2Int { &["x", "y"] } else { &["x", "y", "z"] };
                        let vals: Option<Vec<i32>> = names
                            .iter()
                            .map(|n| {
                                m.get(*n)
                                    .and_then(Value::as_i64)
                                    .and_then(|x| i32::try_from(x).ok())
                            })
                            .collect();
                        return match vals {
                            Some(v) => Node::Ints(v.into()),
                            // `GetVector2Int`: a missing component gives `Vector2Int.zero`.
                            None if k == UnityKind::Vector2Int
                                && names.iter().any(|n| m.get(*n).is_none()) =>
                            {
                                self.cx.note("partial Vector2Int read as zero");
                                Node::Coerced(Box::new(Node::Ints(vec![0, 0].into())), Box::new(v))
                            }
                            None => self.cx.opaque(v, Reason::ConverterError("vector components")),
                        };
                    }
                    UnityKind::Bounds => {
                        let c = m.get("center").and_then(json_vec3);
                        let s = m.get("size").and_then(json_vec3);
                        return match (c, s) {
                            (Some(c), Some(s)) => {
                                Node::Floats(vec![c[0], c[1], c[2], s[0], s[1], s[2]].into())
                            }
                            _ => self.cx.opaque(v, Reason::ConverterError("Bounds")),
                        };
                    }
                    UnityKind::AnimationCurve => return self.curve(&v),
                    UnityKind::Gradient => return self.gradient(&v),
                    UnityKind::Color32 => return self.mismatch(v),
                };
                // Color (its converter can't read) and Vector4 (no converter) go through the
                // field contract: a missing component stays 0. Vector2/3 and Quaternion
                // converters index every component and fail on a missing one.
                let lenient = matches!(k, UnityKind::Color | UnityKind::Vector4);
                let vals: Option<Vec<f32>> = keys
                    .iter()
                    .map(|key| match m.get(*key) {
                        None if lenient => Some(0.0),
                        _ => f32_of(key),
                    })
                    .collect();
                if lenient && keys.iter().any(|key| !m.contains_key(*key)) {
                    self.cx.note("missing vector component read as 0 (field contract)");
                }
                match vals {
                    Some(x) => Node::Floats(x.into()),
                    None => self.cx.opaque(v, Reason::ConverterError("vector components")),
                }
            }
            Codec::PrototypeLink | Codec::Overrides => Node::Discarded(Box::new(v)),
            _ => self.mismatch(v),
        }
    }

    fn curve(&self, v: &Value) -> Node {
        let (pre, post, keys) = match v {
            Value::Array(a) => (0, 0, Some(a)),
            Value::Object(m) => {
                let w =
                    |k: &str| m.get(k).and_then(Value::as_i64).and_then(|x| i32::try_from(x).ok());
                match (w("preWrapMode"), w("postWrapMode")) {
                    (Some(a), Some(b)) => match m.get("keys") {
                        Some(Value::Array(k)) => (a, b, Some(k)),
                        None | Some(Value::Null) => (a, b, None),
                        // `jObject["keys"] as JArray` is null: an empty curve.
                        Some(_) => {
                            self.cx.note("curve keys not an array: read as an empty curve");
                            let empty = Curve { pre_wrap: a, post_wrap: b, keys: Vec::new() };
                            return Node::Coerced(
                                Box::new(Node::Curve(Box::new(empty))),
                                Box::new(v.clone()),
                            );
                        }
                    },
                    _ => {
                        return self
                            .cx
                            .opaque(v.clone(), Reason::ConverterError("curve wrap modes"));
                    }
                }
            }
            _ => return self.mismatch(v.clone()),
        };
        const KEY_FIELDS: [&str; 7] =
            ["time", "value", "inTangent", "outTangent", "inWeight", "outWeight", "weightedMode"];
        let mut out = Vec::new();
        for k in keys.into_iter().flatten() {
            if k.as_object().is_some_and(|o| o.keys().any(|x| !KEY_FIELDS.contains(&x.as_str()))) {
                return self.cx.opaque(
                    v.clone(),
                    Reason::ConverterError("curve key has keys the converter ignores"),
                );
            }
            let f = |n: &str| k.get(n).and_then(json_f32);
            let wm =
                k.get("weightedMode").and_then(Value::as_i64).and_then(|x| i32::try_from(x).ok());
            match (
                f("time"),
                f("value"),
                f("inTangent"),
                f("outTangent"),
                f("inWeight"),
                f("outWeight"),
                wm,
            ) {
                (Some(time), Some(value), Some(it), Some(ot), Some(iw), Some(ow), Some(wm)) => {
                    out.push(Keyframe {
                        time,
                        value,
                        in_tangent: it,
                        out_tangent: ot,
                        in_weight: iw,
                        out_weight: ow,
                        weighted_mode: wm,
                    });
                }
                _ => return self.cx.opaque(v.clone(), Reason::ConverterError("curve key")),
            }
        }
        // `AnimationCurve.AddKey` keeps keys sorted by time and refuses a second key at
        // the same time.
        if out.windows(2).any(|w| w[0].time >= w[1].time) {
            self.cx.note("curve keys not strictly increasing (AddKey reorders or drops)");
        }
        Node::Curve(Box::new(Curve { pre_wrap: pre, post_wrap: post, keys: out }))
    }

    fn gradient(&self, v: &Value) -> Node {
        let colors = v.get("colors").and_then(Value::as_array);
        let alphas = v.get("alpha").and_then(Value::as_array);
        let (Some(colors), Some(alphas)) = (colors, alphas) else {
            return self.cx.opaque(v.clone(), Reason::ConverterError("gradient"));
        };
        let only = |o: &Value, keys: &[&str]| {
            o.as_object().is_some_and(|m| m.keys().all(|k| keys.contains(&k.as_str())))
        };
        if !colors.iter().all(|o| only(o, &["time", "r", "g", "b"]))
            || !alphas.iter().all(|o| only(o, &["time", "a"]))
        {
            return self.cx.opaque(
                v.clone(),
                Reason::ConverterError("gradient key has keys the converter ignores"),
            );
        }
        let f = |o: &Value, k: &str| o.get(k).and_then(json_f32);
        let c: Option<Vec<[f32; 4]>> = colors
            .iter()
            .map(|o| Some([f(o, "time")?, f(o, "r")?, f(o, "g")?, f(o, "b")?]))
            .collect();
        let a: Option<Vec<[f32; 2]>> =
            alphas.iter().map(|o| Some([f(o, "time")?, f(o, "a")?])).collect();
        match (c, a) {
            (Some(colors), Some(alphas)) => {
                Node::Gradient(Box::new(Gradient { colors, alphas, mode: 0 }))
            }
            _ => self.cx.opaque(v.clone(), Reason::ConverterError("gradient key")),
        }
    }

    /// Decodes an object of declared type `declared` from a map.
    fn object<'de, A: MapAccess<'de>>(
        &self,
        declared: TypeIdx,
        mut map: A,
    ) -> Result<Node, A::Error> {
        let cx = self.cx;
        let before = cx.opaque_count();
        // Newtonsoft reads a leading run of metadata properties (`$id`, `$ref`, `$type`,
        // `$values`, in any order) before the object's own properties.
        let mut meta: Vec<(String, Value)> = Vec::new();
        let mut pending_first: Option<Key<'de>> = None;
        while let Some(k) = map.next_key_seed(KeySeed)? {
            if matches!(k.as_str(), "$id" | "$ref" | "$type" | "$values") {
                let v: Value = map.next_value()?;
                meta.push((k.as_str().to_owned(), v));
            } else {
                pending_first = Some(k);
                break;
            }
        }
        let mut tagged = false;
        let mut actual = declared;
        if meta.iter().any(|(k, _)| k == "$ref" || k == "$values") {
            let rest = rest_of_map(&mut map, meta, pending_first)?;
            return Ok(
                cx.opaque(rest, Reason::ConverterError("$ref/$values metadata not modelled"))
            );
        }
        if let Some((_, tag)) = meta.iter().find(|(k, _)| k == "$type") {
            let Value::String(tag_s) = tag else {
                let rest = rest_of_map(&mut map, meta, pending_first)?;
                return Ok(cx.opaque(rest, Reason::BadTypeTag));
            };
            let tag_s = tag_s.clone();
            let guid_part = tag_s.split(',').next().unwrap_or("").trim();
            match Guid::parse(guid_part).and_then(schema::by_type_id) {
                None => {
                    let rest = rest_of_map(&mut map, meta, pending_first)?;
                    return Ok(cx.opaque(rest, Reason::UnknownTypeId(tag_s.as_str().into())));
                }
                Some(t) if !schema::is_assignable(t, declared) => {
                    let rest = rest_of_map(&mut map, meta, pending_first)?;
                    return Ok(cx.opaque(
                        rest,
                        Reason::NotAssignable { tag: tag_s.as_str().into(), declared },
                    ));
                }
                Some(t) => {
                    actual = t;
                    tagged = true;
                }
            }
        }
        let def = schema::ty(actual);
        if def.has(type_flags::ABSTRACT) || def.has(type_flags::INTERFACE) {
            let rest = rest_of_map(&mut map, meta, pending_first)?;
            return Ok(cx.opaque(rest, Reason::AbstractWithoutTag(actual)));
        }
        let fields = def.fields;
        let mut slots: Vec<Node> = vec![Node::Absent; fields.len()];
        // Metadata other than `$type` (an `$id`) isn't a field; keep it.
        let mut extra: Vec<(Box<str>, Value)> = meta
            .into_iter()
            .filter(|(k, _)| k != "$type")
            .map(|(k, v)| (k.into_boxed_str(), v))
            .collect();
        let mut cursor = 0usize;
        let mut key = pending_first;
        loop {
            let k = match key.take() {
                Some(k) => k,
                None => match map.next_key_seed(KeySeed)? {
                    Some(k) => k,
                    None => break,
                },
            };
            let name = k.as_str();
            let found = find_field(fields, name, cursor).or_else(|| {
                let ci =
                    fields.iter().position(|f| f.in_json() && f.json.eq_ignore_ascii_case(name));
                if ci.is_some() {
                    cx.note("property matched ignoring case");
                }
                ci
            });
            match found {
                Some(j) => {
                    cursor = j + 1;
                    let f = &fields[j];
                    if f.flags & schema::field_flags::INIT_POPULATED != 0 {
                        // Not modelled: the game merges into the initializer's value.
                        cx.note(
                            "field whose initializer Newtonsoft would populate, present in JSON",
                        );
                    }
                    let node = map.next_value_seed(NodeSeed { cx, codec: f.codec })?;
                    if !matches!(slots[j], Node::Absent) {
                        // Newtonsoft sets the field again; for a list it adds to the
                        // existing one. Not modelled: we keep the last value.
                        cx.note("duplicate property (last value kept)");
                    }
                    slots[j] = node;
                }
                None => {
                    let v: Value = map.next_value()?;
                    extra.push((name.into(), v));
                }
            }
        }
        // Tally.
        {
            let mut t = cx.tally.borrow_mut();
            let mut typed = 0;
            let mut opaque = 0;
            let mut absent = 0;
            for (j, (s, f)) in slots.iter().zip(fields).enumerate() {
                if !f.in_json() {
                    continue;
                }
                match s {
                    Node::Absent => {
                        absent += 1;
                        *t.absent
                            .entry((actual, u32::try_from(j).unwrap_or(u32::MAX)))
                            .or_default() += 1;
                        if matches!(f.default, Def::Unknown | Def::NonEmptyStr) {
                            t.absent_unknown_default += 1;
                        }
                    }
                    Node::Opaque(_) => opaque += 1,
                    _ => typed += 1,
                }
            }
            for (k, _) in &extra {
                *t.unknown_fields.entry((actual, k.clone())).or_default() += 1;
            }
            let clean = t.opaque_nodes == before;
            let n_extra = extra.len() as u64;
            let tt = t.ty(actual);
            tt.objects += 1;
            if tagged {
                tt.tagged += 1;
            }
            if clean {
                tt.clean += 1;
            }
            tt.fields_typed += typed;
            tt.fields_opaque += opaque;
            tt.fields_absent += absent;
            tt.unknown_fields += n_extra;
        }
        Ok(Node::Object(Box::new(Object { ty: actual, tagged, fields: slots, extra })))
    }
}

/// A value the game's converter changes, with the text it was written as.
fn coerced(n: Node, original: &str) -> Node {
    Node::Coerced(Box::new(n), Box::new(Value::String(original.into())))
}

/// `(float)jToken`: a number, or a string `Convert.ToSingle` accepts (`"Infinity"`,
/// `"-Infinity"`, `"NaN"`, decimal text).
fn json_f32(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(f64_to_f32),
        Value::String(s) => match s.as_str() {
            "Infinity" => Some(f32::INFINITY),
            "-Infinity" => Some(f32::NEG_INFINITY),
            "NaN" => Some(f32::NAN),
            t => t.parse::<f64>().ok().map(f64_to_f32),
        },
        _ => None,
    }
}

/// A `Vector3` inside another converter (`BoundsConverter` calls `ToObject<Vector3>` with
/// the serializer, so the `x|y|z` shorthand works there too).
fn json_vec3(v: &Value) -> Option<[f32; 3]> {
    match v {
        Value::String(s) => {
            let p: Vec<i32> = s.split('|').map(|x| x.parse().ok()).collect::<Option<_>>()?;
            #[allow(clippy::cast_precision_loss)]
            (p.len() == 3).then(|| [p[0] as f32 / 100.0, p[1] as f32 / 100.0, p[2] as f32 / 100.0])
        }
        // Extra keys would be ignored by the converter; refuse them so they aren't lost.
        Value::Object(m) if m.keys().all(|k| matches!(k.as_str(), "x" | "y" | "z")) => {
            Some([json_f32(m.get("x")?)?, json_f32(m.get("y")?)?, json_f32(m.get("z")?)?])
        }
        _ => None,
    }
}

/// Whether `v` fits an integer of `bits` and signedness.
fn fits(v: i128, bits: u8, signed: bool) -> bool {
    let (lo, hi): (i128, i128) = if signed {
        (-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1)
    } else {
        (0, (1i128 << bits) - 1)
    };
    (lo..=hi).contains(&v)
}

/// An unchecked conversion to an integer of `bits` and signedness, as `i64`.
fn wrap_to(v: i128, bits: u8, signed: bool) -> i64 {
    let mask: i128 = if bits >= 64 { -1 } else { (1i128 << bits) - 1 };
    let u = v & mask;
    let r = if signed && bits < 128 && u >> (bits - 1) & 1 == 1 { u - (1i128 << bits) } else { u };
    i64::try_from(r).unwrap_or_else(|_| {
        #[allow(clippy::cast_possible_truncation)]
        let t = r as i64;
        t
    })
}

fn f64_to_f32(v: f64) -> f32 {
    // Newtonsoft reads a double and `Convert.ToSingle` rounds it to nearest.
    #[allow(clippy::cast_possible_truncation)]
    let f = v as f32;
    f
}

/// JSON-visible field `name`, trying the expected position first (JSON follows the
/// schema's order almost always).
fn find_field(fields: &[schema::Field], name: &str, cursor: usize) -> Option<usize> {
    if let Some(f) = fields.get(cursor)
        && f.in_json()
        && f.json == name
    {
        return Some(cursor);
    }
    fields.iter().position(|f| f.in_json() && f.json == name)
}

/// Collects the rest of a map into a `Value`, after already-read entries and a key whose
/// value hasn't been read yet.
fn rest_of_map<'de, A: MapAccess<'de>>(
    map: &mut A,
    read: Vec<(String, Value)>,
    pending: Option<Key<'de>>,
) -> Result<Value, A::Error> {
    let mut m = serde_json::Map::new();
    for (k, v) in read {
        m.insert(k, v);
    }
    if let Some(k) = pending {
        let v: Value = map.next_value()?;
        m.insert(k.as_str().to_owned(), v);
    }
    while let Some(k) = map.next_key::<String>()? {
        let v: Value = map.next_value()?;
        m.insert(k, v);
    }
    Ok(Value::Object(m))
}

/// A map key, borrowed from the input when possible.
#[derive(Debug)]
enum Key<'de> {
    Borrowed(&'de str),
    Owned(String),
}

impl Key<'_> {
    fn as_str(&self) -> &str {
        match self {
            Key::Borrowed(s) => s,
            Key::Owned(s) => s,
        }
    }
}

struct KeySeed;

impl<'de> DeserializeSeed<'de> for KeySeed {
    type Value = Key<'de>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Key<'de>, D::Error> {
        d.deserialize_str(self)
    }
}

impl<'de> Visitor<'de> for KeySeed {
    type Value = Key<'de>;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a property name")
    }
    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Key<'de>, E> {
        Ok(Key::Borrowed(v))
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Key<'de>, E> {
        Ok(Key::Owned(v.to_owned()))
    }
    fn visit_string<E: de::Error>(self, v: String) -> Result<Key<'de>, E> {
        Ok(Key::Owned(v))
    }
}

impl<'de> Visitor<'de> for NodeSeed<'_, '_> {
    type Value = Node;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Node, E> {
        Ok(match self.codec {
            Codec::Bool => Node::Bool(v),
            Codec::PrototypeLink | Codec::Overrides => Node::Discarded(Box::new(Value::Bool(v))),
            _ => self.mismatch(Value::Bool(v)),
        })
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Node, E> {
        Ok(self.int(i128::from(v), Value::from(v)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Node, E> {
        Ok(self.int(i128::from(v), Value::from(v)))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Node, E> {
        let raw = serde_json::Number::from_f64(v).map_or(Value::Null, Value::Number);
        Ok(match self.codec {
            Codec::F32 => Node::F32(f64_to_f32(v)),
            Codec::F64 => Node::F64(v),
            // `Convert.ToInt32(double)`: round half to even.
            Codec::Unity(UnityKind::Color32) if v.is_finite() && v.abs() < 2.1e9 => {
                self.cx.note("Color32 given as a float (rounded half to even)");
                #[allow(clippy::cast_possible_truncation)]
                let r = v.round_ties_even() as i64;
                Node::Coerced(Box::new(Node::Int(r)), Box::new(raw))
            }
            // Integer fields and enums reject float tokens (`ReadAsInt32` "not a valid
            // integer", `StringEnumConverter` throws); a float read into a string keeps
            // its JSON text, which we no longer have exactly.
            Codec::PrototypeLink | Codec::Overrides => Node::Discarded(Box::new(raw)),
            _ => self.mismatch(raw),
        })
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Node, E> {
        Ok(self.string(v))
    }

    fn visit_unit<E: de::Error>(self) -> Result<Node, E> {
        Ok(self.null())
    }

    fn visit_none<E: de::Error>(self) -> Result<Node, E> {
        Ok(self.null())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Node, A::Error> {
        match self.codec {
            Codec::Seq { elem, .. } => {
                let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(4));
                while let Some(n) = seq.next_element_seed(NodeSeed { cx: self.cx, codec: *elem })? {
                    out.push(n);
                }
                Ok(Node::Seq(out))
            }
            _ => {
                let v = Value::deserialize(SeqAccessDeserializer::new(seq))?;
                Ok(match self.codec {
                    Codec::Unity(UnityKind::AnimationCurve) => self.curve(&v),
                    Codec::PrototypeLink | Codec::Overrides => Node::Discarded(Box::new(v)),
                    _ => self.mismatch(v),
                })
            }
        }
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Node, A::Error> {
        match self.codec {
            Codec::Object(t) => self.object(t, map),
            _ => {
                let v = Value::deserialize(MapAccessDeserializer::new(map))?;
                Ok(self.value_map(v))
            }
        }
    }
}

/// A decoded `.jbp`.
#[derive(Debug)]
pub struct Decoded {
    /// `AssetId` from the wrapper.
    pub guid: Option<Guid>,
    /// The wrapper object (`BlueprintJsonWrapper`); its `Data` slot holds the blueprint.
    pub wrapper: Node,
    /// Concrete type of `Data`, if it decoded.
    pub data_type: Option<TypeIdx>,
    pub refs: Vec<RefUse>,
    /// Opaque values anywhere in this blueprint.
    pub opaque: u64,
}

/// Index of the wrapper type in the schema.
#[must_use]
pub fn wrapper_type() -> TypeIdx {
    schema::find("Kingmaker.Blueprints.JsonSystem.BlueprintJsonWrapper")
        .expect("schema has BlueprintJsonWrapper")
}

/// Decodes one `.jbp` file's bytes (a leading UTF-8 BOM is skipped).
///
/// # Errors
///
/// If the bytes aren't valid JSON. Schema mismatches are not errors: they become opaque
/// values and are counted in `tally`.
pub fn decode_jbp(bytes: &[u8], tally: &mut Tally) -> Result<Decoded, serde_json::Error> {
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let before = tally.opaque_nodes;
    let cx = Cx::new(tally);
    let mut de = serde_json::Deserializer::from_slice(bytes);
    let wrapper =
        NodeSeed { cx: &cx, codec: Codec::Object(wrapper_type()) }.deserialize(&mut de)?;
    de.end()?;
    let refs = cx.take_refs();
    drop(cx);
    let opaque = tally.opaque_nodes - before;
    let (guid, data_type) = match &wrapper {
        Node::Object(o) => {
            let guid = match o.get("AssetId") {
                Some(Node::Str(s)) => Guid::parse(s),
                _ => None,
            };
            let dt = match o.get("Data") {
                Some(Node::Object(d)) => Some(d.ty),
                _ => None,
            };
            (guid, dt)
        }
        _ => (None, None),
    };
    Ok(Decoded { guid, wrapper, data_type, refs, opaque })
}

/// The blueprint object (`Data`) of a decoded wrapper.
#[must_use]
pub fn data(wrapper: &Node) -> Option<&Object> {
    wrapper.as_object()?.get("Data")?.as_object()
}

#[cfg(test)]
mod tests;
