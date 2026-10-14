//! A unit's spellbooks and ability resources, read from the save (`Descriptor.m_Spellbooks`
//! and `Resources.PersistantResources`).

use std::collections::HashMap;

use wr_content::Guid;
use wr_save::value::{Entry, Value};

use super::graph::{O, guid_of, identity, int_of};
use crate::csharp::CsDict;
use crate::resources::AbilityResource;
use crate::spellbook::{LEVELS, Slot, SlotKey, Spell, Spellbook};
use crate::world::World;

/// The entries of a dictionary value in document order.
fn entries(v: &Value) -> Vec<(&Value, &Value)> {
    match v {
        Value::Dict(d) => d
            .entries
            .iter()
            .filter_map(|e| match e {
                Entry::Pair(k, v) | Entry::Laid(k, v, _) => Some((k, v)),
                Entry::Raw(..) => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A member holding an array of lists of objects.
fn nested<'a>(o: O<'a>, name: &str) -> Vec<Vec<O<'a>>> {
    let mut out: Vec<Vec<O<'a>>> = match o.get(name) {
        Some(Value::Seq(outer)) => outer
            .iter()
            .map(|x| match x {
                Value::Seq(inner) => inner.iter().filter_map(|y| o.wrap(y)).collect(),
                _ => Vec::new(),
            })
            .collect(),
        _ => Vec::new(),
    };
    out.resize(LEVELS, Vec::new());
    out
}

fn ints(o: O<'_>, name: &str) -> Vec<i64> {
    match o.get(name) {
        Some(Value::Seq(v)) => v.iter().filter_map(int_of).collect(),
        _ => Vec::new(),
    }
}

fn spell(w: &mut World, o: O<'_>) -> Option<Spell> {
    let blueprint = o.guid("Blueprint")?;
    for m in [
        "m_ConvertedFrom",
        "OverrideRequiredResource",
        "OverridenResourceLogic",
        "MetamagicData",
        "MagicHackData",
    ] {
        if o.get(m).is_some_and(|v| !matches!(v, Value::Null)) {
            w.coverage.unsupported(format!("Spellbook: AbilityData.{m} (not modelled)"));
        }
    }
    let unique_id = o.str("UniqueId").unwrap_or_default().to_owned();
    w.saved_spell_ids.insert(unique_id.clone());
    let flag = o
        .obj("HasActionBarSlot")
        .map(|f| f.i32_or("m_Count", 0))
        .or_else(|| o.int("HasActionBarSlot").and_then(|v| i32::try_from(v).ok()))
        .unwrap_or(0);
    Some(Spell {
        blueprint,
        unique_id,
        extra_spell_slot_cost: o.i32_or("ExtraSpellSlotCost", 0),
        save_spellbook_slot: o.bool_or("SaveSpellbookSlot", false),
        force_target_owner: o.bool_or("ForceTargetOwner", false),
        decoration_color: o.i32_or("DecorationColorNumber", -1),
        decoration_border: o.i32_or("DecorationBorderNumber", -1),
        is_mystic_theurge_combined: o.bool_or("IsMysticTheurgeCombinedSpell", false),
        was_substituted: o.bool_or("WasSubstituted", false),
        is_temporary: o.bool_or("IsTemporary", false),
        copied_from_scroll: o.bool_or("CopiedFromScroll", false),
        is_from_mythic_spell_list: o.bool_or("IsFromMythicSpellList", false),
        has_action_bar_slot: flag,
        temporarily_disabled: o.bool_or("TemporarilyDisabled", false),
        level_in_spellbook: None,
        saved_object: Some(identity(o.obj)),
    })
}

/// One spell level of a hidden-slots dictionary: the level and its (index, slot position).
type HiddenGroup = (i32, Vec<(i32, usize)>);

/// A slot and, by object identity, the slots its `LinkedSlots` name.
struct RawSlot<'a> {
    obj: O<'a>,
    slot: Slot,
}

fn raw_slot<'a>(w: &mut World, o: O<'a>) -> RawSlot<'a> {
    let mut slot = Slot::new(o.i32_or("SpellLevel", 0), o.i32_or("Type", 0), o.i32_or("Index", 0));
    slot.spell = o.obj("Spell").and_then(|s| spell(w, s));
    slot.available = o.bool_or("Available", false);
    slot.is_opposition = o.bool_or("IsOpposition", false);
    RawSlot { obj: o, slot }
}

