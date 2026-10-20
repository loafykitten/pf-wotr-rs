//! The units' presentation state at the end of every frame of a driven replay (POC-7, "the units
//! driven by the sim", `docs/poc7/fight.md`): where each unit stands, which way it faces, whether
//! it is alive, down or prone, and how fast it moves, as the replay's world holds them when the
//! frame's controllers have run.
//!
//! Collecting reads the world and nothing else: the collector takes `&World`, it is not consulted
//! by any decision of the replay, and the replay's outputs are the same with and without it
//! (`tests/poc7_timeline.rs` runs a capture both ways). A [`Timeline`] exists only for a valid run
//! (`World::valid`: nothing unsupported executed, no RNG failure); an invalid run gives
//! [`Invalid`] with its reasons, because what it computed is not the game's.
//!
//! Each sample says where its position and facing come from ([`Source`]): the sim's movement agent,
//! physics and orientation writers (turn-based combat, once the agents are synchronized), or the
//! recording's observation rows (real time, which the sim doesn't compute).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{Driver, Outcome};
use crate::combat::agent;
use crate::combat::commands::ty;
use crate::combat::oracle::Trace;
use crate::world::{EntityIdx, World};

/// Where a sample's position and orientation come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// The sim's own: turn-based combat with the movement agents synchronized (the agent, the
    /// physics, the paths and the orientation writers are ported).
    Sim,
    /// The recording's: real time (the walk-up before a fight), which the sim doesn't compute, or
    /// a position or orientation the replay wrote from a recorded input in this frame (a
    /// place-on-ground step by another controller, a write by code the sim doesn't run, the
    /// agents' snapshots, a roaming step's recorded vector).
    Recording,
}

/// One unit at the end of one frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// `UnitEntityData.Position` (world, metres).
    pub position: [f32; 3],
    /// `UnitEntityData.Orientation` (degrees about +Y): what the view's rotation is set to.
    pub orientation: f32,
    /// `UnitEntityData.DesiredOrientation`.
    pub desired_orientation: f32,
    /// `EntityDataBase.IsInGame`.
    pub in_game: bool,
    /// `State.LifeState`: 0 conscious, 1 unconscious, 2 dead.
    pub life_state: i32,
    /// `State.Prone.Active`.
    pub prone: bool,
    /// `IsInCombat`.
    pub in_combat: bool,
    /// The movement agent has a path or a requested one (`WantsToMove`, the recording's `mv`).
    pub moving: bool,
    /// Metres per second the unit moved horizontally in this frame (its position against the
    /// previous frame's, over the frame's 1/60 s): the speed it is seen to go. Zero in a unit's
    /// first sample.
    pub speed: f32,
    /// `UnitMovementAgent.m_Speed`: the agent's own speed (meaningful from [`Source::Sim`] samples
    /// of a unit with an agent; zero elsewhere).
    pub agent_speed: f32,
    /// The unit's running command, as an index into [`Timeline::commands`] plus one (0: none or
    /// the commands aren't modelled).
    pub command: u16,
    pub source: Source,
}

/// One unit's samples, one per frame from [`Track::first`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    /// `UniqueId`.
    pub id: String,
    /// Index into [`Timeline::ticks`] of the first sample (a unit that only comes into the game
    /// later has fewer samples).
    pub first: usize,
    pub samples: Vec<Sample>,
}

/// Every frame's end state of the units in the game.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Timeline {
    /// The recording's frame numbers (`tick` records' `t`), in order. The game steps 1/60 s a frame.
    pub ticks: Vec<i64>,
    pub tracks: Vec<Track>,
    /// Command classes (the game's type names) the samples' `command` fields index.
    pub commands: Vec<String>,
}

impl Timeline {
    #[must_use]
    pub fn track(&self, id: &str) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    /// The unit's sample at frame index `frame` (an index into [`Self::ticks`]).
    #[must_use]
    pub fn sample(&self, id: &str, frame: usize) -> Option<&Sample> {
        let t = self.track(id)?;
        t.samples.get(frame.checked_sub(t.first)?)
    }

