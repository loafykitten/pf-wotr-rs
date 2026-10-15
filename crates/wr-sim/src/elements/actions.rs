//! Actions (each cites its decompiled file under
//! `Kingmaker.Designers.EventConditionActionSystem.Actions/` unless said otherwise).

use std::rc::Rc;

use serde_json::json;

use super::{ElementError, Result, get_int, get_unit, run_actions, unsupported};
use crate::content::{Blueprint, View};
use crate::csharp::arith;
use crate::world::World;

pub const IMPLEMENTED: &[&str] = &[
    "Conditional",
    "GainExp",
    "GiveObjective",
    "SetObjectiveStatus",
    "StartEtude",
    "CompleteEtude",
    "ShowNewTutorial",
    "StartDialog",
    "Recruit",
    "PlayCutscene",
    "StopCutscene",
    "Play2DSound",
    "PlayAnimationOneShot",
    "UnlockFlag",
    "AddItemToPlayer",
    "AttachBuff",
    "Spawn",
    "HideUnit",
    "TranslocateUnit",
];

/// `GameAction.RunAction()`.
pub fn run(w: &mut World, owner: &Rc<Blueprint>, a: View<'_>) -> Result<()> {
    match a.type_name() {
        // `Conditional.cs`.
        "Conditional" => {
            if super::check_conditions(w, owner, a.obj("ConditionsChecker")) {
                run_actions(w, owner, a.obj("IfTrue"));
            } else {
                run_actions(w, owner, a.obj("IfFalse"));
            }
            Ok(())
        }
        // `GainExp.cs`.
        "GainExp" => {
            if a.bool("UseCustomBattlelogMessage") {
                w.coverage.stub("GainExp: custom battle-log message (UI)");
            }
            let exp = if a.bool("GainPureExp") {
                // `Mathf.RoundToInt(((float?)Count?.GetValue() * Modifier) ?? 0)`.
                match a.obj("Count") {
                    Some(c) => {
                        let n = get_int(w, owner, c)?;
                        let x = arith::store_f32(f64::from(n) * f64::from(a.f32("Modifier")));
                        arith::round_to_int(x)
                    }
                    None => 0,
                }
            } else {
                let count = match a.obj("Count") {
                    Some(c) => Some(get_int(w, owner, c)?),
                    None => None,
                };
                crate::xp::get_xp(w, a.i32("Encounter"), a.i32("CR"), a.f32("Modifier"), count)
            };
            crate::xp::gain_experience(w, exp);
            Ok(())
        }
        // `GiveObjective.cs`.
        "GiveObjective" => {
            let Some(o) = a.guid("m_Objective") else {
                return Err(ElementError::Other("NullReferenceException: objective".into()));
            };
            crate::narrative::quests::give_objective(w, o);
            Ok(())
        }
        // `SetObjectiveStatus.cs` (`SummonPoolCountTrigger.ObjectiveStatus`: 0 Complete,
        // 1 Fail).
        "SetObjectiveStatus" => {
            let Some(o) = a.guid("m_Objective") else {
                return Err(ElementError::Other("NullReferenceException: objective".into()));
            };
            if a.bool("StartObjectiveIfNone")
                && crate::narrative::quests::get_objective_state(w, o) == 0
            {
                crate::narrative::quests::give_objective(w, o);
            }
            if crate::narrative::quests::get_objective_state(w, o) != 0 {
                match a.int("Status") {
                    0 => crate::narrative::quests::complete_objective(w, o),
                    1 => crate::narrative::quests::fail_objective(w, o),
                    _ => {}
                }
            }
            Ok(())
        }
        // `StartEtude.cs`: refuses etudes that don't allow action starts.
        "StartEtude" => {
            if a.bool("Evaluate") {
                return Err(unsupported(w, "action", "StartEtude (EtudeEvaluator)"));
            }
            let Some(g) = a.guid("Etude") else {
                return Err(ElementError::Other("NullReferenceException: etude".into()));
            };
            let allow = w.content.get(g).is_some_and(|b| b.view().bool("m_AllowActionStart"));
            if allow {
                crate::narrative::etudes::start_etude(w, g, false, false);
            } else {
                w.log("error", "Etude disallows start action");
            }
            Ok(())
        }
        // `CompleteEtude.cs`.
        "CompleteEtude" => {
            if a.bool("Evaluate") {
                return Err(unsupported(w, "action", "CompleteEtude (EtudeEvaluator)"));
            }
            let Some(g) = a.guid("Etude") else {
                return Err(ElementError::Other("NullReferenceException: etude".into()));
            };
            crate::narrative::etudes::mark_etude_completed(w, g, false, true);
            Ok(())
        }
        // `Kingmaker.Tutorial.Actions/ShowNewTutorial.cs`.
        "ShowNewTutorial" => {
            if !a.objs("Evaluators").is_empty() {
                return Err(unsupported(w, "action", "ShowNewTutorial (context evaluators)"));
            }
            let Some(g) = a.guid("m_Tutorial") else {
                return Err(ElementError::Other("NullReferenceException: tutorial".into()));
            };
            crate::narrative::tutorial::show_new_tutorial(w, g);
            Ok(())
        }
        // `StartDialog.cs`.
        "StartDialog" => {
            let dialog = match a.guid("m_Dialogue") {
                Some(d) => Some(d),
                None if a.obj("DialogEvaluator").is_some() => {
                    return Err(unsupported(w, "action", "StartDialog (DialogEvaluator)"));
                }
                None => None,
            };
            if let Some(owner_ev) = a.obj("DialogueOwner") {
                let u = get_unit(w, owner, Some(owner_ev))?;
                // The owner's blueprint `StartDialog` component overrides the dialogue.
                let comp =
                    w.unit(u).and_then(|x| x.blueprint).and_then(|g| w.content.get(g)).is_some_and(
                        |b| {
                            b.components_array()
                                .iter()
                                .any(|o| crate::content::is_a(o.ty, "StartDialog"))
                        },
                    );
                if comp {
                    return Err(unsupported(
                        w,
                        "action",
                        "StartDialog (owner's StartDialog component)",
                    ));
                }
                if let Some(d) = dialog {
                    // `StartDialogWithUnit`: no speaker name (`"<null>"`).
                    crate::dialog::start_dialog(w, d, None, Some(u), Ok("<null>".to_owned()))
                        .map_err(|e| ElementError::Other(e.to_string()))?;
                }
            } else if let Some(d) = dialog {
                // `StartDialogWithoutTarget(dialog, SpeakerName)`: the name becomes a string
                // as the start data is built (after the request), so the game's exception
                // there stops the start.
                let name = crate::text::localized_field(w, a, "SpeakerName");
                match crate::dialog::start_dialog(w, d, None, None, name) {
                    Ok(()) => {}
                    Err(wr_text::TextError::Throws(e)) => {
                        return Err(ElementError::Other(format!("StartDialog: speaker name: {e}")));
                    }
                    Err(e) => {
                        return Err(unsupported(
                            w,
                            "action",
                            &format!("StartDialog (speaker name: {e})"),
                        ));
                    }
                }
            }
            Ok(())
        }
        // `UnlockFlag.cs`: `Unlock()` if locked, then `Value = flagValue`.
        "UnlockFlag" => {
            let Some(f) = a.guid("m_flag") else {
                return Err(ElementError::Other("NullReferenceException: flag".into()));
            };
            if !crate::narrative::flags::is_unlocked(w, f) {
                crate::narrative::flags::unlock(w, f);
            }
            crate::narrative::flags::set_value(w, f, a.i32("flagValue"));
            Ok(())
        }
        // `AddItemToPlayer.cs`.
        "AddItemToPlayer" => crate::items::add_item_to_player(w, a),
        // `AttachBuff.cs`: `GameHelper.ApplyBuff(target, buff, duration)`; the unit is its
        // own caster.
        "AttachBuff" => {
            let target = get_unit(w, owner, a.obj("Target"))?;
            if a.obj("Duration").is_some() {
                return Err(unsupported(w, "action", "AttachBuff (duration)"));
            }
            // `target.MarkNotOptimizableInSave()`: a save-size flag.
            let Some(bp) = a.guid("m_Buff").and_then(|g| w.content.get(g)) else {
                // `UnitHelper.AddBuff` returns null for a missing blueprint; the battle log
                // then reports a null buff.
                return Ok(());
            };
            let ctx = crate::unit_facts::context(w, Some(target), Some(target), &bp);
            let buff = crate::unit_facts::add_buff(w, target, &bp, ctx, None);
            if a.bool("BattleLog") {
                crate::rulebook::bus::raise_global_fact(
                    w,
                    "ISpecialFeatureChangeHandler",
                    "HandleSpecialFeatureAdded",
                    Some(target),
                    buff,
                );
            }
            Ok(())
        }
        // `Spawn.cs`, `HideUnit.cs`.
        "Spawn" => crate::scene_spawn::spawn_action(w, owner, a),
        "HideUnit" => crate::scene_spawn::hide_unit(w, owner, a),
        "TranslocateUnit" => crate::scene_spawn::translocate_unit(w, owner, a),
        // `Recruit.cs`.
        "Recruit" => crate::dialog::recruit::run(w, owner, a),
        // `PlayCutscene.cs` / `StopCutscene.cs`: cutscenes are T1 (see `cutscene`).
        "PlayCutscene" => super::cutscene::play(w, owner, a),
        "StopCutscene" => super::cutscene::stop(w, owner, a),
        // `Play2DSound.cs`: a Wwise event (presentation).
        "Play2DSound" => {
            w.coverage.stub("Play2DSound (audio)");
            w.events.push("stub", json!({ "what": "sound2d", "sound": a.str("SoundName") }));
            Ok(())
        }
        // `PlayAnimationOneShot.cs`: evaluates its unit (throws if it can't), then plays an
        // animation clip on the view (presentation).
        "PlayAnimationOneShot" => {
            let u = get_unit(w, owner, a.obj("Unit"))?;
            w.coverage.stub("PlayAnimationOneShot (animation)");
            w.events.push(
                "t1",
                json!({ "what": "animation_one_shot", "unit": crate::events::unit_ref(&w.entity(u).unique_id) }),
            );
            Ok(())
        }
        // A `ContextAction` reads the innermost `MechanicsContext.Data` scope (the fact's
        // context, its current target); the ones with a port are in `run_context_action`.
        other => {
            let Some((fact, target)) = mechanics_scope(w).filter(|_| a.is("ContextAction")) else {
                return Err(unsupported(w, "action", other));
            };
            let caster = w
                .fact(fact)
                .context
                .as_ref()
                .and_then(|x| x.caster.clone())
                .and_then(|id| w.by_id.get(&id).copied());
            crate::components::mechanics::run_context_action(w, a, target, caster);
            Ok(())
        }
    }
}

/// The innermost `MechanicsContext.Data` scope.
fn mechanics_scope(w: &World) -> Option<(crate::world::FactIdx, Option<crate::world::EntityIdx>)> {
    w.ctx.iter().rev().find_map(|c| match c {
        super::Ctx::Mechanics { fact, target } => Some((*fact, *target)),
        _ => None,
    })
}
