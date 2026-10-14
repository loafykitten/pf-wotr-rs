//! `wr-oracle-canon`: compares the live game's blueprints (WrathOracle's `blueprints` step,
//! one canonical document per line, `docs/poc2/content.md`'s canonical form) with
//! `wr-content`'s canonical form of the same records decoded from `blueprints-pack.bbp`.
//!
//! usage: `wr-oracle-canon ORACLE.jsonl [--out REPORT.json] [--samples N]`
//! (gunzip WrathOracle's `.jsonl.gz` first). Numbers compare by their field's declared type
//! (from `wr-content`'s schema): `f32` fields by `f32` bits (the game writes floats with .NET's
//! "R", we write the shortest round-trip text), `f64` and integer fields exactly, anything
//! without a declared type exactly as `f64` (counted in the report); the strings `"NaN"`,
//! `"Infinity"`, `"-Infinity"` compare as strings. Objects compare key by key (a key on one side only is a difference), arrays
//! element by element.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde_json::Value;
use wr_content::bbp::Bbp;
use wr_content::canon::{self, Options, Source};
use wr_content::guid::Guid;
use wr_content::schema::{self, Codec, UnityKind};

/// How to compare the numbers under a value, from the field's declared type in the schema
/// (`wr-content`'s generated tables): `f32` fields by `f32` bits (the game writes .NET "R",
/// we write the shortest round-trip text; both name the same `f32`), `f64` fields exactly,
/// integers exactly. Without a declared type, exactly as `f64` (and counted).
#[derive(Clone, Copy)]
enum Hint {
    Unknown,
    F32,
    F64,
    Int,
    Codec(Codec),
}

fn hint_of(c: Codec) -> Hint {
    match c {
        Codec::F32 => Hint::F32,
        Codec::F64 | Codec::Decimal => Hint::F64,
        Codec::Int { .. } | Codec::Enum(_) | Codec::Char | Codec::Bool => Hint::Int,
        Codec::Unity(UnityKind::Vector2Int | UnityKind::Vector3Int | UnityKind::Color32) => {
            Hint::Int
        }
        Codec::Unity(_) => Hint::F32,
        Codec::Asset(_) | Codec::SharedString => Hint::Int,
        other => Hint::Codec(other),
    }
}

struct Cmp {
    out: Vec<String>,
    untyped_numbers: usize,
}

impl Cmp {
    fn number(
        &mut self,
        path: &str,
        a: &serde_json::Number,
        b: &serde_json::Number,
        hint: Hint,
    ) -> bool {
        if let (Some(x), Some(y)) = (a.as_i64(), b.as_i64()) {
            return x == y;
        }
        if let (Some(x), Some(y)) = (a.as_u64(), b.as_u64()) {
            return x == y;
        }
        let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) else { return false };
        match hint {
            #[allow(clippy::cast_possible_truncation)]
            Hint::F32 => (x as f32).to_bits() == (y as f32).to_bits(),
            Hint::F64 | Hint::Int => x.to_bits() == y.to_bits() || (x == 0.0 && y == 0.0),
            Hint::Unknown | Hint::Codec(_) => {
                self.untyped_numbers += 1;
                let _ = path;
                x.to_bits() == y.to_bits() || (x == 0.0 && y == 0.0)
            }
        }
    }

    fn diff(&mut self, path: &str, a: &Value, b: &Value, hint: Hint) {
        match (a, b) {
            (Value::Number(x), Value::Number(y)) => {
                if !self.number(path, x, y, hint) {
                    self.out.push(format!("{path}: game {a} vs ours {b}"));
                }
            }
            (Value::Object(x), Value::Object(y)) => {
                // The object's type: its "$type" if it carries one, else the field's declared type.
                let declared = match hint {
                    Hint::Codec(Codec::Object(t)) => Some(t),
                    _ => None,
                };
                let ty = x
                    .get("$type")
                    .and_then(Value::as_str)
                    .and_then(Guid::parse)
                    .and_then(schema::by_type_id)
                    .or(declared);
                let field_hint = |k: &str| -> Hint {
                    if let Some(f) = ty.and_then(|t| {
                        schema::ty(t).fields.iter().find(|f| f.name == k && f.in_runtime())
                    }) {
                        return hint_of(f.codec);
                    }
                    match hint {
                        // curves, gradients: all numbers are f32 (weightedMode/mode are small ints)
                        Hint::F32 => Hint::F32,
                        Hint::Int => Hint::Int,
                        _ => Hint::Unknown,
                    }
                };
                for (k, v) in x {
                    match y.get(k) {
                        Some(w) => {
                            let h = field_hint(k);
                            self.diff(&format!("{path}.{k}"), v, w, h);
                        }
                        None => self.out.push(format!("{path}.{k}: only in the game's dump")),
                    }
                }
                for k in y.keys() {
                    if !x.contains_key(k) {
                        self.out.push(format!("{path}.{k}: only in ours"));
                    }
                }
            }
            (Value::Array(x), Value::Array(y)) => {
                if x.len() != y.len() {
                    self.out.push(format!("{path}: length game {} vs ours {}", x.len(), y.len()));
                    return;
                }
                let elem = match hint {
                    Hint::Codec(Codec::Seq { elem, .. }) => hint_of(*elem),
                    h => h,
                };
                for (i, (v, w)) in x.iter().zip(y).enumerate() {
                    self.diff(&format!("{path}[{i}]"), v, w, elem);
                }
            }
            _ => {
                if a != b {
                    let s = |v: &Value| {
                        let t = v.to_string();
                        if t.len() > 120 { format!("{}…", &t[..120]) } else { t }
                    };
                    self.out.push(format!("{path}: game {} vs ours {}", s(a), s(b)));
                }
            }
        }
    }
}

