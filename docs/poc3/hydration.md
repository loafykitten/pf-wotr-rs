# POC-3 second round: hydrating Manual_13

Written 2026-10-11. Scope: PLAN.md's POC-3, second round: the `wr-sim` foundation
([`docs/sim/architecture.md`](../sim/architecture.md)) and hydrating Ember's Manual_13 with
it: the main character in `party.json` and the full units of the caves' area state,
Seelah, Anevia (and Lann, who is also there), through the post-load lifecycle, with the
character sheet compared against WrathOracle. Tags: **[M]** measured, **[C]** read in the
decompiled code, **[I]** inferred. Inputs read in place, read only. Outputs are under the
gitignored `cache/sim/`. No game text here: type names, GUIDs and counts.

## Headline

- **Character-sheet match [M]: the main character 507/507 fields (100%); all four units
  2,301/2,372 (97.0%)** against WrathOracle's `after-load` dump of Manual_13
  (`fixtures/captures/oracle/state-manual13-after-load.json`, recorder health clean, Unity
  RNG state unchanged by the dump; the earlier smoke-3 capture gives the same result). Every one of the 71 differences on Seelah, Lann and
  Anevia comes from one unported component, `DifficultyStatAdvancement` (difficulty stat
  scaling, which needs the `DifficultyPresetsList` ScriptableObject from
  `blueprint.assets`), and the sim **reports it**: those three units are flagged invalid
  for exactly that reason. Compared: HP, levels, every stat's base, permanent, modified,
  raw value, subclass extras and modifier list (descriptor, value, source fact, source
  component, item, in list order), and every fact's presence, order, blueprint, class, rank
  and active flag.
- **Fail closed works [M].** Before `AdvanceArmorStats` was ported, Anevia's three skills
  were off by one and the run reported the unported rulebook handler on exactly that
  unit; the main character's armor and shield modifiers were first lost to
  `CleanupModifiers` and the comparison showed it. Each was fixed by porting the C#.
- **Coverage [M]:** the four units' 288 facts carry 721 components of 131 types; 13 are
  fully ported, 8 partly, 61 have no lifecycle or handlers (data other code reads), and 49
  types are required and missing, 113 components on active facts. Over the whole save
  (all 4,386 facts in every document, 2,984 active) the ranking covers the 7,053
  components on active facts, 243 types: 141 required types missing (1,598 components on
  active facts). Lists: `cache/sim/Manual_13.manifest.json` and
  `Manual_13.manifest-whole-save.json`.
- **Manual_10 [M], a first measurement:** the same code hydrates Defender's Heart's 16
  units (2,498 facts) in about 0.3 s after decoding, and matches its oracle dump on
  **97.3% of compared fields** (3 units at 100%); details below.
- **Timing [M]** (release build, M2 Max): read and decode the save 21–30 ms, open the
  `.bbp` and the content pack 45–90 ms (all 223 blueprints the four units need came from
  the `.bbp`; none needed the zip), hydrate 13–15 ms.

## What hydrating runs

`wr-sim hydrate Manual_13` (`crates/wr-sim/src/hydrate/mod.rs`):

1. **Deserialization**, following what Newtonsoft's setters do [C]: units with parts,
   facts (`ComponentsDictionary` setter: null entries create runtimes only for enabled
   runtime providers; saved runtimes keep their `m_Data`), stats (`PersistentModifierList`
   setter refilling the descriptor dictionary in saved order and setting `AppliedTo`;
   `m_Dependents`; subclass members), items (with `m_Modifiers`, the dex limiter, shields'
   armor and weapon components), contexts (`[OnDeserialized]`: a missing caster is the
   owner), proficiencies. Absent members take their C# initializers (`Rank` 1,
   `IgnorePrerequisites` true…). `$ref`s resolve within each document; shared objects map
   to one sim object.
2. **`Player.PostLoad` → `CrossSceneState.PostLoad`** with its barriers: every party
   entity's `PrePostLoad`, then `PostLoad`, then `ApplyPostLoadFixes`. Per unit:
   `UnitEntityData.OnPostLoad` → `UnitDescriptor.PostLoad` → `CharacterStats.PostLoad`
   (bind each stat with its `OnPostLoad`, then `UpdateValue` all), *then* the unit's facts
   (`PostLoad` + `PostLoadComponents`), then parts. `UnitDescriptor.PostLoad` also runs
   `Body.PostLoad`, which post-loads each equipped item (in `CollectAllSlots` order: hand
   sets, armor … shoulders, quick slots) before the unit's facts. Fixes: facts (disabled components,
   `Setup`, reapply flags), parts, then `UnitDescriptor.ApplyPostLoadFixes`
   (`CharacterStats` `CleanupModifiers` per stat…).
