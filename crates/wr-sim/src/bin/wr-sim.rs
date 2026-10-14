//! `wr-sim`: hydrate a save and report (read-only; outputs under `cache/sim/`).
//!
//! ```text
//! wr-sim hydrate <save> [--trace]   hydrate, write the sheet dump, manifest and coverage
//! wr-sim compare <ours> <oracle>    compare our dump with a WrathOracle state dump
//! wr-sim dialog <fixture> [--oracle <fixture dir>] [--run record|replay-1|…]
//!                                   run a POC-4 dialogue fixture (crates/wr-sim/fixtures/
//!                                   poc4.json) and compare it with the oracle's recording
//! ```

use std::rc::Rc;
use std::time::Instant;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(about = "wr-sim: hydrate saves, character sheets, coverage")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    Hydrate {
        save: String,
        #[arg(long)]
        trace: bool,
        /// `AbilityParams.Empty` at the start of the oracle's load: `{"CasterLevel":n,
        /// "RankBonus":m}` or an oracle state dump with an `abilityParamsEmpty` member.
        #[arg(long)]
        ability_params_empty: Option<String>,
    },
    /// Compare our sheet dump with a WrathOracle `state-*.json` dump.
    Compare { ours: std::path::PathBuf, oracle: std::path::PathBuf },
    /// Build `cache/sim/scenes/<area guid>.json` (script zone settings and shapes, the
    /// scene's views and map-object components) from the importer's inventory of the area
    /// (`wr-import import --area <name>` writes `cache/import/<name>/inventory.json`).
    SceneImport {
        inventory: std::path::PathBuf,
        /// The area blueprint's GUID.
        #[arg(long)]
        area: String,
    },
    /// Run a dialogue fixture headless and compare it with an oracle recording.
    Dialog {
        /// The fixture's name in the spec.
        fixture: String,
        #[arg(long, default_value = "crates/wr-sim/fixtures/poc4.json")]
        spec: std::path::PathBuf,
        /// The oracle's fixture directory (`…/fixtures/<name>`).
        #[arg(long)]
        oracle: Option<std::path::PathBuf>,
        /// Which oracle run to replay and compare (`record`, `replay-1`,
        /// `replay-2-perturbed`…).
        #[arg(long, default_value = "record")]
        run: String,
        /// Output directory (default `cache/sim/dialog/<fixture>-<run>`).
        #[arg(long)]
        out: Option<std::path::PathBuf>,
        /// Answer with the oracle run's recorded answers (`answerbp:` each, in order)
        /// instead of the spec's answer steps: for fixtures whose optional steps depend on
        /// T1 timing (the certification fixture).
        #[arg(long)]
        oracle_inputs: bool,
        /// `AbilityParams.Empty` at the start of the oracle's load: `{"CasterLevel":n,
        /// "RankBonus":m}` or an oracle state dump with an `abilityParamsEmpty` member.
        #[arg(long)]
        ability_params_empty: Option<String>,
    },
}

