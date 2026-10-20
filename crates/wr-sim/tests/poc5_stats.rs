//! The stat mutations (`stat_value`, `stat_mod_add`, `stat_mod_remove`) in the driven replay's T0
//! stream against WrathOracle's captures (local, gitignored): `poc5-tb-120/tb-cast` (Enlarge Person
//! on Seelah: the buff's size and stat modifiers and the armor class's dexterity bonus replaced; the
//! helpless condition of three units going prone) and `poc5-tb-131/tb-lizard` (the helpless
//! penalties cleared again). See `docs/poc5/stats.md`. Ignored by default (needs the game's
//! packs, `cache/sim/log-strings.json`, Manual_18 and the captures; skipped when one is absent).
//! `cargo nextest run -p wr-sim --test poc5_stats --run-ignored only`.

use std::path::PathBuf;

use serde_json::Value as V;
use serde_json::json;
use wr_sim::combat::replay;

const SAVE: &str = "Manual_18_WR_FirstBattle";
const CAST: (&str, &str) = ("poc5-tb-120", "tb-cast");
const LIZARD: (&str, &str) = ("poc5-tb-131", "tb-lizard");

fn fixture_in(capture: &str, name: &str) -> Option<PathBuf> {
    // The captures live in the main checkout: look in this checkout and every directory above it
    // (a worktree finds the main one), or where `WR_CAPTURES` says.
    let rel = format!("oracle/{capture}/fixtures/{name}");
    let mut roots: Vec<PathBuf> =
        std::env::var_os("WR_CAPTURES").map(PathBuf::from).into_iter().collect();
    roots.extend(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).ancestors().map(|a| a.join("fixtures/captures")),
    );
    roots.into_iter().map(|r| r.join(&rel)).find(|d| d.join("record.trace.jsonl.gz").exists())
}

fn is_stat(r: &V) -> bool {
    r["k"] == "mut" && r["what"].as_str().is_some_and(|w| w.starts_with("stat_"))
}

/// The indices of the stat mutations of kind `what`.
fn of_kind(recs: &[V], what: &str) -> Vec<usize> {
    (0..recs.len()).filter(|&i| is_stat(&recs[i]) && recs[i]["what"] == what).collect()
}

fn renumber(recs: &mut [V]) {
    for (n, r) in recs.iter_mut().enumerate() {
        r["seq"] = json!(n);
    }
}

/// Every run of `tb-cast` and `tb-lizard` (`poc5-tb-131`) is valid with the stat mutations compared
/// and equal: the helpless penalties of the units going prone (a value change and a modifier for
/// each of six stats), cleared again (value changes only: `RemoveModifiers` reports none), and
/// Enlarge Person's size and stat modifiers with the armor class's dexterity bonus swapped for the
/// new dexterity.
#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-120 and -131 captures"]
fn the_stat_mutations_are_the_sims() {
    for (cap, runs, compared) in [
        (CAST, ["record", "replay-1", "replay-2"], 60),
        (LIZARD, ["record", "replay-1", "replay-2"], 102),
    ] {
        let Some(dir) = fixture_in(cap.0, cap.1) else {
            eprintln!("{}/{} not present: skipped", cap.0, cap.1);
            continue;
        };
        for run in runs {
            let (out, w) = replay::run_fixture(SAVE, &dir, run).expect("replay");
            let reports: Vec<_> = w.coverage.unsupported.keys().collect();
            assert!(w.valid(), "{} {run}: {reports:?}", cap.0);
            assert!(out.diff.equal, "{} {run}: {:?}", cap.0, out.diff.first_difference);
            let s = &out.stats;
            let n: usize = s.compared.values().sum();
            assert!(s.agrees() && s.equal == n && s.gaps.is_empty(), "{} {run}: {s:?}", cap.0);
            assert_eq!(n, compared, "{} {run}: {s:?}", cap.0);
        }
    }
}

/// A change made to the recording's records.
type Change = fn(&mut Vec<V>);

/// The index of the first stat mutation of kind `what`.
fn first(recs: &[V], what: &str) -> usize {
    *of_kind(recs, what).first().expect("a record")
}

