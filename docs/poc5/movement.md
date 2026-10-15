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

**Completions and requests (2026-10-15, later):** `OnPathComplete` (turn-based: `CheckApproachRadius`,
`UnreachableUnits`, the not-found stop, `SetWaypoint(1)`, `RemoveFromGroup`), `FollowPrecomputedPath`, `ForcePath`
and `PathTo` (the request's de-duplication by destination and throttling by `TileHandler.LastUpdateFrame`,
`ResetStuck`, `ConstructTags`, the `ABPathWithClearance`/`LosPathConditionForGrid` request's parameters, the move
command's warm-up and roaming) are ported, and two more record-level checks rerun them: **60 turn-based completions
and precomputed or forced paths and 396 `PathTo` calls (33 requests), 0 differ** (`poc5-tb-104`/`-105`), the request
compared field by field with the recorded one; ticks: **34,074, 0 differ** on `-100` to `-105`. The observer gained
`PathTo`'s and `FindPath`'s reads (`corp`, `cells`, `fudge`, `speedmps`, `navnode`, `move`), `ag_req.tb`, and
`ag_done.combat` with the `UnreachableUnits.Add` call (`unreach`) since `-105`. Not yet established by a capture:
a path ending outside its approach radius (`unreach`, `softcol`/`closest`: none in these fights) and `ForcePath`
(the fights use only `FollowPrecomputedPath`). The `PathTo` check takes `InitMoveDirection`'s result from the state
after (orientation is stream B's, `docs/poc5/orientation.md`); the completion check takes `m_RequestedFindPath`
from the nested completion's state (the AI's `FindPath` callback clears it unobserved; the next capture should
record the state at a call's start).

## The driven replay: design (2026-10-15)

The agent runs inside `wr-combat replay` (`combat/replay.rs`, `replay/agent.rs` new) when the recording declares
`agent` with the `-105` fields; older captures replay as before. Entries, as the commands' slices do it (an entry
record starts the sim's code, which pulls the records it makes and reads, in order; any other agent record in
between is a desync):
- **`ag_ctl`** (`UnitMoveController.TickUnit`, placed in `Phase::Move`): the sim's `TickUnit`: `PreviousPosition`,
  the agent's `IsReallyMoving` and the `canmove` leaf, `InterruptMove` into the commands model, `TickMovement`
  (pulling its leaves, physics, draws and calls), the positions (`SetPositionWithoutWaking`, the view's), the
  orientation (stream B's port; until it lands, `ag_ctl_end.orient` is a recorded input), `UpdateSlowRotation`
  (`ag_rot`), the place-on-ground step (`ag_ground`, an input), `UpdateVelocity`; checked against `ag_ctl_end`
  (`pos`, `vpos`, `orient`). The frame's `ag_ctl` units must be the awake units the controller iterates, in order
  (`State.AwakeUnits` not controlled by the director, with an agent), and none when the controller returns early
  (`WaitingForUI`).
- **`TurnController.TickMovement`** from `CanMoveInTurnBased`: the agent's callback runs the ported
  `cooldowns::tick_movement` with the agent's delta, force mode and `NearTheEnd`, the `cd_move` record's other
  fields staying inputs (limit, the modify flags, the five-foot step, auto-stop, speed, panic); `cd_move`'s
  delta out and the agent's own outcome are checks.
- **`uc_call`** (the commands model's calls): `moveto` runs `PathTo` (`UnitEntityView.MoveTo`'s gate: an agent and
  an active graph), `follow` runs `FollowPrecomputedPath`, `stop` runs `Stop`; each pulls its `ag_req`/`ag_call`
  records.
- **Path deliveries** (`path_ret` of a request the agent made): `OnPathComplete` with the path's contents from
  `ag_done` (until stream C computes them), pulling its records; a `FindPath` delivery clears
  `m_RequestedFindPath` (its combined callback).
- **Outside entries**: an `ag_call` the sim's code didn't make (`Stop` from `TryCancelCommands`, `EnterProneState`,
  `HandleCombatEnd`) is applied to the sim's agent as the recording says and counted (an input), like the targets
  slice's outside writes.
- **The commands' agent leaves** (`moving`, `rmoving`, `wants`) are answered by the sim's agent and checked against
  their records; `ag_tick`'s turn fields (`turn`, `cur`, `acting`, `combat`, `dontint`) are checks of the sim's
  turn and commands state.

## The driven replay: hardening (2026-10-15/16, stream A2)

gpt-6.1-sol's review of the first integration (`docs/poc5/movement-review-integration.md`, 18 findings) is worked
through; `crates/wr-sim/src/combat/replay/agent.rs` was rewritten around it. [M] unless tagged.

**The capability.** The driven agent needs WrathOracle's `agent_sync` (declared in the begin record since
`poc5-tb-113`, `CommandObsAgent.cs`, three gpt-6.1-sol rounds on the observer): `ag_sync`, every agent's state where
real time ends (at `CombatController.HandleCombatStart`, else at a frame's start or before the first agent record
once `IsInTurnBasedCombat()` turned true, read without its `CanChangeWeaponSet` side effect); `ag_pos`, a unit's
position stored by `UnitEntityData.Position`'s setter outside `UnitMoveController.TickUnit` and
`UnitPlaceOnGroundController.TickUnit` (roaming moves a sleeping unit with `unit.Position += ...`), with the caller
chain; `ag_init`'s `prev` (`PreviousPosition`) and `desired`. A capture declaring `agent_sync` requires its initial
states and the impatience/targets/engage/AI/commands/turn chain (else reported); one with `agent` alone
(`poc5-tb-100` to `-109`) replays with the agent's records taken as before. `WR_AGENT_DRIVEN` forces the driven
agent on such a capture for diagnosis (synthesizing the boundary from the last recorded states; reported, never
valid).

**What the sim runs now** (each finding's number in brackets):
- The boundary [1]: real-time spans stay taken from the recording; from then until an `ag_sync` the agents aren't
  the sim's, and turn-based agent activity (an entry, a commands' call or read, `ContinueActing`'s reads) is
  reported. A synchronization is accepted only after real time (a real-time entry or frame, a combat start) and must
  cover every agent, with `AllAgents` as the sim's.
- The commands' calls [2] run the agent (`Driver::ag_command_call`): `View.MoveTo` runs `PathTo` with the sim's
  destination agent (the target view's `AgentASP`) and the call's arguments, checked field by field against `ag_req`;
  `FollowPrecomputedPath` gets a new `ForcedPath` of the command's points (its error false, its definition checked:
  `CompleteState` Complete, the points, a name the observer never used before) [7]; `View.StopMoving` runs `Stop`.
  Top-level agent calls remain only for callers the sim doesn't run, counted by caller: `TickOnUnit`'s `StopMoving`
  of a unit without commands (its view gates aren't recorded for it; for a unit the commands tick it's a desync),
  `EnterProneState` (whose `ObstaclesHelper.RemoveFromGroup` after the `Stop` is now applied: the
  `TickUnit`-end state check found it), `TryCancelCommands`, `HandleCombatEnd`. A top-level `PathTo` in turn-based
  combat is a desync (its caller isn't recorded; every one the fights make is the commands').
- The getters [3]: the commands' `moving` (`View.IsMoving()`), `rmoving` (`IsReallyMoving`, its `IsDone` read
  pulled first) and `wants` are the sim's agent's, checked against their leaves; `tt_ca`'s `wants`, `nomore` and
  `motion` likewise, and `CheckAndClearWantsToMoveInTurnBased`'s clear (only when not busy and without motion) is
  applied to the agent (`TurnCtl::cleared_no_more`). `cd_move`'s delta out is the cooldowns' computed one.
- Positions [4] change only through the sim's code, `ag_ground` (physics, an input) and `ag_pos` (counted by caller);
  a `TickUnit`'s recorded position must be the sim's. Observation rows [10] (`obs u`, written or unchanged) are
  checked against the sim's positions and, in turn-based combat, `IsMoving()`.
- `FindPath` [5] is ported (`agent::find_path`: the in-flight gates, the clearance unless avoidance is off,
  `ConstructTags`, the `ABPathWithClearance` with a `LosPathConditionForGrid` or `MaxDistanceEndingCondition`); its
  arguments are the AI's (inputs), its states before and after checked; only `m_RequestedFindPath` is written.
- Request keys [6] are computed: `PathSync`'s `<requester>#<n>` counts every `path_start` of the window; the sim's
  request pulls its `path_start` (key, end point, strict flag) inside the call. A delivery (`ag_done` at the top)
  needs its `path_ret` in a recording (a replay's trace has none) and is of a request of the window, once.
- `UnitMoveController` [8]: each frame's batch is the awake units in `State.AwakeUnits`' order (dead ones too), none
  while `WaitingForUI`; a `TickUnit` out of turn, missing or extra is reported (the recording's last, cut frame
  aside). `ag_ctl`'s `prev` [16] and `ag_ctl_end`'s whole state [9] are checked.
- Checks tightened: `canmovetb`'s answer and delta [11]; `DontInterruptInTurnBasedMode` from the commands model's
  `Commands.Move` [12] (and `PathTo`'s `move` read: delay and roaming); `cur` and `acting` separately [17]; booleans
  required to be booleans [18]; a leaf's unit is the innermost `TickMovement`'s or `null` (not "any"); `AllAgents`
  indices of `ag_init`, `ag_sync` and `ag_enable` [14]; a `SlowDown` draw outside the agent is reported [13].

**Result.** On `poc5-tb-113` (Manual_18, both fights, record and two replays) **every run is valid with the agent
driven**: per run `tb-nearest` 19,294 entries, 276 commands' calls run, 18,013 states checked, 7,443 units ticked,
13 requests and 2 draws checked, 1 synchronization, 4,348 getters checked (the commands', the turn controller's,
`MoveTo`'s points, the constructed paths' `IsDone`), 13,012 rows checked; `tb-focus` 22,748 entries, 565 calls,
21,626 states, 8,809 ticks, 17 requests, 3,587 getters, 14,525 rows. Still inputs (by kind, `tb-nearest`'s record):
the real-time entries (2,714), `TickOnUnit`'s stops of units without commands (9,121), `IsDone` (2,726: when A*
hands a path back), `DesiredOrientation` (7,443: `LookAt` isn't ported), the gates and reads the agent makes of
the unit and its view (`CanMove` 1,094, `CanRotate` 7,985, the animation 1,071, commands 1,071, `MaxSpeed`
1,090, `FlyHeight` 546, corpulence, cells, speed, fudge), physics (546 queries), path contents and deliveries,
`FindPath`'s arguments (11), the obstacle group (10) and `IsProne` (from the `Stop`'s own end state), one roaming
position. `poc5-tb-109`, `-111` and `-112` (and older) replay valid without the driven agent: they lack `agent_sync`. The agent checkers
(`every_recorded_*`) and the POC-5 suite pass.

`crates/wr-sim/tests/poc5_agent.rs` (`missing_or_wrong_agent_records_are_reported`, ignored: the capture) damages
`poc5-tb-113/tb-nearest` 69 ways (the capability, the initial states and their indices and fields, the
synchronization: missing, partial, reordered, unreadable, changed, repeated mid-fight, in real time; `TickUnit`'s
position, `prev`, fields, end state, a missing, swapped or `WaitingForUI` tick; `CanMoveInTurnBased`; the turn
state; strict booleans; `SlowDown` draws extra or missing; `AllAgents`; request keys, ends, arguments, `FindPath`'s
request and start; the callback; a delivery without `path_ret`; the commands' `ForcedPath`, its request's defaults, definition, completion and `IsDone`; deliveries
(`path_ret`) duplicated, without a callback, at the recording's end, with a wrong request frame; FindPath's
callbacks of unrequested paths; a position write inside `TickUnit`; rows;
the commands' and the turn controller's getters; `ag_pos` missing or wrong; the ground step; a view callback; an
extra `PathTo`; a record of another frame); each is reported, against the undamaged run's reports.

Reviews: gpt-6.1-sol (high), the observer three rounds (the cutscene return, the side-effecting predicate, a path
definition ordered after the snapshot); the replay six rounds: a synchronization accepted mid-fight, a constructed
path's completion `err`; a unit target's `MoveTo` point (now the target's simulated position for the classes whose
`GetTargetPoint` is the base one: no turn-based capture case yet), the constructed path's contents and `IsDone`, a
split snapshot block; FindPath callbacks' provenance, a real-time frame without entries, the constructed request's
defaults; position writes inside the controller, deliveries' pairing and uniqueness, carried requests; the last
frame's delivery, the request frame. Round 6 found nothing.

Left: `UnitCommands.InterruptMove` from `TickUnit` (reported where reached), `LookAt`/`DesiredOrientation`, the
units' `ControlledByDirector` (reported in a cutscene), mounts, real time.

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
