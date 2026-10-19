//! `ModifiableValue`'s algorithms and its subclasses' overrides, ported from
//! `Kingmaker.EntitySystem.Stats/*.cs`. Each function names the C# member it mirrors.
//! Calls re-enter exactly as in the C#: `AddModifier` calls `UpdateValue`, which updates
//! dependents and fires `OnChanged`, whose handlers can add modifiers to other stats.

use super::modifier::{Modifier, is_duplicate, mod_equals};
use super::types::{BonusMod, HitPointsBaseStat, ModifierDescriptor as D, StackMode, StatType};
use super::value::{ChangedHandler, ModList, StatKind};
use crate::csharp::arith;
use crate::csharp::sort::introsort;
use crate::world::{CompRef, EntityIdx, FactIdx, ModIdx, World};

/// The filters `ApplyModifiersFiltered` is called with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Filter {
    All,
    /// `FilterAllowedForNoMorale`.
    NoMorale,
    /// `FilterAllowedForNoMoraleNoInsight`.
    NoMoraleNoInsight,
    /// `FilterIsRacialOrInherent`.
    RacialOrInherent,
    /// `FilterIsPermanent`.
    Permanent,
    /// `ModifiableValueArmorClass.FilterAllowedForFlatFooted`.
    AcFlatFooted,
    /// `FilterAllowedForTouch`.
    AcTouch,
    /// `FilterAllowedForFlatFootedTouch`.
    AcFlatFootedTouch,
    /// `FilterIsArmor`.
    AcArmor,
    /// `FilterIsShield`.
    AcShield,
    /// `FilterIsDodge`.
    AcDodge,
    /// `GetDescriptorBonus(d)`.
    Descriptor(D),
    /// `FilterIsNoTempBuff`: not a modifier of a timed buff (`Modifier.IsTempBuff`).
    NoTempBuff,
}

fn ac_is_armor(d: D) -> bool {
    matches!(
        d,
        D::Armor
            | D::ArmorEnhancement
            | D::ArmorFocus
            | D::NaturalArmor
            | D::NaturalArmorEnhancement
            | D::NaturalArmorForm
    )
}

fn ac_is_shield(d: D) -> bool {
    matches!(d, D::Shield | D::ShieldEnhancement | D::Focus)
}

fn ac_is_dodge(d: D) -> bool {
    matches!(d, D::Dodge | D::DexterityBonus)
}

fn passes(w: &World, f: Filter, m: &Modifier) -> bool {
    let no_morale = m.value < 0 || m.desc != D::Morale;
    let touch = m.value < 0 || (!ac_is_armor(m.desc) && !ac_is_shield(m.desc));
    let flat = m.value < 0 || !ac_is_dodge(m.desc);
    match f {
        Filter::All => true,
        Filter::NoMorale => no_morale,
        Filter::NoMoraleNoInsight => no_morale && m.desc != D::Insight && m.desc != D::Competence,
        Filter::RacialOrInherent => m.desc == D::Racial || m.desc == D::Inherent,
        Filter::Permanent => m.is_permanent(),
        Filter::AcFlatFooted => flat,
        Filter::AcTouch => touch,
        Filter::AcFlatFootedTouch => touch && flat,
        Filter::AcArmor => ac_is_armor(m.desc),
        Filter::AcShield => ac_is_shield(m.desc),
        Filter::AcDodge => ac_is_dodge(m.desc),
        Filter::Descriptor(d) => m.desc == d,
        Filter::NoTempBuff => {
            // `IsTempBuff`: not racial, from a buff that isn't permanent.
            let temp = m.desc != D::Racial
                && m.source.is_some_and(|f| {
                    w.fact(f).kind == crate::world::FactKind::Buff
                        && w.fact(f).buff.as_ref().is_some_and(|b| !b.is_permanent())
                });
            !temp
        }
    }
}

fn stat(w: &World, u: EntityIdx, t: StatType) -> &super::value::StatValue {
    w.stats(u).v(t)
}

fn stat_mut(w: &mut World, u: EntityIdx, t: StatType) -> &mut super::value::StatValue {
    w.stats_mut(u).m(t)
}

/// `ModifiableValueAttributeStat.Bonus`: `ModifiedValue / 2 - 5` (truncating division).
#[must_use]
pub fn attribute_bonus(w: &World, u: EntityIdx, t: StatType) -> i32 {
    let v = stat(w, u, t).modified_value;
    arith::sub(v / 2, 5)
}

/// `MinValueRaw`.
fn min_value_raw(w: &World, u: EntityIdx, t: StatType) -> i32 {
    match stat(w, u, t).kind {
        StatKind::Attribute { .. } => {
            let unit = w.unit(u).expect("unit");
            if unit.is_player_faction && w.settings.dead_companions_rise_after_combat {
                5
            } else {
                i32::MIN
            }
        }
        _ => i32::MIN,
    }
}

/// `ApplyModifiersFiltered(baseValue, filter)`.
#[must_use]
pub fn apply_modifiers_filtered(
    w: &World,
    u: EntityIdx,
    t: StatType,
    base_value: i32,
    filter: Filter,
) -> i32 {
    let s = stat(w, u, t);
    let is_attribute = matches!(s.kind, StatKind::Attribute { .. });
    let mut num = base_value;
    let mut armor = 0i32;
    let mut encumbrance = 0i32;
    for (_, list) in s.modifier_list.iter() {
        let (mut stack, mut best, mut worst) = (0i32, 0i32, 0i32);
        for &mi in &list.items {
            let m = &w.mods[mi.i()];
            if !passes(w, filter, m) {
                continue;
            }
            if is_attribute && m.desc == D::Helpless {
                return 0;
            }
            if m.value < 0 {
                if m.desc == D::Armor {
                    armor = armor.min(m.value);
                    continue;
                }
                if m.desc == D::Encumbrance {
                    encumbrance = encumbrance.min(m.value);
                    continue;
                }
            }
            let stacks = m.stacks();
            if stacks {
                stack = arith::add(stack, m.value);
            }
            if !stacks && m.value > best {
                best = m.value;
            }
            if !stacks && m.value < worst {
                worst = m.value;
            }
        }
        num = arith::add(num, arith::add(arith::add(stack, best), worst));
    }
    num = arith::add(num, armor.min(encumbrance));
    num.max(min_value_raw(w, u, t))
}

