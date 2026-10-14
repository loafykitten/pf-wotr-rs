//! Components with rulebook handlers on armor rules. Source (decompiled, not copied):
//! `Kingmaker.Designers.Mechanics.Facts/AdvanceArmorStats.cs`,
//! `Kingmaker.UnitLogic.FactLogic/AddBackgroundArmorProficiency.cs`.

use super::{bits, context_value, owner_unit, state::multiset, with_settings};
use crate::armor::{RuleCalculateArmorCheckPenalty, RuleCalculateArmorMaxDexBonusLimit};
use crate::registry::{ComponentLogic, Hook, Registry};
use crate::rulebook::RuleEvent;
use crate::world::{CompRef, World};

pub fn register(r: &mut Registry) {
    r.register(Box::new(AdvanceArmorStats));
    r.register(Box::new(AddBackgroundArmorProficiency));
}

/// `AddBackgroundArmorProficiency`: the proficiency joins the unit's armor proficiencies and
/// its background set while on; a background proficiency the unit also has from another
/// source adds `StackBonus` to the armor check penalty.
struct AddBackgroundArmorProficiency;

impl ComponentLogic for AddBackgroundArmorProficiency {
    fn type_name(&self) -> &'static str {
        "AddBackgroundArmorProficiency"
    }

    fn implemented(&self) -> u32 {
        bits(&[Hook::TurnOn, Hook::TurnOff])
    }

    fn handlers(&self) -> &'static [&'static str] {
        &["IInitiatorRulebookHandler<RuleCalculateArmorCheckPenalty>"]
    }

    fn hook(&self, w: &mut World, c: CompRef, h: Hook) {
        let Some(u) = owner_unit(w, c) else { return };
        let Some(p) = with_settings(w, c, |v| v.int("Proficiency")) else { return };
        let add = match h {
            Hook::TurnOn => true,
            Hook::TurnOff => false,
            _ => return,
        };
        let unit = w.unit_mut(u).expect("unit");
        multiset(&mut unit.proficiencies.armor, p, add);
        multiset(&mut unit.proficiencies.background_armor, p, add);
    }

    fn rule(&self, w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
        if !about {
            return;
        }
        let Some(u) = owner_unit(w, c) else { return };
        let Some(p) = with_settings(w, c, |v| v.int("Proficiency")) else { return };
        // `Proficiencies.IsBackgroundStacks(Proficiency)`.
        let prof = &w.unit(u).expect("unit").proficiencies;
        let stacks = prof.background_armor.iter().any(|(k, _)| *k == p)
            && prof.armor.iter().find(|(k, _)| *k == p).is_some_and(|(_, n)| *n > 1);
        if !stacks {
            return;
        }
        let bonus = {
            let Some((bp, i)) = super::settings_of(w, c) else { return };
            let objs = bp.components_array();
            context_value(w, c, crate::content::View(objs[i]).obj("StackBonus"))
        };
        if let Some(r) = rule.as_any().downcast_mut::<RuleCalculateArmorCheckPenalty>() {
            r.bonus = crate::csharp::arith::add(r.bonus, bonus);
        }
    }
}

/// `AdvanceArmorStats`: shifts its own armor's check penalty and max-Dex limit (and arcane
/// spell failure, whose rule isn't ported: that handler reports when it fires).
struct AdvanceArmorStats;

impl ComponentLogic for AdvanceArmorStats {
    fn type_name(&self) -> &'static str {
        "AdvanceArmorStats"
    }

    fn implemented(&self) -> u32 {
        0
    }

    fn handlers(&self) -> &'static [&'static str] {
        &[
            "IInitiatorRulebookHandler<RuleCalculateArmorCheckPenalty>",
            "IInitiatorRulebookHandler<RuleCalculateArmorMaxDexBonusLimit>",
        ]
    }

    fn hook(&self, _w: &mut World, _c: CompRef, _h: Hook) {}

    fn rule(&self, w: &mut World, c: CompRef, rule: &mut dyn RuleEvent, about: bool) {
        if !about {
            return;
        }
        // `Owner`: the item the enchantment is on.
        let owner = w.fact(c.fact).owner;
        let Some((acp, dex)) =
            with_settings(w, c, |v| (v.i32("ArmorCheckPenaltyShift"), v.i32("MaxDexBonusShift")))
        else {
            return;
        };
        let any = rule.as_any();
        if let Some(r) = any.downcast_mut::<RuleCalculateArmorCheckPenalty>() {
            if Some(r.armor) == owner {
                r.bonus = crate::csharp::arith::add(r.bonus, acp);
            }
        } else if let Some(r) = any.downcast_mut::<RuleCalculateArmorMaxDexBonusLimit>() {
            if Some(r.armor) == owner {
                r.bonus = crate::csharp::arith::add(r.bonus, dex);
            }
        } else {
            w.coverage.unsupported("AdvanceArmorStats: rule not ported");
        }
    }
}
