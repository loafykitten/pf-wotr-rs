# Wrath-Rust: Pathfinder: Wrath of the Righteous on a Rust/Bevy engine, built for Apple Silicon

Handoff plan. Written 2026-10-10 for whichever agent continues this work, and revised the
same day after an adversarial review by GPT 6 Astra (high). Read it whole before
starting; don't re-ask Ember things answered here. The oracle (WrathOracle and its tools,
`oracle/`) and the original game's build it runs on (`builds/arm64-oracle`) are our
reference and our baseline. They came from the sister project `~/Developer/Wrath-Mac`, which
is now pf-wotr-silicon and is no longer used or touched from here (2026-10-12).

## Where things stand (2026-10-18, twenty-third session): start here

Done and on `main`, each implementation reviewed (GPT 6 Astra (high) through round 5, gpt-6.1-sol (high) from round 6)
until no finding was left. Sonnet agents did the first rounds; Opus took over where Sonnet kept inferring:
- **Twenty-third session (2026-10-16 to 18; Opus 5.5 (high) streams, then Sonnet 5.5 (high) from 2026-10-17).** Merged,
  each reviewed by gpt-6.1-sol (high) to no findings:
  - *Drezen* (stream drezen): Manual_10's companion spawners and the capital mode; the 13 companion toggles restart at
    the first tick as in the oracle; Drezen's scene cache.
  - *Boxes* (below, under the twenty-second session's streams): physics file format 3.
  - *Roaming* (stream roaming, `combat/roaming.rs`, `replay/roaming.rs`, `Phase::Roaming`, `docs/poc5/orientation.md`):
    `UnitRoamingController` in the turn-based driven replay (rolls, idle waits, arrivals, the sleeping unit's steps;
    `insideUnitCircle` draws), checked against `roam_roll`, `ag_pos`, `ag_look` and `uc_new`.
  - *Line of sight* (stream vision, `crates/wr-los`, `combat/vision.rs`, `replay/vision.rs`, `Phase::LosCache`,
    `docs/poc5/vision.md`): `LineOfSightGeometry` from the scenes' `FogOfWarBlocker`s (`wr-import los-pack`,
    `cache/sim/los/<guid>.wrlos`), the sight caches and `HasLOS`/`CheckLOS` for the engagement, the group memory and the
    AI: every recorded read, miss and clear equal on 96 runs. `CanBeInVisionRange` is computed since stream
    visrange (below).
  - *The memory's other leaves* (stream memleaves, `combat/senses.rs`, `replay/memory.rs`, `docs/poc5/memory.md`):
    `UnitMemoryController`'s apeace, pass, stealth, ready and the no-viewer part of seen computed and checked: 0
    differences on 111 runs; spawner initializers, panic, fog of war and unported producers left to the recording,
    counted.
  - *The vision range* (stream visrange, `combat/vision/range.rs`, `replay/vision_range.rs`, `docs/poc5/vision.md`):
    `UnitVisionRangeController`'s capsule overlap against `EntityBoundsController`'s sphere colliders (PhysX's sync
    points, the radius' 0.05 hysteresis, the result order, the 512-hit buffer). 15,442,732 pairs decided and equal to
    the recording on 111 runs, 0 differences. Left to the recording and counted by cause: the colliders' corpulence
    (most pairs: the importer doesn't read the prefabs' `m_Corpulence` yet), positions only known to Unity's
    tolerance, the view-scale band, polymorph and dummy-view replacements, the result buffer. Decisions are made
    given the window's start hypotheses, which the `bounds_*` observers below will replace. Sol: 20 rounds.
  - *Loot and the prone controller* (below).
  - *The camera-view pin* (`docs/poc5/tb-replay.md`, plans `poc5-tb10`/`-tb11`, captures `-136` to `-138`): extra and
    dead units' `IsInCameraFrustum` pinned to the recording each tick; `-136` valid on all five runs, the flips of
    `-137`/`-138` show the pin drives the saved `m_IsDeathRevealed`.
  - *`life::all_units` in `State.Units.All` order* (the pool's, then other unit entities), from Sol's review of loot:
    the awake list, the prone order and every other caller follow the game's pool instead of entity indices.
  - **Valid on every run** after the merges of 2026-10-17 (sweep: 106 runs; `check.sh` 659 tests; POC-5 suites 39/39):
    every capture of the twenty-second session's list, plus `-131`, `-134` and `-136`. Not valid: `-122/tb-nearest`
    (the known tie).
  - **`poc5-tb-140`** (a `poc5-tb10` tb-lizard run, 2026-10-18; five runs, all valid) records the new observers:
    `ph_col`'s `wcenter`, `roam_init`, `los_geom`/`los_probe`, `bounds_init`/`bounds_tick`/`bounds_set`/`bounds_scene`,
    `vis_order` and `phys_sync`. `-139` is a partial development capture of the same plan (superseded by `-140`). Read
    by the sim so far: `wcenter` (the box centres, below), `los_geom`/`los_probe` (88 segments and 1,280 probes equal a
    run) and, once stream corp merges, `bounds_tick`'s radii. Still unread: `roam_init`, `bounds_set`, `bounds_scene`,
    `vis_order`, `phys_sync` (next: readers that replace the vision range's start hypotheses with them).
  - *Box centres* (stream xform, `docs/poc5/movement-review-integration.md`): the importer's `TransformPoint` is
    Unity's hierarchy walk (`wr_phys::boxes::transform_point`), checked bit for bit against `-140`'s 62 `wcenter`s;
    `wr-los` format 2 (re-pack with `wr-import los-pack`).
  - *Records inside root rule spans* (stream audit, `docs/poc5/tb-replay.md`, `replay/spans.rs`, `replay/life.rs`):
    every record inside a driven root span is checked or reported, with one structural checker shared by the commands'
    spans and the audit; provokes, launches, envs, draws and mutations placed and attributed like a command's; fact and
    activatable handlers compared one to one with the sim's bus deliveries by rule instance (53,294 equal on 111 runs);
    singleton handlers checked against `replay/handler_table.rs` (generated by `scripts/gen-handler-table.py` from
    `decompiled/`); the oracle's own `obs invalid` note accepted and counted. Sol: 10 rounds.
  - *POC-7 stage 1* (stream chars, `docs/poc7/characters.md`, `crates/wr-view`, `crates/wr-visual`,
    `tools/wr-import/src/pack/units/`, `wr-game --look`): the save's 21 units in the caves at their saved positions in
    rest pose; characters composed as `Character.UpdateCharacter`/`BuildMesh`/`MergeOverlays` do, atlases from ports
    of `CharacterAtlasGenerator` and `CharacterTextureRepaint`. Sol: 3 rounds. Next stage (stream anim, running): idle
    and walk animation.
  - **Valid on every run** after the merges of 2026-10-18 (sweep: 25 captures, 116 runs, `-140` included; `check.sh`
    712 tests; POC-5 suites 40/40): all but `-122/tb-nearest` (the known tie).
  - Open: a replay's window can start one frame apart (523B's `TimeToNextRoundTick`; `docs/poc5/tb-replay.md`, end);
    loot's proposed observers (`loot_drop`, `ent_created`, `prone_step`, `tt_prep` Duration, `awake_set`,
    `docs/poc5/loot-and-prone.md`); the stat mutations outside any rule span (`stat_value`, `stat_mod_add`,
    `stat_mod_remove`: about 4,800 on the sweep) are compared by nothing yet.
- **Parallel streams (twenty-second session, Opus 5.5 (high) agents in git worktrees, `scripts/worktree-setup.sh`;
  only the main session builds WrathOracle and records).** Merged, each reviewed by gpt-6.1-sol (high) to no findings:
  - *The agent in the driven replay* (`combat/replay/agent.rs`; hardening A2 per
    `docs/poc5/movement-review-integration.md`, all 18 findings fixed): agents taken where real time ends (observer
    `agent_sync`: `ag_sync`, `ag_pos`); the commands' MoveTo/Follow/Stop run the sim's agent. `poc5-tb-113` valid on
    all six runs with the agent driven; older captures replay with the agent's records taken. `poc5_agent.rs` damages
    `-113` 69 ways; `-114` valid too (a UI guard change ending in a combat start or end is placed with it).
  - *Orientation* (`combat/orientation.rs`, `docs/poc5/orientation.md`): `LookRotation`/`ToEulerRad` native ports;
    `LookAt`/`ForceLookAt` not ported (an `ag_look` observer proposed).
  - *The AI's movement simulation* (`combat/ai_sim.rs`, `docs/poc5/ai-movement.md`): records `ag_sim`; the AI should
    use it once a fixture has an enemy walk several steps.
  - *Group memory* (`combat/memory.rs`, `docs/poc5/memory.md`): `UnitGroupMemory`'s bookkeeping, then slice H1:
    `UnitMemoryController.Tick` exact on all 42,306 recorded ticks of `poc5-tb-111` to `-113`; the sim's `AddToMemory`
    calls at offensive actions and command acts are matched against `mem_q`. H2: the memory computed in the driven
    replay (`Phase::Memory`, `Phase::Conscious`): 88,476 controller ticks and 474,876 reads by other models checked.
  - *Physics* (`crates/wr-phys`, `docs/poc5/physics.md`): PhysX 4.1's height-field and box raycasts, checked against
    the player on a million rays each; all 57,321 recorded `ag_phys` queries of `poc5-tb-100` to `-105` bit-identical;
    `wr-import phys-pack` writes `cache/sim/phys/<guid>.wrphys`. Boxes and meshes unverified (no recording reaches
    one). Not yet behind `AgentInputs::raycast`/`disc_cast` (waits for A2). Observer `phys_probe`.
  - *Turn-based paths* (`crates/wr-nav` `update.rs`/`astar.rs`, `combat/paths.rs`, `docs/poc5/paths.md`):
    CombatController's grid blocking and erosion, `ABPathWithClearance`, the seeker's modifiers. Observer `paths`
    (`poc5-tb-114`): all 129 requests exact, 87 with recorded line of sight; 440 deliveries, 1,206 graph events.
    The nav file is format 2. The UI's path visualizer (the party's follows) isn't ported.
  - *Manual_10's facts* (`docs/poc3/manual10-facts.md`): one-sided facts 35 -> 17, match rate 0.9987.
  - Fixes: `Vector3 ==` stores its sum before comparing; the oracle's tape reader takes a gate only from a boolean `g`.
  - *The combat log* (`crates/wr-sim/src/game_log/`, `docs/poc5/combat-log.md`, observer `combat_log`): every recorded
    message of `-115` to `-117` bit-exact (318); attack and damage tooltips not yet made.
  - *Fixtures* (`docs/poc5/fixtures.md`, plans `poc5-tb5` tb-chain, `-tb6` tb-lizard, `-tb7` tb-cast, `-tb8` tb-tie;
    the driver's `cast/<guid>/<unit>` and `seedinit:` steps). Saves Ember would need (an enemy walking several steps,
    impatience's hard target, iterative attacks) are listed there.
  - *Spellcasting and the fly* (`combat/spells.rs`, `docs/poc5/spells.md`, `Phase::Abilities`, `Phase::Visibility`,
    observer `inspect_reveal`): tb-cast (`-120`) and tb-chain (`-123`, `-124`) valid on every run.
  - *Oracle pins* (`docs/poc5/tb-replay.md`): projectile arrival (`ProjPin`), critter paths, the death effect's
    dismemberment (`DeathFxPin`, its draw taped) and every root rule checked against the recording (`Sched.CheckRoot`):
    tb-lizard (`-121`) is tick-exact in the game now.
  - *Initiative ties* (`combat/ties.rs`, observer `init_ties`, plan step `seedinit`): the comparers' identity-hash
    decisions are recorded inputs; tb-tie (`-127`) valid.
  - **Memory guards** (the Mac froze out of memory once): `scripts/heavy.sh` for every heavy run, capture tests serial
    (`.config/nextest.toml`), stream builds at 4 jobs. A long capture's replay peaked at ~8.5 GB; ~2 GB since stream O (`docs/poc5/replay-memory.md`).
  - *The AI's cast actions* (stream P, `docs/poc5/ai-abilities.md`): `BlueprintAiCastSpell` scored; the cast command.
  - *Critter rays, camera gates* (stream L, `docs/poc5/tb-replay.md`): critters' ground rays classified; the turn's
    camera-scroll gate and cutscene camera commands pinned (`CamPin`, observer `camera_gates`); the camera's view of
    extra and dead units checked. Open: `-134`'s replay-2 fails the view check (camera drift changes a dead body's
    reveal): pin the frustum results for those readers instead of only checking them.
  - *The agent's physics and paths in the driven replay* (stream M, `combat/replay/nav.rs`, `docs/poc5/movement.md`):
    physics queries, the AI's simulation steps, the turn-based grid and paths, place-on-ground (observer `ground_view`).
  - *The mesh raycast* (stream S, `crates/wr-phys/src/bv4.rs`, `docs/poc5/physics.md`): PhysX's BV4 triangle-mesh raycast
    from the arm64 player, bit for bit on 26M rays; the physics file is format 2 and takes the recorded transforms
    (`wr-import phys-pack … --colliders <a trace with ph_col>`). Boxes' Unity pose mapping still unverified.
  - *Boxes* (stream boxes, `crates/wr-phys/src/boxes.rs`, `docs/poc5/physics.md` "Boxes"): `TransformPoint(center)`
    and PhysX's normalised rotation, `max(|lossy * size| / 2, FLT_EPSILON)`, Unity's distance-0 post-filter; every
    probe and `ag_phys` query exact or failing closed (25 presentation queries left, none wrong). Physics file format 3.
  - *Loot and the prone controller* (stream loot, `docs/poc5/loot-and-prone.md`): `UnitProneController` (every unit's posture equals the recording's end state), `UnitReturnToConsciousController`, the combat log's `GameLogEventItemsCollection`, unidentified item names and the dropped bag's creation tick (its prefab, bounds and reveal a stub); `poc5-tb-131` and `-134` valid on every run.
  - *Facing* (stream T, `docs/poc5/orientation.md`, observer `look`): `DesiredOrientation` computed from `LookAt`,
    `ForceLookAt` and the orientation writers. Stopped at wrap-up after a clean review, before its final checks: the
    main session's merge sweep is its verification.
  - *Combat log tooltips* (stream Q): every recorded attack and damage tooltip equal.
  - *Manual_10* (stream R, `docs/poc3/manual10-facts.md`): area parts' suppression; one-sided facts 17 -> 0. The 13
    restarted companion toggles need Drezen's scene (`CompanionSpawner`s), not cached.
  - *Replay memory* (stream O, `docs/poc5/replay-memory.md`): compact `Rv` records; a long replay peaks at ~2 GB (was
    ~9-10 GB) and runs ~40% faster, outputs byte-identical.
  - **Memory guards**: `scripts/heavy.sh` for every heavy run, capture tests serial (`.config/nextest.toml`), stream
    builds at 4 jobs.
  - **Valid on every run** after the final merges (sweep at wrap-up; `check.sh` 614 tests, POC-5 suites 32/32): `-95`,
    `-96`, `-109`, `-111`..`-117`, `-120`, `-122/tb-focus`, `-123`..`-127`, `-129` (tb-lizard, every observer), `-132`,
    `-133` (with facing and the camera gates).
  - Not valid: `-122/tb-nearest` (a tie recorded before `init_ties`; `-127` covers ties), `-118`/`-119`/`-121`/`-128`
    (superseded tb-chain/tb-lizard captures from older observers), `-130/tb-nearest` (the camera-scroll drift the pin
    now fixes), `-134` (the camera-view drift, open).
  - Captures this session up to `-135`: `-129` tb-lizard with every observer through `ground_view`; `-131`/`-132` with
    `look`; `-133`..`-135` with `camera_gates` (`-135` the flipped-scroll test, `poc5-tb9`). Saves Ember could make (an
    enemy walking several steps, impatience's hard target, iterative attacks): `docs/poc5/fixtures.md`.
  - Next: the camera-view pin (`-134`); box colliders' pose mapping; Drezen's scene cache for Manual_10's companions;
    LookAt's remaining inputs (T's doc); then PLAN's list below.
