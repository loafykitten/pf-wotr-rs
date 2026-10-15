//! The movement simulation: its pieces on hand-made cases, and every recorded step rerun
//! against WrathOracle's records bit for bit (`docs/poc5/ai-movement.md`).

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use super::*;
use crate::combat::oracle::Trace;
use crate::combat::turns::tests::{unit, world};

// ---- hand-made cases (the code's branches; the game's numbers come from the record test) ----

/// Fixed answers for a step, and the reads made, in order.
#[derive(Default)]
struct Stub {
    max_speed: f32,
    fly: f32,
    ray: Option<f32>,
    blocked: Option<i8>,
    obstacle: bool,
    reads: Vec<String>,
}

impl SimInputs for Stub {
    fn max_speed(&mut self, _: &mut World, _: EntityIdx) -> R<f32> {
        self.reads.push("maxspeed".into());
        Ok(self.max_speed)
    }
    fn fly_height(&mut self, _: &mut World, _: EntityIdx) -> R<f32> {
        self.reads.push("fly".into());
        Ok(self.fly)
    }
    fn raycast(
        &mut self,
        _: &mut World,
        _: EntityIdx,
        o: V3,
        _: V3,
        _: f32,
        _: i32,
    ) -> R<Option<f32>> {
        self.reads.push(format!("ray {o:?}"));
        Ok(self.ray)
    }
    fn disc_cast(
        &mut self,
        _: &mut World,
        _: EntityIdx,
        o: V3,
        _: f32,
        _: f32,
        _: i32,
    ) -> R<Option<f32>> {
        self.reads.push(format!("disc {o:?}"));
        Ok(None)
    }
    fn ally_blocked(&mut self, _: &mut World, _: EntityIdx, p: V3) -> R<Option<i8>> {
        self.reads.push(format!("near {p:?}"));
        Ok(self.blocked)
    }
    fn has_obstacle(&mut self, _: &mut World, _: EntityIdx, from: V3, _: V3, _: f32) -> R<bool> {
        self.reads.push(format!("los {from:?}"));
        Ok(self.obstacle)
    }
}

fn tb_world() -> (World, EntityIdx) {
    let mut w = world();
    let u = unit(&mut w, "u", None, 10);
    w.combat.turn_based = true;
    (w, u)
}

fn line(ap: V3, ar: f32) -> MovementSimulator {
    MovementSimulator::new([0.0; 3], vec![[0.0; 3], [10.0, 0.0, 0.0]], ap, ar, true, 0.0)
}

