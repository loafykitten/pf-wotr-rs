# POC-5: line of sight (stream "vision")

Status (2026-10-17, stream "visrange"): the sets of units in vision range (`CanBeInVisionRange`) are computed too, no
longer taken from the recording's `vis` records: see "The sets in vision range" below. Before that (stream "vision"):
line of sight is computed in the driven replay. On every run of the 21 captures
`-95`, `-96`, `-109`, `-111` to `-117`, `-120`, `-122` (tb-focus), `-123` to `-127`, `-129`, `-132`, `-133` and `-136` (96
runs: record and two replays of each fight) the engagement's `HasLOS`, the group memory's `group.HasLOS`, every sight
cache miss and clear and every `HasObstacle` answer are the sim's and equal the recording's; every run is valid. The
geometry itself (the 8 blockers of the caves) is untested by these captures (see "Observer changes requested"). Tags as
elsewhere: [M] measured, [C] from the decompiled code, [I] inferred.

The AI's `InLos`, the engagement's `IsAttackOfOpportunityReach` `HasLOS`, the group memory's `group.HasLOS` and the
movement simulation's and path search's `HasObstacle` answers were recorded inputs (`docs/poc5/memory.md`, "Vision").
This note says what the sim computes now, from what, and what the recordings establish. The units in vision range
(`HasLOS` requires membership) were the recording's `vis` records until stream "visrange"; the sim computes them now.

## What the game computes [C]

- `LineOfSightGeometry` (`Owlcat.Runtime.Visual`, managed, no PhysX): `Init(bounds)` makes a grid of 3.8999999 m cells
  over the fog of war area's world bounds (`ceil(size / 3.8999999f)` cells a side, the bounds grown to whole cells) and
  registers every enabled `FogOfWarBlocker` (`FogOfWarBlocker.All`): each segment of its polyline goes into every cell
  of a walk over the grid (`GetCellsForSegment`: a DDA that keeps its quirks, e.g. after an exact tie it continues from
  the cell beside, not the diagonal one). `HasObstacle(from, to, fudge)` walks the same way over the cells that hold
  lines and answers whether a segment at least as high as `from.y` crosses (`VectorMath.SegmentsIntersect2D`). A segment
  with both ends outside the grid has no cell: nothing blocks it.
- A blocker's polyline (`FogOfWarBlocker.BuildPoints`): a `FogOfWarBlockerPolygon`'s transformed points (closed if the
  polygon is or the blocker is two sided; `HeightMinMax.y` the top plus the polygon's `Height`), else the convex hull
  on the ground plane of the `MeshFilter` mesh's transformed vertices (`Polygon.ConvexHullXZ`, a gift wrap; closed;
  `HeightMinMax.y` the highest vertex).
- `UnitEntityData.HasLOS(other)` = `(Blueprint.IsFake || CanBeInVisionRange.Contains(other)) && SightCache.GetLOS(other)`.
  `UnitSightCache` answers from the entry for `other` if `other` stands within 0.5 m (ground plane) of where it stood when
  the entry was made; a miss computes `CheckLOS(other.Position + up, m_LastSamplePosition)`: within the vision radius
  (squared, in double) and no obstacle from `sample + up`. `UnitLineOfSightCacheController` (before `UnitMoveController`)
  clears a cache whose owner moved 0.5 m from the sample, or whose `VisionRangeMeters` changed (11.7 for the player's
  faction without overrides; else 8.5, in combat `Max(11.7, that)`), or when the geometry's `LastUpdateTime` moved.
- The group's `HasLOS(unit)`: the members in order, each `IsInGame && !IsHelpless && member.HasLOS(unit)`, stopping at
  the first true. The engagement's `IsAttackOfOpportunityReach` is `unit.HasLOS(enemy)`. The AI's `InLos` is
  `HasObstacle(context.Unit.EyePosition, Unit.Position)` with no cache.

## What the sim computes