3. The player inventory's `PostLoad` (each item), the other party inventories, the player
   inventory items' fixes.
4. **`Player.TurnOn`** (its `OnTurnOn` turns the inventory's items on: their enchantments'
   components, then armor `RecalculateStats`), then **`CrossSceneState.TurnOn`**: each
   unit's facts in `TurnOnPriority` order (Mono's sort), parts, `UnitEntityData.OnTurnOn`
   (`Descriptor.TurnOn`: stat prerequisites, inventory, body items; then every fact's
   `UpdateIsActive`).
5. **The caves' main scene state**: the same barriers for its full units, with unit
   inventories' `PostLoad` before the fix pass, and in the fix pass each unit's fixes
   followed at once by its inventory items' fixes (`SceneEntitiesState.cs:179–211`), then
   `TurnOn`.

Not hydrated (reported): 17 unit placeholders (the game rebuilds them from blueprints with
`BecomeUnit`), 67 locators, 33 spawners, 29 map objects, 17 script zones, 9 unit groups, 5
cutscene players, other scene documents, the Kenabres area, the player's own facts and
systems (quests, etudes, kingdom…). WrathOracle's dump lists 21 units; we compare the 4
full ones.

**Ported components** (each file cites its C#): stats: `AddStatBonus`,
`AddContextStatBonus` (ContextValue `Simple`/`Rank`/`Shared`), `AddClassSkill`,
`AddBackgroundClassSkill`, `AddStatBonusEquipment`, `AllSavesBonusEquipment`,
`DerivativeStatBonus`, `RecalculateOnStatChange`, `AddStatBonusIfHasFact` and `ShieldFocus`
(lifecycle; their event handlers not yet); state: `AddProficiencies`,
`AddBackgroundWeaponProficiency`, `AddMechanicsFeature`, `AllowDyingCondition`,
`AddImmortality`, `AddLocalMapMarker` (UI list), `AddClassLevels` (level-up provider list),
`AddFacts` and `AddFeatureOnClassLevel` (load-path checks that report when they would
create or remove facts), `OverrideLocoMotion` (view only); items: `AdvanceArmorStats`
(its two armor-rule handlers); plus `ActivatableAbility.OnPostLoad`/fixes,
`ItemEntityArmor.RecalculateStats` with `RuleCalculateArmorCheckPenalty` and
`RuleCalculateArmorMaxDexBonusLimit`, shields' component lifecycle, and item
`ReapplyAbilities` checks.

## Coverage manifest (required versus implemented)

Hydrated units, components on facts (`cache/sim/Manual_13.manifest.json`) [M]:

| | Types | Components on active facts |
|---|---|---|
| Implemented | 13 | 82 |
| Partial (some hooks or handlers missing) | 8 | 47 |
| Required, missing (has lifecycle overrides or handlers) | 49 | 113 |
| Data only (no hooks, no handlers) or known inert | 61 | 477 |

Top required-and-missing on the hydrated units, by components on active facts:
`AddSpecialSpellListForArchetype` 12, `AddFeatureToNPC` 8, `AddFactContextActions` 5,
`AddBuffInBadWeather`, `AddFeatureIfHasFact`, `CombatStateTrigger`, `DeathActions`,
`FactSinglify`, `SavesFixedRecalculateThievery`, `TwoWeaponFightingAttackPenalty`,
`TwoWeaponFightingAttacks`, `TwoWeaponFightingDamagePenalty` 4 each, `AddAbilityResources`,
`AddBuffOnCombatStart`, `DifficultyStatAdvancement`, `ForbidSpellbookOnAlignmentDeviation` 3
each; then 33 types with 1–2 (mostly rulebook handlers for attacks, damage and saves:
`WeaponFocusParametrized`, `AttackStatReplacement`, `Evasion`, `PreciseShot`…). Most of these
only act on activation, level-up or a rule the sheet doesn't trigger, which is why the
main character's sheet matches; they become live in POC-4/5.

Whole save (`Manual_13.manifest-whole-save.json`) [M]: 243 types: 13 implemented (601
components on active facts), 8 partial (457), 141 required and missing (1,598), 81 data
only (4,397). The largest by components on active
facts: `HideFeatureInInspect` 399 (data), `AddFacts` 379 (partial), `RecommendationForWeaponCategory`
360 (data), `AddProficiencies` 327 (done), `SpellListComponent` 319, `FeatureTagsComponent`
316, prerequisites (data, read by level-up), `ContextRankConfig` 181 (data, but read by
`MechanicsContext.Recalculate`, which reports it), `AddFeatureToNPC` 120 (required),
`AddStatBonus` 112 (done), `AddDamageResistanceEnergy` 69 (required).

**Unsupported executions this run [M]** (per unit, `cache/sim/Manual_13.coverage.json`):
main character: `AddAdditionalLimb.OnTurnOn` (the kitsune bite's extra limb: an item and
body-limb subsystem; no effect on the sheet's stats). Seelah: `DifficultyStatAdvancement`,
`ForbidSpellbookOnAlignmentDeviation` ×2, `Spellbook.PostLoad`. Lann:
`DifficultyStatAdvancement`, `AddCondition`, `EmptyHandWeaponOverride` ×2,
`ForbidSpellbookOnAlignmentDeviation`, an enchantment not on its item's blueprint, a
`ContextRankConfig` recalculation. Anevia: `DifficultyStatAdvancement`,
`AddConditionImmunity`, `SpellImmunityToSpellDescriptor`, an enchantment not on its item's
blueprint. So no unit's run is valid yet, and the comparison shows which gap matters for
the sheet (only the difficulty scaling).

## How to rerun the comparison

```sh
cargo run --release -p wr-sim -- hydrate Manual_13            # writes cache/sim/Manual_13.*
cargo run --release -p wr-sim -- compare cache/sim/Manual_13.state-after-load.json \
    fixtures/captures/oracle/state-manual13-after-load.json  # writes cache/sim/compare.json
cargo nextest run -p wr-sim --run-ignored only               # real-save tests
```

(The oracle's earlier smoke captures gave the same per-unit results.)

## Tests

`cargo nextest run -p wr-sim`: 44 unit tests (synthetic data) plus 2 ignored real-save
tests, all passing [M]; `wr-codegen` gained a test for runtime providers, fact hooks and
handler interfaces. Covered: modifier stacking (best bonus and worst penalty per
descriptor, stackable sums, `ForceStack`, duplicate removal, armor/encumbrance pooling,
`Helpless`, every filter), recomputation through dependents and `OnChanged` (Dexterity to
AC, initiative, saves and skills; limiters; Persuasion to the checks), dictionary slot
reuse, value-equality removal, Mono's sort order on ties, integer wrap; fact lifecycle (load
path order with no `Activate`, turn-off/on idempotence, inactive facts, orphaned runtimes,
`Setup` creating a new component, fail-closed versus empty hooks, turn-on order on ties);
rulebook (all steps and managers in order, subscriber order and deferred removal,
nesting depth and sequence, no re-trigger, overflow at 26); RNG tape (in order, extra,
missing, other site, other range, out-of-range value, play log replays); C# arithmetic
(wrap, truncating division and its exceptions, ties, saturating casts, double-then-store
diverging from strict f32); the clock (cap, millisecond steps, 80 Hz drift, turn-based
scale, just-activated delta). The ignored tests hydrate Manual_13, check the main
character's key values and lifecycle state and its single gap, and that hydration is
deterministic.

`scripts/check.sh` on the whole workspace currently stops at `cargo fmt --check` in
another agent's in-progress files (`crates/wr-game`, `tools/wr-import`); fmt, clippy
(`-D warnings`) and nextest pass for `wr-sim`, `wr-content`, `wr-save` and `wr-codegen`.

## Manual_10

**Measured [M]** (`wr-sim hydrate Manual_10`, compared with
`fixtures/captures/oracle/state-manual10-after-load.json.gz`): 16 full units, 2,498 facts;
hydration 0.31 s after a 0.30 s decode. Overall 13,073 of 13,429 compared fields, **97.3%**;
three units at 100%, the main character at 91.0% (1,187/1,304). Facts: 2,089 of ours all match on
blueprint, class, rank and active flag, but the oracle has **33 facts we don't** (made at
load by repairs we only report: `AddUnitFeatureEquipment`/`AddUnitFactEquipment` fixes,
`AddFacts` restores…), so 9 of 16 fact orders differ. Stat differences are mostly missing
modifiers from `BuffAllSkillsBonus` (33), `ToughnessLogic` (10), `BuffAllSavesBonus` (6),
`SpeedBonusInArmorCategory`, `ArmorFocus`, plus difficulty scaling again. The run reports
82 kinds of unsupported execution; the largest: `AddUnitFeatureEquipment` fixes (65),
`ContextRankConfig` recalculation (22), `AddUnitFactEquipment` fixes (19), item
enchantment repairs (18), `SavesFixerRecalculate` (17), `UnitPartCorruption` (16),
`ForbidSpellbookOnAlignmentDeviation` and `LockEquipmentSlot` (15 each), `MaxDexBonusIncrease`
(12), spellbooks (11), damage resistances, `UnitHealthGuard`'s health event (10). Its
manifest: 343 component types, 213 required and missing (1,197 components on active facts).

**What it needs** to pass [I, ranked by the measured gaps]:

1. **`DifficultyPresetsList`** extracted from `blueprint.assets` (also POC-4's DC
   adjustments) to port `DifficultyStatAdvancement`, plus the difficulty settings from the
   save.
2. **`ContextRankConfig`** and `ContextCalculateSharedValue` (contexts recalculate on
   `RecalculateOnStatChange` turn-on and on any reapply), and the property calculators.
3. **Spellbooks** (`Spellbook.PostLoad`, spell slots, caster level) and
   `ForbidSpellbookOnAlignmentDeviation`.
4. **Fact processors** (buff stacking and ranks, feature ranks) and fact creation, for
   `AddFacts`, `AddFeatureOnClassLevel`, `AddFeatureIfHasFact` repairs.
5. **Unit state**: conditions and immunities with status effects, body limbs, empty-hand
   weapon overrides, polymorph.
6. **Event arguments** on the bus (fact gained/lost, equipment changed) and the attack,
   damage and save rules with their handlers, if the sheet includes rule-derived values.
7. **Pets and mythic** progression, item `ApplyEnchantments` repair (tracking
   `ParentContext`), placeholders' `BecomeUnit` for any companion stored as one.

## Effort (agent time, estimated [I])

One working session (about 9 hours of agent time, with an API interruption). Rough split:
reading the C# (stats, facts, rulebook, load path) 30%; framework (`csharp`, `rng`,
`time`, `world`, stats, facts, rulebook, registry) 35%; hydration and the sheet 15%;
component ports 10%; comparison, fixes and tests 10%. Per component type, once the
framework existed: the simple stat components (`AddStatBonus`, `AddClassSkill`,
`DerivativeStatBonus`, equipment bonuses) took minutes each; `AddContextStatBonus` and
`AddStatBonusIfHasFact` (context values, restrictions) about a quarter hour each; armor
recalculation with its two rules and `AdvanceArmorStats` about an hour, including finding
the `CleanupModifiers` interaction through the comparison. The codegen extension (runtime
providers, hooks, handlers) took about an hour and made the manifest and fail-closed
dispatch precise. Review and rework are listed below.

