# POC-3 saves, first round: reading `.zks`, typed decoding, our format

Written 2026-10-10. Scope: PLAN.md's POC-3, first round: reading original saves with the
game's Newtonsoft semantics, typed decoding with a lossless fallback, a first version of
our own format with a preservation proof, and what hydration will need. Tags: **[M]**
measured on this Mac (M2 Max, 12 cores), **[C]** read in the decompiled code, **[I]**
inferred and not yet checked against the running game. Inputs were read in place, read
only: the fixtures from `fixtures/saves/` and the
other saves from the game's save folder. Reports, dumps and our files are under the
gitignored `cache/saves/`. No game text appears here, only type names, GUIDs and counts.

## Headline

- **Typed coverage [M]: 100% on both fixtures.** Every JSON value of every document of
  Manual_13 (140,771 values, 21,860 objects, 106 distinct `$type`s, 219 concrete types) and
  Manual_10 (3,304,476 values, 556,592 objects, 262 `$type`s, 494 concrete types) decodes
  through the generated schema: no opaque value, no unknown property, no coerced value,
  every `$ref` resolved. One value per save is raw JSON by design (a member declared
  `object` holding a plain JSON value). Values are counted as JSON values (objects,
  arrays, scalars; not property names), and the decoder's count equals a direct count of
  each document.
- **Long tail [M]:** all 17 saves in the folder decode 100% typed (4.84 M objects, 522
  distinct concrete types between them). The schema covers 3,478 types; the saves use 522.
  By object count the top 10 types are 50% of all objects, the top 100 94%, the top 200
  99.4%; 60 types have fewer than 10 objects across all saves.
- **Blueprint references [M]:** 13,670 in Manual_13 and 322,537 in Manual_10 (2,984 and
  32,333 distinct GUIDs). All resolve in `blueprints.zip` with the right type, except 263
  in Manual_10 declared as the interface `IFeatureSelection`, which exist but can't be
  type-checked by name (the content schema has no interfaces).
- **Preservation [M]: holds for all 17 saves**, the fixtures included. Original `.zks` →
  typed values → our format → typed values again gives byte-identical canonical text and
  bit-identical values for every document, and every decoded value says exactly what the
  original JSON said under its declared C# type.
- **Our format [M]** (parallel over entries, median of 5–7 runs):

  | | Manual_13 | Manual_10 |
  |---|---|---|
  | Original `.zks` / JSON inside | 0.98 MB / 3.6 MB | 20.0 MB / 87.9 MB |
  | Our file | **0.61 MB** (62%) | **10.6 MB** (53%) |
  | Read and inflate the `.zks` | 4.5 ms | 123 ms |
  | Parse all JSON (our lossless parser / `serde_json::Value`) | 5.8 / 8.6 ms | 53 / 74 ms |
  | Parse and decode typed, all documents | 15 ms (27 single-thread) | 140 ms (704 single-thread) |
  | **Encode ours** | **1.9 ms** (3.9 single-thread) | **49 ms** (133 single-thread) |
  | **Decode ours** (with index validation) | **2.5 ms** (5.7 single-thread) | **40 ms** (167 single-thread) |

  The game itself only deserializes `player.json`, `party.json` and the current area's
  documents on load (the rest wait in the area stash), and we have no per-document
  timing of its Newtonsoft path yet (open question 1).
- **Hydration is not done** (it needs `wr-sim`). What it needs is listed below, with a
  ranked draft of types and hooks for Manual_13.

## The format as the game writes it

### Archive

A `.zks` is a zip (`ZipSaver`, Ionic Zip, deflate) of separately serialized documents
[C: `Kingmaker.EntitySystem.Persistence/SaveManager.cs:964` `SerializeAndSaveThread`,
`AreaDataStash.cs:57` `StashAreaState`, `ThreadedGameLoader.cs:83` `DoLoad`]:

| Entry | Root type | Notes |
|---|---|---|
| `header.json` | `SaveInfo` | Manual_13's starts with a UTF-8 BOM, Manual_10's doesn't [M] |
| `player.json` | `Player` | campaign state: quests, etudes, flags, dialogue, settings, game time |
| `party.json` | `SceneEntitiesState` | the cross-scene state (party units, their items) |
| `<area guid>.json` | `AreaPersistentState` | one per visited area |
| `<area guid><scene>.json` | `SceneEntitiesState` | one per additional scene of an area (`AreaDataStash.FileName`; long names are cut to 62 characters) |
| `tbm.json` | `CombatController` | empty or a BOM outside turn-based combat [M: 3 bytes in Manual_13, 0 in Manual_10]; **never read on load** [C: `ThreadedGameLoader.cs:236`] |
| `clockwork.json`, `statistic.json` | QA (`PlayData`, a string) | absent from Ember's saves |
| `<area guid>.<scene>.fog` | fog-of-war mask | JPEG, quality 50 [C: `FogOfWarArea.cs:278`; M: all 133 in Manual_10 are JFIF] |
| `header.png`, `history`, `checksums` | screenshot, game-history text log, debug MD5s | `checksums` is MD5 bytes run through `Encoding.Default`, so lossy text [C: `SaveManager.cs:1390`] |

