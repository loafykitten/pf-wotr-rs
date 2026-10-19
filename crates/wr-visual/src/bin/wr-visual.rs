//! `wr-visual`: the unit views of a save.
//!
//! ```text
//! wr-visual spec <save> [-o out.json]   write the units' spec (default cache/poc7/<save>.units.json)
//! ```
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(about = "wr-visual: unit views from a hydrated save")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Writes the spec of the units a save puts in its area.
    Spec {
        save: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Spec { save, out } => {
            let spec = wr_visual::spec::build(&save)?;
            let out = out.unwrap_or_else(|| {
                wr_paths::cache_root().join("poc7").join(format!("{}.units.json", spec.save))
            });
            spec.write(&out)?;
            eprintln!("{} units -> {}", spec.units.len(), out.display());
            for u in &spec.units {
                eprintln!(
                    "  {} {} prefab {} at {:?} facing {:.1}, life {} prone {}, {} equipment entities",
                    u.id,
                    u.name,
                    u.prefab,
                    u.position,
                    u.orientation,
                    u.life_state,
                    u.prone,
                    u.equipment.len()
                );
            }
            Ok(())
        }
    }
}
