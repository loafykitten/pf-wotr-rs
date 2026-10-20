//! `wr-combat`: POC-5's headless combat tool (read-only; outputs under `cache/poc5/`).
//!
//! ```text
//! wr-combat hydrate <save>                 hydrate (placeholders become units), write the sheet
//!                                          dump, coverage and manifest
//! wr-combat compare <ours> <oracle>        compare a sheet dump with a WrathOracle state dump
//! ```

use std::rc::Rc;
use std::time::Instant;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(about = "wr-combat: headless turn-based combat (POC-5)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Hydrate a save and write `cache/poc5/<save>.*`.
    Hydrate {
        save: String,
        /// `AbilityParams.Empty` at the start of the oracle's load: `{"CasterLevel":n,
        /// "RankBonus":m}` or an oracle state dump with an `abilityParamsEmpty` member.
        #[arg(long)]
        ability_params_empty: Option<String>,
    },
    /// Compare our sheet dump with a WrathOracle `state-*.json` dump.
    Compare {
        ours: std::path::PathBuf,
        oracle: std::path::PathBuf,
        /// Only these unit ids (comma-separated).
        #[arg(long)]
        units: Option<String>,
    },
    /// Project an oracle trace to the T0 comparison stream (`cache/poc5/<name>.t0.jsonl`);
    /// with a second trace, diff the two projections.
    Project { trace: std::path::PathBuf, other: Option<std::path::PathBuf> },
    /// Driven replay of a turn-based recording: hydrate its save, fire each root rule from
    /// its snapshot on the recording's tape, diff the T0 projections.
    Replay {
        /// The oracle's fixture directory (`…/fixtures/<name>`).
        fixture: std::path::PathBuf,
        #[arg(long, default_value = "Manual_18_WR_FirstBattle")]
        save: String,
        /// Which run of the fixture (`record`, `replay-1`…).
        #[arg(long, default_value = "record")]
        run: String,
        /// Write the outputs here instead of `cache/poc5/`.
        #[arg(long)]
        out_dir: Option<std::path::PathBuf>,
        /// Also collect the units' presentation timeline (`replay::timeline`) and summarize it on
        /// stderr; stdout and the outputs are the same as without.
        #[arg(long)]
        timeline: bool,
        /// Collect the timeline and write it here (gzip JSON), for `wr-game --fight --timeline`.
        #[arg(long)]
        timeline_out: Option<std::path::PathBuf>,
    },
    /// Names a blueprint (type, name, its components' types).
    Bp { guid: String },
}

