//! POC-4 against WrathOracle recordings (local captures, gitignored): the certification
//! fixture `dialog-checks` (Seelah/Anevia meeting, Athletics check) recorded, replayed,
//! and replayed with the check's d20 perturbed (success branch). Ignored by default: it
//! needs the game's blueprint pack, Manual_13 and `fixtures/captures/oracle/cert-24`.
//! `cargo nextest run -p wr-sim --run-ignored only`.

use std::path::PathBuf;

/// The etude ToyBox (active in the oracle build) keeps playing: its any-gender-romance
/// patch makes `PcFemale` true for this etude (docs/poc4/dialogue.md).
const TOYBOX_ETUDE: &str = "etude d9baf40d38ceaf248bd5306f0e344bdb";

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn run(name: &str, oracle: &str, run: &str) -> wr_sim::runner::FixtureRun {
    let spec = root().join("crates/wr-sim/fixtures/poc4.json");
    let dir = root().join("fixtures/captures/oracle").join(oracle);
    wr_sim::runner::run_fixture(&wr_sim::runner::FixtureOptions {
        ability_params_empty: None,
        spec: &spec,
        fixture: name,
        oracle: Some(&dir),
        run,
        oracle_inputs: true,
    })
    .expect("fixture")
}

/// The run compared the dialogue's text: the recording declares it, and the sim wrote a history
/// line for every answered cue and a text record for every transition, none of them null.
fn assert_text_compared(f: &wr_sim::runner::FixtureRun, what: &str) {
    assert!(f.world.dialog.text_records, "{what}: no text records");
    let recs = &f.world.events.records;
    let texts = recs.iter().filter(|e| e.k == "dlg_text").count();
    let lines = recs.iter().filter(|e| e.k == "dlg_history").count();
    assert!(texts > 0 && lines > 0, "{what}: {texts} texts, {lines} history lines");
    for e in recs.iter().filter(|e| e.k == "dlg_history" || e.k == "dlg_text") {
        let null = e.fields.get("line").is_some_and(serde_json::Value::is_null)
            || e.fields.get("cue_text").is_some_and(serde_json::Value::is_null)
            || e.fields.get("speaker_name").is_some_and(serde_json::Value::is_null);
        assert!(!null, "{what}: {} {:?}", e.k, e.fields);
    }
}

/// The character sheets against the oracle's end state: a unit matches exactly unless
/// unsupported behaviour was reported for it while it was built (`Coverage::unsupported_by_unit`)
/// or, for a unit the run created, in the run: no difference is left without a reported cause.
fn check_units(f: &wr_sim::runner::FixtureRun, what: &str) {
    let r = f.units_diff.as_ref().expect("units");
    for (u, (equal, total)) in &r.per_unit {
        if equal == total {
            continue;
        }
        let id = u.trim_matches('"');
        // A unit the run created (an id drawn after the load): its construction is
        // reported for the whole run, not for the unit.
        let created = u64::from_str_radix(id, 16).is_ok_and(|n| n >= f.first_runtime_id)
            && f.world
                .coverage
                .unsupported
                .keys()
                .any(|k| k.starts_with("EntityCreationController."));
        let reported = f.world.coverage.unsupported_by_unit.contains_key(id) || created;
        assert!(reported, "{what}: unit {id} {equal}/{total} differs with no reported cause");
    }
}

