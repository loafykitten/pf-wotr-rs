//! The agent against WrathOracle's `agent` records: every recorded `TickMovement` rerun from its
//! state before (`ag_tick`) with the leaves, physics and draws recorded inside it, its result and
//! state after compared with `ag_tick_end` bit for bit.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::combat::oracle::Rv;

use super::*;
use crate::combat::oracle::Trace;
use crate::combat::turns::tests::{unit, world};

fn f(v: &Rv) -> f32 {
    v.as_f64().map_or(f32::NAN, |x| x as f32)
}

fn v3(v: &Rv) -> V3 {
    [f(&v[0]), f(&v[1]), f(&v[2])]
}

fn v2(v: &Rv) -> V2 {
    [f(&v[0]), f(&v[1])]
}

fn opt_v3(v: &Rv) -> Option<V3> {
    if v.is_null() { None } else { Some(v3(v)) }
}

fn opt_f(v: &Rv) -> Option<f32> {
    if v.is_null() { None } else { Some(f(v)) }
}

fn b(v: &Rv) -> bool {
    v.as_bool().unwrap_or(false)
}

/// The paths the window defined so far, by key.
#[derive(Default)]
struct Paths(HashMap<String, Vec<V3>>);

impl Paths {
    fn note(&mut self, r: &Rv) {
        let ev = r["ev"].as_str().unwrap_or("");
        let key = r["path"].as_str().map(str::to_owned);
        if let (Some(key), true) =
            (key, ev == "ag_done" || (ev == "ag_pathdef" && !b(&r["pending"])))
        {
            let pts = r["v"].as_array().map(|a| a.iter().map(v3).collect()).unwrap_or_default();
            self.0.insert(key, pts);
        }
    }

    fn get(&self, key: &Rv, kind_forced: bool) -> Option<AgentPath> {
        let key = key.as_str()?;
        Some(AgentPath {
            key: key.to_owned(),
            points: self.0.get(key).cloned().unwrap_or_default(),
            forced: kind_forced || key.starts_with("?ForcedPath"),
        })
    }
}

/// `m_DestinationAgent` from a recorded state (`dhas` since `poc5-tb-102`; before, a destination
/// is known only with its view).
fn dest_from(s: &Rv) -> Option<DestAgent> {
    let view = s["dview"].as_str().map(str::to_owned);
    let has = s.get("dhas").map_or(view.is_some(), b);
    has.then(|| DestAgent { view, attached: b(&s["dattached"]) })
}

/// An agent from a recorded state `s`.
fn agent_from(s: &Rv, paths: &Paths) -> Agent {
    Agent {
        pos: v3(&s["pos"]),
        vel: v3(&s["vel"]),
        next_vel: v3(&s["nvel"]),
        path: paths.get(&s["path"], false),
        requested: paths.get(&s["req"], false),
        requested_find: paths.get(&s["find"], false),
        dest_agent: dest_from(s),
        requested_new: b(&s["reqnew"]),
        roaming: b(&s["roam"]),
        force_roaming: b(&s["froam"]),
        first_tick: b(&s["first"]),
        force: b(&s["force"]),
        follow: b(&s["follow"]),
        no_more_tb: b(&s["nomoretb"]),
        repath: b(&s["repath"]),
        stopping: b(&s["stopping"]),
        avoidance_off: b(&s["avoidoff"]),
        connected: b(&s["conn"]),
        warmup: f(&s["warmup"]),
        remaining: f(&s["remain"]),
        eta: f(&s["eta"]),
        last_turn: f(&s["lastturn"]),
        stuck_stop: f(&s["stuckstop"]),
        stuck_dir: f(&s["stuckdir"]),
        min_wp: f(&s["minwp"]),
        slow_down: f(&s["slow"]),
        speed: f(&s["speed"]),
        min_speed: f(&s["minspeed"]),
        accel: f(&s["accel"]),
        angular: f(&s["ang"]),
        combat_angular: f(&s["cang"]),
        approach: f(&s["ar"]),
        max_approach: f(&s["mar"]),
        saved_min_speed: opt_f(&s["savedmin"]),
        max_speed_override: opt_f(&s["maxover"]),
        next_index: s["idx"].as_i64().unwrap_or(0) as i32,
        path_frame: s["pframe"].as_i64().unwrap_or(0) as i32,
        repath_after: s["repathafter"].as_i64().unwrap_or(-1) as i32,
        charging: s["charging"].as_i64().unwrap_or(0) as i32,
        next_wp: v2(&s["wp"]),
        next_next_wp: v2(&s["wp2"]),
        next_dir: v2(&s["ndir"]),
        move_dir: v2(&s["mdir"]),
        dest: opt_v3(&s["dest"]),
        no_path_dest: opt_v3(&s["nopath"]),
    }
}

