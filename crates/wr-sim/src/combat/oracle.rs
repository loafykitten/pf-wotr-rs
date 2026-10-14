//! Reading a WrathOracle fixture trace (`wrath-oracle-trace-v1`, `docs/poc1/oracle.md`):
//! the records, the call-site table from the `begin` record, and the replay tape per call
//! site ([`SiteTape`]).
//!
//! The oracle replays sim draws per call site (a site that drew a different number of
//! times can't shift the others), and so does the sim: a [`SiteTape`] gives each site its
//! own queue. Sites the sim models must use their whole queue (leftovers fail the run);
//! draws at sites the sim doesn't model (movement jitter, animation, camera…) stay on the
//! tape and are reported, never consumed by another site.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use serde_json::Value as J;

use crate::rng::{CallSite, Draw, DrawKind, DrawValue, Rng, RngFailure};

/// A fixture trace in memory.
#[derive(Debug)]
pub struct Trace {
    pub records: Vec<J>,
    /// Call-site id → (key, class `sim`/`pres`).
    pub sites: HashMap<i64, (String, String)>,
}

impl Trace {
    /// Reads `*.trace.jsonl` or `*.trace.jsonl.gz` (through the system's `gzip -dc`).
    ///
    /// # Errors
    ///
    /// I/O, a bad record, or a trace without a `begin` record.
    pub fn load(path: &Path) -> Result<Self> {
        let text = if path.extension().is_some_and(|e| e == "gz") {
            let out = std::process::Command::new("gzip")
                .arg("-dc")
                .arg(path)
                .output()
                .with_context(|| format!("gzip -dc {}", path.display()))?;
            if !out.status.success() {
                bail!("gzip -dc {} failed", path.display());
            }
            String::from_utf8(out.stdout)?
        } else {
            std::fs::read_to_string(path)?
        };
        let mut records = Vec::new();
        for (n, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            records.push(
                serde_json::from_str::<J>(line)
                    .with_context(|| format!("{}: line {}", path.display(), n + 1))?,
            );
        }
        let begin = records.iter().find(|r| r["k"] == "begin").context("no begin record")?;
        let mut sites = HashMap::new();
        for e in begin["rng_sites"].as_array().into_iter().flatten() {
            if let (Some(id), Some(key)) = (e[0].as_i64(), e[1].as_str()) {
                sites.insert(id, (key.to_owned(), e[2].as_str().unwrap_or("sim").to_owned()));
            }
        }
        Ok(Self { records, sites })
    }

    /// The call-site key of a `draw` record (`key` for global-hook draws).
    #[must_use]
    pub fn site_key(&self, r: &J) -> Option<String> {
        if let Some(k) = r["key"].as_str() {
            return Some(k.to_owned());
        }
        self.sites.get(&r["site"].as_i64()?).map(|s| s.0.clone())
    }

    /// The sim-class draws, per call site in recorded order: the replay tape.
    #[must_use]
    pub fn tape(&self) -> SiteTape {
        let mut per: BTreeMap<String, VecDeque<TapeDraw>> = BTreeMap::new();
        for r in &self.records {
            if r["k"] != "draw" || r["c"] == "pres" {
                continue;
            }
            let Some(key) = self.site_key(r) else { continue };
            let kind = match (r["m"].as_str(), r["a"].as_array()) {
                (Some("Range_ii"), Some(a)) => DrawKind::RangeInt {
                    min: a
                        .first()
                        .and_then(J::as_i64)
                        .and_then(|v| i32::try_from(v).ok())
                        .unwrap_or(0),
                    max: a
                        .get(1)
                        .and_then(J::as_i64)
                        .and_then(|v| i32::try_from(v).ok())
                        .unwrap_or(0),
                },
                (Some("Range_ff"), Some(a)) => {
                    DrawKind::RangeFloat { min: f32_of(a.first()), max: f32_of(a.get(1)) }
                }
                (Some("value"), _) => DrawKind::Value,
                _ => {
                    // Vector draws (`insideUnitCircle`…) aren't modelled: kept as opaque.
                    per.entry(key).or_default().push_back(TapeDraw {
                        kind: None,
                        value: DrawValue::Int(0),
                        in_rule: r["rule"].as_i64().unwrap_or(0) != 0,
                    });
                    continue;
                }
            };
            let value = match kind {
                DrawKind::RangeInt { .. } => {
                    DrawValue::Int(r["v"].as_i64().and_then(|v| i32::try_from(v).ok()).unwrap_or(0))
                }
                _ => DrawValue::Float(f32_of(Some(&r["v"]))),
            };
            per.entry(key).or_default().push_back(TapeDraw {
                kind: Some(kind),
                value,
                in_rule: r["rule"].as_i64().unwrap_or(0) != 0,
            });
        }
        SiteTape { per, used: BTreeMap::new(), log: Vec::new(), failures: Vec::new() }
    }
}