    /// Writes the timeline as gzip-compressed JSON (a run's timeline exists only if the run is
    /// valid, so a file holds a valid run's).
    ///
    /// # Errors
    ///
    /// I/O, or a timeline that is not consistent ([`Self::check`]).
    pub fn write(&self, path: &Path) -> Result<()> {
        use std::io::Write as _;
        self.check()?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let file =
            std::fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
        let mut z = flate2::write::GzEncoder::new(
            std::io::BufWriter::new(file),
            flate2::Compression::fast(),
        );
        serde_json::to_writer(&mut z, &File { schema: SCHEMA.to_owned(), timeline: self.clone() })?;
        z.finish()?.flush()?;
        Ok(())
    }

    /// Reads a file [`Self::write`] made. The file is not trusted: the decompressed JSON is
    /// limited to [`MAX_BYTES`] before it is parsed, and the timeline must pass [`Self::check`].
    ///
    /// # Errors
    ///
    /// I/O, malformed JSON, more than the limits allow, another schema or an inconsistent timeline.
    pub fn read(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
        let unzipped = flate2::read::GzDecoder::new(std::io::BufReader::new(file));
        let f: File = serde_json::from_reader(std::io::BufReader::new(Limited {
            inner: unzipped,
            left: MAX_BYTES,
        }))
        .with_context(|| format!("parse {}", path.display()))?;
        ensure!(f.schema == SCHEMA, "{}: schema {} (want {SCHEMA})", path.display(), f.schema);
        f.timeline.check().with_context(|| path.display().to_string())?;
        Ok(f.timeline)
    }

    /// Whether the frames, tracks and command indices fit together: at most [`MAX_FRAMES`] frames,
    /// [`MAX_TRACKS`] tracks and [`MAX_SAMPLES`] samples; the frames consecutive (the game steps
    /// one per 1/60 s, and a viewer plays them by index); each track non-empty, inside the frames
    /// and finite; every command index in the table.
    ///
    /// # Errors
    ///
    /// The first thing that doesn't fit.
    pub fn check(&self) -> Result<()> {
        ensure!(
            self.ticks.len() <= MAX_FRAMES,
            "{} frames (at most {MAX_FRAMES})",
            self.ticks.len()
        );
        ensure!(
            self.tracks.len() <= MAX_TRACKS,
            "{} tracks (at most {MAX_TRACKS})",
            self.tracks.len()
        );
        ensure!(
            self.commands.len() < usize::from(u16::MAX),
            "{} command classes",
            self.commands.len()
        );
        ensure!(
            self.ticks.windows(2).all(|w| w[0].checked_add(1) == Some(w[1])),
            "the frames are not consecutive"
        );
        let mut samples = 0usize;
        for t in &self.tracks {
            ensure!(!t.samples.is_empty(), "{}: a track without samples", t.id);
            let end = t.first.checked_add(t.samples.len());
            ensure!(
                end.is_some_and(|e| e <= self.ticks.len()),
                "{}: samples outside the frames",
                t.id
            );
            samples += t.samples.len();
            ensure!(samples <= MAX_SAMPLES, "more than {MAX_SAMPLES} samples");
            for s in &t.samples {
                let finite = s
                    .position
                    .iter()
                    .chain([&s.orientation, &s.desired_orientation, &s.speed, &s.agent_speed])
                    .all(|x| x.is_finite());
                ensure!(finite, "{}: a sample that is not finite", t.id);
                ensure!(
                    usize::from(s.command) <= self.commands.len(),
                    "{}: a command out of range",
                    t.id
                );
            }
        }
        Ok(())
    }

    /// The command class of a sample, if it has one.
    #[must_use]
    pub fn command(&self, s: &Sample) -> Option<&str> {
        usize::from(s.command).checked_sub(1).and_then(|i| self.commands.get(i)).map(String::as_str)
    }
}

/// Schema tag of a timeline file.
pub const SCHEMA: &str = "wr-sim/timeline/1";
/// What a timeline file may hold: decompressed bytes (a 3 hour fight of 21 units is about 0.3
/// GiB), frames (19 hours), tracks and samples.
pub const MAX_BYTES: u64 = 1 << 30;
pub const MAX_FRAMES: usize = 4_000_000;
pub const MAX_TRACKS: usize = 65_536;
pub const MAX_SAMPLES: usize = 8_000_000;

/// A reader that fails once `left` bytes have been read.
struct Limited<R> {
    inner: R,
    left: u64,
}

