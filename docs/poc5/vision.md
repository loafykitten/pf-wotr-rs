# POC-5: line of sight (stream "vision")

Status (2026-10-19, stream "bstart"): **the vision range's start state is the recording's** where a recording has WrathOracle's
`bounds_*` observers (`poc5-tb-140`, five runs): `bounds_init` (the entity-bounds scene's 64 roots before the window's first tick),
`bounds_tick` (every `TickEntity` call), `bounds_set` (collider writes outside it), `bounds_scene`, `phys_sync`, and `vis_order`
(each unit's set order). The colliders' radii, transforms, `enabled` flags, creation order and history are then the game's own, not
hypotheses, and the sim's model of the bounds controller (the radius rule, the update list, the transform rule, `IsInGame` against
`enabled`) is a check on them: it agrees with every record (7,832 calls a run). On `-140`'s five runs the pairs decided rise from
6,774,380 to 6,795,850 and the pairs left to the recording fall from 21,470 (all of them the view-scale band) to 0, with 0 differences
in 367,840 sets and every record consumed. What stays hypothesized is PhysX's copy of the transforms (the physics-clock inference), and
PhysX's result order, which the recording shows is **not always the creation order** (one of 162 order records, 34 frames of `-140`).
Older captures replay with the hypotheses unchanged (sweep below). See "The window's start, from the records".

Status (2026-10-18, stream "xform"): **the geometry is compared with the game's own** (`poc5-tb-140`'s `los_geom` and
`los_probe`, all five runs): the 8 caves blockers' 88 segments and the grid are equal bit for bit, and so are the
1,280 `HasObstacle` probes. The blockers' points are Unity's `Transform.TransformPoint` (its hierarchy walk, ported from the
player), no longer the product with the `f32` world matrix, which had put 13 of the 88 segments one ulp off ("Observer
changes requested"). Every run of the 25 captures is valid but `-122`'s tb-nearest runs and `-135`'s perturbed one, as before,
and no count of the vision range changed.
Update (2026-10-18, stream "corp", `docs/poc5/corpulence.md`): `UnitEntityView.Corpulence` is computed from the unit's prefab and
size and used where the recording has no valid read: 39,218,317 more pairs of the 111 runs are decided (54,661,049, 0
differences), 6,650,019 are left for the corpulence (96% of them lost colliders, not for want of a corpulence). The numbers and
the corpulence statements below are those of stream "visrange"; where they say the corpulence is the recording's or "not done",
read that note.

Status (2026-10-17, stream "visrange"): the sets of units in vision range (`CanBeInVisionRange`) are computed too and
checked against the recording's `vis` records, on every pair the sim can decide (a pair that an unknown corpulence or view
scale could change is left to the recording, counted): see "The sets in vision range" below. Before that (stream "vision"):
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

Round 7 (colliders of units out of the game, or after a frame the sim didn't follow, lost until a modelled update; the
last frame without the driven agent left to the recording; sampler stamps kept apart from hooked events): all 111 runs
as valid as before (`-122`/tb-nearest's tie and `-135`'s `replay-3-perturbed` excepted), 0 differences, no frame unfollowed.
The record runs decide 5,256,586 pairs (190 fewer) and leave 14,373,101 for the corpulence, 864 for the position (the
last frames of `-95`, `-96`, `-109`, `-111`, `-112`: 90 to 108 a run) and 14,325 for the scale band; all 111 runs decide
16,434,480 and leave 44,887,244, 2,592 and 45,726.

