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
//!   at the same place among the stat mutations and between the same other records; the rest are differences, which make the run
//!   invalid as well as the stream unequal.

use std::collections::{BTreeMap, HashMap, HashSet};

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

/// Why the sim couldn't write the recorded mutation, if so. `units`: the ids of the units the sim
/// made during the run, disposed ones included (`World::by_id` forgets a unit at its disposal).
fn gap(units: &HashSet<&str>, r: &J) -> Option<&'static str> {
    match r["owner"].as_str() {
        None => Some("the stat's owner is not a unit"),
        Some(id) if !units.contains(id) => Some("the stat's owner is a unit the sim hasn't made"),
        Some(_) => None,
    }
}

/// A record other than a stat mutation, identified by its payload and its occurrence among the
/// equal ones (the projected stream repeats identical records: a rule and its result, run twice).
type Anchor<'a> = (&'a J, usize);

/// A stat mutation with its place in the stream: the nearest other (non-stat) record before it and
/// after it. A mutation moved across a fact attached, a damage or a repeated record differs in a
/// neighbour; a difference elsewhere in the stream is no stat mutation's (but for the occurrence
/// numbers of equal records after it).
type Placed<'a> = (&'a J, Option<Anchor<'a>>, Option<Anchor<'a>>);

fn placed(stream: &[J]) -> Vec<Placed<'_>> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut anchors: Vec<Option<Anchor<'_>>> = Vec::with_capacity(stream.len());
    for r in stream {
        anchors.push((!is_stat(r)).then(|| {
            let n = seen.entry(r.to_string()).or_default();
            *n += 1;
            (r, *n - 1)
        }));
    }
    let mut next: Vec<Option<Anchor<'_>>> = vec![None; stream.len()];
    let mut after = None;
    for i in (0..stream.len()).rev() {
        next[i] = after;
        if anchors[i].is_some() {
            after = anchors[i];
        }
    }
    let mut before = None;
    let mut out = Vec::new();
    for (i, r) in stream.iter().enumerate() {
        if anchors[i].is_some() {
            before = anchors[i];
        } else {
            out.push((r, before, next[i]));
        }
    }
    out
}

/// Removes the gaps from the recorded stream, then counts the two streams' stat mutations against
/// each other, each with its place among the other records.
pub(super) fn settle(w: &World, oracle: &mut Vec<J>, sim: &[J]) -> StatCounts {
    let mut c = StatCounts::default();
    let units: HashSet<&str> = (0..w.entities.len())
        .map(|i| crate::world::EntityIdx(u32::try_from(i).unwrap_or(0)))
        .filter(|e| w.unit(*e).is_some())
        .map(|e| w.entity(e).unique_id.as_str())
        .collect();
    oracle.retain(|r| {
        let g = if is_stat(r) { gap(&units, r) } else { None };
        if let Some(g) = g {
            *c.gaps.entry(g).or_default() += 1;
        }
        g.is_none()
    });
    let rec = placed(oracle);
    let got = placed(sim);
    for (i, r) in rec.iter().enumerate() {
        *c.compared.entry(r.0["what"].as_str().unwrap_or("?").to_owned()).or_default() += 1;
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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::stats::tests::world_with_unit;

    fn stat(owner: &str, new: i64) -> J {
        json!({"k": "mut", "what": "stat_value", "t": 3, "stat": 11, "owner": owner,
            "old": 1, "new": new, "permanent": 1})
    }

    fn damage() -> J {
        json!({"k": "mut", "what": "damage", "unit": "u", "old": 0, "new": 1})
    }

    /// A stat mutation moved across another record is a difference, though the stat mutations
    /// alone are in the same order; one far from the difference is not blamed.
    #[test]
    fn a_stat_mutation_moved_across_another_record_differs() {
        let (w, _) = world_with_unit();
        let fact = json!({"k": "mut", "what": "fact_attach", "owner": "u"});
        let rec = vec![stat("u", 2), damage(), stat("u", 3), fact.clone()];
        let run = |mut oracle: Vec<J>, sim: &[J]| settle(&w, &mut oracle, sim);
        let same = run(rec.clone(), &rec);
        assert!(same.agrees() && same.equal == 2, "{same:?}");
        // The first moved after the damage.
        let moved = [damage(), stat("u", 2), stat("u", 3), fact.clone()];
        let c = run(rec.clone(), &moved);
        assert!(!c.agrees() && c.differing == 1, "{c:?}");
        // The second moved after the fact attached.
        let moved = [stat("u", 2), damage(), fact.clone(), stat("u", 3)];
        let c = run(rec.clone(), &moved);
        assert!(!c.agrees() && c.differing == 1, "{c:?}");
        // The same record repeated: a mutation moved between identical rule and result pairs
        // keeps both neighbours' payloads, not their occurrences.
        let (r, e) = (json!({"k": "rule", "class": "R"}), json!({"k": "result", "class": "R"}));
        let at = |n: usize| {
            let mut v = Vec::new();
            for k in 0..3 {
                v.push(r.clone());
                v.push(e.clone());
                if k + 1 == n {
                    v.push(stat("u", 2));
                }
            }
            v
        };
        let (a, b) = (at(1), at(2));
        assert_eq!(settle(&w, &mut a.clone(), &a).equal, 1);
        let c = settle(&w, &mut a.clone(), &b);
        assert!(!c.agrees() && c.differing == 1, "{c:?}");
        // A different record elsewhere is no stat mutation's.
        let mut far = rec.clone();
        far.push(damage());
        far.push(damage());
        let mut sim = far.clone();
        sim[4] = json!({"k": "mut", "what": "damage", "unit": "u", "old": 5, "new": 6});
        let c = run(far, &sim);
        assert!(c.agrees(), "{c:?}");
    }

    /// A unit that mutated a stat and was disposed later is still a unit the sim made: its
    /// recorded mutations are compared, not excused as gaps.
    #[test]
    fn a_disposed_units_mutations_are_not_gaps() {
        let (mut w, u) = world_with_unit();
        crate::entity::dispose(&mut w, u);
        assert!(!w.by_id.contains_key("u"), "the registry forgets it");
        let rec = vec![stat("u", 2)];
        let mut oracle = rec.clone();
        let c = settle(&w, &mut oracle, &rec);
        assert!(c.gaps.is_empty() && c.agrees() && c.equal == 1, "{c:?}");
        // Without the sim's own record it is an unmatched recorded mutation, not a gap.
        let mut oracle = rec.clone();
        let c = settle(&w, &mut oracle, &[]);
        assert!(c.gaps.is_empty() && !c.agrees() && c.differing == 1, "{c:?}");
        // An id of no entity is a gap.
        let mut oracle = vec![stat("nobody", 2)];
        let c = settle(&w, &mut oracle, &[]);
        assert_eq!(c.gaps.values().sum::<usize>(), 1);
        assert!(c.agrees());
    }
}
