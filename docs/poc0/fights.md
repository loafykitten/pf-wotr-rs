> Investigation by gpt-6.1-sol (Codex, read-only), 2026-10-10; reviewed by Claude, with key claims spot-checked against the save. Relative `evidence/`, `inputs/` and script paths refer to `cache/research/sol-fights/` (gitignored, local only).

# Prologue_Caves_1 encounter audit

## Result and scope

**No combat encounter was identified on the ordinary route from this save to either meeting.** The authored sequence is **save → Meet Seelah/Anevia → Meet Camellia → FirstBattle**. This ordering is a **route inference**, supported by the verified meeting triggers, recruitment actions and enemy positions; it is **not** a proof that every possible navigation/trigger bypass is impossible. The save does not already contain either meeting in its dialogue history. There is no verified three-fight set inside the requested pre-meeting window.

If the fixture window may extend **past both meetings**, recommend **FirstBattle: three young giant centipedes**, with **Optional1: one melee and one ranged centipede** and **BeforeLann: one monitor lizard** as extras. These are recommendations, not claims that those fights precede recruitment. The nine scene-authored groups below are an explicitly **out-of-window comparison inventory**, included to avoid silently selecting a later fight as an earlier one.

**Evidence labels:** **V** = directly verified saved/authored data or decompiled semantics; **I** = inferred ordinary route, future runtime behavior, or recommendation; **U** = unresolved. Counts are for the save's **Standard encounter capacity**, not the maximum number of editor spawners. "Fight" means an authored `UnitGroupView`; actual combat islands can merge/split with movement and aggro (**U**, not simulated).

## Sources and reproducibility

All writes are inside this scratch directory. The Steam install, original save, decompiled source and prior survey were read only. No tools/packages were installed, game launched, or saves changed.

Source aliases used below:

- **S**: `~/Library/Application Support/Owlcat Games/Pathfinder Wrath Of The Righteous/Saved Games/Manual_13_Underground_Caves_of_Kenabres__15_Arodus__VIII__4715__12_03_41.zks`; byte-identical scratch copy in `inputs/`, extracted to `inputs/save/`. SHA-256 and copy comparison: `evidence/save-provenance.json`.
- **B**: `~/Library/Application Support/Steam/steamapps/common/Pathfinder Second Adventure/Wrath.app/blueprints.zip`. Blueprint paths below are **archive-relative**, with selected local copies at `inputs/blueprints/<path>`. Every GUID maps to its source file in `evidence/guid-index.json`; the retained selection is `evidence/extracted-blueprints.txt`.
- **L**: the same `Wrath.app/Contents/Resources/Data/StreamingAssets/Localization/enGB.json`. Only short names were exported, with exact localization JSON paths, in `evidence/names.json`.
- **M**: the same `Wrath.app/Bundles/prologue_caves_1_mechanics.scenes`, scene asset GUID `93ce88a5d3c3c8041a07cd8d78f49422`. Scene object data are in `evidence/CAB-a19ece1c8d2e5eb637ed3ed09b6a2efe.json`; script identities in its `.sharedAssets.json` partner. `evidence/scene-entities.json` adds hierarchy and composed world transforms. A scene `component_id` below is a serialized **path ID**, not a blueprint GUID.
- **C**: `decompiled/Assembly-CSharp/`; C# paths below are relative to this directory.
- Prior survey: `~/Developer/Wrath-Rust/docs/research/architecture-survey.md`, sections **6** and **A**. Its warnings about scene-authored gameplay, started-versus-playing etudes, difficulty assets and incomplete handler closure were checked against the present inputs; its different dialogue fixture is not reused as this save's state.

`audit.py` indexes blueprint headers without extracting the archive, extracts selected GUIDs, and decodes the two mechanics bundles using Python's standard library and a local LZ4 block decoder. `scene_read.py` uses their embedded type trees; every parsed object's byte consumption is checked. `scene_summary.py`, `enemy_inventory.py` and `build_evidence.py` generate the supporting evidence. Main-scene GameObjects, Transforms and MonoBehaviours parsed successfully. Mesh/occlusion/texture failures are recorded in the scene JSON `$.failures`; these prevent a navigation/topology certification, not the spawner census. The Terendelev-scales addon was also inspected and contains no unit spawner. Static/light/audio scene bundles and unit prefabs were not decoded (**U** for gameplay content outside the two mechanics scenes).

Validation receipt: `evidence/validation.json`. All **599 retained blueprints** match their archive-entry bytes; the copied save matches the original SHA-256; every counted `$type` occurrence resolves back to its original JSON path. Group totals, capacity-disabled spawners, 15 living enemy placeholders and script syntax were checked. No live combat/navigation parity test was performed.

## 1. Save boundary

### Area, party and position — V