#[test]
fn the_constructor_starts_at_the_second_point() {
    let s = MovementSimulator::new([1.0; 3], vec![], [0.0; 3], 1.0, true, 0.0);
    assert_eq!((s.next_waypoint, s.next_waypoint_3d, s.waypoint_index), ([0.0; 2], [0.0; 3], 0));
    let s = MovementSimulator::new([1.0; 3], vec![[1.0, 2.0, 3.0]], [0.0; 3], 1.0, true, 0.0);
    assert_eq!(
        (s.next_waypoint, s.next_waypoint_3d, s.waypoint_index),
        ([1.0, 3.0], [1.0, 2.0, 3.0], 0)
    );
    let mut s = MovementSimulator::new(
        [1.0; 3],
        vec![[0.0; 3], [1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
        [0.0; 3],
        1.0,
        true,
        0.0,
    );
    assert_eq!((s.next_waypoint, s.waypoint_index), ([1.0, 3.0], 1));
    assert!(s.on_last_segment());
    assert!(s.advance_waypoint().unwrap());
    assert_eq!((s.next_waypoint_3d, s.waypoint_index), ([4.0, 5.0, 6.0], 2));
    assert!(!s.on_last_segment());
    assert!(!s.advance_waypoint().unwrap());
    assert_eq!(s.waypoint_index, 2);
}

#[test]
fn the_position_setter_measures_the_horizontal_move() {
    let mut s = line([0.0; 3], 1.0);
    s.set_position([3.0, 100.0, 4.0]);
    assert_eq!(s.last_move_length, 5.0);
    s.set_position([3.0, -7.0, 4.0]);
    assert_eq!(s.last_move_length, 0.0);
}

#[test]
fn segment_circle_intersection_cases() {
    // Entering a circle halfway: the smaller root.
    assert_eq!(
        segment_circle_intersection([0.0, 0.0], [2.0, 0.0], [1.0, 0.0], 0.5).unwrap(),
        Some(0.25)
    );
    // Tangent (`det == 0`).
    assert_eq!(
        segment_circle_intersection([0.0, 0.0], [2.0, 0.0], [1.0, 1.0], 1.0).unwrap(),
        Some(0.5)
    );
    // Missing it, a circle beyond the end, a zero segment.
    assert_eq!(segment_circle_intersection([0.0, 0.0], [2.0, 0.0], [1.0, 3.0], 1.0).unwrap(), None);
    assert_eq!(
        segment_circle_intersection([0.0, 0.0], [2.0, 0.0], [10.0, 0.0], 1.0).unwrap(),
        None
    );
    assert_eq!(segment_circle_intersection([5.0, 5.0], [5.0, 5.0], [5.0, 5.0], 1.0).unwrap(), None);
    // Starting inside: the larger root is the exit, past the end; the smaller negative.
    assert_eq!(segment_circle_intersection([1.0, 0.0], [1.5, 0.0], [1.0, 0.0], 1.0).unwrap(), None);
    // A segment so short that `Mathf.Epsilon`'s setting would decide: reported.
    assert!(segment_circle_intersection([0.0, 0.0], [1e-20, 0.0], [1.0, 0.0], 1.0).is_err());
    // ... unless either epsilon rejects it: the smallest denormal, a negative determinant.
    assert_eq!(
        segment_circle_intersection([0.0, 0.0], [4e-23, 0.0], [1.0, 0.0], 1.0).unwrap(),
        None
    );
    assert_eq!(
        segment_circle_intersection([0.0, 0.0], [1e-20, 0.0], [5.0, 5.0], 1.0).unwrap(),
        None
    );
    // A tangent at the start: `(0f - B)` with `B` +0 is +0, not -0.
    let t = segment_circle_intersection([0.0, 0.0], [1.0, 0.0], [0.0, 1.0], 1.0).unwrap();
    assert_eq!(t.map(f32::to_bits), Some(0));
}

#[test]
fn a_step_onto_the_last_point_ends_the_simulation() {
    let (mut w, u) = tb_world();
    let mut s = MovementSimulator::new(
        [0.0; 3],
        vec![[0.0; 3], [0.3, 0.0, 0.4]],
        [5.0, 0.0, 0.0],
        1.0,
        true,
        0.0,
    );
    let mut inp = Stub { max_speed: 5.0, ray: Some(2.5), ..Stub::default() };
    let go = simulate_tick_movement_for_turn_based(&mut w, u, &mut s, 0.1, 10.0, &mut inp).unwrap();
    assert!(!go);
    // Raised by the ray's hit (2 - 2.5 = -0.5) only after the disc test (0 - (0 - 0.5) > 0).
    assert_eq!(s.position(), [0.3, -0.5, 0.4]);
    assert_eq!(s.last_move_length, 0.5);
    assert_eq!(inp.reads, ["maxspeed", "fly", "ray [0.3, 2.0, 0.4]", "disc [0.3, 0.65, 0.4]"]);
}

#[test]
fn a_step_onto_an_inner_point_goes_on() {
    let (mut w, u) = tb_world();
    let mut s = MovementSimulator::new(
        [0.0; 3],
        vec![[0.0; 3], [0.3, 0.0, 0.4], [5.0, 0.0, 0.0]],
        [9.0, 0.0, 0.0],
        1.0,
        true,
        0.0,
    );
    let mut inp = Stub { max_speed: 5.0, blocked: Some(1), ..Stub::default() };
    assert!(simulate_tick_movement_for_turn_based(&mut w, u, &mut s, 0.1, 0.0, &mut inp).unwrap());
    assert_eq!(
        (s.position(), s.waypoint_index, s.reached_max_distance),
        ([0.3, 0.0, 0.4], 2, true)
    );
    assert!(simulate_tick_movement_for_turn_based(&mut w, u, &mut s, 0.0, 1.0, &mut inp).unwrap());
    // A zero delta: a step of nothing toward the next point.
    assert_eq!(s.position(), [0.3, 0.0, 0.4]);
}

#[test]
fn a_step_along_a_segment_moves_by_speed_and_stops_at_the_length() {
    let (mut w, u) = tb_world();
    // No approach radius: the node is looked up, never dereferenced.
    let mut s = line([10.0, 0.0, 0.0], 0.0);
    let mut inp = Stub { max_speed: 2.0, ..Stub::default() };
    assert!(
        simulate_tick_movement_for_turn_based(&mut w, u, &mut s, 0.1, 100.0, &mut inp).unwrap()
    );
    assert_eq!(s.position(), [0.2, 0.0, 0.0]);
    assert!(!s.reached_max_distance);
    assert_eq!(inp.reads.last().map(String::as_str), Some("near [0.2, 0.0, 0.0]"));
    // Capped by `maxMoveLength`.
    let mut s = line([10.0, 0.0, 0.0], 0.0);
    assert!(simulate_tick_movement_for_turn_based(&mut w, u, &mut s, 0.1, 0.05, &mut inp).unwrap());
    assert!((s.position()[0] - 0.05).abs() < 1e-7);
    assert!(s.reached_max_distance);
}

#[test]
fn a_step_into_the_approach_radius_stops_inside_it() {
    let (mut w, u) = tb_world();
    let mut s = line([1.1, 0.0, 0.0], 1.0);
    let mut inp = Stub { max_speed: 2.0, blocked: Some(0), ..Stub::default() };
    assert!(
        !simulate_tick_movement_for_turn_based(&mut w, u, &mut s, 0.1, 100.0, &mut inp).unwrap()
    );
    // Entering at t = 0.5 of the 0.2 step, plus 0.0001.
    assert!((s.position()[0] - 0.100_02).abs() < 1e-6, "{:?}", s.position());
    assert!(
        inp.reads
            .last()
            .is_some_and(|r| r.starts_with("los [0.1000") && r.ends_with(", 1.0, 0.0]"))
    );
    // Line of sight blocked: the whole step, and on.
    let mut s = line([1.1, 0.0, 0.0], 1.0);
    let mut inp = Stub { max_speed: 2.0, blocked: Some(0), obstacle: true, ..Stub::default() };
    assert!(
        simulate_tick_movement_for_turn_based(&mut w, u, &mut s, 0.1, 100.0, &mut inp).unwrap()
    );
    assert_eq!(s.position(), [0.2, 0.0, 0.0]);
    // A cell an ally blocks: no stop there.
    let mut s = line([1.1, 0.0, 0.0], 1.0);
    let mut inp = Stub { max_speed: 2.0, blocked: Some(1), ..Stub::default() };
    assert!(
        simulate_tick_movement_for_turn_based(&mut w, u, &mut s, 0.1, 100.0, &mut inp).unwrap()
    );
    assert_eq!(s.position(), [0.2, 0.0, 0.0]);
    // Already inside: the step's end, without the intersection.
    let mut s = line([0.5, 0.0, 0.0], 1.0);
    let mut inp = Stub { max_speed: 2.0, blocked: Some(0), ..Stub::default() };
    assert!(
        !simulate_tick_movement_for_turn_based(&mut w, u, &mut s, 0.1, 100.0, &mut inp).unwrap()
    );
    assert_eq!(s.position(), [0.2, 0.0, 0.0]);
}

#[test]
fn what_the_game_throws_on_or_isnt_ported_is_reported() {
    let (mut w, u) = tb_world();
    let mut s = line([1.1, 0.0, 0.0], 1.0);
    let mut inp = Stub { max_speed: 2.0, blocked: None, ..Stub::default() };
    assert!(
        simulate_tick_movement_for_turn_based(&mut w, u, &mut s, 0.1, 100.0, &mut inp).is_err()
    );
    let mut s = MovementSimulator::new([0.0; 3], vec![], [0.0; 3], 1.0, true, 0.0);
    assert!(
        simulate_tick_movement_for_turn_based(&mut w, u, &mut s, 0.1, 100.0, &mut inp).is_err()
    );
    w.combat.turn_based = false;
    let mut s = line([1.1, 0.0, 0.0], 1.0);
    assert!(
        simulate_tick_movement_for_turn_based(&mut w, u, &mut s, 0.1, 100.0, &mut inp).is_err()
    );
}

// ---- the recordings ----

fn f(v: &Value) -> Option<f32> {
    // The AI observer writes floats as round-trip strings, the agent observer as numbers.
    match v {
        Value::String(s) => s.parse::<f32>().ok(),
        _ => v.as_f64().map(|x| x as f32),
    }
}

fn v3(v: &Value) -> Option<V3> {
    Some([f(&v[0])?, f(&v[1])?, f(&v[2])?])
}

fn v2(v: &Value) -> Option<V2> {
    Some([f(&v[0])?, f(&v[1])?])
}

fn bits<const N: usize>(v: [f32; N]) -> [u32; N] {
    v.map(f32::to_bits)
}

/// A value the recording doesn't establish: the step isn't checked.
const UNSET: &str = "not established: ";

fn unset<T>(what: &str) -> R<T> {
    Err(Desync(format!("{UNSET}{what}")))
}

/// Where the step's inputs come from.
enum Source<'a> {
    /// `ag_sim` .. `ag_sim_end`: the step's own leaves (`ag_v` with ctx `sim`).
    Leaves,
    /// Captures before `ag_sim`: what other records establish.
    Older(&'a Older),
}

/// For captures without `ag_sim`: the inputs the step reads, as far as other records give them.
#[derive(Default, Clone)]
struct Older {
    /// `MaxSpeed`: `MaxSpeedOverride` (FindPath's state) or `CurrentSpeedMps` (FindPath's
    /// `speedmps` leaf) of the same `ScorePath` [C]; else the unit's last `TickMovement` read [I].
    max_speed: Option<(f32, bool)>,
    /// `FlyHeight`: the unit's last `MoveUnit` read [I].
    fly: Option<f32>,
    /// The `ablk` the AI reads right after the step (`GetNearest(sim.Position)`) and the step's
    /// recorded end: the step's own lookup of its end when it didn't stop short [C].
    after: Option<(Option<i64>, V3)>,
}

/// The inputs recorded inside one step, in order.
struct Recorded<'a> {
    /// The stepping agent's unit: every input record must be its.
    unit: String,
    recs: Vec<&'a Value>,
    at: usize,
    errors: Vec<String>,
    src: Source<'a>,
    /// Inputs taken from neighbouring records [I] (not from the step itself).
    inferred: bool,
}

impl Recorded<'_> {
    fn next(&mut self, what: &str) -> R<&Value> {
        let r = self.recs.get(self.at).copied();
        self.at += 1;
        r.ok_or_else(|| Desync(format!("wanted {what}, the step's records ended")))
    }

    fn leaf(&mut self, name: &str) -> R<Value> {
        let r = self.next(name)?.clone();
        if r["ev"] != "ag_v" || r["f"] != name || r["ctx"] != "sim" || r["u"] != self.unit.as_str()
        {
            return Err(Desync(format!("wanted the step's leaf {name}, the recording has {r}")));
        }
        Ok(r)
    }

    fn phys(&mut self, kind: &str, o: V3, shape: &[f32], max: f32, mask: i32) -> R<Option<f32>> {
        let r = self.next(kind)?.clone();
        if r["ev"] != "ag_phys"
            || r["fn"] != kind
            || r["ctx"] != "sim"
            || r["u"] != self.unit.as_str()
        {
            return Err(Desync(format!("wanted a {kind} query, the recording has {r}")));
        }
        let recorded: Vec<Option<f32>> =
            if kind == "ray" { (0..3).map(|i| f(&r["d"][i])).collect() } else { vec![f(&r["r"])] };
        if v3(&r["o"]).map(bits) != Some(bits(o))
            || !recorded
                .iter()
                .map(|x| x.map(f32::to_bits))
                .eq(shape.iter().map(|x| Some(x.to_bits())))
            || f(&r["max"]).map(f32::to_bits) != Some(max.to_bits())
            || r["mask"].as_i64() != Some(i64::from(mask))
        {
            self.errors.push(format!("{kind} query from {o:?}: the recording's is {r}"));
        }
        match r["hit"].as_bool() {
            Some(true) => Ok(Some(
                f(&r["dist"]).ok_or_else(|| Desync(format!("a hit without its distance: {r}")))?,
            )),
            Some(false) => Ok(None),
            None => Err(Desync(format!("a {kind} query without its result: {r}"))),
        }
    }
}