const CASES: [(&str, Change); 25] = [
    // A mutation dropped.
    ("a modifier added, dropped", |recs| {
        recs.remove(first(recs, "stat_mod_add"));
    }),
    ("a value change, dropped", |recs| {
        recs.remove(first(recs, "stat_value"));
    }),
    ("a modifier removed, dropped", |recs| {
        recs.remove(first(recs, "stat_mod_remove"));
    }),
    ("the helpless group of a unit, dropped", |recs| {
        let at = first(recs, "stat_value");
        recs.drain(at..at + 12);
    }),
    // A value changed.
    ("a value change's new value", |recs| {
        let at = first(recs, "stat_value");
        recs[at]["new"] = json!(recs[at]["new"].as_i64().expect("int") + 1);
    }),
    ("a value change's old value", |recs| {
        let at = first(recs, "stat_value");
        recs[at]["old"] = json!(recs[at]["old"].as_i64().expect("int") + 1);
    }),
    ("a value change's permanent value", |recs| {
        let at = first(recs, "stat_value");
        recs[at]["permanent"] = json!(recs[at]["permanent"].as_i64().expect("int") + 1);
    }),
    ("a modifier's value", |recs| {
        let at = first(recs, "stat_mod_add");
        recs[at]["mod"]["value"] = json!(recs[at]["mod"]["value"].as_i64().expect("int") - 1);
    }),
    ("a modifier's descriptor", |recs| {
        let at = first(recs, "stat_mod_add");
        recs[at]["mod"]["descriptor"] = json!(39);
    }),
    ("a modifier's stack mode", |recs| {
        let at = first(recs, "stat_mod_add");
        recs[at]["mod"]["stack_mode"] = json!(1);
    }),
    ("a modifier's source", |recs| {
        // The buff's modifier from its component.
        let at = *of_kind(recs, "stat_mod_add")
            .iter()
            .find(|&&i| !recs[i]["mod"]["source"].is_null())
            .expect("a modifier with a source");
        recs[at]["mod"]["source"]["blueprint"] = json!("00000000000000000000000000000001");
    }),
    ("a modifier's source fact id", |recs| {
        let at = *of_kind(recs, "stat_mod_add")
            .iter()
            .find(|&&i| !recs[i]["mod"]["source"].is_null())
            .expect("a modifier with a source");
        recs[at]["mod"]["source"]["$fact"] = json!("1");
    }),
    ("a modifier's source component", |recs| {
        let at = *of_kind(recs, "stat_mod_add")
            .iter()
            .find(|&&i| !recs[i]["mod"]["source"].is_null())
            .expect("a modifier with a source");
        recs[at]["mod"]["source_component"] = json!("$AddGenericStatBonus$other");
    }),
    ("the stat after a modifier", |recs| {
        let at = first(recs, "stat_mod_add");
        recs[at]["modified"] = json!(recs[at]["modified"].as_i64().expect("int") + 1);
    }),
    // Another stat, owner or frame.
    ("a record's stat", |recs| {
        let at = first(recs, "stat_value");
        recs[at]["stat"] = json!(12);
    }),
    ("a record's owner", |recs| {
        let at = first(recs, "stat_value");
        recs[at]["owner"] = json!("5671");
    }),
    ("a record's frame", |recs| {
        let at = first(recs, "stat_mod_add");
        recs[at]["t"] = json!(recs[at]["t"].as_i64().expect("int") + 1);
    }),
    // Moved across another record (the stat mutations alone keep their order).
    ("a stat mutation moved across a fact attached", |recs| {
        let at = *of_kind(recs, "stat_mod_add")
            .iter()
            .find(|&&i| {
                (i..recs.len()).find(|&j| !is_stat(&recs[j])).is_some_and(|j| {
                    recs[j]["what"] == "fact_attach" && recs[(i + 1)..j].iter().all(is_stat)
                })
            })
            .expect("a group of stat mutations just before a fact_attach");
        let next = (at..recs.len()).find(|&j| !is_stat(&recs[j])).expect("the fact attached");
        let x = recs.remove(next);
        recs.insert(at, x);
    }),
    ("a stat mutation moved across a damage mutation", |recs| {
        let d = recs.iter().position(|r| r["k"] == "mut" && r["what"] == "damage").expect("damage");
        // The next stat mutation after it moves back before it.
        let s = (d..recs.len()).find(|&j| is_stat(&recs[j])).expect("a later stat mutation");
        let x = recs.remove(s);
        recs.insert(d, x);
    }),
    // Two exchanged.
    ("a value change and its modifier, exchanged", |recs| {
        let at = first(recs, "stat_value");
        assert!(recs[at + 1]["what"] == "stat_mod_add");
        recs.swap(at, at + 1);
    }),
    ("two units' groups, exchanged", |recs| {
        let at = first(recs, "stat_value");
        recs.swap(at, at + 2);
        recs.swap(at + 1, at + 3);
    }),
    // An extra one.
    ("a value change, repeated", |recs| {
        let at = first(recs, "stat_value");
        let x = recs[at].clone();
        recs.insert(at + 1, x);
    }),
    ("a modifier removal, added", |recs| {
        let at = first(recs, "stat_mod_remove");
        let mut x = recs[at].clone();
        x["stat"] = json!(11);
        recs.insert(at + 1, x);
    }),
    ("a temporary scope, added", |recs| {
        let at = first(recs, "stat_mod_add");
        let mut x = recs[at].clone();
        x["what"] = json!("stat_temp_scope_add");
        x["mods"] = json!([x["mod"].clone()]);
        x.as_object_mut().expect("object").remove("mod");
        recs.insert(at, x);
    }),
    // The stat of a unit the sim hasn't made (a counted gap) leaves the sim's own mutation extra.
    ("a record of a unit the sim hasn't made", |recs| {
        let at = first(recs, "stat_value");
        recs[at]["owner"] = json!("NOSUCHUNIT");
    }),
];

