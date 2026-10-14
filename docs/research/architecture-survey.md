# Architecture survey: Wrath of the Righteous → Rust/Bevy

## Executive assessment

**A faithful headless reimplementation is viable as a first milestone. A faithful replacement for the complete game is a much larger undertaking than porting its rules or loading its blueprints.**

The strongest starting point is the separation between static blueprints, persistent entity/fact state, and synchronous rule/dialogue execution. The biggest obstacle is that **Unity assets contain gameplay configuration and authored world structure—not just presentation assets**. A second major obstacle is behavioral coverage: thousands of component/action/condition types, fact activation and post-load callbacks, ordered event subscriptions, and scene-dependent rules collectively define the game.

Four findings materially change the plan:

1. **The shipped runtime loads `blueprints-pack.bbp`, not `blueprints.zip`.** The JSON archive is an excellent Rust input, but equivalence to the effective runtime content needs verification.
2. **`PrototypeLink` and `m_Overrides` are not runtime inheritance instructions in this build.** A generic prototype-overlay loader would introduce behavior that the examined runtime does not perform.
3. **I found no persisted general combat RNG state.** Loading a save does not establish the next attack or dialogue die roll.
4. **Some balance data live in Unity ScriptableObjects outside the blueprint JSON.** The difficulty adjustment table is one concrete example relevant even to a headless dialogue slice.

This survey was read-only. I inspected source, archive metadata and JSON, and a real save; I did not modify files, extract/rebuild Unity assets, install instrumentation, or execute a live fidelity comparison.

---

## Scope and measurements

Measurements use `tokei`, `wc -l`, filesystem enumeration, ZIP enumeration, and JSON parsing. **LOC below means physical lines unless explicitly labeled code lines.** Namespace-family totals overlap and should not be added together.

### Decompiled source

| Assembly | Physical C# lines |
|---|---:|
| `Assembly-CSharp` | 778,041 |
| `Owlcat.Runtime.Core` | 17,771 |
| `Owlcat.Runtime.Visual` | 24,860 |
| `Owlcat.Runtime.UI` | 14,719 |
| `Owlcat.SharedTypes` | 146 |
| `DXTCompressor` | 1,091 |
| **Total** | **836,628** |

`tokei` reports **10,872 C# files, 725,624 code lines, and 111,004 blank lines**.

Selected family measurements:

| Family | Files | Physical lines |
|---|---:|---:|
| Blueprint JSON/converters/binary serialization | 29 | 3,292 |
| `Kingmaker.EntitySystem*` | 216 | 27,225 |
| `Kingmaker.RuleSystem*` | 103 | 8,579 |
| `Kingmaker.PubSubSystem*` | 428 | 4,703 |
| `Kingmaker.UnitLogic.Mechanics*` | 294 | 17,054 |
| Fact logic plus designers’ mechanics | 896 | 43,956 |
| Elements plus designers’ actions/conditions/evaluators | 567 | 23,795 |
| Dialogue controller plus dialogue-system families | 39 | 3,252 |
| Etudes | 57 | 4,231 |
| Quests and quest blueprints | 36 | 1,870 |
| Cutscenes | 99 | 9,640 |
| Unit commands | 30 | 5,508 |
| AI | 67 | 4,672 |
| Turn-based families | 14 | 6,500 |
| Character visuals | 69 | 7,585 |
| Visual animation | 83 | 9,205 |
| `Kingmaker.UI*` | 3,034 | 274,274 |

The UI alone is approximately one-third of the decompiled physical source.

### Game data

The supplied “252,498 blueprints” figure is the archive-entry count:

- **252,498 ZIP entries**
- **236,661 `.jbp` files**
- **15,836 directory entries**
- **One legacy `.asset` entry**
- Approximately **568.8 MB of uncompressed `.jbp` JSON**
- **335 distinct root blueprint types**
- **2,155 distinct nested `$type` identifiers**, counting `$type` values below the root `Data` object
- **2,490 root-plus-nested types**

The nested count differs slightly from the supplied 2,151; the counting method above is the one used here.

`Bundles/blueprints-pack.bbp` contains **236,661 indexed records** and is **239,868,334 bytes**. Matching counts are useful evidence, but not proof of identical effective objects.

`Bundles/` contains **23,569 regular files**, totaling **42,678,322,205 bytes**, approximately **39.75 GiB**:

- **23,565 files with a `UnityFS` signature**
- `blueprints-pack.bbp`
- `locationlist.json`
- `dependencylist.json`
- `cheatdata.json`

---

# 1. Blueprint loading

### How it works

The blueprint JSON path uses **Newtonsoft.Json**, a custom field contract resolver, and a GUID-based type binder rather than ordinary assembly-qualified polymorphic names. Each `.jbp` is a wrapper containing `AssetId` and `Data`; the wrapper establishes the blueprint identity and deserialization context. Strings such as `!bp_<guid>` become typed blueprint references that retain an identifier and resolve lazily through `ResourcesLibrary`/`BlueprintsCache`. The leading GUID in `$type` selects a registered C# type; the trailing human-readable type name is not the authoritative discriminator. The shipped base-game cache instead reads a binary table of GUIDs and offsets from `blueprints-pack.bbp`, seeks to records, and deserializes them with Owlcat’s reflection-based binary serializer. Loading also passes through modification callbacks and blueprint initialization, so “JSON parsed successfully” is not equivalent to “runtime object reconstructed correctly.”

### Key types and evidence

