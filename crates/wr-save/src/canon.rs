//! Canonical JSON of a decoded document, and equivalence with the original text.
//!
//! The canonical form is the document rewritten from its [`Value`]s alone: properties in
//! their original order, `$id`s and `$type` strings as written, enums and GUIDs spelled
//! as written, positions as their hundredths, coerced and opaque values as their original
//! JSON. Numbers are the one thing written differently: a typed float is written as the
//! shortest text that reads back to the same `float` or `double` (Newtonsoft writes
//! `"R"` text, which can be longer). [`check_source`] checks a decoded value against the
//! original under the declared C# types.
//!
//! Two decoded documents are equal for preservation when their canonical texts are
//! byte-identical (which covers every typed value, `$id` and `$ref`) and their values are
//! structurally equal (which also covers what the game reads for coerced values).

use std::fmt::Write as _;

use crate::decode::fmt_timespan;
use crate::json::{self, Json};
use crate::schema;
use crate::value::{Dict, DictForm, Doc, ENUM_NUMBER, Entry, Key, NO_TAG, Value};

/// Writes the canonical text of a document.
#[must_use]
pub fn doc_text(d: &Doc) -> String {
    let mut s = String::new();
    if d.bom {
        s.push('\u{feff}');
    }
    write_value(&d.root, &d.tags, &mut s);
    s
}

fn hex(g: &[u8; 16], out: &mut String) {
    for b in g {
        let _ = write!(out, "{b:02x}");
    }
}

fn write_f32(x: f32, out: &mut String) {
    if x.is_nan() {
        out.push_str("\"NaN\"");
    } else if x.is_infinite() {
        out.push_str(if x > 0.0 { "\"Infinity\"" } else { "\"-Infinity\"" });
    } else {
        let s = format!("{x:?}");
        out.push_str(&s);
    }
}

fn write_f64(x: f64, out: &mut String) {
    if x.is_nan() {
        out.push_str("\"NaN\"");
    } else if x.is_infinite() {
        out.push_str(if x > 0.0 { "\"Infinity\"" } else { "\"-Infinity\"" });
    } else {
        let s = format!("{x:?}");
        out.push_str(&s);
    }
}

fn enum_name(e: u16, name: u16) -> Option<&'static str> {
    if name == ENUM_NUMBER {
        return None;
    }
    schema::enum_def(u32::from(e)).members.get(usize::from(name)).map(|m| m.0)
}

/// The JSON property name a dictionary key is written under (object form).
fn key_text(k: &Value) -> String {
    match k {
        Value::Str(s) => s.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Enum { e, name, value } => {
            enum_name(*e, *name).map_or_else(|| value.to_string(), str::to_owned)
        }
        Value::EnumText(t) => t.text.to_string(),
        Value::Coerced(c) => match &c.1 {
            Json::Str(s) => s.to_string(),
            other => other.to_text(),
        },
        other => {
            let mut s = String::new();
            write_value(other, &[], &mut s);
            s
        }
    }
}

