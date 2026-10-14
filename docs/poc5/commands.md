# POC-5 commands: survey and plan (round 8: slice A done 2026-10-13, eleventh session; round 9: slice B done, twelfth session; slice C done, thirteenth session; slice D done, fourteenth session; slice E done, fifteenth session; slices F and G done, sixteenth session)

Round 7 left the units' commands as the main recorded input of the turn controller: whether a rider's commands are
empty or running, the turn's running commands (`HasRunningCommands`), and when a command acts (`SpendAction`'s call from
`UnitCommandController`). This note surveys the C# and plans the port in slices. Tags: [M] measured, [C] checked against
the C#, [I] inferred.

## How commands run [C]

- `UnitCommands` (per unit): slots `m_Commands[CommandType]` (Free 0, Standard 1, Swift 2, Move 3), a queue, and
  `PreviousCommand` (a running command parked while the game is paused). `Run(cmd)` (not from the queue): checks
  (`CantUseStandardActions`, conscious), `Init(owner)`, clears the queue, merges into a running `PreviousCommand`
  (`TryMergeInto`), else special handlers, interrupts the slot's command and its pair (Move and Standard pair up),
  interrupts the other unstarted commands when this one isn't close enough, `OnRun` (`DidRun`, `ApproachStartTime`,
  `StartInTbm`: the current turn's `AddRunningCommand` for its unit or a reaction), `HandleUnitRunCommand`.
  `RemoveFinishedAndUpdateQueue` clears finished slots and runs the queue's head once its slot and pair are free.
- `UnitCommandController.TickOnUnit`: interrupt `PreviousCommand`; skip a unit without a view, held by its rigidbody or
  getting up; tick each slot's command (`canMoveUnit`: no earlier command moving the unit, and not held/stopped/ranged
  rider unless it isn't the AI's); `RemoveFinishedAndUpdateQueue`; stop the view when empty; on an exception,
  `InterruptAll`.
- `TickCommandTurnBased`: in turn-based combat a command ticks only while the UI isn't waited for, and (an attack of
  opportunity always) for the current unit or one of the turn's running commands while the turn is `Acting` or
  `Ending` (or, between turns, a unit not in combat). An unstarted command that is close enough but can't act (`CanAct`,
  nausea, `CanActInCombat`, the cooldown for its type) is force-finished with `Success` and the queue cleared; one that
  should approach but has stopped moving after its approach time, likewise.
- `TickCommand`: interrupt when it should be (`ShouldBeInterrupted`, the target gone, untargetable or hidden); an
  unstarted one approaches (`TickApproaching`: movement, pathing) then starts when `ShouldStartCommand` (hands busy with
  an equipment animation, `AwaitMovementFinish`, the mount, `CanStart`, close enough, `CanAct`, the equipment update,
  the cooldown, nausea); a running one turns to its target, ticks, spends its action when it first acts, and is
  interrupted when it can't act or cast.
- `UnitCommand.Tick`: `TimeSinceStart += DeltaTime`, `OnTick` (per class), then acts once its animation acts (or it has
  none, is one frame, skips animation, or pretends to act after `PretendActTime`): the AI's `UseAction`, `OnAction`
  (per class: the result), `IsActed`, `HandleUnitCommandDidAct`; it ends (`OnEnded`) once it has a result and its
  animation is finished; after 9 s it's interrupted (and the turn's `OnInterruptCommand`).
- `OnEnded`: `IsFinished`, the animation released, the view's weapon handling, `HandleUnitCommandDidEnd` (the turn
  controller's), `EndInTbm` (`RemoveRunningCommand`).

## What the fights use [M]

`poc5-tb-68`: `UnitAttack` (the player's and the AI's, 14 to 17 run per fight, 4 or 5 started), `UnitAttackOfOpportunity`
(1), `UnitMoveTo` (5198's follow moves outside its turns: about 250 run and ended, none started, in `tb-nearest`; a few
others). Most runs never start: they're replaced or interrupted (the AI re-decides, the follow logic re-issues).

## Inputs and what decides them [I]

| what | decided by | first |
|---|---|---|
| a command is run (class, type, target, AI action) | the AI, the player (the oracle's driver), follow, predictions | input |
| close enough, approaching, `IsReallyMoving`, path | positions, pathing | input |
| when the animation acts and finishes; hands busy | animation | input |
| `ShouldBeInterrupted`, target state | rules state (dead, untargetable), stealth | sim where ported |
| the slots, queue, `IsStarted`/`IsActed`/`IsFinished`/`Result`, `TimeSinceStart`, the turn's running commands | the lifecycle | **sim (slice A)** |
| `UnitAttack`'s attacks (count, iteration delay, index, target update) | the attack sequence | sim (slice B) |

## Slices

- **A (done 2026-10-13: "Slice A: result" below): the lifecycle.** Port `UnitCommands` (slots, pairs, queue, `Run`'s bookkeeping,
  `RemoveFinishedAndUpdateQueue`, `InterruptAll`), `UnitCommandController`'s order and gates, `TickCommandTurnBased`,
  `ShouldStartCommand`'s sim-computable parts, `UnitCommand`'s state (`Start`, `Tick`, act, `OnEnded`, `Interrupt`,
  `ForceFinish`, `TimeSinceStart`) and the turn's running commands. New WrathOracle records give each run command an
  identity (the oracle can number commands at `Init`) and, at each decision, the inputs above (close enough, moving,
  hands busy, the animation's act/finish). Then the tick's `Commands.Empty`, `IsRunning` and `HasRunningCommands`
  become checks, as does `cd_spend`'s call from `UnitCommandController`. Follow and prediction commands outside combat
  count too: they change the slots the tick reads.
- **B (next): `UnitAttack`.** Its attack list (`InitAttacks`: full attack or single, already counted by
  `RuleCalculateAttacksCount`), the iteration delays (`MaxDelayBetweenIterativeAttacks`), `OnAction`'s attack rules
  (today driven as recorded roots: they would come from the command), the target update. Since slice A its
  `Init`, `OnStart`, `OnTick` and `OnAction` are class code whose effect is recorded (`uc_fx`); slice B makes them the
  sim's. First reading of the C# (`Kingmaker.UnitLogic.Commands/UnitAttack.cs`, not yet a plan): `Init` sets
  `m_AnimationsDuration = 6`, runs `InitAttacks` (full or single: `IsFullAttackRestricted`, staggered, charge/pounce;
  `CreateFullAttack` triggers `RuleCalculateAttacksCount` and `...Limbs`, `EnumerateAttacks` interleaves primary and
  secondary hand, penalized attacks `5*j` or `4*j` when flawless; `CreateSingleAttack`), `ConfigureAnimations` (animation
  handles, sequences reordered on a `UnityEngine.Random` draw, `SpellSpeedScale`: presentation, but it decides the
  handles' presence, which the lifecycle reads), the approach radius (corpulences and the weapon's range).
  `OnStart`: `InitAttacks` again, `TryStartNextAttack(first)`. `OnTick`: the iteration delay
  (`ModifyDelay(m_AnimationsDuration / count) * index`, capped by `MaxDelayBetweenIterativeAttacks` in turn-based
  combat; bows and sequential attacks use `TimeSinceStart`), `TryStartNextAttack`, `ClearAnimation`, `UpdateTarget`
  (retargets to the nearest conscious enemy in five-foot range from the group's memory when the target died or left
  range). `OnAction`: `MakeSaveCheckForAttack`, `ProvideNextAttack` (`TriggerAttackRule`: a `RuleAttackWithWeapon` with
  `AttackNumber`, `AttacksCount`, `IsFirstAttack`, `IsFullAttack`, `WeaponSlot`), skipping attacks without an
  animation handle, `Success` once every attack is made. The animation handles (presence, `IsActed`, `IsFinished`,
  durations) stay inputs, so slice B needs new records for them where `UnitAttack` reads them; the attack rules would
  then be fired from the command instead of from their recorded roots.
- **C (done 2026-10-13: "Slice C: result" below): approach.** `TickApproaching`, `UnitMoveTo`,
  `UnitAttackOfOpportunity`; the movement agent stays an input (its calls checked) until the nav port.
- **D (done 2026-10-13: "Slice D: result" below): the AI** (`AiBrainController`), whose choices are the runs above.
- **E (done 2026-10-14: "Slice E: result" below): the engagement controller** (`UnitCombatEngagementController`,
  `Engage`/`Disengage`, attacks of opportunity and their count).
- **F (done 2026-10-14: "Slice F: result" below): units' targets** (`LastTarget`, `ManualTarget`:
  `UnitCommands.UpdateCombatTarget` and the other writers).
- **G (done 2026-10-14: "Slice G: result" below): the impatience controller** (`UnitImpatienceController`: what
  the AI reads as `ipt` and `disliked`).

Before slice A's first review, walk the fail-closed checklist (every field, frame id, finite values, declared
observers, completeness at each boundary; hooks never call side-effecting game methods).

## Slice A: design (2026-10-13, eleventh session)

The sim runs the base lifecycle (`UnitCommands`, `UnitCommandController`, `UnitCommand`'s base methods, the turn's
running commands). Everything else that the lifecycle reads or calls is an input, recorded where the game evaluates it.
WrathOracle's `CommandObs.cs` (observer `commands`) writes `uc_*` records:

