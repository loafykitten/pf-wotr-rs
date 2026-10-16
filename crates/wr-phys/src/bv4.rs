//! Triangle meshes: PhysX 4.1's BVH34 ("BV4") midphase raycast as the arm64 player runs it,
//! `Gu::raycast_triangleMesh_BV4` (`GuMidphaseBV4.cpp`) for a single closest hit, with
//! `BV4_RaycastSingle` (`GuBV4_Raycast.cpp`), its ordered Kajiya slab traversal over the
//! quantized swizzled tree (`GuBV4_Slabs_KajiyaOrdered.h`), `LeafFunction_RaycastClosest` and
//! `RayTriOverlapT`, and `computeImpactData`. Ported from the player's machine code
//! (`UnityPlayer.dylib`: the entry at `0x13e68bc`, `BV4_RaycastSingle` at `0x13e2420`, the leaf
//! at `0x13e3b0c`, `PxMeshScale * PxTransform` at `0x1325dd0`, `PxMeshScale::transform` at
//! `0x13e7230`), where the public 4.1 source has an SSE-only BV4: the player's is a NEON build
//! of it. Its slab setup takes the reciprocal with NEON's estimate and one fused Newton step
//! (`frecpe`, `frecps`), ported bit for bit below; everything else is separate `f32`
//! multiplies and adds (no fused multiply-add), in the order of the machine code. Checked
//! against the player's own function on generated rays (`native_check`).

use serde::{Deserialize, Serialize};

use crate::px::{Quat, Transform, Vec3};

/// A cooked BVH34 triangle mesh as `Gu::MeshFactory` loads it: the welded vertices, the
/// triangles in the tree's order (cooking remaps them so each leaf's triangles are
/// consecutive), `mGeomEpsilon` as cooked, and the tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bv4Mesh {
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    /// `TriangleMesh::mGeomEpsilon`: the triangle test's barycentric enlargement.
    pub geom_epsilon: f32,
    pub tree: Bv4Tree,
}

/// `Gu::BV4Tree` as `BV4Tree::load` reads it (quantized, swizzled nodes).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bv4Tree {
    /// `mLocalBounds`: the mesh bounds' centre and the magnitude of their extents.
    pub center: [f32; 3],
    pub extents_magnitude: f32,
    /// `mInitData`: the root's child data.
    pub init_data: u32,
    /// `mCenterOrMinCoeff`, `mExtentsOrMaxCoeff`: the dequantization coefficients.
    pub min_coeff: [f32; 3],
    pub max_coeff: [f32; 3],
    /// `BVDataPackedQ` nodes, 16 bytes each as four little-endian words (`mNbNodes` of them);
    /// four consecutive ones form a `BVDataSwizzledQ`: `mX[4]`, `mY[4]`, `mZ[4]`, `mData[4]`,
    /// each axis word holding a child's quantized minimum (low half) and maximum (high half).
    pub nodes: Vec<[u32; 4]>,
}

/// Why a cooked mesh can't be queried as the player would.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidMesh(pub String);

impl std::fmt::Display for InvalidMesh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for InvalidMesh {}

/// `GU_BV4_STACK_SIZE`.
const STACK_SIZE: usize = 256;

/// Deliberate errors the native check switches on one at a time to show it catches them.
#[cfg(test)]
pub(crate) mod mutation {
    use std::cell::Cell;

    thread_local! {
        pub(crate) static ACTIVE: Cell<u32> = const { Cell::new(0) };
    }

    pub(crate) fn on(m: u32) -> bool {
        ACTIVE.with(|a| a.get() == m)
    }

    /// What each one changes.
    pub(crate) const ALL: [(u32, &str); 10] = [
        (1, "the slabs' reciprocal frecpe's raw estimate (no steps)"),
        (10, "the slabs' reciprocal exact instead of frecpe and its steps"),
        (2, "no barycentric enlargement (geomEpsilon 0)"),
        (3, "a later triangle at the same distance wins (<= for <)"),
        (4, "children pushed in the reverse order"),
        (5, "the determinant with a fused multiply-add"),
        (6, "no clipRay"),
        (7, "nodes beyond the closest hit not culled"),
        (8, "the scaled path without its 1e-3"),
        (9, "an identity scale through the scaled path"),
    ];
}

#[cfg(not(test))]
mod mutation {
    #[inline(always)]
    pub(crate) fn on(_: u32) -> bool {
        false
    }
}