#[allow(clippy::too_many_lines)]
pub fn write_value(v: &Value, tags: &[Box<str>], out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => {
            let _ = write!(out, "{i}");
        }
        Value::UInt(u) => {
            let _ = write!(out, "{u}");
        }
        Value::F32(x) => write_f32(*x, out),
        Value::F64(x) => write_f64(*x, out),
        Value::Str(s) => json::write_str(s, out),
        Value::Enum { e, name, value } => match enum_name(*e, *name) {
            Some(n) => json::write_str(n, out),
            None => {
                let _ = write!(out, "{value}");
            }
        },
        Value::EnumText(t) => json::write_str(&t.text, out),
        Value::GuidN(g) => {
            out.push('"');
            hex(g, out);
            out.push('"');
        }
        Value::GuidD(g) => {
            let mut h = String::with_capacity(32);
            hex(g, &mut h);
            let _ = write!(
                out,
                "\"{}-{}-{}-{}-{}\"",
                &h[..8],
                &h[8..12],
                &h[12..16],
                &h[16..20],
                &h[20..]
            );
        }
        Value::GuidEmpty => out.push_str("\"\""),
        Value::NullText => out.push_str("\"null\""),
        Value::Cm(c) => {
            out.push('"');
            for (i, x) in c.iter().enumerate() {
                if i > 0 {
                    out.push('|');
                }
                let _ = write!(out, "{x}");
            }
            out.push('"');
        }
        Value::Floats(f) => {
            const N3: [&str; 3] = ["x", "y", "z"];
            const N4: [&str; 4] = ["x", "y", "z", "w"];
            let names: &[&str] = if f.len() == 4 { &N4 } else { &N3[..f.len().min(3)] };
            out.push('{');
            for (i, (n, x)) in names.iter().zip(f.iter()).enumerate() {
                if i > 0 {
                    out.push(',');
                }
                let _ = write!(out, "\"{n}\":");
                write_f32(*x, out);
            }
            out.push('}');
        }
        Value::TimeSpan(t) => json::write_str(&fmt_timespan(*t), out),
        Value::Date(s) => json::write_str(s, out),
        Value::Seq(items) => {
            out.push('[');
            for (i, it) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(it, tags, out);
            }
            out.push(']');
        }
        Value::Dict(d) => write_dict(d, tags, out),
        Value::Fields(fs) => {
            out.push('{');
            for (i, (k, x)) in fs.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                json::write_str(k, out);
                out.push(':');
                write_value(x, tags, out);
            }
            out.push('}');
        }
        Value::Obj(o) => {
            out.push('{');
            let mut first = true;
            let mut sep = |out: &mut String| {
                if !first {
                    out.push(',');
                }
                first = false;
            };
            let write_id = |out: &mut String| {
                if let Some(id) = &o.id {
                    out.push_str("\"$id\":");
                    json::write_str(&id.text(), out);
                }
            };
            let write_tag = |out: &mut String| {
                if o.tag != NO_TAG {
                    out.push_str("\"$type\":");
                    let t = tags.get(o.tag as usize).map_or("?", |t| &**t);
                    json::write_str(t, out);
                }
            };
            if o.type_first {
                sep(out);
                write_tag(out);
                sep(out);
                write_id(out);
            } else {
                if o.id.is_some() {
                    sep(out);
                    write_id(out);
                }
                if o.tag != NO_TAG {
                    sep(out);
                    write_tag(out);
                }
            }
            let t = schema::ty(o.ty);
            for p in &o.props {
                sep(out);
                match &p.key {
                    Key::Member(i) => {
                        let name = t.members.get(usize::from(*i)).map_or("?", |m| m.json);
                        json::write_str(name, out);
                    }
                    Key::MemberCase(_, k) | Key::Unknown(k) => json::write_str(k, out),
                }
                out.push(':');
                write_value(&p.value, tags, out);
            }
            out.push('}');
        }
        Value::Ref(r) => {
            out.push_str("{\"$ref\":");
            json::write_str(&r.id.text(), out);
            out.push('}');
        }
        Value::Json(j) => json::write(j, out),
        Value::Coerced(c) => json::write(&c.1, out),
        Value::Opaque(o) => json::write(&o.json, out),
    }
}

fn write_dict(d: &Dict, tags: &[Box<str>], out: &mut String) {
    let pair = |k: &Value, v: &Value, out: &mut String| {
        out.push_str("{\"Key\":");
        write_value(k, tags, out);
        out.push_str(",\"Value\":");
        write_value(v, tags, out);
        out.push('}');
    };
    match d.form {
        DictForm::KeyValue => match d.entries.first() {
            Some(Entry::Pair(k, v)) => pair(k, v, out),
            Some(Entry::Raw(j, _) | Entry::Laid(_, _, j)) => json::write(j, out),
            None => out.push_str("{}"),
        },
        DictForm::Array => {
            out.push('[');
            for (i, e) in d.entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                match e {
                    Entry::Pair(k, v) => pair(k, v, out),
                    Entry::Raw(j, _) | Entry::Laid(_, _, j) => json::write(j, out),
                }
            }
            out.push(']');
        }
        DictForm::Object => {
            out.push('{');
            for (i, e) in d.entries.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                match e {
                    Entry::Pair(k, v) => {
                        json::write_str(&key_text(k), out);
                        out.push(':');
                        write_value(v, tags, out);
                    }
                    Entry::Raw(j, _) | Entry::Laid(_, _, j) => {
                        // Object-form entries are never raw; keep the JSON visible anyway.
                        json::write_str("$raw", out);
                        out.push(':');
                        json::write(j, out);
                    }
                }
            }
            out.push('}');
        }
    }
}

fn parse_num(s: &str) -> Option<f64> {
    match s {
        "NaN" => Some(f64::NAN),
        "Infinity" => Some(f64::INFINITY),
        "-Infinity" => Some(f64::NEG_INFINITY),
        _ => s.parse::<f64>().ok(),
    }
}

/// Exact JSON equality: same structure, keys in order, equal strings, identical number
/// lexemes.
fn exact(a: &Json, b: &Json, path: &str) -> Result<(), String> {
    if a == b { Ok(()) } else { Err(format!("{path}: {} vs {}", a.to_text(), b.to_text())) }
}

