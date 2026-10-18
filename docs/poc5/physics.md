# POC-5: physics queries (movement step 4)

Status (2026-10-17, stream boxes): **box colliders are computed too.** How Unity hands a `BoxCollider` to PhysX is
read from the player and checked against every probe: the static actor's pose is `TransformPoint(center)` (Unity's
own hierarchy walk, ported and checked against the player on a million calls) with the GameObject's rotation, which
PhysX normalises, and the half extents `max(|lossyScale * size * 0.5|, FLT_EPSILON)` ("Boxes", below). Unity's
raycast also drops every hit at a distance not above zero (its post-filter). With the boxes' rotation and lossy
scale recorded (`ph_col`), all 176,106 probe hits on a box are computed exactly where the answer doesn't hang on the
pruner (169,050 before the tolerance below, 175,224 after), none wrong; of the 393,802 recorded `ag_phys` queries of
every current capture, 25 fail closed (presentation code's, on two floor tiles in one plane or the pruner's
culling; 840 of the table's captures failed closed on a box before), none wrong.

Status (2026-10-16, stream S): **triangle meshes are computed too.** PhysX 4.1's BVH34 mesh raycast is ported from
the player's NEON build and equals its own function on 26 million generated rays (12 million hits); with each
mesh's transform recorded (`phys_probe`'s `ph_col`), every recorded query of `poc5-tb-121`, `-128` and `-129` that
came near a mesh (10,916 over their nine runs) is computed exactly, `-129` replays valid on every run, and the only
queries left failing closed are presentation code's near a box ("Triangle meshes", below).

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
- Unity's query filter (`0x6bbcd0`/`0x6bbdac`, the filter object's vtable at `0x1683860`; the query's filter flags
  `0xf`, `ePREFILTER | ePOSTFILTER` included): the pre-filter is the layer mask and the trigger rule; the post-filter
  keeps a hit only when its distance is above zero (`fcmp` then `b.le`: zero and NaN dropped), so a ray that starts
  inside a box, or on a surface, doesn't see it (stream boxes; `Physics.Raycast`'s documented behaviour).
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
drops degenerate ones (`camp_cart_01_broken`: 2,016 of 2,018). The importer decodes the whole cooked stream as
`loadMeshData` and `BV4Tree::load` read it (`phys::decode_cooked_mesh`: vertices, triangles in the tree's order,
materials, face remap and adjacencies read past, the BV4 tree (version 3, quantized: local bounds, root data,
dequantization coefficients, the packed nodes), the cooked `mGeomEpsilon` (2^-22 of the largest coordinate in all 27,
as stream D's conservative test assumed), the bounds and extra triangle data, nothing left over); a mesh without baked
data, with the BVH33 midphase or GPU data is refused (cooking isn't modelled). That the player uses the baked stream
rather than re-cooking (the tree's shape decides ties and node culling) is [I]: the colliders' options are the baked
ones, and every recorded mesh query agrees with the baked tree (below).

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
- `boxes`: `raycast_box`, Unity's `TransformPoint` and the box's pose and geometry ("Boxes", below).
- `bv4` (stream S): the triangle mesh, below.
- `query::Scene`: `raycast_down(origin, max, mask)` and `heuristic_disc_cast_down(origin, radius, max, mask)`, as
  the game calls them.

## Triangle meshes (stream S) [C, M]

**The port** (`bv4.rs`). The public PhysX 4.1 has BV4 for SSE only (elsewhere its midphase table holds an
"unsupported" stub); the player's is a NEON build of it, so the port follows the machine code, the source a guide:

- `raycast_triangle_mesh` = `Gu::raycast_triangleMesh_BV4` (`0x13e68bc`, reached from `gRaycastMap[eTRIANGLEMESH]`
  `0x1326018` through the midphase table at `0x16ef948` by the mesh's concrete type), one hit. An identity
  `PxMeshScale` (all three scales exactly 1) takes `raycastVsMesh`: `setupWorldMatrix` (none when the pose's bits are
  the identity's, else `PxMat44(q)` and the translation) and the ray moved into mesh space by `inverseRotate`/
  `inverseTransform`; any other scale goes to vertex space with `scale.getInverse() * pose.getInverse()` (a
  `Matrix34`, `0x1325dd0`), the direction normalised, `maxDist * |d| + 1e-3`, then the distance times `1/|d|` and the
  point through `pose.transform(scale.transform(p))` (`0x13e7230`). Flags: `eMESH_BOTH_SIDES` or the geometry's
  `eDOUBLE_SIDED` turn culling off; `eMESH_ANY` isn't ported (Unity's closest-hit query doesn't set it).
- `raycast_single` = `BV4_RaycastSingle` (`0x13e2420`): `clipRay` against the tree's bounding sphere, then
  `BV4_ProcessStreamKajiyaOrderedQ`: per swizzled node, the four children's quantized boxes (min in the low 16 bits,
  max in the high, times the coefficients) slab-tested against the ray at once; the slab reciprocal is NEON's
  `frecpe` estimate refined by `frecps` (a fused `2 - a*b`) and one more separate Newton step, all ported bit for bit
  (`frecpe` per the ARM ARM's `FPRecipEstimate`; an exhaustive test against the hardware covers every `f32`); `fmin`/
  `fmax` with ARM's NaN and signed-zero rules; children tested 3, 2, 1, 0, leaves at once (the closest distance cuts
  the later boxes of the next node), inner children pushed in the precomputed node-sort order for the ray's octant.
  The tree's absence (under 16 triangles) is the brute-force leaf over all.
- The leaf (`0x13e3b0c`): `RayTriOverlapT` with back-face culling (det `< FLT_EPSILON²` rejects; the barycentric
  limits enlarged by `det * mGeomEpsilon`; the division last) or without; the comparisons as the machine code's flag
  chains (`fcmp`/`fccmp`, NaN included); a hit kept when strictly closer, so the first of equal distances stays.
- `computeImpactData`: `(P1 u + P2 v) + P0 (1 - u - v)`, through the world matrix when there is one.

No fused multiply-add anywhere but `frecps` (whose fusion is the instruction's definition). `Bv4Mesh::validate`
refuses what the player would read out of bounds: child offsets past the nodes, a node reached twice, a stack past
the player's 256 entries, empty leaves or triangles past the end.

**What Unity hands PhysX** [M]: a static actor at the GameObject's `Transform.position` and `.rotation`, the shape's
local pose the identity, and the geometry's `PxMeshScale` its `lossyScale` with an identity rotation, single-sided;
the query's flags `ePOSITION | eNORMAL | eUV`. `phys_probe`'s probes establish it: of `poc5-tb-129`'s 1,136 probe
rays that hit a mesh, all 1,136 have the port's distance and point with the recorded pose and lossy scale; with an
identity scale only 668 distances (607 points) agree, with the importer's own lossy scale 1,130 (it differs from
Unity's for `camp_cart_01_broken`, whose y scale Unity computes as 1.00000012). The cave walls' lossy scales are a
few ulps off one (1.00000024), so their queries take the scaled path.

**The transforms are recorded inputs.** The importer's world transforms aren't Unity's to the bit (`glam`'s
composition; 35% of the area's GameObjects differ, `docs/poc0/extraction.md`), and Unity's `lossyScale` isn't
ported. `wr-import phys-pack --colliders <trace>` gives each mesh the position, rotation, lossy scale and run-time
layer of its `ph_col` record (matched by scene, path, sibling path and type; active, not a trigger, non-convex,
cooking options 30, and within 1e-4 of the importer's transform, which it equals bit for bit for the 25 meshes'
positions and rotations and 24 of their scales); a mesh recorded disabled (a loot corpse's) is left out, one recorded
with a zero scale (another corpse's) keeps no recorded transform. The recording writes a zero without its sign, so
a query is computed under every sign of the recorded pose's zero components and fails closed if they disagree. The
driven replay checks a recording's `ph_col` against the file (each recorded mesh recorded once, present, with its
layer, transform, trigger flag, non-convex, its cooking options; every present mesh collider of the area's scenes in
the file) and reports a difference.

**The scene query.** PhysX tests the shapes in its pruner's order (not modelled), each with the distance shrunk to
the closest hit so far, keeping a hit when `distance <= shrunk`. `Scene::raycast_down` runs every order of the
shapes that can hit: the height field (a shorter ray can turn its miss into a hit) and each recorded mesh that hits
the whole ray (a mesh's raycast only loses hits as the ray shortens: node culling and the leaf's test only tighten).
A shape the pruner may skip (the ray not surely across its bounds by a tolerance of 2e-6 of the coordinates involved,
the operands PhysX's own bounds round from, and the scene's extent, at which its tree nodes round; 1e-5 until stream
boxes, below) is tried both tested and skipped. All outcomes must agree, distance, point and shape, or the query fails closed (`Unsupported`):
two shapes hit at the same distance, for instance.

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

| `raycast_triangleMesh` (stream S) | 26,000,000 rays, a million on each of the area's 26 mesh colliders (their cooked BV4 meshes laid out as `BV4TriangleMesh`): the collider's pose, a random one, its zeros' signs flipped, the identity; an identity, the recorded lossy, a near-one (±4 ulps), or an arbitrary scale (0.3–3, negative, rotated); at a point on a triangle, its edge or vertex, just outside it, or anywhere in the bounds; straight down, against the normal, grazing, any direction; from on the surface, behind, near, up to 100 m or 1e5–1e9 m away; max 0–200 m, 100 m, 1e7, `FLT_MAX` or exactly the hit; single- and double-sided, `eMESH_BOTH_SIDES` | **bit-identical**: hit or not, distance, point, face index, u, v (12,000,754 hits) |
| `frecpe`, `frecps` | every `f32` (`frecpe`), 10,000,000 pairs (`frecps`), against the CPU's own instructions | **bit-identical** |

The height field check catches mutations: dropping the triangle test's `enlarge` changes 11,798 of the million,
dropping the cell's height-range epsilon 27. The mesh check catches each of ten (`the_mesh_check_catches_errors`,
100,000 rays per collider): of 2,600,000 rays, the reciprocal frecpe's raw estimate changes 35,923, an exact
reciprocal 4,532, no barycentric enlargement 75,004, a later triangle winning a tie 193,474, children pushed in
reverse 9,094, a fused determinant 246,551, no `clipRay` 412 (far rays), nodes beyond the closest hit not culled
1,473, the scaled path without its 1e-3 147,058, an identity scale through the scaled path 557,420.

**Checked against the recordings** (`tests::every_recorded_query_is_computed_exactly`, ignored: needs the captures and
the area's physics file):

| Captures | Runs | Queries (discs) | Distance exact | Point exact | Wrong / failed closed |
|---|---:|---:|---:|---:|---:|
| `poc5-tb-103` | 6 | 6,351 | 6,351 | 6,351 | 0 / 0 |
| `poc5-tb-100` to `-105` | 36 | 57,321 (408) | 57,321 | 57,321 | 0 / 0 |

(Records and replays repeat the same queries: 10,699 distinct.) Shortening the ray (`max` 50, 7, 2.5) changes none of
these vertical results: the origins lie inside the field's inflated bounds, so the trace starts at the origin.

With the meshes (stream S; the test now covers every run of the listed captures, checks each hit's collider and
sibling path, and runs `phys_probe`'s probes too; "near a mesh" is a query stream D's conservative test answered
`Unverified` for a mesh):

| Captures | Runs | Queries (near a mesh) | Exact (distance, point, collider) | Wrong | Failed closed |
|---|---:|---:|---:|---:|---:|
| `poc5-tb-121` (tb-lizard) | 3 | 26,669 (829, 841, 868) | 26,388 | 0 | 281 (`other` rays near a box) |
| `poc5-tb-128` (tb-lizard) | 3 | 30,327 (1,267, 1,230, 1,252) | 30,042 | 0 | 285 (critters' rays near a box) |
| `poc5-tb-129` (tb-lizard) | 3 | 32,359 (1,511, 1,581, 1,537) | 32,085 | 0 | 274 (critters' rays near a box) |
| `-95`, `-96`, `-100` to `-105`, `-109`, `-111` to `-118`, `-120` to `-129` | 129 | 279,186 (10,916) | 278,346 | 0 | 840 (as above) |
| their probes (`-112` onward) | 72 | 484,056 (81,792 hitting a mesh) | 176,184 (51,552 mesh hits) | 0 | 307,872 (301,968 near a box, 5,904 order or pruning) |

Every query near a mesh is computed: the mesh's own raycast agrees that it isn't hit (or hits beyond the terrain),
and none of the sim's queries hits one. The 840 left fail closed on a box (`crusade_floor_01_Collider`; boxes are
the open item above) and are presentation code's (critters', `ctx: "other"`), which the driven replay takes from the
recording without reporting. Of the probes that hit a mesh, those not failing closed on a nearby box or an
ambiguous order (two shapes within the pruner's tolerance) are exact, 51,552.

## Boxes (stream boxes) [C, M]

**What Unity hands PhysX for a static `BoxCollider`**, read from the arm64 player (`objdump -d` of
`UnityPlayer.dylib`; the functions found from the box warning string and the icall table) after the probes had
established it:

- **The pose** [C]: `0x6aac44` (which the centre's setter `0x6b5554` calls; `0x6b53cc` is `GetGlobalCenter`, the same
  `TransformPoint` alone) gets the GameObject's Transform, computes `Transform::TransformPoint(m_Center)` (`0x6744c0`)
  and `Transform.rotation` (`0x679474`), and passes them to the shape's actor's `setGlobalPose` (the shape's vtable
  `+0x90`, the actor's `+0xa0`). So the box's world centre is Unity's hierarchy walk, not the GameObject's position
  plus its rotated, lossy-scaled centre, and the shape's local pose is the identity [I: with any other the probes'
  last bits would differ]. PhysX normalises the pose (`PxQuat::getNormalized`, `1 / sqrt(((x² + y²) + z²) + w²)` times
  each component, as `NpRigidStatic::setGlobalPose` does) [M: below].
- **The geometry** [C]: `BoxCollider::Create` (`0x6b5404`) and `set_size` (`0x6b4db0`) take the half extents from
  `0x6b5280` with the lossy scale (`0x675afc`) and `(1, 1, 1)`: `((lossyScale * 1) * m_Size) * 0.5`, its absolute
  value (a negative one logs "BoxColliders does not support negative scale or size"), `fmaxnm` with `FLT_EPSILON`
  (`0x34000000`); then a `PxBoxGeometry` (type 3).
- **`TransformPoint`** [C, M]: per level from the GameObject up, `v = p * s`, then
  `t + ((v + c0 vx) + (c1 vy + c2 vz))` with `c0..c2` the rotation matrix less the identity, each entry a difference
  of two products (`qy (-2 qy) - qz (2 qz)`, ...: constants `(-2, 2, -2)`, `(2, -2, -2)`, `(-2, -2, 2)` times one
  component), no fused multiply-add; `boxes::transform_point`. `transform_point(chain, 0)` is `Transform.position`:
  it reproduces all 31,518 objects of `check-1`'s hierarchy dump in the static and UI scenes bit for bit, 28,848 of
  them under a rotated or scaled ancestor (only `BaseMechanics`' animated units differ, 1,419 of its 4,335).

**How the probes decided it** [M]. `poc5-tb-129`'s record has 1,797 probe hits on a box (on layers 0 and 8: the
crusade floor tiles, the tilted `tower_wood_trash_*` boards with z scales 0.07 to 0.36, the market's carts and tents,
three colliders on one GameObject, the fog-of-war walls scaled 10 x 4 x 0.1). With the recorded rotation and lossy
scale, distance and point exact:

| Pose and geometry | Exact of 1,797 |
|---|---:|
| the GameObject's position plus the rotated, lossy-scaled centre (PhysX's pose composition, Unity's managed quaternion or the recorded `localToWorldMatrix`), rotation as recorded | 1,544 |
| the same with the rotation normalised as PhysX does (dividing instead: 1,545) | 1,566 |
| **`TransformPoint(center)`, the rotation normalised, half extents from the lossy scale** | **1,797** |
| the same, half extents from the local scale | 1,784 |
| the same, rotation not normalised / normalised twice | 1,766 / 1,766 |

The misses were the boxes tilted about several axes: each one's probes all became exact with one coordinate of its
centre moved by one ulp (seven boxes) or, for one board, its rotation normalised. Across every current capture the
probes hit a box 176,106 times; every one the scene query answers (175,224; the rest hang on the pruner, below) is
exact, distance, point and collider.

**Checked against the player** (`native_check::transform_point_and_box_extents_match_the_player`): `0x6744c0` called
with a `TransformHierarchy` laid out as it reads it (fence 0x0, locals 0x18 as 48-byte `t, q, s`, parents 0x20; the
transform's hierarchy 0x38 and index 0x40) on a million generated hierarchies (1 to 5 levels, identity, one-axis and
arbitrary rotations, uniform, non-uniform, negative, tiny and subnormal scales, zeros of both signs) and points, and
`0x6b5280` on a million sizes and scales (negative, zero, tiny, subnormal): **bit-identical**. Changing the sum's
association or dropping the `FLT_EPSILON` clamp is caught.

**Established for** (`BoxRuntime::established`; others keep the conservative test): a static box (the importer
refuses a mesh or box collider under a `Rigidbody`, whose shape is posed relative to the body; none in the caves), its
ancestors only translating (rotation `(0, 0, 0, 1)`, scale exactly one: every recorded box's; the code takes the same
path under any hierarchy [I], but no probe covers one), its hierarchy placing the GameObject where the recording has
it with the recorded rotation and (to a millionth) scale, a positive lossy scale (a negative one takes the same code [C], but no recorded box has one). In the caves 61
boxes are computed; `Loot/Loot_1/luxery_chest_02_Collider` (a rotated ancestor; layer 10, outside the walkable mask)
keeps the conservative test, and 26 recorded inactive or disabled (loot, interactions, Terendelev's scales, all layer
10) are left out.

**The centre is computed, and now compared with the game's** [M]: `TransformPoint(center)` runs on the scene file's local
transforms, taken to be the ones the game holds. The earlier captures pinned the GameObject's world position and rotation
bit for bit and its scale only to the lossy scale's rounding (a scale one ulp off, or translations moved between
ancestors, would have left those checks passing and could move the centre's last bit); `poc5-tb-140` records
`transform.TransformPoint(center)` itself (`ph_col` `wcenter`, observer change 3 below): of its 167 box records, the 62
boxes in the area's physics file (51 in `Prologue_Caves_1_Static`, 11 in its `_Mechanics`; 26 more recorded inactive or
disabled are left out, and the 78 of `BaseMechanics` and `UI_Ingame_Scene` are units' and interface colliders) have
their `wcenter` **equal to the file's `TransformPoint(center)` bit for bit on all five runs** (0 differences; the
`box centres:` line of `wr-combat replay`). The importer still computes the centre; the recorded one is a check, reported
when it differs (also for a box whose mapping isn't established). The caves' boxes are all under translating ancestors, so
rotated or scaled ancestors are covered by the player check above, not by a recording.

**The importer** (`wr-import phys-pack --colliders`): each box keeps its local transforms from the root down (the
scene's, which Unity loads unchanged) and, from its `ph_col` record (matched by scene, path, sibling path, type,
centre and size: a GameObject can have several), the recorded rotation, lossy scale, position and layer; its world
centre is `transform_point(chain, center)`. The driven replay checks a recording's box `ph_col` against the file as it
does the meshes' (`World::check_recorded_colliders`: a mesh or box without a recorded transform must be recorded on
its scene layer, trigger flag and position too, and a scene whose colliders were all left out is still checked);
every capture with `phys_probe` (`-112` onward, 98 runs) records
the same boxes and meshes as `-129`'s record.

**The scene query.** A box with its recorded transform joins the meshes: a candidate when it hits the whole ray
(`raycast_box` only loses hits as the ray shortens: `max` is only compared with), tried under every sign of its
recorded zeros, its bounds (the box's exact AABB under the `f32` pose) deciding whether the pruner surely tests it.
Floor tiles lie a few millimetres above the terrain, so a terrain hit found first shrinks the ray to end just below a
tile's top: with stream S's tolerance (1e-5 of the magnitudes involved, about 7 mm here) 85 `ag_phys` records and
16,464 probes hung on the pruner. The tolerance is now 2e-6 of the same sum [I]: the pruner's rounding is a few `f32`
ulps of the coordinates it works with (node centres and extents from bounds, the segment's centre and half-diagonal,
leaf bounds from the pose and extents, which PhysX also inflates), and 2e-6 of the summed magnitudes is still over 16
ulps of each; it leaves 9 and 10,290, and nothing wrong. What remains is genuine: two floor tiles whose tops lie in
one plane (16 records "tie": the pruner's order picks the collider, the distance and point the same), shapes a few
micrometres apart (two cave meshes, a mesh under a tile), or the pruner's culling within the tolerance.

**Against the recordings** (`tests::every_recorded_query_is_computed_exactly`; "near a box": a query the file without
the boxes' transforms answered `Unverified` for a box):

| Captures | Runs | Queries (near a box) | Exact (box hits) | Wrong | Failed closed |
|---|---:|---:|---:|---:|---:|
| `-95`, `-96`, `-100` to `-105`, `-109`, `-111` to `-118`, `-120` to `-135` | 155 | 393,802 (4,752) | 393,777 (3,307) | 0 | 25 (16 ties, 9 the pruner; critters' and `other`) |
| their probes (`-112` onward) | 98 | 658,854 (298,606) | 648,564 (175,224 of 176,106 box hits) | 0 | 10,290 (the pruner or order) |
| the table's captures before (stream S, `-95` to `-129`) | 129 | 279,186 | 278,346 | 0 | 840 (a box) |
| their probes before | 72 | 484,056 | 176,184 | 0 | 307,872 (301,968 a box) |

The test now fails on any sim query that fails closed but presentation code's (critters', `other`), which the driven
replay takes from the recording.

## Failing closed

`Scene` answers only what is established; otherwise a `QueryError` the caller reports:

- `Unverified { shape }`: the ray may touch a box or a mesh without a recorded transform (or one the mapping isn't
  established for), or another collider, within `[0, max]`. The test is in `f64` on
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
  hands PhysX); none occurs here. A mesh or box with a recorded transform is computed (above).
- `Unsupported`: a trigger collider in the mask (`queriesHitTriggers`), the `*_!Visual` layer ambiguity above (a
  recorded mesh has its run-time layer instead), more than one height field in the mask, an answer that depends on
  the scene query's shape order, the pruner's culling or the sign of a recorded zero (above; "shapes tie" when only
  the collider depends on it), more than 5 shapes that can hit, a non-finite origin or negative distance.

## The file

`wr-import phys-pack --area prologue_caves_1 --guid bad465e5404604346bdf71ef50b3299a --colliders
fixtures/captures/oracle/poc5-tb-129/fixtures/tb-lizard/record.trace.jsonl.gz` writes `cache/sim/phys/<guid>.wrphys`
(header as the nav file's: magic, version, length, FNV-1a; postcard payload; the height samples packed, materials
stored once when uniform). Format 2 (stream S): every shape's sibling path (names repeat: three `cave_wall_01`s; the
driven replay now matches a hit's collider by it exactly), each mesh's decoded BV4 mesh and recorded transform:
9.1 MB. Format 3 (stream boxes): each box's local transforms from the root down and its recorded transform (world
centre, rotation, lossy scale, position, layer). Without `--colliders` the meshes and boxes keep the conservative
test. It isn't in `scripts/scene-cache.sh` yet (other
areas may hit the refusals above, and need a recording with `phys_probe`).

## For the sim

**Done** (stream M, `docs/poc5/movement.md`, "Physics and paths computed"): in the driven replay the agent's
`AgentPhysics` and the movement simulation's queries are `Scene::raycast_down` and `Scene::heuristic_disc_cast_down`
on the area's file (`Scene::owned` keeps it), every recorded `ag_phys` a check; a `QueryError` is reported and the
recorded answer taken. Stream S's hooks (`combat/replay/nav.rs`): a hit's collider is the shape whose name and
sibling path the record names (it was a name that had to be unique); a recording with `phys_probe` has its `ph_col`
checked against the file's recorded transforms (`World::check_recorded_colliders`, the boxes' too since stream
boxes), reported as "the recorded colliders (ph_col) aren't the physics file's" (a damage case in `poc5_agent.rs`).

On every run of `-121`, `-128` and `-129` no mesh is reported any more: `-129` is valid on all three, `-121` reports
only its pre-`origin` `other` records, `-128` only the place-on-ground view's difference (a capture before
`ground_view`). Every run of the valid captures (`-95`, `-96`, `-109`, `-111` to `-117`, `-120`, `-122/tb-focus`,
`-123` to `-127`) stays valid.

With the boxes (stream boxes): the critters' and `other` rays near the floor tiles are computed and checked like the
rest (25 left failing closed, taken from the recording), and the recorded boxes' `ph_col` is checked against the
file. Every run of the valid captures (`-95`, `-96`, `-109`, `-111` to `-117`, `-120`, `-122/tb-focus`, `-123` to
`-127`, `-129`, `-132`, `-133`: 93 runs) stays valid and T0-equal; the POC-5 suites pass (34 of 34).

## Not modelled

- Colliders that aren't in the static scenes: units (layer 9, outside the mask), spawned objects, and map objects
  whose colliders change state; a runtime collider on a walkable layer would be missed. `ph_col` (below) lists them.
- Meshes and boxes without a recorded transform (above), boxes under a rotated or scaled ancestor or with a negative
  scale, colliders under a `Rigidbody`, capsules, spheres, controllers, convex meshes, cooking (a mesh without baked
  data), the BVH33 midphase, any-hit and multiple-hit mesh queries, Unity's world rotation and `lossyScale` (recorded
  instead; `TransformPoint` is ported), the pruner's tree and order (every order tried instead).
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

3. **`ph_col` `wcenter`** (stream boxes, `PhysProbe.cs`): for a `BoxCollider`, `transform.TransformPoint(center)`
   (round-trip floats), so the importer can take the box's world centre as recorded (and check it against its
   `TransformPoint` port) instead of computing it from the scene's local transforms.
   **Recorded since `poc5-tb-140`** (stream xform): `ph_col`'s `wcenter` for every `BoxCollider` (167 in its record). The
   driven replay compares each box of the physics file with its `wcenter` bit for bit
   (`World::check_recorded_colliders`; the count is the `box centres:` line of `wr-combat replay`); the importer still
   computes the centre from the scene's local transforms, which makes the recorded one a check, not an input.

## Reproduce

```sh
eval "$(scripts/worktree-setup.sh phys)"   # or the main checkout's cache
cargo run --release -p wr-import -- phys-pack --area prologue_caves_1 --guid bad465e5404604346bdf71ef50b3299a \
  --colliders fixtures/captures/oracle/poc5-tb-129/fixtures/tb-lizard/record.trace.jsonl.gz
WR_PHYS_CAPTURES=poc5-tb-100,poc5-tb-101,poc5-tb-102,poc5-tb-103,poc5-tb-104,poc5-tb-105,poc5-tb-121,poc5-tb-128,poc5-tb-129 \
  scripts/heavy.sh cargo test --release -p wr-phys -- --ignored --nocapture every_recorded
# The native checks (a million rays each, 26 million on the meshes; the mutations; every f32 through frecpe;
# TransformPoint and the box extents, a million each):
WR_PHYS_MUTATION_RAYS=100000 scripts/heavy.sh cargo test --release -p wr-phys -- --ignored --nocapture \
  --test-threads=1 native_check
```