- **Identity.** Every command gets an id at first sight (`uc_run`, `uc_queue`, the window's snapshot), and every
  `UnitCommands` object too (a unit's own, and the turn controller's prediction sets, which `UnitCommands.Temporary`
  swaps in with `ReplaceCommands`). Records name commands and sets by id.
- **Entries.** The lifecycle code runs from a few entry points, each recorded with its caller (for the replay to place
  it in its phase): `UnitCommands.Run` (the private overload: the AI's, the player's, follow's, the predictions' and
  the queue's runs; `uc_run`, with the command's description, and `uc_run_end`), `AddToQueue*` (`uc_queue`),
  `UnitCommandController.TickOnUnit` (`uc_tick`, for a unit with a command, a queue or a parked `PreviousCommand`, with
  its gate: view, rigidbody, get-up), `TickCommand` (`uc_tc`: its `canMoveUnit` argument), the external mutators
  (`InterruptAll`, `InterruptAiCommands`, `InterruptMove`, `ReplaceCommands`...: `uc_x`) and the controller's own
  `Tick` (`uc_ctl`, `uc_ctl_end`).
- **Leaves** (`uc_v`): each environmental read the base lifecycle makes, with its result: `IsUnitCloseEnough`,
  `CannotStop`, the view's `IsMoving`, `IsReallyMoving`, hands busy, the equipment update, `IsDirectlyControllable`, the
  view's `IsGetUp`, `ShouldBeProne`, the target's `IsInState`, untargetable, stealth and enmity, and the class's
  answers (`ShouldBeInterrupted`, `IsInterruptible`, `CanStart`, `TryMergeInto`, `AwaitMovementFinish`,
  `DontWaitForHands`, `IsOneFrameCommand`, `CanMoveAfterStart`, `ShouldTurnToTarget`, `NeedEquipWeapons`), the custom
  run handlers and a mounted target. A read is a leaf when every frame between it and an entry is base lifecycle code
  (a stack walk): the same getter called from class code (`UnitAttack.TryMergeInto`'s own `IsUnitCloseEnough`), from
  an event handler or from `TickApproaching` isn't recorded, and the sim doesn't ask for it.
- **Class code** (`uc_fx`): `OnStart`, `OnTick`, `OnAction` (its result), `TickApproaching` and the overridden
  `OnRun`/`Init`, called by the lifecycle: the base state they leave (animation handle, its act/finish flags,
  `HasAnimation`, `PretendActTime`, `m_IsAnimationActHandled`, the type, the target, the approach state, an end).
  Ends inside them come as `uc_end` records first (inputs); the turn's running list changes as `uc_rc` records.
- **Ends** (`uc_end`, `OnEnded`): from base code they're the sim's (checks), from class code or outside the lifecycle
  (the AI, the turn's end) inputs, placed by their caller.
- **State** (`uc_state`, `uc_cs`): every set's slots, queue and parked command, every command's state, and the turn's
  running commands, as changes, at the window's start and at `uc_ctl` and `uc_ctl_end`. The window's first rows are
  the sim's initial state (the save doesn't hold commands); later rows are checks (a difference is reported and the
  recorded state taken).

The replay pulls: an entry record starts the ported code, which takes the following records in order (the leaves and
class effects it asks for, processing everything between, such as the attack rules `OnAction` triggers, as usual).
A leaf the sim asks for that isn't the next one recorded, or a recorded one it didn't ask for, is reported, and the
rest of that entry's records are skipped up to its end record. Then `cmds`, the tick's `r_empty`, `running`, `run` and
`r_run`, and `cd_spend` from the controller are the sim's (checks), and a root attack's running command is the sim's.

## Slice A: result (2026-10-13, eleventh session) [M]

On **`poc5-tb-74`** (idle, Manual_18; `tb-nearest` and `tb-focus`, record and two replays, the replays T0-equal and
tick-exact to their record under `oracle-trace-diff --t0`) **every run is valid with the units' commands computed**:
nothing reported, end state 1.0000, and the sim's commands equal the recording's at every check (rows taken: 0).
`tb-nearest`: 4,403 unit-ticks of `UnitCommandController`, 23 runs, 18 ends computed (4 taken as inputs: the movement
agent's interrupts and class code), 4 `SpendAction` calls computed, 3,193 turn controller reads checked
(`Commands.Empty`, `HasRunningCommands`, `IsRunning()`), 5,228 state checks; `tb-focus`: 2,895 unit-ticks, 28 runs, 18
ends computed (9 taken), 6 `SpendAction`. (`poc5-tb-73`, a version of the observer before review round 4, ran 274 runs in
`tb-nearest`: how much follow re-issues its moves differs by capture.) The runs come from follow
(`UnitFollowUnitController`), the AI (in `TurnController.Tick` and `Prepare`), the hover predictions
(`UnitCommands.Temporary`, from the UI's coroutines and the turn controller's tick), roaming, the oracle's driver, attacks
of opportunity (`UnitCombatEngagementController`) and `LeaveCombat`'s return to the starting position; the interrupts
from `TurnController.Prepare`/`End`, the AI (`InterruptAll(cmd => cmd.AiAction != null)` from `TickBrain`),
`LeaveCombat` (`InterruptAiCommands`), the life and prone controllers and combat's end.

What became checks: the turn controller's `r_empty`, `running`, `run` and `r_run` (a difference is reported, the
recorded value taken); the leave controller's `cmds` rows; `cd_spend` from `UnitCommandController` (the sim makes the call,
the record checks its unit, type, full round and `TimeSinceStart`); a root attack's running attack command. What stays an
input: the class code (`OnStart`, `OnTick`, `OnAction`, `TickApproaching`, `Init`, an overridden `OnRun`: their effect on
the base state, checked where only base code writes: started, finished, result, `TimeSinceStart`, the hands' wait, the
executor outside `Init`), the leaves (positions and line of sight, the view, the movement agent, the hands, `CanAct` and
`CanActInCombat` as the game evaluated them, the class's answers), every call from outside the lifecycle (the AI's,
follow's, the UI's runs; ends from the movement agent), and `TickCommand`'s `canMoveUnit` for the AI's commands (the hold
and stop states aren't modelled; for the others it's checked).

Ported (`combat/commands.rs`): `UnitCommands.Run` (both overloads), `TryUseSpecialHandlers`,
`TryAddToQueueInsteadOfRunImmediately` (with `object.Equals`' reference fast path, by target wrapper id), `AddToQueue*`,
`RemoveFinishedAndUpdateQueue`, `InterruptAndRemoveCommand`, `InterruptAll` (both; the predicate is its caller's, the
AI's ported), `InterruptAiCommands`, `InterruptMove`, `InterruptGroupCommand`, `RemoveNoStartStadartCommand`;
`UnitCommandController.TickOnUnit`, `TickCommandTurnBased`, `TickCommand`, `ShouldStartCommand`; `UnitCommand.Start`,
`Tick`, `OnEnded`, `Interrupt`, `ForceFinish`, `OnRun`, `ShouldUnitApproach`, `IsIgnoreCooldown`;
`UnitCombatState.HasCooldownForCommand` (`cooldowns.rs`); the turn's running commands. Reported, not modelled: mounts,
pause (parked commands), abilities (`IsAvailableForCast`, `CanCast`, full-round abilities), the magus' spell combat,
custom run handlers (the kineticist), group commands, a command running longer than a round, class code changing
`TimeSinceStart` (`UnitDoNothing`), the other callers' `InterruptAll` predicates. Not modelled and not read by the sim
yet: `UpdateCombatTarget` (last and manual targets), `CancelStop`, the AI's `UseAction`, the view's calls, the command
events' handlers (their effects that matter are recorded: `cd_cmd_end`, `mem_add`).

The replay's side (`combat/replay/commands.rs`): entries are placed in their frame's controller phase by their caller (new
phases: `Follow`, `Ai`, `Engagement`, `Roaming`, `Prone`); a turn the sim's life controller cleared before the game's
`LeaveCombat` interrupted its rider's commands counts as current until its recorded clear (`commands::current_turn`).
Every state row is checked as it comes (a row is the state at the moment it's written) and again at the controller's
boundaries and the recording's end.

**Reviews:** gpt-6.1-sol (high, Fast mode), ten rounds, every finding confirmed and fixed; rounds 1 to 9 each found
fail-closed gaps of the checklist's kinds (fields unvalidated or a null indistinguishable from malformed, records of
another frame, rows overwritten before a check, classification metadata, identities not resolved to units) plus a few
port details (`object.Equals`' reference fast path, `CanAct`'s animation gates now a recorded leaf, a repeated
`OnEnded`, a failed merge's target, the executor outside `Init`, the AI's `canMoveUnit`, the comma-decimal culture);
round 10 found nothing. Re-recorded as `poc5-tb-72` (base classes), `-73` (target wrapper ids) and `-74` (`CanAct`).
`missing_or_wrong_command_records_are_reported` damages the capture 72 ways (71 at slice A; slice B review round 32 added one) and each is reported. The regression is
identical to `out-turntick`. Gaps no capture exercises yet: the queue, a merge, two targets compared (unit tests only),
and the observer under a comma-decimal locale.

## Slice B: design (2026-10-13, twelfth session)

The sim runs `UnitAttack`'s class code (`Init`, `InitAttacks`, `OnStart`, `TryStartNextAttack`, `OnTick`, `UpdateTarget`,
`OnAction`, `TriggerAttackRule`) as lifecycle code, and fires the rules it triggers (`RuleCalculateAttacksCount` and
`...Limbs` from `InitAttacks`, `RuleAttackWithWeapon` from `OnAction`) with the fields it computes; the recorded roots
are their checks. A recording with WrathOracle's new observer `attacks` has it; an older one replays as in slice A
(class code recorded). `UnitAttackOfOpportunity` stays recorded class code (its threat hand isn't ported).

WrathOracle (`CommandObs.cs`, observer `attacks`):
- **`UnitAttack`'s code is base lifecycle code** for the stack walk (every method declared on it and on its
  compiler-generated nested types, except the leaves it answers: `TryMergeInto`, the virtual getters,
  `IsUnitInFiveFeetRange`), as are `UnitCommand.StartAnimation`, `ClearAnimation`, `ScheduleAct` and the base
  `Init`/`OnStart`/`OnTick`, and the getters between `InitAttacks` and the attack mode
  (`UnitCombatState.IsFullAttackRestrictedBecauseOfMoveAction`, `TBMState.EnabledFullAttack`). So the leaves its code
  reads are recorded (`close`, `untgt`, ...), and its ends and running-list calls are `base` (the sim's).
- **New leaves:** `five` (`IsUnitInFiveFeetRange(unit)`: positions, corpulences, the weapon's range); `efa`
  (`TurnController.GetEnabledFullAttack(unit)`: the UI's attack mode); and four reads a transpiler taps where
  `UnitAttack` makes them (their getters are auto-properties the JIT inlines): `afin` (`Animation.IsFinished` in
  `OnTick`), `arel` (the previous sequential attack's `AnimationHandle.IsReleased`), `enemies` (`Group.Memory.Enemies`
  in `UpdateTarget`, the units in order) and `sorted` (the candidates after `UpdateTarget`'s sort by distance). The
  walk for `five` and the taps passes `System.Linq` and `System.Collections.Generic` frames (`Where`, `ToList`, `Sort`).
- **`ua_atk`** (at `InitAttacks`' end): the flags it read (`IsSingleAttack`, `ForceFullAttack`, `IsCharge`,
  `OnlyForInspect`, an `OverrideAction`), `IsAttackFull`, `m_AnimationsDuration`, and the attack list in its final
  order: each attack's slot (primary, secondary, limb index), weapon item, penalty, and what `ConfigureAnimations` left
  (a handle, `IsSequential`, the skip test `handle == null || Variant < 0 && clip == null`, a bow).
  `ConfigureAnimations` is presentation except for the order (a sequence reorders on a `UnityEngine.Random` draw),
  the handles' presence and the sequential flags: those are inputs; the sim checks the list is a permutation of its own.
- **`uc_fx` of a `UnitAttack`** also carries its class state (`ua`: attack index, `m_HasStartedAttack`,
  `m_ClearAnimationOnFinish`, `m_AnimationsDuration`, `IsAttackFull`, the last attack rule's number, the target's
  unconsciousness at `Init`, the list's length), a check after each call.

The sim (`combat/commands/attack.rs`): the class state above; `InitAttacks` (`IsAttackFull`: `ForceFullAttack`, pounce
and surprise, `IsSingleAttack`, `IsFullAttackRestricted` with `IsFullAttackRestrictedBecauseOfMoveAction` computed but
for the attack mode, staggered; `CreateFullAttack`/`CreateSingleAttack` from the rules' results and the body's slots;
`EnumerateAttacks`' interleaving and penalties), `TryStartNextAttack`, `OnTick`'s iteration delay
(`MaxDelayBetweenIterativeAttacks` is 1 s), `UpdateTarget` with its retarget, `OnAction` and `TriggerAttackRule`
(`AttackNumber`, `AttacksCount`, `IsFirstAttack`, `IsFullAttack`, `IsRend`, the penalty, the slot). The `uc_fx` row is
then a check of every field but the animation's progress (`IsActed`, `IsFinished` of the current handle, which stay
inputs). Reported, not modelled: a charge, `OverrideAction`, `OnlyForInspect` run through the lifecycle, a save for
attacks (`UnitPartSaveThrowForAttackOrSpell`), a mount, an `InitAttacks` exception, `IsIgnoreCooldown` with a standard
action (`m_AnimationsDuration` from the cooldown is computed, but unexercised), tactical combat.

## Slice B: result (2026-10-13, twelfth session) [M]

On **`poc5-tb-77`** (idle, Manual_18; `tb-nearest` and `tb-focus`, record and two replays, the replays T0-equal to their
record under `oracle-trace-diff --t0`) **every run is valid with `UnitAttack`'s class code computed**: nothing reported,
end state 1.0000, every class-code call's recorded state equal to the sim's, every rule it triggers fired by the sim with
the fields it computed and equal, field for field, to the recorded root. `tb-nearest`: 2,519 `UnitAttack` calls computed
(36 `InitAttacks`: 19 single attacks, 17 full attacks of one to three attacks: two-weapon fighting and an
additional limb among them), 65 rules
fired (attack counts and attacks); `tb-focus`: 1,673 calls, 33 rules. The roots that `UnitAttack.Init` made in the AI's
runs used to be placed as the AI's (`ai`); they are the command's now.

What the fights exercise: the attack mode (`efa`, 30 and 17 reads) and `IsFullAttackRestrictedBecauseOfMoveAction`
(`farm`, computed in turn-based combat, taken outside it: the first attack starts combat), the iteration delay with its
turn-based cap and a bow's (5198 shoots: style 9), `UpdateTarget` every tick (`five`, `close`) and its retarget, both
ways: a dead target replaced by the nearest conscious enemy in reach (`51FC` by `51DD`, `523B` by `51DD`), and a dead
target with no enemy left in reach, restored as a new wrapper and the command interrupted from `OnTick`. Not exercised
(unit tests only, or reported): `IsSingleAttack` and `ForceFullAttack` set by a creator, `arel` (a sequence of two
attacks), `m_AnimationsDuration` from the cooldown, rend, a charge, `OverrideAction`, a save for attacks.

`poc5-tb-75` (it walked `ConfigureAnimations`' internals: a `saddled` leaf per attack) and `-76` (before review round 1:
the weapon read through `MaybeWeapon`, which resolves entity references; no `ConfigureAnimations` gate) are superseded.
`missing_or_wrong_attack_records_are_reported` damages `poc5-tb-77` 202 ways (and a replay run once) and each is reported
(or, for coherent bonus corruption, makes the replay T0-unequal); `a_bonus_addition_the_rule_never_asked_for_is_unequal`
damages `tb-nearest` once more. Since review round 38 T0 also compares each rule's bonuses with the sim's (a `bonuses`
record: the final list and every addition asked for, with its source fact and its place among the nested rules); on
`poc5-tb-77` they agree on every rule. The regression is identical to `out-commands` (rerun after rounds 38 to 40:
`out-slice-b-r38`, `-r39`, `-r40`).

Known gaps for the next capture: WrathOracle's `Rules.Identity` writes no fact or owner for a handler that is a fact
itself (`ActivatableAbility`: the replay counts them against the initiator's subscribed abilities instead), and
`rule_bonus` doesn't record a bonus's dice (the replay requires none: no `AddModifier` caller builds a dice modifier).

**Reviews:** gpt-6.1-sol (high, Fast mode). Round 1: attack roots' other fields defaulted (now every field of the push
snapshot is matched exactly, as are the count roots', explicit nulls included), the weapon's blueprint and the slot's item
unchecked, root metadata and the call chain beyond `UnitAttack`'s method unchecked (`attack_chain`: from the rule up to
the class code running, then its lifecycle caller), rule spans skipped unvalidated (`attack_span`), a recorded
`TickApproaching`'s class state unchecked, `ua_atk` resolving entity references (it now reads the slot's fields), the
duration taken without the `ConfigureAnimations` gate (`cfg`). Round 2: the weapon slot's other fields (its whole snapshot is now matched: a hand's, a
limb's with its parent fact), the span's identities (`span_ok`: each record of the rule or handler open at it, rules and
handlers nested and closed in order), the chain beyond the class code (now down to the active entry's recorded call
site, or the controller's chain, through lifecycle frames only), a list reordered without `ConfigureAnimations`, flags no
handle can have, a style outside `int`, and an attack initialized before the window (reported: its class state isn't
recorded). Round 3: the span's attribution (each nested rule's handler and context parent, each draw's and
mutation's rule and handler, a draw's context, a handler's phase and class; rules a handler triggers nest inside it), a
`CountableFlag` outside `short` or whose value isn't `m_Count > 0`, a list reordered without a sequential prefix (a
sequence adopts the working copy, which earlier failed sequences may have swapped: any order then; without one, the
sim's), a retarget's wrapper seen before (WrathOracle numbers wrappers at first sight: a new one is above every one
shown). Round 4: the span validated as `Rules.cs` writes it (`span_ok`: each rule's steps once and in
order, handlers on the global stack with their phase in its stage, a nested rule's draws in its ancestor's open handler,
a draw's whole context with the units' stable ids and its ordinal, environments of the innermost rule's class and units
with positions where that kind has them, diffs well formed). The roots the replay drives from their records
(`drive_root`, the rules no ported code triggers) still skip their spans unvalidated: a gap older than slice B, for the
round that ports their callers. Round 5: the payloads inside the span (environments of the kinds the replay reads with every
field typed and positions of two finite `float` triples; handler identities: a component handler's class its
component's type and its fact the sim's, of that blueprint and owner; nested rules' snapshots and reasons; diffs of the
rule's own fields with values of their kind; bonus entries; no foreign record), and the limb's item reference the same
from `ua_atk` to the attack's weapon and slot. Round 6: a shield bash rejected (the active hand's weapon may be its shield's weapon
component), `RulebookEvent`'s fields in every snapshot and their flags never null in a diff (a class's own fields may
move between null and a value: the replay has no schema of them), mutations of the kinds seen inside attacks with their
payloads (another kind is reported), reasons' fields each of its kind, an attack roll's and a resolve's environment
exactly once, rule and handler ids in allocation order (checked over the whole trace), the origin's lifecycle frames
named, and global handlers among the game's subscribers seen inside attacks (another is reported). Round 7: a presentation draw inside a rule rejected (it has no context and doesn't advance the
rule's draw ordinal), draws' payloads (class, member with its arguments, finite values, site and key, generator state),
a component handler resolved on its fact (the component of that source name, of the handler's class), `UnitAttack`'s
frames of a root's call site named (the full attack's iterator, `CreateFullAttack` or `CreateSingleAttack`,
`InitAttacks`, `Init` or `OnStart`; `TriggerAttackRule`, `ProvideNextAttack`, `OnAction`), a modifier's references each by
its schema, the slot reference's declared class (`ItemEntity`), and a field twice in one diff. Round 8: the replay runs were invalid (a replay's simulation draws carry `forced`, `match` and
a `fallback`, which round 7's draw check rejected), and the suite's validity checks didn't cover `poc5-tb-77` (an older
whitelist): both fixed, every run checked. Also: draws' numbers by overload (`int` or finite `float`), the call chain
by the lifecycle's call edges, nested rules' classes in the rules' namespaces, a bonus's target its own rule, C# `int`
widths, the `AllBonuses` exception for `RuleCalculateAttackBonus` alone, and sequence numbers on every span record. Round 9: an attack queued into a free slot rejected (`AddToQueueOrRun` calls `Run`), a replay's
draw from a failed tape accepted (now only forced and matched, without generator steps), steps and gaps beyond their
writer's searches, and records without their sequence number (now checked over the whole trace, in order). Round 10: a replay's draw schema from the trace's mode (`begin.mode`), the engagement ids
resolving to units, a rule triggered at its end (`OnDidTrigger`), and ids within C# `int`. Round 11: a reason's rule reference (an `int`, allocated before), the triggered flags false
until `rule_end`, a global handler only on the rules it handles, and a handler's owner by its kind (an item with the
sim's blueprint). Round 12: a component handler only on a rule and phase its type handles (the schema's
`TypeDef::handlers`), an enemy the sim lacks reported (it can't tell a reference that doesn't resolve), a reason's caster
the sim's unit, simulation draws and environments in the rule's computation (`OnTrigger`) and HP changes after `did`,
draws' results within their member's bounds, and draw sites in the trace's table. Round 13: `ShadowSpellController` (a `RulebookTargetEvent` subscriber) only on the target-event
rules seen, a reason's fact and item the sim's (a limb's item by the id `InitAttacks` recorded), concealment within its
enum, and nested rules' units the sim's. Round 14: a reason's rule one the trace allocated, a bonus's fact the sim's, and a draw's
member and class its site's (the bark picker's presentation exception kept). Round 15: the root's reason exactly the one `AssignSource` gives (itself, its initiator, an
attack's weapon), mutations' identities the sim's (units, a bonus's or modifier's fact, a modifier's item with its
blueprint, a detached fact), a buff's timing required, rule types by their full names, and the trace's mode read
strictly. Round 16: a reason's ability and context blueprints the game's, a global hook's draw (site
-1, its key naming its member), and a modifier's stacking and descriptor name derived from its stack mode and descriptor. Round 17: a rule's end bonuses the same as its `AllBonuses` after the last diff. Round 18: a rule's end bonuses exactly the ones its `rule_bonus` mutations added, in order (folded as
`ModifiableBonus.Add` does since round 19: a non-stacking descriptor's bonus replaces a smaller one or is dropped), and a
target event's snapshot naming the envelope's target. Round 19: the bonus folding above, and a target event's `Target` kept through every diff (it's
readonly). Round 20: a target event's snapshot required to have its `Target` (no other rule has one), and
`AllBonuses` never null. Round 21: a nested rule's reason kept from its push to `about` (unless a `mod_before` handler
ran), the bonus container's schema after every diff (`m_ModifiableBonus` null or its mirrored, counted modifiers, the same
as `AllBonuses`), and an animation read as finished in `OnTick` and kept, finished at its check. Round 22: a nested rule's reason non-null at `about` (`AssignSource` gives one), the end bonuses
checked against the container where the snapshot has no `AllBonuses`, and a handle seen finished kept finished in every
later row until the class code replaces or clears it. Round 23: `OnTick`'s finished read of a handle it kept equal to the row after it (unless an
interrupt released it), `RulebookEvent`'s containers keeping their kinds and its battle-log invariant after every diff,
and bonus facts resolved in every intermediate state. Round 24: a nested rule's assigned reason its context's (`AssignSource` without a fact in context;
another is reported), its context parent its parent, and no temporary modifiers or custom data (reported if seen). Round 25: a limb's
recorded item bound to the game's id for that limb (the limbs rule's `Slots` at its end, or a first attack list where no
rule ran), kept for the run, and an environment's `EngagedBy` ids ordinal-sorted and unique. Round 26: every recorded `Slots`
value of the limbs rule a growing prefix of the sim's counted limbs, each a whole slot snapshot, and a limb's id never
another entity's (an id the sim knows must be that limb's item) or another limb's. Round 27: a reason's `ability` a
`BlueprintAbility` (`AbilityData.Blueprint`), and a limb's id bound to the sim's item it holds rather than the limb's
index, so a limb granted again (`AddAdditionalLimbItem` reuses the empty slot for a new item) binds the new item's id. Round 28:
a stat mutation's stat resolved on its owner, an inactive limb's weapon the owner's empty hand by the sim's id
(`WeaponSlot.MaybeWeapon`; only a limb's own item goes by its recorded id), an empty attack list reported like a null one
(`m_AllAttacks.Empty()`), and every rule step's record exactly the fields `Rules.cs` writes for its kind. Round 29: a `stat_value` mutation a change
(old != new, never `UpdateValuePrefix`'s sentinel), and a modifier's `source_component` a component of its source fact
(none without a source). Round 30: the memory's enemies each once, at most two `Run` frames in a row in an origin
(the public overload calls the private one), every diff pair a change, each bonus and modifier one a `Modifier`
constructor makes (a stat's has no type, descriptor, dice or fact; a type's no stat, dice or fact; dice or a fact no type
or stat), and a `ua_atk` record exactly its writer's fields. Round 31: a rule's bonuses the ones its mutations added
so far after every diff (not only at its end), records numbered without a gap from 0 and rule and handler ids from 1
(`Rules.Reset`, then `++`), and a flat-footed check's environment only at the root (`FlatFootedPrefix`). Round 32: a `rule_bonus` mutation's target the
innermost rule (0, an untracked rule, reported), every leaf record (slice A's too) exactly its writer's fields, and the
animation style a `WeaponAnimationStyle` (0 to 23). Round 33: an about/did handler subscribed to the rule
(`RulebookEventBus`: a global handler, an initiator handler whose subscribing unit, its owner or an item's wielder, is
the rule's initiator, or a target handler whose unit is its target). Round 34: a keyed draw's key the writer's format
(`Rng.CallerSite`), its member the draw's and its class the caller type's (`IsPresentation`; no caller: simulation);
and `ActivatableAbility` handlers, whose records name no ability (`Rules.Identity` writes a component's fact, not a
fact's own), one per subscribed ability of the initiator at about and at did. (Known gap: the oracle could write such a
handler's fact and owner; the next capture should, so the replay can check which ability ran.) Round 35: a rule's component handlers at about and at did
exactly the subscribed components' dispatch (each subscribed component of a turned-on fact once per manager it's
subscribed to that dispatches the rule; a multiset: the subscribers' order isn't checked), and no `$failed` marker in a
snapshot or diff value. Round 36: each global singleton (the controllers and achievements in `GLOBAL_HANDLERS`, subscribed for
the session) once at about and at did on every rule it handles, and a rule's bonuses without dice (`AddModifier`'s
callers build none: `Modifier`'s dice constructor serves damage descriptions). (Known gap: `rule_bonus` doesn't record
the dice; the next capture's oracle could.) Round 37: inside attacks only a rule bonus or damage mutation (stat values
and modifiers, facts and experience aren't reconciled there: reported; round 28's and 29's stat checks stay for
elsewhere), an achievement's handler once or never and the same on every rule of the run it handles (its subscription
follows the unlock state, which the save doesn't hold), and a snap map a `WeaponParticlesSnapMap`. Round 38: rule bonuses compared with the sim's
computation, not only among the recorded fields: T0 gains a `bonuses` record (a rule's `rule_end` bonuses when it has
any: value, descriptor, type, stat; the sim's from `RuleEvent::all_bonuses`, which `RuleCalculateAC` and both attack
bonus rules give from their `BonusList`), so a bonus the sim's rule never adds, however coherently recorded, makes the
replay unequal. On `poc5-tb-77` the sim's bonuses equal the recorded ones on every rule that has any (8 in `tb-focus`,
23 in `tb-nearest`). Round 39: the `bonuses` record carries each bonus's source fact (the sim's `Bonus` keeps
the fact a component passes to `AddModifier`) and every addition asked for in order, rejected ones too (the sim's
`BonusList` keeps its calls; the oracle's from the `rule_bonus` mutations), so another fact named coherently, or an
addition the container rejected but the rule never asked for (`a_bonus_addition_the_rule_never_asked_for_is_unequal`,
on `tb-nearest`), makes the replay unequal; on `poc5-tb-77` both agree on every rule. Round 40: each addition carries its place (the
rules pushed after its rule before it: the sim's from `Rulebook.next_seq` through `BonusList::add_in`, the oracle's
from its rule ids), so one moved past a nested rule is unequal; a kept handle seen acted stays acted (`IsActed`
only becomes true) until the class code starts another or clears it; and a target wrapper's first sight is the next id
`TargetStr` allocates (`++_nextTarget`, within a C# `int`). Round 41: a sequence's previous handle, read released, released in every later
read until `InitAttacks` builds another list (`IsReleased` only becomes true; `attack::State.released`, scripted test
`a_released_sequence_handle_stays_released`), and a rule's bonus container allocated only with an addition
(`AddModifier` creates it as it adds the first). Round 42: `OnAction` releasing a skipped sequential entry's handle as it moves past it
(`AnimationHandle?.Release()`), so a later read of it unreleased is a desync (scripted test
`a_skipped_sequence_handle_is_released`). Round 43 found nothing: the reviews converged after 43 rounds (residual gaps it names: live
captures of sequences and of the ignore-cooldown duration; charge, rend and the save check reported as unsupported).

## Slice C: design (2026-10-13, thirteenth session)

The sim runs `UnitCommand.TickApproaching` as base lifecycle code, and the class code of `UnitMoveTo` and
`UnitAttackOfOpportunity` as slice B runs `UnitAttack`'s. The movement agent stays an input: its tick (positions,
the A* graph, avoidance, the agent's own interrupts) needs the nav port. What the lifecycle reads from it is a
leaf (`moving`, `rmoving`, and now `WantsToMove`), and the lifecycle's calls into it (`UnitEntityView.MoveTo`,
`UnitMovementAgent.FollowPrecomputedPath`, `UnitEntityView.StopMoving`) are recorded and checked against the sim's.
A recording with WrathOracle's new observer `approach` has it; an older one replays as in slice B (`TickApproaching`,
`UnitMoveTo`'s and the attack of opportunity's class code recorded).

`TickApproaching` [C], as the sim runs it: return when `!ShouldUnitApproach` for a free or swift command, or for a
mounted unit's AI command that isn't a rider's (`SaddledPart`: reported); `OnTickApproaching` (class code: only
`UnitInteractWithObject`, `UnitUseAbility` and `UnitDestroyObject` override it, recorded); `ShouldUnitApproach` again
(its leaves `await`, `moving`, `close`, `nostop`): for a rider (`RiderPart`: reported) `IsMoveUnit`; before
`NextApproachTime` an interrupt in turn-based combat when the agent doesn't want to move and the turn has no movement
for it (`HasMovement`, `State.CanMove`, `UnitCanGetUpOnCommand`); after it, an AI command that finished approaching and
never started is interrupted, else `FinishedApproaching = false`, `IsMoveUnit`, and in turn-based combat (or tactical:
reported) the command's `ForcedPath` or the path visualizer's (`PathVisualizer.Instance?.CurrentPathForUnit`): none or
empty interrupts the command (and its rider's), else `SavedCommandPath_ForSave` (the turn's, kept in the sim's turn
state) and `FollowPrecomputedPath(path, tactical, ApproachRadius)` with `NextApproachTime = MaxValue`; outside it
`View.MoveTo(this, ApproachPoint, ApproachRadius, MaxApproachRadius, target's view or own, DistanceXZ)`. Close enough:
`FinishedApproaching`, `StopMoving`, `TurnToTarget` (the orientation: not modelled), and after a move
`NextApproachTime = GameTime + Random.Range(0.2, 0.6)` (a simulation draw: site `UnitCommand.TickApproaching`, taken
from the tape where the sim draws, its record checked there; `float.Seconds()` is `TimeSpan.FromSeconds`, rounded to
milliseconds).

