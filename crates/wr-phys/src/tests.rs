use std::io::BufRead;

use super::*;
use crate::hf::{HeightField, Sample};
use crate::px::{Quat, Transform, Vec3};
use crate::query::Scene;

fn flat_field(height: i16) -> HeightField {
    HeightField {
        rows: 4,
        columns: 4,
        samples: vec![Sample { height, material0: 0x80, material1: 0 }; 16],
        height_scale: 0.5,
        row_scale: 2.0,
        column_scale: 2.0,
        double_sided: false,
    }
}

fn terrain_world(field: HeightField) -> World {
    World {
        shapes: vec![Shape {
            name: "terrain".into(),
            layer: 8,
            is_trigger: false,
            visual_layer_move: false,
            kind: ShapeKind::HeightField { pose: Transform::IDENTITY, field },
        }],
    }
}

fn sample_file() -> PhysFile {
    let mut world = terrain_world(flat_field(4));
    world.shapes.push(Shape {
        name: "box".into(),
        layer: 0,
        is_trigger: false,
        visual_layer_move: true,
        kind: ShapeKind::Box {
            local_to_world: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 20.0, 0.0, 0.0, 1.0,
            ],
            center: [0.0; 3],
            size: [1.0; 3],
        },
    });
    PhysFile {
        area: "bad465e5404604346bdf71ef50b3299a".into(),
        scenes: vec!["a.scenes".into()],
        world,
    }
}

fn encode(f: &PhysFile) -> Vec<u8> {
    static CALLS: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "wr-phys-test-{}-{}",
        std::process::id(),
        CALLS.fetch_add(1, Ordering::Relaxed)
    ));
    let path = dir.join("a.wrphys");
    f.write(&path).unwrap();
    let bytes = fs::read(&path).unwrap();
    fs::remove_dir_all(&dir).unwrap();
    bytes
}

#[test]
fn round_trips() {
    let f = sample_file();
    assert_eq!(PhysFile::decode(&encode(&f)).unwrap(), f);
}

#[test]
fn damaged_files_are_rejected() {
    let good = encode(&sample_file());
    let mut cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        ("truncated header", good[..20].to_vec()),
        ("truncated payload", good[..good.len() - 1].to_vec()),
        ("trailing byte", [good.as_slice(), &[0]].concat()),
    ];
    let mut magic = good.clone();
    magic[0] ^= 1;
    cases.push(("magic", magic));
    let mut version = good.clone();
    version[8] = 2;
    cases.push(("version", version));
    let mut flipped = good.clone();
    *flipped.last_mut().unwrap() ^= 0x40;
    cases.push(("payload byte", flipped));
    for (what, bytes) in cases {
        assert!(PhysFile::decode(&bytes).is_err(), "{what} accepted");
    }
    // A structurally wrong world is refused even with a good checksum.
    let mut f = sample_file();
    if let ShapeKind::HeightField { field, .. } = &mut f.world.shapes[0].kind {
        field.samples.pop();
    }
    assert!(PhysFile::decode(&encode(&f)).is_err(), "short sample array accepted");
}

#[test]
fn a_flat_field_is_hit_at_its_height() {
    // Height 4 * 0.5 = 2 over x, z in [0, 6].
    let w = terrain_world(flat_field(4));
    let scene = Scene::new(&w);
    let hit = scene.raycast_down([1.3, 7.0, 4.1], 100.0, 1 << 8).unwrap().unwrap();
    assert_eq!(hit.distance, 5.0);
    assert_eq!(hit.point[1], 2.0);
    assert_eq!(scene.raycast_down([1.3, 7.0, 4.1], 4.0, 1 << 8).unwrap(), None, "too short");
    assert_eq!(scene.raycast_down([1.3, 7.0, 4.1], 100.0, 1).unwrap(), None, "other layers");
    assert_eq!(scene.raycast_down([7.3, 7.0, 4.1], 100.0, 1 << 8).unwrap(), None, "beside it");
    assert_eq!(scene.raycast_down([1.3, 1.0, 4.1], 100.0, 1 << 8).unwrap(), None, "under it");
}