- **The movement agent (twenty-second session, `docs/poc5/movement.md`): `TickMovement` exact against the game.**
  WrathOracle's new observer `agent` (`CommandObsAgent.cs`, three gpt-6.1-sol review rounds) records the agent's state,
  leaves, physics queries, path requests and completions; `poc5-tb-100` to `-103` (Manual_18, both fights) are clean.
  `crates/wr-sim/src/combat/agent.rs` ports turn-based `TickMovement` (`CanMoveInTurnBased`, `SlowDown`, `Move`
  without the navmesh clamp, `SetWaypoint`, the ends, `Stop`'s `ConnectToGroups`) with Mono's arithmetic and Unity's
  managed vector math (`decompiled/UnityEngine.CoreModule`), and the native `Vector3.RotateTowards` (`wr-nav`, checked
  against the player): rerun from each recorded state, **all 24,525 recorded turn-based ticks are bit-identical**, the
  agent's own calls and its calls into the view in the recorded order (gpt-6.1-sol: the observer four rounds, the
  model three).
  Then in the driven replay with `PathTo`, `OnPathComplete` and `UnitMoveController` (see the streams above).
- **Nav (twenty-second session, `docs/poc0/extraction.md` §9): every path query exact, smoothing included.** The
  funnel's `Quaternion.FromToRotation` is now a port of the arm64 player's own machine code (found through the icall
  table in `UnityPlayer.dylib`): single-precision magnitudes, Möller and Hughes' original `h = (1 - e) / |v|²`, double
  thresholds and Unity's own 180° case. On `check-1` and `oracle-3` 3,387 of 3,387 query checks are exact (real-time
  corners were 26/46, turn-based 32/46 and 44/46). `native_check.rs` (ignored test) `dlopen`s the player and
  compares our `FromToRotation`, `Matrix4x4.inverse`, `TRS` and `Quaternion.Euler` with its functions on a million
  inputs each: bit-identical. gpt-6.1-sol (high): no defects; its one residual (subnormal angles in `Euler`) fixed and tested.
  **Then `crates/wr-nav`:** the runtime (search, nearest, modifiers, queries, `cs`) moved out of `wr-import`, which keeps
  the cache decoder and builds the `World`; `wr-import nav-pack --area <name> --guid <guid>` (also run by
  `scripts/scene-cache.sh`) writes the area's nav file, `cache/sim/nav/<guid>.wrnav` (postcard behind a checked header,
  1.2 MB for the caves). `wr-import diff --nav-file` runs the oracle's queries on the world read from it: still 3,387/3,387.
  gpt-6.1-sol (high): the move preserves behaviour; its three findings (a temporary name shared within a process, the
  scenes' `AstarPath` settings unchecked by `--nav-file`, a test race) fixed.
- **POC-4 round 13 (twenty-first session, `docs/poc4/dialogue.md`): facts' handler interfaces.** No fact had ever
  subscribed to the event bus (the save schema drops handler interfaces, as it did for `EtudesSystem`):
  `rulebook/bus.rs` `FACT_HANDLERS` gives `ActivatableAbility` its ten (and `KingdomMoraleFlag` its one), every
  `ActivatableAbility` handler is ported from the C# (`unit_facts::activatable_event`, `activatable_rule`, and
  `OnDidTurnOff`, which was a blanket report), and the commands model now raises the run, act and end command events
  where the game does. Regression identical except Manual_10's two `OnDidTurnOff` reports, gone; every POC-4 and
  POC-5 run stays valid with the handlers delivered (the fights reach `OnAttack`/`OnHit` on Inspired Rage and Rapid
  Shot, no-ops as in the game). gpt-6.1-sol (high): no findings.
- **POC-4 round 12 (twentieth session, `docs/poc4/dialogue.md`): Camelia, a dialogue started by a cutscene.** Last
  session's `poc4-camelia-1` turned out to be Seelah's meeting (the walk enters her trigger first); Camelia's trigger also
  lies behind `LootFirst`, which turns the party back until it loots a cloak. Ember made `Manual_20_WR_PreCamelia`
  (hydration 1.0000): on `poc4-camelia-6` (walking into her trigger) **all 4 runs are valid** with the whole T0 stream,
  end state and units equal, as on the development capture `poc4-camelia-5` (Seelah's meeting, then a teleport). WrathOracle records `cs_dialog` and `cs_entry` (`cutscene_commands`) and the caller
  chain of any mutation in Pause mode (`paused_origins`); the sim starts a cutscene's dialogue and skips commands where
  the recording did, orders parallel tracks by the recording, makes the zone entries no step claims (other units, a
  teleport) at their ticks, and ports Camelia's recruitment (identification, `AddUnitFactEquipment`, `CasterProperty`,
  `Conditional`, `DealDamage`). Found and fixed: **`EtudesSystem` never received its events** (the save schema drops
  handler interfaces): flags, recruits, party and quest changes now mark its conditions dirty (`ENTITY_HANDLERS`); no
  other run changed. The original's combat log, in Pause mode, copies an item (a fact id): left out as UI (T2).
- **POC-4 round 11 (nineteenth session, `docs/poc4/text.md`): text.** `crates/wr-text` ports the localization packs
  (read once from the install's JSON into our own memory-mapped format, `cache/text/<locale>.wrtext`: opens in 1.9 ms,
  the JSON takes 66 ms) and `TextTemplateEngine` (the scan equal to the decompiled loop run on Mono on 335,690 strings;
  `ToLowerInvariant` is Mono's table, from the game's `mscorlib`); templates read game state through an `Env` that fails
  closed. The sim makes the dialogue's text (`crates/wr-sim/src/text.rs`: `mf`, `name`, `race`, the UI colours, unit names
  through the shared strings `tools/wr-rules-data` now extracts from `blueprint.assets` into `cache/sim/shared-strings.json`,
  the speaker's name and colour, `DialogFormats`), and WrathOracle records the game's (`TextObs.cs`, declared
  `dialog_text`: each history line, each transition's cue and answer text and speaker, the controller's custom speaker
  fields at window start). On `poc4-seelah-7`, `poc4-horgus-11` and `cert-24` **all 21 dialogue runs are valid with every
  text record equal**. Camelia's meeting is recorded for the first time (`poc4-camelia-1`; her trigger is the zone
  `CameliaDialogueTrigger`), but the sim can't run it yet: the dialogue starts from a foreground cutscene's
  `CommandStartDialog` (`docs/poc4/dialogue.md`, round 11).
- **POC-4 round 10 (eighteenth session, `docs/poc4/dialogue.md`): every dialogue run valid** (`poc4-seelah-5`'s 12,
  `poc4-horgus-9`'s 6, `cert-22`'s 3). The Horgus runs too: roaming's two draws are tied to their recorded producers (WrathOracle's `roam_point` now names
  the caller and the unit; a draw counts as T1 only right after its producer, by `UnitRoamingController`, and
  `runner::audit_roaming` checks each producer against the scene cache and the units: no idle cutscene can start),
  and the unit groups model runs in dialogue runs (`UnitGroupsController` start, tick, spawn and destroy handlers),
  checked after every tick against WrathOracle's new `grp_tick` states. The group check found the units pool's order
  wrong for units that join a state after the load (the game appends them; `scene::units_pool` now does). Saved
  roaming next points with their own idle cutscene are reported at load.
- **POC-4 round 9 (eighteenth session, `docs/poc4/dialogue.md`): 18 of 24 dialogue runs valid with every input recorded.**
  `CommandLockControls`' buff tick and postpone are modelled (`UnitBuffsController` in dialogue runs, `m_PreviousTime`,
  `TickAll`/`PostponeAll` over `Units.All`, the controller's `Activate` at each mode activation), with the game-mode
  stack and `CutsceneLock`'s Cutscene mode; WrathOracle records each lock (`cs_lock`) and each script-zone entry
  (`zone_enter`), and the runner checks every tick's mode against the recording (Pause, a UI request, taken from it).
  The zone step now enters at the recorded tick (it was 115 ticks early). On the new captures `poc4-seelah-4`,
  `cert-22` and `poc4-horgus-8`: all 15 Seelah and 3 `dialog-checks` runs valid; the 6 Horgus runs T0-equal, end
  state 1.0000, invalid only on group membership and roaming's two draws. Older captures report the lock.
- **POC-5: three recorded fights with different choices, every run valid (seventeenth session).** The third fight,
  `poc5-tb-96/tb-hold` (the focus reversed, the archer ending its turns), replays valid on its record and both replays
  with every slice computed, like `-95`'s `tb-nearest` and `tb-focus` (`docs/poc5/commands.md`, "A third fight"). That
  meets POC-5's pass as worded, given the inputs that remain recorded (movement agent and pathing, group memory and
  visibility, animation, the Unity clock, the UI's requests). No impatience hard target in it either, nor in the fourth,
  `poc5-tb-97/tb-solo` (one party member fights; the AI archer ends it in round 2), also valid on every run.
- **POC-4 rounds 6 to 8 (seventeenth session, `docs/poc4/dialogue.md`): Horgus T0-equal.** WrathOracle now records,
  outside turn-based windows, the units `UnitActivatableAbilitiesController` iterates (`aa_awake`), each cutscene
  `CommandAction` run (`cs_action`), random gate draws with their producer (`gate_track`, flagged inside a player's
  `Start`) and roaming's draw producers; the begin record declares `cutscene_progress` and `aa_awake`. The dialogue
  runner takes these as recorded inputs (awake states where the sim can't tell, a cutscene's T0 steps when the game ran
  them, accepted random gates' draws at their place) and applies dialogue inputs at their recorded ticks. On
  `poc4-horgus-6` every run's T0 stream is equal and the end state 1.0000; still invalid on `OnIsInGameChanged`'s group
  membership and two roaming draws, and a gap found in review applies to every dialogue fixture: `CommandLockControls`'
  buff tick/postpone and the runner's missing `UnitBuffsController` (plan in round 8's notes). Seven Sol rounds also
  closed older cutscene-classification holes (`OnStopped`, in-fade starts, `RunAway`, revival by unit-controlling
  commands, `TranslocateUnit`'s `MarkNoExtra`, now ported). `scripts/scene-cache.sh` works again (lower-case area name);
  the scene cache is schema v2 (roaming waypoints): rebuild both areas' caches.
- **Hydration (seventeenth session):** mythic class levels in `AddClassLevels`, `AddMythicEnemyHitPointsBonus`,
  `AddFeatureOnAlignment`, `HasArmorFeatureUnlock`: Manual_10's one-sided facts 72 -> 35 (Mephisto, Galfrey match).
  `manual10-ids-1` (WrathOracle `IdTrace`) names every id the load allocates: the rest of Manual_10's id offset is the
  facts still missing (working notes).
- **POC-5 round 14, slice G (sixteenth session, `docs/poc5/commands.md`): the impatience controller computed.** On
  `poc5-tb-95` (both fights, record and two replays) every run is valid with `UnitImpatienceController` the sim's
  (`combat/impatience.rs`, `combat/replay/impatience.rs`, `csharp/hashset.rs`): its tick (a phase before the AI's) and
  `UpdatePriorityTarget` (also `TurnController.Prepare`'s, before its `ForceTick`), each unit's hard, disliked and bother
  units (.NET `HashSet`s), attacks without effect and counts, the handlers (command starts, damage, offensive actions,
  deaths) queued and applied in order as their records come, leaving combat's clear; the AI's `ipt`/`disliked` reads are
  checks. WrathOracle's observer `impatience` (`CommandObsImpatience.cs`). gpt-6.1-sol's reviews converged after seven rounds;
  57 damage cases. No hard target, priority target or draw happens in these fights (a fight where an enemy keeps missing one
  target would exercise them).
- **POC-5 round 13, slice F (sixteenth session, `docs/poc5/commands.md`): units' targets computed.** On `poc5-tb-92`
  (both fights, record and two replays) every run is valid with `UnitCombatState.LastTarget` and `ManualTarget` the
  sim's (`combat/targets.rs`, `combat/replay/targets.rs`): `UnitCommands.UpdateCombatTarget` in `Run` and the merge (its
  `NextCommandTime` resets now checks: 403 in `-92/tb-focus`), `ShouldAttackOnDisengage`'s write and its scope's
  restore, leaving combat's `Clear`, and the writes that follow an outside entry (the predictions' `OnClick` after its
  `Run`, combat end's `TryCancelCommands` and `Stop` after their `InterruptAll`, the driver's after its click). The AI's
  and the engagement's `last`/`manual` reads are checks. WrathOracle's observer `targets` (`CommandObsTarget.cs`):
  `tgt_init`, `tgt_w` (each changing write with its writer's call chain, checked exactly), `UpdateCombatTarget`'s leaves
  `tact`/`aien`. gpt-6.1-sol's reviews converged after eleven rounds; 85 damage cases. Impatience
  (next) never triggers in `-92`'s fights (`ipt` null, `disliked` false throughout): its draws need a fight where an
  enemy keeps missing one target.
- **POC-5 round 12, slice E (fifteenth session, `docs/poc5/commands.md`): the engagement controller computed.** On
  `poc5-tb-85` and `-91` (both fights, record and two replays) every run is valid with `UnitCombatEngagementController`
  the sim's (`combat/engage.rs`): who engages whom (`m_EngagedUnits`/`m_EngagedBy` in .NET `Dictionary` order, freed
  slots included), `Disengage` and `ShouldAttackOnDisengage`, `AttackOfOpportunity` (its `UnitAttackOfOpportunity` run
  nested in the tick: `-91/tb-focus` has one), the attacks of opportunity's count (refills, the 5.4 s cooldown), the
  rule handlers' provokes (the archer's ranged attacks, matched call by call) and the turn's
  `ImmuneAttackOfOpportunityOnDisengage`. `Engage`'s and `Disengage`'s `NextCommandTime` resets and the attack
  environment's `EngagedBy`/`IsFlanked` are checks now. WrathOracle's observer `engage` (`CommandObsEngage.cs`) taps
  what it reads outside the sim (group memory, corpulences, distances, line of sight, concealment, motion, threat range,
  `LastTarget`/`ManualTarget`, prevent flags). gpt-6.1-sol's reviews converged after five rounds; 71 damage cases.
