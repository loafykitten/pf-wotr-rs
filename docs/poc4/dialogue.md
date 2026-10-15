# POC-4: dialogue, headless — handoff

Second round, written 2026-10-11. Tags: **[M]** measured, **[C]** read in the decompiled C#,
**[I]** inferred. No game text here: GUIDs, type names and counts only. Nothing committed (the
coordinator commits after review). Captures are local and gitignored
(`fixtures/captures/oracle/`); sim outputs go to `cache/sim/dialog/<fixture>-<run>/`.

## Headline

| | Result |
|---|---|
| Certification fixture `dialog-checks` (cert-9: record, replay, perturbed replay) | **The whole ordered T0 stream equal on all three runs [M]**: 78/78 records (47 dialogue records and answers, 6 narrative mutations, 18 rule records with their results, 7 rule draws, in one order), failure and success branch of the Athletics check; character sheets 2,372/2,372 fields; narrative end state equal except one etude (ToyBox, below) |
| Seelah/Anevia meeting, paths A, B, C (12 runs: record, replay, perturbed replays); **round 3 below: now the whole stream equal to the end** | **Every run's dialogue stream equal [M]** (71, 81, 76 records per path), every rule draw equal (7, 13, 7), every rule result equal up to the recruitment; the ordered stream equal up to the recruitment (102/354, 130/382, 108/357 records), where the oracle builds the companion's new unit (`ChangeUnitBlueprint`) and applies a buff (`AttachBuff`), both reported unsupported by the sim. Character sheets 100% on every run (3 units, 1,737 fields) |
| The three reachable Seelah checks on success and failure tapes | **Yes [M]**: Athletics `ded4aae5…`, Knowledge World `d530baf0…`, Diplomacy `d0ef7c0d…`, each recorded failing and passing (perturbed d20), each matched by the sim. The fourth (Oread-only Athletics) is unreachable from Manual_13 (Kitsune PC) |
| Horgus (Manual_19, Prologue_Neathholm), check path and alternative path (6 runs) | **Dialogue streams equal on every run [M]** (52 and 64 records); the party Diplomacy check (authored DC 16) passing in the record (flag `ddfedbde…` = 2000) and failing in a perturbed replay (= 1000), the alternative path setting 500; all 10 rule records before the closing cutscene equal (roller, stat 8, DC 16, d20, total); character sheets 100% (7 units, 4,269 fields); narrative end state equal except the ToyBox etude. The ordered stream diverges at the closing cutscene (`HideUnit`, `Spawn`: reported unsupported) |
| Round 12 (2026-10-14, twentieth session): Camelia | **All 4 runs of `poc4-camelia-6` valid [M]** (from Ember's Manual_20, walking into her trigger), and all 4 of the development capture `poc4-camelia-5` (Seelah's meeting, then a teleport): her cutscene starts the dialogue (`cs_dialog`), skips a command on its entry check (`cs_entry`), and she joins; whole T0 stream, end state and units equal (round 12 below) |
| Round 11 (2026-10-14, nineteenth session): text | **Every dialogue run valid with the dialogue's text compared [M]**: on `poc4-seelah-7`, `poc4-horgus-11` and `cert-24` (recorded with `dialog_text`), every history line, cue and answer text and speaker name and colour equals the game's (`docs/poc4/text.md`). Camelia's meeting recorded for the first time (`poc4-camelia-1`, round 11 below): not yet run by the sim |
| POC-4 pass criterion, round 10 (2026-10-14) | **Met on every run**: the 6 Horgus runs (`poc4-horgus-9`) are valid too, with the unit groups modelled and checked against every recorded group state and roaming's two draws tied to their recorded producers; the 12 Seelah runs (`poc4-seelah-5`) stay valid with the groups checked, and the 3 `dialog-checks` runs (`cert-22`) (round 10 below) |
| POC-4 pass criterion, round 9 (2026-10-14) | **Met on 18 of 24 runs, on captures with every recorded input (`poc4-seelah-4`, `cert-22`)**: the 15 Seelah and 3 `dialog-checks` runs are valid with the cutscene lock's buff tick and postpone, Cutscene and Pause modes, the zone entry's tick and every tick's mode checked. The 6 Horgus runs (`poc4-horgus-8`) are T0-equal with the end state 1.0000, invalid on group membership and roaming's two draws (round 9 below) |
| POC-4 pass criterion, round 5 | **Met on 15 of 21 runs (round 5 below, after its review)**: the 12 Seelah and 3 `dialog-checks` runs are valid (nothing unsupported, whole ordered stream equal). The 6 Horgus runs are not valid: they report the cutscenes' random-track gates (the oracle draws there and in roaming, which the sim doesn't run) and the awake state of the unit the closing cutscene spawns, whose toggle the sim therefore doesn't tick (stream 127/130, the 3 records that toggle) |
| Hydration, sheets after load | Manual_13 2,372/2,372 (100%); **Manual_19 4,269/4,269 (100%, first measurement)**; Manual_10 98.46% (was 97.35%) |

**Found: ToyBox changes the oracle's results [M, via gpt-6.1-sol, verified in the installed
DLL].** The oracle build loads ToyBox 1.7.27 with `toggleAllowAnyGenderRomance` on
(`Wrath-Mac/builds/arm64/Mods/ToyBox/Settings.xml:89`). ToyBox's Harmony postfix on
`PcFemale.CheckCondition` returns true for conditions owned by the etude `LannRomance`
(`d9baf40d…`), so in the oracle that etude stays playing with a male PC while the
unmodded C# (and the sim) stops it at the first `UpdateEtudes`. That is the one narrative
difference in every run. **The oracle should run without gameplay mods** (or the comparison
must model them); needs Ember/the coordinator.

## How a run is compared

`wr-sim dialog <fixture> --oracle <capture>/fixtures/<fixture> --run <record|replay-N|…>`
(fixtures in `crates/wr-sim/fixtures/poc4.json`; `--oracle-inputs` replays the oracle's
recorded answers, for fixtures whose optional steps depend on T1 timing):

1. Hydrate the save, then `Game.AfterSceneLoaderFinishedArea`'s stages (`area_activated`).
2. Replace the RNG with a tape of the oracle run's sim draws at the sites the sim models; set
   the clock to the oracle's window-start game time; compare the narrative state at the
   start.
3. Run the steps on the sim clock (`runner.rs`): before-tick tasks, `TimeController`, the
   cutscene controller's recorded progress, `UnitActivatableAbilitiesController`,
   `UnitBuffsController` (not in Cutscene mode), script zone input, `DialogController`,
   `EtudeSystemController` (nothing in Pause mode). Steps are WrathOracle's,
   with the T1 walk into a trigger replaced by `zone:<script zone>`.
4. Compare: **one ordered T0 stream** (dialogue records and answers, narrative mutations
   in WrathOracle's postfix/prefix positions, rules and their results per class, draws at
   modelled sites, run-time ids renamed by first appearance); oracle sim draws at the
   audited T1 sites (`oracle-trace-diff`'s list) are counted and left out, any other sim
   draw the sim doesn't make fails the comparison; the narrative end state
   (`narrative/dump.rs`: etudes, etude facts and their component data, flags, quests,
   dialogue history, alignment, party and XP by unit, area/mode, structure; game time
   reported as T1); and every unit's character sheet.

## Done and measured

- **Etude brackets [C, M]** (`components/etudes.rs`): the generic `EtudeBracketRuntime`
  (activate/deactivate/post-load, `MaybeEnter`, enter/exit/resume with linked-area and
  read-only checks), the proxy subscription (`Sub::Runtime`: the delegate's interfaces
  through the runtime, then the runtime's own; delivery prefers the delegate, so
  `EtudePlayTrigger`'s handler runs twice per raise, as in the C#), `EtudePlayTrigger`,
  `EtudeInvokeActionsDelayed`, and 24 bracket types (unit and area `CountableFlag`s as named
  counters, interaction overrides, tutorials, follow-unit state, camp/skip-time overrides;
  audio, music and weather as stubs; locator placement, party changes, capital logic and
  corruption clearing reported unsupported when they would act). The etude bracket kinds
  are gone from both saves' load reports (Manual_13 had 64 unsupported kinds, Manual_19
  74); what remains is listed below [M].
- **Area activation after load [C]** (`hydrate::area_activated`): `OnAreaScenesLoaded`,
  `OnAreaDidLoad`, the entity-creation queue (placeholders not built: reported), the area's
  main fact (no components in these areas), `UpdateEtudes`, `OnAreaActivated`,
  `Player.OnAreaLoaded`. Area scene documents not hydrated are now reported.
- **Elements [C]**: 26 conditions (`CompanionInParty`, `CueSeen`, `DialogSeen`,
  `FlagInRange`, `FlagUnlocked`, `IsDLCEnabled`, `LessThan`, `ObjectiveStatus`,
  `OrAndLogic`, `PcFemale`, `QuestStatus`, `UnitClass`, `UnitArchetypes`, `HasFact`,
  `IsCampaign`, `IsCampaignImported`, `IsEqual`, `PartyUnits`, `UnitIsDead`,
  `CompanionIsDead`, `KingdomStatCheck`, `PlayerTopClassIs`, …), evaluators `FlagValue`,
  `PartyUnit` (and `UnitFromSpawner` fixed: it read the wrong field name and never
  resolved), action `UnlockFlag`. DLC availability is platform state:
  `cache/sim/dlc-rewards.json` (four rewards, inferred from the oracle's after-load etude
  states [M, I]).
- **Unit state [C, M]**: conditions and immunities as `UnitState` counters (saved arrays
  hydrated, `State.PostLoad`'s Fatigued/DifficultTerrain reset, `SpellcastingForbidden`
  fix), status effects (Fatigued ported; saved ones and the others reported), `Prone`,
  `CanAct`; components `AddCondition`, `AddConditionImmunity`, `BuffAllSkillsBonus`,
  `BuffAllSavesBonus`, `ForbidSpellbookOnAlignmentDeviation`, `AreaDidLoadTrigger`,
  `DeathActions`/`EvaluatedUnitDeathTrigger` (spawn events; deaths reported). These took
  Manual_19's sheets to 100%.
- **Quests**: `Experience` objective and quest callbacks; trace records moved to
  WrathOracle's positions (etude start/complete, flags, XP and fact attach as postfixes;
  fact detach and dialogue stop as prefixes; check end after the nested cue).