| Item | Value | Exact source fields |
|---|---|---|
| Area and part | `bad465e5404604346bdf71ef50b3299a`, `Prologue_Caves_1` | S `header.json: $.Area, $.AreaPart`; `player.json: $.CurrentArea, $.CurrentAreaPart`; B `World/Areas/Act_0_Prologue/Prologue_Caves_1/Prologue_Caves_1.jbp: $.AssetId, $.Data.m_DynamicScene.m_SceneName` |
| Area state | `Prologue_Caves_1_Mechanics` | S `bad465e5404604346bdf71ef50b3299a.json: $.m_MainState.SceneName` |
| Actual party | **FennA, Fighter 1**, Kitsune; no companions yet | S `party.json: $.m_EntityData[0].Descriptor.CustomName`, `.Progression.Classes[0].CharacterClass = 48ac8db94d5de7645906c7d0ad3bcfbd`, `.Level = 1`, `.Progression.m_Race = fd188bb7bb0002e49863aec93bfb9d99`; B `Classes/Fighter/FighterClass.jbp`, `Races/KitsuneRace.jbp` |
| Identity/membership | `35fffa0a-06ba-4cba-be40-7914924be190`; one party entity | S `party.json: $.m_EntityData[0].UniqueId`; `player.json: $.PartyCharacters[0], $.m_MainCharacter`; array length 1 |
| Position | raw `11682\|3930\|14093` → **(116.82, 39.30, 140.93)** | S `party.json: $.m_EntityData[0].Position`; C `Kingmaker.EntitySystem.Persistence.JsonUtility/Vector3Converter.cs: ReadJson` divides each integer by 100 |
| Orientation/camera | yaw `-161.616272`; camera **(115.57, 39.30, 137.92)** | S `party.json: $.m_EntityData[0].m_Orientation`; `player.json: $.m_CameraPos` — camera is not party position |
| Rules settings | Normal; stat adjustments **Decline**; enemy difficulty Normal; encounter capacity **Standard**; additional AI false; limited AI true; party damage multiplier 0.8 | S `player.json: $.SettingsList["settings.difficulty.game-difficulty" / "settings.difficulty.stats-adjustments" / "settings.difficulty.enemy-difficulty" / "settings.difficulty.combat-encounters-capacity" / "settings.difficulty.additional-ai-behaviours" / "settings.difficulty.limited-ai" / "settings.difficulty.damage-to-party"]` |

Newtonsoft IDs are graph identities, not enemy blueprint IDs. `party.json` and `player.json` are separate graphs; resolve `$ref` within its own file. Units use game `UniqueId`; unrevealed `UnitPlaceholderEntityData` stores its real unit identity under **`m_Id`**, with `UniqueId = "-"`. Do **not** count only `UnitEntityData` or treat null `m_SpawnedUnit` as a kill: a disabled-capacity spawner can set `HasSpawned = true` without a unit. **V:** C `Kingmaker.EntitySystem.Entities/UnitPlaceholderEntityData.cs` fields/restore path; `Kingmaker.View.Spawners/UnitSpawnerBase.cs: Spawn`, `Disable` branch.

The starting party is not a generic sword-only character: active **KitsuneChangeShapeBiteBuff** `08ce3d993d3a69e43a7b77425f93e964` and **FeatherFallBuff** `6784d2dfa2e552448828bffc9ca7a01b` are at S `party.json: $.m_EntityData[0].Facts.m_Facts[11]` and `[47]` (`Blueprint`, `IsActive`). `Descriptor.m_Spellbooks` is empty, but this does not mean the unit has no granted abilities. Enemy simplicity does not certify the party's full combat handler closure.

**Not party members:** Seelah is a materialized Paladin 1 at area-state entity `[163]`, position (110.50, 39.24, 132.76); Anevia is Rogue 3/Fighter 1 at `[167]`, position (111.73, 39.27, 131.54). Their class evidence is `.Descriptor.Progression.Classes[*]`, name identity `.Descriptor.Blueprint` (`54be53f0b35bf3c4592a97ae335fe765`, `ea562adea1736874c9c5616d140fe773`). Camellia is placeholder `[166]`, `m_Blueprint = 73775d22a1050a241aaa1ce8d560ba3f`, `m_Id = 37D2`, `m_Position = 7660|3653|15371`; her authored level-1 NPC is B `Units/Companions/Camelia/Camelia_NPC_Level1.jbp`, `$.Data.Components` (`AddClassLevels`). Lann/Wenduag are also scene NPCs, not saved party members.

`tbm.json` consists only of the three-byte UTF-8 BOM, not a populated turn-state JSON. Five background cutscene-player records remain at area-state entities `[175..179]` (`m_Cutscene`, `m_Remove = false`): sheltering, Camellia looting, Lann digging, Seelah/Anevia blockage, earthquake. They are individually listed in `evidence/state.json`; "after the opening" must not be interpreted as "all background cutscene state absent".

### Etudes and flags — V

Use **both** `player.json: $.EtudesSystem.m_EtudesData[*]` (`Key`, `Value`) and `$.EtudesSystem.Facts.m_Facts[*]` (`Blueprint`, `IsActive`, `IsCompleted`). A `Started` entry alone is not proof an etude is playing.

| State relevant to the caves | GUID | Current evidence / consequence |
|---|---|---|
| `0Prologue` | `cee461e88e5d62b4ebdfc1f4cecc13c5` | Started; fact `[1].IsActive = true` |
| `Caves_1` | `fb8827a1b4515af408f92e1ac1bdf794` | Started; fact `[1303].IsActive = true`; linked area and scales addon |
| `WakeUp` | `cab903611be84934b8ebb21623db4cfa` | Started; fact `[1304].IsActive = true`; completes after Seelah/Anevia recruitment |
| `PartyAssemble` | `5722b2a325c3d7043968fe44bd4b443c` | No status-map entry; not an active saved fact; starts when WakeUp completes |
| `AneviaFollow` | `b5f1df4ce72c8cd459eb18a1cffc091b` | No saved status/fact; started by Seelah/Anevia dialogue end |
| `SeelahFatigued` | `deac985ff40aeab45bda996027473cfc` | No saved status/fact; future dialogue branch can start it and attach Fatigued |
| `MongrelCaves`, `ToNeathholm` | `510860f7ee4ad2e4a89151daa7dd576c`, `af73a3cfa0cea0e41bfdfba04d85868e` | No saved status/fact; later progression; ToNeathholm supplies AfterClimb combat-spawn trigger |

