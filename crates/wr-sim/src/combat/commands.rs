//! Units' commands (POC-5 round 8, slice A; `docs/poc5/commands.md`): `UnitCommands` (a unit's
//! slots, queue and parked command), `UnitCommandController`'s tick, `UnitCommand`'s base
//! lifecycle (run, start, tick, act, end, interrupt) and the turn's running commands.
//!
//! What this code reads outside itself is an input, taken in the order the game evaluated it
//! ([`Inputs`]: in a driven replay, WrathOracle's `uc_*` records): positions and line of sight
//! (`IsUnitCloseEnough`), the view and the movement agent, the hands, a class's answers to the
//! lifecycle's virtual members, and the class code it calls (`OnStart`, `OnTick`, `OnAction`,
//! `TickApproaching`, `Init`, an overridden `OnRun`), whose effect on the base state is recorded.
//! Everything else is computed: the slots, the queue, the base state machine, the turn-based gates
//! (turn, status, cooldowns, `CanAct`), the running list, `SpendAction`.
//!
//! With WrathOracle's `attacks` observer `UnitAttack`'s class code is the sim's ([`attack`]); with
//! `approach`, `TickApproaching` ([`approach`]) and the class code of `UnitMoveTo` ([`move_to`])
//! and `UnitAttackOfOpportunity` ([`aoo`]), whose answers to the lifecycle's getters are checked
//! against the recorded leaves.

use std::collections::{HashMap, VecDeque};

use crate::world::{EntityIdx, World};

pub mod aoo;
pub mod approach;
pub mod attack;
pub mod move_to;

/// `UnitCommand.CommandType`.
pub mod ty {
    pub const FREE: u8 = 0;
    pub const STANDARD: u8 = 1;
    pub const SWIFT: u8 = 2;
    pub const MOVE: u8 = 3;
}

/// `UnitCommand.ResultType`.
pub mod res {
    pub const NONE: u8 = 0;
    pub const FAIL: u8 = 1;
    pub const INTERRUPT: u8 = 2;
    pub const SUCCESS: u8 = 3;
}

/// A command's target (`TargetWrapper`): a unit by id, or a point.
#[derive(Debug, Clone, PartialEq)]
pub enum Target {
    None,
    Unit(String),
    Point([f32; 3]),
}

impl Target {
    #[must_use]
    pub fn unit(&self) -> Option<&str> {
        match self {
            Self::Unit(u) => Some(u),
            _ => None,
        }
    }

    /// WrathOracle's form: empty (no target), or the unit's id or `@x,y,z` (finite round-trip floats)
    /// then `#` and the wrapper's id.
    #[must_use]
    pub fn parse(s: &str) -> Option<(Self, Option<u32>)> {
        if s.is_empty() {
            return Some((Self::None, None));
        }
        let (what, wrapper) = s.rsplit_once('#')?;
        let wrapper: u32 = wrapper.parse().ok()?;
        if let Some(p) = what.strip_prefix('@') {
            let v: Vec<f32> = p.split(',').map(finite).collect::<Option<_>>()?;
            let [x, y, z] = v[..] else { return None };
            return Some((Self::Point([x, y, z]), Some(wrapper)));
        }
        if what.is_empty() {
            return None;
        }
        Some((Self::Unit(what.to_owned()), Some(wrapper)))
    }
}

/// A command's base state, as WrathOracle's row has it (`CommandObs.Row`).
#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub started: bool,
    pub acted: bool,
    pub finished: bool,
    /// `Result` ([`res`]).
    pub result: u8,
    pub time_since_start: f32,
    /// `m_IsAnimationActHandled`.
    pub act_handled: bool,
    /// `HasAnimation`.
    pub has_animation: bool,
    /// `Animation != null`.
    pub animation: bool,
    /// The animation handle's `IsActed` and `IsFinished` (`None` without one). They change with
    /// the animation between frames: read only right after class code returns.
    pub anim_acted: Option<bool>,
    pub anim_finished: Option<bool>,
    pub pretend_act_time: f32,
    pub hand_wait: f32,
    /// `NextApproachTime` (ticks).
    pub next_approach: i64,
    pub finished_approaching: bool,
    pub move_unit: bool,
    pub did_run: bool,
    /// `InterruptAsSoonAsPossible`.
    pub asap: bool,
    pub skip_animation: bool,
    /// `Type` ([`ty`]).
    pub ty: u8,
    pub target: Target,
    /// The target's `TargetWrapper` (its id; `None` without a target): one wrapper equals itself
    /// without `TargetWrapper.Equals`.
    pub target_ref: Option<u32>,
    /// `Executor` (unit id).
    pub executor: Option<String>,
}

fn flag(c: u8) -> Option<bool> {
    match c {
        b'0' => Some(false),
        b'1' => Some(true),
        _ => None,
    }
}

fn flags<const N: usize>(s: &str) -> Option<[bool; N]> {
    let b = s.as_bytes();
    if b.len() != N {
        return None;
    }
    let mut out = [false; N];
    for (o, c) in out.iter_mut().zip(b) {
        *o = flag(*c)?;
    }
    Some(out)
}

fn finite(s: &str) -> Option<f32> {
    s.parse::<f32>().ok().filter(|v| v.is_finite())
}

impl State {
    /// `started acted finished|result|TimeSinceStart|actHandled HasAnimation animation animActed
    /// animFinished|PretendActTime|HandAnimationWaitTime|NextApproachTime|FinishedApproaching
    /// IsMoveUnit DidRun|asap skip|type|target|executor`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let p: Vec<&str> = s.split('|').collect();
        let [saf, result, tsc, anim, pretend, hand, next, approach, asap, ty, target, exec] = p[..]
        else {
            return None;
        };
        let [started, acted, finished] = flags::<3>(saf)?;
        let a = anim.as_bytes();
        if a.len() != 5 {
            return None;
        }
        let (act_handled, has_animation, animation) = (flag(a[0])?, flag(a[1])?, flag(a[2])?);
        let opt = |c: u8| if c == b'-' { Some(None) } else { flag(c).map(Some) };
        let (anim_acted, anim_finished) = (opt(a[3])?, opt(a[4])?);
        if animation != anim_acted.is_some() || animation != anim_finished.is_some() {
            return None;
        }
        let [finished_approaching, move_unit, did_run] = flags::<3>(approach)?;
        let [asap, skip_animation] = flags::<2>(asap)?;
        let result: u8 = result.parse().ok().filter(|r| *r <= res::SUCCESS)?;
        let ty: u8 = ty.parse().ok().filter(|t| *t <= ty::MOVE)?;
        let (target, target_ref) = Target::parse(target)?;
        Some(Self {
            started,
            acted,
            finished,
            result,
            time_since_start: finite(tsc)?,
            act_handled,
            has_animation,
            animation,
            anim_acted,
            anim_finished,
            pretend_act_time: finite(pretend)?,
            hand_wait: finite(hand)?,
            next_approach: next.parse().ok()?,
            finished_approaching,
            move_unit,
            did_run,
            asap,
            skip_animation,
            ty,
            target,
            target_ref,
            executor: (!exec.is_empty()).then(|| exec.to_owned()),
        })
    }

    /// Equal but for the animation handle's flags, which change between frames.
    #[must_use]
    pub fn same(&self, o: &Self) -> bool {
        let strip = |s: &Self| Self { anim_acted: None, anim_finished: None, ..s.clone() };
        let (a, b) = (strip(self), strip(o));
        a.time_since_start.to_bits() == b.time_since_start.to_bits()
            && a.pretend_act_time.to_bits() == b.pretend_act_time.to_bits()
            && a.hand_wait.to_bits() == b.hand_wait.to_bits()
            && a == b
    }

    /// `IsRunning`.
    #[must_use]
    pub fn running(&self) -> bool {
        self.started && !self.finished
    }
}

/// A command: what it is (its class and flags, from its description at first sight) and its state.
#[derive(Debug, Clone)]
pub struct Cmd {
    pub class: String,
    /// Its base classes below `UnitCommand` (the lifecycle's type tests).
    pub bases: Vec<String>,
    /// `AiAction != null`.
    pub ai: bool,
    /// `ReactionAction`.
    pub reaction: bool,
    /// `m_IgnoreCooldown`, `m_IgnoreCooldownBeforeTime` (ticks).
    pub ignore_cooldown: bool,
    pub ignore_before: Option<i64>,
    pub cutscene: bool,
    /// `DontInterruptInTurnBasedMode`.
    pub dont_interrupt_tb: bool,
    /// `CreatedByPlayer`.
    pub player: bool,
    /// `RiderCommand`, `MountCommand` (mounts aren't modelled).
    pub rider: bool,
    pub mount: bool,
    /// The lifecycle's virtual members its class overrides.
    pub overrides: Vec<String>,
    pub s: State,
    /// A `UnitAttack`'s own state, from its description's flags (`ua`; the `attacks` observer).
    pub ua: Option<attack::State>,
    /// Its approach (`ap`), a `UnitMoveTo`'s fields (`mv`) and an attack of opportunity's state
    /// (`aoo`): the `approach` observer.
    pub ap: Option<approach::Params>,
    pub mv: Option<move_to::Fields>,
    pub aoo: Option<aoo::State>,
}

