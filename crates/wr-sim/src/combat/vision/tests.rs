use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use wr_los::{Blocker, Bounds, Geometry};

use super::*;

const O: EntityIdx = EntityIdx(1);
const T: EntityIdx = EntityIdx(2);
const U: EntityIdx = EntityIdx(3);

#[derive(Default)]
struct Wv {
    pos: RefCell<HashMap<EntityIdx, V3>>,
    radius: RefCell<f32>,
    fake: RefCell<Vec<EntityIdx>>,
}

impl Wv {
    fn new() -> Self {
        let w = Self::default();
        *w.radius.borrow_mut() = 11.7;
        w
    }
    fn put(&self, u: EntityIdx, p: V3) {
        self.pos.borrow_mut().insert(u, p);
    }
}

impl VisionWorld for Wv {
    fn position(&self, u: EntityIdx) -> V3 {
        self.pos.borrow()[&u]
    }
    fn radius(&self, _: EntityIdx) -> f32 {
        *self.radius.borrow()
    }
    fn is_fake(&self, u: EntityIdx) -> bool {
        self.fake.borrow().contains(&u)
    }
}

fn wall(x: f32) -> Blocker {
    Blocker {
        scene: "S".into(),
        path: "W".into(),
        sibling_path: vec![],
        points: vec![[x, -10.0], [x, 10.0]],
        closed: false,
        height_y: 50.0,
        source: "polygon".into(),
        updatable: false,
    }
}

fn vision(walls: &[Blocker]) -> Vision {
    let b = Bounds { center: [0.0, 0.0, 0.0], extents: [30.0, 30.0, 30.0] };
    let mut v =
        Vision { geometry: Some(Rc::new(Geometry::init(b, walls).unwrap())), ..Vision::default() };
    v.near.entry(O).or_default().insert(T);
    v.near.entry(O).or_default().insert(U);
    v
}

fn miss(wv: &Wv, owner: EntityIdx, target: EntityIdx, sample: V3, result: bool) -> Miss {
    let p = wv.position(target);
    Miss { owner, point: [p[0], p[1] + 1.0, p[2]], sample, result }
}

/// A cache the window has cleared once (known), the owner at the origin.
fn known(v: &mut Vision, wv: &Wv) {
    v.adopt_clear(wv, O, wv.position(O)).unwrap();
}

#[test]
fn a_known_cache_misses_once_then_hits_while_the_target_stays_within_half_a_metre() {
    let wv = Wv::new();
    wv.put(O, [0.0, 0.0, 0.0]);
    wv.put(T, [5.0, 0.0, 0.0]);
    let mut v = vision(&[]);
    known(&mut v, &wv);
    v.pending.push_back(miss(&wv, O, T, [0.0; 3], true));
    assert_eq!(v.has_los(&wv, O, T), Ok(Los::Value(true)));
    assert!(v.pending.is_empty());
    // A hit: no record, and a recorded miss would be an error.
    wv.put(T, [5.3, 0.0, 0.2]);
    assert_eq!(v.has_los(&wv, O, T), Ok(Los::Value(true)));
    assert_eq!((v.counts.misses, v.counts.hits), (1, 1));
    // Beyond 0.5 m the cache misses again: the sim wants the record.
    wv.put(T, [5.6, 0.0, 0.0]);
    assert!(v.has_los(&wv, O, T).unwrap_err().contains("none was recorded"));
    v.pending.push_back(miss(&wv, O, T, [0.0; 3], true));
    assert_eq!(v.has_los(&wv, O, T), Ok(Los::Value(true)));
}

#[test]
fn a_blocker_between_the_units_hides_the_target_and_a_low_one_does_not() {
    let wv = Wv::new();
    wv.put(O, [0.0, 10.0, 0.0]);
    wv.put(T, [6.0, 10.0, 0.0]);
    let mut v = vision(&[wall(3.0)]);
    known(&mut v, &wv);
    v.pending.push_back(miss(&wv, O, T, [0.0, 10.0, 0.0], false));
    assert_eq!(v.has_los(&wv, O, T), Ok(Los::Value(false)));
    // The same wall below the eye is ignored.
    let mut w2 = wall(3.0);
    w2.height_y = 10.5;
    let mut v = vision(&[w2]);
    known(&mut v, &wv);
    v.pending.push_back(miss(&wv, O, T, [0.0, 10.0, 0.0], true));
    assert_eq!(v.has_los(&wv, O, T), Ok(Los::Value(true)));
}