#[test]
fn holes_and_tess_flags() {
    // A sloped cell: heights 0 at x=0, 8 at x=1 (scaled 0.5 → 0..4 over 2 m).
    let mut f = flat_field(0);
    for (i, s) in f.samples.iter_mut().enumerate() {
        s.height = if i / 4 >= 1 { 8 } else { 0 };
    }
    let w = terrain_world(f.clone());
    let scene = Scene::new(&w);
    let hit = scene.raycast_down([1.0, 10.0, 1.0], 100.0, 1 << 8).unwrap().unwrap();
    assert!((hit.distance - 8.0).abs() < 1e-5, "{hit:?}");
    // Both triangles of cell (0, 0) holes: the ray falls through to nothing.
    f.samples[0].material0 = 0x80 | crate::hf::HOLE;
    f.samples[0].material1 = crate::hf::HOLE;
    let w = terrain_world(f);
    let scene = Scene::new(&w);
    assert_eq!(scene.raycast_down([1.0, 10.0, 1.0], 100.0, 1 << 8).unwrap(), None);
}

#[test]
fn unverified_shapes_fail_closed() {
    let mut f = sample_file();
    // The box at x = 20 is on layer 0, its GameObject a `*_!Visual` one (maybe on 11).
    let both = 1 | 1 << 11;
    let scene = Scene::new(&f.world);
    assert!(matches!(
        scene.raycast_down([20.2, 5.0, 0.3], 100.0, both),
        Err(QueryError::Unverified { .. })
    ));
    assert!(matches!(scene.raycast_down([20.2, 5.0, 0.3], 1.0, both), Ok(None)), "out of reach");
    assert_eq!(scene.raycast_down([22.0, 5.0, 0.3], 100.0, both).unwrap(), None);
    // A mask with 0 and not 11 can't be answered while the layer move isn't modelled.
    assert!(matches!(
        scene.raycast_down([22.0, 5.0, 0.3], 100.0, 1),
        Err(QueryError::Unsupported(_))
    ));
    f.world.shapes[1].visual_layer_move = false;
    let scene = Scene::new(&f.world);
    assert_eq!(scene.raycast_down([22.0, 5.0, 0.3], 100.0, 1).unwrap(), None);
    // A trigger in the mask depends on `queriesHitTriggers`.
    f.world.shapes[1].is_trigger = true;
    let scene = Scene::new(&f.world);
    assert!(matches!(
        scene.raycast_down([22.0, 5.0, 0.3], 100.0, 1),
        Err(QueryError::Unsupported(_))
    ));
}