- `BlueprintsCache`, **120 LOC**: binary index initialization, lazy loading, caching and initialization.  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints.JsonSystem/BlueprintsCache.cs:32`
- `ResourcesLibrary`: public blueprint lookup and separate Unity resource loading.  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints/ResourcesLibrary.cs:1`
- `Json`, **52 LOC**: Newtonsoft settings and converter registration.  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints.JsonSystem/Json.cs:1`
- `FieldsContractResolver`, **120 LOC**: Unity-style serializable fields, inherited fields and exclusions.  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints.JsonSystem/FieldsContractResolver.cs:1`
- `GuidClassBinder`, **53 LOC**, and `TypeIdAttribute`: GUID-to-type registration.  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints.JsonSystem/GuidClassBinder.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints.JsonSystem/TypeIdAttribute.cs:1`
- `BlueprintJsonWrapper`, **84 LOC**; `BlueprintReferenceConverter`, **51 LOC**; `BlueprintReferenceBase`, **114 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints.JsonSystem/BlueprintJsonWrapper.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints.JsonSystem.Converters/BlueprintReferenceConverter.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints/BlueprintReferenceBase.cs:1`
- `ReflectionBasedSerializer` and `PrimitiveSerializer`: the binary pack’s object/primitive format.  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints.JsonSystem.BinaryFormat/ReflectionBasedSerializer.cs:1`

Owlcat’s public mod template independently documents the `.jbp` wrapper and GUID-based `$type` format. Its separate `.patch` merge mechanism should not be confused with prototype metadata. ([github.com](https://github.com/OwlcatOpenSource/WrathModificationTemplate))

### `PrototypeLink` and `m_Overrides`

The important evidence is:

- `BlueprintScriptableObject` retains a serialized prototype identifier and overrides list.
- **Its runtime `PrototypeLink` property returns `null`.**
- Component prototype links are authoring-oriented and marked for exclusion from builds.
- The examined prototype/override converters do not implement a runtime recursive merge.

See `decompiled/Assembly-CSharp/Kingmaker.Blueprints/BlueprintScriptableObject.cs:17`, especially the property at line 66, and `decompiled/Assembly-CSharp/Kingmaker.Blueprints/BlueprintComponent.cs:1`.

I counted **5,151 blueprints with nonempty prototype metadata** and **28,351 with nonempty overrides**. Those numbers do **not** establish inheritance behavior.

**Rust should preserve that metadata, not automatically overlay parent objects.** Use the concrete exported fields and verified defaults.

### Unity dependency and port difficulty

**Unity dependence: thin for blueprint objects and JSON; deep for referenced assets.** Despite their names, the examined `SimpleBlueprint`/`BlueprintScriptableObject` data hierarchy is not simply a hierarchy of Unity `ScriptableObject` instances.

**Port difficulty: moderate for the loader, hard for complete semantic initialization.** The difficult parts are defaults, polymorphic schema coverage, initialization callbacks and asset references—not ZIP decompression.

### Rust loader design

A direct JSON importer should:

1. Read `blueprints.zip` without unpacking it.
2. Index `.jbp` entries by `AssetId`, retaining archive path and root type GUID.
3. Dispatch on a generated registry of `TypeIdAttribute` identifiers.
4. Represent `!bp_…` as durable typed IDs, not recursively expanded object copies.
5. Implement field defaults explicitly; missing JSON fields do not universally mean zero.
6. Preserve component names, ordering, disabled flags and unknown fields.
7. Resolve `{guid,fileid}` Unity asset references through a separate asset database.
8. Run explicit equivalents of required initialization, then compare representative objects against the live game.

Unknown types can be retained as opaque data for inventory and save preservation. **An unknown type that must execute should fail visibly, not become a silent no-op.**

---

# 2. Entity system and persistent state

### How it works

`EntityDataBase` is the durable runtime object foundation, with a stable `UniqueId`, parts, facts and lifecycle methods; its Unity view is a separate attachment. `UnitEntityData` adds substantial unit behavior, while `UnitDescriptor` owns much of the unit’s inventory, stats, progression, spellbooks, equipment/body, state, faction and alignment. `UnitPart*` classes extend entities with feature-specific persistent state rather than requiring every feature to be a field on the unit. Facts—features, buffs and related effects—instantiate runtime delegates from blueprint components and retain mutable component data separately from static blueprint configuration. Saving uses opt-in Newtonsoft serialization with object reference preservation, and loading performs ownership restoration, callbacks and version/post-load fixes. Consequently, a save importer must reconstruct an object graph and lifecycle state, not just deserialize a flat list of units.

### Key types and size

- `EntityDataBase`, **662 LOC**: parts/facts, identity, load/save and activation lifecycle.  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem/EntityDataBase.cs:1`
- `UnitEntityData`, **2,555 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Entities/UnitEntityData.cs:1`
- `UnitDescriptor`, **1,270 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.UnitLogic/UnitDescriptor.cs:1`
- `EntityPartsManager`, **294 LOC**, and `EntityPart`, **262 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem/EntityPartsManager.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem/EntityPart.cs:1`
- `EntityFact`, **1,068 LOC**, and `EntityFactComponentDelegate`, **410 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem/EntityFact.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem/EntityFactComponentDelegate.cs:1`
- `Feature`, **394 LOC**, and `Buff`, **618 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.UnitLogic/Feature.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.UnitLogic.Buffs/Buff.cs:1`
- `Player`, **1,769 LOC**: party and campaign-wide state.  
  `decompiled/Assembly-CSharp/Kingmaker/Player.cs:1`

The `Kingmaker.UnitLogic.Parts` family alone is **117 files / 9,352 LOC**.

### Save format

The relevant implementation is:

- `SaveManager`, **1,419 LOC**, particularly `SerializeAndSaveThread`.  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Persistence/SaveManager.cs:964`
- `ZipSaver`, **171 LOC**, using Ionic ZIP.  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Persistence/ZipSaver.cs:1`
- `DefaultJsonSettings`, **97 LOC**: `TypeNameHandling.Auto`, preserved object references, opt-in contracts.  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Persistence.JsonUtility/DefaultJsonSettings.cs:1`
- `OptInContractResolver`, **58 LOC**, and custom blueprint/entity/dictionary/vector converters.  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Persistence.JsonUtility/OptInContractResolver.cs:1`
- `ThreadedGameLoader`: deserialize, restore and upgrade.  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Persistence/ThreadedGameLoader.cs:1`

**Save `$type` values are .NET type names, not the blueprint JSON’s GUID type tags.** Other important differences:

- Blueprint references are generally bare GUID strings.
- Entity references use stable entity identifiers.
- `$id`/`$ref` reconstruct shared objects.
- Reference identity is scoped to separately serialized JSON documents.
- Some dictionaries are serialized as `[{Key,Value}, …]`.
- Position conversion can use centimeter integer triplets encoded as `"x|y|z"`.
- Serialized component-state dictionaries are keyed by **blueprint component name**.

Component names and reference identity must survive import.

### A real `.zks` archive

I listed and inspected:

`~/Library/Application Support/Owlcat Games/Pathfinder Wrath Of The Righteous/Saved Games/Manual_13_Underground_Caves_of_Kenabres__15_Arodus__VIII__4715__12_03_41.zks`

Its actual contents:

```text
Bytes       Entry
844         header.json
294155      header.png
758252      e31ab967a7accd543bdc420fda370e6d.json
1868884     e31ab967a7accd543bdc420fda370e6dPrologue_Kenabres_CitizenIdle.json
313673      bad465e5404604346bdf71ef50b3299a.json
3011        bad465e5404604346bdf71ef50b3299aPrologue_Caves_1_TerendelevScales_Mechanics.json
43516       bad465e5404604346bdf71ef50b3299a.Prologue_Caves_1_Static.fog
596287      player.json
44685       party.json
3           tbm.json
136747      history
226         checksums

Total: 4,060,283 uncompressed bytes, 12 entries
```

The three-byte `tbm.json` is **only a UTF-8 BOM**, not a valid JSON document. An importer must handle this inactive-turn-based case.

`player.json` includes dialogue histories, quests, flags, etudes, settings and game time. `party.json` is cross-scene party state; other entries hold area and scene state. The fog entry is binary, and `history` is not an RNG-state checkpoint.

`SceneEntitiesStateConverter` deliberately filters or substitutes some entities during save serialization. **A save is not a complete dump of the authored scene.** See `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Persistence.JsonUtility/SceneEntitiesStateConverter.cs:1`.

### Unity dependency and port difficulty

**Unity dependence: thin in identity/parts/facts abstractions; deep in full unit/world behavior.**

**Port difficulty: hard.** Stable identity, cyclic/shared references, fact activation, serialized versus derived stats, timers and post-load fixes are the main risks. Keep durable game IDs separate from transient Bevy `Entity` values.

---

# 3. Rules, modifiers and randomness

### How it works

The rule system executes synchronous, nested `RulebookEvent` objects against a current rule context rather than publishing independent asynchronous ECS events. Events run ordered “about to trigger” subscribers, their own calculation, and ordered “did trigger” subscribers; these phases can create further rules or modify the current rule. Initiator, target and global subscriber scopes connect active facts to rules affecting the relevant units. Stats use dependency-aware `ModifiableValue` objects and descriptor-specific stacking rules, not a simple sum of bonuses. Attack, saving throw and damage rules combine these abstractions with extensive exceptions, rerolls, immunities and other nested checks. Ordinary dice ultimately draw from **Unity’s global RNG**, while some procedural systems use separate seeded generators.

### Key types and size

