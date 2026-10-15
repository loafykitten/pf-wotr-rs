//! The engagement controller (decompiled, not copied: `Kingmaker.Controllers.Combat`):
//! `UnitCombatEngagementController` (`Tick`, `TickUnit`, the queued attacks of opportunity),
//! `UnitEngagementExtension.IsEngage`, `UnitHelper.IsAttackOfOpportunityReach`, and
//! `UnitCombatState`'s engagement: `IsEngage`, `Engage`, `Disengage`, `ShouldAttackOnDisengage`,
//! `AttackOfOpportunity` and the attacks of opportunity's count. docs/poc5/commands.md, slice E.
//!
//! `m_EngagedUnits` and `m_EngagedBy` are .NET dictionaries ([`CsDict`]: their enumeration order
//! is observable). What the code reads outside the sim is an input, recorded where the game reads
//! it ([`EngInputs`]): the group memory, corpulences, distances, line of sight, concealment, the
//! movement this tick, the threat range, `LastTarget`/`ManualTarget`, the prevent flags, the
//! attack restriction. The attack of opportunity it runs goes through the commands model
//! ([`super::commands`]). Reported, not modelled: mounts, tactical combat, the mobility checks,
//! `AttackOfOpportunityLimits`, a trickster's attack, `UnitPartForceMove`, the real-time branch of
//! `ShouldAttackOnDisengage`, the facts that force or provoke attacks of opportunity.

use super::commands::{self as uc, Inputs, Of, R};
use crate::components::state::has_mechanics_feature;
use crate::csharp::arith::store_f32;
use crate::csharp::{CsDict, TimeSpan};
use crate::stats::types::StatType;
use crate::unit_state::has_condition;
use crate::world::{EntityIdx, World};

/// `UnitCondition` values read here.
pub mod cond {
    pub const CONFUSION: i32 = 25;
    pub const DISABLE_ATTACKS_OF_OPPORTUNITY: i32 = 29;
    pub const IMMUNE_TO_ATTACK_OF_OPPORTUNITY: i32 = 35;
    pub const USE_MOBILITY_TO_NEGATE_ATTACK_OF_OPPORTUNITY: i32 = 36;
    pub const ATTACK_OF_OPPORTUNITY_BEFORE_INITIATIVE: i32 = 39;
    pub const USE_MOBILITY_TO_NEGATE_DEFENSIVELY_FAIL: i32 = 56;
}

/// `Concealment.Total`.
const TOTAL: i64 = 2;

/// `UnitAttackOfOpportunity`'s cooldown when the round's first one is used.
pub const AOO_COOLDOWN: f32 = 5.399_999_6;

/// A unit's engagement (`UnitCombatState`).
#[derive(Debug, Clone, Default)]
pub struct UnitEngage {
    /// The state is the game's: the window's initial state adopted (`eng_init`), or the unit
    /// constructed since. A unit's count is established separately ([`Self::count`]).
    pub known: bool,
    /// `m_EngagedUnits`: the units this one engages, with the time.
    pub engaged_units: CsDict<EntityIdx, TimeSpan>,
    /// `m_EngagedBy`: the units engaging this one.
    pub engaged_by: CsDict<EntityIdx, TimeSpan>,
    /// `DisengageAttackTargets`.
    pub disengage_targets: Vec<EntityIdx>,
    /// `m_AttackedByAooUnits` (cleared each engagement tick).
    pub attacked_by_aoo: Vec<EntityIdx>,
    /// `m_AttackOfOpportunityCount`, once established.
    pub count: Option<i32>,
}

