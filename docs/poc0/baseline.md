# POC-0 baseline: the original game on the matched workload

> Measured 2026-10-10 by Claude on Ember's Mac Studio (M2 Max, 32 GB, 2560×1440 at 240 Hz,
> macOS 27) with the native arm64 build of the original game (`Wrath-Mac/builds/arm64`,
> incremental GC on). Raw runs are local only (paths below); this page holds summaries
> without game text. Status labels: **[M]** measured, **[I]** inferred.

**Headline [M]** (median and range over three clean runs per configuration; "game" = the
game's own behaviour, "best" = WrathPerf on; details in §3):

| Metric | game | best |
|---|---|---|
| Area load, process-cold (first load in a fresh process) | 10.27 s (9.94–10.80) | **6.00 s** (5.95–6.20) |
| Area load, warm (reload in the same process, 9 per config) | 9.07 s (8.86–9.32) | **5.00 s** (4.70–5.20) |
| Area load, OS-cold (shader cache cleared; 1 run each) | 12.24 s | 7.77 s |
| Route frame time p50 / p95 / p99 (2 × 5 min per run) | 12.45 / 17.0 / 21.1 ms | the same |
| Route frames ≥ 33 ms / ≥ 50 ms, shader cache warm (per run, 2 × 5 min) | 0–1 / 0 (max 42 ms) | 0 / 0 |
| Route frames ≥ 50 ms, shader cache cleared, first pass (1 run) | 6 (max 355 ms) | 7 (max 371 ms) |
| Manual save: `Game.SaveGame` to the save routine finishing (the full blocking path) | 0.465 s (0.456–0.473) | 0.469 s (0.460–0.490) |
| … of which: `.zks` completely written (`ZipSaver.Save` returned on the worker) | 0.430 s (0.423–0.440) | 0.433 s (0.427–0.450) |
| Frames during the save / of them ≥ 50 ms / longest | 9 / 3 / 239–253 ms | 9 / 3 / 240–248 ms |
| Resident memory after first load → end of run | 3.50 → 3.65 GB | 3.50 → 3.64 GB |
| Mono heap after first load → end | 0.46 → 0.50 GB | the same |
| Perceptual noise floor, original vs itself (FLIP mean per point) | 0.021–0.074 (cross-process 0.026–0.057) | |

Reading: with VSync on at 240 Hz the frame intervals cluster at multiples of 4.17 ms —
p50 at three (12.5 ms, 80 fps) and p99 at five — so the original looks **vsync-quantised**
[I: inferred from the interval distribution; presentation timing wasn't measured]. With
a warm shader cache it has **0–1 frames ≥ 33 ms per 10 minutes of route**. Its weak
points on this workload are loads (6–12 s), stalls after the shader cache is cleared, and
the save (three frames ≥ 50 ms, one ~250 ms), not steady-state frame time. The named targets in PLAN.md are relative to these numbers; see §6 for what
that implies.


## 1. The matched workload

This is the definition POC-6 compares against. Anything not listed here is "as the game
does it".

| Item | Definition |
|---|---|
| Build | `Wrath-Mac/builds/arm64` (Steam build 20212986, game 2.7.0y.782, Unity 2020.3.48f1 arm64 player with the CVE-2025-59489 patch, Harmony 2.4.2), `boot.config` `gc-max-time-slice=3` (incremental GC). Mods: UMM 0.32.4 with ToyBox, BuffIt2TheLimit, WrathStats 0.2.5, WrathBench 0.1.1 (idle), WrathPerf 0.1.0, WrathOracle 0.1.0. Exact hashes per run in `identity.json`. |
| Save | `Manual_13_Underground_Caves_of_Kenabres__15_Arodus__VIII__4715__12_03_41.zks` (Ember's; read-only backup in `fixtures/saves/`; SHA-256 in each run's `identity.json`). Area `Prologue_Caves_1` (`bad465e5404604346bdf71ef50b3299a`); party FennA alone; game mode Default; not paused. |
| Settings | Ember's `general_settings.json` as is (copied into every run): 2560×1440 **FullScreenWindow**, **VSync on** (`vSyncCount` 1 at 240 Hz), quality **High**, textures High, shadows High, SSAO High, SSR High, SMAA High, depth of field and bloom on. Difficulty Normal / stats Decline / encounters Standard (full list in `identity.json`). |
| Configurations | **game**: WrathPerf native DXT off, cleanup wait 100 frames (the game's own behaviour). **best**: native DXT on, cleanup wait 10 frames (as in `soak-both`). Everything else identical. |
| Camera route | The `Caves_1` Arbiter instruction (`0c9239f8cff8dd348a3a4251573d9fd6`), resolved **in game by GUID** (it matched the authored copy exactly), eight points in authored order. Each move is `CameraRig.ScrollToImmediately(pos)`, `RotateToImmediately(yaw)`, `CameraZoom.ZoomToImmediate(zoom)` (Arbiter's order: grounding uses the current camera forward). Step transitions, no interpolation. |
| Route profile | Live simulation (not paused); UI hidden (`StaticCanvas`/`DynamicCanvas` inactive and every enabled screen-space root `Canvas` disabled, restored afterwards); **fog of war off** (`FogOfWarArea.Active.enabled = false` for the route, as Arbiter's QA passes do; most of the cave is unexplored in this save, so with FoW on most points would be black); units, effects, weather and lights as loaded. `FixCamera` held for the route so the player camera controller can't move it. |
| Schedule (bench) | Per point: move, then a **37.5 s wall-clock dwell** recording every frame; 8 points = 300 s. Frame times are intervals between consecutive `LateUpdate`s (as WrathStats measures them); a point owns the intervals from the one ending in the move frame's `LateUpdate` up to, not including, the next move's. Stats over all of them, and again without the first 15 (`frames_after_warmup`). |
| Schedule (shots) | Per point: move, wait 5 s, capture at end of frame (`ScreenCapture.CaptureScreenshotAsTexture` after `WaitForEndOfFrame`; the final back buffer at 2560×1440), with the resolved camera pose from that same frame. |
| Replay export | `route-pass1.json` (schema `wrath-camera-replay-v1`) in each run: per point the requested rig target/yaw/zoom, resolved rig pose, the camera's parent chain, and the **render camera's world position, rotation (xyzw), vertical FOV, near/far, aspect, pixel rect, `worldToCameraMatrix`, projection, non-jittered projection and GPU projection**, captured in `RenderPipelineManager.beginCameraRendering` for the game camera. Floats are written round-trip (`"R"`), so they parse back to the exact float32 values. |
| Load boundary | From the frame the load is requested (`MainMenu.LoadGame` for the first load, `Game.LoadGame` for reloads) to the first frame in which `LoadingProcess.IsLoadingInProcess` is false again, provided it stays false for 1 s (otherwise the chain continues). WrathStats' load span (busy→idle) is reported alongside; the two differ by the 0–6 frames before the loading process starts. |
| Cache states | **Process-cold**: the first load in a fresh process (OS file cache and macOS Metal shader cache warm). **Warm**: later reloads in the same process. **OS-cold**: `tools/metal-cache.sh clear` before launch (macOS's per-app Metal shader cache moved aside; restored afterwards). The OS file cache can't be purged without root, so OS-cold here means shader-cold only. |
| Background | Not controllable: OBS and a camera service (UVCAssistant) were running throughout, as in Ember's earlier baselines. Each bench run waited for 60 s of quiet (no render spike, compiler or AssetRipper; nothing else above 150% CPU), was sampled every 5 s during the run, and was discarded and repeated if anything ran (§3, methodology). Each clean run has an empty `contention.txt`. |

## 2. WrathOracle 0.1

`oracle/mods/WrathOracle` (UMM/Harmony, built like WrathStats via
`mods/Directory.Build.props`). **Idle without a plan**: it reads
`<persistent data>/WrathOracle/plan.json` at startup and deletes it at once (one-shot,
so Ember's next normal launch is unaffected). With a plan it waits for the main menu,
loads the save through the menu's own `MainMenu.LoadGame`, and runs the plan's steps on
the main thread: `load`, `reload`, `settle:N`, `mem:LABEL`, `identity`, `dump`, `nav`,
`route`, `save`, `quit`. It patches three methods: postfixes on `MainMenu.Start` (to find the menu) and `ZipSaver.Save` (save commit time), and a guard prefix on `SaveManager.SaveRoutine` that, while a plan runs, skips every save except the oracle's own registered bench slot (so a scripted autosave during a live route can't touch Ember's slots). The only game state it changes is temporary and restored (UI canvases, FoW component, its own camera-guard acquisition) plus the camera position itself, and the timed save's slot, which it deletes once the save's worker has finished. The plan file is consumed by an atomic rename before it is parsed; if that fails, nothing runs. Dumps are taken between frames; none is timed. `run.json` is rewritten after every step.

**Outputs** (one folder per run; all game data, so only under gitignored paths):

| File | Contents (exact field names are in the files; schema strings in brackets) |
|---|---|
| `identity.json` | Build identity: oracle version, game version and revision, Unity version, `BUILD_INFO`, `boot.config`, `Version.info`, `startup.json`; SHA-256 of the key managed assemblies, `UnityPlayer.dylib`, `blueprints-pack.bbp`, `blueprints.zip` and the save; mods with versions and DLL hashes; every difficulty and graphics setting by key; display (resolution, refresh, full-screen mode, vsync, target frame rate, quality level, shadow distance, LOD bias, GPU, API, incremental GC); the plan. |
| `scene-dump.json` [`wrath-oracle-scene-dump-v1`] | `frame`, `random_state_before/after` (UnityEngine.Random's four state words: the dump consumes no draws); `area` (area and part GUIDs, part scenes, light/audio scene, time of day, game time, mode); `scenes` (every loaded scene: name, root names, object count, component counts by type); `loaded_bundles`; **`views`**: every `EntityViewBase` in the loaded scenes (class, `unique_id`, scene, `path`, `sibling_path`, activity, layer, tag, local and world transform with `local_to_world`, attached data's ID/type/`IsInGame`/`IsInState`/`IsRevealed`, and every component with its Unity-serialized fields by their serialized names, blueprint references as `{"$bp": guid}`, object references as `{"$ref", scene, path}`); **`entities`**: every `EntityDataBase` in `State.Entities` and the loaded area state (ID, type, blueprint GUID/name, holding scene state, in-game/in-state/revealed/turned-on, view path and pose; units: position, orientation, name, faction, corpulence, dead, in party; placeholders: real unit ID and position; spawner data: has spawned/died, spawned unit); **`spawners`** (ID, blueprint, `spawn_on_scene_init`, `has_spawned`, `has_died`, spawned unit ID/blueprint/position/orientation, owning group, settings components with fields); `unit_groups` (ID, member spawner IDs); **`script_zones`** (ID, blueprint, `is_active`, `once_only`, `players_only`, and every shape in hierarchy order: type, path, activity, whether and where the zone collected it, serialized fields, `local_to_world`, `world_center`, `world_bounds`, a fixed 5×3×5 probe lattice over the bounds +1 m evaluated with the game's own `Contains`, and the extraction tool's 38 probes per shape from `inventory.json` evaluated with `Contains(point, corpulence)` and `ScriptZone.ContainsPosition`); `locators`; `area_transitions` (with the `AreaTransition` component's settings); `area_enter_points` (blueprint, pose); `cutscene_players`; `map_objects` (with their runtime part types); `navmesh_cuts`; `health`. |
| `hierarchy/<scene>.json` [`wrath-oracle-hierarchy-v1`] | Every GameObject of every loaded scene: name, `path`, `root_sibling_index`, `sibling_path`, `active_self`, `active_in_hierarchy`, layer, static, local TRS, world position/rotation/lossy scale, `local_to_world`, components as `[type, enabled]`, mesh name, renderer materials and shaders. |
| `nav-graph.json` [`wrath-oracle-nav-graph-v1`] | `astar` settings (heuristic, scale, max nearest distance, threads); per graph its public settings and, for the RecastGraph, every tile (`verts` in Int3 world space, `tris`, nodes with vertex indices, raw flags, tag, walkable, penalty and connections as `[tile, index in tile, cost, shape edge]`); for the GridGraph, width/depth/node size/centre/rotation/erosion, neighbour costs and offsets, and base64 arrays of node positions (Int3), raw flags, grid flags, `DynamicBlocked`/`AllyBlocked` and effective walkability. Pending graph updates are flushed first. |
| `nav-queries.json` [`wrath-oracle-nav-queries-v1`] | Twelve endpoints (party, the two meetings' trigger zones and the nine encounter groups of `fights.md`, resolved in game by unique ID) with nearest nodes on both graphs; the eight route points' nearest nodes; 16 endpoint pairs **and the extraction tool's 30 `nav.json` queries**, each with `navmesh_raw` (ABPath, no modifiers: the path before modifiers), `navmesh_agent` (a `SeekerWithClearance` configured exactly as `UnitMovementAgent.Init`, graph mask 1, as real-time movement requests it), `grid_raw`, and `tb_clearance_0.5` / `tb_clearance_1.0` (`ABPathWithClearance`, mask 2, a turn-based-configured seeker, `ConstructTags`' tag mask). Per path: state, error, duration, searched nodes, **end node G**, start/end points and nodes, node keys (`[graph, tile, index in tile]` for the navmesh, `[graph, 0, NodeInGridIndex]` for the grid), per-step connection costs and their sum, the corner points and length. `random_state_before/after`; `graph_changed_during_queries`. |
| `route-passN.json` [`wrath-camera-replay-v1`] | §1's replay export; plus per point pose stability over the settle/dwell, and either the screenshot (path, SHA-256, frame) or the frame-time statistics and raw frame times (base64 uint32 µs). |
| `shots/passN-pointM.png` | 2560×1440 screenshots (shots mode). |
| `run.json` [`wrath-oracle-run-v1`] | The steps with timings: loads (`call_to_busy_s`, `busy_to_idle_s`, `call_to_idle_s`, restarts, frames, max frame, area), memory samples (resident and peak resident from `mach_task_basic_info`, Mono used/heap, GC total, Unity allocated/reserved, graphics driver, GC count), the timed save, errors, health. |

**Health counters** (in every file and `run.json`): `records_written`, `records_failed`
(a record whose capture threw is counted, logged and replaced by `{"$failed": ...}`; it
can't leave half-written JSON because each record is built in memory first), and
`records_dropped` (frames beyond the preallocated frame buffer, 262,144 frames).

**First results against the extraction spike [M]:** all 646 of the extractor's
script-zone probes give the same inside/outside answer as the game's `Contains`; the
RNG state is unchanged across dumps and queries; the live graphs have the same sizes as
the decoded cache (RecastGraph 1,061 triangles in 28×26 tiles; GridGraph 280×260); the
first nav query already shows the expected differences (game end-node G 15,016 vs the
extractor's cost 14,956, which omits endpoint costs; one grid node differs along the
path). Diffing everything is the extraction side's next step.

## 3. Baseline results [M]

Raw runs: `fixtures/captures/runs-raw/baseline/{game,best}-{1,2,3}`, `{game,best}-oscold`
(summaries: `docs/baseline-runs/poc0-baseline/`). Each run is one fresh launch with the bench
plan (§5). All runs: WrathOracle health 0 failed / 0 dropped, plan finished, bench slot
deleted.

**Loads** (call to idle boundary, §1; WrathStats' busy-to-idle span is 0.15 s shorter on
the first load and equal on reloads):

| | game-1 | game-2 | game-3 | best-1 | best-2 | best-3 | game-oscold | best-oscold |
|---|---|---|---|---|---|---|---|---|
| First load | 10.80 | 10.27 | 9.94 | 6.20 | 5.95 | 6.00 | 12.24 | 7.77 |
| Reload 1 / 2 / 3 | 9.07 / 8.97 / 9.32 | 8.93 / 8.86 / 9.16 | 9.14 / 8.89 / 9.20 | 5.09 / 4.70 / 4.87 | 5.06 / 5.00 / 5.20 | 4.97 / 5.05 / 4.95 | 9.34 / 8.97 / 9.26 | 5.16 / 4.80 / 5.03 |

Reload 1 follows the first 5-minute route, reload 2 the second, reload 3 a 5 s settle;
they don't differ. With the shader cache cleared the first load took about 2 s longer
(one run per configuration, so not a stable estimate).

**Route frame times** (all frames of the 8 × 37.5 s dwell; pass 1 after the first load,
pass 2 after reload 1):

| | frames per pass | fps | p50 | p95 | p99 | max | ≥ 33 ms | ≥ 50 ms | GC frames |
|---|---|---|---|---|---|---|---|---|---|
| game, warm, pass 1 (3 runs) | 24,199–24,208 | 80.7 | 12.45 | 17.0 | 21.1 | 26–42 | 0–1 | 0 | 4 |
| game, warm, pass 2 | 24,136–24,180 | 80.4–80.6 | 12.46 | 17.0 | 21.0–21.1 | 22–23 | 0 | 0 | 2 |
| best, warm, pass 1 | 24,199–24,231 | 80.6–80.8 | 12.45 | 17.0 | 21.1 | 23–26 | 0 | 0 | 3 |
| best, warm, pass 2 | 24,149–24,190 | 80.5–80.6 | 12.45 | 17.0 | 21.0–21.1 | 27–28 | 0 | 0 | 2 |
| game, OS-cold, pass 1 / 2 | 24,132 / 24,142 | 80.4 / 80.5 | 12.45 | 17.0 | 21.1 | 355 / 169 | 6 / 2 | 6 / 2 | 3 / 2 |
| best, OS-cold, pass 1 / 2 | 24,115 / 24,143 | 80.4 / 80.5 | 12.45 | 17.0 | 21.0 | 371 / 94 | 7 / 1 | 7 / 1 | 3 / 2 |

The two configurations differ only in load-time work, so their route numbers match. Per
point the frame rate varies (e.g. 2,673–3,352 frames per 37.5 s dwell in the OS-cold
run). In the cache-cleared runs the stalls fall at points 1–5 of the first pass and are
mostly gone after the reload, consistent with first-use shader compilation (not
instrumented, so not attributed individually).

**Memory** at the plan's fixed points (resident / Mono heap, GB, medians over the six
warm runs; game and best agree within 0.03 GB): after first load 3.50 / 0.46; after route 1 3.52 / 0.46; after
reload 3.59 / 0.49; after route 2 3.63 / 0.52; before save 3.64 / 0.51; after save
3.65 / 0.50. Peak resident (`resident_max`) and Unity/graphics-driver counters are in
`run.json`. Resident memory grows about 0.15 GB over the run, mostly across the first
reload.

**Manual save:** the `.zks` is completely written 0.423–0.450 s after `Game.SaveGame`
(`ZipSaver.Save` returning on the worker); the save routine, which blocks the game behind
the loading process and polls the worker every 0.15 s, finishes 0.03–0.04 s later, at
0.456–0.490 s (the "total"). The save spans 9 frames, 3 of them ≥ 50 ms, the longest
239–253 ms. The slot was `Manual_18_WR_Bench_1.zks` every time (the next free manual
number), deleted through `SaveManager.DeleteSave` afterwards. (These runs waited for the
replacement save's `OperationState` and `SaveManager.CurrentState`; since the review the
oracle also waits for the `SerializeAndSaveThread` task itself before deleting.)

**Methodology notes.** Each run first waited until the machine had been quiet for 60 s
and a sampler checked every 5 s during the run; a run in which any sample showed a
compiler/tool process above 20% CPU, or any other process above 150%, was discarded and
repeated (up to four times). This detects sustained load, not every burst: `ps` %CPU is a
decaying average and samples are 5 s apart; OBS, the camera service and WindowServer are
exempt. Other agents' `rustc` builds contaminated nine attempts (a tenth, an OS-cold launch, hung; §6)
(`runs/raw/baseline/contended/`): those showed up to 415 frames ≥ 50 ms per pass and 27–35 s
reloads, so contention, not the game, dominates such numbers. OBS and a camera service
ran throughout (not controllable), as in Ember's earlier Wrath-Mac baselines.

## 4. Noise floor: the original against itself [M]

Screenshots at the eight points (shots mode, §1) from three fresh processes
(`fixtures/captures/poc0/oracle-2`, `oracle-3`, `shots-3`; best configuration), compared
point by point with `tools/compare-shots` (pixel statistics and NVIDIA FLIP 1.x, LDR,
viewing conditions of Ember's display: 0.7 m from a 0.597 m-wide 2560 px panel, about
52 pixels per degree). Comparison outputs: `fixtures/captures/poc0/noise/`.

| Comparison | FLIP mean, per point | FLIP p99 | pixels with FLIP > 0.1 | mean abs diff (0–255) | PSNR | camera position delta |
|---|---|---|---|---|---|---|
| same process, pass 1 vs 2 (oracle-2) | 0.021–0.044 (mean 0.030) | 0.08–0.41 | 0.5–8.1% | 0.94–1.87 | 33.8–42.2 dB | 2.8 mm |
| same process, pass 1 vs 2 (oracle-3) | 0.038–0.074 (mean 0.048) | 0.22–0.45 | 8.4–21.2% | 1.82–3.65 | 30.8–34.2 dB | 23 mm |
| cross-process (oracle-2 vs oracle-3) | 0.034–0.057 (mean 0.040) | 0.17–0.36 | 5.1–13.9% | 1.57–2.81 | 32.8–36.1 dB | 14 mm |
| cross-process (oracle-2 vs shots-3) | 0.026–0.046 (mean 0.033) | 0.11–0.38 | 1.4–9.2% | 1.33–2.16 | 33.4–38.5 dB | 6.6 mm |
| cross-process (oracle-3 vs shots-3) | 0.026–0.055 (mean 0.035) | 0.15–0.40 | 3.5–12.3% | 1.21–2.72 | 32.6–36.3 dB | 10 mm |

What differs (FLIP maps, `noise/maps-oracle2-shots3/`): floating dust particles,
time-animated emissive shaders (the glowing fungi pulse), idle animations of units and
critters, and small whole-image shifts from the camera position itself. **The camera is
not exactly static:** the resolved camera position differs by 3–23 mm between captures,
and drifts by up to 9 mm within a 5 s settle (`stability` in the route files), most likely because
`CameraRig.UpdateInternal` keeps easing the rig toward its target minus a camera-shake
anchor offset even while `FixCamera` is held [I, from code; the shake source isn't
identified — the save has a background earthquake cutscene]. Rotation and FOV are identical. Point 5 is the noisiest in every comparison (most particles and fungi); the quietest
varies (points 1, 4, 6, 7).

**Provisional triage threshold for POC-6** [I] (three processes are too few to bound the
noise distribution; calibrate with more captures and time-frozen ones): a Rust capture at
a point is "within the original's noise" if its FLIP mean against the original is at most the worst
cross-process value at that point (overall ≤ 0.057, ≤ 0.074 within a process) and its
FLIP > 0.1 share is at most the same, and its FLIP map shows no localised structure the
original's maps don't; anything else goes to a person. Mean FLIP can hide a small
deterministic error, so the maps matter. If the other engine replays the *resolved* pose
exactly (to be verified with landmark projections), its comparison has no camera term and
this threshold is slightly generous; a stricter floor needs time-frozen captures (shared
shader time and particle seeds), which is future work.

## 5. How to rerun

(2026-10-12: these tools now live in this repository under `oracle/tools/` and run on
`builds/arm64-oracle`, which has no gameplay mods; the commands below are as they were run then.)
Everything runs from `~/Developer/Wrath-Mac` with Steam running, the game closed and
(for timed runs) nothing heavy on the machine. Each run is one fresh launch of
`builds/arm64` through `tools/launch.sh`.

```sh
# build and install WrathOracle into builds/arm64/Mods (game closed)
(cd mods/WrathOracle && dotnet build -c Release -p:WrathBuild=$PWD/../../builds/arm64)

# functional: identity, scene dump, nav dump + queries, route with screenshots, reload, route again
tools/oracle-run.sh --label oracle-N oracle ~/Developer/Wrath-Rust/fixtures/captures/poc0/oracle-N
# screenshots only (one route pass)
tools/oracle-run.sh --label shots-N shots ~/Developer/Wrath-Rust/fixtures/captures/poc0/shots-N
# the timed baseline, per configuration (game = the game's own behaviour, best = WrathPerf on)
tools/oracle-run.sh --config game --label game-N bench "$PWD/runs/raw/baseline/game-N"
tools/oracle-run.sh --config best --label best-N bench "$PWD/runs/raw/baseline/best-N"
# OS-cold (shader cache): clear, run, restore
tools/metal-cache.sh clear && tools/oracle-run.sh --config game bench "$PWD/runs/raw/baseline/game-oscold"; tools/metal-cache.sh restore
# what was actually used: the same runs with the contention policy of §3 (waits for 60 s
# of quiet, samples during the run, repeats contaminated runs up to 4 times, clears and
# restores the shader cache around :cold runs). One instance at a time, detached:
nohup tools/oracle-bench-batch.sh game:game-1 best:best-1 game:game-2 best:best-2 \
  game:game-3 best:best-3 game:game-oscold:cold best:best-oscold:cold > batch.log 2>&1 &

# summaries
tools/summarize-oracle-bench runs/raw/baseline/game-{1,2,3}
tools/summarize-oracle-bench runs/raw/baseline/best-{1,2,3}
# noise floor (venv: Wrath-Rust/cache/tools/flip-venv with numpy, pillow, flip-evaluator)
~/Developer/Wrath-Rust/cache/tools/flip-venv/bin/python tools/compare-shots A/route-pass1.json B/route-pass1.json -o out.json --maps maps/
```

`oracle-run.sh` writes WrathPerf's `Settings.xml` for the configuration and restores the
previous file afterwards, writes WrathOracle's plan (removed on exit even if the game
never starts), and passes the extraction spike's `cache/import/prologue_caves_1/`
`inventory.json` and `nav.json` to the oracle when they exist. After the game quits it
copies the WrathStats session, `Player.log`, `GameLogFull.txt` and `general_settings.json`
into the output folder and warns if any `Manual_*_WR_*.zks` slot survived. The bench
plan is: `load` (process-cold), `settle:10`, `mem`, `identity`, `route` (5 min), `mem`,
`reload`, `settle:10`, `mem`, `route` (5 min), `mem`, `reload`, `settle:5`, `reload`,
`settle:10`, `mem`, `save`, `mem`, `quit` (about 13 minutes).

**Gotcha for scripts that wait on the game:** `launch.sh` refuses to start while
`pgrep -f 'Pathfinder Wrath Of The Righteous'` matches anything, and that includes any
other shell whose command line merely contains the string (a `grep` of `Player.log`'s
path, a watch loop). Match the process name instead (`pgrep -x`), as `oracle-run.sh`
does for the render spike. `metal-cache.sh` has the same `pgrep -f` check, so a
watch loop that mentions the game's name can make a cache restore refuse (this happened
once here and was repaired by hand).

## 6. Open issues

- **VSync quantisation.** Ember's settings (VSync on, 240 Hz) lock the original to 80 fps
  with p99 at 21 ms; frame-time percentiles measure vsync steps, not render cost. "p99 at
  least 25% lower" is reachable only by presenting at 120/240 Hz. For the render-cost
  comparison, a VSync-off variant of the route (and GPU timings) is still to be measured;
  the matched workload above stays the like-for-like player experience. Lowering p99
  could also come from a shorter tail at the same rate; the quantisation reading is an
  inference until presentation timing is measured.
- **Hitch target.** The original already has 0–1 frames ≥ 33 ms per run on the route with
  a warm shader cache. "Better" therefore means matching that from a cold start (no shader
  stalls on first sighting) and keeping it through loads and saves (the original's save
  costs one ~250 ms frame).
- **Live, not frozen.** The route keeps the game running (NPC idles, particles, shader
  time). That is the realistic workload but makes screenshots time-dependent; see §4.
  WrathOracle does not freeze shader time or seed particles.
- **Camera drift.** Up to 23 mm between captures, cause not identified (the rig's shake
  anchor easing is the candidate, §4). The replay
  export records the pose actually rendered, so replays are exact; only the original's
  own captures vary.
- **Fog of war off** for the route, as Arbiter's QA passes do. A Rust replay must also
  disable it (or a second route with FoW as saved can be added).
- **OS-cold is shader-cold only.** The file cache can't be dropped without root; one run
  per configuration. One OS-cold launch hung before the main menu (main thread blocked
  on a semaphore inside UnityPlayer, 0% CPU for 20 minutes) and was killed; not
  reproduced in two later cold launches.
- **Load boundary** excludes the loading screen's fade-out and the first frames after
  idle; WrathStats' spans and the oracle's agree within 0.15 s.
- **Dumps vs extraction:** the full diff of `scene-dump.json`/`nav-*.json` against
  `wr-import`'s outputs is the extraction side's next step (first checks in §2). The
  navmesh_agent points are the Seeker's result plus `RemoveExcessPoints`; real-time
  combat's `ObstaclePathfinder` step isn't reproduced. `BlockUntilCalculated` returns
  other pending paths early, so nav dumps belong before timed steps (as in the presets).
- **Review.** GPT 6 Astra (high) reviewed WrathOracle and the tools (18 findings, all
  checked against the code and fixed before the clean baseline runs: §7).

## 7. Review

GPT 6 Astra (high) reviewed WrathOracle, the tools and this page in two rounds; every
finding was checked against the code and the decompiled game before acting.

- **Round 1 (code, 18 findings, all fixed before the clean runs):** the save worker could
  still own the bench slot when it was deleted, and nothing stopped other saves during a
  live run (now a `SaveRoutine` guard allows only the bench slot); plan consumption by
  atomic rename; nav snapshot taken after flushing pending graph updates, with a change
  check; the side-effecting `IsInTurnBasedCombat()` replaced by its predicate; route frame
  ranges were shifted by one interval (now `[start, end)`); camera-guard release only of
  our own acquisition; load timeouts fail instead of passing; grid dynamic/ally blocking
  exported; end-node G only for a reached endpoint of this search; `navmesh_agent`
  relabelled as the Seeker result plus `RemoveExcessPoints`; FieldDumper follows Unity's
  declared-type layout; `run.json` written atomically; driver exit status, pgrep handling,
  `compare-shots` validation, quaternion normalisation and JSON-safe PSNR; summarizer
  handles failed runs.
- **Round 2 (verification, scripts, this page):** confirmed the round-1 fixes; found and
  fixed: unvalidated batch labels and no locking (now validated specs, a batch lock and a
  per-run lock in `oracle-run.sh`, existing outputs never deleted), non-transactional
  shader-cache handling (clear must succeed, restore waits for the game to exit and stops
  the batch on failure), exhausted retries reporting success, the save's worker task not
  awaited (now `SerializeAndSaveThread`'s Task), the save guard outliving a non-quitting
  plan, a graph-change check that missed in-place changes (now a fingerprint over every
  node), container element layout, incomplete runs in summaries, empty screenshot sets;
  and in this page the save numbers (commit vs routine; three save frames ≥ 50 ms, not
  one), several small numbers, relative paths in §5, and over-claims (vsync, shader
  compilation, camera-shake cause, the FLIP threshold), now marked as inferences or
  provisional. A `check` smoke run (dumps, nav, timed save) passed on the final build.
- The baseline runs were made with the round-1 build; the round-2 code changes don't touch
  the measured paths except the save step's extra wait for the worker task, which happens
  after the timing.
