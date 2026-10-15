//! The driven replay's side of units' targets ([`crate::combat::targets`]): WrathOracle's
//! observer `targets`. `tgt_init` gives the window's initial values (one block, right after
//! `eng_init`'s, over the same units in order). A write the sim makes inside a lifecycle entry
//! (`UpdateCombatTarget`'s, `ShouldAttackOnDisengage`'s and its restore) or after the driver's
//! click (`tbattack`, then its `Run`) takes the next `tgt_w` where it makes it, its call chain
//! exactly the writer's; leaving combat's `Clear` writes are queued and taken inside that
//! `Clear`, their chain the one `UnitEntityData.LeaveCombat` calls `InterruptAiCommands` from
//! next. The writes that follow an entry from outside the lifecycle are the sim's too, checked
//! after it: `ClickUnitHandler.OnClick`'s after its `Run` (turn-based: the clicked unit, the
//! command's target), `TryCancelCommands`' and `Stop`'s after their `InterruptAll` (both
//! cleared), their chains the entry's. `InterruptActions`' and a destroyed target's aren't
//! modelled: reported. The AI's and the engagement's reads (`last`, `manual`) are checked
//! against the sim's values.

use crate::combat::oracle::Rv;

use super::Driver;
use super::commands::{describe, is_unit, obs, opt_str};
use super::engage::fields;
use crate::combat::commands::R;
use crate::combat::targets::{self, Field, Site, Write};
use crate::world::{EntityIdx, World};

/// The writer of a `tgt_w`, from its call chain (innermost first: the setter, then its caller):
/// one of the sim's, with the number of frames its own code takes after the setter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Writer(Site, usize);

const CS: &str = "Kingmaker.Controllers.Combat.UnitCombatState.";
const UCS: &str = "Kingmaker.UnitLogic.Commands.UnitCommands.";
/// The driver's own write (its frames are WrathOracle's, which a chain leaves out).
const DRIVER: &str = "WrathOracle.TurnBased.IssueHovered";
/// The driver's `Run` (its click's, from its coroutine).
const DRIVER_RUN: &str = "Kingmaker.UnitLogic.Commands.UnitCommands.Run < Kingmaker.UnitLogic.Commands.UnitCommands.Run < UnityEngine.SetupCoroutine.InvokeMoveNext";
const CLICK: &str = "Kingmaker.Controllers.Clicks.Handlers.ClickUnitHandler.OnClick";
const CANCEL: &str = "Kingmaker.EntitySystem.Entities.UnitEntityData.TryCancelCommands";
const STOP: &str = "Kingmaker.EntitySystem.Entities.UnitEntityData.Stop";
/// The caller whose target writes aren't modelled: `SelectionManagerBase.InterruptActions`'.
const INTERRUPT_ACTIONS: &str = "SelectionManagerBase+<>c.<InterruptActions>b__";

/// The frames between the setter and the chain the write is checked against (innermost first):
/// `UpdateCombatTarget`'s down to the innermost `Run`'s recorded chain, the engagement's down to
/// its tick's, `Clear`'s down to `UnitEntityData.LeaveCombat`.
fn site_frames(site: Site) -> &'static [&'static str] {
    match site {
        Site::UpdateCombatTarget => &["UnitCommands.UpdateCombatTarget"],
        Site::UpdateCombatTargetMerge => {
            &["UnitCommands.UpdateCombatTarget", "UnitCommands.TryUseSpecialHandlers"]
        }
        Site::ShouldAttackOnDisengage => &[
            "UnitCombatState.ShouldAttackOnDisengage",
            "UnitCombatState.Disengage",
            "UnitCombatEngagementController.TickUnit",
        ],
        Site::Restore => &[
            "UnitCombatState.<ShouldAttackOnDisengage>b__",
            "ScopedVariableModification.Dispose",
            "UnitCombatState.ShouldAttackOnDisengage",
            "UnitCombatState.Disengage",
            "UnitCombatEngagementController.TickUnit",
        ],
        Site::Clear => &["UnitCombatState.Clear", "UnitCombatState.LeaveCombat"],
        Site::Driver | Site::Click | Site::Cancel | Site::Stop => &[],
    }
}

