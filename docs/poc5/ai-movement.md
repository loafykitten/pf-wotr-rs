# POC-5: the AI's movement simulation

Status (2026-10-15, stream G): `AiAction.ScorePath`'s movement simulation is ported
(`crates/wr-sim/src/combat/ai_sim.rs`) and checked against every step the recordings hold, as far
as they establish its inputs. They establish little: the captures hold 21 steps, all one kind, and
none records the step's own reads (`MaxSpeed`, `FlyHeight`, the grid lookup, line of sight) or the
simulator's start. The observer change below records them; it is written, not built or recorded.
Not yet driven: `combat/ai.rs` still takes each step's result from the `msim` leaf. Tags as
elsewhere: [M] measured, [I] inferred, [C] from the decompiled code.

## The code [C]

`ScorePath` (`Kingmaker.AI/AiAction.cs`), once its path (`FixPathIfTooShort`'s) is complete, long
enough and not close enough, builds `new MovementSimulator(Unit.Position, CurrentPath,
destinationPoint, ActionRangeWithCorpulence, checkLOS: true, 0f)` and calls
`Unit.View.AgentASP.SimulateTickMovementForTurnBased(ref sim, 0.1f, num5)` until a step returns
false, reaches its maximum distance or `num5` (the movement range left) drops below 0.0001. The
path visualizer (`Kingmaker.TurnBasedMode/PathVisualizer.cs`) runs the same step for the UI.

- `MovementSimulator` (`Kingmaker.View/MovementSimulator.cs`, a struct): `m_Position` behind a
  `Position` setter that first stores `LastMoveLength = (m_Position - value).To2D().magnitude`; the
  path; `NextWaypoint` (2D) and `NextWaypoint3D`; `WaypointIndex`; `ReachedMaxDistance`; the
  readonly `ApproachPoint`, `ApproachRadius`, `CheckLOS`, `IgnoreBlockerRadius`. The constructor
  starts at point 1 (point 0 for a one-point path; `Vector2.zero`, i.e. (0, 0, 0), for an empty one).
  `AdvanceWaypoint` stops at the last point; `OnLastSegment` is `WaypointIndex == Count - 2`.
- `SimulateTickMovementForTurnBased(ref sim, deltaTime, maxMoveLength)`
  (`Kingmaker.View/UnitMovementAgent.cs`, line 761): reads `MaxSpeed` and
  `(Unit?.EntityData?.FlyHeight).GetValueOrDefault()`; with `num` the 2D distance to the next
  waypoint, if `num <= maxSpeed * deltaTime` it advances the waypoint and moves onto it (`Move(p,
  Vector3.zero)`; at the path's end onto `vectorPath.Last()` and returns false; else
  `ReachedMaxDistance = maxMoveLength <= 0` and returns true). Otherwise it shifts by
  `((NextWaypoint - pos) / Mathf.Max(num, 0.0001f)).To3D() * maxSpeed * deltaTime`, scaled down to
  `maxMoveLength` (then `ReachedMaxDistance`), moves (`Move(Position, shift)`), looks up
  `AstarPath.active.data.gridGraph.GetNearest(end).node`, and when `ApproachRadius > 0` and the
  node's `AllyBlocked == 0`, stops inside the approach radius: already inside
  (`GeometryUtils.SqrMechanicsDistance(ApproachPoint, Position) <= r * r`) or entering it on the
  step (`SegmentCircleIntersection`, `t + 0.0001` clamped, the end `Position + (end - Position) *
  t`), if `HasObstacle(end + EyeShift, ApproachPoint, IgnoreBlockerRadius)` doesn't hold; that
  returns false. `ReachedMaxDistance = LastMoveLength >= maxMoveLength`.
- `UnitMovementAgentBase.Move` in turn-based combat: no navmesh clamp, `noMove` always false (so
  the step's `if (noMove) return false` never fires): `agent::move_tb`, bit-identical against the
  recorded ticks (`docs/poc5/movement.md`). Outside turn-based combat the clamp runs; not ported, so
  the step reports it.
- `SegmentCircleIntersection(a, b, c, radius, out t)` with its local function
  `LineCircleIntersection`: `A = dx² + dy²`, `B = 2(dx(ax - cx) + dy(ay - cy))`, `C = (ax - cx)² +
  (ay - cy)² - r²`, `det = B² - 4AC`; -1 when `A <= Mathf.Epsilon` or `det < 0`; `-B / 2A` when
  `det == 0`; else the two roots in double (`(float)((-(double)B ± Math.Sqrt(det)) / (double)(2A))`),
  the first when it's the smaller and positive, else the second; true when `t` is in [0, 1].

Mono's arithmetic as everywhere (each expression in double, stored as `float` at a local, field,
argument or return): the comparisons `num <= maxSpeed * deltaTime` and `<= r * r` take the product
in double; `maxMoveLength / shift.magnitude` is an argument (stored) before scaling; the local
function's variables (`A`, `B`, `C`, `det`, `t`, `dx`, `dy`) are captured, so each assignment
stores a `float`; `(a.x - c.x)` inside them is not stored. `Mathf.Epsilon` is
`MathfInternal.FloatMinNormal` under flush-to-zero, else `float.Epsilon`: which the player runs
with isn't established, and the two differ only for `0 < A <= 1.1754944e-38` (a segment shorter
than about 1e-19 m), which the port reports. `LineOfSightGeometry.EyeShift` is a mutable static
initialised to `Vector3.up` that nothing in the decompiled code assigns. `GetNearest` is called
before the radius test, its node dereferenced only after it: no node with a positive radius is the
game's `NullReferenceException`, reported.

