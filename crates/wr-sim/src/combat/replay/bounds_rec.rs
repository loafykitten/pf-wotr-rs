//! WrathOracle's `bounds_init`, `bounds_tick`, `bounds_set`, `bounds_scene` and `phys_sync`
//! records, and `vis_order` (docs/poc5/vision.md, "The window's start, from the records"): the
//! entity-bounds colliders as the game made and changed them, and the order of each unit's
//! `CanBeInVisionRange`.
//!
//! A recording that declares the first five observers (they are installed together) gives the
//! vision range its colliders instead of the window-start hypotheses: `range::Bounds` starts from
//! `bounds_init`, applies each frame's `bounds_tick` calls and `bounds_set` / `phys_sync` writes,
//! and checks the sim's own model of the bounds controller (the radius rule, the update list, the
//! transform rule, `enabled`) against them. One that declares `vis_order` has each unit's set
//! order checked, or taken where the sim's colliders don't give it. An older recording replays
//! with the hypotheses, as before.
//!
//! Fail closed: every record is validated up front (its fields and their types, finite numbers,
//! the count in `bounds_init_end`, a record that threw, a scene remade); one that doesn't fit is
//! reported, the family is then not used, and the run falls back to the hypotheses. Every record
//! must be consumed exactly once: the tick-indexed ones (`bounds_init`, `bounds_tick`,
//! `vis_order`) by the frame's vision phase, the others (`bounds_set`, `phys_sync`) as the replay
//! reaches them; one that none consumed is reported at the run's end with the other vision
//! records.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::combat::oracle::Rv;
use crate::combat::orientation;
use crate::combat::vision::V3;
use crate::combat::vision::range::{
    self, BoundsUnit, Event, RecCollider, RecCounts, RecTick, Step, Write,
};
use crate::world::{EntityIdx, World};

use super::Driver;

/// The observers installed together (WrathOracle's `BoundsOn`).
const FAMILY: [&str; 5] = ["bounds_init", "bounds_tick", "bounds_set", "bounds_scene", "phys_sync"];

/// Records of the family (`bounds_init_end` besides the observers' names).
const RECORDS: [&str; 6] =
    ["bounds_init", "bounds_init_end", "bounds_tick", "bounds_set", "bounds_scene", "phys_sync"];

/// The kinds of record the family is made of, as `unconsumed` names them.
const FAMILY_RECORDS: [&str; 5] =
    ["bounds_init", "bounds_init_end", "bounds_tick", "bounds_set", "phys_sync"];

const UNIT_KIND: &str = "UnitEntityData";
/// `Layers.SphereBoundsUnit`.
const UNIT_LAYER: i32 = 23;

/// One root of the entity-bounds scene at the window's first tick.
#[derive(Debug, Clone)]
struct Root {
    seq: i64,
    id: i32,
    layer: i32,
    e: Option<String>,
    kind: Option<String>,
    r: Option<f32>,
    on: Option<bool>,
    pos: V3,
    fade: Option<bool>,
}

/// One `TickEntity` call.
#[derive(Debug, Clone)]
struct TickRaw {
    seq: i64,
    e: String,
    kind: String,
    made: bool,
    r0: Option<f32>,
    r: Option<f32>,
    pos: Option<V3>,
    on: Option<bool>,
    c: Option<i32>,
}

/// A record the replay reaches in stream order.
#[derive(Debug, Clone, PartialEq)]
enum Stream {
    Active {
        c: i32,
        on0: bool,
        on: bool,
    },
    Pos {
        c: i32,
        pos0: V3,
        pos: V3,
    },
    Gone {
        c: i32,
    },
    /// `SetupSphereColliderEntityLink`: the view's collider relinked (`c` null: the view has none).
    Link {
        c: Option<i32>,
        e0: Option<String>,
        e: Option<String>,
    },
    /// `EntityDataLink.Entity`'s setter.
    Entity {
        c: i32,
        e0: Option<String>,
        e: Option<String>,
    },
    Sync,
}

/// A stream record that has reached the replay and waits for the frame's vision phase.
#[derive(Debug, Clone)]
struct Queued {
    seq: i64,
    t: i64,
    /// It came after the frame's vision phase ran.
    late: bool,
    s: Stream,
}

/// The `bounds_*` family of a recording.
#[derive(Debug, Default)]
pub(super) struct BoundsRec {
    init_t: i64,
    roots: Vec<Root>,
    /// `bounds_init_end`'s seq.
    end_seq: i64,
    /// The unit-or-not `TickEntity` calls of each frame, in stream order, not applied yet.
    ticks: BTreeMap<i64, Vec<TickRaw>>,
    /// The stream records by seq: their frame and content.
    stream: HashMap<i64, (i64, Stream)>,
    queue: Vec<Queued>,
    /// The instance ids of the colliders that are units', and of the others (map objects, zones):
    /// a write to the latter is not the bounds model's.
    unit_ids: HashSet<i32>,
    other_ids: HashSet<i32>,
    /// The entity each collider is linked to (`EntityDataLink`), from `bounds_init` and the calls
    /// that made colliders.
    bound: HashMap<i32, String>,
}

/// `vis_order`: the order of a unit's set, by frame and unit.
#[derive(Debug, Default)]
pub(super) struct OrderRec {
    records: HashMap<(i64, String), (i64, Vec<String>)>,
}

/// What the family and `vis_order` did in a run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BoundsRecCounts {
    /// The family was used (else the window-start hypotheses).
    pub used: bool,
    /// `bounds_init` roots: units' colliders, others (map objects, zones).
    pub roots: usize,
    pub other_roots: usize,
    /// Of the units in the pool: ones with a collider that were out of the game at the start
    /// (their history, unknown under the hypotheses), ones in the game with none.
    pub hidden_with_collider: usize,
    pub in_game_without: usize,
    /// The collider creation order of `bounds_init` against the pool's: units compared.
    pub order_units: usize,
    /// The start transform against the unit's position at the previous frame's end: equal,
    /// within `Vector3 ==`'s tolerance, not checked.
    pub start_equal: usize,
    pub start_near: usize,
    pub start_unchecked: usize,
    /// `bounds_tick` calls of other entities (not units).
    pub other_ticks: usize,
    /// What the unit ticks, writes and syncs did (see [`RecCounts`]).
    pub rec: RecCounts,
    /// Writes to other entities' colliders, links that changed nothing or had no collider.
    pub other_writes: usize,
    pub links: usize,
    /// Frames with a recorded sync, of which the sim had flagged a possible one / had not.
    pub sync_frames: usize,
    pub sync_flagged: usize,
    pub sync_unflagged: usize,
    /// Frames where the sim flagged a possible sync and none was recorded.
    pub sync_flag_only: usize,
    /// Views the sim thought a polymorph or the dummy swap replaced (the records show them).
    pub views_replaced: usize,
    /// `vis_order` records consumed; sets whose order the sim's colliders give (their creation
    /// order), found equal to the recorded one / differing from it (PhysX's result order isn't
    /// always the creation order; counted, the recorded order is the one used); sets whose order
    /// the sim could not give, taken.
    pub order_records: usize,
    pub order_equal: usize,
    pub order_differ: usize,
    pub order_taken: usize,
}

