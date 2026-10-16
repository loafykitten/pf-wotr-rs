//! The driven replay's physics and turn-based paths (`docs/poc5/movement.md`, "Physics and paths
//! computed"): with the movement agent driven, the agent's physics queries are `wr-phys`'s
//! (the area's `cache/sim/phys/<guid>.wrphys`), each recorded `ag_phys` a check of the sim's
//! answer; the AI's and the path visualizer's movement simulation steps
//! (`SimulateTickMovementForTurnBased`, [`crate::combat::ai_sim`]) run from their recorded
//! simulator with the sim's physics and, with the paths computed, the sim's grid.
//!
//! With WrathOracle's `paths` declared, the turn-based grid is the sim's
//! ([`crate::combat::paths::NavRun`]: the turn controller's graph updates at its points, the
//! navigation timer's refresh) and each `ABPathWithClearance` the sim's agent requests
//! (`PathTo`, `FindPath`) is computed with `wr-nav` when the recording returns it (`path_fin`),
//! line of sight answered from `path_fin`'s transcript (an input: `LineOfSightGeometry` isn't
//! ported), and checked: the traced nodes, `error`, `CompleteState`, `searchedNodes`, the
//! seeker's modifiers (`path_mod`), each definition (`ag_pathdef`), the callback's
//! (`ag_done`), the AI's `ScorePath` read. The paths the agent follows are the sim's. The graph
//! updates the recording holds (`nav_guo`, `nav_update`, `nav_tags`) are paired in order with the
//! sim's ([`crate::combat::paths::CombatNav::log`]). What stays an input: when a path is
//! delivered (`PathSync`), line of sight, the units' corpulences, the Unity clock the navigation
//! timer reads, the path visualizer's replacement paths (the commands' follow), `FindPath`'s
//! arguments.

use std::collections::{HashMap, VecDeque};

use wr_nav::astar::{Ending, LosQuery, State};
use wr_nav::query::TurnBasedRequest;
use wr_nav::world::Kind;

use super::agent::{bits3, boolean, f, fields, v2, v3};
use super::commands::describe;
use super::{Driver, Phase, mode};
use crate::combat::agent::{PathRequest, V3};
use crate::combat::ai_sim::{self, MovementSimulator, SimInputs};
use crate::combat::commands::{Desync, R};
use crate::combat::oracle::Rv;
use crate::combat::paths::{self, CombatNav, NavEvent, parse_los};
use crate::csharp::arith::store_f32;
use crate::world::{EntityIdx, World};

/// What the sim computed of the physics and paths (the replay's coverage line).
#[derive(Debug, Default, Clone)]
pub struct NavCounts {
    /// Physics queries the sim answered (each checked against its `ag_phys`): the agent's
    /// (`Move` in `TickMovement`), the movement simulation's; queries taken from the recording
    /// (the physics file missing, an unverified collider: each reported).
    pub phys_tick: usize,
    pub phys_sim: usize,
    /// The place-on-ground step's rays (`ground_view`), and the steps computed.
    pub phys_ground: usize,
    pub ground_steps: usize,
    /// Queries inside spans taken from the recording (real time: the agent's code isn't run),
    /// answered by the sim from their recorded arguments and checked.
    pub phys_span: usize,
    pub phys_taken: usize,
    /// Movement simulation steps run (each checked against its `ag_sim_end`), the grid reads
    /// they made checked (`AllyBlocked`), taken (no grid: a capture without `paths`).
    pub sim_steps: usize,
    pub sim_grid_checked: usize,
    pub sim_grid_taken: usize,
    /// Paths computed (each checked against its `path_fin`), line-of-sight answers taken from
    /// the recording, deliveries checked (`path_mod`, `ag_pathdef`, `ag_done`, FindPath's
    /// callback, the AI's read), the paths the agent took from the sim.
    pub paths: usize,
    pub los_answers: usize,
    pub deliveries: usize,
    pub delivered_to_agent: usize,
    /// The grid: graph events paired with the recording's, turn starts, removals, tag refreshes
    /// the sim made; the navigation timer's clock reads (a recording's, inputs).
    pub graph_events: usize,
    pub turn_starts: usize,
    pub removals: usize,
    pub refreshes: usize,
    pub clock_reads: usize,
}

/// Where a physics query is made: in the agent's `Move` the sim runs, in a movement simulation
/// step it runs, or in a span taken from the recording (its origin the recording's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysCtx {
    Agent,
    Simulation,
    Span,
    /// `UnitPlaceOnGroundController.TickUnit`'s (`ground_view`).
    Ground,
    /// Another caller's (`ctx: "other"`: a critter's move, presentation): the sim doesn't use the
    /// answer, so one `wr-phys` can't give is taken, not reported; one it gives is checked.
    Other,
}

/// A recorded definition of a path: its points, `error`, `CompleteState`.
type Def = (Vec<V3>, bool, i64);

/// A request the sim's agent made, waiting for its search (`path_fin`).
#[derive(Debug, Clone)]
struct Pending {
    req: TurnBasedRequest,
    /// The grid's log length when the request was made (the grid mustn't change before the
    /// search: when it ran isn't recorded).
    log_len: usize,
    /// Definitions of the path recorded before its return (the traced nodes).
    defs: Vec<(Vec<V3>, bool, i64)>,
}

/// A computed path.
#[derive(Debug, Clone)]
struct Computed {
    raw: Vec<V3>,
    vector_path: Vec<V3>,
    err: bool,
    cs: i64,
    /// `Seeker.OnPathComplete`'s decision (`path_seek`): taken, modifiers, callbacks.
    seek: Option<(bool, bool, bool)>,
    /// The modifiers' result recorded (`path_mod`).
    modded: bool,
    /// The frame of its return (`Path.ReturnPath` runs the seeker, its modifiers and the
    /// callbacks there, synchronously).
    tick: i64,
}

impl Computed {
    /// The path its callbacks get: modified when the seeker took it and ran the modifiers on a
    /// path without error.
    fn delivered(&self) -> Option<&[V3]> {
        let (taken, mods, _) = self.seek?;
        if taken && mods && !self.err {
            self.modded.then_some(self.vector_path.as_slice())
        } else {
            Some(self.raw.as_slice())
        }
    }
}

/// The driven replay's physics and paths state.
#[derive(Default)]
pub struct NavDriver {
    /// The paths are the sim's (`paths` declared, the agent driven).
    pub paths: bool,
    /// The area's physics scene (loaded at the first query).
    phys: Option<Result<wr_phys::query::Scene<'static>, String>>,
    pending: HashMap<String, Pending>,
    computed: HashMap<String, Computed>,
    /// Paths the sim couldn't compute (reported): their deliveries take the recording's.
    failed: HashMap<String, ()>,
    /// The recording's graph events not yet paired, the sim's log paired so far, the
    /// recorded object ids to the sim's.
    rec_nav: VecDeque<usize>,
    log_at: usize,
    guo_ids: HashMap<i64, u32>,
    /// `m_NavmeshUpdateTimer` (the recording's `nav_timer0`), a recording's clock read waiting
    /// for its use, a replay's tape of its recording's reads (WrathOracle replays them in order).
    timer: Option<f32>,
    clock: Option<f32>,
    tape: Option<VecDeque<f32>>,
    /// The last `FindPath` request made (the AI's `ScorePath` reads its path next).
    last_find: Option<(String, EntityIdx)>,
    /// Each requester's seeker: its current path (`Seeker.path`, the last request it started),
    /// and the path each request replaced.
    seeker: HashMap<String, String>,
    replaced: HashMap<String, String>,
    /// Definitions of a path recorded before the sim registered its request (a worker can finish
    /// before `ag_req_end`): its kind, and its points with `error` and `CompleteState` when given.
    early: HashMap<String, Vec<(String, Option<Def>)>>,
    /// The last step the sim ran (`maxMoveLength`, its result, the simulator after it), not yet
    /// read by the AI.
    last_step: Option<(i64, EntityIdx, f32, bool, MovementSimulator)>,
    /// Each path's last return record (`path_fin`, `path_seek`, `path_mod`).
    returned_at: HashMap<String, usize>,
    /// The recording declares `ground_view`: the place-on-ground step is the sim's.
    pub ground_view: bool,
    /// The recorded `path_to`/`path_done` waiting for the sim's `PathTo`/`OnPathComplete`.
    path_to: Option<usize>,
    path_done: Option<usize>,
    /// Critters' requests (`path_critter`), and requests started inside spans taken from the
    /// recording.
    critters: std::collections::HashSet<String>,
    taken: std::collections::HashSet<String>,
}

/// A replay's recording's navigation timer and clock reads (`record.trace.jsonl.gz` beside its
/// trace: the begin record's `nav_timer0`, each `nav_clock`), which WrathOracle replays.
///
/// # Errors
///
/// The recording or its timer missing.
pub(crate) fn recording_clock(dir: &std::path::Path) -> Result<(f32, Vec<f32>), String> {
    use std::io::BufRead as _;
    let path = dir.join("record.trace.jsonl.gz");
    let file = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut timer = None;
    let mut reads = Vec::new();
    for (n, line) in std::io::BufReader::new(flate2::read::GzDecoder::new(file)).lines().enumerate()
    {
        let line = line.map_err(|e| format!("{}: {e}", path.display()))?;
        if n > 0 && !line.contains("\"nav_clock\"") {
            continue;
        }
        let r = Rv::from(
            serde_json::from_str::<serde_json::Value>(&line)
                .map_err(|e| format!("{}: {e}", path.display()))?,
        );
        if n == 0 {
            timer = f(&r["nav_timer0"]);
        } else if r["k"] == "obs" && r["ev"] == "nav_clock" {
            reads.push(f(&r["v"]).ok_or("a navigation clock read without its value")?);
        }
    }
    Ok((timer.ok_or("the recording's begin record has no nav_timer0")?, reads))
}

impl std::fmt::Debug for NavDriver {
    fn fmt(&self, fm: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fm.debug_struct("NavDriver")
            .field("paths", &self.paths)
            .field("pending", &self.pending.len())
            .field("computed", &self.computed.len())
            .finish_non_exhaustive()
    }
}

fn same_pts(a: &[V3], b: &[V3]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| bits3(*x) == bits3(*y))
}

fn pts(v: &Rv) -> Option<Vec<V3>> {
    v.as_array().and_then(|a| a.iter().map(v3).collect())
}