/// Checks that a decoded value says exactly what the original JSON said, under the
/// declared C# types: every value outside typed floats must match exactly (strings,
/// integers by lexeme, structure, key order); a typed `float` must be the original read
/// as a double and rounded to float, a typed `double` the original read as a double
/// (NaNs match NaNs). This is what makes the canonical text a faithful rewrite of the
/// original.
///
/// # Errors
///
/// The path of the first difference.
pub fn check_source(orig: &Json, v: &Value, tags: &[Box<str>]) -> Result<(), String> {
    let mut path = String::new();
    chk(orig, v, tags, &mut path)
}

fn float_token(o: &Json) -> Option<f64> {
    match o {
        // Newtonsoft reads an unsigned leading-zero number as octal: never a match.
        Json::Num(n)
            if n.len() > 1 && n.starts_with('0') && !n[1..].starts_with(['.', 'e', 'E']) =>
        {
            None
        }
        Json::Num(n) => parse_num(n),
        Json::Str(s) => parse_num(s.trim()),
        _ => None,
    }
}

#[allow(clippy::too_many_lines)]
fn chk(o: &Json, v: &Value, tags: &[Box<str>], path: &mut String) -> Result<(), String> {
    match v {
        Value::F32(x) => match float_token(o) {
            Some(y) if (x.is_nan() && y.is_nan()) || (y as f32).to_bits() == x.to_bits() => Ok(()),
            _ => Err(format!("{path}: float {x} vs {}", o.to_text())),
        },
        Value::F64(x) => match float_token(o) {
            Some(y) if (x.is_nan() && y.is_nan()) || y.to_bits() == x.to_bits() => Ok(()),
            _ => Err(format!("{path}: double {x} vs {}", o.to_text())),
        },
        Value::Floats(fs) => {
            let Json::Obj(p) = o else { return Err(format!("{path}: vector vs {}", o.to_text())) };
            let names = ["x", "y", "z", "w"];
            if p.len() != fs.len() {
                return Err(format!("{path}: vector length"));
            }
            for (i, ((k, jv), x)) in p.iter().zip(fs.iter()).enumerate() {
                if &**k != names[i.min(3)] {
                    return Err(format!("{path}: vector key {k}"));
                }
                chk(jv, &Value::F32(*x), tags, path)?;
            }
            Ok(())
        }
        Value::Seq(items) => {
            let Json::Arr(a) = o else { return Err(format!("{path}: array vs {}", o.to_text())) };
            if a.len() != items.len() {
                return Err(format!("{path}: array length {} vs {}", a.len(), items.len()));
            }
            for (i, (x, y)) in a.iter().zip(items.iter()).enumerate() {
                let len = path.len();
                let _ = write!(path, "[{i}]");
                chk(x, y, tags, path)?;
                path.truncate(len);
            }
            Ok(())
        }
        Value::Obj(ob) => {
            let Json::Obj(p) = o else { return Err(format!("{path}: object vs {}", o.to_text())) };
            let mut expect: Vec<(String, Option<&Value>, Option<String>)> = Vec::new();
            let id = ob.id.as_ref().map(|i| ("$id".to_owned(), None, Some(i.text().into_owned())));
            let tag = (ob.tag != NO_TAG).then(|| {
                (
                    "$type".to_owned(),
                    None,
                    Some(tags.get(ob.tag as usize).map_or("?", |t| &**t).to_owned()),
                )
            });
            if ob.type_first {
                expect.extend(tag);
                expect.extend(id);
            } else {
                expect.extend(id);
                expect.extend(tag);
            }
            let t = schema::ty(ob.ty);
            for pr in &ob.props {
                let name = match &pr.key {
                    Key::Member(i) => {
                        t.members.get(usize::from(*i)).map_or("?", |m| m.json).to_owned()
                    }
                    Key::MemberCase(_, k) | Key::Unknown(k) => k.to_string(),
                };
                expect.push((name, Some(&pr.value), None));
            }
            if p.len() != expect.len() {
                return Err(format!("{path}: {} vs {} properties", p.len(), expect.len()));
            }
            for ((k, jv), (name, val, text)) in p.iter().zip(&expect) {
                if **k != **name {
                    return Err(format!("{path}: key {k} vs {name}"));
                }
                let len = path.len();
                let _ = write!(path, ".{k}");
                match (val, text) {
                    (Some(val), _) => chk(jv, val, tags, path)?,
                    (None, Some(t)) => exact(jv, &Json::Str(t.as_str().into()), path)?,
                    _ => {}
                }
                path.truncate(len);
            }
            Ok(())
        }
        Value::Fields(fs) => {
            let Json::Obj(p) = o else { return Err(format!("{path}: object vs {}", o.to_text())) };
            if p.len() != fs.len() {
                return Err(format!("{path}: field count"));
            }
            for ((k, jv), (name, val)) in p.iter().zip(fs.iter()) {
                if k != name {
                    return Err(format!("{path}: key {k} vs {name}"));
                }
                chk(jv, val, tags, path)?;
            }
            Ok(())
        }
        Value::Dict(d) => chk_dict(o, d, tags, path),
        Value::Coerced(c) => exact(o, &c.1, path),
        Value::Opaque(op) => exact(o, &op.json, path),
        Value::Json(j) => exact(o, j, path),
        // Scalars and references: their canonical text must parse to exactly the original.
        other => {
            let mut s = String::new();
            write_value(other, tags, &mut s);
            match json::parse(s.as_bytes()) {
                Ok(d) => exact(o, &d.root, path),
                Err(e) => Err(format!("{path}: canonical text {s} doesn't parse: {e}")),
            }
        }
    }
}

