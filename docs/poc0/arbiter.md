> Investigation by gpt-6.1-sol (Codex, read-only), 2026-10-10; reviewed by Claude, with key claims spot-checked against the save. Relative `evidence/`, `inputs/` and script paths refer to `cache/research/sol-arbiter/` (gitignored, local only).

# Arbiter camera benchmarks in Prologue_Caves_1

## Verdict

**Use the shipped Arbiter instruction's eight authored points, but drive them from a small UMM mod rather than running the complete Arbiter job.** The camera operations and screenshot/measurement tasks have real player-build implementations. However, this installation is missing Arbiter's name-to-blueprint index, its normal area job starts a **new preset game rather than loading your save**, and its output/server/cleanup behavior is unsuitable as an untouched local benchmark harness. None of this prevents a mod from resolving the instruction directly by GUID and calling the game's camera API.

**Arbiter does not provide an exact cross-engine replay by itself.** Its points are rig/ground targets plus yaw and zoom, not final render-camera poses; its waits mix real-time seconds and rendered-frame counts; its measurement pass runs an evolving game. Bake the **actual rendered camera transforms and projection** once, export an explicit schedule, and replay those resolved poses in both engines. This gives a reproducible camera workload, not a guarantee of identical simulation, pixels, or performance.

Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterClientIntegration.cs:293`, `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaCheckerTask.cs:18`, `Assembly-CSharp/Kingmaker.QA.Arbiter/StartPresetTask.cs:19`, `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaMeasurementsTask.cs:35`, and `evidence/shipped-inventory.json:5`.

### Scope and evidence notation

- **Verified-static** means directly inspected decompiled method bodies, archive JSON, or filesystem inventory. It does **not** mean executed in the game.
- **Proposed/inferred** means an implementation recipe or consequence of those bodies. No game was launched, no mod was installed, and no QA endpoint was contacted during this investigation.
- Code references beginning `Assembly-CSharp/` or `Owlcat.Runtime.*` are relative to `decompiled/`. References beginning `mods/` are relative to `oracle/` (then `Wrath-Mac/`). References beginning `evidence/` are relative to this scratch directory.
- Blueprint evidence is copied **byte-for-byte**, preserving original JSON formatting and line numbers, from `~/Library/Application Support/Steam/steamapps/common/Pathfinder Second Adventure/Wrath.app/blueprints.zip`. `evidence/blueprint-audit.json:1` records archive path, scan counts, selected entries, GUIDs, and SHA-256 hashes.
- The inspected installation's `Version.info` says `2.7.0y.782`, revision `ab086432e8fe696a81af54c6c4639cbb0311add6`, with a build timestamp of April 8, 2025. This is installation metadata, **not a claim about the newest release**. Inventory: `evidence/shipped-inventory.json:13`.

## 1. What Arbiter is and how it starts

### Architecture and entry points — verified-static

`Kingmaker.QA.Arbiter.Arbiter` is a persistent `MonoBehaviour` singleton, with a string instruction queue, a current instruction blueprint, and a stack-like parent/subtask runner. `Awake()` initializes it. `Update()` advances one task enumerator, handles nested tasks, and subtracts float delays using `Time.unscaledDeltaTime`. `ArbiterTask.Delay` treats a yielded float as seconds; a yielded `null` is a next-update wait, **not** an end-of-render fence.

Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/Arbiter.cs:19`, `Assembly-CSharp/Kingmaker.QA.Arbiter/Arbiter.cs:131`, `Assembly-CSharp/Kingmaker.QA.Arbiter/Arbiter.cs:203`, `Assembly-CSharp/Kingmaker.QA.Arbiter/Arbiter.cs:233`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterTask.cs:15`.

| Entry | Actual behavior | Evidence |
|---|---|---|
| Startup argument `-arbiter=Caves_1` | `GameStarter.IsArbiterMode()` selects Arbiter startup; startup loads the bundled `Arbiter` scene. | `Assembly-CSharp/Kingmaker/GameStarter.cs:448`, `Assembly-CSharp/Kingmaker/GameStarter.cs:597` |
| `PlayerPrefs["ArbiterInstruction"]` | Also selects Arbiter mode. `InstructionSettings` consumes and deletes this preference. | `Assembly-CSharp/Kingmaker/GameStarter.cs:604`, `Assembly-CSharp/Kingmaker.QA.Arbiter/InstructionSettings.cs:38` |
| `-arbiter=/absolute/path/scenarios.txt` | `.txt` denotes a list, read in line order. Without an instruction, default is `arbiter.txt`; missing list falls back to server discovery. | `Assembly-CSharp/Kingmaker.QA.Arbiter/InstructionSettings.cs:14`, `Assembly-CSharp/Kingmaker.QA.Arbiter/Arbiter.cs:161` |
| `arbiter_start`, `arbiter_stop`, `arbiter_status`, `arbiter_list_instructions` | Compiled cheat methods; start sets the preference and loads the scene, or queues another instruction on an existing instance. | `Assembly-CSharp/Kingmaker.Cheats/CheatsBots.cs:81` |
| `Arbiter.Instance.RunInstruction(string)` / `RunInstructionsFile(string)` | Public methods enqueue an instruction or replace the queue, and clear the finished flag; require successful initialization first. | `Assembly-CSharp/Kingmaker.QA.Arbiter/Arbiter.cs:207` |
| `camera_teleport`, `arbiter_camera_teleport` | Cheat entry points to `ArbiterInstantMoveCameraStarter`; latter selects an authored sample by instruction name and sample ID. | `Assembly-CSharp/Kingmaker.Cheats/CheatsBots.cs:119`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterInstantMoveCameraStarter.cs:20` |

