//! Box colliders: PhysX 4.1's `raycast_box` (`GuRaycastTests.cpp`), checked against the
//! player's own function (`native_check`), and the geometry and pose Unity hands PhysX for a
//! static `BoxCollider` (`docs/poc5/physics.md`, "Boxes"; the player's code, and every recorded
//! probe of a box):
//!
//! - the static actor's pose is `setGlobalPose(TransformPoint(center), rotation)` (the
//!   player's `0x6aac44`): the position Unity's own hierarchy walk ([`transform_point`]), not
//!   the GameObject's position plus the rotated, scaled centre; the rotation the GameObject's,
//!   which PhysX normalises ([`px_normalized`]); the shape's local pose the identity;
//! - the `PxBoxGeometry`'s half extents `max(|((lossyScale * 1) * size) * 0.5|, FLT_EPSILON)`
//!   (`0x6b5280`).

use serde::{Deserialize, Serialize};

use crate::hf::RayHit;
use crate::px::{Quat, Transform, Vec3, ray_aabb_intersect2};

/// `raycast_box(geom, pose, rayOrigin, rayDir, maxDist, ePOSITION, 1, hits)`.
#[must_use]
pub fn raycast_box(
    half_extents: Vec3,
    pose: Transform,
    ray_origin: Vec3,
    ray_dir: Vec3,
    max_dist: f32,
) -> Option<RayHit> {
    let local_origin = pose.q.rotate_inv(ray_origin.minus(pose.p));
    let local_dir = pose.q.rotate_inv(ray_dir);
    let mut impact = Vec3::ZERO;
    let mut t = 0.0f32;
    if ray_aabb_intersect2(
        half_extents.negated(),
        half_extents,
        local_origin,
        local_dir,
        &mut impact,
        &mut t,
    ) == 0
    {
        return None;
    }
    if t > max_dist {
        return None;
    }
    let position = if t != 0.0 { pose.transform(impact) } else { ray_origin };
    Some(RayHit { distance: t, position, face_index: u32::MAX })
}

/// A Transform's local position, rotation (`x, y, z, w`) and scale, as the scene file holds
/// them (Unity loads them unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LocalTrs {
    pub position: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl LocalTrs {
    /// No rotation (`(0, 0, 0, 1)`, zeros of either sign) and a scale of exactly one: the
    /// ancestors the recorded boxes have (only their translation adds to a point).
    #[must_use]
    pub fn is_translation(&self) -> bool {
        self.rotation[..3].iter().all(|&x| x == 0.0)
            && self.rotation[3].to_bits() == 1.0f32.to_bits()
            && self.scale.iter().all(|&x| x.to_bits() == 1.0f32.to_bits())
    }
}

/// One level of Unity's `Transform::TransformPoint` (the arm64 player's loop at `0x6744c0`,
/// reached from the icall `Transform::TransformPoint_Injected`): `v = p * s`, then
/// `t + ((v + c0 vx) + (c1 vy + c2 vz))` with `c0..c2` the columns of the rotation matrix less
/// the identity, each entry a difference of two products (`qy (-2 qy) - qz (2 qz)`, ...), no
/// fused multiply-add.
#[must_use]
pub fn transform_point_level(l: &LocalTrs, p: [f32; 3]) -> [f32; 3] {
    let [qx, qy, qz, qw] = l.rotation;
    let s = l.scale;
    let v = [p[0] * s[0], p[1] * s[1], p[2] * s[2]];
    let c0 = [
        qy * (-2.0 * qy) - qz * (2.0 * qz),
        qx * (2.0 * qy) - qw * (-2.0 * qz),
        qw * (-2.0 * qy) - qx * (-2.0 * qz),
    ];
    let c1 = [
        qw * (-2.0 * qz) - qy * (-2.0 * qx),
        qz * (-2.0 * qz) - qx * (2.0 * qx),
        qy * (2.0 * qz) - qw * (-2.0 * qx),
    ];
    let c2 = [
        qz * (2.0 * qx) - qw * (-2.0 * qy),
        qw * (-2.0 * qx) - qz * (-2.0 * qy),
        qx * (-2.0 * qx) - qy * (2.0 * qy),
    ];
    let t = l.position;
    [0, 1, 2].map(|k| t[k] + ((v[k] + c0[k] * v[0]) + (c1[k] * v[1] + c2[k] * v[2])))
}

