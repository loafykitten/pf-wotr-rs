# POC-0 rendering spike: Bevy on Metal, and hosting Owlcat's shading

Written 2026-10-10 by the rendering-spike agent, revised after a GPT 6 Astra (high) review
(§10). Code and raw numbers: [`spikes/render/`](../../spikes/render/) (its `README.md` says how
to rerun each experiment; numbers are in `spikes/render/results/`). **[M]** = measured here,
**[I]** = inferred or read from source, **[S]** = from the decompiled-code survey by
gpt-6.1-sol (claims to check).

Machine: Mac Studio M2 Max (30-core GPU), 32 GB, macOS 27.0, AW2725DF 2560×1440 at 240 Hz
as main display (a second, 75 Hz Dell is attached). **Conditions were not quiet:** two other
agents compiled, OBS (~40–50% of a core, capturing) and the webcam assistant (~60%) ran
throughout, and the baseline agent kept relaunching the original game. Every scene run records
the 1-minute load average and whether the game was running (`conditions.txt`). The
`quiet-*` runs waited until the game was closed and the load was under 6 (the floor here with
OBS and the webcam assistant running); everything else is marked provisional. Shader-compile
counts and correctness checks don't depend on load.

## Verdict

| Question | Answer |
|---|---|
| Version to pin | **Bevy `=0.20.0`, wgpu `=30.0.1`** (naga 30.0.1, objc2-metal 0.3.2, objc2-metal-fx 0.3.2) |
| Can Bevy host Owlcat's shading? | **The strongest route works: run the game's own compiled Metal shaders, unmodified.** Wrath's bundles carry every shader variant as plain MSL. A real `Owlcat/Lit` G-buffer pair compiles and draws through wgpu 30's MSL passthrough, both headless and inside Bevy's render schedule, and its six targets hold the values the shader should produce at 81 checked pixels **[M]**. Every `Owlcat/Lit` fragment variant (3,803, five passes) builds into a pipeline with a linked vertex variant **[M]**. This is an integration smoke test, **not** proof of whole-frame parity: that needs the pipeline around the shaders (Owlcat's SRP passes, Unity's built-ins and per-object data) ported and diffed against captures of the original |
| Skinned characters | 150 skinned foxes: GPU busy ~3.1 ms per 1440p frame, 1.7 ms of it the main opaque pass (Metal System Trace) **[M]**. Bevy's CPU frame (~4.8 ms, nearly flat from 50 to 1000 characters) is the limit **[M]** |
| 240 Hz | Fifo and Immediate work; Mailbox isn't offered on Metal. Displayed intervals, measured at the panel, are exact multiples of 4.167 ms. Bevy doesn't hold 240 Hz here: Fifo settles at 150 Hz, alternating 1, 2 and 3 refreshes (judder); Immediate reaches 207 Hz with tearing **[M]** |
| GPU timings | `TIMESTAMP_QUERY` works at pass boundaries only; Bevy's `RenderDiagnosticsPlugin` therefore gives CPU spans only. A whole-frame timestamp bracket measures GPU *timeline* (including waits), not cost. Per-pass GPU busy time comes from Metal System Trace, automated **[M]** |
| Pipelines during load | A load gate on Bevy's `PipelineCache` works for what the load renders: no pipeline was added after the first visible frame in any run **[M]**. Cold Owlcat shader front end (MSL → AIR) costs **91 ms** mean per new G-buffer source and **173 ms** per new forward source on one thread; 8 threads cut a full pass ~5×; with macOS's shader cache warm, **0.12–0.16 ms** per pipeline **[M]** |
| Persisting pipelines | macOS already persists compiled Metal code per user, ~500× faster on the next launch **[M]**. `MTLBinaryArchive` works through raw Metal (verified hit) but only covers the back end and wgpu 30 has no hook for it **[M/I]** |
| BC textures | `TEXTURE_COMPRESSION_BC` on; BC1/3/5/7 sampleable and filterable; a BC1 base map read by Owlcat's shader gives the exact texel colour at the analytic UV (64/64 samples) **[M]** |
| MetalFX | Spatial (~0.5 ms) and temporal (~2.3 ms) run on wgpu's textures and command buffers through our own `objc2-metal-fx` calls and pass content checks **[M]**. Frame interpolation runs (~5.5 ms at 1440p) but in our test its output equalled the current frame, so interpolation is **not demonstrated** **[M]**. `bevy_metalfx` requires Bevy 0.19; it didn't decide the version |

## 1. Version to pin

