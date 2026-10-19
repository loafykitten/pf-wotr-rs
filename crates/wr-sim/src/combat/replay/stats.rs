//! The stat mutations the T0 stream compares (`docs/poc5/stats.md`): `stat_value`, `stat_mod_add`,
//! `stat_mod_remove` and the temporary scopes' records. The sim writes them from its own port of
//! `ModifiableValue` (`stats::ops`, at the points WrathOracle's hooks fire), so they come with
//! whatever produces a stat change: the helpless condition, a buff's components, the AC's
//! dexterity bonus. `combat::project` keeps them on both sides, so the stream comparison already
//! fails on a missing, extra, changed or misplaced one (its frame is part of the record).
//!
//! This module does the accounting around that comparison, and nothing else:
//! - **Gaps.** A recorded mutation of a stat the sim has nowhere to write (its owner is not a unit
//!   the sim made, or not a unit at all) is left out of the stream and counted by name. No other
//!   gap exists: a producer the sim doesn't port shows as a difference (the recorded mutations
//!   missing from the sim's stream), never as a gap.
//! - **Counts.** The recorded mutations compared, by kind, and how many have an equal sim record
//!   at the same place among the stat mutations; the rest are differences, which make the run
//!   invalid as well as the stream unequal.

use std::collections::BTreeMap;

use serde_json::Value as J;

use super::project::is_stat_mut;
use crate::world::World;

/// What the replay compared of the stat mutations (the summary's line).
#[derive(Debug, Default, Clone)]
pub struct StatCounts {
    /// The recording's stat mutations in the compared stream, by kind.
    pub compared: BTreeMap<String, usize>,
    /// Of them, the ones equal to the sim's record at the same place among the stat mutations.
    pub equal: usize,
    /// Recorded ones whose place holds another sim record or none.
    pub differing: usize,
    /// The sim's records beyond the recorded ones.
    pub extra: usize,
    /// Recorded mutations left out of the stream, by the reason their producer is a gap.
    pub gaps: BTreeMap<&'static str, usize>,
}

fn is_stat(r: &J) -> bool {
    r["k"] == "mut" && r["what"].as_str().is_some_and(is_stat_mut)
}

/// Why the sim couldn't write the recorded mutation, if so: the owner's id.
fn gap(w: &World, r: &J) -> Option<&'static str> {
    match r["owner"].as_str() {
        None => Some("the stat's owner is not a unit"),
        Some(id) if !w.by_id.contains_key(id) => {
            Some("the stat's owner is a unit the sim hasn't made")
        }
        Some(_) => None,
    }
}

/// Removes the gaps from the recorded stream, then counts the two streams' stat mutations against
/// each other.
pub(super) fn settle(w: &World, oracle: &mut Vec<J>, sim: &[J]) -> StatCounts {
    let mut c = StatCounts::default();
    oracle.retain(|r| {
        let g = if is_stat(r) { gap(w, r) } else { None };
        if let Some(g) = g {
            *c.gaps.entry(g).or_default() += 1;
        }
        g.is_none()
    });
    let rec: Vec<&J> = oracle.iter().filter(|r| is_stat(r)).collect();
    let got: Vec<&J> = sim.iter().filter(|r| is_stat(r)).collect();
    for (i, r) in rec.iter().enumerate() {
        *c.compared.entry(r["what"].as_str().unwrap_or("?").to_owned()).or_default() += 1;
        if got.get(i) == Some(r) {
            c.equal += 1;
        } else {
            c.differing += 1;
        }
    }
    c.extra = got.len().saturating_sub(rec.len());
    c
}

impl StatCounts {
    /// Whether the sim's stat mutations are the recording's.
    #[must_use]
    pub fn agrees(&self) -> bool {
        self.differing == 0 && self.extra == 0
    }
}