/// `ModifiableValueAttributeStat.CalculatePermanentValueWithoutTempBuffs()`.
#[must_use]
pub fn permanent_value_without_temp_buffs(w: &World, u: EntityIdx, t: StatType) -> i32 {
    let base = calculate_base_value(w, u, t, stat(w, u, t).base_value);
    apply_modifiers_filtered(w, u, t, base, Filter::NoTempBuff)
}

/// `CalculateBaseValue(baseValue)` (virtual).
fn calculate_base_value(w: &World, u: EntityIdx, t: StatType, base: i32) -> i32 {
    match &stat(w, u, t).kind {
        StatKind::Attribute { disabled, .. } => {
            if *disabled > 0 {
                10
            } else {
                base
            }
        }
        StatKind::Skill { current_base, class_skill, background_source, .. } => {
            let class = *class_skill > 0 || background_source.is_some();
            let bonus = if class && base > 0 { 3 } else { 0 };
            arith::add(arith::add(base, bonus), attribute_bonus(w, u, *current_base))
        }
        StatKind::SavingThrow { base_stat } => arith::add(base, attribute_bonus(w, u, *base_stat)),
        StatKind::Dependant { base_stat } => {
            arith::add(base, stat(w, u, *base_stat).modified_value)
        }
        StatKind::TemporaryHitPoints => 0,
        _ => base,
    }
}

/// `CalculatePermanentValue()` (virtual).
fn calculate_permanent_value(w: &World, u: EntityIdx, t: StatType) -> i32 {
    let s = stat(w, u, t);
    let base = calculate_base_value(w, u, t, s.base_value);
    match s.kind {
        StatKind::Attribute { .. } | StatKind::HitPoints { .. } => {
            apply_modifiers_filtered(w, u, t, base, Filter::Permanent)
        }
        _ if t == StatType::BaseAttackBonus => apply_modifiers_filtered(w, u, t, base, Filter::All),
        _ => apply_modifiers_filtered(w, u, t, base, Filter::RacialOrInherent),
    }
}

/// `RecalculateMoraleInsight()` (virtual; saving throws add the attribute bonus).
fn recalculate_morale_insight(w: &mut World, u: EntityIdx, t: StatType) {
    let base = stat(w, u, t).base_value;
    let mut a = apply_modifiers_filtered(w, u, t, base, Filter::NoMorale);
    let mut b = apply_modifiers_filtered(w, u, t, base, Filter::NoMoraleNoInsight);
    if let StatKind::SavingThrow { base_stat } = stat(w, u, t).kind {
        let bonus = attribute_bonus(w, u, base_stat);
        a = arith::add(a, bonus);
        b = arith::add(b, bonus);
    }
    let s = stat_mut(w, u, t);
    s.no_morale = s.modified_value.min(a);
    s.no_morale_no_insight = s.modified_value.min(b);
}

/// `OnUpdate()` (virtual: AC's derived values, speed's racial value).
fn on_update(w: &mut World, u: EntityIdx, t: StatType) {
    match stat(w, u, t).kind {
        StatKind::ArmorClass(_) => {
            let base = stat(w, u, t).base_value;
            let ff = apply_modifiers_filtered(w, u, t, base, Filter::AcFlatFooted);
            let to = apply_modifiers_filtered(w, u, t, base, Filter::AcTouch);
            let fft = apply_modifiers_filtered(w, u, t, base, Filter::AcFlatFootedTouch);
            let armor = apply_modifiers_filtered(w, u, t, 0, Filter::AcArmor);
            let shield = apply_modifiers_filtered(w, u, t, 0, Filter::AcShield);
            let dodge = apply_modifiers_filtered(w, u, t, 0, Filter::AcDodge);
            let s = stat_mut(w, u, t);
            let (mv, pv) = (s.modified_value, s.permanent_value);
            if let StatKind::ArmorClass(ac) = &mut s.kind {
                ac.flat_footed = mv.min(ff);
                ac.touch = mv.min(to);
                ac.flat_footed_touch = mv.min(fft);
                ac.flat_footed_permanent = pv.min(ff);
                ac.touch_permanent = pv.min(to);
                ac.flat_footed_touch_permanent = pv.min(fft);
                ac.armor_ac = armor;
                ac.shield_ac = shield;
                ac.dodge_ac = dodge;
            }
        }
        StatKind::Speed { .. } => {
            let base = calculate_base_value(w, u, t, stat(w, u, t).base_value);
            let r = apply_modifiers_filtered(w, u, t, base, Filter::RacialOrInherent);
            if let StatKind::Speed { racial } = &mut stat_mut(w, u, t).kind {
                *racial = r;
            }
        }
        _ => {}
    }
}

/// `UpdateValue()` (virtual: the AC first rebuilds its self bonus), and WrathOracle's hook on the
/// base method: only a stat's outermost `UpdateValue` (the stat is in `Hooks.Updating` from its
/// prefix to its postfix, dependents' updates included) reports a changed value, after the whole
/// update.
pub fn update_value(w: &mut World, u: EntityIdx, t: StatType) {
    if matches!(stat(w, u, t).kind, StatKind::ArmorClass(_)) {
        update_self_bonus(w, u);
    }
    if stat(w, u, t).in_update {
        update_value_base(w, u, t);
        return;
    }
    let before = stat(w, u, t).modified_value;
    stat_mut(w, u, t).in_update = true;
    update_value_base(w, u, t);
    stat_mut(w, u, t).in_update = false;
    if stat(w, u, t).modified_value != before {
        super::trace::value_changed(w, u, t, before);
    }
}

