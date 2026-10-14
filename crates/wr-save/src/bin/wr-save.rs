//! `wr-save`: reports over original saves (read-only) and our save format.
//!
//! ```text
//! wr-save report <save>            typed coverage, references, opaque values
//! wr-save roundtrip <save> [-n N]  preservation through our format, timings, sizes
//! wr-save longtail                 match rates over every save in the game's folder
//! wr-save dump <save> <entry>      canonical text of one document (to cache/saves/dump)
//! ```
//!
//! `<save>` is a path or a name prefix (`Manual_13`), looked up in
//! `fixtures/saves/` (gitignored) and then the save folder. Saves
//! are only read. Reports go to `cache/saves/` (gitignored).

use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use serde_json::json;
use wr_save::report::{self, short_name};
use wr_save::zks;

#[derive(Parser, Debug)]
#[command(about = "Original .zks saves: typed decoding, coverage, our format")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    Report {
        save: String,
    },
    Roundtrip {
        save: String,
        #[arg(short = 'n', default_value_t = 5)]
        runs: usize,
    },
    Longtail,
    /// Types and hooks a hydrating loader needs for a save, ranked by state.
    Hydration {
        save: String,
    },
    Dump {
        save: String,
        entry: String,
    },
}

fn summary_line(name: &str, cov: &serde_json::Value) -> String {
    format!(
        "{name}: {} objects, {:.2}% clean; {:.2}% of JSON values typed; {} of {} $types known; {} unknown properties",
        cov["objects"]["total"],
        cov["objects"]["clean_pct"].as_f64().unwrap_or(0.0),
        cov["values"]["typed_pct"].as_f64().unwrap_or(0.0),
        cov["objects"]["types_known"],
        cov["objects"]["types_seen"],
        cov["unknown_property_total"],
    )
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Report { save } => {
            let path = report::find_save(&save)?;
            let t = Instant::now();
            let entries = zks::read(&path)?;
            zks::check_versions(&entries)?;
            let decoded = zks::decode_all(&entries);
            eprintln!("decoded {} in {:.0} ms", path.display(), t.elapsed().as_secs_f64() * 1000.0);
            let cov = report::coverage(&path, &entries, &decoded);
            let name = short_name(&path);
            let p = report::write_report(&format!("{name}.coverage.json"), &cov)?;
            println!("{}", summary_line(&name, &cov));
            println!("blueprint refs: {}", cov["blueprint_refs"]);
            println!("refs: {}", cov["refs"]);
            println!("opaque: {}", cov["opaque"]);
            println!("wrote {}", p.display());
        }
        Cmd::Roundtrip { save, runs } => {
            let path = report::find_save(&save)?;
            let r = report::roundtrip(&path, runs)?;
            let name = short_name(&path);
            let p = report::write_report(&format!("{name}.roundtrip.json"), &r)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "sizes": r["sizes"], "ms_min_median": r["ms_min_median"],
                    "source_equivalence_failures": r["source_equivalence"]["failures"].as_array().map_or(0, Vec::len),
                    "preservation": r["preservation"]["pass"],
                }))?
            );
            println!("wrote {}", p.display());
        }
        Cmd::Hydration { save } => {
            let path = report::find_save(&save)?;
            let entries = zks::read(&path)?;
            zks::check_versions(&entries)?;
            let decoded = zks::decode_all(&entries);
            let h = report::hydration(&decoded);
            let p = report::write_report(&format!("{}.hydration.json", short_name(&path)), &h)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "types": h["types"], "by_category": h["by_category"], "hooks": h["hooks"],
                    "top": h["ranked"].as_array().map(|a| a.iter().take(25).cloned().collect::<Vec<_>>()),
                    "fact_components": h["fact_components"].get("component_types"),
                }))?
            );
            println!("wrote {}", p.display());
        }
        Cmd::Longtail => {
            let mut union = std::collections::HashMap::new();
            let mut rows = Vec::new();
            for path in report::all_saves()? {
                let t = Instant::now();
                let entries = zks::read(&path).with_context(|| path.display().to_string())?;
                if let Err(e) = zks::check_versions(&entries) {
                    println!("{}: skipped: {e}", short_name(&path));
                    continue;
                }
                let decoded = zks::decode_all(&entries);
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                let cov = report::coverage(&path, &entries, &decoded);
                let name = short_name(&path);
                report::write_report(&format!("{name}.coverage.json"), &cov)?;
                for (k, v) in report::merged(&decoded).per_type {
                    let e: &mut wr_save::decode::TypeStats = union.entry(k).or_default();
                    e.objects += v.objects;
                    e.clean += v.clean;
                }
                println!("{}  ({ms:.0} ms)", summary_line(&name, &cov));
                rows.push(json!({
                    "save": name,
                    "zks_bytes": std::fs::metadata(&path)?.len(),
                    "objects": cov["objects"]["total"],
                    "clean_pct": cov["objects"]["clean_pct"],
                    "values_typed_pct": cov["values"]["typed_pct"],
                    "values": cov["values"],
                    "type_tags": cov["type_tags"],
                    "coerced": cov["coerced"],
                    "skipped_at": cov["skipped_at"],
                    "entity_refs": cov["entity_refs"],
                    "types_seen": cov["objects"]["types_seen"],
                    "types_known": cov["objects"]["types_known"],
                    "types_unknown": cov["objects"]["types_unknown"],
                    "unknown_properties": cov["unknown_property_total"],
                    "opaque": cov["opaque"],
                    "refs": cov["refs"],
                    "blueprint_refs": cov["blueprint_refs"],
                    "parse_errors": cov["parse_errors"],
                    "pending_upgraders": cov["pending_upgraders"],
                    "decode_ms": (ms * 10.0).round() / 10.0,
                }));
            }
            let curve = report::type_curve(&union);
            println!("all saves: {curve}");
            let p = report::write_report("longtail.json", &json!({"saves": rows, "curve": curve}))?;
            println!("wrote {}", p.display());
        }
        Cmd::Dump { save, entry } => {
            let path = report::find_save(&save)?;
            let entries = zks::read(&path)?;
            let e = entries.iter().find(|e| e.name == entry).context("no such entry")?;
            let d = zks::decode_entry(e);
            let zks::Body::Doc(doc) = &d.body else { anyhow::bail!("not a JSON document") };
            let dir = report::cache_dir().join("dump");
            std::fs::create_dir_all(&dir)?;
            let p = dir.join(format!("{}.{entry}", short_name(&path)));
            std::fs::write(&p, wr_save::canon::doc_text(doc))?;
            println!("wrote {}", p.display());
        }
    }
    Ok(())
}
