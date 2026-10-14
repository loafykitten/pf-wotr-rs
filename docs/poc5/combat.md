# POC-5 combat: handoff (round 7: turn ends computed; rounds 6 to 1 below)

Round 8 (2026-10-13, eleventh session; the units' commands' lifecycle computed, on `poc5-tb-74`) is in
`docs/poc5/commands.md`.

Round 7 (2026-10-12, tenth session) makes the sim compute the turn controller's status, and with it when a turn ends
(`TurnController.Tick`: `Preparing` → `Acting` → `Ending` → `Ended`). The recorded `turn_end`, an input until round 6,
is now a check. Tags as below: [M] measured, [C] checked against the C#, [I] inferred.

## Result [M]

On **`poc5-tb-68`** (idle, Manual_18; `tb-nearest` and `tb-focus`, record and two replays, all T0-equal to their
record) **every run is valid** with the turn controller's ticks computed: T0-equal, end state 1.0000, nothing reported,
every `TurnController.Tick` (1,484 and 1,046 per run; 8 and 7 turn ends) ported and its status and counters checked at its start, at
`Prepare`, `ContinueActing` and `ContinueWaiting` and at its end, and **every `TurnController.End` the sim's** (the
count of computed ends equals the recorded `turn_end`s; a computed end without one, and one without a computed end,
are reported). `wr-sim/tests/poc5.rs` asserts it; `missing_or_wrong_turn_tick_records_are_reported` damages the
recording 32 ways (an input changed; a tick's start, end, end wait, `IsAbleToAct` or time record dropped; every tick
record or `turn_end` dropped; records of another frame; malformed fields, counts, timings or observer lists) and each is
reported. Older captures replay as before (turn ends recorded). The
regression (`scripts/regress.sh`) is identical to `out-cooldowns2`; every ignored real-data test passes. Superseded: `poc5-tb-64` to
`-67` (earlier versions of the hooks, below). The trace's `begin` record now lists its `observers` (`groups`,
`cooldowns`, `turn_ticks`): a replay requires each declared family's records (an older capture, without the list, replays
with what it has).

## The oracle's new records (`oracle/mods/WrathOracle/TurnTickObs.cs`, observation only) [C]

