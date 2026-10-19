//! A unit's locomotion as the game's `UnitAnimationManager` plays it for a unit that stands or
//! walks outside combat.
//!
//! Two kinds exist (`docs/poc7/animation.md`):
//! - `UnitAnimationActionLocoMotion` (monsters): an animator controller, driven by the parameters
//!   `WalkSpeed` (`speed / DefaultSpeedMps`), `MoveType`, `Stopping`, `InCombat` and `IdleOffset`;
//! - `UnitAnimationActionLocoMotionHuman` (the human rig): plain clips. A unit standing still
//!   (`speed < 0.1`) loops the idle clip from a random time; a walking unit plays the walk clip of
//!   its walk speed type at `speed * 10/33` times its speed, and the clip changes with a
//!   cross-fade of `transition` seconds.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::clip::Clip;
use crate::controller::{BoundClip, Controller, Player};
use crate::rig::{Binder, Pose, Rig, apply_curves};

/// `UnitAnimationActionLocoMotion.WalkSpeedType`.
pub const SLOW: usize = 0;
pub const NORMAL: usize = 1;
pub const CHARGE: usize = 2;
pub const STEALTH: usize = 3;

/// `UnitAnimationActionLocoMotionHuman.IdleSpeedThreshold`.
pub const IDLE_SPEED_THRESHOLD: f32 = 0.1;
/// `UnitAnimationActionLocoMotionHuman.SpeedMapping`: playback speed per metre per second.
pub const SPEED_MAPPING: f32 = 10.0 / 33.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Locomotion {
    Controller {
        /// Index into the pack's controllers.
        controller: u32,
        /// The layer whose weight is one (`m_DefaultLayerIndex`; no weapon style applies).
        layer: u32,
        /// `DefaultSpeedMps`.
        default_speed_mps: f32,
        has_move_type: bool,
        has_idle_offset: bool,
    },
    Clips {
        /// Pack clip index of `NonCombatIdleWrapper`'s clip.
        idle: u32,
        /// `NonCombatWalkWrappers`' clips by walk speed type (`SLOW`..`STEALTH`).
        walk: Vec<Option<u32>>,
        /// `TransitionIn` of the action: the cross-fade between clips.
        transition: f32,
    },
}

/// One animated unit: its rig, the clips it may play and how it moves.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitAnimation {
    /// The unit's id in the view spec.
    pub unit: String,
    /// Index into the pack's rigs.
    pub rig: u32,
    /// Pack clip indices the unit's locomotion uses.
    pub clips: Vec<u32>,
    pub loco: Locomotion,
}

impl UnitAnimation {
    /// Checks the indices against the pack's tables.
    ///
    /// # Errors
    ///
    /// An index out of range, or a clip the locomotion plays that isn't in `clips`.
    pub fn validate(&self, rigs: usize, clips: usize, controllers: &[Controller]) -> Result<()> {
        ensure!((self.rig as usize) < rigs, "{}: rig {}", self.unit, self.rig);
        for c in &self.clips {
            ensure!((*c as usize) < clips, "{}: clip {c}", self.unit);
        }
        match &self.loco {
            Locomotion::Controller { controller, layer, .. } => {
                let c = controllers
                    .get(*controller as usize)
                    .ok_or_else(|| anyhow::anyhow!("{}: controller {controller}", self.unit))?;
                ensure!((*layer as usize) < c.layers.len(), "{}: layer {layer}", self.unit);
                for k in &c.clips {
                    ensure!(
                        self.clips.contains(k),
                        "{}: controller clip {k} not listed",
                        self.unit
                    );
                }
            }
            Locomotion::Clips { idle, walk, .. } => {
                for k in std::iter::once(idle).chain(walk.iter().flatten()) {
                    ensure!(self.clips.contains(k), "{}: clip {k} not listed", self.unit);
                }
            }
        }
        Ok(())
    }
}

/// The walk clip `SelectWalk` takes for a walk speed type: that type's, else (for Stealth) Slow's,
/// else (for the others) Normal's, else a lone clip.
#[must_use]
pub fn select_walk(walk: &[Option<u32>], ty: usize) -> Option<u32> {
    if let Some(Some(c)) = walk.get(ty) {
        return Some(*c);
    }
    match ty {
        STEALTH => select_walk(walk, SLOW),
        NORMAL => (walk.len() == 1).then(|| walk[0]).flatten(),
        _ => select_walk(walk, NORMAL),
    }
}

#[derive(Debug, Clone)]
struct Playing {
    clip: u32,
    time: f32,
}

/// A unit's locomotion being played.
#[derive(Debug)]
pub struct UnitPlayer {
    anim: UnitAnimation,
    binders: Vec<Binder>,
    controller: Option<(Controller, Player, Vec<usize>)>,
    speed: f32,
    walk_type: usize,
    /// Controller kind: the `IdleOffset` parameter, else the idle clip's start (cycles).
    idle_phase: f32,
    cur: Option<Playing>,
    /// The clip fading out and how far the fade is (`Clips` kind).
    prev: Option<(Playing, f32)>,
}