Manual_10 holds 423 entries: 68 area states, 216 scene states, 133 fog masks, a 26 MB
`history` [M].

Before deserializing, `JsonUpgradeSystem` rewrites the text of saves whose
`SaveInfo.Versions` lacks one of the six registered upgraders (10–60), and refuses a save
naming an unknown version [C: `Versioning/JsonUpgradeSystem.cs:35,64`]. All of Ember's saves
carry exactly the six [M], so no text upgrade applies. Our reader fails closed: it refuses
a save that would need an upgrader or that the game would reject (`zks::check_versions`).

### Serializer settings

`DefaultJsonSettings.DefaultSettings` [C: `JsonUtility/DefaultJsonSettings.cs:76`]:
`TypeNameHandling.Auto`, `PreserveReferencesHandling.Objects`,
`DefaultValueHandling.Include`, `ReferenceLoopHandling.Serialize`, the default binder,
`OptInContractResolver`, and 22 converters. The game's `Newtonsoft.Json.dll` is version
8.0 (decompiled into the scratchpad to check the reader's behaviour; not kept).

**Contracts** (`OptInContractResolver.cs`): members are fields and properties carrying
`[JsonProperty]` (or `[JsonRequired]`), any visibility, base classes included, a derived
declaration replacing a base one of the same JSON name; types from
`UnityEngine.CoreModule` instead get Newtonsoft's default contract (public fields and
public readable properties). Types in Assembly-CSharp are built through a
`JsonConstructorMark` constructor, else a parameterless one; a `[JsonConstructor]`
constructor with parameters (e.g. `EntityFactsManager(EntityDataBase owner)`) makes
Newtonsoft read every property first and construct afterwards. Newtonsoft picks the
contract kind first: `[JsonObject]` → object, `IDictionary` → dictionary, `IEnumerable` →
array, else object.

**Converters** are matched on the **declared** type when reading, in registration order;
a converter that can't read (`CanRead == false`) falls back to the contract:

