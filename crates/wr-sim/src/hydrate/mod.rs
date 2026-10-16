//! Hydrating a save: from `wr-save`'s decoded documents to a live [`World`], running the
//! post-load lifecycle in the original's order (`cache/research/poc3-lifecycle.md`):
//!
//! 1. **Deserialization** (Newtonsoft populating objects): entities, facts (the
//!    `Components` dictionary setter creating runtimes for null entries), stats (the
//!    `PersistentModifierList` setter refilling descriptor buckets and `AppliedTo`), items.
//!    Shared objects (`$ref`) become one sim object.
//! 2. **`Player.PostLoad`** → `CrossSceneState.PostLoad` (per-scene barriers: every
//!    entity's `PrePostLoad`, then every `PostLoad`, then every `ApplyPostLoadFixes`; the
//!    cross-scene state skips its inventory passes), then the player inventory's
//!    `PostLoad` (each item's `PostLoad`), then the other cross-scene units' inventories.
//! 3. **`Player.TurnOn`** (its `OnTurnOn` turns the inventory's items on), then
//!    `CrossSceneState.TurnOn` (each party unit).
//! 4. **The area**: `AreaPersistentState.PostLoad` → the main scene state's `PostLoad`
//!    (barriers, with unit inventories' `PostLoad` and item fixes), then its `TurnOn`.
//!
//! Only full `UnitEntityData` entities (and their items) are hydrated. Placeholders
//! (rebuilt from blueprints by `BecomeUnit`), spawners, map objects, script zones and
//! cutscene players are counted and reported as not modelled.

mod books;
pub mod graph;
pub mod narrative;
mod placeholders;
mod scene;

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::{Context as _, Result, bail};
use wr_save::value::Value;
use wr_save::zks;

use self::graph::{DocIndex, O, guid_of, int_of, str_of};
use crate::content::Content;
use crate::csharp::TimeSpan;
use crate::item::Item;
use crate::registry::Registry;
use crate::stats::modifier::Modifier;
use crate::stats::types::{
    BonusMod, HitPointsBaseStat, ModifierDescriptor, Size, StackMode, StatType,
};
use crate::stats::value::{AcState, DexBonusLimiter, ModList, StatKind, StatValue, Stats};
use crate::unit::{Body, ClassData, Progression, Slot, Unit};
use crate::world::{
    Component, Entity, EntityIdx, EntityKind, Fact, FactIdx, FactKind, FactsManager, ModIdx, Part,
    World,
};

/// A decoded save.
#[derive(Debug)]
pub struct Save {
    pub name: String,
    pub entries: Vec<zks::Decoded>,
}

impl Save {
    /// Reads and decodes a `.zks` (read only).
    ///
    /// # Errors
    ///
    /// I/O, a save the game would refuse (`check_versions`), or a document that doesn't
    /// decode.
    pub fn read(path: &std::path::Path) -> Result<Self> {
        let raw = zks::read(path)?;
        zks::check_versions(&raw)?;
        let entries = zks::decode_all(&raw);
        for e in &entries {
            if let Some(err) = &e.error {
                bail!("{}: {err}", e.name);
            }
        }
        Ok(Self {
            name: path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            entries,
        })
    }

    #[must_use]
    pub fn doc(&self, name: &str) -> Option<&wr_save::Doc> {
        self.entries.iter().find(|e| e.name == name).and_then(|e| match &e.body {
            zks::Body::Doc(d) => Some(d),
            zks::Body::Bytes(_) => None,
        })
    }
}

/// What hydration did, for the report.
#[derive(Debug, Default, serde::Serialize)]
pub struct HydrateReport {
    pub area: Option<String>,
    pub party_units: Vec<String>,
    pub area_units: Vec<String>,
    /// Area entities not hydrated, by CLR type.
    pub not_hydrated: std::collections::BTreeMap<String, u64>,
    pub facts: usize,
    pub items: usize,
    pub modifiers: usize,
}

struct Loader<'a> {
    w: World,
    facts: HashMap<usize, FactIdx>,
    mods: HashMap<usize, ModIdx>,
    items: HashMap<usize, EntityIdx>,
    /// Stats seen so far by object identity: (unit, type).
    stats: HashMap<usize, StatType>,
    root: Option<Rc<crate::content::Blueprint>>,
    /// Slots whose item is named by `UniqueId`: (unit, slot index, item id).
    pending_slots_all: Vec<(EntityIdx, usize, String)>,
    /// Saved additional-limb items: (unit, limb index, item id).
    pending_limbs: Vec<(EntityIdx, usize, String)>,
    /// Pets' `UnitPartPet.m_MasterRef`: (pet, master's unique id).
    pending_masters: Vec<(EntityIdx, String)>,
    _p: std::marker::PhantomData<&'a ()>,
}

fn to_json(v: &Value, ix: &DocIndex<'_>) -> Option<serde_json::Value> {
    let mut s = String::new();
    wr_save::canon::write_value(v, &ix.doc.tags, &mut s);
    serde_json::from_str(&s).ok()
}

/// A context's saved `m_Params` (a `$ref` resolved), `None` when null or absent. Members at
/// their default (0) are left out of the save (`DefaultValueHandling.IgnoreAndPopulate`).
fn own_params(o: O<'_>) -> Option<serde_json::Value> {
    let p = o.obj("m_Params")?;
    let mut m = serde_json::Map::new();
    for k in [
        "SpellSource",
        "CasterLevel",
        "SpellLevel",
        "DC",
        "Concentration",
        "Metamagic",
        "RankBonus",
    ] {
        if let Some(v) = p.int(k) {
            m.insert(k.into(), v.into());
        }
    }
    Some(serde_json::Value::Object(m))
}

/// `ParentContext?.Params`: the first saved `m_Params` up the parent chain.
fn parent_params(o: O<'_>) -> Option<serde_json::Value> {
    let mut cur = o.obj("ParentContext");
    for _ in 0..32 {
        let c = cur?;
        if let Some(p) = own_params(c) {
            return Some(p);
        }
        cur = c.obj("ParentContext");
    }
    None
}

fn fact_kind(clr: &str) -> FactKind {
    match clr {
        "Feature" => FactKind::Feature,
        "Buff" => FactKind::Buff,
        "Ability" => FactKind::Ability,
        "ActivatableAbility" => FactKind::ActivatableAbility,
        "ItemEnchantment" => FactKind::ItemEnchantment,
        "MainUnitFact" => FactKind::MainUnitFact,
        "UnitFact" => FactKind::UnitFact,
        "Etude" => FactKind::Etude,
        _ => FactKind::Other,
    }
}

