# POC-5: the stat mutations outside the rules' spans (stream stats)

Written 2026-10-19 (twenty-third session, stream `stats`). Tags: **[M]** measured, **[C]** read in the decompiled C#
(`decompiled/`, never copied here), **[I]** inferred. No game text here: keys, GUIDs, type names and counts only.

`docs/poc5/tb-replay.md` ("Records inside root rule spans", the `mut` bullet) left one thing open: the recordings'
`mut` records of `what` = `stat_value`, `stat_mod_add` and `stat_mod_remove` (a stat's value change, a modifier added or
removed) lie outside every root rule's span, the T0 projection dropped them, the sim didn't write them, and nothing
else read them. A wrong stat change outside a rule passed unnoticed. They are now written by the sim's own port of
`ModifiableValue`, kept by the projection on both sides and compared in the T0 stream.

## Headline

| | Result |
|---|---|
| The sweep (25 captures, 116 runs) | `scripts/sweep.sh`, 116 runs: 113 `ok` (valid, T0 equal, end state equal), 3 NOT (`poc5-tb-122` tb-nearest, the known tie; its 36 stat mutations are equal too) [M] |
| Stat mutations compared [M] | 5,202 compared (2,616 `stat_value`, 2,580 `stat_mod_add`, 6 `stat_mod_remove`), 5,202 equal, 0 differing, 0 extra, 0 gaps; T0 equal on all 116 runs |
| Producers [M] | the helpless condition's penalties added (426 events) and cleared (3), a buff's modifiers (3: Enlarge Person); every recorded mutation has a producer the sim ports, so there is **no gap** (below) |
| Damage tests | 23 damaged recordings of `poc5-tb-120/tb-cast`, each reported (stream unequal, mutations differing, run invalid) |

## What WrathOracle records [C]

`oracle/mods/WrathOracle/Hooks.cs` patches `ModifiableValue` (all through `Hooks.Mut`, which adds `rule` and `h`, the open
rule and handler, and, in Pause mode only, `origin`):

