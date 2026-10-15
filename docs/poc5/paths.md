# POC-5: turn-based paths

Status (2026-10-15, twenty-second session, stream C of `docs/poc5/movement.md` step 3): the movement agent's
turn-based paths are computed from the fight. On **`poc5-tb-114`**, recorded with this stream's observer `paths`, every
path is exact: **129 requests, 87 of them with line of sight answered from the recording, 440 deliveries, 1,206 graph
events and 2,076 `AllyBlocked` reads equal, 6 runs**. On `poc5-tb-100` to `-103` (before the observer) every recorded
delivery is equal too (291 requests, 345 deliveries, 24 runs), 246 of them with their line-of-sight answer taken from
the recorded end. Tags as elsewhere: [M] measured, [C] from the decompiled code, [I] inferred.

## What the game does [C]

**The grid in turn-based combat** (`TurnBased.Controllers.CombatController`):

- `HandleCombatStart` sets `gridGraph.erosionOnlyAroundCellsWithTag = 31` (never reset: it stays for the session) and
  runs `Clear`, which reverts every unit's object and calls `UpdateNavigationGridTags`.
- `StartTurn(unit)`: with `CurrentTurn` already the new turn, for each unit of `m_Units` in order, its previous
  `GraphUpdateObject` is reverted (`RevertNavmeshForUnit`: the same object, `setDynamicWalkability` and
  `setAllyBlocked` flipped, `setTag` 31 to 0, queued again), and, unless it is the acting unit or its mount or the
  acting unit's agent has avoidance off, a new object is prepared and queued (`PrepareUnitNavmeshUpdateObject`):
  `Bounds(unit.Position, (corp * 2 + grow, 10, corp * 2 + grow))` (size zero when the unit's own avoidance is off),
  graph mask 2, no physics. An **enemy** of the acting rider blocks: `DynamicBlocked += 1` and tag 31; an **ally**
  marks: `AllyBlocked += 1`, its box grown by the acting unit's (or mount's) corpulence. Then
  `UpdateNavigationGridTags`.
- `UpdateNavigationGridTags`: `erodeIterations = 0`, `FlushGraphUpdates` (every queued object applied now, so with no
  erosion), then, when the turn's rider is in combat and alive, `erodeIterations = Rider.View.GetSizeInCells(grid)`
  (`Mathf.RoundToInt(Corpulence / nodeSize)`) and `ErodeWalkableArea()` over the whole grid.