crates.io on 2026-10-10 **[M]**: `bevy` newest is **0.20.0** (2026-10-08; rc.1 09-15, rc.2
09-28); 0.19.1 is 2026-08-13. `bevy_render 0.20.0` depends on **wgpu ^30** (lock resolves
30.0.1, 2026-08-22); 0.19.1 on wgpu ^29.0.3. `bevy_metalfx` 0.4.2 (2026-08-07) requires
`bevy ^0.19`, `wgpu ^29`, `wgpu-hal ^29`.

**Pin `bevy = "=0.20.0"` and `wgpu = "=30.0.1"`** (the wgpu Bevy uses, so raw-wgpu code shares
types with Bevy's `RenderDevice`). Reasons:

- wgpu 30 has what the Owlcat path needs: `Features::PASSTHROUGH_SHADERS` with **MSL and
  metallib** sources, `as_hal` on device, queue and textures, and a raw-only
  `CommandEncoder::as_hal_mut` (the hook for MetalFX and command-buffer timing).
- Bevy 0.20's renderer is **schedules, not a node graph**: a root `RenderGraph` schedule and
  per-camera schedules (`Core3d`) with system sets. A custom camera schedule
  (`CameraRenderGraph::new(OurSchedule)`) is a natural place for an Owlcat frame.
- 0.19's only advantage was `bevy_metalfx`, and our own MetalFX calls work on 0.20.

Risks: 0.20.0 is two days old; take patches deliberately. wgpu's passthrough API has changed
shape across releases (25→30). We did not build 0.19 for comparison.

## 2. Skinned characters

`render-spike scene`: N copies of Bevy's `Fox.glb` (skinned, ~1.7k vertices, 24 bones,
animated) in rings under a Wrath-like high camera, a shadowed directional light (2 cascades),
a shadowed point light, a ground plane and 28 material-variant objects; 1440p borderless
fullscreen, MSAA off (Wrath uses FXAA/SMAA). `--parts 4` gives each character four co-located
skinned meshes with their own skeletons (a rough stand-in for body plus equipment layers).
**The fox is far lighter than a Wrath character** (several equipment meshes, ~60–100 bones):
treat this as a floor.

Quiet runs (`results/scene/quiet-*`; original game not running, 1-minute load 5.5–6.9 of
which OBS and the webcam assistant are ~1.5 cores; borderless fullscreen 1440p, 20 s each):

| Scene, mode | App frame mean / p50 / p99 (ms) | Displayed rate |
|---|---|---|
| 50 foxes, Fifo | 6.67 / 7.16 / 12.95 | 150 Hz |
| 150 foxes, Fifo | 6.67 / 7.00 / 12.84 | 150 Hz |
| 150 × 4 parts (600 skinned meshes), Fifo | 6.69 / 6.43 / 12.19 | 149 Hz |
| 150 foxes, Immediate | 4.83 / 4.70 / 6.59 | 207 Hz |
| 150 foxes, Immediate, no pipelined rendering | 5.78 / 5.76 / 6.44 | 173 Hz |

Earlier contended runs (Immediate, `results/scene/immediate-*`) gave app frames of 4.98 ms at
150 foxes, 5.38 ms at 400 and 5.85 ms at 1000: nearly flat in character count.

**GPU cost per pass** (Metal System Trace, 150 foxes, Fifo, fullscreen, 432 frames, grouped by
Metal frame number; `results/scene/quiet-trace-fifo-150/gpu_passes.csv`) **[M]**, busy time per
frame: main opaque pass 1.68 ms fragment + 0.06 ms vertex; transparent pass 0.09 ms; upscaling
0.05 ms; two directional-shadow cascades ~0.07 ms each; six point-light faces 0.01–0.03 ms
each; Bevy's GPU preprocessing, uniform allocation, bin unpacking, indirect-parameter and
clustering compute passes 0.01–0.16 ms each; wgpu's own indirect-draw validation pass
0.075 ms. Summed busy time 3.07 ms per frame (p99 3.50; channels overlap, so this is an upper
bound); first-start to last-end span 4.77 ms (p99 6.50). 79,459 GPU intervals joined to
labelled encoders, 1,046 unmatched (driver work).

**The limit is Bevy's CPU frame, not skinning or the GPU.** In Immediate mode the app runs a
~4.8 ms frame regardless of character count; at 240 Hz the budget is 4.17 ms. A 6 s CPU sample
(`sample`, 150 foxes, under contention) shows the main thread parked ~34% of the time waiting
for the render thread; the render thread spends ~47% in `render_system` (wgpu `queue.submit`
and its `maintain` ~19%, `CommandEncoder::finish` ~8%) and ~36% parked waiting for parallel
render systems; the dominant leaf symbols across all threads are condition-variable waits
**[M]**. That pattern is hand-off latency between many small systems and threads, not raw
work, which suggests a leaner app (no Bevy PBR, fewer systems) could get under budget
**[I, untested]**.

