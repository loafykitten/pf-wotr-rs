# POC-5: `UnitEntityView.Corpulence` computed (stream "corp")

Status (2026-10-18): a unit's corpulence is the sim's: the view prefab's `m_Corpulence` times the view's size scale,
from the unit's prefab, its size and its scale percent, instead of the last `corp` read of the recording. Every `corp`
read of the 111 runs (the sweep's 24 captures) is checked against it. Tags as elsewhere: [M] measured, [C] from the
decompiled code, [I] inferred.

## What the game computes [C]

- `UnitEntityData.Corpulence` is `View ? View.Corpulence : 0.5f`.
- `UnitEntityView.Corpulence` (`UnitEntityView.cs:401`) is the saddled unit's `View.Corpulence` if the unit rides
  (`EntityData.GetSaddledUnit()`, `UnitPartRider.SaddledUnit`), else `Mathf.Max(m_Corpulence * GetSizeScale(), 0.5f)`.
- `m_Corpulence` is a serialized field of the prefab's `UnitEntityView` (default 0.5). `Object.Instantiate` copies it.
- `GetSizeScale()` is 1 if `DisableSizeScaling` (a property, not serialized: set on a polymorph's view by
  `UnitEntityData.CreateView` and `Polymorph.TryReplaceView`, and on a view `RecreateView` makes). Otherwise it is the cached
  `m_CachedSizeScale`, filled by `CalculateSizeScale()` at the first call after a reset. The view's `DoLateUpdate` calls
  it every frame (`UnitEntityView.cs:1687`), so a reset is refilled by the end of the frame.
- `CalculateSizeScale()` (`UnitEntityView.cs:1868`): with `original = Descriptor.OriginalSize` (the race's size, else the
  blueprint's), `size = State.Size`, `p = Progression.CurrentScalePercent`, `k = m_RacialPetScaleCoeff`:
  - `original == size`: `(1 + p if p > 0.01) * k`;
  - else `1`, divided (`size > original`) or multiplied (`size < original`) by `ResultReduceSizeScaleStep` once per step, times
    `1 + p` if `p > 0.01`, times `k`, then `OverrideSizeScale.GetFinalScale` (`Math.Min(.., m_MaxScale)`) if the view has one.
  `ResultReduceSizeScaleStep` is the prefab's `OverrideSizeScale.m_ScaleStepCoefficient`, else 0.66.
  Enlarge Person (Medium to Large) on a 0.5 prefab: `1 / 0.66 * 0.5 = 0.75757575`, the recorded value [M].
- `m_RacialPetScaleCoeff` is a private field set in `Start()`: 1 without a `Master`, else `BlueprintRoot.MountModelSizing.
  GetCoeff(master's race)` (the entries: dwarf 0.9, halfling 0.78, gnome 0.85, the others 1).
- **The cache is reset** (`ResetSizeScale`) by: `UnitDescriptor.UpdateSizeModifiers()` (every write of `UnitState.Size`,
  `AddUnitScale.OnTurnOn`/`OnTurnOff`, `TacticalCombatRider`), `UnitProgressionData.SetRace`, and the view's
  `OnDidAttachToData`, `OnWillDetachFromData` and `OnAreaDidLoad`. Nothing else: `UpdateScalePercent` and
  `ResetScalePercent` (`UnitProgressionData.PostLoad` calls the latter) do not, nor does `Start()` setting the pet coefficient.
  Every sim input changes right before a reset (or before the view exists), so a value is the inputs' as of the end of the
  frame of the last reset; an input that changed without one (none occurs in the sim's paths) would stay stale.
- A unit's view is `UnitEntityData.CreateView()` at a load and whenever the view is made again (`AttachToViewOnLoad(null)`:
  `ReplaceUnitPrefab`, `Polymorph.RestoreView`, `UpdateHasDummyView`): the polymorph's view if one is active; a doll's
  (`DollData.CreateUnitView`: `BlueprintRoot.CharGen.MaleDoll` or `FemaleDoll`, when `UnitPartDollData.Default.RacePreset` is set);
  else the prefab of `Descriptor.CustomPrefabGuid`, else `Blueprint.Prefab`, or `BlueprintRoot.OptimizationDummyUnit` if `HasDummyView`.

## What the sim computes

`combat/corpulence.rs` (`corpulence(w, u)`), with these inputs:

- **The prefab's settings** (`wr-import unit-prefabs`, `cache/sim/unit-prefabs.json`, schema `wr-sim-unit-prefabs-v2`): per
  asset GUID, the `UnitEntityView`'s `m_IsInGame` (as before), `m_Corpulence`, whether its game object has an
  `OverrideSizeScale` and its `m_ScaleStepCoefficient` and `m_MaxScale`. `DisableSizeScaling` and `m_RacialPetScaleCoeff` are not
  serialized. 15,087 `.unit` resources, 3,463 with a `UnitEntityView` in their bundle (the other 11,624 have none: the same
  split as the `m_IsInGame` table had), of which 378 have a `m_Corpulence` other than 0.5 and none an `OverrideSizeScale`. The importer keeps the code for it (a unit
  test builds one); `scripts/scene-cache.sh` rebuilds a table of another schema.
- **The doll prefabs** (`wr-rules-data`, `cache/sim/doll-prefabs.json`): `CharGen.MaleDoll` and `FemaleDoll` are asset indices
  into `BlueprintReferencedAssets`, whose entries hold the prefab's asset id and object (a `Character` in `blueprint.assets`
  itself); the `UnitEntityView` on its game object gives `m_Corpulence` 0.5 for both. The unit's doll gender and whether it has a
  race preset are hydrated from `UnitPartDollData.Default` (`Unit::doll_gender_female`).
- **The unit's prefab**: the one it was spawned with in this run (`Unit::view_prefab`); after its view was made again (a
  polymorph started or ended, the dummy swapped: `corpulence::view_replaced`) or for a loaded unit, `CreateView`'s order above.
  The spawning data's prefab id becomes `CustomPrefabGuid` (`UnitDescriptor` constructor, guarded by `!Body.IsPolymorphed`: the body
  is the placeholder's saved one, hydrated with its `IsPolymorphed`, and a polymorphed one keeps the blueprint's prefab; such a body
  makes the corpulence unknown, cause polymorph) so a view made again is the spawn's prefab, not the blueprint's. A placeholder saved with a doll
  (`Unit::spawned_with_doll`) has an unknown view origin, whichever prefab it names. `HasDummyView`
  is the vision module's (`Vision::dummy`: saved, flipped by `update_has_dummy_view`).