- `Rulebook`, **197 LOC**, and `RulebookEvent`, **279 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.RuleSystem/Rulebook.cs:67`  
  `decompiled/Assembly-CSharp/Kingmaker.RuleSystem/RulebookEvent.cs:1`
- `RulebookEventContext`: nested event context and stack.  
  `decompiled/Assembly-CSharp/Kingmaker.RuleSystem/RulebookEventContext.cs:1`
- `RulebookEventBus`, subscriber managers and handler interfaces.  
  `decompiled/Assembly-CSharp/Kingmaker.PubSubSystem/RulebookEventBus.cs:166`
- `RuleAttackRoll`, **452 LOC**; `RuleCalculateAC`, **102 LOC**; `RuleSavingThrow`, **244 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.RuleSystem.Rules/RuleAttackRoll.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.RuleSystem.Rules/RuleCalculateAC.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.RuleSystem.Rules/RuleSavingThrow.cs:1`
- `RuleDealDamage`, **255 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.RuleSystem.Rules.Damage/RuleDealDamage.cs:1`
- `ModifiableValue`, **1,081 LOC**, and `CharacterStats`, **302 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Stats/ModifiableValue.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Stats/CharacterStats.cs:1`

### Event ordering is part of the specification

The examined order is:

```text
Mod Before hook
  → AboutTo hook
  → Global AboutTo subscribers
  → Target AboutTo subscribers
  → Initiator AboutTo subscribers
  → Event.OnTrigger
  → Mod After hook
  → Global DidTrigger subscribers
  → Target DidTrigger subscribers
  → Initiator DidTrigger subscribers
  → DidTrigger hook
  → Event.OnDidTrigger: mark triggered, remove temporary modifiers
```

Subscriber-list order matters. Nested rules use a stack with an overflow guard, and rule source/reason is inferred where needed.

**The mod-facing `IAfterRulebookEventTriggerHandler<T>` is not a final-after-everything hook.** It runs before ordinary `OnEventDidTrigger` subscribers.

A Rust implementation should preserve this ordering explicitly. Sending these rules through unconstrained parallel Bevy systems would change semantics.

### Modifier behavior

`ModifiableValue` groups modifiers by descriptor:

- Stackable categories sum.
- Nonstacking groups generally select the strongest positive and strongest negative contributions.
- Armor/encumbrance-related penalties have additional handling.
- Source fact/component identity matters.
- Dependencies trigger recalculation.
- `PermanentValue` is not interchangeable with every computed current value.

Stackable descriptors are enumerated in `decompiled/Assembly-CSharp/Kingmaker.Enums/ModifierDescriptorHelper.cs:1`.

Also, `CheckDiplomacy`, `CheckBluff` and `CheckIntimidate` are dependent checks backed by Persuasion-related state, not independent skill-rank buckets. See `CharacterStats`.

### RNG and determinism

The decisive normal-dice call is:

`decompiled/Assembly-CSharp/Kingmaker.RuleSystem/RulebookEvent.cs:70`

It calls:

```text
UnityEngine.Random.Range(1, dice.Sides() + 1)
```

`RuleRollDice`, `RuleRollD20` and `RuleRollD100` add reroll, override and history behavior around those draws:

`decompiled/Assembly-CSharp/Kingmaker.RuleSystem.Rules/RuleRollDice.cs:1`

I found **no save serialization of the global combat RNG state**. The `Random.state`/`Random.InitState` usage found in the surveyed source is a tactical-grid generation scope that saves the old Unity state, temporarily seeds generation and restores it:

`decompiled/Assembly-CSharp/Kingmaker.Armies.TacticalCombat.Controllers/TacticalCombatGridController.cs:63`

`PersistentRandom` is a separate deterministic generator used by dungeon generation, with persistent seeds:

`decompiled/Assembly-CSharp/Kingmaker.Dungeon.Utils/PersistentRandom.cs:1`

Its arithmetic overflow and string-hash behavior require compatibility work if reproduced. It is **not** the general combat dice generator.

