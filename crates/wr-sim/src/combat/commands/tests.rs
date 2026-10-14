//! The commands' base lifecycle (`super`) on scripted inputs.

use std::collections::VecDeque;

use super::*;
use crate::combat::turns::tests::{unit, world};

/// One recorded input, in order.
#[derive(Debug, Clone)]
enum Ev {
    Leaf(&'static str, Of, bool),
    Fx(&'static str, u32, Fx),
    Ended(u32, u8, bool),
    Running(u32, bool),
    TickCommand(u32, bool),
    Spend(u8, bool),
    Custom(u32, bool),
    Saddled(Option<String>),
    /// `GetSaddledUnit` of exactly this target.
    SaddledOf(Option<String>),
    /// A merge's result; a failed one leaves `into`'s target unless one is given.
    Merge(u32, u32, bool),
    MergeTarget(u32, u32, bool, Target, Option<u32>),
    TargetEquals(bool),
    NestedRun(u32, u32, bool),
    NestedRunEnd(u32, u32),
    LeafInt(&'static str, u32, i64),
    LeafIds(&'static str, u32, Vec<Option<String>>),
    AttackList(u32, attack::List),
    /// The counts rule (`true`: the limbs') `c` triggers, and its answer.
    Counts(u32, bool, attack::Counts),
    /// The attack rule `c` triggers (its number, count, penalty, first, full), and whether it hits.
    AttackRule(u32, (i32, i32, i32, bool, bool), bool),
    /// A path leaf of `c`.
    Path(&'static str, u32, Option<Vec<[f32; 3]>>),
    /// The threat hand of `c` the sim computes, and the recorded item.
    Threat(u32, Option<attack::Slot>, Option<String>),
    /// A call into the movement agent.
    Call(approach::Call),
    /// `TickApproaching`'s draw for `c`.
    Draw(u32, f32),
}

/// Inputs from a script: each call must be the next event, else a desync.
#[derive(Default)]
struct Script(VecDeque<Ev>);

impl Script {
    fn next(&mut self, what: &str) -> R<Ev> {
        self.0.pop_front().ok_or_else(|| Desync(format!("{what}: the script ends")))
    }
}

impl Inputs for Script {
    fn leaf(&mut self, _: &mut World, f: &'static str, of: Of) -> R<bool> {
        match self.next(f)? {
            Ev::Leaf(g, o, v) if g == f && o == of => Ok(v),
            e => Err(Desync(format!("leaf {f} {of:?}: the script has {e:?}"))),
        }
    }
    fn merge(&mut self, w: &mut World, c: u32, into: u32) -> R<(bool, Target, Option<u32>)> {
        match self.next("merge")? {
            Ev::Merge(a, b, true) if (a, b) == (c, into) => {
                Ok((true, Target::Unit("b".into()), Some(9)))
            }
            Ev::Merge(a, b, false) if (a, b) == (c, into) => {
                let s = &w.combat.commands.cmds[&into].s;
                Ok((false, s.target.clone(), s.target_ref))
            }
            Ev::MergeTarget(a, b, v, t, r) if (a, b) == (c, into) => Ok((v, t, r)),
            e => Err(Desync(format!("merge {c} {into}: the script has {e:?}"))),
        }
    }
    fn saddled(&mut self, _: &mut World, t: Option<&str>) -> R<Option<String>> {
        match self.next("saddled")? {
            Ev::Saddled(v) => Ok(v),
            Ev::SaddledOf(of) if of.as_deref() == t => Ok(None),
            e => Err(Desync(format!("saddled {t:?}: the script has {e:?}"))),
        }
    }
    fn target_equals(&mut self, _: &mut World, _: &State, _: &State) -> R<bool> {
        match self.next("teq")? {
            Ev::TargetEquals(v) => Ok(v),
            e => Err(Desync(format!("teq: the script has {e:?}"))),
        }
    }
    fn custom(&mut self, _: &mut World, c: u32) -> R<bool> {
        match self.next("custom")? {
            Ev::Custom(d, v) if d == c => Ok(v),
            e => Err(Desync(format!("custom {c}: the script has {e:?}"))),
        }
    }
    fn fx(&mut self, _: &mut World, f: &'static str, c: u32) -> R<Fx> {
        match self.next(f)? {
            Ev::Fx(g, d, fx) if g == f && d == c => Ok(fx),
            e => Err(Desync(format!("fx {f} {c}: the script has {e:?}"))),
        }
    }
    fn ended(&mut self, _: &mut World, c: u32, result: u8, raise: bool) -> R<()> {
        match self.next("ended")? {
            Ev::Ended(d, r, x) if (d, r, x) == (c, result, raise) => Ok(()),
            e => Err(Desync(format!("ended {c} {result}: the script has {e:?}"))),
        }
    }
    fn running_call(&mut self, _: &mut World, c: u32, add: bool, _: usize) -> R<()> {
        match self.next("running")? {
            Ev::Running(d, a) if (d, a) == (c, add) => Ok(()),
            e => Err(Desync(format!("running {c} {add}: the script has {e:?}"))),
        }
    }
    fn tick_command(&mut self, _: &mut World, c: u32) -> R<bool> {
        match self.next("tick_command")? {
            Ev::TickCommand(d, v) if d == c => Ok(v),
            e => Err(Desync(format!("tick_command {c}: the script has {e:?}"))),
        }
    }
    fn spend(&mut self, _: &mut World, _: EntityIdx, t: u8, full: bool, _: f32) -> R<()> {
        match self.next("spend")? {
            Ev::Spend(d, f) if (d, f) == (t, full) => Ok(()),
            e => Err(Desync(format!("spend {t} {full}: the script has {e:?}"))),
        }
    }
    fn nested_run(&mut self, _: &mut World, s: u32, c: u32, fq: bool) -> R<()> {
        match self.next("nested run")? {
            Ev::NestedRun(a, b, f) if (a, b, f) == (s, c, fq) => Ok(()),
            e => Err(Desync(format!("nested run {s} {c}: the script has {e:?}"))),
        }
    }
    fn nested_run_end(&mut self, _: &mut World, s: u32, c: u32) -> R<()> {
        match self.next("nested run end")? {
            Ev::NestedRunEnd(a, b) if (a, b) == (s, c) => Ok(()),
            e => Err(Desync(format!("nested run end {s} {c}: the script has {e:?}"))),
        }
    }
    fn leaf_int(&mut self, _: &mut World, f: &'static str, c: u32) -> R<i64> {
        match self.next(f)? {
            Ev::LeafInt(g, d, v) if (g, d) == (f, c) => Ok(v),
            e => Err(Desync(format!("leaf {f} {c}: the script has {e:?}"))),
        }
    }
    fn leaf_ids(&mut self, _: &mut World, f: &'static str, c: u32) -> R<Vec<Option<String>>> {
        match self.next(f)? {
            Ev::LeafIds(g, d, v) if (g, d) == (f, c) => Ok(v),
            e => Err(Desync(format!("leaf {f} {c}: the script has {e:?}"))),
        }
    }
    fn attack_list(&mut self, _: &mut World, c: u32) -> R<attack::List> {
        match self.next("ua_atk")? {
            Ev::AttackList(d, l) if d == c => Ok(l),
            e => Err(Desync(format!("ua_atk {c}: the script has {e:?}"))),
        }
    }
    fn attacks_count(
        &mut self,
        _: &mut World,
        c: u32,
        _: EntityIdx,
        limbs: bool,
    ) -> R<attack::Counts> {
        match self.next("counts")? {
            Ev::Counts(d, l, n) if (d, l) == (c, limbs) => Ok(n),
            e => Err(Desync(format!("counts {c} {limbs}: the script has {e:?}"))),
        }
    }
    fn attack_rule(&mut self, _: &mut World, c: u32, x: &attack::Spec) -> R<bool> {
        let got =
            (x.attack_number, x.attacks_count, x.penalty, x.is_first_attack, x.is_full_attack);
        match self.next("attack")? {
            Ev::AttackRule(d, want, hit) if d == c && want == got => Ok(hit),
            e => Err(Desync(format!("attack {c} {got:?}: the script has {e:?}"))),
        }
    }
    fn path(&mut self, _: &mut World, f: &'static str, c: u32) -> R<Option<Vec<[f32; 3]>>> {
        match self.next(f)? {
            Ev::Path(g, d, p) if (g, d) == (f, c) => Ok(p),
            e => Err(Desync(format!("path {f} {c}: the script has {e:?}"))),
        }
    }
    fn threat(
        &mut self,
        _: &mut World,
        c: u32,
        _: EntityIdx,
        hand: Option<attack::Slot>,
    ) -> R<Option<String>> {
        match self.next("threat")? {
            Ev::Threat(d, h, item) if (d, h) == (c, hand) => Ok(item),
            e => Err(Desync(format!("threat {c} {hand:?}: the script has {e:?}"))),
        }
    }
    fn call(&mut self, _: &mut World, call: &approach::Call) -> R<()> {
        match self.next("call")? {
            Ev::Call(x) if &x == call => Ok(()),
            e => Err(Desync(format!("call {call:?}: the script has {e:?}"))),
        }
    }
    fn approach_draw(&mut self, _: &mut World, c: u32) -> R<f32> {
        match self.next("draw")? {
            Ev::Draw(d, v) if d == c => Ok(v),
            e => Err(Desync(format!("draw {c}: the script has {e:?}"))),
        }
    }
}

fn state(ty: u8, target: Target, executor: Option<&str>) -> State {
    State {
        started: false,
        acted: false,
        finished: false,
        result: res::NONE,
        time_since_start: 0.0,
        act_handled: false,
        has_animation: false,
        animation: false,
        anim_acted: None,
        anim_finished: None,
        pretend_act_time: 0.0,
        hand_wait: 0.0,
        next_approach: 0,
        finished_approaching: false,
        move_unit: false,
        did_run: false,
        asap: false,
        skip_animation: false,
        ty,
        target_ref: (target != Target::None).then_some(1),
        target,
        executor: executor.map(str::to_owned),
    }
}

fn command(class: &str, s: State, overrides: &[&str]) -> Cmd {
    Cmd {
        class: class.into(),
        bases: Vec::new(),
        ai: false,
        reaction: false,
        ignore_cooldown: class == class::MOVE_TO,
        ignore_before: None,
        cutscene: false,
        dont_interrupt_tb: false,
        player: false,
        rider: false,
        mount: false,
        overrides: overrides.iter().map(|x| (*x).to_owned()).collect(),
        s,
        ua: None,
        ap: None,
        mv: None,
        aoo: None,
    }
}

/// A world with one unit `a` holding set 1 (empty), the model on.
fn setup() -> (World, EntityIdx) {
    let mut w = world();
    let a = unit(&mut w, "a", None, 10);
    let m = &mut w.combat.commands;
    m.on = true;
    m.sets.insert(1, Set { owner: Some("a".into()), ..Set::default() });
    m.unit_set.insert("a".into(), 1);
    (w, a)
}

#[test]
fn rows_parse() {
    let s = State::parse("110|3|0.25|11101|0|0|0|010|10|1|523B#3|5671").expect("row");
    assert!(s.started && s.acted && !s.finished);
    assert_eq!((s.result, s.ty), (res::SUCCESS, ty::STANDARD));
    assert_eq!((s.anim_acted, s.anim_finished), (Some(0 == 1), Some(true)));
    assert_eq!((s.target.clone(), s.target_ref), (Target::Unit("523B".into()), Some(3)));
    assert_eq!(s.executor.as_deref(), Some("5671"));
    assert!(s.move_unit && s.asap && !s.skip_animation);
    let p = State::parse("000|0|0|000--|0|0|0|000|00|3|@1.5,2,-3#9|").expect("point");
    assert_eq!(p.target, Target::Point([1.5, 2.0, -3.0]));
    // A target without its wrapper, or with a non-finite coordinate.
    assert!(State::parse("000|0|0|000--|0|0|0|000|00|3|523B|").is_none());
    for bad in ["NaN", "inf", "-inf"] {
        assert!(State::parse(&format!("000|0|0|000--|0|0|0|000|00|3|@{bad},0,0#1|")).is_none());
    }
    assert_eq!(p.executor, None);
    // Malformed: animation flags without an animation, a bad result, a short row.
    assert!(State::parse("000|0|0|00010|0|0|0|000|00|3||").is_none());
    assert!(State::parse("000|7|0|000--|0|0|0|000|00|3||").is_none());
    assert!(State::parse("000|0|NaN|000--|0|0|0|000|00|3||").is_none());
    assert!(State::parse("000|0|0|000--|0|0|0|000|00|3|").is_none());
    let set = Set::parse("5671|0|-,4,-,-|7,8|-|0").expect("set");
    assert_eq!(set.slots, [None, Some(4), None, None]);
    assert_eq!(set.queue, VecDeque::from([7, 8]));
    assert!(Set::parse("5671|0|-,4,-|7|-|0").is_none());
    // Flags are one character each.
    assert!(Set::parse("5671|00|-,4,-,-|7|-|0").is_none());
    assert!(Set::parse("5671|0|-,4,-,-|7|-|00").is_none());
}

/// A follow move outside combat: `Run` puts it in its slot (its overridden `OnRun` is class code),
/// the controller's tick approaches, and it doesn't start while the agent moves.
#[test]
fn a_move_runs_and_waits_for_the_movement() {
    let (mut w, a) = setup();
    let mv =
        command(class::MOVE_TO, state(ty::MOVE, Target::Point([1.0, 0.0, 2.0]), None), &["OnRun"]);
    w.combat.commands.cmds.insert(7, mv);
    let ran = |w: &World| {
        let mut s = w.combat.commands.cmds[&7].s.clone();
        s.executor = Some("a".into());
        s
    };
    let after_init = ran(&w);
    let mut after_run = after_init.clone();
    after_run.did_run = true;
    let mut script = Script(VecDeque::from([
        Ev::Saddled(None),
        Ev::Fx("Init", 7, Fx { s: after_init, ret: None, ua: None, fresh: false }),
        Ev::Custom(7, false),
        Ev::Fx("OnRun", 7, Fx { s: after_run.clone(), ret: None, ua: None, fresh: false }),
    ]));
    run(&mut w, &mut script, 1, 7, false, false).expect("run");
    assert!(script.0.is_empty());
    assert_eq!(w.combat.commands.sets[&1].slots[usize::from(ty::MOVE)], Some(7));
    assert!(busy(&w, a));

    let mut approached = after_run.clone();
    approached.move_unit = true;
    let mut script = Script(VecDeque::from([
        Ev::Leaf("getup", Of::Unit(Some("a".into())), false),
        Ev::TickCommand(7, true),
        Ev::Leaf("sbi", Of::Cmd(7), false),
        Ev::Fx("TickApproaching", 7, Fx { s: approached, ret: None, ua: None, fresh: false }),
        Ev::Leaf("hands", Of::Unit(Some("a".into())), false),
        Ev::Leaf("await", Of::Cmd(7), true),
        Ev::Leaf("rmoving", Of::Unit(Some("a".into())), true),
    ]));
    tick_on_unit(&mut w, &mut script, a, true, false).expect("tick");
    assert!(script.0.is_empty());
    let s = &w.combat.commands.cmds[&7].s;
    assert!(s.move_unit && !s.started);
    assert_eq!(w.combat.commands.empty("a"), Some(false));
}

/// A recorded input other than the one the lifecycle reads is a desync.
#[test]
fn another_input_is_a_desync() {
    let (mut w, a) = setup();
    let mv = command(class::MOVE_TO, state(ty::MOVE, Target::Point([0.0; 3]), Some("a")), &[]);
    w.combat.commands.cmds.insert(7, mv);
    w.combat.commands.sets.get_mut(&1).expect("set").slots[usize::from(ty::MOVE)] = Some(7);
    let mut script = Script(VecDeque::from([
        Ev::Leaf("getup", Of::Unit(Some("a".into())), false),
        Ev::TickCommand(7, true),
        // The lifecycle reads ShouldBeInterrupted next.
        Ev::Leaf("close", Of::Cmd(7), true),
    ]));
    let e = tick_on_unit(&mut w, &mut script, a, true, false).expect_err("desync");
    assert!(e.0.contains("leaf sbi"), "{e:?}");
}

/// A finished command leaves its slot; `InterruptAll` keeps an uninterruptible one (and the queue).
#[test]
fn interrupt_all_keeps_an_uninterruptible_command() {
    let (mut w, _) = setup();
    let mut s = state(ty::STANDARD, Target::Unit("b".into()), Some("a"));
    s.started = true;
    w.combat.commands.cmds.insert(4, command(class::ATTACK, s.clone(), &[]));
    let mv = state(ty::MOVE, Target::Point([0.0; 3]), Some("a"));
    w.combat.commands.cmds.insert(5, command(class::MOVE_TO, mv, &[]));
    w.combat
        .commands
        .cmds
        .insert(6, command(class::MOVE_TO, state(ty::MOVE, Target::None, Some("a")), &[]));
    {
        let set = w.combat.commands.sets.get_mut(&1).expect("set");
        set.slots[usize::from(ty::STANDARD)] = Some(4);
        set.slots[usize::from(ty::MOVE)] = Some(5);
        set.queue.push_back(6);
    }
    let mut script = Script(VecDeque::from([
        Ev::Leaf("intr", Of::Cmd(4), false),
        Ev::Leaf("intr", Of::Cmd(5), true),
        Ev::Leaf("nequip", Of::Cmd(5), false),
        Ev::Ended(5, res::INTERRUPT, true),
    ]));
    interrupt_all(&mut w, &mut script, 1, true).expect("interrupt all");
    assert!(script.0.is_empty());
    let set = &w.combat.commands.sets[&1];
    assert_eq!(set.slots[usize::from(ty::STANDARD)], Some(4));
    assert_eq!(set.slots[usize::from(ty::MOVE)], None);
    assert_eq!(set.queue, VecDeque::from([6]));
    assert!(w.combat.commands.cmds[&5].s.finished);
}

/// An attack in turn-based combat, the current unit's: run (the turn's running list takes it), then
/// in one tick it starts, acts and ends (its animation done), and its `SpendAction` follows; the
/// finished command leaves its slot.
#[test]
fn an_attack_runs_starts_acts_and_ends() {
    use crate::combat::turn_tick::Status;
    let (mut w, a) = setup();
    let b = unit(&mut w, "b", Some("enemies"), 5);
    crate::combat::turns::tests::in_combat(&mut w, &[a, b]);
    w.combat.current_unit = Some(a);
    w.combat.turns.ctl.status = Status::Acting;
    w.clock.set_delta_time(0.25);
    let att =
        command(class::ATTACK, state(ty::STANDARD, Target::Unit("b".into()), None), &["Init"]);
    w.combat.commands.cmds.insert(4, att);
    let s0 = state(ty::STANDARD, Target::Unit("b".into()), Some("a"));
    let mut ran = s0.clone();
    ran.did_run = true;
    let mut script = Script(VecDeque::from([
        Ev::Saddled(None),
        Ev::Fx("Init", 4, Fx { s: s0, ret: None, ua: None, fresh: false }),
        Ev::Custom(4, false),
        Ev::Leaf("nequip", Of::Cmd(4), true),
        Ev::Running(4, true),
    ]));
    run(&mut w, &mut script, 1, 4, false, false).expect("run");
    assert!(script.0.is_empty());
    assert_eq!(w.combat.commands.running, [4]);

    let ua = || Of::Unit(Some("a".into()));
    let ub = || Of::Unit(Some("b".into()));
    let mut started = ran.clone();
    started.started = true;
    started.has_animation = true;
    started.animation = true;
    started.anim_acted = Some(false);
    started.anim_finished = Some(false);
    let mut ticked = started.clone();
    ticked.time_since_start = 0.25;
    ticked.anim_acted = Some(true);
    let mut acted = ticked.clone();
    acted.act_handled = true;
    acted.anim_finished = Some(true);
    let mut script = Script(VecDeque::from([
        Ev::Leaf("getup", ua(), false),
        Ev::TickCommand(4, true),
        // TickCommandTurnBased: not getting up; close enough, can stop, able, no cooldown.
        Ev::Leaf("ctrl", ua(), true),
        Ev::Leaf("getup", ua(), false),
        Ev::Leaf("close", Of::Cmd(4), true),
        Ev::Leaf("nostop", Of::Cmd(4), false),
        Ev::Leaf("canact", ua(), true),
        Ev::Leaf("caic", ua(), true),
        // TickCommand: not to be interrupted.
        Ev::Leaf("sbi", Of::Cmd(4), false),
        Ev::Leaf("instate", ub(), true),
        Ev::Leaf("untgt", Of::Pair(Some("a".into()), Some("b".into())), false),
        Ev::Leaf("stealth", ub(), false),
        Ev::Fx("TickApproaching", 4, Fx { s: ran.clone(), ret: None, ua: None, fresh: false }),
        // ShouldStartCommand, then Start.
        Ev::Leaf("hands", ua(), false),
        Ev::Leaf("await", Of::Cmd(4), false),
        Ev::Leaf("canstart", Of::Cmd(4), true),
        Ev::Leaf("close", Of::Cmd(4), true),
        Ev::Leaf("canact", ua(), true),
        Ev::Leaf("equip", ua(), false),
        Ev::Leaf("caic", ua(), true),
        Ev::Leaf("instate", ub(), true),
        Ev::Leaf("close", Of::Cmd(4), true),
        Ev::Fx("OnStart", 4, Fx { s: started, ret: None, ua: None, fresh: false }),
        // Tick: the animation acted, then finished in OnAction: the command ends.
        Ev::Leaf("stt", Of::Cmd(4), true),
        Ev::Leaf("instate", ub(), true),
        Ev::Fx("OnTick", 4, Fx { s: ticked, ret: None, ua: None, fresh: false }),
        Ev::Fx("OnAction", 4, Fx { s: acted, ret: Some(res::SUCCESS), ua: None, fresh: false }),
        Ev::Leaf("nequip", Of::Cmd(4), true),
        Ev::Running(4, false),
        Ev::Ended(4, res::SUCCESS, true),
        Ev::Leaf("full", Of::Cmd(4), false),
        Ev::Spend(ty::STANDARD, false),
        Ev::Leaf("canact", ua(), true),
    ]));
    tick_on_unit(&mut w, &mut script, a, true, false).expect("tick");
    assert!(script.0.is_empty(), "{:?}", script.0);
    let s = &w.combat.commands.cmds[&4].s;
    assert!(s.started && s.acted && s.finished);
    assert_eq!(s.result, res::SUCCESS);
    assert!(w.combat.commands.running.is_empty());
    assert_eq!(w.combat.commands.sets[&1].slots, [None; 4]);
    assert_eq!(w.combat.commands.empty("a"), Some(true));
}

/// Class code may not change what only the base code writes: an `OnTick` that reports the command
/// finished without its end is a desync.
#[test]
fn class_code_cannot_finish_a_command_unannounced() {
    let (mut w, a) = setup();
    let mut s = state(ty::MOVE, Target::Point([0.0; 3]), Some("a"));
    s.started = true;
    s.did_run = true;
    // The AI's: whether it may move is the hold and stop states' (an input).
    let mut x = command(class::MOVE_TO, s.clone(), &[]);
    x.ai = true;
    w.combat.commands.cmds.insert(7, x);
    w.combat.commands.sets.get_mut(&1).expect("set").slots[usize::from(ty::MOVE)] = Some(7);
    let mut done = s.clone();
    done.finished = true;
    let mut script = Script(VecDeque::from([
        Ev::Leaf("getup", Of::Unit(Some("a".into())), false),
        Ev::TickCommand(7, false),
        Ev::Leaf("sbi", Of::Cmd(7), false),
        Ev::Leaf("stt", Of::Cmd(7), false),
        Ev::Fx("OnTick", 7, Fx { s: done, ret: None, ua: None, fresh: false }),
    ]));
    let e = tick_on_unit(&mut w, &mut script, a, true, false).expect_err("desync");
    assert!(e.0.contains("base state after OnTick"), "{e:?}");
}

/// With an uninterruptible command in a slot, a new command waits in the queue (the current one is
/// to stop as soon as it can); once the slot is free, the controller's queue update runs it.
#[test]
fn a_command_waits_in_the_queue_then_runs() {
    let (mut w, _) = setup();
    let mut busy = state(ty::STANDARD, Target::Unit("b".into()), Some("a"));
    busy.started = true;
    w.combat.commands.cmds.insert(4, command(class::ATTACK, busy, &[]));
    w.combat.commands.sets.get_mut(&1).expect("set").slots[usize::from(ty::STANDARD)] = Some(4);
    let next = state(ty::STANDARD, Target::Unit("b".into()), Some("a"));
    w.combat.commands.cmds.insert(5, command(class::ATTACK, next, &[]));
    let mut script = Script(VecDeque::from([
        Ev::Saddled(None),
        // TryUseSpecialHandlers: the running attack doesn't merge it; it's queued instead.
        Ev::Merge(5, 4, false),
        Ev::Leaf("intr", Of::Cmd(4), false),
    ]));
    run(&mut w, &mut script, 1, 5, false, false).expect("run");
    assert!(script.0.is_empty(), "{:?}", script.0);
    let set = &w.combat.commands.sets[&1];
    assert_eq!(set.queue, VecDeque::from([5]));
    assert_eq!(set.slots[usize::from(ty::STANDARD)], Some(4));
    assert!(w.combat.commands.cmds[&4].s.asap);

    // The attack is done: the queue's head runs from the queue (no target check, no Init).
    w.combat.commands.cmds.get_mut(&4).expect("4").s.finished = true;
    let mut ran = w.combat.commands.cmds[&5].s.clone();
    ran.did_run = true;
    let mut script = Script(VecDeque::from([
        Ev::NestedRun(1, 5, true),
        Ev::Custom(5, false),
        Ev::Leaf("nequip", Of::Cmd(5), true),
        Ev::NestedRunEnd(1, 5),
    ]));
    remove_finished_and_update_queue(&mut w, &mut script, 1).expect("queue");
    assert!(script.0.is_empty(), "{:?}", script.0);
    let set = &w.combat.commands.sets[&1];
    assert!(set.queue.is_empty());
    assert_eq!(set.slots[usize::from(ty::STANDARD)], Some(5));
    assert_eq!(w.combat.commands.cmds[&5].s, ran);
}

/// Two commands with one target wrapper are equal without `TargetWrapper.Equals`; with two wrappers
/// the comparison is an input.
#[test]
fn one_target_wrapper_is_equal_without_a_comparison() {
    let (mut w, _) = setup();
    let mut a = state(ty::STANDARD, Target::Unit("b".into()), Some("a"));
    a.target_ref = Some(3);
    w.combat.commands.cmds.insert(4, command(class::ATTACK, a.clone(), &[]));
    w.combat.commands.cmds.insert(5, command(class::ATTACK, a.clone(), &[]));
    let mut script = Script::default();
    assert!(targets_equal(&mut w, &mut script, 4, 5).expect("same wrapper"));
    a.target_ref = Some(7);
    w.combat.commands.cmds.insert(6, command(class::ATTACK, a, &[]));
    let mut script = Script(VecDeque::from([Ev::TargetEquals(false)]));
    assert!(!targets_equal(&mut w, &mut script, 4, 6).expect("two wrappers"));
    assert!(script.0.is_empty());
}

/// A running attack absorbs a new one for the same target (`TryMergeInto`): the running one takes
/// the new one's target, `InterruptAsSoonAsPossible` and AI action; the new one isn't run.
#[test]
fn a_running_attack_merges_a_new_one() {
    let (mut w, _) = setup();
    let mut cur = state(ty::STANDARD, Target::Unit("c".into()), Some("a"));
    cur.started = true;
    w.combat.commands.cmds.insert(4, command(class::ATTACK, cur, &[]));
    w.combat.commands.sets.get_mut(&1).expect("set").slots[usize::from(ty::STANDARD)] = Some(4);
    let mut new =
        command(class::ATTACK, state(ty::STANDARD, Target::Unit("b".into()), Some("a")), &[]);
    new.ai = true;
    new.s.asap = true;
    w.combat.commands.cmds.insert(5, new);
    let mut script = Script(VecDeque::from([Ev::Saddled(None), Ev::Merge(5, 4, true)]));
    run(&mut w, &mut script, 1, 5, false, false).expect("run");
    assert!(script.0.is_empty());
    let x = &w.combat.commands.cmds[&4];
    assert_eq!((x.s.target.clone(), x.s.target_ref), (Target::Unit("b".into()), Some(9)));
    assert!(x.ai && x.s.asap);
    assert_eq!(w.combat.commands.sets[&1].slots[usize::from(ty::STANDARD)], Some(4));
}

/// A failed merge leaves the running command's target: one that reports another is a desync.
#[test]
fn a_failed_merge_cannot_change_the_target() {
    let (mut w, _) = setup();
    let mut cur = state(ty::STANDARD, Target::Unit("c".into()), Some("a"));
    cur.started = true;
    w.combat.commands.cmds.insert(4, command(class::ATTACK, cur, &[]));
    w.combat.commands.sets.get_mut(&1).expect("set").slots[usize::from(ty::STANDARD)] = Some(4);
    let new = state(ty::STANDARD, Target::Unit("b".into()), Some("a"));
    w.combat.commands.cmds.insert(5, command(class::ATTACK, new, &[]));
    let mut script = Script(VecDeque::from([
        Ev::Saddled(None),
        Ev::MergeTarget(5, 4, false, Target::Unit("b".into()), Some(9)),
    ]));
    let e = run(&mut w, &mut script, 1, 5, false, false).expect_err("desync");
    assert!(e.0.contains("failed changed its target"), "{e:?}");
}

/// `ForceFinish` of a finished command runs `OnEnded` again: its leaves and running-list call, but
/// no end record (the observer records only the transition).
#[test]
fn ending_a_finished_command_again_records_no_end() {
    let (mut w, a) = setup();
    let b = unit(&mut w, "b", Some("enemies"), 5);
    crate::combat::turns::tests::in_combat(&mut w, &[a, b]);
    w.combat.current_unit = Some(a);
    let mut s = state(ty::MOVE, Target::Point([0.0; 3]), Some("a"));
    s.finished = true;
    s.result = res::SUCCESS;
    w.combat.commands.cmds.insert(7, command(class::MOVE_TO, s, &[]));
    let mut script =
        Script(VecDeque::from([Ev::Leaf("nequip", Of::Cmd(7), false), Ev::Running(7, false)]));
    force_finish(&mut w, &mut script, 7, res::SUCCESS).expect("again");
    assert!(script.0.is_empty(), "{:?}", script.0);
}

/// `canMoveUnit` is the sim's unless the command is the AI's: false once an earlier command moved
/// the unit, true otherwise.
#[test]
fn can_move_unit_is_checked_where_the_sim_knows_it() {
    let (mut w, a) = setup();
    let s = state(ty::MOVE, Target::Point([0.0; 3]), Some("a"));
    w.combat.commands.cmds.insert(7, command(class::MOVE_TO, s, &[]));
    w.combat.commands.sets.get_mut(&1).expect("set").slots[usize::from(ty::MOVE)] = Some(7);
    let mut script = Script(VecDeque::from([
        Ev::Leaf("getup", Of::Unit(Some("a".into())), false),
        Ev::TickCommand(7, false),
    ]));
    let e = tick_on_unit(&mut w, &mut script, a, true, false).expect_err("desync");
    assert!(e.0.contains("canMoveUnit"), "{e:?}");
}

/// A queued command's `Init` must make it the set owner's: one that leaves no executor is a desync.
#[test]
fn a_queued_command_is_initialized_for_its_set_owner() {
    let (mut w, _) = setup();
    let mut busy = state(ty::STANDARD, Target::Unit("b".into()), Some("a"));
    busy.started = true;
    w.combat.commands.cmds.insert(4, command(class::ATTACK, busy, &[]));
    w.combat.commands.sets.get_mut(&1).expect("set").slots[usize::from(ty::STANDARD)] = Some(4);
    let mut next = command(class::ATTACK, state(ty::STANDARD, Target::Unit("b".into()), None), &[]);
    next.s.target_ref = Some(2);
    w.combat.commands.cmds.insert(5, next.clone());
    let mut script = Script(VecDeque::from([Ev::Fx(
        "Init",
        5,
        Fx { s: next.s, ret: None, ua: None, fresh: false },
    )]));
    let e = add_to_queue_or_run(&mut w, &mut script, 1, 5, false).expect_err("desync");
    assert!(e.0.contains("Init didn't set its executor"), "{e:?}");
}

/// `FixTargetIfTargetOnMount` reads the target unit, null when its reference doesn't resolve.
#[test]
fn an_unresolved_target_is_read_as_none() {
    let (mut w, _) = setup();
    let x =
        command(class::ATTACK, state(ty::STANDARD, Target::Unit("gone".into()), Some("a")), &[]);
    w.combat.commands.cmds.insert(5, x);
    let mut script = Script(VecDeque::from([Ev::SaddledOf(None), Ev::Custom(5, false)]));
    script.0.push_back(Ev::Leaf("nequip", Of::Cmd(5), false));
    run(&mut w, &mut script, 1, 5, false, false).expect("run");
    assert!(script.0.is_empty(), "{:?}", script.0);
}

// ---- UnitAttack (slice B) ----

/// A world with an attacker `a` whose empty hands both wield its fist, targets `b` and `c`, and an
/// attack command 7 of `a` on `b` (not initialized), the attacks observer on.
fn attack_setup() -> (World, EntityIdx) {
    use wr_content::node::Node;
    let (mut w, a) = setup();
    unit(&mut w, "b", Some("enemies"), 5);
    unit(&mut w, "c", Some("enemies"), 4);
    w.combat.commands.attacks = true;
    let ty = crate::content::Blueprint {
        guid: wr_content::Guid(0xA1),
        name: "WeaponType".into(),
        obj: crate::facts::tests::obj(
            "Kingmaker.Blueprints.Items.Weapons.BlueprintWeaponType",
            &[],
        ),
        source: crate::content::Source::Bbp,
    };
    w.content.insert(ty);
    let bp = crate::content::Blueprint {
        guid: wr_content::Guid(0xA2),
        name: "Fist".into(),
        obj: crate::facts::tests::obj(
            "Kingmaker.Blueprints.Items.Weapons.BlueprintItemWeapon",
            &[("m_Type", Node::BpRef(wr_content::Guid(0xA1)))],
        ),
        source: crate::content::Source::Bbp,
    };
    w.content.insert(bp);
    let fist = crate::combat::weapon::create_weapon(&mut w, wr_content::Guid(0xA2), a);
    {
        let body = &mut w.unit_mut(a).expect("unit").body;
        body.empty_hand = Some(fist);
        for hand in ["PrimaryHand", "SecondaryHand"] {
            body.slots.push(crate::unit::Slot {
                name: format!("HandsEquipmentSets[0].{hand}"),
                item: None,
                active: true,
            });
        }
    }
    let mut x = command(class::ATTACK, state(ty::STANDARD, Target::Unit("b".into()), None), &[]);
    x.ua = Some(attack::State::default());
    w.combat.commands.cmds.insert(7, x);
    (w, a)
}

fn entry(slot: &str, penalty: i32) -> attack::Entry {
    attack::Entry::parse(&format!("{slot},,1,0,{penalty},100")).expect("entry")
}

fn counts(main: i32, penalized: i32) -> crate::combat::counts::AttacksCount {
    crate::combat::counts::AttacksCount { additional: main, penalized, ..Default::default() }
}

/// A sequence's previous handle, read released by `OnTick`, stays released (`Release()` never
/// clears `IsReleased`): a later read of it unreleased is a desync.
#[test]
fn a_released_sequence_handle_stays_released() {
    let (mut w, _) = attack_setup();
    let seq = |slot| attack::Attack {
        slot,
        penalty: 0,
        item: String::new(),
        handle: true,
        seq: true,
        skip: false,
        is_hit: false,
    };
    {
        let x = w.combat.commands.cmds.get_mut(&7).expect("cmd");
        x.s.started = true;
        x.s.executor = Some("a".into());
        let ua = x.ua.as_mut().expect("ua");
        ua.list = Some(vec![seq(attack::Slot::Primary), seq(attack::Slot::Primary)]);
        ua.index = 1;
        ua.started_attack = true;
        ua.duration = 6.0;
    }
    let tick = |w: &mut World, released: bool| {
        let mut script = Script(
            vec![
                Ev::LeafInt("style", 7, 1),
                Ev::Leaf("arel", Of::Cmd(7), released),
                Ev::Leaf("close", Of::Cmd(7), true),
                Ev::Leaf("five", Of::CmdUnit(7, Some("b".into())), true),
            ]
            .into(),
        );
        attack::on_tick(w, &mut script, 7)
    };
    tick(&mut w, true).expect("released");
    let ua = w.combat.commands.cmds[&7].ua.clone().expect("ua");
    assert_eq!(ua.released, vec![0]);
    let e = tick(&mut w, false).expect_err("unreleased");
    assert!(format!("{e:?}").contains("unreleased"), "{e:?}");
}

/// `OnAction` releases a skipped sequential entry's handle (`AnimationHandle?.Release()`) as it
/// moves past it: the next `OnTick` reading it unreleased is a desync.
#[test]
fn a_skipped_sequence_handle_is_released() {
    let (mut w, _) = attack_setup();
    let seq = |skip| attack::Attack {
        slot: attack::Slot::Primary,
        penalty: 0,
        item: String::new(),
        handle: true,
        seq: true,
        skip,
        is_hit: false,
    };
    {
        let x = w.combat.commands.cmds.get_mut(&7).expect("cmd");
        x.s.started = true;
        x.s.executor = Some("a".into());
        let ua = x.ua.as_mut().expect("ua");
        ua.list = Some(vec![seq(false), seq(true), seq(false)]);
        ua.full = true;
        ua.started_attack = true;
        ua.duration = 6.0;
    }
    let mut script = Script(
        vec![
            Ev::AttackRule(7, (0, 3, 0, true, true), true),
            Ev::AttackRule(7, (1, 3, 0, false, true), true),
        ]
        .into(),
    );
    attack::on_action(&mut w, &mut script, 7).expect("act");
    assert!(script.0.is_empty(), "{:?}", script.0);
    let ua = w.combat.commands.cmds[&7].ua.clone().expect("ua");
    assert_eq!((ua.index, ua.released.clone()), (2, vec![1]));
    let mut script =
        Script(vec![Ev::LeafInt("style", 7, 1), Ev::Leaf("arel", Of::Cmd(7), false)].into());
    let e = attack::on_tick(&mut w, &mut script, 7).expect_err("unreleased");
    assert!(format!("{e:?}").contains("unreleased"), "{e:?}");
}

/// A full attack without an attack (no hand or limb counts one): `InitAttacks` throws ("Executor
/// can't make any attack", `m_AllAttacks.Empty()`), which isn't modelled and is reported.
#[test]
fn a_full_attack_without_attacks_is_reported() {
    let (mut w, _) = attack_setup();
    let list = attack::List {
        flags: attack::Flags::default(),
        full: true,
        configured: false,
        duration: 6.0,
        attacks: Some(Vec::new()),
    };
    let mut script = Script(VecDeque::from([
        Ev::Leaf("farm", Of::Unit(Some("a".into())), false),
        Ev::Counts(7, false, attack::Counts::Hands(counts(0, 0), counts(0, 0))),
        Ev::Counts(7, true, attack::Counts::Limbs(Vec::new())),
        Ev::AttackList(7, list),
    ]));
    let e = attack::init(&mut w, &mut script, 7, Some("a".into())).expect_err("no attack");
    assert!(format!("{e:?}").contains("no attack"), "{e:?}");
}

/// A full attack of two (an iterative attack at -5): `Init` builds the list from the rules, the first
/// attack starts with its animation and acts, and the second waits `m_AnimationsDuration / count *
/// index` (3 s here, outside turn-based combat), then starts, acts and finishes the command.
#[test]
fn a_full_attack_waits_for_its_iterative_attack() {
    let (mut w, _) = attack_setup();
    let list = || attack::List {
        flags: attack::Flags::default(),
        full: true,
        configured: true,
        duration: 6.0,
        attacks: Some(vec![entry("p", 0), entry("p", 5)]),
    };
    let init_attacks = |list: attack::List| {
        vec![
            Ev::Leaf("farm", Of::Unit(Some("a".into())), false),
            Ev::Counts(7, false, attack::Counts::Hands(counts(1, 1), counts(0, 0))),
            Ev::Counts(7, true, attack::Counts::Limbs(Vec::new())),
            Ev::AttackList(7, list),
        ]
    };
    let mut script = Script(init_attacks(list()).into());
    attack::init(&mut w, &mut script, 7, Some("a".into())).expect("init");
    let st = |w: &World| w.combat.commands.cmds[&7].ua.clone().expect("ua");
    assert!(st(&w).full);
    assert_eq!(st(&w).list.as_ref().map(Vec::len), Some(2));
    w.combat.commands.cmds.get_mut(&7).expect("cmd").s.started = true;
    let mut script = Script(init_attacks(list()).into());
    attack::on_start(&mut w, &mut script, 7).expect("start");
    assert!(w.combat.commands.cmds[&7].s.animation && st(&w).started_attack);
    let tick = |w: &mut World, since: f32, evs: Vec<Ev>| {
        w.combat.commands.cmds.get_mut(&7).expect("cmd").s.time_since_start = since;
        let mut script = Script(evs.into());
        attack::on_tick(w, &mut script, 7).expect("tick");
        assert!(script.0.is_empty(), "{:?}", script.0);
    };
    let close = || Ev::Leaf("close", Of::Cmd(7), true);
    let style = || Ev::LeafInt("style", 7, 1);
    let afin = |v| Ev::Leaf("afin", Of::Cmd(7), v);
    tick(&mut w, 0.5, vec![style(), afin(false), close()]);
    let mut script = Script(vec![Ev::AttackRule(7, (0, 2, 0, true, true), true)].into());
    assert_eq!(attack::on_action(&mut w, &mut script, 7).expect("act"), res::NONE);
    assert!(st(&w).clear_on_finish && !st(&w).started_attack);
    // The first animation finishes: cleared; the next attack waits for its delay.
    tick(&mut w, 1.0, vec![style(), afin(true), close()]);
    assert!(!w.combat.commands.cmds[&7].s.animation && !st(&w).started_attack);
    tick(&mut w, 3.0, vec![style(), close(), afin(false), close()]);
    assert!(w.combat.commands.cmds[&7].s.animation && st(&w).started_attack);
    let mut script = Script(vec![Ev::AttackRule(7, (1, 2, 5, false, true), false)].into());
    assert_eq!(attack::on_action(&mut w, &mut script, 7).expect("act"), res::SUCCESS);
    assert_eq!(st(&w).last_number, Some(1));
    assert!(st(&w).list.expect("list")[0].is_hit);
}

/// In turn-based combat the delay is at most `MaxDelayBetweenIterativeAttacks` (1 s) per attack,
/// and a bow's attacks start at once.
#[test]
fn turn_based_iterative_attacks_wait_a_second_and_bows_none() {
    let (mut w, _) = attack_setup();
    w.combat.turn_based = true;
    let s = attack::State {
        list: Some(
            [0, 5]
                .into_iter()
                .map(|p| attack::Attack {
                    slot: attack::Slot::Primary,
                    penalty: p,
                    item: String::new(),
                    handle: false,
                    seq: false,
                    skip: false,
                    is_hit: false,
                })
                .collect(),
        ),
        index: 1,
        duration: 6.0,
        full: true,
        ..attack::State::default()
    };
    let x = w.combat.commands.cmds.get_mut(&7).expect("cmd");
    x.ua = Some(s);
    x.s.executor = Some("a".into());
    x.s.started = true;
    let five = || Ev::Leaf("five", Of::CmdUnit(7, Some("b".into())), true);
    let close = || Ev::Leaf("close", Of::Cmd(7), true);
    x.s.time_since_start = 0.9;
    let mut script = Script(vec![Ev::LeafInt("style", 7, 1), close(), five()].into());
    attack::on_tick(&mut w, &mut script, 7).expect("tick");
    assert!(!w.combat.commands.cmds[&7].ua.as_ref().expect("ua").started_attack, "before 1 s");
    w.combat.commands.cmds.get_mut(&7).expect("cmd").s.time_since_start = 1.0;
    let mut script =
        Script(vec![Ev::LeafInt("style", 7, 1), close(), five(), close(), five()].into());
    attack::on_tick(&mut w, &mut script, 7).expect("tick");
    let x = &w.combat.commands.cmds[&7];
    assert!(x.ua.as_ref().expect("ua").started_attack && !x.s.animation);
    // Without a handle the act is scheduled a second on.
    assert!((x.s.pretend_act_time - 2.0).abs() < f32::EPSILON);
    // A bow: the delay is the time since the start.
    let x = w.combat.commands.cmds.get_mut(&7).expect("cmd");
    x.ua.as_mut().expect("ua").started_attack = false;
    x.s.time_since_start = 0.1;
    let mut script =
        Script(vec![Ev::LeafInt("style", 7, 9), close(), five(), close(), five()].into());
    attack::on_tick(&mut w, &mut script, 7).expect("tick");
    assert!(w.combat.commands.cmds[&7].ua.as_ref().expect("ua").started_attack);
}

/// A single attack takes the off hand when only it has main attacks; when the target dies, the
/// nearest conscious enemy in reach (the memory's, in the recorded order) becomes the target.
#[test]
fn a_single_attack_takes_the_off_hand_and_retargets() {
    let (mut w, _) = attack_setup();
    w.combat.commands.cmds.get_mut(&7).expect("cmd").ua = Some(attack::State {
        flags: attack::Flags { single: true, ..Default::default() },
        ..Default::default()
    });
    let list = attack::List {
        flags: attack::Flags { single: true, ..Default::default() },
        full: false,
        configured: true,
        duration: 6.0,
        attacks: Some(vec![entry("s", 0)]),
    };
    let mut script = Script(
        vec![
            Ev::Counts(7, false, attack::Counts::Hands(counts(0, 0), counts(1, 0))),
            Ev::AttackList(7, list),
        ]
        .into(),
    );
    attack::init(&mut w, &mut script, 7, Some("a".into())).expect("init");
    let st = |w: &World| w.combat.commands.cmds[&7].ua.clone().expect("ua");
    assert_eq!(st(&w).list.expect("list")[0].slot, attack::Slot::Secondary);
    // `b` dies; `c` is in reach.
    let b = w.by_id["b"];
    w.unit_mut(b).expect("b").life_state = 2;
    w.combat.commands.cmds.get_mut(&7).expect("cmd").s.started = true;
    let mut script = Script(
        vec![
            Ev::LeafInt("style", 7, 1),
            Ev::LeafIds("enemies", 7, vec![Some("c".into()), Some("b".into())]),
            Ev::Leaf("five", Of::CmdUnit(7, Some("c".into())), true),
            Ev::Leaf("five", Of::CmdUnit(7, Some("b".into())), false),
            Ev::LeafIds("sorted", 7, vec![Some("c".into())]),
            Ev::Leaf("untgt", Of::Pair(Some("a".into()), Some("c".into())), false),
            Ev::Leaf("close", Of::Cmd(7), true),
        ]
        .into(),
    );
    st(&w);
    w.combat.commands.cmds.get_mut(&7).expect("cmd").ua.as_mut().expect("ua").started_attack = true;
    attack::on_tick(&mut w, &mut script, 7).expect("tick");
    assert!(script.0.is_empty(), "{:?}", script.0);
    let x = &w.combat.commands.cmds[&7];
    assert_eq!(x.s.target, Target::Unit("c".into()));
    assert!(x.ua.as_ref().expect("ua").new_wrapper && !x.s.finished);
    // A sorted list that isn't the sim's candidates is a desync.
    let mut script = Script(
        vec![
            Ev::LeafInt("style", 7, 1),
            Ev::Leaf("close", Of::Cmd(7), false),
            Ev::LeafIds("enemies", 7, vec![Some("c".into())]),
            Ev::Leaf("five", Of::CmdUnit(7, Some("c".into())), true),
            Ev::LeafIds("sorted", 7, vec![Some("b".into())]),
        ]
        .into(),
    );
    let e = attack::on_tick(&mut w, &mut script, 7).expect_err("desync");
    assert!(e.0.contains("candidates"), "{e:?}");
}

// ---- slice C: the approach, `UnitMoveTo`, the attack of opportunity ----

/// A world with the `approach` observer on and a described `UnitMoveTo` 7 of `a` to a point.
fn approach_setup() -> (World, EntityIdx) {
    let (mut w, a) = setup();
    w.combat.commands.approach = true;
    let mut x = command(
        class::MOVE_TO,
        state(ty::MOVE, Target::Point([1.0, 0.0, 2.0]), Some("a")),
        &["OnRun"],
    );
    x.ap = approach::Params::parse("1|1000000|10");
    x.mv = move_to::Fields::parse("000|-|-|0");
    assert!(x.ap.is_some() && x.mv.is_some());
    w.combat.commands.cmds.insert(7, x);
    w.combat.commands.sets.get_mut(&1).expect("set").slots[usize::from(ty::MOVE)] = Some(7);
    (w, a)
}

fn moveto(radius: Option<f32>) -> approach::Call {
    approach::Call::MoveTo {
        c: 7,
        unit: "a".into(),
        point: Some([1.0, 0.0, 2.0]),
        radius,
        max_radius: radius.map(|r| r.max(1_000_000.0)),
        target: "a".into(),
        xz: true,
    }
}

/// Far from its point, a move asks the agent to path there; once close, it stops it and draws its
/// next approach time (`GameTime + Range(0.2, 0.6)`, in whole milliseconds).
#[test]
fn an_approach_paths_then_stops_and_draws_its_next_time() {
    let (mut w, _) = approach_setup();
    let mut script = Script(VecDeque::from([
        // `ShouldUnitApproach`: not moving yet, not close.
        Ev::Leaf("close", Of::Cmd(7), false),
        Ev::Leaf("spart", Of::Cmd(7), false),
        Ev::Leaf("rpart", Of::Cmd(7), false),
        Ev::Leaf("close", Of::Cmd(7), false),
        Ev::Leaf("tact", Of::Cmd(7), false),
        Ev::Call(moveto(Some(1.0))),
    ]));
    approach::tick_approaching(&mut w, &mut script, 7).expect("approach");
    assert!(script.0.is_empty(), "{:?}", script.0);
    let s = &w.combat.commands.cmds[&7].s;
    assert!(s.move_unit && !s.finished_approaching);
    // Arrived: `IsMoveUnit && AwaitMovementFinish && IsMoving` reads the agent first.
    let t0 = w.clock.game_time.0;
    let mut script = Script(VecDeque::from([
        Ev::Leaf("await", Of::Cmd(7), true),
        Ev::Leaf("moving", Of::Unit(Some("a".into())), false),
        Ev::Leaf("close", Of::Cmd(7), true),
        Ev::Leaf("nostop", Of::Cmd(7), false),
        Ev::Leaf("spart", Of::Cmd(7), false),
        Ev::Leaf("rpart", Of::Cmd(7), false),
        Ev::Leaf("await", Of::Cmd(7), true),
        Ev::Leaf("moving", Of::Unit(Some("a".into())), false),
        Ev::Leaf("close", Of::Cmd(7), true),
        Ev::Leaf("nostop", Of::Cmd(7), false),
        Ev::Call(approach::Call::Stop { unit: "a".into(), c: Some(7) }),
        Ev::Leaf("stt", Of::Cmd(7), true),
        Ev::Draw(7, 0.482_229_24),
    ]));
    approach::tick_approaching(&mut w, &mut script, 7).expect("arrive");
    assert!(script.0.is_empty(), "{:?}", script.0);
    let s = &w.combat.commands.cmds[&7].s;
    assert!(s.finished_approaching);
    assert_eq!(s.next_approach, t0 + 482 * 10_000);
}

/// A `UnitMoveTo`'s answers are its own: a recorded `DontWaitForHands` of false is a desync.
#[test]
fn a_move_answers_the_lifecycle_itself() {
    let (mut w, _) = approach_setup();
    let mut script = Script(VecDeque::from([Ev::Leaf("dwfh", Of::Cmd(7), true)]));
    assert!(answer(&mut w, &mut script, "dwfh", 7).expect("answer"));
    let mut script = Script(VecDeque::from([Ev::Leaf("dwfh", Of::Cmd(7), false)]));
    let e = answer(&mut w, &mut script, "dwfh", 7).expect_err("desync");
    assert!(e.0.contains("dwfh"), "{e:?}");
    // Its await override.
    w.combat.commands.cmds.get_mut(&7).expect("cmd").mv = move_to::Fields::parse("000|-|0|0");
    let mut script = Script(VecDeque::from([Ev::Leaf("await", Of::Cmd(7), true)]));
    assert!(answer(&mut w, &mut script, "await", 7).is_err());
}

/// In turn-based combat a unit that wants to move but can't before its next approach time is
/// interrupted (the log message reads the movement and `CanMove` again).
#[test]
fn a_stuck_approach_is_interrupted() {
    let (mut w, a) = approach_setup();
    w.combat.turn_based = true;
    w.combat.current_unit = Some(a);
    w.combat.commands.cmds.get_mut(&7).expect("cmd").s.next_approach = i64::MAX;
    let mut script = Script(VecDeque::from([
        Ev::Leaf("close", Of::Cmd(7), false),
        Ev::Leaf("spart", Of::Cmd(7), false),
        Ev::Leaf("rpart", Of::Cmd(7), false),
        Ev::Leaf("close", Of::Cmd(7), false),
        Ev::Leaf("wants", Of::Cmd(7), false),
        Ev::Leaf("hasmove", Of::Cmd(7), true),
        Ev::Leaf("canmove", Of::Cmd(7), false),
        Ev::Leaf("ucgu", Of::Cmd(7), false),
        Ev::Leaf("hasmove", Of::Cmd(7), true),
        Ev::Leaf("canmove", Of::Cmd(7), false),
        Ev::Leaf("nequip", Of::Cmd(7), false),
        Ev::Running(7, false),
        Ev::Ended(7, res::INTERRUPT, true),
    ]));
    approach::tick_approaching(&mut w, &mut script, 7).expect("interrupt");
    assert!(script.0.is_empty(), "{:?}", script.0);
    assert_eq!(w.combat.commands.cmds[&7].s.result, res::INTERRUPT);
}

/// In turn-based combat the approach follows the command's path (or the path visualizer's), saved
/// for the turn; without one the command is interrupted.
#[test]
fn a_turn_based_approach_follows_its_path() {
    let (mut w, a) = approach_setup();
    w.combat.turn_based = true;
    w.combat.current_unit = Some(a);
    w.unit_mut(a).expect("a").combat.in_combat = true;
    let path = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 2.0]];
    let head = || {
        vec![
            Ev::Leaf("close", Of::Cmd(7), false),
            Ev::Leaf("spart", Of::Cmd(7), false),
            Ev::Leaf("rpart", Of::Cmd(7), false),
            Ev::Leaf("close", Of::Cmd(7), false),
            Ev::Leaf("tact", Of::Cmd(7), false),
        ]
    };
    let mut ev = head();
    ev.extend([
        Ev::Path("fpath", 7, None),
        Ev::Leaf("pvis", Of::Cmd(7), true),
        Ev::Path("vpath", 7, Some(path.clone())),
        Ev::Call(approach::Call::Follow {
            c: 7,
            unit: "a".into(),
            path: path.clone(),
            forced: false,
            radius: Some(1.0),
        }),
    ]);
    let mut script = Script(ev.into());
    approach::tick_approaching(&mut w, &mut script, 7).expect("follow");
    assert!(script.0.is_empty(), "{:?}", script.0);
    assert_eq!(w.combat.commands.cmds[&7].s.next_approach, i64::MAX);
    assert_eq!(w.combat.turns.ctl.saved_command_path.as_ref(), Some(&path));
    // An empty path interrupts.
    let s = &mut w.combat.commands.cmds.get_mut(&7).expect("cmd").s;
    (s.next_approach, s.move_unit) = (0, false);
    let mut ev = head();
    ev.extend([
        Ev::Path("fpath", 7, Some(Vec::new())),
        Ev::Leaf("nequip", Of::Cmd(7), false),
        Ev::Running(7, false),
        Ev::Ended(7, res::INTERRUPT, true),
    ]);
    let mut script = Script(ev.into());
    approach::tick_approaching(&mut w, &mut script, 7).expect("interrupt");
    assert!(script.0.is_empty(), "{:?}", script.0);
}

/// A `UnitMoveTo` acts as it arrives: it stops the view and succeeds, unless its AI action
/// prevents the finish.
#[test]
fn a_move_stops_and_succeeds_unless_its_ai_action_prevents_it() {
    let (mut w, _) = approach_setup();
    let mut script =
        Script(VecDeque::from([Ev::Call(approach::Call::Stop { unit: "a".into(), c: Some(7) })]));
    assert_eq!(move_to::on_action(&mut w, &mut script, 7).expect("act"), res::SUCCESS);
    w.combat.commands.cmds.get_mut(&7).expect("cmd").ai = true;
    let mut script = Script(VecDeque::from([
        Ev::Call(approach::Call::Stop { unit: "a".into(), c: Some(7) }),
        Ev::Leaf("pmf", Of::Cmd(7), true),
    ]));
    assert_eq!(move_to::on_action(&mut w, &mut script, 7).expect("act"), res::NONE);
    assert!(script.0.is_empty());
}

/// An attack of opportunity takes its threat hand at `Init` (none interrupts it) and makes its
/// attack, flagged as one, from `OnAction`; its `ShouldTurnToTarget` is its creator's.
#[test]
fn an_attack_of_opportunity_takes_its_threat_hand_and_attacks() {
    let (mut w, _) = attack_setup();
    w.combat.commands.approach = true;
    let mut x = command(class::AOO, state(ty::FREE, Target::Unit("b".into()), None), &[]);
    x.ap = approach::Params::parse("1000000|0|00");
    x.aoo = aoo::State::parse("10");
    w.combat.commands.cmds.insert(8, x);
    // The empty hand (a melee weapon, not unarmed) threatens.
    let mut script =
        Script(VecDeque::from([Ev::Threat(8, Some(attack::Slot::Primary), Some(String::new()))]));
    aoo::init(&mut w, &mut script, 8, Some("a".into())).expect("init");
    assert!(script.0.is_empty());
    assert!(!w.combat.commands.cmds[&8].s.finished);
    let mut script = Script(VecDeque::from([Ev::AttackRule(8, (0, 0, 0, false, false), true)]));
    assert_eq!(aoo::on_action(&mut w, &mut script, 8).expect("act"), res::SUCCESS);
    assert!(script.0.is_empty());
    assert_eq!(aoo::answer(&w, 8, "stt").expect("stt"), Some(false));
    assert_eq!(aoo::answer(&w, 8, "oneframe").expect("oneframe"), Some(true));
    // A dead target: no attack.
    let b = w.by_id["b"];
    w.unit_mut(b).expect("b").life_state = 2;
    let mut script = Script(VecDeque::new());
    assert_eq!(aoo::on_action(&mut w, &mut script, 8).expect("act"), res::SUCCESS);
}

/// Without a threat hand (an unarmed strike without improved unarmed strike) the attack of
/// opportunity is interrupted in `Init`.
#[test]
fn an_attack_of_opportunity_without_a_threat_hand_is_interrupted() {
    use wr_content::node::Node;
    let (mut w, a) = attack_setup();
    w.combat.commands.approach = true;
    let ty = crate::content::Blueprint {
        guid: wr_content::Guid(0xA3),
        name: "UnarmedType".into(),
        obj: crate::facts::tests::obj(
            "Kingmaker.Blueprints.Items.Weapons.BlueprintWeaponType",
            &[("m_IsUnarmed", Node::Bool(true))],
        ),
        source: crate::content::Source::Bbp,
    };
    w.content.insert(ty);
    let bp = crate::content::Blueprint {
        guid: wr_content::Guid(0xA4),
        name: "UnarmedFist".into(),
        obj: crate::facts::tests::obj(
            "Kingmaker.Blueprints.Items.Weapons.BlueprintItemWeapon",
            &[("m_Type", Node::BpRef(wr_content::Guid(0xA3)))],
        ),
        source: crate::content::Source::Bbp,
    };
    w.content.insert(bp);
    let fist = crate::combat::weapon::create_weapon(&mut w, wr_content::Guid(0xA4), a);
    w.unit_mut(a).expect("a").body.empty_hand = Some(fist);
    assert_eq!(aoo::threat_hand(&mut w, a), None);
    let mut x = command(class::AOO, state(ty::FREE, Target::Unit("b".into()), None), &[]);
    x.ap = approach::Params::parse("1000000|0|00");
    x.aoo = aoo::State::parse("01");
    w.combat.commands.cmds.insert(8, x);
    let mut script = Script(VecDeque::from([
        Ev::Threat(8, None, None),
        Ev::Leaf("nequip", Of::Cmd(8), false),
        Ev::Ended(8, res::INTERRUPT, true),
    ]));
    aoo::init(&mut w, &mut script, 8, Some("a".into())).expect("init");
    assert!(script.0.is_empty(), "{:?}", script.0);
    assert_eq!(w.combat.commands.cmds[&8].s.result, res::INTERRUPT);
}

#[test]
fn approach_descriptions_parse() {
    let p = approach::Params::parse("0.3|1000000|01").expect("ap");
    assert_eq!((p.radius, p.max_radius, p.xz, p.los), (0.3, 1_000_000.0, false, true));
    assert!(approach::Params::parse("NaN|0|00").is_none());
    assert!(approach::Params::parse("1|0|0").is_none());
    let m = move_to::Fields::parse("101|1.5|1|0.25").expect("mv");
    assert!(m.roaming && !m.prevent_interrupt && m.rider_target);
    assert_eq!((m.orientation, m.await_override, m.movement_delay), (Some(1.5), Some(true), 0.25));
    assert!(move_to::Fields::parse("000|-|2|0").is_none());
    let s = aoo::State::parse("10").expect("aoo");
    assert!(s.force_flat_footed && !s.turn);
    assert!(aoo::State::parse("1").is_none());
}

/// `MoveTo`'s target view: the target unit's while it's alive (`tview`), else the executor's own;
/// a unit's position is an input (no point to check).
#[test]
fn a_move_to_a_unit_without_a_live_view_paths_with_its_own() {
    let (mut w, _) = approach_setup();
    unit(&mut w, "b", None, 5);
    let x = w.combat.commands.cmds.get_mut(&7).expect("cmd");
    x.s.target = Target::Unit("b".into());
    for (alive, target) in [(true, "b"), (false, "a")] {
        w.combat.commands.cmds.get_mut(&7).expect("cmd").s.move_unit = false;
        let mut script = Script(VecDeque::from([
            Ev::Leaf("close", Of::Cmd(7), false),
            Ev::Leaf("spart", Of::Cmd(7), false),
            Ev::Leaf("rpart", Of::Cmd(7), false),
            Ev::Leaf("close", Of::Cmd(7), false),
            Ev::Leaf("tact", Of::Cmd(7), false),
            Ev::Leaf("tview", Of::Cmd(7), alive),
            Ev::Call(approach::Call::MoveTo {
                c: 7,
                unit: "a".into(),
                point: None,
                radius: Some(1.0),
                max_radius: Some(1_000_000.0),
                target: target.into(),
                xz: true,
            }),
        ]));
        approach::tick_approaching(&mut w, &mut script, 7).expect("approach");
        assert!(script.0.is_empty(), "{:?}", script.0);
    }
}

/// An attack of opportunity whose threat limb's item is removed or replaced after `Init` (the
/// slot object it holds isn't followed by the limb's index): reported, not attacked with.
#[test]
fn an_attack_of_opportunity_with_a_changed_threat_limb_is_reported() {
    let (mut w, a) = attack_setup();
    w.combat.commands.approach = true;
    let fist = w.unit(a).and_then(|x| x.body.empty_hand).expect("fist");
    let claw = crate::combat::weapon::create_weapon(&mut w, wr_content::Guid(0xA2), a);
    w.unit_mut(a).expect("a").body.limbs.push(crate::unit::Slot {
        name: "AdditionalLimbs[0]".into(),
        item: Some(claw),
        active: true,
    });
    let mut x = command(class::AOO, state(ty::FREE, Target::Unit("b".into()), None), &[]);
    x.ap = approach::Params::parse("1000000|0|00");
    x.aoo = aoo::State::parse("01");
    w.combat.commands.cmds.insert(8, x);
    // The hands don't threaten here: the limb does.
    w.unit_mut(a).expect("a").body.empty_hand = None;
    let mut script =
        Script(VecDeque::from([Ev::Threat(8, Some(attack::Slot::Limb(0)), Some("claw".into()))]));
    aoo::init(&mut w, &mut script, 8, Some("a".into())).expect("init");
    w.unit_mut(a).expect("a").body.empty_hand = Some(fist);
    w.unit_mut(a).expect("a").body.limbs[0].item = None;
    let mut script = Script(VecDeque::new());
    let e = aoo::on_action(&mut w, &mut script, 8).expect_err("reported");
    assert!(e.0.contains("threat limb's item changed"), "{e:?}");
}