The regular command-line parser splits each argument on `=`, **not** an argument followed by a space-separated value; use `-arbiter=Caves_1`, not `-arbiter Caves_1`. Keys are case-sensitive dictionary strings. The status server separately recognizes the exact spelling `-ArbiterStatusPort=44444`. Evidence: `Assembly-CSharp/Kingmaker.Utility/CommandLineArguments.cs:19`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterStatusServer.cs:46`.

Compiled cheat methods do not establish that the consumer build exposes an accessible console or registers every command. A UMM/Harmony mod can call public runtime APIs directly, or invoke private cheat methods by reflection; neither requires a visible console. **Runtime availability remains untested.**

### Instructions and tasks — verified-static

- `BlueprintArbiterInstruction.Test` is **only the first blueprint component**, not an arbitrary list of tasks. `GetNextTest()` dispatches an `AreaCheckerComponent` to `AreaCheckerTask` or a `WeaponCheckerComponent` to `WeaponCheckerTask`. Unknown components log an error. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/BlueprintArbiterInstruction.cs:9`, `Assembly-CSharp/Kingmaker.QA.Arbiter/Arbiter.cs:297`.
- `AreaCheckerTask`: report start → `StartPresetTask` → resolution → optional time of day → **entire screenshot pass** → load measurements → **entire profiling pass** → send/cleanup → optional local-map screenshot → report pass. The screenshot and profiling are not paired at each stop. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaCheckerTask.cs:18`.
- `StartPresetTask` resets to the main menu if necessary, overrides preset difficulty to Core, and calls `MainMenu.EnterGame(() => Game.Instance.LoadNewGame(preset))`. **It replaces your loaded save state.** Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/StartPresetTask.cs:19`.
- `AreaCheckerComponent`: area, optional override preset, optional time of day, map screenshot flag, ordered parts and ordered points. Preset is override or area's default. Sample IDs flatten the part lists in order, zero-based. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaCheckerComponent.cs:18`, `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaCheckerComponent.cs:33`.
- Screenshot/measurement tasks activate configured entry-point scene objects, teleport to each part's entry point, wait for loading and cutscenes, and acquire `FixCamera`. `WaitForCutsceneTask` skips cutscenes, checking at five-second intervals, and force-stops control-locking cutscenes after ten seconds. These are game-state mutations, not passive camera observation. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaScreenshotsTask.cs:21`, `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaMeasurementsTask.cs:35`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterClientIntegration.cs:266`, `Assembly-CSharp/Kingmaker.QA.Arbiter/WaitForCutsceneTask.cs:21`.
- Weapon checking operates on equipment and inventory/doll-room UI, with resolution and screenshot/probe machinery; it is not a cave camera-route generator. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/WeaponCheckerTask.cs:60`.
- `ExportMaps` is a special queue string, but its shipped task **only logs `Map export is not supported in build`**. Separately, the optional *current-area local-map screenshot* has a real `LocalMapRenderer.Draw`/PNG implementation, normally 2048×2048; this is a map image, not geometry, navmesh, or route export. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/Arbiter.cs:285`, `Assembly-CSharp/Kingmaker.QA.Arbiter/MapsExportTask.cs:7`, `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaCheckerTask.cs:48`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterUtils.cs:94`.

### Exact built-in route timing and visual profiles

**Screenshot pass:** disable VSync, hide UI, set clear weather, disable clouds/wind/FoW/FX, hide rabbits/birds, wait 5 seconds, pause the game. For each point: move immediately, wait 5 seconds, hide units, synchronously capture PNG, show units. Unpause/restore operations and further 5-second waits follow the part. Some “disable”/“restore” operations are stubs or buggy in this build; see section 3. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaScreenshotsTask.cs:39`.

**Measurement pass:** disable VSync; teleport/wait; explicitly **unpause**, wait 5 seconds; disable FoW. At each point move immediately, add a revealer at the requested point, warm up **15 yielded frames**, then sample **120 yielded frames**. FPS is:

```text
round(sum(round(1 / unscaledDeltaTime_i), i=0..119) / 120)
```

