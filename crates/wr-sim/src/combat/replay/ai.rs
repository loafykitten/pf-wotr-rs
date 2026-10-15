//! The driven replay's side of the AI ([`crate::combat::ai`]): WrathOracle's observer `ai`. An
//! entry record (`uc_ai`: `AiBrainController.Tick`, or `ForceTick` from the turn controller)
//! starts the sim's AI, which pulls the leaves it reads (`uc_v`), its draws, the commands it
//! runs and its checks (`ai_sc`, `ai_best`, `ai_sel`) in order, as the commands model does
//! ([`super::commands`]).

use serde_json::Value as J;
use wr_content::Guid;

use super::Driver;
use super::commands::{describe, id, is_unit, obs, opt_id, opt_str, phase_of, points};
use crate::combat::ai::{
    self, AiInputs, BestCheck, GroupRow, NewCommand, Path, ScoreCheck, SimStep,
};
use crate::combat::commands::R;
use crate::world::World;

/// A `float` written round-trip as a string (finite).
fn float(v: &J) -> Option<f32> {
    v.as_str().and_then(|s| s.parse::<f32>().ok()).filter(|f| f.is_finite())
}

/// A blueprint written as `BlueprintGuid.ToString()` (32 lowercase digits).
fn guid(v: &J) -> Option<Option<Guid>> {
    match v {
        J::Null => Some(None),
        J::String(s) => Guid::parse(s).filter(|g| g.to_string() == *s).map(Some),
        _ => None,
    }
}

/// A unit the sim has, or null.
fn unit_or_null(w: &World, v: &J) -> Option<Option<String>> {
    match v {
        J::Null => Some(None),
        J::String(s) if is_unit(w, s) => Some(Some(s.clone())),
        _ => None,
    }
}

/// A C# `int`.
fn int(v: &J) -> Option<i64> {
    v.as_i64().filter(|x| i32::try_from(*x).is_ok())
}

fn same_points(a: Option<&[[f32; 3]]>, b: Option<&[[f32; 3]]>) -> bool {
    let bits = |p: &[[f32; 3]]| p.iter().flat_map(|x| x.map(f32::to_bits)).collect::<Vec<_>>();
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => bits(a) == bits(b),
        _ => false,
    }
}

/// A recorded path: null, or points.
fn path_points(v: &J) -> Option<Option<Vec<[f32; 3]>>> {
    match v {
        J::Null => Some(None),
        v => points(v).map(Some),
    }
}

/// A two-point path `FixPathIfTooShort` keeps: `vector = p[0] - p[1]` (stored as `float`s),
/// `vector.x * vector.x + vector.z * vector.z > 0.01f` in double.
fn kept_two_points(p: &[[f32; 3]]) -> bool {
    #[allow(clippy::cast_possible_truncation)]
    let d = |i: usize| (f64::from(p[0][i]) - f64::from(p[1][i])) as f32;
    let (x, z) = (f64::from(d(0)), f64::from(d(2)));
    x * x + z * z > f64::from(0.01f32)
}

/// Exactly these fields besides the envelope (`seq`, `k`, `t`, `ev`).
fn fields(r: &J, payload: &[&str]) -> bool {
    const BASE: [&str; 4] = ["seq", "k", "t", "ev"];
    r.as_object().is_some_and(|o| {
        o.len() == BASE.len() + payload.len()
            && BASE.iter().chain(payload).all(|k| o.contains_key(*k))
    })
}

