# POC-5: group memory and visibility (stream F)

Status (2026-10-15): design, and the first part the recordings establish implemented and checked: the memory's
bookkeeping (`crates/wr-sim/src/combat/memory.rs`), driven by the recorded memory changes, reproduces every
membership read recorded in `poc5-tb-103` (59,991 `Memory.Enemies` reads, 369 flat-footed `Memory.Contains` reads,
609 dictionary rows, 42 cached `UnitsList`s, 6 friends' additions on their frame). The visibility reads
(`ContainsVisible`: the impatience controller's `ivis` and `imp_s` `vis`, 16 a record run) aren't checked: `Visible`
isn't recorded. Not wired into the driven replay (the
recordings don't carry what the controller's tick reads at each call: "Observer changes requested"). Tags as
elsewhere: [M] measured, [C] from the decompiled code, [I] inferred.

Group memory and visibility are among the biggest inputs the POC-5 replay still takes from the recording: the AI's,
the engagement controller's and `UnitAttack`'s `enemies` reads, the engagement's `los`, the commands' `stealth`, the
flat-footed check's `target_remembers_initiator`, the impatience controller's `ivis`, the leave controller's memory
rows, the turn order's `IsVisibleForPlayer`, the sleeping units and ready groups (fog of war). This note says what the
game computes, what each reader reads, what the sim can compute and in which order, and the records a slice needs.

## What the game computes [C]

### `UnitGroupMemory` (one per `UnitGroup`, `Kingmaker.UnitLogic`)

- `m_Units`: a .NET `Dictionary<string, UnitInfo>` keyed by unique id. Enumeration is the entries array's order
  (a new key takes the most recently freed slot: `csharp/dict.rs`). `UnitInfo`: the `UnitReference`,
  `LastDetectTime` (game time) and `Visible` (not serialized; false on a new entry). The memory isn't in the save in
  practice: `UnitMemoryController.Start` clears every group's memory at area load, so the window's memory is what
  the frames since the load built.
- `Add(unit)`: invalidates both caches (even when the unit is already there), creates the entry with
  `LastDetectTime = GameTime` if new, returns it. `Remove(unit)`: invalidates only when it removed. `Find`,
  `Contains`, `ContainsVisible` (`Visible`). `Clear`, `ClearEnemies`.
- `UnitsList` (units that resolve and are `IsInGame`) and `Enemies` (also `Group.IsEnemy(unit)`) are cached lists
  with their own validity flags. `ClearEnemies` comes from every `UnitGroup.UpdateAttackFactionsCache` (a unit added
  to or removed from the group, `UnitGroupsController`'s deferred faction updates, a spawn, a destruction).
- `Cleanup()`: removes each entry whose unit doesn't resolve, is out of the game, or wasn't detected for more than
  9 s (`LastDetectTime + 9f.Seconds() < GameTime`, `TimeSpan` ticks, strict), in enumeration order (so the last one
  removed heads the free list). It removes from the dictionary directly: **neither cache is invalidated**, so a
  cached list can still hold a removed unit (WrathOracle's `leave_tick` comment says the same).
- `HasPlayerCharacterInMemory()`: any unit of `UnitsList` of the player's faction (a cached unit that no longer
  resolves throws).
- `Group` resolves lazily through `UnitGroupsController.GetGroup(GroupId)` (which can create a group).

### `UnitMemoryController` (`Kingmaker.Controllers.Units`)

Registered (`GameModesFactory`, Default mode only) after `SleepingUnitsController` and before `UnitGuardController`,
`UnitCombatJoinController` and everything after. Its `Tick`:

1. Returns at once in a peaceful area (`LoadedAreaState.Settings.Peaceful`), its queue kept.
2. For each group of `ReadyForCombatUnitGroups` (`SleepingUnitsController`'s list: a group with a member not
   sleeping, not of a peaceful faction, and not in fog of war or in combat or with `AwakeTimer > 0`), in order:
   `ExtractNearUnits` (the union of the members' `CanBeInVisionRange` into a `HashSet`, cleared first: enumeration =
   first-insertion order), then for each near unit not of a peaceful faction and not `Passive`: a unit of the group
   itself is added (`LastDetectTime = GameTime`, `Visible` untouched); another goes through `UpdateVisibility`. Then
   the group's `Cleanup()`.
3. `m_UnitsForAdd` (pairs queued by `AddToMemory`): unless either unit is disposed or the target is in stealth for
   the observer's group, the target is added to the observer's group (the real group by `GroupId` when the observer's
   is fake) with `LastDetectTime = GameTime`, and the observer is woken (`AwakeTimer = Max(0, AwakeTimer)`). The queue
   is cleared.

`UpdateVisibility(group, target)`:
- the target in stealth for the group (`IsInStealth && !Stealth.IsSpottedBy(group)`): removed, done;
- a target that isn't the group's enemy, when `GameDeltaTime < 0.5`: looked at only on one frame in five,
  `(uint)(target.GetHashCode() >>> 2) % 5u == Time.frameCount % 5`. `UnitEntityData` doesn't override `GetHashCode`,
  so it's the object's identity hash: WrathOracle records it (`hash`, on first use; a replay uses the record's
  tape) and pins `Time.frameCount` in a replay (`WallClock.Frame`: its own frame plus `(record's first frame - its
  first frame) mod 60`);
- `group.HasLOS(target)` (a member in the game, not helpless, with `HasLOS(target)`): added, `Visible = true`; an
  invisible target (`UnitCondition.Invisible`) is visible only if a member sees invisibility and has line of sight,
  or its blindsense reaches; when visible, `LastDetectTime = GameTime`;
- else an existing entry's `Visible = false` (its detect time kept: forgotten 9 s later unless seen again).

Handlers (queue through `AddToMemory(observer, target)`: skipped when the target's faction is peaceful or the pair
is queued): `HandleUnitCommandDidAct` (a players' enemy's command on a unit of the player's faction: each remembers
the other, the executor only a target that isn't invisible); `HandleUnitMakeOffensiveAction` (raised by
`OffensiveActionsController`; the target's `UnitAggroFilter.OnAggroAction` first; with both in the game, the unit
remembers the target unless it's invisible to it, `LastSurpriseActionTime` when the unit isn't in combat, the target
remembers the unit, and a neutral target adds the unit's faction to its `AttackFactions`);
`HandleUnitEnterAreaEffect` (an aggroing effect's caster). Outside the controller: `StartCombat` (an action),
random encounters and dungeons queue; `UnitGroupsController.HandleUnitDestroyed` removes the unit from every group's
memory; `UnitGroup.Dispose` and `CommandEndCombat` clear; `RandomEncounterUnitSelector` adds directly.

### Vision: `CanBeInVisionRange`, `HasLOS`, line of sight

- `UnitVisionRangeController` (after `UnitMoveController` and `EntityBoundsController`): each awake unit that
  isn't an extra rebuilds `CanBeInVisionRange` (a `HashSet`, cleared) from a PhysX `OverlapCapsule` (from
  `pos - 2r·up` to `pos + 2r·up`, radius `r = VisionRangeMeters`, layer mask `1 << 23`) in the private
  `EntityBounds` physics scene, where `EntityBoundsController` keeps one sphere collider per entity (a unit's radius
  `Max(0.05, Corpulence · Max(|scale.x|, |scale.z|))`, at its `Position`). `VisionRangeMeters`: 11.7 for the
  player's faction without overrides, else the override or 8.5, and in combat `Max(11.7, that)` unless a combat
  override. The overlap's order is PhysX's.
- `UnitEntityData.HasLOS(other)`: `(Blueprint.IsFake || CanBeInVisionRange.Contains(other)) &&
  SightCache.GetLOS(other)`.
- `UnitSightCache` (per unit, by the other's unique id): a hit when the other is within 0.5 m (2D) of the position
  it was cached at; a miss computes `CheckLOS(other.Position + EyeShift, m_LastSamplePosition)`: within vision range
  (2D, squared) and no obstacle (`LineOfSightGeometry.HasObstacle(sample + EyeShift, point)`), or inside the unit's
  `ExtendedVisionArea`. `UnitLineOfSightCacheController` (before `UnitFollowUnitController`) clears a unit's cache
  when it moved 0.5 m from its sample, the geometry changed (`LineOfSightGeometry.LastUpdateTime`, the Unity clock at
  `Init`/`UpdateBlocker`) or its vision radius changed. `EyeShift` is `Vector3.up`.
- `LineOfSightGeometry` (`Owlcat.Runtime.Visual`): managed 2D geometry, the `FogOfWarBlocker`s' polylines
  (points, closed, `HeightMinMax.y`) bucketed in 3.9 m cells over the area's bounds; `HasObstacle` walks the cells
  the segment crosses and intersects segments (`VectorMath.SegmentsIntersect2D`), ignoring blockers lower than the
  eye. No PhysX: **portable**, given the blockers imported from the scene.
- The AI's `los` (`TargetInfo.InLos`) is `LineOfSightGeometry.HasObstacle(EyePosition, Position)` directly.

### Fog of war, sleeping, visibility for the player

`FogOfWarController` (after vision) sets each entity's `IsInFogOfWar`: revealed when inside a revealer's radius
(the party and pets, 11.7 m scaled by the campaign; in combat the party memory's enemies, 1 m; additional
revealers), found by a PhysX `OverlapCapsule` in the same physics scene, not too far (the entity's position within
the radius, else its **renderers' bounds**: presentation) and with no obstacle on the line of sight (with
`ShadowCullingHeightOffset`). Readers: `SleepingUnitsController` (`ShouldBeSleeping`: in fog of war with no
commands; the ready groups), `UnitCombatJoinController.ShouldEngageEnemy`, `IsVisibleForPlayer` (the turn order's
`ChooseNextUnit`, the UI), `IsUnseen`.

### Stealth and perception

`InStealthFor(group)` is false unless the unit `IsInStealth` (`UnitState`, saved: false for every unit of
Manual_18's state dump), set by `RuleEnterStealth` and cleared by `UnitStealthController`, `ForceExitStealth` and
`RemoveAmbush`. The stealth controller's spotting (perception checks, their draws) and `PartyPerceptionController`
(map objects) don't touch the memory unless someone is in stealth: no unit is in these fights (all 1,870 `stealth`
reads are false).

## Who reads the memory, and what the sim takes today [C][M]

Counts are `poc5-tb-103`'s record runs, `tb-nearest` / `tb-focus` (1,702 / 2,166 ticks).

| reader | read | today | count |
|---|---|---|---|
| engagement `TickUnit` | `Memory.Enemies` | `enemies` leaf (input) | 8,128 / 11,716 |
| AI `CalculateEnemies` | `Memory.Enemies` | `enemies` leaf | 69 / 80 |
| `UnitAttack.UpdateTarget` | `Executor.Group.Memory.Enemies` | `enemies` leaf | 2 / 2 |
| AI `CalculateEnemies` | `HasPlayerCharacterInMemory` | `hpcm` leaf | 0 / 0 |
| engagement `AttackOfOpportunity` | `target.Memory.Contains(unit)` | `memc` leaf | 0 / 0 |
| `RuleCheckTargetFlatFooted` | `Target.Memory.Contains(Initiator)` | `env` field | 51 / 72 |
| impatience tick, `TryAddBotherUnit` | `ContainsVisible` | `ivis` leaf, `imp_s` `vis` | 0+7 / 0+9 |
| leave controller | `Contains`, `UnitsList` | `leave_tick` rows (memory as changes) | 101 / 102 rows |
| join controller | `Enemies`, `Contains`, fog of war | `join_combat` records | 7 / 7 |
| engagement `IsAttackOfOpportunityReach` | `HasLOS` | `los` leaf | 1,178 / 4,674 |
| commands (target hidden) | `InStealthFor` | `stealth` leaf | 897 / 973 |
| turn order `ChooseNextUnit` | `IsVisibleForPlayer` | `choose_next` `vis` | 8 / 9 records |
| AI tick, sleeping | `ReadyForCombatUnitGroups`, awake units | `groups` leaf, `awake` | 269+8 / 272+8 |
| agent `ConnectToGroups` | `Group?.Memory != null` | `ag_v gmem` | 15 / 13 (all true) |

Visibility records the recordings already carry (observation only, not read by the replay): `vis` (each awake
unit's `CanBeInVisionRange` changes, ids sorted: the set's order is lost), `los` (each sight-cache miss: observer,
target eye point, sample position, result; not the target's id), `los_clear` (cache clears), `mem_add` (a new key,
with its caller chain), `mem_remove` (`Remove`, or `Cleanup`'s removals, sorted), `mem_clear`, `group_state` (the
window's first memory: keys only), `hash` (identity hashes), `u` rows (`fog=`, positions in short format).

In a tick the records come in the controllers' order [M]: `turn_tick`, `los_clear` (sight caches), `vis` (vision
range), `los` (the memory tick's misses), `mem_add`/`mem_remove` (the memory tick), then `cooldowns` (combat
prepare's prefix).

## What the sim can compute, and from what

1. **The bookkeeping** (done, `combat/memory.rs`): the dictionary, both caches, `Cleanup`, the controller's tick
   and `UpdateVisibility` as decisions over their reads, the queue and the handlers' memory part. Pure, generic over
   the unit and group keys; its reads come through `MemoryWorld`. Failure paths as the game's: an exception in
   `UpdateVisibility`'s invisibility check leaves the target added and visible; one in the queue's loop keeps the
   whole queue (it's cleared after the loop).
2. **From what the sim has**: the groups and their faction caches (`groups.rs`: `IsEnemy`), the same-group test,
   `IsInGame`, `IsDisposed`, peaceful factions (content), `InStealthFor` (`IsInStealth` from the save, false; a
   `RuleEnterStealth` reported), `GameTime`/`GameDeltaTime` (the time model), the identity hash (`hash` records:
   the recording's tape), `Time.frameCount` (the `tick` record's frame, a replay's shifted as WrathOracle does),
   `Invisible` (conditions), the handlers' triggers (the commands model's act; the offensive actions the impatience
   slice already raises), `Wake` (the awake timers). `Passive` is an AI leaf today (input).
3. **Needs geometry, so stays an input until ported**: `HasLOS` (line-of-sight geometry: portable once the
   `FogOfWarBlocker`s are imported; the sight cache needs exact positions: the movement agent), `CanBeInVisionRange`
   (a capsule-sphere overlap: membership computable with a margin, order PhysX's; the order is observable whenever
   a tick both removes and adds keys in one group's memory, e.g. a stealthed unit removed before a new one takes
   its slot, so it stays a recorded input), `IsInFogOfWar` (revealers
   and LOS are portable; an entity beyond the radius needs its renderers' bounds: input), the ready groups (fog of
   war), `ExtendedVisionArea` (script zones: portable), see-invisibility and blindsense (reported).

## Result so far: the bookkeeping against `poc5-tb-103` [M]

`combat::memory::tests::recorded_memory_changes_reproduce_every_memory_read` (ignored: needs the captures and the
game's blueprints) drives each group's memory from the recording's own changes (`group_state`'s keys, each
`mem_add`, `mem_remove`, `Cleanup`'s removals applied as `Cleanup` makes them, in enumeration order, `mem_clear`) and
checks, on all six runs (record and two replays of both fights):

- every `leave_tick` row's dictionary (609 rows: keys in enumeration order) and every group without a row unchanged;
- every valid cached `UnitsList` in a row (42) against the dictionary's units in the game;
- **every `Memory.Enemies` read (59,991: engagement 59,532, AI 447, `UnitAttack` 12)** against the dictionary
  filtered as `Enemies` fills its cache, `IsEnemy` computed from the groups' recorded faction caches and the
  factions' `AlwaysEnemy` (blueprints), the reader's group from the frame it's read in (`eng_tu`, the AI's unit,
  `uc_tick`);
- every flat-footed check's `target_remembers_initiator` (369) against `Contains`;
- not the visibility reads (`ContainsVisible`), since `Visible` isn't recorded;
- every `mem_add` of a unit absent, every `mem_clear`'s count, and each `Cleanup` removal explained by what the
  recording shows: all 45 are units that left their group a tick before and weren't put back (`group_remove` of a
  member; a unit still in the game would have been put back by the next `UnitGroupsController.Tick`, which runs
  before `Cleanup`). The recording doesn't say why a unit left its group (`OnIsInGameChanged`, a disposal, the
  `GroupId` setter): the check takes such a unit as out of the game, and counts the reads whose result depends on
  that (none in these fights);
- every addition of a unit that isn't the group's enemy (6: the archer 5198's group adding 558A at `t=83` /
  `t=85`) on its frame: `(hash >>> 2) % 5 == frame % 5` (4 and 2), replays with WrathOracle's frame shift.

All equal, and each run must check every kind above at least once (engagement, AI and `UnitAttack` reads,
flat-footed reads, rows, cached lists, additions, a friend's frame compared, a removal). `damaged_memory_records_are_caught`
damages `-103/tb-focus` 14 ways (an addition missing or doubled, an `Enemies` read reordered, a unit forgotten
without leaving its group, a flat-footed read flipped, a clear of another size, a row reordered, a friend's hash on
another frame, every flat-footed read, time record but the first, AI entry, `Cleanup` removal or clear gone): each
is caught. The unit tests port the C#'s branches
(the free slot reused, caches invalidated by a known unit's `Add` but not by `Cleanup`, the 9 s boundary, `TimeSpan`
overflow, the null reference, the queue's dedup and peaceful targets, both handlers, the tick's friend frame, a
long frame, stealth, invisibility, the fake observer group).

What this doesn't establish: the order of the window's first dictionary (`group_state` has keys only, not freed
slots: no addition in these fights lands in a freed slot that isn't the last), `LastDetectTime` and `Visible` (not
recorded: no 9 s expiry happens in these fights, every removal is a unit leaving the game), the caches' staleness
(each read equals a fresh fill: the recordings can't tell a cache hit from a refill), and the tick's own reads.

## Slice H: the memory computed (design)

### H1: the controller's tick, line of sight and vision range as inputs

The sim keeps every group's `GroupMemory` (on `groups::Group`, replacing `groups::Memory`) and runs:
- `UnitMemoryController.Tick` in a new phase `Phase::Memory`, between `Phase::Sleeping` and `Phase::Join`, every
  frame of Default mode, with the reads below pulled strictly as the next records;
- the handlers: the commands model's act (`HandleUnitCommandDidAct`), the offensive actions the impatience slice
  already raises from `RuleAttackRoll` (`HandleUnitMakeOffensiveAction`: the memory queue, `LastSurpriseActionTime`,
  a neutral target's `AttackFactions` (a faction change: ported or reported), `UnitAggroFilter` reported), area
  effects and `StartCombat` reported until reached;
- the other writers: `HandleUnitDestroyed`'s `Remove` (the replay already reports destruction), `UnitGroup.Dispose`
  (`groups::tick`), `CommandEndCombat` (reported).

Then become checks: `enemies`, `memc`, `hpcm`, `ivis`, `imp_s` `vis`, the flat-footed `target_remembers_initiator`,
the `leave_tick` memory rows (the replay stops adopting them), the agent's `gmem` (always true: `Memory` is
`readonly`, built in the constructor), and the commands' `stealth` (false while nobody `IsInStealth`). The caches are
the sim's (`Enemies` through `GroupMemory::enemies`, invalidated by the sim's own `Add`s every tick of a ready
group): the reads check them.

Inputs that remain in H1, each recorded where the controller reads it: the ready groups (fog of war; checked against
what the sim can prove, as the AI's `groups` are), each group's near units in their `HashSet` order (vision range),
`Passive`, `group.HasLOS(target)` (line of sight), see-invisibility (reported until reached).

### H2: line of sight

Import the `FogOfWarBlocker`s (points, `Closed`, `HeightMinMax`) and the area's bounds for `LineOfSightGeometry.Init`
into the scene cache; port `LineOfSightGeometry` (cells, `GetCellsForSegment`, `SegmentsIntersect2D`: Mono's float
arithmetic, `csharp/arith.rs`), `UnitSightCache`, `UnitLineOfSightCacheController` (`Phase::LosCache`, before
`Phase::Follow`) and `HasLOS`. Then `los` (each miss: checked), `los_clear`, the engagement's `los` leaves, the AI's
`los` and H1's `group.HasLOS` become the sim's. Needs exact positions (the movement agent, `docs/poc5/movement.md`)
and `LastUpdateTime` changes (blockers updated at run time, e.g. doors: an input, `los_geom`).

### H3: vision range

`CanBeInVisionRange` as the capsule-sphere overlap (membership computed; a pair within a small margin of the
boundary reported, PhysX's exact test isn't ported), in `Phase::Vision` (after `Phase::Move`). Its order stays an
input (`vis` in the set's order, or H1's `mnear`): order is observable through the dictionary's slots whenever
removals and additions interleave in a group's tick (and through the free list in later ticks), not only when two
keys are added, so the sim never orders the set itself.

### H4: fog of war

`IsInFogOfWar` from the revealers, distances and line of sight, the renderer-bounds branch an input; then the
sleeping units, the ready groups, the join controller's fog read and `IsVisibleForPlayer` follow. Last, since the
renderer bounds are presentation.

## Observer changes requested

None for the part implemented. For H1, a WrathOracle observer `memory` (requires `impatience`), following
`CommandObsImpatience.cs`'s conventions (fail closed, read fields, never side-effecting getters: `UnitInfo.Unit` and
`UnitReference.Value` resolve proxies, `UnitEntityData.Group` runs `UpdateGroup`, `Memory.Enemies`/`UnitsList` fill
caches):

- `mem_init`, one block right after `imp_init`: every group of `UnitGroupsController.Groups` (and every unit's fake
  group with a non-empty memory) with its memory's `m_Units` **slots** in array order (key, `LastDetectTime` ticks,
  `Visible`, or free with its free-list link; `freeList`, `freeCount`), both caches (valid flag and contents), and
  `UnitMemoryController.m_UnitsForAdd` in order.
- Entry `uc_mem` (`UnitMemoryController.Tick`, from `GameMode.Tick`) and `uc_mem_end`, with `Peaceful`.
- Leaves (`uc_v`, transpiler taps at the call, as the AI's): `mready` (`ReadyForCombatUnitGroups`: ids in order),
  `mnear` (after `ExtractNearUnits`: the set's enumeration order, with the group), `mpass` (`Passive` of a near
  unit), `mstl` (`InStealthFor`), `mlos` (`group.HasLOS`, with the group and target), `minv` (`HasCondition
  (Invisible)`), `mseen` (the invisibility loop's result), `mdisp` (`IsDisposed` in the queue), `mwake` (the
  observer's `AwakeTimer` before and after `Wake`).
- Checks: `mem_s` after each group's pass and after the queue (the group's slots, `LastDetectTime`, `Visible`, cache
  validity), `mem_q` for each `AddToMemory` (observer, target, whether queued, the caller chain), and `mem_add` /
  `mem_remove` / `mem_clear` as today but `Cleanup`'s removals **in removal order** (today sorted by id).
- `hash` in replays too (the tape's value, as read), so a replay's run carries its own.
- `in_game`: each `IsInGame` change of a unit (with the caller chain), and `group_remove`'s caller, so a unit leaving
  its group is known to have left the game rather than taken as such.

For H2: `los` with the target's id (`GetLOS`'s `other`, from a prefix) and `los_geom` (`LineOfSightGeometry.Init` and
`UpdateBlocker`: the blocker and `LastUpdateTime`). For H3: `vis` in the set's order (not sorted).

## Open questions

- Whether `UnitGroupMemory`'s initial layout ever has freed slots at a fight's start (only `mem_init` can tell);
  until it's recorded, the sim reports an addition into a dictionary whose layout isn't established.
- A fixture where a remembered enemy stays out of sight for 9 s (the expiry), one with a friend group seeing a unit
  late (`hash`), and one with stealth or invisibility, to exercise the branches these fights don't.