- **POC-5 round 11, slice D (fourteenth session, `docs/poc5/commands.md`): the AI computed.** On `poc5-tb-82` (both
  fights, record and two replays) every run is valid with `AiBrainController` the sim's (`combat/ai.rs`): its ticks,
  the brains' gates, `SelectAction`, the scoring (C# `decimal`s: `csharp/decimal.rs`, a port of Mono's `decimal-ms.c`),
  the static `DecisionContext`, `TargetInfo`'s weights and sort, `ScorePath`'s arithmetic, `BlueprintAiAttack` and the
  five considerations the fights use; its `Random.Range` taken from the tape where the sim draws, its commands run
  through the commands model and its `ForcedPath` checked where `TickApproaching` reads it. WrathOracle's observer `ai`
  (`CommandObsAi.cs`) taps every read the AI makes that the sim can't compute (movement and pathing, line of sight, the
  Unity clock, group memory, impatience, settings) and every write of `NextCommandTime` (`ai_nct`), and checks each
  score (`ai_sc`, `ai_best`, `ai_sel`). Scene units get their brain from their blueprint. gpt-6.1-sol's reviews
  converged after eleven rounds; the damage test covers 71 cases. Not exercised by a capture: `ScorePath`'s movement
  simulation (every AI path here is close enough), line of sight, impatience, priority facts.
- **POC-5 round 10, slice C (thirteenth session, `docs/poc5/commands.md`): the approach computed.** On `poc5-tb-80` and
  `-81` (both fights, record and two replays) every run is valid with `UnitCommand.TickApproaching` (its `Random.Range`
  draw taken from the tape where the sim draws), `UnitMoveTo`'s and `UnitAttackOfOpportunity`'s class code the sim's:
  no class code is taken from the recording any more. `-81/tb-focus` has two attacks of opportunity, their threat hands
  computed and their attack rules fired by the sim. WrathOracle's observer `approach` taps the reads in that code and
  records the lifecycle's calls into the movement agent (`uc_call`: `MoveTo`, `FollowPrecomputedPath`, `StopMoving`),
  each checked; the movement agent itself (positions, paths, its own interrupts) stays an input until the nav port. Also
  ported: `ACBonusAgainstFactOwner` (a Sonnet agent; `-79` reached it). gpt-6.1-sol's reviews converged after four
  rounds, GPT 6 Astra found nothing in the hooks; the damaged-capture tests cover 43 cases.
- **POC-5 round 9, slice B (twelfth session, `docs/poc5/commands.md`): `UnitAttack` computed.** On `poc5-tb-77` (both
  fights, record and two replays) every run is valid with `UnitAttack`'s class code the sim's (`combat/commands/attack.rs`):
  `Init`/`InitAttacks` (full or single, the attack list from the count rules, two-weapon interleaving and iterative
  penalties), `TryStartNextAttack`, `OnTick`'s iteration delay, `UpdateTarget` with its retarget, `OnAction` and
  `TriggerAttackRule`, whose `RuleAttackWithWeapon` and attack-count rules the sim fires with the fields it computes (the
  recorded roots are their checks). WrathOracle's new observer `attacks` makes `UnitAttack`'s code base lifecycle code for
  the stack walk and records its reads (`five`, `efa`, `farm`, and transpiler taps for inlined getters: `afin`, `arel`,
  `style`, `enemies`, `sorted`) and what `ConfigureAnimations` decides (`ua_atk`). gpt-6.1-sol's reviews converged after
  43 rounds (each round's findings fixed; the driven replay fails closed on 202 kinds of damaged record, and T0 now also
  compares every rule's bonuses with the sim's). Inputs left: `UnitAttackOfOpportunity` and other classes' code, approach
  and movement (slice C), the AI's and the UI's calls (slice D). For the next capture: have WrathOracle write a fact
  handler's fact and owner (`ActivatableAbility`) and a rule bonus's dice (`docs/poc5/commands.md`, slice B result).
- **POC-5 round 8, slice A (eleventh session, `docs/poc5/commands.md`): units' commands computed.** On `poc5-tb-74`
  (both fights, record and two replays) every run is valid with the commands' base lifecycle the sim's:
  `UnitCommands` (run, queue, the interrupts), `UnitCommandController`'s tick (its turn-based gates, approach, start,
  `SpendAction`), `UnitCommand`'s start, tick, act and end, the turn's running commands. The turn controller's command
  reads, the leave controller's `cmds` and the controller's `cd_spend` are now checks. WrathOracle's `CommandObs.cs`
  records each command's identity, the lifecycle's entries with their callers, each environmental read the base code
  makes (a stack walk tells base code from class code) and the class code's effect on the base state. Inputs left:
  class code (`UnitAttack`'s attacks: slice B), approach and movement (slice C), the AI's and the UI's calls (slice D).
- **POC-5 round 7 (tenth session, `docs/poc5/combat.md`): turn ends computed.** On `poc5-tb-68` (both fights, record and
  two replays) every run is valid with the turn controller's status computed: `TurnController.Tick`'s status change and
  end wait, `Prepare`'s status, `ContinueActing` (the AI's stuck frames and idle wait), `ContinueWaiting` and `End`
  (`combat/turn_tick.rs`); the recorded `turn_end`s are checks, every one matched by the sim's own. WrathOracle's
  `TurnTickObs.cs` records each step's reads at the step, under the game's own conditions. Inputs left: commands
  (start, act, end, running), the movement agent, the movement limit, the end-turn button, the camera's scroll, the AI,
  `IsAbleToAct`/`IsDirectlyControllable` (animation state), a magus' extra action, projectiles in flight.
- **POC-5 round 6 (ninth session, `docs/poc5/combat.md`): units' cooldowns computed.** On `poc5-tb-63` (both fights,
  record and two replays) every run is valid with no cooldown row taken from the recording: `UnitCombatCooldownsController`,
  `SpendAction`, `TurnController`'s `Prepare` clear, `End`, `ForceToEnd`, `HandleUnitCommandDidEnd` and `TickMovement` (the
  turn's movement budget) are ported (`combat/cooldowns.rs`), and `TickTime`'s turn starts follow from them (checked
  against every recorded `StartTurn`). WrathOracle's `CooldownObs.cs` hooks every cooldown writer, before and after.
  Inputs left: when commands act and end, the movement agent's deltas, the movement limit, the end-turn button, the
  camera's scroll (every turn here is prepared after it), the AI.
- **POC-4: 15 of 21 dialogue runs are valid end to end.** That's the 12 Seelah/Anevia runs and the 3
  `dialog-checks` runs: valid, and equal to the oracle's whole ordered T0 stream and every sheet (351, 382,
  357, 78 records). The area's scene entities load from `scripts/scene-cache.sh` (script zones, map
  objects, cutscene players, unit groups, view attach), along with `ActivateTrigger`, the closing
  cutscene's `HideUnit`/`Spawn` and the NPC components. The 6 Horgus runs equal 127/130 but stay invalid:
  the restored background cutscene's `RandomTrack` gate (the oracle draws there) and the awake state of
  the unit the cutscene spawns aren't ported (`docs/poc4/dialogue.md`, round 5).
- **Hydration:** Manual_13 100% with nothing unsupported at load; Manual_19 1.0000; Manual_18 100%;
  Manual_10 0.9097 (sixteenth session: `compare` pairs facts by blueprint and occurrence, the ids their own field:
  every paired fact's fields agree, 4,149 of 21,843 ids do (the run's id offset); it was 0.8206 pairing by id. Then the
  load pass stopped starting extras' toggles (`MarkExtra` now sets `IsExtra`; an extra sleeps out of the camera, so its
  toggle isn't started, reported): facts on one side only 483 -> 72). Placeholders leave the game through `entity::set_is_in_game` (the one IsInGame
  setter). `AbilityParams.Empty` is modelled as process state: the oracle records it at load start, and
  Manual_10 ends at caster level 16 in both.
- **POC-5 round 5 (eighth session, `docs/poc5/combat.md`): the first valid POC-5 runs.** On `poc5-tb-60` (both fights,
  record and two replays) every run is `valid=true`: T0-equal, end state 1.0000, nothing reported, nothing echoed.
  WrathOracle records each root rule's call site (`origin`) and a root flat-footed check's environment; the replay computes
  the flat-footed checks and attack counts it used to echo, in the phase their caller places them (the initiative tracker,
  the hover predictions, the AI in the turn's tick or `StartTurn`, the commands). New ports: `RuleCalculateAttacksCount`
  and `…Limbs` with three handlers. "Valid" is given the recorded inputs: which attack the AI and the player choose, when the
  UI refreshes, turn starts and ends, commands, cooldowns, memory and visibility are still the recording's.
- **POC-5 round 4 (seventh session, `docs/poc5/combat.md`): the turn port.** On the new capture `poc5-tb-55`
  (both fights, 6 runs) the only report left is the echoed presentation roots: leaving combat
  (`UnitCombatLeaveController`), unit groups, the turn controller's order (`ChooseNextUnit`, `StartRound`, `m_NextUnit`,
  `HandleUnitRollsInitiative`, the prepare batch), `LeaveCombat`'s state, the death listeners, `DropLootToGround`,
  `UpdateSlotIndex` and the combat triggers' actions are ported and agree with every record. WrathOracle records what they
  read that the sim can't compute (`GroupObs.cs`: group memory, commands, visibility, cooldowns, the UI guard), as inputs.
  Older captures replay as in round 3.
- **POC-5: the driven replay reproduces six turn-based fights exactly** (sixth session,
  `docs/poc5/combat.md` round 3): `poc5-tb-51`, `-52`, `-53` (current oracle; the fights differ by
  capture), 18 runs, every compared T0 record equal and **every end state equal to the oracle's**
  (all units and fields). The replay consumes the oracle's turn-controller records (`turn_start_begin`,
  `turn_clear`, `loot_timer`, `awake_set`/`awake`, resolve `env`); `CombatController.m_Units` is ported;
  new ports: `CalculateMissTarget`'s draws, `AddBackgroundWeaponProficiency`, shield bash,
  `UnitPartFollowUnit`. The id gap of round 2 was the tutorial fact's unique id (not the dropped loot).
  Manual_18 loads with nothing unsupported. Runs aren't valid yet: the presentation roots are echoed,
  and the death and combat-leave paths below are reported. `poc5-tb-29` is superseded.
- **Oracle: turn-based replays are tick-exact, under load too.** `poc5-tb-51` and `-52` (synthetic load)
  and `-53` (idle): 4/4 replays valid and T0-equal tick-exact each, every fixture clean. The pins:
  player input neutral at the source (`InputPin.cs`: every `UnityEngine.Input` reader transpiled,
  Rewired disabled, focus/hot-plug held to after the window); the fixture's attack points the game's own
  pointer at the target; per-unit path delivery (`PathSync`, `PrePathSync`); the physics step phase
  steered to the recording's (`PhysicsPhase.cs`, the visibility frontier); idle-animation draws
  (`IdlePin.cs`). **The fights changed with the pointer fix** (tb-nearest runs 2,619–2,764 ticks, was 1,990):
  recordings before Wrath-Mac `8fa7cc6` don't replay on the current mod; the sim's POC-5 numbers
  above are against the current ones (`poc5-tb-51`..`-53`). Certification: `cert-20` (from this repo's layout) dice,
  dialog-checks and attack all T0-equal tick-exact; perturbed replays fail (`docs/poc5/tb-replay.md`).
  Remaining frontier: Unity's absolute clock (animation timing, the AI deadlines in
  `AiBrainController`); seen once in 54 replays (`poc5-tb-50`, a projectile a tick late, within T0
  tolerance).
