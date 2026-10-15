//! The controller's tick against WrathOracle's `memory` observer (docs/poc5/memory.md, H1): every
//! recorded `UnitMemoryController.Tick` rerun from the recorded state with its recorded reads, the
//! memory compared after each group's pass and at the tick's end, the memory's recorded additions
//! and removals compared with the model's; the queue's additions (`AddToMemory`) checked as they
//! come; one tick in every Default-mode frame.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::combat::oracle::Trace;

/// `GameModeType.Default`: the only mode `UnitMemoryController` is registered in.
const DEFAULT_MODE: i64 = 1;

/// A failure to follow the recording: the tick's check stops there.
#[derive(Debug)]
struct Desync(String);

impl From<MemError> for Desync {
    fn from(e: MemError) -> Self {
        Desync(format!("the model threw {e:?}"))
    }
}

/// The check's state: the recording's records, a cursor, and what the records establish.
struct Check<'a> {
    recs: &'a [Value],
    i: usize,
    ids: HashMap<String, u32>,
    names: Vec<String>,
    /// Each group's memory as last recorded (`mem_init`, `mem_s`).
    recorded: HashMap<u32, MemoryLayout<u32>>,
    /// The controller's queue as the model keeps it (`mem_initq`, then each `mem_q`); `None`
    /// until `mem_initq` and after a desync, until the next tick's entry says what it holds.
    queue: Option<Vec<(u32, u32)>>,
    /// `s_UnitsForRemove` as the model keeps it (`None` as the queue).
    pending: Option<Vec<u32>>,
    /// This run's identity hashes (`hash`, on first use).
    hashes: HashMap<u32, i32>,
    /// The record run's hashes (a replay's taped values are checked against them).
    tape: HashMap<String, i64>,
    /// A replay run (its `hash` records say whether the value was the tape's).
    replay: bool,
    /// A replay's pinned `Time.frameCount` is its own plus this.
    frame_shift: i64,
    /// The frame being run: its `t`, its pinned `Time.frameCount`, its game mode, the ticks it
    /// ran.
    t: i64,
    frame: Option<i64>,
    mode: Option<i64>,
    frame_ticks: usize,
    /// `mem_initq` records seen: exactly one, before the first tick.
    initq: usize,
    started: bool,
    errors: Vec<String>,
    counts: BTreeMap<&'static str, usize>,
}

impl<'a> Check<'a> {
    fn new(recs: &'a [Value], tape: HashMap<String, i64>, frame_shift: i64, replay: bool) -> Self {
        Self {
            recs,
            i: 0,
            ids: HashMap::new(),
            names: Vec::new(),
            recorded: HashMap::new(),
            queue: None,
            pending: None,
            hashes: HashMap::new(),
            tape,
            replay,
            frame_shift,
            t: -1,
            frame: None,
            mode: None,
            frame_ticks: 0,
            initq: 0,
            started: false,
            errors: Vec::new(),
            counts: BTreeMap::new(),
        }
    }

    fn id(&mut self, s: &str) -> u32 {
        if let Some(&i) = self.ids.get(s) {
            return i;
        }
        let i = u32::try_from(self.names.len()).expect("fewer than 2^32 names");
        self.ids.insert(s.to_owned(), i);
        self.names.push(s.to_owned());
        i
    }

    fn name(&self, i: u32) -> &str {
        &self.names[i as usize]
    }

    fn names_of(&self, v: &[u32]) -> Vec<String> {
        v.iter().map(|&k| self.name(k).to_owned()).collect()
    }

    fn fail(&mut self, what: impl Into<String>) {
        let t = self.t;
        self.errors.push(format!("t={t}: {}", what.into()));
    }

    fn count(&mut self, k: &'static str, n: usize) {
        *self.counts.entry(k).or_default() += n;
    }

    /// A string field as an interned id.
    fn id_of(&mut self, v: &Value, what: &str) -> Result<u32, Desync> {
        let s = v.as_str().ok_or_else(|| Desync(format!("{what}: not an id ({v})")))?;
        Ok(self.id(s))
    }

    fn ids_of(&mut self, v: &Value, what: &str) -> Result<Vec<u32>, Desync> {
        let a = v.as_array().ok_or_else(|| Desync(format!("{what}: not a list ({v})")))?;
        a.iter().map(|x| self.id_of(x, what)).collect()
    }

    fn pairs_of(&mut self, v: &Value) -> Result<Vec<(u32, u32)>, Desync> {
        let a = v.as_array().ok_or_else(|| Desync(format!("a queue that isn't a list ({v})")))?;
        a.iter()
            .map(|p| match p.as_array().map(Vec::as_slice) {
                Some([o, t]) => {
                    Ok((self.id_of(o, "a queued observer")?, self.id_of(t, "a queued target")?))
                }
                _ => Err(Desync(format!("a queued pair that isn't one ({p})"))),
            })
            .collect()
    }

    /// A memory record's layout (`mem_init`, `mem_s`): its group and the memory, checked as
    /// recorded to be a dictionary's, then as the model holds it (the free list's head -1 while
    /// no slot is free).
    fn layout_of(&mut self, r: &Value) -> Result<(u32, MemoryLayout<u32>), Desync> {
        let g = self.id_of(&r["g"], "a memory's group")?;
        let mut slots = Vec::new();
        for s in r["slots"].as_array().ok_or_else(|| Desync(format!("slots: {}", r["slots"])))? {
            match s.as_array().map(Vec::as_slice) {
                Some([Value::Null, next]) => {
                    let next = next.as_i64().and_then(|n| i32::try_from(n).ok());
                    slots.push(RawSlot::Free(
                        next.ok_or_else(|| Desync(format!("a free slot {s}")))?,
                    ));
                }
                Some([k, ticks, vis]) => {
                    let k = self.id_of(k, "a slot's key")?;
                    let (Some(ticks), Some(visible)) = (ticks.as_i64(), vis.as_bool()) else {
                        return Err(Desync(format!(
                            "a slot without its detect time or visibility ({s})"
                        )));
                    };
                    slots.push(RawSlot::Used(
                        k,
                        UnitInfo { last_detect_time: TimeSpan(ticks), visible },
                    ));
                }
                _ => return Err(Desync(format!("a slot that isn't one ({s})"))),
            }
        }
        let free = r["free"].as_i64().and_then(|n| i32::try_from(n).ok());
        let nfree = r["nfree"].as_u64().and_then(|n| usize::try_from(n).ok());
        let (Some(free), Some(nfree)) = (free, nfree) else {
            return Err(Desync("a memory without its free list".into()));
        };
        let cache = |c: &mut Self, v: &Value, what: &str| -> Result<Option<Vec<u32>>, Desync> {
            if v.is_null() { Ok(None) } else { c.ids_of(v, what).map(Some) }
        };
        let raw = MemoryLayout {
            slots,
            free_list: free,
            free_count: nfree,
            units_list: cache(self, &r["list"], "a cached UnitsList")?,
            enemies: cache(self, &r["enemies"], "a cached Enemies")?,
        };
        let Some(m) = GroupMemory::from_layout(raw) else {
            return Err(Desync(format!("{}'s memory isn't a dictionary's layout", self.name(g))));
        };
        Ok((g, m.layout()))
    }

    /// A layout as text, for reports.
    fn show(&self, l: &MemoryLayout<u32>) -> String {
        let slots: Vec<String> = l
            .slots
            .iter()
            .map(|s| match s {
                RawSlot::Used(k, i) => format!(
                    "{}@{}{}",
                    self.name(*k),
                    i.last_detect_time.0,
                    if i.visible { "v" } else { "" }
                ),
                RawSlot::Free(n) => format!("~{n}"),
            })
            .collect();
        let ids = |v: &Option<Vec<u32>>| v.as_ref().map(|v| self.names_of(v));
        format!(
            "[{}] free {}/{} list {:?} enemies {:?}",
            slots.join(" "),
            l.free_list,
            l.free_count,
            ids(&l.units_list),
            ids(&l.enemies)
        )
    }