impl Loader<'_> {
    fn fact_for(&mut self, o: O<'_>) -> FactIdx {
        if let Some(&f) = self.facts.get(&o.id()) {
            return f;
        }
        let clr = o.ty();
        let mut fact = Fact::blank(fact_kind(clr), clr);
        fact.blueprint = o.guid("Blueprint");
        fact.bp = fact.blueprint.and_then(|g| self.w.content.get(g));
        fact.unique_id = o.str("UniqueId").unwrap_or_default().to_owned();
        fact.attach_time = TimeSpan(o.timespan("AttachTime").unwrap_or(0));
        fact.is_active = o.bool_or("IsActive", false);
        fact.is_disposed = o.bool_or("IsDisposed", false);
        fact.rank = o.i32_or("Rank", 1);
        fact.ignore_prerequisites = o.bool_or("IgnorePrerequisites", true);
        fact.disabled_because_of_prerequisites = o.bool_or("DisabledBecauseOfPrerequisites", false);
        fact.source_blueprint = o.guid("Source");
        fact.is_suppressed = o.bool_or("IsSuppressed", false);
        fact.parent_context = clr == "ItemEnchantment" && o.obj("ParentContext").is_some();
        fact.param = o.get("Param").and_then(|v| to_json(v, o.ix)).filter(|v| !v.is_null());
        fact.ability_saved_extras = clr == "Ability"
            && ["UsagesPerDayResource", "StickyTouch"]
                .iter()
                .any(|k| o.get(k).is_some_and(|v| !matches!(v, wr_save::value::Value::Null)));
        if clr == "Buff" {
            fact.buff = Some(crate::world::BuffTimes {
                end_time: o.timespan("m_EndTime"),
                tick_time: o.timespan("TickTime").unwrap_or(0),
                next_tick_time: o.timespan("NextTickTime").unwrap_or(0),
                round_number: o.int("RoundNumber").unwrap_or(0),
                is_from_spell: o.bool_or("IsFromSpell", false),
                source_area_effect: o
                    .str("SourceAreaEffectId")
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
                stored_facts: Vec::new(),
                stored_mods: !o.objs("m_StoredMods").is_empty(),
            });
        }
        let f = self.w.add_fact(fact);
        self.facts.insert(o.id(), f);
        if clr == "Buff" {
            // `m_StoredFacts`: references to the owner's facts (nulls skipped: `OnRemove`'s
            // `RemoveFact(null)` does nothing).
            let stored: Vec<FactIdx> =
                o.objs("m_StoredFacts").into_iter().map(|x| self.fact_for(x)).collect();
            if let Some(b) = self.w.fact_mut(f).buff.as_mut() {
                b.stored_facts = stored;
            }
        }
        // References that can point back at this fact are resolved after registering it.
        if let Some(ctx) = o.obj("m_Context") {
            let c = self.context(ctx);
            self.w.fact_mut(f).context = Some(c);
        }
        // `ItemEnchantment.ParentContext` (saved).
        if clr == "ItemEnchantment"
            && let Some(pc) = o.obj("ParentContext")
        {
            let c = self.context(pc);
            self.w.fact_mut(f).enchant_parent = Some(Box::new(c));
        }
        if let Some(si) = o.obj("SourceItem") {
            let it = self.item_for(si);
            self.w.fact_mut(f).source_item = Some(it);
        }
        if clr == "ActivatableAbility" {
            let applied = o.obj("m_AppliedBuff").map(|b| self.fact_for(b));
            self.w.fact_mut(f).activatable =
                Some((o.bool_or("m_IsOn", false), o.bool_or("IsStarted", false), applied));
            self.w.fact_mut(f).ready_to_start = o.bool_or("ReadyToStart", false);
            let target = o.obj("SelectTargetAbility").map(|a| self.fact_for(a));
            self.w.fact_mut(f).select_target = target;
            self.w.fact_mut(f).was_in_combat = o.bool_or("m_WasInCombat", false);
            self.w.fact_mut(f).time_to_next_round = match o.get("TimeToNextRound") {
                Some(wr_save::value::Value::F32(x)) => *x,
                _ => 0.0,
            };
        }
        if let Some(sf) = o.obj("SourceFact") {
            let s = self.fact_for(sf);
            self.w.fact_mut(f).source_fact = Some(s);
        }
        self.components(f, o);
        f
    }

    /// The `ComponentsDictionary` setter (and the legacy `m_Components` list).
    fn components(&mut self, f: FactIdx, o: O<'_>) {
        if self.w.fact(f).bp.is_none() {
            if o.has("Components") {
                self.w.log("error", "Can't deserialize fact because blueprint is null");
            }
            return;
        }
        let bcs = self.w.blueprint_components(f);
        for (k, v) in o.dict("Components") {
            let Some(name) = str_of(k).map(str::to_owned) else { continue };
            let saved = o.wrap(v);
            let comp = match saved {
                Some(rt) => Some(Component {
                    name: name.clone(),
                    source: None,
                    saved_clr: o.ix.doc.tags.get(rt.obj.tag as usize).map(ToString::to_string),
                    data: rt.get("m_Data").and_then(|d| to_json(d, o.ix)).filter(|d| !d.is_null()),
                    is_turned_on: false,
                    is_disposed: false,
                    subscribed: false,
                    fact_refs: rt.obj("m_Data").map(|d| self.data_fact_refs(d)).unwrap_or_default(),
                    runtime: runtime_members(rt),
                    need_resume: false,
                }),
                None => {
                    // Null: create a runtime for an enabled runtime-provider component of
                    // that name.
                    let found = bcs.iter().find(|(bp, i)| {
                        crate::content::View(bp.components_array()[*i]).str("name")
                            == Some(name.as_str())
                    });
                    found.and_then(|(bp, i)| {
                        let obj = bp.components_array()[*i];
                        let t = wr_content::schema::ty(obj.ty);
                        let disabled = crate::content::View(obj).int("m_Flags") & 1 != 0;
                        (t.has(wr_content::schema::type_flags::RUNTIME_PROVIDER) && !disabled).then(
                            || Component {
                                name: name.clone(),
                                source: None,
                                saved_clr: None,
                                data: None,
                                is_turned_on: false,
                                is_disposed: false,
                                subscribed: false,
                                fact_refs: HashMap::new(),
                                runtime: serde_json::Map::new(),
                                need_resume: false,
                            },
                        )
                    })
                }
            };
            if let Some(c) = comp {
                self.w.fact_mut(f).components.push(c);
            }
        }
        if o.has("m_Components") {
            self.w.coverage.unsupported("EntityFact.m_Components (legacy component list)");
        }
    }

    /// Facts referenced by a component's saved data, by member.
    fn data_fact_refs(&mut self, d: O<'_>) -> HashMap<String, Vec<Option<FactIdx>>> {
        let mut out = HashMap::new();
        let t = wr_save::schema::ty(d.obj.ty);
        for p in &d.obj.props {
            let (wr_save::value::Key::Member(i) | wr_save::value::Key::MemberCase(i, _)) = &p.key
            else {
                continue;
            };
            let Some(m) = t.members.get(usize::from(*i)) else { continue };
            match &p.value {
                Value::Seq(items) => {
                    let objs: Vec<Option<O<'_>>> = items.iter().map(|v| d.wrap(v)).collect();
                    if objs.iter().flatten().any(|x| x.is("EntityFact")) {
                        let v = objs
                            .into_iter()
                            .map(|x| x.filter(|x| x.is("EntityFact")).map(|x| self.fact_for(x)))
                            .collect();
                        out.insert(m.json.to_owned(), v);
                    }
                }
                v => {
                    if let Some(x) = d.wrap(v).filter(|x| x.is("EntityFact")) {
                        let f = self.fact_for(x);
                        out.insert(m.json.to_owned(), vec![Some(f)]);
                    }
                }
            }
        }
        out
    }

    fn context(&mut self, o: O<'_>) -> crate::context::MechanicsContext {
        let ints = |name: &str| -> Vec<i32> {
            match o.get(name) {
                Some(Value::Seq(v)) => v
                    .iter()
                    .map(|x| int_of(x).and_then(|i| i32::try_from(i).ok()).unwrap_or(0))
                    .collect(),
                _ => Vec::new(),
            }
        };
        let owner = o.str("m_OwnerRef").map(str::to_owned);
        // `[OnDeserialized]`: a missing caster reads as the owner.
        let caster = o.str("m_CasterRef").map(str::to_owned).or_else(|| owner.clone());
        // `EntityRef`s resolve lazily in the C#: keep the ids.
        let parent_source_item =
            o.obj("ParentContext").and_then(|p| p.str("m_SourceItemRef")).map(str::to_owned);
        crate::context::MechanicsContext {
            associated_blueprint: o.guid("AssociatedBlueprint"),
            parent_associated_blueprint: o
                .obj("ParentContext")
                .and_then(|p| p.guid("AssociatedBlueprint")),
            owner,
            caster,
            ranks: ints("m_Ranks"),
            shared_values: ints("m_SharedValues"),
            params: own_params(o),
            parent_params: parent_params(o),
            source_item: o.str("m_SourceItemRef").map(str::to_owned),
            parent_source_item,
            spell_descriptor: o.int("SpellDescriptor").unwrap_or(0),
            has_ability_context: {
                let mut cur = Some(o);
                let mut found = false;
                for _ in 0..32 {
                    let Some(c) = cur else { break };
                    if c.is("AbilityExecutionContext") {
                        found = true;
                        break;
                    }
                    cur = c.obj("ParentContext");
                }
                found
            },
            source_ability: None,
        }
    }

    fn mod_for(&mut self, o: O<'_>) -> ModIdx {
        if let Some(&m) = self.mods.get(&o.id()) {
            return m;
        }
        let m = self.w.add_mod(Modifier {
            applied_to: None,
            desc: ModifierDescriptor(o.i32_or("ModDescriptor", 0)),
            stack_mode: StackMode(o.i32_or("StackMode", 0)),
            value: o.i32_or("ModValue", 0),
            source: None,
            source_component: o.str("SourceComponent").map(str::to_owned),
            item_source: None,
        });
        self.mods.insert(o.id(), m);
        if let Some(s) = o.obj("Source") {
            let f = self.fact_for(s);
            self.w.mods[m.i()].source = Some(f);
        }
        if let Some(i) = o.obj("ItemSource") {
            let it = self.item_for(i);
            self.w.mods[m.i()].item_source = Some(it);
        }
        m
    }

    fn item_for(&mut self, o: O<'_>) -> EntityIdx {
        if let Some(&e) = self.items.get(&o.id()) {
            return e;
        }
        let uid = o.str("UniqueId").unwrap_or_default().to_owned();
        let item = Item {
            blueprint: o.guid("m_Blueprint"),
            count: o.i32_or("m_Count", 1),
            charges: o.i32_or("Charges", 0),
            is_identified: o.bool_or("IsIdentified", false),
            wielder: None,
            collection_owner: None,
            holding_slot: None,
            collection: o.obj("Collection").map(|c| c.id()),
            modifiers: Vec::new(),
            ability: None,
            activatable_ability: None,
            facts_applied_to_wielder: !o.objs("m_FactsAppliedToWielder").is_empty(),
            armor_component: None,
            weapon_component: None,
            shield: None,
            dex_limiter: o.obj("m_DexBonusLimeterAC").map(|l| limiter_id(l)),
            force_secondary: o.bool_or("ForceSecondary", false),
            is_non_removable: o.bool_or("IsNonRemovable", false),
            force_stackable: match o.get("ForceStackable") {
                Some(Value::Bool(b)) => Some(*b),
                _ => None,
            },
            runtime_created: false,
            visual_source: o.guid("VisualSourceItemBlueprint"),
            remove_from_slot_in_progress: false,
            not_lootable: o.bool_or("m_NotLootable", false),
            skinning_successful: o.bool_or("m_SkinningSuccessful", false),
            inventory_slot_index: Some(o.i32_or("m_InventorySlotIndex", -1)),
        };
        let e = self.w.add_entity(Entity {
            unique_id: uid,
            clr: o.ty(),
            kind: EntityKind::Item(Box::new(item)),
            facts: FactsManager::default(),
            parts: parts_of(o),
            is_turned_on: false,
            post_load_executed: false,
            is_in_game: o.bool_or("m_IsInGame", true),
            subscribed: false,
            disposing: false,
            is_disposed: false,
        });
        self.items.insert(o.id(), e);
        // Registered with `EntityService` by its `PrePostLoad` only.
        self.w.unregistered_items.insert(e);
        let facts = o.obj("Facts").map(|f| f.objs("m_Facts")).unwrap_or_default();
        let list: Vec<FactIdx> = facts.into_iter().map(|f| self.fact_for(f)).collect();
        let cap = crate::csharp::sort::grown_capacity(list.len());
        self.w.entity_mut(e).facts = FactsManager { list, capacity: cap, is_turned_on: false };
        let mods: Vec<ModIdx> =
            o.objs("m_Modifiers").into_iter().map(|m| self.mod_for(m)).collect();
        let ability = o.obj("Ability").map(|a| self.fact_for(a));
        let armor = o.obj("ArmorComponent").map(|a| self.item_for(a));
        let weapon = o.obj("WeaponComponent").map(|a| self.item_for(a));
        for c in [armor, weapon].into_iter().flatten() {
            if let Some(it) = self.w.item_mut(c) {
                it.shield = Some(e);
            }
        }
        let activatable = o.obj("ActivatableAbility").map(|a| self.fact_for(a));
        if let Some(it) = self.w.item_mut(e) {
            it.modifiers = mods;
            it.ability = ability;
            it.activatable_ability = activatable;
            it.armor_component = armor;
            it.weapon_component = weapon;
        }
        e
    }

    /// Second pass over items once units exist: wielder and collection owner.
    fn link_items(&mut self, ixs: &[&DocIndex<'_>]) {
        let by_obj: Vec<(usize, EntityIdx)> = self.items.iter().map(|(k, v)| (*k, *v)).collect();
        for ix in ixs {
            ix.doc.root.walk(&mut |v| {
                if let Value::Obj(ob) = v
                    && let Some(&(_, e)) = by_obj.iter().find(|(k, _)| *k == graph::identity(ob))
                {
                    let o = ix.o(ob);
                    let wielder =
                        o.str("m_WielderRef").and_then(|id| self.w.by_id.get(id).copied());
                    let owner = o
                        .obj("Collection")
                        .and_then(|c| c.str("OwnerRef"))
                        .and_then(|id| self.w.by_id.get(id).copied());
                    if let Some(it) = self.w.item_mut(e) {
                        it.wielder = wielder;
                        it.collection_owner = owner;
                    }
                }
            });
        }
    }

    fn stat_value(&mut self, u: EntityIdx, o: O<'_>) -> StatValue {
        let ty = StatType(o.i32_or("Type", 0));
        let st = |name: &str| -> StatType {
            o.obj(name).map_or(StatType::Unknown, |s| StatType(s.i32_or("Type", 0)))
        };
        let kind = match o.ty() {
            "ModifiableValueAttributeStat" => StatKind::Attribute {
                disabled: o.int("Disabled").and_then(|v| i32::try_from(v).ok()).unwrap_or(0),
                damage: None,
                drain: None,
            },
            "ModifiableValueSkill" => {
                let base = st("m_BaseAttribute");
                StatKind::Skill {
                    base_attribute: base,
                    current_base: base,
                    replace: StatType(o.i32_or("m_ReplaceBaseAttributeType", 0)),
                    replace_if_higher: o.bool_or("m_ReplaceAttributeIfHigher", false),
                    background_modifier: o
                        .obj("m_BackgroundSkillModifier")
                        .map(|m| self.mod_for(m)),
                    class_skill: o
                        .int("ClassSkill")
                        .and_then(|v| i32::try_from(v).ok())
                        .unwrap_or(0),
                    background_source: o.obj("BackgroundSkillSource").map(|f| self.fact_for(f)),
                }
            }
            "ModifiableValueSavingThrow" => StatKind::SavingThrow { base_stat: st("BaseStat") },
            "ModifiableValueDependant" => StatKind::Dependant { base_stat: st("BaseStat") },
            "ModifiableValueInitiative" => StatKind::Initiative {
                dexterity: st("m_Dexterity"),
                dex_bonus: o.obj("m_DexBonus").map(|m| self.mod_for(m)),
                min_value: o.i32_or("m_MinValue", i32::MIN),
            },
            "ModifiableValueHitPoints" => StatKind::HitPoints {
                base_stat: match o.int("m_BaseStat") {
                    Some(1) => HitPointsBaseStat::Charisma,
                    Some(2) => HitPointsBaseStat::Size,
                    _ => HitPointsBaseStat::Constitution,
                },
            },
            "ModifiableValueTemporaryHitPoints" => StatKind::TemporaryHitPoints,
            "ModifiableValueSpeed" => StatKind::Speed { racial: 0 },
            "ModifiableValueArmorClass" => {
                let lim = o.get("m_BaseAttributeBonusLimiters").map(|_| {
                    o.objs("m_BaseAttributeBonusLimiters")
                        .into_iter()
                        .map(|l| DexBonusLimiter {
                            value: l.i32_or("Value", 0),
                            source: l.i32_or("Source", 0),
                            id: limiter_id(l),
                        })
                        .collect()
                });
                StatKind::ArmorClass(Box::new(AcState {
                    limiters: lim,
                    applied_bonus: o.obj("m_BaseAttributeAppliedBonus").map(|m| self.mod_for(m)),
                    replace: StatType(o.i32_or("m_ReplaceBaseAttributeType", 0)),
                    replace_if_higher: o.bool_or("m_ReplaceAttributeIfHigher", false),
                    replace_bonus_mod: match o.int("m_ReplaceBaseAttributeBonusMod") {
                        Some(1) => BonusMod::Div2,
                        Some(2) => BonusMod::OnePlusDiv2,
                        Some(3) => BonusMod::Custom,
                        _ => BonusMod::AsIs,
                    },
                    ..AcState::default()
                }))
            }
            "ModifiableValue" => StatKind::Plain,
            other => {
                self.w.coverage.unsupported(format!("stat class {other}"));
                StatKind::Plain
            }
        };
        let mut s = StatValue::new(ty, kind);
        s.base_value = o.i32_or("m_BaseValue", 0);
        s.permanent_value = o.i32_or("PermanentValue", 0);
        // `PersistentModifierList` setter.
        for item in o.items("PersistentModifierList") {
            let Some(mo) = item else {
                self.w.log(
                    "error",
                    "ModifiableValue.PersistentModifierList: modifier in null on load",
                );
                continue;
            };
            let m = self.mod_for(mo);
            let d = self.w.mods[m.i()].desc;
            s.modifier_list.get_or_insert_with(d, ModList::default).push(m);
            self.w.mods[m.i()].applied_to = Some((u, ty));
        }
        if o.has("m_Dependents") {
            let deps: Vec<StatType> =
                o.objs("m_Dependents").iter().map(|d| StatType(d.i32_or("Type", 0))).collect();
            s.dependents = Some(deps);
        }
        self.stats.insert(o.id(), ty);
        s
    }

    fn unit(&mut self, o: O<'_>, main_id: Option<&str>) -> Result<EntityIdx> {
        let d = o.obj("Descriptor").context("unit without Descriptor")?;
        let uid = o.str("UniqueId").unwrap_or_default().to_owned();
        let pet_master = o
            .obj("Parts")
            .map(|p| p.objs("m_Parts"))
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.ty() == "UnitPartPet")
            .map(|p| {
                p.obj("m_MasterRef").and_then(|r| r.str("m_Ref")).unwrap_or_default().to_owned()
            });
        let e = self.w.add_entity(Entity {
            unique_id: uid.clone(),
            clr: o.ty(),
            kind: EntityKind::Unit(Box::default()),
            facts: FactsManager::default(),
            parts: parts_of(o),
            is_turned_on: false,
            post_load_executed: false,
            is_in_game: o.bool_or("m_IsInGame", true),
            subscribed: false,
            disposing: false,
            is_disposed: false,
        });
        // A saved roaming state that would start a cutscene the roaming controller plays (one
        // mid-way, or a next point of its own with an idle cutscene) isn't modelled; otherwise
        // roaming only moves the unit (T1).
        if let Some(r) = o
            .obj("Parts")
            .map(|p| p.objs("m_Parts"))
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.ty() == "UnitPartRoaming")
        {
            if r.obj("IdleCutscene").is_some() {
                self.w.coverage.unsupported("UnitPartRoaming: an idle cutscene playing");
            }
            if graph::roaming_next_idle_cutscene(r) {
                self.w.coverage.unsupported(crate::spawn::ROAMING_NEXT_IDLE_CUTSCENE);
            }
        }
        // Items first (modifiers and facts point at them), then facts, then stats.
        let inv = d.obj("m_Inventory");
        let inv_items: Vec<EntityIdx> = inv
            .map(|c| c.objs("m_Items"))
            .unwrap_or_default()
            .into_iter()
            .map(|i| self.item_for(i))
            .collect();
        let facts = o.obj("Facts").map(|f| f.objs("m_Facts")).unwrap_or_default();
        let list: Vec<FactIdx> = facts.into_iter().map(|f| self.fact_for(f)).collect();
        let cap = crate::csharp::sort::grown_capacity(list.len());
        self.w.entity_mut(e).facts = FactsManager { list, capacity: cap, is_turned_on: false };
        let stats_o = d.obj("Stats").context("descriptor without Stats")?;
        let mut stats = Stats::default();
        for t in StatType::ALL {
            match stats_o.obj(&t.name()) {
                Some(so) => {
                    let v = self.stat_value(e, so);
                    stats.all.push(v);
                }
                None => {
                    // Not in the save: the JSON constructor's own instance (only
                    // `BonusCasterLevel` gets one) or nothing.
                    let mut v = StatValue::new(t, StatKind::Plain);
                    if t != StatType::BonusCasterLevel {
                        self.w.coverage.unsupported(format!("stat {} missing from save", t.name()));
                    }
                    v.bound = true;
                    stats.all.push(v);
                }
            }
        }
        let prog = d.obj("Progression");
        let progression = Progression {
            race: prog.and_then(|p| p.guid("m_Race")),
            classes: prog
                .map(|p| p.objs("Classes"))
                .unwrap_or_default()
                .into_iter()
                .filter_map(|c| {
                    Some(ClassData {
                        class: c.guid("CharacterClass")?,
                        level: c.i32_or("Level", 0),
                        archetypes: match c.get("Archetypes") {
                            Some(Value::Seq(v)) => v.iter().filter_map(guid_of).collect(),
                            _ => Vec::new(),
                        },
                        spellbook: c.guid("Spellbook"),
                    })
                })
                .collect(),
            experience: prog.and_then(|p| p.int("Experience")).unwrap_or(0),
            mythic_experience: prog
                .and_then(|p| p.int("MythicExperience"))
                .and_then(|x| i32::try_from(x).ok())
                .unwrap_or(0),
            character_level: None,
            mythic_level: None,
            progressions: Vec::new(),
            selections: Vec::new(),
        };
        let body = d.obj("Body").map(|b| self.body(b, e)).unwrap_or_default();
        let blueprint = d.guid("Blueprint");
        let race_size =
            progression.race.and_then(|g| self.w.content.get(g)).map(|b| b.view().int("Size"));
        let bp_size = blueprint.and_then(|g| self.w.content.get(g)).map(|b| b.view().int("Size"));
        let faction = d.guid("Faction");
        let player_faction = self.root.as_ref().and_then(|r| r.view().guid("m_PlayerFaction"));
        let resources = books::resources(d);
        let spellbooks = books::spellbooks(&mut self.w, d);
        // `UnitPartDamageReduction.m_SourceFacts` (saved); its chunks are built on first use.
        let damage_reduction = o
            .obj("Parts")
            .map(|p| p.objs("m_Parts"))
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.ty() == "UnitPartDamageReduction")
            .map(|p| crate::unit::DamageReductionPart {
                sources: p.objs("m_SourceFacts").into_iter().map(|f| self.fact_for(f)).collect(),
                chunks: Vec::new(),
                initialized: false,
            });
        let size_part_from_save = o
            .obj("Parts")
            .map(|p| p.objs("m_Parts"))
            .unwrap_or_default()
            .into_iter()
            .any(|p| p.ty() == "UnitPartSizeModifier");
        let unit = Unit {
            size_part_from_save,
            blueprint,
            original_blueprint: d.guid("OriginalBlueprint"),
            stats,
            progression,
            body,
            inventory: inv_items,
            has_own_inventory: d.bool_or("HasOwnInventory", false),
            has_dropped_loot_ref: d
                .get("m_DroppedLootEntityDataRef")
                .is_some_and(|v| !matches!(v, Value::Null)),
            inventory_collection: inv.map(|c| c.id()),
            damage: d.i32_or("m_Damage", 0),
            is_player_faction: faction.is_some() && faction == player_faction,
            // `UnitAttackFactions.m_Factions` (saved); `PostLoad` drops the unit's own faction.
            attack_factions: match d.obj("AttackFactions").and_then(|a| a.get("m_Factions")) {
                Some(Value::Seq(v)) => {
                    v.iter().filter_map(guid_of).filter(|g| Some(*g) != faction).collect()
                }
                _ => Vec::new(),
            },
            faction,
            original_size: race_size.or(bp_size).and_then(Size::from_i64),
            main_fact: d.obj("MainFact").map(|f| self.fact_for(f)),
            descriptor_turned_on: false,
            is_main_character: main_id == Some(uid.as_str()),
            // `IsPet`: the unit has `UnitPartPet` (its master is resolved after every unit).
            is_pet: pet_master.is_some(),
            spellbooks,
            group_size_increases: books::group_size_increases(o),
            extra_spells_per_day: books::extra_spells_per_day(o),
            proficiencies: d.obj("Proficiencies").map(proficiencies).unwrap_or_default(),
            applied_upgraders: match o.get("m_AppliedUpgraders") {
                Some(Value::Seq(v)) => v.iter().filter_map(guid_of).collect(),
                _ => Vec::new(),
            },
            mechanics_features: std::collections::BTreeMap::new(),
            level_up_plan_providers: Vec::new(),
            resources,
            custom_gender: d.int("CustomGender"),
            custom_name: d.str("CustomName").unwrap_or_default().to_owned(),
            replace_blueprint_for_inspection: d.guid("ReplaceBlueprintForInspection"),
            damage_reduction,
            brain: d.obj("Brain").map(|b| crate::combat::ai::Brain {
                blueprint: b.guid("Blueprint"),
                available: b
                    .items("AvailableActions")
                    .into_iter()
                    .map(|a| {
                        a.filter(|a| a.ty() == "AiActionSimple").and_then(|a| a.guid("m_Blueprint"))
                    })
                    .collect(),
                custom: b.items("CustomActions").len(),
                auto_use: b.get("m_AutoUseAbility").is_some_and(|v| !matches!(v, Value::Null)),
            }),
            // `UnitEntityData.BonusExperience` is serialized (`DefaultValueHandling.Ignore`: an
            // omitted value is 0), and the JSON constructor doesn't call `Initialize`.
            bonus_experience: Some(saved_bonus_experience(o.get("BonusExperience"))),
            ..Unit::default()
        };
        self.w.entity_mut(e).kind = EntityKind::Unit(Box::new(unit));
        // `ShiftersFuryPart` (its saved `State`).
        if let Some(p) = o
            .obj("Parts")
            .map(|p| p.objs("m_Parts"))
            .unwrap_or_default()
            .into_iter()
            .find(|p| p.ty() == crate::components::shifters_fury::PART)
            && let Some(x) = self.w.unit_mut(e)
        {
            x.shifters_fury = Some(crate::components::shifters_fury::part_from_save(p));
        }
        if let Some(m) = pet_master {
            self.pending_masters.push((e, m));
        }
        if let Some(st) = d.obj("State") {
            let ints = |n: &str| -> Vec<i64> {
                match st.get(n) {
                    Some(Value::Seq(v)) => v.iter().filter_map(int_of).collect(),
                    _ => Vec::new(),
                }
            };
            crate::unit_state::hydrate(
                &mut self.w,
                e,
                &ints("m_Conditions"),
                &ints("m_ConditionsImmunity"),
            );
        }
        self.unit_narrative(e, o);
        Ok(e)
    }

    fn body(&mut self, b: O<'_>, e: EntityIdx) -> Body {
        let mut slots = Vec::new();
        let slot = |name: String, s: Option<O<'_>>| -> Option<(String, Option<String>, bool)> {
            let s = s?;
            Some((name, s.str("m_ItemRef").map(str::to_owned), s.bool_or("m_Active", true)))
        };
        let mut raw = Vec::new();
        let mut grip_types = Vec::new();
        for (i, set) in b.items("m_HandsEquipmentSets").into_iter().enumerate() {
            grip_types.push(set.map_or(0, |s| s.i32_or("m_GripType", 0)));
            if let Some(set) = set {
                raw.extend(slot(
                    format!("HandsEquipmentSets[{i}].PrimaryHand"),
                    set.obj("PrimaryHand"),
                ));
                raw.extend(slot(
                    format!("HandsEquipmentSets[{i}].SecondaryHand"),
                    set.obj("SecondaryHand"),
                ));
            }
        }
        for n in [
            "Armor",
            "Shirt",
            "Belt",
            "Head",
            "Glasses",
            "Feet",
            "Gloves",
            "Neck",
            "Ring1",
            "Ring2",
            "Wrist",
            "Shoulders",
        ] {
            raw.extend(slot(n.to_owned(), b.obj(n)));
        }
        // `CollectAllSlots` order: hand sets, the named slots, then quick slots.
        for (i, q) in b.items("m_QuickSlots").into_iter().enumerate() {
            raw.extend(slot(format!("QuickSlot{i}"), q));
        }
        // Additional limbs: saved items resolve like hand items; limbs of
        // `AddAdditionalLimb` facts are created again when the fact turns on.
        let mut limbs = Vec::new();
        for (i, l) in b.objs("m_AdditionalLimbs").into_iter().enumerate() {
            let item = l.str("m_ItemRef").map(str::to_owned);
            limbs.push(Slot {
                name: format!("AdditionalLimbs[{i}]"),
                item: None,
                active: l.bool_or("m_Active", true),
            });
            if let Some(id) = item {
                self.pending_limbs.push((e, i, id));
            }
        }
        let empty_hand = b.obj("m_EmptyHandWeapon").map(|i| self.item_for(i));
        let empty_hand_stack: Vec<EntityIdx> =
            b.objs("m_EmptyHandWeaponsStack").into_iter().map(|i| self.item_for(i)).collect();
        // Not modelled: the polymorph slots (`UnitBody.AllSlots`, `GetAllItemsInternal` would
        // include them).
        for n in ["m_PolymorphHandsEquipmentSet", "m_PolymorphAdditionalLimbs"] {
            let present = match b.get(n) {
                None | Some(Value::Null) => false,
                Some(Value::Seq(v)) => !v.is_empty(),
                Some(_) => true,
            };
            if present {
                self.w.coverage.unsupported(format!("UnitBody: saved {n}"));
            }
        }
        for (name, item, active) in raw {
            slots.push(Slot { name, item: None, active });
            if let Some(id) = item {
                self.pending_slots_all.push((e, slots.len() - 1, id));
            }
        }
        Body {
            current_hand_set: b.i32_or("m_CurrentHandsEquipmentSetIndex", 0),
            slots,
            empty_hand,
            empty_hand_stack,
            // `IsPermanentEmptyHand` and `IsMonkUnarmedStrike` are not saved: false.
            empty_hand_flags: Vec::new(),
            hands_disabled: !b.bool_or("HandsAreEnabled", true),
            limbs,
            grip_types,
            limb_parents: std::collections::HashMap::new(),
        }
    }
}