/// Whether frame `f` is the site frame `pat` names.
fn frame_is(pat: &str, f: &str) -> bool {
    match pat {
        // The scope's lambda and `ScopedVariableModification<UnitEntityData>.Dispose`, as this
        // build names them (another build's names are reported).
        "UnitCombatState.<ShouldAttackOnDisengage>b__" => {
            f.strip_prefix(CS) == Some("<ShouldAttackOnDisengage>b__120_0")
        }
        "ScopedVariableModification.Dispose" => {
            f == "TurnBased.Utility.ScopedVariableModification`1[[Kingmaker.EntitySystem.Entities.UnitEntityData, Assembly-CSharp, Version=0.0.0.0, Culture=neutral, PublicKeyToken=null]].Dispose"
        }
        _ => match pat.split_once('.') {
            Some(("UnitCommands", m)) => f.strip_prefix(UCS) == Some(m),
            Some(("UnitCombatState", m)) => f.strip_prefix(CS) == Some(m),
            Some(("UnitCombatEngagementController", m)) => {
                f.strip_prefix("Kingmaker.Controllers.Combat.UnitCombatEngagementController.")
                    == Some(m)
            }
            _ => false,
        },
    }
}

fn setter(f: Field) -> &'static str {
    match f {
        Field::Last => "Kingmaker.Controllers.Combat.UnitCombatState.set_LastTarget",
        Field::Manual => "Kingmaker.Controllers.Combat.UnitCombatState.set_ManualTarget",
    }
}

fn writer(f: Field, origin: &str) -> Option<Writer> {
    if origin == DRIVER {
        return (f == Field::Manual).then_some(Writer(Site::Driver, 0));
    }
    if origin.ends_with("...") {
        return None;
    }
    let frames: Vec<&str> = origin.split(" < ").collect();
    if frames.first() != Some(&setter(f)) {
        return None;
    }
    match frames.get(1).copied() {
        Some(CLICK) if f == Field::Manual => return Some(Writer(Site::Click, 0)),
        Some(CANCEL) => return Some(Writer(Site::Cancel, 0)),
        Some(STOP) => return Some(Writer(Site::Stop, 0)),
        _ => {}
    }
    for site in [
        Site::UpdateCombatTargetMerge,
        Site::UpdateCombatTarget,
        Site::ShouldAttackOnDisengage,
        Site::Restore,
        Site::Clear,
    ] {
        let p = site_frames(site);
        if frames.len() > p.len() && p.iter().zip(&frames[1..]).all(|(pat, x)| frame_is(pat, x)) {
            return Some(Writer(site, p.len()));
        }
    }
    None
}

/// A `tgt_w` read: unit, field, value, writer and chain; `None` when it can't be read or its
/// writer isn't one the sim models.
fn parse(w: &World, r: &Rv) -> Option<(EntityIdx, Field, Option<EntityIdx>, Writer, String)> {
    let f = field(&r["f"])?;
    let origin = opt_str(r, "origin")??;
    let writer = writer(f, origin)?;
    let (true, Some(u), Some(v)) =
        (fields(r, &["u", "f", "v", "origin"]), unit_of(w, &r["u"]), unit_or_null(w, &r["v"]))
    else {
        return None;
    };
    Some((u, f, v, writer, origin.to_owned()))
}

/// A click's `at` (`TurnBased.IssueHovered`): the unit's and the target's positions, what the
/// pointer is on and its handler, the predicted path's point count and (with points) its end.
fn click_at(v: &Rv) -> bool {
    let vec3 = |x: &Rv| {
        x.as_array().is_some_and(|a| {
            a.len() == 3 && a.iter().all(|f| f.as_f64().is_some_and(|f| (f as f32).is_finite()))
        })
    };
    let Some(o) = v.as_object() else { return false };
    // (`List<Vector3>.Count`, a C# `int`, or -1 without a path.)
    let n = v["path_points"].as_i64().filter(|n| i32::try_from(*n).is_ok());
    let with_end = n.is_some_and(|n| n > 0);
    let keys = ["from", "target_at", "pointer_on", "pointer_handler", "path_points"];
    o.len() == keys.len() + usize::from(with_end)
        && keys.iter().all(|k| o.contains_key(*k))
        && vec3(&v["from"])
        && vec3(&v["target_at"])
        && (v["pointer_on"].is_string() || v["pointer_on"].is_null())
        && (v["pointer_handler"].is_string() || v["pointer_handler"].is_null())
        && n.is_some_and(|n| n >= -1)
        && (!with_end || vec3(&v["path_end"]))
}

/// A frame as WrathOracle's short chains write it (`Type.Method`, the type's own name).
fn short(f: &str) -> String {
    let (ty, m) = f.rsplit_once('.').unwrap_or(("", f));
    let ty = ty.rsplit(['.', '+']).next().unwrap_or(ty);
    format!("{ty}.{m}")
}

