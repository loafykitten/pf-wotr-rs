//! The driven replay's side of line of sight ([`crate::combat::vision`], docs/poc5/vision.md):
//! the engagement's and the group memory's `HasLOS` reads and the AI's `InLos` are the sim's,
//! checked against their leaves, over the sight caches the sim keeps from WrathOracle's `los`
//! (every cache miss) and `los_clear` (every cache clear) records and the units in vision range
//! (the sim's overlap, `vision_range.rs`, applied through its `vis` records: the record is the
//! set's difference from the last one, checked against the sim's).
//!
//! Computed only when the area's line-of-sight file is built (`wr-import los-pack`) and its fog of
//! war bounds are extracted (`wr-rules-data`); otherwise every read stays the recording's, as
//! before, and the report says why.

use std::collections::HashMap;
use std::rc::Rc;

use crate::combat::oracle::Rv;
use crate::combat::vision::{self, Los, Miss, V3, Vision, VisionWorld};
use crate::world::{EntityIdx, World};

use super::commands::describe;
use super::{Driver, Phase, Row};

/// `UnitEntityData.VisionRangeMeters`: `IsPlayerFaction` without overrides, 11.7 m; else
/// `VisionRangeMetersOverride ?? 8.5f`, and in combat `CombatVisionRangeMetersOverride ??
/// Math.Max(11.7f, num)`. A unit with an `OverrideVisionRange` fact is refused (`Err`).
pub(super) fn vision_radius(w: &World, u: EntityIdx) -> Result<f32, String> {
    if super::has_component(w, u, "OverrideVisionRange") {
        return Err("a unit with an OverrideVisionRange fact".into());
    }
    if crate::combat::units::is_player_faction(w, u) {
        return Ok(11.7);
    }
    let num = 8.5f32;
    if !crate::combat::is_in_combat(w, u) {
        return Ok(num);
    }
    // `Math.Max(11.7f, num)`.
    Ok(if 11.7f32 > num { 11.7 } else { num })
}

/// The driver's line-of-sight state.
#[derive(Debug, Default)]
pub(super) struct VisDriver {
    /// The sim computes line of sight in this run.
    pub on: bool,
    /// Why it doesn't (reported with the run), when the recording would let it.
    pub off: Option<String>,
    /// Without the driven agent the units' positions are the recording's rows (a unit's position
    /// at a frame's prefix; after `UnitMoveController` it is the next prefix's).
    pub(super) rows: HashMap<String, Vec<(i64, V3)>>,
    last_tick: i64,
    /// The owners whose `vis` record the frame has had.
    vis_seen: (i64, Vec<EntityIdx>),
    ag: bool,
    /// The frame whose cache phase ran.
    phase_frame: Option<i64>,
    pub(super) range: super::vision_range::RangeDriver,
    pub counts: VisCounts,
}

/// What a run computed, by reader.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct VisCounts {
    /// The engagement's `HasLOS` reads: the sim's answer equal to the leaf's (of which taken from
    /// the recording for an entry made before the window).
    pub engage: usize,
    pub engage_pre: usize,
    /// The memory's `group.HasLOS` reads (members' reads inside them are the cache's).
    pub memory: usize,
    pub memory_pre: usize,
    /// The AI's `InLos` reads.
    pub ai: usize,
    /// Sight cache misses computed and equal to their record (the cache's counters, merged).
    pub misses: usize,
    pub hits: usize,
    pub pre_hits: usize,
    pub pre_fresh_equal: usize,
    pub pre_fresh_different: usize,
    pub out_of_range: usize,
    pub clears: usize,
    pub adopted_clears: usize,
    pub unmodelled_fills: usize,
    pub obstacle_reads: usize,
    /// `vis` records applied.
    pub vis_records: usize,
    /// The vision controller's sets the sim computed (an awake unit in a frame) and compared with
    /// the recording's, of which the frames with a `vis` record (a change), and the differences
    /// found (reported). Pairs (a unit and a collider) the recording decided because the
    /// collider's corpulence is unknown.
    pub range_sets: usize,
    pub range_changes: usize,
    pub range_differences: usize,
    pub range_maybe_pairs: usize,
    /// Pairs computed with a known corpulence, the pairs within 1 cm and within 1 mm of the
    /// capsule's surface, the closest pair's distance in micrometres, and the pairs whose `f32`
    /// decision differs from the exact one.
    pub range_pairs: usize,
    pub range_within_1cm: usize,
    pub range_within_1mm: usize,
    pub range_closest_um: u64,
    pub range_rounding: usize,
    /// Collider radii taken from a read after the frame (the unit's first read is later).
    pub range_corp_later: usize,
    /// Colliders made after the window's first frame (units that joined later).
    pub range_made_later: usize,
    /// The group memory's near units (`ExtractNearUnits`, the members' sets in the overlap's
    /// order) the sim computed and found equal to the recording's.
    pub near_unions: usize,
    /// The game's own segments (`los_geom`) and `HasObstacle` probes (`los_probe`) the sim's
    /// geometry equals.
    pub geometry_segments: usize,
    pub probes: usize,
    /// Reads and fills in the recording's last frame without the driven agent (positions as of
    /// the last prefix, see [`Driver::note_last_frame`]).
    pub last_frame_reads: usize,
}

