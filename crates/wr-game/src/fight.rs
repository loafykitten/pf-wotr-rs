//! `--fight`: a fight the sim computes (POC-7, "the units driven by the sim", `docs/poc7/fight.md`).
//!
//! The recording's driven replay runs in this process (`wr_sim::combat::replay::timeline`), and
//! the timeline it collects, every unit's position, facing, life state and speed at each frame,
//! becomes the [`Drive`] the renderer moves the pack's units by. A replay that is not valid gives
//! no timeline and no fight. The replay needs the memory of a capture replay (about 2 GB, freed
//! before the window opens): start it under `scripts/heavy.sh`. A timeline file that
//! `wr-combat replay --timeline-out` wrote plays without replaying, and without the memory.

use std::path::Path;

use anyhow::{Context, Result, bail};
use bevy::math::Vec3;
use wr_sim::combat::replay::timeline::{Source, Timeline};

use crate::owlcat::skinning::{Drive, DrivePose, DriveTrack};
use crate::route::{self, RoutePoint};

/// A fight ready to play.
#[derive(Debug)]
pub struct Fight {
    pub drive: Drive,
}

/// The save a capture was recorded from (`run.json`'s `save_file`, two levels above a fixture
/// directory), without its directory and extension.
fn recorded_save(fixture: &Path) -> Result<String> {
    let run = fixture
        .ancestors()
        .nth(2)
        .map(|d| d.join("run.json"))
        .filter(|p| p.exists())
        .with_context(|| format!("{}: no run.json two levels up", fixture.display()))?;
    let j: serde_json::Value = serde_json::from_slice(&std::fs::read(&run)?)?;
    let file = j["save_file"].as_str().context("run.json has no save_file")?;
    Path::new(file)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .context("run.json: save_file has no name")
}

