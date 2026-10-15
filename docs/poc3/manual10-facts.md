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
  as the C# does.
- **Slot items that don't resolve.** What decided Christoff's toggles: `ItemSlot.MaybeItem` reads
  `m_ItemRef.Entity` through `EntityService`, where a loaded item is registered only by its
  `PrePostLoad`. A placeholder's saved body is post-loaded in `SetupUnit` (`BeforeAttachView`),
  after the new unit's `Initialize` activated its facts, so during it the body's slots are empty
  and the empty-hand weapon answers. The sim's hydrated entities were all resolvable; loaded items
  are now unregistered until `pre_post_load` (`World::unregistered_items`), and the new code reads
  slots through `items::hand_maybe_weapon`, `hand_has_weapon`, `limb_slots`. The combat code's own
  slot reads (`combat/weapon.rs`) don't use them yet (other streams own that directory).

## What is left

- **The 17 toggles on sleeping units** (vendors and officers of Drezen: `InspiredRageAllyToggleSwitchBuff`).
  [C] `SleepingUnitsController.ShouldBeSleeping` checks, in order: out of the game, fake,
  `Suppressed`; a companion; then `AwakeTimer`. A unit spawned at load is woken by its creation
  (`CreateEntityData` sets `Position`, whose setter calls `Wake()`: `AwakeTimer = 0`, counted down
  only after that tick), so the camera, cutscene, fog-of-war and command checks after the timer
  don't apply to it: it is awake at the first tick unless something before the timer puts it to
  sleep. The sim now establishes that (`awake_after_spawn`): in Manual_13, 19 and 20 (no area
  parts) every such unit is awake, as the oracle shows (Manual_13's cave monsters, far from the
  party, start their toggles with the same ids as the sim's). [M] Manual_10's area has parts:
  `SuppressEntitiesController` suppresses units outside the current part's `MechanicBounds`, and
  the 17 sleeping units are all far from the party (x ≤ 69 against the party's 185) while the two
  that are awake (the gold golem and Anevia, x ≈ 215) stand by it [I: suppression, not the fog of
  war, keeps them asleep]. The sim has no area-part bounds (a scene's `AreaPartBounds`), so it
  reports suppression as unknown (as before) and now also each unit whose toggle it starts on
  that assumption (`SleepingUnitsController: a toggle acts on a unit taken as awake at load`,
  19 units). Closing it needs the area's part bounds in the scene cache (`wr-import`).
- **16 ids allocated after the load** [M]: the oracle's controllers restart toggles in their
  first ticks (`UnitActivatableAbilitiesController` → `TryStart` → `ReapplyBuff`) for 13 party
  members' toggles the sim keeps started from the save (Lann, Woljif, Ember, Ulbrig, Sosiel,
  Greybor, Trever), after three area effects spawn (`AddAreaEffect.OnAreaActivated` and the view's
  `HandleCharacterParticleSnapMapUpdated`), and the main character's `LifeBondingFriendshipEffectBuff`
  comes from an area effect's unit-enter actions. Why those toggles stopped (they are on and were
  started when saved) isn't established yet: the next step, with the area effects (the second is
  driven by the view).
- `facts.order` (19 units) and the stat differences left (difficulty and a few components) are
  unchanged by this work; Hurlun's difficulty modifier (-29 against -25) predates it.

## Observer changes requested

None needed for what was done. For the next steps: an IdTrace-style record of who calls
`ActivatableAbility.Stop` during a load (with the call chain) would show why the party's toggles
restart; and a dump of `Suppressed`/`IsInFogOfWar`/`AwakeTimer` per unit at the first
`SleepingUnitsController` tick after the load would confirm the suppression inference above.
