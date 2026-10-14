# POC-6 first light: Prologue_Caves_1's static geometry in Bevy, through Owlcat's shaders

Written 2026-10-11. Scope: PLAN.md POC-6, first step: the caves' static geometry drawn
with the game's own shaders from the eight Arbiter camera poses, compared with the
original's screenshots. **[M]** measured here, **[I]** inferred. Images and run outputs
are under `fixtures/captures/poc6/` (gitignored); the pack and its report under
`cache/poc6/`.

## Result

- **It renders, from the exact poses [M].** `wr-game` draws 889 G-buffer draws (626
  renderers, 50 materials) with the game's `Owlcat/Lit`, `Owlcat/Unlit` and
  `Owlcat/Terrain` variants, then Owlcat's own deferred reflections, light culling
  (compute), deferred lighting, fog, grading LUT and Uber passes, all as offline-compiled
  metallibs of the game's MSL. Camera matrices are the recorded ones
  (`world_to_camera`, `gpu_projection_render_into_texture`), used as they are; the
  silhouettes line up with the original's at every point (checked by eye, not by
  landmark projection).
- **FLIP vs the original: 0.077–0.257 per point, mean 0.144 [M]**, against the noise
  floor of 0.021–0.074. Far above it, as expected at this stage; point 4 (a lit room,
  no units or particles in view) is at 0.077, just above the floor. The same numbers
  within ±0.006 against all three original runs (`oracle-2`, `oracle-3`, `shots-3`).
- **Load: 0.15–0.17 s from process start to every pipeline and bind group built, first
  frame submitted at 0.33–0.38 s [M, OS shader cache warm, the original game not
  running]** — for static geometry only (no units, audio, simulation); the original's
  best area load is 6.0 s.
- **Frame intervals [M, provisional]:** with vsync (`Fifo`) on the 240 Hz panel, p50
  4.10 ms (240 Hz), p99 9.6 ms; unpaced (`Immediate`) p50 3.8 ms, p99 10.6 ms. These are
  the main world's update intervals, not GPU timings (§4).

## 1. The pack (`wr-import pack`, crate `wr-assets`)

One memory-mapped file per area, `cache/poc6/packs/prologue_caves_1.wrpack` (160 MiB,
built in 4.3 s, metallibs cached by MSL hash under `cache/poc6/metallib/`):

```text
0      header (64 B): magic WRPACK\0 1, version, metadata offset/length/FNV-1a, file length
4096   blobs, 256-byte aligned: vertex streams, index buffers, texture payloads, MSL, metallibs
end    metadata (postcard): textures, meshes, programs, shaders, materials, renderers,
       lights, lightmaps, scene settings, reflection probes, LightController setup,
       pipeline programs, skipped-object counts
```

- **Identity:** every object keeps its Unity identity (`AssetId`: serialized file +
  path id), renderers also their GameObject's.
- **Textures:** shipped bytes unchanged for BC1/3/4/5/6H/7, RGBA8, BGRA8, half and float
  formats (130 MiB); labelled transcodes for RGB24 (1 texture) and the 4 crunched
  lightmaps (decoded to BGRA8, **mip 0 only**: the public crunch decoder returns pixels,
  not DXT blocks, so these are not yet the bit-identical DXT5 Unity transcodes to).
- **Meshes:** Unity's vertex streams and index buffer byte for byte, with the 14-channel
  layout; 608 of 626 renderers are statically batched into 35 meshes.
- **Materials:** shader, keywords, render queue, disabled passes, every texture
  (scale/offset), float and colour property.
- **Shaders:** each pass's serialized state (blend, ZTest, ZWrite, cull, stencil, with
  property bindings like `[_CullMode]`), properties (defaults, flags), and per material
  a G-buffer variant for its keywords plus the global (`DEFERRED_ON`) and renderer
  keywords (`LIGHTMAP_ON`, `DIRLIGHTMAP_COMBINED`): 43 exact matches, 11 picked by our
  nearest-match heuristic (most enabled keywords, then fewest others; e.g. lightmap
  variants exist only with `DYNAMICLIGHTMAP_ON`). **The heuristic is not verified against
  Unity's choice** [I]; a capture of the programs the game binds would settle it. Programs carry their MSL, an offline `xcrun metal` library, and
  the resource interface parsed from the MSL (slots, constant-buffer layouts, vertex
  inputs). 59 programs, all with metallibs.
- **Pipeline programs:** `Hidden/Owlcat/DeferredLighting`, `DeferredReflections`, `Fog`
  (from `utility_shaders`), the `LightCulling` compute kernel, and `LutBuilderHdr` and
  `UberPost` (from the player's `globalgamemanagers.assets`, which has no type trees:
  the reader borrows the `Shader` tree from a bundle of the same Unity version).