#[test]
fn meshes_and_other_shapes_fail_closed() {
    let id = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    let world = World {
        shapes: vec![
            Shape {
                name: "mesh".into(),
                layer: 0,
                is_trigger: false,
                visual_layer_move: false,
                kind: ShapeKind::Mesh {
                    local_to_world: id,
                    vertices: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 100.0], [100.0, 0.0, 0.0]],
                    triangles: vec![[0, 1, 2]],
                    geom_epsilon: 100.0 * 2f32.powi(-22),
                },
            },
            Shape {
                name: "capsule".into(),
                layer: 1,
                is_trigger: false,
                visual_layer_move: false,
                kind: ShapeKind::Other {
                    class: "CapsuleCollider".into(),
                    min: [200.0, 0.0, 0.0],
                    max: [201.0, 2.0, 1.0],
                },
            },
        ],
    };
    let scene = Scene::new(&world);
    let unverified = |o: [f32; 3], mask: i32| {
        matches!(scene.raycast_down(o, 100.0, mask), Err(QueryError::Unverified { .. }))
    };
    assert!(unverified([30.0, 1.0, 30.0], 1));
    // 1.41 mm outside the triangle, inside BV4's enlargement.
    assert!(unverified([50.001, 1.0, 50.001], 1));
    // Beyond an outer edge, within the enlargement: the prefilter must not drop it.
    assert!(unverified([-0.002, 1.0, 25.0], 1));
    // A sliver beside a far triangle: its f32 arithmetic can hit outside its own bounds, inside
    // the mesh's (the player's BV4_RaycastSingle hits this at 0.5).
    let sliver = World {
        shapes: vec![Shape {
            name: "sliver".into(),
            layer: 0,
            is_trigger: false,
            visual_layer_move: false,
            kind: ShapeKind::Mesh {
                local_to_world: id,
                vertices: vec![
                    [0.0, 0.0, 0.0],
                    [2.0, 2.0, 2.0],
                    [1.0, 1.000_000_1, 0.999_999_9],
                    [-10.0, -200.0, -10.0],
                    [-10.0, -200.0, 10.0],
                    [10.0, -200.0, 10.0],
                ],
                triangles: vec![[0, 1, 2], [3, 4, 5]],
                geom_epsilon: 2f32.powi(-21),
            },
        }],
    };
    let s2 = Scene::new(&sliver);
    assert!(matches!(
        s2.raycast_down([2.3, 3.0, 2.3], 100.0, 1),
        Err(QueryError::Unverified { .. })
    ));
    assert_eq!(scene.raycast_down([60.0, 1.0, 60.0], 100.0, 1).unwrap(), None);
    assert_eq!(scene.raycast_down([30.0, 1.0, 30.0], 100.0, 2).unwrap(), None, "other layer");
    assert!(unverified([200.5, 5.0, 0.5], 2));
    assert_eq!(scene.raycast_down([202.5, 5.0, 0.5], 100.0, 2).unwrap(), None);
}

#[test]
fn disc_points_match_the_static_constructor() {
    let p = query::disc_points();
    // sin(3π/4) and sin(π/2) as Mathf computes them.
    assert_eq!(p[0], [0.70710677, 0.0, 0.0]);
    assert_eq!(p[4], [1.0, 0.0, 0.0]);
    assert_eq!(p[6][0], -4.371139e-8);
    assert_eq!(p[6][2], 1.0);
    assert!(p.iter().all(|v| v[1] == 0.0));
}

// --- The recorded queries ---------------------------------------------------------------