This is an average of rounded instantaneous FPS, **not** `120 / elapsedSeconds`, a frame-time distribution, or GPU execution time. Then collect memory/custom measurements/counter medians, remove the revealer, and proceed. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaMeasurementsTask.cs:55`, `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaMeasurementsTask.cs:65`, `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaMeasurementsTask.cs:79`.

Consequences, inferred from these bodies: wall-clock waits overshoot at frame boundaries, sample durations vary with rendering speed, loading/cutscene duration is not fixed, and live simulation has advanced by different amounts when different machines reach a sample. There is **no authored continuous camera path, travel speed, timestamps, or interpolation curve** in this instruction. It is an ordered sequence of teleports and dwell samples.

### What it records

| Record | Contents and limitations | Evidence |
|---|---|---|
| Point PNGs | `<sample-id>.png`; `Screenshot.CapturePNG(Game.GetCamera(), root width, root height)`, not normal desktop screenshots. | `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterClientIntegration.cs:23` |
| Area probe | Area/part names, active/static/light scenes, sample ID, integer FPS, custom dictionary. **No rig or camera transform, projection matrix, or per-frame timestamps.** | `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaProbeData.cs:8` |
| Memory | Total allocated/reserved Unity memory, Mono used/heap, graphics-driver allocated memory, all divided by 1,048,576 into integer MiB. These are not necessarily total process RSS or VRAM resident usage. | `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaProbeData.cs:49` |
| Custom counts | Awake/total units, unique unit prefabs, corpses, map/interactable objects, traps, cutscenes, script zones; UI atlas/non-atlas sizes collected after screenshot pass. | `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterMeasurements.cs:21`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterMeasurements.cs:40` |
| External memory tool | Tries `http://localhost:8000/memory`, fields `vms`, `rss`, `gpu`; a WebException disables subsequent requests. Units are forwarded from that service, not established here. | `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterMeasurements.cs:17`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterMeasurements.cs:83` |
| Profiling counters | Owlcat/game `Counter` medians, e.g. Runner/animation/particles/render. Counter storage is a 30-frame ring of frames that actually added measurements; medians are not the median of all 120 sample frame times. Unused counters can report zero. `RecorderAdapter.AddMeasurements()` is empty. | `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaMeasurementsTask.cs:94`, `Assembly-CSharp/Kingmaker.QA.Arbiter.Profiling/Counters.cs:78`, `Owlcat.Runtime.Core/Owlcat.Runtime.Core.ProfilingCounters/Counter.cs:36`, `Assembly-CSharp/Kingmaker.QA.Arbiter.Profiling/RecorderAdapter.cs:25` |
| Probe envelope | Project/build/revision/platform/branch/job GUID/timestamp/instruction/type/hardware plus samples. `SaveJsonData()` writes `info.json`; upload zips the data folder. | `Assembly-CSharp/Kingmaker.QA.Arbiter/GeneralProbeData.cs:18`, `Assembly-CSharp/Kingmaker.QA.Arbiter/GeneralProbeData.cs:72`, `Assembly-CSharp/Kingmaker.QA.Arbiter/GeneralProbeData.cs:110` |
| Job reports | XML at `Arbiter/report.xml`, cache JSON at `Arbiter/report_cache.json`; optional instruction/area/scene suites and statuses. Flags configure paths and which suites are generated. | `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterReporter.cs:18`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterReporter.cs:51`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterReporter.cs:196` |

## 2. Shipped Caves blueprint data

### Search coverage — verified-static

The read-only ZIP scan examined **236,661 `.jbp` files**, finding **341 instruction-type matches**. Exactly **one** instruction references the target area or selected entry-point GUID: `QA/Arbiter/Scenario/Act_0_Prologue/Caves_1.jbp`. This is bounded to this inspected archive and the known instruction type/area references, not a claim about unreleased editor content. Audit algorithm: `audit_blueprints.py:42`; results: `evidence/blueprint-audit.json:3`.

### Relevant assets, GUIDs, and contents

| Asset | Asset GUID | Verified content | Copied JSON evidence |
|---|---|---|---|
| `QA/Arbiter/Scenario/Act_0_Prologue/Caves_1.jbp` | `0c9239f8cff8dd348a3a4251573d9fd6` | `BlueprintArbiterInstruction`; one `AreaCheckerComponent`; area below; no override preset; `OverrideTimeOfDay=false` (stored `Morning` is inactive); `MakeMapScreenshot=false`; one part; eight points. | `evidence/blueprints/QA/Arbiter/Scenario/Act_0_Prologue/Caves_1.jbp:2` |
| `World/Areas/Act_0_Prologue/Prologue_Caves_1/Prologue_Caves_1.jbp` | `bad465e5404604346bdf71ef50b3299a` | `BlueprintArea`; no additional parts (`m_Parts=[]`); not global map; default preset below; scroll multiplier 1; no forced default yaw; Underground/IndoorLikeNight. | `evidence/blueprints/World/Areas/Act_0_Prologue/Prologue_Caves_1/Prologue_Caves_1.jbp:2`, `:66`, `:85`, `:124` |
| `.../EnterPoints/Prologue_Caves_1_Prologue_Caves_1_Enter.jbp` | `c1e6f74927029f846b403b20c7a70d20` | Both `m_Area` and `m_AreaPart` reference the same cave area. No camera coordinates or entrance transform in this JSON. | `evidence/blueprints/World/Areas/Act_0_Prologue/Prologue_Caves_1/EnterPoints/Prologue_Caves_1_Prologue_Caves_1_Enter.jbp:2` |
| `.../Presets/Prologue_Caves_1_Default_Preset.jbp` | `816615290645ad44f9b5e142490ebbd6` | Cave area/entry point; no autosave/chargen; player `a251579eef412214698134e4cab4e7b9`; XP 0; no companions; quest `c45dd2a8c06ebb145b79447db69a94e4`; starts etudes `fb8827a1b4515af408f92e1ac1bdf794` and `cab903611be84934b8ebb21623db4cfa`; moves party weapons/shields into scene loot `Loot_1`, entity `7d7c1267-7c09-4c30-9f00-ae3ddf013cce`. | `evidence/blueprints/World/Areas/Act_0_Prologue/Prologue_Caves_1/Presets/Prologue_Caves_1_Default_Preset.jbp:2`, `:22`, `:49` |
| `QA/Arbiter/ArbiterRoot.jbp` | `14b876bb540b47978760242b0919e355` | Project `PF2`, resolution **1920×1080**, ignored scene references/name filters, no route points. Runtime root uses this hard-coded GUID. | `evidence/blueprints/QA/Arbiter/ArbiterRoot.jbp:2`, `Assembly-CSharp/Kingmaker.QA.Arbiter/BlueprintArbiterRoot.cs:13` |
| `QA/Arbiter/arbiter_start_preset.jbp` | `c25c4fdd4aabfdc4cae1daa0f7db93d5` | General Arbiter start preset, **not the cave preset**: area `f11fcd859faeb4b4c8299794afc27bf0`, entry `2267cfdd9a0e7d74dafe1271b52f3d23`, XP 23000, three remote companions, no autosave/chargen. Full retained contents in copy. | `evidence/blueprints/QA/Arbiter/arbiter_start_preset.jbp:2` |

The runtime `Arbiter.StartGamePreset` serialized field is declared at `Assembly-CSharp/Kingmaker.QA.Arbiter/Arbiter.cs:33` but not used in the inspected initialization/job body. Do not assume the similarly named general preset is needed for the direct point driver.

Area scene references:

| Scene | Unity asset GUID | Evidence |
|---|---|---|
| `Prologue_Caves_1_Mechanics` | `93ce88a5d3c3c8041a07cd8d78f49422` | Cave area JSON `:9` |
| `Prologue_Caves_1_Static` | `462357f776967ad4da3d8c11312eccc9` | Cave area JSON `:16` |
| `Prologue_Caves_1_Light` | `08f41e75791cecb4c860039ac7fc347e` | Cave area JSON `:30` |
| `Prologue_Caves_1_Audio` | `5e199c97649f71f43a0deff012097567` | Cave area JSON `:48` |
| Area bounds ScriptableObject | `4d79a4e0689db6d469d19a8c5344ee2a`, file ID `11400000` | Cave area JSON `:44` |

“Cave area JSON” above means `evidence/blueprints/World/Areas/Act_0_Prologue/Prologue_Caves_1/Prologue_Caves_1.jbp`. Console static/light overrides also exist there at lines 23 and 37; benchmark the actual PC/Mac scenes, not those overrides.

### Complete authored point list, in execution order

All are inside the single component-part for entry `c1e6f74927029f846b403b20c7a70d20`. Coordinates below are **requested rig targets**, not the optical camera's final world coordinates.

| Sample | Inline point name | Position x, y, z | Yaw degrees | Raw zoom | Caves instruction JSON line |
|---:|---|---|---:|---:|---:|
| 0 | `Caves_1_1604385054612` | `120.859131, 39.99878, 144.380127` | 343.5062 | 0 | 27 |
| 1 | `Caves_1_1604385062629` | `89.7974854, 39.15758, 141.323242` | 139.810257 | 0 | 38 |
| 2 | `Caves_1_1604385069002` | `42.9290924, 34.6545143, 153.390091` | 77.8682861 | 0 | 49 |
| 3 | `Caves_1_1604385076909` | `51.7694359, 41.88687, 127.315735` | 357.044678 | 0 | 60 |
| 4 | `Caves_1_1604385085659` | `112.482346, 46.14825, 102.853241` | 336.52478 | 0 | 71 |
| 5 | `Caves_1_1604385091407` | `123.299934, 45.61065, 80.6090851` | 342.1219 | 0 | 82 |
| 6 | `Caves_1_1604385098289` | `78.56158, 39.10982, 67.9544449` | 144.222931 | 0 | 93 |
| 7 | `Caves_1_1604385108701` | `50.48398, 38.9198341, 59.285038` | 115.383659 | 0 | 104 |

Evidence for all rows: `evidence/blueprints/QA/Arbiter/Scenario/Act_0_Prologue/Caves_1.jbp:27` through its final point at `:104`. The exported `caves-authored-route.json:1` preserves the same sequence and values with explicit field names.

**These points have no separate blueprint AssetIds.** They are inline `ArbiterPoint` elements. `c5df78d16039466a9c6a8466a9847270` is their **serialization type ID**, not an individual point GUID; likewise the part's type ID is `05c1a401230a49719bc119a38d101cfd`. Their numeric name suffixes are not used as scheduling timestamps in runtime code. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterPoint.cs:7`, `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaCheckerComponentPart.cs:9`, `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaCheckerComponentPart.cs:47`.