/// The engagement controller's state.
#[derive(Debug, Clone, Default)]
pub struct Engage {
    /// The model runs (a recording with WrathOracle's `engage` observer).
    pub on: bool,
    /// `m_UnitsProvokeAttackOfOpportunity` (a `HashSet`, cleared each tick: insertion order).
    pub provoke: Vec<EntityIdx>,
    /// `m_UnitsProvokeAttackOfOpportunityAfterDefensivelyFail`.
    pub provoke_adf: Vec<EntityIdx>,
    /// Every `ProvokeAttackOfOpportunity` call since the last tick, in order, with the rule whose
    /// handler made it (the queue keeps a unit once): what the recorded calls are checked against.
    pub provoke_calls: Vec<(EntityIdx, &'static str)>,
}

/// What the engagement's code reads outside itself, and its checks (the driven replay's side:
/// WrathOracle's observer `engage`).
pub trait EngInputs: Inputs {
    /// A boolean leaf `f` (of unit `u`, and `to` when the record names one).
    fn eng_bool(
        &mut self,
        w: &mut World,
        f: &'static str,
        u: Option<&str>,
        to: Option<&str>,
    ) -> R<bool>;
    /// A `float` leaf (`corp`, `sqd`).
    fn eng_float(&mut self, w: &mut World, f: &'static str, u: &str, to: Option<&str>) -> R<f32>;
    /// `GetThreatRange(u, hand)`: null or the meters.
    fn eng_threat_range(&mut self, w: &mut World, u: &str) -> R<Option<f32>>;
    /// A unit leaf (`last`, `manual`) of unit `u`.
    fn eng_unit(&mut self, w: &mut World, f: &'static str, u: &str) -> R<Option<String>>;
    /// `UnitPartConcealment.Calculate(u, to, attack)`.
    fn eng_conc(&mut self, w: &mut World, u: &str, to: &str) -> R<i64>;
    /// `Memory.Enemies` (the units' ids, in order).
    fn eng_enemies(&mut self, w: &mut World) -> R<Vec<String>>;
    /// `GetThreatHand(u)`: the sim's `hand`, checked.
    fn eng_threat(&mut self, w: &mut World, u: EntityIdx, hand: Option<uc::attack::Slot>) -> R<()>;
    /// `TickUnit(u)` starts: its count (`None`: not established, adopted from the record).
    fn eng_tick_unit(&mut self, w: &mut World, u: EntityIdx, count: Option<i32>) -> R<i32>;
    /// `Engage`/`Disengage(t)` of `u` returned: the two dictionaries after it.
    fn eng_done(&mut self, w: &mut World, check: &EngageCheck) -> R<()>;
    /// `AttackOfOpportunity` returned.
    fn eng_aoo(&mut self, w: &mut World, check: &AooCheck) -> R<()>;
    /// The count's setter changed `u`'s cooldowns row (`cd_write`): `before` and `after`.
    fn eng_count_write(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        before: super::Cooldowns,
        after: super::Cooldowns,
        disengaging: bool,
    ) -> R<()>;
    /// `Commands.Run(new UnitAttackOfOpportunity(target, trickster, shouldTurnToTarget))` in
    /// set `set`: its id (the recording's), once its description is checked.
    fn eng_new_run(
        &mut self,
        w: &mut World,
        set: u32,
        target: &str,
        trick: bool,
        disengaging: bool,
    ) -> R<u32>;
}

/// What `eng_e` checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngageCheck {
    pub f: &'static str,
    pub unit: String,
    pub target: String,
    /// The unit's `EngagedUnits`, the target's `EngagedBy`, the unit's `DisengageAttackTargets`.
    pub engaged_units: Vec<String>,
    pub engaged_by: Vec<String>,
    pub disengage_targets: Vec<String>,
}

/// What `eng_aoo` checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AooCheck {
    pub unit: String,
    pub target: String,
    pub trick: bool,
    pub simulate: bool,
    pub disengaging: bool,
    pub after_defensively_fail: bool,
    pub ret: bool,
    pub count: i32,
    pub attacked_by_aoo: Vec<String>,
}

fn uid(w: &World, u: EntityIdx) -> String {
    w.entity(u).unique_id.clone()
}

fn ids(w: &World, us: impl IntoIterator<Item = EntityIdx>) -> Vec<String> {
    us.into_iter().map(|u| uid(w, u)).collect()
}

fn desync<T>(what: impl Into<String>) -> R<T> {
    Err(uc::Desync(what.into()))
}

fn unsupported<T>(w: &mut World, what: &str) -> R<T> {
    w.coverage.unsupported(format!("engagement: {what}"));
    desync(format!("not modelled: {what}"))
}

fn unit_of(w: &World, id: &str) -> R<EntityIdx> {
    match w.by_id.get(id).copied().filter(|e| w.unit(*e).is_some()) {
        Some(u) => Ok(u),
        None => desync(format!("the engagement reads unit {id}, which the sim doesn't have")),
    }
}

fn eng(w: &World, u: EntityIdx) -> R<&UnitEngage> {
    match w.unit(u) {
        Some(x) if x.combat.eng.known => Ok(&x.combat.eng),
        Some(_) => desync(format!("unit {}'s engagement isn't established", uid(w, u))),
        None => desync("the engagement of an entity that isn't a unit"),
    }
}

fn eng_mut(w: &mut World, u: EntityIdx) -> R<&mut UnitEngage> {
    if !eng(w, u)?.known {
        return desync("unreachable");
    }
    Ok(&mut w.unit_mut(u).expect("a unit").combat.eng)
}

/// `AttackOfOpportunityPerRound`: `Stats.AttackOfOpportunityCount` (its modified value).
fn per_round(w: &World, u: EntityIdx) -> i32 {
    w.unit(u)
        .and_then(|x| x.stats.get(StatType::AttackOfOpportunityCount))
        .map_or(0, |s| s.modified_value)
}

fn count(w: &World, u: EntityIdx) -> R<i32> {
    match eng(w, u)?.count {
        Some(c) => Ok(c),
        None => {
            desync(format!("unit {}'s attacks of opportunity count isn't established", uid(w, u)))
        }
    }
}

/// `AttackOfOpportunityCount`'s setter: lowering a full count starts the cooldown. Returns
/// whether the cooldowns row changed.
fn set_count(w: &mut World, u: EntityIdx, v: i32) -> R<bool> {
    let old = count(w, u)?;
    let mut changed = false;
    if v < old && old == per_round(w, u) {
        let x = &mut w.unit_mut(u).expect("a unit").combat.cooldown;
        changed = x.attack_of_opportunity.to_bits() != AOO_COOLDOWN.to_bits();
        x.attack_of_opportunity = AOO_COOLDOWN;
    }
    eng_mut(w, u)?.count = Some(v);
    Ok(changed)
}

/// The refill of the cooldowns controller (real time) and `TurnController.Prepare` (the
/// rider's): a count at most the round's back to it, `DisengageAttackTargets` cleared. A unit
/// whose engagement isn't established is reported.
pub fn refill(w: &mut World, u: EntityIdx) {
    if !w.combat.engage.on {
        return;
    }
    let Some(x) = w.unit(u) else { return };
    if !x.combat.eng.known {
        w.coverage.unsupported("engagement: a refill of a unit whose engagement isn't established");
        return;
    }
    let pr = per_round(w, u);
    let Some(e) = w.unit_mut(u).map(|x| &mut x.combat.eng) else { return };
    // (Not established: a count at most the round's is the round's afterwards either way,
    // but one above it stays unknown.)
    match e.count {
        Some(c) if pr > 0 && c <= pr => {
            e.count = Some(pr);
            e.disengage_targets.clear();
        }
        Some(_) => {}
        None if pr > 0 => w.coverage.unsupported(
            "engagement: a refill of an attacks of opportunity count that isn't established",
        ),
        None => {}
    }
}

/// `UnitCombatState.Clear()`'s engagement part (leaving combat): the count back to the
/// round's, the unit removed from the others' dictionaries, its own cleared.
pub fn clear(w: &mut World, u: EntityIdx) {
    if !w.combat.engage.on {
        return;
    }
    let Some(x) = w.unit(u) else { return };
    if !x.combat.eng.known {
        w.coverage.unsupported("engagement: Clear of a unit whose engagement isn't established");
        return;
    }
    let pr = per_round(w, u);
    let (units, by): (Vec<EntityIdx>, Vec<EntityIdx>) =
        (x.combat.eng.engaged_units.keys().collect(), x.combat.eng.engaged_by.keys().collect());
    for k in units {
        if let Some(y) = w.unit_mut(k) {
            y.combat.eng.engaged_by.remove(&u);
        }
    }
    for k in by {
        if let Some(y) = w.unit_mut(k) {
            y.combat.eng.engaged_units.remove(&u);
        }
    }
    let e = &mut w.unit_mut(u).expect("a unit").combat.eng;
    // (The setter: raising the count never starts the cooldown.)
    e.count = Some(pr);
    e.engaged_units.clear();
    e.engaged_by.clear();
    e.disengage_targets.clear();
}

/// The engagement's rule handlers: `RuleAttackRoll` (ranged, not `DoNotProvokeAttacksOfOpportunity`,
/// not fake) and `RuleAttackWithWeapon` (unarmed without `ImprovedUnarmedStrike`) provoke.
pub fn provoke(w: &mut World, u: EntityIdx, rule: &'static str) {
    if !w.combat.engage.on {
        return;
    }
    w.combat.engage.provoke_calls.push((u, rule));
    if !w.combat.engage.provoke.contains(&u) {
        w.combat.engage.provoke.push(u);
    }
}

/// The controller's `OnEventDidTrigger(RuleAttackRoll)` and `(RuleAttackWithWeapon)`. A fake
/// attack roll (`IsFake`) isn't modelled: the sim fires none.
pub fn rule_did_trigger(w: &mut World, rule: &mut dyn crate::rulebook::RuleEvent) {
    use super::attack::{RuleAttackRoll, RuleAttackWithWeapon};
    use super::weapon::AttackType;
    if !w.combat.engage.on {
        return;
    }
    let any = rule.as_any();
    let unit = if let Some(r) = any.downcast_mut::<RuleAttackRoll>() {
        (matches!(r.attack_type, AttackType::Ranged | AttackType::RangedTouch) && !r.do_not_provoke)
            .then_some((r.initiator, "RuleAttackRoll"))
    } else if let Some(r) = any.downcast_mut::<RuleAttackWithWeapon>() {
        (r.weapon.is_unarmed()
            && !super::weapon::has_feature(w, r.initiator, super::weapon::IMPROVED_UNARMED_STRIKE))
        .then_some((r.initiator, "RuleAttackWithWeapon"))
    } else {
        None
    };
    if let Some((u, rule)) = unit {
        provoke(w, u, rule);
    }
}

/// `UnitCombatEngagementController.Tick`.
pub fn tick<I: EngInputs>(w: &mut World, inp: &mut I) -> R<()> {
    // `foreach unit in State.Units`: the pool through its enumerator.
    let units: Vec<EntityIdx> = crate::scene::units_pool(w);
    for u in units {
        if !crate::scene::enumerated_by_pool(w, u) {
            continue;
        }
        if !super::units::is_dead(w, u) && super::is_in_combat(w, u) {
            tick_unit(w, inp, u)?;
        }
    }
    // `m_ForceAttackOfOpportunity`: its callers (facts, the mobility trickster) aren't modelled.
    for unit in std::mem::take(&mut w.combat.engage.provoke) {
        for by in eng(w, unit)?.engaged_by.keys().collect::<Vec<_>>() {
            attack_of_opportunity(w, inp, by, unit, Flags::default())?;
        }
    }
    for unit in std::mem::take(&mut w.combat.engage.provoke_adf) {
        for by in eng(w, unit)?.engaged_by.keys().collect::<Vec<_>>() {
            let f = Flags { after_defensively_fail: true, ..Flags::default() };
            attack_of_opportunity(w, inp, by, unit, f)?;
        }
    }
    // Every unit's `PreventAttacksOfOpporunityNextFrame` (an input) and `m_AttackedByAooUnits`.
    for u in crate::scene::units_pool(w) {
        if !crate::scene::enumerated_by_pool(w, u) {
            continue;
        }
        if let Some(x) = w.unit_mut(u) {
            x.combat.eng.attacked_by_aoo.clear();
        }
    }
    Ok(())
}

/// `TickUnit(unit)`.
fn tick_unit<I: EngInputs>(w: &mut World, inp: &mut I, u: EntityIdx) -> R<()> {
    let id = uid(w, u);
    let known = eng(w, u)?.count;
    let c = inp.eng_tick_unit(w, u, known)?;
    eng_mut(w, u)?.count = Some(c);
    let mut can = inp.leaf(w, "canact", Of::Unit(Some(id.clone())))?;
    let mut range = 0.0f32;
    if can {
        let hand = super::commands::aoo::threat_hand(w, u);
        inp.eng_threat(w, u, hand)?;
        let num = match hand {
            Some(_) => inp.eng_threat_range(w, &id)?,
            None => None,
        };
        can &= num.is_some();
        if let Some(n) = num {
            range = n;
        }
    }
    for e in inp.eng_enemies(w)? {
        let t = unit_of(w, &e)?;
        if !eng(w, u)?.engaged_units.contains_key(&t)
            && super::units::is_conscious(w, t)
            && is_engage(w, inp, u, t, can, range)?
        {
            engage(w, inp, u, t)?;
        }
    }
    let mut out = Vec::new();
    for t in eng(w, u)?.engaged_units.keys().collect::<Vec<_>>() {
        if !is_engage(w, inp, u, t, can, range)? {
            out.push(t);
        }
    }
    for t in out {
        disengage(w, inp, u, t)?;
    }
    Ok(())
}

/// `UnitEngagementExtension.IsEngage(unit, enemy, canEngage, threatRange)`.
fn is_engage<I: EngInputs>(
    w: &mut World,
    inp: &mut I,
    u: EntityIdx,
    t: EntityIdx,
    can: bool,
    range: f32,
) -> R<bool> {
    if !can || !is_aoo_reach(w, inp, u, t, range)? {
        return Ok(false);
    }
    let (a, b) = (uid(w, u), uid(w, t));
    Ok(inp.eng_conc(w, &a, &b)? != TOTAL)
}

/// `UnitHelper.IsAttackOfOpportunityReach(unit, enemy, threatRange)`.
fn is_aoo_reach<I: EngInputs>(
    w: &mut World,
    inp: &mut I,
    u: EntityIdx,
    t: EntityIdx,
    range: f32,
) -> R<bool> {
    let (a, b) = (uid(w, u), uid(w, t));
    let c1 = inp.eng_float(w, "corp", &a, None)?;
    let c2 = inp.eng_float(w, "corp", &b, None)?;
    let num = store_f32(f64::from(c1) + f64::from(c2) + f64::from(range));
    let d = inp.eng_float(w, "sqd", &a, Some(&b))?;
    Ok(f64::from(d) < f64::from(num) * f64::from(num)
        && inp.eng_bool(w, "los", Some(&a), Some(&b))?)
}

/// `UnitCombatState.Engage(target)`.
fn engage<I: EngInputs>(w: &mut World, inp: &mut I, u: EntityIdx, t: EntityIdx) -> R<()> {
    if !eng(w, u)?.engaged_units.contains_key(&t) {
        // (`flag`: the target wasn't engaged; `HandleUnitEngaged` is the real-time auto-pause's.)
        let now = w.clock.game_time;
        eng(w, t)?;
        eng_mut(w, u)?.engaged_units.insert(t, now);
        eng_mut(w, t)?.engaged_by.insert(u, now);
        for x in [u, t] {
            let id = uid(w, x);
            inp.nct_zero(w, &id, "Engage")?;
            w.combat.ai.nct.insert(x, 0.0);
        }
    }
    done(w, inp, "Engage", u, t)
}

fn done<I: EngInputs>(
    w: &mut World,
    inp: &mut I,
    f: &'static str,
    u: EntityIdx,
    t: EntityIdx,
) -> R<()> {
    let check = EngageCheck {
        f,
        unit: uid(w, u),
        target: uid(w, t),
        engaged_units: ids(w, eng(w, u)?.engaged_units.keys().collect::<Vec<_>>()),
        engaged_by: ids(w, eng(w, t)?.engaged_by.keys().collect::<Vec<_>>()),
        disengage_targets: ids(w, eng(w, u)?.disengage_targets.clone()),
    };
    inp.eng_done(w, &check)
}

/// `UnitCombatState.Disengage(target)`.
fn disengage<I: EngInputs>(w: &mut World, inp: &mut I, u: EntityIdx, t: EntityIdx) -> R<()> {
    if inp.eng_bool(w, "tact", None, None)? {
        return unsupported(w, "Disengage in tactical combat");
    }
    if should_attack_on_disengage(w, inp, u, t)? {
        let f = Flags { disengaging: true, ..Flags::default() };
        if attack_of_opportunity(w, inp, u, t, f)? {
            eng_mut(w, u)?.disengage_targets.push(t);
        }
    }
    eng_mut(w, u)?.engaged_units.remove(&t);
    eng_mut(w, t)?.engaged_by.remove(&u);
    let (a, b) = (uid(w, u), uid(w, t));
    // `target == LastTarget || target == ManualTarget` (a live target: reference equality).
    let hit = inp.eng_unit(w, "last", &a)?.as_deref() == Some(b.as_str())
        || inp.eng_unit(w, "manual", &a)?.as_deref() == Some(b.as_str());
    if hit {
        inp.nct_zero(w, &a, "Disengage")?;
        w.combat.ai.nct.insert(u, 0.0);
    }
    // (`HandleUnitDisengaged` when the target is left unengaged: the real-time auto-pause's.)
    done(w, inp, "Disengage", u, t)
}

/// `ShouldAttackOnDisengage(target, simulate: false)`: in turn-based combat the scope sets
/// `LastTarget` to the target and restores it on every exit (its change handlers: none). With the
/// targets modelled ([`super::targets`]) both writes are the sim's; otherwise `LastTarget` is an
/// input.
fn should_attack_on_disengage<I: EngInputs>(
    w: &mut World,
    inp: &mut I,
    u: EntityIdx,
    t: EntityIdx,
) -> R<bool> {
    use super::targets::{self, Field, Site};
    let a = uid(w, u);
    // `ScopedVariableModification.Create(LastTarget, ...)`.
    inp.eng_unit(w, "last", &a)?;
    let saved = if w.combat.targets.on { Some(targets::value(w, u, Field::Last)?) } else { None };
    let v = should_attack_scoped(w, inp, u, t)?;
    if let Some(s) = saved
        && let Some(x) = targets::set(w, u, Field::Last, s, Site::Restore)?
    {
        inp.tgt_write(w, &x)?;
    }
    Ok(v)
}

fn should_attack_scoped<I: EngInputs>(
    w: &mut World,
    inp: &mut I,
    u: EntityIdx,
    t: EntityIdx,
) -> R<bool> {
    use super::targets::{self, Field, Site};
    let (a, b) = (uid(w, u), uid(w, t));
    let tb = w.combat.turn_based;
    if tb {
        // `LastTarget = target`, then `target.IsCurrentUnit() && CurrentTurn.Immune...`.
        if w.combat.targets.on
            && let Some(x) =
                targets::set(w, u, Field::Last, Some(t), Site::ShouldAttackOnDisengage)?
        {
            inp.tgt_write(w, &x)?;
        }
        if is_current_unit(w, t)? {
            // `CurrentTurn.ImmuneAttackOfOpportunityOnDisengage`: the sim's, checked.
            let v = inp.eng_bool(w, "immune", None, None)?;
            match w.combat.turns.immune_on_disengage {
                Some(x) if x == v => {}
                Some(x) => {
                    return desync(format!(
                        "the turn's ImmuneAttackOfOpportunityOnDisengage is {x} in the sim, {v} in the recording"
                    ));
                }
                None => {
                    return unsupported(
                        w,
                        "ImmuneAttackOfOpportunityOnDisengage of a turn the window didn't see start or tick",
                    );
                }
            }
            if v {
                return Ok(false);
            }
        }
    }
    if inp.eng_bool(w, "motion", Some(&a), None)? {
        return Ok(false);
    }
    if has_mechanics_feature(w, u, &["DuelistNoRetreat"]) {
        return Ok(true);
    }
    if eng(w, u)?.disengage_targets.contains(&t) {
        return Ok(false);
    }
    if inp.eng_conc(w, &a, &b)? == TOTAL {
        return Ok(false);
    }
    if eng(w, u)?.engaged_units.len() <= 1 {
        return Ok(true);
    }
    let last = inp.eng_unit(w, "last", &a)?;
    if tb && last.as_deref() != Some(b.as_str()) {
        return desync(format!(
            "ShouldAttackOnDisengage set {a}'s LastTarget to {b}, the recording reads {last:?}"
        ));
    }
    if last.as_deref() == Some(b.as_str()) {
        return Ok(true);
    }
    if inp.eng_bool(w, "tbon", None, None)? && !w.combat.cutscene {
        return Ok(true);
    }
    unsupported(w, "ShouldAttackOnDisengage's real-time branch (the engagement's time)")
}

/// `UnitEntityData.IsCurrentUnit()` (`CombatController.Rider`; mounts aren't modelled).
fn is_current_unit(w: &mut World, u: EntityIdx) -> R<bool> {
    if w.combat.current_unit_maybe_cleared {
        return unsupported(w, "IsCurrentUnit while the current turn may have been cleared");
    }
    let cur = w.combat.current_unit;
    for x in [Some(u), cur].into_iter().flatten() {
        if w.entity(x).parts.iter().any(|p| p.clr == "UnitPartRider" || p.clr == "UnitPartSaddled")
        {
            return unsupported(w, "IsCurrentUnit of a mounted unit (CombatController.Mount)");
        }
    }
    Ok(cur == Some(u))
}

/// `AttackOfOpportunity`'s flags (`simulate` is never set by the code ported).
#[derive(Debug, Clone, Copy, Default)]
struct Flags {
    disengaging: bool,
    after_defensively_fail: bool,
}

/// `UnitCombatState.AttackOfOpportunity(target, tricksterAttack: false, simulate: false, ...)`.
fn attack_of_opportunity<I: EngInputs>(
    w: &mut World,
    inp: &mut I,
    u: EntityIdx,
    t: EntityIdx,
    f: Flags,
) -> R<bool> {
    let ret = aoo_body(w, inp, u, t, f)?;
    let check = AooCheck {
        unit: uid(w, u),
        target: uid(w, t),
        trick: false,
        simulate: false,
        disengaging: f.disengaging,
        after_defensively_fail: f.after_defensively_fail,
        ret,
        count: count(w, u)?,
        attacked_by_aoo: ids(w, eng(w, u)?.attacked_by_aoo.clone()),
    };
    inp.eng_aoo(w, &check)?;
    Ok(ret)
}

fn aoo_body<I: EngInputs>(
    w: &mut World,
    inp: &mut I,
    u: EntityIdx,
    t: EntityIdx,
    f: Flags,
) -> R<bool> {
    let (a, b) = (uid(w, u), uid(w, t));
    if f.disengaging && has_mechanics_feature(w, t, &["DisengageWithoutAttackOfOpportunity"]) {
        return Ok(false);
    }
    if inp.eng_bool(w, "prevent", Some(&a), None)? || inp.eng_bool(w, "prevent", Some(&b), None)? {
        return Ok(false);
    }
    if super::units::is_dead(w, t) {
        return Ok(false);
    }
    let blocked = (!inp.leaf(w, "caic", Of::Unit(Some(a.clone())))?
        && !has_condition(w, u, cond::ATTACK_OF_OPPORTUNITY_BEFORE_INITIATIVE))
        || !can_attack_of_opportunity(w, inp, u)?
        || !inp.leaf(w, "canact", Of::Unit(Some(a.clone())))?;
    if blocked {
        return Ok(false);
    }
    if inp.eng_bool(w, "fmove", Some(&b), None)? {
        return unsupported(w, "AttackOfOpportunity: a target forced to move (UnitPartForceMove)");
    }
    if inp.leaf(w, "untgt", Of::Pair(Some(a.clone()), Some(b.clone())))? {
        return Ok(false);
    }
    if inp.eng_bool(w, "motion", Some(&a), None)? {
        return Ok(false);
    }
    if inp.eng_bool(w, "fmove", Some(&a), None)? {
        return Ok(false);
    }
    let hand = super::commands::aoo::threat_hand(w, u);
    inp.eng_threat(w, u, hand)?;
    if hand.is_none() || count(w, u)? <= 0 {
        return Ok(false);
    }
    if !inp.eng_bool(w, "memc", None, Some(&a))? {
        return Ok(false);
    }
    if blueprint_has(w, u, "AttackOfOpportunityLimits") {
        return unsupported(w, "AttackOfOpportunity: AttackOfOpportunityLimits (the orientation)");
    }
    if has_condition(w, t, cond::IMMUNE_TO_ATTACK_OF_OPPORTUNITY) {
        return Ok(false);
    }
    if eng(w, u)?.attacked_by_aoo.contains(&t) {
        return Ok(false);
    }
    if (f.after_defensively_fail
        && has_condition(w, t, cond::USE_MOBILITY_TO_NEGATE_DEFENSIVELY_FAIL))
        || (f.disengaging
            && has_condition(w, t, cond::USE_MOBILITY_TO_NEGATE_ATTACK_OF_OPPORTUNITY))
    {
        return unsupported(w, "AttackOfOpportunity: the mobility check (RuleCalculateCMD)");
    }
    let Some(set) = w.combat.commands.unit_set.get(&a).copied() else {
        return desync(format!("unit {a}'s command set is unknown"));
    };
    let c = inp.eng_new_run(w, set, &b, false, f.disengaging)?;
    uc::run(w, inp, set, c, false, false)?;
    inp.nested_run_end(w, set, c)?;
    // `IAttackOfOpportunityHandler`: the game log's.
    crate::game_log::attack_of_opportunity(w, u, t);
    let before = w.unit(u).map(|x| x.combat.cooldown).unwrap_or_default();
    let n = count(w, u)?;
    if set_count(w, u, n - 1)? {
        let after = w.unit(u).map(|x| x.combat.cooldown).unwrap_or_default();
        inp.eng_count_write(w, u, before, after, f.disengaging)?;
    }
    eng_mut(w, u)?.attacked_by_aoo.push(t);
    Ok(true)
}

/// `CanAttackOfOpportunity`.
fn can_attack_of_opportunity<I: EngInputs>(w: &mut World, inp: &mut I, u: EntityIdx) -> R<bool> {
    let a = uid(w, u);
    Ok(count(w, u)? > 0
        && !has_condition(w, u, cond::DISABLE_ATTACKS_OF_OPPORTUNITY)
        && !has_condition(w, u, cond::CONFUSION)
        && !inp.eng_bool(w, "passive", None, None)?
        && inp.eng_bool(w, "canatk", Some(&a), None)?)
}

/// `Unit.Blueprint.GetComponent<T>()` for a component type named `name`.
fn blueprint_has(w: &World, u: EntityIdx, name: &str) -> bool {
    w.unit(u).and_then(|x| x.blueprint).and_then(|g| w.content.get(g)).is_some_and(|b| {
        b.components_array().iter().any(|o| wr_content::schema::ty(o.ty).short == name)
    })
}