/// The fields that differ, by their bits (`connected` only where the recording has it:
/// `poc5-tb-100` predates it).
fn diff(ours: &Agent, want: &Agent, has_conn: bool) -> Vec<String> {
    let mut out = Vec::new();
    macro_rules! cmp {
        ($($f:ident),*) => {$(
            if format!("{:?}", bits(&ours.$f)) != format!("{:?}", bits(&want.$f)) {
                out.push(format!("{}: {:?} != {:?}", stringify!($f), ours.$f, want.$f));
            }
        )*};
    }
    cmp!(
        pos,
        vel,
        next_vel,
        requested_new,
        force_roaming,
        avoidance_off,
        accel,
        angular,
        combat_angular,
        max_speed_override,
        path_frame,
        charging,
        roaming,
        first_tick,
        force,
        follow,
        no_more_tb,
        repath,
        stopping,
        warmup,
        remaining,
        eta,
        last_turn,
        stuck_stop,
        stuck_dir,
        min_wp,
        slow_down,
        speed,
        min_speed,
        approach,
        max_approach,
        saved_min_speed,
        next_index,
        repath_after,
        next_wp,
        next_next_wp,
        next_dir,
        move_dir,
        dest,
        no_path_dest
    );
    if ours.dest_agent != want.dest_agent {
        out.push(format!("dest_agent: {:?} != {:?}", ours.dest_agent, want.dest_agent));
    }
    if has_conn && ours.connected != want.connected {
        out.push(format!("connected: {} != {}", ours.connected, want.connected));
    }
    if ours.path.as_ref().map(|p| &p.key) != want.path.as_ref().map(|p| &p.key) {
        out.push(format!(
            "path: {:?} != {:?}",
            ours.path.as_ref().map(|p| &p.key),
            want.path.as_ref().map(|p| &p.key)
        ));
    }
    if ours.requested_find.as_ref().map(|p| &p.key) != want.requested_find.as_ref().map(|p| &p.key)
    {
        out.push("requested_find".into());
    }
    if ours.requested.as_ref().map(|p| &p.key) != want.requested.as_ref().map(|p| &p.key) {
        out.push("requested".into());
    }
    out
}

/// Field values as bits, so -0 and NaNs compare exactly.
trait Bits {
    fn bits(&self) -> String;
}

fn bits<T: Bits>(x: &T) -> String {
    x.bits()
}

impl Bits for f32 {
    fn bits(&self) -> String {
        format!("{:08x}", self.to_bits())
    }
}
impl Bits for bool {
    fn bits(&self) -> String {
        self.to_string()
    }
}
impl Bits for i32 {
    fn bits(&self) -> String {
        self.to_string()
    }
}
impl<const N: usize> Bits for [f32; N] {
    fn bits(&self) -> String {
        self.iter().map(Bits::bits).collect::<Vec<_>>().join(",")
    }
}
impl<T: Bits> Bits for Option<T> {
    fn bits(&self) -> String {
        self.as_ref().map_or("none".into(), Bits::bits)
    }
}

/// The inputs recorded inside one `TickMovement` (its agent records and draws, in order).
struct Recorded<'a> {
    tick: &'a Rv,
    recs: Vec<&'a Rv>,
    at: usize,
    errors: Vec<String>,
    /// The capture records the agent's callbacks into the view (`ag_cb`, since `poc5-tb-103`).
    has_cb: bool,
    /// For `PathTo`: the request's recorded key, `InitMoveDirection`'s result (taken from the
    /// state after until orientation is modelled) and the grid's erosion settings (nav file).
    key: Option<String>,
    init_dir: Option<V2>,
    erosion: Option<(i32, i32)>,
}

/// Records that aren't checked in order: `canmovetb` (the postfix of the call around its
/// leaves), the ends of the agent's own calls (their starts are checked) and path definitions.
fn not_input(r: &Rv) -> bool {
    (r["ev"] == "ag_v" && r["f"] == "canmovetb")
        || r["ev"] == "ag_call_end"
        || r["ev"] == "ag_pathdef"
}

impl Recorded<'_> {
    /// The next input record (consumed).
    fn next_input(&mut self) -> Option<&Rv> {
        while let Some(r) = self.recs.get(self.at).copied() {
            self.at += 1;
            if !not_input(r) {
                return Some(r);
            }
        }
        None
    }

    /// The next input record, which must be an `ev` record.
    fn take_ev(&mut self, ev: &str) -> R<Rv> {
        match self.next_input() {
            Some(r) if r["ev"] == ev => Ok(r.clone()),
            Some(r) => Err(Desync(format!("wanted {ev}, the recording has {r}"))),
            None => Err(Desync(format!("wanted {ev}, the records ended"))),
        }
    }

    /// The next record that must be the agent leaf `f`.
    fn take_leaf(&mut self, f: &str) -> R<&Rv> {
        match self.next_input() {
            Some(r) if r["ev"] == "ag_v" && r["f"] == f => Ok(r),
            Some(r) => Err(Desync(format!("wanted leaf {f}, the recording has {r}"))),
            None => Err(Desync(format!("wanted leaf {f}, the tick's records ended"))),
        }
    }

    /// The next record of any kind, not consumed (the agent's calls included).
    fn peek_leaf(&self) -> Option<&Rv> {
        self.recs[self.at..]
            .iter()
            .copied()
            .find(|r| !(r["ev"] == "ag_v" && r["f"] == "canmovetb") && r["ev"] != "ag_pathdef")
    }

    /// The next input record, not consumed.
    fn peek_input(&self) -> Option<&Rv> {
        self.recs[self.at..].iter().copied().find(|r| !not_input(r))
    }

    /// A call into the view: the recording's (with its destination or index), then what the
    /// unit did inside it as far as the agent sees it: a `Stop` re-entering the agent (the move
    /// command's end stopping the view) runs the model's own `stop`.
    fn callback(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        f: &'static str,
        dest: Option<V3>,
        index: Option<i32>,
    ) -> R<()> {
        if !self.has_cb {
            return Ok(());
        }
        let r = match self.next_input() {
            Some(r) if r["ev"] == "ag_cb" && r["fn"] == f => r.clone(),
            Some(r) => {
                return Err(Desync(format!(
                    "the agent calls the view's {f}, the recording has {r}"
                )));
            }
            None => {
                return Err(Desync(format!(
                    "the agent calls the view's {f}, the tick's records ended"
                )));
            }
        };
        if bits(&opt_v3(&r["dest"])) != bits(&dest) || r["i"].as_i64().map(|x| x as i32) != index {
            return Err(Desync(format!("{f}({dest:?}, {index:?}): the recording's is {r}")));
        }
        loop {
            let Some(x) = self.peek_input().cloned() else {
                return Err(Desync(format!("{f} without its end")));
            };
            if x["ev"] == "ag_cb_end" && x["fn"] == f {
                self.next_input();
                return Ok(());
            }
            if x["ev"] == "ag_call" && x["fn"] == "Stop" {
                stop(w, u, self)?;
                continue;
            }
            return Err(Desync(format!("inside {f}: {x} (only a re-entering Stop is modelled)")));
        }
    }

    fn can_move_dt(&self) -> R<f32> {
        self.recs
            .iter()
            .find(|r| r["ev"] == "ag_v" && r["f"] == "canmovetb")
            .map(|r| f(&r["dt"]))
            .ok_or_else(|| Desync("no canmovetb record".into()))
    }

    /// The next physics query, which must be the agent's: kind, origin, direction (a ray) or
    /// radius (a disc), distance and mask, bit for bit.
    fn take_phys(
        &mut self,
        kind: &str,
        o: V3,
        shape: &[f32],
        max: f32,
        mask: i32,
    ) -> R<Option<f32>> {
        let Some(r) = self.next_input().cloned() else {
            return Err(Desync(format!("wanted a {kind} query, the tick's records ended")));
        };
        if r["ev"] != "ag_phys" || r["fn"] != kind {
            return Err(Desync(format!("wanted a {kind} query, the recording has {r}")));
        }
        let recorded: Vec<f32> =
            if kind == "ray" { v3(&r["d"]).to_vec() } else { vec![f(&r["r"])] };
        if bits(&v3(&r["o"])) != bits(&o)
            || !recorded.iter().map(|x| x.to_bits()).eq(shape.iter().map(|x| x.to_bits()))
            || bits(&f(&r["max"])) != bits(&max)
            || r["mask"].as_i64() != Some(i64::from(mask))
        {
            self.errors.push(format!(
                "{kind} query from {o:?}: the recording's is from {:?}",
                v3(&r["o"])
            ));
        }
        Ok(if b(&r["hit"]) { Some(f(&r["dist"])) } else { None })
    }
}

