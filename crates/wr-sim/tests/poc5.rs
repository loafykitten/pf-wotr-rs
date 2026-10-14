//! POC-5's driven replay against WrathOracle's turn-based recordings (local captures,
//! gitignored): `poc5-tb-51`, `-52` and `-53` (WrathOracle `8fa7cc6`, each a different pair of
//! fights), `-55` (the turn port's records) and `-57` (root rules' call sites), made on
//! Manual_18. Ignored by default (needs the game's blueprint pack, the fixture
//! save and the captures; skipped when a capture is absent).
//! `cargo nextest run -p wr-sim --run-ignored only`.

use std::path::PathBuf;

use serde_json::json;
use wr_sim::combat::{oracle::Trace, replay};

const SAVE: &str = "Manual_18_WR_FirstBattle";

/// The captures made on the current WrathOracle: their fights differ (the clocks of the runs
/// that recorded them differ), and each has a record and two replays. `poc5-tb-55` and `-57`
/// have the oracle's group, leave and turn-choice records, so the turn port runs on them; the
/// older ones replay without it. `poc5-tb-60` also has root rules' call sites (`origin`) and a
/// root flat-footed check's environment, so no root is echoed and its runs are valid.
/// `poc5-tb-63` also has the oracle's cooldown records: the sim computes the units' cooldowns and
/// when turns start. `poc5-tb-68` also has the turn controller's tick records: the sim computes
/// when turns end. `poc5-tb-74` also has the command records: the sim runs the units' commands.
/// `poc5-tb-77` also has the `attacks` observer: `UnitAttack`'s class code is the sim's.
/// `poc5-tb-80` also has the `approach` observer: `TickApproaching`, `UnitMoveTo`'s and the attack
/// of opportunity's class code are the sim's. `poc5-tb-82` also has the `ai` observer: the AI is
/// the sim's. `poc5-tb-85` also has the `engage` observer: the engagement controller is the sim's.
/// `poc5-tb-92` also has the `targets` observer: units' `LastTarget` and `ManualTarget` are the sim's.
/// `poc5-tb-95` also has the `impatience` observer: the impatience controller is the sim's.
/// `poc5-tb-96` is a third fight (`tb-hold`: the focus reversed, the archer ending its turns).
const CAPTURES: [&str; 17] = [
    "poc5-tb-51",
    "poc5-tb-52",
    "poc5-tb-53",
    "poc5-tb-55",
    "poc5-tb-60",
    "poc5-tb-63",
    "poc5-tb-68",
    "poc5-tb-74",
    "poc5-tb-77",
    "poc5-tb-80",
    "poc5-tb-81",
    "poc5-tb-82",
    "poc5-tb-85",
    "poc5-tb-91",
    "poc5-tb-92",
    "poc5-tb-95",
    "poc5-tb-96",
];

/// The captures whose root rules carry their call sites.
const WITH_ORIGINS: [&str; 13] = [
    "poc5-tb-60",
    "poc5-tb-63",
    "poc5-tb-68",
    "poc5-tb-74",
    "poc5-tb-77",
    "poc5-tb-80",
    "poc5-tb-81",
    "poc5-tb-82",
    "poc5-tb-85",
    "poc5-tb-91",
    "poc5-tb-92",
    "poc5-tb-95",
    "poc5-tb-96",
];

/// The captures with WrathOracle's cooldown records (`cd_*`).
const WITH_COOLDOWNS: [&str; 12] = [
    "poc5-tb-63",
    "poc5-tb-68",
    "poc5-tb-74",
    "poc5-tb-77",
    "poc5-tb-80",
    "poc5-tb-81",
    "poc5-tb-82",
    "poc5-tb-85",
    "poc5-tb-91",
    "poc5-tb-92",
    "poc5-tb-95",
    "poc5-tb-96",
];

/// The captures with WrathOracle's turn controller tick records (`tt*`).
const WITH_TURN_TICKS: [&str; 11] = [
    "poc5-tb-68",
    "poc5-tb-74",
    "poc5-tb-77",
    "poc5-tb-80",
    "poc5-tb-81",
    "poc5-tb-82",
    "poc5-tb-85",
    "poc5-tb-91",
    "poc5-tb-92",
    "poc5-tb-95",
    "poc5-tb-96",
];

/// The captures with WrathOracle's command records (`uc_*`).
const WITH_COMMANDS: [&str; 10] = [
    "poc5-tb-74",
    "poc5-tb-77",
    "poc5-tb-80",
    "poc5-tb-81",
    "poc5-tb-82",
    "poc5-tb-85",
    "poc5-tb-91",
    "poc5-tb-92",
    "poc5-tb-95",
    "poc5-tb-96",
];

/// The captures with WrathOracle's `attacks` observer (`ua_atk`, `UnitAttack`'s leaves).
const WITH_ATTACKS: [&str; 9] = [
    "poc5-tb-77",
    "poc5-tb-80",
    "poc5-tb-81",
    "poc5-tb-82",
    "poc5-tb-85",
    "poc5-tb-91",
    "poc5-tb-92",
    "poc5-tb-95",
    "poc5-tb-96",
];

/// The captures with WrathOracle's `approach` observer (`uc_call`, the approach's taps).
const WITH_APPROACH: [&str; 8] = [
    "poc5-tb-80",
    "poc5-tb-81",
    "poc5-tb-82",
    "poc5-tb-85",
    "poc5-tb-91",
    "poc5-tb-92",
    "poc5-tb-95",
    "poc5-tb-96",
];

/// The captures with WrathOracle's `ai` observer (`uc_ai`, the AI's taps and checks).
const WITH_AI: [&str; 6] =
    ["poc5-tb-82", "poc5-tb-85", "poc5-tb-91", "poc5-tb-92", "poc5-tb-95", "poc5-tb-96"];

/// The captures with WrathOracle's `engage` observer (`uc_eng`, the engagement's taps and checks).
const WITH_ENGAGE: [&str; 5] =
    ["poc5-tb-85", "poc5-tb-91", "poc5-tb-92", "poc5-tb-95", "poc5-tb-96"];

/// The captures with WrathOracle's `targets` observer (`tgt_init`, `tgt_w`, `UpdateCombatTarget`'s
/// leaves).
const WITH_TARGETS: [&str; 3] = ["poc5-tb-92", "poc5-tb-95", "poc5-tb-96"];

/// The captures with WrathOracle's `impatience` observer (`imp_init`, `uc_imp`, `imp_s`, its leaves).
const WITH_IMPATIENCE: [&str; 2] = ["poc5-tb-95", "poc5-tb-96"];

/// Each capture's fights (a capture has the ones its plan ran: `poc5-tb-96` only `tb-hold`).
const FIGHTS: [&str; 3] = ["tb-nearest", "tb-focus", "tb-hold"];

/// The capture the approach records are damaged in.
const APPROACH: &str = "poc5-tb-80";

/// The reports the turn port makes when it disagrees with a recording or can't decide.
const PORT_REPORTS: &[&str] = &[
    "driven replay: the unit groups",
    "driven replay: a unit group",
    "driven replay: a recorded unit group",
    "driven replay: a leave_tick",
    "driven replay: the leave controller",
    "driven replay: the turn controller",
    "driven replay: the sim's turn event",
    "driven replay: a turn event",
    "driven replay: a LeaveCombat",
    "driven replay: the sim's LeaveCombat",
    "driven replay: a ChooseNextUnit",
    "driven replay: a choose_next",
    "driven replay: a UI guard",
    "driven replay: the UI guard",
    "driven replay: a ui_guard",
    "driven replay: a cooldowns record",
    "CombatController.",
    "UnitCombatLeaveController",
    "HandleUnitRollsInitiative",
    "UnitCombatPrepareController",
    "InspectUnits",
    "UnitEntityData.LeaveCombat",
    "UnitEntityData.OnIsInGameChanged",
    "UnitLifeController: controllers listening",
    "SettingsRoot.Difficulty",
    "BuffCollection.",
];

fn fixture(name: &str) -> Option<PathBuf> {
    fixture_in(CAPTURES[2], name)
}

fn fixture_in(capture: &str, name: &str) -> Option<PathBuf> {
    // The captures are gitignored and live in the main checkout: look in this checkout and
    // every directory above it (a worktree under `.claude/worktrees/` finds the main one),
    // or where `WR_CAPTURES` says.
    let rel = format!("oracle/{capture}/fixtures/{name}");
    let mut roots: Vec<PathBuf> =
        std::env::var_os("WR_CAPTURES").map(PathBuf::from).into_iter().collect();
    roots.extend(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().map(|a| a.join("fixtures/captures")),
    );
    roots.into_iter().map(|r| r.join(&rel)).find(|d| d.join("record.trace.jsonl.gz").exists())
}

/// Every run of both fights of every capture: the sim's rules, results, draws and mutations
/// equal the recording's, in order; the turn order is the comparer's.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-51..53 captures"]
fn driven_replay_equals_the_recordings() {
    for (capture, name) in CAPTURES.iter().flat_map(|c| FIGHTS.map(move |n| (*c, n))) {
        let Some(dir) = fixture_in(capture, name) else {
            eprintln!("{capture}/{name} not present: skipped");
            continue;
        };
        for run in ["record", "replay-1", "replay-2"] {
            let name = format!("{capture}/{name}");
            let (out, w) = replay::run_fixture(SAVE, &dir, run).expect("replay");
            assert!(
                out.diff.equal,
                "{name} {run}: first difference at {:?}: {:?}",
                out.diff.first_difference, out.diff.context
            );
            assert!(w.rng.failures().is_empty(), "{name} {run}: {:?}", w.rng.failures());
            assert!(out.skipped.is_empty(), "{name} {run}: skipped {:?}", out.skipped);
            assert!(out.sim.len() > 150, "{name} {run}: a real stream, not an empty one");
            assert!(out.echo_rejected.is_empty(), "{name} {run}: {:?}", out.echo_rejected);
            if WITH_ORIGINS.contains(&capture) {
                // Every presentation root is computed where its call site places it: the UI's
                // initiative tracker, the hover and click predictions, the AI and the commands.
                assert!(out.echoed.is_empty(), "{name} {run}: {:?}", out.echoed);
                assert_eq!(out.echoed_records, 0, "{name} {run}");
                let callers: Vec<&str> = out.by_origin.keys().copied().collect();
                // With the attacks observer the commands' rules are `UnitAttack`'s sim code's
                // (the AI's attack counts were its runs' `InitAttacks`).
                let attacks = WITH_ATTACKS.contains(&capture);
                let command = if attacks { "command (UnitAttack)" } else { "command" };
                let required: &[&str] = if WITH_APPROACH.contains(&capture) {
                    // (`poc5-tb-81/tb-nearest`'s inputs trigger no rule.)
                    &[command, "ui"]
                } else if attacks {
                    &[command, "input", "ui"]
                } else {
                    &["ai", command, "input", "ui"]
                };
                for &c in required {
                    assert!(callers.contains(&c), "{name} {run}: {callers:?}");
                }
                // (With the approach observer an attack of opportunity's rule is its sim code's.)
                let aoo = WITH_APPROACH.contains(&capture);
                assert!(
                    callers
                        .iter()
                        .all(|c| ["ai", "ai (StartTurn)", "command", "input", "ui"].contains(c)
                            || *c == command
                            || (aoo && *c == "command (UnitAttackOfOpportunity)")),
                    "{name} {run}: {callers:?}"
                );
                // (The approach capture's fights are shorter.)
                let ui = if aoo { 30 } else { 50 };
                assert!(out.by_origin["ui"] >= ui, "{name} {run}: {:?}", out.by_origin);
            } else {
                // What was echoed, not computed, is only the presentation layer's rules.
                let echoed: Vec<&str> = out.echoed.keys().map(String::as_str).collect();
                assert_eq!(
                    echoed,
                    [
                        "RuleCalculateAttacksCount",
                        "RuleCalculateAttacksCountLimbs",
                        "RuleCheckTargetFlatFooted"
                    ]
                );
                // Only the computed records are compared; the echoed roots are unverified and
                // none of them held anything that changes state.
                assert!(out.echoed_records >= 100, "{name} {run}: {}", out.echoed_records);
            }
            assert_eq!(out.sim.len(), out.oracle.len());
            // The end state's flags (alive, conscious, in game) equal the oracle's.
            let units: Vec<_> = (0..w.entities.len())
                .map(|i| wr_sim::world::EntityIdx(u32::try_from(i).unwrap()))
                .filter(|e| w.unit(*e).is_some())
                .collect();
            let sheet = wr_sim::sheet::dump(&w, &units, "end");
            let end: serde_json::Value = serde_json::from_slice(
                &std::fs::read(dir.join(format!("state-end-{run}.json"))).expect("end state"),
            )
            .expect("json");
            let (flags, diffs) = wr_sim::compare::compare_flags(&sheet, &end);
            assert!(diffs.is_empty(), "{name} {run}: {diffs:?}");
            assert_eq!(flags.len(), 3, "all three flags were compared");
            assert!(units.iter().all(|u| w.unit(*u).is_some_and(|x| x.bonus_experience.is_some())));
            // Positions at impact: every ranged resolve that deals damage has its own `env`
            // record, so none is computed with a recorded decision.
            assert_eq!(out.impact_positions[1], 0, "{name} {run}");
            assert!(out.unverified.is_empty(), "{name} {run}: {:?}", out.unverified);
            assert_eq!(out.unverified_records, 0, "{name} {run}");
            // The awake states the sim can't establish come from the recording, every frame.
            assert!(out.awake_recorded > 10_000, "{name} {run}: {}", out.awake_recorded);
            // Every event is placed in its frame's controller phase, the turn starts' sweeps are
            // ordered, the attacks come from running commands, the turn-based state agrees with
            // every time record, and the current unit is established whenever a buff is due.
            for bad in [
                "driven replay: a combat join",
                "driven replay: a root",
                "driven replay: a turn start",
                "driven replay: initiative of a unit",
                "driven replay: turn-based combat state differs",
                "driven replay: the time record",
                "driven replay: an input",
                "driven replay: game mode",
                "driven replay: a game mode change",
                "driven replay: an attack environment",
                "driven replay: a recording without",
                "driven replay: the current turn differs",
                "driven replay: a turn clear of a unit",
                "driven replay: a turn_start",
                "driven replay: a loot timer",
                "driven replay: a recorded awake unit",
                "driven replay: when an ended turn",
                "driven replay: positions at impact",
                "DropLootAndDestroyOnDeactivate",
                // Ported: the dead are natural-attack carriers (nothing lootable, no drop), their
                // slot indices recomputed, the trigger actions run in their contexts.
                "UnitEntityData.DropLootToGround",
                "UnitLifeController: ItemEntity.UpdateSlotIndex",
                "ItemEntity.UpdateSlotIndex",
                "CombatStateTrigger",
                "EvaluatedUnitCombatTrigger",
                "CombatController.StartTurn",
                "BuffCollection.Tick",
                "SleepingUnitsController",
            ] {
                let hit: Vec<_> =
                    w.coverage.unsupported.keys().filter(|k| k.starts_with(bad)).collect();
                assert!(hit.is_empty(), "{name} {run}: {hit:?}");
            }
            // With the turn port, the turn order, groups, leaving combat and the death listeners are
            // the sim's and agree with every record.
            if ["poc5-tb-55", "poc5-tb-60", "poc5-tb-63", "poc5-tb-68", "poc5-tb-74", "poc5-tb-77"]
                .contains(&capture)
                || WITH_APPROACH.contains(&capture)
            {
                assert!(w.combat.turns.on, "{name} {run}: the port runs");
                for bad in PORT_REPORTS {
                    let hit: Vec<_> =
                        w.coverage.unsupported.keys().filter(|k| k.starts_with(bad)).collect();
                    assert!(hit.is_empty(), "{name} {run}: {hit:?}");
                }
                assert_eq!(out.sorts.equal, out.sorts.checked, "{name} {run}: {:?}", out.sorts);
                let left: Vec<&String> = w.coverage.unsupported.keys().collect();
                if WITH_ORIGINS.contains(&capture) {
                    // Nothing is reported: the run is valid.
                    assert!(left.is_empty(), "{name} {run}: {left:?}");
                    assert!(w.valid(), "{name} {run}");
                }
                if WITH_TURN_TICKS.contains(&capture) {
                    // Every turn controller tick is the sim's, and every turn end: the recorded
                    // `turn_end` records are checks.
                    assert!(out.turn_ticks > 500, "{name} {run}: {}", out.turn_ticks);
                    let trace =
                        Trace::load(&dir.join(format!("{run}.trace.jsonl.gz"))).expect("trace");
                    let recorded = trace
                        .records
                        .iter()
                        .filter(|r| r["k"] == "tb" && r["ev"] == "turn_end")
                        .count();
                    assert!(recorded >= 4, "{name} {run}: {recorded}");
                    assert_eq!(out.turn_ends, recorded, "{name} {run}");
                } else {
                    assert_eq!((out.turn_ticks, out.turn_ends), (0, 0), "{name} {run}");
                }
                if WITH_COMMANDS.contains(&capture) {
                    // The units' commands are the sim's: every unit the command controller ticks,
                    // every run, start, act and end, the turn's running commands and the commands'
                    // SpendAction, checked against every recorded row and turn controller read;
                    // only the class code, the movement, the AI's and the UI's calls are inputs.
                    assert_eq!(out.commands_recorded, 0, "{name} {run}");
                    assert!(out.commands_checked > 2_000, "{name} {run}: {}", out.commands_checked);
                    assert!(out.commands_ticked > 1_000, "{name} {run}: {}", out.commands_ticked);
                    assert!(out.commands_run >= 10, "{name} {run}: {}", out.commands_run);
                    assert!(out.commands_ends >= 5, "{name} {run}: {}", out.commands_ends);
                    assert!(out.commands_values > 1_000, "{name} {run}: {}", out.commands_values);
                } else {
                    assert_eq!(out.commands_ticked + out.commands_run, 0, "{name} {run}");
                }
                if WITH_ATTACKS.contains(&capture) {
                    // `UnitAttack`'s class code is the sim's, each call checked against its
                    // recorded state, and its rules (the attack counts, the attacks) are fired by
                    // it with the fields it computes, each checked against its recorded root.
                    // (With the approach observer `TickApproaching` isn't class code: fewer calls.)
                    let (calls, rules) =
                        if WITH_APPROACH.contains(&capture) { (100, 20) } else { (1_000, 30) };
                    assert!(out.attacks_calls > calls, "{name} {run}: {}", out.attacks_calls);
                    assert!(out.attacks_rules >= rules, "{name} {run}: {}", out.attacks_rules);
                    assert_eq!(
                        out.by_origin.get("command (UnitAttack)").copied(),
                        Some(out.attacks_rules),
                        "{name} {run}: {:?}",
                        out.by_origin
                    );
                } else {
                    assert_eq!(out.attacks_calls + out.attacks_rules, 0, "{name} {run}");
                }
                if WITH_APPROACH.contains(&capture) {
                    // `TickApproaching` is the sim's (no class code taken), with its draws and its
                    // calls into the movement agent, each checked against its record; so are
                    // `UnitMoveTo`'s and the attack of opportunity's class code.
                    assert_eq!(out.commands_fx, 0, "{name} {run}");
                    assert!(out.approach_calls > 500, "{name} {run}: {}", out.approach_calls);
                    assert!(out.approach_draws > 50, "{name} {run}: {}", out.approach_draws);
                    // (`poc5-tb-82/tb-focus`'s follow re-issues few moves: 9 class-code calls.)
                    assert!(out.approach_class > 5, "{name} {run}: {}", out.approach_class);
                    // Every attack of opportunity's rule is the sim's (`poc5-tb-81/tb-focus` has
                    // two).
                    let trace =
                        Trace::load(&dir.join(format!("{run}.trace.jsonl.gz"))).expect("trace");
                    let recorded = trace
                        .records
                        .iter()
                        .filter(|r| {
                            r["k"] == "rule"
                                && r["parent"] == 0
                                && r["origin"].as_str().is_some_and(|o| {
                                    o.starts_with(
                                        "Kingmaker.UnitLogic.Commands.UnitAttackOfOpportunity.OnAction <",
                                    )
                                })
                        })
                        .count();
                    let computed = out
                        .by_origin
                        .get("command (UnitAttackOfOpportunity)")
                        .copied()
                        .unwrap_or(0);
                    assert_eq!(computed, recorded, "{name} {run}");
                    assert_eq!(out.approach_rules, computed, "{name} {run}");
                    if name == "poc5-tb-81/tb-focus" {
                        assert_eq!(computed, 2, "{name} {run}");
                    }
                } else {
                    assert_eq!(
                        out.approach_calls
                            + out.approach_draws
                            + out.approach_class
                            + out.approach_rules,
                        0,
                        "{name} {run}"
                    );
                }
                if WITH_AI.contains(&capture) {
                    // The AI is the sim's: every decision made, each score checked, every command it
                    // runs and every draw it makes the sim's.
                    assert!(out.ai_entries > 1_000, "{name} {run}: {}", out.ai_entries);
                    assert!(out.ai_decisions >= 5, "{name} {run}: {}", out.ai_decisions);
                    assert!(out.ai_scores > out.ai_decisions, "{name} {run}: {}", out.ai_scores);
                    assert!(out.ai_runs >= 5, "{name} {run}: {}", out.ai_runs);
                    assert_eq!(out.ai_draws, out.ai_runs, "{name} {run}");
                    assert!(!out.by_origin.contains_key("ai"), "{name} {run}: {:?}", out.by_origin);
                } else {
                    assert_eq!(out.ai_entries + out.ai_decisions + out.ai_runs, 0, "{name} {run}");
                }
                if WITH_ENGAGE.contains(&capture) {
                    // The engagement controller is the sim's: every tick run, every engagement
                    // change and attack of opportunity checked, the attacks of opportunity it runs
                    // the sim's.
                    assert!(out.eng_entries > 1_000, "{name} {run}: {}", out.eng_entries);
                    assert!(out.eng_changes >= 4, "{name} {run}: {}", out.eng_changes);
                    assert_eq!(out.eng_runs, out.eng_count_writes, "{name} {run}");
                    // (`-84`'s archer provokes with each ranged attack; in `-92/tb-focus` it doesn't
                    // shoot.)
                    if name != "poc5-tb-92/tb-focus" {
                        assert!(out.eng_provokes >= 1, "{name} {run}: {}", out.eng_provokes);
                    }
                } else {
                    assert_eq!(out.eng_entries + out.eng_changes + out.eng_aoos, 0, "{name} {run}");
                }
                if WITH_TARGETS.contains(&capture) {
                    // Units' targets are the sim's: every write it makes checked (the attacks'
                    // `UpdateCombatTarget`, leaving combat's `Clear`), every read of them checked.
                    assert!(out.tgt_checked >= 10, "{name} {run}: {}", out.tgt_checked);
                    assert!(out.tgt_reads >= 20, "{name} {run}: {}", out.tgt_reads);
                } else {
                    assert_eq!(out.tgt_checked + out.tgt_reads, 0, "{name} {run}");
                }
                if WITH_IMPATIENCE.contains(&capture) {
                    // The impatience controller is the sim's: every tick and `UpdatePriorityTarget`
                    // run, each state after it checked, every handler's record matched.
                    assert!(out.imp_entries > 1_000, "{name} {run}: {}", out.imp_entries);
                    assert!(out.imp_states > out.imp_entries, "{name} {run}: {}", out.imp_states);
                    assert!(out.imp_handlers >= 5, "{name} {run}: {}", out.imp_handlers);
                } else {
                    assert_eq!(
                        out.imp_entries + out.imp_states + out.imp_handlers,
                        0,
                        "{name} {run}"
                    );
                }
                if WITH_COOLDOWNS.contains(&capture) {
                    // The cooldowns are the sim's at every prefix and around every recorded
                    // call; the calls that decide them (movement, commands acting and ending, the
                    // end-turn button, an attack of opportunity) are the recorded inputs.
                    assert_eq!(out.cooldowns_recorded, 0, "{name} {run}");
                    assert!(
                        out.cooldowns_checked > 5_000,
                        "{name} {run}: {}",
                        out.cooldowns_checked
                    );
                    let n = |k: &str| out.cooldown_inputs.get(k).copied().unwrap_or(0);
                    // (`poc5-tb-81/tb-nearest` is a short fight.)
                    let moves = if WITH_APPROACH.contains(&capture) { 300 } else { 500 };
                    assert!(n("TickMovement") > moves, "{name} {run}: {:?}", out.cooldown_inputs);
                    // A command acting: recorded, or the sim's commands' (each checked).
                    let spends = if WITH_COMMANDS.contains(&capture) {
                        out.commands_spends
                    } else {
                        n("SpendAction")
                    };
                    assert!(spends >= 3, "{name} {run}: {:?}", out.cooldown_inputs);
                    assert!(
                        n("HandleUnitCommandDidEnd") >= 8,
                        "{name} {run}: {:?}",
                        out.cooldown_inputs
                    );
                } else if WITH_ORIGINS.contains(&capture) {
                    assert!(out.cooldown_inputs.is_empty(), "{name} {run}");
                } else {
                    // What keeps the run from being valid is only the presentation roots' origin.
                    assert_eq!(
                        left,
                        [
                            "driven replay: presentation root rules echoed, their origin is not established"
                        ],
                        "{name} {run}"
                    );
                }
            }
            // The end state equals the oracle's, every unit and field (fact ids included: the
            // tutorial fact takes its unique id).
            let r = wr_sim::compare::compare(&sheet, &end);
            assert!(r.diffs.is_empty(), "{name} {run}: {:?}", r.diffs);
            // Every order is the comparer's on the sim's initiatives. At most one sort, the round
            // that starts as the party leaves combat, holds a unit whose initiative its
            // `LeaveCombat` cleared (not ported: reported).
            let s = out.sorts;
            assert!(s.checked >= 7);
            assert_eq!(s.hash_fallback, 0, "{name} {run}");
            assert!(s.initiative_mismatch <= 1, "{name} {run}: {s:?}");
            assert!(s.equal + s.initiative_mismatch >= s.checked, "{name} {run}: {s:?}");
            // The replay reports what it can't run; nothing about the attack chain.
            let attack = ["RuleAttack", "RuleCalculate", "RuleDealDamage", "RuleInitiative"];
            for k in w.coverage.unsupported.keys() {
                assert!(!attack.iter().any(|p| k.starts_with(p)), "{name} {run}: unsupported {k}");
            }
        }
    }
}

/// A draw taken from another value must not go unnoticed: with one attack roll's d20 changed
/// on the tape (not in the recording's projection), the replay diverges.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-53 capture"]
fn a_perturbed_tape_diverges() {
    let Some(dir) = fixture("tb-nearest") else {
        eprintln!("poc5-tb-53/tb-nearest not present: skipped");
        return;
    };
    let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
    let mut perturbed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
    // The first d20 of an attack roll (a draw made inside a nested RuleRollD20 at depth 2).
    let at = perturbed
        .records
        .iter()
        .position(|r| r["k"] == "draw" && r["m"] == "Range_ii" && r["a"] == json!([1, 21]))
        .expect("a d20");
    let was = perturbed.records[at]["v"].as_i64().expect("value");
    perturbed.records[at]["v"] = json!(if was == 1 { 2 } else { 1 });
    let out = replay::run_with_tape(&mut w, &trace, &perturbed);
    assert!(!out.diff.equal, "a changed draw must show");
}

/// Wrong life or in-game results must show in the end-state flags: a dead centipede made
/// conscious, and an out-of-game one back in the game, are both caught.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-53 capture"]
fn wrong_life_state_or_in_game_flags_are_caught() {
    let Some(dir) = fixture("tb-nearest") else {
        eprintln!("poc5-tb-53/tb-nearest not present: skipped");
        return;
    };
    let (out, mut w) = replay::run_fixture(SAVE, &dir, "record").expect("replay");
    assert!(out.diff.equal);
    let dead: Vec<_> = (0..w.entities.len())
        .map(|i| wr_sim::world::EntityIdx(u32::try_from(i).unwrap()))
        .filter(|e| w.unit(*e).is_some_and(|x| x.life_state == 2))
        .collect();
    assert_eq!(dead.len(), 3);
    let end: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("state-end-record.json")).unwrap()).unwrap();
    let dump = |w: &wr_sim::world::World| {
        let units: Vec<_> = (0..w.entities.len())
            .map(|i| wr_sim::world::EntityIdx(u32::try_from(i).unwrap()))
            .filter(|e| w.unit(*e).is_some())
            .collect();
        wr_sim::sheet::dump(w, &units, "end")
    };
    assert!(wr_sim::compare::compare_flags(&dump(&w), &end).1.is_empty());
    w.unit_mut(dead[0]).unwrap().life_state = 0;
    w.entity_mut(dead[1]).is_in_game = true;
    let (_, diffs) = wr_sim::compare::compare_flags(&dump(&w), &end);
    assert!(diffs.iter().any(|d| d.1 == "is_dead"));
    assert!(diffs.iter().any(|d| d.1 == "is_conscious"));
    assert!(diffs.iter().any(|d| d.1 == "in_game"));
}

