# POC-5: the combat log

Written 2026-10-15 (stream I, twenty-second session). Tags: **[M]** measured, **[C]** read in the
decompiled C# (`decompiled/`, never copied here), **[I]** inferred. No game text here: keys, GUIDs,
type names and counts only.

## Headline

| | Result |
|---|---|
| Messages | **Every combat log message of `poc5-tb-115` (tb-nearest, tb-focus), `-116` (tb-hold) and `-117` (tb-solo) is the sim's [M]**: 318 of 318 over the 12 runs (25, 27, 27 and 27 per run), text bit-exact, with its colour, prefix icon, thread instance, event and frame, and `GameLogContext` as the thread adds it (every field, and its scope count) |
| Queue | Every event the log queues (1,446 over the 12 runs) and every tick's queue (330) equal the game's [M] |
| Tooltips | **Every recorded initiative, attack and damage tooltip is the sim's, compared whole [M]** (class, header, description: the breakdowns of the attack bonus, armour class and damage, a critical's confirmation, the difficulty's multiplier): 219 of 219 on `-115`..`-117`'s 12 runs (81 initiative, 138 attack and damage), 1,038 of 1,038 on `-120`..`-128`'s 36 (360 and 678). Not compared: a spell's `TooltipTemplateAbility` (`-120`, one per run) |
| Inputs | 9 projectile hits (`GameLogEventProjectileHit`) taken from the recording: projectiles aren't modelled |
| Fail closed | `crates/wr-sim/tests/poc5_log.rs` damages `-117` 54 ways (a message's text, colour, icon, thread, thread instance, event, frame, number, scope, handler flag or context; a dropped, duplicated or reordered message or tick; a tick's queue; an event's fields, type, override, rule (another of its class and units), or its place in its rule's lifecycle; a non-rule event before the gameplay records it follows, out of its raise (an attack of opportunity after its count's decrement, the party's combat state before the turn controller's handler has done its work or after the join controller), or raised by another caller; a log tick and its messages before an earlier controller's record; a projectile's rule reference; a rule's own event in its span: disabled, of another rule, early, twice; an unknown log record, a gate flag; the begin record's fields, root types, dispatch rows (a duplicate, another thread, an ordinal), context fields (missing, malformed); the declaration dropped): each is reported |

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
  raises, the experience and damage mutations before it; the driven replay places each log tick
  and message in `GameLogController`'s phase and the party's combat state events in the join
  controller's (a record out of the frame's controller order is reported); then it compares, at
  the run's end, the sim's queue, ticks and messages with the
  recorded ones as one ordered stream (frame, type, the rule itself: the window's rules are
  numbered alike, its class and units; a non-rule event's fields, `IsEnabled`, the override; each
  tick's queue; each message's thread instance, event, text, colour, icon, scope count, context
  and, where made, tooltip) and the begin record's tables and context with the sim's, and reports
  each difference. The context a message is added with includes what its tooltip's code sets
  (a critical roll's confirmation: `Roll`, `D20`, `Modifier`, `DC`), so that part of the tooltip's
  code is ported though its text isn't. The driven
  replay's span check (`replay/commands.rs` `span_ok`) accepts a rule's own `clog_q` only where the
  game writes it: after its `did`, outside its handlers, once, enabled, of its class and id.

## The tooltips (stream Q)

What the observer records is the whole tooltip [C]: an attack's and a damage's is a
`TooltipTemplateCombatLogMessage(header, description)`, a `TooltipTemplateSimple` whose bricks are
its `Header` (a title) and `Description` (a text) and nothing else (`TooltipBaseTemplate`'s other
members are defaults no log code sets), and `clog_msg`'s `tip` has its class and both strings. The
observer is unchanged.

How the game makes them [C]:
- **`AttackLogMessage.GetData`** (no auto hit or miss): a concealment's lines (the context's
  `ConcealmentStatus`, which can outlive one message in a tick's scope), the attack number (`{attack_number}`
  of `{attacks_count}`, when more than one), the roll (natural 20, critical miss when that string
  isn't empty, else the roll with `{roll}`, `{d20}`, `{mod}`, `{dc}`) and its result, a critical
  roll's confirmation (the context set to the confirmation's `Roll`, `D20`, `Modifier` and the
  critical AC as `DC`, then `DC` back), fortification, then `AppendAttackBonusBreakdown`
  (`<b>…: +N</b>`, `AllBonuses`: the rule's own then its inner rule's, then the inner rule's
  `StatModifiersAtTheMoment`, the `AdditionalAttackBonus` display modifiers) and
  `AppendArmorClassBreakdown` (`<b>…: N (…)</b>` with the `Flatfooted` and `Touch` strings, `ArmorClassBase: AC.BaseValue`
  read when the text is made, then `StatModifiersAtTheMoment`: `GetDisplayArmorModifiers` taken
  inside the rule's temporary modifiers scope, so the rule's own bonuses are among them).
- **`StatModifiersBreakdown`**: a static list of entries (value, source text, descriptor):
  a rule bonus shows its fact's name, else its `BonusType`'s (`BonusSourceStrings`), else its
  stat's (`StatsStrings`), and a stat's shows even at zero; a stat modifier's descriptor (`Other`
  for none) with its item source or fact (the attack bonus takes the item first, the AC the fact);
  sizes without a source merge. `AppendModifiersBreakdown` sorts by `ModifierDescriptorComparer`
  (its 45 listed descriptors, then the enum's others ascending) with `List.Sort`, Mono's
  **unstable** introsort (equal descriptors, e.g. several `Other` feats, keep the order insertion
  sort leaves them in), then writes each nonzero entry: the descriptor's name (`AbilityModifiers`)
  or the source, `<color=#…>` (`UIRoot.TooltipColors` `Bonus`/`Penaty` through `Color`), the sign
  (`+` or the en dash), the source in brackets after a descriptor.
- **`GetDisplayModifiers`**: each descriptor's list in the comparer's order, its stacking
  modifiers, then its largest bonus and largest penalty of the rest (an armour or encumbrance
  penalty above the stacking ones skipped), without zeros (and disabled morale or insight
  modifiers for the attack bonus); the AC's drops dodge and Dexterity for a flat-footed target,
  armour and shield for a touch attack.
- **Names**: `EntityFact.Name` is the blueprint's `m_DisplayName` (converted in the log's scope),
  a feature's with its parameter (a weapon category in parentheses: `StatsStrings.GetText(WeaponCategory)`); an
  item enchantment shows its owner's name; an empty name falls back to the fact's context's
  parent's source item. Items: `ItemEntity.Name`, now with `BlueprintItemArmor.Name` (its type's
  default name) and `BlueprintItemShield.Name` (its armour component's) [M: armour and shields here
  have no names of their own].
