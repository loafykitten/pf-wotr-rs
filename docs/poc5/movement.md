# POC-5: the movement agent

Status (2026-10-15, twenty-second session): the observer is in and recorded (`poc5-tb-100` to `-103`); the sim's
`TickMovement` (turn-based) reruns every recorded call bit for bit; not yet run by the driven replay (next, below).
Ember agreed the order: `crates/wr-nav` and the area nav file first (done), then `UnitMovementAgent`.

Everything POC-5 computes today runs on the movement agent's results taken from the recording: positions (the
`obs` rows), `WantsToMove`/`IsMoving`/`IsReallyMoving`, the paths, the agent's own ends
(`OnMovementInterrupted`, 2 to 4 per fight) and the delta it hands the turn's movement budget
(`cooldowns::MoveInput`). This slice computes the agent. Tags as elsewhere: [M] measured, [I] inferred, [C] from
the decompiled code.

## Scope

**Turn-based combat first**, as the fixtures are. While `CombatController.IsInTurnBasedCombat()` holds, for
every unit (in combat or not, e.g. the roaming lizard) [C]:

- `UnitMovementAgentBase.Move` skips its navmesh clamp: x and z are `p + shift`, managed float arithmetic. Only
  the height comes from physics (below).
- `TickMovement` skips avoidance (`UpdateAvoidance`, `ObstaclePathfinder`, the obstacle groups).
- `OnPathComplete` skips `RemoveExcessPoints` and `PathAroundStandingObstacles`.
- Paths come from `SeekerWithClearance.StartPathWithClerance` (`ABPathWithClearance`, graph mask 2: the grid,
  the raycast modifier on, the funnel off), or `ForcePath`/`FollowPrecomputedPath` (the AI's and the path
  visualizer's paths).

Real time (avoidance, the navmesh clamp, `XPath` with `LosPathCondition`, `RemoveExcessPoints`, standing
obstacles, `UnitMovementAgentContinuous`) comes after, and is reported where reached.

## The code [C]

- `UnitMoveController.Tick`/`TickUnit` (a controller phase; skipped while `WaitingForUI`): for each awake unit not
  controlled by the director, `PreviousPosition`, the `IsReallyMoving && CanMove` gate (`InterruptMove` when
  moving but unable to in turn-based), `TickMovement`, the position (`SetPositionWithoutWaking`), `LastMoveTime`,
  the orientation from `MoveDirection` (`Quaternion.LookRotation(forward).eulerAngles.y`, native), the
  `IUnitMoveHandler` event (one subscriber, `MovementDistanceTrigger`, a fact component), the place-on-ground and
  IK paths when standing (presentation, except `UnitPlaceOnGroundController.ForcedTick`'s position: physics),
  `UpdateSlowRotation`, the rider's copy, `UpdateVelocity`.
- `UnitMovementAgent`: `PathTo` (the request, its de-duplication by destination, `m_PathFrame` against
  `TileHandler.LastUpdateFrame`, `ResetStuck`, `m_WarmupTime` from `UnitMoveTo.MovementDelay`, roaming),
  `FindPath`, `CanMoveInTurnBased` (calls the ported `TurnController.TickMovement`), `TickMovement`,
  `OnPathComplete` (`AccelerateIfPathIsLong` on every command, `CheckApproachRadius`, `UnreachableUnits`,
  `OnPathNotFound`, `SetWaypoint(1)`, `OnMovementStarted`), `SetWaypoint`, `CompleteMovement`,
  `InterruptMovement`, `Stop`, `ForcePath`, `FollowPrecomputedPath`, `CheckAndClearWantsToMoveInTurnBased`,
  `ConstructTags`, `SlowDown` (draws `Random.Range(0.2, 0.3)` for a unit not in combat), `UpdateVelocity`.
- Native math on the way: `Vector3.RotateTowards`, `Quaternion.LookRotation`, `Quaternion.eulerAngles`. Each is
  ported from the player's machine code and checked against it as `wr-nav`'s `native_check` does
  (`UnityPlayer.dylib`'s icall table names them).

## What stays an input, at first

1. **When a path is delivered** (A* runs on worker threads): the frame and the call (`ReturnPaths`) are recorded
   and pinned by `PathSync` already; the sim delivers each path at its recorded place.
2. **The path's contents** (`vectorPath`, `error`, `CompleteState`): recorded at delivery, used by the sim until
   step 3 computes them with `wr-nav` (which already reproduces `ABPathWithClearance` on idle graphs; combat adds
   the units as blockers: `DynamicBlocked`/`AllyBlocked`, `ConstructTags`).
3. **Physics**: `Move`'s `Physics.Raycast` (origin, mask 2099457, 100 m) and
   `PhysicUtils.HeuristicDiscCastDown`, `CheckApproachRadius`'s `SoftCollider.ClosestPoint`. Recorded as calls
   with their arguments (checked: the sim computes them) and results (taken). Porting PhysX's queries against the
   area's colliders is its own step, after this one.
4. Animation gates: `UnitAnimationManager.IsPreventingMovement`, `UnitEntityView.IsCommandsPreventMovement` (the
   sim may compute the second from the commands; recorded and checked first).
5. The unit's speed (`CurrentSpeedMps`; already a `MoveInput` leaf) and `FlyHeight`.

## WrathOracle: observer `agent`

Declared in the begin record (`observers`), so older captures replay as before. All floats as written by
`Json` (round-trip). Records, each with the unit:

- `ag_req`: `PathTo`/`FindPath` entry with its arguments, the agent's relevant state before (destinations,
  `RequestedPath` present and done, `m_PathFrame`, `TileHandler.LastUpdateFrame`, `RepathNeeded`), and what it did
  (returned early, the request's kind and parameters: clearance, tags, approach radius, LOS, max distance), with
  the request's `PathSync` key.
- `ag_done`: `OnPathComplete` with the path (`vectorPath`, `error`, `CompleteState`, forced), and its outcome
  (`m_NoPathDestination`, path-not-found, the first waypoint).
- `ag_tick`: each `TickMovement` call: delta in, the state before (position, `m_NextPointIndex`, `m_Speed`,
  `MoveDirection`, `m_WarmupTime`, `m_SlowDownTime`, stuck timers, first tick, force and precomputed flags), the
  leaves read (gates, `MaxSpeed`), the delta after `CanMoveInTurnBased`, and the result and state after.
- `ag_phys`: each physics query inside the agent: kind, origin, direction, distance, mask; hit and distance.
- `ag_end`: `CompleteMovement`, `InterruptMovement`, `Stop` (with the caller chain, as `paused_origins` does),
  `OnMovementStarted`/`Complete`/`Interrupted`/`WaypointUpdate`.
- `ag_ctl`: `UnitMoveController.TickUnit`'s gate per unit (`IsReallyMoving`, `CanMove`, `InterruptMove`) and the
  orientation written.

## The sim

`crates/wr-sim/src/combat/agent.rs` (state per unit, as the game keeps it on the view) and `combat/movement.rs`
(`UnitMoveController`), run in the controller phase the recording places `UnitMoveController`, against the
area's `wr_nav::NavFile` (`cache/sim/nav/<guid>.wrnav`; missing means movement is reported, never approximated).
The commands model's agent leaves (`moving`, `rmoving`, `wants`, the agent's ends, `MoveInput`'s delta and
`NearTheEnd`) become the sim's, checked against their records; positions are checked against every `obs` row and
`ag_tick` exactly.

## Result so far (2026-10-15) [M]

**The observer** (`CommandObsAgent.cs`, declared `agent`) records what is above, plus what three review rounds
(gpt-6.1-sol, high) asked for: `ag_pathdef` (a path's definition at its first reference, so paths carried into the
window are known; a path still on A*'s worker is noted `pending` and defined later), `IsDone()` where the agent
evaluates it, `CheckApproachRadius`'s collider query, `GetCurrentlyActive()`, the destination agent's view identity
(and `PathTo`'s argument's), `ForceRoaming`, `ConnectedToObstacles`, `IsProne`, the turn state `CanMoveInTurnBased`
reads (`ag_tick`: `combat`, `tb`, `turn`, `cur`, `acting`, and `dontint`, the move command's
`DontInterruptInTurnBasedMode`), the obstacle group's and the unit group memory's presence (`ogroup`, `gmem`: the
early returns of `RemoveFromGroup` and `ConnectToGroups`), the destination agent's presence (`dhas`),
`UpdateSlowRotation`'s inputs (`ag_rot`), the place-on-ground step as a boundary (`ag_ground`), and since `-103`
`ConnectToGroups`' own `Group` read (`ag_v gmem`: the getter resolves the group lazily), the proxy's resolved agent and
the agent's calls into the view (`ag_cb`: `OnMovementInterrupted` with the destination, `OnMovementComplete`,
`OnMovementWaypointUpdate` with the index). Recordings stay
clean (all six runs of `poc5-tb-98` to `-103`, the replays tick-exact); `-98` and `-99` were development captures
(archived); `-100` lacks `conn` and later fields, `-101` lacks `dontint`, `ogroup`, `gmem`, `dhas`.

**The model** (`crates/wr-sim/src/combat/agent.rs`): the agent's state as the game keeps it, an `AgentInputs`
trait for what it reads and the calls it makes into the unit, and turn-based `TickMovement` with
`CanMoveInTurnBased`, `SlowDown` (its draw), `Move` without the navmesh clamp (the physics queries as inputs),
`SetWaypoint`, `CompleteMovement`, `InterruptMovement`, `Stop`, `UpdateVelocity`, the destination agent's cleanup
(dropped when its view is gone, re-resolved through its proxy when the view lost its data), and of `ObstaclesHelper`
what the agent sees: `ConnectToGroups`/`RemoveFromGroup` set every agent's `RepathNeeded` (they run in turn-based
combat too, though their groups serve only real-time avoidance), unless the unit's group has no memory or the agent
no obstacle group. A mount is refused (its rider's half of the ends isn't modelled). Unity's managed vector math is ported from
`UnityEngine.CoreModule` (decompiled into `decompiled/`): each expression in double, rounded at its store, e.g.
`Mathf.Sqrt(2f * m_Acceleration * num)` rounds its argument to float first (one tick of `poc5-tb-100` shows it).
`Vector3.RotateTowards` is the player's native code (`wr_nav::cs::rotate_towards`, 1M inputs bit-identical).

**The check** (`combat::agent::tests::every_recorded_tick_reruns_bit_for_bit`, ignored: needs the captures):
every recorded turn-based `TickMovement` rerun from its `ag_tick` state with the leaves, physics and draws recorded
inside it, result and state compared with `ag_tick_end` bit for bit, every input record consumed, the agent's own calls (`Stop`, `CompleteMovement`, `InterruptMovement`,
`SetWaypoint` with its index) and its calls into the view (with their arguments; a `Stop` re-entering from inside one
runs the model's own) in the recorded order, the turn's `TickMovement` call checked against its `cd_move`
(delta in and force) and the physics queries' origin, direction or radius, distance and mask: **24,525 ticks on
`poc5-tb-100` to `-103`, 0 differ** (the real-time walk-up before each fight, 8,196 ticks, isn't checked). Mutations
are caught (`Mathf.Sqrt`'s rounding, a missing `InterruptMovement`, a dropped waypoint callback). What the checker still takes from the
recording, which the integration computes: the turn's state and budget (`cd_move`'s delta out), the move command's
flag, the destination's proxy, and `RepathNeeded` on other agents (its world holds one agent).

## Steps

1. `agent` observer; record `tb-nearest` and `tb-focus`. **Done** (`poc5-tb-100` to `-103`; `-103` is current).
2. The sim's agent with recorded paths and physics. `TickMovement` **done** against the records. Next: `PathTo`
   (it needs the unit's orientation, `InitMoveDirection`: orientation isn't modelled yet, so `UnitMoveController`'s
   `LookRotation`/`eulerAngles` and `UpdateSlowRotation` come with it), `OnPathComplete` and the deliveries at
   `path_ret`, `FollowPrecomputedPath`/`ForcePath`, then `UnitMoveController` in the driven replay's `Phase::Move`
   (`cd_move` becomes a check, the commands' `moving`/`rmoving`/`wants` leaves become the sim's answers, positions
   checked against every `obs` row): every run valid with positions computed.
3. Paths computed with `wr-nav` (blockers, tags, approach and LOS ending conditions), checked against `ag_done`.
4. Physics: the area's colliders imported and PhysX's raycast and disc cast ported, checked against `ag_phys`.
5. Real time.
