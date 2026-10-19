//! Mecanim animator controllers as the player runs them: parameters, layers of state machines,
//! states playing a clip or a 1D blend tree, and transitions that cross-fade on conditions.
//!
//! This is the part of Unity's `AnimatorController` the monsters' locomotion uses. Not modelled:
//! avatar masks (the importer refuses a layer with one), additive layers, 2D and direct blend
//! trees, mirroring, triggers, sub-state machines and transition interruption. A transition
//! cross-fades linearly; while it runs the source and the destination both advance and no other
//! transition starts.

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};

use crate::clip::Clip;
use crate::rig::{Binder, Pose, Rig, apply_curves};

/// A parameter's type (`AnimatorControllerParameterType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamKind {
    Float,
    Int,
    Bool,
    Trigger,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Param {
    /// The hash conditions and speed parameters name it by (`Animator.StringToHash`).
    pub id: u32,
    pub name: String,
    pub kind: ParamKind,
    pub default: f32,
}

/// What a state plays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Motion {
    /// A clip, by index into [`Controller::clips`].
    Clip(u32),
    /// A 1D blend tree: the parameter (an index into [`Controller::params`]) picks the two
    /// children around its value; thresholds ascend.
    Blend1D { param: u32, children: Vec<(f32, Motion)> },
}

/// `AnimatorConditionMode`.
pub const IF: u8 = 1;
pub const IF_NOT: u8 = 2;
pub const GREATER: u8 = 3;
pub const LESS: u8 = 4;
pub const EQUALS: u8 = 6;
pub const NOT_EQUAL: u8 = 7;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    pub mode: u8,
    /// Index into [`Controller::params`].
    pub param: u32,
    pub threshold: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    /// Index into the layer's states.
    pub dest: u32,
    pub conditions: Vec<Condition>,
    pub has_exit_time: bool,
    /// Normalised time of the source state the transition waits for.
    pub exit_time: f32,
    /// `duration` is seconds (else a fraction of the source state's length).
    pub fixed_duration: bool,
    pub duration: f32,
    /// Normalised time the destination starts at.
    pub offset: f32,
    pub can_transition_to_self: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub name: String,
    pub motion: Motion,
    pub speed: f32,
    /// The state's speed is multiplied by this parameter.
    pub speed_param: Option<u32>,
    pub cycle_offset: f32,
    /// The state starts this far (in cycles) into the motion; replaces `cycle_offset`.
    pub cycle_param: Option<u32>,
    pub looping: bool,
    pub transitions: Vec<Transition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    pub default_state: u32,
    pub states: Vec<State>,
    pub any_state: Vec<Transition>,
    /// The weight the layer has before anything sets it (`m_DefaultWeight`).
    pub default_weight: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Controller {
    pub name: String,
    pub params: Vec<Param>,
    /// Pack clip index of each controller clip (`m_AnimationClips`).
    pub clips: Vec<u32>,
    pub layers: Vec<Layer>,
}

impl Controller {
    /// Checks every index the player will follow.
    ///
    /// # Errors
    ///
    /// A state, parameter or clip index out of range.
    pub fn validate(&self, pack_clips: usize) -> Result<()> {
        for c in &self.clips {
            ensure!((*c as usize) < pack_clips, "{}: clip {c} of {pack_clips}", self.name);
        }
        let np = self.params.len() as u32;
        for p in &self.params {
            ensure!(
                p.default.is_finite(),
                "{}: parameter {} defaults to {}",
                self.name,
                p.name,
                p.default
            );
        }
        for l in &self.layers {
            if l.states.is_empty() {
                continue;
            }
            ensure!((l.default_state as usize) < l.states.len(), "{}: default state", l.name);
            ensure!(l.default_weight.is_finite(), "{}: default weight", l.name);
            let trs = l.states.iter().flat_map(|s| &s.transitions).chain(&l.any_state);
            for t in trs {
                ensure!((t.dest as usize) < l.states.len(), "{}: transition to {}", l.name, t.dest);
                ensure!(
                    t.duration.is_finite()
                        && t.duration >= 0.0
                        && t.offset.is_finite()
                        && t.exit_time.is_finite(),
                    "{}: transition to {} has a duration, offset or exit time that isn't a number",
                    l.name,
                    t.dest
                );
                ensure!(
                    t.conditions.iter().all(|c| c.threshold.is_finite()),
                    "{}: a condition threshold isn't a number",
                    l.name
                );
                for c in &t.conditions {
                    ensure!(c.param < np, "{}: condition on parameter {}", l.name, c.param);
                }
            }
            for s in &l.states {
                ensure!(
                    s.speed.is_finite() && s.cycle_offset.is_finite(),
                    "{}: speed or cycle offset isn't a number",
                    s.name
                );
                for p in s.speed_param.iter().chain(&s.cycle_param) {
                    ensure!(*p < np, "{}: parameter {p}", s.name);
                }
                self.check_motion(&s.motion)?;
            }
        }
        Ok(())
    }