/// The computed cooldowns must notice a wrong input: with one movement delta of the recording
/// doubled, the sim's move action differs from the recording's and is reported.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-63 capture"]
fn a_changed_movement_delta_is_reported() {
    let Some(dir) = fixture_in("poc5-tb-63", "tb-nearest") else {
        eprintln!("poc5-tb-63/tb-nearest not present: skipped");
        return;
    };
    let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
    let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
    let at = changed
        .records
        .iter()
        .position(|r| r["k"] == "obs" && r["ev"] == "cd_move" && r["dt"] == r["dt_out"])
        .expect("a movement tick");
    let dt: f32 = changed.records[at]["dt"].as_str().unwrap().parse().unwrap();
    changed.records[at]["dt"] = json!((dt * 2.0).to_string());
    let out = replay::run_with_tape(&mut w, &changed, &trace);
    assert!(out.cooldowns_recorded > 0);
    assert!(!w.valid());
    assert!(
        w.coverage
            .unsupported
            .keys()
            .any(|k| k.starts_with("driven replay: the sim's cooldowns after TickMovement")),
        "{:?}",
        w.coverage.unsupported.keys().collect::<Vec<_>>()
    );
}

/// Each recorded cooldown check and input is required: frames without their prefix, a unit in
/// combat without its row, a call without its after row or with an input missing, and a clear whose
/// before row differs from the sim's are each reported.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-63 capture"]
fn missing_or_wrong_cooldown_records_are_reported() {
    let Some(dir) = fixture_in("poc5-tb-63", "tb-nearest") else {
        eprintln!("poc5-tb-63/tb-nearest not present: skipped");
        return;
    };
    type Change = fn(&mut Vec<serde_json::Value>);
    let cases: [(&str, Change); 8] = [
        ("driven replay: cd_clear of an unknown unit", |recs| {
            let at = recs
                .iter()
                .position(|r| {
                    r["ev"] == "cd_clear"
                        && r["caller"]
                            .as_str()
                            .unwrap_or("")
                            .contains("UnitCombatPrepareController")
                })
                .unwrap();
            recs[at]["u"] = json!("no-such-unit");
        }),
        ("driven replay: a frame without the prepare controller's prefix", |recs| {
            recs.retain(|r| r["ev"] != "cd_prefix");
        }),
        ("driven replay: a unit in combat without a recorded cooldowns row", |recs| {
            let at = recs.iter().position(|r| r["ev"] == "cooldowns").unwrap();
            let tick = recs[at]["t"].clone();
            recs.retain(|r| !(r["ev"] == "cooldowns" && r["t"] == tick));
        }),
        ("driven replay: a cd_move record it can't read", |recs| {
            let at = recs.iter().position(|r| r["ev"] == "cd_move").unwrap();
            recs[at].as_object_mut().unwrap().remove("force");
        }),
        ("driven replay: a cd_cmd_end record it can't read", |recs| {
            let at = recs.iter().position(|r| r["ev"] == "cd_cmd_end").unwrap();
            recs[at].as_object_mut().unwrap().remove("status");
        }),
        ("driven replay: a frame without the prepare controller's prefix", |recs| {
            let at = recs.iter().rposition(|r| r["ev"] == "cd_prefix").unwrap();
            recs.remove(at);
        }),
        ("driven replay: TickMovement: its unit's cooldowns after it are unknown", |recs| {
            let at = recs.iter().position(|r| r["ev"] == "cd_move").unwrap();
            recs[at].as_object_mut().unwrap().remove("a");
        }),
        (
            "driven replay: a unit's cooldowns differ from the recording's before the prepare",
            |recs| {
                let at = recs
                    .iter()
                    .position(|r| {
                        r["ev"] == "cd_clear"
                            && r["caller"]
                                .as_str()
                                .unwrap_or("")
                                .contains("UnitCombatPrepareController")
                    })
                    .unwrap();
                recs[at]["b"] = json!("0|0|1|0|0");
            },
        ),
    ];
    for (want, change) in cases {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
        change(&mut changed.records);
        let _ = replay::run_with_tape(&mut w, &changed, &trace);
        assert!(!w.valid(), "{want}");
        assert!(
            w.coverage.unsupported.keys().any(|k| k.starts_with(want)),
            "{want}: {:?}",
            w.coverage.unsupported.keys().collect::<Vec<_>>()
        );
    }
}

/// The turn controller's tick is computed, so each of its recorded inputs and checks is required:
/// a changed input changes the sim's decision and is reported, and a missing record (a tick, an end
/// wait, a turn end) is reported rather than skipped.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-68 capture"]
fn missing_or_wrong_turn_tick_records_are_reported() {
    let Some(dir) = fixture_in("poc5-tb-68", "tb-nearest") else {
        eprintln!("poc5-tb-68/tb-nearest not present: skipped");
        return;
    };
    type Change = fn(&mut Vec<serde_json::Value>);
    fn idle_end(recs: &[serde_json::Value]) -> usize {
        recs.iter().position(|r| r["ev"] == "tt_ca" && r["res"] == false).unwrap()
    }
    fn preparing(recs: &[serde_json::Value]) -> usize {
        recs.iter()
            .position(|r| {
                r["ev"] == "tt_sc"
                    && r["st"] == "Preparing"
                    && r["running"] == 0
                    && r["r_empty"] == true
            })
            .unwrap()
    }
    fn last_wait(recs: &[serde_json::Value]) -> usize {
        recs.iter().position(|r| r["ev"] == "tt_cw" && r["res"] == false).unwrap()
    }
    let cases: [(&str, Change); 32] = [
        // A tick's end replaced by another frame's, or without its frame.
        ("driven replay: tt_end of another frame", |recs| {
            let ends: Vec<usize> = recs
                .iter()
                .enumerate()
                .filter(|(_, r)| r["ev"] == "tt_end")
                .map(|(i, _)| i)
                .collect();
            let (a, b) = (ends[0], ends[1]);
            recs[b] = recs[a].clone();
        }),
        ("driven replay: tt_sc of another frame", |recs| {
            for r in recs.iter_mut().filter(|r| r["ev"] == "tt_sc") {
                r.as_object_mut().unwrap().remove("t");
            }
        }),
        ("driven replay: a turn end the sim's turn controller didn't make", |recs| {
            let at = recs.iter().position(|r| r["ev"] == "turn_end").unwrap();
            recs[at].as_object_mut().unwrap().remove("t");
        }),
        ("driven replay: ContinueWaiting: a time it can't read", |recs| {
            for r in recs.iter_mut().filter(|r| r["ev"] == "tt_cw") {
                let t = r["time"].as_str().unwrap().rsplit_once('|').unwrap().0.to_owned();
                r["time"] = json!(format!("{t}|NaN"));
            }
        }),
        // The frame's time record replaced by the previous frame's, or without its frame.
        ("driven replay: a time record of another frame", |recs| {
            let tick = recs[last_wait(recs)]["t"].as_i64().unwrap();
            let prev =
                recs.iter().find(|r| r["k"] == "time" && r["t"] == tick - 1).unwrap().clone();
            let at = recs.iter().position(|r| r["k"] == "time" && r["t"] == tick).unwrap();
            recs[at] = prev;
        }),
        ("driven replay: a time record of another frame", |recs| {
            let tick = recs[last_wait(recs)]["t"].clone();
            let at = recs.iter().position(|r| r["k"] == "time" && r["t"] == tick).unwrap();
            recs[at].as_object_mut().unwrap().remove("t");
        }),
        // The time record of the frame whose end wait ends a turn, and of an ordinary tick.
        ("driven replay: a turn controller tick without its frame's time record", |recs| {
            let tick = recs[last_wait(recs)]["t"].clone();
            recs.retain(|r| !(r["k"] == "time" && r["t"] == tick));
        }),
        ("driven replay: a turn controller tick without its frame's time record", |recs| {
            let tick = recs.iter().find(|r| r["ev"] == "tt").unwrap()["t"].clone();
            recs.retain(|r| !(r["k"] == "time" && r["t"] == tick));
        }),
        (
            "driven replay: a time record without the deltas or time scales the turn controller reads",
            |recs| {
                let at = recs.iter().rposition(|r| r["k"] == "time").unwrap();
                recs[at].as_object_mut().unwrap().remove("delta");
            },
        ),
        (
            "driven replay: a time record without the deltas or time scales the turn controller reads",
            |recs| {
                let at = recs.iter().rposition(|r| r["k"] == "time").unwrap();
                recs[at]["player_time_scale"] = json!("1");
            },
        ),
        // Every tick record lost: the recording declares the observer, so no fallback.
        (
            "driven replay: the recording declares its turn_ticks observer, but its records are missing",
            |recs| {
                recs.retain(|r| !r["ev"].as_str().is_some_and(|e| e.starts_with("tt")));
            },
        ),
        ("driven replay: a tt_sc record it can't read", |recs| {
            let at = recs.iter().position(|r| r["ev"] == "tt_sc").unwrap();
            recs[at]["running"] = json!(-1);
        }),
        // A preparing turn whose rider now has a command acts, and with nothing running asks for a
        // projectile and ContinueActing in the same status change.
        (
            "driven replay: TurnController.Tick's end, but the status change's ContinueActing isn't recorded",
            |recs| {
                let at = preparing(recs);
                recs[at]["r_empty"] = json!(false);
                recs[at]["proj"] = json!(false);
            },
        ),
        // The game asked the preparing rider's IsAbleToAct; without its record the sim can't decide.
        (
            "driven replay: the tick's status change: the rider's IsAbleToAct isn't recorded",
            |recs| {
                let at = preparing(recs);
                let able = at + recs[at..].iter().position(|r| r["ev"] == "tt_able").unwrap();
                recs.remove(able);
            },
        ),
        // With a command, the status change doesn't ask IsAbleToAct: the recorded one is reported.
        ("driven replay: an IsAbleToAct the sim's status change doesn't ask", |recs| {
            let at = preparing(recs);
            recs[at]["r_empty"] = json!(false);
        }),
        ("driven replay: a begin record whose observers list it can't read", |recs| {
            recs[0]["observers"] = json!(null);
        }),
        ("driven replay: a begin record whose observers list it can't read", |recs| {
            recs[0]["observers"] = json!(["groups", "cooldowns", 7]);
        }),
        ("driven replay: a tt record it can't read", |recs| {
            let at = recs.iter().position(|r| r["ev"] == "tt").unwrap();
            recs[at].as_object_mut().unwrap().remove("surprise");
        }),
        ("TurnController.Prepare: a turn with a mount (or not recorded)", |recs| {
            let at = recs.iter().position(|r| r["ev"] == "tt_prep").unwrap();
            recs[at].as_object_mut().unwrap().remove("mctrl");
        }),
        (
            "driven replay: the tick's status change: a turn with a mount (or not recorded)",
            |recs| {
                let at = recs.iter().position(|r| r["ev"] == "tt_sc").unwrap();
                recs[at].as_object_mut().unwrap().remove("m_empty");
            },
        ),
        // Every tick's start lost: the port must not fall back to the recorded turn ends.
        ("driven replay: a tt_end without its tick's start (tt)", |recs| {
            recs.retain(|r| r["ev"] != "tt");
        }),
        // The tick that ends a turn lost its end.
        ("driven replay: a turn controller tick without its end (tt_end)", |recs| {
            let at = last_wait(recs);
            let end = at + recs[at..].iter().position(|r| r["ev"] == "tt_end").unwrap();
            recs.remove(end);
        }),
        ("driven replay: ContinueActing where the sim's status change doesn't call it", |recs| {
            let at = idle_end(recs);
            recs[at].as_object_mut().unwrap().remove("u");
        }),
        ("driven replay: ContinueActing where the sim's status change doesn't call it", |recs| {
            let at = idle_end(recs);
            recs[at]["u"] = json!("tt_ca");
        }),
        // The AI's unit moves after all: it goes on acting.
        ("driven replay: ContinueActing's result differs", |recs| {
            let at = idle_end(recs);
            recs[at]["motion"] = json!(true);
        }),
        // Its command still runs: the turn doesn't end yet.
        ("driven replay: ContinueWaiting's result differs", |recs| {
            let at = last_wait(recs);
            recs[at]["r_run"] = json!(true);
        }),
        ("driven replay: a turn end the sim's turn controller didn't make", |recs| {
            let at = last_wait(recs);
            recs.remove(at);
        }),
        (
            "driven replay: the sim's tick ended the turn, but the recording has no turn_end",
            |recs| {
                recs.retain(|r| r["ev"] != "turn_end");
            },
        ),
        (
            "driven replay: the turn controller ticks in this frame, but its tick (tt) isn't recorded",
            |recs| {
                let at = recs.iter().position(|r| r["ev"] == "tt").unwrap();
                let tick = recs[at]["t"].clone();
                recs.retain(|r| {
                    !(r["t"] == tick && r["ev"].as_str().is_some_and(|e| e.starts_with("tt")))
                });
            },
        ),
        // Nothing running after all: the status change asks ContinueActing, which isn't there.
        (
            "driven replay: TurnController.Tick's end, but the status change's ContinueActing isn't recorded",
            |recs| {
                let at = recs
                    .iter()
                    .position(|r| r["ev"] == "tt_sc" && r["st"] == "Acting" && r["running"] == 1)
                    .unwrap();
                recs[at]["running"] = json!(0);
                recs[at]["proj"] = json!(false);
            },
        ),
        ("driven replay: a tt record it can't read", |recs| {
            let at = recs.iter().position(|r| r["ev"] == "tt").unwrap();
            recs[at].as_object_mut().unwrap().remove("ctrl");
        }),
        (
            "driven replay: the turn controller's status or counters differ from the recording's at TurnController.Prepare's end",
            |recs| {
                let at = recs.iter().position(|r| r["ev"] == "tt_prep").unwrap();
                let ctrl = recs[at]["ctrl"].as_bool().unwrap();
                recs[at]["ctrl"] = json!(!ctrl);
            },
        ),
    ];
    for (want, change) in cases {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
        change(&mut changed.records);
        let _ = replay::run_with_tape(&mut w, &changed, &trace);
        assert!(!w.valid(), "{want}");
        assert!(
            w.coverage.unsupported.keys().any(|k| k.starts_with(want)),
            "{want}: {:?}",
            w.coverage.unsupported.keys().collect::<Vec<_>>()
        );
    }
}

/// The units' commands are computed, so each recorded input and check is required: a lost or
/// changed leaf, class-code result, end, running-list call, `TickCommand`, `SpendAction` or state
/// row, a record of another frame or an unplaceable caller, and every command read of the turn
/// controller and the leave controller, are each reported rather than taken.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-74 capture"]
fn missing_or_wrong_command_records_are_reported() {
    let Some(dir) = fixture_in("poc5-tb-74", "tb-nearest") else {
        eprintln!("poc5-tb-74/tb-nearest not present: skipped");
        return;
    };
    type Change = fn(&mut Vec<serde_json::Value>);
    fn pos(recs: &[serde_json::Value], f: impl Fn(&serde_json::Value) -> bool) -> usize {
        recs.iter().position(f).expect("a record to change")
    }
    fn ev(r: &serde_json::Value, e: &str) -> bool {
        r["k"] == "obs" && r["ev"] == e
    }
    let cases: [(&str, Change); 72] = [
        (
            "driven replay: the recording declares its commands observer, but its records are missing",
            |recs| recs.retain(|r| !r["ev"].as_str().is_some_and(|e| e.starts_with("uc_"))),
        ),
        ("driven replay: the command controller ticks before the commands snapshot", |recs| {
            recs.retain(|r| !ev(r, "uc_snap"));
        }),
        ("driven replay: a frame without the command controller's tick (uc_ctl)", |recs| {
            let at = pos(recs, |r| ev(r, "uc_ctl"));
            recs.remove(at);
        }),
        ("driven replay: commands: the command controller's end", |recs| {
            let at = pos(recs, |r| ev(r, "uc_ctl_end"));
            recs.remove(at);
        }),
        // A leaf lost, renamed, of another command, or with another value.
        ("driven replay: commands: ", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v"));
            recs.remove(at);
        }),
        ("driven replay: commands: the sim reads", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "sbi");
            recs[at]["f"] = json!("canstart");
        }),
        ("driven replay: commands: the sim reads", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "sbi");
            recs[at]["c"] = json!(99_999);
        }),
        ("driven replay: commands: ", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "sbi" && r["v"] == false);
            recs[at]["v"] = json!(true);
        }),
        ("driven replay: commands: a leaf sbi with fields its writer doesn't write", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "sbi");
            recs[at].as_object_mut().unwrap().remove("v");
        }),
        ("driven replay: commands: the sim reads", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "sbi");
            recs[at]["v"] = json!("yes");
        }),
        // Class code: finishing a command without its end, another call, an exception.
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, |r| ev(r, "uc_fx") && r["fn"] == "OnTick");
            let s = recs[at]["s"].as_str().unwrap().to_owned();
            recs[at]["s"] = json!(format!("{}1{}", &s[..2], &s[3..]));
        }),
        ("driven replay: commands: the sim calls", |recs| {
            let at = pos(recs, |r| ev(r, "uc_fx") && r["fn"] == "OnTick");
            recs[at]["fn"] = json!("OnStart");
        }),
        ("driven replay: commands: an exception out of", |recs| {
            let at = pos(recs, |r| ev(r, "uc_fx"));
            recs[at]["exc"] = json!(true);
        }),
        ("driven replay: commands: a uc_fx record", |recs| {
            let at = pos(recs, |r| ev(r, "uc_fx") && r["fn"] == "OnAction");
            recs[at]["ret"] = json!(null);
        }),
        // A base end lost or with another result; a running-list call lost or of another length.
        ("driven replay: commands: the end of command", |recs| {
            let at = pos(recs, |r| ev(r, "uc_end") && r["ctx"] == "base");
            recs.remove(at);
        }),
        ("driven replay: commands: the sim ends command", |recs| {
            let at = pos(recs, |r| ev(r, "uc_end") && r["ctx"] == "base" && r["res"] == 2);
            recs[at]["res"] = json!(3);
        }),
        ("driven replay: commands: the running list's", |recs| {
            let at = pos(recs, |r| ev(r, "uc_rc") && r["ctx"] == "base");
            recs.remove(at);
        }),
        ("driven replay: commands: the sim's running list", |recs| {
            let at = pos(recs, |r| ev(r, "uc_rc") && r["ctx"] == "base" && r["op"] == "add");
            recs[at]["a"] = json!(7);
        }),
        // The controller's calls: a TickCommand, a unit's tick.
        ("driven replay: commands: TickCommand(", |recs| {
            let at = pos(recs, |r| ev(r, "uc_tc"));
            recs.remove(at);
        }),
        ("driven replay: commands: the controller's next unit", |recs| {
            let at = pos(recs, |r| ev(r, "uc_tick"));
            recs.remove(at);
        }),
        // A command acting: its SpendAction lost, changed, or one the sim's commands didn't make.
        ("driven replay: commands: a command's SpendAction", |recs| {
            let at = pos(recs, |r| {
                ev(r, "cd_spend") && r["caller"].as_str().unwrap().contains("UnitCommandController")
            });
            recs.remove(at);
        }),
        ("driven replay: commands: the sim's SpendAction", |recs| {
            let at = pos(recs, |r| {
                ev(r, "cd_spend") && r["caller"].as_str().unwrap().contains("UnitCommandController")
            });
            recs[at]["tsc"] = json!("0.5");
        }),
        ("driven replay: a command's SpendAction the sim's commands didn't make", |recs| {
            let at = pos(recs, |r| {
                ev(r, "cd_spend") && r["caller"].as_str().unwrap().contains("UnitCommandController")
            });
            let tick = recs[at]["t"].clone();
            let end = at + recs[at..].iter().position(|r| ev(r, "uc_ctl_end")).unwrap();
            let mut dup = recs[at].clone();
            dup["t"] = tick;
            recs.insert(end + 1, dup);
        }),
        // State rows: a command's, a set's, the running list's; malformed rows and descriptions.
        ("driven replay: the sim's commands differ from the recording's at", |recs| {
            let at = pos(recs, |r| ev(r, "uc_cs") && r["s"].as_str().unwrap().starts_with("11"));
            let s = recs[at]["s"].as_str().unwrap().replacen("|0|", "|1|", 1);
            recs[at]["s"] = json!(s);
        }),
        ("driven replay: the sim's commands differ from the recording's at", |recs| {
            let at =
                pos(recs, |r| ev(r, "uc_state") && r["s"].as_str().unwrap().contains("|-,-,-,-|"));
            let s = recs[at]["s"].as_str().unwrap().replace("|-,-,-,-|", "|-,-,-,77|");
            recs[at]["s"] = json!(s);
        }),
        ("driven replay: the sim's commands differ from the recording's at", |recs| {
            let at = pos(recs, |r| ev(r, "uc_running") && r["s"] != "-");
            recs[at]["s"] = json!("-");
        }),
        ("driven replay: a uc_new record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_new"));
            recs[at].as_object_mut().unwrap().remove("react");
        }),
        ("driven replay: a uc_cs record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_cs"));
            recs[at]["s"] = json!("000|0|NaN|000--|0|0|0|000|00|1||");
        }),
        ("driven replay: a uc_state record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_state"));
            recs[at]["s"] = json!("x");
        }),
        // A record of another frame; a caller that can't be placed; a set the sim doesn't hold.
        ("driven replay: commands: a command record (uc_v", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v"));
            recs[at].as_object_mut().unwrap().remove("t");
        }),
        ("driven replay: a command record (uc_run) of another frame", |recs| {
            let at = pos(recs, |r| ev(r, "uc_run") && r["nested"] == false);
            recs[at].as_object_mut().unwrap().remove("t");
        }),
        ("driven replay: commands: a call from a source the replay can't place", |recs| {
            let at = pos(recs, |r| ev(r, "uc_run") && r["nested"] == false);
            recs[at]["origin"] = json!("X.Y < Z.W");
        }),
        ("driven replay: commands: a unit's replaced set isn't the sim's", |recs| {
            let at = pos(recs, |r| ev(r, "uc_set"));
            recs[at]["from"] = json!(99_999);
        }),
        // The turn controller's and the leave controller's reads of the commands.
        ("driven replay: the turn controller's Tick's Commands.Empty differs", |recs| {
            let at = pos(recs, |r| ev(r, "tt") && r["r_empty"] == true);
            recs[at]["r_empty"] = json!(false);
        }),
        (
            "driven replay: the turn's running commands differ from the sim's at the status change",
            |recs| {
                let at = pos(recs, |r| ev(r, "tt_sc") && r["running"] == 0);
                recs[at]["running"] = json!(1);
            },
        ),
        ("driven replay: the turn controller's ContinueActing's IsRunning differs", |recs| {
            let at = pos(recs, |r| ev(r, "tt_ca"));
            let run = recs[at]["run"].as_bool().unwrap();
            recs[at]["run"] = json!(!run);
        }),
        (
            "driven replay: a unit's commands at the leave controller differ from the sim's",
            |recs| {
                let at = pos(recs, |r| ev(r, "cmds") && r["s"] != "/");
                recs[at]["s"] = json!("/");
            },
        ),
        // Review round 1: every field of the entries, ends, running-list calls, leaves and rows.
        ("driven replay: commands: a Run's parked or group command differs", |recs| {
            let at = pos(recs, |r| ev(r, "uc_run") && r["nested"] == false);
            recs[at]["prev"] = json!(99_999);
        }),
        ("driven replay: commands: the sim's running list", |recs| {
            let at = pos(recs, |r| ev(r, "uc_rc") && r["ctx"] == "base");
            recs[at]["b"] = json!(99_999);
        }),
        ("driven replay: a uc_end record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_end") && r["ctx"] == "other");
            recs[at]["raise"] = json!("yes");
        }),
        ("driven replay: commands: the sim reads untgt", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "untgt");
            recs[at]["src"] = json!("no-such-unit");
        }),
        ("driven replay: commands: a saddled record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "saddled");
            recs[at]["v"] = json!(7);
        }),
        ("driven replay: a uc_running record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_running"));
            recs[at]["s"] = json!("garbage");
        }),
        ("driven replay: a uc_state record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_state"));
            let s = recs[at]["s"].as_str().unwrap().replacen("|0|", "|00|", 1);
            recs[at]["s"] = json!(s);
        }),
        ("driven replay: a uc_cs record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_cs") && r["s"].as_str().unwrap().contains("|@"));
            let s = recs[at]["s"].as_str().unwrap();
            let (head, rest) = s.split_once("|@").unwrap();
            let tail = rest.split_once(',').unwrap().1;
            recs[at]["s"] = json!(format!("{head}|@NaN,{tail}"));
        }),
        ("driven replay: a uc_new record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_new"));
            recs[at]["type"] = json!(999);
        }),
        ("driven replay: a uc_new record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_new"));
            recs[at].as_object_mut().unwrap().remove("player");
        }),
        // Review round 2: classification metadata, a mutator's argument, a replacement's caller,
        // a row of an undescribed command.
        ("driven replay: commands: the sim ends command", |recs| {
            let at = pos(recs, |r| ev(r, "uc_end") && r["ctx"] == "base");
            recs[at]["origin"] = json!(7);
        }),
        ("driven replay: commands: the sim ends command", |recs| {
            let at = pos(recs, |r| ev(r, "uc_end") && r["ctx"] == "base");
            recs[at]["ctx"] = json!("garbage");
        }),
        ("driven replay: commands: the sim's running list", |recs| {
            let at = pos(recs, |r| ev(r, "uc_rc") && r["ctx"] == "base");
            recs[at]["ctx"] = json!("garbage");
        }),
        ("driven replay: commands: a uc_x record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_x") && r["fn"] == "InterruptAll");
            recs[at]["raise"] = json!("garbage");
        }),
        ("driven replay: commands: a call from a source the replay can't place", |recs| {
            let at = pos(recs, |r| ev(r, "uc_set"));
            recs[at]["origin"] = json!("X.Y < Z.W");
        }),
        ("driven replay: a uc_cs record of an undescribed command", |recs| {
            let at = recs.iter().rposition(|r| ev(r, "uc_cs")).unwrap();
            recs[at]["c"] = json!(99_999);
        }),
        // Review round 3: a row of a command no longer held, a set row of an unknown unit.
        ("driven replay: the sim's commands differ from the recording's at", |recs| {
            let at = recs.iter().rposition(|r| ev(r, "uc_ctl")).unwrap();
            let c = recs.iter().find(|r| ev(r, "uc_new")).unwrap()["c"].clone();
            let row = recs.iter().rev().find(|r| ev(r, "uc_cs") && r["c"] == c).unwrap().clone();
            let mut bad = row.clone();
            let s = row["s"].as_str().unwrap();
            let mut p: Vec<&str> = s.split('|').collect();
            p[2] = "123";
            bad["s"] = json!(p.join("|"));
            bad["t"] = recs[at]["t"].clone();
            recs.insert(at, bad);
        }),
        ("driven replay: a uc_state record of an unknown or other unit", |recs| {
            let at = pos(recs, |r| ev(r, "uc_state") && r["u"].is_string());
            let s = recs[at]["s"].as_str().unwrap();
            let rest = s.split_once('|').unwrap().1;
            recs[at]["s"] = json!(format!("no-such-unit|{rest}"));
            recs[at]["u"] = json!("no-such-unit");
        }),
        // Review round 4: CanAct as the game evaluated it; a new set's first row; a replacement's
        // unit and source set; rows after the last check; two rows of one command; an unknown executor.
        ("driven replay: commands: ", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "canact" && r["v"] == true);
            recs[at]["v"] = json!(false);
        }),
        ("driven replay: commands: a new command set without its first row", |recs| {
            let at = pos(recs, |r| ev(r, "uc_set") && r["sim"] == true);
            assert!(ev(&recs[at + 1], "uc_state"), "a new prediction set's row follows");
            recs.remove(at + 1);
        }),
        ("driven replay: a uc_set record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_set"));
            recs[at]["u"] = json!("no-such-unit");
        }),
        ("driven replay: a uc_set record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_set"));
            recs[at].as_object_mut().unwrap().remove("from");
        }),
        ("driven replay: the sim's commands differ from the recording's at", |recs| {
            let end = recs.iter().rposition(|r| ev(r, "uc_ctl_end")).unwrap();
            let row = recs.iter().rev().find(|r| ev(r, "uc_cs")).unwrap().clone();
            let mut bad = row.clone();
            let mut p: Vec<String> =
                row["s"].as_str().unwrap().split('|').map(str::to_owned).collect();
            p[2] = "123".into();
            bad["s"] = json!(p.join("|"));
            bad["t"] = recs[end]["t"].clone();
            recs.insert(end + 1, bad);
        }),
        ("driven replay: two uc_cs rows of one command between checks", |recs| {
            let at = recs.iter().rposition(|r| ev(r, "uc_cs")).unwrap();
            let dup = recs[at].clone();
            recs.insert(at, dup);
        }),
        ("driven replay: commands: a command of an unknown unit", |recs| {
            let at = pos(recs, |r| ev(r, "uc_new") && r["exec"].is_string());
            let e = recs[at]["exec"].as_str().unwrap().to_owned();
            recs[at]["exec"] = json!("no-such-unit");
            let s = recs[at]["s"].as_str().unwrap().replace(&format!("|{e}"), "|no-such-unit");
            recs[at]["s"] = json!(s);
        }),
        // Review round 5: a set or running row overwritten before a check; an item for a unit.
        ("driven replay: a uc_state record of an undescribed command", |recs| {
            let at = recs.iter().rposition(|r| ev(r, "uc_state") && r["u"].is_string()).unwrap();
            let mut bad = recs[at].clone();
            let s = bad["s"].as_str().unwrap().to_owned();
            let mut p: Vec<&str> = s.split('|').collect();
            p[3] = "999999";
            bad["s"] = json!(p.join("|"));
            recs.insert(at, bad);
        }),
        ("driven replay: the sim's commands differ from the recording's at a row", |recs| {
            let at = recs.iter().rposition(|r| ev(r, "uc_running")).unwrap();
            let mut bad = recs[at].clone();
            bad["s"] = json!(if recs[at]["s"] == "-" { "" } else { "-" });
            recs.insert(at, bad);
        }),
        ("driven replay: a uc_state record of an unknown or other unit", |recs| {
            // An item's id (an entity, not a unit) as a set's holder and owner.
            let item = recs
                .iter()
                .find_map(|r| r["fields"]["Weapon"]["$item"].as_str().map(str::to_owned))
                .expect("an item id");
            let at = pos(recs, |r| ev(r, "uc_state") && r["u"].is_string());
            let s = recs[at]["s"].as_str().unwrap();
            let rest = s.split_once('|').unwrap().1.to_owned();
            recs[at]["s"] = json!(format!("{item}|{rest}"));
            recs[at]["u"] = json!(item);
        }),
        // Review round 6: the snapshot against its descriptions and its roster; canMoveUnit where the
        // sim knows it; an external running-list add against the executor's combat state.
        (
            "driven replay: the commands snapshot's rows differ from the commands' descriptions",
            |recs| {
                let at = pos(recs, |r| ev(r, "uc_new"));
                let s = recs[at]["s"].as_str().unwrap().replacen('0', "1", 1);
                recs[at]["s"] = json!(s);
            },
        ),
        ("driven replay: the commands snapshot lacks a unit's command set", |recs| {
            let snap = pos(recs, |r| ev(r, "uc_snap"));
            let at =
                recs[..snap].iter().rposition(|r| ev(r, "uc_state") && r["u"].is_string()).unwrap();
            recs.remove(at);
        }),
        ("driven replay: commands: TickCommand(", |recs| {
            let at = pos(recs, |r| ev(r, "uc_tc") && r["canmove"] == true);
            recs[at]["canmove"] = json!(false);
        }),
        ("driven replay: commands: the running list's length differs", |recs| {
            // A remove of a running command (its executor in combat) recorded as an add that adds
            // nothing.
            let at = pos(recs, |r| {
                ev(r, "uc_rc") && r["ctx"] == "other" && r["op"] == "remove" && r["b"] == 1
            });
            recs[at]["op"] = json!("add");
            let b = recs[at]["b"].clone();
            recs[at]["a"] = b;
        }),
        // Review round 7: a new prediction set's first row naming another owner.
        (
            "driven replay: commands: a new prediction set's owner isn't the unit replacing its set",
            |recs| {
                let at = pos(recs, |r| ev(r, "uc_set") && r["sim"] == true);
                let other = recs
                    .iter()
                    .find(|r| ev(r, "uc_state") && r["u"].is_string() && r["u"] != recs[at]["u"])
                    .unwrap()["u"]
                    .clone();
                let s = recs[at + 1]["s"].as_str().unwrap();
                let rest = s.split_once('|').unwrap().1.to_owned();
                recs[at + 1]["s"] = json!(format!("{}|{rest}", other.as_str().unwrap()));
            },
        ),
        // Review round 9: class code other than Init changing the executor.
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, |r| ev(r, "uc_fx") && r["fn"] == "OnTick");
            let s = recs[at]["s"].as_str().unwrap().to_owned();
            let exec = s.rsplit('|').next().unwrap().to_owned();
            let other = recs
                .iter()
                .find(|r| ev(r, "uc_state") && r["u"].is_string() && r["u"] != exec.as_str())
                .unwrap()["u"]
                .as_str()
                .unwrap()
                .to_owned();
            let head = s.rsplit_once('|').unwrap().0;
            recs[at]["s"] = json!(format!("{head}|{other}"));
        }),
        // An end outside the lifecycle (the movement agent's) of an unknown command, or twice.
        ("driven replay: commands: an end of", |recs| {
            let at = pos(recs, |r| ev(r, "uc_end") && r["ctx"] == "other");
            recs[at]["c"] = json!(99_999);
        }),
    ];
    for (want, change) in cases {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
        change(&mut changed.records);
        let _ = replay::run_with_tape(&mut w, &changed, &trace);
        assert!(!w.valid(), "{want}");
        assert!(
            w.coverage.unsupported.keys().any(|k| k.starts_with(want)),
            "{want}: {:?}",
            w.coverage.unsupported.keys().collect::<Vec<_>>()
        );
    }
}

