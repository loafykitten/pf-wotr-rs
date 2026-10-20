# POC-7 step 2: animated units (idle, walk)

Written 2026-10-18, stream `anim` (Sonnet 5.5). **[M]** measured here, **[C]** read in the decompiled code,
**[I]** inferred. Output frames are under `fixtures/captures/poc7/anim/` (gitignored); nothing here is
committed game data.

## Result

Every unit of Manual_13 (21: 15 plain skinned prefabs, Anevia's baked character and 5 composed characters)
is in the caves playing the idle loop its `UnitAnimationManager` would play for a unit standing still outside
combat, skinned on the GPU through Owlcat's own G-buffer and shadow draws. One unit at a time can walk a
straight line with its walk cycle (an offline demo, not gameplay): the player (human clip locomotion) and the
monitor lizard (controller locomotion) are captured.

- **Pack format v3** (`wr-assets`; `Pack::open` also checks that every skin weight is a number and every weighted bone index is one of the renderer's bones, and that a skinned mesh's position channel is a float32x3 inside a stream that holds every vertex, so the skinning pass cannot index out of range): `PackMeta` gains `rigs`, `clips`, `controllers` and `animations` (one
  `UnitAnimation` per animated unit); a skinned `Mesh` keeps its unposed vertex streams plus `skin` (32 bytes a
  vertex: four weights, four bone indices); a skinned `Renderer` carries a `SkinBinding` (the unit's animation,
  the rig bone of each mesh bone, the bind poses). Opening a v2 pack fails with "rebuild it"; tests cover the
  round trip, every reference check and the version gate (`crates/wr-assets`).
- **Pack size** 260.0 MiB (stage 1: 256.6): clips, rigs and skin are small. Import time is unchanged (about
  22 s) [M].
- **Runtime crate `wr-anim`**: clips (streamed, dense, constant curves; path-hash bindings), a rig and its pose
  (blend, world matrices, skin palettes), Mecanim controllers (states, parameters, conditions, cross-fading
  transitions, 1D blend trees), and the locomotion players (below). It reads no Unity file; `wr-import` decodes
  into its types and the pack carries them.
- **GPU skinning** (`crates/wr-game/src/owlcat/skinning.rs`): each frame the CPU poses every unit and writes the
  skin matrices of every skinned renderer into one storage buffer; one compute pass copies each renderer's
  position stream into a buffer of its own and replaces position, normal and tangent by the skinned ones (the
  other attributes of that stream pass through). That buffer is what the G-buffer and shadow caster draws bind as
  the vertex stream, so both passes see the animated mesh through the game's own vertex shaders. A test runs the
  compute shader on a three-vertex mesh and compares it with the CPU formula (skipped without a GPU adapter).
- **`wr-game --mode anim`**: `--frames N --fps F [--t0 S]` captures `N` frames at animation time `t0 + k / F`
  (animation advances in ticks of 1/60 s, the game's frame at 60 Hz; capture times snap to that grid, so rates that are multiples of 1/60 s land on ticks exactly and the simulation doesn't depend on the capture rate), from `--look` or the first route point; `--walk UNIT[,SPEED[,START[,DIST]]]`
  sends the unit with that spec id on a straight walk (default 3.3 m/s from 1 s for 6 m).

How to rerun (repo root, game installed, `cache/poc7/Manual_13.units.json` from `wr-visual spec Manual_13`):

```sh
cargo build --release -p wr-import -p wr-game
target/release/wr-import pack --area prologue_caves_1 --units cache/poc7/Manual_13.units.json \
    --out cache/poc7/packs/anim.wrpack
target/release/wr-game --windowed --mode anim --pack cache/poc7/packs/anim.wrpack \
    --look ex,ey,ez,tx,ty,tz --frames 12 --fps 6 --out <dir> [--walk 35fffa0a-06ba-4cba-be40-7914924be190]
```

Frames kept (gitignored): `idle/<unit>/frame000..011.png` (2560x1440, 12 frames at 6 fps: two seconds of idle),
`sheets/<unit>.png` (frames 0, 3, 6, 9, cropped and brightened x3: the caves are dark), `walk/player/`,
`walk/lizard/` (36 frames at 12 fps, the unit sets off at 0.5 s) with `sheets/walk-*.png` and `walk-*.mp4`.
A first-light style `wr-game` run takes about 3 s per sequence. Camera positions were chosen by eye per unit
(several units stand behind rock and coral); nothing in the picture compares with the original, which lights
units with probes and a party light we don't have (stage 1's list).