- **The repos were split (2026-10-12).** Everything pf-wotr-rs needs moved here from Wrath-Mac:
  `oracle/` (the mods and tools, tracked; `oracle/README.md`), and gitignored `builds/arm64-oracle`
  (APFS clone), `builds/.oracle-run.lock`, `decompiled/`, `fixtures/saves/` and
  `fixtures/captures/runs-raw/`; `docs/baseline-runs/` has Wrath-Mac's benchmark reports.
  `wr_paths::{repo_root, fixture_saves, decompiled}` resolve the new places. Verified: `check.sh`, the
  real-save tests, the sim regression (identical to the post-merge baseline) and `cert-20`. Wrath-Mac
  became pf-wotr-silicon (`github.com/loafykitten/pf-wotr-silicon`, public since
  2026-10-12): a fresh one-commit history with the build tooling, WrathPerf, a README and an MIT
  license. Its old history is on local branches `archive/private-history` and `archive/wip-tb-hooks`,
  never pushed. Don't use it from here.
- **POC-6:** realtime spot and point shadows (pack format v2: rerun `wr-import pack`), FLIP mean 0.1246.

**Next, in order:**
1. Replace the recorded inputs with their controllers (`docs/poc5/commands.md`). The ports now exist; next is wiring
   them into the driven replay, one at a time (each touches `combat/replay/*.rs`, so run them one after another, not
   in parallel): A2's hardened agent (running); then the memory tick as `Phase::Memory` (`docs/poc5/memory.md`, "H1:
   result"); then `wr-phys` behind `AgentInputs::raycast`/`disc_cast` and the turn-based paths (`combat/paths.rs`)
   behind the agent's `PathTo`/`FindPath` (the player's `FindPath` results are recorded since `poc5-tb-114`). Still
   inputs with no port: visibility's fog of war (`IsInFogOfWar`; the vision range is computed), animation, the Unity clock, the UI's requests and its path visualizer. The
   impatience draws need a fight that reaches a hard target; a fixture whose enemies start out of reach would exercise
   the AI's movement simulation (`ScorePath`, `combat/ai_sim.rs`). More fights recorded on the current mod (a
   critical, a spell, iterative attacks, an ally dying, a loot drop, a five-foot step, a full attack), with call sites
   and the cooldown and tick hooks (POC-5's three fights with different choices are recorded and valid: `-95`'s two
   and `-96`).
2. POC-4 (`docs/poc4/dialogue.md`): every dialogue run is valid, Camelia's included (round 12, on Ember's Manual_20).
   Facts' handler interfaces: done (round 13). Left: looting when a
   fixture needs it, the combat log's (stream I, running) and UI's templates. The fixture saves' load tail
   is closed (Manual_13 and Manual_18 load with nothing unsupported; Manual_19 reports only the `RandomTrack` gate, a
   recorded input in its runs); Manual_10's 224 kinds are item 3's.
3. Manual_10's 72 facts on one side only (after the extras' toggles: mounts and animal companions, monster mythic
   features, a few abilities and buffs; `compare-Manual_10.json` and the two dumps show them). They also offset every
   later fact id: `manual10-ids-1/ids-load.json` names each allocation the sim misses (working notes).
4. POC-6 (booklab is unavailable for now: on this Mac, after item 1, if at all): a Metal frame capture of the original (round 3's open shadow
   questions), light probes, the rest of post. The timing benchmarks stay on this Mac (screen unlocked).
5. Oracle, when it bites: Unity's absolute clock (animation timing, AI deadlines); real-time fixtures
   get only the input pin, not the phase or idle pins.

**Working notes for the next session:**
- **Twenty-second session (2026-10-15):** captures `poc5-tb-100` to `-103` are the agent's (`-103` has the current observer; the checker
  reads the older ones with what they lack taken as noted in `docs/poc5/movement.md`); `-98`/`-99` archived. `UnityEngine.CoreModule` is decompiled into
  `decompiled/UnityEngine.CoreModule` (Vector2/Vector3/Mathf/Quaternion's managed code). The agent's check:
  `cargo test --release -p wr-sim --lib every_recorded_tick -- --ignored --nocapture` (`WR_AGENT_CAPTURES` picks others).
  The sim regression (`scripts/regress.sh`) wasn't rerun: the agent isn't wired into any replay yet (only a field on
  `UnitCombat`), so the baseline stays `cache/regress/out-facthandlers`; rerun it once `UnitMoveController` runs in
  `Phase::Move`. For `PathTo`'s orientation (`InitMoveDirection` reads `OrientationDirection`; `UnitMoveController`
  writes `Quaternion.LookRotation(forward).eulerAngles.y`): the player's icall stubs are `LookRotation_Injected` at
  0x81c2d0 and `Internal_ToEulerRad_Injected` at 0x81c074 (not yet ported); port and check them as `wr-nav`'s
  `native_check.rs` does `RotateTowards` (through the stub when the implementation returns through x8).
- **Twenty-first session (2026-10-15):** the regression baseline is `cache/regress/out-facthandlers` (identical to
  `out-camelia2` but Manual_10's coverage). `wr-combat replay` doesn't print executed handlers; a temporary print of
  `w.coverage.executed` is how round 13 counted them.
- **Twentieth session (2026-10-14):** Ember's save `Manual_20_WR_PreCamelia` (in `fixtures/saves/`, read-only; dump
  `manual20-dump-1`); captures `poc4-camelia-6` (walk-in) and `poc4-camelia-5` (development: `teleport` after Seelah's
  meeting); `poc4-camelia-1`, `-3`, `-4` and two aborted `-2` tries archived. The regression baseline is
  `cache/regress/out-camelia2` (identical to `out-text2` plus Manual_20 and the Camelia runs). The WrathOracle build
  installed has `cutscene_commands` and `paused_origins`. A mutation in Pause mode is UI or input: recordings keep player
  input neutral, so a `Kingmaker.UI` frame in its origin marks the original's UI.
- **Nineteenth session (2026-10-14):** captures `poc4-seelah-7`, `poc4-horgus-11`, `cert-24` (with `dialog_text`) and
  `poc4-camelia-1`; `poc4-seelah-5`/`-6`, `poc4-horgus-9`/`-10`, `cert-22`/`-23` superseded (archived). The POC-4 suite
  and `scripts/regress.sh` use the new ones; the regression baseline is `cache/regress/out-text2` (only the dialogue
  logs differ from `out-round10`: the text records). Run `wr-rules-data` once to write `cache/sim/shared-strings.json`
  (the runner loads it with the English pack; a recording with `dialog_text` reports their absence). `wr-text build`
  builds the pack (also on first use). A .NET harness running the decompiled `Process` loop is a useful oracle for the
  engine, but run it on Mono (`mono`, Homebrew): CoreCLR's casing differs from the game's.
