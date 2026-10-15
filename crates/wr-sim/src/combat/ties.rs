//! The initiative comparers' identity-hash tie-break as a recorded input.
//!
//! `UnitsOrderComaprer` (`CombatController.SortUnits`) and `UnitEntityOrderComaprer`
//! (`UnitCombatPrepareController.Tick`) end, when initiative, sequence, the initiative stat,
//! Dexterity and `InitiativeRandom` all tie, with `unit.GetHashCode() >= unit2.GetHashCode() ? -1 :
//! 1`: the objects' identity hashes (`UnitEntityData` doesn't override `GetHashCode`), which Mono
//! derives from an address. The sim can't compute them, so a recording that declares WrathOracle's
//! `init_ties` observer gives each decision (`init_tie`: the comparer, the decision's index within
//! its sort call, the two units in call order, the hashes the comparer got and its result), and the
//! sorts take them in order where they reach the fallback ([`decide`]), checked fail closed.
//!
//! Each decision is bound to its sort call. A `SortUnits` decision belongs to the recorded `sort`
//! record that follows it in its frame (the call's postfix): the sim's k-th `sort_units` call (the
//! turn port matches the sim's sorts against the recorded ones, in order) takes the decisions of
//! the k-th recorded `sort`, and no other. `UnitCombatPrepareController.Tick` sorts at most once a
//! frame, and so does the sim's `prepare_flush` (its batch is the frame's): a prepare decision
//! belongs to its frame's prepare sort. A record must sit in the frame its tick names (after that
//! tick's `tick` record, before the next). Reported: a fallback with no matching record, a record
//! its sort call didn't need (or whose call the sim never made), and any malformed, misplaced or
//! inconsistent record. Without the observer ([`Turns::ties`] `None`) a fallback is reported as
//! before. Where the end record counts each comparer's `Compare` calls and decisions in the window
//! (`init_tie_compares_*`, `init_ties_*`), the sim's must be the same.
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
    fn ix(self) -> usize {
        match self {
            Self::Turn => 0,
            Self::Prepare => 1,
        }
    }

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

/// The recorded decisions of a run, bound to their sort calls.
#[derive(Debug, Default)]
pub struct Ties {
    /// Prepare decisions by tick.
    prepare: BTreeMap<i64, VecDeque<TieDecision>>,
    /// The current frame's prepare decisions.
    prepare_frame: VecDeque<TieDecision>,
    /// `SortUnits` decisions by the index of the recorded `sort` they precede, with its tick.
    turn: BTreeMap<usize, (i64, VecDeque<TieDecision>)>,
    /// The sim's `SortUnits` calls so far.
    turn_calls: usize,
    /// The running `SortUnits` call's decisions.
    turn_call: VecDeque<TieDecision>,
    /// The current frame's tick.
    tick: i64,
    /// Decisions taken.
    pub used: usize,
    /// Per comparer ([`TieCmp::ix`]): the sim's `Compare` calls and decisions taken, and the end
    /// record's counts of the game's (when it has them).
    compares: [u64; 2],
    decided: [u64; 2],
    recorded: Option<[[u64; 2]; 2]>,
}

/// Reports `n` recorded decisions the sim didn't take.
fn unneeded(w: &mut World, n: usize, what: &str) {
    for _ in 0..n {
        w.coverage
            .unsupported(format!("{what}: a recorded identity-hash decision the sim didn't need"));
    }
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
    let mut ties = Ties { tick: -1, ..Ties::default() };
    // The end record's counts: all four, or none (a capture made before WrathOracle counted).
    if let Some(end) = recs.iter().rev().find(|r| r["k"] == "end") {
        let keys = [
            "init_tie_compares_turn",
            "init_ties_turn",
            "init_tie_compares_prepare",
            "init_ties_prepare",
        ];
        let v: Vec<Option<u64>> = keys.iter().map(|k| end[*k].as_u64()).collect();
        if v.iter().all(Option::is_some) {
            let v: Vec<u64> = v.into_iter().flatten().collect();
            ties.recorded = Some([[v[0], v[1]], [v[2], v[3]]]);
        } else if keys.iter().any(|k| end.get(*k).is_some()) {
            w.coverage
                .unsupported("driven replay: an end record whose init_tie counts it can't read");
        }
    }
    let mut hashes: HashMap<String, i32> = HashMap::new();
    // Per comparer, the last decision's tick and index (a decision after the first of its sort
    // call follows the one before it).
    let mut last: HashMap<&'static str, (i64, usize)> = HashMap::new();
    // The frame the records are in (the last `tick` record's), the recorded `sort`s so far, and
    // the `SortUnits` decisions waiting for theirs.
    let mut frame: Option<i64> = None;
    let mut sorts = 0;
    let mut pending: VecDeque<TieDecision> = VecDeque::new();
    let unbound = |w: &mut World, n: usize| {
        for _ in 0..n {
            w.coverage.unsupported(
                "driven replay: a SortUnits init_tie record without its sort record in its frame",
            );
        }
    };
    for r in recs {
        if r["k"] == "tick" {
            unbound(w, std::mem::take(&mut pending).len());
            frame = r["t"].as_i64();
            continue;
        }
        if r["k"] == "tb" && r["ev"] == "sort" {
            if !pending.is_empty() {
                let t = r["t"].as_i64().unwrap_or(-1);
                ties.turn.insert(sorts, (t, std::mem::take(&mut pending)));
            }
            sorts += 1;
            continue;
        }
        if !ours(&r) {
            continue;
        }
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
        if frame != Some(t) {
            w.coverage.unsupported("driven replay: an init_tie record outside its tick's frame");
            continue;
        }
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
        match d.cmp {
            TieCmp::Turn => pending.push_back(d),
            TieCmp::Prepare => ties.prepare.entry(t).or_default().push_back(d),
        }
    }
    unbound(w, pending.len());
    w.combat.turns.ties = Some(ties);
}

