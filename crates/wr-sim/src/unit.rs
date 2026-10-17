//! `UnitEntityData` / `UnitDescriptor` state and the descriptor-level behaviour the stats
//! and the load path call into.

use wr_content::Guid;

use crate::stats::types::{Size, StatType};
use crate::stats::value::Stats;
use crate::world::{EntityIdx, FactIdx, FactKind, World};

/// `ClassData`.
#[derive(Debug, Clone)]
pub struct ClassData {
    pub class: Guid,
    pub level: i32,
    pub archetypes: Vec<Guid>,
    /// `ClassData.Spellbook`: the class's spellbook, replaced or removed by archetypes and
    /// spellbook-replacing features (`new ClassData(class)` takes the class's).
    pub spellbook: Option<Guid>,
}

/// `ProgressionData` (`UnitProgressionData.m_Progressions`): the level a progression has
/// been applied up to, and the archetypes that change its level entries.
#[derive(Debug, Clone)]
pub struct ProgressionData {
    pub progression: Guid,
    pub level: i32,
    pub archetypes: Vec<Guid>,
}

/// `FeatureSelectionData` (`UnitProgressionData.m_Selections`): the features picked from a
/// selection, by character level, and the source the selection came from.
#[derive(Debug, Clone)]
pub struct SelectionData {
    pub selection: Guid,
    pub source: Option<Guid>,
    pub by_level: Vec<(i32, Vec<Guid>)>,
}

/// `UnitProgressionData` (the parts the sim reads).
#[derive(Debug, Clone, Default)]
pub struct Progression {
    pub race: Option<Guid>,
    pub classes: Vec<ClassData>,
    pub experience: i64,
    /// `MythicExperience` (the unit's: `AddClassLevel` raises it to the mythic level).
    pub mythic_experience: i32,
    /// `m_CharacterLevel` / `m_MythicLevel` caches (`SetupLevelsIfNecessary`).
    pub character_level: Option<i32>,
    pub mythic_level: Option<i32>,
    /// `m_Progressions` of the progressions the sim builds (`spawn::levelup`), in dictionary
    /// (insertion) order.
    pub progressions: Vec<ProgressionData>,
    /// `m_Selections`, in dictionary (insertion) order.
    pub selections: Vec<SelectionData>,
}

/// `UnitPartSpellResistance.SpellImmunity` (`UnitPartSpellResistance.cs`).
#[derive(Debug, Clone)]
pub struct SpellImmunity {
    pub id: i32,
    /// `SpellImmunityType` (3: SpellDescriptor).
    pub kind: i32,
    /// `AlignmentComponent` (0: None).
    pub alignment: i64,
    /// `SpellDescriptor` flags.
    pub spell_descriptor: i64,
    pub caster_ignore_fact: Option<Guid>,
}

/// The modelled state of `UnitPartSpellResistance`: the next id and the immunities. Its
/// resistance values, bonuses and ignored descriptors aren't modelled (nothing in the sim
/// adds them).
#[derive(Debug, Clone, Default)]
pub struct SpellResistancePart {
    pub next_id: i32,
    pub immunities: Vec<SpellImmunity>,
}

/// The modelled state of `UnitPartDamageReduction` (`UnitPartDamageReduction.cs`): the saved
/// `m_SourceFacts`, the chunks (one per `AddDamageResistanceBase` runtime of a source fact;
/// built lazily by `TryInitialize`, so empty on a loaded part until then) and whether they
/// were built. Combat's reduction (`ApplyDamageReduction`) is not ported: a unit with the
/// part reports it when it takes damage.
#[derive(Debug, Clone, Default)]
pub struct DamageReductionPart {
    pub sources: Vec<FactIdx>,
    /// The source fact of each chunk, in order.
    pub chunks: Vec<FactIdx>,
    pub initialized: bool,
}

