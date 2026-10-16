//! [`Rv`], a recorded value: the immutable, compact JSON value a [`super::Trace`] holds its
//! records in (`docs/poc5/replay-memory.md`).
//!
//! A long capture holds millions of small records. As `serde_json::Value`s each took about
//! 900 bytes (a B-tree leaf of eleven slots per object, an allocation per key and per string);
//! an `Rv` object is one sorted slice of (key, value) pairs, keys are interned for the process
//! and strings per trace, so a record takes about a quarter of that.
//!
//! `Rv` reads like `serde_json::Value` (indexing, `as_*`, `is_*`, `get`, comparisons with
//! strings, numbers and booleans, `Display` and `Debug`), with the same results: numbers are
//! `serde_json::Number`s made as `serde_json` makes them, objects iterate in key order (as the
//! default `serde_json::Map` does, the last of duplicate keys kept), and a missing key or index
//! reads as `Null`. [`Rv::to_json`] and [`Rv::from_json`] convert.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap as _, SerializeSeq as _};
use serde_json::{Number, Value as J};

/// A recorded JSON value.
#[derive(Clone, Default, PartialEq)]
pub enum Rv {
    #[default]
    Null,
    Bool(bool),
    Number(Number),
    String(Str),
    Array(Box<[Rv]>),
    Object(Obj),
}

/// A string value (shared: equal strings of a trace are one allocation).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Str(Arc<str>);

impl Str {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::ops::Deref for Str {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Str {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&*self.0, f)
    }
}

impl fmt::Display for Str {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&*self.0, f)
    }
}

impl PartialEq<str> for Str {
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<&str> for Str {
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == *other
    }
}

impl PartialEq<String> for Str {
    fn eq(&self, other: &String) -> bool {
        *self.0 == **other
    }
}

impl PartialEq<Str> for String {
    fn eq(&self, other: &Str) -> bool {
        **self == *other.0
    }
}

impl PartialEq<Str> for str {
    fn eq(&self, other: &Str) -> bool {
        *self == *other.0
    }
}

impl PartialEq<Str> for &str {
    fn eq(&self, other: &Str) -> bool {
        **self == *other.0
    }
}

/// An object key, interned for the process (the trace vocabulary is a few hundred names): one
/// pointer.
#[derive(Clone, Copy)]
pub struct Key(&'static String);

impl Key {
    /// The interned key for `s`.
    fn intern(s: &str) -> Self {
        static KEYS: OnceLock<Mutex<HashMap<Box<str>, &'static String>>> = OnceLock::new();
        let mut keys = KEYS.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner());
        Self(keys.entry(Box::from(s)).or_insert_with(|| Box::leak(Box::new(s.to_owned()))))
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_str(), f)
    }
}

impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.0, other.0) || self.as_str() == other.as_str()
    }
}

/// An object: its entries sorted by key, keys unique.
#[derive(Clone, Default, PartialEq)]
pub struct Obj(Box<[(Key, Rv)]>);

impl Obj {
    /// From entries in any order; of duplicate keys the last is kept (as `serde_json::Map`'s
    /// inserts do).
    fn from_entries(mut v: Vec<(Key, Rv)>) -> Self {
        v.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        let mut out: Vec<(Key, Rv)> = Vec::with_capacity(v.len());
        for (k, x) in v {
            match out.last_mut() {
                Some(last) if last.0 == k => last.1 = x,
                _ => out.push((k, x)),
            }
        }
        Self(out.into_boxed_slice())
    }

    #[must_use]
    pub fn get<Q: AsRef<str> + ?Sized>(&self, key: &Q) -> Option<&Rv> {
        let key = key.as_ref();
        self.0.binary_search_by(|(k, _)| k.as_str().cmp(key)).ok().map(|i| &self.0[i].1)
    }