impl UnitPlayer {
    /// Starts the unit standing still. `idle_phase` (0..1) stands for the game's `Random.value`
    /// (the idle clip's start time, or the controller's `IdleOffset`).
    ///
    /// # Errors
    ///
    /// A controller or clip index out of range.
    pub fn new(
        anim: &UnitAnimation,
        rig: &Rig,
        clips: &[Clip],
        controllers: &[Controller],
        idle_phase: f32,
    ) -> Result<Self> {
        anim.validate(1 << 30, clips.len(), controllers)?;
        let binders = anim.clips.iter().map(|c| Binder::new(&clips[*c as usize], rig)).collect();
        let controller = match &anim.loco {
            Locomotion::Controller { controller, layer, has_idle_offset, .. } => {
                let c = controllers[*controller as usize].clone();
                let mut p = Player::new(&c);
                for l in 0..c.layers.len() {
                    p.set_layer_weight(l, if l == *layer as usize { 1.0 } else { 0.0 });
                }
                if *has_idle_offset && let Some(i) = c.param("IdleOffset") {
                    p.set(i, idle_phase);
                }
                let local = c
                    .clips
                    .iter()
                    .map(|k| anim.clips.iter().position(|x| x == k).unwrap_or(0))
                    .collect();
                Some((c, p, local))
            }
            Locomotion::Clips { .. } => None,
        };
        let mut me = Self {
            anim: anim.clone(),
            binders,
            controller,
            speed: 0.0,
            walk_type: NORMAL,
            idle_phase,
            cur: None,
            prev: None,
        };
        if let Locomotion::Clips { idle, .. } = &anim.loco {
            let len = clips[*idle as usize].length();
            me.cur = Some(Playing { clip: *idle, time: idle_phase * len });
        }
        Ok(me)
    }

    /// The unit's speed in metres per second (`UnitAnimationManager.Speed`).
    pub fn set_speed(&mut self, mps: f32) {
        self.speed = mps;
    }

    pub fn set_walk_type(&mut self, ty: usize) {
        self.walk_type = ty;
    }

    #[must_use]
    pub fn speed(&self) -> f32 {
        self.speed
    }

    fn index_of(&self, clip: u32) -> usize {
        self.anim.clips.iter().position(|x| *x == clip).unwrap_or(0)
    }

    /// Advances by `dt` seconds.
    pub fn update(&mut self, clips: &[Clip], dt: f32) {
        match &self.anim.loco {
            Locomotion::Controller { default_speed_mps, has_move_type, .. } => {
                let (c, p, local) = self.controller.as_mut().expect("controller player");
                if let Some(i) = c.param("WalkSpeed") {
                    p.set(i, self.speed / default_speed_mps);
                }
                if *has_move_type && let Some(i) = c.param("MoveType") {
                    p.set(i, self.walk_type as f32);
                }
                let bound: Vec<BoundClip<'_>> = local
                    .iter()
                    .zip(&c.clips)
                    .map(|(l, k)| BoundClip {
                        clip: &clips[*k as usize],
                        binder: &self.binders[*l],
                    })
                    .collect();
                p.update(c, &bound, dt);
            }
            Locomotion::Clips { idle, walk, transition } => {
                let idle_now = self.speed < IDLE_SPEED_THRESHOLD;
                let want = if idle_now { Some(*idle) } else { select_walk(walk, self.walk_type) };
                let (idle, transition) = (*idle, *transition);
                if let (Some(want), Some(cur)) = (want, &self.cur)
                    && cur.clip != want
                {
                    let start = if want == idle {
                        self.idle_phase * clips[want as usize].length()
                    } else {
                        0.0
                    };
                    self.prev = Some((cur.clone(), 0.0));
                    self.cur = Some(Playing { clip: want, time: start });
                }
                let speed = self.speed;
                let rate = |clip: u32| if clip == idle { 1.0 } else { speed * SPEED_MAPPING };
                if let Some(cur) = self.cur.as_mut() {
                    cur.time += dt * rate(cur.clip);
                }
                let mut done = false;
                if let Some((p, f)) = self.prev.as_mut() {
                    p.time += dt * rate(p.clip);
                    *f += dt;
                    done = transition <= 0.0 || *f >= transition;
                }
                if done {
                    self.prev = None;
                }
            }
        }
    }

    fn clip_pose(&self, clips: &[Clip], rig: &Rig, p: &Playing, scratch: &mut Vec<f32>) -> Pose {
        let clip = &clips[p.clip as usize];
        let mut pose = rig.rest_pose();
        clip.sample(clip.local_time(p.time), scratch);
        apply_curves(&self.binders[self.index_of(p.clip)], scratch, &mut pose);
        pose
    }

