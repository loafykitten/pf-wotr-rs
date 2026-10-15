//! Spellcasting and the giant fly's facts against WrathOracle's captures (local, gitignored):
//! `poc5-tb-120/tb-cast` (Camelia casts Enlarge Person on Seelah from a memorized slot, through
//! the driver's `tbcast`) and `poc5-tb-118/tb-chain` (a second fight, against the giant fly). See
//! `docs/poc5/spells.md`. Ignored by default (needs the game's packs, `cache/sim/log-strings.json`,
//! Manual_18 and the captures; skipped when a capture is absent).
//! `cargo nextest run -p wr-sim --test poc5_spells --run-ignored only`.

use std::path::PathBuf;

use serde_json::Value as V;
use serde_json::json;
use wr_sim::combat::{oracle::Trace, replay};

const SAVE: &str = "Manual_18_WR_FirstBattle";
const CAST: (&str, &str) = ("poc5-tb-120", "tb-cast");
const CHAIN: (&str, &str) = ("poc5-tb-118", "tb-chain");

fn fixture_in(capture: &str, name: &str) -> Option<PathBuf> {
    // The captures live in the main checkout: look in this checkout and every directory above it
    // (a worktree under `.claude/worktrees/` finds the main one), or where `WR_CAPTURES` says.
    let rel = format!("oracle/{capture}/fixtures/{name}");
    let mut roots: Vec<PathBuf> =
        std::env::var_os("WR_CAPTURES").map(PathBuf::from).into_iter().collect();
    roots.extend(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().map(|a| a.join("fixtures/captures")),
    );
    roots.into_iter().map(|r| r.join(&rel)).find(|d| d.join("record.trace.jsonl.gz").exists())
}

/// Every run of `tb-cast` is valid with the cast the sim's: its two roots fired at their place in
/// the command's `OnAction` (`RuleCalculateAbilityParams`, `RuleCastSpell` with its nested
/// `RuleApplySpell`), the process's five in `AbilityExecutionController`'s phase (the spell
/// resistance check with its d20, the duration's `RuleRollDice`, the buff's `RuleCanApplyBuff`, two
/// `RuleApplySpell`), the end state (the spent slot, Seelah's size and stats) the oracle's.
#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-120 capture"]
fn the_cast_is_the_sims() {
    let Some(dir) = fixture_in(CAST.0, CAST.1) else {
        eprintln!("{}/{} not present: skipped", CAST.0, CAST.1);
        return;
    };
    for run in ["record", "replay-1", "replay-2"] {
        let (out, w) = replay::run_fixture(SAVE, &dir, run).expect("replay");
        let reports: Vec<_> = w.coverage.unsupported.keys().collect();
        assert!(w.valid(), "{run}: {reports:?}");
        assert!(out.diff.equal, "{run}: {:?}", out.diff.first_difference);
        assert!(out.skipped.is_empty(), "{run}: {:?}", out.skipped);
        assert_eq!(out.by_origin.get("command (UnitUseAbility)"), Some(&2), "{run}");
        assert_eq!(out.by_origin.get("ability process"), Some(&5), "{run}");
        let units: Vec<wr_sim::world::EntityIdx> = (0..w.entities.len())
            .map(|i| wr_sim::world::EntityIdx(u32::try_from(i).unwrap_or(0)))
            .filter(|e| w.unit(*e).is_some())
            .collect();
        let sheet = wr_sim::sheet::dump(&w, &units, "end");
        let end: V = serde_json::from_slice(
            &std::fs::read(dir.join(format!("state-end-{run}.json"))).expect("end state"),
        )
        .expect("json");
        let r = wr_sim::compare::compare(&sheet, &end);
        assert!((r.match_rate - 1.0).abs() < f64::EPSILON, "{run}: {:?}", r.diffs);
        // Seelah is Large, by the buff's one size change.
        let seelah = w.by_id["558A"];
        assert_eq!(
            w.unit(seelah).and_then(|u| u.state_size),
            Some(wr_sim::stats::types::Size::Large),
            "{run}"
        );
        assert_eq!(w.unit(seelah).map(|u| u.size_change_facts.len()), Some(1), "{run}");
    }
}

