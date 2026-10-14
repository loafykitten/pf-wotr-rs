//! `ItemEntityArmor.RecalculateStats` (run from its `OnTurnOn`) and the two rules it
//! triggers, `RuleCalculateArmorCheckPenalty` and `RuleCalculateArmorMaxDexBonusLimit`.
//! Sources (decompiled, not copied): `Kingmaker.Items/ItemEntityArmor.cs`,
//! `ItemEntityShield.cs`, `Kingmaker.RuleSystem.Rules/RuleCalculateArmor*.cs`,
//! `Kingmaker.Designers/GameHelper.cs` (`GetItemEnhancementBonus`).
//!
//! On load the armor's saved modifiers are removed and added again (so `CleanupModifiers`
//! dropping a shield component's modifiers, whose `Collection` isn't saved, is repaired
//! here, as in the game).

use crate::rulebook::{RuleBase, RuleEvent};
use crate::stats::ops;
use crate::stats::types::{ModifierDescriptor as D, StatType};
use crate::stats::value::{DexBonusLimiter, StatKind};
use crate::world::{EntityIdx, World};

/// `RuleCalculateArmorCheckPenalty`.
#[derive(Debug)]
pub struct RuleCalculateArmorCheckPenalty {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub armor: EntityIdx,
    pub armor_checks_penalty: i32,
    /// `m_Bonus`, raised by subscribers through `AddBonus`.
    pub bonus: i32,
    pub result: i32,
}

impl RuleEvent for RuleCalculateArmorCheckPenalty {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCalculateArmorCheckPenalty"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn on_trigger(&mut self, _w: &mut World) {
        self.result = crate::csharp::arith::add(self.armor_checks_penalty, self.bonus).min(0);
    }
    fn base(&self) -> &RuleBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut RuleBase {
        &mut self.base
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// `RuleCalculateArmorMaxDexBonusLimit`.
#[derive(Debug)]
pub struct RuleCalculateArmorMaxDexBonusLimit {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub armor: EntityIdx,
    pub has_limit: bool,
    pub max_dexterity_bonus: i32,
    pub bonus: i32,
    pub result: Option<i32>,
}

impl RuleEvent for RuleCalculateArmorMaxDexBonusLimit {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleCalculateArmorMaxDexBonusLimit"]
    }
    fn initiator(&self) -> EntityIdx {
        self.initiator
    }
    fn on_trigger(&mut self, _w: &mut World) {
        if self.has_limit {
            self.result = Some(crate::csharp::arith::add(self.max_dexterity_bonus, self.bonus));
        }
    }
    fn base(&self) -> &RuleBase {
        &self.base
    }
    fn base_mut(&mut self) -> &mut RuleBase {
        &mut self.base
    }
    fn as_any(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// The armor's enchantments' components of a type, in fact order.
fn enchantment_components<'a>(
    w: &'a World,
    e: EntityIdx,
    ty: &str,
) -> Vec<crate::content::View<'a>> {
    let mut out = Vec::new();
    for f in &w.entity(e).facts.list {
        if let Some(bp) = &w.fact(*f).bp {
            for o in bp.components_array() {
                if crate::content::is_a(o.ty, ty) {
                    out.push(crate::content::View(o));
                }
            }
        }
    }
    out
}

/// `GameHelper.GetItemEnhancementBonus(armor)`: the highest `ArmorEnhancementBonus`
/// among the enchantments (first component per enchantment), plus fake enhancement.
fn item_enhancement_bonus(w: &mut World, e: EntityIdx, wielder: EntityIdx) -> i32 {
    let mut best: Option<i32> = None;
    for f in w.entity(e).facts.list.clone() {
        let Some(bp) = w.fact(f).bp.clone() else { continue };
        let Some(c) = bp
            .components_array()
            .into_iter()
            .find(|o| crate::content::is_a(o.ty, "ArmorEnhancementBonus"))
        else {
            continue;
        };
        let v = crate::content::View(c).i32("EnhancementValue");
        if best.is_none_or(|b| v > b) {
            best = Some(v);
            if crate::components::state::has_mechanics_feature(
                w,
                wielder,
                &["AdvancedFakeEnhancement", "FakeEnhancement"],
            ) {
                w.coverage.unsupported("GetItemEnhancementBonus: fake enhancement");
            }
        }
    }
    best.unwrap_or(0)
}

