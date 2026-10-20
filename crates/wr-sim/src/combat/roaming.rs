//! `UnitPartRoaming` and `UnitRoamingController` (`docs/poc5/orientation.md`, "Roaming"): a
//! roaming unit's next point, its idle time, and the controller's tick, which rolls a point of
//! the unit's own around its origin, waits there, steps a sleeping unit toward its next point
//! (with its `LookAt`), and runs a `UnitMoveTo` for an awake one.
//!
//! What it reads and makes outside the unit's own state goes through [`RoamInputs`] (the
//! driven replay pulls the records the game wrote and checks the sim's against them).
//! Arithmetic is the game's Mono's ([`crate::csharp::arith`]): each `float` expression in
//! double, rounded where it is stored.
//!
//! Modelled: a unit with its own points (`RoamingPoint`: `SpawnerRoamingSettings` in `Radius`
//! mode, or none). Reported where they're reached: scene waypoints (`RoamingWaypointData`:
//! `Patrol` mode), idle cutscenes (the settings', a point's, one playing), a rider or mount.

use wr_nav::cs;

use super::commands::{Desync, R};
use super::orientation::{V3, set_position};
use crate::csharp::TimeSpan;
use crate::csharp::arith::store_f32;
use crate::rng::CallSite;
use crate::world::{EntityIdx, World};

/// `RollNextPoint`'s draws.
pub const SITE_ROLL_CIRCLE: CallSite = CallSite(
    "Assembly-CSharp|Kingmaker.Controllers.Units.UnitRoamingController|RollNextPoint(UnitPartRoaming)|#0@75|get_insideUnitCircle",
);
pub const SITE_ROLL_IDLE: CallSite = CallSite(
    "Assembly-CSharp|Kingmaker.Controllers.Units.UnitRoamingController|RollNextPoint(UnitPartRoaming)|#1@125|Range(float,float)",
);

fn d(x: f32) -> f64 {
    f64::from(x)
}

/// `UnitAnimationActionLocoMotion.WalkSpeedType`.
pub mod walk {
    pub const SLOW: i64 = 0;
    pub const NORMAL: i64 = 1;
    pub const CHARGE: i64 = 2;
    pub const STEALTH: i64 = 3;
}

/// An `IRoamingPoint`.
#[derive(Debug, Clone, PartialEq)]
pub enum Point {
    /// `RoamingPoint` (`RollNextPoint`'s, or a saved one): `SelectNextPoint` and
    /// `SelectPrevPoint` return null, `SelectIdleTime` its `IdleTime`, `SelectCutscene` its
    /// `IdleCutscene` (whether one is set), `Orientation` null.
    Own { position: V3, idle_time: f32, idle_cutscene: bool },
    /// `RoamingWaypointData` (a scene waypoint, its unique id): not modelled.
    Waypoint(String),
}

impl Point {
    #[must_use]
    pub fn position(&self) -> Option<V3> {
        match self {
            Self::Own { position, .. } => Some(*position),
            Self::Waypoint(_) => None,
        }
    }
}

/// `RoamingUnitSettings` (not saved: `SpawnerRoamingSettings.Part.OnInitialize` sets it when the
/// spawner's view attaches).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    pub radius: f32,
    pub min_idle: f32,
    pub max_idle: f32,
    /// `IdleCutscenes`' count.
    pub idle_cutscenes: usize,
    /// `MovementType` ([`walk`]).
    pub movement_type: i64,
    /// `Sleepless` (not read by the controller: `SleepingUnitsController` reads the unit's
    /// retained flag).
    pub sleepless: bool,
}

/// `UnitPartRoaming`.
#[derive(Debug, Clone, PartialEq)]
pub struct Part {
    /// `NextPoint` (`m_NextPoint`; the saved `m_PersistentNextPoint`/`m_PersistentNextPointId`).
    pub next: Option<Point>,
    /// `m_Disabled` (`SetEnabled`).
    pub disabled: bool,
    pub original: V3,
    pub reverse: bool,
    /// `IdleCutscene != null` (a cutscene player).
    pub idle_cutscene: bool,
    pub idle_end: TimeSpan,
    pub settings: Option<Settings>,
    /// A saved next point of another type (reported where the controller reads it).
    pub unknown_next: Option<String>,
}

