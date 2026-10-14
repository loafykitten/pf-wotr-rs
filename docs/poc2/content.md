# POC-2 content: schemas, decoding, references, pack

Written 2026-10-10. Scope: PLAN.md's POC-2, first round (everything except the live
comparison, which needs the oracle mod). Inputs were read in place from Ember's install;
reports and the pack live under the gitignored `cache/content/`. Tags: **[M]** measured on
this Mac (M2 Max), **[I]** inferred from the decompiled code or the data, not yet checked
against the running game. No game text appears here: only type names, GUIDs and counts.

## Headline

- **Readability [M]:** all **236,661** `.jbp` files parse and are well formed (an
  `AssetId` GUID and a `Data` object); no duplicate GUIDs; 568.8 MB decoded in 1.4 s.
- **Typed coverage [M]: 100%.** Every blueprint decodes through generated schemas with
  **zero opaque values**: 2,502,938 objects, 334 root types, 2,151 nested `$type` types
  (2,488 of the 2,913 registered TypeIds occur). The opaque fallback exists and is tested,
  but nothing in the shipped data needs it. Not dropped, and not counted as typed:
  1,283,442 JSON properties the runtime types don't have (editor-only fields, kept on
  each object and counted per type), and 371,026 fields missing from the JSON (they take
  their C# initializers, all statable except 8).
- **References [M]:** 889,268 typed blueprint references; **886,352 resolve** (99.67%),
  **2,671 dangle**, **245 have the wrong type** (231 of them
  `BlueprintParametrizedFeature` ↔ `BlueprintFeatureSelection`). The game reads both
  kinds of failure as null.
- **The runtime pack, decoded [M].** A `.bbp` reader built on the same schemas parses all
  236,661 records of `blueprints-pack.bbp`, each consuming exactly its bytes. Same GUID
  set, same names. In canonical form **233,886 blueprints (98.83%) are identical** to
  the zip's; the 2,775 that differ are explained below. **The zip is not quite what the
  game loads.**
- **Content pack [M]:** built in 1.3–1.4 s, 494 MB. **Ready in 25 µs warm** (p50; p99
  68 µs), 1.2–1.5 ms cold. GUID lookup p50 125 ns / p99 500 ns; with payload validation
  p50 0.9 µs / p99 5.7 µs.

## How it works

**`tools/wr-codegen`** parses the decompiled C# with tree-sitter (`tree-sitter-c-sharp`)
into declarations (types, bases, attributes, fields with initializers, parameterless
constructor assignments, enum members, lifecycle methods), resolves names the way C# does
(generic parameters, nested types including inherited ones, namespace chain, usings and
aliases, arity), and applies the game's serialization rules to every type reachable
from a `[TypeId]` type:

- `FieldsContractResolver.GetUnitySerializedFields`: instance fields, base class first,
  public or `[SerializeField]`/`[SerializeReference]`, not `[NonSerialized]`, and
  `IsSerializableType` (primitives, enums, strings, Unity objects, a fixed list of Unity
  value types, one level of `T[]`/`List<T>`, `[Serializable]` types outside mscorlib).
  So `Dictionary`, `Nullable<T>`, `Quaternion`, interfaces and nested lists never
  serialize.
- Newtonsoft on top: `[JsonProperty]` names, `[JsonIgnore]`, derived fields hiding base
  fields of the same JSON name, Unity's `name` property for `UnityEngine.Object`
  subclasses, and the converter picked by `Json`'s registration order (references,
  assets, weak links, shared strings, enums, vectors, curves, gradients, prototype
  links; the `Color` and `Rect` converters can't read, so those go through the field
  contract). With `MemberSerialization.Fields`, readonly fields are written too.
- The TypeId registry is the instantiable part of what
  `StartGameLoader.PrepareTypeCache` registers: non-generic `[TypeId]` types in
  Assembly-CSharp (`Assembly.GetExecutingAssembly()`). The game also registers 7 open
  generic definitions, which a `$type` can't instantiate; we treat them as unknown, so
  both fail [I]. The GUID before the comma in `$type` is authoritative
  (`GuidClassBinder`).
- C# constants are evaluated with C# typing (literal types, promotion, masked shift
  counts, wrapping casts) for enum members and defaults; a parameterless constructor's
  `Field = …` assignments override initializers.
- The `.bbp` field set: the same fields minus `[ExcludeFieldFromBuild]`, under C# names.