impl Cmd {
    /// `uc_new`'s description; its type, target and executor must be its state row's.
    #[must_use]
    pub fn parse(r: &serde_json::Value) -> Option<Self> {
        use serde_json::Value as J;
        let b = |k: &str| r[k].as_bool();
        let ignore_before = match r.get("ign_before")? {
            J::Null => None,
            v => Some(v.as_i64()?),
        };
        let s = State::parse(r["s"].as_str()?)?;
        let ty: u8 = r["type"].as_u64().and_then(|t| u8::try_from(t).ok())?;
        let tgt = match r.get("tgt")? {
            J::Null => None,
            J::String(t) => Some(t.as_str()),
            _ => return None,
        };
        let pt = match r.get("pt")? {
            J::Null => None,
            J::Array(a) => {
                let v: Vec<f32> = a
                    .iter()
                    .map(|x| x.as_f64().map(|f| f as f32).filter(|f| f.is_finite()))
                    .collect::<Option<_>>()?;
                let [x, y, z] = v[..] else { return None };
                Some([x, y, z])
            }
            _ => return None,
        };
        let exec = match r.get("exec")? {
            J::Null => None,
            J::String(e) => Some(e.clone()),
            _ => return None,
        };
        let target_ok = match (&s.target, tgt, pt) {
            (Target::None, None, None) => true,
            (Target::Unit(u), Some(t), None) => u == t,
            (Target::Point(p), Some(""), Some(q)) | (Target::Point(p), None, Some(q)) => {
                p.iter().zip(&q).all(|(a, b)| a.to_bits() == b.to_bits())
            }
            _ => false,
        };
        if ty != s.ty || !target_ok || exec != s.executor {
            return None;
        }
        let ua = match r.get("ua") {
            None => None,
            Some(J::String(f)) => {
                Some(attack::State { flags: attack::Flags::parse(f)?, ..attack::State::default() })
            }
            Some(_) => return None,
        };
        let opt = |k: &str| match r.get(k) {
            None => Some(None),
            Some(J::String(f)) => Some(Some(f.as_str())),
            Some(_) => None,
        };
        let ap = match opt("ap")? {
            None => None,
            Some(f) => Some(approach::Params::parse(f)?),
        };
        let mv = match opt("mv")? {
            None => None,
            Some(f) => Some(move_to::Fields::parse(f)?),
        };
        let aoo = match opt("aoo")? {
            None => None,
            Some(f) => Some(aoo::State::parse(f)?),
        };
        Some(Self {
            class: r["class"].as_str()?.to_owned(),
            bases: r["bases"]
                .as_str()?
                .split(',')
                .filter(|x| !x.is_empty())
                .map(str::to_owned)
                .collect(),
            ai: b("ai")?,
            reaction: b("react")?,
            ignore_cooldown: b("ign")?,
            ignore_before,
            cutscene: b("cut")?,
            dont_interrupt_tb: b("dit")?,
            player: b("player")?,
            rider: b("rider")?,
            mount: b("mount")?,
            overrides: r["ovr"]
                .as_str()?
                .split(',')
                .filter(|x| !x.is_empty())
                .map(str::to_owned)
                .collect(),
            s,
            ua,
            ap,
            mv,
            aoo,
        })
    }

    fn overrides(&self, m: &str) -> bool {
        self.overrides.iter().any(|x| x == m)
    }

    /// `this is T`: the class or one of its bases.
    #[must_use]
    pub fn is(&self, t: &str) -> bool {
        self.class == t || self.bases.iter().any(|b| b == t)
    }
}

/// A `UnitCommands`: a unit's own, or a turn's prediction set.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Set {
    /// `m_Owner` (unit id).
    pub owner: Option<String>,
    /// `IsSimulating`.
    pub simulating: bool,
    /// `m_Commands` by [`ty`].
    pub slots: [Option<u32>; 4],
    pub queue: VecDeque<u32>,
    /// `PreviousCommand`.
    pub prev: Option<u32>,
    /// `GroupCommand != null`.
    pub group: bool,
}

impl Set {
    /// `owner|simulating|slots|queue|prev|group` (`CommandObs.SetRow`).
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        let p: Vec<&str> = s.split('|').collect();
        let [owner, sim, slots, queue, prev, group] = p[..] else { return None };
        let id = |x: &str| -> Option<Option<u32>> {
            if x == "-" { Some(None) } else { x.parse().ok().map(Some) }
        };
        let slot_ids: Vec<Option<u32>> = slots.split(',').map(id).collect::<Option<_>>()?;
        let slots: [Option<u32>; 4] = slot_ids.try_into().ok()?;
        let queue = if queue.is_empty() {
            VecDeque::new()
        } else {
            queue.split(',').map(|x| x.parse().ok()).collect::<Option<_>>()?
        };
        let [simulating] = flags::<1>(sim)?;
        let [group] = flags::<1>(group)?;
        Some(Self {
            owner: (!owner.is_empty()).then(|| owner.to_owned()),
            simulating,
            slots,
            queue,
            prev: id(prev)?,
            group,
        })
    }

    fn trivial(&self) -> bool {
        self.slots.iter().all(Option::is_none) && self.queue.is_empty() && self.prev.is_none()
    }
}

/// The commands model (a driven replay of a recording with WrathOracle's command records).
#[derive(Debug, Default)]
pub struct Commands {
    pub on: bool,
    /// `UnitAttack`'s class code is the sim's (the recording has the `attacks` observer).
    pub attacks: bool,
    /// `TickApproaching`, `UnitMoveTo`'s and `UnitAttackOfOpportunity`'s class code are the sim's
    /// (the `approach` observer).
    pub approach: bool,
    /// The class code running (`Init`, `OnStart`, ...), innermost last.
    pub class_call: Vec<&'static str>,
    /// The highest target wrapper id the recording has shown.
    pub max_wrapper: u32,
    pub cmds: HashMap<u32, Cmd>,
    pub sets: HashMap<u32, Set>,
    /// Each unit's `Commands` (set id), by unit id.
    pub unit_set: HashMap<String, u32>,
    /// The current turn's `m_RunningCommands` (a turn exists while [`current_turn`] is set; a new
    /// one starts empty).
    pub running: Vec<u32>,
}

impl Commands {
    #[must_use]
    pub fn get(&self, c: u32) -> Option<&Cmd> {
        self.cmds.get(&c)
    }

    /// A unit's current set.
    #[must_use]
    pub fn set_of(&self, unit: &str) -> Option<&Set> {
        self.sets.get(self.unit_set.get(unit)?)
    }

    /// `Commands.Empty` of a unit's current set.
    #[must_use]
    pub fn empty(&self, unit: &str) -> Option<bool> {
        self.set_of(unit).map(|s| s.slots.iter().all(Option::is_none))
    }

    /// `Commands.IsRunning()` of a unit's current set.
    #[must_use]
    pub fn is_running(&self, unit: &str) -> Option<bool> {
        let s = self.set_of(unit)?;
        Some(s.slots.iter().flatten().any(|c| self.cmds.get(c).is_some_and(|c| c.s.running())))
    }

    /// The commands of a unit's current set (`Raw`, then `Queue`).
    #[must_use]
    pub fn all_of(&self, unit: &str) -> Vec<u32> {
        let Some(s) = self.set_of(unit) else { return Vec::new() };
        s.slots.iter().flatten().chain(s.queue.iter()).copied().collect()
    }
}

/// Why the lifecycle can't go on with the recording: what it asked for and what the recording
/// had instead (reported; the entry's remaining records are skipped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Desync(pub String);

pub type R<T> = Result<T, Desync>;

fn desync<T>(what: impl Into<String>) -> R<T> {
    Err(Desync(what.into()))
}

/// What a leaf is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Of {
    Cmd(u32),
    Unit(Option<String>),
    /// Two units: `CommandTargetUntargetable(source, target)`, `unit.IsEnemy(other)`.
    Pair(Option<String>, Option<String>),
    /// A command's answer about a unit (`UnitAttack.IsUnitInFiveFeetRange(unit)`).
    CmdUnit(u32, Option<String>),
}

/// Class code's result: the base state it left (and `OnAction`'s result).
#[derive(Debug, Clone)]
pub struct Fx {
    pub s: State,
    pub ret: Option<u8>,
    /// A `UnitAttack`'s class state row (`ua`), raw.
    pub ua: Option<String>,
    /// Its target wrapper was first seen with it (a new `TargetWrapper`).
    pub fresh: bool,
}