fn keys_are(r: &Rv, required: &[&str], optional: &[&str]) -> bool {
    r.as_object().is_some_and(|o| {
        required.iter().all(|k| o.contains_key(k))
            && o.keys().all(|k| required.contains(&k) || optional.contains(&k))
    })
}

#[allow(clippy::cast_possible_truncation)]
fn finite(v: &Rv) -> Option<f32> {
    v.as_f64().map(|x| x as f32).filter(|x| x.is_finite())
}

fn vec3(v: &Rv) -> Option<V3> {
    let [x, y, z] = v.as_array()? else { return None };
    Some([finite(x)?, finite(y)?, finite(z)?])
}

fn int32(v: &Rv) -> Option<i32> {
    i32::try_from(v.as_i64()?).ok()
}

/// A field that may be null: `Some(None)` for null, `Some(Some(x))` for a valid value, `None`
/// for anything else.
fn nullable<T>(v: &Rv, f: impl Fn(&Rv) -> Option<T>) -> Option<Option<T>> {
    if v.is_null() { Some(None) } else { f(v).map(Some) }
}

fn string(v: &Rv) -> Option<String> {
    v.as_str().map(str::to_owned)
}

/// What a recording's bounds records come to.
#[derive(Debug, Default)]
pub(super) struct Parsed {
    pub rec: Option<BoundsRec>,
    pub orders: Option<OrderRec>,
    /// Records the replay must consume, with their kinds.
    pub expect: Vec<(i64, String)>,
    pub problems: Vec<String>,
}

/// Validates and indexes the records. `declared`: the begin record's observers.
pub(super) fn parse(records: &[Rv], declared: &[&str]) -> Parsed {
    let mut out = Parsed::default();
    let bad = |out: &mut Parsed, what: String| out.problems.push(format!("bounds records: {what}"));
    let n_declared = FAMILY.iter().filter(|f| declared.contains(f)).count();
    let family_records = records
        .iter()
        .any(|r| r["k"] == "obs" && r["ev"].as_str().is_some_and(|e| RECORDS.contains(&e)));
    let order_records = records.iter().any(|r| r["k"] == "obs" && r["ev"] == "vis_order");
    if n_declared == 0 && family_records {
        bad(&mut out, "records of the bounds observers without their declaration".into());
    }
    if !declared.contains(&"vis_order") && order_records {
        bad(&mut out, "vis_order records without their declaration".into());
    }
    let family_on = n_declared == FAMILY.len();
    let (fam_frames, ord_frames) =
        check_frames(records, family_on, declared.contains(&"vis_order"));
    out.problems.extend(ord_frames);
    if declared.contains(&"vis_order") {
        parse_orders(records, &mut out);
    }
    if n_declared == 0 {
        return out;
    }
    if n_declared < FAMILY.len() {
        let missing: Vec<&str> = FAMILY.iter().filter(|f| !declared.contains(f)).copied().collect();
        bad(
            &mut out,
            format!(
                "the recording declares only part of the bounds observers (missing {missing:?})"
            ),
        );
        return out;
    }
    let mut rec = BoundsRec::default();
    let before = out.problems.len();
    out.problems.extend(fam_frames);
    parse_init(records, &mut rec, &mut out);
    parse_ticks(records, &mut rec, &mut out);
    parse_stream(records, &mut rec, &mut out);
    // (A problem of the family refuses it; those of `vis_order` alone don't.)
    if out.problems.len() == before {
        out.rec = Some(rec);
    }
    out
}

/// Each record is where its frame says: after the `tick` record of the frame it names (the last
/// one before it in the stream), the roots before the first call, the calls before the frame's
/// `vis_order` records. Returns the findings about the family's records and about `vis_order`'s.
fn check_frames(records: &[Rv], family: bool, orders: bool) -> (Vec<String>, Vec<String>) {
    let (mut fam, mut ord) = (Vec::new(), Vec::new());
    let mut frame: Option<i64> = None;
    let mut order_seen = false;
    let mut init_end = false;
    let mut tick_seen = false;
    for r in records {
        if r["k"] == "tick" {
            frame = r["t"].as_i64();
            order_seen = false;
            continue;
        }
        if r["k"] != "obs" {
            continue;
        }
        let Some(ev) = r["ev"].as_str() else { continue };
        let is_order = ev == "vis_order";
        if !((family && RECORDS.contains(&ev)) || (orders && is_order)) {
            continue;
        }
        let seq = r["seq"].as_i64();
        let into = if is_order { &mut ord } else { &mut fam };
        let prefix = if is_order { "vis_order" } else { "bounds records" };
        if frame.is_none() || r["t"].as_i64() != frame {
            into.push(format!(
                "{prefix}: a {ev} record (seq {seq:?}) claims tick {:?} and stands in the stream in tick {frame:?}",
                r["t"].as_i64()
            ));
            continue;
        }
        match ev {
            "bounds_tick" => {
                if !init_end {
                    into.push(format!(
                        "{prefix}: a bounds_tick (seq {seq:?}) before bounds_init_end"
                    ));
                }
                if order_seen {
                    into.push(format!(
                        "{prefix}: a bounds_tick (seq {seq:?}) after the frame's vis_order records (the bounds controller precedes the vision controller)"
                    ));
                }
                tick_seen = true;
            }
            "bounds_init" if tick_seen || init_end => {
                into.push(format!(
                    "{prefix}: a bounds_init (seq {seq:?}) after the window's first call or end"
                ));
            }
            "bounds_init_end" => init_end = true,
            "vis_order" => order_seen = true,
            _ => {}
        }
    }
    (fam, ord)
}