- **Runner, dumps, comparison** (`runner.rs`, `narrative/dump.rs`, `wr-sim dialog`), the
  before-tick task queue (`BeforeTick`: `ScheduleAction`, `StartMode`, `StopMode` in one
  FIFO, as `Game.ExecuteBeforeTickActions`).
- **Speakers built from placeholders**: dialogue speakers that the load builds from
  `UnitPlaceholderEntityData` (Horgus) are found by blueprint and position (finally dead
  excluded), and reported unsupported (the unit isn't built).
- **Tests**: 60 unit tests (11 new, synthetic: bracket runtime and subscriptions, play
  trigger semantics, conditions, flags, unit conditions and `CanAct`, the Fatigued effect,
  runner helpers, narrative compare) and 4 ignored real-data tests (Manual_13 ×2; cert-9 on
  three runs: whole stream equal; 10 of the POC-4 runs: dialogue and draws equal, sheets
  100%). fmt, clippy `-D warnings`, nextest pass for `wr-sim`.

## What every run still reports (why no run is valid yet)

After round 4 (`cbe8d15`) [M]:
- Seelah runs (all 12): `ActivateTrigger.OnActivate` (2) and the area's script zone (`ScriptZone: scene state
  and OnUnitEntered callbacks not hydrated`).
- Horgus runs: the closing cutscene's `HideUnit` and `Spawn`.
- `dialog-checks`: the script zone.
- At load: Manual_13 9 kinds, Manual_19 29, Manual_10 210 (each save's `coverage.json` lists them): the
  area's other scene documents and entities (script zones, map objects, cutscene players, unit groups),
  units not built from placeholders, and unit components outside the dialogue.
- Other narrative differences: Seelah runs end in Pause mode in the oracle (the tutorial pop-up the sim
  stubs pauses the game) [I].

## Review

GPT 6 Astra (high). **Round 1: 23 findings**, each checked against the C# and the oracle.
22 were real and are fixed (comparison soundness: one ordered stream, consistent id
correspondence, unmodelled draws failing, full rule results; the before-tick queue order;
saved conditions, status effects and posture; placeholder speakers and omitted load stages,
zone and cutscene actions made fail-closed; follow-unit state; `CompanionInParty`
finally-dead; optional-step waits; oracle-input mapping; party and XP by unit identity;
`dlg_check_end` fields; quest XP float rounding; DLC-restricted classes; disposal guards; a
test's tolerance; a fabricated log). One (the delayed etude's recalculation) was a false
positive, which round 2 confirmed.

**Round 2: 13 findings** on the fixes; round 1 confirmed resolved for 14, partly for 8.
Fixed: the oracle recording's health is now checked (missing begin/end, failed or dropped
records, structural errors, hook exceptions, unknown unhooked RNG steps fail the
comparison); saved fact ids (UUID-form) count as saved; recognized saved status effects and
saved posture are reported at hydration; main-scene entities not hydrated (script zones,
map objects, cutscene players, unit groups) are reported; rule results fold handler and
final deltas; transition skill-check payloads use the oracle's form; the follow-unit part
keeps the leader it was initialized with and the leader's part goes when empty; flattened
`dialogrun` steps keep their wait for the dialogue to end; a fact removed during its own
activation gets no `fact_attach` record; dialogue inputs (initiator, unit, first speaker,
manual unit) and rule initiators are compared; the dump's structure is type-checked; the
certification test asserts the start state and the exact load gaps; the placeholder-speaker
fallback (left as a stub by an edit that missed) now reports unsupported. All results above
are after these fixes, unchanged. **Open, documented, not fixed:** (a) the sim records a
rule's results at the end of `OnTrigger`, the oracle after its did-handlers: equal for every
rule in these fixtures (no handler changes a compared field), but a rule whose handler does
would compare wrongly until the sim records at the final phase; (b) custom runtimes' own
members (`EtudeInvokeActionDelayedData`'s timer, brackets' processed flag) aren't in either
dump (the oracle dumps only delegate `Data`), so they're outside the narrative comparison.
No round 3 was run.

## Round 3: run-time items (2026-10-11)

Tags as above. Captures: the clean-build `poc4-seelah-2` and `poc4-horgus-2` (no ToyBox).

| | Before (round 2 merged with unit creation) | Now [M] |
|---|---|---|
| Seelah path A, ordered T0 stream (record, replay-1, perturbed) | 337/351, equal prefix 328 | **351/351 equal on all three runs** |
| Seelah path B (record, 4 replays) | 365/382, equal prefix 356 | **382/382 equal on all five runs** (the perturbed ones 379/379, 382/382) |
| Seelah path C (record, 3 replays) | 343/357, equal prefix 334 | **357/357 equal on all four runs** (the perturbed one 355/355) |
| Character sheets after the run, Seelah A/B/C | 0.9967 / 0.9938 / 0.9967 | **1.0000** on every run: 21 units, the recruited companion's AC, skill penalties, speed and modifiers included |
| Seelah C, narrative end state | money 411 (oracle 611) | money equal (`AddItemToPlayer`'s +200) |
| Horgus (`horgus-check`, `horgus-alt`) | 80/130, prefix 80 | unchanged (80/130): they stop at unit creation for Horgus's speaker |

What the stream needed, all read in the C# [C] and ported in `crates/wr-sim`:

- **`ItemsEntityFactory.CreateEntity`** and the `ItemEntity` constructors: weapon, armor,
  shield (an armor and a weapon component of its own), usable and simple items, fresh ids,
  the blueprint's enchantments (a weapon's or armor's type enchantments, then its own), charges,
  identification. The shield's components are entities, the order the C# creates them in.
- **`UnitBody` constructor and `Initialize`**: the empty-hand weapon entity, the four hands
  sets (inactive sets don't equip), limbs, quick slots, then the named slots.
  `TryInsertItem` → `ItemSlot.CanInsertItem` (slot support, `CanBeEquippedBy` with class and
  stat restrictions, weapon/armor/shield proficiencies, the hand pair rules) → `InsertItem`
  (the item enters the unit's inventory collection first: the player's shared inventory for a
  party member, where an identified item **merges into an existing stack** and a stack **splits
  one off**; then the slot, `OnDidEquipped`, `HandSlot.OnItemInserted`, the equipment events).
  `AddStartingInventory`.
- **Equipping**: `ItemEntity.OnDidEquipped`/`OnWillUnequip` with the subclass overrides: an armor
  recalculates its stats (the armor check penalty and max-Dex rules show in the stream), a shield
  does the same for its two components, enchantments activate with a context for the wielder, a
  usable item's ability is added to the wielder (the potion's `Ability` fact).
- **Equipment events** now reach their subscribers: `ShieldFocus` (its +1 AC in the oracle's
  sheet), `ArmorFocus`, `DeflectArrows`, `MonkNoArmorFeatureUnlock`,
  `MonkNoArmorAndMonkWeaponFeatureUnlock` are ported; `PartyInventoryTrigger` and the
  `TutorialTriggerNewItem` family handle the collection events (a tutorial trigger that would
  fire is reported).
- **Disposal**: `EntityDataBase.Dispose` generally (also the level-up plan unit), and
  `EntityDestructionController` for the replaced NPC: `Destroy` (`HandleUnitDestroyed`), then
  `Dispose`, whose `Body.Dispose` disposes each slot item, which leaves its collection and its
  slot (`OnWillUnequip`): the item's `Ability` detaches (the last record of every Seelah
  stream), then the state, `HandleEntityDestruction`.
- **`UnitActivatableAbilitiesController`** runs each tick before the script zones and the
  dialogue (`TickAbility`: the start of a toggle that is on and not started, with the group
  capacity of `TryStart`): the recruited companion's `InspiredRage` toggle buff attaches in the
  first tick after the answer. Awake units are the party's units in game [I]: fog of war is
  not modelled, so area units aren't ticked outside the load pass.
- **`AttachBuff`** (path B's buff) and **`AddItemToPlayer`** (item creation, `MoneyReplacement`
  gaining money; equipping isn't ported and is reported).
- Fixes found on the way: `MechanicsFeatureType` values `ImprovedUnarmedStrike` (21),
  `FighterOnHandedGrip` (112) and `ShieldBash` (4) were wrong in `combat/weapon.rs`; a
  constructed unit starts with `UnitProficiency`'s default armor and weapon proficiencies; a
  placeholder's saved body isn't resolvable while the unit's facts turn on (`ArmorFocus` and
  `ShieldFocus` see no armor: the oracle's Manual_10 units without the +1 agree); the unit sheet
  comparison renames the ids of items created at run time (`Guid.NewGuid()`, different in every
  run) by order of first appearance, as the stream comparison does.
- Not built, reported unsupported: double weapons, `ItemFact` (runtime components on an item's
  blueprint), unidentified items entering the player's inventory (`TryIdentify`), slot locks,
  polymorph, `AddItemToPlayer` with `Equip`, explicit grips on insertion, the per-round tick of a
  started toggle (now ported, below), spellbook disposal (now ported, below).

Tests: `tests/poc4.rs::seelah_streams_equal_the_oracle_to_the_end` (ignored; every Seelah run's
T0 stream equal and the sheets above 0.9999), the synthetic item tests in `items.rs`.

### Round 3, part 2: review fixes, spellbooks and ability resources

**Review of run-time items** (GPT 6 Astra, nine findings, each checked against the C#): simple
equipment and usable items return their own enchantments (`CollectEnchantments`); the unit sheet
comparison renames only ids positively created during the run (the dump lists the ids that came
from the save, `sim.saved_item_ids`; a modifier from the wrong saved item still differs); a
started toggle keeps its round timer and runs `OnNewRound` (deactivation of an off toggle, first
round, combat end), `ShouldBeDisabled` applies the Rest mode on its own, an area unit with a
toggle to tick and a unit destroyed with dropped loot are reported; moving an equipped item
notifies the vacated slot first; an item with an overridden visual doesn't stack; stack counts
wrap like C#.

**Spellbooks and ability resources.** `crates/wr-sim/src/spellbook.rs`, `resources.rs`,
`hydrate/books.rs`, `components/books.rs` (architecture.md has the list). The sheet dump and
`compare` now carry `spellbooks[]` (blueprint, caster level, the oracle writer's rendering of the
persisted spellbook, depth limit and `$cycle` markers included) and `resources[]` (amount,
`RetainCount`); spells created during the run are compared with their random ids renamed in each
spellbook, spells from the save strictly. Match per field, equal/total [M]:

| | Spellbooks present | Caster level | Spellbook persisted | Resources present | Amount | Resource persisted |
|---|---|---|---|---|---|---|
| Manual_13, before → now | 0/2 → 2/2 | – → 2/2 | – → 2/2 | 3/6 → 6/6 | 3/3 → 6/6 | 0/3 → 6/6 |
| Manual_19 | 0/39 → 39/39 | – → 39/39 | – → 39/39 | 6/6 → 6/6 | 6/6 → 6/6 | 0/6 → 6/6 |
| Manual_18 (placeholders spawned) | 0/2 → 2/2 | – → 2/2 | – → 2/2 | 6/6 → 6/6 | 6/6 → 6/6 | 0/6 → 6/6 |
| Manual_10 | 0/102 → 101/102 | – → 101/101 | – → 87/101 | 90/320 → 320/320 | 90/90 → 302/320 | 0/90 → 302/320 |
| Seelah A, B, C (all 12 runs) | 0/2 → 2/2 | – → 2/2 | – → 2/2 | 2/6 → 6/6 | 2/2 → 6/6 | 0/2 → 6/6 |
| Horgus (6 runs) | 0/39 → 39/39 | – → 39/39 | – → 39/39 | 6/6 → 6/6 | 6/6 → 6/6 | 0/6 → 6/6 |
| `dialog-checks` (3 runs) | 0/2 → 2/2 | – → 2/2 | – → 2/2 | 3/6 → 6/6 | 3/3 → 6/6 | 0/3 → 6/6 |

("Before" is the previous binary's dump measured with the new comparison: no spellbooks, and
resources without their persisted members. The 3/6 and 2/6 are the companion units built from
placeholders, which had no resources before.) Manual_10 is the late-game save with 539 units:
every unit that still differs reports a cause: attribute points of `AddClassLevels.LevelsStat`
(the casting stat differs, so slots and the Wisdom-based resources do), spell selections,
mythic spellbook selection, `AddOppositionSchool`, `SpontaneousSpellConversion`.

What the C# does that the stream needed [C]: `DemandSpellbook`, `AddLevelFromClass` /
`LearnSpellsOnRaiseLevel` (cantrips on the first level, special lists), `AddKnown` (a second
entry for a known spell), `UpdateSlotsSize` (grow, hide and restore slots through the two hidden
dictionaries, resorting), `Memorize` (free slot by type, `RemoveMemorizedSpells`), `Rest`, the
casting stat's `OnChanged` handler installed by `Spellbook.PostLoad` (a stat change shrinks or
grows the slots, also while a unit's facts turn on), `UnitPartExtraSpellsPerDay`;
`AddClassLevels` raises `ILevelUpCompleteUIHandler`, so `AddKnownSpell` of every unit runs;
`AddAbilityResources` (not while a feature reapplies), the max amount of a resource from class
levels, stat bonus and the `IResourceAmountBonusHandler` components. The sheets of the Seelah and
Horgus runs stay 1.0000 and the streams equal.

New unsupported kinds: `CompanionBoon: ILevelUpCompleteUIHandler` and `ShareFeaturesWithPet:
ILevelUpCompleteUIHandler` (Manual_10 only, on units that already report the components'
`OnActivate`/`OnTurnOn`). Gone from every Seelah run: `ApplySpellbook`, `PrepareSpellbook`,
`LearnSpells`, `AddSpecialSpellListForArchetype`, `AddAbilityResources` (both),
`IncreaseActivatableAbilityGroupSize`, `AutoMetamagic`, `Replenish`, `CopySpellbook`,
`UnitDescriptor.Dispose: spellbooks`. What the Seelah runs still report: the parametrized
feature selection, the mythic-spellbook selection items, `AddClassLevels` attribute points,
`AddDamageResistancePhysical`, `AuraFeatureComponent`, the tutorial pop-up stub, the recruit's
`RemoveNegativeEffects` over buffs.

## Round 4: validity (2026-10-11, fourth session)

Porting what the runs reported, in three pieces, each reviewed by GPT 6 Astra (high) until no finding was
left (merged into `poc` at `cbe8d15`):
- Attribute points from automatic levels (`AddClassLevels.LevelsStat`, wrapping increments), DLC-restricted
  selection items, `ContextCalculateAbilityParams` (`rules/ability_params.rs`: `SpellSource` Unknown, the
  result recorded and compared on its stored members) and item enchantment repair at turn-on. Review rounds
  fixed `ContextRankConfig` reading `context.Params` (`RankBonus` for class levels, `CasterLevel`;
  `m_Params ?? ParentContext?.Params ?? AbilityParams.Empty`, saved nulls and `$ref`s resolved at
  hydration, `CloneFor` children inheriting rather than copying), and null-reward `DlcCondition`s reported
  wherever nothing handles the original's exception.
- Parametrized features and mythic-spellbook selection (`feature_param.rs`: the saved param form,
  `Items`/`CanSelect` by parameter type, `PrerequisiteConditionForWeaponCategory`, `CanSelectAny`,
  `PrepareFactForAttach`, the `ParamContextData` scope, `SelectFeature.Apply`, `GetReplaceSpellbook`).
- `AddDamageResistancePhysical` (the shared base runtime, `UnitPartDamageReduction`, saved pools kept at
  load), `AuraFeatureComponent`, `AddLoot`, `Replenish`'s `RemoveNegativeEffects`, the auto-cast gate;
  cloned contexts' ranks start at zero as in `MechanicsContext`'s constructor.

Regression against the round-3 build [M]: every dialogue run's stream and sheets unchanged; Manual_13 1.0000,
Manual_19 0.9988 and Manual_18 unchanged; **Manual_10 0.8134 → 0.8177** (62 units better, none worse). The
Seelah runs report two kinds (was 15). Four Manual_10 placeholder units' `ShieldOfFaithBuff` (inactive and
`IsFromSpell` in the oracle) is built active and not from a spell by the placeholder spawn path: open, see
next steps.

## Round 5: scene entities and the closing cutscene (2026-10-12)

**15 of the 21 dialogue runs are valid now [M]** (12 Seelah, 3 `dialog-checks`: `coverage.unsupported` empty at load
and in the run, no oracle draw missed), streams and sheets as before. The 6 Horgus runs report nothing unsupported
but are **not valid**: a run is valid only if the oracle draws nowhere the sim doesn't (`FixtureRun::valid`). Manual_13 and Manual_19 hydrate valid with no
unsupported kind (was 9 and 29). (This was the state before the review below, which changes the Horgus runs and
Manual_19's load.)

What was ported (all [C] from the decompiled C#; the cached scene data [M] from the importer):
- **Scene cache** (`scene.rs`, `hydrate/scene.rs`; `scripts/scene-cache.sh <area> <guid>`): the area's
  scene documents come from the importer's Unity scene read (`wr-import import --area`), then
  `wr-sim scene-import` writes `cache/sim/scenes/<guid>.json` (script zone shapes, map object components,
  cutscene players, unit groups). Cylinder and box `Contains` (with the game's quirks) match hundreds of
  importer probes (ignored real-data test). A missing cache or view is reported, never guessed.
- **Load order** as in the original: `SceneEntitiesState.PostLoad`, `SceneLoader.MatchStateWithScene` (view
  attach by UniqueId, `scene_parts.rs`: part set check, `SetSource`, loot tables, `InteractionLootPart`,
  `KingdomLootPart`, `UnitPartCorruption`, `UnitPartVendor`), then `TurnOn`; visibilities (`AreaTransitionPart.
  UpdateVisibility`) follow flag and etude changes and the area load.
- **`ScriptZone`**: `TryAddUnit`, `OnceOnly`, `PlayersOnly`, inside units, `OnAreaDidLoad`, trigger
  conditions/actions, `IScriptZoneHandler`; **`ActivateTrigger`** (conditions outside the `FactData` scope,
  actions inside, `AlreadyTriggered`); `FactOwner` and `SpawnedUnit` evaluators.
- **Closing cutscene** (`scene_spawn.rs`, `elements/cutscene.rs`): `Spawn` (`UnitSpawnerBase.Spawn`,
  `UnitSpawner.SpawnUnit`, `EntityCreator.SpawnUnit`, `OnSpawn` handlers) and `HideUnit`
  (`UnitEntityData.OnIsInGameChanged`: buffs, activatable abilities, abilities deactivate). The cutscene's T0
  steps run in order (a gate's tracks, each track's commands, end gates), queued at `PlayCutscene` and run at
  the next tick's cutscene step; more T1 commands classified (`CommandCameraShake`, `MarkUnitControlled`,
  `ActivateGate`, `RequireUnits`, `AddFogOfWarRevealer`; `SpawnFx`).
- Long tail that the two saves reported: `AddBuffActions`, `AddUndetectableAlignment`, `AddUnitFactEquipment`,
  `ArmorCheckPenaltyIncrease`, `MaxDexBonusIncrease`, `ArmorSpeedPenaltyRemoval`, `WeaponTraining(Bonuses)`
  with `UnitPartWeaponTraining`, `AddVendorItems`, `EmptyHandWeaponOverride` with the empty-hand stack
  (`UnitBody.SetEmptyHandWeapon`, `RemoveEmptyHandWeapon`), `RuleIdentifyItem` (the player inventory is a
  global subscriber), summon pools, difficulty requirements on `AddFacts`, and `AddFacts.CasterLevel` writing
  the shared `AbilityParams.Empty` (the merged `World::empty_params`, `context::set_params_caster_level`).

Measured against the poc build [M]: Seelah and `dialog-checks` streams equal as before (351/351, 382, 357,
78/78), sheets 1.0000 unchanged. **Horgus: ordered T0 stream 130/130 equal (125/125 on the perturbed replay),
units 47 at 1.0000 (was 80/130, 0.9984)**. Horgus stays invalid because the oracle draws at
`CutscenePlayerGateData.SelectTrackToStart` (x68-70; `StopExtraTracksOnStart`: `Random.Range(0, count)` over a
background cutscene's started tracks unless the gate is `FirstTrack`) and `RoamingWaypointData.SelectPrevPoint`
(x1; `PrevWaypoints.Random()` in `UnitRoamingController.AdvancePoint`). Neither is on `AUDITED_T1_SITES` and neither
is ported: the comparison lists them as `UNMODELLED sim draws`, `TraceDiff::equal()` is false, and the run is not
valid. Roaming: its siblings `SelectIdleTime` and `RollNextPoint` are audited T1 (durations and positions), but
`SelectPrevPoint`/`SelectNextPoint` choose the unit's next waypoint, so they are not argued into T1 here [C].
Porting them means running the background cutscene players and the roaming controller (their start times come
from the game clock), which is the next step. Manual_19 0.9988 -> 1.0000. Manual_18 and the combat compares unchanged.

After merging `poc` (`a97aef3`: `entity::set_is_in_game`, `World::empty_params`, placeholders leaving the game): one
implementation of each (`HideUnit` and `Spawn` use `set_is_in_game`, which also raises `IInGameHandler` and
the T1 event), and against that base no unit's match count drops: Manual_10 0.8196 -> 0.8206 (9 units up),
Manual_19 0.9988 -> 1.0000. The exception measured against the older base was:

**One exception, older base [M]:** Manual_10 0.8177 -> 0.8187 (9 units up, 1 down): unit 62460 AC 26 -> 27. With the
`AbilityParams.Empty` caster-level quirk the placeholder's `ShieldOfFaithBuff` Deflection is 5 instead of 2;
the oracle holds that buff inactive (the known open issue above), so the old number matched by coincidence.
Kept: the port is the faithful one.

Gaps at the end of the first part (superseded by the review below): units spawned by the area's awake
spawners ticked as `Controller` units [I]; `HideUnit` is supported only inside a cutscene step (25 etude
`HideUnit`s hide units the oracle keeps in game); shared stash items are not hydrated; the cutscene player's ticking
is classified (T1) not run; `InteractionSkillCheckPart`'s load-time re-roll was a stub.

### Round 5, review: paths accepted as supported that skipped or approximated the original (2026-10-12)

GPT 6 Astra (high) reviewed `poc...133dccb`; its findings were of one kind: paths taken as supported that skip or
approximate the original. Each was checked against the decompiled C# and fixed, or the path is now reported
(`coverage.unsupported`); each fix has a test that fails with the fix reverted (checked).
- **Audited T1 draws excluded from validity** (`runner.rs`): not a finding. The fidelity contract's T1 tier
  (`AUDITED_T1_SITES`: cutscene delays, animation, roaming idle times) needn't be reproduced; left as is.
- **`InteractionSkillCheckPart.OnSettingsDidSet(true)`** at every load [C]: ported (`scene_parts.rs`).
  `DCOverride = DC == 0 || Exact ? DC : DC + Math.Abs(UniqueId.GetHashCode()) % (2v+1) - v`
  (`InteractionDCVariation` v = 2), `SkillOverride` the empty pool's `Unknown`; a pool's draw from the global
  RNG, a calculated DC, DC modifiers and the dungeon generator are reported. `String.GetHashCode` is the shipped
  Mono `mscorlib`'s legacy two-lane hash (`csharp/string_hash.rs`, decompiled with `ilspycmd` into the
  scratchpad) [C]; every saved part of Manual_13 and Manual_19 (9, five with a hashed DC) is what the sim
  recomputes [M] (ignored real-data test).
- **`ScriptZone.OnAreaDidLoad` enumerates `Game.State.Units`** [C]: the `NotFake` pool, each unit tested as the
  `EntityPoolEnumerator` reaches it (`m_IsInGame && !m_Suppressed`; suppression is only set in areas with parts,
  reported there) (`scene::units_pool`, `enumerated_by_pool`). A zone firing at load is reported: it runs among
  the other `IAreaHandler.OnAreaDidLoad` subscribers, whose order isn't modelled (none fires in the fixtures).
- **Cutscene classification** (`elements/cutscene.rs`) [C]: commands with an `EntryCondition`, `RandomTrack`
  gates (`SelectTrackToStart` draws) and `CommandStartDialog` (it blocks its track until the dialogue ends; the sim
  never started a cutscene's dialogue, so the dead `cutscene_dialogs` queue is gone) are always reported. A
  closure with T0 actions must also have its T0 tracks on one chain of `And` gates (they're ordered by that
  chain, not by the walk), no T0 action on a continuous or repeating track, no `FirstTrack`/`Or` gates, gate
  reactivations, command signals or `CommandRequireUnits`, and every T0 gate reached. `CommandActivateGate`'s
  gate is now read from `SignalData.m_Gate` (it was missed). Audit: `CutsceneCommandFadeout`'s in-fade actions
  were skipped (now classified), `CommandDismountSpan` is reported while any unit has a rider part, a nested
  cutscene with T0 actions of its own is reported, and stopping a cutscene with pending T0 actions is reported.
- **A restored running cutscene with T0 actions** (`CutscenePlayerData.Restore` resumes saved track positions,
  which aren't read): reported, not dropped.
- **A unit's `IsInGame` at construction is its view's `IsInGameBySettings`** [C]: `wr-import unit-prefabs`
  writes `cache/sim/unit-prefabs.json` (every `.unit` bundle's `UnitEntityView.m_IsInGame`: 3,463 prefabs with
  one view, all in game [M]); spawners, placeholders (`m_PrefabId`, else the blueprint's prefab, or the
  optimization dummy when spawned hidden and simplified) and `ChangeUnitBlueprint` (the old unit's view prefab:
  the spawned one, `CustomPrefabGuid` or the blueprint's; polymorph, doll and dummy views reported) pass it to
  `spawn::construct`; a missing table or prefab is reported. `Spawn` then sets `IsInGame` through the setter.
  Placeholders spawned simplified get `UnitPartOptimizeWhenHidden` (was missing).
- **Awake state** (`spawn::awake_state`, `SleepingUnitsController.ShouldBeSleeping` [C]): out of game or fake
  sleeps; a companion while the area hides the party sleeps, in a pit wakes; the wake timers (set by view fades,
  proning, death) are unknown, so a finally dead unit is unknown; in combat or in the party is awake; anything
  else (fog of war, cutscene control, commands) is unknown and reported when its toggle would tick. A spawned unit
  is no longer assumed awake. `IsExtra` is hydrated.
- **`SceneLoader.LoadSceneEntities` both ways** (`hydrate/scene.rs`) [C]: each state matches the views of its
  own scene (`SceneEntitiesState.SceneName`); a saved scene entity (any hydrated kind but cutscene players, and
  spawners' data) without a view there is reported (removed in the original), and an active view of a
  `CreatesDataOnLoad` class with no saved data is reported (`CreateDataForView`).
- **Load order** (`hydrate::load_area_states`) [C]: `AreaPersistentState.PostLoad` post-loads every state first,
  then each state is matched with its scene and turned on (was state by state).
- **`AddUndetectableAlignment`** retains and releases the unit's own `UnitAlignment.Undetectable`
  (`CountableFlag`: a `short`, `Release` floors at zero), no side counter.
- **`WeaponTraining`'s rank** reads the hands through `combat::weapon::hand_weapon` (the empty-hand weapon, double
  weapons and shields reported) and the limbs as `WeaponSlot.MaybeWeapon` (empty or inactive: the empty-hand
  weapon); the fighter group's flag is an `int` shift. Manual_10 now reports a shield in hand there.
- Also: `entity::set_is_in_game` raises `IInGameHandler` for every entity and runs `MapObjectEntityData.
  OnIsInGameChanged` (the area transition's visibility), which `AreaTransitionPart` now goes through.

Measured [M] (`scripts/regress.sh` against `poc` `a97aef3`, private caches with the scene caches and the prefab
table; `scripts/regress-diff.sh`, per-unit counts compared): **no unit's match count drops** (0 drops, 12 gains).
Seelah (12) and `dialog-checks` (3) valid, streams 351/382/357 and 78 equal, sheets unchanged. Horgus (6) invalid,
reporting the random-track gates and the spawned unit's awake state; stream 127/130 (122/125 on the perturbed
replay), the three differing records being the spawned unit's toggle the sim no longer ticks on a guess; its sheet
444/446 (the unit didn't exist at the base; 446/446 when it was assumed awake). Manual_13 1.0000 and valid;
Manual_19 1.0000, now reporting the restored background cutscenes' random-track gates at load (the oracle draws
there); Manual_10 0.8196 -> 0.8206; Manual_18 and the combat compare unchanged.

Known gaps left [I] (pre-existing, outside the review): restored roaming (`UnitPartRoaming.RestoreFrom`) is stubbed
as T1, but the roaming controller's waypoint choices draw from the global RNG (`SelectPrevPoint`/`SelectNextPoint`,
not audited T1): the comparison catches those as unmodelled oracle draws, a play run wouldn't. A plain toggle's
round timer on a unit whose awake state is unknown isn't advanced. A cutscene moving a unit into a script zone
(`CommandMoveUnit`) doesn't trigger the zone in the sim.

Next: (1) run the background cutscene players (`SelectTrackToStart`), roaming (`SelectPrevPoint`) and fog of war
(the awake state) so Horgus is valid; (2) HideUnit outside cutscene steps (etude triggers) once the etude conditions
are ported; (3) record Camelia's meeting; (4) `crates/wr-text`.

## Round 6: the Horgus runs' draws (2026-10-14, seventeenth session) [M]

Tried and withdrawn after review: exempting the Horgus runs' two unmodelled draws per run (`SelectTrackToStart` of a
random-track gate whose cutscene has no T0 action, and the roaming waypoint choices when no idle cutscene of the
area has one). gpt-6.1-sol showed why the per-run exemption can't be sound with today's records: a draw's site key
names only the method, not the cutscene player or unit that made it, so exempting the method for one audited gate
also exempts gates the sim never classified; `UnitRoamingController.RollNextPoint` picks an idle cutscene through a
global helper draw besides its audited position and time draws; and a saved roaming next point can carry its own
idle cutscene. **What's needed:** WrathOracle records, for each such draw, its producer (the cutscene and gate, the
roaming unit and point), and the comparison checks each draw against the sim's classification of that producer.

Kept from the round:
- **Cutscene classification holes closed** (found in review, older than this round): a cutscene's `OnStopped`
  actions (run when it finishes or stops) weren't classified at all, and a fade's in-fade `PlayCutscene` didn't
  classify the cutscene it starts. Both now go through the same check as `CommandAction`'s actions; a T0 action there
  is reported (test `stop_and_in_fade_actions_are_classified`).
- A `RandomTrack` gate stays reported wherever it is (a second review round): without an oracle comparison nothing
  else would invalidate a run whose shared RNG it draws from, so the report is what keeps a standalone run fail-closed.
- A placeholder restored with a roaming idle cutscene playing is reported.
- The scene cache (schema v2) carries `RoamingWaypointView`'s fields (the importer now treats the type as gameplay):
  data for the roaming audit above. Neathholm's 22 waypoints and both roaming spawner settings have no idle cutscene.
- `scripts/scene-cache.sh` failed with "no scene bundles": the dependency list names bundles in lower case; it now
  passes the lower-case area name (regenerated caches identical but for the new fields).

The Horgus runs stay invalid on the two draws above, the awake states (180 reports a run; the spawned unit's toggle,
3 stream records) and `OnIsInGameChanged`'s group membership. Next: the producer records and the awake states
(WrathOracle's `awake_set`/`awake` observation runs only in turn-based windows; record it in dialogue fixtures, with
a spawned unit's blueprint, and take it as an input), then the groups model in dialogue runs.

## Round 7: awake states as recorded inputs (2026-10-14, seventeenth session) [M]

The 180 "area unit with a toggle to tick" reports a Horgus run came from units whose awake state needs fog of war
(not modelled). They are now inputs, as in the combat replay:

- **WrathOracle** (`AwakeObs.cs`): outside turn-based windows, one `aa_awake` observation per run of
  `UnitActivatableAbilitiesController`, written at that controller's own tick prefix (`BaseUnitController.Tick`
  filtered to it): the units it iterates, the whole set first, then changes, and a unit created in the run with its
  blueprint. (`ProducerObs.cs` also records, before each, the producer of the global draws the sim doesn't make: a
  cutscene gate's track choice, roaming's waypoint and idle choices. Not used yet.)
- **The dialogue runner** (`spawn::RecordedAwake`, `World::recorded_awake`) parses them strictly (a malformed record is
  reported and the input dropped), takes the oracle tick's set before the sim's controller (the sim counts ticks from
  1, the oracle from 0), and matches each id to a live unit: a saved id as is, a run-created one by blueprint, the
  match unique. `awake_state`'s unknown states take the recording's; a known state that disagrees is reported (the
  sim keeps its own). A tick the recording's controller didn't run, or an id the sim can't match, is reported.

On `poc4-horgus-4` (Manual_19, re-recorded with these observers) **the ordered T0 stream is equal on all six runs**
(130/130, 125/125): the spawned unit's toggle now ticks as in the game. Left per run: the awake state of 62DE
differs for 146 ticks (the closing cutscene's `HideUnit` takes it out of the game at tick 187 in the game, at the
cutscene's start in the sim: cutscene timing is T1 and the sim runs a cutscene's T0 actions in order at its start),
`OnIsInGameChanged`'s group membership, and the background cutscene's `RandomTrack` gate. Next: take the cutscene
commands' timing from the recording (a `CommandAction.OnRun` observation), and the gate's draws as recorded inputs
tied to their producer.

## Round 8: the cutscenes' progress as recorded inputs (2026-10-14, seventeenth session) [M]

A cutscene's timing (delays, animations) is T1, so the sim ran a cutscene's T0 actions in order at its start; with
recorded awake states that showed (62DE hidden at tick 187 in the game, at tick 40 in the sim). And a random-track
gate's draw (`CutscenePlayerGateData.SelectTrackToStart`) can't be made without knowing when the gate activates. Both
are now taken from the recording:

- **WrathOracle** (`ProducerObs.cs`) writes `cs_action` at each `CommandAction.OnRun` (the command, the player's
  cutscene) besides `gate_track`; the begin record declares `cutscene_progress` (and `aa_awake`) when the patches
  installed, the sim's marker for these inputs.
- **Classification** (`elements/cutscene.rs`): a random gate in a closure without T0 actions is listed
  (`Classified::random_gates`, with the cutscene whose player activates it) and settled by the run: reported unless the
  run takes cutscene progress from a recording (decided before the load), then accepted. Found in review and closed
  (most older than this round, they also applied to accepted non-random cutscenes): `CommandMoveUnit` with `RunAway`
  (the unit leaves the game) is reported; a `StopCutscene` classifies the stopped cutscene's `OnStopped`; a command
  that controls a unit (mark, move, look-at, animation) is reported while any unit is unconscious or dead but not
  finally (marking revives it in Cutscene mode); `TranslocateUnit` is a T0 action (`MarkNoExtra`, now ported, also
  for etudes) unless it can only move a spawner's non-extra unit; a command that starts a cutscene and runs T0 actions,
  and a random-track start from `OnStopped` or in-fade actions (no recorded place), are reported; a T0 step carries its
  command invocation.
- **Replay** (`RecordedCutscene`, `recorded_tick`): each oracle tick's events, in order, at the cutscene controller's
  place (after the clock, before the units' controllers): an accepted gate's draw (`Random.Range(0, tracks with
  commands)`, the new modelled site `GATE_SELECT_TRACK`, its started count checked), a `FirstTrack` gate's selection (no
  draw), a `CommandAction`'s pending T0 steps (that invocation's, in order). WrathOracle flags a draw made inside a
  player's `Start`: it waits for the sim's start of that cutscene (`PlayCutscene`, or a recorded presentation command
  that starts it), which covers the start's whole synchronous chain; later activations are the controller's. A
  recorded command without pending steps must be presentation (classified recursively). A step out of order, an event
  not taken by the end of its tick's inputs, one after the run's last tick: reported.
- **Dialogue inputs at their ticks** (`align_input`): with these records, each `dialog:`/`answer:` input is applied at
  the recording's tick (WrathOracle's driver applies a step's first input a frame after the previous step ended);
  a different, late or unreached input is reported.

On `poc4-horgus-6` (Manual_19): **every run's T0 stream is equal, gate draws included (191 to 197 records), the end
state 1.0000, and the only report left is `OnIsInGameChanged`'s group membership**; the comparison still fails on the
two roaming draws (`SelectNextPoint`'s `WeightedRandom`, `SelectPrevPoint`). Seelah and `dialog-checks` (no such
records) are unchanged and valid. A recorded command of a cutscene the sim never classified is reported.

**Known gaps found in review, not closed yet:** `CommandLockControls` (accepted as presentation in every fixture's
cutscenes, these included) ticks every unit's buffs at its start and postpones them at its stop
(`UnitBuffsController.TickAll`/`PostponeAll`): with timed buffs, deadlines shift by the time since each collection's
last tick, a cutscene's duration in Cutscene mode. And the dialogue runner doesn't run `UnitBuffsController` at all
(the combat replay does: `combat::life::buffs_tick`). Reporting the lock with timed buffs around invalidates every
dialogue fixture (all have such cutscenes and buffs), so it waits for the model: record the lock's start and stop
(WrathOracle), track each collection's last tick time (`m_PreviousTime`), run the controller in Default and Dialog
modes, and replay `TickAll`/`PostponeAll` at the recorded places; then re-record the Seelah and `dialog-checks`
captures. Next after that: the roaming draws as recorded inputs tied to their producers (`roam_point`, `roam_roll`),
and the groups model in dialogue runs.

## Round 9: the cutscene lock, game modes and the zone entry (2026-10-14, eighteenth session) [M]

Round 8's known gap, closed: `CommandLockControls` ticks every unit's buffs at its start and
postpones them at its stop [C: `CommandLockControls.OnRun`/`OnStop`, `UnitBuffsController.TickAll`/
`PostponeAll`], and its lock puts the game in Cutscene mode, which has no `UnitBuffsController`.

- **The buffs controller in dialogue runs** (`runner::tick`, `combat::life`): `UnitBuffsController`
  ticks in Default, Dialog (and Rest, GlobalMap), after the activatable abilities' controller.
  `BuffCollection.m_PreviousTime` is tracked per unit (`buffs_previous_time`: set by a tick that
  runs to its end, not saved, zero after a load); `Postpone` moves every finite next tick and end
  by the time since it (nothing when it's zero). `TickAll`/`PostponeAll` sweep `Units.All`
  (`scene::units_pool`: NotFake, the states' order, in the game or not); the controller's tick
  sweeps the pool's enumeration (in the game; suppression reported), and the combat replay's
  now does too. A dual companion is reported (its inactive pair would be ticked).
- **Game modes** (`runner::execute_before_tick`): a stack (`Player::modes_below`); `StartMode`'s
  request check, `DoStartMode`/`DoStopMode` (a stop of a mode that isn't current does nothing);
  `CutsceneLock` (a counting guard and its scheduled `UpdateModeForCutsceneLock`, which starts or
  stops Cutscene mode). Each activation of a mode with `UnitBuffsController` runs its `Activate`:
  `RemoveAfterDelayIfInvalid` ends a buff with time left whose source area effect is gone or
  whose parent context's activatable ability the owner lacks (`MechanicsContext.
  parent_associated_blueprint`, `BuffTimes.source_area_effect` are new); at load, where the game
  activates Default at the first tick, a buff it would end is reported. The combat replay
  still reports any mode change.
- **WrathOracle** (`ProducerObs.cs`): `cs_lock` at `CommandLockControls.OnRun`/`OnStop` (on, the
  command, the player's cutscene, whether inside `CutsceneController.Tick`, the cutscene whose
  player's `Start` it ran in), declared as `cutscene_lock`; `zone_enter` at `ScriptZone.TryAddUnit`
  (the zone's blueprint reference GUID, the unit, whether it's the main character), declared as
  `zone_enter`. Fields only.
- **Replay** (`elements/cutscene.rs`): a recorded lock runs at its place in the cutscene
  queue (`TickAll` then the lock, or `PostponeAll` then the release); one inside a start waits
  for the sim's start of that cutscene; one elsewhere (a `StopCutscene` action, an unload) is
  reported, as are a lock of an unclassified cutscene, a background cutscene's, and a lock in a
  restored running cutscene (its `Restore` would rerun it at load). Without recorded locks a
  closure's `CommandLockControls` is reported. With them, **every tick's mode must be the
  recording's** (`check_recorded_mode`; `WR_DEBUG_MODES=1` prints each difference), and **Pause
  is taken from the recording** (a UI request, e.g. the tutorial window after Seelah joins: 181
  ticks at the end of every Seelah run): requested before the tick's tasks, no controller runs
  in it, not even the clock.
- **The zone entry at its tick**: the mode check showed that the `zone:` step entered the zone at
  tick 1 while the oracle's walk reached it at tick 116 (the dialogue's records were equal in
  order, but Dialog mode began 115 ticks early). With `zone_enter` recorded, the step is applied
  at the recorded tick (`align_zone`); a different, late or unmade entry is reported, and a zone
  input outside Default and Cutscene modes too.

Result (`poc4-seelah-4`, `cert-22`, `poc4-horgus-8`, new captures with all of the above): **every
Seelah run (15) and every `dialog-checks` run (3) valid**, whole ordered T0 stream equal, end
state 1.0000. Horgus: every run's stream equal (191 to 197 records) and the end state 1.0000;
invalid only on `OnIsInGameChanged`'s group membership and roaming's two draws. Older captures
without `cutscene_lock` now report the lock (test `a_recording_without_locks_reports_the_lock`).
Reviews: gpt-6.1-sol (high), two rounds (the first found the pool order, the controller's
`Activate` and the stale tests; the second, that a recorded Pause must follow `DoStartMode`'s
admission: only over Default and outside turn-based combat, and Default, Dialog, Cutscene and
Rest stop a Pause first).

## Round 10: roaming's producers and the unit groups (2026-10-14, eighteenth session) [M]

The two things that kept the Horgus runs invalid, closed with recorded inputs and checks:

- **Roaming's draws** (round 6's open question). `RoamingWaypointData.SelectNextPoint` draws through
  `LinqExtensions.WeightedRandom` and `SelectPrevPoint` through `LinqExtensions.Random`: their site
  keys name the helper, not the roaming unit [C]. The waypoint they choose is only where the unit
  walks next (T1); what it does on arrival is another choice, `SelectCutscene` [C:
  `UnitRoamingController.TickOnUnit`, `AdvancePoint`]. **WrathOracle** (`ProducerObs.InstallRoaming`,
  declared `roam_producers`) adds to each `roam_point` the candidates' count (`n`), the caller
  (`controller` inside `UnitRoamingController.TickOnUnit`, `advance` inside `UnitPartRoaming.Advance`,
  the innermost) and the roaming unit. **The comparison** (`runner::oracle_stream`) excludes such a
  draw as T1 only when the record just before it is its producer: that method's `roam_point` in the
  same tick, by the controller, for a unit, with candidates, and the draw's range theirs (`Random`:
  `[0, n)`); a producer with candidates and no draw right after it is reported. **The audit**
  (`runner::audit_roaming`) checks each producer against the sim's world: a roaming unit the sim has
  (`UnitPartRoaming`), a waypoint of the scene cache, the next waypoints' count the cache's, and no
  idle cutscene to start (the waypoint's `IdleCutscenes` in the cache and the record, `n` 0;
  `RollNextPoint`'s settings' list empty). Not the controller (`Advance`: the AI's roam, overrun and
  teleport abilities) is reported. At load, a saved roaming state whose next point is a
  `RoamingPoint` of its own with an idle cutscene (it plays on arrival without a draw) is reported,
  for units and placeholders.
- **The unit groups.** `OnIsInGameChanged` drops a unit's cached group (`m_Group?.Remove`); the
  groups model (`combat/groups.rs`, the combat replay's) now runs in dialogue runs whose recording
  declares `group_ticks`: `UnitGroupsController.Start` after the load, its tick in Default, Dialog and
  Cutscene modes before the clock [C: `GameModesFactory`], `HandleUnitSpawned` (`entityData.Group`)
  and `HandleUnitDestroyed` (memory removal, reported where a memory is known). Group memory follows
  vision, so in dialogue runs it is unknown (a read is reported). **WrathOracle**
  (`GroupObs.InstallTicks`) writes `grp_tick` after each `UnitGroupsController.Tick` outside
  turn-based windows when anything changed (the first always): the groups in order (units, combat
  count, leave timer) and every unit of `State.Units` with its cached group. **The runner** checks
  after every groups tick that the sim's state, run-created ids renamed through the recorded awake
  states' matches, equals the latest recorded one (the first check takes the recorded list order,
  as the combat replay does: the load's first `Group` accesses aren't modelled); a record in a tick
  the sim's controller didn't run, or after the run's end, is reported (`WR_DEBUG_GROUPS=1` prints a
  difference). A damaged copy of the recording (a unit dropped from one state) fails 19 ticks.

- **The units pool's order** (found by the group check on Seelah): `State.Units` is one
  `EntityPool` list that appends a unit when it joins any state [C: `EntityPool.HandleAdded`], so
  the recruited companion's new unit comes last; the sim ordered the pool by state (cross-scene
  first). `scene::units_pool` now keeps the load's order and appends units that join a state
  later in join order (`Player::joined_after_load`). The order also drives the buffs controller's
  and the other sweeps, in the combat replay too.
- Run-created ids are resolved through one rule (`RecordedAwake::resolve`: a saved id as is, a
  run-created one through its blueprint match), used by the awake inputs, the group check (before
  renaming, as a unit spawned last tick may not be in an awake record yet) and the roaming audit.

Result on `poc4-horgus-9` (Manual_19) and `poc4-seelah-5` (Manual_13), both recorded with the new
observers: **all 6 Horgus runs valid**, whole ordered T0 stream equal (193 to 199 records), end
state 1.0000, nothing reported, roaming's two draws excluded with their producers, and the three
recorded group states (the load's, the hidden unit leaving at tick 188, the spawned unit joining
at 207) equal; **all 12 Seelah runs valid** with the groups model checked (two states: the load's
and Seelah's new unit joining). With `cert-22`'s 3 `dialog-checks` runs, **every dialogue run is
valid**. Reviews: gpt-6.1-sol (high); the first round found the late id match in the group check,
the roaming audit's id lookup and a lint.

## Round 11: text, and Camelia's first capture (2026-10-14, nineteenth session) [M]

- **Text** (`docs/poc4/text.md`): `crates/wr-text` (packs, the template engine), the sim's
  text environment and the dialogue's text records (`dlg_history`, `dlg_text`), checked on
  every run of the three new captures. All 21 runs valid.
- **Camelia's meeting.** Her trigger is the script zone `CameliaDialogueTrigger`
  (`d34daae6…`, a cylinder of radius 10 centred at (73.06, 36.43, 154.3), once only, the
  player character or Seelah), whose `EnterActions` play the cutscene `Meet_Camelia`
  (`9c848d79…`: six tracks, moves, camera, look-at, an animation, a lock, and
  `CommandStartDialog` of `MeetCamelia`, `e92a8b79…`). The dialogue is a hub: four topic
  answers (`35d480d9…`, `cbde3303…`, `712417ce…`, `ff96f56e…`) return to it, `691061ff…`
  leads to the closing cues (one gated on `AnswerSelected`, the last with `GainExp`). Fixtures
  `camelia-a` (leave at once) and `camelia-b` (two topics, then leave):
  `oracle/tools/oracle-fixtures/poc4-camelia.spec.json`, plan beside it; the sim's
  (`fixtures/poc4.json`) enter the zone with `zone:d34daae6…`. **Recorded** as
  `poc4-camelia-1` (record and replay each, clean records). **Correction (round 12):** that
  capture isn't Camelia's meeting. The walk from Manual_13's start enters Seelah's trigger
  first, so every run played the Seelah/Anevia dialogue (answer 0 throughout) and never reached
  Camelia; the sim's reports came from the fixture's `zone:` step naming the wrong zone.

## Round 12: Camelia, a dialogue started by a cutscene (2026-10-14, twentieth session) [M]

- **Reaching Camelia from Manual_13.** Her trigger lies past Seelah's (the walk enters Seelah's
  zone first) and past `LootFirst` (`25837b80…`, repeating), whose cutscene turns the party back
  until the party picks up the cloak `4aea6773…` from `Prologue_Kenabres_Loot`
  (`PartyAssemble`'s `PartyInventoryTrigger` runs `ScriptZoneDeactivate` on it). The driver can't
  loot yet, so the fixtures are a **development capture**: Seelah's meeting (seelah-a's path),
  `run:120` through the tutorial pause, then `teleport:80,36.5,152` into Camelia's zone (Seelah
  stays behind). A save just before Camelia (Ember's) would make it a regular fixture.
- **WrathOracle** (`ProducerObs.InstallCommands`, declared `cutscene_commands`): `cs_dialog` at
  `CommandStartDialog.OnRun` and `cs_entry` for each entry-condition check in
  `CutscenePlayerTrackData.StartNextCommand` (the command taken in the prefix, written when
  `StartCommand` starts or the frame ends without it; `passed`, `on_fail`), both with `start` and
  `controller` as `cs_lock`. And every mutation made in Pause mode carries its caller chain
  (`origin`, declared `paused_origins`).
- **The sim** (`elements/cutscene.rs`): with `cutscene_commands`, a `CommandStartDialog` without
  speaker or evaluator is a T0 step run where the recording ran it
  (`StartDialogWithoutTarget(Dialog, SpeakerName)`); a failed `SkipCommand` entry check drops its
  command's steps (other results are reported); T0 steps on parallel tracks are accepted, each
  recorded run taking the first step left on its own track (a command on several tracks is
  reported); events inside a player's `Start` wait for the sim's start, whose pending steps are
  now queued before it. The runner: `run:N` (Pause from the recording), `teleport` (T1, reported
  without recorded zone entries), and every recorded zone entry no `zone:` step claims (another
  unit's, the main character moved by a cutscene or a teleport) made at its tick; an entry of a
  unit the sim has inside is reported (exits aren't modelled).
- **Camelia's recruitment, ported:** `ItemEntity.TryIdentify()` for items added to the player
  inventory (each party member's `RuleIdentifyItem`; the tutorial triggers'
  `OnFailedToIdentify` is empty), `AddUnitFactEquipment` activate/deactivate (her amulet's
  feature), `ContextValue` `CasterProperty` for the attributes, `Conditional` in
  `AddFactContextActions` lists with `ContextConditionCasterHasFact`, the `DealDamage` action
  (her 4 slashing damage), `ShowNewTutorial`'s evaluators as part of the tutorial stub.
- **Found: `EtudesSystem` never got its events.** The save schema keeps only interfaces that
  are serialized types, so no entity class had its handler interfaces: `EtudesSystem` never saw
  a flag, a recruit, a party change or a quest objective, and its activation conditions weren't
  re-evaluated after them (Camelia's two in-party etudes stayed inactive). `rulebook/bus.rs`
  `ENTITY_HANDLERS` now lists the entity classes' handler interfaces (from the decompiled
  declarations) and `narrative::entity_event` delivers per method. No other run changed
  (`scripts/regress.sh`: identical to `out-text2`). Facts have the same schema gap
  (`ActivatableAbility` implements eight handler interfaces): to check next.
- **The original's UI in Pause mode:** after Camelia joins, the combat log's
  `IdentifyLogThread.OnFailedToIdentify` copies her amulet for its message, attaching an
  enchantment (a fact id). With `paused_origins`, the comparison leaves out a mutation whose
  origin has a `Kingmaker.UI` frame (`ui_excluded`; player input is neutral in recordings).
  `FollowersFormationController.PrepareFormation`'s draw (a follower's facing) is audited T1.

**The regular fixture:** Ember's `Manual_20_WR_PreCamelia` (Seelah recruited, the cloak looted,
the party just outside Camelia's trigger; hydration 1.0000 against `manual20-dump-1`, nothing
unsupported at load). At load the main character stands in `Camelia_Radius_Check`, whose entry
has no conditions, actions, scene calls or `IScriptZoneHandler` subscribers: such an entry at
load no longer reports the `OnAreaDidLoad` subscribers' order (one with effects still does).
`poc4-camelia.spec.json` walks into the trigger (`camelia-walk-a`, `-b`); recorded as
`poc4-camelia-6` (all four runs clean): **all 4 runs valid** on the first try, the whole stream
(312 and 356 records), end state and units equal. The teleport spec is now
`poc4-camelia-tp.spec.json`.

Result on `poc4-camelia-5` (record and replay of `camelia-a` and `camelia-b`): **all 4 runs
valid**, the whole ordered T0 stream equal (695 and 739 records: both dialogues, the cutscenes'
commands, the recruitment), the end state equal (etudes 1307/1307) and the units 1.0000; one UI
mutation left out per run. Reviews: gpt-6.1-sol (high); the first round found a re-entry
silently merged, a command shared by tracks matched to the wrong one, `cs_entry` lost on
`FinishTrack`, and teleport accepted without zone records; the second, a tutorial evaluator that
draws (`RandomPartyUnit`) skipped, a command after a gate taken before the tracks the gate waits
for, and `DealDamage` defaulting a null type description; the third, `EvaluatedBonus` checked
after the null type description (C# evaluates it first) and entry conditions that may draw
taking the recorded result (now only `UnitInScriptZone` with read-only evaluators, `FlagUnlocked`
and `EtudeStatus` do). All fixed, with tests.

## Recordings (local, gitignored)

| Capture | Fixtures | Runs |
|---|---|---|
| `fixtures/captures/oracle/poc4-seelah-7` (Manual_13) | as `poc4-seelah-5`, with `dialog_text` and the window's speaker state | record, replay-1, perturbed replays (b: 3, c: 2) |
| `fixtures/captures/oracle/poc4-horgus-11` (Manual_19) | as `poc4-horgus-9`, with `dialog_text` | record, replay-1, perturbed |
| `fixtures/captures/oracle/cert-24` (Manual_13, the certification plan) | `dialog-checks`, `dice`, `attack`, with `dialog_text` | record, replay-1, perturbed |
| `fixtures/captures/oracle/poc4-camelia-6` (Manual_20) | `camelia-walk-a`, `camelia-walk-b` (the main character walks into Camelia's trigger) | record, replay-1 |
| `fixtures/captures/oracle/manual20-dump-1` (Manual_20) | the state after load | |
| `fixtures/captures/oracle/poc4-camelia-5` (Manual_13) | `camelia-a`, `camelia-b` (development: Seelah's meeting, then a teleport into Camelia's zone), with `cutscene_commands` and `paused_origins` | record, replay-1 |
| `poc4-camelia-1`, `-3`, `-4` | superseded (`-1` played Seelah's dialogue; `-3`/`-4` predate the final hooks); archived on the external volume, with the two aborted `-2` tries | |
| `poc4-seelah-5`/`-6`, `poc4-horgus-9`/`-10`, `cert-22`/`-23` | superseded (the `-6`, `-10`, `-23` without the window's speaker state); archived on the external volume | |
| `fixtures/captures/oracle/poc4-seelah-5` (Manual_13) | as `poc4-seelah-4`, with `roam_producers`, `group_ticks` | record, replay-1, perturbed replays (b: 3, c: 2) |
| `fixtures/captures/oracle/poc4-seelah-4` (Manual_13, superseded by `-5`) | as `poc4-seelah-1`, with `cutscene_progress`, `cutscene_lock`, `zone_enter`, `aa_awake` | record, replay-1, perturbed replays |
| `fixtures/captures/oracle/poc4-horgus-9` (Manual_19) | as `poc4-horgus-1`, with the same observers and `roam_producers`, `group_ticks` | record, replay-1, perturbed |
| `fixtures/captures/oracle/poc4-horgus-8` (Manual_19) | as `poc4-horgus-1`, with the same observers (superseded by `-9`) | record, replay-1, perturbed |
| `fixtures/captures/oracle/cert-22` (Manual_13, the certification plan) | `dialog-checks`, `dice`, `attack` | record, replay-1, perturbed |
| `poc4-seelah-3`, `poc4-horgus-7`, `cert-21` | the same, without `zone_enter` (superseded; these and `-4`/`-8` archived on the external volume) | |
| `fixtures/captures/oracle/poc4-seelah-1` (Manual_13) | `seelah-a` (Athletics, then `1cfe1ed0…`), `seelah-b` (question `7ce19f7c…`, Knowledge World), `seelah-c` (Diplomacy) | record, replay-1, perturbed replays (a: +8 on d20 #7; b: +8/+14 on #13, +8 on #7; c: +8/+14 on #7) |
| `fixtures/captures/oracle/poc4-horgus-6` (Manual_19) | as `poc4-horgus-1`, with `aa_awake`, `cs_action` and the draw producers | record, replay-1, perturbed (as below) |
| `fixtures/captures/oracle/poc4-horgus-1` (Manual_19) | `horgus-check` (`905c535d…`, check `3ab56408…`, `d641426b…`, `7a580452…`), `horgus-alt` (`5422edfb…`, `db14014b…`, `905c535d…`, `e497fc46…`, `7a580452…`) | record, replay-1, perturbed (+7/+13 on d20 #3) |

Specs: `oracle/tools/oracle-fixtures/poc4-seelah.spec.json`, `poc4-horgus.spec.json`
(plans beside them). They use a new WrathOracle plan step, **`dialogrun[:G1,G2,…]`**
(`mods/WrathOracle/Driver.cs`, additive): answer until the dialogue ends, the first listed
answer shown (each used once) else answer 0, up to 3600 frames per answer, at most 200
answers. Built and installed. The Horgus replays report tape mismatches at T1 sites
(cutscene delays, animation); the oracle also writes a stale `perturbation` note into some
non-perturbed runs' `begin` records (cosmetic, oracle side).

## Exact next steps

1. (Done in round 5.) Remaining: the cutscene player's `SelectTrackToStart`/`SelectPrevPoint` draws.
2. See round 5's next steps.
3. The load-time long tail per save (`coverage.json`), and placeholder spawning's pre-cast buffs
   (`ShieldOfFaithBuff` above: `IsFromSpell` and inactive in the oracle).
4. Camelia's meeting: valid on Ember's Manual_20 (round 12). Left: facts' handler interfaces
   (the save schema's gap, round 12); looting (a driver `loot` step and the sim's container
   interaction, take-all and `PartyInventoryTrigger`) when a fixture needs it. `crates/wr-text`: done (round 11); left: the combat log's,
   kingdom's and UI templates.

## Files

- New: `crates/wr-sim/src/components/{etudes,logic}.rs`, `narrative/{dump,tests}.rs`,
  `runner.rs`, `fixtures/poc4.json`, `tests/poc4.rs`.
- Changed in `crates/wr-sim`: `registry.rs`, `rulebook/bus.rs`, `components/{mod,stats}.rs`,
  `elements/*`, `dialog/{mod,recruit}.rs`, `narrative/*`, `hydrate/{mod,narrative}.rs`,
  `facts/{lifecycle,mod,tests}.rs`, `entity.rs`, `unit.rs`, `world.rs`, `events.rs`,
  `rng.rs`, `rules/checks.rs`, `xp.rs`, `bin/wr-sim.rs`, `Cargo.toml` (`flate2`).
- Wrath-Mac: `mods/WrathOracle/Driver.cs` (`dialogrun`), `tools/oracle-fixtures/poc4-*`.
- Local: `cache/sim/dlc-rewards.json`.
