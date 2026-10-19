//! A bone hierarchy and the pose clips put on it.
//!
//! A [`Rig`] is the transform hierarchy under an `Animator`: every transform with its name, the
//! CRC-32 of its path from the animator's root (what clip bindings name) and its rest local
//! transform. A [`Pose`] holds one local transform per bone. Clips overwrite the bones they bind,
//! the rest keep the rig's rest transform; blending two poses interpolates position and scale
//! linearly and rotation as a normalised linear interpolation (`Quaternion::Lerp`'s shortest
//! arc). World matrices compose `parent * T * R * S` down the hierarchy.

use anyhow::{Result, ensure};
use glam::{EulerRot, Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::clip::{ATTR_EULER, ATTR_POSITION, ATTR_ROTATION, ATTR_SCALE, Clip, TYPE_TRANSFORM};

/// A local transform: position, rotation quaternion `x, y, z, w`, scale.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Trs {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Trs {
    pub const IDENTITY: Self =
        Self { position: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] };

    #[must_use]
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::from_array(self.scale),
            Quat::from_array(self.rotation).normalize(),
            Vec3::from_array(self.position),
        )
    }

    /// Blend of two transforms: `a` at weight `1 - w`, `b` at `w`.
    #[must_use]
    pub fn lerp(&self, b: &Self, w: f32) -> Self {
        let l = |x: f32, y: f32| x + (y - x) * w;
        let qa = Quat::from_array(self.rotation);
        let mut qb = Quat::from_array(b.rotation);
        if qa.dot(qb) < 0.0 {
            qb = -qb;
        }
        let q = (qa * (1.0 - w) + qb * w).normalize();
        Self {
            position: [
                l(self.position[0], b.position[0]),
                l(self.position[1], b.position[1]),
                l(self.position[2], b.position[2]),
            ],
            rotation: q.to_array(),
            scale: [
                l(self.scale[0], b.scale[0]),
                l(self.scale[1], b.scale[1]),
                l(self.scale[2], b.scale[2]),
            ],
        }
    }
}

/// One bone of a rig.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bone {
    pub name: String,
    /// CRC-32 of the path from the animator's root (0 for the root).
    pub hash: u32,
    /// Index of the parent bone (always lower than this bone's), `u32::MAX` for the root.
    pub parent: u32,
    pub rest: Trs,
}

/// What `Character`'s skeleton asset forces on a bone after the animation: its scale, and its
/// position when `offset` is set (`BoneUpdateJob`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Override {
    pub bone: u32,
    pub scale: [f32; 3],
    pub offset: Option<[f32; 3]>,
}

/// The hierarchy under an animator.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Rig {
    pub name: String,
    /// Parents before children; bone 0 is the animator's root.
    pub bones: Vec<Bone>,
    /// The root's matrix in the unit's root space (column-major): bone world matrices are
    /// relative to it.
    pub root: [f32; 16],
    /// Applied after the animation, in order.
    pub overrides: Vec<Override>,
}

/// One local transform per bone.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pose {
    pub local: Vec<Trs>,
}

impl Rig {
    /// Checks that parents precede children and the overrides name bones.
    ///
    /// # Errors
    ///
    /// A broken hierarchy.
    pub fn validate(&self) -> Result<()> {
        ensure!(!self.bones.is_empty(), "{}: no bones", self.name);
        for (i, b) in self.bones.iter().enumerate() {
            ensure!(
                (i == 0 && b.parent == u32::MAX) || (i > 0 && (b.parent as usize) < i),
                "{}: bone {i} ({}) has parent {}",
                self.name,
                b.name,
                b.parent
            );
        }
        for o in &self.overrides {
            ensure!(
                (o.bone as usize) < self.bones.len(),
                "{}: override of bone {}",
                self.name,
                o.bone
            );
        }
        Ok(())
    }

    #[must_use]
    pub fn rest_pose(&self) -> Pose {
        Pose { local: self.bones.iter().map(|b| b.rest).collect() }
    }

    /// The bone whose path hash is `hash`.
    #[must_use]
    pub fn bone_by_hash(&self, hash: u32) -> Option<usize> {
        self.bones.iter().position(|b| b.hash == hash)
    }

    /// The rest pose with the skeleton's overrides applied: what a unit shows with no animation.
    #[must_use]
    pub fn posed_rest(&self) -> Pose {
        let mut p = self.rest_pose();
        self.apply_overrides(&mut p);
        p
    }

    /// Applies the skeleton's forced scales and offsets to `pose`.
    pub fn apply_overrides(&self, pose: &mut Pose) {
        for o in &self.overrides {
            let t = &mut pose.local[o.bone as usize];
            t.scale = o.scale;
            if let Some(p) = o.offset {
                t.position = p;
            }
        }
    }