- **The size**: `OriginalSize` is derived on every read (`spawn::original_size`: `Progression.Race?.Size ?? Blueprint.Size`, from
  the unit's race and its *current* blueprint, which `PretendUnit` swaps; the stored `Unit::original_size` answers only when the
  content has neither). The HP size bonus, the sheet, `ChangeUnitSize` and the size part read it the same way. `State.Size`
  (`Unit::state_size`) is written by `set_state_size` and the size-change facts' `UnitPartSizeModifier`, and by `SelectRace.Apply`
  (`State.Size = OriginalSize` after `SetRace`, race changed or not: the setter's `UpdateSizeModifiers` and size event). The
  `State.Size` getter keeps `OriginalSize` as first read (`m_Size`), and the view's per-frame `Update` reads it, so the evaluator
  latches it through the same `spawn::state_size` the rules code uses: a race set later moves `OriginalSize` and not `State.Size`
  (the scale then follows the difference, as in the game). The latch happens at the first evaluation or other read in the sim,
  not at the game's first frame: a race set between the two is a window the sim cannot see.
- **The scale percent**: `AddUnitScale` is ported (`components/state.rs`: `UpdateScalePercent` with its 0.005 dead band,
  `ResetScalePercent`, then `UpdateSizeModifiers`); `UnitHelper.CreateUnitCopy` copies it.
- **The cache**: filled when first asked, reset where the game resets (`spawn::update_size_modifiers`, `levelup::set_race`,
  `view_replaced`); a value stays until then.

Each input the sim doesn't have makes the result unknown (`Cause`), counted by the replay, never a guess:

| cause | when |
|---|---|
| rider | the unit has `UnitPartRider`: its view answers with the saddled unit's, which the sim doesn't track (the part holds a reference the sim doesn't hydrate) |
| polymorph | a polymorph is active: the view is the polymorph's prefab, with `DisableSizeScaling` |
| doll | `UnitPartDollData` on a unit the sim didn't hydrate from a save (a spawned one): which doll isn't known |
| dummy view | `HasDummyView` unknown, or an area with parts (suppression makes dummy views) |
| no prefab, prefab unlisted, no prefab table | the blueprint names no prefab; the table has no single `m_Corpulence` for it or no entry; the table isn't built |
| size scale override | an `OverrideSizeScale` the importer found no values for |
| size | the original size is unknown, or a `UnitPartSizeModifier` came from a save (its facts aren't hydrated) |
| pet coefficient | a pet whose `MountModelSizing` table has an entry other than 1 (`Start` reads it once; whether `Master` was set by then isn't known) |

Not seen by the sim, so not unknown: a size or view change by code it doesn't run (an unported action such as
`RecreateUnitView`, a `ReplaceUnitPrefab` fact). The check below is what finds those.

## The check [M]

The driven replay (`replay/corp.rs`) takes every `corp` read of the recording (`uc_v` by the commands, the AI and the
engagement; `ag_v` by the agent's requests, whose unit is the request's; the movement simulation's `ctx` reads are the AI's own
steps) and, after the top-level entry that contains it (`Driver::record`), compares it with the sim's value for that unit with
the state the entry left. A unit that has a corpulence event in the entry (a size write, a fact added or removed, a view
replaced, the size scale reset, the scale percent set) is not compared: the read could be from either side of it, counted.
Equal means the same `f32` bits. A difference is reported (`driven replay: corpulence: ... is X in the recording (record N), Y in
the sim`) and the run is invalid. The summary prints `corpulence computed: reads, compared (equal, differing), unknown (units the
sim hasn't made, events in the entry, by cause)`.

Results (sweep of 2026-10-18 on the 24 captures, 111 runs, and `-140`): **8,663,220 reads compared on the 111 runs, all equal,
0 differing, 0 unknown** (no read of a unit the sim hasn't made, none in an entry with an event for its unit; the 1,428,207
reads of units with a doll were unknown before the doll prefabs were extracted). The values: 0.5 (the minimum) for most units,
0.7 and 0.9 for others by prefab, and 0.75757575 for Seelah after Enlarge Person (`-120`). `-140` (tb-lizard, with WrathOracle's
`bounds_tick`): 782,800 more reads equal on its five runs. Validity is the baseline's: only `-122`/tb-nearest
(record, replay-1, replay-2: the initiative tie) and `-135`'s `replay-3-perturbed` are invalid; `-140`'s five runs fail on the
line-of-sight geometry item another stream is fixing.

**The entity-bounds colliders' radii as a second check** (`-140`, each run: 7,832 `bounds_tick` records of units): `TickEntity`
leaves a radius within 0.05 of `Max(0.05, Corpulence)` whatever the view's scale, so the sim's value must put it there.
259 are exactly `Max(0.05, Corpulence)`, 7,573 within the band (a view whose scale isn't exactly 1: the radius made from
`c * scale` is kept by the hysteresis, e.g. 0.49999997 for 0.5), 0 outside, 0 not checked. This covers the units the fights
never read. It doesn't pin the corpulence more finely than the band; where a collider's scale is exactly 1 the two agree bit
for bit (the exact ones). **A `bounds_init` radius is not compared**: the band holds only after a tick of that collider, and the
controller ticks only the entities on its update list (a move, a game-state change, `HandleUnitSizeChanged`). A corpulence that
changed without one of those (`AddUnitScale` resets the size scale but raises no size event) leaves the old radius until
something queues the entity, so a capture starting then would be falsely invalidated.

Damaged records (`tests/poc5_corpulence.rs`, `-120`/tb-cast, 4 cases, each reported): the enlarged unit's reads left at the old
size, the same one bit up, one read of an unchanged unit one bit off, an agent request's read changed. `tests/poc5_agent.rs`'s
case that changes a unit's `corp` reads now finds them reported by the check (the grid no longer takes them).

Unit tests (`combat/corpulence/tests.rs`): the size scale of a size step, the percent's two branches, the override (only on the
changed-size path), the 0.5 minimum, the cache staying stale until a reset and refilling with the inputs of that time, the
percent's dead band, a rider (unknown) against a saddled unit (computed), dolls by gender, a dummy view, causes. Importer
(`prefabs.rs`): a view's settings, an override on its game object, disagreeing views.

## In the vision range

`UnitVisionRangeController`'s colliders get their radius from `Corpulence` (docs/poc5/vision.md). `Driver::vis_corpulence` now
takes the recording's read where it has a valid one (the last read after the last event the sim saw), else the sim's value;
where both exist they are compared at each phase (counted, a difference reported). A value the sim doesn't have stays unknown, so
nothing unknown makes a decision definite: the radius is unbounded as before.

Result on the 111 runs (baseline: the same sweep before this stream; the pair counts are the runs' `vision range:` line):

| | before | now |
|---|---|---|
| sets compared / differences | 3,152,227 / 0 | 3,152,227 / 0 |
| pairs decided given the start hypotheses, each equal to the recording's | 15,442,732 | 54,661,049 |
| left to the recording: corpulence (an unbounded radius) | 45,876,635 | 6,650,019 |
| left: position | 5,115 | 7,632 |
| left: view-scale band (all equal to the scale-1 answer) | 45,560 | 51,342 |

So 39,218,317 pairs moved from "left to the recording: corpulence" to decided, with 0 differences; the totals add up to the same
61,370,042 pairs. 7,021,949 unit-frames took the sim's value where the recording had no valid read; 2,454,028 had both and agreed
(0 differing); 0 had neither. A few thousand pairs went on to the position and band causes: they were hidden behind the
unbounded radius before.

**What is left under "corpulence" is not the corpulence.** The cause names an unbounded radius, and 6,411,822 of the remaining
6,650,019 pairs (96%) are *lost* colliders: units the sim has no position for (in a run without the driven agent, a unit with
no observation rows), whose collider's radius is unknown with the collider until an update the sim models (docs/poc5/vision.md,
"Lost colliders"). That is the vision range's rule and is unchanged: keeping the radius of a lost collider whose corpulence
history is known would loosen it (an update the sim didn't see may have run), so it isn't done here. `-140`, which has rows or
the agent for every unit, leaves 0 pairs for the corpulence (6,774,380 decided, 21,470 for the scale band on its five runs).
The run's line prints `pairs left for the corpulence that are lost colliders'`.

The recording stays the first source (the check passes, so the two agree wherever both exist), and the vision range's
guarantees are as before: a unit with no value is `None` and its radius unbounded.

## In the paths

The turn-based grid (`combat/paths.rs`) takes `Unit.Corpulence` from the sim now, the recording's last read before the record
only where the sim has none. It is as clean as the vision range: every box the grid queues is checked against the recorded one
(`nav_guo`), the erosion against `nav_tags`, and every read against the sim's value by the check above. `-114`/tb-nearest
(the capture with the `paths` observer): 63 corpulences taken from the sim on each of its runs, all three still valid, every
request exact. The AI's, the engagement's and the agent's corpulence leaves are still pulled from the recording (they are the
reads the check compares).

## What is left unknown, and what would close it

- **Riders and mounts** (cause rider): the saddled unit's identity, `UnitPartRider.m_SaddledUnitRef`. Hydrating the reference
  from a save would give it for loaded units; the mount and dismount actions (`UnitPartRider.Mount`) are not ported, so a
  WrathOracle observer on `Mount`/`Dismount` (the unit pair) would close the rest.
- **Polymorph views**: the polymorph's prefab (`Polymorph.GetPrefab`, a blueprint field naming a prefab link) and
  `DisableSizeScaling` (scale 1). The prefab's link is an asset id the sim could read; no run has one.
- **Pets with a master of a race with a coefficient other than 1**: whether `Master` was set before `Start`. An observer
  on `m_RacialPetScaleCoeff` would settle it.
- **`RecreateView` and `ReplaceUnitPrefab`**: `DisableSizeScaling` and the new prefab. Not ported; the check would report a
  recording that has one.
- **The view's scale** (the prefab root's `Transform.m_LocalScale` times the size scale), which would close the vision range's
  scale band: the importer would read the root's `Transform`; not done.