/// A saved component runtime's own members besides `m_Data` (scalars only).
fn runtime_members(rt: O<'_>) -> serde_json::Map<String, serde_json::Value> {
    let mut out = serde_json::Map::new();
    // `EtudeBracketRuntime.m_AlreadyProcessedActivation`; `EtudeInvokeActionDelayedData`'s
    // `Executed`, `LastTickTime`, `TimeRemaining` (time spans as ticks).
    for name in ["m_AlreadyProcessedActivation", "Executed", "LastTickTime", "TimeRemaining"] {
        match rt.get(name) {
            Some(Value::Bool(b)) => {
                out.insert(name.to_owned(), serde_json::Value::Bool(*b));
            }
            Some(Value::TimeSpan(t)) => {
                out.insert(name.to_owned(), serde_json::Value::from(*t));
            }
            _ => {}
        }
    }
    // `AddDamageResistanceBase.ComponentRuntime.m_RemainPool`.
    if let Some(n) = rt.int("m_RemainPool") {
        out.insert("m_RemainPool".to_owned(), serde_json::Value::from(n));
    }
    out
}

/// A stable identity for a saved `DexBonusLimiter` (compared by reference in the C#).
fn limiter_id(o: O<'_>) -> u32 {
    u32::try_from(o.id() % (u32::MAX as usize)).unwrap_or(0)
}

