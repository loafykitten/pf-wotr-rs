//! PhysX 4.1's `raycast_box` (`GuRaycastTests.cpp`), checked against the player's own function
//! (`native_check`). The scene queries don't use it yet: how Unity turns a `BoxCollider` into a
//! `PxBoxGeometry` and a shape pose isn't verified (`docs/poc5/physics.md`).

use crate::hf::RayHit;
use crate::px::{Transform, Vec3, ray_aabb_intersect2};

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