Round 8 (a radius known only while the corpulence it was established with is known and unchanged; a guessed position never
known; a unit that joins hidden has a lost collider of unknown `enabled`; no buffer evidence from a guessed capsule): all
111 runs as valid as before, 0 differences. The record runs decide 4,926,808 pairs and leave 14,702,936 for the
corpulence (the reads made stale by a buff now make the radius unknown until the unit's next update), 864 for the
position and 14,268 for the scale band; all 111 runs decide 15,452,626 and leave 45,869,264, 2,592 and 45,560.
Round 9 (a set of fewer than 512 must hold every pair decided as in; a collider first met after an unfollowed frame is
lost): the same counts, all runs as valid, 0 differences.
Round 10 (without the driven agent a row gives a position only at its own frame or while the observer saw the unit
awake): all runs as valid, 0 differences. The record runs decide 4,923,510 pairs and leave 14,705,393 for the
corpulence, 1,705 for the position and 14,268 for the scale band; all 111 runs decide 15,442,732 and leave 45,876,635,
5,115 and 45,560 (the 27 runs without the agent: 2,570,208 decided before, 2,560,314 now).
Round 11 (a unit the sim moves after `Phase::Visibility` is unknown for that frame's queries and collider; every vision
record must be consumed exactly once): the same counts, all runs as valid, 0 differences; units moved by the sim after
the phase 0 and vision records unconsumed 0 (checked on `-129`/tb-lizard and `-111`/tb-focus).

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
  mask 1 << 23)` on the entity-bounds physics scene with `r = VisionRangeMeters`, and add each hit's unit (fake units
  included: the hits aren't filtered). The set is a `HashSet` refilled after a `Clear`, so it enumerates in the overlap's
  order. `IsSleeping`'s setter (either way) and `OnDispose` clear it too.
- `EntityBoundsController.Tick` (registered just before it, also in Pause): one `SphereCollider` per entity of
  `State.EntitiesWithBounds` (`RequiresSphereBoundsCollider`: units, fake ones too, map objects, cutscene anchors; in
  the order they joined their states), on a private object in that scene with unit scale. The first tick walks the pool
  through `EntityPoolEnumerator`, which skips what isn't `ShouldBeEnumeratedByEntityPoolEnumerator` (`IsInGame` and not
  suppressed): a unit hidden since the load gets its collider only when it comes into the game (`TryAddToUpdateList`, on
  `HandleObjectInGameChanged`), after the others. Later ticks handle the update list (entities whose position,
  game state or size changed). A unit's collider is made with `Max(0.05, Corpulence * view.lossyScale)`
  (`Corpulence` is `View.Corpulence`: the prefab's `m_Corpulence` times the size scale, at least 0.5) at the origin
  (`AddComponent` before the position is set), and in the same call set to `Max(0.05, Corpulence)` if that differs by more
  than 0.05 (the collider's own scale is 1); later calls do the same against the radius it has. Its transform is set
  to `entity.Position` in every call. `EntityViewBase.UpdateViewActive` sets `enabled =
  ShouldBeEnumeratedByEntityPoolEnumerator` the moment `IsInGame` or `Suppressed` change, but returns early while the view
  fade-hides (`m_IsFadeHide`, set by `FadeHide`, which `HideUnit` with `Fade` calls): the collider keeps its state until the
  fade ends. The collider is destroyed with its view (`OnDestroy`); a disposed entity's view is destroyed at once, after
  1.8 s (`FadeOutAndDestroy`) or not at all, by its kind. `Physics.autoSyncTransforms` is off [M, the begin
  record: `physics_auto_sync` false], so a transform change reaches PhysX at the next physics step (`FixedUpdate`, before
  the frame's controllers).
- PhysX's capsule-against-sphere overlap: the squared distance from the sphere's centre to the capsule's segment against
  the squared sum of the radii, in `f32` (`Gu::distancePointSegmentSquared`; ported, not read from PhysX).

### What the sim does

(The model below is the sim's for a recording without the `bounds_*` records. With them (`poc5-tb-140`) the colliders' radii,
transforms, `enabled` flags, creation order and history are the recorded ones, nothing is left for the corpulence, the view-scale
band, the history or the fade, and this model runs as a check on the records; see "The window's start, from the records".)

A pair (an awake unit's capsule and a collider) is answered by the sim only when the things the sim doesn't know (the
view's scale, a corpulence not read, a physics step that may not have run, a fade it saw begin) cannot change the answer;
otherwise it is left to the recording (`Tri::Maybe`), counted by cause, and the recording's answer is the run's. What the
sim *hypothesizes* about the colliders' state at the window's start is a different kind of thing, listed in "Hypotheses
about the window's start" below: every decided pair rests on those hypotheses. A wrong one is found only if the sim then
decides a pair differently from the recording; a start state the recorded sets can't tell from the true one passes. `Bounds::tick` keeps each collider's transform, PhysX's copy
(taken from the transform when the frame ran a step; both candidates when that isn't known), the radius as an interval
and its `enabled`; `Bounds::overlap` answers the query in the colliders' order.
- **Population.** `scene::units_with_bounds`: the units of `State.Units`' order, fake ones too (map objects and anchors have
  colliders on another layer and are left out, which doesn't change the units' relative order). The first tick makes
  colliders for the units in the game. A unit hidden then may already own a disabled collider from before the window or get
  one when it comes into the game: its collider is *lost* (below) and its place in the order unknown
  always (sets holding it have an unknown order: the memory's `near` leaf is taken from the recording for the groups
  that use them). A unit that comes into the game later gets its collider then (at the origin until a step, appended: the
  update list's order is taken to be the pool's). A collider whose entity has left the pool is removed and **reported**
  (when its view is destroyed isn't modelled).
- **Radius.** The collider's radius changes only at a `TickEntity` the sim models (the first tick, a position change, a change
  of game state, a size write the sim saw): by the C# rule (0.05 hysteresis) from its interval before, with the corpulence
  the recording shows at that moment. Learning a corpulence later changes what the sim knows of the target, never the
  collider. The interval is `[c - 0.05, c + 0.05]` around `c` (the view's scale, which decides whether the creation radius
  `c * s` survives the hysteresis, is unknown), exact only after a size change of more than 0.1; **unbounded above after an
  update with the corpulence unknown** (a unit with no valid `corp` read at that moment: nothing bounds a corpulence, and the
  C# imposes none), until an update with a known one. So a unit that stands still keeps the radius it got at the first tick. The point estimate (scale 1, radius `c`) is kept for the margins and the
  *guess*: of the pairs the scale band alone leaves open, how many the recording answers as scale 1 would.
- **Lost colliders.** The update list takes an entity whose position, game state or size changed while it was in the
  game, and `TickEntity` runs for it at the next tick even if it has left the game since; the sim sees each unit at its
  ticks only. So once a unit is out of the game its collider's transform, radius and PhysX copy are unknown (*lost*), and
  they are re-established only by a tick the sim models after it is back (the transform at the unit's position, the radius
  by the rule from an unknown radius: the band around the corpulence then, unbounded without one) and, for PhysX, the next
  step known to run. A frame the sim didn't follow (a skipped Unity frame, a mode it doesn't model; counted as `frames not
  followed`) loses every collider, and the next tick can't tell which units it updates: a radius is re-established only
  by a later move, size write or return to the game. A collider first met right after such a frame may have been made,
  and synced, in it: it is lost too, at an unknown place in the order. `TickEntity` sets the transform only when it isn't `Vector3 ==` the unit's position (a
  squared distance of 9.9999994e-11, 1e-5 m; `orientation::vec3_eq`): a unit in the game has its collider's transform
  within that tolerance of its position after every tick (a change of it in the game puts it on the update list), and
  exactly where the sim followed the updates (an update keeps a transform that close). After a loss the transform is
  known only to the tolerance, and a pair the tolerance could decide goes to the recording (cause position).
- **Enabled.** `IsInGame` (suppression exists only in areas with parts, which the vision run refuses); a unit hidden with a
  fade (`HideUnit` with `Fade`, noted by `scene_spawn::hide_unit`), or not in the game at the first tick, has an unknown
  `enabled` whenever it is out of the game from then on (when the fade ends isn't modelled, and a fade can outlast a return
  to the game), so its pairs are the recording's (its collider is lost as well).
- **Physics step.** From the clock `fixed_phase0` + each tick's scaled `delta` (below); when the header's fields or the
  tick's frame are missing the step is unknown, and a moving unit's collider has both candidates (more than six: unknown);
  a first tick without a known step, or a unit without an observation row then, doesn't know the colliders' positions
  either.
- **Positions.** Without the driven agent a unit's position after the movement is the next frame's row; the recording's
  last frame has none, nor does a unit without rows. WrathOracle writes a unit's `u` row (when it changed) only while the
  unit is awake, and `UnitRoamingController` moves sleeping units, so the last row gives the position at a frame only when
  it is of that frame or the observer saw the unit awake then (its `awake_set`/`awake` records); a sleeping unit's position
  is unknown. The next prefix's row is where the unit stood at the vision phase only if nothing moved it later in the
  frame: the sim counts its own position writes (`note_position_write`: spawns, view replacement, companions placed,
  roaming, the movement controller), and without the driven agent a frame's comparisons wait for its end; a unit the sim
  moved after the phase has its queries and its collider's pairs of that frame left to the recording, and its collider
  lost (counted as `units moved by the sim after the phase`). (With the driven agent the positions are the sim's at the
  phase, its roaming included.)
- **Physics settings.** The step and sync model is for `Physics.autoSyncTransforms` off and `autoSimulation` on, as the
  begin record's `physics_auto_sync` and `physics_auto_simulation` say; with other values, or without them, no collider's
  PhysX position is known (counted as `frames of unmodelled physics settings`).
- **Explicit syncs.** `Physics.SyncTransforms()` flushes every transform to PhysX (all physics scenes) outside the fixed
  steps. Its callers: `Polymorph.RestoreView` (a polymorph ending), `UnitPlaceOnGroundController.Tick` when a forced place
  is pending (`Start`, which `GameMode.OnStart` calls when a mode with the controller, Default or Pause, starts after
  one without it; `RequestTick`, from scene loading and Alushinyrra's isle view), `GameActionSetIsleState` and
  `IsleStateControllerView`. In a frame where one may have run since the last vision phase (the sim deactivated or
  removed a fact with a `Polymorph` component, or a mode with the controller started after one without it) every
  collider's PhysX copy is unknown until a step known to run (counted as `frames with a possible explicit sync`).
- **Replaced views.** A polymorph's start (`Polymorph.TryReplaceView`) and end (`RestoreView`) attach a new view to the
  unit (`AttachToViewOnLoad`), positioned by copying the old one's transform: the next update of the unit makes a new
  collider (at the view's position, `==` but not necessarily equal to the unit's; a radius from the new view's scale; a
  new place in the creation and result order), and the old collider, with its last transform and `enabled`, lives
  until the old view is destroyed (now, or after a visual transition the sim doesn't follow). When the sim activates,
  deactivates or removes a fact with a `Polymorph` component, the owner's collider is lost, its place in the order
  unknown, and its pairs are the recording's for the rest of the run (cause `enabled`: which collider exists; counted
  as `views replaced by a polymorph`). The optimization dummy swap does the same (`UnitEntityData.UpdateHasDummyView`,
  from `OnIsInGameChanged` and from the sleeping controller for awake or death-revealed units): when its flag
  (`HoldingState != CrossSceneState && (Suppressed || (UnitPartOptimizeWhenHidden && !IsInGame))`; suppression is
  refused with the areas with parts, the build mode's `DisableUnitOptimization` taken to be off) differs from
  `HasDummyView` (hydrated from the save, then followed), the view is replaced; a dummy kept while sleeping depends on
  `IsDeathRevealed`, which isn't modelled, so such a swap is possible, and possible swaps count apart (`dummy view
  swaps`, `possible`; a dummy swap isn't counted among the views replaced by a polymorph). A replaced view's old
  collider may also take one of the result buffer's 512 slots (dropped after, its link gone), and the sim never knows
  when an old view is destroyed (now, at the frame's end, or after a transition): a unit replaced k times counts as k
  more possible hits for the capacity and for the short-set check. Replaced
  colliders are left out of the margin statistics.
- **Every vision record is consumed.** Each `vis`, `los`, `los_clear`, `los_geometry`, `los_geom` and `los_probe` record
  must reach its handler exactly once; one that none consumed by the run's end (inside a root rule's span the replay
  drives, whose records it skips, say) is reported (`vision records unconsumed`). A position that isn't the driven agent's or a row's is never taken as
  known: a capsule there decides nothing (cause position, and no evidence about the result buffer), and a collider there
  is lost until its position is known and an update the sim models has run.
- **Radius against corpulence.** A radius stays known only while the unit's corpulence is known and equal to the one it
  was established with at the last modelled update: a move away and back between two ticks (which the sim can't see)
  would apply a changed corpulence. Once the corpulence is unknown or different, the radius is unknown until the next
  modelled update.
- **Units that join hidden.** A unit that joined in the game and was hidden before the bounds tick still has its update
  queued, which makes its collider while it is hidden, enabled (`UpdateViewActive` ran before it existed) and appended at
  that moment: such a unit has a lost collider of unknown `enabled` and place in the order until it is back in the game.
- **Result buffer.** `OverlapCapsule` fills a buffer of 512: with more colliders possibly in reach every such pair is the
  recording's (which ones PhysX keeps is its order). A recorded set of more than 512 is reported; one of fewer than 512
  wasn't truncated, so it must hold every pair the sim decided as in (a missing one is reported); only a set of exactly
  512 may leave such pairs out.
- **Vision radius.** A unit with an `OverrideVisionRange` fact refuses the run at the start; one that gets it later is
  reported, its set left to the recording.

The replay's inputs the sim doesn't have are the recording's: each unit's corpulence, the physics clock, the awake units.
The unit positions are `Vw::pos` after the movement (the driven agent's, else the observation rows). Each frame's phase
computes every awake unit's overlap and compares it, when the frame has a `vis` record for the unit, with the set the record
gives (the last record's set plus its `add`/`rem`), and otherwise with the set the last record left (no record means no
change; the first record of a unit is always written). A difference is reported (`driven replay: vision: the vision range
of X differs ...`) and the recording's set is the one the run goes on with, as every reader here. Units put to sleep lose
their set. `UnitGroup.ExtractNearUnits` (the memory controller's `near` leaf) is the union of the group members' sets in the
overlap's order, checked against the leaf when every set's order is the sim's (`vis_near_union`, counted among
`mem_leaf_checked`'s `near`; else `mem_leaf_taken`).

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
   (the oracle's fast-forward stretches run at time scale 3, `delta` 0.05 s, which moves the phase; the fixed step is
   `fixed_delta_time`). Differences against the recordings over the 36 record runs (with the first version of the radius
   rule, a 5 cm tolerance): a step in every frame, 683 (1 to 36 a run, 543 of them one static pair of
   `-133`/tb-nearest); the clock advanced by the fixed frame time, 682; **the clock advanced by each tick's scaled
   delta, 0**. (A frame in six without a step by the begin record's phase alone is right until the time scale moves the
   phase.)
3. **The collider's radius** is `Max(0.05, Corpulence)` in every pair the recordings can test, with the view's scale taken
   as 1: the 14,325 pair-frames (record runs; 45,726 in all 109) the scale band leaves open are all answered by the
   recording as scale 1 would (0 differing), among them 33 within 1 mm of the capsule's surface (100 in all runs): a scale
   off by 1% of a 0.5 m radius would have flipped some. This is evidence, not an answer: those pairs stay the recording's.
   The corpulence is the recording's: the last `corp` read (`uc_v`, `ag_v`) in the stream before the record the phase is
   placed at (a size change applied later in the frame, in a phase after the vision controller's, stays out of the tick)
   and after the last event the sim saw that could change it (a size write, a fact or buff added or removed, a view
   replaced: the stream index it was made at; any such event invalidates the reads before it, whether or not the
   value changed back; an event made while a driven entry ran is stamped at the entry's last record, as its readers
   consume records past the entry's own index: a read and a change in one entry count as read before change; a change
   seen only by sampling (the unit's size without a size write, an `AddUnitScale` or polymorph
   fact without a fact event, a mount, which no hook sees) counts from the phase that saw it, and the sampler's own stamps
   never explain a later change); a
   unit with no valid read has none. **Size or view changes made by code the sim doesn't run produce no invalidation**: a
   corpulence known before one, and the radius computed from it, can stay wrong, and a recording that shows the same wrong
   answer would pass (see "Limits"). The party's corpulence before the first fight read, every unit
   nobody reads, and the reads after a buff came or went, are such units: 2.2 million unit-frames in the 36 record runs
   (0.29 million of them with a read made stale). **That is most of the pairs** (below).
   (Done in stream "corp", `docs/poc5/corpulence.md`: the formula below, ported and checked against every read; the rest of this
   paragraph is what it replaced.) Computing `Corpulence` would close it: the unit prefab's `m_Corpulence` (a `UnitEntityView` field the importer
   already finds in `wr-import unit-prefabs`, which writes only `m_IsInGame` today) times `GetSizeScale()` (`Descriptor.State.Size`
   against `OriginalSize`, the progression's scale percent from an `AddUnitScale` fact, the master's pet coefficient, the prefab's
   `OverrideSizeScale` component, `DisableSizeScaling` of a polymorph): an importer change, a regenerated cache, the prefab
   of each unit (`current_view_prefab`) and those rules, validated on the units that have reads; not done. The prefab root's
   `Transform.m_LocalScale` times the same size scale would give the view's scale and close the band.
4. **The enabled flag** is the sim's `is_in_game` (a unit that dies leaves everybody's set in the frame the recording
   shows). Fade-hidden views, collider creation after the first frame, hidden-at-start units revealed, a missing clock and
   destruction with the entity are modelled and unit-tested, **not exercised**: no run has a fade-hidden unit (0
   unit-frames), makes a collider after its first frame (0), leaves a pair to the recording for its position (0) or loses an
   entity.
5. **The margin** (item 3 of the task): the closest approach of any pair, over the record runs, to the capsule's surface is
   **70 um** (at scale 1; 27 um in the previous version, which decided more pairs); 4,004 pair-frames of the 10.5 million with a known corpulence and one position are within 1 cm and 100 within
   1 mm, in sets that agree (margins at scale 1). PhysX's `f32` test against the same test with the radii summed in `f64`
   (the margin is that distance less the `f64` sum): **0** of them decide differently, so the `f32` rounding doesn't matter
   for these captures (the sim decides in `f32`; the other is only logged).
6. **The order.** The recorded `near` leaves' first member's prefix gives that member's set order exactly: over five
   captures (`-111`, `-120`, `-123`, `-129`, `-136`) there is one static total order, never a conflict: the party first,
   then the other units of the area by id, that is the pool order [`State.Units.All`: the units of the cross-scene state,
   then the area's]. The sim's overlap in the colliders' order (made in pool order, a later collider appended) reproduces
   **every** recorded `near` leaf: 385,005 of 385,005 in the record runs of `-111` and later (1,218,083 in all 109 runs;
   `-95`, `-96` and `-109` have no `memory` observer), so the order is no longer an input, given the hypotheses
   below. (Why PhysX returns the colliders in creation order for this scene isn't established; it holds for every capture
   here, which have at most 21 colliders.)

### The window's start, from the records (`poc5-tb-140`), else hypothesized

The recording opens mid-session. Without the `bounds_*` records the sim hypothesizes the colliders' state at the first tick of the
window (the list below), and everything it decides rests on these: they are hypotheses, not checks, and a wrong one is caught only
when it makes the sim decide a pair differently from the recording (one the recorded sets can't tell from the truth passes). A
recording that declares the observers `bounds_init`, `bounds_tick`, `bounds_set`, `bounds_scene` and `phys_sync` (installed together:
a part of the family, or its records without the declaration, is reported and the hypotheses are used) and optionally `vis_order`
replaces them with the game's own state, read by `replay/bounds_rec.rs` into `range::Bounds::start_recorded` / `tick_recorded`
(`combat/vision/range/recorded.rs`).

**How the records are used [C]/[M].**
- `bounds_init` (one root per `GameObject` of the scene at the first tick of the window, before any controller of the tick, then
  `bounds_init_end {n, valid}`): `r` is each unit collider's first radius (exact: no scale band, no corpulence), `on` its `enabled`,
  `pos` its transform (the previous frame's end: **equal bit for bit to the unit's prefix row for all 21 units** of `-140`),
  `fade` the initial fade mark (an init collider with `fade` true marks the unit fade-hidden; none in `-140`). A root's presence is
  whether a hidden unit owns a collider (the unknown history of units hidden at the start is ended: `-140` has none hidden, so it
  isn't exercised on data), and `i` the creation order: the sim creates the colliders in `i` order and **compares it with the
  pool's** (`State.Units`' order of the units that have a root): equal for all 21 units on all five runs; a disagreement is reported.
  A record is refused (reported, the hypotheses used) unless `valid` is true, `n` is the count, the roots are numbered from 0 in
  order, their instance ids differ, a unit root has its entity, a positive radius, `on` and the units' layer (23), no other root is on
  that layer, and every field is present with its type and finite.
- The window's first `TickEntity` pass is **not** `m_UpdateAll` [M]: the controller had ticked before the window, so at tick 0 the
  update list holds the units that moved (`5052` alone in `-140`). The hypotheses' first-tick model (every unit in the game is
  updated at the first tick) was wrong in kind, harmless where nothing moved.
- `bounds_tick` (one per `TickEntity` call: the radius before and after, the transform and `enabled` after, the collider's id, whether
  the call made it): applied in the stream's order. The collider's radius, transform and (for a made collider) existence are the
  record's; the sim's rule is checked against each: the radius after is inside the interval `next_radius` gives from the radius
  before and the sim's corpulence (7,832 of 7,832: 259 exactly `Max(0.05, Corpulence)`, 7,573 within the hysteresis band of it), the
  radius before is the one the last record left, `on` is the collider's state, and the transform after is the sim's
  `Vector3 ==` rule applied to the unit's position (7,832 of 7,832). A call that threw (`x`) refuses the family.
- The update list is checked the same way: a unit in the game that the sim's model puts on the list (it moved, changed size, came
  back) must have a `bounds_tick`, and one that has a `bounds_tick` must be on the sim's list: **205,465 of 205,465 unit-frames in the
  game agree** on a run (0 disagreeing; 30,008 unit-frames out of the game are not checked, as an update queued before may still
  run). A disagreement is reported, so a dropped or invented `bounds_tick` is found.
- `bounds_set` (`active`: `UpdateViewActive` changed `enabled`; `pos`; `gone`: destroyed at the frame's end; `link`/`entity`): queued
  as the replay reaches each record (so a death's `active` is where the sim's `IsInGame` changes), applied before the frame's query
  when it came before the frame's vision phase and before the next one's when after it. `-140` has 4 `active` (units leaving the
  game: the collider's `enabled` is the sim's `IsInGame` in 235,473 of 235,473 unit-frames, a unit with a fade-hiding view being
  the unknown case) and 2 `link` with no collider (a view without one). A write that names a collider the sim hasn't seen made, or
  contradicts its state (`on0`, `pos0`), is reported; a relink that changes the entity, and an `entity` record that isn't the link of
  a collider the frame's tick made, are reported as not modelled.
- `bounds_scene` (the scene remade: every collider destroyed unseen) in a window refuses the family, as does an `x`. None in `-140`.
- `phys_sync` (every `Physics.SyncTransforms()`, and DollRoom's `Simulate`): replaces the "possible explicit sync" guess. A recorded
  sync in a frame gives each collider both transforms it can have (the one before the frame's tick and the one after: the order
  inside the frame isn't known) as PhysX's candidates; one after the frame's query gives the transform as it stands. The sim's own
  flag (`Polymorph` ended, the place-on-ground controller started) is only counted against it. None in `-140`'s five runs, so this is
  unit-tested only (`recorded.rs`).
- `vis_order` (a unit's `CanBeInVisionRange` enumeration order, whenever it changes; the first of a unit is always written): the order
  of a unit's set is the recorded one. It must be a permutation of the unit's `vis` set; every pair it orders must agree with
  every earlier record's (one static order of the colliders, a conflict is reported: a swapped record is found if the pair was
  seen before); and the sim's own (the overlap in the colliders' creation order) is compared: **equal in 73,534 of 73,568 sets of
  `-140`; 34 differ**, all one record: at tick 8019 `558A`'s set, which held `527A` (collider 62), gained `529A` (collider 63) and
  enumerates **`529A` before `527A`**, and keeps that order for 34 frames. So PhysX's result order is **not** always the colliders'
  creation order: the sim takes the recorded order (counted, not reported: the sim doesn't port PhysX's pruner) and the sets whose
  order the sim couldn't give (a collider of unknown place) are settled by it (none in `-140`). The memory's `near` unions are
  built from the recorded orders.

**What each hypothesis became.**
- *The creation order is the pool's*: the recorded `i`, equal to the pool's for the 21 units (the order the sim now uses is the
  recording's). Not a hypothesis any more; the *result order = creation order* assumption behind it is contradicted once (above).
- *No fade at the first tick for a unit in the game; a unit not in the game is possibly fade-hiding*: replaced by `fade` and by the
  `enabled` flags themselves. The collider's `enabled` is the recorded state, and the sim's `IsInGame` (with its fade marks) is checked
  against it.
- *The positions PhysX has at the window's first frame are the prefix rows when the frame ran a step*: the collider's transform is
  `bounds_init`'s `pos` (equal to the prefix rows for all 21 units); PhysX's copy of it is still the step model: the transform at the
  frame's start if the frame ran a step, unknown if it didn't, **still hypothesized (the clock)**.
- *The radii at the first tick come from a first `TickEntity` pass (`m_UpdateAll`)*: `bounds_init`'s `r`, exact. Every `bounds_tick`
  after it has `r0 == r` in `-140` (no radius ever changed), which the sim's rule gives.
- *The physics clock `Time.time - Time.fixedTime`* (`fixed_phase0` plus each tick's scaled `delta`): **still an inference** (109 runs
  equal; nothing records the steps a frame ran). It decides which frames' PhysX copies are the previous tick's transforms.
- *Units hidden at the first tick*: ended by presence in `bounds_init` (not exercised on data). *Pool order for colliders made
  later*: the order of the `made` `bounds_tick`s, a collider at the origin until a step (unit-tested only: no run makes one).

**Hypothesized when a recording lacks the family** (every capture before `-140`; the text of the sections above and below is for
this case, and for the checks of the recorded one):
- the creation order of the colliders made before the window is the pool's (checked by the memory's `near` leaves: 385,005 equal in
  the record runs of `-111` and later, 1,218,083 in all 109 runs);
- no fade is in progress at the first tick for a unit that is in the game (a unit *not* in the game is treated as possibly
  fade-hiding: its `enabled` is unknown until the sim sees it in the game again, cause `enabled`);
- the positions PhysX has at the first frame are the observation rows at the frame's prefix, when the frame ran a step;
- the radii at the first tick come from a first `TickEntity` pass with the corpulence the recording shows then (none in these
  captures: unbounded until the unit's next update);
- the physics clock (above);
- units hidden at the first tick may already own a disabled collider (lost, order unknown); pool order for colliders made later.

### What the sim leaves to the recording, by cause

**With the `bounds_*` records (`poc5-tb-140`) nothing is left for the corpulence, the view-scale band, the history of hidden units,
the fade or the position of a collider's transform**: the radii and transforms are exact, so every pair is decided but where PhysX's
copy of a transform is unknown (cause position: a frame with no known step, or a capsule whose position the sim doesn't have). On
`-140`'s five runs all 6,795,850 pairs are decided (21,470 were the scale band's), 0 left for any cause, and no set has a pair left.
What the sim still takes from the recording there is the awake units, the unit positions (the driven agent's), the physics clock
and, for the `near` unions, the sets' orders (the recorded `vis_order`).

Without them, every item is a pair left to the recording (counted in the run's line). In the 36 record runs the sim decided 5,256,776
pairs (given the hypotheses; each equal to the recording's) and left 14,388,100 to the recording:
- **Corpulence unknown, or a collider radius not updated since** (no valid read at the collider's last update, or a read made
  stale by an event the sim saw, or a unit with a producer it can't follow: `AddUnitScale`, a polymorph, a rider): 14,373,775 pairs. Every set has some (the units nobody
  reads are in every observer's query), so **no set is wholly decided by the sim**: the 1,003,446 sets each equal the
  recording's on the pairs the sim decided. The decided part is where the flips are: the pairs of units whose corpulence is
  read. A read stays valid until the sim sees an event that could change the unit's corpulence (above); the reads before it
  are not used.
- **View scale** (the band +-0.05): 14,325 pairs, all equal to the scale-1 answer.
- **Position, enabled (a unit not in the game at the start, or fade-hidden), history of hidden units, destruction**: 0
  pairs for lost colliders (no run has the case: all 21 units are in the game at the first tick, and no frame goes
  unfollowed); the position pairs counted are those of the recording's last frame in the runs without the driven agent. A
  missing or discontinuous physics clock makes every later step unknown, for good.

### Results [M]

`wr-combat replay <capture>/fixtures/<fight> --run <run>` prints the counts after `vision range:`. Record runs (a fight's
replays have the counts of their own length). **Every run of all 24 captures** (`-95`, `-96`, `-109`, `-111` to `-117`,
`-120`, `-122` to `-127`, `-129`, `-131` to `-136`: 109 runs, on the result of merging `poc` at 88bba63) is as valid as
before (only `-122`/tb-nearest's initiative tie is not), with 0 differences between the sim's sets and the recording's.
Which part the sim decided (given the hypotheses above) and which the recording: each set has pairs left to the
recording (the unread units), so the columns say what was decided:

| capture / fight (record run) | sets compared | changes | pairs decided given the hypotheses, equal | left: corpulence | left: enabled | left: scale band (scale-1 answer equal / differing) | near-unit unions equal |
|---|---|---|---|---|---|---|---|
| -95 tb-focus | 20,641 | 53 | 114,606 | 314,318 | 0 | 21 (21 / 0) | 0 |
| -95 tb-nearest | 20,817 | 56 | 114,534 | 316,740 | 0 | 1026 (1026 / 0) | 0 |
| -96 tb-hold | 17,394 | 55 | 90,444 | 268,244 | 0 | 16 (16 / 0) | 0 |
| -109 tb-focus | 15,050 | 56 | 71,242 | 235,865 | 0 | 19 (19 / 0) | 0 |
| -109 tb-nearest | 13,755 | 52 | 58,011 | 220,235 | 0 | 18 (18 / 0) | 0 |
| -111 tb-focus | 18,612 | 61 | 95,934 | 287,746 | 0 | 42 (42 / 0) | 7,400 |
| -111 tb-nearest | 20,805 | 59 | 112,530 | 319,301 | 0 | 37 (37 / 0) | 8,087 |
| -112 tb-focus | 22,918 | 62 | 125,469 | 348,373 | 0 | 995 (995 / 0) | 8,969 |
| -112 tb-nearest | 15,109 | 55 | 74,486 | 237,908 | 0 | 17 (17 / 0) | 5,924 |
| -113 tb-focus | 14,532 | 50 | 60,678 | 236,016 | 0 | 20 (20 / 0) | 6,047 |
| -113 tb-nearest | 13,019 | 52 | 53,378 | 209,398 | 0 | 22 (22 / 0) | 5,567 |
| -114 tb-focus | 21,320 | 46 | 99,094 | 336,709 | 0 | 19 (19 / 0) | 8,649 |
| -114 tb-nearest | 18,574 | 56 | 79,243 | 293,822 | 0 | 29 (29 / 0) | 7,916 |
| -115 tb-focus | 15,663 | 48 | 77,125 | 244,046 | 0 | 13 (13 / 0) | 6,365 |
| -115 tb-nearest | 18,677 | 52 | 81,259 | 295,961 | 0 | 22 (22 / 0) | 7,766 |
| -116 tb-hold | 19,214 | 52 | 86,662 | 303,902 | 0 | 24 (24 / 0) | 7,802 |
| -117 tb-solo | 18,734 | 52 | 84,960 | 299,390 | 0 | 21 (21 / 0) | 7,412 |
| -120 tb-cast | 18,693 | 46 | 82,333 | 297,366 | 0 | 15 (15 / 0) | 7,598 |
| -122 tb-focus | 18,260 | 53 | 81,479 | 289,643 | 0 | 33 (33 / 0) | 6,592 |
| -122 tb-nearest | 19,336 | 47 | 87,550 | 308,181 | 0 | 99 (99 / 0) | 7,635 |
| -123 tb-chain | 36,097 | 90 | 160,124 | 533,068 | 0 | 95 (95 / 0) | 15,676 |
| -124 tb-chain | 37,531 | 84 | 171,097 | 548,760 | 0 | 327 (327 / 0) | 15,676 |
| -125 tb-focus | 24,357 | 47 | 115,604 | 384,721 | 0 | 25 (25 / 0) | 9,884 |
| -125 tb-nearest | 14,390 | 54 | 57,961 | 232,452 | 0 | 299 (299 / 0) | 5,801 |
| -126 tb-focus | 15,918 | 57 | 67,513 | 255,713 | 0 | 15 (15 / 0) | 6,380 |
| -126 tb-nearest | 19,581 | 57 | 105,137 | 301,152 | 0 | 757 (757 / 0) | 7,619 |
| -127 tb-tie | 20,422 | 55 | 111,199 | 312,582 | 0 | 27 (27 / 0) | 7,949 |
| -129 tb-lizard | 87,004 | 185 | 437,609 | 1,096,430 | 0 | 4200 (4200 / 0) | 38,016 |
| -131 tb-lizard | 110,856 | 128 | 856,711 | 1,281,395 | 0 | 1197 (1197 / 0) | 50,813 |
| -132 tb-focus | 21,824 | 62 | 119,385 | 333,254 | 0 | 994 (994 / 0) | 8,489 |
| -132 tb-nearest | 16,091 | 52 | 69,896 | 256,999 | 0 | 19 (19 / 0) | 6,734 |
| -133 tb-focus | 29,145 | 64 | 171,397 | 435,858 | 0 | 43 (43 / 0) | 11,063 |
| -133 tb-nearest | 16,254 | 51 | 81,013 | 251,174 | 0 | 557 (557 / 0) | 6,590 |
| -134 tb-lizard | 88,670 | 179 | 473,305 | 1,120,196 | 0 | 1305 (1305 / 0) | 39,141 |
| -135 tb-nearest | 21,826 | 53 | 123,556 | 334,390 | 0 | 35 (35 / 0) | 8,097 |
| -136 tb-lizard | 82,357 | 160 | 404,252 | 1,032,467 | 0 | 1922 (1922 / 0) | 37,348 |
| all 36 record runs | 1,003,446 | 2,441 | 5,256,776 | 14,373,775 | 0 | 14,325 (14,325 / 0) | 385,005 |

All 109 runs: 3,146,005 sets compared, 7,566 of them changes (each equal to its `vis` record on the pairs the sim decided),
16,412,252 pairs decided and equal, 44,781,402 left to the recording for an unknown corpulence, 45,726 for the view-scale
band (all equal to the scale-1 answer), none for a position, `enabled` or history. The no-record frames show no
change on the decided pairs. 1,218,083 `near` leaves equal given the creation-order hypothesis (the sets' orders are the sim's). Earlier versions decided more by
assuming more: the unknown corpulences as 0.5 to 4.0 and the scale as exactly 1 (18.9 million pairs decided, 147,756
left), then the corpulence valid until a size sample changed (19.9 million decided, 41.2 million left).

Damaged records (`computed_line_of_sight_replays_are_valid_and_damage_is_reported`, now 17 cases on `-111`/tb-nearest,
each reported): the 14 above (a `vis` record missing a unit or removing one that wasn't there are now reported by the
overlap too) and a change with no record (a `vis` record deleted), a unit of known corpulence more than 14 m away added to a set, a `vis` record inside a driven root span (unconsumed).

**`poc5-tb-140` (tb-lizard, the `bounds_*` records and `vis_order`), the same sweep before and after this stream** (five runs, all
valid both times; per run the counts are the same to the pair for the four replays, which have the record run's length):

| `-140`, five runs | before (hypotheses) | after (the records) |
|---|---|---|
| sets compared / differences | 367,840 / 0 | 367,840 / 0 |
| sets with a pair left to the recording | 21,420 | 0 |
| pairs decided, each equal to the recording's | 6,774,380 | 6,795,850 |
| left: corpulence / position / enabled / result buffer | 0 / 0 / 0 / 0 | 0 / 0 / 0 / 0 |
| left: view-scale band (scale-1 answer equal / differing) | 21,470 (21,470 / 0) | 0 |
| vision records unconsumed | 0 | 0 |

The record run alone: 1,354,876 pairs decided and 4,294 for the band before, 1,359,170 and 0 after. The records of a run (each
consumed once): 64 `bounds_init` roots and their end, 7,832 `bounds_tick`, 6 `bounds_set` (4 `active`, 2 `link`), 162 `vis_order`;
no `bounds_scene` and no `phys_sync`. Checks (a run): 21 unit colliders in the pool's creation order; 21 start transforms equal to the
prefix rows; 7,832 radii inside the sim's rule (259 exactly `Max(0.05, Corpulence)`) and 7,832 transforms equal to its
`Vector3 ==` rule; the update list in agreement in 205,465 unit-frames (0 disagreeing); the collider's `enabled` equal to the sim's in
235,473 unit-frames; 73,534 set orders equal to the creation order and 34 not (the one record above).
`-140`'s `vision range:` line says "decided given the window's start hypotheses": with the records read "given the records and the
physics clock" (the line is shared with the captures that have no records, and `vision bounds records:` /
`vision_order:` lines follow it).

Damaged records (`bounds_records_replace_the_start_hypotheses_and_damage_is_reported`, `-140`/tb-lizard, 17 cases, each reported):
a `bounds_init` root without a field, with a wrong count, a wrong radius, missing, swapped with another (the creation order), disabled,
or moved; a `bounds_tick` that threw, with a radius or transform the sim's rule can't give, a wrong radius before, or a collider
nobody made; a `bounds_scene` mid-window; a `bounds_set` the collider contradicts (`on0`) or that names an unknown collider; a
`vis_order` swapped or a unit's first one missing; and a deleted `bounds_tick`. A `phys_sync` inserted mid-window is valid and counted.

### What stays recorded

- The awake units (`aa_awake`; `-95` and `-96` have none: the sim's own, with the units it can't establish settled by
  the `vis` records' observers), as everywhere.
- The physics clock (`fixed_phase0` of the begin record, each tick's `delta`, `fixed_delta_time`), which decides which frames' PhysX
  copies are the previous tick's transforms: **the number of fixed steps each frame ran** is not recorded (see below).
- Unit positions: the recording's rows without the driven agent, as before.
- Without the `bounds_*` records (every capture before `-140`): the colliders' state at the window's start (the hypotheses above),
  each unit's `Corpulence` (the `corp` reads, now also the sim's) and the colliders' positions at the first frame (the prefix rows).
- With them: nothing of the colliders but what is listed above. The orders of the sets are the recorded ones; PhysX's result order
  is not the sim's.
- Observer request, not yet made (none is needed for these captures to be valid): in WrathOracle's physics phase (`PhysicsPhase`)
  one record per tick, `phys_steps {n}`: the number of fixed steps (`FixedUpdate` rounds) that ran before the tick's controllers.
  It would replace the clock inference, the last hypothesis under the decided pairs, and end the frames whose PhysX copy is unknown.
  A second, to settle PhysX's result order: `OverlapCapsule`'s raw hit order (the colliders' ids as the buffer returns them) before the
  controller's filter, for one unit per tick, would show whether the pruner's order is a function of the structure's history.

## Observer changes requested

**The blockers could not be checked against the captures before `poc5-tb-140`.** The caves' 8 `FogOfWarBlocker`s are thin walls
(cubes scaled 10 x 4 x 0.001) between x 120 and 145, z 58 and 94, away from the fights: the 4,805 recorded cache misses of
`-111`, `-114`, `-123`, `-129` and `-136` (record runs; a miss is the segment from the owner's sample to the target) stay at
least 38.9 m from every wall [M], so no recorded read has a blocker near its segment, and every recorded `HasLOS` that is
false is false for the vision radius or the range set. What the 800,000 reads establish for the geometry is that the imported one never blocks a segment the
game found clear; its DDA, intersection test and rounding are checked by unit tests against a brute-force test, not
against the game. Two things stayed open for the blockers themselves, and one recording closes both:

1. `Transform.TransformPoint`'s rounding. **Settled** (stream xform, with `-140`): the importer first multiplied the point by
   the `f32` world matrix glam built (`m0 x + m4 y + m8 z + m12`), which is not what Unity does; 13 of the 88 recorded
   segments differed, by one ulp in one coordinate (x 125.559845 against 125.559853). Unity's `Transform.TransformPoint`
   is a walk up the hierarchy (the player's `0x6744c0`: per level `v = p * s`, then `t + ((v + c0 vx) + (c1 vy + c2 vz))`
   with `c0..c2` the rotation matrix less the identity), ported as `wr_phys::boxes::transform_point` for the boxes and
   checked bit for bit against the player on a million generated hierarchies (`docs/poc5/physics.md`, "Boxes");
   both `FogOfWarBlockerPolygon.TransformedPoints` and `FogOfWarBlocker.BuildPointFromMesh` call it, so `wr-import
   los-pack` now uses the same function over the GameObject's local transforms (`SceneData::transform_chain`). The
   product with the matrix of the levels' `f32` matrices differs from the walk in about four of five generated
   three-level hierarchies (`wr-phys`'s `transform_point_gives_the_players_values` holds three of the player's values),
   so neither the matrix nor a double-precision sum was a candidate: the importer's hedge (`ambiguous_points`) is gone and
   the line-of-sight file's format is 2. Checked against the game: **all 88 segments of `-140`'s `los_geom` equal**
   on its five runs, bit for bit, and its 1,280 `HasObstacle` probes;
2. the grid's bounds and the registration: **compared by the same record** (the grid equals the game's on all five runs).

**Added 2026-10-17 in the main session** (`LosProbe.cs`, observer `los_geom`, written right after the begin record
next to `phys_probe`; gpt-6.1-sol: two rounds, no findings left; recorded in `poc5-tb-140`): `los_geom` and `los_probe` as
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

## Limits of the model

Each of the following can only produce a false report (the sim and the recording disagree), except where the item says it
can also be absorbed:

- A group read over several entries from before the window leaves them unresolved when the aggregate is true; a later
  contradictory aggregate isn't caught (9,336 of 1.49 million group reads touch such entries; every one agrees with a
  fresh computation).
- `vis_group` takes a miss pending for a later member and the same target as that member's read (an earlier unmodelled
  miss on the same pair could be mistaken for it: only in a capture with unmodelled readers and then a false report).
- The clears are matched by owner, not in `AwakeUnits` order (independent caches).
- `CanBeInVisionRange` is the sim's on the pairs it can decide, but its inputs aren't: the corpulences (read by the fights
  only), the physics clock and the awake units are the recording's, and the colliders' state at the window's start is
  hypothesized (above) in a recording without the `bounds_*` records. PhysX's result order is taken to be the colliders' creation
  order, which every recorded `near` leaf agrees with (385,005 of them) and `-140`'s `vis_order` in 73,534 of 73,568 sets; it
  is **not** the order in the other 34 (one record: a collider created later enumerated before an earlier one), so the order is
  the recorded one where a recording has `vis_order`, and the creation order, unchecked, in every older capture (PhysX's pruner is
  not ported; a scene with more colliders than 21 could order differently). The capsule test is PhysX's `f32` distance, which agrees with the same test in `f64` on all 10.5 million
  pairs here.
- **With the `bounds_*` records, what can still be absorbed** is only PhysX's copy of the transforms: the physics clock (which
  frames ran a step) and the order of a sync inside a frame (both candidates are kept). The radii, transforms, `enabled` flags,
  history and creation order are the game's; a record the sim's model contradicts, a dropped or invented `bounds_tick`, an `x`, a
  remade scene and a record no phase consumed are reported. Not modelled and reported: a collider relinked to another entity,
  and an `entity` record that isn't a made collider's link. Unexercised on data (unit-tested only): a collider made in the window,
  a destruction, a position write, a sync, a unit hidden at the start.
- **Can be absorbed (an equally wrong recording would pass) in a recording without them:** the start-state hypotheses (above); the corpulence
  inference: a size or view change made by code the sim doesn't run (an unported action, a mount change outside the sampled
  check, a polymorph it doesn't see) produces no invalidation, so a corpulence read before it, and a radius computed from
  it, can stay wrong while the recording (whose sets follow the true radius) disagrees only where a pair crosses the
  boundary; the physics clock (an inference that the decided pairs confirm only where a pair crosses). A wrong radius shows
  as a reported difference when a pair near the boundary disagrees, and is silent otherwise.
- **Can be absorbed, besides:** a corpulence change the sim's inference doesn't see (above) combined with an update it
  can't see (a move away and back between two ticks): the sim keeps the radius because the corpulence it knows didn't
  change. Any change it knows of (a value, or the corpulence becoming unknown) makes the radius unknown until a modelled
  update (a hide and a return between two ticks is counted by `set_is_in_game` and is one).
- **Can be absorbed, besides (without the driven agent):** a position write after the vision phase that the sim doesn't
  run (an unported ability or action, a cutscene's move): the next prefix's row then stands for the unit's position at
  the phase although it isn't. The sim's own writes are counted and fail closed (above); the others are assumed absent.
- **Can be absorbed, besides:** a `Physics.SyncTransforms()` the sim can't see: a polymorph ended by code it doesn't
  run, the isle actions and view (Alushinyrra only; the action isn't ported), a place-on-ground request from a scene
  load (outside the window). They are assumed absent.
- **Only a false report:** the memory and line-of-sight items above.
- **Neither (the pairs are the recording's, counted):** the result buffer (more than 512 colliders possibly in reach; an
  impossible set size is reported); the bounds queue and fades (a collider of a unit out of the game, or after a frame
  the sim didn't follow, is unknown until a modelled update reaches PhysX); the last frame without the driven agent.
- The blockers' points and the grid's bounds are compared with the game's own (`-140`'s `los_geom`) on the caves' 8 blockers only; the
  polygon blockers of other areas use the same `TransformPoint` but have no recording.
- Without the driven agent (`-95`, `-96`, `-109`, `-111`, `-112`) a unit's position is the recording's rows, and in the
  recording's last frame the last prefix's (counted in `last_frame_reads`; checked against the leaves like any other).

## Rebuilding after merging

`wr-rules-data` (once per install; the area bounds table gains the fog of war bounds; an old table leaves line of sight
a recorded input, reported in the run's `vision:` line), then per area `scripts/scene-cache.sh <name> <guid>` (now writes
the line-of-sight file too) or just `wr-import los-pack --area <name> --guid <guid>`.