## 1. What the game plays for a standing or walking unit [C]

`UnitAnimationManager` (on the rig's `Animator`) always runs the **LocoMotion** action of its `AnimationSet`
(the first action of type `LocoMotion`); `Speed` (m/s) is the only input outside combat. The random idle
variants (`VariantIdle`, `MicroIdle`) are extra actions started by a timer and are not modelled. Two kinds:

- **`UnitAnimationActionLocoMotionHuman`**: plain clips, no controller. `Speed < 0.1`: `NonCombatIdleWrapper`
  (`Common_Idle`, 2.63 s, looping) starting at `Random.value * length`, speed 1. Walking: the clip of
  `NonCombatWalkWrappers[WalkSpeedType]` (Slow, Normal, Charge, Stealth; `SelectWalk` falls back Stealth to Slow
  and the others to Normal), played at `Speed * 10/33` unless the race has its own walk (the race entries inspected
  only override Charge's clip, so Normal is unaffected). A change of clip cross-fades over the action's
  `TransitionIn` (0.1 s). Characters have no set of their own: `Character.AnimationSet` is
  `BlueprintRoot.HumanAnimationSet`, `Human_AnimationSet` in `humananimation.animations`, whose first action is
  `Human_AnimationSet_Locomotion2`. The player, Seelah, Lann, Camelia, Wenduag and Anevia use it.
- **`UnitAnimationActionLocoMotion`** (monsters, from the `AnimationSet` on the prefab's own manager): an
  `AnimatorController`, driven by `WalkSpeed = Speed / DefaultSpeedMps`, `MoveType`, `Stopping`, `InCombat` and
  `IdleOffset` (`Random.value` when `m_HasOffset`). The five controllers here are one layer (index
  `m_DefaultLayerIndex`, weight one; the weapon style overrides are empty) of single-clip states Idle, Walk, Run,
  Charge, Stealth (each with a speed multiplier and, for walking states, the `WalkSpeed` parameter as speed; the
  spider and the elemental also have walk-into and run-into states left by an exit time) and transitions on
  `WalkSpeed` and `MoveType`, fixed durations from about 0.01 to 0.4 s. No blend tree with children appears
  in these units (the player supports 1D trees; 2D and direct trees are refused at import).

Stand-ins for `Random.value`: the idle start phase and `IdleOffset` come from a hash of the unit's id, so a
capture repeats (`phase_of`).

## 2. Clips [C, M]

`AnimationClip` objects are generic (transform curves; no humanoid muscles), `m_MuscleClip.m_Clip` holding

- a **streamed clip**: frames of `time, key count, keys`, a key being `curve index, c0, c1, c2, c3` (a frame at
  `-f32::MAX` carries the initial values, one at `+inf` ends the stream); curve value at time `t` is the cubic
  `((c0 dt + c1) dt + c2) dt + c3` of `dt = t - key time` for the latest key at or before `t`;
- a **dense clip**: `m_FrameCount` x `m_CurveCount` samples at `m_SampleRate`, interpolated linearly;
- a **constant clip**: values.

Curves are numbered streamed, dense, constant; `genericBindings` list the properties in that order, each taking
3 curves (position, scale, euler), 4 (rotation quaternion) or 1 (anything else), `path` being the CRC-32
(zlib polynomial) of the bone's path from the animator's root (`wr_anim::clip::path_hash`). All 843 clips of the
five monster bundles and `humananimation.animations` decode, none is legacy or compressed, none binds a
non-transform property.

**How the decoding was checked.** There is no native reference: Unity samples a clip inside the engine on a
live `Animator` (the `UnityPlayer.dylib` functions `wr-nav`'s `native_check` calls are pure maths on plain
arguments; an `AnimationClip` cannot be built outside the engine), and no oracle capture of bone poses exists
yet (`docs/poc7/characters.md` section 5, item 2, is that capture; it would settle pose equality). What was
checked (`wr-import anim-check <bundle>`):

- **Continuity of every streamed segment**: the cubic of each key, evaluated at the next key's time, equals the
  next key's value. A wrong reading of the coefficients (order, which key owns the segment) cannot do that over
  every segment of every clip: worst error over all 843 clips **2.4e-7** (float rounding); the unit test fails on swapped
  coefficients.
- Every sampled value is finite; sampled rotation quaternions have length 1 within 0.19 for the whole human
  set (0.09 or less for the monsters), the cubic interpolating components of unit quaternions between keys
  and Unity normalising at use (`apply_curves` normalises).
- Looping clips close: the first and last pose of the human idle differ by 4e-4 and the monsters' locomotion
  cycles by 3e-4 to 4e-3 (rotations compared up to sign); the worst loop gaps are clips that are not cycles
  (`Centipede_LoMo_Walk_Into` 0.25, `Ambient_Drunk_Walking` 0.65 among the human clips).
- Bindings resolve: for the 21 units every locomotion clip's bindings find bones by path hash, except
  properties of bones the rig lacks (the tendril bones on the no-tendril centipede, 12 of 90 on the elemental's
  idle, 3 of 200 on `FreeHands_Stealth`): the pack report lists them (`anim_notes`). Hash and path scheme
  therefore agree with the data.
- **The rig at rest equals stage 1's posed meshes** (measured before the review fix that gave Anevia's baked rig its skeleton overrides, which moves her slightly): for all 21 units, skinning the unposed mesh with the new
  rig's rest pose and bind poses reproduces the stage-1 baked positions to 5e-6 m (a throwaway comparison
  against the stage-1 pack; the plain prefabs also assert at import that each rig bone's rest matrix equals
  the prefab's transform).
- Visual: units hold natural idle poses, breathe and sway (frame differences of the party in the sheets), the
  flies' wings and the lizard's head and tail move, a walk has a gait and the shadow follows.

Not modelled: events (`AnimationClipEventTrack`), `m_LoopBlend`'s baking (the editor closes loops; nothing at
run time), mirroring, additive clips, root motion (the game's `Animator.applyRootMotion` is false [M: the
`AnimatorPrefab`'s `m_ApplyRootMotion`]; curves bound to the root path 0, which the elemental clips have, are
applied to the root bone like any other).

## 3. Rigs and binding [C, M]

A rig is every transform under the clips' path root with its rest local transform and the path hash. The
`Animator`'s own transform is the root, except for prefabs that nest the rig below it (the monitor lizard's
animator sits on `FakePivot`, above `Lizard_Monitor_RIG`): `bind_root` takes the descendant under which most of
the locomotion clips' paths resolve [I: the avatar's skeleton starts at the model's root; how Unity picks it is
not read]. A character's rig is its `AnimatorPrefab`, reset to the unit's origin as `Character.Start` does, with
the `Skeleton` asset's per-bone scale (and, with `ApplyOffset`, position) as **overrides** applied after the
animation, as `BoneUpdateJob` does every late update. Anevia's baked character keeps her `Skeleton` too
(`SkeletonUpdateService` updates every registered character): her rig has the same 165 overrides as the other
female characters; the rest check compares the rig *before* the overrides with the prefab.

