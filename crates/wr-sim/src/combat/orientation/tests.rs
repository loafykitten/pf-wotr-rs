//! Orientation against WrathOracle's `agent` records (`ag_ctl` .. `ag_ctl_end`, `ag_rot`, the
//! `canrotate` leaves, `ag_tick_end`'s `MoveDirection`, `ag_req`/`ag_req_end`), and the managed
//! `Mathf` members on their own.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::combat::oracle::Rv;

use super::*;
use crate::combat::oracle::Trace;
use crate::csharp::arith::store_f32;

fn f(v: &Rv) -> f32 {
    v.as_f64().map_or(f32::NAN, |x| x as f32)
}

fn v3(v: &Rv) -> V3 {
    [f(&v[0]), f(&v[1]), f(&v[2])]
}

fn v2(v: &Rv) -> V2 {
    [f(&v[0]), f(&v[1])]
}

fn bits<const N: usize>(a: [f32; N]) -> [u32; N] {
    a.map(f32::to_bits)
}

#[test]
fn repeat_wraps_into_the_length() {
    assert_eq!(repeat(370.0, 360.0), 10.0);
    assert_eq!(repeat(-10.0, 360.0), 350.0);
    assert_eq!(repeat(720.0, 360.0).to_bits(), 0);
    // `-0 - (-0 * 360)` is +0.
    assert_eq!(repeat(-0.0, 360.0).to_bits(), 0);
    // `t - Floor(t / length) * length` in double, rounded once as Clamp's argument.
    assert_eq!(repeat(1.0, 0.3), store_f32(1.0 - 3.0 * f64::from(0.3f32)));
}

#[test]
fn delta_angle_and_move_towards_angle() {
    assert_eq!(delta_angle(10.0, 350.0), -20.0);
    assert_eq!(delta_angle(350.0, 10.0), 20.0);
    assert_eq!(delta_angle(-82.94409, -82.94409).to_bits(), 0);
    // Within reach: the target itself, not normalised.
    assert_eq!(move_towards_angle(10.0, 725.0, 6.0), 725.0);
    // Out of reach: a step of maxDelta toward the nearer way round, from the current angle.
    assert_eq!(move_towards_angle(10.0, 350.0, 6.0), 4.0);
    assert_eq!(move_towards_angle(-170.0, 170.0, 6.0), -176.0);
    // `maxDelta` 0 and an equal target: `MoveTowards(current, current + 0, 0)` gives the target.
    assert_eq!(move_towards_angle(-0.0, -0.0, 0.0).to_bits(), (0.0f32).to_bits());
    let mut o = Orientation { value: 10.0, desired: 350.0 };
    o.update_slow_rotation(6.0, false);
    assert_eq!(o.value, 10.0);
    o.update_slow_rotation(6.0, true);
    assert_eq!(o.value, 4.0);
    o.set(90.0);
    assert_eq!((o.value, o.desired), (90.0, 90.0));
}

#[test]
fn look_angle_and_orientation_direction() {
    // Unity's conventions: +Z is 0°, +X is 90°, angles in [0, 360).
    assert_eq!(look_angle([0.0, 0.0, 1.0]), 0.0);
    assert_eq!(look_angle([1.0, 0.0, 0.0]), 90.0);
    assert!((look_angle([-1.0, 0.0, 0.0]) - 270.0).abs() < 1e-4);
    assert!((look_angle([0.0, 0.0, -1.0]) - 180.0).abs() < 1e-4);
    let d = init_move_direction(90.0);
    assert!((d[0] - 1.0).abs() < 1e-6 && d[1].abs() < 1e-6, "{d:?}");
    let mut o = Orientation::default();
    let none: Result<_, ()> = tick_unit_write(&mut o, true, || Ok(true), || Ok(true), [0.05, 0.05]);
    assert_eq!(none, Ok(None), "sqrMagnitude 0.005 is too short");
    let mut gates = 0;
    let r: Result<_, ()> = tick_unit_write(
        &mut o,
        false,
        || {
            gates += 1;
            Ok(true)
        },
        || Ok(true),
        [1.0, 0.0],
    );
    assert_eq!((r, gates), (Ok(None), 0), "CanRotate isn't read without motion");
    let r: Result<_, ()> = tick_unit_write(&mut o, true, || Ok(true), || Ok(true), [1.0, 0.0]);
    assert_eq!(r, Ok(Some(90.0)));
    assert_eq!((o.value, o.desired), (90.0, 90.0));
}