impl<R: std::io::Read> std::io::Read for Limited<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.left = self.left.checked_sub(n as u64).ok_or_else(|| {
            std::io::Error::other(format!("more than {MAX_BYTES} bytes after decompression"))
        })?;
        Ok(n)
    }
}

#[derive(Serialize, Deserialize)]
struct File {
    schema: String,
    timeline: Timeline,
}

/// Why a run has no timeline: it is not valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    /// What the replay reported as unsupported (the keys of `Coverage::unsupported`).
    pub unsupported: Vec<String>,
    pub rng_failures: usize,
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the run is not valid ({} unsupported kinds, {} RNG failures): {}",
            self.unsupported.len(),
            self.rng_failures,
            self.unsupported.iter().take(3).cloned().collect::<Vec<_>>().join("; ")
        )
    }
}

impl std::error::Error for Invalid {}

/// What the driver collects while it runs.
#[derive(Debug, Default)]
pub(super) struct Collector {
    out: Timeline,
    /// The unit entities, refreshed when the world gains entities.
    units: Vec<EntityIdx>,
    scanned: usize,
    /// Track of each unit entity.
    track_of: HashMap<EntityIdx, usize>,
    class_of: HashMap<String, u16>,
    /// The units whose position or orientation the replay wrote from a recorded input in the
    /// frame so far (cleared at each sample).
    taken: HashSet<EntityIdx>,
}

impl Collector {
    fn refresh(&mut self, w: &World) {
        if self.scanned == w.entities.len() {
            return;
        }
        self.units = (0..w.entities.len())
            .filter_map(|i| {
                let e = EntityIdx(u32::try_from(i).ok()?);
                w.unit(e).map(|_| e)
            })
            .collect();
        self.scanned = w.entities.len();
    }

    fn class(&mut self, name: &str) -> u16 {
        if let Some(&c) = self.class_of.get(name) {
            return c;
        }
        self.out.commands.push(name.to_owned());
        let c = u16::try_from(self.out.commands.len()).unwrap_or(u16::MAX);
        self.class_of.insert(name.to_owned(), c);
        c
    }

    /// The end of frame `tick`: one sample per unit in the game.
    fn sample(&mut self, w: &World, tick: i64, source: Source) {
        self.refresh(w);
        let frame = self.out.ticks.len();
        self.out.ticks.push(tick);
        let taken = std::mem::take(&mut self.taken);
        for &e in &self.units.clone() {
            let ent = w.entity(e);
            let Some(u) = w.unit(e) else { continue };
            let ti = match self.track_of.get(&e) {
                Some(&ti) => ti,
                None if ent.is_in_game => {
                    self.out.tracks.push(Track {
                        id: ent.unique_id.clone(),
                        first: frame,
                        samples: Vec::new(),
                    });
                    self.track_of.insert(e, self.out.tracks.len() - 1);
                    self.out.tracks.len() - 1
                }
                None => continue,
            };
            // `Commands.GetCommand(Move) ?? Commands.Standard`, what the animation controller reads.
            let command = if w.combat.commands.on {
                let cmds = &w.combat.commands;
                cmds.set_of(&ent.unique_id)
                    .and_then(|s| {
                        s.slots[usize::from(ty::MOVE)].or(s.slots[usize::from(ty::STANDARD)])
                    })
                    .and_then(|c| cmds.get(c))
                    .map(|c| c.class.clone())
            } else {
                None
            };
            let command = command.map_or(0, |c| self.class(&c));
            let track = &mut self.out.tracks[ti];
            let speed = track.samples.last().map_or(0.0, |p| {
                let (dx, dz) = (u.position[0] - p.position[0], u.position[2] - p.position[2]);
                dx.hypot(dz) * 60.0
            });
            track.samples.push(Sample {
                position: u.position,
                orientation: u.orientation,
                desired_orientation: u.combat.desired_orientation,
                in_game: ent.is_in_game,
                life_state: u.life_state,
                prone: u.prone_active,
                in_combat: u.combat.in_combat,
                moving: agent::wants_to_move(w, e).unwrap_or(false),
                speed,
                agent_speed: u.combat.agent.speed,
                command,
                source: if taken.contains(&e) { Source::Recording } else { source },
            });
        }
    }
}

