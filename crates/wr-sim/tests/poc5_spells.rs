//! Spellcasting and the giant fly's facts against WrathOracle's captures (local, gitignored):
//! `poc5-tb-120/tb-cast` (Camelia casts Enlarge Person on Seelah from a memorized slot, through
//! the driver's `tbcast`) and `poc5-tb-118/tb-chain` (a second fight, against the giant fly;
//! recorded again with the fly's reveal and the projectile pin as `poc5-tb-123`), and the AI's
//! cast actions on `poc5-tb-121/tb-lizard` (the ranged centipede's spit, scored twice). See
//! `docs/poc5/spells.md` and `docs/poc5/ai-abilities.md`. Ignored by default (needs the game's
//! packs, `cache/sim/log-strings.json`,
//! Manual_18 and the captures; skipped when a capture is absent).
//! `cargo nextest run -p wr-sim --test poc5_spells --run-ignored only`.

use std::path::PathBuf;

use serde_json::Value as V;
use serde_json::json;
use wr_sim::combat::replay;

const SAVE: &str = "Manual_18_WR_FirstBattle";
const CAST: (&str, &str) = ("poc5-tb-120", "tb-cast");
const CHAIN: (&str, &str) = ("poc5-tb-118", "tb-chain");
const CHAIN_REVEALED: (&str, &str) = ("poc5-tb-123", "tb-chain");
const LIZARD: (&str, &str) = ("poc5-tb-121", "tb-lizard");

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
        assert!(r.units_missing_ours.is_empty() && r.units_missing_oracle.is_empty(), "{run}");
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