/// `ModifiableValue.UpdateValue()`'s body.
fn update_value_base(w: &mut World, u: EntityIdx, t: StatType) {
    if stat(w, u, t).updating_internal {
        return;
    }
    let modified_before = stat(w, u, t).modified_value;
    stat_mut(w, u, t).updating_internal = true;
    update_internal_modifiers(w, u, t);
    stat_mut(w, u, t).updating_internal = false;
    let base = calculate_base_value(w, u, t, stat(w, u, t).base_value);
    let raw = apply_modifiers_filtered(w, u, t, base, Filter::All);
    let min = stat(w, u, t).min_value();
    {
        let s = stat_mut(w, u, t);
        s.modified_value_raw = raw;
        s.modified_value = min.max(raw);
    }
    let permanent_before = stat(w, u, t).permanent_value;
    let p = calculate_permanent_value(w, u, t);
    if p != stat(w, u, t).permanent_value {
        stat_mut(w, u, t).permanent_value = p;
        on_permanent_value_updated(w, u, t);
    }
    recalculate_morale_insight(w, u, t);
    on_update(w, u, t);
    if stat(w, u, t).updating_dependent_facts {
        return;
    }
    update_dependent_facts_and_components(w, u, t);
    let s = stat(w, u, t);
    if modified_before != s.modified_value || permanent_before != s.permanent_value {
        let handlers = s.on_changed.clone();
        on_value_changed(w, u, t, modified_before);
        for h in handlers {
            invoke_changed(w, u, h);
        }
    }
    let Some(deps) = stat(w, u, t).dependents.clone() else { return };
    for d in deps {
        update_value(w, u, d);
    }
}

/// `OnPermanentValueUpdated()`: attributes re-check stat prerequisites of features.
fn on_permanent_value_updated(w: &mut World, u: EntityIdx, t: StatType) {
    if matches!(stat(w, u, t).kind, StatKind::Attribute { .. }) {
        crate::unit::check_stat_prerequisites_and_update(w, u);
    }
}

/// `OnValueChanged(previous)`: hit points raise `IUnitHealthChangedHandler`.
fn on_value_changed(w: &mut World, u: EntityIdx, t: StatType, _previous: i32) {
    if matches!(stat(w, u, t).kind, StatKind::HitPoints { .. }) {
        crate::rulebook::bus::raise_global(w, "IUnitHealthChangedHandler", Some(u));
    }
}

fn invoke_changed(w: &mut World, u: EntityIdx, h: ChangedHandler) {
    match h {
        ChangedHandler::AcAttribute => ac_check_attribute(w, u),
        ChangedHandler::SkillAttribute(s) => skill_check_attribute(w, u, s),
        ChangedHandler::Spellbook(serial) => {
            crate::spellbook::on_casting_stat_changed(w, u, serial)
        }
        ChangedHandler::Helpless => crate::unit_state::helpless_changed(w, u),
        ChangedHandler::MaxDexBonus(fact, comp) => {
            crate::components::npc_ports::max_dex_stat_changed(
                w,
                crate::world::CompRef {
                    fact: crate::world::FactIdx(fact),
                    comp: usize::try_from(comp).unwrap_or(usize::MAX),
                },
            );
        }
    }
}

/// `UpdateDependentFactsAndComponents()`.
fn update_dependent_facts_and_components(w: &mut World, u: EntityIdx, t: StatType) {
    if w.entity(u).disposing || stat(w, u, t).updating_dependent_facts {
        return;
    }
    stat_mut(w, u, t).updating_dependent_facts = true;
    let facts = stat(w, u, t).dependent_facts.clone();
    for f in facts {
        if !w.fact(f).reapplying {
            crate::facts::lifecycle::reapply(w, f);
        }
    }
    let comps = stat(w, u, t).dependent_components.clone();
    for c in comps {
        if w.fact(c.fact).is_active {
            crate::facts::lifecycle::component_recalculate(w, c);
        }
    }
    stat_mut(w, u, t).updating_dependent_facts = false;
}

/// `UpdateInternalModifiers()` (virtual).
fn update_internal_modifiers(w: &mut World, u: EntityIdx, t: StatType) {
    match stat(w, u, t).kind {
        StatKind::ArmorClass(_) => ac_update_internal_modifiers(w, u),
        StatKind::Skill { .. } => skill_update_internal_modifiers(w, u, t),
        StatKind::Initiative { .. } => initiative_update_internal_modifiers(w, u, t),
        StatKind::HitPoints { .. } => hp_update_internal_modifiers(w, u, t),
        _ => {}
    }
}

/// `GetModifiersToAdd(descriptor)`: the descriptor's list, created (and so inserted into
/// the dictionary) if missing.
fn modifiers_to_add(w: &mut World, u: EntityIdx, t: StatType, d: D) -> &mut ModList {
    stat_mut(w, u, t).modifier_list.get_or_insert_with(d, ModList::default)
}