- **Lights and settings:** all 976 lights (81 realtime/mixed) with `OwlcatAdditionalLightData`,
  lightmaps per scene, reflection probes (23, BC6H cubemaps), each scene's
  `RenderSettings`, and the light scene's `LightController` (Kingmaker.Visual.DayNightCycle):
  its solo `SceneLightConfig` (fog, ambient, main light), camera clear flag and camera
  light flag, and its post-processing volume with every component's fields. The game
  camera rig's `CameraLight` comes from the player's `level2` (no type trees; trees
  borrowed the same way).
- **Script-driven state:** `BakedTerrain` activates the four baked terrain meshes and gives
  them `OwlcatTerrain`'s property block (layer arrays, texture arrays); the pack records
  that block per renderer.
- **Runtime side (`wr_assets::Pack`):** `mmap`, header and checksum check, every blob
  reference bounds-checked at open; blobs are borrowed from the mapping (metallibs go to
  Metal without a copy). Safety argument in `Pack::open`: packs are written to a
  temporary file and renamed, never modified in place.

Skipped, with counts in the pack: particle renderers (423), skinned meshes (13), decal
projectors (6 materials), particle-shader materials on meshes (3), one transparent
(queue 3000) material, 63 renderers whose only materials are those.

## 2. The render path (`crates/wr-game`)

A Bevy 0.20 app (wgpu 30, Metal). Bevy hosts the window, the frame loop and the display;
the frame is a port of `ClusteredRenderer`'s pass list recorded with raw wgpu on Bevy's
`RenderDevice`, in the root render graph's `Begin` set, into an offscreen 2560×1440
target that a sprite shows.

**Load gate [M]:** `OwlcatPlugin::finish` opens the pack, uploads everything and builds
every pipeline, bind group and constant buffer before the app's first frame. Per frame
the renderer creates only its command encoder; when the route point changes it rewrites
the camera-scope constant buffers and light buffers (no pipelines, bind groups or
textures).

**Unity layer reimplemented:**
- Bind group layouts reproduce each shader's Metal slots (one group; per stage buffers,
  samplers, textures in slot order, which wgpu-hal assigns sequentially).
- Constant buffers filled member by member from each program's own layout: material
  properties (colours, and floats/vectors marked `[Gamma]`, converted to linear), `_ST`/`_TexelSize`/`_HDR`
  texture vectors, per-draw built-ins (`unity_ObjectToWorld` — identity for statically
  batched renderers — lightmap ST, world transform params), and camera globals
  (`UnityPerCamera`, `UnityPerFrame`, `UnityPerPass`, Owlcat's `_PerCamera`: projection
  params with the render-into-texture flip, reversed-Z `_ZBufferParams`, camera basis,
  inverse view-projection, clustering and light-data params).
- Static batching: batched renderers draw with identity transforms *and* identity
  lightmap scale/offset, since Unity bakes both into the combined mesh (found in review:
  applying the scale/offset twice flattened the lightmaps of 590 renderers).
- Render state from the pass state plus material properties, reversed Z (comparisons
  flipped as Unity does on Metal), front faces CCW (the flipped projection).
- Vertex inputs matched by HLSLcc semantic to mesh channels; missing channels read a
  stride-0 constant (white for colour, zero otherwise).
- Textures and samplers by name: material, property block, lightmaps, G-buffer targets,
  then the shader property's Unity default (`white`, `black`, `gray`, `bump`).

**Passes, in order:**

| Pass | What runs | Status |
|---|---|---|
| Light culling | `LightCulling.BuildLightTiles` compute over Owlcat's light data | Ported: `ClusteredLights` setup (MinMaxZ, radix sort order, `ExtractLightDataJob`, Z-bins) in Rust; visible-light selection approximated (enabled, not fully baked, range sphere in frustum) |
| G-buffer | 6 targets + depth, `Owlcat/Lit`/`Unlit`/`Terrain` | Complete for static meshes |
| Deferred reflections | default reflection (skybox cubemap), 23 probe volumes, blend with emission | Ported |
| Deferred lighting | `Hidden/Owlcat/DeferredLighting` (fragment path, no shadows) | Ported; the camera rig's spot light added from the rig pose |
| Fog | `Hidden/Owlcat/Fog` `FOG_LINEAR`, the `LightController` config | Ported |
| Grading LUT | `LutBuilderHdr` (`_TONEMAP_NEUTRAL`), 1024×32 | Ported: `ColorGradingLutPass` parameters from the volume stack (defaults per component, overrides from the profile, the player's gamma setting as the gamma profile does) |
| Uber | `UberPost` `_HDR_GRADING`: LUT + vignette | Partial: no bloom, film grain, dithering |
| Final | our blit: Y flip, bytes to the display target | — |

**Colour space [I, measured fit]:** Owlcat's deferred-lighting and fog fragments end by
encoding to sRGB, while Uber treats its input as linear. Measured mean FLIP over the 8
points: without post 0.178; post on the camera colour as written 0.389; post with the
camera colour decoded to linear before Uber **0.144**; decoded before fog instead 0.182.
The game must reach the same effect some other way (an sRGB view, the compute lighting
path); a Metal frame capture of the original would settle it. `WR_LINEARIZE=off|lighting`
and `WR_NO_POST` switch the alternatives.

## 3. Comparison

`wr-game --mode shots` captures each point after 10 settle frames and writes a
`wrath-camera-replay-v1` route next to the PNGs, so `oracle/tools/compare-shots`
compares it with the original's route directly (`first-light/compare-*.json`, FLIP maps
in `first-light/maps-oracle-2/`).