    #[must_use]
    pub fn contains_key<Q: AsRef<str> + ?Sized>(&self, key: &Q) -> bool {
        self.get(key).is_some()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The entries in key order.
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&str, &Rv)> + ExactSizeIterator {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &str> + ExactSizeIterator {
        self.0.iter().map(|(k, _)| k.as_str())
    }

    pub fn values(&self) -> impl DoubleEndedIterator<Item = &Rv> + ExactSizeIterator {
        self.0.iter().map(|(_, v)| v)
    }

    /// Removes `key`'s entry, returning its value.
    pub fn remove(&mut self, key: &str) -> Option<Rv> {
        let i = self.0.binary_search_by(|(k, _)| k.as_str().cmp(key)).ok()?;
        let mut v = std::mem::take(&mut self.0).into_vec();
        let (_, x) = v.remove(i);
        self.0 = v.into_boxed_slice();
        Some(x)
    }

    /// As a `serde_json::Map`.
    #[must_use]
    pub fn to_json(&self) -> serde_json::Map<String, J> {
        self.0.iter().map(|(k, v)| (k.as_str().to_owned(), v.to_json())).collect()
    }
}

impl<'a> IntoIterator for &'a Obj {
    type Item = (&'a str, &'a Rv);
    type IntoIter =
        std::iter::Map<std::slice::Iter<'a, (Key, Rv)>, fn(&'a (Key, Rv)) -> (&'a str, &'a Rv)>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }
}

/// `map[key]`: panics when the key is missing (as `serde_json::Map`'s does).
impl std::ops::Index<&str> for Obj {
    type Output = Rv;
    fn index(&self, key: &str) -> &Rv {
        self.get(key).expect("no entry found for key")
    }
}

impl fmt::Debug for Obj {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.0.iter().map(|(k, v)| (k.as_str(), v))).finish()
    }
}

static NULL: Rv = Rv::Null;

impl Rv {
    /// The value at `index` (a key of an object, a position in an array).
    pub fn get<I: RvIndex>(&self, index: I) -> Option<&Rv> {
        index.index_into(self)
    }

    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_boolean(&self) -> bool {
        self.as_bool().is_some()
    }

    #[must_use]
    pub fn as_number(&self) -> Option<&Number> {
        match self {
            Self::Number(n) => Some(n),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_number(&self) -> bool {
        matches!(self, Self::Number(_))
    }

    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        self.as_number().and_then(Number::as_i64)
    }

    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        self.as_number().and_then(Number::as_u64)
    }

    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        self.as_number().and_then(Number::as_f64)
    }

    #[must_use]
    pub fn is_i64(&self) -> bool {
        self.as_number().is_some_and(Number::is_i64)
    }

    #[must_use]
    pub fn is_u64(&self) -> bool {
        self.as_number().is_some_and(Number::is_u64)
    }

    #[must_use]
    pub fn is_f64(&self) -> bool {
        self.as_number().is_some_and(Number::is_f64)
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_string(&self) -> bool {
        matches!(self, Self::String(_))
    }

    #[must_use]
    pub fn as_array(&self) -> Option<&[Rv]> {
        match self {
            Self::Array(a) => Some(a),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_array(&self) -> bool {
        matches!(self, Self::Array(_))
    }

    #[must_use]
    pub fn as_object(&self) -> Option<&Obj> {
        match self {
            Self::Object(o) => Some(o),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_object(&self) -> bool {
        matches!(self, Self::Object(_))
    }

    pub fn as_object_mut(&mut self) -> Option<&mut Obj> {
        match self {
            Self::Object(o) => Some(o),
            _ => None,
        }
    }

    /// As a `serde_json::Value`.
    #[must_use]
    pub fn to_json(&self) -> J {
        match self {
            Self::Null => J::Null,
            Self::Bool(b) => J::Bool(*b),
            Self::Number(n) => J::Number(n.clone()),
            Self::String(s) => J::String(s.as_str().to_owned()),
            Self::Array(a) => J::Array(a.iter().map(Self::to_json).collect()),
            Self::Object(o) => J::Object(o.to_json()),
        }
    }

    /// From a `serde_json::Value` (strings not shared).
    #[must_use]
    pub fn from_json(v: &J) -> Self {
        Interner::default().from_json(v)
    }
}

/// What indexes an [`Rv`]: `str` and `String` (object keys) and `usize` (array positions).
pub trait RvIndex {
    fn index_into<'v>(&self, v: &'v Rv) -> Option<&'v Rv>;
}

impl RvIndex for usize {
    fn index_into<'v>(&self, v: &'v Rv) -> Option<&'v Rv> {
        match v {
            Rv::Array(a) => a.get(*self),
            _ => None,
        }
    }
}

impl RvIndex for str {
    fn index_into<'v>(&self, v: &'v Rv) -> Option<&'v Rv> {
        match v {
            Rv::Object(o) => o.get(self),
            _ => None,
        }
    }
}

