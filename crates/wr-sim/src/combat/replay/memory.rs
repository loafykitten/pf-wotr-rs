//! The driven replay's side of the group memory (docs/poc5/memory.md, H2): with WrathOracle's
//! `memory` observer, each group's memory is the sim's (`groups::Group::mem`), loaded from the
//! window's `mem_init` and kept by the sim: `UnitMemoryController.Tick` runs in its phase
//! ([`Phase::Memory`], placed by its `mem_tick` record) with the reads the sim makes itself checked
//! against their `mem_v` leaves and the rest (the ready groups, the units in vision range,
//! `Passive`, line of sight, stealth, the invisibility loop, `AwakeTimer`) taken from them; the
//! memory after each group's pass (`mem_s` `pass`) and at the tick's end (`end`) is checked, and
//! the memory's own records of its changes (`mem_add`, `mem_remove`, `mem_clear`) against the
//! sim's, in order, inside the tick and between ticks. Between ticks the memory changes only as the
//! sim changes it (its handlers, `Remove` at a destruction, `Clear` at a disposal, the caches its
//! readers fill, `UnitReturnToConsciousController`'s read among them): each `pre` row is checked;
//! a cache filled by a reader the sim doesn't run is taken only when it's the sim's own fill.

use std::collections::HashMap;

use serde_json::Value as J;

use super::commands::describe;
use super::{Driver, Phase};
use crate::combat::commands::Desync;
use crate::combat::groups::{self, GroupIdx};
use crate::combat::memory::sim::{self, group_key};
use crate::combat::memory::{
    Change, GroupMemory, MemError, MemoryController, MemoryLayout, MemoryWorld, Resolved, UnitInfo,
    tick,
};
use crate::csharp::{RawSlot, TimeSpan};
use crate::world::{EntityIdx, World};

/// The driver's memory state.
#[derive(Debug, Default)]
pub(super) struct MemDriver {
    /// The memory is the sim's (the `memory` observer, with the impatience, command and turn
    /// records).
    pub on: bool,
    /// A replay's pinned `Time.frameCount` is its own plus this (WrathOracle's `WallClock`);
    /// `None` until a replay replayed on its own trace shows it.
    pub shift: Option<i64>,
    /// A replay run (its `hash` records say whether the value was the tape's), and the record
    /// run's hashes when its trace is supplied.
    pub replay: bool,
    pub tape: Option<HashMap<String, i64>>,
    /// The run's identity hashes (`hash`, on first use), by unit id.
    hashes: HashMap<String, i32>,
    /// Each group's memory as last recorded (`mem_init`, `mem_s`), by group id (as WrathOracle's
    /// `LastMem`: it outlives a group disposed and made again).
    last: HashMap<String, MemoryLayout<EntityIdx>>,
    /// The memory's changes recorded since the last comparison (between ticks).
    between: Changes,
    /// `mem_initq` records seen.
    initq: usize,
    /// A tick ran (the window's initial state is complete).
    started: bool,
    /// The frame of the last tick.
    tick_frame: Option<i64>,
}

/// The memory's changes as its records give them: (group id, change).
type Changes = Vec<(String, Change<EntityIdx>)>;

/// Exactly these fields besides the envelope (`seq`, `k`, `t`, `ev`, and `f` for a leaf).
fn has_fields(r: &J, payload: &[&str]) -> bool {
    let base: &[&str] =
        if r["ev"] == "mem_v" { &["seq", "k", "t", "ev", "f"] } else { &["seq", "k", "t", "ev"] };
    r.as_object().is_some_and(|o| {
        o.len() == base.len() + payload.len()
            && base.iter().chain(payload).all(|k| o.contains_key(*k))
    })
}

const ROW: [&str; 7] = ["at", "g", "slots", "free", "nfree", "list", "enemies"];
const INIT: [&str; 6] = ["g", "slots", "free", "nfree", "list", "enemies"];

fn obs<'r>(r: &'r J, ev: &str) -> Option<&'r J> {
    (r["k"] == "obs" && r["ev"] == ev).then_some(r)
}

/// A unit the sim has, by id.
fn unit(w: &World, v: &J) -> Option<EntityIdx> {
    let e = *w.by_id.get(v.as_str()?)?;
    w.unit(e).is_some().then_some(e)
}

/// A group's id as the memory records name it (`UnitGroupMemory.GroupId`).
fn group_id(w: &World, g: GroupIdx) -> String {
    w.combat.groups.get(g).id.clone().unwrap_or_else(|| "~fake".into())
}

/// The sim's changes since the last call, by group id.
fn sim_changes(w: &mut World) -> Result<Changes, Desync> {
    let c = sim::take_changes(w).map_err(Desync)?;
    Ok(c.into_iter().map(|(g, x)| (group_id(w, g), x)).collect())
}

/// A memory change record (`mem_add`, `mem_remove`, `mem_clear`) of its writer's shape.
fn change_of(w: &World, r: &J) -> Option<(String, Change<EntityIdx>)> {
    let g = r["group"].as_str()?.to_owned();
    let c = match r["ev"].as_str()? {
        "mem_add" if has_fields(r, &["group", "u", "by"]) && r["by"].is_string() => {
            Change::Added(unit(w, &r["u"])?)
        }
        "mem_remove" if has_fields(r, &["group", "u", "by"]) => match r["by"].as_str()? {
            "Remove" => Change::Removed(unit(w, &r["u"])?),
            "Cleanup" => Change::Cleaned(unit(w, &r["u"])?),
            _ => return None,
        },
        "mem_clear" if has_fields(r, &["group", "n"]) => {
            Change::Cleared(usize::try_from(r["n"].as_u64().filter(|n| *n > 0)?).ok()?)
        }
        _ => return None,
    };
    Some((g, c))
}

