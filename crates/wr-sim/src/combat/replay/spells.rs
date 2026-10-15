//! The driven replay's side of spellcasting ([`crate::combat::spells`]): a cast's root rules.
//!
//! `UnitUseAbility.OnAction` (class code, recorded) constructs a `RuleCastSpell`, whose
//! `CreateExecutionContext` triggers `RuleCalculateAbilityParams` as a root, then triggers the
//! cast: the two roots are the sim's cast, fired at their place in the commands' phase for the
//! running command's spell (its fields checked against the recorded snapshots). The process the
//! cast starts runs in `AbilityExecutionController`'s phase ([`Phase::Abilities`]): the roots it
//! records there (`RuleSpellResistanceCheck`, `RuleRollDice`, `RuleCanApplyBuff`,
//! `RuleApplySpell`) are the sim's own, compared through the T0 stream.

use serde_json::Value as J;

use super::{Driver, Phase, unit_id};
use crate::combat::commands::class;
use crate::world::World;

/// `RuleCastSpell`'s constructor in `UnitUseAbility.OnAction`.
const PARAMS: &str = "Kingmaker.UnitLogic.Abilities.AbilityData.CalculateParams < Kingmaker.UnitLogic.Abilities.AbilityData.CreateExecutionContext < Kingmaker.RuleSystem.Rules.Abilities.RuleCastSpell..ctor < Kingmaker.UnitLogic.Commands.UnitUseAbility.OnAction < Kingmaker.UnitLogic.Commands.Base.UnitCommand.Tick < Kingmaker.Controllers.Units.UnitCommandController.";
/// `Rulebook.Trigger(ruleCastSpell)` in `UnitUseAbility.OnAction`.
const CAST: &str = "Kingmaker.UnitLogic.Commands.UnitUseAbility.OnAction < Kingmaker.UnitLogic.Commands.Base.UnitCommand.Tick < Kingmaker.Controllers.Units.UnitCommandController.";
/// An ability process's tick.
const PROCESS: &str = " < Kingmaker.Controllers.AbilityExecutionController.Tick < Kingmaker.GameModes.GameMode.Tick < ";