- **`DamageLogMessage.GetData`**: `Tooltip` (`{count}`), the values' sum line when there are
  several or the difficulty changed the result (`(a + b) (×m) = r`), then each value
  (`AppendDamageDetails`: the modified dice, the total bonus with a critical's multiplier, the
  multipliers, the reduction, `= <b>N</b>`, the damage forms (`DamageFormString`), sneak or
  precision; then, indented, `AppendSources`: the weapon (`Tooltips.Source`) for the bundle's
  first damage, the dice with `AppendDiceModifiersBreakdown` (`Tooltips.BaseValue` then each
  modification: a critical's `CombatTexts.CriticalHit`, a size's descriptor) and `SkillcheckTooltips.RollResult`
  when there is more than dice, the bonus's breakdown (`+N` with its modifiers, a base value line
  for what they don't explain), a critical's multiplier with `CombatTexts.CriticalHit`), and the difficulty's line
  (the multiplier, then the `Difficulty` descriptor's name and, in parentheses, `UISettingsRoot`'s `DamageToParty.Description`). The
  `DamageLogMessage` object's own fields (`m_BonusPercentModifier`…) carry from one value to the
  next. Multipliers are `float.ToString()` (Mono's "G7"; the oracle's culture writes `.`).

The port (`crates/wr-sim/src/game_log/tips.rs`): the rules keep what the tooltips read while the
log is on (`RuleCalculateAC.stat_mods` inside its scope, `RuleCalculateAttackBonusWithoutTarget.stat_mods`,
`RuleAttackRoll.log_rules`, `RuleDealDamage.result_list`), and the threads make the text when they
handle the event. The game's strings: `log-strings.json` has a 35th container, `SettingsRoot`
(`BlueprintRoot.SettingsRoot`'s `Settings.DamageToParty` and `EnemyCriticalHits` entities, followed
to their objects by `wr-rules-data`: rerun it). Three rule fixes the tooltips found (the numbers
were right, the sources weren't kept): `RuleCalculateAC`'s temporary modifiers carry their bonus's
fact (`TemporaryModifiersScope` sets `Source`), `RuleCalculateWeaponStats` copies the
`AdditionalDamage` modifiers with their source fact, and `DistanceDamageBonus`
adds its damage with its fact. Every capture run stays as valid as it was.

Reported when reached, not made: a breakdown of more than 16 entries (the static list's capacity
decides the sort past insertion sort), hidden facts (`UnitPartHiddenFacts`), a buff without a name
of its own, a feature parametrized by a blueprint, a weapon category or enum value without a string,
a usable item without a name, maximized, empowered, declined, immune or reduced damage (their
sources' texts), a critical not applied to the bonus (`EnemyCriticalHits`' setting text), a damage
capped at its maximum, damage types other than physical, a multiplier that `G7` writes in
scientific notation. Mirror images, parry, fortification and concealment rolls are reported by the
rules already; the sim's damage has no redirection, `SourceFact`, post-critical bonus or additional
critical multiplier (none is made by a ported handler).

## The collection event and the unidentified name (stream loot, 2026-10-17)

`GameLogEventItemsCollection` (`ItemsCollection.OnItemAdded`/`OnItemsRemoved` through `IItemsCollectionHandler`) is raised
by the sim now (`game_log::items_collection`; the observer's `clog_q` has `Event` (0 added, 1 removed), the collection by
type, the item entity with its class, `Count`). `ItemsCollectionLogThread` makes a message only for the player's inventory
and a lootable item outside the additional limbs' slots (reported where reached); a dead creature's collection makes none,
but its removal event takes a place among the frame's five (`MaxLogsInFrame`), which is the whole of the one-frame shift
`poc5-tb-131` showed. The check places a removal right after the dropped loot's rotation draw. Item names now cover
`BlueprintItem.DefaultNonIdentifiedName` (`UnidentifiedItemNames[ItemType]`, a usable item's `UsableItemTypeNames`).
Details, the dropped loot and the prone controller: `docs/poc5/loot-and-prone.md`.

## Not done

- A spell's tooltip (`TooltipTemplateAbility`, `RulebookCastSpellLogThread`): counted as "not made".
- The tooltips' branches the fights don't reach are reported when reached (see "The tooltips").
- Threads and events the fights don't reach: reported when reached (`… isn't ported`), among them
  healing, saving throws, skill checks, spells, combat manoeuvres, stat damage, a parry, mirror
  images, fortification, concealment's message parts, a map object's bark, the dialogue's threads
  in a combat replay.
- `GameLogDisabled` scopes and units' `DisableBattleLog` are taken as unset [I] (no code that sets
  them runs in these fights; the recorded `en` checks every event).
- Barks and projectile hits are recorded inputs (UI and projectiles); a bark wasn't recorded in
  these fights.
- Real-time combat and dialogue windows: the observer records only in turn-based windows.