impl RvIndex for String {
    fn index_into<'v>(&self, v: &'v Rv) -> Option<&'v Rv> {
        self.as_str().index_into(v)
    }
}

impl<T: RvIndex + ?Sized> RvIndex for &T {
    fn index_into<'v>(&self, v: &'v Rv) -> Option<&'v Rv> {
        (**self).index_into(v)
    }
}

/// `v[key]`, `v[i]`: `Null` when missing (as `serde_json::Value`'s read does).
impl<I: RvIndex> std::ops::Index<I> for Rv {
    type Output = Rv;
    fn index(&self, index: I) -> &Rv {
        index.index_into(self).unwrap_or(&NULL)
    }
}

// Comparisons as `serde_json::Value`'s (`value/partial_eq.rs`).
impl PartialEq<str> for Rv {
    fn eq(&self, other: &str) -> bool {
        self.as_str() == Some(other)
    }
}

impl PartialEq<&str> for Rv {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == Some(*other)
    }
}

impl PartialEq<String> for Rv {
    fn eq(&self, other: &String) -> bool {
        self.as_str() == Some(other.as_str())
    }
}

impl PartialEq<Rv> for str {
    fn eq(&self, other: &Rv) -> bool {
        other.as_str() == Some(self)
    }
}

impl PartialEq<Rv> for &str {
    fn eq(&self, other: &Rv) -> bool {
        other.as_str() == Some(*self)
    }
}

impl PartialEq<Rv> for String {
    fn eq(&self, other: &Rv) -> bool {
        other.as_str() == Some(self.as_str())
    }
}

macro_rules! partialeq_numeric {
    ($($as:ident $t:ty [$($ty:ty)*])*) => {
        $($(
            impl PartialEq<$ty> for Rv {
                #[allow(clippy::cast_lossless, clippy::float_cmp)]
                fn eq(&self, other: &$ty) -> bool {
                    self.$as() == Some(*other as $t)
                }
            }

            impl PartialEq<Rv> for $ty {
                #[allow(clippy::cast_lossless, clippy::float_cmp)]
                fn eq(&self, other: &Rv) -> bool {
                    other.$as() == Some(*self as $t)
                }
            }

            impl PartialEq<$ty> for &Rv {
                #[allow(clippy::cast_lossless, clippy::float_cmp)]
                fn eq(&self, other: &$ty) -> bool {
                    self.$as() == Some(*other as $t)
                }
            }
        )*)*
    }
}

#[allow(clippy::cast_possible_wrap, clippy::cast_sign_loss)]
mod numeric {
    use super::Rv;
    partialeq_numeric! {
        as_i64 i64 [i8 i16 i32 i64 isize]
        as_u64 u64 [u8 u16 u32 u64 usize]
        as_f64 f64 [f64]
        as_bool bool [bool]
    }
}

/// Structurally, as the `serde_json::Value` would compare.
impl PartialEq<J> for Rv {
    fn eq(&self, other: &J) -> bool {
        match (self, other) {
            (Self::Null, J::Null) => true,
            (Self::Bool(a), J::Bool(b)) => a == b,
            (Self::Number(a), J::Number(b)) => a == b,
            (Self::String(a), J::String(b)) => a.as_str() == b,
            (Self::Array(a), J::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x == y)
            }
            (Self::Object(a), J::Object(b)) => {
                a.len() == b.len()
                    && a.iter().zip(b).all(|((ka, va), (kb, vb))| ka == kb && va == vb)
            }
            _ => false,
        }
    }
}

impl PartialEq<Rv> for J {
    fn eq(&self, other: &Rv) -> bool {
        other == self
    }
}

/// Structurally, as the `serde_json::Map` would compare.
impl PartialEq<serde_json::Map<String, J>> for Obj {
    fn eq(&self, other: &serde_json::Map<String, J>) -> bool {
        self.len() == other.len()
            && self.iter().zip(other).all(|((ka, va), (kb, vb))| ka == kb && va == vb)
    }
}