    /// `hash` (a unit's identity hash, once a window, on first use): this run's value, and in a
    /// replay whether it was the tape's (then the record run's).
    fn see_hash(&mut self, r: &Value) {
        let shape: &[&str] = if self.replay { &["u", "h", "tape"] } else { &["u", "h"] };
        let (true, Some(u), Some(h)) = (has_fields(r, shape), r["u"].as_str(), r["h"].as_i64())
        else {
            self.fail(format!("a hash record it can't read ({r})"));
            return;
        };
        let Ok(h32) = i32::try_from(h) else {
            self.fail(format!("a hash out of range ({r})"));
            return;
        };
        // A replay takes a unit's hash from the tape when the record run recorded one.
        if self.replay {
            let taped = self.tape.get(u);
            if r["tape"].as_bool() != Some(taped.is_some()) || taped.is_some_and(|&t| t != h) {
                self.fail(format!(
                    "{u}'s hash {h} (tape {}) against the record run's {taped:?}",
                    r["tape"]
                ));
            }
        }
        let k = self.id(u);
        if self.hashes.insert(k, h32).is_some() {
            self.fail(format!("{u}'s hash recorded twice"));
        }
    }
}

/// Exactly these fields besides the envelope (`seq`, `k`, `t`, `ev`, and `f` for a leaf).
fn has_fields(r: &Value, payload: &[&str]) -> bool {
    let base: &[&str] =
        if r["ev"] == "mem_v" { &["seq", "k", "t", "ev", "f"] } else { &["seq", "k", "t", "ev"] };
    r.as_object().is_some_and(|o| {
        o.len() == base.len() + payload.len()
            && base.iter().chain(payload).all(|k| o.contains_key(*k))
    })
}

const ROW: [&str; 7] = ["at", "g", "slots", "free", "nfree", "list", "enemies"];
const INIT: [&str; 6] = ["g", "slots", "free", "nfree", "list", "enemies"];

fn obs<'v>(r: &'v Value, ev: &str) -> Option<&'v Value> {
    (r["k"] == "obs" && r["ev"] == ev).then_some(r)
}

/// A record the tick asks for (a leaf or a state): never passed on the way.
fn asked(r: &Value) -> bool {
    r["k"] == "obs" && matches!(r["ev"].as_str(), Some("mem_v" | "mem_s"))
}

/// A record that ends or starts a tick, a frame or the window's state: never consumed by a tick
/// that didn't expect it.
fn structural(r: &Value) -> bool {
    r["k"] != "obs"
        || matches!(
            r["ev"].as_str(),
            Some("mem_tick" | "mem_tick_end" | "mem_init" | "mem_initq" | "mem_q")
        )
}

/// The memory's own records of what changed (`mem_add` of a new key, `mem_remove` by `Remove`),
/// in order: (group, change).
type Changes = Vec<(u32, Change<u32>)>;

/// The tick's world: its reads pulled strictly as the next records.
struct TickWorld<'c, 'a> {
    c: &'c mut Check<'a>,
    /// The memories the tick touched, from their recorded state.
    mem: HashMap<u32, GroupMemory<u32>>,
    now: TimeSpan,
    frame: i32,
    /// `Cleanup`'s removals (`mem_remove`, by `Cleanup`) seen on the way: (group, unit).
    removals: Vec<(u32, u32)>,
    /// The memory's recorded changes since the last comparison, in order.
    changes: Changes,
    /// The model's, in the order it made them (collected from the memories it touched before each
    /// read: a step changes one memory at most between two reads).
    model_changes: Changes,
}

impl<'a> TickWorld<'_, 'a> {
    /// The model's changes since the last read, appended in order.
    fn sync(&mut self) -> Result<(), Desync> {
        let mut changed = Vec::new();
        for (&g, m) in &mut self.mem {
            let c = m.take_changes();
            if !c.is_empty() {
                changed.push((g, c));
            }
        }
        if changed.len() > 1 {
            return Err(Desync("the model changed two memories between two reads".into()));
        }
        for (g, c) in changed {
            self.model_changes.extend(c.into_iter().map(|x| (g, x)));
        }
        Ok(())
    }

    /// The recorded changes so far equal the model's, in order.
    fn compare_changes(&mut self) -> Result<(), Desync> {
        self.sync()?;
        let (rec, model) =
            (std::mem::take(&mut self.changes), std::mem::take(&mut self.model_changes));
        check_changes(self.c, &rec, &model);
        Ok(())
    }

    /// A record passed on the way to the next one the tick asks for.
    fn on_the_way(&mut self, r: &Value) -> Result<(), Desync> {
        if r["t"].as_i64() != Some(self.c.t) {
            return Err(Desync(format!("a record of another frame inside the tick ({r})")));
        }
        // `mem_add` and `mem_remove`: exactly their writers' fields.
        let pair = |w: &mut Self| -> Result<(u32, u32), Desync> {
            if !has_fields(r, &["group", "u", "by"]) || !r["by"].is_string() {
                return Err(Desync(format!("a memory change of another shape ({r})")));
            }
            Ok((w.c.id_of(&r["group"], "a memory's group")?, w.c.id_of(&r["u"], "a unit")?))
        };
        match r["ev"].as_str() {
            Some("hash") => self.c.see_hash(r),
            Some("mem_remove") if r["by"] == "Cleanup" => {
                let p = pair(self)?;
                self.removals.push(p);
            }
            Some("mem_remove") if r["by"] == "Remove" => {
                let (g, u) = pair(self)?;
                self.changes.push((g, Change::Removed(u)));
            }
            Some("mem_remove") => return Err(Desync(format!("a removal it can't read ({r})"))),
            Some("mem_add") => {
                let (g, u) = pair(self)?;
                self.changes.push((g, Change::Added(u)));
            }
            Some("mem_clear") => {
                return Err(Desync(format!("a memory cleared inside the tick ({r})")));
            }
            _ => {}
        }
        Ok(())
    }

    /// Up to the next record the tick asks for, or a structural one.
    fn drain(&mut self) -> Result<(), Desync> {
        let recs = self.c.recs;
        while let Some(r) = recs.get(self.c.i) {
            if asked(r) || structural(r) {
                return Ok(());
            }
            self.on_the_way(r)?;
            self.c.i += 1;
        }
        Ok(())
    }

    /// The next record the tick asks for (a structural one is left in place).
    fn next(&mut self, what: &str) -> Result<&'a Value, Desync> {
        self.sync()?;
        self.drain()?;
        let recs: &'a [Value] = self.c.recs;
        let r = recs.get(self.c.i).ok_or_else(|| Desync(format!("{what}: the recording ends")))?;
        if structural(r) || r["t"].as_i64() != Some(self.c.t) {
            return Err(Desync(format!("{what}: the recording has {}", describe(r))));
        }
        self.c.i += 1;
        Ok(r)
    }

    /// The next leaf, of `f`, with exactly its writer's fields.
    fn leaf(&mut self, f: &str, payload: &[&str]) -> Result<&'a Value, Desync> {
        let r = self.next(&format!("the model reads {f}"))?;
        if obs(r, "mem_v").is_none() || r["f"] != f {
            // (Left for the resynchronisation, which takes a state as recorded.)
            self.c.i -= 1;
            return Err(Desync(format!("the model reads {f}, the recording has {}", describe(r))));
        }
        if !has_fields(r, payload) {
            return Err(Desync(format!("a leaf {f} with fields its writer doesn't write ({r})")));
        }
        Ok(r)
    }

    /// The leaf is about the unit (or group) the model reads about.
    fn about(&self, r: &Value, k: &str, want: u32) -> Result<(), Desync> {
        if r[k].as_str() == Some(self.c.name(want)) {
            return Ok(());
        }
        Err(Desync(format!(
            "the model reads {} of {k} {}, the recording of {}",
            r["f"],
            self.c.name(want),
            r[k]
        )))
    }

    fn real(&self, g: u32) -> Result<(), Desync> {
        if self.c.name(g).starts_with('~') {
            return Err(Desync(format!("a fake or disposed group's memory ({})", self.c.name(g))));
        }
        Ok(())
    }
}

