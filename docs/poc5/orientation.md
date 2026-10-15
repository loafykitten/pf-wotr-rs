# POC-5: units' orientation

Status (2026-10-15): units' orientation as `UnitMoveController.TickUnit` writes it, `UpdateSlowRotation` turns it
and `UnitMovementAgent.InitMoveDirection` reads it is ported and bit-identical against every recorded `TickUnit`
and every recorded `PathTo` that reaches `InitMoveDirection` (`poc5-tb-100` to `-103`). Pure functions, not yet
driven: the integration comes with `PathTo` and `UnitMoveController` (`docs/poc5/movement.md`, step 2). Tags as
elsewhere: [M] measured, [I] inferred, [C] from the decompiled code.

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
- `DesiredOrientation` is set outside `TickUnit` (`UnitEntityData.LookAt`, from commands and the AI; 1,986 slow
  turns follow it): `GetLookAtAngle` (`LookRotation(point - Position).eulerAngles.y`, with `point.y = Position.y`
  and the `point == Position` case) and `ForceLookAt` aren't ported yet; the recordings don't show their calls.

## Observer changes requested

None for what this slice checks: everything it reads is recorded. For the next step (porting `LookAt`,
`ForceLookAt` and `GetLookAtAngle`, which the commands call), a record per call would let them be checked:
`ag_look` on `UnitEntityData.LookAt(Vector3)`, `ForceLookAt(Vector3)` and the `Orientation` setter outside
`UnitMoveController.TickUnit`, with the unit, the point, `Position`, the saddled unit if any, the caller chain
(as `paused_origins` does), and `m_Orientation`/`DesiredOrientation` after.
