# POC-7 step 3 (first part): units driven by the sim

Written 2026-10-19, stream `drive` (Sonnet 5.5). **[M]** measured here, **[C]** read in the decompiled code,
**[I]** inferred. Output frames are under `fixtures/captures/poc7/fight/` (gitignored); nothing here is
committed game data.

## Result

`wr-game --fight <fixture dir>` replays a recorded turn-based fight in `wr-sim` (the driven replay), takes from it
where every unit stands, which way it faces, whether it lives, lies down or is prone, and how fast it moves at each
60 Hz frame, and plays that on the pack's units: the locomotion player picks idle or walk from the speed, the
facing and position are applied to the skinned unit, and a camera follows the fighters. It shows the fight the
sim computes; the original's combat animations are not in yet (below).

- **`wr_sim::combat::replay::timeline`** (`crates/wr-sim/src/combat/replay/timeline.rs`): `run_timeline(world,
  trace)` is the driven replay with a read-only collector; it returns the replay's `Outcome` (unchanged) and a
  `Timeline` only if the run is valid. At the end of every frame (when the next `tick` record arrives, after the
  frame's controller phases and the vision, memory and navigation frame ends; and once more when the recording
  ends) it samples every unit that is in the game. An invalid run (`World::valid` false: something unsupported
  executed, or an RNG failure) gives `Invalid` with the reasons and no timeline.