fn field(v: &Rv) -> Option<Field> {
    match v.as_str()? {
        "last" => Some(Field::Last),
        "manual" => Some(Field::Manual),
        _ => None,
    }
}

/// A unit's id the sim has, or an explicit null.
fn unit_or_null(w: &World, v: &Rv) -> Option<Option<EntityIdx>> {
    match v {
        Rv::Null => Some(None),
        Rv::String(s) if is_unit(w, s) => Some(w.by_id.get(s.as_str()).copied()),
        _ => None,
    }
}

fn unit_of(w: &World, v: &Rv) -> Option<EntityIdx> {
    v.as_str().filter(|s| is_unit(w, s)).and_then(|s| w.by_id.get(s).copied())
}

/// An open `UnitCombatState.Clear` (after its `cd_clear`): its unit and the chains below
/// `UnitEntityData.LeaveCombat` of the writes it took.
#[derive(Debug, Clone, Default)]
pub(super) struct TgtClear {
    u: Option<EntityIdx>,
    tails: Vec<String>,
}

/// A driver's input that runs a command, whose `Run` hasn't come yet (with its frame).
#[derive(Debug, Clone)]
pub(super) struct TgtClick {
    t: i64,
    /// Its record.
    at: usize,
    input: DriverInput,
}

/// What the driver's input runs: a click (`tbattack`: the unit's attack on the target, then its
/// `ManualTarget`), the plan's `move` (a `UnitMoveTo` to the point) or `attack:` (an attack on
/// the target).
#[derive(Debug, Clone)]
pub(super) enum DriverInput {
    Click {
        unit: String,
        target: String,
    },
    /// `tbcast`: the driver's cast of a memorized spell (`UnitUseAbility.CreateCastCommand`,
    /// `Run`) by `unit` on `target`, the spell its slot holds.
    Cast {
        unit: String,
        target: String,
        ability: wr_content::Guid,
        spellbook: wr_content::Guid,
        level: i32,
    },
    Move {
        to: [f32; 3],
    },
    Attack {
        target: String,
    },
}

