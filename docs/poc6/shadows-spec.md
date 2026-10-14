# POC-6: realtime shadows, implementation spec (static reading, not yet implemented)

Written 2026-10-11 by gpt-6.1-sol (read-only, `codex exec -m gpt-6.1-sol -s read-only`) for the
render round 2 handoff (`render-round2.md`), from the decompiled `Owlcat.Runtime.Visual`, the shipped
DeferredLighting and Owlcat/Lit ShadowCaster MSL, our pack and `crates/wr-game`. Everything here is
**[I]**: claims to check against the code before relying on them, and native-engine behaviour is
flagged as needing a Metal frame capture of the original. Spot-checked by Claude: `ShadowCasterPass.cs`
(per-entry/face state table, keywords, fade scale/bias) and the DeferredLighting soft-variant
bindings (`depth2d _ShadowmapRT`, `_ShadowDataBuffer` 12 words, `_ShadowMatricesBuffer` 28 words).

**Use Owlcat’s buffer-driven caster path, not Unity’s conventional `unity_MatrixVP` / `unity_LightShadowBias` shadow path.** The implementation needs one depth atlas, four-face tetrahedral point shadows, Owlcat’s exact structured-buffer ABI, and the shipped `ShadowCaster` material variants. DeferredLighting already contains the filtering and shadow-strength logic.

No files were modified. Findings below distinguish directly recovered behavior from native-engine behavior that needs capture validation.

## Evidence notation

Paths are abbreviated to keep references readable:

- `R/` = `decompiled/Owlcat.Runtime.Visual/Owlcat.Runtime.Visual.RenderPipeline/`
- `S/`, `L/`, `P/`, `D/` = sibling directories ending `.Shadows/`, `.Lighting/`, `.Passes/`, `.Data/`.
- `U/` = `decompiled/UnityEngine.CoreModule/UnityEngine.Rendering/`
- `DL/` = `cache/poc6/msl/Hidden_Owlcat_DeferredLighting/`
- `CAST/` = `cache/poc0-render/shaders/msl-all/Owlcat_Lit/`

All Rust/cache paths are relative to the repository root.

**Pack evidence:** I ran the requested `pack-info` command and queried its JSON with `jq`. Because `.wrpack` is binary, pack findings use JSON paths/asset identities rather than invented file-line numbers. The read-only command implementation is `tools/wr-import/src/main.rs:138`.

## 1. Light selection and quality

### Exact Owlcat selection

For each camera:

1. Unity performs native culling, producing `CullResults.visibleLights`.
2. Owlcat traverses that array **in its existing order**.
3. A light is considered only when `GetShadowCasterBounds(visibleLightIndex)` returns true.
4. `ProcessLight` rejects null `Light` objects and `LightShadows.None`; its supported branches are spot, directional, and point.
5. Allocation failure or exhausted buffer capacity drops that request.

**Limits:** 64 shadow entries and 256 matrices. One spot consumes one matrix, one point four, and one directional its configured cascade count. The declared `kMaxDirectionalShadowsCount = 4` is **not enforced** by the inspected allocation path.  
Evidence: `R/OwlcatRenderPipeline.cs:155`, `R/OwlcatRenderPipeline.cs:205`, `S/ClusteredShadows.cs:13`, `S/ClusteredShadows.cs:221`, `S/ClusteredShadows.cs:238`, `S/ClusteredShadows.cs:303`, `S/ClusteredShadows.cs:337`.

There is **no Owlcat shadow-priority sort** by distance, screen size, intensity, or strength. Screen size affects resolution, not ordering. The mean-Z radix sort belongs to clustered-light extraction and runs **after shadow allocation**. Shadow indices are attached to descriptors before those descriptors are reordered.  
Evidence: `L/ClusteredLights.cs:70`, `L/ClusteredLights.cs:100`, `L/ClusteredLights.cs:185`.

There is also **no explicit `bake_type == Realtime` test in `ProcessLight`**, nor a `shadowStrength > 0` test. Native visibility/caster culling determines what reaches this path. Do not accidentally exclude mixed lights by implementing a realtime-only gate. The existing Rust visibility approximation excludes fully baked lights, but is not an exact reproduction of Unity’s native result.  
Evidence: `S/ClusteredShadows.cs:248`, `L/ClusteredLights.cs:198`, `L/ClusteredLights.cs:217`, `crates/wr-game/src/owlcat/lights.rs:45`.