/// The drive a timeline gives: one pose per unit per frame.
#[must_use]
pub fn to_drive(tl: &Timeline) -> Drive {
    Drive {
        frames: tl.ticks.len(),
        tracks: tl
            .tracks
            .iter()
            .map(|t| DriveTrack {
                unit: t.id.clone(),
                first: t.first,
                poses: t
                    .samples
                    .iter()
                    .map(|s| DrivePose {
                        position: Vec3::from_array(s.position),
                        yaw: s.orientation,
                        speed: s.speed,
                        in_game: s.in_game,
                        up: s.life_state == 0 && !s.prone,
                        in_combat: s.in_combat,
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// Replays `fixture`'s `run` and collects the timeline, or reads the timeline file `fixture` is.
///
/// # Errors
///
/// A missing capture, save or content, or a replay that is not valid; an unreadable file.
pub fn load(fixture: &Path, run: &str, save: Option<&str>) -> Result<Fight> {
    if fixture.is_file() {
        let timeline = Timeline::read(fixture)?;
        eprintln!(
            "timeline {}: {} frames, {} units",
            fixture.display(),
            timeline.ticks.len(),
            timeline.tracks.len()
        );
        return Ok(Fight { drive: to_drive(&timeline) });
    }
    let save = match save {
        Some(s) => s.to_owned(),
        None => recorded_save(fixture)?,
    };
    eprintln!("replaying {} ({run}) from {save} ...", fixture.display());
    let t = std::time::Instant::now();
    let (out, world, timeline) =
        wr_sim::combat::replay::timeline::run_fixture_timeline(&save, fixture, run)?;
    drop(world);
    let timeline = match timeline {
        Ok(t) => t,
        Err(e) => bail!("{e}"),
    };
    let sim = |s: &&wr_sim::combat::replay::timeline::Sample| s.source == Source::Sim;
    let (frames_sim, frames_recorded) = timeline.tracks.first().map_or((0, 0), |t| {
        let n = t.samples.iter().filter(sim).count();
        (n, t.samples.len() - n)
    });
    eprintln!(
        "  {} frames, {} units, {frames_sim} frames the sim's and {frames_recorded} the recording's \
         ({:.1} s; {} roots driven, equal to the recording: {})",
        timeline.ticks.len(),
        timeline.tracks.len(),
        t.elapsed().as_secs_f64(),
        out.roots,
        out.diff.equal
    );
    let frames: Vec<i64> = timeline.ticks.windows(2).map(|w| w[1] - w[0]).collect();
    if frames.iter().any(|d| *d != 1) {
        bail!("{}: the recording's frames are not consecutive", fixture.display());
    }
    Ok(Fight { drive: to_drive(&timeline) })
}

/// How a following camera looks at a fight: it keeps the direction of the camera the original
/// played with (the route's first pose) and backs off until the fighters fit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Follow {
    /// Unit vector, from the target towards the eye.
    back: Vec3,
    /// Tangent of half the vertical field of view.
    half_tan: f32,
    /// The eye and target of the last smoothed pose (live play only).
    state: Option<(Vec3, Vec3)>,
}

/// Metres added to the fighters' radius, and the nearest the camera comes.
const MARGIN: f32 = 4.0;
const NEAREST: f32 = 24.0;
/// Where the camera looks above the fighters' ground.
const EYE_LEVEL: f32 = 1.0;

impl Follow {
    /// From the pose the camera had in the original (its pitch is the game's 43 degrees); `yaw`
    /// (degrees about +Y, the direction it looks in) replaces the pose's own.
    #[must_use]
    pub fn from_pose(base: &RoutePoint, yaw: Option<f32>) -> Self {
        let w2c = base.pose.world_to_camera;
        // Camera space looks down -Z: the world forward is minus the third row.
        let mut forward =
            Vec3::new(-w2c.x_axis.z, -w2c.y_axis.z, -w2c.z_axis.z).normalize_or(Vec3::Z);
        if let Some(yaw) = yaw {
            let flat = forward.x.hypot(forward.z);
            let r = yaw.to_radians();
            forward = Vec3::new(r.sin() * flat, forward.y, r.cos() * flat);
        }
        let cot = base.pose.projection.y_axis.y.abs();
        Self { back: -forward, half_tan: if cot > 1e-3 { 1.0 / cot } else { 0.5 }, state: None }
    }

    /// The eye and target for fighters around `centre` with `radius`.
    #[must_use]
    pub fn frame(&self, centre: Vec3, radius: f32) -> (Vec3, Vec3) {
        let target = centre + Vec3::Y * EYE_LEVEL;
        let dist = ((radius + MARGIN) / self.half_tan).max(NEAREST);
        (target + self.back * dist, target)
    }

    /// The camera pose for the fighters, from `base`'s lens. With `smooth` (live play, seconds
    /// since the last call) the eye and target ease towards the framing; without it they are
    /// exactly the framing, so a capture does not depend on the frames before it.
    pub fn pose(
        &mut self,
        base: &RoutePoint,
        centre: Vec3,
        radius: f32,
        smooth: Option<f32>,
    ) -> RoutePoint {
        let (eye, target) = self.frame(centre, radius);
        let (eye, target) = match (smooth, self.state) {
            (Some(dt), Some((e0, t0))) => {
                let k = 1.0 - (-dt / 0.6).exp();
                (e0.lerp(eye, k), t0.lerp(target, k))
            }
            _ => (eye, target),
        };
        self.state = smooth.map(|_| (eye, target));
        route::look_at(base, eye, target)
    }
}

#[cfg(test)]
mod tests {
    use bevy::math::Mat4;
    use wr_sim::combat::replay::timeline::{Sample, Track};

    use super::*;
    use crate::owlcat::CameraPose;

    fn sample(x: f32, life: i32, prone: bool, source: Source) -> Sample {
        Sample {
            position: [x, 0.0, 0.0],
            orientation: 90.0,
            desired_orientation: 90.0,
            in_game: true,
            life_state: life,
            prone,
            in_combat: true,
            moving: false,
            speed: 2.0,
            agent_speed: 2.0,
            command: 0,
            source,
        }
    }

    fn base() -> RoutePoint {
        // A camera looking down and along +Z with a 60 degree field of view (cot 30 = 1.732).
        let eye = Vec3::new(0.0, 10.0, -10.0);
        let b = RoutePoint {
            sample: 0,
            pose: CameraPose {
                world_to_camera: Mat4::IDENTITY,
                gpu_projection: Mat4::IDENTITY,
                projection: Mat4::from_cols_array(&[
                    1.0,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    3.0_f32.sqrt(),
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    -1.0,
                    -1.0,
                    0.0,
                    0.0,
                    -0.2,
                    0.0,
                ]),
                position: Vec3::ZERO,
                near: 0.3,
                far: 100.0,
                width: 16,
                height: 9,
                rig: None,
            },
        };
        route::look_at(&b, eye, Vec3::ZERO)
    }

    #[test]
    fn a_timeline_becomes_one_pose_per_frame() {
        let tl = Timeline {
            ticks: vec![5, 6, 7],
            tracks: vec![Track {
                id: "u".into(),
                first: 1,
                samples: vec![
                    sample(1.0, 0, false, Source::Sim),
                    sample(2.0, 0, true, Source::Sim),
                    sample(3.0, 2, true, Source::Recording),
                ],
            }],
            commands: Vec::new(),
        };
        let d = to_drive(&tl);
        assert_eq!((d.frames, d.tracks.len(), d.tracks[0].first), (3, 1, 1));
        let p = &d.tracks[0].poses;
        assert_eq!((p[0].position.x, p[0].yaw, p[0].speed), (1.0, 90.0, 2.0));
        // Prone and dead units are not up: the player has no clip for either.
        assert_eq!(p.iter().map(|p| p.up).collect::<Vec<_>>(), [true, false, false]);
    }

    #[test]
    fn the_camera_keeps_its_direction_and_fits_the_fighters() {
        let b = base();
        let f = Follow::from_pose(&b, None);
        // Looking down 45 degrees along +Z: the eye sits behind (-Z) and above the target.
        let (eye, target) = f.frame(Vec3::new(5.0, 0.0, 5.0), 20.0);
        let d = (target - eye).normalize();
        assert!((d - Vec3::new(0.0, -1.0, 1.0).normalize()).length() < 1e-4, "{d:?}");
        // The distance puts a circle of radius + margin inside half the vertical field of view.
        let dist = (target - eye).length();
        assert!((dist - (20.0 + MARGIN) * 3.0_f32.sqrt()).abs() < 1e-3, "{dist}");
        // A small group is not framed closer than the nearest distance.
        let (e2, t2) = f.frame(Vec3::ZERO, 0.0);
        assert!(((t2 - e2).length() - NEAREST).abs() < 1e-4);
        // The pose looks at the target: it is straight ahead of the camera.
        let mut f = f;
        let p = f.pose(&b, Vec3::new(5.0, 0.0, 5.0), 20.0, None);
        // A yaw turns the direction and keeps the pitch.
        let (e, t) = Follow::from_pose(&b, Some(90.0)).frame(Vec3::ZERO, 20.0);
        let d = (t - e).normalize();
        assert!((d - Vec3::new(1.0, -1.0, 0.0).normalize()).length() < 1e-4, "{d:?}");
        let c = p.pose.world_to_camera.transform_point3(target);
        assert!(c.x.abs() < 1e-3 && c.y.abs() < 1e-3 && c.z < 0.0, "{c:?}");
    }

    #[test]
    fn smoothing_eases_and_a_capture_does_not() {
        let b = base();
        let mut f = Follow::from_pose(&b, None);
        let a = f.pose(&b, Vec3::ZERO, 0.0, Some(0.0166));
        let eased = f.pose(&b, Vec3::new(20.0, 0.0, 0.0), 0.0, Some(0.0166));
        let exact = Follow::from_pose(&b, None).pose(&b, Vec3::new(20.0, 0.0, 0.0), 0.0, None);
        // The eased eye moved towards the new framing, not all the way.
        let (x0, x1, x2) = (a.pose.position.x, eased.pose.position.x, exact.pose.position.x);
        assert!(x0 < x1 && x1 < x2 - 10.0, "{x0} {x1} {x2}");
    }
}
