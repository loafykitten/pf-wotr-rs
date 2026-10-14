//! The run's T0 event trace, in WrathOracle's trace vocabulary (`docs/poc1/oracle.md`,
//! schema `wrath-oracle-trace-v1`) so a sim run and an oracle recording can be diffed
//! record by record: `input`, `dlg_*` (dialogue transitions), `mut` (narrative and stat
//! mutations), `rule`/`rule_end` (rules), `draw` (random draws), plus `t1` records for
//! timing-coupled effects the sim stubs (cutscene starts, animations, sounds) and `stub`
//! records for presentation the sim leaves out.
//!
//! Records carry `t`, the sim tick in the run (the dialogue controller's ticks), and a
//! `seq`. Values follow the state dump's conventions: GUIDs as 32 lowercase hex digits,
//! units as `{"$unit": id}`.

use serde::Serialize;
use serde_json::{Map, Value, json};

/// One trace record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Event {
    pub seq: u64,
    pub k: &'static str,
    pub t: i64,
    /// Random draws made before this record (its position among the draws).
    pub d: usize,
    #[serde(flatten)]
    pub fields: Map<String, Value>,
}

/// The run's trace.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EventLog {
    pub records: Vec<Event>,
    /// The current tick (set by the runner).
    pub tick: i64,
    /// Whether to record (off for hydration-only runs).
    pub on: bool,
    /// The count of random draws so far, shared with [`crate::rng::Counted`].
    #[serde(skip)]
    pub draws: std::rc::Rc<std::cell::Cell<usize>>,
}

impl EventLog {
    /// Appends a record (when on). `fields` must be a JSON object.
    pub fn push(&mut self, k: &'static str, fields: Value) {
        if !self.on {
            return;
        }
        let seq = self.records.len() as u64;
        let fields = match fields {
            Value::Object(m) => m,
            Value::Null => Map::new(),
            other => {
                let mut m = Map::new();
                m.insert("value".into(), other);
                m
            }
        };
        self.records.push(Event { seq, k, t: self.tick, d: self.draws.get(), fields });
    }

    /// A `mut` record: `{"what": what, …fields}`.
    pub fn mutation(&mut self, what: &str, fields: Value) {
        let mut m = match fields {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        m.insert("what".into(), json!(what));
        self.push("mut", Value::Object(m));
    }
}

/// A unit reference in the trace vocabulary.
#[must_use]
pub fn unit_ref(id: &str) -> Value {
    json!({ "$unit": id })
}