- **Eighteenth session (2026-10-14):** captures `poc4-seelah-5`, `poc4-horgus-9` (with `roam_producers` and
  `group_ticks`) and `cert-22`; `poc4-seelah-3`/`-4`, `poc4-horgus-7`/`-8`, `cert-21` are superseded (archived on the
  external volume; `cert-22`'s `dialog-checks` has no roaming and runs without the groups model, valid). The POC-4
  suite and `scripts/regress.sh` use the new ones; the regression baseline is `cache/regress/out-round10`. The
  certification's real-time `attack` replay is one tick late in `cert-22` (as in `cert-16` and `-18`, not in `-21`):
  the known real-time frontier (no phase pin), not the new hooks. The combat replay still reports any game-mode change
  (its controllers' `Activate`), and the dialogue runner models only Default, Dialog, Cutscene and recorded Pause.
- **Seventeenth session (2026-10-14):** `poc5-tb-96` recorded and in the suite; `-93`/`-94` deleted (Ember's say-so).
  Ember agrees with the assumption that non-extra units spawned at load are awake, but Manual_10 shows its limit: about
  15 vendors and officers there sleep in the game (`SleepingUnitsController.ShouldBeSleeping`: in fog of war with no
  commands), so the toggle the load pass starts for them stays off in the oracle. Fog of war isn't modelled.
  Manual_10's fact ids: `manual10-ids-1` (WrathOracle `IdTrace`, plan `dump-manual10-ids.json`) has every id the load
  allocates with its call chain (18,518; its state dump is the old one's). The sim allocates 64 fewer, the facts it
  doesn't create: 5 before the first spawned unit (the main character's three `ShifterFuryWeaponList` from
  `UnitHelper.AddFact`, two enchantments from `BuffEnchantSpecificWeaponWorn.OnTurnOn` in `SceneEntitiesState.TurnOn`),
  then the mythic levels (Mephisto 14, Galfrey 18), mounts (Seilkind, Ciar), `ResiliencyJudgmentAbility*`,
  `HellknightInfernalArmorResistance`/`FireImmunity`, `KitsuneChangeShapeBiteBuff`, `SacredWeaponEnchantAnarchicChoice`.
  Every id after a missing one is offset by them (17,696 of Manual_10's 21,845 fact ids differ), so closing those
  closes most of the ids. Not all: about 14 buffs are recreated later in the game (a toggle's `ReapplyBuff` in a
  controller tick: Trever's `PowerAttackBuff` is `1E69AC` there, `1E2137` in the sim), a separate follow-up.
  **Done this session:** mythic class levels (`AddClassLevels` through `SelectClass`, `ApplyClassMechanics`'
  5 hit points, no skill points, `Classes.Sort`, the unit's `MythicExperience`, `SetupLevelsIfNecessary`),
  `AddMythicEnemyHitPointsBonus`, `AddFeatureOnAlignment` and `HasArmorFeatureUnlock`: Mephisto and Galfrey match,
  Liotr, Hurlun and Morveg's alignment facts and the Hellknights' too; facts on one side 72 -> 35, match 0.9099 (ids
  still offset until the rest is closed). Left: `ShiftersFury` (3), `BuffEnchantAnyWeapon` (2), `AddFeatureOnApply`
  (the mounts: during a level-up its `UpdateProgression` runs on the live `LevelUpController`'s state, which
  `spawn/levelup.rs` passes down as a local, so the port needs that state reachable from the world), the Kitsune's
  `AddFactContextActions` buff, the sleeping vendors' toggles; Hurlun's difficulty modifier (-29 vs -25) predates this.
- **booklab, the second Mac (set up 2026-10-12; details in `docs/booklab.md`):** `ssh booklab`, an Apple M1 (8-core
  GPU, 16 GB), dedicated to this project, kept awake and plugged in. It runs the game (without Steam:
  `WR_NO_STEAM=1`) for **correctness work in parallel with this Mac**: oracle recordings, and POC-6's render work
  against the original (screenshots, Metal frame captures). **Not timing:** the performance targets and benchmarks are
  this Mac's (M2 Max), the machine Ember plays on (Ember, 2026-10-12: the M1 isn't a target). Compare renders against
  the original captured on the same machine. **Certified for fights** (`booklab-tb-1`: all six runs tick-exact, the
  sim valid on each). Dialogue plans stay on this Mac: on booklab a reload right after a dialogue fixture stalls
  (`docs/booklab.md`). **Not available for now (Ember, 2026-10-13): plan for this Mac
  only.** Record here, one capture at a time (the game/GPU lock), and leave booklab's items (POC-6's captures of the
  original) until Ember says it's back.
- **Disk (2026-10-14):** `target/`, `cache/` and `fixtures/captures/` live on Ember's external SSD (symlinks; 2.5 TB
  free), the internal disk has about 94 GiB free for the repo, `builds/` and the saves. Keep Metal traces short;
  superseded captures move to the external volume's archive folder (`poc4-seelah-3`/`-4`, `poc4-horgus-7`/`-8` and
  `cert-21` are there), deleted only by Ember.
- Record through `scripts/oracle-record.sh LABEL SAVE PLAN [options]` (watchdog for startup and mid-run
  hangs, with retries; a failed try's leftover game is killed; waits for the game/GPU lock; refuses a locked screen; keeps the display awake with
  `caffeinate`). `displaysleep` is 120 min and a sleeping display locks the session, so for unattended
  runs keep `caffeinate -dim` going (`caffeinate -i` isn't enough). Check with
  `ioreg -n Root -d1 -a | grep CGSSessionScreenIsLocked`. Plans are in `oracle/tools/oracle-fixtures/`.
- Build oracle mods with the game closed: `dotnet build -c Release oracle/mods/WrathOracle/WrathOracle.csproj`
  (installs into `builds/arm64-oracle/Mods`; `oracle-run.sh` insists on exactly the four oracle mods there). Check
  that it says "Build succeeded" before recording: a failed build leaves the old DLL in place, and a recording chained
  after it records with the old hooks.
- Driven replay of one run: `target/debug/wr-combat replay fixtures/captures/oracle/<capture>/fixtures/<fight> --run
  record` (its `commands:` line counts what the commands model computed); `WR_DEBUG_COMMANDS=1` prints each command
  desync and state difference, `WR_DEBUG_UNSUPPORTED=1` each report under its frame. The POC-5 suite:
  `cargo nextest run -p wr-sim --test poc5 --run-ignored only` (about three minutes). The current captures are `poc5-tb-95`
  (the impatience controller computed; the summary's `impatience:` line) and `-92` (slice F, the targets); `-85` and
  `-91` replay as slice E, `-82` as slice D, `-80` and `-81` as slice C, `-77` as slice B, `-74` as slice A. The superseded ones
  (`poc5-tb-69` to `-73`, `-75`, `-76`, `-78`, `-79`, `-83`, `-84`, `-86` to `-90`) were deleted on 2026-10-14. Fights vary by capture: disengages (and so
  attacks of opportunity) were in 3 of 18 fights recorded this session; record again for one.
- A recording that "hangs at startup" may be the main menu's gamepad/keyboard prompt waiting for a click (2026-10-14):
  ask Ember whether it's showing before suspecting the hooks.
- Scene caches for POC-4: `scripts/scene-cache.sh <area> <guid>` (schema v2 since 2026-10-14, with roaming waypoints;
  rebuild both areas' caches after pulling).
- The oracle regression: `scripts/regress.sh <label> <bindir>` then `scripts/regress-diff.sh <new> <base>`
  (outputs in `cache/regress/`; the last baseline is `cache/regress/out-round10` (release build; the dialogue fixtures from
  `poc4-seelah-5`, `poc4-horgus-9` and `cert-22`; before it `out-lockmodel`, with `poc4-seelah-4`, `poc4-horgus-8`; before it `out-cs4`, the Horgus fixtures from `poc4-horgus-6`; before it `out-mythic2`: the mythic levels and three components, only
  Manual_10 changed; before it `out-awake-start`, the fact pairing's `out-fact-pairing`,
  `out-slice-g`); pass the outputs' full paths to `regress-diff.sh`). Run parallel ones with a
  private `WR_CACHE` (symlink every `cache/*` entry except `sim` and `poc5`, which are copied).
- Worktree agents sharing the main `target/` can bake their checkout path into `wr-paths`;
  `touch crates/wr-paths/src/lib.rs` before building in the main tree.

## Goal

Reimplement Wrath of the Righteous on a new engine written in Rust with Bevy. It loads
the game's own data from Ember's install and runs **better than the original on Ember's
Apple Silicon Mac**. Ember's words: "We don't need to 1:1, we need to be better across
the board for this rewrite to be worth it." Here "better" means the named metrics in
"Apple Silicon", with no regressions on the others.

Ember's thesis, which this plan tests rather than assumes:

- most of the game is **rules and data** (blueprints), not mechanically complex code;
- most of its hours are **dialogue**, which is data plus a small controller;
- the **decompiled source** is available locally as a specification of the behaviour.
  It's one part of the specification: Unity's native behaviour, some managed libraries
  (e.g. `AstarPathfindingProject.dll`, not yet decompiled), shaders and serialized Unity
  configuration are the rest.

**First goal: the proof of concept below** (POC-0 to POC-8). It ends in a go/no-go
decision with measured evidence. Nothing past it is committed.

## Ground rules

- **Personal use only; this repo stays private.** No publishing. The only remote is the
  private GitHub repo `pf-wotr-rs` (Ember approved it 2026-10-10).
  Never commit game data, extracted assets, decompiled code or oracle captures. Keep all
  of them under the gitignored paths (`game-data/`, `cache/`, `fixtures/captures/`,
  `fixtures/saves/`, `builds/`, `decompiled/`) and
  check `git status` before every commit: `.gitignore` only protects the paths it lists.
  The engine reads everything from Ember's install at runtime, or from local caches built
  from it.
- **Licensing: Ember's decision (2026-10-10) is to continue.** Ember's position: this isn't
  a derivative work but a mechanical swap of the engine, with no change in content or
  outcome; it depends entirely on owning the game, and it's for personal use only. For
  the record, Owlcat's EULA (owlcat.games/eula, §§2.2, 2.6, 2.9) restricts decompilation,
  modification and derivative works, and Audiokinetic licenses Wwise per title. Don't
  reopen the question; keep the project private and personal, and make no clean-room
  claim.
- **Never modify the Steam install.** The oracle runs on `builds/arm64-oracle`, an APFS
  clone of the native arm64 build (built with pf-wotr-silicon; `oracle/README.md`).
- **Back up saves** before letting any build write to the game's save folder. The Rust
  engine writes its own saves elsewhere until save compatibility is proven.
- Ask before anything outward-facing.
- **Model routing** (Ember's global CLAUDE.md, 2026-10-10; subagent model changed by Ember
  2026-10-11, 2026-10-15 and 2026-10-17): Claude implements, in the main session or through **Sonnet 5.5
  (high) subagents** for large or parallel work, stepping up to Opus 5.5 (medium) when they have problems.
  **gpt-6.1-sol (high) reviews every implementation** (`codex-review` skill;
  `codex exec -m gpt-6.1-sol -c model_reasoning_effort="high"`, Codex Fast mode
  `service_tier = "priority"`); **GPT 6 Astra (high)** (`-m gpt-6-astra`) instead when Sol misses
  things or the work is particularly complex or touchy (Ember, 2026-10-12: Astra reviewed
  everything before and is slow even in Fast mode). Bulk investigation and
  computer use go to **gpt-6.1-sol** (`codex exec -m gpt-6.1-sol`; Ember said 2026-10-10
  it supersedes gpt-5.6-sol). Ember also offered Astra for computer use; prefer Sol and
  switch to Astra if Sol isn't good enough at a task. Never Haiku. Decide yourself when
  computer use helps (comparing renders with the real game, driving the game for
  fixtures).

## The fidelity contract

This is the core design decision (proposed 2026-10-10; Ember can override).

| Tier | What | Standard | How it's checked |
|---|---|---|---|
| **T0 exact** | Rules outcomes, dialogue flow, story state (etudes, flags, quests, dialogue history), XP, alignment, character sheets, inventory and resources, reading original saves | Identical to the original **given the same inputs, draw tape and time deltas** | Differential tests against the running game (oracle) |
| **T1 equivalent** | Timing-coupled play: movement, command durations, animation-driven hit/cast timing, turn-based budgets, AI choices | Same T0 events in the same order. Tolerances may apply to *when* something is shown, never to *whether* or *in what order* it happens in the rules | Oracle traces replayed with recorded deltas |
| **T2 free, must be better** | Rendering implementation, loading, memory, frame pacing, UI plumbing, audio plumbing, data and save formats, threading | Free to redesign, under named targets and non-regression rules. **Visual output must not change perceivably** (below) | Matched-workload benchmarks against the Wrath-Mac baseline; side-by-side captures |

**Visual rule (Ember, 2026-10-10):** the picture stays as close to the original as
possible. Optimize and clean up the rendering however you like, as long as the output
doesn't change in ways a player could perceive. So: same assets, same shading, same
lighting and effects. An optimization that changes the image (e.g. lossy re-encoding, a
cheaper shading model) is allowed only if it's shown to be imperceptible.

**Upscaling and similar techniques are acceptable** (Ember, 2026-10-10): MetalFX
upscaling, and things in that realm such as frame interpolation, don't change the output
substantially. They go in **the options menu**, and every option makes clear whether it's
part of the original game or **an addition** (e.g. a visible "Addition" tag with a short
note on what it does). That labelling applies to all settings, not just graphics.

The tiers are **not independent** in this game, so the simulation/presentation contract
below is part of T0. The review found three examples, verified in the decompiled code:

- `RuleCalculateAC.OnTrigger` applies the prone modifier when `Target.View.IsGetUp`
  (an animation-view state) is true. Animation timing can change an AC.
- `AttackHandInfo` (line 135) draws animation variants, and `UnitAttack` (line 801) draws
  attack-sequence choices, from the **same global RNG as the dice**.
  `UnitAttack.ConfigureAnimations` reorders attacks using animation sequences.
- `TimeController` advances game time from capped frame deltas, and buffs store end times
  and next-tick times. Frame rate leaks into the rules.

### Simulation/presentation contract

1. **The simulation owns time.** It advances on a sim clock with the original's capped
   variable-delta semantics (`TimeController`). In replay, the deltas come from the
   oracle's tape. In play, the sim steps at a fixed rate decoupled from rendering, and
   Bevy interpolates between steps for display. That choice must be shown not to change
   T0 events over the fixture domain.
2. **The simulation owns posture and animation timing.** Clip durations, hit and cast
   event times, sequences and variant counts are *extracted data* the sim reads. States
   like `IsGetUp` are sim state. Bevy's animation system displays them and never drives
   the rules.
3. **One RNG interface, every consumer.** Dice, direct `UnityEngine.Random` calls (e.g.
   `RuleSkillCheck` line 157), animation variants, attack sequences, dialogue cue
   selection and AI jitter all draw through it. Replay: a tape, consumed in the original
   order, failing on a missing or extra draw. Play: our own generator. Saves don't store
   the original's RNG state (survey §3), so play can't reproduce historical rolls anyway.
4. **C# arithmetic semantics.** Integer overflow wraps where the C# wraps; float rules
   follow Mono's (Wrath-Mac's DXT port found the game's Mono evaluates float expressions
   in double and rounds on store; strict float32 mismatched); rounding and ties as in the
   C#.
5. **Ordering is explicit.** Controller order per game mode, subscriber order, and
   tie-breaking in sorts and selections all mirror the original.

## Facts established (measured 2026-10-10)

### The game's data, from Ember's install

| | Size | Notes |
|---|---|---|
| Blueprints | 236,661 `.jbp` JSON files in `blueprints.zip` (568.8 MB uncompressed) | 335 root types, **2,155 distinct nested `$type` tags** (components, actions, conditions, evaluators), 462k uses; 2,490 tags in all. Untagged field types (`ActionList`, `CharacterSelection`…) come on top. |
| Runtime blueprint pack | `Bundles/blueprints-pack.bbp`, 240 MB, 236,661 records | **What the shipped game actually loads** (binary, Owlcat's reflection serializer). The zip is our input; equivalence must be verified. |
| Dialogue | 1,711 dialogues, 43,214 cues, 20,920 answers, 4,202 answer lists, 953 checks | Plus 1,159 companion banters |
| Narrative | 8,355 etudes, 5,048 cutscenes (60,902 commands of 60 types, 11,397 gates), 1,560 quest objectives, 2,212 flags | |
| Mechanics | 9,085 features, 7,212 buffs, 4,456 abilities, 6,136 units, 3,013 weapons | |
| Text | `Localization/enGB.json`: 110,399 strings; `Sound.json`: 7,815 voice mappings | Templates can depend on game state |
| Assets | `Bundles/`: 23,565 UnityFS bundles, 39.75 GiB | 2,134 `.scenes`, 285 `.nav`, 3,137 `.ee` (equipment), 15,130 `.unit` bundles |
| Audio | Wwise: 45 `.bnk` + 36 `.pck`, 5.91 GiB; `SoundbanksInfo.xml` (33 MB) | The game's `AkSoundEngine.bundle` is universal (arm64) and exports 1,068 SWIG `CSharp_*` C functions; substantial integration code sits above them (`AkCallbackManager`, `AkSoundEngineController`) |
| Saves | `.zks` = zip of Newtonsoft JSON (`$type` CLR names, `$id`/`$ref`), plus `history`, a binary `.fog`, `checksums` | Manual_13 has 106 distinct `$type`s across its documents, and holds state for both Kenabres and the caves |

Nested behaviour types by frequency: the top 100 cover 78.6% of uses, the top 400 95.4%,
the top 800 98.7%; 484 are used exactly once. Frequency isn't effort, though: the most
used, `UnitFromSpawner` (35,440 uses), is a short lookup whose correctness depends on the
whole scene and spawner system.

### The game's code (decompiled, `decompiled/`, never committed)

837k physical lines of C# in six assemblies. The full survey by gpt-6.1-sol is
[`docs/research/architecture-survey.md`](docs/research/architecture-survey.md); read it
before designing any subsystem, and treat it as claims to check. Headlines:

- **UI is a third of the code** (274k lines). Rules are small: RuleSystem 8.6k, EntitySystem
  27k, UnitLogic 88k, dialogue 3.3k, etudes 4.2k, cutscenes 9.6k, AI 4.7k, turn-based 6.5k.
  Kingdom (22k), armies (14k) and the global map (8k) are separate later chunks.
- **Rule order is part of the spec**: synchronous nested `RulebookEvent`s with a fixed
  subscriber order (survey §3, confirmed by Astra).
- **Dice and other randomness come from Unity's global RNG**, and saves don't store its
  state.
- `PrototypeLink`/`m_Overrides` are **authoring metadata, not runtime inheritance**.
- **Some rules data lives outside blueprints**: e.g. `DifficultyPresetsList` (skill-check
  DC adjustments, XP and HP multipliers) is a Unity ScriptableObject in `blueprint.assets`.
- **Areas are Unity scenes**: spawners, map objects, script zones, locators and stable
  entity IDs live in `.scenes` bundles, several per area. Navigation is the A* Pathfinding
  Project (graph caches in `.nav` bundles, decoded by `AstarPathfindingProject.dll`) with
  Owlcat clearance code and dynamic `NavmeshCut`s.
- **The tick is variable-delta** (`Runner.Update → Game.Tick → GameMode.Tick →
  controllers`) with a capped delta.
- `DialogController.SelectAnswer` schedules the next cue and `Tick` plays it later, so
  the completed transition is observed after a tick, not in the answer call.
- Owlcat's **Arbiter** QA system is compiled in (`Kingmaker.QA.Arbiter`). POC-0
  (`docs/poc0/arbiter.md`, static reading by gpt-6.1-sol): the shipped `Caves_1`
  instruction has **eight authored camera points**. Running Arbiter itself won't do (it
  starts a preset new game instead of loading a save, its name index is missing, and it
  deletes its output), but a small mod can resolve the instruction by GUID and drive the
  game's camera through those points. For cross-engine replay, export the **resolved render
  camera poses and projection**, not Arbiter's rig targets. Not yet run in the game.

### Baseline to beat (Wrath-Mac, 2026-10-10)

Each figure is labelled with its build and workload; "native" means the arm64 player with
incremental GC.

| Metric | Value | Build, workload, source |
|---|---|---|
| Save load, first / median reload | 7.42 / 6.50 s | native + WrathPerf, Deskari bench save, 15 reloads (`fixtures/captures/runs-raw/loads/soak-both`) |
| Same, without WrathPerf | 12.39 / 11.44 s | native, same workload (`soak-base`; not the untouched Steam game) |
| Frame rate | ~72 fps, all in-game frames | native, Deskari protocol, 240 Hz display, no WrathPerf (`runs/arm64-igc/run2.md`) |
| Hitches ≥ 50 ms | 8 per run (5–6 on first play: pipeline creation, asset loading and JIT together) | same |
| Memory | 4.4–4.6 GB resident, Mono heap 0.5–0.6 GB, +0.2 GB over 15 reloads | `soak-base` / `soak-both` |
| Hardware | Mac Studio, M2 Max (8P + 4E cores, 30-core GPU), 32 GB; 2560×1440 at 240 Hz; macOS 27 | |

### Toolchain on this Mac

- **Rust isn't installed yet.** Install with rustup (POC-0).
- **Pinned after the POC-0 rendering spike: `bevy =0.20.0` with `wgpu =30.0.1`**
  (`docs/poc0/rendering.md`). The findings:
  - **Owlcat's shaders:** the game ships every shader variant as **plain Metal source**,
    and wgpu 30 accepts MSL as-is. Real `Owlcat/Lit` shaders draw unmodified through
    wgpu and inside Bevy, with the expected values at 81 checked pixels (exact BC1 texels
    included), and all 3,803 Lit fragment variants build. That's an integration smoke
    test, not whole-frame parity. Approach: run Owlcat's own shaders and port the render
    pipeline around them (unvalidated estimate for the caves: 17–23 weeks, with particles
    and VFX Graph on top).
  - **Pipelines:** with a load gate, no pipeline was created after the first visible
    frame; without it, early frames stalled for up to 242 ms. Cold Metal compiles cost
    91–173 ms per Owlcat shader (8 threads ~5× faster), and 0.12 ms with macOS's cache warm.
  - **240 Hz is the open problem.** Bevy's CPU frame is ~4.8 ms almost whatever the
    character count, against a 4.17 ms budget. Fifo settles at a juddering 150 Hz;
    Immediate gives 207 Hz with tearing. Drawable-to-panel latency is ~13 ms with Fifo and
    ~7.5 ms with Immediate. The GPU is fine: ~3.1 ms for 150 skinned characters at 1440p.
  - **GPU timing:** Metal gives timestamps only at pass boundaries, so per-pass GPU time
    comes from an automated `xctrace` script.
  - **MetalFX:** spatial (0.5 ms) and temporal (2.3 ms) upscaling work. Frame
    interpolation runs but isn't demonstrated yet. `bevy_metalfx` targets Bevy 0.19; we
    use our own bindings.
- .NET 11 preview SDK (Wrath-Mac builds UMM mods with it), Unity 2020.3.48f1 editor,
  `ilspycmd` (`DOTNET_ROLL_FORWARD=Major DOTNET_ROLL_FORWARD_TO_PRERELEASE=1`).
- Wrath-Mac's `monohost` (`native/wrathdxt/test/monohost.c`) runs the game's managed code
  on the game's arm64 Mono outside Unity. It provides only two Unity internal calls
  (`MemCpy`, `MemSet`), enough for the DXT compressor. Rules code needs `Game.Instance`,
  active facts, RNG and sometimes views, so monohost is **not an oracle** until its
  dependency closure is built and its results are cross-checked against the live game.

## Lessons from other rewrites

Details: [`docs/research/other-rewrites.md`](docs/research/other-rewrites.md). These are
engineering examples, not evidence of how fast or how much faster this will be.

- **Skate 3 and MW2 are static recompilations**: Xbox 360 machine code translated to
  C++, with a hand-written runtime. Skate 3's big macOS speed-up (~25 → ~250 fps) came
  from replacing an *emulated* GPU. Wrath-Mac already renders natively on Metal, so
  don't expect that kind of jump from the renderer alone. Both projects are AI-assisted,
  and the main criticism of them is unmaintainable code. Skate's own docs list parity
  defects in the new renderer.
- **Rust Minecraft reimplementations** succeed on the server (logic and data) and stall
  on the client (presentation).
- **CRPG engine reimplementations** (GemRB, xoreos, OpenMW) need the user's data and have
  taken over a decade. OpenMW does compare against the original, so our advantage isn't
  having a reference. It's how easily we can **instrument** ours from inside (Harmony
  hooks on decompiled code), and with how much detail.

What we take: our goal is **verified equivalence over a stated test domain**, through
translation from the decompiled spec and differential testing against the instrumented
game. We reuse native pieces Ember already has where that's better (Wwise, if POC-0's
spike works). We put the speed-up where Wrath actually loses time: loading, GC,
first-use pipeline creation and asset handling. We review everything adversarially.

## Architecture (proposed)

```
wrath-rust/                  Cargo workspace
  crates/
    wr-content/      blueprints: zip reader, GUID type registry, generated typed schemas,
                     opaque fallback, reference resolution, compiled content pack
    wr-save/         .zks reader (Newtonsoft graph: $id/$ref, converters), hydration,
                     preservation of state it doesn't model
    wr-sim/          the game: entities, parts, facts, stats (ModifiableValue), rulebook,
                     mechanics context, action/condition/evaluator registries, dialogue,
                     etudes, quests, flags, commands, AI, time, RNG interface.
                     Plain Rust, no Bevy, deterministic, headless.
    wr-text/         localization packs, template engine
    wr-audio/        Wwise over FFI (if POC-0's spike works)
    wr-assets/       runtime side of the Apple-optimized asset packs (mmap, streaming)
    wr-game/         the Bevy app: world, characters, camera, UI, input; drives wr-sim
  tools/
    wr-import/       offline: Unity bundles -> our packs (AssetRipper, then our own
                     converter), keeping original asset IDs; scenes -> gameplay objects
                     and navmesh
    wr-codegen/      offline: decompiled C# -> TypeId registry + serde schemas for the
                     transitive field-type closure, following FieldsContractResolver
    wr-oracle-replay/  replays oracle fixtures through wr-sim and diffs results
  docs/
```

Game-side oracle tooling lives in `oracle/` (`oracle/mods/WrathOracle`, a UMM/Harmony mod
built like WrathStats, against `builds/arm64-oracle`; it was in Wrath-Mac until 2026-10-12).

### Decisions (proposed; record changes here)

1. **`wr-sim` is engine-agnostic plain Rust**, following the contract above: mirrored
   ordering, nested rule stacks, fact delegates with per-fact state, durable game IDs
   separate from Bevy `Entity`s. Bevy hosts presentation and calls into it. Bevy doesn't
   order systems unless told to, and rule order is T0.
2. **Content comes from `blueprints.zip`** through generated schemas, and decoded objects
   are compared with the live game's loaded objects in a defined canonical form, which
   settles zip-versus-`.bbp` equivalence object by object.
   **POC-2 first round** (`docs/poc2/content.md`): `tools/wr-codegen` generates schemas
   for 4,202 types and 401 enums from the decompiled C#, and `crates/wr-content`
   decodes **all 236,661 blueprints fully typed** (2.50 M objects, zero opaque values) in
   1.4 s, keeping editor-only properties rather than dropping them. Of 889,268 typed
   references, 2,671 point at nothing and 245 at the wrong type. The memory-mapped pack
   (494 MB, built in 1.4 s) is ready in 25 µs warm and 1.2–1.5 ms cold; a GUID lookup
   takes about 125 ns. A reader for the runtime `.bbp` decodes every record with the same
   schemas: **98.83% match the zip in canonical form**. Most of the rest is an
   editor-computed field the zip lacks (2,352) and stale localization keys in the zip
   (379). Proposed (pending Ember): build content from the `.bbp`, which is what the game
   loads, and use the zip only to fill gaps. **Decided by Ember on 2026-10-11: the `.bbp` is
   the source.** Still to do: the live comparison (WrathOracle
   dumping loaded blueprints in the canonical form) and the pack's asset list.
3. **Read original saves; write only our own, faster format** (Ember, 2026-10-10: reading
   is enough). Original `.zks` saves are imported (T0), preserving every document and
   field the engine doesn't model, so nothing is lost. Our format is designed for speed:
   a versioned binary snapshot (e.g. `rkyv` or `bincode`, compressed with zstd), taken
   from the sim on the main thread in well under a frame and written on a background
   thread; loads memory-map and validate it. Consider incremental saves (only areas and
   entities that changed). Target: manual save at most 25% of the original's 0.33 s, with
   no frame over 33 ms while saving, and loading faster than the original's equivalent.
4. **Assets are imported offline** into our own packs; the runtime never parses Unity
   formats. AssetRipper (GPL, used as a local tool) handles the Unity side, subject to the
   POC-0 extraction gate. The Rust Unity parsers (e.g. `unity-asset` 0.3) are too young
   for meshes.
   **POC-0 result** (`docs/poc0/extraction.md`): our own `tools/wr-import` reads the
   bundles directly through their embedded type trees. Every object in the area's
   200-bundle closure (487,816) and in all 2,134 `.scenes` bundles (8.2 M, 18.5 s) decodes
   with every byte consumed, and every scene, asset and blueprint reference in the caves
   resolves. AssetRipper 2.0.0 (headless) agrees: 9,457 of 9,460 meshes are byte-identical
   and every transform is bit-identical. So AssetRipper becomes a cross-check, and our
   reader the import path. Missing: seven Owlcat shader families, crunch-compressed
   lightmaps (we need Unity's crunch decoder), VFX Graph and compute shaders, and
   spawn-time bundles that `dependencylist.json` omits. The `.nav` cache is plain A*
   Pathfinding Project 4.3.10 (a tiled Recast navmesh plus a grid); exact path parity
   still needs A*'s tie-breaking, nearest-node rules, clearance, cuts and modifiers.
   Estimate for a production importer, excluding shaders and VFX: 12–18 weeks.
5. **Audio:** reuse Ember's own Wwise runtime over FFI *if* the POC-0 spike shows full
   integration (initialisation, banks and packages, streamed voice, callbacks, 3D
   positions, suspend and resume, shutdown). Otherwise we decide again.
   **POC-0 result: go** (`docs/poc0/wwise.md`). Rust `dlopen`s the game's
   `AkSoundEngine.bundle` (Wwise 2019.2.15; no Unity or Mono dependency), initialises it
   with the game's own settings decoded from `sharedassets0.assets`, loads the 11 preload
   packages and 8 banks in 45–83 ms, and passes every item above, measured from Wwise's
   output capture. It costs about 1% of a core and 140 MiB. Budget: 1.5–2 weeks for the
   `wr-audio` runtime, 4–6 weeks for full audio parity (mostly the game's audio behaviour
   and scene data). The Wwise profiler connection stays as a developer option that a final build must be able to turn off (Ember, 2026-10-11). **Ember's listening check passed (2026-10-11):** no distortion, sounds as expected.
6. **UI is new**, in Bevy: the game's information architecture and view-model behaviour,
   not a port of 274k lines of prefab-bound views.
7. **Fail closed, for real.** Executing unsupported semantics invalidates the
   compatibility run and **blocks progression saves**. The original's own behaviour of
   catching an action's exception and continuing (`ActionList`) is reproduced as
   behaviour, and kept separate from "we haven't implemented this". Coverage combines
   executed types with **required-vs-implemented manifests** for active facts, callbacks,
   scene components and lifecycle stages, which catch things that never execute.
8. **No clean-room claim** (see Ground rules).

## Apple Silicon: how the rewrite should beat the original

T2 says free, but better. The levers below are hypotheses until measured.

| Area | Original | Rust/Bevy design |
|---|---|---|
| Garbage collection | Mono GC; incremental GC removed GC hitches but costs heap and write barriers | No GC: ownership, arenas for transient rule objects |
| Blueprint startup | Binary pack deserialized by reflection, lazily | A compiled content pack (zero-copy, e.g. `rkyv`) memory-mapped; index-only startup |
| Loads | 6.5–12.4 s (cleanup passes, atlas building, Unity scene loading) | Area packs built offline, memory-mapped and streamed on efficiency cores (QoS utility); no cleanup waits; no runtime texture compression |
| Textures | DXT (BC1/BC3), plus runtime atlas compression | **Keep the shipped BC blocks unchanged**: Apple Silicon Macs sample BC natively, so textures stay bit-identical at no runtime cost. Equipment layers composed on the GPU with Owlcat's four atlas shaders (`CharacterAtlas`) reimplemented exactly; atlases compressed with Wrath-Mac's byte-identical native DXT port (off the main thread) so they match the original too |
| First-use stalls | Pipeline creation, asset loads and JIT on first play | No JIT; every pipeline the area needs is created during the load. Persisting Metal pipelines across launches is a separate experiment (wgpu's pipeline cache is Vulkan-only) |
| Rendering | Unity 2020.3, native Metal | Bevy on wgpu/Metal, GPU-driven, with the original's shading; frame pacing tied to the display. MetalFX upscaling and frame interpolation as options, labelled as additions |
| Unified memory | Unity's staging copies | Minimise copies; shared-memory mappable buffers where wgpu allows (to verify) |
| CPU | Mostly main-thread logic | Deterministic sim on one P-core; pathfinding, pure AI scoring, animation and streaming in parallel |
| Memory | 4.4–4.6 GB resident | Target under half (mmap'd packs are reclaimable page cache) |
| Saves | 0.33 s manual save (JSON through Newtonsoft, zipped) | Our own binary snapshot format (Decision 3): snapshot in well under a frame, written in the background |
| Audio | Wwise | The same Wwise if the spike works. "No regression" is a test, not an assumption |

**Baseline for the POC area, measured in POC-0** (`docs/poc0/baseline.md`; native arm64,
incremental GC, Manual_13, the eight-point Caves_1 camera route; three clean runs per
configuration, median with range):

| | Game's own behaviour | Best original (WrathPerf on) |
|---|---|---|
| First load, fresh process | 10.27 s (9.94–10.80) | 6.00 s (5.95–6.20) |
| Reload, same process | 9.07 s | 5.00 s |
| First load, OS shader cache cleared (1 run) | 12.24 s | 7.77 s |
| Route frame time p50 / p95 / p99 | 12.45 / 17.0 / 21.1 ms | same |
| Route frames ≥ 33 ms, warm shader cache | 0–1 per run | 0 |
| Manual save | 0.465 s, with one frame of ~250 ms | 0.469 s |
| Resident memory | 3.50 → 3.65 GB | same |

The original is vsync-locked: frame times sit at multiples of 4.17 ms, so it caps at
80 fps on the 240 Hz display. It already has almost no route hitches with a warm shader
cache, so the hitch target bites mainly on cold caches, first use and saving. The
screenshot noise floor (the original against itself, three processes) is FLIP 0.021–0.074
per point, from particles, pulsing shaders, idle animation and a 3–23 mm camera drift.
**We measure against "best original"** (Ember, 2026-10-11), the harder bar: that means a load ≤ 1.5 s,
p99 ≤ 15.8 ms, and a manual save ≤ 0.12 s with no frame over 33 ms.

**Named targets for the POC area** (vs the original in the same area, same workload; see
POC-6): load ≤ 25% of the original's; frame time p99 at least 25% lower at native 1440p
with matched settings; zero hitches ≥ 33 ms in a fixed 5-minute route after load;
resident memory ≤ 50%; manual save ≤ 25% of the original's. **Non-regressions:**
input latency, visual output (the visual rule), audio, and the T0 fidelity results.

## The oracle: the real game as the test suite

`oracle/mods/WrathOracle`, a UMM/Harmony mod in the style of WrathStats and WrathBench.
Hook points are in survey §B. The oracle is **certified before its results count**
(POC-1).

- **Rules:** `IRulebookEventAboutToTriggerHook` and `IRulebookEventDidTriggerHook` through
  `EventBus.Subscribe`, plus a postfix on `RulebookEvent.OnDidTrigger`. Nested traces
  with parent IDs, inputs, modifier breakdowns and results, plus **per-handler execution
  and state mutations**, not just event envelopes.
- **Every random draw, individually:** `Dice.D` returns a sum, so instrument the
  `UnityEngine.Random` call sites (transpilers at the managed call sites): dice, direct
  draws such as `RuleSkillCheck`'s, animation variants, attack sequences, dialogue
  selection and AI jitter. Record call site, range, value and sequence number.
- **Time:** each tick's delta and controller order, so replays can use the same deltas.
- **Dialogue:** `StartDialog`, `SelectAnswer`, `PlayCue`, `PlayCheck`, observed at the
  **completed transition** (after the `Tick` that plays the scheduled cue).
- **State dumps on demand:** every unit's character sheet with modifier breakdowns,
  inventory, resources and buffs with timers; narrative state; scene entities (IDs,
  spawners, transforms, zones); and blueprints as the live game loaded them.
- **Health:** recorder counters for dropped and failed records, because
  `RulebookEventHooksManager` swallows hook exceptions and a broken recorder wouldn't stop
  the game. Fidelity instrumentation stays off in performance runs.
- **Driving:** WrathBench already loads saves unattended. WrathOracle adds scripted
  inputs (move, start a dialogue, choose answer N, attack) so fixtures can be recorded
  without Ember.
- Fixtures record the exact build (game version, assembly and blueprint hashes, mods,
  difficulty, settings). Captures stay local and gitignored. Small derived test cases
  without game text can be committed.

## How the porting work gets done

1. **Codegen what's mechanical:** the TypeId registry, blueprint schemas for the
   transitive field-type closure, and save DTOs, all from the decompiled declarations.
   Behaviour can't be generated.
2. **Port behaviour with a spec bundle per type:** the decompiled source, base classes,
   real blueprint usages, and oracle fixtures that exercise its branches. Implementation
   by Claude or a Sonnet subagent (Opus when Sonnet falls short), and Sol (high; Astra for touchy work) reviews
   against the C#. A
   type is done when its fixtures pass *and* its branch coverage is reported.
3. **Track coverage** by executed type and against the required manifests (Decision 7).
4. **Track effort** from the start: per type, and separately for integration, fixture
   making, review and rework.

## Proof of concept (the first goal)

**Question:** can a Rust/Bevy engine run real Wrath content, including a fight, from
Ember's own save, with outcomes identical to the original, and run it measurably better on
this Mac? And what would the rest cost?

**Setting: the prologue caves, `Prologue_Caves_1`, starting from Ember's own save**
`Manual_13_Underground_Caves_of_Kenabres…`. It was saved right after the caves' opening
cutscene, before Camelia and Seelah/Anevia. The area has five scene bundles (`static`,
`light`, `audio`, `mechanics`, `terendelevscales_mechanics`) plus `prologue_caves_1.nav`.
It holds, in reach of the save:

- **Camelia's meeting** (`World/Dialogs/c0/CavesUnderKenabres/MeetCamelia/`): 23
  blueprints, 13 of 18 lines voiced, recruitment, `CompanionInParty` conditions, a
  `DealDamage` action.
- **Seelah and Anevia's meeting** (`…/MeetSeelahAnevia/`): 57 blueprints, 36 of 50 lines
  voiced, **4 skill checks**, recruitment, XP, etudes, objectives, a buff, cutscenes.
- **The first fights.** POC-0 found (`docs/poc0/fights.md`, gpt-6.1-sol, spot-checked) that
  **no fight lies between the save and the meetings**: the ordinary route is save →
  Seelah/Anevia → Camelia → **FirstBattle** (three young giant centipedes, melee only). The
  area holds nine encounter groups (ten enemy blueprints, 18 enemies at the save's Standard
  encounter capacity). Recommended fixtures: FirstBattle first, then **Optional1** (a melee
  and a ranged centipede) and **BeforeLann** (one monitor lizard, with a difficulty-gated
  poison). The party at the save is FennA alone (Fighter 1); Seelah, Anevia (an NPC ally
  with Friends AI) and Camelia (who takes 4 scripted damage on recruitment) join first.

The second rules case is **Horgus's negotiation** in the next area, `Prologue_Neathholm`
(`…/NeathholmVillage/Horgus_GetMeOut/`: 48 blueprints, a party Diplomacy check at authored
DC 16 setting a price flag to 2000 or 1000, survey §A). It's a clean check-to-flag
branch, but **unvoiced** (none of its 42 lines has a `Sound.json` mapping), so it can't
test audio. Ember will make a Neathholm save by playing on from Manual_13.

Milestones run in order unless noted. POC-0 front-loads the riskiest unknowns.

### POC-0: setup and risk spikes

**Status (2026-10-11): done.** Reports are in `docs/poc0/`: `rendering.md`, `wwise.md`,
`extraction.md`, `baseline.md`, `fights.md`, `arbiter.md`. Setup: Rust 1.99.0 via
Homebrew `rustup` (pinned in `rust-toolchain.toml`; PATH in `~/.zshenv`), cargo-nextest,
`scripts/check.sh [dir]`. Spikes are standalone workspaces under `spikes/`. Game-side
tooling: `WrathOracle` in `oracle/mods/` and `oracle/tools/oracle-*.sh`.
**Dump vs import** (`wr-import diff`, rerunnable): all 157 entity views, 33 spawners and
17 zones match the live game (1,447/1,447 serialized fields, 409/409 blueprint
references, bit-identical transforms); all 5,204 GameObjects are present, and the only
mismatches are things the game changes at runtime. The navigation graphs are identical.
On 46 queries every A* search field matches; the smoothed corners match in 26–44 of 46
depending on the query kind, the rest most likely down to Unity's native
`Quaternion.FromToRotation` (to confirm with an oracle capture). Still to do: a
vsync-off route run for raw render cost.

- rustup and stable Rust; the Cargo workspace; clippy, fmt, nextest; scripts in the style
  of Wrath-Mac's `tools/`.
- **Rendering spike on the M2 Max:** Bevy 0.20, a skinned test mesh, presentation at
  240 Hz, real GPU timings, and building pipelines during a load. Above all, check that
  Bevy's renderer can host **Owlcat's shading exactly** (custom materials and render
  passes for `Owlcat/Lit`-style shaders, their lighting and post-processing), since the
  visual rule depends on it. MetalFX upscaling and frame interpolation (`bevy_metalfx`
  or our own `objc2` bindings) are wanted as options; prove they work, but don't let them
  decide the version if they're the only blocker. Output: the version to pin, and what
  works.
- **Wwise spike:** standalone use of the game's `AkSoundEngine.bundle` from Rust:
  initialisation, banks and packages, one streamed voiced cue from MeetSeelahAnevia,
  callbacks, a moving 3D source, suspend and resume, clean shutdown. Output: go or no-go
  for Decision 5, and an integration budget.
- **Extraction spike, dependency-complete:** all of `Prologue_Caves_1` (five scene
  bundles, `.nav`, dependencies) through AssetRipper into a first `wr-import`. Compare
  with a live oracle dump: entity IDs, spawners, transforms, zone geometry, references.
  Decompile `AstarPathfindingProject.dll`, decode the graph, and compare path queries with
  the live game. Count unresolved references and unsupported shader and component
  families. Output: the import yield, and the list of what's missing.
- **Baseline:** define the matched workload (POC-6) and measure the original on it: cold
  and warm area load, frame-time percentiles on the route, memory, manual save time.
  Capture the original against itself at the route's points to calibrate the perceptual
  metric's noise floor. Check whether Arbiter works for camera routes.
- Identify the fights between Manual_13 and the two meetings.

### POC-1: oracle certification

**Status (2026-10-11): passed** (`docs/poc1/oracle.md`). WrathOracle 0.2 records rules,
handlers, every RNG draw, time, dialogue and state; three certification fixtures from Manual_13
record cleanly and replay at the T0 level in five runs (`dice` bit-exact); injected faults are
caught. Astra reviewed it three times; round 3 tightened certification (only audited T1 RNG
sites may differ on replay, durations kept, nesting and install checked), and `cert-10`
passes under those rules.

- `WrathOracle` with the hooks above, recorder health counters, and state dumps.
- Certification fixtures from Manual_13: a nested rule, a multi-die roll, a reroll, a
  forced roll, a deferred dialogue transition, a temporary modifier, and an animation-
  variant draw.
- **Mutation test:** deliberately perturb one recorded value or draw and show the diff
  catches it; deliberately break a hook and show the health counters catch it.
- **Pass:** every certification fixture records completely and replays its own capture
  consistently; injected faults are detected.

### POC-2: content

- Generate the TypeId registry and schemas for the transitive field-type closure.
- Report separately: JSON readability (all 236,661), **typed coverage** (share decoded
  through real schemas, not opaque), reference resolution, initialisation, and behaviour
  support.
- Compare decoded objects with the live game's in canonical form: the slice's whole
  dependency closure, plus a **stratified corpus** (every type and serialization pattern,
  rare ones included).
- Build the content pack; measure cold and warm open and lookup costs.
- **Pass:** zero mismatches on the compared set; typed coverage reported per type; the
  content system ready in under 100 ms from a warm pack (cold cost reported).

### POC-3: saves (wr-save, wr-sim)

- Read `.zks` with its graph and converters (BOM-only `tbm.json`, `x|y|z` positions,
  `[{Key,Value}]` dictionaries, component-name-keyed state); hydrate; run fact activation
  and post-load lifecycle (`EntityDataBase` restore).
- **Snapshot equivalence:** every party unit's character sheet with breakdowns, plus
  inventory, spell and ability resources, and buffs with timers, equal to the oracle's dump.
- **Mutation after load,** compared step by step with the oracle: equip and unequip,
  apply, remove and expire a buff, spend a resource, advance narrative state, move to
  another area and back, save, reload, repeat.
- **Preservation:** a load and re-save in our format keeps every document and field we
  don't model (e.g. Kenabres state in Manual_13).
- **First round done (2026-10-10, `docs/poc3/saves.md`):** `crates/wr-save` reads
  `.zks` with the game's Newtonsoft settings and converters, through save-side schemas
  generated by `wr-codegen`. **All 17 of Ember's saves decode 100% typed** (4.84 M objects,
  522 concrete types; every `$ref` and blueprint reference resolves), and preservation
  holds for all 17 (`.zks` → our format → back gives byte-identical canonical text). Our
  format (postcard + zstd per entry) is 53–62% of the `.zks` size; Manual_10 encodes in
  49 ms and decodes in 40 ms, against 140 ms for a typed decode of its JSON. So the long
  tail that matters is behaviour, not decoding. Next: hydration (the post-load lifecycle
  order, fact `PostLoad`/`TurnOn`, matching component state to blueprint components by
  name, stats `PostLoad`/`UpdateValue`, matching scene entities by `UniqueId`), which needs
  `wr-sim`, and snapshot equivalence, which needs WrathOracle's character-sheet dumps.
- **Second round (2026-10-11, `docs/poc3/hydration.md`, `docs/sim/architecture.md`):**
  `crates/wr-sim` now hydrates saves with the C#'s post-load lifecycle. Against
  WrathOracle's after-load dumps, Manual_13's **main character matches 507/507 fields**
  and its four units 97.0% (2,301/2,372). Every miss is one unported component,
  `DifficultyStatAdvancement`, which the fail-closed manifest flags on exactly those
  units. Manual_10: 16 units hydrate in 0.31 s, 97.3% match, three units at 100%. Next:
  `DifficultyPresetsList` and `DifficultyStatAdvancement` (in POC-4), `ContextRankConfig`,
  spellbooks, fact creation and repair at load, conditions, and unit placeholders. **Update (POC-4 round):** with `DifficultyPresetsList` extracted (`tools/wr-rules-data`) and `DifficultyStatAdvancement` ported, **Manual_13 matches 2,372/2,372 (100%)**; Manual_10 is unchanged at 97.35%.
- **Pass:** all of the above for Manual_13 **and** a richer save, Manual_10 (Defender's
  Heart, a full party in Act 1). Ember's other saves (Shield Maze, Drezen, Worldwound) are
  reported as match rates per type, to show the shape of the long tail.

### POC-4: dialogue, headless

- The rulebook core, the check rules, the difficulty adjustment (extract
  `DifficultyPresetsList` from `blueprint.assets`), XP, the dialogue controller, the
  conditions and actions both meetings use, and text with template expansion.
- **Seelah and Anevia's meeting, complete,** including world-side effects: cutscenes as T1
  (positions, facing and timing within tolerance; same T0 events), objectives, etudes,
  recruitment and the buff.
- **Horgus's check,** with Sol's ten acceptance tests (survey §A), on a success and a
  failure tape, from a Neathholm save that Ember makes by playing on from Manual_13.
- **Pass:** final state (flags, etudes, quests, XP, histories, party, character sheets)
  equal to the oracle's for at least two answer paths per dialogue, all four checks on
  success and failure tapes.
- **Second round (2026-10-11, `docs/poc4/dialogue.md`):** `wr-sim dialog` runs a fixture
  headless on the oracle's tape and compares one ordered T0 stream (dialogue, mutations,
  rules with results, draws) and the end state. The certification fixture matches
  completely (78/78 records) on record, replay and the perturbed success branch. Seelah/
  Anevia paths A, B, C (12 runs, the three reachable checks each passing and failing) and
  Horgus (6 runs, the party Diplomacy check passing and failing, flag 2000/1000/500): every
  dialogue stream, rule draw and character sheet equal; the stream diverges only where the
  sim reports unsupported behaviour (building the recruited companion's unit, a buff, an
  item, the closing cutscene), so **no run is valid yet**. Narrative end state equal except
  one etude that **ToyBox** (active in the oracle build) keeps playing: the oracle should
  run without gameplay mods (needs Ember). Manual_19 sheets 100% at load, Manual_10 98.46%.

- **Round 3 (2026-10-11, `docs/poc4/dialogue.md`):** recorded again on the clean build
  (`poc4-seelah-2`, `poc4-horgus-2`): the ToyBox etude difference is gone. Placeholders become
  units at load (speakers resolve to them), `Recruit` builds the companion's unit with its
  items, spellbooks and resources are ported and compared: **all 12 Seelah/Anevia runs equal
  the oracle's whole ordered T0 stream and every sheet.** Still invalid: unported behaviour is
  reported in every run (the validity work was in flight at wrap-up, see the top). Horgus
  stops at the closing cutscene (`HideUnit`, `Spawn`).

### POC-5: one fight, complete lifecycle (headless first)

The strongest test of the architecture, and the reason combat is in the POC.

- The first fight reachable from Manual_13: commands with approach, attack or cast, hit
  timing from extracted animation data, interruption, damage, resulting facts and buffs,
  enemy AI choices, death and XP.
- **Turn-based first, then real-time with pause.** Ember plays both: real-time with pause
  for minor fights, turn-based for harder ones. Turn-based advances in discrete turns
  with action and movement budgets, so it depends less on continuous frame-delta time.
  In real-time, rounds run on game time in the background, so that's where the time
  contract gets its hardest test.
- Replay the oracle's tape and deltas; then free play in the Bevy build (POC-7).
- **Pass:** identical T0 event sequence and final state over the replay, for at least
  three recorded fights with different choices.
- **Status (2026-10-11, round 1, wrapped up mid-flight):** Manual_18 hydrates with all 15
  placeholders spawned (0.9086 field match, party 100%); WrathOracle drives turn-based
  fights (`tbfight`), exploration recordings only (made with ToyBox; re-record); the
  attack rule chain is written in `wr-sim` but not yet replayed. Handoff:
  `docs/poc5/combat.md`.
- **Round 2 (2026-10-11):** merged into `poc`/`main` and reviewed (two Astra rounds, all
  fixed); Manual_18 now hydrates to **100%** with the original's fact ids. Turn-based
  recordings on the clean build have zero unhooked RNG steps. Turn-based replays drifted from
  their recordings because the game's turn-based play is wall-clock coupled (the 4 s
  combat-start banner, camera and animation on Unity's clock, identity-hash scheduling, A*
  delivery; gpt-6.1-sol's analysis). The oracle rework to make them replay exactly was
  rejected in review for changing the game while recording and was being redone at wrap-up
  (`docs/poc5/tb-replay.md`). The sim must take those boundaries (when `WaitingForUI` clears,
  visibility phase, initiative tie order, path delivery) from the recording as inputs.

### POC-6: Prologue_Caves_1 in Bevy, on Apple Silicon (alongside POC-2 to POC-5)

**First light (2026-10-11, `docs/poc6/first-light.md`):** `crates/wr-assets` (a
memory-mapped area pack, 160 MiB: BC blocks as shipped, the game's MSL compiled to
metallibs, original asset IDs) and `crates/wr-game` (Bevy 0.20) render the caves' static
geometry through **Owlcat's own** G-buffer (Lit, Unlit, Terrain), reflections,
light-culling compute, deferred lighting, fog, grading LUT and Uber, from the eight
recorded route poses. Ready in 0.15–0.17 s, first frame at 0.33–0.38 s. FLIP per point
0.077–0.257 (mean 0.144) vs a noise floor of 0.021–0.074. Missing, roughly by visual
impact: units, particles and VFX, light shafts and bloom, shadows, light probes,
HBAO/SSR, decals, SMAA and grain. Side-by-side sheets:
`fixtures/captures/poc6/first-light/side-by-side-*.jpg`.
**Round 2 (2026-10-11, `docs/poc6/render-round2.md`):** the colour space is settled from the
code and data: lighting and fog write sRGB-encoded numbers into an RGBA16F target, and
post's first step, Owlcat's Stop NaN blit, decodes them; we now run that shader instead of
our fitted decode. BloomEnhanced runs on Owlcat's shaders with the cave profile: **FLIP mean
0.126** (from 0.144). Frame stalls: first light's 1.09 s one fits the display going idle (the
original holds a no-display-sleep assertion; we now do too); with it, a 5-minute route had 0–4
intervals ≥ 33 ms, in short bursts of CPU starvation that most likely come from other load on
the shared machine. G-buffer draws are pre-recorded in a render bundle and the grading LUT is
built once (identical pixels, less CPU and GPU). With bloom the route's intervals are p50
7.3–7.6 ms, p99 12.7 ms (target ≤ 15.8): the 3.4 ms GPU frame now misses 240 Hz about half
the time. Shadows are specified (`docs/poc6/shadows-spec.md`), not built; they're next.
**Round 3 (2026-10-11, `docs/poc6/render-round3.md`):** realtime spot and point shadows through
Owlcat's buffer-driven caster path (4096² depth atlas, MaxRects allocation, tetrahedral point
faces, the `SHADOWS_SOFT` lighting variant; rendering-layer test and two-sided casters after
review): **FLIP mean 0.1246** (from 0.1262); on the pixels shadows change, mean error 10.8 →
8.5. Open, needing a Metal frame capture of the original: the atlas format, compare function
and clear value, the shipped pipeline settings. Next: light probes, then the rest of post.

- The imported area: geometry, materials (Owlcat's shading reimplemented to match), lights, the navmesh, the gameplay objects, the NPCs and party with equipment
  composed on the GPU, animation driven by sim state, effects in the fight, fog of war,
  and the game's camera.
- **Matched workload:** a fixed camera route (Arbiter points if they work), the same
  active entities and NPC behaviour, the same effects, shadows and fog, native 1440p for
  the like-for-like comparison, plus the same route with MetalFX options on, reported
  separately; the same settings, defined cache states,
  a defined load-completion boundary, five minutes recorded, background activity checked.
- **Visual equivalence (the visual rule):** captures of both engines at the route's
  points, same camera, time of day and settings. Screen them with a perceptual metric
  (e.g. NVIDIA's FLIP). Calibrate its threshold in POC-0 from the original against itself
  (run-to-run noise: animation phase, particles), and flag anything above it. Then a blind
  A/B by Ember: pass if Ember can't reliably tell which is which. Sol (computer use) does
  the capture runs.
- **Report:** rendered fps, displayed fps, input latency, frame-time percentiles, memory,
  cold vs warm load, save and load times in our format. **Pass:** the named targets above,
  visual equivalence, and no non-regression failures.

### POC-7: the slice end to end

- In the Bevy build, from Manual_13: walk the caves, meet Camelia, fight, meet Seelah and
  Anevia, with dialogue UI, portraits, voice, checks and the fight played live, then save
  and reload in our format.
- **Pass:** a scripted run (the same inputs, tape and deltas as an oracle run) ends in the
  same state as the original; a free play-through feels like Wrath (Ember's judgement).
- **Status (2026-10-18):** stage 1 of `docs/poc7/characters.md` done (the save's units composed and placed, rest
  pose); animation is next.

### POC-8: what the rest would cost (alongside POC-2 to POC-7)

- Port a **sample stratified by subsystem and complexity**, not by frequency alone:
  common types, rare interacting mechanics (e.g. from `ContextRankConfig`,
  `AddClassLevels` and polymorphs), a cutscene command family, and an AI action family,
  each with fixtures.
- Combine the effort data (types, integration, fixtures, review, rework), POC-3's long-tail
  curve, the survey's subsystem sizes and POC-0/6's import yield into **scenario
  estimates** for the prologue, Act 1 and the full game, with explicit exclusions. Report
  uncertainty bounds only once there's enough representative data.

### Go / no-go

At the end, write `docs/poc-report.md`: each pass criterion with evidence; the matched-
workload comparison; the scenario estimates and what they exclude; the riskiest unknowns
left (expected: cutscenes at scale, character visuals at scale, UI breadth, the long
tail); and a recommendation. Ember decides.

**It's "doable"** if T0 holds over the slice (dialogue *and* combat, plus mutation after
load), the named T2 targets are met without regressions, and the estimated cost is one
Ember wants to pay. Any of these failing is also a real result.

## After the POC (sketch only; plan in detail after go)

1. **The prologue, playable:** Kenabres to the end of the caves, all its fights and
   cutscenes.
2. **Character creation and level-up** (progression, feats, spellbooks): about as big as
   rules and dialogue combined.
3. **Act 1 and beyond,** driven by the coverage tracker. Global map, kingdom and crusade
   are separate projects inside this one.
4. **Mods:** UMM/Harmony mods can't run on the new engine. **Ember wants ToyBox and Buff It 2
   The Limit recreated as mods of our version** (2026-10-11; Ember, 2026-10-14: ported over as
   separate, optional mods by the end, not baked into the rewrite; both are MIT, keep their
   notices), in whatever form our build
   supports best (Rust plugins, a scripting layer; not necessarily C#). So `wr-sim` and
   `wr-game` need a mod API: rule and controller hooks, settings and UI panels, with every mod
   option labelled as an addition and recorded in our saves. Mods stay off for fidelity tests
   (the oracle runs on a clean build); Ember's play build, which has both mods, can serve as the
   oracle for our reimplementations (the same fixture with and without a mod feature). Start
   from the features Ember actually uses (ToyBox's `Settings.xml` lists them).

## Risks (ranked; full list in survey §C)

1. **Silent semantic incompleteness:** thousands of behaviour types and lifecycle
   callbacks. Mitigation: real fail-closed, manifests plus executed coverage, oracle diffs.
2. **Presentation leaking into rules** (posture, animation timing, RNG shared with
   animation, frame-delta time). Mitigation: the simulation/presentation contract; combat
   in the POC.
3. **Scene-authored gameplay and asset extraction.** Mitigation: the dependency-complete
   extraction spike in POC-0, compared with live dumps.
4. **Save hydration vs merely parsing.** Mitigation: snapshot, mutation-after-load and
   preservation tests on two saves.
5. **An oracle that misses things.** Mitigation: certification and mutation testing
   (POC-1).
6. **Presentation scale:** paper-doll equipment, shaders, VFX, UI.
7. **Third-party stack maturity:** Bevy releases, wgpu's Metal limits, MetalFX bindings,
   Wwise FFI. Mitigation: POC-0 spikes and version pinning.
8. **AI-written code nobody can maintain.** Mitigation: small crates with clear seams,
   Astra review of every change, specs and fixtures beside each ported type.
9. **Licensing** (Ground rules): unresolved, Ember's call.

## Answered by Ember (2026-10-10)

- **Combat mode:** Ember plays both, real-time with pause for minor fights and turn-based
  for harder ones. Turn-based is tested first (POC-5).
- **Fixture saves:** Ember plays to make them (e.g. Neathholm before Horgus); it's faster
  than automating it.
- **Licensing:** continue (see Ground rules).
- **Remote:** a private GitHub repo, `pf-wotr-rs`.
- **Saves:** reading original saves is enough; write only our own format, optimized for
  speed (Decision 3).
- **Visuals:** as close to the original as possible; optimize and clean the
  implementation freely, but not in ways that change the output perceivably (the visual
  rule, in the fidelity contract). Upscaling and similar techniques are fine as options,
  with every option labelled as original or an addition.

## Answered by Ember (2026-10-11)

- **Benchmark bar:** measure against the best original (the native build with WrathPerf
  on), the harder bar.
- **Fight fixture save:** Ember will play on from Manual_13 and save just before
  FirstBattle (after recruiting Seelah and Camelia). It's **the last task of the current
  round**, and nothing that relies on it starts before then. Manual_13 and Manual_10 are
  backed up read-only in `fixtures/saves/`.
- **Wwise profiler:** keep the profiler connection as a developer option for now. It must
  be **possible to disable it in a final build** (the game opens profiler ports on all
  interfaces).
- **Xcode's Metal toolchain:** installed 2026-10-11 (`xcodebuild -downloadComponent
  MetalToolchain`, Metal Toolchain 27A5194o); `xcrun metal` and `metallib` work, so
  Owlcat's shaders can be built offline into metallibs.
- **Content source (Decision 2):** build content from the runtime `blueprints-pack.bbp`,
  the file the game loads, with `blueprints.zip` filling gaps.
- **Disk:** Ember freed space on 2026-10-11 (the old Wine/GPTK builds); about 94 GiB free.
- **Oracle without gameplay mods:** ToyBox changes game logic (its any-gender-romance patch
  keeps an etude playing that the original stops), and it and BuffIt2TheLimit were loaded in
  every oracle run until then. The oracle now runs on `builds/arm64-oracle`, an APFS
  clone whose `Mods/` holds only our own mods; Ember's play build `arm64` is unchanged.
  Recordings made before the switch were made with ToyBox loaded.
- **Mods in our version:** ToyBox and Buff It 2 The Limit should be recreated as mods of the
  new engine by the end, in any language our build supports well (After the POC, item 4).

## Open questions for Ember

None right now. Add them here as milestones raise them.
