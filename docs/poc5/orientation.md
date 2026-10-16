# POC-5: units' orientation

Status (2026-10-16, stream T): the sim has ported, bit-identical against every recorded call (`poc5-tb-100` to
`-103`):
- the orientation `UnitMoveController.TickUnit` writes;
- `UpdateSlowRotation`'s turn;
- what `UnitMovementAgent.InitMoveDirection` reads, on every recorded `PathTo` that reaches it.

This runs in the driven replay with the agent. Since stream T, `DesiredOrientation` is the sim's too:
- every writer turn-based combat reaches is ported, or taken by cause;
- with WrathOracle's observer `look`, every `UpdateSlowRotation` read is the sim's and checked ("The writers",
  below).

Tags as elsewhere: [M] measured, [I] inferred, [C] from the decompiled code.

## The code [C]

- `UnitEntityData.Orientation` (degrees): the getter returns `m_Orientation`; the setter stores the value as given
  (no normalisation) in `m_Orientation` **and** `DesiredOrientation` (an auto-property).
- `UnitEntityData.OrientationDirection`: `Quaternion.Euler(0f, Orientation, 0f) * Vector3.forward`.
- `UnitEntityData.UpdateSlowRotation(maxAngle)`: if `Descriptor.State.CanRotate`,
  `m_Orientation = Mathf.MoveTowardsAngle(m_Orientation, DesiredOrientation, maxAngle)` (the field, not the
  setter: `DesiredOrientation` stays).
- `UnitEntityData.HasMotionThisTick`: `Position != PreviousPosition` (`Vector3 ==`, squared distance below
  `9.9999994E-11f`). The `Position` setter stores only when `m_Position != value`, so a step shorter than 1e-5
  leaves the position (and `HasMotionThisTick`) unchanged: 69 recorded ticks in `poc5-tb-100` to `-103` are such
  (e.g. `TickMovement` returns a point 3e-6 away; the unit stays).
- `UnitMoveController.TickUnit`, after `TickMovement` (only when `IsReallyMoving && CanMove`): if
  `HasMotionThisTick && CanRotate && (!TacticalCombatHelper.IsActive || IsReallyMoving)`, `forward =
  MoveDirection.To3D()` and, if `forward.sqrMagnitude > 0.01f`, `unit.Orientation =
  Quaternion.LookRotation(forward).eulerAngles.y`. Then, moving or not, the move handlers (moving only),
  `UpdateSlowRotation(deltaTime * m_AngularSpeed)` (`TimeController.DeltaTime`; `m_AngularSpeed` is a serialized
  field nothing writes), the view's rotation (presentation), the rider's copy (`Orientation = mount's`).
- `UnitMovementAgent.InitMoveDirection` (from `Init` and `PathTo`, right before `m_PathFrame = Time.frameCount`):
  `MoveDirection = Unit.EntityData.OrientationDirection.To2D()`.
- Unity's managed members (`decompiled/UnityEngine.CoreModule`): `Quaternion.eulerAngles` is
  `Internal_MakePositive(Internal_ToEulerRad(this) * 57.29578f)` (each angle below -0.005729578 gets +360, above
  360 - 0.005729578 gets -360, once); `Quaternion.Euler(x, y, z)` is `Internal_FromEulerRad(new Vector3(x, y, z) *
  ((float)Math.PI / 180f))`; `LookRotation(forward)` passes `Vector3.up`; `Mathf.MoveTowardsAngle`, `DeltaAngle`,
  `Repeat` (`Clamp(t - Floor(t / length) * length, 0, length)`), `MoveTowards`, `Sign` as decompiled, with Mono's
  arithmetic (each expression in double, rounded at its store: here the arguments of `Floor`, `Clamp`, `Abs`,
  `Sign` and `MoveTowards`, and `target` re-assigned in `MoveTowardsAngle`).

## Native code [M]

Ported from the arm64 player (`UnityPlayer.dylib`; icall table at 0x1686c10, functions at 0x168cba8) into
`crates/wr-nav/src/cs.rs` and checked by calling the player (`native_check.rs`, ignored test, a million generated
inputs each, all bit-identical):