- **`wr-combat replay FIXTURE --timeline`** collects it and summarizes it on stderr (`--timeline-out FILE`
  writes it as gzip JSON, `--out-dir DIR` moves the replay's usual outputs from `cache/poc5/`); stdout and the
  outputs are the same as without. `WR_TL_CSV=DIR` also writes one CSV per unit.
- **`wr-game --fight`** (`crates/wr-game/src/fight.rs`, `owlcat/skinning.rs`): see "Running it".

## What a sample is

One `Sample` per unit per frame, the end state of the frame in the replay's world:

| field | meaning | where it comes from |
|---|---|---|
| `position` | `UnitEntityData.Position` | `source` below |
| `orientation`, `desired_orientation` | `Orientation` (degrees about +Y, what the view's rotation is set to), `DesiredOrientation` | `source` below |
| `in_game`, `life_state`, `prone` | `IsInGame`; `State.LifeState` (0 conscious, 1 unconscious, 2 dead); `State.Prone.Active` | the sim (rules, the prone controller) |
| `in_combat` | `IsInCombat` | the sim |
| `moving` | the agent has a path or a requested one (`WantsToMove`, the recording's `mv`) | the sim's agent (checked against the rows in turn-based frames) |
| `speed` | horizontal metres per second between this frame's position and the last (×60) | derived from `position` |
| `agent_speed` | `UnitMovementAgent.m_Speed` | the sim's agent; meaningful where `source` is `Sim` |
| `command` | the class of `GetCommand(Move) ?? Standard`, an index into `Timeline::commands` | the sim's commands model |
| `source` | `Sim` or `Recording` | below |

`source` is per unit and frame. It is `Sim` when the frame is turn-based combat with the movement agents
synchronized (`w.combat.turn_based && ags.synced`) and the replay wrote nothing of the unit's position or orientation
from a recorded input in it: the agent (`TickMovement`), the physics (ground height), the paths and the orientation
writers are ports checked against the recording, so position and orientation are the sim's [C, M: every observation
row the recording writes in those frames equals the sample bit for bit, below]. Otherwise it is `Recording`: the
real-time stretches (the player's walk-up before each fight, the pauses between fights) that the sim doesn't compute,
where the position is the recording's observation row and the orientation what the replay took from the recording's
writes; and, inside a turn-based frame, the unit whose position or orientation the replay *took* from the recording
this frame. The driver marks those (`Driver::timeline_taken`, a set the collector owns; no replay decision reads it):
a place-on-ground step on a capture without `ground_view`, by the controller itself (`ag_ground` outside `TickUnit`) or
inside `TickUnit` (`MoveInputs::place_on_ground` returns the recorded position after), a position written by code the sim doesn't run (`ag_pos`), the agents' snapshots (`ag_init`,
`ag_sync`: every unit's position and orientation at the window's start and where real time ends), a span the replay
takes from the recording (`ag_ctl_end`, its `ag_rot` desired orientation), `UpdateSlowRotation`'s desired orientation
where the recording's value is used (without the `look` observer, or where `look` differs and is reported), a write of orientation by code the sim
doesn't run (`ag_look` taken: the angle is computed from a recorded point, or taken), and a roaming unit's step (its
recorded vector). A frame can hold both kinds: on `poc5-tb-113` (agent_sync, no `ground_view`) a unit is the
recording's among sim ones (`a_position_taken_from_the_recording_in_a_sim_frame_is_labelled_so`: 1,063 of its 1,873 frames since the place-on-ground step inside `TickUnit` is marked too (1 before)). Physics results
the replay takes from the recording where it doesn't compute them (`phys_taken`, `phys_span` in the coverage line)
set a unit's ground height; they are not marked per sample [I: the heights they give are the recording's, in frames
labelled `Sim`; the counts are 0 to a few hundred per run].

**A recording without the driven agents has no timeline.** The replay of a capture with no `agent` observer and
`agent_sync` (poc5-tb-95 and -96, the oldest) keeps the observation rows apart from the units
(`Driver::pos.last` is not `Unit::position`), so the units' positions would be the save's, still; `run_timeline` then
returns `Invalid` with "no movement agent records" whatever else is valid
(`a_recording_without_agent_records_has_no_timeline`). Filling samples from the rows in collector-owned state would
be possible; it isn't done: those captures predate the agent and aren't fights the sim moves.

## Collecting changes nothing [M]

The collector reads the world and is read by nothing: `&World` in, no decision of the replay consults it. Proof on
`poc5-tb-140` (`tb-lizard`, record run, `Manual_18_WR_FirstBattle`):

- `& `... --out-dir B --timeline`: the four outputs (`.oracle.t0.jsonl`,
  `.sim.t0.jsonl`, `.state-end.json`, `.compare-end.json`) are byte-identical (`cmp`), and stdout is identical but for
  the wall-clock milliseconds on its first line; it is also identical to the stdout of the binary built before this
  change (same capture, `valid=true`, equal=true, 1,673 records each side).
- `tests/poc7_timeline.rs` (ignored, needs the captures; `cargo nextest run -p wr-sim --test poc7_timeline
  --run-ignored only`) runs the replay both ways in one process and compares the oracle and sim streams, the diff
  and the unsupported kinds; they are equal.

## The timeline is the replay's own [M]

`the_timeline_is_the_replays_own_and_changes_nothing` (same suite), on `poc5-tb-140`'s record run: 11,213 frames
(ticks 0 to 11,212, one per `tick` record, consecutive), 21 units tracked (all those in the game), and

- every unit's last sample equals the world's end state (position and orientation bit for bit, life state, prone);
- at every observation row `obs u` the recording wrote in a frame the sim computed (the frame before and the row's own
  both `Sim`), the unit's sample at the end of the previous frame equals the row's position bit for bit: **4,549 rows,
  0 differ** (4,354 of them of a unit that is moving). Those rows are what the replay itself checks against the sim's
  position, so the timeline carries the checked values;
- units end dead and prone as the recording's end state says (the replay's own `compare` reports the end state at 21 of
  21, flags and postures with no difference), none of them dead at the start;
- 9,057 of the 11,213 frames are `Sim` for the first unit (9,061 before the per-unit marking: the agents' snapshots at the window's start and each synchronization are taken), the rest `Recording` (the walk-ups: the player's real-time walk to each fight,
  the pause between fights).

`a_run_that_is_not_valid_has_no_timeline`: one of those rows, in a frame the sim computed, moved by a metre: the
replay reports "an observation row's position differs from the sim's", `World::valid` is false and `run_timeline`
gives `Err(Invalid)` with that reason, no timeline. Unit tests in `timeline.rs` cover the rest without captures: the
samples are the world's and a recording without agent records has none; a unit written from a recorded input is
the recording's for that frame only; speed is the displacement per frame; an invalid world gives none; the file
round-trips; `check` refuses a track outside the frames (a start of `usize::MAX` doesn't overflow), an empty
track, frames [0, 60] or [0, 0, 1], a NaN and a command past the table, on write and on read; the decompressed size is
limited and a write is held to the same budget (a file over it isn't left behind); unit ids and command names are bounded;
the collector stops at the caps (frames, tracks, samples, command classes) while the replay finishes unchanged, and
`conclude` returns no timeline, with the reason, for a capped or inconsistent one (`wr-combat --timeline` and
`run_fixture_timeline` go through it); hook-level tests drive `place_on_ground`, `desired_orientation`, `ag_ground`,
`ag_pos`, `ag_look` and `ag_ctl_end` through the driver and check the unit is marked. In `wr-game`: a track of no poses with any start has none (no clamp panic), and the framing radius is the
extent in x and z.

## What was learned about the data [M]

- `command` is `GetCommand(Move) ?? Standard`: a moving unit nearly always has one, `UnitAttack` (the approach to
  its target is part of the attack: the party and the monsters) or `UnitMoveTo` (the player, Anevia, the monitor
  lizard); 29 of 7,822 frames with a speed above 0.1 m/s have none (a command that ended in the frame, or the unit's
  last step). Only these two classes occur in this run (`UnitMoveTo` 5,868 frames, `UnitAttack` 6,116 over all units
  [M, from the per-unit CSVs]); the commands model only knows the classes the recording shows it.
- The party's speed in turn-based combat is 2.032 m/s, in the walk-ups 3.048 m/s (the agent's `m_Speed`, equal to the
  displacement-derived `speed` to float rounding). In real-time frames `agent_speed` holds the last speed the sim's
  agent was synchronized with: ignore it where `source` is `Recording`.
- `Source::Recording` frames still have `moving` and a sensible `speed`, from the observation rows' positions.

## Running it

Pack (format v4) with the save's units (the fight is in `Prologue_Caves_1`, the area of Manual_13, so only the units
differ; the spec's ids are the sim's `UniqueId`s, which the timeline uses):

```sh
cargo build --release -p wr-import -p wr-visual -p wr-game -p wr-sim
target/release/wr-visual spec Manual_18_WR_FirstBattle -o cache/poc7/Manual_18.units.json
target/release/wr-import pack --area prologue_caves_1 --units cache/poc7/Manual_18.units.json \
    --out cache/poc7/packs/fight.wrpack                    # 26 s, 260 MiB
```

Play (the replay runs in the process under `scripts/heavy.sh`, which waits for memory; the game and the GPU
share `builds/.oracle-run.lock`, and the game must not be running):

```sh
scripts/heavy.sh lockf -k builds/.oracle-run.lock target/release/wr-game --windowed \
    --fight fixtures/captures/oracle/poc5-tb-140/fixtures/tb-lizard --run record \
    --pack cache/poc7/packs/fight.wrpack                  # live, at 60 frames a second, until the fight ends
... --at 0,100,1000,2300 --out <dir>                      # images at those frames (tick00100.png, ...)
... --frames N --fps F --t0 SECONDS --out <dir>           # N images F per second from t0
```

`--save NAME` overrides the capture's save (`run.json`), `--rate X` scales live playback, `--cam-yaw DEGREES` turns the
camera, `--look ex,ey,ez,tx,ty,tz` fixes it. The replay can be done once and kept:
`wr-combat replay FIXTURE --save S --run record --out-dir D --timeline-out F.json.gz` (under `heavy.sh`), then
`wr-game --fight F.json.gz ...` needs no replay and no memory. The file is gzip JSON, 0.3 MiB for this fight; it is not trusted when read: at most 1 GiB decompressed (checked while
reading, before parsing), 4,000,000 frames, 65,536 tracks and 8,000,000 samples, frames consecutive, every track non-empty
and inside the frames, numbers finite, command indices in the table (`Timeline::check`, also run on a replay's timeline
and on write).

## What the game side does

- **Placement** (`Skinning::set_drive`): the unit's draw constants (`unity_ObjectToWorld`) stay at the pack's
  placement, so the unit's new placement `T(position) R_y(orientation) S` (the scale of its own placement) is folded
  into its skin matrices as `placement⁻¹ · now`, and its shadow casters' boxes are moved by the same transform
  (the box of the moved corners). This is the walk demo's mechanism with rotation added; the same limits hold
  (anything reading the object's world position in a shader would not follow; nothing used here does).
  A view that forbids rotation keeps the identity rotation: `UnitEntityView.ForbidRotation` is a field of the unit's
  *prefab* (`UnitEntityData.InstantiateView_Internal` and `UnitMoveController.TickUnit` set the view's rotation only
  `if (!ForbidRotation)`) [C]. The importer reads it (`UnitPrefab::forbid_rotation`, already used for the placement)
  and the pack carries it now: `SkinBinding::forbid_rotation`, **pack format v4** (re-pack with `wr-import pack`; a v3
  pack is refused). `wr-game` doesn't turn such a unit and prints its id. **This is not the `ForbidRotation` fact
  component** (`Features.RotationForbidden`, also set by grapples and a cutscene command): that one only gates
  `UnitState.CanRotate`, which decides whether the sim writes the unit's orientation at all (a port, checked), so
  it is already in the timeline's orientation and no view rotation depends on it; the timeline doesn't carry it
  [C: `rg RotationForbidden decompiled/`: `UnitState.CanRotate` is its only reader]. None of the 21 units has the
  prefab flag [M: `wr-game` prints nothing for this pack]; an earlier version of this stage took the monitor lizard
  for one from its placement, which was wrong.
- **Locomotion**: `UnitAnimationManager.Speed` is fed the sample's `speed` (displacement per second), so the human clip
  locomotion and the monsters' controllers pick idle or walk exactly as in the walk demo. [I] The game's
  `GetSpeedAnimationCoeff` (race animation settings, size scale) is taken as 1, its walk type as Normal.
- **Facing** is the sample's `orientation` applied at once, as the view's rotation is (`UnitMoveController` sets the
  view's rotation to the unit's orientation each tick, after `UpdateSlowRotation`) [C]: the sim already turns it
  gradually, so the unit swings round with no turn animation.
- **Units that are down** (`life_state` not 0, or prone) keep the pose they had: their animation clock stops. A unit
  that is not in the game is collapsed (not drawn).
- **Camera** (`fight::Follow`): the pitch (43 degrees) and, unless `--cam-yaw`, the direction of the original's first
  recorded route pose; the target is the centre of the units in combat that are up (those that are up and moving if none is in
  combat, else all that are up) a metre above ground, and the camera backs off along its direction until the circle of those units (plus
  4 m) fits the 30 degree vertical field of view, never nearer than 24 m (the original's own distance is about
  that). Live play eases the eye and target (time constant 0.6 s); captures frame their tick exactly, so an image
  doesn't depend on the ones before it. It is not the original's `CameraRig` (no scroll, edge or key input,
  rotation, zoom, level bounds, rubber band, follow modes); and it does not avoid geometry: from some directions
  rock hides the fighters (the pictures below use `--cam-yaw`).

## Pictures [M]

`fixtures/captures/poc7/fight/poc5-tb-140/` (gitignored; `poc5-tb-140`'s record run, `Manual_18_WR_FirstBattle`, pack
`fight.wrpack`, `--cam-yaw 90`, 2560x1440, the caves are dark: no party light or probes yet). `tick00100.png`,
`tick00900.png`, `tick01000.png`, `tick02300.png`, `tick04100.png`, `tick07300.png`, one image per frame number
(`--at`); `run-fight.json` is the run's report. What they show, by eye:

- **`tick00100`** (real time: the player's walk-up before the first fight, `source` Recording): the party in the
  corridor, four or five figures walking the way the recording walked them; the camera frames the units that are
  moving (none is in combat yet).
- **`tick00900`** (first fight, the sim's frames; Seelah is moving): four party figures in the corridor and three
  centipedes strung along the ground below them, each on the spot the sim has it, bodies pointing along the
  corridor; the camera takes in everyone in combat, so the figures are small.
- **`tick01000`** (100 frames later): one figure (Seelah) has walked down the corridor towards the centipedes, about
  a tenth of the picture's height further than in `tick00900`; the others stand. The walk is the locomotion player's
  (idle to walk with the speed), seen only in motion, not in a still.
- **`tick02300`** (after the first centipede died at frame 2,267): the framing has shrunk to the survivors: two
  party members standing close, two centipedes still in front of them. The dead one's animation has stopped (below).
- **`tick04100`** (second fight, a party member crossing): the camera at this yaw has rock between it and the
  fight, only two small figures show at the middle of the picture. **`tick07300`** (third fight): the same, the
  fighters are small and far, a giant fly at the top. These are a limit of the camera (it follows the fighters at a
  fixed direction and doesn't avoid the cave's geometry); another `--cam-yaw` shows them (a yaw of 180 shows the
  third fight's fly and the party beyond a rock).

Each image is drawn from one tick of the sim's timeline: `--at 900` is frame 900 and the units are where
`timeline.sample(unit, 900)` has them. Not compared with the original's pictures (the original lights units with
probes and a party light we don't have, and we have no recording of its camera in these fights).

## What is not there yet (gaps)

Animation (what the player lacks: `docs/poc7/animation.md` section 6 lists the rest):
- **Dead, unconscious and prone units** show no death, fall or get-up animation: they keep the idle pose they had
  when they went down, upright, their clock stopped (seven of the run's units end dead and prone). Their facing and
  place are the sim's. Needs the death, knockdown and get-up actions of the `AnimationSet`s.
- **Attacks, spells, hit reactions, dodges and the casting poses**: nothing plays when a unit attacks or is hit; a unit
  walks up to its target, stands, and nothing happens until it moves on. The sim knows the commands and the events
  (`command` is in the timeline; rule results and `UnitAttack` states are in the replay), the animation side
  doesn't use them. Needs `UnitAnimationManager`'s actions (attack, cast, hit) and the weapon styles.
- **Combat idles and walks** (`InCombat`, `ActiveMainHandWeaponStyle`, the weapon style overrides), **walk types**
  (charge, slow, stealth) and the **speed coefficient**: units use the peaceful locomotion at Normal.
- **Turning on the spot**: a unit's facing changes at once, as the sim's orientation does (the sim turns it at the
  agent's angular speed, so it swings over a few frames); the game plays no turn clip for it either unless the
  controller has one [I: not checked].
- Weapons in hand, hand and foot IK, ground tilt, root motion, the idle variants and micro idles (stage 1 and step 2
  lists).

World:
- **Camera**: fixed pitch, a chosen yaw, no scrolling, rotation, zoom, bounds or geometry avoidance: not the
  original's `CameraRig` (step 3 of `characters.md`, section 4).
- **Units not in the pack** (summoned or spawned during the fight: the spec lists the units in the save) are not
  drawn; `wr-game` reports tracks without a unit. This fight has none. The size scale (Enlarge Person in other
  captures) is not applied to a view; Seelah in `poc5-tb-120` would look the wrong size.
- **Real-time stretches** are the recording's, not the sim's (`Source::Recording`), and the fight's start and end
  state depend on them. The timeline starts at the recording's first frame and is not a live game: nothing here
  takes input.
- A whole run is replayed before the first frame (about 15 s of the machine's time and 2 GB for this fight; the file
  option avoids it).
Tests that need the captures are ignored by default (`.config/nextest.toml` puts `poc7_timeline` in the capture
group).
