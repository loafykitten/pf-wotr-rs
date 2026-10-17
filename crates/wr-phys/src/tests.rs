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
        scenes: Vec::new(),
        shapes: vec![Shape {
            name: "terrain".into(),
            sibling_path: Vec::new(),
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
        sibling_path: Vec::new(),
        layer: 0,
        is_trigger: false,
        visual_layer_move: true,
        kind: ShapeKind::Box {
            local_to_world: [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 20.0, 0.0, 0.0, 1.0,
            ],
            center: [0.0; 3],
            size: [1.0; 3],
            chain: Vec::new(),
            runtime: None,
        },
    });
    world.shapes.push(recorded_square("square", [30.0, 0.0, 0.0], 0));
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
    version[8] = 1;
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
    let mut f = sample_file();
    if let ShapeKind::Mesh { mesh, .. } = &mut f.world.shapes[2].kind {
        mesh.triangles[0][1] = 4;
    }
    assert!(PhysFile::decode(&encode(&f)).is_err(), "a mesh index out of range accepted");
    let mut f = sample_file();
    if let ShapeKind::Mesh { runtime: Some(r), .. } = &mut f.world.shapes[2].kind {
        r.lossy_scale[1] = 0.0;
    }
    assert!(PhysFile::decode(&encode(&f)).is_err(), "a zero recorded scale accepted");
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
fn several_height_fields_fail_closed() {
    let mut w = terrain_world(flat_field(4));
    w.shapes.push(w.shapes[0].clone());
    let scene = Scene::new(&w);
    assert!(matches!(
        scene.raycast_down([1.3, 7.0, 4.1], 100.0, 1 << 8),
        Err(QueryError::Unsupported(_))
    ));
    assert_eq!(scene.raycast_down([1.3, 7.0, 4.1], 100.0, 1).unwrap(), None, "neither in the mask");
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

/// A small mesh (under 16 triangles: no tree, brute force) under `local_to_world`, its pose
/// not established (the conservative test answers for it).
fn small_mesh(
    local_to_world: [f32; 16],
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[u32; 3]>,
    geom_epsilon: f32,
) -> ShapeKind {
    ShapeKind::Mesh {
        local_to_world,
        pose: Transform::IDENTITY,
        lossy_scale: [1.0; 3],
        runtime: None,
        mesh: crate::bv4::Bv4Mesh {
            vertices,
            triangles,
            geom_epsilon,
            tree: crate::bv4::Bv4Tree {
                center: [0.0; 3],
                extents_magnitude: 1e6,
                init_data: 0,
                min_coeff: [0.0; 3],
                max_coeff: [0.0; 3],
                nodes: Vec::new(),
            },
        },
    }
}

#[test]
fn meshes_and_other_shapes_fail_closed() {
    let id = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    let world = World {
        scenes: Vec::new(),
        shapes: vec![
            Shape {
                name: "mesh".into(),
                sibling_path: Vec::new(),
                layer: 0,
                is_trigger: false,
                visual_layer_move: false,
                kind: small_mesh(
                    id,
                    vec![[0.0, 0.0, 0.0], [0.0, 0.0, 100.0], [100.0, 0.0, 0.0]],
                    vec![[0, 1, 2]],
                    100.0 * 2f32.powi(-22),
                ),
            },
            Shape {
                name: "capsule".into(),
                sibling_path: Vec::new(),
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
        scenes: Vec::new(),
        shapes: vec![Shape {
            name: "sliver".into(),
            sibling_path: Vec::new(),
            layer: 0,
            is_trigger: false,
            visual_layer_move: false,
            kind: small_mesh(
                id,
                vec![
                    [0.0, 0.0, 0.0],
                    [2.0, 2.0, 2.0],
                    [1.0, 1.000_000_1, 0.999_999_9],
                    [-10.0, -200.0, -10.0],
                    [-10.0, -200.0, 10.0],
                    [10.0, -200.0, 10.0],
                ],
                vec![[0, 1, 2], [3, 4, 5]],
                2f32.powi(-21),
            ),
        }],
    };
    let s2 = Scene::new(&sliver);
    assert!(matches!(
        s2.raycast_down([2.3, 3.0, 2.3], 100.0, 1),
        Err(QueryError::Unverified { .. })
    ));
    // The sliver with an unused vertex widening PhysX's bounds.
    let unused = World {
        scenes: Vec::new(),
        shapes: vec![Shape {
            name: "unused".into(),
            sibling_path: Vec::new(),
            layer: 0,
            is_trigger: false,
            visual_layer_move: false,
            kind: small_mesh(
                id,
                vec![
                    [0.0, 0.0, 0.0],
                    [2.0, 2.0, 2.0],
                    [1.0, 1.000_000_1, 0.999_999_9],
                    [3.0, 0.0, 3.0],
                ],
                vec![[0, 1, 2]],
                3.0 * 2f32.powi(-22),
            ),
        }],
    };
    let s5 = Scene::new(&unused);
    assert!(matches!(
        s5.raycast_down([2.3, 3.0, 2.3], 100.0, 1),
        Err(QueryError::Unverified { .. })
    ));
    // The same sliver beside a small far triangle: the mesh's own bounds, grown, still decide.
    let mut far = sliver.clone();
    if let ShapeKind::Mesh { mesh, .. } = &mut far.shapes[0].kind {
        mesh.vertices[3..].copy_from_slice(&[
            [-1000.0, -200.0, -1000.0],
            [-1000.0, -200.0, -999.0],
            [-999.0, -200.0, -1000.0],
        ]);
    }
    let s3 = Scene::new(&far);
    assert!(matches!(
        s3.raycast_down([2.3, 3.0, 2.3], 100.0, 1),
        Err(QueryError::Unverified { .. })
    ));
    // A long ray far out: the origin's rounding, amplified by the triangle's angle (the player's
    // midphase hits at 999996.625).
    let long = World {
        scenes: Vec::new(),
        shapes: vec![Shape {
            name: "long".into(),
            sibling_path: Vec::new(),
            layer: 0,
            is_trigger: false,
            visual_layer_move: false,
            kind: small_mesh(
                id,
                vec![
                    [0.0, 0.0, 0.0],
                    [1.0, -1.0, 1.00528],
                    [1.0, 1.0, 1.0],
                    [-10.0, 10.0, -10.0],
                    [-10.0, 10.0, -9.0],
                    [-9.0, 10.0, -10.0],
                ],
                vec![[0, 1, 2], [3, 4, 5]],
                10.0 * 2f32.powi(-22),
            ),
        }],
    };
    let s4 = Scene::new(&long);
    assert!(matches!(
        s4.raycast_down([0.5, 1_000_000.0, 0.501584], 999_997.0, 1),
        Err(QueryError::Unverified { .. })
    ));
    assert_eq!(scene.raycast_down([60.0, 1.0, 60.0], 100.0, 1).unwrap(), None);
    assert_eq!(scene.raycast_down([30.0, 1.0, 30.0], 100.0, 2).unwrap(), None, "other layer");
    assert!(unverified([200.5, 5.0, 0.5], 2));
    assert_eq!(scene.raycast_down([202.5, 5.0, 0.5], 100.0, 2).unwrap(), None);
}

/// A 4 m square facing up (two triangles, no tree), recorded at `position` (identity rotation
/// and scale) on `layer`.
fn recorded_square(name: &str, position: [f32; 3], layer: u32) -> Shape {
    let mut kind = small_mesh(
        [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        vec![[0.0, 0.0, 0.0], [0.0, 0.0, 4.0], [4.0, 0.0, 0.0], [4.0, 0.0, 4.0]],
        vec![[0, 1, 2], [1, 3, 2]],
        4.0 * 2f32.powi(-22),
    );
    if let ShapeKind::Mesh { runtime, mesh, .. } = &mut kind {
        mesh.tree.center = [2.0, 0.0, 2.0];
        mesh.tree.extents_magnitude = 8f32.sqrt();
        *runtime = Some(crate::MeshRuntime {
            position,
            rotation: [0.0, 0.0, 0.0, 1.0],
            lossy_scale: [1.0; 3],
            layer,
            source: "test".into(),
        });
    }
    Shape {
        name: name.into(),
        sibling_path: Vec::new(),
        layer,
        is_trigger: false,
        visual_layer_move: false,
        kind,
    }
}

#[test]
fn a_recorded_mesh_is_computed() {
    // The terrain at 2 over [0, 6]², a square at 3 over [1, 5]².
    let mut w = terrain_world(flat_field(4));
    w.shapes.push(recorded_square("square", [1.0, 3.0, 1.0], 0));
    let scene = Scene::new(&w);
    let both = 1 | 1 << 8;
    let hit = scene.raycast_down([2.0, 7.0, 2.0], 100.0, both).unwrap().unwrap();
    assert_eq!((hit.distance, hit.point, hit.shape), (4.0, [2.0, 3.0, 2.0], 1));
    // Beside the square, or the square's layer out of the mask: the terrain.
    let hit = scene.raycast_down([5.5, 7.0, 2.0], 100.0, both).unwrap().unwrap();
    assert_eq!((hit.distance, hit.shape), (5.0, 0));
    let hit = scene.raycast_down([2.0, 7.0, 2.0], 100.0, 1 << 8).unwrap().unwrap();
    assert_eq!((hit.distance, hit.shape), (5.0, 0));
    // Ending above the square; from below it (its back face isn't hit).
    assert_eq!(scene.raycast_down([2.0, 7.0, 2.0], 3.5, both).unwrap(), None);
    let hit = scene.raycast_down([2.0, 2.5, 2.0], 100.0, both).unwrap().unwrap();
    assert_eq!((hit.distance, hit.shape), (0.5, 0));
}

#[test]
fn a_tie_between_shapes_fails_closed() {
    // Two squares in the same place: the pruner's order decides which one the game reports.
    let mut w = terrain_world(flat_field(4));
    w.shapes.push(recorded_square("a", [1.0, 3.0, 1.0], 0));
    w.shapes.push(recorded_square("b", [1.0, 3.0, 1.0], 0));
    let scene = Scene::new(&w);
    assert!(matches!(
        scene.raycast_down([2.0, 7.0, 2.0], 100.0, 1 | 1 << 8),
        Err(QueryError::Unsupported(_))
    ));
    // One a centimetre higher: no tie.
    w.shapes[2] = recorded_square("b", [1.0, 3.01, 1.0], 0);
    let scene = Scene::new(&w);
    let hit = scene.raycast_down([2.0, 7.0, 2.0], 100.0, 1 | 1 << 8).unwrap().unwrap();
    assert_eq!(hit.shape, 2);
}

#[test]
fn recorded_colliders_are_checked_against_the_file() {
    let mut s = recorded_square("S:Static/sq (MeshCollider)", [1.0, 0.0, 2.0], 11);
    s.sibling_path = vec![3];
    let mut b = recorded_box("S:Static/b (BoxCollider)", [1.0, 2.0, 3.0], [0.0, 0.5, 0.0]);
    b.sibling_path = vec![4];
    let w = World { shapes: vec![s, b.clone()], scenes: Vec::new() };
    let good = RecordedCollider {
        name: "S:Static/sq (MeshCollider)".into(),
        sibling_path: vec![3],
        present: true,
        layer: 11,
        position: [1.0, -0.0, 2.0],
        rotation: [0.0, 0.0, -0.0, 1.0],
        lossy_scale: [1.0; 3],
        trigger: false,
        kind: RecordedKind::Mesh { convex: false, cooking: 30 },
    };
    let ShapeKind::Box { center, size, runtime: Some(r), .. } = &b.kind else { unreachable!() };
    let good_box = RecordedCollider {
        name: "S:Static/b (BoxCollider)".into(),
        sibling_path: vec![4],
        present: true,
        layer: r.layer,
        position: r.position,
        rotation: r.rotation,
        lossy_scale: r.lossy_scale,
        trigger: false,
        kind: RecordedKind::Box { center: *center, size: *size },
    };
    let both = [good.clone(), good_box.clone()];
    assert_eq!(w.check_recorded_colliders(&both), Ok(()));
    assert!(w.check_recorded_colliders(&[]).is_err(), "unrecorded");
    assert!(w.check_recorded_colliders(std::slice::from_ref(&good)).is_err(), "the box unrecorded");
    assert!(
        w.check_recorded_colliders(&[good.clone(), good.clone(), good_box.clone()]).is_err(),
        "twice"
    );
    let other_scene = RecordedCollider { name: "T:Unit/c (MeshCollider)".into(), ..good.clone() };
    assert_eq!(
        w.check_recorded_colliders(&[good.clone(), good_box.clone(), other_scene]),
        Ok(()),
        "another scene"
    );
    let changes: [fn(&mut RecordedCollider); 7] = [
        |c| c.lossy_scale[0] = 1.000_000_1,
        |c| c.position[2] = 2.000_000_2,
        |c| c.layer = 0,
        |c| c.present = false,
        |c| c.trigger = true,
        |c| c.kind = RecordedKind::Mesh { convex: true, cooking: 30 },
        |c| c.kind = RecordedKind::Mesh { convex: false, cooking: 14 },
    ];
    for (k, change) in changes.iter().enumerate() {
        let mut c = good.clone();
        change(&mut c);
        assert!(w.check_recorded_colliders(&[c, good_box.clone()]).is_err(), "change {k} accepted");
    }
    let box_changes: [fn(&mut RecordedCollider); 6] = [
        |c| c.lossy_scale[1] = 1.000_000_1,
        |c| c.rotation[3] = 0.999_999_9,
        |c| c.layer = 0,
        |c| c.present = false,
        |c| c.trigger = true,
        // Another box of the same GameObject: this one is then unrecorded.
        |c| c.kind = RecordedKind::Box { center: [0.0; 3], size: [1.0; 3] },
    ];
    for (k, change) in box_changes.iter().enumerate() {
        let mut c = good_box.clone();
        change(&mut c);
        assert!(w.check_recorded_colliders(&[good.clone(), c]).is_err(), "box change {k} accepted");
    }
    let mut extra = good.clone();
    extra.sibling_path = vec![5];
    assert!(
        w.check_recorded_colliders(&[good, good_box, extra]).is_err(),
        "a mesh the file doesn't have"
    );
}

/// A 2 x 1 x 4 box (centre `center`) on a GameObject at `position` under a root at the origin,
/// unrotated and unscaled, recorded on layer 8.
fn recorded_box(name: &str, position: [f32; 3], center: [f32; 3]) -> Shape {
    let chain = vec![
        boxes::LocalTrs { position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
        boxes::LocalTrs { position, rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] },
    ];
    let size = [2.0, 1.0, 4.0];
    let mut m = [0.0f32; 16];
    (m[0], m[5], m[10], m[15]) = (1.0, 1.0, 1.0, 1.0);
    m[12..15].copy_from_slice(&position);
    Shape {
        name: name.into(),
        sibling_path: Vec::new(),
        layer: 8,
        is_trigger: false,
        visual_layer_move: false,
        kind: ShapeKind::Box {
            local_to_world: m,
            center,
            size,
            runtime: Some(boxes::BoxRuntime {
                world_center: boxes::transform_point(&chain, center),
                rotation: [0.0, 0.0, 0.0, 1.0],
                lossy_scale: [1.0; 3],
                position,
                layer: 8,
                source: "test".into(),
            }),
            chain,
        },
    }
}

#[test]
fn a_recorded_box_is_computed() {
    // The terrain at 2 over [0, 6]², a box with its top at 3.5 over [1, 3] x [0, 4].
    let mut w = terrain_world(flat_field(4));
    w.shapes.push(recorded_box("box", [2.0, 2.5, 2.0], [0.0, 0.5, 0.0]));
    let scene = Scene::new(&w);
    let hit = scene.raycast_down([2.5, 7.0, 1.0], 100.0, 1 << 8).unwrap().unwrap();
    assert_eq!((hit.distance, hit.point, hit.shape), (3.5, [2.5, 3.5, 1.0], 1));
    // Beside it; ending above it; from inside it (PhysX's hit at distance 0, which Unity's
    // post-filter drops: the terrain below).
    let hit = scene.raycast_down([3.5, 7.0, 1.0], 100.0, 1 << 8).unwrap().unwrap();
    assert_eq!((hit.distance, hit.shape), (5.0, 0));
    assert_eq!(scene.raycast_down([2.5, 7.0, 1.0], 3.0, 1 << 8).unwrap(), None);
    let hit = scene.raycast_down([2.5, 3.0, 1.0], 100.0, 1 << 8).unwrap().unwrap();
    assert_eq!((hit.distance, hit.point, hit.shape), (1.0, [2.5, 2.0, 1.0], 0));
    // From exactly on its top: distance 0 too.
    let hit = scene.raycast_down([2.5, 3.5, 1.0], 100.0, 1 << 8).unwrap().unwrap();
    assert_eq!((hit.distance, hit.shape), (1.5, 0));
    // A box whose mapping isn't established (an ancestor rotated) keeps the conservative test.
    let mut b = recorded_box("rotated parent", [2.0, 2.5, 2.0], [0.0, 0.5, 0.0]);
    if let ShapeKind::Box { chain, .. } = &mut b.kind {
        chain[0].rotation = [0.0, 0.6, 0.0, 0.8];
    }
    w.shapes[1] = b;
    let scene = Scene::new(&w);
    assert!(matches!(
        scene.raycast_down([2.5, 7.0, 1.0], 100.0, 1 << 8),
        Err(QueryError::Unverified { .. })
    ));
}

#[test]
fn the_box_mapping_is_unitys() {
    // TransformPoint(center), not position + rotation * (lossy * center); the rotation as PhysX
    // normalises it; half extents |size * lossy| / 2. A tilted, non-uniformly scaled box under
    // a translated root (Static/tower_wood_trash_04 (2) [453] in the caves, whose probes'
    // distances need the first two: `docs/poc5/physics.md`).
    let chain = [
        boxes::LocalTrs {
            position: [67.316_185, -14.138_412, 6.578_766],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        },
        boxes::LocalTrs {
            position: [49.237_022, 53.973_633, 127.707_03],
            rotation: [0.116_654_08, -0.706_635_36, 0.038_710_81, 0.696_821_2],
            scale: [1.0, 1.0, 0.257_168_6],
        },
    ];
    let center = [0.100_269_49, 0.031_055_193, -1.674_569_3e-8];
    let p = boxes::transform_point(&chain, [0.0; 3]);
    assert_eq!(p, [116.553_21, 39.835_22, 134.2858], "Transform.position as recorded");
    let c = boxes::transform_point(&chain, center);
    // The actor's position plus the rotated, scaled centre rounds the other way in x.
    assert_eq!(c[0].to_bits(), 116.546_25f32.to_bits());
    let rot = Quat { x: 0.116_654_08, y: -0.706_635_36, z: 0.038_710_81, w: 0.696_821_2 };
    let lossy = [1.0, 0.999_999_94, 0.257_168_62];
    let alt =
        rot.rotate(Vec3::new(center[0] * lossy[0], center[1] * lossy[1], center[2] * lossy[2]));
    assert_eq!(alt.x + p[0], 116.546_24, "the actor's pose and the shape's local one, composed");
    let q = boxes::px_normalized([0.023_362_182, -0.291_294_8, 0.011_831_824, 0.956_274_8]);
    assert_eq!([q.x, q.y, q.z, q.w], [0.023_362_184, -0.291_294_84, 0.011_831_826, 0.956_274_9]);
    // An ancestor that rotates or scales isn't covered.
    assert!(chain[0].is_translation() && !chain[1].is_translation());
}

#[test]
fn a_shape_the_pruner_may_skip_fails_closed() {
    // From 1e8 m up, the ray ending 8 m above the square: the mesh's raycast, rounding at that
    // scale, can report a hit the player's pruner never asks for (its segment stops short).
    let w =
        World { shapes: vec![recorded_square("square", [0.0, 0.0, 0.0], 0)], scenes: Vec::new() };
    let scene = Scene::new(&w);
    let r = scene.raycast_down([1.119_281_8, 1.0e8, 1.425_916_6], 99_999_992.0, 1);
    assert!(!matches!(r, Ok(Some(_))), "{r:?}");
    // Near by, the same square is surely tested.
    assert!(scene.raycast_down([1.1, 10.0, 1.4], 100.0, 1).unwrap().is_some());
    // An 8 m square 1e8 m out in its own space, translated back: PhysX's bounds keep the
    // operands' rounding (about ±4 m), so a ray 2 m inside the f64 bounds isn't surely tested.
    let mut far = recorded_square("far", [-1.0e8, 0.0, 0.0], 0);
    if let ShapeKind::Mesh { mesh, .. } = &mut far.kind {
        mesh.vertices = vec![
            [1.0e8, 0.0, 0.0],
            [1.0e8, 0.0, 8.0],
            [1.0e8 + 8.0, 0.0, 0.0],
            [1.0e8 + 8.0, 0.0, 8.0],
        ];
        mesh.geom_epsilon = 1.0e8 * 2f32.powi(-22);
        mesh.tree.center = [1.0e8 + 4.0, 0.0, 4.0];
        mesh.tree.extents_magnitude = 32f32.sqrt();
    }
    let w = World { shapes: vec![far], scenes: Vec::new() };
    let scene = Scene::new(&w);
    let r = scene.raycast_down([6.0, 10.0, 2.0], 100.0, 1);
    assert!(!matches!(r, Ok(Some(_))), "{r:?}");
    // Another shape 1e8 m out (on another layer): the pruner's node over both rounds at that
    // scale and can cut the square off.
    let mut w =
        World { shapes: vec![recorded_square("near", [-2.0, 0.0, 0.0], 0)], scenes: Vec::new() };
    w.shapes.push(recorded_square("far", [1.0e8, 0.0, 0.0], 5));
    let scene = Scene::new(&w);
    let r = scene.raycast_down([-1.0, 10.0, 1.4], 100.0, 1);
    assert!(!matches!(r, Ok(Some(_))), "{r:?}");
}

#[test]
fn every_order_and_zero_sign_is_tried() {
    let mut v = vec![0, 1, 2, 3];
    let mut seen = std::collections::HashSet::new();
    loop {
        seen.insert(v.clone());
        if !query::next_permutation(&mut v) {
            break;
        }
    }
    assert_eq!((seen.len(), v), (24, vec![0, 1, 2, 3]));
    let poses = query::zero_sign_variants([1.0, 0.0, 2.0], [0.0, 0.6, 0.0, 0.8]);
    assert_eq!(poses.len(), 8);
    let signs: std::collections::HashSet<[u32; 3]> =
        poses.iter().map(|p| [p.q.x.to_bits(), p.q.z.to_bits(), p.p.y.to_bits()]).collect();
    assert_eq!(signs.len(), 8);
    assert!(poses.iter().all(|p| p.q.y == 0.6 && p.q.w == 0.8 && p.p.x == 1.0 && p.p.z == 2.0));
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

/// Every run of the captures named in `WR_PHYS_CAPTURES` (comma-separated labels; default
/// `poc5-tb-103`): each fixture's `*.trace.jsonl.gz`.
fn captures() -> Vec<PathBuf> {
    let labels = std::env::var("WR_PHYS_CAPTURES").unwrap_or_else(|_| "poc5-tb-103".into());
    let mut out = Vec::new();
    for label in labels.split(',') {
        let root =
            wr_paths::repo_root().join("fixtures/captures/oracle").join(label).join("fixtures");
        let mut fixtures: Vec<PathBuf> = fs::read_dir(&root)
            .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
            .map(|e| e.unwrap().path())
            .collect();
        fixtures.sort();
        for fx in fixtures {
            for run in ["record", "replay-1", "replay-2"] {
                let p = fx.join(format!("{run}.trace.jsonl.gz"));
                if p.exists() {
                    out.push(p);
                }
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

#[derive(Default, Debug, Clone, Copy)]
struct Tally {
    /// `ag_phys` queries, of them disc casts, and `ph_probe` rays.
    queries: usize,
    discs: usize,
    probes: usize,
    /// Of all, those a mesh's conservative test can't rule out (what stream D's file answered
    /// `Unverified` for a mesh), and of those, mesh hits.
    near_mesh: usize,
    mesh_hits: usize,
    /// Likewise for boxes (what the file answered `Unverified` for a box before their recorded
    /// transforms), and box hits.
    near_box: usize,
    box_hits: usize,
    exact: usize,
    point_exact: usize,
    collider_exact: usize,
    near: usize,
    wrong: usize,
    /// Failed closed: a collider not verified (a box, ...), something not modelled.
    unverified: usize,
    unsupported: usize,
    /// Records whose recorded collider is a mesh, and of those computed exactly.
    mesh_records: usize,
    mesh_records_exact: usize,
    /// Records whose recorded collider is a box, and of those computed exactly.
    box_records: usize,
    box_records_exact: usize,
}

impl std::ops::AddAssign for Tally {
    fn add_assign(&mut self, o: Self) {
        self.queries += o.queries;
        self.discs += o.discs;
        self.probes += o.probes;
        self.near_mesh += o.near_mesh;
        self.mesh_hits += o.mesh_hits;
        self.near_box += o.near_box;
        self.box_hits += o.box_hits;
        self.box_records += o.box_records;
        self.box_records_exact += o.box_records_exact;
        self.exact += o.exact;
        self.point_exact += o.point_exact;
        self.collider_exact += o.collider_exact;
        self.near += o.near;
        self.wrong += o.wrong;
        self.unverified += o.unverified;
        self.unsupported += o.unsupported;
        self.mesh_records += o.mesh_records;
        self.mesh_records_exact += o.mesh_records_exact;
    }
}

/// The world without the meshes' recorded transforms (`boxes`: the boxes' too): their queries
/// go to the conservative test.
fn conservative_world(w: &World, boxes: bool) -> World {
    let mut w = w.clone();
    for s in &mut w.shapes {
        match &mut s.kind {
            ShapeKind::Mesh { runtime, .. } => *runtime = None,
            ShapeKind::Box { runtime, .. } if boxes => *runtime = None,
            _ => {}
        }
    }
    w
}

/// The record's collider (`col`: scene, path, sibling path, type) is the shape's.
fn same_collider(r: &serde_json::Value, s: &Shape) -> bool {
    let c = &r["col"];
    let name = format!(
        "{}:{} ({})",
        c["scene"].as_str().unwrap_or(""),
        c["path"].as_str().unwrap_or(""),
        c["type"].as_str().unwrap_or("")
    );
    let sib: Option<Vec<u32>> = c["sib"]
        .as_array()
        .map(|a| a.iter().map(|x| x.as_u64().map_or(u32::MAX, |x| x as u32)).collect());
    name == s.name && sib.as_deref() == Some(s.sibling_path.as_slice())
}

#[test]
#[ignore = "needs the captures and the area's physics file (wr-import phys-pack)"]
fn every_recorded_query_is_computed_exactly() {
    let t0 = std::time::Instant::now();
    let file = PhysFile::read(&path_for("bad465e5404604346bdf71ef50b3299a")).unwrap();
    let scene = Scene::new(&file.world);
    let old = conservative_world(&file.world, false);
    let old_scene = Scene::new(&old);
    let older = conservative_world(&file.world, true);
    let older_scene = Scene::new(&older);
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
    // The sim's queries (`ag_phys`) and the probes (`ph_probe`: a check of the colliders, not
    // a query the sim makes, so failing closed there isn't a failure).
    let (mut total, mut total_probes) = (Tally::default(), Tally::default());
    let mut failures = Vec::new();
    let mut samples = Vec::new();
    let mut failed_closed = std::collections::BTreeMap::<String, usize>::new();
    for path in captures() {
        let gz = fs::File::open(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let (mut tq, mut tp) = (Tally::default(), Tally::default());
        for line in std::io::BufReader::new(flate2::read::GzDecoder::new(gz)).lines() {
            let line = line.unwrap();
            if !line.contains("\"ag_phys\"") && !line.contains("\"ph_probe\"") {
                continue;
            }
            let r: serde_json::Value = serde_json::from_str(&line).unwrap();
            let probe = r["ev"] == "ph_probe";
            if r["ev"] != "ag_phys" && !probe {
                continue;
            }
            let t = if probe { &mut tp } else { &mut tq };
            let o = v3(&r["o"]);
            // A probe: `Physics.Raycast(o, Vector3.down, out h, 100f, WalkableMask)`.
            let (kind, max, mask) = if probe {
                ("ray", 100.0, 2_099_457)
            } else {
                let k = r["fn"].as_str().unwrap_or("?");
                (k, f(&r["max"]), r["mask"].as_i64().unwrap() as i32)
            };
            if probe {
                t.probes += 1;
            } else {
                t.queries += 1;
                t.discs += usize::from(kind == "disc");
            }
            distinct.insert((kind.to_owned(), o.map(f32::to_bits)));
            let run = |s: &Scene| match kind {
                "ray" => {
                    if !probe {
                        assert_eq!(v3(&r["d"]), [0.0, -1.0, 0.0], "a ray not straight down: {r}");
                    }
                    s.raycast_down(o, max, mask)
                }
                "disc" => s.heuristic_disc_cast_down(o, f(&r["r"]), max, mask),
                other => panic!("unknown query {other:?}"),
            };
            let got = run(&scene);
            let near_mesh = matches!(
                run(&old_scene),
                Err(QueryError::Unverified { shape }) if shape.ends_with("(MeshCollider)")
            );
            t.near_mesh += usize::from(near_mesh);
            let near_box = matches!(
                run(&older_scene),
                Err(QueryError::Unverified { shape }) if shape.ends_with("(BoxCollider)")
            );
            t.near_box += usize::from(near_box);
            let want = r["hit"].as_bool().unwrap().then(|| (f(&r["dist"]), v3(&r["pt"])));
            let what = format!(
                "{}/{} t={} {} {kind} from {o:?}",
                path.parent().unwrap().file_name().unwrap().to_string_lossy(),
                path.file_name().unwrap().to_string_lossy(),
                r["t"],
                r["ev"],
            );
            let mesh_record = r["col"]["type"] == "MeshCollider";
            let box_record = r["col"]["type"] == "BoxCollider";
            t.mesh_records += usize::from(mesh_record);
            t.box_records += usize::from(box_record);
            if let Ok(Some(h)) = &got
                && want.is_some_and(|(d, pt)| {
                    h.distance.to_bits() == d.to_bits()
                        && h.point.map(f32::to_bits) == pt.map(f32::to_bits)
                        && same_collider(&r, &file.world.shapes[h.shape])
                })
            {
                t.mesh_records_exact += usize::from(mesh_record);
                t.box_records_exact += usize::from(box_record);
            }
            match (got, want) {
                (Err(e), _) => {
                    match e {
                        QueryError::Unverified { .. } => t.unverified += 1,
                        QueryError::Unsupported(_) => t.unsupported += 1,
                    }
                    let class = match &e {
                        QueryError::Unverified { shape } => {
                            format!("unverified {}", shape.rsplit(' ').next().unwrap_or(""))
                        }
                        QueryError::Unsupported(s) => s.split(" (").next().unwrap_or("").into(),
                    };
                    let source = if probe {
                        "ph_probe".to_owned()
                    } else {
                        format!("ag_phys {}", r["ctx"].as_str().unwrap_or("?"))
                    };
                    *failed_closed.entry(format!("{source}: {class}")).or_default() += 1;
                    // A probe is a check of the colliders, not a query the sim makes; neither
                    // is presentation code's query (a critter's, `other`), which the driven
                    // replay takes from the recording when it fails closed.
                    let presentation = matches!(r["ctx"].as_str(), Some("critter" | "other"));
                    if probe || presentation {
                        if samples.len() < 12 {
                            samples.push(format!("{what}: {e}"));
                        }
                    } else {
                        failures.push(format!("{what}: {e}"));
                    }
                }
                (Ok(None), None) => {
                    t.exact += 1;
                    t.point_exact += 1;
                    t.collider_exact += 1;
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
                    let shape = &file.world.shapes[h.shape];
                    t.mesh_hits += usize::from(matches!(shape.kind, ShapeKind::Mesh { .. }));
                    t.box_hits += usize::from(matches!(shape.kind, ShapeKind::Box { .. }));
                    // Records before `col` (`poc5-tb-100` to `-105`) don't name the collider.
                    if r.get("col").is_none() || same_collider(&r, shape) {
                        t.collider_exact += 1;
                    } else {
                        failures.push(format!("{what}: {} vs the game's {}", shape.name, r["col"]));
                    }
                }
                (got, want) => {
                    t.wrong += 1;
                    failures.push(format!("{what}: {got:?} vs the game's {want:?}"));
                }
            }
        }
        eprintln!("{}: queries {tq:?}", path.display());
        if tp.probes > 0 {
            eprintln!("    probes {tp:?}");
        }
        total += tq;
        total_probes += tp;
    }
    eprintln!("total: {total:?}; {} distinct queries", distinct.len());
    eprintln!("probes: {total_probes:?}");
    eprintln!("failed closed: {failed_closed:#?}");
    for x in &samples {
        eprintln!("  (failed closed) {x}");
    }
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

#[test]
fn what_the_recordings_dont_cover_isnt_computed_or_accepted() {
    // A box whose own rotation or scale isn't the recorded one (changed at load): its centre
    // computed from the scene wouldn't be the game's.
    let b = recorded_box("b", [2.0, 2.5, 2.0], [1.0, 0.5, 0.0]);
    let ShapeKind::Box { center, size, chain, runtime: Some(r), .. } = &b.kind else {
        unreachable!()
    };
    assert_eq!(r.established(chain, *center, *size), Ok(()));
    let mut scaled = r.clone();
    scaled.lossy_scale = [2.0, 1.0, 1.0];
    assert!(scaled.established(chain, *center, *size).is_err());
    let mut turned = r.clone();
    turned.rotation = [0.0, 0.6, 0.0, 0.8];
    assert!(turned.established(chain, *center, *size).is_err());
    let mut negative = r.clone();
    negative.lossy_scale = [-1.0, 1.0, 1.0];
    assert!(negative.established(chain, *center, *size).is_err());
    // A box without a recorded transform: the recording must agree with the scene's layer.
    let mut plain = b.clone();
    if let ShapeKind::Box { runtime, .. } = &mut plain.kind {
        *runtime = None;
    }
    plain.name = "S:b (BoxCollider)".into();
    let w = World { shapes: vec![plain], scenes: vec!["S".into(), "T".into()] };
    let rec = RecordedCollider {
        name: "S:b (BoxCollider)".into(),
        sibling_path: Vec::new(),
        present: true,
        layer: 8,
        position: [2.0, 2.5, 2.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        lossy_scale: [1.0; 3],
        trigger: false,
        kind: RecordedKind::Box { center: [1.0, 0.5, 0.0], size: [2.0, 1.0, 4.0] },
    };
    assert_eq!(w.check_recorded_colliders(std::slice::from_ref(&rec)), Ok(()));
    let moved = RecordedCollider { layer: 0, ..rec.clone() };
    assert!(w.check_recorded_colliders(&[moved]).is_err(), "another layer");
    // A scene whose every collider was left out still has its recordings checked.
    let other = RecordedCollider { name: "T:c (BoxCollider)".into(), ..rec.clone() };
    assert!(w.check_recorded_colliders(&[rec, other]).is_err(), "a collider the file lacks");
}