/// `ItemEntityArmor.ArmorType()` of an armor item.
#[must_use]
pub fn item_armor_type(w: &World, e: EntityIdx) -> i64 {
    let group = w
        .item(e)
        .and_then(|i| i.blueprint)
        .and_then(|g| w.content.get(g))
        .and_then(|b| b.view().guid("m_Type"))
        .and_then(|g| w.content.get(g))
        .map_or(0, |t| t.view().int("m_ProficiencyGroup"));
    armor_type(w, e, group)
}

/// `ArmorType()`: mithral lowers the proficiency group one step.
fn armor_type(w: &World, e: EntityIdx, group: i64) -> i64 {
    let mithral = !enchantment_components(w, e, "MithralEnchantment").is_empty();
    match (mithral, group) {
        (true, 3) => 2,
        (true, 2) => 1,
        (_, g) => g,
    }
}

/// `ModifiableValueArmorClass.RemoveDexBonusLimiter(limiter)`.
fn remove_dex_bonus_limiter(w: &mut World, u: EntityIdx, id: u32) {
    let mut changed = false;
    if let StatKind::ArmorClass(ac) = &mut w.stats_mut(u).m(StatType::AC).kind
        && let Some(l) = &mut ac.limiters
        && !l.is_empty()
    {
        if let Some(p) = l.iter().position(|x| x.id == id) {
            l.remove(p);
        }
        changed = true;
    }
    if changed {
        ops::update_value(w, u, StatType::AC);
    }
}

/// `ModifiableValueArmorClass.AddDexBonusLimiter(value, source)`.
fn add_dex_bonus_limiter(w: &mut World, u: EntityIdx, value: i32, source: i32) -> u32 {
    let id = w.next_limiter_id();
    let existing = match &w.stats(u).v(StatType::AC).kind {
        StatKind::ArmorClass(ac) => {
            ac.limiters.as_ref().and_then(|l| l.iter().find(|x| x.source == source).map(|x| x.id))
        }
        _ => None,
    };
    if let Some(old) = existing {
        w.log(
            "error",
            "ModifiableValueArmorClass.AddDexBonusLimiter: limiter already exists, remove old one",
        );
        remove_dex_bonus_limiter(w, u, old);
    }
    if let StatKind::ArmorClass(ac) = &mut w.stats_mut(u).m(StatType::AC).kind {
        ac.limiters.get_or_insert_with(Vec::new).push(DexBonusLimiter { value, source, id });
    }
    ops::update_value(w, u, StatType::AC);
    id
}

fn limiter_applied(w: &World, u: EntityIdx, id: u32) -> bool {
    matches!(&w.stats(u).v(StatType::AC).kind, StatKind::ArmorClass(ac)
        if ac.limiters.as_ref().is_some_and(|l| l.iter().any(|x| x.id == id)))
}

