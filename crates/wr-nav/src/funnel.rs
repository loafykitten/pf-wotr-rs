//! `FunnelModifier` (unwrap on, no split at portals): `Funnel.SplitIntoParts`,
//! `ConstructFunnelPortals` with `TriangleMeshNode.GetPortal` and `GridNode.GetPortal`,
//! `Unwrap`, `FixFunnel` and the funnel itself.
//!
//! The corners come out as portal vertices or endpoints, so they are exact as long as
//! the same corners are chosen. `Unwrap` uses Unity's native
//! `Quaternion.FromToRotation`, ported from the arm64 player's machine code and checked
//! against it bit for bit (`native_check`): which corner wins depends on its last bits.

use super::cs::{self, V3, d, r};
use super::world::{Kind, NodeId, World};

type Q = [f32; 4];

/// `Quaternion * Quaternion` (managed).
fn qmul(a: Q, b: Q) -> Q {
    let [ax, ay, az, aw] = a.map(d);
    let [bx, by, bz, bw] = b.map(d);
    [
        r(aw * bx + ax * bw + ay * bz - az * by),
        r(aw * by + ay * bw + az * bx - ax * bz),
        r(aw * bz + az * bw + ax * by - ay * bx),
        r(aw * bw - ax * bx - ay * by - az * bz),
    ]
}

/// `Quaternion * Vector3` (managed).
fn qrot(q: Q, p: V3) -> V3 {
    let n1 = q[0] * 2.0;
    let n2 = q[1] * 2.0;
    let n3 = q[2] * 2.0;
    let n4 = q[0] * n1;
    let n5 = q[1] * n2;
    let n6 = q[2] * n3;
    let n7 = q[0] * n2;
    let n8 = q[0] * n3;
    let n9 = q[1] * n3;
    let n10 = q[3] * n1;
    let n11 = q[3] * n2;
    let n12 = q[3] * n3;
    let [px, py, pz] = p.map(d);
    [
        r((1.0 - (d(n5) + d(n6))) * px + (d(n7) - d(n12)) * py + (d(n8) + d(n11)) * pz),
        r((d(n7) + d(n12)) * px + (1.0 - (d(n4) + d(n6))) * py + (d(n9) - d(n10)) * pz),
        r((d(n8) - d(n11)) * px + (d(n9) + d(n10)) * py + (1.0 - (d(n4) + d(n5))) * pz),
    ]
}

/// `FromToRotation`'s identity threshold on the dot product, a double in the player:
/// `1.0 - (double)1e-6f`.
const FROM_TO_PARALLEL: f64 = 1.0 - 1e-6_f32 as f64;

/// Unity's native `Quaternion.FromToRotation` (`FromToQuaternionSafe`), ported from the
/// arm64 player's machine code: single-precision operations in this order, none fused.
pub(crate) fn from_to_rotation(from: V3, to: V3) -> Q {
    let nf = cs::native_magnitude(from);
    let nt = cs::native_magnitude(to);
    if nf < 1e-5 || nt < 1e-5 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    matrix_to_quaternion(from_to_matrix(cs::div(from, nf), cs::div(to, nt)))
}