/// The memory's recorded additions (new keys) and removals by `Remove`, in order across every
/// memory, are the model's.
fn check_changes(c: &mut Check<'_>, recorded: &Changes, model: &Changes) {
    if recorded != model {
        let show = |v: &Changes| -> Vec<String> {
            v.iter()
                .map(|&(g, x)| match x {
                    Change::Added(k) => format!("{}+{}", c.name(g), c.name(k)),
                    Change::Removed(k) => format!("{}-{}", c.name(g), c.name(k)),
                })
                .collect()
        };
        let (a, b) = (show(model), show(recorded));
        c.fail(format!("the memories' changes: model {a:?}, game {b:?}"));
    }
    let adds = recorded.iter().filter(|x| matches!(x.1, Change::Added(_))).count();
    c.count("additions recorded", adds);
    c.count("removals recorded", recorded.len() - adds);
}

fn bool_of(r: &Value, k: &str) -> Result<bool, Desync> {
    r[k].as_bool().ok_or_else(|| Desync(format!("{k} isn't a bool in {r}")))
}

/// A float written as C#'s round-trip string.
fn float_of(r: &Value, k: &str) -> Result<f32, Desync> {
    r[k].as_str()
        .and_then(|s| s.parse::<f32>().ok())
        .ok_or_else(|| Desync(format!("{k} isn't a float in {r}")))
}

impl MemoryWorld<u32, u32> for TickWorld<'_, '_> {
    type Error = Desync;

    fn area_peaceful(&mut self) -> Result<bool, Desync> {
        bool_of(self.leaf("apeace", &["v"])?, "v")
    }

    fn ready_groups(&mut self) -> Result<Vec<u32>, Desync> {
        let r = self.leaf("ready", &["v"])?;
        let gs = self.c.ids_of(&r["v"], "a ready group")?;
        for &g in &gs {
            self.real(g)?;
        }
        Ok(gs)
    }

    fn near_units(&mut self, g: u32) -> Result<Vec<u32>, Desync> {
        let r = self.leaf("near", &["g", "v"])?;
        self.about(r, "g", g)?;
        let us = self.c.ids_of(&r["v"], "a near unit")?;
        self.c.count("near units", us.len());
        Ok(us)
    }

    fn faction_peaceful(&mut self, u: u32) -> Result<bool, Desync> {
        let r = self.leaf("peace", &["u", "v"])?;
        self.about(r, "u", u)?;
        bool_of(r, "v")
    }

    fn passive(&mut self, u: u32) -> Result<bool, Desync> {
        let r = self.leaf("pass", &["u", "v"])?;
        self.about(r, "u", u)?;
        bool_of(r, "v")
    }

    fn group_of(&mut self, u: u32) -> Result<u32, Desync> {
        let r = self.leaf("grp", &["u", "v"])?;
        self.about(r, "u", u)?;
        self.c.id_of(&r["v"], "a unit's group")
    }

    fn is_fake(&mut self, g: u32) -> bool {
        self.c.name(g) == "~fake"
    }

    fn group_by_id(&mut self, _u: u32) -> Result<u32, Desync> {
        let r = self.leaf("getgrp", &["id", "v"])?;
        // `GetGroup(id)` returns the live group with that id (made if none).
        if r["id"].as_str().is_none_or(|id| id.starts_with('~')) || r["v"] != r["id"] {
            return Err(Desync(format!("GetGroup returned another group ({r})")));
        }
        self.c.id_of(&r["v"], "a group by id")
    }

    fn in_stealth_for(&mut self, u: u32, g: u32) -> Result<bool, Desync> {
        let r = self.leaf("stl", &["u", "g", "v"])?;
        self.about(r, "u", u)?;
        self.about(r, "g", g)?;
        bool_of(r, "v")
    }

    fn is_enemy(&mut self, g: u32, u: u32) -> Result<bool, Desync> {
        let r = self.leaf("enemy", &["g", "u", "v"])?;
        self.about(r, "g", g)?;
        self.about(r, "u", u)?;
        let v = bool_of(r, "v")?;
        self.c.count(if v { "looked at: enemies" } else { "looked at: friends" }, 1);
        Ok(v)
    }

    fn game_delta_time(&mut self) -> Result<f32, Desync> {
        float_of(self.leaf("gdt", &["v"])?, "v")
    }

    fn frame_count(&mut self) -> Result<i32, Desync> {
        Ok(self.frame)
    }

    fn unit_hash(&mut self, u: u32) -> Result<i32, Desync> {
        // A first use records the hash as it's read.
        self.drain()?;
        self.c.count("friend phase computed", 1);
        self.c.hashes.get(&u).copied().ok_or_else(|| {
            Desync(format!("{}'s identity hash read, none recorded", self.c.name(u)))
        })
    }

    fn group_has_los(&mut self, g: u32, u: u32) -> Result<bool, Desync> {
        let r = self.leaf("los", &["g", "u", "v"])?;
        self.about(r, "g", g)?;
        self.about(r, "u", u)?;
        let v = bool_of(r, "v")?;
        self.c.count(if v { "line of sight" } else { "no line of sight" }, 1);
        Ok(v)
    }

    fn invisible(&mut self, u: u32) -> Result<bool, Desync> {
        let r = self.leaf("inv", &["u", "c", "v"])?;
        self.about(r, "u", u)?;
        if r["c"].as_i64() != Some(i64::from(INVISIBLE)) {
            return Err(Desync(format!("a condition other than Invisible ({r})")));
        }
        bool_of(r, "v")
    }

    fn sees_invisible(&mut self, _g: u32, u: u32) -> Result<bool, Desync> {
        let r = self.leaf("seen", &["u", "v"])?;
        self.about(r, "u", u)?;
        self.c.count("invisible: seen taken from the recording", 1);
        bool_of(r, "v")
    }

    fn game_time(&mut self) -> Result<TimeSpan, Desync> {
        Ok(self.now)
    }

    fn memory(&mut self, g: u32) -> Result<&mut GroupMemory<u32>, Desync> {
        self.real(g)?;
        if !self.mem.contains_key(&g) {
            let mut m = match self.c.recorded.get(&g) {
                Some(l) => GroupMemory::from_layout(l.clone()).expect("checked when recorded"),
                None => GroupMemory::new(),
            };
            m.log_changes();
            self.mem.insert(g, m);
        }
        Ok(self.mem.get_mut(&g).expect("just inserted"))
    }

    fn resolves(&mut self, u: u32) -> Result<bool, Desync> {
        let r = self.leaf("unit", &["u", "v"])?;
        self.about(r, "u", u)?;
        match &r["v"] {
            Value::Null => Ok(false),
            v if v.as_str() == Some(self.c.name(u)) => Ok(true),
            _ => Err(Desync(format!("a reference resolved to another unit ({r})"))),
        }
    }

    fn in_game(&mut self, u: u32) -> Result<bool, Desync> {
        let r = self.leaf("ingame", &["u", "v"])?;
        self.about(r, "u", u)?;
        bool_of(r, "v")
    }

    fn disposed(&mut self, u: u32) -> Result<bool, Desync> {
        let r = self.leaf("disp", &["u", "v"])?;
        self.about(r, "u", u)?;
        bool_of(r, "v")
    }

    fn wake(&mut self, u: u32) -> Result<(), Desync> {
        let r = self.leaf("wake", &["u", "time", "fade", "b", "a"])?;
        self.about(r, "u", u)?;
        if r["time"] != "0" || r["fade"] != false {
            return Err(Desync(format!("Wake with arguments the controller doesn't pass ({r})")));
        }
        let (b, a) = (float_of(r, "b")?, float_of(r, "a")?);
        // `AwakeTimer = Mathf.Max(0f, AwakeTimer)` (`a > b ? a : b`).
        let want = if 0.0 > b { 0.0 } else { b };
        if want.to_bits() != a.to_bits() {
            let n = self.c.name(u).to_owned();
            self.c.fail(format!("{n} woken: AwakeTimer {b} -> {a}, the model {want}"));
        }
        self.c.count("observers woken", 1);
        Ok(())
    }