/// `AddModifier(Modifier)` (virtual: a disabled attribute ignores it).
pub fn add_modifier(w: &mut World, u: EntityIdx, t: StatType, m: ModIdx) -> ModIdx {
    if let StatKind::Attribute { disabled, .. } = stat(w, u, t).kind
        && disabled > 0
    {
        return m;
    }
    let d = w.mods[m.i()].desc;
    let existing = modifiers_to_add(w, u, t, d).items.clone();
    // `RemoveDuplicates`.
    let dups: Vec<ModIdx> = existing.iter().copied().filter(|&x| is_duplicate(w, m, x)).collect();
    if !dups.is_empty() {
        for x in &dups {
            // `modifiers.Remove(item)`: removes the first element equal to `item`.
            let list = modifiers_to_add(w, u, t, d).items.clone();
            if let Some(pos) = list.iter().position(|&y| mod_equals(w, *x, y)) {
                modifiers_to_add(w, u, t, d).items.remove(pos);
            }
        }
        w.log(
            "error",
            format!("ModifiableValue.AddModifier: remove duplicated modifier ({t:?}, {d:?})"),
        );
    }
    modifiers_to_add(w, u, t, d).push(m);
    sort_list(w, u, t, d);
    w.mods[m.i()].applied_to = Some((u, t));
    update_value(w, u, t);
    // `HandleModifierAdded`: `UnitPartNonStackBonuses` bookkeeping for the UI's
    // non-stacking warning; it changes no value (not modelled).
    super::trace::modifier(w, "stat_mod_add", u, t, m);
    m
}

/// `list.Sort(Modifier.ValueComparer)`.
fn sort_list(w: &mut World, u: EntityIdx, t: StatType, d: D) {
    let mut items = std::mem::take(&mut modifiers_to_add(w, u, t, d).items);
    let cap = modifiers_to_add(w, u, t, d).capacity;
    let mods = &w.mods;
    introsort(&mut items, cap, &mut |a: &ModIdx, b: &ModIdx| {
        if a == b { 0 } else { mods[a.i()].value.cmp(&mods[b.i()].value) as i32 }
    });
    modifiers_to_add(w, u, t, d).items = items;
}

/// `AddModifier(int value, ModifierDescriptor desc)`.
pub fn add_modifier_value(w: &mut World, u: EntityIdx, t: StatType, value: i32, d: D) -> ModIdx {
    let m = w.add_mod(Modifier::new(value, d));
    add_modifier(w, u, t, m)
}

/// `fact.MaybeContext?.ParentContext?.SourceItem`.
fn context_source_item(w: &World, f: FactIdx) -> Option<EntityIdx> {
    w.fact(f).context.as_ref().and_then(|c| c.parent_source_item(w))
}

/// `AddModifier(int value, EntityFact sourceFact, ModifierDescriptor desc)`.
pub fn add_modifier_from_fact(
    w: &mut World,
    u: EntityIdx,
    t: StatType,
    value: i32,
    fact: Option<FactIdx>,
    d: D,
) -> ModIdx {
    let item = fact.and_then(|f| context_source_item(w, f));
    let m = w.add_mod(Modifier {
        applied_to: None,
        desc: d,
        stack_mode: StackMode::Default,
        value,
        source: fact,
        source_component: None,
        item_source: item,
    });
    add_modifier(w, u, t, m)
}

/// `AddModifier(int value, EntityFactComponent source, ModifierDescriptor desc)`.
pub fn add_modifier_from_component(
    w: &mut World,
    u: EntityIdx,
    t: StatType,
    value: i32,
    c: CompRef,
    d: D,
) -> ModIdx {
    let item = context_source_item(w, c.fact);
    let name = w.component(c).name.clone();
    let m = w.add_mod(Modifier {
        applied_to: None,
        desc: d,
        stack_mode: StackMode::Default,
        value,
        source: Some(c.fact),
        source_component: Some(name),
        item_source: item,
    });
    add_modifier(w, u, t, m)
}

/// `AddModifierUnique(int value, EntityFactComponent source, ModifierDescriptor desc)`:
/// skips if the descriptor's list already holds a modifier from the same fact and
/// component name (how a restored modifier is recognised on load).
pub fn add_modifier_unique(w: &mut World, u: EntityIdx, t: StatType, value: i32, c: CompRef, d: D) {
    let name = w.component(c).name.clone();
    if let Some(list) = stat(w, u, t).modifier_list.get(&d) {
        for &mi in &list.items {
            let m = &w.mods[mi.i()];
            if m.source == Some(c.fact) && m.source_component.as_deref() == Some(name.as_str()) {
                return;
            }
        }
    }
    add_modifier_from_component(w, u, t, value, c, d);
}

/// `AddItemModifier(int value, EntityFactComponent source, ItemEntity itemSource, desc)`.
pub fn add_item_modifier(
    w: &mut World,
    u: EntityIdx,
    t: StatType,
    value: i32,
    c: Option<CompRef>,
    item: Option<EntityIdx>,
    d: D,
) -> ModIdx {
    let item_source = item.or_else(|| c.and_then(|c| context_source_item(w, c.fact)));
    let m = w.add_mod(Modifier {
        applied_to: None,
        desc: d,
        stack_mode: StackMode::Default,
        value,
        source: c.map(|c| c.fact),
        source_component: c.map(|c| w.component(c).name.clone()),
        item_source,
    });
    add_modifier(w, u, t, m)
}

/// `Modifier.Remove()`.
pub fn remove(w: &mut World, m: ModIdx) -> bool {
    let Some((u, t)) = w.mods[m.i()].applied_to else { return false };
    remove_modifier(w, u, t, m)
}

/// `RemoveModifier(Modifier)`: removes the first *equal* modifier from the descriptor's
/// list, clears the given modifier's `AppliedTo`, updates.
pub fn remove_modifier(w: &mut World, u: EntityIdx, t: StatType, m: ModIdx) -> bool {
    let d = w.mods[m.i()].desc;
    let Some(list) = stat(w, u, t).modifier_list.get(&d) else { return false };
    let items = list.items.clone();
    let Some(pos) = items.iter().position(|&x| mod_equals(w, m, x)) else { return false };
    if let Some(l) = stat_mut(w, u, t).modifier_list.get_mut(&d) {
        l.items.remove(pos);
    }
    prepare_for_removal(w, m);
    update_value(w, u, t);
    super::trace::modifier(w, "stat_mod_remove", u, t, m);
    true
}