/// `tb-chain`'s fly: its bite's handlers (`AddInitiatorAttackWithWeaponTrigger` from its disease
/// feature, `AttackBonusConditional` from `Airborne`) are the sim's, every run T0-equal; what is
/// left is the combat log's knowledge event of the fly revealed by fog of war (not recorded).
#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-118 capture"]
fn the_flys_handlers_are_the_sims() {
    let Some(dir) = fixture_in(CHAIN.0, CHAIN.1) else {
        eprintln!("{}/{} not present: skipped", CHAIN.0, CHAIN.1);
        return;
    };
    for run in ["record", "replay-1", "replay-2"] {
        let (out, w) = replay::run_fixture(SAVE, &dir, run).expect("replay");
        assert!(out.diff.equal, "{run}: {:?}", out.diff.first_difference);
        let other: Vec<_> =
            w.coverage.unsupported.keys().filter(|k| !k.starts_with("combat log:")).collect();
        assert!(other.is_empty(), "{run}: {other:?}");
        assert!(
            w.coverage.unsupported.keys().any(|k| k.contains("GameLogEventKnowledge")),
            "{run}: the knowledge event of the revealed fly is the one known gap"
        );
    }
}

fn root(recs: &[V], class: &str) -> usize {
    recs.iter()
        .position(|r| r["k"] == "rule" && r["depth"] == 0 && r["class"] == class)
        .expect("a root")
}

fn span_end(recs: &[V], i: usize) -> usize {
    let id = recs[i]["id"].clone();
    (i..recs.len()).find(|&j| recs[j]["k"] == "rule_end" && recs[j]["id"] == id).expect("its end")
}

fn renumber(recs: &mut [V]) {
    for (n, r) in recs.iter_mut().enumerate() {
        r["seq"] = json!(n);
    }
}

fn input(recs: &[V]) -> usize {
    recs.iter().position(|r| r["k"] == "input" && r["step"] == "tbcast").expect("the cast input")
}