## 3. Presentation at 240 Hz

What wgpu does on Metal **[I, wgpu-hal 30 source; M for the layer values]**: `Fifo` sets
`CAMetalLayer.displaySyncEnabled = true`, `Immediate` sets it false; Mailbox and FifoRelaxed
aren't offered. `desired_maximum_frame_latency` maps to `maximumDrawableCount` (1 → 2
drawables, 2 → 3, 3 → 3).

How we measured **[M]**: wgpu keeps the `CAMetalDrawable` private, so the spike swizzles
`-[CAMetalLayer nextDrawable]` to attach `addPresentedHandler` to every drawable, which reports
`presentedTime` (when it reached the display, host clock) or 0 if dropped. Displayed intervals
come out as exact multiples of 4.1667 ms (sd 0.000 within each multiple), so this is the real
display cadence, not the render rate.

Quiet runs, 150 foxes unless noted, 20 s, borderless fullscreen on the 240 Hz panel **[M]**:

| Mode | Displayed mean / p50 / p99 (ms) | Vsync multiples 1× / 2× / 3× | Acquire→displayed mean | Drops |
|---|---|---|---|---|
| Fifo, latency 2 (3 drawables) | 6.67 / 4.17 / 12.50 (150 Hz) | 1500 / 1200 / 300 | 12.9 ms | 0 |
| Fifo, latency 2, 50 foxes | 6.67 / 4.17 / 12.50 (150 Hz) | 1501 / 1197 / 301 | 13.0 ms | 0 |
| Fifo, no pipelined rendering | 6.47 / 4.17 / 12.50 (155 Hz) | 1586 / 1296 / 207 | 12.4 ms | 0 |
| Immediate, latency 2 | 4.83 / 4.72 / 7.92 (207 Hz, tearing) | n/a | 7.7 ms | 0 |
| Immediate, no pipelined rendering | 5.78 / 5.78 / 8.70 (173 Hz) | n/a | 7.4 ms | 0 |

From the contended campaign (`results/scene/fifo-*`): latency 3 behaved like latency 2 (both
give 3 drawables); latency 1 (2 drawables) fell to 70 Hz with stalls up to 67 ms.

- **Fifo is correct but the app misses vsync**: with a ~4.8 ms CPU frame against a 4.17 ms
  budget it settles into a 1/2/3-refresh pattern averaging exactly 150 Hz, which is visible
  judder. A steady 120 Hz (every second refresh) would look better than this unless the CPU
  frame is reliably under 4.17 ms.
- **Two drawables is worse**: the CPU blocks on `nextDrawable`.
- **Latency**: acquire→displayed (render thread getting a drawable to it reaching the panel)
  is ~13 ms in Fifo with 3 drawables and ~7.5 ms in Immediate. Pipelined rendering adds time
  before that (the main world runs a frame ahead); turning it off costs ~1 ms of frame time
  in Immediate mode. We did not measure input-to-photon; the original game's latency is the
  baseline agent's to measure.
- **Without the load gate** (`quiet-fifo-l2-150-nogate`, OS shader cache warm): 61 pipelines
  were created after the window showed, and the first frames stalled the app for up to
  242 ms (3 frames over 33 ms). With the gate: none, and load to first visible frame 0.77–0.96 s.
- Recommendations **[I]**: pace frames from `CAMetalDisplayLink` (macOS 14+) instead of blocking
  in `nextDrawable`; offer a frame cap (120 or 240) so Fifo never alternates; keep 3 drawables
  until the CPU frame is comfortably under budget.

## 4. GPU timing on Metal

Adapter report (`results/features.txt`) **[M]**: `TIMESTAMP_QUERY` yes,
`TIMESTAMP_QUERY_INSIDE_ENCODERS` no, `TIMESTAMP_QUERY_INSIDE_PASSES` no,
`PIPELINE_STATISTICS_QUERY` no. The M2 Max samples GPU counters only `AtStageBoundary`; draw,
dispatch, tile and blit boundaries are unsupported.

What that means **[M]**:

- Bevy's `RenderDiagnosticsPlugin` writes timestamps only with `INSIDE_ENCODERS`, so on this
  Mac it records **CPU spans only** (`render/*/elapsed_cpu`, 0 `elapsed_gpu`).
- Pass-boundary timestamps (`timestamp_writes` on a render or compute pass) **do** work. The
  Owlcat G-buffer pass (512², one sphere, 6 targets) reads 0.03–2 ms (p50 0.10 ms): each
  frame there is an isolated submission, so the number mostly reflects GPU power state, not
  cost.
