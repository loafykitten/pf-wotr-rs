# POC-5: fixture fights for the driven replay's coverage (stream J)

Status (2026-10-15, twenty-second session, stream J): what the recorded fights already exercise, what no available save
can reach with the fixture driver, three new fixtures (`tb-cast`, `tb-chain`, `tb-lizard`), the driver's new
`cast/<guid>/<unit>` policy action, and the saves Ember would need to make for the rest. Tags as elsewhere: [M] measured,
[C] from the decompiled code, [I] inferred.

## What the current captures already exercise [M]

Checked on `poc5-tb-95` to `-97` and `-111` to `-117` (record runs; the replays are tick-equal):

- **Critical hits, confirmed.** `-95/tb-focus` (valid on every run): t=1706, 523B on the PC, the d20
  20 then the confirmation 19, the damage dice rolled twice (`Critical: true` modifications); t=2228 the PC on 51FC,
  20 then 13 against AC 13. Unconfirmed threats in `-95`, `-96`, `-97`, `-113` to `-115` too.
- **Five-foot steps.** A party attack whose approach path is at most `TurnController.MetersOfFiveFootStep` (7.5 ft)
  takes the smart cursor's five-foot-step variant [C: `UpdateSmartCursorVariants`]: `cd_move` records with `limit` 1
  in `-96/tb-hold` (558A, path 1.28 m), `-97/tb-solo` (5671, path 1.01 m) and `-115/tb-focus` (558A, path 1.83 m).
- **Full attacks.** The PC fights with two longswords: its full attacks are three attack rules (`IsFullAttack`,
  `AttacksCount` 3, `TwoWeaponFightingAttacks`), `-95/tb-focus` t=2148 to 2305 and `-96/tb-hold` t=1372 to 1510;
  Anevia's Rapid Shot full attacks (two) in both.
- **The movement simulation's branches through the UI.** The path visualizer runs
  `SimulateTickMovementForTurnBased` for the party: 300 to 400 `ag_sim` steps per fight in `-114` to `-117`, multi-step,
  with an approach radius on most. No AI unit has a `ScorePath` step in `-111` to `-117`: the AI's use is case (a) below.

Not exercised by any capture: (a) an AI `ScorePath` simulation of more than one step; (b) an impatience hard target;
iterative attacks (base attack +6); an ally unconscious or dying; a spell; a loot drop.

## Why the available saves don't reach (a) and (b)

**(a) The AI's movement simulation.** `AiAction.ScorePath` simulates only when the path's end isn't close enough to
the target [C]. The AI's `FindPath` in turn-based combat is an `ABPathWithClearance` whose search also stops at a node
`6 * CurrentSpeedMps` from the start [C: `UnitMovementAgent.FindPath`, `docs/poc5/paths.md`]; that is the unit's double
move: 24.38 m for the 40 ft centipedes and flies (`maxd` in `ag_req_end` [M]), 18.29 m for the 30 ft lizard, 12.19 m
for a 20 ft unit. So the simulation runs for a target more than a double move away (or, as in `-103`, the grid-snap
case of a one-step two-point path). The AI scores every enemy in its group's memory, reachable or not [C:
`AiBrainController.CalculateTargetsScore`, `DecisionContext.CalculateEnemies`], so one remembered far target is enough.
But a unit enters an enemy group's memory only when a member sees it (8.5 m out of combat, 11.7 m in combat, for these
factions: `docs/poc5/memory.md`) or when it makes an offensive action on one [C]. In Manual_18, 19 and 20 every enemy
group's members stand within about 10 m of each other [M: the state dumps], and nobody in the party attacks from beyond
an enemy's double move: Seelah's shortbow (second weapon set, which the driver doesn't switch to) reaches 40 ft, Anevia's
longbow approach radius is 16.64 m [M] (and she is an AI ally), Camelia's cantrips aren't on her action bar (below). The units the
enemies remember are therefore always within their reach [I]; the party can't retreat either (the driver has no
turn-based move, and nothing in the party outruns a 40 ft centipede). A save is needed (below).

**(b) An impatience hard target.** `UnitImpatienceController.UpdatePriorityTarget` adds a hard target when a unit's
consecutive standard `UnitAttack`/`UnitUseAbility` commands on one enemy that ended with `Success` and dealt no damage of
1 or more reach the threshold of the unit's impatience [C]; then each further change draws the 20% switch. Every unit in
these saves has normal impatience (threshold 3, switch 20 [C: `BlueprintRoot.SystemMechanics.Impatience`]); only army
units carry `ChangeImpatience`. It counts for any unit, the party's included. Recorded attack bonuses [M]: the young
centipedes +4 (+6 flanking) against the PC's AC 18 (hit 35%) and Seelah's 21 (20%); the party +3 to +7 against AC 11 to
13. The young centipedes have 3 HP and Anevia's Rapid Shot kills one with each hit, so no centipede lives for three of
its own attacks in the recorded fights. A fixture here reaches a hard target only by the dice; the new fixtures lengthen
the fights (more attacks by the fly and the lizard, which survive longer). A deterministic one needs a save (below).

