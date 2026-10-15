# POC-5: the movement agent

Status (2026-10-15, twenty-second session): the observer is in and recorded (`poc5-tb-100` to `-103`); the sim's
`TickMovement` (turn-based) reruns every recorded call bit for bit; not yet run by the driven replay (next, below).
Ember agreed the order: `crates/wr-nav` and the area nav file first (done), then `UnitMovementAgent`.
Since then the agent runs in the driven replay (A2), and its physics and turn-based paths are the sim's ports
("Physics and paths computed", stream M).

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

**`poc5-tb-114`** (with the `paths` observer too) ends its fight while the UI guard is up: the frame's
`UnitMoveController` returns early (`turn_tick` says `guard=1`), and the guard drops later in the frame, inside the
UI's `HandlePartyCombatStateChanged` right before `HandleCombatEnd`. The replay's `ui_guard` handler had placed that
change in the turn controller's phase, so the move batch saw the guard down and expected every unit. A guard change
followed in its frame by `combat_start`/`combat_end` (only other guard records or `delayed_schedule` between) is now
placed with the combat change (`Phase::Join`, after the move phase): **every run of `-114` is valid with the agent
driven**, and `-113`, `-112`, `-111` and `-109` still are (`-115` to `-117` report only the combat log, not the agent).
`driven_agent_replays_are_valid` (`poc5_agent.rs`) replays `-113` and `-114`. gpt-6.1-sol: no findings.

Left: `UnitCommands.InterruptMove` from `TickUnit` (reported where reached), `LookAt`/`DesiredOrientation`, the
units' `ControlledByDirector` (reported in a cutscene), mounts, real time.

## Physics and paths computed (2026-10-16, stream M)

With the agent driven (`agent_sync`), the movement agent's physics and, with WrathOracle's `paths` (`poc5-tb-114`
onward), its turn-based paths are the sim's ports instead of the recording's: `wr-phys` (`docs/poc5/physics.md`) and
`wr-nav`'s turn-based search with `combat/paths.rs` (`docs/poc5/paths.md`). The driven replay's side is
`combat/replay/nav.rs`; the grid's side the sim's turn controller runs is `combat/paths.rs`'s `NavRun`. [M] unless
tagged.

