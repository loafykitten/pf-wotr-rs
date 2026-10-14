//! Spellbooks (decompiled, not copied: `Kingmaker.UnitLogic/Spellbook.cs`, `SpellSlot.cs`,
//! `Kingmaker.UnitLogic.Abilities/AbilityData.cs` as a spellbook entry, `UnitDescriptor.cs`'s
//! spellbook dictionary and `UnitHelper.CopySpellbook`): the data a save holds, `PostLoad`
//! and its fixes, class levels, known / special / memorized spells and their slots, rest.
//!
//! A spellbook belongs to a unit and is keyed by its blueprint, in a `Dictionary` that
//! enumerates in Mono's order. Entries are plain values: an `AbilityData` that two lists share
//! in the C# is a copy here (the C# compares them by value, `AbilityData.Equals`). A slot is
//! named by its (type, index) within a spell level, which is how `LinkedSlots` refer to it.
//!
//! Not ported, and reported when reached: metamagic and magic-hack data on a spell, spells
//! converted from others, custom spells, temporary spells, `MagicHackSpellbookComponent`,
//! `UnitPartExtraSpellsPerDay`, opposition schools and descriptors (their school and
//! descriptor lookups), swapping slots, spending and casting.

use std::collections::HashMap;
use std::rc::Rc;

use serde_json::{Value as J, json};
use wr_content::Guid;
use wr_content::node::Node;

use crate::content::{Blueprint, View};
use crate::csharp::arith;
use crate::csharp::dict::CsDict;
use crate::stats::ops;
use crate::stats::types::StatType;
use crate::stats::value::{ChangedHandler, StatKind};
use crate::world::{EntityIdx, World};

/// Spell levels 0..=10.
pub const LEVELS: usize = 11;
/// `SpellSlotType.Common`.
pub const COMMON: i32 = 0;
/// `SpellSlotType.Favorite`.
pub const FAVORITE: i32 = 1;
const TYPE_NORMAL: i32 = 0;
const TYPE_MYTHIC: i32 = 1;
const TYPE_STANDALONE_MYTHIC: i32 = 2;
/// `MechanicsFeatureType.EnableSpellLevel10`.
const ENABLE_SPELL_LEVEL_10: i64 = 102;
/// Depth limit of the oracle's persisted-members writer (`Snap.MaxDepthPersisted`).
const MAX_DEPTH: i32 = 8;

/// A slot of one spell level: (`SpellSlotType`, `Index`).
pub type SlotKey = (i32, i32);

/// An `AbilityData` held by a spellbook.
#[derive(Debug, Clone)]
pub struct Spell {
    pub blueprint: Guid,
    pub unique_id: String,
    pub extra_spell_slot_cost: i32,
    pub save_spellbook_slot: bool,
    pub force_target_owner: bool,
    pub decoration_color: i32,
    pub decoration_border: i32,
    pub is_mystic_theurge_combined: bool,
    pub was_substituted: bool,
    pub is_temporary: bool,
    pub copied_from_scroll: bool,
    pub is_from_mythic_spell_list: bool,
    /// `HasActionBarSlot` (`CountableFlag` count).
    pub has_action_bar_slot: i32,
    pub temporarily_disabled: bool,
    /// `SpellLevelInSpellbook` (not saved: `PostLoad` and the constructor set it).
    pub level_in_spellbook: Option<i32>,
    /// The save's object for a hydrated spell (`$id`/`$ref` share one `AbilityData` between
    /// lists): compared during the load's `PostLoad`, never dereferenced.
    pub saved_object: Option<usize>,
}

impl Spell {
    /// `new AbilityData(blueprint, spellbook, spellLevel)`.
    pub fn new(w: &mut World, blueprint: Guid, level: i32) -> Self {
        Self {
            blueprint,
            unique_id: w.new_guid(),
            extra_spell_slot_cost: 0,
            save_spellbook_slot: false,
            force_target_owner: false,
            decoration_color: -1,
            decoration_border: -1,
            is_mystic_theurge_combined: false,
            was_substituted: false,
            is_temporary: false,
            copied_from_scroll: false,
            is_from_mythic_spell_list: false,
            has_action_bar_slot: 0,
            temporarily_disabled: false,
            level_in_spellbook: Some(level),
            saved_object: None,
        }
    }

    /// `AbilityData.Equals(AbilityData)` for two spells of one spellbook (no fact, no
    /// metamagic, no overrides): blueprint, spell level in the spellbook, theurge flag.
    #[must_use]
    pub fn same(&self, o: &Spell) -> bool {
        self.blueprint == o.blueprint
            && self.level_in_spellbook == o.level_in_spellbook
            && self.is_mystic_theurge_combined == o.is_mystic_theurge_combined
    }
}

/// A `SpellSlot`.
#[derive(Debug, Clone)]
pub struct Slot {
    pub spell_level: i32,
    pub ty: i32,
    pub index: i32,
    /// `Spell` (`SpellShell`).
    pub spell: Option<Spell>,
    pub available: bool,
    /// `LinkedSlots`: the slots of this spell level, by (type, index).
    pub linked: Option<Vec<SlotKey>>,
    pub is_opposition: bool,
}

impl Slot {
    /// `new SpellSlot(spellLevel, type, index, spellbook)`.
    #[must_use]
    pub fn new(spell_level: i32, ty: i32, index: i32) -> Self {
        Self {
            spell_level,
            ty,
            index,
            spell: None,
            available: false,
            linked: None,
            is_opposition: false,
        }
    }

    fn key(&self) -> SlotKey {
        (self.ty, self.index)
    }
}

/// A `Spellbook`.
#[derive(Debug, Clone)]
pub struct Spellbook {
    pub blueprint: Guid,
    /// Identity of this object (the casting stat's `OnChanged` handler outlives a deleted
    /// spellbook in the C#).
    pub serial: u32,
    pub base_level: i32,
    pub mythic_level: i32,
    pub ty: i32,
    pub known: Vec<Vec<Spell>>,
    pub special: Vec<Vec<Spell>>,
    pub special_lists: Vec<Guid>,
    pub custom: Vec<Vec<Spell>>,
    /// `m_KnownSpellLevels` (not saved: rebuilt by `PostLoad`).
    pub known_levels: CsDict<Guid, Vec<i32>>,
    pub memorized: Vec<Vec<Slot>>,
    pub hidden_common: CsDict<i32, CsDict<i32, Slot>>,
    pub hidden_favorite: CsDict<i32, CsDict<i32, Slot>>,
    pub remove_memorized: Vec<Spell>,
    pub spontaneous: [i32; LEVELS],
    pub max_caster_level: i32,
    pub opposition_descriptors: i64,
    pub opposition_schools: Vec<i64>,
    pub ex_opposition_schools: Vec<i64>,
    pub ignore_opposition_schools: Vec<i64>,
    pub bonus_spell_slots: [i32; LEVELS],
}

impl Spellbook {
    fn empty(blueprint: Guid, serial: u32, max_caster_level: i32) -> Self {
        let lists = || vec![Vec::new(); LEVELS];
        Self {
            blueprint,
            serial,
            base_level: 0,
            mythic_level: 0,
            ty: TYPE_NORMAL,
            known: lists(),
            special: lists(),
            special_lists: Vec::new(),
            custom: lists(),
            known_levels: CsDict::new(),
            memorized: vec![Vec::new(); LEVELS],
            hidden_common: CsDict::new(),
            hidden_favorite: CsDict::new(),
            remove_memorized: Vec::new(),
            spontaneous: [0; LEVELS],
            max_caster_level,
            opposition_descriptors: 0,
            opposition_schools: Vec::new(),
            ex_opposition_schools: Vec::new(),
            ignore_opposition_schools: Vec::new(),
            bonus_spell_slots: [0; LEVELS],
        }
    }

    /// A spellbook as a save holds it, before `PostLoad` (the caller fills the lists).
    pub fn loaded(w: &mut World, blueprint: Guid) -> Self {
        w.spellbook_serial += 1;
        let serial = w.spellbook_serial;
        let max = max_caster_level_of(w, blueprint);
        Self::empty(blueprint, serial, max)
    }

    fn is_standalone_mythic(&self) -> bool {
        self.ty == TYPE_STANDALONE_MYTHIC
    }

    /// `IsMythic`.
    #[must_use]
    pub fn is_mythic(&self) -> bool {
        self.ty != TYPE_NORMAL
    }

    fn slot_mut(&mut self, level: usize, key: SlotKey) -> Option<&mut Slot> {
        if let Some(s) = self.memorized[level].iter_mut().find(|s| s.key() == key) {
            return Some(s);
        }
        let hidden =
            if key.0 == COMMON { &mut self.hidden_common } else { &mut self.hidden_favorite };
        hidden.get_mut(&i32::try_from(level).ok()?)?.get_mut(&key.1)
    }

    fn slot(&self, level: usize, key: SlotKey) -> Option<&Slot> {
        if let Some(s) = self.memorized[level].iter().find(|s| s.key() == key) {
            return Some(s);
        }
        let hidden = if key.0 == COMMON { &self.hidden_common } else { &self.hidden_favorite };
        hidden.get(&i32::try_from(level).ok()?)?.get(&key.1)
    }