### Not a generated grid

The shipped tasks iterate `AreaParts.ElementList` and `PointList.ElementList` exactly as serialized; sample IDs flatten those lists. No navmesh sampling/grid-generation algorithm is on this route execution path. The points are verified authored data; **how Owlcat's editor originally created them is unknown**. Do not invent a grid algorithm or infer camera timing from names. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaMeasurementsTask.cs:41`, `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaScreenshotsTask.cs:50`, `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaCheckerComponent.cs:33`.

## 3. Shipped-build dependencies and failure modes

### Assets/index — verified-static

- `Bundles/arbiter.scenes` **exists**, 119,726 bytes, hash recorded in `evidence/shipped-inventory.json:8`. `BundledSceneLoader.GetBundleName("Arbiter")` defaults to `arbiter.scenes`; loading is additive by default. File presence does **not** verify successful deserialization, scene registration, component references, or runtime startup. Evidence: `Assembly-CSharp/Kingmaker.EntitySystem.Persistence.Scenes/BundledSceneLoader.cs:26`.
- **`Contents/Resources/Data/StreamingAssets/arbiter.json` is absent.** A filename search under this app found only the Arbiter scene bundle. `ArbiterInstructionIndex` asks for `arbiter.json`; `BotInstructionIndex.Init()` reads the literal StreamingAssets file, otherwise creates an empty dictionary and tries to `Dump()` it back into StreamingAssets. `RegenerateIndex()` is empty. It does not scan the shipped blueprints automatically. Thus plain name-based `Caves_1` lookup is not ready on this install, and even an innocent index query can attempt a game-directory write. Evidence: `evidence/shipped-inventory.json:5`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterInstructionIndex.cs:21`, `Assembly-CSharp/Kingmaker.QA/BotInstructionIndex.cs:64`, `Assembly-CSharp/Kingmaker.QA/BotInstructionIndex.cs:144`.
- The desired instruction/root/area/preset JSON all exist. A mod can bypass the missing name index with `ResourcesLibrary.TryGetBlueprint<BlueprintArbiterInstruction>(guid)`, which loads via `BlueprintsCache`. Successful in-process resolution remains a smoke-test requirement; JSON existence alone is not proof of the loaded cache entry. Evidence: `Assembly-CSharp/Kingmaker.Blueprints/ResourcesLibrary.cs:101`, `Assembly-CSharp/Kingmaker.Blueprints/ResourcesLibrary.cs:540`.

### Server behavior — verified-static

- Server defaults to `https://localhost:7179`; endpoints are `/Instructions`, `/Create`, `/OnJobFinished`; `-arbiterServer=...` changes it. `SendToServer` is enabled by `-arbiter` or editor mode, **not** by the mere existence of an Arbiter instance. Missing `.txt` input can trigger `/Instructions` even when `SendToServer` is false. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/Arbiter.cs:25`, `:60`, `:147`, `:174`, `:339`.
- Completion notification is **not gated by `SendToServer`**. Use `-arbiterDisableNotifications` or patch `OnJobFinished` for an offline full-job experiment. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/Arbiter.cs:361`.
- Requests are synchronous `WebClient` operations on task/startup paths and can stall the benchmark. They also replace the process-global certificate-validation callback with “accept all”; avoid invoking those paths in a normal session. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/Arbiter.cs:344`, `:369`, `Assembly-CSharp/Kingmaker.QA.Arbiter/GeneralProbeData.cs:93`.
- Initialization opens `http://*:44444/` with `HttpListener` for `GET /Status`; this is a wildcard listener, not an explicitly loopback-only service. Listener failure is caught/logged. No endpoint or Mono/macOS support was tested. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterStatusServer.cs:21`, `:46`, `:82`.

### Output loss and other shipped limitations — verified-static

- **Offline mode does not automatically save `info.json`.** `GeneralProbeData.SendToServer()` returns before `SaveJsonData()` if sending is disabled. After a successful return, `AreaCheckerTask` calls `CleanupData()`, which deletes the probe directory/zip outside the editor. Screenshots may therefore be deleted despite a locally “passed” run. Upload failure throws before that cleanup in the area task, but failure residue is not a supported archival strategy. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/GeneralProbeData.cs:87`, `:165`, `Assembly-CSharp/Kingmaker.QA.Arbiter/AreaCheckerTask.cs:32`.
- Probe folder initialization also deletes any existing `Arbiter/<ProbeType>/<Instruction>` directory. Use a fresh run directory or intercept the output lifecycle, not a shared archival folder. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/GeneralProbeData.cs:154`.
- `MapsExportTask` is explicitly unavailable in build; `ArbiterInstantMoveCameraStarter.MoveInEditor`, `BotInstructionIndex.RegenerateIndex`, `RecorderAdapter.AddMeasurements`, and fog disable/enable methods are empty. This proves **shipped bodies are stubs**, not which original `#if` symbols produced them: preprocessor branches cannot be reconstructed from decompiled IL alone. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/MapsExportTask.cs:7`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterInstantMoveCameraStarter.cs:71`, `Assembly-CSharp/Kingmaker.QA/BotInstructionIndex.cs:144`, `Assembly-CSharp/Kingmaker.QA.Arbiter.Profiling/RecorderAdapter.cs:25`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterClientIntegration.cs:55`.
- **Do not trust the restoration helpers as exact state restoration.** `EnableFx()` sets TransparentFX objects inactive; `ShowUnits()` keeps trap objects inactive/disabled; rabbit/bird restoration searches active objects without `includeInactive:true`, so objects deactivated by hide may not be found. Other helpers generally force enabled states rather than restoring prior states. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterClientIntegration.cs:124`, `:178`, `:220`.
- The instant camera task forces the numeric parser's decimal separator to **comma**, while its starter concatenates floats using current culture. Dot-decimal strings can be misparsed or fall back to zero. Its `arbiter_...` branch immediately `yield break`s; the usual authored-point starter instead constructs `instant_move_camera ...`, so that dead branch is not itself proof the normal command is dead. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterInstantMoveCameraTask.cs:28`, `:80`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterInstantMoveCameraStarter.cs:26`.
- Instant camera task can load a fresh preset, teleport the party to `(1000,1000,1000)`, pause, disable FoW, and increment `FixCamera` without a corresponding release in that task. It is a QA inspection shortcut, not a clean reusable benchmark primitive. Evidence: `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterInstantMoveCameraTask.cs:93`.

### Would it run from a mod?

**Inferred: the direct point driver is viable and avoids the missing infrastructure.** Its actual camera methods have shipped implementations, no Arbiter singleton/server/index is needed, and the existing UMM benchmark already uses the game's save-loading path and Unity screenshot API. **Not verified:** a compiled route mod, successful GUID resolution from the running game's cache, actual shots, counter availability, or repeat-run camera drift.

**Inferred: a full Arbiter job is also potentially repairable**, but requires at least an in-memory name index, offline network/output interception, and testing the Arbiter scene. In a mod, Harmony can patch `BotInstructionIndex<BlueprintArbiterInstruction>.Init()` to populate its protected `m_Index` through reflection with `Caves_1` → a `BlueprintReference<BlueprintArbiterInstruction>` whose `deserializedGuid` is the GUID above, and skip the original initializer. This avoids writing `arbiter.json`; do **not** call `Register()` because it calls `Dump()`. If the original initializer/index is already empty, replace that in-memory dictionary explicitly as well. Evidence for this proposal: `Assembly-CSharp/Kingmaker.QA/BotInstructionIndex.cs:17`, `:46`, `:108`.

For an offline full-job prototype also prevent `/Instructions` fallback by providing a single instruction; suppress `ArbiterStatusServer` construction and `Arbiter.OnJobFinished`; replace `GeneralProbeData.SendToServer` with `SaveJsonData()` plus successful return, and suppress `CleanupData`. Specify scratch output/report/cache paths before initialization (`-arbiterPlatformDataPath`, `-arbiterReportPath`, `-arbiterReportCache`) or redirect them in memory. The cached report can skip instructions across runs, so use a unique cache path. These are **proposed hooks, not applied patches**. A full area job still loads a new preset and mutates visual/game state; it does not meet the requested save-based benchmark unchanged.

## 4. Concrete proposed UMM mod recipe

### A. Load a fixed save safely

1. Use a save already in `Prologue_Caves_1`, with startup dialogue/cutscenes completed, no combat, and a known world state. Require area GUID `bad465e5404604346bdf71ef50b3299a`; verify actual current part/scene and camera mode after loading. Copy/hash the **save input** into the run manifest rather than selecting “the first filename matching a prefix.” Do not resave it during benchmarks.
2. UMM `Load(ModEntry)` creates a persistent `MonoBehaviour` driver. Detect/capture `Kingmaker.MainMenu` with the existing Harmony `MainMenu.Start` postfix pattern, or find it when ready. On Unity's main thread: `Game.Instance.SaveManager.LoadZipSave(exactPath)` to obtain `SaveInfo`, then **`mainMenu.LoadGame(saveInfo)`** from the menu. If already in a normal loaded game, use `Game.Instance.LoadGame(saveInfo)`.
3. Wait for loading to actually start and finish, then require stable idle plus the expected area, part, rig, camera, and non-cutscene mode. Add a real-time timeout and fail closed: `LoadGame` can refuse a load, e.g. for missing DLC. Avoid treating “never began loading” as a successful load. Record warm/cold cache policy separately.
4. Do not call `Game.LoadGameFromMainMenu` directly as the initial driver entry: the main-menu wrapper shows the loading screen, disposes menu state, and loads obligatory scenes first.

Evidence: `mods/WrathBench/Main.cs:45`, `:77`, `:153`, `:183`; `Assembly-CSharp/Kingmaker/MainMenu.cs:306`, `:387`; `Assembly-CSharp/Kingmaker/Game.cs:2159`. The existing mod is source precedent, not an executed verification in this audit.

### B. Get the authored route without starting Arbiter

The following is a **source-backed implementation sketch, not a compiled/tested mod**:

```csharp
using System;
using Kingmaker;
using Kingmaker.Blueprints;
using Kingmaker.QA.Arbiter;
using Kingmaker.View;

