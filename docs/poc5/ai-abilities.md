# POC-5: the AI's ability actions (stream P)

Written 2026-10-15 (stream P, twenty-second session). Tags: **[M]** measured, **[C]** read in the
decompiled C# (`decompiled/`, never copied here), **[I]** inferred. No game text here: GUIDs, type
names and counts only.

## What `poc5-tb-121/tb-lizard` needed [M]

The ranged giant centipede 5031 (`CR0.25_GiantCentipedeRanged_NoPoison`, spawned by the scene: its
brain is the blueprint's) has two AI actions: `GiantCentipedeSpitAbilityAiAction`
(`14db26a8…`, a `BlueprintAiCastSpell`, base score 2) and an attack (`866ffa6c…`, base score 1). Its
spit (`GiantCentipedeSpitAbility`, `d7290891…`) is an `Ability` fact from the blueprint's
`m_AddFacts`: medium range (40 ft), a projectile delivery with an attack roll, acid damage. The sim's
brain kept the cast action as `None` (the cast spell's `ShouldBeInActionsList` wasn't ported), so
both of the centipede's turn-based decisions reported "an AI action other than AiActionSimple", and
the `UnitAttack` each decision then runs was missing (the running list, `Commands.Empty` and the
turn's running commands, four more kinds).

What the game did, from the recording [M]:

- **Decision 1** (t=6091, `TurnController.Prepare`'s `ForceTick`): the spit's range 12.192, its
  `ScoreActor` 2 (not engaged: `ThreatedByConsideration` 1), its `ScoreTarget` on the PC 0 (distance
  14.667: `DistanceConsideration` 0 beyond 8 m), no path scored, no impatience reads (the spit isn't
  `m_AffectedByImpatience`); then the attack, chosen.
- **Decision 2** (t=6281): the spit's `ScoreActor` 0 (engaged by the PC: `ThreatedByConsideration`
  0); then the attack, chosen.

The spit is never chosen in this fight: the AI's cast command is not exercised by a capture.

## Ported [C]

`crates/wr-sim/src/combat/ai.rs`:

- **The brain.** `RestoreAvailableActions` of a new brain lists a `BlueprintAiCastSpell` only when
  `ShouldBeInActionsList(unit)` (`!IsExpended(GetAbilityForUse, GetSpellbookForUse)`): the unit has
  the ability as a fact (`AbilityCollection.GetAbility`: the first `Ability` fact of the blueprint),
  or, without one, a spellbook that can spend it. No fact and no spellbook: left out. No fact but a
  spellbook (`Spellbook.CanSpend(BlueprintAbility)` isn't ported): kept as not established,
  reported when the AI reaches it. A random variant with the fact: `GetAbilityForUse` draws while
  the brain is made, reported then. A saved brain's cast actions are its `AiActionSimple`s, as
  before.
- **`BlueprintAiCastSpell`** as an action (`action`): the ability, `m_ForceTargetSelf`,
  `m_ForceTargetEnemy`, `m_TargetPointUnderTarget`, `m_DeadTargetType`, `m_AffectedByImpatience`.
  Reported: a variant (fixed or random), locators, no ability; cooldowns, combat counts,
  `AdditionalBehaviour` and action lists as for an attack.
- **`InitContext`**: the action's `Data.AbilityData` (`new AbilityData(fact)`), made the first time
  it is null and kept on the action until `ClearCombatInfo` (`UnitCombatState.Clear`, leaving
  combat: `ai::clear_combat_info`). `ActionData` isn't saved and a new brain's actions are new
  objects, so every cache is empty at the load. Without a fact the spellbook's `AbilityData` is
  reported (when the unit has a spellbook); without either the context's `Ability` is null.
- **`GetRange`**: `Ability.GetRange()` in meters (`feet * 0.3048f`) for close, medium, long,
  personal and custom ranges, the recorded `range` leaf checked against it; touch, double move and
  weapon ranges read the unit, so their recorded value is an input, as an attack's reach is; a
  projectile range and an unlimited one (infinite: the replay's leaves are finite) are reported.
- **`ScoreActor`**: `IsAvailable` (the context's `AbilityData.IsAvailable`, `combat::spells`), then
  `IsExpended` (no `AbilityData`: the action goes to `ExpendedActions`, which `SelectAction` removes
  from the brain's `AvailableActions`), the actor considerations, then in turn-based combat the
  full-round check (`RequireFullRoundAction` of a subject neither surprising nor with a full-round
  action: score 0; the sim's cooldowns).
- **`GetTargetType`/`GetDeadTargetType`**: a forced enemy target, alive only. Reported: `Self`
  (`m_ForceTargetSelf`), an unforced target type (the ability's anchor and targets), dead targets.
- **`IsTargetValid`**: the static `IsTargetValid(Ability, unit, target)`: a minimum range
  (`IAbilityMinRangeProvider`: reported) and the ability's target restrictions
  (`spells::target_restrictions_passed`: `AbilityTargetHasFact` ported, the rest reported).
- **`IsAffectedByImpatience`**: an attack's always, a cast's by its blueprint: the `disliked` and
  `ipt` reads only then.
- **Considerations**: `ThreatedByConsideration` (`EngagedBy.Count` of `Target.Unit ?? Unit`: the
  engagement model's; reported without it) and `DistanceConsideration` (the target's distance; 0
  without a target), with Unity's `Mathf.InverseLerp`, `Lerp`, `LerpUnclamped` and `Clamp01` in
  Mono's double evaluation.
- **`CreateCommand`** (not exercised): an available ability's
  `UnitUseAbility.CreateCastCommand(ability, target)`: `new UnitUseAbility(RuntimeActionType,
  ability, target)`, the command described by the recording's `uc_new` checked as the AI's (class,
  type, target, the constructor's approach with `NeedLoS` from `ILineOfSightIgnore`), its `Ability`
  the context's (`uc_new` doesn't name the ability: it is the checked AI action's).
  `NextCommandTime += 0.3f + Random.Range(0f, 0.1f)` only for a command neither free nor swift. A `CanKeepCommand` of a cast in real time is the base's (true). Reported: a missing or
  unavailable ability (the `UnitMoveTo` fallback), a point target, a touch part or a magus' part.

`crates/wr-sim/src/combat/spells.rs` (stream K's): `Ability` is now an `AbilityData` from a
memorizing spellbook's slot or from an ability fact (`Source`). For a fact's: `IsAvailable` (no
spellbook: available in it), `CanBeCastByCaster` (no charges and no resources, the fact active, no
forbidden spellbooks check), `ActionType`, `RuntimeActionType`, `RequireFullRoundAction`,
`CanTarget`'s `ForceTargetOwner` (false). What a fact's `AbilityData` reads that the sim doesn't keep
is reported (`fact_state`): a source item, `StickyTouch` and the fact's `Spellbook` (only
`UnitPartTouch` sets them), `UsagesPerDayResource` (only `AddSpellListAsAbilities`), and a saved
fact's `UsagesPerDayResource` or `StickyTouch` (the loader marks it: `Fact::ability_saved_extras`).
The cast itself
of a fact's ability is reported where it begins (`RuleCalculateAbilityParams` of an `AbilityData`
without a spellbook, and the spit's projectile delivery: `UNPORTED_CAST`).

## What a chosen spit would still need [C]

If the AI chose the spit, its command would run through the commands model and stream K's lifecycle
checks (`IsAvailableForCast`, `CanCast`, `ShouldRestrictCommand`), then report at `OnAction`: the
cast's params (`AbilityData.CalculateParams` of an ability fact: caster level and DC from the
fact's context, not a spellbook), the delivery (`AbilityDeliverProjectile`: the projectile, its
ranged touch attack roll), and the damage action's context. A fixture where the centipede spits:
its spit's distance consideration is 1 up to 7.5 m, falls linearly to 0 at 8 m, and its threat
consideration is 0 once a unit engages it, so a target closer than about 8 m to a centipede that
nobody engages, scoring above the attack's 1 after the path factor [I].

## Results [M]

(Filled in below.)

## Tests

- `combat::ai::tests`: `Mathf`'s lerps, the cast range's meters, decimal scales of a cast's scores.
- `tests/poc5_spells.rs`: `poc5-tb-121` valid on every run; damaged records of the cast action's
  scoring, each reported.