/// A memory record's group (a live group), its id and its layout, in the sim's units, checked as
/// recorded to be a dictionary's, then as the sim holds it.
fn layout_of(w: &World, r: &J) -> Result<(GroupIdx, String, MemoryLayout<EntityIdx>), String> {
    let id = r["g"].as_str().ok_or("a memory record without its group")?;
    let g = w
        .combat
        .groups
        .find(id)
        .ok_or_else(|| format!("a memory of a group the sim doesn't have ({id})"))?;
    let units = |v: &J| -> Result<Vec<EntityIdx>, String> {
        v.as_array()
            .ok_or("a cache that isn't a list")?
            .iter()
            .map(|x| {
                unit(w, x).ok_or_else(|| format!("a remembered unit the sim doesn't have ({x})"))
            })
            .collect()
    };
    let mut slots = Vec::new();
    for s in r["slots"].as_array().ok_or("a memory without its slots")? {
        match s.as_array().map(Vec::as_slice) {
            Some([J::Null, next]) => {
                let next = next.as_i64().and_then(|n| i32::try_from(n).ok());
                slots.push(RawSlot::Free(next.ok_or("a free slot it can't read")?));
            }
            Some([k, ticks, vis]) => {
                let k = unit(w, k)
                    .ok_or_else(|| format!("a remembered unit the sim doesn't have ({k})"))?;
                let (Some(ticks), Some(visible)) = (ticks.as_i64(), vis.as_bool()) else {
                    return Err("a slot without its detect time or visibility".into());
                };
                slots.push(RawSlot::Used(
                    k,
                    UnitInfo { last_detect_time: TimeSpan(ticks), visible },
                ));
            }
            _ => return Err("a slot it can't read".into()),
        }
    }
    let free = r["free"].as_i64().and_then(|n| i32::try_from(n).ok());
    let nfree = r["nfree"].as_u64().and_then(|n| usize::try_from(n).ok());
    let (Some(free), Some(nfree)) = (free, nfree) else {
        return Err("a memory without its free list".into());
    };
    let cache = |v: &J| if v.is_null() { Ok(None) } else { units(v).map(Some) };
    let raw = MemoryLayout {
        slots,
        free_list: free,
        free_count: nfree,
        units_list: cache(&r["list"])?,
        enemies: cache(&r["enemies"])?,
    };
    let m = GroupMemory::from_layout(raw).ok_or("a memory that isn't a dictionary's layout")?;
    Ok((g, id.to_owned(), m.layout()))
}

/// A memory the sim takes as recorded (its changes logged from now on).
fn adopt(w: &mut World, g: GroupIdx, l: MemoryLayout<EntityIdx>) {
    w.combat.groups.get_mut(g).mem = GroupMemory::from_layout(l).map(sim::logged);
}

/// A float written as C#'s round-trip string.
fn float_of(r: &J, k: &str) -> Option<f32> {
    r[k].as_str()?.parse::<f32>().ok()
}

fn fresh() -> MemoryLayout<EntityIdx> {
    GroupMemory::new().layout()
}