/// `Transform.TransformPoint(p)` for the GameObject whose local transforms from the root down
/// are `chain` (its own last): each level from the GameObject up to the root.
/// `transform_point(chain, [0; 3])` is `Transform.position` (zeros' signs aside).
#[must_use]
pub fn transform_point(chain: &[LocalTrs], p: [f32; 3]) -> [f32; 3] {
    chain.iter().rev().fold(p, |p, l| transform_point_level(l, p))
}

/// `PxQuat::getNormalized`: `s = 1 / sqrt(((x² + y²) + z²) + w²)`, each component times `s`.
#[must_use]
pub fn px_normalized(q: [f32; 4]) -> Quat {
    let [x, y, z, w] = q;
    let s = 1.0 / (x * x + y * y + z * z + w * w).sqrt();
    Quat { x: x * s, y: y * s, z: z * s, w: w * s }
}

/// A box collider as the game holds it: its GameObject's world rotation and lossy scale and
/// its layer, recorded by WrathOracle's `phys_probe` (`ph_col`; Unity's lossy scale isn't
/// ported), and the shape's world position computed from the scene's local transforms
/// ([`transform_point`]), checked against the recorded position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoxRuntime {
    /// `Transform.TransformPoint(center)`.
    pub world_center: [f32; 3],
    /// `Transform.rotation` (`x, y, z, w`) and `.lossyScale`, recorded.
    pub rotation: [f32; 4],
    pub lossy_scale: [f32; 3],
    /// `Transform.position`, recorded (for the checks).
    pub position: [f32; 3],
    /// The GameObject's layer at run time.
    pub layer: u32,
    /// The recording it came from, for reports.
    pub source: String,
}

impl BoxRuntime {
    /// The `PxBoxGeometry` half extents for the collider's `size` (the player's `0x6b5280`):
    /// `((lossyScale * (1, 1, 1)) * size) * 0.5`, its absolute value (a negative one logs
    /// "BoxColliders does not support negative scale or size"), at least `FLT_EPSILON`
    /// (`fmaxnm`).
    #[must_use]
    pub fn half_extents(&self, size: [f32; 3]) -> Vec3 {
        let h = |k: usize| ((self.lossy_scale[k] * size[k]) * 0.5).abs().max(f32::EPSILON);
        Vec3::new(h(0), h(1), h(2))
    }
}

impl BoxRuntime {
    /// Whether the mapping is established for this box (`docs/poc5/physics.md`, "Boxes"):
    /// ancestors that only translate (the recorded boxes'; Unity's `TransformPoint` is checked
    /// on deeper hierarchies too, but no recorded box sits in one), its world position the
    /// recorded one and its centre's [`transform_point`], a positive lossy scale (no recorded
    /// box has a negative one) and finite half extents. Otherwise the box keeps the
    /// conservative test.
    pub fn established(
        &self,
        chain: &[LocalTrs],
        center: [f32; 3],
        size: [f32; 3],
    ) -> Result<(), String> {
        let Some((_, ancestors)) = chain.split_last() else {
            return Err("no local transforms".into());
        };
        if !ancestors.iter().all(LocalTrs::is_translation) {
            return Err("an ancestor rotates or scales (not covered by the recordings)".into());
        }
        let same = |a: [f32; 3], b: [f32; 3]| {
            a.iter().zip(&b).all(|(x, y)| x.to_bits() == y.to_bits() || (*x == 0.0 && *y == 0.0))
        };
        if !same(transform_point(chain, [0.0; 3]), self.position) {
            return Err(format!(
                "its hierarchy places it at {:?}, the recording at {:?}",
                transform_point(chain, [0.0; 3]),
                self.position
            ));
        }
        if transform_point(chain, center).map(f32::to_bits) != self.world_center.map(f32::to_bits) {
            return Err("its world centre isn't its centre's TransformPoint".into());
        }
        if !self.lossy_scale.iter().all(|&x| x.is_finite() && x > 0.0) {
            return Err(format!(
                "lossy scale {:?} (only positive ones are recorded)",
                self.lossy_scale
            ));
        }
        let h = self.half_extents(size);
        if !(h.x.is_finite() && h.y.is_finite() && h.z.is_finite()) {
            return Err(format!("half extents {h:?}"));
        }
        // A unit quaternion as the game keeps it (PhysX refuses a pose that isn't one).
        let n2: f64 = self.rotation.iter().map(|&x| f64::from(x).powi(2)).sum();
        if !self.rotation.iter().all(|x| x.is_finite()) || (n2 - 1.0).abs() > 1e-4 {
            return Err(format!("rotation {:?}", self.rotation));
        }
        Ok(())
    }
}