impl Driver<'_> {
    /// A draw of `SelectAction` (with the `ai` observer a lifecycle record: the sim's draw takes it).
    pub(super) fn ai_draw_record(&self, r: &J) -> bool {
        self.ai
            && r["k"] == "draw"
            && (r["site"].as_i64().is_some_and(|s| self.ai_sites.contains(&s))
                || self.trace.site_key(r).as_deref()
                    == Some(crate::rng::sites::AI_NEXT_COMMAND_TIME.0))
    }

    /// `uc_ai` from outside the AI (`AiBrainController.Tick`, the turn controller's `ForceTick`),
    /// placed by its caller.
    pub(super) fn ai_entry(&mut self, w: &mut World, i: usize) -> usize {
        self.tgt_runs.clear();
        let trace = self.trace;
        let r = &trace.records[i];
        self.i = i + 1;
        let f = r["fn"].as_str().filter(|f| *f == "Tick" || *f == "ForceTick");
        let ok = self.ai
            && fields(r, &["fn", "nested", "origin", "limited"])
            && f.is_some()
            && r["limited"].is_boolean()
            && matches!(opt_str(r, "origin"), Some(Some(_)));
        let Some(f) = f.filter(|_| ok) else {
            w.coverage.unsupported(if self.ai {
                "driven replay: a uc_ai record it can't read"
            } else {
                "driven replay: an AI entry without the ai observer"
            });
            return self.i;
        };
        let origin = r["origin"].as_str().unwrap_or("");
        // `Tick` is the controller's own (`AiBrainController.Tick` from `GameMode.Tick`).
        // Its own frame first, then its caller: `Tick` the controller's (from `GameMode.Tick`),
        // `ForceTick` the turn controller's (checked below).
        let mut frames = origin.split(" < ");
        let own = frames.next() == Some(format!("Kingmaker.AI.AiBrainController.{f}").as_str());
        let caller_ok = f != "Tick" || frames.next() == Some("Kingmaker.GameModes.GameMode.Tick");
        let placed = phase_of(origin)
            .filter(|p| (f == "Tick") == (*p == super::Phase::Ai))
            .filter(|_| own && caller_ok);
        match placed {
            Some(p) => self.place(w, p, "the AI"),
            None => w
                .coverage
                .unsupported("driven replay: the AI called from a source the replay can't place"),
        }
        w.combat.ai.limited = r["limited"] == true;
        if f == "Tick" {
            // One AI controller tick a frame (checked at the frame's end).
            if self.ai_tick_frame.replace(self.tick) == Some(self.tick) {
                w.coverage.unsupported("driven replay: two AI controller ticks in a frame");
            }
        } else {
            // The turn controller's: from its tick (its forced AI tick) or `Prepare`.
            match origin.split(" < ").nth(1) {
                // Within the turn controller's tick (its start seen, its status change not yet).
                Some("TurnBased.Controllers.TurnController.Tick") if self.tt_open => {
                    self.ai_forced += 1;
                }
                Some("TurnBased.Controllers.TurnController.Prepare") => {
                    self.ai_prepared += 1;
                    // (After `Prepare`'s `UpdatePriorityTarget`, from the same chain.)
                    let tail = origin.strip_prefix("Kingmaker.AI.AiBrainController.ForceTick < ");
                    if self.imp
                        && (self.imp_prepared.is_empty()
                            || self.imp_prepare_tail.take().as_deref() != tail)
                    {
                        w.coverage.unsupported(
                            "driven replay: TurnController.Prepare's ForceTick before its UpdatePriorityTarget, or from another chain",
                        );
                    }
                }
                Some("TurnBased.Controllers.TurnController.Tick") => w.coverage.unsupported(
                    "driven replay: a forced AI tick outside the turn controller's tick",
                ),
                _ => w
                    .coverage
                    .unsupported("driven replay: ForceTick from a caller the replay doesn't know"),
            }
            self.ai_force_origin = origin.to_owned();
        }
        let outer = std::mem::replace(&mut self.uc_origin, origin.to_owned());
        self.uc_active += 1;
        self.out.ai_entries += 1;
        let res = (|| -> R<()> {
            if f == "Tick" {
                ai::tick(w, self)?;
            } else {
                ai::force_tick(w, self)?;
            }
            let at = self.uc_pull(w, "the AI's end", |x| obs(x, "uc_ai_end").is_some())?;
            let e = &self.trace.records[at];
            if !fields(e, &["fn", "exc"]) || e["fn"] != f || e["exc"] != false {
                return self.uc_reject(at, "an AI's end of another call, or an exception");
            }
            Ok(())
        })();
        self.uc_active -= 1;
        self.uc_origin = outer;
        if let Err(d) = res {
            self.uc_desync(w, &d);
            self.uc_skip(w, |x| obs(x, "uc_ai_end").is_some() && x["fn"] == f);
            // The static context's state is the game's again only after a release: start afresh
            // (what outlives a decision unknown again).
            w.combat.ai.ctx = ai::Context::default();
        }
        self.i
    }

    /// The next leaf `f`: its record (exactly its writer's fields).
    fn ai_leaf(&mut self, w: &mut World, f: &'static str, payload: &[&str]) -> R<usize> {
        self.uc_leaf_record(w, f, payload)
    }

    fn ai_unit_check(&mut self, at: usize, f: &str, u: Option<&str>) -> R<()> {
        if let Some(u) = u
            && self.trace.records[at]["u"].as_str() != Some(u)
        {
            let d = describe(&self.trace.records[at]);
            return self.uc_reject(at, format!("the sim reads {f} of {u}, the recording has {d}"));
        }
        Ok(())
    }

    fn ai_check(&mut self, w: &mut World, ev: &str, payload: &[&str]) -> R<usize> {
        let at = self.uc_pull(w, &format!("the AI's {ev}"), |r| obs(r, ev).is_some())?;
        if !fields(&self.trace.records[at], payload) {
            return self
                .uc_reject(at, format!("an {ev} record with fields its writer doesn't write"));
        }
        Ok(at)
    }
}

