//! `wr-text`: build and query our localization packs.
//!
//! ```text
//! wr-text build [--locale enGB]       read the install's JSON pack once, write cache/text/<locale>.wrtext
//! wr-text get <key> [--locale enGB]   a key's raw text
//! wr-text bench [--locale enGB]       time the JSON read, our pack's open and lookups
//! wr-text tags                        the engine's registered tags
//! wr-text scan-generic <in> <out>     the scan on a JSON array of strings, every template as
//!                                     `<tag:capitalized:params>` (to check against the C#'s)
//! ```

use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use wr_text::{Locale, Pack, pack};

#[derive(Parser, Debug)]
#[command(about = "wr-text: localization packs and the text template engine")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    Build {
        #[arg(long, default_value = "enGB")]
        locale: String,
    },
    Get {
        key: String,
        #[arg(long, default_value = "enGB")]
        locale: String,
    },
    Bench {
        #[arg(long, default_value = "enGB")]
        locale: String,
    },
    Tags,
    ScanGeneric {
        input: std::path::PathBuf,
        output: std::path::PathBuf,
    },
}

fn locale(s: &str) -> Result<Locale> {
    Locale::parse(s).with_context(|| format!("unknown locale {s}"))
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Build { locale: l } => {
            let l = locale(&l)?;
            let t = Instant::now();
            let built = pack::ensure_built(l)?;
            let p = Pack::load(l)?;
            println!(
                "{}: {} strings, {} in {:.1} ms -> {}",
                l.name(),
                p.len(),
                if built { "built" } else { "up to date" },
                t.elapsed().as_secs_f64() * 1e3,
                pack::cache_path(l).display()
            );
        }
        Cmd::Get { key, locale: l } => {
            let p = Pack::load(locale(&l)?)?;
            println!("{}", p.get_text(&key, true)?);
        }
        Cmd::Bench { locale: l } => {
            let l = locale(&l)?;
            let t = Instant::now();
            let strings = pack::read_json(&pack::source_path(l))?;
            let json = t.elapsed();
            pack::ensure_built(l)?;
            let t = Instant::now();
            // SAFETY: the cache's packs are only ever replaced by a rename.
            let p = unsafe { Pack::open(&pack::cache_path(l), l) }?;
            let open = t.elapsed();
            let t = Instant::now();
            let mut bytes = 0usize;
            for (k, v) in &strings {
                let got = p.find(k)?.context("key missing from our pack")?;
                anyhow::ensure!(got == v, "key {k}: text differs");
                bytes += got.len();
            }
            let look = t.elapsed();
            #[allow(clippy::cast_precision_loss)]
            let per = look.as_secs_f64() * 1e9 / strings.len() as f64;
            println!(
                "{}: {} strings ({} MB of text); JSON read {:.1} ms; our pack open {:.2} ms; every key looked up and equal, {:.0} ns each",
                l.name(),
                strings.len(),
                bytes / 1_000_000,
                json.as_secs_f64() * 1e3,
                open.as_secs_f64() * 1e3,
                per
            );
        }
        Cmd::Tags => {
            for t in wr_text::engine::template_tags() {
                println!("{t}");
            }
        }
        Cmd::ScanGeneric { input, output } => {
            let inp: Vec<String> = serde_json::from_slice(&std::fs::read(&input)?)?;
            let out: Vec<String> = inp
                .iter()
                .map(|s| wr_text::engine::scan_generic(s).unwrap_or_else(|_| "!THROWS".to_owned()))
                .collect();
            std::fs::write(&output, serde_json::to_vec(&out)?)?;
        }
    }
    Ok(())
}