Blueprints: B `World/Etudes/Common/WrathOfTheRighteous/0Prologue/Caves_1.jbp` and `Caves_1/{WakeUp,PartyAssemble,AneviaFollow,SeelahFatigued,MongrelCaves,ToNeathholm}.jbp`. Links are `$.Data.m_Parent`, `.m_StartsWith`, `.m_StartsOnComplete`, `.m_LinkedAreaPart`, `.m_AddedAreaMechanics`; actions are `.Components[*]`. `Caves_1.m_AddedAreaMechanics[0]` is `7fab345f3e534857a2c2735621671f16` (Terendelev scales). Completed Kenabres/opening etudes and all exact matching status indices are retained in `evidence/state.json`.

All seven saved unlockable flags, from `player.json: $.m_UnlockableFlags.m_UnlockedFlags[index].{Key,Value}`:

| Index | Flag name | GUID | Value |
|---|---|---|---:|
| 0 | Horgus_GetMeOut_price | `ddfedbdeab95ed941b6968b06162c921` | 1000 |
| 1 | Areelu_Leaves_Flag | `d15742b0a6c73ec4eaf1bac00f939782` | 0 |
| 2 | TelenderevBark | `2132298719ab88e47af2ca4efc1216fd` | 0 |
| 3 | TerendelevHealsPlayer_flag | `0c8df4717e569414ab10a181e45138bc` | 0 |
| 4 | PlayerDart_flag | `134d504812b291f40a1e5d85b744b909` | 0 |
| 5 | Counter_Entertain | `d295d1719937e9c488d9e8b11b9f88cb` | 2 |
| 6 | TestFlag_CutsceneKenabres | `4b0e3a5919cf15243a4a446f3706c106` | 0 |

These are **unlocked flag entries**, including zero values, not seven true boolean gates. Exact flag blueprint paths are in `evidence/state.json: $.flags[*].file`. No saved caves-specific fight-unlock flag was found; the inspected cave encounter gates use spawner capacity, etudes, script zones and recruitment instead.

## 2. Meetings, encounter groups and ordering

### Verified meeting boundary; inferred route

- **Seelah/Anevia**: B `World/Dialogs/c0/CavesUnderKenabres/MeetSeelahAnevia/MeetSeelahAnevia_Dialogue.jbp`, GUID **`2c5241474f3e16d40ab3f48e33921bcd`**. `World/Encounters/Prologue_Caves_1/ScriptZones/AneviaDialogueTrigger.jbp` (**`69c8d5fe0e4a02f42b70a3b067925bb7`**), `$.Data.EnterActions.Actions[0].IfTrue.Actions[0].m_Dialogue`, starts it for the PC; pet entry instead starts cutscene `9c6dac6d261545baa4883c81a4cac9de`. Scene zone UUID `e1c57f38-117a-4cdf-b80e-ca4f68271377`, center **(108.38, 40.32, 127.85)**, cylinder radius **10**, height **3**. `Cue_0049.jbp: $.Data.OnStop.Actions[1]` recruits Seelah, `[2]` starts AneviaFollow, `[3]` completes WakeUp. `WakeUp.jbp: $.Data.m_StartsOnComplete[0]` starts PartyAssemble. **V**.
- **Camellia** (asset folder/editor spelling `Camelia`): B `World/Dialogs/c0/CavesUnderKenabres/MeetCamelia/MeetCamelia.jbp`, GUID **`e92a8b79aa9c02f4d9641eb9ef22495d`**. `ScriptZones/CameliaDialogueTrigger.jbp` (**`d34daae626f91364ca0775f047057508`**), `$.Data.EnterActions.Actions[0].m_Cutscene = 9c848d79020634a47a482db09ad5b16c`; B `World/Cutscenes/Prologue_Caves_1/Meet_Camelia/CommandStartDialog.jbp: $.Data.m_Dialog` starts the meeting. Zone UUID `f515ee41-9fc6-44bb-97b8-dd145cffd481`, center **(73.06, 36.43, 154.30)**, radius **10**, height **3**. Trigger conditions accept PC **or** party Seelah. `MeetCamelia/Cue_0026.jbp: $.Data.OnStop.Actions[0]` recruits Camellia; its `.OnRecruit.Actions[0].Damage.Bonus = 4`, physical/slashing, scripts four damage and a healing tutorial. **V**.
- Both dialogue GUIDs are absent from S `player.json: $.m_Dialog.ShownDialogs`; area-state entities `[138]` (Anevia zone) and `[139]` (Camellia zone) have `IsActive = true`. FirstBattle units are farther along at x≈44–54, z≈152–159, beyond Camellia's zone on the ordinary path. Seelah/Anevia precedes Camellia and these enemies on that path (**I**); no movement/navmesh simulation or live playthrough was performed. **Do not promote this to a universal no-bypass guarantee.**

### Complete mechanics-scene group census — V; route column — I

Enemy IDs E1–E10 resolve to full GUID/name/class-level definitions in §3. Positions are approximate world **(x,z)** extents from composed scene transforms; restored units may differ slightly after ground placement. The reachability columns concern the **ordinary route**, not exploits. Every row is **outside** the strict pre-meeting window.

