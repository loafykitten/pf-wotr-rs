# POC-1: oracle certification (WrathOracle 0.2)

Written 2026-10-11. WrathOracle is the UMM/Harmony mod in `oracle/mods/WrathOracle` that turns
the real game (`Wrath-Mac/builds/arm64`) into the test suite. Tags: **[M]** measured in the
game on this Mac, **[C]** read in the decompiled code, **[I]** inferred. Captures are local and
gitignored (`fixtures/captures/oracle/`); this page has no game text, only type names, GUIDs
and counts. Companion files: [`../oracle/state-dump-format.md`](../oracle/state-dump-format.md)
(the state dump contract) and [`rng-sites.md`](rng-sites.md) (every RNG call site).

## Headline

| POC-1 criterion | Result |
|---|---|
| Fixtures record completely | **Yes [M]**: 3 fixtures covering all 7 required cases; 0 failed or dropped records, 0 structural errors and 0 hook exceptions; 0 unrecorded RNG steps in two fixtures and 1 (in presentation code) in the third |
| Each fixture replays its own capture consistently | **Yes, at the T0 level, in five separate runs [M]** (cert-5 to cert-7, cert-9, and cert-10 under the stricter round-3 rules: only audited T1 sites may differ, durations kept): `dice` replays bit-identically. `dialog-checks` and `attack` replay every T0 record (rules, handlers, mutations, rule draws, dialogue transitions, inputs) in the same order with the same values; depending on the run, either tick-exactly or shifted by a few ticks. The differences are confined to timing-coupled (T1) consumers: cutscene delays, movement, roaming, approach jitter. Details and causes below |
| Injected faults are detected | **Yes [M]**: a perturbed draw changes the trace (and, for the dialogue check, the branch taken and the final narrative state); an edited trace value is flagged; a hook that throws, a hook that silently stops recording, and an untranspiled call site are each caught by the health counters |
| State dumps (for POC-3 to POC-5) | Done [M]: Manual_13, Manual_10, and Ember's two fixture saves Manual_18_WR_FirstBattle and Manual_19_WR_PreHorgus (format v1, below) |
| Live blueprints vs `wr-content` (for POC-2) | **7,102 of 7,121 identical [M]** (slice closure plus a stratified corpus covering all 334 root types); the 19 differences are fields the game writes into blueprints at run time, and right after loading only 15 remain, all one such field. Same result after loading Manual_18 (7,072/7,087) and in Neathholm (Manual_19, 7,386/7,401) |

Found along the way [M]: the global RNG that rolls the dice is also drawn by rendering
(post-process dithering and film grain, every frame), particles, sound barks and UI, so in
the original the dice depend on the frame rate. Showing a dialogue cue with skill-check
answers rolls a hidden d20 per answer for the UI (6 at the cue in `dialog-checks`).
This happens because the answer list's `BlueprintAnswer.SkillChecksDC` calls
`RuleStatCheck.Calculate()`, which triggers a real `RuleRollD20` [C], so what the UI shows
moves the dice stream. `TimeController` reads wall-clock time on the first tick after any
mode change. Background
cutscenes and movement draw at times that drift between two otherwise identical runs.

## How it works

Everything below is installed only when the plan says `"fidelity": true`, at mod load (3.8 s
on this Mac [M]); performance runs carry none of it. WrathOracle 0.1 behaviour (plan
consumption, the save guard, dumps, routes, the timed save) is unchanged.

### Rules (`Rules.cs`)

Observation points, in the game's own order (`Rulebook.TriggerEvent` [C]):

