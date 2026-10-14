# WrathOracle state dump format (`wrath-oracle-state-v1`)

Written 2026-10-11 for POC-1, so `wr-sim` and `wr-save` can build their comparisons against
it. Status: **final for v1** (the emitter, `WrathOracle/StateDump.cs`, implements exactly this;
example dumps for Manual_13 and Manual_10 are listed in `docs/poc1/oracle.md`). It is the contract for what WrathOracle 0.2 writes with the
`state:LABEL` plan step (and inside fixtures). Changes bump the schema string and are listed
at the end. No game text appears here: only type names, field names and formats.

A dump is the game's state **between two frames**, read without triggering any rule, action,
event or draw (the dump records `UnityEngine.Random.state` before and after; they must be
equal). Nothing is computed by WrathOracle except where a field says so.

## File

- `state-<label>.json` (or `.json.gz`, same bytes gzipped), UTF-8, no BOM, one JSON object.
- Top-level keys, in this order:

| Key | Contents |
|---|---|
| `schema` | `"wrath-oracle-state-v1"` |
| `label` | the plan step's label (e.g. `after-load`) |
| `identity` | build identity (below) |
| `frame`, `tick` | Unity `Time.frameCount`; the fixture trace's tick (game-mode ticks since the fixture window began, so a dump can be lined up with a trace), `-1` outside a fixture |
| `random_state_before` | `UnityEngine.Random.state` as `"s0:s1:s2:s3"` (8 lowercase hex digits each) |
| `game` | game time, mode, area, party, money (below) |
| `units` | every unit's character sheet (below), sorted by `id` (ordinal string order) |
| `narrative` | etudes, flags, quests, dialogue history, alignment (below) |
| `random_state_after` | the same, after the dump (must equal `random_state_before`) |
| `health` | recorder counters for this dump (below) |

## Value conventions

These apply everywhere, including inside the generic `persisted` objects.

| C# value | JSON |
|---|---|
| `bool` | `true` / `false` |
| integer types, `char` | JSON integer |
| enum | JSON integer (its underlying value; `[Flags]` enums as the combined integer). A sibling `*_name` string is added only where this document says so |
| `float`, `double` | JSON number written with .NET `"R"` (round-trip), so parsing to the same width gives the exact bits. Non-finite values are the strings `"NaN"`, `"Infinity"`, `"-Infinity"`. **Compare floats by parsing to the declared width and comparing bits** (any NaN equals any NaN) |
| `string` | JSON string; `null` stays `null` |
| `TimeSpan` | JSON integer: `Ticks` (100 ns units). `TimeSpan?` null → `null` |
| `Vector2/3/4` | `[x, y(, z, w)]`; `Quaternion` `[x, y, z, w]` |
| blueprint (any `SimpleBlueprint`), `BlueprintReference<T>` | blueprint GUID string, 32 lowercase hex digits, no dashes; null or empty → `null` |
| unit (`UnitEntityData`, `UnitReference`, `EntityRef<UnitEntityData>`) | `{"$unit": "<UniqueId>"}`; null → `null` |
| other entity (`EntityDataBase` subclasses, `EntityRef<T>`) | `{"$entity": "<UniqueId>", "class": "<C# type name>"}` |
| fact (`EntityFact` subclasses) | `{"$fact": "<UniqueId>", "blueprint": "<guid>"}` |
| item (`ItemEntity` subclasses) | `{"$item": "<UniqueId>", "blueprint": "<guid>"}` |
| stat (`ModifiableValue` referenced from somewhere else) | `{"$stat": <StatType integer>, "owner": "<unit UniqueId>"}` |
| list, array, `HashSet<T>` | JSON array in the container's enumeration order (the order is the game's; a `HashSet`'s order is **not** meaningful and comparers must treat it as a set) |
| dictionary | array of `[key, value]` pairs in enumeration order (also a set for comparison) |
| Unity object | `{"$unity": "<C# type name>", "name": "<object name>"}` |
| anything else with persisted members | a nested object of its persisted members (below) |
| a value that could not be read | `{"$failed": "<exception type>: <message>"}` (counted in `health.value_failures`) |
| an object already being written further up (a cycle) | `{"$cycle": "<C# type name>"}` |
| nesting deeper than 8 levels | `{"$depth": "<C# type name>"}` (counted in `health.truncated`) |

**Persisted members** (`persisted` objects): the members the game's own save serializer
writes for that runtime type, in the order it writes them, under their JSON names. WrathOracle
asks the game's `OptInContractResolver` (the resolver in `DefaultJsonSettings`) for the
type's `JsonObjectContract` and reads each non-ignored property through the contract's value
provider, which is exactly what saving does. So a `persisted` object has the same keys as the
corresponding object in a `.zks` document (minus `$id`/`$ref`/`$type`), with the value
conventions above instead of the save's converters. Members whose `ShouldSerialize` returns
false or whose `DefaultValueHandling` would omit them are **still written** (the dump is
complete; the save may omit them). Collections of entities/facts/items are written as
references, never inlined, except at their one canonical position in this document.