/// `tb-chain` recorded with the inspection controller's reveals (`inspect_reveal`): every run
/// valid, T0-equal, its end state the oracle's. The fly is revealed by fog of war and queued for
/// its knowledge check where the game did; its bite hits here, so the disease trigger's actions
/// run (their difficulty condition false at the save's setting: no saving throw), and `Airborne`'s
/// `DamageBonusConditional` is the sim's. Needs `cache/sim/rules-data.json` with the presets'
/// asset indices (`wr-rules-data`).
#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json and rules-data.json, Manual_18 and the poc5-tb-123 capture"]
fn the_revealed_flys_fight_is_valid() {
    let Some(dir) = fixture_in(CHAIN_REVEALED.0, CHAIN_REVEALED.1) else {
        eprintln!("{}/{} not present: skipped", CHAIN_REVEALED.0, CHAIN_REVEALED.1);
        return;
    };
    for run in ["record", "replay-1", "replay-2"] {
        let (out, w) = replay::run_fixture(SAVE, &dir, run).expect("replay");
        let reports: Vec<_> = w.coverage.unsupported.keys().collect();
        assert!(w.valid(), "{run}: {reports:?}");
        assert!(out.diff.equal, "{run}: {:?}", out.diff.first_difference);
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
        assert!(r.units_missing_ours.is_empty() && r.units_missing_oracle.is_empty(), "{run}");
        // The bite's disease trigger ran its actions: their difficulty condition was evaluated.
        assert!(
            w.coverage.executed.contains_key("condition ContextConditionDifficultyHigherThan"),
            "{run}: {:?}",
            w.coverage.executed.keys().filter(|k| k.starts_with("condition")).collect::<Vec<_>>()
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

const CASES: [(Want, Change); 20] = [
    // The params' snapshot isn't the command's spell.
    (Want::Report("a cast's ability params"), |recs| {
        let at = root(recs, PARAMS);
        recs[at]["fields"]["m_SpellLevel"] = json!(2);
    }),
    (Want::Report("a cast's ability params"), |recs| {
        let at = root(recs, PARAMS);
        recs[at]["fields"]["Spellbook"]["blueprint"] = json!("00000000000000000000000000000001");
    }),
    (Want::Report("a cast's ability params"), |recs| {
        let at = root(recs, PARAMS);
        recs[at]["fields"]["AbilityData"]["caster"]["$unit"] = json!("558A");
    }),
    // The cast's target, spell or context isn't the command's.
    (Want::Report("a RuleCastSpell"), |recs| {
        let at = root(recs, CAST_RULE);
        recs[at]["fields"]["SpellTarget"]["UnitRef"]["$unit"] = json!("5671");
    }),
    (Want::Report("a RuleCastSpell"), |recs| {
        let at = root(recs, CAST_RULE);
        recs[at]["fields"]["Spell"]["caster"]["$unit"] = json!("558A");
    }),
    (Want::Report("a RuleCastSpell"), |recs| {
        let at = root(recs, CAST_RULE);
        recs[at]["fields"]["Context"]["$context"] = json!("00000000000000000000000000000005");
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
        let changed = trace.edited(change);
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

/// The projectile pin's launch records (`proj_launch`, `poc5-tb-123`'s one ranged attack) are
/// matched with the sim's launches: another attack result (even another hit), an unknown one,
/// another blueprint or a missing launch is reported.
#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json and rules-data.json, Manual_18 and the poc5-tb-123 capture"]
fn wrong_projectile_launches_are_reported() {
    let Some(dir) = fixture_in(CHAIN_REVEALED.0, CHAIN_REVEALED.1) else {
        eprintln!("{}/{} not present: skipped", CHAIN_REVEALED.0, CHAIN_REVEALED.1);
        return;
    };
    /// The first projectile launch record.
    fn launch(recs: &[V]) -> usize {
        recs.iter().position(|r| r["ev"] == "proj_launch").expect("the ranged attack's launch")
    }
    let cases: [Change; 4] = [
        |recs| {
            let at = launch(recs);
            recs[at]["res"] = json!("CriticalHit");
        },
        |recs| {
            let at = launch(recs);
            recs[at]["res"] = json!("NotAResult");
        },
        |recs| {
            let at = launch(recs);
            recs[at]["bp"] = json!("00000000000000000000000000000006");
        },
        |recs| {
            let at = launch(recs);
            recs.remove(at);
            renumber(recs);
        },
    ];
    let mut missed = Vec::new();
    for (i, change) in cases.into_iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let changed = trace.edited(change);
        let _ = replay::run_with_tape(&mut w, &changed, &trace);
        if !w.coverage.unsupported.keys().any(|k| k.contains("projectiles launched differ")) {
            missed
                .push(format!("case {i}: {:?}", w.coverage.unsupported.keys().collect::<Vec<_>>()));
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
}

/// Observation records inside a driven root rule's span are checked or reported, never skipped
/// (`docs/poc5/tb-replay.md`, "Records inside root rule spans"): a combat log event of an
/// initiative roll damaged or lost, and a record of each family (or of none the replay knows)
/// added to the span of a root the sim fires without it.
#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-123 capture"]
fn records_inside_driven_root_spans_are_checked_or_reported() {
    let Some(dir) = fixture_in(CHAIN_REVEALED.0, CHAIN_REVEALED.1) else {
        eprintln!("{}/{} not present: skipped", CHAIN_REVEALED.0, CHAIN_REVEALED.1);
        return;
    };
    /// The first root `RuleInitiativeRoll` (driven by its phase): its record's index.
    fn initiative(recs: &[V]) -> usize {
        recs.iter()
            .position(|r| {
                r["k"] == "rule"
                    && r["depth"] == 0
                    && r["class"].as_str().is_some_and(|c| c.ends_with("RuleInitiativeRoll"))
            })
            .expect("an initiative roll")
    }
    /// The first record of kind `k` inside a phase-driven root's span (an initiative roll's or a
    /// resolve's): its index.
    fn handled(recs: &[V], k: &str) -> usize {
        let mut open = false;
        for (i, r) in recs.iter().enumerate() {
            if r["k"] == "rule" && r["depth"] == 0 {
                open = r["class"].as_str().is_some_and(|c| {
                    c.ends_with("RuleInitiativeRoll") || c.ends_with("RuleAttackWithWeaponResolve")
                });
            } else if open && r["k"] == k {
                return i;
            }
        }
        panic!("no {k} in a phase-driven root")
    }
    /// The first record satisfying `f` inside a depth-0 root of the class (`suffix`) that has one.
    fn in_span(recs: &[V], suffix: &str, f: impl Fn(&V) -> bool) -> usize {
        let mut open = false;
        for (i, r) in recs.iter().enumerate() {
            if r["k"] == "rule" && r["depth"] == 0 {
                open = r["class"].as_str().is_some_and(|c| c.ends_with(suffix));
            } else if open && f(r) {
                return i;
            }
        }
        panic!("no such record in a {suffix} root")
    }
    /// The first `clog_q` of that root (its rule event).
    fn event(recs: &[V]) -> usize {
        let root = initiative(recs);
        let id = recs[root]["id"].clone();
        recs.iter()
            .position(|r| r["ev"] == "clog_q" && r["rule"] == id)
            .expect("the initiative's event")
    }
    /// A copy of the first record of observation `ev` (it is in a command's span), added right
    /// after the initiative root's record, in its frame.
    fn add(recs: &mut Vec<V>, ev: &str) {
        let mut x =
            recs.iter().find(|r| r["k"] == "obs" && r["ev"] == ev).expect("a record").clone();
        let root = initiative(recs);
        x["t"] = recs[root]["t"].clone();
        recs.insert(root + 1, x);
        renumber(recs);
    }
    /// The events after id `from` renumbered by `delta` in every record that names one (the events,
    /// the ticks' queues, the messages' sources), the numbering staying valid: an event removed
    /// (`delta` -1) leaves no gap, one added (+1) no duplicate.
    fn shift_ids(recs: &mut [V], from: i64, delta: i64) {
        let shift = |v: &mut V| {
            if let Some(i) = v.as_i64().filter(|i| *i > from) {
                *v = json!(i + delta);
            }
        };
        for r in recs.iter_mut() {
            match r["ev"].as_str() {
                Some("clog_q") => shift(&mut r["id"]),
                Some("clog_msg") => shift(&mut r["src"]),
                Some("clog_tick") => {
                    if let Some(q) = r["q"].as_array_mut() {
                        for e in q {
                            shift(&mut e[0]);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let cases: [(&str, Change); 27] = [
        // The initiative's rule event, queued by the combat log's did hook: compared with the
        // sim's queue, the event numbering valid (a malformed record is not accepted as the report).
        ("events queued, the sim queued", |recs| {
            let at = event(recs);
            let id = recs[at]["id"].as_i64().expect("an id");
            // (A tick's queue holding it loses the entry.)
            for r in recs.iter_mut().filter(|r| r["ev"] == "clog_tick") {
                if let Some(q) = r["q"].as_array_mut() {
                    q.retain(|e| e[0].as_i64() != Some(id));
                }
            }
            recs.remove(at);
            shift_ids(recs, id, -1);
            renumber(recs);
        }),
        ("combat log: queued event #", |recs| {
            let at = event(recs);
            recs[at]["rule"] = json!(recs[at]["rule"].as_i64().unwrap_or(0) + 1);
        }),
        ("combat log: queued event #", |recs| {
            let at = event(recs);
            recs[at]["en"] = json!(false);
        }),
        ("events queued, the sim queued", |recs| {
            let at = event(recs);
            let id = recs[at]["id"].as_i64().expect("an id");
            shift_ids(recs, id, 1);
            let mut x = recs[at].clone();
            x["id"] = json!(id + 1);
            recs.insert(at + 1, x);
            renumber(recs);
        }),
        // The event of a recording that doesn't declare the observer.
        ("driven replay: clog_q inside a root rule's span", |recs| {
            if let Some(o) = recs[0]["observers"].as_array_mut() {
                o.retain(|x| x != "combat_log");
            }
            // (Only the rules' events inside root spans stay.)
            let mut open = None;
            recs.retain(|r| {
                if r["k"] == "rule" && r["depth"] == 0 {
                    open = Some(r["id"].clone());
                } else if r["k"] == "rule_end" && open.as_ref() == Some(&r["id"]) {
                    open = None;
                }
                let log =
                    r["k"] == "obs" && r["ev"].as_str().is_some_and(|e| e.starts_with("clog_"));
                !log || (open.is_some() && r["ev"] == "clog_q")
            });
            renumber(recs);
        }),
        // Another family's record in a span of a root that has none of its handlers.
        ("driven replay: impatience: a record out of its rule's handler", |recs| {
            add(recs, "imp_s")
        }),
        ("driven replay: memory: an AddToMemory the sim didn't make", |recs| add(recs, "mem_q")),
        ("driven replay: eng_prov inside a root rule's span", |recs| add(recs, "eng_prov")),
        ("driven replay: eng_prov inside a root rule's span", |recs| {
            add(recs, "eng_prov");
            let root = initiative(recs);
            recs[root + 1]["u"] = json!("none");
        }),
        ("driven replay: proj_launch inside a root rule's span", |recs| add(recs, "proj_launch")),
        // A record of no family the replay knows, and of no kind it takes.
        ("driven replay: an unknown_family observation inside a root rule's span", |recs| {
            let root = initiative(recs);
            let t = recs[root]["t"].clone();
            recs.insert(root + 1, json!({"seq": 0, "k": "obs", "t": t, "ev": "unknown_family"}));
            renumber(recs);
        }),
        ("driven replay: a cmd record inside a root rule's span", |recs| {
            let root = initiative(recs);
            let t = recs[root]["t"].clone();
            recs.insert(root + 1, json!({"seq": 0, "k": "cmd", "t": t, "ev": "start"}));
            renumber(recs);
        }),
        // A record of another frame than its root's (the span is one frame's).
        ("driven replay: a rule_end record inside a root rule's span", |recs| {
            let root = initiative(recs);
            let id = recs[root]["id"].clone();
            let end = (root..recs.len())
                .find(|&i| recs[i]["k"] == "rule_end" && recs[i]["id"] == id)
                .expect("its end");
            recs[end]["t"] = json!(recs[end]["t"].as_i64().unwrap_or(0) + 1);
        }),
        // The structure of the span (`life.rs`, shared with the commands' `span_ok`): a handler
        // closed with another id, a `mod_after` handler after a `did` one, a bonus to a rule that
        // was never allocated, and one to a rule already closed (T0 emits a bonus at its rule's end).
        ("driven replay: a h_end record inside a root rule's span", |recs| {
            let at = handled(recs, "h_end");
            recs[at]["id"] = json!(recs[at]["id"].as_i64().unwrap_or(0) + 1000);
        }),
        ("driven replay: a h record inside a root rule's span", |recs| {
            // Just before the initiative roll's own `did`: a `did` handler of it, then a
            // `mod_after` one (ids above every other).
            let root = initiative(recs);
            let id = recs[root]["id"].clone();
            let did = (root..recs.len())
                .find(|&i| recs[i]["k"] == "did" && recs[i]["id"] == id)
                .expect("its did");
            let top = recs.iter().filter_map(|r| r["id"].as_i64()).max().unwrap_or(0) + 1_000;
            let t = recs[root]["t"].clone();
            let pair = |n: i64, phase: &str| {
                [
                    json!({"seq": 0, "k": "h", "t": t, "id": n, "rule": id, "phase": phase,
                        "class": "Kingmaker.Controllers.UnitImpatienceController"}),
                    json!({"seq": 0, "k": "h_end", "t": t, "id": n, "rule": id, "changes": []}),
                ]
            };
            let new: Vec<V> = [pair(top, "did"), pair(top + 1, "mod_after")].concat();
            recs.splice(did..did, new);
            renumber(recs);
        }),
        ("driven replay: a rule_bonus mutation inside a root rule's span", |recs| {
            let root = initiative(recs);
            let t = recs[root]["t"].clone();
            recs.insert(
                root + 1,
                json!({"seq": 0, "k": "mut", "t": t, "what": "rule_bonus", "target_rule": 1_000_000_000}),
            );
            renumber(recs);
        }),
        ("driven replay: a rule_bonus mutation inside a root rule's span", |recs| {
            // To a rule that is already closed: the root before the last initiative roll.
            let root = recs
                .iter()
                .rposition(|r| {
                    r["k"] == "rule"
                        && r["depth"] == 0
                        && r["class"].as_str().is_some_and(|c| c.ends_with("RuleInitiativeRoll"))
                })
                .expect("an initiative roll");
            let before = recs[..root]
                .iter()
                .rev()
                .find(|r| r["k"] == "rule" && r["depth"] == 0)
                .expect("an earlier root")["id"]
                .clone();
            let t = recs[root]["t"].clone();
            recs.insert(
                root + 1,
                json!({"seq": 0, "k": "mut", "t": t, "what": "rule_bonus", "target_rule": before}),
            );
            renumber(recs);
        }),
        // A draw or a damage of no rule the span has (T0 projects both whatever their rule), a root
        // whose own stack fields are not a root's.
        ("driven replay: a draw record inside a root rule's span", |recs| {
            let at = in_span(recs, "RuleInitiativeRoll", |r| r["k"] == "draw");
            recs[at]["rule"] = json!(999_999_999);
        }),
        ("driven replay: a draw record inside a root rule's span", |recs| {
            let at = in_span(recs, "RuleInitiativeRoll", |r| r["k"] == "draw");
            recs[at]["h"] = json!(5);
        }),
        ("driven replay: a damage mutation inside a root rule's span", |recs| {
            let at = in_span(recs, "RuleAttackWithWeaponResolve", |r| {
                r["k"] == "mut" && r["what"] == "damage"
            });
            recs[at]["rule"] = json!(999_999_999);
        }),
        ("rule record inside a root rule's span: a root whose stack fields", |recs| {
            let root = initiative(recs);
            recs[root]["parent"] = json!(999_999_999);
        }),
        ("rule record inside a root rule's span: a root whose stack fields", |recs| {
            let root = initiative(recs);
            recs[root]["handler"] = json!(999_999_999);
        }),
        // A mutation the T0 projection drops (stat values, modifiers) compares nowhere.
        ("driven replay: a stat_value mutation inside a root rule's span", |recs| {
            let root = initiative(recs);
            let t = recs[root]["t"].clone();
            recs.insert(root + 1, json!({"seq": 0, "k": "mut", "t": t, "what": "stat_value"}));
            renumber(recs);
        }),
        // A rule's `did` before its `trig_end` (and its rewind) is out of its place.
        ("driven replay: a did record inside a root rule's span", |recs| {
            let root = initiative(recs);
            let id = recs[root]["id"].clone();
            let at = (root..recs.len())
                .find(|&i| recs[i]["k"] == "trig_end" && recs[i]["id"] == id)
                .expect("its trig_end");
            let mut did = recs[at].clone();
            did["k"] = json!("did");
            recs.insert(at, did);
            renumber(recs);
        }),
        // The oracle writes no raw `result` or `bonuses` records, and an environment is only the
        // one the root's code reads (the initiative roll reads none).
        ("driven replay: a result record inside a root rule's span", |recs| {
            let root = initiative(recs);
            let t = recs[root]["t"].clone();
            recs.insert(root + 1, json!({"seq": 0, "k": "result", "t": t, "id": 1}));
            renumber(recs);
        }),
        ("driven replay: a bonuses record inside a root rule's span", |recs| {
            let root = initiative(recs);
            let t = recs[root]["t"].clone();
            recs.insert(root + 1, json!({"seq": 0, "k": "bonuses", "t": t, "id": 1}));
            renumber(recs);
        }),
        ("driven replay: an env record inside a root rule's span", |recs| {
            let mut x = recs.iter().find(|r| r["k"] == "env").expect("an environment").clone();
            let root = initiative(recs);
            x["t"] = recs[root]["t"].clone();
            recs.insert(root + 1, x);
            renumber(recs);
        }),
    ];
    let mut missed = Vec::new();
    for (i, (want, change)) in cases.into_iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let changed = trace.edited(change);
        let _ = replay::run_with_tape(&mut w, &changed, &trace);
        let reports: Vec<_> = w.coverage.unsupported.keys().collect();
        let numbering = reports.iter().any(|k| k.contains("malformed or unknown log"));
        if w.valid() || numbering || !reports.iter().any(|k| k.contains(want)) {
            missed.push(format!("case {i}: {want}: {reports:?}"));
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
    // The oracle's own `invalid` note is accepted in every kind of root, counted: the first
    // initiative roll (phase-driven), the presentation roots with a call site, and the attack
    // counts and attacks of the commands (their spans never enter `Driver::root`).
    let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
    let mut notes = 0;
    let changed = trace.edited(|recs| {
        let mut roots = vec![initiative(recs)];
        for (i, r) in recs.iter().enumerate() {
            let class = r["class"].as_str().unwrap_or("");
            if r["k"] == "rule"
                && r["depth"] == 0
                && [
                    "RuleCheckTargetFlatFooted",
                    "RuleCalculateAttacksCount",
                    "RuleAttackWithWeapon",
                ]
                .iter()
                .any(|c| class.ends_with(c) || class.ends_with(&format!("{c}Limbs")))
            {
                roots.push(i);
            }
        }
        notes = roots.len();
        // (From the last, so the earlier indices hold.)
        for &root in roots.iter().rev() {
            let t = recs[root]["t"].clone();
            recs.insert(
                root + 1,
                json!({"seq": 0, "k": "obs", "t": t, "ev": "invalid", "why": "x"}),
            );
        }
        renumber(recs);
    });
    let out = replay::run_with_tape(&mut w, &changed, &trace);
    assert!(w.valid(), "{:?}", w.coverage.unsupported.keys().collect::<Vec<_>>());
    assert_eq!((out.spans.oracle_invalid, out.spans.reported), (notes, 0));
    assert!(notes > 10, "{notes}");
}

/// The ranged centipede's spit (`GiantCentipedeSpitAbilityAiAction`, a `BlueprintAiCastSpell`).
const SPIT: &str = "14db26a8a9b3be448860e12a07559f46";

/// Every run of `tb-lizard` is T0-equal with the centipede's cast action scored by the sim: its
/// range the ability's, `ScoreActor` 2 then 0 (`ThreatedByConsideration` on the engagement
/// model), `ScoreTarget` 0 (`DistanceConsideration`), no impatience reads; the attack chosen both
/// times. The one kind left is the movement agent's: ground rays of no unit outside its entries
/// (`docs/poc5/ai-abilities.md`, "Results").
#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-121 capture"]
fn the_centipedes_cast_action_is_scored() {
    let Some(dir) = fixture_in(LIZARD.0, LIZARD.1) else {
        eprintln!("{}/{} not present: skipped", LIZARD.0, LIZARD.1);
        return;
    };
    for run in ["record", "replay-1", "replay-2"] {
        let (out, w) = replay::run_fixture(SAVE, &dir, run).expect("replay");
        let reports: Vec<_> = w.coverage.unsupported.keys().collect();
        let rays = "driven replay: agent: a record outside the sim's agent entries (ag_phys fn=\"ray\" ctx=\"other\")";
        // The cave walls' mesh queries are computed (stream S, `docs/poc5/physics.md`).
        assert!(reports.iter().all(|k| k.as_str() == rays), "{run}: {reports:?}");
        assert!(out.diff.equal, "{run}: {:?}", out.diff.first_difference);
        // The centipede's brain lists the spit first (base score 2), then its attack.
        let centipede = w.by_id["5031"];
        let available = w.unit(centipede).and_then(|u| u.brain.clone()).map(|b| b.available);
        assert_eq!(
            available.map(|a| a.iter().map(|g| g.map(|g| g.to_string())).collect::<Vec<_>>()),
            Some(vec![Some(SPIT.to_owned()), Some("866ffa6c34000cd4a86fb1671f86c7d8".to_owned())]),
            "{run}"
        );
    }
}

/// The cast action's scoring is computed, so its recorded reads and checks are required: another
/// range, a score the sim doesn't make, a missing range read, or a read the sim doesn't make
/// (impatience: the spit isn't affected) is reported.
#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-121 capture"]
fn wrong_cast_action_records_are_reported() {
    let Some(dir) = fixture_in(LIZARD.0, LIZARD.1) else {
        eprintln!("{}/{} not present: skipped", LIZARD.0, LIZARD.1);
        return;
    };
    fn spit_range(recs: &[V], n: usize) -> usize {
        (0..recs.len())
            .filter(|&i| recs[i]["ev"] == "uc_v" && recs[i]["f"] == "range" && recs[i]["a"] == SPIT)
            .nth(n)
            .expect("the spit's range read")
    }
    fn spit_score(recs: &[V], f: &str, n: usize) -> usize {
        (0..recs.len())
            .filter(|&i| recs[i]["ev"] == "ai_sc" && recs[i]["fn"] == f && recs[i]["a"] == SPIT)
            .nth(n)
            .expect("the spit's score")
    }
    let cases: [(&str, Change); 6] = [
        // The ability's medium range is 40 ft: 12.192 m.
        ("the cast action 14db26a8a9b3be448860e12a07559f46's range", |recs| {
            let at = spit_range(recs, 0);
            recs[at]["v"] = json!("12.0");
        }),
        // Not engaged: `ThreatedByConsideration` 1, the actor's score the base score 2.
        ("ScoreActor of None: the sim scores 2", |recs| {
            let at = spit_score(recs, "ScoreActor", 0);
            recs[at]["s"] = json!("1");
        }),
        // Engaged by the PC: 0.
        ("ScoreActor of None: the sim scores 0", |recs| {
            let at = spit_score(recs, "ScoreActor", 1);
            recs[at]["s"] = json!("2");
        }),
        // 14.67 m away: `DistanceConsideration` 0 beyond 8 m.
        ("ScoreTarget of Some(", |recs| {
            let at = spit_score(recs, "ScoreTarget", 0);
            recs[at]["s"] = json!("2");
        }),
        // The range read missing.
        ("driven replay: commands:", |recs| {
            let at = spit_range(recs, 1);
            recs.remove(at);
            renumber(recs);
        }),
        // An impatience read after the spit's target: the sim reads none for it.
        ("driven replay: commands:", |recs| {
            let at = spit_score(recs, "ScoreTarget", 0) + 1;
            assert_eq!(recs[at]["f"], "ptf");
            let mut r = recs[at].clone();
            r["f"] = json!("ipt");
            r["v"] = V::Null;
            r.as_object_mut().expect("a record").remove("u");
            recs.insert(at + 1, r);
            renumber(recs);
        }),
    ];
    let mut missed = Vec::new();
    for (i, (want, change)) in cases.into_iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let changed = trace.edited(change);
        let _ = replay::run_with_tape(&mut w, &changed, &trace);
        if w.valid() || !w.coverage.unsupported.keys().any(|k| k.contains(want)) {
            missed
                .push(format!("case {i}: {:?}", w.coverage.unsupported.keys().collect::<Vec<_>>()));
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
}
