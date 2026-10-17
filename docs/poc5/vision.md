# POC-5: line of sight (stream "vision")

Status (2026-10-17, stream "vision"): line of sight is computed in the driven replay. On every run of the 21 captures
`-95`, `-96`, `-109`, `-111` to `-117`, `-120`, `-122` (tb-focus), `-123` to `-127`, `-129`, `-132`, `-133` and `-136` (96
runs: record and two replays of each fight) the engagement's `HasLOS`, the group memory's `group.HasLOS`, every sight
cache miss and clear and every `HasObstacle` answer are the sim's and equal the recording's; every run is valid. The
geometry itself (the 8 blockers of the caves) is untested by these captures (see "Observer changes requested"). Tags as
elsewhere: [M] measured, [C] from the decompiled code, [I] inferred.

The AI's `InLos`, the engagement's `IsAttackOfOpportunityReach` `HasLOS`, the group memory's `group.HasLOS` and the
movement simulation's and path search's `HasObstacle` answers were recorded inputs (`docs/poc5/memory.md`, "Vision").
This note says what the sim computes now, from what, and what the recordings establish.

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
  `CheckLOS`. `CanBeInVisionRange` is WrathOracle's `vis` records applied (a unit's set and its changes: the overlap's
  order doesn't matter for `Contains`; the PhysX capsule overlap itself isn't ported, the sets stay a recorded input).
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

What was taken from the recording in these runs: `CanBeInVisionRange` (the `vis` records), and 9,336 of the 1,492,226 group
reads (0.6%) answered from a cache entry made before the window (the leaf's value; the sim's fresh computation agrees with
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
- `CanBeInVisionRange` is the recording's (`vis`): the capsule-sphere overlap isn't ported (PhysX; membership is what
  `HasLOS` needs, but the sets' order, which the memory dictionary observes, is PhysX's).
- The TransformPoint rounding of the blockers and the grid's bounds are derived, not compared (above).
- Without the driven agent (`-95`, `-96`, `-109`, `-111`, `-112`) a unit's position is the recording's rows, and in the
  recording's last frame the last prefix's (counted in `last_frame_reads`; checked against the leaves like any other).

## Rebuilding after merging

`wr-rules-data` (once per install; the area bounds table gains the fog of war bounds; an old table leaves line of sight
a recorded input, reported in the run's `vision:` line), then per area `scripts/scene-cache.sh <name> <guid>` (now writes
the line-of-sight file too) or just `wr-import los-pack --area <name> --guid <guid>`.