/// A leaf's payload: with or without its unit.
fn with_unit(u: Option<&str>) -> &'static [&'static str] {
    if u.is_some() { &["u", "v"] } else { &["v"] }
}

impl Driver<'_> {
    /// An `ai_nct` record outside the AI: a write of `NextCommandTime` by another writer (an input).
    pub(super) fn ai_nct_input(&mut self, w: &mut World, r: &J) {
        if !self.ai {
            w.coverage.unsupported("driven replay: an ai_nct record without the ai observer");
            return;
        }
        let unit =
            r["u"].as_str().and_then(|u| w.by_id.get(u).copied()).filter(|e| w.unit(*e).is_some());
        let site = r["site"].as_str();
        match (fields(r, &["u", "site", "v"]), unit, site, float(&r["v"])) {
            (true, Some(_), Some("Engage" | "Disengage"), Some(_))
                if self.eng && !self.uc_skipping =>
            {
                w.coverage.unsupported(
                    "driven replay: an engagement's write of NextCommandTime the sim didn't make",
                );
            }
            (true, Some(_), Some("UpdateCombatTarget"), Some(_))
                if self.tgt && !self.uc_skipping =>
            {
                w.coverage.unsupported(
                    "driven replay: an UpdateCombatTarget write of NextCommandTime the sim didn't make",
                );
            }
            (
                true,
                Some(u),
                Some("Engage" | "Disengage" | "UpdateCombatTarget" | "set_AutoUseAbility"),
                // (Each of them writes `0f`.)
                Some(v),
            ) if v.to_bits() == 0 => {
                w.combat.ai.nct.insert(u, v);
            }
            (true, Some(_), Some("SelectAction"), Some(_)) if self.uc_skipping => {}
            (true, Some(_), Some("SelectAction"), Some(_)) => {
                w.coverage.unsupported(
                    "driven replay: an AI write of NextCommandTime the sim's AI didn't make",
                );
            }
            _ => w.coverage.unsupported("driven replay: an ai_nct record it can't read"),
        }
    }
}

