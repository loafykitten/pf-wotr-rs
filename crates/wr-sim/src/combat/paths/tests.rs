//! Turn-based paths against WrathOracle's `agent` records: every `ABPathWithClearance` the
//! movement agent requests (`ag_req_end`'s `rq`) computed with `wr-nav` on the grid as
//! [`CombatNav`] blocks it from the recorded fight (turn order, positions, corpulences,
//! avoidance flags, turn starts, units leaving, the navigation timer), the request's
//! parameters checked against the recorded ones, the path compared bit for bit with every
//! delivery recorded: its raw `vectorPath` (`ag_pathdef` before the return), the callback's
//! (`ag_done`, the AI's `ScorePath` tap) with `error` and `CompleteState`.
//!
//! Line of sight (`LineOfSightGeometry.HasObstacle`) inside the search isn't recorded by
//! `poc5-tb-103`: a path that asks is checked with the answers its recorded end implies
//! (clear at the node it ended on, blocked elsewhere) and counted apart.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use serde_json::Value;
use wr_nav::astar::{Ending, LosQuery, State};

use super::*;
use crate::combat::oracle::Trace;
use crate::combat::replay::Row;

const CAVES: &str = "bad465e5404604346bdf71ef50b3299a";

use wr_nav::world::NodeId;

fn f(v: &Value) -> f32 {
    v.as_f64().map_or(f32::NAN, |x| x as f32)
}

fn v3(v: &Value) -> V3 {
    [f(&v[0]), f(&v[1]), f(&v[2])]
}

fn opt_v3(v: &Value) -> Option<V3> {
    if v.is_null() { None } else { Some(v3(v)) }
}

fn opt_f(v: &Value) -> Option<f32> {
    if v.is_null() { None } else { Some(f(v)) }
}

fn pts(v: &Value) -> Vec<V3> {
    v.as_array().map(|a| a.iter().map(v3).collect()).unwrap_or_default()
}

fn bits(p: &[V3]) -> Vec<[u32; 3]> {
    p.iter().map(|x| x.map(f32::to_bits)).collect()
}

fn same(a: V3, b: V3) -> bool {
    a.map(f32::to_bits) == b.map(f32::to_bits)
}

fn captures() -> Vec<(String, PathBuf)> {
    let labels = std::env::var("WR_PATH_CAPTURES").unwrap_or_else(|_| "poc5-tb-103".into());
    let mut out = Vec::new();
    for label in labels.split(',') {
        let root =
            wr_paths::repo_root().join("fixtures/captures/oracle").join(label).join("fixtures");
        for fx in ["tb-nearest", "tb-focus"] {
            for run in ["record", "replay-1", "replay-2"] {
                out.push((
                    format!("{label}/{fx}/{run}"),
                    root.join(fx).join(format!("{run}.trace.jsonl.gz")),
                ));
            }
        }
    }
    out
}

/// A recorded delivery of a path's `vectorPath`.
#[derive(Debug)]
struct Delivery {
    what: &'static str,
    seq: i64,
    v: Vec<V3>,
    /// `error`, `CompleteState` where recorded.
    err: Option<bool>,
    cs: Option<i64>,
    /// `searchedNodes` where recorded (`path_fin`).
    searched: Option<i64>,
    /// Before the modifiers (the traced nodes).
    raw: bool,
}

/// A recorded `LineOfSightGeometry.HasObstacle` call inside a search (`path_fin`'s `los`).
#[derive(Debug, Clone, Copy)]
struct LosAnswer {
    from: V3,
    to: V3,
    fudge: f32,
    blocked: bool,
}

/// How a request's check came out.
#[derive(Debug, Default)]
struct Tally {
    /// Computed without line of sight; every recorded delivery equal.
    exact: usize,
    /// Computed with line-of-sight answers implied by the recorded end; deliveries equal.
    los_implied: usize,
    /// Of those, answers implied "blocked" (the search went on past a node in range).
    los_blocked_answers: usize,
    los_clear_answers: usize,
    /// Computed, nothing recorded to compare with (the player's `FindPath`s).
    unrecorded: usize,
    unrecorded_needs_los: usize,
    /// Deliveries compared.
    deliveries: usize,
    /// `FollowPrecomputedPath`s equal to a computed `FindPath` result.
    forced_equal: usize,
    /// `FollowPrecomputedPath`s of a path whose `FindPath` result isn't recorded.
    forced_unchecked: usize,
    /// Turn starts and removals applied to the grid.
    turn_starts: usize,
    removals: usize,
    tag_refreshes: usize,
    /// Requests whose corpulence came from the request's own clearance.
    corpulence_from_request: usize,
    /// Paths computed with recorded line-of-sight answers (observer `paths`).
    los_recorded: usize,
    /// Graph events (`nav_guo`, `nav_update`, `nav_tags`) compared.
    nav_events: usize,
    /// `ag_req_end`s reporting a request already started (`PathTo` keeping it).
    retained: usize,
}