/// `ModifiableValue.GetTemporaryModifiersScope(modifiers)`: opening the scope. Each rule
/// modifier (value, descriptor, source fact) becomes a stat modifier with the default
/// stack mode; one whose duplicate (`ContainsDuplicates`) is already in its descriptor's
/// list is skipped; the value updates once if any was added. Returns the scope's modifiers
/// (all of them, as `m_Modifiers`) for [`close_temporary_scope`].
pub fn open_temporary_scope(
    w: &mut World,
    u: EntityIdx,
    t: StatType,
    rule_mods: &[(i32, D, Option<FactIdx>)],
) -> Vec<ModIdx> {
    let mods: Vec<ModIdx> = rule_mods
        .iter()
        .map(|&(value, desc, source)| {
            w.add_mod(Modifier {
                applied_to: None,
                desc,
                stack_mode: StackMode::Default,
                value,
                source,
                source_component: None,
                item_source: None,
            })
        })
        .collect();
    if mods.is_empty() {
        return mods;
    }
    let mut added = 0;
    for &m in &mods {
        let d = w.mods[m.i()].desc;
        let existing = modifiers_to_add(w, u, t, d).items.clone();
        if existing.iter().any(|&x| is_duplicate(w, m, x)) {
            continue;
        }
        modifiers_to_add(w, u, t, d).push(m);
        sort_list(w, u, t, d);
        added += 1;
        w.mods[m.i()].applied_to = Some((u, t));
    }
    if added > 0 {
        update_value(w, u, t);
    }
    super::trace::scope(w, "stat_temp_scope_add", u, t, &mods);
    stat_mut(w, u, t).temporary_scopes += 1;
    mods
}

/// `TemporaryModifiersScope.Dispose`: `List.Remove` (value equality) of each scope
/// modifier from its descriptor's list, clearing `AppliedTo` of those removed, then one
/// update if any was.
pub fn close_temporary_scope(w: &mut World, u: EntityIdx, t: StatType, mods: &[ModIdx]) {
    if mods.is_empty() {
        return;
    }
    super::trace::scope(w, "stat_temp_scope_remove", u, t, mods);
    let mut removed = 0;
    for &m in mods {
        let d = w.mods[m.i()].desc;
        let Some(list) = stat(w, u, t).modifier_list.get(&d) else { continue };
        let items = list.items.clone();
        let Some(pos) = items.iter().position(|&x| mod_equals(w, m, x)) else { continue };
        if let Some(l) = stat_mut(w, u, t).modifier_list.get_mut(&d) {
            l.items.remove(pos);
        }
        removed += 1;
        w.mods[m.i()].applied_to = None;
    }
    if removed > 0 {
        update_value(w, u, t);
    }
    stat_mut(w, u, t).temporary_scopes -= 1;
}

/// `PrepareForRemoval`: the non-stacking UI bookkeeping (not modelled), then
/// `AppliedTo = null`.
fn prepare_for_removal(w: &mut World, m: ModIdx) {
    w.mods[m.i()].applied_to = None;
}

/// `RemoveModifiers(ModifierDescriptor)`: drops the descriptor's whole list (and its
/// dictionary slot).
pub fn remove_modifiers(w: &mut World, u: EntityIdx, t: StatType, d: D) {
    if let Some(list) = stat_mut(w, u, t).modifier_list.remove(&d) {
        for m in list.items {
            prepare_for_removal(w, m);
        }
        update_value(w, u, t);
    }
}

/// `RemoveAllModifiers()`.
pub fn remove_all_modifiers(w: &mut World, u: EntityIdx, t: StatType) {
    let all: Vec<ModIdx> = stat(w, u, t).modifiers().collect();
    for m in all {
        prepare_for_removal(w, m);
    }
    stat_mut(w, u, t).modifier_list.clear();
    update_value(w, u, t);
}

/// `RemoveModifiersFrom(EntityFactComponent)`: removes the component's modifiers, and any
/// modifier whose `AppliedTo` is already null.
pub fn remove_modifiers_from(w: &mut World, u: EntityIdx, t: StatType, c: CompRef) {
    let name = w.component(c).name.clone();
    let keys: Vec<D> = stat(w, u, t).modifier_list.keys().collect();
    for d in keys {
        let items =
            stat(w, u, t).modifier_list.get(&d).map(|l| l.items.clone()).unwrap_or_default();
        let mut keep = Vec::with_capacity(items.len());
        for m in items {
            let md = &w.mods[m.i()];
            if md.source == Some(c.fact) && md.source_component.as_deref() == Some(name.as_str()) {
                prepare_for_removal(w, m);
            }
            if w.mods[m.i()].applied_to.is_some() {
                keep.push(m);
            }
        }
        if let Some(l) = stat_mut(w, u, t).modifier_list.get_mut(&d) {
            l.items = keep;
        }
    }
    update_value(w, u, t);
}

/// `BaseValue` setter: `Math.Max(MinValue, value)` and update, if changed.
pub fn set_base_value(w: &mut World, u: EntityIdx, t: StatType, v: i32) {
    if v != stat(w, u, t).base_value {
        let min = stat(w, u, t).min_value();
        stat_mut(w, u, t).base_value = min.max(v);
        update_value(w, u, t);
    }
}

/// `AddDependentFact` / `AddDependentComponent`.
pub fn add_dependent_component(w: &mut World, u: EntityIdx, t: StatType, c: CompRef) {
    let s = stat_mut(w, u, t);
    if !s.dependent_components.contains(&c) {
        s.dependent_components.push(c);
    }
}

pub fn remove_dependent_component(w: &mut World, u: EntityIdx, t: StatType, c: CompRef) {
    let s = stat_mut(w, u, t);
    if let Some(p) = s.dependent_components.iter().position(|x| *x == c) {
        s.dependent_components.remove(p);
    }
}

pub fn add_dependent_fact(w: &mut World, u: EntityIdx, t: StatType, f: FactIdx) {
    let s = stat_mut(w, u, t);
    if !s.dependent_facts.contains(&f) {
        s.dependent_facts.push(f);
    }
}