It reads 27 assemblies (17,910 declarations, 1.2 s). Besides Assembly-CSharp and
Owlcat's, that's 17 Unity and helper assemblies decompiled this round into
`decompiled/` (UnityEngine.CoreModule, UI, TextMeshPro, Timeline…) for base
classes and `[Serializable]` flags. 12 files contain syntax tree-sitter can't parse
cleanly (unsafe pointer code in method bodies, mostly UI and Unity internals); the only
closure type among them is `UnityEngine.Object`, whose fields aren't affected (the list
is in `codegen.json`). The closure is **4,202 types and 401 enums**, with no unresolved
field type (one mscorlib enum, `DayOfWeek`, is built in). The output,
`crates/wr-content/src/generated/schema.rs` (4.2 MB: types, fields, enums and TypeIds,
no game data), is static tables rather than one Rust struct per type: 4,202
serde-derived structs would cost minutes of build time per change, and a table-driven
decoder keeps unknown fields, defaults and statistics uniform. A clean build of
wr-content takes 5.3 s. Typed views for the types wr-sim ports first can be generated
from the same model later. Handwritten code: about 8,600 lines across both crates.

**`crates/wr-content`:**

- `source`: reads `blueprints.zip` into memory once and inflates entries in parallel
  from cheap clones of the central directory.
- `decode`: a serde `DeserializeSeed` driven by each field's codec decodes the token
  stream straight into `Node`s, following Newtonsoft: a leading run of metadata
  (`$id`, `$type`…) with the tagged type checked for assignability, exact-then-
  case-insensitive property matching, `ReadAsInt32`'s refusal of float tokens and
  acceptance of numeric strings, `Enum.ToObject`'s wrapping and `Enum.Parse`'s
  overflow-to-default, `"x|y|z"` vector shorthand (also inside `Bounds`), floats from
  `"Infinity"`/`"NaN"`/numeric strings. Anything that doesn't fit becomes
  `Node::Opaque` with the raw JSON and a reason. Unknown properties go into
  `Object::extra`; fields missing from the JSON are `Node::Absent`; values a converter
  changes (an unknown enum name read as 0, an incomplete asset reference read as null)
  are `Node::Coerced`, holding both the game's value and the JSON; values a converter
  discards (prototype links) are `Discarded`. Converter objects with keys the converter
  would ignore become opaque rather than lose them. (Values kept as JSON don't keep key
  order inside them.)
- `refs`: GUID index and typed resolution (exists, and the target is a `T`).
- `bbp` and `packfmt`: the `.bbp` reader and the table of how `ReflectionBasedSerializer`
  encodes each codec.
- `canon`: the canonical form (spec below). `compare`: zip versus `.bbp`.
- `pack`: the content pack. `report`: the reports.

Commands (`cargo run --release -p wr-content -- <cmd>`): `report`, `compare-bbp`,
`pack-build`, `pack-bench`, `canon <guid> [--bbp]`, `find-opaque`; for the schema,
`cargo run -p wr-codegen -- gen`.

## Reports (each in `cache/content/`)

| Report | File | Result |
|---|---|---|
| JSON readability | `readability.json` | 236,661 / 236,661 parsed and well formed [M] |
| Typed coverage | `coverage.json` | 236,661 / 236,661 blueprints, 2,502,938 / 2,502,938 objects with no opaque value; per root type and per nested type, with field counts typed / opaque / absent / unknown [M] |
| References | `references.json` | 886,352 resolved, 2,671 dangling, 245 wrong type, per target type [M] |
| Initialisation | `init.json` | below [M for counts, I for what the hooks do] |
| Behaviour support | column in `coverage.json` | 0 for every type (out of scope this round) |
| Zip vs `.bbp` | `zip_vs_bbp.json` | 233,886 / 236,661 identical [M] |
| Schema | `codegen.json` | closure, hooks per type, runtime-only fields, parse problems |

Unknown properties are the editor's: `LocalizedString`'s `m_JsonPath`,
`m_OwnerPropertyPath` and `m_OwnerString` (963,051), the wrapper's `Meta` (99,698),
dialog `ParentAsset` (70,000+), component `m_Overrides` and a few stale fields
(`BlueprintFeature.DlcType`). None is in the runtime types, so the game never reads
them. Missing fields are fields added after a file was last saved (`ContextValue.
PropertyName` 34,027, `SelectionEntry.m_Spellbook` 8,137…); the game gets their
initializers, which we carry as literals, constructor assignments, `new Vector2(…)`-style
constants or `new T()`. The 8 we can't state are `ArmorClassBonusAgainstAlignment.Bonus`,
initialised through `ContextValue`'s implicit conversion from `int`; the canonical form
flags them. Decoded the way the game would, and counted (`coverage.json` notes): 3,171
weak links written as asset references (`{guid, fileid}`; the converter takes `guid`,
we keep both), 8,616 values of 8 fields whose initializer is a non-empty object that
Newtonsoft would fill rather than replace (not modelled; the pack comparison shows no
difference in them), one unknown enum name, four numbers in string fields, three
properties matching only by case, one reference without `!bp_`.