fn out_dir() -> std::path::PathBuf {
    let d = wr_paths::cache_root().join("sim");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Hydrate { save, trace, ability_params_empty } => {
            let t0 = Instant::now();
            let path = wr_save::report::find_save(&save)?;
            let s = wr_sim::hydrate::Save::read(&path)?;
            let t_read = t0.elapsed();
            let content = Rc::new(wr_sim::content::Content::open_default()?);
            let t_content = t0.elapsed();
            let registry = Rc::new(wr_sim::components::registry());
            let (mut w, report) = wr_sim::hydrate::hydrate(
                content.clone(),
                registry,
                &s,
                &wr_sim::hydrate::Options {
                    trace,
                    ability_params_empty: ability_params_empty
                        .as_deref()
                        .map(wr_sim::hydrate::parse_ability_params_empty)
                        .transpose()?
                        .flatten(),
                    ..Default::default()
                },
            )?;
            let t_hyd = t0.elapsed();
            let units: Vec<wr_sim::world::EntityIdx> = (0..w.entities.len())
                .map(|i| wr_sim::world::EntityIdx(u32::try_from(i).unwrap_or(0)))
                .filter(|e| w.unit(*e).is_some())
                .collect();
            // The run ends here: a replay tape with draws left would fail now.
            w.finish_run();
            let manifest = wr_sim::manifest::build(&w, &units);
            let whole = wr_sim::manifest::from_save(&w, &s);
            let sheet = wr_sim::sheet::dump(&w, &units, "after-load");
            let name = wr_save::report::short_name(&path);
            let dir = out_dir();
            std::fs::write(
                dir.join(format!("{name}.manifest.json")),
                serde_json::to_string_pretty(&manifest)?,
            )?;
            std::fs::write(
                dir.join(format!("{name}.manifest-whole-save.json")),
                serde_json::to_string_pretty(&whole)?,
            )?;
            std::fs::write(
                dir.join(format!("{name}.state-after-load.json")),
                serde_json::to_string_pretty(&sheet)?,
            )?;
            let cov = serde_json::json!({
                "valid": w.valid(),
                "rng_failures": w.rng.failures(),
                "unsupported": w.coverage.unsupported,
                "unsupported_by_unit": w.coverage.unsupported_by_unit,
                "executed": w.coverage.executed,
                "log": w.log,
                "report": report,
                "content_loaded_bbp_zip": content.loaded_by_source(),
                "timings_ms": {"read_decode": t_read.as_secs_f64()*1e3, "open_content": (t_content - t_read).as_secs_f64()*1e3, "hydrate": (t_hyd - t_content).as_secs_f64()*1e3},
            });
            std::fs::write(
                dir.join(format!("{name}.coverage.json")),
                serde_json::to_string_pretty(&cov)?,
            )?;
            if let Some(t) = &w.trace {
                std::fs::write(
                    dir.join(format!("{name}.trace.json")),
                    serde_json::to_string_pretty(t)?,
                )?;
            }
            println!(
                "{name}: {} units, {} facts, valid={}, {} unsupported kinds; manifest: {} types, {} implemented, {} required-missing ({} on active facts)",
                units.len(),
                report.facts,
                w.valid(),
                w.coverage.unsupported.len(),
                manifest.types,
                manifest.implemented,
                manifest.required_missing,
                manifest.missing_on_active_facts
            );
            println!(
                "timings: read+decode {:.1} ms, open content {:.1} ms, hydrate {:.1} ms",
                t_read.as_secs_f64() * 1e3,
                (t_content - t_read).as_secs_f64() * 1e3,
                (t_hyd - t_content).as_secs_f64() * 1e3
            );
        }
        Cmd::Dialog { fixture, spec, oracle, run, out, oracle_inputs, ability_params_empty } => {
            let start = ability_params_empty
                .as_deref()
                .map(wr_sim::hydrate::parse_ability_params_empty)
                .transpose()?
                .flatten();
            dialog(&fixture, &spec, oracle.as_deref(), &run, out, oracle_inputs, start)?;
        }
        Cmd::SceneImport { inventory, area } => {
            let c = wr_sim::scene::SceneCache::import(&inventory, &area)?;
            let p = c.save()?;
            println!(
                "area {}: {} zones, {} views -> {}",
                c.area,
                c.zones.len(),
                c.views.len(),
                p.display()
            );
        }
        Cmd::Compare { ours, oracle } => {
            let read = |p: &std::path::Path| -> Result<serde_json::Value> {
                let b = std::fs::read(p)?;
                Ok(serde_json::from_slice(&b)?)
            };
            let r = wr_sim::compare::compare(&read(&ours)?, &read(&oracle)?);
            let out = out_dir().join("compare.json");
            std::fs::write(&out, serde_json::to_string_pretty(&r)?)?;
            println!(
                "units compared {}, match rate {:.4}; per field:",
                r.units_compared, r.match_rate
            );
            for (k, (e, t)) in &r.fields {
                println!("  {k}: {e}/{t}");
            }
            for (u, (e, t)) in &r.per_unit {
                println!("  unit {u}: {e}/{t}");
            }
            println!("details: {}", out.display());
        }
    }
    Ok(())
}