- Metal quirks: an **empty** pass returns 0 for both timestamps (our markers dispatch one
  no-op thread), and resolving a query in the **same command buffer** that samples an
  end-of-encoder counter returns 0 (we resolve after that command buffer completes).
- **A frame bracket can't isolate GPU cost.** Metal orders encoders only through resource
  dependencies. Our first end marker (an independent compute pass) finished ~0.8 ms *before*
  Bevy's last pass in the trace (found in review), so its 2.4–2.6 ms "GPU frame" figures
  under-measured and have been dropped. The end marker is now a render pass on the swapchain
  texture (so Metal orders it after the last write to the drawable), but then the span also
  includes waiting for the compositor to release the drawable (~9 ms on the 75 Hz panel). It's
  recorded per frame in `gpu_frames.csv` as a timeline span, not as cost.
- **Per-pass GPU cost:** `scripts/trace-scene.sh` records Metal System Trace with `xctrace`,
  exports `metal-gpu-intervals` and `metal-application-encoders-list`, joins them on encoder
  ID (wgpu forwards pass labels as encoder labels; join coverage is reported), groups by
  Metal's own frame number and writes busy time per pass and GPU channel. Other processes'
  GPU work is in the same trace (OBS's was), and on a TBDR GPU spans overlap, so use busy time.

## 5. Building pipelines during a load

**In Bevy [M].** On macOS Bevy 0.20 builds pipelines **synchronously** inside
`PipelineCache::process_queue` (its async path is compiled out for macOS), so a pipeline first
needed mid-play is a hitch on the render thread. The spike's gate: an opaque loading overlay
covers the window while the 3D camera renders the real scene behind it; the gate opens when
every character is ready, no pipeline has failed (Bevy's transient `ShaderNotLoaded` state
counts as pending), and the render world reports nothing queued and no new pipelines for 30
consecutive frames.

- 62–63 pipelines for this scene; 58–59 built inside our timed `process_queue` call (the rest
  inside upscaling's own `block_on`), 230–830 ms (4–14 ms each with the OS shader cache warm
  for Bevy's shaders, so mostly naga WGSL → MSL translation, which nothing caches).
- **No pipeline was added to the cache after the first visible frame, in any run.** The gate
  covers what the load rendered. A pipeline first needed later (something that becomes visible
  after the load) isn't covered; the engine should pre-specialise an explicit set per area
  from its material census.

**Real Owlcat pipelines [M]** (`owlcat-bench`: every fragment variant of each `Owlcat/Lit` pass,
each paired with the vertex variant Unity's per-stage rule selects, the fragment's keywords
restricted to those the vertex stage knows, or the nearest linkable one when there's no exact
match; Metal must accept the link):

| Pass | Fragment variants | Unique MSL | Pairing exact / nearest | Vertex variants exercised |
|---|---|---|---|---|
| GBuffer | 2,018 | 1,157 | 1,762 / 256 | 216 of 389 |
| ForwardLit | 1,738 | 796 | 1,738 / 0 | 49 of 63 |
| ShadowCaster | 12 | 8 | 4 / 8 | 6 of 12 |
| DistortionVectors | 24 | 12 | 24 / 0 | 4 of 6 |
| Meta | 11 | 12 | 11 / 0 | 1 of 1 |

All built; none failed. Vertex variants not exercised are those no fragment variant selects
(e.g. vertex-only keywords such as instancing); those combinations aren't covered.

Cost, one thread (`--salt` appends a comment to every source so the **front end** misses the
OS cache; the back end, AIR → GPU binary, can still hit because a comment doesn't change the
AIR):

| | GBuffer | ForwardLit |
|---|---|---|
| New fragment source, cold front end: mean / p50 / p95 | 91 / 108 / 118 ms | 173 / 167 / 234 ms |
| Repeated source in the same run (cache hit) | 0.37 ms mean | 0.20 ms mean |
| Whole pass, 1 thread / 8 threads | 103 s / 20 s | 136 s / 28 s |
| Whole pass, warm OS cache (same salt again) | 0.24 s (0.12 ms per pipeline) | 0.27 s (0.16 ms) |

The back end was fully cold only in the very first, unsalted run (400 G-buffer pipelines,
older pairing heuristic): pipeline-state creation then averaged **7.7 ms** (p95 14 ms) on top
of the front end, 73 ms per pipeline in all (`results/owlcat-bench/unsalted-first400-gbuffer`,
restored from the session log). An area needs a small subset: the caves use 57 `Owlcat/Lit`
materials **[M, extraction spike]**, so on the order of 100–200 variants across passes
**[I]**, roughly 3–5 s on 8 threads with a cold front end and ~20 ms warm. That's an estimate, not a
measurement of the caves.

**Persisting across launches [M/I]:**

1. **macOS does it already.** Metal keeps a per-user shader cache
   (`$(getconf DARWIN_USER_CACHE_DIR)/com.apple.metal/`). Second launch: 0.12 ms instead of
   ~73 ms per Owlcat pipeline **[M]**. It's evictable and invalidated by OS/driver updates, so
   the first launch after an update is cold.
2. **`MTLBinaryArchive` works** with a real Owlcat pipeline through raw Metal: built and
   serialized (106 KiB for one pipeline), reloaded in a fresh process, and creation with
   `FailOnBinaryArchiveMiss` succeeded in 0.17 ms (vs 18 ms without it, first time) **[M]**.
   It covers the back end only; the front end still needs the MSL compile or the OS cache.
3. **Full persistence would need [I]:** `metallib`s compiled offline from each variant's MSL
   (Xcode's Metal toolchain; not installed here) loaded through wgpu's `metallib`
   passthrough, plus a binary archive for the back end. wgpu 30 builds its
   `MTLRenderPipelineDescriptor` internally with no hook for `binaryArchives`, so that needs a
   wgpu-hal patch (size unknown; wgpu's `PipelineCache` is Vulkan-only). Recommendation: rely
   on the OS cache plus building each area's pipelines during its load on several threads;
   treat metallib + archive as a later fix for the first launch after an OS update.

## 6. Can Bevy host Owlcat's shading?

### How Wrath renders [S, partly checked against the MSL]

Owlcat's own SRP (`Owlcat.Runtime.Visual`, 318 files, 24.9k lines): `OwlcatRenderPipelineAsset`
→ `ClusteredRenderer`, **hybrid deferred** (clustered forward also supported). Per camera, in
order: GPU skinning and indirect-instancing submits and culling; fog-of-war shadowmap and
setup; position-based-dynamics (cloth) simulation; shadow atlas (default 4096², 1–4 cascades,
point lights as **4 tetrahedral faces**); clear; **G-buffer** (6 MRTs); copy depth; **light
culling** compute (16-pixel tiles, 4,096 linear Z-bins, light data in 64 KiB constant buffers
as structure-of-arrays); grading LUT; **deferred lighting** (compute or fullscreen); decals and
**HBAO** (order depends on SSR); cloud shadows; skybox; colour pyramid; refraction G-buffer;
depth pyramid and **SSR**; distance **fog**; **fog-of-war composite**; sorted transparents;
**distortion**; post (StopNaN → SMAA → DOF → motion blur → radial blur → Panini → Uber
(bloom, lens distortion, chromatic aberration, vignette, grading/tonemapping Neutral/ACES,
grain) → Daltonization → FXAA); highlighting; GUI decals; final blit. The full survey is
`cache/poc0-render/owlcat-pipeline-survey.md` (gitignored; cites decompiled files).

What the MSL adds **[M]**, from `Owlcat/Lit`'s GBuffer fragment: RT0 = albedo (sRGB) +
metallic; RT1 = normal as **octahedral, 12 bits per axis packed into RGB8** + smoothness;
RT2 = emission into the camera colour target; RT3 = baked GI (R11G11B10F: lightmaps,
directional and dynamic lightmaps, or SH/LPPV); RT4 = shadowmask; RT5 = translucency colour ×
thickness + a material flag byte. **Lightmaps are used.** Normal maps are DXT5nm (x = r·a);
masks map x → roughness, z → metallic, w → thickness. Ordinary skinning happens before the
material shaders, but **PBD variants deform in the vertex shader** from blend weights and
indices (`PBD_SKINNING`, `PBD_MESH`; e.g. blob 35), so cloth is partly in the shaders.

Shader families **[M]**: the `shaders` bundle holds `Owlcat/Lit` (4,270 Metal blobs, 101 MB
of MSL), `Owlcat/Particles` (7,902 blobs, 152 MB), `Owlcat/Decals/{Projector,FullScreen,
ProjectorGUI}`, `Owlcat/Terrain`, `Owlcat/Water`, `Owlcat/Unlit`, `Owlcat/Skybox/*`, UI and
TextMeshPro. The pipeline's own shaders are in `globalgamemanagers.assets` and the
`utility_shaders` bundle: `Hidden/Owlcat/DeferredLighting`, `DeferredReflections`, `HBAO`,
`ScreenSpaceReflections`, `Fog`, `FogOfWar`, `ScreenSpaceFogOfWar`, `ColorPyramid`,
`ApplyDistortion`, the post shaders (`UberPost` 640 blobs, `Bloom`, `BloomEnhanced`,
`LutBuilderHdr/Ldr`, SMAA, DOF, motion blur, `FinalPost`), highlighting and the
`CharacterAtlas*` bake shaders; plus 24 compute shaders (`LightCulling`, `DeferredLighting`,
`DepthPyramid`, `ScreenSpaceReflections`, `IndirectCulling`, PBD cloth, VFX Graph).

### Where the shader code is, and its form [M]

Unity 2020.3's Metal player stores each variant as **plain MSL text** (HLSLcc output), LZ4
compressed in the `Shader` object's blob (split into ~16 MiB segments for big shaders). Not
encrypted, not precompiled. Each subprogram carries its program type, global and local keyword
names, a small Metal header (`0xf00dcafe`), the MSL with entry point `xlatMtlMain`, then a
binding table (constant buffers with sizes and member offsets; texture, sampler and buffer
slots). Slots are explicit `[[buffer(n)]]`/`[[texture(n)]]`/`[[sampler(n)]]`; matrices are
`float4 hlslcc_mtx4x4X[4]` column arrays (glam's layout); colour outputs use
`color(xlt_remap_o[i])` with an identity default. Per-pass render state is serialized
separately, as constants (GBuffer: ZTest LEqual, ZWrite on) or **bound to material
properties** (`_CullMode`, `_ZTest`, `_SrcBlend`/`_DstBlend`, `_StencilRef`). UnityPy 1.25.4's
shader exporter fails on this blob version; the spike's `scripts/extract_msl.py` (~100 lines)
dumps it.

### The smoke test [M]

`render-spike owlcat` takes the `Owlcat/Lit` GBuffer vertex variant `DEFERRED_ON _NORMALMAP`
and fragment variant `DEFERRED_ON _MASKSMAP _NORMALMAP` (base, normal and masks maps, SH probe
volume; a typical character or prop material), **unmodified**:

- parses the MSL interface (bindings, `stage_in` attributes, varyings, struct layouts by MSL
  rules; the layout matches offsets in Unity's own binding table, unit-tested);
- builds one wgpu bind group layout whose entry order makes wgpu-hal's per-stage sequential
  slot counters land on the shader's slots (VS: 4 constant buffers; FS: 2 constant buffers,
  4 samplers, 4 textures including a 3D one);
- fills Unity's constant buffers (`UnityPerMaterial`, `UnityPerDraw`, `UnityPerCamera`,
  `UnityPerFrame`) by member name (24 members) with a reversed-Z projection as Unity uses on
  Metal, and synthetic textures: a **BC1** checker base map, a flat DXT5nm normal map, a
  constant masks map;
- draws a sphere into Owlcat's 6-target G-buffer (RGBA8 sRGB, RGBA8, RGBA16F, RG11B10F, RGBA8,
  RGBA8) and reads every target back.

At 81 interior pixels (`results/owlcat-passthrough.txt`): decoded normals match the analytic
sphere normal (worst dot 0.99997); 64/64 albedo samples are exactly the BC1 texel colour at the
analytic UV (17 near cell edges skipped); metallic (64), smoothness (127 = 1 − 128/255), the
shadowmask (255), translucency (128, 64, 32) and its flag byte (10) are within one byte of the
values the shader's arithmetic gives; emission is zero and baked GI is the SH ambient (0.2)
within 0.002. `scene --owlcat` does the same **inside Bevy**: built on Bevy's `RenderDevice`,
recorded every frame by a system in Bevy's root `RenderGraph` schedule beside Bevy's passes,
read back after 60 frames, same checks (`results/scene/owlcat-in-bevy-150/summary.txt`).

What this does **not** show: matched inputs from the real game, lighting, post-processing, or
that wgpu compiles the MSL the way Unity does. wgpu-hal compiles passthrough MSL with default
`MTLCompileOptions` (fast math on, latest language version); Unity's options are unknown and
can change results in the last bits.

### Recommended approach

**Run Wrath's own Metal shaders; port the pipeline, not the shading.** Re-deriving Owlcat's
lighting in WGSL or as Bevy `StandardMaterial`s couldn't be shown imperceptible without the
same comparison work, and would differ in many small ways. The game's MSL removes the
shader arithmetic from the list of things to match, leaving inputs, state and pass order.

1. **Offline (in `wr-import`):** extract every shader's MSL, keywords, binding tables and pass
   render state from Ember's install into a local shader pack under the gitignored cache;
   optionally precompile `metallib`s. Materials keep their keywords and properties unchanged.
2. **Runtime (a `wr-render` crate on Bevy 0.20):** Bevy is the host (ECS, windowing, assets,
   extraction, the wgpu device, camera driving). Owlcat content doesn't use Bevy's PBR or core
   3D pipeline: a custom camera schedule runs a port of `ClusteredRenderer`'s pass list, with
   every draw and dispatch on passthrough pipelines from the pack.
3. **The Unity layer to reimplement exactly:** keyword → variant selection (including its
   nearest-match fallback); the built-in constant buffers (`UnityPerDraw`, `UnityPerCamera`,
   `UnityPerFrame`, `_PerCamera`, `UnityPerPass`) and their semantics (`_ProjectionParams`
   flip, `_ZBufferParams`, reversed Z, `unity_WorldTransformParams`, lightmap ST, SH/LPPV,
   reflection-probe selection); Owlcat's light data, Z-bins and tiles; shadow atlas packing;
   sorting and render-queue order; skinning that matches Unity's to within rounding; render
   state from pass state plus material properties, as part of the pipeline key; Unity's
   Metal compile options.
4. **Verification:** Metal frame captures of Wrath-Mac give each draw's real constant buffers
   and targets; diff ours pass by pass, then whole frames with FLIP (POC-6).

Risks **[I]**:

- **Unity-native behaviour outside the shaders**: Shuriken particle simulation
  (`Owlcat/Particles`, 457 renderer slots in the caves), VFX Graph, skinning, culling and
  sorting, light-probe interpolation, reflection-probe blending, terrain. Each is a port.
- **Passthrough is unchecked**: a layout mismatch is silent garbage or a GPU fault. Mitigation:
  the interface parser (rejects slot gaps and unknown types), Metal API validation in debug
  builds, per-pass capture diffs.
- **Variant selection** must match Unity's; a wrong variant looks plausible but wrong.
- **Metal-only**, and tied to Unity 2020.3's HLSLcc conventions (fine for the goal).
- **Bevy CPU overhead** at 240 Hz (§2–3) matters more than GPU cost.
- **wgpu churn** around passthrough between versions.

### Effort for the Owlcat render path [I: unvalidated planning guess]

No pass has been ported end to end yet, so these ranges have no measured throughput behind
them; revise after the first real pass (e.g. G-buffer + deferred lighting for one caves view).
Claude/Opus implementing with Astra review, on top of the extraction spike's importer:

| Work | Guess |
|---|---|
| Shader pack (MSL, keywords, binding tables, pass state, all families and compute shaders) and variant selection | 1.5–2 weeks |
| Unity built-in constant buffers and per-object data (transforms, lightmaps, SH/LPPV, probes) | 2–3 weeks |
| Frame core: shadows (incl. tetrahedral), G-buffer, light culling + Z-bins, deferred lighting and reflections, skybox, fog, transparents, sorting | 4–6 weeks |
| Fog of war (render side), decals, HBAO, SSR + pyramids, distortion, highlighting | 3–4 weeks |
| Post stack (LUT builder, Uber, bloom, SMAA/FXAA, DOF, motion/radial blur, Daltonization) | 2–3 weeks |
| Skinning parity (incl. PBD deformation inputs), character atlas bake shaders | 2 weeks |
| Capture-diff tooling and pass-by-pass parity on the caves | 2–3 weeks |
| **Core frame at parity for the caves** | **~17–23 weeks** |
| Particles (Shuriken simulation) and VFX Graph | separate; likely 8–16 weeks |

## 7. BC textures [M]

`TEXTURE_COMPRESSION_BC` and `_SLICED_3D` are available (`supportsBCTextureCompression`);
BC1/BC3/BC5/BC7 (sRGB where applicable) allow `TEXTURE_BINDING`, `COPY_*` and filtering. In
the smoke test a BC1 texture uploaded as-is and sampled by the game's fragment shader returns
the exact texel colours. The shipped DXT1/DXT5 blocks can stay bit-identical. (Crunched
lightmaps still need Unity's crunch decoder: extraction spike.)

## 8. MetalFX [M]

`metalfx` (`results/metalfx.txt`): spatial, temporal and frame interpolation are all reported
supported. Each is created with `objc2-metal-fx` 0.3.2 on wgpu's own `MTLDevice` and encoded
into the raw `MTLCommandBuffer` of a raw-only wgpu command encoder, on wgpu-created textures
(Private storage, Metal's default hazard tracking, so no fences). Output 2560×1440 RGBA16F,
31 timed frames each; GPU time = command-buffer GPU start→end:

| Effect | Input | GPU mean / p99 (ms) | Check |
|---|---|---|---|
| Spatial 2.0× | 1280×720 | 0.52 / 1.20 | output is the input upscaled: MAD vs the true image 0.027 (flat grey 0.185, bilinear 0.012) |
| Spatial 1.5× | 1707×960 | 0.46 / 0.57 | same, 0.027 |
| Spatial 1.33× | 1920×1080 | 0.49 / 0.59 | same, 0.026 |
| Temporal 1.5× | 1707×960, jittered static scene | 2.39 / 2.77 | output is the scene: MAD 0.0068 (one frame bilinear 0.0066) |
| Frame interpolation | 2560×1440 | 5.4–5.5 / ~6.2 | **fails**: output equals the current frame (MAD 0.001) rather than the midpoint, for six motion conventions tried |

Notes: on this synthetic test image MetalFX spatial lands *further* from the true image than
plain bilinear (it sharpens edges for rendered content), and temporal accumulation showed no
measurable gain over one frame; neither is a quality verdict, only proof the plumbing works.
Frame interpolation needs a working reference (Apple's sample) before its cost can be
budgeted; if it works, 5.5 ms per generated 1440p frame exceeds a whole 240 Hz frame (4.17 ms),
so it only fits schemes like 120 → 240 Hz. One integration trap: wgpu lazily zero-fills
textures it believes were never written and can't see MetalFX's raw writes, so outputs must
first be initialised through wgpu (a clear), or the first wgpu read blanks them. Temporal
upscaling also needs Owlcat to produce motion vectors (camera motion vectors exist for its
motion blur; object motion is unclear). All options go in the menu labelled **Addition**.
`bevy_metalfx` wasn't built: it pins Bevy 0.19/wgpu 29.

## 9. Open questions

- Can a lean Bevy app (no PBR, few systems) hold a sub-4 ms CPU frame on a quiet machine?
  That decides 240 Hz.
- The original game's input latency and frame pacing on this display (baseline agent).
- Unity's Metal compile options for these shaders (check with a Metal capture of Wrath-Mac).
- Which keywords and variants the caves use (the importer's material census).
- Whether to install the Metal toolchain (~1 GB) for offline `metallib` builds.
- Whether Owlcat writes per-object motion vectors (MetalFX temporal and interpolation).
- How Unity's skinning rounds compared with ours (vertex diff against a capture).

## 10. Review

GPT 6 Astra (high) reviewed `spikes/render` and this document (read-only). Findings, checked
against the code and data:

| # | Finding | Verdict | Fix |
|---|---|---|---|
| 1 | GPU end marker (independent compute pass) isn't ordered after rendering; the trace shows it finishing ~0.8 ms before `upscaling` | Real | End marker is now a render pass on the swapchain texture; the bracket is reported as a timeline span, GPU cost comes from Metal System Trace; the earlier 2.4–2.6 ms figures were dropped |
| 2 | Bench pairs vertex variants by similarity and excludes skinned/PBD variants; "skinning isn't in the shaders" is wrong for PBD | Real | Blend weight/index attributes supported; Unity's per-stage keyword rule with counted nearest-match fallback; vertex coverage reported; PBD noted |
| 3 | "Exact" fidelity overclaimed from one synthetic sphere; compile options unverified | Real | Verdict reworded as an integration smoke test; compile options listed as a risk |
| 4 | Pipeline accounting misses upscaling's internal creation; gate ignored failed pipelines | Real | Post-load count now uses cache totals (all paths); gate fails on errors (treating `ShaderNotLoaded` as pending); limits of the gate stated |
| 5 | "Cold" salted runs mix cold front end with possibly warm back end and repeated sources | Real | Bench reports first-use vs repeated sources and the salt; doc separates front end and back end, cites the unsalted run for back-end cost |
| 6 | Trace summary inferred frames from timing; "display interval" mislabelled; join coverage unreported | Real | Groups by Metal frame number, reports join coverage, label removed |
| 7 | MetalFX checks were only mean colours | Real | Reference-based checks; interpolation turned out not to work in our test, reported as such |
| 8 | GPU spans called robust to contention; ~1.3 ms gap attributed to scheduling; 17–20 ms latency estimate unsupported | Real | Claims removed; acquire→displayed kept as the measured metric |
| 9 | 0.033 ms pass time didn't match the saved file; missing `gpu_passes.csv`; checks looser than described | Real | Numbers taken from saved files; checks tightened to exact expected bytes at 81 pixels incl. RT2–RT5 |
| 10 | Failed checks still exited 0 | Real | `owlcat`, `owlcat-bench`, `archive`, `metalfx` and `scene --owlcat` exit non-zero on failure |
| 11 | Swizzle publication race | Real (latent) | Original IMP published before the hook is installed, under a lock |
| 12 | Vsync period inferred from p01; Python percentile used round-half-even | Real | Period from `--refresh-hz` (default 240), vsync'd modes only; nearest-rank in Python |
| 13 | Effort estimates over-precise | Fair | Labelled as unvalidated planning guesses; archive patch size no longer guessed |

Astra found no lifetime bugs in the presented/completed handlers or the archive code, and
confirmed the bind-group slot mapping, MSL layouts and normal decode against wgpu-hal 30.
