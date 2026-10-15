# POC-5: the combat log

Written 2026-10-15 (stream I, twenty-second session). Tags: **[M]** measured, **[C]** read in the
decompiled C# (`decompiled/`, never copied here), **[I]** inferred. No game text here: keys, GUIDs,
type names and counts only.

## Headline

| | Result |
|---|---|
| Messages | **Every combat log message of `poc5-tb-115` (tb-nearest, tb-focus), `-116` (tb-hold) and `-117` (tb-solo) is the sim's [M]**: 318 of 318 over the 12 runs (25, 27, 27 and 27 per run), text bit-exact, with its colour, prefix icon, thread instance, event and frame, and `GameLogContext` as the thread adds it (every field, and its scope count) |
| Queue | Every event the log queues (1,446 over the 12 runs) and every tick's queue (330) equal the game's [M] |
| Tooltips | 81 compared and equal (the initiative rolls'); the attack and damage tooltips (the stat breakdowns, 138) are not made yet: counted, not compared |
| Inputs | 9 projectile hits (`GameLogEventProjectileHit`) taken from the recording: projectiles aren't modelled |
| Fail closed | `crates/wr-sim/tests/poc5_log.rs` damages `-117` 51 ways (a message's text, colour, icon, thread, thread instance, event, frame, number, scope, handler flag or context; a dropped, duplicated or reordered message or tick; a tick's queue; an event's fields, type, override, rule (another of its class and units), or its place in its rule's lifecycle; a non-rule event before the gameplay records it follows, out of its raise (an attack of opportunity after its count's decrement, the party's combat state before the turn controller's handler), or raised by another caller; a projectile's rule reference; a rule's own event in its span: disabled, of another rule, early, twice; an unknown log record, a gate flag; the begin record's fields, root types, dispatch rows (a duplicate, another thread, an ordinal), context fields (missing, malformed); the declaration dropped): each is reported |

## How the game writes the log [C]

- **`GameLogController`** (`Kingmaker.UI.Models.Log/GameLogController.cs`) is the last controller
  of every game mode's tick (`GameModesFactory`: registered after everything else in Default,
  Dialog, Pause, Cutscene and the rest, and in tactical combat). It is a rulebook hook
  (`IRulebookEventAboutToTriggerHook`, `IRulebookEventDidTriggerHook`, through
  `RulebookEventHooksManager`, registered when its game mode activates).
- **Rule events.** `Start` builds `m_EventToThreads` (each event type's threads, from the threads'
  `IGameLogRuleHandler<T>`/`IGameLogEventHandler<T>` interfaces, in `LogThreadService.AllThreads`
  order) and `m_RootRulesTypes` (the rule types a thread handles: 21). When a rule is about to
  trigger, a rule of a root type, or any rule while an event is open, becomes a
  `GameLogRuleEvent<T>` (`GameLogEventsFactory`) and is pushed on `m_EventsStack`
  (`TryAddInnerEvent` always succeeds). When a rule did trigger (after its did subscribers,
  before `OnDidTrigger`) and its event is the stack's top, it is popped and queued. So an attack
  queues every rule it triggers, innermost first (completion order) [C, M].
- **Other events** come from `GameEventsHandler` subclasses subscribed to the event bus
  (`IPartyCombatHandler` twice: `GameLogEventCombatStateChanged` then
  `GameLogEventPartyCombatState` [M], `IUnitLifeStateChanged`, `IAttackOfOpportunityHandler`,
  `IPartyGainExperienceHandler`, `IKnowledgeHandler`, `IProjectileHitHandler`, barks, …), queued
  as they are raised.
- **`AddReadyEvent`** opens a `GameLogContext` scope, sets `IsEnabled &= !GameLogDisabled` (a rule
  event's `IsEnabled` also reads its rule's `DisableBattleLog`: `DisableBattleLogSelf`, the
  initiator's and a target event's target's `DisableBattleLog`), takes `GameLogContext.Message` as
  the override, and appends.
- **`Tick`** (in one scope): an event stack left open is an error (cleared); then at most
  **five** queued events (`MaxLogsInFrame`), in order, go to their threads (an event without a
  thread, `GameLogEventKnowledge` for one, still takes its place); the rest wait for the next
  frame. A thread's exception is logged and the next thread runs. So a message's frame depends on
  every rule an attack triggers [C, M: a fight's 112 to 136 queued events make 25 to 27
  messages over 26 to 31 ticks].
- **`GameLogContext`** (`GameLogContext.cs`): static `Property<T>` fields. A scope counts;
  releasing the last calls `Clear`, which resets every field but `SourceFact`, `Formula`,
  `Rations`, `RoundNumber` and `RollChance` (those outlive scopes, so a message can read a value
  from long before). Inside a tick everything set by one thread is seen by the next event's
  threads in the same tick (one outer scope) [C].
- **Threads** (`LogThreads/*`) build `CombatLogMessage`s (text, `Color`, `PrefixIcon`, tooltip)
  and `AddMessage` them (unless `GameLogDisabled`). `MessageLogThread` and
  `WarningNotificationLogThread` are two instances each; `KnowledgeLogThread` isn't in the
  service. Those the fights use:
  - `AttackLogThread` (`RuleAttackRoll`): nothing for a fake roll, a unit out of the game or an
    auto hit; the message is `AttackLogEntry ?? Attack.GetData(rule)`; while `SuspendCombatLog`
    (a ranged `RuleAttackWithWeapon`'s roll) it is kept on the rule, and
    **`ProjectileHitLogThread`** adds it when the projectile hits (clearing the flag; it makes
    the message then if none is kept) [C, M].
  - `AttackLogMessage.GetData`: `{source}`, `{target}`, `{text}` (the weapon's `Name`, or the
    reason's when the weapon has no wielder), then a suffix: critical (confirmed, or negated by
    fortification), hit, auto miss, natural 1 (its own suffix unless blank), miss (and a mirror
    image's), then sneak. The tooltip: attack number, concealment, the roll, the critical
    confirmation, fortification, then the attack bonus and armour class breakdowns
    (`StatModifiersBreakdown`).
  - `DamageDealtLogThread` with `DamageLogMessage.GetData`: `SourceUnit` is the initiator unless
    it is the target, `SourceFact` the reason's fact; with a source, the reduced message when
    the damage before difficulty is below the damage without reduction or any part is halved;
    without one, the failed-check or unknown-source message. Tooltip: the damage breakdown.
  - `UnitInitiativeLogThread`, `UnitLifeStateChangedLogThread` (the unit dead when the tick
    runs, not immortal), `PartyCombatLogThread`, `AttackOfOpportunityLogThread`,
    `PartyGainExperienceLogThread` (`Experience * ExperienceRatePercent / 100`): a
    `GameLogMessage` each, the last four without a tooltip (`hasTooltip: false`).
  - `RulebookAttackWithWeaponLogThread` writes only when the attack's reason has a context whose
    blueprint is a feature; `RulebookCanApplyBuffLogThread` only on an immunity.
- **`GameLogMessage.GetData`**: the text (the override, else `Message`), the tooltip (with the
  concealment's text when the context has one), `null` for empty text, the message's own icon or
  `GameLogContext.GetIcon()` (no target: none; target of the player's faction: left arrow;
  source of the player's faction: right arrow; else grey arrow), its colour or `DefaultColor`
  when all four channels are 0.
- **Templates** (`Kingmaker.TextTools/LogTemplate*.cs`, `LogHelper`): `source` (the unit as
  `<b><color=#RRGGBB>name</color></b>` with `ColorUtility.ToHtmlStringRGB(Blueprint.Color)`,
  else `SourceFact?.Name ?? ""`), `target` (`<no unit>` for none), `text`, `description`,
  `formula`, `text_with_tags` (processed again), `count`, `count_form` (the first form for
  exactly one), `roll`, `mod`, `dc`, `chance_dc`, `roll_chance`, `attack_number` (one-based),
  `attacks_count`, `round`, `d20`/`d100` (`GetRollDescription`: the result, or the roll history
  with the kept roll marked and the rerolls' sources). In a log scope `mf` reads the source
  unit's gender first.
- **Strings**: `GameLogStrings` is a `StringsContainer` asset, `BlueprintRoot.LocalizedTexts.GameLog`,
  outside the blueprints: the runtime pack holds it as an index into `BlueprintReferencedAssets`
  (`Bundles/blueprint.assets`), like the shared strings [M].
- **Item names**: `ItemEntity.Name` is the blueprint's `Name` when identified, else
  `NonIdentifiedName` (its own text when set; a weapon's default is its type's `DefaultName`).
  A weapon's `Name`: its own text, else the type's default name with the enchantments' prefixes
  and suffixes, else the type's name [C; M: an unidentified bow in these fights].

## WrathOracle's observer `combat_log` (`oracle/mods/WrathOracle/LogObs.cs`)

Records in turn-based windows (`Sched.Obs`), field and auto-property reads only:
- the begin record's `game_log`: `roots`, `threads` (each event type's threads, by name and
  ordinal), the queue and open stack when the window opens, `GameLogContext`'s fields (their
  `m_Value`, without the getters, which log outside a scope) and the scope count;
- `clog_q` at each `AddReadyEvent` (postfix): the event's id, type, rule id (or its own fields,
  units by id), `m_IsEnabled`, the override's key; a non-rule event's caller chain;
- `clog_tick` at `Tick`'s prefix when anything is queued or open: each queued event's id and
  `IsEnabled`, the stack's size, the scope count;
- `clog_msg` when a thread's list grows in `AddMessage`: thread, ordinal, the event being
  handled (from every `HandleEvent`'s prefix), text, `ToHtmlStringRGBA(Color)`, icon, tooltip
  (class, header, description), and the context as a diagnostic (never an input).

Captures: `poc5-tb-115` (tb-nearest, tb-focus), `-116` (tb-hold), `-117` (tb-solo), each record and two
replays, all tick-exact in the game.

## The port

- `crates/wr-text`: the log templates (`Template::Log`, `LogTag`), the context asked of the
  `Env` (`log_unit`, `log_source_fact_name`, `log_str`, `log_int`, `log_roll`: unsupported by
  default), `LogRoll::description` (`GetRollDescription`).
- `tools/wr-rules-data` also writes `cache/sim/log-strings.json`: the 34 `LocalizedTexts`
  containers (followed through `BlueprintReferencedAssets`), each `LocalizedString` reduced to
  its actual key. Run it after pulling.
- `crates/wr-sim/src/game_log/`: `GameLogController` and `GameLogContext` over the world
  (`World::game_log`, on when a recording declares `combat_log`): the rulebook hooks
  (`rulebook::trigger`), the bus events the sim raises (party combat in the recorded
  `combat_start`/`combat_end`, life state in `UnitLifeController`, attacks of opportunity in the
  engagement controller, party experience in `GainPartyExperience`, knowledge in
  `InspectUnitsManager`), the tick (`Phase::GameLog`, the frame's last work phase), the threads
  listed above with their messages and the `GameLogMessage` tooltips, item names (`names.rs`).
  A rule's `D20` is the first d20 the sim rolled inside it (known with its history), else known
  by its result only (reading its history is reported).
- `check.rs` checks every log record's exact shape and numbering (event ids from 1, messages
  from 0), each rule event's place (right after its rule's `did`, before its `rule_end`, only its
  damage between), each non-rule event's caller (its handler and raiser) and, for one the sim
  raises, the experience and damage mutations before it, then compares, at the run's end, the sim's queue, ticks and messages with the
  recorded ones as one ordered stream (frame, type, the rule itself: the window's rules are
  numbered alike, its class and units; a non-rule event's fields, `IsEnabled`, the override; each
  tick's queue; each message's thread instance, event, text, colour, icon, scope count, context
  and, where made, tooltip) and the begin record's tables and context with the sim's, and reports
  each difference. The context a message is added with includes what its tooltip's code sets
  (a critical roll's confirmation: `Roll`, `D20`, `Modifier`, `DC`), so that part of the tooltip's
  code is ported though its text isn't. The driven
  replay's span check (`replay/commands.rs` `span_ok`) accepts a rule's own `clog_q` only where the
  game writes it: after its `did`, outside its handlers, once, enabled, of its class and id.

## Not done

- **Tooltips** of attacks and damage (the attack bonus, armour class and damage breakdowns,
  `StatModifiersBreakdown`, the difficulty settings' texts): counted as "not made".
- Threads and events the fights don't reach: reported when reached (`… isn't ported`), among them
  healing, saving throws, skill checks, spells, combat manoeuvres, stat damage, a parry, mirror
  images, fortification, concealment's message parts, a map object's bark, the dialogue's threads
  in a combat replay.
- `GameLogDisabled` scopes and units' `DisableBattleLog` are taken as unset [I] (no code that sets
  them runs in these fights; the recorded `en` checks every event).
- Barks and projectile hits are recorded inputs (UI and projectiles); a bark wasn't recorded in
  these fights.
- Real-time combat and dialogue windows: the observer records only in turn-based windows.