**Physics.** Every recorded `ag_phys` is a check of the sim's answer: `Scene::raycast_down` or
`heuristic_disc_cast_down` on the area's `cache/sim/phys/<guid>.wrphys`, compared with the recorded hit, distance,
point and collider (`scene:path (type)`, the importer's shape name, which must name one collider of the area; its
sibling path `sib` must have an index per ancestor: the file keeps no sibling indices), bit for bit. Inside `TickMovement`'s `Move` the
query's arguments are the sim's (`AgentPhysics` answered by the driver); inside a movement simulation step the sim
runs (below) likewise; inside a span the driven replay takes from the recording (real time: the walk-up, the agent's
code isn't run there) the arguments are the recording's and only the answer is checked; so are another caller's
(`ctx: "other"`: a critter's move, presentation), except that one `wr-phys` can't answer is taken there, not
reported (the sim never uses it). A query `wr-phys` answers
`Unverified` (a box, a mesh or another collider within reach) or `Unsupported` is reported (`driven replay:
physics: ...`) and the recorded answer taken; none occurs here. With observer `phys_probe` the recorded global query
settings must be `wr-phys`'s (`ph_settings`: back faces not hit; triggers fail closed in `wr-phys` either way).

**The movement simulation.** Each `ag_sim` .. `ag_sim_end` (`SimulateTickMovementForTurnBased`, from the AI's
`ScorePath` or the path visualizer) is run with `combat/ai_sim.rs` from its recorded simulator: `MaxSpeed`,
`FlyHeight` and line of sight are its records (inputs), its physics the sim's, its `GetNearest(p).node.AllyBlocked`
the sim's grid (with `paths`; before, the record's), and the simulator after it and the result are compared with
`ag_sim_end`. A simulation record outside a step is reported (in turn-based combat); in real time the steps are
inputs and their physics checked. The AI still reads `ScorePath`'s step results from its own leaves (`msim`; the
AI isn't driven through the step yet: `docs/poc5/ai-movement.md`), each now required to be the step the sim just ran
for the AI's unit in its frame (`maxMoveLength`, the result and the simulator after it), and its grid reads (`ablk`, `GetNearest(p).node
.AllyBlocked`) the sim's grid's; every step's delta must be `0.1f` (both callers'); these captures' steps cover the shift, the cap at the
movement left (`ReachedMaxDistance`), the approach radius entered (`SegmentCircleIntersection`, with line of sight),
waypoint advances and the path's end.

**The grid** (with `paths`; `combat/paths.rs`, the sim's code at the game's points):
- `HandleCombatStart`: `erosionOnlyAroundCellsWithTag = 31`, `Clear()` (`ag_combat_start`, before `m_Units` is
  filled); `StartTurn`: after the buffs' tick, every unit of the sim's `m_Units` reverted and prepared around the
  sim's position (the enemy test `IsEnemy` the sim's), then the tags with the rider's size; `RemoveUnit` (the sim's
  `turns::remove_unit`, from a leave or a death): the unit's object reverted, the tags refreshed with the current turn
  (the rider's combat and death state the sim's); `HandleCombatEnd`: `Clear()` then a refresh without a turn.
- `UpdateNavigationGrid` in `CombatController.Tick` (the turn phase, when the tick goes on in the default mode, in
  combat, not waiting for the UI): no navmesh cut; while `m_NavmeshUpdateTimer` isn't negative it reads
  `Time.unscaledTime` (a recording's `nav_clock`, an input; the timer starts at the begin record's `nav_timer0`) and
  refreshes the tags once it runs out. A replay's trace has neither the recording's timer nor its reads, which
  WrathOracle replays in order: `prepare_world` reads them from the recording beside it (`record.trace.jsonl.gz`),
  and the sim's timer takes them in order (one left unused, or one missing, is reported; a replay without its
  recording is reported).
- Inputs: each unit's corpulence (its last recorded read before the record being processed, `uc_v corp` or the
  agent's request reads; it can change in a fight, as a size spell does in `poc5-tb-120`; every use of it reaches a
  recorded output: a unit's box, an ally's box grown by the rider's, the rider's size in cells as the refresh's
  `erodeIterations`); `AvoidanceDisabled` is the agent's field or the unit not conscious (its counter,
  raised by cutscene moves, `UnitMoveAlongPath`, riders and a few abilities, isn't modelled). Every object the sim
  queues (`nav_guo`: centre, extents, mask, flags, tag), applies (`nav_update`: its erosion) and every refresh
  (`nav_tags`: `erodeIterations`, the tag, the grid's hash, the blocked and ally-blocked counts) is paired in order
  with the recording's (each record's id an integer, mapped one to one onto the sim's objects), and every frame's
  events are paired by the frame's end, so the corpulences and flags used are checked where they're used. `ConstructTags` reads the
  grid's current `erodeIterations` (it was the nav file's).

**The paths.** Each `ABPathWithClearance` the sim's agent requests (`PathTo` from the commands, `FindPath` from the
AI or the path visualizer, its request already checked against `ag_req_end`'s) is searched with
`World::turn_based_path` when the recording returns it (`path_fin`, right after its `PathSync` delivery `path_ret` in a
recording), on the grid as it was at the request: a graph event between the two is reported (when the worker
searched isn't recorded; none occurs in these fights), as are graph events out of step at a request and a request
that replaces its seeker's path still in flight (`StartPathInternal` fails it unless a worker had finished it, which
isn't recorded). Line of sight is
answered from `path_fin`'s transcript, each call's arguments checked and every answer used (an input:
`LineOfSightGeometry` isn't ported); a request with `distanceXZ` must give the same path without it. Checked: the
traced nodes, `error`, `CompleteState`, `searchedNodes`, the end node's grid index, each definition (its
kind too, also of a definition written before the sim registered the request: a worker can finish first; a pending
one after the return is reported) (`ag_pathdef`: the traced nodes before the
seeker's modifiers ran, the delivered path after), the seeker's decision (`path_seek`: `OnPathComplete(Path)` runs the modifiers and calls back,
taking the path if it is still the seeker's latest request, from the window's `path_start`s) and modifiers
(`path_mod`), and
every delivery: `OnPathComplete` (`ag_done`) **runs on the sim's path** (the recorded one a check, as is its
prefix's `path_done`: error, count, end, `GetTotalLength()`; `PathTo`'s prefix `path_to` is checked against the sim's
call; a prefix is used in its frame), FindPath's callback
(`ag_findclr`) must be one the seeker makes, and the AI's `ScorePath` read (`uc_v path`) must be the sim's path of that `ScorePath`'s own `FindPath`
(each read takes its request once), or,
when `FixPathIfTooShort` made a fake path, the sim's path must be one it replaces (fewer than two points, or two
within 0.1 m in x and z; the fake path's points are an input). Every `nav_*`/`path_*` record must be of its frame, and a return's seeker, modifiers and callbacks (with the AI's
read after `BlockUntilCalculated`) of the return's frame (`Path.ReturnPath` runs them synchronously),
and a `path_fin`, `path_seek` or `path_mod` of a path no request started is reported; a critter's request
(`path_critter`, `critter#n`: `CritterMoveAgent`, presentation) and a request started inside a span taken from the
recording keep the recording's path. Captures without `paths` (up to `-113`) take the
recorded paths as before (A2); `paths` declared without the driven agent's chain is reported.

**Still inputs:** when a path is delivered (`PathSync`), line of sight (search and simulation), the units'
corpulences, `MaxSpeed`/`FlyHeight` in the simulation, the Unity clock the navigation timer reads (a replay's are
its recording's), `FixPathIfTooShort`'s fake path, `FindPath`'s arguments, critters' paths and physics, the path visualizer's replacement paths (the commands' `FollowPrecomputedPath`
of the party: the UI), the `AvoidanceDisabled` counter, a path of real time delivered in turn-based combat.

**Result.** On `poc5-tb-114` to `-117` **every run is valid with the physics, the movement simulation's
steps, the grid and the turn-based paths the sim's**, and `-113` with the physics and the steps (its grid reads stay
inputs: no `paths`); `-112`, `-111`, `-109`, `-95` and `-96` replay valid as before. Per record run (the replays
compute the same; the deliveries counted vary by one or two with where a replay's trace writes path definitions):

| Capture / fight | Physics queries (agent, simulation, spans) | Steps (grid reads) | Paths (LOS answers) | Deliveries checked (to the agent) | Graph events | Turn starts, removals, refreshes |
|---|---:|---:|---:|---:|---:|---:|
| `-113/tb-nearest` | 911 (546, 44, 321) | 43 (inputs) | – | – | – | – |
| `-113/tb-focus` | 864 (467, 75, 322) | 72 (inputs) | – | – | – | – |
| `-114/tb-nearest` | 1,735 (1,068, 346, 321) | 336 (326) | 20 (13) | 66 (2) | 183 | 9, 6, 4 |
| `-114/tb-focus` | 1,696 (984, 390, 322) | 378 (366) | 23 (16) | 77 (2) | 219 | 10, 6, 3 |
| `-115/tb-nearest` | 1,679 (1,016, 343, 320) | 334 (325) | 21 (14) | 70 (2) | 199 | 9, 6, 4 |
| `-115/tb-focus` | 1,176 (705, 151, 320) | 143 (135) | 21 (15) | 68 (3) | 128 | 7, 6, 3 |
| `-116/tb-hold` | 1,711 (1,040, 349, 322) | 342 (335) | 22 (15) | 70 (3) | 199 | 9, 6, 4 |
| `-117/tb-solo` | 1,066 (642, 99, 325) | 96 (93) | 19 (15) | 66 (4) | 220 | 10, 6, 4 |

Over the 24 runs of `-114` to `-117`: 27,189 physics queries, 4,887 simulation steps, 378 paths (the 129 of `-114`
that `paths.md`'s checker computes among them) and 3,444 graph events, none differing and none taken. Every recorded
`ag_phys` of these runs is a check now (the agent's `physics` input is gone), and no path the agent follows in them is
the recording's. `poc5-tb-118` (`tb-chain`, not a target here) computes 4,131 queries, 627 steps and 45 paths without
a difference; its runs are invalid on other ports (`AttackBonusConditional`, `AddInitiatorAttackWithWeaponTrigger`,
the combat log).

On the main session's newer captures (current oracle build, every observer, the projectile pin and critter paths;
judged by this stream's kinds only, the runs' other reports being other streams'): `poc5-tb-122/tb-focus` is valid
on every run (1,719 physics queries, 334 steps, 21 paths, 149 graph events per run); `-123/tb-chain` (4,503 queries,
920 steps, 45 paths, 297 graph events) has no report of this stream's kinds; `-122/tb-nearest` (24 paths) reports
graph events that differ only because the sim's turn order differs there (an initiative tie broken by object
identity, reported by the turn port: its `m_Units` order is the boxes' order); `-121/tb-lizard` (three fights,
7,994 queries, 1,593 steps, 86 paths, 556 graph events) reports 802 agent and simulation queries near a cave wall
whose ray may touch its mesh collider (`Unverified`: the BVH34 mesh raycast isn't ported; `phys_probe`'s `ph_probe`
rays over the meshes would check one), each answer taken from the recording and reported.

The coverage line (`wr-combat replay`): `physics and paths: physics queries computed ... simulation steps ... paths
computed ... deliveries checked ... graph events matched ...`.

`missing_or_wrong_physics_and_path_records_are_reported` (`poc5_agent.rs`) damages `poc5-tb-114/tb-nearest` 63
ways (its first replay twice: the timer's refresh removed, a return moved a frame before its callback;
`poc5-tb-123` once: the AI reading a step of the frame before; `poc5-tb-118` four ways: the AI reading a step other
than the sim's, or another unit's, its steps' records removed, its grid read changed), each reported: the agent's, a simulation step's and a
span's query (distance, point, collider and its sibling path, hit); a step's
end (position, result, length), a grid read, a missing read or end, a stray simulation read, a line-of-sight
argument; a path's traced nodes, `searchedNodes`, `CompleteState`, its line-of-sight answer flipped, an argument
changed, the transcript empty or unreadable, its return missing, moved before its delivery, of another frame or of
a path no request started, its end node, its modifiers changed or missing, the seeker's decision (not taken, no
modifiers, no callback), a stray `path_seek`, the delivered path, a definition, the AI's read, a fake path where
`FixPathIfTooShort` keeps the path, `path_done`'s length, the record itself or its frame,
the callback's kind, a pending definition after the return, `path_to`'s radius, a step's delta; a queued object's extents, centre and flags, an application's erosion, a refresh's hash, an object
missing, a refresh repeated, the clock read missing or another, the timer negative, the observer undeclared or declared
without `agent_sync`, a refresh moved into the next frame, a graph update's id unreadable, a definition of another
kind of path (complete, or pending before the request's registration), a unit's corpulence reads all changed, the query settings. `driven_agent_replays_are_valid` replays `-113` to `-117` (every run)
and checks the counts.

**Edits outside the agent's and paths' files** (kept small for the other streams): `combat/replay.rs` (the module,
`Outcome::nav`, the `nav_*`/`path_*` dispatch, `nav_start`, `StartTurn`'s and `HandleCombatEnd`'s hooks,
`UpdateNavigationGrid` in the turn phase), `combat/turns.rs` (`RemoveUnit`'s hook), `combat/mod.rs`
(`CombatWorld::nav`), `combat/replay/ai.rs` (the AI's path read), `bin/wr-combat.rs` (the coverage line),
`wr-phys` (`Scene::owned`).

## Steps

1. `agent` observer; record `tb-nearest` and `tb-focus`. **Done** (`poc5-tb-100` to `-103`; `-103` is current).
2. The sim's agent with recorded paths and physics. `TickMovement` **done** against the records. Next: `PathTo`
   (it needs the unit's orientation, `InitMoveDirection`: orientation isn't modelled yet, so `UnitMoveController`'s
   `LookRotation`/`eulerAngles` and `UpdateSlowRotation` come with it), `OnPathComplete` and the deliveries at
   `path_ret`, `FollowPrecomputedPath`/`ForcePath`, then `UnitMoveController` in the driven replay's `Phase::Move`
   (`cd_move` becomes a check, the commands' `moving`/`rmoving`/`wants` leaves become the sim's answers, positions
   checked against every `obs` row): every run valid with positions computed.
3. Paths computed with `wr-nav` (blockers, tags, approach and LOS ending conditions), checked against `ag_done`.
   **Done** (`paths.md`; in the driven replay: "Physics and paths computed").
4. Physics: the area's colliders imported and PhysX's raycast and disc cast ported, checked against `ag_phys`.
   **Done** for the terrain (`physics.md`; in the driven replay: "Physics and paths computed").
5. Real time.