/// `a == b` for optional values (`Option<&Value>`'s comparison).
#[must_use]
pub fn opt_eq(a: Option<&Rv>, b: Option<&J>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

impl fmt::Debug for Rv {
    /// As `serde_json::Value`'s.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("Null"),
            Self::Bool(b) => write!(f, "Bool({b})"),
            Self::Number(n) => fmt::Debug::fmt(n, f),
            Self::String(s) => write!(f, "String({:?})", s.as_str()),
            Self::Array(a) => {
                f.write_str("Array ")?;
                f.debug_list().entries(a.iter()).finish()
            }
            Self::Object(o) => {
                f.write_str("Object ")?;
                fmt::Debug::fmt(o, f)
            }
        }
    }
}

impl fmt::Display for Rv {
    /// As `serde_json::Value`'s: compact JSON, pretty with `{:#}`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = if f.alternate() {
            serde_json::to_string_pretty(self)
        } else {
            serde_json::to_string(self)
        }
        .map_err(|_| fmt::Error)?;
        f.write_str(&s)
    }
}

impl serde::Serialize for Rv {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => s.serialize_unit(),
            Self::Bool(b) => s.serialize_bool(*b),
            Self::Number(n) => n.serialize(s),
            Self::String(x) => s.serialize_str(x.as_str()),
            Self::Array(a) => {
                let mut seq = s.serialize_seq(Some(a.len()))?;
                for x in a {
                    seq.serialize_element(x)?;
                }
                seq.end()
            }
            Self::Object(o) => o.serialize(s),
        }
    }
}

impl serde::Serialize for Obj {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(self.len()))?;
        for (k, v) in self {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

/// [`Rv::from_json`].
impl From<&J> for Rv {
    fn from(v: &J) -> Self {
        Self::from_json(v)
    }
}

/// [`Rv::from_json`].
impl From<J> for Rv {
    fn from(v: J) -> Self {
        Self::from_json(&v)
    }
}

/// A string value (not shared).
impl From<&str> for Rv {
    fn from(s: &str) -> Self {
        Self::String(Str(Arc::from(s)))
    }
}

impl From<&Rv> for J {
    fn from(v: &Rv) -> Self {
        v.to_json()
    }
}

impl From<Rv> for J {
    fn from(v: Rv) -> Self {
        v.to_json()
    }
}

/// Shares a trace's strings while parsing it.
#[derive(Debug, Default)]
pub struct Interner {
    strs: HashSet<Arc<str>>,
    keys: HashMap<Box<str>, Key>,
}

impl Interner {
    fn string(&mut self, s: &str) -> Rv {
        if let Some(a) = self.strs.get(s) {
            return Rv::String(Str(a.clone()));
        }
        let a: Arc<str> = Arc::from(s);
        self.strs.insert(a.clone());
        Rv::String(Str(a))
    }

    fn key(&mut self, s: &str) -> Key {
        if let Some(k) = self.keys.get(s) {
            return *k;
        }
        let k = Key::intern(s);
        self.keys.insert(Box::from(s), k);
        k
    }

    /// Parses one JSON text (as `serde_json::from_str` would, trailing characters an error).
    ///
    /// # Errors
    ///
    /// Malformed JSON.
    pub fn parse(&mut self, text: &str) -> serde_json::Result<Rv> {
        let mut de = serde_json::Deserializer::from_str(text);
        let v = Seed(self).deserialize(&mut de)?;
        de.end()?;
        Ok(v)
    }

    /// Converts a `serde_json::Value`.
    pub fn from_json(&mut self, v: &J) -> Rv {
        match v {
            J::Null => Rv::Null,
            J::Bool(b) => Rv::Bool(*b),
            J::Number(n) => Rv::Number(n.clone()),
            J::String(s) => self.string(s),
            J::Array(a) => Rv::Array(a.iter().map(|x| self.from_json(x)).collect()),
            J::Object(o) => Rv::Object(Obj::from_entries(
                o.iter().map(|(k, x)| (self.key(k), self.from_json(x))).collect(),
            )),
        }
    }
}

/// Parses a value into an [`Rv`] the way `serde_json::Value`'s visitor does.
struct Seed<'a>(&'a mut Interner);

impl<'de> DeserializeSeed<'de> for Seed<'_> {
    type Value = Rv;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Rv, D::Error> {
        d.deserialize_any(self)
    }
}

/// An object key, interned.
struct KeySeed<'a>(&'a mut Interner);

impl<'de> DeserializeSeed<'de> for KeySeed<'_> {
    type Value = Key;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Key, D::Error> {
        d.deserialize_str(self)
    }
}