/// What the controller's tick reads or makes outside the unit's roaming state, in the game's
/// order.
pub trait RoamInputs {
    /// `RollNextPoint`'s start (WrathOracle's `roam_roll`), before its draws.
    fn roll(&mut self, w: &mut World, u: EntityIdx, settings: Option<Settings>) -> R<()>;
    /// The roll's draws as the sim made them (`insideUnitCircle`, `Range(min, max)`), against
    /// their records.
    fn drew(&mut self, circle: [f32; 2], idle: f32, min: f32, max: f32) -> R<()>;
    /// `ObstacleAnalyzer.TraceAlongNavmesh(start, end)`.
    fn trace_along_navmesh(&mut self, w: &mut World, start: V3, end: V3) -> R<V3>;
    /// `UnitEntityData.IsSleeping`.
    fn sleeping(&mut self, w: &mut World, u: EntityIdx) -> R<bool>;
    /// The sleeping step's `unit.Position += …` (`value`, before the setter's equality test):
    /// the position after, or `None` when the sim can't establish the step (`why`).
    fn step(&mut self, w: &mut World, u: EntityIdx, value: Result<V3, String>) -> R<()>;
    /// `unit.LookAt(point)`.
    fn look_at(&mut self, w: &mut World, u: EntityIdx, point: V3) -> R<()>;
    /// `unit.Commands.Run(new UnitMoveTo(point, 1f, distanceXZ: true) { MovementType, Orientation,
    /// Roaming = true })`.
    fn run_move(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        point: V3,
        orientation: Option<f32>,
        movement_type: i64,
    ) -> R<()>;
    /// The unit waits at its point (`IsPlayingIdle`).
    fn idle(&mut self) {}
    /// The unit reached its point.
    fn arrived(&mut self) {}
}

fn part(w: &World, u: EntityIdx) -> R<&Part> {
    match w.unit(u).and_then(|x| x.combat.roaming.as_deref()) {
        Some(p) => Ok(p),
        None => Err(Desync("the unit's roaming state isn't known".into())),
    }
}

fn part_mut(w: &mut World, u: EntityIdx) -> R<&mut Part> {
    match w.unit_mut(u).and_then(|x| x.combat.roaming.as_deref_mut()) {
        Some(p) => Ok(p),
        None => Err(Desync("the unit's roaming state isn't known".into())),
    }
}

fn has_part(w: &World, u: EntityIdx, clr: &str) -> bool {
    w.entity(u).parts.iter().any(|p| p.clr == clr)
}

/// `UnitRoamingController.ShouldTickOnUnit` (the controller's `TickSleeping`: every unit of the
/// pool): conscious, no commands, not in a dialogue, not in combat, with `UnitPartRoaming`.
/// `Err`: a gate the sim can't establish (the commands of a unit it doesn't track, a dialogue).
pub fn should_tick_on_unit(w: &World, u: EntityIdx, dialog: bool) -> Result<bool, String> {
    if !super::units::is_conscious(w, u) {
        return Ok(false);
    }
    let id = &w.entity(u).unique_id;
    match w.combat.commands.empty(id) {
        Some(true) => {}
        Some(false) => return Ok(false),
        None => return Err("the unit's commands aren't known".into()),
    }
    if dialog {
        // `DialogController.InvolvedUnits` (not modelled in combat replays).
        return Err("a dialogue's involved units".into());
    }
    if super::is_in_combat(w, u) {
        return Ok(false);
    }
    Ok(has_part(w, u, "UnitPartRoaming"))
}