/// The lifecycle's inputs, in the order the game evaluated them.
pub trait Inputs {
    /// A boolean leaf (`uc_v`) named `f`, about `of`.
    fn leaf(&mut self, w: &mut World, f: &'static str, of: Of) -> R<bool>;
    /// `TryMergeInto(into)`: the result and `into`'s target after it (and its wrapper).
    fn merge(&mut self, w: &mut World, c: u32, into: u32) -> R<(bool, Target, Option<u32>)>;
    /// `GetSaddledUnit(target)` in `Run`.
    fn saddled(&mut self, w: &mut World, target: Option<&str>) -> R<Option<String>>;
    /// `TargetWrapper.Equals` between two commands' targets.
    fn target_equals(&mut self, w: &mut World, a: &State, b: &State) -> R<bool>;
    /// `UnitCommands.TryUseCustomHandler(c)`.
    fn custom(&mut self, w: &mut World, c: u32) -> R<bool>;
    /// Class code `f` on `c` returned.
    fn fx(&mut self, w: &mut World, f: &'static str, c: u32) -> R<Fx>;
    /// `OnEnded` of `c` (base code): the recorded end must be it.
    fn ended(&mut self, w: &mut World, c: u32, result: u8, raise: bool) -> R<()>;
    /// `AddRunningCommand`/`RemoveRunningCommand(c)` was called (base code), the list `before`
    /// long: its record.
    fn running_call(&mut self, w: &mut World, c: u32, add: bool, before: usize) -> R<()>;
    /// `TickCommand(c, canMoveUnit)`: the recorded call (its argument).
    fn tick_command(&mut self, w: &mut World, c: u32) -> R<bool>;
    /// `SpendAction(type, full, timeSinceStart)` from `TickCommand`.
    fn spend(&mut self, w: &mut World, u: EntityIdx, ty: u8, full: bool, since: f32) -> R<()>;
    /// A nested `Run(c, fromQueue)` starts and ends (base code: the queue's head).
    fn nested_run(&mut self, w: &mut World, set: u32, c: u32, from_queue: bool) -> R<()>;
    fn nested_run_end(&mut self, w: &mut World, set: u32, c: u32) -> R<()>;
    /// An integer leaf of command `c` (`style`: the weapon's animation style).
    fn leaf_int(&mut self, w: &mut World, f: &'static str, c: u32) -> R<i64>;
    /// A list of units read by command `c` (`enemies`, `sorted`), in order (`None`: a null entry).
    fn leaf_ids(&mut self, w: &mut World, f: &'static str, c: u32) -> R<Vec<Option<String>>>;
    /// What `InitAttacks` of `c` left (`ua_atk`).
    fn attack_list(&mut self, w: &mut World, c: u32) -> R<attack::List>;
    /// `RuleCalculateAttacksCount(u)` (`limbs`: `...Limbs`) triggered by `c`'s `InitAttacks`: the
    /// sim's rule, fired.
    fn attacks_count(
        &mut self,
        w: &mut World,
        c: u32,
        u: EntityIdx,
        limbs: bool,
    ) -> R<attack::Counts>;
    /// `RuleAttackWithWeapon` triggered by `c`'s `TriggerAttackRule` (an attack of opportunity's
    /// `OnAction`): the sim's rule, fired; whether it hit.
    fn attack_rule(&mut self, w: &mut World, c: u32, spec: &attack::Spec) -> R<bool>;
    /// A path read by `c`'s `TickApproaching` (`fpath`: its `ForcedPath`; `vpath`: the path
    /// visualizer's): none, or its points.
    fn path(&mut self, w: &mut World, f: &'static str, c: u32) -> R<Option<Vec<[f32; 3]>>>;
    /// `GetThreatHand(u)` in attack of opportunity `c`'s `Init`, the sim's `hand`: the recorded
    /// hand's item reference id (`None`: no hand).
    fn threat(
        &mut self,
        w: &mut World,
        c: u32,
        u: EntityIdx,
        hand: Option<attack::Slot>,
    ) -> R<Option<String>>;
    /// A call into the movement agent (base code).
    fn call(&mut self, w: &mut World, call: &approach::Call) -> R<()>;
    /// `TickApproaching`'s `Random.Range(0.2f, 0.6f)` for `c`.
    fn approach_draw(&mut self, w: &mut World, c: u32) -> R<f32>;
    /// `NextCommandTime = 0` of `u`, written by `site` (`Engage`, `Disengage`,
    /// `UpdateCombatTarget`).
    fn nct_zero(&mut self, w: &mut World, u: &str, site: &'static str) -> R<()>;
    /// A write that changed a unit's `LastTarget` or `ManualTarget` ([`super::targets`]).
    fn tgt_write(&mut self, w: &mut World, x: &super::targets::Write) -> R<()>;
    /// A boolean leaf read right after the last record taken (code that records nothing in
    /// between): `UpdateCombatTarget`'s.
    fn leaf_next(&mut self, w: &mut World, f: &'static str, of: Of) -> R<bool> {
        self.leaf(w, f, of)
    }
}

fn cmd(w: &World, c: u32) -> R<&Cmd> {
    match w.combat.commands.cmds.get(&c) {
        Some(x) => Ok(x),
        None => desync(format!("command {c} is unknown")),
    }
}

fn cmd_mut(w: &mut World, c: u32) -> R<&mut Cmd> {
    match w.combat.commands.cmds.get_mut(&c) {
        Some(x) => Ok(x),
        None => desync(format!("command {c} is unknown")),
    }
}

fn set_mut(w: &mut World, s: u32) -> R<&mut Set> {
    match w.combat.commands.sets.get_mut(&s) {
        Some(x) => Ok(x),
        None => desync(format!("command set {s} is unknown")),
    }
}

fn set(w: &World, s: u32) -> R<&Set> {
    match w.combat.commands.sets.get(&s) {
        Some(x) => Ok(x),
        None => desync(format!("command set {s} is unknown")),
    }
}

fn unit(w: &World, id: Option<&str>) -> Option<EntityIdx> {
    id.and_then(|id| w.by_id.get(id).copied())
}

fn executor(w: &World, c: u32) -> R<EntityIdx> {
    let x = cmd(w, c)?;
    match unit(w, x.s.executor.as_deref()) {
        Some(u) => Ok(u),
        None => desync(format!("command {c}'s executor is unknown")),
    }
}

/// `CombatController.CurrentTurn`'s rider: the sim's current turn, or the one the sim's own
/// `RemoveUnit` cleared before the game's (the life controller runs whole in the sim; the game's
/// `LeaveCombat` interrupts the unit's commands first), until the recorded clear matches it.
#[must_use]
pub fn current_turn(w: &World) -> Option<EntityIdx> {
    let pending = w.combat.turns.log.iter().find_map(|e| match e {
        super::turns::TbEvent::Clear(u) => Some(*u),
        _ => None,
    });
    w.combat.current_unit.or(w.combat.cleared_by_remove).or(pending)
}

/// `IsIgnoreCooldown`.
fn ignores_cooldown(w: &World, c: &Cmd) -> bool {
    c.ignore_cooldown || c.ignore_before.is_some_and(|t| t >= w.clock.game_time.0)
}

/// `CombatController.IsInTurnBasedCombat()`.
fn turn_based(w: &World) -> bool {
    w.combat.turn_based
}

fn unsupported<T>(w: &mut World, what: &str) -> R<T> {
    let _ = w;
    desync(format!("{what} (not modelled)"))
}

/// The class names the lifecycle tests (`is UnitMoveTo`, `is UnitAttack`, ...).
pub mod class {
    pub const MOVE_TO: &str = "UnitMoveTo";
    pub const ATTACK: &str = "UnitAttack";
    pub const AOO: &str = "UnitAttackOfOpportunity";
    pub const MOVE_ALONG_PATH: &str = "UnitMoveAlongPath";
    pub const USE_ABILITY: &str = "UnitUseAbility";
    pub const AREA_TRANSITION: &str = "UnitAreaTransition";
    pub const GROUP_MOVE_TO_INTERACT: &str = "GroupMoveToInteractObject";
    pub const MOVE_CONTINUOUSLY: &str = "UnitMoveContiniously";
    pub const GROUP: &str = "UnitGroupCommand";
}

// ---- UnitCommand ----

/// `OnEnded(raiseEvent)` (base): finished; the view's weapon handling (`NeedEquipWeapons`); the
/// end event (its handlers are recorded as they act: the turn controller's `cd_cmd_end`); the
/// turn's `RemoveRunningCommand`.
fn on_ended<I: Inputs>(w: &mut World, inp: &mut I, c: u32, raise: bool) -> R<()> {
    if cmd(w, c)?.overrides("OnEnded") {
        return unsupported(w, "a command class that overrides OnEnded");
    }
    // An end of a finished command (`ForceFinish` doesn't check) is recorded only as a transition.
    let was = cmd(w, c)?.s.finished;
    cmd_mut(w, c)?.s.finished = true;
    // `(NeedEquipWeapons && DidRun && !SimulatingClick) & raiseEvent`: the getter is evaluated first.
    answer(w, inp, "nequip", c)?;
    // `EndInTbm`: `CurrentTurn?.RemoveRunningCommand(this)`.
    if current_turn(w).is_some() {
        let before = w.combat.commands.running.len();
        remove_running(w, c);
        inp.running_call(w, c, false, before)?;
    }
    if was {
        return Ok(());
    }
    let result = cmd(w, c)?.s.result;
    inp.ended(w, c, result, raise)
}

fn remove_running(w: &mut World, c: u32) {
    let l = &mut w.combat.commands.running;
    if let Some(p) = l.iter().position(|x| *x == c) {
        l.remove(p);
    }
}

/// `Interrupt(raiseEvent)`.
pub fn interrupt<I: Inputs>(w: &mut World, inp: &mut I, c: u32, raise: bool) -> R<()> {
    let s = &cmd(w, c)?.s;
    if s.result != res::INTERRUPT && !s.finished {
        cmd_mut(w, c)?.s.result = res::INTERRUPT;
        on_ended(w, inp, c, raise)?;
    }
    Ok(())
}

/// `ForceFinish(result)` (`ForceFinishForTurnBased`).
fn force_finish<I: Inputs>(w: &mut World, inp: &mut I, c: u32, result: u8) -> R<()> {
    let u = executor(w, c)?;
    if turn_based(w) && super::is_in_combat(w, u) && !cmd(w, c)?.s.acted {
        cmd_mut(w, c)?.s.acted = true;
    }
    let x = cmd_mut(w, c)?;
    x.s.result = result;
    x.s.acted = true;
    on_ended(w, inp, c, true)
}

/// `OnRun()` (base): `DidRun`, the view's weapon handling, `StartInTbm`.
fn on_run_base<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<()> {
    if w.combat.commands_paused {
        return unsupported(w, "UnitCommand.OnRun while paused");
    }
    cmd_mut(w, c)?.s.did_run = true;
    answer(w, inp, "nequip", c)?;
    // `StartInTbm`: the current turn's unit's commands (or a reaction) join its running list,
    // the executor's in combat.
    let u = executor(w, c)?;
    let turn = current_turn(w);
    if turn.is_some() && (turn == Some(u) || cmd(w, c)?.reaction) {
        let before = w.combat.commands.running.len();
        if super::is_in_combat(w, u) {
            w.combat.commands.running.push(c);
        }
        inp.running_call(w, c, true, before)?;
    }
    Ok(())
}

/// The command's class code returned: what it left of the base state. The fields the base
/// code alone writes (`IsStarted`, `IsFinished`, `Result`, `TimeSinceStart`, `HandAnimationWaitTime`)
/// must be the sim's (an end inside the class code came as its own record first); the rest is
/// taken.
fn apply_fx(w: &mut World, c: u32, f: &str, fx: &Fx) -> R<()> {
    let x = cmd_mut(w, c)?;
    let s = &fx.s;
    if (s.started, s.finished, s.result) == (x.s.started, x.s.finished, x.s.result)
        && s.time_since_start.to_bits() != x.s.time_since_start.to_bits()
    {
        // `UnitDoNothing.OnTick`'s `SetTimeSinceStart`.
        return desync(format!(
            "command {c}'s class code changed TimeSinceStart in {f} (SetTimeSinceStart: not modelled)"
        ));
    }
    if (s.started, s.finished, s.result) != (x.s.started, x.s.finished, x.s.result)
        || s.hand_wait.to_bits() != x.s.hand_wait.to_bits()
    {
        return desync(format!(
            "command {c}'s base state after {f} differs from the sim's (sim {:?}, recorded {:?})",
            x.s, s
        ));
    }
    // `Executor` is assigned only by `Init`.
    if f != "Init" && s.executor != x.s.executor {
        return desync(format!("command {c}'s executor changed in {f}"));
    }
    x.s = s.clone();
    Ok(())
}

/// Class code `f` of `c` (`owner`: `Init`'s executor). A `UnitAttack`'s is the sim's
/// ([`attack`]), and its recorded `uc_fx` a check; any other's is recorded. `OnAction`'s result.
fn class_code<I: Inputs>(
    w: &mut World,
    inp: &mut I,
    f: &'static str,
    c: u32,
    owner: Option<String>,
) -> R<Option<u8>> {
    let x = cmd(w, c)?;
    if move_to::ported(w, x) || aoo::ported(w, x) {
        return own_class_code(w, inp, f, c, owner);
    }
    let ported = attack::ported(w, cmd(w, c)?);
    if !ported || !matches!(f, "Init" | "OnStart" | "OnTick" | "OnAction") {
        let fx = inp.fx(w, f, c)?;
        apply_fx(w, c, f, &fx)?;
        // A `UnitAttack`'s recorded call (`TickApproaching`, base code) leaves its class state and
        // its handle (one seen finished stays so).
        if ported {
            let x = cmd(w, c)?;
            let seen = x.ua.as_ref().is_some_and(|s| s.finished_handle);
            if seen && x.s.animation && x.s.anim_finished != Some(true) {
                return desync(format!(
                    "command {c}'s animation, seen finished, unfinished after {f}"
                ));
            }
            let acted = x.ua.as_ref().is_some_and(|s| s.acted_handle);
            if acted && x.s.animation && x.s.anim_acted != Some(true) {
                return desync(format!("command {c}'s animation, seen acted, unacted after {f}"));
            }
            let ua = cmd(w, c)?.ua.as_ref().map(attack::Row::of);
            let row = fx.ua.as_deref().and_then(attack::Row::parse);
            if !matches!((&ua, &row), (Some(a), Some(b)) if a.same(b)) {
                return desync(format!(
                    "command {c}'s UnitAttack state after {f}: the sim's {ua:?}, the recording's {row:?}"
                ));
            }
            let x = cmd_mut(w, c)?;
            let finished = x.s.animation && x.s.anim_finished == Some(true);
            let acted = x.s.animation && x.s.anim_acted == Some(true);
            if let Some(s) = &mut x.ua {
                s.finished_handle |= finished;
                s.acted_handle |= acted;
            }
        }
        return Ok(fx.ret);
    }
    w.combat.commands.class_call.push(f);
    let ret = match f {
        "Init" => attack::init(w, inp, c, owner).map(|()| None),
        "OnStart" => attack::on_start(w, inp, c).map(|()| None),
        "OnTick" => attack::on_tick(w, inp, c).map(|()| None),
        _ => attack::on_action(w, inp, c).map(Some),
    };
    w.combat.commands.class_call.pop();
    let ret = ret?;
    let fx = inp.fx(w, f, c)?;
    check_fx(w, c, f, &fx, ret)?;
    Ok(ret)
}

/// The class code of `UnitMoveTo` or `UnitAttackOfOpportunity`, the sim's; its recorded `uc_fx` is
/// a check of the whole state.
fn own_class_code<I: Inputs>(
    w: &mut World,
    inp: &mut I,
    f: &'static str,
    c: u32,
    owner: Option<String>,
) -> R<Option<u8>> {
    let mt = cmd(w, c)?.is(class::MOVE_TO);
    w.combat.commands.class_call.push(f);
    let ret = match (mt, f) {
        (true, "Init") => move_to::init(w, c, owner).map(|()| None),
        (true, "OnRun") => move_to::on_run(w, inp, c).map(|()| None),
        (true, "OnStart") => move_to::on_start(w, c).map(|()| None),
        (true, "OnAction") => move_to::on_action(w, inp, c).map(Some),
        (false, "Init") => aoo::init(w, inp, c, owner).map(|()| None),
        (false, "OnRun") => aoo::on_run(w, inp, c).map(|()| None),
        (false, "OnStart") => Ok(None),
        (false, "OnAction") => aoo::on_action(w, inp, c).map(Some),
        // `OnTick` (the base one) does nothing.
        (_, "OnTick") => Ok(None),
        _ => desync(format!("command {c}'s class code {f} isn't one the sim runs")),
    };
    w.combat.commands.class_call.pop();
    let ret = ret?;
    let fx = inp.fx(w, f, c)?;
    let want = &cmd(w, c)?.s;
    if fx.ua.is_some() || !want.same(&fx.s) || fx.s.animation || ret != fx.ret {
        return desync(format!(
            "command {c}'s state after {f} differs from the sim's (sim {want:?} {ret:?}, recorded {:?} {:?})",
            fx.s, fx.ret
        ));
    }
    Ok(ret)
}

/// The base implementations of the lifecycle's getters (`f`: their leaf names) that the sim
/// computes (`None`: not computed).
fn base_answer(x: &Cmd, f: &str) -> Option<bool> {
    let s = &x.s;
    Some(match f {
        "sbi" | "await" | "dwfh" | "cmas" | "nequip" => false,
        "canstart" | "stt" => true,
        "oneframe" => s.ty != ty::STANDARD && s.ty != ty::MOVE,
        // `IsStarted && IsActed && Animation != null ? Animation.IsFinished : true`.
        "intr" => !(s.started && s.acted && s.animation) || s.anim_finished == Some(true),
        _ => return None,
    })
}

/// A class's answer to the lifecycle's getter (`f`: its leaf): recorded, and for a class whose code
/// the sim runs, its own (a difference is a desync).
fn answer<I: Inputs>(w: &mut World, inp: &mut I, f: &'static str, c: u32) -> R<bool> {
    let v = inp.leaf(w, f, Of::Cmd(c))?;
    let x = cmd(w, c)?;
    let own = if move_to::ported(w, x) {
        move_to::answer(w, c, f)?
    } else if aoo::ported(w, x) {
        aoo::answer(w, c, f)?
    } else {
        None
    };
    if own.is_some_and(|o| o != v) {
        return desync(format!("command {c}'s {f} is {v}, the sim's {own:?}"));
    }
    Ok(v)
}

/// `TickApproaching()`: the sim's with the `approach` observer, else recorded.
fn tick_approaching<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<()> {
    if w.combat.commands.approach {
        return approach::tick_approaching(w, inp, c);
    }
    class_code(w, inp, "TickApproaching", c, None).map(|_| ())
}

/// The recorded state after the sim's class code: every field the sim's, but the current
/// animation's progress (taken) and, after `UpdateTarget` assigned the target, its new wrapper's
/// id (taken); and the class state.
fn check_fx(w: &mut World, c: u32, f: &str, fx: &Fx, ret: Option<u8>) -> R<()> {
    let x = cmd(w, c)?;
    let Some(ua) = x.ua.clone() else {
        return desync(format!("command {c} has no UnitAttack state"));
    };
    let mut want = x.s.clone();
    // `Target = unit` makes a new `TargetWrapper`: one the recording hasn't shown before.
    if ua.new_wrapper {
        if !fx.fresh {
            return desync(format!("command {c}'s new target after {f} has a wrapper seen before"));
        }
        want.target_ref = fx.s.target_ref;
    }
    if !want.same(&fx.s) || fx.s.animation != fx.s.anim_acted.is_some() || ret != fx.ret {
        return desync(format!(
            "command {c}'s state after {f} differs from the sim's (sim {want:?} {ret:?}, recorded {:?} {:?})",
            fx.s, fx.ret
        ));
    }
    // A handle seen finished is finished still; one `OnTick` read and kept is as read.
    if ua.finished_handle && fx.s.animation && fx.s.anim_finished != Some(true) {
        return desync(format!("command {c}'s animation, seen finished, unfinished after {f}"));
    }
    if ua.acted_handle && fx.s.animation && fx.s.anim_acted != Some(true) {
        return desync(format!("command {c}'s animation, seen acted, unacted after {f}"));
    }
    if let Some(v) = ua.finished_read
        && fx.s.animation
        && fx.s.anim_finished != Some(v)
    {
        return desync(format!(
            "command {c}'s animation read {v} in {f}, the row after it differs"
        ));
    }
    let Some(row) = fx.ua.as_deref().and_then(attack::Row::parse) else {
        return desync(format!("command {c}'s {f} without its UnitAttack state (ua)"));
    };
    if !row.same(&attack::Row::of(&ua)) {
        return desync(format!(
            "command {c}'s UnitAttack state after {f}: the sim's {:?}, the recording's {row:?}",
            attack::Row::of(&ua)
        ));
    }
    let x = cmd_mut(w, c)?;
    x.s.anim_acted = fx.s.anim_acted;
    x.s.anim_finished = fx.s.anim_finished;
    x.s.target_ref = fx.s.target_ref;
    let finished = x.s.animation && x.s.anim_finished == Some(true);
    let acted = x.s.animation && x.s.anim_acted == Some(true);
    if let Some(s) = &mut x.ua {
        s.new_wrapper = false;
        s.finished_handle = finished;
        s.acted_handle = acted;
        s.finished_read = None;
    }
    Ok(())
}

/// `Start()`.
fn start<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<()> {
    if cmd(w, c)?.s.started {
        // "Command {0} is already started".
        return interrupt(w, inp, c, true);
    }
    if let Some(t) = target_in_world(w, c)?
        && !inp.leaf(w, "instate", Of::Unit(Some(t)))?
    {
        return interrupt(w, inp, c, true);
    }
    if !inp.leaf(w, "close", Of::Cmd(c))? {
        return interrupt(w, inp, c, true);
    }
    cmd_mut(w, c)?.s.started = true;
    class_code(w, inp, "OnStart", c, None)?;
    // `IUnitCommandStartHandler.HandleUnitCommandDidStart`: the impatience controller's.
    super::impatience::command_started(w, c);
    Ok(())
}

/// `Target?.Unit`: the target unit, when it resolves.
fn target_in_world(w: &World, c: u32) -> R<Option<String>> {
    Ok(cmd(w, c)?
        .s
        .target
        .unit()
        .filter(|id| w.by_id.get(*id).is_some_and(|e| w.unit(*e).is_some()))
        .map(str::to_owned))
}

/// `Tick()`.
fn tick<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<()> {
    if !cmd(w, c)?.s.running() {
        return interrupt(w, inp, c, true);
    }
    if let Some(t) = target_in_world(w, c)?
        && !inp.leaf(w, "instate", Of::Unit(Some(t)))?
    {
        return interrupt(w, inp, c, true);
    }
    let dt = w.clock.delta_time();
    cmd_mut(w, c)?.s.time_since_start += dt;
    class_code(w, inp, "OnTick", c, None)?;
    if cmd(w, c)?.s.finished {
        return Ok(());
    }
    if !cmd(w, c)?.s.act_handled {
        let s = cmd(w, c)?.s.clone();
        let mut acts =
            s.anim_acted == Some(true) || answer(w, inp, "oneframe", c)? || s.skip_animation;
        // "Animation {0} is done but not acted".
        if s.anim_finished == Some(true) && !acts {
            acts = true;
        }
        // `IsPretendAct`: no animation handle and a scheduled act.
        if !s.animation && s.pretend_act_time > 0.0 && s.time_since_start >= s.pretend_act_time {
            acts = true;
        }
        if !s.has_animation {
            acts = true;
        }
        if acts {
            cmd_mut(w, c)?.s.act_handled = true;
            // `AIData.UseAction` (the AI's bookkeeping) isn't modelled; `OnAction`'s result.
            let Some(ret) = class_code(w, inp, "OnAction", c, None)? else {
                return desync(format!("command {c}'s OnAction without its result"));
            };
            let x = cmd_mut(w, c)?;
            x.s.result = ret;
            x.s.acted = true;
        }
    }
    let s = &cmd(w, c)?.s;
    if s.result != res::NONE && (!s.animation || s.anim_finished == Some(true)) {
        return on_ended(w, inp, c, true);
    }
    if s.time_since_start > 9.0 && !cmd(w, c)?.is(class::MOVE_CONTINUOUSLY) {
        return unsupported(w, "a command running longer than a round (OnInterruptCommand)");
    }
    Ok(())
}

/// `ShouldUnitApproach` (base): `(!IsMoveUnit || !AwaitMovementFinish || !IsMoving) &&
/// IsUnitCloseEnough ? CannotStop : true`.
fn should_approach<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<bool> {
    let exec = cmd(w, c)?.s.executor.clone();
    let not_moving = !cmd(w, c)?.s.move_unit
        || !answer(w, inp, "await", c)?
        || !inp.leaf(w, "moving", Of::Unit(exec))?;
    if not_moving && inp.leaf(w, "close", Of::Cmd(c))? {
        return inp.leaf(w, "nostop", Of::Cmd(c));
    }
    Ok(true)
}

// ---- UnitCombatState ----

/// `UnitEntityData.HasCooldownForCommand(command)` (`UnitCombatState`).
fn has_cooldown_for_command(w: &mut World, c: u32) -> R<bool> {
    let x = cmd(w, c)?;
    let ignore = ignores_cooldown(w, x);
    if x.is(class::USE_ABILITY) {
        return unsupported(w, "HasCooldownForCommand: an ability (ShouldRestrictCommand)");
    }
    let t = x.s.ty;
    let u = executor(w, c)?;
    if ignore {
        return Ok(false);
    }
    Ok(super::cooldowns::has_cooldown_for_type(w, u, t, standard_move_running))
}

/// The unit's move command is running (`Unit.Commands.GetCommand(Move)?.IsRunning`).
fn standard_move_running(w: &World, u: EntityIdx) -> bool {
    let id = &w.entity(u).unique_id;
    w.combat
        .commands
        .set_of(id)
        .and_then(|s| s.slots[usize::from(ty::MOVE)])
        .and_then(|m| w.combat.commands.cmds.get(&m))
        .is_some_and(|m| m.s.running())
}

// ---- UnitCommandController ----

/// `TickCommandTurnBased(command)`.
fn tick_command_turn_based<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<bool> {
    use crate::unit_state::{cond, has_condition};
    if !turn_based(w) {
        return Ok(true);
    }
    if w.combat.turns.waiting_for_ui > 0 {
        return Ok(false);
    }
    let u = executor(w, c)?;
    let uid = w.entity(u).unique_id.clone();
    let x = cmd(w, c)?.clone();
    if x.is(class::AOO) {
        return Ok(true);
    }
    let turn = current_turn(w);
    if turn.is_none() {
        return Ok(!super::is_in_combat(w, u));
    }
    let mut go = {
        let running = w.combat.commands.running.contains(&c);
        let status = w.combat.turns.ctl.status;
        use super::turn_tick::Status;
        (turn == Some(u) || running) && matches!(status, Status::Acting | Status::Ending)
    };
    {
        if go && !x.s.started {
            if x.ignore_cooldown || x.ignore_before.is_some() {
                // `IsSpellCombatAttack()`: an attack ignoring its cooldown, with the magus' spell combat.
                if x.is(class::ATTACK) && ignores_cooldown(w, &x) {
                    return unsupported(w, "TickCommandTurnBased: the magus' spell combat attack");
                }
            }
            if x.is(class::USE_ABILITY) {
                return unsupported(w, "TickCommandTurnBased: an ability's IsAvailableForCast");
            }
        }
        // `IsDirectlyControllable && (Prone.Active || View.IsGetUp) && !ShouldBeProne`.
        let getting_up = inp.leaf(w, "ctrl", Of::Unit(Some(uid.clone())))?
            && (w.unit(u).is_some_and(|x| x.prone_active)
                || inp.leaf(w, "getup", Of::Unit(Some(uid.clone())))?)
            && !inp.leaf(w, "shprone", Of::Unit(Some(uid.clone())))?;
        let x = cmd(w, c)?.clone();
        if inp.leaf(w, "close", Of::Cmd(c))?
            && !inp.leaf(w, "nostop", Of::Cmd(c))?
            && !getting_up
            && !x.s.started
            && x.s.result == res::NONE
        {
            let able = inp.leaf(w, "canact", Of::Unit(Some(uid.clone())))?
                & !has_condition(w, u, cond::NAUSEATED);
            let allowed = ignores_cooldown(w, &x)
                || (inp.leaf(w, "caic", Of::Unit(Some(uid.clone())))?
                    && !has_cooldown_for_command(w, c)?);
            if !able || !allowed {
                force_finish(w, inp, c, res::SUCCESS)?;
                clear_queue_of(w, u);
                go = false;
            }
        } else {
            let next = cmd(w, c)?.s.next_approach;
            if should_approach(w, inp, c)?
                && next > 0
                && !inp.leaf(w, "moving", Of::Unit(Some(uid.clone())))?
            {
                force_finish(w, inp, c, res::SUCCESS)?;
                clear_queue_of(w, u);
                if cmd(w, c)?.rider {
                    return unsupported(w, "TickCommandTurnBased: a mount's rider command");
                }
                go = false;
            }
        }
    }
    Ok(go)
}

/// `command.Executor.Commands.Queue.Clear()`.
fn clear_queue_of(w: &mut World, u: EntityIdx) {
    let id = w.entity(u).unique_id.clone();
    if let Some(s) = w.combat.commands.unit_set.get(&id).copied()
        && let Some(s) = w.combat.commands.sets.get_mut(&s)
    {
        s.queue.clear();
    }
}

/// `TickCommand(command, canMoveUnit)`.
fn tick_command<I: Inputs>(w: &mut World, inp: &mut I, c: u32, can_move: bool) -> R<()> {
    if !tick_command_turn_based(w, inp, c)? {
        return Ok(());
    }
    let u = executor(w, c)?;
    let uid = w.entity(u).unique_id.clone();
    let x = cmd(w, c)?.clone();
    let target = target_in_world(w, c)?;
    if !x.cutscene {
        let mut stop = answer(w, inp, "sbi", c)?;
        if !stop && let Some(t) = &target {
            stop = !inp.leaf(w, "instate", Of::Unit(Some(t.clone())))?
                || inp.leaf(w, "untgt", Of::Pair(Some(uid.clone()), Some(t.clone())))?
                || (inp.leaf(w, "stealth", Of::Unit(Some(t.clone())))?
                    && inp.leaf(w, "enemy", Of::Pair(Some(uid.clone()), Some(t.clone())))?);
        }
        if stop {
            interrupt(w, inp, c, true)?;
        }
    }
    if !cmd(w, c)?.s.started {
        if cmd(w, c)?.s.asap {
            interrupt(w, inp, c, true)?;
        } else {
            if can_move {
                tick_approaching(w, inp, c)?;
            }
            if should_start_command(w, inp, c)? {
                start(w, inp, c)?;
            }
        }
    } else if can_move && answer(w, inp, "cmas", c)? {
        tick_approaching(w, inp, c)?;
    }
    if cmd(w, c)?.s.running() {
        // `TurnToTarget` (the view's orientation) isn't modelled.
        answer(w, inp, "stt", c)?;
        let acted = cmd(w, c)?.s.acted;
        tick(w, inp, c)?;
        let x = cmd(w, c)?.clone();
        if !acted && x.s.acted && !ignores_cooldown(w, &x) {
            let full = inp.leaf(w, "full", Of::Cmd(c))?;
            inp.spend(w, u, x.s.ty, full, x.s.time_since_start)?;
        }
        let x = cmd(w, c)?.clone();
        if (x.s.asap && answer(w, inp, "intr", c)?)
            || !inp.leaf(w, "canact", Of::Unit(Some(uid.clone())))?
            || x.is(class::USE_ABILITY)
        {
            if x.is(class::USE_ABILITY) {
                return unsupported(w, "TickCommand: an ability's CanCast");
            }
            interrupt(w, inp, c, true)?;
        }
    }
    Ok(())
}

/// `ShouldStartCommand(command)`.
fn should_start_command<I: Inputs>(w: &mut World, inp: &mut I, c: u32) -> R<bool> {
    use crate::unit_state::{cond, has_condition};
    let u = executor(w, c)?;
    let uid = w.entity(u).unique_id.clone();
    if inp.leaf(w, "hands", Of::Unit(Some(uid.clone())))? && !answer(w, inp, "dwfh", c)? {
        let dt = w.clock.delta_time();
        let x = cmd_mut(w, c)?;
        if x.s.hand_wait <= 5.0 {
            x.s.hand_wait += dt;
            return Ok(false);
        }
        // "HandAnimation waits too long": the view releases its interrupt animations.
        x.s.hand_wait = 0.0;
        return Ok(false);
    }
    if answer(w, inp, "await", c)? && inp.leaf(w, "rmoving", Of::Unit(Some(uid.clone())))? {
        return Ok(false);
    }
    if cmd(w, c)?.mount {
        return unsupported(w, "ShouldStartCommand: a mount command");
    }
    if !answer(w, inp, "canstart", c)? {
        return Ok(false);
    }
    let x = cmd(w, c)?.clone();
    let close = !x.s.started && x.s.result == res::NONE && inp.leaf(w, "close", Of::Cmd(c))?;
    if close
        && (x.is(class::AREA_TRANSITION)
            || x.is(class::GROUP_MOVE_TO_INTERACT)
            || x.is(class::MOVE_TO))
    {
        return Ok(true);
    }
    if !inp.leaf(w, "canact", Of::Unit(Some(uid.clone())))?
        || inp.leaf(w, "equip", Of::Unit(Some(uid.clone())))?
    {
        return Ok(false);
    }
    let allowed = !super::is_in_combat(w, u)
        || ignores_cooldown(w, &x)
        || (inp.leaf(w, "caic", Of::Unit(Some(uid)))? && !has_cooldown_for_command(w, c)?);
    if close & allowed {
        return Ok(!has_condition(w, u, cond::NAUSEATED));
    }
    Ok(false)
}

/// `UnitCommandController.TickOnUnit(unit)` past `BaseUnitController`'s checks, with the gate's
/// view and rigidbody (recorded).
pub fn tick_on_unit<I: Inputs>(
    w: &mut World,
    inp: &mut I,
    u: EntityIdx,
    view: bool,
    rigidbody: bool,
) -> R<()> {
    let uid = w.entity(u).unique_id.clone();
    let Some(s) = w.combat.commands.unit_set.get(&uid).copied() else {
        return desync(format!("unit {uid} has no command set"));
    };
    if let Some(p) = set(w, s)?.prev {
        interrupt(w, inp, p, true)?;
        set_mut(w, s)?.prev = None;
    }
    if !view || rigidbody || inp.leaf(w, "getup", Of::Unit(Some(uid.clone())))? {
        return Ok(());
    }
    let mut free = true;
    for slot in 0..4 {
        let Some(c) = set(w, s)?.slots[slot] else { continue };
        let arg = inp.tick_command(w, c)?;
        // `canMoveUnit = flag && ((!HoldState && !StopState && !HasRangedRider) || AiAction == null)`:
        // false once an earlier command moved the unit, true for a command that isn't the AI's; for
        // the AI's the hold and stop states (not modelled) decide it, and the argument is taken.
        let computed = if !free {
            Some(false)
        } else if !cmd(w, c)?.ai {
            Some(true)
        } else {
            None
        };
        if computed.is_some_and(|v| v != arg) {
            return desync(format!(
                "TickCommand({c})'s canMoveUnit differs from the sim's ({computed:?})"
            ));
        }
        tick_command(w, inp, c, arg)?;
        free = !cmd(w, c)?.s.move_unit;
    }
    remove_finished_and_update_queue(w, inp, s)?;
    // `if (unit.Commands.Empty) unit.View.StopMoving()` (recorded with the `approach` observer).
    if w.combat.commands.approach && w.combat.commands.empty(&uid) == Some(true) {
        inp.call(w, &approach::Call::Stop { unit: uid, c: None })?;
    }
    Ok(())
}

// ---- UnitCommands ----

/// `RemoveFinishedAndUpdateQueue()`.
pub fn remove_finished_and_update_queue<I: Inputs>(w: &mut World, inp: &mut I, s: u32) -> R<()> {
    for slot in 0..4 {
        if let Some(c) = set(w, s)?.slots[slot]
            && cmd(w, c)?.s.finished
        {
            set_mut(w, s)?.slots[slot] = None;
        }
    }
    let Some(head) = set(w, s)?.queue.front().copied() else { return Ok(()) };
    let t = cmd(w, head)?.s.ty;
    let st = set(w, s)?;
    let own = st.slots[usize::from(t)];
    let paired = match t {
        ty::MOVE => st.slots[usize::from(ty::STANDARD)],
        ty::STANDARD => st.slots[usize::from(ty::MOVE)],
        _ => None,
    };
    if own.is_none() && paired.is_none() {
        set_mut(w, s)?.queue.pop_front();
        inp.nested_run(w, s, head, true)?;
        run(w, inp, s, head, true, false)?;
        inp.nested_run_end(w, s, head)?;
    }
    Ok(())
}

/// `InterruptAndRemoveCommand(type, interruptPaired)`.
fn interrupt_and_remove<I: Inputs>(
    w: &mut World,
    inp: &mut I,
    s: u32,
    t: u8,
    paired: bool,
) -> R<()> {
    if let Some(c) = set(w, s)?.slots[usize::from(t)]
        && !cmd(w, c)?.s.finished
    {
        interrupt(w, inp, c, true)?;
    }
    set_mut(w, s)?.slots[usize::from(t)] = None;
    if paired {
        match t {
            ty::MOVE => interrupt_and_remove(w, inp, s, ty::STANDARD, false)?,
            ty::STANDARD => interrupt_and_remove(w, inp, s, ty::MOVE, false)?,
            _ => {}
        }
    }
    Ok(())
}

/// `InterruptAll(raiseEvent)`.
pub fn interrupt_all<I: Inputs>(w: &mut World, inp: &mut I, s: u32, raise: bool) -> R<()> {
    let mut kept = false;
    for slot in 0..4 {
        let Some(c) = set(w, s)?.slots[slot] else { continue };
        if cmd(w, c)?.s.finished {
            continue;
        }
        if answer(w, inp, "intr", c)? {
            let x = cmd(w, c)?;
            if (x.dont_interrupt_tb || x.cutscene) && turn_based(w) {
                let owner = unit(w, set(w, s)?.owner.as_deref());
                if owner.is_none_or(|o| super::is_in_combat(w, o)) {
                    kept = true;
                    continue;
                }
            }
            interrupt(w, inp, c, raise)?;
            set_mut(w, s)?.slots[slot] = None;
            continue;
        }
        kept = true;
    }
    if !kept {
        set_mut(w, s)?.queue.clear();
    }
    Ok(())
}

/// `InterruptAll(pred)`: the slots' and the queue's commands `pred` takes are interrupted and leave.
pub fn interrupt_all_where<I: Inputs>(
    w: &mut World,
    inp: &mut I,
    s: u32,
    pred: impl Fn(&Cmd) -> bool,
) -> R<()> {
    for slot in 0..4 {
        let Some(c) = set(w, s)?.slots[slot] else { continue };
        if pred(cmd(w, c)?) {
            interrupt(w, inp, c, true)?;
            set_mut(w, s)?.slots[slot] = None;
        }
    }
    let queue: Vec<u32> = set(w, s)?.queue.iter().copied().collect();
    for c in queue {
        if pred(cmd(w, c)?) {
            interrupt(w, inp, c, true)?;
            let q = &mut set_mut(w, s)?.queue;
            if let Some(p) = q.iter().position(|x| *x == c) {
                q.remove(p);
            }
        }
    }
    Ok(())
}

/// `InterruptAiCommands()`: the AI's commands are interrupted and leave their slots.
pub fn interrupt_ai_commands<I: Inputs>(w: &mut World, inp: &mut I, s: u32) -> R<()> {
    for slot in 0..4 {
        let Some(c) = set(w, s)?.slots[slot] else { continue };
        if cmd(w, c)?.ai {
            interrupt(w, inp, c, true)?;
            set_mut(w, s)?.slots[slot] = None;
        }
    }
    Ok(())
}

/// `InterruptMove()`: the commands moving the unit or about to.
pub fn interrupt_move<I: Inputs>(w: &mut World, inp: &mut I, s: u32) -> R<()> {
    for slot in 0..4 {
        let Some(c) = set(w, s)?.slots[slot] else { continue };
        if cmd(w, c)?.s.move_unit || should_approach(w, inp, c)? {
            interrupt(w, inp, c, true)?;
            set_mut(w, s)?.slots[slot] = None;
        }
    }
    Ok(())
}

/// `RemoveNoStartStadartCommand()`: an unstarted standard command (and its paired move).
pub fn remove_unstarted_standard<I: Inputs>(w: &mut World, inp: &mut I, s: u32) -> R<()> {
    if let Some(c) = set(w, s)?.slots[usize::from(ty::STANDARD)]
        && !cmd(w, c)?.s.started
    {
        interrupt_and_remove(w, inp, s, ty::STANDARD, true)?;
    }
    Ok(())
}

/// `InterruptGroupCommand()`: the set's group command and its units' group commands.
pub fn interrupt_group_command<I: Inputs>(w: &mut World, inp: &mut I, s: u32) -> R<()> {
    let st = set(w, s)?;
    if st.group {
        return unsupported(w, "UnitCommands.InterruptGroupCommand: a group command");
    }
    for slot in 0..4 {
        let Some(c) = set(w, s)?.slots[slot] else { continue };
        if cmd(w, c)?.is(class::GROUP) {
            interrupt(w, inp, c, true)?;
            set_mut(w, s)?.slots[slot] = None;
        }
    }
    Ok(())
}

/// `HasUniterrapitble()`.
fn has_uninterruptible<I: Inputs>(w: &mut World, inp: &mut I, s: u32) -> R<bool> {
    for slot in 0..4 {
        if let Some(c) = set(w, s)?.slots[slot]
            && !answer(w, inp, "intr", c)?
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `AddToQueueInternal(cmd, first)`.
fn add_to_queue_internal<I: Inputs>(
    w: &mut World,
    inp: &mut I,
    s: u32,
    c: u32,
    first: bool,
) -> R<()> {
    if cmd(w, c)?.s.executor.is_none() {
        let owner = set(w, s)?.owner.clone();
        class_code(w, inp, "Init", c, owner)?;
        if cmd(w, c)?.s.executor != set(w, s)?.owner {
            return desync(format!("command {c}'s Init didn't set its executor"));
        }
    }
    let q = &mut set_mut(w, s)?.queue;
    if first {
        q.push_front(c);
    } else {
        q.push_back(c);
    }
    Ok(())
}

/// `object.Equals(a.Target, b.Target)`: equal references or both null without a call; else
/// `TargetWrapper.Equals`, an input.
fn targets_equal<I: Inputs>(w: &mut World, inp: &mut I, a: u32, b: u32) -> R<bool> {
    let (sa, sb) = (cmd(w, a)?.s.clone(), cmd(w, b)?.s.clone());
    match (sa.target_ref, sb.target_ref) {
        (None, None) => Ok(true),
        (None, _) | (_, None) => Ok(false),
        (Some(x), Some(y)) if x == y => Ok(true),
        _ => inp.target_equals(w, &sa, &sb),
    }
}

/// `TryAddToQueueInsteadOfRunImmediately(next)`.
fn try_add_to_queue<I: Inputs>(w: &mut World, inp: &mut I, s: u32, next: u32) -> R<bool> {
    let t = cmd(w, next)?.s.ty;
    let st = set(w, s)?;
    let at = |t: u8| st.slots[usize::from(t)];
    let current = at(t).or(match t {
        ty::STANDARD => at(ty::STANDARD).or(at(ty::MOVE)),
        ty::MOVE => at(ty::MOVE).or(at(ty::STANDARD)),
        _ => None,
    });
    if has_uninterruptible(w, inp, s)? {
        if let Some(cur) = current {
            cmd_mut(w, cur)?.s.asap = true;
        }
        add_to_queue_internal(w, inp, s, next, false)?;
        return Ok(true);
    }
    if cmd(w, next)?.is(class::MOVE_TO) {
        return Ok(false);
    }
    if let Some(cur) = current
        && cmd(w, cur)?.is(class::MOVE_TO)
    {
        return Ok(false);
    }
    let mut queue = match current {
        Some(cur) if cmd(w, cur)?.s.running() => {
            targets_equal(w, inp, cur, next)? && inp.leaf(w, "close", Of::Cmd(next))?
        }
        _ => false,
    };
    if t == ty::MOVE && current.is_none_or(|cur| !cmd(w, cur).is_ok_and(|x| x.s.running())) {
        queue = match set(w, s)?.slots[usize::from(ty::STANDARD)] {
            Some(std) => {
                let x = cmd(w, std)?;
                !x.is(class::ATTACK) && x.s.running()
            }
            None => false,
        };
    }
    if queue {
        if let Some(cur) = current {
            cmd_mut(w, cur)?.s.asap = true;
        }
        add_to_queue_internal(w, inp, s, next, false)?;
    }
    Ok(queue)
}

/// `TryUseSpecialHandlers(cmd, fromQueue)`.
fn try_special_handlers<I: Inputs>(
    w: &mut World,
    inp: &mut I,
    s: u32,
    c: u32,
    from_queue: bool,
) -> R<bool> {
    let t = cmd(w, c)?.s.ty;
    if let Some(cur) = set(w, s)?.slots[usize::from(t)]
        && cmd(w, cur)?.s.running()
    {
        let (merged, target, target_ref) = inp.merge(w, c, cur)?;
        let x = &cmd(w, cur)?.s;
        if !merged && (x.target.clone(), x.target_ref) != (target.clone(), target_ref) {
            return desync(format!("a TryMergeInto into {cur} that failed changed its target"));
        }
        if merged {
            let (asap, ai) = (cmd(w, c)?.s.asap, cmd(w, c)?.ai);
            let x = cmd_mut(w, cur)?;
            x.s.asap = asap;
            x.ai = ai;
            x.s.target = target;
            x.s.target_ref = target_ref;
            update_combat_target(w, inp, cur, true)?;
            return Ok(true);
        }
    }
    if !from_queue && try_add_to_queue(w, inp, s, c)? {
        return Ok(true);
    }
    if inp.custom(w, c)? {
        return unsupported(
            w,
            "UnitCommands.TryUseCustomHandler: a custom handler (the kineticist)",
        );
    }
    Ok(false)
}

/// `UnitCommands.Run(cmd, fromQueue, doNotClearQueue)` (the private overload).
pub fn run<I: Inputs>(
    w: &mut World,
    inp: &mut I,
    s: u32,
    c: u32,
    from_queue: bool,
    no_clear: bool,
) -> R<()> {
    use crate::unit_state::{cond, has_condition};
    let owner_id = set(w, s)?.owner.clone();
    let Some(owner) = unit(w, owner_id.as_deref()) else {
        return desync(format!("command set {s}'s owner is unknown"));
    };
    // `CanRunCommand`.
    let t = cmd(w, c)?.s.ty;
    if (t == ty::STANDARD && has_condition(w, owner, cond::CANT_USE_STANDARD_ACTIONS))
        || !super::units::is_conscious(w, owner)
    {
        return Ok(());
    }
    if !from_queue {
        // `FixTargetIfTargetOnMount`.
        // `cmd.TargetUnit`: null when the reference doesn't resolve.
        let target = target_in_world(w, c)?;
        if inp.saddled(w, target.as_deref())?.is_some() {
            return unsupported(w, "UnitCommands.Run: a mounted target");
        }
        match cmd(w, c)?.s.executor.clone() {
            Some(e) if Some(&e) != owner_id.as_ref() => {
                // "Invalid command, can't run".
                return Ok(());
            }
            Some(_) => {}
            None => {
                class_code(w, inp, "Init", c, owner_id.clone())?;
                if cmd(w, c)?.s.executor != owner_id {
                    return desync(format!("command {c}'s Init didn't set its executor"));
                }
            }
        }
        if !no_clear {
            set_mut(w, s)?.queue.clear();
        }
    }
    if let Some(p) = set(w, s)?.prev
        && cmd(w, p)?.s.running()
    {
        let (merged, target, target_ref) = inp.merge(w, c, p)?;
        // (`TryMergeInto` is the base one, false, for the classes the sim runs.)
        let x = cmd(w, c)?;
        if merged && (move_to::ported(w, x) || aoo::ported(w, x)) {
            return desync(format!("command {c}'s TryMergeInto is true, the sim's false"));
        }
        let x = &cmd(w, p)?.s;
        if !merged && (x.target.clone(), x.target_ref) != (target, target_ref) {
            return desync(format!("a TryMergeInto into {p} that failed changed its target"));
        }
        if merged {
            return unsupported(w, "UnitCommands.Run: merging into a parked command (paused)");
        }
    }
    if try_special_handlers(w, inp, s, c, from_queue)? {
        return Ok(());
    }
    interrupt_group_command(w, inp, s)?;
    let t = cmd(w, c)?.s.ty;
    let st = set(w, s)?;
    let current = st.slots[usize::from(t)].or(match t {
        ty::MOVE => st.slots[usize::from(ty::STANDARD)],
        ty::STANDARD => st.slots[usize::from(ty::MOVE)],
        _ => None,
    });
    if w.combat.commands_paused && st.prev.is_none() && current.is_some() {
        return unsupported(w, "UnitCommands.Run while paused (a parked command)");
    }
    interrupt_and_remove(w, inp, s, t, true)?;
    set_mut(w, s)?.slots[usize::from(t)] = Some(c);
    // `CancelStop()` (the stop state) isn't modelled.
    for slot in 0..4 {
        let Some(other) = set(w, s)?.slots[slot] else { continue };
        if other == c || cmd(w, other)?.s.started {
            continue;
        }
        if inp.leaf(w, "close", Of::Cmd(c))? {
            continue;
        }
        let ot = cmd(w, other)?.s.ty;
        if ot == ty::STANDARD || ot == ty::MOVE || !inp.leaf(w, "close", Of::Cmd(other))? {
            interrupt(w, inp, other, true)?;
            set_mut(w, s)?.slots[slot] = None;
        }
    }
    if t == ty::STANDARD || set(w, s)?.slots[usize::from(ty::STANDARD)].is_none() {
        update_combat_target(w, inp, c, false)?;
    }
    if cmd(w, c)?.overrides("OnRun") {
        class_code(w, inp, "OnRun", c, None)?;
    } else {
        on_run_base(w, inp, c)?;
    }
    Ok(())
}

/// `UpdateCombatTarget(cmd)`: the executor's `LastTarget` and `ManualTarget`, when the targets are
/// modelled ([`super::targets`]). Only `UnitAttack` marks its target as the manual one. `merge`:
/// from `TryUseSpecialHandlers`.
fn update_combat_target<I: Inputs>(w: &mut World, inp: &mut I, c: u32, merge: bool) -> R<()> {
    use super::targets::{self, Field};
    if !w.combat.targets.on {
        return Ok(());
    }
    let site = if merge {
        super::targets::Site::UpdateCombatTargetMerge
    } else {
        super::targets::Site::UpdateCombatTarget
    };
    let set = |w: &mut World, inp: &mut I, e, f, v| -> R<()> {
        match targets::set(w, e, f, v, site)? {
            Some(x) => inp.tgt_write(w, &x),
            None => Ok(()),
        }
    };
    // `cmd is UnitUseAbility && (IsAutoUseAbility(...) || !ShouldUpdateManualTarget)`.
    if cmd(w, c)?.is(class::USE_ABILITY) {
        return unsupported(w, "UnitCommands.UpdateCombatTarget for a UnitUseAbility");
    }
    if inp.leaf_next(w, "tact", Of::Cmd(c))? {
        return unsupported(w, "UnitCommands.UpdateCombatTarget in tactical combat");
    }
    let exec_id = cmd(w, c)?.s.executor.clone();
    let Some(e) = unit(w, exec_id.as_deref()).filter(|e| w.unit(*e).is_some()) else {
        return desync(format!("command {c}'s executor is unknown"));
    };
    let exec_id = exec_id.unwrap_or_default();
    let target = target_in_world(w, c)?.and_then(|t| unit(w, Some(&t)));
    let hit = match target {
        Some(t) if t != e => {
            (super::units::is_conscious(w, t)
                || !inp.leaf_next(w, "aien", Of::Unit(Some(exec_id.clone())))?)
                && super::groups::can_attack(w, e, t)
        }
        _ => false,
    };
    let x = cmd(w, c)?;
    let (ai, mark) = (x.ai, x.is(class::ATTACK));
    if let Some(t) = target.filter(|_| hit) {
        set(w, inp, e, Field::Last, Some(t))?;
        if !ai && mark {
            set(w, inp, e, Field::Manual, Some(t))?;
        } else if targets::value(w, e, Field::Manual)? != Some(t) {
            set(w, inp, e, Field::Manual, None)?;
        }
    } else {
        set(w, inp, e, Field::Manual, None)?;
        if !ai {
            inp.nct_zero(w, &exec_id, "UpdateCombatTarget")?;
            w.combat.ai.nct.insert(e, 0.0);
        }
    }
    Ok(())
}

/// `AddToQueueOrRun(cmd, first)`.
pub fn add_to_queue_or_run<I: Inputs>(
    w: &mut World,
    inp: &mut I,
    s: u32,
    c: u32,
    first: bool,
) -> R<()> {
    let t = cmd(w, c)?.s.ty;
    if set(w, s)?.slots[usize::from(t)].is_none() {
        inp.nested_run(w, s, c, false)?;
        run(w, inp, s, c, false, false)?;
        return inp.nested_run_end(w, s, c);
    }
    add_to_queue_internal(w, inp, s, c, first)
}

/// Whether the unit's current set has a command, a queue or a parked command (the controller's
/// tick does something for it).
#[must_use]
pub fn busy(w: &World, u: EntityIdx) -> bool {
    let id = &w.entity(u).unique_id;
    w.combat.commands.set_of(id).is_some_and(|s| !s.trivial())
}

#[cfg(test)]
mod tests;