struct Fight<'a> {
    label: String,
    recs: &'a [Value],
    nav: CombatNav,
    node_size: f32,
    pos: HashMap<String, V3>,
    corp: HashMap<String, f32>,
    avoid: HashMap<String, bool>,
    order: Vec<String>,
    initialized: bool,
    current: Option<String>,
    timer: f32,
    enemy: &'a mut dyn FnMut(&str, &str) -> Option<bool>,
    failures: Vec<String>,
    tally: Tally,
    /// The `FindPath` results computed and compared, per unit (for `FollowPrecomputedPath`).
    found_paths: HashMap<String, Vec<Vec<V3>>>,
    /// The acting rider who left combat, until `ChooseNextUnit` has run.
    leaving: Vec<String>,
    /// Whether the unit's last `FindPath` had its result recorded and compared.
    last_find_checked: HashMap<String, bool>,
    /// The requests checked, by `PathSync` key, with their recorded fields.
    started: HashMap<String, Value>,
    /// The trace declares observer `paths`: its graph records are compared with
    /// [`CombatNav::log`] in order.
    nav_paths: bool,
    rec_nav: VecDeque<usize>,
    log_at: usize,
    /// Recorded `GraphUpdateObject` ids to ours.
    guo_ids: HashMap<i64, u32>,
}