    /// `SpellSlot.SpellShell` setter.
    fn set_spell_shell(&mut self, level: usize, key: SlotKey, value: Option<Spell>) {
        let Some(slot) = self.slot(level, key) else { return };
        if let Some(prev) = slot.spell.clone()
            && slot.available
        {
            self.remove_memorized.push(prev);
        }
        let mut available = false;
        if let Some(v) = &value
            && let Some(p) = self.remove_memorized.iter().position(|r| r.same(v))
        {
            available = true;
            self.remove_memorized.remove(p);
        }
        if let Some(s) = self.slot_mut(level, key) {
            s.spell = value;
            s.available = available;
        }
    }

    /// `SpellSlot.Clear()`: the linked slots, then this one.
    fn clear_slot(&mut self, level: usize, key: SlotKey) {
        let linked = self.slot(level, key).and_then(|s| s.linked.clone());
        for k in linked.into_iter().flatten() {
            self.clear_internal(level, k);
        }
        self.clear_internal(level, key);
    }

    fn clear_internal(&mut self, level: usize, key: SlotKey) {
        self.set_spell_shell(level, key, None);
        if let Some(s) = self.slot_mut(level, key) {
            s.linked = None;
            s.is_opposition = false;
        }
    }

    /// `AddKnownSpellLevel`.
    fn add_known_spell_level(&mut self, spell: Guid, level: i32) {
        let list = self.known_levels.get_or_insert_with(spell, Vec::new);
        if list.contains(&level) {
            return;
        }
        match list.iter().position(|l| level <= *l) {
            Some(i) => list.insert(i, level),
            None => list.push(level),
        }
    }

    /// `RemoveKnownSpellLevel`.
    fn remove_known_spell_level(&mut self, spell: Guid, level: i32) {
        let empty = match self.known_levels.get_mut(&spell) {
            Some(list) => {
                if let Some(p) = list.iter().position(|l| *l == level) {
                    list.remove(p);
                }
                list.is_empty()
            }
            None => true,
        };
        if empty {
            self.known_levels.remove(&spell);
        }
    }

    /// `IsKnownOnLevel`.
    #[must_use]
    pub fn is_known_on_level(&self, spell: Guid, level: i32) -> bool {
        self.known_levels.get(&spell).is_some_and(|l| l.contains(&level))
    }

    /// `GetMinSpellLevel`.
    #[must_use]
    pub fn min_spell_level(&self, spell: Guid) -> i32 {
        self.known_levels.get(&spell).and_then(|l| l.first().copied()).unwrap_or(-1)
    }

    /// `GetSpellLevel(data)` for a spell without metamagic or magic hack data.
    #[must_use]
    pub fn spell_level_of(&self, data: &Spell) -> i32 {
        data.level_in_spellbook.unwrap_or_else(|| self.min_spell_level(data.blueprint))
    }
}

// ---- blueprint access -------------------------------------------------------------------

fn bp(w: &World, g: Guid) -> Option<Rc<Blueprint>> {
    w.content.get(g)
}

fn node_i32(n: &Node) -> i32 {
    match n {
        Node::Int(i) | Node::Enum(i) => i32::try_from(*i).unwrap_or(0),
        _ => 0,
    }
}

/// `BlueprintSpellsTable.Levels[].Count`.
fn table_levels(w: &World, table: Option<Guid>) -> Vec<Vec<i32>> {
    let Some(t) = table.and_then(|g| bp(w, g)) else { return Vec::new() };
    t.view()
        .objs("Levels")
        .into_iter()
        .map(|e| View(e).seq("Count").iter().map(node_i32).collect())
        .collect()
}

/// `BlueprintSpellsTable.GetCount(classLevel, spellLevel)`.
fn table_count(levels: &[Vec<i32>], class_level: i32, spell_level: i32) -> i32 {
    if class_level < 0 || spell_level < 0 {
        return -1;
    }
    let entry = usize::try_from(class_level).ok().and_then(|i| levels.get(i)).or(levels.last());
    match entry {
        Some(e) => usize::try_from(spell_level).ok().and_then(|i| e.get(i)).copied().unwrap_or(-1),
        None => -1,
    }
}

fn per_day_levels(w: &World, sb: Guid) -> Vec<Vec<i32>> {
    table_levels(w, bp(w, sb).and_then(|b| b.view().guid("m_SpellsPerDay")))
}

fn max_caster_level_of(w: &World, sb: Guid) -> i32 {
    i32::try_from(per_day_levels(w, sb).len()).unwrap_or(0).wrapping_sub(1)
}

/// `BlueprintSpellList.GetSpells(level)`, positional like the C# (`SpellsByLevel[level]`), with the
/// DLC filter of `SpellLevelList.SpellsFiltered` (the platform's state, `Settings::dlc_rewards`).
pub fn list_spells(w: &mut World, list: Guid, level: i32) -> Rc<Vec<Guid>> {
    if let Some(c) = w.spell_cache.borrow().get(&(list, level)) {
        return c.clone();
    }
    let spells: Vec<Guid> = (|| {
        let l = bp(w, list)?;
        let idx = usize::try_from(level).ok()?;
        let by_level = l.view().objs("SpellsByLevel");
        Some(View(by_level.get(idx).copied()?).guids("m_Spells"))
    })()
    .unwrap_or_default();
    let mut out = Vec::new();
    for g in spells {
        // `!spell.IsDlcRestricted()`.
        let restricted = match bp(w, g) {
            Some(b) => crate::elements::conditions::dlc_restricted_or_report(
                w,
                &b,
                "BlueprintSpellList.GetSpells",
            ),
            None => false,
        };
        if !restricted {
            out.push(g);
        }
    }
    let out = Rc::new(out);
    w.spell_cache.borrow_mut().insert((list, level), out.clone());
    out
}

/// `BlueprintSpellList.IsMythic`.
fn list_is_mythic(w: &World, list: Guid) -> bool {
    bp(w, list).is_some_and(|b| b.view().bool("IsMythic"))
}

/// `BlueprintSpellList.MaxLevel`: the highest `SpellLevel` with spells; it throws when
/// there are none, which is reported.
fn list_max_level(w: &mut World, list: Guid) -> i32 {
    let Some(l) = bp(w, list) else { return 0 };
    let n = l.view().objs("SpellsByLevel").len();
    let mut best: Option<i32> = None;
    for (i, o) in l.view().objs("SpellsByLevel").into_iter().enumerate() {
        let level = View(o).i32("SpellLevel");
        let idx = i32::try_from(i).unwrap_or(0);
        if !list_spells(w, list, idx).is_empty() {
            best = Some(best.map_or(level, |b| b.max(level)));
        }
    }
    let _ = n;
    match best {
        Some(b) => b,
        None => {
            w.coverage
                .unsupported("BlueprintSpellList.MaxLevel: no spells (InvalidOperationException)");
            0
        }
    }
}

fn sb_flag(w: &World, sb: Guid, field: &str) -> bool {
    bp(w, sb).is_some_and(|b| b.view().bool(field))
}

fn sb_spell_list(w: &World, sb: Guid) -> Option<Guid> {
    bp(w, sb).and_then(|b| b.view().guid("m_SpellList"))
}

// ---- access to the unit's spellbooks ----------------------------------------------------

fn book(w: &World, u: EntityIdx, sb: Guid) -> &Spellbook {
    w.unit(u).and_then(|x| x.spellbooks.get(&sb)).expect("spellbook")
}

fn book_mut(w: &mut World, u: EntityIdx, sb: Guid) -> &mut Spellbook {
    w.unit_mut(u).and_then(|x| x.spellbooks.get_mut(&sb)).expect("spellbook")
}

/// `UnitDescriptor.GetSpellbook(blueprint)`.
#[must_use]
pub fn get_spellbook(w: &World, u: EntityIdx, sb: Guid) -> bool {
    w.unit(u).is_some_and(|x| x.spellbooks.contains_key(&sb))
}

/// `UnitDescriptor.GetSpellbook(characterClass)`: the spellbook of the unit's class data.
#[must_use]
pub fn class_spellbook(w: &World, u: EntityIdx, class: Guid) -> Option<Guid> {
    let sb = w.unit(u)?.progression.classes.iter().find(|c| c.class == class)?.spellbook?;
    get_spellbook(w, u, sb).then_some(sb)
}

/// `UnitDescriptor.DemandSpellbook(blueprint)`.
pub fn demand_spellbook(w: &mut World, u: EntityIdx, sb: Guid) {
    if get_spellbook(w, u, sb) {
        return;
    }
    w.spellbook_serial += 1;
    let serial = w.spellbook_serial;
    let max = max_caster_level_of(w, sb);
    let mut s = Spellbook::empty(sb, serial, max);
    let mythic = sb_flag(w, sb, "IsMythic");
    if mythic {
        s.ty = TYPE_STANDALONE_MYTHIC;
    }
    w.unit_mut(u).expect("unit").spellbooks.insert(sb, s);
    if mythic && let Some(l) = bp(w, sb).and_then(|b| b.view().guid("m_MythicSpellList")) {
        add_special_list(w, u, sb, l);
    }
}

/// `UnitDescriptor.DeleteSpellbook`.
pub fn delete_spellbook(w: &mut World, u: EntityIdx, sb: Guid) {
    w.unit_mut(u).expect("unit").spellbooks.remove(&sb);
}

