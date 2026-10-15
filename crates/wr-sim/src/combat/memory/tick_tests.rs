//! The controller's tick against WrathOracle's `memory` observer (docs/poc5/memory.md, H1): every
//! recorded `UnitMemoryController.Tick` rerun from the recorded state with its recorded reads, the
//! memory compared after each group's pass and at the tick's end; the queue's additions
//! (`AddToMemory`) checked as they come.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::*;
use crate::combat::oracle::Trace;

/// `UnitCondition.Invisible`.
const INVISIBLE: i64 = 7;

/// A failure to follow the recording: the run's check stops at this tick.
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
    /// The controller's queue as the model keeps it (`mem_initq`, then each `mem_q`); `None` after
    /// a desync, until the next tick's entry says what it holds.
    queue: Option<Vec<(u32, u32)>>,
    /// `s_UnitsForRemove` as the model keeps it.
    pending: Option<Vec<u32>>,
    /// This run's identity hashes (`hash`, on first use).
    hashes: HashMap<u32, i32>,
    /// The record run's hashes (a replay's taped values are checked against them).
    tape: HashMap<String, i64>,
    /// A replay's pinned `Time.frameCount` is its own plus this.
    frame_shift: i64,
    /// The frame being run: its `t` and its pinned `Time.frameCount`.
    t: i64,
    frame: Option<i64>,
    started: bool,
    errors: Vec<String>,
    counts: BTreeMap<&'static str, usize>,
}