impl Bv4Mesh {
    /// Checks every structural assumption the raycast relies on (indices in range, the tree a
    /// tree whose traversal stack stays within the player's 256 entries, every leaf's
    /// triangles in range); the player would read out of bounds where this refuses.
    pub fn validate(&self) -> Result<(), InvalidMesh> {
        let bad = |s: String| Err(InvalidMesh(s));
        let nv = self.vertices.len();
        let nt = self.triangles.len();
        if self.triangles.iter().flatten().any(|&i| i as usize >= nv) {
            return bad("triangle index out of range".into());
        }
        if !self.vertices.iter().flatten().all(|x| x.is_finite()) {
            return bad("a vertex isn't finite".into());
        }
        let t = &self.tree;
        if t.nodes.is_empty() {
            // `doBruteForceTests(nbTris)`: the leaf reads `nbTris & 15` triangles from 0.
            if nt == 0 || nt >= 16 {
                return bad(format!("no tree for {nt} triangles"));
            }
            return Ok(());
        }
        if !t.nodes.len().is_multiple_of(4) {
            return bad(format!("{} packed nodes, not whole swizzled ones", t.nodes.len()));
        }
        let mut seen = vec![false; t.nodes.len() / 4];
        // (child data, the stack depth when it's pushed)
        let mut todo = vec![(t.init_data, 1usize)];
        while let Some((data, depth)) = todo.pop() {
            let o = (data >> 11) as usize;
            if !o.is_multiple_of(4) || o + 3 >= t.nodes.len() {
                return bad(format!("child offset {o} outside the tree"));
            }
            if std::mem::replace(&mut seen[o / 4], true) {
                return bad(format!("node {o} reached twice"));
            }
            let node_type = (data >> 1) & 3;
            if node_type == 3 {
                return bad(format!("node {o} with child type 3"));
            }
            let children = 2 + node_type as usize;
            // A node pushes at most its children, popped one: the stack grows by at most 3.
            if depth - 1 + children > STACK_SIZE {
                return bad("the traversal stack could overflow".into());
            }
            for i in 0..children {
                let d = t.nodes[o + 3][i];
                if d & 1 != 0 {
                    let prim = d >> 1;
                    let (count, start) = ((prim & 15) as usize, (prim >> 4) as usize);
                    if count == 0 || start + count > nt {
                        return bad(format!("leaf {start}+{count} outside {nt} triangles"));
                    }
                } else {
                    todo.push((d, depth - 1 + children));
                }
            }
        }
        Ok(())
    }
}

// --- The arm64 floating-point operations the machine code uses -------------------------

/// The NZCV flags `fcmp a, b` sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Nzcv(u8);

impl Nzcv {
    const ZERO: Self = Self(0b0000);
    const N: Self = Self(0b1000);

    fn fcmp(a: f32, b: f32) -> Self {
        if a.is_nan() || b.is_nan() {
            Self(0b0011)
        } else if a < b {
            Self(0b1000)
        } else if a == b {
            Self(0b0110)
        } else {
            Self(0b0010)
        }
    }
    fn n(self) -> bool {
        self.0 & 8 != 0
    }
    fn z(self) -> bool {
        self.0 & 4 != 0
    }
    fn v(self) -> bool {
        self.0 & 1 != 0
    }
    /// `pl`: N clear.
    fn pl(self) -> bool {
        !self.n()
    }
    /// `le`: Z set or N != V.
    fn le(self) -> bool {
        self.z() || self.n() != self.v()
    }
    /// `gt`: Z clear and N == V.
    fn gt(self) -> bool {
        !self.z() && self.n() == self.v()
    }
}

/// `fcmp a, b; b.mi`: ordered and less.
fn mi(a: f32, b: f32) -> bool {
    Nzcv::fcmp(a, b).n()
}

/// `fcmgt`: ordered and greater.
fn fcmgt(a: f32, b: f32) -> bool {
    a > b
}

/// `fmin`: NaN if either is NaN, -0 below +0.
fn arm_fmin(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return f32::NAN;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() { a } else { b };
    }
    if a < b { a } else { b }
}

/// `fmax`: NaN if either is NaN, +0 above -0.
fn arm_fmax(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        return f32::NAN;
    }
    if a == 0.0 && b == 0.0 {
        return if a.is_sign_negative() { b } else { a };
    }
    if a > b { a } else { b }
}

