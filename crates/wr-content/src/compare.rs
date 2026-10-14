//! Zip versus `.bbp`: every blueprint decoded from both, written in canonical form, and
//! compared as text. Asset references are compared by presence only until the pack's
//! asset list (`BlueprintReferencedAssets`) is decoded.

use std::collections::HashMap;

use serde::Serialize;

use crate::bbp::Bbp;
use crate::canon::{self, Assets};
use crate::decode::{self, Tally};
use crate::guid::Guid;
use crate::schema;
use crate::source::BlueprintZip;

#[derive(Debug, Default, Serialize)]
pub struct Comparison {
    pub zip_blueprints: usize,
    pub bbp_records: usize,
    pub bbp_empty_offsets: usize,
    pub only_in_zip: Vec<String>,
    pub only_in_bbp: Vec<String>,
    pub bbp_decode_errors: usize,
    pub bbp_error_examples: Vec<(String, String)>,
    /// Records whose decode didn't end where the next record starts.
    pub bbp_length_mismatches: usize,
    pub compared: usize,
    pub equal: usize,
    pub different: usize,
    /// The pack's `name` differs from the zip file name.
    pub name_mismatches: usize,
    pub name_examples: Vec<(String, String)>,
    /// (root type, differing field key) → blueprints with that difference.
    pub first_diffs: Vec<(String, String, u64)>,
    pub diff_examples: Vec<(String, String, String, String)>,
    pub seconds: f64,
}

fn file_stem(path: &str) -> &str {
    let base = path.rsplit('/').next().unwrap_or(path);
    base.strip_suffix(".jbp").unwrap_or(base)
}

fn first_diff(a: &str, b: &str) -> (String, String) {
    for (x, y) in a.lines().zip(b.lines()) {
        if x != y {
            return (x.trim().to_owned(), y.trim().to_owned());
        }
    }
    (format!("{} lines", a.lines().count()), format!("{} lines", b.lines().count()))
}

#[derive(Default)]
struct Acc {
    compared: usize,
    equal: usize,
    errors: Vec<(String, String)>,
    names: Vec<(String, String)>,
    diffs: HashMap<(String, String), u64>,
    examples: Vec<(String, String, String, String)>,
    seen: Vec<Guid>,
}

/// Compares every blueprint in the zip with its `.bbp` record.
#[must_use]
pub fn compare(zip: &BlueprintZip, bbp: &Bbp) -> Comparison {
    let t = std::time::Instant::now();
    let by_guid: HashMap<Guid, u32> = bbp.toc.iter().copied().collect();
    // Record lengths from the offsets (file order), to check each decode consumed exactly
    // its record.
    let mut offs: Vec<u32> = bbp.toc.iter().map(|x| x.1).filter(|&o| o != 0).collect();
    offs.sort_unstable();
    let next_of: HashMap<u32, u32> = offs
        .windows(2)
        .map(|w| (w[0], w[1]))
        .chain(offs.last().map(|&l| (l, u32::try_from(bbp.len()).unwrap_or(u32::MAX))))
        .collect();
    let parts = zip.par_fold(Acc::default, |acc, _pos, path, bytes| {
        let Ok(bytes) = bytes else { return };
        let mut tally = Tally::default();
        let Ok(d) = decode::decode_jbp(&bytes, &mut tally) else { return };
        let (Some(guid), Some(data)) = (d.guid, decode::data(&d.wrapper)) else { return };
        acc.seen.push(guid);
        let Some(&off) = by_guid.get(&guid) else { return };
        if off == 0 {
            return;
        }
        let rec = match bbp.decode(off) {
            Ok(r) => r,
            Err(e) => {
                acc.errors.push((guid.to_string(), format!("{e:#}")));
                return;
            }
        };
        if next_of.get(&off).is_some_and(|&n| n as usize != off as usize + rec.len) {
            acc.errors.push((guid.to_string(), "record length mismatch".into()));
        }
        let stem = file_stem(path);
        if rec.name != stem {
            acc.names.push((guid.to_string(), format!("zip {stem:?} vs pack {:?}", rec.name)));
        }
        let Some(bdata) = rec.data else { return };
        let a = canon::blueprint_with(
            guid,
            stem,
            data,
            canon::Options { assets: Assets::Presence, source: canon::Source::Json },
        );
        let b = canon::blueprint_with(
            guid,
            stem,
            &bdata,
            canon::Options { assets: Assets::Presence, source: canon::Source::Pack },
        );
        acc.compared += 1;
        if a == b {
            acc.equal += 1;
        } else {
            let root = schema::ty(data.ty).name.to_owned();
            let mut keys = std::collections::BTreeSet::new();
            let (la, lb): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
            if la.len() == lb.len() {
                for (x, y) in la.iter().zip(&lb) {
                    if x != y {
                        keys.insert(x.trim().split(':').next().unwrap_or("").to_owned());
                    }
                }
            } else {
                let (x, _) = first_diff(&a, &b);
                keys.insert(format!("{} (structure)", x.split(':').next().unwrap_or("")));
            }
            for k in keys {
                *acc.diffs.entry((root.clone(), k)).or_default() += 1;
            }
            if acc.examples.len() < 3 {
                let (x, y) = first_diff(&a, &b);
                acc.examples.push((guid.to_string(), root, x, y));
            }
        }
    });
    let mut c = Comparison {
        zip_blueprints: zip.len(),
        bbp_records: bbp.toc.len(),
        bbp_empty_offsets: bbp.toc.iter().filter(|x| x.1 == 0).count(),
        ..Comparison::default()
    };
    let mut seen = std::collections::HashSet::new();
    let mut diffs: HashMap<(String, String), u64> = HashMap::new();
    for a in parts {
        c.compared += a.compared;
        c.equal += a.equal;
        c.bbp_length_mismatches +=
            a.errors.iter().filter(|e| e.1 == "record length mismatch").count();
        c.bbp_decode_errors += a.errors.iter().filter(|e| e.1 != "record length mismatch").count();
        for e in a.errors {
            if c.bbp_error_examples.len() < 20 {
                c.bbp_error_examples.push(e);
            }
        }
        c.name_mismatches += a.names.len();
        for n in a.names {
            if c.name_examples.len() < 20 {
                c.name_examples.push(n);
            }
        }
        for (k, v) in a.diffs {
            *diffs.entry(k).or_default() += v;
        }
        for e in a.examples {
            if c.diff_examples.len() < 40 {
                c.diff_examples.push(e);
            }
        }
        seen.extend(a.seen);
    }
    c.different = c.compared - c.equal;
    let mut fd: Vec<(String, String, u64)> =
        diffs.into_iter().map(|((r, k), v)| (r, k, v)).collect();
    fd.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    fd.truncate(100);
    c.first_diffs = fd;
    let toc: std::collections::HashSet<Guid> = bbp.toc.iter().map(|x| x.0).collect();
    c.only_in_zip = seen.difference(&toc).take(50).map(ToString::to_string).collect();
    c.only_in_bbp = toc.difference(&seen).take(50).map(ToString::to_string).collect();
    c.seconds = t.elapsed().as_secs_f64();
    c
}