Unity exposes a serializable, gettable/settable full `Random.state`; an oracle mod could capture it prospectively. That API capability does not mean Wrath saves it. ([docs.unity3d.com](https://docs.unity3d.com/2020.3/Documentation/ScriptReference/Random-state.html))

### Unity dependency and port difficulty

**Unity dependence: thin for most arithmetic; deep at spatial/view and effect-application boundaries.** For example, AC calculation can inspect view-dependent posture, and damage application invokes hit presentation.

**Port difficulty: hard.** The arithmetic is tractable; exceptional rules, mutation order, temporary modifiers and RNG draw order are the fidelity challenge.

---

# 4. Mechanics content: components, actions, conditions and contexts

### How it works

Blueprints carry static component configurations and nested action/condition objects that form an executable content language. `GameAction` and `Condition` provide the basic execution interfaces, while `ActionList` and `ConditionsChecker` compose them with ordered actions and short-circuit Boolean logic. Context-aware actions read a scoped `MechanicsContext`, including caster, target, parent/root context, ability parameters, ranks and shared values. Fact components use runtime delegates and per-fact mutable data, so sharing a blueprint component does not mean sharing all effect state. `AddFacts`, buff application and rank calculation illustrate that execution also involves lifecycle, ownership, duration and source relationships. The port therefore needs several interpreter/runtime categories—not one generic switch that treats all nested `$type` values as actions.

### Key types and examples

- `BlueprintComponent`: static configuration and owning blueprint.  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints/BlueprintComponent.cs:1`
- `GameAction`, `Condition`, `ActionList`, `ConditionsChecker`, `Element`.  
  `decompiled/Assembly-CSharp/Kingmaker.ElementsSystem/GameAction.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.ElementsSystem/Condition.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.ElementsSystem/ActionList.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.ElementsSystem/ConditionsChecker.cs:1`
- `ContextAction`, **34 LOC**, and `UnitFactComponentDelegate`, **17 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.UnitLogic.Mechanics.Actions/ContextAction.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.UnitLogic/UnitFactComponentDelegate.cs:1`
- `MechanicsContext`, **561 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.UnitLogic.Mechanics/MechanicsContext.cs:1`
- `AddFacts`, **169 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.UnitLogic.FactLogic/AddFacts.cs:1`
- `ContextActionApplyBuff`, **136 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.UnitLogic.Mechanics.Actions/ContextActionApplyBuff.cs:1`
- `ContextRankConfig`, **793 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.UnitLogic.Mechanics.Components/ContextRankConfig.cs:1`
- `Conditional` and `ContextConditionHasFact`.  
  `decompiled/Assembly-CSharp/Kingmaker.Designers.EventConditionActionSystem.Actions/Conditional.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.UnitLogic.Mechanics.Conditions/ContextConditionHasFact.cs:1`

### Behavior worth preserving

- Empty condition collections evaluate true, including the examined empty-OR case.
- Conditions short-circuit and apply their `Not` flag.
- Action lists catch failures per action and can continue to later actions; they are not transactions.
- `AddFacts` records which facts it applied for later removal/reconciliation.
- Buff application preserves caster/source/context and parent-child relationships.
- Rank configuration includes class levels, fact ranks, stats, equipment, custom properties and progression/clamping formulas.

### Unity dependency and port difficulty

**Unity dependence: none/thin for many leaf handlers; deep for actions manipulating views, scenes, animation or physics.**

**Port difficulty: moderate for individual pure handlers, very hard for complete coverage.** The measured **2,155 nested types** include reusable infrastructure, data structures and specialized behaviors, not 2,155 equally sized scripts.

The appropriate Rust pattern is:

- Typed static content ASTs.
- Explicit context stacks.
- Fact-instance state keyed separately from blueprint configuration.
- Ordered rule subscribers.
- Action/condition/evaluator registries.
- Coverage tracking by **executed type**, with unsupported execution rejected.

Schema generation can automate field decoding. It cannot infer behavior.

---

# 5. Dialogue, localization and companion bookkeeping

### How it works

The gameplay dialogue controller maintains current cue, available answers, acting unit, sequence/book-page state, and local history while updating persistent global dialogue history. Cue selection evaluates eligibility and chooses the first eligible cue or a random candidate according to the authored strategy. Answer visibility and selectability are separate concepts, with show-once history, conditions, alignment/mythic requirements and optional valid-next-cue checks. Selecting an answer records it, executes actions and alignment/experience effects, then advances through cues, checks or sequences; cue transitions execute the outgoing cue’s stop actions and the incoming cue’s show actions. Skill checks choose an explicit/acting unit or a party roller, then execute the same rule system used elsewhere. Localized text resolves primarily through string keys and can pass through template expansion; voice-over is a separate key-to-Wwise-event lookup.

### Key types and size

Use the gameplay controller, **not the older UI class with the same name**:

- `DialogController`, **1,209 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.Controllers.Dialog/DialogController.cs:1`
- `BlueprintDialog`, `BlueprintCue`, `BlueprintAnswer`, `BlueprintAnswersList`, `BlueprintCueSequence`, `BlueprintCheck`.  
  `decompiled/Assembly-CSharp/Kingmaker.DialogSystem.Blueprints/BlueprintDialog.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.DialogSystem.Blueprints/BlueprintAnswer.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.DialogSystem.Blueprints/BlueprintCheck.cs:1`
- `CueSelection` and `CharacterSelection`, **70 LOC each**.  
  `decompiled/Assembly-CSharp/Kingmaker.DialogSystem/CueSelection.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.DialogSystem/CharacterSelection.cs:1`
- `DialogState`: persistent shown-cue/selected-answer histories.  
  `decompiled/Assembly-CSharp/Kingmaker.DialogSystem.State/DialogState.cs:1`
- `SkillCheckResult`, `RulePartyStatCheck`, `RuleStatCheck`, `RuleSkillCheck`.  
  `decompiled/Assembly-CSharp/Kingmaker.Controllers.Dialog/SkillCheckResult.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.RuleSystem.Rules/RulePartyStatCheck.cs:1`

Useful execution points in the controller:

- Current-cue transition: line **146**
- Start dialogue: line **335**
- Select answer: line **641**
- Play cue: line **804**
- Play check: line **936**

Party checks select eligible units by computed stat value, respect ability to act and companion restrictions, and optionally include remote companions in capital mode. Authored DC and final rule DC can differ because of difficulty adjustment.

### Localization

- `LocalizedString`, **214 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.Localization/LocalizedString.cs:1`
- `LocalizationManager`, **347 LOC**, and `LocalizationPack`, **135 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.Localization/LocalizationManager.cs:219`
- Shared string decoding and template expansion.  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints.JsonSystem.Converters/SharedStringConverter.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.TextTools/TextTemplateEngine.cs:1`

In the supplied installation:

- `Localization/enGB.json`: **110,399 string entries**
- `Localization/Sound.json`: **7,815 mappings**

The runtime lookup uses `m_Key` or the shared string’s key. **`m_JsonPath` identifies authoring provenance; it is not a requirement to open a separate per-string JSON file.**

Templates can depend on player, gender and other game state. Returning raw pack text is not always equivalent to the displayed text.

### Voice and companion/alignment state

`LocalizedString.PlayCueVoiceOver` looks up the localization key in the sound pack and posts the resulting event through `SoundEventsManager`:

`decompiled/Assembly-CSharp/Kingmaker.Localization/LocalizedString.cs:110`

Alignment shifts mutate unit alignment and are logged as dialogue shifts:

`decompiled/Assembly-CSharp/Kingmaker.UnitLogic.Alignments/AlignmentShiftExtension.cs:1`

Companion-related outcomes are distributed across facts, `UnitPartCompanion`, flags, etudes and dialogue history. There is no single universal “companion approval” field that substitutes for all of that bookkeeping.

### Unity dependency and port difficulty

**Unity dependence: thin for blueprint selection/history/check logic; deep in the complete controller’s speakers, camera, animation and UI integration.**

**Port difficulty: moderate for a bounded headless conversation branch; hard for general dialogue parity.** Book pages, scene speakers, contextual text and actions that start cutscenes expand the scope quickly.

---

# 6. Etudes, quests, flags, cutscenes and scenes

### How it works

Etudes form a persistent narrative-state system with parent relationships, activation/completion conditions, linked/conflicting groups, priorities and area-mechanics effects. An etude being started is distinct from it being actively playing, and the controller reevaluates dirty conditions to decide which etudes should run. Quests/objectives and unlockable flags provide additional persistent state, with actions and events linking them to dialogue, etudes and the world. Cutscenes are executable graphs of gates, tracks and commands, including conditions, timing, skipping, completion and failure behavior. Areas reference multiple additive scenes, while scene objects supply persistent IDs, transforms, spawners, map-object behavior, zones and locators. Therefore, blueprint extraction alone cannot reconstruct an area’s playable world.

### Key types and size

- `BlueprintEtude`, `Etude`, `EtudesSystem`; family **4,231 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.AreaLogic.Etudes/BlueprintEtude.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.AreaLogic.Etudes/EtudesSystem.cs:193`
- `QuestBook`, `Quest`, `QuestObjective`; quests/blueprints **1,870 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.AreaLogic.QuestSystem/QuestBook.cs:1`
- `UnlockableFlagsManager`, **78 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.AreaLogic.QuestSystem/UnlockableFlagsManager.cs:1`
- `Cutscene`, `Gate`, `CommandBase`, `CutscenePlayerData`; family **9,640 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.AreaLogic.Cutscenes/Cutscene.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.AreaLogic.Cutscenes/Gate.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.AreaLogic.Cutscenes/CommandBase.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.AreaLogic.Cutscenes/CutscenePlayerData.cs:1`
- `BlueprintArea`, `BlueprintAreaPart`, `SceneReference`.  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints.Area/BlueprintArea.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints.Area/BlueprintAreaPart.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints.Area/SceneReference.cs:1`
- Scene loading and bundle loading.  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Persistence.Scenes/SceneLoader.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Persistence.Scenes/BundledSceneLoader.cs:1`

Flags require both **presence/unlocked state and integer value**. Missing and present-with-zero are not interchangeable.

### What lives in scenes rather than blueprints

Concrete view types:

- `EntityViewBase`: MonoBehaviour attachment and persistent identity.  
  `decompiled/Assembly-CSharp/Kingmaker.View/EntityViewBase.cs:32`
- `UnitSpawner`/`UnitSpawnerBase`: blueprint plus authored spawn configuration.  
  `decompiled/Assembly-CSharp/Kingmaker.View.Spawners/UnitSpawner.cs:1`
- `MapObjectView`: scene map objects and interaction configuration.  
  `decompiled/Assembly-CSharp/Kingmaker.View.MapObjects/MapObjectView.cs:1`
- `ScriptZone`: trigger geometry and zone behavior; the namespace really is spelled `SriptZones`.  
  `decompiled/Assembly-CSharp/Kingmaker.View.MapObjects.SriptZones/ScriptZone.cs:1`
- `LocatorView`: authored world locations.  
  `decompiled/Assembly-CSharp/Kingmaker.View/LocatorView.cs:1`
- `EntityReference`: scene/object identity resolution.  
  `decompiled/Assembly-CSharp/Kingmaker.Blueprints/EntityReference.cs:1`

`BlueprintAreaPart` references dynamic, static, lighting and other scenes, as well as a navigation graph cache. `SceneReference` retains a scene name; the editor scene asset is not the runtime representation.

At minimum, extraction must retain:

- Scene/prefab hierarchy and transforms.
- Persistent entity/view IDs.
- Spawner configuration and initial state.
- Map-object components and interaction links.
- Script-zone shapes and attached behavior.
- Locators and evaluator targets.
- Cross-object references.
- Collision/navigation geometry.
- Non-blueprint ScriptableObjects.
- Relevant material, animation, lighting and audio references.

The bundle tables provide another useful bridge:

- `locationlist.json`: **1,775,085 bytes**, parallel GUID/bundle arrays.
- `dependencylist.json`: **3,154,310 bytes**, bundle dependency entries.

There are **2,134 `.scenes` bundles** and **285 `.nav` bundles**.

### Unity dependency and port difficulty

**Unity dependence: thin for flags/quest state and much etude logic; deep for cutscene/world execution.**

**Port difficulty: moderate for flags/quests, hard for etude lifecycle, very hard for world reconstruction.** Stable scene identity and extracted gameplay components are prerequisites, not optional polish.

---

# 7. Combat flow, commands, AI, pathfinding and time

### How it works

Real-time and turn-based combat share units, commands, abilities and rules, while turn-based controllers impose turn order, action budgets, movement accounting and waiting/timing behavior. Unit commands progress through approach, start, act, end and interruption states rather than resolving instantly at issuance. Attacks and ability use connect rule execution to animation handles, movement and timed execution processes. AI evaluates blueprint-defined actions using unit/target state, scores and path checks, then issues commands on its own tick schedule. Movement uses **A* Pathfinding Project**, augmented by Owlcat clearance, obstacle and postprocessing code, rather than simply Unity NavMesh. The main simulation is updated per frame with capped variable deltas; it is not a single fixed-step rules loop.

### Key types and size

- Turn-based `CombatController`, **1,320 LOC**, and `TurnController`, **2,913 LOC**.  
  `decompiled/Assembly-CSharp/TurnBased.Controllers/CombatController.cs:1`  
  `decompiled/Assembly-CSharp/TurnBased.Controllers/TurnController.cs:1`
- `UnitCommand`, **1,154 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.UnitLogic.Commands.Base/UnitCommand.cs:1`
- `UnitAttack`, **857 LOC**, and `UnitUseAbility`, **1,055 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.UnitLogic.Commands/UnitAttack.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.UnitLogic.Commands/UnitUseAbility.cs:1`
- `BlueprintBrain`, `AiAction`, `AiBrainController`; AI family **4,672 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.AI.Blueprints/BlueprintBrain.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.AI/AiAction.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.AI/AiBrainController.cs:1`
- `UnitMovementAgent`, **1,316 LOC**, and movement/clearance extensions.  
  `decompiled/Assembly-CSharp/Kingmaker.View/UnitMovementAgent.cs:1`
- `NavMeshManager`.  
  `decompiled/Assembly-CSharp/Kingmaker/NavMeshManager.cs:52`

`NavMeshManager` requests a `TextAsset` graph cache and calls:

```text
AstarPath.active.data.DeserializeGraphs(asset.bytes)
```

The installation includes `AstarPathfindingProject.dll`. Owlcat’s extensions include `SeekerWithClearance`, `ABPathWithClearance`, obstacle handling and path postprocessing.

**The graph cache’s exact graph types and serialization version were not decoded in this survey.** Do not assume a specific Recast/triangle representation until extracting them.

### Tick model

The path is:

`Runner.Update → Game.Tick → GameMode.Tick → ordered registered controllers`

Evidence:

- `decompiled/Assembly-CSharp/Kingmaker/Runner.cs:81`
- `decompiled/Assembly-CSharp/Kingmaker/Game.cs:832`
- `decompiled/Assembly-CSharp/Kingmaker.GameModes/GameMode.cs:30`
- `decompiled/Assembly-CSharp/Kingmaker.Controllers/TimeController.cs:65`

The examined delta calculation is:

```text
min(PlayerTimeScale * DebugTimeScale / 12,
    Time.deltaTime or activation-time unscaledDeltaTime)
```

**The `/12` is a cap, not proof of a fixed 12 Hz simulation.** Game time advances only in relevant modes, while real/UI time have separate handling. Real-time unit rounds accumulate six-second intervals; turn-based timing changes with current turn, movement and UI waiting.

AI adds randomized scheduling jitter, another consumer of randomness outside ordinary dice.

### Unity dependency and port difficulty

**Unity dependence: deep.**

**Port difficulty: very hard for faithful combat execution.** Attack arithmetic alone is much smaller than synchronizing commands, animation hit timing, casting, interruption, movement, turn budgets, pathfinding and frame-order behavior.

A new fixed-step Rust simulation is a reasonable design proposal, but its compatibility must be demonstrated—not assumed from the existing tick model.

---

# 8. Presentation: characters, animation, VFX, camera, fog, UI and audio

### How it works

Characters are assembled from equipment entities, body parts, skeleton attachments and appearance/color settings, with atlas generation and rendering optimization around the resulting meshes. Animation is controlled by Owlcat managers and action layers using Unity Playables, not merely a single Animator controller per unit. Effects are spawned from referenced prefabs and can be attached to bones or world objects with lifetime, visibility and priority management. Camera and fog systems consume live world/view state, and fog also affects what entities are revealed. UI uses Owlcat’s MVVM/disposable binding infrastructure, UniRx and Unity scene/prefab-authored views. Audio uses Wwise events, banks, packages, switches/states and spatial targets, so reproducing sound requires more than extracting individual media files.

### Key types and evidence

**Characters and equipment**

- `Character`, **2,449 LOC**; `CharacterAtlas`, **713 LOC**; `EquipmentEntity`, **537 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.Visual.CharacterSystem/Character.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.Visual.CharacterSystem/CharacterAtlas.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.Visual.CharacterSystem/EquipmentEntity.cs:1`

The “EE” system encodes body-part hiding/layers, attachments, gender/race variants, color ramps and renderer configuration. `CharacterAtlas` uses Unity render textures and graphics operations. There are **3,137 `.ee` bundles** and **15,130 `.unit` bundles**.

**Animation**

- `AnimationManager`, **853 LOC**, with Animator/PlayableGraph setup.  
  `decompiled/Assembly-CSharp/Kingmaker.Visual.Animation/AnimationManager.cs:157`
- `AnimationSet`, `AnimationClipWrapper`, `UnitAnimationManager`.  
  `decompiled/Assembly-CSharp/Kingmaker.Visual.Animation/AnimationSet.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.Visual.Animation/AnimationClipWrapper.cs:1`

Export must preserve clips, skeletal binding paths, curves, masks, event timings and relevant root-motion semantics. Attack/casting timing makes parts of this system gameplay-critical.

**VFX, camera and fog**

- `FxHelper`, **591 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.Visual/ FxHelper.cs:1` — the source file is in the `Kingmaker.Visual` family; effect spawning also involves dedicated spawn controllers.
- `CameraRig`, **1,534 LOC**.  
  `decompiled/Assembly-CSharp/Kingmaker.View/CameraRig.cs:1`
- `FogOfWarController`, **343 LOC**, plus Owlcat render-pipeline fog types.  
  `decompiled/Assembly-CSharp/Kingmaker.Controllers/FogOfWarController.cs:1`

Fog persistence, revealer geometry and entity visibility must be separated from shader/render-texture implementation. It cannot all be discarded as cosmetic.

**UI**

- `ViewBase`: Unity view binding.  
  `decompiled/Owlcat.Runtime.UI/Owlcat.Runtime.UI.MVVM/ViewBase.cs:1`
- `RootUIContext`: UI scene roots and initialization.  
  `decompiled/Assembly-CSharp/Kingmaker.UI.MVVM/RootUIContext.cs:295`
- Dialogue PC view and VM.  
  `decompiled/Assembly-CSharp/Kingmaker.UI.MVVM._PCView.Dialog.Dialog/DialogPCView.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.UI.MVVM._VM.Dialog.Dialog/DialogVM.cs:1`

The C# view class is not the entire layout: references and hierarchy live in Unity prefabs/scenes. Reuse behavior and state contracts, not a line-for-line widget port.

**Audio**

- `AkAudioService`, `SoundEventsManager`, `SoundBanksManager`, `SoundPackagesManager`, `AudioFilePackagesSettings`.  
  `decompiled/Assembly-CSharp/Kingmaker.Sound/AkAudioService.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.Sound/SoundEventsManager.cs:1`  
  `decompiled/Assembly-CSharp/Kingmaker.Sound/AudioFilePackagesSettings.cs:1`

The actual directory is:

`builds/arm64/Wrath.app/Contents/Resources/Data/StreamingAssets/Audio/GeneratedSoundBanks/Mac/`

It contains:

- **45 `.bnk`**
- **36 `.pck`**
- Two XML files and one other file
- **6,347,396,493 bytes**, approximately **5.91 GiB**
- **No loose `.wem` files in the enumerated directory**

Streamed media are packaged; “load all `.wem` files from StreamingAssets” is not the observed layout.

### Unity dependency and port difficulty

**Unity dependence: deep throughout.**

**Port difficulty: very hard for presentation parity.** Mesh import is only the beginning: equipment composition, shaders, animation orchestration, UI semantics and Wwise event behavior each need their own replacement. A first Bevy slice should intentionally avoid reproducing atlas/rendering optimizations.

---

# 9. Important simplifiers and missing scope

## Gameplay data outside `.jbp`

This is the most important extra blocker for the proposed headless slice.

`DifficultyPresetsList` is a Unity `ScriptableObject` containing stat-adjustment coefficients, **skill-check DC bonuses**, HP multipliers and experience coefficients:

`decompiled/Assembly-CSharp/Kingmaker.Settings.Difficulty/DifficultyPresetsList.cs:1`

The JSON `Root/BlueprintRoot.jbp` references it through `{guid,fileid}`. Startup loads `blueprint.assets` and a `BlueprintReferencedAssets` list:

- `decompiled/Assembly-CSharp/Kingmaker.Blueprints.JsonSystem/StartGameLoader.cs:35`
- `decompiled/Owlcat.SharedTypes/Kingmaker.SharedTypes/BlueprintReferencedAssets.cs:1`

The exact difficulty table contents were **not extracted**. This is why I do not assert the final DC for the concrete dialogue fixture below.

## Scope beyond the listed systems

Measured additional families:

| Family | Physical lines |
|---|---:|
| Full `Kingmaker.UnitLogic*` | 87,886 |
| Abilities | 13,767 |
| Kingdom | 22,149 |
| Armies | 14,288 |
| Global map | 8,228 |
| Persistence versioning | 3,990 |
| Settings | 5,807 |

Progression, class/feat selection, spellbooks, items, equipment, polymorphs, pets/mounts, rest, travel, crusade and DLC/campaign gating all affect “same game” fidelity. Save versioning and post-load fixes are part of observable behavior, not merely migration housekeeping.

## Existing tools and documentation

Owlcat’s official modification template documents blueprint JSON, patching, EventBus integration, Harmony and controller extension points. **Its README specifies Unity 2019.4.26f1**, so it should not be treated as a verified drop-in extraction/editor project for this Unity 2020.3 build. ([github.com](https://github.com/OwlcatOpenSource/WrathModificationTemplate))

AssetRipper is a credible extraction candidate with broad Unity-version support, but its maintainers explicitly distinguish supported versions from uniformly reliable extraction. **I did not test it on these bundles**, and a successful Unity export would still require a game-specific conversion pipeline to Bevy-compatible data. ([github.com](https://github.com/AssetRipper/AssetRipper))

The current decompilation also does not cover every third-party implementation. Navigation-library internals, Unity serialization/rendering behavior and native Wwise behavior require separate investigation.

## Architecture recommendation

Use Bevy as the host and presentation framework, not as the definition of compatibility:

```text
content
  Blueprint IDs, typed schemas, defaults, localization, asset references

save_compat
  ZIP documents, object references, stable entity IDs, version handling

simulation
  Entities/facts, ordered rules, mechanics DSL, narrative state, clocks

presentation
  Bevy entities, meshes, animation, UI, camera, effects, audio adapters

oracle_tests
  Versioned inputs, draw tapes, nested traces, state-delta comparisons
```

Keep the simulation usable headlessly. Maintain explicit ordering and reentrant rule/context stacks; do not let parallel scheduling determine mechanics.

Finally, this survey is **code-informed architecture research**, not an isolated clean-room implementation process. If implementation isolation matters, convert these findings into behavioral/data-format specifications and oracle fixtures, and establish the implementation/provenance boundary before writing the replacement.

---

# A. Smallest fidelity-bearing vertical slice

## Recommended slice

**Load the real save and JSON blueprints, hydrate the relevant party/narrative state, then replay the Horgus payment dialogue check from an oracle-recorded answer checkpoint to the same resulting cue, flag, XP and histories.**

Run it in a headless Bevy app or minimal text UI. The comparison boundary is:

> After selecting `Answer_0011`, resolving `Check_0013`, executing the resulting cue’s `OnShow`, and rebuilding its answer list—but before subsequent controller ticks or another answer selection.

This proves a meaningful chain:

```text
Real save graph
 → durable entities and facts
 → authored answer eligibility
 → party stat selection
 → nested rule/dice execution
 → real dialogue branch
 → real persistent mutation
 → localized output
```

It does **not** claim complete unit lifecycle restoration, full conversation completion, or playable area reconstruction.

## Concrete content

Archive folder:

`World/Dialogs/c0/CavesUnderKenabres/NeathholmVillage/Horgus_GetMeOut/`

It contains **48 `.jbp` files**: one dialogue, two answer lists, twelve answers, thirty cues, one check, one cue sequence and one sequence exit.

| Object | Asset ID | Relevant behavior |
|---|---|---|
| `Horgus_GetMeOut_dialogue` | `7c26eaf59be4e5848927d73526e20748` | Conversation root |
| `Answer_0011` | `3ab564082485b034a9d0a7b550e1a3e2` | Negotiation answer; global show-once/history conditions |
| `Check_0013` | `5732a0f69d6777e46865f33dc7a095be` | `CheckDiplomacy`, authored DC **16**, party-selected roller |
| `Cue_0014` | `a2e8fcae1aea51f439cd1783e4a15c36` | Success: unlock/set price flag to **2000** |
| `Cue_0015` | `7f5fa939ab726d8449c7da6200843071` | Failure: unlock/set same flag to **1000** |
| `AnswersList_0009` | `12e42316950f8c9498afb8b0fb2baaae` | Seven possible answers after the branch |
| `Horgus_GetMeOut_price` | `ddfedbdeab95ed941b6968b06162c921` | Persistent price flag |

The selected answer’s conditions require three alternative negotiation answers not to have been selected. The rebuilt answer list additionally uses `HasFact` with `PlayerCharacter` evaluators for alternative answers.

In the inspected save:

- Party state contains **one unit**.
- It contains **48 fact instances**, referring to **31 distinct fact blueprints**.
- Saved Charisma is **10**.
- Saved Persuasion `PermanentValue` is **4**; the dependent diplomacy check also carries **4**.
- `Answer_0011` and its three mutually relevant alternatives are not recorded as selected.
- The price flag is already **1000**.

These are fixture observations, **not proof that the live computed check bonus is exactly 4 after all load callbacks**.

The save has Normal difficulty with the examined stats-adjustment setting. **Do not infer final DC 16 from the authored DC alone:** `RuleSkillCheck` applies difficulty data that still needs extraction or oracle export.

## Required semantic types and source surface

This is the **minimum semantic manifest**, not the transitive C# compile dependency graph.

| Responsibility | Types needed | Principal source |
|---|---|---|
| Blueprint decoding | `BlueprintJsonWrapper`, `GuidClassBinder`, `FieldsContractResolver`, `BlueprintReferenceBase`, blueprint/shared-string converters; concrete defaults | `decompiled/Assembly-CSharp/Kingmaker.Blueprints.JsonSystem/BlueprintJsonWrapper.cs:1` |
| Save decoding | ZIP reader; save contracts; `$id/$ref`; entity/blueprint/dictionary/countable-flag converters | `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Persistence.JsonUtility/DefaultJsonSettings.cs:1` |
| Durable state | Relevant fields of `Player`, `EntityDataBase`, `UnitEntityData`, `UnitDescriptor`, `EntityFact`, `Feature`/`Buff`; party IDs, fact membership, unit eligibility | `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Entities/UnitEntityData.cs:1` |
| Stats | `CharacterStats`, `ModifiableValue`, attribute/skill/dependent stat classes, modifier descriptors | `decompiled/Assembly-CSharp/Kingmaker.EntitySystem.Stats/CharacterStats.cs:1` |
| Rule execution | `Rulebook`, `RulebookEvent`, `RulebookEventContext`, scoped subscriber ordering | `decompiled/Assembly-CSharp/Kingmaker.RuleSystem/Rulebook.cs:67` |
| Check execution | `RuleStatCheck`, `RulePartyStatCheck`, `RuleSkillCheck`, `RuleRollDice`, `RuleRollD20`, `SkillCheckResult` | `decompiled/Assembly-CSharp/Kingmaker.RuleSystem.Rules/RuleSkillCheck.cs:1` |
| Dialogue | Relevant `DialogController` transition logic; `BlueprintDialog`, `BlueprintCueBase`, `BlueprintCue`, `BlueprintAnswerBase`, `BlueprintAnswer`, `BlueprintAnswersList`, `BlueprintCheck`, `CueSelection`, `CharacterSelection`, `DialogState` | `decompiled/Assembly-CSharp/Kingmaker.Controllers.Dialog/DialogController.cs:641` |
| Conditions | `Element`, `Condition`, `ConditionsChecker`, `AnswerSelected`, `HasFact`, `PlayerCharacter` evaluator | `decompiled/Assembly-CSharp/Kingmaker.Designers.EventConditionActionSystem.Conditions/AnswerSelected.cs:1` |
| Actions/flags | `GameAction`, `ActionList`, `UnlockFlag`, `BlueprintUnlockableFlag`, `UnlockableFlagsManager`; etude dirty notification | `decompiled/Assembly-CSharp/Kingmaker.Designers.EventConditionActionSystem.Actions/UnlockFlag.cs:1` |
| Difficulty | Relevant settings and `DifficultyPresetsList.StatsAdjustmentPreset` | `decompiled/Assembly-CSharp/Kingmaker.Settings.Difficulty/DifficultyPresetsList.cs:1` |
| Experience | `ExperienceHelper`, progression tables, dialogue XP modifier table, relevant party/initiator XP distribution | `decompiled/Assembly-CSharp/Kingmaker.Blueprints.Classes.Experience/ExperienceHelper.cs:1` |
| Text | `LocalizedString`, `LocalizationPack`, shared-string key resolution and encountered text-template handlers | `decompiled/Assembly-CSharp/Kingmaker.Localization/LocalizedString.cs:1` |

The answer-list conditions specifically require:

- `decompiled/Assembly-CSharp/Kingmaker.Designers.EventConditionActionSystem.Conditions/HasFact.cs:1`
- `decompiled/Assembly-CSharp/Kingmaker.Designers.EventConditionActionSystem.Evaluators/PlayerCharacter.cs:1`

### Fact-handler closure: an explicit certification gate

The save’s fact blueprints include stat/fact-related components such as:

- `AddFacts`
- `AddClassSkill`
- `AddBackgroundClassSkill`
- `AddStatBonus`
- `AddStatBonusIfHasFact`
- `AddFeatureOnSkill`
- `AddFeatureIfHasFact`
- `RecalculateOnStatChange`

Sources include:

`decompiled/Assembly-CSharp/Kingmaker.UnitLogic.FactLogic/AddStatBonus.cs:1`  
`decompiled/Assembly-CSharp/Kingmaker.Designers.Mechanics.Facts/AddFeatureIfHasFact.cs:1`  
`decompiled/Assembly-CSharp/Kingmaker.Designers.Mechanics.Buffs/AddStatBonusIfHasFact.cs:1`

**I have not established an exact complete active runtime-handler closure for this loaded unit.** The slice must either implement and certify that closure, or explicitly limit itself to an oracle-certified loaded-state/check boundary. Reading saved cached values while ignoring relevant active fact handlers would not qualify as full party-state restoration.

This limitation should remain visible in the implementation plan.

## Size estimate

Measured reference anchors:

- Seven JSON-loader core files: approximately **524 LOC**, excluding common blueprint infrastructure.
- Entity/unit/descriptor/fact classes alone: **5,555 LOC**.
- Relevant stats/reference classes: approximately **2,000 LOC**.
- Rulebook/check/dice core: approximately **1,200 LOC**.
- Dialogue families: approximately **3,250 LOC**.
- Additional save conversion, conditions, flags, settings, XP and text: several thousand lines.

A reasonable **reference-reading surface is roughly 15–20k physical C# lines**, with overlap and partial-class use.

**Planning guess, not a measured port estimate:** a carefully bounded headless implementation might be **6–12k Rust lines**, excluding extractor tooling and a generalized fact system. An exact handler closure or broader restoration requirement could expand that considerably.

## Acceptance tests

The slice passes only when it matches the oracle on:

1. Imported durable identities, relevant fact membership and object references.
2. Visible/selectable answer GUIDs before selection.
3. Chosen roller and full check-bonus breakdown.
4. Authored DC, difficulty adjustment and final DC.
5. Every nested rule and RNG draw consumed.
6. Success/failure cue GUID.
7. Flag presence and value.
8. XP delta and distribution.
9. Global/local selected-answer, shown-cue and passed/failed-check state.
10. Rebuilt answer list and resolved text.

Use recorded success and failure traces. Since the save does not establish the next die, use a **recorded draw tape**, not an invented “save RNG seed.”

### Why stop here?

Later in the same conversation, `Cue_0024.OnStop` starts `Horgus_GoesBack` and grants an objective. The cutscene is:

`World/Cutscenes/Prologue_Neathholm/Horgus_GoesBack/Horgus_GoesBack.jbp`

It has movement/animation/timing implications. **Treating that cutscene as a no-op to claim full conversation parity would be misleading.**

The next milestone should be the complete conversation with its world-side effects; after that, one complete weapon attack through AC, dice, damage and resulting fact/HP changes.

---

# B. Using the running game as a test oracle

## Existing integration foothold

The repo already has a UMM/Harmony host pattern:

- `mods/WrathStats/Main.cs:24`
- `mods/WrathStats/Core/Patches.cs:1`

The entry point loads settings, creates Harmony patches and creates a persistent recorder MonoBehaviour. That is a useful scaffold for a **separate compatibility recorder**, not evidence that the proposed hooks have already been tested.

Harmony supports prefix/postfix/transpiler patches, but patch design must account for the exact target and runtime; generic method interception should not be assumed reliable without validation. ([harmony.pardeike.net](https://harmony.pardeike.net/articles/patching.html))

## Rulebook hooks

Prefer the game’s existing non-generic hook interfaces:

- `IRulebookEventAboutToTriggerHook`  
  `decompiled/Assembly-CSharp/Kingmaker.PubSubSystem/IRulebookEventAboutToTriggerHook.cs:1`
- `IRulebookEventDidTriggerHook`  
  `decompiled/Assembly-CSharp/Kingmaker.PubSubSystem/IRulebookEventDidTriggerHook.cs:1`
- `RulebookEventHooksManager`  
  `decompiled/Assembly-CSharp/Kingmaker.PubSubSystem/RulebookEventHooksManager.cs:1`

Register a recorder through `EventBus.Subscribe`. These hooks receive the base `RulebookEvent`, avoiding a separate subscriber implementation for every concrete rule type.

Use three observation boundaries:

| Boundary | Capture |
|---|---|
| About-to hook | Input state before ordinary rule subscribers |
| Did-trigger hook | Result after ordinary did-trigger subscribers |
| Harmony postfix on `RulebookEvent.OnDidTrigger` | Final state after temporary modifier cleanup |

The third target is:

`decompiled/Assembly-CSharp/Kingmaker.RuleSystem/RulebookEvent.cs:161`

Record nesting and parent event identity. Do not flatten nested checks into an unordered event list.

## RNG hooks

Instrument the managed normal-dice boundary:

`RulebookEvent.Dice.D(int, DiceType)`

Record each underlying die draw, not just the sum. Also record:

- `RuleRollDice` roll history, overrides and rerolls.
- Forced-success/failure branches that draw outside the normal helper.
- Dialogue random selection.
- AI/random scheduling where relevant to a world-combat fixture.
- Fake/preview checks if they consume randomness.

A managed call-site transpiler or narrowly scoped postfix is preferable to assuming Harmony can patch Unity’s native RNG implementation directly.

For replay, give Rust a tape containing:

```text
event path / call site
range or dice specification
returned value
draw sequence number
```

Fail on missing or extra draws. Capturing full Unity RNG state can support future algorithm experiments, but **draw-tape replay separates mechanics parity from RNG-algorithm parity**.

## Dialogue hooks

Useful controller targets:

- `StartDialog`
- `SelectAnswer`
- Private `PlayCue`
- Private `PlayCheck`
- Current-cue setter

Use the paths and lines in section 5.

Capture:

- Dialogue/cue/answer IDs.
- Acting unit and roller.
- Available answer IDs and selectability.
- Condition results.
- Local and global histories.
- Authored versus effective DC.
- Actions executed in order.
- State after outgoing `OnStop` and incoming `OnShow`.

`IDialogCueHandler` and `ISelectAnswerHandler` provide additional high-level observation points. They supplement, rather than replace, rule/action traces.

## Persistent mutation hooks

Observe:

- `UnlockableFlagsManager.SetFlagValue`, unlock and lock.
- `EtudesSystem.StartEtude`, completion and update boundaries.
- Quest/objective mutations.
- XP mutations.
- Relevant fact add/remove/rank and buff changes.

For condition/action tracing, instrument concrete overrides or composition boundaries. **Patching an abstract `GameAction.RunAction` declaration does not automatically observe every override.**

## Fixture design

Each fixture should include:

- Exact game/build and assembly hashes.
- Blueprint ZIP and binary-pack hashes.
- Mods and load order.
- Difficulty, campaign/DLC and relevant settings.
- Save/area IDs and initial relevant state.
- Input actions.
- Ordered nested rule trace.
- RNG draw tape.
- Final state delta.
- Relevant Unity-asset data, such as raw difficulty table entries.

Export raw configuration as well as computed results. Feeding Rust only oracle-derived totals would test transcription, not implementation.

Snapshot live objects on the main thread, then serialize immutable DTOs in the background. Avoid recursively serializing whole units/views. Report dropped records and keep observation code from consuming RNG or triggering rules.

**No oracle recording or hook validation was performed during this survey.**

---

# C. Biggest risks, ranked

## 1. Silent semantic incompleteness

**Highest risk.** Thousands of executable content types, active fact handlers, initialization and post-load callbacks can affect otherwise straightforward checks. A port that silently ignores unknown behavior can look convincing while being systematically wrong.

**Mitigation:** execution coverage manifests, fail-closed unsupported handlers, ordered traces and state-delta tests.

## 2. Scene-authored gameplay and asset extraction

Blueprints and saves do not contain all spawners, locators, map objects, trigger geometry, stable view identities or ScriptableObject configuration. Missing these prevents faithful world behavior even with perfect rules.

**Mitigation:** extract one small area end-to-end, including identities and gameplay components, before committing to a campaign-scale asset pipeline.

## 3. Save restoration versus merely reading JSON

Shared object identity, named component data, derived stats, activation, timers and version fixes all matter. Saved values can be useful observations without being a complete executable state.

**Mitigation:** distinguish parsed, hydrated and lifecycle-certified state; compare post-load snapshots and the first subsequent mutations.

## 4. Nondeterminism and compatibility arithmetic

Global RNG consumers, variable frame timing, iteration/subscription order, integer overflow, rounding and runtime-dependent hashes complicate reproducibility. A save alone does not recover the historical next roll.

**Mitigation:** draw tapes first, explicit ordering, arithmetic edge-case fixtures, and separate deterministic redesign from compatibility claims.

## 5. Combat synchronization

Rules interact with movement, clearance, animation hit/cast timing, interruptions, mounts and turn budgets. A correct standalone attack roll is not proof of correct combat flow.

**Mitigation:** progress from isolated rules to a complete recorded command lifecycle with timing and resulting state.

## 6. Presentation and audio scale

Paper-doll equipment, skeleton/clip binding, custom shaders, VFX, UI behavior and Wwise packages represent several major subsystems. The measured source and asset volume make a complete visual replacement a substantial project.

**Mitigation:** declare fidelity tiers; prove mechanics headlessly, then one character/area/UI path.

## 7. An ambiguous baseline

JSON export, binary pack, runtime modifications and post-load fixes can produce different effective content. “Version 2.7.0y” is not enough to identify every behavior-affecting input.

**Mitigation:** hash and record the exact installation, mods, settings and effective loaded objects.

## 8. Campaign breadth

Progression, spellbooks, equipment, companions, travel, crusade, DLC and endings multiply the coverage burden. A successful early dialogue or combat fixture proves architecture, not campaign completeness.

**Mitigation:** explicit supported-content manifests and milestone boundaries rather than a blanket “Wrath compatible” label.

## 9. Decompiled-source and process limitations

The decompilation is valuable evidence, but it omits some third-party/native behavior and does not replace testing serialized Unity data or running behavior. A code-informed rewrite also needs a deliberate provenance/process boundary if “clean-room” isolation is an actual requirement.

**Mitigation:** behavioral specifications, trace-backed fixtures, independently tracked asset/tool provenance, and clear separation between measured facts and implementation proposals.

---

## Recommended first decision

**Build the oracle and bounded Horgus check slice before committing to a general Bevy world renderer.**

That experiment forces the project to solve the essential compatibility boundaries—content identity, save graphs, live stat semantics, ordered rules, RNG observation, dialogue execution and persistent mutation—while exposing the Unity-data dependency through the difficulty table. If those boundaries match the real game, the project has a credible foundation; if they do not, a renderer would only hide the uncertainty.