impl Driver<'_> {
    /// A root rule of a cast (record `i`, its end `end`), handled; `None` for any other root.
    pub(super) fn spell_root(
        &mut self,
        w: &mut World,
        i: usize,
        end: usize,
        class: &str,
    ) -> Option<usize> {
        let r = &self.trace.records[i];
        let origin = r["origin"].as_str().unwrap_or("");
        if class == "RuleCalculateAbilityParams" && origin.starts_with(PARAMS) {
            self.place(w, Phase::Commands, "a cast's RuleCalculateAbilityParams");
            if let Err(e) = self.begin_cast(w, r) {
                w.coverage.unsupported(format!("driven replay: a cast's ability params: {e}"));
                *self.out.skipped.entry(class.to_owned()).or_default() += 1;
            }
            return Some(end + 1);
        }
        if class == "RuleCastSpell" && origin.starts_with(CAST) {
            self.place(w, Phase::Commands, "a RuleCastSpell");
            if let Err(e) = self.cast(w, r) {
                w.coverage.unsupported(format!("driven replay: a RuleCastSpell: {e}"));
                *self.out.skipped.entry(class.to_owned()).or_default() += 1;
            }
            return Some(end + 1);
        }
        let own = matches!(
            class,
            "RuleSpellResistanceCheck" | "RuleRollDice" | "RuleCanApplyBuff" | "RuleApplySpell"
        );
        if own && origin.contains(PROCESS) {
            self.place(w, Phase::Abilities, &format!("an ability process's {class}"));
            if w.combat.spells.ran_at != Some(self.tick) {
                w.coverage.unsupported(format!(
                    "driven replay: an ability process's {class} the sim's processes didn't make"
                ));
            }
            *self.out.by_origin.entry("ability process").or_default() += 1;
            return Some(end + 1);
        }
        if matches!(class, "RuleCastSpell" | "RuleApplySpell" | "RuleSpellResistanceCheck")
            || origin.contains(PROCESS)
        {
            w.coverage.unsupported(format!("driven replay: a {class} from a caller not modelled"));
            *self.out.skipped.entry(class.to_owned()).or_default() += 1;
            return Some(end + 1);
        }
        None
    }

    /// `insp_reveal` (record `i`, WrathOracle's `inspect_reveal`): `InspectUnitsController.
    /// HandleEntityRevealed`, placed by its caller (fog of war, a view's visibility); for a unit the
    /// sim has, the sim's `HandleEntityRevealed` (queued for the controller's next tick, its
    /// knowledge check, without an inspection info of its blueprint), checked against the
    /// record's `added`.
    pub(super) fn inspect_reveal(&mut self, w: &mut World, i: usize) {
        let r = &self.trace.records[i];
        let declared = self.trace.records.iter().find(|x| x["k"] == "begin").is_some_and(|b| {
            b["observers"].as_array().is_some_and(|a| a.iter().any(|o| o == "inspect_reveal"))
        });
        if !declared {
            w.coverage.unsupported(
                "driven replay: an insp_reveal record without the inspect_reveal observer",
            );
            return;
        }
        let keys = ["seq", "k", "t", "ev", "e", "unit", "added", "origin"];
        let shaped = r
            .as_object()
            .is_some_and(|o| o.len() == keys.len() && keys.iter().all(|k| o.contains_key(*k)))
            && (r["e"].is_string() || r["e"].is_null())
            && r["unit"].is_boolean()
            && (r["added"].is_boolean() || r["added"].is_null())
            && r["origin"].is_string();
        if !shaped || r["t"].as_i64() != Some(self.tick) {
            w.coverage.unsupported("driven replay: an insp_reveal record it can't read");
            return;
        }
        match r["origin"].as_str().and_then(super::commands::phase_of) {
            Some(p) => self.place(w, p, "an entity's reveal"),
            None => {
                w.coverage
                    .unsupported("driven replay: an entity's reveal from a caller not modelled");
                return;
            }
        }
        let u = r["e"]
            .as_str()
            .and_then(|id| w.by_id.get(id).copied())
            .filter(|u| w.unit(*u).is_some());
        if r["unit"] != true {
            // Not a unit: nothing is queued (`added` null), and the sim has no unit of its id.
            if !r["added"].is_null() || u.is_some() {
                w.coverage.unsupported("driven replay: an insp_reveal record it can't read");
            }
            return;
        }
        let Some(added) = r["added"].as_bool() else {
            w.coverage.unsupported("driven replay: a reveal whose queuing wasn't read");
            return;
        };
        let Some(u) = u else {
            w.coverage.unsupported("driven replay: a revealed unit the sim doesn't have");
            return;
        };
        if crate::combat::inspect::entity_revealed(w, u).is_some_and(|sim| sim != added) {
            w.coverage.unsupported("driven replay: a reveal's queuing differs from the sim's");
        }
    }

    /// The running `UnitUseAbility` of the root's initiator (the command whose `OnAction` this
    /// is): its id.
    fn casting_command(&self, w: &World, r: &J) -> Result<u32, String> {
        let u = unit_id(&r["initiator"]).ok_or("no initiator")?;
        let m = &w.combat.commands;
        let cmds: Vec<u32> = m
            .all_of(u)
            .into_iter()
            .filter(|c| m.get(*c).is_some_and(|x| x.class == class::USE_ABILITY && x.s.running()))
            .collect();
        match cmds[..] {
            [c] => Ok(c),
            _ => Err(format!("{} running UnitUseAbility commands of the caster", cmds.len())),
        }
    }

    /// `RuleCalculateAbilityParams` of `new RuleCastSpell(Ability, Target)`: its snapshot names
    /// the command's spell (blueprint, spellbook, level), checked; the sim's cast begins.
    fn begin_cast(&mut self, w: &mut World, r: &J) -> Result<(), String> {
        if w.combat.spells.pending.take().is_some() {
            w.coverage.unsupported("driven replay: a cast's context without its RuleCastSpell");
        }
        let c = self.casting_command(w, r)?;
        let x = w.combat.commands.get(c).ok_or("the command is unknown")?;
        let a = x.ability.clone().ok_or("the command's ability isn't established")?;
        let target =
            x.s.target
                .unit()
                .and_then(|t| w.by_id.get(t).copied())
                .ok_or("the command's target isn't a unit the sim has")?;
        let f = &r["fields"];
        let guid = a.blueprint().to_string();
        let (sb, level, spell_source) =
            (a.book("the cast's params")?.0, a.spell_level(w)?, a.spell_source(w)?);
        let sb = sb.to_string();
        let caster = &w.entity(a.caster).unique_id;
        let ok = f["Blueprint"].as_str() == Some(guid.as_str())
            && f["Spellbook"]["blueprint"].as_str() == Some(sb.as_str())
            && f["Spellbook"]["owner"].as_str() == Some(caster.as_str())
            && f["AbilityData"]["$ability"].as_str() == Some(guid.as_str())
            && f["AbilityData"]["caster"]["$unit"].as_str() == Some(caster.as_str())
            && f["m_SpellLevel"].as_i64() == Some(i64::from(level))
            && f["m_SpellSource"].as_i64() == Some(spell_source)
            && f["m_MetamagicData"].is_null()
            && f["IgnoreDCBonuses"] == false
            && ["ReplaceCasterLevel", "ReplaceSpellLevel", "ReplaceDC", "ReplaceConcentration"]
                .iter()
                .chain(&["ReplaceStatBonusModifier", "ReplaceStat"])
                .all(|k| f[*k].is_null());
        if !ok {
            return Err("its snapshot isn't the command's spell".into());
        }
        crate::combat::spells::begin_cast(w, &a, target).ok_or("the cast's context")?;
        *self.out.by_origin.entry("command (UnitUseAbility)").or_default() += 1;
        Ok(())
    }

    /// The pending cast's `RuleCastSpell`: its snapshot's spell, context and target the pending cast's.
    fn cast(&mut self, w: &mut World, r: &J) -> Result<(), String> {
        let i = w.combat.spells.pending.take().ok_or("no context before it")?;
        let ctx = w.combat.spells.context(i).cloned().ok_or("its context")?;
        let c = self.casting_command(w, r)?;
        let cutscene = w.combat.commands.get(c).is_some_and(|x| x.cutscene);
        let f = &r["fields"];
        let inputs =
            crate::combat::spells::spell_inputs(w, i, Some(ctx.target)).ok_or("its context")?;
        let ok = ["Spell", "Context", "SpellTarget"].iter().all(|k| f[*k] == inputs[*k])
            && f["IsCutscene"] == false
            && f["ForceFail"] == false
            && f["SpellFailureChance"] == 0;
        if !ok {
            return Err("its snapshot isn't the pending cast".into());
        }
        crate::combat::spells::cast(w, i, cutscene).ok_or("the cast")?;
        *self.out.by_origin.entry("command (UnitUseAbility)").or_default() += 1;
        Ok(())
    }
}
