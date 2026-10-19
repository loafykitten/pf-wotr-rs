# POC-7 characters: units in the Bevy build (survey, plan, stage 1)

Written 2026-10-18, stream `chars` (Sonnet 5.5). **[M]** measured here, **[C]** read in the decompiled
code, **[I]** inferred. Local outputs (packs, specs, screenshots) are under `cache/poc7/` and the
scratch folder; nothing here is committed data.

## Result of stage 1

`wr-visual spec Manual_13` hydrates the save through `wr-sim` and writes the 21 units that are in the game
(`cache/poc7/Manual_13.units.json`); `wr-import pack --units` bakes them into the area pack at their
saved positions and rotations; `wr-game` draws them through Owlcat's own G-buffer, unchanged. All 21 are
placed [M]: 15 plain skinned prefabs (centipedes, flies, the spider, the monitor lizard, earth
elementals), 1 baked character (Anevia) and 5 characters composed from equipment entities (the player,
Seelah, Lann, Camelia, Wenduag). Every unit is in its rig's rest pose (a T-pose): no animation yet.

- Pack: 256.6 MiB (the unitless pack is 160 MiB; the five atlases at 1024 px are most of the difference),
  built in 10 to 13 s; renderer ready 0.20 s, first frame 0.37 s [M, same machine and settings as first light].
- Route poses: the eight recorded poses now contain units at 0 (player, Seelah, Anevia), 1 (Camelia), 2, 3, 6
  (centipedes, a fly), 4 (Wenduag, Lann) and 5 (the spider) [M, by projecting the saved positions through the
  recorded matrices]. FLIP mean vs `oracle-2` is **unchanged** at this stage: 0.2529 / 0.1326 / 0.1017 /
  0.1125 / 0.0694 / 0.1442 / 0.1003 / 0.0832 against the unitless 0.2526 / 0.1326 / 0.1015 / 0.1125 / 0.0705 /
  0.1444 / 0.0995 / 0.0832. The units are a few hundred pixels in a 2560x1440 frame, the T-pose is not
  the original's idle, and the original's units are lit by light probes and a party light we don't have. The
  player's feet land on the original's player to within a few pixels at pose 0 (checked by eye on a side by side).
- Screenshot: `/Volumes/Storage/Wrath-Rust/scratch-chars/route-units/point0.png` (the caves from route pose 0,
  with the player; Seelah and Anevia are in the frame too, see "Differences from the original"). Close-ups from
  a free camera (`wr-game --look ex,ey,ez,tx,ty,tz`): `/Volumes/Storage/Wrath-Rust/scratch-chars/shot-a/point0.png`
  (Seelah, Anevia and the player), `.../shot-b/point0.png` (Lann), `.../close/sheet2.png` (four characters).

How to rerun (from the repo root, with the game installed and the main caches present):

```sh
cargo build --release -p wr-import -p wr-visual -p wr-game
target/release/wr-visual spec Manual_13                       # cache/poc7/Manual_13.units.json
target/release/wr-import pack --area prologue_caves_1 \
    --units cache/poc7/Manual_13.units.json --out cache/poc7/packs/prologue_caves_1.wrpack [--atlas-size 2048]
target/release/wr-game --windowed --mode shots --pack cache/poc7/packs/prologue_caves_1.wrpack \
    [--look ex,ey,ez,tx,ty,tz] --out <dir>
```