/// `UnitRoamingController.TickOnUnit(unit)`. `game_time` and `delta` are `TimeController`'s
/// `GameTime` and `DeltaTime`. Cutscene players aren't modelled: `CutsceneControlledUnit
/// .GetControllingPlayer(unit)` is taken as null (a unit a cutscene holds doesn't roam: every
/// roll, step and move the sim would make instead is a record it pulls, so the difference is
/// reported; waiting at a point changes nothing either way).
///
/// # Errors
///
/// A record or input differs from the sim's, or the state reaches what isn't modelled.
pub fn tick_on_unit<I: RoamInputs>(
    w: &mut World,
    inp: &mut I,
    u: EntityIdx,
    game_time: TimeSpan,
    delta: f32,
) -> R<()> {
    let p = part(w, u)?;
    // `Disable || GetControllingPlayer(unit) != null || unitPartRoaming.Disabled`: a roaming
    // move to interrupt (`ShouldTickOnUnit` left only units without commands: none).
    if p.disabled || has_part(w, u, "UnitPartFollowUnit") {
        return Ok(());
    }
    if let Some(t) = &p.unknown_next {
        return Err(Desync(format!("a saved next point of a type not modelled ({t})")));
    }
    if p.next.is_none() {
        let next = roll_next_point(w, inp, u)?;
        part_mut(w, u)?.next = Some(next);
    }
    if is_playing_idle(w, u, game_time)? {
        inp.idle();
        return Ok(());
    }
    let Some(next) = part(w, u)?.next.clone() else { return Ok(()) };
    let Some(np) = next.position() else {
        return Err(Desync("a scene waypoint (RoamingWaypointData: not modelled)".into()));
    };
    let pos = w.unit(u).map_or([0.0; 3], |x| x.position);
    let num = sqr_distance_2d(np, pos);
    let sleeping = inp.sleeping(w, u)?;
    let movement_type = part(w, u)?.settings.map(|s| s.movement_type);
    if sleeping {
        let step = sleeping_step(w, u, np, pos, num, movement_type, delta);
        inp.step(w, u, step)?;
        inp.look_at(w, u, np)?;
    }
    if num > 1.0 {
        if !sleeping {
            // `MovementType = Settings?.MovementType ?? Normal`; `Orientation =
            // nextPoint.Orientation` (null for the unit's own points).
            let mt = part(w, u)?.settings.map_or(walk::NORMAL, |s| s.movement_type);
            inp.run_move(w, u, np, None, mt)?;
        }
        return Ok(());
    }
    // Arrived: `IdleEndTime = GameTime + SelectIdleTime()`, `SelectCutscene()`, `AdvancePoint`.
    let Point::Own { idle_time, idle_cutscene, .. } = next else { unreachable!("an own point") };
    let idle = TimeSpan::from_seconds(d(idle_time))
        .and_then(|t| game_time.checked_add(t))
        .map_err(|e| Desync(format!("IdleEndTime: {e:?}")))?;
    if idle_cutscene {
        return Err(Desync(
            "a point's idle cutscene (CutscenePlayerView.Play: not modelled)".into(),
        ));
    }
    let p = part_mut(w, u)?;
    p.idle_end = idle;
    // `AdvancePoint`: an own point has no next or previous one, so the direction turns and
    // the next point is null.
    p.reverse = !p.reverse;
    p.next = None;
    inp.arrived();
    Ok(())
}

/// `IsPlayingIdle(part)`: waiting until `IdleEndTime`; then (no idle cutscene playing) both are
/// cleared.
fn is_playing_idle(w: &mut World, u: EntityIdx, game_time: TimeSpan) -> R<bool> {
    let p = part_mut(w, u)?;
    if p.idle_end > game_time {
        return Ok(true);
    }
    if p.idle_cutscene {
        return Err(Desync("an idle cutscene playing (CutscenePlayerData: not modelled)".into()));
    }
    p.idle_end = TimeSpan::ZERO;
    Ok(false)
}

/// `RollNextPoint(roaming)`: a point of the unit's own, `OriginalPoint + insideUnitCircle
/// .To3D() * Radius` traced along the navmesh, idle for `Random.Range(MinIdleTime,
/// MaxIdleTime)`, with an idle cutscene drawn from the settings' (none modelled).
fn roll_next_point<I: RoamInputs>(w: &mut World, inp: &mut I, u: EntityIdx) -> R<Point> {
    let p = part(w, u)?;
    let settings = p.settings;
    let original = p.original;
    inp.roll(w, u, settings)?;
    let radius = settings.map_or(0.0, |s| s.radius);
    let (min, max) = settings.map_or((0.0, 0.0), |s| (s.min_idle, s.max_idle));
    let [x, y] = w.rng.inside_unit_circle(SITE_ROLL_CIRCLE);
    let end = cs::add(original, cs::mul([x, 0.0, y], radius));
    let end = inp.trace_along_navmesh(w, original, end)?;
    let idle_time = w.rng.range_float(SITE_ROLL_IDLE, min, max);
    inp.drew([x, y], idle_time, min, max)?;
    // `settings?.IdleCutscenes.Random()`: no draw from an empty list.
    if settings.is_some_and(|s| s.idle_cutscenes > 0) {
        return Err(Desync("the settings' idle cutscenes (RollNextPoint: not modelled)".into()));
    }
    Ok(Point::Own { position: end, idle_time, idle_cutscene: false })
}

/// `GeometryUtils.SqrDistance2D(a, b)`.
#[must_use]
pub fn sqr_distance_2d(a: V3, b: V3) -> f32 {
    let x = a[0] - b[0];
    let z = a[2] - b[2];
    store_f32(d(x) * d(x) + d(z) * d(z))
}

/// A sleeping unit's step toward its next point: `Position += (next - Position).normalized *
/// Math.Min(Math.Sqrt(num), speed * DeltaTime)` with `CurrentSpeedMps`, halved for a `Slow` or
/// `Stealth` movement type (none without settings); the value given to the `Position` setter.
fn sleeping_step(
    w: &World,
    u: EntityIdx,
    np: V3,
    pos: V3,
    num: f32,
    movement_type: Option<i64>,
    delta: f32,
) -> Result<V3, String> {
    let normalized = cs::normalize(cs::sub(np, pos));
    let val = store_f32(d(num).sqrt());
    let mut speed = current_speed_mps(w, u)?;
    if matches!(movement_type, Some(walk::SLOW | walk::STEALTH)) {
        speed = store_f32(d(speed) * 0.5);
    }
    let step = val.min(store_f32(d(speed) * d(delta)));
    Ok(cs::add(pos, cs::mul(normalized, step)))
}

