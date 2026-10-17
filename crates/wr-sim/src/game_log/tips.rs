//! The combat log's attack and damage tooltips (decompiled, not copied:
//! `Kingmaker.Blueprints.Root.Strings.GameLog/AttackLogMessage.cs` `GetData`,
//! `AppendAttackBonusBreakdown`, `AppendArmorClassBreakdown`; `DamageLogMessage.cs` `GetData`,
//! `AppendDamageDetails`, `AppendMultipliers`, `AppendSources`, `AppendDamageType`, `AppendDice`,
//! `AppendDifficultySettings`; `Kingmaker.UI.Common/StatModifiersBreakdown.cs`;
//! `StatBonusEntry.cs`; `Kingmaker.Enums/ModifierDescriptorComparer.cs`;
//! `Kingmaker.EntitySystem.Stats/ModifiableValue.cs` `GetDisplayModifiers`;
//! `ModifiableValueArmorClass.cs`' filters; `Kingmaker.EntitySystem/EntityFact.cs` `Name`,
//! `Kingmaker.UnitLogic/Feature.cs` `Name`; the string containers' `GetText`/`GetName`).
//!
//! The rules keep what the tooltips read when they trigger (the attack bonus's and armour
//! class's bonuses and the stat modifiers "at the moment", the damage's values), as the C#
//! rules keep them; the text is made when the log's thread handles the event, from the
//! world as it is then (names, `AC.BaseValue`), as the game's is. Branches the sim can't make
//! are reported (`TextError::Unsupported`), never approximated.

use wr_text::{Color32, TextError};

use crate::combat::attack::{BaseDamage, Bonus, DamageValue};
use crate::combat::weapon::DiceFormula;
use crate::stats::types::{DESCRIPTOR_DISPLAY_ORDER, ModifierDescriptor as MD, StatType};
use crate::world::{EntityIdx, FactIdx, FactKind, World};

use super::{names, text_of};

type Res<T> = Result<T, TextError>;

fn unsupported<T>(what: impl Into<String>) -> Res<T> {
    Err(TextError::Unsupported(what.into()))
}

// ---- what the rules keep ----

/// A `ModifiableValue.Modifier` of a rule's `StatModifiersAtTheMoment`: the members the
/// breakdown reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatMod {
    pub value: i32,
    pub desc: MD,
    /// `Source`.
    pub source: Option<FactIdx>,
    /// `ItemSource`.
    pub item_source: Option<EntityIdx>,
}

/// `RuleAttackRoll.AttackBonusRule` as the tooltip reads it: `Result`, `AllBonuses` (its own
/// then its inner rule's) and the inner rule's `StatModifiersAtTheMoment`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AttackBonusRule {
    pub result: i32,
    pub all_bonuses: Vec<Bonus>,
    pub stat_mods: Vec<StatMod>,
}

/// `RuleAttackRoll.ACRule` as the tooltip reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcRule {
    pub result: i32,
    pub flat_footed: bool,
    /// `AttackType.IsTouch()`.
    pub touch: bool,
    /// `Target` (its `Stats.AC.BaseValue` is read when the text is made).
    pub target: EntityIdx,
    pub stat_mods: Vec<StatMod>,
}

/// What `DamageLogMessage.GetData` reads of a `RuleDealDamage` beyond the message's members.
#[derive(Debug, Clone, Default)]
pub struct DamageRule {
    /// `ResultList`.
    pub results: Vec<DamageValue>,
    /// `DamageBundle.WeaponDamage` is `ResultList[0].Source` (a weapon's bundle: its first
    /// damage is the weapon's, the first value calculated).
    pub first_is_weapon_damage: bool,
    /// `DamageBundle.Weapon`.
    pub weapon: Option<EntityIdx>,
    pub difficulty_modifier: f32,
    pub damage_before_difficulty: i32,
    pub result: i32,
}

// ---- ModifiableValue.GetDisplayModifiers and StatModifiersBreakdown's filters ----

/// `ModifierDescriptorComparer.SortedValues`: its 45 listed descriptors, then the enum's other
/// values in `Enum.GetValues` order (ascending).
fn sorted_descriptors() -> impl Iterator<Item = MD> {
    const OTHERS: [i32; 13] = [6, 35, 45, 46, 47, 48, 49, 50, 51, 52, 54, 55, 56];
    DESCRIPTOR_DISPLAY_ORDER.into_iter().chain(OTHERS.into_iter().map(MD))
}

/// `ModifierDescriptorComparer`'s order of a descriptor (`None` for a value outside the enum,
/// whose comparison throws).
fn descriptor_order(d: MD) -> Option<usize> {
    sorted_descriptors().position(|x| x == d)
}

