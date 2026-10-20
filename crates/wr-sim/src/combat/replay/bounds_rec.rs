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
    self, BoundsUnit, Event, Hit, RecCollider, RecCounts, RecTick, Step, Tri, Write,
};
use crate::world::{EntityIdx, World};

use super::Driver;
use super::bounds_life::{Ev, Kind, Lifecycle, SetFn, SetRec};

/// The observers installed together (WrathOracle's `BoundsOn`).
const FAMILY: [&str; 5] = ["bounds_init", "bounds_tick", "bounds_set", "bounds_scene", "phys_sync"];

/// Records of the family (`bounds_init_end` besides the observers' names).
const RECORDS: [&str; 6] =
    ["bounds_init", "bounds_init_end", "bounds_tick", "bounds_set", "bounds_scene", "phys_sync"];

/// The kinds of record the family is made of, as `unconsumed` names them.
const FAMILY_RECORDS: [&str; 5] =
    ["bounds_init", "bounds_init_end", "bounds_tick", "bounds_set", "phys_sync"];

/// The callers of `Physics.SyncTransforms()` the oracle taps (CommandObsBounds.cs), and DollRoom's
/// `Simulate`.
const SYNC_SOURCES: [&str; 5] = [
    "Polymorph.RestoreView",
    "UnitPlaceOnGroundController.Tick",
    "GameActionSetIsleState.RunAction",
    "IsleStateControllerView.ResetAllIslands",
    "DollRoom.Update",
];

