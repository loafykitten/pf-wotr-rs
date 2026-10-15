# POC-0 extraction spike: Prologue_Caves_1

Written 2026-10-10. Scope: PLAN.md's POC-0 extraction bullet. Everything was read from
Ember's Steam install in place; outputs live under the gitignored `cache/`. Tags: **[M]**
measured on this Mac, **[I]** inferred from code or data and not yet checked against the
running game. **Updated 2026-10-11** with the live comparison (§9): WrathOracle 0.1's
dumps diffed field by field against wr-import (`wr-import diff`), and an A* port that
matches the game's searches exactly.

## Headline

- **Live comparison (§9) [M]:** every scene entity, spawner, zone, enter point and
  transition the game has loaded from these scenes is in our inventory with the same ID,
  class, path and blueprint; 1,447 of 1,447 comparable serialized fields and 409 of 409
  blueprint references are equal; all 646 zone probes agree. Transforms of the 157 entity
  views are bit-identical; across all 5,204 GameObjects 65% of world transforms are
  bit-identical and the rest are within 8 ULP (composition order), except objects the
  game moves or toggles at runtime (listed). **Both navigation graphs are identical to
  the live ones** (373,357 node fields, connections and costs). **Path searches match
  exactly** on all 46 queries × 5 kinds (nodes, step costs, end-node G, searched-node
  count, endpoints, nearest nodes); smoothed corner points match in 26/46 (real-time
  agent), 32/46 and 44/46 (turn-based) queries.

- **Every object decodes [M].** `tools/wr-import` reads UnityFS bundles and type trees
  directly (no AssetRipper needed for gameplay data). All **487,816** objects in the
  area's 200-bundle dependency closure decode with every byte consumed. All **8,245,482**
  objects in all 2,134 `.scenes` bundles of the game decode too (18.5 s).
- **Gameplay inventory [M]:** 5 scenes, 5,204 GameObjects, 180 of them carrying gameplay
  or supporting components: 157 entities with stable `UniqueId`s (67 locators,
  33 spawners, 31 map objects, 17 script zones, 9 unit groups), 33 spawners with their
  unit blueprints, 17 zones with 17 world-space shapes, 6 enter points, 1 area
  transition, 29 interactions, 17 fact holders, 10 perception checks. No duplicate IDs.
- **References resolve 100% [M]:** 32,404 scene-object pointers, 2,718 asset pointers
  across 39 bundles (37 into Unity's built-in resources), 118 blueprint GUIDs (all in
  `blueprints.zip`). No entity or resource-link references occur in these scenes.
- **AssetRipper 2.0.0 exports the closure faithfully where we can check [M]:**
  9,457 of 9,460 meshes byte-identical (index and vertex data), all 5,204 scene
  Transforms bit-identical to our reader, all GameObject names equal. It omits Unity's
  built-in shaders by design and drops a few same-named duplicates.
- **Navigation decoded [M]:** A* Pathfinding Project 4.3.10 cache with a tiled
  RecastGraph (1,061 triangles in 728 tiles) and a 280×260 GridGraph (14,040 walkable
  cells); no Owlcat-specific data in the cache. Basic A*+funnel and grid queries run.
  Both graphs split into the same three components.
