use super::*;
use crate::geometry::segments_intersect_2d;

fn blocker(points: &[[f32; 2]], closed: bool, height_y: f32) -> Blocker {
    Blocker {
        scene: "S".into(),
        path: "B".into(),
        sibling_path: vec![0],
        points: points.to_vec(),
        closed,
        height_y,
        source: "polygon".into(),
        updatable: false,
    }
}

fn area() -> Bounds {
    Bounds { center: [0.0, 0.0, 0.0], extents: [20.0, 25.0, 20.0] }
}

#[test]
fn a_wall_blocks_a_segment_that_crosses_it_and_not_one_that_doesnt() {
    let g = Geometry::init(area(), &[blocker(&[[0.0, -5.0], [0.0, 5.0]], false, 3.0)]).unwrap();
    assert!(g.has_obstacle([-3.0, 1.0, 0.0], [3.0, 1.0, 0.0], 0.0).unwrap());
    assert!(!g.has_obstacle([-3.0, 1.0, 7.0], [3.0, 1.0, 7.0], 0.0).unwrap());
    // Both ends on one side.
    assert!(!g.has_obstacle([1.0, 1.0, -2.0], [5.0, 1.0, 2.0], 0.0).unwrap());
}

#[test]
fn a_blocker_lower_than_the_eye_is_ignored() {
    let g = Geometry::init(area(), &[blocker(&[[0.0, -5.0], [0.0, 5.0]], false, 0.5)]).unwrap();
    assert!(!g.has_obstacle([-3.0, 1.0, 0.0], [3.0, 1.0, 0.0], 0.0).unwrap());
    assert!(g.has_obstacle([-3.0, 0.5, 0.0], [3.0, 0.5, 0.0], 0.0).unwrap());
}

#[test]
fn an_open_polyline_has_no_closing_segment() {
    let pts = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0]];
    let open = Geometry::init(area(), &[blocker(&pts, false, 3.0)]).unwrap();
    let closed = Geometry::init(area(), &[blocker(&pts, true, 3.0)]).unwrap();
    let (a, b) = ([1.0, 1.0, 3.0], [3.0, 1.0, 1.0]);
    assert!(!open.has_obstacle(a, b, 0.0).unwrap());
    assert!(closed.has_obstacle(a, b, 0.0).unwrap());
}

#[test]
fn nothing_outside_the_grid_is_blocked() {
    let g = Geometry::init(area(), &[blocker(&[[0.0, -5.0], [0.0, 5.0]], false, 3.0)]).unwrap();
    // Both ends outside: no cell, no obstacle (as the game's walk has it).
    assert!(!g.has_obstacle([-30.0, 1.0, 0.0], [30.0, 1.0, 0.0], 0.0).unwrap());
    // One end outside: the walk starts from the end that is inside.
    assert!(g.has_obstacle([-30.0, 1.0, 0.0], [3.0, 1.0, 0.0], 0.0).unwrap());
    assert!(g.has_obstacle([3.0, 1.0, 0.0], [-30.0, 1.0, 0.0], 0.0).unwrap());
}

#[test]
fn the_grid_is_the_bounds_in_3_9_metre_cells() {
    let g = Geometry::init(area(), &[]).unwrap();
    let (nx, nz, min_x, min_z) = g.grid();
    // 40 / 3.8999999 = 10.256: 11 cells, the bounds grown to 11 * 3.8999999 = 42.9.
    assert_eq!((nx, nz), (11, 11));
    assert!((min_x + 21.45).abs() < 1e-4 && (min_z + 21.45).abs() < 1e-4, "{min_x} {min_z}");
}

#[test]
fn a_fudge_radius_shortens_the_segment_at_its_far_end() {
    let g = Geometry::init(area(), &[blocker(&[[3.5, -5.0], [3.5, 5.0]], false, 3.0)]).unwrap();
    let (a, b) = ([0.0, 1.0, 0.0], [4.0, 1.0, 0.0]);
    assert!(g.has_obstacle(a, b, 0.0).unwrap());
    assert!(!g.has_obstacle(a, b, 1.0).unwrap());
}

/// The walk over the cells finds every crossing a brute-force test does, on generic positions.
#[test]
fn the_walk_agrees_with_every_segment_tested_directly() {
    // A small generator (xorshift): reproducible, no dependency.
    let mut s = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f64 / (1u64 << 53) as f64
    };
    let pt = |next: &mut dyn FnMut() -> f64| -> [f32; 2] {
        [(next() * 36.0 - 18.0) as f32, (next() * 36.0 - 18.0) as f32]
    };
    let mut hits = 0;
    for _ in 0..40 {
        let walls: Vec<Blocker> = (0..6)
            .map(|_| {
                let (a, b) = (pt(&mut next), pt(&mut next));
                blocker(&[a, b], false, 3.0)
            })
            .collect();
        let g = Geometry::init(area(), &walls).unwrap();
        for _ in 0..200 {
            let (a, b) = (pt(&mut next), pt(&mut next));
            let want = walls
                .iter()
                .any(|w| segments_intersect_2d(a, b, w.points[0], w.points[1]).is_some());
            let got = g.has_obstacle([a[0], 1.0, a[1]], [b[0], 1.0, b[1]], 0.0).unwrap();
            assert_eq!(got, want, "{a:?} -> {b:?} against {walls:?}");
            hits += usize::from(want);
        }
    }
    assert!(hits > 300, "{hits} blocked segments: the test should have both kinds");
}

#[test]
fn the_file_round_trips_and_damage_is_caught() {
    let f = LosFile {
        area: "a".into(),
        scenes: vec!["s.scenes".into()],
        blockers: vec![blocker(&[[0.0, 0.0], [1.0, 2.0]], true, 4.0)],
        areas: vec![FogArea { scene: "s".into(), path: "FoW".into(), position: [1.0, 2.0, 3.0] }],
        extended_vision_spawners: 0,
    };
    let dir = std::env::temp_dir().join(format!("wr-los-test-{}", std::process::id()));
    let path = dir.join("a.wrlos");
    f.write(&path).unwrap();
    assert_eq!(LosFile::read(&path).unwrap(), f);
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    assert!(LosFile::decode(&bytes).is_err());
    std::fs::remove_dir_all(&dir).ok();
}
