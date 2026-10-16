//! `UnitMoveTo`'s class code (POC-5 slice C; `docs/poc5/commands.md`), run as lifecycle code when
//! the recording has WrathOracle's `approach` observer: `Init` and `OnStart` (the base ones),
//! `OnRun`, `OnAction`, and its answers to the lifecycle (`AwaitMovementFinish`,
//! `DontWaitForHands`, the base ones otherwise), each checked against the recorded leaf.
//!
//! Inputs: the AI action's `PreventMovementFinish` (`pmf`). `OnAction`'s `DesiredOrientation`
//! write is the sim's (`docs/poc5/orientation.md`). Not modelled: `CombatState.ReturnOrientation`
//! and `ReturnPosition` (read by the return to the starting position, a recorded run), and a
//! rider's target (mounts).

use super::approach::Call;
use super::{Inputs, Of, R, cmd, cmd_mut, executor, on_run_base, res};
use crate::combat::orientation::{LookArg, LookFn};
use crate::world::World;

/// What the creator set (`uc_new`'s `mv`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fields {
    pub roaming: bool,
    pub prevent_interrupt: bool,
    /// `m_UseRiderTarget` (a mount's).
    pub rider_target: bool,
    pub orientation: Option<f32>,
    /// `m_OverrideAwaitMovementFinish`.
    pub await_override: Option<bool>,
    pub movement_delay: f32,
}

impl Fields {
    /// `Roaming PreventInterrupt UseRiderTarget|Orientation|await override|MovementDelay`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let p: Vec<&str> = s.split('|').collect();
        let [f, o, a, d] = p[..] else { return None };
        let [roaming, prevent_interrupt, rider_target] = super::flags::<3>(f)?;
        let orientation = if o == "-" { None } else { Some(super::finite(o)?) };
        let await_override = if a == "-" { None } else { Some(super::flags::<1>(a)?[0]) };
        Some(Self {
            roaming,
            prevent_interrupt,
            rider_target,
            orientation,
            await_override,
            movement_delay: super::finite(d)?,
        })
    }
}

fn fields(w: &World, c: u32) -> R<Fields> {
    match cmd(w, c)?.mv {
        Some(f) => Ok(f),
        None => super::desync(format!("command {c} has no UnitMoveTo fields (mv)")),
    }
}

/// `Init(executor)` (the base one).
pub fn init(w: &mut World, c: u32, owner: Option<String>) -> R<()> {
    cmd_mut(w, c)?.s.executor = owner;
    Ok(())
}

/// `OnRun()`: the base one, then an AI's command of a non-player unit in combat sets the unit's
/// return position and orientation to its target (not modelled).
pub fn on_run<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<()> {
    on_run_base(w, inp, c)?;
    let u = executor(w, c)?;
    if super::super::is_in_combat(w, u)
        && !super::super::units::is_player_faction(w, u)
        && cmd(w, c)?.ai
    {
        w.coverage.stub(
            "UnitMoveTo.OnRun: CombatState.ReturnPosition/ReturnOrientation (read by the return to the starting position, a recorded run)",
        );
    }
    Ok(())
}

/// `OnStart()` (the base one): a move triggers its animation (`TriggerAnimation`), which
/// `UnitMoveTo` doesn't override (none).
pub fn on_start(w: &World, c: u32) -> R<()> {
    fields(w, c).map(|_| ())
}

/// `OnAction()`: the view stops, the orientation is set (`Executor.DesiredOrientation =
/// Orientation.Value` when it has one), and the move succeeds unless its AI action prevents the
/// finish.
pub fn on_action<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<u8> {
    let u = executor(w, c)?;
    let unit = w.entity(u).unique_id.clone();
    inp.call(w, &Call::Stop { unit, c: Some(c) })?;
    if let Some(o) = fields(w, c)?.orientation {
        inp.look(
            w,
            u,
            super::LookSite::MoveToAction,
            LookFn::Desired,
            Some(LookArg::Value(o)),
            false,
        )?;
    }
    if cmd(w, c)?.ai && inp.leaf(w, "pmf", Of::Cmd(c))? {
        return Ok(res::NONE);
    }
    Ok(res::SUCCESS)
}

/// Its answer to a lifecycle getter `f` (`None`: not one it answers).
pub fn answer(w: &World, c: u32, f: &str) -> R<Option<bool>> {
    let x = cmd(w, c)?;
    Ok(match f {
        "await" => Some(fields(w, c)?.await_override.unwrap_or(true)),
        "dwfh" => Some(true),
        _ => super::base_answer(x, f),
    })
}

/// Whether the sim runs this command's class code.
#[must_use]
pub fn ported(w: &World, c: &super::Cmd) -> bool {
    w.combat.commands.approach && c.class == super::class::MOVE_TO
}