    fn cleaned(&mut self, g: u32, removed: &[u32]) -> Result<(), Desync> {
        let r = self.next("the group's state after its Cleanup")?;
        if obs(r, "mem_s").is_none() || r["at"] != "pass" || !has_fields(r, &ROW) {
            self.c.i -= 1;
            return Err(Desync(format!(
                "the model's Cleanup ends, the recording has {}",
                describe(r)
            )));
        }
        let (rg, row) = self.c.layout_of(r)?;
        if rg != g {
            let (a, b) = (self.c.name(g).to_owned(), self.c.name(rg).to_owned());
            return Err(Desync(format!("{a}'s pass, the recording has {b}'s")));
        }
        let model = self.memory(g)?.layout();
        if model != row {
            let (a, b) = (self.c.show(&model), self.c.show(&row));
            let n = self.c.name(g).to_owned();
            self.c.fail(format!("{n}'s memory after its pass: model {a}, game {b}"));
        }
        // What the tick added and removed so far, in order.
        self.compare_changes()?;
        // `Cleanup`'s removals, in removal order.
        let seen: Vec<u32> = self.removals.iter().filter(|x| x.0 == g).map(|x| x.1).collect();
        self.removals.retain(|x| x.0 != g);
        if seen != removed {
            let (a, b) = (self.c.names_of(removed), self.c.names_of(&seen));
            let n = self.c.name(g).to_owned();
            self.c.fail(format!("{n}'s Cleanup removed {a:?} in the model, {b:?} in the game"));
        }
        self.c.count("groups passed", 1);
        self.c.count("forgotten", removed.len());
        // The rest of the tick goes on from the recorded state.
        self.c.recorded.insert(g, row.clone());
        let mut m = GroupMemory::from_layout(row).expect("checked");
        m.log_changes();
        self.mem.insert(g, m);
        Ok(())
    }
}

impl Check<'_> {
    /// Runs the whole recording.
    fn run(&mut self) {
        let recs = self.recs;
        if !recs.iter().find(|r| r["k"] == "begin").is_some_and(|b| {
            b["observers"].as_array().is_some_and(|o| o.iter().any(|x| x == "memory"))
        }) {
            self.fail("not recorded with the memory observer");
            return;
        }
        while let Some(r) = recs.get(self.i) {
            self.i += 1;
            if let Err(Desync(d)) = self.record(r) {
                self.fail(d);
            }
        }
        self.frame_done();
    }

    /// A frame's end: one tick in a Default-mode frame, none in another.
    fn frame_done(&mut self) {
        let Some(mode) = self.mode else { return };
        let want = usize::from(mode == DEFAULT_MODE);
        if self.frame_ticks != want {
            let n = self.frame_ticks;
            self.fail(format!("a frame of mode {mode} with {n} memory ticks, not {want}"));
        }
        self.count("frames", 1);
    }

    fn record(&mut self, r: &Value) -> Result<(), Desync> {
        if r["k"] == "tick" {
            self.frame_done();
            self.t = r["t"].as_i64().unwrap_or(-1);
            self.frame = r["frame"].as_i64().map(|f| f + self.frame_shift);
            self.mode = Some(r["mode"].as_i64().ok_or_else(|| Desync("a frame's mode".into()))?);
            self.frame_ticks = 0;
            return Ok(());
        }
        if r["k"] != "obs" {
            return Ok(());
        }
        let ev = r["ev"].as_str().unwrap_or("");
        if (ev.starts_with("mem_") || ev == "hash") && r["t"].as_i64() != Some(self.t) {
            return Err(Desync(format!("{} of another frame", describe(r))));
        }
        match ev {
            "mem_init" => {
                if self.started || !has_fields(r, &INIT) {
                    return Err(Desync(format!(
                        "a mem_init out of place or of another shape ({})",
                        describe(r)
                    )));
                }
                if r["g"].as_str().is_some_and(|g| g.starts_with("~fake:")) {
                    return Err(Desync(format!(
                        "a fake group's memory holds entries ({})",
                        r["g"]
                    )));
                }
                let (g, l) = self.layout_of(r)?;
                if self.recorded.insert(g, l).is_some() {
                    return Err(Desync(format!("{}'s initial memory twice", r["g"])));
                }
                self.count("initial memories", 1);
            }
            "mem_initq" => {
                self.initq += 1;
                if self.started || self.initq > 1 || !has_fields(r, &["q", "pending"]) {
                    return Err(Desync(
                        "a mem_initq out of place, twice or of another shape".into(),
                    ));
                }
                self.queue = Some(self.pairs_of(&r["q"])?);
                self.pending = Some(self.ids_of(&r["pending"], "a pending removal")?);
            }
            "hash" => self.see_hash(r),
            "mem_q" => self.queued(r)?,
            "mem_tick" => {
                if !self.started && self.initq != 1 {
                    self.fail("the first tick without the window's mem_initq");
                }
                self.started = true;
                self.frame_ticks += 1;
                self.tick(r)?;
            }
            "mem_v" | "mem_s" | "mem_tick_end" => {
                return Err(Desync(format!("{} outside a tick", describe(r))));
            }
            "mem_add" | "mem_remove" | "mem_clear" if self.started => {
                // The memory changed outside the tick: the next tick's entry records the state.
                self.count("changes outside the tick", 1);
            }
            _ => {}
        }
        Ok(())
    }

    /// `AddToMemory(observer, target)`: queued unless the target's faction is peaceful or the pair
    /// is queued.
    fn queued(&mut self, r: &Value) -> Result<(), Desync> {
        if !has_fields(r, &["o", "tgt", "peaceful", "queued", "exc", "by"]) {
            return Err(Desync(format!("a mem_q of another shape ({r})")));
        }
        if r["exc"] != false {
            return Err(Desync(format!("AddToMemory threw ({r})")));
        }
        let o = self.id_of(&r["o"], "an observer")?;
        let t = self.id_of(&r["tgt"], "a target")?;
        let peaceful = bool_of(r, "peaceful")?;
        let queued = bool_of(r, "queued")?;
        let Some(q) = self.queue.take() else { return Ok(()) };
        let mut c = MemoryController { units_for_add: q, cleanup_pending: Vec::new() };
        let before = c.units_for_add.len();
        c.add_to_memory(o, t, peaceful);
        let model = c.units_for_add.len() > before;
        if model != queued {
            let (a, b) = (self.name(o).to_owned(), self.name(t).to_owned());
            self.fail(format!("AddToMemory({a}, {b}): model queued {model}, game {queued}"));
        }
        self.count(if queued { "additions queued" } else { "additions not queued" }, 1);
        self.queue = Some(c.units_for_add);
        Ok(())
    }

    /// One `UnitMemoryController.Tick`, from `mem_tick` to `mem_tick_end`.
    fn tick(&mut self, r: &Value) -> Result<(), Desync> {
        if !has_fields(r, &["gt", "frame", "q", "pending"]) {
            self.resync();
            return Err(Desync(format!("a mem_tick of another shape ({r})")));
        }
        let gt = r["gt"].as_i64();
        let frame = r["frame"].as_i64().and_then(|f| i32::try_from(f).ok());
        let (Some(gt), Some(frame)) = (gt, frame) else {
            self.resync();
            return Err(Desync("a tick without its GameTime or frame".into()));
        };
        if self.frame != Some(i64::from(frame)) {
            let mine = self.frame;
            self.fail(format!(
                "the tick reads frame {frame}, the tick record's pinned frame is {mine:?}"
            ));
        }
        let q = self.pairs_of(&r["q"])?;
        let pending = self.ids_of(&r["pending"], "a pending removal")?;
        if let Some(model) = self.queue.take() {
            self.count("queues compared", 1);
            if model != q {
                let (a, b) = (format!("{model:?}"), format!("{q:?}"));
                self.fail(format!("the queue at the tick: model {a}, game {b}"));
            }
        }
        if let Some(model) = self.pending.take()
            && model != pending
        {
            self.fail(format!("s_UnitsForRemove at the tick: model {model:?}, game {pending:?}"));
        }
        self.count("ticks", 1);
        // The changes made since the last record, at the tick's entry.
        let recs = self.recs;
        let mut pre = BTreeSet::new();
        while let Some(x) =
            recs.get(self.i).filter(|x| obs(x, "mem_s").is_some() && x["at"] == "pre")
        {
            self.i += 1;
            if !has_fields(x, &ROW) || x["t"].as_i64() != Some(self.t) {
                self.resync();
                return Err(Desync(format!("a mem_s of another shape or frame ({x})")));
            }
            let (g, l) = self.layout_of(x)?;
            // One row per group whose memory changed.
            let last =
                self.recorded.get(&g).cloned().unwrap_or_else(|| GroupMemory::new().layout());
            if !pre.insert(g) || l == last {
                self.resync();
                return Err(Desync(format!("{}'s pre row twice or unchanged", self.name(g))));
            }
            self.recorded.insert(g, l);
            self.count("changes between ticks", 1);
        }
        let mut ctl = MemoryController { units_for_add: q, cleanup_pending: pending };
        let mut w = TickWorld {
            c: self,
            mem: HashMap::new(),
            now: TimeSpan(gt),
            frame,
            removals: Vec::new(),
            changes: Vec::new(),
            model_changes: Vec::new(),
        };
        let res = super::tick(&mut ctl, &mut w).and_then(|()| w.drain()).and_then(|()| w.sync());
        let TickWorld { mem, removals, changes, model_changes, .. } = w;
        let res = res.and_then(|()| {
            if !removals.is_empty() {
                return Err(Desync(format!("Cleanup removals outside a pass: {removals:?}")));
            }
            // The queue's changes, in order.
            check_changes(self, &changes, &model_changes);
            self.tick_end(gt, mem)
        });
        match res {
            Ok(()) => {
                self.queue = Some(ctl.units_for_add);
                self.pending = Some(ctl.cleanup_pending);
            }
            Err(Desync(d)) => {
                self.fail(d);
                self.resync();
            }
        }
        Ok(())
    }

    /// The tick's end: each group whose memory changed since its last record (`mem_s` at "end"),
    /// then `mem_tick_end`; every memory the tick touched equals its last record.
    fn tick_end(&mut self, gt: i64, mut mem: HashMap<u32, GroupMemory<u32>>) -> Result<(), Desync> {
        let recs = self.recs;
        let mut seen = BTreeSet::new();
        loop {
            let r = recs
                .get(self.i)
                .ok_or_else(|| Desync("the recording ends inside a tick".into()))?;
            if r["t"].as_i64() != Some(self.t) || r["k"] != "obs" {
                return Err(Desync(format!("the tick ends at {}", describe(r))));
            }
            if obs(r, "mem_s").is_some() && r["at"] == "end" && has_fields(r, &ROW) {
                self.i += 1;
                let (g, row) = self.layout_of(r)?;
                let before =
                    self.recorded.get(&g).cloned().unwrap_or_else(|| GroupMemory::new().layout());
                // One row per changed group.
                if !seen.insert(g) || row == before {
                    return Err(Desync(format!("{}'s end row twice or unchanged", self.name(g))));
                }
                let model = mem.remove(&g).map_or(before, |m| m.layout());
                if model != row {
                    let (a, b) = (self.show(&model), self.show(&row));
                    self.fail(format!(
                        "{}'s memory at the tick's end: model {a}, game {b}",
                        self.name(g)
                    ));
                }
                self.recorded.insert(g, row);
                self.count("changed by the queue", 1);
                continue;
            }
            if obs(r, "mem_tick_end").is_some() && has_fields(r, &["gt", "exc"]) {
                self.i += 1;
                if r["exc"] != false {
                    self.fail("the game's tick threw");
                }
                if r["gt"].as_i64() != Some(gt) {
                    self.fail(format!("GameTime changed during the tick ({gt} -> {})", r["gt"]));
                }
                break;
            }
            return Err(Desync(format!(
                "the model's tick ends, the recording has {}",
                describe(r)
            )));
        }
        // The memories the tick touched and the game didn't record as changed.
        for (g, m) in mem {
            let model = m.layout();
            let last =
                self.recorded.get(&g).cloned().unwrap_or_else(|| GroupMemory::new().layout());
            if model != last {
                let (a, b) = (self.show(&model), self.show(&last));
                self.fail(format!(
                    "{}'s memory changed in the model only: {a}, game {b}",
                    self.name(g)
                ));
            }
        }
        Ok(())
    }

    /// After a desync: on to the tick's end (its states taken as recorded), leaving the next
    /// frame, tick or window record to the run.
    fn resync(&mut self) {
        let recs = self.recs;
        while let Some(r) = recs.get(self.i) {
            if r["k"] != "obs"
                || matches!(r["ev"].as_str(), Some("mem_tick" | "mem_init" | "mem_initq" | "mem_q"))
            {
                break;
            }
            self.i += 1;
            // Only this frame's records, of their writers' shape, are taken.
            if r["t"].as_i64() != Some(self.t) {
                self.fail(format!("{} of another frame", describe(r)));
                continue;
            }
            if obs(r, "mem_s").is_some() {
                match self.layout_of(r) {
                    Ok((g, l)) if has_fields(r, &ROW) => {
                        self.recorded.insert(g, l);
                    }
                    _ => self.fail(format!("a mem_s it can't read ({})", describe(r))),
                }
            }
            if obs(r, "hash").is_some() {
                self.see_hash(r);
            }
            if obs(r, "mem_tick_end").is_some() {
                break;
            }
        }
        self.queue = None;
        self.pending = None;
    }
}