impl AiInputs for Driver<'_> {
    fn ai_nct_write(&mut self, w: &mut World, u: &str, v: f32) -> R<()> {
        let at = self.uc_pull(w, "SelectAction's NextCommandTime", |r| {
            obs(r, "ai_nct").is_some() && r["site"] == "SelectAction"
        })?;
        let r = &self.trace.records[at];
        if !fields(r, &["u", "site", "v"])
            || r["u"].as_str() != Some(u)
            || float(&r["v"]).map(f32::to_bits) != Some(v.to_bits())
        {
            let d = describe(r);
            return self.uc_reject(
                at,
                format!(
                    "SelectAction sets {u}'s NextCommandTime to {v}, the recording has {d} v={}",
                    r["v"]
                ),
            );
        }
        Ok(())
    }

    fn ai_bool(&mut self, w: &mut World, f: &'static str, u: Option<&str>) -> R<bool> {
        let at = self.ai_leaf(w, f, with_unit(u))?;
        self.ai_unit_check(at, f, u)?;
        match self.trace.records[at]["v"].as_bool() {
            Some(v) => Ok(v),
            None => self.uc_reject(at, format!("a leaf {f} it can't read")),
        }
    }

    fn ai_float(&mut self, w: &mut World, f: &'static str, u: Option<&str>) -> R<f32> {
        let at = self.ai_leaf(w, f, with_unit(u))?;
        self.ai_unit_check(at, f, u)?;
        // `UnitEntityView.Corpulence`: at least 0.5 (`Mathf.Max(..., 0.5f)`).
        // A magnitude, a speed or a length never negative.
        let v = float(&self.trace.records[at]["v"]);
        // `Time.time` is one value through a frame, and never goes back.
        if f == "time"
            && let Some(v) = v
        {
            match self.ai_time {
                Some((t, x)) if t == self.tick && x.to_bits() != v.to_bits() => {
                    return self.uc_reject(at, "two values of Time.time in one frame");
                }
                Some((_, x)) if v < x => return self.uc_reject(at, "Time.time going back"),
                _ => self.ai_time = Some((self.tick, v)),
            }
        }
        let min = match f {
            "corp" => 0.5,
            "mag" | "speed" | "plen" => 0.0,
            _ => f32::NEG_INFINITY,
        };
        match v.filter(|v| *v >= min) {
            Some(v) => Ok(v),
            None => self.uc_reject(at, format!("a leaf {f} it can't read")),
        }
    }

    fn ai_unit(&mut self, w: &mut World, f: &'static str, u: Option<&str>) -> R<Option<String>> {
        let at = self.ai_leaf(w, f, with_unit(u))?;
        self.ai_unit_check(at, f, u)?;
        match unit_or_null(w, &self.trace.records[at]["v"]) {
            Some(v) => {
                if let Some(u) = u {
                    self.tgt_read(w, at, f, u, v.as_deref())?;
                }
                Ok(v)
            }
            None => {
                self.uc_reject(at, format!("a leaf {f} it can't read (or a unit the sim lacks)"))
            }
        }
    }

    fn ai_blueprint(&mut self, w: &mut World, f: &'static str, u: &str) -> R<Option<Guid>> {
        let at = self.ai_leaf(w, f, &["u", "v"])?;
        self.ai_unit_check(at, f, Some(u))?;
        match guid(&self.trace.records[at]["v"]) {
            Some(v) => Ok(v),
            None => self.uc_reject(at, format!("a leaf {f} it can't read")),
        }
    }

    fn ai_dist(&mut self, w: &mut World, from: &str, to: &str) -> R<f32> {
        let at = self.ai_leaf(w, "dist", &["u", "to", "v"])?;
        let r = &self.trace.records[at];
        // `MechanicsDistance`: never negative.
        let v = float(&r["v"]).filter(|d| *d >= 0.0);
        match (r["u"].as_str() == Some(from) && r["to"].as_str() == Some(to), v) {
            (true, Some(v)) => Ok(v),
            _ => {
                let d = describe(r);
                self.uc_reject(
                    at,
                    format!("the sim reads the distance {from} to {to}, the recording has {d}"),
                )
            }
        }
    }

    fn ai_range(&mut self, w: &mut World, action: Guid) -> R<f32> {
        let at = self.ai_leaf(w, "range", &["a", "v"])?;
        let r = &self.trace.records[at];
        match (guid(&r["a"]) == Some(Some(action)), float(&r["v"])) {
            (true, Some(v)) => Ok(v),
            _ => self.uc_reject(
                at,
                format!("the sim reads action {action}'s range, the recording has another"),
            ),
        }
    }

    fn ai_diff(&mut self, w: &mut World, option: i64) -> R<i64> {
        let at = self.ai_leaf(w, "diff", &["o", "v"])?;
        let r = &self.trace.records[at];
        // `CurrentDifficultyStatAdjustmentCompareTo`: -1, 0 or 1.
        match (int(&r["o"]) == Some(option), int(&r["v"]).filter(|v| (-1..=1).contains(v))) {
            (true, Some(v)) => Ok(v),
            _ => self
                .uc_reject(at, "a difficulty comparison of another option, or one it can't read"),
        }
    }

    fn ai_movement_range(&mut self, w: &mut World, u: &str) -> R<f32> {
        let at = self.ai_leaf(w, "mrange", &["u", "total", "single", "v"])?;
        let r = &self.trace.records[at];
        match (
            r["u"].as_str() == Some(u) && r["total"] == true && r["single"] == false,
            // (A step's remainder or a time times a speed: never negative.)
            float(&r["v"]).filter(|v| *v >= 0.0),
        ) {
            (true, Some(v)) => Ok(v),
            _ => {
                self.uc_reject(at, "a movement range of another unit or call, or one it can't read")
            }
        }
    }

    fn ai_path(&mut self, w: &mut World) -> R<Path> {
        let at = self.ai_leaf(w, "path", &["fake", "err", "st", "v"])?;
        let r = &self.trace.records[at];
        // A path (FixPathIfTooShort always leaves one): its state a `PathCompleteState` (0 to 3),
        // `error` exactly `CompleteState == Error` (1).
        match (
            r["fake"].as_bool(),
            r["err"].as_bool(),
            int(&r["st"]).filter(|s| (0..=3).contains(s)),
            points(&r["v"]),
        ) {
            // `ABPath.FakePath` of two points, Complete.
            (Some(fake), Some(error), Some(state), Some(points))
                if error == (state == 1)
                    && points.len() >= 2
                    && (!fake || (state == 2 && points.len() == 2))
                    && (fake || points.len() > 2 || kept_two_points(&points)) =>
            {
                // The sim's path of its FindPath, when it computes the paths.
                self.nav_ai_path(fake, error, state, &points)?;
                Ok(Path { error, state, points })
            }
            _ => self.uc_reject(at, "a path it can't read"),
        }
    }

    fn ai_close(&mut self, w: &mut World, radius: f32) -> R<bool> {
        let at = self.ai_leaf(w, "pclose", &["r", "v"])?;
        let r = &self.trace.records[at];
        match (float(&r["r"]).map(f32::to_bits) == Some(radius.to_bits()), r["v"].as_bool()) {
            (true, Some(v)) => Ok(v),
            _ => self.uc_reject(
                at,
                format!("IsUnitCloseEnough with the radius {radius}: the recording has another"),
            ),
        }
    }

    fn ai_sim_step(&mut self, w: &mut World, max: f32) -> R<SimStep> {
        let at = self.ai_leaf(w, "msim", &["max", "v", "p", "len", "rmd", "wi"])?;
        let r = &self.trace.records[at];
        let p = points(&J::Array(vec![r["p"].clone()])).and_then(|v| v.first().copied());
        match (
            float(&r["max"]).map(f32::to_bits) == Some(max.to_bits()),
            r["v"].as_bool(),
            p,
            float(&r["len"]),
            r["rmd"].as_bool(),
            int(&r["wi"]).and_then(|x| i32::try_from(x).ok()),
        ) {
            (
                true,
                Some(ret),
                Some(position),
                Some(last_move_length),
                Some(reached_max),
                Some(waypoint),
            ) => Ok(SimStep { ret, position, last_move_length, reached_max, waypoint }),
            _ => self.uc_reject(
                at,
                format!("a movement simulation step of {max}: the recording has another"),
            ),
        }
    }

    fn ai_blocked(&mut self, w: &mut World) -> R<i64> {
        let at = self.ai_leaf(w, "ablk", &["v"])?;
        // `sbyte AllyBlocked`; no node: the game throws.
        match self.trace.records[at]["v"].as_i64().filter(|x| i8::try_from(*x).is_ok()) {
            Some(v) => Ok(v),
            None => self.uc_reject(at, "a node's AllyBlocked it can't read (or no node)"),
        }
    }

    fn ai_aoo(&mut self, w: &mut World) -> R<i64> {
        let at = self.ai_leaf(w, "aoo", &["v"])?;
        match int(&self.trace.records[at]["v"]).filter(|x| *x >= 0) {
            Some(v) => Ok(v),
            None => self.uc_reject(at, "an attack-of-opportunity count it can't read"),
        }
    }

    fn ai_enemies(&mut self, w: &mut World) -> R<Vec<String>> {
        let at = self.ai_leaf(w, "enemies", &["v"])?;
        let v: Option<Vec<String>> = self.trace.records[at]["v"].as_array().and_then(|a| {
            a.iter().map(|x| x.as_str().filter(|s| is_unit(w, s)).map(str::to_owned)).collect()
        });
        // (`UnitGroupMemory` keys its units: each once.)
        let unique = |v: &Vec<String>| v.iter().enumerate().all(|(i, x)| !v[..i].contains(x));
        match v.filter(unique) {
            Some(v) => Ok(v),
            None => self.uc_reject(
                at,
                "a memory's enemies it can't read (a unit the sim lacks, or one twice)",
            ),
        }
    }

    fn ai_groups(&mut self, w: &mut World) -> R<Vec<GroupRow>> {
        let at = self.ai_leaf(w, "groups", &["v"])?;
        let row = |x: &J| -> Option<GroupRow> {
            let [id, units, in_combat] = x.as_array()?.as_slice() else { return None };
            Some(GroupRow {
                id: match id {
                    J::Null => None,
                    J::String(s) => Some(s.clone()),
                    _ => return None,
                },
                units: units
                    .as_array()?
                    .iter()
                    .map(|u| u.as_str().map(str::to_owned))
                    .collect::<Option<_>>()?,
                in_combat: in_combat.as_bool()?,
            })
        };
        match self.trace.records[at]["v"].as_array().and_then(|a| a.iter().map(row).collect()) {
            Some(v) => Ok(v),
            None => self.uc_reject(at, "the AI's groups it can't read"),
        }
    }

    fn ai_draw(&mut self, w: &mut World) -> R<f32> {
        let site = crate::rng::sites::AI_NEXT_COMMAND_TIME;
        let ids = self.ai_sites.clone();
        let at = self.uc_pull(w, "SelectAction's draw", |r| {
            r["k"] == "draw"
                && (r["site"].as_i64().is_some_and(|s| ids.contains(&s))
                    || r["key"].as_str() == Some(site.0))
        })?;
        let r = &self.trace.records[at];
        // A simulation draw outside rules, of `Range(0f, 0.1f)` at the site, as `Rng` writes it.
        let ok = self.trace.site_key(r).as_deref() == Some(site.0)
            && r.get("key").is_none()
            && r["c"] == "sim"
            && r["m"] == "Range_ff"
            && r["rule"] == 0
            && r["h"] == 0
            && r.get("ctx").is_none()
            && r.get("forced").is_some() == self.replay_mode
            && super::commands::draw_ok(r, &self.trace.sites)
            && r["a"].as_array().is_some_and(|a| {
                #[allow(clippy::cast_possible_truncation)]
                let f = |x: &J| x.as_f64().map(|v| (v as f32).to_bits());
                a.len() == 2
                    && f(&a[0]) == Some(0.0f32.to_bits())
                    && f(&a[1]) == Some(0.1f32.to_bits())
            });
        if !ok {
            let d = describe(r);
            return self.uc_reject(at, format!("SelectAction's draw: the recording has {d}"));
        }
        let v = w.rng.range_float(site, 0.0, 0.1);
        #[allow(clippy::cast_possible_truncation)]
        let rec = r["v"].as_f64().map(|x| x as f32);
        if rec.map(f32::to_bits) != Some(v.to_bits()) {
            return self.uc_reject(
                at,
                format!("SelectAction's draw {v} differs from the recorded {rec:?}"),
            );
        }
        self.out.ai_draws += 1;
        Ok(v)
    }

    fn ai_new_run(&mut self, w: &mut World, set: u32, spec: &NewCommand) -> R<u32> {
        let at =
            self.uc_pull(w, "the AI's Run", |r| obs(r, "uc_run").is_some() && r["nested"] == true)?;
        let r = &self.trace.records[at];
        let Some(c) = id(&r["c"]) else { return self.uc_reject(at, "a Run it can't read") };
        // Its description comes right before it (WrathOracle numbers a command at first sight).
        let described = at > 0
            && obs(&self.trace.records[at - 1], "uc_new").is_some()
            && id(&self.trace.records[at - 1]["c"]) == Some(c);
        // `RunPrefix`'s fields; called by `SelectAction` within the active `ForceTick`.
        let origin = format!(
            "Kingmaker.UnitLogic.Commands.UnitCommands.Run < Kingmaker.UnitLogic.Commands.UnitCommands.Run < Kingmaker.AI.AiBrainController.SelectAction < Kingmaker.AI.AiBrainController.TickBrain < {}",
            self.ai_force_origin
        );
        let st = &w.combat.commands.sets[&set];
        if !described
            || !fields(r, &["set", "u", "c", "fq", "ncq", "prev", "group", "nested", "origin"])
            || r["origin"].as_str() != Some(origin.as_str())
            || id(&r["set"]) != Some(set)
            || r["fq"] != false
            || r["ncq"] != false
            || opt_id(r, "prev") != Some(st.prev)
            || r["group"].as_bool() != Some(st.group)
            || opt_str(r, "u") != Some(st.owner.as_deref())
        {
            return self.uc_reject(at, "the AI's Run of another command, or one it can't read");
        }
        // `new UnitAttack(target)` or `new UnitUseAbility(type, ability, target)` with its
        // `AiAction`: nothing else set.
        let Some(x) = w.combat.commands.cmds.get(&c) else {
            return self.uc_reject(at, "the AI's command isn't described");
        };
        let fresh = !x.s.started && !x.s.acted && !x.s.finished && x.s.executor.is_none();
        let flags = !x.reaction
            && !x.ignore_cooldown
            && x.ignore_before.is_none()
            && !x.cutscene
            && !x.dont_interrupt_tb
            && !x.player
            && !x.rider
            && !x.mount
            && x.bases.is_empty();
        // The constructor's approach: `ApproachRadius` 0.3, no maximum, not `DistanceXZ`, `NeedLoS`
        // (`UnitAttack`'s; a `UnitUseAbility`'s by its ability).
        let approach = x.ap.as_ref().is_none_or(|a| {
            a.radius.to_bits() == 0.3f32.to_bits()
                && a.max_radius == 0.0
                && !a.xz
                && a.los == spec.los
        });
        let attack_flags = x.ua.as_ref().is_none_or(|a| {
            let f = &a.flags;
            !(f.single || f.force_full || f.charge || f.inspect || f.override_action)
        });
        if x.class != spec.class
            || x.s.ty != spec.ty
            || x.s.target.unit() != Some(spec.target.as_str())
            || !x.ai
            || !fresh
            || !flags
            || !attack_flags
            || !approach
        {
            return self.uc_reject(
                at,
                format!("the AI's command {c} isn't the one the sim made ({spec:?})"),
            );
        }
        self.out.ai_runs += 1;
        self.tgt_run_push(at);
        Ok(c)
    }

    fn ai_nested(&mut self, w: &mut World, end: bool) -> R<()> {
        let ev = if end { "uc_ai_end" } else { "uc_ai" };
        let at = self.uc_pull(w, "the AI's nested ForceTick", |r| obs(r, ev).is_some())?;
        let r = &self.trace.records[at];
        let ok = if end {
            fields(r, &["fn", "exc"]) && r["fn"] == "ForceTick" && r["exc"] == false
        } else {
            fields(r, &["fn", "nested", "origin", "limited"])
                && r["fn"] == "ForceTick"
                && r["nested"] == true
                && r["limited"].as_bool() == Some(w.combat.ai.limited)
                && r["origin"].as_str()
                    == Some(
                        format!("Kingmaker.AI.AiBrainController.ForceTick < {}", self.uc_origin)
                            .as_str(),
                    )
        };
        if !ok {
            return self.uc_reject(at, "a nested ForceTick record it can't read");
        }
        if !end {
            self.ai_force_origin =
                self.trace.records[at]["origin"].as_str().unwrap_or("").to_owned();
        }
        Ok(())
    }

    fn ai_score(&mut self, w: &mut World, check: &ScoreCheck) -> R<()> {
        let path = check.path.is_some();
        let payload: &[&str] = if path {
            &["fn", "a", "tgt", "s", "b", "five", "path"]
        } else {
            &["fn", "a", "tgt", "s", "b"]
        };
        let at = self.ai_check(w, "ai_sc", payload)?;
        let r = &self.trace.records[at];
        let same = r["fn"] == check.f
            && guid(&r["a"]) == Some(Some(check.action))
            && opt_str(r, "tgt") == Some(check.target.as_deref())
            && r["s"].as_str() == Some(check.score.to_string().as_str())
            && r["b"].as_str() == Some(check.best.to_string().as_str())
            && check.path.as_ref().is_none_or(|(five, p)| {
                r["five"].as_bool() == Some(*five)
                    && path_points(&r["path"])
                        .is_some_and(|q| same_points(q.as_deref(), p.as_deref()))
            });
        if !same {
            let d = describe(r);
            return self.uc_reject(
                at,
                format!(
                    "{} of {:?}: the sim scores {} (best {}), the recording has {d} s={} b={}",
                    check.f, check.target, check.score, check.best, r["s"], r["b"]
                ),
            );
        }
        self.out.ai_scores += 1;
        Ok(())
    }

    fn ai_best(&mut self, w: &mut World, check: &BestCheck) -> R<()> {
        let at =
            self.ai_check(w, "ai_best", &["u", "a", "tgt", "best", "cur", "auto", "five", "path"])?;
        let r = &self.trace.records[at];
        let rec_five = r["five"].as_bool();
        let same = r["u"].as_str() == Some(check.unit.as_str())
            && guid(&r["a"]) == Some(check.action)
            && opt_str(r, "tgt") == Some(check.target.as_deref())
            && r["best"].as_str() == Some(check.best.to_string().as_str())
            && r["cur"].as_str() == Some(check.cur.to_string().as_str())
            && r["auto"] == false
            && rec_five.is_some()
            && check.five.is_none_or(|f| Some(f) == rec_five)
            && path_points(&r["path"])
                .is_some_and(|q| same_points(q.as_deref(), check.path.as_deref()));
        if !same {
            let d = describe(r);
            return self.uc_reject(at, format!(
                "FindBestAction of {}: the sim has {:?} {:?} best {} cur {}, the recording {d} best={} cur={}",
                check.unit, check.action, check.target, check.best, check.cur, r["best"], r["cur"]
            ));
        }
        // `BestEnableFiveFootStep` outlives decisions: the window's first is the recording's.
        if check.five.is_none() {
            w.combat.ai.ctx_adopt_best_five(rec_five == Some(true));
        }
        Ok(())
    }

    fn ai_selected(&mut self, w: &mut World, u: &str, ret: bool, nct: Option<f32>) -> R<()> {
        let at = self.ai_check(w, "ai_sel", &["u", "ret", "nct"])?;
        let r = &self.trace.records[at];
        let rec = float(&r["nct"]);
        let same = r["u"].as_str() == Some(u)
            && r["ret"].as_bool() == Some(ret)
            && rec.is_some()
            && nct.is_none_or(|n| rec.map(f32::to_bits) == Some(n.to_bits()));
        if !same {
            let d = describe(r);
            return self.uc_reject(at, format!(
                "SelectAction of {u}: the sim returns {ret} with NextCommandTime {nct:?}, the recording has {d} nct={}",
                r["nct"]
            ));
        }
        self.out.ai_decisions += 1;
        Ok(())
    }
}
