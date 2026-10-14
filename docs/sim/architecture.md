# wr-sim architecture

Written 2026-10-11 (POC-3, second round). `crates/wr-sim` is the game simulation: plain
Rust, no Bevy, deterministic, headless (Decision 1). This page says how it is built and
which C# each part ports. Tags: **[M]** measured, **[C]** read in the decompiled code (paths
are under `decompiled/Assembly-CSharp/` unless said otherwise), **[I]** inferred,
not yet checked against the running game. Hydrating Manual_13 with it is in
[`docs/poc3/hydration.md`](../poc3/hydration.md).

## Shape

One `World` holds everything, in arenas indexed by small integers: `entities` (units,
items, the rest; each keyed by its durable `UniqueId` string, never a Bevy `Entity`),
`facts`, and `mods` (stat modifiers). Every C# method that mutates the object graph is a
function taking `&mut World` and indices. That is what lets the C#'s re-entrant call chains
run unchanged: `AddModifier` → `UpdateValue` → dependents' `UpdateValue` → `OnChanged` →
the AC's `CheckAttribute` → `AddModifier` on the AC, in the same order and with the same
guards (`m_UpdateInternalModifiers`, `m_UpdateOnlyBase`, `m_UpdateDependentFacts`). Shared
C# references (a modifier in a stat's list and in the AC's `m_BaseAttributeAppliedBonus`,
a fact referenced from modifiers and from `AddFactsData.AppliedFacts`) are one arena
entry referenced twice. Object identity where the C# compares references (item
collections, dex limiters) is kept as an identity value from the save.