fn out_dir() -> std::path::PathBuf {
    let d = wr_paths::cache_root().join("poc5");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// One line per unit: its frames, the distance it walked, where its samples come from, the life
/// state at the end. With `WR_TL_CSV=DIR`, each unit's samples as `DIR/<id>.csv`.
fn timeline_summary(tl: &wr_sim::combat::replay::timeline::Timeline) {
    use wr_sim::combat::replay::timeline::Source;
    let sim = |t: &wr_sim::combat::replay::timeline::Track| {
        t.samples.iter().filter(|s| s.source == Source::Sim).count()
    };
    eprintln!(
        "timeline: {} frames ({}..={}), {} units, commands {:?}",
        tl.ticks.len(),
        tl.ticks.first().copied().unwrap_or(0),
        tl.ticks.last().copied().unwrap_or(0),
        tl.tracks.len(),
        tl.commands
    );
    if let Some(dir) = std::env::var_os("WR_TL_CSV") {
        let _ = std::fs::create_dir_all(&dir);
        for t in &tl.tracks {
            let mut text = String::from(
                "tick,x,y,z,o,d,ingame,life,prone,combat,moving,speed,aspeed,cmd,src\n",
            );
            for (i, s) in t.samples.iter().enumerate() {
                text.push_str(&format!(
                    "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{:?}\n",
                    tl.ticks[t.first + i],
                    s.position[0],
                    s.position[1],
                    s.position[2],
                    s.orientation,
                    s.desired_orientation,
                    s.in_game as u8,
                    s.life_state,
                    s.prone as u8,
                    s.in_combat as u8,
                    s.moving as u8,
                    s.speed,
                    s.agent_speed,
                    tl.command(s).unwrap_or("-"),
                    s.source
                ));
            }
            let _ = std::fs::write(std::path::Path::new(&dir).join(format!("{}.csv", t.id)), text);
        }
    }
    for t in &tl.tracks {
        let walked: f32 = t
            .samples
            .windows(2)
            .map(|p| {
                (p[1].position[0] - p[0].position[0]).hypot(p[1].position[2] - p[0].position[2])
            })
            .sum();
        let last = t.samples.last();
        eprintln!(
            "  {} frames {} (sim {}), walked {walked:.1} m, end life {} prone {}",
            t.id,
            t.samples.len(),
            sim(t),
            last.map_or(-1, |s| s.life_state),
            last.is_some_and(|s| s.prone)
        );
    }
}

fn read_json(p: &std::path::Path) -> Result<serde_json::Value> {
    let b = std::fs::read(p)?;
    Ok(serde_json::from_slice(&b)?)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Replay { fixture, save, run, out_dir, timeline, timeline_out } => {
            replay(
                &fixture,
                &save,
                &run,
                out_dir,
                timeline || timeline_out.is_some(),
                timeline_out.as_deref(),
            )?;
        }
        Cmd::Bp { guid } => {
            let content = wr_sim::content::Content::open_default()?;
            let g = wr_content::Guid::parse(&guid).ok_or_else(|| anyhow::anyhow!("bad guid"))?;
            let bp = content.get(g).ok_or_else(|| anyhow::anyhow!("no such blueprint"))?;
            println!("{} {}", bp.type_name(), bp.name);
            for c in bp.components_array() {
                println!("  {}", wr_content::schema::ty(c.ty).short);
            }
            if let Some(t) = bp.view().guid("m_Type").and_then(|g| content.get(g)) {
                println!(
                    "  weapon type {} category {} attack type {}",
                    t.name,
                    t.view().int("Category"),
                    t.view().int("m_AttackType")
                );
            }
        }
        Cmd::Project { trace, other } => {
            let t = wr_sim::combat::oracle::Trace::load(&trace)?;
            let a = wr_sim::combat::project::project_oracle(&t);
            let name = trace
                .file_name()
                .map(|s| s.to_string_lossy().replace(".trace.jsonl.gz", ""))
                .unwrap_or_default();
            let mut text = String::new();
            for r in &a {
                text.push_str(&r.to_string());
                text.push('\n');
            }
            std::fs::write(out_dir().join(format!("{name}.t0.jsonl")), text)?;
            println!("{}: {} records, {} projected", trace.display(), t.records.len(), a.len());
            if let Some(o) = other {
                let b = wr_sim::combat::project::project_oracle(
                    &wr_sim::combat::oracle::Trace::load(&o)?,
                );
                let d = wr_sim::combat::project::diff(&a, &b);
                println!("{}", serde_json::to_string_pretty(&d)?);
            }
        }
        Cmd::Hydrate { save, ability_params_empty } => {
            let t0 = Instant::now();
            let path = wr_save::report::find_save(&save)?;
            let s = wr_sim::hydrate::Save::read(&path)?;
            let content = Rc::new(wr_sim::content::Content::open_default()?);
            let registry = Rc::new(wr_sim::components::registry());
            let (mut w, report) = wr_sim::hydrate::hydrate(
                content.clone(),
                registry,
                &s,
                &wr_sim::hydrate::Options {
                    ability_params_empty: ability_params_empty
                        .as_deref()
                        .map(wr_sim::hydrate::parse_ability_params_empty)
                        .transpose()?
                        .flatten(),
                    ..Default::default()
                },
            )?;
            let units: Vec<wr_sim::world::EntityIdx> = (0..w.entities.len())
                .map(|i| wr_sim::world::EntityIdx(u32::try_from(i).unwrap_or(0)))
                .filter(|e| w.unit(*e).is_some())
                .collect();
            w.finish_run();
            let manifest = wr_sim::manifest::build(&w, &units);
            let sheet = wr_sim::sheet::dump(&w, &units, "after-load");
            let name = wr_save::report::short_name(&path);
            let dir = out_dir();
            std::fs::write(
                dir.join(format!("{name}.manifest.json")),
                serde_json::to_string_pretty(&manifest)?,
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
            });
            std::fs::write(
                dir.join(format!("{name}.coverage.json")),
                serde_json::to_string_pretty(&cov)?,
            )?;
            println!(
                "{name}: {} units, {} facts, valid={}, {} unsupported kinds, {:.0} ms",
                units.len(),
                report.facts,
                w.valid(),
                w.coverage.unsupported.len(),
                t0.elapsed().as_secs_f64() * 1e3
            );
        }
        Cmd::Compare { ours, oracle, units } => {
            let mut o = read_json(&ours)?;
            let mut t = read_json(&oracle)?;
            if let Some(list) = units {
                let keep: Vec<&str> = list.split(',').collect();
                for v in [&mut o, &mut t] {
                    if let Some(a) = v["units"].as_array_mut() {
                        a.retain(|u| u["id"].as_str().is_some_and(|id| keep.contains(&id)));
                    }
                }
            }
            let r = wr_sim::compare::compare(&o, &t);
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
            println!("missing ours: {:?}", r.units_missing_ours);
            println!("details: {}", out.display());
        }
    }
    Ok(())
}