#[test]
#[ignore = "needs the game's blueprint pack, Manual_13 and the cert-24 capture"]
fn dialog_checks_matches_the_oracle_on_both_branches() {
    for r in ["record", "replay-1", "replay-2-perturbed"] {
        let f = run("dialog-checks", "cert-24/fixtures/dialog-checks", r);
        assert!(f.outcome.failure.is_none(), "{r}: {:?}", f.outcome.failure);
        // Preconditions: the start state equals the oracle's (but for ToyBox's etude), and
        // loading reports exactly the documented gaps.
        let start = f.start_diff.as_ref().expect("start");
        for (k, c) in &start.categories {
            let unexplained: Vec<_> =
                c.diffs.iter().filter(|d| !d.starts_with(TOYBOX_ETUDE)).collect();
            assert!(
                unexplained.is_empty() && c.total - c.equal == c.diffs.len().min(c.total),
                "{r}: start {k} {unexplained:?}"
            );
        }
        // Behaviour the load doesn't port, reported (fail closed). Units built from
        // placeholders run into the level-up and spellbook machinery of the original: spell
        // books, ability resources and ability parameters aren't modelled (the sheet doesn't
        // read them), and `check_units` below holds every difference of a built unit to a
        // reported cause.
        let load_known = [
            "AddAbilityResources.OnActivate",
            "AddAbilityResources: IUnitReapplyFeaturesOnLevelUpHandler",
            "AddClassLevels.PrepareSpellbook: spellbook slots and memorization",
            "AddFacts: difficulty requirements",
            "AddSpecialSpellList.OnActivate",
            "AddSpecialSpellListForArchetype.OnActivate",
            "ApplySpellbook: class spellbook",
            "BecomeUnit: SummonPools.Register (pool membership)",
            "BlueprintFeatureSelection.Items: DLC restriction",
            "LearnSpells.OnActivate",
            "MechanicsContext.Recalculate: ContextCalculateAbilityParams",
            "MonkNoArmorAndMonkWeaponFeatureUnlock: IUnitBuffHandler",
            "Replenish: resources or spellbooks",
            "UnitPlaceholderEntityData.BecomeUnit: saved m_Roaming",
            "AddAdditionalLimb.OnTurnOn",
            "EmptyHandWeaponOverride.OnTurnOn",
            "ItemEntity.ApplyEnchantments: enchantment not on the blueprint",
            "SpellImmunityToSpellDescriptor.OnTurnOn",
            "Spellbook.PostLoad",
            "area scene state not hydrated: Prologue_Caves_1_TerendelevScales_Mechanics",
        ];
        let load_unexpected: Vec<_> = f
            .load_unsupported
            .keys()
            .filter(|k| {
                !load_known.contains(&k.as_str())
                    && !k.starts_with("area scene entity not hydrated: ")
            })
            .collect();
        assert!(load_unexpected.is_empty(), "{r}: {load_unexpected:?}");
        let td = f.trace_diff.as_ref().expect("trace");
        assert!(td.equal(), "{r}: {td:?}");
        assert!(f.world.rng.failures().is_empty(), "{r}");
        // The zone step stands in for the T1 walk into the trigger (the zone's own state and
        // settings come from the scene cache, `scripts/scene-cache.sh`); the run reports
        // nothing unsupported, and the load only its documented gaps.
        assert!(f.run_unsupported().is_empty(), "{r}: {:?}", f.run_unsupported());
        assert_text_compared(&f, r);
        check_units(&f, r);
        let nd = f.end_diff.as_ref().expect("narrative");
        for (k, c) in &nd.categories {
            // One etude stays active in the game although its activation condition is false
            // (open; docs/poc4/dialogue.md): exactly that one, nothing else.
            let known = [format!("{TOYBOX_ETUDE} active: ours false, oracle true")];
            let unexplained: Vec<_> =
                c.diffs.iter().filter(|d| !known.iter().any(|x| x == *d)).collect();
            assert!(unexplained.is_empty(), "{r}: {k} {unexplained:?}");
            assert_eq!(c.total - c.equal, c.diffs.len().min(c.total - c.equal), "{r}: {k}");
        }
    }
}