/// `ModifiableValue.GetDisplayModifiers()` then `StatModifiersBreakdown`'s
/// `GetDisplayModifiersInternal(stat, excludeDisabled)` (a non-attribute stat: no helpless
/// copy): each descriptor's list in the display order, its stacking modifiers and its largest
/// bonus and penalty of the others, without zeros (and disabled morale or insight ones).
/// Reported: facts hidden while an etude plays (`UnitPartHiddenFacts`).
pub fn display_modifiers(
    w: &mut World,
    u: EntityIdx,
    stat: StatType,
    exclude_disabled: bool,
) -> Vec<StatMod> {
    if w.entity(u).parts.iter().any(|p| p.clr == "UnitPartHiddenFacts") {
        w.coverage.unsupported(format!(
            "combat log: a tooltip's {stat:?} modifiers (UnitPartHiddenFacts: HideWhileEtudePlaying)"
        ));
    }
    let s = w.stats(u).v(stat);
    let snap = |m: crate::world::ModIdx| {
        let x = &w.mods[m.i()];
        (
            StatMod { value: x.value, desc: x.desc, source: x.source, item_source: x.item_source },
            x.stacks(),
        )
    };
    // `GetStackingPenalty`: the smallest stacking value below 0 of a descriptor's list.
    let stacking_penalty = |d: MD| {
        s.modifier_list.get(&d).map_or(0, |l| {
            l.items
                .iter()
                .map(|&m| snap(m))
                .filter(|(m, st)| *st && m.value < 0)
                .fold(0, |a, (m, _)| a.min(m.value))
        })
    };
    let armor_encumbrance = stacking_penalty(MD::Armor).min(stacking_penalty(MD::Encumbrance));
    let mut out = Vec::new();
    for d in sorted_descriptors() {
        let Some(list) = s.modifier_list.get(&d) else { continue };
        let (mut bonus, mut penalty): (Option<StatMod>, Option<StatMod>) = (None, None);
        for &m in &list.items {
            let (m, stacks) = snap(m);
            if stacks {
                out.push(m);
                continue;
            }
            if (m.desc == MD::Armor || m.desc == MD::Encumbrance)
                && m.value < 0
                && m.value > armor_encumbrance
            {
                continue;
            }
            if m.value > 0 {
                if bonus.is_none_or(|b| b.value < m.value) {
                    bonus = Some(m);
                }
            } else if penalty.is_none_or(|p| p.value > m.value) {
                penalty = Some(m);
            }
        }
        out.extend(bonus);
        out.extend(penalty);
    }
    let (morale, insight) = (s.morale_disabled > 0, s.insight_disabled > 0);
    out.retain(|m| {
        let disabled = (morale && m.desc == MD::Morale)
            || (insight && (m.desc == MD::Insight || m.desc == MD::Competence));
        m.value != 0 && (!exclude_disabled || !disabled)
    });
    out
}

/// `StatModifiersBreakdown.GetDisplayArmorModifiers(acStat, flatfooted, touch)`.
pub fn display_armor_modifiers(
    w: &mut World,
    u: EntityIdx,
    flat_footed: bool,
    touch: bool,
) -> Vec<StatMod> {
    let is_armor = |d: MD| {
        matches!(
            d,
            MD::Armor
                | MD::ArmorEnhancement
                | MD::ArmorFocus
                | MD::NaturalArmor
                | MD::NaturalArmorEnhancement
                | MD::NaturalArmorForm
        )
    };
    let is_shield = |d: MD| matches!(d, MD::Shield | MD::ShieldEnhancement | MD::Focus);
    let is_dodge = |d: MD| matches!(d, MD::Dodge | MD::DexterityBonus);
    let mut v = display_modifiers(w, u, StatType::AC, false);
    v.retain(|m| {
        (!flat_footed || m.value < 0 || !is_dodge(m.desc))
            && (!touch || m.value < 0 || (!is_armor(m.desc) && !is_shield(m.desc)))
    });
    v
}

// ---- strings ----