- **Missing, top items:** Owlcat's shaders (7 families in the area, all custom), crunched
  lightmaps (4 textures need Unity's crunch decoder), 24 gameplay component classes to
  port for this area (72 game-wide), the last bits of path smoothing (funnel unwrap and
  graph raycasts, §9), navmesh cuts, and spawn-time resources not covered by the
  dependency list.

## 1. Procedure

### Tools

- **AssetRipper 2.0.0** (`AssetRipper_mac_arm64.tar.xz`, compiled 2026-08-24), unpacked
  into `cache/tools/assetripper/`. Its headless mode is a local web server with a REST API
  (OpenAPI at `/openapi.json`):
  ```sh
  cache/tools/assetripper/AssetRipper.GUI.Free --headless --port 47123 \
      --log-path cache/extract/logs/assetripper.log
  curl -X POST localhost:47123/Settings/Update --data-urlencode BundledAssetsExportMode=GroupByBundleName \
      --data-urlencode ScriptContentLevel=Level1 --data-urlencode ScriptExportMode=DllExportWithoutRenaming \
      --data-urlencode ShaderExportMode=Yaml --data-urlencode TextExportMode=Bytes \
      --data-urlencode ImageExportFormat=Png --data-urlencode LightmapTextureExportFormat=Yaml \
      --data-urlencode SpriteExportMode=Yaml --data-urlencode AudioExportFormat=Native
  curl -X POST localhost:47123/Reset
  curl -X POST localhost:47123/LoadFolder --data-urlencode Path=$PWD/cache/extract/stage_closure
  curl -X POST localhost:47123/Export/UnityProject --data-urlencode Path=$PWD/cache/extract/closure_project
  ```
  Both `LoadFolder` and `Export/UnityProject` block until done (they answer 302). The
  staging folder holds **symlinks** to the bundles, so the install is only read.
- **UnityPy 1.25.4** in `cache/tools/venv` (run with `python -I`), used only to peek at
  type trees while writing the Rust reader.
- **ilspycmd**: `AstarPathfindingProject.dll` decompiled into
  `decompiled/AstarPathfindingProject/` (1.7 s).
- **wr-import** (this spike), release build:
  ```sh
  wr-import import --area prologue_caves_1 \
      --assetripper cache/extract/closure_project --pruned cache/extract/logs/pruned_png.tsv
  wr-import path --from 85,40,105 --to 100,46,104   # one query on both graphs
  wr-import scene-census                           # all 2,134 scene bundles
  ```
  Outputs: `cache/import/prologue_caves_1/{inventory,nav,report,assetripper_check}.json`,
  `cache/import/scene_census.json`, and two cached indexes (`cab_index.json`,
  `blueprint_index.json`).

### Timings and sizes [M]

| Run | Input | Time | Output |
|---|---|---|---|
| AssetRipper, area bundles only | 6 bundles, 44 MB | load <1 s, export 3 s | 198 MB; 84 "dependency not found" warnings (expected) |
| **AssetRipper, full closure** | **200 bundles, 4.27 GiB** | **load 11 s, export 220 s** | **51,760 assets, 0 warnings, ~11.2 GiB** (7.71 GiB of PNGs) |
| AssetRipper, spawned-unit bundles | 18 bundles, 16 MiB | 1 s | 87 MB; 32 warnings (static closure not co-loaded) |
| wr-import CAB index (first run) | 23,565 bundle headers | 1.45 s | 115,851 files indexed, 0 errors |
| wr-import blueprint index (first run) | 236,661 `.jbp` heads | 53 s | cached; later runs 0.1 s |
| wr-import area import, no census | 5 scenes + nav | 3.7 s | inventory 0.97 MB, nav 0.49 MB |
| wr-import full import | + closure census + AssetRipper check | 18.5 s, 9.1 GB peak RSS | report 40 KB |
| wr-import scene census | 2,134 bundles, 25.0 GiB | 18.5 s, 5.8 GB peak RSS | |

Disk was tight (16 to 42 GiB free, shared with the other spikes), so a pruner recorded
and deleted the closure export's PNGs as they were written (14,805 files, 7.71 GiB;
sizes and paths in `cache/extract/logs/pruned_png.tsv`), keeping
`settings_caves_prologue.worldtex`'s 94 as a sample. 3.5 GiB of the export remains. A
production pipeline wouldn't export textures through AssetRipper at all (section 4).

## 2. The dependency closure

**How the game resolves bundles [I, from code]:** `BundlesLoadService.RequestBundle`
loads a bundle, then recursively requests every entry of its `dependencylist.json` list
(`LoadDependencies`), skipping the ten `AssetBundleNames.CommonBundles` (`shaders`,
`techart`, `fx`, `ui`, `icons`, `portraits`, `equipment_prologue`, `weapons.prefabs`,
`blueprint.assets`, `humananimation.animations`), which are loaded at startup together
with their own dependencies. Assets requested by GUID (unit prefabs, FX, etc.) map to
bundles through `locationlist.json` (parallel GUID/bundle arrays). `NavMeshManager`
loads the area part's graph cache through that same path. `wr-import`'s
`catalog::Dependencies::closure` mirrors this.

**Static closure [M]:** the five scene bundles plus `prologue_caves_1.nav` pull in
**200 bundles, 4.27 GiB**:

| Family | Bundles | Size |
|---|---:|---:|
| `.worldtex` | 113 | 2.59 GiB |
| `equipment` | 1 | 651 MiB |
| `.worldart` | 42 | 377 MiB |
| `equipment_prologue` | 1 | 184 MiB |
| `fx` | 1 | 160 MiB |
| `.terrainlayers` | 2 | 80 MiB |
| `extra` | 1 | 77 MiB |
| `.animations` | 9 | 68 MiB |
| `.unit` | 17 | 48 MiB |
| `.scenes` | 5 | 43 MiB |
| `techart`, `shaders`, `.prefabs`, `.fx`, `.nav` | 8 | 39 MiB |

The size comes from the common bundles: `fx` alone depends on 100+ `.worldtex`
bundles. **185 of the 200 are already resident from startup** (the startup closure is
295 bundles, 4.99 GiB); only **15 bundles, 138 MiB, are specific to the area** (the 6
roots, 2 terrain-layer bundles, 4 world-art/texture bundles, one unit and one FX
bundle). Our runtime packs should mirror that split: a shared pack plus a small
per-area pack.