/// The player's `Matrix3x3f::SetFromToRotation` (Möller and Hughes' original code, with
/// `h = (1 - e) / |v|²`), as `m[row][col]`. Its 180° case turns about `u × f`, where `u`
/// is `f × X` normalised (`f × Y` when that is too short), and ignores `to`.
fn from_to_matrix(f: V3, t: V3) -> [[f32; 3]; 3] {
    let e = f[0] * t[0] + f[1] * t[1] + f[2] * t[2];
    if f64::from(e) > FROM_TO_PARALLEL {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    if f64::from(e) < -FROM_TO_PARALLEL {
        let [fx, fy, fz] = f;
        let (a, b, c) =
            if (fz * fz + 0.0) + fy * fy < 1e-6 { (-fz, 0.0, fx) } else { (0.0, fz, -fy) };
        let inv = 1.0 / (c * c + (a * a + b * b)).sqrt();
        let (a, b, c) = (a * inv, b * inv, c * inv);
        let vx = fz * b - fy * c;
        let vy = fx * c - fz * a;
        let vz = fy * a - fx * b;
        let m01 = (vx * vy - fx * fy) - a * b;
        let m02 = (vx * vz - fx * fz) - a * c;
        let m12 = (vy * vz - fz * fy) - b * c;
        return [
            [(vx * vx - fx * fx) - a * a, m01, m02],
            [m01, (vy * vy - fy * fy) - b * b, m12],
            [m02, m12, (vz * vz - fz * fz) - c * c],
        ];
    }
    let v = [f[1] * t[2] - f[2] * t[1], f[2] * t[0] - f[0] * t[2], f[0] * t[1] - f[1] * t[0]];
    let h = (1.0 - e) / (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
    let hvx = h * v[0];
    let hvz = h * v[2];
    let hvxy = hvx * v[1];
    let hvxz = hvx * v[2];
    let hvyz = hvz * v[1];
    [
        [e + hvx * v[0], hvxy - v[2], hvxz + v[1]],
        [hvxy + v[2], e + h * v[1] * v[1], hvyz - v[0]],
        [hvxz - v[1], hvyz + v[0], e + hvz * v[2]],
    ]
}

/// The player's `MatrixToQuaternion` (Shoemake), then its normalisation.
fn matrix_to_quaternion(m: [[f32; 3]; 3]) -> Q {
    let trace = m[0][0] + m[1][1] + m[2][2];
    let mut q = [0.0f32; 4];
    if trace > 0.0 {
        let root = (trace + 1.0).sqrt();
        q[3] = 0.5 * root;
        let root = 0.5 / root;
        q[0] = (m[2][1] - m[1][2]) * root;
        q[1] = (m[0][2] - m[2][0]) * root;
        q[2] = (m[1][0] - m[0][1]) * root;
    } else {
        let next = [1usize, 2, 0];
        let mut i = 0;
        if m[1][1] > m[0][0] {
            i = 1;
        }
        if m[2][2] > m[i][i] {
            i = 2;
        }
        let j = next[i];
        let k = next[j];
        let root = (m[i][i] - m[j][j] - m[k][k] + 1.0).sqrt();
        q[i] = 0.5 * root;
        let root = 0.5 / root;
        q[3] = (m[k][j] - m[j][k]) * root;
        q[j] = (m[j][i] + m[i][j]) * root;
        q[k] = (m[k][i] + m[i][k]) * root;
    }
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    q.map(|c| c / n)
}

/// `VectorMath.IsColinear(Vector3, Vector3, Vector3)`.
fn is_colinear(a: V3, b: V3, c: V3) -> bool {
    let v = cs::sub(b, a);
    let w = cs::sub(c, a);
    let n1 = r(d(v[1]) * d(w[2]) - d(v[2]) * d(w[1]));
    let n2 = r(d(v[2]) * d(w[0]) - d(v[0]) * d(w[2]));
    let n3 = r(d(v[0]) * d(w[1]) - d(v[1]) * d(w[0]));
    d(n1) * d(n1) + d(n2) * d(n2) + d(n3) * d(n3) <= d(1e-7_f32)
}

fn unwrap_helper(start: V3, end: V3, prev: V3, next: V3, rot: &mut Q, off: &mut V3) -> bool {
    if is_colinear(start, end, next) {
        return false;
    }
    let v = cs::sub(end, start);
    let sq = cs::sqr_magnitude(v);
    let prev = cs::sub(prev, cs::mul(v, r(d(cs::dot(cs::sub(prev, start), v)) / d(sq))));
    let next = cs::sub(next, cs::mul(v, r(d(cs::dot(cs::sub(next, start), v)) / d(sq))));
    let q = from_to_rotation(cs::sub(next, start), cs::sub(start, prev));
    *off = cs::add(*off, qrot(*rot, cs::sub(start, qrot(q, start))));
    *rot = qmul(*rot, q);
    true
}

/// `Funnel.Unwrap`.
fn unwrap(left: &[V3], right: &[V3]) -> (Vec<[f32; 2]>, Vec<[f32; 2]>) {
    let n = left.len();
    let mut l2 = vec![[0.0; 2]; n];
    let mut r2 = vec![[0.0; 2]; n];
    let mut k = 1;
    let mut from = cs::cross(cs::sub(right[1], left[0]), cs::sub(left[1], left[0]));
    while cs::sqr_magnitude(from) <= 1e-8 && k + 1 < n {
        k += 1;
        from = cs::cross(cs::sub(right[k], left[0]), cs::sub(left[k], left[0]));
    }
    let mut a = left[1];
    let mut b = right[1];
    let mut prev = left[0];
    let mut rot = from_to_rotation(from, [0.0, 0.0, 1.0]);
    let mut off = qrot(rot, [-right[0][0], -right[0][1], -right[0][2]]);
    for i in 1..n {
        if unwrap_helper(a, b, prev, left[i], &mut rot, &mut off) {
            prev = a;
            a = left[i];
        }
        let p = cs::add(qrot(rot, left[i]), off);
        l2[i] = [p[0], p[1]];
        if unwrap_helper(a, b, prev, right[i], &mut rot, &mut off) {
            prev = b;
            b = right[i];
        }
        let p = cs::add(qrot(rot, right[i]), off);
        r2[i] = [p[0], p[1]];
    }
    (l2, r2)
}

/// `Funnel.FixFunnel`.
fn fix_funnel(left: &mut [[f32; 2]], right: &mut [[f32; 2]], n: usize) -> Option<usize> {
    if n < 3 {
        return None;
    }
    let mut k = 0;
    while cs::approx_eq2(left[k + 1], left[k + 2]) && cs::approx_eq2(right[k + 1], right[k + 2]) {
        left[k + 1] = left[k];
        right[k + 1] = right[k];
        k += 1;
        if n - k < 3 {
            return None;
        }
    }
    Some(k)
}

fn sub2(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn cross2(a: [f32; 2], b: [f32; 2]) -> f64 {
    d(a[0]) * d(b[1]) - d(b[0]) * d(a[1])
}

/// `Funnel.Calculate(Vector2[]...)`: corner indices (negative for right-side corners).
fn calculate_indices(
    left: &[[f32; 2]],
    right: &[[f32; 2]],
    n: usize,
    start: usize,
    out: &mut Vec<i64>,
) {
    let mut apex_i = start;
    let mut right_i = start + 1;
    let mut left_i = start + 1;
    let mut apex = left[apex_i];
    let mut pl = left[left_i];
    let mut pr = right[right_i];
    out.push(apex_i as i64);
    let mut i = start + 2;
    while i < n {
        if out.len() > 2000 {
            break;
        }
        let l = left[i];
        let rr = right[i];
        if cross2(sub2(pr, apex), sub2(rr, apex)) >= 0.0 {
            // `!RightOrColinear(..)`, kept negated: NaN behaves as in the C#.
            #[expect(clippy::neg_cmp_op_on_partial_ord, reason = "mirrors the C# expression")]
            let not_right = !(cross2(sub2(pl, apex), sub2(rr, apex)) <= 0.0);
            if !cs::approx_eq2(apex, pr) && not_right {
                apex = pl;
                pr = pl;
                apex_i = left_i;
                right_i = left_i;
                i = apex_i;
                out.push(apex_i as i64);
                i += 1;
                continue;
            }
            pr = rr;
            right_i = i;
        }
        if cross2(sub2(pl, apex), sub2(l, apex)) <= 0.0 {
            if cs::approx_eq2(apex, pl) || cross2(sub2(pr, apex), sub2(l, apex)) >= 0.0 {
                pl = l;
                left_i = i;
            } else {
                apex = pr;
                pl = pr;
                apex_i = right_i;
                left_i = right_i;
                i = apex_i;
                out.push(-(apex_i as i64));
            }
        }
        i += 1;
    }
    out.push(n as i64 - 1);
}

/// `Funnel.Calculate(FunnelPortals, unwrap: true, splitAtEveryPortal: false)`.
#[must_use]
pub fn calculate(left: &[V3], right: &[V3]) -> Vec<V3> {
    let n = left.len();
    let (mut l2, mut r2) = unwrap(left, right);
    let mut idx = Vec::new();
    match fix_funnel(&mut l2, &mut r2, n) {
        None => {
            idx.push(0);
            idx.push(n as i64 - 1);
        }
        Some(s) => calculate_indices(&l2, &r2, n, s, &mut idx),
    }
    idx.iter().map(|&i| if i >= 0 { left[i as usize] } else { right[(-i) as usize] }).collect()
}

impl World {
    /// `TriangleMeshNode.SharedEdge`: the last matching connection's edge, or -1.
    fn shared_edge(&self, a: usize, b: NodeId) -> i32 {
        let mut res = -1;
        for c in &self.tris[a].conns {
            if c.node == b {
                res = i32::from(c.shape_edge);
            }
        }
        res
    }

    /// `TriangleMeshNode.GetPortal`.
    fn tri_portal(&self, a: usize, b: usize, left: &mut Vec<V3>, right: &mut Vec<V3>) -> bool {
        let e = self.shared_edge(a, b as NodeId);
        if e == 255 || e == -1 {
            return false;
        }
        let e = e as usize;
        let mut v1 = self.vertex(self.tris[a].v[e]);
        let mut v2 = self.vertex(self.tris[a].v[(e + 1) % 3]);
        let ta = self.tris[a].tile as i32;
        let tb = self.tris[b].tile as i32;
        if ta != tb {
            let rc = self.recast.as_ref().expect("navmesh");
            let (az, ax) = (ta / rc.tile_x, ta - (ta / rc.tile_x) * rc.tile_x);
            let (bz, bx) = (tb / rc.tile_x, tb - (tb / rc.tile_x) * rc.tile_x);
            let axis = if (ax - bx).abs() == 1 {
                2
            } else if (az - bz).abs() == 1 {
                0
            } else {
                return false;
            };
            let e2 = self.shared_edge(b, a as NodeId);
            if e2 != -1 && e2 != 255 {
                let e2 = e2 as usize;
                let mut lo = v1[axis].min(v2[axis]);
                let mut hi = v1[axis].max(v2[axis]);
                let w1 = self.vertex(self.tris[b].v[e2]);
                let w2 = self.vertex(self.tris[b].v[(e2 + 1) % 3]);
                lo = lo.max(w1[axis].min(w2[axis]));
                hi = hi.min(w1[axis].max(w2[axis]));
                if v1[axis] < v2[axis] {
                    v1[axis] = lo;
                    v2[axis] = hi;
                } else {
                    v1[axis] = hi;
                    v2[axis] = lo;
                }
            }
        }
        left.push(cs::from_int3(v1));
        right.push(cs::from_int3(v2));
        true
    }

    /// `GridNode.GetPortal`.
    fn grid_portal(&self, a: usize, b: usize, left: &mut Vec<V3>, right: &mut Vec<V3>) -> bool {
        let g = self.grid.as_ref().expect("grid");
        let pa = g.positions[a];
        let pb = g.positions[b];
        let nb = |dir: usize| (a as i64 + i64::from(g.neighbour_offsets[dir])) as usize;
        for dir in 0..4 {
            if g.has_connection(a, dir) && nb(dir) == b {
                let mid = cs::mul(cs::from_int3(cs::int3_add(pa, pb)), 0.5);
                let mut v = cs::normalize(cs::cross(g.up, cs::from_int3(cs::int3_sub(pb, pa))));
                v = cs::mul(v, r(d(g.node_size) * 0.5));
                left.push(cs::sub(mid, v));
                right.push(cs::add(mid, v));
                return true;
            }
        }
        for dir in 4..8 {
            if !(g.has_connection(a, dir) && nb(dir) == b) {
                continue;
            }
            let mut fl = false;
            let mut fr = false;
            if g.has_connection(a, dir - 4) {
                let n = nb(dir - 4);
                if g.flags[n] & 1 != 0 && g.has_connection(n, (dir - 4 + 1) % 4) {
                    fl = true;
                }
            }
            if g.has_connection(a, (dir - 4 + 1) % 4) {
                let n = nb((dir - 4 + 1) % 4);
                if g.flags[n] & 1 != 0 && g.has_connection(n, dir - 4) {
                    fr = true;
                }
            }
            let mid = cs::mul(cs::from_int3(cs::int3_add(pa, pb)), 0.5);
            let mut v = cs::normalize(cs::cross(g.up, cs::from_int3(cs::int3_sub(pb, pa))));
            // The C# literal is 1.4142f, not sqrt(2).
            #[expect(clippy::approx_constant, reason = "the game's literal")]
            let k = 1.4142_f32;
            v = cs::mul(v, r(d(g.node_size) * d(k)));
            left.push(cs::sub(mid, if fr { v } else { [0.0; 3] }));
            right.push(cs::add(mid, if fl { v } else { [0.0; 3] }));
            return true;
        }
        false
    }

    fn portal(&self, a: NodeId, b: NodeId, left: &mut Vec<V3>, right: &mut Vec<V3>) -> bool {
        if self.graph_index(a) != self.graph_index(b) {
            return false;
        }
        match (self.kind(a), self.kind(b)) {
            (Kind::Tri(x), Kind::Tri(y)) => self.tri_portal(x, y, left, right),
            (Kind::Cell(x), Kind::Cell(y)) => self.grid_portal(x, y, left, right),
            _ => false,
        }
    }

    /// `FunnelModifier.Apply` on a path of one graph (no node links).
    #[must_use]
    pub fn funnel_modifier(&self, path: &[NodeId], vector_path: &[V3]) -> Vec<V3> {
        if path.is_empty() || vector_path.is_empty() {
            return vector_path.to_vec();
        }
        let mut out = Vec::new();
        // SplitIntoParts: runs of nodes in the same graph.
        let mut i = 0;
        while i < path.len() {
            let start = i;
            let gi = self.graph_index(path[i]);
            while i < path.len() && self.graph_index(path[i]) == gi {
                i += 1;
            }
            let end = i - 1;
            let sp = if start == 0 { vector_path[0] } else { self.position_v3(path[start - 1]) };
            let ep = if end == path.len() - 1 {
                vector_path[vector_path.len() - 1]
            } else {
                self.position_v3(path[end + 1])
            };
            let mut left = vec![sp];
            let mut right = vec![sp];
            for k in start..end {
                if !self.portal(path[k], path[k + 1], &mut left, &mut right) {
                    let a = self.position_v3(path[k]);
                    let b = self.position_v3(path[k + 1]);
                    left.push(a);
                    right.push(a);
                    left.push(b);
                    right.push(b);
                }
            }
            left.push(ep);
            right.push(ep);
            out.extend(calculate(&left, &right));
        }
        out
    }
}