/// `frecpe` (single precision, the ARM ARM's `FPRecipEstimate` with `FPCR.FZ` clear and
/// round to nearest): an 8-bit estimate of `1 / x`.
#[must_use]
pub fn frecpe(x: f32) -> f32 {
    let bits = x.to_bits();
    let sign = bits & 0x8000_0000;
    if x.is_nan() {
        return f32::from_bits(bits | 0x0040_0000);
    }
    if x.is_infinite() {
        return f32::from_bits(sign);
    }
    if x == 0.0 {
        return f32::from_bits(sign | 0x7f80_0000);
    }
    if x.abs() < 2f32.powi(-128) {
        // Overflow, round to nearest: infinity.
        return f32::from_bits(sign | 0x7f80_0000);
    }
    let mut exp = i32::try_from((bits >> 23) & 0xff).expect("8 bits");
    // `fraction`, as the 52-bit double-precision fraction field.
    let mut fraction = u64::from(bits & 0x7f_ffff) << 29;
    if exp == 0 {
        if fraction & (1 << 51) == 0 {
            exp = -1;
            fraction = (fraction & ((1 << 50) - 1)) << 2;
        } else {
            fraction = (fraction & ((1 << 51) - 1)) << 1;
        }
    }
    let scaled = 256 + ((fraction >> 44) & 0xff);
    let mut result_exp = 253 - exp;
    // RecipEstimate: `a` in 256..512 for 0.5..1.0.
    let a = scaled * 2 + 1;
    let b = (1u64 << 19) / a;
    let r = b.div_ceil(2);
    debug_assert!((256..512).contains(&r));
    let mut fraction = (r & 0xff) << 44;
    if result_exp == 0 {
        fraction = (1 << 51) | (fraction >> 1);
    } else if result_exp == -1 {
        fraction = (1 << 50) | (fraction >> 2);
        result_exp = 0;
    }
    let e = u32::try_from(result_exp).expect("in range") & 0xff;
    f32::from_bits(sign | e << 23 | u32::try_from(fraction >> 29).expect("23 bits"))
}

/// `frecps`: `2 - a * b` fused (one rounding); 2 for an infinity times a zero.
#[must_use]
pub fn frecps(a: f32, b: f32) -> f32 {
    if (a.is_infinite() && b == 0.0) || (a == 0.0 && b.is_infinite()) {
        return 2.0;
    }
    (-a).mul_add(b, 2.0)
}

// --- Matrices ---------------------------------------------------------------------------

/// `PxMat44` (columns; the fourth components unused here).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat44 {
    pub c: [Vec3; 4],
}

impl Mat44 {
    /// `PxMat44(PxQuat)` as the player builds it (translation zero).
    #[must_use]
    pub fn from_quat(q: Quat) -> Self {
        let (x, y, z, w) = (q.x, q.y, q.z, q.w);
        let x2 = x + x;
        let y2 = y + y;
        let z2 = z + z;
        let xx = x2 * x;
        let yy = y * y2;
        let zz = z * z2;
        let xy = x2 * y;
        let xz = x2 * z;
        let xw = x2 * w;
        let yz = y2 * z;
        let yw = y2 * w;
        let zw = z2 * w;
        let one_xx = 1.0 - xx;
        Self {
            c: [
                Vec3::new((1.0 - yy) - zz, xy + zw, xz - yw),
                Vec3::new(xy - zw, one_xx - zz, yz + xw),
                Vec3::new(xz + yw, yz - xw, one_xx - yy),
                Vec3::ZERO,
            ],
        }
    }
}

/// `Cm::Matrix34` (three columns and a translation).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix34 {
    pub m: [Vec3; 3],
    pub p: Vec3,
}

impl Matrix34 {
    /// `transform(v)`: `((m0 * v.x + m1 * v.y) + m2 * v.z) + p`.
    #[must_use]
    pub fn transform(&self, v: Vec3) -> Vec3 {
        self.rotate(v).plus(self.p)
    }

    /// `rotate(v)`: `(m0 * v.x + m1 * v.y) + m2 * v.z`.
    #[must_use]
    pub fn rotate(&self, v: Vec3) -> Vec3 {
        self.m[0].scale(v.x).plus(self.m[1].scale(v.y)).plus(self.m[2].scale(v.z))
    }
}