**Dynamic dependencies [M]:** the dependency list doesn't cover units spawned at
runtime. The 33 spawners use 15 unit blueprints whose `Prefab.AssetId`s map to 11 unit
bundles; 10 of them are not in the static closure. With their own dependencies that's
**18 more bundles, 16 MiB** (`cache/extract/closure_dynamic_units.txt`). Not yet
covered **[I]**: other blueprint-driven resources (equipment entities `.ee` for the
spawned units' gear, ability and buff FX, portraits, dialog cameras). A production
closure has to walk blueprints, not just the bundle list.

## 3. wr-import

A new crate in the main workspace (`tools/wr-import`, about 6,100 lines of Rust with tests; passes
`scripts/check.sh`). It reads bundles directly rather than AssetRipper's YAML: type
trees are present in every serialized file, so a direct reader is simpler, keeps the
original identities (`CAB-…` file name plus path ID), and is fast enough to run on every
build.

| Module | What it does |
|---|---|
| `unity::bundle` | UnityFS format 8 (2020.3.34+ flags, 16-byte alignment, LZ4/LZ4HC/LZMA blocks) |
| `unity::serialized` | Serialized file format 22: types with type-tree blobs, objects, externals, `[SerializeReference]` ref types |
| `unity::typetree` | Generic decode into a `Value` tree; alignment rules; Unity's common-string table; managed-reference registry v1/v2; fails if any byte is left over |
| `catalog` | `dependencylist.json`, `locationlist.json`, CAB → bundle index over all bundles |
| `area::Universe` | Opens bundles on demand and resolves `PPtr`s across files and bundles (and Unity's `unity default resources`) |
| `scene` | Hierarchy, world transforms, classification, references, entities, spawners, zones with world shapes and containment probes |
| `refs` | Finds object pointers, blueprint refs (`{guid}`), entity refs (`UniqueId` + `EntityNameInEditor`/`SceneAssetGuid`), resource links (`{AssetId}`), `UnityEvent` method bindings |
| `transform` | Unity's `T·R·S` composition in `f32`, cycle/dangling-parent detection |
| `texture` | Payload-size check per format (BC1/3/4/5/6H/7, raw), stream slice check |
| `nav` | A* graph cache decoder, A*+funnel and grid queries |
| `report`, `archeck` | Yield report, closure and scene censuses, AssetRipper comparison |
| `blueprints` | GUID index over `blueprints.zip` (reads only each file's head) |

**Inventory format** (`inventory.json`, schema `wr-import/area-inventory/0`): per scene
a summary; `entities` (unique ID, class, GameObject key, path, world pose);
`spawners` (unique ID, blueprint GUID, type and zip path, spawn-on-init, pose, settings
components); `zones` (unique ID, blueprint, shapes in `GetComponentsInChildren` order
with local fields, world matrix, `Center()`, `GetBounds()`, and 38 containment probes
each); and `objects`: every GameObject carrying a component of a gameplay or
supporting family (colliders alone don't qualify), with `key` (`CAB-…`, path ID),
hierarchy `path`, `sibling_path`, active flags, layer, local and world transforms
(position, rotation, lossy scale, column-major matrix). Its gameplay and supporting
components carry their decoded fields and resolved references; colliders carry their
fields; other components on those GameObjects are listed by class only.

**Tests:** 32 unit tests on synthetic data (bundle and serialized-file parsing, type-tree
alignment and arrays, the managed-reference registry, transform composition and
evaluation order, reference finding, zone bounds and containment with the game's quirks,
texture size math, graph-cache meta/grid decoding and grid dimensions, `Int3`
cost rounding, funnel, A*, YAML parsing). Real-data
checks are the census and AssetRipper comparisons above, run by `wr-import import`.

## 4. Import yield

### Scenes [M]

| Scene | Objects | GameObjects |
|---|---:|---:|
| Prologue_Caves_1_Static | 10,330 | 3,158 |
| Prologue_Caves_1_Light | 4,557 | 1,199 |
| Prologue_Caves_1_Mechanics | 2,206 | 821 |
| Prologue_Caves_1_TerendelevScales_Mechanics | 81 | 22 |
| Prologue_Caves_1_Audio | 25 | 4 |

Components by relevance tier: gameplay 5,477 (5,204 of them Transforms), supporting
244, presentation 5,931, inert 343. No hierarchy errors.

### References [M]

| Kind | Total | Resolved | Targets |
|---|---:|---:|---|
| Scene object | 32,404 | 32,404 | same scene file |
| Asset | 2,718 | 2,718 | `fx` 1,546, `.worldtex` 839, `.scenes` (shared assets) 176, `.worldart` 86, Unity built-ins 37, `extra` 12, `techart` 9, others 13 |
| Blueprint | 118 | 118 | all exist in `blueprints.zip` |
| Entity, resource link | 0 | | none in these scenes |

Material → shader → texture traversal from every material reference reaches 91 materials,
73 meshes (691,934 vertices, 2 Unity built-in), 170 textures, 976 lights (932 point,
42 spot, 1 directional, 1 area; 895 baked, 81 realtime).

### Assets in the closure [M]

| Class | Our reader | AssetRipper export |
|---|---:|---|
| GameObject / Transform | 177,631 each | 177,631 each |
| MonoBehaviour | 52,954 | 52,954 |
| Mesh | 9,460 | 9,458; **9,457 byte-identical**, 1 vertex data differs, 2 missing (same-named duplicates in `fx`) |
| Texture2D + Cubemap + arrays + 3D | 14,907 | 14,906 (PNG, lightmaps as YAML); 1 missing in `fx` |
| Material | 10,111 | 10,106 (5 fewer: 2 in `fx`, 3 scene/skybox materials) |
| Shader | 138 | 115: 20 Unity built-ins omitted by design (`Hidden/TerrainEngine/*`, `Skybox/Procedural`, `Standard`, `Legacy Shaders/VertexLit`), 3 same-named VFX Graph shaders merged [I] |
| AnimationClip | 1,333 | 1,333 |
| Light, Animator, ParticleSystem, Sprite, colliders… | equal | equal |
| AnimatorController | 192 | 192 (plus reconstructed state machines) |

Scene cross-check: all 5,204 Transforms' local position, rotation and scale are
**bit-identical** between our reader and AssetRipper's YAML, and all GameObject names
match. This independently validates the type-tree reader.

**Textures [M]:** 14,901 of 14,905 textures in the closure have their payload exactly
where and at the size their format implies (inline or a `.resS` slice); formats:
9,416 DXT5/BC3, 4,435 DXT1/BC1, 891 RGB24, 90 BC7, 35 RGBA32, 28 BC6H (reflection
probes), 4 RGBAHalf, 2 ARGB32, **4 DXT5Crunched**. So the shipped BC blocks are
reachable unchanged and can be uploaded as-is. The 4 crunched ones are this area's
lightmaps (`Lightmap-{0,1}_comp_{light,dir}`, 2048²): Unity decodes crunch to DXT5 at
load, so we need Unity's crunch decoder (a port or FFI of its fork of crnlib) to get
bit-identical blocks.

### The missing list

**Unsupported shader families** [M]. Everything is unsupported until the rendering
work; these are what this area needs. "Slots" are non-null entries of renderers'
`m_Materials` arrays (1,400 in all); "other" counts skybox, terrain-template and
script-field references:

| Shader | Slots | Other | Materials |
|---|---:|---:|---:|
| `Owlcat/Lit` | 832 | 53 | 57 |
| `Owlcat/Particles` | 457 | 0 | 19 |
| `Owlcat/Unlit` | 84 | 0 | 3 |
| `Owlcat/Decals/Projector` | 23 | 0 | 6 |
| `Owlcat/Terrain` | 4 | 1 | 1 |
| `Owlcat/Skybox/Cubemap` | 0 | 2 | 2 |
| `Skybox/Procedural` (Unity) | 0 | 3 | 3 |

The closure also carries 164 `ComputeShader`s (Owlcat's render pipeline) and VFX Graph
content (27 `VisualEffectAsset`s, 3 `VisualEffect`s, 3 `VFXRenderer`s) **[M]**; VFX
Graph would be a separate porting job **[I]**.

**Unsupported component families, gameplay first** (instances in the area; all decode,
none have behaviour in Rust yet) [M]:

| Tier | Classes (count) |
|---|---|
| Gameplay | `LocatorView` 67, `UnitSpawner` 33, `MapObjectView` 31, `InteractionLoot` 20, `FactHolder` 17, `ScriptZone` 17, `ScriptZoneCylinder` 12, `PerceptionCheckComponent` 10, `SpawnerDifficultySettings` 10, `CRCalculator` 9, `UnitGroupView` 9, `InteractionSkillCheck` 8, `AreaEnterPoint` 6, `ScriptZoneBox` 5, `SpawnerCustomCutscene` 5, `SpawnerSummonPoolSettings` 5, `SpawnerMakePassive` 3, `SpawnerInteractionBark` 1, `SpawnerInteractionDialog` 1, `AreaTransition` 1, `KingdomLootComponent` 1, `SpawnerRoamingSettings` 1, `SpawnerAdditionalActions` 1 (24 classes with Transform) |
| Supporting | `ValidNavmeshArea` 73, `Highlighter` 31, `FogOfWarBlocker` 8, `PlatformDependent` 4, `UnitMultiHighlight` 2, `AstarPath`, `NavMeshManager`, `SwitchPrefabs`, `FogOfWarArea` 1 each; colliders (88 box, 28 mesh, 3 character controller, 2 capsule, 1 terrain) |
| Presentation (top) | `Light` 976 with `OwlcatAdditionalLightData` 974, `ParticleSystem` 423 with `ParticlesMaterialController` 387, `MeshRenderer` 697, `DisableSunShadowWhenNotVisible` 272, `ForcedCullingRadius` 247, `SoundSurfaceObject` 239, `PooledFx` 171, `FxFadeOut` 167, `FxLocator` 75, 37 more classes |
| Inert | `ApplyPrefabAtRuntime` 318 (empty marker), `TransformFreezer` 16, 3 editor helpers |

Only one kind of `UnityEvent` binding occurs: 4 `set_enabled` calls from `DistanceController` events.

**Game-wide long tail [M]** (`scene_census.json`): 1,224,286 MonoBehaviours of 439
script classes in all scenes: 72 gameplay classes (104,232 instances), 13 supporting
(83,289), 136 presentation, 5 inert, and **213 not yet classified** (79,228 instances,
e.g. `NonConvexMeshCollider`, `VFXOwlcatTransformTarget`, Dungeon Architect graph data,
Timeline tracks, `RoamingWaypointView`, `FogOfWarRevealerSettings`). The caves use 24
of the 72 gameplay classes. 26,816 MonoBehaviours point at scripts in another bundle
(the census resolves scripts within a bundle only).

**Unresolved references:** none in the area. **Formats not yet handled:** crunched
textures (above); Enlighten GI payloads (`GI/…/*.pos` entries in some scene bundles,
carried as raw data); serialized-file formats other than 17–22; multi-tile navmesh
tiles (not used here).

## 5. Navigation

**Format [M, from the decompiled `AstarSerializer`]:** the `.nav` bundle holds one
`TextAsset` whose bytes are a zip (DotNetZip): `meta.json` (version `4.3.10`, written
with a bare, non-JSON version number), `graph{N}.json` settings (TinyJson), and binary
`graph{N}_extra.binary` (nodes), `graph_references.binary` (each node's serialized
`NodeIndex` in `GetNodes` order), `graph{N}_references.binary` (connections) and
`node_link2.binary` (empty here). Owlcat adds nothing to the cache.

| | Graph 0: RecastGraph | Graph 1: GridGraph |
|---|---|---|
| Settings | cell 0.2 m, 25-cell tiles (5 m), radius 0.5, height 3, climb 0.5, slope 75°, colliders and terrain rasterized, navmesh cutting on | 0.5 m nodes, 8 neighbours, sphere collision Ø1, climb 1, erosion 2 iterations with tags from 1 |
| Size | 28 × 26 = 728 tiles, 4,192 vertices, **1,061 triangles**, 2,484 connections | **280 × 260 = 72,800 cells, 14,040 walkable**, 105,206 connections |
| Bounds (walkable) | (20.0, 31.0, 40.0) to (155.0, 49.9, 170.0) | (26.3, 31.1, 48.3) to (149.3, 46.4, 162.3) |
| Tags | all 0 | 0: 9,955, 1: 2,103, 2: 1,982 (erosion tags) |
| Components | 666, 345, 50 triangles | 9,200, 4,217, 623 cells |

Both graphs agree on connectivity: three regions in the same proportions, which
cross-checks both decoders [M]. (The regions are presumably joined by the "ClimbOver"
transitions [I].) Node penalties are all zero, and Owlcat's `GraphNode.Penalty` getter
returns 0 regardless [M, code].

**Queries [M]:** `nav.json` holds 30 queries from the area's enter points to spawners and
zone centres: 24 found, 6 unreachable (different regions) on both graphs. Funnel paths
are never shorter than their endpoints' straight line, run 0.93–1.02× the grid paths'
length, and snap off-mesh authored points by up to 1.85 m. Each result lists node keys
that survive a reload (recast `(tile, index in tile)`, grid `NodeInGridIndex`), since
`NodeIndex` is reassigned on load.

**What a faithful port needs [I, from code; Codex Sol brief checked against the
source at the cited points]:**

- **Which graph:** real-time movement searches graph 0 (`UnitMovementAgent.PathTo`,
  mask 1); turn-based movement searches graph 1 with `ABPathWithClearance` (mask 2); AI
  and click previews call `FindPath`, which uses mask 2 in turn-based combat and **no
  mask** (both graphs eligible) in real time.
- **Search:** 4-ary heap whose bubble-up prefers lower F then *higher* G and whose
  removal prefers lower F then *lower* G (verified in `BinaryHeap.cs`); triangle
  connections expanded **backwards** (verified); grid directions 0→7; closed nodes
  re-opened through `UpdateRecursiveG`; ABPath endpoint special costs; Euclidean
  heuristic, scale 1, on `Int3.costMagnitude` (double magnitude narrowed to float,
  then rounded to even; ported); triangle position
  `(v0+v1+v2)*0.333333f` rounded per component (we evaluate the product in double, as
  Mono does; to confirm).
- **Endpoints:** `GetNearest` with a 100 m limit, 3D comparison across graphs, XZ mode
  when the constraint asks for it, area-constrained end node, ally-blocked nodes
  excluded.
- **Turn-based clearance:** unit cell size `RoundToInt(corpulence / nodeSize)`; tag
  masks exclude the first `size−1` erosion tags and tag 31; a start-escape region of
  2×corpulence ignores dynamic blocking; combat writes `DynamicBlocked`/`AllyBlocked`
  counters and tag 31 into the grid and re-erodes it per acting unit.
- **Navmesh cuts:** 5,189 `NavmeshCut`s in scenes game-wide (none in this area's
  scenes); doors and map objects toggle authored cuts, pit area effects create them.
  Cuts re-tile the Recast graph (`CutPoly → DelaunayRefinement → ReplaceTile`, with
  `System.Random` perturbation) and `CombatController` separately projects them onto
  the grid.
- **Modifiers:** real time: StartEnd → Funnel (twice) → Owlcat `RemoveExcessPoints`;
  turn-based: StartEnd → Funnel → Raycast (graph raycasts).
- **RNG:** `RandomPointOnSurface` draws from `UnityEngine.Random` (QA code only); gameplay
  draws in `FreePlaceSelector` and movement slow-down; these belong on the RNG tape.

## 6. Live-dump spec (for WrathOracle)

Purpose: diff the running game against `inventory.json` and `nav.json`. One JSON-lines
file per capture, taken after the area finishes loading (`AreaDidLoad`) from a fixed save
(Manual_13), with no units moving. Every float is written as decimal and as its 32-bit
pattern (`"x": [12.5, "0x41480000"]`).

1. **Header:** game version, SHA-256 of `Assembly-CSharp.dll` and
   `blueprints-pack.bbp`, area and area-part blueprint GUIDs, loaded scene names
   (`SceneManager`), save name, difficulty, mods, frame number, and
   `UnityEngine.Random.state` before and after the dump.
2. **Scene objects:** for every GameObject in a loaded area scene that has any component
   in `inventory.json`'s classes (or any `EntityViewBase`): scene name, root name,
   sibling-index path (`GetSiblingIndex` from below the root; our `sibling_path`),
   name, `activeSelf`, `activeInHierarchy`, layer, `localPosition`, `localRotation`,
   `localScale`, `position`, `rotation`, `lossyScale`, `localToWorldMatrix`, and each
   component's full type name and `enabled`. Join key: (scene, root name,
   sibling path); for entities, `UniqueId` too.
3. **Entities:** for each `EntityViewBase`: `UniqueId`, `IsInGame`, view type, whether
   `Data` is attached, `Data.UniqueId`, `Data` type, `IsInState`, `IsRevealed`; and
   `Game.Instance.State.LoadedAreaState.AllEntityData`: `UniqueId`, type, `IsInState`,
   `IsInGame`, has view, position (units), so scene-authored and save-provided state can
   be told apart.
4. **Spawners** (`UnitSpawnerBase`): `UniqueId`, blueprint GUID, `HasSpawned`,
   `SpawnedUnit` unique ID and blueprint GUID, unit position and orientation, owning
   `UnitGroupView` ID, and the settings components on the GameObject with their
   serialized fields.
5. **Script zones:** `UniqueId`, blueprint GUID, `IsActive`, `OnceOnly`, `PlayersOnly`,
   and `Shapes` in list order: shape type, GameObject key, serialized fields,
   `localToWorldMatrix`, `Center()`, `GetBounds()` (center, size). Then, for each
   shape, the 38 probes (19 points × corpulence 0 and 0.5) from `inventory.json` (`zones[].shapes[].probes`), each
   evaluated with `shape.Contains(point, corpulence)` at the listed corpulence, and
   `ScriptZone.ContainsPosition(point)` at corpulence 0.
6. **Map objects and interactions:** per `MapObjectView`: `UniqueId`, type, interaction
   parts (`InteractionLoot` loot blueprint and container settings, `InteractionSkillCheck`
   skill and DC as evaluated, `PerceptionCheckComponent` DC), highlight state;
   `AreaEnterPoint` blueprint GUID and pose; `AreaTransition` settings; `LocatorView`
   pose; `FactHolder` blueprint and the facts it attached.
7. **Navigation state:** for each graph in `AstarPath.active.data`: type, GUID, node
   count, walkable count, and an FNV-1a-64 hash over (node key, position, walkable,
   tag, sorted connection keys and costs) using our node keys; `AstarPath` settings
   (`heuristic`, `heuristicScale`, `maxNearestNodeDistance`, thread count); every active
   `NavmeshCut` (GameObject key, type, enabled, world contour points); and the
   tiles they modified.
8. **Navigation queries:** for each query in `nav.json` (exact start and end points):
   (a) `GetNearest(point, NNConstraint.Default)` with graph masks 1 and 2 → node key and
   clamped position; (b) a real-time path: `ABPath.Construct(start, end)` on mask 1
   through a Seeker set up like `UnitMovementAgent` (StartEnd + Funnel), recording
   `CompleteState`, node keys, `vectorPath` before and after modifiers, end-node G and
   searched-node count; (c) a turn-based path: `ABPathWithClearance` on mask 2 for
   corpulence 0.5 and 1.0, same fields. Check that `Random.state` is unchanged.

A diff tool then compares, per key, presence, flags, transforms (bit-exact first,
then within 1e-5), IDs, blueprint GUIDs, shape results and path fields.

## 7. Risks

1. **Spawn-time content isn't in the dependency list [M].** Unit prefabs alone add 18
   bundles here; equipment, FX and portraits come from blueprints. The importer must
   walk blueprints to build complete packs, or loads will miss assets.
2. **Exact pathing is a port of a modified A* [M: searches and smoothing exact (2026-10-15); cuts
   left, §9].** Heap tie-breaking, backward
   expansion, re-opening, nearest-node rules, clearance and runtime cuts all affect
   which path wins, and turn-based movement is T0/T1. Expect a few weeks of oracle-driven
   work.
3. **Shading is entirely custom [M].** Seven Owlcat shader families in one area, plus
   compute shaders and VFX Graph in the closure. That's the rendering spike's problem,
   but the importer must carry every material property and keyword.
4. **Crunched lightmaps [M]** need Unity's decoder to stay bit-identical.
5. **Runtime behaviour hidden in scenes [I]:** platform switches (`PlatformDependent`,
   `SwitchPrefabs`) and 213 unclassified script classes game-wide; some may matter to
   rules.
6. **Float reproduction [I]:** transforms are composed in `f32` by us and by Unity's
   native code in its own order; small differences are likely and need a tolerance in
   T1, never in T0 (zone containment edges).
7. **AssetRipper isn't on the critical path [M].** It's faithful for meshes and scene
   data, but our reader already covers everything gameplay needs. Keep it as a
   cross-check and for formats we haven't written yet.

## 8. Effort estimate for a production importer [I]

Assuming the reader above as the base and Claude/Opus implementing with Astra review:

| Work | Estimate |
|---|---|
| Reader hardening: crunch decode, GI payloads, error reporting, fuzzing | 1 week |
| Blueprint-driven closure (units, equipment, FX, portraits) and pack layout (shared + per-area) | 1–2 weeks |
| Scene gameplay → `wr-sim` types for the 72 gameplay classes (24 for the caves first), with oracle diffs | 3–5 weeks |
| Meshes, materials (all properties/keywords), textures (BC as-is), lights, probes, lightmaps into packs | 2–3 weeks |
| Animation data the sim needs (clip durations, events, `AnimationClipEvent` references) | 1–2 weeks |
| Navigation: the rest after this round (funnel unwrap and raycast smoothing: done 2026-10-15; navmesh cuts, combat blocking, dynamic graph updates), oracle-matched | 2–3 weeks |
| Classifying the 213 unknown classes and deciding which matter | 1 week |
| **Total** | **11–17 weeks**, excluding shader ports and VFX Graph |

## 9. Live comparison: WrathOracle dumps vs wr-import (2026-10-11)

`wr-import diff [--capture DIR] [--only scene,hierarchy,nav_graph,nav_queries]` decodes
the area from the install (0.25 s), diffs it with a WrathOracle capture field by field
and writes `cache/import/<area>/oracle_diff.json`: per field a tally (exact, close,
mismatched, expected runtime difference) and up to 400 examples of each kind. "Exact" is
bit-identical; "close" is within 1e-5 or 8 ULP (the dump's floats round-trip exactly,
and the writer prints -0 as 0, so ±0 count as equal). Numbers below are for `check-1`
[M]; `oracle-2` and `oracle-3` give the same within a few objects.

**Scene and entities: exact where the data is static.**

| What | Result |
|---|---|
| Entity views (157 in our scenes) | all present with the same unique ID, class, scene, path, sibling path, layer, active flags (4 differ, below), local and world transforms (157/157 bit-identical `local_to_world`; 16 lossy scales and 14 rotations within 8 ULP), component types and order, `enabled` flags (365/365) |
| Serialized fields of their components | 1,447/1,447 equal; 409/409 blueprint references; 162 enums are names in the dump and values in the file (counted as expected); 14 object references not compared (different encodings) |
| Spawners (33) | blueprint, `spawn_on_scene_init`, position, settings components exact; 2 rotations within 8 ULP; runtime state (spawned unit, died) not comparable |
| Unit groups (9) | member spawners equal (derived from the hierarchy) |
| Script zones (17) | blueprint, shape count, type, `local_to_world`, centre, bounds, local fields exact; 646/646 probes agree with the game's `Contains` |
| Enter points (6), area transition (1) | present, poses and blueprints exact |
| Only in the game | 25 views of units spawned at runtime (`BaseMechanics`) — expected |
| Mismatches | 4 views whose active flag the game changes from state (`Angel_Sword`, `Climb`, `Climb_Back`, `Exit`) |

**Hierarchy: all 5,204 GameObjects of the five scenes present** (3 extra runtime objects
in the game). Names and component lists match; local transforms are bit-identical for
5,198 positions and 5,127 rotations. World transforms: 3,334 positions and 2,931
`local_to_world` matrices bit-identical, nearly all the rest within 8 ULP (Unity
composes in another order). Real mismatches, all runtime changes [M: examples in the
report]: 245 `*_!Visual` objects the game moves to layer 11 at load (cause not traced
[I]); 47 objects under `Angel_Sword` and the four `TERRAIN/Baked Terrain` meshes whose
activity the game sets (`BakedTerrain` turns those on); critters that walk
(`Critters/Tarantula*`) and animated fungi (`PrologueCaves_VioletMushroom00`,
`BigMushroom00`, rotation and scale).

**Navigation graphs: identical.** 373,357 of 373,364 checks exact: every Recast tile's
vertices, triangles, node vertex indices, flags, tags, walkability, penalties and
connections (`[tile, index, cost, shape edge]`), and every grid node's position, flags
and connection bits, plus graph settings; the other 7 are settings the dump has and the
cache doesn't serialize.

**Path queries.** The A* Pathfinding Project 4.3.10 search is ported (since 2026-10-15 in
`crates/wr-nav`, the importer keeping the cache decoder; `astar.rs`:
4-ary heap with its tie-breaking, `PathHandler` bookkeeping, backward triangle
expansion, `UpdateRecursiveG` reopening, grid direction order, `ABPath` endpoint costs
and partial paths, `ABPathWithClearance`'s tag masks and ending condition), as are
nearest-node rules for both graphs (`nearest.rs`: BBTree search, XZ vs 3D distance,
`maxNearestNodeDistance`, constraints) and the modifiers (`modifiers.rs`,
`funnel.rs`: StartEnd, Funnel with unwrap, Owlcat's `RemoveExcessPoints`, the
graph-raycast smoother), with Mono's float semantics (`cs.rs`).

| Query kind (46 queries each: 16 endpoint pairs + the 30 `nav.json` ones) | Exact |
|---|---|
| Nearest nodes and clamped points, both graphs (12 endpoints + 8 route points) | 20/20 |
| `navmesh_raw` (ABPath, mask 1, no modifiers) | 46/46, every field |
| `grid_raw` | 46/46, every field |
| `navmesh_agent` (real-time seeker): state, nodes, step costs, end-node G, searched nodes, endpoints | 46/46 |
| … corner points after StartEnd + Funnel ×2 / after `RemoveExcessPoints` | 46/46 / 46/46 (were 26 / 27) |
| `tb_clearance_0.5`: search fields | 46/46; corner points 46/46 (were 32) |
| `tb_clearance_1.0`: search fields | 46/46; corner points 46/46 (were 44) |

So **every query agrees with the game, smoothing included**: 3,387 of 3,387 checks on
`check-1` and on `oracle-3` (`oracle-2`, older, lacks the graph-unchanged guard: its one
mismatch). Until 2026-10-15 the smoothing differed:

- **Funnel unwrap [M, fixed 2026-10-15].** `Funnel.Unwrap` rotates portals into a plane
  with Unity's native `Quaternion.FromToRotation`. Unwrapped coordinates feed exact-sign
  cross products, so its last bits decide *which* portal vertex becomes a corner (e.g.
  `party -> camelia_zone`: the game emits `(100.0, 38.68, 134.6)` twice and
  `(85.0, 38.45, 145.0)` twice, the approximation emitted `(106.6, 39.04, 134.0)` twice and
  an extra `(82.4, 37.65, 141.6)`). The port now follows the arm64 player's machine code
  (`UnityPlayer.dylib`; the icall table maps `Quaternion::FromToRotation_Injected` to
  `FromToQuaternionSafe`): the magnitudes are single precision (Mono's `magnitude` is
  double), `SetFromToRotation` is Möller and Hughes' original code with
  `h = (1 - e) / |v|²` (not `1 / (1 + e)`), its parallel thresholds compare in double
  against `±(1 - (double)1e-6f)`, and its 180° case turns about `u × f` with
  `u = normalize(f × X)` (or `f × Y`), ignoring `to`. `MatrixToQuaternion` was already
  exact. The turn-based raycast smoothing, downstream of the same corners, became exact
  with it.
- **Native math checked against the player itself.** `wr-nav`'s `native_check.rs` (an ignored
  test: `cargo test --release -p wr-nav --lib native_check -- --ignored`) `dlopen`s the
  oracle build's player and calls its `FromToQuaternionSafe`, `InvertMatrix4x4_Full`
  (Mesa's `invert_matrix_general`), `Matrix4x4f::SetTRS` and `EulerToQuaternion` (order 4,
  Z-X-Y) on a million generated inputs each: every result bit-identical with our ports. The
  grid transform's `Matrix4x4.inverse`, `TRS` and `Quaternion.Euler` (trivial in the caves,
  not elsewhere) are so verified; Rust's `sin_cos` equals the player's `__sincosf_stret`
  on every float in [-8, 8].

**Not compared yet:** navmesh cuts (none active in these scenes), combat-time blocking
(`DynamicBlocked`/`AllyBlocked`, all zero in the dump), A*'s threading, and
`ScriptZone.ContainsPosition` (the dump has it; we compare only the shapes' `Contains`).

**Review of the diff tool** (GPT 6 Astra, high; findings checked and fixed): zone probes
were compared by index with our regenerated probes instead of evaluated at the captured
points (now evaluated at each captured point and corpulence; still 646/646); missing
capture sections, scene dumps or query kinds were skipped silently (now counted as
mismatches); repeated components of one type were all joined to our first (now joined by
type and occurrence, and "added at runtime" requires ours to be an ordered subsequence).

## Review

GPT 6 Astra (high) reviewed `tools/wr-import` and this report read-only. It confirmed
the format-22 header, type and object tables, type-tree blobs, transform composition,
PPtr/CAB resolution, zone bounds and containment (including the quirks), Recast vertex
packing, connections, portal clipping, funnel orientation, BC size math, and the
caves numbers. Its findings, each checked against the code and reference before
acting:

| Finding | Verdict | Fix |
|---|---|---|
| Elements of aligned arrays were aligned one by one; UnityPy reads them packed and aligns once | Real (no shipped object hit it: all still decode) | Array-context alignment, with a test |
| A `ManagedReferencesRegistry` inside referenced data would be read again | Real | Nested registries read nothing, as in UnityPy; tested |
| Block table at end of file could be parsed from a 64 KiB prefix | Real (not used by these bundles) | Prefix parse refuses it and falls back to the whole file |
| Grid width/depth didn't follow `GridGraph.CalculateDimensions` | Real | Ported literally (absolute sizes, node-size clamp, `Mathf.Approximately` ceil); tested |
| `cost_dist` didn't narrow to float and round to even like `Int3.costMagnitude` | Real | Ported; tested with the reviewer's case |
| AssetRipper mesh matching could reuse one export twice | Real | One shared record per export; results unchanged (9,457 identical) |
| A hierarchy cycle could hang zone traversal | Real (no cycles in data) | Visited sets in all hierarchy walks |
| "Renderer slots" counted every material reference | Real | Slots counted from renderers' `m_Materials` only (1,400, matching the reviewer's independent count); other references reported separately |
| Inventory contract overstated; probe count was 38, not 36 | Real | This report corrected |
| "Bit-identical" treated ±0 as equal | Real | Strict bit comparison; all 5,204 transforms still match |

After the fixes, all 487,816 closure objects and 8,245,482 scene objects still decode,
and `scripts/check.sh` passes.