## The sim

`crates/wr-sim/src/combat/ai_sim.rs`: `MovementSimulator` (`new`, `position`/`set_position`,
`advance_waypoint`, `on_last_segment`; `m_position` public for a state read from a record),
`segment_circle_intersection`, and `simulate_tick_movement_for_turn_based(w, u, sim, deltaTime,
maxMoveLength, inp)`. What a step reads outside itself is the trait `SimInputs`, in the game's
order: `max_speed`, `fly_height`, `raycast`/`disc_cast` (inside `Move`), `ally_blocked`
(`GetNearest(p).node?.AllyBlocked`), `has_obstacle`. `Move` is `agent::move_tb` through a private
adapter (`MovePhysics`) that hands `move_tb` the two physics queries and reports any other agent
read (it makes none).

**Change suggested for `agent.rs`** (not made: the main session owns it): split the two physics
queries out of `AgentInputs` into a supertrait (say `MovePhysics { raycast, disc_cast }`, with
`AgentInputs: MovePhysics`) and have `move_tb` take `&mut dyn MovePhysics`; the adapter, 40
one-line refusals, then goes.

## The check [M]

`combat::ai_sim::tests::every_recorded_simulation_step_reruns_bit_for_bit` (ignored: needs the
captures; `WR_AISIM_CAPTURES` picks them, default `poc5-tb-100` to `-105`, both fixtures, record and
two replays). Two ways, by capture:

- **With the requested records** (`ag_sim`, none yet): each step rerun from the simulator it
  records, with the step's own leaves and physics in order, compared with `ag_sim_end` (result and
  every field) and with the AI's `msim` leaf that follows, bit for bit; every record consumed;
  `deltaTime` 0.1. `the_checker_reads_the_requested_records` (not ignored) builds the records from
  `poc5-tb-103`'s step and damages them 11 ways (a query's origin or distance, a leaf missing,
  extra or without its context, the result, a field after, the `msim` leaf, the delta, the end):
  each is reported.
- **Older captures**: each `msim` leaf rerun from the simulator `ScorePath` builds (chained
  through its steps), the step's `ag_phys` (`ctx: "sim"`) checked and consumed, the rest from other
  records where they establish it: the destination and radius from the `FindPath` request
  (`ag_req`: `ScorePath` passes the same `destinationPoint` and evaluates the same
  `ActionRangeWithCorpulence` again, taken only when the corpulences read again are the same) [C];
  `MaxSpeed` from `FindPath`'s state (`MaxSpeedOverride`) or its `speedmps` leaf (since
  `poc5-tb-105`) [C], else the unit's last `TickMovement` read [I]; `FlyHeight` the unit's last
  `MoveUnit` read [I]; `Unit.Position` the unit's last `ag_ctl_end`/`ag_ground` position [I]; the
  step's own `GetNearest` the AI's `ablk` right after it when the step ended where it looked up
  [C]; line of sight never. A step whose inputs aren't established is counted, not checked.

