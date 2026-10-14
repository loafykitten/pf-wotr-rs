//! `wr-content`: blueprint reports, content pack, canonical dumps.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use wr_content::source::BlueprintZip;
use wr_paths::GamePaths;

#[derive(Parser, Debug)]
#[command(about = "Blueprint content tools (POC-2)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Decodes every blueprint and writes the reports to cache/content/.
    Report {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Compares every zip blueprint with its `.bbp` record in canonical form.
    CompareBbp {
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Prints a blueprint's canonical form (from the zip, or the `.bbp` with `--bbp`).
    Canon {
        guid: String,
        #[arg(long)]
        bbp: bool,
    },
    /// Builds the content pack (cache/content/blueprints.wrcpack).
    PackBuild {
        #[arg(long)]
        out: Option<PathBuf>,
        /// Keep editor-only JSON properties in the pack.
        #[arg(long)]
        keep_extra: bool,
    },
    /// Measures the pack: open (warm and cold), readiness, GUID lookups.
    PackBench {
        #[arg(long)]
        pack: Option<PathBuf>,
        #[arg(long, default_value_t = 3)]
        cold_runs: usize,
    },
    /// Prints where opaque values are (path inside the zip, field path, reason).
    FindOpaque {
        #[arg(long, default_value_t = 5)]
        per_reason: usize,
    },
}

mod bench {
    //! Pack measurements.

    use std::path::Path;
    use std::time::Instant;

    use anyhow::Result;
    use serde::Serialize;
    use wr_content::pack::{self, Pack};

    #[derive(Debug, Serialize)]
    pub struct Stats {
        pub n: usize,
        pub p50_ns: u64,
        pub p99_ns: u64,
        pub max_ns: u64,
        pub mean_ns: u64,
    }

    fn stats(mut v: Vec<u64>) -> Stats {
        v.sort_unstable();
        let n = v.len();
        let pick = |q: f64| {
            #[allow(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss
            )]
            let i = ((n as f64 - 1.0) * q).round() as usize;
            v[i]
        };
        Stats {
            n,
            p50_ns: pick(0.5),
            p99_ns: pick(0.99),
            max_ns: v[n - 1],
            mean_ns: v.iter().sum::<u64>() / n as u64,
        }
    }

    fn ns(t: Instant) -> u64 {
        u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    #[derive(Debug, Serialize)]
    pub struct Cold {
        pub resident_after_copy: f64,
        /// `Pack::open`: mmap and header/section validation.
        pub open_us: f64,
        /// Open plus the first GUID lookup and payload access (ready to serve a request).
        pub ready_us: f64,
        /// 1,000 random lookups with payload access right after the cold open.
        pub first_1000_lookups_ms: f64,
    }

    #[derive(Debug, Serialize)]
    pub struct Report {
        pub pack_bytes: usize,
        pub blueprints: usize,
        pub warm_open: Stats,
        /// Open + `is_current` (stat of the zip) + one lookup: the content system ready.
        pub warm_ready: Stats,
        pub cold: Vec<Cold>,
        pub cold_method: &'static str,
        pub lookup_index_only: Stats,
        pub lookup_with_payload_access: Stats,
        pub lookup_with_deserialize: Stats,
        pub all_lookups_with_access_seconds: f64,
        pub timer_overhead_ns: u64,
    }

    pub fn run(path: &Path, zip: &Path, cold_runs: usize) -> Result<Report> {
        // Warm: touch everything once.
        // SAFETY: packs from `build` are never modified in place, and this tool doesn't.
        let p = unsafe { Pack::open(path) }?;
        let guids: Vec<_> = (0..p.len()).map(|i| p.entry(i).guid).collect();
        for g in &guids {
            let e = p.find(*g).expect("present");
            let _ = p.archived(&e)?;
        }
        let blueprints = p.len();
        let pack_bytes = p.bytes();
        drop(p);
        let mut opens = Vec::new();
        let mut readies = Vec::new();
        for i in 0..200 {
            let t = Instant::now();
            // SAFETY: packs from `build` are never modified in place, and this tool doesn't.
            let p = unsafe { Pack::open(path) }?;
            opens.push(ns(t));
            drop(p);
            let t = Instant::now();
            // SAFETY: packs from `build` are never modified in place, and this tool doesn't.
            let p = unsafe { Pack::open(path) }?;
            let _ = p.is_current(zip)?;
            let e = p.find(guids[(i * 7919) % guids.len()]).expect("present");
            let _ = p.archived(&e)?;
            readies.push(ns(t));
        }
        // Shuffled lookup order.
        let mut order: Vec<usize> = (0..guids.len()).collect();
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        for i in (1..order.len()).rev() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let j = usize::try_from(x % (i as u64 + 1)).unwrap_or(0);
            order.swap(i, j);
        }
        // SAFETY: packs from `build` are never modified in place, and this tool doesn't.
        let p = unsafe { Pack::open(path) }?;
        let mut overhead = Vec::new();
        for _ in 0..10_000 {
            let t = Instant::now();
            overhead.push(ns(t));
        }
        let timer_overhead_ns = stats(overhead).p50_ns;
        let mut idx = Vec::with_capacity(order.len());
        let mut acc = Vec::with_capacity(order.len());
        for &i in &order {
            let t = Instant::now();
            let e = p.find(guids[i]);
            idx.push(ns(t));
            std::hint::black_box(e);
        }
        for &i in &order {
            let t = Instant::now();
            let e = p.find(guids[i]).expect("present");
            let a = p.archived(&e)?;
            std::hint::black_box(a);
            acc.push(ns(t));
        }
        let mut de = Vec::new();
        for &i in order.iter().take(20_000) {
            let t = Instant::now();
            let e = p.find(guids[i]).expect("present");
            let v = p.load(&e)?;
            std::hint::black_box(v);
            de.push(ns(t));
        }
        let t = Instant::now();
        for &i in &order {
            let e = p.find(guids[i]).expect("present");
            std::hint::black_box(p.archived(&e)?);
        }
        let all = t.elapsed().as_secs_f64();
        drop(p);
        // Cold: an F_NOCACHE copy, opened before anything touches it.
        let mut cold = Vec::new();
        let tmp = path.with_extension("cold");
        for r in 0..cold_runs {
            let resident = pack::cold_copy(path, &tmp)?;
            let t = Instant::now();
            // SAFETY: our own fresh copy, not modified while open.
            let p = unsafe { Pack::open(&tmp) }?;
            let open_us = t.elapsed().as_secs_f64() * 1e6;
            let e = p.find(guids[(r * 104_729) % guids.len()]).expect("present");
            let _ = p.archived(&e)?;
            let ready_us = t.elapsed().as_secs_f64() * 1e6;
            let t2 = Instant::now();
            for k in 0..1000 {
                let e = p.find(guids[order[(r * 1000 + k) % order.len()]]).expect("present");
                std::hint::black_box(p.archived(&e)?);
            }
            cold.push(Cold {
                resident_after_copy: resident,
                open_us,
                ready_us,
                first_1000_lookups_ms: t2.elapsed().as_secs_f64() * 1e3,
            });
            drop(p);
            std::fs::remove_file(&tmp)?;
        }
        Ok(Report {
            pack_bytes,
            blueprints,
            warm_open: stats(opens),
            warm_ready: stats(readies),
            cold,
            cold_method: "pack copied with F_NOCACHE on both file descriptors (purge needs root); mincore confirms the copy's residency before opening",
            lookup_index_only: stats(idx),
            lookup_with_payload_access: stats(acc),
            lookup_with_deserialize: stats(de),
            all_lookups_with_access_seconds: all,
            timer_overhead_ns,
        })
    }
}

