# RNG call sites (WrathOracle 0.2)

Generated 2026-10-11 by `oracle/tools/oracle-rng-table` from the runtime site table of the certification run (`rng-sites.json`, the IL scan of every game assembly in `Managed/`) joined with a static classification of the decompiled call sites by gpt-6.1-sol (category and whether the value can affect T0 state: yes / maybe / no; `?` = no static row, mostly third-party assemblies that weren't decompiled). Coverage: **transpiled** = the call is replaced by a recording wrapper; **global hook** = in a generic method, recorded by the prefix on the managed `Random` member; **gap-detected only** = in a generic method calling an extern-only member (`value`, `Range(float,float)`), not interceptable, counted by the generator-state gap check. Class: **sim** draws are replayed from the tape, **pres** (presentation) draws are recorded but left out of the tape and of trace comparison. Draws: number of draws at the site in the three certification recordings. See `oracle.md` for the method.

| Category | Sites | Transpiled | Global hook only | Gap-detected only | Class sim / pres | T0 yes / maybe / no |
|---|---|---|---|---|---|---|
| ai | 21 | 21 | 0 | 0 | 21 / 0 | 21 / 0 / 0 |
| anim-variant | 28 | 28 | 0 | 0 | 12 / 16 | 1 / 9 / 18 |
| attack-sequence | 2 | 2 | 0 | 0 | 2 / 0 | 2 / 0 / 0 |
| audio | 1 | 1 | 0 | 0 | 0 / 1 | 0 / 0 / 1 |
| camera | 3 | 3 | 0 | 0 | 0 / 3 | 0 / 0 / 3 |
| cutscene | 3 | 3 | 0 | 0 | 3 / 0 | 3 / 0 / 0 |
| dialogue | 9 | 9 | 0 | 0 | 2 / 7 | 2 / 0 / 7 |
| dice | 2 | 2 | 0 | 0 | 2 / 0 | 2 / 0 / 0 |
| procedural | 12 | 12 | 0 | 0 | 12 / 0 | 7 / 5 / 0 |
| qa-editor | 10 | 10 | 0 | 0 | 4 / 6 | 3 / 0 / 7 |
| rules-direct | 27 | 27 | 0 | 0 | 27 / 0 | 26 / 1 / 0 |
| spawn | 28 | 28 | 0 | 0 | 27 / 1 | 27 / 1 / 0 |
| ui | 2 | 2 | 0 | 0 | 0 / 2 | 0 / 0 / 2 |
| unclassified | 13 | 11 | 1 | 1 | 6 / 7 | 0 / 0 / 0 |
| visual | 97 | 97 | 0 | 0 | 26 / 71 | 1 / 8 / 88 |
| wrapper | 22 | 10 | 8 | 4 | 22 / 0 | 11 / 11 / 0 |

| Category | Type | Method | Member | Coverage | Class | T0 | Draws in the given traces |
|---|---|---|---|---|---|---|---|
| ai | Kingmaker.AI.AiBrainController | SelectAction(UnitEntityData,DecisionContext,AIDebugScope) | Range(float,float) | transpiled | sim | yes | 9 |
| ai | Kingmaker.AI.Blueprints.BlueprintAiCastSpell | CreateAbilityData(UnitDescriptor,Ability,BlueprintSpellbook) | Range(int,int) | transpiled | sim | yes | 0 |
| ai | Kingmaker.AI.Blueprints.BlueprintAiCastSpell | CreateAbilityData(UnitDescriptor,Ability,BlueprintSpellbook) | Range(int,int) | transpiled | sim | yes | 0 |
| ai | Kingmaker.AI.Blueprints.BlueprintAiRandomMove | CalculateDestinationPosition(DecisionContext) | get_insideUnitCircle | transpiled | sim | yes | 0 |
| ai | Kingmaker.AI.Blueprints.Considerations.RandomConsideration | Score(DecisionContext) | Range(float,float) | transpiled | sim | yes | 0 |
| ai | Kingmaker.Controllers.UnitImpatienceController | SetupPriorityTarget(UnitEntityData) | Range(float,float) | transpiled | sim | yes | 0 |
| ai | Kingmaker.Controllers.UnitImpatienceController | UpdatePriorityTarget(UnitEntityData) | Range(int,int) | transpiled | sim | yes | 0 |
| ai | Kingmaker.Controllers.Units.FollowersFormationController | PrepareFormation(UnitPartFollowedByUnits,IList`1,Vector3,Dictionary`2) | Range(float,float) | transpiled | sim | yes | 0 |
| ai | Kingmaker.Controllers.Units.SummonedUnitsController | TickUnitFollowMaster(UnitEntityData,UnitPartSummonedMonster) | Range(float,float) | transpiled | sim | yes | 0 |
| ai | Kingmaker.Controllers.Units.SummonedUnitsController | TickUnitFollowMaster(UnitEntityData,UnitPartSummonedMonster) | Range(int,int) | transpiled | sim | yes | 0 |
| ai | Kingmaker.Controllers.Units.UnitFearController | GetPointForMove(UnitEntityData) | Range(int,int) | transpiled | sim | yes | 0 |
| ai | Kingmaker.Controllers.Units.UnitFleeOrApproachController | GetPointForFleeMove(UnitEntityData,UnitEntityData) | Range(int,int) | transpiled | sim | yes | 0 |
| ai | Kingmaker.Controllers.Units.UnitFleeOrApproachController | GetPointForFleeMove(UnitEntityData,UnitEntityData) | Range(int,int) | transpiled | sim | yes | 0 |
| ai | Kingmaker.Controllers.Units.UnitInPitController | TickOnUnit(UnitEntityData) | get_onUnitSphere | transpiled | sim | yes | 0 |
| ai | Kingmaker.Controllers.Units.UnitInPitController | TickOnUnit(UnitEntityData) | get_onUnitSphere | transpiled | sim | yes | 0 |
| ai | Kingmaker.Controllers.Units.UnitInPitController | TickOnUnit(UnitEntityData) | get_onUnitSphere | transpiled | sim | yes | 0 |
| ai | Kingmaker.Controllers.Units.UnitRoamingController | RollNextPoint(UnitPartRoaming) | Range(float,float) | transpiled | sim | yes | 3 |
| ai | Kingmaker.Controllers.Units.UnitRoamingController | RollNextPoint(UnitPartRoaming) | get_insideUnitCircle | transpiled | sim | yes | 3 |
| ai | Kingmaker.Controllers.Units.UnitRunBackController | GetPointForMove(UnitPartRunBack) | Range(int,int) | transpiled | sim | yes | 0 |
| ai | Kingmaker.View.Roaming.RoamingWaypointData | SelectIdleTime() | Range(float,float) | transpiled | sim | yes | 0 |
| ai | Kingmaker.View.UnitMovementAgent | SlowDown(Single&) | Range(float,float) | transpiled | sim | yes | 9 |
| anim-variant | AnimatorControllerSpeedMultiplyer | OnEnable() | Range(float,float) | transpiled | pres | maybe | 0 |
| anim-variant | AnimatorControllerSpeedMultiplyer | OnEnable() | Range(float,float) | transpiled | pres | maybe | 0 |
| anim-variant | AnimatorControllerSpeedMultiplyer | OnEnable() | Range(float,float) | transpiled | pres | maybe | 0 |
| anim-variant | AnimatorControllerSpeedMultiplyer | OnEnable() | Range(float,float) | transpiled | pres | maybe | 0 |
| anim-variant | AnimatorControllerSpeedMultiplyer | OnEnable() | Range(int,int) | transpiled | pres | maybe | 0 |
| anim-variant | AnimatorControllerSpeedMultiplyer | OnEnable() | get_value | transpiled | pres | maybe | 0 |
| anim-variant | Kingmaker.AreaLogic.AlushenyrraIsles.SMBRandomizer | OnStateEnter(Animator,AnimatorStateInfo,Int32) | Range(int,int) | transpiled | sim | maybe | 0 |
| anim-variant | Kingmaker.AreaLogic.AlushenyrraIsles.SMBRandomizer | OnStateMachineEnter(Animator,Int32) | Range(int,int) | transpiled | sim | maybe | 0 |
| anim-variant | Kingmaker.Controllers.Units.UnitAnimationController | Activate() | Range(float,float) | transpiled | sim | maybe | 24 |
| anim-variant | Kingmaker.UnitLogic.Commands.AttackHandInfo | CreateAnimationHandleForAttack(IEnumerable`1,Boolean) | Range(int,int) | transpiled | sim | yes | 23 |
| anim-variant | Kingmaker.Visual.Animation.Kingmaker.Actions.UnitAnimationActionLocoMotion | OnStart(UnitAnimationActionHandle) | get_value | transpiled | sim | no | 0 |
| anim-variant | Kingmaker.Visual.Animation.Kingmaker.Actions.UnitAnimationActionLocoMotionHuman | UpdateCurrentClips(UnitAnimationActionHandle) | get_value | transpiled | sim | no | 6 |
| anim-variant | Kingmaker.Visual.Animation.Kingmaker.Actions.UnitAnimationActionMicroIdle | OnStart(UnitAnimationActionHandle) | Range(int,int) | transpiled | sim | no | 0 |
| anim-variant | Kingmaker.Visual.Animation.Kingmaker.Actions.UnitAnimationActionVariantIdle | OnStart(UnitAnimationActionHandle) | Range(int,int) | transpiled | sim | no | 0 |
| anim-variant | Kingmaker.Visual.Animation.Kingmaker.Actions.UnitAnimationActionVariantIdle | OnStart(UnitAnimationActionHandle) | get_value | transpiled | sim | no | 0 |
| anim-variant | Kingmaker.Visual.Animation.Kingmaker.Actions.UnitAnimationActionVariantIdle | OnStart(UnitAnimationActionHandle) | get_value | transpiled | sim | no | 0 |
| anim-variant | Kingmaker.Visual.Animation.Kingmaker.Actions.UnitAnimationActionVariantIdle | OnStart(UnitAnimationActionHandle) | get_value | transpiled | sim | no | 0 |
| anim-variant | Kingmaker.Visual.Animation.Kingmaker.UnitAnimationManager | TickIdleVariants() | Range(float,float) | transpiled | sim | no | 0 |
| anim-variant | Kingmaker.Visual.Critters.Familiar | StartIdle() | Range(int,int) | transpiled | pres | no | 0 |
| anim-variant | Kingmaker.Visual.Critters.Rabbit+<Think>d__22 | MoveNext() | Range(float,float) | transpiled | pres | no | 0 |
| anim-variant | Kingmaker.Visual.Critters.Rabbit+<Think>d__22 | MoveNext() | Range(float,float) | transpiled | pres | no | 6 |
| anim-variant | Kingmaker.Visual.Critters.Rabbit+<Think>d__22 | MoveNext() | Range(float,float) | transpiled | pres | no | 8 |
| anim-variant | Kingmaker.Visual.Critters.Rabbit+<Think>d__22 | MoveNext() | Range(float,float) | transpiled | pres | no | 8 |
| anim-variant | Kingmaker.Visual.Critters.Rabbit+<Think>d__22 | MoveNext() | Range(int,int) | transpiled | pres | no | 0 |
| anim-variant | Kingmaker.Visual.Critters.Rabbit+<Think>d__22 | MoveNext() | Range(int,int) | transpiled | pres | no | 6 |
| anim-variant | Kingmaker.Visual.Critters.Rabbit+<Think>d__22 | MoveNext() | get_insideUnitCircle | transpiled | pres | no | 8 |
| anim-variant | Kingmaker.Visual.Critters.Rabbit+<Think>d__22 | MoveNext() | get_value | transpiled | pres | no | 14 |
| anim-variant | LanternKingExpressionsRandom | Update() | Range(int,int) | transpiled | pres | no | 0 |
| attack-sequence | Kingmaker.UnitLogic.Commands.Base.UnitCommand | TickApproaching() | Range(float,float) | transpiled | sim | yes | 790 |
| attack-sequence | Kingmaker.UnitLogic.Commands.UnitAttack | TryToReorder(AnimationSequence,List`1,Int32) | Range(float,float) | transpiled | sim | yes | 40 |
| audio | AkObstructionOcclusion | InitIntervalsAndFadeRates() | Range(float,float) | transpiled | pres | no | 0 |
| camera | Kingmaker.View.ShakeSettings | Tick(Single) | get_insideUnitSphere | transpiled | pres | no | 595 |
| camera | Kingmaker.Visual.CameraShakeFx | OnEnable() | get_value | transpiled | pres | no | 32 |
| camera | Kingmaker.Visual.CameraShakeFx | OnEnable() | get_value | transpiled | pres | no | 32 |
| cutscene | Kingmaker.AreaLogic.Cutscenes.Commands.CommandDelay | OnRun(CutscenePlayerData,Boolean) | Range(float,float) | transpiled | sim | yes | 6 |
| cutscene | Kingmaker.AreaLogic.Cutscenes.Commands.CommandUnitPlayCutsceneAnimation | OnRun(CutscenePlayerData,CutscenePlayerTrackData,Boolean) | Range(float,float) | transpiled | sim | yes | 0 |
| cutscene | Kingmaker.AreaLogic.Cutscenes.CutscenePlayerGateData | SelectTrackToStart() | Range(int,int) | transpiled | sim | yes | 0 |
| dialogue | Kingmaker.DialogSystem.CharacterSelection | SelectUnit(BlueprintAnswer,UnitEntityData,Boolean) | Range(int,int) | transpiled | sim | yes | 0 |
| dialogue | Kingmaker.DialogSystem.CueSelection | Select() | Range(int,int) | transpiled | sim | yes | 0 |
| dialogue | Kingmaker.Visual.Sound.SoundState | PlayRemindingFatiguedAsk() | Range(float,float) | transpiled | pres | no | 3 |
| dialogue | Kingmaker.Visual.Sound.SoundState | SetState() | Range(int,int) | transpiled | pres | no | 0 |
| dialogue | Kingmaker.Visual.Sound.SoundState | SetState() | Range(int,int) | transpiled | pres | no | 0 |
| dialogue | Kingmaker.Visual.Sound.UnitAsksComponent+AnimationBark | CheckBarkChance(Single) | get_value | transpiled | pres | no | 0 |
| dialogue | Kingmaker.Visual.Sound.UnitAsksComponent+AnimationBark | CheckBarkChance(Single) | get_value | transpiled | pres | no | 16 |
| dialogue | Kingmaker.Visual.Sound.UnitAsksComponent+Bark | CheckBarkChance(Single) | get_value | transpiled | pres | no | 6 |
| dialogue | Kingmaker.Visual.Sound.UnitAsksComponent+Bark | Schedule() | Range(float,float) | transpiled | pres | no | 16 |
| dice | Kingmaker.RuleSystem.RulebookEvent+Dice | D(Int32,DiceType) | Range(int,int) | transpiled | sim | yes | 37 |
| dice | Kingmaker.RuleSystem.Rules.RuleSkillCheck | OnTrigger(RulebookEventContext) | Range(int,int) | transpiled | sim | yes | 2 |
| procedural | DungeonArchitect.BarbarianDungeonBuilder | BuildDungeon(DungeonConfig,DungeonModel) | get_value | transpiled | sim | yes | 0 |
| procedural | DungeonArchitect.TransformationRuleRandomPositionRotationScale | GetTransform(PropSocket,DungeonModel,Matrix4x4,Random,Vector3&,Quaternion&,Vector3&) | Range(float,float) | transpiled | sim | maybe | 0 |
| procedural | DungeonArchitect.TransformationRuleRandomPositionRotationScale | GetTransform(PropSocket,DungeonModel,Matrix4x4,Random,Vector3&,Quaternion&,Vector3&) | Range(float,float) | transpiled | sim | maybe | 0 |
| procedural | DungeonArchitect.TransformationRuleRandomPositionRotationScale | GetTransform(PropSocket,DungeonModel,Matrix4x4,Random,Vector3&,Quaternion&,Vector3&) | Range(float,float) | transpiled | sim | maybe | 0 |
| procedural | DungeonArchitect.TransformationRuleRandomPositionRotationScale | GetTransform(PropSocket,DungeonModel,Matrix4x4,Random,Vector3&,Quaternion&,Vector3&) | Range(float,float) | transpiled | sim | maybe | 0 |
| procedural | DungeonArchitect.TransformationRuleRandomPositionRotationScale | GetTransform(PropSocket,DungeonModel,Matrix4x4,Random,Vector3&,Quaternion&,Vector3&) | Range(float,float) | transpiled | sim | maybe | 0 |
| procedural | Kingmaker.Armies.TacticalCombat.Controllers.TacticalCombatController | Setup() | Range(int,int) | transpiled | sim | yes | 0 |
| procedural | Kingmaker.Armies.TacticalCombat.Controllers.TacticalCombatGridController | HandleTacticalCombatPrepare(Boolean) | InitState | transpiled | sim | yes | 0 |
| procedural | Kingmaker.Armies.TacticalCombat.Controllers.TacticalCombatGridController | HandleTacticalCombatPrepare(Boolean) | get_state | transpiled | sim | yes | 0 |
| procedural | Kingmaker.Armies.TacticalCombat.Controllers.TacticalCombatGridController | HandleTacticalCombatPrepare(Boolean) | set_state | transpiled | sim | yes | 0 |
| procedural | Kingmaker.Dungeon.DungeonController | MoveToIsland(DungeonIslandState) | get_value | transpiled | sim | yes | 0 |
| procedural | Kingmaker.Plugins.DungeonArchitect.DLC3.Scripts.CaveDungeonBuilder | BuildDungeon(DungeonConfig,DungeonModel) | get_value | transpiled | sim | yes | 0 |
| qa-editor | Kingmaker.Cheats.Logic.FPSFreezer | Update() | Range(int,int) | transpiled | sim | no | 0 |
| qa-editor | Kingmaker.Controllers.Dialog.DebugDialogPlayer+PositionsSelector | PlaceUnit(UnitEntityData) | get_insideUnitCircle | transpiled | sim | yes | 0 |
| qa-editor | Kingmaker.Visual.Critters.SillyCheatCritterSprinkler+<EnableRabbit>d__10 | MoveNext() | Range(float,float) | transpiled | pres | no | 0 |
| qa-editor | Kingmaker.Visual.Critters.SillyCheatCritterSprinkler+<SpawnCritters>d__9 | MoveNext() | Range(float,float) | transpiled | pres | no | 0 |
| qa-editor | Kingmaker.Visual.Critters.SillyCheatCritterSprinkler+<SpawnCritters>d__9 | MoveNext() | Range(float,float) | transpiled | pres | no | 0 |
| qa-editor | Kingmaker.Visual.Critters.SillyCheatCritterSprinkler+<SpawnCritters>d__9 | MoveNext() | Range(int,int) | transpiled | pres | no | 0 |
| qa-editor | Kingmaker.Visual.Critters.SillyCheatCritterSprinkler+<SpawnCritters>d__9 | MoveNext() | Range(int,int) | transpiled | pres | no | 0 |
| qa-editor | Kingmaker.Visual.HitSystem.HitPlayer | PlayDamageHit(RuleDealDamage) | get_value | transpiled | pres | no | 0 |
| qa-editor | Pathfinding.GraphModifier | Reset() | Range(int,int) | transpiled | sim | yes | 0 |
| qa-editor | Pathfinding.GraphModifier | Reset() | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Armies.TacticalCombat.Components.RandomLeaderSpellReplacement | OnEventAboutToTrigger(RuleCastSpell) | get_value | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Armies.TacticalCombat.Components.RunActionOnTurnStart | HandleNextTurn(Int32,UnitEntityData) | get_value | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Armies.TacticalCombat.Parts.UnitPartTacticalMorale | IsSuccess(Single) | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Controllers.Clicks.Handlers.ClickWithSelectedAbilityHandler | TryUseServiceCaster(TargetWrapper) | get_insideUnitCircle | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Controllers.InclemencyController | GetNextInclemency() | Range(float,float) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Designers.EventConditionActionSystem.Actions.RandomAction | RunAction() | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Designers.EventConditionActionSystem.Evaluators.RandomPosition | GetValueInternal() | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Designers.Mechanics.Facts.ModifyD20 | OnEventAboutToTrigger(RuleRollD20) | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.EntitySystem.Stats.ModifiableValueArmorClass | SelectMissReason(Boolean,Boolean) | Range(int,int) | transpiled | sim | maybe | 4 |
| rules-direct | Kingmaker.Kingdom.Actions.KingdomActionModifyStatRandom | RunAction() | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Kingdom.Armies.ArmyDismissManager | CreateArmyCompensationForDismissal(BlueprintGlobalMapPoint,Boolean) | Range(float,float) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Kingdom.CrusadeEventsTimeline | TryStartEvent() | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Kingdom.KingdomTimelineManager | RollRandomEvents() | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Kingdom.KingdomTimelineManager | RollRandomEvents() | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.UnitLogic.Abilities.Components.AbilitySillyFeed+<Deliver>d__1 | MoveNext() | get_insideUnitCircle | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.UnitLogic.Mechanics.Actions.ContextActionAddRandomFeatureFromList | RunAction() | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.UnitLogic.Mechanics.Actions.ContextActionBreathOfMoney | RunAction() | Range(float,float) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.UnitLogic.Mechanics.Actions.ContextActionBreathOfMoney | RunAction() | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.UnitLogic.Mechanics.Actions.ContextActionBreathOfMoney | RunAction() | get_insideUnitSphere | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.UnitLogic.Mechanics.Actions.ContextActionRandomize | RunAction() | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.UnitLogic.Parts.UnitPartMirrorImage | TryAbsorbHit(Boolean) | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.UnitLogic.Parts.UnitPartSpotaneousSpellSaving | IsSaved() | Range(float,float) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.View.MapObjects.Traps.Simple.SimpleTrapObjectView | CreateData() | Range(int,int) | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Visual.WeatherSystem.WeatherMechanicController+<GetTargets>d__15 | MoveNext() | get_value | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Visual.WeatherSystem.WeatherMechanicController+<GetTargets>d__15 | MoveNext() | get_value | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Visual.WeatherSystem.WeatherMechanicController+<GetTargets>d__15 | MoveNext() | get_value | transpiled | sim | yes | 0 |
| rules-direct | Kingmaker.Visual.WeatherSystem.WeatherMechanicController+<GetTargets>d__15 | MoveNext() | get_value | transpiled | sim | yes | 0 |
| spawn | Dreamteck.Splines.ObjectController | InstantiateSingle() | Range(int,int) | transpiled | pres | maybe | 0 |
| spawn | Kingmaker.Assets.Controllers.GlobalMap.RandomEncountersController | RollCombatOrSe(Vector3) | Range(float,float) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.Assets.Controllers.GlobalMap.RandomEncountersController | StartEncounter(BlueprintRandomEncounter,Nullable`1,RestStatus,Boolean) | Range(int,int) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.Blueprints.Loot.LootItemsPackVariable | AddItemsTo(List`1) | Range(int,int) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.Blueprints.Loot.LootRandomItem | AddItemsTo(List`1) | Range(int,int) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.Blueprints.OverrideLegacyUnits | TryGetOverrideGuid(String,String&) | Range(int,int) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.Controllers.Rest.RestController | RollEncounter(CampingRoot,Int32&,TimeSpan&) | Range(int,int) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.Controllers.Rest.RestController | RollEncounter(CampingRoot,Int32&,TimeSpan&) | Range(int,int) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.Controllers.Rest.RestController | TryRollSpecialEncounter(CampingState) | Range(float,float) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.Controllers.Rest.RestController | TryRollSpecialEncounter(CampingState) | Range(int,int) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.EntitySystem.Entities.UnitEntityData | DropLootToGround(Boolean,Nullable`1,Boolean) | Range(float,float) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.EntitySystem.Entities.UnitEntityData | TranslocatePetsClose() | get_insideUnitCircle | transpiled | sim | yes | 0 |
| spawn | Kingmaker.Kingdom.Artisans.Artisan | CollectExtraGiftWithTier(ItemsCollection,Int32) | Range(int,int) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.Kingdom.Artisans.Artisan | ScheduleProoduction(Boolean,ArtisanItemDeck) | Range(int,int) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.Kingdom.Artisans.Artisan | ScheduleProoduction(Boolean,ArtisanItemDeck) | Range(int,int) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.Kingdom.Artisans.Artisan | ScheduleProoduction(Boolean,ArtisanItemDeck) | get_value | transpiled | sim | yes | 0 |
| spawn | Kingmaker.Kingdom.Blueprints.ArmyRoot | SummonTravellingArmy(GlobalMapState) | Range(int,int) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.RandomEncounters.RandomEncounterUnitSelector | PlaceUnitsInCamp(IList`1,Vector3,CampingRole) | Range(float,float) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.RandomEncounters.RandomEncounterUnitSelector | PlaceUnitsInCamp(IList`1,Vector3,CampingRole) | Range(float,float) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.RandomEncounters.RandomEncounterUnitSelector | PlaceUnitsInCamp(IList`1,Vector3,CampingRole) | Range(float,float) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.RandomEncounters.RandomEncounterUnitSelector | PlaceUnitsInCamp(IList`1,Vector3,CampingRole) | get_value | transpiled | sim | yes | 0 |
| spawn | Kingmaker.RandomEncounters.RandomEncounterUnitSelector | PlaceUnitsOutsideOfCamp(IList`1,Vector3) | Range(float,float) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.RandomEncounters.RandomEncounterUnitSelector | PlaceUnitsOutsideOfCamp(IList`1,Vector3) | Range(float,float) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.RandomEncounters.RandomEncounterUnitSelector | PlaceUnitsOutsideOfCamp(IList`1,Vector3) | Range(float,float) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.RandomEncounters.RandomEncounterUnitSelector | SelectUnits(Int32) | Range(int,int) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.RandomEncounters.Settings.REVendorChapter | TryRoll() | Range(float,float) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.UnitLogic.Customization.UnitCustomizationPreset | SelectLeftHanded() | Range(float,float) | transpiled | sim | yes | 0 |
| spawn | Kingmaker.View.AreaEnterPoint | PositionCharactersByPreset() | get_insideUnitCircle | transpiled | sim | yes | 0 |
| ui | Kingmaker.UI.LoadingScreen.LoadingScreenHints | TakeHint() | Range(int,int) | transpiled | pres | no | 0 |
| ui | Kingmaker.UI.MVVM._PCView.ServiceWindows.Spellbook.Metamagic.SpellbookColorEntityPCView | BindViewImplementation() | Range(int,int) | transpiled | pres | no | 0 |
| unclassified | DG.Tweening.DOTween | Shake(DOGetter`1,DOSetter`1,Single,Vector3,Int32,Single,Boolean,Boolean,Boolean) | Range(float,float) | transpiled | pres | ? | 0 |
| unclassified | DG.Tweening.DOTween | Shake(DOGetter`1,DOSetter`1,Single,Vector3,Int32,Single,Boolean,Boolean,Boolean) | Range(float,float) | transpiled | pres | ? | 0 |
| unclassified | DG.Tweening.DOTween | Shake(DOGetter`1,DOSetter`1,Single,Vector3,Int32,Single,Boolean,Boolean,Boolean) | Range(float,float) | transpiled | pres | ? | 0 |
| unclassified | DG.Tweening.DOTween | Shake(DOGetter`1,DOSetter`1,Single,Vector3,Int32,Single,Boolean,Boolean,Boolean) | Range(float,float) | transpiled | pres | ? | 0 |
| unclassified | DG.Tweening.Plugins.StringPluginExtensions | AppendScrambledChars(StringBuilder,Int32,Char[]) | Range(int,int) | transpiled | pres | ? | 0 |
| unclassified | DG.Tweening.Plugins.StringPluginExtensions | ScrambleChars(Char[]) | Range(int,int) | transpiled | pres | ? | 0 |
| unclassified | DungeonArchitect.Builders.Grid.GridDungeonQuery | GetRandomCell() | Range(int,int) | transpiled | sim | ? | 0 |
| unclassified | DungeonArchitect.Builders.SimpleCity.LandscapeTransformerCity | UpdateBaseTexture(SimpleCityDungeonModel,Single[,,]) | get_value | transpiled | sim | ? | 0 |
| unclassified | DungeonArchitect.Dungeon | RandomizeSeed() | get_value | transpiled | sim | ? | 0 |
| unclassified | DungeonArchitect.RoomDesigner.DungeonRoomDesigner | DebugDrawGizmos() | ColorHSV/0 | transpiled | sim | ? | 0 |
| unclassified | Kingmaker.Visual.Animation.Kingmaker.Actions.TimedProbabilityCurve+Tracker | .ctor(TimedProbabilityCurve) | get_value | transpiled | sim | ? | 0 |
| unclassified | Kingmaker.Visual.WeatherSystem.WeatherMinMaxRateSpawnController`1 | Update(Camera,Single,Vector2,Single) | get_value | gap-detected only | sim | ? | 0 |
| unclassified | Rewired.Utils.Classes.Data.AList`1 | GetRandom() | Range(int,int) | global hook | pres | ? | 0 |
| visual | Dreamteck.Splines.ParticleController | OnParticleBorn(Int32) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Dreamteck.Splines.ParticleController | OnParticleBorn(Int32) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Dreamteck.Splines.ParticleController | OnParticleBorn(Int32) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Dreamteck.Splines.ParticleController | OnParticleBorn(Int32) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Dreamteck.Splines.ParticleController | OnParticleBorn(Int32) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Dreamteck.Splines.ParticleController | OnParticleBorn(Int32) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Dreamteck.Splines.ParticleController | OnParticleBorn(Int32) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Dreamteck.Splines.ParticleController | OnParticleBorn(Int32) | get_insideUnitCircle | transpiled | pres | no | 0 |
| visual | Dreamteck.Splines.ParticleController | OnParticleBorn(Int32) | get_insideUnitCircle | transpiled | pres | no | 0 |
| visual | DungeonArchitect.LightMarker | RandomizeLightColor() | Range(int,int) | transpiled | sim | no | 0 |
| visual | Kingmaker.Assets.Visual.FlickeringLight | Update() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Assets.Visual.FlickeringLight | Update() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Assets.Visual.FlickeringLight | Update() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | AddForceToArrrow() | Range(int,int) | transpiled | sim | no | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | AddForceToArrrow() | Range(int,int) | transpiled | sim | no | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | AddForceToArrrow() | Range(int,int) | transpiled | sim | no | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | AddForceToArrrow() | Range(int,int) | transpiled | sim | no | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | AddForceToArrrow() | get_value | transpiled | sim | no | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | CalculateMissTarget() | Range(float,float) | transpiled | sim | maybe | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | CalculateMissTarget() | Range(float,float) | transpiled | sim | maybe | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | CalculateMissTarget() | Range(int,int) | transpiled | sim | maybe | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | CalculateMissTarget() | get_insideUnitCircle | transpiled | sim | maybe | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | CalculateMissTarget() | get_value | transpiled | sim | maybe | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | HandleArmorHit() | Range(int,int) | transpiled | sim | no | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | HandleShieldHit() | Range(int,int) | transpiled | sim | no | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | HandleShieldHit() | get_value | transpiled | sim | no | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | SetShieldTarget() | Range(float,float) | transpiled | sim | maybe | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | SetShieldTarget() | Range(float,float) | transpiled | sim | maybe | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | SetShieldTarget() | Range(float,float) | transpiled | sim | maybe | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | calcForce(Vector3,Single,Single) | Range(float,float) | transpiled | sim | no | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | calcTorque(Single,Single) | Range(float,float) | transpiled | sim | no | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | calcTorque(Single,Single) | Range(float,float) | transpiled | sim | no | 0 |
| visual | Kingmaker.Controllers.Projectiles.Projectile | calcTorque(Single,Single) | Range(int,int) | transpiled | sim | no | 0 |
| visual | Kingmaker.Utility.ParticleSystemDestroyer+<Start>d__5 | MoveNext() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.View.Spawners.Projectile.ProjectileSpawner | CalculateSpawnRate(Single) | get_value | transpiled | sim | no | 0 |
| visual | Kingmaker.View.SwarmBehaviour | PlayDeath(GameObject,SwarmDeathType) | Range(float,float) | transpiled | sim | no | 0 |
| visual | Kingmaker.Visual.CharactersRigidbody.RigidbodyCreatureController | ApplyImpulseDirectly(Vector3,Single) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.CharactersRigidbody.RigidbodyCreatureController | ApplyImpulseDirectly(Vector3,Single) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.CharactersRigidbody.RigidbodyCreatureController | ApplyImpulseDirectly(Vector3,Single) | Range(int,int) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.CharactersRigidbody.RigidbodyCreatureController | ApplyImpulseDirectly(Vector3,Single) | get_value | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Critters.Birds+<Fly>d__6 | MoveNext() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Critters.Birds+<Fly>d__6 | MoveNext() | Range(int,int) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Critters.Familiar | FindFreePlaceNearPoint(Vector3) | Range(int,int) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Critters.Familiar | TryStartRoaming() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Critters.Familiar | TryStartRoaming() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Critters.Familiar | TryStartRoaming() | Range(int,int) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Critters.Familiar | Update() | Range(int,int) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Critters.Rabbit | MaybeRunAway() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Particles.ArrowDeflectorSurface | ApplyDeflectImpulse(Rigidbody,Vector3) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Particles.ArrowDeflectorSurface | ApplyDeflectImpulse(Rigidbody,Vector3) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Particles.ArrowDeflectorSurface | ApplyDeflectImpulse(Rigidbody,Vector3) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Particles.ArrowDeflectorSurface | ApplyDeflectImpulse(Rigidbody,Vector3) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Particles.ArrowDeflectorSurface | GetRandomArrowLocalPosition() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Particles.ArrowDeflectorSurface | GetRandomArrowLocalPosition() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Particles.ArrowDeflectorSurface | GetRandomArrowLocalPosition() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Particles.MirrorImageFX | Init(UnitEntityView) | Range(int,int) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Particles.ParticlesMaterialController | DoUpdate() | get_value | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Particles.ParticlesMaterialController | DoUpdate() | get_value | transpiled | pres | no | 1732 |
| visual | Kingmaker.Visual.RandomRotationOnStart | Rotate() | Range(float,float) | transpiled | pres | no | 7 |
| visual | Kingmaker.Visual.RandomRotationOnStart | Rotate() | Range(float,float) | transpiled | pres | no | 7 |
| visual | Kingmaker.Visual.RandomRotationOnStart | Rotate() | Range(float,float) | transpiled | pres | no | 7 |
| visual | Kingmaker.Visual.RandomScaleOnStart | Scale() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.RandomScaleOnStart | Scale() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.RandomScaleOnStart | Scale() | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Trails.InstancedCompositeTrailsManager | RequestInstance(InstancedCompositeTrail) | Range(int,int) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Trails.TrailEmitter | GetVelocity(CompositeTrailRenderer) | get_onUnitSphere | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Trails.TrailEmitter | Randomize(CompositeTrailRenderer) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Trails.TrailEmitter | Randomize(CompositeTrailRenderer) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Trails.TrailEmitter | Randomize(CompositeTrailRenderer) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Trails.TrailEmitter | Randomize(CompositeTrailRenderer) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.Trails.TrailEmitter | Randomize(CompositeTrailRenderer) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Kingmaker.Visual.WeatherSystem.WeatherLightningBoltController | Spawn(Camera,Single,Vector2,Single) | get_value | transpiled | sim | no | 0 |
| visual | Kingmaker.Visual.WeatherSystem.WeatherLightningBoltController | Spawn(Camera,Single,Vector2,Single) | get_value | transpiled | sim | no | 0 |
| visual | Owlcat.Runtime.Visual.Effects.ParticleSumEmitter.ParticleSystemCustomSubEmitter | RandomRangeVector3(Vector3,Vector3) | Range(float,float) | transpiled | pres | no | 159 |
| visual | Owlcat.Runtime.Visual.Effects.ParticleSumEmitter.ParticleSystemCustomSubEmitter | RandomRangeVector3(Vector3,Vector3) | Range(float,float) | transpiled | pres | no | 159 |
| visual | Owlcat.Runtime.Visual.Effects.ParticleSumEmitter.ParticleSystemCustomSubEmitter | RandomRangeVector3(Vector3,Vector3) | Range(float,float) | transpiled | pres | no | 159 |
| visual | Owlcat.Runtime.Visual.Effects.WeatherSystem.WeatherMinMaxArray | Sample(InclemencyType) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Owlcat.Runtime.Visual.Effects.WeatherSystem.WindParams | Sample(WindProfile,InclemencyType) | get_insideUnitCircle | transpiled | pres | no | 0 |
| visual | Owlcat.Runtime.Visual.RenderPipeline.Passes.HbaoPass | CreateRandomTexture(Int32) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Owlcat.Runtime.Visual.RenderPipeline.Passes.HbaoPass | CreateRandomTexture(Int32) | Range(float,float) | transpiled | pres | no | 0 |
| visual | Owlcat.Runtime.Visual.RenderPipeline.PostProcess.PostProcessUtils | ConfigureDithering(PostProcessData,Int32,Camera,Material) | get_value | transpiled | pres | no | 1439 |
| visual | Owlcat.Runtime.Visual.RenderPipeline.PostProcess.PostProcessUtils | ConfigureDithering(PostProcessData,Int32,Camera,Material) | get_value | transpiled | pres | no | 1439 |
| visual | Owlcat.Runtime.Visual.RenderPipeline.PostProcess.PostProcessUtils | ConfigureFilmGrain(PostProcessData,FilmGrain,Camera,Material) | get_value | transpiled | pres | no | 1439 |
| visual | Owlcat.Runtime.Visual.RenderPipeline.PostProcess.PostProcessUtils | ConfigureFilmGrain(PostProcessData,FilmGrain,Camera,Material) | get_value | transpiled | pres | no | 1439 |
| visual | RootMotion.FinalIK.IKSolverVR+Locomotion | Solve(VirtualBone,Spine,Leg,Leg,Arm,Arm,Int32,Vector3&,Vector3&,Quaternion&,Quaternion&,Single&,Single&,Single&,Single&) | Range(float,float) | transpiled | pres | no | 0 |
| visual | RootMotion.FinalIK.Recoil | Fire(Single) | get_value | transpiled | pres | no | 0 |
| visual | RootMotion.FinalIK.Recoil | Fire(Single) | get_value | transpiled | pres | no | 0 |
| visual | UnityEngine.Timeline.ControlPlayableAsset | OnEnable() | Range(int,int) | transpiled | sim | yes | 0 |
| visual | _2dxFX_AL_Distortion | Update() | Range(float,float) | transpiled | pres | no | 0 |
| visual | _2dxFX_AL_JellyAutoMove | Update() | Range(int,int) | transpiled | pres | no | 0 |
| visual | _2dxFX_AL_Mystic_Distortion | Update() | Range(float,float) | transpiled | pres | no | 0 |
| visual | _2dxFX_Distortion | Update() | Range(float,float) | transpiled | pres | no | 0 |
| visual | _2dxFX_Distortion_Additive | Update() | Range(float,float) | transpiled | pres | no | 0 |
| visual | _2dxFX_JellyAutoMove | Update() | Range(int,int) | transpiled | pres | no | 0 |
| visual | _2dxFX_Mystic_Distortion | Update() | Range(float,float) | transpiled | pres | no | 0 |
| visual | _2dxFX_Mystic_Distortion_Additive | Update() | Range(float,float) | transpiled | pres | no | 0 |
| visual | _2dxFX_Wave | Update() | Range(float,float) | transpiled | pres | no | 0 |
| wrapper | Kingmaker.Blueprints.BlueprintTrapSettings+IntRange | PickRandom() | Range(int,int) | transpiled | sim | yes | 0 |
| wrapper | Kingmaker.Utility.LinqExtensions | Random`1(IEnumerable`1,Func`3) | Range(int,int) | global hook | sim | yes | 0 |
| wrapper | Kingmaker.Utility.LinqExtensions | Random`1(IList`1) | Range(int,int) | global hook | sim | yes | 0 |
| wrapper | Kingmaker.Utility.LinqExtensions | Shuffle`1(IList`1) | Range(int,int) | global hook | sim | yes | 0 |
| wrapper | Kingmaker.Utility.LinqExtensions | WeightedRandom`1(IList`1) | Range(float,float) | gap-detected only | sim | yes | 0 |
| wrapper | Kingmaker.Utility.RandomUtils | GetRandomValue`1(IReadOnlyList`1) | Range(int,int) | global hook | sim | maybe | 0 |
| wrapper | Kingmaker.Utility.RandomUtils | GetRandomValue`1(IReadOnlyList`1,IReadOnlyList`1) | Range(float,float) | gap-detected only | sim | maybe | 0 |
| wrapper | Kingmaker.Utility.RandomUtils | GetRandomValue`1(T[]) | Range(int,int) | global hook | sim | maybe | 0 |
| wrapper | Kingmaker.Utility.RandomUtils | GetRandomValue`1(T[],Single[]) | Range(float,float) | gap-detected only | sim | maybe | 0 |
| wrapper | Kingmaker.View.FreePlaceSelector | MoveAwayFromObstacle(Vector3&,Vector3&,Boolean) | get_insideUnitCircle | transpiled | sim | yes | 0 |
| wrapper | Owlcat.Runtime.Core.LinqExtensions | Random`1(IEnumerable`1,Func`3) | Range(int,int) | global hook | sim | yes | 0 |
| wrapper | Owlcat.Runtime.Core.LinqExtensions | Random`1(IList`1) | Range(int,int) | global hook | sim | yes | 0 |
| wrapper | Owlcat.Runtime.Core.LinqExtensions | Shuffle`1(IList`1) | Range(int,int) | global hook | sim | yes | 0 |
| wrapper | Owlcat.Runtime.Core.LinqExtensions | WeightedRandom`1(IList`1) | Range(float,float) | gap-detected only | sim | yes | 0 |
| wrapper | Owlcat.Runtime.Core.Math.ProbabilityCurveSampler | SampleRandom() | get_value | transpiled | sim | yes | 0 |
| wrapper | Pathfinding.GridNodeBase | RandomPointOnSurface() | get_value | transpiled | sim | maybe | 0 |
| wrapper | Pathfinding.GridNodeBase | RandomPointOnSurface() | get_value | transpiled | sim | maybe | 0 |
| wrapper | Pathfinding.PathUtilities | GetPointsAroundPoint(Vector3,IRaycastableGraph,List`1,Single,Single) | get_onUnitSphere | transpiled | sim | maybe | 0 |
| wrapper | Pathfinding.PathUtilities | GetPointsOnNodes(List`1,Int32,Single) | Range(int,int) | transpiled | sim | maybe | 0 |
| wrapper | Pathfinding.PathUtilities | GetPointsOnNodes(List`1,Int32,Single) | get_value | transpiled | sim | maybe | 0 |
| wrapper | Pathfinding.TriangleMeshNode | RandomPointOnSurface() | get_value | transpiled | sim | maybe | 0 |
| wrapper | Pathfinding.TriangleMeshNode | RandomPointOnSurface() | get_value | transpiled | sim | maybe | 0 |