fn set_ctx(w: &mut World, e: EntityIdx) {
    w.coverage.context = Some(w.entity(e).unique_id.clone());
}

fn multiset(o: Option<O<'_>>) -> Vec<(i64, i32)> {
    o.map(|m| {
        m.dict("m_Data")
            .into_iter()
            .filter_map(|(k, v)| Some((int_of(k)?, i32::try_from(int_of(v)?).ok()?)))
            .collect()
    })
    .unwrap_or_default()
}

fn proficiencies(p: O<'_>) -> crate::unit::Proficiencies {
    crate::unit::Proficiencies {
        armor: multiset(p.obj("m_ArmorProficiencies")),
        weapons: multiset(p.obj("m_WeaponProficiencies")),
        background_armor: multiset(p.obj("BackgroundArmorProficiencies")),
        background_weapons: multiset(p.obj("BackgroundWeaponProficiencies")),
    }
}

fn parts_of(o: O<'_>) -> Vec<Part> {
    o.obj("Parts")
        .map(|p| p.objs("m_Parts"))
        .unwrap_or_default()
        .into_iter()
        .map(|p| Part { clr: p.ty() })
        .collect()
}

/// One scene state's entities in `m_EntityData` order: units and (unless skipped)
/// placeholders, and the units' entities.
#[derive(Debug, Default)]
struct SceneState {
    order: Vec<crate::spawn::AreaEntry>,
    units: Vec<EntityIdx>,
    /// The scene's other entities (script zones, map objects…), in order.
    statics: Vec<EntityIdx>,
    /// `SceneEntitiesState.SceneName`: the scene whose views the state matches.
    scene: String,
    /// Every saved entity's `UniqueId` and CLR type, in `m_EntityData` order.
    saved: Vec<(String, &'static str)>,
}

/// `AreaDataStash.CutSceneName`: at most 62 UTF-16 code units
/// (`Kingmaker.EntitySystem.Persistence/AreaDataStash.cs:269`).
fn cut_scene_name(name: &str) -> String {
    let units: Vec<u16> = name.encode_utf16().collect();
    if units.len() <= 62 {
        return name.to_owned();
    }
    String::from_utf16_lossy(&units[..62])
}

/// `BlueprintArea.GetActiveDynamicScenes()` as scene names (`HashSet` insertion order): the
/// area's mechanics scene, its parts', then the active additional mechanics'.
fn active_scene_names(w: &World, area: &str) -> Vec<String> {
    let Some(bp) = wr_content::Guid::parse(area).and_then(|g| w.content.get(g)) else {
        return Vec::new();
    };
    let v = bp.view();
    let scene = |o: crate::content::View<'_>, f: &str| -> Option<String> {
        o.obj(f).and_then(|s| s.str("m_SceneName")).filter(|n| !n.is_empty()).map(str::to_owned)
    };
    let mut names: Vec<String> = Vec::new();
    // The area's own mechanics scene is first even when unnamed, so the callers can
    // `skip(1)` it (the main state is read from the area document).
    names.push(scene(v, "m_DynamicScene").unwrap_or_default());
    for part in v.guids("m_Parts") {
        if let Some(n) = w.content.get(part).and_then(|p| scene(p.view(), "m_DynamicScene"))
            && !names.contains(&n)
        {
            names.push(n);
        }
    }
    for m in crate::narrative::etudes::active_additional_mechanics(w) {
        if let Some(n) = w.content.get(m).and_then(|b| scene(b.view(), "Scene"))
            && !names.contains(&n)
        {
            names.push(n);
        }
    }
    names
}