/// `ItemEntityArmor.RecalculateStats()`.
pub fn recalculate_stats(w: &mut World, e: EntityIdx) {
    let Some(item) = w.item(e).cloned() else { return };
    let Some(u) = item.wielder.filter(|u| w.unit(*u).is_some()) else { return };
    for m in &item.modifiers {
        if w.mods[m.i()].applied_to.is_some() {
            ops::remove(w, *m);
        }
    }
    let shield = item.shield.is_some();
    let (desc, edesc) =
        if shield { (D::Shield, D::ShieldEnhancement) } else { (D::Armor, D::ArmorEnhancement) };
    let Some(bp) = item.blueprint.and_then(|g| w.content.get(g)) else { return };
    let Some(ty) = bp.view().guid("m_Type").and_then(|g| w.content.get(g)) else {
        w.coverage.unsupported("ItemEntityArmor.RecalculateStats: armor type missing");
        return;
    };
    let tv = ty.view();
    let (armor_bonus, acp, has_limit, max_dex, group) = (
        tv.i32("m_ArmorBonus"),
        tv.i32("m_ArmorChecksPenalty"),
        tv.bool("m_HasDexterityBonusLimit"),
        tv.i32("m_MaxDexterityBonus"),
        tv.int("m_ProficiencyGroup"),
    );
    let mut mods = Vec::new();
    let enh = item_enhancement_bonus(w, e, u);
    mods.push(ops::add_item_modifier(w, u, StatType::AC, armor_bonus, None, Some(e), desc));
    mods.push(ops::add_item_modifier(w, u, StatType::AC, enh, None, Some(e), edesc));
    let mut rule = RuleCalculateArmorCheckPenalty {
        base: RuleBase::default(),
        initiator: u,
        armor: e,
        armor_checks_penalty: acp,
        bonus: 0,
        result: 0,
    };
    crate::rules::trigger_rule(w, &mut rule);
    if rule.result < 0 {
        for s in [
            StatType::SkillMobility,
            StatType::SkillAthletics,
            StatType::SkillStealth,
            StatType::SkillThievery,
        ] {
            mods.push(ops::add_item_modifier(w, u, s, rule.result, None, Some(e), desc));
        }
    }
    // `RecalculateMaxDexBonus`.
    if let Some(id) = item.dex_limiter
        && limiter_applied(w, u, id)
    {
        remove_dex_bonus_limiter(w, u, id);
    }
    let mut limiter = None;
    let max_dex_value = if has_limit { max_dex } else { i32::MAX };
    let mut r = RuleCalculateArmorMaxDexBonusLimit {
        base: RuleBase::default(),
        initiator: u,
        armor: e,
        has_limit,
        max_dexterity_bonus: max_dex_value,
        bonus: 0,
        result: None,
    };
    crate::rules::trigger_rule(w, &mut r);
    if let Some(v) = r.result {
        // `SourceType`: Armor 1, Shield 2.
        limiter = Some(add_dex_bonus_limiter(w, u, v, if shield { 2 } else { 1 }));
    }
    let at = armor_type(w, e, group);
    let medium_immune = crate::components::state::has_mechanics_feature(
        w,
        u,
        &["ImmunityToMediumArmorSpeedPenalty"],
    );
    let immune =
        crate::components::state::has_mechanics_feature(w, u, &["ImmunityToArmorSpeedPenalty"]);
    if ((at == 2 && !medium_immune) || at == 3) && !immune {
        let racial = match w.stats(u).v(StatType::Speed).kind {
            StatKind::Speed { racial } => racial,
            _ => 0,
        };
        let v = if racial < 30 { -5 } else { -10 };
        mods.push(ops::add_item_modifier(w, u, StatType::Speed, v, None, Some(e), desc));
    }
    if let Some(it) = w.item_mut(e) {
        it.modifiers = mods;
        it.dex_limiter = limiter;
    }
}

/// `ItemEntityArmor.OnWillUnequip()`: the modifiers it added and its dex limiter go.
pub fn on_will_unequip(w: &mut World, e: EntityIdx) {
    let Some(item) = w.item(e).cloned() else { return };
    for m in &item.modifiers {
        if w.mods[m.i()].applied_to.is_some() {
            ops::remove(w, *m);
        }
    }
    if let (Some(id), Some(u)) = (item.dex_limiter, item.wielder)
        && limiter_applied(w, u, id)
    {
        remove_dex_bonus_limiter(w, u, id);
    }
    if let Some(it) = w.item_mut(e) {
        it.modifiers = Vec::new();
        it.dex_limiter = None;
    }
}
