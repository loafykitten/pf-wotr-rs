//! POC-4 against WrathOracle recordings (local captures, gitignored): the certification
//! fixture `dialog-checks` (Seelah/Anevia meeting, Athletics check) recorded, replayed,
//! and replayed with the check's d20 perturbed (success branch). Ignored by default: it
//! needs the game's blueprint pack, Manual_13 and `fixtures/captures/oracle/cert-9`.
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
#[ignore = "needs the game's blueprint pack, Manual_13 and the cert-9 capture"]
fn dialog_checks_matches_the_oracle_on_both_branches() {
    for r in ["record", "replay-1", "replay-2-perturbed"] {
        let f = run("dialog-checks", "cert-9/fixtures/dialog-checks", r);
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
            "poc4-seelah-1/fixtures/seelah-a",
            &["record", "replay-1", "replay-2-perturbed"],
        ),
        ("seelah-b", "poc4-seelah-1/fixtures/seelah-b", &["record", "replay-2-perturbed"]),
        ("seelah-c", "poc4-seelah-1/fixtures/seelah-c", &["record", "replay-2-perturbed"]),
        ("horgus-check", "poc4-horgus-1/fixtures/horgus-check", &["record", "replay-2-perturbed"]),
        ("horgus-alt", "poc4-horgus-1/fixtures/horgus-alt", &["record"]),
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

/// Round 3: the Seelah runs on the clean-build captures (`poc4-seelah-2`) equal the oracle's
/// ordered T0 stream to the end (the recruited companion's body, its items, the armor rules,
/// the potion's ability, the late toggle buff, the replaced NPC's disposal), and the end
/// state of every unit matches the oracle's dump.
#[test]
#[ignore = "needs the game's blueprint pack, the fixture saves and the poc4-seelah-2 capture"]
fn seelah_streams_equal_the_oracle_to_the_end() {
    let runs: &[(&str, &[&str])] = &[
        ("seelah-a", &["record", "replay-1", "replay-2-perturbed"]),
        ("seelah-b", &["record", "replay-1", "replay-2-perturbed"]),
        ("seelah-c", &["record", "replay-1", "replay-2-perturbed"]),
    ];
    for (name, rs) in runs {
        for r in *rs {
            let spec = root().join("crates/wr-sim/fixtures/poc4.json");
            let d = root().join("fixtures/captures/oracle/poc4-seelah-2/fixtures").join(name);
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

/// Horgus: the dialogue and the rule draws equal the oracle's, and the run reports what it
/// doesn't port: the background and played cutscenes' random-track gates (whose draws, with a
/// roaming waypoint's previous-point choice, are the oracle's only draws the sim misses) and
/// the awake state of the unit the closing cutscene spawns (`SleepingUnitsController`: fog of
/// war and the view's wake timers), whose toggle the sim therefore doesn't tick: the ordered T0
/// stream equals the oracle's up to that unit's first toggle rule.
#[test]
#[ignore = "needs the game's blueprint pack, the fixture saves and the poc4-horgus-2 capture"]
fn horgus_runs_report_what_they_do_not_port() {
    const RANDOM_TRACK: &str =
        "cutscene: Gate RandomTrack (SelectTrackToStart draws from the global RNG)";
    const AWAKE: &str =
        "UnitActivatableAbilitiesController: area unit with a toggle to tick (AwakeUnits)";
    const GROUP: &str = "UnitEntityData.OnIsInGameChanged: group membership";
    for (name, rs) in
        [("horgus-check", &["record", "replay-2-perturbed"][..]), ("horgus-alt", &["record"][..])]
    {
        for r in rs {
            let spec = root().join("crates/wr-sim/fixtures/poc4.json");
            let d = root().join("fixtures/captures/oracle/poc4-horgus-2/fixtures").join(name);
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
            // The first difference of the ordered stream is the spawned unit's toggle.
            let first = td.all.first_difference.as_ref().expect("the spawned unit's toggle");
            let text = format!("{first:?}");
            assert!(text.contains("new#"), "{name} {r}: {text}");
            let kinds: Vec<&String> = f.world.coverage.unsupported.keys().collect();
            // The closing cutscene's HideUnit and Spawn change IsInGame, whose group `Remove`
            // isn't modelled (entity::set_is_in_game reports it).
            assert!(
                kinds.iter().all(|k| *k == RANDOM_TRACK || *k == AWAKE || *k == GROUP),
                "{name} {r}: {kinds:?}"
            );
            assert!(f.load_unsupported.contains_key(RANDOM_TRACK), "{name} {r}");
            assert!(f.world.coverage.unsupported.contains_key(AWAKE), "{name} {r}");
            assert!(!f.valid(), "{name} {r}");
            let sites: Vec<&String> = td.unmodelled_draws.keys().collect();
            assert!(
                !sites.is_empty()
                    && sites
                        .iter()
                        .all(|k| k.contains("SelectTrackToStart") || k.contains("SelectPrevPoint")),
                "{name} {r}: {sites:?}"
            );
            check_units(&f, &format!("{name} {r}"));
        }
    }
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