impl Driver<'_> {
    /// A memory record outside the controller's tick (`mem_init`, `mem_initq`, `hash`, the
    /// memory's changes) or the tick's entry (`mem_tick`, which runs the tick).
    pub(super) fn mem_record(&mut self, w: &mut World, i: usize) -> usize {
        let trace = self.trace;
        let r = &trace.records[i];
        let ev = r["ev"].as_str().unwrap_or("");
        if r["t"].as_i64() != Some(self.tick) {
            w.coverage
                .unsupported(format!("driven replay: memory: a {ev} record of another frame"));
            return i + 1;
        }
        match ev {
            "mem_init" => self.mem_init(w, r),
            "mem_initq" => self.mem_initq(w, r),
            "mem_tick" => return self.mem_tick(w, i),
            "hash" => self.mem_hash(w, r),
            // A change outside the tick (a destruction's `Remove`, a disposal's `Clear`):
            // matched with the sim's at the next tick's entry.
            "mem_add" | "mem_remove" | "mem_clear" if self.mem.started || ev == "mem_clear" => {
                match change_of(w, r) {
                    Some(c) => self.mem.between.push(c),
                    None => w
                        .coverage
                        .unsupported("driven replay: memory: a memory change it can't read"),
                }
            }
            _ => w.coverage.unsupported(format!(
                "driven replay: memory: a {ev} record outside the memory controller's tick"
            )),
        }
        i + 1
    }

    /// The memory's changes between ticks: the recorded ones are the sim's, in order.
    fn mem_between(&mut self, w: &mut World) {
        let model = match sim_changes(w) {
            Ok(m) => m,
            Err(Desync(d)) => {
                w.coverage.unsupported(format!("driven replay: memory: {d}"));
                Vec::new()
            }
        };
        let rec = std::mem::take(&mut self.mem.between);
        self.out.mem_changes += rec.len();
        if rec != model {
            if std::env::var_os("WR_DEBUG_MEMORY").is_some() {
                eprintln!(
                    "DEBUG memory tick {} between: sim {model:?}, recording {rec:?}",
                    self.tick
                );
            }
            w.coverage.unsupported(
                "driven replay: memory: the memory's changes between ticks differ from the sim's",
            );
        }
    }

    /// `hash`: a unit's identity hash (WrathOracle records it once a window, on first use; in a
    /// replay with whether it was the record run's, checked against that run's when given).
    fn mem_hash(&mut self, w: &mut World, r: &J) {
        let shape: &[&str] = if self.mem.replay { &["u", "h", "tape"] } else { &["u", "h"] };
        let (true, Some(u), Some(h)) = (
            has_fields(r, shape),
            r["u"].as_str(),
            r["h"].as_i64().and_then(|h| i32::try_from(h).ok()),
        ) else {
            w.coverage.unsupported("driven replay: memory: a hash record it can't read");
            return;
        };
        if self.mem.replay {
            let taped = r["tape"].as_bool();
            let ok = match &self.mem.tape {
                Some(tape) => {
                    let t = tape.get(u);
                    taped == Some(t.is_some()) && t.is_none_or(|v| *v == i64::from(h))
                }
                None => taped.is_some(),
            };
            if !ok {
                w.coverage.unsupported(
                    "driven replay: memory: a replay's hash differs from the record run's tape",
                );
            }
        }
        if self.mem.hashes.insert(u.to_owned(), h).is_some() {
            w.coverage.unsupported("driven replay: memory: a unit's hash recorded twice");
        }
    }

    /// `mem_init`: a group's memory at the window's start, the sim's from now on.
    fn mem_init(&mut self, w: &mut World, r: &J) {
        if self.mem.started || !has_fields(r, &INIT) {
            w.coverage
                .unsupported("driven replay: memory: a mem_init out of place or of another shape");
            return;
        }
        if r["g"].as_str().is_some_and(|g| g.starts_with("~fake:")) {
            w.coverage.unsupported("driven replay: memory: a fake group's memory holds entries");
            return;
        }
        match layout_of(w, r) {
            Ok((g, id, l)) => {
                if self.mem.last.insert(id, l.clone()).is_some() {
                    w.coverage.unsupported("driven replay: memory: a group's initial memory twice");
                }
                adopt(w, g, l);
                self.out.mem_rows += 1;
            }
            Err(e) => w.coverage.unsupported(format!("driven replay: memory: mem_init: {e}")),
        }
    }

    /// `mem_initq`: the controller's queue and `s_UnitsForRemove`; every live group's memory is
    /// established by now.
    fn mem_initq(&mut self, w: &mut World, r: &J) {
        self.mem.initq += 1;
        let pairs = mem_pairs(w, &r["q"]);
        let pending = mem_units(w, &r["pending"]);
        let (true, false, 1, Some(pairs), Some(pending)) =
            (has_fields(r, &["q", "pending"]), self.mem.started, self.mem.initq, pairs, pending)
        else {
            w.coverage.unsupported(
                "driven replay: memory: a mem_initq out of place, twice or of another shape",
            );
            return;
        };
        w.combat.memory.ctl = MemoryController { units_for_add: pairs, cleanup_pending: pending };
        if w.combat.groups.list.iter().any(|g| w.combat.groups.get(*g).mem.is_none()) {
            w.coverage.unsupported(
                "driven replay: memory: a group without its initial memory (mem_init)",
            );
        }
        // The window's state is the sim's from here: what changed before is in it.
        let _ = sim::take_changes(w);
        self.mem.between.clear();
    }

    /// A `leave_tick` group row's memory (the leave controller's prefix): the sim's dictionary keys
    /// and cached `UnitsList` (one a reader the sim doesn't run filled since taken when it's the
    /// sim's own fill).
    pub(super) fn mem_leave_row(&mut self, w: &mut World, g: GroupIdx, row: &groups::Memory) {
        let Some(m) = w.combat.groups.get(g).mem.as_ref() else {
            w.coverage.unsupported("driven replay: memory: a group without its memory");
            return;
        };
        let keys = sim::ids(w, &m.keys().collect::<Vec<_>>());
        let mut list = m.units_list_cache().map(|l| sim::ids(w, l));
        self.out.mem_rows += 1;
        if keys == row.dict
            && list.is_none()
            && let Some(want) = &row.list
        {
            let want: Option<Vec<EntityIdx>> =
                want.iter().map(|id| w.by_id.get(id).copied()).collect();
            if let Some(want) = want
                && self.mem_fill(w, g, false, &want)
            {
                list = row.list.clone();
            }
        }
        if keys != row.dict || list != row.list {
            if std::env::var_os("WR_DEBUG_MEMORY").is_some() {
                eprintln!(
                    "DEBUG memory tick {} leave row {}: sim {keys:?} {list:?}, recording {:?} {:?}",
                    self.tick,
                    group_key(w, g),
                    row.dict,
                    row.list
                );
            }
            w.coverage.unsupported(
                "driven replay: memory: the leave controller's memory row differs from the sim's",
            );
        }
    }

    /// A frame's end: a Default-mode frame runs the memory controller once.
    pub(super) fn mem_frame_end(&mut self, w: &mut World) {
        if !self.mem.on {
            return;
        }
        if let Some(m) = self.frame.mode
            && super::registered(m, Phase::Memory) == Some(true)
            && self.frame_at.is_some()
            && self.mem.tick_frame != Some(self.tick)
        {
            w.coverage.unsupported(
                "driven replay: a frame without the memory controller's tick (mem_tick)",
            );
        }
    }

    /// The window's end: the changes since the last tick are the sim's.
    pub(super) fn mem_finish(&mut self, w: &mut World) {
        if self.mem.on && self.mem.started {
            self.mem_between(w);
        }
    }

    /// `UnitReturnToConsciousController.Tick` (after the leave controller): out of combat, every
    /// unit of `State.Units` not in combat reads its group's `Memory.Enemies` (`HasAlive`; filled
    /// if invalid). Its own effect (`MakeUnitConscious`) isn't the memory's.
    pub(super) fn mem_return_to_conscious(&mut self, w: &mut World) {
        if !self.mem.on || w.combat.combat_active {
            return;
        }
        for u in crate::scene::units_pool(w) {
            if !crate::scene::enumerated_by_pool(w, u) || crate::combat::is_in_combat(w, u) {
                continue;
            }
            if sim::enemies(w, u).is_none() {
                w.coverage.unsupported(
                    "driven replay: memory: UnitReturnToConsciousController reads a memory the sim doesn't have",
                );
                return;
            }
        }
    }

    /// `mem_tick`: the controller's tick, run by the sim from its own memory, through
    /// `mem_tick_end`. Returns the next record's index.
    fn mem_tick(&mut self, w: &mut World, i: usize) -> usize {
        let trace = self.trace;
        let r = &trace.records[i];
        self.i = i + 1;
        self.place(w, Phase::Memory, "the memory controller");
        if self.mem.tick_frame.replace(self.tick) == Some(self.tick) {
            w.coverage.unsupported("driven replay: two memory controller ticks in a frame");
        }
        if !self.mem.started && self.mem.initq != 1 {
            w.coverage.unsupported(
                "driven replay: memory: the first tick without the window's mem_initq",
            );
        }
        self.mem.started = true;
        self.out.mem_ticks += 1;
        // The changes since the last tick.
        self.mem_between(w);
        let (true, Some(gt), Some(frame), Some(q), Some(pending)) = (
            has_fields(r, &["gt", "frame", "q", "pending"]),
            r["gt"].as_i64(),
            r["frame"].as_i64().and_then(|f| i32::try_from(f).ok()),
            mem_pairs(w, &r["q"]),
            mem_units(w, &r["pending"]),
        ) else {
            w.coverage.unsupported("driven replay: memory: a mem_tick it can't read");
            self.mem_resync(w);
            return self.i;
        };
        // The clocks the tick reads: GameTime and the frame (as WrathOracle pins it).
        if w.clock.game_time.0 != gt {
            w.coverage.unsupported("driven replay: memory: GameTime differs from the tick's");
        }
        let own = self.frame_at.and_then(|f| trace.records[f]["frame"].as_i64());
        if self.mem.shift.is_none()
            && let Some(own) = own
        {
            // (A replay on its own trace: the shift its first tick shows, within WallClock's.)
            let s = i64::from(frame) - own;
            if (0..60).contains(&s) {
                self.mem.shift = Some(s);
                self.out.mem_inputs += 1;
            }
        }
        let pinned = own.zip(self.mem.shift).map(|(f, s)| f + s);
        if pinned != Some(i64::from(frame)) {
            w.coverage.unsupported("driven replay: memory: the frame differs from the tick's");
        }
        // The queue and `s_UnitsForRemove`: the sim's.
        let ctl = &w.combat.memory.ctl;
        if ctl.units_for_add != q || ctl.cleanup_pending != pending {
            w.coverage.unsupported(
                "driven replay: memory: the controller's queue differs from the recording's",
            );
            w.combat.memory.ctl = MemoryController { units_for_add: q, cleanup_pending: pending };
        }
        // The changes since the last record (`pre`), then every memory as last recorded.
        self.mem_pre_rows(w);
        self.mem_all_as_recorded(w, "at the tick's entry");
        let _ = sim::take_changes(w);
        let mut ctl = std::mem::take(&mut w.combat.memory.ctl);
        let mut mw = MemWorld {
            d: self,
            w,
            now: TimeSpan(gt),
            frame,
            changes: Vec::new(),
            model: Vec::new(),
        };
        let res = tick(&mut ctl, &mut mw).and_then(|()| mw.drain()).and_then(|()| mw.compare());
        drop(mw);
        w.combat.memory.ctl = ctl;
        let res = res.and_then(|()| self.mem_tick_end(w, gt));
        if let Err(Desync(d)) = res {
            if std::env::var_os("WR_DEBUG_MEMORY").is_some() {
                eprintln!("DEBUG memory tick {}: {d}", self.tick);
            }
            w.coverage.unsupported(format!("driven replay: memory: {d}"));
            self.mem_resync(w);
        }
        // What the tick changed is matched; the next changes are between ticks.
        let _ = sim::take_changes(w);
        self.i
    }

    /// The `pre` rows after `mem_tick`: each group whose memory changed since its last record. The
    /// memory must be the sim's; a cache the sim holds invalid and the recording valid is a fill by
    /// a reader the sim doesn't run, taken when it's the sim's own fill; anything else is reported
    /// and the recorded memory taken.
    fn mem_pre_rows(&mut self, w: &mut World) {
        let trace = self.trace;
        let mut seen = Vec::new();
        while let Some(r) =
            trace.records.get(self.i).filter(|r| obs(r, "mem_s").is_some() && r["at"] == "pre")
        {
            self.i += 1;
            let parsed = if has_fields(r, &ROW) && r["t"].as_i64() == Some(self.tick) {
                layout_of(w, r)
            } else {
                Err("a pre row of another shape or frame".into())
            };
            let (g, id, row) = match parsed {
                Ok(x) => x,
                Err(e) => {
                    w.coverage.unsupported(format!("driven replay: memory: {e}"));
                    continue;
                }
            };
            let last = self.mem.last.get(&id).cloned().unwrap_or_else(fresh);
            if seen.contains(&id) || row == last {
                w.coverage.unsupported("driven replay: memory: a pre row twice or unchanged");
            }
            seen.push(id.clone());
            self.out.mem_rows += 1;
            let Some(sim) = w.combat.groups.get(g).mem.as_ref().map(GroupMemory::layout) else {
                w.coverage.unsupported("driven replay: memory: a group without its memory");
                continue;
            };
            let mut ok = sim.slots == row.slots
                && sim.free_list == row.free_list
                && sim.free_count == row.free_count;
            for enemies in [true, false] {
                let (s, r) = if enemies {
                    (&sim.enemies, &row.enemies)
                } else {
                    (&sim.units_list, &row.units_list)
                };
                if ok && s != r {
                    ok = match (s, r) {
                        (None, Some(want)) => self.mem_fill(w, g, enemies, want),
                        _ => false,
                    };
                }
            }
            if !ok {
                if std::env::var_os("WR_DEBUG_MEMORY").is_some() {
                    eprintln!(
                        "DEBUG memory tick {} pre {}: sim {sim:?}, recording {row:?}",
                        self.tick,
                        group_key(w, g)
                    );
                }
                w.coverage.unsupported(
                    "driven replay: memory: a memory between ticks differs from the sim's",
                );
                adopt(w, g, row.clone());
            }
            self.mem.last.insert(id, row);
        }
    }

    /// A cache (`Enemies`, or `UnitsList`) the sim holds invalid and the recording valid: a reader
    /// the sim doesn't run filled it (the AI's ally lists, the fog of war, the agent's obstacles,
    /// the UI). Taken when it's the sim's own fill now; anything else is refused.
    fn mem_fill(&mut self, w: &mut World, g: GroupIdx, enemies: bool, want: &[EntityIdx]) -> bool {
        let fill = if enemies {
            sim::enemies_of(w, g)
        } else {
            sim::with_memory(w, g, |w, m| {
                m.units_list_by(|k| sim::resolve(w, k).in_game()).to_vec()
            })
        };
        if fill.as_deref() == Some(want) {
            self.out.mem_fills += 1;
            return true;
        }
        if std::env::var_os("WR_DEBUG_MEMORY").is_some() {
            eprintln!(
                "DEBUG memory tick {} fill {} (enemies {enemies}): sim {fill:?}, recording {want:?}",
                self.tick,
                group_key(w, g)
            );
        }
        false
    }

    /// Every live group's memory is the one last recorded for its id (or a new one's).
    fn mem_all_as_recorded(&mut self, w: &mut World, when: &str) {
        for g in w.combat.groups.list.clone() {
            let last = self.mem.last.get(&group_id(w, g)).cloned().unwrap_or_else(fresh);
            let Some(sim) = w.combat.groups.get(g).mem.as_ref().map(GroupMemory::layout) else {
                w.coverage.unsupported("driven replay: memory: a group without its memory");
                continue;
            };
            if sim != last {
                if std::env::var_os("WR_DEBUG_MEMORY").is_some() {
                    eprintln!(
                        "DEBUG memory tick {} {when} {}: sim {sim:?}, recording {last:?}",
                        self.tick,
                        group_key(w, g)
                    );
                }
                w.coverage.unsupported(format!(
                    "driven replay: memory: a memory differs from the recording's {when}"
                ));
                adopt(w, g, last);
            }
        }
    }

    /// The tick's end: an `end` row for each group the queue changed, then `mem_tick_end`.
    fn mem_tick_end(&mut self, w: &mut World, gt: i64) -> Result<(), Desync> {
        let trace = self.trace;
        let mut seen = Vec::new();
        loop {
            let r = trace
                .records
                .get(self.i)
                .ok_or_else(|| Desync("the recording ends inside the memory tick".into()))?;
            if r["t"].as_i64() != Some(self.tick) || r["k"] != "obs" {
                return Err(Desync(format!("the memory tick ends at {}", describe(r))));
            }
            if obs(r, "mem_s").is_some() && r["at"] == "end" && has_fields(r, &ROW) {
                self.i += 1;
                let (g, id, row) = layout_of(w, r).map_err(Desync)?;
                let last = self.mem.last.get(&id).cloned().unwrap_or_else(fresh);
                if seen.contains(&id) || row == last {
                    return Err(Desync("an end row twice or unchanged".into()));
                }
                seen.push(id.clone());
                self.out.mem_rows += 1;
                let sim = w.combat.groups.get(g).mem.as_ref().map(GroupMemory::layout);
                if sim.as_ref() != Some(&row) {
                    w.coverage.unsupported(
                        "driven replay: memory: a memory at the tick's end differs from the sim's",
                    );
                    adopt(w, g, row.clone());
                }
                self.mem.last.insert(id, row);
                continue;
            }
            if obs(r, "mem_tick_end").is_some() && has_fields(r, &["gt", "exc"]) {
                self.i += 1;
                if r["exc"] != false {
                    w.coverage.unsupported("driven replay: memory: the game's tick threw");
                }
                if r["gt"].as_i64() != Some(gt) {
                    w.coverage
                        .unsupported("driven replay: memory: GameTime changed during the tick");
                }
                break;
            }
            return Err(Desync(format!(
                "the sim's memory tick ends, the recording has {}",
                describe(r)
            )));
        }
        self.mem_all_as_recorded(w, "at the tick's end");
        Ok(())
    }

    /// After a desync: on to the tick's end (its states taken as recorded), leaving the next
    /// frame, tick or window record to the run.
    fn mem_resync(&mut self, w: &mut World) {
        let trace = self.trace;
        while let Some(r) = trace.records.get(self.i) {
            if r["k"] != "obs"
                || matches!(r["ev"].as_str(), Some("mem_tick" | "mem_init" | "mem_initq" | "mem_q"))
                || r["t"].as_i64() != Some(self.tick)
            {
                break;
            }
            self.i += 1;
            if obs(r, "mem_s").is_some() && has_fields(r, &ROW) {
                match layout_of(w, r) {
                    Ok((g, id, l)) => {
                        adopt(w, g, l.clone());
                        self.mem.last.insert(id, l);
                    }
                    Err(e) => w.coverage.unsupported(format!("driven replay: memory: {e}")),
                }
            }
            if obs(r, "hash").is_some() {
                self.mem_hash(w, r);
            }
            if obs(r, "mem_tick_end").is_some() {
                break;
            }
        }
    }
}