/// What a damaged recording must make: a report containing the text, or the T0 stream unequal.
#[derive(Clone, Copy)]
enum Want {
    Report(&'static str),
    Unequal,
}

type Change = fn(&mut Vec<V>);

const PARAMS: &str = "Kingmaker.RuleSystem.Rules.Abilities.RuleCalculateAbilityParams";
const CAST_RULE: &str = "Kingmaker.RuleSystem.Rules.Abilities.RuleCastSpell";
const SR: &str = "Kingmaker.RuleSystem.Rules.RuleSpellResistanceCheck";
const APPLY: &str = "Kingmaker.RuleSystem.Rules.Abilities.RuleApplySpell";

/// The `n`th `RuleApplySpell` record (any depth).
fn apply(recs: &[V], n: usize) -> usize {
    (0..recs.len())
        .filter(|&i| recs[i]["k"] == "rule" && recs[i]["class"] == APPLY)
        .nth(n)
        .expect("a RuleApplySpell")
}

const CASES: [(Want, Change); 17] = [
    // The params' snapshot isn't the command's spell.
    (Want::Report("a cast's ability params"), |recs| {
        let at = root(recs, PARAMS);
        recs[at]["fields"]["m_SpellLevel"] = json!(2);
    }),
    (Want::Report("a cast's ability params"), |recs| {
        let at = root(recs, PARAMS);
        recs[at]["fields"]["Spellbook"]["blueprint"] = json!("00000000000000000000000000000001");
    }),
    // The cast's target isn't the command's.
    (Want::Report("a RuleCastSpell"), |recs| {
        let at = root(recs, CAST_RULE);
        recs[at]["fields"]["SpellTarget"]["UnitRef"]["$unit"] = json!("5671");
    }),
    // A cast without its context (the params' root gone).
    (Want::Report("a RuleCastSpell"), |recs| {
        let at = root(recs, PARAMS);
        let end = span_end(recs, at);
        recs.drain(at..=end);
        renumber(recs);
    }),
    // The cast's rule gone: its process's roots aren't the sim's.
    (Want::Report("the sim's processes didn't make"), |recs| {
        let at = root(recs, CAST_RULE);
        let end = span_end(recs, at);
        recs.drain(at..=end);
        renumber(recs);
    }),
    // The resistance check from a caller not modelled.
    (Want::Report("a RuleSpellResistanceCheck from a caller not modelled"), |recs| {
        let at = root(recs, SR);
        recs[at]["origin"] = json!(
            "Kingmaker.Controllers.SomethingElse.Tick < Kingmaker.GameModes.GameMode.Tick < Kingmaker.Game.Tick < Kingmaker.Runner.Update"
        );
    }),
    // A process root of a class the process doesn't make.
    (Want::Report("a RuleSavingThrow from a caller not modelled"), |recs| {
        let at = root(recs, SR);
        recs[at]["class"] = json!("Kingmaker.RuleSystem.Rules.RuleSavingThrow");
    }),
    // `RuleApplySpell`'s inputs (its target is `$impure`): the cast's `Before` with another
    // target, the process's first `After` as a `Before` or without its target, the second
    // `After` of another spell.
    (Want::Unequal, |recs| {
        let at = apply(recs, 0);
        recs[at]["fields"]["SpellTarget"]["UnitRef"]["$unit"] = json!("5671");
    }),
    (Want::Unequal, |recs| {
        let at = apply(recs, 1);
        recs[at]["fields"]["Place"] = json!(0);
    }),
    (Want::Unequal, |recs| {
        let at = apply(recs, 1);
        recs[at]["fields"]["SpellTarget"] = json!(null);
    }),
    (Want::Unequal, |recs| {
        let at = apply(recs, 2);
        recs[at]["fields"]["Spell"]["$ability"] = json!("00000000000000000000000000000004");
    }),
    // The spell resistance roll's draw, the params' result, the buff: the T0 stream differs.
    (Want::Unequal, |recs| {
        let at = root(recs, SR);
        let d = (at..recs.len()).find(|&i| recs[i]["k"] == "draw").expect("its d20");
        recs[d]["v"] = json!(13);
    }),
    (Want::Unequal, |recs| {
        let at = root(recs, PARAMS);
        let end = span_end(recs, at);
        let t = (at..end).find(|&i| recs[i]["k"] == "trig_end").expect("its trig_end");
        recs[t]["changes"][0][1]["DC"] = json!(15);
    }),
    (Want::Unequal, |recs| {
        let at = recs
            .iter()
            .position(|r| r["k"] == "mut" && r["what"] == "fact_attach" && r["owner"] == "558A")
            .expect("the buff's attach");
        recs[at]["fact"]["blueprint"] = json!("00000000000000000000000000000002");
    }),
    // The driver's input: another spell, another slot kind.
    (Want::Report("the driver's cast's spell"), |recs| {
        let at = input(recs);
        recs[at]["ability"] = json!("00000000000000000000000000000003");
    }),
    (Want::Report("a driver's tbcast input it can't read"), |recs| {
        let at = input(recs);
        recs[at]["slot"] = json!("spontaneous");
    }),
    // The command's end through its class: another caller.
    (Want::Report("through its class"), |recs| {
        let at = recs
            .iter()
            .position(|r| r["ev"] == "uc_end" && r["c"] == 21 && r["ctx"] == "other")
            .expect("the cast's end");
        recs[at]["origin"] = json!(
            "Kingmaker.UnitLogic.Commands.Base.UnitCommand.OnEnded < Kingmaker.UnitLogic.Commands.UnitUseAbility.OnEnded < Kingmaker.Controllers.Units.UnitCommandController.TickCommand"
        );
    }),
];

#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-120 capture"]
fn missing_or_wrong_cast_records_are_reported() {
    let Some(dir) = fixture_in(CAST.0, CAST.1) else {
        eprintln!("{}/{} not present: skipped", CAST.0, CAST.1);
        return;
    };
    let mut missed = Vec::new();
    for (i, (want, change)) in CASES.into_iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let mut changed = Trace { records: trace.records.clone(), sites: trace.sites.clone() };
        change(&mut changed.records);
        let out = replay::run_with_tape(&mut w, &changed, &trace);
        let ok = match want {
            Want::Report(s) => !w.valid() && w.coverage.unsupported.keys().any(|k| k.contains(s)),
            Want::Unequal => !out.diff.equal,
        };
        if !ok {
            missed.push(format!(
                "case {i}: equal {} {:?}",
                out.diff.equal,
                w.coverage.unsupported.keys().collect::<Vec<_>>()
            ));
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
}