**Result: the captures hold 21 steps, in `poc5-tb-100/tb-nearest` (3 per run), `-101/tb-focus` (3)
and `-103/tb-focus` (1); every one is a two-point path whose end lies within one step (the first
branch: `Move` onto the last point, false).** 3 steps (`-103/tb-focus` t=1754, unit `51FC`, record
and both replays) **equal the game bit for bit**, with `MaxSpeed`, `FlyHeight` and the position
from neighbouring records [I]; 18 aren't established (`523B`'s `MaxSpeed`: it never moves in the
window, so no record has its speed); 0 differ. A `LastMoveLength` measured in 3D instead of the
`To2D` magnitude fails all 3. Nothing exercises the shift, the cap at `maxMoveLength`, the approach
radius, `SegmentCircleIntersection` or line of sight: no fight so far has an AI unit walking toward
a target out of reach (commands.md, slice D: "every AI path in these fights is close enough").
The hand-made tests (`a_step_*`, `segment_circle_intersection_cases`) cover those branches' logic
only, not the game's numbers.

## Observer changes requested

In `oracle/mods/WrathOracle/CommandObsAgent.cs` (written, not built; the agent observer, so the
AI's replay, which pulls `uc_v` leaves only, reads none of it):

- `ag_sim` (prefix of `SimulateTickMovementForTurnBased`): `u`, `dt`, `max` and `sim`, the
  simulator before the step read as fields (`p`, `len`, `rmd`, `wi`, `nwp`, `nwp3`, `ap`, `ar`,
  `los`, `ibr`) with its path's points (`pts`); `ag_sim_end` (postfix): `ret` and `sim` after
  (without `pts`).
- The step's reads as `ag_v` leaves with `ctx: "sim"` (`AgLeaf` adds the field inside a
  simulation): the existing `maxspeed` and `fly` taps on the step's `get_MaxSpeed` and
  `get_FlyHeight`, and two new taps, `near` (`NavGraph.GetNearest(Vector3)`: `p` and the node's
  `AllyBlocked`, null without a node) and `los` (`LineOfSightGeometry.HasObstacle`: `from`, `to`,
  `r`, `v`). Each tap site expects one call (the transpiler's count check reports a mismatch).

What it makes checkable: every step from its own records, both branches, the cap, the approach
radius and `SegmentCircleIntersection`, line of sight; the path visualizer's steps too (they run in
turn-based windows while the player hovers a destination). `ScorePath`'s `Unit.Position` is the
first step's `sim.p`.

**A fixture to exercise it:** an AI enemy that starts beyond its reach and must walk (several
steps, approach-radius entry), ideally one whose path bends (waypoint advances) and one whose
approach is blocked by an ally's cell. `poc5-tb-106` (`tb-nearest`, `tb-focus` again) won't add
any.

## For the main session

- The driven agent replay (`replay/agent.rs`, in progress) will meet `ag_sim`, `ag_sim_end` and
  `ag_v` with `ctx: "sim"` outside any tick, inside the AI's `ScorePath` and in the UI's path
  visualizer: they are the simulation's, not the agent's (consume or skip them there; the
  `ag_phys` with `ctx: "sim"` already appear the same way).
- **Driving it** (next, once a capture has `ag_sim`): in `ai.rs`'s `score_path`, build
  `MovementSimulator::new(Unit.Position, path, DestinationPoint, ActionRangeWithCorpulence, true,
  0)` (the position an input from the first `ag_sim` until the agent's positions are the sim's; the
  destination is already the AI's) and replace `ai_sim_step` by `simulate_tick_movement_for_turn_based`
  with an `AiInputs` method returning a `SimInputs` view over the step's records (`ag_sim` checked
  against the sim's simulator, the leaves as above, `ag_sim_end` and `msim` checks). The path
  visualizer's steps need no driving: they are UI.
- Open: `Mathf.Epsilon`'s flush-to-zero (recordable once, e.g. the begin record's
  `MathfInternal.IsFlushToZeroEnabled`); `MaxSpeed` computed (`MaxSpeedOverride ??
  CurrentSpeedMps`) once `CurrentSpeedMps` is the sim's.
