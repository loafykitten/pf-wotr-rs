//! Finding references inside decoded component fields.
//!
//! Scene components point at other things in four ways, recognised by value shape:
//! - Unity object pointers (`{m_FileID, m_PathID}`): scene objects and assets;
//! - blueprint references (`BlueprintReferenceBase`: a lone `{guid}` string field);
//! - entity references (`EntityReference`: `UniqueId` with `EntityNameInEditor` or
//!   `SceneAssetGuid`), resolved against scene entity IDs at runtime;
//! - resource links (`WeakResourceLink`/`PrefabLink`: a lone `{AssetId}`), resolved
//!   through `locationlist.json`.

use serde::Serialize;

use crate::unity::value::{PPtr, Value};

/// Base fields every component has; not references worth reporting.
pub const BASE_FIELDS: [&str; 4] = ["m_GameObject", "m_Enabled", "m_Script", "m_Name"];

/// A reference found in a value, before resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RawRef {
    Object { field: String, pptr: PPtr },
    Blueprint { field: String, guid: String },
    Entity { field: String, unique_id: String },
    Resource { field: String, asset_id: String },
}

impl RawRef {
    #[must_use]
    pub fn field(&self) -> &str {
        match self {
            RawRef::Object { field, .. }
            | RawRef::Blueprint { field, .. }
            | RawRef::Entity { field, .. }
            | RawRef::Resource { field, .. } => field,
        }
    }
}

fn is_only(v: &Value, key: &str) -> Option<String> {
    match v.fields() {
        [(k, Value::Str(s))] if k == key => Some(s.clone()),
        _ => None,
    }
}

/// Collects every reference in `v` (a whole component), skipping base fields and null
/// or empty references.
#[must_use]
pub fn collect(v: &Value) -> Vec<RawRef> {
    let mut out = Vec::new();
    for (k, child) in v.fields() {
        if BASE_FIELDS.contains(&k.as_str()) {
            continue;
        }
        let mut path = k.clone();
        walk(child, &mut path, &mut out);
    }
    out
}

fn walk(v: &Value, path: &mut String, out: &mut Vec<RawRef>) {
    match v {
        Value::Object(fields) => {
            if let Some(p) = v.as_pptr().filter(|_| fields.len() == 2) {
                if !p.is_null() {
                    out.push(RawRef::Object { field: path.clone(), pptr: p });
                }
                return;
            }
            if let Some(guid) = is_only(v, "guid") {
                if !guid.is_empty() {
                    out.push(RawRef::Blueprint { field: path.clone(), guid });
                }
                return;
            }
            if let Some(asset_id) = is_only(v, "AssetId") {
                if !asset_id.is_empty() {
                    out.push(RawRef::Resource { field: path.clone(), asset_id });
                }
                return;
            }
            let has = |k: &str| fields.iter().any(|(n, _)| n == k);
            if has("UniqueId") && (has("EntityNameInEditor") || has("SceneAssetGuid")) {
                if let Some(id) = v.str_at("UniqueId").filter(|s| !s.is_empty()) {
                    out.push(RawRef::Entity { field: path.clone(), unique_id: id.to_owned() });
                }
                // An EntityReference has no nested references.
                return;
            }
            for (k, child) in fields {
                let len = path.len();
                path.push('.');
                path.push_str(k);
                walk(child, path, out);
                path.truncate(len);
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().enumerate() {
                let len = path.len();
                path.push_str(&format!("[{i}]"));
                walk(child, path, out);
                path.truncate(len);
            }
        }
        _ => {}
    }
}

/// Method names bound in serialized `UnityEvent`s (`m_PersistentCalls.m_Calls`): scene
/// data that invokes code by name at runtime.
#[must_use]
pub fn persistent_calls(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    v.walk(&mut String::new(), &mut |path, val| {
        if path.ends_with("m_PersistentCalls.m_Calls") {
            for call in val.as_array().unwrap_or_default() {
                if let Some(m) = call.str_at("m_MethodName") {
                    out.push(m.to_owned());
                }
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(f: Vec<(&str, Value)>) -> Value {
        Value::Object(f.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
    }
    fn pptr(file: i64, path: i64) -> Value {
        obj(vec![("m_FileID", Value::Int(file)), ("m_PathID", Value::Int(path))])
    }
    fn s(x: &str) -> Value {
        Value::Str(x.to_owned())
    }

    #[test]
    fn finds_each_reference_kind() {
        let v = obj(vec![
            ("m_GameObject", pptr(0, 5)),
            ("m_Script", pptr(1, 9)),
            ("m_Blueprint", obj(vec![("guid", s("abc"))])),
            ("m_Empty", obj(vec![("guid", s(""))])),
            ("Target", pptr(2, 77)),
            ("Null", pptr(0, 0)),
            (
                "Settings",
                obj(vec![
                    ("m_AreaEnterPoint", obj(vec![("guid", s("def"))])),
                    ("Prefab", obj(vec![("AssetId", s("123"))])),
                ]),
            ),
            (
                "Units",
                Value::Array(vec![obj(vec![
                    ("EntityNameInEditor", s("x")),
                    ("UniqueId", s("u-1")),
                    ("SceneAssetGuid", s("")),
                ])]),
            ),
        ]);
        let refs = collect(&v);
        assert_eq!(
            refs,
            vec![
                RawRef::Blueprint { field: "m_Blueprint".into(), guid: "abc".into() },
                RawRef::Object { field: "Target".into(), pptr: PPtr { file_id: 2, path_id: 77 } },
                RawRef::Blueprint { field: "Settings.m_AreaEnterPoint".into(), guid: "def".into() },
                RawRef::Resource { field: "Settings.Prefab".into(), asset_id: "123".into() },
                RawRef::Entity { field: "Units[0]".into(), unique_id: "u-1".into() },
            ]
        );
    }

    #[test]
    fn finds_persistent_calls() {
        let v = obj(vec![(
            "OnUnitEntered",
            obj(vec![(
                "m_PersistentCalls",
                obj(vec![(
                    "m_Calls",
                    Value::Array(vec![obj(vec![("m_MethodName", s("SetActive"))])]),
                )]),
            )]),
        )]);
        assert_eq!(persistent_calls(&v), ["SetActive"]);
    }
}