impl<'a> Check<'a> {
    fn new(recs: &'a [Value], tape: HashMap<String, i64>, frame_shift: i64) -> Self {
        Self {
            recs,
            i: 0,
            ids: HashMap::new(),
            names: Vec::new(),
            recorded: HashMap::new(),
            queue: Some(Vec::new()),
            pending: Some(Vec::new()),
            hashes: HashMap::new(),
            tape,
            frame_shift,
            t: -1,
            frame: None,
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

    /// A memory record's layout (`mem_init`, `mem_s`): its group and the memory, checked to be a
    /// dictionary's.
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
        let l = MemoryLayout {
            slots,
            free_list: if nfree == 0 { -1 } else { free },
            free_count: nfree,
            units_list: cache(self, &r["list"], "a cached UnitsList")?,
            enemies: cache(self, &r["enemies"], "a cached Enemies")?,
        };
        if GroupMemory::from_layout(l.clone()).is_none() {
            return Err(Desync(format!("{}'s memory isn't a dictionary's layout", self.name(g))));
        }
        Ok((g, l))
    }

    /// A layout as text, for reports.
    fn show(&self, l: &MemoryLayout<u32>) -> String {
        let slots: Vec<String> = l
            .slots
            .iter()
            .map(|s| match s {
                RawSlot::Used(k, i) => {
                    format!(
                        "{}@{}{}",
                        self.name(*k),
                        i.last_detect_time.0,
                        if i.visible { "v" } else { "" }
                    )
                }
                RawSlot::Free(n) => format!("~{n}"),
            })
            .collect();
        let ids = |v: &Option<Vec<u32>>| {
            v.as_ref().map(|v| v.iter().map(|&k| self.name(k).to_owned()).collect::<Vec<_>>())
        };
        format!(
            "[{}] free {}/{} list {:?} enemies {:?}",
            slots.join(" "),
            l.free_list,
            l.free_count,
            ids(&l.units_list),
            ids(&l.enemies)
        )
    }

    /// `hash` (a unit's identity hash on first use): this run's value, and in a replay the tape's.
    fn see_hash(&mut self, r: &Value) {
        let (Some(u), Some(h)) = (r["u"].as_str(), r["h"].as_i64()) else {
            self.fail(format!("a hash record it can't read ({r})"));
            return;
        };
        let Ok(h32) = i32::try_from(h) else {
            self.fail(format!("a hash out of range ({r})"));
            return;
        };
        if r["tape"] == true && self.tape.get(u) != Some(&h) {
            self.fail(format!("{u}'s taped hash {h} isn't the record run's"));
        }
        let k = self.id(u);
        if self.hashes.insert(k, h32).is_some_and(|old| old != h32) {
            self.fail(format!("{u}'s hash recorded twice, differently"));
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

/// A record the tick asks for: never passed on the way to a leaf.
fn boundary(r: &Value) -> bool {
    r["k"] == "tick"
        || (r["k"] == "obs"
            && matches!(
                r["ev"].as_str(),
                Some(
                    "mem_v"
                        | "mem_s"
                        | "mem_tick"
                        | "mem_tick_end"
                        | "mem_init"
                        | "mem_initq"
                        | "mem_q"
                )
            ))
}

/// The tick's world: its reads pulled strictly as the next records.
struct TickWorld<'c, 'a> {
    c: &'c mut Check<'a>,
    /// The memories the tick touched, from their recorded state.
    mem: HashMap<u32, GroupMemory<u32>>,
    now: TimeSpan,
    frame: i32,
    /// `Cleanup`'s removals (`mem_remove`, by `Cleanup`) seen on the way: (group, unit).
    removals: Vec<(u32, u32)>,
}

impl<'a> TickWorld<'_, 'a> {
    /// A record passed on the way to the next one the tick asks for.
    fn on_the_way(&mut self, r: &Value) -> Result<(), Desync> {
        if r["t"].as_i64() != Some(self.c.t) {
            return Err(Desync(format!("a record of another frame inside the tick ({r})")));
        }
        if obs(r, "hash").is_some() {
            self.c.see_hash(r);
        } else if obs(r, "mem_remove").is_some() && r["by"] == "Cleanup" {
            let g = self.c.id_of(&r["group"], "a removal's group")?;
            let u = self.c.id_of(&r["u"], "a removal's unit")?;
            self.removals.push((g, u));
        }
        Ok(())
    }

    /// Up to the next record the tick asks for.
    fn drain(&mut self) -> Result<(), Desync> {
        let recs = self.c.recs;
        while let Some(r) = recs.get(self.c.i) {
            if boundary(r) {
                return Ok(());
            }
            self.on_the_way(r)?;
            self.c.i += 1;
        }
        Ok(())
    }

    /// The next record the tick asks for.
    fn next(&mut self, what: &str) -> Result<&'a Value, Desync> {
        self.drain()?;
        let recs: &'a [Value] = self.c.recs;
        let r = recs.get(self.c.i).ok_or_else(|| Desync(format!("{what}: the recording ends")))?;
        if r["t"].as_i64() != Some(self.c.t) {
            return Err(Desync(format!("{what}: the recording has {}", describe(r))));
        }
        self.c.i += 1;
        Ok(r)
    }

    /// The next leaf, of `f`, with exactly its writer's fields.
    fn leaf(&mut self, f: &str, payload: &[&str]) -> Result<&'a Value, Desync> {
        let r = self.next(&format!("the model reads {f}"))?;
        if obs(r, "mem_v").is_none() || r["f"] != f {
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
        if r["id"].as_str().is_none() || r["v"] != r["id"] {
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
        if r["c"].as_i64() != Some(INVISIBLE) {
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
            let m = match self.c.recorded.get(&g) {
                Some(l) => GroupMemory::from_layout(l.clone()).expect("checked when recorded"),
                None => GroupMemory::new(),
            };
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
            self.c
                .fail(format!("{} woken: AwakeTimer {b} -> {a}, the model {want}", self.c.name(u)));
        }
        self.c.count("observers woken", 1);
        Ok(())
    }

    fn cleaned(&mut self, g: u32, removed: &[u32]) -> Result<(), Desync> {
        let r = self.next("the group's state after its Cleanup")?;
        if obs(r, "mem_s").is_none() || r["at"] != "pass" || !has_fields(r, &ROW) {
            return Err(Desync(format!(
                "the model's Cleanup ends, the recording has {}",
                describe(r)
            )));
        }
        let (rg, row) = self.c.layout_of(r)?;
        if rg != g {
            return Err(Desync(format!(
                "{}'s pass, the recording has {}'s",
                self.c.name(g),
                self.c.name(rg)
            )));
        }
        let model = self.memory(g)?.layout();
        if model != row {
            let (a, b) = (self.c.show(&model), self.c.show(&row));
            self.c.fail(format!("{}'s memory after its pass: model {a}, game {b}", self.c.name(g)));
        }
        // `Cleanup`'s removals, in removal order.
        let seen: Vec<u32> = self.removals.iter().filter(|x| x.0 == g).map(|x| x.1).collect();
        self.removals.retain(|x| x.0 != g);
        if seen != removed {
            let n = |v: &[u32]| v.iter().map(|&k| self.c.name(k).to_owned()).collect::<Vec<_>>();
            let (a, b) = (n(removed), n(&seen));
            self.c.fail(format!(
                "{}'s Cleanup removed {a:?} in the model, {b:?} in the game",
                self.c.name(g)
            ));
        }
        self.c.count("groups passed", 1);
        self.c.count("forgotten", removed.len());
        // The rest of the tick goes on from the recorded state.
        self.c.recorded.insert(g, row.clone());
        self.mem.insert(g, GroupMemory::from_layout(row).expect("checked"));
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
    }

    fn record(&mut self, r: &Value) -> Result<(), Desync> {
        if r["k"] == "tick" {
            self.t = r["t"].as_i64().unwrap_or(-1);
            self.frame = r["frame"].as_i64().map(|f| f + self.frame_shift);
            return Ok(());
        }
        if r["k"] != "obs" {
            return Ok(());
        }
        match r["ev"].as_str().unwrap_or("") {
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
                self.recorded.insert(g, l);
                self.count("initial memories", 1);
            }
            "mem_initq" => {
                if self.started || !has_fields(r, &["q", "pending"]) {
                    return Err(Desync("a mem_initq out of place or of another shape".into()));
                }
                self.queue = Some(self.pairs_of(&r["q"])?);
                self.pending = Some(self.ids_of(&r["pending"], "a pending removal")?);
            }
            "hash" => self.see_hash(r),
            "mem_q" => self.queued(r)?,
            "mem_tick" => {
                self.started = true;
                self.tick(r)?;
            }
            "mem_v" | "mem_s" | "mem_tick_end" => {
                return Err(Desync(format!("{} outside a tick", describe(r))));
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
        if r["t"].as_i64() != Some(self.t) || !has_fields(r, &["gt", "frame", "q", "pending"]) {
            return Err(Desync(format!("a mem_tick of another frame or shape ({r})")));
        }
        let gt = r["gt"].as_i64().ok_or_else(|| Desync("a tick without its GameTime".into()))?;
        let frame = r["frame"].as_i64().and_then(|f| i32::try_from(f).ok());
        let frame = frame.ok_or_else(|| Desync("a tick without its frame".into()))?;
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
                self.fail(format!("the queue at the tick: model {model:?}, game {q:?}"));
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
        while let Some(x) =
            recs.get(self.i).filter(|x| obs(x, "mem_s").is_some() && x["at"] == "pre")
        {
            self.i += 1;
            if !has_fields(x, &ROW) {
                return Err(Desync(format!("a mem_s of another shape ({x})")));
            }
            let (g, l) = self.layout_of(x)?;
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
        };
        let res = super::tick(&mut ctl, &mut w).and_then(|()| w.drain());
        let TickWorld { mem, removals, .. } = w;
        match res {
            Ok(()) => {
                if !removals.is_empty() {
                    self.fail(format!("Cleanup removals outside a pass: {removals:?}"));
                }
                self.tick_end(gt, mem)?;
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
        loop {
            let r = recs
                .get(self.i)
                .ok_or_else(|| Desync("the recording ends inside a tick".into()))?;
            self.i += 1;
            if obs(r, "mem_s").is_some() && r["at"] == "end" && has_fields(r, &ROW) {
                let (g, row) = self.layout_of(r)?;
                let model = match mem.remove(&g) {
                    Some(m) => m.layout(),
                    None => self
                        .recorded
                        .get(&g)
                        .cloned()
                        .unwrap_or_else(|| GroupMemory::new().layout()),
                };
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

    /// After a desync: on to the tick's end, its states taken as recorded.
    fn resync(&mut self) {
        let recs = self.recs;
        while let Some(r) = recs.get(self.i) {
            self.i += 1;
            if obs(r, "mem_s").is_some()
                && let Ok((g, l)) = self.layout_of(r)
            {
                self.recorded.insert(g, l);
            }
            if obs(r, "hash").is_some() {
                self.see_hash(r);
            }
            if obs(r, "mem_tick_end").is_some() || r["k"] == "tick" {
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
    let labels = std::env::var("WR_MEMTICK_CAPTURES").unwrap_or_else(|_| "poc5-tb-104".into());
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

/// A run's check, with the record run's tape and frame (for a replay).
fn check_run(path: &Path) -> (Vec<String>, BTreeMap<&'static str, usize>) {
    let trace = Trace::load(path).unwrap_or_else(|e| panic!("{}: {e:#}", path.display()));
    let record = path.with_file_name("record.trace.jsonl.gz");
    let (tape, shift) = if record == path {
        (HashMap::new(), 0)
    } else {
        let rec = Trace::load(&record).expect("the record run").records;
        let shift = (begin_frame(&rec).expect("record frame")
            - begin_frame(&trace.records).expect("frame"))
        .rem_euclid(60);
        (hashes_of(&rec), shift)
    };
    checked(&trace.records, tape, shift)
}

fn checked(
    recs: &[Value],
    tape: HashMap<String, i64>,
    shift: i64,
) -> (Vec<String>, BTreeMap<&'static str, usize>) {
    let mut c = Check::new(recs, tape, shift);
    c.run();
    for k in COVERAGE {
        if c.counts.get(k).copied().unwrap_or(0) == 0 {
            c.errors.push(format!("nothing checked: {k}"));
        }
    }
    (c.errors, c.counts)
}

/// What every run must check at least once.
const COVERAGE: [&str; 6] = [
    "ticks",
    "groups passed",
    "near units",
    "line of sight",
    "queues compared",
    "changes between ticks",
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

const GT: i64 = 434_800_860_000;

/// A window whose first tick has group A (units a1, a2) see enemy e1 of group E and friend f1 of
/// group F (on its frame), then forget the gone unit x9; the queue then makes F remember e1.
fn window() -> Vec<Value> {
    let begin = json!({"k": "begin", "t": -1, "seq": 0, "frame": 100, "observers": ["memory"]});
    vec![
        begin,
        row(-1, None, "A", json!([["a1", 0, false], ["x9", GT, false]])),
        rec(-1, "mem_initq", json!({"q": [["f1", "e1"]], "pending": []})),
        json!({"k": "tick", "t": 0, "seq": 0, "frame": 101}),
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
        leaf(0, "inv", json!({"u": "e1", "c": 7, "v": false})),
        leaf(0, "peace", json!({"u": "a2", "v": false})),
        leaf(0, "pass", json!({"u": "a2", "v": false})),
        leaf(0, "grp", json!({"u": "a2", "v": "A"})),
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
        row(
            0,
            Some("pass"),
            "A",
            json!([["a1", GT, false], [null, -1], ["e1", GT, true], ["a2", GT, false]]),
        ),
        // The queue: f1 (group F) remembers e1.
        leaf(0, "disp", json!({"u": "f1", "v": false})),
        leaf(0, "disp", json!({"u": "e1", "v": false})),
        leaf(0, "grp", json!({"u": "f1", "v": "F"})),
        leaf(0, "stl", json!({"u": "e1", "g": "F", "v": false})),
        leaf(0, "grp", json!({"u": "f1", "v": "F"})),
        leaf(0, "grp", json!({"u": "f1", "v": "F"})),
        leaf(0, "wake", json!({"u": "f1", "time": "0", "fade": false, "b": "-1", "a": "0"})),
        row(0, Some("end"), "F", json!([["e1", GT, false]])),
        rec(0, "mem_tick_end", json!({"gt": GT, "exc": false})),
        // Between ticks: an enemy command on f1 queues a pair, a peaceful target doesn't.
        rec(
            0,
            "mem_q",
            json!({"o": "e1", "tgt": "f1", "peaceful": false, "queued": true, "exc": false, "by": "x"}),
        ),
        rec(
            0,
            "mem_q",
            json!({"o": "e1", "tgt": "f1", "peaceful": false, "queued": false, "exc": false, "by": "x"}),
        ),
        rec(
            0,
            "mem_q",
            json!({"o": "e1", "tgt": "p1", "peaceful": true, "queued": false, "exc": false, "by": "x"}),
        ),
        json!({"k": "tick", "t": 1, "seq": 0, "frame": 102}),
        rec(1, "mem_tick", json!({"gt": GT + 1, "frame": 102, "q": [["e1", "f1"]], "pending": []})),
        row(
            1,
            Some("pre"),
            "A",
            json!([["a1", GT, false], [null, -1], ["e1", GT, true], ["a2", GT, false]]),
        ),
        leaf(1, "apeace", json!({"v": true})),
        rec(1, "mem_tick_end", json!({"gt": GT + 1, "exc": false})),
    ]
}

fn errors_of(v: &[Value]) -> Vec<String> {
    let mut c = Check::new(v, HashMap::new(), 0);
    c.run();
    c.errors
}

#[test]
fn a_hand_made_window_checks_clean() {
    let v = window();
    let mut c = Check::new(&v, HashMap::new(), 0);
    c.run();
    assert_eq!(c.errors, Vec::<String>::new());
    assert_eq!(c.counts["ticks"], 2);
    assert_eq!(c.counts["groups passed"], 1);
    assert_eq!(c.counts["forgotten"], 1);
    assert_eq!(c.counts["friend phase computed"], 1);
    assert_eq!(c.counts["changed by the queue"], 1);
    assert_eq!(c.counts["additions queued"], 1);
    assert_eq!(c.counts["additions not queued"], 2);
    assert_eq!(c.counts["observers woken"], 1);
}

#[test]
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
    let los = at(&|r| is_leaf(r, "los", "e1"));
    let grp = at(&|r| is_leaf(r, "grp", "a2"));
    let hash = at(&|r| r["ev"] == "hash");
    let gdt = at(&|r| r["f"] == "gdt");
    let pass = at(&|r| r["at"] == "pass");
    let end = at(&|r| r["at"] == "end");
    let wake = at(&|r| r["f"] == "wake");
    let q = at(&|r| r["ev"] == "mem_q");
    let removal = at(&|r| r["ev"] == "mem_remove");
    let unit = at(&|r| is_leaf(r, "unit", "x9"));
    let ingame = at(&|r| is_leaf(r, "ingame", "a2"));
    let tick2 = at(&|r| r["ev"] == "mem_tick" && r["t"] == 1);
    let near = at(&|r| r["f"] == "near");
    let cases: Vec<(&str, Damage)> = vec![
        ("line of sight flipped", flip(los, "v")),
        ("an own unit taken for another group's", Box::new(move |v| v[grp]["v"] = json!("E"))),
        ("a friend's hash on its frame", Box::new(move |v| v[hash]["h"] = json!(392_888_324))),
        ("a long frame", Box::new(move |v| v[gdt]["v"] = json!("0.5"))),
        ("a pass's detect time", Box::new(move |v| v[pass]["slots"][0][1] = json!(GT + 1))),
        ("a pass's free slot", Box::new(move |v| v[pass]["slots"][1] = json!(["x9", 0, false]))),
        ("the queue's addition not recorded", Box::new(move |v| drop(v.remove(end)))),
        ("Wake to another value", Box::new(move |v| v[wake]["a"] = json!("-1"))),
        ("a queued flag flipped", flip(q, "queued")),
        ("a removal not recorded", Box::new(move |v| drop(v.remove(removal)))),
        ("a gone unit resolved", Box::new(move |v| v[unit]["v"] = json!("x9"))),
        ("a unit out of the game", flip(ingame, "v")),
        ("the next tick's queue", Box::new(move |v| v[tick2]["q"] = json!([]))),
        ("the next tick's frame", Box::new(move |v| v[tick2]["frame"] = json!(103))),
        (
            "near units reordered",
            Box::new(move |v| {
                v[near]["v"] = json!(["a1", "a2", "e1", "f1"]);
            }),
        ),
        ("a leaf's extra field", Box::new(move |v| v[los]["x"] = json!(1))),
        (
            "an end GameTime",
            Box::new(move |v| {
                let i = v.iter().position(|r| r["ev"] == "mem_tick_end").unwrap();
                v[i]["gt"] = json!(GT + 5);
            }),
        ),
        ("not the memory observer", Box::new(|v| v[0]["observers"] = json!([]))),
    ];
    for (what, damage) in cases {
        let mut d = v.clone();
        damage(&mut d);
        assert!(!errors_of(&d).is_empty(), "{what}: not caught");
    }
}

/// A record's short description, for reports.
fn describe(r: &Value) -> String {
    if r["k"] == "tick" {
        return format!("the next frame ({})", r["t"]);
    }
    let mut s = r["ev"].as_str().unwrap_or("?").to_owned();
    for k in ["f", "at", "g", "u"] {
        if let Some(v) = r.get(k).filter(|v| !v.is_null()) {
            s.push_str(&format!(" {k}={v}"));
        }
    }
    s
}