| Group / location | Enemies at this save's Standard capacity; enlarged/editor maximum | Spawn/gate and saved state | Before Seelah/Anevia? / Before Camellia? | Ordinary ordering and special mechanics |
|---|---|---|---|---|
| **FirstBattle**, (44–54, 152–159) | **E1 ×3**; ×5 enlarged | Scene-init spawners; three saved placeholders, two capacity-disabled spawners. Pool `184f1e6ade5d92f4b993aeb90521f971` is a registry, **not** a summoned reinforcement ability. | No / No (**I**) | First ordinary fight after both recruits (**I**). Melee-only young centipedes; no authored poison or enemy spell/buff action. PartyAssemble combat trigger shows tutorial. |
| **Optional1**, (33–37, 110–120) | **E2 ×1 + E3 ×1**; E3 ×2 enlarged | Scene-init; two saved placeholders, one capacity-disabled ranged spawner | No / No (**I**) | Side encounter after initial centipedes (**I**); acid spit adds ranged projectile/attack-roll/energy damage, not poison. |
| **Optional2**, (43–48, 132–135) | **E4 ×1**; ×2 enlarged | Scene-init; one saved fly placeholder, one disabled extra | No / No (**I**) | Nearby side branch after first fight (**I**); airborne and difficulty-gated disease-on-bite. Not a mechanics-free melee enemy. |
| **Optional3**, (67–74, 115–123) | **E4 ×1 + E3 ×1**; E3 ×2 enlarged | Scene-init; two saved placeholders, one disabled extra | No / No (**I**) | Another side branch (**I**); flight/disease + acid spit. |
| **BeforeLann**, (≈88, ≈110) | **E6 ×1**, unchanged enlarged | Scene-init; saved lizard placeholder `[162]`; roaming radius 3 | No / No (**I**) | After both meetings, before meeting Lann/Wenduag on ordinary route (**I**). Four-leg trip defense, natural armor, fortitude/perception feats; poison feature grant is difficulty-gated despite filename `NoPoison`. |
| **AfterClimb**, (118–125, 74–79) | **E7 ×1**, then **E8 ×1**; another E8 only with additional difficulty/capacity checks | Standard spider scene-init placeholder `[165]`; young spiders **not** init-spawned. ToNeathholm combat trigger on Spider1 runs `Spawn`; extra hard spider additionally conditional. | No / No (**I**) | Later, after ToNeathholm progression (**V** gate; **I** route). Poison, mid-combat spawn, spawn animation/passive/untargetable spans and scripted StartCombat. |
| **Optional4**, (143–145, 64–66) | **E10 ×2**; ×3 enlarged | Scene-init; two saved elemental placeholders `[155,156]`; one disabled extra | No / No (**I**) | Later side encounter (**I**); elemental immunities, Earth Mastery, Power Attack, Blind-Fight. Poor first target. |
| **AttackACDamageTutor**, (75–86, 64–71) | **E3 ×2 + E5 ×1**; E3 ×3 + E5 ×2 enlarged | Scene-init; three saved placeholders `[159,160,172]`; two disabled extras | No / No (**I**) | Later tutorial encounter (**I**); mixed ranged acid and airborne/disease enemies; tutorials are additional narrative state, not another enemy type. |
| **Optional5**, (55–62, 55–60) | **E9 ×1 + E8 ×1**; third E8 additionally difficulty/capacity-gated | All three have init-spawn off; Spider Ambush zone → Spider_Ambush cutscene → CommandSpawnUnits; saved spawned-unit references null | No / No (**I**) | Later ambush (**I**); prologue spider applies Sickened on failed Fortitude save, young spider melee-only. Spawn cutscenes and StartCombat; not an uncomplicated initial fixture. |

**Count check:** nine groups, **28 enemy spawners / ten unit blueprints** in M; **18 potential enemies** under Standard capacity and the unconditional future-spawn branches (15 already represented as placeholders, three later spawns). There are **33 total unit spawners**, the other five being Camellia, Seelah, Anevia, Lann and Wenduag. This is a census, not an assertion that 18 enemies must all be fought or that every group is accessible without additional story progression. The addon contributes none.

For **each** enemy count/spawn claim, `evidence/encounters.json: $[group].spawners[*]` carries the source GUID/file, editor name, scene `component_id`, stable `spawner_id`, capacity setting, scene position, exact save `save_path`, `save_HasSpawned` and `save_spawned_unit`. Its `components[*].data` retains full decoded spawner settings. In M the original fields are `$.objects[?path_id == component_id].data.{UniqueId,m_Blueprint.guid,m_SpawnOnSceneInit}` and the corresponding `SpawnerDifficultySettings.data.Setting`; positional evidence is Transform `m_LocalPosition`, `m_LocalRotation`, `m_LocalScale`, `m_Father` recursively composed.

Group identities (M `UnitGroupView.UniqueId`; save group identity cross-check):

| Group | Stable scene UUID |
|---|---|
| FirstBattle | `6da882ea-e3df-43f8-a566-daaf2797c538` |
| Optional1 | `05d04025-637d-4682-9811-1879229ae7d9` |
| Optional2 | `a613693c-5857-43ae-8834-7ee8dbe677c2` |
| Optional3 | `5f8c819e-ba22-4362-ae6c-7282198b5c34` |
| BeforeLann | `14436cf5-b5e5-439e-b2e1-58ee49a9f07a` |
| AfterClimb | `a4ac186e-26e6-4b59-b840-70592ea07e1a` |
| Optional4 | `a8eebc18-32de-4693-9c0e-4787f97a485c` |
| AttackACDamageTutor | `2e47134b-2df1-4533-a3ea-71c91a5703a5` |
| Optional5 | `00c54470-15a6-4443-87f8-62d825a888c8` |

### Script and ally mechanics that must not be silently dropped