/// A JSON number (or the strings `NaN`/`Infinity`) parsed to `f32` (the oracle writes
/// floats round-trip).
fn f32_of(v: Option<&J>) -> f32 {
    match v {
        Some(J::Number(n)) => {
            #[allow(clippy::cast_possible_truncation)]
            let f = n.as_f64().unwrap_or(0.0) as f32;
            f
        }
        Some(J::String(s)) => match s.as_str() {
            "NaN" => f32::NAN,
            "Infinity" => f32::INFINITY,
            "-Infinity" => f32::NEG_INFINITY,
            _ => 0.0,
        },
        _ => 0.0,
    }
}

/// One recorded draw on a [`SiteTape`].
#[derive(Debug, Clone)]
pub struct TapeDraw {
    /// `None` for draws the tape can't type (vector members).
    pub kind: Option<DrawKind>,
    pub value: DrawValue,
    /// Made inside a rule in the recording.
    pub in_rule: bool,
}

/// A replay tape with one queue per call site.
#[derive(Debug, Clone)]
pub struct SiteTape {
    pub per: BTreeMap<String, VecDeque<TapeDraw>>,
    /// Draws consumed per site.
    pub used: BTreeMap<String, usize>,
    log: Vec<Draw>,
    failures: Vec<RngFailure>,
}

impl SiteTape {
    fn next(&mut self, site: CallSite, kind: DrawKind) -> Option<DrawValue> {
        let seq = self.log.len() as u64;
        let Some(q) = self.per.get_mut(site.0) else {
            self.failures.push(RngFailure::ExtraDraw { seq, site: site.0.into() });
            return None;
        };
        let Some(d) = q.pop_front() else {
            self.failures.push(RngFailure::ExtraDraw { seq, site: site.0.into() });
            return None;
        };
        if d.kind != Some(kind) {
            self.failures.push(RngFailure::KindMismatch { seq, site: site.0.into() });
            return None;
        }
        *self.used.entry(site.0.to_owned()).or_default() += 1;
        self.log.push(Draw { seq, site: site.0.into(), kind, value: d.value });
        Some(d.value)
    }

    /// Draws left per site (after a run).
    #[must_use]
    pub fn leftovers(&self) -> BTreeMap<String, usize> {
        self.per.iter().filter(|(_, q)| !q.is_empty()).map(|(k, q)| (k.clone(), q.len())).collect()
    }

    /// Ends the run for the sites in `modelled`: leftovers there are missing draws.
    pub fn finish_sites(&mut self, modelled: &[CallSite]) {
        for s in modelled {
            let left = self.per.get(s.0).map_or(0, VecDeque::len);
            if left > 0 {
                self.failures.push(RngFailure::MissingDraws {
                    from_seq: self.log.len() as u64,
                    count: left,
                });
            }
        }
    }
}

impl Rng for SiteTape {
    fn range_int(&mut self, site: CallSite, min: i32, max: i32) -> i32 {
        match self.next(site, DrawKind::RangeInt { min, max }) {
            Some(DrawValue::Int(v)) => v,
            _ => min,
        }
    }

    fn range_float(&mut self, site: CallSite, min: f32, max: f32) -> f32 {
        match self.next(site, DrawKind::RangeFloat { min, max }) {
            Some(DrawValue::Float(v)) => v,
            _ => min,
        }
    }

    fn value(&mut self, site: CallSite) -> f32 {
        match self.next(site, DrawKind::Value) {
            Some(DrawValue::Float(v)) => v,
            _ => 0.0,
        }
    }

    fn log(&self) -> &[Draw] {
        &self.log
    }

    fn failures(&self) -> &[RngFailure] {
        &self.failures
    }

    /// The generic finish: nothing (the caller says which sites it models with
    /// [`SiteTape::finish_sites`]).
    fn finish(&mut self) {}
}
