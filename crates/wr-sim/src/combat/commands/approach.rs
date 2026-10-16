//! `UnitCommand.TickApproaching` (POC-5 slice C; `docs/poc5/commands.md`), run as base lifecycle
//! code when the recording has WrathOracle's `approach` observer.
//!
//! The movement agent is an input: what the lifecycle reads from it is a leaf (`WantsToMove`, the
//! view's `IsMoving`), and its calls into it (`View.MoveTo`, `AgentASP.FollowPrecomputedPath`,
//! `View.StopMoving`) are [`Call`]s, checked against the recorded ones where the sim knows their
//! arguments. Also inputs: the paths (the command's `ForcedPath`, the path visualizer's), the
//! turn's movement and the unit's `State.CanMove`, `UnitCanGetUpOnCommand`, positions and line of
//! sight (`IsUnitCloseEnough`), a target unit's position (the approach point).

use super::{
    Inputs, Of, R, Target, class, class_code, cmd, cmd_mut, current_turn, executor, interrupt,
    should_approach, target_in_world, turn_based, ty, unsupported,
};
use crate::csharp::timespan::TimeSpan;
use crate::world::World;

/// The lifecycle's calls into the movement agent (`uc_call`); `None` where the sim doesn't know an
/// argument (a target unit's position; `UnitAttack`'s approach radius, from corpulences and the
/// weapon's reach).
#[derive(Debug, Clone, PartialEq)]
pub enum Call {
    /// `View.MoveTo(command, point, approachRadius, maxApproachRadius, targetView, distanceXZ)` of
    /// `unit`'s view; `target`: the view's unit (the target's, or the executor's own).
    MoveTo {
        c: u32,
        unit: String,
        point: Option<[f32; 3]>,
        radius: Option<f32>,
        max_radius: Option<f32>,
        target: String,
        xz: bool,
    },
    /// `AgentASP.FollowPrecomputedPath(path, forcedPath, maxApproachRadius)` of `unit`'s agent.
    Follow { c: u32, unit: String, path: Vec<[f32; 3]>, forced: bool, radius: Option<f32> },
    /// `View.StopMoving()` of `unit`'s view, within command `c`'s code (`None`: the controller's).
    Stop { unit: String, c: Option<u32> },
}

/// What a command's description says of its approach (`uc_new`'s `ap`): `ApproachRadius`,
/// `m_MaxApproachRadius`, `DistanceXZ`, `NeedLoS`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub radius: f32,
    pub max_radius: f32,
    pub xz: bool,
    pub los: bool,
}

impl Params {
    /// `ApproachRadius|m_MaxApproachRadius|DistanceXZ NeedLoS`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let p: Vec<&str> = s.split('|').collect();
        let [r, m, f] = p[..] else { return None };
        let [xz, los] = super::flags::<2>(f)?;
        Some(Self { radius: super::finite(r)?, max_radius: super::finite(m)?, xz, los })
    }
}

/// Whether the sim runs `c`'s class code, so the approach fields it described at first sight are
/// still its own: any other class's recorded code may change them (`UnitInteractWithUnit.Init` sets
/// `ApproachRadius`) or override their getters (`MaxApproachRadius`, `GetTargetPoint`).
fn own_fields(w: &World, c: u32) -> R<bool> {
    let x = cmd(w, c)?;
    Ok(super::move_to::ported(w, x) || super::aoo::ported(w, x))
}

/// `ApproachRadius` as the sim knows it (a `UnitAttack` sets it in `Init` and `UpdateTarget` from
/// the units' corpulences, their views, and the weapon's reach, which aren't modelled).
fn approach_radius(w: &World, c: u32) -> R<Option<f32>> {
    if !own_fields(w, c)? {
        return Ok(None);
    }
    Ok(cmd(w, c)?.ap.map(|a| a.radius))
}

/// `MaxApproachRadius`: `Max(ApproachRadius, m_MaxApproachRadius)`, a `UnitAttack`'s `1e6`
/// without an AI action.
fn max_approach_radius(w: &World, c: u32) -> R<Option<f32>> {
    let x = cmd(w, c)?;
    if x.is(class::ATTACK) && !x.ai {
        return Ok(Some(1_000_000.0));
    }
    let Some(ap) = x.ap else { return Ok(None) };
    Ok(approach_radius(w, c)?.map(|r| r.max(ap.max_radius)))
}

/// `ApproachPoint` (`GetTargetPoint()`: `Target?.Point ?? Executor.Position`): a point target's
/// point, for the classes that don't override it; a unit's position (and the executor's) is an
/// input.
fn approach_point(w: &mut World, c: u32) -> R<Option<[f32; 3]>> {
    if !own_fields(w, c)? && !cmd(w, c)?.is(class::ATTACK) {
        return Ok(None);
    }
    // `UnitMoveTo.GetTargetPoint` with `m_UseRiderTarget`: a rider's target (mounts).
    if cmd(w, c)?.mv.is_some_and(|m| m.rider_target) {
        return unsupported(w, "UnitMoveTo: a rider's target (m_UseRiderTarget)");
    }
    Ok(match &cmd(w, c)?.s.target {
        Target::Point(p) => Some(*p),
        _ => None,
    })
}