impl Driver<'_> {
    /// `tgt_init` (record `i`): a unit's targets at the window's first tick, in one block right
    /// after `eng_init`'s, over the same units in the same order.
    pub(super) fn tgt_init(&mut self, w: &mut World, i: usize) {
        let r = &self.trace.records[i];
        if !self.tgt {
            w.coverage.unsupported("driven replay: a tgt_init record without the targets observer");
            return;
        }
        let u = unit_of(w, &r["u"]);
        let (last, manual) = (unit_or_null(w, &r["last"]), unit_or_null(w, &r["manual"]));
        let (true, Some(u), Some(last), Some(manual)) =
            (fields(r, &["u", "last", "manual"]), u, last, manual)
        else {
            w.coverage.unsupported("driven replay: a tgt_init record it can't read");
            return;
        };
        if self.tgt_init_closed {
            w.coverage
                .unsupported("driven replay: a tgt_init record after the window's first frame");
            return;
        }
        let prev = i.checked_sub(1).map(|j| &self.trace.records[j]);
        let n = self.tgt_init_n;
        self.tgt_init_n += 1;
        let before = if n == 0 { "eng_init" } else { "tgt_init" };
        if !prev.is_some_and(|p| obs(p, before).is_some()) || self.tgt_init_units.get(n) != Some(&u)
        {
            w.coverage.unsupported(
                "driven replay: a tgt_init record out of its block (after eng_init, in its units' order)",
            );
            return;
        }
        let t = &mut w.unit_mut(u).expect("a unit").combat.tgt;
        if t.known {
            w.coverage.unsupported("driven replay: a unit's targets given twice");
            return;
        }
        *t = targets::UnitTargets { known: true, last, manual };
    }

    /// `eng_init` of `u`: the order `tgt_init` follows. None may come once the targets' block
    /// started.
    pub(super) fn tgt_eng_init(&mut self, w: &mut World, u: EntityIdx) {
        if !self.tgt {
            return;
        }
        if self.tgt_init_n > 0 {
            w.coverage.unsupported("driven replay: an eng_init record inside the targets' block");
        }
        self.tgt_init_units.push(u);
    }

    /// `tgt_w` (record `i`) outside a write the sim makes where it makes it: leaving combat's
    /// `Clear` (queued, taken inside its `Clear`), or reported.
    pub(super) fn tgt_w(&mut self, w: &mut World, i: usize) {
        let r = &self.trace.records[i];
        if !self.tgt {
            w.coverage.unsupported("driven replay: a tgt_w record without the targets observer");
            return;
        }
        let Some((u, f, v, Writer(site, n), origin)) = parse(w, r) else {
            let o = r["origin"].as_str().unwrap_or("");
            let ui = o.contains(INTERRUPT_ACTIONS)
                || o.contains(
                    " < Kingmaker.Controllers.Combat.UnitCombatState.HandleEntityDestruction < ",
                );
            w.coverage.unsupported(if ui {
                format!(
                    "driven replay: a target write of the UI's or a destruction's (not modelled) ({})",
                    describe(r)
                )
            } else {
                format!(
                    "driven replay: a tgt_w record it can't read, or of a writer the sim doesn't model ({})",
                    describe(r)
                )
            });
            self.tgt_lost(w, r);
            return;
        };
        let known = w.unit(u).is_some_and(|x| x.combat.tgt.known);
        if self.uc_skipping {
            // After a desync: the recording's state taken.
            if known {
                self.tgt_take(w, &Write { u, f, v, site });
            }
            return;
        }
        if site != Site::Clear {
            w.coverage.unsupported(format!(
                "driven replay: a target write the sim didn't make ({})",
                describe(r)
            ));
            self.tgt_take(w, &Write { u, f, v, site });
            return;
        }
        let want = Write { u, f, v, site };
        // Inside its unit's `Clear` (opened by its `cd_clear`), from that unit's `LeaveCombat`
        // (its `leave_combat`'s callers); the rest of its chain is checked when the `Clear` ends.
        let frames: Vec<&str> = origin.split(" < ").collect();
        let tail = frames[1 + n..].join(" < ");
        let by: Vec<String> = frames[1 + n..].iter().take(3).map(|f| short(f)).collect();
        let placed = self.tgt_clear.u == Some(u)
            && self.tgt_leave_by.get(&u).is_some_and(|b| *b == by.join("<"));
        match w.combat.targets.pending.pop_front() {
            Some(x) if x == want && placed => {
                self.tgt_clear.tails.push(tail);
                self.out.tgt_checked += 1;
            }
            x => {
                w.combat.targets.pending.clear();
                w.coverage.unsupported(format!(
                    "driven replay: the sim's target write ({x:?}) differs from the recording's, or isn't in its Clear ({})",
                    describe(r)
                ));
                self.tgt_take(w, &want);
            }
        }
    }

    /// A write the sim made: the next record is its `tgt_w`, of the same unit, field, value and
    /// writer, its call chain exactly the setter, the writer's frames and the chain it's checked
    /// against (the innermost `Run`'s, or the engagement tick's); the driver's, its own.
    pub(super) fn tgt_pull(&mut self, w: &mut World, x: &Write) -> R<()> {
        // The setter runs right where the sim writes: nothing is recorded before it, but (after
        // a public `Run` returned, for a click's or the driver's write) WrathOracle's `cmd` note.
        let mut k = self.i;
        while matches!(x.site, Site::Click | Site::Driver)
            && self.trace.records.get(k).is_some_and(|r| r["k"] == "cmd" && r["ev"] == "run")
        {
            k += 1;
        }
        if self.trace.records.get(k).is_none_or(|r| obs(r, "tgt_w").is_none()) {
            let d = self.trace.records.get(k).map_or_else(|| "its end".to_owned(), describe);
            return self
                .uc_reject(k, format!("the sim's target write {x:?}, the recording has {d}"));
        }
        let at = self.uc_pull(w, "a target write", |r| obs(r, "tgt_w").is_some())?;
        let r = &self.trace.records[at];
        let base = match x.site {
            Site::UpdateCombatTarget | Site::UpdateCombatTargetMerge => {
                self.tgt_runs.last().cloned()
            }
            Site::ShouldAttackOnDisengage | Site::Restore => Some(self.uc_origin.clone()),
            // The entry's chain without its own frames (`Run`'s two, `InterruptAll`'s one).
            Site::Click => Some(self.uc_origin.splitn(3, " < ").nth(2).unwrap_or("").to_owned()),
            Site::Cancel | Site::Stop => {
                Some(self.uc_origin.split_once(" < ").map_or("", |x| x.1).to_owned())
            }
            Site::Clear | Site::Driver => None,
        };
        let ok = parse(w, r).is_some_and(|(u, f, v, Writer(site, n), origin)| {
            if (Write { u, f, v, site }) != *x {
                return false;
            }
            if site == Site::Driver {
                return origin == DRIVER;
            }
            let Some(base) = base.filter(|b| !b.is_empty()) else { return false };
            let frames: Vec<&str> = origin.split(" < ").collect();
            let base: Vec<&str> = base.split(" < ").collect();
            frames.len() == 1 + n + base.len() && frames.ends_with(&base)
        });
        if !ok {
            let d = describe(r);
            return self
                .uc_reject(at, format!("the sim's target write {x:?}, the recording has {d}"));
        }
        self.out.tgt_checked += 1;
        Ok(())
    }

    /// After a reported write: the recording's value taken, when the unit's targets are known.
    fn tgt_take(&mut self, w: &mut World, x: &Write) {
        if let Some(t) = w.unit_mut(x.u).map(|y| &mut y.combat.tgt).filter(|t| t.known) {
            match x.f {
                Field::Last => t.last = x.v,
                Field::Manual => t.manual = x.v,
            }
        }
    }

    /// An unreadable write: its unit's targets are no longer established.
    fn tgt_lost(&mut self, w: &mut World, r: &Rv) {
        if let Some(u) = unit_of(w, &r["u"])
            && let Some(x) = w.unit_mut(u)
        {
            x.combat.tgt.known = false;
        }
    }

    /// A read of `LastTarget` or `ManualTarget` (`f`: `last`, `manual`) of `u` the recording has
    /// as `v` (record `at`): with the targets modelled, checked against the sim's.
    pub(super) fn tgt_read(
        &mut self,
        w: &World,
        at: usize,
        f: &str,
        u: &str,
        v: Option<&str>,
    ) -> R<()> {
        if !self.tgt {
            return Ok(());
        }
        let Some(field) = field(&Rv::from(f)) else { return Ok(()) };
        let Some(e) = w.by_id.get(u).copied() else {
            return self.uc_reject(at, format!("a read of {f} of {u}, which the sim doesn't have"));
        };
        let sim = match targets::value(w, e, field) {
            Ok(x) => x.map(|x| w.entity(x).unique_id.clone()),
            Err(d) => return self.uc_reject(at, d.0),
        };
        if sim.as_deref() != v {
            return self
                .uc_reject(at, format!("{u}'s {f} is {sim:?} in the sim, {v:?} in the recording"));
        }
        self.out.tgt_reads += 1;
        Ok(())
    }

    /// The end of a frame: every write the sim made was recorded, every click had its `Run`,
    /// and after the window's first frame every unit's targets are established (`tgt_init`, or
    /// constructed since).
    pub(super) fn tgt_frame_end(&mut self, w: &mut World) {
        if !self.tgt {
            return;
        }
        self.tgt_close_clear(w, None);
        if !w.combat.targets.pending.is_empty() {
            w.combat.targets.pending.clear();
            w.coverage.unsupported(
                "driven replay: a UnitCombatState.Clear target write the recording doesn't have",
            );
        }
        if self.tgt_click.take().is_some() {
            w.coverage.unsupported("driven replay: a driver's input without its Run");
        }
        self.tgt_runs.clear();
        if !self.tgt_init_closed && self.uc_rows.ready {
            self.tgt_init_closed = true;
            if self.tgt_init_n != self.tgt_init_units.len() {
                w.coverage.unsupported(
                    "driven replay: the initial targets (tgt_init) don't cover eng_init's units",
                );
            }
            for u in crate::scene::units_pool(w) {
                if !w.unit(u).is_some_and(|x| x.combat.tgt.known) {
                    w.coverage.unsupported(
                        "driven replay: a unit without its initial targets (tgt_init)",
                    );
                }
            }
        }
    }

    /// `cd_clear` of `UnitCombatState.Clear` for `u`: its target writes come next.
    pub(super) fn tgt_clear_open(&mut self, w: &mut World, u: EntityIdx) {
        if self.tgt {
            self.tgt_close_clear(w, None);
            self.tgt_clear = TgtClear { u: Some(u), tails: Vec::new() };
        }
    }

    /// A record that ends an open `Clear`'s target writes: anything but a target write (nothing
    /// else is recorded between `Cooldowns.Clear` and them).
    pub(super) fn tgt_clear_boundary(&mut self, w: &mut World, r: &Rv) {
        if self.tgt_clear.u.is_some() && obs(r, "tgt_w").is_none() {
            self.tgt_close_clear(w, Some(r));
        }
    }

    /// The end of an open `Clear` at record `r` (`None`: the frame's end): the sim's writes of it
    /// must all have come, and when it took any, `r` is the `InterruptAiCommands` its
    /// `UnitEntityData.LeaveCombat` calls next, from the same chain.
    fn tgt_close_clear(&mut self, w: &mut World, r: Option<&Rv>) {
        let TgtClear { u: Some(u), tails } = std::mem::take(&mut self.tgt_clear) else { return };
        if w.combat.targets.pending.iter().any(|x| x.u == u) {
            w.combat.targets.pending.retain(|x| x.u != u);
            w.coverage.unsupported(
                "driven replay: a UnitCombatState.Clear target write the recording doesn't have",
            );
        }
        if tails.is_empty() {
            return;
        }
        let next = r.filter(|r| {
            obs(r, "uc_x").is_some()
                && r["fn"] == "InterruptAiCommands"
                && r["u"].as_str() == Some(w.entity(u).unique_id.as_str())
        });
        let chain = next.and_then(|r| r["origin"].as_str()).and_then(|o| {
            o.strip_prefix("Kingmaker.UnitLogic.Commands.UnitCommands.InterruptAiCommands < ")
        });
        if !chain.is_some_and(|c| tails.iter().all(|t| t == c)) {
            w.coverage.unsupported(
                "driven replay: a UnitCombatState.Clear's target writes aren't from the LeaveCombat that calls InterruptAiCommands next",
            );
        }
    }

    /// `leave_combat` of `u`: its callers (`by`), which its `Clear`'s writes come from.
    pub(super) fn tgt_leave(&mut self, u: EntityIdx, by: &str) {
        if self.tgt {
            self.tgt_leave_by.insert(u, by.to_owned());
        }
    }

    /// A driver's input that runs a command (record `i`): a click (`tbattack`), the plan's `move`
    /// or `attack:<guid>` step, each with exactly its writer's fields. Its `Run` comes next.
    pub(super) fn tgt_driver_input(&mut self, w: &mut World, i: usize) {
        if !self.tgt {
            return;
        }
        let r = &self.trace.records[i];
        let unit = |k: &str| {
            let v = &r[k];
            v.as_object()
                .filter(|o| o.len() == 1)
                .and_then(|_| v["$unit"].as_str())
                .filter(|s| is_unit(w, s))
                .map(str::to_owned)
        };
        let point = || -> Option<[f32; 3]> {
            let a = r["to"].as_array().filter(|a| a.len() == 3)?;
            let v: Vec<f32> = a
                .iter()
                .map(|x| x.as_f64().map(|f| f as f32).filter(|f| f.is_finite()))
                .collect::<Option<_>>()?;
            Some([v[0], v[1], v[2]])
        };
        let has = |extra: &[&str]| {
            const BASE: [&str; 5] = ["seq", "k", "t", "step", "frame"];
            r.as_object().is_some_and(|o| {
                o.len() == BASE.len() + extra.len()
                    && BASE.iter().chain(extra).all(|k| o.contains_key(*k))
            }) && r["frame"].is_u64()
        };
        // Its frame's (the tick record's), and the turns the window started (`turn_index`).
        let frame = self.frame_at.map(|f| &self.trace.records[f]["frame"]);
        let frame_ok = frame.is_some_and(|f| f.is_u64() && *f == r["frame"]);
        let turns = self.trace.records[..i]
            .iter()
            .filter(|x| x["k"] == "tb" && x["ev"] == "turn_start")
            .count();
        let step = r["step"].as_str().unwrap_or("");
        let input = match step {
            // `ClickUnitHandler.CreateCommand`'s attack, in the sim's current round.
            "tbattack" => unit("unit")
                .zip(unit("target"))
                .filter(|_| {
                    has(&["unit", "target", "policy", "round", "turn_index", "command", "at"])
                        && r["command"] == "UnitAttack"
                        && r["policy"].is_string()
                        && r["turn_index"].as_u64() == Some(turns as u64)
                        && click_at(&r["at"])
                        && r["round"].as_i64() == Some(i64::from(w.combat.turns.round))
                })
                .map(|(unit, target)| DriverInput::Click { unit, target }),
            "move" => point().filter(|_| has(&["to"])).map(|to| DriverInput::Move { to }),
            // The cast's `at`: the click's, with the pointer's mode.
            "tbcast" => unit("unit")
                .zip(unit("target"))
                .zip(r["ability"].as_str().and_then(wr_content::Guid::parse))
                .zip(r["spellbook"].as_str().and_then(wr_content::Guid::parse))
                .zip(r["level"].as_i64().and_then(|l| i32::try_from(l).ok()))
                .filter(|_| {
                    let at = &r["at"];
                    let mut click = at.clone();
                    let mode = click.as_object_mut().and_then(|o| o.remove("pointer_mode"));
                    has(&[
                        "unit",
                        "target",
                        "ability",
                        "slot",
                        "level",
                        "spellbook",
                        "policy",
                        "round",
                        "turn_index",
                        "command",
                        "type",
                        "at",
                    ]) && r["command"] == "UnitUseAbility"
                        && r["slot"] == "memorized"
                        && r["type"].as_u64()
                            == Some(u64::from(crate::combat::commands::ty::STANDARD))
                        && r["policy"].is_string()
                        && r["turn_index"].as_u64() == Some(turns as u64)
                        && r["round"].as_i64() == Some(i64::from(w.combat.turns.round))
                        && mode.is_some_and(|m| m.is_string())
                        && click_at(&click)
                })
                .map(|((((unit, target), ability), spellbook), level)| DriverInput::Cast {
                    unit,
                    target,
                    ability,
                    spellbook,
                    level,
                }),
            // `Driver.Attack`: the target is a unit of the step's blueprint.
            _ => step
                .strip_prefix("attack:")
                .and_then(wr_content::Guid::parse)
                .zip(unit("target"))
                .filter(|(g, t)| {
                    has(&["target"])
                        && w.by_id
                            .get(t.as_str())
                            .and_then(|e| w.unit(*e))
                            .is_some_and(|u| u.blueprint == Some(*g))
                })
                .map(|(_, target)| DriverInput::Attack { target }),
        };
        let (Some(input), Some(t), true) = (input, r["t"].as_i64(), frame_ok) else {
            w.coverage.unsupported(format!("driven replay: a driver's {step} input it can't read"));
            return;
        };
        if self.tgt_click.is_some() {
            w.coverage.unsupported("driven replay: a driver's input without its Run");
        }
        self.tgt_click = Some(TgtClick { t, at: i, input });
    }

    /// A top-level entry (record `at`): `InterruptActions`' call (its target writes aren't
    /// modelled) is reported. The first `Run` after the driver's click is its own (from its
    /// coroutine, of its unit, in its frame; a driver's `Run` without a click is reported): the
    /// click's target is returned.
    pub(super) fn tgt_entry(&mut self, w: &mut World, at: usize) -> Option<DriverInput> {
        if !self.tgt {
            return None;
        }
        let r = &self.trace.records[at];
        let origin = r["origin"].as_str().unwrap_or("");
        if origin.contains(INTERRUPT_ACTIONS) {
            w.coverage.unsupported(
                "driven replay: SelectionManagerBase.InterruptActions (its target writes aren't modelled)",
            );
        }
        obs(r, "uc_run")?;
        let driver = origin == DRIVER_RUN;
        let Some(pending) = self.tgt_click.take() else {
            if driver {
                w.coverage.unsupported("driven replay: the driver's Run without its input");
            }
            return None;
        };
        let unit_ok = match &pending.input {
            DriverInput::Click { unit, .. } | DriverInput::Cast { unit, .. } => {
                r["u"].as_str() == Some(unit.as_str())
            }
            DriverInput::Move { .. } | DriverInput::Attack { .. } => true,
        };
        // Right after the input: its new command's first sight (`uc_new`), then the `Run`.
        let c = r["c"].as_u64();
        let adjacent = pending.at + 2 == at && {
            let x = &self.trace.records[pending.at + 1];
            obs(x, "uc_new").is_some() && x["c"].as_u64() == c
        };
        if !driver || !unit_ok || !adjacent || r["t"].as_i64() != Some(pending.t) {
            w.coverage.unsupported("driven replay: a driver's input without its Run");
            return None;
        }
        Some(pending.input)
    }

    /// After a top-level entry of unit `u` (record `at`) ended: the target writes its caller
    /// makes next. `ClickUnitHandler.OnClick` after its `Run` (turn-based: `ManualTarget` = the
    /// clicked unit, the attack's target), `TryCancelCommands` (no command was running: `running`,
    /// before the entry) and `Stop` after their `InterruptAll` (both cleared).
    pub(super) fn tgt_after_entry(
        &mut self,
        w: &mut World,
        at: usize,
        u: EntityIdx,
        c: Option<u32>,
        running: Option<bool>,
    ) -> R<()> {
        use crate::combat::commands::Desync;
        if !self.tgt {
            return Ok(());
        }
        let r = &self.trace.records[at];
        let origin = r["origin"].as_str().unwrap_or("");
        let frames: Vec<&str> = origin.split(" < ").collect();
        let writes: Vec<(Field, Option<EntityIdx>, Site)> = if obs(r, "uc_run").is_some()
            && frames.get(2) == Some(&CLICK)
        {
            let x = c.and_then(|c| w.combat.commands.cmds.get(&c));
            let target = x
                .filter(|x| x.is(crate::combat::commands::class::ATTACK))
                .and_then(|x| x.s.target.unit())
                .and_then(|t| w.by_id.get(t).copied());
            match target {
                Some(t) if w.combat.turn_based => vec![(Field::Manual, Some(t), Site::Click)],
                _ => {
                    return Err(Desync(
                        "ClickUnitHandler.OnClick out of turn-based combat or without an attack (not modelled)"
                            .into(),
                    ));
                }
            }
        } else if obs(r, "uc_x").is_some()
            && r["fn"] == "InterruptAll"
            && matches!(frames.get(1), Some(&CANCEL | &STOP))
        {
            let site = if frames.get(1) == Some(&CANCEL) { Site::Cancel } else { Site::Stop };
            if site == Site::Cancel && running != Some(false) {
                return Err(Desync(
                    "TryCancelCommands' InterruptAll while the unit's commands were running".into(),
                ));
            }
            vec![(Field::Last, None, site), (Field::Manual, None, site)]
        } else {
            return Ok(());
        };
        for (f, v, site) in writes {
            if let Some(x) = targets::set(w, u, f, v, site)? {
                self.tgt_pull(w, &x)?;
            }
        }
        Ok(())
    }

    /// After the driver's `Run` of command `c` (its unit `u`): the command its input runs (a
    /// click's: `ClickUnitHandler.CreateCommand`'s attack on the clicked unit), then a click's
    /// `unit.CombatState.ManualTarget = target`.
    pub(super) fn tgt_driver_after(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        c: u32,
        input: &DriverInput,
    ) -> R<()> {
        use crate::combat::commands::{Desync, Target, class};
        let Some(x) = w.combat.commands.cmds.get(&c) else {
            return Err(Desync(format!("the driver's command {c} isn't described")));
        };
        let ok = match input {
            DriverInput::Click { target, .. } | DriverInput::Attack { target } => {
                x.is(class::ATTACK) && x.s.target.unit() == Some(target.as_str())
            }
            DriverInput::Move { to } => {
                x.is(class::MOVE_TO)
                    && matches!(x.s.target, Target::Point(p) if p.map(f32::to_bits) == to.map(f32::to_bits))
            }
            // `CreateCastCommand`: a plain `UnitUseAbility` of the spell's runtime action type.
            DriverInput::Cast { target, .. } => {
                x.class == class::USE_ABILITY
                    && x.s.ty == crate::combat::commands::ty::STANDARD
                    && x.s.target.unit() == Some(target.as_str())
                    && x.ability.is_some()
            }
        };
        if !ok {
            return Err(Desync(format!(
                "the driver's Run (command {c}) isn't its input's ({input:?})"
            )));
        }
        let DriverInput::Click { target, .. } = input else { return Ok(()) };
        let Some(t) = w.by_id.get(target.as_str()).copied() else {
            return Err(Desync(format!("the click's target {target} is unknown")));
        };
        match targets::set(w, u, Field::Manual, Some(t), Site::Driver)? {
            Some(x) => self.tgt_pull(w, &x),
            None => Ok(()),
        }
    }

    /// `eng_init` (record `i`): with the targets observed, the oracle's fixed sequence: right after
    /// `uc_snap`, the units in ordinal id order (`StateDump.AllUnits`).
    pub(super) fn tgt_eng_init_place(&mut self, w: &mut World, i: usize) {
        if !self.tgt {
            return;
        }
        let recs = &self.trace.records;
        let prev = i.checked_sub(1).map(|j| &recs[j]);
        let first = self.tgt_init_units.is_empty();
        let placed = prev.is_some_and(|p| {
            if first { obs(p, "uc_snap").is_some() } else { obs(p, "eng_init").is_some() }
        });
        let ordered = prev.filter(|_| !first).is_none_or(
            |p| matches!((p["u"].as_str(), recs[i]["u"].as_str()), (Some(a), Some(b)) if a < b),
        );
        if !placed || !ordered {
            w.coverage.unsupported(
                "driven replay: an eng_init record out of the oracle's sequence (after uc_snap, in ordinal id order)",
            );
        }
    }

    /// A `Run` starts (record `at`: its chain is what `UpdateCombatTarget`'s writes are checked
    /// against) or ends.
    pub(super) fn tgt_run_push(&mut self, at: usize) {
        if self.tgt {
            let o = self.trace.records[at]["origin"].as_str().unwrap_or("").to_owned();
            self.tgt_runs.push(o);
        }
    }

    pub(super) fn tgt_run_pop(&mut self) {
        self.tgt_runs.pop();
    }
}