FLIP mean per point vs `oracle-2` pass 1 [M] (noise floor per point from `baseline.md`
§4: 0.021–0.074):

| Point | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | mean |
|---|---|---|---|---|---|---|---|---|---|
| G-buffer + lighting + fog (`fl-1`, batched lightmaps wrong) | 0.269 | 0.181 | 0.143 | 0.166 | 0.177 | 0.222 | 0.159 | 0.174 | 0.186 |
| + post, linearised before Uber (before review fixes) | 0.273 | 0.172 | 0.140 | 0.136 | 0.089 | 0.172 | 0.154 | 0.143 | 0.160 |
| **final**: + review fixes (batched lightmap ST, probe samplers, `[Gamma]`, curve texels) | 0.257 | 0.161 | 0.120 | 0.130 | 0.077 | 0.155 | 0.131 | 0.120 | **0.144** |

What the images show (final vs original): geometry, materials, lightmaps and the overall
layout agree point for point (silhouettes, mushrooms, ruins, terrain). Point 0 is the
furthest: the original's centre is lit by the party and things on screen we don't draw
(units, their lights, particles), ours is darker there. Point 5's glowing fungi are flat
in ours (no bloom, no glow particles) and its hanging roots render as bright streaks
(alpha-tested foliage, see below).

**What's missing, ranked by visual impact [I from the FLIP maps and images]:**

1. **Units and critters** (party, NPCs, the spider, tarantulas): not drawn; also their
   equipment, selection rings and any lights they carry.
2. **Particles and VFX** (423 Shuriken systems: floating dust, fungus glow, sparkles; VFX
   Graph): absent. They are most of the original's high-frequency differences.
3. **Bloom** (`BloomEnhanced`, active in the cave profile), **film grain**, dithering,
   **SMAA**: not run. Bloom carries the fungi's glow.