// ---- the recordings ----

/// The runs to check: `WR_MEMTICK_CAPTURES` (comma-separated labels), both fights, the record and
/// two replays.
fn captures() -> Vec<PathBuf> {
    let labels = std::env::var("WR_MEMTICK_CAPTURES").unwrap_or_else(|_| "poc5-tb-111".into());
    let mut out = Vec::new();
    for label in labels.split(',') {
        let root =
            wr_paths::repo_root().join("fixtures/captures/oracle").join(label).join("fixtures");
        for fx in ["tb-nearest", "tb-focus"] {
            for run in ["record", "replay-1", "replay-2"] {
                out.push(root.join(fx).join(format!("{run}.trace.jsonl.gz")));
            }
        }
    }
    out
}

fn begin_frame(recs: &[Value]) -> Option<i64> {
    recs.iter().find(|r| r["k"] == "begin").and_then(|b| b["frame"].as_i64())
}

fn hashes_of(recs: &[Value]) -> HashMap<String, i64> {
    recs.iter()
        .filter(|r| obs(r, "hash").is_some())
        .filter_map(|r| Some((r["u"].as_str()?.to_owned(), r["h"].as_i64()?)))
        .collect()
}

/// A capture's envelope: it ends with its `end` record, which reports nothing failed, dropped or
/// caught, and its records are numbered in order.
fn envelope_errors(recs: &[Value]) -> Vec<String> {
    let mut e = Vec::new();
    let Some(end) = recs.last().filter(|r| r["k"] == "end") else {
        return vec!["the capture doesn't end with its end record".into()];
    };
    for k in ["records_failed", "records_dropped", "structural_errors", "hook_exceptions_caught"] {
        if end["health"][k].as_i64() != Some(0) {
            e.push(format!("the capture's health: {k} {}", end["health"][k]));
        }
    }
    if recs.iter().enumerate().any(|(i, r)| r["seq"].as_u64() != u64::try_from(i).ok()) {
        e.push("the capture's records aren't numbered in order".into());
    }
    e
}

/// A run's check, with the record run's tape and frame (for a replay).
fn check_run(path: &Path) -> (Vec<String>, BTreeMap<&'static str, usize>) {
    let trace = Trace::load(path).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
    let record = path.with_file_name("record.trace.jsonl.gz");
    let (tape, shift, replay) = if record == path {
        (HashMap::new(), 0, false)
    } else {
        let rec = Trace::load(&record).expect("the record run").records;
        let shift = (begin_frame(&rec).expect("record frame")
            - begin_frame(&trace.records).expect("frame"))
        .rem_euclid(60);
        (hashes_of(&rec), shift, true)
    };
    let mut errors = envelope_errors(&trace.records);
    let mut c = Check::new(&trace.records, tape, shift, replay);
    c.run();
    for k in COVERAGE {
        if c.counts.get(k).copied().unwrap_or(0) == 0 {
            c.errors.push(format!("nothing checked: {k}"));
        }
    }
    errors.extend(c.errors);
    (errors, c.counts)
}