fn dialog(
    fixture: &str,
    spec: &std::path::Path,
    oracle: Option<&std::path::Path>,
    run: &str,
    out: Option<std::path::PathBuf>,
    oracle_inputs: bool,
    ability_params_empty: Option<(i32, i32)>,
) -> Result<()> {
    use wr_sim::narrative::dump::summary;
    let r = wr_sim::runner::run_fixture(&wr_sim::runner::FixtureOptions {
        spec,
        fixture,
        oracle,
        run,
        oracle_inputs,
        ability_params_empty,
    })?;
    let dir = out.unwrap_or_else(|| out_dir().join("dialog").join(format!("{fixture}-{run}")));
    std::fs::create_dir_all(&dir)?;
    let pretty = |name: &str, v: &dyn erased::Ser| -> Result<()> {
        std::fs::write(dir.join(name), v.pretty()?)?;
        Ok(())
    };
    if let Some(n) = &r.narrative_start {
        pretty("narrative-start.json", n)?;
    }
    if let Some(d) = &r.start_diff {
        pretty("compare-start.json", d)?;
    }
    pretty("narrative-end.json", &r.narrative_end)?;
    pretty("state-end.json", &r.sheet_end)?;
    let mut trace = String::new();
    for e in &r.world.events.records {
        trace.push_str(&serde_json::to_string(e)?);
        trace.push('\n');
    }
    std::fs::write(dir.join("trace.jsonl"), trace)?;
    let w = &r.world;
    let run_unsupported = r.run_unsupported();
    let mut report = serde_json::json!({
        "fixture": fixture, "save": r.save, "run": run,
        "outcome": r.outcome,
        "valid": r.valid(),
        "rng_failures": w.rng.failures(),
        "unsupported_at_load": r.load_unsupported,
        "unsupported_in_run": run_unsupported,
        "stubbed": w.coverage.stubbed,
        "log": w.log,
        "run_ms": r.run_ms,
        "start": r.start_diff.as_ref().map(summary),
    });
    println!(
        "{fixture} ({run}): {} steps, {} ticks, {} answers, failure: {:?}; valid={}; rng failures {}; run {:.1} ms",
        r.outcome.steps_done,
        r.outcome.ticks,
        r.outcome.answers.len(),
        r.outcome.failure,
        r.valid(),
        w.rng.failures().len(),
        r.run_ms
    );
    if !run_unsupported.is_empty() {
        println!("  unsupported in the run: {run_unsupported:?}");
    }
    if let (Some(td), Some(nd), Some(sd)) = (&r.trace_diff, &r.end_diff, &r.units_diff) {
        if let Some(d) = &r.start_diff {
            println!("  start state (narrative) vs oracle: {:?}", summary(d));
        }
        let s = |d: &wr_sim::runner::StreamDiff| {
            format!("{}/{} equal prefix {} ({})", d.ours, d.oracle, d.equal_prefix, d.equal)
        };
        println!("  trace T0 (one ordered stream): {}; equal={}", s(&td.all), td.equal());
        println!(
            "    by kind: dialog {}, mutations {}, rules {}, draws {}",
            s(&td.dialog),
            s(&td.mutations),
            s(&td.rules),
            s(&td.draws)
        );
        println!("    without facts of units created in the run: {}", s(&td.all_except_new_owners));
        if !td.unmodelled_draws.is_empty() {
            println!("    UNMODELLED sim draws: {:?}", td.unmodelled_draws);
        }
        println!("  end state (narrative) vs oracle: {:?}", summary(nd));
        println!(
            "  end state (units) vs oracle: {} units, match rate {:.4}",
            sd.units_compared, sd.match_rate
        );
        report["trace"] = serde_json::to_value(td)?;
        report["end_narrative"] = serde_json::to_value(nd)?;
        report["end_units"] = serde_json::json!({ "units_compared": sd.units_compared, "match_rate": sd.match_rate, "per_unit": sd.per_unit, "fields": sd.fields, "diffs": sd.diffs.iter().take(50).collect::<Vec<_>>() });
    }
    if let Some((ours, theirs)) = &r.streams {
        pretty("streams-ours.json", ours)?;
        pretty("streams-oracle.json", theirs)?;
    }
    pretty("report.json", &report)?;
    println!("  details: {}", dir.display());
    Ok(())
}

/// Pretty JSON for any serializable value behind a trait object.
mod erased {
    pub trait Ser {
        fn pretty(&self) -> serde_json::Result<String>;
    }
    impl<T: serde::Serialize> Ser for T {
        fn pretty(&self) -> serde_json::Result<String> {
            serde_json::to_string_pretty(self)
        }
    }
}