fn spellbook(w: &mut World, blueprint: Guid, o: O<'_>) -> Spellbook {
    let mut b = Spellbook::loaded(w, blueprint);
    b.base_level = o.i32_or("m_BaseLevelInternal", 0);
    b.mythic_level = o.i32_or("m_MythicLevelInternal", 0);
    b.ty = o.i32_or("m_Type", 0);
    for (name, target) in [("m_KnownSpells", 0), ("m_SpecialSpells", 1), ("m_CustomSpells", 2)] {
        for (lv, list) in nested(o, name).into_iter().enumerate().take(LEVELS) {
            let spells: Vec<Spell> = list.into_iter().filter_map(|s| spell(w, s)).collect();
            match target {
                0 => b.known[lv] = spells,
                1 => b.special[lv] = spells,
                _ => b.custom[lv] = spells,
            }
        }
    }
    b.special_lists = match o.get("m_SpecialLists") {
        Some(Value::Seq(v)) => v.iter().filter_map(guid_of).collect(),
        _ => Vec::new(),
    };
    // The slots: memorized lists, then the hidden dictionaries.
    let mut raw: Vec<(usize, RawSlot<'_>)> = Vec::new();
    let mut shape: Vec<Vec<usize>> = vec![Vec::new(); LEVELS];
    for (lv, list) in nested(o, "m_MemorizedSpells").into_iter().enumerate().take(LEVELS) {
        for s in list {
            shape[lv].push(raw.len());
            raw.push((lv, raw_slot(w, s)));
        }
    }
    let mut hidden: [Vec<HiddenGroup>; 2] = [Vec::new(), Vec::new()];
    for (i, name) in ["m_HiddenCommonSlots", "m_HiddenFavoriteSlots"].into_iter().enumerate() {
        for (k, v) in o.dict(name) {
            let Some(level) = int_of(k).and_then(|l| i32::try_from(l).ok()) else { continue };
            let mut inner = Vec::new();
            for (ik, iv) in entries(v) {
                let (Some(index), Some(so)) =
                    (int_of(ik).and_then(|x| i32::try_from(x).ok()), o.wrap(iv))
                else {
                    continue;
                };
                inner.push((index, raw.len()));
                raw.push((usize::try_from(level).unwrap_or(0).min(LEVELS - 1), raw_slot(w, so)));
            }
            hidden[i].push((level, inner));
        }
    }
    let ids: HashMap<usize, (usize, SlotKey)> =
        raw.iter().map(|(lv, r)| (identity(r.obj.obj), (*lv, (r.slot.ty, r.slot.index)))).collect();
    for (_, r) in &mut raw {
        if let Some(Value::Seq(items)) = r.obj.get("LinkedSlots") {
            let mut keys = Vec::new();
            for it in items.iter() {
                match r.obj.wrap(it).and_then(|x| ids.get(&identity(x.obj))) {
                    Some((_, k)) => keys.push(*k),
                    None => w
                        .coverage
                        .unsupported("Spellbook: SpellSlot.LinkedSlots entry not resolved"),
                }
            }
            r.slot.linked = Some(keys);
        }
    }
    let mut slots: Vec<Option<Slot>> = raw.into_iter().map(|(_, r)| Some(r.slot)).collect();
    for (lv, idxs) in shape.into_iter().enumerate() {
        b.memorized[lv] = idxs.into_iter().filter_map(|i| slots[i].take()).collect();
    }
    for (i, groups) in hidden.into_iter().enumerate() {
        let dict = if i == 0 { &mut b.hidden_common } else { &mut b.hidden_favorite };
        for (level, inner) in groups {
            let mut d = CsDict::new();
            for (index, ri) in inner {
                if let Some(s) = slots[ri].take() {
                    d.insert(index, s);
                }
            }
            dict.insert(level, d);
        }
    }
    b.remove_memorized =
        o.objs("RemoveMemorizedSpells").into_iter().filter_map(|s| spell(w, s)).collect();
    for (src, dst) in [
        (ints(o, "m_SpontaneousSlots"), &mut b.spontaneous),
        (ints(o, "BonusSpellSlots"), &mut b.bonus_spell_slots),
    ] {
        for (i, v) in src.into_iter().enumerate().take(LEVELS) {
            dst[i] = i32::try_from(v).unwrap_or(0);
        }
    }
    b.opposition_descriptors = o.int("OppositionDescriptors").unwrap_or(0);
    b.opposition_schools = ints(o, "OppositionSchools");
    b.ex_opposition_schools = ints(o, "ExOppositionSchools");
    b.ignore_opposition_schools = ints(o, "IgnoreOppositionSchools");
    b
}

/// `Descriptor.m_Spellbooks`.
pub(super) fn spellbooks(w: &mut World, d: O<'_>) -> CsDict<Guid, Spellbook> {
    let mut out = CsDict::new();
    for (k, v) in d.dict("m_Spellbooks") {
        let (Some(bp), Some(so)) = (guid_of(k), d.wrap(v)) else { continue };
        let b = spellbook(w, bp, so);
        out.insert(bp, b);
    }
    out
}

/// `Resources.PersistantResources`.
pub(super) fn resources(d: O<'_>) -> CsDict<Guid, AbilityResource> {
    let mut out = CsDict::new();
    for r in d.obj("Resources").map(|r| r.objs("PersistantResources")).unwrap_or_default() {
        if let Some(blueprint) = r.guid("Blueprint") {
            out.insert(
                blueprint,
                AbilityResource {
                    blueprint,
                    amount: r.i32_or("Amount", 0),
                    retain_count: r.i32_or("RetainCount", 0),
                },
            );
        }
    }
    out
}

/// `UnitPartActivatableAbility.PersistentGroupsSizeIncreases` of the entity's part.
pub(super) fn group_size_increases(entity: O<'_>) -> Vec<i32> {
    entity
        .obj("Parts")
        .map(|p| p.objs("m_Parts"))
        .unwrap_or_default()
        .into_iter()
        .find(|p| p.ty() == "UnitPartActivatableAbility")
        .map(|p| {
            ints(p, "PersistentGroupsSizeIncreases")
                .into_iter()
                .map(|v| i32::try_from(v).unwrap_or(0))
                .collect()
        })
        .unwrap_or_default()
}

/// `UnitPartExtraSpellsPerDay.BonusSpells` of the entity's part.
pub(super) fn extra_spells_per_day(entity: O<'_>) -> Option<Vec<i32>> {
    let part = entity
        .obj("Parts")
        .map(|p| p.objs("m_Parts"))
        .unwrap_or_default()
        .into_iter()
        .find(|p| p.ty() == "UnitPartExtraSpellsPerDay")?;
    Some(ints(part, "BonusSpells").into_iter().map(|v| i32::try_from(v).unwrap_or(0)).collect())
}