impl Fight<'_> {
    fn fail(&mut self, i: usize, msg: String) {
        let r = &self.recs[i];
        self.failures.push(format!("{} seq {} t={}: {msg}", self.label, r["seq"], r["t"]));
    }

    fn turn(&mut self, i: usize, rider: &str, alive_in_combat: bool) -> Option<Turn> {
        let Some(&corp) = self.corp.get(rider) else {
            self.fail(i, format!("no corpulence recorded for the acting unit {rider}"));
            return None;
        };
        let Some(&avoid) = self.avoid.get(rider) else {
            self.fail(i, format!("no avoidance flag recorded for the acting unit {rider}"));
            return None;
        };
        Some(Turn {
            unit: rider.to_owned(),
            mount: None,
            corpulence: corp,
            avoidance_disabled: avoid,
            rider_avoidance_disabled: avoid,
            rider_size_in_cells: size_in_cells(corp, self.node_size),
            rider_in_combat_alive: alive_in_combat,
        })
    }

    fn current_turn(&mut self, i: usize) -> Option<Turn> {
        let c = self.current.clone()?;
        self.turn(i, &c, true)
    }

    fn start_turn(&mut self, i: usize, rider: &str) {
        let Some(turn) = self.turn(i, rider, true) else { return };
        let mut units = Vec::new();
        for id in self.order.clone() {
            let (Some(&position), Some(&corpulence), Some(&avoid)) =
                (self.pos.get(&id), self.corp.get(&id), self.avoid.get(&id))
            else {
                self.fail(
                    i,
                    format!("turn of {rider}: {id}'s position, corpulence or avoidance unknown"),
                );
                return;
            };
            let enemy = if id == rider {
                false
            } else if let Some(e) = (self.enemy)(&id, rider) {
                e
            } else {
                self.fail(i, format!("IsEnemy({id}, {rider}) unknown"));
                return;
            };
            units.push(OrderUnit {
                id,
                position,
                corpulence,
                avoidance_disabled: avoid,
                enemy_of_current: enemy,
            });
        }
        if let Err(e) = self.nav.start_turn(&units, &turn) {
            self.fail(i, format!("StartTurn({rider}): {e:#}"));
        }
        self.current = Some(rider.to_owned());
        self.tally.turn_starts += 1;
    }

    fn remove_unit(&mut self, i: usize, u: &str) {
        if !self.order.iter().any(|x| x == u) {
            return;
        }
        self.nav.revert(u);
        // UpdateNavigationGridTags: the acting rider erodes only while in combat and alive;
        // the unit leaving is out of combat already.
        let turn = match self.current.clone() {
            Some(c) => self.turn(i, &c, c != u),
            None => None,
        };
        if let Err(e) = self.nav.update_navigation_grid_tags(turn.as_ref()) {
            self.fail(i, format!("RemoveUnit({u}): {e:#}"));
        }
        if self.current.as_deref() == Some(u) {
            self.current = None;
        }
        // For the acting or the next unit, RemoveUnit runs ChooseNextUnit before the unit
        // leaves m_Units: the `choose_next` that follows (before any other unit leaves or the
        // tick ends) lists it, and whatever ChooseNextUnit does (a sort, a turn start) sees it.
        let inner = self.recs[i + 1..]
            .iter()
            .find(|r| r["k"] == "tick" || r["ev"] == "leave_combat" || r["ev"] == "choose_next")
            .is_some_and(|r| {
                r["ev"] == "choose_next"
                    && r["units"].as_array().is_some_and(|a| {
                        a.iter().any(|x| x.as_str().and_then(|s| s.split('|').next()) == Some(u))
                    })
            });
        if inner {
            self.leaving.push(u.to_owned());
        } else {
            self.order.retain(|x| x != u);
        }
        self.tally.removals += 1;
    }

    /// `CombatController.Clear`: every unit's object reverted, the tags refreshed, no turn,
    /// no units.
    fn clear(&mut self, i: usize) {
        for id in self.order.clone() {
            self.nav.revert(&id);
        }
        let turn = self.current_turn(i);
        if let Err(e) = self.nav.update_navigation_grid_tags(turn.as_ref()) {
            self.fail(i, format!("Clear: {e:#}"));
        }
        self.tally.tag_refreshes += 1;
        self.current = None;
        self.order.clear();
    }

    /// Pairs the recorded graph events with ours, in order.
    fn match_nav(&mut self) {
        while let (Some(&j), Some(ev)) = (self.rec_nav.front(), self.nav.log.get(self.log_at)) {
            let ev = ev.clone();
            self.rec_nav.pop_front();
            self.log_at += 1;
            self.tally.nav_events += 1;
            let r = &self.recs[j];
            let why = match (r["ev"].as_str().unwrap_or(""), &ev) {
                ("nav_guo", NavEvent::Queued { id, update: o }) => {
                    let mut why = self.same_guo(r["id"].as_i64().unwrap_or(-1), *id);
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
                    if !same(v3(&r["c"]), o.bounds.center)
                        || !same(v3(&r["e"]), o.bounds.extents)
                        || r["mask"].as_i64() != Some(i64::from(o.graph_mask))
                        || r["f"] != flags.as_str()
                        || r["tag"].as_i64() != Some(i64::from(o.set_tag))
                        || r["pen"].as_i64() != Some(0)
                        || r["shape"] != false
                    {
                        why.get_or_insert(format!("the sim queued {o:?} (flags {flags})"));
                    }
                    why
                }
                ("nav_update", NavEvent::Applied { id, erosion }) => {
                    let mut why = self.same_guo(r["id"].as_i64().unwrap_or(-1), *id);
                    if r["erosion"].as_i64() != Some(i64::from(*erosion)) {
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
                        || r["ally"].as_u64() != Some(*ally as u64))
                    .then(|| {
                        format!(
                            "the sim's grid: erode {erode}, only {only}, hash {ours}, {dynamic} blocked, \
                             {ally} ally-blocked"
                        )
                    })
                }
                (k, ev) => Some(format!("{k} recorded where the sim has {ev:?}")),
            };
            if let Some(why) = why {
                let r = self.recs[j].clone();
                self.fail(j, format!("{}: {why} ({r})", r["ev"]));
            }
        }
    }

    /// Recorded object ids map one to one onto ours.
    fn same_guo(&mut self, theirs: i64, ours: u32) -> Option<String> {
        match self.guo_ids.get(&theirs) {
            Some(&o) if o == ours => None,
            Some(&o) => Some(format!("graph update {theirs} is the sim's {o}, not {ours}")),
            None if self.guo_ids.values().any(|&o| o == ours) => {
                Some(format!("the sim's graph update {ours} recorded under a new id {theirs}"))
            }
            None => {
                self.guo_ids.insert(theirs, ours);
                None
            }
        }
    }

    /// The rider who left mid-turn leaves the order once `ChooseNextUnit` is done.
    fn settle_leaving(&mut self) {
        for u in std::mem::take(&mut self.leaving) {
            self.order.retain(|x| *x != u);
        }
    }

    fn deliveries(&self, i: usize, key: &str, fun: &str) -> Vec<Delivery> {
        let mut out = Vec::new();
        // With observer `paths`, the seeker's modifiers end at `path_mod`: a definition before
        // it is the traced nodes (`PathFinPrefix`'s own reference writes one), after it the
        // returned path. A path that failed has no modifiers: both are the same.
        let modified = self.nav_paths.then(|| {
            self.recs[i + 1..]
                .iter()
                .position(|r| r["ev"] == "path_mod" && r["path"] == key)
                .map(|k| i + 1 + k)
        });
        // Delivered: the recording's `path_ret` (ReturnPaths), or the callback itself.
        let mut returned = false;
        for j in i + 1..self.recs.len() {
            let r = &self.recs[j];
            if r["k"] != "obs" {
                continue;
            }
            let ev = r["ev"].as_str().unwrap_or("");
            if r["path"] == key {
                match ev {
                    "path_ret" => returned = true,
                    // A definition written by the callback's own first reference (the
                    // `ag_done` prefix) is the returned path; one before the return, the
                    // traced nodes.
                    "ag_pathdef" if r["pending"] != true => out.push(Delivery {
                        what: "ag_pathdef",
                        seq: r["seq"].as_i64().unwrap_or(-1),
                        v: pts(&r["v"]),
                        err: r["err"].as_bool(),
                        cs: r["cs"].as_i64(),
                        searched: None,
                        raw: match modified {
                            Some(m) => m.is_none_or(|m| j < m),
                            None => {
                                !returned
                                    && !self
                                        .recs
                                        .get(j + 1)
                                        .is_some_and(|n| n["ev"] == "ag_done" && n["path"] == key)
                            }
                        },
                    }),
                    "ag_done" => out.push(Delivery {
                        what: "ag_done",
                        seq: r["seq"].as_i64().unwrap_or(-1),
                        v: pts(&r["v"]),
                        err: r["err"].as_bool(),
                        cs: r["cs"].as_i64(),
                        searched: None,
                        raw: false,
                    }),
                    "path_fin" => out.push(Delivery {
                        what: "path_fin",
                        seq: r["seq"].as_i64().unwrap_or(-1),
                        v: pts(&r["raw"]),
                        err: r["err"].as_bool(),
                        cs: r["cs"].as_i64(),
                        searched: r["searched"].as_i64(),
                        raw: true,
                    }),
                    "path_mod" => out.push(Delivery {
                        what: "path_mod",
                        seq: r["seq"].as_i64().unwrap_or(-1),
                        v: pts(&r["v"]),
                        err: None,
                        cs: None,
                        searched: None,
                        raw: false,
                    }),
                    _ => {}
                }
            }
        }
        // The AI's ScorePath: FindPath, BlockUntilCalculated, then FixPathIfTooShort's tap
        // (before any other request).
        if fun == "find" {
            for j in i + 1..self.recs.len() {
                let r = &self.recs[j];
                if r["ev"] == "ag_req_end" || r["ev"] == "ag_req" {
                    break;
                }
                if r["ev"] == "uc_v" && r["f"] == "path" {
                    if r["fake"] == false {
                        out.push(Delivery {
                            what: "ai path",
                            seq: r["seq"].as_i64().unwrap_or(-1),
                            v: pts(&r["v"]),
                            err: r["err"].as_bool(),
                            cs: r["st"].as_i64(),
                            searched: None,
                            raw: false,
                        });
                    }
                    break;
                }
            }
        }
        out
    }

    /// The line-of-sight calls the path's search made, as `path_fin` recorded them.
    /// `None`: no `path_fin`; an error: a transcript that isn't one.
    fn recorded_los(&self, i: usize, key: &str) -> Result<Option<Vec<LosAnswer>>, String> {
        let Some(r) = self.recs[i + 1..].iter().find(|r| r["ev"] == "path_fin" && r["path"] == key)
        else {
            return Ok(None);
        };
        parse_los(&r["los"]).map(Some)
    }

    fn request(&mut self, i: usize) {
        let r = &self.recs[i];
        let rq = &r["rq"];
        let u = r["u"].as_str().unwrap_or("").to_owned();
        let fun = r["fn"].as_str().unwrap_or("").to_owned();
        let key = rq["path"].as_str().unwrap_or("").to_owned();
        let s = &r["s"];
        // PathTo can return early keeping its RequestedPath: `ag_req_end` then reports that
        // request again (same key, same fields); it was checked when it started.
        if let Some(first) = self.started.get(&key) {
            if *first != *rq {
                self.fail(
                    i,
                    format!("{key}: reported again with other fields ({first} then {rq})"),
                );
            }
            self.tally.retained += 1;
            return;
        }
        self.started.insert(key.clone(), rq.clone());
        if fun == "find" {
            self.last_find_checked.insert(u.clone(), false);
        }
        // The request as the agent built it.
        let clear = opt_v3(&rq["clear"]);
        let corp = match self.corp.get(&u) {
            Some(&c) => c,
            None => {
                let Some(c) = clear else {
                    self.fail(i, format!("{key}: no corpulence and no clearance"));
                    return;
                };
                self.tally.corpulence_from_request += 1;
                c[0] / 2.0
            }
        };
        let avoid_field = s["avoidoff"].as_bool();
        let Some(avoid_field) = avoid_field else {
            self.fail(i, format!("{key}: the agent's avoidance flag isn't recorded"));
            return;
        };
        let (Some(start), Some(end)) = (opt_v3(&rq["start"]), opt_v3(&rq["end"])) else {
            self.fail(i, format!("{key}: no endpoints"));
            return;
        };
        if opt_v3(&s["pos"]).is_none_or(|p| !same(p, start)) {
            self.fail(i, format!("{key}: start {start:?} isn't the agent's position {}", s["pos"]));
        }
        // The call's arguments (`ag_req`, its entry), and what the command or the AI passed
        // (fudge radius, the unit's speed: as the request recorded them).
        let Some(entry) = self.recs[..i]
            .iter()
            .rev()
            .find(|x| x["ev"] == "ag_req" && x["u"] == u.as_str() && x["fn"] == fun.as_str())
        else {
            self.fail(i, format!("{key}: no ag_req entry"));
            return;
        };
        if opt_v3(&entry["p"]).is_none_or(|p| !same(p, end)) {
            self.fail(
                i,
                format!("{key}: destination {} but the request ends at {end:?}", entry["p"]),
            );
        }
        let need_los =
            if fun == "find" { entry["los"].as_bool() } else { Some(!entry["da"].is_null()) };
        let (Some(need_los), Some(maxd)) = (need_los, opt_f(&rq["maxd"])) else {
            self.fail(i, format!("{key}: no line-of-sight flag or max distance"));
            return;
        };
        let agent = AgentRequest {
            find: fun == "find",
            start,
            destination: end,
            corpulence: corp,
            avoidance_disabled_field: avoid_field,
            approach_radius: opt_f(&entry["r"]),
            need_los,
            ignore_blocker_radius: if fun == "find" {
                f(&entry["ibr"])
            } else {
                opt_f(&rq["ibr"]).unwrap_or(0.0)
            },
            max_distance: maxd,
            distance_xz: fun != "find" && entry["xz"] == true,
        };
        let req = match agent.to_request(&self.nav, self.node_size) {
            Ok(q) => q,
            Err(e) => {
                self.fail(i, format!("{key}: {e:#}"));
                return;
            }
        };
        // The request against the one the game built.
        if opt_v3(&rq["clear"]).is_none_or(|c| !same(c, req.clearance)) {
            self.fail(i, format!("{key}: clearance {}, the sim {:?}", rq["clear"], req.clearance));
        }
        if rq["tags"].as_i64() != Some(i64::from(req.enabled_tags)) {
            self.fail(i, format!("{key}: tags {}, the sim {}", rq["tags"], req.enabled_tags));
        }
        if rq["gmask"].as_i64() != Some(i64::from(req.graph_mask)) {
            self.fail(i, format!("{key}: graph mask {}", rq["gmask"]));
            return;
        }
        let ending_same = match req.ending {
            Ending::Los { approach_radius, check_los, ignored_blocker_radius, max } => {
                rq["ec"] == "LosPathConditionForGrid"
                    && opt_f(&rq["ar"]).is_some_and(|a| a.to_bits() == approach_radius.to_bits())
                    && rq["los"].as_bool() == Some(check_los)
                    && opt_f(&rq["ibr"])
                        .is_some_and(|a| a.to_bits() == ignored_blocker_radius.to_bits())
                    && max.to_bits() == maxd.to_bits()
            }
            Ending::MaxDistance { max } => {
                rq["ec"] == "MaxDistanceEndingCondition" && max.to_bits() == maxd.to_bits()
            }
            Ending::Ab => false,
        };
        if !ending_same {
            self.fail(i, format!("{key}: ending {:?}, the game {}", req.ending, rq));
            return;
        }
        let dels = self.deliveries(i, &key, &fun);
        let recorded = match self.recorded_los(i, &key) {
            Ok(r) => r,
            Err(e) => {
                self.fail(i, format!("{key}: {e}"));
                return;
            }
        };
        // With observer `paths`, every returned path has its `path_fin`: one returned (by any
        // record of its return or callback) without it fails.
        let returned = self.recs[i + 1..].iter().any(|r| {
            r["path"] == key.as_str()
                && matches!(r["ev"].as_str(), Some("path_ret" | "ag_done" | "path_mod"))
        });
        if self.nav_paths && recorded.is_none() && (returned || !dels.is_empty()) {
            self.fail(i, format!("{key}: returned without a path_fin (observer paths declared)"));
            return;
        }
        let end_at = dels.iter().find(|d| !d.v.is_empty()).map(|d| d.v[d.v.len() - 1]);
        let world = &self.nav.world;
        let compute = |req: &wr_nav::query::TurnBasedRequest| {
            let mut los_mismatch = None;
            let got = if let Some(answers) = &recorded {
                // Each call checked against the recorded one, its answer taken.
                let mut at = 0;
                let mut los = |q: &LosQuery| {
                    let a = answers.get(at)?;
                    at += 1;
                    if !(same(a.from, q.from)
                        && same(a.to, q.to)
                        && a.fudge.to_bits() == q.fudge.to_bits())
                    {
                        los_mismatch
                            .get_or_insert(format!("line of sight {q:?}, the game asked {a:?}"));
                    }
                    Some(a.blocked)
                };
                let got = world.turn_based_path(req, &mut los);
                if at != answers.len() && los_mismatch.is_none() {
                    los_mismatch = Some(format!(
                        "{} line-of-sight calls, the game made {}",
                        at,
                        answers.len()
                    ));
                }
                got
            } else {
                let mut los = |q: &LosQuery| end_at.map(|e| !same(e, world.position_v3(q.node)));
                world.turn_based_path(req, &mut los)
            };
            (got, los_mismatch)
        };
        let (got, los_mismatch) = compute(&req);
        // PathTo sets distanceXZ after StartPath: a worker may have searched without it. Only a
        // request whose result doesn't depend on it is checked.
        if rq["xz"].as_bool() != Some(req.distance_xz) {
            self.fail(i, format!("{key}: distanceXZ {}, the sim {}", rq["xz"], req.distance_xz));
            return;
        }
        if req.distance_xz {
            let (alt, _) = compute(&wr_nav::query::TurnBasedRequest { distance_xz: false, ..req });
            if bits(&alt.vector_path) != bits(&got.vector_path)
                || bits(&alt.raw) != bits(&got.raw)
                || alt.search.state != got.search.state
                || alt.search.los_unknown != got.search.los_unknown
            {
                self.fail(i, format!("{key}: the result depends on when PathTo set distanceXZ"));
                return;
            }
        }
        if let Some(q) = got.search.los_unknown {
            self.tally.unrecorded_needs_los += 1;
            if !dels.is_empty() {
                self.fail(i, format!("{key}: line of sight needed at {q:?}"));
            }
            return;
        }
        if dels.is_empty() {
            self.tally.unrecorded += 1;
            return;
        }
        let err = got.search.state == State::Error;
        // With observer `paths`, the seeker's `OnPathComplete` says whether it took the path
        // (`path_seek`: a path superseded once computed returns without modifiers); a path it
        // took that didn't fail ran the modifiers, so its `path_mod` must be there.
        if self.nav_paths {
            let seek = self.recs[i + 1..]
                .iter()
                .find(|r| r["ev"] == "path_seek" && r["path"] == key.as_str());
            let Some(seek) = seek else {
                self.fail(
                    i,
                    format!("{key}: returned without a path_seek (observer paths declared)"),
                );
                return;
            };
            let (Some(accepted), Some(mods)) = (seek["accepted"].as_bool(), seek["mods"].as_bool())
            else {
                self.fail(i, format!("{key}: path_seek without its decision ({seek})"));
                return;
            };
            let modified = accepted && mods && !err;
            if modified != dels.iter().any(|d| d.what == "path_mod") {
                self.fail(
                    i,
                    format!(
                        "{key}: path_mod {}expected ({seek})",
                        if modified { "" } else { "not " }
                    ),
                );
                return;
            }
        }
        let cs = match got.search.state {
            State::NotCalculated => 0,
            State::Error => 1,
            State::Complete => 2,
            State::Partial => 3,
        };
        let mut bad: Vec<String> = los_mismatch.into_iter().collect();
        for d in &dels {
            if d.searched.is_some_and(|n| n != i64::from(got.search.searched_nodes)) {
                bad.push(format!(
                    "{}: searched {:?} nodes, the sim {}",
                    d.what, d.searched, got.search.searched_nodes
                ));
            }
            let want = if d.raw { &got.raw } else { &got.vector_path };
            if bits(want) != bits(&d.v) {
                bad.push(format!(
                    "{} (seq {}{}): the game {:?}, the sim {:?}",
                    d.what,
                    d.seq,
                    if d.raw { ", raw" } else { "" },
                    d.v,
                    want
                ));
            }
            if d.err.is_some_and(|e| e != err) || d.cs.is_some_and(|c| c != cs) {
                bad.push(format!(
                    "{}: error {:?} state {:?}, the sim {err} {cs} ({})",
                    d.what, d.err, d.cs, got.search.error
                ));
            }
        }
        self.tally.deliveries += dels.len();
        // An implied "blocked" answer would be the recording's end read back into the
        // search: only the answer at the node the path ended on is taken from it.
        let implied_blocked: Vec<NodeId> = if recorded.is_some() {
            Vec::new()
        } else {
            got.search.los.iter().filter(|(_, b)| *b).map(|(q, _)| q.node).collect()
        };
        if !implied_blocked.is_empty() {
            bad.push(format!(
                "line of sight asked at nodes {implied_blocked:?} before the recorded end (not \
                 recorded)"
            ));
        }
        if bad.is_empty() {
            if recorded.is_some() {
                self.tally.los_recorded += 1;
            } else if got.search.los.is_empty() {
                self.tally.exact += 1;
            } else {
                self.tally.los_implied += 1;
                for (_, blocked) in &got.search.los {
                    if *blocked {
                        self.tally.los_blocked_answers += 1;
                    } else {
                        self.tally.los_clear_answers += 1;
                    }
                }
            }
            // A FindPath whose returned path was compared (the AI's tap, or `path_mod` for any
            // caller): the paths a FollowPrecomputedPath must come from.
            if fun == "find" && dels.iter().any(|d| !d.raw) {
                self.found_paths.entry(u.clone()).or_default().push(got.vector_path.clone());
                self.last_find_checked.insert(u, true);
            }
        } else {
            let state = format!(
                "order {:?}, current {:?}, erode {}, searched {}, los {:?}",
                self.order,
                self.current,
                self.nav.erode_iterations(),
                got.search.searched_nodes,
                got.search.los.iter().map(|(q, b)| (q.node, *b)).collect::<Vec<_>>()
            );
            self.fail(i, format!("{key} ({fun} by {u}): {} [{state}]", bad.join("; ")));
        }
    }

    fn run(&mut self) {
        for i in 0..self.recs.len() {
            let r = &self.recs[i];
            let k = r["k"].as_str().unwrap_or("");
            let ev = r["ev"].as_str().unwrap_or("");
            // RemoveUnit returns before the next unit leaves, the tick ends or combat ends.
            if !self.leaving.is_empty()
                && (k == "tick" || ev == "leave_combat" || ev == "combat_end")
            {
                self.settle_leaving();
            }
            if k == "begin" {
                self.timer = f(&r["nav_timer0"]);
                self.nav_paths =
                    r["observers"].as_array().is_some_and(|a| a.iter().any(|o| o == "paths"));
                continue;
            }
            if self.nav_paths && k == "obs" && matches!(ev, "nav_guo" | "nav_update" | "nav_tags") {
                self.rec_nav.push_back(i);
            }
            if let Some(u) = r["u"].as_str()
                && let Some(a) = r["s"].get("avoidoff").and_then(Value::as_bool)
            {
                self.avoid.insert(u.to_owned(), a);
            }
            match (k, ev) {
                ("obs", "u") => {
                    if let (Some(u), Some(row)) =
                        (r["u"].as_str(), r["s"].as_str().and_then(Row::parse))
                    {
                        self.pos.insert(u.to_owned(), row.pos);
                    }
                }
                ("obs", "ag_ctl_end") => {
                    if let Some(u) = r["u"].as_str() {
                        self.pos.insert(u.to_owned(), v3(&r["pos"]));
                    }
                }
                ("obs", "uc_v") if r["f"] == "corp" => {
                    let (Some(u), Some(v)) =
                        (r["u"].as_str(), r["v"].as_str().and_then(|x| x.parse::<f32>().ok()))
                    else {
                        continue;
                    };
                    if let Some(old) = self.corp.insert(u.to_owned(), v)
                        && old.to_bits() != v.to_bits()
                    {
                        self.fail(i, format!("{u}'s corpulence changed {old} -> {v}"));
                    }
                }
                ("tb", "combat_start") => {
                    // HandleCombatStart: erosion around blocked cells only, Clear, the units
                    // in combat.
                    if let Some(g) = self.nav.world.grid.as_mut() {
                        g.erosion_only_around_cells_with_tag = 31;
                    }
                    self.clear(i);
                    self.order = units_of(r);
                    self.initialized = true;
                }
                ("tb", "sort") => self.order = units_of(r),
                ("obs", "join_combat") if self.initialized => {
                    if let Some(u) = r["u"].as_str()
                        && !self.order.iter().any(|x| x == u)
                    {
                        self.order.push(u.to_owned());
                    }
                }
                ("obs", "leave_combat") if self.initialized => {
                    if let Some(u) = r["u"].as_str() {
                        let u = u.to_owned();
                        self.remove_unit(i, &u);
                    }
                }
                ("obs", "choose_next") => {
                    let theirs: Vec<String> = r["units"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str())
                                .map(|x| x.split('|').next().unwrap_or("").to_owned())
                                .collect()
                        })
                        .unwrap_or_default();
                    if theirs != self.order {
                        self.fail(i, format!("turn order {theirs:?}, ours {:?}", self.order));
                    }
                    let saddled = r["units"].as_array().is_some_and(|a| {
                        a.iter().any(|x| x.as_str().is_some_and(|s| !s.ends_with("saddled=0")))
                    });
                    if saddled {
                        self.fail(i, "a mount in the turn order (not modelled)".into());
                    }
                }
                ("tb", "turn_start_begin") => {
                    if let Some(u) = r["unit"]["$unit"].as_str() {
                        let u = u.to_owned();
                        // A turn's FollowPrecomputedPath comes from that turn's FindPaths.
                        self.found_paths.remove(&u);
                        self.start_turn(i, &u);
                    }
                }
                ("tb", "turn_clear") => self.current = None,
                ("tb", "combat_end") => {
                    // HandleCombatEnd: Clear, then the (emptied) order's reverts and another
                    // refresh without a turn.
                    self.clear(i);
                    if let Err(e) = self.nav.update_navigation_grid_tags(None) {
                        self.fail(i, format!("combat end: {e:#}"));
                    }
                    self.tally.tag_refreshes += 1;
                    self.initialized = false;
                }
                ("obs", "nav_clock") => {
                    // UpdateNavigationGrid: `m_NavmeshUpdateTimer -= Time.unscaledTime` is read
                    // only while the timer is not negative (no navmesh cut updated here).
                    if self.timer < 0.0 {
                        self.fail(i, "navigation clock read with the timer negative".into());
                    }
                    self.timer = r_(d(self.timer) - r["v"].as_f64().unwrap_or(f64::NAN));
                    if self.timer <= 0.0 {
                        let turn = self.current_turn(i);
                        if let Err(e) = self.nav.update_navigation_grid_tags(turn.as_ref()) {
                            self.fail(i, format!("UpdateNavigationGridTags: {e:#}"));
                        }
                        self.tally.tag_refreshes += 1;
                    }
                }
                ("obs", "ag_req_end") if r["rq"]["kind"] == "ABPathWithClearance" => {
                    self.request(i)
                }
                ("obs", "ag_req_end") if !r["rq"].is_null() && self.initialized => {
                    self.fail(i, format!("a {} request in turn-based combat", r["rq"]["kind"]));
                }
                ("obs", "uc_call") if r["fn"] == "follow" => {
                    let u = r["u"].as_str().unwrap_or("").to_owned();
                    let v = pts(&r["v"]);
                    let known = self
                        .found_paths
                        .get(&u)
                        .is_some_and(|l| l.iter().any(|p| bits(p) == bits(&v)));
                    // A path cut short (AiAction.ScorePath's movement simulation, the path
                    // visualizer's movement range) isn't computed here: it fails like any other
                    // path that isn't a computed one.
                    if self.nav_paths && self.last_find_checked.get(&u) != Some(&true) {
                        // With observer `paths` every FindPath's result is recorded.
                        self.fail(
                            i,
                            format!("FollowPrecomputedPath of {u}: its FindPath wasn't compared"),
                        );
                    } else if self.last_find_checked.get(&u) != Some(&true) {
                        // The last FindPath's result wasn't recorded (the player's, before
                        // observer `paths`). Its candidates are spent all the same.
                        self.tally.forced_unchecked += 1;
                        self.found_paths.remove(&u);
                    } else if known {
                        self.tally.forced_equal += 1;
                        // Its candidates are spent: the next follow comes from new FindPaths.
                        self.found_paths.remove(&u);
                    } else {
                        self.fail(
                            i,
                            format!("FollowPrecomputedPath of {u}: {v:?} isn't a computed path"),
                        );
                    }
                }
                _ => {}
            }
            if self.nav_paths {
                self.match_nav();
            }
        }
        if self.nav_paths {
            let left_rec = self.rec_nav.len();
            let left_ours = self.nav.log.len() - self.log_at;
            if left_rec + left_ours > 0 {
                let last = self.recs.len() - 1;
                self.fail(
                    last,
                    format!("graph events left: {left_rec} recorded, {left_ours} the sim's"),
                );
            }
        }
    }
}