    fn check_motion(&self, m: &Motion) -> Result<()> {
        match m {
            Motion::Clip(c) => ensure!((*c as usize) < self.clips.len(), "motion clip {c}"),
            Motion::Blend1D { param, children } => {
                ensure!((*param as usize) < self.params.len(), "blend parameter {param}");
                ensure!(!children.is_empty(), "blend tree without children");
                ensure!(
                    children.iter().all(|c| c.0.is_finite())
                        && children.windows(2).all(|w| w[0].0 <= w[1].0),
                    "blend tree thresholds are not finite and ascending"
                );
                for (_, c) in children {
                    self.check_motion(c)?;
                }
            }
        }
        Ok(())
    }

    /// The parameter index for a hash or a name.
    #[must_use]
    pub fn param(&self, name: &str) -> Option<usize> {
        self.params.iter().position(|p| p.name == name)
    }
}

/// A controller's clips resolved against one rig, in [`Controller::clips`] order.
#[derive(Debug)]
pub struct BoundClip<'a> {
    pub clip: &'a Clip,
    pub binder: &'a Binder,
}

#[derive(Debug, Clone)]
struct Fade {
    dest: u32,
    dest_time: f32,
    elapsed: f32,
    duration: f32,
}

#[derive(Debug, Clone)]
struct LayerState {
    state: u32,
    /// Normalised time in the state's motion (cycles, unbounded while it loops).
    time: f32,
    fade: Option<Fade>,
    weight: f32,
}

/// A controller being played: its parameters and each layer's current state.
#[derive(Debug, Clone)]
pub struct Player {
    params: Vec<f32>,
    layers: Vec<LayerState>,
}

impl Player {
    /// Starts every layer in its default state with the parameters at their defaults.
    #[must_use]
    pub fn new(c: &Controller) -> Self {
        let params: Vec<f32> = c.params.iter().map(|p| p.default).collect();
        let layers = c
            .layers
            .iter()
            .map(|l| LayerState {
                state: l.default_state,
                time: 0.0,
                fade: None,
                weight: l.default_weight,
            })
            .collect();
        Self { params, layers }
    }

    /// Where in its motion a state's clock starts (cycles): the cycle offset, or its parameter.
    fn phase(&self, s: &State) -> f32 {
        s.cycle_param.map_or(s.cycle_offset, |i| self.params[i as usize])
    }

    /// Sets a parameter; a value that isn't a number is ignored.
    pub fn set(&mut self, param: usize, value: f32) {
        if value.is_finite() {
            self.params[param] = value;
        }
    }

    #[must_use]
    pub fn get(&self, param: usize) -> f32 {
        self.params[param]
    }

    pub fn set_layer_weight(&mut self, layer: usize, w: f32) {
        self.layers[layer].weight = w;
    }

    /// The state a layer is in (the destination once a fade has started).
    #[must_use]
    pub fn state(&self, layer: usize) -> u32 {
        let l = &self.layers[layer];
        l.fade.as_ref().map_or(l.state, |f| f.dest)
    }

    /// Whether `t` may fire in an update that took the state's normalised time from `prev` to
    /// `time`: its exit time was crossed in that update (a looping state crosses it again every
    /// cycle when it is below one, so a transition waiting on a condition fires at the next
    /// crossing, not at once) and its conditions hold. Following Unity's documentation of
    /// `AnimatorStateTransition.exitTime`, the exit time is true on the update that crosses it and
    /// false on the next, for a state that doesn't loop too (the page doesn't address that case;
    /// not verified against the original until the pose capture exists).
    fn test(&self, t: &Transition, prev: f32, time: f32, looping: bool) -> bool {
        if t.has_exit_time {
            let crossed = if looping && t.exit_time < 1.0 {
                // Some cycle's exit point lies in (prev, time].
                (prev.div_euclid(1.0) as i64..=time.div_euclid(1.0) as i64).any(|k| {
                    let at = t.exit_time + k as f32;
                    prev < at && at <= time
                })
            } else {
                // Once (above one, or a state that doesn't loop): crossed in this update.
                prev < t.exit_time && t.exit_time <= time
            };
            if !crossed {
                return false;
            }
        }
        if t.conditions.is_empty() && !t.has_exit_time {
            return false;
        }
        t.conditions.iter().all(|c| {
            let v = self.params[c.param as usize];
            match c.mode {
                IF => v != 0.0,
                IF_NOT => v == 0.0,
                GREATER => v > c.threshold,
                LESS => v < c.threshold,
                EQUALS => (v - c.threshold).abs() < 1e-6,
                NOT_EQUAL => (v - c.threshold).abs() >= 1e-6,
                _ => false,
            }
        })
    }