/// What every run must check at least once.
const COVERAGE: [&str; 7] = [
    "ticks",
    "groups passed",
    "near units",
    "line of sight",
    "queues compared",
    "changes between ticks",
    "additions recorded",
];

#[test]
#[ignore = "needs the memory observer's captures (WR_MEMTICK_CAPTURES)"]
fn every_recorded_memory_tick_reruns_to_the_recorded_memory() {
    let mut totals: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    for path in captures() {
        let (errors, counts) = check_run(&path);
        let name = path.strip_prefix(wr_paths::repo_root()).unwrap_or(&path).display().to_string();
        eprintln!("{name}: {counts:?}");
        for (k, v) in &counts {
            *totals.entry(k).or_default() += v;
        }
        failures.extend(errors.iter().take(20).map(|e| format!("{name}: {e}")));
    }
    eprintln!("total: {totals:?}");
    assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
}

/// The capture's records damaged one way at a time: each is caught.
#[test]
#[ignore = "needs the memory observer's captures (WR_MEMTICK_CAPTURES)"]
fn a_damaged_capture_is_caught() {
    let label = std::env::var("WR_MEMTICK_CAPTURES").unwrap_or_else(|_| "poc5-tb-111".into());
    let label = label.split(',').next().unwrap_or_default().to_owned();
    let path = wr_paths::repo_root()
        .join("fixtures/captures/oracle")
        .join(label)
        .join("fixtures/tb-focus/record.trace.jsonl.gz");
    let recs = Trace::load(&path).expect("capture").records;
    let errors = |v: &[Value]| {
        let mut c = Check::new(v, HashMap::new(), 0, false);
        c.run();
        c.errors
    };
    assert_eq!(errors(&recs), Vec::<String>::new());
    let pos = |f: &dyn Fn(&Value) -> bool| recs.iter().position(f).expect("a record to damage");
    let leaf = |r: &Value, f: &str| r["ev"] == "mem_v" && r["f"] == f;
    type Damage = Box<dyn Fn(&mut Vec<Value>)>;
    let los = pos(&|r| leaf(r, "los") && r["v"] == true);
    let near = pos(&|r| leaf(r, "near") && r["v"].as_array().is_some_and(|a| a.len() > 2));
    let grp = pos(&|r| leaf(r, "grp"));
    let gdt = pos(&|r| leaf(r, "gdt"));
    let pass = pos(&|r| r["ev"] == "mem_s" && r["at"] == "pass");
    let wake = pos(&|r| leaf(r, "wake"));
    let q = pos(&|r| r["ev"] == "mem_q");
    let add = pos(&|r| r["ev"] == "mem_add" && r["t"].as_i64() > Some(0));
    let unit = pos(&|r| leaf(r, "unit") && r["v"].is_string());
    let cases: Vec<(&str, Damage)> = vec![
        ("line of sight flipped", Box::new(move |v| v[los]["v"] = json!(false))),
        ("near units reversed", Box::new(move |v| v[near]["v"].as_array_mut().unwrap().reverse())),
        ("a group read changed", Box::new(move |v| v[grp]["v"] = json!("~fake"))),
        ("a long frame", Box::new(move |v| v[gdt]["v"] = json!("0.5"))),
        ("a pass's detect time", Box::new(move |v| v[pass]["slots"][0][1] = json!(0))),
        ("a wake's result", Box::new(move |v| v[wake]["a"] = json!("7"))),
        (
            "a queued flag flipped",
            Box::new(move |v| v[q]["queued"] = json!(!v[q]["queued"].as_bool().unwrap())),
        ),
        ("an addition not recorded", Box::new(move |v| drop(v.remove(add)))),
        ("a unit gone", Box::new(move |v| v[unit]["v"] = json!(null))),
    ];
    for (what, damage) in cases {
        let mut v = recs.clone();
        damage(&mut v);
        assert!(!errors(&v).is_empty(), "{what}: not caught");
    }
}

// ---- the check itself, on records made by hand ----

/// A record (`seq` and `k` filled in) of frame `t`.
fn rec(t: i64, ev: &str, mut body: Value) -> Value {
    body["k"] = json!("obs");
    body["t"] = json!(t);
    body["ev"] = json!(ev);
    body["seq"] = json!(0);
    body
}

fn leaf(t: i64, f: &str, mut body: Value) -> Value {
    body["f"] = json!(f);
    rec(t, "mem_v", body)
}

fn row(t: i64, at: Option<&str>, g: &str, slots: Value) -> Value {
    // One freed slot at most here: it heads the free list.
    let free = slots.as_array().and_then(|s| s.iter().position(|x| x[0].is_null()));
    let (free, nfree) = free.map_or((-1, 0), |i| (i64::try_from(i).unwrap(), 1));
    let mut b = json!({"g": g, "slots": slots, "free": free, "nfree": nfree, "list": null, "enemies": null});
    if let Some(at) = at {
        b["at"] = json!(at);
    }
    rec(t, if at.is_some() { "mem_s" } else { "mem_init" }, b)
}

fn frame(t: i64) -> Value {
    json!({"k": "tick", "t": t, "seq": 0, "frame": 101 + t, "mode": DEFAULT_MODE})
}

const GT: i64 = 434_800_860_000;