// ---- levels -----------------------------------------------------------------------------

/// `BaseLevel`.
#[must_use]
pub fn base_level(w: &World, u: EntityIdx, sb: Guid) -> i32 {
    let b = book(w, u, sb);
    let modifier = bp(w, sb).map_or(0, |x| x.view().i32("CasterLevelModifier"));
    arith::add(b.base_level, modifier).min(b.max_caster_level).max(0)
}

/// `MythicLevel`.
#[must_use]
pub fn mythic_level(w: &World, u: EntityIdx, sb: Guid) -> i32 {
    let b = book(w, u, sb);
    if b.is_standalone_mythic() { base_level(w, u, sb) } else { b.mythic_level }
}

/// `GetMythicLevelForCasterLevel`.
fn mythic_level_for_caster_level(w: &World, u: EntityIdx, sb: Guid) -> i32 {
    if crate::unit::mythic_level(w, u) <= 3 {
        let last = w.unit(u).and_then(|x| {
            x.progression
                .classes
                .iter()
                .rev()
                .find(|c| bp(w, c.class).is_some_and(|b| b.view().bool("IsMythic")))
                .map(|c| c.class)
        });
        if last.and_then(|c| bp(w, c)).is_some_and(|c| c.view().bool("DontAddCasterLevels")) {
            return 0;
        }
    }
    book(w, u, sb).mythic_level
}

/// `CasterLevel`.
#[must_use]
pub fn caster_level(w: &World, u: EntityIdx, sb: Guid) -> i32 {
    arith::add(base_level(w, u, sb), mythic_level_for_caster_level(w, u, sb))
}

/// `MaxSpellLevel` (`GetMaxSpellLevel`).
#[must_use]
pub fn max_spell_level(w: &World, u: EntityIdx, sb: Guid) -> i32 {
    let cl = caster_level(w, u, sb);
    let levels = per_day_levels(w, sb);
    let mut num = -1;
    for i in (0..=10).rev() {
        if table_count(&levels, cl, i) >= 0 {
            num = i;
            break;
        }
    }
    let ten = w
        .unit(u)
        .and_then(|x| x.mechanics_features.get(&ENABLE_SPELL_LEVEL_10))
        .copied()
        .unwrap_or(0)
        > 0;
    if num == 9 && ten { 10 } else { num }
}

/// `AddLevelFromClass(characterClass)`.
pub fn add_level_from_class(w: &mut World, u: EntityIdx, sb: Guid, class: Guid) {
    let mythic_class = bp(w, class).is_some_and(|c| c.view().bool("IsMythic"));
    if !mythic_class {
        add_base_level(w, u, sb);
    } else if book(w, u, sb).is_mythic() {
        let owner_mythic = crate::unit::mythic_level(w, u);
        let target = if book(w, u, sb).is_standalone_mythic() {
            arith::mul(owner_mythic, 2)
        } else {
            owner_mythic
        };
        while mythic_level(w, u, sb) < target {
            let before = mythic_level(w, u, sb);
            add_mythic_level(w, u, sb);
            if mythic_level(w, u, sb) == before {
                break;
            }
        }
    } else {
        w.log(
            "error",
            "Spellbook.AddLevelFromClass: gain level from mythic class but spellbook isn't mythic",
        );
    }
}

/// `AddBaseLevel()`.
pub fn add_base_level(w: &mut World, u: EntityIdx, sb: Guid) {
    if book(w, u, sb).is_standalone_mythic() {
        w.log("error", "Can't increase base level of standalone mythic spellbook");
        return;
    }
    let old = max_spell_level(w, u, sb);
    book_mut(w, u, sb).base_level += 1;
    let new = max_spell_level(w, u, sb);
    learn_spells_on_raise_level(w, u, sb, old, new, false);
}

/// `AddMythicLevel()`.
pub fn add_mythic_level(w: &mut World, u: EntityIdx, sb: Guid) {
    if !book(w, u, sb).is_mythic() {
        w.log("error", "Can't increase mythic level of not mythic spellbook");
        return;
    }
    let old = max_spell_level(w, u, sb);
    let b = book_mut(w, u, sb);
    if b.is_standalone_mythic() {
        b.base_level += 1;
    } else {
        b.mythic_level += 1;
    }
    let new = max_spell_level(w, u, sb);
    learn_spells_on_raise_level(w, u, sb, old, new, false);
}

/// `LearnSpellsOnRaiseLevel`.
fn learn_spells_on_raise_level(
    w: &mut World,
    u: EntityIdx,
    sb: Guid,
    old_max: i32,
    new_max: i32,
    only_mythic: bool,
) {
    if old_max == new_max {
        return;
    }
    let list = sb_spell_list(w, sb);
    let all_known = sb_flag(w, sb, "AllSpellsKnown");
    if !only_mythic
        && old_max < 0
        && new_max > 0
        && let Some(l) = list
    {
        for spell in list_spells(w, l, 0).iter() {
            if !book(w, u, sb).is_known_on_level(*spell, 0) {
                add_known(w, u, sb, 0, *spell, true);
            }
        }
    }
    if !all_known && !book(w, u, sb).is_mythic() {
        return;
    }
    let mut i = old_max + 1;
    while i <= new_max {
        if all_known
            && !only_mythic
            && let Some(l) = list
        {
            for spell in list_spells(w, l, i).iter() {
                add_known(w, u, sb, i, *spell, true);
            }
        }
        for sl in book(w, u, sb).special_lists.clone() {
            if (!all_known || only_mythic) && !list_is_mythic(w, sl) {
                continue;
            }
            for spell in list_spells(w, sl, i).iter() {
                add_special(w, u, sb, i, sl, *spell);
            }
        }
        i += 1;
    }
}

// ---- known and special spells -----------------------------------------------------------

/// `AddKnown(spellLevel, spell, raiseLearnSpellEvent)`.
pub fn add_known(w: &mut World, u: EntityIdx, sb: Guid, level: i32, spell: Guid, raise: bool) {
    let Ok(lv) = usize::try_from(level) else {
        w.log("error", "Trying to add known spell on invalid level");
        return;
    };
    // `SureKnownSpells(level)[...]` is out of range above level 10.
    if lv >= LEVELS {
        w.coverage
            .unsupported("Spellbook.AddKnown: spell level above 10 (IndexOutOfRangeException)");
        return;
    }
    let existing = book(w, u, sb).known[lv].iter().position(|s| s.blueprint == spell);
    let data = match existing {
        Some(i) => book(w, u, sb).known[lv][i].clone(),
        None => Spell::new(w, spell, level),
    };
    if data.is_temporary {
        // `abilityData.IsTemporary = false` on the entry the list holds.
        if let Some(i) = existing {
            book_mut(w, u, sb).known[lv][i].is_temporary = false;
        }
    } else {
        book_mut(w, u, sb).known[lv].push(data);
    }
    book_mut(w, u, sb).add_known_spell_level(spell, level);
    if !sb_flag(w, sb, "AllSpellsKnown") {
        let lists = book(w, u, sb).special_lists.clone();
        let mut hit = None;
        for l in lists {
            if list_spells(w, l, level).contains(&spell) {
                hit = Some(l);
                break;
            }
        }
        if let Some(l) = hit {
            add_special(w, u, sb, level, l, spell);
        }
    }
    if raise {
        crate::rulebook::bus::raise_global_call(w, "ILearnSpellHandler", "HandleLearnSpell", None);
    }
}

/// `AddSpecial(spellLevel, source, spell)`.
fn add_special(w: &mut World, u: EntityIdx, sb: Guid, level: i32, source: Guid, spell: Guid) {
    let Ok(lv) = usize::try_from(level) else { return };
    if lv >= LEVELS {
        w.coverage
            .unsupported("Spellbook.AddSpecial: spell level above 10 (IndexOutOfRangeException)");
        return;
    }
    let mut item = Spell::new(w, spell, level);
    let mythic = list_is_mythic(w, source);
    item.is_from_mythic_spell_list = mythic;
    let b = book_mut(w, u, sb);
    if mythic {
        if !b.known[lv].iter().any(|s| s.same(&item)) {
            b.known[lv].push(item);
        }
    } else {
        b.special[lv].push(item);
    }
    b.add_known_spell_level(spell, level);
}

/// `AddSpecialList(spellList)`.
pub fn add_special_list(w: &mut World, u: EntityIdx, sb: Guid, list: Guid) {
    let mythic = list_is_mythic(w, list);
    {
        let b = book_mut(w, u, sb);
        if mythic && b.ty == TYPE_NORMAL {
            b.ty = TYPE_MYTHIC;
        }
        b.special_lists.push(list);
    }
    if sb_flag(w, sb, "AllSpellsKnown") || mythic {
        let max = max_spell_level(w, u, sb);
        for i in 1..=max {
            for spell in list_spells(w, list, i).iter() {
                add_special(w, u, sb, i, list, *spell);
            }
        }
        return;
    }
    let snapshot: Vec<(Guid, Vec<i32>)> =
        book(w, u, sb).known_levels.iter().map(|(k, v)| (*k, v.clone())).collect();
    for (key, levels) in snapshot {
        for item in levels {
            if list_spells(w, list, item).contains(&key) {
                add_special(w, u, sb, item, list, key);
            }
        }
    }
}