- **Capacity is not simply the difficulty name.** C `Kingmaker.View.Spawners/SpawnerDifficultySettings.cs: Type, CanSpawn` maps serialized `Setting = 2` to **CoreAndHigher**, but checks `CombatEncountersCapacity >= Enlarged`. The save uses Standard. Settings 2 spawners cannot produce units here even if an action invokes them. All ten capacity extras are recorded individually. **V**.
- **FirstBattle tutorial**, not reinforcements: B `Caves_1/PartyAssemble.jbp: $.Data.Components[2]` is `EvaluatedUnitCombatTrigger`, references spawner `ea597e14-8096-4e95-9b7b-328809bd47dc`, and shows tutorial `bdbf88de16ef2fb45b133c7d667b0166`. M first-battle `SpawnerSummonPoolSettings.Pools[0].guid = 184f1e6ade5d92f4b993aeb90521f971`; C `SpawnerSummonPoolSettings.cs: Part.OnSpawn` only registers the existing unit. **V**.
- **AfterClimb reinforcements:** B `Caves_1/ToNeathholm.jbp` GUID `af73a3cfa0cea0e41bfdfba04d85868e`, `$.Data.Components[4].Unit.Spawner._entity_id = 4e40981f-52c6-4f55-bbf4-fc7cc14f86c2`. `.Actions.Actions[1].Spawners[0]` spawns young spider `3a61b429-eb12-4a65-995c-a2db21657e68`; `[0].IfTrue.Actions[0]` additionally targets `5a04ba78-bc69-4612-a659-bede978dde35`. Both use **Spawn**, not a summoning spell. **V**.
- **Optional5 ambush:** B `ScriptZones/Spider_Ambush.jbp` GUID `9bc9d97f2ed343a6b4cf36f52c323727`, `$.Data.EnterActions.Actions[0].m_Cutscene = 6dd9b169f9984eeaa8248d390c9597ae`; `World/Cutscenes/Prologue_Caves_1/Spider_Ambush/CommandSpawnUnits.jbp` GUID `b4f627f8c4444fab8139f9ca8fea3ae5` spawns `85efb432-366f-4e20-bd7a-8a178b588aec`; `CommandSpawnUnits2.jbp` GUID `799e7fab4adf4ce88e174c445896c346` spawns `eb4dee17-bfce-4fec-92c6-6aaef46adcdf`; `CommandSpawnUnits1.jbp` GUID `54282f6d5fe540dd8afcf3fa6604e00a` conditionally targets `ea0c2bd6-a89d-4f24-8987-bf8cbe5a5c85`. Root fields `.EntryCondition` and `.Spawners[*]`; `.ActionsOnSpawn.Actions` empty. **V**.
- **Spider spawn behavior:** `SpawnerCustomCutscene.Cutscene.guid = 02cb95a2d4134e47947f550a76a46a65`, B `World/Cutscenes/Prologue_Caves_1/Spider_Spawn/`. `CommandMarkPassiveSpan`, `CommandMarkUntargetableSpan`, `CommandUnitPlayCutsceneAnimation` apply to SpawnedUnit; `CommandAction.jbp` GUID `64025566066a4f6b8775e3cd28d7a333`, `$.Data.Action.Actions[0]`, explicitly starts combat with the PC. This is a scripted combat-start synchronization boundary. **V**.
- **Allies:** Camellia is initially passive (`SpawnerMakePassive`), then recruited before the ordinary FirstBattle (**V** recruitment; **I** participation). Seelah joins first; dialogue `Cue_0049.OnStop.Actions[1].OnRecruit` attaches **Fatigued** `e6f2fc5d73d88064583cb828801212f4` if SeelahFatigued is playing. Camellia's four scripted damage and a player's optional healing choice affect fixture HP. `AneviaFollow.jbp: $.Data.Components[0]` follows the PC and allows slower speed; Anevia is an NPC ally, not a party-array entry. Her blueprint `ea562adea1736874c9c5616d140fe773` has Friends faction, Dumb Monster Brain and composite longbow +1 `2753a0159681be94da3a5eeabc8c8d1a`, so AI contribution is plausible (**I**, not observed). Saved Anevia facts include BrokenLeg `13e59e6d056c3ba4e8b44b9dce3b641a` and NPC_Immortality `281a1f606d92728409ee5cbf5599855d`. The later Caves etudes additionally mark specified companions essential. **V** fields; runtime attack/essential-death outcomes need an oracle.

## 3. Enemy definitions and porting footprint

**CR caveat:** "CR label" below is the blueprint filename's label, not a fractional `$.Data.CR` field. Unit XP is authored in `Experience` components as `CR` **and** `Modifier` (e.g. young centipede **CR 1, Modifier 0.25**). Monster levels are `AddClassLevels.Levels`; do not conflate level/hit dice, the filename label and XP CR. MaxHP fields can be zero because creation computes HP; no runtime HP/AC or difficulty-adjusted DC is asserted here.

Unit-definition field paths for every row: B file `$.AssetId`, `$.Data.LocalizedName.stringkey`, `.Size`, `.Components[*]` (`$type = AddClassLevels`, `Levels`, `m_CharacterClass`; `$type = Experience`, `CR`, `Modifier`), `.m_AddFacts[*]`, `.Body.{m_PrimaryHand,m_AdditionalLimbs,m_EmptyHandWeapon}`, `.m_Brain`. Names are L `$.strings[unit.LocalizedName.stringkey]`; exact keys in `evidence/names.json`.

The five referenced `Units/Types/{CentipedeGiant,GiantFly,MonitorLizard,Spider,ElementalEarth}.jbp` definitions were also inspected: `$.Data.Components` and `.m_SignatureAbilities` are empty. All ten units have empty `.m_AdditionalTemplates` and `.AlternativeBrains`. These empty grouping definitions do not add active combat effects to the bounded footprint below.