const UNIT_KIND: &str = "UnitEntityData";
/// `Layers.SphereBoundsUnit`, and `Layers.SphereBoundsObject` of every other entity's.
const UNIT_LAYER: i32 = 23;
const OBJECT_LAYER: i32 = 24;

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
    /// A `bounds_set` with every field it carries.
    Set {
        f: SetFn,
        v: Option<String>,
        c: Option<i32>,
        on0: Option<bool>,
        on: Option<bool>,
        pos0: Option<V3>,
        pos: Option<V3>,
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
    /// The life of every collider, units' and others': each record is checked against it.
    life: Lifecycle,
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
    /// Colliders the lifecycle machine saw in all (roots and makings).
    pub colliders: usize,
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
    /// Units with several colliders that may be hit in an observer's query, whose place in a
    /// recorded order couldn't be tied to one (each reported).
    pub order_ambiguous: usize,
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
    // The units whose `vis` record the frame has had (the oracle writes a unit's changed order
    // just before its `vis`), and the first `vis_order` of each frame by stream place, against the
    // block of `bounds_init` records (written at the window's first prefix, before the
    // controllers).
    let mut vis_units: HashSet<String> = HashSet::new();
    let mut first_order: HashMap<i64, usize> = HashMap::new();
    let mut first_vis: HashMap<i64, usize> = HashMap::new();
    let mut vision_seen = false;
    let mut init_place: Option<(i64, usize)> = None;
    for (at, r) in records.iter().enumerate() {
        if r["k"] == "tick" {
            frame = r["t"].as_i64();
            order_seen = false;
            vision_seen = false;
            vis_units.clear();
            continue;
        }
        if r["k"] != "obs" {
            continue;
        }
        let Some(ev) = r["ev"].as_str() else { continue };
        if (orders || family) && ev == "vis" {
            // (The vision controller follows the bounds controller whatever is declared.)
            if let Some(u) = r["u"].as_str() {
                vis_units.insert(u.to_owned());
            }
            vision_seen = true;
            if let Some(f) = frame {
                first_vis.entry(f).or_insert(at);
            }
            continue;
        }
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
                if order_seen || vision_seen {
                    into.push(format!(
                        "{prefix}: a bounds_tick (seq {seq:?}) after the frame's vis or vis_order records (the bounds controller precedes the vision controller)"
                    ));
                }
                tick_seen = true;
            }
            "bounds_set" | "phys_sync" | "bounds_scene" if !init_end => {
                // (`BoundsInit` writes its snapshot synchronously, with getters that write
                // nothing, before any controller: no setter or sync is inside or before it.)
                into.push(format!(
                    "{prefix}: a {ev} (seq {seq:?}) before the end of the bounds_init block"
                ));
            }
            "bounds_init" if tick_seen || init_end => {
                into.push(format!(
                    "{prefix}: a bounds_init (seq {seq:?}) after the window's first call or end"
                ));
            }
            "bounds_init_end" => {
                init_end = true;
                init_place = frame.map(|f| (f, at));
            }
            "vis_order" => {
                order_seen = true;
                if r["u"].as_str().is_some_and(|u| vis_units.contains(u)) {
                    ord.push(format!(
                        "vis_order: the order of {} (seq {seq:?}) stands after its unit's vis record (the oracle writes it just before)",
                        r["u"]
                    ));
                }
                if let Some(f) = frame {
                    first_order.entry(f).or_insert(at);
                }
            }
            _ => {}
        }
    }
    if let Some((f, end_at)) = init_place {
        if first_order.get(&f).is_some_and(|o| *o < end_at) {
            ord.push(format!(
                "vis_order: a record of tick {f} stands before the bounds_init block (written at the frame's prefix, before the controllers)"
            ));
        }
        if first_vis.get(&f).is_some_and(|o| *o < end_at) {
            fam.push(format!(
                "bounds records: a vis record of tick {f} stands before the bounds_init block (written at the frame's prefix, before the controllers)"
            ));
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
                // Every root of the scene is a collider `SetupSphereBoundsCollider` made: a radius of
                // at least 0.05, an `enabled` flag, the units' layer for a unit's and the objects'
                // for any other, and a unit's has its entity.
                let unit = root.kind.as_deref() == Some(UNIT_KIND);
                // (`BoundsInit` derives the type from the entity: both or neither, and a scene
                // root is a collider of an entity.)
                if root.kind.is_none()
                    || root.e.is_none()
                    || root.r.is_none_or(|r| r < 0.05)
                    || root.on.is_none()
                    || root.layer != if unit { UNIT_LAYER } else { OBJECT_LAYER }
                {
                    bad(
                        out,
                        format!(
                            "the bounds_init root {} isn't a collider the game makes (kind {:?}, radius {:?}, enabled {:?}, layer {}, entity {:?})",
                            root.id, root.kind, root.r, root.on, root.layer, root.e
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
            (keys_are(r, &["seq", "k", "t", "ev", "src"], &[])
                && r["src"].as_str().is_some_and(|x| SYNC_SOURCES.contains(&x)))
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

/// A `bounds_set` record, every field kept (`fn` and the types are checked here, the values by
/// the lifecycle machine).
fn parse_set(r: &Rv) -> Option<Stream> {
    if !keys_are(
        r,
        &["seq", "k", "t", "ev", "fn", "v", "c", "on0", "on", "pos0", "pos", "e0", "e"],
        &["x"],
    ) {
        return None;
    }
    let f = match r["fn"].as_str()? {
        "active" => SetFn::Active,
        "pos" => SetFn::Pos,
        "gone" => SetFn::Gone,
        "link" => SetFn::Link,
        "entity" => SetFn::Entity,
        _ => return None,
    };
    Some(Stream::Set {
        f,
        v: nullable(&r["v"], string)?,
        c: nullable(&r["c"], int32)?,
        on0: nullable(&r["on0"], |v| v.as_bool())?,
        on: nullable(&r["on"], |v| v.as_bool())?,
        pos0: nullable(&r["pos0"], vec3)?,
        pos: nullable(&r["pos"], vec3)?,
        e0: nullable(&r["e0"], string)?,
        e: nullable(&r["e"], string)?,
    })
}

/// The units whose collider is on a fade-hiding view in `roots` (`resolve`: the sim's unit of an id).
fn faded_units(roots: &[Root], resolve: impl Fn(&str) -> Option<EntityIdx>) -> Vec<EntityIdx> {
    roots
        .iter()
        .filter(|r| r.kind.as_deref() == Some(UNIT_KIND) && r.fade == Some(true))
        .filter_map(|r| r.e.as_deref().and_then(&resolve))
        .collect()
}

/// The order the recorded sets put the colliders in (PhysX's result order, which isn't the
/// creation order everywhere): one static order of the colliders, so the pairs the records put
/// (a before b) must have no cycle, however far apart the records are. Colliders are keyed by
/// identity (the instance id), so a unit's destroyed collider and its recreated one are two.
#[derive(Debug, Default)]
pub(super) struct OrderGraph {
    after: HashMap<i32, HashSet<i32>>,
}

impl OrderGraph {
    /// Whether `from` is (transitively) before `to`.
    fn reaches(&self, from: i32, to: i32) -> bool {
        let mut seen = HashSet::new();
        let mut stack = vec![from];
        while let Some(x) = stack.pop() {
            if x == to {
                return true;
            }
            if seen.insert(x) {
                stack.extend(self.after.get(&x).into_iter().flatten().copied());
            }
        }
        false
    }

    /// Records the pairs `order` puts; the first pair (a before b) that the pairs held reach the
    /// other way round, if any (not recorded).
    pub(super) fn add(&mut self, order: &[i32]) -> Option<(i32, i32)> {
        let mut conflict = None;
        for (i, a) in order.iter().enumerate() {
            for b in &order[i + 1..] {
                if a == b || self.after.get(a).is_some_and(|s| s.contains(b)) {
                    continue;
                }
                if self.reaches(*b, *a) {
                    conflict.get_or_insert((*a, *b));
                } else {
                    self.after.entry(*a).or_default().insert(*b);
                }
            }
        }
        conflict
    }
}

/// The collider each unit of an observer's recorded order stands for in its query (`hits`: the
/// colliders the overlap may hit): the one it may have hit. A unit with none that may be hit is
/// left out (the set differs, which is reported where the sets are compared); a unit with
/// several can't be tied to one, and is listed apart.
fn order_keys(order: &[EntityIdx], hits: &[Hit]) -> (Vec<Option<i32>>, Vec<EntityIdx>) {
    let mut keys = Vec::with_capacity(order.len());
    let mut ambiguous = Vec::new();
    for &x in order {
        let cands: Vec<&Hit> = hits.iter().filter(|h| h.unit == x && h.tri != Tri::Out).collect();
        keys.push(match cands.as_slice() {
            // (Without recorded colliders a unit is its own identity.)
            [h] => h.id.or_else(|| i32::try_from(x.0).ok().map(|n| -1 - n)),
            [] => None,
            _ => {
                ambiguous.push(x);
                None
            }
        });
    }
    (keys, ambiguous)
}

/// A `bounds_init` root as the machine takes it.
fn link_pos(life: &Lifecycle, c: i32) -> Option<V3> {
    life.link_pos(c)
}

/// A `bounds_init` root as the machine takes it.
fn root_event(root: &Root) -> Ev<'_> {
    Ev::Root {
        id: root.id,
        kind: if root.kind.as_deref() == Some(UNIT_KIND) { Kind::Unit } else { Kind::Other },
        kind_name: root.kind.as_deref().unwrap_or(""),
        entity: root.e.as_deref(),
        on: root.on.unwrap_or(false),
        radius: root.r.unwrap_or(0.0),
        pos: root.pos,
    }
}

/// A `bounds_tick` as the machine takes it.
fn tick_event(t: &TickRaw) -> Ev<'_> {
    Ev::Tick {
        entity: &t.e,
        kind: if t.kind == UNIT_KIND { Kind::Unit } else { Kind::Other },
        kind_name: &t.kind,
        made: t.made,
        c: t.c,
        r0: t.r0,
        r: t.r,
        pos: t.pos,
        on: t.on,
    }
}

/// A stream record as the machine takes it.
fn stream_event(s: &Stream) -> Ev<'_> {
    match s {
        Stream::Sync => Ev::Sync,
        Stream::Set { f, v, c, on0, on, pos0, pos, e0, e } => Ev::Set(SetRec {
            f: *f,
            v: v.as_deref(),
            c: *c,
            on0: *on0,
            on: *on,
            pos0: *pos0,
            pos: *pos,
            e0: e0.as_deref(),
            e: e.as_deref(),
        }),
    }
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
    /// from `bounds_init`. Each frame is replayed whole, its `bounds_tick` calls and the queued
    /// writes in the stream's order; a frame the sim didn't follow has no step known and no
    /// units to check, and the writes that came after an earlier frame's query open the next
    /// frame's call.
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
        let mut frames: BTreeMap<i64, (Vec<TickRaw>, Vec<Queued>)> = BTreeMap::new();
        let due: Vec<i64> = rec.ticks.range(..=tick).map(|(t, _)| *t).collect();
        for t in due {
            frames.entry(t).or_default().0 = rec.ticks.remove(&t).unwrap_or_default();
        }
        for q in std::mem::take(&mut rec.queue) {
            frames.entry(q.t).or_default().1.push(q);
        }
        frames.entry(tick).or_default();
        let mut carry: Vec<Event> = Vec::new();
        let mut sync_now = false;
        for (f, (raws, queued)) in frames {
            let current = f == tick;
            let (late_q, early_q): (Vec<Queued>, Vec<Queued>) =
                queued.into_iter().partition(|q| q.late);
            if !current && raws.is_empty() && early_q.is_empty() {
                // A frame whose phase ran: its late writes open the next call.
                for q in late_q {
                    carry.extend(self.bounds_rec_write(w, &mut rec, &q, true));
                }
                continue;
            }
            // The previous frame has ended: what was destroyed is gone.
            self.bounds_rec_frame(w, &mut rec);
            // The calls and the writes in the stream's order.
            let mut items: Vec<(i64, Result<TickRaw, Queued>)> = raws
                .into_iter()
                .map(|t| (t.seq, Ok(t)))
                .chain(early_q.into_iter().map(|q| (q.seq, Err(q))))
                .collect();
            items.sort_by_key(|(seq, _)| *seq);
            let mut ticks: Vec<RecTick> = Vec::new();
            let mut events: Vec<Event> = std::mem::take(&mut carry);
            for (_, item) in items {
                match item {
                    Ok(raw) => {
                        self.vis.consume_record(raw.seq);
                        ticks.extend(self.bounds_rec_unit(w, &mut rec, &raw));
                    }
                    Err(q) => events.extend(self.bounds_rec_write(w, &mut rec, &q, false)),
                }
            }
            sync_now |= events.iter().any(|e| e.write == Write::Sync);
            let (step, units) = if current { (step, list) } else { (Step::Unknown, &[][..]) };
            let out = w.combat.vision.bounds.tick_recorded(step, &events, &ticks, units);
            self.bounds_rec_out(w, out);
            for q in late_q {
                carry.extend(self.bounds_rec_write(w, &mut rec, &q, true));
            }
        }
        // (What came after this frame's own phase isn't queued yet.)
        debug_assert!(carry.is_empty());
        let c = &mut self.vis.counts.rec;
        let flagged = std::mem::take(&mut w.combat.vision.explicit_sync);
        c.sync_frames += usize::from(sync_now);
        c.sync_flagged += usize::from(sync_now && flagged);
        c.sync_unflagged += usize::from(sync_now && !flagged);
        c.sync_flag_only += usize::from(flagged && !sync_now);
        c.views_replaced += std::mem::take(&mut w.combat.vision.views_replaced).len();
        self.vis.range.rec = Some(rec);
    }

    /// Lets the machine take a record; what it finds is reported. `true` when it found nothing.
    fn bounds_rec_life(&mut self, w: &mut World, rec: &mut BoundsRec, ev: Ev<'_>) -> bool {
        let mut problems = Vec::new();
        rec.life.apply(ev, &mut problems);
        for p in &problems {
            Self::vis_report(w, &format!("bounds records: {p}"));
        }
        problems.is_empty()
    }

    /// A frame's end for the machine.
    fn bounds_rec_frame(&mut self, w: &mut World, rec: &mut BoundsRec) {
        let mut problems = Vec::new();
        rec.life.frame(&mut problems);
        for p in &problems {
            Self::vis_report(w, &format!("bounds records: {p}"));
        }
    }

    /// A queued stream record: checked by the machine at its place, then (for a unit's collider)
    /// the write the colliders take. `late`: it came after its frame's phase.
    fn bounds_rec_write(
        &mut self,
        w: &mut World,
        rec: &mut BoundsRec,
        q: &Queued,
        late: bool,
    ) -> Option<Event> {
        let Stream::Set { f, c, on0, on, pos0, pos, .. } = &q.s else {
            self.bounds_rec_life(w, rec, stream_event(&q.s));
            return Some(Event { seq: q.seq, write: Write::Sync, late });
        };
        let ok = self.bounds_rec_life(w, rec, stream_event(&q.s));
        if matches!(f, SetFn::Link | SetFn::Entity) {
            self.vis.counts.rec.links += 1;
            return None;
        }
        let c = (*c)?;
        match rec.life.kind_of(c) {
            Some(Kind::Unit) if ok => {
                let write = match f {
                    SetFn::Active => Write::Active { id: c, on0: (*on0)?, on: (*on)? },
                    SetFn::Pos => Write::Pos { id: c, pos0: (*pos0)?, pos: (*pos)? },
                    _ => Write::Gone { id: c },
                };
                Some(Event { seq: q.seq, write, late })
            }
            Some(Kind::Other) => {
                self.vis.counts.rec.other_writes += 1;
                None
            }
            _ => None,
        }
    }

    /// The run's end: the writes queued after the last frame's phase have no controller tick
    /// after them; they are checked against the colliders as they stand.
    pub(super) fn bounds_rec_finish(&mut self, w: &mut World) {
        let Some(mut rec) = self.vis.range.rec.take() else { return };
        let mut queued = std::mem::take(&mut rec.queue);
        queued.sort_by_key(|q| q.seq);
        let mut late = Vec::new();
        for q in &queued {
            late.extend(self.bounds_rec_write(w, &mut rec, q, true));
        }
        // (The last frame ends: a collider linked and never made is reported.)
        self.bounds_rec_frame(w, &mut rec);
        let out = w.combat.vision.bounds.finish_recorded(&late);
        self.bounds_rec_out(w, out);
        self.vis.counts.rec.colliders = rec.life.seen();
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
        let kind = if t.kind == UNIT_KIND { Kind::Unit } else { Kind::Other };
        let ok = self.bounds_rec_life(w, rec, tick_event(t));
        if kind == Kind::Other {
            self.vis.counts.rec.other_ticks += 1;
            return None;
        }
        let Some(&u) = w.by_id.get(&t.e).filter(|u| w.unit(**u).is_some()) else {
            Self::vis_report(w, &format!("a bounds_tick of {}, a unit the sim doesn't have", t.e));
            return None;
        };
        ok.then_some(RecTick {
            seq: t.seq,
            unit: u,
            made: t.made,
            r0: t.r0,
            r: t.r,
            pos: t.pos,
            on: t.on,
            id: t.c,
            link_pos: t.c.and_then(|c| link_pos(&rec.life, c)),
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
        for root in rec.roots.clone() {
            self.vis.consume_record(root.seq);
            let kind =
                if root.kind.as_deref() == Some(UNIT_KIND) { Kind::Unit } else { Kind::Other };
            ok &= self.bounds_rec_life(w, rec, root_event(&root));
            if kind == Kind::Other {
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
        self.vis.range.order_last.insert(u, units);
    }

    /// The constraints the observer's effective order puts on the colliders, every frame (a
    /// cached order is the order of the frame's set too, and a collider may have been destroyed
    /// and made again since it was recorded): each unit stands for the one collider of it the
    /// query may have hit. A unit with several possible ones can't be told from the record; it is
    /// reported (named, counted), not dropped.
    pub(super) fn vis_order_constrain(&mut self, w: &mut World, u: EntityIdx) {
        let Some(order) = self.vis.range.order_last.get(&u).cloned() else { return };
        let Some(hits) = self.vis.range.expect.get(&u) else { return };
        let (keys, ambiguous) = order_keys(&order, hits);
        if self.vis.range.order_keys.get(&u) == Some(&keys) {
            return;
        }
        self.vis.range.order_keys.insert(u, keys.clone());
        let id = w.entity(u).unique_id.clone();
        for x in ambiguous {
            let name = w.entity(x).unique_id.clone();
            self.vis.counts.rec.order_ambiguous += 1;
            Self::vis_report(
                w,
                &format!(
                    "vis_order: {name} has several colliders that may be hit in {id}'s query, so its place in the recorded order can't be tied to one (not validated)"
                ),
            );
        }
        let keyed: Vec<(EntityIdx, i32)> =
            order.iter().zip(&keys).filter_map(|(x, k)| Some((*x, (*k)?))).collect();
        let ids: Vec<i32> = keyed.iter().map(|(_, k)| *k).collect();
        if let Some((a, b)) = self.vis.range.order_pairs.add(&ids) {
            let name = |k: i32| {
                keyed.iter().find(|(_, x)| *x == k).map(|(u, _)| w.entity(*u).unique_id.clone())
            };
            let (a, b) = (name(a).unwrap_or_default(), name(b).unwrap_or_default());
            Self::vis_report(
                w,
                &format!(
                    "vis_order: the order of {id}'s set has {a} before {b}, which the colliders' static order contradicts"
                ),
            );
        }
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
                "a set of a function the oracle does not write",
                vec![
                    init(0, 0, -8, "a"),
                    end(1, 1),
                    rv(json!({"seq": 2, "k": "obs", "t": 5, "ev": "bounds_set", "fn": "teleport",
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
            p.problems.iter().any(|x| x.contains("after the frame's vis or vis_order")),
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

    #[test]
    fn the_orders_of_a_frame_stand_where_the_oracle_writes_them() {
        let order = |seq: i64, t: i64, u: &str| {
            rv(json!({"seq": seq, "k": "obs", "t": t, "ev": "vis_order", "u": u, "v": [u]}))
        };
        let vis = |seq: i64, t: i64, u: &str| {
            rv(json!({"seq": seq, "k": "obs", "t": t, "ev": "vis", "u": u, "add": [u], "rem": []}))
        };
        // A unit's order just before its `vis`: fine; after it: refused (the oracle writes the
        // order first), the other units' being no matter.
        let ok = framed(vec![order(0, 3, "a"), vis(1, 3, "a"), order(2, 3, "b")]);
        assert!(parse(&ok, &["vis_order"]).problems.is_empty());
        let moved = framed(vec![vis(0, 3, "a"), order(1, 3, "a")]);
        let p = parse(&moved, &["vis_order"]);
        assert!(
            p.problems.iter().any(|x| x.contains("after its unit's vis record")),
            "{:?}",
            p.problems
        );
        // The next frame's `vis` of the unit doesn't count.
        let next = framed(vec![vis(0, 3, "a"), order(1, 4, "a")]);
        assert!(parse(&next, &["vis_order"]).problems.is_empty());
        // In the window's first frame the initial block comes first, with no calls in the
        // frame to show it.
        let block = |recs: Vec<Rv>| {
            let p = parse(&framed(recs), &ALL);
            (p.rec.is_some(), p.problems)
        };
        let (ok, problems) = block(vec![init(0, 0, -8, "a"), end(1, 1), order(2, 0, "a")]);
        assert!(ok && problems.is_empty(), "{problems:?}");
        let (ok, problems) = block(vec![order(0, 0, "a"), init(1, 0, -8, "a"), end(2, 1)]);
        assert!(ok, "(a refused order doesn't refuse the family)");
        assert!(
            problems.iter().any(|x| x.contains("before the bounds_init block")),
            "{problems:?}"
        );
    }

    #[test]
    fn the_order_of_the_colliders_is_one_static_order_across_records() {
        let mut g = OrderGraph::default();
        assert_eq!(g.add(&[1, 2]), None);
        assert_eq!(g.add(&[2, 3]), None);
        // Sol's round 4: [1,2], [2,3], [3,1] has no static order; neither has the reverse pair.
        assert_eq!(g.add(&[3, 1]), Some((3, 1)));
        assert_eq!(g.add(&[2, 1]), Some((2, 1)));
        // The pairs implied by earlier records stay: [3,2] reverses 2 < 3.
        assert_eq!(g.add(&[3, 2]), Some((3, 2)));
        // A collider destroyed and recreated is another identity (another instance id): the
        // old one's place says nothing of the new one's.
        assert_eq!(g.add(&[2, 9]), None);
        assert_eq!(g.add(&[9, 2]), Some((9, 2)));
        // Colliders no record ordered yet are free, and a record keeps nothing it contradicts.
        assert_eq!(g.add(&[4, 1]), None);
        assert_eq!(g.add(&[1, 4]), Some((1, 4)));
    }

    fn hit(unit: u32, id: i32, tri: Tri) -> Hit {
        Hit {
            unit: EntityIdx(unit),
            tri,
            cause: None,
            margin: None,
            rounding: false,
            guess: None,
            order_known: true,
            capped: None,
            extra_slots: 0,
            lost: false,
            id: Some(id),
        }
    }

    #[test]
    fn an_order_is_keyed_by_the_colliders_the_query_may_have_hit() {
        // Sol's round 5: a unit's old collider (destroyed this frame, still a sure hit) and
        // its new one (out of reach) make the entry the old one's, so a reversal against the
        // old one's earlier place is found; both possible: reported, not dropped.
        let (a, b) = (EntityIdx(1), EntityIdx(2));
        let hits = [hit(1, 7, Tri::In), hit(2, 8, Tri::In), hit(1, 9, Tri::Out)];
        let (keys, amb) = order_keys(&[a, b], &hits);
        assert_eq!((keys, amb.is_empty()), (vec![Some(7), Some(8)], true));
        let mut g = OrderGraph::default();
        assert_eq!(g.add(&[7, 8]), None);
        let (keys, _) = order_keys(&[b, a], &hits);
        let ids: Vec<i32> = keys.into_iter().flatten().collect();
        assert_eq!(g.add(&ids), Some((8, 7)));
        let both = [hit(1, 7, Tri::In), hit(2, 8, Tri::In), hit(1, 9, Tri::Maybe)];
        let (keys, amb) = order_keys(&[b, a], &both);
        assert_eq!((keys, amb), (vec![Some(8), None], vec![a]));
        // A cached order is keyed again when the collider changes: the new one's place is bound
        // by the same order, and another observer's reverse order for it is found.
        let after = [hit(1, 9, Tri::In), hit(2, 8, Tri::In)];
        let (keys, _) = order_keys(&[a, b], &after);
        let ids: Vec<i32> = keys.into_iter().flatten().collect();
        let mut g = OrderGraph::default();
        assert_eq!(g.add(&ids), None);
        let other = [hit(1, 9, Tri::In), hit(2, 8, Tri::In)];
        let (keys, _) = order_keys(&[b, a], &other);
        let ids: Vec<i32> = keys.into_iter().flatten().collect();
        assert_eq!(g.add(&ids), Some((8, 9)));
    }
}

#[cfg(test)]
mod table;