impl Driver<'_> {
    /// The replay wrote unit `u`'s position or orientation from a recorded input: its sample of
    /// this frame is the recording's. Reads nothing, changes nothing the replay uses.
    pub(super) fn timeline_taken(&mut self, u: EntityIdx) {
        if let Some(c) = self.timeline.as_mut() {
            c.taken.insert(u);
        }
    }

    /// A frame's end (when the next `tick` record arrives, or the recording ends): the units'
    /// state in it. Reads the world only.
    pub(super) fn timeline_sample(&mut self, w: &World) {
        if self.frame_at.is_none() {
            return; // before the first frame
        }
        let source =
            if w.combat.turn_based && self.ags.synced { Source::Sim } else { Source::Recording };
        let tick = self.tick;
        if let Some(c) = self.timeline.as_mut() {
            c.sample(w, tick, source);
        }
    }
}

/// [`super::run`] collecting the units' timeline. The outcome is the replay's, unchanged; the
/// timeline is there only when the run is valid and the sim computes or takes the units' positions
/// from the recording's agent records.
pub fn run_timeline(w: &mut World, trace: &Trace) -> (Outcome, Result<Timeline, Invalid>) {
    let mut d = Driver::start(w, trace, trace);
    d.timeline = Some(Collector::default());
    while d.i < trace.records.len() {
        let i = d.i;
        d.i = d.record(w, i);
    }
    let agents = d.ag;
    let (out, collected) = d.finish(w);
    (out, conclude(w, collected, agents))
}

/// The timeline of a finished run, or why there is none: the run is not valid, or the replay
/// didn't drive the movement agents (a recording without the `agent` observer and its
/// synchronization: its observation rows are kept apart from the units, so their positions are
/// the save's and a timeline of them would be wrong).
fn conclude(w: &World, collected: Option<Collector>, agents: bool) -> Result<Timeline, Invalid> {
    let mut unsupported: Vec<String> = w.coverage.unsupported.keys().cloned().collect();
    if !agents {
        unsupported.push(
            "the recording has no movement agent records (observer `agent` with `agent_sync`): \
             the units' positions are neither computed nor taken"
                .to_owned(),
        );
    }
    if w.valid() && agents {
        Ok(collected.map(|c| c.out).unwrap_or_default())
    } else {
        Err(Invalid { unsupported, rng_failures: w.rng.failures().len() })
    }
}