- `RemoveUnit` (a unit leaving combat, dying, destroyed): revert, `UpdateNavigationGridTags` (a rider who is leaving
  isn't in combat: no erosion, `erodeIterations` stays 0), then, for the acting or the next unit, `ChooseNextUnit`
  before the unit leaves `m_Units` (so a sort or a turn start inside it still sees the unit).
- `HandleCombatEnd`: `Clear`, then another `UpdateNavigationGridTags` with no turn.
- `UpdateNavigationGrid` (each combat tick): navmesh cuts (none here), and a timer that subtracts the absolute
  `Time.unscaledTime` (WrathOracle's `nav_clock`): from 0 it fires `UpdateNavigationGridTags` once, then stays
  negative.

**A graph update** (`AstarPath.UpdateGraphs`, `GraphUpdateProcessor`, `GridGraph.UpdateArea`): objects are processed in
queue order. The affected rectangle comes from the bounds (`GraphTransform.InverseTransform(Bounds)`, eight corners,
`Mathf.RoundToInt(min - 0.5)`); a cell changes if `Bounds.Contains` holds for its position (3D, so within ±5 m of the
unit's height) and, when walkability may change, for its XZ position in the flattened box (`bounds.min/max` set with
`SetMinMax`, extents y 1). Then `Walkable = WalkableErosion`, `Apply` (`DynamicBlocked`, `AllyBlocked`, tag),
`WalkableErosion = StaticWalkable`. With erosion 0 the connections of the rectangle grown by one are recalculated
(`CalculateConnections`: four axis neighbours, the diagonals with `cutCorners`, `IsValidConnection` with `maxClimb`
through `GraphTransform.up`); with erosion (an object processed outside a flush, e.g. a revert queued by `RemoveUnit`
before `UpdateNavigationGridTags` flushes it, which here never happens) the original's erosion branch, ported as is
(it can clear a cell's static walkable flag through `TmpWalkable`). `GraphNode.Walkable` is the flag and
`DynamicBlocked == 0`; the connected components are recomputed after each batch (`EnsureValidFloodFill`).

**Tag erosion** (`ErodeWalkableArea`, `erosionUseTags`): the first iteration gives `erosionFirstTag` to every walkable
cell missing an axis connection towards a cell tagged `erosionOnlyAroundCellsWithTag` (31 in combat: the enemies'
boxes) and 0 to the rest (cells tagged 31 keep their tag); each further iteration spreads `first + i` to the axis
neighbours (through connections) still untagged or tagged higher.

**The request** (`UnitMovementAgent.PathTo`/`FindPath` in turn-based combat ->
`SeekerWithClearance.StartPathWithClerance` -> `ABPathWithClearance.Construct`): the seeker's clearance
`(corp * 2, 0, corp * 2)` (`FindPath`: zero when the agent's `m_AvoidanceDisabled` field is set), tags from
`ConstructTags()` (`int.MaxValue` minus the erosion tags `first .. first + size - 2`, `size` capped by the grid's current
`erodeIterations`: tag 31 is never traversable), graph mask 2, and the ending condition: `LosPathConditionForGrid`
when an approach radius is given (`PathTo` always; `FindPath` clamps a negative one to 0), else
`MaxDistanceEndingCondition`; the maximum distance is `6 * CurrentSpeedMps`.

**The search** (`ABPathWithClearance`, an `XPath` and its own `ITraversalProvider`): `Prepare` is `ABPath`'s
(`PathNNConstraint`, whose `constrainAllyBlocked` keeps start and end off ally-blocked cells; the default traversal
check), then the provider: cells within `Bounds((Vector3)startNode.position, clearance)` (height 0, so only cells at
the start's exact height; `(Vector3)Int3` scales by `0.001f`, a little over a thousandth, so a cell on the box's lower
face can be inside and one on its upper face outside) are traversable when only statically walkable. `TargetFound`: the end node, or a node at
least the maximum distance from the original start, or (LOS condition) a node not ally-blocked within the approach
radius (`SqrMechanicsDistance` from the node, or the original start for the start node, to the original end) whose line
of sight is clear (`LineOfSightGeometry.HasObstacle(from, originalEnd, fudge)`). `ModifyHScore` subtracts
`(uint)(approachRadius * 1000f)` from the heuristic. **Modifiers** (`Seeker.RunModifiers`, unless the path failed):
`StartEndModifier` (original start), the second `FunnelModifier` (the agent's own one is disabled in turn-based
combat), `RaycastModifier` with graph raycasting.

## The port

- `crates/wr-nav`: `update.rs` (Unity's `Bounds`, managed except the native `Contains`; `IntRect`;
  `GraphUpdateObject`; `GridGraph.UpdateArea` with both erosion branches, `CalculateConnections`,
  `ErodeWalkableArea`; `World::apply_graph_updates`; the grid's state hash WrathOracle's `nav_tags` records);
  `astar.rs` (`ABPathWithClearance`: the provider, `Ending::{Ab, MaxDistance, Los}`, `ModifyHScore`, line-of-sight
  answers through a callback that may say "unknown", which stops the search); `query.rs`
  (`World::turn_based_path`: search and modifiers); `world.rs`/`nearest.rs` (`DynamicBlocked`, `AllyBlocked`,
  `StaticWalkable`, `constrainAllyBlocked`); the grid's settings the port needs (`erosionUseTags`,
  `erosionOnlyAroundCellsWithTag`, `cutCorners`, `maxClimb`, and `GraphTransform.up` beside `GraphCollision.up`:
  `IsValidConnection` uses the first, `MultiplyVector(up).normalized`; the funnel's portals the second, a difference
  of transformed points).
- **The nav file is version 2** (the new settings): rebuild it (`scripts/scene-cache.sh`, or `wr-import nav-pack
  --area prologue_caves_1 --guid bad465e5404604346bdf71ef50b3299a`); version 1 files are refused.
- `crates/wr-sim/src/combat/paths.rs`: `CombatNav` (`CombatController`'s side: `start_turn`, `revert`,
  `update_navigation_grid_tags`, `process_pending`, `construct_tags`, with a log of every object queued and applied and
  every tag refresh, in WrathOracle's `paths` shape), `AgentRequest::to_request` (`PathTo`/`FindPath`'s request),
  `size_in_cells`, `seeker_clearance`, `max_distance`.

## What is exact [M]

- The baked grid rebuilt from its walkability: `CalculateConnections` on all 72,800 cells gives the cache's
  connection flags, and tag erosion with the serialized `erodeIterations` (2) gives its tags, on every cell
  (`update::tests::connections_and_erosion_rebuild_the_baked_grid`, ignored: needs the nav file).
- `Bounds.Contains` against the player's `BoundsScripting::Contains` (found through the icall table: the stub
  `Bounds.Contains_Injected` at 0x7ce45c calls 0x4b4fa4): 1,000,000 inputs (faces, their neighbouring floats, zero
  extents, NaN) bit-identical (`native_check`).
- The oracle's idle queries still 3,387 of 3,387 exact on `check-1` and `oracle-3` (`wr-import diff --nav-file`).
- **The recorded paths** (`combat::paths::tests::every_turn_based_path_is_computed_bit_for_bit`, ignored; captures
  from `WR_PATH_CAPTURES`, default `poc5-tb-103,poc5-tb-114`). The checker replays the fight's graph side from the records:
  `m_Units` (`combat_start`, `sort`, joins, leaves; checked against every `choose_next`), positions (`ag_ctl_end`,
  `u` rows), corpulences (`corp` taps), the agents' avoidance flag, `IsEnemy` (the sim's, on the hydrated save), turn
  starts and clears, removals, the navigation timer (a replay reads its recording's clock tape and writes no
  `nav_clock`, so each read is placed where the recording made it, before the same record of the same tick; its begin
  record writes the game's timer before WrathOracle sets the recording's); for each `ABPathWithClearance` request (`ag_req_end`'s `rq`) it
  builds the request from the call's arguments (`ag_req`), checks it against the game's (clearance, tags, graph mask,
  ending condition and its parameters, start), computes the path and compares every delivery: the raw `vectorPath`
  (an `ag_pathdef` written before the return), the callback's (`ag_done`, an `ag_pathdef` written by it, the AI's
  `ScorePath` tap), with `error` and `CompleteState` (a request `PathTo` keeps and reports again is checked once:
  15 such reports in the four captures); a `FollowPrecomputedPath` after a `FindPath` whose result was
  compared must be one of that unit's paths computed since its turn started or its last follow, or, after the path
  visualizer's `FindPath`, its replacement path (below). With observer `paths` it also pairs every `nav_guo`,
  `nav_update` and `nav_tags` (the grid hash) with `CombatNav::log`, takes line of sight from `path_fin`, compares
  `searchedNodes`, and checks each `AllyBlocked` the movement simulation reads (`ag_v near`) against the grid.

| Captures | Runs | Requests compared | Exact | of which LOS recorded | LOS from the end | Deliveries equal | `FollowPrecomputedPath` |
|---|---|---|---|---|---|---|---|
| `poc5-tb-114` | 6 | 129 | 129 | 87 | 0 | 440 / 440 | 15 computed, 18 visualizer |
| `poc5-tb-103` | 6 | 63 | 9 | 0 | 54 | 78 / 78 | 21 computed (AI), 9 unchecked |
| `poc5-tb-100` to `-103` | 24 | 291 | 45 | 0 | 246 | 345 / 345 | 63 computed (AI), 75 unchecked |

  `poc5-tb-114` also: 1,206 graph events (objects queued, applied with their erosion, 21 tag refreshes with the grid's
  hash) and 2,076 `AllyBlocked` reads equal; 57 turn starts, 36 removals. What isn't exact, with its cause:
  - **18 `FollowPrecomputedPath`s on `-114` (3 a run) are the path visualizer's replacement path**
    (`PathVisualizer`, for the party the fixture driver moves through the UI): the computed path's points up to the
    last waypoint passed, then a position its movement simulation reached (where the move runs out, by the UI's
    action predictions). The visualizer isn't ported; the checker requires the follow to be the start, the waypoints
    `path[1..w]` and the position a recorded simulation step reached (`ag_sim_end`), `w` the waypoint index that step
    started at (`ag_sim`, whose recorded path must be the computed one), a step before the follow, from the unit's latest `FindPath` in its turn; stream G's `combat/ai_sim.rs` checks those
    steps. None is an AI path cut by `ScorePath`.
  - On `-100` to `-103`, before the observer: line of sight taken from the recorded end (246 paths, below), the
    player's `FindPath` results not recorded (174 requests compared without a delivery, 162 more would need line of
    sight) and the follows made from them (75). Mutations are caught: no blockers, enemy
  and ally swapped, no erosion, no `ModifyHScore` (through an implied "blocked" answer, below). The start's clearance
  region can only matter for a cell with a connection and a dynamic blocker, which `CalculateConnections` never
  leaves, so not in these fights: its unit test covers it.