/// An addition to a rule's bonuses the rule never asked for, which its non-stacking container
/// rejected (so the bonuses themselves are unchanged): the sim's additions differ in T0
/// (`poc5-tb-77/tb-nearest`, an enhancement bonus; slice B review round 39).
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-77 capture"]
fn a_bonus_addition_the_rule_never_asked_for_is_unequal() {
    let Some(dir) = fixture_in("poc5-tb-77", "tb-nearest") else {
        eprintln!("poc5-tb-77/tb-nearest not present: skipped");
        return;
    };
    let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
    let mut recs = trace.records.clone();
    let n = recs
        .iter()
        .position(|r| r["k"] == "mut" && r["what"] == "rule_bonus" && r["descriptor"] == 9)
        .expect("an enhancement bonus");
    let mut m = recs[n].clone();
    m["value"] = json!(-777);
    m["fact"] = json!(null);
    recs.insert(n + 1, m);
    for (i, r) in recs.iter_mut().enumerate() {
        r["seq"] = json!(i);
    }
    let changed = Trace { records: recs, sites: trace.sites.clone() };
    let out = replay::run_with_tape(&mut w, &changed, &trace);
    assert!(!out.diff.equal, "the replay equal to the damaged recording");
}

/// `poc5-tb-77`'s `UnitAttack` records damaged (`docs/poc5/commands.md`, slice B): a lost, unreadable
/// or wrong attack list, class state, leaf, description or root rule is reported, never taken.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-77 capture"]
fn missing_or_wrong_attack_records_are_reported() {
    let Some(dir) = fixture_in("poc5-tb-77", "tb-focus") else {
        eprintln!("poc5-tb-77/tb-focus not present: skipped");
        return;
    };
    type Change = fn(&mut Vec<serde_json::Value>);
    fn pos(recs: &[serde_json::Value], f: impl Fn(&serde_json::Value) -> bool) -> usize {
        recs.iter().position(f).expect("a record to change")
    }
    fn ev(r: &serde_json::Value, e: &str) -> bool {
        r["k"] == "obs" && r["ev"] == e
    }
    /// A root rule of `UnitAttack`'s method `m`.
    fn root_of(r: &serde_json::Value, class: &str, m: &str) -> bool {
        r["k"] == "rule"
            && r["depth"] == 0
            && r["class"].as_str().is_some_and(|c| c.ends_with(class))
            && r["origin"].as_str().is_some_and(|o| o.contains(&format!("UnitAttack.{m}")))
    }
    /// A full attack's list (two attacks or more).
    fn full_list(r: &serde_json::Value) -> bool {
        ev(r, "ua_atk") && r["list"].as_array().is_some_and(|l| l.len() >= 2)
    }
    /// Renumbers the trace's sequence after records were moved.
    fn renumber(recs: &mut [serde_json::Value]) {
        for (n, r) in recs.iter_mut().enumerate() {
            r["seq"] = json!(n);
        }
    }
    /// The first attack's first global handler (`ShadowSpellController`) that changed nothing: its
    /// `h` record, its `h_end` next.
    fn shadow_handler(recs: &[serde_json::Value]) -> usize {
        let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
        start
            + (start..recs.len() - 1)
                .position(|i| {
                    recs[i]["k"] == "h"
                        && recs[i]["class"]
                            .as_str()
                            .is_some_and(|c| c.ends_with("ShadowSpellController"))
                        && recs[i + 1]["k"] == "h_end"
                        && recs[i + 1]["changes"] == json!([])
                })
                .expect("a handler to remove")
    }
    /// Inserts the trace's first keyed (global hook) draw, with `key`, into the first attack's rule
    /// after its `trig`.
    fn insert_keyed_draw(recs: &mut Vec<serde_json::Value>, key: &str) {
        let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
        let id = recs[start]["id"].clone();
        let t = start + pos(&recs[start..], |r| r["k"] == "trig" && r["id"] == id);
        let mut d = recs[pos(recs, |r| r["k"] == "draw" && r["site"] == -1)].clone();
        d["t"] = recs[t]["t"].clone();
        d["rule"] = id;
        d["h"] = json!(0);
        d["key"] = json!(key);
        recs.insert(t + 1, d);
        renumber(recs);
    }
    /// Inserts a mutation (`payload` of the attacker) into the first attack's rule, where its
    /// bonuses are written (`OnTrigger`).
    fn insert_mut(
        recs: &mut Vec<serde_json::Value>,
        payload: fn(serde_json::Value) -> serde_json::Value,
    ) {
        let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
        let n = start + pos(&recs[start..], |r| r["k"] == "mut" && r["what"] == "rule_bonus");
        let mut m = payload(recs[start]["initiator"]["$unit"].clone());
        for k in ["seq", "k", "t", "rule", "h"] {
            m[k] = recs[n][k].clone();
        }
        recs.insert(n + 1, m);
        renumber(recs);
    }
    fn set_entry(recs: &mut [serde_json::Value], at: usize, k: usize, v: &str) {
        let e = recs[at]["list"][0].as_str().unwrap().to_owned();
        let mut p: Vec<String> = e.split(',').map(str::to_owned).collect();
        p[k] = v.to_owned();
        recs[at]["list"][0] = json!(p.join(","));
    }
    let cases: [(&str, Change); 202] = [
        // The attack list: lost, of another command or frame, an exception, unreadable.
        ("driven replay: commands: InitAttacks' list of command", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            recs.remove(at);
        }),
        ("driven replay: commands: the sim's InitAttacks of command", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            recs[at]["c"] = json!(99_999);
        }),
        ("driven replay: commands: a command record (ua_atk", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            recs[at]["t"] = json!(recs[at]["t"].as_i64().unwrap() + 1);
        }),
        ("driven replay: commands: an exception out of InitAttacks", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            recs[at]["exc"] = json!(true);
        }),
        ("driven replay: commands: a ua_atk record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            recs[at]["full"] = json!("yes");
        }),
        ("driven replay: commands: a ua_atk record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            recs[at]["list"][0] = json!("x");
        }),
        ("driven replay: commands: a ua_atk record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            recs[at]["dur"] = json!("NaN");
        }),
        // ... or other than the sim's: flags, IsAttackFull, the attacks, a weapon, the duration.
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            recs[at]["flags"] = json!("10000");
        }),
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            recs[at]["full"] = json!(!recs[at]["full"].as_bool().unwrap());
        }),
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            set_entry(recs, at, 4, "15");
        }),
        ("driven replay: commands: command", |recs| {
            let at =
                pos(recs, |r| ev(r, "ua_atk") && r["list"][0].as_str().unwrap().starts_with('p'));
            set_entry(recs, at, 1, "not-an-item");
        }),
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, full_list);
            recs[at]["list"].as_array_mut().unwrap().pop();
        }),
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            recs[at]["dur"] = json!("5");
        }),
        // The class state after a call: another index, lost; a description without its flags,
        // or with them on another class.
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, |r| ev(r, "uc_fx") && r["ua"].is_string() && r["fn"] == "OnTick");
            let ua = recs[at]["ua"].as_str().unwrap().to_owned();
            recs[at]["ua"] = json!(format!("7{}", &ua[1..]));
        }),
        (
            "driven replay: commands: a uc_fx record whose UnitAttack state (ua) doesn't fit",
            |recs| {
                let at = pos(recs, |r| ev(r, "uc_fx") && r["ua"].is_string());
                recs[at].as_object_mut().unwrap().remove("ua");
            },
        ),
        (
            "driven replay: a uc_new record whose UnitAttack flags (ua) don't fit its class",
            |recs| {
                let at = pos(recs, |r| ev(r, "uc_new") && r["class"] == "UnitAttack");
                recs[at].as_object_mut().unwrap().remove("ua");
            },
        ),
        (
            "driven replay: a uc_new record whose UnitAttack flags (ua) don't fit its class",
            |recs| {
                let at = pos(recs, |r| ev(r, "uc_new") && r["class"] == "UnitMoveTo");
                recs[at]["ua"] = json!("00000");
            },
        ),
        ("driven replay: UnitAttack flags (ua) without the attacks observer", |recs| {
            let at = pos(recs, |r| r["k"] == "begin");
            let o = recs[at]["observers"].as_array_mut().unwrap();
            o.retain(|x| x != "attacks");
        }),
        // The class state after OnTick: the target changed.
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, |r| ev(r, "uc_fx") && r["ua"].is_string() && r["fn"] == "OnTick");
            let s = recs[at]["s"].as_str().unwrap().to_owned();
            let p: Vec<&str> = s.split('|').collect();
            let (_, wrapper) = p[10].rsplit_once('#').unwrap();
            let mut p: Vec<String> = p.iter().map(|x| (*x).to_owned()).collect();
            p[10] = format!("FFFF#{wrapper}");
            recs[at]["s"] = json!(p.join("|"));
        }),
        // UnitAttack's leaves: of another unit, unreadable, inconsistent.
        ("driven replay: commands: the sim reads five", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "five");
            recs[at]["u"] = json!("FFFF");
        }),
        ("driven replay: commands: the sim reads style", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "style");
            recs[at]["v"] = json!("bow");
        }),
        ("driven replay: commands: the sim reads enemies", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "enemies");
            recs[at]["v"] = json!(3);
        }),
        ("driven replay: commands: UpdateTarget: the sim's candidates", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "sorted");
            recs[at]["v"].as_array_mut().unwrap().push(json!("FFFF"));
        }),
        ("driven replay: commands: IsFullAttackRestrictedBecauseOfMoveAction", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "efa");
            let farm = at + 1;
            assert_eq!(recs[farm]["f"], "farm");
            recs[farm]["v"] = json!(!recs[farm]["v"].as_bool().unwrap());
        }),
        ("driven replay: commands: ", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "farm");
            recs.remove(at);
        }),
        // The root rules: the attack's fields, call site, initiator; the count's fields.
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["fields"]["AttackNumber"] = json!(5);
        }),
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["fields"]["Weapon"]["$item"] = json!("not-an-item");
            recs[at]["fields"]["Weapon"]["blueprint"] = json!("0");
        }),
        ("driven replay: commands: the sim's command", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["origin"] = json!("Kingmaker.UnitLogic.Commands.UnitAttack.OnAction");
        }),
        (
            "driven replay: commands: a RuleCalculateAttacksCount of UnitAttack with fields",
            |recs| {
                let at = pos(recs, |r| root_of(r, "RuleCalculateAttacksCount", "InitAttacks"));
                recs[at]["fields"]["ForceIterativeNaturalAttacks"] = json!(true);
            },
        ),
        // Review round 1: every field of a command-driven attack root, its weapon and slot.
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["fields"]["SkipMainDamage"] = json!(true);
        }),
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["fields"]["ExtraAttack"] = json!(true);
        }),
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["fields"].as_object_mut().unwrap().remove("Target");
        }),
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["fields"]["SkipAnimation"] = json!("false");
        }),
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["fields"]["Weapon"]["blueprint"] = json!("wrong");
        }),
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["fields"]["WeaponSlot"]["m_ItemRef"]["$entity"] = json!("another-item");
        }),
        // ... the count roots' explicit nulls, the roots' metadata and call chain, their spans.
        (
            "driven replay: commands: a RuleCalculateAttacksCount of UnitAttack with fields",
            |recs| {
                let at = pos(recs, |r| root_of(r, "RuleCalculateAttacksCount", "InitAttacks"));
                recs[at]["fields"].as_object_mut().unwrap().remove("ForcePrimaryAttackWeapon");
            },
        ),
        (
            "driven replay: commands: a RuleCalculateAttacksCountLimbs of UnitAttack with fields",
            |recs| {
                let at = pos(recs, |r| root_of(r, "RuleCalculateAttacksCountLimbs", "InitAttacks"));
                recs[at]["fields"]["WeaponFilter"] = json!(0);
            },
        ),
        ("driven replay: commands: the sim's command", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["parent"] = json!(12_345);
        }),
        ("driven replay: commands: the sim's command", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["context_parent"] = json!(12_345);
        }),
        ("driven replay: commands: the sim's command", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let o = recs[at]["origin"].as_str().unwrap().to_owned();
            let (head, _) =
                o.split_once(" < Kingmaker.UnitLogic.Commands.Base.UnitCommand.Tick").unwrap();
            recs[at]["origin"] = json!(format!("{head} < Fake.Unknown.Root"));
        }),
        ("driven replay: commands: the sim's command", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleCalculateAttacksCount", "InitAttacks"));
            recs[at].as_object_mut().unwrap().remove("target");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let id = recs[start]["id"].clone();
            let end = start
                + recs[start..].iter().position(|r| r["k"] == "rule_end" && r["id"] == id).unwrap();
            recs[end]["t"] = json!(recs[end]["t"].as_i64().unwrap() + 7);
        }),
        // ... a recorded call's class state, ConfigureAnimations' gate.
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, |r| {
                ev(r, "uc_fx") && r["ua"].is_string() && r["fn"] == "TickApproaching"
            });
            recs[at]["ua"] = json!("garbage");
        }),
        ("driven replay: commands: a ua_atk record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            recs[at]["cfg"] = json!(1);
        }),
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk") && r["cfg"] == true);
            recs[at]["cfg"] = json!(false);
        }),
        // Review round 2: the slot's whole snapshot, the span's identities, the chain to the entry,
        // the list's order without ConfigureAnimations, impossible flags, the style's range, an attack
        // initialized before the window.
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["fields"]["WeaponSlot"]["Disabled"] = json!("garbage");
        }),
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["fields"]["WeaponSlot"].as_object_mut().unwrap().remove("ParentFact");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let h = start + recs[start..].iter().position(|r| r["k"] == "h").unwrap();
            recs[h]["rule"] = json!(99_999);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + 1 + recs[start + 1..].iter().position(|r| r["k"] == "rule").unwrap();
            recs[n]["parent"] = json!(99_999);
        }),
        ("driven replay: commands: the sim's command", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let o = recs[at]["origin"].as_str().unwrap().to_owned();
            let k =
                o.find("Kingmaker.Controllers.Units.UnitCommandController.TickCommand").unwrap();
            recs[at]["origin"] = json!(format!("{}Fake.Unknown.Caller", &o[..k]));
        }),
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, |r| full_list(r) && r["cfg"] == true);
            recs[at]["cfg"] = json!(false);
            let l = recs[at]["list"].as_array_mut().unwrap();
            for e in l.iter_mut() {
                let x = e.as_str().unwrap().to_owned();
                *e = json!(format!("{}001", &x[..x.len() - 3]));
            }
            l.reverse();
        }),
        ("driven replay: commands: a ua_atk record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            let x = recs[at]["list"][0].as_str().unwrap().to_owned();
            recs[at]["list"][0] = json!(format!("{}000", &x[..x.len() - 3]));
        }),
        ("driven replay: commands: the sim reads style", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "style");
            recs[at]["v"] = json!(i64::MAX);
        }),
        ("driven replay: commands: a UnitAttack initialized before its first sight", |recs| {
            let at = pos(recs, |r| ev(r, "uc_new") && r["class"] == "UnitAttack");
            let exec =
                recs.iter().find(|r| ev(r, "uc_state") && r["u"].is_string()).unwrap()["u"].clone();
            recs[at]["exec"] = exec.clone();
            let s = recs[at]["s"].as_str().unwrap().to_owned();
            let head = s.rsplit_once('|').unwrap().0;
            recs[at]["s"] = json!(format!("{head}|{}", exec.as_str().unwrap()));
        }),
        // Review round 3: the span's attribution, CountableFlag's invariant, a reorder without a
        // sequence, a reused wrapper after a retarget.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + 1 + recs[start + 1..].iter().position(|r| r["k"] == "rule").unwrap();
            recs[n]["context_parent"] = json!(99_999);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + 1 + recs[start + 1..].iter().position(|r| r["k"] == "rule").unwrap();
            recs[n]["handler"] = json!(99_999);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "mut").unwrap();
            recs[n]["rule"] = json!(99_999);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "draw").unwrap();
            recs[n]["ctx"] = json!(null);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "h").unwrap();
            recs[n].as_object_mut().unwrap().remove("class");
        }),
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[at]["fields"]["WeaponSlot"]["Lock"] =
                json!({"m_Count": i64::MAX, "Count": i64::MAX, "Value": false});
        }),
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, |r| full_list(r) && r["cfg"] == true);
            recs[at]["list"].as_array_mut().unwrap().reverse();
        }),
        ("driven replay: commands: command", |recs| {
            // The first OnTick whose target wrapper differs from its command's last row.
            let mut last = std::collections::HashMap::new();
            let mut hit = None;
            for (n, r) in recs.iter().enumerate() {
                if !(ev(r, "uc_fx") || ev(r, "uc_cs") || ev(r, "uc_new")) {
                    continue;
                }
                let t = r["s"].as_str().unwrap().split('|').nth(10).unwrap().to_owned();
                let c = r["c"].as_u64().unwrap();
                if ev(r, "uc_fx")
                    && r["fn"] == "OnTick"
                    && r["ua"].is_string()
                    && let Some(old) = last.get(&c).filter(|o: &&String| **o != t)
                {
                    hit = Some((n, old.clone()));
                    break;
                }
                last.insert(c, t);
            }
            let (n, old) = hit.unwrap();
            let s = recs[n]["s"].as_str().unwrap().to_owned();
            let mut p: Vec<String> = s.split('|').map(str::to_owned).collect();
            let unit = p[10].split('#').next().unwrap().to_owned();
            p[10] = format!("{unit}#{}", old.split('#').nth(1).unwrap());
            recs[n]["s"] = json!(p.join("|"));
        }),
        // Review round 4: each rule's steps, handler ends and phases, draw contexts, environments,
        // diffs.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs.remove(start + 1);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let id = recs[start]["id"].clone();
            let n = start
                + recs[start..].iter().position(|r| r["k"] == "trig" && r["id"] == id).unwrap();
            recs.remove(n);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let (t, id) = (recs[start]["t"].clone(), recs[start]["id"].clone());
            recs.insert(
                start + 2,
                json!({"k": "h_end", "t": t, "id": 0, "rule": id, "changes": []}),
            );
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "draw").unwrap();
            let c = recs[n]["ctx"].as_str().unwrap().split('|').next().unwrap().to_owned();
            recs[n]["ctx"] = json!(format!("{c}|wrong-unit|wrong-target|999"));
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "env").unwrap();
            recs[n]["rule"] = json!("FakeRule");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "env" && r.get("at").is_some())
                    .unwrap();
            recs[n].as_object_mut().unwrap().remove("at");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "h").unwrap();
            recs[n]["phase"] = json!("nonsense");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1]["changes"] = json!("garbage");
        }),
        // Review round 5: environments' schemas, handler identities, nested snapshots, diff values,
        // bonuses, a foreign record, the limb's item identity.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "env" && r["rule"] == "RuleAttackRoll")
                    .unwrap();
            recs[n]["at"] = json!({"junk": [0]});
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "env" && r["rule"] == "RuleAttackRoll")
                    .unwrap();
            recs[n].as_object_mut().unwrap().remove("target_engaged_by");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "env" && r["rule"] == "RuleAttackRoll")
                    .unwrap();
            recs[n]["at"]["initiator_pos"][0] = json!(1e100);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "h" && r.get("component").is_some())
                    .unwrap();
            recs[n]["class"] = json!("Fake.Handler");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "h" && r.get("fact").is_some())
                    .unwrap();
            recs[n].as_object_mut().unwrap().remove("fact");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + 1 + recs[start + 1..].iter().position(|r| r["k"] == "rule").unwrap();
            recs[n].as_object_mut().unwrap().remove("fields");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + 1 + recs[start + 1..].iter().position(|r| r["k"] == "rule").unwrap();
            recs[n]["reason_at_push"] = json!(77);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1]["changes"]
                .as_array_mut()
                .unwrap()
                .push(json!(["DisableBattleLog", 77]));
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1]["changes"].as_array_mut().unwrap().push(json!(["NoSuchField", 1]));
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let id = recs[start]["id"].clone();
            let end = start
                + recs[start..].iter().position(|r| r["k"] == "rule_end" && r["id"] == id).unwrap();
            recs[end]["bonuses"] = json!([77]);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let t = recs[start]["t"].clone();
            recs.insert(start + 1, json!({"k": "result", "t": t}));
        }),
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| {
                root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule")
                    && r["fields"]["WeaponSlot"]["ParentFact"]["IsNull"] == false
            });
            recs[at]["fields"]["WeaponSlot"]["m_ItemRef"]["$entity"] = json!("unrelated");
        }),
        // Review round 6: snapshots' and diffs' base flags, mutation kinds and payloads, reasons'
        // fields, environments' completeness, ids out of order, origin methods, global handlers.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + 1 + recs[start + 1..].iter().position(|r| r["k"] == "rule").unwrap();
            recs[n]["fields"]["DisableBattleLog"] = json!("garbage");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1]["changes"]
                .as_array_mut()
                .unwrap()
                .push(json!(["DisableBattleLog", null]));
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "mut").unwrap();
            recs[n]["what"] = json!("not-a-real-mutation");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "mut" && r["what"] == "rule_bonus")
                    .unwrap();
            recs[n]["value"] = json!({"x": 1});
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1]["reason"]["ability"] = json!({"junk": true});
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1]["reason"]["fact"] = json!("not-a-fact");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "env" && r["rule"] == "RuleAttackRoll")
                    .unwrap();
            recs.remove(n);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "env" && r["rule"] == "RuleAttackRoll")
                    .unwrap();
            let e = recs[n].clone();
            recs.insert(n, e);
        }),
        ("driven replay: a h record whose id breaks the allocation order", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "h").unwrap();
            let id = recs[n]["id"].clone();
            let e = n + recs[n..].iter().position(|r| r["k"] == "h_end" && r["id"] == id).unwrap();
            recs[n]["id"] = json!(1);
            recs[e]["id"] = json!(1);
        }),
        ("driven replay: commands: the sim's command", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let o = recs[at]["origin"].as_str().unwrap().to_owned();
            let o = o.replacen(
                "Kingmaker.UnitLogic.Commands.Base.UnitCommand.Tick < ",
                "Kingmaker.UnitLogic.Commands.Base.UnitCommand.Tick < Kingmaker.UnitLogic.Commands.Base.UnitCommand.DoesNotExist < ",
                1,
            );
            recs[at]["origin"] = json!(o);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "h" && r.get("component").is_none())
                    .unwrap();
            recs[n]["class"] = json!("Fake.NonexistentHandler");
        }),
        // Review round 7: draws' payloads, UnitAttack's frames, the slot reference's class, a field twice
        // in a diff, a component handler's class.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "draw").unwrap();
            recs[n]["c"] = json!("garbage");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "draw").unwrap();
            recs[n]["a"].as_array_mut().unwrap().push(json!("garbage"));
        }),
        ("driven replay: commands: the sim's command", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let o = recs[at]["origin"].as_str().unwrap().replacen(
                "Kingmaker.UnitLogic.Commands.UnitAttack.ProvideNextAttack < ",
                "Kingmaker.UnitLogic.Commands.UnitAttack.ProvideNextAttack < Kingmaker.UnitLogic.Commands.UnitAttack.DoesNotExist < ",
                1,
            );
            recs[at]["origin"] = json!(o);
        }),
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| {
                root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule")
                    && r["fields"]["WeaponSlot"]["m_ItemRef"].is_object()
            });
            recs[at]["fields"]["WeaponSlot"]["m_ItemRef"]["class"] =
                json!("ItemEntityDoesNotExist");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| {
                        r["k"] == "h_end" && r["changes"].as_array().is_some_and(|c| !c.is_empty())
                    })
                    .unwrap();
            let first = recs[n]["changes"][0].clone();
            recs[n]["changes"].as_array_mut().unwrap().push(first);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "h" && r.get("component").is_some())
                    .unwrap();
            let short = recs[n]["class"].as_str().unwrap().rsplit('.').next().unwrap().to_owned();
            recs[n]["class"] = json!(format!("Fake.{short}"));
        }),
        // Review round 8: draws' numeric domains, caller edges, nested rule classes, a bonus's target, C#
        // widths, the AllBonuses exception, sequence numbers.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "draw" && r["m"] == "Range_ii")
                    .unwrap();
            recs[n]["v"] = json!(4.5);
        }),
        ("driven replay: commands: the sim's command", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let o = recs[at]["origin"].as_str().unwrap().replacen(
                "Kingmaker.UnitLogic.Commands.Base.UnitCommand.Tick < ",
                "Kingmaker.UnitLogic.Commands.Base.UnitCommand.Tick < Kingmaker.UnitLogic.Commands.UnitCommands.AddToQueueInternal < ",
                1,
            );
            recs[at]["origin"] = json!(o);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + 1
                + recs[start + 1..]
                    .iter()
                    .position(|r| {
                        r["k"] == "rule" && r["class"] == "Kingmaker.RuleSystem.Rules.RuleRollD20"
                    })
                    .unwrap();
            recs[n]["class"] = json!("Fake.RuleRollD20");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "mut" && r["what"] == "rule_bonus")
                    .unwrap();
            recs[n]["target_rule"] = json!(999_999);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "mut" && r["what"] == "rule_bonus")
                    .unwrap();
            recs[n]["value"] = json!(i64::MAX);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + 1
                + recs[start + 1..]
                    .iter()
                    .position(|r| {
                        r["k"] == "rule" && r["class"] == "Kingmaker.RuleSystem.Rules.RuleRollD20"
                    })
                    .unwrap();
            recs[n]["fields"].as_object_mut().unwrap().remove("AllBonuses");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1].as_object_mut().unwrap().remove("seq");
        }),
        // Review round 9: failed-tape metadata, step and gap bounds, observations' sequence numbers.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..].iter().position(|r| r["k"] == "draw" && r["c"] == "sim").unwrap();
            recs[n]["forced"] = json!(true);
            recs[n]["match"] = json!(false);
            recs[n]["fallback"] = json!(true);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "draw").unwrap();
            recs[n]["steps"] = json!(35_184_372_088_832_i64);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "draw").unwrap();
            recs[n]["gap"] = json!(5000);
        }),
        ("driven replay: a record without its sequence number, or out of order", |recs| {
            let at = pos(recs, |r| ev(r, "ua_atk"));
            recs[at].as_object_mut().unwrap().remove("seq");
        }),
        // Review round 10: engagement ids, the triggered flags at a rule's end, ids' width.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "env" && r["rule"] == "RuleAttackRoll")
                    .unwrap();
            recs[n]["target_engaged_by"].as_array_mut().unwrap().push(json!("NOT_A_UNIT"));
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let id = recs[start]["id"].clone();
            let end = start
                + recs[start..].iter().position(|r| r["k"] == "rule_end" && r["id"] == id).unwrap();
            for c in recs[end]["changes"].as_array_mut().unwrap() {
                if c[0] == "m_Triggered" || c[0] == "IsTriggered" {
                    c[1] = json!(false);
                }
            }
        }),
        ("driven replay: a rule record whose id breaks the allocation order", |recs| {
            for r in recs.iter_mut().filter(|r| r["k"] == "rule") {
                r["id"] = json!(r["id"].as_i64().unwrap() + (1_i64 << 40));
            }
        }),
        // Review round 11: reasons' rule references, the triggered flags before the end, global
        // handlers' rules, handler owners by kind.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1]["reason"]["rule"] = json!(999_999);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1]["changes"].as_array_mut().unwrap().push(json!(["m_Triggered", true]));
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let id = recs[start]["id"].clone();
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| {
                        r["k"] == "h"
                            && r["rule"] == id
                            && r["class"] == "Kingmaker.Controllers.ShadowSpellController"
                    })
                    .unwrap();
            recs[n]["class"] = json!(
                "Kingmaker.Achievements.Logic.AchievementLogicMassiveDamageMountedChargeKill"
            );
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let n = pos(recs, |r| {
                r["k"] == "h"
                    && r["owner"].get("$item").is_some()
                    && r["class"].as_str().unwrap().ends_with("WeaponMaterial")
            });
            recs[n]["owner"]["blueprint"] = json!("not-a-blueprint");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let n = pos(recs, |r| {
                r["k"] == "h"
                    && r["owner"].get("$item").is_some()
                    && r["class"].as_str().unwrap().ends_with("WeaponMaterial")
            });
            let id = recs[n]["owner"]["$item"].clone();
            recs[n]["owner"] = json!({"$unit": id});
        }),
        // Review round 12: a component handler on a rule it doesn't handle, an enemy the sim lacks, a
        // reason's caster, records out of their lifecycle phase.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "h" && r.get("component").is_some())
                    .unwrap();
            // Another component handler's identity, of a component of another class.
            let class = recs[n]["class"].clone();
            let other = recs
                .iter()
                .find(|r| r["k"] == "h" && r.get("component").is_some() && r["class"] != class)
                .unwrap()
                .clone();
            for k in ["class", "component", "blueprint", "fact", "owner"] {
                recs[n][k] = other[k].clone();
            }
        }),
        (
            "driven replay: commands: UnitAttack.UpdateTarget: an enemy in the memory the sim doesn't have",
            |recs| {
                let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "enemies");
                recs[at]["v"].as_array_mut().unwrap().push(json!("no-such-unit"));
            },
        ),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1]["reason"]["caster"] = json!({"$unit": "no-such-unit"});
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "env" && r["rule"] == "RuleAttackRoll")
                    .unwrap();
            let roll = (start..n)
                .rev()
                .find(|i| {
                    recs[*i]["k"] == "rule"
                        && recs[*i]["class"].as_str().unwrap().ends_with("RuleAttackRoll")
                })
                .unwrap();
            let id = recs[roll]["id"].clone();
            let did = n + recs[n..].iter().position(|r| r["k"] == "did" && r["id"] == id).unwrap();
            let e = recs.remove(n);
            recs.insert(did, e);
            renumber(recs);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..].iter().position(|r| r["k"] == "draw" && r["c"] == "sim").unwrap();
            let rule = recs[n]["rule"].clone();
            let did =
                n + recs[n..].iter().position(|r| r["k"] == "did" && r["id"] == rule).unwrap();
            let d = recs.remove(n);
            recs.insert(did, d);
            renumber(recs);
        }),
        // Review round 13: a target-event subscriber on another rule, reasons' sources, concealment's
        // domain, nested rules' units.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleCalculateAttacksCount", "InitAttacks"));
            let id = recs[start]["id"].clone();
            let t = recs[start]["t"].clone();
            let about = start
                + recs[start..].iter().position(|r| r["k"] == "about" && r["id"] == id).unwrap();
            recs.insert(
                about + 1,
                json!({"k": "h", "t": t, "id": 900_000, "rule": id, "phase": "about",
                "class": "Kingmaker.Controllers.ShadowSpellController"}),
            );
            recs.insert(
                about + 2,
                json!({"k": "h_end", "t": t, "id": 900_000, "rule": id, "changes": []}),
            );
            renumber(recs);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1]["reason"]["item"] = json!("no-such-item");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1]["reason"]["fact"] = json!({"$fact": "no-such-fact", "blueprint": "0"});
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "env" && r["rule"] == "RuleAttackRoll")
                    .unwrap();
            recs[n]["concealment_initiator_from_target"] = json!(-1);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + 1
                + recs[start + 1..]
                    .iter()
                    .position(|r| r["k"] == "rule" && !r["initiator"].is_null())
                    .unwrap();
            recs[n]["initiator"] = json!({"$unit": "no-such-unit"});
        }),
        // Review round 14: a bonus's fact the sim hasn't.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let id = recs[start]["id"].clone();
            let end = start
                + recs[start..].iter().position(|r| r["k"] == "rule_end" && r["id"] == id).unwrap();
            recs[end]["bonuses"]
                .as_array_mut()
                .unwrap()
                .push(json!({"value": 1, "descriptor": 0, "type": 0,
                "stat": 0, "dice": null, "fact": {"$fact": "no-such-fact", "blueprint": "0"}}));
        }),
        // Review round 15: the root's assigned reason, mutations' identities, exact rule types, the
        // trace's mode.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1].as_object_mut().unwrap().remove("reason");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            recs[start + 1]["reason"] = json!(null);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let earlier =
                recs[..start].iter().rev().find(|r| r["k"] == "rule").unwrap()["id"].clone();
            recs[start + 1]["reason"]["rule"] = earlier;
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "mut" && r["what"] == "rule_bonus")
                    .unwrap();
            recs[n]["fact"] = json!({"$fact": "FFFF", "blueprint": "bad"});
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + 1
                + recs[start + 1..]
                    .iter()
                    .position(|r| {
                        r["k"] == "rule"
                            && r["class"] == "Kingmaker.RuleSystem.Rules.RuleCalculateWeaponStats"
                    })
                    .unwrap();
            recs[n]["class"] = json!("Kingmaker.RuleSystem.Rules.Damage.RuleCalculateWeaponStats");
        }),
        ("driven replay: a trace whose begin record has no mode it can read", |recs| {
            let at = pos(recs, |r| r["k"] == "begin");
            recs[at]["mode"] = json!("garbage");
        }),
        // Review round 16: a nested reason's blueprints.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + 2 + recs[start + 2..].iter().position(|r| r["k"] == "about").unwrap();
            recs[n]["reason"]["ability"] = json!("not-a-blueprint");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + 2 + recs[start + 2..].iter().position(|r| r["k"] == "about").unwrap();
            recs[n]["reason"]["context_blueprint"] = json!("not-a-blueprint");
        }),
        // Review round 17: a rule's end bonuses against its AllBonuses.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| {
                        r["k"] == "rule_end"
                            && r["bonuses"].as_array().is_some_and(|b| !b.is_empty())
                    })
                    .unwrap();
            recs[n]["bonuses"][0]["value"] = json!(123_456);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| {
                        r["k"] == "rule_end"
                            && r["bonuses"].as_array().is_some_and(|b| !b.is_empty())
                    })
                    .unwrap();
            recs[n]["bonuses"] = json!([]);
        }),
        // Review round 18: bonus mutations against the rule's bonuses, a snapshot's target.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "mut" && r["what"] == "rule_bonus")
                    .unwrap();
            recs[n]["value"] = json!(123_456);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| r["k"] == "mut" && r["what"] == "rule_bonus")
                    .unwrap();
            recs.remove(n);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + 1
                + recs[start + 1..]
                    .iter()
                    .position(|r| {
                        r["k"] == "rule"
                            && r["class"] == "Kingmaker.RuleSystem.Rules.RuleAttackRoll"
                    })
                    .unwrap();
            recs[n]["fields"]["Target"] = json!({"$unit": "no-such-unit"});
        }),
        // Review round 19: a target event's Target changed by a diff.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let id = recs[start]["id"].clone();
            let end = start
                + recs[start..].iter().position(|r| r["k"] == "rule_end" && r["id"] == id).unwrap();
            let other = recs[start]["initiator"].clone();
            recs[end]["changes"].as_array_mut().unwrap().push(json!(["Target", other]));
        }),
        // Review round 20: a target event without its Target, AllBonuses made null.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + 1
                + recs[start + 1..]
                    .iter()
                    .position(|r| {
                        r["k"] == "rule"
                            && r["class"] == "Kingmaker.RuleSystem.Rules.RuleAttackRoll"
                    })
                    .unwrap();
            recs[n]["fields"].as_object_mut().unwrap().remove("Target");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "h_end").unwrap();
            recs[n]["changes"].as_array_mut().unwrap().push(json!(["AllBonuses", null]));
        }),
        // Review round 21: a nested rule's push reason, the bonus container, a finished read.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + 1
                + recs[start + 1..]
                    .iter()
                    .position(|r| r["k"] == "rule" && r["reason_at_push"].is_object())
                    .unwrap();
            let caster = recs[n]["reason_at_push"]["caster"].clone();
            let other = [recs[start]["initiator"].clone(), recs[start]["target"].clone()]
                .into_iter()
                .find(|u| *u != caster)
                .unwrap();
            recs[n]["reason_at_push"]["caster"] = other;
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "h_end").unwrap();
            recs[n]["changes"]
                .as_array_mut()
                .unwrap()
                .push(json!(["m_ModifiableBonus", {"bogus": true}]));
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + recs[start..].iter().position(|r| r["k"] == "h_end").unwrap();
            recs[n]["changes"].as_array_mut().unwrap().push(json!(["AllBonuses", {}]));
        }),
        ("driven replay: commands: command", |recs| {
            // A kept handle read as finished whose check has it unfinished.
            let at = (0..recs.len())
                .find(|&n| {
                    ev(&recs[n], "uc_v") && recs[n]["f"] == "afin" && recs[n]["v"] == false && {
                        let fx = (n..recs.len()).find(|&m| ev(&recs[m], "uc_fx")).unwrap();
                        recs[fx]["fn"] == "OnTick"
                            && recs[fx]["s"]
                                .as_str()
                                .unwrap()
                                .split('|')
                                .nth(3)
                                .unwrap()
                                .ends_with("10")
                    }
                })
                .unwrap();
            recs[at]["v"] = json!(true);
        }),
        // Review round 22: a nested reason made null, a container without AllBonuses changed, a
        // finished handle unfinished later.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + 2 + recs[start + 2..].iter().position(|r| r["k"] == "about").unwrap();
            recs[n]["reason"] = json!(null);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            // A RuleCalculateAttackBonus (no AllBonuses) whose end bonuses and bonus mutation change
            // together: only its container disagrees.
            // The first such rule with a bonus mutation, inside a UnitAttack attack.
            let (rule, m) = (0..recs.len())
                .filter(|&n| {
                    recs[n]["k"] == "rule"
                        && recs[n]["class"] == "Kingmaker.RuleSystem.Rules.RuleCalculateAttackBonus"
                })
                .filter(|&n| {
                    recs[..n]
                        .iter()
                        .rev()
                        .find(|r| r["k"] == "rule" && r["depth"] == 0)
                        .is_some_and(|r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"))
                })
                .find_map(|n| {
                    let id = recs[n]["id"].clone();
                    let end =
                        n + recs[n..].iter().position(|r| r["k"] == "rule_end" && r["id"] == id)?;
                    let m = (n..end).find(|&m| {
                        recs[m]["k"] == "mut"
                            && recs[m]["what"] == "rule_bonus"
                            && recs[m]["rule"] == id
                    })?;
                    Some((n, m))
                })
                .unwrap();
            let id = recs[rule]["id"].clone();
            let end = rule
                + recs[rule..].iter().position(|r| r["k"] == "rule_end" && r["id"] == id).unwrap();
            recs[m]["value"] = json!(99);
            recs[end]["bonuses"][0]["value"] = json!(99);
        }),
        ("driven replay: commands: command", |recs| {
            // An OnAction row unfinished after an OnTick row of the same handle finished.
            let n = (0..recs.len())
                .find(|&n| {
                    let anim = |r: &serde_json::Value| {
                        r["s"].as_str().unwrap().split('|').nth(3).unwrap().to_owned()
                    };
                    ev(&recs[n], "uc_fx")
                        && recs[n]["fn"] == "OnTick"
                        && recs[n]["ua"].is_string()
                        && anim(&recs[n]).as_bytes()[2] == b'1'
                        && anim(&recs[n]).ends_with('0')
                        && recs[n + 1..].iter().find(|r| ev(r, "uc_fx")).is_some_and(|r| {
                            r["fn"] == "OnAction"
                                && r["c"] == recs[n]["c"]
                                && anim(r).ends_with('0')
                        })
                })
                .unwrap();
            let s = recs[n]["s"].as_str().unwrap().to_owned();
            let mut p: Vec<String> = s.split('|').map(str::to_owned).collect();
            p[3].pop();
            p[3].push('1');
            recs[n]["s"] = json!(p.join("|"));
        }),
        // Review round 23: a finished read flipped to false, base containers' kinds, an intermediate
        // bonus fact, the battle-log invariant.
        ("driven replay: commands: command", |recs| {
            let n = (0..recs.len())
                .rev()
                .find(|&n| {
                    ev(&recs[n], "uc_v") && recs[n]["f"] == "afin" && recs[n]["v"] == true && {
                        let fx = (n..recs.len()).find(|&m| ev(&recs[m], "uc_fx")).unwrap();
                        recs[fx]["fn"] == "OnTick"
                            && recs[fx]["s"]
                                .as_str()
                                .unwrap()
                                .split('|')
                                .nth(3)
                                .unwrap()
                                .ends_with("11")
                    }
                })
                .unwrap();
            recs[n]["v"] = json!(false);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + 1 + recs[start + 1..].iter().position(|r| r["k"] == "did").unwrap();
            recs[n]["changes"].as_array_mut().unwrap().push(json!(["m_TemporaryModifiers", 7]));
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| {
                        r["changes"].as_array().is_some_and(|c| {
                            c.iter().any(|p| p[0] == "m_ModifiableBonus" && p[1].is_object())
                        })
                    })
                    .unwrap();
            for p in recs[n]["changes"].as_array_mut().unwrap() {
                if p[0] == "m_ModifiableBonus" {
                    p[1]["m_Modifiers"][0]["Fact"] =
                        json!({"$fact": "no-such-fact", "blueprint": "0"});
                    p[1]["Modifiers"][0]["Fact"] =
                        json!({"$fact": "no-such-fact", "blueprint": "0"});
                }
                if p[0] == "AllBonuses" {
                    p[1][0]["Fact"] = json!({"$fact": "no-such-fact", "blueprint": "0"});
                }
            }
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + 1
                + recs[start + 1..]
                    .iter()
                    .position(|r| r["k"] == "rule" && r["fields"]["DisableBattleLog"] == false)
                    .unwrap();
            recs[n]["fields"]["DisableBattleLogSelf"] = json!(true);
        }),
        // Review round 24: temporary modifiers, custom data, an assigned reason, a context parent.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + 1 + recs[start + 1..].iter().position(|r| r["k"] == "did").unwrap();
            recs[n]["changes"]
                .as_array_mut()
                .unwrap()
                .push(json!(["m_TemporaryModifiers", [true]]));
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + 1 + recs[start + 1..].iter().position(|r| r["k"] == "did").unwrap();
            recs[n]["changes"]
                .as_array_mut()
                .unwrap()
                .push(json!(["m_CustomData", {"bogus": true}]));
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + 1
                + recs[start + 1..]
                    .iter()
                    .position(|r| r["k"] == "rule" && r["reason_at_push"].is_null())
                    .unwrap();
            let id = recs[n]["id"].clone();
            let about =
                n + recs[n..].iter().position(|r| r["k"] == "about" && r["id"] == id).unwrap();
            recs[about]["reason"]["rule"] = id;
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + 1
                + recs[start + 1..]
                    .iter()
                    .position(|r| r["k"] == "rule" && r["depth"] == 2)
                    .unwrap();
            recs[n]["context_parent"] = recs[start]["id"].clone();
        }),
        // Review round 25: a limb's recorded item, engagement ids twice.
        ("driven replay: commands: limb 0's recorded item", |recs| {
            let at = pos(recs, |r| {
                ev(r, "ua_atk")
                    && r["list"]
                        .as_array()
                        .is_some_and(|l| l.iter().any(|e| e.as_str().unwrap().starts_with("l0,")))
            });
            for e in recs[at]["list"].as_array_mut().unwrap() {
                let x = e.as_str().unwrap().to_owned();
                if x.starts_with("l0,") {
                    let rest = x.splitn(3, ',').nth(2).unwrap().to_owned();
                    *e = json!(format!("l0,bogus,{rest}"));
                }
            }
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + recs[start..]
                    .iter()
                    .position(|r| {
                        r["k"] == "env"
                            && r["rule"] == "RuleAttackRoll"
                            && r["target_engaged_by"].as_array().is_some_and(|a| !a.is_empty())
                    })
                    .unwrap();
            let first = recs[n]["target_engaged_by"][0].clone();
            recs[n]["target_engaged_by"].as_array_mut().unwrap().push(first);
        }),
        // Review round 26: the limbs rule's slots whole, a limb's item another entity's id.
        ("driven replay: commands: RuleCalculateAttacksCountLimbs' slots", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleCalculateAttacksCountLimbs", "InitAttacks"));
            let id = recs[at]["id"].clone();
            let n = at
                + pos(&recs[at..], |r| {
                    r["id"] == id
                        && r["changes"]
                            .as_array()
                            .is_some_and(|c| c.iter().any(|p| p[0] == "Slots"))
                });
            for p in recs[n]["changes"].as_array_mut().unwrap() {
                if p[0] == "Slots" {
                    p[1][0]["Owner"] = json!({"$unit": "bogus"});
                }
            }
        }),
        ("driven replay: commands: RuleCalculateAttacksCountLimbs' slots", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleCalculateAttacksCountLimbs", "InitAttacks"));
            let id = recs[at]["id"].clone();
            let n = at
                + pos(&recs[at..], |r| {
                    r["id"] == id
                        && r["changes"]
                            .as_array()
                            .is_some_and(|c| c.iter().any(|p| p[0] == "Slots"))
                });
            for p in recs[n]["changes"].as_array_mut().unwrap() {
                if p[0] == "Slots" {
                    p[1][0]["m_Active"] = json!("yes");
                }
            }
        }),
        ("driven replay: commands: limb 0's recorded item", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleCalculateAttacksCountLimbs", "InitAttacks"));
            let unit = recs[at]["initiator"]["$unit"].as_str().unwrap().to_owned();
            let l = pos(recs, |r| {
                ev(r, "ua_atk")
                    && r["list"]
                        .as_array()
                        .is_some_and(|l| l.iter().any(|e| e.as_str().unwrap().starts_with("l0,")))
            });
            let entry = recs[l]["list"]
                .as_array()
                .unwrap()
                .iter()
                .find(|e| e.as_str().unwrap().starts_with("l0,"))
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned();
            let item = entry.split(',').nth(1).unwrap().to_owned();
            for r in recs.iter_mut().skip(at) {
                let x = r.to_string().replace(&item, &unit);
                *r = serde_json::from_str(&x).unwrap();
            }
        }),
        // Review round 27: a reason's ability a weapon's blueprint (a damage's reason at its push,
        // so its own and its nested rules' assigned reasons).
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let weapon = recs[start]["fields"]["Weapon"]["blueprint"].clone();
            let n = start
                + pos(&recs[start..], |r| {
                    r["k"] == "rule"
                        && r["class"].as_str().is_some_and(|c| c.ends_with(".RuleDealDamage"))
                        && r["reason_at_push"].is_object()
                });
            let id = recs[n]["id"].clone();
            let end = n + pos(&recs[n..], |r| r["k"] == "rule_end" && r["id"] == id);
            recs[n]["reason_at_push"]["ability"] = weapon.clone();
            let mut ids = vec![id];
            for r in &mut recs[n..end] {
                if r["k"] == "rule" && ids.contains(&r["parent"]) && r["reason_at_push"].is_null() {
                    ids.push(r["id"].clone());
                }
                if r["k"] == "about" && ids.contains(&r["id"]) && r["reason"].is_object() {
                    r["reason"]["ability"] = weapon.clone();
                }
            }
        }),
        // Review round 28: a stat mutation of a stat the owner hasn't, a limb attack's weapon
        // another id (by the weapon and the slot), a rule step with a field its writer doesn't write.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + pos(&recs[start..], |r| r["k"] == "mut" && r["what"] == "rule_bonus");
            let owner = recs[start]["initiator"]["$unit"].clone();
            let mut m = recs[n].clone();
            let o = m.as_object_mut().unwrap();
            for k in ["descriptor", "stat_type", "type", "value", "target_rule", "fact"] {
                o.remove(k);
            }
            o.insert("what".into(), json!("stat_value"));
            o.insert("owner".into(), owner);
            o.insert("stat".into(), json!(i32::MAX));
            for k in ["old", "new", "permanent"] {
                o.insert(k.into(), json!(0));
            }
            recs.insert(n + 1, m);
            renumber(recs);
        }),
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| {
                root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule")
                    && r["fields"]["WeaponSlot"]["ParentFact"]["IsNull"] == false
            });
            recs[at]["fields"]["Weapon"]["$item"] = json!("bogus");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let id = recs[start]["id"].clone();
            let n = start + pos(&recs[start..], |r| r["k"] == "trig" && r["id"] == id);
            recs[n]["exception"] = json!({"what": "an extra field"});
        }),
        // Review round 29: a stat value unchanged, a modifier's source component not its fact's.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            insert_mut(
                recs,
                |owner| json!({"what": "stat_value", "owner": owner, "stat": 11, "old": 4, "new": 4, "permanent": 10}),
            );
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            insert_mut(recs, |owner| {
                json!({"what": "stat_mod_add", "owner": owner, "stat": 11, "modified": 4, "mod": {
                    "descriptor": 34, "descriptor_name": "Difficulty", "value": -2, "stack_mode": 0,
                    "stacks": false,
                    "source": {"$fact": "5874", "blueprint": "6e318c139554e3341b52991ba2c5a3c7"},
                    "source_component": "$Nonexistent$bogus", "item": null}})
            });
        }),
        // Review round 30: an enemy twice (with its read and its place), three `Run` frames, a diff of
        // a field unchanged, a stat bonus with dice, a ua_atk field its writer doesn't write.
        ("driven replay: commands: command 291's recorded enemies list a unit twice", |recs| {
            let e = pos(recs, |r| ev(r, "uc_v") && r["f"] == "enemies" && r["c"] == 291);
            let twice = |v: &serde_json::Value| {
                let mut a = v.as_array().unwrap().clone();
                a.extend(v.as_array().unwrap().iter().cloned());
                json!(a)
            };
            recs[e]["v"] = twice(&recs[e]["v"]);
            let f = e + pos(&recs[e..], |r| ev(r, "uc_v") && r["f"] == "five");
            let read = recs[f].clone();
            recs.insert(f + 1, read);
            let s = e + pos(&recs[e..], |r| ev(r, "uc_v") && r["f"] == "sorted");
            recs[s]["v"] = twice(&recs[s]["v"]);
            renumber(recs);
        }),
        (
            "driven replay: commands: the sim's command 40 triggers RuleCalculateAttacksCount",
            |recs| {
                const RUN: &str = "Kingmaker.UnitLogic.Commands.UnitCommands.Run";
                let two = format!("{RUN} < {RUN}");
                let n = pos(recs, |r| {
                    root_of(r, "RuleCalculateAttacksCount", "InitAttacks")
                        && r["origin"].as_str().unwrap().contains(&two)
                });
                let o = recs[n]["origin"].as_str().unwrap().replacen(
                    &two,
                    &format!("{two} < {RUN}"),
                    1,
                );
                recs[n]["origin"] = json!(o);
            },
        ),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let id = recs[start]["id"].clone();
            let n = start + pos(&recs[start..], |r| r["k"] == "trig" && r["id"] == id);
            recs[n]["changes"].as_array_mut().unwrap().push(json!(["DisableBattleLogSelf", false]));
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + pos(&recs[start..], |r| {
                    r["k"] == "trig_end"
                        && r["changes"].as_array().unwrap().iter().any(|p| p[0] == "AllBonuses")
                });
            let id = recs[n]["id"].clone();
            let dice = |a: &mut serde_json::Value| {
                let m = a.as_array_mut().unwrap().iter_mut().find(|m| m["Stat"] != 0).unwrap();
                m["DiceFormula"] = json!("1d6");
            };
            for p in recs[n]["changes"].as_array_mut().unwrap() {
                if p[0] == "m_ModifiableBonus" {
                    dice(&mut p[1]["m_Modifiers"]);
                    dice(&mut p[1]["Modifiers"]);
                } else if p[0] == "AllBonuses" {
                    dice(&mut p[1]);
                }
            }
            let e = n + pos(&recs[n..], |r| r["k"] == "rule_end" && r["id"] == id);
            let b = recs[e]["bonuses"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|b| b["stat"] != 0)
                .unwrap();
            b["dice"] = json!("1d6");
        }),
        ("driven replay: commands: a ua_atk record with fields its writer doesn't write", |recs| {
            let n = pos(recs, |r| ev(r, "ua_atk"));
            recs[n]["extra"] = json!(1);
        }),
        // Review round 31: a bonus no mutation added (overwritten later), a handler's records
        // removed (renumbered, or a gap), a nested flat-footed check's environment.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + pos(&recs[start..], |r| {
                    r["k"] == "trig_end"
                        && r["changes"].as_array().unwrap().iter().any(|p| p[0] == "AllBonuses")
                });
            let id = recs[n]["id"].clone();
            let a = start + pos(&recs[start..], |r| r["k"] == "about" && r["id"] == id);
            let x = json!({"Value": 999, "DiceFormula": null, "Fact": null, "Type": 0, "Stat": 0, "Descriptor": 0});
            let changes = recs[a]["changes"].as_array_mut().unwrap();
            changes.push(json!(["m_ModifiableBonus", {"m_Modifiers": [x], "Modifiers": [x], "ModifiersCount": 1}]));
            changes.push(json!(["AllBonuses", [x]]));
        }),
        ("driven replay: a h record whose id breaks the allocation order", |recs| {
            let h = shadow_handler(recs);
            recs.drain(h..h + 2);
            renumber(recs);
        }),
        (
            "driven replay: a record without its sequence number, or out of order (or one missing)",
            |recs| {
                let h = shadow_handler(recs);
                recs.drain(h..h + 2);
            },
        ),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let f = start
                + pos(&recs[start..], |r| {
                    r["k"] == "rule"
                        && r["class"].as_str().unwrap().ends_with(".RuleCheckTargetFlatFooted")
                });
            let id = recs[f]["id"].clone();
            let t = f + pos(&recs[f..], |r| r["k"] == "trig" && r["id"] == id);
            let env = json!({"seq": 0, "k": "env", "t": recs[t]["t"], "rule": "RuleCheckTargetFlatFooted",
                "initiator": recs[f]["initiator"], "target": recs[f]["target"],
                "target_remembers_initiator": true, "target_can_act_in_combat": false,
                "concealment_initiator_from_target": 0});
            recs.insert(t + 1, env);
            renumber(recs);
        }),
        // Review round 32: a bonus of an untracked rule, attack leaves with a field their taps
        // don't write, an animation style outside its enum.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start + pos(&recs[start..], |r| r["k"] == "mut" && r["what"] == "rule_bonus");
            recs[n]["target_rule"] = json!(0);
        }),
        ("driven replay: commands: a leaf enemies with fields its writer doesn't write", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "enemies");
            recs[at]["ctx"] = json!("other");
        }),
        ("driven replay: commands: a leaf style with fields its writer doesn't write", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "style");
            recs[at]["ctx"] = json!("other");
        }),
        ("driven replay: commands: the sim reads style", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "style");
            recs[at]["v"] = json!(i32::MAX);
        }),
        // Review round 33: an initiator handler another unit's (its fact of the same blueprint): it
        // isn't subscribed to the rule's initiator.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let h = start
                + pos(&recs[start..], |r| {
                    r["k"] == "h"
                        && r["class"].as_str().unwrap().ends_with(".TwoWeaponFightingAttackPenalty")
                });
            let (rule, component) = (recs[h]["rule"].clone(), recs[h]["component"].clone());
            // The other unit's fact of that blueprint, as another handler shows it.
            let bp = recs[h]["blueprint"].clone();
            let owner = recs[h]["owner"].clone();
            let other = pos(recs, |r| {
                r["k"] == "h"
                    && r["blueprint"] == bp
                    && r["owner"].is_object()
                    && r["owner"] != owner
            });
            let (fact, unit) = (recs[other]["fact"].clone(), recs[other]["owner"].clone());
            for r in recs.iter_mut() {
                if r["k"] == "h" && r["rule"] == rule && r["component"] == component {
                    r["fact"] = fact.clone();
                    r["owner"] = unit.clone();
                }
            }
        }),
        // Review round 34: a keyed draw whose key names a simulation caller (or none) as
        // presentation, an `ActivatableAbility` handler the initiator hasn't.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            insert_keyed_draw(
                recs,
                "global|Assembly-CSharp|Kingmaker.RuleSystem.Rules.RuleRollD20|OnTrigger(RulebookEventContext)|Range_ii",
            );
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            insert_keyed_draw(recs, "global||Range_ii");
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let h = shadow_handler(recs);
            recs[h]["class"] = json!("Kingmaker.UnitLogic.ActivatableAbilities.ActivatableAbility");
        }),
        // Review round 35: one component's handler twice in a phase (another's left out), a
        // snapshot value the oracle failed to read.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let stat = |r: &serde_json::Value| {
                r["k"] == "h"
                    && r["phase"] == "about"
                    && r["class"].as_str().unwrap().ends_with(".AttackStatReplacement")
            };
            let a = start + pos(&recs[start..], stat);
            let rule = recs[a]["rule"].clone();
            let b = a + 1 + pos(&recs[a + 1..], |r| stat(r) && r["rule"] == rule);
            for k in ["component", "fact", "blueprint", "owner"] {
                recs[b][k] = recs[a][k].clone();
            }
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let id = recs[start]["id"].clone();
            let n = start + pos(&recs[start..], |r| r["k"] == "trig_end" && r["id"] == id);
            for p in recs[n]["changes"].as_array_mut().unwrap() {
                if p[0] == "WeaponStats" {
                    p[1] = json!({"$failed": "InvalidOperationException: failed to snapshot WeaponStats"});
                }
            }
        }),
        // Review round 36: a global singleton's handler left out (handler ids and sequence
        // renumbered), a rule's bonus with dice.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let h = shadow_handler(recs);
            let gone = recs[h]["id"].as_i64().unwrap();
            recs.drain(h..h + 2);
            let shift = |v: &mut serde_json::Value| {
                if let Some(i) = v.as_i64().filter(|i| *i > gone) {
                    *v = json!(i - 1);
                }
            };
            for r in recs.iter_mut() {
                match r["k"].as_str() {
                    Some("h" | "h_end") => shift(&mut r["id"]),
                    Some("draw" | "mut") => shift(&mut r["h"]),
                    Some("rule") => shift(&mut r["handler"]),
                    _ => {}
                }
            }
            renumber(recs);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            // (A bonus of no type or stat, whose constructors allow dice: a fact's.)
            let start = pos(recs, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
            let n = start
                + pos(&recs[start..], |r| {
                    r["k"] == "trig_end"
                        && r["changes"].as_array().unwrap().iter().any(|p| {
                            p[0] == "AllBonuses"
                                && p[1]
                                    .as_array()
                                    .unwrap()
                                    .first()
                                    .is_some_and(|m| m["Type"] == 0 && m["Stat"] == 0)
                        })
                });
            let id = recs[n]["id"].clone();
            for p in recs[n]["changes"].as_array_mut().unwrap() {
                let lists: Vec<&mut serde_json::Value> = if p[0] == "m_ModifiableBonus" {
                    let o = p[1].as_object_mut().unwrap();
                    o.iter_mut().filter(|(k, _)| *k != "ModifiersCount").map(|(_, v)| v).collect()
                } else if p[0] == "AllBonuses" {
                    vec![&mut p[1]]
                } else {
                    Vec::new()
                };
                for l in lists {
                    l[0]["DiceFormula"] = json!("not dice");
                }
            }
            let e = n + pos(&recs[n..], |r| r["k"] == "rule_end" && r["id"] == id);
            recs[e]["bonuses"][0]["dice"] = json!("not dice");
        }),
        // Review round 37: an achievement's handlers left out of one rule only (the others keep
        // them), a stat mutation inside an attack (not reconciled), a snap map of another type.
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let dvd = "Kingmaker.Achievements.Logic.AchievementLogicTacticalCombatDemonVsDemon";
            let last = recs.iter().rposition(|r| r["k"] == "h" && r["class"] == dvd).unwrap();
            let rule = recs[last]["rule"].clone();
            let mut gone = Vec::new();
            let mut i = 0;
            while i < recs.len() {
                if recs[i]["k"] == "h" && recs[i]["class"] == dvd && recs[i]["rule"] == rule {
                    gone.push(recs[i]["id"].as_i64().unwrap());
                    recs.drain(i..i + 2);
                } else {
                    i += 1;
                }
            }
            let shift = |v: &mut serde_json::Value| {
                if let Some(x) = v.as_i64().filter(|x| *x > 0) {
                    *v = json!(x - gone.iter().filter(|g| **g < x).count() as i64);
                }
            };
            for r in recs.iter_mut() {
                match r["k"].as_str() {
                    Some("h" | "h_end") => shift(&mut r["id"]),
                    Some("draw" | "mut") => shift(&mut r["h"]),
                    Some("rule") => shift(&mut r["handler"]),
                    _ => {}
                }
            }
            renumber(recs);
        }),
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            insert_mut(
                recs,
                |owner| json!({"what": "stat_value", "owner": owner, "stat": 11, "old": 13, "new": 4, "permanent": 10}),
            );
        }),
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, |r| {
                root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule")
                    && r["fields"]["WeaponSlot"]["m_FxSnapMap"].is_object()
            });
            recs[at]["fields"]["WeaponSlot"]["m_FxSnapMap"]["$unity"] = json!("NotAUnityType");
        }),
        // Review round 38: a bonus the rule's computation never adds, recorded coherently (its
        // mutation, container and end): the sim's bonuses differ in T0 ("T0": the replay isn't
        // equal).
        ("T0", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleCalculateAttacksCount", "InitAttacks"));
            let id = recs[at]["id"].clone();
            let t = at + pos(&recs[at..], |r| r["k"] == "trig" && r["id"] == id);
            let m = json!({"seq": 0, "k": "mut", "t": recs[t]["t"], "what": "rule_bonus", "rule": id,
                "h": 0, "target_rule": id, "value": 777, "descriptor": 0, "type": 0, "stat_type": 0,
                "fact": null});
            recs.insert(t + 1, m);
            let te = t + pos(&recs[t..], |r| r["k"] == "trig_end" && r["id"] == id);
            let x = json!({"Value": 777, "DiceFormula": null, "Fact": null, "Type": 0, "Stat": 0, "Descriptor": 0});
            let changes = recs[te]["changes"].as_array_mut().unwrap();
            changes.push(json!(["m_ModifiableBonus", {"m_Modifiers": [x], "Modifiers": [x], "ModifiersCount": 1}]));
            changes.push(json!(["AllBonuses", [x]]));
            let e = te + pos(&recs[te..], |r| r["k"] == "rule_end" && r["id"] == id);
            recs[e]["bonuses"] = json!([{"value": 777, "descriptor": 0, "type": 0, "stat": 0, "dice": null, "fact": null}]);
            renumber(recs);
        }),
        // Review round 39: a bonus's source another fact of the sim's (in its mutation, container
        // and end alike); (a rejected addition: `a_bonus_addition_the_rule_never_asked_for_is_unequal`).
        ("T0", |recs| {
            let n = pos(recs, |r| {
                r["k"] == "mut" && r["what"] == "rule_bonus" && r["fact"].is_object()
            });
            let (rule, fact) = (recs[n]["target_rule"].clone(), recs[n]["fact"].clone());
            let other = recs
                .iter()
                .find_map(|r| {
                    let f = &r["fact"];
                    (r["k"] == "h" && f.is_object() && *f != fact).then(|| f.clone())
                })
                .unwrap();
            let swap = |v: &mut serde_json::Value| {
                let x = v.to_string().replace(&fact.to_string(), &other.to_string());
                *v = serde_json::from_str(&x).unwrap();
            };
            for r in recs.iter_mut() {
                let of_rule = (r["k"] == "mut" && r["target_rule"] == rule)
                    || (["about", "trig", "trig_end", "did", "rule_end"]
                        .contains(&r["k"].as_str().unwrap_or(""))
                        && r["id"] == rule)
                    || (r["k"] == "h_end" && r["rule"] == rule);
                if of_rule {
                    swap(r);
                }
            }
        }),
        // Review round 40: a kept handle, seen acted, unacted after `OnAction`; a retarget's wrapper
        // id out of the allocator's sequence; the flanking bonus added after its nested rule.
        ("driven replay: commands: command 246's animation, seen acted", |recs| {
            let anim = |r: &serde_json::Value| {
                r["s"].as_str().unwrap().split('|').nth(3).unwrap().to_owned()
            };
            let at = (0..recs.len())
                .find_map(|i| {
                    let r = &recs[i];
                    if !(ev(r, "uc_fx") && r["fn"] == "OnTick" && anim(r).as_bytes()[3] == b'1') {
                        return None;
                    }
                    let j = i
                        + 1
                        + recs[i + 1..].iter().position(|x| ev(x, "uc_fx") && x["c"] == r["c"])?;
                    (recs[j]["fn"] == "OnAction" && anim(&recs[j]).as_bytes()[3] == b'1')
                        .then_some(j)
                })
                .unwrap();
            let mut p: Vec<String> =
                recs[at]["s"].as_str().unwrap().split('|').map(str::to_owned).collect();
            p[3].replace_range(3..4, "0");
            recs[at]["s"] = json!(p.join("|"));
        }),
        ("driven replay: commands: a target wrapper id out of", |recs| {
            let last = recs
                .iter()
                .filter(|r| ev(r, "uc_fx"))
                .filter_map(|r| {
                    r["s"].as_str()?.split('|').nth(10)?.rsplit_once('#')?.1.parse::<u32>().ok()
                })
                .max()
                .unwrap();
            for r in recs.iter_mut() {
                let x = r
                    .to_string()
                    .replace(&format!("#{last}\""), "#4294967295\"")
                    .replace(&format!("#{last}|"), "#4294967295|");
                *r = serde_json::from_str(&x).unwrap();
            }
        }),
        ("T0", |recs| {
            let n = pos(recs, |r| r["k"] == "mut" && r["what"] == "rule_bonus" && r["type"] == 2);
            let m = recs.remove(n);
            let rule = m["target_rule"].clone();
            let nested = n + pos(&recs[n..], |r| r["k"] == "rule" && r["parent"] == rule);
            let id = recs[nested]["id"].clone();
            let e = nested + pos(&recs[nested..], |r| r["k"] == "rule_end" && r["id"] == id);
            recs.insert(e + 1, m);
            renumber(recs);
        }),
        // Review round 41: a bonus container allocated without an addition (a sequence's release
        // history: `a_released_sequence_handle_stays_released`, scripted).
        ("driven replay: commands: a UnitAttack rule's span", |recs| {
            let at = pos(recs, |r| root_of(r, "RuleCalculateAttacksCount", "InitAttacks"));
            let id = recs[at]["id"].clone();
            let te = at + pos(&recs[at..], |r| r["k"] == "trig_end" && r["id"] == id);
            recs[te]["changes"].as_array_mut().unwrap().push(json!(["m_ModifiableBonus",
                {"m_Modifiers": [], "Modifiers": [], "ModifiersCount": 0}]));
        }),
        // A root of UnitAttack's code outside its calls.
        (
            "driven replay: a root RuleCalculateAttacksCount of UnitAttack's code the sim's commands didn't trigger",
            |recs| {
                let start = pos(recs, |r| root_of(r, "RuleCalculateAttacksCount", "InitAttacks"));
                let id = recs[start]["id"].clone();
                let end = start
                    + recs[start..]
                        .iter()
                        .position(|r| r["k"] == "rule_end" && r["id"] == id)
                        .unwrap();
                let span: Vec<_> = recs[start..=end].to_vec();
                let ctl = start + recs[start..].iter().position(|r| ev(r, "uc_ctl_end")).unwrap();
                let t = recs[ctl]["t"].clone();
                for (k, mut r) in span.into_iter().enumerate() {
                    r["t"] = t.clone();
                    recs.insert(ctl + 1 + k, r);
                }
            },
        ),
    ];
    for (n, (want, change)) in cases.into_iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
        change(&mut changed.records);
        let out = replay::run_with_tape(&mut w, &changed, &trace);
        if want == "T0" {
            assert!(!out.diff.equal, "case {n}: the replay equal to the damaged recording");
            continue;
        }
        assert!(!w.valid(), "case {n}: {want}");
        assert!(
            w.coverage.unsupported.keys().any(|k| k.starts_with(want)),
            "{want}: {:?}",
            w.coverage.unsupported.keys().collect::<Vec<_>>()
        );
    }
    // A replay's simulation draw without its tape status (round 10).
    let (mut w, trace) = replay::prepare_world(SAVE, &dir, "replay-1").expect("prepare");
    let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
    let start = pos(&changed.records, |r| root_of(r, "RuleAttackWithWeapon", "TriggerAttackRule"));
    let n = start
        + changed.records[start..]
            .iter()
            .position(|r| r["k"] == "draw" && r["c"] == "sim")
            .expect("a draw");
    let o = changed.records[n].as_object_mut().expect("draw");
    o.remove("forced");
    o.remove("match");
    let _ = replay::run_with_tape(&mut w, &changed, &trace);
    assert!(!w.valid(), "a replay's draw without its tape status");
    assert!(
        w.coverage
            .unsupported
            .keys()
            .any(|k| k.starts_with("driven replay: commands: a UnitAttack rule's span")),
        "{:?}",
        w.coverage.unsupported.keys().collect::<Vec<_>>()
    );
}

