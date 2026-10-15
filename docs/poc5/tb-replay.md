# POC-5: turn-based replays follow their recordings (WrathOracle 0.4)

Written 2026-10-11 on Wrath-Mac `poc` (`c7117fd`, `6e7a760`, `ba96d22`; the code is in
`oracle/mods/WrathOracle`). Tags: **[M]** measured in the game on this Mac, **[C]** checked in
the decompiled code, **[I]** inferred. Starting point: Sol's investigation (`cache/poc5/sol/` copy
of `tb-replay-report.md`; its source-level claims held, its ranking mostly did). No game text here.

## Result

Record plus two replays of `tb-nearest` and `tb-focus` on the clean oracle build
(`builds/arm64-oracle`, `LOADED 4/4 MODS`), plan `tools/oracle-fixtures/poc5-tb.plan.json`
(spec `poc5-tb.spec.json`), `oracle-trace-diff record replay-N --t0`:

| Capture | Build | tb-nearest 1 / 2 | tb-focus 1 / 2 |
|---|---|---|---|
| `poc5-tb-14` | before the camera fix | pass (1 tick shifted) / pass (1) | T0-equal / T0-equal |
| `poc5-tb-15` | | pass (3) / **T0-equal** | T0-equal / T0-equal |
| `poc5-tb-16` | | **fail** / **fail** (order) | T0-equal / T0-equal |
| `poc5-tb-17` | final | T0-equal / T0-equal | T0-equal / T0-equal |
| `poc5-tb-18` | final | pass (1) / pass (1) | T0-equal / T0-equal |

"pass" = exit 0 under `--t0`: same T0 records in the same order with the same values, zero T0 tape
problems; "(n)" = largest tick shift; "T0-equal" = also tick-exact. [M] Final build: 8 of 8
comparisons pass (`poc5-tb-17`, `-18`); 18 of 20 since `-14`, the two failures being the `-16` nearest ones, which
the camera fix (below) removed. Replays use the recording's 2,100 to 2,800 ticks, 90 to 130 gates and 10
to 15 inputs; tapes are used completely (`tape n/n`); combat membership, initiative order, rolls,
HP, deaths and XP are equal. `oracle-state-diff`: start states equal in all replays; end states
equal for `tb-focus`, and for `tb-nearest` four round-timer floats differ in the last digits (the
one-tick shift). Both recordings `--check` valid. Certification re-run on the same build (`cert-12`,
`cert-13`): `dice` strict-equal, `dialog-checks` and `attack` T0-equal, perturbed replays fail,
selftest 6/6, as in `cert-10`. A baseline run of the previous mod on the same oracle build
(`cert-base-1`) behaves the same, so the build change (4 mods instead of 6) accounts for the
longer `dialog-checks` (498 ticks, was 419), not this work. One flaky `cert-11` replay ran 2 ticks
longer than its recording (`time_tape_exhausted`), which can happen to the real-time fixtures.

Not achieved: tick-exact replay of `tb-nearest` every time. About half of its runs show a
one-tick shift somewhere (animation event or scroll end); T0 order still holds then.

## What was found (ranked as in Sol's report, with the evidence)

All fixes apply only inside a window whose plan has a `tbm:` step (`Sched.Window`), so the
real-time fixtures keep their records. Each is a deliberate pinning of an environment input to
something the original could also see; the recording sees the same pinned values as its replay.

1. **Clocks and scheduler boundaries.** Sol proposed recording and replaying the wall clock. I fixed
   the clock instead (every run sees "every frame took exactly 1/60 s"), as `TimeController` already
   did in fixture mode, and made the boundaries gates that a replay must hit at the recorded tick
   (below). Replaying real wall deltas would need per-frame alignment across loads of different
   lengths and gives nothing the Rust sim could use. [I]
   - Combat-start window: `DelayedInvoker.InvokeInTime(realtime)` waits on a virtual clock (sum of
     fixed frame deltas), then calls the game's own `HideCombatStartWindow`. [M] Fires at tick
     411/412 in recording and replays (it was 4 real seconds: 293 vs 317 ticks).
   - Initiative tracker: DOTween's independent update takes its delta from `Time.realtimeSinceStartup`
     differences (`DOTweenComponent.Update` reads it twice [M, IL]). Its animation end releases
     `WaitingForUI` (`UnlockUI`). Replacing `Time.unscaledDeltaTime` reads alone was not enough; with
     DOTween's clock on a small fixed base (16 s plus fixed deltas; at uptime magnitude each delta rounded
     to a different float step, `dotween_dt` 0.0166626 vs 0.0166667) the guard release is at the same tick.
     The icalls `Time.get_unscaledDeltaTime`/`get_frameCount` can't be Harmony-patched (invalid IL);
     81 game methods that call the former are transpiled instead.
   - Camera scroll: ends at the first frame with `Time.time > start + duration`, float arithmetic at the
     magnitude of process game time. For a duration of whole frames the rounding decided n or n+1 frames
     by run (record early in the process, replays later), shifting the turn start and reordering other
     units' command events within the tick [M, `poc5-tb-16`: UnitAttack end of the PC vs Anevia's
     UnitMoveTo end at tick 448]. The rig's routines now read 1000 s plus elapsed game time (double
     difference).
   - Navigation refresh (`CombatController.UpdateNavigationGrid`) subtracts the absolute
     `Time.unscaledTime` from its timer [C]: uses 600 s plus window time (a long-running game).
