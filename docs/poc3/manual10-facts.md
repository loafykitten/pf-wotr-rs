# Manual_10: the facts on one side only, and the fact ids

Written 2026-10-15 (stream E of the twenty-second session; PLAN's "Next" item 3). Scope: the
facts the sim and WrathOracle's state dump of Manual_10 disagree on after the load, and the
unique ids the load allocates. Tags as in `docs/poc3/hydration.md`: **[M]** measured, **[C]** read
in the decompiled code, **[I]** inferred.

## Result [M]

`wr-sim hydrate Manual_10` against `state-manual10-after-load.json.gz` (the regression's compare):

| | before (`out-facthandlers`) | after |
|---|---|---|
| match rate | 0.9099 | **0.9987** |
| facts on one side only | 35 | **17** (all reported, see below) |
| `fact.id` equal | 4,149 / 21,882 | **21,884 / 21,900** |
| `stat.modifiers` equal | 15,098 / 20,482 | 20,440 / 20,482 |
| `facts.order` equal | 518 / 539 | 520 / 539 |
| unsupported kinds at load | 224 | 209 |

Every allocation the load makes now lines up with `manual10-ids-1`'s `IdTrace` up to the end of
the load: the 16 ids still different are allocated after it (below). Most of the `stat.modifiers`
gain is the modifiers' source-fact ids, which follow the fact ids. The regression against
`cache/regress/out-facthandlers` changes nothing else: Manual_13, 19, 20 and 18 (combat) compare as
before, every dialogue run's log is unchanged but for its output path, and Manual_10's compare is
the only section that differs.

## How the misses were found

`WR_DEBUG_IDS=1 wr-sim hydrate <save>` now prints each unique id the sim allocates with its call
chain (`IDTRACE <id> fn@file:line < …`, from the release build's line tables). Aligning it with
WrathOracle's `ids-load.json` on the facts both final dumps hold (unit, blueprint, occurrence) as
anchors gives, between consecutive anchors, the oracle's and the sim's allocations side by side:
each gap names the C# that allocated and the sim code that didn't (or did when it shouldn't). The
scripts are throwaway (they live in the session's scratchpad); the method is all there is to them.

## What was missing, and what is ported [C]

In load order:

- **`ShiftersFury`** (the main character's three `ShifterFuryWeaponList` toggles). The fact's
  `OnTurnOn` rebuilds its toggle list from the part's saved `State` (`ShiftersFuryPart.RestoreState`:
  one toggle per natural, non-unarmed weapon in the current hands and the additional limbs; the
  saved selection is turned on again when the count and weapon match). Ported with the part
  (`components/shifters_fury.rs`): `Setup` (turn-on priority 1, turn-off -1), `RestoreState`,
  `SaveState`, `RecreateListItems`, and the part's own event handlers. Unit parts didn't subscribe
  to the bus in the sim; a part kind can now (`Sub::Part`, subscribed in `EntityPart.TurnOn`,
  `rulebook/bus.rs`), and only `ShiftersFuryPart` does so far.
- **`BuffEnchantSpecificWeaponWorn`, `BuffEnchantAnyWeapon`** (Seelah's and Regill's weapon
  enchantments): turning on removes the enchantments the buff holds (saved) and adds them again
  with the buff's context as `ParentContext` (`ItemEntity.AddEnchantment`/`RemoveEnchantment`,
  `items.rs`; the enchantment's context is the parent's `CloneFor` on activation;
  `m_HasExternalEnchantments` is kept for stacking). `BuffEnchantAnyWeapon`'s
  `HandleEquipmentSlotUpdated` is ported too. An item has no `ItemEnchantmentCollection` until its
  constructor, `OnPostLoad` or `OnDidEquipped` makes one, so a placeholder's saved weapon gets
  nothing while the new unit's facts activate (the oracle allocates nothing there).
- **`ContextActionApplyBuff`** (Mephisto's `MonsterMythicFeature1Buff` child buff, applied and
  removed twice; the Kitsune officer's bite buff): the action in a mechanics context, the target or
  the caster, the parent context cloned (or none with `IgnoreParentContext`), `AsChild` storing the
  buff in the enclosing `Buff.Data` buff. `Buff.OnRemove` now removes stored facts (saved
  `m_StoredFacts` are hydrated; saved stored modifiers are reported when the buff goes).
  Durations from context values (`DurationValue`), `SameDuration` and `IsNotDispelable` are
  reported.
- **`AddFeatureOnSkill`** (the Kitsune officer's two facts at a level-up).
- **`AddFeatureOnApply`** (the mounts Seilkind and Ciar). Its `UpdateProgression` runs on
  `Game.Instance.LevelUpController?.State`, which `AddClassLevels.AddLevel` sets to its own
  controller. The level-up's state is now reachable from the world (`World::level_up`,
  `spawn/levelup.rs`): `AddLevel` pushes a live slot, `UnitHelper.Copy` a null one, and the
  level-up lends its state to the world around the calls that activate facts (`lend`: adding a
  feature, the race, the class progression, `IOwnerGainLevelHandler`). A component that needs
  the state while the level-up's own code holds it reports that.
- **Targeted toggles' `SelectTargetAbility`** (`MountTargetSwitch`): `ActivatableAbility.OnActivate`
  adds it, `OnDeactivate` removes it; saved, and hydrated.
- **`BuffExtraEffects`** (Ciar's `MountedCombatBuff`; also 6,239 `IUnitBuffHandler` reports gone).
- **`BuffOnEntityCreated`** (the Herald's buff, at the placeholder's `BecomeUnit`).
- **`ActivatableAbility.IsAvailable`** (Christoff's two Sword Saint toggles started in the sim
  only, and their buffs reapplied at the level-up). The sim answered true and reported every
  restriction and resource; now `IsAvailableByResources` (`ActivatableAbilityResourceLogic`: free by
  a fact, the resource's amount, judgment's switch) and `IsAvailableByRestrictions` (`OnlyInCombat`,
  `IgnoreIfStarted`, `RestrictionKensaiWeapon`) are computed; other restriction types, a source
  item's charges and a polymorph buff are reported. `OnNewRound` now uses `IsAvailableByRestrictions`
  as the C# does, and `Stop` turns a toggle off when its restrictions fail. Spending the resource
  (`OnStart`, rounds, attacks) isn't ported and is reported (the content schema doesn't list
  `IActivatableAbilitySpendResourceLogic` as a handler, so the old guards never fired).
- **Slot items that don't resolve.** What decided Christoff's toggles: `ItemSlot.MaybeItem` reads
  `m_ItemRef.Entity` through `EntityService`, where a loaded item is registered only by its
  `PrePostLoad`. A placeholder's saved body is post-loaded in `SetupUnit` (`BeforeAttachView`),
  after the new unit's `Initialize` activated its facts, so during it the body's slots are empty
  and the empty-hand weapon answers (`HandSlot.HasWeapon` is `MaybeWeapon != null`). The sim's hydrated entities were all resolvable; loaded items
  are now unregistered until `pre_post_load` (`World::unregistered_items`), and the new code reads
  slots through `items::hand_maybe_weapon`, `hand_has_weapon`, `limb_slots`. The combat code's own
  slot reads (`combat/weapon.rs`) don't use them yet (other streams own that directory).

## Area parts and suppression (stream R, same session)

`wr-sim hydrate Manual_10` against the same dump [M]:

| | before (stream E) | after |
|---|---|---|
| match rate | 0.9988 | **0.9989** |
| facts on one side only | 17 | **0** |
| `fact.id` equal | 21,884 / 21,900 | 21,884 / 21,900 (the 16 below) |
| `facts.order` equal | 520 / 539 | **537 / 539** |
| unsupported kinds at load | 205 | **201** |
| unsupported reports at load | 51,339 | 49,922 |

(Stream Q's merge had moved E's numbers slightly: 0.9988 and 205 kinds are the base this was
measured from.) The kinds gone: `SleepingUnitsController: suppressed units (area parts)` (430
reports), `EntityCreationController.ShouldSpawnOptimizedDummy: area part bounds` (488),
`UnitActivatableAbilitiesController: area unit with a toggle to tick (AwakeUnits)` (411) and
`SleepingUnitsController: a toggle acts on a unit taken as awake at load` (19).

**The rule** [C]: an area with parts (`BlueprintArea.HasParts`) loads one of them as
`SceneLoader.CurrentlyLoadedAreaPart`, which `AreaService.CurrentAreaPart` answers. Loading a save
(`LoadAreaCoroutine`, no area loaded before it, no enter point) takes the save's `AreaPart` (the
player's `CurrentAreaPart`), or the area, and corrects it to
`AreaService.FindLocalMapBoundsContainsPoint(area, main character's position)`: the area if its own
`LocalMapBounds` contain the point in x and z (`GeometryUtils.ContainsXZ`, 0.1 of slack), else the
first part whose do, else the closest by `ManhattanDistance2D` (the area first among equals).
`SuppressEntitiesController` is the first controller of every game mode's frame; `Activate` sets
`m_UpdateAll`, so its first tick after the load evaluates every suppressible entity:
`Suppressed = HoldingState != CrossSceneState && !MechanicBounds.ContainsXZ(Position)` (`m_Suppressed`
isn't saved, and nothing else sets it). `SleepingUnitsController`, later in the same frame, puts a
suppressed unit to sleep before its `AwakeTimer` is read, so a unit spawned at load outside the bounds
is asleep at the first tick despite the wake its creation gave it, and the activatable-abilities
controller doesn't tick it. `EntityCreationController.ShouldSpawnOptimizedDummy` gives a unit spawned
into the area's state outside the bounds (3D `Bounds.Contains`, no slack) the optimization dummy's view.

**The data** [C]: each part's bounds are an `AreaPartBounds` asset, not a blueprint; the runtime pack
holds `m_Bounds` as an index into `BlueprintReferencedAssets`. `wr-rules-data` now also extracts every
`AreaPartBounds` that list holds in `blueprint.assets` (544; default, mechanic and local-map bounds with
their overrides) into `cache/sim/area-bounds.json`, by index (`crate::area_parts::AreaBoundsTable`). A
scene cache wasn't needed: the bounds aren't scene data. Rerun `wr-rules-data` to write the file.

**In Manual_10** [M]: the area is `DrezenCapital`, whose own bounds asset is `DrezenThroneRoomBounds`
(mechanic bounds centred at x ≈ 200, z ≈ -2, 24 each way); the main character (x 185.19, z -7.8) is in
its local map, so the loaded part is the area itself, as saved. The 17 sleeping vendors and officers are
outside, the gold golem and Anevia inside: the sim now starts exactly the toggles the oracle starts, with
no unit's awake state assumed.

**Computed** (`crates/wr-sim/src/area_parts.rs`): the loaded part (`establish`, before the area's states
load; a part other than the saved one is reported, since the etudes still read the saved one), suppression
at the first tick (`suppressed_at_load`, in `spawn::awake_after_spawn`'s `ShouldBeSleeping`), and the dummy
view of placeholders and spawners (`outside_mechanic_bounds`). `Bounds.Contains` is `wr-nav`'s port, checked
against the player; `ContainsXZ` and `ManhattanDistance2D` follow Mono's double evaluation. A missing
table, an index the table doesn't hold or a zip-sourced reference fails closed (the old reports). Still
reported: suppression in later frames (`SuppressEntitiesController` re-evaluates only entities that moved
or joined a state, at the start of the frame; the sim doesn't follow that), the entity pool's suppressed
entities (`EntityPoolEnumerator`, in later frames), and `UpdateHasDummyView`'s later view swaps.

## Companion spawners and the capital mode (stream drezen, 2026-10-17)

`wr-sim hydrate Manual_10` against `state-manual10-after-load.json.gz` (the regression's compare) [M]:

| | before (branch point, no Drezen scene cache) | scene cache only (old code) | after |
|---|---|---|---|
| match rate | 0.9989 | 0.9989 | 0.9989 |
| facts on one side only | 0 | 0 | 0 |
| `fact.id` equal | 21,884 / 21,900 | 21,884 / 21,900 | 21,884 / 21,900 (below) |
| `facts.order` equal | 537 / 539 | 537 / 539 | **538 / 539** (Greybor) |
| ids the load allocates (oracle: 18,518) | 18,501 | 18,501 | **18,514** |
| unsupported kinds at load | 201 | 208 | 208 |

**The ids** [M]: the sim now allocates the 15 `ActivatableAbility.ReapplyBuff` ids of the first tick
in the oracle's order and with its owners: Lann (`420D`: 3), Woljif (`9960`: 2), Ember (`B909`),
Ulbrig (`121C0`: 2), Sosiel (`21B5A`), Greybor (`7D739`: 3), Trever (`1C02EB`), then `5FA03` and
`6241B`, the 13 restarted companion toggles first (`1E699D`..`1E69AB` in the sim, `1E69A0`..`1E69AE`
in the oracle). Each is 3 lower than the oracle's: the three area-effect ids before them aren't
allocated (What is left), so `fact.id` counts the same 16 differences. Before, the 13 companions'
toggle buffs kept their saved ids and `5FA03`/`6241B` came 13 ids early.

**What the load does** [C]:

- `SceneLoader.LoadSceneEntities` attaches each `CompanionSpawner.MyData` to its view:
  `UnitSpawnerBase.MyData.OnViewDidAttach` runs its `IUnitInitializer` parts' `OnInitialize` for the
  spawned unit (`SpawnerInteractionPart`: the unit's `UnitPartInteractions`) and, when the unit
  stands where it was spawned (`(SpawnPosition.To2D() - Position.To2D()).magnitude < 0.1f`), moves it
  to the view's exact position. A saved position is rounded to hundredths, so the position changes
  (`Vector3 !=`) and `UnitEntityData.Position`'s setter wakes the unit (`AwakeTimer = 0`).
- The views subscribe to the event bus as their scenes load (`EntityViewBase.OnEnable`), after the
  player's state turned on and before the area's states are post-loaded.
- `ProcessSpawnerActivationCoroutine`, after the states are on: every spawner data of
  `Game.State.Entities` (the pool's enumerator skips data out of the game; `m_Suppressed` is false,
  `SuppressEntitiesController` hasn't ticked) whose `ShouldProcessActivation(false)` holds (always for
  a companion spawner; for a unit spawner, `SpawnOnSceneInit && !HasSpawned`), sorted by its
  blueprint's `Prefab.AssetId` (`List.Sort` with an ordinal comparison: Mono's introsort, ties
  included, capacity from `ToList`), gets `HandleAreaSpawnerInit`. A companion spawner that controls
  its companion places it (`PlaceCompanion`): capital mode is off (`RuntimeAreaSettings.CapitalPartyMode`
  isn't saved), the seven `DefaultPosition` spawners show their companion only in capital mode, and
  a `Remote` companion isn't controllable in the party, so each leaves the game:
  `OnIsInGameChanged` deactivates its buffs and toggles (`Stop`: the toggles' buffs removed).
- `Game.AfterSceneLoaderFinishedArea` → `UpdateEtudes` → the etude `CapitalCompanionLogic`'s bracket
  enters (`OnEnter`, `m_RestAllRemoteCompanions` set): `PlaceAllCompanions` turns capital mode on
  (`SetCapitalMode`: the `CountingGuard` changes, `IPartyHandler.HandleCapitalModeChanged`), and each
  spawner that can control its companion (`CanControlCompanion`) places it at its position, back in
  the game: its buffs and toggles activate, the toggles on but not started. The remote companions
  without a spawner would leave the game (none here); every inventory item tries to be identified by
  the remote companions and the main character; the rest of remote companions isn't due (they rested
  18 seconds before the save); `ISafeZoneHandler.OnEnterSafeZone` is raised.
- The first tick's `UnitActivatableAbilitiesController` ticks the awake units in `State.Units` order:
  the companions are awake through the wake above (`ShouldBeSleeping` reads `AwakeTimer >= 0` before
  the fog of war; a cross-scene unit is never suppressed), so their toggles start (`TryStart` →
  `ReapplyBuff`: new buffs), before the units spawned at load.

**The data**: Drezen's scene cache (`scripts/scene-cache.sh DrezenCapital
2570015799edf594daf2f076f2f975d8`; the cache now keeps a `CompanionSpawner` view's components: its
settings and the `SpawnerInteraction*`, `SpawnerAdditionalActions`, `SpawnerCustomCutscene`
components), the spawner data (`DidSpawnOnce`, `m_IsInGame`, the saved parts) and the companions'
`UnitPartCompanion.m_Spawner`, `IsIgnoreFixInGame`, `SpawnPosition`, `Descriptor.LastRestTime` and
the pets' `UnitPartPet.Type` and masters' `UnitPartPetMaster.m_Pets`.

**Ported** (`crates/wr-sim/src/companion_spawner.rs`): `CompanionSpawner` (`ShouldShowUnit`,
`CanControlCompanion`, `IsControllingCompanion`, `GetMyCompanion` with pets, `BeginControllingCompanion`,
`PlaceCompanion`, `MyData.OnPlaceCompanion`/`OnSpawned`/`Clear`, `HandleAreaSpawnerInit`, `Spawn`, and its
`IPartyHandler`/`ICompanionChangeHandler` handlers), `UnitSpawnerBase.MyData.OnViewDidAttach` for
companion spawners, `ProcessSpawnerActivationCoroutine`, `SetCapitalMode`, `CapitalCompanionLogic`
(`OnEnter`, `OnResume`, `OnExit`), `Player.AddCharacterToLists` in capital mode (only the main
character, and its pets no spawner controls, are in the party), `ItemEntity.TryIdentify()` in capital
mode, `UnitPartCompanion.HandleObjectInGameChanged`, and the empty `HandleCapitalModeChanged` of
`EtudesSystem` (it was matched under the wrong interface, which would have marked the etudes'
conditions dirty), `DisableMountRiding`, `AddFeatureToPet` and `HideAllPets`;
`EtudeBracketSetCompanionPosition.HandleCapitalModeChanged` enters in capital mode. The first tick's
start pass (`spawn::after_area_load`) now runs over `State.Units` order on the units spawned at load
and the loaded units the load woke.

**Inferred** [I]: Unity's enable order within a scene (the order the capital mode reaches the
spawners) is taken to be the scene cache's object order; on Manual_10 the seven spawners placed
their companions in that order (WrathOracle's `in_game` events) [M]. The order matters only when two
spawners can take the same companion.

**Still reported**: a unit spawner that would spawn at load (none in the slice's saves), a pet's
`UnitPartPet.ShouldBeHidden` (`HideAllPets` isn't ported; on Manual_10 the main character's dragon
spawners ask: `CompanionSpawner.ShouldShowUnit: a pet`, and one of their show conditions,
`UnitIsInAreaPart`), placing a companion's pets, a spawn's ground placement
(`UnitPlaceOnGroundController.ForcedTick`), custom cutscenes at spawn, settings components without a
port, the respawn of a dead companion, `HandleCompanionRemoved` (its `stayInGame` argument isn't in
the sim's event call), `HandleUnrecruit` when it would act, `AttachPartyMember` for a detached
companion, and the remote companions' rest. `UnitSpawnerBase.MyData.OnViewDidAttach` of *unit*
spawners (their units' interactions and position) is still not modelled, as before. Each
companion's two changes of `IsInGame` add to the existing `UnitEntityData.OnIsInGameChanged: group
membership` reports (58 -> 72: `m_Group` isn't saved, and whether something read it before is
unknown). Every unit's `IsInGame` after the load equals the oracle's dump [M].

**The other reports that changed** [M]: the companions' buffs deactivate and activate again, so
their unported components' hooks (`ActivationDisable`, `BuffOnHealthTickingTrigger`,
`SuppressBuffs`, `UnitHealthGuard`, a `ContextRankConfig`) and the event-bus handlers that see the
buffs go and come (`EnduringSpells: IUnitBuffHandler`, `RecalculateOnFactsChange`) report more; 21
fewer `EntityPoolEnumerator` reports (the seven companions are out of the game when the three zones'
`OnAreaDidLoad` enumerate the units); `CapitalCompanionLogic.OnEnter` and `area scene entity not
hydrated: MyData` (72) are gone. Building the scene cache alone (the old code) changes the reports
too: `SceneLoader: no scene cache`, `ScriptZone.OnAreaDidLoad: scene settings unavailable` and
`action Spawn (no scene cache…)` go, and the scene's map objects' parts that aren't ported
(`EntityDataBase.OnViewDidAttach: … removed`, `DestructionComponent`, a loot table) and the
suppression enumerator are reported instead.

## What is left

- **4 ids allocated after the load** [M, `manual10-ids-2`]: the three `AddAreaEffect` area effects
  (`AreaEffectsController.SpawnAttachedToTarget`: twice from the view's
  `HandleCharacterParticleSnapMapUpdated`, between the last placeholder's unit and the spawner
  activation, and once from `OnAreaActivated`, before the first tick) and the main character's
  `LifeBondingFriendshipEffectBuff`, applied by an area effect's unit-enter actions
  (`AbilityAreaEffectRunAction.OnUnitEnter` in `AreaEffectsController.Tick`). Whether `AddAreaEffect`
  spawns depends on the owner's view (`IsOwnerAvatarValid`: the view's `ParticlesSnapMap`, built
  by the character avatar on its own schedule), which is presentation the sim can't know; and the
  area effects themselves (`AreaEffectEntityData`, its ticks and unit enter/exit) aren't modelled.
  They stay reported (`AddAreaEffect.OnActivate`, `AddAreaEffect: IAreaActivationHandler`,
  `AddAreaEffect: IAreaHandler`). Computing them needs, from the oracle, which units' views had a
  snap map when (an `IdTrace` field naming the `AddAreaEffect` buff and owner of each spawn would
  pin the three ids), and a port of the area effect entity. Until then every id after the first of
  them is 3 lower in the sim (the 15 restarted toggles' buffs, below).
- **In `crates/wr-sim/src/combat/` (another stream's, so not changed here)**: `AddAdditionalLimb`
  fills a limb slot directly, without `ItemSlot.InsertItem`'s `HandleEquipmentSlotUpdated`, so a
  limb added while `ShiftersFuryPart` is on or a `BuffEnchantAnyWeapon` buff is active isn't seen
  by them (at Manual_10's load the part turns on after the limbs, so nothing is missed there); and
  `create_weapon` makes a limb weapon turned on with its facts off (an enchantment added to it is
  reported). The combat code's slot reads don't resolve items through registration either.
- `facts.order` (19 units) and the stat differences left (difficulty and a few components) are
  unchanged by this work; Hurlun's difficulty modifier (-29 against -25) predates it.

## Observer changes

Stream R extended `IdTrace` (schema `wrath-oracle-ids-v2`: the ids gain `seq`, and `events` holds the
`aa_stop` and `in_game` records in the same sequence, field reads only) and recorded `manual10-ids-2`
(plan `dump-manual10-ids.json`, as for `manual10-ids-1`). The per-unit `Suppressed`/`AwakeTimer` dump E
proposed wasn't needed: the rule is read in the code, and its prediction matches the oracle's dump on
every unit (each toggle the oracle starts and each it doesn't).

## Review

gpt-6.1-sol (high) reviewed the branch in five rounds until it found nothing: runtime-component
lookups (`SelectComponentsWithRuntime`, `GetComponent`'s fallback without runtime providers),
`HandSlot.HasWeapon`, items resolved by live registration, part disposal on every removal,
`Stop`'s restriction clause, resource spending reported over the fact's runtimes, and the
context's owner as the target of fact context actions came from it. Verified after the last
round: `scripts/check.sh` (531 tests), the ignored real-save and POC-5 suites (33 tests), and the
regression above.