| ID | Short localized name / authored variant | Unit GUID | CR label / class levels | B source path |
|---|---|---|---|---|
| E1 | Young Giant Centipede | `cd057d4d28be9fc46949fa1148bece7f` | ¼ / Vermin 1 | `Units/Monsters/Giant Centipede/CR0.25_GiantCentipedeYoung.jbp` |
| E2 | Giant Centipede, melee NoPoison | `64f75609947044d2be8c3c5569dd3a4c` | ¼ / Vermin 1 | `Units/Monsters/Giant Centipede/CR0.25_GiantCentipedeStandard_NoPoison.jbp` |
| E3 | Spitting Giant Centipede, NoPoison | `53375e396ad943d595b2b3a5bff3741a` | ¼ / Vermin 1 | `Units/Monsters/Giant Centipede/CR0.25_GiantCentipedeRanged_NoPoison.jbp` |
| E4 | Giant Fly | `7e63418db0c4ec0428ab59c0947d628d` | 1 / Vermin 2 | `Units/Monsters/GiantFly/CR1_GiantFlyStandard.jbp` |
| E5 | Young Giant Fly | `17e37098da8445244b66cb6438492b1a` | ½ / Vermin 2 | `Units/Monsters/GiantFly/CR0.5_GiantFlyYoung.jbp` |
| E6 | Monitor Lizard, NoPoison | `bba7c699d1464a03b1c0ff94332ee74c` | 2 / Animal 3 | `Units/Monsters/Monitor Lizard/CR2_MonitorLizardStandart_NoPoison.jbp` |
| E7 | Giant Spider, standard | `c4b33e5fd3d3a6f46b2aade647b0bf25` | 1 / Vermin 3 | `Units/Monsters/Spider/CR1_GiantSpiderStandard.jbp` |
| E8 | Young Giant Spider | `00f6b260b3727b44ba30a9e51abf3b11` | ½ / Vermin 3 | `Units/Monsters/Spider/CR0.5_GiantSpiderYoung.jbp` |
| E9 | Giant Spider, prologue | `b048bb08e51492a4092063026282fa93` | ½ / Vermin 3 | `Units/Monsters/Spider/CR0.5_GiantSpiderStandard_Prologue.jbp` |
| E10 | Small Earth Elemental | `a940662426e68524680bc06e1794fa8d` | 1 / Outsider 2 | `Units/Monsters/EarthElemental/CR1_EarthElementalSmall.jbp` |

Class sources: Vermin `d1a15612d1a96334d94edf5f1d3b8d29` → progression `7d22ef9a80229194cb7aaaf8e7608a54`; Animal `4cd1757a0eea7694ba5c933729a53920` → `ca3fcf31e31262d44a999a1733bbd84d`; Outsider `92ab5f2fe00631b44810deffcc1a97fd` → `d63657c4ae347ac4fb87f123b98f78d4`. B `Classes/Monster/{Vermin,Animal,Outsider}/`, class `.m_Progression`, progression `.LevelEntries[*].m_Features`. These supply creature-type facts and stat progressions in addition to direct unit features.

### Per-enemy abilities, features, buffs and weapons — V

Common weapon **W1** = Bite1d6 `a000716f88c969c499a535dadcf09286`, B `Weapons/Items/Standard/NaturalWeapon/Bite1d6.jbp`; **W2** = BiteDragonLarge1d8 `c988aa874d11ff84d873508ddc9b928f`, B `Weapons/Items/LargeStandard/NaturalWeapon/BiteDragonLarge1d8.jbp`; **W3** = EarthElementalSlam_Small `f217709b11a0f8b439e5d658842e587c`, B `Units/Monsters/EarthElemental/EarthElementalSlam_Small.jbp`. W2's filename is not evidence the lizard itself is Large; unit `.Size = Medium`. Each unit also authors EmptyHand fallback `20375b5a0c9243d45966bd72c690ab74` (`WeaponEmptyHand.jbp`); it is not counted as a guaranteed additional attack.

| Type | Direct and class-granted features / feats | Active abilities and authored buff effects | Weapon placement |
|---|---|---|---|
| E1 | WeaponFinesse, TripImmune; VerminType → VerminImmunities; HeadLocatorFeature | No granted active ability, poison, buff-application or damage action found in bounded closure | W1 as additional limb; **Small**, Str 5/Dex 19 |
| E2 | E1's gameplay features plus NaturalArmor2 | No granted active ability or poison found | W1 additional limb; Medium |
| E3 | E2 plus GiantCentipedeSpitAbility | Spit: **Special** ability, projectile needing attack roll, **1d4 acid** action; no poison buff | W1 additional limb; separate projectile weapon `f6ef95b1f7bb52b408a5b345a330ffe8` |
| E4/E5 | Airborne (Flying, descriptor immunities, conditional attack/damage modifiers), TripImmune, VerminType/immunities, GiantFly_Feature_Disease | On-hit bite → difficulty condition → Fortitude → DemonPlague; referenced disease buff also has subsequent actions/Fatigued dependency. No enemy castable spell was found | W1 primary hand; E4 Medium, E5 Small |
| E6 | NaturalArmor3, SkillFocusPerception, GreatFortitude, TripDefenseFourLegs (+4 trip defense), AnimalType, MonitorLizard_Feature_NoPoison | The last feature **can grant MonitorLizardPoisonFeature at Core's stat-adjustment threshold**, on hit → Fortitude → MonitorLizardPoisonBuff. It is not unconditionally poison-free | W2 additional limb; Medium |
| E7 | NaturalArmor1, TripDefenseEightLegs (+8), SpiderWebImmunity, VerminType/immunities, GiantSpiderPoisonFeature | On-hit Fortitude save → GiantSpiderPoisonBuff; no Web casting ability found. Web immunity is not a granted Web spell/buff | W1 additional limb; Medium |
| E8 | TripDefenseEightLegs, SpiderWebImmunity, WeaponFinesse, VerminType/immunities; ChangeUnitSize component | No poison, Web casting or buff action in this young variant | W1 additional limb; unit size Small; size-change component separately retained |
| E9 | NaturalArmor1, TripDefenseEightLegs, SpiderWebImmunity, VerminType/immunities, GiantSpiderPoisonFeature_Prologue | On-hit Fortitude save → **Sickened for four rounds**, rather than standard spider poison buff | W1 additional limb; Medium |
| E10 | NaturalArmor7, Earth/Elemental/Extraplanar subtypes, OutsiderType, PowerAttackFeature, BlindFight, EarthMastery, NotADemonFeature | PowerAttack activatable/buffs; EarthMastery conditional attack/damage **+1 vs non-Airborne / −4 vs Airborne**; elemental immunity machinery. Outsider progression includes an unchosen feat-selection definition; not a certification of its runtime selected feat | W3 additional limb; Small |