    /// The pose now, with the skeleton's overrides applied.
    #[must_use]
    pub fn pose(&self, clips: &[Clip], rig: &Rig, scratch: &mut Vec<f32>) -> Pose {
        let mut pose = match &self.anim.loco {
            Locomotion::Controller { .. } => {
                let (c, p, local) = self.controller.as_ref().expect("controller player");
                let bound: Vec<BoundClip<'_>> = local
                    .iter()
                    .zip(&c.clips)
                    .map(|(l, k)| BoundClip {
                        clip: &clips[*k as usize],
                        binder: &self.binders[*l],
                    })
                    .collect();
                p.evaluate(c, &bound, rig, scratch)
            }
            Locomotion::Clips { transition, .. } => {
                let cur = self.cur.as_ref().expect("a playing clip");
                let mut pose = self.clip_pose(clips, rig, cur, scratch);
                if let Some((prev, f)) = &self.prev {
                    let mut out = self.clip_pose(clips, rig, prev, scratch);
                    let w = if *transition > 0.0 { (f / transition).clamp(0.0, 1.0) } else { 1.0 };
                    out.blend(&pose, w);
                    pose = out;
                }
                pose
            }
        };
        rig.apply_overrides(&mut pose);
        pose
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{ATTR_POSITION, Binding, TYPE_TRANSFORM};
    use crate::rig::{Bone, Trs};

    fn rig() -> Rig {
        Rig {
            name: "r".into(),
            bones: vec![Bone {
                name: "root".into(),
                hash: 7,
                parent: u32::MAX,
                rest: Trs::IDENTITY,
            }],
            root: glam::Mat4::IDENTITY.to_cols_array(),
            overrides: Vec::new(),
        }
    }

    /// A looping clip whose position along X equals `x0` at its start and rises 1 per second.
    fn ramp(x0: f32, length: f32) -> Clip {
        Clip {
            name: "ramp".into(),
            stop: length,
            looping: true,
            bindings: vec![Binding { path: 7, attribute: ATTR_POSITION, type_id: TYPE_TRANSFORM }],
            dense: crate::clip::Dense {
                begin: 0.0,
                rate: 1.0 / length,
                frames: 2,
                curves: 3,
                samples: vec![x0, 0.0, 0.0, x0 + length, 0.0, 0.0],
            },
            ..Clip::default()
        }
    }

    fn human() -> (UnitAnimation, Vec<Clip>) {
        let clips = vec![ramp(0.0, 2.0), ramp(100.0, 1.0)];
        let anim = UnitAnimation {
            unit: "u".into(),
            rig: 0,
            clips: vec![0, 1],
            loco: Locomotion::Clips {
                idle: 0,
                walk: vec![None, Some(1), None, None],
                transition: 0.5,
            },
        };
        (anim, clips)
    }

    #[test]
    fn walk_selection_falls_back_like_select_walk() {
        let w = [Some(5), Some(6), None, None];
        assert_eq!(select_walk(&w, CHARGE), Some(6));
        assert_eq!(select_walk(&w, STEALTH), Some(5));
        assert_eq!(select_walk(&[Some(9)], NORMAL), Some(9));
        assert_eq!(select_walk(&[None, None], NORMAL), None);
    }

    #[test]
    fn a_standing_unit_loops_its_idle_from_the_given_phase() {
        let (anim, clips) = human();
        let mut p = UnitPlayer::new(&anim, &rig(), &clips, &[], 0.25).unwrap();
        let mut s = Vec::new();
        let r = rig();
        assert!((p.pose(&clips, &r, &mut s).local[0].position[0] - 0.5).abs() < 1e-5);
        p.update(&clips, 1.0);
        assert!((p.pose(&clips, &r, &mut s).local[0].position[0] - 1.5).abs() < 1e-5);
        p.update(&clips, 1.0);
        // The idle wrapped: 2.5 s into a 2 s clip.
        assert!((p.pose(&clips, &r, &mut s).local[0].position[0] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn walking_switches_clip_scales_the_clock_and_fades() {
        let (anim, clips) = human();
        let r = rig();
        let mut p = UnitPlayer::new(&anim, &r, &clips, &[], 0.0).unwrap();
        let mut s = Vec::new();
        p.set_speed(3.3);
        p.update(&clips, 0.25);
        // Half way through the 0.5 s fade between idle (at 0.25 s: x = 0.25) and the walk, which
        // has played 0.25 s at 3.3 * 10/33 = 1 (x = 100.25).
        let x = p.pose(&clips, &r, &mut s).local[0].position[0];
        assert!((x - 50.25).abs() < 1e-3, "{x}");
        p.update(&clips, 0.5);
        let x = p.pose(&clips, &r, &mut s).local[0].position[0];
        assert!((x - 100.75).abs() < 1e-3, "{x}");
        // Stopping returns to the idle.
        p.set_speed(0.0);
        p.update(&clips, 1.0);
        p.update(&clips, 0.0);
        assert!(p.pose(&clips, &r, &mut s).local[0].position[0] < 5.0);
    }

    #[test]
    fn indices_are_checked() {
        let (mut anim, clips) = human();
        anim.clips.pop();
        assert!(UnitPlayer::new(&anim, &rig(), &clips, &[], 0.0).is_err());
    }
}