- `tt` (`TurnController.Tick`'s prefix): the status, the counters (`TimeWaitedForIdleAI`, `TimeWaitedToEndTurn`,
  `FramesWaitedForStuckAI`, `AIForcedTickCount`), `m_WaitForGetUp`, `UnitCanGetUpOnCommand`, the rider's
  `Commands.Empty` and (for an acting turn without commands: the forced AI tick's gate) `IsDirectlyControllable`, the
  frame's time (`GameDeltaTime`, `DeltaTime`, the player and debug time scales, `TimeScaleAverage`);
- `tt_sc` (the tick's `StatusChange` section, at its `ProfileScope.New`): `Commands.Empty`, `HasRunningCommands`, a
  launched projectile (scanned passively by the launcher reference's id). After the forced AI tick or a `Prepare`, which
  run before it in the tick: the rider's commands here are the AI's choice;
- `tt_able` (in the status change, until its `EndWaiting` section): each `IsAbleToAct` the game evaluates there, from a
  postfix (the method sets `Prone.Active` and restores it, so the hooks never call it);
- `tt_prep` (`Prepare`'s postfix, from `StartTurn` or from the tick once the camera's scroll ends): the status it set,
  `IsDirectlyControllable`, `Prone.Active`;
- `tt_ca` (`ContinueActing`): its unit's running commands, get-up animation, panic, `HasMotionThisTick`, the movement
  agent's `WantsToMove` and its uninterruptable-command flag, `UnitCanGetUpOnCommand`, the surprise flag, the time; and
  `IsAbleToAct`, `IsDirectlyControllable` and `HasExtraAction` as the game evaluated them in the call (postfixes scoped
  to it; null where it didn't); its result and the counters around it;
- `tt_cw` (`ContinueWaiting`): the rider's running commands and get-up animation, the time, its result, the counters;
- `tt_end` (the tick's postfix): the status, counters and `m_WaitForGetUp` after it.

## Ported (`combat/turn_tick.rs`) [C]

- The tick's status change and end wait: the forced AI tick's count (`AIForcedTickCount`, a stuck AI's `ToEnd` after
  50), a preparing turn acting (`IsAllActed`/`IsActed` from the sim's cooldowns and movement stats), an acting turn
  asking `ContinueActing` once nothing runs, no projectile flies and the rider isn't getting up, `ToEnd`,
  `ContinueWaiting` and `End` (its writes, `Ended`; `CombatController.Tick` then chooses the next unit).
- `ContinueActing`: panic with the five-foot step used, `IsInCombat`, `GetRemainingTime` (the sim's), `HasExtraAction`
  (recorded: the magus' state isn't modelled), the AI's stuck frames and idle wait (`m_IdleWaitTimeBaseValue /
  m_AnimationTimeDevider * PlayerTimeScale * DebugTimeScale`, evaluated wide and stored once, as Mono does),
  `CheckAndClearWantsToMoveInTurnBased` in turn-based combat. `ContinueWaiting`'s wait (`TimeToWaitForEndingTurn`).
- `Prepare`'s status (`Preparing` for a controllable rider, else `Acting`; `m_WaitForGetUp` from the sim's prone
  state), `Start`'s scroll (`Scrolling`, an input: the camera setting and the rider's visibility), `ForceToEnd`'s
  `ToEnd`, `m_ActingInSurpriseRound` from the unit's `TBUnitInfo` as its turn starts.
- Checks: the frame must tick the turn exactly when `CombatController.Tick` would (default mode, in combat, not waiting
  for the UI, past its units-changed check), every tick must be complete (`tt_end`) by the end of
  `CombatController.Tick`, the frame and the recording; `HandleUnitCommandDidEnd`'s recorded status and the observed
  turn status at each frame's prefix equal the sim's. An input the sim reads that the record doesn't have is reported.
- The replay's clock takes the time record's `DeltaTime` and time scales as well as `GameDeltaTime`.

Inputs that remain recorded: when commands start, act and end (animation), the movement agent (deltas, motion,
`WantsToMove`), the movement limit, the end-turn button, the camera's scroll, the AI's choices, `IsAbleToAct` and
`IsDirectlyControllable` (animation state, the party), `HasExtraAction` (the magus), a projectile in flight.

**Reviews:** gpt-6.1-sol (high, Fast mode). Round 1 found six, all confirmed against the C# and fixed: the hooks called
`PreparedSpellStrike` (its spellbook getter fills a cache), `IsAbleToAct` and `IsDirectlyControllable` where the game
may not, and `HasLaunchedProjectile` (it resolves launcher references); the port was detected from `tt` alone (dropping
every `tt` fell back to recorded turn ends); a tick without its `tt_end` went unnoticed; the idle wait was rounded at
each step (Sol checked Mono: wide, then once at the store; it changes the end frame at non-unit time scales); a
`tt_ca`'s unit wasn't checked. Re-recorded as `poc5-tb-65`. Round 2: the projectile read after a preparing turn starts
acting in the same status change (not exercised by the captures); re-recorded as `poc5-tb-66`. Round 3: a capture
that lost every tick record fell back to recorded turn ends (now the `begin` record declares its observers), a negative
`running` count; re-recorded as `-67`. Round 4: `IsAbleToAct` called by the hook (now `tt_able`, from the game's own
calls), a malformed observer list, unchecked mandatory fields; re-recorded as `-68`. Rounds 5 to 8, damaged-capture
paths: a time record without its deltas, missing, or of another frame; tick records of another frame; non-finite
timings. Round 9: nothing substantive. Gaps Sol names: the hooks' non-interference beyond the C# read, and captures
with mounts, a delay, a prone rider, a stuck AI, a magus' extra action, a surprise round, non-unit time scales,
`Prepare` without a scroll.

## Next steps (round 7)

1. Commands (`UnitCommandController`, `UnitCommand`'s lifecycle: start, act, end, `IsRunning`, the turn's running
   commands): they decide most of the tick's remaining inputs. Survey and slices: `docs/poc5/commands.md`. Then the AI
   (`AiBrainController`).
2. More fights on the current mod (a critical, a spell, iterative attacks, an ally dying, a loot drop, a five-foot
   step, a full attack, a delay), recorded with the tick and cooldown hooks.
3. Gaps no recording exercises: mounts, a delay (`Delayed`), a prone rider (`m_WaitForGetUp`, `UnitCanGetUpOnCommand`),
   a stuck AI (`FramesWaitedForStuckAI`, `AIForcedTickCount` past 50), a magus' extra action, non-unit time scales,
   the surprise round's `ContinueActing`.

---

# POC-5 combat: round 6 (units' cooldowns computed)

Round 6 (2026-10-12, ninth session) makes the sim compute units' cooldowns, and with them when a turn starts
(`CombatController.TickTime`: the next unit's `GetTimeToNextTurn()` has run out). The recorded `cooldowns` rows,
inputs until round 5, are now checks. Tags as below: [M] measured, [C] checked against the C#, [I] inferred.

## Result [M]

On **`poc5-tb-63`** (idle, Manual_18; `tb-nearest` and `tb-focus`, record and two replays, all T0-equal to their
record) **every run is valid** with the cooldowns computed: T0-equal (478 and 408 records), end state 1.0000, nothing
reported, **no cooldown row taken from the recording**, 11,000–12,000 unit-frames checked against the prepare
controller's prefix rows, and every recorded call's row checked before and after it. `wr-sim/tests/poc5.rs` asserts
it; `a_changed_movement_delta_is_reported` and `missing_or_wrong_cooldown_records_are_reported` change the recording
(a doubled movement delta, a dropped prefix, a missing after row, a wrong before row) and each is reported. Older
captures replay as before (the cooldowns recorded). The regression (`scripts/regress.sh`) is identical to
`out-origins`; every ignored real-data test passes. Superseded: `poc5-tb-61` (no per-frame `cd_prefix`), `-62` (its
hooks called two getters with side effects, below).

## The oracle's new records (`oracle/mods/WrathOracle/CooldownObs.cs`, observation only) [C]

Every writer of `UnitCombatState.Cooldowns` is hooked; each record holds the unit's row before (`b`) and after (`a`)
the call, and what the call reads that the sim doesn't compute yet:
- `cd_spend` (`UnitEntityData.SpendAction`): type, full round, time since the command started, call site (a command
  acting in `UnitCommandController`, or an acting command as the turn is prepared);
- `cd_move` (`TurnController.TickMovement`, from `UnitMoveController`): the agent's delta in and out, force mode,
  `NearTheEnd`, the turn's movement limit state (`CurrentMovementLimit`; `m_ModifyMovementLimit`'s current and
  requested values and the unit's `View.IsMoving()`, since its getter takes the request when nothing moves;
  `m_AIUsedFiveFootStep`, `m_AutoStopAfterFirstMoveAction`), the unit's memoized speed (null when stale: reading
  `CurrentSpeedMps` would compute it), panic; the turn's movement stats before and after (`sb`, `sa`);
- `cd_force_end` (`ForceToEnd`: the end-turn button, an interrupted AI command, a stuck AI), `cd_cmd_end`
  (`HandleUnitCommandDidEnd`: the command's `DoesCommandEmptiesMovement` as the handler evaluated it, captured by a
  postfix scoped to the handler (null when it wasn't evaluated: calling it runs `ShouldUnitApproach`),
  `SimulatingClick`, the turn's status);
- `cd_clear` (`Cooldowns.Clear`, with its caller: `TurnController.Prepare`, the prepare controller's batch, leaving
  combat; the sim checks the batch's before rows, and leaving combat's against a log of its own clears);
- `cd_write`: the writers not ported (getting up, delaying, mounting, a weapon-set change's animation, spell combat,
  `DeactivateImmediatelyIfNoAttacksThisRound`, the attack-of-opportunity counter), every unit whose row changed;
- `cd_prefix` (`GroupObs`): after the prepare controller's prefix rows, every frame (the rows are written as changes);
  a default-mode frame without one is reported.
The rider is read by id (`m_Rider`'s `UniqueId`, never `Rider`), the unit by scanning the state's units.

## Ported (`combat/cooldowns.rs`) [C]

- `UnitCombatCooldownsController` (new phase `Cooldowns`, after the prepare controller's batch): in turn-based combat the
  awake, living units in combat run down only while time passes between turns (`IsPassing`), the initiative first
  (not a surprised unit's); outside it all run down, a unit waiting for its initiative only that. The real-time
  branch's new round (`OnNewRound`, `HandleNewCombatRound`) is reported where the unit has a handler.
- `SpendAction` (both branches), `GetTimeToNextTurn`, `TurnController.End`, `ForceToEnd`, `HandleUnitCommandDidEnd`.
- `TickMovement` (new phase `Move`, `UnitMoveController`) with the turn's `MovementStats`: the budget
  (`GetRemainingMovementTime`, `GetRemainingTime`, `IsMoveActionRestricted`, `UsedStandardAction`, the five-foot
  step's `ShouldRestrict…`/`HasFiveFootStep`), the clamp unless near the path's end, the auto-stop, force mode, the
  meters of a five-foot step and the step's speed check (`speed * 3f <= MetersOfFiveFootStep`) in double (Mono), the
  speed read only on the branches that read it.
- `TurnController.Prepare`'s clear, placed by its recorded call: every turn here starts with the camera scrolling to the
  unit (`TurnStatus.Scrolling`), and `Prepare` runs ~36 ticks later from `TurnController.Tick` (phase `Turn`). Its
  other parts: `HandleNewCombatRound` (reported for `NewRoundTrigger`, `ShieldWall`, `ShifterGrabInitiatorBuff`) and
  `UnitTicksController.TickNextRound` (non-buff facts' `ITickEachRound`: `AddFactContextActions` with an empty
  `NewRound` list and `ImpatienceWatcherTickingResolve` do nothing, the rest is reported). Neither was run before.
- `TickTime`'s decision: a recorded `StartTurn` with the sim's time to the next turn above zero is reported, as is a
  frame without one where the sim's has run out, or a game delta not clamped to it.
- The attack-of-opportunity counter's write (`AttackOfOpportunity = 5.3999996` when an AoO is used with the round's
  count full) is computed; that an AoO is used is the input (the engagement controller isn't ported).

Inputs that remain recorded: when a command acts and ends (animation), the movement agent's deltas (pathing), the
movement limit (the UI's choice, an AI command's five-foot step), the end-turn button, the camera's scroll, the AI.

**Reviews:** gpt-6.1-sol (high, Fast mode). Round 1 found eight, all confirmed against the C# and fixed: two hook reads
with side effects (`ModifyMovementLimitValue` runs `m_ModifyMovementLimit`'s getter, which can write its current value;
`DoesCommandEmptiesMovement` runs `ShouldUnitApproach`), `CantUseStandardActions` is 55 (not 13, `Frightened`), the
five-foot speed check in double and its short-circuit, and four checks that could be skipped silently (the prepare
batch's and leaving combat's before rows, a missing after row, a frame without its prefix). Re-recorded as `poc5-tb-63`.
`scripts/oracle-record.sh` now kills a failed try's own game left running (a startup hang left it waiting forever).
Round 2: three fail-open paths on a damaged trace (cooldown records detected only by their prefixes, an in-combat unit
without a row skipped, inputs defaulted when missing); round 3: a `cd_clear` of an unresolved unit skipped, and the
script's cleanup could kill another wrapper's game; round 4: an owner that isn't a unit, and an ownerless clear's
before row unchecked. All fixed, each with a perturbation case in `missing_or_wrong_cooldown_records_are_reported`.
Round 5: nothing substantive. Gaps Sol names, which no recording exercises: ownerless clears, mounts, five-foot steps,
full attacks, real-time combat, `Prepare` without a scroll, the hooks' non-interference beyond the C# read.

## Next steps (round 6)

1. The turn controller's status (`TurnController.Tick`: `Preparing` → `Acting` → `Ending` → `Ended`, `ContinueActing`,
   `ContinueWaiting`, `IsAllActed`): turn ends computed from the cooldowns, with commands running, motion, projectiles in
   flight and the AI's idle wait as inputs. Then commands, then the AI.
2. More fights on the current mod (a critical, a spell, iterative attacks, an ally dying, a loot drop, a five-foot
   step, a full attack, a delay), recorded with the cooldown hooks.
3. Gaps no recording exercises: mounts, a five-foot step (the speed input), the real-time branch, a unit with a
   new-round handler, `Prepare` inside `StartTurn` (no scroll), the unported cooldown writers.

---

# POC-5 combat: round 5 (presentation roots at their call sites)

Round 5 (2026-10-12, eighth session) makes the first valid POC-5 runs: the presentation roots the replay echoed are
computed where their recorded call site places them. Tags as below: [M] measured, [C] checked against the C#, [I] inferred.

## Result [M]

On **`poc5-tb-60`** (idle, Manual_18; `tb-nearest` and `tb-focus`, record and two replays, all T0-equal to their record)
**every run is valid**: `valid=true`, its T0 stream equal to the oracle's (532 and 721 records), end state 1.0000, nothing
reported, nothing echoed. (`poc5-tb-57` to `-59` were the same on earlier versions of the hooks, whose reads could change
a unit's group, `CanChangeWeaponSet` or caches: superseded.) `wr-sim/tests/poc5.rs` asserts it. Older captures (no call sites) echo as before. The regression
(`scripts/regress.sh`) is identical to `out-poc5r4`; every ignored real-data test passes.

What "valid" means here: every record is computed by ported code, given the recorded inputs. The inputs are still recorded
(which attack the AI and the player choose, when the UI refreshes, the turn controller's turn starts and ends, commands,
cooldowns, group memory and visibility): replacing them with their controllers is the next step (below).

## The oracle's new records (observation only) [C]

- `origin` on every root rule (`Rules.OnPush`, depth 0): the game's frames below the rulebook, innermost first, joined by
  `" < "` (compiler-generated names hold `<` and `|`, never spaces), 24 deep; the oracle's, Harmony's and the rulebook's
  frames left out. `poc5-tb-56` used `<` and 10 frames: superseded.
- `env` for a root `RuleCheckTargetFlatFooted` (`OnTrigger`'s prefix with one rule open): `Target.Memory.Contains(Initiator)`,
  `CombatState.CanActInCombat`, `UnitPartConcealment.Calculate(Target, Initiator)`, the parts of the check that rest on
  vision and combat state the sim doesn't run.
- Both `env` hooks (this one and the attack roll's) read passively, null where they can't (the replay then reports):
  the memory from the unit's cached group (`GroupObs.RemembersPassive`: `UnitEntityData.Group` runs `UpdateGroup`, which can
  create or join a group; null exactly where it would act); `CanActInCombat` through its own branches with the oracle's
  turn-based predicate (`CombatController.IsInTurnBasedCombat()` writes `CanChangeWeaponSet` outside turn-based combat);
  concealment only on `Calculate`'s plain path (blindsight and distance-bound entries read `View.Corpulence`'s size caches,
  weapon-range entries and bad weather the current or first weapon; See Invisibility or True Seeing evaluate condition
  exceptions, which load blueprints); a saddled unit's `CanActInCombat` is unknown (`GetRider()` resolves an entity
  reference). None was null in the captures.

## Where the roots come from [M]

| caller (`Caller`) | call site | phase | per run (nearest / focus) |
|---|---|---|---|
| `ui` | `InitiativeTrackerUnitVM.UpdateData` (the tracker's flat-footed badges), `CursorController` (UniRx `LateUpdate`) | `Post` | 66 / 100 |
| `input` | coroutines: `TurnController.HandlePortraitHover` → `UpdateActionPredictions` (hover, `SimulateClick`), the oracle's driver running a command | `Post` | 23 / 55 |
| `ai` | `AiBrainController.SelectAction`/`TickBrain` in `TurnController.Tick` (`CombatController.Tick`) | `Turn` | 9 / 14 |
| `ai (StartTurn)` | the same from `TurnController.Prepare` in `CombatController.StartTurn` (`TimeController.Tick`), after its buff sweep | `Time` | 4 / 2 |
| `command` | `UnitAttack.OnStart`/`Init` in `UnitCommandController` | `Commands` | 9 / 14 |

A root of the three classes whose call site maps to none of these, or whose span holds anything but its own steps,
handlers, presentation draws and its own `env` (an observation such as a combat join, a mutation, a nested rule, a sim
draw), is reported and skipped. The placement is checked like every other event's: one recorded out of its phase's order
is reported.

## Ported [C]

- `RuleCalculateAttacksCount` (`combat/counts.rs`): `CalculatePenalizedAttacksCount` (BAB, the companion cap),
  `PrepareHandVariables`, both hands, `AddExtraAttacks` (both hands only for a weapon that counts as double, reported:
  two empty hands share the empty-hand weapon without being double); handlers `TwoWeaponFightingAttacks`,
  `BuffExtraAttackWeaponSpecific` (`WeaponRangeType.IsSuitableAttackType`), `BuffExtraAttackForFastBombs` (its context is
  only requested by `FastBombs`' own ability, not ported: nothing). `ForcePrimaryAttackWeapon` and `WeaponFilter` are
  reported when a snapshot sets them.
- `RuleCalculateAttacksCountLimbs`: each limb with a weapon and an item counts one attack. Its `Slots` compare by the slot's
  `ParentFact` (`UnitBody.limb_parents`, set by `AddAdditionalLimb`): the limb weapon is created again at each load with a
  fresh GUID, so its item id differs between the record and its replays [M].
- The projection compares `RuleCalculateAttacksCount.Result` and the limbs' `Slots`/`AttacksCounts` (it compared nothing for
  these before).

**Reviews:** GPT 6 Astra (high), four rounds on `0a6eaf2`..: (1) the memory read could run `UpdateGroup`, a combat join
inside a call-site root went unreported, two empty hands were taken for a double weapon; (2) `CanActInCombat` reached
`IsInTurnBasedCombat()`, concealment's distance and weapon paths, `UpdateGroup`'s exact conditions; (3) a saddled unit's
rider reference, concealment's condition exceptions; (4) nothing substantive. All checked against the C#; the sim's fixes have
tests that fail without them, and each oracle fix was re-recorded (`poc5-tb-58`..`-60`). Gaps Astra names: count tests for
high BAB, two-weapon and natural attacks against data; the observer's unknowns (mounts, concealment exceptions,
blindsight, weather) aren't exercised by any recording.

## Next steps (round 5)

1. PLAN step 2's other half: replace the recorded inputs with their controllers: the turn controller's turn starts and ends,
   commands, cooldowns, then the AI (`AiBrainController`, whose evaluation these attack counts are part of). The UI's
   requests stay inputs (presentation).
2. More fights on the current mod (a critical, a spell, iterative attacks, an ally dying, a loot drop), recorded with call
   sites.
3. Group memory, visibility and line of sight stay recorded inputs until vision is ported.

---

# POC-5 combat: round 4 (the turn port)

Round 4 (2026-10-12, seventh session) ports what kept the runs from being valid: leaving combat, unit groups, the turn
controller's order and the death paths. Tags as below: [M] measured, [C] checked against the C#, [I] inferred.

## Result [M]

On **`poc5-tb-55`** (a new capture, idle, Manual_18, WrathOracle with `GroupObs.cs`; `tb-nearest` and `tb-focus`, record
and two replays, all T0-equal to their record) every run equals its recording's T0 stream, end state 1.0000, and the
**only** report left is the echoed presentation roots (83 and 88). `wr-sim/tests/poc5.rs` asserts exactly that. The
older captures (`poc5-tb-51`..`-53`, and `-54`, recorded before `turn_tick` existed) replay without the turn port, as in
round 3, still T0-equal. The regression (`scripts/regress.sh`) is identical to `out-poc5r4`; every ignored real-data
test passes.

## The oracle's new records (`oracle/mods/WrathOracle/GroupObs.cs`, observation only) [C]

The leave controller and the turn order read state the sim can't compute: group memory follows vision and line of
sight, commands and cooldowns aren't simulated yet, `IsVisibleForPlayer` is fog of war and the view. These are recorded
where they are read, as inputs:
- `group_state` (window start): `UnitGroupsController.Groups` in order (units, combat count, timer, memory), each unit's
  cached `m_Group`;
- `leave_tick` (`UnitCombatLeaveController.Tick`'s prefix, every call): the turn-based gate (`IsPassing`, `HasEnemyInCombat`,
  `HadEnemyAtSomePoint`, current and next unit) and, as changes, each group's count, timer, memory (dictionary keys in
  enumeration order, the cached `UnitsList` and its validity: `Cleanup` doesn't invalidate it) and faction caches;
  `cmds` there: each unit's `Raw` and `Queue` commands (target ids read without resolving references);
- `choose_next` (`ChooseNextUnit`'s prefix): current and next unit, each `TBUnitInfo`'s flags and `IsVisibleForPlayer`;
- `turn_tick` (`CombatController.Tick`'s prefix, every call): the UI guard and `m_IsUnitsChanged`;
- `cooldowns` (`UnitCombatPrepareController.Tick`'s prefix): units' cooldowns, as changes;
- `leave_combat`, `group_new`/`_add`/`_remove`/`_dispose`, `mem_remove`/`mem_clear`.
`IsInTurnBasedCombat()` (it sets `CanChangeWeaponSet`) and `TurnController.Rider` (it resolves a reference) aren't called.
Three GPT 6 Astra reviews of the hooks (side effects, completeness, patch signatures), all findings fixed.

## Ported [C]

- **Unit groups** (`combat/groups.rs`): `UpdateGroup` (a held, in-game unit joins `GetGroup(GroupId)`, else a fake group),
  membership dropped on `IsInGame` changes and disposal, `UnitGroupsController.Tick` (new phase `Groups`), the `IsInCombat`
  counts, `IsEnemy` from the members' factions. The list's order at the window start is the recording's: the game creates
  the area units' groups during the area load, before the party's, which isn't modelled; members, counts and timers are
  checked.
- **`UnitCombatLeaveController`** (phase `Leave`, after `Life`): the turn-based gate, `TickGroups`, `TickGroup`'s timer and
  `MarkedAsLeave`, `ShouldLeaveCombat` with the recorded memory and commands (`IsOffensiveCommand`, `CanAttack`); outside
  turn-based combat, line of sight is reported only where it decides.
- **The turn controller's order** (`combat/turns.rs`): `m_Units` in order, `m_NextUnit`, `ChooseNextUnit` (with the recorded
  visibility), `StartRound`, `SortUnits`, `RemoveUnit`/`AddUnit`, `HasEnemyInCombat` (phases `TurnPrefix`, `Turn`,
  `TurnChoose`), `TickTime`'s passing and `TimeSinceStart`, `StartTurn`'s checks, `HandleUnitRollsInitiative`'s overrides,
  sequences and a late joiner's cooldown, and the prepare controller's batch (roll all, spread cooldowns, handlers by
  result, Mono's introsort). The sim's turn events are logged and matched in order against the recorded `sort`,
  `round_start` and `turn_clear`; its `LeaveCombat` calls against `leave_combat`; every `leave_tick` gate and `turn_tick`
  is compared. Recorded cooldowns replace the sim's where they differ (the cooldown controller isn't ported).
- **`LeaveCombat`**: the group count, `UnitCombatState.Clear` (cooldowns, initiative, prepared), `RemoveAnnoyingBuffs`'s
  setting; commands and the view are T1 stubs.
- **Death listeners**: summon pools, `Player.Stalker`, `InspectUnitsController` (phase `Inspect`; the saved
  `InspectUnitsManager` infos; a knowledge check that would roll is reported). The AI's bother lists are a stub.
- `ContextActionReduceDebilitatingBuffsDuration`; and (a Sonnet subagent) `DropLootToGround`, `UpdateSlotIndex`,
  `CombatStateTrigger` and `EvaluatedUnitCombatTrigger` actions (below).

**Reviews:** GPT 6 Astra (high), five rounds on the sim (`20eaaaa`..): (1) commands established empty from the first
`leave_tick`, `HasOffensiveCommand`'s short-circuit, a mid-fight combat start (turn-based mode switched on) reported; (2)
unknown remembered units reported, faction counts compared as a mapping; (3) a frame's remaining phases run with its own
game mode; (4) fact-context actions in `RunActionInContext`'s scopes (`Buff.Data` for `FactOwner`); (5) nothing
substantive. All checked against the C#; each fix has a test that fails without it. Gaps Astra names, which no recording
exercises yet: late joiners' cooldowns against data, surprise rounds, mounts, a death that drops loot, a trained knowledge
check.

## Next steps (round 4)

1. PLAN step 2: replace the recorded inputs with their controllers (the turn controller's turn starts and ends, commands,
   cooldowns, the AI); then the presentation roots are computed, not echoed, and the runs are valid.
2. More fights on the current mod (a critical, a spell, iterative attacks, an ally dying, a loot drop: the drop's rotation
   draw is untested against data).
3. Group memory, visibility and line of sight stay recorded inputs until vision is ported.

`WR_DEBUG_UNSUPPORTED=1 wr-combat replay …` prints each report under its frame's tick.

---

# POC-5 combat: round 3 (the current oracle's recordings)

Round 3 (2026-10-12, sixth session) points the driven replay at the recordings made on the current
WrathOracle (`8fa7cc6`): `poc5-tb-51`, `-52` (under load) and `-53` (idle), each `tb-nearest` and
`tb-focus` with a record and two replays, on Manual_18. The fights differ between captures (six
fights, 18 runs). Tags as below: [M] measured, [C] checked against the C#, [I] inferred.

## Result [M]

| capture | fight | records compared (oracle = sim) | roots driven | end state |
|---|---|---|---|---|
| `poc5-tb-51` | `tb-nearest` / `tb-focus` | 292 / 256 | 128 / 120 | 1.0000 / 1.0000 |
| `poc5-tb-52` | `tb-nearest` / `tb-focus` | 189 / 307 | 93 / 135 | 1.0000 / 1.0000 |
| `poc5-tb-53` | `tb-nearest` / `tb-focus` | 223 / 243 | 128 / 121 | 1.0000 / 1.0000 |

All 18 runs equal their recording's T0 stream, no RNG failure, nothing skipped, and the end state
equals the oracle's in every unit and field (`wr-sim/tests/poc5.rs` asserts all of it). Manual_18
loads with nothing unsupported. No resolve is unverified any more (each ranged resolve has its own
positions), and the end-state id offset of round 2 is gone (below).

## What changed

**The replay consumes the new oracle records** (all appear together from `poc5-tb-38` on; a
recording without them keeps round 2's fallbacks and says so):
- `turn_start_begin` (`StartTurn`'s prefix): the buff sweep runs there, with the previous
  `TurnStartTime`; `turn_start` (the postfix) sets the new one. `StartTurn`'s early returns are
  followed: not in combat, or not in `CombatController.m_Units`, which is now ported (filled at
  combat start, `AddUnit` on joins, `RemoveUnit` on deaths and leaves; `Clear()` at combat
  start/end; filled from `State.Units`: the states' units without fakes, in the game, not suppressed)
  and checked against every recorded `sort` [C]. A root, input or turn event inside
  `StartTurn` is reported.
- `turn_clear` (`CurrentTurn = null`): placed by its source [C]: right after `TurnController.End`
  (private, only called from `TurnController.Tick` in `CombatController.Tick`) it is the turn
  controller's phase (new `Phase::Turn`, between time and sleeping units); the sim's own
  `RemoveUnit` (a current unit dying in the life controller: the replay runs the frame up to that
  phase and accepts the clear only if the sim made it); `Clear()` with combat start/end. Any other
  source is reported while the frame's buffs controller (the only reader of the current turn) is
  pending. The three-valued "maybe cleared" current unit is gone for these recordings.
- `loot_timer`: `DropLootAndDestroyOnDeactivate`'s timer fires at its record (after `Game.Tick`),
  only for the buff with the recorded id.
- `awake_set`/`awake`: `State.AwakeUnits` at each prefix, which is the previous frame's
  `SleepingUnitsController` snapshot (it rebuilds the list every frame [C]). A unit whose state the
  sim establishes must agree (else reported); the others take the recorded state (counted:
  `Outcome::awake_recorded`, about 30,000 unit-frames per run: units out of combat whose state
  rests on fog of war, `AwakeTimer` and commands). `ShouldBeSleeping` now reads `Sleepless`.
- A ranged resolve's `env` record gives the positions at impact.
- An entity marked for destroy is reported: `EntityDestructionController` (late in the frame) isn't
  run, and it would take a destroyed unit out of `AwakeUnits` before the next prefix.

**New ground in the six fights** (each found by the first difference):
- `Projectile.CalculateMissTarget` draws at a missed shot's launch (`RuleAttackWithWeapon.LaunchProjectile`):
  the deviation `Range(Min/MaxProjectileMissDeviation)` and the side `Random.value`; `MaxRange` is 0
  for a weapon's projectile so the range draw isn't made; `FallsOnMiss` is reported. The miss point
  itself is geometry (T1).
- `AddBackgroundWeaponProficiency`'s attack bonus handler (`IsBackgroundStacks`).
- `HandSlot.MaybeWeapon` with a shield: its weapon component with `ShieldBash`, else no weapon.
- `UnitPartFollowUnit.OnTurnOn`/`OnTurnOff` from the saved `m_LeaderRef` (Anevia follows the main
  character in Manual_18): `Sleepless` and the leader's followers.
- **The tutorial fact takes a unique id** [M]: at combat start the tutorial system attaches a fact
  (`5950` in every run), so every later id was one short in the sim. Round 2's guess that the
  dropped loot's entity caused the gap [I] was wrong. The tutorial system stays unported (UI); the
  replay allocates its id at the record and checks it equals the recorded one.

**Reviews:** GPT 6 Astra (high), five rounds on `6d0f9e0`..: (1) five findings (clear placement, `Sleepless`,
`StartTurn`'s `FindUnitInfo` return, timer identity, a sweep test that passed without the sweep); (2) three (destroyed
units and the awake snapshot, unknown turn units, a phase-blind clear test); (3) two (`m_Units` from the entity pool, a
sweep test that couldn't tell entry from postfix); (4) one (`NotFake`); (5) nothing substantive. All checked against
the C# and real; the tests added for them fail when their fix is reverted. Test gaps Astra left: negative-path unit
tests for the background proficiency bonus and shield bash, the follower's turn-on/off round trip, a sort naming a unit
the sim doesn't have.

## Ported after round 3: loot, slot indices, combat trigger actions [C]

`UnitEntityData.DropLootToGround` (`combat/loot.rs`: `HasLoot`, the new unique id, the rotation draw at
`UnitEntityData|DropLootToGround|#0@237`, the dismember skinning removal, the `DroppedLoot` entity with `Loot` = the unit's
inventory, `m_DroppedLootEntityDataRef`); `ItemEntity.UpdateSlotIndex(force)` for a dead unit's items (`items::update_slot_index`;
`m_InventorySlotIndex`, `m_NotLootable`, `m_SkinningSuccessful` are hydrated, an index of an item added at run time is
unknown and reported); `CombatStateTrigger` and `EvaluatedUnitCombatTrigger` actions (`IFactContextOwner.RunActionInContext`:
`Ctx::Mechanics`; the `FactData` and `UnitData` scopes, the `CombatUnit` evaluator). In all six fights the dead centipedes hold
only natural weapons (not lootable), so no loot entity is spawned, and the slot indices become -1. The combat-start `Tutorial`
fact now comes from the sim's own `ShowNewTutorial` (a trigger's start action); the replay checks its id against the recording.
What is still reported: the dropped-loot prefab's view and parts (when a drop happens). (`ContextActionReduceDebilitatingBuffsDuration`,
in the end-of-combat `CombatEndActions`, was ported after: round 4.) A drop's rotation draw is made by the sim outside any rule; none of the six fights exercises it, so the driven replay's handling of such a draw is untested.

## Where it stops (the runs are not valid yet) [M]

Every fight reports only: the presentation roots echoed (74 to 107), `CombatStateTrigger` end actions,
`EvaluatedUnitCombatTrigger` actions, `DropLootToGround`, `UpdateSlotIndex`, the death listeners,
group membership on `IsInGame`, `LeaveCombat` (commands, view). Four fights also report a sort whose
units differ from the sim's and one initiative cleared: at combat end the party leaves combat
through `UnitCombatLeaveController` (after the life controller), whose `LeaveCombat` clears the
initiative and whose `RemoveUnit` reaches `ChooseNextUnit`; that controller isn't ported.

## Next steps (round 3)
1. `UnitCombatLeaveController` (the group leave timers, `ShouldLeaveCombat`) and `LeaveCombat`'s
   remaining effects, so the combat-end sorts and clears are the sim's.
2. The rest of PLAN's step 2: `DropLootToGround`, the death listeners, group membership,
   `UpdateSlotIndex`, `CombatStateTrigger`/`EvaluatedUnitCombatTrigger` actions; then the turn
   controller itself (`ChooseNextUnit`, `StartRound`, turn status) in place of the recorded `tb`
   events.

---

# POC-5 combat: round 2 (the driven replay on `poc5-tb-29`)

Round 2 (2026-10-12) runs the sim against the certified turn-based recordings `poc5-tb-29`
(Wrath-Mac `234fdef`; `tb-nearest` and `tb-focus`, each record + two replays, on Manual_18).
Tags: [M] measured, [C] checked against the C#, [I] inferred. No game text here.

## Result [M]

`wr-combat replay fixtures/captures/oracle/poc5-tb-29/fixtures/<fight> [--run record|replay-1|replay-2]`
hydrates Manual_18, sets the clocks to the run's start state, fires each root rule of the
recording in order on the recording's draw tape, and diffs the sim's T0 projection (`rule`,
`result`, `draw`, `mut` of damage, XP, fact attach/detach) against the recording's, in order.

| fixture | runs | computed records, recording's | computed records, sim's | equal | roots | echoed roots (unverified) | unverified driven roots |
|---|---|---|---|---|---|---|---|
| `tb-nearest` | record, replay-1, replay-2 | 167 | 167 | **3 of 3** | 85 | 63 roots, 121 records | 1 resolve, 8 records |
| `tb-focus` | record, replay-1, replay-2 | 218 | 218 | **3 of 3** | 105 | 81 roots, 157 records | 1 resolve, 8 records |

"Equal" counts only what the sim computed: the echoed roots (below) are left out of **both**
streams (they were counted as matches in the first version of this table: 296 and 383 records),
and so, since review round 3, is the one ranged resolve per fight whose positions at impact the
recording doesn't establish (the shot at 51DD: its distance-bonus outcome is taken from the
recording, so the resolve is computed but not verified; it was counted as a match before: 175 and
226 records).
All six: no RNG failure (the sim draws exactly the recorded in-rule draws), nothing skipped.
`wr-sim/tests/poc5.rs` runs all six and a control (one d20 changed on the tape: the diff
shows it). The sim's end state against the oracle's `state-end-*.json`: 21 units, **match
rate 0.9997** (HP, conditions, every stat, modifier, resource, spellbook and all facts but one
id: the archer 5198's Impatience buff is `5954` here and `5955` there, the oracle allocated an id
between, see below); the party units 508/508, 560/563, 651/651, 689/689. The `sort` events of the
turn controller (7 per fight): our `UnitsOrderComaprer` on our rolled initiatives, with the recorded
override and sequence of each unit as inputs, orders them as recorded in 7 of 7, and every rolled
initiative equals the recorded one (including the tie at 17 between a companion and a centipede).

**What is computed and what is not (read this before trusting the table).**
- *Computed by the sim, compared:* the combat-start initiative of every unit (`RuleInitiativeRoll`
  with its `AddBuffOnCombatStart` buff and the tie-break d20), every attack (the whole
  `RuleAttackWithWeapon` chain: weapon stats, attack roll with concealment/AC/flat-footed/attack bonus,
  crit threat and confirmation, the ranged resolve, damage with sneak attack and distance bonuses),
  death (`UnitLifeController`), the party's XP, the Helpless condition and its stat penalty, the buff
  cleanup of a dead owner (`BuffCollection.UpdateOnDeadOwner`) and the dismember buff's removal timer.
- *Inputs taken from the recording:* which unit attacks whom and when (the player's and the AI's
  commands; the turn controller is not ported); who joins combat (the `join_combat` observations,
  `UnitCombatJoinController`); the attack roll's environment (`env` records: flanking, who engages
  the target, memory, can-act, concealment, positions; a value the oracle wrote as null is not taken
  as false); the clocks (game and real time per tick, `TurnStartTime` from the `combat_start`/`turn_start`
  records' own game time); the turn controller's turns (`turn_start`/`turn_end`, the per-frame
  `turn_status` gate); the overrides and sequences of `sort`; `DismemberUnitFX.Init` adding the
  dismember buff when `FxSpawnController` spawns the dying unit's queued blood effect (the
  `RuleCanApplyBuff` root of the system `DismemberedUnitBuff` becomes `AddBuff`; any other root
  `AddBuff` is reported, not driven).
- *Computed with a recorded decision, not verified (left out of the comparison):* a ranged resolve
  whose positions at impact the observations don't establish takes its `DistanceDamageBonus`
  outcome from the recording (the handler's `about` call changed the damage bundle or not); the
  resolve's records are left out of both streams (`Outcome::unverified`, `unverified_records`) and
  reported. One per fight (the shot at 51DD).
- *Echoed, not computed, not verified (and left out of the comparison):* the presentation layer's root rules: 46/61
  `RuleCheckTargetFlatFooted` (the UI's and the AI's predictions; they read the target's memory of the
  attacker and its can-act state, which only attack rolls record), 12/15 `RuleCalculateAttacksCount`
  and 5/5 `RuleCalculateAttacksCountLimbs` (nearest/focus). They are counted in `Outcome::echoed` /
  `echoed_records`; a presentation root that holds a sim draw, a mutation or a nested rule is not
  echoed but reported (`echo_rejected`, none here), so an echoed root provably changes no state.
- *Left out of the comparison:* the `Tutorial` fact attached at combat start (the tutorial
  system is UI-driven), and the `tb`, `cmd` and `input` kinds (the turn controller and commands).

## Ported this round (C# followed)

Not copied; each file cites its sources.
- `combat/initiative.rs`: `RuleInitiativeRoll`, `UnitCombatPrepareController.Tick`'s per-unit body
  (cached initiative `m_KnownInitiative`, the tie-break `RuleRollD20`), `HandleUnitRollsInitiative`'s cache.
- `combat/life.rs`: `UnitLifeController` (`ForceTickOnUnit`, `SetLifeState`, `OnUnitDeath`,
  `GiveExperience`), `UnitHelplessController`, `BuffCollection.UpdateOnDeadOwner`, `IsInGame = false`'s
  buff/ability deactivation, the real-time timer of `DropLootAndDestroyOnDeactivate`.
  `xp.rs`: `Experience.Apply` (`BonusExperience`, `Mathf.RoundToInt`).
  `unit_state.rs`: `UnitStatusEffectHelpless` (the `Dexterity.OnChanged` subscription and penalty).
- `combat/components.rs`, component ports: `AddBuffOnCombatStart`, `AttackStatReplacement`,
  `WeaponFocusParametrized`, `WeaponParametersAttackBonus`, `TwoWeaponFightingAttackPenalty`,
  `DistanceAttackBonus`, `PreciseShot`, `WeaponTypeDamageStatReplacement`, `TwoWeaponFightingDamagePenalty`,
  `WeaponEnhancementBonus`, `WeaponDamageMultiplierStatReplacement`, `WeaponMaterial`, `DistanceDamageBonus`,
  `DeathActions`, `EvaluatedUnitDeathTrigger` (their stubs in `components/logic.rs` are gone),
  `DropLootAndDestroyOnDeactivate`, `ImpatienceCalmingPart` (empty handlers).
- `combat/attack.rs`: `EnhancementValue`, the weapon enhancement in `RuleCalculateAttackBonusWithoutTarget`,
  sneak attack (`RuleAttackRoll` and `RulePrepareDamage`, the shared `IsSneakAttackUsed`),
  `LaunchProjectiles` (a ranged attack's resolve waits in `CombatWorld::pending_resolves` for the
  recording's `RuleAttackWithWeaponResolve` root), shoot-into-combat from the `env` engagement list,
  `BaseDamage.AddModifierTargetRelated`, `BaseDamage.Sneak`.
- `combat/replay.rs` (the driver), `combat/project.rs` (`project_records`).
- `wr-combat bp <guid>` names a blueprint; `wr-combat replay` also writes the oracle's and the
  sim's projections under `cache/poc5/<fight>-<run>.{oracle,sim}.t0.jsonl` and the end-state compare.

**Bugs found in earlier code** (all three would have given wrong answers on any other fight):
- `CATEGORY_SHORTBOW`/`LONGBOW` were 6 and 7; the enum's values are 42 and 43 [M: the bow in the fixture is 43].
- `BlueprintBuff` flags (`StayOnDeath`, `IsFromSpell`, `RemoveOnResurrect`) were read as fields
  that don't exist (always false): they are bits of `m_Flags` (`unit_facts::buff_flag`).
- `UnitEntityData.GroupId` defaults to the party group (player faction) or the unit's own id; units
  without a saved group made `IsEnemy` unsupported.

## Where it stops (the run is not valid yet) [M]

**Controller phases (review round 3: placed, not assumed).** Every recorded event is placed in a phase of its frame
(`replay::Phase`, the registration order of `GameModesFactory.Initialize`, which `GameMode.Tick` runs in order):
prefix (the `tick` record, observation rows and gates, written in `GameMode.Tick`'s prefix), `TimeController` (the `time`
record is its postfix; `CombatController.StartTurn` runs inside it, from `TickTime`), `SleepingUnitsController` (the
awake snapshot), `UnitCombatJoinController` (joins; `HandleCombatStart`/`End` through `Player.UpdateIsInCombat`),
`UnitCombatPrepareController` (initiative), `UnitCommandController` (a root attack, only if its initiator runs an attack
command per the `cmd` records), `UnitBuffsController`, `ProjectileController` (resolves), `FxSpawnController`
(`DismemberUnitFX`'s `AddBuff`: its blood effect is queued with `NotImportant` priority, so `Init` runs in the controller's
tick, before the life controller), `UnitLifeController`, `UnitHelplessController`, then after `Game.Tick` (`Runner.Update`):
the UI, coroutines, the oracle's inputs. The controllers run lazily in that order as the events of the frame reach them, and
at its end; **which controllers run follows the tick's game mode** (`registered`: a pause runs only sleeping units and
helplessness, a cutscene no buffs, preparation only the default mode; other modes and mode changes are reported). An
event recorded after a later phase of its frame (or in a mode that doesn't run its controller) is reported, not reordered.
An input runs after its frame's controllers (`tbm:off` no longer reaches back into the frame). `StartTurn`'s sweep runs at
`turn_start` with the new turn installed and the previous `TurnStartTime`, and `TurnStartTime` becomes the record's game
time (the `time` record comes after it); because `StartTurn`'s entry isn't recorded, anything between the frame's `tick`
and `turn_start` that the sim would act on (a driven root, a mutation, a turn event, an input, a join) is reported (none in
either fight: only echoed roots, out-of-rule draws, the AI's commands and observations lie there). The turn-based state is
checked against every `time` record's `turn_based`. The current unit: `turn_start` installs it; `turn_end` leaves it
installed but possibly cleared by `CombatController.Tick` at a point not recorded, so until the next prefix's `turn_status`
gate settles it, a buff due on that unit is reported instead of ticked or skipped (`IsCurrentUnit` is three-valued);
`HandleCombatStart`/`End` (`Clear()`) remove it and empty the initiative cache.
The awake units are a per-frame snapshot in `ShouldBeSleeping`'s order; a finally dead unit is now *unknown* (its
`AwakeTimer`, which `Position` changes, memory additions and the view set, comes first and isn't tracked), and an area with
parts makes every unit unknown (`SuppressEntitiesController`; not the case here). A controller that would change an unknown
unit is reported: `ForceTickOnUnit` (not for a finally dead unit: `ShouldTickOnUnit`), helplessness, and the turn-start
sweep, whose check is now `BuffCollection.Tick`'s own: `UpdateOnDeadOwner` for a newly dead owner first, then a due event
the turn gate lets through (or can't tell). Positions at impact: the observation rows are written at each frame's prefix for
the units awake at the window's first tick, when they change; a resolve keeps the roll's positions only if, for both units,
every prefix from the roll's frame to the impact's and the next one shows them there and not moving while they were
established awake; otherwise (51DD has no rows: asleep at the first tick) the distance bonus's recorded outcome is used and
the resolve is unverified (above). The `DropLootAndDestroyOnDeactivate` timer (UniRx, Unity's `Time.time`, after `Game.Tick`)
fires after the frame's controllers once `Player.RealTime` has passed it, and reports that its frame isn't established.
The echoed presentation roots are *not* controller boundaries (their origin is not established); they are left out of both
streams and reported once each. Resolves match their attack by the recorded rule identity. The end state also compares
`in_game`, `is_dead`, `is_conscious` (`compare::compare_flags`, additive): 21 of 21 each, on all six runs, and every unit's
damage (the test). `BonusExperience`: a loaded unit keeps its saved value, a spawned unit gets the difficulty's; the death's
companion test uses the blueprints' factions.

Both fights report, in the replay phase, only: `EmptyHandWeaponOverride.OnTurnOn` (2, hydration),
`UnitEntityData.DropLootToGround` (3: the dead centipedes hold loot; the original spawns a dropped-loot entity
with a random rotation draw and a unique id; the one-id gap in the end state is this [I]),
`UnitLifeController: ItemEntity.UpdateSlotIndex` (3), `UnitLifeController: controllers listening to a
unit's death` (3: group, summon pools, impatience, turn controller queue), `UnitEntityData.OnIsInGameChanged:
group membership` (3), `UnitEntityData.LeaveCombat` (3: commands, view, turn controller), `CombatStateTrigger` end actions (3) and
`EvaluatedUnitCombatTrigger` actions (1 in nearest; the actions run in the fact's context, not ported), the
`DropLootAndDestroyOnDeactivate` timer's frame (3), the shot at 51DD (positions at impact not established, 1; the resolve
computed with the recorded distance outcome, 1), and the echoed presentation roots' unestablished origin (63/81). The
turn-start sweep no longer reports (it was 41/68): its check for unknown units was "a buff is due", without the turn gate;
with `BuffCollection.Tick`'s own gate none of those units is the current unit or has a due buff cast by it. `tb-nearest`
also reports `DistanceAttackBonus`/`DistanceDamageBonus: range beyond the units' corpulence` (1 each): a shot from 11 m at a
30 ft bonus; `Corpulence` is a unit view prefab field (at least 0.5), so only distances within the range plus 1 m are
decided, and the recording's outcome (no bonus) can't be derived. The load-time kinds are POC-4's (script zones, map
objects, unit groups: another worker's).

**Review round 3 (Astra on b0ab4f0), all six findings checked against the C# and WrathOracle and real:** positions at
impact accepted on missing rows (51DD has none; now positive evidence only, above); the turn-start sweep's entry
(`turn_start` is `StartTurn`'s postfix; now placed and checked, above); buffs and life run in paused and UI frames
(`GameModesFactory` registers neither in `Pause`, and no buffs in `Cutscene`; now per mode); an input between ticks
changing the semantics of the previous frame's deferred controllers (now an input runs after them); the sweep's check for
unknown units ignoring `UpdateOnDeadOwner` (now included); and the sweep test passing without the sweep (now one replay that
asserts the removal comes before the frame's initiative, which fails without the sweep). The audit of the same class found
and fixed: `turn_end` clearing the current unit (the turn stays `CurrentTurn` until the turn controller clears it);
`TurnStartTime` taken from the previous frame's clock (now the `combat_start`/`turn_start` record's); combat joins inferred
at the initiative root (recorded as `join_combat`); `Clear()` at combat start and end not modelled (current turn, initiative
cache); a root attack assumed to be a command's, and any root `AddBuff` assumed to be the view's (now checked: running
attack command; the system `DismemberedUnitBuff` in `FxSpawnController`'s phase, before life, not after it); a null or
missing `env` value taken as false or zero; a finally dead unit assumed asleep (`AwakeTimer`); suppression assumed absent
(now checked: the area's parts); the UniRx timer fired in the buffs phase on `Player.RealTime` (now after the frame,
reported); and the fixture test, which asserted the inferred positions were accepted.

**WrathOracle changes that would remove the remaining reports of this kind** (the coordinator's; not made):
1. A `StartTurn` *prefix* record (`tb` `turn_start_begin` with the unit), so the sweep's entry is recorded and records
   before it in the frame are known to precede it.
2. Positions at impact: an `env`-style record in a `RuleAttackWithWeaponResolve.OnTrigger` prefix (initiator and target
   `Position` under `at`), and `Observe`'s known set taken from every unit in `State.Units` at the first tick (not
   `AwakeUnits`), so a unit that wakes later (51DD) gets rows.
3. The current turn's clearing: a postfix on `CombatController.CurrentTurn`'s setter (or `turn_cleared` when it becomes
   null), so an ended turn's rider is never uncertain.
4. The awake set itself: an `obs` record of the units entering and leaving `State.AwakeUnits` per tick
   (`SetNewAwakeUnits`), which would replace the derived snapshot and its unknown units.
5. The UniRx timer: a record when `DropLootAndDestroyOnDeactivate`'s callback fires (its closure's method), so its frame is
   an input.

## Next steps (round 2)
1. Make the run valid: corpulence from the prefabs (`wr-import`), `DropLootToGround` (entity,
   id, draw), `EmptyHandWeaponOverride`, the death listeners (`UnitGroupsController`, `SummonPoolsManager`,
   `UnitImpatienceController`), `IsInGame` group membership, `UpdateSlotIndex`.
2. Replace the inputs by the controllers they come from: the turn controller (`TBUnitInfo` bookkeeping
   in `HandleUnitRollsInitiative`, turn start/end, `sort`), commands (`UnitAttack`, `UnitMoveTo`), the AI
   (target choice: nearest, focus), engagement, flanking, memory and concealment (the `env` record's
   fields), so that the `tb`/`cmd`/`input` kinds can be compared and `RuleCheckTargetFlatFooted` and
   `RuleCalculateAttacksCount` are computed, not echoed.
3. More fights (a miss with a projectile's `CalculateMissTarget`, a critical, a spell, a full attack
   with iterative attacks, an ally dying: companion death and `DeathDoor`), recorded on the clean build.
4. Then `wr-combat` plays instead of replays: our own RNG, the AI and the commands.

Effort: one session (~6 h wall): the driver and the first divergences in the first hour, the rest
was porting what each divergence showed, in the order the stream reached it.

---

# POC-5 combat: handoff (round 1, wrapped up mid-flight)

Written on branch `worktree-agent-a1033899f1df3bad3` (based on `poc` 4b7cbef), merged into
`poc` as 64d2855; review fixes below. Stopped early for the usage window. Tags: [M] measured, [C] checked against code, [I] inferred.

## Done

**Hydration of Manual_18 (goal 1).** `wr-combat hydrate Manual_18` then `wr-combat compare`
against `fixtures/captures/oracle/manual18-dump-1/state-manual18-after-load.json`.
- [M] 21/21 units present, all 15 placeholders become units (`spawn/`: construction,
  automatic class levels with `AddClassLevels`, selections, skill points, fact processors).
- [M] Field match 0.9086 overall; stats base/permanent 798/798, modified 797/798 (one
  lizard Perception: `ContextRankConfig` not ported); party units 100% (PC 505/505, Lann,
  Anevia, Seelah, Camellia); Wenduag 636/640.
- [M] Spawned units' fact sequences match the oracle structurally (same blueprints, order,
  id hole after `ReapplyFeaturesOnLevelUp`), but ids are offset: ours start at 0x5866, the
  oracle's at 0x5868 (two unknown `GetNewUniqueId` calls first), and late toggle buffs are
  offset by 5 more (Wenduag's three item abilities 593E–5940 are recreated by the game, not
  by us). Most remaining per-unit mismatches are these id-keyed comparisons.
- Still unsupported at load (coverage report, fail-closed): `m_Roaming` of a placeholder,
  `AddFacts` difficulty requirements, `ContextRankConfig`, `EmptyHandWeaponOverride`
  (2 units), plus POC-4's etude brackets and companion components.
- Added this round: unit conditions/immunities/status effects (`unit_state.rs`), immunity,
  context-action and visual components, item ability reapply, weapon slots (empty-hand
  weapon, additional limbs, grips; `AddAdditionalLimb` creates the PC's bite limb at load).

**Turn-based recordings (goal 2), exploration only.**
- WrathOracle (uncommitted in Wrath-Mac, see below) gained steps `tbm:on|off`,
  `tbwaitcombat`, `tbfight:POLICY`, `tbend`, `idtrace:on|write`, and TB-window-only records
  `tb`, `cmd`, `ai`, `input` (`tbattack`/`tbend`/`tbfight_done`) and, newest, `env` (per
  attack roll: target flanked/engaged-by, memory, can-act, concealment both ways;
  positions under `at`). Every TB record is gated on a `tbm` step in the window, so the
  real-time certification fixtures are unchanged ([M] cert-10 ran with these hooks
  installed: 0 failed records, no install failures).
- [M] `poc5-explore-1` (spec `tools/oracle-fixtures/poc5-explore.spec.json`): fixtures
  `tb-nearest` and `tb-focus`, each record + replay-1. **Made with ToyBox loaded (6/6 mods),
  before the switch to `builds/arm64-oracle`: exploration only, re-record.** `tb-nearest`
  is a complete TB fight: PC, Camellia, Seelah, Anevia vs three young centipedes, two
  rounds, 6 weapon attacks (1 crit threat unconfirmed), 3 damage, 3 deaths, 9 XP muts,
  combat ends. Problems: `rng_unhooked_steps` 5 (record) — unidentified TB-path RNG calls,
  must be found before certification; replay-1 has `time_tape_exhausted` (390) and
  tb-focus replay diverged (7 mismatches). Also 61 root `RuleCheckTargetFlatFooted` pushes
  outside attacks (UI/AI prediction, source not yet identified) and one root
  `RuleAttackWithWeaponResolve` (Anevia's ranged projectile).
- `ids-load.json` (IdTrace of every unique-id allocation during load, with stacks) is in
  `poc5-explore-1/`; not analysed yet — it should identify the two unknown allocations.
- The `env` record version of the mod is compiled but **not installed** (the game was busy).

**Headless TB combat (goal 3): rule layer written, not exercised.**
- `combat/oracle.rs` (trace reader, per-site replay tape), `combat/project.rs` (T0
  projection: rule/result/draw/mut/tb/cmd/input; results seeded from the pushed snapshot),
  `combat/fire.rs` (rule firing that writes the sim's side of the T0 stream),
  `combat/order.rs` (`UnitsOrderComaprer`), `combat/rules.rs` (`RuleCanApplyBuff`).
  [M] cert-9 attack record vs replay-1 projections are equal (418 records).
- New and **unexercised**: `combat/attack.rs` (the whole weapon attack chain: weapon
  stats/size, attack roll, concealment, AC, flat-footed, attack bonus, resolve, deal/
  prepare/calculate damage, miss reason; unmodelled branches report unsupported),
  `combat/weapon.rs` (dice, scale table, weapon properties, slots, grips, limbs),
  `combat/units.rs` (relations, helplessness, difficulty settings, driven `AttackEnv`).
  `attack_from_snapshot` builds an attack from an oracle `rule` snapshot for a driven
  replay. Not yet ported: initiative/`UnitCombatPrepareController`, TB turn controller,
  commands/AI, death/XP/Helpless, rule subscribers (`AttackStatReplacement`,
  `WeaponFocusParametrized`, TWF components, `ActivatableAbility` fact handlers,
  `AddBuffOnCombatStart`), projectiles.

## Sol reports (gpt-6.1-sol, copied to `cache/poc5/sol/`, contain decompiled quotes: never commit)
- `spawn-report.md`: placeholder `BecomeUnit` order, automatic level-up, id allocation
  [C, used for the spawn port].
- `attack-report.md`: the melee attack chain with arithmetic, subscriber order (G→T→I per
  scope), death/XP (XP staged truncation 25·.75→18, (int)(18f·.25f)=4), initiative,
  reconciled against cert-9's attack fixture [C].
- `tb-report.md`: TB controller, turn status machine, budgets, AI selection [C, not ported].

## Merge and review (2026-10-11, coordinator)

**Merged into `poc`** (64d2855) with POC-4's round: one conditions implementation
(`unit_state.rs`, with the C#'s events and disposing guards); placeholders' units join the
area at `EntityCreator.Tick` inside `area_activated`; superseded ports removed and duplicate
registration made a panic; every `thread_local!` moved into `World`/`Unit` (two hydrations in
one process differed); `DisableMountRiding` ported; pets' masters hydrated. Every oracle
comparison equal or better than on either branch.

**Astra (high), round 1:** 17 findings on the `wr-sim` code, 5 on WrathOracle `ffcb670`; all
checked against the C# and real.
- WrathOracle (`38059cd` in Wrath-Mac): the turn-based setting is kept until a restore
  succeeds; the `env` record and `tbwaitcombat` no longer call the side-effecting
  `CombatController.IsInTurnBasedCombat()` (`target_flanked` is null outside turn-based
  combat); the portrait hover is released in `finally`; a command `CanRunCommand` would drop
  ends the turn; ID tracing stops on every cleanup. Not yet run in a game.
- `wr-sim` (three Sonnet fixes, merged as a1935a1): `RuleCalculateAC`'s temporary bonus
  scope (`stats::ops::open_temporary_scope`); `IsAnimating` in helplessness; `AttackEnv`
  explicit or taken from the driven queue, else reported; Mono double intermediates in damage
  and weapon stats; prone from posture (`IsGetUp` is a new `AttackEnv` field the oracle
  doesn't record yet); attack factions per unit (hydrated, blueprint overrides at spawn);
  `ForceSecondary`; the projection folds `about`/`trig`/`h_end` changes; rank changes inside
  the reapply scope; the fact processors' gain/loss/rank/buff events; attach order for
  default-on abilities; limb and empty-hand weapons in lifecycle passes; spawned alignment;
  `UnitPartSpellResistance` immunity entries; placeholder stealth, summon pools and
  `Unit.Wake` fail closed. Oracle comparisons unchanged; Manual_18 reports two more kinds
  (`SummonPools.Register`, a Monk feature's `IUnitBuffHandler`).
- **Round 2** (on 64d2855..a1935a1): 15 of 17 resolved; two partly, both fixed: the
  attack environment is now taken as `RuleAttackRoll.OnTrigger` starts (where WrathOracle
  records it, after about-to-trigger subscribers whose nested attacks take theirs first), an
  explicit one kept as an override; `IsGetUp` is optional and reported only when the rule
  reads it (posture not prone), and WrathOracle's `env` record now carries it
  (`target_is_get_up`, Wrath-Mac `b26f3a1`, read passively).

**Recording note:** oracle game runs hang at startup while the Mac's screen is locked
(poc4-seelah-2 stalled; stopped and removed). Record only with the session unlocked.

## Wrath-Mac changes
Committed on Wrath-Mac `poc`: `ffcb670` (turn-based steps, `IdTrace`, fixture specs),
`38059cd` (review fixes) and `b26f3a1` (`IsGetUp` in `env`). Installed in `builds/arm64/Mods` (shared with `arm64-oracle`), but
no turn-based run has used them yet.

## Next steps
1. With the screen unlocked, re-run
   `poc5-explore` on the clean build (`LOADED 4/4 MODS`), find the 5 unhooked RNG steps and
   the root `RuleCheckTargetFlatFooted` source; tell the coordinator if a TB site needs the
   audited-T1 list. Fix the TB replay (time tape exhaustion, tb-focus divergence).
2. Record ≥3 fights (nearest, focus order, a perturbed d20 that flips a hit) with
   self-replays certified by `oracle-trace-diff --t0` and `oracle-state-diff`; final specs
   `poc5-*.spec.json`.
3. (Done in round 2, above.) Driven replay in `wr-combat`: hydrate, load trace + tape, fire each root rule from its
   snapshot (`attack_from_snapshot`, `env` records as `AttackEnv`), run death/XP after
   damage, diff the T0 projection (rule/result/draw/mut). Port the subscribers listed above,
   initiative and the life controller first; then the TB turn controller and AI to replace
   the driven inputs.
4. Analyse `ids-load.json` to close the fact-id offsets. (Done for Manual_18, 13 and 19; Manual_10 keeps a -5 offset before its first spawned unit.)

Effort: one long session (~3 h wall); most went into placeholder spawning and the hydration
match, then the TB oracle steps; the attack port was written in the last hour.