fn parse_orders(records: &[Rv], out: &mut Parsed) {
    let mut ord = OrderRec::default();
    for r in records {
        if r["k"] != "obs" || r["ev"] != "vis_order" {
            continue;
        }
        let parsed = (|| {
            if !keys_are(r, &["seq", "k", "t", "ev", "u", "v"], &[]) {
                return None;
            }
            let ids: Vec<String> =
                r["v"].as_array()?.iter().map(string).collect::<Option<Vec<_>>>()?;
            Some((r["seq"].as_i64()?, r["t"].as_i64()?, r["u"].as_str()?.to_owned(), ids))
        })();
        let Some((seq, t, u, ids)) = parsed else {
            out.problems.push("vis_order: a record it can't read".into());
            continue;
        };
        if ord.records.insert((t, u), (seq, ids)).is_some() {
            out.problems.push("vis_order: two records of one unit in one frame".into());
        }
        out.expect.push((seq, "vis_order".into()));
    }
    out.orders = Some(ord);
}

fn parse_init(records: &[Rv], rec: &mut BoundsRec, out: &mut Parsed) {
    let bad = |out: &mut Parsed, what: String| out.problems.push(format!("bounds records: {what}"));
    let mut end: Option<(i64, i64, usize, bool)> = None;
    let mut ids = HashSet::new();
    let mut t0: Option<i64> = None;
    for r in records {
        if r["k"] != "obs" {
            continue;
        }
        match r["ev"].as_str() {
            Some("bounds_init") => {
                let parsed = (|| {
                    if !keys_are(
                        r,
                        &[
                            "seq", "k", "t", "ev", "i", "id", "layer", "e", "kind", "r", "on",
                            "pos", "fade",
                        ],
                        &[],
                    ) {
                        return None;
                    }
                    Some((
                        r["t"].as_i64()?,
                        Root {
                            seq: r["seq"].as_i64()?,
                            id: int32(&r["id"])?,
                            layer: int32(&r["layer"])?,
                            e: nullable(&r["e"], string)?,
                            kind: nullable(&r["kind"], string)?,
                            r: nullable(&r["r"], finite)?,
                            on: nullable(&r["on"], |v| v.as_bool())?,
                            pos: vec3(&r["pos"])?,
                            fade: nullable(&r["fade"], |v| v.as_bool())?,
                        },
                        usize::try_from(r["i"].as_i64()?).ok()?,
                    ))
                })();
                let Some((t, root, i)) = parsed else {
                    bad(out, "a bounds_init record it can't read".into());
                    continue;
                };
                if *t0.get_or_insert(t) != t {
                    bad(out, "bounds_init records of two frames".into());
                }
                if i != rec.roots.len() {
                    bad(
                        out,
                        format!("bounds_init root {i} out of order (expected {})", rec.roots.len()),
                    );
                }
                if !ids.insert(root.id) {
                    bad(out, format!("two bounds_init roots with instance id {}", root.id));
                }
                if root.kind.as_deref() == Some(UNIT_KIND)
                    && (root.e.is_none()
                        || root.r.is_none_or(|r| r <= 0.0)
                        || root.on.is_none()
                        || root.layer != UNIT_LAYER)
                {
                    bad(
                        out,
                        format!(
                            "the bounds_init root {} of a unit lacks its entity, radius, enabled flag or has another layer",
                            root.id
                        ),
                    );
                }
                if root.kind.as_deref() != Some(UNIT_KIND) && root.layer == UNIT_LAYER {
                    bad(
                        out,
                        format!(
                            "the bounds_init root {} is on the units' layer and no unit's",
                            root.id
                        ),
                    );
                }
                out.expect.push((root.seq, "bounds_init".into()));
                rec.roots.push(root);
            }
            Some("bounds_init_end") => {
                let parsed = (|| {
                    if !keys_are(r, &["seq", "k", "t", "ev", "n", "valid"], &[]) {
                        return None;
                    }
                    Some((
                        r["seq"].as_i64()?,
                        r["t"].as_i64()?,
                        usize::try_from(r["n"].as_i64()?).ok()?,
                        r["valid"].as_bool()?,
                    ))
                })();
                let Some(e) = parsed else {
                    bad(out, "a bounds_init_end record it can't read".into());
                    continue;
                };
                if end.replace(e).is_some() {
                    bad(out, "two bounds_init_end records".into());
                }
                out.expect.push((e.0, "bounds_init_end".into()));
            }
            _ => {}
        }
    }
    let Some((seq, t, n, valid)) = end else {
        bad(out, "bounds_init without its bounds_init_end".into());
        return;
    };
    if !valid {
        bad(out, "bounds_init_end says the scene was not valid".into());
    }
    if n != rec.roots.len() {
        bad(
            out,
            format!("bounds_init_end counts {n} roots, the recording has {}", rec.roots.len()),
        );
    }
    if t0.is_some_and(|t0| t0 != t) || rec.roots.iter().any(|r| r.seq > seq) {
        bad(out, "bounds_init_end isn't after the roots of its frame".into());
    }
    rec.init_t = t;
    rec.end_seq = seq;
}

fn parse_ticks(records: &[Rv], rec: &mut BoundsRec, out: &mut Parsed) {
    let bad = |out: &mut Parsed, what: String| out.problems.push(format!("bounds records: {what}"));
    // (The update list is a set: an entity is ticked once a frame.)
    let mut ticked: HashSet<(i64, String)> = HashSet::new();
    for r in records {
        if r["k"] != "obs" || r["ev"] != "bounds_tick" {
            continue;
        }
        let parsed = (|| {
            if !keys_are(
                r,
                &["seq", "k", "t", "ev", "e", "kind", "made", "r0", "r", "pos", "on", "c"],
                &["x"],
            ) {
                return None;
            }
            Some((
                r["t"].as_i64()?,
                TickRaw {
                    seq: r["seq"].as_i64()?,
                    e: r["e"].as_str()?.to_owned(),
                    kind: r["kind"].as_str()?.to_owned(),
                    made: r["made"].as_bool()?,
                    r0: nullable(&r["r0"], finite)?,
                    r: nullable(&r["r"], finite)?,
                    pos: nullable(&r["pos"], vec3)?,
                    on: nullable(&r["on"], |v| v.as_bool())?,
                    c: nullable(&r["c"], int32)?,
                },
            ))
        })();
        let Some((t, tick)) = parsed else {
            bad(out, "a bounds_tick record it can't read".into());
            continue;
        };
        if !r["x"].is_null() {
            bad(
                out,
                format!(
                    "TickEntity threw {} on {} (tick {t}): a collider the records may not name",
                    r["x"], tick.e
                ),
            );
        }
        if !ticked.insert((t, tick.e.clone())) {
            bad(out, format!("two bounds_tick records of {} in tick {t}", tick.e));
        }
        if t < rec.init_t {
            bad(
                out,
                format!("a bounds_tick (tick {t}) before the window's start (tick {})", rec.init_t),
            );
        }
        out.expect.push((tick.seq, "bounds_tick".into()));
        rec.ticks.entry(t).or_default().push(tick);
    }
}