## Review

GPT 6 Astra (high) reviewed `crates/wr-sim`, the codegen changes and these two documents
against the decompiled C#. While it ran, two findings it raised early were already fixed
(`CleanupModifiers` must remove *every* entry of a group of three or more equal sourced
modifiers, because the C# keeps the first one in a local; `SubscribersList.Cleanup` runs in
every frame's `finally`, even while an outer frame iterates). Its 13 remaining findings
were each checked against the C#; all were real, and all are fixed:

- **Codegen:** handler interfaces are now every declared interface deriving from
  `ISubscriber` (e.g. `ISavingThrowFailed`), not those named `…Handler`; a delegate that
  overrides `CreateRuntimeFactComponent` is a custom runtime (e.g. `EtudeBracketTrigger`),
  except `UnitBuffComponentDelegate`, whose runtime only adds buff-removal forwarding.
  Schema and pack regenerated; test extended.
- **Fail-closed gaps now reported:** turn-based `CombatController.TickTime` (the clock flags
  it, `World::tick_clock` reports it); fact removal through a unit's fact processors; unit
  upgraders not yet applied; duplicate replace-stacking buffs; stale attribute
  damage/drain status buffs; additional-limb items; RNG tape failures now make a run
  invalid (`World::valid`, `finish_run`, and the dump's `sim.rng_failures`).
- **Order:** `Body.PostLoad` post-loads equipped items before the unit's facts, slots in
  `CollectAllSlots` order (quick slots were in the wrong place); the area's fix pass fixes
  each unit's items right after the unit.
- **Semantics:** saved runtimes are matched to delegates by their closed generic type
  (both arguments), not by a short data-type name (a test pins it); context item
  references resolve lazily like `EntityRef`; an overflow raised inside a subscriber is
  caught at that subscriber, as the C#'s dispatch does; a custom runtime's `OnRecalculate`
  defaults to off-then-on.
- **Docs:** the replace-stacking and fixes claims were corrected; the overflow and
  turn-based statements below were narrowed.

The comparison results above are after these fixes (unchanged for Manual_13).