#[test]
fn the_vision_radius_limits_the_miss_and_the_range_set_limits_the_read() {
    let wv = Wv::new();
    wv.put(O, [0.0; 3]);
    wv.put(T, [10.0, 0.0, 0.0]);
    wv.put(U, [20.0, 0.0, 0.0]);
    *wv.radius.borrow_mut() = 8.5;
    let mut v = vision(&[]);
    known(&mut v, &wv);
    // 10 m is beyond 8.5 m: the miss is false.
    v.pending.push_back(miss(&wv, O, T, [0.0; 3], false));
    assert_eq!(v.has_los(&wv, O, T), Ok(Los::Value(false)));
    // A unit outside CanBeInVisionRange asks no cache.
    v.near.get_mut(&O).unwrap().remove(&U);
    assert_eq!(v.has_los(&wv, O, U), Ok(Los::Value(false)));
    assert_eq!(v.counts.out_of_range, 1);
    // Unless the owner's blueprint is fake.
    wv.fake.borrow_mut().push(O);
    v.pending.push_back(miss(&wv, O, U, [0.0; 3], false));
    assert_eq!(v.has_los(&wv, O, U), Ok(Los::Value(false)));
}

#[test]
fn a_miss_that_differs_from_the_sims_is_refused() {
    let wv = Wv::new();
    wv.put(O, [0.0; 3]);
    wv.put(T, [5.0, 0.0, 0.0]);
    let mut v = vision(&[]);
    known(&mut v, &wv);
    // A wrong result, a wrong sample.
    v.pending.push_back(miss(&wv, O, T, [0.0; 3], false));
    assert!(v.has_los(&wv, O, T).is_err());
    v.pending.clear();
    v.pending.push_back(miss(&wv, O, T, [0.1, 0.0, 0.0], true));
    assert!(v.has_los(&wv, O, T).is_err());
    v.pending.clear();
    // A miss recorded for a read the sim answers from the cache.
    v.pending.push_back(miss(&wv, O, T, [0.0; 3], true));
    assert!(v.has_los(&wv, O, T).is_ok());
    v.pending.push_back(miss(&wv, O, T, [0.0; 3], true));
    assert!(v.has_los(&wv, O, T).unwrap_err().contains("the game missed"));
}

#[test]
fn the_cache_controller_clears_at_half_a_metre_or_a_new_radius() {
    let wv = Wv::new();
    wv.put(O, [0.0; 3]);
    wv.put(T, [5.0, 0.0, 0.0]);
    let mut v = vision(&[]);
    known(&mut v, &wv);
    v.pending.push_back(miss(&wv, O, T, [0.0; 3], true));
    v.has_los(&wv, O, T).unwrap();
    wv.put(O, [0.4, 0.0, 0.0]);
    assert_eq!(v.clear_if_needed(&wv, O), None);
    wv.put(O, [0.5, 0.0, 0.0]);
    assert_eq!(v.clear_if_needed(&wv, O), Some([0.5, 0.0, 0.0]));
    // Cleared: the entry is gone and the sample is the new position.
    v.pending.push_back(miss(&wv, O, T, [0.5, 0.0, 0.0], true));
    v.has_los(&wv, O, T).unwrap();
    *wv.radius.borrow_mut() = 8.5;
    assert_eq!(v.clear_if_needed(&wv, O), Some([0.5, 0.0, 0.0]));
    assert_eq!(v.clear_if_needed(&wv, O), None);
}

#[test]
fn a_cache_from_before_the_window_answers_hits_with_the_recordings_value_until_it_is_cleared() {
    let wv = Wv::new();
    wv.put(O, [0.0; 3]);
    wv.put(T, [5.0, 0.0, 0.0]);
    let mut v = vision(&[]);
    // Not known: no clear decided, an unknown entry is a hit whose value is the recording's.
    assert_eq!(v.clear_if_needed(&wv, O), None);
    assert_eq!(v.has_los(&wv, O, T), Ok(Los::Pre));
    v.resolve_pre(&wv, O, T, true).unwrap();
    assert_eq!((v.counts.pre_hits, v.counts.pre_fresh_equal), (1, 1));
    assert_eq!(v.has_los(&wv, O, T), Ok(Los::Value(true)));
    // The recording says otherwise later: refused.
    assert!(v.resolve_pre(&wv, O, T, false).is_err());
    // A miss recorded for a unit it has an entry for: the cache missed (the target moved), and
    // the sample is the record's.
    wv.put(T, [7.0, 0.0, 0.0]);
    v.pending.push_back(miss(&wv, O, T, [0.2, 0.0, 0.0], true));
    assert_eq!(v.has_los(&wv, O, T), Ok(Los::Value(true)));
    assert_eq!(v.caches[&O].sample, Some([0.2, 0.0, 0.0]));
    // The owner's recorded clear makes it known, at its own position only.
    assert!(v.adopt_clear(&wv, O, [9.0, 0.0, 0.0]).is_err());
    v.adopt_clear(&wv, O, [0.0; 3]).unwrap();
    assert!(v.caches[&O].known && v.caches[&O].entries.is_empty());
}