/// The approach capture's records damaged (`docs/poc5/commands.md`, slice C): a lost, changed or
/// unreadable agent call, `TickApproaching` draw, tap or description, a class answer or class-code
/// state other than the sim's; each is reported.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the approach capture"]
fn missing_or_wrong_approach_records_are_reported() {
    let Some(dir) = fixture_in(APPROACH, "tb-nearest") else {
        eprintln!("{APPROACH}/tb-nearest not present: skipped");
        return;
    };
    type Change = fn(&mut Vec<serde_json::Value>);
    fn pos(recs: &[serde_json::Value], f: impl Fn(&serde_json::Value) -> bool) -> usize {
        recs.iter().position(f).expect("a record to change")
    }
    fn ev(r: &serde_json::Value, e: &str) -> bool {
        r["k"] == "obs" && r["ev"] == e
    }
    fn call(r: &serde_json::Value, f: &str) -> bool {
        ev(r, "uc_call") && r["fn"] == f
    }
    // `TickApproaching`'s draws (the trace's site table names them).
    fn draw(r: &serde_json::Value) -> bool {
        r["k"] == "draw" && r["m"] == "Range_ff" && r["a"] == json!([0.2, 0.6]) && r["rule"] == 0
    }
    // A `UnitMoveTo`'s description.
    fn move_to(r: &serde_json::Value) -> bool {
        ev(r, "uc_new") && r["class"] == "UnitMoveTo"
    }
    let cases: [(&str, Change); 32] = [
        // The lifecycle's calls into the movement agent: lost, changed, unreadable, extra.
        ("driven replay: commands: MoveTo", |recs| {
            let at = pos(recs, |r| call(r, "moveto"));
            recs.remove(at);
        }),
        ("driven replay: commands: the sim calls MoveTo", |recs| {
            let at = pos(recs, |r| call(r, "moveto"));
            recs[at]["p"][0] = json!(1.5);
        }),
        ("driven replay: commands: the sim calls MoveTo", |recs| {
            let at = pos(recs, |r| call(r, "moveto"));
            recs[at]["r"] = json!("2");
        }),
        ("driven replay: commands: the sim calls MoveTo", |recs| {
            let at = pos(recs, |r| call(r, "moveto"));
            recs[at]["r"] = json!("NaN");
        }),
        ("driven replay: commands: the sim calls MoveTo", |recs| {
            let at = pos(recs, |r| call(r, "moveto"));
            let xz = recs[at]["xz"].as_bool().unwrap();
            recs[at]["xz"] = json!(!xz);
        }),
        ("driven replay: commands: the sim calls MoveTo", |recs| {
            let at = pos(recs, |r| call(r, "moveto"));
            recs[at]["tu"] = json!("5198");
        }),
        (
            "driven replay: commands: a uc_call record with fields its writer doesn't write",
            |recs| {
                let at = pos(recs, |r| call(r, "moveto"));
                recs[at]["extra"] = json!(1);
            },
        ),
        ("driven replay: commands: the sim calls Follow", |recs| {
            let at = pos(recs, |r| call(r, "follow"));
            recs[at]["v"][0][1] = json!(99.0);
        }),
        ("driven replay: commands: the sim calls Follow", |recs| {
            let at = pos(recs, |r| call(r, "follow"));
            recs[at]["forced"] = json!(true);
        }),
        ("driven replay: commands: the sim calls Stop", |recs| {
            let at = pos(recs, |r| call(r, "stop") && r["c"].is_u64());
            recs[at]["c"] = json!(null);
        }),
        ("driven replay: commands: Stop", |recs| {
            let at = pos(recs, |r| call(r, "stop") && r["c"].is_null());
            recs.remove(at);
        }),
        ("driven replay: commands: ", |recs| {
            let at = pos(recs, |r| call(r, "stop"));
            let x = recs[at].clone();
            recs.insert(at, x);
        }),
        // `TickApproaching`'s draws: lost, changed, extra, or of another range or rule.
        ("driven replay: commands: TickApproaching's draw for command", |recs| {
            let at = pos(recs, draw);
            recs.remove(at);
        }),
        ("driven replay: commands: TickApproaching's draw 0.", |recs| {
            let at = pos(recs, draw);
            recs[at]["v"] = json!(0.25);
        }),
        ("driven replay: commands: TickApproaching's draw for command", |recs| {
            let at = pos(recs, draw);
            recs[at]["rule"] = json!(5);
        }),
        ("driven replay: commands: ", |recs| {
            let at = pos(recs, draw);
            let x = recs[at].clone();
            recs.insert(at, x);
        }),
        ("driven replay: a TickApproaching draw the sim's commands didn't make", |recs| {
            let d = recs[pos(recs, draw)].clone();
            let at = pos(recs, |r| r["k"] == "tick" && r["t"] == d["t"]);
            recs.insert(at + 1, d);
        }),
        // Review round 1: an extra one hidden by a contradictory key (sequence renumbered).
        ("driven replay: a TickApproaching draw the sim's commands didn't make", |recs| {
            let mut d = recs[pos(recs, draw)].clone();
            d["key"] = json!("wrong-key");
            let at = pos(recs, |r| r["k"] == "tick" && r["t"] == d["t"]);
            recs.insert(at + 1, d);
            for (i, r) in recs.iter_mut().enumerate() {
                r["seq"] = json!(i);
            }
        }),
        // The taps: a mount, tactical combat, a lost read, another path.
        ("driven replay: commands: TickApproaching: a mount (SaddledPart)", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "spart");
            recs[at]["v"] = json!(true);
        }),
        ("driven replay: commands: TickApproaching: tactical combat", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "tact");
            recs[at]["v"] = json!(true);
        }),
        ("driven replay: commands: ", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "rpart");
            recs.remove(at);
        }),
        ("driven replay: commands: the sim calls Follow", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "fpath" && r["v"].is_array());
            recs[at]["v"][0][0] = json!(-3.0);
        }),
        ("driven replay: commands: a fpath record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "fpath" && r["v"].is_array());
            recs[at]["v"][0] = json!([1.0, 2.0]);
        }),
        // The descriptions: missing, of another class, another radius or await override.
        ("driven replay: a uc_new record whose approach fields", |recs| {
            let at = pos(recs, |r| ev(r, "uc_new"));
            recs[at].as_object_mut().unwrap().remove("ap");
        }),
        ("driven replay: a uc_new record whose approach fields", |recs| {
            let at = pos(recs, move_to);
            recs[at].as_object_mut().unwrap().remove("mv");
        }),
        ("driven replay: commands: the sim calls MoveTo", |recs| {
            let at = pos(recs, move_to);
            let ap = recs[at]["ap"].as_str().unwrap().to_owned();
            let rest = ap.split_once('|').unwrap().1.to_owned();
            recs[at]["ap"] = json!(format!("0.75|{rest}"));
        }),
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, move_to);
            let mv = recs[at]["mv"].as_str().unwrap().to_owned();
            let p: Vec<&str> = mv.split('|').collect();
            recs[at]["mv"] = json!(format!("{}|{}|0|{}", p[0], p[1], p[3]));
        }),
        // `UnitMoveTo`'s class code and answers the sim's.
        ("driven replay: commands: command", |recs| {
            let moves: Vec<serde_json::Value> =
                recs.iter().filter(|r| move_to(r)).map(|r| r["c"].clone()).collect();
            let at = pos(recs, |r| ev(r, "uc_v") && r["f"] == "await" && moves.contains(&r["c"]));
            recs[at]["v"] = json!(false);
        }),
        ("driven replay: commands: command", |recs| {
            let moves: Vec<serde_json::Value> =
                recs.iter().filter(|r| move_to(r)).map(|r| r["c"].clone()).collect();
            let at = pos(recs, |r| ev(r, "uc_fx") && r["fn"] == "OnRun" && moves.contains(&r["c"]));
            let s = recs[at]["s"].as_str().unwrap().to_owned();
            // `DidRun` (the eighth field's third flag) cleared.
            let mut p: Vec<String> = s.split('|').map(str::to_owned).collect();
            p[7] = format!("{}0", &p[7][..2]);
            recs[at]["s"] = json!(p.join("|"));
        }),
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, |r| {
                ev(r, "uc_fx") && r["fn"] == "OnAction" && r["ret"] == 3 && r["ua"].is_null()
            });
            recs[at]["ret"] = json!(1);
        }),
        // The observer without the commands or the attacks observer.
        (
            "driven replay: the recording declares its approach observer without the command and attack records",
            |recs| {
                let at = pos(recs, |r| r["k"] == "begin");
                recs[at]["observers"] =
                    json!(["groups", "cooldowns", "turn_ticks", "attacks", "approach"]);
            },
        ),
        (
            "driven replay: the recording declares its approach observer without the command and attack records",
            |recs| {
                let at = pos(recs, |r| r["k"] == "begin");
                recs[at]["observers"] =
                    json!(["groups", "cooldowns", "turn_ticks", "commands", "approach"]);
            },
        ),
    ];
    for (i, (want, change)) in cases.into_iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
        change(&mut changed.records);
        let _ = replay::run_with_tape(&mut w, &changed, &trace);
        assert!(!w.valid(), "case {i}: {want}");
        assert!(
            w.coverage.unsupported.keys().any(|k| k.starts_with(want)),
            "case {i}: {want}: {:?}",
            w.coverage.unsupported.keys().collect::<Vec<_>>()
        );
    }
}