// ---- ModifiableValueArmorClass ------------------------------------------------------

/// `UpdateSelfBonus()`: the `SelfBonus` modifier `UnitPartAcAddAc` derives from armor.
fn update_self_bonus(w: &mut World, u: EntityIdx) {
    let t = StatType::AC;
    if let Some(list) = stat_mut(w, u, t).modifier_list.remove(&D::SelfBonus) {
        for m in list.items {
            prepare_for_removal(w, m);
        }
    }
    if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartAcAddAc") {
        // Not ported: the part's settings and custom properties.
        w.coverage.unsupported("part UnitPartAcAddAc (ModifiableValueArmorClass.UpdateSelfBonus)");
    }
}

/// `UpdateInternalModifiers` of the AC: the Dexterity (or replacement) bonus, with bonus
/// mod and the lowest limiter.
fn ac_update_internal_modifiers(w: &mut World, u: EntityIdx) {
    let t = StatType::AC;
    let StatKind::ArmorClass(ac) = &stat(w, u, t).kind else { return };
    if ac.update_only_base {
        return;
    }
    let applied = ac.applied_bonus.map(|m| w.mods[m.i()].value);
    let mut bonus = attribute_bonus(w, u, ac.base_attribute);
    match ac.replace_bonus_mod {
        BonusMod::Div2 => bonus /= 2,
        BonusMod::OnePlusDiv2 => bonus = arith::add(bonus / 2, 1),
        BonusMod::Custom => {
            w.log("error", "ModifiableValueArmorClass: custom bonus mod isn't supported")
        }
        BonusMod::AsIs => {}
    }
    if let StatKind::ArmorClass(ac) = &mut stat_mut(w, u, t).kind
        && let Some(lims) = &mut ac.limiters
        && !lims.is_empty()
    {
        let cap = crate::csharp::sort::grown_capacity(lims.len());
        introsort(lims, cap, &mut |a, b| a.value.cmp(&b.value) as i32);
        bonus = lims[0].value.min(bonus);
    }
    if applied != Some(bonus) {
        if let StatKind::ArmorClass(ac) = &mut stat_mut(w, u, t).kind {
            ac.update_only_base = true;
        }
        let old = match &stat(w, u, t).kind {
            StatKind::ArmorClass(ac) => ac.applied_bonus,
            _ => None,
        };
        if let Some(m) = old {
            remove(w, m);
        }
        let m = add_modifier_value(w, u, t, bonus, D::DexterityBonus);
        if let StatKind::ArmorClass(ac) = &mut stat_mut(w, u, t).kind {
            ac.applied_bonus = Some(m);
            ac.update_only_base = false;
        }
    }
}

/// `CheckAttribute()` of the AC (also its `AttributeChangeHandler`).
pub fn ac_check_attribute(w: &mut World, u: EntityIdx) {
    let t = StatType::AC;
    let StatKind::ArmorClass(ac) = &stat(w, u, t).kind else { return };
    let replace = ac.replace;
    let if_higher = ac.replace_if_higher;
    let attr = replace.is_attribute().then_some(replace);
    let base = if if_higher {
        let Some(a) = attr else { return };
        if attribute_bonus(w, u, StatType::Dexterity) < attribute_bonus(w, u, a) {
            a
        } else {
            StatType::Dexterity
        }
    } else {
        attr.unwrap_or(StatType::Dexterity)
    };
    if let StatKind::ArmorClass(ac) = &mut stat_mut(w, u, t).kind {
        ac.base_attribute = base;
    }
    ac_update_internal_modifiers(w, u);
}

/// `event -= handler`: removes the last matching subscription.
fn unsubscribe_changed(w: &mut World, u: EntityIdx, from: StatType, h: ChangedHandler) {
    let s = stat_mut(w, u, from);
    if let Some(p) = s.on_changed.iter().rposition(|x| *x == h) {
        s.on_changed.remove(p);
    }
}

fn subscribe_changed(w: &mut World, u: EntityIdx, to: StatType, h: ChangedHandler) {
    stat_mut(w, u, to).on_changed.push(h);
}

/// `stat.OnChanged -= handler` from outside the stat classes.
pub fn unsubscribe_changed_handler(w: &mut World, u: EntityIdx, from: StatType, h: ChangedHandler) {
    unsubscribe_changed(w, u, from, h);
}

/// `stat.OnChanged += handler` from outside the stat classes.
pub fn subscribe_changed_handler(w: &mut World, u: EntityIdx, to: StatType, h: ChangedHandler) {
    subscribe_changed(w, u, to, h);
}

/// `SetupBaseAttribute(replace, replaceIfHigher, replaceBonusMod)` of the AC.
pub fn ac_setup_base_attribute(
    w: &mut World,
    u: EntityIdx,
    replace: StatType,
    if_higher: bool,
    bonus_mod: BonusMod,
) {
    let t = StatType::AC;
    let old = match &stat(w, u, t).kind {
        StatKind::ArmorClass(ac) => ac.replace,
        _ => return,
    };
    if old.is_attribute() {
        unsubscribe_changed(w, u, old, ChangedHandler::AcAttribute);
    }
    if let StatKind::ArmorClass(ac) = &mut stat_mut(w, u, t).kind {
        ac.replace_bonus_mod = bonus_mod;
        ac.replace_if_higher = if_higher;
        ac.replace = replace;
    }
    if replace.is_attribute() {
        subscribe_changed(w, u, replace, ChangedHandler::AcAttribute);
    }
    ac_check_attribute(w, u);
}

// ---- ModifiableValueSkill ----------------------------------------------------------