| Point | How | Record |
|---|---|---|
| `RulebookEventContext.PushEvent` | Harmony postfix | `rule`: id, parent (the rule executing when it was pushed), `context_parent` (the previous rule in the game's own context, which differs when `TriggerEventInRestoredContext` swapped in a saved context), depth, class, initiator, target (if `GetRuleTarget` passes the purity check), `reason_at_push` (usually null), **full field snapshot** |
| mod-facing `IBeforeRulebookEventTriggerHandler<T>` | every concrete implementation patched | `h`/`h_end`, phase `mod_before` |
| `IRulebookEventAboutToTriggerHook` | our subscriber via `EventBus.Subscribe` | `about`: the reason as `Rulebook.AssignSource` set it (fact, ability, item, caster, context blueprint, parent rule), field changes since the last record (inputs after mods) |
| AboutTo subscribers (global, target, initiator lists, in list order) | every concrete `IRulebookHandler<T>.OnEventAboutToTrigger` patched (1,290 methods) | `h` (identity: class, component name, owner blueprint, fact, owner unit from the runtime in context) then `h_end` (the rule's field changes across the handler); `h_exc` if it throws |
| `OnTrigger` of each rule class (56) | prefix/postfix/finalizer | `trig`, `trig_end` (the rule's own computation; nested rules appear between), `trig_exc` |
| `IAfterRulebookEventTriggerHandler<T>`, DidTrigger subscribers | as above | `h`/`h_end`, phases `mod_after`, `did` |
| `IRulebookEventDidTriggerHook` | our subscriber | `did` |
| `RulebookEvent.OnDidTrigger` | postfix | `rule_end`: changes after temporary modifiers were removed, the rule's bonuses (`AllBonuses`), and HP changes of initiator and target across the rule |
| `PopEvent` | postfix | closes the rule (structure check) |

Field snapshots are every instance field of the rule (base classes first, auto-property
backing fields under the property name) plus every getter that passes a **conservative IL
check** (`Purity`: only field reads, arithmetic, constructors of a few plain value types and
calls to methods that pass the same check; no stores, `initobj`, allocation, interface or
virtual calls, `ref` parameters; unknown means rejected). It is a heuristic, not a proof. So computed results
such as `RuleSkillCheck.RollResult` are recorded without risking a side effect; impure ones
(e.g. `Dice.D20`, which triggers a rule) never run. Nested rules, units, facts, items,
contexts and abilities appear as references.

**Mutations** (`Hooks.cs`, record `mut`, each with the current rule and handler): stat modifiers
added (`ModifiableValue.AddModifier`, base method only, so an override calling it reports once
and a disabled attribute's early return reports nothing) and removed, `TemporaryModifiersScope`
add/remove, stat value changes (outermost `UpdateValue` per stat), rule bonuses
(`RulebookEvent.AddModifier`), rule temporary modifiers (`AddTemporaryModifier`), facts
attached and detached (buffs included, with end time), damage (at rule boundaries, attributed
to the innermost rule; the `Damage` setters are small enough to be inlined into callers, so
they are not hooked [I: the setter hook never fired]), flags, etudes (start, complete,
unstart), quest objectives, XP, alignment shifts.

34 handler methods can't be patched: they are declared in generic classes (all
`TutorialTriggerRulebookEvent<T>`, UI tutorials) [M]. Listed in each run's `rng-sites.json`.

### Random draws (`Rng.cs`)

1. **Scan [M].** At install the mod scans the IL of every method in the 89 game assemblies of
   `Managed/` (162,785 methods, 1.0 s) for calls to `UnityEngine.Random` and finds **280 call
   sites** in 175 methods. Each gets a stable key (`assembly|type|method(signature)|#ordinal@IL
   offset|member`) and an id (its rank in the sorted key list). A static enumeration by
   gpt-6.1-sol over the decompiled source found 272 (the runtime scan adds DOTween,
   DungeonArchitect and Rewired, which weren't decompiled).
2. **Transpile [M].** 266 sites in 161 methods are rewritten to `ldc.i4 id; call Rng.W_<member>`;
   the wrapper calls the same member with the same arguments (same consumption) and records
   site, member, arguments, value, generator state before, steps consumed, tick, rule and
   handler.
3. **Global net.** The 14 sites in generic methods can't be transpiled safely; the managed
   members (`Range(int,int)`, the vector and rotation getters) get a prefix that records any
   draw not coming from a wrapper and names the caller from the stack (looking through the
   generic helpers `LinqExtensions.Random` and `RandomUtils`). Five generic sites call
   extern-only members (`value`, `Range(float,float)`) and can't be intercepted at all
   ([`rng-sites.md`](rng-sites.md): "gap-detected only").
4. **Gap detection [M].** `UnityEngine.Random` is xorshift128; a self-test at install (state
   saved and restored) confirms one step per `Range`/`value` and the `Range(int)` mapping.
   Before every hooked draw and at every tick the oracle steps the generator forward from the
   last state it saw; any steps nobody recorded are counted (`rng_unhooked_steps`), including at
   the window's start and end and before game code reseeds or restores the generator. This is
   the coverage check: native code or a missed site shows up here, except draws whose state
   game code later restores (the tactical-grid generator saves and restores the state; not in
   these fixtures).
5. **Classes.** Sites in rendering, particles, critters, sound, camera shake, UI and visual
   plugins are **presentation** (`"c":"pres"`, prefix list in `Rng.cs`); everything else is
   **sim** (conservative; weather stays sim because `WeatherMechanicController` picks units for
   buffs and spells). Both are recorded; only sim draws form the replay tape, and
   trace comparison leaves presentation draws out (their number depends on how many frames
   were rendered).
6. **Replay.** With a tape, each sim site returns its own recorded values in order (a per-site
   queue: a site that drew a different number of times can't shift the others), after checking
   member and arguments, and **never calls the real generator** (strict), so any unhooked
   consumer sees a different stream than in the recording. A draw with no matching tape entry
   gets a fixed fallback (the range minimum, zero, identity), is marked `fallback`, and is
   counted (`replay_tape_exhausted`, `replay_mismatches`); out-of-order use and leftovers are
   counted too. Any of these makes the replay invalid for strict comparison.

Coverage summary (from [`rng-sites.md`](rng-sites.md)): of 280 sites, 266 transpiled, 9 global
hook only, 5 gap-detected only (all 14 untranspiled ones are in generic helpers); by category ai 21, anim-variant 28, attack-sequence 2, cutscene
3, dialogue 9, dice 2, procedural 12, rules-direct 27, spawn 28, visual 97, wrapper 22, other
small groups. `System.Random` and `PersistentRandom` (dungeons, random encounters) are
separate seeded generators, not hooked; none ran in the fixtures [I].

### Time (`Hooks.cs`, `OracleTime.cs`)

Per sim tick (`GameMode.Tick`, once per `Game.Tick` that ticks): `tick` (mode, frame, Unity
delta, unscaled delta, time scale, capture delta, RNG gap) and, from `TimeController.Tick`,
`time` (raw delta and whether it came from the just-activated branch, player and debug time
scales, the cap `PlayerTimeScale·DebugTimeScale/12`, the capped delta, game delta, game and
real time after, turn-based). `controllers` lists each game mode's controller order the first
time the mode ticks. In turn-based combat `TimeController` temporarily multiplies
`PlayerTimeScale` before computing the cap and restores it after; the `time` record carries the
effective scale, read in the `DeltaTime` setter at the moment the cap is applied.

**Finding [M, C]:** `TimeController.Tick` uses `Time.unscaledDeltaTime` (wall-clock) on the
first tick after the controller is activated (after a load, entering or leaving dialogue or
pause), and `TickRealTime` adds wall-clock time to `Player.RealTime`. A fixed
`Time.captureDeltaTime` doesn't cover these, so game time drifted by a wall-clock amount at
every mode change. In fixture mode (only between `prepare` or `fixture` and the end of the
window; cleared unconditionally when a plan ends or aborts) the oracle feeds the fixed delta
to those two read sites (a transpiler), which is a deliberate change of input: the game sees
what it would see if every frame took exactly 1/60 s. In replay it feeds the recorded values
(the time tape). The wall-clock value is still recorded (`tick.wall_delta`).

### Dialogue

`StartDialog` (request), `StartScheduledDialogImmediately` (started), `SelectAnswer` (answer,
its index, the cue; after: scheduled cue, acting unit), `PlayCue`, `PlayCheck` (check, stat,
authored DC; after: passed/failed), `StopDialog`, and the **completed transition**
`dlg_transition` after the `DialogController.Tick` that played a scheduled cue: dialogue, cue,
answers shown (GUIDs, in order), continue cue, acting unit, current speaker, skill-check
results (unit, stat, DC, d20, roll, stat value, passed), local and global history sizes, local
passed/failed checks, alignment.

### State dumps and blueprints

State dumps follow [`../oracle/state-dump-format.md`](../oracle/state-dump-format.md) (written
first, implemented, then reconciled with the emitter after review). Every dump records the RNG state before and after; all
dumps this round left it unchanged [M]. Blueprints: `blueprints:LABEL` writes every loaded
blueprint (`BlueprintsCache`) plus a GUID list in `docs/poc2/content.md`'s canonical form,
emitted by the same recursion as `ReflectionBasedSerializer.WriteField` over the game's own
`BlueprintFieldsTraverser` field list; Unity objects as `{"asset_index": i}` (their index in the
pack's referenced-asset list, as `wr-content` writes them from the `.bbp`).

### Health

Every trace's `end` record and every `run.json` carry: records written, failed (a record
whose building threw becomes `{"k":"failed"}` with the same `seq`), dropped (writer queue
full), **structural errors** (a rule missing any of `about`, `trig`, `trig_end`, `did`,
`rule_end` without an exception to explain it, a handler without its end, anything still open
at the end), **hook exceptions caught** (our hooks
catch their own exceptions and count them, because the game's hook manager, subscriber
lists and controllers swallow them), RNG unhooked steps (known and unknown), global-hook draws,
off-main-thread draws, replay mismatches and exhausted queues, snapshot truncations and
snapshot value failures. The trace header carries the install result: `installed` and
`install_failures` (any failure recorded while installing: a hook that didn't patch, a failed
transpiler, `Install` throwing). `seq` has no gaps, so a lost record is visible too. Dumps have
their own health block. `oracle-trace-diff --check` re-derives the structure from the file
(phase order per rule, handler pairing, and nesting: a rule's `parent` must be the innermost
open rule and its `depth` the number of open rules, a handler must belong to the innermost open
rule, and every close must close the innermost open frame) and rejects a capture with any
problem, including an incomplete install; comparisons require both captures to be valid.
`oracle-run.sh` prints and checks each fixture window's health from `run.json` (any nonzero
counter fails the window, including `snapshot_value_failures` and `install_incomplete`).

**Save safety.** As in 0.1, while a plan runs `SaveManager.SaveRoutine` only accepts the bench
slot; 0.2 also guards `SaveManager.DeleteSave`, which the game itself calls on an Ironman game
over: only files named `Manual_N_WR_*.zks` may be deleted. No fixture plan saves at all. Fixture
timing, tapes and traces are cleaned up when a plan ends, aborts or the runner is destroyed.

### Driving and fixtures

Plan steps (full list in `Driver.cs`): `move`, `teleport`, `dialog`, `answer:N`,
`answerbp:GUID`, `waitdialog`, `waitnodialog`, `attack`, `run:N` (N frames, resuming whenever
the game pauses itself), `wait:N`, `skillcheck`, `rolldice`, `d20reroll`, `tempmod`,
`savebonus`, `addfact`, `seed`; steps ending in `?` are optional. Each input is a trace record.

A fixture window (`tools/oracle-fixtures/make-plan` expands a spec):
`prepare` (fixed frame delta `Time.captureDeltaTime = 1/60`, `Random.InitState(seed)`),
`reload`, `wait:60` (the load is confirmed by frames, not wall time), `fixture:NAME` (state
dump `start-record`, trace on), the fixture's steps, `end` (state dump `end-record`, trace
closed). **Replay** is the same sequence with `replay:NAME`, in the same process: the
generator is reseeded differently, sim draws come from the recording's tape per site, time
from the time tape. Equal start states are checked, not assumed (`oracle-state-diff`).

Comparison (`tools/oracle-trace-diff`) reports three levels: **strict** (every record but
presentation draws, in order); **T0** (rules, handlers, mutations, draws made inside rules,
dialogue, inputs without positions, ticks and times: exact, in order); **T0, tick-tolerant**
(the same without tick numbers; absolute game times such as buff end times are compared
relative to the game time of the tick they were recorded in, so durations still have to match;
i.e. the T1 contract: same events, same order, timing may shift); plus, for sim draws outside
rules, per-site value sequences and the largest tick shift. Exit status: 0 for strict
equality, or tick-tolerant T0 equality with identical per-site sequences; with `--t0` (the
certification level), per-site differences are tolerated **only at audited T1 sites**
(`AUDITED_T1_SITES` in the tool: cutscene delays and cutscene animation, animation start and
the `Kingmaker.Visual.Animation` namespace, roaming, command approach, movement slow-down).
Every other sim draw outside a rule, AI choices included, must replay the same values per
site. `tools/oracle-trace-diff-selftest RECORD REPLAY` checks this by mutation (below).

## Formats

Trace: `fixtures/<name>/{record,replay-N}.trace.jsonl.gz`, schema `wrath-oracle-trace-v1`, one
JSON object per line, every record `{"seq", "k", "t"}` (`t` = sim tick in the window).

| `k` | Fields (besides seq, k, t) |
|---|---|
| `begin` | name, mode, fixed_delta, seed, random_state, save and hash, versions, steps, faults, install stats, `rng_sites` (`[id, key, class]`) |
| `tick`, `time`, `controllers` | as in "Time" |
| `rule`, `about`, `trig`, `trig_end`, `did`, `rule_end`, `trig_exc` | id; `rule` adds parent, depth, class, handler, initiator, target, reason, fields; the others `changes: [[field, value], …]`; `rule_end` adds bonuses |
| `h`, `h_end`, `h_exc` | handler id, rule, phase, class, component, blueprint, fact, owner / changes / exception |
| `draw` | site (id, or -1 with `key` for global-hook sites), c (sim/pres), m (member), a (arguments), v (value), rule, h, gap, steps, st (state before), forced/match in replays |
| `rng_state` | InitState / state set / seed set by game code |
| `mut` | what, rule, h, and per kind: stat, owner, mod, old/new, fact, flag, etude, objective, unit… |
| `dlg_*` | as in "Dialogue" |
| `input` | step and its resolved targets |
| `end` | totals, per-site draw counts, tape use (replay), health |

Values follow the state dump's conventions (floats round-trip, enums as integers, GUIDs as 32
hex digits, references as `$unit`/`$fact`/`$item`/`$rule`). Other files per run: `run.json`,
`identity.json`, `rng-sites.json` (the full site table with draw counts), state dumps,
`blueprints-LABEL.jsonl.gz`.

## Certification fixtures (Manual_13) [M]

Reference run `cert-7` (one launch: each fixture recorded, replayed once, and two of them
replayed again with a perturbed draw; `fixtures/captures/oracle/cert-7/`, 51 MB). The same
plan also ran as `cert-5` (before the review fixes) and `cert-6` (after them, without the
reseed after the load); their results are in the variability table below.

| Fixture | Inputs | Covers |
|---|---|---|
| `dialog-checks` | walk into the Seelah/Anevia trigger zone, three continues, the Athletics answer, continues | **nested rule** (`RulePartyStatCheck` → `RuleSkillCheck` → `RuleRollD20`), **deferred dialogue transition** (9 completed transitions), the check, XP, objective, etude, 6 UI preview d20s |
| `dice` | 3d6, a d20 with a take-best reroll, two forced checks (`EnsureSuccess` true and false), a plain check, a check carrying a temporary +4, a Fortitude save with a +2 rule bonus | **multi-die roll**, **reroll**, **forced roll** (the direct `Random.Range` in `RuleSkillCheck.OnTrigger` and `RuleRollD20.FromInt`), **temporary modifier** (`AddTemporaryModifier`, removed in `OnDidTrigger`, and the saving throw's `TemporaryModifiersScope`) |
| `attack` | teleport next to the FirstBattle centipedes, attack one, keep the game running 15 s (resuming auto-pause) | **animation-variant draws** (`AttackHandInfo`), attack sequence (`UnitAttack.TryToReorder`), weapon attacks with AC, concealment, flat-footedness, damage, initiative, enemy AI, deaths and XP; rules nested 4 deep |

`cert-7` recordings:

| Fixture | Ticks | Records | Rules (handler calls) | Sim draws (in rules) / presentation | Notable | Health |
|---|---|---|---|---|---|---|
| `dice` | 57 | 534 | 12 (22) | 10 (10) / 273 | 1 multi-die, 1 reroll, 2 forced, 1 `rule_temp_mod`, 1 scope add/remove | clean |
| `dialog-checks` | 420 | 3,390 | 9 (4) | 37 (7) / 2,391 | 9 transitions, check failed (d20 9) | clean |
| `attack` | 962 | 11,251 | 164 (626) | 909 (26) / 6,092 | 9 attacks, 5 HP changes, 23 `AttackHandInfo`, 40 `TryToReorder` draws, 3 deaths | 1 unhooked RNG step |

"Clean": 0 failed, 0 dropped, 0 structural errors, 0 hook exceptions, 0 unhooked RNG steps.
The unhooked step in `attack` falls between two sound-bark draws: most likely the bark choice
through `LinqExtensions.WeightedRandom` or `RandomUtils.GetRandomValue`, generic helpers that
call `Random.Range(float, float)`, which can't be intercepted [I]. Every rule's `about` record
carries its reason (all 185).

Replays (`oracle-trace-diff record replay`, `oracle-state-diff` on the start and end dumps):

| Run | `dice` | `dialog-checks` | `attack` |
|---|---|---|---|
| `cert-5` | **strict equal**; start and end state equal (those dumps didn't yet include the units' entity-level members) | T0 equal (tick-exact) | T0 equal in order and values, ticks shifted |
| `cert-6` | **strict equal** | T0 equal in order and values, ticks shifted | T0 equal (tick-exact) |
| `cert-7` | **strict equal** | T0 equal (tick-exact) | T0 equal in order and values, ticks shifted |
| Always | start and end states differ only in the roaming monitor lizard's next wander point (since `cert-6`, which dumps it) | per-site T1 value sequences equal except 1–3 sites drawing once more or less; start states differ only in the lizard's next wander point | the same, plus a few hundredths of a second of buff and death times |

So **every fixture replays its own capture at the T0 level in every run**: the same rules,
handlers, mutations, rule draws, dialogue transitions and inputs, in the same order, with the
same values. The `dice` fixture replays bit-for-bit. In the other two, timing-coupled (T1)
systems vary between runs:

- **What differs.** Sim draws made outside rules come from cutscene delays (`CommandDelay`,
  the save's background cutscenes), movement (`UnitMovementAgent.SlowDown`), command approach
  (`UnitCommand.TickApproaching`), roaming (`UnitRoamingController`, the lizard) and animation
  (`UnitAnimationController` start offsets). Their values replay per site, but some happen at
  other ticks (up to 70) and 1–3 sites draw once more or less (e.g. 787 vs 788 approach draws).
  The lizard's next wander point is already rolled during the load, before the window, so it
  differs from the start. Walk end points differ by 2–8 cm.
- **Why** [I, not confirmed]: these systems wait on something not tied to the sim tick.
  Candidates are audio playback, which runs on wall-clock time, for the cutscenes, and
  asynchronous pathfinding and view/asset loading for movement and the start of combat. When
  the shift reaches the dialogue's start or combat's start (initiative), the whole T0 stream
  moves by a few ticks, which also shifts absolute game times stored in records (buff end
  times, death time). The diff's tick-tolerant level leaves exactly those out.
- **Replay health.** Under the strict tape, the T1 sites that draw once more get a fallback
  value and those that draw once less leave tape behind. The comparison classifies these as T1
  (none at a site that ever drew inside a rule), so the replays count as valid at the T0 level.
  A tick-exact replay of these two fixtures would need the asynchronous sources pinned (views
  and paths prepared before the window, pathfinding run synchronously). That is future work.

## Mutation tests [M]

| Injected | Detected by |
|---|---|
| Replay with the dialogue check's d20 (the 7th d20 of the tape) changed by +8 (`replay:dialog-checks:d20#7:+8`) | trace diff: T0 differs at that draw (9 → 17); the check passes instead of failing, so the dialogue takes the success branch; end-state diff: 1,389 differences (shown cues, selected answers, …) |
| Replay with the first d20 of `dice` changed by +5 (`d20#1:+5`) | trace diff: T0 differs at that draw (2 → 7) |
| One recorded rule result edited in a copy of the `dice` trace (`Result` 10 → 11) | trace diff: T0 differs at that record |
| Our about-hook throws at rule 3 (`faults.throwAboutAt`) | `records_failed` 1, `hook_exceptions_caught` 1, `structural_errors` (rule 3 missing `about`) |
| Our about-hook silently records nothing at rule 2 (`faults.dropAboutAt`) | `structural_errors` (rule 2 missing `about`), also found again by `--check` from the file |
| The per-frame dithering call site left untranspiled (`faults.disableSite`) | `rng_unhooked_steps` 66 (two per frame over 33 ticks) |
| The dice site left untranspiled (first fault run, `faults-1`) | not a gap: the global net recorded every die anyway (defence in depth) |

Run `faults-3` holds the last three together; `run.json`'s `fixture_health` and
`oracle-run.sh` report the window as failed, and `tools/oracle-trace-diff --check` rejects the
trace with all five problems.

## State dumps for POC-3, POC-4 and POC-5 [M]

| Save | File | Size | Units | Notes |
|---|---|---|---|---|
| Manual_13 | `fixtures/captures/oracle/state-manual13-after-load.json` | 2.7 MB | 21 | area `bad465e5…` (Prologue_Caves_1), party of 1 |
| Manual_10 | `fixtures/captures/oracle/state-manual10-after-load.json.gz` | 38.1 MB gzipped | 539 | area `25700157…` (asset name `DrezenCapital`), `Player.Party` has 1 member, 16 characters in `AllCharacters`; 116 values truncated at depth 8, 243 cycles |
| Manual_18_WR_FirstBattle (Ember, 2026-10-11) | `fixtures/captures/oracle/manual18-dump-1/state-manual18-after-load.json` | 2.9 MB | 21 | area `bad465e5…` (Prologue_Caves_1), party of 3, just before FirstBattle |
| Manual_19_WR_PreHorgus (Ember, 2026-10-11) | `fixtures/captures/oracle/manual19-dump-2/state-manual19-after-load.json` | 6.9 MB | 46 | area `61fcf2a3…` (Prologue_Neathholm), party of 5, before Horgus; 227 cycles |

All: plan without fidelity instrumentation, dumped one second after the load completed
(`OnAreaLoadingComplete`), RNG state unchanged by the dump, 0 failed records, build identity
in the file (`identity`). Runs: `manual13-dump-3/`, `manual10-dump-3/`, `manual18-dump-1/`,
`manual19-dump-2/` (a plan on a save outside the caves needs `"area": "any"`; `manual19-dump-1`
aborted on that check). Both of Ember's new saves are backed up read-only in
`fixtures/saves/`.

## Blueprints vs `wr-content` [M]

`tools/wr-oracle-canon` (new, in Wrath-Rust; reads only) decodes each GUID's record from
`blueprints-pack.bbp` with `wr-content`, writes `wr-content`'s canonical form, and compares
it with the game's dump field by field. Numbers compare by the field's declared type from
`wr-content`'s schema: `f32` by `f32` bits (the two sides format floats differently), `f64` and
integers exactly; numbers without a declared type exactly (none occurred).

| Dump | Compared | Identical | Differences |
|---|---|---|---|
| After the certification fixtures (`cert-7/blueprints-slice`): 5,117 loaded + 2,348 corpus | 7,121 (334 root types) | **7,102** | 15 `BlueprintEncyclopediaPage.m_ParentAsset`, 4 `Cutscene.Sleepless` |
| Right after loading Manual_13 (`manual13-dump-3`): 5,023 loaded + corpus | 7,035 | **7,020** | 15 `m_ParentAsset` |
| Right after loading Manual_18 (`manual18-dump-1`): 5,080 loaded + corpus | 7,087 | **7,072** | 15 `m_ParentAsset` |
| Right after loading Manual_19, Neathholm (`manual19-dump-2`): 5,397 loaded + corpus | 7,401 | **7,386** | 15 `m_ParentAsset` |

Both fields are written into the blueprint objects at run time [C]: `PlayCutscene` sets
`Cutscene.Sleepless = true` when it plays a cutscene, and encyclopedia pages get their
`ParentAsset` through a property setter when the encyclopedia tree is built. So the `.bbp`
reader matches what the game loads for every field of every compared blueprint; a comparison
must take dumps before such writes or exempt these two fields. The corpus
(`cache/oracle/blueprint-corpus.txt`) is two blueprints of every root type, the first
blueprint using each of the 2,151 nested types, and up to 20 using each rare encoding found by
pattern (curves, shared strings, assets, weak links, non-finite floats; no gradients matched
the pattern).

## Not covered

- **Tick-exact replay of timing-coupled systems** (above): cutscene timing, movement, combat
  start. The T0 stream holds; positions and ticks don't.
- **Draws the oracle can't intercept**: 5 call sites in generic helpers that call
  extern-only members (`LinqExtensions.WeightedRandom` in two assemblies,
  `RandomUtils.GetRandomValue` with weights twice, a weather spawn controller), and native
  code. Both show in the gap count (0 in two fixtures, 1 in the third). Callers of
  `WeightedRandom` that matter for rules would need those helpers' callers transpiled instead.
- **Separate generators**: `System.Random` instances and `PersistentRandom` aren't hooked.
- **Derived combat values in state dumps** (attack bonus, CMB, CMD per weapon): computing them
  runs rules, so dumps carry their inputs and traces carry the rules when the game runs them.
- **Mutations outside rules**: only the listed kinds are hooked (no inventory moves, money,
  spell slots, ability resources yet); damage only at rule boundaries.
- **Rule hooks through generic code**: 34 tutorial handlers, and any rule class with a generic
  `OnTrigger` (none found).
- **Scripted fixture inputs**: the multi-die roll, reroll, forced roll and temporary modifiers
  are triggered by plan steps through the game's own rule code, not reached through content.
  Content in reach of Manual_13 has none of them in the fixtures' paths (FennA is unarmed, so
  weapon damage is one die; no reroll or temporary-modifier feature fired).
- **Startup hangs** (3 of 16 launches this round): the game froze with its main thread
  waiting on a dispatch semaphore inside UnityPlayer at ~0% CPU, once at the first load and
  twice right after "Completed reload", before any mod had loaded, so not caused by
  WrathOracle. POC-0 saw the same signature once. Each was sampled, killed and relaunched;
  `launch.sh` reports "timed out waiting for mods to load" and `oracle-run.sh` removes the plan.

## How to record a new fixture

```sh
# from the repository root (the oracle moved here from Wrath-Mac on 2026-10-12)
# 1. a spec: oracle/tools/oracle-fixtures/certification.spec.json is the example
#    {"plan": {...}, "fixtures": [{"name": "...", "steps": [...], "replays": 1, "perturb": ["d20#K:+D"]}]}
oracle/tools/oracle-fixtures/make-plan my.spec.json > my.plan.json
# 2. run (Steam running, game closed; one run at a time). scripts/oracle-record.sh wraps
#    oracle-run.sh with the hang watchdog and retries: use it for anything unattended.
scripts/oracle-record.sh my-1 Manual_13 my.plan.json --deadline 1800
# 3. check and compare
oracle/tools/oracle-trace-diff --check .../my-1/fixtures/NAME/record.trace.jsonl.gz
oracle/tools/oracle-trace-diff .../NAME/record.trace.jsonl.gz .../NAME/replay-1.trace.jsonl.gz --json diff.json
oracle/tools/oracle-state-diff .../NAME/state-start-record.json .../NAME/state-start-replay-1.json
# state dump of any save (no instrumentation): see oracle/tools/oracle-fixtures/dump-manual10.json
# blueprints vs wr-content: gunzip the dump, then
cargo run --release -p wr-oracle-canon -- blueprints.jsonl --out report.json
```

To find answer GUIDs and positions, run once with a spec whose steps end at `waitdialog`
and read the `dlg_transition` and `input` records. The build must have WrathOracle installed
(`dotnet build -c Release oracle/mods/WrathOracle/WrathOracle.csproj`, game closed; it installs into
`builds/arm64-oracle/Mods`).

## Files

- Mod: `oracle/mods/WrathOracle/` 0.2.0: new `Trace.cs`, `Rules.cs`, `Rng.cs`,
  `OracleTime.cs`, `Hooks.cs`, `Snap.cs`, `StateDump.cs`, `BlueprintCanon.cs`, `Driver.cs`,
  `Fixture.cs`; changed `Main.cs`, `Plan.cs`, `Runner.cs`, `Info.json`, `WrathOracle.csproj`.
- Tools (then in Wrath-Mac, now `oracle/tools/`): `tools/oracle-run.sh` (`plan` preset), `tools/oracle-trace-diff`,
  `tools/oracle-state-diff`, `tools/oracle-rng-table`, `tools/oracle-fixtures/` (specs, plans,
  `make-plan`).
- Tool (Wrath-Rust): `tools/wr-oracle-canon`.
- Local data: `fixtures/captures/oracle/` (216 MB: `cert-5`, `cert-6`, `cert-7`,
  `faults-1` to `faults-3`, `manual13-dump-3`, `manual10-dump-3`, the two state dumps),
  `cache/oracle/` (40 MB: corpus list, the static RNG classification, comparison reports,
  unpacked blueprint dumps).

## Review

GPT 6 Astra (high) reviewed the mod, the tools and both documents against the decompiled
code and the captures. **Round 1: 19 findings**, each checked against the code before acting;
all were real and are fixed:

- **Save safety:** `SaveManager.DeleteSave` wasn't guarded (the game deletes the save on an
  Ironman game over); now only `Manual_N_WR_*` files may be deleted while a plan runs.
  Fixture timing, tapes and traces are now cleaned up on every plan end, abort or teardown,
  and the wall-clock substitution is gated on the oracle's own fixture mode.
- **Strict replay wasn't strict:** an exhausted or mismatching tape fell back to the real
  generator (2 draws in `cert-5`); now a fixed fallback, counted, and classified as T0 or T1 by
  the diff.
- **Verification accepted unhealthy captures:** `--check`, comparisons, the state diff and
  `oracle-run.sh` now all require valid captures (completeness, every health counter, tape use,
  self-test, structure re-derived from the file).
- **Classification:** weather draws were wrongly presentation (`WeatherMechanicController` picks
  units for buffs and spells); now sim.
- **Rules trace:** the reason was read before `AssignSource` set it (now in `about`); restored
  contexts lost their parent (now `context_parent`); `AddModifier`/`UpdateValue` mutations were
  reported twice through overrides; the structure check ignored `trig`/`trig_end` and handler
  pairing; the turn-based time record showed the configured, not the effective, scale.
- **RNG gaps:** blind spots at window boundaries and around reseeds closed; the coverage claim
  qualified (state restored by game code hides draws).
- **Dumps:** `GetMaxAmount` raised events (removed); entity-level and body persisted members were
  missing (added); dump counters overwrote the trace's; the purity check had loopholes
  (`initobj`, value-type constructors, `ref` parameters; tightened and now called a heuristic);
  three contract mismatches with the emitter (resolved, format note added).
- **Tools:** GUID renaming could hide changes to save-stable ids (now only ids missing from one
  side are renamed); the blueprint comparison rounded real doubles to `f32` (now typed by the
  schema; the result didn't change); transpiler bookkeeping sorted ordinals as strings and put
  exception-block ends on the wrong instruction (no current site was affected).

All results on this page come from runs made after these fixes (`cert-7`, `faults-3`,
`manual13-dump-3`, `manual10-dump-3`).

**Round 3** (2026-10-11, on Wrath-Mac `2457bd3` and the cert-9 captures): 9 findings, each
checked against the code; all real, all fixed:

- **P1, certification too lenient on draws outside rules.** `--t0` tolerated per-site
  differences at every sim site outside a rule, AI target selection and action choice included
  (`UnitImpatienceController`, `AiBrainController.SelectAction`). None of those actually
  differed in cert-9 (only approach, animation, roaming and cutscene-delay sites did), but the
  tool wouldn't have noticed. Now only the audited T1 sites may differ.
- **P1, tick-tolerant comparison dropped times.** Every absolute-time field was removed, so a buff
  lasting 60 s longer passed. Absolute game times are now compared relative to their tick's game
  time; durations are kept.
- End-of-window state dumps had no save identity, which weakened the id policy (now the window's
  save). Cut entity parts in rule snapshots were all `{"$part":"Spellbook"}` and the like (now
  owner unit, and blueprint for spellbooks). Snapshot value failures were missing from the
  fixture health in `run.json` (added), and a dump's `records_failed` subtracted value failures,
  which could cancel a real failure (now whole records only). An incomplete install
  (`installed: false`, a hook that didn't patch) still certified (now `install_failures` is in
  the header and fixture health, and `--check` rejects it). `--check` didn't validate nesting
  (now parent, depth, handler ownership and crossed closures). Empty-hand weapons, extra limbs and
  polymorph hand sets were only references in dumps (now full items).
- `tools/oracle-trace-diff-selftest` (new) mutates a certified replay and recording: an AI draw
  and a 60 s longer buff fail, a draw at an audited T1 site passes, and a bad parent and a crossed
  closure fail `--check`. On cert-9's `attack` all six cases behave as expected [M].
- Process note: the first cert-9 reading used a shell loop that reported every exit status as 0;
  the results above were rerun with the status captured correctly.

## Status (2026-10-11): certified

**POC-1 passes [M].** Reference run `cert-10` (WrathOracle at Wrath-Mac `40ff3e9`, after Astra's
round 3): all three recordings pass `--check` (clean install, 0 failed/dropped records, 0
structural or nesting errors, 0 snapshot value failures; `attack` has its 1 known unhooked step);
`dice` replays strictly equal; `dialog-checks` and `attack` are T0-equal in order and value
under `--t0` (tick shifts up to 68 and 40), every per-site difference at an audited T1 site;
both perturbed replays fail at T0, as they must. `oracle-trace-diff-selftest` passes all six
mutation cases on `attack`. End-state differences between record and replay are timing-only
(positions, orientation, move and round timers, death time; none in HP, facts, buffs or XP).
`cert-9` (before the round-3 fixes) gave the same T0 results and showed the depth-6 snapshot
fix working (0 snapshot value failures, against 1,332 in `cert-8`).

Open, none blocking: tick-exact replay of timing-coupled systems (pin views and paths before the
window), the five extern-only RNG sites, `System.Random`/`PersistentRandom`, mutations outside
rules beyond the listed kinds. New fixtures (POC-4 dialogue, POC-5 turn-based fights) are
certified the same way: record, replay, `--t0`, the self-test, and state diffs.

Captures: `fixtures/captures/oracle/` (`cert-5`, `cert-7` to `cert-10`, `faults-1`, `faults-4`,
`manual13-dump-3`, `manual10-dump-3`, `manual18-dump-1`, `manual19-dump-2`); reports in
`cache/oracle/`.