/// Every damaged recording is reported: the T0 stream unequal, the stat mutations differing, the
/// replay invalid.
#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-120 capture"]
fn damaged_stat_mutations_are_reported() {
    let Some(dir) = fixture_in(CAST.0, CAST.1) else {
        eprintln!("{}/{} not present: skipped", CAST.0, CAST.1);
        return;
    };
    let mut missed = Vec::new();
    for (i, (what, change)) in CASES.into_iter().enumerate() {
        let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
        let changed = trace.edited(|recs| {
            change(recs);
            renumber(recs);
        });
        let out = replay::run_with_tape(&mut w, &changed, &trace);
        let s = &out.stats;
        let ok = !out.diff.equal
            && !s.agrees()
            && !w.valid()
            && w.coverage.unsupported.keys().any(|k| k.contains("stat mutations differ"));
        if !ok {
            missed.push(format!("case {i} ({what}): equal {} {s:?}", out.diff.equal));
        }
    }
    assert!(missed.is_empty(), "{missed:#?}");
}

/// A recorded mutation of a unit the sim hasn't made is a gap, counted by its reason: the stream
/// leaves it out, so the sim's own mutation of that stat is an extra one (a difference).
#[test]
#[ignore = "needs the game's packs, cache/sim/log-strings.json, Manual_18 and the poc5-tb-120 capture"]
fn a_mutation_of_a_unit_the_sim_hasnt_made_is_a_counted_gap() {
    let Some(dir) = fixture_in(CAST.0, CAST.1) else {
        eprintln!("{}/{} not present: skipped", CAST.0, CAST.1);
        return;
    };
    let (mut w, trace) = replay::prepare_world(SAVE, &dir, "record").expect("prepare");
    let changed = trace.edited(|recs| {
        let at = first(recs, "stat_value");
        recs[at]["owner"] = json!("NOSUCHUNIT");
        let at = first(recs, "stat_mod_add");
        recs[at]["owner"] = V::Null;
    });
    let out = replay::run_with_tape(&mut w, &changed, &trace);
    let g = &out.stats.gaps;
    assert_eq!(g.get("the stat's owner is a unit the sim hasn't made"), Some(&1), "{g:?}");
    assert_eq!(g.get("the stat's owner is not a unit"), Some(&1), "{g:?}");
    assert_eq!(out.stats.extra, 2, "{:?}", out.stats);
    assert!(!out.stats.agrees() && !w.valid());
}