/// `RemoveSpecialList(spellList)`.
pub fn remove_special_list(w: &mut World, u: EntityIdx, sb: Guid, list: Guid) {
    if list_is_mythic(w, list) {
        w.log("error", "Can't remove Mythic spell list");
        return;
    }
    {
        let b = book_mut(w, u, sb);
        let Some(p) = b.special_lists.iter().position(|l| *l == list) else { return };
        b.special_lists.remove(p);
    }
    let Some(l) = bp(w, list) else { return };
    for sll in l.view().objs("SpellsByLevel") {
        let level = View(sll).i32("SpellLevel");
        let Ok(lv) = usize::try_from(level) else { continue };
        if lv >= LEVELS {
            w.coverage.unsupported("Spellbook.RemoveSpecialList: spell level above 10");
            continue;
        }
        // `GetMemorizedSpells(level).ToArray()`: the slots holding a spell, now.
        let slots: Vec<SlotKey> = book(w, u, sb).memorized[lv]
            .iter()
            .filter(|s| s.spell.is_some())
            .map(Slot::key)
            .collect();
        for spell in View(sll).guids("m_Spells") {
            let b = book_mut(w, u, sb);
            b.remove_known_spell_level(spell, level);
            if let Some(p) = b.special[lv].iter().position(|s| s.blueprint == spell) {
                b.special[lv].remove(p);
            }
            let hits: Vec<SlotKey> = slots
                .iter()
                .copied()
                .filter(|k| {
                    b.slot(lv, *k)
                        .and_then(|s| s.spell.as_ref())
                        .is_some_and(|s| s.blueprint == spell)
                })
                .collect();
            for k in hits {
                forget_memorized(w, u, sb, lv, k);
            }
        }
    }
}

/// `RemoveSpell(spell)`.
pub fn remove_spell(w: &mut World, u: EntityIdx, sb: Guid, spell: Guid) {
    let mut cleared = Vec::new();
    {
        let b = book_mut(w, u, sb);
        b.known_levels.remove(&spell);
        for l in b.known.iter_mut().chain(b.custom.iter_mut()).chain(b.special.iter_mut()) {
            l.retain(|s| s.blueprint != spell);
        }
        for (lv, list) in b.memorized.iter().enumerate() {
            for s in list {
                if s.spell.as_ref().is_some_and(|x| x.blueprint == spell) {
                    cleared.push((lv, s.key()));
                }
            }
        }
        for (lv, k) in cleared {
            b.clear_slot(lv, k);
        }
    }
}

/// `UpdateMythicLevel()`.
pub fn update_mythic_level(w: &mut World, u: EntityIdx, sb: Guid) {
    if !book(w, u, sb).is_mythic() {
        return;
    }
    let old = max_spell_level(w, u, sb);
    let owner_mythic = crate::unit::mythic_level(w, u);
    let standalone = book(w, u, sb).is_standalone_mythic();
    let num = if owner_mythic >= 3 {
        if standalone { arith::mul(owner_mythic, 2) } else { owner_mythic }
    } else {
        0
    };
    {
        let b = book_mut(w, u, sb);
        if standalone {
            b.base_level = num;
        } else {
            b.mythic_level = num;
        }
    }
    let new = max_spell_level(w, u, sb);
    let mut level = old;
    while level > new {
        if let Ok(lv) = usize::try_from(level)
            && lv < LEVELS
        {
            for item in book(w, u, sb).known[lv].clone() {
                if !item.copied_from_scroll && !item.is_temporary {
                    remove_spell(w, u, sb, item.blueprint);
                }
            }
        }
        level -= 1;
    }
}

// ---- slots ------------------------------------------------------------------------------

/// `GetSpellsPerDay(spellLevel)`.
pub fn spells_per_day(w: &mut World, u: EntityIdx, sb: Guid, level: i32) -> i32 {
    let standalone = book(w, u, sb).is_standalone_mythic();
    let Some(sbp) = bp(w, sb) else { return 0 };
    if sbp
        .components_array()
        .iter()
        .any(|o| crate::content::is_a(o.ty, "MagicHackSpellbookComponent"))
    {
        w.coverage.unsupported("Spellbook.GetSpellsPerDay: MagicHackSpellbookComponent");
        return 0;
    }
    // `Owner.Get<UnitPartExtraSpellsPerDay>()?.BonusSpells.Get(spellLevel, 0) ?? 0`.
    let extra = usize::try_from(level)
        .ok()
        .and_then(|l| w.unit(u).and_then(|x| x.extra_spells_per_day.as_ref()?.get(l).copied()))
        .unwrap_or(0);
    let attr = StatType(sbp.view().i32("CastingAttribute"));
    let is_attr = matches!(w.stats(u).v(attr).kind, StatKind::Attribute { .. });
    if !standalone {
        if !is_attr {
            return 0;
        }
        let modified = w.stats(u).v(attr).modified_value;
        let perm = ops::permanent_value_without_temp_buffs(w, u, attr);
        if modified.min(perm) < arith::add(10, level) {
            return 0;
        }
    }
    let cl = caster_level(w, u, sb);
    let count = table_count(&per_day_levels(w, sb), cl, level);
    if count == -1 {
        return 0;
    }
    let mut num = count;
    if !standalone {
        let player = w.unit(u).is_some_and(|x| x.is_player_faction);
        let base = if player {
            ops::permanent_value_without_temp_buffs(w, u, attr)
        } else {
            w.stats(u).v(attr).base_value
        };
        num = arith::add(num, extra);
        let n2 = arith::sub(arith::sub(base, 10) / 2, level);
        if n2 >= 0 && level > 0 {
            num = arith::add(num, n2 / 4 + 1);
        }
    }
    num
}

/// `GetSpellSlotsCount(spellLevel)`.
fn spell_slots_count(w: &mut World, u: EntityIdx, sb: Guid, level: i32) -> i32 {
    if sb_flag(w, sb, "IsArcanist") {
        let slots = bp(w, sb).and_then(|b| {
            b.view().guid("m_SpellSlots").or_else(|| b.view().guid("m_SpellsPerDay"))
        });
        let cl = caster_level(w, u, sb);
        return table_count(&table_levels(w, slots), cl, level).max(0);
    }
    spells_per_day(w, u, sb, level)
}

/// `CalcSlotsLimit(spellLevel, slotType)`.
fn calc_slots_limit(w: &mut World, u: EntityIdx, sb: Guid, level: i32, ty: i32) -> i32 {
    if ty == COMMON {
        return spell_slots_count(w, u, sb, level);
    }
    if level > max_spell_level(w, u, sb) {
        return 0;
    }
    let mut num = 0;
    for l in book(w, u, sb).special_lists.clone() {
        if !list_is_mythic(w, l) && level <= list_max_level(w, l) {
            num += 1;
        }
    }
    if num <= 0 {
        return 0;
    }
    let sin = bp(w, sb).is_some_and(|b| {
        b.components_array()
            .iter()
            .any(|o| crate::content::is_a(o.ty, "IsSinMagicSpecialistSpellbook"))
    });
    if sin { 2 } else { 1 }
}

/// `UpdateAllSlotsSize(allowShrink)`.
pub fn update_all_slots_size(w: &mut World, u: EntityIdx, sb: Guid, allow_shrink: bool) {
    let max = max_spell_level(w, u, sb);
    for i in 1..=max {
        update_slots_size(w, u, sb, i, allow_shrink);
    }
}

/// `UpdateSlotsSize(spellLevel, allowShrink)`.
fn update_slots_size(w: &mut World, u: EntityIdx, sb: Guid, level: i32, allow_shrink: bool) {
    let Ok(lv) = usize::try_from(level) else { return };
    if lv >= LEVELS {
        return;
    }
    let num = calc_slots_limit(w, u, sb, level, COMMON);
    let num2 = calc_slots_limit(w, u, sb, level, FAVORITE);
    let b = book_mut(w, u, sb);
    let count = i32::try_from(b.memorized[lv].len()).unwrap_or(i32::MAX);
    if count == arith::add(num, num2) || (!allow_shrink && count > arith::add(num, num2)) {
        return;
    }
    let (mut num3, mut num4) = (-1, -1);
    for s in &b.memorized[lv] {
        if s.ty == COMMON {
            num3 = num3.max(s.index);
        } else {
            num4 = num4.max(s.index);
        }
    }
    b.hidden_common.get_or_insert_with(level, CsDict::new);
    b.hidden_favorite.get_or_insert_with(level, CsDict::new);
    let mut slots_added = false;
    for (ty, max, limit) in [(COMMON, num3, num), (FAVORITE, num4, num2)] {
        let hidden = if ty == COMMON { &mut b.hidden_common } else { &mut b.hidden_favorite };
        let hidden = hidden.get_mut(&level).expect("inserted");
        let mut i = max + 1;
        while i < limit {
            if let Some(h) = hidden.remove(&i) {
                b.memorized[lv].push(h);
            } else {
                b.memorized[lv].push(Slot::new(level, ty, i));
            }
            slots_added = true;
            i += 1;
        }
    }
    if slots_added {
        b.memorized[lv].sort_by_key(|s| (s.ty, s.index));
    }
    if allow_shrink {
        for (ty, max, limit) in [(COMMON, num3, num), (FAVORITE, num4, num2)] {
            if max < limit {
                continue;
            }
            let hidden = if ty == COMMON { &mut b.hidden_common } else { &mut b.hidden_favorite };
            let hidden = hidden.get_mut(&level).expect("inserted");
            let mut i = 0;
            while i < b.memorized[lv].len() {
                let s = &b.memorized[lv][i];
                if s.ty == ty && s.index >= limit {
                    hidden.insert(s.index, s.clone());
                    b.memorized[lv].remove(i);
                } else {
                    i += 1;
                }
            }
        }
    }
}