/// At a frame's start (tick `t`): its prepare decisions are the ones the frame's prepare sort
/// takes; those of earlier ticks still waiting were never reached.
pub fn frame_start(w: &mut World, t: i64) {
    let Some(ties) = w.combat.turns.ties.as_mut() else { return };
    ties.tick = t;
    let later = ties.prepare.split_off(&t);
    let missed = std::mem::replace(&mut ties.prepare, later);
    ties.prepare_frame = ties.prepare.remove(&t).unwrap_or_default();
    let n = missed.values().map(VecDeque::len).sum();
    unneeded(w, n, TieCmp::Prepare.name());
}

/// At a frame's end: the frame's prepare decisions no prepare sort took.
pub fn frame_end(w: &mut World) {
    let Some(ties) = w.combat.turns.ties.as_mut() else { return };
    let n = std::mem::take(&mut ties.prepare_frame).len();
    unneeded(w, n, TieCmp::Prepare.name());
}

/// At the run's end, after the last frame's [`frame_end`]: decisions of sort calls or frames the
/// sim never reached. Returns the number of decisions taken.
pub fn finish(w: &mut World) -> usize {
    let Some(ties) = w.combat.turns.ties.as_mut() else { return 0 };
    let p: usize = std::mem::take(&mut ties.prepare).values().map(VecDeque::len).sum();
    let s: usize = std::mem::take(&mut ties.turn).values().map(|(_, q)| q.len()).sum();
    let used = ties.used;
    let counts = ties.recorded.map(|rec| (rec, [ties.compares, ties.decided]));
    unneeded(w, p, TieCmp::Prepare.name());
    unneeded(w, s, TieCmp::Turn.name());
    if let Some((rec, [compares, decided])) = counts {
        for cmp in [TieCmp::Turn, TieCmp::Prepare] {
            let k = cmp.ix();
            if rec[k] != [compares[k], decided[k]] {
                w.coverage.unsupported(format!(
                    "{}: the sim's comparer calls or decisions differ from the end record's counts",
                    cmp.name()
                ));
            }
        }
    }
    used
}

/// A call of comparer `cmp` (counted against the end record's).
pub fn compared(w: &mut World, cmp: TieCmp) {
    if let Some(ties) = w.combat.turns.ties.as_mut() {
        ties.compares[cmp.ix()] += 1;
    }
}

/// What a comparer's identity-hash fallback returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tie {
    /// No recorded decisions (an older recording): the caller reports the fallback.
    Off,
    /// The recorded result.
    Decided(i32),
    /// The next recorded decision of this sort call isn't this one (or there is none): reported
    /// here.
    Missing,
}

/// The start of a sort call by comparer `cmp`: a `SortUnits` call takes the decisions of its
/// recorded `sort` (by count), which must be of this frame.
pub fn begin_call(w: &mut World, cmp: TieCmp) {
    let Some(ties) = w.combat.turns.ties.as_mut() else { return };
    if cmp == TieCmp::Prepare {
        return;
    }
    let k = ties.turn_calls;
    ties.turn_calls += 1;
    let tick = ties.tick;
    let (t, q) = ties.turn.remove(&k).unwrap_or((tick, VecDeque::new()));
    ties.turn_call = q;
    if t != tick {
        let n = std::mem::take(&mut ties.turn_call).len();
        w.coverage.unsupported(
            "CombatController.SortUnits: a call whose recorded sort (and decisions) is of another tick",
        );
        unneeded(w, n, cmp.name());
    }
}

/// The `i`-th fallback of the running sort call by comparer `cmp`, on units `a` (`unit`) and `b`
/// (`unit2`): the call's next recorded decision, which must be this one.
pub fn decide(w: &mut World, cmp: TieCmp, i: usize, a: EntityIdx, b: EntityIdx) -> Tie {
    let Some(ties) = w.combat.turns.ties.as_ref() else { return Tie::Off };
    let q = match cmp {
        TieCmp::Turn => &ties.turn_call,
        TieCmp::Prepare => &ties.prepare_frame,
    };
    let matches = q.front().is_some_and(|d| {
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
    let q = match cmp {
        TieCmp::Turn => &mut ties.turn_call,
        TieCmp::Prepare => &mut ties.prepare_frame,
    };
    let d = q.pop_front().expect("checked above");
    ties.used += 1;
    ties.decided[cmp.ix()] += 1;
    Tie::Decided(d.r)
}

/// The end of a sort call by comparer `cmp`: its recorded decisions it didn't take (a frame has
/// one prepare sort at most, so the frame's remaining prepare decisions too).
pub fn end_call(w: &mut World, cmp: TieCmp) {
    let Some(ties) = w.combat.turns.ties.as_mut() else { return };
    let n = match cmp {
        TieCmp::Turn => std::mem::take(&mut ties.turn_call).len(),
        TieCmp::Prepare => std::mem::take(&mut ties.prepare_frame).len(),
    };
    unneeded(w, n, cmp.name());
}

#[cfg(test)]
mod tests;