impl Visitor<'_> for KeySeed<'_> {
    type Value = Key;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a string key")
    }
    fn visit_str<E: de::Error>(self, v: &str) -> Result<Key, E> {
        Ok(self.0.key(v))
    }
}

impl<'de> Visitor<'de> for Seed<'_> {
    type Value = Rv;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Rv, E> {
        Ok(Rv::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Rv, E> {
        Ok(Rv::Number(v.into()))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Rv, E> {
        Ok(Rv::Number(v.into()))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Rv, E> {
        Ok(Number::from_f64(v).map_or(Rv::Null, Rv::Number))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Rv, E> {
        Ok(self.0.string(v))
    }

    fn visit_none<E>(self) -> Result<Rv, E> {
        Ok(Rv::Null)
    }

    fn visit_some<D: de::Deserializer<'de>>(self, d: D) -> Result<Rv, D::Error> {
        Seed(self.0).deserialize(d)
    }

    fn visit_unit<E>(self) -> Result<Rv, E> {
        Ok(Rv::Null)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Rv, A::Error> {
        let mut v = Vec::new();
        while let Some(x) = seq.next_element_seed(Seed(&mut *self.0))? {
            v.push(x);
        }
        Ok(Rv::Array(v.into_boxed_slice()))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Rv, A::Error> {
        let mut v = Vec::new();
        while let Some(k) = map.next_key_seed(KeySeed(&mut *self.0))? {
            v.push((k, map.next_value_seed(Seed(&mut *self.0))?));
        }
        Ok(Rv::Object(Obj::from_entries(v)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parsed as an `Rv` and as a `serde_json::Value`, the same value: equal, printed and
    /// debugged alike, the same reads.
    #[test]
    fn reads_as_serde_json_does() {
        let texts = [
            r#"{"seq":3,"k":"obs","t":-1,"ev":"mem_v","v":false,"f":0.0166666675,"z":null}"#,
            r#"{"b":1,"a":2,"b":3,"n":-0,"x":1e3,"y":18446744073709551615,"w":-9223372036854775808}"#,
            r#"{"arr":[1,"two",[3.5,{"c":"d","a":[]}],{}],"s":"é\"\\\n","e":""}"#,
            r#"[1,2,{"k":true}]"#,
            r#""just a string""#,
            "12.5",
        ];
        let mut i = Interner::default();
        for t in texts {
            let j: J = serde_json::from_str(t).unwrap();
            let r = i.parse(t).unwrap();
            assert!(r == j, "{t}");
            assert_eq!(r.to_json(), j, "{t}");
            assert_eq!(Rv::from_json(&j), r, "{t}");
            assert_eq!(r.to_string(), j.to_string(), "{t}");
            assert_eq!(format!("{r:#}"), format!("{j:#}"), "{t}");
            assert_eq!(format!("{r:?}"), format!("{j:?}"), "{t}");
            assert_eq!(format!("{r:#?}"), format!("{j:#?}"), "{t}");
            for k in ["seq", "k", "t", "v", "f", "z", "b", "n", "x", "y", "w", "arr", "s", "e", "q"]
            {
                assert!(r[k] == j[k], "{t} {k}");
                assert_eq!(r[k].as_i64(), j[k].as_i64());
                assert_eq!(r[k].as_u64(), j[k].as_u64());
                assert_eq!(r[k].as_f64().map(f64::to_bits), j[k].as_f64().map(f64::to_bits));
                assert_eq!(r[k].as_str(), j[k].as_str());
                assert_eq!(r[k].as_bool(), j[k].as_bool());
                assert_eq!(r[k].is_null(), j[k].is_null());
                assert_eq!(r.get(k).is_some(), j.get(k).is_some());
                assert_eq!(r[k] == "obs", j[k] == "obs");
                assert_eq!(r[k] == 3, j[k] == 3);
                assert_eq!(r[k] == false, j[k] == false);
                assert_eq!(r[k] == 1000.0, j[k] == 1000.0);
            }
            for n in 0..4 {
                assert!(r[n] == j[n]);
                assert!(r["arr"][n] == j["arr"][n]);
            }
        }
        assert!(i.parse(r#"{"a":1} x"#).is_err());
        assert!(i.parse(r#"{"a":"#).is_err());
    }
}