impl AgentPhysics for Recorded<'_> {
    fn raycast(
        &mut self,
        _: &mut World,
        _: EntityIdx,
        o: V3,
        d: V3,
        max: f32,
        mask: i32,
    ) -> R<Option<f32>> {
        self.take_phys("ray", o, &d, max, mask)
    }
    fn disc_cast(
        &mut self,
        _: &mut World,
        _: EntityIdx,
        o: V3,
        r: f32,
        max: f32,
        mask: i32,
    ) -> R<Option<f32>> {
        self.take_phys("disc", o, &[r], max, mask)
    }
}

impl AgentInputs for Recorded<'_> {
    fn is_done(&mut self, _: &mut World, _: EntityIdx, path: &str) -> R<bool> {
        let r = self.take_leaf("isdone")?;
        if r["path"].as_str() != Some(path) {
            return Err(Desync(format!("IsDone of {path}: the recording's is of {}", r["path"])));
        }
        Ok(b(&r["v"]))
    }
    fn has_current_turn(&mut self, _: &mut World) -> R<bool> {
        Ok(b(&self.tick["turn"]))
    }
    fn current_and_acting(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(b(&self.tick["cur"]) && b(&self.tick["acting"]))
    }
    fn turn_tick_movement(&mut self, _: &mut World, _: EntityIdx, dt: f32, force: bool) -> R<f32> {
        // The turn's own record of the call (`cd_move`, CooldownObs): its delta in and force.
        let r = match self.next_input() {
            Some(r) if r["ev"] == "cd_move" => r.clone(),
            Some(r) => {
                return Err(Desync(format!(
                    "wanted the turn's TickMovement (cd_move), the recording has {r}"
                )));
            }
            None => return Err(Desync("wanted the turn's TickMovement (cd_move)".into())),
        };
        let num = |k: &str| r[k].as_str().and_then(|x| x.parse::<f32>().ok());
        if num("dt").map(f32::to_bits) != Some(dt.to_bits()) || r["force"].as_bool() != Some(force)
        {
            return Err(Desync(format!(
                "TickMovement({dt}, force {force}): the turn's call was {r}"
            )));
        }
        let out = num("dt_out").ok_or_else(|| Desync("cd_move without dt_out".into()))?;
        if out.to_bits() != self.can_move_dt()?.to_bits() {
            self.errors.push("cd_move's dt_out isn't canmovetb's".into());
        }
        Ok(out)
    }
    fn can_move_end(&mut self, _: &mut World, _: EntityIdx, result: bool, dt: f32) -> R<()> {
        // `CanMoveInTurnBased`'s postfix: its answer and the delta it leaves.
        let r = self
            .recs
            .iter()
            .find(|r| r["ev"] == "ag_v" && r["f"] == "canmovetb")
            .ok_or_else(|| Desync("no canmovetb record".into()))?;
        if r["v"].as_bool() != Some(result) || f(&r["dt"]).to_bits() != dt.to_bits() {
            self.errors.push(format!("CanMoveInTurnBased({result}, {dt}): the recording's is {r}"));
        }
        Ok(())
    }
    fn cutscene_active(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        let held = b(&self.take_leaf("csctl")?["v"]);
        Ok(held && b(&self.take_leaf("csactive")?["v"]))
    }
    fn move_dont_interrupt(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        match self.tick.get("dontint") {
            Some(v) => Ok(b(v)),
            // Before `poc5-tb-102`: the commands' answer as the next call shows it.
            None => Ok(!self
                .peek_leaf()
                .is_some_and(|r| r["ev"] == "ag_call" && r["fn"] == "CompleteMovement")),
        }
    }
    fn call(&mut self, _: &mut World, _: EntityIdx, f: &'static str, index: Option<i32>) -> R<()> {
        match self.next_input() {
            Some(r)
                if r["ev"] == "ag_call"
                    && r["fn"] == f
                    && r["i"].as_i64().map(|x| x as i32) == index =>
            {
                Ok(())
            }
            Some(r) => {
                Err(Desync(format!("the agent calls {f}({index:?}), the recording has {r}")))
            }
            None => {
                Err(Desync(format!("the agent calls {f}({index:?}), the tick's records ended")))
            }
        }
    }
    fn call_end(&mut self, w: &mut World, u: EntityIdx, f: &'static str) -> R<()> {
        // The call's end record: the next one of the agent's (ends are skipped by the input pulls),
        // its state compared with the agent's now.
        let Some(k) = self.recs[self.at..]
            .iter()
            .position(|r| !(r["ev"] == "ag_v" && r["f"] == "canmovetb") && r["ev"] != "ag_pathdef")
        else {
            return Err(Desync(format!("{f} returned; the tick's records ended")));
        };
        let r = self.recs[self.at + k];
        if r["ev"] != "ag_call_end" || r["fn"] != f {
            return Err(Desync(format!("{f} returned; the recording has {r}")));
        }
        self.at += k + 1;
        let want = agent_from(&r["s"], &Paths::default());
        let ours = &w.unit(u).ok_or_else(|| Desync("no unit".into()))?.combat.agent;
        // Paths compare by key (the empty table gives keys only).
        for d in diff(ours, &want, r["s"].get("conn").is_some()) {
            self.errors.push(format!("at {f}'s end: {d}"));
        }
        Ok(())
    }
    fn tick_start(&mut self, _: &mut World, _: EntityIdx, dt: f32) -> R<()> {
        let r = self.take_ev("ag_tick")?;
        if bits(&f(&r["dt"])) != bits(&dt) {
            return Err(Desync(format!("TickMovement({dt}): the recording's is {}", r["dt"])));
        }
        Ok(())
    }
    fn tick_end(&mut self, _: &mut World, _: EntityIdx, ret: V3) -> R<()> {
        let r = self.take_ev("ag_tick_end")?;
        if bits(&v3(&r["ret"])) != bits(&ret) {
            self.errors
                .push(format!("TickMovement returned {ret:?}, the game {:?}", v3(&r["ret"])));
        }
        Ok(())
    }
    fn done_start(
        &mut self,
        _: &mut World,
        _: EntityIdx,
        path: &AgentPath,
        forced: bool,
        _: bool,
    ) -> R<()> {
        let path = path.key.as_str();
        let r = self.take_ev("ag_done")?;
        if r["path"].as_str() != Some(path) || r["forced"].as_bool() != Some(forced) {
            return Err(Desync(format!(
                "OnPathComplete({path}, {forced}): the recording's is {r}"
            )));
        }
        Ok(())
    }
    fn done_end(&mut self, _: &mut World, _: EntityIdx) -> R<()> {
        self.take_ev("ag_done_end").map(|_| ())
    }
    fn req_start(&mut self, _: &mut World, _: EntityIdx) -> R<()> {
        self.take_ev("ag_req").map(|_| ())
    }
    fn req_end(&mut self, _: &mut World, _: EntityIdx, _: Option<&PathRequest>) -> R<()> {
        self.take_ev("ag_req_end").map(|_| ())
    }
    fn group_memory(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        // Since `poc5-tb-103` the evaluation `ConnectToGroups` makes (the `Group` getter resolves the
        // group lazily); before, the state at the tick's start.
        if self.has_cb {
            return Ok(b(&self.take_leaf("gmem")?["v"]));
        }
        Ok(self.tick["s"].get("gmem").is_none_or(b))
    }
    fn obstacles_group(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(self.tick["s"].get("ogroup").is_none_or(b))
    }
    fn saddled(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(false)
    }
    fn dest_proxy(&mut self, _: &mut World, _: EntityIdx, _: &str) -> R<Proxy> {
        let r = self.take_leaf("proxy")?.clone();
        if !b(&r["unit"]) {
            return Ok(Proxy::NotUnit);
        }
        // `unitEntityData.View.Or(null)?.AgentASP` (recorded since `poc5-tb-103`).
        Ok(Proxy::Unit(match r.get("agent") {
            Some(a) if b(a) => r["aview"].as_str().map(str::to_owned),
            Some(_) => None,
            None => r["v"].as_str().map(str::to_owned),
        }))
    }
    fn nav_obstacles(&mut self, _: &mut World) -> R<bool> {
        Ok(b(&self.tick["s"]["navobst"]))
    }
    fn gamepad(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(b(&self.take_leaf("gamepad")?["v"]))
    }
    fn random_range(&mut self, _: &mut World, _: EntityIdx, min: f32, max: f32) -> R<f32> {
        let Some(r) = self.next_input().cloned() else {
            return Err(Desync("wanted a draw".into()));
        };
        if r["k"] != "draw"
            || r["m"] != "Range_ff"
            || bits(&f(&r["a"][0])) != bits(&min)
            || bits(&f(&r["a"][1])) != bits(&max)
        {
            return Err(Desync(format!(
                "wanted Random.Range({min}, {max}), the recording has {r}"
            )));
        }
        Ok(f(&r["v"]))
    }
    fn prone(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(b(&self.tick["s"]["prone"]))
    }
    fn repath_all(&mut self, w: &mut World) -> R<()> {
        // Only this unit's agent is in the checker's world.
        for i in 0..w.entities.len() {
            if let Some(x) = w.unit_mut(EntityIdx(i as u32)) {
                x.combat.agent.repath = true;
            }
        }
        Ok(())
    }
    fn accelerate_if_long(&mut self, _: &mut World, _: EntityIdx, _: &AgentPath) -> R<()> {
        // The commands' acceleration: no agent records (its effect reaches the agent as MaxSpeed).
        Ok(())
    }
    fn unreachable(&mut self, _: &mut World, _: EntityIdx, dest_view: &str) -> R<()> {
        // Recorded since `poc5-tb-105` (the call's leaf); before, unchecked.
        if self.tick.get("combat").is_none() {
            return Ok(());
        }
        let r = self.take_leaf("unreach")?;
        if r["v"].as_str() != Some(dest_view) {
            return Err(Desync(format!(
                "UnreachableUnits.Add({dest_view}): the recording's is {r}"
            )));
        }
        Ok(())
    }
    fn movement_started(&mut self, _: &mut World, _: EntityIdx, _: V3) -> R<()> {
        Ok(())
    }
    fn has_commands(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(true)
    }
    fn soft_collider_closest(&mut self, _: &mut World, _: EntityIdx, point: V3) -> R<Option<V3>> {
        if !b(&self.take_leaf("softcol")?["v"]) {
            return Ok(None);
        }
        let r = self.take_leaf("closest")?.clone();
        if bits(&v3(&r["p"])) != bits(&point) {
            self.errors
                .push(format!("ClosestPoint({point:?}): the recording's is of {:?}", v3(&r["p"])));
        }
        Ok(Some(v3(&r["v"])))
    }
    fn on_navmesh(&mut self, _: &mut World, _: EntityIdx, pos: V3) -> R<bool> {
        let r = self.take_leaf("navnode")?.clone();
        if bits(&v3(&r["p"])) != bits(&pos) {
            self.errors
                .push(format!("GetNearestNode({pos:?}): the recording's is of {:?}", v3(&r["p"])));
        }
        Ok(b(&r["v"]))
    }
    fn frames(&mut self, _: &mut World) -> R<(i32, i32)> {
        let n = |k: &str| {
            self.tick[k]
                .as_i64()
                .map(|x| x as i32)
                .ok_or_else(|| Desync(format!("ag_req without {k}")))
        };
        Ok((n("frame")?, n("tile")?))
    }
    fn init_move_direction(&mut self, _: &mut World, _: EntityIdx) -> R<V2> {
        self.init_dir.ok_or_else(|| Desync("InitMoveDirection: orientation isn't modelled".into()))
    }
    fn corpulence(&mut self, _: &mut World, _: EntityIdx) -> R<f32> {
        Ok(f(&self.take_leaf("corp")?["v"]))
    }
    fn size_in_cells(&mut self, _: &mut World, _: EntityIdx) -> R<i32> {
        Ok(self.take_leaf("cells")?["v"].as_i64().unwrap_or(-1) as i32)
    }
    fn grid_erosion(&mut self, _: &mut World) -> R<Option<(i32, i32)>> {
        Ok(self.erosion)
    }
    fn fudge_radius(&mut self, _: &mut World, _: EntityIdx, command: Option<u32>) -> R<f32> {
        if command.is_none() {
            return Ok(0.0);
        }
        Ok(f(&self.take_leaf("fudge")?["v"]))
    }
    fn speed_mps(&mut self, _: &mut World, _: EntityIdx) -> R<f32> {
        Ok(f(&self.take_leaf("speedmps")?["v"]))
    }
    fn move_command(&mut self, _: &mut World, _: EntityIdx) -> R<Option<(f32, bool)>> {
        let r = self.take_leaf("move")?.clone();
        Ok(b(&r["v"]).then(|| (f(&r["delay"]), b(&r["roaming"]))))
    }
    fn request_key(&mut self, _: &mut World, _: EntityIdx, _: V3) -> R<String> {
        self.key
            .clone()
            .ok_or_else(|| Desync("PathTo made a request; the recording has none".into()))
    }
    fn rider_in_combat(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(false)
    }
    fn anim_prevents(&mut self, _: &mut World, _: EntityIdx) -> R<Option<bool>> {
        if self.peek_input().is_some_and(|r| r["ev"] == "ag_v" && r["f"] == "animprev") {
            return Ok(Some(b(&self.take_leaf("animprev")?["v"])));
        }
        Ok(None)
    }
    fn commands_prevent(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(b(&self.take_leaf("cmdprev")?["v"]))
    }
    fn max_speed(&mut self, _: &mut World, _: EntityIdx) -> R<f32> {
        Ok(f(&self.take_leaf("maxspeed")?["v"]))
    }
    fn fly_height(&mut self, _: &mut World, _: EntityIdx) -> R<f32> {
        Ok(f(&self.take_leaf("fly")?["v"]))
    }
    fn in_combat(&mut self, _: &mut World, _: EntityIdx) -> R<bool> {
        Ok(b(&self.tick["combat"]))
    }
    fn movement_interrupted(&mut self, w: &mut World, u: EntityIdx, dest: V3) -> R<()> {
        self.callback(w, u, "OnMovementInterrupted", Some(dest), None)
    }
    fn movement_complete(&mut self, w: &mut World, u: EntityIdx) -> R<()> {
        self.callback(w, u, "OnMovementComplete", None, None)
    }
    fn waypoint_update(&mut self, w: &mut World, u: EntityIdx, index: i32) -> R<()> {
        self.callback(w, u, "OnMovementWaypointUpdate", None, Some(index))
    }
}