- `Quaternion.LookRotation_Injected` (stub 0x81c2d0, implementation 0x81c300): `LookRotationToQuaternion`
  (0x5c4e64) = `LookRotationToMatrix` (0x5cd444: `z` = forward normalised, failing below a length of 1e-5;
  `x = up × z` normalised, failing below 1e-5; `y = z × x`, failing when `||y|² - 1| > 1e-6`; the axes as the
  matrix's columns) then `MatrixToQuaternion` (0x5c48d4, the one `FromToRotation` uses). When that fails and the
  forward is longer than 1e-5: `SetFromToRotation(+Z, forward normalised)` (0x5cd220) then `MatrixToQuaternion`;
  else the identity (the player logs "Look rotation viewing vector is zero"). `cs::look_rotation`. Inputs: XZ
  directions with up = +Y (the game's use), random forwards and ups, zero and tiny forwards around the 1e-5 cuts,
  forwards (anti)parallel or nearly parallel to up, zero ups: 304,435 of the million take the fallback.
- `Quaternion.Internal_ToEulerRad_Injected` (stub 0x81c074): the quaternion normalised (`(((x² + y²) + z²) + w²)`,
  the identity below 1e-5), then `QuaternionToEuler` (0x5bc46c) in Unity's default order (4, Z-X-Y; per-axis
  functions from a table at 0x1680740: `asinf` of the clamped argument times -1 for x, `atan2f` for y and z):
  `x = -asin(2(yz - xw))`, `y = atan2(2(xz + yw), w² + ((z² - x²) - y²))`, `z = atan2(2(xy + zw), w² + ((y² - z²) -
  x²))`; at the poles (`|yz - xw| ≥ 0.49999899`, `0x3effffde`) `y = atan2((xy + zw)(yz + xw) + (xw - yz)(xy - zw),
  (xw - yz)(yz + xw) - (xy + zw)(xy - zw))` and `z = 0`. `cs::to_euler_rad`. Rust's `asin`/`atan2` call the same
  `libsystem_m` functions. Inputs: rotations `LookRotation` makes, rotations from Euler angles, near the poles
  (199,344 take the pole branch), unnormalised and tiny quaternions.
- Edges (20,175 more cases): forward lengths within 4 ulps of 1e-5 along, against and across up; NaN and infinite
  components in forward, up and the quaternion; x angles swept across the pole threshold in 1e-5° steps. A NaN
  keeps its sign through the x axis's `* -1` (`fmul`), which Rust's `* -1.0` would turn into a negation: the port
  returns the NaN as is.
- `Internal_FromEulerRad_Injected` (stub 0x81c040) is `EulerToQuaternion(euler, 4)`, already ported and checked
  (`cs::quat_euler`). `Quaternion * Vector3` (managed) moved from `funnel.rs` to `cs::quat_mul_vec`.

Mutations are caught (an operand order in `QuaternionToEuler`, a reciprocal multiply in `LookRotationToMatrix`).

## The sim

`crates/wr-sim/src/combat/orientation.rs`, pure functions: `Orientation { value, desired }` with the setter and
`update_slow_rotation`; `repeat`, `delta_angle`, `move_towards`, `move_towards_angle`; `make_positive`,
`euler_angles`, `look_angle`; `orientation_direction`, `init_move_direction`; `vec3_eq`, `set_position`,
`has_motion`, `max_angle`; and `tick_unit_write` (the gates read in the game's order through closures, so a
recorded leaf is taken only where the game reads it).

## The check [M]

`combat::orientation::tests::every_recorded_orientation_reruns_bit_for_bit` (ignored: needs the captures;
`WR_ORIENT_CAPTURES` picks them, default `poc5-tb-100` to `-103`, both fixtures, record and two replays):

- Each `TickUnit` (`ag_ctl` .. `ag_ctl_end`): the leaves in the game's order (`canmove` twice when the agent is
  really moving, `canrotate` before `ag_rot` only with motion, `canrotate` after it from `UpdateSlowRotation`), the
  position after (`set_position(ag_ctl.pos, ag_tick_end.ret)` equal to `ag_ctl_end.pos`), `TickMovement`'s delta
  equal to the frame's `TimeController.DeltaTime` (`time` record), the write computed from `ag_tick_end`'s
  `MoveDirection` and compared with `ag_rot`'s `o` and `desired`, `maxAngle` computed from `DeltaTime` and the
  unit's `m_AngularSpeed` (`s.ang`) and compared with `ag_rot.max`, then `UpdateSlowRotation` from `ag_rot`'s
  values compared with `ag_ctl_end.orient`; no leaf left unread. `TacticalCombatHelper.IsActive` from the `tick`
  record's mode (never `TacticalCombat` here).