/// `poc5-tb-81/tb-focus`'s attacks of opportunity damaged (slice C): a threat hand, an animation
/// gate, a description or an attack rule other than the sim's; each is reported.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-81 capture"]
fn missing_or_wrong_opportunity_records_are_reported() {
    let Some(dir) = fixture_in("poc5-tb-81", "tb-focus") else {
        eprintln!("poc5-tb-81/tb-focus not present: skipped");
        return;
    };
    type Change = fn(&mut Vec<serde_json::Value>);
    fn pos(recs: &[serde_json::Value], f: impl Fn(&serde_json::Value) -> bool) -> usize {
        recs.iter().position(f).expect("a record to change")
    }
    fn leaf(r: &serde_json::Value, f: &str) -> bool {
        r["k"] == "obs" && r["ev"] == "uc_v" && r["f"] == f
    }
    fn aoo_new(r: &serde_json::Value) -> bool {
        r["k"] == "obs" && r["ev"] == "uc_new" && r["class"] == "UnitAttackOfOpportunity"
    }
    fn aoo_root(r: &serde_json::Value) -> bool {
        r["k"] == "rule"
            && r["parent"] == 0
            && r["origin"].as_str().is_some_and(|o| {
                o.starts_with("Kingmaker.UnitLogic.Commands.UnitAttackOfOpportunity.")
            })
    }
    let cases: [(&str, Change); 11] = [
        // The threat hand: another slot, item or flag, none, unreadable.
        ("driven replay: commands: the sim's threat hand", |recs| {
            let at = pos(recs, |r| leaf(r, "threat"));
            let v = recs[at]["v"].as_str().unwrap().replacen("p,", "s,", 1);
            recs[at]["v"] = json!(v);
        }),
        ("driven replay: commands: the sim's threat hand", |recs| {
            let at = pos(recs, |r| leaf(r, "threat"));
            let v = recs[at]["v"].as_str().unwrap().to_owned();
            let p: Vec<&str> = v.split(',').collect();
            recs[at]["v"] = json!(format!("{},{},{},1", p[0], p[1], p[2]));
        }),
        ("driven replay: commands: the sim's threat hand", |recs| {
            let at = pos(recs, |r| leaf(r, "threat"));
            recs[at]["v"] = json!(null);
        }),
        ("driven replay: commands: a threat record it can't read", |recs| {
            let at = pos(recs, |r| leaf(r, "threat"));
            recs[at]["v"] = json!("p,x");
        }),
        // `OnRun`'s gate: the manager's read lost, `CanRunIdleAction` other than recorded.
        ("driven replay: commands: ", |recs| {
            let at = pos(recs, |r| leaf(r, "anim"));
            recs.remove(at);
        }),
        ("driven replay: commands: ", |recs| {
            let at = pos(recs, |r| leaf(r, "idle") && r["v"] == true);
            recs[at]["v"] = json!(false);
        }),
        // The description: its turn flag (the `stt` answer), its fields lost.
        ("driven replay: commands: command", |recs| {
            let at = pos(recs, aoo_new);
            let v = recs[at]["aoo"].as_str().unwrap();
            let flipped = format!("{}{}", &v[..1], if &v[1..] == "1" { "0" } else { "1" });
            recs[at]["aoo"] = json!(flipped);
        }),
        ("driven replay: a uc_new record whose approach fields", |recs| {
            let at = pos(recs, aoo_new);
            recs[at].as_object_mut().unwrap().remove("aoo");
        }),
        // Its rule: not an attack of opportunity, flat-footed forced, with a slot.
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, aoo_root);
            recs[at]["fields"]["IsAttackOfOpportunity"] = json!(false);
        }),
        ("driven replay: commands: the sim's attack", |recs| {
            let at = pos(recs, aoo_root);
            recs[at]["fields"]["ForceFlatFooted"] = json!(true);
        }),
        ("driven replay: commands: the sim's command", |recs| {
            let at = pos(recs, aoo_root);
            let o = recs[at]["origin"].as_str().unwrap().replacen(".OnAction", ".OnRun", 1);
            recs[at]["origin"] = json!(o);
        }),
    ];
    for (i, (want, change)) in cases.into_iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
        change(&mut changed.records);
        let _ = replay::run_with_tape(&mut w, &changed, &trace);
        assert!(!w.valid(), "case {i}: {want}");
        assert!(
            w.coverage.unsupported.keys().any(|k| k.starts_with(want)),
            "case {i}: {want}: {:?}",
            w.coverage.unsupported.keys().collect::<Vec<_>>()
        );
    }
}

/// The capture the AI records are damaged in.
const AI: &str = "poc5-tb-82";

