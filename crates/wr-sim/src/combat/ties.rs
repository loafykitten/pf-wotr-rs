//! The initiative comparers' identity-hash tie-break as a recorded input.
//!
//! `UnitsOrderComaprer` (`CombatController.SortUnits`) and `UnitEntityOrderComaprer`
//! (`UnitCombatPrepareController.Tick`) end, when initiative, sequence, the initiative stat,
//! Dexterity and `InitiativeRandom` all tie, with `unit.GetHashCode() >= unit2.GetHashCode() ? -1 :
//! 1`: the objects' identity hashes (`UnitEntityData` doesn't override `GetHashCode`), which Mono
//! derives from an address. The sim can't compute them, so a recording that declares WrathOracle's
//! `init_ties` observer gives each decision (`init_tie`: the comparer, the decision's index within
//! its sort call, the two units in call order, the hashes the comparer got and its result), and the
//! sorts take them in order where they reach the fallback ([`decide`]), checked fail closed: a
//! fallback with no matching record, a record the sim didn't need (left over at the end of its
//! sort call or of its frame), and any malformed or inconsistent record are reported. Without the
//! observer ([`Turns::ties`] `None`) a fallback is reported as before.
//!
//! [`Turns::ties`]: super::turns::Turns::ties

use std::collections::{BTreeMap, HashMap, VecDeque};

use serde_json::Value as J;

use crate::world::{EntityIdx, World};

/// Which comparer decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TieCmp {
    /// `CombatController.UnitsOrderComaprer` (`SortUnits`).
    Turn,
    /// `UnitCombatPrepareController.UnitEntityOrderComaprer` (`Tick`'s sort).
    Prepare,
}

impl TieCmp {
    fn name(self) -> &'static str {
        match self {
            Self::Turn => "CombatController.SortUnits",
            Self::Prepare => "UnitCombatPrepareController",
        }
    }
}

/// One recorded decision (`init_tie`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TieDecision {
    pub cmp: TieCmp,
    /// The decision's index within its sort call.
    pub i: usize,
    /// The comparer's `unit` and `unit2` (the order of its two hash calls).
    pub a: String,
    pub b: String,
    /// `Compare`'s result: -1 or 1.
    pub r: i32,
}

impl TieDecision {
    /// A strict reading of an `init_tie` record: every field present with its type and in its
    /// domain, and the result the one the hashes give.
    fn parse(r: &J) -> Option<(i64, Self, i32, i32)> {
        let t = r["t"].as_i64().filter(|t| *t >= 0)?;
        let cmp = match r["cmp"].as_str()? {
            "turn" => TieCmp::Turn,
            "prepare" => TieCmp::Prepare,
            _ => return None,
        };
        let i = usize::try_from(r["i"].as_u64()?).ok()?;
        let a = r["a"].as_str().filter(|s| !s.is_empty())?.to_owned();
        let b = r["b"].as_str().filter(|s| !s.is_empty())?.to_owned();
        let ha = i32::try_from(r["ha"].as_i64()?).ok()?;
        let hb = i32::try_from(r["hb"].as_i64()?).ok()?;
        let res = i32::try_from(r["r"].as_i64()?).ok()?;
        if a == b || res != if ha >= hb { -1 } else { 1 } {
            return None;
        }
        Some((t, Self { cmp, i, a, b, r: res }, ha, hb))
    }
}

/// The recorded decisions of a run, by tick, and the current frame's.
#[derive(Debug, Default)]
pub struct Ties {
    by_tick: BTreeMap<i64, VecDeque<TieDecision>>,
    queue: VecDeque<TieDecision>,
    /// Decisions taken.
    pub used: usize,
}