| record | patch | fields |
|---|---|---|
| `stat_mod_add` | postfix of `AddModifier(Modifier)`, so **after** the add's `UpdateValue` (the value change and the dependents' first) | `stat`, `owner` (the owner's unit id), `mod` (`Snap.Modifier`: descriptor, its name, value, stack mode, `stacks`, source fact, source component, item), `modified` (the stat's value after) |
| `stat_mod_remove` | postfix of `RemoveModifier(Modifier)`, only when a modifier was removed | the same |
| `stat_value` | the **outermost** `UpdateValue` of a stat (a per-instance set, `Hooks.Updating`, from prefix to postfix, the dependents' updates inside it): reported at its postfix if `ModifiedValue` changed | `stat`, `owner`, `old`, `new`, `permanent` |
| `stat_temp_scope_add` / `_remove` | postfix of `AddTemporaryModifiers`, prefix of `RemoveTemporaryModifiers` (a rule's temporary modifiers on a stat) | `stat`, `owner`, `mods` (all of the scope's) |

`AddModifier(Modifier)` is the base method of the virtual: `ModifiableValueAttributeStat.AddModifier` returns early for a
disabled attribute and calls the base otherwise, so a disabled attribute writes nothing. `UpdateValue` is patched on the
base too: the armour class' override runs `UpdateSelfBonus()` first, outside the hook. **Not recorded**: the bulk
removals (`RemoveModifiers(descriptor)`, `RemoveAllModifiers`, `RemoveModifiersFrom`, `CleanupModifiers`,
`UpdateSelfBonus`) call `PrepareForRemoval` and `UpdateValue` without `RemoveModifier`, so they leave only the stat's
value change (none at all if `ModifiedValue` doesn't change); a change of `PermanentValue` alone writes nothing.

Consequences of the order [C]: a modifier's `stat_mod_add` comes after the `stat_value`s of the stat and its dependents; an
armour class that swaps its dexterity bonus inside an update (`UpdateInternalModifiers`, with `m_UpdateInternalModifiers`
set, so the nested updates return at once) writes the removal and the add with the old `modified`, and the new value
after the outermost update.

## The survey [M]

All `stat_*` records of the sweep's 116 runs (record and replays of 25 captures): 2,616 `stat_value`, 2,580
`stat_mod_add`, 6 `stat_mod_remove` (5,202), **none inside a root rule's span** (`rule` 0, `h` 0 on every one; they lie
between a `rule_end` or a `uc_x_end`, a draw or an observation and the next record). `stat_temp_scope_*` and
`rule_temp_mod`: none. The recording doesn't name the producer, so it is read from the records around the group of
adjacent stat records (a group is one producer's work on one unit); the producer's code is [C]:

| producer | group | records | where it lies |
|---|---|---|---|
| the helpless condition added: `UnitHelplessController.Tick` -> `UnitState.AddCondition(Helpless)` -> `UnitStatusEffectHelpless.OnActivate` -> `UnitStatHandler` (`ClearPenalty`, then `AddPenalty`) | 6 x (`stat_value`, `stat_mod_add` of descriptor Helpless, value `-5 - Dexterity.Bonus`, less the dodge bonus on the AC and the CMD): stats 11 (AC), 16 (reflex), 13 (CMD), 27 (thievery), 17 (mobility), 42 (stealth), in that order | 426 groups: 2,556 `stat_value`, 2,556 `stat_mod_add` (all 25 captures) | a unit that has fallen unconscious or died (its prone controller's `InterruptAll` just before), at the frame's `UnitHelplessController` |
| the helpless condition cleared: `RemoveConditionAll(Helpless)` -> `OnDeactivate` -> `ClearPenalty` (`RemoveModifiers(Helpless)` on six stats) | 6 `stat_value` and nothing else (the bulk removal writes no `stat_mod_remove`) | 3 groups, 18 `stat_value` (`-131`, three runs) | the main character waking after the fight (`UnitReturnToConsciousController`: her `damage` mutation just before) |
| a buff's stat modifiers: Enlarge Person (`-120/tb-cast`), `AddGenericStatBonus` and the size change | 14 `stat_value`, 8 `stat_mod_add` (6 Size, 2 DexterityBonus), 2 `stat_mod_remove` (DexterityBonus), then the `fact_attach` | 3 groups (three runs): 42 `stat_value`, 24 `stat_mod_add`, 6 `stat_mod_remove` | between `RuleCanApplyBuff`'s `rule_end` and the buff's `fact_attach` (the components activate inside the attach) |

The buff's group, in the recorded order [M, `-120` record, tick 2047, unit 558A]: the Size modifiers of the unit's size
change (descriptor Size, no source: stats 8, 11, 42, 23), then the buff's `AddGenericStatBonus` modifiers (Size, source the
buff, `source_component` the component: Strength +2, Dexterity -2). A stat's dependents are updated inside its update, so
their `stat_value`s come first: the athletics skill (19) before Strength (1); before Dexterity (2) the AC (11), whose
`DexterityBonus` modifier is swapped (`stat_mod_remove` with the old value, the outermost update's `stat_value`, then
`stat_mod_add`), the saves and skills, and initiative (26), which swaps its `DexterityBonus` too.

## What the sim writes [C, M]

`stats/ops.rs` is a port of `ModifiableValue`, and the helpless penalties, the buff's components and the AC's dexterity
swap already ran through it. `stats/trace.rs` writes the records at the points the hooks fire:

- `update_value` is the hooked base method: a stat's `in_update` flag stands for `Hooks.Updating` (set from the base body's
  start to its end, dependents inside it, after the AC's `update_self_bonus`); the outermost call writes `stat_value` if
  the value changed;
- `add_modifier` writes `stat_mod_add` after its update (not for a disabled attribute, which returns before), `remove_modifier`
  `stat_mod_remove` when it removed one; the temporary scopes the same (`open_temporary_scope` after its update,
  `close_temporary_scope` before its removal, only for a non-empty scope);
- the record is the oracle's (`sheet::modifier_json` is `Snap.Modifier`, shared with the sheet dump); inside a recorded
  combat run the pending draws are written first, as for the other records; nothing is written while the event log is off
  (hydration).

## Comparison: in the T0 stream [I]

Two designs fit: a dedicated checker in the driven replay (an observation handled where the recording has it), or the T0
projection. The projection fits better, and is what is done:

- the claim is *the same mutations at the same points, in the same order*. The T0 stream is position-by-position over
  rules, results, draws, damage, experience and facts, so a stat mutation's place among them (the `fact_attach` that
  follows a buff's modifiers; a `damage` before a helpless penalty) is compared for free; a checker would need its own
  placement logic and could not see that order;
- the machinery is there: the sim's events are the stream's other side, damaged recordings fail the same way, and the
  oracle's span and echo handling already removes what is not compared;
- cost: the projection needs a time. T0 leaves times out, but a stat mutation has a frame, so these records carry `t`
  (the recorded tick; the driver sets the sim's log tick at each frame's `tick` record): a mutation in the wrong frame is a
  difference.

The compared record (`project::stat_record`, both sides): `what`, `t`, `stat`, `owner`; for a value `old`, `new`,
`permanent`; for a modifier its descriptor, value, stack mode, `stacks`, the source fact (its unique id and blueprint: the sim's
id for the buff Enlarge Person creates is the game's, 5956 in `-120`, [M]), the source component and the item (`$item` and
blueprint), and `modified`; for a scope its modifiers. Left out: `rule`, `h`, `seq`, `origin`, `descriptor_name` (the
descriptor's name).

**Inside root spans nothing changes.** `spans::projected` still says no to every stat mutation, so a span that holds
one reports it (`stat_value` in `records_inside_driven_root_spans_are_checked_or_reported`); the sweep has none, and the
sim writes none inside a span either.

## Gaps and the run summary

An unmatched recorded mutation or an extra sim mutation is a difference: the stream is unequal and `replay unsupported`
holds "the sim's stat mutations differ from the recording's" (so `valid=false`). The one gap there is: a recorded
mutation whose owner is not a unit the sim made, or not a unit at all (`owner` null), has no stat for the sim to write, so
it is left out of the stream and counted by that reason (`StatCounts::gaps`, `replay/stats.rs`). [M] The sweep has none.
A producer the sim doesn't port is *not* a gap: its recorded mutations stay in the stream and show as differences. (Every
producer of the survey is ported, so no producer is a counted gap today.)

The summary line of `wr-combat replay`: "stat mutations in the T0 stream: compared N {by kind}, equal E, differing D, sim's
extra X; gaps G {by reason}". Equal is counted by position among the stat mutations of both streams, so one dropped
record shows as differing from there on (the stream's first difference names it).

## Results [M]

- The sweep after the change (`scripts/sweep.sh`, release build, merged with `poc` at 6bf8fef): 113 of 116 runs `ok`; the
  three not are `poc5-tb-122` tb-nearest (30 unsupported kinds of the initiative tie and its graph events, as before; the
  stat line of its record is 36 compared, 36 equal).
- Compared, summed over the 116 runs: 5,202 stat mutations, all equal at their place, frame included; no gap, no extra
  sim mutation. By producer (the survey's table): the helpless condition added 5,112 (426 groups of 12), cleared 18, the
  buff 72 (3 groups of 24).
- `scripts/check.sh` (fmt, clippy `-D warnings`, nextest): 810 tests passed. The POC-5 capture suites (`poc5`,
  `poc5_agent`, `poc5_corpulence`, `poc5_log`, `poc5_look`, `poc5_loot`, `poc5_roaming`, `poc5_spells`, `poc5_ties`,
  `poc5_stats`): 46 of 46.
- Before the change the run summaries had no such line and a wrong stat change passed: the damaged recordings of
  `poc5_stats` are the proof the comparison now fails on one.

## Tests

`stats/tests.rs` `stat_records_follow_the_oracles_hooks` (order of dependents, value and modifier; removal found or not; the
log off; bulk removal), `combat/project.rs` `stat_mutations_project_alike_from_both_sides`, and
`tests/poc5_stats.rs` (ignored by default, needs the captures):

- `the_stat_mutations_are_the_sims`: every run of `poc5-tb-120/tb-cast` and `poc5-tb-131/tb-lizard` valid, T0 equal, every
  recorded stat mutation equal, no gap;
- `damaged_stat_mutations_are_reported`: 23 damaged recordings of the cast, each unequal, differing and invalid: a
  modifier added, a value change, a removal and a unit's whole helpless group dropped; a new, old and permanent value, a
  modifier's value, descriptor, stack mode, source blueprint, source fact id, source component and the stat after it changed; a
  record's stat, owner and frame changed; a value change and its modifier exchanged, two units' groups exchanged; a value
  change repeated, a removal and a temporary scope added; a record of a unit the sim hasn't made;
- `a_mutation_of_a_unit_the_sim_hasnt_made_is_a_counted_gap`: a recorded mutation of an unknown unit and one with no owner
  are counted as gaps, and the sim's own mutations of those stats are extra (the run invalid).

## For WrathOracle (not done: only the main session records)

Neither is needed for the comparison above; both would make the survey exact instead of read from the neighbours, and
cover what is blind today.

1. **`origin` on every stat mutation**, not only in Pause mode (the caller chain, as a root rule's): the producer
   (`UnitHelplessController`, a fact's component, a level-up, an item) would be named by the game.
2. **Bulk removals**: a record of `PrepareForRemoval(mod)` (`stat_mod_drop`: the stat, the modifier), which
   `RemoveModifiers(descriptor)`, `RemoveAllModifiers`, `RemoveModifiersFrom` and `CleanupModifiers` use. Today a removal
   that leaves the stat's value unchanged writes nothing, and the removed modifiers are never named; a change of the
   permanent value alone is not recorded either (`stat_value` could add it).
