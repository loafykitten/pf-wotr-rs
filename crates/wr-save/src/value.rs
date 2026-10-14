//! Decoded save values.
//!
//! A [`Value`] is what the game reads for a JSON value, plus whatever is needed to write
//! the JSON back exactly: how an enum or GUID was spelled, `"x|y|z"` positions as their
//! integer hundredths, `$id`s and `$type` strings as written, property order. Values a
//! converter changes are [`Value::Coerced`] (the game's value and the original JSON);
//! values that don't fit the schema are [`Value::Opaque`] with a reason. Nothing is
//! dropped.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::json::Json;
use crate::schema::TypeIdx;

/// No `$type` on the object.
pub const NO_TAG: u32 = u32::MAX;
/// Enum written as a number.
pub const ENUM_NUMBER: u16 = u16::MAX;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    F32(f32),
    F64(f64),
    /// Strings, chars, versions, entity ids.
    Str(Box<str>),
    /// Enum `e` (index into the schema's enums) with `value`; `name` is the member whose
    /// name the JSON used, or [`ENUM_NUMBER`].
    Enum {
        e: u16,
        name: u16,
        value: i64,
    },
    /// An enum written as other text that parses (`"A, B"`, a numeric string).
    EnumText(Box<EnumText>),
    /// A GUID written in `N` form (32 lowercase hex digits).
    GuidN([u8; 16]),
    /// A GUID written in `D` form (lowercase, hyphenated).
    GuidD([u8; 16]),
    /// An empty string meaning `BlueprintGuid.Empty`.
    GuidEmpty,
    /// `"null"`, which `BlueprintConverter` writes for a null blueprint.
    NullText,
    /// `"x|y(|z…)"` in hundredths (vectors, Euler angles, bone transforms).
    Cm(Box<[i32]>),
    /// A vector written as an object `{x, y[, z[, w]]}`.
    Floats(Box<[f32]>),
    /// `TimeSpan` ticks, written in the constant (`"c"`) format.
    TimeSpan(i64),
    /// `DateTime` / `DateTimeOffset`, as written (format checked).
    Date(Box<str>),
    Seq(Box<[Value]>),
    Dict(Box<Dict>),
    /// An object read by a converter (no metadata): named values in order.
    Fields(Box<[(Box<str>, Value)]>),
    Obj(Box<Obj>),
    Ref(Box<Ref>),
    /// JSON that is the value itself (`JToken`, an untyped `object`, a decimal).
    Json(Box<Json>),
    /// What the game reads, and the JSON as written.
    Coerced(Box<(Value, Json)>),
    Opaque(Box<Opaque>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnumText {
    pub e: u16,
    pub value: i64,
    pub text: Box<str>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Obj {
    /// Concrete type (index into the schema).
    pub ty: TypeIdx,
    /// Index into the document's `$type` strings, or [`NO_TAG`].
    pub tag: u32,
    pub id: Option<Id>,
    /// `$type` was written before `$id`.
    pub type_first: bool,
    pub props: Vec<Prop>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Id {
    /// A decimal string without leading zeros (what Newtonsoft writes).
    Num(u32),
    Text(Box<str>),
}

impl Id {
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s.parse::<u32>() {
            Ok(n) if n.to_string() == s => Self::Num(n),
            _ => Self::Text(s.into()),
        }
    }

    #[must_use]
    pub fn text(&self) -> std::borrow::Cow<'_, str> {
        match self {
            Self::Num(n) => n.to_string().into(),
            Self::Text(t) => std::borrow::Cow::Borrowed(t),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prop {
    pub key: Key,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Key {
    /// Index into the type's members.
    Member(u16),
    /// A member matched case-insensitively: its index and the name as written.
    MemberCase(u16, Box<str>),
    /// A property the type doesn't have (the game ignores it); its value is `Json`.
    Unknown(Box<str>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ref {
    pub id: Id,
    pub state: RefState,
}

/// What `DefaultReferenceResolver.ResolveReference` returns at the point the reference
/// is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RefState {
    /// An object registered earlier in the document.
    Resolved,
    /// The `$id` comes later (or belongs to an object still being constructed): the game
    /// reads null.
    Forward,
    /// No such `$id` in the document: null.
    Dangling,
    /// The target is inside a value we kept opaque.
    IntoOpaque,
    /// The target is inside a member the game skips: null.
    IntoSkipped,
    /// Decided after the document is read.
    Pending,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dict {
    pub form: DictForm,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DictForm {
    /// `{"key": value, …}`.
    Object,
    /// `[{"Key": k, "Value": v}, …]`.
    Array,
    /// One `{"Key": k, "Value": v}` (`KeyValuePair`).
    KeyValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Entry {
    Pair(Value, Value),
    /// A key and value the game reads from an element laid out otherwise (other
    /// property order, extra properties) or skipped after a failed key: the decoded pair and
    /// the element as written.
    Laid(Value, Value, Json),
    /// An entry the converter would skip or fail on, as written.
    Raw(Json, Reason),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Opaque {
    pub json: Json,
    pub reason: Reason,
}

/// Why a value wasn't decoded through the schema.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Reason {
    /// A `$type` the schema doesn't have.
    UnknownType(Box<str>),
    /// A `$type` from another assembly than the schema's type.
    WrongAssembly(Box<str>),
    /// A `$type` not assignable to the declared type.
    NotAssignable(Box<str>),
    /// No `$type` and the declared type is abstract or an interface.
    AbstractWithoutTag(Box<str>),
    /// The type has no constructor Newtonsoft can use.
    NoCreator(Box<str>),
    /// Malformed metadata (`$type`, `$id`, `$ref`, `$values`).
    BadMetadata(Cow<'static, str>),
    /// The JSON's shape doesn't fit the codec: (codec, JSON kind).
    Mismatch(Cow<'static, str>, Cow<'static, str>),
    /// The declared type has no codec.
    Unsupported(Box<str>),
    /// The converter would throw, or skip the entry.
    Converter(Cow<'static, str>),
}

impl Reason {
    /// Grouping key for reports.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::UnknownType(t) => format!("unknown $type {t}"),
            Self::WrongAssembly(t) => format!("wrong assembly {t}"),
            Self::NotAssignable(t) => format!("not assignable {t}"),
            Self::AbstractWithoutTag(t) => format!("abstract without $type {t}"),
            Self::NoCreator(t) => format!("no constructor {t}"),
            Self::BadMetadata(w) => format!("bad metadata: {w}"),
            Self::Mismatch(c, j) => format!("{j} for {c}"),
            Self::Unsupported(t) => format!("unsupported {t}"),
            Self::Converter(w) => format!("converter: {w}"),
        }
    }
}

/// A decoded document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Doc {
    pub bom: bool,
    pub root: Value,
    /// `$type` strings as written, indexed by [`Obj::tag`].
    pub tags: Vec<Box<str>>,
}

impl Value {
    #[must_use]
    pub fn as_obj(&self) -> Option<&Obj> {
        match self {
            Self::Obj(o) => Some(o),
            _ => None,
        }
    }

    /// Visits this value and everything inside it, depth-first in document order.
    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a Self)) {
        f(self);
        match self {
            Self::Seq(items) => items.iter().for_each(|v| v.walk(f)),
            Self::Dict(d) => {
                for e in &d.entries {
                    if let Entry::Pair(k, v) | Entry::Laid(k, v, _) = e {
                        k.walk(f);
                        v.walk(f);
                    }
                }
            }
            Self::Fields(fs) => fs.iter().for_each(|(_, v)| v.walk(f)),
            Self::Obj(o) => o.props.iter().for_each(|p| p.value.walk(f)),
            Self::Coerced(c) => c.0.walk(f),
            _ => {}
        }
    }

    /// Mutable depth-first visit (used to settle pending references).
    pub fn walk_mut(&mut self, f: &mut dyn FnMut(&mut Self)) {
        f(self);
        match self {
            Self::Seq(items) => items.iter_mut().for_each(|v| v.walk_mut(f)),
            Self::Dict(d) => {
                for e in &mut d.entries {
                    if let Entry::Pair(k, v) | Entry::Laid(k, v, _) = e {
                        k.walk_mut(f);
                        v.walk_mut(f);
                    }
                }
            }
            Self::Fields(fs) => fs.iter_mut().for_each(|(_, v)| v.walk_mut(f)),
            Self::Obj(o) => o.props.iter_mut().for_each(|p| p.value.walk_mut(f)),
            Self::Coerced(c) => c.0.walk_mut(f),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_stays_small() {
        assert!(std::mem::size_of::<Value>() <= 24, "{}", std::mem::size_of::<Value>());
    }

    #[test]
    fn ids() {
        assert_eq!(Id::parse("17"), Id::Num(17));
        assert_eq!(Id::parse("017"), Id::Text("017".into()));
        assert_eq!(Id::parse("x").text(), "x");
    }
}