/// Is `r` a record the agent's code makes (or reads) inside a tick of `u`?
fn inside(r: &Rv, u: &str) -> bool {
    if r["k"] == "draw" {
        return true;
    }
    let ev = r["ev"].as_str().unwrap_or("");
    (ev == "ag_v"
        || ev == "ag_tick"
        || ev == "ag_tick_end"
        || ev == "ag_done"
        || ev == "ag_done_end"
        || ev == "ag_req"
        || ev == "ag_req_end"
        || ev == "ag_phys"
        || ev == "ag_call"
        || ev == "ag_call_end"
        || ev == "ag_pathdef"
        || ev == "cd_move"
        || ev == "ag_cb"
        || ev == "ag_cb_end")
        && (r["u"].as_str().is_none_or(|x| x == u) || ev == "ag_pathdef")
}

/// The captures checked: `WR_AGENT_CAPTURES` (comma-separated labels), by default `poc5-tb-100`
/// (it has a stopping tick where `Mathf.Sqrt`'s float argument shows) to `-105`.
fn captures() -> Vec<PathBuf> {
    let labels = std::env::var("WR_AGENT_CAPTURES").unwrap_or_else(|_| {
        "poc5-tb-100,poc5-tb-101,poc5-tb-102,poc5-tb-103,poc5-tb-104,poc5-tb-105".into()
    });
    let mut out = Vec::new();
    for label in labels.split(',') {
        let root =
            wr_paths::repo_root().join("fixtures/captures/oracle").join(label).join("fixtures");
        for fx in ["tb-nearest", "tb-focus"] {
            for run in ["record", "replay-1", "replay-2"] {
                out.push(root.join(fx).join(format!("{run}.trace.jsonl.gz")));
            }
        }
    }
    out
}