#[test]
fn get_look_at_angle_and_the_writes() {
    let pos = [10.0, 2.0, -3.0];
    // The point's height is the unit's: only x and z count.
    assert_eq!(look_at_angle(pos, 7.0, [11.0, 50.0, -3.0]), 90.0);
    assert_eq!(look_at_angle(pos, 7.0, [10.0, -9.0, -2.0]), 0.0);
    // The unit's own position (within `Vector3 ==`, 1e-5): its desired orientation, unchanged.
    assert_eq!(look_at_angle(pos, 7.0, [10.0, 40.0, -3.0]), 7.0);
    assert_eq!(look_at_angle(pos, -725.5, [10.000_001, 2.0, -3.0]), -725.5);
    // The difference is rounded per component before `LookRotation`.
    let p = [10.3, 0.0, -2.9];
    let diff = [store_f32(f64::from(10.3f32) - 10.0), 0.0, store_f32(f64::from(-2.9f32) + 3.0)];
    assert_eq!(look_at_angle(pos, 0.0, p).to_bits(), look_angle(diff).to_bits());

    let o = Orientation { value: 30.0, desired: 40.0 };
    let east = LookArg::Point([12.0, 0.0, -3.0]);
    // `LookAt` turns the desired orientation only; `ForceLookAt` both.
    assert_eq!(
        o.write(LookFn::LookAt, east, pos),
        Some(Orientation { value: 30.0, desired: 90.0 })
    );
    assert_eq!(
        o.write(LookFn::ForceLookAt, east, pos),
        Some(Orientation { value: 90.0, desired: 90.0 })
    );
    // At its own position: `LookAt` keeps the desired orientation, `ForceLookAt` sets the
    // orientation to it.
    let here = LookArg::Point(pos);
    assert_eq!(o.write(LookFn::LookAt, here, pos), Some(o));
    assert_eq!(
        o.write(LookFn::ForceLookAt, here, pos),
        Some(Orientation { value: 40.0, desired: 40.0 })
    );
    // The setters, unnormalised.
    let v = LookArg::Value(-400.0);
    assert_eq!(
        o.write(LookFn::Orientation, v, pos),
        Some(Orientation { value: -400.0, desired: -400.0 })
    );
    assert_eq!(
        o.write(LookFn::Desired, v, pos),
        Some(Orientation { value: 30.0, desired: -400.0 })
    );
    // An argument of the other kind.
    assert_eq!(o.write(LookFn::Desired, east, pos), None);
    assert_eq!(o.write(LookFn::LookAt, v, pos), None);
    for f in [LookFn::LookAt, LookFn::ForceLookAt, LookFn::Orientation, LookFn::Desired] {
        assert_eq!(LookFn::parse(f.name()), Some(f));
    }
    assert_eq!(LookFn::parse("Desired"), None);
}