fn complete_state(s: State) -> i64 {
    match s {
        State::NotCalculated => 0,
        State::Error => 1,
        State::Complete => 2,
        State::Partial => 3,
    }
}

/// The request the sim's agent made, as `ABPathWithClearance.Construct` gets it.
fn tb_request(q: &PathRequest) -> TurnBasedRequest {
    let ending = match q.approach {
        Some(a) => Ending::Los {
            approach_radius: a,
            check_los: q.los,
            ignored_blocker_radius: q.ignore_blocker,
            max: q.max_distance,
        },
        None => Ending::MaxDistance { max: q.max_distance },
    };
    TurnBasedRequest {
        start: q.start,
        end: q.end,
        clearance: q.clearance,
        enabled_tags: q.tags,
        graph_mask: q.graph_mask,
        ending,
        distance_xz: q.distance_xz,
    }
}

/// A `ph_col` record of a mesh collider (observer `phys_probe`).
fn recorded_mesh(r: &J) -> Option<wr_phys::RecordedMesh> {
    let floats = |v: &J, n: usize| -> Option<Vec<f32>> {
        let a = v.as_array().filter(|a| a.len() == n)?;
        a.iter().map(f).collect()
    };
    let (p, q, l) = (floats(&r["pos"], 3)?, floats(&r["rot"], 4)?, floats(&r["lossy"], 3)?);
    Some(wr_phys::RecordedMesh {
        name: format!("{}:{} (MeshCollider)", r["scene"].as_str()?, r["path"].as_str()?),
        sibling_path: r["sib"]
            .as_array()?
            .iter()
            .map(|x| x.as_u64().and_then(|x| u32::try_from(x).ok()))
            .collect::<Option<_>>()?,
        present: r["active"].as_bool()? && r["enabled"].as_bool()?,
        layer: u32::try_from(r["layer"].as_u64()?).ok()?,
        position: [p[0], p[1], p[2]],
        rotation: [q[0], q[1], q[2], q[3]],
        lossy_scale: [l[0], l[1], l[2]],
        trigger: r["trigger"].as_bool()?,
        convex: r["convex"].as_bool()?,
        cooking: r["cooking"].as_i64()?,
    })
}