#[test]
#[ignore = "needs the poc5-tb-100 to -105 captures"]
fn every_recorded_tick_reruns_bit_for_bit() {
    let mut total = 0;
    let mut real_time = 0;
    let mut failures = Vec::new();
    for path in captures() {
        let trace = Trace::load(&path).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
        let recs = &trace.records;
        let has_cb = recs.iter().any(|r| r["ev"] == "ag_cb");
        let mut paths = Paths::default();
        let mut i = 0;
        while i < recs.len() {
            let r = &recs[i];
            paths.note(r);
            if r["k"] != "obs" || r["ev"] != "ag_tick" {
                i += 1;
                continue;
            }
            let u = r["u"].as_str().unwrap_or("").to_owned();
            if !b(&r["tb"]) {
                // Real time (the walk up before combat): not ported yet.
                real_time += 1;
                i += 1;
                continue;
            }
            let Some(end) = (i + 1..recs.len())
                .find(|&j| recs[j]["ev"] == "ag_tick_end" && recs[j]["u"] == u.as_str())
            else {
                failures.push(format!("{}: tick {i} of {u} has no end", path.display()));
                break;
            };
            let inner: Vec<&Rv> = recs[i..=end].iter().filter(|x| inside(x, &u)).collect();
            for x in &inner {
                paths.note(x);
            }
            let mut w = world();
            let unit_ix = unit(&mut w, &u, None, 10);
            w.combat.turn_based = b(&r["tb"]);
            w.unit_mut(unit_ix).unwrap().combat.agent = agent_from(&r["s"], &paths);
            let mut inp = Recorded {
                tick: r,
                recs: inner,
                at: 0,
                errors: Vec::new(),
                has_cb,
                key: None,
                init_dir: None,
                erosion: None,
            };
            let got = tick_movement(&mut w, unit_ix, f(&r["dt"]), &mut inp);
            let want_state = agent_from(&recs[end]["s"], &paths);
            let want_ret = v3(&recs[end]["ret"]);
            let mut why = inp.errors.clone();
            match got {
                Ok(ret) => {
                    if bits(&ret) != bits(&want_ret) {
                        why.push(format!("returned {ret:?}, the game {want_ret:?}"));
                    }
                    let has_conn = recs[end]["s"].get("conn").is_some();
                    why.extend(diff(&w.unit(unit_ix).unwrap().combat.agent, &want_state, has_conn));
                    let left: Vec<_> =
                        inp.recs[inp.at..].iter().filter(|x| !not_input(x)).collect();
                    if !left.is_empty() {
                        why.push(format!("records left unread: {}", left.len()));
                    }
                }
                Err(e) => why.push(format!("desync: {}", e.0)),
            }
            if !why.is_empty() {
                failures.push(format!(
                    "{} t={} {u}: {}",
                    path.file_name().unwrap().to_string_lossy(),
                    r["t"],
                    why.join("; ")
                ));
            }
            total += 1;
            i = end + 1;
        }
    }
    eprintln!(
        "{total} turn-based ticks, {} differ; {real_time} real-time ticks not checked",
        failures.len()
    );
    for x in failures.iter().take(30) {
        eprintln!("  {x}");
    }
    assert!(failures.is_empty());
}