## What stays an input

- **Line of sight** (`LineOfSightGeometry`: segments of the scene's `FogOfWarBlocker`s, polygons or mesh convex hulls,
  in a 3.9 m cell cache). Not computable from what's recorded; porting it means importing the blockers. Observer
  `paths` records the answers (`-114`: 87 paths asked, every call's arguments equal). Without it, the checker answers a query "clear" at the node the recorded path ended on and
  "blocked" elsewhere, and **fails any path that would need a "blocked" answer**: so every LOS path accepted asked
  exactly once, at its end (246 times, 0 implied "blocked"). That shows the search reaches that node first and the
  path from it is exact; it doesn't show that the game's answer there was "clear" (it must have been, or the search
  would have gone on).
- `AvoidanceDisabled` beyond the agent's field: its counter (cutscene moves, `UnitMoveAlongPath`: none in these
  fights) and `!IsConscious` (no one is unconscious here). The `nav_guo` records check the box sizes (`-114`: equal).
- The unit's speed (the maximum distance, `rq.maxd`), the command's fudge radius (`ibr`), corpulence where no tap
  recorded it (Anevia before she joins, the lizard: taken from the request's clearance, 21 requests in `-103`'s six runs).
- `IsEnemy`: the sim's (`combat::units::is_enemy`) on the save hydrated at the window's start.
- **Areas**: A*'s `HierarchicalGraph` follows connections one way inside its 256-node clusters (it links clusters
  both ways), so its components are the undirected connected components (`World::compute_areas`, compared for
  equality only) as long as every connection has its reverse. A diagonal can be one-way (`cutCorners` with different
  climbs): `apply_graph_updates` refuses a grid with any (none in the baked grid or its navmesh, nor after any of the
  228 turn starts and 144 removals of `-100` to `-103`); porting the hierarchical graph itself waits for a case.