`GetShadowCasterBounds` computes an enclosing caster bounding box and reports whether the light affects any eligible shadow caster; Owlcat discards the returned bounds and uses only the boolean. Its implementation is native.  
Evidence: `U/CullingResults.cs:72`, `U/CullingResults.cs:130`, `S/ClusteredShadows.cs:230`. ([docs.unity3d.com](https://docs.unity3d.com/2020.3/Documentation/ScriptReference/Rendering.CullingResults.GetShadowCasterBounds.html))

### Quality and distance

The supplied shipped configuration, **All / 4096**, matches the C# defaults. Mapping is:

| Pipeline quality | Lighting keyword |
|---|---|
| Disable | Neither |
| HardOnly | `SHADOWS_HARD` |
| All | `SHADOWS_SOFT` |

Within `SHADOWS_SOFT`, a light’s `SoftShadows` flag still decides between filtered and single-comparison sampling. `HardOnly` does not rewrite the light’s flags during allocation.  
Evidence: `S/ShadowSettings.cs:9`, `P/ShadowCasterPass.cs:107`, `S/ClusteredShadows.cs:269`, `DL/index.tsv:5`.

Default settings are atlas 4096, spot/point maximum 512, directional cascade resolution 1024, one cascade, shadow distance 50, directional near-plane offset 2, depth bias 2.6, normal bias 0.1. These are **code defaults**, not an independently captured runtime asset configuration. `InitializeShadowData` copies asset settings with a multiplier of exactly 1.  
Evidence: `S/ShadowSettings.cs:12`, `S/Cascades.cs:10`, `R/OwlcatRenderPipeline.cs:189`.

The camera shadow distance is passed to native culling. Disabling shadows or setting distance approximately zero removes `CullingOptions.ShadowCasters`; disabling lighting suppresses the shadow pass.  
Evidence: `R/OwlcatRenderPipeline.cs:256`, `R/ClusteredRenderer.cs:239`, `R/ClusteredRenderer.cs:491`.

### Cave-specific result

Pack `/lights` contains **48 enabled, active, non-baked shadow-enabled candidates: 31 spots and 17 points**. Seven have zero strength—one spot and six points—and Owlcat’s explicit selection logic does not discard them. This is a pre-culling candidate count, **not a frame’s allocated-light count**.

The main directional light, asset `CAB-f316821e057eea494b8b74ddf78849e5:3059`, is serialized as realtime, soft-shadowed, strength 1, but **inactive and black**. Thus it does not cast realtime shadows in the imported scene state. `LightController` sets its color/intensity/strength/rotation/mask, but that method does not activate it; our port likewise preserves inactivity. Runtime activation elsewhere remains a capture question. The camera-rig spot has `shadows = 0`.  
Evidence: pack `/lights` and `/lighting`; `decompiled/Assembly-CSharp/Kingmaker.Visual.DayNightCycle/LightController.cs:281`; `crates/wr-game/src/owlcat/mod.rs:365`.

## 2. Atlas allocation

### Resolution and packing

Let:

```text
projectedPixels = trunc(max(camera.pixelWidth  * screenRect.width,
                           camera.pixelHeight * screenRect.height))
requestedPixels = projectedPixels * (point ? 4 : 1)
resolution      = requestedPixels > maximum
                  ? maximum
                  : max(128, trunc(requestedPixels / 128) * 128)
```

This is **128-pixel quantization, not power-of-two rounding**. Spot and point maxima come from pipeline settings; `OwlcatAdditionalLightData` provides no resolution override. Owlcat uses camera pixel dimensions here, not explicitly the scaled render-target descriptor.  
Evidence: `S/ClusteredShadows.cs:469`, `R/OwlcatAdditionalLightData.cs:10`.

Each frame resets a non-rotating **MaxRects** allocator and inserts requests sequentially using `RectBestLongSideFit`. It minimizes the maximum leftover side, then the minimum leftover side; ties retain traversal order. Occupied rectangles split intersecting free rectangles, followed by containment pruning. There is **no retry at lower resolution**, no persistent allocation, and no separate padded rectangle. Guard bands are handled through projection expansion.  
Evidence: `S/ClusteredShadows.cs:221`, `S/ClusteredShadows.cs:256`, `R/MaxRectsBinPack.cs:49`, `R/MaxRectsBinPack.cs:258`, `R/MaxRectsBinPack.cs:409`.

### Tile shapes

- **Spot:** one `resolution × resolution` rectangle.
- **Point:** one `resolution × resolution` rectangle containing **four triangular tetrahedral faces**, rendered with the same full-tile viewport—not six faces or four independent square allocations.
- **Directional:** one rectangle containing 1–4 cascades:
  - 1: `R × R`
  - 2: `2R × R`
  - 3: `3R × R`
  - 4: `2R × 2R`

Evidence: `S/ClusteredShadows.cs:254`, `S/ClusteredShadows.cs:305`, `S/ClusteredShadows.cs:337`, `P/ShadowCasterPass.cs:78`, `P/ShadowCasterPass.cs:83`.

Exact `_FaceVectors`, in face order:

```text
( 0,          -0.57735026,  0.8164966)
( 0,          -0.57735026, -0.8164966)
(-0.8164966,   0.57735026,  0)
( 0.8164966,   0.57735026,  0)
```

Evidence: `S/TetrahedronUtils.cs:13`.

Point faces use these clip-space XY scale/translation transforms, leaving Z/W unchanged:

| Face | XY scale | XY translation |
|---|---|---|
| 0 | `(1, .5)` | `(0, .5)` |
| 1 | `(.5, 1)` | `(.5, 0)` |
| 2 | `(1, .5)` | `(0, -.5)` |
| 3 | `(.5, 1)` | `(-.5, 0)` |

Two shader clip distances delimit each face’s triangle. The eight `_Clips` coefficients are:

```text
(-1,-1,0), ( 1,-1,0),
(-1, 1,0), (-1,-1,0),
( 1, 1,0), (-1, 1,0),
( 1,-1,0), ( 1, 1,0)
```

Evidence: `S/ClusteredShadows.cs:101`, `S/ClusteredShadows.cs:109`, `S/ClusteredShadows.cs:513`, `CAST/blob4243.metal:221`.

Directional cascade transforms use XY scales `(1,1)`, `(.5,1)`, `(.33333,1)`, or `(.5,.5)`. Translations are respectively:

```text
1: (0,0)
2: (-.5,0), (.5,0)
3: (-.66666,0), (0,0), (.66666,0)
4: (-.5,.5), (.5,.5), (.5,-.5), (-.5,-.5)
```

Preserve the literal `0.33333`/`0.66666` values for faithful reproduction.  
Evidence: `S/ClusteredShadows.cs:59`, `S/ClusteredShadows.cs:143`, `S/ClusteredShadows.cs:159`.

## 3. Matrices, buffers, bias, and light linkage

### Matrix construction

`worldToShadow` is **world → GPU clip space**, including the point-face/cascade placement transform. It does **not** include the final atlas rectangle scale/offset or the clip-to-UV conversion. Both caster and receiver use it.  
Evidence: `S/ClusteredShadows.cs:291`, `S/ClusteredShadows.cs:402`, `S/ClusteredShadows.cs:442`, `DL/blob4.metal:400`.

**Spot:**

```text
view = Scale(1,1,-1) * inverse(light.localToWorld)
projection = GPUProjection(
    Perspective(spotAngle + guardAngle, aspect=1,
                near=light.shadowNearPlane, far=light.range),
    renderIntoTexture=true)
worldToShadow = projection * view
lightDirection = -localToWorld.column2.xyz
```

Owlcat constructs this itself; it does **not** call Unity’s native spot-matrix helper.  
Evidence: `S/ClusteredShadows.cs:278`.

**Point:**

```text
view[face] = faceScale[face] * tetraRotation[face] * Translate(-lightPosition)
worldToShadow[face] = facePlacement[face] * projection[face & 1] * view[face]
```

- Alternating `faceScale`: `(-1,1,1)`, `(1,-1,1)`, `(-1,1,1)`, `(1,-1,1)`.
- Rotation products:
  - 0: `Rx(27.367805) * Ry(180)`
  - 1: `Rz(90) * Rx(27.367805) * Ry(0)`
  - 2: `Rx(-27.367805) * Ry(270)`
  - 3: `Rz(90) * Rx(-27.367805) * Ry(90)`
- Even-face horizontal/vertical FOVs: `143.9857`, `125.26439`, plus guards; odd faces swap them.
- Projection near plane: **`shadowNearPlane * .5`**; far plane: range.
- Per-face matrix light direction: **`-FaceVectors[face]`**, independent of the light’s rotation.
- Caster culling splits contain three tetrahedral face planes through the light position. Although initial setup assigns a range sphere, `CalculatePointLightMatrices` replaces the splits with these plane-based records.

Evidence: `S/ClusteredShadows.cs:27`, `S/ClusteredShadows.cs:361`, `S/ClusteredShadows.cs:387`, `S/ClusteredShadows.cs:518`, `S/TetrahedronUtils.cs:20`.

**Directional:**

```text
(view, projection, split) =
    ComputeDirectionalShadowMatricesAndCullingPrimitives(
        visibleLightIndex, cascadeIndex, cascadeCount, splitRatios,
        cascadeResolution, pipeline.ShadowNearPlane)

worldToShadow = cascadePlacement * GPUProjection(projection, true) * view
```

Copy the returned split sphere center, radius, and radius² into `ShadowMatrix`. Ratios default to `.067` for two cascades, `(.067,.2)` for three, and `(.067,.2,.467)` for four.  
Evidence: `S/ClusteredShadows.cs:423`, `S/Cascades.cs:15`.

Unity’s native helpers return light view/projection matrices and caster split primitives; directional fitting uses camera/culling information. Their exact fitting, snapping, and caster-volume treatment are not recoverable from the C# extern wrappers. A conventional orthographic cascade implementation is an approximation until captured.  
Evidence: `U/CullingResults.cs:76`, `U/CullingResults.cs:83`. ([docs.unity.cn](https://docs.unity.cn/Documentation/ScriptReference/Rendering.CullingResults.ComputeDirectionalShadowMatricesAndCullingPrimitives.html))

### Guard angle and bias

Port `CalcGuardAnglePerspective` literally. It uses filter width **5**, normal-bias guard maximum **4 for spots / .5 for points**, and maximum guards `180 - spotAngle`, `36.014297`, or `54.73561`. This guard is not calculated from the actual configured normal bias.  
Evidence: `S/ClusteredShadows.cs:287`, `S/ClusteredShadows.cs:389`, `S/ClusteredShadows.cs:483`, `S/ClusteredShadows.cs:499`.

Bias source selection is important:

- Default: pipeline `DepthBias` and `NormalBias`.
- Only if an additional-light component exists **and `UsePipelineSettings == false`**: use `Light.shadowBias` / `shadowNormalBias`.
- Absence of the component still means pipeline bias—not per-light bias.

Evidence: `S/ShadowmapEntry.cs:45`.

Computed matrix biases:

```text
worldUnitsPerTexel = frustumSize / entry.Viewport.width
depthBias  = -configuredDepthBias  * worldUnitsPerTexel
normalBias = -configuredNormalBias * worldUnitsPerTexel
if SoftShadows: multiply both by 2.5
```

`frustumSize` is:

- spot: `tan(spotAngle/2) * range`
- point: `tan(1.2565123) * range`
- directional: `2 / GPUProjection.m00`

Use the **whole entry viewport width**, including a directional light’s combined cascade rectangle. Point matrices subsequently force `depthBias = 0`; their depth offset instead uses raster slope bias.  
Evidence: `S/ClusteredShadows.cs:296`, `S/ClusteredShadows.cs:374`, `S/ClusteredShadows.cs:414`, `S/ClusteredShadows.cs:447`.

The shipped Lit caster applies world-space bias before projection:

```text
biasedPosition =
    worldPosition
    + lightDirection * depthBias
    + worldNormal * normalBias * (1 - saturate(dot(lightDirection, worldNormal)))
```

There is no corresponding receiver-normal bias in the inspected DeferredLighting sampling path.  
Evidence: `CAST/blob4234.metal:175`, `CAST/blob4243.metal:185`, `DL/blob4.metal:676`.

### Exact structured-buffer ABI

The MSL declarations are **raw word arrays**, not native Metal structs containing aligned `float3` fields:

```metal
struct _ShadowDataBuffer_Type
{
    uint value[12];
};

struct _ShadowMatricesBuffer_Type
{
    uint value[28];
};
```

Evidence: `DL/blob4.metal:84`.

Serialize the recovered fields at these byte offsets:

| Buffer | Offset | Field/type | C# value |
|---|---:|---|---|
| ShadowData, stride **48** | 0 | `matrixIndices: float4` | Matrix indices converted to floats |
| | 16 | `atlasScaleOffset: float4` | `(width/A,height/A,x/A,y/A)` |
| | 32 | `shadowFlags: int32` | Point bit 0; SoftShadows bit 1 |
| | 36 | `screenSpaceMask: int32` | Entry field; no assignment found in this path, initially zero |
| | 40 | `unused: float2` | Zero |
| ShadowMatrix, stride **112** | 0 | `worldToShadow: float4x4` | Four column vectors |
| | 64 | `spherePosition: packed float3` | Directional sphere center; otherwise zero |
| | 76 | `sphereRadius: float` | Directional radius; otherwise zero |
| | 80 | `sphereRadiusSq: float` | Radius² |
| | 84 | `lightDirection: packed float3` | Directions specified above |
| | 96 | `normalBias: float` | Computed bias |
| | 100 | `depthBias: float` | Computed bias; zero for point |
| | 104 | `unused: float2` | Zero |

Evidence: `S/ShadowData.cs:7`, `S/ShadowMatrix.cs:7`, `S/ShadowFlags.cs:6`, `S/ClusteredShadows.cs:200`, `S/ClusteredShadows.cs:95`.

Allocate **3072 bytes** for 64 shadow records and **28672 bytes** for 256 matrices. Avoid a Rust layout that silently pads the packed `float3` members.

### Common constants and light reference

Exact MSL common buffer:

```metal
struct ShadowCommonBuffer_Type
{
    float4 _ShadowmapRT_TexelSize;
    int _ShadowCurrentAlgorithm;
    float2 _ShadowFadeDistanceScaleAndBias;
    float3 _FaceVectors[4];
};
```

Its recovered Metal layout is 96 bytes: texel size at 0; algorithm at 16; padding at 20; fade at 24; face vectors at 32 with **16-byte stride**. `_ShadowCurrentAlgorithm` is unused by this variant and not set by the inspected binding code.  
Evidence: `DL/blob4.metal:71`; pack `/programs` → soft DeferredLighting → `/cbuffers`.

Set:

```text
_ShadowmapRT_TexelSize = (1/A, 1/A, A, A)
_ShadowFadeDistanceScaleAndBias = (10/D, -9)
```

For defaults: `(1/4096,1/4096,4096,4096)` and `(.2,-9)`. C# explicitly sets fade and face vectors; texel size is the texture-associated constant consumed by MSL.  
Evidence: `P/ShadowCasterPass.cs:122`, `P/ShadowCasterPass.cs:130`, `DL/blob4.metal:406`.

Light linkage is:

```text
_LightPositionsAndPackedData[i].w =
    bitcast<float>(existingFlags | 4 | (shadowDataIndex << 3))
_LightColorsAndShadowStrength[i].w = light.shadowStrength
```

Bit 2 indicates a shadow record; bits 3–8 contain its index. No allocation means bit 2 clear. Strength is **not** stored in ShadowData. The visible-index→shadow-index mapping must survive mean-Z reordering.  
Evidence: `L/ClusteredLights.cs:199`, `L/ExtractLightDataJob.cs:52`, `L/ExtractLightDataJob.cs:110`.

Our port currently packs only falloff/snap bits, although it already fills strength. That is the direct integration point.  
Evidence: `crates/wr-game/src/owlcat/lights.rs:77`, `crates/wr-game/src/owlcat/lights.rs:181`.

## 4. Caster pass and GPU state

Owlcat binds the atlas and clears depth **once**, then loops entries. For each entry it sets the full entry viewport; for each face/cascade it sets `_FaceId`, assigns the split, and calls `context.DrawShadows` with the original visible-light index and `useRenderingLayerMaskTest = true`. No per-face clear is performed.  
Evidence: `P/ShadowCasterPass.cs:35`, `P/ShadowCasterPass.cs:83`.

Per-entry/face state:

| State | Spot | Directional | Point |
|---|---|---|---|
| `GEOMETRY_CLIP` | Off | On if >1 cascade | On |
| `_ShadowFaceCount` | 1 | Set to count when >1 | 4 |
| `_Clips` | Unneeded | Cascade clip equations | Eight point equations |
| `_LightDirection` | Not set here | `(-forward.xyz,0)` | `(position.xyz,1)` |
| `_PunctualNearClip` | Not set | Not set | Original, unhalved near plane |
| `_OffsetFactor` | 0 | 0 | Configured entry DepthBias |
| `_OffsetUnits` | 0 | 0 | 0 |
| `_ZClip` | 1 | 0 | 1 |

All entries also receive `_ShadowEntryIndex`; all draws receive `_FaceId`.  
Evidence: `P/ShadowCasterPass.cs:45`.

**Do not add per-light `unity_MatrixVP`, `unity_LightShadowBias`, or `_LightPosition` writes as the core solution.** This pass does not set them. The inspected Lit caster reads the selected `worldToShadow`, matrix biases, and per-object transforms directly. Its vertex bindings are material CB 0, per-draw CB 1, caster CB 2, shadow data buffer 3, matrices buffer 4.  
Evidence: `CAST/blob4243.metal:150`, `CAST/blob4243.metal:181`.

The caster CB is:

```metal
struct ShadowCasterBuffer_Type
{
    float4 _LightDirection;
    float3 _Clips[8];
    int _ShadowEntryIndex;
    int _FaceId;
    float _PunctualNearClip;
    int _ShadowFaceCount;
};
```

Metal layout: 160 bytes; clips start at 16 with 16-byte stride; final fields at 144, 148, 152, 156. The geometry-clip vertex variant emits two `[[clip_distance]]` values—retain it unmodified.  
Evidence: `CAST/blob4243.metal:111`, `CAST/blob4243.metal:138`.

### Material and renderer eligibility

Select the material’s **ShadowCaster LightMode**, not GBuffer and not necessarily its pass name. Pack tags use uppercase `SHADOWCASTER`; Lit’s caster pass name is empty. Material alpha testing must remain active: the shipped alpha-test fragment samples `_BaseMap`, applies `_Cutoff`, and discards.  
Evidence: pack `/shaders/*/passes`; `tools/wr-import/src/pack/shader.rs:252`; `CAST/index.tsv:4240`; `CAST/blob4247.metal:86`.

The native draw list applies renderer activation/enabling, shadow-casting mode, light layer/mask restrictions, and split culling. Honor `Off`, `On`, `TwoSided`, and `ShadowsOnly`; TwoSided disables face culling, while ShadowsOnly must remain absent from ordinary visible rendering. Do not restrict casters to the camera-visible GBuffer draw list.  
Evidence: `U/ShadowCastingMode.cs:3`, `P/ShadowCasterPass.cs:87`, `crates/wr-assets/src/model.rs:422`. ([docs.unity3d.com](https://docs.unity3d.com/ja/current/ScriptReference/Rendering.ShadowCastingMode.html))

Static batching does not remove shadow eligibility. Reuse our existing combined-mesh handling: identity object matrix, original renderer’s selected combined submesh range, and material/property overrides. Pack counts are 624 `On`, two `Off`, with 608 statically batched renderers.  
Evidence: pack `/renderers`; `crates/wr-game/src/owlcat/mod.rs:891`, `crates/wr-game/src/owlcat/mod.rs:916`.

### Depth, culling, and raster bias

C# requests a **16-bit `RenderTextureFormat.Shadowmap`**, bilinear filtering, clamp wrapping, and a logical depth clear of **1**. The exact resulting Metal pixel format is native-selected.  
Evidence: `R/ClusteredRenderer.cs:427`, `P/ShadowCasterPass.cs:37`.

Metal uses **reversed Z**; `GL.GetGPUProjectionMatrix` performs the depth conversion. The port should therefore use GPU-space depth **near 1 / far 0**, with expected actual atlas clear **0**, depth test **GreaterEqual**, and comparison sampling **GreaterEqual**. The actual native clear and sampler equality mode should be confirmed in capture rather than inferred solely from the C# clear argument. Unity also reverses fixed-function depth-bias sign.  
Evidence: `S/ClusteredShadows.cs:289`, `S/ClusteredShadows.cs:541`, `crates/wr-game/src/owlcat/pipeline.rs:216`. ([docs.unity3d.com](https://docs.unity3d.com/2020.3/Documentation/Manual/SL-PlatformDifferences.html))

The packed caster passes specify ZWrite on, ShaderLab ZTest LEqual, ColorMask 0, `Cull [_CullMode]`, and `Offset [_OffsetFactor], [_OffsetUnits]`. Thus use material culling, not universally front-face culling. Most packed materials use `_CullMode = 2`—back-face culling. For points, start with reversed-Z slope bias **`-entry.DepthBias`**, constant bias zero; capture validates the native sign/application.  
Evidence: pack `/shaders/*/passes/*/state`, `/materials/*/floats`; `tools/wr-import/src/pack/shader.rs:84`; `P/ShadowCasterPass.cs:46`.

Directional `_ZClip = 0` requires depth clamping rather than ordinary clipping. In wgpu this corresponds to `DEPTH_CLIP_CONTROL` and `primitive.unclipped_depth = true`; its Metal backend maps that to `MTLDepthClipMode::Clamp`. The current pass-state schema does not preserve ZClip, and current pipeline creation ignores raster offsets.  
Evidence: `crates/wr-game/src/owlcat/pipeline.rs:242`, `crates/wr-game/src/owlcat/pipeline.rs:324`, `tools/wr-import/src/pack/shader.rs:84`. ([docs.rs](https://docs.rs/wgpu/30.0.1/wgpu/struct.PrimitiveState.html))

## 5. DeferredLighting sampling

### Bindings and coordinate conversion

The soft fragment requires:

```text
buffer 4: ShadowCommonBuffer
buffer 5: _LightTilesBuffer
buffer 6: _ShadowDataBuffer
buffer 7: _ShadowMatricesBuffer
texture 7: depth2d _ShadowmapRT
sampler 0: sampler_ShadowmapRT
```

This shifts `_LightTilesBuffer` from buffer 4 in the no-shadow variant to buffer 5. Build the layout from the selected program’s bindings; do not reuse the no-shadow layout.  
Evidence: `DL/blob4.metal:131`, `DL/blob3.metal:107`.

Sampling performs:

1. Decode shadow-present/index from packed light data.
2. Select matrix:
   - spot: `matrixIndices.x`
   - point: face with maximum `dot(FaceVectors[face], receiverPosition - lightPosition)`; ties prefer the lowest face index
   - directional: first containing cascade sphere, using radius²; no cascade blending
3. Multiply world position by the selected matrix and divide XYZ by W.
4. Convert XY from clip to UV, flip local Y when `_ProjectionParams.x < 0`, then apply `atlasScaleOffset`.
5. Compare against saturated GPU-space Z.

Evidence: `DL/blob4.metal:338`, `DL/blob4.metal:347`, `DL/blob4.metal:400`, `DL/blob4.metal:591`, `DL/blob4.metal:656`.

### Filter and sampler

Soft-flagged lights use the recognizable **5×5 tent PCF formulation implemented with nine weighted bilinear comparison samples**. It calculates subtexel-dependent weights and three paired sample positions per axis—not nine equally weighted fixed-offset taps. Hard lights, including all lights in blob5, use one comparison sample. There is no blocker search/PCSS in this path.  
Evidence: `DL/blob4.metal:405`, `DL/blob4.metal:429`, `DL/blob4.metal:452`, `DL/blob4.metal:471`, `DL/blob5.metal:398`.

`sampler_ShadowmapRT` is an externally supplied sampler used by `depth2d.sample_compare`. **There is no inline `compare_func` declaration in this MSL.** Its actual comparison function cannot be read from shader text. Provide a clamp-to-edge, linear comparison sampler, with expected reversed-Z `GreaterEqual`, and validate against capture. In wgpu, use `SamplerBindingType::Comparison` and `SamplerDescriptor.compare = Some(...)`, not the renderer’s current generic Filtering sampler binding.  
Evidence: `DL/blob4.metal:137`, `DL/blob4.metal:148`, `crates/wr-game/src/owlcat/pipeline.rs:33`. ([docs.rs](https://docs.rs/wgpu/30.0.1/wgpu/type.SamplerDescriptor.html))

### Fade and strength

Fade uses **linear camera eye depth**, not Euclidean distance to the camera:

```text
fade = saturate(eyeDepth * (10 / shadowDistance) - 9)
attenuation = 1 - shadowStrength * (1 - fade) * (1 - comparisonResult)
```

At distance 50, shadows fade over eye-depth 45–50. Missing shadow records yield fully lit attenuation. The directional path also returns lit for no containing cascade or nonpositive shadow Z.  
Evidence: `DL/blob4.metal:252`, `DL/blob4.metal:321`, `DL/blob4.metal:473`, `DL/blob4.metal:480`, `DL/blob4.metal:775`.

## 6. Minimal ordered implementation

1. **Extend pack metadata and shader selection.** Import actual pipeline shadow settings, `OwlcatAdditionalLightData.m_UsePipelineSettings`, light rendering-layer masks, and renderer bounds needed for caster culling. Existing light shadow values, renderer casting modes/masks, static-batch ranges, material overrides, and pass state already exist. Import caster variants for both `GEOMETRY_CLIP` states and required material/deformation keywords. The current pack contains caster pass metadata but **no caster programs**; `select_variant` only chooses GBuffer.  
   Evidence: `crates/wr-assets/src/model.rs:454`, `tools/wr-import/src/pack/mod.rs:649`, `tools/wr-import/src/pack/mod.rs:511`; pack `/programs`.

2. **Add atlas resources and ABI buffers.** Create a 4096² depth attachment/sample texture, comparison sampler, 64×48 shadow buffer, 256×112 matrix buffer, and 96-byte common CB. Start with supported `Depth16Unorm`; use a different format only as an explicitly documented approximation until the original format is captured. Extend named storage-buffer resolution and comparison-sampler layout handling.  
   Integration points: `crates/wr-game/src/owlcat/mod.rs:685`, `crates/wr-game/src/owlcat/pipeline.rs:33`.

3. **Share visible-light identity across shadow and lighting setup.** Build the unsorted visible list once; allocate shadows in that order; attach shadow indices to descriptors; then execute existing mean-Z sorting, LightDataCB filling, ZBins, and tiles. Preserve zero-strength allocation behavior. For exact >1024-light behavior, do not truncate the input before Owlcat’s full-list setup/sort.  
   Integration points: `crates/wr-game/src/owlcat/lights.rs:47`, `crates/wr-game/src/owlcat/lights.rs:153`, `crates/wr-game/src/owlcat/mod.rs:482`.

4. **Compute a per-camera shadow draw plan.** Reproduce MaxRects, projected resolutions, guard angles, tetrahedral matrices/planes, matrix biases, and fade. Cull casters independently of camera-visible geometry. Upload both structured buffers before any caster draws. Implement directional fitting separately, explicitly marked approximate until native matrices/spheres are captured.

5. **Render the atlas before lighting.** Clear once; draw each entry’s faces using immutable per-face caster constants and exact material ShadowCaster programs. Use clip distances, correct winding/material culling, point slope bias, directional depth clamp, and the existing static-batch submesh handling. **Do not use one repeatedly overwritten CB for all encoded faces:** current `scope()` would classify `ShadowCasterBuffer_Type` as camera-global, so add per-face/dynamic-offset handling.  
   Integration points: `crates/wr-game/src/owlcat/mod.rs:178`, `crates/wr-game/src/owlcat/mod.rs:520`, `crates/wr-game/src/owlcat/mod.rs:1576`; `CAST/blob4243.metal:142`.

6. **Select soft DeferredLighting and validate incrementally.** Replace the hardcoded `fs[]` role with `fs[SHADOWS_SOFT]` and corresponding bindings; the pack already contains these deferred variants. Validate a spot first, then all point-face seams, alpha-cutout/static-batched casters, overflow ordering, fade, and finally directional cascades. The shaders themselves need no edits.  
   Integration point: `crates/wr-game/src/owlcat/mod.rs:1179`; pack `/pipeline_programs`; `DL/index.tsv:6`.

### Required original-frame capture checks

- **Native selection:** visible-light ordering, baked/mixed eligibility, `screenRect`, shadow-distance culling, off-camera casters, masks, and runtime activation of the cave directional light.
- **Atlas state:** actual Metal depth format, clear value, sampler compare function/equality behavior, filters, viewport origin, winding, slope-bias sign, and directional depth clamp.
- **Uploaded data:** active ShadowData/ShadowMatrix records, constant-buffer padding, GPU projections, cascades/spheres, and actual runtime settings rather than defaults.
- **Native directional details:** projection fitting/snapping, split-plane construction, and successive-split caster rejection. Unity can omit casters fully covered by earlier splits. ([docs.unity3d.com](https://docs.unity3d.com/es/2020.2/ScriptReference/Rendering.ScriptableRenderContext.DrawShadows.html))
- **Boundary behavior:** point-face seams/guard coverage and unused directional matrix-index slots. Entries are pooled without fully resetting their arrays; also, the shipped directional shader computes cascade index 4 outside all spheres and accesses `ImmCB_0` before its later lit fallback. Do not silently invent a different sentinel convention.  
  Evidence: `S/ShadowmapEntry.cs:66`, `S/ShadowmapEntry.cs:81`, `DL/blob4.metal:372`, `DL/blob4.metal:474`.

## Summary table

| Concern | Required reproduction | Certainty / evidence |
|---|---|---|
| Selection | Native visible order → caster-bounds boolean → supported shadow-enabled light | Explicit C#; native eligibility opaque — `S/ClusteredShadows.cs:221` |
| Capacity | 64 light entries, 256 matrices; overflow drops requests | Exact — `S/ClusteredShadows.cs:13` |
| Quality | All → soft variant; per-light soft flag selects tent filtering | Exact — `P/ShadowCasterPass.cs:107`, `DL/blob4.metal:405` |
| Atlas | 4096² target; MaxRects best-long-side fit; 128-step punctual resolution | Supplied configuration + exact algorithm — `S/ClusteredShadows.cs:469` |
| Point shadows | Four tetrahedral triangular faces sharing one square tile | Exact — `S/ClusteredShadows.cs:387` |
| Directional | 1–4 cascades; native fitted matrices/spheres | ABI exact; fitting requires capture — `S/ClusteredShadows.cs:423` |
| Cave directional | Realtime-shadow settings exist, but inactive/black in packed state | Verified pack `/lights`, asset `…:3059` |
| Matrices | Column-major GPU clip matrices; face/cascade placement baked in, atlas rectangle separate | Exact — `S/ShadowMatrix.cs:9`, `DL/blob4.metal:400` |
| Bias | World-space caster bias; point depth bias replaced by raster slope bias | Exact formula; native raster sign capture — `CAST/blob4243.metal:185` |
| Buffer ABI | ShadowData 48 bytes; ShadowMatrix 112; common CB 96; caster CB 160 | Recovered C#/MSL — `DL/blob4.metal:71`, `CAST/blob4243.metal:111` |
| Depth/sampler | Reversed Z; expected clear 0, GreaterEqual depth/comparison; bilinear clamp | Direction established; native state capture required |
| Filtering/fade | Nine bilinear comparisons implementing 5×5 tent; eye-depth fade over final 10% | Exact — `DL/blob4.metal:405`, `DL/blob4.metal:480` |
| Port blockers | Missing caster programs, shadow bits/resources, comparison layout, per-face CB scope, raster offset/ZClip | Verified current code — `tools/wr-import/src/pack/mod.rs:511`, `crates/wr-game/src/owlcat/pipeline.rs:324` |