New pieces: `crates/wr-view` (the spec's types), `crates/wr-visual` (spec from the sim), `tools/wr-import/src/pack/units/`
(`mod.rs` prefab and placement, `skin.rs` CPU skinning, `character.rs` composition, `atlas.rs` and `image.rs`
the CPU port of the atlas shaders), `tools/wr-import/src/tree.rs` (survey subcommands `tree`, `obj`,
`shader-msl`), `wr-game`'s `--look`. Tests: skinning (identity, blends, normals, rigid and two-bone meshes),
the atlas blits and repaint against hand computed values, body part selection and .NET dictionary order, the spec
round trip, `look_at`.

## 1. How the original builds a unit's visual

### 1.1 The unit prefab and `UnitEntityView` [C, M]

`BlueprintUnit.Prefab` is a `UnitViewLink` (a weak asset id) naming a `<guid>.unit` bundle in
`Bundles/`: a prefab whose root GameObject carries `UnitEntityView` (corpulence, soft and core colliders,
`m_IsInGame`, animation coefficients), `UnitMovementAgent`, `Highlighter`, `ParticlesSnapMap`, `EntityFader`. The
sim already has the prefab's `m_IsInGame` (`cache/sim/unit-prefabs.json`). 15,130 `.unit` bundles exist. Two
shapes matter:

- **Plain skinned prefab** (monsters, elementals, spiders): the whole body is in the prefab. A rig under the root
  (`Animator` + `UnitAnimationManager`, a generic Mecanim rig of 20 to 150 bones), one to four
  `SkinnedMeshRenderer`s with their own material and 1 to 3 textures; the centipede is 388 objects in total. The mesh's
  vertex streams hold position, normal and tangent (float32), UV0 (half) and the skin (weights float4, bone
  indices uint32x4); `m_BindPose` per bone. Some prefabs hold a second, unrelated root (a mount's rig and armour
  variants): only the `UnitEntityView` root is the unit.
- **Character prefab** (`Character` component, no meshes): the party, humanoid NPCs. 1.2 below. A `Character` with a
  `BakedCharacter` (Anevia) carries its finished renderer in the prefab and the character system leaves it
  alone (`Character.LoadBakedCharacter`, `UpdateBodyEquipmentModel` returns early).

The view's world transform is set by the entity (`Position`, `Orientation` in degrees about +Y); its local scale is
the prefab root's times `GetSizeScale()` (1 unless the unit's size differs from its original). Units that stand far
from the party can carry the *optimisation dummy* prefab instead of a real view (`OptimizationLevel`); not
modelled in stage 1.

### 1.2 Owlcat's character system [C, M] (`Kingmaker.Visual.CharacterSystem`)

- **`Character`**: `m_Skeleton` (a `Skeleton` asset: per-bone scale, offset, `ApplyOffset`: the race and build; applied
  by `BoneUpdateJob` every late update, so scale always wins over animation), `AnimatorPrefab` (the rig, a
  hierarchy in `techart`: `Male_BaseSkeletonGeneric`, `Female_…`; `Character.Start` instantiates it under the unit with
  an identity transform), `m_SavedEquipmentEntities` and `m_SavedRampIndices`, `MaxAtlasSize`, gender and race enums.
- **`EquipmentEntity` (EE)**, a ScriptableObject in a `.ee` bundle (3,137 of them): `Layer`, `m_HideBodyParts` (flags),
  `ShowLowerMaterials`, `PrimaryColorsProfile`/`SecondaryColorsProfile` (lists of 1D ramp textures), `ColorPresets`,
  `BodyParts` and `OutfitParts`. A **`BodyPart`** has a `BodyPartType` flag, a renderer prefab (a GameObject with
  one `SkinnedMeshRenderer`, bones named like the rig's), a material (`Character_Diffuse_Cutout`, an `Owlcat/Lit` variant with
  `_NORMALMAP _MASKSMAP CUTOUT_ON`) and a list of **texture layers** (`CharacterTextureDescription`: diffuse, normal and
  mask textures, default and ramp masks, flags `UseRamp1Mask`…). A body part with no renderer prefab is texture-only
  (an overlay such as underwear or accessories). **`OutfitPart`s** are prefabs attached to bones (capes, shields,
  backpacks, props).