impl SimInputs for Recorded<'_> {
    fn max_speed(&mut self, _: &mut World, _: EntityIdx) -> R<f32> {
        match &self.src {
            Source::Leaves => {
                f(&self.leaf("maxspeed")?["v"]).ok_or_else(|| Desync("maxspeed".into()))
            }
            Source::Older(o) => match o.max_speed {
                Some((v, inferred)) => {
                    self.inferred |= inferred;
                    Ok(v)
                }
                None => unset("MaxSpeed"),
            },
        }
    }
    fn fly_height(&mut self, _: &mut World, _: EntityIdx) -> R<f32> {
        match &self.src {
            Source::Leaves => f(&self.leaf("fly")?["v"]).ok_or_else(|| Desync("fly".into())),
            Source::Older(o) => match o.fly {
                Some(v) => {
                    self.inferred = true;
                    Ok(v)
                }
                None => unset("FlyHeight"),
            },
        }
    }
    fn raycast(
        &mut self,
        _: &mut World,
        _: EntityIdx,
        o: V3,
        d: V3,
        max: f32,
        mask: i32,
    ) -> R<Option<f32>> {
        self.phys("ray", o, &d, max, mask)
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
        self.phys("disc", o, &[r], max, mask)
    }
    fn ally_blocked(&mut self, _: &mut World, _: EntityIdx, p: V3) -> R<Option<i8>> {
        let v = match &self.src {
            Source::Leaves => {
                let r = self.leaf("near")?;
                if v3(&r["p"]).map(bits) != Some(bits(p)) {
                    self.errors.push(format!("GetNearest({p:?}): the recording's is {r}"));
                }
                r["v"].as_i64()
            }
            Source::Older(o) => match o.after {
                Some((v, at)) if bits(at) == bits(p) => v,
                _ => return unset("the step's GetNearest (its end moved after the lookup)"),
            },
        };
        v.map(|x| i8::try_from(x).map_err(|_| Desync(format!("AllyBlocked {x}")))).transpose()
    }
    fn has_obstacle(
        &mut self,
        _: &mut World,
        _: EntityIdx,
        from: V3,
        to: V3,
        fudge: f32,
    ) -> R<bool> {
        match &self.src {
            Source::Leaves => {
                let r = self.leaf("los")?;
                if v3(&r["from"]).map(bits) != Some(bits(from))
                    || v3(&r["to"]).map(bits) != Some(bits(to))
                    || f(&r["r"]).map(f32::to_bits) != Some(fudge.to_bits())
                {
                    self.errors.push(format!(
                        "HasObstacle({from:?}, {to:?}, {fudge}): the recording's is {r}"
                    ));
                }
                r["v"].as_bool().ok_or_else(|| Desync(format!("los without a value: {r}")))
            }
            Source::Older(_) => unset("HasObstacle"),
        }
    }
}