/// An equipment slot of `UnitBody`.
#[derive(Debug, Clone)]
pub struct Slot {
    /// The `UnitBody` member (`Armor`, `Belt`, `PrimaryHand` of hand set 0…).
    pub name: String,
    pub item: Option<EntityIdx>,
    pub active: bool,
}

/// `UnitBody` (slots only).
#[derive(Debug, Clone, Default)]
pub struct Body {
    pub current_hand_set: i32,
    pub slots: Vec<Slot>,
    /// `m_EmptyHandWeapon`.
    pub empty_hand: Option<EntityIdx>,
    /// `m_EmptyHandWeaponsStack` (`null` is empty).
    pub empty_hand_stack: Vec<EntityIdx>,
    /// `ItemEntityWeapon.IsPermanentEmptyHand` and `IsMonkUnarmedStrike` of the empty-hand
    /// weapons (neither is saved, so a loaded weapon has both false): (item, permanent, monk).
    pub empty_hand_flags: Vec<(EntityIdx, bool, bool)>,
    /// `HandsAreEnabled` (`[DefaultValue(true)]`, saved).
    pub hands_disabled: bool,
    /// `m_AdditionalLimbs`.
    pub limbs: Vec<Slot>,
    /// `WeaponSlot.ParentFact` of the limbs an `AddAdditionalLimb` fact filled, by limb index.
    pub limb_parents: std::collections::HashMap<usize, crate::world::FactIdx>,
    /// `HandsEquipmentSet.m_GripType` per set (`GripType`: 0 Auto, 1 OneHanded, 2
    /// TwoHanded).
    pub grip_types: Vec<i32>,
}

/// `UnitProficiency`.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct Proficiencies {
    pub armor: Vec<(i64, i32)>,
    pub weapons: Vec<(i64, i32)>,
    pub background_armor: Vec<(i64, i32)>,
    pub background_weapons: Vec<(i64, i32)>,
}

