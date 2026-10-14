//! Coverage, reference and preservation reports for one save.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde_json::{Value as J, json};

use crate::decode::{BpUse, Stats};
use crate::zks::{self, Body, Decoded, RawEntry};
use crate::{canon, format, json, schema};

/// Where reports and our files go (`cache/saves`, gitignored: save contents are game data).
#[must_use]
pub fn cache_dir() -> PathBuf {
    wr_paths::cache_root().join("saves")
}

/// Resolves a save argument: a path, or a name prefix in the fixture backups, then in
/// the game's save folder (read-only).
///
/// # Errors
///
/// No match.
pub fn find_save(arg: &str) -> Result<PathBuf> {
    let p = Path::new(arg);
    if p.is_file() {
        return Ok(p.to_owned());
    }
    let dirs = [wr_paths::fixture_saves(), wr_paths::GamePaths::from_env().saves()];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut names: Vec<PathBuf> = rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "zks"))
            .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(arg)))
            .collect();
        names.sort();
        if let Some(n) = names.into_iter().next() {
            return Ok(n);
        }
    }
    anyhow::bail!("no save matching {arg}")
}

/// Every save in the game's folder (read-only).
///
/// # Errors
///
/// The folder can't be listed.
pub fn all_saves() -> Result<Vec<PathBuf>> {
    let d = wr_paths::GamePaths::from_env().saves();
    let mut v: Vec<PathBuf> = std::fs::read_dir(&d)
        .with_context(|| d.display().to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "zks"))
        .collect();
    v.sort();
    Ok(v)
}

/// Short name of a save file for reports (`Manual_13`).
#[must_use]
pub fn short_name(p: &Path) -> String {
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("save");
    let mut parts = stem.splitn(3, '_');
    match (parts.next(), parts.next()) {
        (Some(a), Some(b)) if b.chars().all(|c| c.is_ascii_digit()) => format!("{a}_{b}"),
        _ => stem.to_owned(),
    }
}

/// Merged statistics of a decoded save.
#[must_use]
pub fn merged(decoded: &[Decoded]) -> Stats {
    let mut s = Stats::default();
    for d in decoded {
        if let Some(st) = &d.stats {
            s.merge(st.clone());
        }
    }
    s
}

/// Blueprint references checked against the content pack.
#[derive(Debug, Default)]
pub struct BpTally {
    pub total: u64,
    pub distinct: u64,
    pub resolved: u64,
    pub dangling: u64,
    pub wrong_type: u64,
    /// Target type not in the content schema (can't type-check).
    pub unchecked: u64,
    /// Dangling or wrong-type converter reads outside dictionary entries: the game's
    /// `BlueprintConverter` throws on these.
    pub fatal: u64,
    pub dangling_examples: Vec<String>,
    pub wrong_examples: Vec<String>,
    pub unchecked_targets: BTreeMap<String, u64>,
}

/// Opens the content pack, if built and current.
fn open_pack() -> Option<wr_content::pack::Pack> {
    let path = wr_paths::cache_root().join("content/blueprints.wrcpack");
    if !path.is_file() {
        return None;
    }
    // SAFETY: the pack is written by `wr_content::pack::build` (unique name, read-only,
    // renamed into place) and nothing modifies it while we hold the mapping.
    unsafe { wr_content::pack::Pack::open(&path) }.ok()
}