    /// World matrices (relative to the unit's root space) of every bone.
    pub fn world_matrices(&self, pose: &Pose, out: &mut Vec<Mat4>) {
        out.clear();
        let root = Mat4::from_cols_array(&self.root);
        for (i, b) in self.bones.iter().enumerate() {
            let local = pose.local[i].matrix();
            let m =
                if b.parent == u32::MAX { root * local } else { out[b.parent as usize] * local };
            out.push(m);
        }
    }
}

impl Rig {
    /// [`Rig::world_matrices`] as column-major arrays.
    pub fn world_arrays(&self, pose: &Pose, out: &mut Vec<[f32; 16]>) {
        let mut m = Vec::new();
        self.world_matrices(pose, &mut m);
        out.clear();
        out.extend(m.iter().map(Mat4::to_cols_array));
    }
}

/// The skin matrices of one renderer: `pre * world[bones[i]] * bind[i]` for each of its bones,
/// appended to `out` as 16 floats each (column-major). `pre` places the unit in its root space
/// (a displacement the unit has walked).
pub fn palette(
    worlds: &[[f32; 16]],
    bones: &[u32],
    bind: &[[f32; 16]],
    pre: &[f32; 16],
    out: &mut Vec<f32>,
) {
    let pre = Mat4::from_cols_array(pre);
    for (b, bind) in bones.iter().zip(bind) {
        let m = pre * Mat4::from_cols_array(&worlds[*b as usize]) * Mat4::from_cols_array(bind);
        out.extend_from_slice(&m.to_cols_array());
    }
}

/// A clip's bindings resolved against one rig: for each transform property the bone, the
/// attribute and the first curve.
#[derive(Debug, Clone, Default)]
pub struct Binder {
    pub entries: Vec<(u32, u32, usize)>,
    /// Transform bindings whose path no bone has.
    pub unbound: usize,
}

impl Binder {
    #[must_use]
    pub fn new(clip: &Clip, rig: &Rig) -> Self {
        let mut entries = Vec::new();
        let mut unbound = 0;
        let hashes: std::collections::HashMap<u32, usize> =
            rig.bones.iter().enumerate().map(|(i, b)| (b.hash, i)).collect();
        for (b, off) in clip.bindings.iter().zip(clip.binding_offsets()) {
            if b.type_id != TYPE_TRANSFORM {
                continue;
            }
            match hashes.get(&b.path) {
                Some(&bone) => entries.push((bone as u32, b.attribute, off)),
                None => unbound += 1,
            }
        }
        Self { entries, unbound }
    }
}

/// Writes the clip's curve values (`curves`, from [`Clip::sample`]) into `pose` for every
/// property `binder` resolved.
pub fn apply_curves(binder: &Binder, curves: &[f32], pose: &mut Pose) {
    for &(bone, attribute, o) in &binder.entries {
        let t = &mut pose.local[bone as usize];
        match attribute {
            ATTR_POSITION => t.position = [curves[o], curves[o + 1], curves[o + 2]],
            ATTR_SCALE => t.scale = [curves[o], curves[o + 1], curves[o + 2]],
            ATTR_ROTATION => {
                let q = Quat::from_xyzw(curves[o], curves[o + 1], curves[o + 2], curves[o + 3]);
                t.rotation = q.normalize().to_array();
            }
            ATTR_EULER => {
                // Unity's `Quaternion.Euler`: degrees, applied about Z, then X, then Y.
                let q = Quat::from_euler(
                    EulerRot::YXZ,
                    curves[o + 1].to_radians(),
                    curves[o].to_radians(),
                    curves[o + 2].to_radians(),
                );
                t.rotation = q.to_array();
            }
            _ => {}
        }
    }
}

/// The pose a clip gives the rig at clip time `t`: the rest pose with the clip's properties
/// written over it.
pub fn sample_pose(
    clip: &Clip,
    binder: &Binder,
    rig: &Rig,
    t: f32,
    scratch: &mut Vec<f32>,
) -> Pose {
    let mut pose = rig.rest_pose();
    clip.sample(t, scratch);
    apply_curves(binder, scratch, &mut pose);
    pose
}

