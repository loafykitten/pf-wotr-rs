//! `wr-import tree`: a prefab or scene bundle's object census and GameObject hierarchy, with each
//! component's class (a MonoBehaviour's script) and, on request, its serialized fields. The survey
//! tool for the unit and equipment prefabs.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use anyhow::Result;

use crate::area::Universe;
use crate::unity::serialized::{SerializedFile, class_id, class_name};
use crate::unity::value::Value;

/// What to print besides the hierarchy.
#[derive(Debug, Default)]
pub struct TreeOptions {
    /// Script classes (or Unity class names) whose fields are printed, as JSON.
    pub fields: Vec<String>,
    /// Unity classes (by name, e.g. `AnimationClip`) whose objects are listed by name and id.
    pub list: Vec<String>,
    /// Deepest hierarchy level printed (`None`: all).
    pub max_depth: Option<usize>,
}

/// The census and hierarchy of one serialized file.
pub fn describe(universe: &Universe, f: &SerializedFile<'_>, opt: &TreeOptions) -> Result<String> {
    let mut out = String::new();
    let mut census: BTreeMap<String, usize> = BTreeMap::new();
    for o in &f.objects {
        *census.entry(class_name(o.class_id)).or_default() += 1;
    }
    writeln!(out, "file {}: {} objects", f.name, f.objects.len())?;
    for (k, n) in &census {
        writeln!(out, "  {k}: {n}")?;
    }
    for o in f.objects.iter().filter(|o| o.class_id == class_id::SHADER) {
        let v = f.read_prefix(o, "m_ParsedForm")?;
        writeln!(
            out,
            "Shader '{}' [obj {}]",
            v.str_at("m_ParsedForm.m_Name").unwrap_or_default(),
            o.path_id
        )?;
    }
    for o in &f.objects {
        let cn = class_name(o.class_id);
        if opt.list.contains(&cn) {
            let v = f.read_prefix(o, "m_Name")?;
            writeln!(out, "{cn} '{}' [obj {}]", v.str_at("m_Name").unwrap_or_default(), o.path_id)?;
        }
    }
    let mut names: HashMap<i64, String> = HashMap::new();
    let mut comps: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut go_of_transform: HashMap<i64, i64> = HashMap::new();
    let mut transform_of_go: HashMap<i64, i64> = HashMap::new();
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut roots = Vec::new();
    for o in &f.objects {
        match o.class_id {
            class_id::GAME_OBJECT => {
                let v = f.read(o)?;
                names.insert(o.path_id, v.str_at("m_Name").unwrap_or_default().to_owned());
                comps.insert(
                    o.path_id,
                    v.get("m_Component")
                        .and_then(Value::as_array)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|c| c.get("component").and_then(Value::as_pptr))
                        .filter(|p| p.file_id == 0)
                        .map(|p| p.path_id)
                        .collect(),
                );
            }
            class_id::TRANSFORM | class_id::RECT_TRANSFORM => {
                let v = f.read(o)?;
                let go = v.get("m_GameObject").and_then(Value::as_pptr).map_or(0, |p| p.path_id);
                go_of_transform.insert(o.path_id, go);
                transform_of_go.insert(go, o.path_id);
                match v.get("m_Father").and_then(Value::as_pptr).filter(|p| !p.is_null()) {
                    Some(p) => children.entry(p.path_id).or_default().push(o.path_id),
                    None => roots.push(o.path_id),
                }
            }
            _ => {}
        }
    }
    // ScriptableObjects (MonoBehaviours without a GameObject): their script class and, on request, fields.
    for o in f.objects.iter().filter(|o| o.class_id == class_id::MONO_BEHAVIOUR) {
        // Player files carry no type trees: their MonoBehaviours can't be read.
        let Ok(v) = f.read(o) else { continue };
        if v.get("m_GameObject").and_then(Value::as_pptr).is_some_and(|p| !p.is_null()) {
            continue;
        }
        let class = universe.script_of(f, &v)?.map(|s| s.class).unwrap_or_default();
        let name = v.str_at("m_Name").unwrap_or_default();
        writeln!(out, "ScriptableObject {class} '{name}' [obj {}]", o.path_id)?;
        if opt.fields.contains(&class) {
            writeln!(out, "  <{class}> {}", serde_json::to_string(&v)?)?;
        }
    }
    roots.sort_unstable();
    for t in &roots {
        walk(universe, f, opt, *t, 0, &names, &comps, &go_of_transform, &children, &mut out)?;
    }
    Ok(out)
}