| Converter | Declared type | Reads |
|---|---|---|
| `BlueprintConverter` | strict subclasses of `SimpleBlueprint`, `IFeatureSelection` | a GUID string; `"null"` or null → null; **throws if the blueprint doesn't exist** |
| `BlueprintGuidConverter` | `BlueprintGuid` | `Guid.TryParse`; `""` → empty |
| `BlueprintRefConverter` | strict subclasses of `BlueprintReferenceBase` | a GUID string, `{"guid": …}`, or null/empty → null; resolved lazily |
| `BoneTransformDataConverter` | `BoneTransformData` | `"x|y|z|ex|ey|ez"` in hundredths |
| `BoundsConverter` | `Bounds` | `{"center", "size"}` through the vector converter; anything else → default |
| `ColorConverter`, `RectConverter`, `MechanicsContextConverter`, `SceneEntitiesStateConverter`, `ComponentDelegateRuntimeConverter`, `UnitStripConverter` | | write only: reading uses the contract |
| `CountableFlagConverter` | `CountableFlag` | an integer, null (0), or `{"m_Count": n}` |
| `DictionaryConverter` | generic `Dictionary<,>` / `IDictionary<,>` | an object keyed by string, int or enum name, or `[{"Key", "Value"}, …]`; a failing entry is logged and dropped, a repeated key ignored |
| `EntityRefConverter` | `UnitReference`, `MapObjReference`, `EntityRef`, `EntityRef<T>` | an id string, null, or an object's first property |
| `SpriteConverter` | `Sprite` | always null |
| `Vector2Converter`, `Vector3Converter`, `QuaternionConverter` | exact type | `"x|y(|z)"` as `int.Parse(part) / 100f` (Euler angles for quaternions), or an object |
| `CollectionsConverter` | lists, arrays, sets | only with the `saves-support` command line flag: off |
| `LocalizationPack.EntryConverter` | `StringEntry` | a string |
| `StringEnumConverter` (Owlcat's) | enums, `Nullable<enum>` | a name, a name list, a number string (`Enum.Parse`); an unknown name logs and reads 0; an integer through `Enum.ToObject` |
| `VersionConverter` | `Version` | a string |

**References** [C: Newtonsoft `JsonSerializerInternalReader`, `DefaultReferenceResolver`]:
`$id` scopes are per `Deserialize` call, so per document. An object's `$id` is registered
right after creation, before its properties, so references from inside it to it work
(cycles); for constructor-with-parameters types it is registered only after all
properties were read. A second object under an id already registered throws
(`BidirectionalDictionary.Set`). A `$ref` to an id not registered yet resolves to **null**
without error (`TryGetByFirst` fails). Metadata (`$ref`, `$type`, `$id`, `$values`) counts only as
a leading run of properties; a `$ref` object must have no other property. Ids inside JSON
a converter loads as a `JObject` and only partly reads are never registered, and neither
are ids inside members Newtonsoft skips (non-writable and not populating an existing
object).

**Constructors:** a JSON property whose name matches a creator parameter
(case-insensitively) feeds it, so it is read even when its member has no setter
(`GlobalMapArmyState.ArmyType`). A class with neither a usable parameterless constructor
nor exactly one public constructor can't be created.

**Types:** `$type` is an assembly-qualified CLR name, e.g.
``Kingmaker.EntitySystem.EntityFactComponentDelegate`2+ComponentRuntime[[…UnitEntityData,
Assembly-CSharp, Version=…],[…AddFactsData, …]], Assembly-CSharp``; nested types of
generic types take the enclosing type's arguments. It must name a type assignable to the
declared one, from the named assembly (generic arguments included); without an assembly,
`Type.GetType` only finds mscorlib types. An unknown or unassignable `$type` fails the
whole document.

**Numbers** (`JsonTextReader`): `int`, `short`, `byte` are read with `ReadAsInt32`
(integer tokens only; leading zeros are read as octal), other integers as `Int64` and
converted with overflow checks, `float` and `double` as a double (`double.Parse`) then
rounded to float. Floats are written as `"R"` text, NaN and infinities as strings.

**Quirks found** [C unless noted]:

- A `Nullable<Vector3>` member is written through Vector3's *default contract*, with an
  `$id` and `$type: "System.Nullable`1[[UnityEngine.Vector3…]], mscorlib"` and the
  read-only `normalized`, `magnitude`, `sqrMagnitude`, which the reader skips [M: 7 in
  Manual_10's random-encounter states].
- `MechanicsContextConverter` writes `m_ShadowFactorPercents` a second time instead of
  `m_ShadowDisbelieveData`, so that list never survives a save. Not seen in the saves.
- Alushinyrra's `IsleData.IsLocked` is a getter-only `CountableFlag` read through its
  converter, so Newtonsoft skips it: written, never read back [M: 172 in the Act 1+
  saves].
- `DictionaryConverter` loads entries as `JObject`s with `DateParseHandling.DateTime`, so
  an ISO-date-like string inside a dictionary value would come back reformatted. Counted:
  none in the fixtures. It reads `Key` and `Value` by name; a key that fails to convert
  makes it skip the value unread; repeated keys keep the first.
- `JsonTextReader` reads an unsigned number with a leading zero as octal (`010` is 8).
  The decoder keeps such numbers opaque.
- `NullValueHandling.Ignore` / `DefaultValueHandling.Ignore` members skip a null / default
  token on read, keeping the initializer (`JsonSerializerInternalReader.cs:857`); the
  schema flags them for hydration.
- `SceneEntitiesStateConverter` drops trap actors and, with `RemoveLootAndCorpses`, some
  corpses and loot, and replaces optimizable units with `UnitPlaceholderEntityData`
  (survey §2). A save is not a full dump of a scene.

## How our reader follows it

**`tools/wr-codegen gen-save`** (new, beside the blueprint `gen`) builds the save schema
from the same parsed declarations, now with properties and constructors:

- roots: `SaveInfo`, `Player`, `SceneEntitiesState`, `AreaPersistentState`,
  `CombatController`, `PlayData`;
- members by the opt-in rules above (`[JsonProperty]` names, readable/writable,
  `DefaultValueHandling`/`NullValueHandling`/`ObjectCreationHandling` flags), Unity core
  types by the default contract;
- codecs by the converter table, matched on declared types in registration order;
- constructors: the creator Newtonsoft would use (a `JsonConstructorMark` or parameterless
  constructor, a `[JsonConstructor]`, or the single public one) and, for creator
  parameters, the parameter's own type as the codec (a member of the same name and another
  type is read as the parameter); parameters with no member become members of their own;
- polymorphism: every class or struct a `$type` could name (non-generic declarations,
  closed generic types found in any declaration's ancestry, and nested types of those)
  joins the closure when it derives from a declared type or implements a declared
  interface. Nested types of generic types get their enclosing arguments, including
  nested types inherited through a base class (the model skipped those when computing
  bases);
- `UnityEngine.Object` members (never non-null in saves) are a codec of their own rather
  than a walk of the engine's object graph; UniRx's `ReactiveProperty<T>` (not decompiled)
  is an empty opt-in contract.

The result: **3,478 types, 96 enums**, keyed by CLR name without assembly
qualifications, with each type's interfaces (for `$type` checks) and lifecycle hooks
(`crates/wr-save/src/generated/schema.rs`, 3.3 MB, tables only). The blueprint schema is
regenerated byte-identical. Generation takes 1.8 s.

**`crates/wr-save`:**

- `json`: a lossless JSON tree (key order, duplicate keys, number lexemes, BOM,
  `NaN`/`Infinity`).
- `clr`: `$type` name parsing.
- `decode`: the typed decoder, mirroring Newtonsoft's reader as described above (metadata,
  `$id` registration timing, forward/dangling/skipped references, assignability and
  assembly checks, converter rules, C# numeric rules). Each value is typed, **coerced**
  (what the game reads plus the original JSON, when a converter changes it), or
  **opaque** (the JSON and a reason). Unknown properties are kept as JSON and counted.
  Each decoded value keeps what's needed to write the JSON back: enum and GUID spelling,
  positions as integer hundredths, `$id`s, `$type` strings, property order.
- `canon`: the canonical text of a decoded document, and a type-aware check of decoded
  values against the original JSON.
- `format`: our format. `zks`: archive reading. `report`: coverage, references,
  preservation, long tail, hydration ranking.
- CLI (`cargo run --release -p wr-save -- <cmd>`): `report <save>`, `roundtrip <save>`,
  `longtail`, `hydration <save>`, `dump <save> <entry>`; `<save>` is a path or a name
  prefix.

**What counts as typed:** a JSON value (object, array or scalar; property names aren't
values) consumed by a schema codec, including coerced values (none in these saves). Opaque
values and unknown properties count as untyped, with every value inside them. Raw JSON
under members declared `object`/`JToken`/`decimal` counts as typed but is reported
separately (1 per save). Typed plus untyped equals a direct count of the document (tested).
An object is *clean* when none of its own properties is opaque or unknown. Values the game
would reject are never counted as typed: unknown or unbindable `$type`s, duplicate `$id`s,
out-of-range or non-canonical integers, octal numbers, impossible dates, malformed
converter input all become opaque.

## Coverage [M]

| | Manual_13 | Manual_10 |
|---|---|---|
| Entries / JSON documents | 12 / 7 | 423 / 286 |
| Objects / clean | 21,860 / 21,860 | 556,592 / 556,592 |
| Distinct `$type`s / concrete types | 106 / 219 | 262 / 494 |
| JSON values / typed | 140,771 / 100% | 3,304,476 / 100% |
| Opaque values, unknown properties, coerced values | 0, 0, 0 | 0, 0, 0 |
| `$id`s / `$ref`s (all resolve backwards) | 17,808 / 6,937 | 447,533 / 139,559 |
| Skipped properties (no setter, not populating) | 0 | 193: `IsleData.IsLocked` 172, `Nullable<Vector3>`'s three read-only properties 21 |
| Blueprint references: total / distinct / resolve with the right type | 13,670 / 2,984 / all | 322,537 / 32,333 / all but 263 unchecked (`IFeatureSelection`) |
| Entity references / ids defined in the save / references to ids not in the save | 3,417 / 1,074 / 128 | 91,225 / 28,679 / 1,835 |

References to entities not in the save point at scene-authored entities that the game
resolves after loading the scene, or at units in other areas [I]; hydration must resolve
them against imported scene data. Per-type tables (objects, clean objects, properties,
unknown properties for every concrete type) are in
`cache/saves/reports/<save>.coverage.json`.

## Long tail [M]

Every save in the folder, decoded (`wr-save longtail`, `cache/saves/reports/longtail.json`):

| Save | `.zks` MB | Objects | `$type`s | Concrete types | Typed | Decode ms |
|---|---|---|---|---|---|---|
| Manual_8 (Shield Maze) | 22.1 | 613,467 | 269 | 508 | 100% | 275 |
| Manual_9 (Defender's Heart) | 19.1 | 531,332 | 264 | 495 | 100% | 231 |
| Manual_10 (Defender's Heart) | 20.0 | 556,592 | 262 | 494 | 100% | 263 |
| Manual_11 (Drezen) | 18.4 | 506,256 | 263 | 493 | 100% | 228 |
| Manual_12 (Worldwound) | 18.5 | 508,160 | 263 | 493 | 100% | 229 |
| Manual_13 (caves) | 1.0 | 21,860 | 106 | 219 | 100% | 18 |
| Manual_14 (WrathStats bench) | 1.1 | 15,260 | 95 | 208 | 100% | 13 |
| Manual_15–17, Auto_7–9 (prologue) | 0.9–1.0 | 21,865 | 106 | 219 | 100% | 18–19 |
| Auto_4–6 | 22.9–23.3 | 633,975–656,590 | 267–270 | 505–510 | 100% | 277–327 |
| ForImport_1 | 5.7 | 34,614 | 174 | 379 | 100% | 69 |

At the decoding level there is no long tail left in these saves: every type they use is
generated from the C# rather than handwritten. The long tail POC-8 cares about is
behavioural, and its shape here is: 522 concrete types over 4.84 M objects; the top 10
types hold 50.0% of objects, the top 25 68.2%, the top 50 81.3%, the top 100 94.2%, the top
200 99.4%, the top 300 99.9%; 60 types have fewer than 10 objects and 4 have one. Saves
from later acts add types (prologue saves use 219, Act 1+ saves about 500), and other
saves (Act 3+, crusade, mythic paths) will add more; unknown types would show up as
opaque values with reason `unknown $type`.

## Our format (Decision 3, version 1)

```text
header   "WRSAVE\0\x01" | format version u32 | schema fingerprint u64 | entry count u32
index    per entry: name | kind (document or bytes) | raw length | stored length
chunks   per entry: zstd level 3 (postcard (entry))
```

Each entry is a separately compressed chunk, so encoding and decoding run in parallel and
a loader can read one area's documents alone (the basis for incremental saves: rewrite
only changed chunks). A document entry holds its typed values plus the preserved opaque
remainder (none in these saves); other entries hold their bytes. A file written with
another schema is refused (values index the schema's types, members and enums), as are
truncated, extended and corrupt files (tested).

This version stores the decoded documents, which is what we have until `wr-sim` exists:
the next version holds the simulation's own state in the same container, with the
remainder alongside. `rkyv` (zero-copy, memory-mapped, as the content pack does) is the
likely encoding then; postcard is used now because the decoded value tree is recursive and
the chunks are compressed anyway. Numbers are in the headline table: our files are 53–62%
of the `.zks`, encode in 1.8 / 51 ms and decode in 2.3 / 39 ms (Manual_13 / Manual_10).
For scale, the original manual save takes 0.33 s (PLAN.md baseline; a different save) and
loading takes 6.5–12.4 s, though most of that isn't JSON.

## Preservation

**Definition.** A decoded document's canonical text is written from its values alone:
properties in their original order, `$id`s and `$type`s as written, enums, GUIDs and
positions spelled as written, coerced and opaque values as their original JSON, typed
floats as the shortest text that reads back to the same `float`/`double`. Preservation
holds when, for every document, (a) the canonical text of the original's decoding and of
our format's decoding are byte-identical; (b) the decoded values are identical (compared
through their serialized bytes, so floats compare by bits and a NaN equals itself; this
also covers what the game reads for coerced values); and (c) every decoded value of the
original's decoding says exactly what the original JSON said (`canon::check_source`):
same structure and key order, identical strings and integer lexemes, a typed `float` equal
to the original read as a double and rounded, a typed `double` equal to the original read
as a double, coerced and opaque values identical JSON. A `$ref` is compared by id, and a
document can't hold two objects under one id (the decoder refuses it, as Newtonsoft
does), so references compare structurally. Non-JSON entries must be byte-identical. `$id`
renumbering is allowed by the definition but not needed: we keep ids.

**Result [M]:** holds for every document and entry of Manual_13 (7 documents, 12
entries), Manual_10 (286 documents, 423 entries) and the 15 other saves in the folder
(`wr-save roundtrip <save>`; our files are 53–64% of the `.zks`, 80% for the small bench
save Manual_14), and in the ignored tests `manual_13_decodes_and_round_trips` /
`manual_10_decodes_and_round_trips`. Kenabres's
state in Manual_13 (two area and scene documents the caves don't use) comes back with
every field.

## What hydration needs (not this round)

Decoding gives the object graph the game deserializes; it is not yet a live game. The load
path, traced statically by gpt-6.1-sol and spot-checked by Claude [C], is:
`Game.LoadGame` → `LoadArea` → `SaveManager.LoadRoutine` (dispose state, run
`ThreadedGameLoader`: deserialize `player.json` and `party.json`, index area documents as
lazy shells) → `Player.PostLoad`, `Player.TurnOn`, `CrossSceneState.TurnOn`,
`Player.ApplyUpgrades` (`SaveManager.cs:1290–1298`) → `SceneLoader` unstashes and
deserializes only the target area and its loaded scenes, `PostLoad`s them, matches saved
entities to scene-authored views by `UniqueId` (`SceneLoader.cs:1444`), turns them on →
`Game.AfterSceneLoaderFinishedArea` (area main fact activation or `TriggerOnLoad`, etude
update, entity creation and destruction queues, party onto the navmesh) → a second
`ApplyUpgrades` → `OnAreaLoadingComplete`.

Per entity, `EntityDataBase.PostLoad` runs `PrePostLoad` (register the entity, restore
`Facts.Owner`/`Parts.Owner`), `OnPostLoad`, `Facts.PostLoad`, `Parts.PostLoad`,
`OnComponentsDidPostLoad` (`EntityDataBase.cs:330`); `ApplyPostLoadFixes` and `TurnOn`
are separate passes with their own barriers per scene state (`SceneEntitiesState.cs:107`).
The key semantic: **`IsActive` is saved, `IsTurnedOn` isn't. Loaded facts get `PostLoad`
and `TurnOn`, not `Activate`.** The game saves after `TurnOff` (`SaveManager.cs:891`), so
effects a component adds in `OnTurnOn` are rebuilt on load, while effects added in
`OnActivate` are restored from saved state (stat modifiers live in
`PersistentModifierList`, which holds *all* current modifiers; `AddModifierUnique`
recognises a restored one by source fact and component name). Unit placeholders are not
hydrated at all: `BecomeUnit` spawns a fresh blueprint-initialised unit under the saved
id. One deserialization callback changes state: `MechanicsContext.[OnDeserialized]` sets
`m_CasterRef` to the owner when absent, which is exactly when the converter left it out
(caster == owner). The full trace with file:line references is in
`cache/research/poc3-lifecycle.md` (local; it quotes decompiled code).

What each item left out of this round needs:

| Item | Needs |
|---|---|
| Hydration into a live sim | `wr-sim`'s entity, part, fact and stat types; ownership links and the entity registry (`EntityRef` resolves lazily through `EntityService` proxies); blueprint resolution for every `BlueprintConverter` value (all resolve today); scene data from `wr-import` to match saved entities to authored ones by `UniqueId` and to resolve the 128 (Manual_13) / 1,835 (Manual_10) entity references to ids not in the save; placeholder respawning through the unit spawn path |
| Fact activation and post-load lifecycle | the per-scene barrier order above; `EntityFact.PostLoadComponents` (match saved component state to blueprint components by name, dispose orphans), `ApplyPostLoadFixes` (create components new in the blueprint, honour `ReapplyOnApplyPostLoadFixes`), `TurnOn` in `TurnOnPriority` order (no tie-breaker in the comparer: keep the saved order); `CharacterStats.PostLoad` then `UpdateValue` for every stat; player and unit upgraders (`Player.ApplyUpgrades`, `UnitUpgraderHelper`), each recording itself even when it throws; the `OnTurnOn` of every component on an active fact: Manual_13's 2,984 active facts carry 7,053 blueprint components of 243 types (`wr-save hydration`) |
| Snapshot equivalence | WrathOracle dumping every party unit's sheet with breakdowns, inventory, resources and buffs with timers after load (after `OnAreaLoadingComplete`), in a canonical form our sim emits too; buff times are absolute game time and aren't rebased on load |
| Mutation after load | the above, plus the commands and rules POC-4/5 bring, and our format's next version holding sim state (save → reload → compare) |

**Draft for Manual_13, ranked by the state that depends on each** [M counts from
`wr-save hydration`; hook lists from the schema; ranking judgement I]:

| # | Types | Objects / own properties | Hooks a loader must implement |
|---|---|---|---|
| 1 | `Feature` (+ `UnitFact`, `MainUnitFact`, `ItemEnchantment`) | 2,313 / 16,410 | `RestoreOwnerLink`, `PostLoad` + `PostLoadComponents`, `ApplyPostLoadFixes`, `TurnOn`; component delegates' `OnPostLoad`/`OnTurnOn` |
| 2 | `Etude` (+ `EtudesSystem`) | 1,306 / 10,443 | `EtudesSystem.OnPostLoad` (`RestoreTreeStructure`), `UpdateEtudes` after area activation, bracket components' `OnTurnOn` |
| 3 | `MechanicsContext` | 2,446 / 7,550 | `[OnDeserialized]` caster fix; context recalculation on `TurnOn` when flagged |
| 4 | Stats: `CharacterStats`, the `ModifiableValue` family and modifiers | 3,220 / 12,504 | `CharacterStats.PostLoad` (bind all, then `UpdateValue` all, in `CollectAllStats` order), per-subclass `OnPostLoad` (attribute subscriptions, AC and initiative rebuild their internal modifiers), `CleanupModifiers` |
| 5 | Abilities: `Ability`, `AbilityData`, `ActivatableAbility`, `Spellbook` | 1,235 / 7,247 | `Ability.OnPostLoad` → `AbilityData.PostLoad`, `Spellbook.PostLoad`, activatable `OnApplyPostLoadFixes` |
| 6 | Units: `UnitEntityData`, `UnitDescriptor`, `UnitProgressionData`, `UnitBody` and slots | 2,321 (60 units) / 5,558 | `UnitEntityData.OnPostLoad` (fact processors, `Buffs.UpdateNextEvent`, `Descriptor.PostLoad` before facts), `UnitDescriptor.ApplyPostLoadFixes`, unit upgraders, `OnTurnOn` (`UpdateIsActive`) |
| 7 | Items: `ItemEntity*`, `ItemsCollection`, `HandsEquipmentSet` | 805 (380 items) / 4,472 | `ItemsCollection.PostLoad`, `UnitBody.PrePostLoad`/`PostLoad`, `ItemEntity.OnTurnOn` (enchantments, wielder facts) |
| 8 | Spawners and scene entities: `UnitSpawnerBase+MyData`, `LocatorView+EntityData`, `MapObjectEntityData`, `ScriptZone` data, cutscene players | 1,312 / 6,143 | `SceneLoader.LoadSceneEntities` (match by `UniqueId`), `CreatesDataOnLoad`, spawner activation respecting `HasSpawned` |
| 9 | Fact component state (`ComponentRuntime` of 33 `TData` types: `AddFactsData`, `AddFeatureToNPCData`…) | 992 / 999 | `Setup` against the named blueprint component, delegate `OnPostLoad`, `OnTurnOn`; `AddFacts.UpdateFacts(postLoad: true)` restores missing children |
| 10 | `UnitPlaceholderEntityData` | 20 / 272 | `BecomeUnit`: spawn from blueprint with saved id, apply saved customisation and state |
| 11 | `Buff` | 102 (48 on units) / 979 | saved absolute end and tick times; `UpdateNextEvent`; no `OnAttach` on load |
| 12 | `Player` (own properties only; its quests, flags and dialogue sit in nested objects) | 1 / 66 | `Player.OnPostLoad` (quest book, cross-scene state, inventories, then its own `ApplyPostLoadFixes` *inside* `OnPostLoad`), `ApplyUpgrades` twice |

By category: 13 fact types hold 32,669 properties (40%), 22 entity types 9,106, 26 part
types 998, 33 component-state types 999, and 125 other types (stats, abilities, items,
contexts) 38,260. Hooks by objects affected: `PostLoad` 15,141, `ApplyPostLoadFixes`
11,816, `OnActivate` 4,713 (not run on load for existing facts), `OnPostLoad` 2,862,
`OnDeserialized` 2,446, `OnApplyPostLoadFixes` 1,920, `OnTurnOn` 1,494. The behaviour
behind the facts is the larger job: 243 blueprint component types on Manual_13's facts,
led by `AddFacts`, `AddProficiencies`, `ContextRankConfig`, `AddStatBonus`, prerequisite
and recommendation components (inert after level-up), and `AddDamageResistanceEnergy`.

## Tests

`scripts/check.sh` passes (fmt, clippy `-D warnings`, nextest: 84 tests, 3 ignored).
wr-save's unit tests use small synthetic documents against the generated schema:
backward, forward and dangling references, a cycle, a reference inside a polymorphic list
element, late `$id` registration for constructor-with-parameters types, unknown /
unassignable / wrong-assembly `$type`s, metadata rules (`$ref` with other properties,
`$type` before `$id`, a late `$id` as a plain property), unknown and case-insensitive
members, the vector converters (canonical, non-canonical, object form, too few parts) and
`Nullable<Vector3>` with its `Nullable` `$type`, enums (name, number, flags list, unknown
name, float), dictionaries (array form with a malformed entry and a repeated key, enum-key
object form, an unknown enum key, string-key object form with polymorphic values),
blueprint and reference converters (`"null"`, object form, empty), GUIDs, TimeSpan and
DateTime, integer range and spelling, entity references (string and object form), float
strings, the JSON parser (order, duplicates, lexemes, escapes, rejection), `$type` name
parsing (including its nesting bound), and our format (round trip, wrong magic, schema,
truncation, trailing bytes, a forged out-of-range type index). After review: duplicate
`$id`s, a failed dictionary key hiding its value's ids, octal floats, wrapping enum
integers, impossible dates, invalid `Nullable<enum>` names, unbindable assemblies
(assembly-less game types, a wrong generic-argument assembly), constructor-parameter
members, and the type-aware source check (integers exact where a float would round them
together). Each decoding test also checks the decoded values against the input with that
check and that every JSON value is counted exactly once. wr-codegen gained a parser test
for properties, constructors, their visibility and parameter names. The ignored fixture
tests check, per document, the source check, the value count, our format's bit-identical
round trip and the version guard.

## Review

GPT 6 Astra (high) reviewed crates/wr-save, the wr-codegen changes and this document twice
against the decompiled converters and the game's Newtonsoft 8. Every finding was checked
against the cited code before acting; all were real, and all are fixed and tested:

- **First round (19 findings).** The source check compared numbers type-blind (two
  integers that round to the same float passed): replaced by `canon::check_source`, which
  walks the decoded values against the original under the declared C# types. Duplicate
  `$id`s were accepted (Newtonsoft throws): refused. Octal numbers (`010`) were read as
  decimal: opaque. Wrapping enum integers lost what was written: coerced. A dictionary
  entry whose key failed still registered its value's ids, entries were matched by
  property order rather than name, and repeats were detected on our representation rather
  than C# keys: rewritten to follow `DictionaryConverter`. Constructor detection marked
  unconstructible classes (two public constructors, no default) as creatable, and members
  fed by a constructor parameter (`GlobalMapArmyState.ArmyType`) were counted as skipped:
  constructor visibility and parameter names now come from the parser. Getter-only arrays
  were treated as populated: skipped. Converter lookups took the first of repeated
  properties (`JObject.Load` keeps the last): refused. Assemblies of generic arguments and
  assembly-less game types weren't checked: they are. Invalid `Nullable<enum>` names read
  as 0 instead of null; impossible dates counted as typed; coverage counted property names
  in some places and not others (Manual_13's 226,598 became the true 140,771); a preserved
  NaN failed structural equality; a forged snapshot body could index outside the schema;
  `$type` parsing recursed without bound; archive sizes drove allocations; the version
  guard only reported. All fixed.
- **Second round (12 findings, on the fixes).** Octal numbers through the vector
  converter's object form; a duplicate `$id` taken by a child of a constructor-with-
  parameters parent; repeated properties nested inside converter-loaded values; failure
  detected only at the top of a dictionary value; `Entry::Laid` skipping the semantic
  check; a missing `Value` stopping the key's conversion; object-form dictionaries parsing
  keys before values (the game converts the value first); reference-identity keys not
  detected as repeats; extra entry properties not counted; constructor parameters read
  with the member's type instead of the parameter's (`UnitRunAway(Vector3 target)`);
  enum generic arguments' assemblies; and a `Versions` list repeated or written in octal.
  All fixed.

None of these changed the fixtures' or the other saves' results (they're all edge cases
the game's own writer doesn't produce), except the corrected value counts and the
`ArmyType` reclassification. Known limits, documented in the code: when a dictionary value
fails part-way, the ids the game registered before the failure point stay registered in
the game but not in our model (we don't track partial construction); a member without a
setter is assumed to have a non-null initializer to populate; octal dictionary keys are
kept opaque rather than converted; raw JSON under `object` members doesn't convert ISO
dates or octal numbers as Newtonsoft would.

## Open questions

1. **The game's own JSON cost.** We don't know how long Newtonsoft takes for
   `player.json`, `party.json` and an area in the game. WrathPerf or WrathOracle could
   time `OwlcatJsonConvert.DeserializeObject` per document (the `CodeTimer` blocks in
   `ThreadedGameLoader` already name them).
2. **Oracle comparison of decoded values.** Our decoding is checked against the game's
   reader by reading its code; the oracle should dump selected deserialized objects
   (before `PostLoad`) for a field-by-field comparison, as POC-2 plans for blueprints.
3. **Saves from later in the game** (Act 3+, crusade, a mythic path, a turn-based save
   with a non-empty `tbm.json`) would extend the long-tail curve.
4. **`IFeatureSelection`** references (263 in Manual_10) need an implements check on the
   content side to be type-checked.

## Effort

About one working session of agent time for the codegen extension, wr-save, the reports
and this document. The schema needed no handwritten per-type code; handwritten code is
about 6,300 lines (wr-save with its tests, and `save.rs` in wr-codegen), including the two
review rounds' fixes.