- **The geometry** (`crates/wr-los`): `Geometry::init` and `has_obstacle` are the C#'s, expression by expression, in Mono's
  float semantics (`wr_nav::cs`: an expression in double, rounded where the C# stores). `wr-import los-pack` writes the
  area's blockers (`cache/sim/los/<guid>.wrlos`) from the scenes (`tools/wr-import/src/los.rs`); the grid's bounds are
  the area's `AreaPartBounds.FogOfWarBounds` (the area bounds table, `wr-rules-data` extracts the override flag and the
  bounds now) moved by the `FogOfWarArea`'s position as `SetFogOfWarParameters` and `GetWorldBounds` do (`(c - p) + p`
  in `f32`). Unit tests: the walk against a brute-force test of every wall on 8,000 random segments, the outside-the-grid
  rule, the height rule, open and closed polylines, the fudge radius.
- **The sight caches** (`combat/vision.rs`): `UnitSightCache` per unit, `ClearIfNeeded` in the new `Phase::LosCache`
  (`UnitLineOfSightCacheController`, before the follow and move controllers; Default and Cutscene modes), `HasLOS`,
  `CheckLOS`. `CanBeInVisionRange` is the sim's capsule-sphere overlap over the entity-bounds colliders
  (`combat/vision/range.rs`, `Phase::Visibility`), checked against WrathOracle's `vis` records (see "The sets in vision
  range"); the sets the readers use are equal to the recording's.
- **The readers** (`combat/replay/vision.rs`): the engagement's `unit.HasLOS(enemy)` (`uc_v` `los`), the group memory's
  `group.HasLOS(target)` (`mem_v` `los`: the members in order, `IsInGame`, `IsHelpless`), the AI's `InLos`
  (`HasObstacle(EyePosition, Position)`), the movement simulation's `HasObstacle` (`nav` `has_obstacle`) and the path
  search's recorded answers (`path_fin` `los`) are the sim's, each checked against the recording's leaf (a difference is
  reported and the recorded value used so the run goes on).
- **Positions.** With the driven agent (`agent_sync`, `-113` and later) a unit's position is the sim's (`unit.position`,
  checked against every observation row). Without it (`-95`, `-96`, `-109`, `-111`, `-112`) a unit's position is its
  observation row: at a frame's prefix for the cache controller (before `UnitMoveController`), the next frame's prefix
  for every reader after it. [M] on `-111`/tb-focus: every one of the 349 recorded misses' target eye points equals
  some unit's row at that frame's prefix or the next (188 only the next: the target moved in that frame), every
  `los_clear` is at the owner's prefix row (132 of 132), every miss's sample is a row of its owner.

## What the recordings can't show, and how the sim goes on

A recording opens mid-session, so each sight cache's entries, sample position and radius before the window are unknown.
The sim treats a cache as *unknown* until its owner's first `los_clear` in the window (then its sample, entries and
radius are the sim's: **known**). For an unknown cache:

- a read with a recorded miss (`los`: owner, the target's eye point) is a miss: computed from the record's sample and
  checked (result), the entry kept; without a recorded miss it is a hit on an entry made before the window, whose
  value is the recording's (an aggregate group read, `Pre`: the leaf's value; the engagement's read: its leaf's), kept in
  the entry, and compared with the fresh computation (counted);
- its clears are the recording's (`adopted clears`).

So **"computed and equal"** holds for every read of a known cache, every miss and every clear; the reads answered from an
entry made before the window are counted apart (see the results).