fn out_dir(o: Option<PathBuf>) -> PathBuf {
    o.unwrap_or_else(|| wr_paths::cache_root().join("content"))
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = GamePaths::from_env();
    match cli.cmd {
        Cmd::Report { out } => {
            let t = std::time::Instant::now();
            let zip = BlueprintZip::open(&paths.blueprints_zip())?;
            eprintln!("opened {} entries in {:.2}s", zip.len(), t.elapsed().as_secs_f64());
            let run = wr_content::report::run(&zip);
            eprintln!("decoded in {:.2}s", run.seconds);
            let dir = out_dir(out);
            run.write(&zip, &dir)?;
            let c = run.coverage();
            let r = run.references();
            eprintln!(
                "blueprints {} typed {} ({:.4}); opaque values {}; unknown fields {}; refs {} resolved {} dangling {} wrong {}",
                c.blueprints,
                c.blueprints_typed,
                c.blueprints_typed_share,
                c.opaque_values,
                c.unknown_fields_total,
                r.total,
                r.resolved,
                r.dangling,
                r.wrong_type
            );
            eprintln!("wrote {}", dir.display());
        }
        Cmd::CompareBbp { out } => {
            let zip = BlueprintZip::open(&paths.blueprints_zip())?;
            let bbp = wr_content::bbp::Bbp::open(&paths.bundles().join("blueprints-pack.bbp"))?;
            let c = wr_content::compare::compare(&zip, &bbp);
            let dir = out_dir(out);
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join("zip_vs_bbp.json"), serde_json::to_vec_pretty(&c)?)?;
            eprintln!(
                "compared {} equal {} different {}; bbp errors {} length mismatches {}; names differ {}; only zip {} only bbp {} ({:.1}s)",
                c.compared,
                c.equal,
                c.different,
                c.bbp_decode_errors,
                c.bbp_length_mismatches,
                c.name_mismatches,
                c.only_in_zip.len(),
                c.only_in_bbp.len(),
                c.seconds
            );
        }
        Cmd::Canon { guid, bbp } => {
            let g = wr_content::Guid::parse(&guid).ok_or_else(|| anyhow::anyhow!("bad GUID"))?;
            let zip = BlueprintZip::open(&paths.blueprints_zip())?;
            let mut hit = None;
            for (i, n) in zip.names().enumerate() {
                let b = zip.read(i)?;
                let mut t = wr_content::decode::Tally::default();
                let d = wr_content::decode::decode_jbp(&b, &mut t)?;
                if d.guid == Some(g) {
                    hit = Some((n.to_owned(), d));
                    break;
                }
            }
            let (path, d) = hit.ok_or_else(|| anyhow::anyhow!("not in the zip"))?;
            let stem = path.rsplit('/').next().unwrap_or(&path).trim_end_matches(".jbp").to_owned();
            if bbp {
                let p = wr_content::bbp::Bbp::open(&paths.bundles().join("blueprints-pack.bbp"))?;
                let off = p
                    .toc
                    .iter()
                    .find(|x| x.0 == g)
                    .map(|x| x.1)
                    .ok_or_else(|| anyhow::anyhow!("not in the pack"))?;
                let r = p.decode(off)?;
                let data = r.data.ok_or_else(|| anyhow::anyhow!("null record"))?;
                let opts = wr_content::canon::Options {
                    source: wr_content::canon::Source::Pack,
                    ..wr_content::canon::Options::default()
                };
                print!("{}", wr_content::canon::blueprint_with(g, &r.name, &data, opts));
            } else {
                let data = wr_content::decode::data(&d.wrapper)
                    .ok_or_else(|| anyhow::anyhow!("no Data"))?;
                print!("{}", wr_content::canon::blueprint(g, &stem, data));
            }
        }
        Cmd::PackBuild { out, keep_extra } => {
            let out = out.unwrap_or_else(|| out_dir(None).join("blueprints.wrcpack"));
            let opts = wr_content::pack::BuildOptions { keep_extra };
            let s = wr_content::pack::build(&paths.blueprints_zip(), &out, opts)?;
            eprintln!("{}", serde_json::to_string_pretty(&s)?);
            std::fs::write(out_dir(None).join("pack_build.json"), serde_json::to_vec_pretty(&s)?)?;
        }
        Cmd::PackBench { pack, cold_runs } => {
            let pack = pack.unwrap_or_else(|| out_dir(None).join("blueprints.wrcpack"));
            let r = bench::run(&pack, &paths.blueprints_zip(), cold_runs)?;
            println!("{}", serde_json::to_string_pretty(&r)?);
            std::fs::write(out_dir(None).join("pack_bench.json"), serde_json::to_vec_pretty(&r)?)?;
        }
        Cmd::FindOpaque { per_reason } => {
            let zip = BlueprintZip::open(&paths.blueprints_zip())?;
            let names: Vec<String> = zip.names().map(str::to_owned).collect();
            let found = zip.par_fold(Vec::new, |acc, _pos, name, bytes| {
                let Ok(b) = bytes else { return };
                let mut tally = wr_content::decode::Tally::default();
                let Ok(d) = wr_content::decode::decode_jbp(&b, &mut tally) else { return };
                if d.opaque == 0 {
                    return;
                }
                let mut path = String::new();
                wr_content::node::walk(&d.wrapper, &mut path, &mut |p, n| {
                    if let wr_content::Node::Opaque(o) = n {
                        let mut v = o.value.to_string();
                        v.truncate(300);
                        acc.push((format!("{:?}", o.reason), name.to_owned(), p.to_owned(), v));
                    }
                });
            });
            let _ = names;
            let mut by: std::collections::BTreeMap<String, Vec<(String, String, String)>> =
                std::collections::BTreeMap::new();
            for (r, n, p, v) in found.into_iter().flatten() {
                by.entry(r).or_default().push((n, p, v));
            }
            for (r, list) in by {
                println!("== {r} ({})", list.len());
                for (n, p, v) in list.iter().take(per_reason) {
                    println!("  {n} :: {p} :: {v}");
                }
            }
        }
    }
    Ok(())
}