/// A queue's pairs (observer, target), in the sim's units.
fn mem_pairs(w: &World, v: &J) -> Option<Vec<(EntityIdx, EntityIdx)>> {
    v.as_array()?
        .iter()
        .map(|p| match p.as_array().map(Vec::as_slice) {
            Some([o, t]) => Some((unit(w, o)?, unit(w, t)?)),
            _ => None,
        })
        .collect()
}

fn mem_units(w: &World, v: &J) -> Option<Vec<EntityIdx>> {
    v.as_array()?.iter().map(|x| unit(w, x)).collect()
}

/// The tick's world: the sim's own reads, each checked against its leaf, and the inputs taken
/// from theirs, pulled strictly as the next records.
struct MemWorld<'a, 't> {
    d: &'a mut Driver<'t>,
    w: &'a mut World,
    now: TimeSpan,
    frame: i32,
    /// The memory's recorded changes since the last comparison, and the sim's.
    changes: Changes,
    model: Changes,
}

impl<'t> MemWorld<'_, 't> {
    /// The recorded changes so far are the sim's, in order.
    fn compare(&mut self) -> Result<(), Desync> {
        let model = sim_changes(self.w)?;
        self.model.extend(model);
        if self.changes != self.model {
            return Err(Desync(format!(
                "the memory's changes differ: the sim's {:?}, the recording's {:?}",
                self.model, self.changes
            )));
        }
        self.d.out.mem_changes += self.changes.len();
        self.changes.clear();
        self.model.clear();
        Ok(())
    }

    /// A record passed on the way to the next one the tick asks for.
    fn on_the_way(&mut self, r: &J) -> Result<(), Desync> {
        if r["t"].as_i64() != Some(self.d.tick) {
            return Err(Desync(format!(
                "a record of another frame inside the tick ({})",
                describe(r)
            )));
        }
        match (r["k"].as_str(), r["ev"].as_str()) {
            (Some("obs"), Some("hash")) => self.d.mem_hash(self.w, r),
            (Some("obs"), Some("mem_add" | "mem_remove")) => {
                let c = change_of(self.w, r).ok_or_else(|| {
                    Desync(format!("a memory change it can't read ({})", describe(r)))
                })?;
                self.changes.push(c);
            }
            // `UnitEntityData.Group` runs `UpdateGroup` (the sim's own, as it reads `Group`).
            (Some("obs"), Some("group_new" | "group_add" | "group_remove" | "group_dispose")) => {}
            // A sight cache's miss inside `group.HasLOS` (the line of sight the `los` leaf gives).
            (Some("obs"), Some("los")) => {}
            _ => {
                return Err(Desync(format!("a record inside the memory tick ({})", describe(r))));
            }
        }
        Ok(())
    }

    /// Up to the next record the tick asks for (a leaf or a state), or one that ends the tick.
    fn drain(&mut self) -> Result<(), Desync> {
        let trace = self.d.trace;
        while let Some(r) = trace.records.get(self.d.i) {
            if obs(r, "mem_v").is_some()
                || obs(r, "mem_s").is_some()
                || obs(r, "mem_tick_end").is_some()
                || r["k"] == "tick"
            {
                return Ok(());
            }
            self.on_the_way(r)?;
            self.d.i += 1;
        }
        Ok(())
    }

    /// The next record the tick asks for, after the changes the recording shows before it are
    /// compared with the sim's.
    fn next(&mut self, what: &str) -> Result<&'t J, Desync> {
        self.drain()?;
        self.compare()?;
        let trace: &'t crate::combat::oracle::Trace = self.d.trace;
        let r = trace
            .records
            .get(self.d.i)
            .ok_or_else(|| Desync(format!("{what}: the recording ends")))?;
        if r["t"].as_i64() != Some(self.d.tick)
            || (obs(r, "mem_v").is_none() && obs(r, "mem_s").is_none())
        {
            return Err(Desync(format!("{what}: the recording has {}", describe(r))));
        }
        self.d.i += 1;
        Ok(r)
    }

    /// The next leaf, of `f`, with exactly its writer's fields.
    fn leaf(&mut self, f: &str, payload: &[&str]) -> Result<&'t J, Desync> {
        let r = self.next(&format!("the sim reads {f}"))?;
        if obs(r, "mem_v").is_none() || r["f"] != f {
            self.d.i -= 1;
            return Err(Desync(format!("the sim reads {f}, the recording has {}", describe(r))));
        }
        if !has_fields(r, payload) {
            return Err(Desync(format!("a leaf {f} with fields its writer doesn't write")));
        }
        Ok(r)
    }

    /// The leaf is about this unit or group (its id, or its key).
    fn about(r: &J, k: &str, want: &str) -> Result<(), Desync> {
        if r[k].as_str() == Some(want) {
            return Ok(());
        }
        Err(Desync(format!("the sim reads {} of {k} {want}, the recording of {}", r["f"], r[k])))
    }

    fn uid(&self, u: EntityIdx) -> String {
        self.w.entity(u).unique_id.clone()
    }

    /// A leaf's boolean.
    fn bool_of(r: &J, k: &str) -> Result<bool, Desync> {
        r[k].as_bool().ok_or_else(|| Desync(format!("a leaf {} it can't read", r["f"])))
    }

    /// A read the sim makes itself: its value checked against the leaf's (a difference
    /// reported); the sim's value is used.
    fn checked<T: PartialEq + std::fmt::Debug>(&mut self, f: &str, sim: T, recorded: &T) -> T {
        self.d.out.mem_reads += 1;
        if sim != *recorded {
            if std::env::var_os("WR_DEBUG_MEMORY").is_some() {
                eprintln!(
                    "DEBUG memory tick {} {f}: sim {sim:?}, recording {recorded:?}",
                    self.d.tick
                );
            }
            self.w.coverage.unsupported(format!(
                "driven replay: memory: the sim's {f} differs from the recording's"
            ));
        }
        sim
    }

    /// A read taken from the recording.
    fn input<T>(&mut self, v: T) -> T {
        self.d.out.mem_inputs += 1;
        v
    }
}