/// The simulator from an `ag_sim`/`ag_sim_end` record's `sim` (the path's points given).
fn sim_from(s: &Value, path: Vec<V3>) -> Option<MovementSimulator> {
    Some(MovementSimulator {
        m_position: v3(&s["p"])?,
        path,
        next_waypoint: v2(&s["nwp"])?,
        next_waypoint_3d: v3(&s["nwp3"])?,
        waypoint_index: i32::try_from(s["wi"].as_i64()?).ok()?,
        last_move_length: f(&s["len"])?,
        reached_max_distance: s["rmd"].as_bool()?,
        approach_point: v3(&s["ap"])?,
        approach_radius: f(&s["ar"])?,
        check_los: s["los"].as_bool()?,
        ignore_blocker_radius: f(&s["ibr"])?,
    })
}

/// The fields that differ, by their bits.
fn sim_diff(ours: &MovementSimulator, want: &MovementSimulator) -> Vec<String> {
    let mut out = Vec::new();
    let mut cmp = |name: &str, a: Vec<u32>, b: Vec<u32>| {
        if a != b {
            out.push(format!("{name}: {a:x?} != {b:x?}"));
        }
    };
    cmp("p", bits(ours.m_position).to_vec(), bits(want.m_position).to_vec());
    cmp("nwp", bits(ours.next_waypoint).to_vec(), bits(want.next_waypoint).to_vec());
    cmp("nwp3", bits(ours.next_waypoint_3d).to_vec(), bits(want.next_waypoint_3d).to_vec());
    cmp("len", vec![ours.last_move_length.to_bits()], vec![want.last_move_length.to_bits()]);
    cmp("ap", bits(ours.approach_point).to_vec(), bits(want.approach_point).to_vec());
    cmp("ar", vec![ours.approach_radius.to_bits()], vec![want.approach_radius.to_bits()]);
    cmp(
        "ibr",
        vec![ours.ignore_blocker_radius.to_bits()],
        vec![want.ignore_blocker_radius.to_bits()],
    );
    if ours.waypoint_index != want.waypoint_index {
        out.push(format!("wi: {} != {}", ours.waypoint_index, want.waypoint_index));
    }
    if ours.reached_max_distance != want.reached_max_distance {
        out.push(format!("rmd: {} != {}", ours.reached_max_distance, want.reached_max_distance));
    }
    if ours.check_los != want.check_los {
        out.push("los".into());
    }
    out
}

/// The AI's `msim` leaf compared with the simulator after the step and its result.
fn msim_diff(r: &Value, ret: bool, s: &MovementSimulator) -> Vec<String> {
    let mut out = Vec::new();
    if r["v"].as_bool() != Some(ret) {
        out.push(format!("returned {ret}, the game {}", r["v"]));
    }
    if v3(&r["p"]).map(bits) != Some(bits(s.m_position)) {
        out.push(format!("position {:?}, the game {}", s.m_position, r["p"]));
    }
    if f(&r["len"]).map(f32::to_bits) != Some(s.last_move_length.to_bits()) {
        out.push(format!("LastMoveLength {}, the game {}", s.last_move_length, r["len"]));
    }
    if r["rmd"].as_bool() != Some(s.reached_max_distance) {
        out.push(format!("ReachedMaxDistance {}, the game {}", s.reached_max_distance, r["rmd"]));
    }
    if r["wi"].as_i64() != Some(i64::from(s.waypoint_index)) {
        out.push(format!("WaypointIndex {}, the game {}", s.waypoint_index, r["wi"]));
    }
    out
}