/// The model's view of the world during one read.
struct Vw<'a> {
    w: &'a World,
    d: &'a VisDriver,
    tick: i64,
    after_move: bool,
}

impl Vw<'_> {
    fn pos(&self, u: EntityIdx) -> V3 {
        if !self.d.ag {
            let want = if self.after_move && self.tick < self.d.last_tick {
                self.tick + 1
            } else {
                self.tick
            };
            if let Some(rows) = self.d.rows.get(&self.w.entity(u).unique_id) {
                let i = rows.partition_point(|(t, _)| *t <= want);
                if i > 0 {
                    return rows[i - 1].1;
                }
            }
        }
        self.w.unit(u).map_or([0.0; 3], |x| x.position)
    }
}

impl VisionWorld for Vw<'_> {
    fn position(&self, u: EntityIdx) -> V3 {
        self.pos(u)
    }
    fn units(&self) -> Vec<EntityIdx> {
        crate::combat::life::all_units(self.w)
    }
    fn is_fake(&self, u: EntityIdx) -> bool {
        self.w
            .unit(u)
            .and_then(|x| x.blueprint)
            .and_then(|g| self.w.content.get(g))
            .is_some_and(|b| b.view().bool("IsFake"))
    }
    fn radius(&self, u: EntityIdx) -> f32 {
        // (A unit refused at the start of the run can't be asked here.)
        vision_radius(self.w, u).unwrap_or(f32::NAN)
    }
}

fn v3(v: &Rv) -> Option<V3> {
    let a = v.as_array()?;
    let [x, y, z] = a else { return None };
    #[allow(clippy::cast_possible_truncation)]
    let f = |v: &Rv| v.as_f64().map(|x| x as f32).filter(|x| x.is_finite());
    Some([f(x)?, f(y)?, f(z)?])
}

fn has_fields(r: &Rv, payload: &[&str]) -> bool {
    const BASE: [&str; 4] = ["seq", "k", "t", "ev"];
    r.as_object().is_some_and(|o| {
        o.len() == BASE.len() + payload.len()
            && BASE.iter().chain(payload).all(|k| o.contains_key(*k))
    })
}

