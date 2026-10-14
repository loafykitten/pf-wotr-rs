//! Navigation over a decoded save document (`wr_save::Doc`): members by JSON name, `$ref`s
//! resolved within the document (Newtonsoft's `$id` scope is one document), and object
//! identity, so objects shared through `$ref` (a modifier sitting in a stat's list and in a
//! field) become one sim object.

use std::collections::HashMap;

use wr_content::Guid;
use wr_save::schema;
use wr_save::value::{Doc, Entry, Key, Obj, RefState, Value};

/// A document with its `$id` index.
#[derive(Debug)]
pub struct DocIndex<'a> {
    pub doc: &'a Doc,
    ids: HashMap<String, &'a Obj>,
}

impl<'a> DocIndex<'a> {
    #[must_use]
    pub fn new(doc: &'a Doc) -> Self {
        let mut ids = HashMap::new();
        doc.root.walk(&mut |v| {
            if let Value::Obj(o) = v
                && let Some(id) = &o.id
            {
                ids.insert(id.text().into_owned(), &**o);
            }
        });
        Self { doc, ids }
    }

    /// The object a value denotes: itself, or the target of a resolved `$ref`. Forward and
    /// dangling references read as null in the game, so they give `None`.
    #[must_use]
    pub fn resolve(&self, v: &'a Value) -> Option<&'a Obj> {
        match v {
            Value::Obj(o) => Some(o),
            Value::Ref(r) if r.state == RefState::Resolved => {
                self.ids.get(r.id.text().as_ref()).copied()
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn root(&'a self) -> Option<O<'a>> {
        self.resolve(&self.doc.root).map(|o| O { ix: self, obj: o })
    }

    #[must_use]
    pub fn o(&'a self, obj: &'a Obj) -> O<'a> {
        O { ix: self, obj }
    }
}

/// An object in a document.
#[derive(Debug, Clone, Copy)]
pub struct O<'a> {
    pub ix: &'a DocIndex<'a>,
    pub obj: &'a Obj,
}

/// Identity of a save object (its address in the decoded tree), for mapping shared objects.
#[must_use]
pub fn identity(o: &Obj) -> usize {
    std::ptr::from_ref(o) as usize
}

impl<'a> O<'a> {
    #[must_use]
    pub fn id(&self) -> usize {
        identity(self.obj)
    }

    /// CLR short name of the object's concrete type.
    #[must_use]
    pub fn ty(&self) -> &'static str {
        schema::ty(self.obj.ty).short()
    }

    #[must_use]
    pub fn clr(&self) -> &'static str {
        schema::ty(self.obj.ty).clr
    }

    /// Whether the concrete type is or derives from the CLR type with this short name.
    #[must_use]
    pub fn is(&self, short: &str) -> bool {
        let mut cur = self.obj.ty;
        for _ in 0..64 {
            if cur == schema::NO_TYPE {
                return false;
            }
            let t = schema::ty(cur);
            if t.short() == short {
                return true;
            }
            cur = t.base;
        }
        false
    }

    /// A member's value by JSON name (exact match first, then case-insensitive, as the
    /// decoder matched it).
    #[must_use]
    pub fn get(&self, json: &str) -> Option<&'a Value> {
        let t = schema::ty(self.obj.ty);
        let want = t.member(json)?;
        self.obj.props.iter().rev().find_map(|p| match &p.key {
            Key::Member(i) | Key::MemberCase(i, _) if usize::from(*i) == want => Some(&p.value),
            _ => None,
        })
    }

    #[must_use]
    pub fn has(&self, json: &str) -> bool {
        self.get(json).is_some()
    }

    #[must_use]
    pub fn obj(&self, json: &str) -> Option<O<'a>> {
        self.ix.resolve(self.get(json)?).map(|o| O { ix: self.ix, obj: o })
    }

    #[must_use]
    pub fn int(&self, json: &str) -> Option<i64> {
        self.get(json).and_then(int_of)
    }

    #[must_use]
    pub fn i32_or(&self, json: &str, d: i32) -> i32 {
        self.int(json).and_then(|v| i32::try_from(v).ok()).unwrap_or(d)
    }

    #[must_use]
    pub fn bool_or(&self, json: &str, d: bool) -> bool {
        match self.get(json) {
            Some(Value::Bool(b)) => *b,
            _ => d,
        }
    }

    #[must_use]
    pub fn str(&self, json: &str) -> Option<&'a str> {
        match self.get(json)? {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// A blueprint GUID member (`BlueprintConverter`, `BlueprintRefConverter`).
    #[must_use]
    pub fn guid(&self, json: &str) -> Option<Guid> {
        self.get(json).and_then(guid_of)
    }

    /// `TimeSpan` ticks.
    #[must_use]
    pub fn timespan(&self, json: &str) -> Option<i64> {
        match self.get(json)? {
            Value::TimeSpan(t) => Some(*t),
            _ => None,
        }
    }

    /// The objects of a list member, `$ref`s resolved, nulls skipped.
    #[must_use]
    pub fn objs(&self, json: &str) -> Vec<O<'a>> {
        match self.get(json) {
            Some(Value::Seq(items)) => items
                .iter()
                .filter_map(|v| self.ix.resolve(v))
                .map(|o| O { ix: self.ix, obj: o })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The raw items of a list member (nulls kept as `None`).
    #[must_use]
    pub fn items(&self, json: &str) -> Vec<Option<O<'a>>> {
        match self.get(json) {
            Some(Value::Seq(items)) => items
                .iter()
                .map(|v| self.ix.resolve(v).map(|o| O { ix: self.ix, obj: o }))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Entries of a dictionary member in document order: (key, value).
    #[must_use]
    pub fn dict(&self, json: &str) -> Vec<(&'a Value, &'a Value)> {
        match self.get(json) {
            Some(Value::Dict(d)) => d
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Pair(k, v) | Entry::Laid(k, v, _) => Some((k, v)),
                    Entry::Raw(..) => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    #[must_use]
    pub fn wrap(&self, v: &'a Value) -> Option<O<'a>> {
        self.ix.resolve(v).map(|o| O { ix: self.ix, obj: o })
    }
}

/// An integer-like value (ints, enums, `CountableFlag` counts).
#[must_use]
pub fn int_of(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        Value::UInt(u) => i64::try_from(*u).ok(),
        Value::Enum { value, .. } => Some(*value),
        Value::EnumText(t) => Some(t.value),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Coerced(c) => int_of(&c.0),
        Value::Fields(fs) => {
            fs.iter().find(|(k, _)| &**k == "m_Count").and_then(|(_, v)| int_of(v))
        }
        _ => None,
    }
}

/// A GUID value.
#[must_use]
pub fn guid_of(v: &Value) -> Option<Guid> {
    match v {
        Value::GuidN(g) | Value::GuidD(g) => Some(Guid::from_bytes(*g)),
        Value::Coerced(c) => guid_of(&c.0),
        _ => None,
    }
}

/// A string value.
#[must_use]
pub fn str_of(v: &Value) -> Option<&str> {
    match v {
        Value::Str(s) => Some(s),
        _ => None,
    }
}