- Each `PathTo` (`ag_req`/`ag_req_end`): when `m_PathFrame` becomes the call's frame (it reached
  `InitMoveDirection`), `MoveDirection` after equals `init_move_direction` of the unit's last `ag_ctl_end.orient`;
  when it returned earlier, `MoveDirection` is unchanged.

**Result: 349,575 `TickUnit`s, 32,721 with a tick (69 of them kept by the `Position` setter), 32,526
orientations written, 1,986 turned by `UpdateSlowRotation`, `maxAngle` checked on all; 186 `InitMoveDirection`s;
0 differ.** On `poc5-tb-103` alone: 55,617 `TickUnit`s, 5,619 with a tick (9 kept), 5,574 written, 174 turned, 42
`InitMoveDirection`s. `m_Orientation` never changed between two
`TickUnit`s of a unit in these fights (no `ForceLookAt` or setter outside it), so the orientation `InitMoveDirection`
reads is always the last `ag_ctl_end`'s; no `PathTo` came before a unit's first `TickUnit` in the window, and no
call was ambiguous (`m_PathFrame` already the frame). Dropping `Internal_MakePositive` makes 2,838 of `-103`'s
checks fail.

## Open questions

- `InitMoveDirection`'s 42 cases on `-103` (186 in all) don't discriminate the managed `Quaternion * Vector3`'s
  double evaluation from strict float (a strict-float variant passes too); the formula is the funnel's, which
  `wr-nav`'s query checks exercise.
- `wr_nav::cs::approx_eq` (`Vector3 ==`) compares the unrounded double sum, while the decompiled operator stores
  it in a `float` local first (`orientation::vec3_eq` rounds). The two differ only when the sum rounds across
  `9.9999994E-11f`; worth aligning (and checking) when the agent's position equality is next touched.
- The roaming controller's `LookAt` of a sleeping unit (toward its next roaming point) and its `ag_pos` step stay
  inputs: roaming isn't modelled in combat replays (the dialogue runner models it). Porting it would compute the 1
  to 4 outside writes left per fight.

## The writers (2026-10-16, stream T)

### The code [C]

Every write of `DesiredOrientation` or `m_Orientation` in the decompiled game:

- **Inside `UnitEntityData`:**
  - the `Orientation` setter (writes both);
  - `UpdateSlowRotation` (`m_Orientation`);
  - the `DesiredOrientation` auto-property.

  And through those:
  - `LookAt(point)`: `DesiredOrientation = GetLookAtAngle(point)`, or, for a rider, the saddled unit's `LookAt`;
  - `ForceLookAt(point)`: `Orientation = GetLookAtAngle(point)`, the view's rotation when paused, and the saddled
    unit's too;
  - `Translocate`.
- **`GetLookAtAngle(point)`** (`orientation::look_at_angle`): `point.y = Position.y`. When `point == Position`
  (`Vector3 ==`) it returns `DesiredOrientation`; otherwise `Quaternion.LookRotation(point -
  Position).eulerAngles.y`, with the difference's components rounded at the constructor.