fn chk_dict(o: &Json, d: &Dict, tags: &[Box<str>], path: &mut String) -> Result<(), String> {
    let pair = |o: &Json, k: &Value, v: &Value, path: &mut String| -> Result<(), String> {
        let Json::Obj(p) = o else { return Err(format!("{path}: entry vs {}", o.to_text())) };
        if p.len() != 2 || &*p[0].0 != "Key" || &*p[1].0 != "Value" {
            return Err(format!("{path}: entry layout"));
        }
        chk(&p[0].1, k, tags, path)?;
        chk(&p[1].1, v, tags, path)
    };
    match d.form {
        DictForm::KeyValue => match d.entries.first() {
            Some(Entry::Pair(k, v)) => pair(o, k, v, path),
            Some(Entry::Raw(j, _) | Entry::Laid(_, _, j)) => exact(o, j, path),
            None => Err(format!("{path}: empty KeyValuePair")),
        },
        DictForm::Array => {
            let Json::Arr(a) = o else { return Err(format!("{path}: array vs {}", o.to_text())) };
            if a.len() != d.entries.len() {
                return Err(format!("{path}: entry count"));
            }
            for (i, (x, e)) in a.iter().zip(&d.entries).enumerate() {
                let len = path.len();
                let _ = write!(path, "[{i}]");
                match e {
                    Entry::Pair(k, v) => pair(x, k, v, path)?,
                    Entry::Raw(j, _) => exact(x, j, path)?,
                    Entry::Laid(k, v, j) => {
                        exact(x, j, path)?;
                        // The decoded pair must say what the element's `Key` and `Value`
                        // (by name) said.
                        let kj = x.get("Key").ok_or_else(|| format!("{path}: no Key"))?;
                        chk(kj, k, tags, path)?;
                        match x.get("Value") {
                            Some(vj) => chk(vj, v, tags, path)?,
                            None if matches!(v, Value::Null) => {}
                            None => return Err(format!("{path}: value without Value")),
                        }
                    }
                }
                path.truncate(len);
            }
            Ok(())
        }
        DictForm::Object => {
            let Json::Obj(p) = o else { return Err(format!("{path}: object vs {}", o.to_text())) };
            if p.len() != d.entries.len() {
                return Err(format!("{path}: entry count"));
            }
            for ((k, jv), e) in p.iter().zip(&d.entries) {
                let Entry::Pair(kv, vv) = e else {
                    return Err(format!("{path}: raw object entry"));
                };
                if **k != *key_text(kv) {
                    return Err(format!("{path}: key {k} vs {}", key_text(kv)));
                }
                let len = path.len();
                let _ = write!(path, ".{k}");
                chk(jv, vv, tags, path)?;
                path.truncate(len);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_check_is_type_aware() {
        let j = |s: &str| json::parse(s.as_bytes()).unwrap().root;
        // A float matches when the original, read as a double and rounded, is the value.
        assert!(check_source(&j("-161.616272"), &Value::F32(-161.616_27), &[]).is_ok());
        assert!(check_source(&j("\"NaN\""), &Value::F32(f32::NAN), &[]).is_ok());
        // Integers must match exactly, even where a float would round them together.
        assert!(check_source(&j("16777217"), &Value::Int(16_777_216), &[]).is_err());
        assert!(check_source(&j("16777217"), &Value::F32(16_777_216.0), &[]).is_ok());
        assert!(check_source(&j("16777217"), &Value::F64(16_777_216.0), &[]).is_err());
        let e = check_source(
            &j("[1,\"x\"]"),
            &Value::Seq(vec![Value::Int(1), Value::Str("y".into())].into()),
            &[],
        )
        .unwrap_err();
        assert!(e.starts_with("[1]"), "{e}");
    }
}