/// Resolves blueprint GUIDs against `blueprints.zip` through the content pack.
#[must_use]
pub fn resolve_blueprints(uses: &[BpUse]) -> Option<BpTally> {
    let pack = open_pack()?;
    let names: HashMap<&str, wr_content::schema::TypeIdx> = wr_content::schema::types()
        .iter()
        .enumerate()
        .filter_map(|(i, t)| u32::try_from(i).ok().map(|i| (t.name, i)))
        .collect();
    let mut t = BpTally { total: uses.len() as u64, ..BpTally::default() };
    let mut seen: HashSet<[u8; 16]> = HashSet::new();
    for u in uses {
        seen.insert(u.guid);
        let g = wr_content::Guid::from_bytes(u.guid);
        let target = schema::ty(u.target).clr;
        match pack.find(g) {
            None => {
                t.dangling += 1;
                if u.converter && !u.in_dict {
                    t.fatal += 1;
                }
                if t.dangling_examples.len() < 10 {
                    t.dangling_examples.push(format!("{g} as {target}"));
                }
            }
            Some(e) => match names.get(target) {
                None => {
                    t.unchecked += 1;
                    *t.unchecked_targets.entry(target.to_owned()).or_default() += 1;
                }
                Some(&want) if wr_content::schema::is_assignable(e.ty, want) => t.resolved += 1,
                Some(_) => {
                    t.wrong_type += 1;
                    if u.converter && !u.in_dict {
                        t.fatal += 1;
                    }
                    if t.wrong_examples.len() < 10 {
                        let actual = wr_content::schema::ty(e.ty).name;
                        t.wrong_examples.push(format!("{g}: {actual} as {target}"));
                    }
                }
            },
        }
    }
    t.distinct = seen.len() as u64;
    Some(t)
}