/// `poc5-tb-103`'s six runs (override with `WR_PHYS_CAPTURES`, comma-separated labels).
fn captures() -> Vec<PathBuf> {
    let labels = std::env::var("WR_PHYS_CAPTURES").unwrap_or_else(|_| "poc5-tb-103".into());
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

fn f(v: &serde_json::Value) -> f32 {
    v.as_f64().map_or(f32::NAN, |x| x as f32)
}

fn v3(v: &serde_json::Value) -> [f32; 3] {
    [f(&v[0]), f(&v[1]), f(&v[2])]
}

fn ulps(a: f32, b: f32) -> u32 {
    let key = |x: f32| {
        let i = x.to_bits() as i32;
        if i < 0 { i32::MIN - i } else { i }
    };
    key(a).abs_diff(key(b))
}

#[derive(Default, Debug)]
struct Tally {
    queries: usize,
    discs: usize,
    exact: usize,
    point_exact: usize,
    near: usize,
    wrong: usize,
    errors: usize,
}

#[test]
#[ignore = "needs poc5-tb-103 and the area's physics file (wr-import phys-pack)"]
fn every_recorded_query_is_computed_exactly() {
    let t0 = std::time::Instant::now();
    let file = PhysFile::read(&path_for("bad465e5404604346bdf71ef50b3299a")).unwrap();
    let scene = Scene::new(&file.world);
    eprintln!("physics file read and prepared in {:?}", t0.elapsed());
    // faceHit's distance can exceed the traced length; the scene query drops such a hit.
    let (o, max) = ([128.67328, 62.99409, 67.102905], 21.423319);
    let hf = file.world.shapes.iter().find_map(|s| match &s.kind {
        ShapeKind::HeightField { pose, field } => Some((pose, field)),
        _ => None,
    });
    let (pose, field) = hf.unwrap();
    let alone = field.raycast(*pose, Vec3::from_array(o), Vec3::new(0.0, -1.0, 0.0), max, false);
    assert!(alone.is_some_and(|h| h.distance > max), "{alone:?}");
    assert_eq!(scene.raycast_down(o, max, 1 << 8).unwrap(), None);
    let mut distinct = std::collections::HashSet::new();
    let mut total = Tally::default();
    let mut failures = Vec::new();
    for path in captures() {
        let gz = fs::File::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let mut t = Tally::default();
        for line in std::io::BufReader::new(flate2::read::GzDecoder::new(gz)).lines() {
            let line = line.unwrap();
            if !line.contains("\"ag_phys\"") {
                continue;
            }
            let r: serde_json::Value = serde_json::from_str(&line).unwrap();
            if r["ev"] != "ag_phys" {
                continue;
            }
            t.queries += 1;
            t.discs += usize::from(r["fn"] == "disc");
            distinct.insert((r["fn"].to_string(), v3(&r["o"]).map(f32::to_bits)));
            let o = v3(&r["o"]);
            let (max, mask) = (f(&r["max"]), r["mask"].as_i64().unwrap() as i32);
            let got = match r["fn"].as_str() {
                Some("ray") => {
                    assert_eq!(v3(&r["d"]), [0.0, -1.0, 0.0], "a ray not straight down: {r}");
                    scene.raycast_down(o, max, mask)
                }
                Some("disc") => scene.heuristic_disc_cast_down(o, f(&r["r"]), max, mask),
                other => panic!("unknown query {other:?}"),
            };
            let want = r["hit"].as_bool().unwrap().then(|| (f(&r["dist"]), v3(&r["pt"])));
            let what = format!(
                "{} t={} {} from {o:?}",
                path.parent().unwrap().file_name().unwrap().to_string_lossy(),
                r["t"],
                r["fn"]
            );
            match (got, want) {
                (Err(e), _) => {
                    t.errors += 1;
                    failures.push(format!("{what}: {e}"));
                }
                (Ok(None), None) => {
                    t.exact += 1;
                    t.point_exact += 1;
                }
                (Ok(Some(h)), Some((d, pt))) => {
                    if h.distance.to_bits() == d.to_bits() {
                        t.exact += 1;
                    } else if ulps(h.distance, d) <= 4 {
                        t.near += 1;
                        failures.push(format!(
                            "{what}: distance {} vs the game's {d} ({} ulp)",
                            h.distance,
                            ulps(h.distance, d)
                        ));
                    } else {
                        t.wrong += 1;
                        failures.push(format!("{what}: distance {} vs the game's {d}", h.distance));
                    }
                    if h.point.map(f32::to_bits) == pt.map(f32::to_bits) {
                        t.point_exact += 1;
                    } else {
                        failures.push(format!("{what}: point {:?} vs the game's {pt:?}", h.point));
                    }
                }
                (got, want) => {
                    t.wrong += 1;
                    failures.push(format!("{what}: {got:?} vs the game's {want:?}"));
                }
            }
        }
        eprintln!("{}: {t:?}", path.display());
        total.queries += t.queries;
        total.discs += t.discs;
        total.exact += t.exact;
        total.point_exact += t.point_exact;
        total.near += t.near;
        total.wrong += t.wrong;
        total.errors += t.errors;
    }
    eprintln!("total: {total:?}; {} distinct queries", distinct.len());
    for x in failures.iter().take(30) {
        eprintln!("  {x}");
    }
    assert!(total.queries > 0, "no ag_phys records");
    assert!(failures.is_empty(), "{} queries differ", failures.len());
}

#[test]
fn quat_helpers_are_inverse() {
    let q = Quat { x: 0.0, y: 0.38268343, z: 0.0, w: 0.9238795 };
    let pose = Transform { q, p: Vec3::new(1.0, 2.0, 3.0) };
    let v = Vec3::new(4.0, -5.0, 6.5);
    let back = pose.inverse().transform(pose.transform(v));
    assert!(
        (back.x - v.x).abs() < 1e-5 && (back.y - v.y).abs() < 1e-5 && (back.z - v.z).abs() < 1e-5
    );
}