fn replay(
    fixture: &std::path::Path,
    save: &str,
    run: &str,
    out_dir_arg: Option<std::path::PathBuf>,
    timeline: bool,
    timeline_out: Option<&std::path::Path>,
) -> Result<()> {
    let t0 = Instant::now();
    let (out, w) = if timeline {
        let (out, w, tl) =
            wr_sim::combat::replay::timeline::run_fixture_timeline(save, fixture, run)?;
        match tl {
            Ok(tl) => {
                timeline_summary(&tl);
                if let Some(p) = timeline_out {
                    tl.write(p)?;
                    eprintln!("timeline: written to {}", p.display());
                }
            }
            Err(e) => eprintln!("timeline: none: {e}"),
        }
        (out, w)
    } else {
        wr_sim::combat::replay::run_fixture(save, fixture, run)?
    };
    let load_unsupported = out.load_unsupported;
    let dir = out_dir_arg.map_or_else(out_dir, |d| {
        let _ = std::fs::create_dir_all(&d);
        d
    });
    let name = format!(
        "{}-{run}",
        fixture.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    );
    let dump = |v: &[serde_json::Value]| v.iter().map(|r| format!("{r}\n")).collect::<String>();
    std::fs::write(dir.join(format!("{name}.oracle.t0.jsonl")), dump(&out.oracle))?;
    std::fs::write(dir.join(format!("{name}.sim.t0.jsonl")), dump(&out.sim))?;
    println!(
        "{name}: oracle {} records, sim {} records, equal={}, {} roots driven, {:.0} ms",
        out.oracle.len(),
        out.sim.len(),
        out.diff.equal,
        out.roots,
        t0.elapsed().as_secs_f64() * 1e3
    );
    println!(
        "echoed (recorded, not computed): {:?}, {} records; unverified (computed with a recorded decision): {:?}, {} records; positions at impact [established, not]: {:?}",
        out.echoed,
        out.echoed_records,
        out.unverified,
        out.unverified_records,
        out.impact_positions
    );
    println!("presentation roots computed at their call site, by caller: {:?}", out.by_origin);
    println!(
        "folded into sim logic: {:?}  excluded: {:?}  skipped roots: {:?}",
        out.folded, out.excluded, out.skipped
    );
    println!(
        "awake states the sim couldn't establish, taken from the recording (unit-frames): {}",
        out.awake_recorded
    );
    println!(
        "cooldowns: inputs (recorded calls) {:?}; rows checked {}, taken from the recording {}",
        out.cooldown_inputs, out.cooldowns_checked, out.cooldowns_recorded
    );
    println!(
        "turn controller: ticks computed {}, turn ends computed {}",
        out.turn_ticks, out.turn_ends
    );
    println!(
        "commands: units ticked {}, runs {}, class code taken {}, ends computed {} (taken {}), SpendAction computed {}, turn reads checked {}; state checks {}, rows taken {}",
        out.commands_ticked,
        out.commands_run,
        out.commands_fx,
        out.commands_ends,
        out.commands_input_ends,
        out.commands_spends,
        out.commands_values,
        out.commands_checked,
        out.commands_recorded
    );
    println!(
        "UnitAttack: class code computed {} (each checked), rules fired {} (each checked)",
        out.attacks_calls, out.attacks_rules
    );
    println!(
        "approach: UnitMoveTo/UnitAttackOfOpportunity class code computed {}, attacks of opportunity fired {}, agent calls {}, draws {} (each checked)",
        out.approach_class, out.approach_rules, out.approach_calls, out.approach_draws
    );
    println!(
        "AI: entries {}, decisions {}, scores checked {}, commands run {}, draws {} (each checked)",
        out.ai_entries, out.ai_decisions, out.ai_scores, out.ai_runs, out.ai_draws
    );
    println!(
        "engagement: ticks {}, changes {}, attacks of opportunity {} (run {}), count writes {}, counts adopted {}, provokes {} (each checked)",
        out.eng_entries,
        out.eng_changes,
        out.eng_aoos,
        out.eng_runs,
        out.eng_count_writes,
        out.eng_counts_adopted,
        out.eng_provokes
    );
    println!("targets: writes {} (each checked), reads checked {}", out.tgt_checked, out.tgt_reads);
    println!(
        "impatience: entries {}, states checked {}, handlers matched {}, draws {}",
        out.imp_entries, out.imp_states, out.imp_handlers, out.imp_draws
    );
    println!(
        "memory: offensive actions' AddToMemory calls matched {}, commands' acts' matched {}",
        out.mem_adds, out.mem_act_adds
    );
    println!(
        "memory computed: ticks {}, reads checked {}, inputs {}, states checked {}, fills by unmodelled readers verified {}, changes matched {}; readers' reads checked {:?}",
        out.mem_ticks,
        out.mem_reads,
        out.mem_inputs,
        out.mem_rows,
        out.mem_fills,
        out.mem_changes,
        out.mem_checked
    );
    println!(
        "memory leaves: decided by the sim and compared with the recording's {:?}, of which differing {:?}; taken from the recording {:?}",
        out.mem_leaf_checked, out.mem_leaf_differs, out.mem_leaf_taken
    );
    if let Some(c) = &out.corpulence {
        println!(
            "corpulence computed: reads {} (unattributed {}), compared {} (equal {}, differing {}), unknown {} (of a unit the sim hasn't made {}, an event in the read's entry {}, by cause {:?})",
            c.reads,
            c.unattributed,
            c.equal + c.differ,
            c.equal,
            c.differ,
            c.unknown_total() + c.event,
            c.no_unit,
            c.event,
            c.unknown
        );
        if c.bounds_reads > 0 {
            println!(
                "corpulence against the entity-bounds colliders' radii: records of units {} (equal to Max(0.05, Corpulence) {}, within 0.05 {}, outside {}, not checked {})",
                c.bounds_reads, c.bounds_exact, c.bounds_band, c.bounds_differ, c.bounds_unchecked
            );
        }
    }
    if let Some(v) = &out.vision {
        let causes: Vec<String> = wr_sim::combat::corpulence::Cause::ALL
            .iter()
            .zip(v.range_corp_cause)
            .filter(|(_, n)| *n > 0)
            .map(|(c, n)| format!("{} {n}", c.name()))
            .collect();
        println!(
            "vision range corpulence: pairs left for the corpulence that are lost colliders' {} (of {}); unit-frames with the recording's read and the sim's value {} (differing {}), the sim's value used where the recording had no valid read {}, neither {} ({})",
            v.range_open_lost,
            v.range_open[0],
            v.range_corp_both,
            v.range_corp_disagree,
            v.range_corp_computed,
            v.range_corp_unknown,
            causes.join(", ")
        );
    }
    match (&out.vision, &out.vision_off) {
        (Some(v), _) => println!(
            "vision computed: engagement HasLOS reads {} (of which {} on entries made before the window), memory group.HasLOS reads {} ({} on such entries), AI InLos reads {}; sight cache misses {} (each equal to its record), hits {}, hits on entries made before the window {} (fresh computation equal {}, different {}), reads beyond vision range {}, clears {} (adopted {}), misses by readers the sim doesn't run verified {}; HasObstacle answers checked {}; vis records {}; the game's geometry dump: {} segments equal, {} probes equal; vision range: sets compared {} (changes {}, differences {}); everything below is decided given the window's start hypotheses (docs/poc5/vision.md) and the physics clock; with a pair left to the recording: sets {}, changes {}; pairs decided given the hypotheses and equal to the recording's {}, left to the recording by cause: corpulence {}, position {}, enabled {}, view-scale band {} (the answer at scale 1 equal {}, differing {}), result buffer {}; {} pairs at scale 1 of known one position (within 5 cm {}, within 1 cm {}, within 1 mm {}, closest {} um, f32 against exact differing {}); unit-frames of unknown corpulence {} (stale reads {}), of a fade-hidden collider {}; colliders made after the first frame {}, frames not followed {}, units moved by the sim after the phase {}, frames of unmodelled physics settings {}, frames with a possible explicit sync {}, views replaced by a polymorph {}, dummy view swaps {} (possible {}); vision records unconsumed {}; the memory's near-unit unions equal given the creation-order hypothesis {}",
            v.engage,
            v.engage_pre,
            v.memory,
            v.memory_pre,
            v.ai,
            v.misses,
            v.hits,
            v.pre_hits,
            v.pre_fresh_equal,
            v.pre_fresh_different,
            v.out_of_range,
            v.clears,
            v.adopted_clears,
            v.unmodelled_fills,
            v.obstacle_reads,
            v.vis_records,
            v.geometry_segments,
            v.probes,
            v.range_sets_compared,
            v.range_changes,
            v.range_differences,
            v.range_sets_open,
            v.range_changes_open,
            v.range_decided_given_start,
            v.range_open[0],
            v.range_open[1],
            v.range_open[2],
            v.range_open[3],
            v.range_guess_agree,
            v.range_guess_differ,
            v.range_open[4],
            v.range_pairs,
            v.range_within_5cm,
            v.range_within_1cm,
            v.range_within_1mm,
            v.range_closest_um,
            v.range_rounding,
            v.range_corp_unknown,
            v.range_corp_stale,
            v.range_fade_unit_frames,
            v.range_made_later,
            v.range_frames_lost,
            v.range_moved_after,
            v.range_physics_unknown,
            v.range_sync_frames,
            v.range_views_replaced,
            v.range_dummy_swaps,
            v.range_dummy_swaps_possible,
            v.records_unconsumed,
            v.near_unions_given_order
        ),
        (None, Some(why)) => {
            println!("vision: not computed ({why}); line of sight stays the recording's")
        }
        (None, None) => println!("vision: not computed (no engage observer or no vis records)"),
    }
    println!(
        "agent: entries {}, commands' calls {}, states checked {}, units ticked {}, requests checked {}, draws {}, synchronizations {}, getters checked {}, rows checked {}; inputs {:?}",
        out.ag_entries,
        out.ag_command_calls,
        out.ag_states,
        out.ag_ticks,
        out.ag_requests,
        out.ag_draws,
        out.ag_syncs,
        out.ag_reads,
        out.ag_rows,
        out.ag_inputs
    );
    let l = &out.look;
    println!(
        "orientation: DesiredOrientation reads computed {} (taken {}); writes computed {:?} (points taken {}; attack of opportunity's LookAt made {}, not made {}), writes by code the sim doesn't run {:?} (angles computed {}), taken in real time {}, snapshots differing {}",
        l.reads_checked,
        l.reads_taken,
        l.computed,
        l.points_taken,
        l.aoo_made,
        l.aoo_not_made,
        l.outside,
        l.outside_angles,
        l.real_time,
        l.snapshots_taken
    );
    let r = &out.roam;
    println!(
        "roaming: UnitRoamingController ticks {} (waiting {}), rolls {}, sleeping steps {}, LookAts {}, moves run {}, arrivals {}; states established {} (from roam_init: equal to the save's {}, advanced since the load {}), unknown {}",
        r.ticks,
        r.idle,
        r.rolls,
        r.steps,
        r.looks,
        r.moves,
        r.arrivals,
        r.established,
        r.init_equal,
        r.init_taken,
        r.unknown
    );
    let p = &out.prone;
    println!(
        "prone: UnitProneController unit ticks {}, falls {}, get-ups {}, last-frame units taken as asleep {}",
        p.ticks, p.falls, p.get_ups, p.assumed_asleep
    );
    let st = &out.stats;
    println!(
        "stat mutations in the T0 stream (docs/poc5/stats.md): compared {} {:?}, equal {}, differing {}, sim's extra {}; gaps {} {:?}",
        st.compared.values().sum::<usize>(),
        st.compared,
        st.equal,
        st.differing,
        st.extra,
        st.gaps.values().sum::<usize>(),
        st.gaps
    );
    let sp = &out.spans;
    println!(
        "inside root rules' spans (the commands' own apart): combat log events {} (compared with the sim's queue), impatience handlers {}, memory additions {} (matched), provokes {} (matched at the engagement tick), projectile launches {} (equal to the sim's); the oracle's own invalid notes {}; fact and activatable handlers equal to the sim's deliveries, one to one: {}; reported {}",
        sp.clog_q,
        sp.imp_s,
        sp.mem_q,
        sp.eng_prov,
        sp.proj_launch,
        sp.oracle_invalid,
        sp.handlers,
        sp.reported
    );
    let n = &out.nav;
    println!(
        "physics and paths: physics queries computed {} (agent {}, movement simulation {}, spans taken from the recording {}; taken {}), simulation steps {} (grid reads checked {}, taken {}); paths computed {} (line-of-sight answers taken {}), deliveries checked {}, paths the agent took from the sim {}; graph events matched {}, turn starts {}, removals {}, tag refreshes {} (navigation clock reads taken {}); place-on-ground steps computed {} (rays {})",
        n.phys_tick + n.phys_sim + n.phys_span,
        n.phys_tick,
        n.phys_sim,
        n.phys_span,
        n.phys_taken,
        n.sim_steps,
        n.sim_grid_checked,
        n.sim_grid_taken,
        n.paths,
        n.los_answers,
        n.deliveries,
        n.delivered_to_agent,
        n.graph_events,
        n.turn_starts,
        n.removals,
        n.refreshes,
        n.clock_reads,
        n.ground_steps,
        n.phys_ground
    );
    println!(
        "box centres: the recorded wcenter (TransformPoint(center)) equal to the physics file's, bit for bit: {}",
        n.box_centres
    );
    if n.corp_computed > 0 {
        println!(
            "paths: corpulences taken from the sim instead of the recording's reads {}",
            n.corp_computed
        );
    }
    if let Some(c) = &out.clog {
        println!(
            "combat log [oracle, sim]: queued {:?}, ticks {:?}, messages {:?}; messages equal {}, tooltips equal {}, tooltips not made {}, inputs {:?}, differences {}",
            c.queued,
            c.ticks,
            c.messages,
            c.messages_equal,
            c.tips_equal,
            c.tips_not_ported,
            c.inputs,
            c.differences
        );
    }
    println!("kinds [oracle, sim]: {:?}", out.diff.kinds);
    println!(
        "turn order (sort events, overrides/sequences taken from the recording): {:?}",
        out.sorts
    );
    // The longest equal prefix, by kind.
    if let Some(f) = out.diff.first_difference {
        let eq = out.oracle[..f].iter().filter(|r| r["k"] != "rule" && r["k"] != "result").count();
        println!("first difference at record {f} ({eq} draws/muts equal before it)");
        for (i, (a, b)) in out.diff.context.iter().enumerate() {
            let at = f.saturating_sub(5) + i;
            let mark = if a == b { " " } else { "*" };
            println!("{mark}{at:5} oracle {}", a.as_ref().map_or("-".into(), ToString::to_string));
            if a != b {
                println!("      sim    {}", b.as_ref().map_or("-".into(), ToString::to_string));
            }
        }
    }
    // The sheet at the end of the fight against the oracle's end state.
    let end_path = fixture.join(format!("state-end-{run}.json"));
    if end_path.exists() {
        let units: Vec<wr_sim::world::EntityIdx> = (0..w.entities.len())
            .map(|i| wr_sim::world::EntityIdx(u32::try_from(i).unwrap_or(0)))
            .filter(|e| w.unit(*e).is_some())
            .collect();
        let sheet = wr_sim::sheet::dump(&w, &units, "end");
        std::fs::write(
            dir.join(format!("{name}.state-end.json")),
            serde_json::to_string_pretty(&sheet)?,
        )?;
        let r = wr_sim::compare::compare(&sheet, &read_json(&end_path)?);
        println!(
            "end state vs the oracle's: units compared {}, match rate {:.4} (party {})",
            r.units_compared,
            r.match_rate,
            r.per_unit
                .iter()
                .filter(|(u, _)| ["35fffa0a", "558A", "5671", "5198"].iter().any(|p| u.contains(p)))
                .map(|(_, (e, t))| format!("{e}/{t}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let (flags, flag_diffs) = wr_sim::compare::compare_flags(&sheet, &read_json(&end_path)?);
        println!(
            "end-state flags (in_game, is_dead, is_conscious): {flags:?}; differences {flag_diffs:?}"
        );
        let (prone_n, prone_diffs) = wr_sim::compare::compare_prone(&sheet, &read_json(&end_path)?);
        println!(
            "end-state postures (prone): units compared {prone_n}; differences {prone_diffs:?}"
        );
        std::fs::write(
            dir.join(format!("{name}.compare-end.json")),
            serde_json::to_string_pretty(&r)?,
        )?;
    }
    println!(
        "valid={} rng failures={} load unsupported kinds={} replay unsupported kinds={}",
        w.valid(),
        w.rng.failures().len(),
        load_unsupported,
        w.coverage.unsupported.len().saturating_sub(load_unsupported)
    );
    for (k, v) in &w.coverage.unsupported {
        println!("  unsupported: {k} {v:?}");
    }
    for f in w.rng.failures().iter().take(5) {
        println!("  rng: {f:?}");
    }
    Ok(())
}