/// `PxMat33(PxQuat)`'s columns.
fn mat33_from_quat(q: Quat) -> [Vec3; 3] {
    let c = Mat44::from_quat(q).c;
    [c[0], c[1], c[2]]
}

/// `PxMat33 * PxMat33`: each column of `b` through `a`.
fn mat33_mul(a: &[Vec3; 3], b: &[Vec3; 3]) -> [Vec3; 3] {
    let t = |v: Vec3| a[0].scale(v.x).plus(a[1].scale(v.y)).plus(a[2].scale(v.z));
    [t(b[0]), t(b[1]), t(b[2])]
}

/// `PxMeshScale`: a scale along the axes of `rotation`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MeshScale {
    pub scale: Vec3,
    pub rotation: Quat,
}

impl MeshScale {
    /// `isIdentity()`: the scale exactly one (the rotation isn't looked at).
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.scale.x == 1.0 && self.scale.y == 1.0 && self.scale.z == 1.0
    }

    /// `getInverse()`.
    #[must_use]
    pub fn inverse(&self) -> Self {
        Self {
            scale: Vec3::new(1.0 / self.scale.x, 1.0 / self.scale.y, 1.0 / self.scale.z),
            rotation: self.rotation,
        }
    }

    /// `toMat33()`: `R^T`'s columns scaled, times `R`.
    #[must_use]
    pub fn to_mat33(&self) -> [Vec3; 3] {
        let r = mat33_from_quat(self.rotation);
        let s = self.scale;
        let t = [
            Vec3::new(r[0].x, r[1].x, r[2].x).scale(s.x),
            Vec3::new(r[0].y, r[1].y, r[2].y).scale(s.y),
            Vec3::new(r[0].z, r[1].z, r[2].z).scale(s.z),
        ];
        mat33_mul(&t, &r)
    }

    /// `transform(v)`: `rotation.rotateInv(scale.multiply(rotation.rotate(v)))`.
    #[must_use]
    pub fn transform(&self, v: Vec3) -> Vec3 {
        let r = self.rotation.rotate(v);
        let m = Vec3::new(self.scale.x * r.x, self.scale.y * r.y, self.scale.z * r.z);
        self.rotation.rotate_inv(m)
    }

    /// `PxMeshScale * PxTransform`: `Matrix34(toMat33() * PxMat33(t.q), toMat33() * t.p)`.
    #[must_use]
    pub fn times(&self, t: Transform) -> Matrix34 {
        let s = self.to_mat33();
        let r = mat33_mul(&s, &mat33_from_quat(t.q));
        let p = s[0].scale(t.p.x).plus(s[1].scale(t.p.y)).plus(s[2].scale(t.p.z));
        Matrix34 { m: r, p }
    }
}

// --- The raycast ----------------------------------------------------------------------

/// `QUERY_MODIFIER_ANY_HIT`, `QUERY_MODIFIER_DOUBLE_SIDED`, `QUERY_MODIFIER_MESH_BOTH_SIDES`.
const ANY_HIT: u32 = 1;

/// A hit as `BV4_RaycastSingle` reports it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshHit {
    pub distance: f32,
    pub position: Vec3,
    pub face_index: u32,
    pub u: f32,
    pub v: f32,
}

/// `RayParams` (the parts the closest-hit raycast uses).
struct Params<'m> {
    mesh: &'m Bv4Mesh,
    dir: Vec3,
    origin: Vec3,
    backface_culling: bool,
    /// `mStabbedFace`: distance, triangle, u, v.
    distance: f32,
    triangle: Option<u32>,
    u: f32,
    v: f32,
    p: [Vec3; 3],
}