/// The POC-4 recordings (Seelah/Anevia paths A, B, C; Horgus with and without the check):
/// every run's dialogue records and rule draws equal the oracle's, and the character sheets
/// match. The ordered T0 stream also matches up to the first element the sim reports
/// unsupported (the recruited companion's unit, the closing cutscene), checked in the
/// report (`docs/poc4/dialogue.md`).
#[test]
#[ignore = "needs the game's blueprint pack, the fixture saves and the poc4 captures"]
fn poc4_recordings_match_the_dialogue_and_draws() {
    let runs: &[(&str, &str, &[&str])] = &[
        (
            "seelah-a",
            "poc4-seelah-7/fixtures/seelah-a",
            &["record", "replay-1", "replay-2-perturbed"],
        ),
        ("seelah-b", "poc4-seelah-7/fixtures/seelah-b", &["record", "replay-2-perturbed"]),
        ("seelah-c", "poc4-seelah-7/fixtures/seelah-c", &["record", "replay-2-perturbed"]),
        ("horgus-check", "poc4-horgus-11/fixtures/horgus-check", &["record", "replay-2-perturbed"]),
        ("horgus-alt", "poc4-horgus-11/fixtures/horgus-alt", &["record"]),
    ];
    for (name, dir, rs) in runs {
        for r in *rs {
            let spec = root().join("crates/wr-sim/fixtures/poc4.json");
            let d = root().join("fixtures/captures/oracle").join(dir);
            let f = wr_sim::runner::run_fixture(&wr_sim::runner::FixtureOptions {
                ability_params_empty: None,
                spec: &spec,
                fixture: name,
                oracle: Some(&d),
                run: r,
                oracle_inputs: false,
            })
            .expect("fixture");
            assert!(f.outcome.failure.is_none(), "{name} {r}: {:?}", f.outcome.failure);
            let td = f.trace_diff.as_ref().expect("trace");
            assert!(td.dialog.equal, "{name} {r}: {:?}", td.dialog.first_difference);
            assert!(td.draws.equal, "{name} {r}: {:?}", td.draws.first_difference);
            assert!(f.world.rng.failures().is_empty(), "{name} {r}");
            check_units(&f, &format!("{name} {r}"));
            // `LevelUpPlanUnitHolder.OnDispose`: no level-up plan unit outlives the
            // construction that made it (disposed: no facts, not subscribed, unregistered).
            for e in &f.world.entities {
                if let wr_sim::world::EntityKind::Unit(u) = &e.kind
                    && u.preview_of.is_some()
                {
                    assert!(
                        e.is_disposed && e.facts.list.is_empty() && !e.subscribed,
                        "{name} {r}: preview {} alive",
                        e.unique_id
                    );
                    assert!(!f.world.by_id.contains_key(&e.unique_id));
                }
            }
        }
    }
}