A skinned mesh's bones are matched to rig bones by transform (plain prefabs) or by name (characters). A
character's body is merged from its parts as `Character.BuildMesh` does, now with the skin: the merged bone list
is `EnsureBones`' (a part's bone is looked up by name in the character's rig, the first miss ends that part's
mapping and leaves later bones on merged bone 0, a bone with the same name and bind pose within 1e-4 is shared),
weights are remapped into it. Stage 1 posed a missing bone with an identity skin; the game's behaviour is now
followed (no unit of this save has one).

The skin of a vertex is `sum w_i * rig_world[bone_i] * bind_i`, in the unit's root space; the renderer's matrix
(`T R S`, as in stage 1) places that space in the world. Unity skins normals and tangents by the linear part.

**Bounds.** The renderer's mesh bounds (used to cull shadow casters) cover the rest pose and eight samples of
every locomotion clip, grown by a tenth and 0.1 m.

## 4. The controller player [C, I]

`ControllerConstant` is read as compiled: parameters (`m_Values`, defaults, names from `m_TOS`), layers, state
machines (`m_StateConstantArray`, destinations indexing it; indices past it, which are exits, are dropped), states
(blend tree node 0, speed and its multiplier parameter, cycle offset or its parameter, transitions), conditions
(`If`, `IfNot`, `Greater`, `Less`, `Equals`, `NotEqual`). The player advances a state's clock by
`dt * speed * param / motion length`; an exit time is a crossing (a looping state with an exit time below one crosses
it every cycle, so a transition whose condition turns true after the crossing waits for the next one; a state
that doesn't loop has one crossing, and a condition that turns true after it never fires the transition). This follows
Unity's `AnimatorStateTransition.exitTime` page ("the Exit Time condition will be true" on the first frame past the
point and false on the next; below one checked on every loop, above one "only once"). The page does not address
non-looping states, and the behaviour is documented-but-unverified against the original until the pose capture
exists; the first transition (any-state ones first) whose conditions hold starts a
cross-fade of its duration (seconds for a fixed duration, else a fraction of the source state's length) during
which both states run and no other transition starts, then the destination becomes current. Layers blend over the
rest pose in order by weight; the unit sets layer `m_DefaultLayerIndex` to one and the others to zero (the
game's `UpdateAnimatorLayerWeights`). Not modelled: avatar masks and additive layers (the importer empties such
a layer, so a unit that chose one would stay at rest), triggers, interruption sources, sub-state machines, 2D and
direct blend trees, and the mixer's weight normalisation across actions (outside transitions the loco action
has weight one).