/// Options for [`hydrate`].
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Record a lifecycle trace in `World::trace`.
    pub trace: bool,
    /// `rules-data.json` (default `cache/sim/rules-data.json`).
    pub rules_data: Option<std::path::PathBuf>,
    /// Stop after the post-load lifecycle, before the area activation that
    /// `Game.AfterSceneLoaderFinishedArea` runs (`UpdateEtudes`, `OnAreaActivated`).
    pub skip_area_activation: bool,
    /// Don't build units from the area's placeholders. `UnitPlaceholderEntityData.OnPostLoad`
    /// always calls `BecomeUnit` (`crate::spawn`); this is for a caller that wants the
    /// placeholders' saved data only (reported as not hydrated).
    pub skip_placeholders: bool,
    /// The process's `AbilityParams.Empty` when the load began (the oracle's
    /// `abilityParamsEmpty`); `None`: unknown (see [`crate::world::EmptyAbilityParams`]).
    pub ability_params_empty: Option<(i32, i32)>,
    /// The run will take cutscene progress from a recording (random gates are accepted for
    /// recorded draws instead of reported: `elements::cutscene`).
    pub recorded_cutscenes: bool,
    /// The recording also has the cutscene locks (`CommandLockControls` is accepted with its
    /// buff tick and postpone replayed instead of reported).
    pub recorded_locks: bool,
    /// The recording also has the cutscene commands' dialogue starts and entry checks
    /// (`CommandStartDialog` and skipping `EntryCondition`s are accepted, placed by it).
    pub recorded_commands: bool,
}

/// `AbilityParams.Empty`'s starting value from a command-line argument: an object
/// `{"CasterLevel": n, "RankBonus": m}`, or the path of an oracle state dump whose
/// `abilityParamsEmpty` member is one (the member is optional in older dumps: `None`).
///
/// # Errors
///
/// An unreadable or malformed argument.
pub fn parse_ability_params_empty(arg: &str) -> Result<Option<(i32, i32)>> {
    let v: serde_json::Value = if arg.trim_start().starts_with('{') {
        serde_json::from_str(arg)?
    } else {
        let bytes = std::fs::read(arg)?;
        if std::path::Path::new(arg).extension().is_some_and(|e| e == "gz") {
            use std::io::Read;
            let mut s = Vec::new();
            flate2::read::GzDecoder::new(&bytes[..]).read_to_end(&mut s)?;
            serde_json::from_slice(&s)?
        } else {
            serde_json::from_slice(&bytes)?
        }
    };
    Ok(ability_params_empty_of(&v))
}

/// The `abilityParamsEmpty` object of a dump (or the object itself).
#[must_use]
pub fn ability_params_empty_of(v: &serde_json::Value) -> Option<(i32, i32)> {
    let o = v.get("abilityParamsEmpty").unwrap_or(v);
    let n = |k: &str| o.get(k).and_then(serde_json::Value::as_i64).map(|x| x as i32);
    Some((n("CasterLevel")?, n("RankBonus").unwrap_or(0)))
}

/// `cache/sim/dlc-rewards.json`: `{"<BlueprintDlcReward guid>": available}` for this
/// install (platform state; see `Settings::dlc_rewards`). Missing file: `None`.
fn load_dlc_rewards() -> Option<HashMap<wr_content::Guid, bool>> {
    let path = wr_paths::cache_root().join("sim").join("dlc-rewards.json");
    let text = std::fs::read_to_string(path).ok()?;
    let v: HashMap<String, bool> = serde_json::from_str(&text).ok()?;
    Some(v.into_iter().filter_map(|(k, b)| wr_content::Guid::parse(&k).map(|g| (g, b))).collect())
}

/// What `Game.AfterSceneLoaderFinishedArea` runs once the area's state is on
/// (`Kingmaker/Game.cs`), in order: `IAreaLoadingStagesHandler.OnAreaScenesLoaded`, the
/// area's game mode, `IAreaHandler.OnAreaDidLoad`, `EntityCreator.Tick` (the units the
/// load queued, `created`: placeholders' `BecomeUnit` at post-load; `None` when they
/// weren't built, which is reported), `LoadedAreaState.Activate`
/// (the area's main fact: its `ITriggerOnLoad` components; the slice's areas have no
/// components, anything else is reported), `EtudesSystem.UpdateEtudes()`
/// (unconditionally), `IAreaActivationHandler.OnAreaActivated`, and `Player.OnAreaLoaded`
/// (a scheduled dialogue not yet started starts; `RecoveryMount` has nothing to do after a
/// load). The mechanics scenes active now are the ones the load brought in. Subscribers of
/// each event run fail-closed.
pub fn area_activated(w: &mut World, created: Option<&[EntityIdx]>) {
    use crate::rulebook::bus::raise_global_call;
    w.player.loaded_mechanics = crate::narrative::etudes::active_additional_mechanics(w);
    raise_global_call(w, "IAreaLoadingStagesHandler", "OnAreaScenesLoaded", None);
    if w.player.mode.is_empty() {
        w.player.mode = "Default";
    }
    // The mode's activation (`StartMode` is a before-tick task: the first tick after the load)
    // runs `UnitBuffsController.Activate`; a buff it would end is reported.
    if !crate::combat::life::buffs_invalid_on_activate(w).is_empty() {
        w.coverage.unsupported(
            "load: UnitBuffsController.Activate ends a buff (RemoveAfterDelayIfInvalid) at the first tick",
        );
    }
    raise_global_call(w, "IAreaHandler", "OnAreaDidLoad", None);
    // The scene views' `OnAreaDidLoad` (`MapObjectView` is a global `IAreaHandler`
    // subscriber): the script zones, then the area transitions' visibility. Their position
    // among the other subscribers is not modelled; it matters only when a zone fires at load,
    // which is reported.
    crate::scene::on_area_did_load(w);
    crate::scene_parts::update_all_visibilities(w);
    match created {
        Some(units) => w.player.area_units.extend(units.iter().copied()),
        None => {
            let placeholders = w.player.placeholders.len();
            if placeholders > 0 {
                w.coverage.unsupported(format!(
                    "EntityCreationController.Tick: units from placeholders not built \
                     ({placeholders})"
                ));
            }
        }
    }
    let area_comps =
        w.player.area.and_then(|g| w.content.get(g)).map_or(0, |b| b.components_array().len());
    if area_comps > 0 {
        w.coverage.unsupported("LoadedAreaState.Activate: area main fact components");
    }
    crate::narrative::etudes::update_etudes(w);
    raise_global_call(w, "IAreaActivationHandler", "OnAreaActivated", None);
    // `Player.OnAreaLoaded`.
    if w.player.dialog.scheduled.as_ref().is_some_and(|s| !s.is_scheduled) {
        crate::dialog::start_scheduled_dialog(w);
    }
}