2. **Recorded inputs are replayed.** `ReplayFight` makes the recorded attacks and end-turn inputs, through
   the same entry points, at the recorded tick moved by the offset built up so far, when the right unit
   is idle in the right turn status; the policy runs only while recording. Any departure ends the fixture
   (`Sched.Invalidate`), the runner skips to the window's `end`, and the run goes on with the next fixture.
   Readiness for new recordings: no queued command, no running command, no launched projectile (the
   game's own auto-end test) [C]. `tbfight_done` no longer carries a frame count.
3. **Passive observations** (`obs` records, left out of T0): path requests and completions, one row per
   unit whenever it changes (position, moving, in combat, commands, queue, animation blocking, memory
   size), combat joins, memory additions, physics steps per tick. `tools/oracle-obs-diff` lines two
   traces up by tick. They located four more sources (all [M]):
   - **Friends' visibility phase**: `UnitMemoryController.UpdateVisibility` checks non-enemies only on frames
     where `Time.frameCount % 5 == (target.GetHashCode() >>> 2) % 5` [C]; `UnitEntityData.GetHashCode()`
     is the object's identity hash, a different phase in every run. `mem_add` of the PC into Anevia's
     group at tick 83 vs 84/85, which delayed her combat join (the `tb-focus` pre-combat divergence, 170 vs 171).
     Inside a window that call uses a hash of the unit's id and the frame number is shifted so the window starts
     on a multiple of 60.
   - **Initiative tie-break**: `UnitsOrderComaprer` and `UnitEntityOrderComaprer` end with
     `unit.GetHashCode() >= unit2.GetHashCode()` [C]: two centipedes equal in everything (initiative, roll,
     dexterity, tie-break d20) swapped order between runs (`tb-focus` replay at tick 1693: 51DD vs 51FC).
     Same replacement. The tie order is arbitrary in the original; the port should take it from the recording.
   - **A* delivery**: a path requested in one frame is called back in the next, or one later if the worker
     thread was slow; `PathReturnQueue.ReturnPaths(timeSlice)` also stops after about 0.5 ms of wall time.
     `path_done` at tick 412 vs 413. Now `ReturnPaths` first waits (limit 250 ms, counted) until every path
     requested through `AstarPath.StartPath` reached the return queue, and runs without the time slice.
   - **Physics step phase**: fixed update 0.02 s against 1/60 s frames runs 0 or 1 steps per frame in a
     pattern whose phase depends on where the clocks stood at load (`~fx` rows). The step is set to the frame
     step at `prepare` and colliders sync before every query (`autoSyncTransforms`); restored at the window's end.
4. **Fail fast and reporting.**
   - Draws inside a rule carry `ctx` (rule class, initiator, target, ordinal in the rule; units made in the
     window by blueprint). In a turn-based replay a T0 draw with no tape value, with other member or arguments, or
     taken by another rule ends the fixture at once (the dice Sol found reassigned at `tb-focus` tick 968 would
     now stop the replay there, not at the first fallback at 1163). T0 sites are the ones that drew inside a rule in the recording.
   - `time_tape_exhausted` counts a missing tape position once (was three reads per tick); `round_start` only when
     the round advanced; `m_LaunchFrame` ignored at any depth.
   - `oracle-trace-diff`: skips `obs`, fails a replay with `replay_invalid`, ignores the skeleton-derived fields of a
     launched projectile snapshot (`CorePosition`, `m_LaunchPosition`, `Progress`, `MissTarget`, ...: bone poses differ
     by millimetres between runs, the projectile's `PassedTime`/`PassedDistance` and the attack outcome do not), and
     lists the unit bark picker (`LinqExtensions.WeightedRandom<BarkEntry>`) as an audited T1 site: which barks are
     eligible follows the audio engine (record [0,1], replay [0,0]); in turn-based windows it is a presentation site
     in the oracle too.

## Tick tolerance

Animation events (a strike, the end of an attack animation) and some scroll ends can land a tick earlier or later in a
replay than in the recording with nothing else different [M: `tb-nearest`, same inputs, same tick deltas, act event at
+49 vs +50 ticks]. Gates and inputs are therefore compared exactly in order and content and to within 2 ticks (3 for
inputs) of the offset built up so far (`Sched.Tol`); the replay makes an input no earlier than its recorded tick plus the
offset. Tick-exact equality is reported separately by `oracle-trace-diff` (`t0_equal`). Idle fidgets (micro idle, idle
variants; sites 167 to 175) start at times that depend on how many frames passed between unit activation and the window,
so their draws differ in count between runs; they are audited T1 (12 exhausted draws in `tb-nearest` replays) and have not
changed a T0 outcome since the camera fix. [I]

## Not fixed, and for the Rust port

- Pre-window state is not pinned: animation timers, a roaming unit's position (`5052`, probably the monitor lizard [I], differs at tick 0 in the unit rows), the
  physics and scroll phase before the window. Replays still start from equal save-level state.
- About half of the `tb-nearest` runs have a one-tick shift (the record is usually the odd one out: first window of the process).
  A tolerance-free fix would need the animation event arithmetic pinned too (not found).
- Pinned inputs the port must treat as inputs, not rules: friends' visibility phase, the initiative tie-break and A* delivery
  order are arbitrary in the original; the fixture says which one happened. The oracle's recorded windows run with the pinned
  clocks above (fixed 1/60 s frames), so timing-dependent T1 behaviour is that of a game that never drops a frame.
- `tbfight` recordings made before this are not replayable (inputs lack round/turn identity); re-record. The clean-build
  recordings: `fixtures/captures/oracle/poc5-tb-17` and `-18` (each: record, `replay-1`, `replay-2`).
- Open from before: five unhooked RNG sites; `System.Random`/`PersistentRandom`.

## Running it

```
dotnet build -c Release oracle/mods/WrathOracle/WrathOracle.csproj   # game closed; installs into builds/arm64-oracle/Mods
scripts/oracle-record.sh LABEL Manual_18 oracle/tools/oracle-fixtures/poc5-tb.plan.json --deadline 3600 --allow-unhooked 5
oracle/tools/oracle-trace-diff record.trace.jsonl.gz replay-1.trace.jsonl.gz --t0
oracle/tools/oracle-obs-diff   record.trace.jsonl.gz replay-1.trace.jsonl.gz     # first unequal observation, by tick
```

A replay that left its recording says where in its `run.json` fixture line (`INVALID: ...`), in the trace's last `obs` record and
as `replay_invalid` in the end record. New code: `Sched.cs` (gates, input check, invalidate), `WallClock.cs` (clocks, hashes,
DOTween, physics step, camera time), `PathSync.cs`, `Observe.cs`, and the replay half of `TurnBased.cs`.

## WrathOracle after the rework (2026-10-11, fourth session): replays reproduce their recordings

Wrath-Mac `234fdef` (on `main`), the clean oracle build. Final result: **`poc5-tb-29`**, all four replays valid and T0-equal
tick-exact (last bullet below). The first build to pass, **`poc5-tb-24`** (record plus two replays each of
`tb-nearest` and `tb-focus`, recorded on the unmodified game): all four replays are valid (every gate, input, path delivery
and navigation clock read reproduced, tapes used completely) and pass `oracle-trace-diff --t0`; `tb-focus` is tick-exact,
`tb-nearest` has the same T0 records in the same order with ticks shifted by idle-animation phase. The certification
fixtures (`cert-14`, `cert-15`): `dice` strict-equal, `attack` T0-equal, `dialog-checks` T0-order-equal (cert-12/13 were
tick-exact; the replay's game mode changes one tick apart at tick 466 of 498, T1), perturbed replays fail. The selftest
passes, with three new projectile cases. [M]

What it took, in the order found (each run's capture is kept):
- **`poc5-tb-19` (invalid after the first gate):** the `path_ret` observation had a field named `k`, the trace's record
  kind. `JObject.Parse` keeps the last duplicate, so the tape loader read the kind as `"0"` and dropped every delivery;
  the replay then held the PC's first path forever, its attack never moved and combat never began. The field is `path`,
  and the loader takes the kind from the line prefix it matched. [M]
- **`poc5-tb-20` (every T0 record equal, one unaudited site):** the bark exemption never applied. `Bark.Schedule` holds
  an RNG site, so it is transpiled, and a Harmony-patched method runs as a dynamic method whose stack frame has no
  declaring type. Stack walks now map frames through `Harmony.GetOriginalMethodFromStackframe` (bark check, global site
  keys, guard descriptions, id traces). [M]
- **`poc5-tb-21`:** `tb-focus` passed. `tb-nearest` differed only in projectile geometry (launch position up to 6.4 cm,
  miss point 1.8 cm): an extra idle animation before the attack leaves the launcher's skeleton in another pose.
  **Ember (2026-10-11): skeleton-derived projectile fields are T1** under `--t0` (`CorePosition`, `m_LaunchPosition`,
  `Progress`, `PassedTime`, `PassedDistance`, `MissTarget.m_Point`): reported as `t1_projectile_geometry` (largest
  deviation per field), not compared. The attack's outcome is decided before launch, and the Rust port can't reproduce
  bone poses either; the hit's tick stays under the tick tolerance. Strict (non `--t0`) comparisons still compare them.
- **Astra review of the rework and these fixes (6 findings, all real, all fixed):** path gating moved to the worker's
  hand-over (`PathReturnQueue.Enqueue` holds every tracked path in a replay; due paths are released into the queue
  before each `ReturnPaths`, so nothing finishing mid-drain is delivered early); deliveries are checked completely
  (timeouts, deliveries at another tick or call, extra or missing requests, unused deliveries, off-thread requests all
  invalidate); a recording notes each delivery in `Path.ReturnPath` (pooled paths are reused, so tracks carry the
  `pathID`); the camera rig keeps a running routine's clock offset until it is idle; the navigation clock reads are
  observed and replayed (`nav_clock`); the projectile exemption applies only under `--t0`.
- **`poc5-tb-22`:** every replay timed out on its first path: `Sched.On` is main-thread only, and `Enqueue` runs on an
  A* worker, so nothing was held. **`poc5-tb-23`:** `tb-focus` tick-exact; `tb-nearest` replay 1 made one navigation
  clock read fewer (`m_NavmeshUpdateTimer` survives from earlier windows: now recorded at window open, `nav_timer0`,
  and set in replays), replay 2 delivered a path a tick late because its request came a tick late. Now a replay checks
  each request against the recording's (`path_start`: ordinal and end point, 0.25 m) and delivers it the recorded number
  of ticks after its own request, at the recorded call.

- **Astra rounds 3 to 5 (all real, all fixed):** requests are registered in a `StartPath` *prefix* (it publishes to the
  workers before returning); held paths go back to the game at window end or abort, with interception closed under the
  same lock first; a replay counts and checks each delivery in `Path.ReturnPath` (a throwing callback that cuts a drain
  short is caught); `oracle-trace-diff` bounds T0 records' tick shift (`t0_max_tick_shift` ≤ 4, the scheduler's drift
  bound; measured 0 or 1) and its selftest has a late-hit case and a strict-mode control.
- **`poc5-tb-25`:** a `tb-focus` replay was invalidated because the roaming monitor lizard's request took the global
  ordinal a combatant had in the recording. Requests are now keyed by requester (`<unit>#<n>`, the unit whose `Seeker`
  the path calls back; the `Seeker` sits on the unit's `_navigation` child) and only units in combat or of the player's
  faction are pinned; others (a roaming animal: T1 timing) are delivered natively. `poc5-tb-26`/`-27` ran with the lookup
  still failing (every key `?`, all requests pinned by global ordinal) and passed, tick-exact.
- **`poc5-tb-28`:** the game deadlocked an hour into the unattended recording (main thread on a dispatch semaphore in
  UnityPlayer, the startup-hang signature; sample in `fixtures/captures/runs-raw/startup-hang/`). `scripts/oracle-record.sh` now
  kills and retries a game under 3% CPU for 3 minutes.

- **`poc5-tb-29` on the final mod (Wrath-Mac `234fdef`):** requests keyed per unit (the PC and four combatants, no `?`),
  all four replays valid and **T0-equal tick-exact**.

Recordings that count: **`poc5-tb-29`** (turn-based) and **`cert-16`** (real-time: dice strict-equal,
dialog-checks T0-equal, attack order-equal with a 1-tick shift, perturbed replays fail). Everything earlier and the
superseded set below are kept for reference only.

## Player input isolated at the source, the physics phase pinned (2026-10-12)

Wrath-Mac `8fa7cc6`. Fixes Astra's review of the `Pointer.cs` tree (six findings, all real) and takes
the visibility frontier that broke `poc5-tb-38` under load. Final: **`poc5-tb-51` and `-52` under load and `poc5-tb-53` idle, each 4/4 replays valid and T0-equal tick-exact,
every fixture clean; `cert-19` certifies**. [M]

### Player input: neutral at the source (`InputPin.cs`)

`Pointer.cs` skipped `PointerController.Tick` and disabled the EventSystem; that froze `PointerOn`, the
pointer mode and the click handlers that `TurnController.UpdateActionPredictions` uses, and left keyboard
(`KeyboardAccess`, so `Game.PauseBind`), Rewired, camera (edge scroll, middle button, scroll wheel) and tooltip
reads live [C, Astra 1-4]. It is replaced by a pin on the input itself. From a fixture's `prepare` (before
its reload, so the window opens on pointer, camera and UI state no hardware touched) until its window has
closed (after `Trace.End`), in recording and replay alike, and only with the fidelity hooks:
- **Every reader of `UnityEngine.Input` in the game's assemblies is transpiled** to a replacement with the same
  signature (the getters are native calls; method groups such as `AltCodes.Any(Input.GetKey)` are `ldftn` and
  replaced too; open generics through their closed instantiations in the game's IL). While pinned: pointer at
  (100000, 100000), far off screen and off every edge band, no button, key or touch, zero axes (scroll wheel
  included), no text. `Event.PopEvent` (UI text fields) returns nothing. [M] 161 reading methods, 163 transpiled
  (Assembly-CSharp 99, firstpass 35, UnityEngine.UI 13, VisualEffectGraph 8, RenderPipelines.Core 4,
  TextMeshPro 4; 2 open generics through 4 instantiations), 0 failed; 28 members replaced. A failure fails the
  install (health), so a capture with one is uncertifiable.
- **Rewired**: every controller is disabled for the pin (one here, `Mouse`) and re-enabled after; one
  connecting meanwhile is disabled too.
- **Held, then delivered in order after the window**: every `OnApplicationFocus`/`OnApplicationPause` of the
  game's assemblies (24 with the next item; `Kingmaker.Runner`'s drives `KeyboardAccess` key-ups and AutoPause,
  the EventSystem's stops UI processing) and `GamepadConnectDisconnectVM`'s hot-plug handlers (can pause).
- **IMGUI**: the game's 23 `OnGUI`s see the neutral pointer, no delta or modifiers, and input events used.
- **The pointer's world pick** (`PointerController.SelectClickObject`, a camera ray) picks nothing, or the unit
  the fixture points at, with the first handler of the pointer's mode whose priority is above 0, as the
  game's loop does. The rest of the pointer, camera and UI logic runs as usual on that input.
- **Pending state at the pin's start is cancelled, not completed**: press/drag of both pointer controllers,
  the EventSystem module's pointer data and selection (`DeactivateModule`), IMGUI captures, a camera gesture
  (rotate/drag latch, base point), queued camera input, a pending zoom, a rotation tween the player's input
  started (mouse, keys, gamepad, the camera-reset key). At the window's start a stale hover is cleared through
  the pointer's own Tick and the pointer's last world position is the main character's (what turn-start
  terrain previews go to). [M] At the first `prepare` of every run the pointer was over `TERRAIN`: the
  hardware cursor.
- **A window must open on a reload made under the pin** (`prepare`, then `reload`; else the run's health
  fails): the load puts the camera at the save's position and rotation. The camera at the window's start
  (rig position, yaw, zoom, field of view) is in the trace header, and a turn-based replay whose camera
  differs from the recording's by more than 1e-3 is invalid.

**The fixture's attack** (`TurnBased.IssueAttack`): the pointer goes to the target (`InputPin.PointAt`, then the
game's own `PointerController.Tick`: hover, highlight, `ClickUnitHandler`), the portrait hover asks for the
predictions at once, and the command gets the predicted path. Before, with the pointer over nothing,
`UpdateSelectedClickHandler` chose `ClickGroundHandler` (priority 0.1 for no object) and the "attack" carried a
ground-move preview as its `ForcedPath` [M: `poc5-tb-36`, an attack on 523B whose path ended 21 m from it; now
2 m]. The fights changed (tb-nearest 2,619 to 2,764 ticks, was 1,990), so recordings before this don't replay.
`PointAt` checks the pointer got there; the policy only targets units a pointer can be over (visible,
targetable, read passively) [M: `poc5-tb-42`, the focus policy chose 51FC, not visible to the player, and the
check stopped the recording].

### The visibility frontier: the physics phase (`PhysicsPhase.cs`)

New passive observations (`vis`: vision-candidate changes; `los`: sight-cache misses with sample points and
result; `los_clear`; `los_geometry`; the memory-add caller; fog in unit rows; rows for every unit existing at
window open) show the path [M, `poc5-tb-40`]: enemy 523B gets the PC into `CanBeInVisionRange` at tick 160 (an
overlap query on the entity-bounds physics scene), samples line of sight then (false, out of range), samples
again when the PC has moved 0.5 m (tick 170, true: `UnitMemoryController.UpdateVisibility` adds it) and combat
starts. So the join tick follows the candidate entry tick in steps of about ten ticks. `Physics.autoSyncTransforms`
is off [M], so the entity-bounds colliders (`EntityBoundsController` moves them every tick) reach the physics
scene only at a fixed step, and with 1/60 s frames against 0.02 s steps one frame in six runs none. Which one
is set by the phase phi = `Time.time - Time.fixedTime`, which differs per run because the reload's frame count
varies [M: 0.0167 vs 0.0067 in `poc5-tb-40`; in `poc5-tb-38` the step-less ticks were 4 mod 6 in the
recording and 0 mod 6 in the failing replay, which joined at 173 instead of 172]. [I] The overlap then sees the
PC a tick older on different ticks, the entry tick moves, the join with it.

Pin (replay only, turn-based windows only, to the recording's value): a recording writes phi at its window's
start and the frames from its reload's end. A replay, from its own reload's end, steers phi in pre-window
frames (one fixed step of another length, or a frame without one, then the native step) to the value whose
normal evolution reaches the recording's phi on the frame its window opens. A replay whose phi, frame count or
physics settings (`autoSyncTransforms`, `autoSimulation`, `fixedDeltaTime`) differ is invalid at once. [M] Every
replay since `poc5-tb-41`: 1 or 2 steered frames, phi equal to the recording's to the last bit.

### Before the window: the roaming lizard's path (`PrePathSync.cs`)

[M] `poc5-tb-46` under load: one replay valid but not T0-equal, because the roaming monitor lizard (5052, out
of combat, in fog) was exactly one tick behind on its move at tick 0 (x 87.1714 against 87.1836) and its
`UnitMoveTo` ended at tick 10, not 9. Pre-window path notes (`pre_path`, from the reload's first idle frame) show
its request 93 frames before tick 0 delivered in the same frame in every run of `poc5-tb-48`; a worker that
hasn't finished by that frame's `ReturnPaths` delivers it a frame later. Pin (replay only, turn-based): from
the reload's first idle frame (a fixed number of frames before the window) requests are keyed and noted as in
the window; a replay holds each matched one and delivers it the recorded number of frames after its own
request, at the recorded call, waiting (bounded) for the worker; a recorded delivery not reproduced
invalidates the replay at window open. [M] `poc5-tb-49` on: `5052#0` requested and delivered at -93, call 1,
in recordings and replays.

Due paths (in the window and before it) are now delivered by themselves, as the queue's drain delivers each
(`PathSync.DeliverNow`), before the native drain, which keeps its own wall-clock time slice for untracked
paths; before, the slice was off for every replay call. A replay call that must deliver tracked paths while
untracked ones are already queued is invalid (the recording tracked everything in its window, so it has no
order for that).

### Idle trackers (`IdlePin.cs`)

[M] `poc5-tb-41`/`-42` under load: replays left 3 T1 draws of a combat micro idle the recording made. The
units' idle trackers (`UnitAnimationManager.m_RandomIdleTracker`) are made during the reload; their times are
equal across runs but their `Random.value` draws are not (the generator's draws during loading depend on load
time). A recording observes each tracker at the first tick (`idle0`: time, draw, curve, whether an idle is
playing); a turn-based replay takes the recording's time and draw when the curve and playing state match,
otherwise notes `idle0_unpinned`. These draws stay T1.

### PathSync

A replay now owes the recorded delivery of every lenient request it matched, not only strict ones (Astra 5).

### Validation

`oracle-trace-diff record replay-N --t0` per replay; "clean" = no health problem in `oracle-run` (tapes used
completely). Load = 8 busy `yes` processes for the whole run.

| Capture | Build | Load | tb-nearest 1 / 2 | tb-focus 1 / 2 | Notes |
|---|---|---|---|---|---|
| `poc5-tb-40` | input pin, no phase pin | idle | T0-equal / T0-equal | T0-equal / T0-equal | 2 install failures (open generics) |
| `poc5-tb-41` | + phase pin, generics | load | T0-equal / T0-equal | T0-equal / T0-equal | 3 T1 idle draws left over in tb-nearest |
| `poc5-tb-42` | + review 1 fixes | load | T0-equal / T0-equal | (recording stopped) | focus target not pointable; idle leftovers |
| `poc5-tb-43` | + idle pin, pointable targets | load | T0-equal / T0-equal | T0-equal / T0-equal | all clean |
| `poc5-tb-44` | same | idle | T0-equal / T0-equal | T0-equal / T0-equal | all clean |
| `poc5-tb-45` | + review 2 fixes | load | T0-equal / T0-equal | T0-equal / T0-equal | all clean |
| `poc5-tb-46` | + camera gesture reset | load | **not T0-equal** / T0-equal | T0-equal / T0-equal | lizard a tick behind (pre-window path) |
| `poc5-tb-47` | + camera check, reload required | load | T0-equal / T0-equal | T0-equal / T0-equal | all clean |
| `poc5-tb-48` | + pre-window path notes | load | T0-equal / T0-equal | T0-equal / T0-equal | all clean |
| `poc5-tb-49` | + `PrePathSync` | load | T0-equal / T0-equal | T0-equal / T0-equal | all clean |
| `poc5-tb-50` | + review 6 fixes | load | pass (1) / T0-equal | T0-equal / T0-equal | a projectile hit a tick later (below) |
| **`poc5-tb-51`** | **final, `8fa7cc6`** | load | T0-equal / T0-equal | T0-equal / T0-equal | all clean |
| **`poc5-tb-52`** | final | load | T0-equal / T0-equal | T0-equal / T0-equal | all clean |
| **`poc5-tb-53`** | final | idle | T0-equal / T0-equal | T0-equal / T0-equal | all clean |

"T0-equal" is tick-exact (`t0_max_tick_shift` 0); "pass (1)" is the same T0 records in the same order with one tick
shifted. Certification (`certification.plan.json`, Manual_13): `cert-18` (build of `-43`) dice strict-equal,
dialog-checks T0-equal, attack order-equal (1-tick shift, as `cert-16`); **`cert-19` (final)** dice strict-equal,
dialog-checks order-equal (1-tick shift, as `cert-14`/`-15`), attack T0-equal; perturbed replays fail in both.
Selftest 10/10 (projectile cases included) on `poc5-tb-43` and on `poc5-tb-52` tb-focus.

### Reviews

GPT 6 Astra (high), nine rounds, each finding verified against the code and fixed: (1) ten (hot-plug, IMGUI,
pointer verification and window baseline, focus delivered while the trace was open, the memory row still
filling a cache, missing physics validation, time scale in steering, Rewired readiness, hover cleanup, opening
rows); (2) six (uGUI module state, the policy's group getters, idle tracker curve/playing state, IMGUI
coordinates, release cleanup, disposed recipients); (3) one (camera gesture latch); (4) two (queued camera
input, a user rotation tween); (5) three (camera-reset tween, pending pan, pending zoom: the reload requirement
and the camera check); (6) two (pre-window sync lifecycle, the time slice); (7) one (time slice in calls with
due paths: isolated delivery); (8) one (order within a mixed call: invalidated); (9) nothing substantive.

### Not fixed

- JIT-inlined copies of tiny Input readers compiled before the install (it runs at mod load, before the main
  menu) would bypass the transpiles [I]; nothing observed.
- Unity Mod Manager's own window (outside the game's assemblies) still sees the hardware; debug-only IMGUI
  windows keep their own drag flags (UberLogger); neither is open in fixtures.
- The phase and idle pins apply to turn-based replays only; real-time fixtures get the input pin.
- Other pre-window animation state (a playing idle's remaining time) is observed through `idle0` only.
- Animation timing on Unity's absolute clock (`PlayableInfo.CreationTime` is `Time.time`, rounding by process
  uptime): `poc5-tb-50` had Anevia's projectile hit (`RuleAttackWithWeaponResolve`) a tick later in one replay,
  launch position 2.7 cm apart (skeleton pose, T1 by Ember's decision), the flight crossing a tick boundary
  differently. Order-equal, within the tick tolerance; seen once in the 54 replays of this work. Astra's ranked
  item 5 (absolute clock reads: animation, AI deadlines in `AiBrainController`) is the next frontier.

Recordings that count: **`poc5-tb-51`, `-52`, `-53`** (turn-based) and **`cert-19`** (real-time), all on
`8fa7cc6`. Recordings made before `8fa7cc6` don't replay on it (physics and camera headers, the fixture's attack
path); `poc5-tb-40` to `-50` are kept as the evidence above.

## Projectile arrival and critter paths (2026-10-15, stream L)

`poc5-tb-119/tb-lizard` (Manual_18, plan `poc5-tb6`) recorded cleanly, but both replays were invalidated. A
gpt-6.1-sol investigation found two independent causes; each claim below was checked against the traces and
the code before the fix. Code: `ProjPin.cs` (new), `PathSync.cs`, `PrePathSync.cs`, and their wiring in
`Sched.cs`, `Rng.cs` (the tape) and `Fixture.cs` (end record, observers, the replay's checks).

### Projectile arrival is pinned (`ProjPin.cs`, observer `proj_arrival`)

[M] Anevia's killing arrow (`RuleAttackWithWeapon` 526, attack roll 529, 5198 on the fly 529A) launched at tick
6918 in all three runs with the same snapshot (launch position equal to the last bit), but hit
(`Projectile.OnHit`: `RuleAttackWithWeaponResolve` 537, the damage 538) at 6938 in the recording and 6937 in both
replays, one flight step fewer (`PassedTime` 0.3333 against 0.3167). The same damage draw followed a tick early,
the fight ended a tick early (`tbfight_done` 6940 against 6939), and the scheduler's tolerance (`Sched.Tol` 2)
carried the one-tick lead (`max_tick_offset` 1, two offset changes) until a UI gate at 9898 brought it back.
`poc5-tb-50` had the same kind of shift once, a tick late. [C] `Projectile.Tick` moves toward
`GetTargetPoint()`, the target unit's hit bone (an animated transform) plus an offset, at the blueprint's speed
times the frame's delta, and sets `IsHit` within 0.1 m (or at its maximum range); `ProjectileHitController.Tick`,
later in the same game tick, calls `OnHit` for a projectile that hit this frame, and `OnHit`'s trigger or callback
makes the rules. Bone poses differ by millimetres to centimetres between runs (T1, Ember 2026-10-11), so the
number of steps a flight takes can differ by one. [I] Animation's absolute-clock arithmetic is the likely source
of the pose difference; the captures don't sample the bone per flight tick, so that is not established.

[C] What the game derives from a flight goes through `IsHit`: `OnHit` (`HitThisFrame` is `IsHit` with no
time after the hit), `ProjectileController.HasLaunchedProjectile` (holds `TurnController`'s turn end) and
`HasActive`, `OnTheWay` (the impatience controller), the cleanup (`Destroyed`), and the ability deliveries that poll
`IsHit` (`AbilityDeliverProjectile`, `Touch`, `Chain`, `Ricochet`, `ClashingRocks`, `ProjectileOnGrid`). Two more
fields are read: `PassedDistance` by the line and cone deliveries every tick (`StepBasedDeliver`: which units the
line has reached; the hit clamps it to `MaxRange`), and `IsHitWall` by `AbilityDeliverAttackWithWeapon`. Nothing
reads `CorePosition`, `PassedTime` or `Progress`. So the pin is the tick at which `IsHit` becomes true, with the
distance and the wall flag as the arrival leaves them; the flight itself (positions, the view) stays native.

- **Recording** (changes nothing): every projectile added to the controller in a turn-based window
  (`ProjectileController.AddProjectile`, postfix) is keyed `<launcher unit>#<n>`, n counting that launcher's
  launches in the window (`pt#<n>` for a launcher that isn't a unit), and noted: `proj_launch` (`p` the key,
  `rule` the open rule's id, `roll` the attack roll's rule id, `trig` the on-hit trigger's class or null, `cb`
  whether it has a callback, `tgt` the target unit or null, `bp` the blueprint, `res` the attack result), then
  `proj_hit` at the tick `IsHit` became true (`fl` ticks since the launch, `wall` its `IsHitWall`, `pd` its
  `PassedDistance`). A projectile launched inside one of the two decorative launchers (`ProjectileSpawner.Fire`:
  a scene's ambient spawner, random targets, always a miss; `FxProjectileLauncher.LaunchProjectile`: a scene
  effect on a delay) is presentation: `proj_launch` with `pres`, never pinned (marked whenever it is launched,
  so one launched before a window and landing inside it is still known as decorative). Every other one is pinned, a
  point-launched ability projectile's too (the roll and range an ability sets after the launch don't count: the
  key's contents are what the launch had). The fields are read, not resolved through getters that cache
  (`UnitRef.Id`, not `UnitRef.Entity`).
- **Replay**: a launch must be the recording's: same key and the same contents, on the recorded tick plus the
  offset so far (exactly: unlike the gates' tolerance, since the flight's length is gameplay, the distance a line
  ability has swept by each tick). Its hit is due on the recording's hit tick (plus that offset, 0 in a
  tick-exact replay), so the hit and what it triggers follow the recording. Before then a hit the native flight makes is taken
  back right after `Projectile.Tick`, before anything reads it: `IsHit`, `IsHitWall` if that tick set it, and
  `PassedDistance` back from the range it was clamped to to the sum `Tick` made (the same float expression), so
  a line ability's sweep sees the recording's distance every tick. On the due tick `IsHit` is set if the flight
  hasn't arrived (with `PassedDistance` clamped to `MaxRange` as `Tick` does, unless the recording's arrival was
  a wall's, which `Tick` doesn't clamp), `IsHitWall` is set to the
  recording's, and a `PassedDistance` other than the recording's (bit for bit) invalidates. A recorded projectile
  that never hit in its window doesn't hit in the replay's. The replay's `proj_hit` says what the pin did
  (`held`: native hits taken back, `forced`).
- **Invalid**: a launch the recording didn't make, or made with other contents or at another tick; a recorded
  launch that didn't happen by the window's end; a due hit that didn't happen (the projectile cleared or not
  ticked: checked after every `ProjectileHitController.Tick`, a finalizer, so also when it threw); `OnHit` on
  another tick than the hit, or one that threw (`proj_onhit_exc`) or never completed; another distance flown at
  the hit; a projectile launched before the window hitting inside it (`proj_untracked`, unpinned: in a replay at
  once, and a recording that has one gives its replays nothing to follow); a recording without `proj_arrival` in
  its observers, or whose launch and hit records lack or mistype a field (a hit's `fl` must be its ticks since
  the launch; ticks from 0), or with an `OnHit` or a `Projectile.Tick` that threw (`proj_tick_exc`: the
  controller clears that projectile; a replay's own is invalid too).
- End record (turn-based windows): `projectiles_tracked`, `projectile_hits_held`, `projectile_hits_forced`.

Why this pin and not another [I]: taping the target point per flight tick would keep the native arithmetic but
not its result, since the launch position (a launcher bone) differs between runs too (T1); pinning animation
clocks is broader and not shown to be enough. A held hit leaves the arrow at its target for the held ticks, a
forced one ends the flight up to one step (0.4 m for an arrow) short: presentation only. For the Rust port the
arrival tick is an input, like an A* delivery: the recording's flight length in ticks (`proj_hit`'s `fl`).

### Critters' path requests are presentation (`PathSync.cs`, `PrePathSync.cs`, observer `critter_paths`)

[M] The invalidating request `?#0` (record t=10021, replay-1 t=9947, replay-2 t=10503, three different end points)
comes right after four presentation draws in every run: sites 192 to 195, `Kingmaker.Visual.Critters.Rabbit`'s
`Think` coroutine (`Random.value`, `insideUnitCircle`, two `Range`), unforced and unequal. [C] `Rabbit.Think`
picks a roam point with exactly those draws and calls `CritterMoveAgent.PathTo`, which asks its own `Seeker` (on a
`_navigation` child `CritterMoveAgent.Init` makes) for a path, if `AstarPath.ProcessorQueueLegth` is at most 2.
The coroutine also runs on `Time.frameCount % 5`, a static spawn counter, idle deadlines from `Time.time` and the
camera's distance (it sleeps beyond 25 m). A rabbit isn't a unit, so `PathSync.RequesterKey` found no
`UnitEntityView` above the `Seeker`, named it `?` and kept it strict: every run checked an untaped decorative
choice against the recording. No unit reads a critter's state; a critter reads the units (`MaybeRunAway`).

- **Identification, positive** (`PathSync.CritterOf`, `IsCritterRequest`): the path's callback is a `Seeker`
  whose parent object carries a `CritterMoveAgent` holding that `Seeker` as its `m_Seeker`, with no unit view or
  unit movement agent above it, and, at the request (`AstarPath.StartPath`, inside `Seeker.StartPath`), the
  callback that `Seeker` will hand the path to (`tmpPathCallback`) is that critter's own `OnPathComplete`. Fields
  and components are read, nothing called.
- **Excluded, in recordings and replays alike**: such a request is noted (`path_critter`: `path` = `critter#<n>`,
  n counting critters' requests in the window, `e`), keyed for the path observers (`path_fin`, `path_seek`,
  `path_mod` name it), and delivered natively: no start check, no delivery tape (no `path_ret`), not held,
  not owed, not in `paths_tracked` (end record: `paths_critter`). Before the window (`PrePathSync`) a critter's
  request is neither noted nor held. Every other requester the lookup can't name stays `?` and strict.
- **The shared queues.** Critters' paths go through the same A* processing queue and return queue as the units'.
  [C] Their callbacks (`Seeker.OnPathComplete`, `CritterMoveAgent.OnPathComplete`) change only the critter, the
  seeker's own path and the path pool (which `Path` object a later request reuses; `PathSync` tells requests apart
  by `pathID`), so where a critter's delivery falls among the units' within one `ReturnPaths` call changes nothing
  a unit reads. The replay's check of a call that delivers tracked paths while untracked ones are already queued
  (no recorded order between them) therefore leaves positively identified critter paths out
  (`QueuedUntracked`); any other untracked path still invalidates. What critters can change is the order in which
  the workers finish the units' paths (a worker busy with a critter's search): the recording's drain delivered the
  units' paths of one call in the order its workers finished them, and the replay used to release its held
  paths in its own finish order. Now paths due at one call are delivered in the recording's order (the order of
  its `path_ret` records; before the window, of its `pre_path` "ret" notes), whatever order this replay's workers
  finished them in. Before this change any two unit paths finishing in another order could reorder their
  callbacks unnoticed (the check was by tick and call only).