Cross-fades are linear; Unity's transition curve and the game's `AnimationManager` mixing are not verified.

## 5. The walk demo

`Skinning::set_walk` drives one unit's speed as a script: from `start` it walks at `speed` until it has gone
`distance` metres, then stops. The player's `speed` feeds the locomotion player (`UnitAnimationManager.Speed`),
the displacement is folded into the unit's skin matrices (a translation along its forward through the inverse of
its placement) and its shadow casters' bounds move with it; the draw constants (`unity_ObjectToWorld`) stay at
the start. So the picture is right, but anything reading the object's world position in a shader would not
follow (nothing used here does), and a longer walk would need the arena's per-draw constants rewritten. The
demo moves on a straight line over flat ground and does not consult physics or navigation: the unit follows
nothing and may clip geometry. The movement agent, click-to-move and the sim-to-view glue are step 3 (the units driven by the sim's timeline: `docs/poc7/fight.md`).

## 6. What is left

- Combat and weapon-style idles and walks (`m_WeaponStyleOverrides`), the VariantIdle/MicroIdle timers, prone,
  dead, stunned and dialogue animations, `InCombat` and `Stopping`, and `MoveType` from the movement agent.
- Hand and foot IK (`IKController`, `m_IsShouldHandIKFree`), root motion, avatar masks and additive layers,
  2D blend trees, clip events (footsteps, whooshes) and weapons in the hand (stage 1 draws none).
- The original's pose as a reference: the rig-pose capture of `characters.md` section 5 (bone local TRS per frame
  with the manager's current clip and normalised time) would settle pose equality and the cross-fade shape.
- Light probes on moving units, outfit parts, PBD cloth and hair, the optimisation dummy (stage 1's list).
- The idle phase is a deterministic stand-in for `Random.value`; the original's idle phases were never captured.
- Per-draw constants for a unit that moves far (see 5); the skinning pass runs every frame for every unit (its cost was
  not measured separately).
