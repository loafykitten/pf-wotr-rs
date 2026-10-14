//! Turn order: `CombatController.TBUnitInfo` and `UnitsOrderComaprer`
//! (`TurnBased.Controllers/CombatController.cs:35–135`), and the initiative order of
//! `UnitCombatPrepareController.UnitEntityOrderComaprer`
//! (`Kingmaker.Controllers.Combat/UnitCombatPrepareController.cs`).

use std::cmp::Ordering;

use crate::stats::types::StatType;
use crate::world::{EntityIdx, World};

/// `CombatController.TBUnitInfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TbUnitInfo {
    pub unit: EntityIdx,
    pub initiative_processed: bool,
    pub surprising: bool,
    pub surprised: bool,
    pub initiative_override: Option<i32>,
    pub initiative_sequence: i32,
    pub acting_in_surprise_round: bool,
    pub marked_as_leave: bool,
}

impl TbUnitInfo {
    #[must_use]
    pub fn new(unit: EntityIdx) -> Self {
        Self {
            unit,
            initiative_processed: false,
            surprising: false,
            surprised: false,
            initiative_override: None,
            initiative_sequence: 0,
            acting_in_surprise_round: false,
            marked_as_leave: false,
        }
    }
}

/// A unit's `UnitCombatState` initiative values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Initiative {
    /// `CombatState.Initiative`.
    pub initiative: i32,
    /// `CombatState.InitiativeRandom`.
    pub random: i32,
}

/// The comparison outcome, or the point where the C# falls back to `GetHashCode()`
/// (object identity, which differs between runs: not reproducible, reported).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Less,
    Greater,
    Equal,
    HashFallback,
}

impl Cmp {
    fn of(o: Ordering) -> Option<Self> {
        match o {
            Ordering::Less => Some(Self::Less),
            Ordering::Greater => Some(Self::Greater),
            Ordering::Equal => None,
        }
    }
}

fn modified(w: &World, u: EntityIdx, t: StatType) -> i32 {
    w.stats(u).v(t).modified_value
}

/// The shared tail of both comparers: initiative stat, then Dexterity, then
/// `InitiativeRandom` (all descending), then the hash.
fn tail(w: &World, a: EntityIdx, ai: Initiative, b: EntityIdx, bi: Initiative) -> Cmp {
    // Each step: "x < y → 1 (x after y)", i.e. descending.
    let desc = |x: i32, y: i32| Cmp::of(y.cmp(&x));
    desc(modified(w, a, StatType::Initiative), modified(w, b, StatType::Initiative))
        .or_else(|| desc(modified(w, a, StatType::Dexterity), modified(w, b, StatType::Dexterity)))
        .or_else(|| desc(ai.random, bi.random))
        .unwrap_or(Cmp::HashFallback)
}

/// `UnitsOrderComaprer.Compare(xi, yi)`.
pub fn tb_compare(
    w: &World,
    x: &TbUnitInfo,
    xi: Initiative,
    y: &TbUnitInfo,
    yi: Initiative,
) -> Cmp {
    if x == y {
        return Cmp::Equal;
    }
    match (x.acting_in_surprise_round, y.acting_in_surprise_round) {
        (true, false) => return Cmp::Less,
        (false, true) => return Cmp::Greater,
        _ => {}
    }
    let nx = x.initiative_override.unwrap_or(xi.initiative);
    let ny = y.initiative_override.unwrap_or(yi.initiative);
    if let Some(c) = Cmp::of(ny.cmp(&nx)) {
        return c;
    }
    if let Some(c) = Cmp::of(x.initiative_sequence.cmp(&y.initiative_sequence)) {
        return c;
    }
    tail(w, x.unit, xi, y.unit, yi)
}

/// `UnitEntityOrderComaprer.Compare` (the initiative order at combat start).
pub fn prepare_compare(
    w: &World,
    a: EntityIdx,
    ai: Initiative,
    b: EntityIdx,
    bi: Initiative,
) -> Cmp {
    if a == b {
        return Cmp::Equal;
    }
    if let Some(c) = Cmp::of(bi.initiative.cmp(&ai.initiative)) {
        return c;
    }
    tail(w, a, ai, b, bi)
}

/// `Enumerable.OrderBy(u => u, comparer).ToList()`: a stable sort (LINQ's
/// `EnumerableSorter` breaks ties by index). Returns `false` if the comparer reached its
/// hash fallback (the result then depends on object identities in the original).
pub fn order_by(list: &mut [usize], mut cmp: impl FnMut(usize, usize) -> Cmp) -> bool {
    let mut fallback = false;
    list.sort_by(|&a, &b| match cmp(a, b) {
        Cmp::Less => Ordering::Less,
        Cmp::Greater => Ordering::Greater,
        Cmp::Equal => Ordering::Equal,
        Cmp::HashFallback => {
            fallback = true;
            Ordering::Equal
        }
    });
    !fallback
}