/// The position after a step's `value`, through the `Position` setter.
#[must_use]
pub fn stepped(before: V3, value: V3) -> V3 {
    set_position(before, value)
}

/// `Feet.Meters / 3f` (`ModifiableValueSpeed.Mps`, `GameConsts.MinUnitSpeedMps`).
fn feet_mps(feet: f32) -> f32 {
    let meters = store_f32(d(feet) * d(0.3048_f32));
    store_f32(d(meters) / 3.0)
}

/// `UnitEntityData.CurrentSpeedMps` of a unit without commands (`CalculateCurrentSpeed` with
/// neither a move nor a standard command: `ModifiedSpeedMps`, at least `MinUnitSpeedMps`).
/// The memo (`m_MemoizedSpeed`, one per frame outside tactical combat) is taken as computed
/// here. `Err`: what isn't modelled.
///
/// # Errors
///
/// A rider or mount, a player-faction unit out of combat (`NonCombatBaseSpeedFeet`), a mobility
/// condition, the weather's slowdown, no speed stat.
pub fn current_speed_mps(w: &World, u: EntityIdx) -> Result<f32, String> {
    if has_part(w, u, "UnitPartRider") || has_part(w, u, "UnitPartSaddled") {
        return Err("a rider or mount's speed".into());
    }
    let min = feet_mps(5.0);
    // `BaseSpeedMps`: cheats are off in the oracle's build (its `startup.json` enables none).
    if super::units::is_player_faction(w, u) && !super::is_in_combat(w, u) {
        return Err("a player-faction unit's NonCombatBaseSpeedFeet".into());
    }
    let Some(stat) = w.unit(u).and_then(|x| x.stats.get(crate::stats::types::StatType::Speed))
    else {
        return Err("no speed stat".into());
    };
    #[allow(clippy::cast_precision_loss)]
    let base = feet_mps(stat.modified_value as f32);
    let modifier = speed_modifier(w, u)?;
    let modified = store_f32(d(base) * d(modifier)).max(min);
    // (`!IsInCombat && IsControllerGamepad && IsDirectlyControllable`: not a player's unit.)
    Ok(modified.max(min))
}

/// `UnitEntityData.CalculateSpeedModifier()`.
fn speed_modifier(w: &World, u: EntityIdx) -> Result<f32, String> {
    use crate::unit_state::{cond, has_condition};
    if w.player.capital_party_mode() {
        return Ok(1.0);
    }
    // `IsInStealth` isn't tracked (`RuleEnterStealth` sets it; saved false for every unit of the
    // fixtures): taken as false. The step it would halve is checked against its record.
    let mut num = 1.0_f32;
    for c in [cond::STAGGERED, SLOWED, cond::ENTANGLED, cond::DIFFICULT_TERRAIN] {
        if has_condition(w, u, c) {
            num = store_f32(d(num) * 0.5);
        }
    }
    if has_condition(w, u, USE_MOBILITY) {
        return Err("UseMobilityToNegateAttackOfOpportunity (TricksterMobilityFastMovement)".into());
    }
    let slowdown = w.content.get(crate::unit::BLUEPRINT_ROOT).map(|r| {
        r.view()
            .obj("WeatherSettings")
            .and_then(|s| s.has("UseSlowdown").then(|| s.bool("UseSlowdown")))
    });
    match slowdown {
        Some(Some(false)) => Ok(num),
        Some(Some(true)) => Err("the weather's slowdown (WeatherSettings.UseSlowdown)".into()),
        _ => Err("BlueprintRoot.WeatherSettings unknown".into()),
    }
}

/// `UnitCondition.Slowed`, `UseMobilityToNegateAttackOfOpportunity`.
const SLOWED: i32 = 10;
const USE_MOBILITY: i32 = 36;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_from_feet() {
        // 30 ft: 9.144 m in 3 s; 5 ft (the minimum) 0.508.
        assert_eq!(feet_mps(30.0), 3.048);
        assert_eq!(
            feet_mps(5.0).to_bits(),
            store_f32(d(store_f32(5.0 * d(0.3048_f32))) / 3.0).to_bits()
        );
    }

    #[test]
    fn sqr_distance_is_xz() {
        assert_eq!(sqr_distance_2d([1.0, 5.0, 2.0], [0.0, -3.0, 0.0]), 5.0);
    }
}