WrathOracle (`CommandObs.cs`, observer `approach`):
- **Base code:** `TickApproaching` (no longer `uc_fx`), and every method declared on `UnitMoveTo` and
  `UnitAttackOfOpportunity` but the leaves they answer (`AwaitMovementFinish`, `DontWaitForHands`, `ShouldBeInterrupted`,
  `ShouldTurnToTarget`, `TryMergeInto`), their constructors and `AddRiderCommand`. Their `Init`, `OnRun`, `OnStart`,
  `OnTick` and `OnAction` still write `uc_fx` (checks); a base method called from an override (`base.Init()`) isn't a
  class-code call of its own (the rule now for every class: the call is the instance's most-derived implementation).
- **Taps** (a transpiler on the method, as slice B's): in `TickApproaching` `spart` (`Executor.SaddledPart` non-null),
  `rpart` (`RiderPart`), `tact` (`TacticalCombatHelper.IsActive`), `wants` (`AgentASP.WantsToMove`), `hasmove`
  (`TurnController.HasMovement`), `canmove` (`State.CanMove`), `ucgu` (`UnitCanGetUpOnCommand.Value`), `fpath` (the
  command's `ForcedPath`: null or its points), `pvis` (`PathVisualizer.Instance` non-null) and `vpath` (its
  `CurrentPathForUnit`: null or the points); in `UnitMoveTo.OnAction` `pmf` (`AiAction.PreventMovementFinish`); in
  `UnitAttackOfOpportunity.Init` `threat` (`GetThreatHand`: the slot as `ua_atk` writes it, or null) and in its `OnRun`
  `anim` (the first `View.AnimationManager`, non-null by Unity's `==`) and `idle` (`CanRunIdleAction()`).
- **Calls** (`uc_call`, from base code only, by the stack walk): `moveto` (the point, both radii, the target view's
  unit, `distanceXZ`), `follow` (the path's points, `forcedPath`, the radius) and `stop`, each with its command or
  unit. `TickOnUnit`'s `StopMoving` of a unit left without commands is one too.
- **Descriptions:** `uc_new` gains `ap` for every command (`ApproachRadius`, `m_MaxApproachRadius`, `DistanceXZ`,
  `NeedLoS`), `mv` for a `UnitMoveTo` (`Roaming`, `Orientation`, `PreventInterrupt`, the await override, the rider
  target, `MovementDelay`) and `aoo` for an attack of opportunity (`ForceFlatFooted`, `m_ShouldTurnToTarget`).

The sim (`combat/commands.rs`, `commands/move_to.rs`, `commands/aoo.rs`): `TickApproaching` above; `UnitMoveTo`'s `Init`
(the base), `OnRun` (the base's, then `CombatState.ReturnPosition`/`ReturnOrientation` for an AI's command of a
non-player unit in combat: not modelled, as the return to the starting position is a recorded run), `OnStart` (the
base: a move without `TriggerAnimation`), `OnTick` (none), `OnAction` (`StopMoving`, the orientation, `Success` unless
the AI action prevents the finish) and its answers (`AwaitMovementFinish`, `DontWaitForHands`: computed, the leaves
checked); the attack of opportunity's `Init` (`GetThreatHand` computed from the body and the features, checked against
`threat`; none interrupts), `OnRun` (the base's, then the animation's gate and `GetSaddledUnit`: presentation past
that), `OnAction` (`RuleAttackWithWeapon(executor, target, hand's weapon, 0)` with `IsAttackOfOpportunity` and
`ForceFlatFooted`, fired by the sim unless the target is dead; `Success`) and `ShouldTurnToTarget` (checked).
`ShouldBeInterrupted` (`CanAttack`) stays a recorded leaf. Reported, not modelled: mounts, tactical combat, the
overridden `OnTickApproaching`, `UnitMoveAlongPath`, a rider target.

## Slice C: result (2026-10-13, thirteenth session) [M]

On **`poc5-tb-80`** and **`poc5-tb-81`** (idle, Manual_18; `tb-nearest` and `tb-focus`, record and two replays) **every
run is valid with `TickApproaching`, `UnitMoveTo`'s and the attack of opportunity's class code computed**: nothing
reported, T0-equal, end state 1.0000, no class code taken from the recording (`class code taken 0`), every agent call and
`TickApproaching` draw the sim's and equal to its record. `poc5-tb-80/tb-nearest`: 245 runs, 455 `UnitMoveTo` class-code
calls, 993 agent calls, 165 draws; `tb-focus`: 525 runs (follow re-issuing its moves), 1,016 class-code calls, 874 calls,
124 draws. `poc5-tb-81/tb-focus` has two attacks of opportunity (`UnitCombatState.Disengage`'s), each with its threat hand
computed (`GetThreatHand`), `OnRun`'s animation gate read and its `RuleAttackWithWeapon` fired by the sim
(`IsAttackOfOpportunity`, no slot) and equal to the recorded root. Its `tb-nearest` reached `ACBonusAgainstFactOwner`
in `poc5-tb-79`: ported (`combat/components.rs`, a target handler of `RuleAttackRoll` and `RuleCalculateAC` with its
runtime data; a locked alignment mask is reported). `poc5-tb-78` and `-79` (before review round 1: no `tview`) are
superseded.

What stays an input: the movement agent (its tick: positions, paths, `WantsToMove`, `IsMoving`, `IsReallyMoving`, and its
own interrupts, `OnMovementInterrupted`: 2 to 4 ends per fight are still taken), the paths (`ForcedPath`, the AI's; the
path visualizer's, the UI's), `HasMovement`, `State.CanMove`, `UnitCanGetUpOnCommand`, positions and line of sight
(`IsUnitCloseEnough`, the approach point of a unit target), a `UnitAttack`'s approach radius (corpulences, the weapon's
reach), `ShouldBeInterrupted` of an attack of opportunity (`CanAttack`), the animation gate, the AI action's
`PreventMovementFinish`. Not modelled (reported where read): mounts, tactical combat, `UnitMoveAlongPath`, a rider's
target, an overridden `OnTickApproaching` (recorded class code), a threat limb whose item changed between `Init` and
`OnAction`. Not modelled and not read by anything ported: the orientations (`TurnToTarget`, `LookAt`,
`DesiredOrientation`) and `CombatState.ReturnPosition`. `SavedCommandPath_ForSave` is kept in the sim's turn state.
Not exercised yet by a capture: the stuck approach's interrupt (`hasmove`, `canmove`, `ucgu`), the path visualizer's
path (`pvis`, `vpath`: the player's turn-based moves follow it), `pmf`, a target without a live view (`tview` false):
unit tests only.

`missing_or_wrong_approach_records_are_reported` damages `poc5-tb-80/tb-nearest` 32 ways and
`missing_or_wrong_opportunity_records_are_reported` `poc5-tb-81/tb-focus` 11 ways; each is reported.

**Reviews:** gpt-6.1-sol (high, Fast mode), four rounds. Round 1: an extra `TickApproaching` draw hidden by a contradictory
`key` (draws are now recognized by their numeric site, and a key rejected), approach radii and points checked only for
the classes whose code the sim runs (another class's recorded `Init` may change them: `UnitInteractWithUnit`), a target
unit without a live view (`MoveTo` falls back to the executor's: WrathOracle's `tview` tap; re-recorded as `-80`), the
`commands` observer required with `approach`. Round 2: `ACBonusAgainstFactOwner` read an alignment unmasked (`ValueRaw`
applies the lock: reported), attacks of opportunity counted with `UnitAttack`'s rules (`approach_rules` now; `-81`
added). Round 3: a threat limb's item removed or granted again between `Init` and `OnAction` (reported). Round 4 found
nothing. GPT 6 Astra (high) reviewed the hooks (taps, transpiler sites and counts against the IL, the stack walk's
classification, `_fx` balance): nothing found. The regression (`out-slice-c`) is identical to `out-slice-b-r40` but
for `ACBonusAgainstFactOwner` counted implemented in the manifests.

## Slice D: design (2026-10-13, fourteenth session)

The sim runs the AI (`combat/ai.rs`): `AiBrainController.Tick`, `ForceTick` (the ready groups' units, the turn-based
gate), `TickBrain` (sleeping, passive, the `NextCommandTime` gate, `CanInterrupt`), `SelectAction`, `FindBestAction`,
`CalculateActionScore`, `CalculateTargetsScore` (the priority fact, impatience, sleeping targets), the static
`DecisionContext` it decides in (its enemies list, kept for a group and sorted again for each unit; the paths and
`BestEnableFiveFootStep`, kept beyond a decision), `TargetInfo` (`UpdateDistance`, `FillWeight` with `UnitEntityData ==`'s
null equality, `InLos`' cache, the sort: Mono's introsort), `AiAction.ScoreActor`, `ScoreTarget` and `ScorePath` (its
score arithmetic and the movement simulation's loop), `AiActionSimple`, `BlueprintAiAction.IsActorValid` and
`BlueprintAiAttack` (`CreateCommand`: `new UnitAttack(target)`, `CanKeepCommand`), and the considerations the fights use
(`ComplexConsideration`, `InRangeConsideration`, `LifeStateConsideration`, `FactConsideration`,
`NotImpatientConsideration`). Scores are C# `decimal`s (`csharp/decimal.rs`, a port of Mono's `decimal-ms.c`: the scale is
kept, `0.8 * 0` is `0.0`). The AI's commands go through the commands model: `Run` (nested), `InterruptAiCommands`,
`InterruptAll(cmd => cmd.AiAction != null)`. `SelectAction`'s `Random.Range(0f, 0.1f)` is a simulation draw taken where
the sim draws (site `AiBrainController.SelectAction`). A unit's brain comes from the save (`Descriptor.Brain`), or for a
unit the scene spawns from its blueprint's default brain (`SetBrain`, `RestoreAvailableActions`).

WrathOracle (`CommandObsAi.cs`, observer `ai`): the AI's types are base lifecycle code for the stack walk; `Tick` and
`ForceTick` are entries (`uc_ai`, with the caller and `LimitedAI`; `uc_ai_end`), a `ForceTick` that `Tick` calls nested.
Every read the sim can't compute is a transpiler tap at its call (`uc_v`): `wfui`, `groups` (`ReadyForCombatUnitGroups`:
each group's id, units by reference and `IsInCombat`, checked against the sim's groups), `sleep`, `passive`, `nct` and
`time` (the Unity clock), `aici` (`AiCanInterrupt`: the follow and return-to-position moves carry a mark the descriptions
don't have; a `UnitAttack`'s is checked), `conf`, `auto`, `magus`, `pe`, `ptf`, `disliked`, `ipt` (the impatience
controller isn't ported), `enemies` and `hpcm` (group memory), `manual`, `last`, `rider`, `corp`, `dist`, `los`, `amf`,
`unreach`, `aien`, `diff` (the difficulty setting), `range` (the weapon's reach), and in `ScorePath` the movement's:
`mrange`, `speed`, `path` (what `FixPathIfTooShort` leaves: an input, not base code, since its reads depend on the path
it was given), `pclose`, `mag`, `plen`, `r5`, `h5`, `hfra`, `msim` (each simulation step), `ablk` (`AllyBlocked`) and `aoo`
(`EngagementSimulator`, internal: a postfix). Checks: `ai_sc` (the score `ScoreActor`, `ScoreTarget` and `ScorePath`
leave: decimals as `ToString` writes them, with `ScorePath`'s five-foot step and path), `ai_best` (`FindBestAction`'s
action, target, scores, five-foot step and path) and `ai_sel` (`SelectAction`'s result and the unit's `NextCommandTime`).
The AI's `ForcedPath` is then the sim's: `TickApproaching`'s `fpath` leaf is checked against it.

Reported, not modelled: other actions and considerations (casts, custom actions, an auto-use ability, a magus' spell
before an attack), confusion, a personal enemy, an aggro filter, action cooldowns and combat counts, an action's
`ActionList`, other target types, `InRangeConsideration.OnlyIfThreated`, more than 16 targets (the list's capacity decides
the sort). Inputs that stay: the movement and pathing (paths, the simulation, line of sight, positions), the Unity clock,
the memory, the impatience controller's state, `LastTarget`/`ManualTarget` (their writers aren't ported), the settings.

## Slice D: result (2026-10-13, fourteenth session) [M]

On **`poc5-tb-82`** (idle, Manual_18; `tb-nearest` and `tb-focus`, record and two replays) **every run is valid with the
AI computed**: T0-equal, end state 1.0000, nothing reported, no AI decision taken from the recording. `tb-nearest`: 3,062
AI entries (`Tick` every frame, the turn controller's `ForceTick`s), 8 decisions (each `SelectAction` a `UnitAttack`), 32
scores checked, 8 commands run and 8 draws, each the sim's and equal to its record; `tb-focus`: 3,422 entries, 9
decisions, 44 scores. `NextCommandTime` is the sim's through every write (`tb-nearest`: 16 of the AI's, checked; 28
`Engage`, 1 `Disengage`, 276 `UpdateCombatTarget`, inputs) and every read is checked against it. Enemies spawned by the
scene (placeholders) get their brain from the blueprint (`m_Brain`: "Dumb Monster Brain", one `AttackAiAction`).
The attack-count roots of the AI's runs, placed as the AI's (`ai`) in older captures, are the commands' here
(`command (UnitAttack)`): no root is placed as the AI's.

Not exercised by a capture (code review and damage tests only): `ScorePath`'s movement simulation (every AI path in
these fights is close enough: `plen`, `r5`, `h5`, `hfra`, `msim`, `ablk`, `aoo`), a failed path, line of sight
(`los`: no unit's movement is forbidden or a target unreachable), the impatience factors, a priority fact, sleeping
targets, `CanKeepCommand` (real-time decisions run only for 5198 with no enemy). A fixture whose enemies start out of
reach would exercise the movement branch.

`missing_or_wrong_ai_records_are_reported` damages `poc5-tb-82/tb-nearest` 71 ways (entries, groups, gates, clocks,
the scoring's inputs, the checks, draws, the AI's command, `NextCommandTime`'s writes, observers) and each is reported.
The regression (`out-slice-d`) is identical to `out-slice-c`.

**Reviews:** gpt-6.1-sol (high, Fast mode), eleven rounds, every finding confirmed against the C# and fixed; GPT 6 Astra
(high) reviewed the hooks once (an observer lookup outside the hook's exception guard: every hook's own read is now
guarded). Round 1: the AI controller's tick required every frame (and one `ForceTick` per `Prepare` and per due forced
AI tick), `NextCommandTime` tracked through its writes (`ai_nct`, re-recorded), the inverted difficulty read once, a
`ScorePath` without a free step reported, nested origins and fields checked. Round 2: `ai_nct` only with the observer,
top-level origins, a group listed twice, negative distances, a path's error against its state, `plen` computed from the
points and `r5` from the sim's movement stats. Round 3: external `NextCommandTime` writes are 0, saddled units
resolve, the ready groups in the sim's group order, a fake path is two points, `HasFullRoundAction` the sim's, step
lengths and waypoints bounded. Round 4: corpulence at least 0.5, waypoints below the point count, a step's length its
move's. Round 5: forced AI ticks inside the turn controller's tick, waypoint transitions, paths of two points or more,
enemies unique, magnitudes non-negative, `AllyBlocked` consistent per position. Round 6: `Time.time` one value per
frame, `ReachedMaxDistance` checked, an AI attack's constructor approach. Rounds 7 and 8: every read that can't change
within a decision checked for agreement (corpulence, targets, rider, movement gate, AI switch, range, difficulty,
movement range, speed, priority fact, impatience target, disliked and unreachable, personal enemy, five-foot step), the
sleeping gate against the frame's awake units, the clock never going back, the five-foot restriction. Round 9: every
provably ready group listed, the movement range non-negative. Round 10: a kept two-point path's span, a listed group
provably not ready, a stale step length only on a stopped step, simulated attacks of opportunity capped by the memory.
Round 11 found nothing (residual gaps: the branches no capture exercises, above).

## Slice E: design (2026-10-14, fifteenth session)

The sim runs the engagement controller (`combat/engage.rs`): `UnitCombatEngagementController.Tick` (each unit of
`State.Units` alive and in combat: `TickUnit`; then the forced, provoked and after-defensively-fail attacks of
opportunity; then every unit's `PreventAttacksOfOpporunityNextFrame` cleared and `ClearAttackOfOpportunityTickData`),
`TickUnit` (`CanAct`, `GetThreatHand`, `GetThreatRange`, engaging each conscious enemy of the group memory in reach,
disengaging each engaged unit out of reach), `UnitEngagementExtension.IsEngage`, `IsAttackOfOpportunityReach`, and
`UnitCombatState`'s `IsEngage`, `Engage`, `Disengage`, `ShouldAttackOnDisengage` (turn-based: the real-time branch is
reported), `AttackOfOpportunity` (its `UnitAttackOfOpportunity` run through the commands model, nested: no longer a
recorded run) and the attacks of opportunity's count (`m_AttackOfOpportunityCount`: its setter's 5.4 s cooldown, the
cooldowns controller's and `TurnController.Prepare`'s refills with `DisengageAttackTargets.Clear()`, `Clear()`'s), and
the engagement part of `Clear()` (leaving combat). `m_EngagedUnits`/`m_EngagedBy` are .NET `Dictionary`s: their
enumeration order (`EngagedUnits`, `EngagedBy`) is the entries array's, a removed entry's slot reused first (LIFO free
list); the sim keeps that layout (`csharp/dict.rs`). `ProvokeAttackOfOpportunity` from the controller's rule handlers
(`RuleAttackRoll`: ranged and not `DoNotProvokeAttacksOfOpportunity`, not fake; `RuleAttackWithWeapon`: unarmed without
`ImprovedUnarmedStrike`) is the sim's where it fires the rule; other callers (getting up, facts' forced attacks, the
defensive-casting failure) are reported. `Engage`'s and `Disengage`'s `NextCommandTime` writes are the sim's: their
`ai_nct` records become checks. The attack environment's `target_engaged_by` becomes a check against the sim's
`EngagedBy`.

WrathOracle (`CommandObsEngage.cs`, observer `engage`, which requires `commands`, `attacks`, `approach` and `ai`):
- **Entry:** `UnitCombatEngagementController.Tick` (`uc_eng`, with its caller; `uc_eng_end`). Base code: every method
  of the controller (and its nested types), `UnitEngagementExtension.IsEngage`, `UnitHelper.IsAttackOfOpportunityReach`,
  and `UnitCombatState`'s `IsEngage`, `Engage`, `Disengage`, `ShouldAttackOnDisengage`, `AttackOfOpportunity` (and its
  local `CheckMobility`), `ClearAttackOfOpportunityTickData`, the count's and `LastTarget`'s accessors, `EngagedUnits`,
  `EngagedBy`, `CanAttackOfOpportunity`, `AttackOfOpportunityPerRound`, `PreventAttacksOfOpporunityNextFrame`'s
  accessors, with their nested types.
- **Checks:** `eng_tu` (a `TickUnit`, its unit), `eng_e` (`Engage`'s and `Disengage`'s unit and target, each after its
  work: the two dictionaries' layouts of both units), `eng_aoo` (`AttackOfOpportunity`'s arguments and result),
  `eng_prov` (a call of `ProvokeAttackOfOpportunity`, `...AfterDefensivelyFail` or `ForceAttackOfOpportunity`, with
  its caller).
- **Taps** (`uc_v`, a transpiler on the call as slice D's): in `TickUnit` `threat` (`GetThreatHand`: checked), `trange`
  (`GetThreatRange`: null or the meters) and `enemies` (`Memory.Enemies`); in `IsAttackOfOpportunityReach` `corp`
  (both views), `sqd` (`SqrDistanceTo`) and `los` (`HasLOS(enemy)`); in `IsEngage` and `ShouldAttackOnDisengage`
  `conc` (`UnitPartConcealment.Calculate`); in `Disengage` `tact`, `last` and `manual`; in `ShouldAttackOnDisengage`
  `last` (each read), `immune` (the turn's `ImmuneAttackOfOpportunityOnDisengage`: checked against the sim's, which
  `TickMovement` sets on a five-foot step and the turn's tick clears), `motion` (`HasMotionThisTick`); in
  `AttackOfOpportunity` `prevent` (both units' flag: its writers, teleports, aren't ported), `passive`, `motion`,
  `fmove` (`UnitPartForceMove`, either unit), `atr` (`UnitPartAttackTypeRestriction`), `memc`
  (`target.Memory.Contains(Unit)`), `threat`.
- **Initial state:** `eng_init` at the window's first tick: each unit's count, engaged units and engaged-by (in their
  enumeration order), `DisengageAttackTargets`, the attacked-by-AoO list and the prevent flag. The sim adopts it only
  when both dictionaries are empty (no removal history); otherwise it reports.

Reported, not modelled: mounts (`TurnController.Prepare`'s rider and mount refills), tactical combat, the mobility
checks (`UseMobilityToNegate...`: `RuleCalculateCMD` and a skill check), `AttackOfOpportunityLimits` (the orientation),
a trickster's attack, the real-time branch of `ShouldAttackOnDisengage`, getting up (`HandleUnitWillGetUp`), the
facts that force or provoke attacks of opportunity, a ranged attacker's snap shot (`GetThreatRange` is an input, but
the rule that provokes is the sim's), and `HandleUnitEngaged`/`HandleUnitDisengaged` (`AutoPauseController`: the
real-time auto-pause; a mode change shows in the frame's mode).

## Slice E: result (2026-10-14, fifteenth session) [M]

On **`poc5-tb-85`** and **`poc5-tb-91`** (idle, Manual_18; `tb-nearest` and `tb-focus`, record and two replays) **every run
is valid with the engagement controller computed**: T0-equal, end state 1.0000, nothing reported. Every engagement
tick is the sim's (`tb-nearest` of `-85`: 2,018 ticks, 8 engagements), each `Engage`/`Disengage` checked with both
dictionaries' enumeration order, each `AttackOfOpportunity` call checked, the attack of opportunity a disengage runs
the sim's (its `UnitAttackOfOpportunity` run nested in the tick, its count write and 5.4 s cooldown checked: one in `-91/tb-focus`),
and `Engage`'s and `Disengage`'s `NextCommandTime` resets are checks now (they were the AI's inputs). The archer (5198)
provokes with each ranged attack: the sim's `RuleAttackRoll` handler queues it (1 or 2 a fight, each call matched to
its rule's handler, in order) and the tick's provoked attacks of opportunity run over its `EngagedBy` (empty here). The
attack environment's `target_engaged_by` and, in turn-based combat, its `IsFlanked` are checks against the sim's
engagement. The turn's `ImmuneAttackOfOpportunityOnDisengage` is the sim's (a new turn's and each turn tick's false,
`TickMovement`'s five-foot step true), its read checked.

`poc5-tb-83` and `-84` (before `eu_slots`; `-83` with `CheckMobility` unhooked) were valid in all six runs each with
1–2 attacks of opportunity computed a fight, and are superseded. `-86` to `-90` replay valid but for `-86/tb-focus`,
whose initiative order has a full tie (`CombatController.SortUnits`' identity-hash fallback: the turn order port's,
reported); disengages are rare (none in `-85` to `-90`), so the damage cases use `-91`, whose `tb-focus` has one. `-83`, `-84` and `-86` to `-90` were deleted (2026-10-14).

`missing_or_wrong_engage_records_are_reported` damages `-91/tb-focus` 71 ways (64 cases, and 7 on its
provoke) (entries, checks, every leaf, the nested run and its callers, the count write and its caller, the
`NextCommandTime` writes, the initial state, the provokes, the observers, the environment); each is reported. The
regression (`out-slice-e`) is identical to `out-slice-d`.

Not exercised by a capture: the provoked attacks of opportunity themselves (nobody engaged the archer), getting up,
the forced attacks (facts), the mobility checks, `AttackOfOpportunityLimits`, a target forced to move, tactical combat
(all reported), a unit constructed in the window (its count adopted at its first tick).

**Reviews:** gpt-6.1-sol (high, Fast mode), five rounds, every finding confirmed against the code and fixed. Round 1: provoke calls matched one by one, the disengage
immunity the sim's, the initial state complete before the first tick. Round 2: provokes left after the last tick
compared at the run's end; a provoke accepted only inside its rule's handler; the dictionaries' used slots recorded
(`eu_slots`, `eb_slots`: a freed slot keeps its place, so empty keys aren't a fresh layout; re-recorded as `-85`); the
count write's caller checked. Round 3: each provoke tied to the rule whose handler made it (at most one a rule); every
frame of the nested run's and the count write's caller chains; the damage test on a fight with an attack of
opportunity. Round 4: an additional limb's threat item bound to its recorded id (as the commands' reads). Round 5 found nothing (residual gaps: the branches no capture exercises, above).

## Slice F: design (2026-10-14, sixteenth session)

`UnitCombatState.LastTarget` and `ManualTarget` become the sim's (`combat/targets.rs`), so the AI's and the
engagement's `last`/`manual` reads become checks, and so do `UpdateCombatTarget`'s `NextCommandTime` resets (`ai_nct`,
276 of them in `tb-nearest`: 5198's follow moves). Neither field is saved; a unit's constructor clears both.

The writers [C]: `UnitCommands.UpdateCombatTarget` (from `Run` when the command is a standard one or the standard slot is
empty, and from `TryUseSpecialHandlers`' merge, on the merged-into command); `ShouldAttackOnDisengage` (turn-based, not
simulated: `LastTarget = target`, and its `ScopedVariableModification` restoring the old value on every exit);
`UnitCombatState.Clear()` (leaving combat, the constructor); `HandleEntityDestruction` (a destroyed `ManualTarget`);
the UI's (`ClickUnitHandler.OnClick` after its `Run`, which WrathOracle's driver does as well, `Stop`,
`TryCancelCommands`, `SelectionManagerBase.InterruptActions`); touch spells (`TouchSpellsController`, `UnitPartTouch`,
`RemoveUnitPartTouchOnAllCompanions`) and tactical combat. `LastTarget`'s setter raises `ICombatTargetChangeHandler`
(no implementers); `ManualTarget`'s, set to null, clears the rider's and the mount's (mounts: reported).

`UpdateCombatTarget(cmd)`: nothing for a `UnitUseAbility` (reported: not modelled) or in tactical combat; the target
unit (`cmd.Target?.Unit`) when it isn't the executor, is conscious (or the executor's AI is off: `IsAIEnabled`) and the
executor `CanAttack` it: `LastTarget` is it, `ManualTarget` too for the player's attack (`AiAction == null &&
MarkManualTarget`: only `UnitAttack` marks), else `ManualTarget` cleared unless it's that unit; otherwise
`ManualTarget` cleared and, for a command not the AI's, `NextCommandTime = 0`.

The sim: each unit's `LastTarget`/`ManualTarget` (known from `tgt_init` or constructed in the window); `UpdateCombatTarget`
ported in `Run` and the merge, `Clear()`'s and `ShouldAttackOnDisengage`'s writes, `CanAttack` the sim's
(`groups::can_attack`), `IsAIEnabled` and `TacticalCombatHelper.IsActive` leaves. Each write that changes a field is
queued and checked, in order, against the recorded writes (`tgt_w`) of the sim's sites; a recorded write of another site
is an input (the UI's, the driver's, a destruction's) or reported (touch spells, tactical combat, an unknown caller). A
sim write the recording doesn't have is reported at the next frame. The AI's and the engagement's reads (`manual`,
`last`) are checked against the sim's fields.

WrathOracle (`CommandObsTarget.cs`, observer `targets`, which requires `engage`):
- `tgt_init` at the window's first tick: each unit's `LastTarget` and `ManualTarget`.
- `tgt_w`: every write that changes either field (a prefix on each setter, comparing references as the setters do:
  unit, field, value), with its writer's call chain (`origin`, the setter first); the driver's own write is
  `WrathOracle.TurnBased.IssueHovered`. The replay names the writer from the chain: the sim's (`UpdateCombatTarget`
  from `Run` or `TryUseSpecialHandlers`, `ShouldAttackOnDisengage` from `Disengage`, its scope's lambda from
  `ScopedVariableModification<UnitEntityData>.Dispose`, `Clear` from `LeaveCombat`), an input's (`ClickUnitHandler.OnClick`
  and the driver, `Stop`, `TryCancelCommands`, `InterruptActions`' lambda, `HandleEntityDestruction`), or another
  (reported).
- `UpdateCombatTarget` is base lifecycle code; its leaves `tact` and `aien` (`uc_v`, with the command).

## Slice F: result (2026-10-14, sixteenth session) [M]

On **`poc5-tb-92`** (idle, Manual_18; `tb-nearest` and `tb-focus`, record and two replays) **every run is valid with
units' `LastTarget` and `ManualTarget` the sim's**: T0-equal, end state 1.0000, nothing reported. `tb-nearest`: 16
writes (`UpdateCombatTarget`'s and leaving combat's `Clear`), each checked, 123 reads (the AI's `manual`/`last`, the
engagement's) checked against the sim's fields; `tb-focus`: 20 writes (a disengage's `ShouldAttackOnDisengage` write
and its scope's restore among them), 58 reads, and its 403 `UpdateCombatTarget` resets of `NextCommandTime` (5198's
follow moves) are checks now. The writes that follow an entry from outside the lifecycle are the sim's too: the turn
controller's predictions click (`ClickUnitHandler.OnClick` after each `Run`: `ManualTarget` the clicked unit) and
combat's end (`TryCancelCommands` after its `InterruptAll`: both cleared) both run in these fights, and the driver
writes `ManualTarget` after its click's `Run` (its `tbattack`); none of those changed anything here, so they're
computed and found unchanged. Every unit's targets start null at the window (`tgt_init`).

How the writes are checked: one the sim makes takes the next `tgt_w` where the sim makes it (nothing recorded between), its call chain exactly
the setter, the writer's frames and the chain it's checked against (the innermost `Run`'s recorded chain, a stack of
the `Run` records the sim took; the engagement tick's; the entry's for a click's, `TryCancelCommands`' or `Stop`'s;
the driver's own). `Clear`'s writes (leaving combat, outside entries) are queued and accepted only inside that unit's
`Clear` (after its `cd_clear`, before the next lifecycle record), their chain the `leave_combat`'s callers and the
one the `InterruptAiCommands` that `UnitEntityData.LeaveCombat` calls next has. `InterruptActions`' writes and a
destroyed target's aren't modelled (reported; the replay reports a destruction anyway). The driver's inputs that run a command
(`tbattack`, the plan's `move` and `attack:`) pair one-to-one with the `Run` right after them (only the new command's
`uc_new` between), which has the driver's coroutine origin, its frame, and its command (an attack on the target, a
move to the point); each input has exactly its writer's fields (a click's round the sim's, its frame the tick's, its turn count the window's, its `at` its writer's fields, an `attack:` step's
target of its blueprint). Nothing else is recorded inside a `Clear` between `cd_clear` and its target writes. `tgt_init` is one block
right after `eng_init`'s, over the same units in order, and `eng_init`'s comes right after `uc_snap` in ordinal id
order (the oracle's `StateDump.AllUnits`). The targets observer requires `engage` and `cooldowns`.

`missing_or_wrong_target_records_are_reported` damages `-92/tb-focus` 85 ways (the initial block, every field of a
write, its chain, reorders across lifecycle records with the sequence renumbered, the restore, `Clear`'s window and
chain, a `Stop` without its clears, the driver's inputs and their `Run`s, the initial blocks' sequence, writes the sim didn't make, the reads, `UpdateCombatTarget`'s leaves and resets, the observers); each is reported. The regression
(`out-slice-f`) is identical to `out-slice-e`; the older captures replay as before.

Not exercised by a capture: a click, `Stop` or `TryCancelCommands` write that changes a field, a merge's `UpdateCombatTarget`, an unconscious target (the
`aien` leaf), a `UnitUseAbility` (reported), mounts (reported), touch spells and tactical combat (reported).

**Reviews:** gpt-6.1-sol (high, Fast mode), eleven rounds, every finding confirmed against the code and fixed. Round
1: each write pulled where the sim makes it (it had been matched in order among themselves), its caller chain checked,
the merge its own site, the input writers held to their writer's semantics. Round 2: every frame between a write's
site and its entry exact (the innermost `Run`'s recorded chain), `Clear`'s writes inside their `Clear` and from its
`leave_combat`'s callers, the initial block contiguous and in order. Round 3: the UI path's writes computed rather
than accepted (the predictions' `OnClick`, combat end's `TryCancelCommands`, `Stop`, the driver's), `InterruptActions`'
and a destruction's reported, `Clear`'s whole chain via the `InterruptAiCommands` that follows, `eng_init` not
reopened. Round 4: the driver's click bound to its own `Run`, the restore's frames exact. Round 5: the driver's inputs
typed with their payloads, the initial blocks in the oracle's fixed sequence. Round 6: every input field, the `attack:`
step's blueprint, the input right before its command's first sight, nothing else inside a `Clear`. Round 7: nothing
recorded between the sim's consecutive writes. Round 8: `UpdateCombatTarget`'s leaves and reset taken strictly, a
click's frame, turn count and `at`. Rounds 9 and 10: the reset's own writer, float32 vectors, a C# `int` count. Round
11 found nothing (residual gaps: the paths no capture exercises, above). Left as is: the other slices' leaves are
still pulled with records processed on the way (a reordered presentation draw between them isn't reported).

## Slice G: design (2026-10-14, sixteenth session)

The AI reads the impatience controller's state as inputs: `ipt` (`CombatAiData.ImpatiencePriorityTarget`) and
`disliked` (`DislikedTargets.Contains`). Slice G ports `UnitImpatienceController` so both become checks [C]:
- **State** (`CombatAiData`, per unit, cleared by `UnitCombatState.Clear`): `HardTargets`, `DislikedTargets`,
  `BotherUnits` (.NET `HashSet`s: `SetupPriorityTarget` enumerates `BotherUnits`, so its slot order matters, as the
  engagement's dictionaries' did: `csharp/dict.rs`), `ConsequentAttacksWithoutEffect` (a list of commands),
  `LastOffensiveStandardActionTarget`, `ImpatiencePriorityTarget`, `ConsequentInefficientAttacksCount`. None is saved:
  the window's initial state is recorded (as `eng_init`, `tgt_init`).
- **Tick** (registered right before `AiBrainController`, so a phase before the AI's): in combat, for each awake unit in
  combat, `UpdatePriorityTarget`, then a priority target that is unconscious or not visible in the unit's memory
  (`Memory.ContainsVisible`: an input) is set up again. `TurnController.Prepare` calls `UpdatePriorityTarget(Rider)`
  before its forced AI tick.
- **`UpdatePriorityTarget`**: the count of the consequent attacks without effect whose command succeeded and has no
  projectile on the way (`LaunchedProjectiles`' `OnTheWay`: the sim's pending projectiles, or an input); against the
  threshold of the unit's `Impatience` (`UnitState.Impatience` from `ChangeImpatience`; the thresholds and switch
  chances from `BlueprintRoot.SystemMechanics.Impatience`); a hard target switched on `Random.Range(0, 100)` (a draw
  from the tape where the sim draws) to disliked, then `SetupPriorityTarget`: a weighted pick over the bother units
  (`1 / DistanceTo`: an input) with `Random.Range(0f, sum)` when the sum is at least 1.
- **Handlers**: `HandleUnitCommandDidStart` (the commands model's `Start`: a standard `UnitAttack` on an enemy appends
  to the attacks without effect, resetting them on a new target); `RuleDealDamage`'s `OnEventDidTrigger` (damage of 1
  or more on an enemy clears the attacks up to the one whose attack roll or projectile it is, then adds the initiator
  as a bother unit); heals (`RuleHealDamage`, `RuleHealStatDamage`, `RuleHealEnergyDrain`); `RuleSummonUnit` (reported
  until summons are ported); `HandleUnitMakeOffensiveAction` (raised by `OffensiveActionsController` on
  `RuleAttackRoll`); `HandleUnitDeath`/`HandleUnitDestroyed` (the unit leaves every awake unit's bother units: today a
  stub in `combat/life.rs`). `TryAddBotherUnit` reads `IsInCombat`, `IsEnemy` and `Memory.ContainsVisible`.
- **WrathOracle** (observer `impatience`, requiring `targets`): the controller's tick as an entry; its reads the sim
  can't compute as leaves (`ContainsVisible`, `DistanceTo`, projectiles on the way); checks of each handler's effect
  and of `UpdatePriorityTarget`/`SetupPriorityTarget`'s results; the initial state at the window's first tick.
- **Fixture**: the current fights may not reach a hard target (the threshold of misses); a fight where an enemy keeps
  missing one target would exercise the switch draw. Check what `-92` exercises before recording more.

## Slice G: result (2026-10-14, sixteenth session) [M]

On **`poc5-tb-95`** (idle, Manual_18; `tb-nearest` and `tb-focus`, record and two replays) **every run is valid with
the impatience controller computed** (`combat/impatience.rs`, `combat/replay/impatience.rs`; WrathOracle
`CommandObsImpatience.cs`, observer `impatience`): `tb-focus` runs 2,631 entries (the controller's ticks, their
`UpdatePriorityTarget`s and `TurnController.Prepare`'s), checks 13,968 states after `UpdatePriorityTarget` and matches
50 handler records (command starts, bother units from `OffensiveActionsController`'s offensive actions and from
damage, the damage handler's attacks without effect, deaths). The AI's `ipt` and `disliked` reads are checks. Units'
sets are .NET `HashSet`s (`csharp/hashset.rs`: the reference source's, whose `Remove` of the last entry resets the
set, as the recorded slot counts show); a damage rule carries its attack roll's sequence number
(`AttackLink.roll_seq`), and each `UnitAttack` its rolls, which the damage handler matches.

How it's checked: the controller's entries (`uc_imp`: the tick from `GameMode.Tick`, placed in a new phase before the
AI's; `UpdatePriorityTarget` from `Prepare`, exactly one of its rider before its `ForceTick`) pull their leaves
(`ipic`, `ivis`, `idist`, `iotw`, `ilvl`), nested entries, draws and states strictly as the next record (nothing else
is recorded in the controller's code). The handlers run inside other code, so each queues its operation; the replay
applies the queue in order as the handlers' `imp_s` records come (`TryAddBotherUnit`'s visibility read is its
record's), inside a rule's span after the sim fires the rule (`span_ok` binds each record to its handler, rule and
units). A command start's record must come before the next lifecycle record. Leaving combat's `CombatAiData.Clear` and
a death's removals go through the queue too. The awake units of a tick are checked against the sim's as far as it
knows them. `imp_init` is one block right after `tgt_init`'s. The tick's call chain is exactly `GameMode.Tick`'s;
`Prepare`'s `UpdatePriorityTarget` has its `ForceTick`'s. Inside any rule's span (a command's attack or a projectile's
resolve) each record must be of its frame and inside the did handler of its rule, bound to the rule's units (and the
damage handler's to its id). A draw is the tape's, its recorded value checked; one outside the controller is reported.
A destruction's handler records are reported (the replay doesn't model destruction). A rule handler's record
outside its rule's span, a fired rule's handler record missing from it, and the controller's own records (entries,
leaves, draws) inside a rule are reported. A tick's awake units follow the state's units' order. A command start's record comes before WrathOracle's `cmd` note
of the start; a death's records are one batch where its notification is (after as many compared mutations as the sim
made before it: `GiveExperience`'s come first), done before the unit's `SetLifeState` goes on; `Prepare`'s
`UpdatePriorityTarget` comes after its interrupts and its cooldowns' clear.

A bother unit's record must be in the handler that queued it (the damage handler's or an offensive action's). Any
record of the impatience observer's family (observations, entries, leaves, draws) in a recording without the observer
is reported, in a rule's span or outside (the targets test has those cases on `-92`). `missing_or_wrong_impatience_records_are_reported` damages `-95/tb-focus` 57 ways; each is reported. The regression
(`out-slice-g`) is identical to `out-slice-f`; the older captures replay as before.

Not exercised by a capture: a hard target and its switch draw (three consecutive attacks without effect on one
target), `SetupPriorityTarget` (so `ivis`, `idist` and the setup draw), an impatience level other than normal,
abilities, heals and summons (reported), mounts (reported).


**Reviews:** gpt-6.1-sol (high, Fast mode), seven rounds, every finding confirmed against the code and fixed. Round
1: records in projectile resolves validated, a stray draw reported, draws' values checked against the tape, the entries'
whole chains, a destruction reported. Round 2: a rule handler's record only inside its rule's span (and a fired rule's
all there), the controller's own records never inside a rule, the awake units' order. Round 3: a command start's
record before its `cmd` note, a death's batch together and before `SetLifeState` goes on, `Prepare`'s update after its
clear. Round 4: no impatience record accepted without the observer, a death's batch at its notification (by compared
mutations). Rounds 5 and 6: the whole record family without the observer, in spans and out; a bother unit's record in
the handler that queued it. Round 7 found nothing (residual gaps: the paths no capture exercises, above).
## A third fight: `poc5-tb-96` (2026-10-14, seventeenth session) [M]

`poc5-tb-96` (idle, Manual_18, current WrathOracle with every observer) records one fight, **`tb-hold`**
(`oracle/tools/oracle-fixtures/poc5-tb3.spec.json`): the party moves as in `tb-focus`, then focuses 51DD, 523B, 51FC
in that order (the reverse of `tb-focus`), and the archer 5198 ends each of its turns without acting. The record and
both replays are clean (no unhooked draw), and **all three runs are valid** in the POC-5 suite with every slice
computed and every assertion `-95`'s runs pass (`driven_replay_equals_the_recordings` takes each capture's fights from
`FIGHTS`). With `-95`'s `tb-nearest` and `tb-focus` that is three recorded fights with different choices, each
T0-identical with an end state equal to the oracle's on every run: POC-5's pass as worded, given the inputs listed in
PLAN (movement agent and pathing, group memory and visibility, animation, the Unity clock, the UI's requests).

Not exercised by this fight either: an impatience hard target (`hard` and `ipt` stay empty in every `imp_s`), so the
switch and setup draws and `SetupPriorityTarget` are still untested by a capture.