impl Driver<'_> {
    fn nav_unsupported(w: &mut World, what: impl std::fmt::Display) {
        w.coverage.unsupported(format!("driven replay: paths: {what}"));
    }

    /// The window's start, with the agent driven: the paths are the sim's when the recording
    /// declares `paths` (and the agent's synchronization); the area's nav file gives the grid,
    /// the recording each unit's corpulence (its reads, all equal) and the navigation timer.
    pub(super) fn nav_start(&mut self, w: &mut World) {
        let trace = self.trace;
        let begin = trace.records.iter().find(|r| r["k"] == "begin");
        self.ags.nav.ground_view = begin
            .and_then(|b| b["observers"].as_array())
            .is_some_and(|a| a.iter().any(|o| o == "ground_view"));
        // The global query settings `wr-phys` answers with (observer `phys_probe`): back faces
        // not hit (a mesh's would be; the terrain's rays come from above). Triggers fail closed
        // there either way.
        if begin
            .and_then(|b| b["observers"].as_array())
            .is_some_and(|a| a.iter().any(|o| o == "phys_probe"))
        {
            let settings: Vec<&Rv> = trace
                .records
                .iter()
                .filter(|r| r["k"] == "obs" && r["ev"] == "ph_settings")
                .collect();
            let ok = matches!(settings.as_slice(), [s] if s["hit_backfaces"] == false
                && s["hit_triggers"].is_boolean()
                && s["auto_sync"].is_boolean()
                && fields(s, &["hit_triggers", "hit_backfaces", "auto_sync"]));
            if !ok {
                w.coverage.unsupported(
                    "driven replay: physics: the query settings (ph_settings) aren't wr-phys's (back faces not hit)",
                );
            }
            // The meshes' transforms in the physics file, recorded from another run, are this
            // run's (`ph_col`).
            let recorded: Option<Vec<wr_phys::RecordedMesh>> = trace
                .records
                .iter()
                .filter(|r| r["k"] == "obs" && r["ev"] == "ph_col" && r["type"] == "MeshCollider")
                .map(recorded_mesh)
                .collect();
            let checked = match recorded {
                None => Err("a ph_col record without its fields".to_owned()),
                Some(cols) => match self.nav_scene(w) {
                    // A missing physics file is reported by the queries.
                    Err(_) => Ok(()),
                    Ok(scene) => scene.world().check_recorded_meshes(&cols),
                },
            };
            if let Err(e) = checked {
                w.coverage.unsupported(format!(
                    "driven replay: physics: the recorded colliders (ph_col) aren't the physics file's: {e}"
                ));
            }
        }
        let declared = begin
            .and_then(|b| b["observers"].as_array())
            .is_some_and(|a| a.iter().any(|o| o == "paths"));
        if !declared {
            // (Its records without its declaration: `paths_undeclared`, checked at the start.)
            return;
        }
        if !self.ags.strict {
            Self::nav_unsupported(w, "observer paths without the agent's synchronization");
            return;
        }
        self.ags.nav.paths = true;
        let nav = match w.player.area {
            None => Err("the save names no current area (its nav file)".to_owned()),
            Some(g) => wr_nav::NavFile::read(&wr_nav::path_for(&g.to_string()))
                .map_err(|e| format!("the area's nav file: {e:#}"))
                .and_then(|f| CombatNav::new(f.world).map_err(|e| format!("{e:#}"))),
        };
        match nav {
            Ok(n) => {
                w.combat.nav.node_size = n.world.grid.as_ref().map_or(0.0, |g| g.node_size);
                w.combat.nav.nav = Some(n);
                w.combat.nav.on = true;
            }
            Err(e) => Self::nav_unsupported(w, e),
        }
        // `Unit.Corpulence`: every recorded read of a unit's, in the trace's order (the commands' and
        // the AI's taps, the agent's request reads, whose unit is the request's).
        let mut req_unit: Option<String> = None;
        let mut corp: HashMap<String, Vec<(usize, f32)>> = HashMap::new();
        for (i, r) in trace.records.iter().enumerate() {
            if r["k"] != "obs" {
                continue;
            }
            let read = match r["ev"].as_str() {
                Some("ag_req") => {
                    req_unit = r["u"].as_str().map(str::to_owned);
                    None
                }
                Some("ag_req_end") => {
                    req_unit = None;
                    None
                }
                Some("uc_v") if r["f"] == "corp" => r["u"]
                    .as_str()
                    .map(str::to_owned)
                    .zip(r["v"].as_str().and_then(|v| v.parse::<f32>().ok()).or(Some(f32::NAN))),
                Some("ag_v") if r["f"] == "corp" && r.get("ctx").is_none() => r["u"]
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| req_unit.clone())
                    .zip(f(&r["v"]).or(Some(f32::NAN))),
                _ => None,
            };
            if let Some((u, v)) = read {
                corp.entry(u).or_default().push((i, v));
            }
        }
        w.combat.nav.corp =
            corp.into_iter().filter_map(|(id, v)| w.by_id.get(&id).map(|u| (*u, v))).collect();
        // `m_NavmeshUpdateTimer` at the window's start: a recording's begin record's; a replay's is
        // its recording's (WrathOracle sets it, and replays the recording's clock reads), which
        // its own trace doesn't hold (`prepare_world` reads them beside it).
        if self.replay_mode {
            match w.combat.nav.recording_clock.take() {
                Some(Ok((t, reads))) => {
                    self.ags.nav.timer = Some(t);
                    self.ags.nav.tape = Some(reads.into());
                }
                Some(Err(e)) => {
                    Self::nav_unsupported(w, format!("the recording's navigation clock: {e}"))
                }
                None => Self::nav_unsupported(
                    w,
                    "a replay without its recording's navigation timer and clock reads",
                ),
            }
        } else {
            match begin.and_then(|b| f(&b["nav_timer0"])) {
                Some(t) => self.ags.nav.timer = Some(t),
                None => Self::nav_unsupported(
                    w,
                    "a recording without its navigation timer (nav_timer0)",
                ),
            }
        };
    }

    /// A path whose recorded return isn't the sim's to compute: a critter's, one whose request
    /// was taken from the recording, one the sim couldn't compute (reported), outside synchronized
    /// turn-based combat or carried from real time.
    fn nav_exempt(&self, w: &World, key: &str) -> bool {
        self.ags.nav.critters.contains(key)
            || self.ags.nav.taken.contains(key)
            || self.ags.nav.failed.contains_key(key)
            || self.ags.carried.contains(key)
            || !(w.combat.turn_based && self.ags.synced)
    }

    /// A path whose request the window started (`path_start`) or carried from real time.
    fn nav_known(&self, key: &str) -> bool {
        self.ags.started.contains(key)
            || self.ags.carried.contains(key)
            || self.ags.nav.critters.contains(key)
    }

    /// A request started inside a span taken from the recording (`path_start` skipped): its
    /// path is the recording's.
    pub(super) fn nav_taken_request(&mut self, key: &str) {
        self.ags.nav.taken.insert(key.to_owned());
    }

    /// The grid is the sim's from the window's start and can be used.
    fn nav_live(w: &World) -> bool {
        w.combat.nav.on && !w.combat.nav.broken && w.combat.nav.nav.is_some()
    }

    /// The recording's graph events paired in order with the sim's.
    fn nav_match(&mut self, w: &mut World) {
        let trace = self.trace;
        loop {
            let Some(&j) = self.ags.nav.rec_nav.front() else { return };
            let Some(ev) =
                w.combat.nav.nav.as_ref().and_then(|n| n.log.get(self.ags.nav.log_at)).cloned()
            else {
                return;
            };
            self.ags.nav.rec_nav.pop_front();
            self.ags.nav.log_at += 1;
            self.out.nav.graph_events += 1;
            let r = &trace.records[j];
            let why = match (r["ev"].as_str().unwrap_or(""), &ev) {
                ("nav_guo", NavEvent::Queued { id, update: o }) => {
                    let mut why = self.nav_same_guo(r["id"].as_i64(), *id);
                    let flags: String = [
                        o.update_physics,
                        o.update_erosion,
                        o.modify_walkability,
                        o.set_walkability,
                        o.modify_dynamic_walkability,
                        o.set_dynamic_walkability,
                        o.modify_ally_blocked,
                        o.set_ally_blocked,
                        o.modify_tag,
                    ]
                    .iter()
                    .map(|&b| if b { '1' } else { '0' })
                    .collect();
                    if v3(&r["c"]).map(bits3) != Some(bits3(o.bounds.center))
                        || v3(&r["e"]).map(bits3) != Some(bits3(o.bounds.extents))
                        || r["mask"].as_i64() != Some(i64::from(o.graph_mask))
                        || r["f"] != flags.as_str()
                        || r["tag"].as_i64() != Some(i64::from(o.set_tag))
                        || r["pen"].as_i64() != Some(0)
                        || r["shape"] != false
                        || !fields(
                            r,
                            &["id", "c", "e", "mask", "f", "tag", "pen", "shape", "origin"],
                        )
                    {
                        why.get_or_insert(format!("the sim queued {o:?} (flags {flags})"));
                    }
                    why
                }
                ("nav_update", NavEvent::Applied { id, erosion }) => {
                    let mut why = self.nav_same_guo(r["id"].as_i64(), *id);
                    if r["erosion"].as_i64() != Some(i64::from(*erosion))
                        || !fields(r, &["id", "erosion"])
                    {
                        why.get_or_insert(format!("the sim erodes {erosion}"));
                    }
                    why
                }
                ("nav_tags", NavEvent::Tags { erode, only, hash, dynamic, ally }) => {
                    let ours = format!("{hash:016x}");
                    (r["erode"].as_i64() != Some(i64::from(*erode))
                        || r["only"].as_i64() != Some(i64::from(*only))
                        || r["hash"] != ours.as_str()
                        || r["dyn"].as_u64() != Some(*dynamic as u64)
                        || r["ally"].as_u64() != Some(*ally as u64)
                        || !fields(r, &["erode", "only", "hash", "dyn", "ally"]))
                    .then(|| {
                        format!(
                            "the sim's grid: erode {erode}, only {only}, hash {ours}, {dynamic} blocked, {ally} ally-blocked"
                        )
                    })
                }
                (k, ev) => Some(format!("{k} recorded where the sim has {ev:?}")),
            };
            if let Some(why) = why {
                Self::nav_unsupported(
                    w,
                    format!("a graph event differs from the sim's: {}: {why}", describe(r)),
                );
            }
        }
    }

    /// Recorded object ids map one to one onto the sim's.
    fn nav_same_guo(&mut self, theirs: Option<i64>, ours: u32) -> Option<String> {
        let Some(theirs) = theirs else { return Some("a graph update without its id".into()) };
        let ids = &mut self.ags.nav.guo_ids;
        match ids.get(&theirs) {
            Some(&o) if o == ours => None,
            Some(&o) => Some(format!("graph update {theirs} is the sim's {o}, not {ours}")),
            None if ids.values().any(|&o| o == ours) => {
                Some(format!("the sim's graph update {ours} recorded under a new id {theirs}"))
            }
            None => {
                ids.insert(theirs, ours);
                None
            }
        }
    }

    /// The grid's graph events and the recording's all paired (nothing pending on either side).
    fn nav_in_step(&mut self, w: &mut World) -> bool {
        self.nav_match(w);
        let ours = w.combat.nav.nav.as_ref().map_or(0, |n| n.log.len());
        self.ags.nav.rec_nav.is_empty() && self.ags.nav.log_at == ours
    }

    /// A recorded `nav_*` or `path_*` record of observer `paths` (the agent driven).
    pub(super) fn nav_record(&mut self, w: &mut World, i: usize) {
        let trace = self.trace;
        let r = &trace.records[i];
        let ev = r["ev"].as_str().unwrap_or("");
        // (A capture without `paths` has none of them.)
        if !self.ags.nav.paths {
            return;
        }
        if r["t"].as_i64() != Some(self.tick) {
            Self::nav_unsupported(w, format!("a record of another frame ({})", describe(r)));
            return;
        }
        match ev {
            "nav_guo" | "nav_update" => {
                self.ags.nav.rec_nav.push_back(i);
                self.nav_match(w);
            }
            "nav_tags" => {
                self.ags.nav.rec_nav.push_back(i);
                self.nav_match(w);
            }
            "nav_clock" => self.nav_clock(w, r),
            // A critter's request (`CritterMoveAgent`: presentation, its path not the sim's).
            // (`PathSync` names only identified critters `critter#<n>`, each request once.)
            "path_critter" => match r["path"].as_str() {
                Some(k)
                    if fields(r, &["path", "e"])
                        && v3(&r["e"]).is_some()
                        && k.strip_prefix("critter#").is_some_and(|n| n.parse::<u32>().is_ok())
                        && !self.nav_known(k) =>
                {
                    self.ags.nav.critters.insert(k.to_owned());
                }
                _ => Self::nav_unsupported(
                    w,
                    format!("a critter's request it can't read, or a second ({})", describe(r)),
                ),
            },
            // `UnitMovementAgent.PathTo`'s and `OnPathComplete(Path)`'s prefixes (observer
            // `Observe`): checked by the sim's call that follows, in turn-based combat.
            "path_to" | "path_done" if w.combat.turn_based && self.ags.synced => {
                let slot = if ev == "path_to" {
                    &mut self.ags.nav.path_to
                } else {
                    &mut self.ags.nav.path_done
                };
                if slot.replace(i).is_some() {
                    Self::nav_unsupported(w, format!("a {ev} without the sim's call"));
                }
            }
            "path_to" | "path_done" => {}
            "path_fin" | "path_seek" | "path_mod" => {
                match ev {
                    "path_fin" => self.nav_path_fin(w, r),
                    "path_seek" => self.nav_path_seek(w, r),
                    _ => self.nav_path_mod(w, r),
                }
                // (A callback's prefix comes after its return's records.)
                if let Some(k) = r["path"].as_str() {
                    self.ags.nav.returned_at.insert(k.to_owned(), i);
                }
            }
            _ => {}
        }
    }

    /// `CombatController.Tick`'s `UpdateNavigationGrid` (in its tick, after the units-changed
    /// part: in the default mode, in combat, not waiting for the UI): no navmesh cut; while the
    /// timer isn't negative it subtracts `Time.unscaledTime` (a recording's read, an input) and
    /// refreshes the tags once it runs out.
    pub(super) fn nav_grid_tick(&mut self, w: &mut World) {
        if !self.ags.nav.paths {
            return;
        }
        let m = self.draining.or(self.frame.mode);
        let runs = self.turn_go_on
            && m == Some(mode::DEFAULT)
            && w.combat.combat_active
            && w.combat.turns.waiting_for_ui == 0;
        let Some(t) = self.ags.nav.timer else { return };
        if !runs || t < 0.0 {
            return;
        }
        let read = if self.replay_mode {
            self.ags.nav.tape.as_mut().and_then(VecDeque::pop_front)
        } else {
            self.ags.nav.clock.take()
        };
        let Some(v) = read else {
            Self::nav_unsupported(
                w,
                "UpdateNavigationGrid reads the clock (its timer isn't negative); the recording has no read",
            );
            // (Once: the timer's state isn't known from here.)
            self.ags.nav.timer = Some(-1.0);
            return;
        };
        self.out.nav.clock_reads += 1;
        // `m_NavmeshUpdateTimer -= Time.unscaledTime` (a field store: one rounding).
        let t = store_f32(f64::from(t) - f64::from(v));
        self.ags.nav.timer = Some(t);
        if t <= 0.0 {
            paths::refresh_tags(w);
            self.nav_match(w);
        }
    }

    /// A recording's clock read (`nav_clock`): `UpdateNavigationGrid`'s, in the turn controller's
    /// tick.
    fn nav_clock(&mut self, w: &mut World, r: &Rv) {
        if self.replay_mode {
            Self::nav_unsupported(w, "a replay recorded a navigation clock read");
            return;
        }
        let Some(v) = f(&r["v"]).filter(|_| fields(r, &["v"])) else {
            Self::nav_unsupported(w, "a navigation clock read it can't read");
            return;
        };
        self.ags.nav.clock = Some(v);
        self.place(w, Phase::Turn, "a navigation clock read");
        if self.ags.nav.clock.take().is_some() {
            Self::nav_unsupported(
                w,
                "a navigation clock read the sim's UpdateNavigationGrid doesn't make",
            );
        }
    }

    /// The grid's erosion settings `ConstructTags` reads (`erodeIterations`, `erosionFirstTag`)
    /// when the grid is the sim's.
    pub(super) fn nav_erosion(&self, w: &World) -> Option<Option<(i32, i32)>> {
        if !self.ags.nav.paths {
            return None;
        }
        Some(
            w.combat
                .nav
                .nav
                .as_ref()
                .and_then(|n| n.world.grid.as_ref())
                .map(|g| (g.erode_iterations, g.erosion_first_tag)),
        )
    }

    /// A request the sim's agent made (`ag_req_end`'s): its search waits for the recording's
    /// return (`path_fin`, with the line-of-sight answers).
    pub(super) fn nav_request(&mut self, w: &mut World, u: EntityIdx, q: &PathRequest, find: bool) {
        if find {
            self.ags.nav.last_find = Some((q.key.clone(), u));
        }
        if !self.ags.nav.paths {
            return;
        }
        if !Self::nav_live(w) {
            Self::nav_unsupported(w, format!("{}: the grid isn't the sim's", q.key));
            self.ags.nav.failed.insert(q.key.clone(), ());
            return;
        }
        if !self.nav_in_step(w) {
            Self::nav_unsupported(
                w,
                format!(
                    "{}: requested while the grid's updates differ from the recording's",
                    q.key
                ),
            );
        }
        // `StartPathInternal` fails the seeker's previous path if it is still in the queue or being
        // searched: whether a worker had finished it isn't recorded.
        if let Some(old) = self.ags.nav.replaced.get(&q.key).cloned()
            && self.ags.nav.pending.contains_key(&old)
        {
            Self::nav_unsupported(
                w,
                format!(
                    "{}: requested while the seeker's previous path {old} is in flight (whether it was canceled isn't recorded)",
                    q.key
                ),
            );
            self.ags.nav.pending.remove(&old);
            self.ags.nav.failed.insert(old, ());
        }
        let log_len = w.combat.nav.nav.as_ref().map_or(0, |n| n.log.len());
        let mut defs = Vec::new();
        for (kind, def) in self.ags.nav.early.remove(&q.key).unwrap_or_default() {
            if kind != "ABPathWithClearance" {
                Self::nav_unsupported(w, format!("{}: a definition of a {kind} path", q.key));
            }
            defs.extend(def);
        }
        let p = Pending { req: tb_request(q), log_len, defs };
        if self.ags.nav.pending.insert(q.key.clone(), p).is_some()
            || self.ags.nav.computed.contains_key(&q.key)
        {
            Self::nav_unsupported(w, format!("{}: requested twice", q.key));
        }
    }

    /// A request started (`PathSync`'s `path_start`, `<requester>#<n>`): the requester's seeker's
    /// current path from here (`StartPathInternal`'s `path = p`).
    pub(super) fn nav_seeker_start(&mut self, key: &str) {
        if let Some((who, _)) = key.rsplit_once('#')
            && let Some(old) = self.ags.nav.seeker.insert(who.to_owned(), key.to_owned())
        {
            self.ags.nav.replaced.insert(key.to_owned(), old);
        }
    }

    /// `StartTurn`'s graph part, between its recorded entry and return: the grid's events before
    /// it all paired (none of the turn's recorded early), then the sim's.
    pub(super) fn nav_start_turn(&mut self, w: &mut World, rider: EntityIdx) {
        if self.ags.nav.paths && !w.combat.nav.broken && !self.nav_in_step(w) {
            Self::nav_unsupported(w, "graph events out of step at StartTurn's entry");
        }
        paths::start_turn(w, rider);
    }

    /// `StartTurn`'s return (`turn_start`): its graph events all recorded inside it.
    pub(super) fn nav_turn_started(&mut self, w: &mut World) {
        if self.ags.nav.paths && !w.combat.nav.broken && !self.nav_in_step(w) {
            Self::nav_unsupported(w, "graph events out of step at StartTurn's return");
        }
    }

    /// A `FindPath` starts: the AI's read that follows is of its path only if it makes one.
    pub(super) fn nav_find_start(&mut self) {
        self.ags.nav.last_find = None;
    }

    /// `Path.ReturnPath` (`path_fin`): a request of the sim's agent is searched now, on the grid as
    /// it was when requested, with the recorded line-of-sight answers, and checked.
    fn nav_path_fin(&mut self, w: &mut World, r: &Rv) {
        let Some(key) = r["path"].as_str().map(str::to_owned) else {
            Self::nav_unsupported(w, "a path_fin it can't read");
            return;
        };
        let Some(p) = self.ags.nav.pending.remove(&key) else {
            if self.ags.nav.failed.contains_key(&key) || self.ags.nav.computed.contains_key(&key) {
                if self.ags.nav.computed.contains_key(&key) {
                    Self::nav_unsupported(w, format!("{key}: returned twice"));
                }
                return;
            }
            // A request of real time (taken) or of code the sim doesn't run.
            if !self.nav_known(&key) {
                Self::nav_unsupported(w, format!("{key}: a path no request started"));
            } else if self.ags.nav.critters.contains(&key) {
                self.ag_input("a critter's path (presentation)");
            } else if self.ags.nav.taken.contains(&key) {
                self.ag_input("a path whose request was taken from the recording");
            } else if w.combat.turn_based && self.ags.synced && !self.ags.carried.contains(&key) {
                // (Whatever its kind: in turn-based combat the sim's agent makes every request.)
                Self::nav_unsupported(
                    w,
                    format!("{key}: a turn-based path the sim didn't request"),
                );
            } else {
                self.ag_input("a path the sim didn't request (real time, or carried from it)");
            }
            return;
        };
        // In a recording the return follows its delivery (`PathSync`'s `path_ret`; a replay's
        // trace has none): the search ran before it, on the grid as it was then.
        if !self.replay_mode && self.ags.last_ret.as_deref() != Some(key.as_str()) {
            Self::nav_unsupported(
                w,
                format!("{key}: a return (path_fin) without its delivery (path_ret)"),
            );
        }
        match self.nav_compute(w, r, &p) {
            Ok(c) => {
                self.out.nav.paths += 1;
                self.ags.nav.computed.insert(key, c);
            }
            Err(why) => {
                Self::nav_unsupported(w, format!("{key}: {why}"));
                self.ags.nav.failed.insert(key, ());
            }
        }
    }

    fn nav_compute(&mut self, w: &mut World, r: &Rv, p: &Pending) -> Result<Computed, String> {
        if !fields(r, &["path", "kind", "err", "cs", "searched", "end", "raw", "los"]) {
            return Err(format!("a path_fin it can't read ({})", describe(r)));
        }
        if !Self::nav_live(w) {
            return Err("the grid isn't the sim's".into());
        }
        let nav = w.combat.nav.nav.as_ref().expect("live");
        if nav.log.len() != p.log_len {
            return Err(
                "the grid changed while the path was in flight (when its search ran isn't recorded)"
                    .into(),
            );
        }
        if r["kind"] != "ABPathWithClearance" {
            return Err(format!("a {} path (the sim's request is ABPathWithClearance)", r["kind"]));
        }
        let answers = parse_los(&r["los"])?;
        let world = &nav.world;
        let compute = |req: &TurnBasedRequest| {
            let mut at = 0;
            let mut mismatch: Option<String> = None;
            let mut los = |q: &LosQuery| {
                let a = answers.get(at)?;
                at += 1;
                if !(bits3(a.from) == bits3(q.from)
                    && bits3(a.to) == bits3(q.to)
                    && a.fudge.to_bits() == q.fudge.to_bits())
                {
                    mismatch.get_or_insert(format!("line of sight {q:?}, the game asked {a:?}"));
                }
                Some(a.blocked)
            };
            let got = world.turn_based_path(req, &mut los);
            if at != answers.len() && mismatch.is_none() {
                mismatch =
                    Some(format!("{at} line-of-sight calls, the game made {}", answers.len()));
            }
            (got, mismatch, at)
        };
        let (got, mismatch, asked) = compute(&p.req);
        if let Some(m) = mismatch {
            return Err(m);
        }
        if let Some(q) = got.search.los_unknown {
            return Err(format!("line of sight asked past the recorded answers ({q:?})"));
        }
        // `PathTo` sets `distanceXZ` after `StartPath`: a worker may have searched without it.
        if p.req.distance_xz {
            let (alt, alt_mismatch, alt_asked) =
                compute(&TurnBasedRequest { distance_xz: false, ..p.req });
            if !same_pts(&alt.vector_path, &got.vector_path)
                || !same_pts(&alt.raw, &got.raw)
                || alt.search.state != got.search.state
                || alt.search.searched_nodes != got.search.searched_nodes
                || alt.search.end_node != got.search.end_node
                || alt.search.los_unknown.is_some()
                || alt_mismatch.is_some()
                || alt_asked != asked
            {
                return Err("the result depends on when PathTo set distanceXZ".into());
            }
        }
        let err = got.search.state == State::Error;
        let cs = complete_state(got.search.state);
        let raw = pts(&r["raw"]).ok_or("path_fin without its raw path")?;
        // `endNode`'s `NodeInGridIndex` (null without a grid end node).
        let end = got.search.end_node.and_then(|n| match world.kind(n) {
            Kind::Cell(c) => i64::try_from(c).ok(),
            Kind::Tri(_) => None,
        });
        let end_same = match end {
            Some(e) => r["end"].as_i64() == Some(e),
            None => r["end"].is_null(),
        };
        if !end_same {
            return Err(format!("the end node {end:?}, the game's {}", r["end"]));
        }
        if !same_pts(&raw, &got.raw)
            || r["err"].as_bool() != Some(err)
            || r["cs"].as_i64() != Some(cs)
            || r["searched"].as_i64() != Some(i64::from(got.search.searched_nodes))
        {
            return Err(format!(
                "the sim's path (error {err}, state {cs}, {} searched) {:?}; the game's {}",
                got.search.searched_nodes,
                got.raw,
                describe(r)
            ));
        }
        for (v, e, c) in &p.defs {
            if !same_pts(v, &got.raw) || *e != err || *c != cs {
                return Err(format!(
                    "a definition before its return differs from the sim's: {v:?}"
                ));
            }
            self.out.nav.deliveries += 1;
        }
        self.out.nav.los_answers += asked;
        Ok(Computed {
            raw: got.raw,
            vector_path: got.vector_path,
            err,
            cs,
            seek: None,
            modded: false,
            tick: self.tick,
        })
    }

    /// `Seeker.OnPathComplete` (`path_seek`): whether the seeker takes the path, runs its
    /// modifiers and calls back.
    fn nav_path_seek(&mut self, w: &mut World, r: &Rv) {
        let Some(key) = r["path"].as_str() else {
            Self::nav_unsupported(w, "a path_seek it can't read");
            return;
        };
        if self.ags.nav.pending.contains_key(key) {
            Self::nav_unsupported(w, format!("{key}: path_seek before its return"));
            return;
        }
        // `Seeker.OnPathComplete(Path)`: `OnPathComplete(p, runModifiers: true, sendCallbacks:
        // true)`; it takes the path if it is the seeker's current one.
        let current = key
            .rsplit_once('#')
            .is_some_and(|(who, _)| self.ags.nav.seeker.get(who).map(String::as_str) == Some(key));
        let known = self.nav_known(key);
        let tick = self.tick;
        let exempt = self.nav_exempt(w, key);
        let Some(c) = self.ags.nav.computed.get_mut(key) else {
            if !known {
                Self::nav_unsupported(
                    w,
                    format!("{key}: a path_seek of a path no request started"),
                );
            } else if !exempt {
                Self::nav_unsupported(w, format!("{key}: a path_seek the sim didn't request"));
            }
            return;
        };
        if c.tick != tick {
            Self::nav_unsupported(w, format!("{key}: path_seek outside its return's frame"));
        }
        match (r["accepted"].as_bool(), r["mods"].as_bool(), r["cb"].as_bool()) {
            (Some(a), Some(m), Some(cb))
                if c.seek.is_none() && fields(r, &["path", "accepted", "mods", "cb"]) =>
            {
                if (a, m, cb) != (current, true, true) {
                    Self::nav_unsupported(
                        w,
                        format!(
                            "{key}: the seeker's decision ({a}, {m}, {cb}) isn't the sim's ({current}, true, true)"
                        ),
                    );
                }
                c.seek = Some((current, true, true));
            }
            _ => Self::nav_unsupported(w, format!("{key}: a path_seek it can't read, or a second")),
        }
    }

    /// The seeker's modifiers done (`path_mod`): the sim's modified path.
    fn nav_path_mod(&mut self, w: &mut World, r: &Rv) {
        let Some(key) = r["path"].as_str() else {
            Self::nav_unsupported(w, "a path_mod it can't read");
            return;
        };
        if self.ags.nav.pending.contains_key(key) {
            Self::nav_unsupported(w, format!("{key}: path_mod before its return"));
            return;
        }
        let known = self.nav_known(key);
        let tick = self.tick;
        let exempt = self.nav_exempt(w, key);
        let Some(c) = self.ags.nav.computed.get_mut(key) else {
            if !known {
                Self::nav_unsupported(w, format!("{key}: a path_mod of a path no request started"));
            } else if !exempt {
                Self::nav_unsupported(w, format!("{key}: a path_mod the sim didn't request"));
            }
            return;
        };
        if c.tick != tick {
            Self::nav_unsupported(w, format!("{key}: path_mod outside its return's frame"));
        }
        let expected = c.seek.is_some_and(|(a, m, _)| a && m && !c.err) && !c.modded;
        let same =
            pts(&r["v"]).is_some_and(|v| same_pts(&v, &c.vector_path)) && fields(r, &["path", "v"]);
        if !expected || !same {
            let ours = c.vector_path.clone();
            Self::nav_unsupported(
                w,
                format!(
                    "{key}: the seeker's modifiers ({}expected): the sim's {ours:?}, the game's {}",
                    if expected { "" } else { "not " },
                    r["v"]
                ),
            );
            self.ags.nav.computed.remove(key);
            self.ags.nav.failed.insert(key.to_owned(), ());
            return;
        }
        c.modded = true;
        self.out.nav.deliveries += 1;
    }

    /// A path's definition (`ag_pathdef` with its points): of a request the sim computes, the
    /// traced nodes before the modifiers ran, the delivered path after. Returns whether it is one.
    pub(super) fn nav_def(
        &mut self,
        w: &mut World,
        key: &str,
        kind: &str,
        def: Option<(&[V3], bool, i64)>,
    ) -> bool {
        if !self.ags.nav.paths {
            return false;
        }
        // The sim's requests are `ABPathWithClearance`s.
        let ours =
            self.ags.nav.pending.contains_key(key) || self.ags.nav.computed.contains_key(key);
        if !ours {
            // Before its request is registered (or of a request the sim doesn't make): kept for
            // its registration.
            self.ags
                .nav
                .early
                .entry(key.to_owned())
                .or_default()
                .push((kind.to_owned(), def.map(|(v, e, c)| (v.to_vec(), e, c))));
            return false;
        }
        if kind != "ABPathWithClearance" {
            Self::nav_unsupported(w, format!("{key}: a definition of a {kind} path"));
        }
        let Some((v, err, cs)) = def else {
            // The observer writes a pending definition only while the path is queued or searched.
            if self.ags.nav.computed.contains_key(key) {
                Self::nav_unsupported(w, format!("{key}: a pending definition after its return"));
            }
            return true;
        };
        if let Some(p) = self.ags.nav.pending.get_mut(key) {
            p.defs.push((v.to_vec(), err, cs));
            return true;
        }
        let Some(c) = self.ags.nav.computed.get(key) else { return false };
        let want = if c.modded { c.vector_path.as_slice() } else { c.raw.as_slice() };
        if !same_pts(v, want) || err != c.err || cs != c.cs {
            Self::nav_unsupported(
                w,
                format!("{key}: a definition differs from the sim's path {want:?}: {v:?}"),
            );
        } else {
            self.out.nav.deliveries += 1;
        }
        true
    }

    /// The path a delivery's callback gets (`ag_done`'s, or FindPath's): the sim's, checked
    /// against the recorded one (`None`: a path the sim doesn't compute, the recording's taken).
    pub(super) fn nav_delivered(
        &mut self,
        key: &str,
        recorded: Option<(&[V3], bool, i64, &str)>,
    ) -> R<Option<Vec<V3>>> {
        if !self.ags.nav.paths || self.ags.nav.failed.contains_key(key) {
            return Ok(None);
        }
        if self.ags.nav.pending.contains_key(key) {
            return Err(Desync(format!("{key} delivered before its return (path_fin)")));
        }
        let Some(c) = self.ags.nav.computed.get(key) else { return Ok(None) };
        if c.tick != self.tick {
            return Err(Desync(format!("{key}: a callback outside its return's frame")));
        }
        if !c.seek.is_some_and(|(a, _, cb)| a && cb) {
            return Err(Desync(format!(
                "{key}: a callback the seeker doesn't make ({:?})",
                c.seek
            )));
        }
        let Some(v) = c.delivered().map(<[V3]>::to_vec) else {
            return Err(Desync(format!("{key}: delivered before its modifiers (path_mod)")));
        };
        if let Some((pts, err, cs, kind)) = recorded
            && (!same_pts(pts, &v) || err != c.err || cs != c.cs || kind != "ABPathWithClearance")
        {
            return Err(Desync(format!(
                "{key}: the delivered path differs from the sim's {v:?} (error {}, state {}): {pts:?}",
                c.err, c.cs
            )));
        }
        self.out.nav.deliveries += 1;
        Ok(Some(v))
    }

    /// The AI's read of `ScorePath`'s path (after `FixPathIfTooShort`): unless it made a fake
    /// one, the sim's path of its `FindPath`.
    pub(super) fn nav_ai_path(
        &mut self,
        w: &World,
        fake: bool,
        error: bool,
        state: i64,
        points: &[V3],
    ) -> R<()> {
        if !self.ags.nav.paths {
            return Ok(());
        }
        // (Each `ScorePath` reads the path of its own `FindPath`, once.)
        let Some((key, unit)) = self.ags.nav.last_find.take() else {
            return Err(Desync("ScorePath's path without the sim's FindPath".into()));
        };
        // (`context.Unit.View.AgentASP.FindPath`: the AI's own unit's.)
        if crate::combat::ai::context_unit(w) != Some(unit) {
            return Err(Desync(format!("ScorePath's path {key} isn't the AI's unit's request")));
        }
        if self.ags.nav.failed.contains_key(&key) {
            return Ok(());
        }
        let Some(c) = self.ags.nav.computed.get(&key) else {
            return Err(Desync(format!("ScorePath's path {key} isn't computed")));
        };
        // (`BlockUntilCalculated`: returned in the AI's own frame.)
        if c.tick != self.tick {
            return Err(Desync(format!("ScorePath's path {key} returned in another frame")));
        }
        let Some(v) = c.delivered() else {
            return Err(Desync(format!("ScorePath's path {key} before its modifiers")));
        };
        if fake {
            // `FixPathIfTooShort` replaces a path of fewer than two points, or of two within
            // 0.1 m of each other in x and z (`vector.x * vector.x + vector.z * vector.z > 0.01f`
            // keeps it); its replacement's points are an input.
            let keeps = match v {
                [a, b] => {
                    let (x, z) = (a[0] - b[0], a[2] - b[2]);
                    f64::from(x) * f64::from(x) + f64::from(z) * f64::from(z) > f64::from(0.01f32)
                }
                [] | [_] => false,
                _ => true,
            };
            if keeps {
                return Err(Desync(format!(
                    "ScorePath's path {key}: a fake path where FixPathIfTooShort keeps the sim's {v:?}"
                )));
            }
            self.ag_input("ScorePath's path: FixPathIfTooShort's fake path");
            return Ok(());
        }
        if !same_pts(points, v) || error != c.err || state != c.cs {
            return Err(Desync(format!(
                "ScorePath's path {key}: the sim's {v:?} (error {}, state {}), the AI's {points:?}",
                c.err, c.cs
            )));
        }
        self.out.nav.deliveries += 1;
        Ok(())
    }

    /// The area's physics scene.
    fn nav_scene(&mut self, w: &World) -> Result<&wr_phys::query::Scene<'static>, String> {
        if self.ags.nav.phys.is_none() {
            let s = match w.player.area {
                None => Err("the save names no current area (its physics file)".to_owned()),
                Some(g) => wr_phys::PhysFile::read(&wr_phys::path_for(&g.to_string()))
                    .map(|f| wr_phys::query::Scene::owned(f.world))
                    .map_err(|e| format!("the area's physics file: {e:#}")),
            };
            self.ags.nav.phys = Some(s);
        }
        self.ags.nav.phys.as_ref().expect("loaded").as_ref().map_err(Clone::clone)
    }

    /// The sim's answer to a recorded physics query `r` (`kind` "ray" or "disc", its arguments
    /// already checked against the sim's): `Physics.Raycast` straight down or
    /// `HeuristicDiscCastDown` with `wr-phys`, checked against the recorded hit, distance, point
    /// and collider. A query `wr-phys` can't answer exactly is reported and the recording's taken.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn nav_phys_hit(
        &mut self,
        w: &mut World,
        r: &Rv,
        kind: &str,
        o: V3,
        shape: &[f32],
        max: f32,
        mask: i32,
        ctx: PhysCtx,
    ) -> R<Option<(f32, V3)>> {
        let hit = r["hit"].as_bool().ok_or_else(|| Desync("a query without its hit".into()))?;
        let recorded = if hit {
            Some((
                f(&r["dist"]).ok_or_else(|| Desync("a hit without its distance".into()))?,
                v3(&r["pt"]).ok_or_else(|| Desync("a hit without its point".into()))?,
            ))
        } else {
            None
        };
        let got = match self.nav_scene(w) {
            Err(e) => Err(e),
            Ok(scene) => match (kind, shape) {
                ("ray", [x, y, z])
                    if [*x, *y, *z].map(f32::to_bits) == [0.0f32, -1.0, 0.0].map(f32::to_bits) =>
                {
                    scene.raycast_down(o, max, mask).map_err(|e| e.to_string())
                }
                ("ray", _) => Err("a ray not straight down (not modelled)".into()),
                ("disc", [radius]) => {
                    scene.heuristic_disc_cast_down(o, *radius, max, mask).map_err(|e| e.to_string())
                }
                _ => Err(format!("a {kind} query (not modelled)")),
            }
            .map(|h| {
                h.map(|h| {
                    let s = &scene.world().shapes[h.shape];
                    (h, s.name.clone(), s.sibling_path.clone())
                })
            }),
        };
        let got = match got {
            Ok(g) => g,
            Err(e) => {
                if ctx == PhysCtx::Other {
                    self.ag_input("physics of code the sim doesn't run (unanswered)");
                } else {
                    w.coverage.unsupported(format!("driven replay: physics: {e}"));
                }
                self.out.nav.phys_taken += 1;
                return Ok(recorded);
            }
        };
        let same = match (&got, recorded) {
            (None, None) => true,
            (Some((h, name, sib)), Some((d, _))) => {
                let col = &r["col"];
                let theirs = format!(
                    "{}:{} ({})",
                    col["scene"].as_str().unwrap_or("?"),
                    col["path"].as_str().unwrap_or("?"),
                    col["type"].as_str().unwrap_or("?")
                );
                // The importer's name (`scene:path (type)`) and sibling path (`sib`, an index
                // per ancestor below the root) are the collider's.
                let sib_ok = col["sib"].as_array().is_some_and(|a| {
                    a.len() == sib.len()
                        && a.iter().zip(sib).all(|(x, s)| x.as_u64() == Some(u64::from(*s)))
                });
                h.distance.to_bits() == d.to_bits()
                    && v3(&r["pt"]).map(bits3) == Some(bits3(h.point))
                    && *name == theirs
                    && sib_ok
            }
            _ => false,
        };
        if !same {
            return Err(Desync(format!(
                "a {kind} query from {o:?}: the sim's hit {got:?}, the recording's {}",
                describe(r)
            )));
        }
        match ctx {
            PhysCtx::Agent => self.out.nav.phys_tick += 1,
            PhysCtx::Simulation => self.out.nav.phys_sim += 1,
            PhysCtx::Span | PhysCtx::Other => self.out.nav.phys_span += 1,
            PhysCtx::Ground => self.out.nav.phys_ground += 1,
        }
        Ok(got.map(|(h, ..)| (h.distance, h.point)))
    }

    /// [`Self::nav_phys_hit`]'s distance.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn nav_phys(
        &mut self,
        w: &mut World,
        r: &Rv,
        kind: &str,
        o: V3,
        shape: &[f32],
        max: f32,
        mask: i32,
        ctx: PhysCtx,
    ) -> R<Option<f32>> {
        Ok(self.nav_phys_hit(w, r, kind, o, shape, max, mask, ctx)?.map(|(d, _)| d))
    }

    /// One of the place-on-ground step's rays (`ag_phys` with ctx "ground", the unit's): straight
    /// down from `o`, 100 m, the walkable mask; the sim's answer (distance, point).
    fn nav_ground_ray(&mut self, w: &mut World, u: EntityIdx, o: V3) -> R<Option<(f32, V3)>> {
        let id = w.entity(u).unique_id.clone();
        let at = self.ag_pull(w, "the place-on-ground step's ray", |x| {
            x["k"] == "obs" && x["ev"] == "ag_phys"
        })?;
        let trace = self.trace;
        let r = &trace.records[at];
        const DOWN: V3 = [0.0, -1.0, 0.0];
        if r["u"].as_str() != Some(id.as_str())
            || r["ctx"] != "ground"
            || r["fn"] != "ray"
            || v3(&r["o"]).map(bits3) != Some(bits3(o))
            || v3(&r["d"]).map(bits3) != Some(bits3(DOWN))
            || f(&r["max"]).map(f32::to_bits) != Some(100.0f32.to_bits())
            || r["mask"].as_i64() != Some(2_099_457)
        {
            return Err(Desync(format!(
                "the place-on-ground step's ray from {o:?}: the recording has {}",
                describe(r)
            )));
        }
        self.nav_phys_hit(w, r, "ray", o, &DOWN, 100.0, 2_099_457, PhysCtx::Ground)
    }

    /// `UnitPlaceOnGroundController.TickUnit(unit, (0, 50, 0), 100)` with WrathOracle's
    /// `ground_view`: a ray down from 50 m above the unit (else from 5, 10, .. 95 m above, until
    /// one hits), for a prone unit one from its torso (its position an input: the bone isn't
    /// modelled; the prone action's `AllowFallingBelowGround`, which skips it, an input: recorded
    /// as `fall`, or in captures before it established false by the torso's ray, else reported), the unit
    /// raised by the difference and its `FlyHeight` (an input). The view takes the position; the
    /// unit's setter keeps the old one when `Vector3 ==` holds. Returns (the unit's position, the
    /// view's), checked against the step's record (`ag_ground`: `after`, `vafter`).
    pub(super) fn nav_ground(&mut self, w: &mut World, u: EntityIdx, ctl: bool) -> R<(V3, V3)> {
        let trace = self.trace;
        let id = w.entity(u).unique_id.clone();
        // Its record follows the rays: the reads it made at its start.
        let g = trace.records[self.i..]
            .iter()
            .take_while(|x| x["k"] != "tick")
            .position(|x| x["k"] == "obs" && x["ev"] == "ag_ground" && x["u"] == id.as_str())
            .map(|k| self.i + k)
            .ok_or_else(|| Desync("the place-on-ground step without its record".into()))?;
        let gr = &trace.records[g];
        let (Some(fly), Some(prone)) = (f(&gr["fly"]), gr["prone"].as_bool()) else {
            return Err(Desync(format!(
                "a place-on-ground record it can't read ({})",
                describe(gr)
            )));
        };
        let torso = match (&gr["torso"], prone) {
            (Rv::Null, false) => None,
            (t, true) => {
                Some(v3(t).ok_or_else(|| Desync("a prone unit without its torso".into()))?)
            }
            _ => return Err(Desync("a torso of a unit not prone".into())),
        };
        // The prone action's `AllowFallingBelowGround` (`fall`, null when not prone; absent in
        // captures recorded before it: then only the torso's ray establishes it false).
        let has_fall = gr.as_object().is_some_and(|o| o.contains_key("fall"));
        let fall = match (&gr["fall"], prone) {
            (Rv::Null, false) => None,
            (Rv::Bool(b), true) => Some(*b),
            (Rv::Null, true) if !has_fall => None,
            _ => {
                return Err(Desync(format!(
                    "a place-on-ground record's AllowFallingBelowGround it can't read ({})",
                    describe(gr)
                )));
            }
        };
        self.ag_input("the place-on-ground step's FlyHeight");
        let pos = w.unit(u).ok_or_else(|| Desync("no unit".into()))?.position;
        // `(position + startOffset) ?? Vector3.up`: the offset (0, 50, 0).
        let mut num = 0.0f32;
        let o = [pos[0] + 0.0, pos[1] + 50.0, pos[2] + 0.0];
        if let Some((_, pt)) = self.nav_ground_ray(w, u, o)? {
            num = pt[1] - pos[1];
        } else {
            for i in 1..20 {
                // `Vector3.up * 5f * i`, then `position + vector`.
                let k = i as f32;
                let v = [0.0 * 5.0 * k, 5.0 * k, 0.0 * 5.0 * k];
                let o = [pos[0] + v[0], pos[1] + v[1], pos[2] + v[2]];
                if let Some((_, pt)) = self.nav_ground_ray(w, u, o)? {
                    num = pt[1] - pos[1];
                    break;
                }
            }
        }
        if let Some(t) = torso {
            let next = self.ag_peek();
            let ray = next.is_some_and(|x| {
                x["ev"] == "ag_phys" && x["ctx"] == "ground" && x["u"] == id.as_str()
            });
            match (fall, ray) {
                (Some(true), true) => {
                    return Err(Desync(
                        "a torso ray of a prone unit whose action allows falling below ground"
                            .into(),
                    ));
                }
                // Recorded false: the ray is required (the pull reports it missing).
                (Some(false), _) | (None, true) => {}
                (Some(true), false) => self.ag_input("the prone action's AllowFallingBelowGround"),
                (None, false) => {
                    return Err(Desync(
                        "a prone unit's step without its torso ray, and the prone action's \
                         AllowFallingBelowGround not recorded (ground_view before `fall`)"
                            .into(),
                    ));
                }
            }
            if fall != Some(true) {
                let o = [t[0] + 0.0, t[1] + 1.0, t[2] + 0.0];
                if let Some((_, pt)) = self.nav_ground_ray(w, u, o)? {
                    // `Mathf.Clamp(hit.y - torso.y, num, Mathf.Max(num, 0f) + 0.5f)`.
                    let value = pt[1] - t[1];
                    let hi = store_f32(f64::from(num.max(0.0)) + 0.5);
                    num = if value < num {
                        num
                    } else if value > hi {
                        hi
                    } else {
                        value
                    };
                }
            }
        }
        let mut p = pos;
        p[1] += num;
        p[1] += fly;
        let after = crate::combat::orientation::set_position(pos, p);
        let at = self.ag_pull(w, "the place-on-ground step's end", |x| {
            x["k"] == "obs" && x["ev"] == "ag_ground"
        })?;
        let r = &trace.records[at];
        if r["u"].as_str() != Some(id.as_str())
            || r["ctl"].as_bool() != Some(ctl)
            || v3(&r["pos"]).map(bits3) != Some(bits3(pos))
            || v3(&r["after"]).map(bits3) != Some(bits3(after))
            || v3(&r["vafter"]).map(bits3) != Some(bits3(p))
            || !(fields(
                r,
                &["u", "ctl", "pos", "after", "vafter", "fly", "prone", "torso", "fall"],
            ) || !has_fall
                && fields(r, &["u", "ctl", "pos", "after", "vafter", "fly", "prone", "torso"]))
        {
            return Err(Desync(format!(
                "the place-on-ground step: the sim's unit at {after:?}, view at {p:?}; the recording's {}",
                describe(r)
            )));
        }
        self.out.nav.ground_steps += 1;
        Ok((after, p))
    }

    /// The place-on-ground controller's own step (outside `UnitMoveController.TickUnit`), from
    /// its first ray (`ground_view`). Returns the next record.
    pub(super) fn nav_ground_entry(&mut self, w: &mut World, i: usize) -> usize {
        let trace = self.trace;
        let r = &trace.records[i];
        let Some(u) = r["u"].as_str().and_then(|x| w.by_id.get(x).copied()) else {
            w.coverage.unsupported("driven replay: agent: a place-on-ground ray of no unit");
            return i + 1;
        };
        self.i = i;
        match self.nav_ground(w, u, false) {
            Ok((after, view)) => {
                if let Some(x) = w.unit_mut(u) {
                    x.position = after;
                    x.combat.agent.pos = view;
                }
            }
            Err(d) => {
                w.coverage.unsupported(format!("driven replay: agent: {}", d.0));
                // The step's records, up to its end, are skipped.
                while let Some(x) = trace.records.get(self.i) {
                    if x["k"] == "tick" {
                        break;
                    }
                    self.i += 1;
                    if x["ev"] == "ag_ground" {
                        break;
                    }
                }
            }
        }
        self.i
    }

    /// A physics query of the movement simulation outside a step the sim runs: answered by the
    /// sim from its recorded arguments and checked.
    pub(super) fn nav_phys_alone(&mut self, w: &mut World, r: &Rv, ctx: PhysCtx) {
        let kind = r["fn"].as_str().unwrap_or("").to_owned();
        let shape: Option<Vec<f32>> = if kind == "ray" {
            v3(&r["d"]).map(|d| d.to_vec())
        } else {
            f(&r["r"]).map(|x| vec![x])
        };
        let (Some(o), Some(shape), Some(max), Some(mask)) = (
            v3(&r["o"]),
            shape,
            f(&r["max"]),
            r["mask"].as_i64().and_then(|m| i32::try_from(m).ok()),
        ) else {
            w.coverage.unsupported("driven replay: physics: a query it can't read");
            return;
        };
        if let Err(d) = self.nav_phys(w, r, &kind, o, &shape, max, mask, ctx) {
            w.coverage.unsupported(format!("driven replay: physics: {}", d.0));
        }
    }

    /// `ag_sim` .. `ag_sim_end`: one `SimulateTickMovementForTurnBased` step (the AI's
    /// `ScorePath` or the path visualizer), run from its recorded simulator with the step's reads
    /// (`MaxSpeed`, `FlyHeight`, line of sight: inputs), the sim's physics and (with the paths
    /// the sim's) grid; the simulator after it checked. Returns the next record.
    pub(super) fn nav_sim_step(&mut self, w: &mut World, i: usize) -> usize {
        let trace = self.trace;
        let r = &trace.records[i];
        if !w.combat.turn_based {
            self.ag_input("the AI's simulation steps (real time)");
            return i + 1;
        }
        let parsed = (|| {
            let u = r["u"].as_str().and_then(|x| w.by_id.get(x).copied())?;
            let s = &r["sim"];
            let sim = MovementSimulator {
                m_position: v3(&s["p"])?,
                path: pts(&s["pts"])?,
                next_waypoint: v2(&s["nwp"])?,
                next_waypoint_3d: v3(&s["nwp3"])?,
                waypoint_index: i32::try_from(s["wi"].as_i64()?).ok()?,
                last_move_length: f(&s["len"])?,
                reached_max_distance: s["rmd"].as_bool()?,
                approach_point: v3(&s["ap"])?,
                approach_radius: f(&s["ar"])?,
                check_los: s["los"].as_bool()?,
                ignore_blocker_radius: f(&s["ibr"])?,
            };
            let ok = fields(r, &["u", "dt", "max", "sim"])
                && s.as_object().is_some_and(|o| o.len() == 11);
            ok.then_some((u, sim, f(&r["dt"])?, f(&r["max"])?))
        })();
        // Both callers (`AiAction.ScorePath`, `PathVisualizer`) step by `0.1f`.
        let parsed = parsed.filter(|(_, _, dt, _)| dt.to_bits() == 0.1f32.to_bits());
        let Some((u, mut sim, dt, max)) = parsed else {
            w.coverage.unsupported(format!(
                "driven replay: agent: a simulation step it can't read ({})",
                describe(r)
            ));
            return i + 1;
        };
        self.i = i + 1;
        self.ags.nav.last_step = None;
        let res = (|| {
            let ret = ai_sim::simulate_tick_movement_for_turn_based(
                w,
                u,
                &mut sim,
                dt,
                max,
                &mut SimIn { d: &mut *self },
            )?;
            let id = w.entity(u).unique_id.clone();
            let at = self.ag_pull(w, "the simulation step's end", |x| {
                x["k"] == "obs" && x["ev"] == "ag_sim_end"
            })?;
            let e = &trace.records[at];
            let s = &e["sim"];
            let same = e["u"].as_str() == Some(id.as_str())
                && e["ret"].as_bool() == Some(ret)
                && fields(e, &["u", "ret", "sim"])
                && s.as_object().is_some_and(|o| o.len() == 10)
                && v3(&s["p"]).map(bits3) == Some(bits3(sim.m_position))
                && f(&s["len"]).map(f32::to_bits) == Some(sim.last_move_length.to_bits())
                && s["rmd"].as_bool() == Some(sim.reached_max_distance)
                && s["wi"].as_i64() == Some(i64::from(sim.waypoint_index))
                && v2(&s["nwp"]).map(|v| v.map(f32::to_bits))
                    == Some(sim.next_waypoint.map(f32::to_bits))
                && v3(&s["nwp3"]).map(bits3) == Some(bits3(sim.next_waypoint_3d))
                && v3(&s["ap"]).map(bits3) == Some(bits3(sim.approach_point))
                && f(&s["ar"]).map(f32::to_bits) == Some(sim.approach_radius.to_bits())
                && s["los"].as_bool() == Some(sim.check_los)
                && f(&s["ibr"]).map(f32::to_bits) == Some(sim.ignore_blocker_radius.to_bits());
            if !same {
                return Err(Desync(format!(
                    "the simulation step returned {ret} with {sim:?}; the recording's {}",
                    describe(e)
                )));
            }
            Ok(ret)
        })();
        match res {
            Ok(ret) => {
                self.out.nav.sim_steps += 1;
                self.ags.nav.last_step = Some((self.tick, u, max, ret, sim.clone()));
            }
            Err(d) => {
                w.coverage.unsupported(format!("driven replay: agent: {}", d.0));
                // The rest of the step's records, up to its end in its frame, are skipped (any
                // other record processed as usual).
                while let Some(x) = trace.records.get(self.i) {
                    if x["k"] == "tick" {
                        break;
                    }
                    let obs = x["k"] == "obs";
                    let mine = obs
                        && (x["ev"] == "ag_sim_end"
                            || (matches!(x["ev"].as_str(), Some("ag_v" | "ag_phys"))
                                && x["ctx"] == "sim"));
                    if !mine {
                        // Another agent record is the next entry's.
                        if obs && x["ev"].as_str().is_some_and(|e| e.starts_with("ag_")) {
                            break;
                        }
                        let at = self.i;
                        self.i = self.record(w, at);
                        continue;
                    }
                    self.i += 1;
                    if x["ev"] == "ag_sim_end" {
                        break;
                    }
                }
            }
        }
        self.i
    }

    /// The step's leaf `leaf` of unit `u` (`ag_v` with `ctx: "sim"`).
    fn nav_sim_leaf(&mut self, w: &mut World, u: EntityIdx, leaf: &str, keys: &[&str]) -> R<Rv> {
        let id = w.entity(u).unique_id.clone();
        let at = self.ag_pull(w, &format!("the simulation's {leaf}"), |x| {
            x["k"] == "obs" && x["ev"] == "ag_v" && x["f"] == leaf
        })?;
        let r = self.trace.records[at].clone();
        if r["u"].as_str() != Some(id.as_str()) || r["ctx"] != "sim" || !fields(&r, keys) {
            return Err(Desync(format!(
                "the simulation's {leaf} of {id}: the recording's is {}",
                describe(&r)
            )));
        }
        Ok(r)
    }

    /// A frame's controllers have all run: the grid's events of the frame are the recording's,
    /// all paired.
    pub(super) fn nav_frame_done(&mut self, w: &mut World) {
        // A step the AI didn't read in its frame isn't its.
        self.ags.nav.last_step = None;
        if !self.ags.nav.paths {
            return;
        }
        // A call's prefix is used in its frame.
        if self.ags.nav.path_to.take().is_some() || self.ags.nav.path_done.take().is_some() {
            Self::nav_unsupported(w, "a path_to or path_done without the sim's call in its frame");
        }
        if w.combat.nav.broken {
            return;
        }
        if !self.nav_in_step(w) {
            let ours = w.combat.nav.nav.as_ref().map_or(0, |n| n.log.len());
            Self::nav_unsupported(
                w,
                format!(
                    "graph events left at a frame's end: {} recorded, {} the sim's",
                    self.ags.nav.rec_nav.len(),
                    ours - self.ags.nav.log_at.min(ours)
                ),
            );
            // (Paired from here on as they come.)
        }
    }

    /// `PathTo(destination, approachRadius, ...)`'s recorded prefix (`path_to`): the sim's call.
    pub(super) fn nav_path_to(&mut self, w: &World, u: EntityIdx, dest: V3, radius: f32) -> R<()> {
        if !self.ags.nav.paths {
            return Ok(());
        }
        let Some(at) = self.ags.nav.path_to.take() else {
            return Err(Desync("PathTo without its path_to".into()));
        };
        let r = &self.trace.records[at];
        if r["t"].as_i64() != Some(self.tick) {
            return Err(Desync(format!("PathTo's path_to is of another frame ({})", describe(r))));
        }
        let from = w.unit(u).map(|x| x.combat.agent.pos);
        if r["u"].as_str() != Some(w.entity(u).unique_id.as_str())
            || v3(&r["dest"]).map(bits3) != Some(bits3(dest))
            || f(&r["radius"]).map(f32::to_bits) != Some(radius.to_bits())
            || v3(&r["from"]).map(bits3) != from.map(bits3)
            || !fields(r, &["u", "dest", "radius", "from"])
        {
            return Err(Desync(format!(
                "PathTo({dest:?}, {radius}) from {from:?}: the recording's path_to is {}",
                describe(r)
            )));
        }
        Ok(())
    }

    /// `OnPathComplete(Path)`'s recorded prefix (`path_done`): the delivered path's error, count,
    /// end and `GetTotalLength()`, the sim's.
    pub(super) fn nav_path_done(
        &mut self,
        w: &World,
        u: EntityIdx,
        key: &str,
        err: bool,
        v: &[V3],
    ) -> R<()> {
        if !self.ags.nav.paths {
            return Ok(());
        }
        let Some(at) = self.ags.nav.path_done.take() else {
            return Err(Desync("OnPathComplete without its path_done".into()));
        };
        // `Path.ReturnPath` runs the seeker's modifiers, then its callback (`OnPathComplete`).
        if self.ags.nav.computed.contains_key(key)
            && self.ags.nav.returned_at.get(key).is_none_or(|j| at < *j)
        {
            return Err(Desync(format!("{key}: OnPathComplete's path_done before its return")));
        }
        let r = &self.trace.records[at];
        if r["t"].as_i64() != Some(self.tick) {
            return Err(Desync(format!(
                "OnPathComplete's path_done is of another frame ({})",
                describe(r)
            )));
        }
        let end_ok = match v.last() {
            Some(e) => v3(&r["end"]).map(bits3) == Some(bits3(*e)),
            None => r.get("end").is_none(),
        };
        let keys: &[&str] = if v.is_empty() {
            &["u", "error", "nodes", "length"]
        } else {
            &["u", "error", "nodes", "end", "length"]
        };
        let length = wr_nav::query::total_length(v);
        if r["u"].as_str() != Some(w.entity(u).unique_id.as_str())
            || r["error"].as_bool() != Some(err)
            || r["nodes"].as_u64() != Some(v.len() as u64)
            || !end_ok
            || f(&r["length"]).map(f32::to_bits) != Some(length.to_bits())
            || !fields(r, keys)
        {
            return Err(Desync(format!(
                "OnPathComplete's path (error {err}, {} points, length {length}): the recording's path_done is {}",
                v.len(),
                describe(r)
            )));
        }
        self.out.nav.deliveries += 1;
        Ok(())
    }

    /// The AI's read of a grid node's `AllyBlocked` at `p` (`ScorePath`): the sim's grid's.
    pub(super) fn nav_ai_blocked(&mut self, w: &World, p: V3, recorded: i64) -> R<()> {
        if !self.ags.nav.paths {
            return Ok(());
        }
        if !Self::nav_live(w) {
            return Err(Desync("the AI reads AllyBlocked; the grid isn't the sim's".into()));
        }
        let world = &w.combat.nav.nav.as_ref().expect("live").world;
        let ours = world.grid_nearest(p).node.map(|n| i64::from(world.ally_blocked(n)));
        if ours != Some(recorded) {
            return Err(Desync(format!(
                "the AI's AllyBlocked under {p:?}: the sim's {ours:?}, the recording's {recorded}"
            )));
        }
        self.out.nav.sim_grid_checked += 1;
        Ok(())
    }

    /// The AI's read of `ScorePath`'s simulation step (`msim`): the step the sim just ran.
    pub(super) fn nav_ai_msim(
        &mut self,
        w: &World,
        max: f32,
        step: &crate::combat::ai::SimStep,
    ) -> R<()> {
        if !self.ag {
            return Ok(());
        }
        let Some((tick, u, m, ret, sim)) = self.ags.nav.last_step.take() else {
            return Err(Desync("ScorePath's simulation step without the sim's step".into()));
        };
        if tick != self.tick {
            return Err(Desync(
                "ScorePath's simulation step: the sim's step is of another frame".into(),
            ));
        }
        // The step of the AI's own unit (`context.Unit.View.AgentASP`).
        if crate::combat::ai::context_unit(w) != Some(u) {
            return Err(Desync(format!(
                "ScorePath's simulation step: the sim's step is {}'s, not the AI's unit's",
                w.entity(u).unique_id
            )));
        }
        let same = m.to_bits() == max.to_bits()
            && ret == step.ret
            && bits3(sim.m_position) == bits3(step.position)
            && sim.last_move_length.to_bits() == step.last_move_length.to_bits()
            && sim.reached_max_distance == step.reached_max
            && sim.waypoint_index == step.waypoint;
        if !same {
            return Err(Desync(format!(
                "ScorePath's simulation step: the sim's {ret} with {sim:?} (max {m}), the AI's {step:?} (max {max})"
            )));
        }
        self.out.nav.deliveries += 1;
        Ok(())
    }

    pub(super) fn nav_finish(&mut self, w: &mut World) {
        if !self.ags.nav.paths {
            return;
        }
        self.nav_match(w);
        let ours = w.combat.nav.nav.as_ref().map_or(0, |n| n.log.len());
        let (left_rec, left_ours) =
            (self.ags.nav.rec_nav.len(), ours - self.ags.nav.log_at.min(ours));
        if left_rec + left_ours > 0 && !w.combat.nav.broken {
            Self::nav_unsupported(
                w,
                format!("graph events left: {left_rec} recorded, {left_ours} the sim's"),
            );
        }
        if self.ags.nav.path_to.is_some() || self.ags.nav.path_done.is_some() {
            Self::nav_unsupported(w, "a path_to or path_done without the sim's call");
        }
        if self.ags.nav.tape.as_ref().is_some_and(|t| !t.is_empty()) {
            Self::nav_unsupported(w, "the recording's navigation clock reads left unused");
        }
        let n = &w.combat.nav;
        self.out.nav.turn_starts = n.turn_starts;
        self.out.nav.removals = n.removals;
        self.out.nav.refreshes = n.refreshes;
    }
}

