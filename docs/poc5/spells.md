# POC-5: spellcasting and the giant fly's facts (stream K)

Written 2026-10-15 (stream K, twenty-second session). Tags: **[M]** measured, **[C]** read in the
decompiled C# (`decompiled/`, never copied here), **[I]** inferred. No game text here: GUIDs, type
names and counts only.

## Headline

| Fixture | Result |
|---|---|
| `poc5-tb-120/tb-cast` (Camelia casts Enlarge Person on Seelah from a memorized slot, the driver's `tbcast`) | **Every run valid** (record, replay-1, replay-2) [M]: T0-equal (558 records), end state 1.0000 (the spent slot, Seelah Large with her size, Strength and Dexterity modifiers), nothing reported, every combat log message the sim's (26, with the cast's) |
| `poc5-tb-118/tb-chain` (a second fight, the giant fly) | Every run T0-equal, end state 1.0000; the fly's two facts' handlers are the sim's [M]. Not valid: one combat log event, the fly revealed by fog of war (`GameLogEventKnowledge`), isn't recorded; the new observer `inspect_reveal` records it (`poc5-tb-123`, to record; below) |
| The captures to keep valid (`-95`, `-96`, `-109`, `-111` to `-117`) | Still valid on every run [M] |

## The giant fly (`tb-chain`) [C]

`crates/wr-sim/src/combat/components.rs`:

- **`AttackBonusConditional`** (`Airborne`): before an attack roll its conditions run in the fact's
  context, scoped on the attack's target (on the wielder with `CheckWielder`); when they hold,
  `Data.AttackBonus = rank × Bonus` and `Data.Target` are kept, the attack bonus rule against that
  target adds it (`AddModifier(bonus, Fact, Descriptor)`), and the roll's end clears them. The
  runtime data isn't saved (the save holds the component as null); another shape is reported.
  Its condition (`ContextConditionHasFact`) is ported in `elements/conditions.rs`.
- **`AddInitiatorAttackWithWeaponTrigger`** (`GiantFly_Feature_Disease`): `AbstractWeaponTrigger`'s
  `IsSuitable` in the C#'s order (the roll's outcome now kept on `RuleAttackWithWeapon`:
  `roll_outcome`, `resolves_triggered`), including the `Ensure<UnitPartDamageGrace>` its last
  operand makes. A suitable attack's actions (they run in a `ContextAttackData` scope) and a
  pending resolve's `OnResolve` subscription are reported: the fly's one bite missed, so nothing ran.
- `ContextConditionTargetIsYourself` and `ContextConditionCasterHasFact` (the latter was ported but
  not listed) are in `conditions::IMPLEMENTED` too.

What's left in `tb-chain` [M]: at t=2347 the game's `InspectUnitsController` makes a knowledge check
of the fly (`TryMakeKnowledgeCheck`, which queues `GameLogEventKnowledge`), because fog of war
revealed it (`EntityDataBase.IsRevealed` raises `IEntityRevealedHandler.HandleEntityRevealed`). The
sim doesn't run fog of war and `poc5-tb-118` has no record of the reveal, so the event isn't made
and every later log event is one place off (21 combat log kinds, nothing else).

The reveal is now an input: WrathOracle's observer `inspect_reveal` (`InspectObs.cs`) writes one
`insp_reveal` record per `HandleEntityRevealed` call (the entity, whether it is a unit, whether the
call added it to the controller's set, the caller chain). The driven replay
(`replay/spells.rs::inspect_reveal`) places it by its caller (`FogOfWarController` or
`EntityVisibilityForPlayerController`: the new `Phase::Visibility`, after `Move`, before
`Sleeping`), runs the sim's `HandleEntityRevealed` (`inspect::entity_revealed`: queued without a
`UnitInfo` of its blueprint; a polymorph's inspection blueprint reported) and compares its queuing
with the record's. A record without the observer declared, of another shape or tick, from a caller
not modelled, or of a unit the sim doesn't have is reported. The capture with it is `poc5-tb-123`
(to record). [I] As a diagnostic only (not a result), a scratch copy of `tb-chain`'s record run
with one such record put after t=2347's vision records replays valid.

## Spellcasting (`tb-cast`)

### What the game does [C]

1. The driver selects the spell (`ClickWithSelectedAbilityHandler.SetAbility`: a prediction
   `UnitUseAbility` in a temporary set, interrupted at once), then runs the cast command
   (`UnitUseAbility.CreateCastCommand`, `Run`): `UpdateCombatTarget` of a `UnitUseAbility` (no
   auto-use ability on the brain), its class's `Init`/`OnRun` (recorded class code).
2. `UnitCommandController`: before it starts, `TickCommandTurnBased` interrupts a `UnitUseAbility`
   whose ability isn't `IsAvailableForCast`; `HasCooldownForCommand` restricts a full-round ability
   (`ShouldRestrictCommand`: Enlarge Person is `m_IsFullRoundAction`) to a unit with a full-round
   action; while it runs, `TickCommand` interrupts it when the caster can't cast (`CanCast`:
   `UseAbilityForbidden`).
3. `UnitUseAbility.OnAction` (class code): `new RuleCastSpell(Ability, Target)` makes the execution
   context (`AbilityData.CreateExecutionContext` → `CalculateParams` → a root
   `RuleCalculateAbilityParams`: caster level from the spellbook, the spell's level, DC 10 + level +
   the casting stat's bonus, concentration), then triggers it: a nested `RuleApplySpell(Before)`,
   `CanTarget`, no spell failure, no arcane failure (a divine spellbook), success and a new
   `AbilityExecutionProcess`; then `Ability.Spend()` (the memorized slot spent).
4. `AbilityExecutionController` (after `FxSpawnController`, before `UnitLifeController`): the
   process's routine runs in its first tick (no delivery): `PrepareCast`, `DoApplyEffect` (a
   `RuleSpellResistanceCheck` for a target other than the caster: it rolls a d20 even without
   spell resistance), `AbilityEffectRunAction`'s actions in the context's data scope on the target
   (`Conditional` on `ContextConditionHasFact` and `ContextConditionTargetIsYourself`, then
   `ContextActionApplyBuff`: the duration `1 minute × rank` through `ContextDurationValue` and a
   `RuleRollDice` of zero dice, the buff's context cloned from the ability's), `RuleApplySpell(After)`
   for the target, then without one.
5. The buff: `ChangeUnitSize` (`OnActivate`: `UnitPartSizeModifier.Add` → `State.Size` = Large →
   `UpdateSizeModifiers`) and two `AddGenericStatBonus` (Strength +2, Dexterity −2, size bonuses).
6. `UnitUseAbility.OnEnded` (the command's end) calls the base `OnEnded` first, then presentation.

### Ported [C]

- `crates/wr-sim/src/combat/spells.rs`: `Ability` (a memorizing spellbook's spell: the slot's
  `SpellShell`), `IsAvailableForCast` (`CanBeCastByCaster`: the cast count from the spellbook's
  slots, the conditions, and every branch that reads what isn't modelled reported: restrictions,
  caster restrictions, required parameters, material components, mounts, polymorph (a fact with a
  `Polymorph` component: `Body.IsPolymorphed` isn't modelled; the game has no polymorph part, and
  `RuleCalculateWeaponSizeBonus`'s check now uses this too), forbidden spellbooks, suppressed
  schools, ability modifiers, a caster's `UnitPartSpecialMetamagic`, which `HasMetamagic(Quicken)`
  reads for the action type), `IsAvailable`, `CanTarget` (target
  restrictions: `AbilityTargetHasFact` ported, the others reported; the anchor), `RuntimeActionType`,
  `RequireFullRoundAction`, the execution context (params, ranks = caster level, the spell's
  descriptor and its source ability for the contexts made under it), `RuleCastSpell`,
  `RuleApplySpell`, `RuleSpellResistanceCheck` (no spell resistance part: reported when present),
  `AbilityExecutionController`'s tick for a spell without delivery, `AbilityEffectRunAction`
  (no saving throw), `ContextDurationValue.Calculate` (also for facts' contexts: a
  `ContextActionApplyBuff` with a duration value now runs, where it used to be reported), and
  `AbilityData.Spend` of a spellbook slot. Spells whose blueprint has a component of an unported
  kind (deliveries, area selections, sticky touch, kineticist, custom logic, variants, params
  calculators, rank configs, shared values, properties, spell type overrides) are reported when
  the cast begins.
- `rules/ability_params.rs`: `RuleCalculateAbilityParams` for an `AbilityData` of a spellbook
  (`EffectiveCasterLevel`, `m_SpellLevel`, the casting attribute, the standalone mythic DC,
  `m_SpellSource`); its T0 result includes the computed `IsChanged`.
- `spellbook.rs`: `GetAvailableForCastSpellCount`, `SpendInternal` (`CanSpend`, `Spend`, with
  linked opposition slots); a spell level limit (`AddSpellLevelLimit`) is reported.
- `components/stats.rs`: `ChangeUnitSize` and `AddGenericStatBonus` (a size part from the save, whose facts aren't
  hydrated, is reported wherever the size is read: `Unit::size_part_from_save`); `CalculatePowerfulChangeBonus`
  now evaluated for contexts made under an ability (it was reported). `spawn::state_size` is
  `UnitState.Size` (`m_Size`, read lazily from `OriginalSize`), its setter runs
  `UpdateSizeModifiers` (which reads it now, not `OriginalSize`), `UnitPartSizeModifier.UpdateSize`;
  `weapon::unit_size` reads it (a size part was reported).
- `combat/commands.rs`: the lifecycle's ability checks (`IsAvailableForCast`, `CanCast`,
  `ShouldRestrictCommand`, after which the command type's cooldown is checked as for any other), `UpdateCombatTarget` of a `UnitUseAbility` (`MarkManualTarget`: a
  sticky touch; a held touch or an auto-use ability reported), `UnitUseAbility.OnEnded` (base
  first; its running-list call and end are recorded as class code's, `ctx` other, and checked as
  such: `Inputs::class_running_call`, `class_ended`).
- `combat/impatience.rs`: an ability's start on a target that isn't an enemy doesn't count (on an
  enemy it is reported: its damage is matched by its process's context).
- The combat log: `RulebookCastSpellLogThread` (`BaseUseAbilityLogThread.HandleUseAbility`: the
  spellbook spell's `CastSpell` message, `Text` and `Description` the spell's, `Tooltip` the ability;
  its `TooltipTemplateAbility` not made) and `RulebookSpellResistanceCheckLogThread` (no message
  without a resistance roll or an immunity; one with is reported).
- The rulebook: `RuleEvent::record_target` (the oracle writes `$impure` for `RuleCastSpell`'s and
  `RuleApplySpell`'s `GetRuleTarget`: a null target in T0).

### The driven replay [C]

- `combat/replay/targets.rs`: the driver's `tbcast` input (its fields exactly, a memorized slot, a
  standard action, its `at` a click's with the pointer's mode); the command its `Run` makes gets
  the input's spell (`Ability::memorized`) and is checked against it (`tgt_driver_after`).
- `combat/replay/spells.rs`: a root `RuleCalculateAbilityParams` from `RuleCastSpell`'s constructor
  in `UnitUseAbility.OnAction` begins the sim's cast for the running `UnitUseAbility` of its
  initiator (its snapshot checked: spell, spellbook and owner, level, source, no metamagic or
  overrides); the next root `RuleCastSpell` from `OnAction` is the sim's cast (its spell, target,
  cutscene and failure fields checked). Both are placed in the commands' phase. A root
  `RuleSpellResistanceCheck`, `RuleRollDice`, `RuleCanApplyBuff` or `RuleApplySpell` from
  `AbilityExecutionController.Tick` is the sim's own (placed in the new `Phase::Abilities`, which
  runs the processes' tick; one in a frame where the sim ticked none is reported); one of those
  classes from elsewhere is reported.
- `project.rs`: T0 compares `RuleCalculateAbilityParams.Result`, `RuleCastSpell`'s success and
  failure fields, `RuleSpellResistanceCheck`'s resistance, penetration, roll and immunity, and
  `RuleApplySpell`'s inputs (`Spell`, `Context`, `SpellTarget`, `Place`: the oracle writes its
  target as `$impure`, so they're what tells the cast's `Before` and the process's two `After`
  rules apart).