#[test]
fn a_miss_by_a_reader_the_sim_doesnt_run_is_verified_and_kept() {
    let wv = Wv::new();
    wv.put(O, [0.0; 3]);
    wv.put(T, [5.0, 0.0, 0.0]);
    wv.put(U, [0.0, 0.0, 9.0]);
    let mut v = vision(&[]);
    known(&mut v, &wv);
    let units = [O, T, U];
    v.adopt_miss(&wv, &units, miss(&wv, O, T, [0.0; 3], true)).unwrap();
    // The entry is there: a read now hits.
    assert_eq!(v.has_los(&wv, O, T), Ok(Los::Value(true)));
    // A wrong result, or no unit at the point, is refused.
    assert!(v.adopt_miss(&wv, &units, miss(&wv, O, U, [0.0; 3], false)).is_err());
    let mut lost = miss(&wv, O, U, [0.0; 3], true);
    lost.point = [50.0, 1.0, 50.0];
    assert!(v.adopt_miss(&wv, &units, lost).is_err());
}

#[test]
fn has_obstacle_reads_are_checked_against_the_recorded_answer() {
    let mut v = vision(&[wall(3.0)]);
    assert_eq!(v.obstacle([0.0, 10.0, 0.0], [6.0, 10.0, 0.0], 0.0, true), Ok(true));
    assert!(v.obstacle([0.0, 10.0, 0.0], [6.0, 10.0, 0.0], 0.0, false).is_err());
    assert_eq!(v.counts.obstacle_reads, 1);
}

/// What WrathOracle's `los_geom` and `los_probe` records would hold for the sim's own geometry,
/// and the check catching each way they could differ.
#[test]
fn the_geometry_and_its_probes_are_checked_against_a_dump() {
    let b = Bounds { center: [0.0, 0.0, 0.0], extents: [30.0, 30.0, 30.0] };
    let walls = [wall(3.0), wall(-4.5)];
    let g = Geometry::init(b, &walls).unwrap();
    let (nx, nz, min_x, min_z) = g.grid();
    let dump = GeomDump { nx, nz, min: [min_x, min_z], segs: g.segments() };
    assert_eq!(check_geometry(&g, &dump), Ok(dump.segs.len()));
    assert!(dump.segs.len() > 10);
    // The order within a cell is the registration order: not compared.
    let mut shuffled = dump.clone();
    shuffled.segs.reverse();
    assert!(check_geometry(&g, &shuffled).is_ok());
    // A moved point, a lower height, a missing segment, another grid.
    let mut bad = dump.clone();
    bad.segs[3].2[0] = f32::from_bits(bad.segs[3].2[0].to_bits() + 1);
    assert!(check_geometry(&g, &bad).is_err());
    let mut bad = dump.clone();
    bad.segs[0].4 -= 1.0;
    assert!(check_geometry(&g, &bad).is_err());
    let mut bad = dump.clone();
    bad.segs.pop();
    assert!(check_geometry(&g, &bad).is_err());
    let mut bad = dump.clone();
    bad.min[1] += 0.001;
    assert!(check_geometry(&g, &bad).is_err());
    // Probes.
    let probe = |from: V3, to: V3, result: bool| Probe { from, to, fudge: 0.0, result };
    let probes = [
        probe([0.0, 10.0, 0.0], [6.0, 10.0, 0.0], true),
        probe([-3.0, 10.0, 0.0], [2.0, 10.0, 0.0], false),
        probe([-6.0, 10.0, 5.0], [6.0, 10.0, -5.0], true),
    ];
    assert_eq!(check_probes(&g, &probes), Ok(3));
    let mut bad = probes;
    bad[1].result = true;
    assert!(check_probes(&g, &bad).unwrap_err().contains("probe 1"));
}