impl Pose {
    /// `self` at weight `1 - w`, `other` at `w`.
    pub fn blend(&mut self, other: &Pose, w: f32) {
        if w <= 0.0 {
            return;
        }
        for (a, b) in self.local.iter_mut().zip(&other.local) {
            *a = a.lerp(b, w);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clip::{Binding, Dense, Streamed};

    fn rig() -> Rig {
        let t = |x| Trs { position: [x, 0.0, 0.0], ..Trs::IDENTITY };
        Rig {
            name: "r".into(),
            bones: vec![
                Bone { name: "root".into(), hash: 0, parent: u32::MAX, rest: Trs::IDENTITY },
                Bone { name: "a".into(), hash: 11, parent: 0, rest: t(1.0) },
                Bone { name: "b".into(), hash: 22, parent: 1, rest: t(2.0) },
            ],
            root: Mat4::IDENTITY.to_cols_array(),
            overrides: Vec::new(),
        }
    }

    fn clip() -> Clip {
        // Bone `a`: a position (constant) and a rotation (constant, a quarter turn about Z).
        let s = std::f32::consts::FRAC_1_SQRT_2;
        Clip {
            name: "c".into(),
            stop: 1.0,
            looping: true,
            bindings: vec![
                Binding { path: 11, attribute: ATTR_POSITION, type_id: TYPE_TRANSFORM },
                Binding { path: 11, attribute: ATTR_ROTATION, type_id: TYPE_TRANSFORM },
                Binding { path: 999, attribute: ATTR_SCALE, type_id: TYPE_TRANSFORM },
            ],
            streamed: Streamed::default(),
            dense: Dense::default(),
            constant: vec![0.0, 3.0, 0.0, 0.0, 0.0, s, s, 1.0, 1.0, 1.0],
            ..Clip::default()
        }
    }

    #[test]
    fn world_matrices_compose_down_the_hierarchy() {
        let r = rig();
        let mut w = Vec::new();
        r.world_matrices(&r.rest_pose(), &mut w);
        assert_eq!(w[2].w_axis.truncate(), Vec3::new(3.0, 0.0, 0.0));
        r.validate().unwrap();
    }

    #[test]
    fn a_clip_overwrites_its_bones_and_leaves_the_others_at_rest() {
        let (r, c) = (rig(), clip());
        let binder = Binder::new(&c, &r);
        assert_eq!(binder.entries.len(), 2);
        assert_eq!(binder.unbound, 1);
        let mut scratch = Vec::new();
        let pose = sample_pose(&c, &binder, &r, 0.3, &mut scratch);
        assert_eq!(pose.local[1].position, [0.0, 3.0, 0.0]);
        assert_eq!(pose.local[2], r.bones[2].rest);
        // Bone b hangs off a, which turned a quarter about Z: b's +X offset points along +Y.
        let mut w = Vec::new();
        r.world_matrices(&pose, &mut w);
        let p = w[2].w_axis.truncate();
        assert!((p - Vec3::new(0.0, 5.0, 0.0)).length() < 1e-5, "{p:?}");
    }

    #[test]
    fn palettes_compose_pre_world_and_bind() {
        let r = rig();
        let mut w = Vec::new();
        r.world_arrays(&r.rest_pose(), &mut w);
        // Bone 2 rests 3 along X; its bind pose is the inverse of that, so the skin matrix is the
        // identity, and the pre-translation shows through.
        let bind = Mat4::from_translation(Vec3::new(-3.0, 0.0, 0.0)).to_cols_array();
        let pre = Mat4::from_translation(Vec3::new(0.0, 0.0, 5.0)).to_cols_array();
        let mut out = Vec::new();
        palette(&w, &[2], &[bind], &pre, &mut out);
        let m = Mat4::from_cols_array(&out.try_into().unwrap());
        assert!((m.transform_point3(Vec3::ONE) - Vec3::new(1.0, 1.0, 6.0)).length() < 1e-5);
    }

    #[test]
    fn blending_goes_the_short_way_round() {
        let a = Trs { rotation: Quat::from_rotation_z(0.2).to_array(), ..Trs::IDENTITY };
        let b = Trs {
            rotation: (-Quat::from_rotation_z(0.6)).to_array(),
            position: [2.0, 0.0, 0.0],
            ..Trs::IDENTITY
        };
        let m = a.lerp(&b, 0.5);
        let angle = Quat::from_array(m.rotation).to_euler(EulerRot::ZYX).0;
        assert!((angle - 0.4).abs() < 1e-3, "{angle}");
        assert_eq!(m.position, [1.0, 0.0, 0.0]);
    }

    #[test]
    fn overrides_force_scale_and_position() {
        let mut r = rig();
        r.overrides.push(Override {
            bone: 1,
            scale: [2.0, 2.0, 2.0],
            offset: Some([0.0, 9.0, 0.0]),
        });
        let mut p = r.rest_pose();
        r.apply_overrides(&mut p);
        assert_eq!(p.local[1].scale, [2.0; 3]);
        assert_eq!(p.local[1].position, [0.0, 9.0, 0.0]);
    }

    #[test]
    fn euler_curves_use_unitys_order() {
        // 90 degrees about Y then nothing else: a pure Y turn.
        let c = Clip {
            name: "e".into(),
            stop: 1.0,
            bindings: vec![Binding { path: 11, attribute: ATTR_EULER, type_id: TYPE_TRANSFORM }],
            constant: vec![0.0, 90.0, 0.0],
            ..Clip::default()
        };
        let r = rig();
        let binder = Binder::new(&c, &r);
        let mut s = Vec::new();
        let pose = sample_pose(&c, &binder, &r, 0.0, &mut s);
        let q = Quat::from_array(pose.local[1].rotation);
        assert!((q * Vec3::X - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5);
    }
}