Not modelled, reported: the geometry updated at run time (`los_geometry`), a unit with an `OverrideVisionRange` fact
(the run isn't computed), an `ExtendedVisionArea` (no spawner of the caves has one), an area with parts, a fake-blueprint
unit (modelled: `Blueprint.IsFake` skips the range set).

## Results [M]

`wr-combat replay <capture>/fixtures/<fight> --run <run>` prints a `vision computed:` line. Record runs (a fight's two
replays have the same counts), 96 runs in all:

| capture / fight (record run; its two replays equal) | engagement reads | memory group reads (on pre-window entries) | misses computed | misses by readers the sim doesn't run | clears (adopted) | `HasObstacle` answers |
|---|---|---|---|---|---|---|
| -95 tb-focus | 10,334 | 0 (0) | 8 | 296 | 123 (5) | 0 |
| -95 tb-nearest | 10,546 | 0 (0) | 6 | 296 | 119 (5) | 0 |
| -96 tb-hold | 6,404 | 0 (0) | 11 | 298 | 109 (6) | 0 |
| -109 tb-focus | 5,406 | 0 (0) | 8 | 338 | 105 (6) | 0 |
| -109 tb-nearest | 2,176 | 0 (0) | 4 | 342 | 105 (6) | 0 |
| -111 tb-focus | 7,840 | 14,067 (115) | 349 | 0 | 127 (5) | 0 |
| -111 tb-nearest | 9,054 | 16,258 (117) | 363 | 0 | 126 (5) | 0 |
| -112 tb-focus | 13,082 | 18,960 (115) | 359 | 0 | 129 (5) | 0 |
| -112 tb-nearest | 4,066 | 13,871 (116) | 235 | 0 | 103 (5) | 0 |
| -113 tb-focus | 1,538 | 11,348 (115) | 300 | 0 | 93 (4) | 0 |
| -113 tb-nearest | 1,532 | 8,737 (117) | 334 | 0 | 100 (5) | 2 |
| -114 tb-focus | 8,582 | 16,349 (115) | 369 | 0 | 122 (5) | 24 |
| -114 tb-nearest | 7,000 | 12,760 (115) | 412 | 0 | 128 (5) | 17 |
| -115 tb-focus | 3,144 | 14,362 (116) | 308 | 0 | 109 (6) | 18 |
| -115 tb-nearest | 6,244 | 12,505 (114) | 381 | 0 | 124 (5) | 18 |
| -116 tb-hold | 6,906 | 13,048 (116) | 364 | 0 | 126 (6) | 21 |
| -117 tb-solo | 4,758 | 15,130 (117) | 325 | 0 | 105 (5) | 17 |
| -120 tb-cast | 4,875 | 12,728 (113) | 284 | 0 | 96 (5) | 25 |
| -122 tb-focus | 6,624 | 11,941 (114) | 349 | 0 | 124 (5) | 18 |
| -123 tb-chain | 10,120 | 20,389 (113) | 610 | 0 | 315 (5) | 35 |
| -124 tb-chain | 10,886 | 22,004 (115) | 740 | 0 | 327 (6) | 37 |
| -125 tb-focus | 11,102 | 17,680 (116) | 420 | 0 | 133 (6) | 30 |
| -125 tb-nearest | 1,662 | 9,414 (115) | 351 | 0 | 105 (5) | 17 |
| -126 tb-focus | 4,414 | 12,058 (115) | 282 | 0 | 102 (5) | 15 |
| -126 tb-nearest | 10,498 | 14,975 (116) | 323 | 0 | 125 (5) | 18 |
| -127 tb-tie | 11,542 | 16,503 (117) | 326 | 0 | 121 (5) | 19 |
| -129 tb-lizard | 29,140 | 66,157 (114) | 1462 | 0 | 719 (7) | 75 |
| -132 tb-focus | 12,574 | 17,522 (115) | 359 | 0 | 128 (5) | 20 |
| -132 tb-nearest | 3,664 | 12,802 (116) | 295 | 0 | 101 (5) | 16 |
| -133 tb-focus | 22,620 | 26,794 (116) | 379 | 0 | 133 (5) | 32 |
| -133 tb-nearest | 8,312 | 15,089 (115) | 309 | 0 | 107 (6) | 19 |
| -136 tb-lizard | 20,432 | 53,819 (114) | 1240 | 0 | 742 (6) | 60 |

Over the 96 runs: 831,231 engagement reads and 1,492,226 group reads computed and equal; 35,595 cache misses computed
(each equal to its record: target eye point, sample, result) and 4,710 misses of readers the sim doesn't run verified and
adopted (`-95`, `-96`, `-109` only: they have no `memory` observer, so the memory controller's reads aren't the sim's;
each is checked against `CheckLOS` of its recorded sample, its target is the unit standing at the eye point and its entry
is kept); 15,993 clears equal (510 of caches whose first clear in the window it was: adopted); 1,659 `HasObstacle`
answers (the movement simulation's and the path search's) equal to the sim's geometry; 418,209 reads of a unit outside
`CanBeInVisionRange` (false without asking a cache). The AI's `InLos` isn't read in any of these fights (0 reads: wired
and unit-tested only).

What was taken from the recording in these runs (before the sets in vision range were computed, which they are now): 9,336 of
the 1,492,226 group reads (0.6%) answered from a cache entry made before the window (the leaf's value; the sim's fresh computation agrees with
it in 9,498 of 9,498 cases, evidence that the geometry and the radius are right, not an answer). No engagement read was.
In `-111` and later the sim's `unit.position` is checked against every observation row only with the driven agent
(`-113` and later); `-111` and `-112` have the agent's observer but no `agent_sync`: positions are the rows.

Damaged records (`computed_line_of_sight_replays_are_valid_and_damage_is_reported`, `tests/poc5.rs`, three captures
valid on all runs; 14 damaged cases on `-111`/tb-nearest, each reported): an engagement leaf and a memory leaf flipped; a
miss's result, sample or point changed, deleted or doubled; a clear (of a cache cleared before) deleted, moved or doubled;
a `vis` record missing a unit, or removing one that wasn't there; a `los_geometry` record; a `los_probe` answered against
the geometry (and the same probe answered as the geometry has it: valid, counted). Unit tests (`combat/vision/tests.rs`,
16): the miss/hit/stale cycle, blockers and the eye height, the radius and the range set, refusals of each kind of
wrong miss, the cache controller, caches from before the window (hits with the recording's value, the 1 m limit, a
resolved entry staying resolved, clears from a known sample, adopted clears), unmodelled readers' misses (order,
double adoption, the frame's radius, the target among several units), the geometry dump and probe checks.

### Review

gpt-6.1-sol (high) through Codex, three rounds on `poc..stream-vision`. Round 1: 11 findings, 10 real and fixed
(unmodelled misses adopted in order before the read after them; an adopted miss refused where the cache would have
answered; a cache with a known sample cleared by the owner's movement; hits on pre-window entries limited to 1 m;
group reads resolving the entries of a false aggregate; the position view from the frame's phase; last-frame positions;
`t` of vision records; `vis` deltas as set differences; first clears only for awake owners); rejected: recorded clears
matched by owner rather than in `AwakeUnits` order (independent caches' clears have no observable order). Round 2: 8
findings, 7 fixed (a rounding margin on the 1 m bound; adopted misses' targets among units in range, else all; the
frame's radius for a deferred miss; a miss at a pre-window hit's exact position refused; a single remaining member's
true aggregate resolving its entry; `vis` records placed in the vision controller's phase, once per awake owner and frame;
every `FogOfWarBlocker` component of a GameObject), 1 accepted as a false-report-only limitation (`vis_group`'s `later`
test can take an earlier unmodelled miss on the same pair for a later member's read).

## The sets in vision range: now the sim's (stream "visrange", 2026-10-17)

`UnitEntityData.CanBeInVisionRange` is computed at `Phase::Visibility` (`combat/vision/range.rs` for the game's logic,
`combat/replay/vision_range.rs` for the replay's side) and checked against WrathOracle's `vis` records. Tags as above.

### What the game does [C]

- `UnitVisionRangeController.Tick` (modes Default, Dialog, Cutscene, Rest; not Pause), for each unit of
  `State.AwakeUnits` in order: clear its set; skip it if `IsExtra`; else `OverlapCapsule(pos - up * 2r, pos + up * 2r, r,
  mask 1 << 23)` on the entity-bounds physics scene with `r = VisionRangeMeters`, and add each hit's unit. The set is a
  `HashSet` refilled after a `Clear`, so it enumerates in the overlap's order. `IsSleeping`'s setter (either way) and
  `OnDispose` clear it too.
- `EntityBoundsController.Tick` (registered just before it, also in Pause): one `SphereCollider` per entity with bounds, on a
  private object in that scene (unit scale 1). A unit's radius is `Max(0.05, Corpulence)` (`View.Corpulence`: the prefab's
  `m_Corpulence` times the size scale, at least 0.5); it is made at the view's position, its transform is set to
  `entity.Position` in each tick for the entities whose position changed (all of them on the first tick), and
  `EntityViewBase.UpdateViewActive` sets `enabled = ShouldBeEnumeratedByEntityPoolEnumerator` (`IsInGame` and not
  suppressed) the moment either changes. `Physics.autoSyncTransforms` is off [M, the begin record: `physics_auto_sync`
  false], so a transform change reaches PhysX at the next physics step (`FixedUpdate`, before the frame's controllers).
- PhysX's capsule-against-sphere overlap: the squared distance from the sphere's centre to the capsule's segment against
  the squared sum of the radii, in `f32` (`Gu::distancePointSegmentSquared`; ported, not read from PhysX).

### What the sim does

`BoundsUnit`/`Bounds::tick` keep each collider's transform and PhysX's copy (taken from the transform when the frame ran
a step); `Bounds::overlap` answers the query in the colliders' order (the pool's: `scene::units_pool`, a collider made
later is appended). The replay's inputs the sim doesn't have are the recording's (see below): each unit's corpulence, the
frames that run a physics step, the awake units. The unit positions are `Vw::pos` after the movement (the driven agent's,
else the observation rows). Each frame's phase computes every awake unit's overlap and compares it, when the frame has a
`vis` record for the unit, with the set the record gives (the last record's set plus its `add`/`rem`), and otherwise with
the set the last record left (no record means no change; the first record of a unit is always written). A difference is
reported (`driven replay: vision: the vision range of X differs ...`) and the recording's set is the one the run goes on
with, as every reader here. Units put to sleep lose their set. `UnitGroup.ExtractNearUnits` (the memory controller's
`near` leaf) is the union of the group members' sets in the overlap's order, checked against the leaf (`vis_near_union`).

### What the recordings show [M]

The hypotheses and their results (the first by a script on the party's 5,010 observer-frames of `-111`/tb-focus before
`t` 1670, the others in the sim over the 36 record runs):

1. **The capsule's position** is the querying unit's position after this frame's movement (the next frame's prefix row;
   with the driven agent, `unit.position` after the Move phase). Sets different from the recording's among the 5,010
   observer-frames, by the capsule's position and the collider's (the prefix row is where the unit stood at the previous
   frame's end of tick): capsule at the prefix, collider a frame older than the prefix 14, at the prefix 11, at the
   post-move position 14; capsule post-move, collider a frame older 3, at this frame's post-move position 3, **at the
   prefix 0**.
2. **The collider's position** is its transform at the last physics step: the previous tick's in a frame that runs a
   step, the tick before in one that doesn't. Which frames run none follows the physics clock `phi = Time.time -
   Time.fixedTime` (WrathOracle's `PhysicsPhase`): the begin record's `fixed_phase0` plus each tick's scaled `delta`
   (the oracle's fast-forward stretches run at time scale 3, `delta` 0.05 s, three fixed steps in two frames or so,
   which moves the phase; the fixed step is `fixed_delta_time`). Differences against the recordings over the 36 record
   runs: a step in every frame, 683 (1 to 36 a run, 543 of them one static pair of `-133`/tb-nearest); the clock advanced
   by the fixed frame time, 682; **the clock advanced by each tick's scaled delta, 0**. (The first guess, that a frame
   in six has no step by the begin record's phase, is right except where the time scale moved the phase.)
3. **The collider's radius** is `Max(0.05, Corpulence)` exactly. An intermediate version allowed every collider an offset
   of up to 5 cm from it (the view's scale, which the collider keeps unless more than 0.05 off) and left the pairs that
   offset leaves open to the recording (20 to 66 a run of the 100,000 to 1 million pairs, each answered by the
   recording, with 0 differences once the clock was right). With no tolerance, 109 runs: 0 differences, 0 pairs left
   to the recording. The sim has no unit's corpulence (`View.Corpulence` needs the prefab's
   `m_Corpulence` and the size scale: not imported) so it is the recording's: the last `corp` read (`uc_v`, `ag_v`) at or
   before the frame, else the unit's first read after it (counted per unit and frame in the run's line: the party's
   corpulence before its first fight read; a size effect would change it, `-120`'s is read again). A unit with no read at
   all would have an unknown radius (at least 0.5, `Radius::Unknown`): the pairs that range decides are the sim's, the
   others the recording's; none occurred (0 such pairs in all runs).
4. **The enabled flag** is the sim's `is_in_game` (a unit that dies leaves everybody's set in the frame the recording
   shows). A collider made in a tick after the first (a spawn) is at the origin until the next step [C, unit-tested, **not
   exercised**: no run makes a collider after its first frame, the run's line counts them].
5. **The margin** (item 3 of the task): the closest approach of any pair, over all runs, to the capsule's surface is
   **27 um** (`-131`); 7,259 pairs of the 22.5 million computed (all 109 runs) are within 1 cm and 125 within 1 mm, in
   sets that agree. `f32` against exact arithmetic: **0** of the 22.5 million pairs decide differently, so PhysX's
   rounding doesn't matter for these captures (the sim decides in `f32` as PhysX does; the exact margin is only logged).
6. **The order.** The recorded `near` leaves' first member's prefix gives that member's set order exactly: over five
   captures (`-111`, `-120`, `-123`, `-129`, `-136`) there is one static total order, never a conflict: the party first, then the other units of the area by id, that is the
   pool order [`State.Units.All`: the units of the cross-scene state, then the area's]. The sim's overlap in the colliders'
   order (created in pool order, a later collider appended) reproduces **every** recorded `near` leaf: 385,005 of 385,005
   in the record runs of `-111` and later (1,218,083 in all 109 runs; `-95`, `-96` and `-109` have no `memory` observer),
   so the order is no longer an input either. (Why PhysX returns the colliders in creation order for this scene isn't
   established; it holds for every capture here, which have at most 21 colliders.)

### Results [M]

`wr-combat replay <capture>/fixtures/<fight> --run <run>` prints the counts after `vision range:`. Record runs (a fight's
replays have the counts of their own length). **Every run of all 24 captures** (`-95`, `-96`, `-109`, `-111` to `-117`,
`-120`, `-122` to `-127`, `-129`, `-131` to `-136`: 109 runs) is as valid as before (only `-122`/tb-nearest's initiative
tie is not) with the sets computed and equal to the recording's:

| capture / fight (record run) | sets computed (awake unit, frame) | of which changes (`vis` records) | pairs | within 1 cm | within 1 mm | closest (um) | near-unit unions equal |
|---|---|---|---|---|---|---|---|
| -95 tb-focus | 20,641 | 53 | 139,971 | 8 | 0 | 2,227 | 0 |
| -95 tb-nearest | 20,817 | 56 | 140,862 | 8 | 0 | 1,242 | 0 |
| -96 tb-hold | 17,394 | 55 | 115,188 | 11 | 0 | 2,652 | 0 |
| -109 tb-focus | 15,050 | 56 | 96,426 | 9 | 0 | 4,465 | 0 |
| -109 tb-nearest | 13,755 | 52 | 85,694 | 7 | 0 | 1,489 | 0 |
| -111 tb-focus | 18,612 | 61 | 123,154 | 12 | 1 | 266 | 7,400 |
| -111 tb-nearest | 20,805 | 59 | 140,598 | 12 | 1 | 266 | 8,087 |
| -112 tb-focus | 22,918 | 62 | 153,985 | 16 | 2 | 266 | 8,969 |
| -112 tb-nearest | 15,109 | 55 | 100,885 | 9 | 0 | 1,489 | 5,924 |
| -113 tb-focus | 14,532 | 50 | 93,266 | 12 | 1 | 174 | 6,047 |
| -113 tb-nearest | 13,019 | 52 | 80,532 | 9 | 0 | 1,489 | 5,567 |
| -114 tb-focus | 21,320 | 46 | 137,342 | 15 | 0 | 1,489 | 8,649 |
| -114 tb-nearest | 18,574 | 56 | 113,058 | 13 | 3 | 266 | 7,916 |
| -115 tb-focus | 15,663 | 48 | 101,902 | 7 | 1 | 450 | 6,365 |
| -115 tb-nearest | 18,677 | 52 | 115,764 | 13 | 0 | 1,052 | 7,766 |
| -116 tb-hold | 19,214 | 52 | 121,592 | 11 | 0 | 1,489 | 7,802 |
| -117 tb-solo | 18,734 | 52 | 122,095 | 10 | 1 | 266 | 7,412 |
| -120 tb-cast | 18,693 | 46 | 118,012 | 8 | 0 | 1,489 | 7,598 |
| -122 tb-focus | 18,260 | 53 | 115,515 | 15 | 2 | 266 | 6,592 |
| -122 tb-nearest | 19,336 | 47 | 125,126 | 162 | 0 | 3,527 | 7,635 |
| -123 tb-chain | 36,097 | 90 | 224,026 | 28 | 2 | 185 | 15,676 |
| -124 tb-chain | 37,531 | 84 | 232,281 | 24 | 1 | 140 | 15,676 |
| -125 tb-focus | 24,357 | 47 | 159,352 | 14 | 1 | 266 | 9,884 |
| -125 tb-nearest | 14,390 | 54 | 89,252 | 561 | 0 | 1,489 | 5,801 |
| -126 tb-focus | 15,918 | 57 | 100,389 | 7 | 0 | 1,489 | 6,380 |
| -126 tb-nearest | 19,581 | 57 | 132,912 | 11 | 0 | 1,144 | 7,619 |
| -127 tb-tie | 20,422 | 55 | 137,900 | 12 | 1 | 266 | 7,949 |
| -129 tb-lizard | 87,004 | 185 | 668,199 | 69 | 10 | 70 | 38,016 |
| -131 tb-lizard | 110,856 | 128 | 1,030,743 | 36 | 1 | 27 | 50,813 |
| -132 tb-focus | 21,824 | 62 | 148,097 | 14 | 1 | 266 | 8,489 |
| -132 tb-nearest | 16,091 | 52 | 101,640 | 7 | 0 | 1,489 | 6,734 |
| -133 tb-focus | 29,145 | 64 | 199,268 | 21 | 3 | 266 | 11,063 |
| -133 tb-nearest | 16,254 | 51 | 105,188 | 1,091 | 0 | 2,110 | 6,590 |
| -134 tb-lizard | 88,670 | 179 | 708,106 | 68 | 5 | 87 | 39,141 |
| -135 tb-nearest | 21,826 | 53 | 152,417 | 16 | 2 | 266 | 8,097 |
| -136 tb-lizard | 82,357 | 160 | 615,071 | 49 | 2 | 457 | 37,348 |
| all 36 record runs | 1,003,446 | 2,441 | 7,145,808 | 2,395 | 41 | 27 | 385,005 |

All 109 runs: 3,146,005 sets computed and equal (7,566 of them changes, each equal to its `vis` record), none unequal; the
remaining frames show that no record means no change. The group memory's `near` leaf is now the sim's (no longer counted
among `memory computed`'s inputs; 1,218,083 computed).

Damaged records (`computed_line_of_sight_replays_are_valid_and_damage_is_reported`, now 16 cases on `-111`/tb-nearest,
each reported): the 14 above (a `vis` record missing a unit or removing one that wasn't there are now reported by the
overlap too) and a change with no record (a `vis` record deleted), a far unit added to a set.

### What stays recorded

- The awake units (`aa_awake`; `-95` and `-96` have none: the sim's own, with the units it can't establish settled by
  the `vis` records' observers), as everywhere.
- Each unit's `Corpulence` (the `corp` reads) and the physics clock (`fixed_phase0` of the begin record, each tick's
  `delta`, `fixed_delta_time`), the frames at the window's start (the colliders' positions at the window's first frame are
  the prefix rows).
- Unit positions: the recording's rows without the driven agent, as before.
- Observer requests (to remove the last inferences; none is needed for these captures to be valid): a `bounds` record at
  the window's start and on change, one per collider (`SphereBoundsCollider.radius`, `transform.position`, `enabled`, the
  creation order), and the number of fixed steps each frame ran (`Physics` has no counter: `MonoBehaviour.FixedUpdate`
  count of a helper), so the clock is read, not computed; the corpulence of every unit with a collider (today only the
  units the fights read).

## Observer changes requested

**The blockers can't be checked against these captures.** The caves' 8 `FogOfWarBlocker`s are thin walls (cubes scaled
10 x 4 x 0.001) between x 120 and 145, z 58 and 94, away from the fights: the 4,805 recorded cache misses of
`-111`, `-114`, `-123`, `-129` and `-136` (record runs; a miss is the segment from the owner's sample to the target) stay at
least 38.9 m from every wall [M], so no recorded read has a blocker near its segment, and every recorded `HasLOS` that is
false is false for the vision radius or the range set. What the 800,000 reads establish for the geometry is that the imported one never blocks a segment the
game found clear; its DDA, intersection test and rounding are checked by unit tests against a brute-force test, not
against the game. Two things stay open for the blockers themselves, and one recording closes both:

1. `Transform.TransformPoint`'s rounding (the importer sums `m0 x + m4 y + m8 z + m12` in `f32`, left to right; 30 of the
   192 transformed cube vertices round differently when summed in double; the native routine's order isn't established);
2. the grid's bounds and the registration (the importer derives them; nothing has been compared).

**Added 2026-10-17 in the main session** (`LosProbe.cs`, observer `los_geom`, written right after the begin record
next to `phys_probe`; gpt-6.1-sol: two rounds, no findings left; not yet recorded): `los_geom` and `los_probe` as
specified below, 40 probes per distinct segment (`System.Random`, seed 20261017; Unity's RNG untouched). Left for the
sim: when `los_geom` is declared, require both records (today it checks whichever arrive).

WrathOracle's next recording should, once at the window's start (the geometry is initialised by then; both reads are
pure and take the geometry's read lock only):

- `los_geom`: `{nx, nz, min: [x, z], segs: [[cell i, cell j, A.x, A.y, B.x, B.y, Height], ...]}`, read by reflection from
  `LineOfSightGeometry.Instance` (`m_Cells` is a `GeometryCacheCell[,]` whose `Lines` hold `LineSegment {A, B, Height}`;
  `m_Bounds.min.x` and `.z`), floats round-trip, segments in cell order. The sim compares the grid and, per cell, the
  segments as a set (`vision::check_geometry`; the order within a cell is the `HashSet`'s).
- `los_probe`: `{v: [[from.x, from.y, from.z, to.x, to.y, to.z, fudge, 0 or 1], ...]}`, the game's own
  `LineOfSightGeometry.Instance.HasObstacle(from, to, fudge)` answers for a fixed list of segments around every blocker:
  for each segment of each blocker, 40 segments crossing it at random points (a fixed seed), at eye heights from 1 m
  below to 1 m above the blocker's top, some ending within 0.01 m of an end point. The sim computes each and compares
  (`vision::check_probes`).

The sim already handles both records (`replay/vision.rs`: `vis_geom`, `vis_probe`; unit-tested on its own geometry, and a
test case on `-111` for a probe across a caves wall), so the next capture checks the geometry bit for bit with no sim change.

The caches' state before the window (entries, sample, radius) is the only other thing the recordings lack: with it
(`los_cache` at the window's start: for each awake unit `m_LastSamplePosition`, `m_LastVisionRadius`, `m_LastGeometryUpdateTime`,
and each entry's target id, position and two results) every read would be computed, not just the ones after a unit's first
clear in the window. It is a private field (`UnitSightCache.m_Cache`, a `Dictionary<string, CacheEntry>`), read by reflection.

Two small additions would remove the sim's remaining inferences about the caches:

- the target's id in `los` (`t`): today the target of a miss by a reader the sim doesn't run is the unit standing at the
  eye point (in vision range when several do; a `GetLOS` called directly, as `AreaEffectEntityData.ShouldUnitBeInside`
  does, bypasses that range gate), and a deferred miss may have been made at the frame's starting radius or the current one;
- the caller of each miss (`callers`, as `mem_add` has), so a reader the sim doesn't run is named.

## Limits of the model (accepted, each at worst a false report)

- A group read over several entries from before the window leaves them unresolved when the aggregate is true; a later
  contradictory aggregate isn't caught (9,336 of 1.49 million group reads touch such entries; every one agrees with a
  fresh computation).
- `vis_group` takes a miss pending for a later member and the same target as that member's read (an earlier unmodelled
  miss on the same pair could be mistaken for it: only in a capture with unmodelled readers and then a false report).
- The clears are matched by owner, not in `AwakeUnits` order (independent caches).
- `CanBeInVisionRange` is the sim's, but its inputs aren't: the corpulences (read by the fights only), the physics clock
  and the awake units are the recording's; PhysX's result order is the colliders' creation order, which every recorded
  `near` leaf agrees with (385,005 of them) but PhysX's pruner is not ported (a scene with more colliders than 21 could
  order differently). The capsule test is PhysX's `f32` distance, which agrees with the exact one on all 22.5 million
  pairs here.
- The TransformPoint rounding of the blockers and the grid's bounds are derived, not compared (above).
- Without the driven agent (`-95`, `-96`, `-109`, `-111`, `-112`) a unit's position is the recording's rows, and in the
  recording's last frame the last prefix's (counted in `last_frame_reads`; checked against the leaves like any other).

## Rebuilding after merging

`wr-rules-data` (once per install; the area bounds table gains the fog of war bounds; an old table leaves line of sight
a recorded input, reported in the run's `vision:` line), then per area `scripts/scene-cache.sh <name> <guid>` (now writes
the line-of-sight file too) or just `wr-import los-pack --area <name> --guid <guid>`.

