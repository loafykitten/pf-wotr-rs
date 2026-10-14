//! `RuleIdentifyItem` and the player inventory's handlers (decompiled, not copied:
//! `Kingmaker.RuleSystem.Rules/RuleIdentifyItem.cs`, `Kingmaker.Items/ItemEntity.cs`
//! `TryIdentify(UnitReference, int)`, `Kingmaker.Items/ItemsCollection.cs`
//! `HandleLevelUpComplete`, `HandleRecruit`).
//!
//! The player's inventory is a global subscriber (`ItemsCollection.PostLoad` subscribes it
//! when `IsPlayerInventory`): whenever a unit finishes a level-up or a companion is recruited,
//! each item in it that is not identified gets a `RuleIdentifyItem` for that unit. The rule
//! rolls the unit's skill against the item's identify DC; a unit without a rank in the skill
//! (`BaseValue < 1`) does nothing, which is all the sim ports: a roll is reported.

use std::any::Any;
use std::rc::Rc;

use crate::combat::rules::rule_boilerplate;
use crate::rulebook::bus::{ExternalHandler, RuleStep, Sub};
use crate::rulebook::{RuleBase, RuleEvent};
use crate::stats::types::StatType;
use crate::world::{EntityIdx, World};

/// `RuleIdentifyItem`.
#[derive(Debug)]
pub struct RuleIdentifyItem {
    pub base: RuleBase,
    pub initiator: EntityIdx,
    pub item: EntityIdx,
    pub stat: StatType,
}

impl RuleIdentifyItem {
    /// `new RuleIdentifyItem(initiator, item)`: the stat is `IdentifySkillReplacement`'s, else
    /// Knowledge (Arcana).
    pub fn new(w: &mut World, initiator: EntityIdx, item: EntityIdx) -> Self {
        let stat = StatType::SkillKnowledgeArcana;
        let bp = w.item(item).and_then(|i| i.blueprint).and_then(|g| w.content.get(g));
        if let Some(bp) = bp
            && bp
                .components_array()
                .iter()
                .any(|o| crate::content::is_a(o.ty, "IdentifySkillReplacement"))
        {
            w.coverage.unsupported("RuleIdentifyItem: IdentifySkillReplacement");
        }
        Self { base: RuleBase::default(), initiator, item, stat }
    }
}

impl RuleEvent for RuleIdentifyItem {
    fn chain(&self) -> &'static [&'static str] {
        &["RuleIdentifyItem"]
    }

    fn initiator(&self) -> EntityIdx {
        self.initiator
    }

    fn on_trigger(&mut self, w: &mut World) {
        if w.item(self.item).is_none_or(|i| i.is_identified)
            || crate::combat::units::is_dead(w, self.initiator)
        {
            return;
        }
        let Some(stat) = w.stats(self.initiator).get(self.stat) else {
            w.log("error", format!("Can't identify item with stat '{:?}'", self.stat));
            return;
        };
        // `CanUseUntrainedStat` is false here: no rank, no attempt.
        if stat.base_value < 1 {
            return;
        }
        w.coverage.unsupported("RuleIdentifyItem: identify skill check");
    }

    rule_boilerplate!();
}

/// `ItemEntity.TryIdentify(character, 0)`.
pub fn try_identify(w: &mut World, unit: EntityIdx, item: EntityIdx) {
    if w.item(item).is_some_and(|i| i.is_identified) {
        return;
    }
    if w.unit(unit).is_none() || crate::combat::units::is_dead(w, unit) {
        return;
    }
    let mut rule = RuleIdentifyItem::new(w, unit, item);
    crate::combat::rules::trigger_any(w, &mut rule, |_| None);
}

/// The id of the player inventory's external handler.
const PLAYER_INVENTORY: u32 = 0x5049_4e56;

/// The player inventory as a subscriber (`ItemsCollection : ILevelUpCompleteUIHandler,
/// ICompanionChangeHandler`).
struct PlayerInventory;

impl ExternalHandler for PlayerInventory {
    fn interfaces(&self) -> Vec<String> {
        vec!["ILevelUpCompleteUIHandler".into(), "ICompanionChangeHandler".into()]
    }

    fn event(&self, w: &mut World, iface: &str) {
        let (method, unit) = w.bus.call.last().copied().unwrap_or(("", None));
        let handled = matches!(
            (iface, method),
            ("ILevelUpCompleteUIHandler", "HandleLevelUpComplete")
                | ("ICompanionChangeHandler", "HandleRecruit")
        );
        if !handled {
            // `HandleUnrecruit` does nothing; anything else isn't an event the sim raises.
            if method != "HandleUnrecruit" {
                w.coverage.unsupported(format!("ItemsCollection: {iface}.{method}"));
            }
            return;
        }
        let Some(unit) = unit else { return };
        let items = w
            .main_character
            .and_then(|m| w.unit(m))
            .map(|m| m.inventory.clone())
            .unwrap_or_default();
        for it in items {
            try_identify(w, unit, it);
        }
    }

    fn rule(&self, _w: &mut World, _rule: &mut dyn RuleEvent, _step: RuleStep) {}
}

/// `ItemsCollection.PostLoad()` of the player's inventory: `EventBus.Subscribe(this)`.
pub fn subscribe_player_inventory(w: &mut World) {
    if w.bus.external.contains_key(&PLAYER_INVENTORY) {
        return;
    }
    w.bus.external.insert(PLAYER_INVENTORY, Rc::new(PlayerInventory));
    crate::rulebook::bus::subscribe(w, Sub::External(PLAYER_INVENTORY));
}