impl Params<'_> {
    /// `LeafFunction_RaycastClosest::doLeafTest`.
    fn leaf(&mut self, prim: u32) {
        let mut to_go = (prim & 15).wrapping_sub(1);
        let mut index = prim >> 4;
        loop {
            self.triangle_test(index);
            index = index.wrapping_add(1);
            if to_go == 0 {
                break;
            }
            to_go -= 1;
        }
    }

    /// `RayTriOverlapT` and the closest-hit update, as the leaf's machine code runs them.
    fn triangle_test(&mut self, index: u32) {
        let mesh = self.mesh;
        let t = mesh.triangles[index as usize];
        let v0 = Vec3::from_array(mesh.vertices[t[0] as usize]);
        let v1 = Vec3::from_array(mesh.vertices[t[1] as usize]);
        let v2 = Vec3::from_array(mesh.vertices[t[2] as usize]);
        let eps = if mutation::on(2) { 0.0 } else { mesh.geom_epsilon };
        let d = self.dir;
        let edge1 = v1.minus(v0);
        let edge2 = v2.minus(v0);
        let pvec = d.cross(edge2);
        let det = if mutation::on(5) {
            edge1.z.mul_add(pvec.z, edge1.x * pvec.x + edge1.y * pvec.y)
        } else {
            edge1.dot(pvec)
        };
        let (dist, u, v);
        if self.backface_culling {
            if mi(det, crate::px::CULLING_EPSILON_RAY_TRIANGLE) {
                return;
            }
            let tvec = self.origin.minus(v0);
            let uu = tvec.dot(pvec);
            let enlarge = det * eps;
            let lim1 = -(det * eps);
            let lim2 = det + enlarge;
            let c1 = if Nzcv::fcmp(uu, lim1).pl() { Nzcv::fcmp(uu, lim2) } else { Nzcv::ZERO };
            let qvec = tvec.cross(edge1);
            let vv = d.dot(qvec);
            let uv = uu + vv;
            let c2 = if c1.le() { Nzcv::fcmp(vv, lim1) } else { Nzcv::N };
            let c3 = if c2.pl() { Nzcv::fcmp(uv, lim2) } else { Nzcv::ZERO };
            if c3.gt() {
                return;
            }
            let dd = edge2.dot(qvec);
            if mi(dd, 0.0) {
                return;
            }
            let inv = 1.0 / det;
            dist = inv * dd;
            u = inv * uu;
            v = inv * vv;
        } else {
            if mi(det.abs(), crate::px::CULLING_EPSILON_RAY_TRIANGLE) {
                return;
            }
            let inv = 1.0 / det;
            let tvec = self.origin.minus(v0);
            let uu = inv * tvec.dot(pvec);
            let lim1 = -eps;
            let lim2 = eps + 1.0;
            let c1 = if Nzcv::fcmp(uu, lim1).pl() { Nzcv::fcmp(uu, lim2) } else { Nzcv::ZERO };
            let qvec = tvec.cross(edge1);
            let vv = inv * d.dot(qvec);
            let uv = uu + vv;
            let c2 = if c1.le() { Nzcv::fcmp(vv, lim1) } else { Nzcv::N };
            let c3 = if c2.pl() { Nzcv::fcmp(uv, lim2) } else { Nzcv::ZERO };
            if c3.gt() {
                return;
            }
            let dd = inv * edge2.dot(qvec);
            if mi(dd, 0.0) {
                return;
            }
            dist = dd;
            u = uu;
            v = vv;
        }
        // `if(StabbedFace.distance < params->mStabbedFace.mDistance)`.
        let closer = if mutation::on(3) { dist <= self.distance } else { mi(dist, self.distance) };
        if !closer {
            return;
        }
        self.p = [v0, v1, v2];
        self.triangle = Some(index);
        self.distance = dist;
        self.u = u;
        self.v = v;
    }

    /// `BV4_ProcessStreamKajiyaOrderedQ<0, LeafFunction_RaycastClosest>`.
    fn traverse_ordered(&mut self) {
        let tree = &self.mesh.tree;
        let bit = |x: f32| x.to_bits() >> 31;
        let dir_mask =
            1u32 << (3 + (bit(self.dir.z) | (bit(self.dir.y) << 1) | (bit(self.dir.x) << 2)));
        // SLABS_INIT.
        let mut inv_d = [0f32; 3];
        let mut p_inv_d = [0f32; 3];
        let (dir, orig) = (self.dir.to_array(), self.origin.to_array());
        for k in 0..3 {
            let sign = dir[k].to_bits() & 0x8000_0000;
            let m = arm_fmax(dir[k].abs(), 1e-9);
            let dd = f32::from_bits(sign | m.to_bits());
            let est = frecpe(dd);
            let inv = est * frecps(est, dd);
            let inv = if mutation::on(1) {
                est
            } else if mutation::on(10) {
                1.0 / dd
            } else {
                inv * (2.0 - inv * dd)
            };
            inv_d[k] = inv;
            p_inv_d[k] = 0.0 - orig[k] * inv;
        }
        let mut max_t = self.distance;
        let mut stack = [0u32; STACK_SIZE];
        stack[0] = tree.init_data;
        let mut nb = 1usize;
        loop {
            nb -= 1;
            let child_data = stack[nb];
            let o = (child_data >> 11) as usize;
            let node = &tree.nodes[o..o + 4];
            let mut fails = 0u32;
            // Lane `i` of each axis word: the four children side by side, as the SIMD test.
            #[allow(clippy::needless_range_loop)]
            for i in 0..4 {
                let mut near = [0f32; 3];
                let mut far = [0f32; 3];
                for k in 0..3 {
                    let w = node[k][i] as i32;
                    #[allow(clippy::cast_precision_loss)]
                    let (qmin, qmax) = (((w << 16) >> 16) as f32, (w >> 16) as f32);
                    let mn = tree.min_coeff[k] * qmin;
                    let mx = tree.max_coeff[k] * qmax;
                    let t0 = p_inv_d[k] + inv_d[k] * mn;
                    let t1 = p_inv_d[k] + inv_d[k] * mx;
                    near[k] = arm_fmin(t0, t1);
                    far[k] = arm_fmax(t0, t1);
                }
                let max_of_nears = arm_fmax(arm_fmax(near[0], near[1]), near[2]);
                let min_of_fars = arm_fmin(arm_fmin(far[0], far[1]), far[2]);
                if fcmgt(1e-9, min_of_fars)
                    || (fcmgt(max_of_nears, max_t) && !mutation::on(7))
                    || fcmgt(max_of_nears, min_of_fars)
                {
                    fails |= 1 << i;
                }
            }
            if fails != 15 {
                let data = node[3];
                let node_type = (child_data >> 1) & 3;
                let mut code2 = 0u32;
                for i in (0..4).rev() {
                    let present = match i {
                        3 => node_type > 1,
                        2 => node_type > 0,
                        _ => true,
                    };
                    if !present || fails & (1 << i) != 0 {
                        continue;
                    }
                    if data[i] & 1 != 0 {
                        self.leaf(data[i] >> 1);
                        max_t = self.distance;
                    } else {
                        code2 |= 1 << i;
                    }
                }
                if code2 != 0 {
                    let pns = |i: usize| data[i] & dir_mask != 0;
                    let order: [usize; 4] = match (pns(0), pns(1), pns(2)) {
                        (true, true, true) => [3, 2, 1, 0],
                        (true, true, false) => [2, 3, 1, 0],
                        (true, false, true) => [3, 2, 0, 1],
                        (true, false, false) => [2, 3, 0, 1],
                        (false, true, true) => [1, 0, 3, 2],
                        (false, true, false) => [1, 0, 2, 3],
                        (false, false, true) => [0, 1, 3, 2],
                        (false, false, false) => [0, 1, 2, 3],
                    };
                    let mut order = order;
                    if mutation::on(4) {
                        order.reverse();
                    }
                    for i in order {
                        if code2 & (1 << i) != 0 {
                            stack[nb] = data[i];
                            nb += 1;
                        }
                    }
                }
            }
            if nb == 0 {
                break;
            }
        }
    }
}