| Module | Ports | C# |
|---|---|---|
| `csharp` | integer wrap and C# division/remainder exceptions; Mono's float model (evaluate in double, round on store); arm64 float→int casts; `Math.Round` ties; `List<T>.Sort` (Mono's introsort, unstable, depth limit from the list's *capacity*); `Dictionary` enumeration order (free-slot reuse); `TimeSpan.FromSeconds` (rounds to whole ms) | mscorlib's `ArraySortHelper<T>`, `IntrospectiveSortUtilities`, `List<T>`, `Dictionary<K,V>`, `TimeSpan`, read from the shipped `Managed/mscorlib.dll` with `ilspycmd` [C] |
| `rng` | one `Rng` trait for every draw; `Tape` (replay; fails on a draw from another site or range, an extra draw, or draws left at `finish`); `PlayRng` (our own PCG32); `dice()` = `RulebookEvent.Dice.D` | `RuleSystem/RulebookEvent.cs:70` [C]; call-site keys are WrathOracle's (`rng-sites.json`) |
| `time` | `SimClock::tick` = `TimeController.Tick`: `min(scale*debug/12, Δ)`, game time only in Default/Cutscene, turn-based scale capped at 3 and restored, `GetDeltaTimeForFrames` | `Controllers/TimeController.cs` [C] |
| `world`, `entity`, `unit`, `item` | entity lifecycle (`PrePostLoad`, `PostLoad`, `ApplyPostLoadFixes`, `TurnOn`), unit and item overrides on the load path | `EntitySystem/EntityDataBase.cs:295–430`, `Entities/UnitEntityData.cs:1607–1720`, `UnitLogic/UnitDescriptor.cs:417–520`, `Items/ItemEntity*.cs` [C] |
| `facts` | `Attach`, `Activate`, `Deactivate`, `TurnOn`, `TurnOff`, `Reapply`, `Detach`, `Dispose`, `PostLoad`/`PostLoadComponents`, `ApplyPostLoadFixes`/`Setup`, the manager's passes (turn-on sorted by `TurnOnPriority` with Mono's unstable sort), `UpdateIsActive`, `IsEnabled` | `EntitySystem/EntityFact.cs`, `EntityFactComponent.cs`, `EntityFactComponentDelegate.cs`, `EntityFactsManager.cs` [C] |
| `context` | `MechanicsContext` saved state (ranks, shared values, params, caster defaulting to owner per `[OnDeserialized]`); `Recalculate` | `UnitLogic.Mechanics/MechanicsContext.cs:316,411–470` [C] |
| `stats` | `ModifiableValue` and all nine subclasses: `UpdateValue`, `ApplyModifiersFiltered` with every filter, `AddModifier` (duplicate removal, sort), `RemoveModifier` (value-equality `List.Remove`), `RemoveModifiers`, `RemoveModifiersFrom`, `AddModifierUnique`, `CleanupModifiers` (including the both-entries duplicate removal and the `SelectComponents` predicate quirk), `CharacterStats.PostLoad` (bind all, then update all, in `CollectAllStats` order) | `EntitySystem.Stats/*.cs`, `Enums/ModifierDescriptor*.cs` [C] |
| `rulebook` | `Rulebook.TriggerEvent` (nesting, the 25-deep overflow and context reset, "can't trigger twice"), the six steps of `TriggerEventInternal`, `RulebookEventBus` managers (global → target → initiator, each walking the rule's type chain), hooks, `SubscribersList` (deferred removal while executing), `EventBus` global and unit lists, `OnDidTrigger` (temporary modifiers) | `RuleSystem/Rulebook.cs`, `RulebookEventContext.cs`, `RulebookEvent.cs`, `PubSubSystem/RulebookEventBus.cs`, `RulebookSubscriptionManager.cs`, `RulebookSubscribersList.cs`, `SubscribersList.cs`, `EventBus.cs`, `RulebookEventHooksManager.cs` [C] |
| `items` | run-time item creation, the unit body and its slots, `ItemsCollection`, equip and unequip (below) | `Items/ItemsEntityFactory.cs`, `ItemEntity*.cs`, `UnitBody.cs`, `ItemsCollection.cs`, `Items.Slots/*.cs` [C] |
| `spellbook`, `resources` | `Spellbook` (class levels and caster level, known / special / custom spells, `m_KnownSpellLevels`, memorized slots with the hidden-slot dictionaries, `UpdateSlotsSize`, `Memorize`, `Rest`, `PostLoad` with `TryRestoreKnownSpells`, `ApplyPostLoadFixes`, special lists, `UnitHelper.CopySpellbook`), `SpellSlot.SpellShell`; `UnitAbilityResourceCollection` (`Add`, `Remove`, `Restore`) and `BlueprintAbilityResource.GetMaxAmount` with the `IResourceAmountBonusHandler` event | `UnitLogic/Spellbook.cs`, `SpellSlot.cs`, `UnitDescriptor.cs`, `UnitHelper.cs`, `UnitAbilityResource*.cs`, `Blueprints/BlueprintAbilityResource.cs`, `Blueprints.Classes.Spells/*.cs` [C] |
| `armor` | `ItemEntityArmor.RecalculateStats` and its rules `RuleCalculateArmorCheckPenalty`, `RuleCalculateArmorMaxDexBonusLimit` | `Items/ItemEntityArmor.cs`, `ItemEntityShield.cs`, `RuleSystem.Rules/RuleCalculateArmor*.cs`, `Designers/GameHelper.cs` [C] |
| `registry`, `manifest` | fail-closed component registry, coverage (executed and unsupported, per unit), required-versus-implemented manifest | Decision 7 |
| `components` | ported blueprint components (list in hydration.md) | each file cites its C# |
| `hydrate`, `sheet`, `compare` | save → world, the post-load lifecycle, the oracle-format dump and the diff | below and hydration.md |

## Facts and per-fact component state

A `Fact` keeps what `EntityFact` saves (`IsActive`, `UniqueId`, `AttachTime`, rank, source
item/fact, buff times, the context) and what it doesn't (`IsTurnedOn`, `Activating`,
`Deactivating`, `IsReapplying`, `RecalculateContextBeforeTurnOn`, subscription state). Its
`components` are the runtimes: the saved `ComponentsDictionary` order, each with its
`SourceBlueprintComponentName`, its source blueprint component once `Setup` ran, and its
saved `m_Data` (the delegate's `TData`) as JSON, plus the facts that data references
(`fact_refs`). Blueprint components are shared and stateless, as in the C#, where
`EntityFactComponentDelegate` reads `Owner`/`Fact`/`Data` from the current runtime: here a
component port receives the `CompRef` of the runtime.

The load rules from the lifecycle trace hold, and are tests:

- **`IsActive` is saved, `IsTurnedOn` isn't.** A loaded active fact gets `PostLoad` then
  `TurnOn` (and its components `PostLoad`/`TurnOn`), never `Activate`.
- `PostLoadComponents` matches saved runtimes to `BlueprintComponents` (the
  `ComponentsList`-expanded list) **by name**, disposes runtimes whose name is gone (or
  whose delegate rejects the type), and drops the disposed.
- A null dictionary entry creates a runtime only for an enabled runtime-provider component.
- `ApplyPostLoadFixes` disposes disabled components and runs `Setup(Blueprint)`, which
  creates runtimes new in the blueprint and `Initialize`s and `Activate`s them when the fact
  is active: the one activation on the normal load path.
- The manager turns facts on in `TurnOnPriority` order using Mono's unstable introsort, so
  ties are *not* in list order (test `turn_on_order_follows_monos_unstable_priority_sort`).
  The depth limit uses the list's capacity; `ToTempList` may raise it with a pooled
  list's capacity, which only matters if the heapsort fallback would run, and the sort
  reports that (it hasn't happened).

## Fail closed (Decision 7)

`tools/wr-codegen` now emits, for every content type, whether it is an
`IRuntimeEntityFactComponentProvider` (`type_flags::RUNTIME_PROVIDER`), which fact-component
lifecycle methods it overrides with a body (`fact_hooks`: `OnInitialize` … `OnPreSave`,
plus `CUSTOM_RUNTIME` for providers with their own runtime class, including delegates that
override `CreateRuntimeFactComponent`), and which subscriber interfaces it declares
(`handlers`: every declared interface deriving from `ISubscriber`, with generic arguments).
Interfaces are taken from the declarations, not closed transitively: a handler interface
reached only through another interface's inheritance would be missed [I: none found on
the fixtures' path]. The content schema
was regenerated (types and fields unchanged; the fingerprint changed, so the content pack
was rebuilt). The dispatcher then decides per hook:

- the C# type doesn't override it → a correct no-op (the delegate base's empty virtual;
  `OnRecalculate`'s default runs `OnTurnOff` then `OnTurnOn`);
- it does and the port implements it → run the port, count it as executed;
- otherwise → an **unsupported execution**: the run is invalid (recorded per unit).

The same holds for events: a subscribed component that handles an interface or rule the
port doesn't is reported when the event fires. Lifecycle stages of facts, parts and items
that aren't ported report the same way (`Feature`/`Buff` fact processors on attach,
part hooks that the save schema's hook table shows have a body, item
`ApplyEnchantments` repairs…). On top of execution coverage, the manifest lists every
component type on the loaded facts against what is ported, so gaps in code that didn't run
are visible too.

## The rulebook

`trigger(w, &mut rule)` follows `Rulebook.TriggerEvent`. Rules are Rust structs implementing
`RuleEvent` (their C# type chain, initiator, target, `OnTrigger`, base state); handlers get
`&mut dyn RuleEvent` and downcast. Subscriptions come from the declared interfaces:
`IInitiatorRulebookHandler<T>` subscribes to the initiator manager for `T`, and so on;
`IBeforeRulebookEventTriggerHandler<T>`/`IAfterRulebookEventTriggerHandler<T>` and the two
hook interfaces go to their own lists. The order (tests in `rulebook/tests.rs`):

```
Before hooks → about-to hooks → global → target → initiator (each over the rule's type chain)
→ OnTrigger → After hooks → global → target → initiator (did) → did hooks → OnDidTrigger
```

The mod-facing "after" hook runs before the ordinary did-trigger subscribers, as in the C#.
`RulesOverflowException` is modelled as an error that stops each enclosing rule at its next
step, and is caught where the C# catches it: at each subscriber call (subscriber dispatch
catches every exception), so an overflow inside a handler stops only that handler. The C#
unwinds immediately; code after a nested `Trigger` inside one handler still runs here,
which a port must not rely on. `AssignSource` keeps only the "existing", "previous
rule" and "self" cases: there is no `ContextData` stack yet.

## RNG, time and arithmetic

- Every random draw goes through `Rng`. Call-site keys are WrathOracle's, e.g.
  `Assembly-CSharp|Kingmaker.RuleSystem.RulebookEvent+Dice|D(Int32,DiceType)|#0@13|Range(int,int)`
  (the oracle found 280 sites in this build). `Dice.D(n, d)` makes `n` separate draws.
- The clock keeps game time in `TimeSpan` ticks and steps it like the original's real-time
  path, including the whole-millisecond rounding (80 frames at 80 Hz advance 1.04 s, a
  test). In turn-based combat the original then runs `CombatController.TickTime`, which
  can clamp deltas and overwrite game time: not ported, and reported.
- Mono's float model is "evaluate in double, round on store" (Wrath-Mac's DXT finding);
  helpers make each store explicit. Float→int casts follow arm64 `fcvtzs` (saturating, NaN →
  0) [I: from the ISA, not checked in the game].