/// `path_fin`'s `los`: each call `[from, to, fudge, result]` (null: none). Anything else is an
/// error, never an answer.
fn parse_los(los: &Value) -> Result<Vec<LosAnswer>, String> {
    let none = Vec::new();
    let calls = if los.is_null() {
        &none
    } else {
        los.as_array().ok_or_else(|| format!("path_fin los {los}"))?
    };
    let vec = |v: &Value| -> Option<V3> {
        let a = v.as_array().filter(|a| a.len() == 3)?;
        let p = [a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?].map(|x| x as f32);
        p.iter().all(|x| x.is_finite()).then_some(p)
    };
    calls
        .iter()
        .map(|c| {
            let call = || -> Option<LosAnswer> {
                let a = c.as_array().filter(|a| a.len() == 4)?;
                let fudge = a[2].as_f64()? as f32;
                Some(LosAnswer {
                    from: vec(&a[0])?,
                    to: vec(&a[1])?,
                    fudge: fudge.is_finite().then_some(fudge)?,
                    blocked: a[3].as_bool()?,
                })
            };
            call().ok_or_else(|| format!("line-of-sight call {c} isn't [from, to, fudge, result]"))
        })
        .collect()
}

#[test]
fn damaged_line_of_sight_transcripts_are_errors() {
    let ok = serde_json::json!([[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], 0.5, true]]);
    let got = parse_los(&ok).unwrap();
    assert_eq!(got.len(), 1);
    assert!(got[0].blocked && got[0].fudge == 0.5 && got[0].to == [4.0, 5.0, 6.0]);
    assert!(parse_los(&Value::Null).unwrap().is_empty());
    for bad in [
        serde_json::json!({}),
        serde_json::json!([[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], 0.5]]),
        serde_json::json!([[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], 0.5, null]]),
        serde_json::json!([[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], 0.5, 0]]),
        serde_json::json!([[[1.0, 2.0], [4.0, 5.0, 6.0], 0.5, false]]),
        serde_json::json!([[[1.0, 2.0, 3.0], [4.0, "x", 6.0], 0.5, false]]),
        serde_json::json!([[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0], null, false]]),
        serde_json::json!([null]),
    ] {
        assert!(parse_los(&bad).is_err(), "{bad} accepted");
    }
}