/// `GetFreeSlots(slots, type, slotsCount, includeSlot = null)`.
fn free_slots(slots: &[Slot], ty: i32, count: usize) -> Option<Vec<SlotKey>> {
    let mut out = Vec::new();
    for s in slots {
        if s.ty == ty && s.spell.is_none() {
            out.push(s.key());
            if out.len() >= count {
                return Some(out);
            }
        }
    }
    None
}

/// `OppositionSchools.Contains(school) || OppositionDescriptors.HasAnyFlag(descriptor)`
/// for a spell of the spellbook: false without opposition, else reported.
fn is_opposition(w: &mut World, u: EntityIdx, sb: Guid) -> bool {
    let b = book(w, u, sb);
    if b.opposition_schools.is_empty() && b.opposition_descriptors == 0 {
        return false;
    }
    w.coverage.unsupported("Spellbook: opposition schools or descriptors (spell school lookup)");
    false
}

/// The unit's spellbooks' blueprints in dictionary order (`Descriptor.Spellbooks`).
#[must_use]
pub fn spellbooks_of(w: &World, u: EntityIdx) -> Vec<Guid> {
    w.unit(u).map(|x| x.spellbooks.keys().collect()).unwrap_or_default()
}

/// `GetKnownSpells(level).Any(p => p.Blueprint == spell)`.
#[must_use]
pub fn knows_at(w: &World, u: EntityIdx, sb: Guid, level: i32, spell: Guid) -> bool {
    usize::try_from(level)
        .ok()
        .and_then(|l| book(w, u, sb).known.get(l))
        .is_some_and(|l| l.iter().any(|s| s.blueprint == spell))
}

/// `GetMinSpellLevel(spell)`.
#[must_use]
pub fn min_spell_level(w: &World, u: EntityIdx, sb: Guid, spell: Guid) -> i32 {
    book(w, u, sb).min_spell_level(spell)
}

/// `GetAllKnownSpells()` with each spell's `SpellLevel`: (level, blueprint) in list order.
#[must_use]
pub fn all_known_spells(w: &World, u: EntityIdx, sb: Guid) -> Vec<(i32, Guid)> {
    let b = book(w, u, sb);
    b.known.iter().flatten().map(|s| (b.spell_level_of(s), s.blueprint)).collect()
}

/// `Memorize(data, slot = null)`.
pub fn memorize(w: &mut World, u: EntityIdx, sb: Guid, data: &Spell) -> bool {
    // `Blueprint.MemorizeSpells`: not spontaneous, or an arcanist.
    if sb_flag(w, sb, "Spontaneous") && !sb_flag(w, sb, "IsArcanist") {
        w.log("warning", "Trying to memorize or forget memorized spell for spontaneous casters.");
        return false;
    }
    let level = book(w, u, sb).spell_level_of(data);
    let Some(lv) = usize::try_from(level).ok().filter(|l| *l < LEVELS) else { return false };
    if sb_flag(w, sb, "IsArcanist")
        && book(w, u, sb).memorized[lv]
            .iter()
            .any(|s| s.spell.as_ref().is_some_and(|x| x.blueprint == data.blueprint))
    {
        w.log("warning", "Arcanist can't memorize same spell twice.");
        return false;
    }
    // `GetMemorizeSlots`.
    let b = book(w, u, sb);
    let special = b.special[lv].iter().any(|s| s.blueprint == data.blueprint);
    let known = b.known[lv].iter().any(|s| s.blueprint == data.blueprint);
    let opposed = is_opposition(w, u, sb);
    let sin = bp(w, sb).is_some_and(|x| {
        x.components_array()
            .iter()
            .any(|o| crate::content::is_a(o.ty, "IsSinMagicSpecialistSpellbook"))
    });
    let mut num = if !opposed && (!special || !sin) { 1 } else { 2 };
    let slots = book(w, u, sb).memorized[lv].clone();
    let mut chosen = None;
    if special {
        chosen = free_slots(&slots, FAVORITE, num);
    }
    if chosen.is_none() && known {
        if !opposed {
            num = 1;
        }
        chosen = free_slots(&slots, COMMON, num);
    }
    let Some(chosen) = chosen else { return false };
    // `ClearSlots`, then each slot takes the spell and the link.
    for k in &chosen {
        book_mut(w, u, sb).clear_slot(lv, *k);
    }
    for k in &chosen {
        let b = book_mut(w, u, sb);
        b.set_spell_shell(lv, *k, Some(data.clone()));
        if let Some(s) = b.slot_mut(lv, *k) {
            s.linked = Some(chosen.clone());
            s.is_opposition = opposed;
        }
    }
    crate::rulebook::bus::raise_global_call(
        w,
        "ISpellBookUIHandler",
        "HandleMemorizedSpell",
        Some(u),
    );
    true
}

/// `ForgetMemorized(slot)`.
fn forget_memorized(w: &mut World, u: EntityIdx, sb: Guid, level: usize, key: SlotKey) {
    if sb_flag(w, sb, "Spontaneous") && !sb_flag(w, sb, "IsArcanist") {
        w.log("warning", "Trying to forget memorized spell for spontaneous casters.");
        return;
    }
    if book(w, u, sb).slot(level, key).is_some_and(|s| s.spell.is_some()) {
        book_mut(w, u, sb).clear_slot(level, key);
        crate::rulebook::bus::raise_global_call(
            w,
            "ISpellBookUIHandler",
            "HandleForgetSpell",
            Some(u),
        );
    }
}

/// `Rest()`.
pub fn rest(w: &mut World, u: EntityIdx, sb: Guid) {
    update_all_slots_size(w, u, sb, true);
    if sb_flag(w, sb, "Spontaneous") {
        for i in 0..LEVELS {
            let n = spells_per_day(w, u, sb, i32::try_from(i).unwrap_or(0));
            book_mut(w, u, sb).spontaneous[i] = n;
        }
    }
    let memorize_spells = !sb_flag(w, sb, "Spontaneous") || sb_flag(w, sb, "IsArcanist");
    let b = book_mut(w, u, sb);
    if memorize_spells {
        for list in &mut b.memorized {
            for s in list {
                s.available = true;
            }
        }
    }
    b.remove_memorized.clear();
}

// ---- load -------------------------------------------------------------------------------

/// `Spellbook.PostLoad(owner)` for every spellbook of a unit, in dictionary order
/// (`UnitDescriptor.PostLoad`).
pub fn post_load_unit(w: &mut World, u: EntityIdx) {
    let keys: Vec<Guid> = w.unit(u).map(|x| x.spellbooks.keys().collect()).unwrap_or_default();
    for sb in keys {
        post_load(w, u, sb);
    }
}

fn post_load(w: &mut World, u: EntityIdx, sb: Guid) {
    let max = max_caster_level_of(w, sb);
    {
        let b = book_mut(w, u, sb);
        b.max_caster_level = max;
        for lv in 0..LEVELS {
            let level = i32::try_from(lv).unwrap_or(0);
            let known: Vec<Guid> = b.known[lv].iter().map(|s| s.blueprint).collect();
            for s in &mut b.known[lv] {
                s.level_in_spellbook = Some(level);
            }
            for g in known {
                b.add_known_spell_level(g, level);
            }
        }
        for lv in 0..LEVELS {
            let level = i32::try_from(lv).unwrap_or(0);
            let special: Vec<Guid> = b.special[lv].iter().map(|s| s.blueprint).collect();
            for s in &mut b.special[lv] {
                s.level_in_spellbook = Some(level);
            }
            for g in special {
                b.add_known_spell_level(g, level);
            }
        }
        for lv in 0..LEVELS {
            let level = i32::try_from(lv).unwrap_or(0);
            for s in &mut b.custom[lv] {
                s.level_in_spellbook = Some(level);
            }
        }
    }
    try_restore_known_spells(w, u, sb);
    // The slots' spells: hidden common, hidden favorite, then the memorized lists.
    let b = book_mut(w, u, sb);
    for dict in [&mut b.hidden_common, &mut b.hidden_favorite] {
        for (_, inner) in dict.iter_mut() {
            for (_, slot) in inner.iter_mut() {
                let l = slot.spell_level;
                if let Some(s) = &mut slot.spell {
                    s.level_in_spellbook = Some(l);
                }
            }
        }
    }
    for list in &mut b.memorized {
        for slot in list {
            let l = slot.spell_level;
            if let Some(s) = &mut slot.spell {
                s.level_in_spellbook = Some(l);
            }
        }
    }
    // `RemoveMemorizedSpells` holds no level of its own: an entry that is the same object as
    // a known, special, custom or slot spell (a shared `$ref`) has the level the last of
    // those `PostLoad` calls gave that object, in the order above.
    let mut last: HashMap<usize, Option<i32>> = HashMap::new();
    {
        let mut note = |s: &Spell| {
            if let Some(o) = s.saved_object {
                last.insert(o, s.level_in_spellbook);
            }
        };
        for lists in [&b.known, &b.special, &b.custom] {
            lists.iter().flatten().for_each(&mut note);
        }
        for dict in [&b.hidden_common, &b.hidden_favorite] {
            for (_, inner) in dict.iter() {
                inner.iter().filter_map(|(_, slot)| slot.spell.as_ref()).for_each(&mut note);
            }
        }
        b.memorized.iter().flatten().filter_map(|slot| slot.spell.as_ref()).for_each(&mut note);
    }
    for r in &mut b.remove_memorized {
        if let Some(l) = r.saved_object.and_then(|o| last.get(&o)) {
            r.level_in_spellbook = *l;
        }
    }
    if !book(w, u, sb).is_standalone_mythic() {
        let attr = StatType(bp(w, sb).map_or(0, |x| x.view().i32("CastingAttribute")));
        let serial = book(w, u, sb).serial;
        // `(Owner.Stats.GetStat(CastingAttribute) as ModifiableValueAttributeStat).OnChanged += …`.
        ops::subscribe_changed_handler(w, u, attr, ChangedHandler::Spellbook(serial));
    }
}