impl Driver<'_> {
    /// The window's start: loads the geometry when the recording and the caches allow it.
    pub(super) fn vis_start(&mut self, w: &mut World, declared: &[&str]) {
        let trace = self.trace;
        let has_vis = trace.records.iter().any(|r| r["k"] == "obs" && r["ev"] == "vis");
        if !declared.contains(&"engage") {
            return;
        }
        if !has_vis {
            // (The memory observer needs the vision records; older captures have neither.)
            if declared.contains(&"memory") {
                w.coverage
                    .unsupported("driven replay: vision: a recording without its vis records");
            }
            return;
        }
        let g = match load_geometry(w) {
            Ok(g) => g,
            Err(e) => {
                self.vis.off = Some(e);
                return;
            }
        };
        // A unit with an overridden vision range isn't modelled: refused for the whole run.
        for u in crate::combat::life::all_units(w) {
            if let Err(e) = vision_radius(w, u) {
                self.vis.off = Some(format!("{e} ({})", w.entity(u).unique_id));
                return;
            }
        }
        self.vis.on = true;
        self.vis.ag = self.ag;
        w.combat.vision.geometry = Some(g);
        let mut last = 0;
        for r in &trace.records {
            if r["k"] == "tick" {
                last = r["t"].as_i64().unwrap_or(last);
            }
            if r["k"] == "obs"
                && r["ev"] == "u"
                && let (Some(id), Some(t), Some(row)) =
                    (r["u"].as_str(), r["t"].as_i64(), r["s"].as_str().and_then(Row::parse))
            {
                self.vis.rows.entry(id.to_owned()).or_default().push((t, row.pos));
            }
        }
        self.vis.last_tick = last;
        self.vis.counts.range_closest_um = u64::MAX;
        self.vis.range.start(trace);
    }

    fn vis_world<'a>(&'a self, w: &'a World, after_move: bool) -> Vw<'a> {
        Vw { w, d: &self.vis, tick: self.tick, after_move }
    }

    /// A unit's position as the model reads it (see [`Vw`]).
    pub(super) fn vis_pos(&self, w: &World, u: EntityIdx, after_move: bool) -> V3 {
        self.vis_world(w, after_move).pos(u)
    }

    /// Runs `f` on the sim's vision with the world's view.
    fn with_vision<T>(
        &mut self,
        w: &mut World,
        after_move: bool,
        f: impl FnOnce(&mut Vision, &Vw<'_>) -> T,
    ) -> T {
        let mut v = std::mem::take(&mut w.combat.vision);
        let out = {
            let vw = Vw { w: &*w, d: &self.vis, tick: self.tick, after_move };
            f(&mut v, &vw)
        };
        w.combat.vision = v;
        out
    }

    pub(super) fn vis_report(w: &mut World, what: &str) {
        w.coverage.unsupported(format!("driven replay: vision: {what}"));
    }

    /// Whether a read is after `UnitMoveController` in its frame (the units' positions are then the
    /// next prefix's, without the driven agent).
    fn after_move(&self) -> bool {
        self.frame.at > Phase::Move
    }

    /// Without the driven agent, a position after the movement of the recording's last frame isn't
    /// established (there is no next prefix to read it from): the last prefix's is used and the
    /// read counted. The answers are checked against the recording as any other, so a unit that
    /// moved in that frame shows as a difference.
    fn note_last_frame(&mut self, after_move: bool) {
        if !self.vis.ag && after_move && self.tick >= self.vis.last_tick {
            self.vis.counts.last_frame_reads += 1;
        }
    }

    /// A `vis`, `los`, `los_clear` or `los_geometry` record.
    pub(super) fn vis_record(&mut self, w: &mut World, r: &Rv) {
        // Each is of the frame it's made in.
        if r["t"].as_i64() != Some(self.tick) {
            Self::vis_report(w, "a record of another frame");
            return;
        }
        match r["ev"].as_str() {
            Some("vis") => self.vis_set(w, r),
            Some("los") => self.vis_miss(w, r),
            Some("los_clear") => self.vis_clear(w, r),
            Some("los_geom") => self.vis_geom(w, r),
            Some("los_probe") => self.vis_probe(w, r),
            Some("los_geometry") => Self::vis_report(
                w,
                "the geometry was updated (a blocker changed: LastUpdateTime, not modelled)",
            ),
            _ => {}
        }
    }

    /// `los_geom`: the game's grid and segments, once a window (WrathOracle's dump of
    /// `LineOfSightGeometry`'s private state): the sim's equal.
    fn vis_geom(&mut self, w: &mut World, r: &Rv) {
        let parsed = (|| {
            if !has_fields(r, &["nx", "nz", "min", "segs"]) {
                return None;
            }
            let nx = i32::try_from(r["nx"].as_i64()?).ok()?;
            let nz = i32::try_from(r["nz"].as_i64()?).ok()?;
            let m = r["min"].as_array()?;
            #[allow(clippy::cast_possible_truncation)]
            let f = |v: &Rv| v.as_f64().map(|x| x as f32).filter(|x| x.is_finite());
            let [mx, mz] = m else { return None };
            let min = [f(mx)?, f(mz)?];
            let segs = r["segs"]
                .as_array()?
                .iter()
                .map(|s| {
                    let [i, j, ax, az, bx, bz, h] = s.as_array()? else { return None };
                    Some((
                        i32::try_from(i.as_i64()?).ok()?,
                        i32::try_from(j.as_i64()?).ok()?,
                        [f(ax)?, f(az)?],
                        [f(bx)?, f(bz)?],
                        f(h)?,
                    ))
                })
                .collect::<Option<Vec<_>>>()?;
            Some(vision::GeomDump { nx, nz, min, segs })
        })();
        let (Some(d), Some(g)) = (parsed, w.combat.vision.geometry.clone()) else {
            Self::vis_report(w, "a los_geom record it can't read");
            return;
        };
        match vision::check_geometry(&g, &d) {
            Ok(n) => self.vis.counts.geometry_segments += n,
            Err(e) => Self::vis_report(w, &format!("the geometry differs from the game's: {e}")),
        }
    }

    /// `los_probe`: `HasObstacle` answers the game gave to segments chosen around the blockers.
    fn vis_probe(&mut self, w: &mut World, r: &Rv) {
        let parsed = (|| {
            if !has_fields(r, &["v"]) {
                return None;
            }
            #[allow(clippy::cast_possible_truncation)]
            let f = |v: &Rv| v.as_f64().map(|x| x as f32).filter(|x| x.is_finite());
            r["v"]
                .as_array()?
                .iter()
                .map(|p| {
                    let [fx, fy, fz, tx, ty, tz, fudge, res] = p.as_array()? else { return None };
                    Some(vision::Probe {
                        from: [f(fx)?, f(fy)?, f(fz)?],
                        to: [f(tx)?, f(ty)?, f(tz)?],
                        fudge: f(fudge)?,
                        result: match res.as_i64()? {
                            0 => false,
                            1 => true,
                            _ => return None,
                        },
                    })
                })
                .collect::<Option<Vec<_>>>()
        })();
        let (Some(p), Some(g)) = (parsed, w.combat.vision.geometry.clone()) else {
            Self::vis_report(w, "a los_probe record it can't read");
            return;
        };
        match vision::check_probes(&g, &p) {
            Ok(n) => self.vis.counts.probes += n,
            Err(e) => Self::vis_report(w, &format!("the geometry differs from the game's: {e}")),
        }
    }

    /// `vis`: the changes of a unit's `CanBeInVisionRange` against its last record's set.
    fn vis_set(&mut self, w: &mut World, r: &Rv) {
        // `UnitVisionRangeController` follows the movement and precedes the fog of war; it isn't
        // registered in the pause mode.
        if self.frame.mode == Some(super::mode::PAUSE) {
            Self::vis_report(w, "a vis record in the pause mode");
            return;
        }
        self.place(w, Phase::Visibility, "the vision range controller");
        let (true, Some(u), Some(add), Some(rem)) = (
            has_fields(r, &["u", "add", "rem"]),
            r["u"].as_str().and_then(|u| w.by_id.get(u).copied()),
            r["add"].as_array(),
            r["rem"].as_array(),
        ) else {
            Self::vis_report(w, "a vis record it can't read");
            return;
        };
        let units = |a: &[Rv]| -> Option<Vec<EntityIdx>> {
            a.iter().map(|x| x.as_str().and_then(|s| w.by_id.get(s).copied())).collect()
        };
        let (Some(add), Some(rem)) = (units(add), units(rem)) else {
            Self::vis_report(w, "a vis record names a unit the sim doesn't have");
            return;
        };
        self.vis.counts.vis_records += 1;
        // Once per awake owner and frame, an empty delta never (after the first record).
        if self.vis.vis_seen.0 != self.tick {
            self.vis.vis_seen = (self.tick, Vec::new());
        }
        let first = !self.vis.range.last.contains_key(&u);
        if self.vis.vis_seen.1.contains(&u)
            || !w.combat.awake.contains(&u)
            || (!first && add.is_empty() && rem.is_empty())
        {
            Self::vis_report(w, "a vis record the controller wouldn't write");
            return;
        }
        self.vis.vis_seen.1.push(u);
        // The delta of two sets: unique, disjoint, removals present, additions absent.
        let mut set = self.vis.range.last.get(&u).cloned().unwrap_or_default();
        let unique =
            |a: &[EntityIdx]| a.iter().collect::<std::collections::HashSet<_>>().len() == a.len();
        if !unique(&add)
            || !unique(&rem)
            || add.iter().any(|x| rem.contains(x) || set.contains(x))
            || rem.iter().any(|x| !set.contains(x))
        {
            Self::vis_report(w, "a vis record that isn't the difference of two sets");
            return;
        }
        for x in rem {
            set.remove(&x);
        }
        set.extend(add);
        self.vis.range.last.insert(u, set.clone());
        w.combat.vision.near.insert(u, set.clone());
        self.vis_range_check(w, u, &set, true);
    }

    /// `los`: a sight cache miss, queued for the read that made it.
    fn vis_miss(&mut self, w: &mut World, r: &Rv) {
        let (true, Some(owner), Some(point), Some(sample), Some(result)) = (
            has_fields(r, &["o", "p", "s", "r"]),
            r["o"].as_str().and_then(|u| w.by_id.get(u).copied()),
            v3(&r["p"]),
            v3(&r["s"]),
            r["r"].as_bool(),
        ) else {
            Self::vis_report(w, "a los record it can't read");
            return;
        };
        w.combat.vision.pending.push_back(Miss { owner, point, sample, result });
    }

    /// `los_clear`: a sight cache cleared; a cache the sim knows must be one it predicted.
    fn vis_clear(&mut self, w: &mut World, r: &Rv) {
        self.place(w, Phase::LosCache, "the sight caches' controller");
        let (true, Some(u), Some(at)) = (
            has_fields(r, &["o", "at"]),
            r["o"].as_str().and_then(|u| w.by_id.get(u).copied()),
            v3(&r["at"]),
        ) else {
            Self::vis_report(w, "a los_clear record it can't read");
            return;
        };
        let known = w.combat.vision.caches.get(&u).is_some_and(|c| c.known);
        if known {
            let v = &mut w.combat.vision;
            match v.clears.iter().position(|(o, _)| *o == u) {
                Some(i) if v.clears[i].1.map(f32::to_bits) == at.map(f32::to_bits) => {
                    v.clears.remove(i);
                    self.vis.counts.clears += 1;
                }
                Some(_) => {
                    Self::vis_report(w, "a sight cache cleared at another position than the sim's")
                }
                None => Self::vis_report(w, "the game cleared a sight cache the sim didn't"),
            }
            return;
        }
        // (`ClearIfNeeded` is called for the awake units only.)
        if !w.combat.awake.contains(&u) {
            Self::vis_report(w, "a sight cache cleared whose owner the controller doesn't visit");
            return;
        }
        let res = self.with_vision(w, false, |v, vw| v.adopt_clear(vw, u, at));
        if let Err(e) = res {
            Self::vis_report(w, &e);
        }
    }

    /// `UnitLineOfSightCacheController.Tick` (before `UnitMoveController`): every awake unit's
    /// cache is cleared if it should be; the clears wait for their `los_clear` records.
    pub(super) fn vis_cache_phase(&mut self, w: &mut World) {
        if !self.vis.on || self.vis.phase_frame == Some(self.tick) {
            return;
        }
        self.vis.phase_frame = Some(self.tick);
        let awake = w.combat.awake.clone();
        let clears = self.with_vision(w, false, |v, vw| {
            let mut out = Vec::new();
            v.frame_radius.clear();
            for u in awake {
                v.frame_radius.insert(u, vw.radius(u));
                if let Some(at) = v.clear_if_needed(vw, u) {
                    out.push((u, at));
                }
            }
            out
        });
        w.combat.vision.clears.extend(clears);
    }

    /// A frame's end: the clears and misses nobody explained.
    pub(super) fn vis_frame_end(&mut self, w: &mut World) {
        if !self.vis.on {
            return;
        }
        self.vis.range.expect.clear();
        if !w.combat.vision.clears.is_empty() {
            w.combat.vision.clears.clear();
            Self::vis_report(w, "the sim cleared a sight cache the game didn't");
        }
        // Misses of readers the sim doesn't run: verified, and their entries kept.
        let pending: Vec<Miss> = w.combat.vision.pending.drain(..).collect();
        if pending.is_empty() {
            return;
        }
        self.note_last_frame(true);
        for m in pending {
            let res = self.with_vision(w, true, |v, vw| v.adopt_miss(vw, m));
            if let Err(e) = res {
                Self::vis_report(w, &e);
            }
        }
    }

    /// The counters, merged at the run's end.
    pub(super) fn vis_finish(&mut self, w: &mut World) {
        if !self.vis.on {
            return;
        }
        let c = w.combat.vision.counts;
        let k = &mut self.vis.counts;
        k.misses = c.misses;
        k.hits = c.hits;
        k.pre_hits = c.pre_hits;
        k.pre_fresh_equal = c.pre_fresh_equal;
        k.pre_fresh_different = c.pre_fresh_different;
        k.out_of_range = c.out_of_range;
        k.adopted_clears = c.adopted_clears;
        k.unmodelled_fills = c.unmodelled_fills;
        k.obstacle_reads = c.obstacle_reads;
        k.range_made_later = w.combat.vision.bounds.made_later;
        self.out.vision = Some(self.vis.counts.clone());
        self.out.vision_off = None;
    }

    /// The engagement's `unit.HasLOS(enemy)`: the sim's, checked against the leaf's `recorded`.
    pub(super) fn vis_engage(
        &mut self,
        w: &mut World,
        a: &str,
        b: &str,
        recorded: bool,
        at: usize,
    ) -> bool {
        if !self.vis.on {
            return recorded;
        }
        self.note_last_frame(self.after_move());
        let (Some(ua), Some(ub)) = (w.by_id.get(a).copied(), w.by_id.get(b).copied()) else {
            Self::vis_report(w, "an engagement read of a unit the sim doesn't have");
            return recorded;
        };
        let am = self.after_move();
        let res = self.with_vision(w, am, |v, vw| v.has_los(vw, ua, ub));
        match res {
            Ok(Los::Value(v)) => {
                if v != recorded {
                    Self::vis_report(
                        w,
                        &format!(
                            "the engagement's HasLOS differs: the sim's {v}, the recording's ({})",
                            describe(&self.trace.records[at])
                        ),
                    );
                    return recorded;
                }
                self.vis.counts.engage += 1;
                v
            }
            Ok(Los::Pre) => {
                let res = self.with_vision(w, am, |v, vw| v.resolve_pre(vw, ua, ub, recorded));
                if let Err(e) = res {
                    Self::vis_report(w, &e);
                }
                self.vis.counts.engage += 1;
                self.vis.counts.engage_pre += 1;
                recorded
            }
            Err(e) => {
                Self::vis_report(w, &e);
                recorded
            }
        }
    }

    /// The AI's `LineOfSightGeometry.HasObstacle(context.Unit.EyePosition, Unit.Position)`.
    pub(super) fn vis_ai(
        &mut self,
        w: &mut World,
        me: EntityIdx,
        target: EntityIdx,
        recorded: bool,
    ) -> bool {
        if !self.vis.on {
            return recorded;
        }
        self.note_last_frame(self.after_move());
        let (from, to) = {
            let vw = self.vis_world(w, self.after_move());
            let p = vw.pos(me);
            ([p[0], p[1] + 1.0, p[2]], vw.pos(target))
        };
        match w.combat.vision.obstacle(from, to, 0.0, recorded) {
            Ok(v) => {
                self.vis.counts.ai += 1;
                v
            }
            Err(e) => {
                Self::vis_report(w, &e);
                recorded
            }
        }
    }

    /// The memory's `group.HasLOS(target)`: the members in order, each `IsInGame && !IsHelpless
    /// && HasLOS(target)`; the sim's answer checked against the leaf's `recorded`.
    pub(super) fn vis_group(
        &mut self,
        w: &mut World,
        members: &[EntityIdx],
        target: EntityIdx,
        recorded: bool,
    ) -> bool {
        if !self.vis.on {
            return recorded;
        }
        self.note_last_frame(self.after_move());
        let am = self.after_move();
        let mut pre = false;
        let mut pre_members = Vec::new();
        let mut stopped: Option<EntityIdx> = None;
        let mut found = false;
        for (k, &m) in members.iter().enumerate() {
            if !(w.entity(m).is_in_game && !crate::combat::units::is_helpless(w, m)) {
                continue;
            }
            let res = self.with_vision(w, am, |v, vw| v.has_los(vw, m, target));
            match res {
                Ok(Los::Value(true)) => {
                    found = true;
                    break;
                }
                Ok(Los::Value(false)) => {}
                Ok(Los::Pre) => {
                    pre = true;
                    pre_members.push(m);
                    // The game stopped at this member if the aggregate is true and no later
                    // member's read follows.
                    let later = {
                        let vw = self.vis_world(w, am);
                        let eye = [0, 1, 2].map(|i| vw.pos(target)[i] + vision::EYE[i]);
                        members[k + 1..].iter().any(|o| {
                            w.combat.vision.pending.iter().any(|p| {
                                p.owner == *o && p.point.map(f32::to_bits) == eye.map(f32::to_bits)
                            })
                        })
                    };
                    if recorded && !later {
                        found = true;
                        stopped = Some(m);
                        break;
                    }
                }
                Err(e) => {
                    Self::vis_report(w, &e);
                    return recorded;
                }
            }
        }
        if found != recorded {
            Self::vis_report(
                w,
                &format!(
                    "the memory's group HasLOS differs: the sim's {found}, the recording's {recorded}"
                ),
            );
            return recorded;
        }
        self.vis.counts.memory += 1;
        if pre {
            self.vis.counts.memory_pre += 1;
            // The members of entries made before the window: all but the one the game stopped at
            // returned false (a false aggregate, or a later member's read followed); that is the
            // entry's value now, compared with a fresh computation (evidence, not an answer).
            for m in pre_members {
                if Some(m) == stopped {
                    let fresh =
                        self.with_vision(w, am, |v, vw| v.fresh(vw, m, target)).unwrap_or(false);
                    w.combat.vision.note_fresh(fresh);
                    // The only member left to answer true: its entry's value is true.
                    let k = members.iter().position(|x| *x == m).unwrap_or(0);
                    let mut later = false;
                    for o in &members[k + 1..] {
                        later |=
                            w.entity(*o).is_in_game && !crate::combat::units::is_helpless(w, *o);
                    }
                    if !later {
                        let _ = self.with_vision(w, am, |v, vw| v.resolve_pre(vw, m, target, true));
                    }
                } else if let Err(e) =
                    self.with_vision(w, am, |v, vw| v.resolve_pre(vw, m, target, false))
                {
                    Self::vis_report(w, &e);
                }
            }
        }
        found
    }
}

