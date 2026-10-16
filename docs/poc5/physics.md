# POC-5: physics queries (movement step 4)

Status (2026-10-15, stream D): **every recorded physics query is computed bit for bit**: 57,321 `ag_phys` records
(10,699 distinct; 408 disc casts) in all 36 runs of `poc5-tb-100` to `-105`, distance and hit point both exact.
The terrain's raycast is a port of PhysX 4.1's `raycast_heightField`, checked against the player's own function on
a million generated rays; boxes, meshes and the other colliders are imported, but a query that may touch one fails
closed (no recorded query comes near one). Tags as elsewhere: [M] measured, [I] inferred, [C] from the code
(decompiled C#, PhysX's source, or the player's machine code).

## What the game asks [C]

`UnitMovementAgentBase.Move` (turn-based: no navmesh clamp) sets the unit's height with
`Physics.Raycast(v + (0, 2, 0), Vector3.down, out hit, 100, 2099457)` and, when the unit would drop more than its
shift, `PhysicUtils.HeuristicDiscCastDown(v + (0, 0.65, 0), 0.4, out hit, 100, 2099457)`. Only `hit.distance` is
used. The mask is `Layers.WalkableMask`: layers 0 (Default), 8 (Ground), 11 (OptimizeShadow), 21 (GroundNoCamera).

- `HeuristicDiscCastDown` is managed (`Kingmaker.Utility/PhysicUtils.cs`): eight `Physics.Raycast`s straight down
  from `origin + point * radius`, keeping the first hit with the smallest distance. Its points come from a static
  constructor: four at `Mathf.Sin(3π/4)` and four at `Mathf.Sin(π/2)` from the centre, at angles 0, π/4, π/2, 3π/4
  accumulated in float (`Mathf.Sin`/`Cos` are libm's `sin`/`cos` in double, rounded to float).
- Unity's native `PhysicsScene.Internal_Raycast` (player `0x9d0314` → `0x6b92d8`, through the physics module's icall
  table at file offset `0x169a0f8`, 463 entries, functions right after the names): `maxDistance == +inf` becomes
  `FLT_MAX`; hit flags `0xb` (`ePOSITION | eNORMAL | eUV`), or `0x8b` (adding `eMESH_BOTH_SIDES`) when
  `Physics.queriesHitBackfaces`; a filter on the layer mask and on triggers (`QueryTriggerInteraction`, the global
  `queriesHitTriggers`); then `PxScene::raycast` (a virtual call) for the closest blocking hit, whose distance and
  position it copies unchanged into the `RaycastHit`. The managed side normalises the direction twice
  (`PhysicsScene.Raycast`, `Ray`); `(0, -1, 0)` stays itself.
- PhysX's scene query (`NpSceneQueries`' `MultiQueryCallback`) tests the shapes whose (inflated) bounds the ray
  crosses, each with the distance shrunk to the closest hit so far, and keeps a blocking hit only when its distance is
  `<=` that distance (at first `max`). With one shape hit, the answer doesn't depend on the order. The height field's
  own distance is recomputed from the rounded hit point, so it can exceed the traced length: on the caves,
  `raycast_down((128.67328, 62.99409, 67.102905), 21.423319)` gets 21.42332 from `raycast_heightField`, which the
  scene query drops (a regression in the record test).

## The colliders [M]

`wr-import phys-pack` reads the area's five scene files (`Prologue_Caves_1_*`): **121 colliders** on active
GameObjects (1 more is on an inactive one): 88 boxes, 27 non-convex meshes, 1 terrain, 2 capsules, 3 character
controllers. In the walkable mask: the terrain (layer 8), 44 boxes on layer 8 (`crusade_floor_01_Collider`,
`tower_wood_trash_*`), 15 on layer 0 (the market place's carts and tents, 8 `FoW Blockers/Cube`), and 24 meshes on
layers 0 and 11 (cave walls, columns, a cart). The rest are on layers 9 (critters' controllers) and 10 (interaction
colliders).

Against the game's own hierarchy at run time (`fixtures/captures/poc0/check-1/hierarchy`): 11 `*_!Visual` mesh
colliders are on layer 11 instead of the file's 0 (the load's layer move noted in `docs/poc0/extraction.md`, cause
not traced; both layers are in the walkable mask), and 24 layer-10 boxes of the mechanics scene are disabled (outside
the mask). The file keeps the scene's layer and flags a `*_!Visual` collider; a query whose mask holds one of 0 and 11
but not the other fails closed.