impl From<MemError> for Desync {
    fn from(e: MemError) -> Self {
        Desync(format!("the sim's memory tick threw {e:?}"))
    }
}

impl MemoryWorld<EntityIdx, GroupIdx> for MemWorld<'_, '_> {
    type Error = Desync;

    fn area_peaceful(&mut self) -> Result<bool, Desync> {
        let r = self.leaf("apeace", &["v"])?;
        let v = Self::bool_of(r, "v")?;
        Ok(self.input(v))
    }

    fn ready_groups(&mut self) -> Result<Vec<GroupIdx>, Desync> {
        let r = self.leaf("ready", &["v"])?;
        let groups = r["v"]
            .as_array()
            .ok_or_else(|| Desync("a ready list it can't read".into()))?
            .iter()
            .map(|g| g.as_str().and_then(|id| self.w.combat.groups.find(id)))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| Desync("a ready group the sim doesn't have".into()))?;
        Ok(self.input(groups))
    }

    fn near_units(&mut self, g: GroupIdx) -> Result<Vec<EntityIdx>, Desync> {
        let r = self.leaf("near", &["g", "v"])?;
        Self::about(r, "g", &group_key(self.w, g))?;
        let us = mem_units(self.w, &r["v"])
            .ok_or_else(|| Desync("a near unit the sim doesn't have".into()))?;
        Ok(self.input(us))
    }

    fn faction_peaceful(&mut self, u: EntityIdx) -> Result<bool, Desync> {
        let r = self.leaf("peace", &["u", "v"])?;
        Self::about(r, "u", &self.uid(u))?;
        let rec = Self::bool_of(r, "v")?;
        let Some(sim) = crate::combat::units::faction_peaceful(self.w, u) else {
            return Err(Desync("a near unit without a faction the sim knows".into()));
        };
        Ok(self.checked("peace", sim, &rec))
    }

    fn passive(&mut self, u: EntityIdx) -> Result<bool, Desync> {
        let r = self.leaf("pass", &["u", "v"])?;
        Self::about(r, "u", &self.uid(u))?;
        let v = Self::bool_of(r, "v")?;
        Ok(self.input(v))
    }

    fn group_of(&mut self, u: EntityIdx) -> Result<GroupIdx, Desync> {
        let r = self.leaf("grp", &["u", "v"])?;
        Self::about(r, "u", &self.uid(u))?;
        let g =
            groups::group_of(self.w, u).ok_or_else(|| Desync("a unit without a group".into()))?;
        let rec = r["v"].as_str().unwrap_or_default().to_owned();
        let key = group_key(self.w, g);
        self.checked("grp", key, &rec);
        Ok(g)
    }

    fn is_fake(&mut self, g: GroupIdx) -> bool {
        self.w.combat.groups.get(g).is_fake()
    }

    fn group_by_id(&mut self, u: EntityIdx) -> Result<GroupIdx, Desync> {
        let r = self.leaf("getgrp", &["id", "v"])?;
        let id = crate::combat::units::group_id(self.w, u)
            .ok_or_else(|| Desync("a unit without a group id".into()))?;
        Self::about(r, "id", &id)?;
        let g = groups::get_group(self.w, &id);
        let rec = r["v"].as_str().unwrap_or_default().to_owned();
        let key = group_key(self.w, g);
        self.checked("getgrp", key, &rec);
        Ok(g)
    }

    fn in_stealth_for(&mut self, u: EntityIdx, g: GroupIdx) -> Result<bool, Desync> {
        let r = self.leaf("stl", &["u", "g", "v"])?;
        Self::about(r, "u", &self.uid(u))?;
        Self::about(r, "g", &group_key(self.w, g))?;
        let v = Self::bool_of(r, "v")?;
        Ok(self.input(v))
    }

    fn is_enemy(&mut self, g: GroupIdx, u: EntityIdx) -> Result<bool, Desync> {
        let r = self.leaf("enemy", &["g", "u", "v"])?;
        Self::about(r, "g", &group_key(self.w, g))?;
        Self::about(r, "u", &self.uid(u))?;
        let rec = Self::bool_of(r, "v")?;
        let sim = sim::is_enemy_unit(self.w, g, u)
            .ok_or_else(|| Desync("a unit without a group".into()))?;
        Ok(self.checked("enemy", sim, &rec))
    }

    fn game_delta_time(&mut self) -> Result<f32, Desync> {
        let r = self.leaf("gdt", &["v"])?;
        let rec = float_of(r, "v").ok_or_else(|| Desync("a leaf gdt it can't read".into()))?;
        let sim = self.w.clock.game_delta_time();
        Ok(f32::from_bits(self.checked("gdt", sim.to_bits(), &rec.to_bits())))
    }

    fn frame_count(&mut self) -> Result<i32, Desync> {
        Ok(self.frame)
    }

    fn unit_hash(&mut self, u: EntityIdx) -> Result<i32, Desync> {
        // A first use records the hash as it's read.
        self.drain()?;
        let id = self.uid(u);
        self.d
            .mem
            .hashes
            .get(&id)
            .copied()
            .ok_or_else(|| Desync(format!("{id}'s identity hash read, none recorded")))
    }

    fn group_has_los(&mut self, g: GroupIdx, u: EntityIdx) -> Result<bool, Desync> {
        let r = self.leaf("los", &["g", "u", "v"])?;
        Self::about(r, "g", &group_key(self.w, g))?;
        Self::about(r, "u", &self.uid(u))?;
        let v = Self::bool_of(r, "v")?;
        Ok(self.input(v))
    }

    fn invisible(&mut self, u: EntityIdx) -> Result<bool, Desync> {
        let r = self.leaf("inv", &["u", "c", "v"])?;
        Self::about(r, "u", &self.uid(u))?;
        if r["c"].as_i64() != Some(i64::from(crate::combat::memory::INVISIBLE)) {
            return Err(Desync("a condition other than Invisible".into()));
        }
        let rec = Self::bool_of(r, "v")?;
        let sim = crate::unit_state::has_condition(self.w, u, crate::combat::memory::INVISIBLE);
        Ok(self.checked("inv", sim, &rec))
    }

    fn sees_invisible(&mut self, _g: GroupIdx, u: EntityIdx) -> Result<bool, Desync> {
        let r = self.leaf("seen", &["u", "v"])?;
        Self::about(r, "u", &self.uid(u))?;
        let v = Self::bool_of(r, "v")?;
        Ok(self.input(v))
    }

    fn game_time(&mut self) -> Result<TimeSpan, Desync> {
        Ok(self.now)
    }

    fn memory(&mut self, g: GroupIdx) -> Result<&mut GroupMemory<EntityIdx>, Desync> {
        if self.w.combat.groups.get(g).is_fake() || self.w.combat.groups.get(g).disposed {
            return Err(Desync("a fake or disposed group's memory".into()));
        }
        self.w
            .combat
            .groups
            .get_mut(g)
            .mem
            .as_mut()
            .ok_or_else(|| Desync("a group without its memory".into()))
    }

    fn resolves(&mut self, u: EntityIdx) -> Result<bool, Desync> {
        let r = self.leaf("unit", &["u", "v"])?;
        let id = self.uid(u);
        Self::about(r, "u", &id)?;
        let rec = match &r["v"] {
            J::Null => false,
            v if v.as_str() == Some(id.as_str()) => true,
            _ => return Err(Desync("a reference resolved to another unit".into())),
        };
        let sim = sim::resolve(self.w, u) != Resolved::Missing;
        Ok(self.checked("unit", sim, &rec))
    }

    fn in_game(&mut self, u: EntityIdx) -> Result<bool, Desync> {
        let r = self.leaf("ingame", &["u", "v"])?;
        Self::about(r, "u", &self.uid(u))?;
        let rec = Self::bool_of(r, "v")?;
        let sim = self.w.entity(u).is_in_game;
        Ok(self.checked("ingame", sim, &rec))
    }

    fn disposed(&mut self, u: EntityIdx) -> Result<bool, Desync> {
        let r = self.leaf("disp", &["u", "v"])?;
        Self::about(r, "u", &self.uid(u))?;
        let rec = Self::bool_of(r, "v")?;
        let sim = self.w.entity(u).is_disposed;
        Ok(self.checked("disp", sim, &rec))
    }

    fn wake(&mut self, u: EntityIdx) -> Result<(), Desync> {
        // `AwakeTimer` isn't tracked by the sim (the awake states are the recording's): the call's
        // own arithmetic is checked, `Max(0f, AwakeTimer)`.
        let r = self.leaf("wake", &["u", "time", "fade", "b", "a"])?;
        Self::about(r, "u", &self.uid(u))?;
        if r["time"] != "0" || r["fade"] != false {
            return Err(Desync("Wake with arguments the controller doesn't pass".into()));
        }
        let (Some(b), Some(a)) = (float_of(r, "b"), float_of(r, "a")) else {
            return Err(Desync("a leaf wake it can't read".into()));
        };
        let want = if 0.0 > b { 0.0 } else { b };
        self.checked("wake", want.to_bits(), &a.to_bits());
        self.input(());
        Ok(())
    }

    fn cleaned(&mut self, g: GroupIdx, _removed: &[EntityIdx]) -> Result<(), Desync> {
        let r = self.next("the group's memory after its Cleanup")?;
        if obs(r, "mem_s").is_none() || r["at"] != "pass" || !has_fields(r, &ROW) {
            self.d.i -= 1;
            return Err(Desync(format!(
                "the sim's Cleanup ends, the recording has {}",
                describe(r)
            )));
        }
        let (rg, id, row) = layout_of(self.w, r).map_err(Desync)?;
        if rg != g {
            return Err(Desync("another group's pass".into()));
        }
        self.d.out.mem_rows += 1;
        let sim = self.memory(g)?.layout();
        if sim != row {
            if std::env::var_os("WR_DEBUG_MEMORY").is_some() {
                eprintln!(
                    "DEBUG memory tick {} pass {}: sim {sim:?}, recording {row:?}",
                    self.d.tick,
                    group_key(self.w, g)
                );
            }
            self.w.coverage.unsupported(
                "driven replay: memory: a memory after its pass differs from the sim's",
            );
            adopt(self.w, g, row.clone());
        }
        self.d.mem.last.insert(id, row);
        Ok(())
    }
}