/// `AbilityModifiersStrings.GetName(descriptor)`: the first entry with the key, else
/// `DefaultName`.
fn descriptor_name(w: &World, d: MD) -> Res<String> {
    let s = super::strings(w)?;
    let entries = s
        .get("AbilityModifiers/Entries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| TextError::Unsupported("AbilityModifiers.Entries missing".into()))?;
    for (i, e) in entries.iter().enumerate() {
        if e.get("Key").and_then(serde_json::Value::as_i64) == Some(i64::from(d.0)) {
            return text_of(w, &format!("AbilityModifiers/Entries/{i}/Name"));
        }
    }
    text_of(w, "AbilityModifiers/DefaultName")
}

/// `StatsStrings.GetText(StatType)` (or `GetText(WeaponCategory)` with `weapon`): the last
/// entry with the value, else the enum's name.
fn stats_text(w: &World, v: i64, weapon: bool) -> Res<String> {
    let s = super::strings(w)?;
    let (list, key) = if weapon { ("WeaponEntries", "Proficiency") } else { ("Entries", "Stat") };
    let entries = s
        .get(&format!("Stats/{list}"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| TextError::Unsupported(format!("Stats.{list} missing")))?;
    if let Some(i) =
        entries.iter().rposition(|e| e.get(key).and_then(serde_json::Value::as_i64) == Some(v))
    {
        return text_of(w, &format!("Stats/{list}/{i}/Text"));
    }
    if weapon {
        return unsupported(format!("WeaponCategory {v}'s name (not in StatsStrings)"));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(StatType(v as i32).name().into_owned())
}

/// An `EnumStrings<T>.GetText(value)` container (`DamageForm`, `SpellSchoolNames`…): the last
/// entry with the value, else the enum's name (reported).
pub(super) fn enum_text(w: &World, container: &str, v: i64) -> Res<String> {
    let s = super::strings(w)?;
    let entries = s
        .get(&format!("{container}/Entries"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| TextError::Unsupported(format!("{container}.Entries missing")))?;
    match entries
        .iter()
        .rposition(|e| e.get("Value").and_then(serde_json::Value::as_i64) == Some(v))
    {
        Some(i) => text_of(w, &format!("{container}/Entries/{i}/Text")),
        None => unsupported(format!("{container}: value {v} has no entry (its enum name)")),
    }
}

/// `BonusSourceStrings.GetText(BonusType)`.
fn bonus_type_text(w: &World, ty: &str) -> Res<String> {
    match ty {
        "None" => Ok(String::new()),
        "Concealment" | "Flanking" | "SecondaryWeapon" | "ShootIntoCombat" | "Blindness"
        | "TargetStunned" => text_of(w, &format!("BonusSources/{ty}")),
        _ => Err(TextError::Throws(format!("BonusSourceStrings.GetText({ty})"))),
    }
}

/// `ColorUtility.ToHtmlStringRGB(UIRoot.TooltipColors.<field>)` (a `Color32` through `Color`).
fn tooltip_color(w: &World, field: &str) -> Res<String> {
    let root = w
        .content
        .get(crate::unit::BLUEPRINT_ROOT)
        .ok_or_else(|| TextError::Unsupported("BlueprintRoot missing".into()))?;
    let v = root.view();
    match v.obj("UIRoot").and_then(|u| u.obj("TooltipColors")).and_then(|c| c.node(field)) {
        Some(wr_content::node::Node::Int(i)) => {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let [r, g, b, a] = (*i as i32 as u32).to_le_bytes();
            Ok(Color32 { r, g, b, a }.html_rgb_via_color())
        }
        _ => unsupported(format!("UIRoot.TooltipColors.{field} missing")),
    }
}

// ---- names ----

/// `EntityFact.Name` (`SelectUIData(Name)`: the blueprint's `IUIDataProvider` name when not
/// empty), with `Feature.Name`'s parameter. `UIDataProviderOverride` is set only by Shifter's
/// Fury (not modelled).
fn fact_name(w: &World, f: FactIdx) -> Res<String> {
    let fact = w.fact(f);
    let Some(g) = fact.blueprint else { return unsupported("a fact without a blueprint's name") };
    let b =
        w.content.get(g).ok_or_else(|| TextError::Unsupported(format!("blueprint {g} missing")))?;
    if !b.view().is("BlueprintUnitFact") {
        return unsupported(format!(
            "the name of a fact of {} (not a BlueprintUnitFact)",
            b.view().type_name()
        ));
    }
    let base = names::blueprint_text(w, g, "m_DisplayName")?;
    if base.is_empty() && fact.kind == FactKind::Buff {
        return unsupported("a buff without a name of its own (its context's UI data)");
    }
    if fact.kind != FactKind::Feature {
        return Ok(base);
    }
    let Some(p) = fact.param.as_ref() else { return Ok(base) };
    let p = crate::feature_param::FeatureParam::from_json(p).ok_or_else(|| {
        TextError::Unsupported("FeatureParam: unrecognized persisted form".into())
    })?;
    if let Some(c) = p.weapon_category {
        return Ok(format!("{base} ({})", stats_text(w, c, true)?));
    }
    if let Some(s) = p.spell_school {
        return Ok(format!("{base} ({})", enum_text(w, "SpellSchoolNames", s)?));
    }
    if let Some(s) = p.stat_type {
        return Ok(format!("{base} ({})", stats_text(w, s, false)?));
    }
    if p.blueprint.is_some() {
        return unsupported("a feature's name with a blueprint parameter");
    }
    Ok(base)
}

/// What a source provides to `GetBonusSourceText(IUIDataProvider)`.
#[derive(Clone, Copy)]
enum Provider {
    Fact(FactIdx),
    Item(EntityIdx),
}

/// `StatModifiersBreakdown.GetBonusSourceText(IUIDataProvider)` (not a development build: no
/// placeholder for an empty name): an item enchantment's owner's name, else the source's name;
/// when empty, a fact's context's parent's source item's name.
fn provider_text(w: &World, p: Option<Provider>) -> Res<String> {
    let Some(p) = p else { return Ok(String::new()) };
    let text = match p {
        Provider::Item(i) => names::item_name(w, i)?,
        Provider::Fact(f) => {
            let fact = w.fact(f);
            match (fact.kind, fact.owner) {
                (FactKind::ItemEnchantment, Some(o)) if w.item(o).is_some() => {
                    names::item_name(w, o)?
                }
                (FactKind::ItemEnchantment, _) => {
                    return unsupported("an item enchantment's name without its owner");
                }
                _ => fact_name(w, f)?,
            }
        }
    };
    if !text.is_empty() {
        return Ok(text);
    }
    if let Provider::Fact(f) = p
        && let Some(item) = w.fact(f).context.as_ref().and_then(|c| c.parent_source_item(w))
    {
        return names::item_name(w, item);
    }
    Ok(text)
}

/// `StatModifiersBreakdown.GetBonusSourceText(Modifier)`: its fact's, its bonus type's or its
/// stat's name.
fn bonus_text(w: &World, b: &Bonus) -> Res<String> {
    if let Some(f) = b.fact {
        return provider_text(w, Some(Provider::Fact(f)));
    }
    if b.bonus_type != "None" {
        return bonus_type_text(w, b.bonus_type);
    }
    if b.stat != StatType::Unknown {
        return stats_text(w, i64::from(b.stat.0), false);
    }
    Ok(String::new())
}

// ---- StatModifiersBreakdown ----

/// `StatBonusEntry`.
#[derive(Debug, Clone)]
struct Entry {
    bonus: i32,
    source: Option<String>,
    desc: MD,
}

/// `StatModifiersBreakdown.s_Data` between its additions and `AppendModifiersBreakdown`.
#[derive(Debug, Default)]
struct Breakdown(Vec<Entry>);

impl Breakdown {
    /// `AddBonus(bonusValue, bonusSource, descriptor, addZero)` (no dice formula: the rules'
    /// modifiers the sim makes have none): sizes without a source merge.
    fn add(&mut self, value: i32, source: Option<String>, desc: MD, add_zero: bool) {
        if value == 0 && !add_zero {
            return;
        }
        if desc == MD::Size
            && source.as_deref().is_none_or(str::is_empty)
            && let Some(e) = self
                .0
                .iter_mut()
                .find(|e| e.source.as_deref().is_none_or(str::is_empty) && e.desc == MD::Size)
        {
            e.bonus = crate::csharp::arith::add(e.bonus, value);
            e.source = None;
            return;
        }
        self.0.push(Entry { bonus: value, source, desc });
    }

    /// `AddBonusSources(modifiers)`: each `AddBonus(Modifier)`; a stat's shows at zero.
    fn add_rule_bonuses(&mut self, w: &World, bonuses: &[Bonus]) -> Res<()> {
        for b in bonuses {
            let show_zero = b.stat != StatType::Unknown;
            if b.value != 0 || show_zero {
                let src = bonus_text(w, b)?;
                self.add(b.value, Some(src), b.descriptor, show_zero);
            }
        }
        Ok(())
    }

    /// `AddModifiers(modifiers, itemSourceFirst)`: each stat modifier's descriptor (`Other` for
    /// none) and its item source or source fact, in the order asked.
    fn add_stat_mods(&mut self, w: &World, mods: &[StatMod], item_source_first: bool) -> Res<()> {
        for m in mods {
            let desc = if m.desc == MD::None { MD::Other } else { m.desc };
            let item = m.item_source.map(Provider::Item);
            let fact = m.source.map(Provider::Fact);
            let p = if item_source_first { item.or(fact) } else { fact.or(item) };
            // `AddBonus(int, IUIDataProvider, descriptor)`: nothing for a zero.
            if m.value != 0 {
                let src = provider_text(w, p)?;
                self.add(m.value, Some(src), desc, false);
            }
        }
        Ok(())
    }

    /// `AppendModifiersBreakdown(sb)`: sorted by descriptor (Mono's unstable introsort), each
    /// nonzero entry as a line, then cleared.
    fn append(&mut self, w: &World, sb: &mut String) -> Res<()> {
        if self.0.is_empty() {
            return Ok(());
        }
        // The list's capacity (a static list's, unknown) only matters past insertion sort.
        if self.0.len() > 16 {
            return unsupported("a breakdown of more than 16 entries (its list's capacity)");
        }
        for e in &self.0 {
            if descriptor_order(e.desc).is_none() {
                return Err(TextError::Throws(format!("ModifierDescriptorComparer: {}", e.desc.0)));
            }
        }
        let n = self.0.len();
        crate::csharp::sort::introsort(&mut self.0, n, &mut |a: &Entry, b: &Entry| {
            let (x, y) = (descriptor_order(a.desc), descriptor_order(b.desc));
            match x.cmp(&y) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            }
        });
        let entries = std::mem::take(&mut self.0);
        for e in entries {
            if e.bonus != 0 {
                append_bonus(w, sb, e.bonus, e.source.as_deref(), e.desc)?;
            }
        }
        Ok(())
    }
}

/// `string.IsNullOrWhiteSpace`.
fn blank(s: Option<&str>) -> bool {
    s.is_none_or(|s| s.chars().all(char::is_whitespace))
}

/// `UIUtility.AddSign(value)`: `+` or the en dash, then the magnitude.
fn add_sign(v: i32) -> String {
    format!("{}{}", if v >= 0 { "+" } else { "\u{2013}" }, v.unsigned_abs())
}

/// `StatModifiersBreakdown.AppendBonus(sb, bonusValue, bonusSource, descriptor, null)`.
fn append_bonus(w: &World, sb: &mut String, value: i32, source: Option<&str>, desc: MD) -> Res<()> {
    if desc != MD::None {
        sb.push_str(&descriptor_name(w, desc)?);
        sb.push_str(": ");
    } else if !blank(source) {
        sb.push_str(source.unwrap_or_default());
        sb.push_str(": ");
    }
    let color = tooltip_color(w, if value < 0 { "Penaty" } else { "Bonus" })?;
    sb.push_str("<color=#");
    sb.push_str(&color);
    sb.push('>');
    sb.push_str(&add_sign(value));
    if desc != MD::None && !blank(source) {
        sb.push_str(" [");
        sb.push_str(source.unwrap_or_default());
        sb.push(']');
    }
    sb.push_str("</color>");
    sb.push('\n');
    Ok(())
}

/// `StatModifiersBreakdown.AppendIndented(sb, textBlock)`: each line of the trimmed block that
/// isn't blank, indented by four spaces.
fn append_indented(sb: &mut String, block: &str) {
    for line in block.trim_end().split('\n') {
        if !blank(Some(line)) {
            sb.push_str("    ");
            sb.push_str(line);
            sb.push('\n');
        }
    }
}

/// `float.ToString()` (Mono, the oracle's culture: `.` decimals): "G7".
fn float_text(v: f32) -> Res<String> {
    if !v.is_finite() {
        return unsupported(format!("a multiplier's text ({v})"));
    }
    if v == 0.0 {
        return Ok("0".into());
    }
    // Mono's formatter: the value as a double to 15 significant digits, rounded half up to 7
    // (`1.0078125f` is "1.007813"); fixed-point for a decimal exponent from -5 (exclusive) to 7.
    let s = format!("{:.14e}", f64::from(v));
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let mut exp: i32 = exp.parse().unwrap_or(0);
    let neg = mant.starts_with('-');
    let mut d: Vec<u8> = mant.bytes().filter(u8::is_ascii_digit).map(|b| b - b'0').collect();
    let up = d.get(7).is_some_and(|&x| x >= 5);
    d.truncate(7);
    if up {
        let mut i = d.len();
        loop {
            if i == 0 {
                d.insert(0, 1);
                d.truncate(7);
                exp += 1;
                break;
            }
            i -= 1;
            if d[i] == 9 {
                d[i] = 0;
            } else {
                d[i] += 1;
                break;
            }
        }
    }
    if !(-4..7).contains(&exp) {
        return unsupported(format!("a multiplier's text in scientific notation ({v})"));
    }
    let digits: String = d.iter().map(|x| char::from(b'0' + x)).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let point = exp + 1;
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if point <= 0 {
        out.push_str("0.");
        for _ in 0..-point {
            out.push('0');
        }
        out.push_str(digits);
    } else {
        #[allow(clippy::cast_sign_loss)]
        let p = point as usize;
        if digits.len() <= p {
            out.push_str(digits);
            for _ in digits.len()..p {
                out.push('0');
            }
        } else {
            out.push_str(&digits[..p]);
            out.push('.');
            out.push_str(&digits[p..]);
        }
    }
    Ok(out)
}

/// `StatModifiersBreakdown.AppendMultiplier(sb, multiplier)`.
fn append_multiplier(sb: &mut String, m: f32) -> Res<()> {
    sb.push_str(" (\u{d7}");
    sb.push_str(&float_text(m)?);
    sb.push(')');
    Ok(())
}

// ---- AttackLogMessage ----

/// `AppendAttackBonusBreakdown(sb, rule)`.
pub fn append_attack_bonus(w: &World, sb: &mut String, r: &AttackBonusRule) -> Res<()> {
    sb.push_str("<b>");
    sb.push_str(&text_of(w, "GameLog/Attack/AttackBonusBreakdown")?);
    sb.push_str(": ");
    sb.push_str(&add_sign(r.result));
    sb.push_str("</b>");
    sb.push('\n');
    let mut b = Breakdown::default();
    b.add_rule_bonuses(w, &r.all_bonuses)?;
    b.add_stat_mods(w, &r.stat_mods, true)?;
    b.append(w, sb)
}

/// `AppendArmorClassBreakdown(sb, rule)`: `AC.BaseValue` as it is now.
pub fn append_armor_class(w: &World, sb: &mut String, r: &AcRule) -> Res<()> {
    sb.push_str("<b>");
    sb.push_str(&text_of(w, "GameLog/Attack/ArmorClassBreakdown")?);
    sb.push_str(": ");
    sb.push_str(&r.result.to_string());
    if r.flat_footed || r.touch {
        sb.push_str(" (");
        if r.flat_footed {
            sb.push_str(&text_of(w, "GameLog/Attack/Flatfooted")?);
            if r.touch {
                sb.push_str(", ");
            }
        }
        if r.touch {
            sb.push_str(&text_of(w, "GameLog/Attack/Touch")?);
        }
        sb.push(')');
    }
    sb.push_str("</b>\n");
    sb.push_str(&text_of(w, "BonusSources/ArmorClassBase")?);
    sb.push_str(": ");
    sb.push_str(&w.stats(r.target).v(StatType::AC).base_value.to_string());
    sb.push('\n');
    let mut b = Breakdown::default();
    b.add_stat_mods(w, &r.stat_mods, false)?;
    b.append(w, sb)
}

// ---- DamageLogMessage ----

/// `DamageLogMessage`'s fields its methods set (`m_SavingThrowModifier`,
/// `m_BonusPercentModifier`, `m_DeclineMessage`): reset by `GetData`, kept from one damage value
/// to the next.
#[derive(Debug, Default)]
struct DamageState {
    bonus_percent: Option<f32>,
}

/// `DamageLogMessage.GetData`'s tooltip after `Tooltip` (the context's `Count` set): the values'
/// sum and the difficulty's multiplier, each value's details, the difficulty's line.
pub fn damage_tooltip(w: &World, d: &DamageRule) -> Res<String> {
    let mut sb = text_of(w, "GameLog/Damage/Tooltip")?;
    // `RedirectedDamage`: the sim has no redirection (`RedirectionTarget` null).
    let difficulty = d.result != d.damage_before_difficulty;
    let n = d.results.len();
    if difficulty || n > 1 {
        sb.push_str("\n\n<b>");
        if n > 1 && difficulty {
            sb.push('(');
        }
        for (i, v) in d.results.iter().enumerate() {
            if i != 0 {
                sb.push_str(" + ");
            }
            sb.push_str(&v.final_value().to_string());
        }
        if difficulty {
            if n > 1 {
                sb.push(')');
            }
            append_multiplier(&mut sb, d.difficulty_modifier)?;
        }
        sb.push_str(" = ");
        sb.push_str(&d.result.to_string());
        sb.push_str("</b>");
    }
    sb.push_str("\n\n");
    let mut st = DamageState::default();
    for (i, v) in d.results.iter().enumerate() {
        damage_details(w, &mut sb, d, i, v, &mut st)?;
    }
    if difficulty {
        sb.push_str("<b>");
        append_multiplier(&mut sb, d.difficulty_modifier)?;
        sb.push_str("</b>");
        sb.push_str(": ");
        let name = text_of(w, "SettingsRoot/Settings/DamageToParty/m_Description")?;
        append_difficulty_settings(w, &mut sb, &name)?;
    }
    Ok(sb)
}

/// `AppendDifficultySettings(tooltip, name)`.
fn append_difficulty_settings(w: &World, sb: &mut String, name: &str) -> Res<()> {
    sb.push_str(&descriptor_name(w, MD::Difficulty)?);
    sb.push(' ');
    sb.push('(');
    sb.push_str(name);
    sb.push(')');
    Ok(())
}

/// `AppendDice(sb, dice)`.
fn append_dice(sb: &mut String, f: DiceFormula) {
    sb.push_str(&f.rolls().to_string());
    if f.dice > 1 {
        sb.push('d');
        sb.push_str(&f.dice.to_string());
    }
}

/// `DiceFormula.ToString()`.
fn dice_text(f: DiceFormula) -> String {
    if f == DiceFormula::ZERO || f == DiceFormula::ONE {
        f.rolls().to_string()
    } else {
        format!("{}d{}", f.rolls, f.dice)
    }
}

/// `CriticalApplied`'s `ToBonus`.
const CRIT_TO_BONUS: i32 = 2;

/// `AppendDamageDetails`' loop body for one value.
fn damage_details(
    w: &World,
    sb: &mut String,
    d: &DamageRule,
    index: usize,
    v: &DamageValue,
    st: &mut DamageState,
) -> Res<()> {
    let s = &v.source;
    let modified = s.dice.modified();
    let has_dice = modified.dice != 0 && modified.rolls() > 0;
    let crit_to_bonus = s.critical_modifier.is_some() && (s.critical_applied & CRIT_TO_BONUS) != 0;
    let length = sb.len();
    if has_dice {
        append_dice(sb, modified);
    }
    let total = crate::csharp::arith::add(s.bonus, s.bonus_target_related);
    let mut not_only_dice = false;
    if total != 0 {
        if has_dice && total > 0 {
            sb.push_str(" + ");
        }
        sb.push_str(&total.to_string());
        not_only_dice = true;
        if crit_to_bonus {
            #[allow(clippy::cast_precision_loss)]
            append_multiplier(sb, s.critical_modifier.unwrap_or(1) as f32)?;
        } else if s.critical_modifier.is_some() {
            append_multiplier(sb, 1.0)?;
        }
    }
    // `PostCritIncrements.OverallBonus`: no post-crit increments are made by the sim.
    let length2 = sb.len();
    if !has_dice && total == 0 {
        sb.push('0');
    } else if append_multipliers(sb, v, st)? {
        not_only_dice = true;
        if has_dice && total != 0 {
            sb.insert(length, '(');
            sb.insert(length2 + 1, ')');
        }
    }
    // `IgnoreDamageReduction` is never set by the sim.
    let reduction = v.reduction();
    if reduction > 0 {
        sb.push_str(" - ");
        sb.push_str(&reduction.to_string());
        not_only_dice = true;
    }
    sb.push_str(" = <b>");
    sb.push_str(&v.final_value().to_string());
    sb.push_str("</b> ");
    append_damage_type(w, sb, s)?;
    if s.sneak {
        sb.push_str(&text_of(w, "GameLog/Damage/DamageSneak")?);
        sb.push(' ');
    } else if s.precision {
        sb.push_str(&text_of(w, "GameLog/Damage/DamagePrecision")?);
        sb.push(' ');
    }
    if s.calculation_type == 1 {
        return unsupported("a maximized damage's tooltip (its source)");
    }
    if s.decline() >= 3 {
        return unsupported("an immune damage's tooltip (its decline's source)");
    }
    if s.maximum
        .is_some_and(|m| crate::csharp::arith::sub(v.value_without_reduction, reduction) > m)
    {
        return unsupported("a damage reduced to its maximum (DamageReduced's string.Format)");
    }
    sb.push('\n');
    let mut inner = String::new();
    append_sources(w, &mut inner, d, index, v, not_only_dice, crit_to_bonus, st)?;
    append_indented(sb, &inner);
    sb.push('\n');
    Ok(())
}

/// `AppendMultipliers(sb, rule, damage)`: whether any was written. The sim's damage rules
/// are never halved, by a saving throw or otherwise (`RuleDealDamage.Half`, `HalfBecauseSavingThrow`
/// unset), and have no `ModifierBonus`.
fn append_multipliers(sb: &mut String, v: &DamageValue, st: &mut DamageState) -> Res<bool> {
    let s = &v.source;
    let mut wrote = false;
    if s.half {
        append_multiplier(sb, 0.5)?;
        wrote = true;
    }
    if s.empower > 1.0 {
        append_multiplier(sb, s.empower)?;
        wrote = true;
    }
    if s.bonus_percent != 0 {
        // `(1f + ModifierBonus) * (1f + (float)BonusPercent / 100f)`, stored to a float.
        #[allow(clippy::cast_precision_loss)]
        let m = crate::csharp::arith::store_f32(1.0 + f64::from(s.bonus_percent as f32) / 100.0);
        if m != 1.0 {
            st.bonus_percent = Some(m);
            append_multiplier(sb, m)?;
            wrote = true;
        }
    }
    if s.vulnerability > 1.0 {
        append_multiplier(sb, s.vulnerability)?;
        sb.push(' ');
        wrote = true;
    }
    if s.durability < 1.0 {
        append_multiplier(sb, s.durability)?;
        sb.push(' ');
        wrote = true;
    }
    let decline = s.decline();
    if decline != 0 && decline < 3 {
        return unsupported(
            "a declined damage's tooltip (DamageInvulnerable's string.Format, its source)",
        );
    }
    Ok(wrote)
}

/// `AppendDamageType(sb, damage)`.
fn append_damage_type(w: &World, sb: &mut String, s: &BaseDamage) -> Res<()> {
    match s.type_desc.ty {
        0 => {
            let mut first = true;
            for f in [1, 2, 4] {
                if s.type_desc.form & f != 0 {
                    if !first {
                        sb.push_str(", ");
                    }
                    sb.push_str(&enum_text(w, "DamageForm", f)?);
                    first = false;
                }
            }
            sb.push(' ');
        }
        t => return unsupported(format!("a damage's type name (DamageType {t})")),
    }
    Ok(())
}

/// `AppendSources(sb, rule, damage, hasNotOnlyDice, hasCrit, hasPostCritBonus: false)`.
#[allow(clippy::too_many_arguments)]
fn append_sources(
    w: &World,
    sb: &mut String,
    d: &DamageRule,
    index: usize,
    v: &DamageValue,
    not_only_dice: bool,
    crit_to_bonus: bool,
    st: &mut DamageState,
) -> Res<()> {
    let s = &v.source;
    // `Source.SourceFact` (none on the damage the sim makes), else the bundle's weapon for its
    // weapon damage.
    let weapon = if index == 0 && d.first_is_weapon_damage { d.weapon } else { None };
    if let Some(item) = weapon {
        let t = provider_text(w, Some(Provider::Item(item)))?;
        if !t.is_empty() {
            sb.push_str(&text_of(w, "UserInterfacesText/Tooltips/Source")?);
            sb.push_str(": ");
            sb.push_str(&t);
            sb.push('\n');
        }
    }
    let modified = s.dice.modified();
    if modified.dice != 0 && modified.rolls() > 0 && (s.dice.has_modifications() || not_only_dice) {
        append_dice(sb, modified);
        sb.push('\n');
        let mut inner = String::new();
        // `AppendDiceModifiersBreakdown`.
        if s.dice.has_modifications() {
            append_dice_modifier(
                &mut inner,
                s.dice.base,
                &text_of(w, "UserInterfacesText/Tooltips/BaseValue")?,
            );
            for m in &s.dice.modifications {
                let src = if m.critical {
                    text_of(w, "UserInterfacesText/CombatTexts/CriticalHit")?
                } else if m.descriptor != MD::None {
                    descriptor_name(w, m.descriptor)?
                } else {
                    String::new()
                };
                append_dice_modifier(&mut inner, m.formula, &src);
            }
        }
        if not_only_dice {
            inner.push_str(&text_of(w, "UserInterfacesText/SkillcheckTooltips/RollResult")?);
            inner.push_str(": ");
            inner.push_str(&v.roll_result.to_string());
            inner.push('\n');
        }
        append_indented(sb, &inner);
    }
    let total = crate::csharp::arith::add(s.bonus, s.bonus_target_related);
    if total != 0 && !s.modifiers.0.is_empty() {
        sb.push('+');
        sb.push_str(&total.to_string());
        sb.push('\n');
        let sum = s.modifiers.0.iter().fold(0i32, |a, m| crate::csharp::arith::add(a, m.value));
        let mut b = Breakdown::default();
        if sum < total {
            let base = text_of(w, "UserInterfacesText/Tooltips/BaseValue")?;
            b.add(crate::csharp::arith::sub(total, sum), Some(base), MD::None, false);
        }
        b.add_rule_bonuses(w, &s.modifiers.0)?;
        let mut inner = String::new();
        b.append(w, &mut inner)?;
        append_indented(sb, &inner);
    }
    if crit_to_bonus {
        // `AdditionalCriticalMultiplier`: the weapon stats' (the sim's is always 0).
        #[allow(clippy::cast_precision_loss)]
        append_multiplier(sb, s.critical_modifier.unwrap_or(1) as f32)?;
        sb.push_str(": ");
        sb.push_str(&text_of(w, "UserInterfacesText/CombatTexts/CriticalHit")?);
        sb.push('\n');
    } else if s.critical_modifier.is_some() {
        return unsupported(
            "a critical hit not applied to the bonus (EnemyCriticalHits' setting text)",
        );
    }
    if s.empower > 1.0 {
        return unsupported("an empowered damage's tooltip (its source)");
    }
    if let Some(m) = st.bonus_percent {
        append_multiplier(sb, m)?;
        sb.push_str(": ");
        sb.push('\n');
    }
    if s.vulnerability > 1.0 {
        append_multiplier(sb, s.vulnerability)?;
        sb.push_str(": ");
        sb.push_str(&text_of(w, "GameLog/Damage/DamageVulnerability")?);
        sb.push('\n');
    }
    if s.durability < 1.0 {
        append_multiplier(sb, s.durability)?;
        sb.push_str(": ");
        sb.push_str(&text_of(w, "GameLog/Damage/DamageDurability")?);
        sb.push('\n');
    }
    if v.reduction() <= 0 {
        return Ok(());
    }
    unsupported("a reduced damage's tooltip (its reductions' sources)")
}

/// `AppendDiceModifier(sb, dice, bonusSource)`.
fn append_dice_modifier(sb: &mut String, f: DiceFormula, source: &str) {
    if !blank(Some(source)) {
        sb.push_str(source);
        sb.push_str(": ");
    }
    sb.push_str("-> ");
    sb.push_str(&dice_text(f));
    sb.push_str("</color>");
    sb.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_print_as_mono_g7() {
        for (v, s) in [
            (0.8f32, "0.8"),
            (2.0, "2"),
            (1.0, "1"),
            (0.5, "0.5"),
            (0.25, "0.25"),
            (1.1, "1.1"),
            (1.5, "1.5"),
            (1.32, "1.32"),
            (3.0, "3"),
            (0.2, "0.2"),
            (12.5, "12.5"),
            (1_234_567.0, "1234567"),
            (0.000_12, "0.00012"),
            (-0.75, "-0.75"),
            (1.007_812_5, "1.007813"),
            (1.999_999_9, "2"),
            (0.000_1, "0.0001"),
        ] {
            assert_eq!(float_text(v).unwrap(), s, "{v}");
        }
        assert!(float_text(12_345_678.0).is_err());
        assert!(float_text(0.000_01).is_err(), "Mono writes 1E-05");
    }

    #[test]
    fn signs_use_the_en_dash() {
        assert_eq!(add_sign(3), "+3");
        assert_eq!(add_sign(0), "+0");
        assert_eq!(add_sign(-2), "\u{2013}2");
        assert_eq!(add_sign(i32::MIN), "\u{2013}2147483648");
    }

    #[test]
    fn indented_blocks_skip_blank_lines() {
        let mut s = String::new();
        append_indented(&mut s, "a\n\n  \nb\n\n");
        assert_eq!(s, "    a\n    b\n");
        let mut s = String::new();
        append_indented(&mut s, "");
        assert_eq!(s, "");
    }

    #[test]
    fn descriptors_sort_in_the_comparers_order() {
        assert_eq!(descriptor_order(MD::None), Some(0));
        assert_eq!(descriptor_order(MD::Prone), Some(44));
        assert_eq!(descriptor_order(MD::Alchemical), Some(45));
        assert_eq!(descriptor_order(MD::SelfBonus), Some(57));
        assert_eq!(descriptor_order(MD(99)), None);
        assert!(descriptor_order(MD::Size) < descriptor_order(MD::DexterityBonus));
    }

    #[test]
    fn dice_print_as_the_game_does() {
        assert_eq!(dice_text(DiceFormula { rolls: 1, dice: 8 }), "1d8");
        assert_eq!(dice_text(DiceFormula::ONE), "1");
        assert_eq!(dice_text(DiceFormula::ZERO), "0");
        let mut s = String::new();
        append_dice(&mut s, DiceFormula { rolls: 2, dice: 6 });
        append_dice(&mut s, DiceFormula { rolls: 3, dice: 1 });
        assert_eq!(s, "2d63");
    }
}