/// `BV4_RaycastSingle(origin, dir, tree, world, hit, maxDist, geomEpsilon, flags)` for the
/// closest hit (`flags` without `QUERY_MODIFIER_ANY_HIT`): in the mesh's space, or through
/// `world` (`PxMat44`, rotation and translation) when given.
#[must_use]
pub fn raycast_single(
    mesh: &Bv4Mesh,
    origin: Vec3,
    dir: Vec3,
    world: Option<&Mat44>,
    max_dist: f32,
    flags: u32,
) -> Option<MeshHit> {
    assert_eq!(flags & ANY_HIT, 0, "any-hit raycasts aren't ported");
    // computeLocalRay: inverseRotate and inverseTransform.
    let (local_dir, local_origin) = match world {
        Some(m) => {
            let [c0, c1, c2, c3] = m.c;
            let inv_rot = |p: Vec3| {
                Vec3::new(
                    c0.x * p.x + c0.y * p.y + c0.z * p.z,
                    c1.x * p.x + c1.y * p.y + c1.z * p.z,
                    c2.x * p.x + c2.y * p.y + c2.z * p.z,
                )
            };
            let t = Vec3::new(
                c0.x * c3.x + c0.y * c3.y + c0.z * c3.z,
                c1.x * c3.x + c1.y * c3.y + c1.z * c3.z,
                c2.x * c3.x + c2.y * c3.y + c2.z * c3.z,
            );
            (inv_rot(dir), inv_rot(origin).minus(t))
        }
        None => (dir, origin),
    };
    // clipRay.
    let tree = &mesh.tree;
    let center = Vec3::from_array(tree.center);
    let mag = tree.extents_magnitude;
    let dpc = center.dot(local_dir);
    let dp_min = dpc - mag;
    let dp_max = mag + dpc;
    let dp_o = local_origin.dot(local_dir);
    let box_length = mag + mag;
    let (a, b) = ((dp_min - dp_o).abs(), (dp_max - dp_o).abs());
    let dist_to_box = if mi(a, b) { a } else { b };
    let clip = (box_length + box_length) + dist_to_box;
    let max_dist = if Nzcv::fcmp(clip, max_dist).gt() || mutation::on(6) { max_dist } else { clip };
    let mut p = Params {
        mesh,
        dir: local_dir,
        origin: local_origin,
        backface_culling: flags & 6 == 0,
        distance: max_dist,
        triangle: None,
        u: 0.0,
        v: 0.0,
        p: [Vec3::ZERO; 3],
    };
    if tree.nodes.is_empty() {
        let n = u32::try_from(mesh.triangles.len()).expect("validated: under 16");
        p.leaf(n);
    } else {
        p.traverse_ordered();
    }
    // computeImpactData.
    let face_index = p.triangle?;
    let (u, v) = (p.u, p.v);
    let w = (1.0 - u) - v;
    let [p0, p1, p2] = p.p;
    let local = p1.scale(u).plus(p2.scale(v)).plus(p0.scale(w));
    let position = match world {
        Some(m) => m.c[0]
            .scale(local.x)
            .plus(m.c[1].scale(local.y))
            .plus(m.c[2].scale(local.z))
            .plus(m.c[3]),
        None => local,
    };
    Some(MeshHit { distance: p.distance, position, face_index, u, v })
}