## Character sheet and comparison

`sheet::dump` writes `units[]` in `wrath-oracle-state-v1` (`docs/oracle/state-dump-format.md`)
for what the sim models: identity, flags (subset), `hp`, levels, every stat (`base`,
`permanent`, `modified`, `raw`, the subclass extras, modifiers grouped by descriptor and in
list order with source fact, component name and item), facts in list order with
components and saved data, `resources[]` (blueprint, amount, the persisted `RetainCount`),
`spellbooks[]` (blueprint, caster level, and `persisted`: the oracle writer's rendering of
the spellbook, including its `$cycle` markers for linked slots and its depth limit), body
slots with items and enchantments. `compare` diffs it with an oracle dump field by field
(units by id, stats by type, facts by id in both directions plus order, resources and
spellbooks by blueprint, `$stat` references by stat and owner) and reports the match rate per
field class and per unit. Ids created during the run are random in the original
(`Guid.NewGuid()`): an item's in a modifier, a spell's `UniqueId` in a spellbook. The dump lists
the ids that came from the save (`sim.saved_item_ids`, `sim.saved_spell_ids`); `compare`
renames every other id by order of first appearance (items per dump, spells per spellbook), on
both sides, and keeps a saved id strict: a modifier from the wrong saved item, or a wrong saved
spell, still differs. Without those lists nothing is renamed.