/// A step's reads, pulled from its records.
struct SimIn<'a, 'b> {
    d: &'a mut Driver<'b>,
}

impl SimInputs for SimIn<'_, '_> {
    fn max_speed(&mut self, w: &mut World, u: EntityIdx) -> R<f32> {
        let r = self.d.nav_sim_leaf(w, u, "maxspeed", &["f", "u", "ctx", "v"])?;
        self.d.ag_input("the simulation's MaxSpeed");
        f(&r["v"]).ok_or_else(|| Desync("maxspeed without a value".into()))
    }
    fn fly_height(&mut self, w: &mut World, u: EntityIdx) -> R<f32> {
        let r = self.d.nav_sim_leaf(w, u, "fly", &["f", "u", "ctx", "v"])?;
        self.d.ag_input("the simulation's FlyHeight");
        f(&r["v"]).ok_or_else(|| Desync("fly without a value".into()))
    }
    fn raycast(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        o: V3,
        d: V3,
        max: f32,
        mask: i32,
    ) -> R<Option<f32>> {
        self.query(w, u, "ray", o, &d, max, mask)
    }
    fn disc_cast(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        o: V3,
        r: f32,
        max: f32,
        mask: i32,
    ) -> R<Option<f32>> {
        self.query(w, u, "disc", o, &[r], max, mask)
    }
    fn ally_blocked(&mut self, w: &mut World, u: EntityIdx, p: V3) -> R<Option<i8>> {
        let r = self.d.nav_sim_leaf(w, u, "near", &["f", "u", "ctx", "p", "v"])?;
        if v3(&r["p"]).map(bits3) != Some(bits3(p)) {
            return Err(Desync(format!(
                "the simulation's GetNearest({p:?}): the recording's is {}",
                describe(&r)
            )));
        }
        let recorded = match &r["v"] {
            Rv::Null => None,
            v => Some(
                v.as_i64()
                    .and_then(|x| i8::try_from(x).ok())
                    .ok_or_else(|| Desync("near without a value".into()))?,
            ),
        };
        if !(self.d.ags.nav.paths && Driver::nav_live(w)) {
            self.d.out.nav.sim_grid_taken += 1;
            self.d.ag_input("the simulation's AllyBlocked (no grid)");
            return Ok(recorded);
        }
        let world = &w.combat.nav.nav.as_ref().expect("live").world;
        let ours = world.grid_nearest(p).node.map(|n| world.ally_blocked(n));
        if ours != recorded {
            return Err(Desync(format!(
                "the simulation's AllyBlocked under {p:?}: the sim's {ours:?}, the recording's {recorded:?}"
            )));
        }
        self.d.out.nav.sim_grid_checked += 1;
        Ok(ours)
    }
    fn has_obstacle(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        from: V3,
        to: V3,
        fudge: f32,
    ) -> R<bool> {
        let r = self.d.nav_sim_leaf(w, u, "los", &["f", "u", "ctx", "from", "to", "r", "v"])?;
        if v3(&r["from"]).map(bits3) != Some(bits3(from))
            || v3(&r["to"]).map(bits3) != Some(bits3(to))
            || f(&r["r"]).map(f32::to_bits) != Some(fudge.to_bits())
        {
            return Err(Desync(format!(
                "the simulation's HasObstacle: the recording's is {}",
                describe(&r)
            )));
        }
        self.d.ag_input("the simulation's line of sight");
        boolean(&r, "v", "los")
    }
}