**Initialisation needs [M counts, I semantics].** 266 types in use have an `OnEnable`
body, which `BlueprintsCache.Load` calls after reading each blueprint: 239 only through
`BlueprintScriptableObject.OnEnable` (sets each component's `OwnerBlueprint` and each
element's `Owner`, then `OnEnableWithLibrary`), 20 through `BlueprintItem`, and
`BlueprintAreaPreset`, `BlueprintLoot`, `BlueprintAiCastSpell`,
`BlueprintClockworkScenario`, `BlueprintRoot`, `BlueprintCorruptionRoot` with their own.
170,705 blueprints are of those types. `OnEnableWithLibrary` has a body on 3 types. 449
types in use have non-serialized instance fields (caches and state that code computes
lazily or on enable). `SimpleBlueprint.m_AllElements` is built by the pack reader
(`HandleBlueprintBacklinks`), not stored. `[OnDeserialized]` hooks (2,156 types, mostly
`BlueprintComponent`'s owner link) run on the JSON path only; the pack path doesn't call
them.

## Zip versus the runtime pack

The `.bbp` layout (`BlueprintsCache`, `ReflectionBasedSerializer`, `PrimitiveSerializer`)
[M, by decoding all of it]: `i32` count, then `count × (16-byte GUID, u32 offset)`; each
record is a type GUID, then every runtime field as its C# name (a `BinaryWriter` string)
and its value, then, for a non-null blueprint, its `name` and GUID. GUIDs are in .NET
byte order. Identified types (blueprints, components, elements) carry a type GUID and
can be null; other objects can't be null (the writer creates an instance for null),
strings are `""` for null, arrays are empty for null. Unity objects are indices into
`BlueprintReferencedAssets` (in `blueprint.assets`). Types that miss every `ReadField`
case but are `[Serializable]` with no fields (`byte`, `short`, `List<float>`
elements…) lose their values; no field in this game's schema hits that case (tested).

The comparison writes the zip side in canonical form with the build-time rewrites
below applied, the `.bbp` side literally, and compares Unity assets by presence (null or
not) until the asset list is decoded. 2,775 blueprints differ [M], by field (a blueprint
can have several):

| Difference | Blueprints | Cause |
|---|---|---|
| `CommandUnitPlayCutsceneAnimation.m_ClipIsLooped` | 2,352 | A `[HideInInspector]` field nothing at run time assigns; absent from the JSON (so false), set in the pack, presumably computed by the editor from the clip [I] |
| `LocalizedString.m_Key` | 379 | The pack's key differs from the JSON's (empty on one side, or another key); likely the JSON's `Shared.stringkey` is a stale cache of the shared-string asset's key [I] |
| Asset present on one side only | 36 | Assets missing from the pack's referenced-asset list read as null at run time [I] |
| Unknown initializer | 8 | `ContextValue Bonus = 0` (implicit conversion), flagged by the canonical form |
| Other | 3 | QA (`BlueprintArbiterInstruction.m_ElementTypeGuid`) and `ScenesRoot` fields |

Build-time rewrites the pack shows, applied to the zip side of the canonical form: a
`LocalizedString` with a shared string holds that string's key in `m_Key` and no
`Shared` [M: with the rewrite, no `Shared` differs anywhere]; a `ColorBlock` has no
selected colour, and curves have no wrap modes [M from `PrimitiveSerializer`, which
doesn't write them]. An earlier draft also claimed that keys missing from `enGB.json`
are cleared. The review showed that applying the rule to both sides masked differences;
measured on the zip side only, it makes things worse (233,886 identical without it,
231,229 with it; the reviewer counted 3,142 such keys kept in the pack), so it's gone.

**Recommendation for Decision 2:** since the pack decodes completely through the same
schemas and is what the game runs on, build our content pack **from the `.bbp`**, with
the zip as the source of what the pack lacks (asset GUIDs until `blueprint.assets` is
decoded with wr-import's Unity reader, authoring metadata, editor fields). That removes
the 2,775 differences by construction; the oracle comparison then checks our `.bbp`
reading against the live objects. What remains for a production `.bbp` path: the asset
list (1–2 days with wr-import), and building `pack.rs` from `bbp` nodes instead of zip
nodes (the canonical form already takes the source).

## Canonical form (spec)

One JSON document per blueprint, describing the object **as the running game holds it**
(loaded from the pack), so the game side can emit it from its loaded objects with no
JSON involved:

1. Top level: `"guid"` (32 lowercase hex), `"name"` (the blueprint's `name`), then the
   blueprint object's body.
2. Object body: if the object's type is identified (`SimpleBlueprint`,
   `BlueprintComponent` or `Element` subclass), first `"$type"`: its `[TypeId]` GUID.
   Then every field of `BlueprintFieldsTraverser.GetUnitySerializedFields(type)`, in that
   order (base class first, declaration order), keyed by C# field name. Nothing else:
   no `[JsonIgnore]`, `[ExcludeFieldFromBuild]` or editor-only fields.
3. Values by field type:
   - bool → `true`/`false`; integers, `char` and enums → JSON integers (enums by value);
   - `float`/`double` → a JSON number that round-trips the value (we write the shortest),
     `"NaN"`, `"Infinity"`, `"-Infinity"`. **Compare by parsing to the field's type and
     comparing bits** (any NaN equals any NaN), so the emitter's float formatting doesn't
     matter as long as it round-trips (`"R"` in Mono, or `BitConverter` if in doubt);
   - string → JSON string, null written as `""`;
   - `T[]` and `List<T>` → arrays, null written as `[]`;
   - identified object → body as in 2, or `null`; other `[Serializable]` object → body,
     never null (a null is written as a default-constructed instance);
   - `BlueprintReference<T>` (and `BlueprintReferenceBase`) → the GUID string, or `null`
     for an empty GUID;
   - Unity asset → `{"guid": "<asset guid>", "fileid": n}` via
     `UnityObjectConverter.AssetList.GetAssetId(obj)`, or `null`; `SharedStringAsset` →
     `{"stringkey": obj.String.Key}` or `null`;
   - weak resource link → its `AssetId`, `""` when null or empty;
   - Vector2/3/4 → `[x, y(, z, w)]`; Color → `[r, g, b, a]`; Color32 → `[r, g, b, a]`
     bytes; Vector2Int → `[x, y]`; Bounds → `[cx, cy, cz, sx, sy, sz]`;
   - AnimationCurve → `{"keys": [[time, value, inTangent, outTangent, inWeight,
     outWeight, weightedMode], …]}` (wrap modes left out); Gradient →
     `{"colors": [[t, r, g, b], …], "alphas": [[t, a], …], "mode": n}`, keys sorted by
     time.
4. Something the producer can't state exactly is written as `{"$unknown": "<why>"}`,
   which never compares equal.

Our producer (`canon::blueprint_with`) derives this from the zip by applying the
build-time rewrites above and C# initializers for missing fields; from the `.bbp` it's
direct (`Source::Pack`). The oracle emitter for `WrathOracle` is the same recursion as
`ReflectionBasedSerializer.WriteField` over the live object, with the value mapping in 3.
A diff tool should compare documents field by field with numbers parsed by type; text
equality (what `compare-bbp` uses) is a stricter special case.

## Content pack

`cache/content/blueprints.wrcpack`: a header (magic, format version, rkyv node size,
schema fingerprint generated with the schema, source zip size and modification time,
section offsets), a GUID-sorted index of 40-byte entries, a by-type index, zip paths,
and one rkyv archive per blueprint (16-byte aligned). Opening maps the file and checks
the header and section bounds; offsets use checked arithmetic, and payloads are
validated by rkyv's checker when accessed, so a corrupt pack gives errors, not undefined
behaviour. `Pack::open` is `unsafe`: its contract is that the file isn't modified while
open, which `build` guarantees for its own output (a uniquely named file, marked
read-only, renamed into place). Editor-only properties are left out by default
(`--keep-extra` keeps them).

| Measure [M] | Value |
|---|---|
| Build (from the zip) | 1.3–1.4 s: decode and encode 1.15 s (parallel), write 0.16 s; peak RSS 1.2 GB |
| Size | 494 MB (payload 466 MB); 545 MB with editor-only properties |
| Warm open (map + validate header) | p50 13 µs, p99 45 µs (200 runs) |
| **Warm ready** (open + stat the zip for staleness + one lookup with payload access) | **p50 25 µs, p99 68 µs** |
| Cold open / ready / first 1,000 random lookups | 0.24–0.30 ms / 1.2–1.5 ms / 201–237 ms (3 runs) |
| GUID lookup, index only | p50 125 ns, p99 500 ns (all 236,661, shuffled) |
| GUID lookup + payload validation (zero-copy access) | p50 0.88 µs, p99 5.7 µs |
| GUID lookup + full deserialize | p50 1.6 µs, p99 12.5 µs (20,000) |

Measured at 21:40, before the oracle agent started a game instance (22:32); an earlier
run of the previous pack version gave the same figures within noise. Cold method: `purge`
needs root, so each cold run opens a fresh copy written with `F_NOCACHE` on both file
descriptors; `mincore` confirmed 0% of its pages resident before opening. The timer's own
overhead rounds to 0 ns on this Mac (`Instant` p50 of an empty interval). The pack is
nearly as big as the JSON because every rkyv node is a fixed-size enum; a per-type layout
generated from the schema would shrink it a lot, but nothing in POC-2 needs that yet.

## Tests

`scripts/check.sh` passes (fmt, clippy `-D warnings`, nextest). Unit tests cover the
codegen rules on synthetic C# (field selection, JSON names, hiding, defaults,
constructor assignments, C# constant typing, enums, references, hooks), polymorphic
decoding (metadata prefix, unknown and non-assignable tags, abstract types), references,
Unity value types (shorthand inside `Bounds`, `"Infinity"` in curves, field-contract
`Color`, partial `Vector2Int`), Newtonsoft's scalar rules and the opaque fallback, enums
(names, flags, wrapping, overflow), the canonical form (defaults, nulls, shared-key
rewrite, field order), the pack (build, open, find, by type, round trip, rejecting a
foreign schema), the `.bbp` record format, and that no schema field is pack-lossy.
`full_zip_decodes_through_schemas` (ignored by default; `cargo nextest run -p wr-content
--run-ignored only`) decodes all of Ember's zip and checks the totals.

## Review

GPT 6 Astra (high) reviewed both crates and this document against the decompiled
serializers (17 findings). Verified and fixed:

- **The string-table key rule was wrong** (above): applied to both sides it masked
  differences. The canonical form now rewrites only the JSON side, and the rule is gone.
- **mmap soundness:** `Pack::open` is now `unsafe` with a stated contract; builds use
  unique temporary names and leave packs read-only; the format version was bumped and
  the rkyv node size is checked, so an old pack is refused; payload and path offsets use
  checked arithmetic within their sections.
- **Silent loss:** converter objects with ignored keys, incomplete asset references,
  partial `Vector2Int`s, non-array curve keys and non-string `assetguid`s are now kept
  (opaque or `Coerced`) instead of disappearing; gradient mode is carried.
- **Newtonsoft fidelity:** metadata prefix (`$id` before `$type`), consumed `$type`
  kept on failure, float tokens refused for ints and enums, numeric strings accepted,
  enum wrapping and overflow, `Color`/`Vector4` through the field contract, `Color32`
  conversions, readonly fields writable (checked against Newtonsoft 12.0.3's source).
- **Codegen:** constructor assignments as defaults (three booleans in two types
  changed), C# constant typing (no enum value changed in this game's code), registry
  restricted to Assembly-CSharp, subtype constructors and leftover generic parameters
  reported, populating initializers flagged.
- **`.bbp`:** a null record no longer reads a name and GUID.

Left as documented limits: Newtonsoft's filling of initializer objects and of repeated
properties isn't modelled (counted; none affects the shipped data, per the pack
comparison), and nested types inside generic outer types don't take the outer type's
arguments (reported if it ever leaves a field unresolved; it doesn't today).

## What's missing for POC-2's pass

1. **Zero mismatches against the live game** [not started]: needs `WrathOracle` to dump
   loaded blueprints in the canonical form above, for the slice's dependency closure and
   a stratified corpus (every root and nested type, every codec). Expect the 2,775
   zip-side differences unless content comes from the `.bbp`; the `.bbp` side should
   match except for anything `OnEnable` changes (dump objects as `BlueprintsCache.Load`
   returns them, after `OnEnable`, and check whether the eight `OnEnable`
   implementations listed above touch serialized fields).
2. **Asset identities** from the pack: decode `BlueprintReferencedAssets` so asset
   indices become `{guid, fileid}`.
3. Per-type typed coverage is reported; the "behaviour" column is zero by design.
4. Under 100 ms warm: met (25 µs).

Smaller follow-ups: resolve `PrototypeLink` GUIDs (authoring metadata) as a separate
report; generate typed Rust views over the pack for the types wr-sim ports first; decide
whether to drop the zip path entirely once the `.bbp` path has assets.

## Effort

About one working session of agent time for both crates, the `.bbp` reader, this report
and the review fixes. The schema needed no hand-written per-type code. The expensive
parts were the converter and pack-time semantics found by comparing with the `.bbp` and
in review, which the live comparison will extend rather than replace.