/// Round 3: the Seelah runs equal the oracle's ordered T0 stream to the end (the recruited
/// companion's body, its items, the armor rules, the potion's ability, the late toggle buff,
/// the replaced NPC's disposal), and the end state of every unit matches the oracle's dump.
/// Round 9: on captures with the cutscene locks, the zone entries and the modes, with the
/// lock's buff tick and postpone, Cutscene and Pause modes. Round 10 (`poc4-seelah-5`): the unit
/// groups modelled and equal to every recorded group state (the recruited companion's new unit
/// joins the pool last). Round 11 (`poc4-seelah-7`): the dialogue's text (history lines, cue
/// and answer text, speaker names and colours) equal the game's.
#[test]
#[ignore = "needs the game's blueprint pack, the fixture saves and the poc4-seelah-7 capture"]
fn seelah_streams_equal_the_oracle_to_the_end() {
    let runs: &[(&str, &[&str])] = &[
        ("seelah-a", &["record", "replay-1", "replay-2-perturbed"]),
        (
            "seelah-b",
            &[
                "record",
                "replay-1",
                "replay-2-perturbed",
                "replay-3-perturbed",
                "replay-4-perturbed",
            ],
        ),
        ("seelah-c", &["record", "replay-1", "replay-2-perturbed", "replay-3-perturbed"]),
    ];
    for (name, rs) in runs {
        for r in *rs {
            let spec = root().join("crates/wr-sim/fixtures/poc4.json");
            let d = root().join("fixtures/captures/oracle/poc4-seelah-7/fixtures").join(name);
            let f = wr_sim::runner::run_fixture(&wr_sim::runner::FixtureOptions {
                ability_params_empty: None,
                spec: &spec,
                fixture: name,
                oracle: Some(&d),
                run: r,
                oracle_inputs: false,
            })
            .expect("fixture");
            assert!(f.outcome.failure.is_none(), "{name} {r}: {:?}", f.outcome.failure);
            let td = f.trace_diff.as_ref().expect("trace");
            assert!(td.equal(), "{name} {r}: {td:?}");
            // Valid: nothing unsupported at load or in the run (the area's scene entities,
            // script zone and `ActivateTrigger` included).
            assert!(f.valid(), "{name} {r}: {:?}", f.world.coverage.unsupported);
            assert!(f.world.combat.groups.on && f.world.recorded_groups.is_some(), "{name} {r}");
            assert_text_compared(&f, &format!("{name} {r}"));
            let units = f.units_diff.as_ref().expect("units");
            assert!(
                units.match_rate > 0.9999,
                "{name} {r}: units {} {:?}",
                units.match_rate,
                units.diffs.iter().take(5).collect::<Vec<_>>()
            );
            // The spellbooks (class levels, special lists, slots, memorized spells) and the
            // ability resources of every unit equal the oracle's, field by field.
            for class in [
                "spellbooks.present",
                "spellbook.caster_level",
                "spellbook.persisted",
                "resources.present",
                "resource.amount",
                "resource.persisted",
            ] {
                let (equal, total) = units.fields.get(class).copied().unwrap_or((0, 0));
                assert!(total > 0 && equal == total, "{name} {r}: {class} {equal}/{total}");
            }
        }
    }
}

/// Horgus (`poc4-horgus-11`: cutscene progress, locks, zone entries, awake states, roaming's
/// producers and the unit groups recorded): every run is valid. The ordered T0 stream equals the
/// oracle's (gate draws and the closing cutscene's lock included), the end state matches, the
/// groups model runs and equals every recorded group state, and roaming's two draws are tied to
/// their recorded producers (the roaming controller choosing a waypoint: movement, T1).
#[test]
#[ignore = "needs the game's blueprint pack, the fixture saves and the poc4-horgus-11 capture"]
fn horgus_runs_are_valid() {
    for (name, rs) in [
        ("horgus-check", &["record", "replay-1", "replay-2-perturbed", "replay-3-perturbed"][..]),
        ("horgus-alt", &["record", "replay-1"][..]),
    ] {
        for r in rs {
            let spec = root().join("crates/wr-sim/fixtures/poc4.json");
            let d = root().join("fixtures/captures/oracle/poc4-horgus-11/fixtures").join(name);
            let f = wr_sim::runner::run_fixture(&wr_sim::runner::FixtureOptions {
                ability_params_empty: None,
                spec: &spec,
                fixture: name,
                oracle: Some(&d),
                run: r,
                oracle_inputs: false,
            })
            .expect("fixture");
            assert!(f.outcome.failure.is_none(), "{name} {r}: {:?}", f.outcome.failure);
            let td = f.trace_diff.as_ref().expect("trace");
            assert!(td.equal(), "{name} {r}: {td:?}");
            assert!(f.valid(), "{name} {r}: {:?}", f.world.coverage.unsupported);
            assert!(f.world.combat.groups.on && f.world.recorded_groups.is_some(), "{name} {r}");
            let roaming: u64 = td
                .t1_excluded
                .iter()
                .filter(|(k, _)| k.starts_with("roaming: "))
                .map(|(_, n)| n)
                .sum();
            assert_eq!(roaming, 2, "{name} {r}: {:?}", td.t1_excluded);
            assert_text_compared(&f, &format!("{name} {r}"));
            let units = f.units_diff.as_ref().expect("units");
            assert!(units.match_rate > 0.9999, "{name} {r}: units {}", units.match_rate);
            check_units(&f, &format!("{name} {r}"));
        }
    }
}

