# POC-5: the dropped loot, the collection event, the unidentified name and the prone controller (stream loot)

Written 2026-10-17 (twenty-second session, stream `loot`). Tags: **[M]** measured, **[C]** read in the decompiled C#
(`decompiled/`, never copied here), **[I]** inferred. No game text here: keys, GUIDs, type names and counts only.

`poc5-tb-131` and `-134` (`tb-lizard`, Manual_18's cave fight against the monitor lizards) reach things no earlier
capture did: a dying lizard's skinning item leaves its inventory as its loot drops, the combat log names the leader's
unidentified ring, every creature that dies falls prone, and in `-131` the main character is struck unconscious (she
falls prone, the turn controller waits for her to get up at her next turn, and she wakes when the fight ends:
`UnitReturnToConsciousController`). Before this stream the sim reported each as unported (`-131`: all three runs invalid,
20 kinds; `-134`: the record and replay-1 invalid, replay-2 valid).

## Headline

| | Result |
|---|---|
| `poc5-tb-131/tb-lizard` | record, replay-1, replay-2 **valid** [M] (before: all three invalid, 20 kinds); T0 stream equal (2448 records), combat log 620/620 events, 157 ticks, 101 messages equal (78 tooltips), end state match rate 1.0000, flags equal, every unit's posture equal (21) |
| `poc5-tb-134/tb-lizard` | record, replay-1, replay-2 **valid** [M] (before: record and replay-1 invalid); log 631/631 events, 103 messages; postures equal |
| Other captures | every capture valid before stays valid [M]: `-95`, `-96`, `-109`, `-111`..`-117`, `-120`, `-122/tb-focus`, `-123`..`-127`, `-129`, `-132`, `-133`, `-136` (105 runs replayed, 102 valid; the three invalid are `-122/tb-nearest`, invalid before: a tie recorded before `init_ties`) |
| Merged `poc` (stream vision) | `-131` valid on the merged tree too: its 20 `HasLOS` differences (the memory's group, sim false, recording true) are gone [I: they followed from the leader's unrevived state after combat; the vision code is untouched] |

## The dropped loot [C]

`DropLootAndDestroyOnDeactivate.OnActivate` (the dismember buff the death effect attaches, `Phase::Fx`) calls
`UnitEntityData.DropLootToGround(dismember: true)` on a non-player unit with a lootable item (`HasLoot`):

1. `EntityCreationController.SpawnEntityView(prefab, ...)`: `Instantiate`, `Player.GetNewUniqueId()` names the view, then
   `SpawnEntityWithView`: `CreateEntityData` (the `DroppedLoot.EntityData`, whose parts the prefab's components build),
   `AttachView`, `AddEntity` (queued in `m_ToCreate`). The sim already took the id.
2. The view's scale (`GetScaleMultiplierBySize`) and a random yaw: `Quaternion.Euler(0, Random.Range(0, 360), 0)`, a draw
   (`sites::DROP_LOOT_ROTATION`; ported earlier).
3. `InteractionLootPart.SetUnit(blueprint)`.
4. Dismember: `IsDismember`, `BloodType`, then each item of the inventory that `NeedSkinningForCollect` leaves it:
   `Inventory.Remove(item)` = `ItemsCollection.Remove(item, count)` -> `Extract`, `OnItemsRemoved(item, count)`, which raises
   `IItemsCollectionHandler.HandleItemsRemoved` (ported earlier as a bus call; **nothing raised the combat log's event**).
5. `droppedLoot.Loot = Inventory` (the part's `Loot` setter sets `Enabled`), `DroppedBy = unit` (`IsDroppedByPlayer`),
   `m_DroppedLootEntityDataRef`, `LocalMapMarkerPart` (`IsRuntimeCreated`, `Type = Loot`, the unit's name).

The entity joins its state at the next `EntityCreationController.Tick` (`SceneEntitiesState.AddEntityData`, then
`IMapObjectHandler.HandleMapObjectSpawned` for a map object). The controller is registered after
`InspectUnitsController` and before `EntityDestructionController` and `GameLogController` (`GameModesFactory`).

### What is ported, and what is presentation

Ported (`combat/loot.rs`, `spawn/mod.rs`): the entity with its id, the rotation draw (a taped draw: a drop the sim
makes or misses shows in the draw stream), the skinning items' removal with its events (the combat log's, checked
against the recording), `Loot` = the unit's collection, `DroppedBy`, the unit's reference, and **the creation queue**:
the entity joins `Player.created_entities` (with the state's flag) in a new `Phase::Creation` of the frame
(`entity_creator_tick`, which also runs the queued units).

Presentation and spatial state, noted as a stub (`Coverage::stub`: listed in reports, doesn't invalidate): the prefab's
view and transform (scale, rotation, the position the save keeps), the parts' settings (`InteractionLootPart.Settings`,
`Enabled`, `LocalMapMarkerPart`), and what being a map object adds in a state (`RequiresSphereBoundsCollider`: the
bounds collider `EntityBoundsController` makes, which the fog of war's revealers use for `IsRevealed`, and the camera's
frustum controller). [C] Nothing the fights run reads them:
- `InteractionLootPart` is a global `IItemsCollectionHandler` subscriber, but acts only when `collection == Loot`: while
  the skinning items leave, its `Loot` is the placeholder `new ItemsCollection(Owner)`, and afterwards the unit's
  inventory, which changes only when the player loots (a player input the sim doesn't run);
- `HandleMapObjectSpawned`'s subscribers are the overtip managers (`OvertipManager`, `MapObjectOvertipsManager`,
  `OvertipsVM`); the marker is read by the local map's UI; `Enabled`, `LootContainerType` and the rest by interaction
  and the loot window; the reveal by overtips and highlights;
- the recorded end states carry the unit's `m_DroppedLootEntityDataRef`, not the entity.

**This is "not read by anything the fights run", not "irrelevant to saves"**: the entity's saved fields (position,
`IsRevealed`, the parts) are nowhere checked, and `created_entities` has no consumer yet. Before the sim writes a save of
such a state, or runs a loot interaction (clicking the bag, `MassLootHelper`), they must be computed or recorded: see
"For WrathOracle" below.

## The combat log's collection event [C, M]

`GameLogEventItemsCollection.EventsHandler` subscribes to `IItemsCollectionHandler` and queues an event for **every**
collection's add or removal; `ItemsCollectionLogThread` makes a message only for the player's inventory and a lootable
item that isn't in an additional limb's slot. The lizard's collection is its own: no message, but the event takes a
place among the frame's five (`MaxLogsInFrame`), so every later message of that frame is a frame late. [M] Adding the
event is the whole of the "messages #93..#99 one frame off, the queue counts 620 vs 618, 101 vs 100 messages" in
`-131`. The sim raises it in `ItemsCollection.OnItemAdded`/`OnItemsRemoved` (`game_log::items_collection`), the player's
inventory message is reported where reached. The check places a removal right after the bag's rotation draw.

## The unidentified item's name [C]

`ItemEntity.Name` = `Blueprint.NonIdentifiedName` while unidentified: the blueprint's own text when set, else
`DefaultNonIdentifiedName`: a weapon's or armour's type `DefaultName`, a shield's armour component's, a usable item's
`UsableItemTypeNames[Type]`, any other's `LocalizedTexts.UnidentifiedItemNames[ItemType]` (`ItemsFilter.ItemType` of the
blueprint's class: 15 values). The container is in `cache/sim/log-strings.json` already (an `EnumStrings` with a `Value`
per entry, the last entry with the value wins). The leader's ring (a Deflection bonus on her armour class) is named in
her attack tooltips' armour class breakdown.

## The prone controller [C]

`UnitProneController` (registered for Default, Dialog and Cutscene, after `UnitReturnToConsciousController`) ticks every
awake unit, dead ones included (`ShouldTickOnUnit` is true):

- `ShouldBeProne`: `Prone.ShouldBeActive`, or not conscious, or the Prone or Sleeping condition, or Unconscious.
- In turn-based combat an active posture's `Duration` is re-set first: passing (between turns) and in combat
  `3 s - TimeToNextTurn - 1 tick` (not below 0) minus the frame's delta; else the current unit's own `3 s` (acting and
  with a move action) or 0, minus the delta. `Seconds()` is `TimeSpan.FromSeconds`, whole milliseconds.
- An active posture that shouldn't be, in turn-based combat, of a directly controllable unit without commands stays
  (`flag2 = true`); a cutscene command on the unit forces `flag2 = false`.
- Active: `Duration += delta`; up after 3 s when it shouldn't be and `CanStandUp` (a cutscene, or not helpless, dazed or
  stunned): `HandleUnitWillGetUp`, `Active = false`, `Duration = 0`, `View.LeaveProneState`, `Wake(1)`.
- Not active and it should be (and isn't petrified): `Active = true`, `View.EnterProneState` (`IsProne`, `StopMoving`,
  `ObstaclesHelper.RemoveFromGroup`, `Wake(6)`), `Wake(1)`, `Commands.InterruptAll()`, `HandleUnitFallProne`.
- `ShouldBeActive = false`.

`HandleUnitWillGetUp`'s subscribers: the engagement controller (an attack of opportunity unless the unit gets up without
one) and the turn controller (`MoveAction += 3`, `TryCancelCommands`). `HandleUnitFallProne`: `AddFallProneTrigger`.

### The sim (`combat/prone.rs`, `combat/replay/prone.rs`)

`Unit` has `prone_active`, `prone_should_be_active` and `prone_duration` (ticks), hydrated from the save (a saved
posture is still reported at hydration: the controller runs only in the driven combat replay). `tick_unit` is the
controller's code for one unit but for `InterruptAll`. In the driven replay (on with the commands model) the controller runs unit by unit as the game's
`BaseUnitController.Tick` does, and a unit's whole sequence (its stop, its `InterruptAll`) is processed before the next unit's:
- the first recorded record of a unit's fall (the `ag_call Stop` from `UnitEntityView.EnterProneState`, or the `uc_x`
  `InterruptAll` by `UnitProneController.Tick`) ticks that unit then, in `Phase::Prone` (which runs nothing of its own); the
  unit must fall, so **the agent's `IsProne` read is checked against the sim's posture** instead of being an input;
