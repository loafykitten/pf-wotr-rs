//! A generic decoded Unity object: what a type tree read produces.

use serde::Serialize;
use serde::ser::{SerializeMap, Serializer};

/// A decoded field value. Objects keep field order, as in the type tree.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f32),
    Double(f64),
    Str(String),
    /// `vector<UInt8>`/`TypelessData`: kept as raw bytes.
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

/// A reference to another Unity object: `m_FileID` indexes the file's externals
/// (0 = same file, n = externals[n-1]); `m_PathID` is the object id in that file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct PPtr {
    pub file_id: i32,
    pub path_id: i64,
}

impl PPtr {
    #[must_use]
    pub fn is_null(&self) -> bool {
        self.path_id == 0
    }
}

impl Value {
    /// Field by name (objects only).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(f) => f.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Dotted path lookup, e.g. `"m_LocalPosition.x"`.
    #[must_use]
    pub fn path(&self, path: &str) -> Option<&Value> {
        path.split('.').try_fold(self, |v, k| v.get(k))
    }

    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match *self {
            Value::Int(i) => Some(i),
            Value::UInt(u) => i64::try_from(u).ok(),
            Value::Bool(b) => Some(i64::from(b)),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_f32(&self) -> Option<f32> {
        match *self {
            Value::Float(f) => Some(f),
            #[expect(clippy::cast_possible_truncation, reason = "doubles narrowed on request")]
            Value::Double(d) => Some(d as f32),
            #[expect(clippy::cast_precision_loss, reason = "ints read as floats on request")]
            Value::Int(i) => Some(i as f32),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            Value::Int(i) => Some(i != 0),
            Value::UInt(u) => Some(u != 0),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Raw bytes of a string or byte-array value.
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Str(s) => Some(s.as_bytes()),
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    #[must_use]
    pub fn fields(&self) -> &[(String, Value)] {
        match self {
            Value::Object(f) => f,
            _ => &[],
        }
    }

    /// Interprets an object with `m_FileID`/`m_PathID` as a [`PPtr`].
    #[must_use]
    pub fn as_pptr(&self) -> Option<PPtr> {
        let file_id = i32::try_from(self.get("m_FileID")?.as_i64()?).ok()?;
        let path_id = self.get("m_PathID")?.as_i64()?;
        Some(PPtr { file_id, path_id })
    }

    #[must_use]
    pub fn str_at(&self, path: &str) -> Option<&str> {
        self.path(path)?.as_str()
    }

    #[must_use]
    pub fn f32_at(&self, path: &str) -> Option<f32> {
        self.path(path)?.as_f32()
    }

    #[must_use]
    pub fn i64_at(&self, path: &str) -> Option<i64> {
        self.path(path)?.as_i64()
    }

    /// `{x, y, z}` as an array.
    #[must_use]
    pub fn vec3(&self) -> Option<[f32; 3]> {
        Some([self.f32_at("x")?, self.f32_at("y")?, self.f32_at("z")?])
    }

    /// `{x, y, z, w}` as an array.
    #[must_use]
    pub fn vec4(&self) -> Option<[f32; 4]> {
        Some([self.f32_at("x")?, self.f32_at("y")?, self.f32_at("z")?, self.f32_at("w")?])
    }

    /// Visits every value depth-first, with the dotted field path.
    pub fn walk<'a>(&'a self, path: &mut String, f: &mut dyn FnMut(&str, &'a Value)) {
        f(path, self);
        match self {
            Value::Object(fields) => {
                for (k, v) in fields {
                    let len = path.len();
                    if !path.is_empty() {
                        path.push('.');
                    }
                    path.push_str(k);
                    v.walk(path, f);
                    path.truncate(len);
                }
            }
            Value::Array(items) => {
                for (i, v) in items.iter().enumerate() {
                    let len = path.len();
                    path.push_str(&format!("[{i}]"));
                    v.walk(path, f);
                    path.truncate(len);
                }
            }
            _ => {}
        }
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Bool(b) => s.serialize_bool(*b),
            Value::Int(i) => s.serialize_i64(*i),
            Value::UInt(u) => s.serialize_u64(*u),
            Value::Float(f) => {
                if f.is_finite() {
                    s.serialize_f32(*f)
                } else {
                    s.serialize_str(&f.to_string())
                }
            }
            Value::Double(d) => {
                if d.is_finite() {
                    s.serialize_f64(*d)
                } else {
                    s.serialize_str(&d.to_string())
                }
            }
            Value::Str(t) => s.serialize_str(t),
            Value::Bytes(b) => s.serialize_str(&format!("<{} bytes>", b.len())),
            Value::Array(a) => a.serialize(s),
            Value::Object(fields) => {
                let mut m = s.serialize_map(Some(fields.len()))?;
                for (k, v) in fields {
                    m.serialize_entry(k, v)?;
                }
                m.end()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(f: Vec<(&str, Value)>) -> Value {
        Value::Object(f.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }

    #[test]
    fn paths_and_pptrs() {
        let v = obj(vec![
            (
                "m_LocalPosition",
                obj(vec![
                    ("x", Value::Float(1.0)),
                    ("y", Value::Float(2.0)),
                    ("z", Value::Float(3.0)),
                ]),
            ),
            ("m_Father", obj(vec![("m_FileID", Value::Int(0)), ("m_PathID", Value::Int(42))])),
        ]);
        assert_eq!(v.path("m_LocalPosition").unwrap().vec3(), Some([1.0, 2.0, 3.0]));
        assert_eq!(v.f32_at("m_LocalPosition.y"), Some(2.0));
        assert_eq!(v.get("m_Father").unwrap().as_pptr(), Some(PPtr { file_id: 0, path_id: 42 }));
        let mut seen = Vec::new();
        v.walk(&mut String::new(), &mut |p, _| seen.push(p.to_owned()));
        assert!(seen.contains(&"m_Father.m_PathID".to_owned()));
    }
}