    fn speed(&self, s: &State) -> f32 {
        s.speed * s.speed_param.map_or(1.0, |i| self.params[i as usize])
    }

    /// Advances the controller by `dt` seconds.
    pub fn update(&mut self, c: &Controller, clips: &[BoundClip<'_>], dt: f32) {
        for li in 0..self.layers.len() {
            let layer = &c.layers[li];
            if layer.states.is_empty() {
                continue;
            }
            let ls = self.layers[li].clone();
            let cur = &layer.states[ls.state as usize];
            let len = self.motion_length(clips, &cur.motion);
            let speed = self.speed(cur);
            let mut prev_time = ls.time;
            let mut time = ls.time;
            if len > 0.0 {
                time += dt * speed / len;
            }
            let mut fade = ls.fade;
            let mut state = ls.state;
            if let Some(f) = &mut fade {
                f.elapsed += dt;
                let dst = &layer.states[f.dest as usize];
                let dlen = self.motion_length(clips, &dst.motion);
                if dlen > 0.0 {
                    f.dest_time += dt * self.speed(dst) / dlen;
                }
                if f.elapsed >= f.duration {
                    state = f.dest;
                    prev_time =
                        f.dest_time - if dlen > 0.0 { dt * self.speed(dst) / dlen } else { 0.0 };
                    time = f.dest_time;
                    fade = None;
                }
            }
            if fade.is_none() {
                let cur = &layer.states[state as usize];
                let hit = layer.any_state.iter().chain(&cur.transitions).find(|t| {
                    (t.dest != state || t.can_transition_to_self)
                        && self.test(t, prev_time, time, cur.looping)
                });
                if let Some(t) = hit {
                    let duration = if t.fixed_duration {
                        t.duration
                    } else {
                        t.duration * self.motion_length(clips, &cur.motion)
                    };
                    fade = Some(Fade { dest: t.dest, dest_time: t.offset, elapsed: 0.0, duration });
                    if duration <= 0.0 {
                        state = t.dest;
                        time = t.offset;
                        fade = None;
                    }
                }
            }
            let l = &mut self.layers[li];
            l.state = state;
            l.time = time;
            l.fade = fade;
        }
    }

    /// The motion's length in seconds: a clip's, or a blend tree's at its parameter's value (each
    /// tree reads its own parameter).
    fn motion_length(&self, clips: &[BoundClip<'_>], m: &Motion) -> f32 {
        match m {
            Motion::Clip(i) => clips[*i as usize].clip.length(),
            Motion::Blend1D { param, children } => {
                let (a, b, w) = blend_pair(children, self.params[*param as usize]);
                let la = self.motion_length(clips, &children[a].1);
                let lb = self.motion_length(clips, &children[b].1);
                la + (lb - la) * w
            }
        }
    }

    fn motion_pose(
        &self,
        clips: &[BoundClip<'_>],
        rig: &Rig,
        m: &Motion,
        norm: f32,
        looping: bool,
        scratch: &mut Vec<f32>,
    ) -> Pose {
        match m {
            Motion::Clip(i) => {
                let bc = &clips[*i as usize];
                let n = if looping { norm.rem_euclid(1.0) } else { norm.clamp(0.0, 1.0) };
                let mut pose = rig.rest_pose();
                bc.clip.sample(bc.clip.start + n * bc.clip.length(), scratch);
                apply_curves(bc.binder, scratch, &mut pose);
                pose
            }
            Motion::Blend1D { param, children } => {
                let (a, b, w) = blend_pair(children, self.params[*param as usize]);
                let mut pa = self.motion_pose(clips, rig, &children[a].1, norm, looping, scratch);
                if w > 0.0 && a != b {
                    let pb = self.motion_pose(clips, rig, &children[b].1, norm, looping, scratch);
                    pa.blend(&pb, w);
                }
                pa
            }
        }
    }

    /// The pose the controller gives the rig now: each layer with weight, blended over the rest
    /// pose in order.
    #[must_use]
    pub fn evaluate(
        &self,
        c: &Controller,
        clips: &[BoundClip<'_>],
        rig: &Rig,
        scratch: &mut Vec<f32>,
    ) -> Pose {
        let mut out = rig.rest_pose();
        for (li, layer) in c.layers.iter().enumerate() {
            let ls = &self.layers[li];
            if ls.weight <= 0.0 || layer.states.is_empty() {
                continue;
            }
            let cur = &layer.states[ls.state as usize];
            let mut pose = self.motion_pose(
                clips,
                rig,
                &cur.motion,
                ls.time + self.phase(cur),
                cur.looping,
                scratch,
            );
            if let Some(f) = &ls.fade {
                let dst = &layer.states[f.dest as usize];
                let pd = self.motion_pose(
                    clips,
                    rig,
                    &dst.motion,
                    f.dest_time + self.phase(dst),
                    dst.looping,
                    scratch,
                );
                pose.blend(&pd, (f.elapsed / f.duration).clamp(0.0, 1.0));
            }
            out.blend(&pose, ls.weight.min(1.0));
        }
        out
    }
}

/// The two children a 1D blend value falls between and the weight of the second.
fn blend_pair(children: &[(f32, Motion)], v: f32) -> (usize, usize, f32) {
    let n = children.len();
    // A parameter that isn't a number selects the first child.
    if !v.is_finite() || v <= children[0].0 {
        return (0, 0, 0.0);
    }
    if v >= children[n - 1].0 {
        return (n - 1, n - 1, 0.0);
    }
    let hi = children.iter().position(|c| c.0 >= v).unwrap_or(n - 1).max(1);
    let lo = hi - 1;
    let span = children[hi].0 - children[lo].0;
    (lo, hi, if span > 0.0 { (v - children[lo].0) / span } else { 0.0 })
}

/// Rejects what the player doesn't model, with the reason (the importer calls this on every
/// controller it keeps).
///
/// # Errors
///
/// A feature the player ignores.
pub fn check_supported(c: &Controller) -> Result<()> {
    for l in &c.layers {
        for s in &l.states {
            if has_unsupported(&s.motion) {
                bail!("{}: state {} has a motion the player doesn't model", c.name, s.name);
            }
        }
    }
    Ok(())
}

fn has_unsupported(m: &Motion) -> bool {
    match m {
        Motion::Clip(_) => false,
        Motion::Blend1D { children, .. } => {
            children.windows(2).any(|w| w[0].0 > w[1].0)
                || children.iter().any(|c| has_unsupported(&c.1))
        }
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

    /// A clip of one constant position along X, `length` seconds long.
    fn clip(x: f32, length: f32) -> Clip {
        Clip {
            name: format!("x{x}"),
            stop: length,
            looping: true,
            bindings: vec![Binding { path: 7, attribute: ATTR_POSITION, type_id: TYPE_TRANSFORM }],
            constant: vec![x, 0.0, 0.0],
            ..Clip::default()
        }
    }

    fn controller() -> Controller {
        let tr = |dest, mode, threshold| Transition {
            dest,
            conditions: vec![Condition { mode, param: 0, threshold }],
            has_exit_time: false,
            exit_time: 0.0,
            fixed_duration: true,
            duration: 0.5,
            offset: 0.0,
            can_transition_to_self: false,
        };
        let state = |name: &str, clip, speed_param, transitions| State {
            name: name.into(),
            motion: Motion::Clip(clip),
            speed: 1.0,
            speed_param,
            cycle_offset: 0.0,
            cycle_param: None,
            looping: true,
            transitions,
        };
        Controller {
            name: "c".into(),
            params: vec![Param {
                id: 1,
                name: "WalkSpeed".into(),
                kind: ParamKind::Float,
                default: 0.0,
            }],
            clips: vec![0, 1],
            layers: vec![Layer {
                name: "L".into(),
                default_state: 0,
                states: vec![
                    state("Idle", 0, None, vec![tr(1, GREATER, 0.01)]),
                    state("Walk", 1, Some(0), vec![tr(0, LESS, 0.01)]),
                ],
                any_state: Vec::new(),
                default_weight: 1.0,
            }],
        }
    }

    #[test]
    fn idle_then_walk_cross_fades_then_switches() {
        let (c, r) = (controller(), rig());
        let clips = [clip(1.0, 2.0), clip(3.0, 1.0)];
        let binders: Vec<Binder> = clips.iter().map(|k| Binder::new(k, &r)).collect();
        let bound: Vec<BoundClip<'_>> =
            clips.iter().zip(&binders).map(|(k, b)| BoundClip { clip: k, binder: b }).collect();
        c.validate(2).unwrap();
        let mut p = Player::new(&c);
        let mut s = Vec::new();
        p.update(&c, &bound, 0.1);
        assert_eq!(p.evaluate(&c, &bound, &r, &mut s).local[0].position[0], 1.0);
        p.set(0, 1.0);
        p.update(&c, &bound, 0.0);
        assert_eq!(p.state(0), 1);
        // A quarter of the way through a half-second fade: 1 + 0.25 * (3 - 1).
        p.update(&c, &bound, 0.125);
        let x = p.evaluate(&c, &bound, &r, &mut s).local[0].position[0];
        assert!((x - 1.5).abs() < 1e-5, "{x}");
        p.update(&c, &bound, 1.0);
        assert_eq!(p.evaluate(&c, &bound, &r, &mut s).local[0].position[0], 3.0);
        // And back when the speed drops.
        p.set(0, 0.0);
        p.update(&c, &bound, 0.0);
        assert_eq!(p.state(0), 0);
    }

    #[test]
    fn the_speed_parameter_scales_the_state_clock() {
        let c = controller();
        let r = rig();
        let clips = [clip(1.0, 2.0), clip(3.0, 1.0)];
        let binders: Vec<Binder> = clips.iter().map(|k| Binder::new(k, &r)).collect();
        let bound: Vec<BoundClip<'_>> =
            clips.iter().zip(&binders).map(|(k, b)| BoundClip { clip: k, binder: b }).collect();
        let mut p = Player::new(&c);
        p.set(0, 2.0);
        // Idle -> Walk fade (0.5 s), then walk at twice speed for 0.25 s of a 1 s clip.
        p.update(&c, &bound, 0.0);
        p.update(&c, &bound, 0.5);
        p.update(&c, &bound, 0.25);
        assert!(
            (p.layers[0].time - (0.0 + 0.5 * 2.0 + 0.25 * 2.0)).abs() < 1e-5,
            "{}",
            p.layers[0].time
        );
    }

    #[test]
    fn a_looping_states_exit_time_waits_for_the_next_cycle() {
        let (mut c, r) = (controller(), rig());
        // Idle -> Walk only from 75% of the idle clip on, once WalkSpeed is up.
        c.layers[0].states[0].transitions[0].has_exit_time = true;
        c.layers[0].states[0].transitions[0].exit_time = 0.75;
        let clips = [clip(1.0, 2.0), clip(3.0, 1.0)];
        let binders: Vec<Binder> = clips.iter().map(|k| Binder::new(k, &r)).collect();
        let bound: Vec<BoundClip<'_>> =
            clips.iter().zip(&binders).map(|(k, b)| BoundClip { clip: k, binder: b }).collect();
        let mut p = Player::new(&c);
        // 1.7 s into the 2 s idle: cycle position 0.85, past the exit point, which was crossed with
        // the condition false. Raising WalkSpeed now changes nothing until the next crossing.
        p.update(&c, &bound, 1.7);
        p.set(0, 1.0);
        p.update(&c, &bound, 0.2);
        assert_eq!(p.state(0), 0);
        // The exit point comes round again at 1.75 cycles (3.5 s): not yet at 3.4 s, then crossed.
        p.update(&c, &bound, 1.5);
        assert_eq!(p.state(0), 0);
        p.update(&c, &bound, 0.2);
        assert_eq!(p.state(0), 1);
    }

    #[test]
    fn a_one_shot_states_exit_time_is_one_crossing_and_numbers_must_be_numbers() {
        let (mut c, r) = (controller(), rig());
        c.layers[0].states[0].looping = false;
        c.layers[0].states[0].transitions[0].has_exit_time = true;
        c.layers[0].states[0].transitions[0].exit_time = 0.5;
        let clips = [clip(1.0, 2.0), clip(3.0, 1.0)];
        let binders: Vec<Binder> = clips.iter().map(|k| Binder::new(k, &r)).collect();
        let bound: Vec<BoundClip<'_>> =
            clips.iter().zip(&binders).map(|(k, b)| BoundClip { clip: k, binder: b }).collect();
        let mut p = Player::new(&c);
        // 1.5 s of the 2 s clip is 0.75: the exit time (0.5) was crossed with WalkSpeed at zero, and
        // a condition that turns true afterwards does not start the transition.
        p.update(&c, &bound, 1.5);
        p.set(0, 1.0);
        p.update(&c, &bound, 0.1);
        assert_eq!(p.state(0), 0);
        // With the condition already true, the crossing update fires it.
        let mut q = Player::new(&c);
        q.set(0, 1.0);
        q.update(&c, &bound, 1.0);
        assert_eq!(q.state(0), 1);
        // Non-finite playback values are refused at validation.
        for f in [
            |c: &mut Controller| c.layers[0].states[0].transitions[0].duration = f32::NAN,
            |c: &mut Controller| c.layers[0].states[0].transitions[0].duration = -1.0,
            |c: &mut Controller| c.layers[0].states[0].transitions[0].exit_time = f32::INFINITY,
            |c: &mut Controller| {
                c.layers[0].states[0].transitions[0].conditions[0].threshold = f32::NAN
            },
            |c: &mut Controller| c.layers[0].states[0].speed = f32::NAN,
            |c: &mut Controller| c.layers[0].states[0].cycle_offset = f32::NAN,
            |c: &mut Controller| c.layers[0].default_weight = f32::NAN,
        ] {
            let mut bad = controller();
            f(&mut bad);
            assert!(bad.validate(2).is_err());
        }
    }

    #[test]
    fn nested_blend_trees_use_their_own_parameter_for_their_length() {
        let (mut c, r) = (controller(), rig());
        c.params.push(Param { id: 2, name: "Inner".into(), kind: ParamKind::Float, default: 1.0 });
        // Outer tree on param 0 over [clip 0 (2 s), inner tree on param 1 over clips 0 and 1].
        c.layers[0].states[0].motion = Motion::Blend1D {
            param: 0,
            children: vec![
                (0.0, Motion::Clip(0)),
                (
                    1.0,
                    Motion::Blend1D {
                        param: 1,
                        children: vec![(0.0, Motion::Clip(0)), (1.0, Motion::Clip(1))],
                    },
                ),
            ],
        };
        c.validate(2).unwrap();
        let clips = [clip(1.0, 2.0), clip(3.0, 1.0)];
        let binders: Vec<Binder> = clips.iter().map(|k| Binder::new(k, &r)).collect();
        let bound: Vec<BoundClip<'_>> =
            clips.iter().zip(&binders).map(|(k, b)| BoundClip { clip: k, binder: b }).collect();
        let mut p = Player::new(&c);
        p.set(0, 1.0);
        // Outer value 1 selects the inner tree; its own parameter (1) selects the 1 s clip, so the
        // state's clock runs one cycle a second, not half a cycle as the outer value would give.
        p.set(1, 1.0);
        let len = p.motion_length(&bound, &c.layers[0].states[0].motion);
        assert!((len - 1.0).abs() < 1e-6, "{len}");
    }

    #[test]
    fn non_finite_parameters_and_bad_thresholds_are_harmless() {
        let kids = vec![(0.0, Motion::Clip(0))];
        assert_eq!(blend_pair(&kids, f32::NAN), (0, 0, 0.0));
        assert_eq!(blend_pair(&kids, 5.0), (0, 0, 0.0));
        let mut c = controller();
        let mut p = Player::new(&c);
        p.set(0, f32::NAN);
        assert_eq!(p.get(0), 0.0);
        c.params[0].default = f32::NAN;
        assert!(c.validate(2).is_err());
        let mut c = controller();
        c.layers[0].states[0].motion = Motion::Blend1D {
            param: 0,
            children: vec![(1.0, Motion::Clip(0)), (0.0, Motion::Clip(1))],
        };
        assert!(c.validate(2).is_err());
    }

    #[test]
    fn blend_trees_pick_the_neighbouring_children() {
        let kids = vec![(0.0, Motion::Clip(0)), (1.0, Motion::Clip(1)), (3.0, Motion::Clip(2))];
        assert_eq!(blend_pair(&kids, -1.0), (0, 0, 0.0));
        assert_eq!(blend_pair(&kids, 0.5), (0, 1, 0.5));
        assert_eq!(blend_pair(&kids, 2.0), (1, 2, 0.5));
        assert_eq!(blend_pair(&kids, 9.0), (2, 2, 0.0));
    }

    #[test]
    fn bad_indices_are_rejected() {
        let mut c = controller();
        c.layers[0].states[0].transitions[0].dest = 5;
        assert!(c.validate(2).is_err());
        let mut c = controller();
        c.clips[1] = 9;
        assert!(c.validate(2).is_err());
    }
}