fn r_(x: f64) -> f32 {
    x as f32
}

fn units_of(r: &Value) -> Vec<String> {
    r["units"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| x["unit"].as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

#[test]
#[ignore = "needs poc5-tb-103, the caves' nav file and Manual_18"]
fn every_turn_based_path_is_computed_bit_for_bit() {
    let nav_file =
        wr_nav::NavFile::read(&wr_nav::path_for(CAVES)).unwrap_or_else(|e| panic!("{e:#}"));
    let node_size = nav_file.world.grid.as_ref().expect("grid").node_size;
    let mut failures = Vec::new();
    let mut total = Tally::default();
    let mut world: Option<crate::world::World> = None;
    let mut enemies: HashMap<(String, String), Option<bool>> = HashMap::new();
    for (label, path) in captures() {
        let trace = Trace::load(&path).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
        if world.is_none() {
            let save =
                trace.records[0]["save_file"].as_str().unwrap_or("").trim_end_matches(".zks");
            let dir = path.parent().unwrap();
            let run =
                path.file_name().unwrap().to_str().unwrap().trim_end_matches(".trace.jsonl.gz");
            let (w, _) = crate::combat::replay::prepare_world(save, dir, run)
                .unwrap_or_else(|e| panic!("{save}: {e:#}"));
            world = Some(w);
        }
        let w = world.as_mut().unwrap();
        let mut enemy = |a: &str, b: &str| {
            *enemies.entry((a.to_owned(), b.to_owned())).or_insert_with(|| {
                let (Some(&x), Some(&y)) = (w.by_id.get(a), w.by_id.get(b)) else { return None };
                let before = w.coverage.unsupported.len();
                let e = crate::combat::units::is_enemy(w, x, y);
                (w.coverage.unsupported.len() == before).then_some(e)
            })
        };
        let mut fight = Fight {
            label: label.clone(),
            recs: &trace.records,
            nav: CombatNav::new(nav_file.world.clone()).unwrap_or_else(|e| panic!("{e:#}")),
            node_size,
            pos: HashMap::new(),
            corp: HashMap::new(),
            avoid: HashMap::new(),
            order: Vec::new(),
            initialized: false,
            current: None,
            timer: 0.0,
            enemy: &mut enemy,
            failures: Vec::new(),
            tally: Tally::default(),
            found_paths: HashMap::new(),
            leaving: Vec::new(),
            last_find_checked: HashMap::new(),
            started: HashMap::new(),
            nav_paths: false,
            rec_nav: VecDeque::new(),
            log_at: 0,
            guo_ids: HashMap::new(),
        };
        fight.run();
        eprintln!("{label}: {:?}", fight.tally);
        failures.extend(fight.failures);
        let t = fight.tally;
        total.exact += t.exact;
        total.los_implied += t.los_implied;
        total.los_blocked_answers += t.los_blocked_answers;
        total.los_clear_answers += t.los_clear_answers;
        total.unrecorded += t.unrecorded;
        total.unrecorded_needs_los += t.unrecorded_needs_los;
        total.deliveries += t.deliveries;
        total.forced_equal += t.forced_equal;
        total.forced_unchecked += t.forced_unchecked;
        total.turn_starts += t.turn_starts;
        total.removals += t.removals;
        total.tag_refreshes += t.tag_refreshes;
        total.corpulence_from_request += t.corpulence_from_request;
        total.los_recorded += t.los_recorded;
        total.nav_events += t.nav_events;
        total.retained += t.retained;
    }
    eprintln!("total: {total:?}");
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    assert!(total.exact + total.los_implied + total.los_recorded > 0, "nothing checked");
}