## Items and the body

`items.rs` ports run-time item creation and the unit body: `ItemsEntityFactory.CreateEntity`
and the `ItemEntity` constructors (weapon, armor, shield with its two component entities,
usable, simple; enchantments as `ItemEnchantment` facts, activated when an item has a wielder),
`new UnitBody` (the empty-hand weapon), `UnitBody.Initialize` and `TryInsertItem`, the slot
classes' rules (`CanInsertItem`, `IsItemSupported`, `CanRemoveItem`, the hand pair, `InsertItem`,
`RemoveItem`), `ItemsCollection` (`Add` with stack merging, `Insert`, `Extract`, `Remove`,
`Split`; a collection is the identity items carry, each unit holding it keeps a copy of its
list, so edits go through `collection_edit`), `ItemEntity.OnDidEquipped`/`OnWillUnequip` with
the armor, shield and weapon overrides, and `AddStartingInventory`. A slot is named as in the
save (`HandsEquipmentSets[i].PrimaryHand`, `Armor`, `QuickSlot0`, `AdditionalLimbs[0]`).
Equipment events (`IUnitEquipmentHandler.HandleEquipmentSlotUpdated`, with the slot in
`EventBus::slot`) and collection events (`IItemsCollectionHandler`, with the collection and
item in `EventBus::item`) reach `components/equipment.rs` (`ShieldFocus`, `ArmorFocus`,
`DeflectArrows`, the two Monk unlocks) and `components/inventory.rs` (`PartyInventoryTrigger`,
the `TutorialTriggerNewItem` family).