fn pct(a: u64, b: u64) -> f64 {
    if b == 0 { 100.0 } else { (a as f64) * 100.0 / (b as f64) }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// The coverage report of a decoded save.
#[must_use]
pub fn coverage(path: &Path, entries: &[RawEntry], decoded: &[Decoded]) -> J {
    let s = merged(decoded);
    let mut per_type: Vec<(&str, u64, u64, u64, u64)> = s
        .per_type
        .iter()
        .map(|(&t, st)| (schema::ty(t).clr, st.objects, st.clean, st.props, st.props_unknown))
        .collect();
    per_type.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let objects: u64 = per_type.iter().map(|x| x.1).sum();
    let clean: u64 = per_type.iter().map(|x| x.2).sum();
    // `$type`s seen (typed ones from the stats, unknown ones from the opaque reasons).
    let unknown_types: BTreeMap<&str, u64> = s
        .opaque
        .iter()
        .filter_map(|(k, (n, _))| k.strip_prefix("unknown $type ").map(|t| (t, *n)))
        .collect();
    let mut unknown_props: Vec<(String, u64)> = s
        .unknown_props
        .iter()
        .map(|((t, k), n)| (format!("{}.{k}", schema::ty(*t).clr), *n))
        .collect();
    unknown_props.sort_by_key(|a| std::cmp::Reverse(a.1));
    // Entity references within the save.
    let ids: HashSet<&str> = s.entity_ids.iter().map(|x| &**x).collect();
    let refs_out = s.entity_refs.iter().filter(|r| !r.is_empty() && !ids.contains(&***r)).count();
    let bp = resolve_blueprints(&s.bp_uses);
    let docs: Vec<J> = decoded
        .iter()
        .zip(entries)
        .map(|(d, e)| {
            let st = d.stats.as_ref();
            json!({
                "name": d.name,
                "bytes": e.bytes.len(),
                "root": d.root,
                "kind": match &d.body { Body::Doc(_) => "json", Body::Bytes(_) => "bytes" },
                "values_typed": st.map_or(0, |s| s.values_typed),
                "values_untyped": st.map_or(0, |s| s.values_untyped),
                "error": d.error,
            })
        })
        .collect();
    json!({
        "save": path.file_name().and_then(|n| n.to_str()),
        "entries": entries.len(),
        "json_documents": decoded.iter().filter(|d| matches!(d.body, Body::Doc(_))).count(),
        "parse_errors": decoded.iter().filter(|d| d.error.is_some()).count(),
        "pending_upgraders": zks::pending_upgraders(entries),
        "values": {
            "typed": s.values_typed,
            "untyped": s.values_untyped,
            "raw_by_declared_type": s.values_raw,
            "typed_pct": round2(pct(s.values_typed, s.values_typed + s.values_untyped)),
        },
        "objects": {
            "total": objects,
            "clean": clean,
            "clean_pct": round2(pct(clean, objects)),
            "types_seen": per_type.len() + unknown_types.len(),
            "types_known": per_type.len(),
            "types_unknown": unknown_types,
        },
        "opaque": s.opaque.iter().map(|(k, (n, v))| (k.clone(), json!({"count": n, "values": v}))).collect::<serde_json::Map<_, _>>(),
        "opaque_at": s.opaque_at,
        "coerced": s.coerced,
        "unknown_properties": unknown_props.iter().take(50).map(|(k, n)| json!([k, n])).collect::<Vec<_>>(),
        "unknown_property_total": unknown_props.iter().map(|x| x.1).sum::<u64>(),
        "skipped_properties": s.skipped_props,
        "skipped_at": s.skipped_at,
        "type_tags": { "distinct": s.tags.len(), "uses": s.tags.values().sum::<u64>() },
        "ids": s.ids,
        "refs": s.refs,
        "dictionary_entries_dropped": s.dict_dropped,
        "dictionary_repeated_keys": s.dict_repeats,
        "date_strings_in_dictionary_entries": s.date_strings_in_dict,
        "blueprint_refs": bp.as_ref().map(|b| json!({
            "total": b.total, "distinct": b.distinct, "resolved": b.resolved,
            "dangling": b.dangling, "wrong_type": b.wrong_type, "unchecked": b.unchecked,
            "fatal_in_game": b.fatal, "unchecked_targets": b.unchecked_targets,
            "dangling_examples": b.dangling_examples, "wrong_type_examples": b.wrong_examples,
        })),
        "entity_refs": {
            "total": s.entity_refs.len(),
            "entity_ids_defined": ids.len(),
            "not_defined_in_save": refs_out,
        },
        "per_type": per_type.iter().map(|(n, o, c, p, u)| json!({
            "type": n, "objects": o, "clean": c, "props": p, "unknown_props": u,
        })).collect::<Vec<_>>(),
        "documents": docs,
    })
}

/// Exact structural equality of two decoded documents, floats compared by bits (so a
/// preserved NaN equals itself): their serialized forms are identical.
#[must_use]
pub fn same(a: &crate::value::Doc, b: &crate::value::Doc) -> bool {
    match (postcard::to_allocvec(a), postcard::to_allocvec(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// Timings in milliseconds: min and median over `runs`.
fn time<T>(runs: usize, mut f: impl FnMut() -> T) -> (f64, f64, T) {
    let mut ts = Vec::with_capacity(runs);
    let mut last = None;
    for _ in 0..runs.max(1) {
        let t = Instant::now();
        let r = f();
        ts.push(t.elapsed().as_secs_f64() * 1000.0);
        last = Some(r);
    }
    ts.sort_by(f64::total_cmp);
    let r = last.unwrap_or_else(|| unreachable!("at least one run"));
    (round2(ts[0]), round2(ts[ts.len() / 2]), r)
}

/// Preservation and our format: original → decoded → our format → decoded, compared;
/// plus timings and sizes. Writes our file under `cache/saves/ours/`.
///
/// # Errors
///
/// I/O, or a snapshot that fails to decode.
#[allow(clippy::too_many_lines)]
pub fn roundtrip(path: &Path, runs: usize) -> Result<J> {
    let zks_bytes = std::fs::metadata(path)?.len();
    let (read_min, read_med, entries) = time(runs, || zks::read(path));
    let entries = entries?;
    zks::check_versions(&entries)?;
    let json_entries: Vec<&RawEntry> =
        entries.iter().filter(|e| e.name.ends_with(".json") && !json::is_blank(&e.bytes)).collect();
    let json_bytes: usize = json_entries.iter().map(|e| e.bytes.len()).sum();
    // Parsing only: our lossless parser and serde_json, both in parallel over documents.
    let (parse_min, parse_med, _) = time(runs, || {
        json_entries.par_iter().map(|e| json::parse(&e.bytes).is_ok()).filter(|x| *x).count()
    });
    let (sj_min, sj_med, _) = time(runs, || {
        json_entries
            .par_iter()
            .map(|e| {
                let b = e.bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&e.bytes);
                serde_json::from_slice::<serde_json::Value>(b).is_ok()
            })
            .filter(|x| *x)
            .count()
    });
    let (sj1_min, _, _) = time(runs.min(3), || {
        json_entries
            .iter()
            .map(|e| {
                let b = e.bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&e.bytes);
                serde_json::from_slice::<serde_json::Value>(b).is_ok()
            })
            .filter(|x| *x)
            .count()
    });
    // Full decode (parse + typed decode), parallel and on one thread.
    let (dec_min, dec_med, decoded) = time(runs, || zks::decode_all(&entries));
    let (dec1_min, _, _) = time(runs.min(3), || {
        entries.iter().map(zks::decode_entry).filter(|d| d.error.is_none()).count()
    });
    // Source equivalence: canonical text of each decoded document vs the original.
    let mut source_failures: Vec<String> = Vec::new();
    let canon_orig: Vec<Option<String>> = decoded
        .par_iter()
        .map(|d| match &d.body {
            Body::Doc(doc) => Some(canon::doc_text(doc)),
            Body::Bytes(_) => None,
        })
        .collect();
    let checks: Vec<Option<String>> = decoded
        .par_iter()
        .zip(&entries)
        .zip(&canon_orig)
        .map(|((d, e), c)| {
            let c = c.as_ref()?;
            let a = json::parse(&e.bytes).ok()?;
            let b = match json::parse(c.as_bytes()) {
                Ok(b) => b,
                Err(err) => {
                    return Some(format!("{}: canonical text doesn't parse: {err}", d.name));
                }
            };
            let Body::Doc(doc) = &d.body else { return None };
            if a.bom != b.bom || a.bom != doc.bom {
                return Some(format!("{}: BOM differs", d.name));
            }
            if a.root.count() != b.root.count() {
                return Some(format!("{}: canonical text has a different shape", d.name));
            }
            canon::check_source(&a.root, &doc.root, &doc.tags)
                .err()
                .map(|err| format!("{}: {err}", d.name))
        })
        .collect();
    source_failures.extend(checks.into_iter().flatten());
    // Our format.
    let items: Vec<format::Item> = decoded
        .iter()
        .map(|d| format::Item { name: d.name.clone(), body: d.body.clone() })
        .collect();
    let (enc_min, enc_med, ours) = time(runs, || format::encode(&items));
    let ours = ours?;
    let out_dir = cache_dir().join("ours");
    std::fs::create_dir_all(&out_dir)?;
    let out_path = out_dir.join(format!("{}.wrsave", short_name(path)));
    std::fs::write(&out_path, &ours)?;
    let (od_min, od_med, back) = time(runs, || format::decode(&ours));
    let back = back?;
    let (od1_min, _, _) = time(runs.min(3), || -> Result<usize> {
        let idx = format::index(&ours)?;
        let mut n = 0;
        for e in &idx {
            format::read_entry(&ours, e)?;
            n += 1;
        }
        Ok(n)
    });
    let (enc1_min, _, _) = time(runs.min(3), || -> Result<usize> {
        let mut n = 0;
        for it in &items {
            let raw = postcard::to_allocvec(&it.body)?;
            n += zstd::bulk::compress(&raw, format::LEVEL)?.len();
        }
        Ok(n)
    });
    // Preservation: canonical texts byte-identical and values structurally equal.
    let mut canon_diffs = Vec::new();
    let mut struct_diffs = Vec::new();
    for ((orig, c0), b) in items.iter().zip(&canon_orig).zip(&back) {
        if orig.name != b.name {
            struct_diffs.push(format!("entry order: {} vs {}", orig.name, b.name));
            continue;
        }
        match (&orig.body, &b.body, c0) {
            (Body::Doc(x), Body::Doc(y), Some(c0)) => {
                if canon::doc_text(y) != *c0 {
                    canon_diffs.push(orig.name.clone());
                }
                if !same(x, y) {
                    struct_diffs.push(orig.name.clone());
                }
            }
            (Body::Bytes(x), Body::Bytes(y), _) => {
                if x != y {
                    struct_diffs.push(orig.name.clone());
                }
            }
            _ => struct_diffs.push(format!("{}: kind changed", orig.name)),
        }
    }
    // A cold-ish view: our file read from disk and decoded once.
    let (file_min, _, _) = time(runs, || -> Result<usize> {
        let b = std::fs::read(&out_path)?;
        Ok(format::decode(&b)?.len())
    });
    let doc_count = items.iter().filter(|i| matches!(i.body, Body::Doc(_))).count();
    Ok(json!({
        "save": path.file_name().and_then(|n| n.to_str()),
        "runs": runs,
        "sizes": {
            "zks_bytes": zks_bytes,
            "json_bytes": json_bytes,
            "entries": entries.len(),
            "json_documents": doc_count,
            "ours_bytes": ours.len(),
            "ours_vs_zks": round2(ours.len() as f64 / zks_bytes as f64),
        },
        "ms_min_median": {
            "zks_read_inflate": [read_min, read_med],
            "json_parse_lossless_parallel": [parse_min, parse_med],
            "json_parse_serde_value_parallel": [sj_min, sj_med],
            "json_parse_serde_value_single_thread_min": sj1_min,
            "decode_typed_parallel_incl_parse": [dec_min, dec_med],
            "decode_typed_single_thread_min": dec1_min,
            "ours_encode": [enc_min, enc_med],
            "ours_decode": [od_min, od_med],
            "ours_decode_single_thread_min": od1_min,
            "ours_encode_single_thread_min": enc1_min,
            "ours_read_file_and_decode_min": file_min,
        },
        "source_equivalence": {
            "documents_checked": canon_orig.iter().filter(|c| c.is_some()).count(),
            "failures": source_failures,
        },
        "preservation": {
            "canonical_text_differences": canon_diffs,
            "structural_differences": struct_diffs,
            "pass": canon_diffs.is_empty() && struct_diffs.is_empty() && source_failures.is_empty(),
        },
        "ours_path": out_path.display().to_string(),
    }))
}

/// Writes a JSON report under `cache/saves/reports/`.
///
/// # Errors
///
/// I/O.
pub fn write_report(name: &str, v: &J) -> Result<PathBuf> {
    let dir = cache_dir().join("reports");
    std::fs::create_dir_all(&dir)?;
    let p = dir.join(name);
    std::fs::write(&p, serde_json::to_vec_pretty(v)?)?;
    Ok(p)
}

/// The long tail over several saves: types by object count, with cumulative shares.
#[must_use]
pub fn type_curve(per_type: &HashMap<schema::TypeIdx, crate::decode::TypeStats>) -> J {
    let mut v: Vec<(&str, u64)> =
        per_type.iter().map(|(&t, s)| (schema::ty(t).clr, s.objects)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let total: u64 = v.iter().map(|x| x.1).sum();
    let mut cum = 0u64;
    let mut marks = serde_json::Map::new();
    for (i, (_, n)) in v.iter().enumerate() {
        cum += n;
        let k = i + 1;
        if [1, 5, 10, 25, 50, 100, 200, 300, 400, 500].contains(&k) {
            marks.insert(format!("top_{k}"), json!(round2(pct(cum, total))));
        }
    }
    json!({
        "types": v.len(),
        "objects": total,
        "cumulative_pct": marks,
        "types_with_1_object": v.iter().filter(|x| x.1 == 1).count(),
        "types_with_under_10": v.iter().filter(|x| x.1 < 10).count(),
    })
}

fn category(flags: u32) -> &'static str {
    use crate::schema::type_flags as f;
    if flags & f::ENTITY != 0 {
        "entity"
    } else if flags & f::FACT != 0 {
        "fact"
    } else if flags & f::PART != 0 {
        "part"
    } else if flags & f::COMPONENT != 0 {
        "fact component state"
    } else {
        "other"
    }
}

/// What a hydrating loader needs for a save: concrete types ranked by the state they
/// carry (properties), with their lifecycle hooks; and, through the content pack, the
/// blueprint components behind the save's facts (the behaviour that must be ported for
/// facts to be live).
#[must_use]
pub fn hydration(decoded: &[Decoded]) -> J {
    let s = merged(decoded);
    let mut rows: Vec<_> = s
        .per_type
        .iter()
        .map(|(&t, st)| {
            let td = schema::ty(t);
            (st.props, st.objects, td.clr, category(td.flags), td.hook_names())
        })
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(b.2)));
    let total_props: u64 = rows.iter().map(|r| r.0).sum();
    let mut by_cat: BTreeMap<&str, (u64, u64, u64)> = BTreeMap::new();
    for r in &rows {
        let e = by_cat.entry(r.3).or_default();
        e.0 += 1;
        e.1 += r.1;
        e.2 += r.0;
    }
    let mut hooks: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
    for r in &rows {
        for h in &r.4 {
            let e = hooks.entry(h).or_default();
            e.0 += 1;
            e.1 += r.1;
        }
    }
    // Facts and their blueprints' components.
    let mut facts: Vec<([u8; 16], bool)> = Vec::new();
    for d in decoded {
        let Body::Doc(doc) = &d.body else { continue };
        doc.root.walk(&mut |v| {
            let Some(o) = v.as_obj() else { return };
            let td = schema::ty(o.ty);
            if !td.has(schema::type_flags::FACT) {
                return;
            }
            let mut guid = None;
            let mut active = false;
            for p in &o.props {
                let crate::value::Key::Member(i) = p.key else { continue };
                match td.members.get(usize::from(i)).map(|m| m.json) {
                    Some("Blueprint") => {
                        if let crate::value::Value::GuidN(g) | crate::value::Value::GuidD(g) =
                            p.value
                        {
                            guid = Some(g);
                        }
                    }
                    Some("IsActive") => active = matches!(p.value, crate::value::Value::Bool(true)),
                    _ => {}
                }
            }
            if let Some(g) = guid {
                facts.push((g, active));
            }
        });
    }
    let comps = open_pack().map(|pack| {
        let mut counts: BTreeMap<&'static str, (u64, u64)> = BTreeMap::new();
        let mut cache: HashMap<[u8; 16], Vec<&'static str>> = HashMap::new();
        let mut missing = 0u64;
        for (g, active) in &facts {
            let list = cache.entry(*g).or_insert_with(|| {
                let Some(e) = pack.find(wr_content::Guid::from_bytes(*g)) else {
                    return Vec::new();
                };
                let Ok(p) = pack.load(&e) else { return Vec::new() };
                let n = wr_content::pack::to_node(&p);
                let mut out = Vec::new();
                if let Some(o) = n.as_object()
                    && let Some(wr_content::Node::Seq(cs)) = o.get("Components")
                {
                    for c in cs {
                        if let Some(co) = c.as_object() {
                            out.push(wr_content::schema::ty(co.ty).short);
                        }
                    }
                }
                out
            });
            if list.is_empty() && pack.find(wr_content::Guid::from_bytes(*g)).is_none() {
                missing += 1;
            }
            for c in list.iter() {
                let e = counts.entry(c).or_default();
                e.0 += 1;
                e.1 += u64::from(*active);
            }
        }
        let mut v: Vec<_> = counts.into_iter().collect();
        v.sort_by(|a, b| b.1.1.cmp(&a.1.1).then(b.1.0.cmp(&a.1.0)));
        json!({
            "facts": facts.len(),
            "active_facts": facts.iter().filter(|f| f.1).count(),
            "facts_without_blueprint": missing,
            "component_types": v.len(),
            "components_on_active_facts": v.iter().map(|x| x.1.1).sum::<u64>(),
            "by_type": v.iter().map(|(n, (all, act))| json!([n, all, act])).collect::<Vec<_>>(),
        })
    });
    json!({
        "types": rows.len(),
        "properties": total_props,
        "by_category": by_cat.iter().map(|(k, (t, o, p))| json!({"category": k, "types": t, "objects": o, "properties": p})).collect::<Vec<_>>(),
        "hooks": hooks.iter().map(|(h, (t, o))| json!({"hook": h, "types": t, "objects": o})).collect::<Vec<_>>(),
        "ranked": rows.iter().map(|(p, o, n, c, h)| json!({
            "type": n, "category": c, "objects": o, "properties": p,
            "share_pct": round2(pct(*p, total_props)), "hooks": h,
        })).collect::<Vec<_>>(),
        "fact_components": comps,
    })
}