/// A window whose first tick has group A (units a1, a2) see enemy e1 of group E and friend f1 of
/// group F (not on its frame), then forget the gone unit x9; the queue then makes F remember e1.
/// The second tick is in a peaceful area; the third adds e1 to F's memory again (already there).
fn window() -> Vec<Value> {
    let begin = json!({"k": "begin", "t": -1, "seq": 0, "frame": 100, "observers": ["memory"]});
    let pass_a = json!([["a1", GT, false], [null, -1], ["e1", GT, true], ["a2", GT, false]]);
    let q = |o: &str, t: &str, peaceful: bool, queued: bool| {
        rec(
            0,
            "mem_q",
            json!({"o": o, "tgt": t, "peaceful": peaceful, "queued": queued, "exc": false, "by": "x"}),
        )
    };
    vec![
        begin,
        frame(0),
        row(0, None, "A", json!([["a1", 0, false], ["x9", GT, false]])),
        rec(0, "mem_initq", json!({"q": [["f1", "e1"]], "pending": []})),
        rec(0, "mem_tick", json!({"gt": GT, "frame": 101, "q": [["f1", "e1"]], "pending": []})),
        leaf(0, "apeace", json!({"v": false})),
        leaf(0, "ready", json!({"v": ["A"]})),
        leaf(0, "near", json!({"g": "A", "v": ["a1", "e1", "a2", "f1"]})),
        leaf(0, "peace", json!({"u": "a1", "v": false})),
        leaf(0, "pass", json!({"u": "a1", "v": false})),
        leaf(0, "grp", json!({"u": "a1", "v": "A"})),
        leaf(0, "peace", json!({"u": "e1", "v": false})),
        leaf(0, "pass", json!({"u": "e1", "v": false})),
        leaf(0, "grp", json!({"u": "e1", "v": "E"})),
        leaf(0, "stl", json!({"u": "e1", "g": "A", "v": false})),
        leaf(0, "enemy", json!({"g": "A", "u": "e1", "v": true})),
        leaf(0, "los", json!({"g": "A", "u": "e1", "v": true})),
        rec(0, "mem_add", json!({"group": "A", "u": "e1", "by": "x"})),
        leaf(0, "inv", json!({"u": "e1", "c": 7, "v": false})),
        leaf(0, "peace", json!({"u": "a2", "v": false})),
        leaf(0, "pass", json!({"u": "a2", "v": false})),
        leaf(0, "grp", json!({"u": "a2", "v": "A"})),
        rec(0, "mem_add", json!({"group": "A", "u": "a2", "by": "x"})),
        leaf(0, "peace", json!({"u": "f1", "v": false})),
        leaf(0, "pass", json!({"u": "f1", "v": false})),
        leaf(0, "grp", json!({"u": "f1", "v": "F"})),
        leaf(0, "stl", json!({"u": "f1", "g": "A", "v": false})),
        leaf(0, "enemy", json!({"g": "A", "u": "f1", "v": false})),
        leaf(0, "gdt", json!({"v": "0.0166666675"})),
        // (392888320 >>> 2) % 5 == 0, and frame 101 % 5 == 1: not its frame.
        rec(0, "hash", json!({"u": "f1", "h": 392_888_320})),
        // Cleanup: a1 (kept), x9 (gone), e1, a2 (kept).
        leaf(0, "unit", json!({"u": "a1", "v": "a1"})),
        leaf(0, "unit", json!({"u": "a1", "v": "a1"})),
        leaf(0, "ingame", json!({"u": "a1", "v": true})),
        leaf(0, "unit", json!({"u": "x9", "v": null})),
        leaf(0, "unit", json!({"u": "e1", "v": "e1"})),
        leaf(0, "unit", json!({"u": "e1", "v": "e1"})),
        leaf(0, "ingame", json!({"u": "e1", "v": true})),
        leaf(0, "unit", json!({"u": "a2", "v": "a2"})),
        leaf(0, "unit", json!({"u": "a2", "v": "a2"})),
        leaf(0, "ingame", json!({"u": "a2", "v": true})),
        rec(0, "mem_remove", json!({"group": "A", "u": "x9", "by": "Cleanup"})),
        row(0, Some("pass"), "A", pass_a.clone()),
        // The queue: f1 (group F) remembers e1.
        leaf(0, "disp", json!({"u": "f1", "v": false})),
        leaf(0, "disp", json!({"u": "e1", "v": false})),
        leaf(0, "grp", json!({"u": "f1", "v": "F"})),
        leaf(0, "stl", json!({"u": "e1", "g": "F", "v": false})),
        leaf(0, "grp", json!({"u": "f1", "v": "F"})),
        leaf(0, "grp", json!({"u": "f1", "v": "F"})),
        rec(0, "mem_add", json!({"group": "F", "u": "e1", "by": "x"})),
        leaf(0, "wake", json!({"u": "f1", "time": "0", "fade": false, "b": "-1", "a": "0"})),
        row(0, Some("end"), "F", json!([["e1", GT, false]])),
        rec(0, "mem_tick_end", json!({"gt": GT, "exc": false})),
        // Between ticks: an enemy command on f1 queues a pair, a peaceful target doesn't.
        q("e1", "f1", false, true),
        q("e1", "f1", false, false),
        q("e1", "p1", true, false),
        q("a1", "x7", false, true),
        frame(1),
        rec(
            1,
            "mem_tick",
            json!({"gt": GT + 1, "frame": 102, "q": [["e1", "f1"], ["a1", "x7"]], "pending": []}),
        ),
        row(
            1,
            Some("pre"),
            "A",
            json!([["a1", GT, false], [null, -1], ["e1", GT, false], ["a2", GT, false]]),
        ),
        leaf(1, "apeace", json!({"v": true})),
        rec(1, "mem_tick_end", json!({"gt": GT + 1, "exc": false})),
        frame(2),
        rec(
            2,
            "mem_tick",
            json!({"gt": GT + 2, "frame": 103, "q": [["e1", "f1"], ["a1", "x7"]], "pending": []}),
        ),
        leaf(2, "apeace", json!({"v": false})),
        leaf(2, "ready", json!({"v": []})),
        leaf(2, "disp", json!({"u": "e1", "v": false})),
        leaf(2, "disp", json!({"u": "f1", "v": false})),
        leaf(2, "grp", json!({"u": "e1", "v": "E"})),
        leaf(2, "stl", json!({"u": "f1", "g": "E", "v": false})),
        leaf(2, "grp", json!({"u": "e1", "v": "E"})),
        leaf(2, "grp", json!({"u": "e1", "v": "E"})),
        rec(2, "mem_add", json!({"group": "E", "u": "f1", "by": "x"})),
        leaf(2, "wake", json!({"u": "e1", "time": "0", "fade": false, "b": "3", "a": "3"})),
        // a1 (group A) remembers x7, in A's freed slot.
        leaf(2, "disp", json!({"u": "a1", "v": false})),
        leaf(2, "disp", json!({"u": "x7", "v": false})),
        leaf(2, "grp", json!({"u": "a1", "v": "A"})),
        leaf(2, "stl", json!({"u": "x7", "g": "A", "v": false})),
        leaf(2, "grp", json!({"u": "a1", "v": "A"})),
        leaf(2, "grp", json!({"u": "a1", "v": "A"})),
        rec(2, "mem_add", json!({"group": "A", "u": "x7", "by": "x"})),
        leaf(2, "wake", json!({"u": "a1", "time": "0", "fade": false, "b": "0", "a": "0"})),
        row(2, Some("end"), "E", json!([["f1", GT + 2, false]])),
        row(
            2,
            Some("end"),
            "A",
            json!([["a1", GT, false], ["x7", GT + 2, false], ["e1", GT, false], ["a2", GT, false]]),
        ),
        rec(2, "mem_tick_end", json!({"gt": GT + 2, "exc": false})),
    ]
}

fn errors_of(v: &[Value]) -> Vec<String> {
    let mut c = Check::new(v, HashMap::new(), 0, false);
    c.run();
    c.errors
}

#[test]
fn a_hand_made_window_checks_clean() {
    let v = window();
    let mut c = Check::new(&v, HashMap::new(), 0, false);
    c.run();
    assert_eq!(c.errors, Vec::<String>::new());
    assert_eq!(c.counts["frames"], 3);
    assert_eq!(c.counts["ticks"], 3);
    assert_eq!(c.counts["groups passed"], 1);
    assert_eq!(c.counts["forgotten"], 1);
    assert_eq!(c.counts["friend phase computed"], 1);
    assert_eq!(c.counts["changed by the queue"], 3);
    assert_eq!(c.counts["additions recorded"], 5);
    assert_eq!(c.counts["additions queued"], 2);
    assert_eq!(c.counts["additions not queued"], 2);
    assert_eq!(c.counts["observers woken"], 3);
}

#[test]
fn a_tick_after_a_desync_is_still_checked() {
    let mut v = window();
    // The first tick's model reads one leaf more than the game recorded.
    let i = v.iter().position(|r| r["f"] == "wake").unwrap();
    v.remove(i);
    let mut c = Check::new(&v, HashMap::new(), 0, false);
    c.run();
    assert_eq!(c.errors.len(), 1, "{:?}", c.errors);
    assert_eq!(c.counts["ticks"], 3);
    assert_eq!(c.counts["changed by the queue"], 2);
    // And the third tick's own damage is still caught.
    let j = v.iter().rposition(|r| r["f"] == "wake").unwrap();
    v[j]["a"] = json!("5");
    assert_eq!(errors_of(&v).len(), 2);
}