- Not handled [I]: a unit's request still queued (not yet taken by a worker) when the main thread blocks the
  queue for a graph update in the same frame is searched on the updated graph, one already taken on the old
  graph: a wall-clock race any concurrent search takes part in, critters' among them, which no strict check of
  critters' requests ever covered. Turn-based combat's grid updates run at a turn's start through a synchronous
  `FlushGraphUpdates`; no capture shows the race.
- A recording without `critter_paths` in its observers isn't replayable on this build (its critters' requests
  are `?`): the replay is invalid at once.

### Results

Pending: the main session's captures of `poc5-tb6` (`tb-lizard`) and `poc5-tb` (`tb-nearest`, `tb-focus`).

## Status at the previous wrap-up (Astra review of b26f3a1..ba96d22; superseded by the section above)

**Everything above describes the earlier mod (`c7117fd` to `ba96d22`), whose recordings do not observe the unmodified game
(physics step and sync, replaced identity hashes, shifted frame and camera clocks, waiting for A* in recordings).
Superseded, kept on disk: `poc5-tb-14` to `-18`, `cert-11` to `-13`, `cert-base-1`. Their results are not evidence of fidelity.**

Code for the review, on Wrath-Mac `poc` (WIP commit, compiles, only one game run): fixed in code: physics pinning removed (steps per
frame observed only); identity hashes recorded natively and replayed per unit from the replay's `prepare` (never changing at window
open); frame number, camera `Time.time` and navigation `unscaledTime` pinned in replays only, to the recording's header values; DOTween
pin only in turn-based windows with ownership and restore; camera clock switches only while the rig is idle; bark exemption by caller
(`UnitAsksComponent`) in the oracle, removed from `oracle-trace-diff`; delayed callbacks bound to their window generation; total gate
drift bounded (4 ticks, `max_tick_offset` reported); unused gates/inputs invalidate at close; path delivery recorded and replayed
(no wait while the work-item lock is held); projectile fields all compared (tolerance only `CorePosition`/`m_LaunchPosition` 0.02 m,
`Progress` 0.002, `--t0` only; `MissTarget` exact); `oracle-obs-diff` fails on invalidation.

Not done: the first run on this build (`poc5-tb-19`) diverged immediately (replay invalid after the first gate, 62 gates unused), cause
not found, so no turn-based re-record or certification re-run exists for the corrected mod; the cert fixtures were not re-checked
against the new code (the changes are gated on turn-based windows, unverified). Next: debug `poc5-tb-19` with `tools/oracle-obs-diff`.