## `identity`

```
{"oracle_version": "0.2.0", "game_version": "...", "game_revision": "...",
 "assembly_csharp_sha256": "<hex>", "blueprints_pack_sha256": "<hex>",
 "save_file": "<file name>", "save_sha256": "<hex>",
 "mods": [{"id": "...", "version": "...", "active": true}],
 "difficulty": {"<settings key>": "<value as string>", ...}}
```

Hashes are computed once per run. The full identity (mod DLL hashes, graphics settings…) is in
the run's `identity.json`.

## `game`

| Key | Contents |
|---|---|
| `game_time`, `real_time` | `Player.GameTime`, `Player.RealTime` (ticks) |
| `mode` | `Game.CurrentMode` as an integer, plus `mode_name` |
| `area`, `area_part` | blueprint GUIDs of the loaded area and area part |
| `main_character` | unit reference |
| `party` | `Player.Party` as unit references, in order |
| `party_and_pets`, `all_characters` | the same for `PartyAndPets`, `AllCharacters` |
| `money` | `Player.Money` |
| `shared_inventory` | `Player.Inventory`: items (as in a unit's `inventory`) |
| `turn_based` | `true` if turn-based combat is active (`CombatController.IsInTurnBasedCombat` predicate, without side effects) |

## `units[]`: the character sheet

Every `UnitEntityData` in `Game.Instance.State` that belongs to the loaded area or the
cross-scene state (party, companions, pets), dead or alive, in game or not; each once.

| Key | Contents |
|---|---|
| `id` | `UniqueId` |
| `blueprint`, `blueprint_name` | GUID and the blueprint's asset name (`name`; not localized) |
| `original_blueprint` | `Descriptor.OriginalBlueprint` GUID |
| `class` | runtime C# type name |
| `flags` | `{"in_game", "is_in_state", "is_revealed", "is_main_character", "is_pet", "in_party", "is_player_faction", "is_dead", "is_conscious", "is_turned_on", "is_extra"}` (properties of the same names on the unit; `in_party` = in `Player.Party`) |
| `master` | the pet's master as a unit reference, or `null` |
| `faction` | faction blueprint GUID |
| `position`, `orientation` | `Position` (Vector3) and `Orientation` (degrees, float) |
| `size`, `original_size` | `State.Size` and `OriginalSize` (enum integers) |
| `hp` | `{"max": MaxHP, "damage": Damage, "nonlethal": DamageNonLethal, "temporary": TemporaryHP, "left": HPLeft}` |
| `conditions` | `[[UnitCondition integer, name, count], …]` for every condition with a non-zero count in `UnitState`'s condition counters, ascending by integer |
| `condition_immunities` | the same for the immunity counters |
| `alignment` | `Descriptor.Alignment` persisted members, plus `value` (`ValueRaw`, integer) and `value_name` |
| `progression` | `Progression` persisted members, plus `character_level` (`CharacterLevel`) and `mythic_level` (`MythicLevel`) |
| `stats` | every stat (below), ascending by `type` |
| `facts` | every fact in `Facts.List`, **in that list's order** (it is the activation order) (below) |
| `resources` | ability resources (below), ascending by blueprint GUID |
| `spellbooks` | spellbooks (below), ascending by blueprint GUID |
| `body` | equipment slots (below) |
| `inventory` | the unit's own `Inventory` items, if it has one (`HasOwnInventory`), else `null` |
| `state` | `Descriptor.State` persisted members |
| `persisted` | the descriptor's persisted members **except** `Stats`, `Resources`, `Progression`, `State`, `Alignment`, `m_Inventory`, `Body`, `m_Spellbooks`, `Facts`, `Buffs`, `Abilities`, `ActivatableAbilities`, `UniqueId`, `Blueprint`/`m_Blueprint` (written above): e.g. `CustomName`, `UISettings` |
| `entity_persisted` | the `UnitEntityData`'s own persisted members except `Descriptor`, `UniqueId`, position, orientation and facts (e.g. `CachedPerceptionRoll`, `TimeToNextRoundTick`) |

### `stats[]`

One entry per distinct `ModifiableValue` in `Stats.AllStats` (dependent aliases such as
`CheckBluff` appear under their own `type` when they are separate objects).

| Key | Contents |
|---|---|
| `type`, `type_name` | `StatType` integer and name |
| `class` | runtime type (`ModifiableValueAttributeStat`, `ModifiableValueSkill`, `ModifiableValueArmorClass`, …) |
| `base` | `BaseValue` |
| `permanent` | `PermanentValue` |
| `modified` | `ModifiedValue` (the final value) |
| `raw` | `ModifiedValueRaw` |
| `extra` | every other instance field and pure getter of the runtime class and its bases, by C# name, except the modifier list, dependents and owner links (e.g. attributes: `Bonus`, `PermanentBonus`; skills: `BaseStat`/`BaseAttribute` as `$stat` references, `ClassSkillFlag`; AC: `Touch`, `FlatFooted`, `FlatFootedTouch`, the `*Permanent` variants, `BaseAttributeAppliedBonus`; `NoMorale`, `NoMoraleNoInsight`). A getter is "pure" if its IL only reads fields, does arithmetic and calls other pure getters or `System.Math`; impure getters are left out |
| `modifiers` | every modifier (below), grouped by descriptor ascending (integer), and within a descriptor **in the list's order** (the game keeps each list sorted by value; ties keep insertion order) |

Each modifier:

```
{"descriptor": <ModifierDescriptor int>, "descriptor_name": "...", "value": <ModValue>,
 "stack_mode": <StackMode int>, "stacks": <Modifier.Stacks>,
 "source": <fact reference or null>, "source_component": "<component name or null>",
 "item": <item reference or null>}
```

How the game combines them (for the comparer's information; `modified` is authoritative):
`ModifiableValue.ApplyModifiersFiltered`: per descriptor, stacking modifiers sum, otherwise
the largest positive and the smallest negative count; negative `Armor` and `Encumbrance`
penalties are pooled and only the worse of the two counts; attributes return 0 with a
`Helpless` modifier; the result is clamped below by `MinValueRaw`.

**Derived combat values** (attack bonus with a given weapon, CMB, CMD, AC against a given
attacker) are computed by rules (`RuleCalculateAttackBonus`, `RuleCalculateCMB`,
`RuleCalculateCMD`, `RuleCalculateAC`), which run fact handlers. A dump must not trigger
rules, so they are **not** in the dump; their inputs are (`BaseAttackBonus`,
`AdditionalAttackBonus`, `AdditionalCMB`, `AdditionalCMD`, `AC`, attributes, size). They
appear in rule traces (see `docs/poc1/oracle.md`) when the game computes them.

### `facts[]`

| Key | Contents |
|---|---|
| `index` | position in `Facts.List` |
| `id` | `UniqueId` |
| `class` | runtime type (`Feature`, `Buff`, `Ability`, `ActivatableAbility`, `UnitFact`, …) |
| `blueprint`, `blueprint_name` | GUID and asset name |
| `rank` | `GetRank()` |
| `active` | `IsActive` |
| `source_fact`, `source_item`, `source_ability` | as references / GUID, or `null` |
| `caster` | `MaybeContext?.MaybeCaster` as a unit reference, or `null`; if reading the context throws (an inactive item enchantment), `null` plus `caster_unavailable` with the message |
| `buff` | for buffs only, else absent: `{"start_time": AttachTime, "end_time": EndTime or null (permanent), "next_tick_time": NextTickTime, "tick_time": TickTime, "round_number": RoundNumber, "is_permanent": IsPermanent, "is_suppressed": IsSuppressed, "is_from_spell": IsFromSpell}` (times are absolute game time in ticks; they are not rebased on load) |
| `components` | `[{"name": <blueprint component name>, "class": <component runtime type>, "data": <persisted members of the runtime's data, or null>}]` in the fact's component order |
| `persisted` | the fact's persisted members except those written above (for buffs, also the timing members in `buff`; for other facts, e.g. item enchantments, timing stays here) |

### `resources[]`

`{"blueprint": guid, "amount": <current>, "persisted": {...}}`. The maximum isn't dumped:
`BlueprintAbilityResource.GetMaxAmount` raises `IResourceAmountBonusHandler` events, which a
dump must not do; it is derived from the stats, class levels and facts that are dumped.

### `spellbooks[]`

`{"blueprint": guid, "caster_level": CasterLevel, "persisted": {...}}` (the persisted members
hold known, memorized and custom spells and spontaneous slots).

### `body`

`{"current_hand_set": <index>, "hand_sets": [{"primary": slot, "secondary": slot}, …],
 "slots": {"<slot field name>": slot, …}, "empty_hand_weapon": <item or null>,
 "empty_hand_weapons_stack": [item, …], "additional_limbs": [slot, …],
 "polymorph_hand_set": {"primary": slot, "secondary": slot} or null,
 "polymorph_additional_limbs": [slot, …] or null, "persisted": {...}}` (the weapons outside the
regular slots take part in attacks, so they are full item objects; `persisted`: the body's
other persisted members), where a slot is
`{"item": <item object or null>, "active": bool}` and the slot field names are the
`UnitBody` members (`Armor`, `Shirt`, `Belt`, `Head`, `Glasses`, `Feet`, `Gloves`, `Neck`,
`Ring1`, `Ring2`, `Wrist`, `Shoulders`, quick slots as `QuickSlot0`…). An **item object** is:

```
{"id": UniqueId, "blueprint": guid, "blueprint_name": "...", "class": "<runtime type>",
 "count": Count, "is_identified": IsIdentified, "charges": Charges,
 "enchantments": [<fact entry as in facts[]>...],
 "persisted": {...the item's other persisted members...}}
```

Inventories (`inventory`, `shared_inventory`) are arrays of item objects in the collection's
order.

## `narrative`

| Key | Contents |
|---|---|
| `etudes` | `[{"blueprint": guid, "state": <int>, "state_name": "..."}]` from `EtudesSystem.m_EtudesData` (the private `EtudeState` enum: 0 Unknown, 1 Started, 2 Completed, 3 PreStarted, 4 PreCompleted), ascending by GUID |
| `etude_facts` | the `EtudesSystem`'s own facts (one `Etude` fact per playing etude), as `facts[]` entries without `index`, ascending by blueprint GUID. A `Started` status alone does not mean the etude is playing; check both |
| `flags` | `[{"blueprint": guid, "value": int}]` from `UnlockableFlags.UnlockedFlags` (every unlocked flag, zero values included), ascending by GUID |
| `quests` | `[{"blueprint": guid, "state": <QuestState int>, "objectives": [{"blueprint": guid, "state": <QuestObjectiveState int>, "persisted": {...}}], "persisted": {...}}]`, quests ascending by GUID, objectives in the quest's order |
| `dialog` | `Player.Dialog`: `SelectedAnswers`, `ShownAnswerLists`, `ShownCues`, `ShownDialogs` as GUID arrays **sorted ascending** (they are sets); `AnswerChecks` as `[[answer guid, <CheckResult persisted>], …]` sorted by GUID; `Scheduled` (persisted members or `null`) |
| `alignment` | the main character's alignment (as in its unit entry) |
| `player` | `{"chapter": Player.Chapter, "game_id": Player.GameId, "money": Player.Money}` |

## `health`

```
{"records_failed": n, "value_failures": n, "truncated": n, "cycles": n,
 "random_state_unchanged": true}
```

`records_failed` counts whole records (a unit, `game`, `narrative`) that threw and were replaced
by `{"$failed": "..."}`; `value_failures` counts single values written as `$failed`. A dump with
`records_failed > 0` or `random_state_unchanged == false` is not a valid comparison target;
`value_failures`, `truncated` and `cycles` say how much detail is missing.

## Example dumps (local, gitignored)

- `fixtures/captures/oracle/state-manual13-after-load.json` (2.6 MB, 21 units)
- `fixtures/captures/oracle/state-manual10-after-load.json.gz` (37.6 MB gzipped, 129 MB of
  JSON, 539 units)
- per fixture, `fixtures/captures/oracle/cert-7/fixtures/<name>/state-{start,end}-<run>.json`

`oracle/tools/oracle-state-diff A B` compares two dumps as described below.

## Comparing

- Units by `id`; stats by `type`; facts by `id` (and separately check the order); modifiers
  as a multiset per stat unless the comparison wants order; resources and spellbooks by
  blueprint; narrative arrays by blueprint.
- Floats by bits after parsing to their declared width (C# `float` fields are written from
  `float`, so parse them as `f32`).
- `persisted` objects key by key; a key present on one side only is a difference.
- Entity ids that the game creates during a run (items, facts and units made after the save
  was written) are fresh `System.Guid` strings (36 characters with dashes), different in every
  run. Comparing two runs, rename them in order of first appearance (`oracle-state-diff` does);
  ids that come from the save (short hex like `"3875"`, or the GUID-form ids already in the save)
  are stable.

## Changes

- v1 (2026-10-11): first version. Revised the same day after review, before any consumer
  used it: `resources[].max` removed (computing it raises events); `entity_persisted` and
  `body.persisted` added; unit references inside `EntityRef<UnitEntityData>` written as `$unit`
  and other typed references with `class`, as the conventions table always said.
- v1, additive (2026-10-11, after Astra round 3): `body` gains `empty_hand_weapon`,
  `empty_hand_weapons_stack`, `additional_limbs`, `polymorph_hand_set` and
  `polymorph_additional_limbs` as full items (before, these weapons were only references inside
  `body.persisted`). `health.records_failed` now counts whole failed records only; before, it
  subtracted value failures, which could cancel a real failure. End-of-window dumps keep the
  source save's identity (before, it was null, which weakened the id policy).