`entity::dispose` is `EntityDataBase.Dispose`: unsubscribe, `OnDispose` (a unit clears its unit
subscriptions and disposes the body's items, empty hands and, for a unit with its own inventory,
that collection; an item leaves its collection through `ItemsCollection.Remove`, which takes it
out of its slot with the unequip hooks), the parts, the facts (the owner is disposing, so no
`Detach`), then the registry. `spawn::entity_destroyer_tick` is `EntityDestructionController`;
the level-up plan unit is disposed the same way. `spawn::activatable_abilities_tick` is
`UnitActivatableAbilitiesController` (before the script zones and the dialogue in a tick): the
party's units each tick, with `ShouldBeDisabled` (the Rest mode applies on its own) and, outside
dialogue and cutscenes, the per-ability round timer (`TimeToNextRound`,
`ActivatableAbility.OnNewRound`, first-round deactivation). Turn-based round timing, and an area
unit with a toggle to tick (its awake state isn't modelled), are reported.

## Spellbooks and resources

Spellbooks load from the save (`hydrate/books.rs`), get `Spellbook.PostLoad` (known levels
rebuilt, `TryRestoreKnownSpells`, the casting stat's `OnChanged` handler that resizes the
slots) and `ApplyPostLoadFixes` (an empty book is dropped; classes with a negative caster level
modifier catch up). A level-up goes through `DemandSpellbook`, `AddLevelFromClass`
(`LearnSpellsOnRaiseLevel`), the special lists of `AddSpecialSpellList[ForArchetype]`,
`LearnSpells`, `AddKnownSpell`, `LearnSpellList`, `AddSpellsPerDay`, then `PrepareSpellbook`
(`UpdateAllSlotsSize`, `Memorize`) and `Replenish` (resources back to their maximum, every
spellbook rests, `ISpellBookRest`). `AddClassLevels` raises `ILevelUpCompleteUIHandler` when
it is done: its subscribers are the UI and the components that learn spells, so every unit's
`AddKnownSpell` runs again. Spell lists are filtered for DLC (`SpellLevelList.SpellsFiltered`,
the platform's state in `cache/sim/dlc-rewards.json`). The activatable-ability group size and
the special-metamagic list are unit-part state (`Unit::group_size_increases`,
`special_metamagic`); `UnitPartExtraSpellsPerDay` is `Unit::extra_spells_per_day`.

Not ported, reported when reached: spell selections (`AddClassLevels.SelectSpells`,
`SpellSelectionData`), metamagic, magic hack, converted, temporary and custom spells,
opposition schools and descriptors, `MagicHackSpellbookComponent`, swapping slots, spending
spells, mythic spellbook selection, `CompanionBoon` and `ShareFeaturesWithPet` (their
`ILevelUpCompleteUIHandler`).

## Not built yet

`ContextData` scopes, action/condition/evaluator registries (the registry type exists for
components only), event arguments on the `EventBus` beyond the unit, fact, slot and
collection ones, fact processors (feature ranks), the area's non-unit entities, spell
selection, casting, body limbs' item bookkeeping beyond `AddAdditionalLimb`, double weapons and item facts,
slot locks and polymorph, conditions and their status effects, our own save format's sim
snapshot. Each is reported when reached.

Built since: spellbooks and ability resources (above), placeholders become units in every hydration (`hydrate::restore_area_blueprints`,
scene states, `spawn::become_unit`), the level-up port (`spawn/levelup.rs`), the level-plan
unit (`spawn/copy.rs`), Recruit's `ChangeUnitBlueprint` with the creation/destruction
controller ticks (`spawn::change_unit_blueprint`), run-time items and the body (above), and
`AttachBuff` / `AddItemToPlayer` actions.