/// The captures checked: `WR_ORIENT_CAPTURES` (comma-separated labels), by default
/// `poc5-tb-100` to `-103` (every one with the `agent` observer's `ag_ctl` and `ag_rot`).
fn captures() -> Vec<PathBuf> {
    let labels = std::env::var("WR_ORIENT_CAPTURES")
        .unwrap_or_else(|_| "poc5-tb-100,poc5-tb-101,poc5-tb-102,poc5-tb-103".into());
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

/// `GameModeType.TacticalCombat` as the `tick` record's `mode` writes it.
const TACTICAL_COMBAT: i64 = 14;

#[derive(Default)]
struct Counts {
    ctl: usize,
    moved: usize,
    written: usize,
    turned: usize,
    max_checked: usize,
    stayed: usize,
    changed_outside: usize,
    init: usize,
    init_unknown: usize,
    init_ambiguous: usize,
}

/// A unit's `m_AngularSpeed` from the agent's recorded states (a serialized field nothing
/// writes: one value per unit).
fn angular_speeds(recs: &[Rv]) -> Result<HashMap<String, f32>, String> {
    let mut out: HashMap<String, f32> = HashMap::new();
    for r in recs {
        let (Some(u), Some(ang)) = (r["u"].as_str(), r["s"].get("ang")) else { continue };
        let ang = f(ang);
        if let Some(old) = out.insert(u.to_owned(), ang)
            && old.to_bits() != ang.to_bits()
        {
            return Err(format!("{u}: m_AngularSpeed {old} then {ang}"));
        }
    }
    Ok(out)
}

/// What is known at a `TickUnit` from the records before it.
struct Known {
    /// `TimeController.DeltaTime` (the frame's `time` record).
    dt: Option<f32>,
    /// `TacticalCombatHelper.IsActive` (the `tick` record's game mode).
    tactical: bool,
    /// The unit's `m_AngularSpeed`.
    ang: Option<f32>,
    /// The unit's `m_Orientation` at its last `ag_ctl_end`.
    last: Option<f32>,
}

/// One `UnitMoveController.TickUnit` (`recs[i]` its `ag_ctl`, `recs[end]` its `ag_ctl_end`):
/// the orientation computed from the recorded inputs, compared with the records.
fn check_ctl(recs: &[Rv], i: usize, end: usize, k: &Known, n: &mut Counts) -> Vec<String> {
    let Known { dt, tactical, ang, last } = *k;
    let ctl = &recs[i];
    let u = ctl["u"].as_str().unwrap_or("");
    let mut why = Vec::new();
    // The records this TickUnit makes for its unit: its leaves, the agent's tick and its end
    // (the agent's own records in between belong to the movement check), `ag_rot`.
    let mut leaves = Vec::new();
    let mut tick = None;
    let mut tick_end = None;
    let mut rot = None;
    let mut j = i + 1;
    while j < end {
        let r = &recs[j];
        if r["u"].as_str() == Some(u) {
            match r["ev"].as_str().unwrap_or("") {
                "ag_v" if r["f"] == "canmove" || r["f"] == "canrotate" => {
                    leaves.push((rot.is_some(), r["f"].as_str().unwrap_or(""), r["v"].as_bool()));
                }
                "ag_tick" if tick.is_none() => tick = Some(r),
                "ag_tick_end" if tick.is_some() && tick_end.is_none() => tick_end = Some(r),
                "ag_tick" | "ag_tick_end" => {
                    why.push(format!("a second {} in one TickUnit", r["ev"]))
                }
                "ag_rot" if rot.is_none() => rot = Some(r),
                "ag_rot" => why.push("a second ag_rot".into()),
                _ => {}
            }
        }
        j += 1;
    }
    let Some(rot) = rot else {
        why.push("no ag_rot".into());
        return why;
    };
    if rot["ctl"] != true {
        why.push("ag_rot not from this TickUnit".into());
    }
    let mut leaves = leaves.into_iter();
    let mut take = |after_rot: bool, f: &str| -> Result<bool, String> {
        match leaves.next() {
            Some((a, g, Some(v))) if a == after_rot && g == f => Ok(v),
            other => Err(format!("expected leaf {f} (after ag_rot: {after_rot}), got {other:?}")),
        }
    };
    let mut o = Orientation { value: f32::NAN, desired: f32::NAN };
    // `flag = IsReallyMoving && CanMove`, then `IsReallyMoving && !CanMove && in turn-based`:
    // CanMove is read twice when the agent is really moving (IsReallyMoving is the agent's;
    // the recording shows it through these reads and the tick that follows).
    let mut written = None;
    match (tick, tick_end) {
        (Some(t), Some(te)) => {
            n.moved += 1;
            for _ in 0..2 {
                match take(false, "canmove") {
                    Ok(true) => {}
                    Ok(false) => why.push("CanMove false with a tick".into()),
                    Err(e) => why.push(e),
                }
            }
            // `SetPositionWithoutWaking(vector)`: the setter keeps `m_Position` when the step is
            // shorter than 1e-5; `PreviousPosition` is the position before (`ag_ctl`'s).
            let prev = v3(&ctl["pos"]);
            let pos = set_position(prev, v3(&te["ret"]));
            if bits(pos) != bits(v3(&te["ret"])) {
                n.stayed += 1;
            }
            if bits(pos) != bits(v3(&recs[end]["pos"])) {
                why.push(format!("position {pos:?}, ag_ctl_end {:?}", recs[end]["pos"]));
            }
            if let Some(dt) = dt
                && f(&t["dt"]).to_bits() != dt.to_bits()
            {
                why.push(format!("TickMovement's delta {} != DeltaTime {dt}", t["dt"]));
            }
            let mdir = v2(&te["s"]["mdir"]);
            match tick_unit_write(
                &mut o,
                has_motion(pos, prev),
                || take(false, "canrotate"),
                || Ok(!tactical),
                mdir,
            ) {
                Ok(w) => written = w,
                Err(e) => why.push(e),
            }
        }
        (None, None) => {}
        _ => why.push("a tick without its end".into()),
    }
    let (ro, rd, max) = (f(&rot["o"]), f(&rot["desired"]), f(&rot["max"]));
    if let Some(y) = written {
        n.written += 1;
        if y.to_bits() != ro.to_bits() || y.to_bits() != rd.to_bits() {
            why.push(format!("wrote {y}, the game {ro} (desired {rd})"));
        }
    } else if last.is_some_and(|l| l.to_bits() != ro.to_bits()) {
        // Written between TickUnits (ForceLookAt, the setter): not this check's.
        n.changed_outside += 1;
    }
    if let (Some(dt), Some(ang)) = (dt, ang) {
        n.max_checked += 1;
        let want = max_angle(dt, ang);
        if want.to_bits() != max.to_bits() {
            why.push(format!("maxAngle {want} (DeltaTime {dt} * {ang}), the game {max}"));
        }
    }
    let mut after = Orientation { value: ro, desired: rd };
    match take(true, "canrotate") {
        Ok(can) => after.update_slow_rotation(max, can),
        Err(e) => why.push(e),
    }
    if ro.to_bits() != after.value.to_bits() {
        n.turned += 1;
    }
    let want = f(&recs[end]["orient"]);
    if after.value.to_bits() != want.to_bits() {
        why.push(format!(
            "UpdateSlowRotation({ro} toward {rd} by {max}) = {}, the game {want}",
            after.value
        ));
    }
    if let Some(x) = leaves.next() {
        why.push(format!("leaf left unread: {x:?}"));
    }
    why
}

#[test]
#[ignore = "needs the poc5-tb-100 to -103 captures"]
fn every_recorded_orientation_reruns_bit_for_bit() {
    let mut n = Counts::default();
    let mut failures = Vec::new();
    for path in captures() {
        let trace = Trace::load(&path).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
        let recs = &trace.records;
        let name = path
            .iter()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(|c| c.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let angular = angular_speeds(recs).unwrap_or_else(|e| panic!("{name}: {e}"));
        // `TimeController.DeltaTime` for the frame (the `time` record), the game mode (`tick`).
        let mut dt = None;
        let mut tactical = false;
        // `m_Orientation` as last recorded, per unit (`ag_ctl_end`).
        let mut last: HashMap<String, f32> = HashMap::new();
        for (i, r) in recs.iter().enumerate() {
            match (r["k"].as_str(), r["ev"].as_str()) {
                (Some("time"), _) => dt = Some(f(&r["delta"])),
                (Some("tick"), _) => tactical = r["mode"].as_i64() == Some(TACTICAL_COMBAT),
                (_, Some("ag_ctl")) => {
                    let u = r["u"].as_str().unwrap_or("");
                    let Some(end) = (i + 1..recs.len())
                        .find(|&j| recs[j]["ev"] == "ag_ctl_end" && recs[j]["u"] == u)
                    else {
                        failures.push(format!("{name} t={} {u}: TickUnit without its end", r["t"]));
                        continue;
                    };
                    n.ctl += 1;
                    let k = Known {
                        dt,
                        tactical,
                        ang: angular.get(u).copied(),
                        last: last.get(u).copied(),
                    };
                    let why = check_ctl(recs, i, end, &k, &mut n);
                    if !why.is_empty() {
                        failures.push(format!("{name} t={} {u}: {}", r["t"], why.join("; ")));
                    }
                }
                (_, Some("ag_ctl_end")) => {
                    last.insert(r["u"].as_str().unwrap_or("").to_owned(), f(&r["orient"]));
                }
                (_, Some("ag_req")) if r["fn"] == "pathto" => {
                    let u = r["u"].as_str().unwrap_or("");
                    let Some(end) = (i + 1..recs.len())
                        .find(|&j| recs[j]["ev"] == "ag_req_end" && recs[j]["u"] == u)
                    else {
                        failures.push(format!("{name} t={} {u}: PathTo without its end", r["t"]));
                        continue;
                    };
                    let e = &recs[end];
                    let frame = r["frame"].as_i64();
                    // PathTo sets `m_PathFrame = Time.frameCount` right after InitMoveDirection,
                    // and nothing before it changes the agent's MoveDirection or m_PathFrame.
                    let before = r["s"]["pframe"].as_i64();
                    let after = e["s"]["pframe"].as_i64();
                    if after != frame {
                        if bits(v2(&e["s"]["mdir"])) != bits(v2(&r["s"]["mdir"])) {
                            failures.push(format!(
                                "{name} t={} {u}: PathTo returned early but MoveDirection changed",
                                r["t"]
                            ));
                        }
                        continue;
                    }
                    if before == frame {
                        n.init_ambiguous += 1;
                        continue;
                    }
                    let Some(&o) = last.get(u) else {
                        n.init_unknown += 1;
                        continue;
                    };
                    n.init += 1;
                    let want = v2(&e["s"]["mdir"]);
                    let ours = init_move_direction(o);
                    if bits(ours) != bits(want) {
                        failures.push(format!(
                            "{name} t={} {u}: InitMoveDirection({o}) = {ours:?}, the game {want:?}",
                            r["t"]
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    eprintln!(
        "{} TickUnits ({} moved, {} kept by the Position setter, {} orientations written, {} \
         turned by UpdateSlowRotation, maxAngle checked on {}; {} written between TickUnits); \
         InitMoveDirection: {} checked, {} before any TickUnit of the unit, {} ambiguous; {} \
         differ",
        n.ctl,
        n.moved,
        n.stayed,
        n.written,
        n.turned,
        n.max_checked,
        n.changed_outside,
        n.init,
        n.init_unknown,
        n.init_ambiguous,
        failures.len()
    );
    for x in failures.iter().take(30) {
        eprintln!("  {x}");
    }
    assert!(failures.is_empty());
}
