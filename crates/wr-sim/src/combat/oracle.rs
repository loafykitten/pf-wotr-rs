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

pub mod rv;
pub use rv::{Obj, Rv};

/// A fixture trace in memory.
#[derive(Debug)]
pub struct Trace {
    /// The records, as compact [`Rv`]s (they read like `serde_json::Value`s).
    pub records: Vec<Rv>,
    /// Call-site id → (key, class `sim`/`pres`).
    pub sites: HashMap<i64, (String, String)>,
}

impl Trace {
    /// Reads `*.trace.jsonl` or `*.trace.jsonl.gz` (through the system's `gzip -dc`), a line at a
    /// time.
    ///
    /// # Errors
    ///
    /// I/O, a bad record, or a trace without a `begin` record.
    pub fn load(path: &Path) -> Result<Self> {
        let mut interner = rv::Interner::default();
        let mut records = Vec::new();
        let mut read = |input: &mut dyn std::io::BufRead| -> Result<()> {
            let mut line = String::new();
            let mut n = 0;
            loop {
                line.clear();
                if input.read_line(&mut line)? == 0 {
                    return Ok(());
                }
                n += 1;
                // As `str::lines`: the line feed, then a carriage return before it.
                let l = line
                    .strip_suffix('\n')
                    .map_or(line.as_str(), |l| l.strip_suffix('\r').unwrap_or(l));
                if l.trim().is_empty() {
                    continue;
                }
                records.push(
                    interner.parse(l).with_context(|| format!("{}: line {n}", path.display()))?,
                );
            }
        };
        if path.extension().is_some_and(|e| e == "gz") {
            let mut child = std::process::Command::new("gzip")
                .arg("-dc")
                .arg(path)
                .stdout(std::process::Stdio::piped())
                .spawn()
                .with_context(|| format!("gzip -dc {}", path.display()))?;
            let stdout = child.stdout.take().context("gzip's output")?;
            let res = read(&mut std::io::BufReader::with_capacity(1 << 20, stdout));
            if res.is_err() {
                let _ = child.kill();
            }
            let status = child.wait().with_context(|| format!("gzip -dc {}", path.display()))?;
            res?;
            if !status.success() {
                bail!("gzip -dc {} failed", path.display());
            }
        } else {
            read(&mut std::io::BufReader::new(std::fs::File::open(path)?))?;
        }
        drop(interner);
        records.shrink_to_fit();
        Self::with_records(records)
    }

    /// A trace of `records` (the call-site table from their `begin` record).
    ///
    /// # Errors
    ///
    /// No `begin` record.
    pub fn with_records(records: Vec<Rv>) -> Result<Self> {
        let begin = records.iter().find(|r| r["k"] == "begin").context("no begin record")?;
        let mut sites = HashMap::new();
        for e in begin["rng_sites"].as_array().into_iter().flatten() {
            if let (Some(id), Some(key)) = (e[0].as_i64(), e[1].as_str()) {
                sites.insert(id, (key.to_owned(), e[2].as_str().unwrap_or("sim").to_owned()));
            }
        }
        Ok(Self { records, sites })
    }

    /// The records as `serde_json::Value`s.
    #[must_use]
    pub fn to_json(&self) -> Vec<J> {
        self.records.iter().map(Rv::to_json).collect()
    }

    /// A trace of `records` with this one's call-site table.
    #[must_use]
    pub fn from_json(&self, records: &[J]) -> Self {
        let mut interner = rv::Interner::default();
        Self {
            records: records.iter().map(|r| interner.from_json(r)).collect(),
            sites: self.sites.clone(),
        }
    }

    /// A copy of this trace with its records (as `serde_json::Value`s) changed by `change`, and
    /// this one's call-site table.
    #[must_use]
    pub fn edited(&self, change: impl FnOnce(&mut Vec<J>)) -> Self {
        let mut v = self.to_json();
        change(&mut v);
        self.from_json(&v)
    }

    /// The call-site key of a `draw` record (`key` for global-hook draws).
    #[must_use]
    pub fn site_key(&self, r: &Rv) -> Option<String> {
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
                        .and_then(Rv::as_i64)
                        .and_then(|v| i32::try_from(v).ok())
                        .unwrap_or(0),
                    max: a
                        .get(1)
                        .and_then(Rv::as_i64)
                        .and_then(|v| i32::try_from(v).ok())
                        .unwrap_or(0),
                },
                (Some("Range_ff"), Some(a)) => {
                    DrawKind::RangeFloat { min: f32_of(a.first()), max: f32_of(a.get(1)) }
                }
                (Some("value"), _) => DrawKind::Value,
                // `insideUnitCircle` (roaming's `RollNextPoint`): its `x`, `y`.
                (Some("insideUnitCircle"), _)
                    if r["v"].as_array().is_some_and(|a| a.len() == 2) =>
                {
                    let a = r["v"].as_array().expect("an array");
                    per.entry(key).or_default().push_back(TapeDraw {
                        kind: Some(DrawKind::InsideUnitCircle),
                        value: DrawValue::Vec2([f32_of(a.first()), f32_of(a.get(1))]),
                        in_rule: r["rule"].as_i64().unwrap_or(0) != 0,
                    });
                    continue;
                }
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
fn f32_of(v: Option<&Rv>) -> f32 {
    match v {
        Some(Rv::Number(n)) => {
            #[allow(clippy::cast_possible_truncation)]
            let f = n.as_f64().unwrap_or(0.0) as f32;
            f
        }
        Some(Rv::String(s)) => match s.as_str() {
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

    fn inside_unit_circle(&mut self, site: CallSite) -> [f32; 2] {
        match self.next(site, DrawKind::InsideUnitCircle) {
            Some(DrawValue::Vec2(v)) if crate::rng::in_unit_disc(v) => v,
            Some(_) => {
                let seq = self.log.len() as u64;
                self.failures.push(RngFailure::ValueOutOfRange { seq, site: site.0.into() });
                [0.0; 2]
            }
            None => [0.0; 2],
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