/// `TryRestoreKnownSpells()`.
fn try_restore_known_spells(w: &mut World, u: EntityIdx, sb: Guid) {
    let max = max_spell_level(w, u, sb);
    let all_known = sb_flag(w, sb, "AllSpellsKnown");
    let list = sb_spell_list(w, sb);
    for i in 1..=max {
        let Ok(lv) = usize::try_from(i) else { continue };
        if lv >= LEVELS {
            continue;
        }
        if !all_known {
            for sl in book(w, u, sb).special_lists.clone() {
                for spell in list_spells(w, sl, i).iter() {
                    if list_is_mythic(w, sl) {
                        if !book(w, u, sb).is_known_on_level(*spell, i) {
                            add_special(w, u, sb, i, sl, *spell);
                            w.log("error", "Restore spell");
                        }
                    } else if book(w, u, sb).is_known_on_level(*spell, i)
                        && !book(w, u, sb).special[lv].iter().any(|s| s.blueprint == *spell)
                    {
                        add_special(w, u, sb, i, sl, *spell);
                        w.log("error", "Restore special spell");
                    }
                }
            }
            continue;
        }
        if let Some(l) = list {
            for spell in list_spells(w, l, i).iter() {
                if !book(w, u, sb).is_known_on_level(*spell, i) {
                    add_known(w, u, sb, i, *spell, true);
                    w.log("error", "Restore spell");
                }
            }
        }
        for sl in book(w, u, sb).special_lists.clone() {
            for spell in list_spells(w, sl, i).iter() {
                if !book(w, u, sb).is_known_on_level(*spell, i) {
                    add_special(w, u, sb, i, sl, *spell);
                    w.log("error", "Restore spell");
                } else if !list_is_mythic(w, sl)
                    && !book(w, u, sb).special[lv].iter().any(|s| s.blueprint == *spell)
                {
                    // `SureKnownSpells(i).Find(x => x.Blueprint == spell)`: null when absent.
                    let found =
                        book(w, u, sb).known[lv].iter().find(|s| s.blueprint == *spell).cloned();
                    match found {
                        Some(item) => book_mut(w, u, sb).special[lv].push(item),
                        None => w
                            .coverage
                            .unsupported("Spellbook.TryRestoreKnownSpells: null spell added"),
                    }
                }
            }
        }
    }
}

/// `UnitDescriptor.ApplyPostLoadFixes`'s spellbook part: drop a spellbook with no level,
/// else its own fixes.
pub fn apply_post_load_fixes_unit(w: &mut World, u: EntityIdx) {
    let keys: Vec<Guid> = w.unit(u).map(|x| x.spellbooks.keys().collect()).unwrap_or_default();
    for sb in keys {
        let b = book(w, u, sb);
        if b.base_level < 1 && mythic_level(w, u, sb) < 1 {
            delete_spellbook(w, u, sb);
        } else {
            apply_post_load_fixes(w, u, sb);
        }
    }
}

/// `UnitProgressionData.ApplyPostLoadFixes`'s spellbook part: a class whose spellbook has a
/// negative caster level modifier (paladin, ranger) gets its spellbook raised to the class level.
pub fn restore_class_spellbooks(w: &mut World, u: EntityIdx) {
    let classes = w.unit(u).map(|x| x.progression.classes.clone()).unwrap_or_default();
    for cd in classes {
        let Some(sb) = cd.spellbook else { continue };
        if bp(w, sb).is_none_or(|b| b.view().i32("CasterLevelModifier") >= 0) {
            continue;
        }
        demand_spellbook(w, u, sb);
        if cd.level <= 0 || book(w, u, sb).base_level >= cd.level {
            continue;
        }
        while book(w, u, sb).base_level < cd.level {
            let before = book(w, u, sb).base_level;
            add_base_level(w, u, sb);
            if before == book(w, u, sb).base_level {
                break;
            }
        }
        w.log("info", "Restore spellbook");
    }
}

/// `Spellbook.ApplyPostLoadFixes()`.
fn apply_post_load_fixes(w: &mut World, u: EntityIdx, sb: Guid) {
    if let Some(l) = sb_spell_list(w, sb) {
        let spells = list_spells(w, l, 0);
        let known0 = book(w, u, sb).known[0].len();
        if !spells.is_empty() && known0 != spells.len() {
            for spell in spells.iter() {
                if !book(w, u, sb).is_known_on_level(*spell, 0) {
                    add_known(w, u, sb, 0, *spell, true);
                }
            }
        }
    }
    if max_spell_level(w, u, sb) != 10 || !book(w, u, sb).known[10].is_empty() {
        return;
    }
    for sl in book(w, u, sb).special_lists.clone() {
        if !list_is_mythic(w, sl) {
            continue;
        }
        for spell in list_spells(w, sl, 10).iter() {
            add_special(w, u, sb, 10, sl, *spell);
        }
    }
}

/// The casting stat's `OnChanged` handler: `UpdateAllSlotsSize(allowShrink: true)` of the
/// spellbook (a spellbook deleted since has nothing to update).
pub fn on_casting_stat_changed(w: &mut World, u: EntityIdx, serial: u32) {
    let sb = w
        .unit(u)
        .and_then(|x| x.spellbooks.iter().find(|(_, b)| b.serial == serial).map(|(k, _)| *k));
    if let Some(sb) = sb {
        // What this reports belongs to the spellbook's owner, whoever's code changed the stat.
        let outer = w.coverage.context.replace(w.entity(u).unique_id.clone());
        update_all_slots_size(w, u, sb, true);
        w.coverage.context = outer;
    }
}

// ---- unit-level flows -------------------------------------------------------------------

/// `UnitHelper.Replenish`'s spells: every spellbook rests, then `ISpellBookRest`.
pub fn rest_unit(w: &mut World, u: EntityIdx) {
    let keys: Vec<Guid> = w.unit(u).map(|x| x.spellbooks.keys().collect()).unwrap_or_default();
    for sb in keys {
        rest(w, u, sb);
    }
    crate::rulebook::bus::raise_global_call(w, "ISpellBookRest", "OnSpellBookRestHandler", Some(u));
}

/// `UnitHelper.CopySpellbook(original, target)`.
pub fn copy_spellbooks(w: &mut World, original: EntityIdx, target: EntityIdx) {
    let keys: Vec<Guid> =
        w.unit(original).map(|x| x.spellbooks.keys().collect()).unwrap_or_default();
    for sb in keys {
        demand_spellbook(w, target, sb);
        let src = book(w, original, sb).clone();
        for l in &src.special_lists {
            if !book(w, target, sb).special_lists.contains(l) {
                add_special_list(w, target, sb, *l);
            }
        }
        if !src.is_standalone_mythic() {
            while book(w, target, sb).base_level < src.base_level {
                add_base_level(w, target, sb);
            }
        }
        if src.is_mythic() {
            let want = mythic_level(w, original, sb);
            while mythic_level(w, target, sb) < want {
                add_mythic_level(w, target, sb);
            }
        }
        let b = book_mut(w, target, sb);
        b.opposition_schools.extend(src.opposition_schools.iter().copied());
        b.opposition_descriptors = src.opposition_descriptors;
        for lv in 0..LEVELS {
            for spell in &src.known[lv] {
                let level = src.spell_level_of(spell);
                add_known(w, target, sb, level, spell.blueprint, true);
            }
        }
    }
}

// ---- the oracle's persisted form --------------------------------------------------------