/// [`super::run_fixture`] collecting the units' timeline.
///
/// # Errors
///
/// A missing save, content or capture file.
pub fn run_fixture_timeline(
    save: &str,
    dir: &std::path::Path,
    run_name: &str,
) -> anyhow::Result<(Outcome, World, Result<Timeline, Invalid>)> {
    let (mut w, trace) = super::prepare_world(save, dir, run_name)?;
    let load_unsupported = w.coverage.unsupported.len();
    let (mut out, timeline) = run_timeline(&mut w, &trace);
    out.load_unsupported = load_unsupported;
    Ok((out, w, timeline))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::combat::oracle::Rv;
    use crate::stats::tests::world_with_unit;

    fn trace(frames: i64) -> Trace {
        let mut records = vec![json!({"k": "begin", "mode": "record", "observers": []})];
        for t in 0..frames {
            records.push(json!({"k": "tick", "t": t, "mode": super::super::mode::DEFAULT}));
            records.push(json!({"k": "time", "t": t, "game_time": t, "real_time": t,
                "turn_based": false}));
        }
        for (n, r) in records.iter_mut().enumerate() {
            r["seq"] = json!(n);
        }
        Trace { records: records.iter().map(Rv::from_json).collect(), sites: Default::default() }
    }

    fn world() -> (World, EntityIdx) {
        let (mut w, u) = world_with_unit();
        w.player.area_units.push(u);
        let unit = w.unit_mut(u).expect("unit");
        unit.position = [1.0, 2.0, 3.0];
        unit.orientation = 90.0;
        unit.combat.desired_orientation = 45.0;
        (w, u)
    }

    /// The collector's samples are the world's state at each frame's end, and a run without
    /// the movement agent has no timeline however the world looks (its observation rows aren't in
    /// the units, so the positions are stale).
    #[test]
    fn samples_are_the_worlds_and_a_run_without_agents_has_no_timeline() {
        let (mut w, u) = world();
        let id = w.entity(u).unique_id.clone();
        let mut c = Collector::default();
        for t in 0..3 {
            c.sample(&w, t, Source::Sim);
        }
        let tl = conclude(&w, Some(c), true).expect("a valid run with agents");
        assert_eq!(tl.ticks, [0, 1, 2]);
        let t = tl.track(&id).expect("the unit's track");
        assert_eq!((t.first, t.samples.len()), (0, 3));
        let s = tl.sample(&id, 2).expect("last frame");
        assert_eq!(s.position, [1.0, 2.0, 3.0]);
        assert_eq!((s.orientation, s.desired_orientation, s.speed), (90.0, 45.0, 0.0));
        assert_eq!((s.life_state, s.prone, s.in_game, s.source), (0, false, true, Source::Sim));
        assert!(tl.sample(&id, 3).is_none() && tl.sample("nobody", 0).is_none());

        // The same recording through the replay: no agent records, so no timeline; and the replay's
        // own outputs are those of `run`.
        let (out, tl) = run_timeline(&mut w, &trace(3));
        let e = tl.expect_err("no agent records");
        assert!(e.unsupported.iter().any(|k| k.contains("movement agent")), "{e}");
        let (mut w2, _) = world();
        let plain = super::super::run(&mut w2, &trace(3));
        assert_eq!(format!("{:?}", plain.diff), format!("{:?}", out.diff));
        assert_eq!((plain.sim, plain.oracle), (out.sim, out.oracle));
        assert_eq!(
            w.coverage.unsupported.keys().collect::<Vec<_>>(),
            w2.coverage.unsupported.keys().collect::<Vec<_>>()
        );
    }

    /// A unit whose position or orientation was written from a recorded input in a frame is the
    /// recording's in that frame, and only that unit and frame.
    #[test]
    fn a_unit_written_from_a_recorded_input_is_the_recordings_for_that_frame() {
        let (w, u) = world();
        let id = w.entity(u).unique_id.clone();
        let mut c = Collector::default();
        c.sample(&w, 0, Source::Sim);
        c.taken.insert(u);
        c.sample(&w, 1, Source::Sim);
        c.sample(&w, 2, Source::Sim);
        // (A frame the whole replay takes stays the recording's.)
        c.sample(&w, 3, Source::Recording);
        let t = c.out.track(&id).expect("track");
        let src: Vec<Source> = t.samples.iter().map(|s| s.source).collect();
        assert_eq!(src, [Source::Sim, Source::Recording, Source::Sim, Source::Recording]);
    }

    /// A unit's speed is its horizontal displacement between frames, 60 to the second.
    #[test]
    fn speed_is_the_displacement_per_frame() {
        let (w, u) = world();
        let mut c = Collector::default();
        c.sample(&w, 0, Source::Recording);
        let mut w = w;
        w.unit_mut(u).expect("unit").position = [4.0, 9.0, 7.0];
        c.sample(&w, 1, Source::Sim);
        let id = w.entity(u).unique_id.clone();
        let t = c.out.track(&id).expect("track");
        assert_eq!(t.samples[0].speed, 0.0);
        // 3 and 4 metres in the plane (the 7 metres up don't count): 5 m in 1/60 s.
        assert!((t.samples[1].speed - 300.0).abs() < 1e-3, "{}", t.samples[1].speed);
        assert_eq!(t.samples[1].source, Source::Sim);
    }

    fn timeline() -> Timeline {
        let (w, _) = world();
        let mut c = Collector::default();
        for t in 0..3 {
            c.sample(&w, t, Source::Sim);
        }
        conclude(&w, Some(c), true).expect("valid")
    }

    /// A timeline round-trips through its file.
    #[test]
    fn a_timeline_file_round_trips() {
        let tl = timeline();
        let dir = std::env::temp_dir().join(format!("wr-timeline-test-{}", std::process::id()));
        let path = dir.join("t.json.gz");
        tl.write(&path).expect("write");
        assert_eq!(Timeline::read(&path).expect("read"), tl);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// What `check` refuses, on write and on read: tracks outside the frames (a start near
    /// `usize::MAX` must not overflow), empty tracks, frames that aren't consecutive, numbers
    /// that aren't finite, command indices past the table.
    #[test]
    fn an_inconsistent_timeline_is_refused() {
        let tl = timeline();
        assert!(tl.check().is_ok());
        let bad = |change: &dyn Fn(&mut Timeline)| {
            let mut t = tl.clone();
            change(&mut t);
            t.check().expect_err("refused")
        };
        let e = bad(&|t| t.tracks[0].first = 2);
        assert!(e.to_string().contains("outside the frames"), "{e}");
        let e = bad(&|t| t.tracks[0].first = usize::MAX);
        assert!(e.to_string().contains("outside the frames"), "{e}");
        let e = bad(&|t| {
            t.tracks[0].first = 1;
            t.tracks[0].samples.clear();
        });
        assert!(e.to_string().contains("without samples"), "{e}");
        let e = bad(&|t| t.ticks = vec![0, 60, 61]);
        assert!(e.to_string().contains("consecutive"), "{e}");
        let e = bad(&|t| t.ticks = vec![0, 60]);
        assert!(e.to_string().contains("consecutive"), "{e}");
        let e = bad(&|t| t.ticks = vec![0, 0, 1]);
        assert!(e.to_string().contains("consecutive"), "{e}");
        let e = bad(&|t| t.tracks[0].samples[1].position[0] = f32::NAN);
        assert!(e.to_string().contains("not finite"), "{e}");
        let e = bad(&|t| t.tracks[0].samples[1].command = 1);
        assert!(e.to_string().contains("command"), "{e}");
        // A file is refused the same way, and so is writing one.
        let dir = std::env::temp_dir().join(format!("wr-timeline-bad-{}", std::process::id()));
        let path = dir.join("t.json.gz");
        let mut t = tl.clone();
        t.ticks = vec![0, 60, 61];
        assert!(t.write(&path).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A file that fits the schema but not itself is refused when it is read (the checks aren't
    /// only on the writer's side): frames 0 and 60, and a track that starts at `usize::MAX`.
    #[test]
    fn a_file_is_checked_when_it_is_read() {
        use std::io::Write as _;
        let dir = std::env::temp_dir().join(format!("wr-timeline-raw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let raw = |name: &str, t: &Timeline| {
            let path = dir.join(name);
            let mut z = flate2::write::GzEncoder::new(
                std::fs::File::create(&path).expect("create"),
                flate2::Compression::fast(),
            );
            serde_json::to_writer(&mut z, &File { schema: SCHEMA.to_owned(), timeline: t.clone() })
                .expect("json");
            z.finish().expect("finish").flush().expect("flush");
            path
        };
        let mut gap = timeline();
        gap.ticks = vec![0, 60];
        gap.tracks[0].samples.truncate(2);
        let e = Timeline::read(&raw("gap.json.gz", &gap)).expect_err("gap");
        assert!(format!("{e:#}").contains("consecutive"), "{e:#}");
        let mut far = timeline();
        far.tracks[0].first = usize::MAX;
        let e = Timeline::read(&raw("far.json.gz", &far)).expect_err("far");
        assert!(format!("{e:#}").contains("outside the frames"), "{e:#}");
        let mut other = timeline();
        other.tracks.clear();
        assert!(Timeline::read(&raw("ok.json.gz", &other)).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// The decompressed size is bounded before parsing.
    #[test]
    fn a_reader_stops_at_its_limit() {
        use std::io::Read as _;
        let mut r = Limited { inner: std::io::repeat(b'x'), left: 100 };
        let mut buf = vec![0u8; 64];
        assert_eq!(r.read(&mut buf).expect("first"), 64);
        let e = r.read(&mut buf).expect_err("past the limit");
        assert!(e.to_string().contains("after decompression"), "{e}");
        let mut exact = Limited { inner: &[1u8, 2, 3][..], left: 3 };
        let mut all = Vec::new();
        assert_eq!(exact.read_to_end(&mut all).expect("exactly the limit"), 3);
    }

    /// A run that reported something unsupported (or failed a draw) has no timeline.
    #[test]
    fn an_invalid_run_has_none() {
        let (mut w, _) = world();
        w.coverage.unsupported("a feature the replay can't do");
        let mut c = Collector::default();
        c.sample(&w, 0, Source::Sim);
        let e = conclude(&w, Some(c), true).expect_err("invalid");
        assert!(e.unsupported.iter().any(|k| k.contains("a feature")), "{e}");
        assert!(e.to_string().contains("not valid"));
    }
}