/// A recording without the cutscene locks (`poc4-seelah-2`) can't place the closing cutscene's
/// `CommandLockControls` (its buffs tick and postpone, Cutscene mode): the run reports it.
#[test]
#[ignore = "needs the game's blueprint pack, the fixture saves and the poc4-seelah-2 capture"]
fn a_recording_without_locks_reports_the_lock() {
    let spec = root().join("crates/wr-sim/fixtures/poc4.json");
    let d = root().join("fixtures/captures/oracle/poc4-seelah-2/fixtures/seelah-a");
    let f = wr_sim::runner::run_fixture(&wr_sim::runner::FixtureOptions {
        ability_params_empty: None,
        spec: &spec,
        fixture: "seelah-a",
        oracle: Some(&d),
        run: "record",
        oracle_inputs: false,
    })
    .expect("fixture");
    assert!(!f.valid());
    assert!(
        f.world.coverage.unsupported.keys().any(|k| k.contains("CommandLockControls without")),
        "{:?}",
        f.world.coverage.unsupported
    );
}

/// Every saved `InteractionSkillCheckPart` (`DCOverride` and `SkillOverride`, written by the
/// last load's `OnSettingsDidSet` from the same view settings and id) is what the sim's load
/// recomputes from the scene cache and the object's id.
#[test]
#[ignore = "needs the game's blueprint pack, the fixture saves, their JSON dumps and the scene caches"]
fn skill_checks_recompute_the_saved_dc_and_skill() {
    fn walk(v: &serde_json::Value, out: &mut Vec<(String, i64, String)>) {
        match v {
            serde_json::Value::Object(m) => {
                if let (Some(id), Some(parts)) =
                    (m.get("UniqueId").and_then(|x| x.as_str()), m.get("Parts"))
                {
                    for p in parts["m_Parts"].as_array().into_iter().flatten() {
                        if p["$type"]
                            .as_str()
                            .is_some_and(|t| t.contains("InteractionSkillCheckPart"))
                        {
                            out.push((
                                id.to_owned(),
                                p["DCOverride"].as_i64().unwrap_or(0),
                                p["SkillOverride"].as_str().unwrap_or("Unknown").to_owned(),
                            ));
                        }
                    }
                }
                for x in m.values() {
                    walk(x, out);
                }
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut checked = 0;
    for (save, area) in [
        ("Manual_13", "bad465e5404604346bdf71ef50b3299a"),
        ("Manual_19", "61fcf2a352daa394ebae399b1348ba62"),
    ] {
        let dump = wr_paths::cache_root().join(format!("saves/dump/{save}.{area}.json"));
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&dump).expect("dump")).expect("json");
        let mut saved = Vec::new();
        walk(&v["m_MainState"], &mut saved);
        assert!(!saved.is_empty(), "{save}");
        let path = wr_save::report::find_save(save).expect("save");
        let s = wr_sim::hydrate::Save::read(&path).expect("read");
        let content = std::rc::Rc::new(wr_sim::content::Content::open_default().expect("content"));
        let registry = std::rc::Rc::new(wr_sim::components::registry());
        let (w, _) =
            wr_sim::hydrate::hydrate(content, registry, &s, &wr_sim::hydrate::Options::default())
                .expect("hydrate");
        for (id, dc, skill) in saved {
            let e = w.by_id[id.as_str()];
            let part = w.scene.map_objects[&e].skill_check.expect("part");
            assert_eq!(part.dc_override, i32::try_from(dc).expect("dc"), "{save} {id}");
            assert_eq!((part.skill_override, skill.as_str()), (0, "Unknown"), "{save} {id}");
            checked += 1;
        }
        let reported: Vec<_> = w
            .coverage
            .unsupported
            .keys()
            .filter(|k| k.starts_with("InteractionSkillCheckPart"))
            .collect();
        assert!(reported.is_empty(), "{save}: {reported:?}");
    }
    assert_eq!(checked, 9, "the parts of both saves' main states");
}

/// A copy of an oracle fixture directory with its `record` trace changed by `change`.
fn damaged_copy(
    src: &std::path::Path,
    tag: &str,
    change: &dyn Fn(&mut Vec<serde_json::Value>),
) -> PathBuf {
    use std::io::Write as _;
    let dst = std::env::temp_dir().join(format!("wr-poc4-text-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dst);
    std::fs::create_dir_all(&dst).expect("temp dir");
    for e in std::fs::read_dir(src).expect("fixture dir").flatten() {
        let name = e.file_name();
        if name.to_string_lossy().ends_with(".trace.jsonl.gz") {
            continue;
        }
        std::fs::copy(e.path(), dst.join(&name)).expect("copy");
    }
    let mut records =
        wr_sim::runner::read_trace(&src.join("record.trace.jsonl.gz")).expect("trace");
    change(&mut records);
    let f = std::fs::File::create(dst.join("record.trace.jsonl.gz")).expect("create");
    let mut gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
    for r in &records {
        writeln!(gz, "{r}").expect("write");
    }
    gz.finish().expect("gz");
    dst
}

/// The dialogue's text records damaged (`poc4-seelah-7/seelah-a`): a changed history line, cue
/// text, answer text or speaker name, a dropped record, or the declaration dropped while the
/// records stay make the run unequal; without the window's speaker state the colour of the
/// cue with no speaker is reported. The undamaged copy stays valid.
#[test]
#[ignore = "needs the game's blueprint pack, the fixture saves and the poc4-seelah-7 capture"]
fn damaged_text_records_are_caught() {
    use serde_json::{Value, json};
    let src = root().join("fixtures/captures/oracle/poc4-seelah-7/fixtures/seelah-a");
    let nth = |rs: &mut Vec<Value>, k: &str, n: usize| -> usize {
        rs.iter().enumerate().filter(|(_, r)| r["k"] == k).nth(n).expect("record").0
    };
    type Change = Box<dyn Fn(&mut Vec<Value>)>;
    let cases: Vec<(&str, bool, Change)> = vec![
        ("undamaged", true, Box::new(|_| {})),
        (
            "history line",
            false,
            Box::new(move |rs| {
                let i = nth(rs, "dlg_history", 0);
                rs[i]["line"] = json!(format!("{}x", rs[i]["line"].as_str().unwrap_or("")));
            }),
        ),
        (
            "answer history",
            false,
            Box::new(move |rs| {
                let i = rs
                    .iter()
                    .position(|r| r["k"] == "dlg_history" && r["kind"] == "answer")
                    .expect("answer");
                rs[i]["line"] = json!("Continue.");
            }),
        ),
        (
            "cue text",
            false,
            Box::new(move |rs| {
                let i = nth(rs, "dlg_text", 2);
                rs[i]["cue_text"] = json!("<color=#000000>x</color>");
            }),
        ),
        (
            "answer text",
            false,
            Box::new(move |rs| {
                let i = nth(rs, "dlg_text", 0);
                rs[i]["answers"][0]["text"] = json!("Go on");
            }),
        ),
        (
            "speaker name",
            false,
            Box::new(move |rs| {
                let i = nth(rs, "dlg_text", 1);
                rs[i]["speaker_name"] = json!("Sela");
            }),
        ),
        (
            "speaker colour",
            false,
            Box::new(move |rs| {
                let i = nth(rs, "dlg_text", 1);
                rs[i]["speaker_color"] = json!("38527F");
            }),
        ),
        (
            "dropped record",
            false,
            Box::new(move |rs| {
                let i = nth(rs, "dlg_history", 1);
                rs.remove(i);
            }),
        ),
        (
            "undeclared",
            false,
            Box::new(move |rs| {
                let i = nth(rs, "begin", 0);
                if let Some(a) = rs[i]["observers"].as_array_mut() {
                    a.retain(|o| o != "dialog_text");
                }
            }),
        ),
    ];
    let spec = root().join("crates/wr-sim/fixtures/poc4.json");
    let mut runs = Vec::new();
    for (tag, valid, change) in &cases {
        let dir = damaged_copy(&src, &tag.replace(' ', "-"), change.as_ref());
        let f = wr_sim::runner::run_fixture(&wr_sim::runner::FixtureOptions {
            ability_params_empty: None,
            spec: &spec,
            fixture: "seelah-a",
            oracle: Some(&dir),
            run: "record",
            oracle_inputs: false,
        })
        .expect("fixture");
        let equal = f.trace_diff.as_ref().is_some_and(wr_sim::runner::TraceDiff::equal);
        assert_eq!(equal && f.valid(), *valid, "{tag}: equal {equal}, valid {}", f.valid());
        runs.push(dir);
    }
    // Without the speaker state the window opened with, the cue without a speaker reads a
    // colour from before the window: reported.
    let dir = damaged_copy(&src, "no-speaker-state", &|rs| {
        if let Some(b) = rs.iter_mut().find(|r| r["k"] == "begin") {
            b.as_object_mut().expect("begin").remove("dialog_speaker");
        }
    });
    let f = wr_sim::runner::run_fixture(&wr_sim::runner::FixtureOptions {
        ability_params_empty: None,
        spec: &spec,
        fixture: "seelah-a",
        oracle: Some(&dir),
        run: "record",
        oracle_inputs: false,
    })
    .expect("fixture");
    assert!(!f.valid());
    assert!(
        f.world.coverage.unsupported.keys().any(|k| k.contains("custom speaker colour")),
        "{:?}",
        f.world.coverage.unsupported
    );
    runs.push(dir);
    // The speaker state's fields missing or mistyped: reported, not taken as null.
    for (tag, value) in [
        ("color-missing", None),
        ("color-number", Some(json!(123))),
        ("name-number", Some(json!(7))),
    ] {
        let dir = damaged_copy(&src, tag, &move |rs| {
            if let Some(b) = rs.iter_mut().find(|r| r["k"] == "begin") {
                let s = b["dialog_speaker"].as_object_mut().expect("dialog_speaker");
                let k = if tag.starts_with("name") { "custom_name" } else { "custom_color" };
                match &value {
                    Some(v) => {
                        s.insert(k.into(), v.clone());
                    }
                    None => {
                        s.remove(k);
                    }
                }
            }
        });
        let f = wr_sim::runner::run_fixture(&wr_sim::runner::FixtureOptions {
            ability_params_empty: None,
            spec: &spec,
            fixture: "seelah-a",
            oracle: Some(&dir),
            run: "record",
            oracle_inputs: false,
        })
        .expect("fixture");
        assert!(!f.valid(), "{tag}");
        assert!(
            f.world.coverage.unsupported.keys().any(|k| k.contains("malformed dialog_speaker")),
            "{tag}: {:?}",
            f.world.coverage.unsupported
        );
        runs.push(dir);
    }
    for d in runs {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// A run without a recording loads the text too (a dialogue's start makes its speaker name),
/// and writes no text records (nothing to compare them with).
#[test]
#[ignore = "needs the game's blueprint pack, the localization pack and Manual_13"]
fn a_run_without_a_recording_loads_the_text() {
    let spec = root().join("crates/wr-sim/fixtures/poc4.json");
    let f = wr_sim::runner::run_fixture(&wr_sim::runner::FixtureOptions {
        ability_params_empty: None,
        spec: &spec,
        fixture: "seelah-a",
        oracle: None,
        run: "record",
        oracle_inputs: false,
    })
    .expect("fixture");
    assert!(f.world.text.is_some());
    assert!(!f.world.dialog.text_records);
    assert!(!f.world.events.records.iter().any(|e| e.k == "dlg_history" || e.k == "dlg_text"));
}
