//! Initiative (decompiled, not copied): `RuleInitiativeRoll`
//! (`Kingmaker.RuleSystem.Rules/RuleInitiativeRoll.cs`) and the per-unit body of
//! `UnitCombatPrepareController.Tick` (`Kingmaker.Controllers.Combat/`), plus the part of
//! `CombatController.HandleUnitRollsInitiative` that caches a unit's initiative
//! (`m_KnownInitiative`, `TurnBased.Controllers/CombatController.cs`).
//!
//! Which units are in combat and when they join is the combat entry's business (an input of
//! the driven replay); the turn order itself (`TBUnitInfo` sorting and overrides in
//! `HandleUnitRollsInitiative`) is the turn controller's (`super::turns`).

use std::any::Any;

use serde_json::json;

use super::order::Initiative;
use super::rules::{rule_boilerplate, trigger_any};
use crate::rulebook::{RuleBase, RuleEvent};
use crate::stats::types::StatType;
use crate::world::{EntityIdx, World};

/// `RuleInitiativeRoll`.
#[derive(Debug)]
pub struct RuleInitiativeRoll {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    /// `m_OverrideResult`.
    pub override_result: Option<i32>,
    /// `D20` (the nested `RuleRollD20`'s result), set by `OnTrigger`.
    pub d20: i32,
    /// `Modifier`.
    pub modifier: i32,
}

impl RuleInitiativeRoll {
    #[must_use]
    pub fn new(initiator: EntityIdx, override_result: Option<i32>) -> Self {
        Self { base: RuleBase::default(), initiator, override_result, d20: 0, modifier: 0 }
    }

    /// `Result`: `m_OverrideResult ?? ((int)D20 + Modifier)`.
    #[must_use]
    pub fn result(&self) -> i32 {
        self.override_result.unwrap_or_else(|| crate::csharp::arith::add(self.d20, self.modifier))
    }
}

impl RuleEvent for RuleInitiativeRoll {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleInitiativeRoll"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn on_trigger(&mut self, w: &mut World) {
        self.modifier = w.stats(self.initiator).v(StatType::Initiative).modified_value;
        self.d20 = super::attack::d20(w, self.initiator).result();
    }
    rule_boilerplate!();
}

/// `UnitEntityData.IsSummoned(out caster)` for a unit with the summoned-unit buff.
fn is_summoned(w: &World, u: EntityIdx) -> bool {
    w.entity(u).facts.list.iter().any(|f| {
        w.fact(*f).kind == crate::world::FactKind::Buff
            && w.fact(*f).bp.as_ref().is_some_and(|b| b.name == "SummonedUnitBuff")
    })
}

/// The body of `UnitCombatPrepareController.Tick`'s loop for one unit that is in combat and
/// not yet prepared: the initiative roll (overridden by the turn-based controller's cached
/// value), the tie-break d20 (`InitiativeRandom`), `Prepared`.
pub fn prepare_unit(w: &mut World, u: EntityIdx) {
    let cached = if w.combat.turn_based {
        w.combat.known_initiative.get(&w.entity(u).unique_id).copied()
    } else {
        None
    };
    if cached.is_none() && is_summoned(w, u) {
        // The summoner's initiative is used, after the others have rolled.
        w.coverage.unsupported("UnitCombatPrepareController: summoned unit's initiative");
        return;
    }
    let mut roll = RuleInitiativeRoll::new(u, cached);
    trigger_any(w, &mut roll, |r| Some(json!({"Modifier": r.modifier, "Result": r.result()})));
    let initiative = roll.result();
    let random = super::attack::d20(w, u).result();
    if let Some(x) = w.unit_mut(u) {
        x.combat.initiative = Initiative { initiative, random };
        x.combat.prepared = true;
    }
    if w.combat.turns.on {
        // The controller's batch: its handlers run once every unit has rolled ([`prepare_flush`]).
        w.combat.turns.prepared.push((u, initiative));
    } else {
        handle_unit_rolls_initiative(w, u, initiative);
    }
}

/// The end of `UnitCombatPrepareController.Tick` (with the turn port): the prepared units sorted
/// by `UnitEntityOrderComaprer` (`List.Sort`) get their initiative cooldowns spread over the
/// round, then the rolls, sorted by result (descending, `List.Sort`), raise
/// `HandleUnitRollsInitiative` in that order.
pub fn prepare_flush(w: &mut World) {
    let batch = std::mem::take(&mut w.combat.turns.prepared);
    if batch.is_empty() {
        return;
    }
    let tb = w.combat.turn_based;
    let mut list: Vec<EntityIdx> = batch.iter().map(|(u, _)| *u).collect();
    let cap = crate::csharp::sort::grown_capacity(list.len());
    let mut fallback = false;
    let mut decided = 0;
    crate::csharp::sort::introsort(&mut list, cap, &mut |a, b| {
        use super::ties::{Tie, TieCmp, decide};
        let ai = w.unit(*a).map(|x| x.combat.initiative).unwrap_or_default();
        let bi = w.unit(*b).map(|x| x.combat.initiative).unwrap_or_default();
        match super::order::prepare_compare(w, *a, ai, *b, bi) {
            super::order::Cmp::Less => -1,
            super::order::Cmp::Greater => 1,
            super::order::Cmp::Equal => 0,
            // The recorded decision ([`super::ties`]), or reported.
            super::order::Cmp::HashFallback => {
                let tie = decide(w, TieCmp::Prepare, decided, *a, *b);
                decided += 1;
                match tie {
                    Tie::Decided(r) => r,
                    Tie::Off => {
                        fallback = true;
                        0
                    }
                    Tie::Missing => 0,
                }
            }
        }
    });
    super::ties::end_call(w, super::ties::TieCmp::Prepare);
    if fallback {
        w.coverage.unsupported(
            "UnitCombatPrepareController: the comparer's identity-hash fallback (object identity)",
        );
    }
    #[allow(clippy::cast_precision_loss)]
    let step = (6.0 - if tb { 0.0 } else { 1.0 }) / (list.len().saturating_sub(1).max(1) as f32);
    let mut at: f32 = if tb { 0.0 } else { 1.0 };
    for u in &list {
        if let Some(x) = w.unit_mut(*u) {
            x.combat.cooldown = super::Cooldowns { initiative: at, ..Default::default() };
        }
        at += step;
    }
    let mut rolls = batch;
    let cap = crate::csharp::sort::grown_capacity(rolls.len());
    crate::csharp::sort::introsort(&mut rolls, cap, &mut |a, b| -(a.1.cmp(&b.1) as i32));
    for (u, result) in rolls {
        handle_unit_rolls_initiative(w, u, result);
    }
}

/// `IUnitInitiativeHandler.HandleUnitRollsInitiative` of the turn-based controller (in
/// turn-based combat): the initiative cache, then, with the turn port, the turn-order
/// bookkeeping ([`super::turns::unit_rolls_initiative`]).
fn handle_unit_rolls_initiative(w: &mut World, u: EntityIdx, result: i32) {
    if w.combat.turn_based {
        if w.combat.turns.on && !w.combat.turns.units.iter().any(|i| i.unit == u) {
            // `FindUnitInfo(initiator) == null`: logged, nothing cached.
            w.log("error", "HandleUnitRollsInitiative: unit info not found");
            return;
        }
        let id = w.entity(u).unique_id.clone();
        w.combat.known_initiative.insert(id, result);
        if w.combat.turns.on {
            super::turns::unit_rolls_initiative(w, u);
        }
    }
}