/// `UnitEntityData` + `UnitDescriptor`.
#[derive(Debug, Clone, Default)]
pub struct Unit {
    pub blueprint: Option<Guid>,
    /// `ShiftersFuryPart` (present with the part).
    pub shifters_fury: Option<crate::components::shifters_fury::Part>,
    pub original_blueprint: Option<Guid>,
    pub stats: Stats,
    pub progression: Progression,
    pub body: Body,
    /// The items of the unit's own inventory (or the shared one for the main character).
    pub inventory: Vec<EntityIdx>,
    pub has_own_inventory: bool,
    /// `m_DroppedLootEntityDataRef` is set (saved): destroying the unit hands its inventory
    /// to that dropped loot.
    pub has_dropped_loot_ref: bool,
    /// The dropped-loot entity `m_DroppedLootEntityDataRef` names, when this run created it
    /// (`UnitEntityData.DropLootToGround`; a saved reference isn't resolved).
    pub dropped_loot: Option<crate::world::EntityIdx>,
    /// `Inventory`: identity of the unit's `ItemsCollection` (the player's shared one for
    /// party members without their own).
    pub inventory_collection: Option<usize>,
    /// `m_Damage`.
    pub damage: i32,
    pub is_player_faction: bool,
    pub faction: Option<Guid>,
    /// `Descriptor.AttackFactions` (`UnitAttackFactions.m_Factions`, saved): the factions
    /// the unit attacks. Starts as the blueprint's (`BlueprintUnit.AttackFactions`:
    /// faction's plus the unit's overrides) and changes at runtime.
    pub attack_factions: Vec<Guid>,
    pub original_size: Option<Size>,
    /// `UnitState.m_Size` (not saved: `Size` reads `OriginalSize` until it's set).
    pub state_size: Option<Size>,
    /// `UnitState.m_SizeBeforeDeath`.
    pub size_before_death: Option<Size>,
    /// `UnitPartSizeModifier.m_SizeChangeFacts` (the part exists while it holds any).
    pub size_change_facts: Vec<FactIdx>,
    /// The save had a `UnitPartSizeModifier` (its `m_SizeChangeFacts` and `OnPostLoad` aren't
    /// hydrated: the unit's size is reported wherever it's read).
    pub size_part_from_save: bool,
    pub main_fact: Option<FactIdx>,
    pub descriptor_turned_on: bool,
    pub is_main_character: bool,
    pub is_pet: bool,
    /// `UnitPartActivatableAbility.m_GroupsSizeIncreases` (empty until the part exists).
    pub group_size_increases: Vec<i32>,
    /// `UnitPartExtraSpellsPerDay.BonusSpells`, if the unit has the part.
    pub extra_spells_per_day: Option<Vec<i32>>,
    /// `UnitPartSpecialMetamagic.m_MetamagicOnSpellList`: the facts whose `AutoMetamagic` is on.
    pub special_metamagic: Vec<FactIdx>,
    /// `Descriptor.m_Spellbooks`, by blueprint, in `Dictionary` order.
    pub spellbooks: crate::csharp::CsDict<Guid, crate::spellbook::Spellbook>,
    /// `m_AppliedUpgraders` (unit upgrader blueprints already applied).
    pub applied_upgraders: Vec<Guid>,
    /// `Descriptor.Proficiencies` multisets: (enum value, count) in insertion order.
    pub proficiencies: Proficiencies,
    /// `State.Features` counters by `MechanicsFeatureType`.
    pub mechanics_features: std::collections::BTreeMap<i64, i32>,
    /// `Progression.LevelUpPlanProviders`.
    pub level_up_plan_providers: Vec<crate::world::CompRef>,
    /// `Descriptor.Resources` (`UnitAbilityResourceCollection.m_Resources`), in `Dictionary`
    /// order.
    pub resources: crate::csharp::CsDict<Guid, crate::resources::AbilityResource>,
    /// `UnitPartWeaponTraining.m_weaponTrainings`: the weapon training facts, in order.
    pub weapon_trainings: Vec<crate::world::FactIdx>,
    /// `Position` (the save's `x|y|z` hundredths, divided by 100 as `Vector3Converter`).
    pub position: [f32; 3],
    /// `Orientation` (degrees).
    pub orientation: f32,
    /// `UnitPartCompanion.State` (`CompanionState`: 0 None, 1 Remote, 2 InParty,
    /// 3 ExCompanion, 4 InPartyDetached), if the unit has the part.
    pub companion_state: Option<i32>,
    /// `UnitPartCompanion.m_Spawner` (saved): the `UniqueId` of the companion's current
    /// spawner's data (`GetCurrentSpawner` resolves it to a loaded `CompanionSpawner` view).
    pub companion_spawner: Option<String>,
    /// `UnitPartCompanion.IsIgnoreFixInGame` (saved).
    pub ignore_fix_in_game: bool,
    /// `UnitPartPet.Type` (`PetType`), if the unit has the part.
    pub pet_type: Option<i32>,
    /// `UnitPartPetMaster.m_Pets` (saved): the pets' unique ids, in order.
    pub pet_refs: Vec<String>,
    /// `SpawnPosition` (saved like `Position`).
    pub spawn_position: [f32; 3],
    /// `Descriptor.LastRestTime` (100 ns ticks), when set.
    pub last_rest_time: Option<i64>,
    /// `Descriptor.Alignment`.
    pub alignment: Option<crate::narrative::alignment::UnitAlignment>,
    /// `GroupId`.
    pub group_id: Option<String>,
    /// `State.LifeState` (`UnitLifeState`: 0 Conscious, 1 Unconscious, 2 Dead).
    pub life_state: i32,
    /// `Master` (pets).
    pub master: Option<EntityIdx>,
    /// `UnitPartEssential`-style flags set by etude brackets (`MarkUnitEssential`), and
    /// `UnitPartMakePassive` counters: (source fact, kind) entries.
    pub bracket_marks: Vec<(FactIdx, &'static str)>,
    /// `Descriptor.CustomGender` (`Gender`: 0 Male, 1 Female), if set.
    pub custom_gender: Option<i64>,
    /// `Descriptor.CustomName` (saved; "" when unset).
    pub custom_name: String,
    /// `Descriptor.ReplaceBlueprintForInspection` (saved).
    pub replace_blueprint_for_inspection: Option<Guid>,
    /// `State.IsFinallyDead` (saved).
    pub finally_dead: bool,
    /// `State.IsInStealth` (saved; set by `RuleEnterStealth`, which isn't ported: a recording
    /// that shows it is reported by the checks that read it).
    pub in_stealth: bool,
    /// `UnitStealth.m_SpottedBy` (saved): the unique ids of the units that spotted it, in order.
    pub stealth_spotted_by: Vec<String>,
    /// `UnitStealth` flags the stealth controller acts on (`WantEnterStealth`, `InAmbush`,
    /// `ForceEnterStealth`, `ShouldExitStealth`), any set in the save.
    pub stealth_pending: bool,
    /// The unit has a `UnitPartBlindsense` (saved; its range isn't hydrated).
    pub has_blindsense: bool,
    /// `State.MarkedForDeath`, `State.ForceKill` (saved).
    pub marked_for_death: bool,
    pub force_kill: bool,
    /// `!GiveExperienceOnDeath` (saved; the property defaults to true).
    pub no_xp_on_death: bool,
    /// `UnitEntityData.IsExtra` (saved).
    pub is_extra: bool,
    /// `UnitEntityData.BonusExperience`: set when the unit is initialized
    /// (`DifficultyList.GetExperienceBonus(CombatEncountersCapacity)`), kept on it.
    pub bonus_experience: Option<f32>,
    /// `UnitPartDamageGrace.Weapons`: the weapon categories of its entries (`None`: the unit
    /// has no such part).
    pub damage_grace: Option<Vec<i64>>,
    /// `BuffCollection.m_IsOwnerAlive` is false (`UpdateOnDeadOwner` has seen the owner dead).
    pub buffs_owner_dead_seen: bool,
    /// `BuffCollection.m_PreviousTime` (100 ns ticks): the time of the last `Tick` that ran to
    /// its end, or of the last `Postpone`. Not saved: zero (`TimeSpan.Zero`) after a load.
    pub buffs_previous_time: i64,
    /// `State.DeathTime` (game time of the last death, 100 ns ticks).
    pub death_time: i64,
    /// `Descriptor.CustomPrefabGuid` (saved), when set.
    pub custom_prefab_guid: Option<String>,
    /// The prefab of the view a unit built in this run was given (`EntityCreator.SpawnUnit`).
    pub view_prefab: Option<String>,
    /// `UnitState.Prone.Active` (saved).
    pub prone_active: bool,
    /// `UnitState.Prone.ShouldBeActive` (saved): a knockdown's request, consumed by the next
    /// `UnitProneController` tick.
    pub prone_should_be_active: bool,
    /// `UnitState.Prone.Duration` (saved), in 100 ns ticks.
    pub prone_duration: i64,
    /// `UnitPartSpellResistance` (`m_NextId` and `m_SpellImmunities`; not saved, so empty
    /// after a load). `None`: the unit has no such part (a fresh part restarts at id 0).
    pub spell_resistance: Option<SpellResistancePart>,
    /// `UnitPartDamageReduction` (`None`: the unit has no such part).
    pub damage_reduction: Option<DamageReductionPart>,
    /// `Descriptor.Brain` as saved (`None`: the save has none).
    pub brain: Option<crate::combat::ai::Brain>,
    /// `Descriptor.State`: conditions, immunities, status effects.
    pub state: crate::unit_state::UnitStateData,
    /// `UnitEntityData.CombatState` and combat membership (POC-5).
    pub combat: crate::combat::UnitCombat,
    /// `UnitEntityData.PreviewOf`: the unit a level-up preview copies.
    pub preview_of: Option<EntityIdx>,
    /// `BuffCollection.m_Disabled` (`SetupPreview`): a preview unit takes no buffs.
    pub buffs_disabled: bool,
    /// The `LevelUpPlanUnitHolder` part (present while `UnitDescriptor.Initialize` runs) and
    /// the plan unit it made (`m_PlanUnit`).
    pub plan_holder: bool,
    pub plan_unit: Option<EntityIdx>,
    /// A placeholder's saved body whose items aren't resolvable yet (`ItemSlot.MaybeItem`
    /// answers null until `UnitPlaceholderEntityData.SetupUnit` post-loads them): slot
    /// queries see empty slots while the unit's facts turn on.
    pub body_pending: bool,
}

/// `UnitProgressionData.CharacterLevel` (non-mythic class levels).
#[must_use]
pub fn character_level(w: &World, u: EntityIdx) -> i32 {
    let unit = w.unit(u).expect("unit");
    if let Some(l) = unit.progression.character_level {
        return l;
    }
    levels(w, u).0
}

/// `MythicLevel`.
#[must_use]
pub fn mythic_level(w: &World, u: EntityIdx) -> i32 {
    let unit = w.unit(u).expect("unit");
    if let Some(l) = unit.progression.mythic_level {
        return l;
    }
    levels(w, u).1
}

/// `SetupLevelsIfNecessary()`: unless both caches are set, both are computed from the
/// classes and stored.
pub fn setup_levels_if_necessary(w: &mut World, u: EntityIdx) {
    let p = &w.unit(u).expect("unit").progression;
    if p.character_level.is_some() && p.mythic_level.is_some() {
        return;
    }
    let (c, m) = levels(w, u);
    let p = &mut w.unit_mut(u).expect("unit").progression;
    p.character_level = Some(c);
    p.mythic_level = Some(m);
}

fn levels(w: &World, u: EntityIdx) -> (i32, i32) {
    let unit = w.unit(u).expect("unit");
    let (mut c, mut m) = (0i32, 0i32);
    for cd in &unit.progression.classes {
        let mythic = w.content.get(cd.class).is_some_and(|b| b.view().bool("IsMythic"));
        if mythic {
            m = m.wrapping_add(cd.level);
        } else {
            c = c.wrapping_add(cd.level);
        }
    }
    (c, m)
}

/// `ModifiableValueHitPoints.CalculateSizeBonus(OriginalSize)`.
pub fn hp_size_bonus(w: &mut World, u: EntityIdx) -> i32 {
    match w.unit(u).and_then(|x| x.original_size) {
        Some(Size::Tiny) => 0,
        Some(Size::Small) => 10,
        Some(Size::Medium) => 20,
        Some(Size::Large) => 30,
        Some(Size::Huge) => 40,
        Some(Size::Gargantuan) => 60,
        Some(Size::Colossal) => 80,
        _ => {
            // `ArgumentOutOfRangeException`, caught around `UpdateValue`.
            w.coverage
                .unsupported("ModifiableValueHitPoints.CalculateSizeBonus: size out of range");
            0
        }
    }
}

/// `BlueprintRoot` (`Root/BlueprintRoot`).
pub const BLUEPRINT_ROOT: Guid = Guid(0x2d77_316c_72b9_ed44_f888_ceef_c2a1_31f6);

/// The favoured-class part of `ModifiableValueHitPoints.UpdateInternalModifiers`: the
/// highest level among non-prestige classes listed in
/// `BlueprintRoot.Progression.AvailableCharacterClasses`.
pub fn favoured_class_level(w: &mut World, u: EntityIdx) -> i32 {
    let classes = w.unit(u).expect("unit").progression.classes.clone();
    if classes.is_empty() {
        return 0;
    }
    let available = available_character_classes(w);
    let mut best = 0;
    for cd in classes {
        let prestige = w.content.get(cd.class).is_some_and(|b| b.view().bool("PrestigeClass"));
        if cd.level > best && !prestige && available.contains(&cd.class) {
            best = cd.level;
        }
    }
    best
}

fn available_character_classes(w: &mut World) -> Vec<Guid> {
    let Some(root) = w.content.get(BLUEPRINT_ROOT) else {
        w.coverage.unsupported("BlueprintRoot not found");
        return Vec::new();
    };
    let Some(p) = root.view().obj("Progression") else {
        w.coverage.unsupported("BlueprintRoot.Progression not found");
        return Vec::new();
    };
    // `AvailableCharacterClasses`: `m_CharacterClasses` resolved, nulls dropped, minus DLC-restricted
    // classes (`IsDlcRestricted`; assumed none: Ember owns the DLCs [inferred]).
    p.guids("m_CharacterClasses").into_iter().filter(|g| w.content.get(*g).is_some()).collect()
}

/// `FeatureCollection.CheckStatPrerequisitesAndUpdate()` (via
/// `UnitProgressionData.CheckStatPrerequisitesAndUpdate`): for each feature, the stat
/// prerequisites of its blueprint (non-`Any` group, and checked in progression or the
/// feature has no source progression) decide `DisabledBecauseOfPrerequisites`; then
/// `UpdateIsActive`.
pub fn check_stat_prerequisites_and_update(w: &mut World, u: EntityIdx) {
    let is_pet = w.unit(u).is_some_and(|x| x.is_pet);
    let features: Vec<FactIdx> = w
        .entity(u)
        .facts
        .list
        .iter()
        .copied()
        .filter(|f| w.fact(*f).kind == FactKind::Feature)
        .collect();
    for f in features {
        let mut has = false;
        let mut ok = true;
        if !w.fact(f).ignore_prerequisites && !is_pet {
            let source_is_progression = w
                .fact(f)
                .source_blueprint
                .and_then(|g| w.content.get(g))
                .is_some_and(|b| crate::content::is_a(b.obj.ty, "BlueprintProgression"));
            let comps: Vec<_> = w
                .fact(f)
                .bp
                .as_ref()
                .map(|b| b.components_array().into_iter().cloned().collect())
                .unwrap_or_default();
            for o in &comps {
                let v = crate::content::View(o);
                let is_stat = v.is("PrerequisiteStatValue");
                let is_full = v.is("PrerequisiteFullStatValue");
                if !(is_stat || is_full) || !v.is("Prerequisite") {
                    continue;
                }
                let group_any = v.int("Group") == 1;
                if group_any || !(v.bool("CheckInProgression") || !source_is_progression) {
                    continue;
                }
                has = true;
                let stat = StatType(v.i32("Stat"));
                let need = v.i32("Value");
                let value = if is_stat {
                    stat_value_for_prerequisite(w, u, stat)
                } else {
                    full_stat_value(w, u, stat)
                };
                ok &= value >= need;
            }
        }
        w.fact_mut(f).disabled_because_of_prerequisites = has && !ok;
        crate::facts::lifecycle::update_is_active(w, f);
    }
}

fn replace_stat_components_present(w: &mut World, u: EntityIdx) {
    let any = w.entity(u).facts.list.iter().any(|f| {
        let fact = w.fact(*f);
        fact.kind == FactKind::Feature
            && fact.components.iter().any(|c| {
                c.bp_component()
                    .is_some_and(|o| crate::content::is_a(o.ty, "ReplaceStatForPrerequisites"))
            })
    });
    if any {
        w.coverage.unsupported("ReplaceStatForPrerequisites (prerequisite stat replacement)");
    }
}

fn stat_value_for_prerequisite(w: &mut World, u: EntityIdx, stat: StatType) -> i32 {
    replace_stat_components_present(w, u);
    let s = w.stats(u).v(stat);
    if stat.is_skill() { s.base_value } else { s.permanent_value }
}

fn full_stat_value(w: &mut World, u: EntityIdx, stat: StatType) -> i32 {
    replace_stat_components_present(w, u);
    let s = w.stats(u).v(stat);
    let feat: i32 = s
        .modifiers()
        .filter(|m| w.mods[m.i()].desc == crate::stats::types::ModifierDescriptor::Feat)
        .fold(0i32, |a, m| a.wrapping_add(w.mods[m.i()].value));
    s.permanent_value.wrapping_add(feat)
}

/// `UnitBody.AllSlots` items then `AllEmptyHands` (`UnitBody.cs` `Body.PostLoad`,
/// `GetAllItemsInternal`): equipment slots, additional limbs, then the empty-hand weapon.
/// Polymorph slots and the empty-hand stack are reported when hydrated.
pub fn body_items(w: &World, u: EntityIdx) -> Vec<EntityIdx> {
    let Some(unit) = w.unit(u) else { return Vec::new() };
    let body = &unit.body;
    body.slots
        .iter()
        .chain(body.limbs.iter())
        .filter_map(|s| s.item)
        .chain(body.empty_hand)
        .chain(body.empty_hand_stack.iter().copied())
        .collect()
}

/// `UnitDescriptor.TurnOn()`: stat prerequisites, then the inventory (if the unit owns
/// it), then the body's items, then the brain.
pub fn descriptor_turn_on(w: &mut World, u: EntityIdx) {
    w.unit_mut(u).expect("unit").descriptor_turned_on = true;
    check_stat_prerequisites_and_update(w, u);
    let (own, inv) = {
        let unit = w.unit(u).expect("unit");
        (unit.has_own_inventory, unit.inventory.clone())
    };
    let body_items = body_items(w, u);
    if own {
        for it in inv {
            if !w.entity(it).is_turned_on {
                crate::entity::turn_on(w, it);
            }
        }
    }
    // `UnitBody.GetAllItemsInternal()`: equipment slots, additional limbs, empty hands.
    for it in body_items {
        if !w.entity(it).is_turned_on {
            crate::entity::turn_on(w, it);
        }
    }
    // `UnitBrain.TurnOn`: AI state; nothing on the character sheet.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{Entity, EntityKind, FactsManager};

    fn item(w: &mut World, id: &str) -> EntityIdx {
        w.add_entity(Entity {
            unique_id: id.into(),
            clr: "ItemEntityWeapon",
            kind: EntityKind::Item(Box::default()),
            facts: FactsManager::default(),
            parts: Vec::new(),
            is_turned_on: false,
            post_load_executed: false,
            is_in_game: true,
            subscribed: false,
            disposing: false,
            is_disposed: false,
        })
    }

    fn slot(name: &str, item: Option<EntityIdx>) -> Slot {
        Slot { name: name.into(), item, active: true }
    }

    /// `UnitBody.AllSlots` (equipment slots, then additional limbs) and `AllEmptyHands`
    /// feed `PostLoad` and `TurnOn`; weapons outside the slots are not skipped.
    #[test]
    fn body_items_follow_slots_limbs_then_empty_hand() {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        let (a, l0, l1, e) =
            (item(&mut w, "a"), item(&mut w, "l0"), item(&mut w, "l1"), item(&mut w, "e"));
        w.unit_mut(u).unwrap().body = Body {
            slots: vec![slot("Armor", Some(a)), slot("Belt", None)],
            limbs: vec![
                slot("AdditionalLimbs[0]", Some(l0)),
                slot("AdditionalLimbs[1]", None),
                slot("AdditionalLimbs[2]", Some(l1)),
            ],
            empty_hand: Some(e),
            ..Body::default()
        };
        assert_eq!(body_items(&w, u), vec![a, l0, l1, e]);
        descriptor_turn_on(&mut w, u);
        for it in [a, l0, l1, e] {
            assert!(w.entity(it).is_turned_on);
        }
    }
}
