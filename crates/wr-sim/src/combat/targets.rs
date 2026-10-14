//! Units' combat targets (decompiled, not copied: `Kingmaker.Controllers.Combat.UnitCombatState`'s
//! `LastTarget` and `ManualTarget`). docs/poc5/commands.md, slice F.
//!
//! Neither is saved: the window's initial values are recorded (WrathOracle's `tgt_init`), and a
//! unit constructed since starts with neither. The sim's writers are `UnitCommands.UpdateCombatTarget`
//! ([`super::commands`]), `ShouldAttackOnDisengage` (its turn-based write and its scope's restore,
//! [`super::engage`]) and `Clear()` (leaving combat, [`super::life`]). Each write that changes a
//! field is checked against the recording's (`tgt_w`): one made inside a lifecycle entry where it's
//! made (`Inputs::tgt_write`), `Clear`'s queued ([`Targets::pending`]) and checked in order; the
//! other writers (the UI's, the driver's, a destroyed target's) are the recording's inputs.
//! `LastTarget`'s setter raises `ICombatTargetChangeHandler`, which nothing implements.

use std::collections::VecDeque;

use super::commands::{Desync, R};
use crate::world::{EntityIdx, World};

/// `LastTarget` or `ManualTarget`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Last,
    Manual,
}

impl Field {
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Last => "last",
            Self::Manual => "manual",
        }
    }
}

/// Who writes: the sim's writers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Site {
    /// `UpdateCombatTarget` from `Run`.
    UpdateCombatTarget,
    /// `UpdateCombatTarget` from `TryUseSpecialHandlers` (a command merged into the running one).
    UpdateCombatTargetMerge,
    /// `ShouldAttackOnDisengage`'s `LastTarget = target`.
    ShouldAttackOnDisengage,
    /// `ShouldAttackOnDisengage`'s `ScopedVariableModification` restoring `LastTarget`.
    Restore,
    /// `UnitCombatState.Clear()` (from `LeaveCombat`).
    Clear,
    /// WrathOracle's driver after its click's `Run` (`ClickUnitHandler.OnClick`'s
    /// `ManualTarget = target`; a driven replay's input).
    Driver,
    /// `ClickUnitHandler.OnClick` after its `Run` (the turn controller's predictions click).
    Click,
    /// `UnitEntityData.TryCancelCommands` after its `InterruptAll` (combat's end).
    Cancel,
    /// `UnitEntityData.Stop` after its `InterruptAll`.
    Stop,
}

/// A write that changed a field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Write {
    pub u: EntityIdx,
    pub f: Field,
    pub v: Option<EntityIdx>,
    pub site: Site,
}

/// A unit's targets.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnitTargets {
    /// The values are the game's: the window's initial ones adopted (`tgt_init`), or the unit
    /// constructed since.
    pub known: bool,
    pub last: Option<EntityIdx>,
    pub manual: Option<EntityIdx>,
}

/// The targets' model.
#[derive(Debug, Default)]
pub struct Targets {
    /// The model runs (a recording with WrathOracle's `targets` observer).
    pub on: bool,
    /// `Clear()`'s writes not yet matched with the recording's, in order.
    pub pending: VecDeque<Write>,
}

fn uid(w: &World, u: EntityIdx) -> &str {
    &w.entity(u).unique_id
}

/// Unit `u`'s targets, when established.
pub fn get(w: &World, u: EntityIdx) -> R<UnitTargets> {
    match w.unit(u) {
        Some(x) if x.combat.tgt.known => Ok(x.combat.tgt),
        Some(_) => Err(Desync(format!("unit {}'s targets aren't established", uid(w, u)))),
        None => Err(Desync("the targets of an entity that isn't a unit".to_owned())),
    }
}

/// A field's value, when established.
pub fn value(w: &World, u: EntityIdx, f: Field) -> R<Option<EntityIdx>> {
    let t = get(w, u)?;
    Ok(match f {
        Field::Last => t.last,
        Field::Manual => t.manual,
    })
}

/// The setter of `f` (`LastTarget` returns when the value is the same; `ManualTarget` assigns,
/// and set to null from a value clears the rider's and the mount's: mounts are reported). The
/// change, if any, for its check.
pub fn set(
    w: &mut World,
    u: EntityIdx,
    f: Field,
    v: Option<EntityIdx>,
    site: Site,
) -> R<Option<Write>> {
    let old = value(w, u, f)?;
    if old == v {
        return Ok(None);
    }
    if f == Field::Manual
        && v.is_none()
        && w.entity(u).parts.iter().any(|p| p.clr == "UnitPartRider" || p.clr == "UnitPartSaddled")
    {
        w.coverage.unsupported("targets: ManualTarget cleared on a rider or a mount");
        return Err(Desync("ManualTarget cleared on a rider or a mount (not modelled)".to_owned()));
    }
    let t = &mut w.unit_mut(u).expect("a unit").combat.tgt;
    match f {
        Field::Last => t.last = v,
        Field::Manual => t.manual = v,
    }
    Ok(Some(Write { u, f, v, site }))
}

/// `UnitCombatState.Clear()`'s targets (leaving combat): `LastTarget = null; ManualTarget = null`.
pub fn clear(w: &mut World, u: EntityIdx) {
    if !w.combat.targets.on {
        return;
    }
    let mut res = Ok(());
    for f in [Field::Last, Field::Manual] {
        match set(w, u, f, None, Site::Clear) {
            Ok(Some(x)) => w.combat.targets.pending.push_back(x),
            Ok(None) => {}
            Err(d) => {
                res = Err(d);
                break;
            }
        }
    }
    if let Err(Desync(d)) = res {
        w.coverage.unsupported(format!("targets: Clear: {d}"));
        if let Some(x) = w.unit_mut(u) {
            x.combat.tgt.known = false;
        }
    }
}