Feature, ability, buff and weapon **GUIDs and exact B paths for every row** are enumerated in `evidence/enemy-inventory.json`, selected by `$.guid == E*.guid`: `.edges[*]` records `from`, JSON `path`, `to`; `.classifications` lists each blueprint kind with GUID/file. These are dependency definitions, **not saved active enemy buff instances**. Scene enemies are mostly placeholders at this boundary.

Especially important identities/fields:

- WeaponFinesse `90e54424d682d104ab36436bd527af09`; TripImmune `c1b26f97b974aec469613f968439e7bb`; VerminType `09478937695300944a179530664e42ec`; VerminImmunities `ef52e9b62f00135458c9093783c270e6`. Direct `.m_AddFacts`, class progression `.LevelEntries[0].m_Features[0]`, VerminType `AddFacts.m_Facts`; immune descriptor masks are retained, not replaced by a blanket invulnerability rule.
- Spit ability `d72908910a97ec742920c0e702aeb6dd`, B `Units/Monsters/Giant Centipede/GiantCentipedeRangedBrain/GiantCentipedeSpitAbility.jbp`: `$.Data.Components[0].{NeedAttackRoll,m_Weapon}`, `.Components[2].Actions.Actions[0].{DamageType.Energy,Value.DiceType,Value.DiceCountValue.Value}`; `.Type = Special`.
- Fly disease feature `1fa6430e6064d5f4f8553c67257a1878`, B `Units/Monsters/GiantFly/GiantFlyBrain/GiantFly_Feature_Disease.jbp`: `.Components[0].{OnlyHit,m_WeaponType}`, `.Action.Actions[0].ConditionsChecker.Conditions[0].m_Difficulty.guid = 4164268f34506a741ba333a330cb4d42`, `.IfTrue.Actions[0]` Fortitude save and nested failed-save buff application. The blueprint's comment says Core, but the actual check compares **stat-adjustment presets**, not the displayed difficulty name.
- Monitor no-poison feature `2d1984c4dad24d1f8586b872520c2b33`, B `Units/Monsters/Monitor Lizard/MonitorLizard_Feature_NoPoison.jbp`: `.Components[0].HasDifficultyRequirements = true`, `.MinDifficulty = Core`, `.m_Facts[0] = d88236a83413baa45ae9c8e5ddce5a6c` (MonitorLizardPoisonFeature), eventually buff `0d2aad6b3df1b5848964d07974cfb8c5`. C `Kingmaker.UnitLogic.FactLogic/AddFacts.cs: IsRestrictedByDifficulty`; `Kingmaker.Settings.Difficulty/DifficultyPresetsController.cs: CurrentDifficultyStatAdjustmentCompareTo`.
- Spider poison feature `094714bb08f4e1943a8e9d2384ebe573`; prologue feature `91dcd2cd4d3b6e4439c77c88f20f44d0`. B `Units/Monsters/Spider/GiantSpiderPoisonFeature{,_Prologue}.jbp`, `.Components[0].Action.Actions[0]` and nested `Failed.Actions[0]`; prologue buff GUID `4e42460798665fd4cb9173ffa7ada323`, `.DurationValue.BonusValue.Value = 4`. Young unit does **not** list either poison feature.
- SpiderWebImmunity `3051e7002c803fc47a11bcfa381b9fbd` references `bb72a758112438e4a9c62f7637c974ae` and `a719abac0ea0ce346b401060754cc1c0` under `.Components[*].m_Buff` as **SpecificBuffImmunity targets**, not active/granted buffs. These target identities were not recursively counted as active effects.
- EarthMastery `be65c8334ceac234195be030137563d8`, B `Units/Monsters/EarthElemental/EarthMastery.jbp`: `.Components[0..3].Bonus.Value`, `.Conditions.Conditions[0].{Not,m_Fact}` checks Airborne `70cffb448c132fa409e49156d013b175`; PowerAttackFeature `9972f33f977fc724c838e59641b2fca5` and BlindFight `4e219f5894ad0ea4daa0699e28c37b1d` are direct unit facts.

### Distinct `$type` counts — V structural counts, not certified runtime requirements

`C/A` = distinct component/action **full `$type` strings**, not instances. Direct counts inspect the unit only; bounded counts include granted/referenced combat definitions, actual monster class levels, weapons/types, brains/AI actions, activatable abilities, resources and effect buffs. Action tags are objects at `*.Actions[index]`; evaluators/conditions/blueprint roots are excluded from A but included in **all tags**. Component tags come from `*.Components[index]`. No unchosen selection-option catalogue, prerequisite/recommendation dependency graph, immunity-target buff behavior, projectile presentation or loot graph is recursively traversed. Direct UI/recommendation/presentation components still count where present; branches gated off at some difficulties also count. See `enemy_inventory.py` for the explicit traversal boundary and `.omitted_edges` for excluded reference identities.