/// Every way of losing, changing or adding an AI record (slice D) is reported.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-82 capture"]
fn missing_or_wrong_ai_records_are_reported() {
    let Some(dir) = fixture_in(AI, "tb-nearest") else {
        eprintln!("{AI}/tb-nearest not present: skipped");
        return;
    };
    type Change = fn(&mut Vec<serde_json::Value>);
    fn pos(recs: &[serde_json::Value], f: impl Fn(&serde_json::Value) -> bool) -> usize {
        recs.iter().position(f).expect("a record to change")
    }
    fn ev(r: &serde_json::Value, e: &str) -> bool {
        r["k"] == "obs" && r["ev"] == e
    }
    fn leaf(r: &serde_json::Value, f: &str) -> bool {
        ev(r, "uc_v") && r["f"] == f
    }
    // The second read of leaf `f` in a frame that reads it twice, changed to `v`.
    fn second_in_frame(recs: &mut [serde_json::Value], f: &str, v: serde_json::Value) {
        let n = recs.len();
        let j = (0..n)
            .find_map(|i| {
                if !leaf(&recs[i], f) {
                    return None;
                }
                let j = (i + 1..n).find(|&j| leaf(&recs[j], f))?;
                (recs[j]["t"] == recs[i]["t"]).then_some(j)
            })
            .expect("a frame reading the leaf twice");
        recs[j]["v"] = v;
    }
    // `SelectAction`'s draws.
    fn draw(r: &serde_json::Value) -> bool {
        r["k"] == "draw" && r["m"] == "Range_ff" && r["a"] == json!([0.0, 0.1]) && r["rule"] == 0
    }
    // The first decision that picks an action, and its records.
    fn best(r: &serde_json::Value) -> bool {
        ev(r, "ai_best") && !r["a"].is_null()
    }
    const C: &str = "driven replay: commands:";
    let cases: [(&str, Change); 71] = [
        // The entries.
        ("driven replay: a uc_ai record it can't read", |recs| {
            let at =
                pos(recs, |r| ev(r, "uc_ai") && r["fn"] == "ForceTick" && r["nested"] == false);
            recs[at]["limited"] = json!("no");
        }),
        ("driven replay: a uc_ai record it can't read", |recs| {
            let at =
                pos(recs, |r| ev(r, "uc_ai") && r["fn"] == "ForceTick" && r["nested"] == false);
            recs[at]["extra"] = json!(1);
        }),
        ("driven replay: the AI called from a source the replay can't place", |recs| {
            let at =
                pos(recs, |r| ev(r, "uc_ai") && r["fn"] == "ForceTick" && r["nested"] == false);
            recs[at]["fn"] = json!("Tick");
        }),
        (C, |recs| {
            let at = pos(recs, |r| {
                ev(r, "uc_ai_end") && r["fn"] == "ForceTick" && r["t"].as_i64() > Some(300)
            });
            recs[at]["exc"] = json!(true);
        }),
        (C, |recs| {
            let at = pos(recs, |r| {
                ev(r, "uc_ai_end") && r["fn"] == "ForceTick" && r["t"].as_i64() > Some(300)
            });
            recs.remove(at);
        }),
        ("driven replay: a command record (uc_ai) of another frame", |recs| {
            let at =
                pos(recs, |r| ev(r, "uc_ai") && r["fn"] == "ForceTick" && r["nested"] == false);
            let t = recs[at]["t"].as_i64().unwrap();
            recs[at]["t"] = json!(t + 1);
        }),
        // The groups, gates and clocks.
        (C, |recs| {
            let at = pos(recs, |r| {
                leaf(r, "groups") && r["v"].as_array().is_some_and(|a| !a.is_empty())
            });
            recs[at]["v"][0][2] = json!(!recs[at]["v"][0][2].as_bool().unwrap());
        }),
        (C, |recs| {
            let at = pos(recs, |r| {
                leaf(r, "groups") && r["v"].as_array().is_some_and(|a| !a.is_empty())
            });
            recs[at]["v"][0][1] = json!(["51DD"]);
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "sleep"));
            recs[at]["v"] = json!(true);
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "nct"));
            recs[at]["v"] = json!("100000");
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "time"));
            recs[at]["v"] = json!("NaN");
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "passive"));
            recs.remove(at);
        }),
        // The scoring's inputs.
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "aien"));
            recs[at]["v"] = json!(false);
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "diff"));
            recs[at]["v"] = json!(-1);
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "range"));
            recs[at]["v"] = json!("30");
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "range"));
            recs[at]["a"] = json!("00000000000000000000000000000001");
        }),
        (C, |recs| {
            // A target in reach moved out of it: its `LastTarget` isn't read.
            let at = pos(recs, |r| leaf(r, "corp"));
            let at = at
                + recs[at..]
                    .windows(3)
                    .position(|x| leaf(&x[0], "corp") && leaf(&x[1], "corp") && leaf(&x[2], "last"))
                    .unwrap();
            recs[at]["v"] = json!("-20");
        }),
        (C, |recs| {
            let at = pos(recs, |r| {
                leaf(r, "enemies") && r["v"].as_array().is_some_and(|a| !a.is_empty())
            });
            recs[at]["v"][0] = json!("no-such-unit");
        }),
        (C, |recs| {
            // A target in reach (its `LastTarget` read) moved out of it.
            let at = pos(recs, |r| {
                leaf(r, "dist")
                    && r["v"].as_str().and_then(|v| v.parse::<f32>().ok()).is_some_and(|d| d < 2.5)
            });
            recs[at]["v"] = json!("50");
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "dist"));
            recs[at]["to"] = json!("5198");
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "disliked"));
            recs[at]["v"] = json!(true);
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "ptf"));
            recs[at]["v"] = json!("866ffa6c34000cd4a86fb1671f86c7d8");
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "unreach"));
            recs[at]["v"] = json!(true);
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "pclose"));
            recs[at]["v"] = json!(!recs[at]["v"].as_bool().unwrap());
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "path"));
            recs[at]["v"][0][0] = json!(1.5);
        }),
        // What's reported as not modelled.
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "conf"));
            recs[at]["v"] = json!(true);
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "magus"));
            recs[at]["v"] = json!(true);
        }),
        // The checks.
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "ai_sc") && r["fn"] == "ScoreTarget");
            recs[at]["s"] = json!("0.80");
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "ai_sc") && r["fn"] == "ScoreActor");
            recs[at]["b"] = json!("1");
        }),
        (C, |recs| {
            let at = pos(recs, best);
            recs[at]["tgt"] = json!("558A");
        }),
        (C, |recs| {
            let at = pos(recs, best);
            recs[at]["path"] = json!(null);
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "ai_sel") && r["ret"] == true);
            recs[at]["nct"] = json!("1");
        }),
        // The draws.
        (C, |recs| {
            let at = pos(recs, draw);
            recs.remove(at);
        }),
        (C, |recs| {
            let at = pos(recs, draw);
            let x = recs[at].clone();
            recs.insert(at, x);
        }),
        // A draw outside the AI.
        ("driven replay: an AI draw the sim's AI didn't make", |recs| {
            let at = pos(recs, draw);
            let x = recs.remove(at);
            let end = pos(recs, |r| r["k"] == "tick" && r["t"].as_i64() > x["t"].as_i64());
            recs.insert(end - 1, x);
        }),
        // The command the AI runs.
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "uc_new") && r["ai"] == true);
            recs[at]["tgt"] = json!("558A");
        }),
        // Review round 1: a difficulty comparison outside -1..1, the AI's nested calls' origins and
        // fields, NextCommandTime tracked through its writes, an AI controller tick lost.
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "diff"));
            recs[at]["v"] = json!(2);
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "uc_ai") && r["nested"] == true);
            recs[at]["origin"] = json!("not-a-game-method");
        }),
        (C, |recs| {
            let at = pos(recs, |r| {
                ev(r, "uc_run")
                    && r["nested"] == true
                    && r["origin"].as_str().is_some_and(|o| o.contains("SelectAction"))
            });
            recs[at]["extra"] = json!(1);
        }),
        (C, |recs| {
            let at = pos(recs, |r| {
                ev(r, "uc_run")
                    && r["nested"] == true
                    && r["origin"].as_str().is_some_and(|o| o.contains("SelectAction"))
            });
            recs[at]["origin"] = json!("Kingmaker.UnitLogic.Commands.UnitCommands.Run");
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "ai_nct") && r["site"] == "SelectAction");
            recs[at]["v"] = json!("1");
        }),
        (C, |recs| {
            // A read after the AI's own write: the sim knows the value.
            let w = pos(recs, |r| ev(r, "ai_nct") && r["site"] == "SelectAction");
            let u = recs[w]["u"].clone();
            let at = w + recs[w..]
                .iter()
                .position(|r| leaf(r, "nct") && r["u"] == u)
                .expect("a later read");
            recs[at]["v"] = json!("100000");
        }),
        ("driven replay: an ai_nct record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "ai_nct") && r["site"] != "SelectAction");
            recs[at]["site"] = json!("Elsewhere");
        }),
        ("driven replay: an ai_nct record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "ai_nct") && r["site"] == "Engage");
            recs[at]["u"] = json!("no-such-unit");
        }),
        ("driven replay: a frame without the AI controller's tick (uc_ai)", |recs| {
            let at =
                pos(recs, |r| ev(r, "uc_ai") && r["fn"] == "Tick" && r["t"].as_i64() > Some(600));
            assert!(ev(&recs[at + 1], "uc_ai_end"));
            recs.drain(at..at + 2);
            for (n, r) in recs.iter_mut().enumerate() {
                r["seq"] = json!(n);
            }
        }),
        (
            "driven replay: the turn controller's forced AI tick differs from the AI's entries",
            |recs| {
                // A forced AI tick that decided nothing: its whole entry lost.
                let at = pos(recs, |r| {
                    ev(r, "uc_ai")
                        && r["fn"] == "ForceTick"
                        && r["origin"].as_str().is_some_and(|o| {
                            o.starts_with("Kingmaker.AI.AiBrainController.ForceTick < TurnBased.Controllers.TurnController.Tick")
                        })
                });
                let end = at + recs[at..].iter().position(|r| ev(r, "uc_ai_end")).unwrap();
                recs.drain(at..=end);
                for (n, r) in recs.iter_mut().enumerate() {
                    r["seq"] = json!(n);
                }
            },
        ),
        // Review round 2: a write without the observer, a top-level origin, a group twice, a negative
        // distance, a path's error against its state.
        ("driven replay: an ai_nct record without the ai observer", |recs| {
            let at = pos(recs, |r| r["k"] == "begin");
            recs[at]["observers"] =
                json!(["groups", "cooldowns", "turn_ticks", "commands", "attacks", "approach"]);
        }),
        ("driven replay: the AI called from a source the replay can't place", |recs| {
            let at = pos(recs, |r| ev(r, "uc_ai") && r["fn"] == "Tick");
            let o = recs[at]["origin"].as_str().unwrap().to_owned();
            recs[at]["origin"] = json!(format!("Bogus.Foo < {o}"));
        }),
        (C, |recs| {
            let at =
                pos(recs, |r| leaf(r, "groups") && r["v"].as_array().is_some_and(|a| a.len() > 1));
            let g = recs[at]["v"][1].clone();
            recs[at]["v"].as_array_mut().unwrap().push(g);
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "dist"));
            recs[at]["v"] = json!("-1");
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "path"));
            recs[at]["err"] = json!(true);
        }),
        // Review round 3: an external write other than 0, a saddled unit the sim lacks, the groups'
        // order, a fake path of three points.
        ("driven replay: an ai_nct record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "ai_nct") && r["site"] == "UpdateCombatTarget");
            recs[at]["v"] = json!("123");
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "ai_best"));
            let at = at - recs[..at].iter().rev().position(|r| leaf(r, "saddled")).unwrap() - 1;
            recs[at]["v"] = json!("no-such-unit");
        }),
        (C, |recs| {
            let at =
                pos(recs, |r| leaf(r, "groups") && r["v"].as_array().is_some_and(|a| a.len() > 2));
            recs[at]["v"].as_array_mut().unwrap().swap(1, 2);
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "path"));
            recs[at]["fake"] = json!(true);
            recs[at]["v"].as_array_mut().unwrap().push(json!([1.0, 2.0, 3.0]));
        }),
        // Review round 4: a corpulence below the game's minimum.
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "corp"));
            recs[at]["v"] = json!("0.4");
        }),
        // Review round 5: a forced AI tick outside the turn controller's tick, an enemy twice, a
        // path of one point.
        ("driven replay: a forced AI tick outside the turn controller's tick", |recs| {
            let at = pos(recs, |r| {
                ev(r, "uc_ai")
                    && r["fn"] == "ForceTick"
                    && r["origin"].as_str().is_some_and(|o| {
                        o.starts_with("Kingmaker.AI.AiBrainController.ForceTick < TurnBased.Controllers.TurnController.Tick")
                    })
            });
            let end = at + recs[at..].iter().position(|r| ev(r, "uc_ai_end")).unwrap();
            let span: Vec<serde_json::Value> = recs[at..=end].to_vec();
            // Before its frame's turn controller tick starts.
            let t = recs[at]["t"].clone();
            let tt = pos(recs, |r| r["k"] == "obs" && r["ev"] == "tt" && r["t"] == t);
            for (k, x) in span.into_iter().enumerate() {
                recs.insert(tt + k, x);
            }
            for (n, r) in recs.iter_mut().enumerate() {
                r["seq"] = json!(n);
            }
        }),
        (C, |recs| {
            let at = pos(recs, |r| {
                leaf(r, "enemies") && r["v"].as_array().is_some_and(|a| !a.is_empty())
            });
            let x = recs[at]["v"][0].clone();
            recs[at]["v"].as_array_mut().unwrap().push(x);
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "path"));
            recs[at]["v"].as_array_mut().unwrap().truncate(1);
        }),
        // Review round 6: Time.time changing within a frame, an AI attack's approach not the
        // constructor's.
        (C, |recs| {
            // The gate's read of a frame that reads the clock again (`SelectAction`): a later value
            // the gate's comparison doesn't see (`NextCommandTime < Time.time` either way).
            let n = recs.len();
            let (i, j) = (0..n)
                .find_map(|i| {
                    if !leaf(&recs[i], "time") {
                        return None;
                    }
                    let j = (i + 1..n).find(|&j| leaf(&recs[j], "time"))?;
                    (recs[j]["t"] == recs[i]["t"]).then_some((i, j))
                })
                .expect("a frame reading the clock twice");
            let v: f32 = recs[j]["v"].as_str().unwrap().parse().unwrap();
            recs[i]["v"] = json!(format!("{}", v + 1000.0));
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "uc_new") && r["ai"] == true);
            recs[at]["ap"] = json!("9|0|01");
        }),
        // Review round 7: a decision's stable reads contradicting each other, the clock going back.
        (C, |recs| {
            // A unit's corpulence read again later in the decision.
            let n = recs.len();
            let at = (0..n)
                .find(|&i| {
                    leaf(&recs[i], "corp")
                        && recs[i + 1..]
                            .iter()
                            .take_while(|r| !ev(r, "ai_best"))
                            .any(|r| leaf(r, "corp") && r["u"] == recs[i]["u"])
                })
                .unwrap();
            recs[at]["v"] = json!("1");
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "manual"));
            recs[at]["v"] = json!("558A");
        }),
        (C, |recs| {
            // A later frame's clock below an earlier frame's.
            let first = pos(recs, |r| leaf(r, "time"));
            let t = recs[first]["t"].clone();
            let at =
                first + recs[first..].iter().position(|r| leaf(r, "time") && r["t"] != t).unwrap();
            recs[at]["v"] = json!("0.5");
        }),
        // Review round 8: other reads that can't change within a decision (the second of a frame
        // contradicting the first).
        (C, |recs| second_in_frame(recs, "range", json!("123"))),
        (C, |recs| second_in_frame(recs, "mrange", json!("-100"))),
        (C, |recs| second_in_frame(recs, "ptf", json!("866ffa6c34000cd4a86fb1671f86c7d8"))),
        // Review round 9: a ready group left out (with its units' reads), a negative movement range.
        (C, |recs| {
            let n = recs.len();
            // A ForceTick whose groups list an in-combat group none of whose units decides: only
            // its units' `ctrl` reads follow.
            let (at, k, units, end) = (0..n)
                .find_map(|i| {
                    if !(ev(&recs[i], "uc_ai") && recs[i]["fn"] == "ForceTick") {
                        return None;
                    }
                    let end = i + recs[i..].iter().position(|r| ev(r, "uc_ai_end"))?;
                    let g = i + 1;
                    if !leaf(&recs[g], "groups") {
                        return None;
                    }
                    let groups = recs[g]["v"].as_array()?.clone();
                    groups.iter().enumerate().find_map(|(k, x)| {
                        let units: Vec<serde_json::Value> = x[1].as_array()?.clone();
                        let touched = recs[g + 1..end]
                            .iter()
                            .filter(|r| units.contains(&r["u"]) || units.contains(&r["src"]))
                            .all(|r| leaf(r, "ctrl"));
                        (x[2] == true && touched).then(|| (g, k, units.clone(), end))
                    })
                })
                .expect("a ForceTick with an idle in-combat group");
            recs[at]["v"].as_array_mut().unwrap().remove(k);
            let mut j = end;
            while j > at {
                if leaf(&recs[j], "ctrl") && units.contains(&recs[j]["u"]) {
                    recs.remove(j);
                }
                j -= 1;
            }
            for (n, r) in recs.iter_mut().enumerate() {
                r["seq"] = json!(n);
            }
        }),
        (C, |recs| {
            for r in recs.iter_mut().filter(|r| leaf(r, "mrange")) {
                r["v"] = json!("-100");
            }
        }),
        // Review round 10: a two-point path `FixPathIfTooShort` would have replaced.
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "path") && r["fake"] == false);
            let p0 = recs[at]["v"][0].clone();
            recs[at]["v"] = json!([p0.clone(), p0]);
        }),
        // The observer without the group records.
        ("driven replay: the recording declares its ai observer without", |recs| {
            let at = pos(recs, |r| r["k"] == "begin");
            recs[at]["observers"] =
                json!(["cooldowns", "turn_ticks", "commands", "attacks", "approach", "ai"]);
        }),
    ];
    let mut missed = Vec::new();
    for (i, (want, change)) in cases.into_iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
        change(&mut changed.records);
        let _ = replay::run_with_tape(&mut w, &changed, &trace);
        if w.valid() || !w.coverage.unsupported.keys().any(|k| k.starts_with(want)) {
            missed.push(format!(
                "case {i}: {want}: {:?}",
                w.coverage.unsupported.keys().collect::<Vec<_>>()
            ));
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
}

/// The fights the engagement records are damaged in: one with an attack of opportunity, one
/// with provokes (`poc5-tb-91/tb-focus` has both: a disengage's attack of opportunity, the
/// archer's provoke).
const ENGAGE: (&str, &str) = ("poc5-tb-91", "tb-focus");
const PROVOKE: (&str, &str) = ("poc5-tb-91", "tb-focus");