/// The agent's state after a call, compared, and every input record consumed.
fn outcome(
    got: R<()>,
    inp: &Recorded<'_>,
    w: &World,
    unit_ix: EntityIdx,
    want: &Rv,
    paths: &Paths,
) -> Vec<String> {
    let mut why = inp.errors.clone();
    match got {
        Ok(()) => {
            let want_state = agent_from(want, paths);
            why.extend(diff(&w.unit(unit_ix).unwrap().combat.agent, &want_state, true));
            let left = inp.recs[inp.at..].iter().filter(|x| !not_input(x)).count();
            if left > 0 {
                why.push(format!("records left unread: {left}"));
            }
        }
        Err(e) => why.push(format!("desync: {}", e.0)),
    }
    why
}

#[test]
#[ignore = "needs the poc5-tb-105 capture"]
fn every_recorded_path_completion_reruns_bit_for_bit() {
    let mut total = 0;
    let mut real_time = 0;
    let mut failures = Vec::new();
    // `OnPathComplete`'s records need the `ag_cb` era (the callbacks, the gmem leaf).
    let labels = std::env::var("WR_AGENT_CAPTURES").unwrap_or_else(|_| "poc5-tb-105".into());
    for label in labels.split(',') {
        let root =
            wr_paths::repo_root().join("fixtures/captures/oracle").join(label).join("fixtures");
        for (fx, run) in ["tb-nearest", "tb-focus"]
            .iter()
            .flat_map(|fx| ["record", "replay-1", "replay-2"].map(|r| (*fx, r)))
        {
            let path = root.join(fx).join(format!("{run}.trace.jsonl.gz"));
            let trace = Trace::load(&path).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
            let recs = &trace.records;
            let mut paths = Paths::default();
            let mut tb = false;
            let mut i = 0;
            while i < recs.len() {
                let r = &recs[i];
                paths.note(r);
                if r["ev"] == "ag_tick" {
                    tb = b(&r["tb"]);
                }
                let ev = r["ev"].as_str().unwrap_or("");
                let call = ev == "ag_call"
                    && (r["fn"] == "FollowPrecomputedPath" || r["fn"] == "ForcePath");
                if !(ev == "ag_done" || call) {
                    i += 1;
                    continue;
                }
                let u = r["u"].as_str().unwrap_or("").to_owned();
                let (end_ev, f) = if call {
                    ("ag_call_end", r["fn"].as_str().unwrap_or(""))
                } else {
                    ("ag_done_end", "")
                };
                let Some(end) = (i + 1..recs.len()).find(|&j| {
                    recs[j]["ev"] == end_ev
                        && recs[j]["u"] == u.as_str()
                        && (!call || recs[j]["fn"] == f)
                }) else {
                    failures.push(format!("{run} {fx}: {ev} {i} of {u} has no end"));
                    break;
                };
                if !tb {
                    real_time += 1;
                    i = end + 1;
                    continue;
                }
                for x in &recs[i + 1..end] {
                    paths.note(x);
                }
                // The state before: a completion's own; a call's from its nested completion's
                // prefix is after the call's own writes, so take the latest state before it.
                let before = if call {
                    recs[..i]
                        .iter()
                        .rev()
                        .find(|x| {
                            x["u"] == u.as_str() && x["s"].is_object() && x["ev"] != "ag_tick"
                        })
                        .map_or(Rv::Null, |x| x["s"].clone())
                } else {
                    r["s"].clone()
                };
                // The unit's combat flag at the completion (`ag_done`'s, since `poc5-tb-105`; a call's
                // from its nested completion).
                let done = if call {
                    recs[i + 1..end].iter().find(|x| x["ev"] == "ag_done" && x["u"] == u.as_str())
                } else {
                    Some(r)
                };
                // The AI's `FindPath` callback clears `m_RequestedFindPath` unobserved; neither call
                // writes it and nothing runs between the call's start and its nested completion,
                // so that completion's state has its value at the call.
                let mut before = before.to_json();
                if call
                    && let Some(d) = done.and_then(|x| x.get("s"))
                    && before.is_object()
                {
                    before["find"] = d["find"].to_json();
                }
                if before.is_null() {
                    failures.push(format!("{run} {fx} t={}: no state of {u} before {ev}", r["t"]));
                    i = end + 1;
                    continue;
                }
                let inner: Vec<&Rv> = recs[i..=end].iter().filter(|x| inside(x, &u)).collect();
                let mut w = world();
                let unit_ix = unit(&mut w, &u, None, 10);
                w.combat.turn_based = true;
                w.unit_mut(unit_ix).unwrap().combat.agent = agent_from(&Rv::from(&before), &paths);
                let mut ctx = serde_json::json!({ "s": before });
                if let Some(c) = done.and_then(|x| x.get("combat")) {
                    ctx["combat"] = c.to_json();
                }
                let ctx = Rv::from(ctx);
                let mut inp = Recorded {
                    tick: &ctx,
                    recs: inner,
                    at: 0,
                    errors: Vec::new(),
                    has_cb: true,
                    key: None,
                    init_dir: None,
                    erosion: None,
                };
                let pts: Vec<V3> =
                    r["v"].as_array().map(|a| a.iter().map(v3).collect()).unwrap_or_default();
                let key = if call { r["rq"]["path"].as_str() } else { r["path"].as_str() }
                    .unwrap_or("")
                    .to_owned();
                let p = AgentPath { forced: key.starts_with("?ForcedPath"), key, points: pts };
                let got = if call {
                    // The nested completion's error flag (the call doesn't record one).
                    let err = recs[i + 1..end]
                        .iter()
                        .find(|x| x["ev"] == "ag_done" && x["u"] == u.as_str())
                        .is_some_and(|x| b(&x["err"]));
                    let arg = |k: usize| r["args"][k].as_str().unwrap_or("");
                    if f == "FollowPrecomputedPath" {
                        let max = arg(1).parse::<f32>().unwrap_or(f32::NAN);
                        follow_precomputed_path(
                            &mut w,
                            unit_ix,
                            &p,
                            err,
                            arg(0) == "1",
                            max,
                            &mut inp,
                        )
                    } else {
                        force_path(&mut w, unit_ix, &p, err, arg(0) == "1", &mut inp)
                    }
                } else {
                    on_path_complete(&mut w, unit_ix, &p, b(&r["err"]), b(&r["forced"]), &mut inp)
                };
                let why = outcome(got, &inp, &w, unit_ix, &recs[end]["s"], &paths);
                if !why.is_empty() {
                    failures.push(format!(
                        "{run} {fx} t={} {u} {ev} {f}: {}",
                        r["t"],
                        why.join("; ")
                    ));
                }
                total += 1;
                i = end + 1;
            }
        }
    }
    eprintln!(
        "{total} turn-based completions and precomputed or forced paths, {} differ; {real_time} in real time not checked",
        failures.len()
    );
    assert!(total > 0, "the captures exercise no turn-based completion");
    for x in failures.iter().take(30) {
        eprintln!("  {x}");
    }
    assert!(failures.is_empty());
}

