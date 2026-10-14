//! `wr-codegen`: reads the decompiled C# and writes the blueprint schema for wr-content.
//!
//! ```text
//! wr-codegen gen [--decompiled DIR] [--out crates/wr-content/src/generated/schema.rs]
//! ```
//!
//! Also writes a summary (closure size, unresolved types, hooks) to
//! `cache/content/codegen.json`. That file names game types only, but it lives in the
//! cache with the other reports.

mod emit;
mod model;
mod parse;
mod rules;
mod save;

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(about = "Decompiled C# -> TypeId registry and blueprint schemas")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Generates the schema source file.
    Gen {
        /// Root of the decompiled assemblies (one directory per assembly).
        #[arg(long)]
        decompiled: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Generates the save-side schema for wr-save.
    GenSave {
        #[arg(long)]
        decompiled: Option<PathBuf>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Prints save-schema entries whose CLR name contains the text (debugging).
    ShowSave {
        name: String,
        #[arg(long)]
        decompiled: Option<PathBuf>,
    },
    /// Prints a type's resolved schema entry (debugging).
    Show {
        name: String,
        #[arg(long)]
        decompiled: Option<PathBuf>,
    },
}

/// Assemblies whose declarations feed the schema. Assembly-CSharp holds every `[TypeId]`
/// type; the rest supply base classes, `[Serializable]` field types and Unity types.
const ASSEMBLIES: &[&str] = &[
    "Assembly-CSharp",
    "Owlcat.Runtime.Core",
    "Owlcat.Runtime.UI",
    "Owlcat.Runtime.Visual",
    "Owlcat.Runtime.Validation",
    "Owlcat.Runtime.UniRx",
    "Owlcat.SharedTypes",
    "Assembly-CSharp-firstpass",
    "Core.Cheats",
    "Core.Reflection",
    "UnityEngine.CoreModule",
    "UnityEngine.AnimationModule",
    "UnityEngine.AudioModule",
    "UnityEngine.IMGUIModule",
    "UnityEngine.TextRenderingModule",
    "UnityEngine.ParticleSystemModule",
    "UnityEngine.PhysicsModule",
    "UnityEngine.Physics2DModule",
    "UnityEngine.TerrainModule",
    "UnityEngine.UI",
    "UnityEngine.UIModule",
    "UnityEngine.AIModule",
    "UnityEngine.VFXModule",
    "UnityEngine.VideoModule",
    "UnityEngine.DirectorModule",
    "Unity.TextMeshPro",
    "Unity.Timeline",
];

fn default_decompiled() -> PathBuf {
    wr_paths::decompiled()
}

fn load(dir: &std::path::Path) -> Result<model::Universe> {
    let t = Instant::now();
    let mut all = Vec::new();
    for a in ASSEMBLIES {
        let p = dir.join(a);
        if !p.is_dir() {
            eprintln!("warning: {} missing; its types won't resolve", p.display());
            continue;
        }
        let mut types = parse::parse_assembly(&p).with_context(|| a.to_string())?;
        let base = all.len();
        for ty in &mut types {
            if let Some(o) = ty.outer.as_mut() {
                *o += base;
            }
        }
        all.extend(types);
    }
    eprintln!(
        "parsed {} declarations in {:.1}s ({} files with syntax errors)",
        all.len(),
        t.elapsed().as_secs_f64(),
        parse::PARSE_ERRORS.load(std::sync::atomic::Ordering::Relaxed)
    );
    Ok(model::Universe::new(all))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Gen { decompiled, out } => {
            let dir = decompiled.unwrap_or_else(default_decompiled);
            let u = load(&dir)?;
            let t = Instant::now();
            let mut b = rules::Builder::new(&u);
            b.build_all();
            eprintln!(
                "closure: {} types, {} enums in {:.1}s",
                b.types.len(),
                b.enums.len(),
                t.elapsed().as_secs_f64()
            );
            let out = out.unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../crates/wr-content/src/generated/schema.rs")
            });
            let src = emit::emit(&b);
            std::fs::write(&out, src).with_context(|| out.display().to_string())?;
            eprintln!("wrote {}", out.display());
            let mut summary = emit::summary(&b);
            summary.syntax_error_files =
                parse::ERROR_FILES.lock().map(|v| v.clone()).unwrap_or_default();
            summary.syntax_error_files.sort();
            let cache = wr_paths::cache_root().join("content");
            std::fs::create_dir_all(&cache)?;
            let sp = cache.join("codegen.json");
            std::fs::write(&sp, serde_json::to_vec_pretty(&summary)?)?;
            eprintln!("wrote {}", sp.display());
        }
        Cmd::GenSave { decompiled, out } => {
            let dir = decompiled.unwrap_or_else(default_decompiled);
            let u = load(&dir)?;
            let t = Instant::now();
            let mut b = save::SaveBuilder::new(&u);
            eprintln!("pool: {:.1}s", t.elapsed().as_secs_f64());
            b.build_all();
            eprintln!(
                "save closure: {} types, {} enums in {:.1}s",
                b.types.len(),
                b.enums.len(),
                t.elapsed().as_secs_f64()
            );
            let out = out.unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../crates/wr-save/src/generated/schema.rs")
            });
            std::fs::write(&out, save::emit(&b)).with_context(|| out.display().to_string())?;
            eprintln!("wrote {}", out.display());
            let cache = wr_paths::cache_root().join("saves");
            std::fs::create_dir_all(&cache)?;
            let sp = cache.join("codegen-save.json");
            std::fs::write(&sp, serde_json::to_vec_pretty(&save::summary(&b))?)?;
            eprintln!("wrote {}", sp.display());
        }
        Cmd::ShowSave { name, decompiled } => {
            let dir = decompiled.unwrap_or_else(default_decompiled);
            let u = load(&dir)?;
            let mut b = save::SaveBuilder::new(&u);
            b.build_all();
            for t in b.types.iter().filter(|t| t.clr.contains(&name)) {
                println!("{}", serde_json::to_string_pretty(t)?);
            }
        }
        Cmd::Show { name, decompiled } => {
            let dir = decompiled.unwrap_or_else(default_decompiled);
            let u = load(&dir)?;
            let mut b = rules::Builder::new(&u);
            b.build_all();
            for t in b.types.iter().filter(|t| t.name == name || t.short == name) {
                println!("{}", serde_json::to_string_pretty(t)?);
            }
        }
    }
    Ok(())
}