- **Turn-based combat's writers:**
  - `UnitCommandController.TickCommand`: before its `Tick`, a running command with `ShouldTurnToTarget` calls
    `TurnToTarget()`.
    - The base `TurnToTarget` is `Executor.LookAt(ApproachPoint)`.
    - `UnitMoveTo`'s does nothing.
    - `UnitAreaTransition`, `GroupMoveToInteractObject`, `UnitMoveAlongPath` and the tactical commands override it.
  - `UnitCommand.TickApproaching`: a unit that needn't approach stops and, unless it's a rider, calls
    `TurnToTarget()`.
  - `UnitAttackOfOpportunity.OnRun`: past the animation gate, when the attack's animation handle was made, and if
    `m_ShouldTurnToTarget`: `LookAt(Target.Position)`.
  - `UnitMoveTo.OnAction`: `DesiredOrientation = Orientation.Value` when the move has an orientation (roaming moves
    do).
  - `UnitCommand.OnRun` (`ForceLookAt`) and the merge in `UnitCommands.Run` into a parked command (`ForceLookAt`):
    both happen only while paused, and the commands model reports both paths.
  - `UnitMoveController.TickUnit`: the move's write (ported before) and the rider's copy (mounts aren't modelled).
  - Elsewhere, reached in combat as recorded:
    - `UnitRoamingController.TickOnUnit` (a sleeping unit's roaming `LookAt`);
    - `ContextActionMeleeAttack` and `ContextActionRangedAttack`;
    - `UnitCustomFreeAttackBase`;
    - `AbilityCustomDimensionDoor`;
    - `TranslocateUnit`;
    - summons, cutscene and dialogue commands, spawners.
- **`ApproachPoint`** is `GetTargetPoint()`:
  - the base one is `Target?.Point ?? Executor.Position`, where `Target.Point` is the target unit's position, else
    the wrapper's point;
  - `UnitAttack` and `UnitAttackOfOpportunity` keep the base one; `UnitUseAbility`'s is `Target.Point`;
  - `UnitMoveTo`, `UnitLootUnit`, `UnitInteractWithObject`, `UnitDestroyObject` and `UnitMoveAlongPath` override it.

### WrathOracle: observer `look` [M]

`CommandObsLook.cs`, declared `look`; first recorded in `poc5-tb-130`.

**What it records.** At install it scans the IL of the game's assemblies for every call site of the four members:
`LookAt`, `ForceLookAt` and the two setters (52 sites in 46 methods). It transpiles each site into a tap that
makes the same call and, once the call returns, writes `ag_look` with:
- `fn`, and the unit;
- the point (`p`) or the value (`v`);
- before the call, the unit's `m_Position` and whether it has a `RiderPart`;
- its `m_Orientation` and `DesiredOrientation` before (`o0`, `d0`) and after (`o`, `d`);
- the caller chain.

Where real time just ended, the agent's synchronization comes before the write. Reads are fields and
auto-properties only.

**What it leaves out.** The setters are small enough for Mono to inline, so the members aren't hooked themselves.
Two kinds of write have no `ag_look`, because another record shows them:
- the members' own inner sites: their callers' records cover them;
- `TickUnit`'s write to its own unit: `ag_rot`'s `o` and `desired` follow it.

**How it fails closed.**
- The install checks that only the `Orientation` setter and `UpdateSlowRotation` store `m_Orientation`, and only
  the `DesiredOrientation` setter stores its backing field.
- It fails on a site it can't tap (a generic method, a method group).
- It fails on a later transpilation that taps fewer calls.

gpt-6.1-sol reviewed it. Its findings, all fixed: the synchronization's order, the scan failing closed,
re-transpilation, and the `==` operator on `UnitEntityData`.

### The sim

- **`orientation.rs`:** `look_at_angle` (`GetLookAtAngle`), `LookFn`/`LookArg`, and `Orientation::write`
  (`LookAt`, `ForceLookAt` and the setters, for a unit without a saddled unit).
- **The commands (`commands.rs`):**
  - `turn_to_target`, called from `TickCommand` (running, `stt`) and `TickApproaching` (`approach.rs`).
  - `look_point` gives `ApproachPoint` for `UnitAttack`, `UnitAttackOfOpportunity` and `UnitUseAbility`. Another
    class's point is the recording's, and is counted.
  - `aoo::on_run`'s `LookAt` and `move_to::on_action`'s `DesiredOrientation`.
  - Each calls `Inputs::look`, which by default applies the write.
- **The driven replay (`replay/look.rs`):**
  - A write the sim's code makes pulls its `ag_look`, and checks:
    - the call and the unit;
    - the caller chain's innermost frames (`LookSite::frames`);
    - the argument, and that the unit isn't a rider;
    - the position and the orientation before;
    - the result computed by `Orientation::write`.
  - The attack of opportunity's `LookAt` is made when an `ag_look` comes before the commands' next lifecycle record
    (no other code runs between); the pull then checks it as above. Whether the handle was made is presentation,
    an input.
  - An `ag_look` no sim code pulled is a write by code the sim doesn't run, counted by call and caller. Its position
    and orientation before must be the sim's, and its result is computed from the recorded argument (a point's
    angle is the sim's).
  - Three such writes are reported instead: one whose caller is a site the sim runs, a rider's call, and the
    rider's copy in `TickUnit`. A caller chain must be known (not empty, not WrathOracle's `?`).
  - In real time writes are taken. A turn-based write after real time without the agents' synchronization is
    reported (WrathOracle writes the synchronization first).
  - Every `ag_look` must be reached: one inside a span the replay skips (a rule's) is reported at the end.
  - On a reported write or read the recorded state carries on, so one difference doesn't cascade.
  - `UpdateSlowRotation`'s read (`MoveInputs::desired_orientation`) is the sim's, checked against `ag_rot`'s
    `desired`. A span taken from the recording also takes its `ag_rot`'s `desired`, as it takes the orientation.
  - Older captures, without `look`: the reads stay the recording's, and the sim's writes still apply (but the
    attack of opportunity's, whose occurrence isn't known).
- **The coverage line:** `orientation: DesiredOrientation reads computed ... writes computed ... writes by code the
  sim doesn't run ...`.

### Result [M]

Per run, the same for the record run and both replays:

| Capture / fight | Reads computed (taken) | Writes computed: TickCommand, TickApproaching, AoO | Outside writes (inputs) | Taken in real time | Valid |
|---|---:|---:|---|---:|---|
| `-132/tb-nearest` | 9,577 (0) | 463, 466, 0 | roaming `LookAt` 1 | 2 | yes |
| `-132/tb-focus` | 15,051 (0) | 577, 565, 1 | roaming `LookAt` 1 | 0 | yes |
| `-131/tb-lizard` | 81,918 (0) | 2,465, 2,475, 1 | roaming `LookAt` 4 | 103 | no |

- **`-131`'s other reports belong to other ports:** the walls' meshes and the crates' boxes (`Unverified`
  raycasts), the dropped loot, its combat log, and a prone rider's turn. None is the agent's, the commands' or
  orientation's.
- **`-130`** is an earlier capture of the same plan:
  - `-130/tb-focus` computes 15,183 reads, and its record run is valid;
  - `-130/tb-nearest`'s game replays drifted a tick at a camera-scroll boundary (stream L pins it);
  - `-132` is the same plan recorded again, and is tick-exact.
- **What these fights don't reach:**
  - no outside write's argument is computed: the roaming points are the roaming controller's;
  - no point is taken from a class's own `GetTargetPoint`;
  - no `UnitMoveTo` orientation is set.
- **Before `look`**, on `poc5-tb-129` (tb-lizard, 58,079 reads), only 5 reads differed from the sim's writes, all
  after a sleeping lizard's roaming `LookAt`.

**The tests** (`crates/wr-sim/tests/poc5_look.rs`):
- `every_desired_orientation_read_is_the_sims` replays every run of the three fights: tb-nearest and tb-focus
  valid and T0-equal, tb-lizard with no report of the agent, the commands or orientation.
- `missing_or_wrong_look_records_are_reported` damages `-132/tb-focus` 32 ways, and each is reported:
  - the declaration missing, or `look` declared without `agent_sync`;
  - a computed write: missing; of another call, unit, caller or frame; with another argument, result, state before
    or position; a rider's; with an extra or wrong-kind field; duplicated;
  - the attack of opportunity's `LookAt` missing, wrong or by another caller;
  - a caller chain empty or unknown; a turn-based write before the synchronization;
  - a roaming write with another state before, result or point; missing; a rider's; of an unknown unit;
    unreadable;
  - a setter by an unknown caller whose result isn't its value;
  - a read changed.

### What stays an input, and what's left

- **Inputs, by cause:**
  - the point of a writer the sim doesn't run (the roaming controller's next point for a sleeping unit: 1 to 4
    writes per fight; the angle itself is the sim's);
  - whether the attack of opportunity's animation handle was made (presentation: the gate of its `LookAt`);
  - the desired orientation at the window's start (`ag_init`) and where real time ends (`ag_sync`);
  - writes in real time, which isn't ported;
  - a point a class's own `GetTargetPoint` gives: counted, none in these fights.
- **Reported where reached, not modelled:** riders' and mounts' writes (`LookAt`/`ForceLookAt` of a rider, the
  rider's copy in `TickUnit`), `TurnToTarget` overrides other than `UnitMoveTo`'s, the paused `ForceLookAt`s.
- **Left:**
  - port the roaming controller in combat replays, so its writes' points are computed too;
  - `-131`'s other reports, which belong to other streams: box and mesh raycasts, the dropped loot, its combat log,
    a prone rider's turn;
  - WrathOracle's `ReferenceEquals` hardening in `LookPre` (after review, on this branch) is in no build yet. It
    only replaces a null test that read the descriptor.

Reviews (gpt-6.1-sol, high):
- The observer, one round: the synchronization's order, the scan failing closed, re-transpilation, the `==`
  operator.
- The sim, two rounds:
  - records inside rule spans, now audited at the end;
  - the attack of opportunity's peek;
  - turn-based writes before the synchronization;
  - unknown callers;
  - the state after a reported record or read, which now carries on;
  - the optional write in older captures;
  - the validity test's T0 check.
- Then one pass on the final state.