fn skill_update_internal_modifiers(w: &mut World, u: EntityIdx, t: StatType) {
    let StatKind::Skill { class_skill, background_source, background_modifier, .. } =
        stat(w, u, t).kind
    else {
        return;
    };
    let num = match background_source {
        Some(src) => {
            let n = class_skill > 0;
            let current_src = background_modifier.and_then(|m| w.mods[m.i()].source);
            if n && current_src != Some(src) {
                if let Some(m) = background_modifier {
                    remove(w, m);
                }
                let m = add_modifier_from_fact(w, u, t, 1, Some(src), D::Competence);
                if let StatKind::Skill { background_modifier, .. } = &mut stat_mut(w, u, t).kind {
                    *background_modifier = Some(m);
                }
            }
            n
        }
        None => false,
    };
    let current = match stat(w, u, t).kind {
        StatKind::Skill { background_modifier, .. } => background_modifier,
        _ => None,
    };
    if !num && let Some(m) = current {
        remove(w, m);
        if let StatKind::Skill { background_modifier, .. } = &mut stat_mut(w, u, t).kind {
            *background_modifier = None;
        }
    }
}

/// `CheckAttribute()` of a skill (also its `AttributeChangeHandler`).
pub fn skill_check_attribute(w: &mut World, u: EntityIdx, t: StatType) {
    let StatKind::Skill { replace, replace_if_higher, base_attribute, .. } = stat(w, u, t).kind
    else {
        return;
    };
    let attr = replace.is_attribute().then_some(replace);
    let current = if replace_if_higher {
        let Some(a) = attr else { return };
        if attribute_bonus(w, u, base_attribute) < attribute_bonus(w, u, a) {
            a
        } else {
            base_attribute
        }
    } else {
        attr.unwrap_or(base_attribute)
    };
    if let StatKind::Skill { current_base, .. } = &mut stat_mut(w, u, t).kind {
        *current_base = current;
    }
    skill_update_internal_modifiers(w, u, t);
}

/// `SetupBaseAttribute(replace, replaceIfHigher)` of a skill.
pub fn skill_setup_base_attribute(
    w: &mut World,
    u: EntityIdx,
    t: StatType,
    replace: StatType,
    if_higher: bool,
) {
    let StatKind::Skill { replace: old, .. } = stat(w, u, t).kind else { return };
    if old.is_attribute() {
        unsubscribe_changed(w, u, old, ChangedHandler::SkillAttribute(t));
    }
    if let StatKind::Skill { replace: r, replace_if_higher, .. } = &mut stat_mut(w, u, t).kind {
        *replace_if_higher = if_higher;
        *r = replace;
    }
    if replace.is_attribute() {
        subscribe_changed(w, u, replace, ChangedHandler::SkillAttribute(t));
    }
    skill_check_attribute(w, u, t);
}

/// `ClassSkillRetain()`.
pub fn class_skill_retain(w: &mut World, u: EntityIdx, t: StatType) {
    let mut changed = false;
    if let StatKind::Skill { class_skill, .. } = &mut stat_mut(w, u, t).kind {
        let before = *class_skill > 0;
        *class_skill += 1;
        changed = before != (*class_skill > 0);
    }
    if changed {
        update_value(w, u, t);
    }
}

/// `ClassSkillRelease()` (`CountableFlag.Release` doesn't go below zero).
pub fn class_skill_release(w: &mut World, u: EntityIdx, t: StatType) {
    let mut changed = false;
    if let StatKind::Skill { class_skill, .. } = &mut stat_mut(w, u, t).kind {
        let before = *class_skill > 0;
        *class_skill = (*class_skill - 1).max(0);
        changed = before != (*class_skill > 0);
    }
    if changed {
        update_value(w, u, t);
    }
}

// ---- ModifiableValueInitiative -----------------------------------------------------

fn initiative_update_internal_modifiers(w: &mut World, u: EntityIdx, t: StatType) {
    let StatKind::Initiative { dexterity, dex_bonus, .. } = stat(w, u, t).kind else { return };
    let bonus = attribute_bonus(w, u, dexterity);
    if let Some(m) = dex_bonus {
        remove(w, m);
    }
    let m = add_modifier_value(w, u, t, bonus, D::DexterityBonus);
    if let StatKind::Initiative { dex_bonus, .. } = &mut stat_mut(w, u, t).kind {
        *dex_bonus = Some(m);
    }
}

// ---- ModifiableValueHitPoints ------------------------------------------------------

fn hp_update_internal_modifiers(w: &mut World, u: EntityIdx, t: StatType) {
    remove_modifiers(w, u, t, D::BaseStatBonus);
    let StatKind::HitPoints { base_stat } = stat(w, u, t).kind else { return };
    let level = crate::unit::character_level(w, u);
    let num = match base_stat {
        HitPointsBaseStat::Constitution => {
            arith::mul(attribute_bonus(w, u, StatType::Constitution), level)
        }
        HitPointsBaseStat::Charisma => arith::mul(attribute_bonus(w, u, StatType::Charisma), level),
        HitPointsBaseStat::Size => crate::unit::hp_size_bonus(w, u),
    };
    if num != 0 {
        add_modifier_value(w, u, t, num, D::BaseStatBonus);
    }
    remove_modifiers(w, u, t, D::FavouredClassBonus);
    let fcb = crate::unit::favoured_class_level(w, u);
    add_modifier_value(w, u, t, fcb, D::FavouredClassBonus);
}

// ---- Character stats ---------------------------------------------------------------

/// `ModifiableValue.PostLoad(stats)` → `OnPostLoad()` for one stat.
pub fn stat_post_load(w: &mut World, u: EntityIdx, t: StatType) {
    stat_mut(w, u, t).bound = true;
    match stat(w, u, t).kind.clone() {
        StatKind::ArmorClass(ac) => {
            ac_setup_base_attribute(w, u, ac.replace, ac.replace_if_higher, ac.replace_bonus_mod);
            subscribe_changed(w, u, StatType::Dexterity, ChangedHandler::AcAttribute);
        }
        StatKind::Skill { replace, replace_if_higher, base_attribute, .. } => {
            skill_setup_base_attribute(w, u, t, replace, replace_if_higher);
            if base_attribute.is_attribute() {
                subscribe_changed(w, u, base_attribute, ChangedHandler::SkillAttribute(t));
            }
        }
        _ => {}
    }
}