## The driver: `cast/<ability guid>/<unit>` (`oracle/mods/WrathOracle/TurnBased.cs`)

A new `tbfight` action: on the unit's turn, that spell is cast on that unit (ally or enemy) as a player does it [C]. The
driver rebuilds the action bar's spell slots as `ActionBarGroupElement` does for its Spell slot type (the unit's
visible cantrip abilities, then per spellbook its memorized slots or its special, known and custom spells, each spell
once) and clicks the game's own slot object (`MechanicActionBarSlotAbility`, `...MemorizedSpell` or
`...SpontaneousSpell` `.OnClick`: the turn's ability prediction, `SelectedAbilityHandler.SetAbility`, the pointer in
ability mode); then the pointer over the target (`InputPin.PointAt`, whose pick takes the ability handler in that mode),
the turn's predictions (`TurnController.HandlePortraitHover`), and what `ClickWithSelectedAbilityHandler.OnClick` does
for a spellbook spell (`UnitUseAbility.CreateCastCommand`, `OnCastRequested`, `Run`), the command given the predicted
path as its forced path as the attack path does; the hover, the pointer and the pointer mode released on every path.
When the slot isn't active (`IsPossibleActive`: the slot spent, the action used, prone...), the target is gone or can't
be pointed at, or the spell can't take it, nothing is selected: the input `tbcast_impossible`, and the policy attacks the
nearest enemy instead. Inputs: `tbcast` (unit, target, ability, slot kind, spell level, spellbook, policy, round, turn
index, command, and under `at` the positions, pointer and path), `tbcast_impossible` (with `why`), `tbcast_rejected`
(the command can't run); a replay re-makes them and checks the slot kind, level and spellbook it finds against the
record. Only unit-targeted spells whose cast command is a plain `UnitUseAbility` (no custom move or attack, touch
spells, service casters, items, variants, spellstrike); anything else throws while recording and invalidates a replay.

Camelia's cantrips are known spells but not on her action bar: a memorizing spellbook shows its memorized slots, and
she has none at level 0 [M: Manual_18, memorized level 1: Cure Light Wounds twice, Enlarge Person]. Cure Light Wounds
is a touch spell (delivered by a second click, not driven). So the one spell a player could cast here through the
driver is Enlarge Person.

## The fixtures

All on `Manual_18_WR_FirstBattle` (party: the PC, Seelah 558A, Camelia 5671; Anevia 5198 follows the PC as an AI ally).

- **`tb-cast`** (`poc5-tb7`): the first battle (`move:57,34.5,155.5` as `tb-focus`), everyone `nearest` but Camelia,
  whose policy is `cast/c60969e7f264e6d4b84a1499fdcf9039/558A`: Enlarge Person (her memorized level-1 slot, close range)
  on Seelah in her first turn, approaching if needed, then (the slot spent) the nearest enemy. Exercises a spell from a
  memorized slot: `UnitUseAbility`, the ability's approach, its execution process, the slot spent, a buff that changes
  the target's size (Large: reach, strength, dexterity, AC and attack modifiers, corpulence) for the rest of the fight.
- **`tb-chain`** (`poc5-tb5`): the first battle (`nearest`), then the PC walks (`move:46.2,34.4,137`, a point of the
  recast path 3.5 m from the fly) to the lone giant fly 527A (group Optional2, faction Mobs) and fights it, Anevia
  following; Seelah and Camelia stay where the first battle left them (the driver moves only the main character), about
  20 m away, and walk in over several turns. Exercises a second combat in one window, a flying enemy, partial
  approaches by the party, and longer exchanges.
- **`tb-lizard`** (`poc5-tb6`, lowest priority): `tb-chain`, then up the ramp (`move:65,40.8,120`) to the fly 529A and
  the ranged centipede 5031 (Optional3: an AI ranged attacker), then (`move:80,44.1,115`) to the roaming monitor lizard
  5052 (BeforeLann). The lizard carries a lootable `MonitorLizardPelt` [M: `m_NotLootable` false], so its death should
  drop loot (`DropLootToGround` with a real drop) [I], and its bite (1d8, Str 17) against the PC alone at the front can
  drop the PC [I]. Risks [M: Manual_18's area state]: the ramp crosses the active once-only zone `Earthquake_3`, whose
  enter action plays a cutscene; `LannDialogueTrigger` is active, a box of about 10 by 30 m east of the lizard (x 97 to
  107), which the fight shouldn't reach [I]; Lann and Wenduag stand about 16 m from the lizard and their faction is
  attacked by the lizard's.

## Saves Ember would need to make

- **(a) An AI unit walking several steps:** in the prologue caves, after recruiting Lann and Wenduag (their longbows),
  standing west of the two small earth elementals (group `a8eebc18-32de-4693-9c0e-4787f97a485c`, around x 143, z 65),
  more than about 12 m from them and not in combat. A fixture then walks the PC in to start the fight and has the archers
  shoot (`nearest`) from their longbow range (about 16.6 m): the elementals move 20 ft, so their double move (12.19 m) is
  shorter than their path to an archer, and their `ScorePath` simulates several steps (the shift, the cap at the movement
  left, possibly the approach radius) [I]. Any fight where a party archer can shoot 20 ft melee enemies from beyond 13 m
  would do.
- **(b) A hard target, deterministically:** a save just before a fight against an enemy one party member's weapon
  can't damage (damage reduction it doesn't bypass, a swarm, an incorporeal enemy against a non-magic weapon), out of
  combat. A `focus` policy on it makes every one of that member's attack commands an attack without effect: a hard
  target on the third, then the switch draw on each further one (and `SetupPriorityTarget` over its bother units). For
  the AI's side (an enemy's `ipt`/`disliked` reads): weak melee enemies against a heavily armoured party member who ends
  his turns, with no AI ally to kill them (after Anevia leaves the party's company).
- **Iterative attacks:** any save just before a fight with a party member of base attack +6 or more; the driver's
  attack on an adjacent enemy takes the smart cursor's full attack [C].
- **An ally dying, deterministically:** a fight against enemies that out-damage a weak party member who ends his turns;
  `tb-lizard` may drop the PC by chance.

## Results [M]

Driven replay (`wr-combat replay <fixture> --run record|replay-1|replay-2`, release build of this branch); "new" kinds are
those no run of `-115` (`tb-focus`, `tb-nearest`), `-116` (`tb-hold`) or `-117` (`tb-solo`) reports, after normalizing
command numbers, unit ids and numbers. Those captures carry the `combat_log` observer too, whose `clog_q` records inside
rule spans set off the commands and impatience cascades that make every one of them invalid until stream I's fix
lands (the shared kinds below: `UnitAttack` state rows, attack lists, animations, afin/style/nequip reads, impatience
not established, `HandleUnitDeath` records, `Commands.Empty`, running commands); they are not new.

### `poc5-tb-118/tb-chain`

Tick-exact in the game (5,239 ticks, 413 rules). Every run: 61 unsupported kinds, end state 1.0000 (21 units), all 413
rules, 354 results, 48 draws, 36 mutations equal, turn order checks 13/13. Two combats in the window (t=170 to 1872,
centipedes; t=2434 to 5179, the fly 527A with the PC, Seelah and Camelia), both replayed without a kind of their own:
the second combat start, its sort and its end raise nothing.

New kinds, by area:
- **Rule handlers (the fly's facts, not ported):** `AddInitiatorAttackWithWeaponTrigger` on `RuleAttackWithWeapon`
  (`GiantFly_Feature_Disease`, `1fa6430e6064d5f4f8553c67257a1878`) and `AttackBonusConditional` on `RuleAttackRoll` and
  `RuleCalculateAttackBonus` (`Airborne`, `70cffb448c132fa409e49156d013b175`), about and did, at the fly's bite on the PC
  (t=4668).
- **Commands (`UnitAttack.UpdateTarget`):** "command N's new target after OnTick has a wrapper seen before" (53 per
  run): the PC's full attack retargets with a target wrapper the sim saw before; it may be downstream of the
  combat-log cascade [I], to be rechecked once stream I's fix lands.

Also exercised for the first time [M]: the AI's `ScorePath` five-foot-step branch (the fly, t=4728: an `ag_sim` with
`max` 2.286, the five-foot step's range, score 1.1 for a full-round action), a flying unit's simulation steps
(`ag_sim` of 527A), a crit threat against a flying target (t=4085). Not reached: an AI simulation of more than one step,
a hard target (`cnt` at most 1).

### `poc5-tb-119/tb-lizard`: the game's replays weren't tick-exact (fixed in the oracle, stream L)

The record run is clean (12,288 ticks, 749 rules), but both replays were invalidated by the oracle itself, with no
draw mismatch, at path request `?#0` (record t=10021, replay-1 t=9947, replay-2 t=10503, each to another end point),
leaving 227 and 204 tape entries unused. The guess above it (a drift on the walk up the ramp across `Earthquake_3`)
was wrong: the fights number four (combat starts at t=170, 2609, 4260 and 9352), the ramp walk comes before the
third, and its two cutscene commands are tick-equal in every run [M]. Two independent causes
(`docs/poc5/tb-replay.md`, "Projectile arrival and critter paths"):

- **Projectile arrival** [M]: Anevia's killing arrow on the fly 529A, launched at t=6918 in every run with the
  same snapshot, hit at t=6938 in the recording and 6937 in both replays (the target's bone pose, T1), so the third
  fight ended a tick early; the scheduler's tolerance carried the one-tick offset to t=9898. The oracle now pins
  each projectile's arrival tick (`ProjPin.cs`).
- **The invalidation itself** [M, C]: `?#0` was a rabbit's path (`Kingmaker.Visual.Critters.Rabbit`, its
  `CritterMoveAgent`), chosen with presentation draws, checked as a strict request. Critters' requests are now
  identified and delivered natively, owed nothing.

Results on the fixed oracle: `poc5-tb-121` tick-exact in the game; `-129` (every observer) valid in the driven replay on every run once the mesh raycast is in (stream S); `-134` (with the camera gates) has a camera-view drift in one replay (docs/poc5/tb-replay.md, the camera's gates).