/// Hydrates the party (`party.json`) and the full units of the save's current area.
///
/// # Errors
///
/// Missing documents or a malformed graph.
/// A loaded unit's `BonusExperience` (serialized; an omitted value is 0).
fn saved_bonus_experience(v: Option<&Value>) -> f32 {
    match v {
        Some(Value::F32(f)) => *f,
        #[allow(clippy::cast_possible_truncation)]
        Some(Value::F64(f)) => *f as f32,
        #[allow(clippy::cast_precision_loss)]
        Some(Value::Int(i)) => *i as f32,
        _ => 0.0,
    }
}

#[cfg(test)]
mod bonus_tests {
    use super::*;

    /// The saved value is kept, whatever the current difficulty gives; omitted is 0.
    #[test]
    fn a_loaded_units_bonus_experience_is_the_saved_one() {
        assert!((saved_bonus_experience(Some(&Value::F32(0.25))) - 0.25).abs() < 1e-9);
        assert!((saved_bonus_experience(Some(&Value::F64(-0.5))) + 0.5).abs() < 1e-9);
        assert!(saved_bonus_experience(None).abs() < 1e-9, "omitted: 0");
    }
}

pub fn hydrate(
    content: Rc<Content>,
    registry: Rc<Registry>,
    save: &Save,
    opts: &Options,
) -> Result<(World, HydrateReport)> {
    let mut report = HydrateReport::default();
    let player = save.doc("player.json").context("no player.json")?;
    let party = save.doc("party.json").context("no party.json")?;
    let header = save.doc("header.json").context("no header.json")?;
    let pix = DocIndex::new(player);
    let hix = DocIndex::new(header);
    let area = hix.root().and_then(|h| h.guid("Area")).map(|g| g.to_string());
    report.area = area.clone();
    let area_doc = area.as_ref().and_then(|a| save.doc(&format!("{a}.json")));
    let aix = area_doc.map(DocIndex::new);
    let partyix = DocIndex::new(party);

    let mut w = World::new(content.clone(), registry);
    w.scene.recorded_cutscenes = opts.recorded_cutscenes;
    w.scene.recorded_locks = opts.recorded_locks;
    w.scene.recorded_commands = opts.recorded_commands;
    if let Some((cl, rb)) = opts.ability_params_empty {
        w.empty_params = crate::world::EmptyAbilityParams::supplied(cl, rb);
    }
    if opts.trace {
        w.trace = Some(Vec::new());
    }
    let p = pix.root().context("player root")?;
    w.clock = crate::time::SimClock::new(
        TimeSpan(p.timespan("GameTime").unwrap_or(0)),
        TimeSpan(p.timespan("RealTime").unwrap_or(0)),
    );
    // `SettingsRoot.Difficulty`: the save-dependent settings, and the presets list.
    let settings_list = p.get("SettingsList").and_then(|v| to_json(v, &pix));
    let dsettings = match settings_list.as_ref().and_then(serde_json::Value::as_object) {
        Some(m) => crate::difficulty::DifficultySettings::from_settings_list(m),
        None => {
            w.log("error", "player.json: no SettingsList");
            crate::difficulty::DifficultySettings::default()
        }
    };
    w.settings.dead_companions_rise_after_combat =
        dsettings.dead_companions_rise_after_combat.unwrap_or(false);
    w.settings.difficulty.settings = dsettings;
    if let Some(m) = settings_list.as_ref().and_then(serde_json::Value::as_object) {
        w.player.tutorial_settings = self::narrative::tutorial_settings(m);
    }
    let rules_path =
        opts.rules_data.clone().unwrap_or_else(crate::difficulty::RulesData::default_path);
    match crate::difficulty::RulesData::load(&rules_path) {
        Ok(r) => w.settings.difficulty.list = Some(r.difficulty_presets_list),
        Err(e) => w.log("error", format!("rules data: {e:#}")),
    }
    w.settings.dlc_rewards = load_dlc_rewards();
    let main_id = p.str("m_MainCharacter").map(str::to_owned);
    let root = content.get(crate::unit::BLUEPRINT_ROOT);
    let mut ld = Loader {
        w,
        facts: HashMap::new(),
        mods: HashMap::new(),
        items: HashMap::new(),
        stats: HashMap::new(),
        root,
        pending_slots_all: Vec::new(),
        pending_limbs: Vec::new(),
        pending_masters: Vec::new(),
        _p: std::marker::PhantomData,
    };

    ld.w.scene.cache = area.as_deref().and_then(crate::scene::SceneCache::load);
    ld.w.scene.unit_prefabs = crate::scene_spawn::load_unit_prefabs();

    // ---- deserialization ----
    let mut party_units = Vec::new();
    let mut party_entities = Vec::new();
    for o in partyix.root().context("party root")?.objs("m_EntityData") {
        if o.is("UnitEntityData") {
            let e = ld.unit(o, main_id.as_deref())?;
            party_units.push(e);
            party_entities.push(e);
        } else {
            *report.not_hydrated.entry(o.ty().to_owned()).or_default() += 1;
        }
    }
    // The area's scene states: the main state first, then the active mechanics scenes'
    // states (`SceneLoader.LoadAreaCoroutine`), each with its units and placeholders in
    // `m_EntityData` order: the post-load pass runs them in this order, and placeholders
    // become units then (POC-5, `crate::spawn`).
    let mut states: Vec<SceneState> = Vec::new();
    if let Some(aix) = &aix {
        let main = aix.root().and_then(|r| r.obj("m_MainState"));
        let objs = main.map(|m| m.objs("m_EntityData")).unwrap_or_default();
        let mut st = ld.scene_entities(objs, main_id.as_deref(), opts, &mut report)?;
        st.scene = main.and_then(|m| m.str("SceneName")).unwrap_or_default().to_owned();
        states.push(st);
    }
    ld.player_narrative(p);
    // The other scene states of the area that are loaded: `BlueprintArea.GetActiveDynamicScenes`
    // (the mechanics scenes of the area, its parts and the active additional mechanics, in
    // that order). Saved states of scenes that aren't loaded stay unread, as in the game.
    let mut extra_docs: Vec<DocIndex<'_>> = Vec::new();
    if let Some(a) = &area {
        for scene in active_scene_names(&ld.w, a).into_iter().skip(1) {
            // `AreaDataStash.FileName(areaId, sceneName)`: the scene name is cut to 62 characters.
            let file = format!("{a}{}.json", cut_scene_name(&scene));
            match save.doc(&file) {
                Some(d) => extra_docs.push(DocIndex::new(d)),
                // The game would build the scene's state afresh from the scene's own
                // entities, which aren't modelled: never a silent skip.
                None => ld.w.coverage.unsupported(format!(
                    "SceneLoader: no saved state for the active scene {scene} ({file})"
                )),
            }
        }
    }
    for ix in &extra_docs {
        if let Some(root) = ix.root() {
            let mut st = ld.scene_entities(
                root.objs("m_EntityData"),
                main_id.as_deref(),
                opts,
                &mut report,
            )?;
            st.scene = root.str("SceneName").unwrap_or_default().to_owned();
            states.push(st);
            ld.area_narrative(root);
        }
    }
    if let Some(main) = aix.as_ref().and_then(|a| a.root()).and_then(|r| r.obj("m_MainState")) {
        ld.area_narrative(main);
    }
    let mut ixs: Vec<&DocIndex<'_>> = vec![&partyix];
    if let Some(a) = &aix {
        ixs.push(a);
    }
    ixs.extend(extra_docs.iter());
    ld.link_items(&ixs);
    ld.check_scene_views(&states);
    ld.resolve_slots();
    ld.resolve_masters();
    let mut w = ld.w;
    w.main_character = main_id.as_deref().and_then(|id| w.by_id.get(id).copied());
    w.player.cross_scene_units = party_entities.clone();
    let mut area_units: Vec<EntityIdx> =
        states.iter().flat_map(|st| st.units.iter().copied()).collect();
    let area_order: Vec<crate::spawn::AreaEntry> =
        states.iter().flat_map(|st| st.order.iter().cloned()).collect();
    w.player.area_units = area_units.clone();
    w.player.party_characters = p
        .get("PartyCharacters")
        .map(|v| match v {
            Value::Seq(items) => items
                .iter()
                .filter_map(|x| match x {
                    Value::Str(s) => w.by_id.get(&**s).copied(),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default();
    let systems: Vec<EntityIdx> = [w.player.quest_book, w.player.tutorial, w.player.etudes_system]
        .into_iter()
        .flatten()
        .collect();
    for &e in &systems {
        crate::entity::pre_post_load(&mut w, e);
    }

    // `SaveManager.LoadRoutine` restores every saved area state's `Area` before
    // `Player.PostLoad`.
    restore_area_blueprints(&mut w, save);

    // ---- Player.PostLoad: QuestBook, CrossSceneState, inventories, Tutorial, Etudes ----
    w.trace("Player.PostLoad", |_| String::new());
    if let Some(q) = w.player.quest_book {
        crate::entity::post_load(&mut w, q);
    }
    for &e in &party_entities {
        crate::entity::pre_post_load(&mut w, e);
    }
    for &e in &party_entities {
        set_ctx(&mut w, e);
        crate::entity::post_load(&mut w, e);
    }
    for &e in &party_entities {
        set_ctx(&mut w, e);
        crate::entity::apply_post_load_fixes(&mut w, e);
    }
    // `ItemsCollection.PostLoad()` of the player's inventory subscribes it to the event bus.
    crate::rules::identify::subscribe_player_inventory(&mut w);
    // `Inventory = MainCharacter.Inventory; Inventory.PostLoad()`: items not yet
    // post-loaded, in collection order; then other cross-scene units' inventories.
    let mut inventories: Vec<EntityIdx> = Vec::new();
    if let Some(mc) = w.main_character {
        inventories.push(mc);
    }
    inventories.extend(party_units.iter().copied().filter(|u| Some(*u) != w.main_character));
    for u in &inventories {
        set_ctx(&mut w, *u);
        for it in w.unit(*u).map(|x| x.inventory.clone()).unwrap_or_default() {
            if !w.entity(it).post_load_executed {
                crate::entity::post_load(&mut w, it);
            }
        }
    }
    if let Some(t) = w.player.tutorial {
        crate::entity::post_load(&mut w, t);
    }
    if let Some(e) = w.player.etudes_system {
        crate::entity::post_load(&mut w, e);
        // `EtudesSystem.OnPostLoad`: `RestoreTreeStructure`.
        crate::narrative::etudes::restore_tree_structure(&mut w);
    }
    // Player fixes: QuestBook, Tutorial, EtudesSystem, then the inventory's items.
    w.coverage.context = None;
    for &e in &systems {
        crate::entity::apply_post_load_fixes(&mut w, e);
        if Some(e) == w.player.etudes_system {
            crate::narrative::etudes::apply_post_load_fixes(&mut w);
        }
    }
    if let Some(mc) = w.main_character {
        set_ctx(&mut w, mc);
        for it in w.unit(mc).map(|x| x.inventory.clone()).unwrap_or_default() {
            crate::entity::apply_post_load_fixes(&mut w, it);
        }
    }
    // ---- Player.TurnOn: QuestBook, inventory items, Tutorial, Etudes; CrossSceneState ----
    w.coverage.context = None;
    if let Some(q) = w.player.quest_book {
        crate::entity::turn_on(&mut w, q);
    }
    if let Some(mc) = w.main_character {
        set_ctx(&mut w, mc);
        for it in w.unit(mc).map(|x| x.inventory.clone()).unwrap_or_default() {
            if !w.entity(it).is_turned_on {
                crate::entity::turn_on(&mut w, it);
            }
        }
    }
    w.coverage.context = None;
    for e in [w.player.tutorial, w.player.etudes_system].into_iter().flatten() {
        crate::entity::turn_on(&mut w, e);
    }
    for &e in &party_entities {
        set_ctx(&mut w, e);
        if !w.entity(e).is_turned_on {
            crate::entity::turn_on(&mut w, e);
        }
    }

    // ---- the area's scene states: `AreaPersistentState.PostLoad` post-loads the main state
    // and every loaded additional state, then `SceneLoader` matches each active scene's state
    // with its scene and turns it on ----
    let spawned = load_area_states(&mut w, &states);
    // `Game.AfterSceneLoaderFinishedArea`: the creation queue joins the scene state there,
    // then controllers tick (the activatable-abilities controller restarts stopped toggles).
    w.coverage.context = None;
    let created = (!opts.skip_placeholders).then_some(spawned.as_slice());
    if opts.skip_area_activation {
        w.player.area_units.extend(spawned.iter().copied());
    } else {
        area_activated(&mut w, created);
    }
    if !opts.skip_placeholders {
        crate::spawn::after_area_load(&mut w, &area_order, &spawned);
        area_units.extend(spawned.iter().copied());
    }
    crate::spawn::report_assumed_awake(&mut w);
    w.coverage.context = None;
    report.party_units = party_units.iter().map(|e| w.entity(*e).unique_id.clone()).collect();
    report.area_units = area_units.iter().map(|e| w.entity(*e).unique_id.clone()).collect();
    report.facts = w.facts.len();
    report.items = w.entities.iter().filter(|e| matches!(e.kind, EntityKind::Item(_))).count();
    report.modifiers = w.mods.len();
    Ok((w, report))
}

/// `SceneLoader.LoadAreaCoroutine` for a loaded save's area states: first
/// `AreaPersistentState.PostLoad` (`m_MainState.PostLoad()`, then each loaded additional
/// state's, then `m_Area.PostLoad()`, whose area facts aren't modelled), then for each active
/// dynamic scene in order `MatchStateWithScene(state)` (the saved entities attach to the
/// scene's views) and, if it isn't on yet, `state.TurnOn()`. Returns the units placeholders
/// became during the post-load.
fn load_area_states(w: &mut World, states: &[SceneState]) -> Vec<EntityIdx> {
    w.trace("AreaPersistentState.PostLoad", |_| String::new());
    let mut spawned = Vec::new();
    for st in states {
        scene_state_post_load(w, st, &mut spawned);
    }
    for (i, st) in states.iter().enumerate() {
        w.trace("SceneLoader.MatchStateWithScene", |_| i.to_string());
        for entry in &st.order {
            if let crate::spawn::AreaEntry::Static(e) = entry {
                w.coverage.context = None;
                crate::scene_parts::view_attach(w, *e);
            }
        }
        // `SceneEntitiesState.TurnOn`: every entity not yet on, in order.
        for entry in &st.order {
            match entry {
                crate::spawn::AreaEntry::Unit(e) => {
                    set_ctx(w, *e);
                    if !w.entity(*e).is_turned_on {
                        crate::entity::turn_on(w, *e);
                    }
                }
                crate::spawn::AreaEntry::Static(e) => {
                    w.coverage.context = None;
                    if !w.entity(*e).is_turned_on {
                        crate::entity::turn_on(w, *e);
                    }
                }
                crate::spawn::AreaEntry::Placeholder(_) => {}
            }
        }
    }
    spawned
}

/// `SceneEntitiesState.PostLoad`: every entity's `PrePostLoad`, then `PostLoad` in order
/// (placeholders become units), the units' inventories, then the fix pass.
fn scene_state_post_load(w: &mut World, st: &SceneState, spawned: &mut Vec<EntityIdx>) {
    for entry in &st.order {
        match entry {
            crate::spawn::AreaEntry::Unit(e) | crate::spawn::AreaEntry::Static(e) => {
                crate::entity::pre_post_load(w, *e);
            }
            crate::spawn::AreaEntry::Placeholder(_) => {}
        }
    }
    for entry in &st.order {
        match entry {
            crate::spawn::AreaEntry::Unit(e) => {
                set_ctx(w, *e);
                crate::entity::post_load(w, *e);
            }
            crate::spawn::AreaEntry::Static(e) => {
                w.coverage.context = None;
                crate::entity::post_load(w, *e);
            }
            crate::spawn::AreaEntry::Placeholder(p) => {
                w.coverage.context = Some(p.unit_id.clone());
                if let Some(e) = crate::spawn::become_unit(w, p) {
                    spawned.push(e);
                }
            }
        }
    }
    for &e in &st.units {
        set_ctx(w, e);
        for it in w.unit(e).map(|x| x.inventory.clone()).unwrap_or_default() {
            if !w.entity(it).post_load_executed {
                crate::entity::post_load(w, it);
            }
        }
    }
    // `SceneEntitiesState.PostLoad`'s fix pass: each entity's fixes, then (for a unit in a
    // non-party state) its inventory items' fixes, before the next entity.
    for entry in &st.order {
        match entry {
            crate::spawn::AreaEntry::Unit(e) => {
                let e = *e;
                set_ctx(w, e);
                crate::entity::apply_post_load_fixes(w, e);
                for it in w.unit(e).map(|x| x.inventory.clone()).unwrap_or_default() {
                    crate::entity::apply_post_load_fixes(w, it);
                }
            }
            crate::spawn::AreaEntry::Static(e) => {
                w.coverage.context = None;
                crate::entity::apply_post_load_fixes(w, *e);
            }
            crate::spawn::AreaEntry::Placeholder(_) => {}
        }
    }
}

/// `SaveManager.LoadRoutine` → `AreaPersistentState.RestoreAreaBlueprint()` for every saved
/// area state (`SaveManager.cs:1285`): the states `ThreadedGameLoader.CreateStateData`
/// made from the save's `<area guid>[<scene>].json` entries have no `m_Area` yet, so each
/// gets `new Area(blueprint)` (`Area.cs:27–41`): a fact of the area's blueprint, then one per
/// `BlueprintAreaPart`, each attached with `Player.GetNewUniqueId()`. The areas
/// themselves (saved `m_Area` replaces them later) aren't modelled; their fact ids are drawn
/// from the counter so that everything created later gets the original's ids.
fn restore_area_blueprints(w: &mut World, save: &Save) {
    const NOT_AREAS: [&str; 7] = [
        "header.json",
        "player.json",
        "party.json",
        "statistic.json",
        "tbm.json",
        "clockwork.json",
        "checksums",
    ];
    let mut areas: Vec<wr_content::Guid> = Vec::new();
    for e in &save.entries {
        if !e.name.ends_with(".json") || NOT_AREAS.iter().any(|n| e.name.starts_with(n)) {
            continue;
        }
        let Some(g) = e.name.get(..32).and_then(wr_content::Guid::parse) else {
            w.coverage.unsupported("SaveManager.LoadRoutine: saved area state without a guid");
            continue;
        };
        if !areas.contains(&g) {
            areas.push(g);
        }
    }
    w.coverage.context = None;
    for g in areas {
        let Some(bp) = w.content.get(g) else {
            w.coverage.unsupported("Area..ctor: area blueprint missing");
            continue;
        };
        let mut facts = vec![bp.clone()];
        for p in bp.view().guids("m_Parts") {
            match w.content.get(p) {
                Some(part) => facts.push(part),
                None => w.coverage.unsupported("Area..ctor: area part blueprint missing"),
            }
        }
        for f in facts {
            // `new EntityFact(blueprint)` creates the runtime components of its providers
            // and `Attach` calls their `OnFactAttached`: none are ported for areas.
            if f.components_array().iter().any(|o| {
                o.ty != wr_content::schema::NO_TYPE
                    && wr_content::schema::ty(o.ty)
                        .has(wr_content::schema::type_flags::RUNTIME_PROVIDER)
            }) {
                w.coverage.unsupported("Area..ctor: area fact with runtime components");
            }
            crate::narrative::Player::new_unique_id(w);
        }
    }
}

impl Loader<'_> {
    /// A scene state's `m_EntityData`: units are hydrated, placeholders become pending units
    /// (unless `skip_placeholders`), the other entities are reported (spawners' and
    /// placeholders' saved data is read by `area_narrative`).
    fn scene_entities(
        &mut self,
        objs: Vec<O<'_>>,
        main_id: Option<&str>,
        opts: &Options,
        report: &mut HydrateReport,
    ) -> Result<SceneState> {
        let mut st = SceneState::default();
        for o in objs {
            st.saved.push((o.str("UniqueId").unwrap_or_default().to_owned(), o.clr()));
            if o.ty() == "UnitEntityData" {
                let e = self.unit(o, main_id)?;
                st.units.push(e);
                st.order.push(crate::spawn::AreaEntry::Unit(e));
            } else if o.ty() == "UnitPlaceholderEntityData" && !opts.skip_placeholders {
                st.order.push(crate::spawn::AreaEntry::Placeholder(Box::new(self.placeholder(o))));
            } else if let Some(e) = self.static_entity(o) {
                st.statics.push(e);
                st.order.push(crate::spawn::AreaEntry::Static(e));
            } else {
                *report.not_hydrated.entry(o.ty().to_owned()).or_default() += 1;
                // Other scene entities aren't hydrated, so their load-time callbacks can't run.
                let read = o.clr().contains("UnitSpawnerBase+MyData")
                    || o.ty() == "UnitPlaceholderEntityData";
                if !read {
                    self.w
                        .coverage
                        .unsupported(format!("area scene entity not hydrated: {}", o.ty()));
                }
            }
        }
        Ok(st)
    }

    /// `UnitPartPet.Master`: a master the save doesn't hold is reported.
    fn resolve_masters(&mut self) {
        for (pet, id) in std::mem::take(&mut self.pending_masters) {
            match self.w.by_id.get(id.as_str()).copied() {
                Some(m) => self.w.unit_mut(pet).expect("unit").master = Some(m),
                None => self.w.coverage.unsupported("UnitPartPet: master not in the save"),
            }
        }
    }

    fn resolve_slots(&mut self) {
        for (u, i, id) in std::mem::take(&mut self.pending_limbs) {
            let it = self.w.by_id.get(&id).copied();
            if let Some(unit) = self.w.unit_mut(u) {
                unit.body.limbs[i].item = it;
            }
            if let Some(it) = it
                && let Some(item) = self.w.item_mut(it)
            {
                item.holding_slot = Some((u, format!("AdditionalLimbs[{i}]")));
            }
        }
        let pending = std::mem::take(&mut self.pending_slots_all);
        for (u, slot, id) in pending {
            let it = self.w.by_id.get(&id).copied();
            if let Some(unit) = self.w.unit_mut(u) {
                unit.body.slots[slot].item = it;
            }
            if let Some(it) = it {
                let name =
                    self.w.unit(u).map(|x| x.body.slots[slot].name.clone()).unwrap_or_default();
                if let Some(item) = self.w.item_mut(it) {
                    item.holding_slot = Some((u, name));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DocIndex, cut_scene_name, own_params, parent_params};

    /// `MechanicsContext.Params` from a save: a null `m_Params` is no params, and a parent's
    /// `m_Params` saved as a `$ref` resolves to the object it names.
    #[test]
    fn context_params_resolve_nulls_parents_and_references() {
        let mc = "Kingmaker.UnitLogic.Mechanics.MechanicsContext, Assembly-CSharp";
        let json = format!(
            r#"{{"$id":"1","$type":"{mc}","m_Params":{{"$id":"5","CasterLevel":11,"RankBonus":2}},
            "ParentContext":{{"$id":"2","$type":"{mc}","m_Params":null,
            "ParentContext":{{"$id":"3","$type":"{mc}","m_Params":{{"$ref":"5"}}}}}}}}"#
        );
        let parsed = wr_save::json::parse(json.as_bytes()).expect("json");
        let (doc, _) = wr_save::decode::decode(&parsed, None);
        let ix = DocIndex::new(&doc);
        let root = ix.root().expect("root");
        let child = root.obj("ParentContext").expect("parent");
        let expected = serde_json::json!({ "CasterLevel": 11, "RankBonus": 2 });
        assert_eq!(own_params(root), Some(expected.clone()));
        assert_eq!(own_params(child), None);
        assert_eq!(parent_params(child), Some(expected));
    }

    /// `AreaDataStash.FileName` cuts a scene name to 62 characters: the shipped
    /// `AlushinyrraMediumCity_TenThousandDelights_TableHouseOfSweetTerror_Mechanics` (75) is
    /// saved under its first 62.
    #[test]
    fn long_scene_names_are_cut_to_62_characters() {
        let name = "AlushinyrraMediumCity_TenThousandDelights_TableHouseOfSweetTerror_Mechanics";
        assert_eq!(name.len(), 75);
        let cut = cut_scene_name(name);
        assert_eq!(cut, &name[..62]);
        assert_eq!(cut_scene_name(&name[..62]), &name[..62]);
        assert_eq!(cut_scene_name("Short"), "Short");
        assert_eq!(cut_scene_name(""), "");
    }

    fn static_entity(w: &mut crate::world::World, id: &str) -> crate::world::EntityIdx {
        w.add_entity(crate::world::Entity {
            unique_id: id.into(),
            clr: "MapObjectEntityData",
            kind: crate::world::EntityKind::Other,
            facts: crate::world::FactsManager::default(),
            parts: Vec::new(),
            is_turned_on: false,
            post_load_executed: false,
            is_in_game: true,
            subscribed: false,
            disposing: false,
            is_disposed: false,
        })
    }

    fn state(e: crate::world::EntityIdx, scene: &str, id: &str) -> super::SceneState {
        super::SceneState {
            order: vec![crate::spawn::AreaEntry::Static(e)],
            units: Vec::new(),
            statics: vec![e],
            scene: scene.into(),
            saved: vec![(id.into(), "Kingmaker.EntitySystem.Entities.MapObjectEntityData")],
        }
    }

    /// `AreaPersistentState.PostLoad` post-loads the main state and every loaded additional
    /// state before `SceneLoader` matches any state with its scene and turns it on.
    #[test]
    fn every_area_state_post_loads_before_any_attaches() {
        let (mut w, _) = crate::stats::tests::world_with_unit();
        let a = static_entity(&mut w, "a");
        let b = static_entity(&mut w, "b");
        w.trace = Some(Vec::new());
        super::load_area_states(&mut w, &[state(a, "Main", "a"), state(b, "Extra", "b")]);
        let trace: Vec<(String, String)> = w
            .trace
            .take()
            .expect("trace")
            .into_iter()
            .map(|t| (t.what.to_owned(), t.subject))
            .collect();
        let at = |what: &str, subject: &str| {
            trace
                .iter()
                .position(|(w, s)| w == what && s == subject)
                .unwrap_or_else(|| panic!("{what} {subject}: {trace:?}"))
        };
        assert!(at("Entity.PostLoad", "b") < at("SceneLoader.MatchStateWithScene", "0"));
        assert!(at("Entity.TurnOn", "a") < at("SceneLoader.MatchStateWithScene", "1"));
        assert!(at("SceneLoader.MatchStateWithScene", "1") < at("Entity.TurnOn", "b"));
    }

    fn view(id: &str, scene: &str, class: &str, active: bool) -> crate::scene::ViewInfo {
        crate::scene::ViewInfo {
            unique_id: id.into(),
            class: class.into(),
            scene: scene.into(),
            active_in_hierarchy: active,
            is_in_game: true,
            components: Vec::new(),
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            group: None,
        }
    }

    /// `SceneLoader.LoadSceneEntities` both ways: a saved entity whose scene has no view with
    /// its id is removed, and an active view of a data-creating class with no saved data gets
    /// new data; both are reported. Views of other scenes, inactive ones and classes that
    /// create no data on load are left alone.
    #[test]
    fn scene_views_and_saved_entities_are_reconciled_both_ways() {
        let cache = |views| {
            Some(std::rc::Rc::new(crate::scene::SceneCache {
                schema: crate::scene::SCHEMA.into(),
                area: "a".into(),
                name: "a".into(),
                source: String::new(),
                zones: Vec::new(),
                views,
            }))
        };
        let (mut w, _) = crate::stats::tests::world_with_unit();
        let a = static_entity(&mut w, "a");
        let st = [state(a, "Main", "a")];
        w.scene.cache = cache(vec![
            view("a", "Main", "Kingmaker.View.MapObjects.MapObjectView", true),
            view("x", "Other", "Kingmaker.View.MapObjects.MapObjectView", true),
            view("y", "Main", "Kingmaker.View.MapObjects.MapObjectView", false),
            view("z", "Main", "Kingmaker.View.MapObjects.DynamicMapObjectView", true),
        ]);
        super::scene::check_scene_views(&mut w, &st);
        assert!(w.coverage.valid(), "{:?}", w.coverage.unsupported);
        // A locator view of the scene with no saved data gets data at load.
        w.scene.cache = cache(vec![
            view("a", "Main", "Kingmaker.View.MapObjects.MapObjectView", true),
            view("l", "Main", "Kingmaker.View.LocatorView", true),
        ]);
        super::scene::check_scene_views(&mut w, &st);
        assert!(w.coverage.unsupported.keys().any(|k| k.contains("CreateDataForView")));
        // The saved object's view is in another scene: removed.
        let (mut w, _) = crate::stats::tests::world_with_unit();
        w.scene.cache =
            cache(vec![view("a", "Other", "Kingmaker.View.MapObjects.MapObjectView", true)]);
        super::scene::check_scene_views(&mut w, &st);
        assert!(w.coverage.unsupported.keys().any(|k| k.contains("no view in its scene")));
    }
}