/// `CharacterStats.PostLoad(owner)`: bind every stat (running its `OnPostLoad`), then
/// update every stat, both in `CollectAllStats` order.
pub fn character_stats_post_load(w: &mut World, u: EntityIdx) {
    w.trace("CharacterStats.PostLoad", |w| w.entity(u).unique_id.clone());
    for t in StatType::ALL {
        stat_post_load(w, u, t);
    }
    for t in StatType::ALL {
        update_value(w, u, t);
    }
}

/// `ModifiableValue.CleanupModifiers(owner)`.
pub fn cleanup_modifiers(w: &mut World, u: EntityIdx, t: StatType) {
    let mut changed = false;
    let keys: Vec<D> = stat(w, u, t).modifier_list.keys().collect();
    for d in &keys {
        let items = stat(w, u, t).modifier_list.get(d).map(|l| l.items.clone()).unwrap_or_default();
        let mut keep = Vec::new();
        for m in items {
            if is_modifier_valid(w, u, m) {
                keep.push(m);
            } else {
                prepare_for_removal(w, m);
                changed = true;
            }
        }
        if let Some(l) = stat_mut(w, u, t).modifier_list.get_mut(d) {
            l.items = keep;
        }
    }
    // Duplicate sourced modifiers: both entries of each matching pair are removed.
    for d in &keys {
        let mut items: Vec<Option<ModIdx>> = stat(w, u, t)
            .modifier_list
            .get(d)
            .map(|l| l.items.iter().copied().map(Some).collect())
            .unwrap_or_default();
        for i in 0..items.len() {
            // The C# reads `modifier = list[i]` once, before the inner loop: after it is
            // nulled by a first match it still matches later duplicates, so three equal
            // sourced modifiers all go. A null `modifier` never matches
            // (`modifier?.Source == modifier2.Source` with a non-null source is false).
            let Some(a) = items[i] else { continue };
            for j in i + 1..items.len() {
                let Some(b) = items[j] else { continue };
                let (ma, mb) = (&w.mods[a.i()], &w.mods[b.i()]);
                if mb.source.is_some()
                    && ma.source == mb.source
                    && ma.value == mb.value
                    && ma.source_component == mb.source_component
                {
                    prepare_for_removal(w, a);
                    prepare_for_removal(w, b);
                    items[j] = None;
                    items[i] = None;
                }
            }
        }
        let before = items.len();
        let kept: Vec<ModIdx> = items.into_iter().flatten().collect();
        changed |= kept.len() != before;
        if let Some(l) = stat_mut(w, u, t).modifier_list.get_mut(d) {
            l.items = kept;
        }
    }
    if changed {
        update_value(w, u, t);
    }
}

/// `IsModifierValid(owner, mod)`.
fn is_modifier_valid(w: &mut World, u: EntityIdx, m: ModIdx) -> bool {
    let md = w.mods[m.i()].clone();
    if let Some(item) = md.item_source {
        let Some(it) = w.item(item) else { return false };
        let inv = w.unit(u).and_then(|x| x.inventory_collection);
        if it.collection.is_none() || it.collection != inv || it.wielder != Some(u) {
            return false;
        }
    }
    if let Some(src) = md.source {
        let f = w.fact(src);
        if !f.is_active {
            return false;
        }
        let is_enchantment = f.kind == crate::world::FactKind::ItemEnchantment;
        if !is_enchantment && !w.entity(u).facts.list.contains(&src) {
            return false;
        }
        let Some(name) = md.source_component.as_deref().filter(|s| !s.trim().is_empty()) else {
            return true;
        };
        return crate::facts::select_components_named_any(w, src, name);
    }
    true
}

/// `CharacterStats.ApplyPostLoadFixes()`: `CleanupModifiers` then the stat's own fixes,
/// per stat.
pub fn character_stats_apply_post_load_fixes(w: &mut World, u: EntityIdx) {
    let status_buffs = status_buff_blueprints(w);
    for t in StatType::ALL {
        cleanup_modifiers(w, u, t);
        // `OnApplyPostLoadFixes`: attributes re-sync their damage/drain status buffs,
        // adding a missing one or removing a stale one. Nothing to do only when there is
        // no damage, no drain and no status buff from `BlueprintRoot.StatusBuffs` on the
        // unit; otherwise (not ported) it's reported.
        if let StatKind::Attribute { .. } = stat(w, u, t).kind {
            let has_damage = stat(w, u, t).modifiers().any(|m| w.mods[m.i()].desc == D::StatDamage);
            let has_drain = stat(w, u, t).modifiers().any(|m| w.mods[m.i()].desc == D::StatDrain);
            let has_status = w
                .entity(u)
                .facts
                .list
                .iter()
                .any(|f| w.fact(*f).blueprint.is_some_and(|g| status_buffs.contains(&g)));
            if has_damage || has_drain || has_status {
                w.coverage.unsupported("ModifiableValueAttributeStat.UpdateDamageStatusBuff");
            }
        }
    }
}

/// Every blueprint referenced from `BlueprintRoot.StatusBuffs`.
fn status_buff_blueprints(w: &World) -> Vec<wr_content::Guid> {
    let mut out = Vec::new();
    let Some(root) = w.content.get(crate::unit::BLUEPRINT_ROOT) else { return out };
    if let Some(n) = root.view().node("StatusBuffs") {
        let mut path = String::new();
        wr_content::node::walk(n, &mut path, &mut |_, x| {
            if let wr_content::Node::BpRef(g) = x
                && !g.is_empty()
            {
                out.push(*g);
            }
        });
    }
    out
}