#[test]
#[allow(clippy::too_many_lines)]
fn a_damaged_window_is_caught() {
    let v = window();
    let at = |f: &dyn Fn(&Value) -> bool| v.iter().position(f).expect("record");
    let is_leaf = |r: &Value, f: &str, u: &str| r["ev"] == "mem_v" && r["f"] == f && r["u"] == u;
    type Damage = Box<dyn Fn(&mut Vec<Value>)>;
    let flip = |i: usize, k: &'static str| -> Damage {
        Box::new(move |v| {
            let b = v[i][k].as_bool().unwrap();
            v[i][k] = json!(!b);
        })
    };
    let remove = |i: usize| -> Damage { Box::new(move |v| drop(v.remove(i))) };
    let twice = |i: usize| -> Damage {
        Box::new(move |v| {
            let r = v[i].clone();
            v.insert(i, r);
        })
    };
    let los = at(&|r| is_leaf(r, "los", "e1"));
    let grp = at(&|r| is_leaf(r, "grp", "a2"));
    let hash = at(&|r| r["ev"] == "hash");
    let gdt = at(&|r| r["f"] == "gdt");
    let pass = at(&|r| r["at"] == "pass");
    let pre = at(&|r| r["at"] == "pre");
    let end = at(&|r| r["at"] == "end");
    let wake = at(&|r| r["f"] == "wake");
    let q = at(&|r| r["ev"] == "mem_q");
    let initq = at(&|r| r["ev"] == "mem_initq");
    let removal = at(&|r| r["ev"] == "mem_remove");
    let add = at(&|r| r["ev"] == "mem_add");
    let unit = at(&|r| is_leaf(r, "unit", "x9"));
    let ingame = at(&|r| is_leaf(r, "ingame", "a2"));
    let tick2 = at(&|r| r["ev"] == "mem_tick" && r["t"] == 1);
    let tick3 = at(&|r| r["ev"] == "mem_tick" && r["t"] == 2);
    let near = at(&|r| r["f"] == "near");
    let tick_end = at(&|r| r["ev"] == "mem_tick_end");
    let frame3 = at(&|r| r["k"] == "tick" && r["t"] == 2);
    let cases: Vec<(&str, Damage)> = vec![
        ("line of sight flipped", flip(los, "v")),
        ("an own unit taken for another group's", Box::new(move |v| v[grp]["v"] = json!("E"))),
        ("a friend's hash on its frame", Box::new(move |v| v[hash]["h"] = json!(392_888_324))),
        ("a hash twice", twice(hash)),
        ("a long frame", Box::new(move |v| v[gdt]["v"] = json!("0.5"))),
        ("a pass's detect time", Box::new(move |v| v[pass]["slots"][0][1] = json!(GT + 1))),
        ("a pass's free slot", Box::new(move |v| v[pass]["slots"][1] = json!(["x9", 0, false]))),
        ("a pass's free list head", Box::new(move |v| v[pass]["free"] = json!(3))),
        ("the queue's addition not recorded", remove(end)),
        ("Wake to another value", Box::new(move |v| v[wake]["a"] = json!("-1"))),
        ("a queued flag flipped", flip(q, "queued")),
        ("a removal not recorded", remove(removal)),
        ("an addition not recorded", remove(add)),
        ("an addition twice", twice(add)),
        ("an addition of another unit", Box::new(move |v| v[add]["u"] = json!("a1"))),
        ("a removal by Remove", Box::new(move |v| v[removal]["by"] = json!("Remove"))),
        (
            "a memory cleared in the tick",
            Box::new(move |v| v.insert(los, rec(0, "mem_clear", json!({"group": "A", "n": 2})))),
        ),
        ("a gone unit resolved", Box::new(move |v| v[unit]["v"] = json!("x9"))),
        ("a unit out of the game", flip(ingame, "v")),
        ("the next tick's queue", Box::new(move |v| v[tick2]["q"] = json!([]))),
        ("the next tick's frame", Box::new(move |v| v[tick2]["frame"] = json!(103))),
        ("a pre row of another frame", Box::new(move |v| v[pre]["t"] = json!(900))),
        ("an end row of another frame", Box::new(move |v| v[end]["t"] = json!(900))),
        ("a tick end of another frame", Box::new(move |v| v[tick_end]["t"] = json!(900))),
        ("a mem_q of another frame", Box::new(move |v| v[q]["t"] = json!(900))),
        ("near units reordered", Box::new(move |v| v[near]["v"] = json!(["a1", "a2", "e1", "f1"]))),
        ("a leaf's extra field", Box::new(move |v| v[los]["x"] = json!(1))),
        ("an end GameTime", Box::new(move |v| v[tick_end]["gt"] = json!(GT + 5))),
        ("no mem_initq", remove(initq)),
        ("mem_initq twice", twice(initq)),
        ("a frame without its tick", Box::new(move |v| v.truncate(tick3))),
        ("a frame of another mode with a tick", Box::new(move |v| v[frame3]["mode"] = json!(4))),
        ("not the memory observer", Box::new(|v| v[0]["observers"] = json!([]))),
        (
            "two additions swapped",
            Box::new(move |v| {
                let j = v.iter().skip(add + 1).position(|r| r["ev"] == "mem_add").unwrap();
                let (x, y) = (v[add]["u"].clone(), v[add + 1 + j]["u"].clone());
                v[add]["u"] = y;
                v[add + 1 + j]["u"] = x;
            }),
        ),
        (
            "two groups' additions in the queue swapped",
            Box::new(|v| {
                let adds: Vec<usize> =
                    (0..v.len()).filter(|&i| v[i]["ev"] == "mem_add" && v[i]["t"] == 2).collect();
                let [a, b] = adds[..] else { panic!("two additions in the third tick") };
                v.swap(a, b);
            }),
        ),
        ("a pre row twice", twice(pre)),
        ("an end row twice", twice(end)),
        (
            "an addition without its caller",
            Box::new(move |v| drop(v[add].as_object_mut().unwrap().remove("by"))),
        ),
        ("an addition with another field", Box::new(move |v| v[add]["x"] = json!(1))),
    ];
    for (what, damage) in cases {
        let mut d = v.clone();
        damage(&mut d);
        assert!(!errors_of(&d).is_empty(), "{what}: not caught");
    }
}

#[test]
fn a_replay_takes_its_hashes_from_the_record_runs_tape() {
    let mut v = window();
    let i = v.iter().position(|r| r["ev"] == "hash").unwrap();
    v[i]["tape"] = json!(true);
    let tape = HashMap::from([("f1".to_owned(), 392_888_320)]);
    let errors = |v: &[Value], tape: &HashMap<String, i64>| {
        let mut c = Check::new(v, tape.clone(), 0, true);
        c.run();
        c.errors
    };
    assert_eq!(errors(&v, &tape), Vec::<String>::new());
    // The same phase, another value; the tape's value said not to be; no tape value.
    let mut d = v.clone();
    d[i]["h"] = json!(392_888_340);
    assert!(!errors(&d, &tape).is_empty());
    let mut d = v.clone();
    d[i]["tape"] = json!(false);
    assert!(!errors(&d, &tape).is_empty());
    assert!(!errors(&v, &HashMap::new()).is_empty());
    let mut d = v.clone();
    d[i]["tape"] = json!("yes");
    assert!(!errors(&d, &tape).is_empty());
}

#[test]
fn a_resynchronisation_takes_only_its_frames_states() {
    let mut v = window();
    let i = v.iter().position(|r| r["f"] == "wake").unwrap();
    v.remove(i);
    let end = v.iter().position(|r| r["at"] == "end").unwrap();
    v[end]["t"] = json!(900);
    assert_eq!(errors_of(&v).len(), 2, "{:?}", errors_of(&v));
}

#[test]
fn a_capture_whose_envelope_is_damaged_is_caught() {
    let end = json!({"k": "end", "seq": 2, "health": {"records_failed": 0, "records_dropped": 0, "structural_errors": 0, "hook_exceptions_caught": 0}});
    let ok = vec![json!({"k": "begin", "seq": 0}), json!({"k": "tick", "seq": 1}), end.clone()];
    assert!(envelope_errors(&ok).is_empty());
    assert!(!envelope_errors(&ok[..2]).is_empty());
    let mut bad = ok.clone();
    bad[2]["health"]["hook_exceptions_caught"] = json!(1);
    assert!(!envelope_errors(&bad).is_empty());
    let mut gap = ok;
    gap[2]["seq"] = json!(3);
    assert!(!envelope_errors(&gap).is_empty());
}

/// A record's short description, for reports.
fn describe(r: &Value) -> String {
    if r["k"] == "tick" {
        return format!("the next frame ({})", r["t"]);
    }
    let mut s = r["ev"].as_str().or(r["k"].as_str()).unwrap_or("?").to_owned();
    for k in ["f", "at", "g", "u", "t"] {
        if let Some(v) = r.get(k).filter(|v| !v.is_null()) {
            s.push_str(&format!(" {k}={v}"));
        }
    }
    s
}