var instruction = ResourcesLibrary.TryGetBlueprint<BlueprintArbiterInstruction>(
    "0c9239f8cff8dd348a3a4251573d9fd6");
var test = instruction?.GetTest<AreaCheckerComponent>();
if (test == null || test.Area.Get() != Game.Instance.CurrentlyLoadedArea)
    throw new InvalidOperationException("Caves instruction/loaded area mismatch");

CameraRig rig = Game.Instance.UI.GetCameraRig();
if (rig == null || rig.Camera == null)
    throw new InvalidOperationException("Game camera is not ready");

rig.FixCamera++;
try
{
    foreach (ArbiterElement element in test.AreaParts.ElementList)
    {
        var part = element as AreaCheckerComponentPart;
        if (part == null)
            throw new InvalidOperationException("Unexpected Arbiter element");
        if (part.EnterPoint.Get().AreaPart != Game.Instance.CurrentlyLoadedAreaPart)
            throw new InvalidOperationException("Unexpected cave part");
        foreach (ArbiterPoint point in part.Points)
        {
            rig.ScrollToImmediately(point.Position);
            rig.RotateToImmediately(point.Rotation);
            rig.CameraZoom.ZoomToImmediate(point.Zoom);
        }
    }
}
finally
{
    rig.FixCamera--;
}
```

**Do not run that loop as-is:** it shows lookup/type/API/order only; without yields it teleports through all eight points in one update. Turn it into a coroutine/state machine with the phases below. Alternatively read `caves-authored-route.json` and construct your own stop DTOs: this is the fallback if runtime instruction loading fails. It needs only `Vector3`, a float yaw, a float zoom, and the same three camera calls, not any QA assets/index.

Evidence: `Assembly-CSharp/Kingmaker.Blueprints/ResourcesLibrary.cs:540`, `Assembly-CSharp/Kingmaker.QA.Arbiter/BlueprintArbiterInstruction.cs:21`, `Assembly-CSharp/Kingmaker.QA.Arbiter/ArbiterClientIntegration.cs:293`.

### C. Establish camera ownership; bake and then replay resolved poses

**Proposed implementation:**

- Save initial pause/VSync/frame-cap/UI/FoW states and every component/object state you actually change. Keep an ownership record of the exact rig instance and acquired guard; release it once on completion, abort, destruction, or scene transition. Do not restore a raw guard count over other owners' changes.
- Use `FixCamera++`, optionally release the unit follower, and refuse any active cutscene/director camera takeover. Cancel existing rig scroll/rotate/zoom routines via the immediate calls. Kill outstanding DOTween tweens targeting this rig transform if present (`transform.DOKill()` with `DG.Tweening`), or wait for them to finish before taking ownership. `FixCamera` does **not** cancel a tween already running independently.
- Use one fixed initial camera orientation/state after loading. For the one-time bake, perform the **same order as Arbiter: scroll → yaw → zoom**. This matters: ground projection uses the **previous** camera forward direction, so changing to yaw → scroll changes semantics. Restore the same initial state before rebaking or starting an independent pass. Screenshot-pass point zero and measurement-pass point zero can resolve differently if their prior orientation differs.
- After moving, wait at least until the camera has rendered. Capture pose/projection from `RenderPipelineManager.beginCameraRendering` filtered to `rig.Camera`; confirm it is unchanged at `endCameraRendering` and matches the screenshot frame. This pipeline explicitly emits those camera events. If internal render hooks change matrices, sample at the later relevant point too.
- Save requested point fields **and** resolved rig world position/quaternion, camera world position/quaternion, camera/parent local transforms, `fieldOfView`, projection and view matrices. Do not use only `rig.GetPosition()`; it returns the target, not necessarily the optical camera's current transform.
- For subsequent exact-camera benchmark runs, bypass `PlaceOnGround` and smoothing entirely: reapply the **baked optical world pose** and projection/intrinsics before each game-camera render. A scoped Harmony prefix can skip `CameraRig.Update` for the owned rig during replay, since that method otherwise interpolates toward its private target. A scoped render callback can enforce/verify the camera pose after other animation updates. Keep baked rig pose as well for consumers that depend on the rig, update or restore private targets if needed on release, and do not call `UpdateForce()` expecting it to be passive. If skipping its update, maintain `m_WorldToClipMatrixCached = camera.projectionMatrix * camera.worldToCameraMatrix` through a scoped reflection accessor, because `CameraRig.WorldToViewport` otherwise uses a stale cached matrix (`Assembly-CSharp/Kingmaker.View/CameraRig.cs:276`, `:615`).
- Assert the resolved pose/projection remain equal within a documented float tolerance throughout each sample. Abort on a rig replacement, loading, part change, mode change, controller takeover, or unexpected movement. Use reload/reset to restore the original camera state; do not assume assigning an old transform also restores private targets/routines.

Evidence for these guard caveats: `Assembly-CSharp/Kingmaker.Controllers.Rest/CameraController.cs:141`, `Assembly-CSharp/Kingmaker.View/CameraRig.cs:547`, `:605`, `:1081`, `:1131`, `:1342`, `:1386`, `:297`; render hooks: `Owlcat.Runtime.Visual/Owlcat.Runtime.Visual.RenderPipeline/OwlcatRenderPipeline.cs:57`.

### D. Two explicit measurement profiles

**Static shot/frame-time profile (simplest and recommended first):** load a fixed save, pause simulation, use baked poses, acquire camera ownership, turn off VSync and any imposed frame cap consistently, and select exactly which UI/FoW/FX/units remain visible. Measure the same rendering workload at each point, not Arbiter's screenshot profile followed by a different live profiling profile. Paused measurements do not represent active gameplay CPU cost.

Suggested per-stop schedule: **one boundary frame excluded → 15 warmup rendered frames → 120 measured rendered frames → optional screenshot frame excluded**, teleporting only between stops. Store integer relative render-frame indices and explicit phases; optionally assign a **virtual** 60-Hz timeline (`time = logicalFrame / 60`) for replay in another engine. Virtual time is not measured wall time. Do not change Unity `Time.captureFramerate`/global time to manufacture frame durations while measuring performance. The 15/120 choice mirrors Arbiter's sampling budgets, not its entire mixed-second timing sequence.

For actual frame timing use the existing `WrathStats` recorder's `Stopwatch` interval between `LateUpdate`s, or the same monotonic interval technique in a preallocated recorder. Export raw frame milliseconds, mean/p50/p95/p99/max and `N / sum(frameSeconds)` throughput. Tag boundary/settle/capture frames and correlate records to exact sample IDs/frame numbers. These are **CPU-side frame cadence/whole-loop intervals**, not GPU durations. Use platform GPU timing only if available and separately validated.

Existing-source integration: `mods/WrathStats/Core/Recorder.cs:131` records monotonic intervals to `frames.bin`, while `mods/WrathStats/WrathStatsApi.cs:20` provides `BeginWork`, `EndWork`, and `Note`. Add a note once per phase/point and surround screenshot/export work with Begin/EndWork, not each frame. The recorder's external flags can mark work-contaminated frames; retain an explicit phase map so adjacent asynchronous screenshot work is also excluded.

**Live/animated profile:** explicitly keep simulation active, restore the same input save and party state each run, and record simulation time plus camera schedule. Matching camera stops alone does **not** make gameplay deterministic. To demand identical animated shots across engines, also define/reset random seeds, animation/particle/weather state, lights, and all time-dependent rendering. Game pause alone is insufficient proof: Owlcat rendering passes scaled **and unscaled** time to shaders. A logical-time source or scoped shader-time interception is a separate, untested extension, not a feature Arbiter already provides. Evidence: `Assembly-CSharp/Kingmaker.Controllers/TimeController.cs:114`, `Owlcat.Runtime.Visual/Owlcat.Runtime.Visual.RenderPipeline/ScriptableRenderer.cs:83`, `:281`.

**Timing contract:** you cannot promise both the same number of measured rendered frames and the same elapsed wall-clock dwell when comparing engines of different speed. Pick and serialize a policy: fixed frame budgets for static benchmarks, or an explicit logical simulation-time trajectory for moving shots. For a wall-clock route, publish actual timestamps/dropped samples and accept hardware-dependent camera sampling. Arbiter's mixed waits do not solve that conflict.

### E. Screenshots at the actual benchmark camera points

Two source-supported choices:

1. **Arbiter-style camera-only PNG:** `File.WriteAllBytes(path, Kingmaker.Utility.Screenshot.CapturePNG(rig.Camera, width, height))`. Does not require `Arbiter.Instance` or the root. Its implementation allocates a temporary 24-bit-depth RenderTexture, calls `camera.Render()`, reads RGB24 pixels, and encodes PNG synchronously. Match screenshot dimensions/aspect to the benchmark viewport and capture the camera/projection used for that offscreen render too. This is an **extra render/readback/encode**, so never include it in the measured 120 frames. Pipeline compatibility and image appearance are not runtime-verified here. Evidence: `Assembly-CSharp/Kingmaker.Utility/Screenshot.cs:54`.
2. **Normal rendered-frame screenshot:** `ScreenCapture.CaptureScreenshot(path)` after settling at the point and closing UMM UI, as WrathBench already does. Schedule it for a known target frame; retain the pose sampled from that frame's render callback and verify the file is complete rather than assuming one yielded frame means disk persistence. This captures the normal output/UI composition, not necessarily only the game camera. Evidence: `mods/WrathBench/Main.cs:200`.

For strict shot matching, prefer capture of the ordinary rendered output at a fixed viewport, with unwanted UI explicitly hidden and restored. Use a render-end/readback approach if you need a synchronous exact-frame artifact, but validate it against the normal frame first. Offscreen target changes can change the projection/aspect and render history; do not label a pre-offscreen matrix as necessarily the matrix of the saved PNG.

Do not blindly reuse `HideUnits`/`DisableFx` and their inverse helpers; store references to changed objects and their original active/enabled values, then restore those exact values. The built-in paired helpers are not reliable inverses in this shipped code.

### F. Route export contract for another engine

`caves-authored-route.json` is deliberately labelled **authored inputs only; not resolved camera poses**. A future in-game bake should export something like:

```json
{
  "schema": "wrath-camera-replay-v1",
  "coordinates": {
    "space": "Unity world",
    "up": "+Y",
    "transform_forward": "+Z",
    "quaternion_order": "xyzw",
    "matrix_layout": "row-major array from matrix[row,column]"
  },
  "area_guid": "bad465e5404604346bdf71ef50b3299a",
  "instruction_guid": "0c9239f8cff8dd348a3a4251573d9fd6",
  "timing": {
    "policy": "fixed-render-frame-budgets",
    "transition": "step",
    "boundary_frames": 1,
    "warmup_frames": 15,
    "sample_frames": 120,
    "capture_frames_excluded": true,
    "logical_hz": 60
  },
  "points": [],
  "render_frames": []
}
```

The empty arrays above mean **not captured**, not a complete replay file. Populate each point with requested inputs, resolved rig/camera world pose, FOV/zoom settings, viewport/clip planes, view/projection matrices, screenshot filename/hash and phase indices. Populate per-render samples if the camera or projection is not strictly static. Include save hash, game/Unity/build versions, scene identities, game time, graphics settings/render scale/quality, VSync/frame cap, mods/versions, UI/FoW/FX/units profile, time/seed policy, and repeat number.

Export float values with round-trip precision (or their IEEE-754 bit patterns), not `Vector3.ToString()`'s rounded display. Use plain arrays/scalar DTOs rather than blindly serializing Unity transforms. For matrices explicitly iterate `[row,column]`; do not rely on Unity's one-dimensional indexer to mean row-major. Include both raw authored values and resolved values so a reimplementation never mistakes a rig pivot for an eye position.

**Replay:** map world coordinates/units to the other engine once; use the baked eye pose and view/projection, not the ground target and a guessed isometric angle. Translate handedness, camera-space forward axis, clip-depth range, reversed-Z conventions and framebuffer Y orientation to the target API. If coordinate conversion is `x_other = C * x_unity`, use `V_other = V_unity * inverse(C)` when preserving Unity camera coordinates, followed by whatever camera/clip-space conversion the target renderer requires. Validate several known world landmarks projected to the same pixel coordinates before comparing images. Step at the same logical indices; do not interpolate between teleports unless you explicitly create a new route.

This is a proposed replay contract. Static inputs can define matching views; exact pixels additionally require matching scene assets, renderer/material/light/postprocess behavior and temporal state. A route alone cannot make a second engine render identically.

## 5. Camera definition and exact-shot parameters

### Rig versus optical camera — verified-static

`Kingmaker.View.CameraRig` owns a child `UnityEngine.Camera` and a `CameraZoom`. Serialized attach points exist for default/global-map/tactical/settlement/photo modes. On enabling, the rig discovers the child camera and zoom component and registers itself with `Game.Instance.UI`. Default mode parents the optical camera under `m_CameraAttachPoint`, sets FOV to `FovMax`, and zeros the camera's local position/rotation. The parent attach point supplies the elevation, offset, pitch and any other baked orientation.

Evidence: `Assembly-CSharp/Kingmaker.View/CameraRig.cs:38`, `:194`, `:297`, `:374`. The cave's `IsGlobalMap=false` selects `CameraMode.Default`; area camera-mode selection is at `Assembly-CSharp/Kingmaker.Blueprints.Area/BlueprintArea.cs:277`.

Therefore the idealized optical transform is a composition of rig, attach-point, and camera-local transforms, **not** merely `(point.Position, Euler(0, point.Rotation, 0))`. Camera shake can alter rig and camera offsets and `LookAt` the rig. Pitch/offset/FOV limits are serialized Unity scene/prefab values, not established numerically by these decompiled field declarations. **This audit did not deserialize camera prefabs or execute a runtime snapshot, so it does not assert an exact pitch, eye height/distance, FovMin/FovMax, or clip-plane constant.** Read them from the active rig/camera for the baked route. Evidence: `Assembly-CSharp/Kingmaker.View/CameraRig.cs:832`, `:1391`, `Assembly-CSharp/Kingmaker.View/CameraZoom.cs:9`.

### Position, rotation, zoom, and clamps

| Parameter/API | Shipped semantics | Evidence |
|---|---|---|
| `ScrollToImmediately(Vector3)` | Stops scroll coroutine; runs `PlaceOnGround`; assigns both rig world position and private target; clears cached area part. **Does not call `ClampByLevelBounds`.** “Immediate” does not mean “unmodified input.” | `Assembly-CSharp/Kingmaker.View/CameraRig.cs:1342` |
| `RotateToImmediately(float)` | Stops rotate coroutine if any; replaces **yaw** of private target Euler vector, preserving its x/z; writes rig quaternion. No normalization/clamp here and no explicit DOTween kill. | `Assembly-CSharp/Kingmaker.View/CameraRig.cs:1351` |
| `ZoomToImmediate(float)` | Stops zoom coroutine; assigns raw player/current/smoothed scroll position. `CurrentNormalizePosition = smooth / m_ZoomLenght`; FOV = `Mathf.Lerp(FovMax, FovMin, normalized)`. No raw-position clamp in the immediate setter. | `Assembly-CSharp/Kingmaker.View/CameraZoom.cs:48`, `:92` |
| Normal zoom tick | Clamps raw scroll into `[0,m_ZoomLenght]`, smooths with unscaled delta × `m_Smooth` (field default 5), then applies FOV interpolation. Zoom changes FOV, not an explicit dolly in this class. | `Assembly-CSharp/Kingmaker.View/CameraZoom.cs:76` |
| All Caves `Zoom=0` | Selects `FovMax`, i.e. wide endpoint. It is not a zero-degree field of view. | Caves instruction JSON `:36`; `Assembly-CSharp/Kingmaker.View/CameraZoom.cs:99` |
| `FixCamera` | Counting guard; blocks controller follow/scroll/rotate/zoom/shake ticks. **Does not block `CameraRig.UpdateInternal()`**, externally invoked camera operations, director logic, or existing tweens. Changing camera mode resets the guard. | `Assembly-CSharp/Kingmaker.Controllers.Rest/CameraController.cs:141`, `Assembly-CSharp/Kingmaker.View/CameraRig.cs:204`, `:297`, `:547` |
| Normal rig smoothing | Smoothed unscaled delta = `Lerp(previous,dt,0.2)`, capped at 0.5, multiplied by debug timescale clamped 1..4; rig position lerps toward target minus anchor shake at that delta × rubber-band coefficient (field default 4, rest 3). | `Assembly-CSharp/Kingmaker.View/CameraRig.cs:605` |
| Normal scroll clamp | Area bounds' X/Z rectangle, then polygon/block validation. Fallback bounds aggregate layer-8 collider bounds starting at default `Bounds`. `NoClamp` skips the normal scroll clamp, **not** the immediate setter's ground raycast. | `Assembly-CSharp/Kingmaker.View/CameraRig.cs:260`, `:619`, `:701` |
| Area bounds selection | Uses cutscene override if active; else explicit camera override or default bounds. Polygon clamps to closest XZ point; block bounds can return the last valid rig position. | `Assembly-CSharp/Kingmaker.Blueprints.Area/AreaPartBounds.cs:229`, `:378` |

`PlaceOnGround` algorithm, precisely enough to understand/bypass rather than invent a navmesh route:

- Physics mask **4194576 = bits 4, 8, 22**, with a reusable **16-hit** buffer. Grounding uses physics colliders, not navmesh nodes.
- With `NewBehaviour=false`, let `f` be the current optical camera world forward direction. Cast from `p - 100*f` along `f` for 200 units. No hits → original p; one hit → hit point; multiple hits → selection prioritizes layer-22 objects except those tagged `NonPrioritizedCameraOnlyLayer`, then the smallest squared projection distance along f from `p - 0.25*f`. This can change **x and z as well as y**. Follow the actual comparison order if emulating its priority/tie behavior, including that NonAlloc hit order is not explicitly sorted.
- With `NewBehaviour=true`, cast down from `p + (0,100,0)` for 200 units. Zero/one cases are identical; multiple hits use priority-object rules and greatest hit y.
- No deterministic tie-breaking across collider enumeration is added here. The prior camera direction affects the old-behavior result, and the fixed hit buffer can limit observed intersections. For exact replay, exporting the resolved optical pose is safer than reimplementing this physics query in another engine.

Evidence: `Assembly-CSharp/Kingmaker.View/CameraRig.cs:36`, `:118`, `:1131`, `:1171`, `:1202`. The source's priority-condition implementation should be treated as authoritative rather than simplifying it into a generic nearest-ground query.

### Shot-matching checklist

**Capture at render time:**

- Rig world pose, current target, camera-mode/part, `NewBehaviour` flags, `NoClamp`, guard state and resolved ground result.
- Optical camera world position and full quaternion (`x,y,z,w`), transform parent-chain local poses/scales. Export its `worldToCameraMatrix` as the authoritative view, not just a yaw.
- Actual FOV, `CameraZoom.FovMin/FovMax/m_ZoomLenght`, raw requested zoom and normalized current zoom. Unity's `fieldOfView` is vertical degrees; horizontal FOV depends on aspect. See official Unity 2020.3 `Camera.fieldOfView` documentation; retain actual projection as the stronger contract.
- Perspective/orthographic mode (and orthographic size if relevant), actual aspect, pixel viewport/rect, resolution, near/far planes, physical-camera settings/lens shift if used, projection/non-jittered projection, culling mask and per-layer distance/LOD/occlusion settings.
- Logical view/projection versus API-adjusted GPU projection, render-to-texture flag, render scale, output color space/HDR/MSAA/postprocessing, and temporal/frame state if present. Owlcat uses `GL.GetGPUProjectionMatrix(..., renderIntoTexture:true)` in its camera shader constants: don't blindly substitute that for a target engine's own clip convention. Evidence: `Owlcat.Runtime.Visual/Owlcat.Runtime.Visual.RenderPipeline/OwlcatRenderPipeline.cs:226`.
- Time of day/weather/scene lights/FoW/units/effects/UI and rendering-time inputs. Project known landmarks and compare images only after matching those conditions.

**Numeric camera constants still needing an in-game capture:** attach-point local offset/rotation, actual eye pose, FOV endpoints/current FOV, aspect/near/far, bounds/polygon/block asset contents, projection/culling settings, and any dynamic camera modifications. They are not present in the eight-point JSON. Claiming, for example, “pitch is exactly 45°” or “FOV is exactly 60°” from the route data would be unsupported.

## 6. Verification and next smoke test

### Completed here

- Read decompiled Arbiter, camera, save/main-menu, counter and rendering pipeline code, plus WrathBench/WrathStats source.
- Scanned the full ZIP's `.jbp` population for target/type references; extracted six relevant blueprint files byte-for-byte; recorded GUIDs/hashes and all eight inputs.
- Inventoried Arbiter scene bundle presence and missing StreamingAssets index. No launch, endpoint access, save load, screenshot capture, or installation changes.
- All generated files are in this scratch directory. The game, decompiled checkout, benchmark mods and saves remain untouched.

### Runtime acceptance test for a future mod — proposed, not performed

1. Resolve the instruction/root/area GUIDs in-game; log exact type, point count, scene/part and camera identity. Fall back to authored JSON if the instruction cache entry is missing.
2. Load the exact benchmark save through the correct menu path; verify the expected cave GUID, no loading/cutscene, camera mode Default, and graphics profile.
3. Bake the eight resolved poses and capture points 0/7; explicitly compare requested positions against actual rig/eye positions to detect grounding/hierarchy differences.
4. Reload and replay the baked poses twice; verify every game-camera render's pose/projection matches, screenshot frame IDs are aligned, output persists, and phase timing/counts agree. Use tolerances declared in the manifest, not an unqualified bit-identical promise.
5. Only then collect frame-time runs, with screenshot/export frames excluded and cold/warm cache policy fixed. Validate another engine by landmark projection and matching dimensions before performance/image comparisons.

**Bottom line:** the useful shipped asset is the eight-point **Caves_1 instruction**, and the useful shipped mechanism is **CameraRig's three immediate camera operations plus a render-time pose export**. Full Arbiter is a QA automation/server harness with stripped features and state/output hazards, not an already deterministic portable benchmark system.