fn persisted_spell(sp: &Spell, depth: i32) -> J {
    // `Value(spell, depth)`.
    if depth >= MAX_DEPTH {
        return json!({"$depth": "AbilityData"});
    }
    let d = depth + 1;
    let flag = if d >= MAX_DEPTH {
        json!({"$depth": "CountableFlag"})
    } else {
        json!({"m_Count": sp.has_action_bar_slot})
    };
    json!({
        "m_ConvertedFrom": null,
        "Blueprint": sp.blueprint.to_string(),
        "UniqueId": sp.unique_id,
        "ExtraSpellSlotCost": sp.extra_spell_slot_cost,
        "OverrideRequiredResource": null,
        "SaveSpellbookSlot": sp.save_spellbook_slot,
        "ForceTargetOwner": sp.force_target_owner,
        "OverridenResourceLogic": null,
        "DecorationColorNumber": sp.decoration_color,
        "DecorationBorderNumber": sp.decoration_border,
        "IsMysticTheurgeCombinedSpell": sp.is_mystic_theurge_combined,
        "MetamagicData": null,
        "MagicHackData": null,
        "WasSubstituted": sp.was_substituted,
        "IsTemporary": sp.is_temporary,
        "CopiedFromScroll": sp.copied_from_scroll,
        "IsFromMythicSpellList": sp.is_from_mythic_spell_list,
        "HasActionBarSlot": flag,
        "TemporarilyDisabled": sp.temporarily_disabled,
    })
}

/// `Value(slot, depth)` with the writer's identity stack.
fn persisted_slot(
    b: &Spellbook,
    level: usize,
    slot: &Slot,
    depth: i32,
    stack: &mut Vec<SlotKey>,
) -> J {
    if depth >= MAX_DEPTH {
        return json!({"$depth": "SpellSlot"});
    }
    if stack.contains(&slot.key()) {
        return json!({"$cycle": "SpellSlot"});
    }
    stack.push(slot.key());
    let d = depth + 1;
    let spell = slot.spell.as_ref().map_or(J::Null, |s| persisted_spell(s, d));
    let linked = match &slot.linked {
        None => J::Null,
        Some(keys) => J::Array(
            keys.iter()
                .map(|k| match b.slot(level, *k) {
                    Some(s) => persisted_slot(b, level, s, d + 1, stack),
                    None => J::Null,
                })
                .collect(),
        ),
    };
    stack.pop();
    json!({
        "SpellLevel": slot.spell_level,
        "Type": slot.ty,
        "Index": slot.index,
        "Spell": spell,
        "Available": slot.available,
        "LinkedSlots": linked,
        "IsOpposition": slot.is_opposition,
    })
}

fn persisted_hidden(b: &Spellbook, hidden: &CsDict<i32, CsDict<i32, Slot>>) -> J {
    // A dictionary at depth 1: pairs of (key at depth 2, value at depth 2); the inner
    // dictionary's pairs hold their key and slot at depth 3.
    J::Array(
        hidden
            .iter()
            .map(|(level, inner)| {
                let lv = usize::try_from(*level).unwrap_or(0);
                let pairs: Vec<J> = inner
                    .iter()
                    .map(|(k, slot)| {
                        let mut stack = Vec::new();
                        json!([k, persisted_slot(b, lv.min(LEVELS - 1), slot, 3, &mut stack)])
                    })
                    .collect();
                json!([level, pairs])
            })
            .collect(),
    )
}

/// The spellbook's persisted members as the oracle's dump writes them
/// (`Snap.Persisted` of the spellbook).
#[must_use]
pub fn persisted(b: &Spellbook) -> J {
    let spells = |lists: &[Vec<Spell>]| -> J {
        J::Array(
            lists
                .iter()
                .map(|l| J::Array(l.iter().map(|s| persisted_spell(s, 3)).collect()))
                .collect(),
        )
    };
    let memorized: Vec<J> = b
        .memorized
        .iter()
        .enumerate()
        .map(|(lv, l)| {
            J::Array(
                l.iter()
                    .map(|s| {
                        let mut stack = Vec::new();
                        persisted_slot(b, lv, s, 3, &mut stack)
                    })
                    .collect(),
            )
        })
        .collect();
    json!({
        "m_BaseLevelInternal": b.base_level,
        "m_MythicLevelInternal": b.mythic_level,
        "m_Type": b.ty,
        "m_KnownSpells": spells(&b.known),
        "m_SpecialSpells": spells(&b.special),
        "m_SpecialLists": b.special_lists.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "m_CustomSpells": spells(&b.custom),
        "m_MemorizedSpells": memorized,
        "m_HiddenCommonSlots": persisted_hidden(b, &b.hidden_common),
        "m_HiddenFavoriteSlots": persisted_hidden(b, &b.hidden_favorite),
        "RemoveMemorizedSpells": J::Array(b.remove_memorized.iter().map(|s| persisted_spell(s, 2)).collect()),
        "m_SpontaneousSlots": b.spontaneous.to_vec(),
        "Blueprint": b.blueprint.to_string(),
        "OppositionDescriptors": b.opposition_descriptors,
        "OppositionSchools": b.opposition_schools,
        "ExOppositionSchools": b.ex_opposition_schools,
        "IgnoreOppositionSchools": b.ignore_opposition_schools,
        "BonusSpellSlots": b.bonus_spell_slots.to_vec(),
    })
}