- `distanceXZ` on the request's constraint: `PathTo` sets it after `StartPath` (the lizard's requests), so a worker
  may have searched without it (it drops the height from `AstarPath.GetNearest`'s distance limit). The request
  carries it; the checker accepts a request with it only when the result is the same either way (all six here).

## Observer `paths`

`oracle/mods/WrathOracle/CommandObsPaths.cs` (declared `paths`, installed from `CommandObs.Install`, reset with the
window; recorded in `poc5-tb-114`): `nav_guo` (each `AstarPath.UpdateGraphs(GraphUpdateObject)`: object id, bounds centre and extents, mask,
flags, tag, caller chain), `nav_update` (`GridGraph.UpdateArea`: object id, the erosion it uses), `nav_tags`
(`UpdateNavigationGridTags` returning: `erodeIterations`, `erosionOnlyAroundCellsWithTag`, the grid hash
`Grid::state_hash` computes, blocked and ally-blocked counts), `path_fin` (`Path.ReturnPath`, main thread: key, kind,
error, `CompleteState`, `searchedNodes`, end node, the raw `vectorPath`, and the `HasObstacle` calls
`LosPathConditionForGrid.TargetFound` made on the worker, buffered per path, an empty list when none), `path_seek` (`Seeker.OnPathComplete`: whether the seeker takes the path, its current one,
and with modifiers: a path superseded once computed returns without them), `path_mod` (`Seeker.RunModifiers`
post-processing done: the final `vectorPath`, so the player's `FindPath`s are compared too). The checker already reads
them when the begin record declares `paths`: graph events paired in order with `CombatNav::log` (object ids mapped one
to one), line-of-sight answers taken from `path_fin` (each call's arguments checked, their number too; a delivered path
without `path_fin` fails), `searchedNodes` compared, a definition before `path_mod` taken as the raw path, and `path_mod` required exactly for the paths the seeker
took with modifiers and that didn't fail. Still wanted: a fight where a party member drops unconscious, for
`AvoidanceDisabled`.

## Review

gpt-6.1-sol (high), eight rounds, the last without findings. Fixed along the way: the checker accepted a
`FollowPrecomputedPath` cut short by an unverified end point and stale candidates from earlier turns; areas had no
guard for one-way connections; `GraphCollision.up` (the funnel's portals) had been replaced by `GraphTransform.up`;
`distanceXZ` was dropped; a request `PathTo` keeps and reports again was checked twice; and, for the new observer's
records, the raw/final boundary, empty or malformed line-of-sight transcripts, missing `path_fin`/`path_mod`, and a
superseded path's skipped modifiers (now recorded as `path_seek`). With `poc5-tb-114`: the replays' navigation clock
tape, the counters (an exact path with recorded line of sight counts as exact), the path visualizer's follows; three
more gpt-6.1-sol rounds on that, the last without findings (the visualizer's follows now need the step's waypoint
progress, its recorded path equal to the computed one, and the step before the follow; the counters fixed).

## Next

- The path visualizer's replacement path, if the sim is to drive the party through the UI's paths.
- The integration (movement.md step 2): `PathTo`/`FindPath` build `AgentRequest`s from the sim's state and deliver
  `turn_based_path` at the recorded `PathSync` place; `CombatController`'s turn starts and removals drive `CombatNav`.
- `LineOfSightGeometry` (the `FogOfWarBlocker`s imported with the scene), to take line of sight off the inputs.
- Real-time paths (navmesh, `XPath` with `LosPathCondition`, `RemoveExcessPoints`) stay with movement.md step 5.