fn parse_stream(records: &[Rv], rec: &mut BoundsRec, out: &mut Parsed) {
    let bad = |out: &mut Parsed, what: String| out.problems.push(format!("bounds records: {what}"));
    for r in records {
        if r["k"] != "obs" {
            continue;
        }
        let Some(ev @ ("bounds_set" | "phys_sync" | "bounds_scene")) = r["ev"].as_str() else {
            continue;
        };
        let (Some(seq), Some(t)) = (r["seq"].as_i64(), r["t"].as_i64()) else {
            bad(out, format!("a {ev} record without a seq or a frame"));
            continue;
        };
        if ev == "bounds_scene" {
            // The scene was remade after the window's start: every collider destroyed unseen.
            bad(
                out,
                format!(
                    "the entity-bounds scene was remade at tick {t}: every collider was destroyed (refused)"
                ),
            );
            out.expect.push((seq, ev.into()));
            continue;
        }
        let s = if ev == "phys_sync" {
            (keys_are(r, &["seq", "k", "t", "ev", "src"], &[]) && r["src"].is_string())
                .then_some(Stream::Sync)
        } else {
            parse_set(r)
        };
        let Some(s) = s else {
            bad(out, format!("a {ev} record it can't read (tick {t})"));
            continue;
        };
        if !r["x"].is_null() {
            bad(out, format!("a bounds_set (tick {t}) whose method threw {}", r["x"]));
        }
        out.expect.push((seq, ev.into()));
        rec.stream.insert(seq, (t, s));
    }
}

/// A `bounds_set` record.
fn parse_set(r: &Rv) -> Option<Stream> {
    if !keys_are(
        r,
        &["seq", "k", "t", "ev", "fn", "v", "c", "on0", "on", "pos0", "pos", "e0", "e"],
        &["x"],
    ) {
        return None;
    }
    nullable(&r["v"], string)?;
    let c = nullable(&r["c"], int32)?;
    let on0 = nullable(&r["on0"], |v| v.as_bool())?;
    let on = nullable(&r["on"], |v| v.as_bool())?;
    let pos0 = nullable(&r["pos0"], vec3)?;
    let pos = nullable(&r["pos"], vec3)?;
    let e0 = nullable(&r["e0"], string)?;
    let e = nullable(&r["e"], string)?;
    match r["fn"].as_str()? {
        "active" => Some(Stream::Active { c: c?, on0: on0?, on: on? }),
        "pos" => Some(Stream::Pos { c: c?, pos0: pos0?, pos: pos? }),
        "gone" => Some(Stream::Gone { c: c? }),
        "link" => Some(Stream::Link { c, e0, e }),
        "entity" => Some(Stream::Entity { c: c?, e0, e }),
        _ => None,
    }
}

/// The units whose collider is on a fade-hiding view in `roots` (`resolve`: the sim's unit of an id).
fn faded_units(roots: &[Root], resolve: impl Fn(&str) -> Option<EntityIdx>) -> Vec<EntityIdx> {
    roots
        .iter()
        .filter(|r| r.kind.as_deref() == Some(UNIT_KIND) && r.fade == Some(true))
        .filter_map(|r| r.e.as_deref().and_then(&resolve))
        .collect()
}