**What the rays hit:** all 57,321 recorded queries hit the terrain, and no box, mesh or other collider in the mask is
within reach of any of them (the query's own conservative test, below, would have failed closed). Before the exact
port, an `f64` search over `poc5-tb-103`'s first run found the terrain with the diagonal from (x, z) to (x + 1, z + 1)
within 5e-6 m of every recorded distance.

**Mesh colliders: what PhysX uses.** Every mesh collider's `Mesh` carries `m_BakedTriangleCollisionMesh` (Unity's
"prebake collision meshes"): a PhysX 4.1 cooked stream, `NXS`/`MESH` format 15, midphase 1 (BVH34), the colliders'
cooking options all 30 (`CookForFasterSimulation | EnableMeshCleaning | WeldColocatedVertices | UseFastMidphase`).
It differs from the render mesh: vertices are welded (e.g. `cave_wall_turn_angle_45_02` cooks 1,052 of the render
mesh's 2,179, every cooked vertex bit-identical to a render one), triangles are kept in number except where cleaning
drops degenerate ones (`camp_cart_01_broken`: 2,016 of 2,018). The importer decodes the cooked stream's vertices and
triangles (`phys::decode_cooked_mesh`); a mesh without baked data is refused (cooking isn't modelled). Whether Unity
loads the baked stream or re-cooks with the same options and PhysX build at load makes no difference to the
geometry [I: cooking is deterministic].

**The terrain** (`TerrainCollider`, `TerrainData` 2049² samples, `m_Scale` (0.09765625, 100, 0.09765625), no holes)
becomes, as the records establish it: a height field with rows along x and columns along z (`m_Heights` is z-major,
so transposed), every sample's tess flag set ("zeroth vertex shared"), materials 0, height scale `m_Scale.y / 32766`,
row and column scales `m_Scale.x` and `m_Scale.z`, the actor at the terrain's position (the origin) unrotated.
Sensitivity: without the tess flags 530 of a run's 1,000 rays still match (so the flag is established); the two
candidate expressions for the height scale (`y / 32766`, `y * (1 / 32766)`) give the same float here, so the importer
refuses a terrain where they differ; a terrain elsewhere than the origin, rotated or scaled, or with holes is refused
too (not established), as is one with trees while tree colliders are on (Unity adds the tree prototypes' colliders
to the terrain's actor; the caves have none).

## The port [C, M]

`crates/wr-phys` (its own crate: the physics world is separate data with its own sources and consumers, the agent's
`Move` now and the place-on-ground step and other physics users later; `wr-nav` stays the A* graphs):

- `px`: PhysX's foundation math in `f32`, each operation in the source's order (`PxVec3`, `PxQuat::rotate`/
  `rotateInv`, `PxTransform`, `selectMin`/`selectMax`/`sign`, `rayAABBIntersect2`, the scalar `intersectRayAABB2`,
  Möller–Trumbore `intersectRayTriangle`). The player's PhysX is built without contraction: `raycast_heightField`
  (`0x1326074`, ~1,000 instructions), its out-of-line `faceHit` (`0x133085c`) and `computeLocalBounds`
  (`0x1378f78`) contain no fused multiply-add, so plain Rust `f32` (which never contracts) reproduces them.
- `hf`: `raycast_heightField` with `HeightFieldTraceUtil::traceSegment` (the DDA over cells, both triangles per
  cell with the tess flag's rotation, holes, `enlarge` 1e-4, the 1.01 bounds inflation and the 10 m ray-start
  offset) and `faceHit` (distance `max(0, (p - origin) · dir)` in shape space, the point through the pose).
- `boxes`: `raycast_box`. Not used by the queries yet (below).
- `query::Scene`: `raycast_down(origin, max, mask)` and `heuristic_disc_cast_down(origin, radius, max, mask)`, as
  the game calls them.

**Found in the player** (no symbols; `objdump -d` of `UnityPlayer.dylib`): `Gu::getRaycastFuncTable` returns
`gRaycastMap` at `0x17214c0` (sphere `0x1325210`, plane `0x1325680`, capsule `0x132531c`, box `0x1324e48`, convex
`0x132579c`, triangle mesh `0x1326018`, the unregistered height field stub `0x1327074`, recognisable by its error
string); the registered `raycast_heightField` is the function before the stub. `Gu::HeightField` keeps `mData` at
0x20 (the `CenterExtents` bounds, `rows` 0x38, `columns` 0x3c, `samples` 0x50).

**Checked against the player** (`native_check.rs`, ignored tests that `dlopen` the oracle build's player and call
its functions with objects laid out as they read them):

| Function | Inputs | Result |
|---|---|---|
| `raycast_heightField` | 1,000,000 rays on 4,000 generated height fields (2–11 × 2–11 samples, random tess flags, holes, PhysX's own collision-vertex bit, negative row/column scales, double-sided), random poses, rays on grid lines and vertices, straight down (81,357 hits), grazing, any direction; distances 0–10 m, 100 m, up to 1e7 | **bit-identical**: hit or not, distance, point, triangle index (152,838 hits) |
| `raycast_box` | 1,000,000 rays on random boxes and poses, from inside and outside | **bit-identical** distance and point (452,816 hits) |

The height field check catches mutations: dropping the triangle test's `enlarge` changes 11,798 of the million,
dropping the cell's height-range epsilon 27.

**Checked against the recordings** (`tests::every_recorded_query_is_computed_exactly`, ignored: needs the captures and
the area's physics file):

| Captures | Runs | Queries (discs) | Distance exact | Point exact | Wrong / failed closed |
|---|---:|---:|---:|---:|---:|
| `poc5-tb-103` | 6 | 6,351 | 6,351 | 6,351 | 0 / 0 |
| `poc5-tb-100` to `-105` | 36 | 57,321 (408) | 57,321 | 57,321 | 0 / 0 |

(Records and replays repeat the same queries: 10,699 distinct.) Shortening the ray (`max` 50, 7, 2.5) changes none of
these vertical results: the origins lie inside the field's inflated bounds, so the trace starts at the origin.

## Failing closed

`Scene` answers only what is established; otherwise a `QueryError` the caller reports:

- `Unverified { shape }`: the ray may touch a box, mesh or other collider within `[0, max]`. The test is in `f64` on
  the collider's geometry under its `localToWorldMatrix`, with a slack along the ray and around the edges of 1 mm plus
  a millionth of the coordinates' and distance's magnitude (`f32` rounding in PhysX: a box 1 cm beyond a 1,000 km ray
  rounds into it) and of the game's own `f32` placement of the shape (a millionth of the translation's and each scaled
  local coordinate's magnitude, which cancellation can leave far larger than the world coordinate), plus, for a mesh's
  triangles, BV4's barycentric enlargement (`mGeomEpsilon`, 2^-22 of the largest local coordinate, as cooking computes
  it) and a float-error allowance (the slack and 1e-5 of the edges, divided by the conditioning below) that grows as
  the ray turns parallel to the triangle and as the triangle thins (where the two together pass a thousandth, or the
  triangle is degenerate in `f64`, PhysX's `f32` cancellation can place a hit anywhere its midphase reaches, so the
  shape's own grown bounds decide: a mesh's grown by every triangle's allowance and a hundredth of its extent, the
  pruner's 1.01 inflation and BV4's quantized nodes). The record test runs its 57,321 queries in a few seconds
  (release). The native box check asserts this test counts every hit the player's `raycast_box` reports (a million
  rays, grazing ones and rays ending at the hit included). Spheres, capsules and controllers count as the sphere of
  radius `(r + h/2)` times the largest scale, convex meshes as their grown bounds. The importer refuses a sheared
  transform (a rotated child of a non-uniformly scaled parent: its matrix-transformed shape needn't enclose what Unity
  hands PhysX); none occurs here. PhysX's per-shape box raycast is ported and exact, but how Unity turns a
  `BoxCollider` into a `PxBoxGeometry` and shape pose (centre, lossy scale, its tolerance for "unscaled") isn't
  verified, and the BVH34 mesh raycast (`BV4_RaycastSingle`'s `RayTriOverlapT` and its transforms) isn't ported.
- `Unsupported`: a trigger collider in the mask (`queriesHitTriggers`), the `*_!Visual` layer ambiguity above, more
  than one height field in the mask (the scene query's order shrinks the distance one passes the next, which can turn
  a miss into a closer hit), a non-finite origin or negative distance.

## The file

`wr-import phys-pack --area prologue_caves_1 --guid bad465e5404604346bdf71ef50b3299a` writes
`cache/sim/phys/<guid>.wrphys` (header as the nav file's: magic, version, length, FNV-1a; postcard payload; the
height samples packed, materials stored once when uniform): 8.8 MB, read and prepared in 27 ms (release). It isn't
in `scripts/scene-cache.sh` yet (other areas may hit the refusals above).

## For the sim

**Done** (stream M, `docs/poc5/movement.md`, "Physics and paths computed"): in the driven replay the agent's
`AgentPhysics` and the movement simulation's queries are `Scene::raycast_down` and `Scene::heuristic_disc_cast_down`
on the area's file (`Scene::owned` keeps it), every recorded `ag_phys` a check; a `QueryError` is reported and the
recorded answer taken.

## Not modelled

- Colliders that aren't in the static scenes: units (layer 9, outside the mask), spawned objects, and map objects
  whose colliders change state; a runtime collider on a walkable layer would be missed. `ph_col` (below) lists them.
- Boxes and meshes (above), capsules, spheres, controllers, convex meshes.
- The terrain's pose other than the origin, holes, the geometry's double-sided flag (no effect on rays from above).
- The `*_!Visual` layer move's rule.

## Observer changes requested

Unbuilt, for the main session to build and record (`oracle/mods/WrathOracle/`):

1. **`ag_phys` `col`** (`CommandObsAgent.cs`): the collider hit (scene, hierarchy path, sibling path, type, instance
   id), so each record names what it hit; the sim's check can then match the shape too.
2. **Observer `phys_probe`** (new `PhysProbe.cs`, run from `Fixture.cs` right after the begin record when declared;
   reads only): `ph_settings` (`queriesHitTriggers`, `queriesHitBackfaces`, `autoSyncTransforms`), `ph_col` (every
   collider in the loaded scenes, inactive ones included: identity, layer, activity, enabled, trigger, bounds, the
   transform's position, rotation, lossy scale and matrix, and per type the box's centre and size, the mesh's name,
   convexity and cooking options, the terrain's data, size and resolution, the capsule's or controller's dimensions),
   and `ph_probe`: rays straight down (100 m, the walkable mask) over a 9 × 9 grid on each walkable box and mesh
   collider's bounds, with the hit's distance, point and collider. That gives boxes and meshes the check the terrain
   has (Unity's box pose and scale handling, the BVH34 raycast), the run-time layers and enabled flags, and the
   global query settings the file now assumes. The probe's raycasts are reads, but with `autoSyncTransforms` on (the
   begin record's `physics_auto_sync`) each query first syncs pending transforms into PhysX, as the agent's own
   queries do every tick; the recording should confirm the replays stay tick-exact with it.

## Reproduce

```sh
eval "$(scripts/worktree-setup.sh phys)"   # or the main checkout's cache
cargo run --release -p wr-import -- phys-pack --area prologue_caves_1 --guid bad465e5404604346bdf71ef50b3299a
WR_PHYS_CAPTURES=poc5-tb-100,poc5-tb-101,poc5-tb-102,poc5-tb-103,poc5-tb-104,poc5-tb-105 \
  cargo test --release -p wr-phys -- --include-ignored --nocapture
```