impl SimIn<'_, '_> {
    #[allow(clippy::too_many_arguments)]
    fn query(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        kind: &str,
        o: V3,
        shape: &[f32],
        max: f32,
        mask: i32,
    ) -> R<Option<f32>> {
        let id = w.entity(u).unique_id.clone();
        let at = self.d.ag_pull(w, &format!("the simulation's {kind} query"), |x| {
            x["k"] == "obs" && x["ev"] == "ag_phys"
        })?;
        let trace = self.d.trace;
        let r = &trace.records[at];
        let recorded: Option<Vec<f32>> = if kind == "ray" {
            v3(&r["d"]).map(|d| d.to_vec())
        } else {
            f(&r["r"]).map(|x| vec![x])
        };
        let same_shape = recorded
            .is_some_and(|x| x.iter().map(|v| v.to_bits()).eq(shape.iter().map(|v| v.to_bits())));
        if r["u"].as_str() != Some(id.as_str())
            || r["fn"] != kind
            || r["ctx"] != "sim"
            || v3(&r["o"]).map(bits3) != Some(bits3(o))
            || !same_shape
            || f(&r["max"]).map(f32::to_bits) != Some(max.to_bits())
            || r["mask"].as_i64() != Some(i64::from(mask))
        {
            return Err(Desync(format!(
                "the simulation's {kind} query from {o:?}: the recording has {}",
                describe(r)
            )));
        }
        self.d.nav_phys(w, r, kind, o, shape, max, mask, PhysCtx::Simulation)
    }
}