impl Driver<'_> {
    /// The window's start: the bounds records and `vis_order` of the recording, validated.
    pub(super) fn bounds_rec_start(&mut self, w: &mut World, declared: &[&str]) {
        let p = parse(&self.trace.records, declared);
        for what in &p.problems {
            Self::vis_report(w, what);
        }
        // (A refused family is reported once, as a whole: its records aren't expected one by one.)
        for (seq, ev) in p.expect.iter().filter(|(_, ev)| p.rec.is_some() || ev == "vis_order") {
            self.vis.expect_record(*seq, ev);
        }
        self.vis.range.rec = p.rec;
        self.vis.range.orders = p.orders;
        self.vis.counts.rec.used = self.vis.range.rec.is_some();
    }

    /// Whether the colliders come from the records in this run.
    pub(super) fn bounds_rec_on(&self) -> bool {
        self.vis.range.rec.is_some()
    }

    /// A `bounds_set` or `phys_sync` record (or a `bounds_scene` one: refused at the start, only
    /// consumed here): queued for the frame's vision phase, as before or after it ran.
    pub(super) fn bounds_stream(&mut self, w: &mut World, r: &Rv) {
        let Some(seq) = r["seq"].as_i64() else {
            Self::vis_report(w, "a bounds record without a seq");
            return;
        };
        if !self.vis.consume_record(seq) {
            Self::vis_report(w, "a bounds record handled twice");
            return;
        }
        let Some(rec) = self.vis.range.rec.as_mut() else { return };
        let Some((t, s)) = rec.stream.remove(&seq) else { return };
        if t != self.tick {
            Self::vis_report(w, "a bounds record of another frame");
            return;
        }
        let late = self.vis.range.phase_ran_at == Some(self.tick);
        rec.queue.push(Queued { seq, t, late, s });
    }

    /// `EntityBoundsController.Tick`'s records of the frame (and of the frames the sim didn't
    /// follow before it), applied to the colliders; at the window's first frame they are made
    /// from `bounds_init`.
    pub(super) fn bounds_rec_tick(
        &mut self,
        w: &mut World,
        list: &[BoundsUnit],
        step: Step,
        first: bool,
    ) {
        let tick = self.tick;
        let Some(mut rec) = self.vis.range.rec.take() else { return };
        if first && !self.bounds_rec_init(w, &mut rec, list) {
            // (Reported: the run goes on with the hypotheses.)
            self.vis.counts.rec.used = false;
            self.vis.forget_records(&FAMILY_RECORDS);
            return;
        }
        // The frames before this one the phase didn't run in (not followed), then this frame.
        let due: Vec<i64> = rec.ticks.range(..=tick).map(|(t, _)| *t).collect();
        let mut skipped: Vec<RecTick> = Vec::new();
        let mut now: Vec<RecTick> = Vec::new();
        let mut made_now: Vec<(i32, EntityIdx)> = Vec::new();
        for t in due {
            let raws = rec.ticks.remove(&t).unwrap_or_default();
            for raw in raws {
                self.vis.consume_record(raw.seq);
                let Some(tr) = self.bounds_rec_unit(w, &mut rec, &raw) else { continue };
                if tr.made
                    && t == tick
                    && let Some(c) = tr.id
                {
                    made_now.push((c, tr.unit));
                }
                if t == tick { now.push(tr) } else { skipped.push(tr) }
            }
        }
        let (late, early) = self.bounds_rec_events(w, &mut rec, tick, &made_now);
        let sync_now = early.iter().chain(&late).any(|e| e.write == Write::Sync);
        if !skipped.is_empty() {
            let out = w.combat.vision.bounds.tick_recorded(Step::Unknown, &[], &skipped, &[]);
            self.bounds_rec_out(w, out);
        }
        let units: &[BoundsUnit] = list;
        let mut events = late;
        events.extend(early);
        let out = w.combat.vision.bounds.tick_recorded(step, &events, &now, units);
        self.bounds_rec_out(w, out);
        let c = &mut self.vis.counts.rec;
        let flagged = std::mem::take(&mut w.combat.vision.explicit_sync);
        c.sync_frames += usize::from(sync_now);
        c.sync_flagged += usize::from(sync_now && flagged);
        c.sync_unflagged += usize::from(sync_now && !flagged);
        c.sync_flag_only += usize::from(flagged && !sync_now);
        c.views_replaced += std::mem::take(&mut w.combat.vision.views_replaced).len();
        self.vis.range.rec = Some(rec);
    }

    /// The queued stream records as writes to the colliders: those of earlier frames and those
    /// that came after their frame's phase are `late`, the others `early`. `made_now`: the
    /// colliders the frame's calls made, with their units (an `entity` record must be one's link).
    /// Returns (late, early).
    fn bounds_rec_events(
        &mut self,
        w: &mut World,
        rec: &mut BoundsRec,
        tick: i64,
        made_now: &[(i32, EntityIdx)],
    ) -> (Vec<Event>, Vec<Event>) {
        let mut late: Vec<Event> = Vec::new();
        let mut early: Vec<Event> = Vec::new();
        let mut entity_events: Vec<Queued> = Vec::new();
        for q in std::mem::take(&mut rec.queue) {
            let is_late = q.late || q.t < tick;
            let write = match &q.s {
                Stream::Active { c, on0, on } => {
                    Some((*c, Write::Active { id: *c, on0: *on0, on: *on }))
                }
                Stream::Pos { c, pos0, pos } => {
                    Some((*c, Write::Pos { id: *c, pos0: *pos0, pos: *pos }))
                }
                Stream::Gone { c } => Some((*c, Write::Gone { id: *c })),
                Stream::Sync => {
                    let e = Event { seq: q.seq, write: Write::Sync, late: is_late };
                    if is_late {
                        late.push(e)
                    } else {
                        early.push(e)
                    }
                    continue;
                }
                Stream::Link { c, e0, e } => {
                    // The view's collider relinked to the view's new data. With no collider
                    // (the view has none) nothing changes; with one, it must exist and be bound
                    // to the entity named, whether the relink changes it or not.
                    match c {
                        None => self.vis.counts.rec.links += 1,
                        Some(c) if rec.other_ids.contains(c) && e0 != e => {
                            self.vis.counts.rec.other_writes += 1;
                        }
                        Some(c) if e0 == e && e.is_some() && rec.bound.get(c) == e.as_ref() => {
                            self.vis.counts.rec.links += 1;
                        }
                        Some(c) => Self::vis_report(
                            w,
                            &format!(
                                "bounds records: a link record of collider {c} from {e0:?} to {e:?}: the collider is bound to {:?} (an unknown collider, a contradictory binding or a relink, not modelled)",
                                rec.bound.get(c)
                            ),
                        ),
                    }
                    continue;
                }
                Stream::Entity { .. } => {
                    entity_events.push(q);
                    continue;
                }
            };
            let Some((c, write)) = write else { continue };
            if rec.other_ids.contains(&c) {
                self.vis.counts.rec.other_writes += 1;
                continue;
            }
            let e = Event { seq: q.seq, write, late: is_late };
            if is_late { late.push(e) } else { early.push(e) }
        }
        for q in entity_events {
            let Stream::Entity { c, e0, e } = &q.s else { continue };
            // The link of a collider the frame's tick made: the entity of its unit.
            let made = made_now.iter().find(|(id, _)| id == c);
            let unit_id = made.map(|(_, u)| w.entity(*u).unique_id.as_str());
            if rec.other_ids.contains(c) {
                self.vis.counts.rec.other_writes += 1;
            } else if e0.is_none() && made.is_some() && e.as_deref() == unit_id {
                self.vis.counts.rec.links += 1;
            } else {
                Self::vis_report(
                    w,
                    &format!("a collider {c} linked from {e0:?} to {e:?}: not modelled"),
                );
            }
        }
        (late, early)
    }

    /// The run's end: the writes queued after the last frame's phase have no controller tick
    /// after them; they are checked against the colliders as they stand.
    pub(super) fn bounds_rec_finish(&mut self, w: &mut World) {
        let Some(mut rec) = self.vis.range.rec.take() else { return };
        let (late, early) = self.bounds_rec_events(w, &mut rec, i64::MAX, &[]);
        debug_assert!(early.is_empty());
        let out = w.combat.vision.bounds.finish_recorded(&late);
        self.bounds_rec_out(w, out);
        self.vis.range.rec = Some(rec);
    }

    /// The reports and counts of a recorded tick.
    fn bounds_rec_out(&mut self, w: &mut World, out: range::RecOut) {
        self.vis.counts.rec.rec.add(&out.counts);
        for (u, what) in out.problems {
            let who = u.map(|u| format!(" ({})", w.entity(u).unique_id)).unwrap_or_default();
            Self::vis_report(w, &format!("bounds records: {what}{who}"));
        }
    }

    /// A `bounds_tick` of a unit as the colliders take it; `None` for another kind of entity or
    /// a unit the sim doesn't have (the latter reported).
    fn bounds_rec_unit(
        &mut self,
        w: &mut World,
        rec: &mut BoundsRec,
        t: &TickRaw,
    ) -> Option<RecTick> {
        if t.made
            && let Some(c) = t.c
        {
            rec.bound.insert(c, t.e.clone());
        }
        if t.kind != UNIT_KIND {
            self.vis.counts.rec.other_ticks += 1;
            if t.made
                && let Some(c) = t.c
            {
                rec.other_ids.insert(c);
            }
            return None;
        }
        let Some(&u) = w.by_id.get(&t.e).filter(|u| w.unit(**u).is_some()) else {
            Self::vis_report(w, &format!("a bounds_tick of {}, a unit the sim doesn't have", t.e));
            return None;
        };
        if t.made
            && let Some(c) = t.c
        {
            rec.unit_ids.insert(c);
        }
        Some(RecTick {
            seq: t.seq,
            unit: u,
            made: t.made,
            r0: t.r0,
            r: t.r,
            pos: t.pos,
            on: t.on,
            id: t.c,
        })
    }

    /// The units the recording's `bounds_init` shows with a fade-hiding view: marked before the
    /// frame's first list of units is built, so their `enabled` is unknown there as it is in the
    /// game.
    pub(super) fn bounds_rec_fade_marks(&mut self, w: &mut World) {
        let Some(rec) = self.vis.range.rec.as_ref() else { return };
        let resolve = |e: &str| w.by_id.get(e).copied().filter(|u| w.unit(*u).is_some());
        let marked = faded_units(&rec.roots, resolve);
        w.combat.vision.fade_hidden.extend(marked);
    }

    /// The window's first frame: the colliders from `bounds_init`, in the scene's order. `false`
    /// (reported) when the records don't fit the run.
    fn bounds_rec_init(&mut self, w: &mut World, rec: &mut BoundsRec, list: &[BoundsUnit]) -> bool {
        if rec.init_t != self.tick {
            Self::vis_report(
                w,
                &format!(
                    "bounds records: bounds_init is of tick {}, the bounds controller's first tick the sim follows is {}",
                    rec.init_t, self.tick
                ),
            );
            return false;
        }
        let mut roots: Vec<RecCollider> = Vec::new();
        let mut ok = true;
        let mut seen: HashSet<EntityIdx> = HashSet::new();
        for root in &rec.roots {
            self.vis.consume_record(root.seq);
            if let Some(e) = &root.e {
                rec.bound.insert(root.id, e.clone());
            }
            if root.kind.as_deref() != Some(UNIT_KIND) {
                rec.other_ids.insert(root.id);
                self.vis.counts.rec.other_roots += 1;
                continue;
            }
            let unit = root
                .e
                .as_ref()
                .and_then(|e| w.by_id.get(e).copied())
                .filter(|u| w.unit(*u).is_some());
            let (Some(u), Some(r), Some(on)) = (unit, root.r, root.on) else {
                Self::vis_report(
                    w,
                    &format!(
                        "bounds records: the bounds_init collider {} of {:?}, a unit the sim doesn't have",
                        root.id, root.e
                    ),
                );
                ok = false;
                continue;
            };
            if !seen.insert(u) {
                Self::vis_report(
                    w,
                    &format!("bounds records: two bounds_init colliders of {:?}", root.e),
                );
                ok = false;
                continue;
            }
            rec.unit_ids.insert(root.id);
            self.vis.counts.rec.roots += 1;
            // The collider stands where the unit stood at the end of the previous frame.
            match list.iter().find(|x| x.unit == u).and_then(|x| x.prefix) {
                Some(p) if p.map(f32::to_bits) == root.pos.map(f32::to_bits) => {
                    self.vis.counts.rec.start_equal += 1
                }
                Some(p) if orientation::vec3_eq(p, root.pos) => self.vis.counts.rec.start_near += 1,
                Some(p) => {
                    Self::vis_report(
                        w,
                        &format!(
                            "bounds records: the collider of {:?} starts at {:?}, the unit stood at {p:?} at the previous frame's end",
                            root.e, root.pos
                        ),
                    );
                    ok = false;
                }
                None => self.vis.counts.rec.start_unchecked += 1,
            }
            roots.push(RecCollider { unit: u, id: root.id, radius: r, enabled: on, pos: root.pos });
        }
        self.vis.consume_record(rec.end_seq);
        // The creation order against the pool's.
        let pool = crate::scene::units_with_bounds(w);
        let sim: Vec<EntityIdx> = pool.iter().copied().filter(|u| seen.contains(u)).collect();
        let recorded: Vec<EntityIdx> = roots.iter().map(|r| r.unit).collect();
        self.vis.counts.rec.order_units += recorded.len();
        if sim != recorded {
            let name = |l: &[EntityIdx]| {
                l.iter().map(|u| w.entity(*u).unique_id.clone()).collect::<Vec<_>>().join(" ")
            };
            Self::vis_report(
                w,
                &format!(
                    "bounds records: the colliders' creation order in bounds_init isn't the pool's: the scene lists {}; the pool is {}",
                    name(&recorded),
                    name(&sim)
                ),
            );
            ok = false;
        }
        for u in &pool {
            let in_game = w.entity(*u).is_in_game;
            let c = &mut self.vis.counts.rec;
            c.hidden_with_collider += usize::from(!in_game && seen.contains(u));
            c.in_game_without += usize::from(in_game && !seen.contains(u));
        }
        if ok {
            w.combat.vision.bounds.start_recorded(&roots, list);
        }
        ok
    }

    /// The recorded order of a unit's set at this frame (`vis_order`), when it has a record.
    pub(super) fn vis_order_apply(&mut self, w: &mut World, u: EntityIdx) {
        let tick = self.tick;
        let id = w.entity(u).unique_id.clone();
        let Some(ord) = self.vis.range.orders.as_mut() else { return };
        let Some((seq, ids)) = ord.records.remove(&(tick, id.clone())) else {
            if !self.vis.range.order_last.contains_key(&u) {
                Self::vis_report(
                    w,
                    &format!(
                        "vis_order: the order of {id}'s set was never recorded (the first is always written)"
                    ),
                );
            }
            return;
        };
        self.vis.consume_record(seq);
        self.vis.counts.rec.order_records += 1;
        let units: Option<Vec<EntityIdx>> = ids.iter().map(|x| w.by_id.get(x).copied()).collect();
        let Some(units) = units else {
            Self::vis_report(
                w,
                &format!("vis_order: the order of {id}'s set names a unit the sim doesn't have"),
            );
            return;
        };
        // One static order of the colliders (PhysX's result order, which isn't the creation
        // order everywhere): a record that puts a pair the other way round from an earlier one
        // contradicts it.
        let mut conflict = None;
        for (i, a) in units.iter().enumerate() {
            for b in &units[i + 1..] {
                if self.vis.range.order_pairs.contains(&(*b, *a)) {
                    conflict.get_or_insert((*a, *b));
                }
                self.vis.range.order_pairs.insert((*a, *b));
            }
        }
        if let Some((a, b)) = conflict {
            let (a, b) = (w.entity(a).unique_id.clone(), w.entity(b).unique_id.clone());
            Self::vis_report(
                w,
                &format!(
                    "vis_order: the order of {id}'s set has {a} before {b}, an earlier record had {b} before {a}"
                ),
            );
        }
        self.vis.range.order_last.insert(u, units);
    }

    /// A unit's set order against the recorded one, when the recording has `vis_order`. `order`:
    /// the sim's (the overlap's, then the units without a collider by index); `exact`: the sim's
    /// order holds only colliders whose place is known. Returns the order to use.
    pub(super) fn vis_order_check(
        &mut self,
        w: &mut World,
        u: EntityIdx,
        set: &HashSet<EntityIdx>,
        order: &[EntityIdx],
        exact: bool,
    ) -> Option<Vec<EntityIdx>> {
        let rec_order = self.vis.range.order_last.get(&u)?.clone();
        let id = w.entity(u).unique_id.clone();
        let rec_set: HashSet<EntityIdx> = rec_order.iter().copied().collect();
        if rec_set.len() != rec_order.len() || &rec_set != set {
            Self::vis_report(
                w,
                &format!("vis_order: the order of {id}'s set isn't a permutation of its vis set"),
            );
            return None;
        }
        let c = &mut self.vis.counts.rec;
        // PhysX's result order is its pruner's, which the sim doesn't port: the creation order
        // it assumes is right for most sets, and the recording's is the order either way.
        if !exact {
            c.order_taken += 1;
        } else if order == rec_order.as_slice() {
            c.order_equal += 1;
        } else {
            c.order_differ += 1;
        }
        Some(rec_order)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn rv(v: serde_json::Value) -> Rv {
        Rv::from_json(&v)
    }

    const ALL: [&str; 6] =
        ["bounds_init", "bounds_tick", "bounds_set", "bounds_scene", "phys_sync", "vis_order"];

    /// The records with the `tick` record of each frame before its first (the stream's frames).
    fn framed(recs: Vec<Rv>) -> Vec<Rv> {
        let mut out = Vec::new();
        let mut cur = None;
        for r in recs {
            let t = r["t"].as_i64();
            if r["k"] == "obs" && t != cur {
                out.push(rv(json!({"seq": -1, "k": "tick", "t": t})));
                cur = t;
            }
            out.push(r);
        }
        out
    }

    fn init(seq: i64, i: i64, id: i64, e: &str) -> Rv {
        rv(
            json!({"seq": seq, "k": "obs", "t": 0, "ev": "bounds_init", "i": i, "id": id, "layer": 23,
            "e": e, "kind": "UnitEntityData", "r": 0.5, "on": true, "pos": [1.0, 2.0, 3.0], "fade": false}),
        )
    }

    fn end(seq: i64, n: i64) -> Rv {
        rv(json!({"seq": seq, "k": "obs", "t": 0, "ev": "bounds_init_end", "n": n, "valid": true}))
    }

    fn tick(seq: i64, t: i64) -> Rv {
        rv(
            json!({"seq": seq, "k": "obs", "t": t, "ev": "bounds_tick", "e": "a", "kind": "UnitEntityData",
            "made": false, "r0": 0.5, "r": 0.5, "pos": [1.0, 2.0, 3.0], "on": true, "c": -8}),
        )
    }

    #[test]
    fn a_whole_family_is_indexed_and_a_part_of_it_is_refused() {
        let recs = framed(vec![init(0, 0, -8, "a"), end(1, 1), tick(2, 0), tick(3, 1)]);
        let p = parse(&recs, &ALL);
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        let rec = p.rec.expect("the family");
        assert_eq!((rec.init_t, rec.roots.len(), rec.ticks.len()), (0, 1, 2));
        assert_eq!(p.expect.len(), 4);
        // Not declared at all: the hypotheses (and records without a declaration are reported).
        let p = parse(&recs, &["memory"]);
        assert!(p.rec.is_none() && p.problems.len() == 1, "{:?}", p.problems);
        // Declared in part.
        let p = parse(&recs, &["bounds_init", "bounds_tick"]);
        assert!(p.rec.is_none() && p.problems.len() == 1, "{:?}", p.problems);
        // Nothing declared, nothing recorded: nothing to report.
        let p = parse(&[], &[]);
        assert!(p.rec.is_none() && p.problems.is_empty());
    }

    #[test]
    fn a_record_that_does_not_fit_is_reported_and_the_family_refused() {
        let cases: Vec<(&str, Vec<Rv>)> = vec![
            ("no end", vec![init(0, 0, -8, "a")]),
            ("wrong count", vec![init(0, 0, -8, "a"), end(1, 2)]),
            ("out of order", vec![init(0, 1, -8, "a"), end(1, 1)]),
            ("duplicate id", vec![init(0, 0, -8, "a"), init(1, 1, -8, "b"), end(2, 2)]),
            (
                "a missing field",
                vec![
                    rv(json!({"seq": 0, "k": "obs", "t": 0, "ev": "bounds_init", "i": 0, "id": -8,
                    "layer": 23, "e": "a", "kind": "UnitEntityData", "r": 0.5, "on": true, "pos": [1.0, 2.0, 3.0]})),
                    end(1, 1),
                ],
            ),
            (
                "a radius that is no number",
                vec![
                    rv(json!({"seq": 0, "k": "obs", "t": 0, "ev": "bounds_init", "i": 0, "id": -8,
                    "layer": 23, "e": "a", "kind": "UnitEntityData", "r": "0.5", "on": true, "pos": [1.0, 2.0, 3.0], "fade": false})),
                    end(1, 1),
                ],
            ),
            (
                "an invalid scene",
                vec![
                    init(0, 0, -8, "a"),
                    rv(
                        json!({"seq": 1, "k": "obs", "t": 0, "ev": "bounds_init_end", "n": 1, "valid": false}),
                    ),
                ],
            ),
            (
                "a tick that threw",
                vec![
                    init(0, 0, -8, "a"),
                    end(1, 1),
                    rv(json!({"seq": 2, "k": "obs", "t": 0, "ev": "bounds_tick", "e": "a",
                    "kind": "UnitEntityData", "made": false, "r0": null, "r": null, "pos": null, "on": null, "c": null, "x": "NullReferenceException"})),
                ],
            ),
            (
                "a tick with a field of the wrong type",
                vec![
                    init(0, 0, -8, "a"),
                    end(1, 1),
                    rv(json!({"seq": 2, "k": "obs", "t": 0, "ev": "bounds_tick", "e": "a",
                    "kind": "UnitEntityData", "made": 1, "r0": 0.5, "r": 0.5, "pos": [1.0, 2.0, 3.0], "on": true, "c": -8})),
                ],
            ),
            (
                "a scene remade",
                vec![
                    init(0, 0, -8, "a"),
                    end(1, 1),
                    rv(json!({"seq": 2, "k": "obs", "t": 5, "ev": "bounds_scene"})),
                ],
            ),
            (
                "a set that does not fit",
                vec![
                    init(0, 0, -8, "a"),
                    end(1, 1),
                    rv(json!({"seq": 2, "k": "obs", "t": 5, "ev": "bounds_set", "fn": "active",
                    "v": "a", "c": null, "on0": true, "on": false, "pos0": null, "pos": null, "e0": "a", "e": "a"})),
                ],
            ),
            (
                "a non-finite position",
                vec![
                    init(0, 0, -8, "a"),
                    end(1, 1),
                    rv(json!({"seq": 2, "k": "obs", "t": 0, "ev": "bounds_tick", "e": "a",
                    "kind": "UnitEntityData", "made": false, "r0": 0.5, "r": 0.5, "pos": [1.0, null, 3.0], "on": true, "c": -8})),
                ],
            ),
            (
                "a sync without a source",
                vec![
                    init(0, 0, -8, "a"),
                    end(1, 1),
                    rv(json!({"seq": 2, "k": "obs", "t": 5, "ev": "phys_sync"})),
                ],
            ),
        ];
        for (name, recs) in cases {
            let p = parse(&framed(recs), &ALL);
            assert!(p.rec.is_none() && !p.problems.is_empty(), "{name}: {:?}", p.problems);
        }
    }

    #[test]
    fn vis_order_records_are_indexed_by_frame_and_unit() {
        let rec = |seq: i64, t: i64, u: &str, v: serde_json::Value| {
            rv(json!({"seq": seq, "k": "obs", "t": t, "ev": "vis_order", "u": u, "v": v}))
        };
        let recs =
            framed(vec![rec(0, 0, "a", json!(["a", "b"])), rec(1, 3, "a", json!(["b", "a"]))]);
        let p = parse(&recs, &["vis_order"]);
        assert!(p.problems.is_empty(), "{:?}", p.problems);
        let o = p.orders.expect("orders");
        assert_eq!(o.records.len(), 2);
        // Two records of one unit in one frame, an id that isn't a string, undeclared.
        let dup = framed(vec![rec(0, 0, "a", json!([])), rec(1, 0, "a", json!([]))]);
        assert!(!parse(&dup, &["vis_order"]).problems.is_empty());
        assert!(
            !parse(&framed(vec![rec(0, 0, "a", json!([1]))]), &["vis_order"]).problems.is_empty()
        );
        assert!(!parse(&recs, &[]).problems.is_empty());
    }

    #[test]
    fn a_record_must_stand_in_the_frame_it_names() {
        let ok = framed(vec![init(0, 0, -8, "a"), end(1, 1), tick(2, 0), tick(3, 1)]);
        assert!(parse(&ok, &ALL).problems.is_empty());
        // The call of tick 1 moved before tick 1's `tick` record, its `t` kept (seq renumbered).
        let mut moved = ok.clone();
        let call = moved.len() - 1;
        moved.swap(call, call - 1);
        let p = parse(&moved, &ALL);
        assert!(
            p.rec.is_none() && p.problems.iter().any(|x| x.contains("claims tick")),
            "{:?}",
            p.problems
        );
        // The init block after the window's first call, or the calls before its end.
        let late_init = framed(vec![tick(0, 0), init(1, 0, -8, "a"), end(2, 1)]);
        assert!(parse(&late_init, &ALL).problems.iter().any(|x| x.contains("bounds_init")));
        let early_call = framed(vec![init(0, 0, -8, "a"), tick(1, 0), end(2, 1)]);
        assert!(
            parse(&early_call, &ALL).problems.iter().any(|x| x.contains("before bounds_init_end"))
        );
        // A frame's calls after its vis_order records (the bounds controller comes first), and a
        // vis_order moved across a frame boundary.
        let order = |seq: i64, t: i64| {
            rv(json!({"seq": seq, "k": "obs", "t": t, "ev": "vis_order", "u": "a", "v": ["a"]}))
        };
        let wrong = framed(vec![init(0, 0, -8, "a"), end(1, 1), order(2, 0), tick(3, 0)]);
        let p = parse(&wrong, &ALL);
        assert!(
            p.problems.iter().any(|x| x.contains("after the frame's vis_order")),
            "{:?}",
            p.problems
        );
        let mut crossed = framed(vec![order(0, 0), order(1, 1)]);
        crossed.swap(1, 2);
        let p = parse(&crossed, &["vis_order"]);
        assert!(
            !p.problems.is_empty() && p.problems.iter().all(|x| x.starts_with("vis_order")),
            "{:?}",
            p.problems
        );
        // (A frame problem of vis_order alone doesn't refuse the family.)
        let mut both = framed(vec![init(0, 0, -8, "a"), end(1, 1), order(2, 0)]);
        both.push(rv(json!({"seq": 9, "k": "tick", "t": 1})));
        both.push(order(10, 0));
        let p = parse(&both, &ALL);
        assert!(p.rec.is_some() && !p.problems.is_empty());
    }

    #[test]
    fn an_entity_is_ticked_once_a_frame() {
        let dup = framed(vec![init(0, 0, -8, "a"), end(1, 1), tick(2, 1), tick(3, 1)]);
        let p = parse(&dup, &ALL);
        assert!(
            p.rec.is_none() && p.problems.iter().any(|x| x.contains("two bounds_tick")),
            "{:?}",
            p.problems
        );
    }

    #[test]
    fn a_fade_hiding_unit_is_marked_from_its_root() {
        let roots: Vec<Root> = (0..3)
            .map(|i| Root {
                seq: i,
                id: -8 - i as i32,
                layer: 23,
                e: Some(["a", "b", "c"][i as usize].to_owned()),
                kind: Some(UNIT_KIND.into()),
                r: Some(0.5),
                on: Some(true),
                pos: [0.0; 3],
                fade: Some(i == 1),
            })
            .collect();
        let resolve = |e: &str| (e != "c").then(|| EntityIdx(e.as_bytes()[0] as u32));
        assert_eq!(faded_units(&roots, resolve), vec![EntityIdx(u32::from(b'b'))]);
        let mut all = roots.clone();
        all[1].kind = Some("MapObjectEntityData".into());
        assert!(faded_units(&all, resolve).is_empty());
    }
}