- **What equipment a unit has** [C]: the prefab's saved EEs; for a unit made in the character generator, `DollData.
  CreateUnitView` empties the doll prefab and adds the race preset's skin, the generator's clothes and the chosen
  EEs, then the ramps; every body item adds its blueprint's `KingmakerEquipmentEntity` (a *blueprint* of EE links by
  gender and race; `VisualSourceItemBlueprint` overrides; `ForcedRampColorPresetIndex` picks the EE's colour preset);
  a unit with `UseClassEquipment` (the main character, custom companions) then adds its equipment class' clothes.
  `UnitUISettings` can hide helmet, cloak, backpack (defaults: shown) and `ShowClassEquipment` (default off).
- **Composition at run time** (`Character.UpdateCharacter`, `BuildMesh`, `MergeOverlays`): sort the EEs by layer;
  drop EEs that no other entity leaves any visible part for; for each body part type keep the part of the last EE
  not hidden by another's `m_HideBodyParts` (a `Dictionary<BodyPart,EE>` whose enumeration order, with its
  freed-slot reuse, decides which parts are blitted last); combine the kept parts' meshes into one `SkinnedMeshRenderer`
  (bones matched by name to the rig; UV0 is copied unchanged, because every part's UVs already address the
  fixed 2048-pixel atlas layout, `CharacterAtlas.MapRect`); build three atlases (diffuse sRGB, normal, masks:
  `Hidden/CharacterAtlasGenerator` blits each overlay part's textures into the rectangle of its type, in overlay
  order, lerping by the diffuse alpha; `Hidden/CharacterTextureRepaint` first paints the grey-scale textures with the
  colour ramp chosen for the EE, mask by mask; `CharacterShadowBake`/`DiffuseBake` for `UseShadowMask`); compress to
  DXT5 on a worker (`DxtCompressorService`); copy the first part's material and set `_BaseMap`, `_BumpMap`,
  `_MasksMap`, `_UseNormalMapAtlas`. Atlas size comes from the texture quality setting (High: 2048) except that
  *LOD atlases* start every character at 256 and refine near the camera (`UnitAtlasService`).
- The shaders of the blits are in the player's `globalgamemanagers.assets` (`Hidden/Character*`); their MSL was read
  with `wr-import shader-msl globalgamemanagers.assets <id>` and the blend formulas are ported in `atlas.rs`.

### 1.3 Animation [C, M]

- `UnitAnimationManager` (on the rig's `Animator`) owns a `PlayableGraph` with a layer mixer of 10 inputs;
  `AnimationSet` (a ScriptableObject, `Human_AnimationSet` in `humananimation.animations`) lists *actions*
  (`UnitAnimationActionLocoMotion`, `…VariantIdle`, `…HandAttack`, `…CastSpell`, `…Prone`, …), transitions and attack
  sequences; each action plays `AnimationClipWrapper`s (a clip plus its event track) or an `AnimatorController`
  (`LocoMotion` uses one with blend trees over speed and move type, and per-weapon-style layers).
- **Clip storage**: Unity `AnimationClip`, 760 in `humananimation.animations`. They are **generic** (transform
  curves, not humanoid muscles): `m_MuscleClip.m_Clip` holds a `StreamedClip`, a `DenseClip` (30 Hz samples, e.g. 26 frames
  x 175 curves for a walk) and a `ConstantClip`, and `m_ClipBindingConstant.genericBindings` map each curve to a bone by
  the CRC of its path (`attribute` 1 position, 2 rotation quaternion, 3 scale, 4 euler) [M on `1H_Slashing_Walking`:
  0.8 s, looping]. Events live in `AnimationClipEventTrack` objects.
- Monsters use the same machinery with their own `AnimationSet` and clips in their prefab's bundles.
- Mecanim `AnimatorController`s (state machines, blend trees, avatar masks: 81 `AvatarMask`s sit in `techart`) are the
  one structure we have no reader for.

### 1.4 How NPCs and monsters differ from the doll [M]

| | prefab | body | rig | colour |
|---|---|---|---|---|
| player | `Human_Fighter_Baron` (a stand-in: the view is the **doll** prefab) | race preset skin + generator EEs + class clothes + armour EEs | `MaleHuman` rig and skeleton | ramps from `DollData` |
| Seelah, Lann, Camelia, Wenduag | `Companion_*` with a `Character` | the prefab's 7 to 10 saved EEs + armour EEs | human rig, their own `Skeleton` asset (`FemaleHuman_Seelah`…) | `m_SavedRampIndices` |
| Anevia | `BCT_Anevia_Base`: `Character` + `BakedCharacter` | baked: finished mesh and material in the prefab | embedded copy of the human rig | baked |
| centipedes, spider, lizard, flies, elementals | one skinned renderer under a generic rig | the prefab's own mesh | its own rig | the material's textures |

`UnitEntityView.UpdateBodyEquipmentModel`, `UpdateClassEquipment` and the doll path only exist for `Character`
prefabs. Weapons are separate again: `UnitViewHandsEquipment` instantiates each weapon's model prefab on the hand
locators (and sheaths it out of combat).

## 2. What the importer could read, and what stage 1 added

Already there: UnityFS bundles, serialized files with type trees, `Universe` (cross-bundle pointers), meshes (all 14
channels copied byte for byte), textures (BC, raw, crunch), materials and the Owlcat `Lit` variants chosen by keyword,
the pack format and its renderer, the location list (asset guid to bundle), blueprint content in `wr-sim`.

Added in stage 1: scene-graph reading for prefabs (rest-pose bone matrices), CPU skinning, body part selection and
atlas composition, the doll and item resolution in `wr-visual`, derived materials, the survey tools.

Missing (all needed later):

- `AnimationClip` decoding (streamed, dense, constant curves; the path-hash to bone mapping: `Avatar.m_TOS`), `AnimatorController`
  and blend trees, `AvatarMask`, `AnimationSet` actions and their selection rules.
- **GPU skinning and a per-frame bone palette** (the pack bakes a rest pose; the posed vertex streams keep the skin channels so
  nothing needs re-importing).
- Light probes on dynamic objects (units are lit by the ambient L0 only), the original's unit lighting (rim, `FogOfWar` dissolve).
- Weapons and `OutfitPart`s (capes, shields, backpacks, shoulder props), PBD cloth, hair physics, ragdolls and dismemberment.
- Race preset skeleton overrides for dolls (`BlueprintRaceVisualPreset.MaleSkeleton` is a direct asset reference in the
  blueprint: needs `BlueprintReferencedAssets`), per-item custom ramp indices and `HideEquipmentEntity` (saved, not hydrated by the sim).
- `UseShadowMask` texture layers (59 in this save's EEs; the shadow bake darkens parts of an atlas region).
- DXT/BC compression of the atlases (stage 1 stores RGBA8 with a box-filter mip chain) and the LOD atlases.
- Run-time state: which units are visible (fog of war), `m_IsInGame` changes, prone and dead poses, `Fader` alpha.

## 3. Approximations and differences from the original in stage 1

- **Rest pose**, not the original's idle or prone animation; the party's own idle pose differs visibly.
- Atlases at **1024 px by default** (`--atlas-size 2048` for the High setting), RGBA8 not DXT5, no LOD refinement.
- Blends are done in 8-bit with the sRGB handling of the GPU path, by the formulas of the MSL [C]; **no capture of the
  original's atlases exists**, so the composition is checked by eye and by unit tests, not by comparison (see the captures below).
  Seelah's forearm skin looks lighter than her face: either the data says so or a ramp index is wrong; a capture settles it.
- The player's skeleton is the blueprint prefab's (`MaleHuman`), not the race preset's.
- All 21 units are drawn. The original hides units outside the party's sight (fog of war): at pose 0 Seelah and Anevia are
  not visible in the original and are in ours; the corpse lying beside the player in the original is an animated scene
  prop (one of the 13 scene `SkinnedMeshRenderer`s the area pack still skips), not a unit.
- No weapons, capes, shadows from outfit parts, no selection ring, no unit highlights.
- Skinned units cast shadows through the same caster pass as static meshes (verified visually at the party).
- The sim says no unit of this save is prone or unconscious (`life_state`/`prone` are in the spec for later).

## 4. Plan: live characters, in order

Sizes are working weeks of one engineer, including the review rounds (Sol) the project does; ranges, not promises.

### Step 1: characters visible and positioned (done above; remaining hardening 1 to 2 weeks)

*Pass:* every unit of Manual_13 and of the POC fixtures' saves (Manual_18..20 contain the fights' units) placed; a close
look at each equals the original's *rest-pose* picture. *Comparison with the original:* (a) per-unit rendered mesh and the three atlases
dumped from the running game (`Character.m_AtlasRenderer.sharedMesh`, the three `AtlasTexture`s) compared with ours:
vertex count, bounding box, and per-texel difference (expect small, from the 8-bit blends and DXT5);
(b) FLIP over the eight route poses with the units: not expected to move much until animation and probes are in, so judge by (a).
Hardening: BC7 or BC3 compression of the atlases (pack size, load), light probe sampling for dynamic objects, weapons and outfit
parts, `UseShadowMask`, fog-of-war visibility, the optimisation dummy rule.

### Step 2: animated (idle, walk): 3 to 4 weeks

- Clip reader (dense, streamed, constant; `Avatar` TOS for path hashes; looping and root motion flags) and a sampler: 1 week.
- A minimal `AnimationSet` driver: pick the idle (`VariantIdle` by class and race), the loco-motion blend (walk speed vs `MovementSpeedCoeff`),
  event times; the `AnimatorController` reader for blend trees: 1 to 1.5 weeks. Combat, spells, prone and dialogue animations are the rest of the
  game's set and are *not* in this step.
- GPU skinning (compute producing the same vertex streams the G-buffer and shadow passes already read, a bone palette per unit,
  the skeleton scale applied after the clip): 0.5 to 1 week.
- Sim-to-view glue: one `UnitView` per unit entity, driven by `Position`/`Orientation`/agent state each frame (the sim's agent already exists).
*Pass:* pose equality with the original at fixed clip times, then in motion. *Comparison:* bone local transforms per frame of a few units (idle,
walking at a fixed speed) captured from the game compared with ours: max rotation error in degrees and position error, over a 10 s clip.
Visual: FLIP on a standing pose route and a short walk route; Ember's eye for the gait.

### Step 3: the game's camera and click-to-move with `wr-nav` paths: 4 to 6 weeks

- `CameraRig` (`Kingmaker.View`): modes, scroll with edge and key input and the rubber band, rotation, zoom (`CameraZoom`), bounds clamp
  (`ClampByLevelBounds`), `ScrollTo`/follow; we already render from the recorded matrices, so the port reproduces those from rig state: 1 to 1.5 weeks.
- Pointer and click handlers (`Kingmaker.Controllers.Clicks`): ground click to `UnitMoveTo` through the sim's command queue and the
  existing `wr-nav` path queries (turn-based paths are exact; real-time paths use the same graphs), group movement and formation, stop on arrival: 1.5 weeks.
- **The real-time movement agent is the unknown**: the sim computes the *turn-based* `TickMovement` exactly; the real-time `UnitMovementAgent`
  (avoidance, `ConnectedToObstacles`, group following, slow-down) was taken from recordings, never computed. A faithful live walk needs it: 2 to 3 weeks,
  or a deliberate simplification that is declared (the plan's fidelity contract puts movement in T1/T2, not T0).
*Pass:* a scripted click sequence (same inputs) ends where the original's units end, within the agent's tolerance; the camera matrices of a scripted
camera input script equal the original's. *Comparison:* oracle recording of a camera/click script (below).

### Step 4: selection and a minimal UI: 2 to 3 weeks

- Picking (the units' soft collider capsules in `wr-phys`, mouse ray from the camera), selection state, the selection ring (an Owlcat decal projector:
  decals are not drawn yet, so a flat ring mesh first), the hover highlight (`Highlighter`).
- A minimal HUD in Bevy UI: portraits row with HP, the selected unit's name, click-to-select, pause. Fonts and text from `wr-text`.
*Pass:* select any party member by click; the ring follows; the HUD reflects sim HP. *Comparison:* screenshots of the original's HUD for layout only; the UI
is a redesign inside the project's rules ("everything else is free to redesign but must be measurably better").

## 5. Captures the main session would need

1. **Atlases and combined meshes** of the five characters in Manual_13, dumped from the live game (a WrathOracle observer on `Character`: after
   `IsFinishAtlas`, write `m_AtlasRenderer.sharedMesh` vertex data and the three atlas textures as PNG/raw, plus `m_EquipmentEntities` names, layers
   and `m_RampIndices`): settles step 1's composition (ramp indices, UseShadowMask, the part selection, the original's atlas size and LOD).
2. **Rig pose per frame**: for chosen units, each bone's local TRS per frame (or world matrices) plus the animation manager's current action and clip
   and its normalized time, for an idle and a walk (a unit sent to a point), with the camera pose: step 2's reference.
3. **A camera and click script**: `CameraRig` state (position, rotation, zoom, mode) per frame and every pointer event (`IClickEventHandler`) of a scripted
   session in the caves (scroll, rotate, zoom, click-to-move one party member, select another): step 3's reference.
4. **Visibility**: per unit per frame `IsVisibleForPlayer`, `View.Fader` alpha and the fog-of-war mask at the route poses (for the stage-1 difference above).
5. **Screens with the units in the route**: a second route pass at the eight poses *after the units have settled into their idle* (already partly in `oracle-2`,
   but with animation-phase noise: capture the idle phase too).
6. For step 4: screenshots of the HUD and selection ring at a few resolutions.

## 6. Open items and risks

- The real-time movement agent (step 3) and the animation controller reader (step 2) are the two pieces with no existing code to build on.
- Character atlas size and LOD policy change the picture's sharpness; the "picture must not change perceivably" rule wants the original's policy
  captured, not guessed.
- Memory: five 1024 px characters add ~82 MiB to the pack; 2048 px atlases at RGBA8 would add ~330 MiB, so compression is needed before
  a party of 6 plus enemies at 2048 is sane (a party's atlases at DXT5 2048: ~ 5.3 MiB each channel).
- The sim's `UnitView` inputs (`CustomPrimaryRampIndex`, `HideEquipmentEntity`, the doll's polymorph) are saved in the file but not hydrated; `wr-visual`
  documents what it leaves out and reads the doll directly from the save JSON until the sim hydrates it.