/// The area's geometry (`LineOfSightGeometry.Init` with the area's fog of war bounds and every
/// enabled blocker), or why the sim can't build it.
fn load_geometry(w: &World) -> Result<Rc<wr_los::Geometry>, String> {
    let area = w.player.area.ok_or("the save names no current area")?;
    let guid = area.to_string();
    let path = wr_los::path_for(&guid);
    let file = wr_los::LosFile::read(&path)
        .map_err(|e| format!("no line-of-sight file (wr-import los-pack): {e:#}"))?;
    if file.extended_vision_spawners > 0 {
        return Err("the area has units with an ExtendedVisionArea".into());
    }
    let [fow] = &file.areas[..] else {
        return Err(format!("the area has {} FogOfWarAreas", file.areas.len()));
    };
    if crate::scene::area_has_parts(w) {
        return Err("an area with parts (which part's fog of war bounds is not modelled)".into());
    }
    let asset = crate::area_parts::part_bounds(w, area)?
        .ok_or("the area has no bounds asset (the FogOfWarArea keeps its scene bounds)")?;
    let b = asset
        .fog_of_war_bounds()
        .ok_or("the area bounds table has no fog of war bounds (rebuild it with wr-rules-data)")?;
    // `SetFogOfWarParameters`: `center -= position`; `GetWorldBounds`: `center += position`.
    let p = fow.position;
    let c1 = [b.center[0] - p[0], b.center[1] - p[1], b.center[2] - p[2]];
    let c2 = [c1[0] + p[0], c1[1] + p[1], c1[2] + p[2]];
    wr_los::Geometry::init(wr_los::Bounds { center: c2, extents: b.extents }, &file.blockers)
        .map(Rc::new)
        .map_err(|e| e.to_string())
}
