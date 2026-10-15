//! PhysX 4.1's foundation math (`PxVec3`, `PxQuat`, `PxTransform`, the intrinsics) in `f32`,
//! each operation in the source's order. The player's PhysX is built without floating-point
//! contraction (no fused multiply-add in `raycast_heightField` or its callees), so plain Rust
//! `f32` arithmetic, which never contracts, reproduces it.

use serde::{Deserialize, Serialize};

/// `PxVec3`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    #[must_use]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    #[must_use]
    pub const fn from_array(a: [f32; 3]) -> Self {
        Self::new(a[0], a[1], a[2])
    }

    #[must_use]
    pub const fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }

    #[must_use]
    pub fn plus(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }

    #[must_use]
    pub fn minus(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }

    /// `v * f`.
    #[must_use]
    pub fn scale(self, f: f32) -> Self {
        Self::new(self.x * f, self.y * f, self.z * f)
    }

    #[must_use]
    pub fn negated(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }

    /// `x * v.x + y * v.y + z * v.z`, left to right.
    #[must_use]
    pub fn dot(self, v: Self) -> f32 {
        self.x * v.x + self.y * v.y + self.z * v.z
    }

    #[must_use]
    pub fn cross(self, v: Self) -> Self {
        Self::new(
            self.y * v.z - self.z * v.y,
            self.z * v.x - self.x * v.z,
            self.x * v.y - self.y * v.x,
        )
    }

    #[must_use]
    pub fn magnitude(self) -> f32 {
        self.dot(self).sqrt()
    }

    /// `normalizeSafe()`: unchanged below `PX_NORMALIZATION_EPSILON`, else `*= 1 / mag`.
    #[must_use]
    pub fn normalize_safe(self) -> Self {
        let mag = self.magnitude();
        if mag < NORMALIZATION_EPSILON { self } else { self.scale(1.0 / mag) }
    }

    #[must_use]
    pub fn get(self, i: usize) -> f32 {
        match i {
            0 => self.x,
            1 => self.y,
            _ => self.z,
        }
    }

    pub fn set(&mut self, i: usize, v: f32) {
        match i {
            0 => self.x = v,
            1 => self.y = v,
            _ => self.z = v,
        }
    }
}

/// `PX_NORMALIZATION_EPSILON`.
pub const NORMALIZATION_EPSILON: f32 = 1e-20;

/// `PxQuat` (`x, y, z, w`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Self = Self { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    #[must_use]
    pub fn conjugate(self) -> Self {
        Self { x: -self.x, y: -self.y, z: -self.z, w: self.w }
    }

    /// `PxQuat::rotate`.
    #[must_use]
    pub fn rotate(self, v: Vec3) -> Vec3 {
        let (x, y, z, w) = (self.x, self.y, self.z, self.w);
        let vx = 2.0 * v.x;
        let vy = 2.0 * v.y;
        let vz = 2.0 * v.z;
        let w2 = w * w - 0.5;
        let dot2 = x * vx + y * vy + z * vz;
        Vec3::new(
            vx * w2 + (y * vz - z * vy) * w + x * dot2,
            vy * w2 + (z * vx - x * vz) * w + y * dot2,
            vz * w2 + (x * vy - y * vx) * w + z * dot2,
        )
    }

    /// `PxQuat::rotateInv`.
    #[must_use]
    pub fn rotate_inv(self, v: Vec3) -> Vec3 {
        let (x, y, z, w) = (self.x, self.y, self.z, self.w);
        let vx = 2.0 * v.x;
        let vy = 2.0 * v.y;
        let vz = 2.0 * v.z;
        let w2 = w * w - 0.5;
        let dot2 = x * vx + y * vy + z * vz;
        Vec3::new(
            vx * w2 - (y * vz - z * vy) * w + x * dot2,
            vy * w2 - (z * vx - x * vz) * w + y * dot2,
            vz * w2 - (x * vy - y * vx) * w + z * dot2,
        )
    }
}

/// `PxTransform` (`q`, then `p`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub q: Quat,
    pub p: Vec3,
}

impl Transform {
    pub const IDENTITY: Self = Self { q: Quat::IDENTITY, p: Vec3::ZERO };

    /// `getInverse()`: `PxTransform(q.rotateInv(-p), q.getConjugate())`.
    #[must_use]
    pub fn inverse(self) -> Self {
        Self { q: self.q.conjugate(), p: self.q.rotate_inv(self.p.negated()) }
    }

    /// `transform(v)`: `q.rotate(v) + p`.
    #[must_use]
    pub fn transform(self, v: Vec3) -> Vec3 {
        self.q.rotate(v).plus(self.p)
    }

    /// `rotate(v)`.
    #[must_use]
    pub fn rotate(self, v: Vec3) -> Vec3 {
        self.q.rotate(v)
    }
}

/// `physx::intrinsics::selectMin` (`PxMin` on floats): `a < b ? a : b`.
#[must_use]
pub fn select_min(a: f32, b: f32) -> f32 {
    if a < b { a } else { b }
}

/// `physx::intrinsics::selectMax` (`PxMax` on floats): `a > b ? a : b`.
#[must_use]
pub fn select_max(a: f32, b: f32) -> f32 {
    if a > b { a } else { b }
}

/// `physx::intrinsics::sign` (`PxSign`): `a >= 0 ? 1 : -1`.
#[must_use]
pub fn sign(a: f32) -> f32 {
    if a >= 0.0 { 1.0 } else { -1.0 }
}

/// `PxBounds3` as min and max corners.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: Vec3,
    pub max: Vec3,
}

impl Bounds {
    /// `getCenter()`: `(minimum + maximum) * 0.5f`.
    #[must_use]
    pub fn center(self) -> Vec3 {
        self.min.plus(self.max).scale(0.5)
    }