### Inputs that stay recorded

`UnitUseAbility`'s class code (`Init`, `OnRun`, `OnStart`, `OnTick`, `OnAction`,
`OnTickApproaching`) is still the recording's (`uc_fx`, "class code taken": 190 per run): its
`OnAction`'s own checks (`IsAvailable`, the target's state, the save and concentration checks of a
difficult cast) and the animation it waits for aren't the sim's; its rules are. The execution
context's `Direction` (positions) isn't kept (no element of this spell reads it).

### Tests

`crates/wr-sim/tests/poc5_spells.rs` (ignored, real data): every `tb-cast` run valid with the cast
computed (2 cast roots, 5 process roots), its end state equal; `tb-chain` T0-equal with only the
combat log's knowledge event left; 17 damaged `tb-cast` recordings (the params' and the cast's
snapshots, a missing params or cast root, the resistance check from another caller, a process root
of another class, the resistance d20, the params' DC, the buff's attach, four `RuleApplySpell`
inputs, the driver's spell and slot, the cast's end chain), each reported or T0-unequal.
`replay::tests`: a recorded reveal queues the unit as the game did; reveals the replay can't place
are reported. `facts::tests::an_apply_buff_with_a_context_duration_lasts_its_rounds`.

## Next

1. Check `tb-chain` with the reveal recorded (`poc5-tb-123`).
2. `UnitUseAbility`'s class code as the sim's (as slice B did for `UnitAttack`): taps for its
   reads (`CanTarget`'s range and line of sight aren't in `CanTarget` itself; the animation's
   precast and act), then `OnAction`'s checks and its spend.
3. Spells with deliveries (touch, projectiles), saving throws, spell resistance, area effects;
   spontaneous casters; metamagic.