| Type | Direct C/A | Bounded blueprint count | Bounded C/A | All distinct tags |
|---|---:|---:|---:|---:|
| E1 young centipede | 3 / 0 | 15 | 19 / 0 | 25 |
| E2 melee centipede | 3 / 0 | 16 | 20 / 0 | 27 |
| E3 ranged centipede | 3 / 0 | 20 | 23 / 1 | 33 |
| E4 standard fly | 3 / 0 | 20 | 19 / 7 | 36 |
| E5 young fly | 3 / 0 | 20 | 19 / 7 | 36 |
| E6 monitor lizard | 4 / 0 | 19 | 19 / 3 | 30 |
| E7 standard spider | 3 / 0 | 18 | 16 / 3 | 27 |
| E8 young spider | 4 / 0 | 16 | 20 / 0 | 26 |
| E9 prologue spider | 3 / 0 | 18 | 16 / 3 | 27 |
| E10 earth elemental | 3 / 0 | 29 | 25 / 1 | 36 |

All exact tags and occurrences are in `evidence/enemy-inventory.json: $[unit].type_tags.{components,actions,all}[full_tag][*].{guid,file,path}`; the top-level `.counts` and `.direct_component_types/.direct_action_types` are independently reproducible. Shared types must be unioned, not summed across enemies. Fewer tag types do not necessarily mean a simpler fight: poison/disease handlers are substantially more involved than stat bonuses. No complete loaded-unit event-handler certification is claimed.

## 4. Recommendation

**Strict original window:** no verified eligible fight; consequently no honest first-plus-two recommendation inside it. A traversal/oracle trace is the focused missing evidence, not another blueprint-name search.

**If extending the boundary beyond both meetings:**

1. **FirstBattle, E1 ×3 — first port target.** One enemy blueprint, one monster level, one bite weapon, no enemy poison/disease/projectile/buff-action closure. Standard-capacity counts are established by both scene restrictions and saved placeholders. Exercise initiative, movement, melee attacks, reach/AoO, armor/attack/damage, death/XP and victory without starting with elemental or delayed status mechanics. Preserve WeaponFinesse, Small-size behavior, TripImmune and VerminImmunities; this is not permission to ignore facts. Tutorial and summon-pool registration can be separated from the combat kernel, while keeping their state in the boundary fixture.
2. **Optional1, E2 ×1 + E3 ×1 — first extra.** Only two enemies; shares Vermin/Bite/Finesse/Trip machinery with E1. Adds natural armor and one Special-ability **1d4 acid spit** action: a small, explicit test of ranged attack/projectile delivery, damage typing, targeting and AI selection, without poison or scripted reinforcements.
3. **BeforeLann, E6 ×1 — second extra.** One melee enemy; tests a different monster class/stat progression, stronger bite, natural armor, fortitude/perception features and four-legged trip defense, plus optional roaming-to-combat entry. It is preferable to flies' disease chain or elementals' conditional/offensive/immunity machinery. **Condition:** first resolve the Core preset's stat-adjustment threshold and prove the poison feature is not granted under this save's Decline settings. If it is granted, this fixture must include poison rather than rely on `NoPoison` in the name.

These are **I** design choices based on the **V** inventory. Capture a **new post-recruitment, immediately pre-combat boundary** for each, including actual party spells/facts/equipment, HP, Camellia's scripted damage/healing, Seelah's branch-dependent fatigue, Anevia participation and difficulty state. Replaying this save directly as a solo fight with three centipedes would skip the real intervening recruitment state. A deliberately solo synthetic kernel fixture is possible, but must be labeled synthetic, not a faithful replay of the reachable encounter.

## 5. Open questions / limits

1. **Strict reachability and meeting bypasses:** enemy/zone coordinates and script chains establish the normal-route inference, not navigation connectivity. Need a short live trace from this save to both meetings, or decoded static collision/navmesh/area transitions plus trigger-crossing semantics. Optional1/2/3's exact branch order, and whether any unusual route can reach them before Camellia, remain **U**. Do not claim the requested set is universally empty from a spawner table alone.
2. **Difficulty assets:** Unity preset GUID `4164268f34506a741ba333a330cb4d42` and Core's preset value were not decoded. C `DifficultyHigherThan.CheckCondition` and `ContextConditionDifficultyHigherThan.CheckCondition` compare stat-adjustment thresholds; some names/comments are not sufficient evidence. This affects fly disease, the lizard's poison grant and harder spider branches. Standard-capacity suppression itself is verified independently.
3. **Combat-start and party closure:** no runtime observation establishes initiative order, starting HP/AC, computed DCs, NPC attacks, essential-death behavior, party spell loadout after automatic leveling, or exact aggro islands. Enemy conditional effects and selection definitions are not an exhaustive runtime fact-handler certificate. Unit prefab/world presentation may also carry behavior not decoded here.
4. **Spider timing:** spawn/passive/untargetable/animation/StartCombat graph is verified; the precise turn-based ordering, whether actions pause the combat clock, and interaction with existing combat require runtime/turn-based controller traces.
5. **RNG:** no save-derived historical next-roll seed or draw tape was established. A headless parity fixture needs oracle-recorded rolls/events; inventing a seed would not certify replay fidelity.
6. **Input/build identity:** the copied save hash and scene hashes are preserved, and blueprint files are exact targeted archive entries. No asserted equivalence to a different platform/game build or to the prior survey's dialogue fixture.