/// Every way of losing, changing or adding an engagement record (slice E) is reported.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-91 capture"]
fn missing_or_wrong_engage_records_are_reported() {
    use serde_json::Value as V;
    let (Some(aoo_dir), Some(prov_dir)) =
        (fixture_in(ENGAGE.0, ENGAGE.1), fixture_in(PROVOKE.0, PROVOKE.1))
    else {
        eprintln!("{ENGAGE:?} or {PROVOKE:?} not present: skipped");
        return;
    };
    // The fixtures have what the cases change: a successful attack of opportunity (from a
    // disengage, with its count's cooldown write), and a rule handler's provoke.
    let has = |dir: &PathBuf, f: &dyn Fn(&serde_json::Value) -> bool| {
        let (_, t) = replay::prepare_world(SAVE, dir, "record").expect("prepare");
        t.records.iter().any(f)
    };
    assert!(has(&aoo_dir, &|r| r["ev"] == "eng_aoo" && r["ret"] == true && r["dis"] == true));
    assert!(has(&aoo_dir, &|r| r["ev"] == "cd_write"
        && r["writer"] == "UnitCombatState.set_AttackOfOpportunityCount"));
    assert!(has(&prov_dir, &|r| r["ev"] == "eng_prov"));
    type Change = fn(&mut Vec<V>);
    fn ev(r: &V, e: &str) -> bool {
        r["k"] == "obs" && r["ev"] == e
    }
    fn leaf(r: &V, f: &str) -> bool {
        ev(r, "uc_v") && r["f"] == f
    }
    fn pos(recs: &[V], f: impl Fn(&V) -> bool) -> usize {
        recs.iter().position(f).expect("a record to change")
    }
    // The last leaf `f` before record `j` with field `k` (the engagement's own, not a command's).
    fn before(recs: &[V], j: usize, f: &str, k: &str) -> usize {
        (0..j).rev().find(|&i| leaf(&recs[i], f) && !recs[i][k].is_null()).expect("a leaf before")
    }
    fn engage(recs: &[V]) -> usize {
        pos(recs, |r| ev(r, "eng_e") && r["fn"] == "Engage")
    }
    fn aoo(recs: &[V]) -> usize {
        pos(recs, |r| ev(r, "eng_aoo") && r["ret"] == true)
    }
    const C: &str = "driven replay: commands:";
    let cases: [(&str, Change); 64] = [
        // The entry.
        ("driven replay: a uc_eng record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_eng") && r["t"].as_i64() > Some(300));
            recs[at]["extra"] = json!(1);
        }),
        ("driven replay: a uc_eng record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "uc_eng") && r["t"].as_i64() > Some(300));
            recs[at]["origin"] =
                json!("Kingmaker.Controllers.Combat.UnitCombatEngagementController.Tick < X");
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "uc_eng_end") && r["t"].as_i64() > Some(300));
            recs[at]["exc"] = json!(true);
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "uc_eng_end") && r["t"].as_i64() > Some(300));
            recs.remove(at);
        }),
        ("driven replay: a command record (uc_eng) of another frame", |recs| {
            let at = pos(recs, |r| ev(r, "uc_eng") && r["t"].as_i64() > Some(300));
            let t = recs[at]["t"].as_i64().unwrap();
            recs[at]["t"] = json!(t + 1);
        }),
        ("driven replay: a frame without the engagement controller's tick", |recs| {
            let at = pos(recs, |r| ev(r, "uc_eng") && r["t"].as_i64() > Some(300));
            let end = (at..recs.len()).find(|&i| ev(&recs[i], "uc_eng_end")).unwrap();
            recs.drain(at..=end);
        }),
        // The checks.
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "eng_tu") && r["t"].as_i64() > Some(300));
            recs[at]["cnt"] = json!(2);
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "eng_tu") && r["t"].as_i64() > Some(300));
            recs[at]["u"] = json!("51DD");
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "eng_tu") && r["t"].as_i64() > Some(300));
            recs.remove(at);
        }),
        (C, |recs| {
            let at = engage(recs);
            recs[at]["eu"] = json!([]);
        }),
        (C, |recs| {
            let at = engage(recs);
            recs[at]["eb"] = json!([]);
        }),
        (C, |recs| {
            let at = engage(recs);
            recs[at]["dat"] = json!(["51DD"]);
        }),
        (C, |recs| {
            let at = engage(recs);
            recs[at]["fn"] = json!("Disengage");
        }),
        (C, |recs| {
            let at = engage(recs);
            recs.remove(at);
        }),
        (C, |recs| {
            let at = engage(recs);
            recs[at]["extra"] = json!(1);
        }),
        (C, |recs| {
            // A disengage's order of engaged units (two left).
            let at =
                pos(recs, |r| ev(r, "eng_e") && r["eu"].as_array().is_some_and(|a| a.len() >= 2));
            recs[at]["eu"].as_array_mut().unwrap().reverse();
        }),
        (C, |recs| {
            let at = aoo(recs);
            recs[at]["ret"] = json!(false);
        }),
        (C, |recs| {
            let at = aoo(recs);
            recs[at]["cnt"] = json!(1);
        }),
        (C, |recs| {
            let at = aoo(recs);
            recs[at]["abu"] = json!([]);
        }),
        (C, |recs| {
            let at = aoo(recs);
            recs[at]["dis"] = json!(false);
        }),
        // The leaves of reach and engagement.
        (C, |recs| {
            let e = engage(recs);
            let at = before(recs, e, "corp", "u");
            recs[at]["v"] = json!("0.4");
        }),
        (C, |recs| {
            let e = engage(recs);
            let at = before(recs, e, "sqd", "u");
            recs[at]["v"] = json!("10000");
        }),
        (C, |recs| {
            let e = engage(recs);
            let at = before(recs, e, "sqd", "u");
            recs[at]["v"] = json!("-1");
        }),
        (C, |recs| {
            let e = engage(recs);
            let at = before(recs, e, "los", "u");
            recs[at]["v"] = json!(false);
        }),
        (C, |recs| {
            let e = engage(recs);
            let at = before(recs, e, "conc", "u");
            recs[at]["v"] = json!(2);
        }),
        (C, |recs| {
            let e = engage(recs);
            let at = before(recs, e, "conc", "u");
            recs[at]["attack"] = json!(true);
        }),
        (C, |recs| {
            let e = engage(recs);
            let at = before(recs, e, "trange", "u");
            recs[at]["v"] = V::Null;
        }),
        (C, |recs| {
            let e = engage(recs);
            let at = before(recs, e, "trange", "u");
            recs[at]["v"] = json!("-1");
        }),
        (C, |recs| {
            let e = engage(recs);
            let at = before(recs, e, "threat", "u");
            recs[at]["v"] = V::Null;
        }),
        (C, |recs| {
            let e = engage(recs);
            let at = (0..e).rev().find(|&i| leaf(&recs[i], "enemies")).unwrap();
            recs[at]["v"].as_array_mut().unwrap().push(json!("ZZZZ"));
        }),
        (C, |recs| {
            let e = engage(recs);
            let at = (0..e).rev().find(|&i| leaf(&recs[i], "enemies")).unwrap();
            recs[at]["v"] = json!([]);
        }),
        // The disengage and its attack of opportunity.
        ("engagement: Disengage in tactical combat", |recs| {
            let a = aoo(recs);
            let at = (0..a).rev().find(|&i| leaf(&recs[i], "tact")).unwrap();
            recs[at]["v"] = json!(true);
        }),
        (C, |recs| {
            // `ShouldAttackOnDisengage`'s second read: the target it set.
            let a = aoo(recs);
            let at = before(recs, a, "last", "u");
            recs[at]["v"] = json!("523B");
        }),
        (C, |recs| {
            let a = aoo(recs);
            let at = (0..a).rev().find(|&i| leaf(&recs[i], "immune")).unwrap();
            recs[at]["v"] = json!(true);
        }),
        (C, |recs| {
            let a = aoo(recs);
            let t = (0..a).rev().find(|&i| leaf(&recs[i], "tact")).unwrap();
            let at = (t..a).find(|&i| leaf(&recs[i], "motion")).unwrap();
            recs[at]["v"] = json!(true);
        }),
        (C, |recs| {
            let a = aoo(recs);
            let at = before(recs, a, "prevent", "u");
            recs[at]["v"] = json!(true);
        }),
        (C, |recs| {
            let a = aoo(recs);
            let at = before(recs, a, "caic", "u");
            recs[at]["v"] = json!(false);
        }),
        (C, |recs| {
            let a = aoo(recs);
            let at = (0..a).rev().find(|&i| leaf(&recs[i], "passive")).unwrap();
            recs[at]["v"] = json!(true);
        }),
        (C, |recs| {
            let a = aoo(recs);
            let at = before(recs, a, "canatk", "u");
            recs[at]["v"] = json!(false);
        }),
        ("engagement: AttackOfOpportunity: a target forced to move", |recs| {
            let a = aoo(recs);
            let m = before(recs, a, "memc", "of");
            let at = (0..m).rev().filter(|&i| leaf(&recs[i], "fmove")).nth(1).unwrap();
            recs[at]["v"] = json!(true);
        }),
        (C, |recs| {
            let a = aoo(recs);
            let at = before(recs, a, "memc", "of");
            recs[at]["v"] = json!(false);
        }),
        (C, |recs| {
            let a = aoo(recs);
            let at = before(recs, a, "untgt", "src");
            recs[at]["v"] = json!(true);
        }),
        // The attack of opportunity it runs.
        (C, |recs| {
            let a = aoo(recs);
            let at = (0..a).rev().find(|&i| ev(&recs[i], "uc_new")).unwrap();
            recs[at]["aoo"] = json!("11");
        }),
        (C, |recs| {
            let a = aoo(recs);
            let at = (0..a).rev().find(|&i| ev(&recs[i], "uc_new")).unwrap();
            recs[at]["ap"] = json!("0.3|0|00");
        }),
        (C, |recs| {
            let a = aoo(recs);
            let at = (0..a).rev().find(|&i| ev(&recs[i], "uc_run")).unwrap();
            recs[at]["fq"] = json!(true);
        }),
        // The count's cooldown write and the NextCommandTime writes.
        (C, |recs| {
            let a = aoo(recs);
            let at = (0..a).rev().find(|&i| ev(&recs[i], "cd_write")).unwrap();
            recs[at]["a"] = json!("0|0|0|0|0");
        }),
        (C, |recs| {
            let a = aoo(recs);
            let at = (0..a).rev().find(|&i| ev(&recs[i], "cd_write")).unwrap();
            recs.remove(at);
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "ai_nct") && r["site"] == "Disengage");
            recs.remove(at);
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "ai_nct") && r["site"] == "Engage");
            recs[at]["v"] = json!("1");
        }),
        ("driven replay: an engagement's write of NextCommandTime the sim didn't make", |recs| {
            let at = pos(recs, |r| ev(r, "ai_nct") && r["site"] == "Engage");
            let mut x = recs[at].clone();
            // (Outside an engagement tick: in the window's first frame, after its snapshot.)
            let k = pos(recs, |r| ev(r, "uc_snap")) + 1;
            x["t"] = recs[k]["t"].clone();
            recs.insert(k, x);
        }),
        // The initial state.
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "eng_init") && r["u"] == "5671");
            recs[at]["cnt"] = json!(3);
        }),
        ("driven replay: engagement at the window's start", |recs| {
            let at = pos(recs, |r| ev(r, "eng_init") && r["u"] == "5671");
            recs[at]["eu"] = json!(["51DD"]);
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "eng_init") && r["u"] == "5671");
            recs.remove(at);
        }),
        ("driven replay: a unit's engagement given twice", |recs| {
            let at = pos(recs, |r| ev(r, "eng_init") && r["u"] == "5671");
            let x = recs[at].clone();
            recs.insert(at, x);
        }),
        ("driven replay: an eng_init record it can't read", |recs| {
            let at = pos(recs, |r| ev(r, "eng_init") && r["u"] == "5671");
            recs[at]["extra"] = json!(1);
        }),
        // A provoke the sim doesn't model (another caller), and one from the rule handler the sim
        // didn't make.
        (
            "driven replay: an attack of opportunity provoked or forced that the sim doesn't model",
            |recs| {
                let at = pos(recs, |r| ev(r, "uc_eng") && r["t"].as_i64() > Some(300));
                let t = recs[at]["t"].clone();
                let x = json!({"seq": 0, "k": "obs", "t": t, "ev": "eng_prov", "fn": "ProvokeAttackOfOpportunity",
                "u": "5671", "tgt": null, "trick": false, "origin": "?"});
                recs.insert(at, x);
                for (n, r) in recs.iter_mut().enumerate() {
                    r["seq"] = json!(n);
                }
            },
        ),
        (C, |recs| {
            let a = aoo(recs);
            let at = (0..a).rev().find(|&i| ev(&recs[i], "cd_write")).unwrap();
            recs[at]["caller"] = json!("wrong");
        }),
        ("driven replay: engagement at the window's start", |recs| {
            let at = pos(recs, |r| ev(r, "eng_init") && r["u"] == "5671");
            recs[at]["eu_slots"] = json!(2);
        }),
        ("driven replay: a unit without its initial engagement (eng_init)", |recs| {
            // A unit that never fights.
            let at = pos(recs, |r| ev(r, "eng_init") && r["u"] == "4F22");
            recs.remove(at);
        }),
        ("driven replay: an eng_init record after the window's first engagement tick", |recs| {
            let at = pos(recs, |r| ev(r, "eng_init") && r["u"] == "4F22");
            let mut x = recs.remove(at);
            let k = pos(recs, |r| ev(r, "uc_eng_end"));
            x["t"] = recs[k]["t"].clone();
            recs.insert(k + 1, x);
        }),
        // The observer without the AI's records.
        ("driven replay: the recording declares its engage observer without", |recs| {
            let at = pos(recs, |r| r["k"] == "begin");
            recs[at]["observers"] = json!([
                "groups",
                "cooldowns",
                "turn_ticks",
                "commands",
                "attacks",
                "approach",
                "engage"
            ]);
        }),
        // The attack environment's engagement.
        ("driven replay: an attack environment's EngagedBy differs", |recs| {
            let at = pos(recs, |r| {
                r["k"] == "env"
                    && r["rule"] == "RuleAttackRoll"
                    && r["target_engaged_by"].as_array().is_some_and(|a| !a.is_empty())
            });
            recs[at]["target_engaged_by"] = json!([]);
        }),
        // Review round 3: every frame of the attack of opportunity's caller chains.
        (C, |recs| {
            let a = aoo(recs);
            let at = (0..a).rev().find(|&i| ev(&recs[i], "uc_run")).unwrap();
            let o = recs[at]["origin"]
                .as_str()
                .unwrap()
                .replace("AttackOfOpportunity < ", "AttackOfOpportunity < Wrong.Unrelated < ");
            recs[at]["origin"] = json!(o);
        }),
        (C, |recs| {
            let a = aoo(recs);
            let at = (0..a).rev().find(|&i| ev(&recs[i], "cd_write")).unwrap();
            let o = format!("{} < Wrong", recs[at]["caller"].as_str().unwrap());
            recs[at]["caller"] = json!(o);
        }),
    ];
    // The provoke's records (`PROVOKE`'s archer provokes with each ranged attack).
    let provokes: [(&str, Change); 7] = [
        // Review round 2: a rule handler's provoke moved out of its rule (after the root's end).
        (
            "driven replay: an attack of opportunity provoked or forced that the sim doesn't model",
            |recs| {
                let at = pos(recs, |r| ev(r, "eng_prov"));
                let root = (0..at)
                    .rev()
                    .find(|&i| recs[i]["k"] == "rule" && recs[i]["depth"] == 0)
                    .unwrap();
                let id = recs[root]["id"].clone();
                let x = recs.remove(at);
                let end = (at..recs.len())
                    .find(|&i| recs[i]["k"] == "rule_end" && recs[i]["id"] == id)
                    .unwrap();
                recs.insert(end + 1, x);
                for (n, r) in recs.iter_mut().enumerate() {
                    r["seq"] = json!(n);
                }
            },
        ),
        // Review round 1: each provoke call, its payload and caller; the initial state complete
        // and first; the turn's disengage immunity the sim's.
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "eng_prov"));
            recs[at]["trick"] = json!(true);
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "eng_prov"));
            recs[at]["tgt"] = json!("51DD");
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "eng_prov"));
            recs[at]["origin"] = json!(
                "Kingmaker.Controllers.Combat.UnitCombatEngagementController.ProvokeAttackOfOpportunity < X"
            );
        }),
        (C, |recs| {
            // Another unit provoking than the rule's initiator.
            let at = pos(recs, |r| ev(r, "eng_prov"));
            recs[at]["u"] = json!("51DD");
        }),
        (
            "driven replay: the attacks of opportunity provoked differ from the sim's queue",
            |recs| {
                let at = pos(recs, |r| ev(r, "eng_prov"));
                recs.remove(at);
                for (n, r) in recs.iter_mut().enumerate() {
                    r["seq"] = json!(n);
                }
            },
        ),
        // Review round 3: a provoke under another rule's handler (its caller's subscribers list).
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "eng_prov"));
            let o = recs[at]["origin"].as_str().unwrap().replace(
                "RulebookSubscribersList`1[[Kingmaker.RuleSystem.Rules.RuleAttackRoll,",
                "RulebookSubscribersList`1[[Kingmaker.RuleSystem.Rules.RuleAttackWithWeapon,",
            );
            recs[at]["origin"] = json!(o);
        }),
    ];
    let mut missed = Vec::new();
    for (dir, cases) in [(&aoo_dir, &cases[..]), (&prov_dir, &provokes[..])] {
        for (i, (want, change)) in cases.iter().enumerate() {
            let (mut w, trace) = replay::prepare_world(SAVE, dir, "record").expect("prepare");
            let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
            change(&mut changed.records);
            let _ = replay::run_with_tape(&mut w, &changed, &trace);
            if w.valid() || !w.coverage.unsupported.keys().any(|k| k.starts_with(want)) {
                missed.push(format!(
                    "{}: case {i}: {want}: {:?}",
                    dir.display(),
                    w.coverage.unsupported.keys().collect::<Vec<_>>()
                ));
            }
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
}

/// The fight the targets' records are damaged in: `UpdateCombatTarget`'s writes and resets, a
/// disengage's write and restore, leaving combat's clears.
const TARGETS: (&str, &str) = ("poc5-tb-92", "tb-focus");

/// Every way of losing, changing or adding a targets record (slice F) is reported.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-92 capture"]
fn missing_or_wrong_target_records_are_reported() {
    use serde_json::Value as V;
    let Some(dir) = fixture_in(TARGETS.0, TARGETS.1) else {
        eprintln!("{TARGETS:?} not present: skipped");
        return;
    };
    type Change = fn(&mut Vec<V>);
    fn ev(r: &V, e: &str) -> bool {
        r["k"] == "obs" && r["ev"] == e
    }
    fn leaf(r: &V, f: &str) -> bool {
        ev(r, "uc_v") && r["f"] == f
    }
    fn pos(recs: &[V], f: impl Fn(&V) -> bool) -> usize {
        recs.iter().position(f).expect("a record to change")
    }
    fn caller(r: &V) -> String {
        r["origin"].as_str().unwrap_or("").split(" < ").nth(1).unwrap_or("").to_owned()
    }
    fn write(recs: &[V], by: &str, f: &str) -> usize {
        pos(recs, |r| ev(r, "tgt_w") && r["f"] == f && caller(r).ends_with(by))
    }
    fn uct(recs: &[V]) -> usize {
        write(recs, "UnitCommands.UpdateCombatTarget", "last")
    }
    fn restore(recs: &[V]) -> usize {
        pos(recs, |r| ev(r, "tgt_w") && caller(r).contains("<ShouldAttackOnDisengage>b__"))
    }
    // `UpdateCombatTarget`'s reset of `NextCommandTime` (a command not the AI's on no target).
    fn nct(recs: &[V]) -> usize {
        pos(recs, |r| ev(r, "ai_nct") && r["site"] == "UpdateCombatTarget")
    }
    // A leaf `tact` of `UpdateCombatTarget` (with its command), after the window's first frames.
    fn tact(recs: &[V]) -> usize {
        pos(recs, |r| leaf(r, "tact") && r["t"].as_i64() > Some(300))
    }
    fn first_init(recs: &[V]) -> usize {
        pos(recs, |r| ev(r, "tgt_init"))
    }
    let (_, t) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
    // The fixture has what the cases change.
    for want in [
        "UnitCommands.UpdateCombatTarget",
        "UnitCombatState.ShouldAttackOnDisengage",
        "UnitCombatState.Clear",
    ] {
        assert!(t.records.iter().any(|r| ev(r, "tgt_w") && caller(r).ends_with(want)), "{want}");
    }
    assert!(t.records.iter().any(|r| ev(r, "ai_nct") && r["site"] == "UpdateCombatTarget"));
    fn renumber(recs: &mut [V]) {
        for (n, r) in recs.iter_mut().enumerate() {
            r["seq"] = json!(n);
        }
    }
    // A write by `origin` of the first `UpdateCombatTarget` write's target to its unit's field
    // `f`, and its undoing, at the start of that write's frame.
    fn insert_balanced(recs: &mut Vec<V>, f: &str, origin: &str) {
        let at = uct(recs);
        let mut a = recs[at].clone();
        a["f"] = json!(f);
        a["origin"] = json!(origin);
        let mut b = a.clone();
        b["v"] = json!(null);
        b["origin"] = json!(STOP.replace(
            "set_LastTarget",
            if f == "last" { "set_LastTarget" } else { "set_ManualTarget" }
        ));
        let t = recs[at]["t"].clone();
        let to = pos(recs, |r| r["k"] == "tick" && r["t"] == t) + 1;
        recs.insert(to, b);
        recs.insert(to, a);
        renumber(recs);
    }
    const STOP: &str = "Kingmaker.Controllers.Combat.UnitCombatState.set_LastTarget < Kingmaker.EntitySystem.Entities.UnitEntityData.Stop < X.Y";
    // A record put inside the first initiative roll's span (a root the replay drives: its frame,
    // renumbered).
    fn insert_in_root(recs: &mut Vec<V>, mut r: V) {
        let root = recs
            .iter()
            .position(|x| {
                x["k"] == "rule"
                    && x["parent"] == 0
                    && x["class"].as_str().is_some_and(|c| c.ends_with("RuleInitiativeRoll"))
            })
            .expect("an initiative roll");
        r["t"] = recs[root]["t"].clone();
        recs.insert(root + 1, r);
        renumber(recs);
    }
    const C: &str = "driven replay: commands:";
    const DIFF: &str = "driven replay: the sim's target write";
    const UNMADE: &str = "driven replay: a target write the sim didn't make";
    const UNREAD: &str = "driven replay: a tgt_w record it can't read";
    let cases: [(&str, Change); 90] = [
        // The initial state.
        ("driven replay: a tgt_init record it can't read", |recs| {
            let at = first_init(recs);
            recs[at]["extra"] = json!(1);
        }),
        ("driven replay: a tgt_init record it can't read", |recs| {
            let at = first_init(recs);
            recs[at]["u"] = json!("no-such-unit");
        }),
        ("driven replay: a tgt_init record it can't read", |recs| {
            let at = first_init(recs);
            recs[at]["manual"] = json!("no-such-unit");
        }),
        ("driven replay: a tgt_init record it can't read", |recs| {
            let at = first_init(recs);
            recs[at].as_object_mut().unwrap().remove("last");
        }),
        ("driven replay: a unit without its initial targets", |recs| {
            let at = first_init(recs);
            recs.remove(at);
        }),
        // (A second record of a unit breaks the block's order.)
        ("driven replay: a tgt_init record out of its block", |recs| {
            let at = first_init(recs);
            let r = recs[at].clone();
            recs.insert(at + 1, r);
        }),
        ("driven replay: a tgt_init record after the window's first frame", |recs| {
            let at = first_init(recs);
            let mut r = recs.remove(at);
            let to = pos(recs, |x| x["k"] == "tick" && x["t"].as_i64() > r["t"].as_i64());
            r["t"] = recs[to]["t"].clone();
            recs.insert(to + 1, r);
        }),
        // A unit's initial `LastTarget` already the one its first write sets: the sim's setter
        // changes nothing, and its next write meets the recorded one.
        (C, |recs| {
            let at = uct(recs);
            let (u, v) = (recs[at]["u"].clone(), recs[at]["v"].clone());
            let i = pos(recs, |r| ev(r, "tgt_init") && r["u"] == u);
            recs[i]["last"] = v;
        }),
        // The writes the sim makes in an entry, each taken where it's made.
        (C, |recs| {
            let at = uct(recs);
            recs.remove(at + 1);
            recs.remove(at);
        }),
        (C, |recs| {
            let at = uct(recs);
            recs.remove(at);
        }),
        (C, |recs| {
            let at = uct(recs);
            recs[at]["v"] = json!(null);
        }),
        (C, |recs| {
            let at = uct(recs);
            let u = recs[at]["u"].clone();
            let other =
                recs.iter().find(|r| ev(r, "tgt_init") && r["u"] != u).unwrap()["u"].clone();
            recs[at]["v"] = other;
        }),
        (C, |recs| {
            let at = uct(recs);
            let u = recs[at]["u"].clone();
            let other =
                recs.iter().find(|r| ev(r, "tgt_init") && r["u"] != u).unwrap()["u"].clone();
            recs[at]["u"] = other;
        }),
        (C, |recs| {
            let at = uct(recs);
            recs[at]["f"] = json!("manual");
        }),
        (C, |recs| {
            let at = uct(recs);
            recs[at]["extra"] = json!(1);
        }),
        (C, |recs| {
            let at = uct(recs);
            recs[at]["v"] = json!("no-such-unit");
        }),
        (C, |recs| {
            let at = uct(recs);
            recs[at]["origin"] =
                json!("Kingmaker.Controllers.Combat.UnitCombatState.set_LastTarget < X.Y < Z.W");
        }),
        (C, |recs| {
            let at = uct(recs);
            recs[at]["origin"] = json!(null);
        }),
        (C, |recs| {
            let at = uct(recs);
            recs[at]["origin"] = json!("WrathOracle.TurnBased.IssueHovered");
        }),
        (UNMADE, |recs| {
            let at = uct(recs);
            let r = recs[at].clone();
            let to = at + 1 + recs[at + 1..].iter().position(|r| r["k"] == "tick").unwrap() + 1;
            let mut r = r;
            r["t"] = recs[to - 1]["t"].clone();
            recs.insert(to, r);
            renumber(recs);
        }),
        // `ShouldAttackOnDisengage`'s write and its restore.
        (C, |recs| {
            let at = write(recs, "UnitCombatState.ShouldAttackOnDisengage", "last");
            recs.remove(at);
        }),
        (C, |recs| {
            let at = restore(recs);
            recs[at]["v"] = json!(null);
        }),
        (C, |recs| {
            let at = restore(recs);
            recs.remove(at);
        }),
        (C, |recs| {
            let at = restore(recs);
            let o = recs[at]["origin"].as_str().unwrap().replace(".Dispose", ".Other");
            recs[at]["origin"] = json!(o);
        }),
        // Leaving combat's clear, queued.
        (UNREAD, |recs| {
            let at = write(recs, "UnitCombatState.Clear", "last");
            let o = recs[at]["origin"].as_str().unwrap().replace(
                "UnitCombatState.Clear < Kingmaker.Controllers.Combat.UnitCombatState.LeaveCombat",
                "UnitCombatState.Clear < X.Y",
            );
            recs[at]["origin"] = json!(o);
        }),
        (DIFF, |recs| {
            let at = write(recs, "UnitCombatState.Clear", "last");
            recs[at]["v"] = recs[at]["u"].clone();
        }),
        (
            "driven replay: a UnitCombatState.Clear target write the recording doesn't have",
            |recs| {
                let at = write(recs, "UnitCombatState.Clear", "last");
                recs.remove(at);
            },
        ),
        (DIFF, |recs| {
            let at = write(recs, "UnitCombatState.Clear", "last");
            let r = recs[at].clone();
            recs.insert(at + 1, r);
        }),
        // Writes of the writers whose writes follow their entries (`Stop`), out of place: one
        // inside a `Clear`, one at a frame's start, one before a read the sim then checks.
        (UNMADE, |recs| {
            let at = write(recs, "UnitCombatState.Clear", "last");
            let mut r = recs[at].clone();
            r["origin"] = json!(STOP);
            recs.insert(at, r);
        }),
        (UNMADE, |recs| {
            let at = uct(recs);
            let mut r = recs[at].clone();
            r["origin"] = json!(STOP);
            let to = at + recs[at..].iter().position(|r| r["k"] == "tick").unwrap() + 1;
            r["t"] = recs[to - 1]["t"].clone();
            recs.insert(to, r);
            renumber(recs);
        }),
        (C, |recs| {
            // The engagement's read of `LastTarget` after a UI write the sim takes.
            let at =
                pos(recs, |r| leaf(r, "last") && !r["v"].is_null() && r["t"].as_i64() > Some(1600));
            let mut r = recs[at].clone();
            r.as_object_mut().unwrap().remove("f");
            r["ev"] = json!("tgt_w");
            r["f"] = json!("last");
            r["v"] = json!(null);
            r["origin"] = json!(STOP);
            let to = recs[..at].iter().rposition(|r| r["k"] == "tick").unwrap() + 1;
            recs.insert(to, r);
            renumber(recs);
        }),
        // The reads.
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "manual") && r["t"].as_i64() > Some(300));
            recs[at]["v"] = json!("558A");
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "last") && !r["v"].is_null());
            recs[at]["v"] = json!(null);
        }),
        // `UpdateCombatTarget`'s leaves and its reset of `NextCommandTime`.
        (C, |recs| {
            let at = tact(recs);
            recs[at]["v"] = json!(true);
        }),
        (C, |recs| {
            let at = tact(recs);
            recs.remove(at);
        }),
        (C, |recs| {
            let at = nct(recs);
            recs.remove(at);
        }),
        (
            "driven replay: an UpdateCombatTarget write of NextCommandTime the sim didn't make",
            |recs| {
                let at = nct(recs);
                let r = recs[at].clone();
                recs.insert(at + 1, r);
            },
        ),
        // Review round 1: a write's caller chain bound to its site and the entry; a write moved
        // across a lifecycle record (renumbered); an input held to what its writer can write.
        (C, |recs| {
            let at = uct(recs);
            let o = recs[at]["origin"].as_str().unwrap().replacen(
                "UnitCommands.UpdateCombatTarget < Kingmaker.UnitLogic.Commands.UnitCommands.Run",
                "UnitCommands.UpdateCombatTarget < Kingmaker.UnitLogic.Commands.UnitCommands.TryUseSpecialHandlers < Kingmaker.UnitLogic.Commands.UnitCommands.Run",
                1,
            );
            recs[at]["origin"] = json!(o);
        }),
        (C, |recs| {
            let at = uct(recs);
            recs[at]["origin"] = json!(
                "Kingmaker.Controllers.Combat.UnitCombatState.set_LastTarget < Kingmaker.UnitLogic.Commands.UnitCommands.UpdateCombatTarget < Kingmaker.UnitLogic.Commands.UnitCommands.Run < X.Y"
            );
        }),
        (C, |recs| {
            let at = uct(recs);
            let pair: Vec<V> = recs.drain(at..at + 2).collect();
            let to = at + recs[at..].iter().position(|r| ev(r, "uc_v")).unwrap() + 1;
            for (k, r) in pair.into_iter().enumerate() {
                recs.insert(to + k, r);
            }
            renumber(recs);
        }),
        (C, |recs| {
            let at = uct(recs);
            let pair: Vec<V> = recs.drain(at..at + 2).collect();
            let to = recs[..at].iter().rposition(|r| ev(r, "uc_v")).unwrap();
            for (k, r) in pair.into_iter().enumerate() {
                recs.insert(to + k, r);
            }
            renumber(recs);
        }),
        // At a frame's start: `Stop` setting a target, a click setting `LastTarget` (no writer
        // does), a destruction (not modelled), each undone after.
        (UNMADE, |recs| {
            insert_balanced(recs, "last", STOP);
        }),
        (UNMADE, |recs| {
            insert_balanced(
                recs,
                "last",
                "Kingmaker.Controllers.Combat.UnitCombatState.set_LastTarget < Kingmaker.Controllers.Clicks.Handlers.ClickUnitHandler.OnClick < X.Y",
            );
        }),
        ("driven replay: a target write of the UI's or a destruction's", |recs| {
            insert_balanced(
                recs,
                "manual",
                "Kingmaker.Controllers.Combat.UnitCombatState.set_ManualTarget < Kingmaker.Controllers.Combat.UnitCombatState.HandleEntityDestruction < X.Y",
            );
        }),
        // Review round 2: every frame between a write's site and its entry checked; `Clear`'s
        // writes inside their `Clear` and from its `LeaveCombat`'s callers; the initial block in
        // place and in order; an input right after its writer's own call.
        (C, |recs| {
            let at = write(recs, "UnitCombatState.ShouldAttackOnDisengage", "last");
            let o = recs[at]["origin"].as_str().unwrap().replace(
                "UnitCombatEngagementController.TickUnit",
                "UnitCombatEngagementController.Other",
            );
            recs[at]["origin"] = json!(o);
        }),
        (C, |recs| {
            let at = uct(recs);
            let o = recs[at]["origin"].as_str().unwrap().replacen(
                " < Kingmaker.UnitLogic.Commands.UnitCommands.Run < ",
                " < X.Y < Kingmaker.UnitLogic.Commands.UnitCommands.Run < ",
                1,
            );
            recs[at]["origin"] = json!(o);
        }),
        (
            "driven replay: a UnitCombatState.Clear target write the recording doesn't have",
            |recs| {
                let at = write(recs, "UnitCombatState.Clear", "last");
                let r = recs.remove(at);
                let to = at + recs[at..].iter().position(|x| ev(x, "uc_x_end")).unwrap() + 1;
                recs.insert(to, r);
                renumber(recs);
            },
        ),
        (DIFF, |recs| {
            let at = write(recs, "UnitCombatState.Clear", "last");
            let o = recs[at]["origin"].as_str().unwrap().replacen(
                "UnitLifeController.SetLifeState",
                "X.Y",
                1,
            );
            recs[at]["origin"] = json!(o);
        }),
        ("driven replay: a tgt_init record out of its block", |recs| {
            let a = first_init(recs);
            let b = a + recs[a..].iter().position(|r| !ev(r, "tgt_init")).unwrap();
            recs[a..b].reverse();
            renumber(recs);
        }),
        ("driven replay: a tgt_init record out of its block", |recs| {
            let at = first_init(recs);
            let r = recs.remove(at);
            let to = at + recs[at..].iter().position(|x| ev(x, "uc_eng_end")).unwrap() + 1;
            recs.insert(to, r);
            renumber(recs);
        }),
        (UNMADE, |recs| {
            // `TryCancelCommands` clearing a unit's `ManualTarget` mid-frame, a click restoring it.
            let at = write(recs, "UnitCommands.UpdateCombatTarget", "manual");
            let mut a = recs[at].clone();
            a["v"] = json!(null);
            a["origin"] = json!(
                "Kingmaker.Controllers.Combat.UnitCombatState.set_ManualTarget < Kingmaker.EntitySystem.Entities.UnitEntityData.TryCancelCommands < X.Y"
            );
            let mut b = recs[at].clone();
            b["origin"] = json!("WrathOracle.TurnBased.IssueHovered");
            let to = at + recs[at..].iter().position(|x| ev(x, "uc_cs")).unwrap() + 1;
            a["t"] = recs[to - 1]["t"].clone();
            b["t"] = recs[to - 1]["t"].clone();
            recs.insert(to, b);
            recs.insert(to, a);
            renumber(recs);
        }),
        // Review round 3: a `Stop` whose clears are missing; `Clear`'s whole chain; the targets'
        // block not interleaved with eng_init's; a click's write the sim's (none recorded: it
        // changed nothing).
        (C, |recs| {
            // `Stop`'s `InterruptAll` of a unit whose `LastTarget` is set, without its clears.
            let at = uct(recs);
            let u = recs[at]["u"].clone();
            let x = pos(recs, |r| {
                ev(r, "uc_x") && r["u"] == u && r["t"].as_i64() > recs[at]["t"].as_i64()
            });
            let mut e = recs[x].clone();
            let o = e["origin"].as_str().unwrap().to_owned();
            let (head, rest) = o.split_once(" < ").unwrap();
            e["fn"] = json!("InterruptAll");
            e["raise"] = json!(true);
            e["origin"] = json!(
                format!("{head} < Kingmaker.EntitySystem.Entities.UnitEntityData.Stop < {rest}")
                    .replacen(head, "Kingmaker.UnitLogic.Commands.UnitCommands.InterruptAll", 1)
            );
            let end = x + recs[x..].iter().position(|r| ev(r, "uc_x_end")).unwrap();
            let mut f = recs[end].clone();
            f["set"] = e["set"].clone();
            recs.insert(x, f);
            recs.insert(x, e);
            renumber(recs);
        }),
        (
            "driven replay: a UnitCombatState.Clear's target writes aren't from the LeaveCombat",
            |recs| {
                let at = write(recs, "UnitCombatState.Clear", "last");
                let o = recs[at]["origin"]
                    .as_str()
                    .unwrap()
                    .replace("UnitLifeController.TickOnUnit", "X.Y");
                recs[at]["origin"] = json!(o);
            },
        ),
        ("driven replay: a tgt_init record out of its block", |recs| {
            let e = pos(recs, |r| ev(r, "eng_init"));
            let at = first_init(recs);
            let r = recs.remove(at);
            recs.insert(e + 1, r);
            renumber(recs);
        }),
        (UNMADE, |recs| {
            let at = pos(recs, |r| {
                ev(r, "uc_run")
                    && r["nested"] == false
                    && r["origin"].as_str().unwrap().contains("ClickUnitHandler.OnClick")
            });
            let end = at + recs[at..].iter().position(|r| ev(r, "uc_run_end")).unwrap();
            let u = recs[at]["u"].clone();
            let w =
                recs.iter().find(|r| ev(r, "tgt_w") && r["u"] == u && r["f"] == "manual").unwrap();
            let mut r = w.clone();
            r["t"] = recs[end]["t"].clone();
            let o =
                recs[at]["origin"].as_str().unwrap().splitn(3, " < ").nth(2).unwrap().to_owned();
            r["origin"] = json!(format!(
                "Kingmaker.Controllers.Combat.UnitCombatState.set_ManualTarget < {o}"
            ));
            recs.insert(end + 1, r);
            renumber(recs);
        }),
        // Review round 4: the driver's input bound to its own `Run`; the restore's frames exact.
        // (Its frame and turn count aren't the click's own.)
        ("driven replay: a driver's tbattack input it can't read", |recs| {
            // A second click before a prediction's `Run` (`OnClick`, not the driver's).
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            let mut r = recs[i].clone();
            let at = pos(recs, |x| {
                ev(x, "uc_run")
                    && x["nested"] == false
                    && x["t"].as_i64() > r["t"].as_i64()
                    && x["origin"].as_str().unwrap().contains("ClickUnitHandler.OnClick")
            });
            r["t"] = recs[at]["t"].clone();
            recs.insert(at, r);
            renumber(recs);
        }),
        ("driven replay: the driver's Run without its input", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            recs.remove(i);
            renumber(recs);
        }),
        (C, |recs| {
            let at = restore(recs);
            let o = recs[at]["origin"].as_str().unwrap().replace("b__120_0", "b__NOT_A_LAMBDA");
            recs[at]["origin"] = json!(o);
        }),
        (C, |recs| {
            let at = restore(recs);
            let o = recs[at]["origin"]
                .as_str()
                .unwrap()
                .replace("Assembly-CSharp, Version=", "Assembly-CSharp, CORRUPTED, Version=");
            recs[at]["origin"] = json!(o);
        }),
        // Review round 5: each driver input of its kind, with its payload, matching its `Run`; the
        // initial blocks in the oracle's sequence.
        ("driven replay: a driver's tbattack input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "move");
            recs[i]["step"] = json!("tbattack");
        }),
        ("driven replay: a driver's move input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            recs[i]["step"] = json!("move");
        }),
        ("driven replay: a driver's attack:bogus input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "move");
            recs[i]["step"] = json!("attack:bogus");
        }),
        ("driven replay: a driver's move input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "move");
            recs[i].as_object_mut().unwrap().remove("to");
        }),
        (C, |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "move");
            recs[i]["to"] = json!([1.0, 2.0, 3.0]);
        }),
        ("driven replay: an eng_init record out of the oracle's sequence", |recs| {
            // The first two units swapped in both blocks.
            let e = pos(recs, |r| ev(r, "eng_init"));
            recs.swap(e, e + 1);
            let t = first_init(recs);
            recs.swap(t, t + 1);
        }),
        ("driven replay: an eng_init record out of the oracle's sequence", |recs| {
            // Both blocks before `uc_snap`.
            let snap = pos(recs, |r| ev(r, "uc_snap"));
            let e = pos(recs, |r| ev(r, "eng_init"));
            let end = e + recs[e..]
                .iter()
                .position(|r| !ev(r, "eng_init") && !ev(r, "tgt_init"))
                .unwrap();
            let block: Vec<V> = recs.drain(e..end).collect();
            for (k, r) in block.into_iter().enumerate() {
                recs.insert(snap + k, r);
            }
            renumber(recs);
        }),
        // Review round 6: a driver input's whole payload; the `attack:` step's blueprint; the input
        // right before its command's first sight; nothing else inside a `Clear`.
        ("driven replay: a driver's tbattack input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            recs[i].as_object_mut().unwrap().remove("command");
        }),
        ("driven replay: a driver's tbattack input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            recs[i]["command"] = json!("UnitMoveTo");
        }),
        ("driven replay: a driver's tbattack input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            recs[i]["round"] = json!(999);
        }),
        ("driven replay: a driver's attack:bogus input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            let o = recs[i].as_object_mut().unwrap();
            o.insert("step".into(), json!("attack:bogus"));
            for k in ["unit", "policy", "round", "turn_index", "command", "at"] {
                o.remove(k);
            }
        }),
        (
            "driven replay: a driver's attack:00000000000000000000000000000001 input it can't read",
            |recs| {
                let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
                let o = recs[i].as_object_mut().unwrap();
                o.insert("step".into(), json!("attack:00000000000000000000000000000001"));
                for k in ["unit", "policy", "round", "turn_index", "command", "at"] {
                    o.remove(k);
                }
            },
        ),
        ("driven replay: a driver's input without its Run", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "move");
            recs.swap(i, i + 1);
            renumber(recs);
        }),
        (DIFF, |recs| {
            let at = write(recs, "UnitCombatState.Clear", "last");
            let d = recs[at..].iter().position(|r| r["k"] == "draw").unwrap() + at;
            let mut r = recs.remove(d);
            r["t"] = recs[at]["t"].clone();
            recs.insert(at, r);
            renumber(recs);
        }),
        // Review round 7: nothing recorded between the sim's consecutive writes.
        (C, |recs| {
            let at = write(recs, "UnitCommands.UpdateCombatTarget", "manual");
            let d = recs[at..].iter().position(|r| r["k"] == "draw").unwrap() + at;
            let mut r = recs.remove(d);
            r["t"] = recs[at]["t"].clone();
            recs.insert(at, r);
            renumber(recs);
        }),
        // Review round 8: `UpdateCombatTarget`'s leaf and reset at once; a click's metadata.
        (C, |recs| {
            let at = nct(recs);
            let d = recs[at..].iter().position(|r| r["k"] == "draw").unwrap() + at;
            let mut r = recs.remove(d);
            r["t"] = recs[at]["t"].clone();
            recs.insert(at, r);
            renumber(recs);
        }),
        (C, |recs| {
            // `UpdateCombatTarget`'s `tact` (in `Run`, right after the custom handler's leaf).
            let at = (1..recs.len())
                .find(|&i| {
                    leaf(&recs[i], "tact")
                        && leaf(&recs[i - 1], "custom")
                        && recs[i]["t"].as_i64() > Some(300)
                })
                .unwrap();
            let d = recs[at..].iter().position(|r| r["k"] == "draw").unwrap() + at;
            let mut r = recs.remove(d);
            r["t"] = recs[at]["t"].clone();
            recs.insert(at, r);
            renumber(recs);
        }),
        ("driven replay: a driver's tbattack input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            recs[i]["frame"] = json!(999);
        }),
        ("driven replay: a driver's tbattack input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            recs[i]["turn_index"] = json!(999);
        }),
        ("driven replay: a driver's tbattack input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            recs[i]["at"] = json!({});
        }),
        ("driven replay: a driver's tbattack input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            recs[i]["at"]["from"] = json!([1.0, 2.0]);
        }),
        ("driven replay: a driver's tbattack input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            recs[i]["at"]["extra"] = json!(1);
        }),
        // Review round 9: the reset is the next record and its own writer's; a click's vectors are
        // float32s.
        (C, |recs| {
            let at = nct(recs);
            let mut r = recs[at].clone();
            r["site"] = json!("set_AutoUseAbility");
            recs.insert(at, r);
            renumber(recs);
        }),
        ("driven replay: a driver's tbattack input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            recs[i]["at"]["from"][0] = json!(1e100);
        }),
        // Review round 10: the path's point count a C# `int`.
        ("driven replay: a driver's tbattack input it can't read", |recs| {
            let i = pos(recs, |r| r["k"] == "input" && r["step"] == "tbattack");
            recs[i]["at"]["path_points"] = json!(2_147_483_648_i64);
        }),
        // An impatience record without its observer (slice G, review round 4): inside a rule's
        // handler too.
        (C, |recs| {
            let h = pos(recs, |r| {
                r["k"] == "h"
                    && r["class"] == "Kingmaker.Controllers.OffensiveActionsController"
                    && r["phase"] == "did"
            });
            let mut r = recs.iter().find(|x| ev(x, "tgt_init")).unwrap().clone();
            let o = r.as_object_mut().unwrap();
            o.clear();
            o.insert("k".into(), json!("obs"));
            o.insert("ev".into(), json!("imp_s"));
            o.insert("at".into(), json!("bother"));
            o.insert("t".into(), recs[h]["t"].clone());
            recs.insert(h + 1, r);
            renumber(recs);
        }),
        // The impatience observer's other records without it, inside a root rule.
        ("driven replay: an impatience record without the impatience observer", |recs| {
            insert_in_root(
                recs,
                json!({"k": "obs", "ev": "uc_imp_end", "fn": "Tick", "exc": false}),
            );
        }),
        ("driven replay: an impatience record without the impatience observer", |recs| {
            insert_in_root(recs, json!({"k": "obs", "ev": "uc_v", "f": "ipic", "v": true}));
        }),
        ("driven replay: an impatience record without the impatience observer", |recs| {
            insert_in_root(
                recs,
                json!({"k": "draw", "c": "sim", "key": "Assembly-CSharp|Kingmaker.Controllers.UnitImpatienceController|UpdatePriorityTarget(UnitEntityData)|#0@121|Range(int,int)", "m": "Range_ii", "a": [0, 100], "v": 5, "rule": 0, "h": 0}),
            );
        }),
        ("driven replay: an impatience record without the impatience observer", |recs| {
            // An impatience draw outside any rule.
            let at = pos(recs, |r| r["k"] == "tick" && r["t"].as_i64() > Some(300));
            let d = json!({"k": "draw", "c": "sim", "t": recs[at]["t"], "key": "Assembly-CSharp|Kingmaker.Controllers.UnitImpatienceController|UpdatePriorityTarget(UnitEntityData)|#0@121|Range(int,int)", "m": "Range_ii", "a": [0, 100], "v": 5, "rule": 0, "h": 0});
            recs.insert(at + 1, d);
            renumber(recs);
        }),
        // The observer without the engagement's.
        ("driven replay: the recording declares its targets observer without", |recs| {
            let at = pos(recs, |r| r["k"] == "begin");
            recs[at]["observers"] = json!([
                "groups",
                "cooldowns",
                "turn_ticks",
                "commands",
                "attacks",
                "approach",
                "ai",
                "targets"
            ]);
        }),
    ];
    let mut missed = Vec::new();
    for (i, (want, change)) in cases.into_iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
        change(&mut changed.records);
        let _ = replay::run_with_tape(&mut w, &changed, &trace);
        if w.valid() || !w.coverage.unsupported.keys().any(|k| k.starts_with(want)) {
            missed.push(format!(
                "case {i}: {want}: {:?}",
                w.coverage.unsupported.keys().collect::<Vec<_>>()
            ));
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
}