- `Phase::ProneEnd` (before the next controller's first record, or at the frame's end) ticks the other awake units; each
  fall needs the recording's `InterruptAll` for that unit in that frame (run by the commands model where recorded), and a
  recorded call needs a fall (a fall of a unit without an agent has no stop record);
- a unit whose awake state the sleeping controller couldn't settle (only the window's last frame: no next awake set) is
  taken from the frame's own recorded set [I: its own sleeping controller's change is what that leaves out; counted as
  "last-frame units taken as asleep"], or, without a set, ticked on a copy and reported if that changes its posture;
- the end state's posture of every unit (`Active`, `ShouldBeActive`, `Duration`) is compared with the recording's
  (`compare::compare_prone`: a unit or posture missing on either side is a difference).
[I] No capture has two units falling in one frame: the per-unit design handles it by construction (round 2 of the
review found the first design, which ticked every unit at the phase's first record, would not), unverified by a fight.

Presentation: `EnterProneState`/`LeaveProneState`'s animation and `Wake` (the awake sets are recorded). Reported where
reached: a posture that should end in turn-based combat (the keeps-down branch needs `IsDirectlyControllable`), a
get-up (`HandleUnitWillGetUp`'s subscribers), a fall of a unit with an active `AddFallProneTrigger` that has actions.

## The return to consciousness [C]

`UnitReturnToConsciousController.Tick` (registered right before the prone controller): out of combat
(`Player.IsInCombat` false), every unit of `State.Units` that is not in combat, has no living enemy in its group's
memory (`Enemies.HasAlive()`: a reference that still resolves to a unit that is not dead), is not conscious and not
finally dead is made conscious: `Damage = HitPoints - max(1, CharacterLevel) - DamageNonLethal` (not below 0; the
Kineticist's burn healed first), attributes under 1 healed, negative levels removed, petrifying buffs removed,
`RemoveConditionAll(Petrified)`, `UnitLifeController.ForceUnitConscious` (`SetLifeState`: at death's door a bark; for the
conscious state `ReturnToStartingPositionIfNeeded` and `RemoveBuffsOnResurrect`; then `IUnitLifeStateChanged`, which the
combat log queues). The sim ports it (`life::make_unit_conscious`, run from the replay's memory driver, whose
`Memory.Enemies` reads were already checked); the damage write is a `mut damage` of rule 0 in the recording. The cases the
fights don't have are reported (the Kineticist, attributes, negative levels, petrifying buffs, a bark, a unit outside the
player's faction).

## For WrathOracle (observations to add; nothing here is recorded yet)

1. **`loot` (the dropped bag).** At `UnitEntityData.DropLootToGround`'s postfix: `loot_drop` with the unit, `dismember`,
   `overridePos`, the entity's `UniqueId`, the prefab and view names, the view's position, rotation and local scale, `IsDismember`,
   `BloodType`, `IsDroppedByPlayer`, the part list (class, `Enabled`; `InteractionLootPart`'s `LootContainerType`,
   `AddMapMarker`, `DestroyWhenEmpty`, `m_Unit`; `LocalMapMarkerPart`'s type and description) and the ids of the items that
   left the inventory; and at `EntityCreationController.Tick`, per created entity in order, `ent_created` with the id, class,
   state (main or cross-scene), the bounds collider's radius and `IsRevealed`. The sim would pull both: the drop's id,
   flags and removed items against its own, the creation in the frame and order it makes, the presentation fields taken as
   inputs. Then the stub in `DropLootToGround` becomes a checked input and the entity can be written to a save.
2. **`prone` (the posture).** At `UnitProneController.Tick`'s postfix, one `prone_step` per unit whose `Active` changed
   (`fall`, `getup`) or that took the keep-down branch (with the `IsDirectlyControllable` and `Commands.Empty` reads),
   with `Active`, `ShouldBeActive` before, `Duration` after; and `Duration` in `tt_prep`. Today the posture is checked at
   the end of the window only (every unit's `Duration` matched there), and the keep-down and get-up branches are reported
   when reached.
3. **The last frame's awake set.** The observer's `awake_set` for the tick after the window's last (the state its sleeping
   controller leaves): the sim takes the last frame's awake units from the frame's prefix today (`assumed_asleep`).
4. **`consciousness`.** `UnitReturnToConsciousController.MakeUnitConscious` is checked through its `mut damage` and the
   life-state event; its branches (the Kineticist, attributes under 1, negative levels, petrifying buffs, a bark) are
   reported, not recorded.

## Left

- The dropped bag's presentation and spatial state (see above) and the save writer's need for it.
- A unit that gets up (`HandleUnitWillGetUp`'s attack of opportunity and move cooldown), the keep-down branch's
  `IsDirectlyControllable`, `AddFallProneTrigger`'s actions; the view's `IsProne` is checked against the posture only where the
  agent reads it (a unit prone at the window's start is reported at hydration).
- `ItemsCollectionLogThread`'s messages for the player's inventory (looting, buying) and `InteractionLootPart`'s handler
  (a loot interaction).
- `created_entities` has no consumer (the save writer's, later).

## Results and tests [M]

- Replays: `target/release/wr-combat replay fixtures/captures/oracle/poc5-tb-131/fixtures/tb-lizard --run record` prints the
  `prone:` line (unit ticks, falls, get-ups, last-frame units taken as asleep) and `end-state postures (prone)`. `-131`:
  8 falls (the leader and seven creatures), `-134`: 7 (6 in its second replay's shorter fight), none gets up.
- `crates/wr-sim/tests/poc5_loot.rs` (ignored, local captures): `the_leaders_fall_and_the_lizards_loot_are_the_sims` (every run
  valid, log equal, exact fall counts, postures equal for every unit, the bag joined its state),
  `missing_or_wrong_loot_and_prone_records_are_reported` damages `-131`'s record 14 ways (the collection event dropped, its
  count, kind, item, duplicated, its caller, before the rotation draw, disabled; the prone controller's `InterruptAll` dropped,
  repeated, for another unit, retyped; `tt_prep`'s posture flipped both ways) and `a_damaged_end_posture_is_found`.
  Unit tests: the prone controller's cases (`combat/prone.rs`), the item type of every blueprint class, the collection event
  and its thread, `compare_prone`, the dropped bag's creation, the phase order.

Suites on the tree merged with `poc` (vision): the POC-5 capture suites through `scripts/heavy.sh` pass (36 of 36 as on `poc`) with
`poc5_loot`'s three (39 of 39); `scripts/check.sh` 659 of 659.

## Reviews (gpt-6.1-sol, high)

- Round 1, eight findings, fixed: the last frame's unknown-awake units (now taken from the frame's prefix set or reported), the
  view's `IsProne` unchecked (now the agent's read is compared with the sim's posture), the loot claim too broad (narrowed
  above), a missing posture passing the comparison, the test's fall counts, `AddFallProneTrigger` too broad, the collection
  event's rotation-draw check global (only for the drop's own removals), `HasAlive` not resolving the reference.
- Round 2, three: ticking every unit at the first recorded call broke two falls in one frame (now per unit, `Phase::ProneEnd`),
  `RemoveConditionAll(Petrified)` made unconditional, an oracle unit without a posture still passing.
- Round 3, two: recorded falls now follow the awake list's eligibility and order; a subscriber of an items raise that made
  records is reported (the log's event is queued last, the game's order is subscription order).
- Round 4, one: the order is checked for every recorded prone record, not only a unit's first (a unit's visit is finished before
  the next's). No finding left after it was fixed (not re-reviewed beyond this).
