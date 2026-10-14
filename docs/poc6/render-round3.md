# POC-6 render round 3: realtime shadows (handoff)

Written 2026-10-11. Follows `render-round2.md`; implements its step 1 from `shadows-spec.md`.
**[M]** measured, **[C]** checked against the decompiled code, **[I]** inferred. Run outputs
are under `fixtures/captures/poc6/round3/` locally (gitignored). The screen was locked, so no
timings are reported.

## Result

FLIP vs `oracle-2` pass 1 [M]: round 2's build (no shadows) vs this round.

| Point | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | mean |
|---|---|---|---|---|---|---|---|---|---|
| Round 2 | 0.2517 | 0.1338 | 0.1015 | 0.1163 | 0.0766 | 0.1453 | 0.0987 | 0.0856 | 0.1262 |
| Shadows | 0.2526 | 0.1326 | 0.1015 | 0.1125 | 0.0705 | 0.1444 | 0.0995 | 0.0832 | **0.1246** |

Restricted to the pixels the shadows changed [M]: mean abs error vs the original 10.76 -> 8.51;
at point 4 the changed pixels' mean luma goes 22.9 -> 14.4 against the original's 15.0, at
point 7 24.4 -> 19.3 against 20.2. Points 0 and 6 get slightly worse: the original is much
brighter and hazier there (fog/volumetric haze we don't draw, particles), so removing a light
there moves us away; no sign of a shadow fault (single-light sweeps at point 0 only darken
regions the original lights by other means) [I]. Seen by eye: the pillar and mushroom-stalk
shadows at points 3, 4, 7 land where the original's do. A single spot was validated first
(`WR_SHADOW_KIND=spot WR_SHADOW_PICK=0`, atlas dump with `WR_DUMP_ATLAS=1`: a clean depth image
of the spot's view), then all lights.

Per point the planner allocates 6-13 entries (5-11 spots, 0-5 points, 9-27 faces), all 512
px tiles (every light projects large), 144-1318 caster draws; nothing dropped [M].

## What was built

- **Pack (format version 2, rebuild with `wr-import pack`).** Per material the `ShadowCaster`
  programs for both `GEOMETRY_CLIP` states (`Material.casters`, selected with the material's
  keywords; exact matches for all 47 materials that have the pass), per-submesh vertex
  bounds (`SubMesh.bounds`), `OwlcatLight.use_pipeline_settings`, `Light.rendering_layer_mask`.
  The report is now written next to `--out`.
- **`owlcat/shadows.rs`**: `MaxRectsBinPack` (best long side fit), the 128-px quantised
  resolution, guard angles, spot matrix, four tetrahedral point matrices, biases, the 48-byte
  `ShadowData` and 112-byte `ShadowMatrix` records, `_Clips`. Tests include "a point along
  `FaceVectors[f]` lands inside face f's triangle for both clip equations", which ties our
  rotation/placement construction to the original's constants.
- **`owlcat/mod.rs`**: depth atlas (4096^2 `Depth16Unorm`), comparison sampler, shadow
  buffers, the caster pass before the G-buffer (clear once; per entry/face viewport,
  dynamic-offset `ShadowCasterBuffer` record, caster draws with the materials' own programs),
  caster culling per face, shadow bits in `_LightPositionsAndPackedData.w`
  (`lights::build` now takes the visible list plus shadow slots), `SHADOWS_SOFT`
  DeferredLighting with its own bindings, `ShadowCommonBuffer` constants. Point lights use the
  raster slope bias `-DepthBias` through per-bias pipelines created at load.
- Switches: `WR_NO_SHADOWS=1`, `WR_SHADOW_KIND`, `WR_SHADOW_PICK`, `WR_DUMP_ATLAS`.

## Spec claims checked against the code

All [C] and found right: 64/256 limits (CS:13,15); visible-light order, caster-bounds boolean,
no bake-type test (CS:221-248); resolution formula (CS:469); spot and point matrices, rotation
products, FOVs, near*0.5, per-face light direction (CS:278, 387, 518); guard angles and their
caps (CS:483, 288, 391); bias formula incl. the 2.5 soft factor (CS:374) and bias-source rule
(`ShadowmapEntry.cs:45`); buffer strides 48/112; shadow bits and the index < 64 rule
(`ExtractLightDataJob.PackData`; it also has shadowmask bits at 9+, not needed here); the
caster state table and fade scale/bias (`ShadowCasterPass.cs`); receiver sampling, face
choice, Y flip, nine-tap filter (`blob4.metal`). Two corrections: (1) MaxRects free-list order
matters: the split pushes the strip below before the one at the right, so ties go to the
bottom strip (tested); (2) `GL.GetGPUProjectionMatrix` on Metal is Y negated and depth
`-0.5 z + 0.5 w`, verified on the route's exported matrices (the spec left it implicit).

## Review fixes (Astra, three P2s, all verified real)

- Caster draws now also require the light's and renderer's `rendering_layer_mask` to
  intersect (`ShadowCasterPass.cs:87`, `useRenderingLayerMaskTest = true`); `Caster` keeps the
  renderer mask. Test `disjoint_rendering_layers_do_not_cast`.
- `TwoSided` casters (`cast_shadows == 2`) draw with face culling off in the caster pipeline
  only. Test `two_sided_casters_disable_culling`.
- `Pack::validate` checks `Material.casters` program indices. Test
  `out_of_range_caster_programs_are_rejected`.
The cave's renderers all have cast mode 0/1 and layer mask 1, as do its lights, so the caster
draw set is unchanged and the FLIP numbers above stand (no re-run).

## Approximations [I] and captures to make

- **`screenRect`** is native; ours is the projected cone/sphere bounds clamped to the screen.
  Only matters below 512 px; all tiles came out 512. Capture: `_ShadowDataBuffer` contents.
- **Visible-light order and `GetShadowCasterBounds`** are native: pack order, and "a caster
  within range, light within shadow distance 50 of the camera, layer in the culling mask".
  Order affects atlas placement only; the bounds test affects which lights shadow.
- **Atlas format, clear value, comparison function** (`Depth16Unorm`, 0, `GreaterEqual`),
  **point slope-bias sign** (`-DepthBias`) and **settings** (shipped asset not imported; code
  defaults 4096/512/2.6/0.1/50). A Metal frame capture at point 4 settles all of these.
- Directional shadows are not implemented (the cave's light is inactive); skinned and PBD
  casters, and the point-seam coverage check, are open. Caster draws are recorded per frame
  (about 1.3k draws at point 4); a per-face render bundle is the obvious optimisation.

## Rerun

```sh
cargo build --release -p wr-import -p wr-game
target/release/wr-import pack          # format v2: the old pack no longer opens
# lock first; no game running
target/release/wr-game --windowed --out <dir>     # WR_NO_SHADOWS=1 for the baseline
```