/// The `spellbooks[]` entry of a sheet.
#[must_use]
pub fn dump(w: &World, u: EntityIdx) -> Vec<J> {
    let Some(unit) = w.unit(u) else { return Vec::new() };
    let mut out: Vec<(String, J)> = unit
        .spellbooks
        .iter()
        .map(|(g, b)| {
            (
                g.to_string(),
                json!({"blueprint": g.to_string(), "caster_level": caster_level(w, u, *g), "persisted": persisted(b)}),
            )
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out.into_iter().map(|(_, j)| j).collect()
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use serde_json::json;
    use wr_content::node::Node;

    use super::*;
    use crate::content::{Blueprint, Source};
    use crate::facts::tests::obj;
    use crate::world::World;

    const TABLE: u128 = 0x10;
    const LIST: u128 = 0x11;
    const BOOK: u128 = 0x12;
    const SPELLS: [u128; 4] = [0x21, 0x22, 0x23, 0x24];

    fn insert(w: &World, g: u128, name: &str, o: wr_content::node::Object) {
        w.content.insert(Blueprint {
            guid: Guid(g),
            name: name.into(),
            obj: o,
            source: Source::Bbp,
        });
    }

    fn int_seq(v: &[i64]) -> Node {
        Node::Seq(v.iter().map(|i| Node::Int(*i)).collect())
    }

    /// A wizard-like spellbook: slots by class level (1: 3 cantrips and 1 first-level slot,
    /// 2: 3 and 2, 3: 4, 2 and 1), cantrips SPELLS[0..2], first-level spells SPELLS[2..4].
    fn world() -> (World, EntityIdx) {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        w.registry = Rc::new(crate::components::registry());
        let entry = |c: &[i64]| {
            Node::Object(Box::new(obj(
                "Kingmaker.Blueprints.Classes.Spells.SpellsLevelEntry",
                &[("Count", int_seq(c))],
            )))
        };
        insert(
            &w,
            TABLE,
            "Table",
            obj(
                "Kingmaker.Blueprints.Classes.Spells.BlueprintSpellsTable",
                &[(
                    "Levels",
                    Node::Seq(vec![entry(&[]), entry(&[3, 1]), entry(&[3, 2]), entry(&[4, 2, 1])]),
                )],
            ),
        );
        let level = |n: i64, spells: &[u128]| {
            Node::Object(Box::new(obj(
                "Kingmaker.Blueprints.Classes.Spells.SpellLevelList",
                &[
                    ("SpellLevel", Node::Int(n)),
                    ("m_Spells", Node::Seq(spells.iter().map(|s| Node::BpRef(Guid(*s))).collect())),
                ],
            )))
        };
        insert(
            &w,
            LIST,
            "List",
            obj(
                "Kingmaker.Blueprints.Classes.Spells.BlueprintSpellList",
                &[(
                    "SpellsByLevel",
                    Node::Seq(vec![level(0, &SPELLS[..2]), level(1, &SPELLS[2..])]),
                )],
            ),
        );
        insert(
            &w,
            BOOK,
            "Book",
            obj(
                "Kingmaker.Blueprints.Classes.Spells.BlueprintSpellbook",
                &[
                    ("m_SpellsPerDay", Node::BpRef(Guid(TABLE))),
                    ("m_SpellList", Node::BpRef(Guid(LIST))),
                    ("CastingAttribute", Node::Enum(i64::from(StatType::Intelligence.0))),
                ],
            ),
        );
        crate::stats::ops::set_base_value(&mut w, u, StatType::Intelligence, 14);
        (w, u)
    }

    fn slots(w: &World, u: EntityIdx, level: usize) -> Vec<SlotKey> {
        book(w, u, Guid(BOOK)).memorized[level].iter().map(Slot::key).collect()
    }

    /// `BlueprintSpellsTable.GetCount` falls back to the last entry above the table.
    #[test]
    fn spell_tables_count_like_getcount() {
        let t = vec![vec![], vec![3, 1], vec![4, 2, 1]];
        assert_eq!(table_count(&t, 1, 1), 1);
        assert_eq!(table_count(&t, 2, 2), 1);
        assert_eq!(table_count(&t, 9, 1), 2, "past the end: the last entry");
        assert_eq!(table_count(&t, 1, 2), -1, "no such spell level");
        assert_eq!(table_count(&t, -1, 0), -1);
    }

    /// Levels raise the maximum spell level; the first one learns the cantrips; the slots follow
    /// the spells per day, which need the casting stat; shrinking hides slots and growing again
    /// restores the same ones.
    #[test]
    fn levels_learn_cantrips_and_slots_follow_the_stat() {
        let (mut w, u) = world();
        let sb = Guid(BOOK);
        demand_spellbook(&mut w, u, sb);
        assert_eq!(max_spell_level(&w, u, sb), -1);
        add_base_level(&mut w, u, sb);
        assert_eq!(max_spell_level(&w, u, sb), 1);
        let known: Vec<Guid> = book(&w, u, sb).known[0].iter().map(|s| s.blueprint).collect();
        assert_eq!(known, vec![Guid(SPELLS[0]), Guid(SPELLS[1])]);
        assert_eq!(book(&w, u, sb).known_levels.get(&Guid(SPELLS[0])), Some(&vec![0]));
        // Intelligence 14: one slot from the table, +1 for a bonus of +2 at level 1.
        assert_eq!(spells_per_day(&mut w, u, sb, 1), 2);
        update_all_slots_size(&mut w, u, sb, true);
        assert_eq!(slots(&w, u, 1), vec![(COMMON, 0), (COMMON, 1)]);
        // Intelligence 10 can't cast level 1 spells: the slots hide on a shrink, not on growth.
        crate::stats::ops::set_base_value(&mut w, u, StatType::Intelligence, 10);
        update_all_slots_size(&mut w, u, sb, false);
        assert_eq!(slots(&w, u, 1).len(), 2, "no shrink without allowShrink");
        update_all_slots_size(&mut w, u, sb, true);
        assert!(slots(&w, u, 1).is_empty());
        assert_eq!(book(&w, u, sb).hidden_common.get(&1).map(CsDict::len), Some(2));
        crate::stats::ops::set_base_value(&mut w, u, StatType::Intelligence, 14);
        update_all_slots_size(&mut w, u, sb, false);
        assert_eq!(slots(&w, u, 1), vec![(COMMON, 0), (COMMON, 1)]);
        assert_eq!(book(&w, u, sb).hidden_common.get(&1).map(CsDict::len), Some(0));
    }

    /// A `RemoveMemorizedSpells` entry that is the same save object as a known spell gets the
    /// level `PostLoad` gives that object, so a later memorization matches and removes it.
    #[test]
    fn a_removed_spell_shared_with_a_known_one_takes_its_level_at_load() {
        let (mut w, u) = world();
        let sb = Guid(BOOK);
        demand_spellbook(&mut w, u, sb);
        let mut known = Spell::new(&mut w, Guid(SPELLS[2]), 1);
        known.saved_object = Some(7);
        known.level_in_spellbook = None;
        let mut removed = known.clone();
        let mut alone = Spell::new(&mut w, Guid(SPELLS[3]), 1);
        alone.saved_object = Some(8);
        alone.level_in_spellbook = None;
        removed.level_in_spellbook = None;
        let b = book_mut(&mut w, u, sb);
        b.known[1].push(known);
        b.remove_memorized = vec![removed, alone];
        post_load_unit(&mut w, u);
        let r = &book(&w, u, sb).remove_memorized;
        assert_eq!(r[0].level_in_spellbook, Some(1));
        assert_eq!(r[1].level_in_spellbook, None, "no alias, no level");
    }

    /// `AddKnown` records the level (sorted), adds a copy for a known spell again, and
    /// `Memorize` takes the first free common slot; a rest makes it available.
    #[test]
    fn known_spells_memorize_and_rest() {
        let (mut w, u) = world();
        let sb = Guid(BOOK);
        demand_spellbook(&mut w, u, sb);
        add_base_level(&mut w, u, sb);
        update_all_slots_size(&mut w, u, sb, true);
        add_known(&mut w, u, sb, 1, Guid(SPELLS[2]), true);
        add_known(&mut w, u, sb, 1, Guid(SPELLS[2]), true);
        assert_eq!(book(&w, u, sb).known[1].len(), 2, "a second entry for the same spell");
        assert_eq!(book(&w, u, sb).min_spell_level(Guid(SPELLS[2])), 1);
        let data = Spell::new(&mut w, Guid(SPELLS[2]), 1);
        assert!(memorize(&mut w, u, sb, &data));
        let s = book(&w, u, sb).slot(1, (COMMON, 0)).expect("slot");
        assert_eq!((s.spell.is_some(), s.available), (true, false));
        assert_eq!(s.linked, Some(vec![(COMMON, 0)]));
        rest(&mut w, u, sb);
        assert!(book(&w, u, sb).slot(1, (COMMON, 0)).expect("slot").available);
        // A spell nobody knows can't be memorized.
        let stranger = Spell::new(&mut w, Guid(SPELLS[3]), 1);
        assert!(!memorize(&mut w, u, sb, &stranger));
    }

    /// `SpellSlot.SpellShell` keeps a spent spell in `RemoveMemorizedSpells` until the same spell
    /// comes back, which makes the slot available again.
    #[test]
    fn replacing_an_available_spell_remembers_it() {
        let (mut w, u) = world();
        let sb = Guid(BOOK);
        demand_spellbook(&mut w, u, sb);
        add_base_level(&mut w, u, sb);
        update_all_slots_size(&mut w, u, sb, true);
        let a = Spell::new(&mut w, Guid(SPELLS[2]), 1);
        let b = Spell::new(&mut w, Guid(SPELLS[3]), 1);
        let bk = book_mut(&mut w, u, sb);
        bk.set_spell_shell(1, (COMMON, 0), Some(a.clone()));
        bk.slot_mut(1, (COMMON, 0)).expect("slot").available = true;
        bk.set_spell_shell(1, (COMMON, 0), Some(b));
        assert_eq!(bk.remove_memorized.len(), 1);
        assert!(!bk.slot(1, (COMMON, 0)).expect("slot").available);
        bk.set_spell_shell(1, (COMMON, 0), Some(a));
        assert!(bk.remove_memorized.is_empty());
        assert!(bk.slot(1, (COMMON, 0)).expect("slot").available);
    }

    /// The writer's identity stack: a slot linked to itself shows a cycle; a linked pair shows
    /// the other slot in full, with cycles inside.
    #[test]
    fn persisted_links_show_cycles_like_the_oracle_writer() {
        let mut b = Spellbook::empty(Guid(BOOK), 1, 3);
        let mut s0 = Slot::new(1, COMMON, 0);
        let mut s1 = Slot::new(1, COMMON, 1);
        s0.linked = Some(vec![(COMMON, 0), (COMMON, 1)]);
        s1.linked = Some(vec![(COMMON, 0), (COMMON, 1)]);
        b.memorized[1] = vec![s0, s1];
        let j = persisted(&b);
        let first = &j["m_MemorizedSpells"][1][0]["LinkedSlots"];
        assert_eq!(first[0], json!({"$cycle": "SpellSlot"}));
        assert_eq!(first[1]["Index"], 1);
        assert_eq!(
            first[1]["LinkedSlots"],
            json!([{"$cycle": "SpellSlot"}, {"$cycle": "SpellSlot"}])
        );
        assert_eq!(j["m_HiddenCommonSlots"], json!([]));
    }

    /// `UnitAbilityResource`: maximum from class levels and the stat bonus, retain counting.
    #[test]
    fn resources_count_retains_and_restore_to_the_maximum() {
        let (mut w, u) = crate::stats::tests::world_with_unit();
        w.registry = Rc::new(crate::components::registry());
        let class = 0x31u128;
        insert(
            &w,
            class,
            "Class",
            obj("Kingmaker.Blueprints.Classes.BlueprintCharacterClass", &[]),
        );
        let amount = obj(
            "Kingmaker.Blueprints.BlueprintAbilityResource.Amount",
            &[
                ("BaseValue", Node::Int(2)),
                ("IncreasedByLevel", Node::Bool(true)),
                ("m_Class", Node::Seq(vec![Node::BpRef(Guid(class))])),
                ("LevelIncrease", Node::Int(1)),
                ("IncreasedByStat", Node::Bool(true)),
                ("ResourceBonusStat", Node::Enum(i64::from(StatType::Wisdom.0))),
            ],
        );
        let res = 0x32u128;
        insert(
            &w,
            res,
            "Resource",
            obj(
                "Kingmaker.Blueprints.BlueprintAbilityResource",
                &[("m_MaxAmount", Node::Object(Box::new(amount)))],
            ),
        );
        crate::stats::ops::set_base_value(&mut w, u, StatType::Wisdom, 14);
        w.unit_mut(u).expect("unit").progression.classes.push(crate::unit::ClassData {
            class: Guid(class),
            level: 3,
            archetypes: Vec::new(),
            spellbook: None,
        });
        assert_eq!(max_amount_of(&mut w, u, Guid(res)), 2 + 3 + 2);
        crate::resources::add(&mut w, u, Guid(res), true);
        crate::resources::add(&mut w, u, Guid(res), false);
        let r = w.unit(u).and_then(|x| x.resources.get(&Guid(res))).expect("resource").clone();
        assert_eq!((r.amount, r.retain_count), (7, 2));
        crate::resources::remove(&mut w, u, Guid(res));
        assert!(w.unit(u).is_some_and(|x| x.resources.contains_key(&Guid(res))));
        crate::resources::remove(&mut w, u, Guid(res));
        assert!(w.unit(u).is_some_and(|x| x.resources.is_empty()));
    }

    fn max_amount_of(w: &mut World, u: EntityIdx, g: Guid) -> i32 {
        crate::resources::max_amount(w, u, g)
    }
}
