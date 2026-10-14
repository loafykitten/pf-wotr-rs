//! POC-0 rendering spike. See README.md for the experiments and how to run them.

mod archive;
mod features;
mod gpu;
mod gpu_timing;
mod metalfx;
mod msl;
mod owlcat;
mod owlcat_bevy;
mod present;
mod scene;
mod stats;

fn usage() -> ! {
    eprintln!(
        "usage: render-spike <command> [options]\n\
         commands:\n  \
         features [--out FILE]      adapter features, limits, Metal counters, MetalFX support\n"
    );
    std::process::exit(2)
}

/// Value of `--name VALUE` in `args`, if present.
pub fn opt<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(String::as_str)
}

/// Whether `--name` appears in `args`.
pub fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first() else { usage() };
    let rest = &args[1..];
    let result = match cmd.as_str() {
        "features" => features::run(opt(rest, "--out")),
        "scene" => scene::run(rest),
        "owlcat" => owlcat::run(rest),
        "owlcat-bench" => owlcat::bench(rest),
        "metalfx" => metalfx::run(rest),
        "archive" => archive::run(rest),
        _ => usage(),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