/// The fight the impatience records are damaged in: the controller's ticks, `Prepare`'s
/// `UpdatePriorityTarget`s, command starts, bother units (offensive actions and damage), deaths.
const IMPATIENCE: (&str, &str) = ("poc5-tb-95", "tb-focus");

/// Every way of losing, changing or adding an impatience record (slice G) is reported.
#[test]
#[ignore = "needs the game's blueprint pack, Manual_18 and the poc5-tb-95 capture"]
fn missing_or_wrong_impatience_records_are_reported() {
    use serde_json::Value as V;
    let Some(dir) = fixture_in(IMPATIENCE.0, IMPATIENCE.1) else {
        eprintln!("{IMPATIENCE:?} not present: skipped");
        return;
    };
    type Change = fn(&mut Vec<V>);
    fn ev(r: &V, e: &str) -> bool {
        r["k"] == "obs" && r["ev"] == e
    }
    fn leaf(r: &V, f: &str) -> bool {
        ev(r, "uc_v") && r["f"] == f
    }
    fn pos(recs: &[V], f: impl Fn(&V) -> bool) -> usize {
        recs.iter().position(f).expect("a record to change")
    }
    fn renumber(recs: &mut [V]) {
        for (n, r) in recs.iter_mut().enumerate() {
            r["seq"] = json!(n);
        }
    }
    fn state(recs: &[V], at: &str) -> usize {
        pos(recs, |r| ev(r, "imp_s") && r["at"] == at && r["t"].as_i64() > Some(300))
    }
    // A tick's entry (after the window's first frames).
    fn tick(recs: &[V]) -> usize {
        pos(recs, |r| ev(r, "uc_imp") && r["fn"] == "Tick" && r["t"].as_i64() > Some(300))
    }
    fn prepare_upd(recs: &[V]) -> usize {
        pos(recs, |r| ev(r, "uc_imp") && r["nested"] == false && r["fn"] == "UpdatePriorityTarget")
    }
    // The records of the entry at `at`, through its end.
    fn entry_end(recs: &[V], at: usize) -> usize {
        let f = recs[at]["fn"].clone();
        at + recs[at..].iter().position(|r| ev(r, "uc_imp_end") && r["fn"] == f).unwrap()
    }
    fn first_init(recs: &[V]) -> usize {
        pos(recs, |r| ev(r, "imp_init"))
    }
    // A draw moved right before record `at` (of `at`'s frame).
    fn draw_before(recs: &mut Vec<V>, at: usize) {
        let d = recs[at..].iter().position(|r| r["k"] == "draw").unwrap() + at;
        let mut r = recs.remove(d);
        r["t"] = recs[at]["t"].clone();
        recs.insert(at, r);
        renumber(recs);
    }
    // An `imp_s` `at` inside a projectile's resolve (a root `RuleAttackWithWeaponResolve`'s span).
    fn in_resolve(recs: &[V], at: &str) -> usize {
        let mut root = None;
        for (i, r) in recs.iter().enumerate() {
            if r["k"] == "rule" && r["parent"] == 0 {
                root = r["class"].as_str().map(str::to_owned);
            }
            if ev(r, "imp_s")
                && r["at"] == at
                && root.as_deref().is_some_and(|c| c.ends_with("RuleAttackWithWeaponResolve"))
            {
                return i;
            }
        }
        panic!("no {at} in a projectile's resolve")
    }
    const C: &str = "driven replay: commands:";
    const IMP: &str = "driven replay: impatience:";
    let cases: [(&str, Change); 57] = [
        // The initial state.
        ("driven replay: an imp_init record it can't read", |recs| {
            let at = first_init(recs);
            recs[at]["extra"] = json!(1);
        }),
        ("driven replay: an imp_init record it can't read", |recs| {
            let at = first_init(recs);
            recs[at]["u"] = json!("no-such-unit");
        }),
        ("driven replay: an imp_init record out of its block", |recs| {
            let at = first_init(recs);
            recs.swap(at, at + 1);
        }),
        ("driven replay: the initial impatience (imp_init) doesn't cover", |recs| {
            let at = first_init(recs);
            let end = at + recs[at..].iter().position(|r| !ev(r, "imp_init")).unwrap();
            recs.remove(end - 1);
        }),
        ("driven replay: impatience state at the window's start", |recs| {
            let at = first_init(recs);
            let other = recs[at + 1]["u"].clone();
            recs[at]["bother"] = json!([other]);
            recs[at]["bother_slots"] = json!(1);
        }),
        ("driven replay: impatience state at the window's start", |recs| {
            let at = first_init(recs);
            recs[at]["hard_slots"] = json!(1);
        }),
        // The entries.
        ("driven replay: a uc_imp record it can't read", |recs| {
            let at = tick(recs);
            recs[at]["extra"] = json!(1);
        }),
        (
            "driven replay: the impatience controller called from a source the replay can't place",
            |recs| {
                let at = tick(recs);
                let o =
                    recs[at]["origin"].as_str().unwrap().replace("GameModes.GameMode.Tick", "X.Y");
                recs[at]["origin"] = json!(o);
            },
        ),
        ("driven replay: the impatience controller's awake units aren't the sim's", |recs| {
            let at = tick(recs);
            recs[at]["awake"].as_array_mut().unwrap().pop();
        }),
        ("driven replay: the impatience controller's awake units aren't the sim's", |recs| {
            let at = tick(recs);
            let a = recs[at]["awake"][0].clone();
            recs[at]["awake"].as_array_mut().unwrap().push(a);
        }),
        ("driven replay: a frame without the impatience controller's tick", |recs| {
            let at = tick(recs);
            let end = entry_end(recs, at);
            recs.drain(at..=end);
        }),
        (C, |recs| {
            let at = tick(recs);
            let end = entry_end(recs, at);
            recs[end]["exc"] = json!(true);
        }),
        (C, |recs| {
            let at = pos(recs, |r| {
                ev(r, "uc_imp") && r["nested"] == true && r["t"].as_i64() > Some(300)
            });
            let other = recs.iter().find(|r| ev(r, "imp_init") && r["u"] != recs[at]["u"]).unwrap()
                ["u"]
                .clone();
            recs[at]["u"] = other;
        }),
        (C, |recs| {
            let at = pos(recs, |r| {
                ev(r, "uc_imp") && r["nested"] == true && r["t"].as_i64() > Some(300)
            });
            recs[at]["origin"] =
                json!("Kingmaker.Controllers.UnitImpatienceController.UpdatePriorityTarget < X.Y");
        }),
        // The states after `UpdatePriorityTarget`.
        (C, |recs| {
            let at = state(recs, "upd");
            recs[at]["cnt"] = json!(1);
        }),
        (C, |recs| {
            let at = pos(recs, |r| {
                ev(r, "imp_s")
                    && r["at"] == "upd"
                    && r["cawe"].as_array().is_some_and(|a| !a.is_empty())
            });
            recs[at]["cawe"] = json!([]);
        }),
        (C, |recs| {
            let at = pos(recs, |r| {
                ev(r, "imp_s")
                    && r["at"] == "upd"
                    && r["bother"].as_array().is_some_and(|a| a.len() >= 2)
            });
            recs[at]["bother"].as_array_mut().unwrap().reverse();
        }),
        (C, |recs| {
            let at = pos(recs, |r| {
                ev(r, "imp_s")
                    && r["at"] == "upd"
                    && r["bother"].as_array().is_some_and(|a| !a.is_empty())
            });
            recs[at]["bother_slots"] = json!(9);
        }),
        (C, |recs| {
            let at = state(recs, "upd");
            let u = recs[at]["u"].clone();
            recs[at]["ipt"] = u;
        }),
        (C, |recs| {
            let at = state(recs, "upd");
            let u = recs[at]["u"].clone();
            recs[at]["lost"] = u;
        }),
        (C, |recs| {
            let at = state(recs, "upd");
            recs[at]["extra"] = json!(1);
        }),
        // The leaves and draws.
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "ipic") && r["t"].as_i64() > Some(300));
            recs[at]["v"] = json!(!recs[at]["v"].as_bool().unwrap());
        }),
        (C, |recs| {
            // (Flipped: a projectile on the way, or none.)
            let at = pos(recs, |r| leaf(r, "iotw"));
            let on = recs[at]["v"].as_array().unwrap().iter().any(|b| b == true);
            recs[at]["v"] = if on { json!([]) } else { json!([true]) };
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "iotw"));
            recs.remove(at);
        }),
        (C, |recs| {
            let at = state(recs, "upd");
            draw_before(recs, at);
        }),
        (C, |recs| {
            let at = pos(recs, |r| leaf(r, "ipic") && r["t"].as_i64() > Some(300));
            draw_before(recs, at);
        }),
        // `TurnController.Prepare`'s `UpdatePriorityTarget`.
        (
            "driven replay: TurnController.Prepare without exactly one UpdatePriorityTarget",
            |recs| {
                let at = prepare_upd(recs);
                let end = entry_end(recs, at);
                recs.drain(at..=end);
            },
        ),
        (C, |recs| {
            let at = prepare_upd(recs);
            let other = recs.iter().find(|r| ev(r, "imp_init") && r["u"] != recs[at]["u"]).unwrap()
                ["u"]
                .clone();
            recs[at]["u"] = other;
        }),
        // The handlers.
        (IMP, |recs| {
            let at = state(recs, "start");
            recs[at]["c"] = json!(99_999);
        }),
        (C, |recs| {
            let at = state(recs, "start");
            recs.remove(at);
        }),
        (IMP, |recs| {
            let at = state(recs, "start");
            let r = recs[at].clone();
            recs.insert(at + 1, r);
        }),
        (IMP, |recs| {
            let at = state(recs, "start");
            recs[at]["cnt"] = json!(4);
        }),
        (IMP, |recs| {
            let at = pos(recs, |r| ev(r, "imp_s") && r["at"] == "bother" && r["added"] == true);
            recs[at]["vis"] = json!(false);
        }),
        (IMP, |recs| {
            let at = pos(recs, |r| ev(r, "imp_s") && r["at"] == "bother");
            recs[at]["vis"] = json!(null);
        }),
        (IMP, |recs| {
            let at = pos(recs, |r| ev(r, "imp_s") && r["at"] == "bother");
            recs[at]["added"] = json!(!recs[at]["added"].as_bool().unwrap());
        }),
        (C, |recs| {
            let at = pos(recs, |r| ev(r, "imp_s") && r["at"] == "dmg");
            recs[at]["rule"] = json!(1);
        }),
        (IMP, |recs| {
            let at = pos(recs, |r| ev(r, "imp_s") && r["at"] == "death");
            let other = recs.iter().find(|r| ev(r, "imp_init") && r["u"] != recs[at]["u"]).unwrap()
                ["u"]
                .clone();
            recs[at]["u"] = other;
        }),
        ("driven replay: impatience: a death's records not together", |recs| {
            let at = pos(recs, |r| ev(r, "imp_s") && r["at"] == "death");
            let end = at
                + recs[at..].iter().position(|r| !(ev(r, "imp_s") && r["at"] == "death")).unwrap();
            recs.remove(end - 1);
        }),
        ("driven replay: impatience: a heal or a summon", |recs| {
            let at = state(recs, "start");
            let mut r = recs[at].clone();
            r = json!({"seq": r["seq"], "k": "obs", "t": r["t"], "ev": "imp_other", "fn": "UnitImpatienceController.OnEventDidTrigger(RuleHealDamage)"});
            recs.insert(at + 1, r);
        }),
        // Review round 1: records in a projectile's span checked; a stray draw; the whole chains.
        ("driven replay: impatience: a record out of its rule's handler", |recs| {
            let at = in_resolve(recs, "dmg");
            recs[at]["rule"] = json!(1);
        }),
        (C, |recs| {
            let at = in_resolve(recs, "dmg");
            recs[at]["t"] = json!(-999);
        }),
        ("driven replay: impatience: a record out of its rule's handler", |recs| {
            let at = in_resolve(recs, "dmg");
            let r = recs.remove(at);
            let to = at + recs[at..].iter().position(|x| x["k"] == "h_end").unwrap() + 1;
            let to = to + recs[to..].iter().position(|x| x["k"] == "h_end").unwrap() + 1;
            recs.insert(to, r);
            renumber(recs);
        }),
        ("driven replay: an impatience draw the sim's controller didn't make", |recs| {
            let at = tick(recs);
            let end = entry_end(recs, at);
            let mut d = recs.iter().find(|r| r["k"] == "draw" && r["c"] == "sim").unwrap().clone();
            let o = d.as_object_mut().unwrap();
            o.remove("site");
            o.insert("key".into(), json!("Assembly-CSharp|Kingmaker.Controllers.UnitImpatienceController|UpdatePriorityTarget(UnitEntityData)|#0@121|Range(int,int)"));
            o.insert("t".into(), recs[end]["t"].clone());
            recs.insert(end + 1, d);
            renumber(recs);
        }),
        (
            "driven replay: the impatience controller called from a source the replay can't place",
            |recs| {
                let at = tick(recs);
                let o =
                    recs[at]["origin"].as_str().unwrap().replace("Kingmaker.Runner.Update", "X.Y");
                recs[at]["origin"] = json!(o);
            },
        ),
        (
            "driven replay: TurnController.Prepare's ForceTick before its UpdatePriorityTarget, or from another chain",
            |recs| {
                let at = prepare_upd(recs);
                let o =
                    recs[at]["origin"].as_str().unwrap().replace("Kingmaker.Runner.Update", "X.Y");
                recs[at]["origin"] = json!(o);
            },
        ),
        // Review round 2: a rule handler's record only inside its rule's span; the controller's
        // own records never inside a rule; the awake units in their order.
        ("driven replay: impatience: a rule handler's record outside its rule's span", |recs| {
            let at = in_resolve(recs, "dmg");
            let r = recs.remove(at);
            let to = at
                + recs[at..]
                    .iter()
                    .position(|x| {
                        x["k"] == "rule_end"
                            && x["id"]
                                == recs
                                    .iter()
                                    .rev()
                                    .skip(recs.len() - at)
                                    .find(|y| y["k"] == "rule" && y["parent"] == 0)
                                    .unwrap()["id"]
                    })
                    .unwrap()
                + 1;
            recs.insert(to, r);
            renumber(recs);
        }),
        ("driven replay: impatience: the controller's record inside a rule", |recs| {
            let at = in_resolve(recs, "dmg");
            let mut r = recs.iter().find(|x| leaf(x, "ipic")).unwrap().clone();
            r["t"] = recs[at]["t"].clone();
            recs.insert(at, r);
            renumber(recs);
        }),
        ("driven replay: impatience: the controller's record inside a rule", |recs| {
            let at = in_resolve(recs, "dmg");
            let mut r = recs.iter().find(|x| ev(x, "uc_imp_end")).unwrap().clone();
            r["t"] = recs[at]["t"].clone();
            recs.insert(at, r);
            renumber(recs);
        }),
        ("driven replay: impatience: the controller's draw inside a rule", |recs| {
            let at = in_resolve(recs, "dmg");
            let mut d = recs.iter().find(|r| r["k"] == "draw" && r["c"] == "sim").unwrap().clone();
            let o = d.as_object_mut().unwrap();
            o.remove("site");
            o.insert("key".into(), json!("Assembly-CSharp|Kingmaker.Controllers.UnitImpatienceController|UpdatePriorityTarget(UnitEntityData)|#0@121|Range(int,int)"));
            o.insert("t".into(), recs[at]["t"].clone());
            recs.insert(at, d);
            renumber(recs);
        }),
        ("driven replay: the impatience controller's awake units aren't the sim's", |recs| {
            let at = tick(recs);
            recs[at]["awake"].as_array_mut().unwrap().swap(0, 1);
        }),
        // Review round 3: a start's record before the command's `cmd` note; a death's batch
        // together and before its `SetLifeState` goes on; `Prepare`'s update after its clear.
        ("driven replay: commands: an impatience handler's record is missing", |recs| {
            let at = pos(recs, |r| ev(r, "imp_s") && r["at"] == "start");
            let next = at + 1 + recs[at + 1..].iter().position(|r| r["k"] == "cmd").unwrap();
            let r = recs.remove(at);
            recs.insert(next, r);
            renumber(recs);
        }),
        ("driven replay: impatience: a death's records after its SetLifeState went on", |recs| {
            let at = pos(recs, |r| ev(r, "imp_s") && r["at"] == "death");
            let end = at
                + recs[at..].iter().position(|r| !(ev(r, "imp_s") && r["at"] == "death")).unwrap();
            let batch: Vec<V> = recs.drain(at..end).collect();
            let x = at + recs[at..].iter().position(|r| ev(r, "uc_x_end")).unwrap() + 1;
            for (k, r) in batch.into_iter().enumerate() {
                recs.insert(x + k, r);
            }
            renumber(recs);
        }),
        ("driven replay: impatience: a death's records not together", |recs| {
            let at = pos(recs, |r| ev(r, "imp_s") && r["at"] == "death");
            let end = at
                + recs[at..].iter().position(|r| !(ev(r, "imp_s") && r["at"] == "death")).unwrap();
            let last = recs.remove(end - 1);
            let x = end - 1 + recs[end - 1..].iter().position(|r| ev(r, "uc_x_end")).unwrap() + 1;
            recs.insert(x, last);
            renumber(recs);
        }),
        (
            "driven replay: UpdatePriorityTarget before its TurnController.Prepare's interrupts and cooldowns' clear",
            |recs| {
                let at = prepare_upd(recs);
                let end = entry_end(recs, at);
                let block: Vec<V> = recs.drain(at..=end).collect();
                let c = recs[..at].iter().rposition(|r| ev(r, "cd_clear")).unwrap();
                for (k, r) in block.into_iter().enumerate() {
                    recs.insert(c + k, r);
                }
                renumber(recs);
            },
        ),
        // Review round 4: a death's records where its notification is (after `GiveExperience`).
        ("driven replay: impatience: a death's records away from its notification", |recs| {
            let at = pos(recs, |r| ev(r, "imp_s") && r["at"] == "death");
            let end = at
                + recs[at..].iter().position(|r| !(ev(r, "imp_s") && r["at"] == "death")).unwrap();
            let batch: Vec<V> = recs.drain(at..end).collect();
            let m = recs[..at].iter().rposition(|r| r["k"] == "mut").unwrap();
            for (k, r) in batch.into_iter().enumerate() {
                recs.insert(m + k, r);
            }
            renumber(recs);
        }),
        // Review round 6: a bother record in the handler that made it.
        (IMP, |recs| {
            // The first offensive action's bother unit, moved into the next damage handler's.
            let in_handler = |recs: &[V], i: usize, class: &str| {
                recs[..i]
                    .iter()
                    .rposition(|x| x["k"] == "h")
                    .is_some_and(|h| recs[h]["class"] == class)
            };
            let off = (0..recs.len())
                .find(|&i| {
                    ev(&recs[i], "imp_s")
                        && recs[i]["at"] == "bother"
                        && in_handler(recs, i, "Kingmaker.Controllers.OffensiveActionsController")
                })
                .unwrap();
            let dmg = (off + 1..recs.len())
                .find(|&i| {
                    ev(&recs[i], "imp_s")
                        && recs[i]["at"] == "bother"
                        && in_handler(recs, i, "Kingmaker.Controllers.UnitImpatienceController")
                        && recs[i]["u"] == recs[off]["u"]
                        && recs[i]["b"] == recs[off]["b"]
                })
                .unwrap();
            let mut r = recs.remove(off);
            r["t"] = recs[dmg - 1]["t"].clone();
            recs.insert(dmg - 1, r);
            renumber(recs);
        }),
        // The observer without the targets'.
        ("driven replay: the recording declares its impatience observer without", |recs| {
            let at = pos(recs, |r| r["k"] == "begin");
            recs[at]["observers"] = json!([
                "groups",
                "cooldowns",
                "turn_ticks",
                "commands",
                "attacks",
                "approach",
                "ai",
                "engage",
                "impatience"
            ]);
        }),
    ];
    let mut missed = Vec::new();
    for (i, (want, change)) in cases.into_iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
        change(&mut changed.records);
        let _ = replay::run_with_tape(&mut w, &changed, &trace);
        if w.valid() || !w.coverage.unsupported.keys().any(|k| k.starts_with(want)) {
            missed.push(format!(
                "case {i}: {want}: {:?}",
                w.coverage.unsupported.keys().collect::<Vec<_>>()
            ));
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
}
