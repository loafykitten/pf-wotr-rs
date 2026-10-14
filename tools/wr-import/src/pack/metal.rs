//! Offline Metal compilation: each distinct MSL source becomes a `.metallib` through
//! Xcode's Metal toolchain (`xcrun metal` then `xcrun metallib`), cached by the source's
//! hash so rebuilding a pack only compiles what changed.
//!
//! Options: the toolchain's defaults (latest language version, fast math on), which is
//! what wgpu uses when it compiles passthrough MSL at runtime; Unity's own options are
//! not known yet (POC-0 rendering report, open question).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

/// Compiles `msl` to a metallib, using `cache_dir/<hash>.metallib` if present.
pub fn compile(cache_dir: &Path, hash: u64, msl: &str) -> Result<Vec<u8>> {
    let lib = cache_dir.join(format!("{hash:016x}.metallib"));
    if let Ok(b) = std::fs::read(&lib) {
        return Ok(b);
    }
    std::fs::create_dir_all(cache_dir)?;
    let src = cache_dir.join(format!("{hash:016x}.metal"));
    let air = cache_dir.join(format!("{hash:016x}.air"));
    std::fs::write(&src, msl)?;
    let run = |args: &[&str]| -> Result<()> {
        let out = Command::new("xcrun")
            .args(["-sdk", "macosx"])
            .args(args)
            .output()
            .context("run xcrun (is the Metal toolchain installed?)")?;
        if !out.status.success() {
            bail!(
                "{}",
                String::from_utf8_lossy(&out.stderr).lines().take(6).collect::<Vec<_>>().join("\n")
            );
        }
        Ok(())
    };
    let s = |p: &PathBuf| p.to_string_lossy().into_owned();
    run(&["metal", "-w", "-c", &s(&src), "-o", &s(&air)])?;
    run(&["metallib", &s(&air), "-o", &s(&lib)])?;
    let _ = std::fs::remove_file(&air);
    let _ = std::fs::remove_file(&src);
    Ok(std::fs::read(&lib)?)
}

/// Compiles many sources in parallel; failures are returned per hash (the runtime can
/// still compile those from MSL).
#[must_use]
pub fn compile_all(
    cache_dir: &Path,
    sources: &[(u64, &str)],
    threads: usize,
) -> HashMap<u64, Result<Vec<u8>, String>> {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads.max(1)).build();
    let job = || {
        sources
            .par_iter()
            .map(|(h, msl)| (*h, compile(cache_dir, *h, msl).map_err(|e| format!("{e:#}"))))
            .collect()
    };
    match pool {
        Ok(p) => p.install(job),
        Err(_) => job(),
    }
}