#[expect(clippy::too_many_arguments, reason = "a recursive printer over shared tables")]
fn walk(
    universe: &Universe,
    f: &SerializedFile<'_>,
    opt: &TreeOptions,
    t: i64,
    depth: usize,
    names: &HashMap<i64, String>,
    comps: &HashMap<i64, Vec<i64>>,
    go_of_transform: &HashMap<i64, i64>,
    children: &HashMap<i64, Vec<i64>>,
    out: &mut String,
) -> Result<()> {
    if opt.max_depth.is_some_and(|d| depth > d) {
        return Ok(());
    }
    let go = go_of_transform.get(&t).copied().unwrap_or(0);
    let indent = "  ".repeat(depth);
    let mut labels = Vec::new();
    let mut dumps = Vec::new();
    for c in comps.get(&go).map(Vec::as_slice).unwrap_or_default() {
        let Some(o) = f.object(*c) else { continue };
        if matches!(o.class_id, class_id::TRANSFORM | class_id::RECT_TRANSFORM) {
            continue;
        }
        let mut label = class_name(o.class_id);
        let mut key = label.clone();
        if o.class_id == class_id::MONO_BEHAVIOUR {
            let v = f.read(o)?;
            if let Some(s) = universe.script_of(f, &v)? {
                label = s.class.clone();
                key = s.class;
            }
        }
        if opt.fields.contains(&key) {
            dumps.push((key, serde_json::to_string(&f.read(o)?)?));
        }
        labels.push(label);
    }
    writeln!(
        out,
        "{indent}{} [go {go}, tr {t}] {}",
        names.get(&go).map_or("?", String::as_str),
        labels.join(", ")
    )?;
    for (k, j) in dumps {
        writeln!(out, "{indent}  <{k}> {j}")?;
    }
    let mut kids = children.get(&t).cloned().unwrap_or_default();
    kids.sort_unstable();
    for k in kids {
        walk(universe, f, opt, k, depth + 1, names, comps, go_of_transform, children, out)?;
    }
    Ok(())
}

/// How much of a pointed-to object `annotate` prints.
#[derive(Debug, Clone, Copy)]
pub struct Expand {
    /// Levels of pointers followed (0: a pointer prints as its target's class and name).
    pub depth: usize,
    /// Arrays longer than this print their first items and a count.
    pub max_items: usize,
}

/// `v` (an object read from `file`) as JSON with every pointer replaced by what it points at: its
/// class and name, and with `expand.depth` > 0 the target's own fields.
pub fn annotate(
    universe: &Universe,
    file: &SerializedFile<'_>,
    v: &Value,
    expand: Expand,
) -> Result<serde_json::Value> {
    use serde_json::{Value as J, json};
    if let Some(p) = v.as_pptr()
        && matches!(v, Value::Object(f) if f.len() == 2)
    {
        if p.is_null() {
            return Ok(J::Null);
        }
        let Some(t) = universe.resolve(file, p)? else {
            return Ok(json!(format!("-> missing file {} path {}", p.file_id, p.path_id)));
        };
        let tv = t.read()?;
        let mut name = tv.str_at("m_Name").unwrap_or_default().to_owned();
        let mut class = class_name(t.info.class_id);
        if t.info.class_id == class_id::MONO_BEHAVIOUR
            && let Some(s) = universe.script_of(t.file, &tv)?
        {
            class = s.class;
        }
        if t.info.class_id == class_id::GAME_OBJECT {
            name = tv.str_at("m_Name").unwrap_or_default().to_owned();
        }
        let label = format!("-> {class} '{name}' [{} #{}]", t.file.name, t.info.path_id);
        if expand.depth == 0 {
            return Ok(json!(label));
        }
        let body = annotate(universe, t.file, &tv, Expand { depth: expand.depth - 1, ..expand })?;
        return Ok(json!({ "ref": label, "value": body }));
    }
    Ok(match v {
        Value::Array(items) => {
            let mut out: Vec<J> = Vec::new();
            for i in items.iter().take(expand.max_items) {
                out.push(annotate(universe, file, i, expand)?);
            }
            if items.len() > expand.max_items {
                out.push(json!(format!("... {} items", items.len())));
            }
            J::Array(out)
        }
        Value::Object(fields) => {
            let mut m = serde_json::Map::new();
            for (k, x) in fields {
                m.insert(k.clone(), annotate(universe, file, x, expand)?);
            }
            J::Object(m)
        }
        other => serde_json::to_value(other)?,
    })
}