/// `setupWorldMatrix`: none for the identity pose (by bits), else the pose's rotation and
/// translation.
fn world_matrix(pose: Transform) -> Option<Mat44> {
    let q = [pose.q.x, pose.q.y, pose.q.z, pose.q.w].map(f32::to_bits);
    let mut m = Mat44 {
        c: [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::ZERO,
        ],
    };
    let mut identity = true;
    if q != [0, 0, 0, 0x3f80_0000] {
        m = Mat44::from_quat(pose.q);
        identity = false;
    }
    if pose.p.to_array().iter().any(|x| x.to_bits() != 0) {
        m.c[3] = pose.p;
        identity = false;
    }
    (!identity).then_some(m)
}

/// `PxHitFlag::eMESH_ANY`, `eMESH_BOTH_SIDES`.
const MESH_ANY: u16 = 1 << 6;
const MESH_BOTH_SIDES: u16 = 1 << 7;

/// `Gu::raycast_triangleMesh_BV4` with `maxHits` 1: the closest hit of a ray (world space,
/// unit direction) on the mesh under `pose` and `scale`; distance and position as reported.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn raycast_triangle_mesh(
    mesh: &Bv4Mesh,
    scale: &MeshScale,
    double_sided: bool,
    pose: Transform,
    origin: Vec3,
    dir: Vec3,
    max_dist: f32,
    hit_flags: u16,
) -> Option<MeshHit> {
    let both_sides = double_sided || hit_flags & MESH_BOTH_SIDES != 0;
    let any = u32::from(hit_flags & MESH_ANY != 0);
    let flags = if both_sides { 2 | any } else { any };
    if scale.is_identity() && !mutation::on(9) {
        return raycast_single(mesh, origin, dir, world_matrix(pose).as_ref(), max_dist, flags);
    }
    // Scaling: the ray to vertex space.
    let w2v = scale.inverse().times(pose.inverse());
    let orig = w2v.transform(origin);
    let mut d = w2v.rotate(dir);
    let mag = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
    let coeff = 1.0 / mag;
    if Nzcv::fcmp(mag, 0.0).gt() {
        d = d.scale(coeff);
    }
    let max = if mutation::on(8) { mag * max_dist } else { mag * max_dist + 1e-3 };
    let mut hit = raycast_single(mesh, orig, d, None, max, flags)?;
    hit.distance *= coeff;
    hit.position = pose.transform(scale.transform(hit.position));
    if (scale.scale.x * scale.scale.y) * scale.scale.z < 0.0 {
        std::mem::swap(&mut hit.u, &mut hit.v);
    }
    Some(hit)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tree of one swizzled node over two leaves of one triangle each.
    fn two_leaves() -> Bv4Mesh {
        // Children 0 and 1 (node type 0: two children), quantized boxes [0, 2] on every axis
        // with coefficients 1; leaves: triangle 0 and triangle 1 (count 1 each).
        let q = 2u32 << 16; // min 0, max 2
        let leaf = |t: u32| ((t << 4 | 1) << 1) | 1;
        Bv4Mesh {
            vertices: vec![[0.0, 1.0, 0.0], [0.0, 1.0, 2.0], [2.0, 1.0, 0.0], [2.0, 1.0, 2.0]],
            triangles: vec![[0, 1, 2], [1, 3, 2]],
            geom_epsilon: 2.0 * 2f32.powi(-22),
            tree: Bv4Tree {
                center: [1.0, 1.0, 1.0],
                extents_magnitude: 3f32.sqrt(),
                init_data: 0,
                min_coeff: [1.0; 3],
                max_coeff: [1.0; 3],
                nodes: vec![[q; 4], [q; 4], [q; 4], [leaf(0), leaf(1), 0, 0]],
            },
        }
    }

    #[test]
    fn a_tree_is_traversed_to_the_closest_hit() {
        let m = two_leaves();
        m.validate().unwrap();
        let down = Vec3::new(0.0, -1.0, 0.0);
        let h = raycast_single(&m, Vec3::new(1.5, 5.0, 1.5), down, None, 100.0, 0).unwrap();
        assert_eq!((h.distance, h.face_index, h.position), (4.0, 1, Vec3::new(1.5, 1.0, 1.5)));
        let h = raycast_single(&m, Vec3::new(0.5, 5.0, 0.5), down, None, 100.0, 0).unwrap();
        assert_eq!(h.face_index, 0);
        assert_eq!(raycast_single(&m, Vec3::new(2.5, 5.0, 0.5), down, None, 100.0, 0), None);
        // Back faces only with both sides.
        let up = Vec3::new(0.0, 1.0, 0.0);
        assert_eq!(raycast_single(&m, Vec3::new(0.5, -5.0, 0.5), up, None, 100.0, 0), None);
        assert!(raycast_single(&m, Vec3::new(0.5, -5.0, 0.5), up, None, 100.0, 2).is_some());
    }

    #[test]
    fn malformed_trees_are_refused() {
        let good = two_leaves();
        let mut bad = good.clone();
        bad.tree.init_data = 4 << 11; // past the nodes
        assert!(bad.validate().is_err(), "child outside the tree");
        let mut bad = good.clone();
        bad.tree.nodes[3][1] = 0; // child 1 back to the root: a cycle
        assert!(bad.validate().is_err(), "a cycle");
        let mut bad = good.clone();
        bad.tree.nodes[3][1] = ((2 << 4 | 1) << 1) | 1; // triangle 2 of 2
        assert!(bad.validate().is_err(), "a leaf past the triangles");
        let mut bad = good.clone();
        bad.tree.nodes[3][0] = 1; // a leaf of no triangles
        assert!(bad.validate().is_err(), "an empty leaf");
        let mut bad = good;
        bad.tree.nodes.clear();
        bad.triangles = vec![[0, 1, 2]; 16];
        assert!(bad.validate().is_err(), "brute force over 16 triangles");
    }

    #[test]
    fn frecpe_known_values() {
        // 1 / 1 estimates to 511/512; the sign is kept; infinities and zeros swap.
        assert_eq!(frecpe(1.0), 0.998_046_9);
        assert_eq!(frecpe(-2.0), -0.499_023_44);
        assert_eq!(frecpe(f32::INFINITY), 0.0);
        assert_eq!(frecpe(-0.0), f32::NEG_INFINITY);
        assert_eq!(frecps(0.0, f32::INFINITY), 2.0);
        assert_eq!(frecps(0.5, 2.0), 1.0);
    }
}