/// `TickApproaching()`.
pub fn tick_approaching<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<()> {
    let x = cmd(w, c)?.clone();
    if x.is(class::MOVE_ALONG_PATH) {
        return unsupported(w, "TickApproaching: UnitMoveAlongPath");
    }
    let u = executor(w, c)?;
    let uid = w.entity(u).unique_id.clone();
    // `(!ShouldUnitApproach && (Free || Swift)) || (SaddledPart != null && AiAction != null &&
    // !IsRiderCommand)`.
    if !should_approach(w, inp, c)? && matches!(x.s.ty, ty::FREE | ty::SWIFT) {
        return Ok(());
    }
    if inp.leaf(w, "spart", Of::Cmd(c))? {
        return unsupported(w, "TickApproaching: a mount (SaddledPart)");
    }
    if x.overrides("OnTickApproaching") {
        class_code(w, inp, "OnTickApproaching", c, None)?;
    }
    // `bool flag = Executor.RiderPart`.
    if inp.leaf(w, "rpart", Of::Cmd(c))? {
        return unsupported(w, "TickApproaching: a rider (RiderPart)");
    }
    if !should_approach(w, inp, c)? {
        cmd_mut(w, c)?.s.finished_approaching = true;
        inp.call(w, &Call::Stop { unit: uid, c: Some(c) })?;
        // `if (!flag && ShouldTurnToTarget) TurnToTarget()` (`flag`, a rider, returned above).
        if super::answer(w, inp, "stt", c)? {
            super::turn_to_target(w, inp, c, super::LookSite::TickApproaching)?;
        }
        if cmd(w, c)?.s.move_unit {
            let v = inp.approach_draw(w, c)?;
            let Ok(t) = TimeSpan::from_seconds(f64::from(v))
                .and_then(|d| TimeSpan(w.clock.game_time.0).checked_add(d))
            else {
                return unsupported(w, "TickApproaching: NextApproachTime out of range");
            };
            cmd_mut(w, c)?.s.next_approach = t.0;
        }
        return Ok(());
    }
    let tb = turn_based(w);
    if w.clock.game_time.0 < cmd(w, c)?.s.next_approach {
        // In turn-based combat, a unit that doesn't want to move and can't: interrupted.
        if tb && current_turn(w).is_some() && !inp.leaf(w, "wants", Of::Cmd(c))? {
            let stuck = !inp.leaf(w, "hasmove", Of::Cmd(c))?
                || (!inp.leaf(w, "canmove", Of::Cmd(c))? && !inp.leaf(w, "ucgu", Of::Cmd(c))?);
            if stuck {
                // The log message reads both again.
                inp.leaf(w, "hasmove", Of::Cmd(c))?;
                inp.leaf(w, "canmove", Of::Cmd(c))?;
                interrupt(w, inp, c, true)?;
            }
        }
        return Ok(());
    }
    let s = &cmd(w, c)?.s;
    if s.finished_approaching && !s.started && x.ai {
        return interrupt(w, inp, c, true);
    }
    let s = &mut cmd_mut(w, c)?.s;
    s.finished_approaching = false;
    s.move_unit = true;
    // `(tb && Executor.IsInCombat) | TacticalCombatHelper.IsActive`.
    if inp.leaf(w, "tact", Of::Cmd(c))? {
        return unsupported(w, "TickApproaching: tactical combat");
    }
    if tb && super::super::is_in_combat(w, u) {
        let mut path = inp.path(w, "fpath", c)?;
        // An AI command's `ForcedPath` is the path the sim's AI chose (slice D).
        if let Some(a) = w.combat.ai.commands.get(&c) {
            let bits = |p: &Option<Vec<[f32; 3]>>| {
                p.as_ref().map(|v| v.iter().flat_map(|x| x.map(f32::to_bits)).collect::<Vec<_>>())
            };
            if bits(&a.forced_path) != bits(&path) {
                return super::desync(format!(
                    "command {c}'s ForcedPath isn't the path the sim's AI chose"
                ));
            }
        }
        if path.is_none() && inp.leaf(w, "pvis", Of::Cmd(c))? {
            path = inp.path(w, "vpath", c)?;
        }
        let Some(path) = path.filter(|p| !p.is_empty()) else {
            // (and `RiderCommand?.Interrupt()`: a mount's, not modelled.)
            if x.rider {
                return unsupported(w, "TickApproaching: a rider command without a path");
            }
            return interrupt(w, inp, c, true);
        };
        if current_turn(w).is_some() {
            w.combat.turns.ctl.saved_command_path = Some(path.clone());
        }
        let radius = approach_radius(w, c)?;
        inp.call(w, &Call::Follow { c, unit: uid, path, forced: false, radius })?;
        cmd_mut(w, c)?.s.next_approach = TimeSpan::MAX.0;
        return Ok(());
    }
    // `targetUnit?.View.Or(null) ?? Executor.View` (a target without a live view: the executor's).
    let target = match target_in_world(w, c)? {
        Some(t) if inp.leaf(w, "tview", Of::Cmd(c))? => t,
        _ => uid.clone(),
    };
    let call = Call::MoveTo {
        c,
        unit: uid,
        point: approach_point(w, c)?,
        radius: approach_radius(w, c)?,
        max_radius: max_approach_radius(w, c)?,
        target,
        xz: x.ap.is_some_and(|a| a.xz),
    };
    inp.call(w, &call)
}