/// A recorded request (`ag_req_end`'s `rq`) against ours.
fn request_diff(ours: &PathRequest, rq: &Rv) -> Vec<String> {
    let mut out = Vec::new();
    let mut cmp = |what: &str, a: String, b: String| {
        if a != b {
            out.push(format!("request {what}: {a} != {b}"));
        }
    };
    cmp("kind", "ABPathWithClearance".into(), rq["kind"].as_str().unwrap_or("").into());
    cmp("ending", "LosPathConditionForGrid".into(), rq["ec"].as_str().unwrap_or("").into());
    cmp("key", ours.key.clone(), rq["path"].as_str().unwrap_or("").into());
    cmp("start", bits(&ours.start), bits(&v3(&rq["start"])));
    cmp("end", bits(&ours.end), bits(&v3(&rq["end"])));
    cmp("clearance", bits(&ours.clearance), bits(&v3(&rq["clear"])));
    cmp("tags", ours.tags.to_string(), rq["tags"].to_string());
    cmp("graph mask", ours.graph_mask.to_string(), rq["gmask"].to_string());
    cmp("approach", bits(&ours.approach.unwrap_or(f32::NAN)), bits(&f(&rq["ar"])));
    cmp("los", ours.los.to_string(), rq["los"].to_string());
    cmp("ignore", bits(&ours.ignore_blocker), bits(&f(&rq["ibr"])));
    cmp("max distance", bits(&ours.max_distance), bits(&f(&rq["maxd"])));
    cmp("xz", ours.distance_xz.to_string(), rq["xz"].to_string());
    out
}