/// The captures checked: `WR_AISIM_CAPTURES` (comma-separated labels), by default `poc5-tb-100`
/// to `-105`: `-100`, `-101` and `-103` have `ScorePath` simulations.
fn captures() -> Vec<PathBuf> {
    let labels = std::env::var("WR_AISIM_CAPTURES").unwrap_or_else(|_| {
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

fn is(r: &Value, ev: &str) -> bool {
    r["k"] == "obs" && r["ev"] == ev
}

fn leaf<'a>(r: &'a Value, name: &str) -> Option<&'a Value> {
    (is(r, "uc_v") && r["f"] == name).then_some(r)
}

/// One step rerun: the simulator after it and its result, or why it wasn't checked.
fn rerun(sim: &mut MovementSimulator, max: f32, inp: &mut Recorded<'_>) -> R<bool> {
    let (mut w, u) = {
        let mut w = world();
        let u = unit(&mut w, "u", None, 10);
        w.combat.turn_based = true;
        (w, u)
    };
    // `ScorePath` and the path visualizer pass 0.1f.
    simulate_tick_movement_for_turn_based(&mut w, u, sim, 0.1, max, inp)
}

#[derive(Default)]
struct Tally {
    exact: usize,
    inferred: usize,
    unset: Vec<String>,
    failures: Vec<String>,
}

impl Tally {
    fn note(&mut self, at: String, got: R<bool>, inp: &Recorded<'_>, mut why: Vec<String>) {
        match got {
            // The rest isn't established; what was compared before it still counts.
            Err(e) if e.0.starts_with(UNSET) && inp.errors.is_empty() => {
                self.unset.push(format!("{at}: {}", e.0));
                return;
            }
            Err(e) => why.push(format!("desync: {}", e.0)),
            Ok(_) => {}
        }
        why.extend(inp.errors.iter().cloned());
        let left = inp.recs.len().saturating_sub(inp.at);
        if left > 0 {
            why.push(format!("records left unread: {left}"));
        }
        if why.is_empty() {
            if inp.inferred { self.inferred += 1 } else { self.exact += 1 }
        } else {
            self.failures.push(format!("{at}: {}", why.join("; ")));
        }
    }
}

/// Captures with `ag_sim`: each step from its recorded simulator, with its own leaves.
fn check_leaves(name: &str, recs: &[Value], t: &mut Tally) {
    // Every simulation record must belong to a step: an orphan (a missing `ag_sim`, a second end,
    // a leaf outside a step, an `msim` without its step) is reported.
    let mut used = vec![false; recs.len()];
    let mut i = 0;
    while i < recs.len() {
        let r = &recs[i];
        if !is(r, "ag_sim") {
            i += 1;
            continue;
        }
        let u = r["u"].as_str().unwrap_or("");
        let at = format!("{name} t={} {u}", r["t"]);
        used[i] = true;
        let Some(end) =
            (i + 1..recs.len()).find(|&j| is(&recs[j], "ag_sim_end") || is(&recs[j], "ag_sim"))
        else {
            t.failures.push(format!("{at}: a step without its end"));
            break;
        };
        if !is(&recs[end], "ag_sim_end") || recs[end]["u"] != u {
            t.failures.push(format!("{at}: a step whose next record is {}", recs[end]));
            i = end;
            continue;
        }
        used[end] = true;
        for (j, x) in recs.iter().enumerate().take(end).skip(i + 1) {
            if sim_record(x) {
                used[j] = true;
            }
        }
        let pts: Option<Vec<V3>> =
            r["sim"]["pts"].as_array().and_then(|a| a.iter().map(v3).collect());
        let (Some(mut sim), Some(want), Some(max)) = (
            pts.clone().and_then(|p| sim_from(&r["sim"], p)),
            pts.and_then(|p| sim_from(&recs[end]["sim"], p)),
            f(&r["max"]),
        ) else {
            t.failures.push(format!("{at}: a step record it can't read"));
            i = end + 1;
            continue;
        };
        if f(&r["dt"]).map(f32::to_bits) != Some(0.1_f32.to_bits()) {
            t.failures.push(format!(
                "{at}: a step of {} (ScorePath's and the visualizer's are 0.1)",
                r["dt"]
            ));
        }
        // The step's own reads: its leaves and physics queries (another unit's records in between
        // are a desync too: nothing else runs inside a step).
        let inner: Vec<&Value> = recs[i + 1..end]
            .iter()
            .filter(|x| x["k"] == "obs" && x["ev"].as_str().is_some_and(|e| e.starts_with("ag_")))
            .collect();
        let mut inp = Recorded {
            unit: u.to_owned(),
            recs: inner,
            at: 0,
            errors: Vec::new(),
            src: Source::Leaves,
            inferred: false,
        };
        let got = rerun(&mut sim, max, &mut inp);
        let mut why = Vec::new();
        if let Ok(ret) = got {
            if recs[end]["ret"].as_bool() != Some(ret) {
                why.push(format!("returned {ret}, the game {}", recs[end]["ret"]));
            }
            why.extend(sim_diff(&sim, &want));
            // The AI's own record of the step, when the AI made it: the next leaf.
            if let Some(k) =
                (end + 1..recs.len()).find(|&k| is(&recs[k], "uc_v") || is(&recs[k], "ag_sim"))
                && let Some(m) = leaf(&recs[k], "msim")
            {
                used[k] = true;
                why.extend(msim_diff(m, ret, &sim));
                if f(&m["max"]).map(f32::to_bits) != Some(max.to_bits()) {
                    why.push(format!("msim's max {}, the step's {max}", m["max"]));
                }
            }
        }
        t.note(at, got, &inp, why);
        i = end + 1;
    }
    for (j, x) in recs.iter().enumerate() {
        if !used[j] && (sim_record(x) || is(x, "ag_sim_end") || leaf(x, "msim").is_some()) {
            t.failures.push(format!("{name}: a simulation record outside a step: {x}"));
        }
    }
}

/// A record only a simulation step makes: its leaves and physics queries (`ctx: "sim"`).
fn sim_record(r: &Value) -> bool {
    (is(r, "ag_v") || is(r, "ag_phys")) && r["ctx"] == "sim"
}

/// `ScorePath`'s state as the AI's and the agent's records give it, for captures before `ag_sim`.
struct Scope {
    unit: String,
    path: Vec<V3>,
    dest: V3,
    radius: f32,
    /// `ActionRangeWithCorpulence`'s corpulences at `FindPath` (the constructor evaluates it
    /// again: the same value when they're the same).
    corps: Vec<u32>,
    max_speed: Option<(f32, bool)>,
    sim: Option<MovementSimulator>,
    /// The simulator was built with the unit's position from other records [I].
    inferred: bool,
    /// Why the simulator's state isn't known any more (an input not established, a step that
    /// differed): the rest of this `ScorePath`'s steps are counted, not checked.
    lost: Option<String>,
    /// The simulation stopped (a step returned false, reached its maximum distance or left less
    /// than 0.0001): `ScorePath` makes no further step.
    stopped: bool,
    /// The next step's `maxMoveLength`: `num5 - LastMoveLength` [C].
    next_max: Option<f32>,
    /// Where the current step's records start.
    from: usize,
}

/// `FindPath`'s request: unit, destination, radius, `MaxSpeedOverride`, the corpulences read before
/// it, `CurrentSpeedMps` (its `speedmps` leaf, since `poc5-tb-105`).
type Find = (String, V3, f32, Option<f32>, Vec<u32>, Option<f32>);

/// Captures without `ag_sim`: each `msim` rerun from the simulator `ScorePath` builds (chained
/// through its steps), with the step's physics (`ag_phys` ctx `sim`) and the rest from other
/// records where they establish it.
fn check_older(name: &str, recs: &[Value], t: &mut Tally) {
    // [I] each unit's last recorded position (`ag_ctl_end`, `ag_ground`), `MaxSpeed` and `FlyHeight`.
    let mut pos: HashMap<String, V3> = HashMap::new();
    let mut max_speed: HashMap<String, f32> = HashMap::new();
    let mut fly: HashMap<String, f32> = HashMap::new();
    // The corpulences read since the movement range (FindPath's argument: one evaluation of
    // `ActionRangeWithCorpulence`, two reads) and since the path leaf (`IsUnitCloseEnough`'s, the
    // magnitude test's and the constructor's: six reads before the first step).
    let mut corps: Vec<u32> = Vec::new();
    let mut corps_since_path: Vec<u32> = Vec::new();
    // FindPath's request: unit, destination, radius, MaxSpeedOverride, corpulences, CurrentSpeedMps.
    let mut find: Option<Find> = None;
    let mut in_find = false;
    let mut scope: Option<Scope> = None;
    for (i, r) in recs.iter().enumerate() {
        let u = r["u"].as_str().unwrap_or("").to_owned();
        if is(r, "ag_ctl_end") {
            if let Some(p) = v3(&r["pos"]) {
                pos.insert(u, p);
            }
        } else if is(r, "ag_ground") {
            if let Some(p) = v3(&r["after"]) {
                pos.insert(u, p);
            }
        } else if is(r, "ag_v") && r["f"] == "maxspeed" && r["ctx"].is_null() {
            if let Some(v) = f(&r["v"]) {
                max_speed.insert(u, v);
            }
        } else if is(r, "ag_v") && r["f"] == "fly" && r["ctx"].is_null() {
            if let Some(v) = f(&r["v"]) {
                fly.insert(u, v);
            }
        } else if let Some(c) = leaf(r, "corp") {
            let v = f(&c["v"]).map_or(u32::MAX, f32::to_bits);
            corps.push(v);
            corps_since_path.push(v);
        } else if leaf(r, "mrange").is_some() || leaf(r, "speed").is_some() {
            corps.clear();
        } else if is(r, "ag_req") && r["fn"] == "find" {
            let maxover = f(&r["s"]["maxover"]);
            find = match (v3(&r["p"]), f(&r["r"]), corps.len()) {
                (Some(p), Some(rad), 2) => Some((u, p, rad, maxover, corps.clone(), None)),
                _ => None,
            };
            in_find = true;
        } else if is(r, "ag_req_end") {
            in_find = false;
        } else if in_find && is(r, "ag_v") && r["f"] == "speedmps" {
            if let (Some(x), Some(v)) = (find.as_mut(), f(&r["v"])) {
                x.5 = Some(v);
            }
        } else if let Some(p) = leaf(r, "path") {
            // FixPathIfTooShort's path: ScorePath's simulation starts after it.
            let pts: Option<Vec<V3>> = p["v"].as_array().and_then(|a| a.iter().map(v3).collect());
            corps_since_path.clear();
            scope = match (find.clone(), pts) {
                (Some((unit, dest, radius, maxover, c, speed)), Some(path)) => Some(Scope {
                    // `MaxSpeedOverride ?? CurrentSpeedMps` [C]; else the unit's last read [I].
                    max_speed: maxover
                        .or(speed)
                        .map(|v| (v, false))
                        .or_else(|| max_speed.get(&unit).map(|v| (*v, true))),
                    unit,
                    path,
                    dest,
                    radius,
                    corps: c,
                    sim: None,
                    inferred: false,
                    lost: None,
                    stopped: false,
                    next_max: None,
                    from: i + 1,
                }),
                _ => None,
            };
        } else if is(r, "ai_sc") && r["fn"] == "ScorePath" {
            scope = None;
        } else if let Some(m) = leaf(r, "msim") {
            let at = format!("{name} t={} seq {}", r["t"], r["seq"]);
            let Some(s) = scope.as_mut() else {
                t.failures.push(format!("{at}: a simulation step outside a ScorePath with a path"));
                continue;
            };
            // Every simulation query in the step's interval (the reader checks whose it is).
            let inner: Vec<&Value> =
                recs[s.from..i].iter().filter(|x| is(x, "ag_phys") && x["ctx"] == "sim").collect();
            s.from = i + 1;
            if s.stopped {
                t.failures.push(format!("{at}: a step after the simulation stopped"));
                continue;
            }
            if let Some(why) = &s.lost {
                t.unset.push(format!("{at}: {UNSET}the simulator after {why}"));
                continue;
            }
            if s.sim.is_none() {
                // `new MovementSimulator(Unit.Position, CurrentPath, destination,
                // ActionRangeWithCorpulence, checkLOS: true, 0f)`: the radius FindPath was given
                // when the corpulences read again are the same [C]; the position [I].
                if corps_since_path.len() != 6 || corps_since_path[4..] != s.corps[..] {
                    t.unset.push(format!("{at}: {UNSET}the simulator's ActionRangeWithCorpulence"));
                    s.lost = Some("an unknown start".into());
                    continue;
                }
                let Some(&p) = pos.get(&s.unit) else {
                    t.unset.push(format!("{at}: {UNSET}the unit's position"));
                    s.lost = Some("an unknown start".into());
                    continue;
                };
                s.inferred = true;
                s.sim =
                    Some(MovementSimulator::new(p, s.path.clone(), s.dest, s.radius, true, 0.0));
            }
            // `ScorePath`'s own lookup right after the step: the AI's next leaf.
            let after = recs[i + 1..]
                .iter()
                .find(|x| is(x, "uc_v"))
                .and_then(|x| leaf(x, "ablk"))
                .zip(v3(&m["p"]))
                .map(|(a, p)| (a["v"].as_i64(), p));
            let older = Older { max_speed: s.max_speed, fly: fly.get(&s.unit).copied(), after };
            let Some(max) = f(&m["max"]) else {
                t.failures.push(format!("{at}: msim without its max"));
                continue;
            };
            if let Some(want) = s.next_max
                && want.to_bits() != max.to_bits()
            {
                t.failures.push(format!("{at}: msim's max {max}, the last step left {want}"));
            }
            let Some(mut sim) = s.sim.clone() else { continue };
            let mut inp = Recorded {
                unit: s.unit.clone(),
                recs: inner,
                at: 0,
                errors: Vec::new(),
                src: Source::Older(&older),
                inferred: s.inferred,
            };
            let got = rerun(&mut sim, max, &mut inp);
            let why = match &got {
                Ok(ret) => msim_diff(m, *ret, &sim),
                Err(_) => Vec::new(),
            };
            match &got {
                Ok(true) if why.is_empty() => {
                    // `num5 -= sim.LastMoveLength`; `ReachedMaxDistance || num5 < 0.0001f` ends it.
                    let left = store_f32(f64::from(max) - f64::from(sim.last_move_length));
                    s.stopped = sim.reached_max_distance || left < 0.0001;
                    s.next_max = Some(left);
                    s.sim = Some(sim);
                }
                Ok(false) if why.is_empty() => s.stopped = true,
                Err(e) if e.0.starts_with(UNSET) => s.lost = Some("a step not established".into()),
                _ => s.lost = Some("a step that differed".into()),
            }
            t.note(at, got, &inp, why);
        }
    }
}

#[test]
#[ignore = "needs the poc5-tb-100 to -105 captures"]
fn every_recorded_simulation_step_reruns_bit_for_bit() {
    let mut t = Tally::default();
    let mut runs = 0;
    for path in captures() {
        if !path.exists() {
            continue;
        }
        let trace = Trace::load(&path).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
        let name = format!(
            "{}/{}/{}",
            path.ancestors()
                .nth(3)
                .and_then(|p| p.file_name())
                .unwrap_or_default()
                .to_string_lossy(),
            path.parent().and_then(|p| p.file_name()).unwrap_or_default().to_string_lossy(),
            path.file_name().unwrap_or_default().to_string_lossy()
        );
        runs += 1;
        // The requested records' format (any of its records: a missing prefix is then reported).
        if trace
            .records
            .iter()
            .any(|r| is(r, "ag_sim") || is(r, "ag_sim_end") || sim_record(r) && is(r, "ag_v"))
        {
            check_leaves(&name, &trace.records, &mut t);
        } else {
            check_older(&name, &trace.records, &mut t);
        }
    }
    eprintln!(
        "{runs} runs: {} steps exact from their own records, {} with inputs from neighbouring records, {} not established, {} differ",
        t.exact,
        t.inferred,
        t.unset.len(),
        t.failures.len()
    );
    for x in t.unset.iter().take(10) {
        eprintln!("  unset {x}");
    }
    for x in t.failures.iter().take(30) {
        eprintln!("  {x}");
    }
    assert!(runs > 0, "no capture found");
    assert!(t.failures.is_empty());
}

/// The records the observer change asks for (`ag_sim`, its `ctx: "sim"` leaves, `ag_sim_end`),
/// built from `poc5-tb-103/tb-focus`'s step at t=1754 (its `MaxSpeed` and `FlyHeight` aren't
/// recorded there: 4 and 0 stand in; any speed above 1.77 takes the same branch).
#[allow(clippy::excessive_precision)] // the recorded values as written
fn requested_step() -> Vec<Value> {
    use serde_json::json;
    let start = [61.685_562_1_f32, 35.086_124_4, 157.585_739];
    let wp = [61.750_003_8_f32, 35.203_003, 157.75];
    let ap = [61.846_942_9_f32, 35.159_595_5, 155.611_313];
    let end = [61.750_003_8_f32, 35.107_784_3, 157.75];
    let sim = |p: [f32; 3], len: f32, pts: bool| {
        let mut s = json!({
            "p": p, "len": len, "rmd": false, "wi": 1, "nwp": [wp[0], wp[2]], "nwp3": wp,
            "ap": ap, "ar": 2.0096_f32, "los": true, "ibr": 0.0_f32,
        });
        if pts {
            s["pts"] = json!([start, wp]);
        }
        s
    };
    let leaf = |f: &str, v: f32| json!({"k": "obs", "t": 1754, "ev": "ag_v", "f": f, "u": "51FC", "ctx": "sim", "v": v});
    vec![
        json!({"k": "obs", "t": 1754, "ev": "ag_sim", "u": "51FC", "dt": 0.1_f32,
            "max": 9.956_802_f32, "sim": sim(start, 0.0, true)}),
        leaf("maxspeed", 4.0),
        leaf("fly", 0.0),
        json!({"k": "obs", "t": 1754, "ev": "ag_phys", "fn": "ray", "ctx": "sim", "u": "51FC",
            "o": [wp[0], 37.203_003_f32, wp[2]], "d": [0.0, -1.0, 0.0], "max": 100.0,
            "mask": 2_099_457, "hit": true, "dist": 2.115_902_f32}),
        json!({"k": "obs", "t": 1754, "ev": "ag_phys", "fn": "disc", "ctx": "sim", "u": "51FC",
            "o": [wp[0], 35.853_004_5_f32, wp[2]], "r": 0.4_f32, "max": 100.0,
            "mask": 2_099_457, "hit": true, "dist": 0.745_220_2_f32}),
        json!({"k": "obs", "t": 1754, "ev": "ag_sim_end", "u": "51FC", "ret": false,
            "sim": sim(end, 0.176_449_314, false)}),
        json!({"k": "obs", "t": 1754, "ev": "uc_v", "f": "msim", "max": "9.956802", "v": false,
            "p": end, "len": "0.176449314", "rmd": false, "wi": 1}),
    ]
}

#[test]
fn the_checker_reads_the_requested_records() {
    let recs = requested_step();
    let mut t = Tally::default();
    check_leaves("made", &recs, &mut t);
    assert_eq!((t.exact, t.inferred, t.unset.len()), (1, 0, 0), "{:?}", t.failures);
    // Damaged: each is reported.
    type Damage = Box<dyn Fn(&mut Vec<Value>)>;
    let damage: Vec<Damage> = vec![
        Box::new(|r| r[3]["o"][1] = Value::from(37.0)),
        Box::new(|r| r[4]["dist"] = Value::from(0.7)),
        Box::new(|r| {
            r.remove(2);
        }),
        Box::new(|r| r[1]["ctx"] = Value::Null),
        Box::new(|r| r[5]["ret"] = Value::from(true)),
        Box::new(|r| r[5]["sim"]["wi"] = Value::from(0)),
        Box::new(|r| r[6]["len"] = Value::from("0.17")),
        Box::new(|r| r[6]["max"] = Value::from("9")),
        Box::new(|r| r[0]["dt"] = Value::from(0.2)),
        Box::new(|r| {
            let x = r[2].clone();
            r.insert(5, x);
        }),
        Box::new(|r| {
            r.remove(5);
        }),
        // No prefix; a second end; another unit's leaf or query; a query without its result; a
        // leaf after the step.
        Box::new(|r| {
            r.remove(0);
        }),
        Box::new(|r| {
            let x = r[5].clone();
            r.insert(6, x);
        }),
        Box::new(|r| r[1]["u"] = Value::from("5198")),
        Box::new(|r| r[3]["u"] = Value::from("5198")),
        Box::new(|r| {
            r[3].as_object_mut().map(|o| o.remove("hit"));
        }),
        Box::new(|r| {
            let x = r[1].clone();
            r.push(x);
        }),
    ];
    for (i, d) in damage.iter().enumerate() {
        let mut recs = requested_step();
        d(&mut recs);
        let mut t = Tally::default();
        check_leaves("made", &recs, &mut t);
        assert!(!t.failures.is_empty(), "damage {i} not reported");
    }
}

/// `poc5-tb-103/tb-focus`'s `ScorePath` at t=1754 as the older captures record it (`MaxSpeed`,
/// `FlyHeight` and the position from earlier records, here made up: 4, 0 and the path's start).
#[allow(clippy::excessive_precision)] // the recorded values as written
fn older_score_path() -> Vec<Value> {
    use serde_json::json;
    let start = [61.685_562_1_f32, 35.086_124_4, 157.585_739];
    let wp = [61.750_003_8_f32, 35.203_003, 157.75];
    let end = [61.750_003_8_f32, 35.107_784_3, 157.75];
    let uv = |f: &str, extra: Value| {
        let mut r = json!({"k": "obs", "t": 1754, "ev": "uc_v", "f": f});
        for (k, v) in extra.as_object().into_iter().flatten() {
            r[k] = v.clone();
        }
        r
    };
    let corp = |u: &str, v: &str| uv("corp", json!({"u": u, "v": v}));
    let me = || corp("51FC", "0.9");
    let them = || corp("35ff", "0.5");
    vec![
        json!({"k": "obs", "t": 1700, "ev": "ag_v", "f": "maxspeed", "u": "51FC", "v": 4.0}),
        json!({"k": "obs", "t": 1700, "ev": "ag_v", "f": "fly", "u": "51FC", "v": 0.0}),
        json!({"k": "obs", "t": 1753, "ev": "ag_ctl_end", "u": "51FC", "pos": start}),
        uv("mrange", json!({"u": "51FC", "v": "9.956802"})),
        me(),
        them(),
        json!({"k": "obs", "t": 1754, "ev": "ag_req", "fn": "find", "u": "51FC",
            "p": [61.846_942_9_f32, 35.159_595_5, 155.611_313], "r": 2.0096_f32,
            "s": {"maxover": null}}),
        json!({"k": "obs", "t": 1754, "ev": "ag_req_end", "u": "51FC", "fn": "find"}),
        uv("path", json!({"fake": false, "err": false, "st": 2, "v": [start, wp]})),
        me(),
        them(),
        uv("pclose", json!({"r": "2.0096", "v": false})),
        uv("mag", json!({"v": "2.14132285"})),
        me(),
        them(),
        uv("plen", json!({"v": "0.211648166"})),
        me(),
        them(),
        json!({"k": "obs", "t": 1754, "ev": "ag_phys", "fn": "ray", "ctx": "sim", "u": "51FC",
            "o": [wp[0], 37.203_003_f32, wp[2]], "d": [0.0, -1.0, 0.0], "max": 100.0,
            "mask": 2_099_457, "hit": true, "dist": 2.115_902_f32}),
        json!({"k": "obs", "t": 1754, "ev": "ag_phys", "fn": "disc", "ctx": "sim", "u": "51FC",
            "o": [wp[0], 35.853_004_5_f32, wp[2]], "r": 0.4_f32, "max": 100.0,
            "mask": 2_099_457, "hit": true, "dist": 0.745_220_2_f32}),
        uv(
            "msim",
            json!({"max": "9.956802", "v": false, "p": end, "len": "0.176449314",
            "rmd": false, "wi": 1}),
        ),
        uv("ablk", json!({"v": 0})),
        uv("ablk", json!({"v": 0})),
        json!({"k": "obs", "t": 1754, "ev": "ai_sc", "fn": "ScorePath"}),
    ]
}

#[test]
fn the_checker_reads_older_captures() {
    let mut t = Tally::default();
    check_older("made", &older_score_path(), &mut t);
    assert_eq!((t.exact, t.inferred, t.unset.len()), (0, 1, 0), "{:?} {:?}", t.failures, t.unset);
    // Not established: no constructor's corpulences, no earlier speed.
    for d in [
        |r: &mut Vec<Value>| {
            r.drain(16..18);
        },
        |r: &mut Vec<Value>| {
            r.remove(0);
        },
    ] {
        let mut recs = older_score_path();
        d(&mut recs);
        let mut t = Tally::default();
        check_older("made", &recs, &mut t);
        assert_eq!((t.inferred, t.unset.len(), t.failures.len()), (0, 1, 0));
    }
    // Reported: another unit's query, a step after the end, a different position.
    type Damage = Box<dyn Fn(&mut Vec<Value>)>;
    let damage: Vec<Damage> = vec![
        Box::new(|r| {
            let mut x = r[18].clone();
            x["u"] = Value::from("5198");
            r.insert(18, x);
        }),
        Box::new(|r| {
            let x = r[20].clone();
            r.insert(21, x);
        }),
        Box::new(|r| r[20]["p"][1] = Value::from(35.0)),
    ];
    for (i, d) in damage.iter().enumerate() {
        let mut recs = older_score_path();
        d(&mut recs);
        let mut t = Tally::default();
        check_older("made", &recs, &mut t);
        assert!(!t.failures.is_empty(), "damage {i} not reported");
    }
}
