//! Decoded blueprint values.
//!
//! A [`Node`] is a value decoded through the schema. Everything the JSON holds is kept:
//! fields the schema doesn't know go to [`Object::extra`], values that don't fit their
//! field's codec become [`Node::Opaque`] with the reason, and converter-discarded values
//! become [`Node::Discarded`]. Nothing is dropped silently.

use serde_json::Value;

use crate::guid::Guid;
use crate::schema::TypeIdx;

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// The field isn't in the JSON: the object keeps its C# initializer value.
    Absent,
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    F32(f32),
    F64(f64),
    Str(Box<str>),
    /// An enum's numeric value (names are resolved through the schema).
    Enum(i64),
    /// A blueprint reference; [`Guid::EMPTY`] for null or empty.
    BpRef(Guid),
    /// `WeakResourceLink.AssetId`.
    WeakLink(Option<Box<str>>),
    /// A weak link written as a Unity asset reference `{"guid", "fileid"}` (the field was
    /// a direct asset reference when the file was saved). The game's converter takes
    /// `guid` as the `AssetId` and ignores `fileid`; both are kept.
    WeakLinkLegacy(Box<str>, Option<i64>),
    /// A `SharedStringAsset`: the key the converter reads, and the asset GUID the JSON
    /// also carries (the converter ignores it; kept so nothing is dropped).
    SharedString(Option<Box<SharedRef>>),
    /// A Unity asset reference.
    Asset(Option<Box<AssetRef>>),
    /// A Unity object as the runtime pack stores it: an index into the game's
    /// `BlueprintReferencedAssets` list (-1 for null). Only the `.bbp` decoder makes these.
    AssetIndex(i32),
    /// Vector2/3/4 (`x y z w`), Quaternion, Color (`r g b a`), Bounds (`center`, `size`).
    Floats(Box<[f32]>),
    /// Vector2Int, Vector3Int.
    Ints(Box<[i32]>),
    Curve(Box<Curve>),
    Gradient(Box<Gradient>),
    Seq(Vec<Node>),
    Object(Box<Object>),
    /// The game's converter reads this field as null whatever it holds; kept as written.
    Discarded(Box<Value>),
    /// A value the game's converter changes (an unknown enum name read as 0, reference
    /// text without `!bp_` read as empty): the value as the game reads it, and the JSON
    /// as written.
    Coerced(Box<Node>, Box<Value>),
    /// Couldn't be decoded through the schema; kept as written, with the reason.
    Opaque(Box<Opaque>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedRef {
    pub key: Box<str>,
    pub asset: Option<Box<str>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRef {
    pub guid: Box<str>,
    pub file_id: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Keyframe {
    pub time: f32,
    pub value: f32,
    pub in_tangent: f32,
    pub out_tangent: f32,
    pub in_weight: f32,
    pub out_weight: f32,
    pub weighted_mode: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub pre_wrap: i32,
    pub post_wrap: i32,
    pub keys: Vec<Keyframe>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Gradient {
    /// `(time, r, g, b)`.
    pub colors: Vec<[f32; 4]>,
    /// `(time, a)`.
    pub alphas: Vec<[f32; 2]>,
    /// `Gradient.mode` (0 = Blend, Unity's default). The JSON doesn't carry it; the pack
    /// does.
    pub mode: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Object {
    /// The concrete type (the `$type` tag's, or the field's declared type).
    pub ty: TypeIdx,
    /// Whether the JSON carried a `$type` tag.
    pub tagged: bool,
    /// One slot per schema field, in schema order.
    pub fields: Vec<Node>,
    /// JSON properties the schema doesn't have (editor-only or stale fields), in order.
    pub extra: Vec<(Box<str>, Value)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Opaque {
    pub value: Value,
    pub reason: Reason,
}

/// Why a value couldn't be decoded through the schema.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Reason {
    /// A `$type` GUID with no registered type.
    UnknownTypeId(Box<str>),
    /// A `$type` not assignable to the field's declared type.
    NotAssignable { tag: Box<str>, declared: TypeIdx },
    /// No `$type` and the declared type is abstract or an interface.
    AbstractWithoutTag(TypeIdx),
    /// A malformed `$type` value.
    BadTypeTag,
    /// The JSON value's shape doesn't fit the codec (`codec`, JSON kind).
    Mismatch { codec: &'static str, json: &'static str },
    /// The field's type has no codec.
    Unsupported(&'static str),
    /// A converter would throw on this value.
    ConverterError(&'static str),
}

impl Node {
    #[must_use]
    pub fn as_object(&self) -> Option<&Object> {
        match self {
            Self::Object(o) => Some(o),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_opaque(&self) -> bool {
        matches!(self, Self::Opaque(_))
    }
}

impl Object {
    /// The slot for a field by C# name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Node> {
        let t = crate::schema::ty(self.ty);
        t.fields.iter().position(|f| f.name == name).map(|i| &self.fields[i])
    }
}

/// Name of a JSON value's kind, for reasons.
#[must_use]
pub fn json_kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "integer",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// Visits every node depth-first with a dotted path (`Data.Components[3].m_Value`).
pub fn walk<'a>(n: &'a Node, path: &mut String, f: &mut dyn FnMut(&str, &'a Node)) {
    f(path, n);
    match n {
        Node::Coerced(inner, _) => walk(inner, path, f),
        Node::Seq(items) => {
            for (i, it) in items.iter().enumerate() {
                let len = path.len();
                path.push_str(&format!("[{i}]"));
                walk(it, path, f);
                path.truncate(len);
            }
        }
        Node::Object(o) => {
            let t = crate::schema::ty(o.ty);
            for (field, it) in t.fields.iter().zip(&o.fields) {
                let len = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(field.json);
                walk(it, path, f);
                path.truncate(len);
            }
        }
        _ => {}
    }
}