#[test]
#[ignore = "needs the poc5-tb-104 and -105 captures"]
fn every_recorded_path_to_reruns_bit_for_bit() {
    let nav = wr_nav::NavFile::read(&wr_nav::path_for("bad465e5404604346bdf71ef50b3299a"))
        .expect("the caves' nav file");
    let erosion = nav.world.grid.as_ref().map(|g| (g.erode_iterations, g.erosion_first_tag));
    let (mut total, mut requests, mut real_time) = (0, 0, 0);
    let mut failures = Vec::new();
    let labels =
        std::env::var("WR_AGENT_CAPTURES").unwrap_or_else(|_| "poc5-tb-104,poc5-tb-105".into());
    for label in labels.split(',') {
        let root =
            wr_paths::repo_root().join("fixtures/captures/oracle").join(label).join("fixtures");
        for (fx, run) in ["tb-nearest", "tb-focus"]
            .iter()
            .flat_map(|fx| ["record", "replay-1", "replay-2"].map(|r| (*fx, r)))
        {
            let path = root.join(fx).join(format!("{run}.trace.jsonl.gz"));
            let trace = Trace::load(&path).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
            let recs = &trace.records;
            let mut paths = Paths::default();
            let mut i = 0;
            while i < recs.len() {
                let r = &recs[i];
                paths.note(r);
                if !(r["ev"] == "ag_req" && r["fn"] == "pathto") {
                    i += 1;
                    continue;
                }
                let u = r["u"].as_str().unwrap_or("").to_owned();
                let Some(end) = (i + 1..recs.len())
                    .find(|&j| recs[j]["ev"] == "ag_req_end" && recs[j]["u"] == u.as_str())
                else {
                    failures.push(format!("{run} {fx}: PathTo {i} of {u} has no end"));
                    break;
                };
                let Some(tb) = r["tb"].as_bool() else {
                    failures.push(format!(
                        "{run} {fx}: PathTo without `tb` (a capture before poc5-tb-104)"
                    ));
                    break;
                };
                if !tb {
                    real_time += 1;
                    i = end + 1;
                    continue;
                }
                for x in &recs[i + 1..end] {
                    paths.note(x);
                }
                let e = &recs[end];
                let inner: Vec<&Rv> = recs[i..=end].iter().filter(|x| inside(x, &u)).collect();
                let mut w = world();
                let unit_ix = unit(&mut w, &u, None, 10);
                w.combat.turn_based = true;
                w.unit_mut(unit_ix).unwrap().combat.agent = agent_from(&r["s"], &paths);
                let made = e["s"]["req"] != r["s"]["req"] && !e["rq"].is_null();
                let mut inp = Recorded {
                    tick: r,
                    recs: inner,
                    at: 0,
                    errors: Vec::new(),
                    has_cb: true,
                    key: made.then(|| e["rq"]["path"].as_str().unwrap_or("").to_owned()),
                    init_dir: Some(v2(&e["s"]["mdir"])),
                    erosion,
                };
                let dest = b(&r["dhas"]).then(|| DestAgent {
                    view: r["dview"].as_str().map(str::to_owned),
                    attached: b(&r["dattached"]),
                });
                let command = r["c"].as_u64().map(|c| c as u32);
                let got = path_to(
                    &mut w,
                    unit_ix,
                    command,
                    v3(&r["p"]),
                    f(&r["r"]),
                    f(&r["mr"]),
                    b(&r["xz"]),
                    dest,
                    b(&r["dself"]),
                    &mut inp,
                );
                let mut why = Vec::new();
                match &got {
                    Ok(Some(req)) if made => why.extend(request_diff(req, &e["rq"])),
                    Ok(Some(_)) => why.push("PathTo made a request the game didn't".into()),
                    Ok(None) if made => why.push("the game made a request PathTo didn't".into()),
                    _ => {}
                }
                if made {
                    requests += 1;
                }
                why.extend(outcome(got.map(|_| ()), &inp, &w, unit_ix, &e["s"], &paths));
                if !why.is_empty() {
                    failures.push(format!("{run} {fx} t={} {u}: {}", r["t"], why.join("; ")));
                }
                total += 1;
                i = end + 1;
            }
        }
    }
    eprintln!(
        "{total} turn-based PathTo calls ({requests} requests), {} differ; {real_time} in real time not checked",
        failures.len()
    );
    assert!(requests > 0, "the captures exercise no turn-based PathTo request");
    for x in failures.iter().take(30) {
        eprintln!("  {x}");
    }
    assert!(failures.is_empty());
}