    /// `getExtents()`: `(maximum - minimum) * 0.5f`.
    #[must_use]
    pub fn extents(self) -> Vec3 {
        self.max.minus(self.min).scale(0.5)
    }
}

/// `RAYAABB_EPSILON`.
const RAYAABB_EPSILON: f32 = 0.00001;

/// `Gu::rayAABBIntersect2`: 0 on a miss, else 1 + the axis of the plane hit (1 also when the
/// origin is inside, with `t = 0`); `coord` the impact.
#[must_use]
pub fn ray_aabb_intersect2(
    minimum: Vec3,
    maximum: Vec3,
    origin: Vec3,
    dir: Vec3,
    coord: &mut Vec3,
    t: &mut f32,
) -> u32 {
    let mut inside = true;
    let mut max_t = Vec3::new(-1.0, -1.0, -1.0);
    for i in 0..3 {
        if origin.get(i) < minimum.get(i) {
            coord.set(i, minimum.get(i));
            inside = false;
            // `if(idir[i])`: the direction's bits, so -0 counts as non-zero.
            if dir.get(i).to_bits() != 0 {
                max_t.set(i, (minimum.get(i) - origin.get(i)) / dir.get(i));
            }
        } else if origin.get(i) > maximum.get(i) {
            coord.set(i, maximum.get(i));
            inside = false;
            if dir.get(i).to_bits() != 0 {
                max_t.set(i, (maximum.get(i) - origin.get(i)) / dir.get(i));
            }
        }
    }
    if inside {
        *coord = origin;
        *t = 0.0;
        return 1;
    }
    let mut which = 0;
    if max_t.get(1) > max_t.get(which) {
        which = 1;
    }
    if max_t.get(2) > max_t.get(which) {
        which = 2;
    }
    if max_t.get(which).to_bits() & 0x8000_0000 != 0 {
        return 0;
    }
    for i in 0..3 {
        if i != which {
            let c = origin.get(i) + max_t.get(which) * dir.get(i);
            coord.set(i, c);
            if c < minimum.get(i) - RAYAABB_EPSILON || c > maximum.get(i) + RAYAABB_EPSILON {
                return 0;
            }
        }
    }
    *t = max_t.get(which);
    1 + which as u32
}

/// `Gu::intersectRayAABB2` (the scalar one): `(tnear < tfar, tnear, tfar)`.
#[must_use]
pub fn intersect_ray_aabb2(
    minimum: Vec3,
    maximum: Vec3,
    ro: Vec3,
    rd: Vec3,
    max_dist: f32,
) -> (bool, f32, f32) {
    const D_EPSILON: f32 = 1e-9;
    let axis = |i: usize| {
        let inv = 1.0 / (select_max(rd.get(i).abs(), D_EPSILON) * sign(rd.get(i)));
        let t0 = (minimum.get(i) - RAYAABB_EPSILON - ro.get(i)) * inv;
        let t1 = (maximum.get(i) + RAYAABB_EPSILON - ro.get(i)) * inv;
        (select_min(t0, t1), select_max(t0, t1))
    };
    let (tx_min, tx_max) = axis(0);
    let (ty_min, ty_max) = axis(1);
    let (tz_min, tz_max) = axis(2);
    let max_of_nears = select_max(select_max(tx_min, ty_min), tz_min);
    let min_of_fars = select_min(select_min(tx_max, ty_max), tz_max);
    let tnear = select_max(max_of_nears, 0.0);
    let tfar = select_min(min_of_fars, max_dist);
    (tnear < tfar, tnear, tfar)
}

/// `GU_CULLING_EPSILON_RAY_TRIANGLE`: `FLT_EPSILON * FLT_EPSILON`.
pub const CULLING_EPSILON_RAY_TRIANGLE: f32 = f32::EPSILON * f32::EPSILON;

/// `Gu::intersectRayTriangle` (Möller–Trumbore): `Some((t, u, v))` on a hit.
#[must_use]
pub fn intersect_ray_triangle(
    orig: Vec3,
    dir: Vec3,
    vert0: Vec3,
    vert1: Vec3,
    vert2: Vec3,
    cull: bool,
    enlarge: f32,
) -> Option<(f32, f32, f32)> {
    let edge1 = vert1.minus(vert0);
    let edge2 = vert2.minus(vert0);
    let pvec = dir.cross(edge2);
    let det = edge1.dot(pvec);
    if cull {
        if det < CULLING_EPSILON_RAY_TRIANGLE {
            return None;
        }
        let tvec = orig.minus(vert0);
        let u = tvec.dot(pvec);
        let enlarge_coeff = enlarge * det;
        let uvlimit = -enlarge_coeff;
        let uvlimit2 = det + enlarge_coeff;
        if u < uvlimit || u > uvlimit2 {
            return None;
        }
        let qvec = tvec.cross(edge1);
        let v = dir.dot(qvec);
        if v < uvlimit || (u + v) > uvlimit2 {
            return None;
        }
        let t = edge2.dot(qvec);
        let inv_det = 1.0 / det;
        Some((t * inv_det, u * inv_det, v * inv_det))
    } else {
        if det.abs() < CULLING_EPSILON_RAY_TRIANGLE {
            return None;
        }
        let inv_det = 1.0 / det;
        let tvec = orig.minus(vert0);
        let u = tvec.dot(pvec) * inv_det;
        if u < -enlarge || u > 1.0 + enlarge {
            return None;
        }
        let qvec = tvec.cross(edge1);
        let v = dir.dot(qvec) * inv_det;
        if v < -enlarge || (u + v) > 1.0 + enlarge {
            return None;
        }
        let t = edge2.dot(qvec) * inv_det;
        Some((t, u, v))
    }
}