/// `$.Components[3].m_Foo[2]` → `$.Components[].m_Foo[]`, for grouping.
fn pattern(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    let mut skip = false;
    for c in p.chars() {
        if c == '[' {
            out.push_str("[]");
            skip = true;
        } else if c == ']' {
            skip = false;
        } else if !skip {
            out.push(c);
        }
        if c == ':' {
            break;
        }
    }
    out.trim_end_matches(':').to_owned()
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let input = PathBuf::from(
        args.next()
            .context("usage: wr-oracle-canon ORACLE.jsonl [--out REPORT.json] [--samples N]")?,
    );
    let mut out_path = None;
    let mut samples = 20usize;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--out" => out_path = Some(PathBuf::from(args.next().context("--out needs a path")?)),
            "--samples" => samples = args.next().context("--samples needs N")?.parse()?,
            _ => bail!("unknown option {a}"),
        }
    }
    let paths = wr_paths::GamePaths::from_env();
    let pack = Bbp::open(&paths.bundles().join("blueprints-pack.bbp"))?;
    let toc: BTreeMap<u128, u32> = pack.toc.iter().map(|(g, o)| (g.0, *o)).collect();
    let opts = Options { source: Source::Pack, ..Options::default() };

    let (mut total, mut equal, mut missing, mut errors, mut untyped) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut by_type: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut by_pattern: BTreeMap<String, usize> = BTreeMap::new();
    let mut sample_diffs: Vec<Value> = Vec::new();
    for line in
        BufReader::new(File::open(&input).with_context(|| format!("open {}", input.display()))?)
            .lines()
    {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        total += 1;
        let game: Value = serde_json::from_str(&line).context("oracle line is not JSON")?;
        let guid_s = game.get("guid").and_then(Value::as_str).unwrap_or_default().to_owned();
        let ty = game.get("$type").and_then(Value::as_str).unwrap_or_default().to_owned();
        let entry = by_type.entry(ty).or_default();
        entry.0 += 1;
        let Some(g) = Guid::parse(&guid_s) else {
            errors += 1;
            continue;
        };
        let Some(&off) = toc.get(&g.0) else {
            missing += 1;
            continue;
        };
        let rec = match pack.decode(off) {
            Ok(r) => r,
            Err(e) => {
                errors += 1;
                eprintln!("{guid_s}: decode: {e:#}");
                continue;
            }
        };
        let Some(data) = rec.data else {
            missing += 1;
            continue;
        };
        let ours_text = canon::blueprint_with(g, &rec.name, &data, opts);
        let ours: Value =
            serde_json::from_str(&ours_text).context("our canonical text is not JSON")?;
        let mut cmp = Cmp { out: Vec::new(), untyped_numbers: 0 };
        let root = data.ty;
        cmp.diff("$", &game, &ours, Hint::Codec(Codec::Object(root)));
        untyped += cmp.untyped_numbers;
        let d = cmp.out;
        if d.is_empty() {
            equal += 1;
            entry.1 += 1;
        } else {
            for p in &d {
                *by_pattern.entry(pattern(p)).or_default() += 1;
            }
            if sample_diffs.len() < samples {
                sample_diffs.push(serde_json::json!({"guid": guid_s, "name": rec.name, "differences": d.iter().take(20).collect::<Vec<_>>()}));
            }
        }
    }
    let mut patterns: Vec<(String, usize)> = by_pattern.into_iter().collect();
    patterns.sort_by_key(|a| std::cmp::Reverse(a.1));
    let types_with_mismatch: Vec<Value> = by_type
        .iter()
        .filter(|(_, (n, e))| n != e)
        .map(|(t, (n, e))| serde_json::json!({"type": t, "compared": n, "equal": e}))
        .collect();
    let report = serde_json::json!({
        "input": input.display().to_string(),
        "compared": total,
        "equal": equal,
        "different": total - equal - missing - errors,
        "missing_in_pack": missing,
        "errors": errors,
        "numbers_compared_without_a_declared_type": untyped,
        "types": by_type.len(),
        "types_with_mismatch": types_with_mismatch,
        "difference_patterns": patterns.iter().take(100).map(|(p, n)| serde_json::json!([p, n])).collect::<Vec<_>>(),
        "samples": sample_diffs,
    });
    let text = serde_json::to_string_pretty(&report)?;
    if let Some(p) = out_path {
        std::fs::write(&p, &text)?;
    }
    println!("  numbers compared without a declared type (exact): {untyped}");
    println!(
        "{total} compared: {equal} equal, {} different, {missing} missing in the pack, {errors} errors; {} types, {} with a mismatch",
        total - equal - missing - errors,
        by_type.len(),
        report["types_with_mismatch"].as_array().map_or(0, Vec::len)
    );
    for (p, n) in patterns.iter().take(15) {
        println!("  {n:>7}  {p}");
    }
    Ok(())
}