4. **Lighting gaps:** no shadow maps (realtime shadowed lights draw unshadowed); light
   probes not interpolated (the 30 non-lightmapped renderers and the baked terrain use
   the ambient probe's L0); the visible-light set is approximated; the game's
   `ForcedCullingRadius` and other per-light scripts not applied; HBAO (active, intensity
   1.59) and SSR not run.
5. **Colour pipeline uncertainty** (above) and the brightness/contrast (`Daltonization`)
   pass not run (neutral at Ember's settings [I]).
6. **Alpha-tested foliage** (hanging roots at point 5) looks wrong: likely alpha-to-
   coverage or a missing material feature; not investigated.
7. **Decals** (6 projector materials) and the one transparent material: not drawn.
8. **Lightmaps** use mip 0 only (crunch decoder limitation); minor at these distances.
9. Distortion, fog of war (off in the captures anyway), sky (none visible), DOF (inactive).

## 4. Measured numbers (provisional)

Machine idle apart from OBS and the webcam service; `pgrep -f 'Pathfinder Wrath'` was
empty during the timing runs. OS Metal shader cache warm; OS-cold not measured (the
cache isn't per-app here).

| | Value |
|---|---|
| Pack build (`wr-import pack`) | 4.3 s, of which metallib compile 1.7 s (warm cache: 0) |
| Process start → renderer ready (all pipelines and bind groups) | 0.15–0.17 s |
| … pack open (mmap, metadata decode, validation incl. MSL hashes) | 6–7 ms |
| … upload (166 MB of textures and buffers) | 40–51 ms |
| … 34 pipelines + compute, 38 shader modules from metallibs, constant buffers | 24–30 ms |
| Process start → first frame submitted | 0.33–0.38 s (0.80 s in one `Immediate` run) |
| Update interval, 8 points × 10 s, `Fifo`, 240 Hz panel | p50 4.10 ms, p95 6.98, p99 9.62; 9 intervals ≥ 33 ms (one of 1.09 s at point 6, others 46–54 ms) |
| Same, `Immediate` (unpaced) | p50 3.79 ms, p95 8.49, p99 10.55; none ≥ 33 ms (max 21.6 ms) |

These are intervals between the main world's `Update`s (what WrathStats measures for the
original), not GPU durations; Bevy's own sprite pass and presentation are included. An
earlier unattended run, with the display apparently asleep, gave unpaced 1.8 ms
intervals; it is not used. The long `Fifo` stalls are unexplained (nothing is created
after the gate; candidates are drawable acquisition and pipelined rendering hand-off) and
need a Metal System Trace. For reference, the original on this route: p50 12.45 ms, p99
21.1 ms, vsync-locked at 80 fps — but with units, particles and post the original has
and we don't.

## 5. Effort estimate, updated [I]

POC-0's guess for the core frame at caves parity was 17–23 weeks. After this round the
G-buffer, deferred reflections and lighting, fog, light culling, grading and the
pack/Unity layer exist for static geometry, without their hard tails (shadows, probes,
exact colour handling). Remaining, roughly:

| Work | Guess |
|---|---|
| Shadows (atlas, cascades, tetrahedral point faces, shadow caster pass) | 2–3 weeks |
| Light probes (tetrahedral interpolation), reflection probe selection, visible-light rules, light scripts | 1–2 weeks |
| Post: bloom pyramid, grain, dithering, SMAA, HBAO, SSR, Daltonization; settling the colour space with a frame capture | 3–4 weeks |
| Skinned characters and equipment (with POC-7's units), PBD | 3–4 weeks |
| Transparents, decals, distortion, fog of war | 2–3 weeks |
| Frame-capture diff tooling and pass-by-pass parity | 2–3 weeks |
| **Core frame at caves parity, remaining** | **~13–19 weeks** |
| Particles (Shuriken) and VFX Graph | separate, 8–16 weeks |

## 6. Review

GPT 6 Astra (high) reviewed the pack, the renderer, the diff tool and both reports
(read-only). Every finding was checked against the code or data before acting:

| Finding | Verdict | Fix |
|---|---|---|
| Statically batched renderers applied the lightmap scale/offset a second time (UV1 already in lightmap space) | Real: confirmed on the packed UV1 ranges; 590 renderers | Identity ST for batched renderers; FLIP 0.160 → 0.144 |
| One shared `.tmp` name let two writers share an inode; a safe `Pack::open` can't promise immutability | Real | Unique, exclusively created temp files; `Pack::open` is now `unsafe` with a documented contract (as `wr-content`'s pack is) |
| Pack trust: shader payloads and interfaces unchecked at load; missing layouts became tiny buffers; unknown storage buffers bound the light tiles | Real | `Pack::open` checks every program's MSL against its hash and that each constant buffer has a layout; unknown storage buffers or layouts skip the draw |
| Reflection probes sampled with a nearest-mip default sampler | Real | Each cubemap's own (trilinear) sampler |
| `[Gamma]` handled backwards (it applies to floats/vectors; colours always convert) | Real | Fixed for defaults and material values |
| Default curve textures sampled at `i/127`; `TextureCurve` uses `i/128` | Real | Fixed |
| Frame-time claims overstated; bind groups created per frame despite the gate claim | Real | Blit/linearize bind groups and the display view made once; numbers relabelled as update intervals |
| Variant fallback presented as Unity's rule | Fair | Labelled as our unverified heuristic |
| Diff tool: probes compared by index, missing coverage silent, components joined to the first of each type | Real | See `docs/poc0/extraction.md` §9 |

## 7. How to rerun

```sh
export PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
cargo build --release -p wr-import -p wr-game
target/release/wr-import pack                       # → cache/poc6/packs/prologue_caves_1.wrpack
target/release/wr-game --windowed --out fixtures/captures/poc6/first-light   # shots
cache/tools/flip-venv/bin/python oracle/tools/compare-shots \
  fixtures/captures/poc0/oracle-2/route-pass1.json fixtures/captures/poc6/first-light/route-pass1.json \
  -o out.json --maps maps/
target/release/wr-game --mode bench --dwell 37.5 --out fixtures/captures/poc6/bench   # timings
```

Options: `--route` (default `oracle-2/route-pass1.json`), `--point N`, `--present
fifo|immediate`, `--settle N`; environment `WR_NO_FOG`, `WR_NO_POST`,
`WR_LINEARIZE=off|lighting` for the experiments above.