/// Reads the run's `init_tie` records when its begin record declares `init_ties` (`declared`)
/// and the turn port runs (`port`: the sorts are the sim's), and turns the decisions on;
/// otherwise a recording must have none.
pub fn load(w: &mut World, recs: &[J], declared: bool, port: bool) {
    let ours =
        |r: &&J| r["k"] == "obs" && matches!(r["ev"].as_str(), Some("init_tie" | "init_tie_bad"));
    if !declared {
        if recs.iter().any(|r| ours(&r)) {
            w.coverage.unsupported(
                "driven replay: init_tie records in a recording that doesn't declare init_ties",
            );
        }
        return;
    }
    if !port {
        w.coverage.unsupported(
            "driven replay: the recording declares its init_ties observer without the turn controller's records",
        );
        return;
    }
    let mut ties = Ties::default();
    let mut hashes: HashMap<String, i32> = HashMap::new();
    // Per comparer, the last decision's tick and index (a decision after the first of its sort
    // call follows the one before it).
    let mut last: HashMap<&'static str, (i64, usize)> = HashMap::new();
    for r in recs.iter().filter(ours) {
        if r["ev"] == "init_tie_bad" {
            w.coverage.unsupported(
                "driven replay: an identity-hash decision WrathOracle couldn't record (init_tie_bad)",
            );
            continue;
        }
        let Some((t, d, ha, hb)) = TieDecision::parse(r) else {
            w.coverage.unsupported("driven replay: an init_tie record it can't read");
            continue;
        };
        for (id, h) in [(&d.a, ha), (&d.b, hb)] {
            if *hashes.entry(id.clone()).or_insert(h) != h {
                w.coverage.unsupported(
                    "driven replay: a unit's identity hash differs between its init_tie records",
                );
            }
        }
        let key = d.cmp.name();
        if d.i > 0 && last.get(key) != Some(&(t, d.i - 1)) {
            w.coverage.unsupported(
                "driven replay: an init_tie record that doesn't follow its sort call's previous one",
            );
        }
        last.insert(key, (t, d.i));
        ties.by_tick.entry(t).or_default().push_back(d);
    }
    w.combat.turns.ties = Some(ties);
}

/// At a frame's start (tick `t`): its recorded decisions are the ones the sorts take; decisions
/// of earlier ticks still waiting were never reached.
pub fn frame_start(w: &mut World, t: i64) {
    let Some(ties) = w.combat.turns.ties.as_mut() else { return };
    let later = ties.by_tick.split_off(&t);
    let missed = std::mem::replace(&mut ties.by_tick, later);
    ties.queue = ties.by_tick.remove(&t).unwrap_or_default();
    for _ in missed.values().flatten() {
        w.coverage.unsupported(
            "driven replay: a recorded identity-hash decision at a tick the sim didn't run",
        );
    }
}

/// At a frame's end: the frame's recorded decisions the sorts didn't take.
pub fn frame_end(w: &mut World) {
    let Some(ties) = w.combat.turns.ties.as_mut() else { return };
    let n = std::mem::take(&mut ties.queue).len();
    for _ in 0..n {
        w.coverage
            .unsupported("driven replay: a recorded identity-hash decision the sim didn't need");
    }
}

/// At the run's end, after the last frame's [`frame_end`]: decisions of ticks never reached.
/// Returns the number of decisions taken.
pub fn finish(w: &mut World) -> usize {
    let Some(ties) = w.combat.turns.ties.as_mut() else { return 0 };
    let n: usize = std::mem::take(&mut ties.by_tick).values().map(VecDeque::len).sum();
    let used = ties.used;
    for _ in 0..n {
        w.coverage.unsupported(
            "driven replay: a recorded identity-hash decision at a tick the sim didn't run",
        );
    }
    used
}

/// What a comparer's identity-hash fallback returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tie {
    /// No recorded decisions (an older recording): the caller reports the fallback.
    Off,
    /// The recorded result.
    Decided(i32),
    /// The next recorded decision isn't this one (or there is none): reported here.
    Missing,
}

/// The `i`-th fallback of a sort call by comparer `cmp`, on units `a` (`unit`) and `b` (`unit2`):
/// the frame's next recorded decision, which must be this one.
pub fn decide(w: &mut World, cmp: TieCmp, i: usize, a: EntityIdx, b: EntityIdx) -> Tie {
    let Some(ties) = w.combat.turns.ties.as_ref() else { return Tie::Off };
    let matches = ties.queue.front().is_some_and(|d| {
        d.cmp == cmp && d.i == i && d.a == w.entity(a).unique_id && d.b == w.entity(b).unique_id
    });
    if !matches {
        w.coverage.unsupported(format!(
            "{}: the comparer's identity-hash fallback without its recorded decision",
            cmp.name()
        ));
        return Tie::Missing;
    }
    let ties = w.combat.turns.ties.as_mut().expect("checked above");
    let d = ties.queue.pop_front().expect("checked above");
    ties.used += 1;
    Tie::Decided(d.r)
}

/// The end of a sort call by comparer `cmp`: recorded decisions that continue a call (an index
/// above 0: a new call's first is 0) weren't needed.
pub fn end_call(w: &mut World, cmp: TieCmp) {
    let Some(ties) = w.combat.turns.ties.as_mut() else { return };
    let mut extra = 0;
    while ties.queue.front().is_some_and(|d| d.cmp == cmp && d.i > 0) {
        ties.queue.pop_front();
        extra += 1;
    }
    for _ in 0..extra {
        w.coverage.unsupported(format!(
            "{}: a recorded identity-hash decision the sort didn't need",
            cmp.name()
        ));
    }
}

#[cfg(test)]
mod tests;
