//! Path post-processing: `StartEndModifier` (as the oracle and `UnitMovementAgent`
//! configure it), Owlcat's `PathPostprocess.RemoveExcessPoints` over
//! `ObstacleAnalyzer.TraceAlongNavmesh` (`NavmeshBase.Linecast`), and the
//! `RaycastModifier` with graph raycasts on the grid (`GridGraph.Linecast`, quality
//! Medium: two greedy passes with `Polygon.Subdivide` between them).

use super::cs::{self, V3, d, r};
use super::nearest::{Constraint, Nearest};
use super::world::{NodeId, World};
use crate::Int3;

/// `Side` of `VectorMath.SideXZ`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Colinear = 0,
    Left = 1,
    Right = 2,
}

fn side_xz(a: Int3, b: Int3, p: Int3) -> Side {
    let n = i64::from(b[0] - a[0]) * i64::from(p[2] - a[2])
        - i64::from(p[0] - a[0]) * i64::from(b[2] - a[2]);
    if n > 0 {
        Side::Left
    } else if n < 0 {
        Side::Right
    } else {
        Side::Colinear
    }
}

/// `NavmeshBase.LinecastShapeEdgeLookup`.
fn shape_edge_lookup() -> [u8; 64] {
    let mut t = [255u8; 64];
    for (i, cell) in t.iter_mut().enumerate() {
        let s = [i & 3, (i >> 2) & 3, (i >> 4) & 3];
        if s.contains(&3) {
            continue;
        }
        let mut best = i32::MAX;
        for j in 0..3 {
            let a = s[j];
            let b = s[(j + 1) % 3];
            if (a == 1 || a == 0) && (b == 2 || b == 0) {
                let n = i32::from(a == 0) + i32::from(b == 0);
                if n < best {
                    *cell = j as u8;
                    best = n;
                }
            }
        }
    }
    t
}

/// `VectorMath.LineIntersectionPointXZ`.
fn line_intersection_point_xz(s1: V3, e1: V3, s2: V3, e2: V3) -> V3 {
    let v = cs::sub(e1, s1);
    let w = cs::sub(e2, s2);
    let n = r(d(w[2]) * d(v[0]) - d(w[0]) * d(v[2]));
    if n == 0.0 {
        return s1;
    }
    let t = r((d(w[0]) * (d(s1[2]) - d(s2[2])) - d(w[2]) * (d(s1[0]) - d(s2[0]))) / d(n));
    cs::add(s1, cs::mul(v, t))
}

/// `GraphHitInfo` fields used here.
#[derive(Debug, Clone, Copy)]
pub struct Hit {
    pub point: V3,
    pub hit: bool,
}

impl World {
    /// `TriangleMeshNode.ContainsPointInGraphSpace`.
    fn tri_contains_graph(&self, i: usize, p: Int3) -> bool {
        let [a, b, c] = self.tri_vertices_graph(i);
        let s = |u: Int3, v: Int3| {
            i64::from(v[0] - u[0]) * i64::from(p[2] - u[2])
                - i64::from(p[0] - u[0]) * i64::from(v[2] - u[2])
        };
        s(a, b) <= 0 && s(b, c) <= 0 && s(c, a) <= 0
    }

    /// `TriangleMeshNode.ContainsPoint(Vector3)`.
    #[must_use]
    pub fn tri_contains_point(&self, i: usize, p: V3) -> bool {
        let rc = self.recast.as_ref().expect("navmesh");
        self.tri_contains_graph(i, cs::to_int3(cs::sub(p, rc.translation)))
    }

    /// `TriangleMeshNode.ClosestPointOnNodeXZInGraphSpace`.
    fn tri_closest_xz_graph(&self, i: usize, p: V3) -> Int3 {
        let rc = self.recast.as_ref().expect("navmesh");
        let [a, b, c] = self.tri_vertices_graph(i);
        let lp = cs::sub(p, rc.translation);
        let q = cs::to_int3(super::nearest::closest_point_on_triangle_xz(
            cs::from_int3(a),
            cs::from_int3(b),
            cs::from_int3(c),
            lp,
        ));
        if self.tri_contains_graph(i, q) {
            return q;
        }
        for dx in -1..=1 {
            for dz in -1..=1 {
                if dx != 0 || dz != 0 {
                    let q2 = [q[0] + dx, q[1], q[2] + dz];
                    if self.tri_contains_graph(i, q2) {
                        return q2;
                    }
                }
            }
        }
        let sq = |v: Int3| {
            let dd = cs::int3_sub(v, q).map(i64::from);
            dd[0] * dd[0] + dd[1] * dd[1] + dd[2] * dd[2]
        };
        let (s1, s2, s3) = (sq(a), sq(b), sq(c));
        if s1 >= s2 {
            if s2 >= s3 { c } else { b }
        } else if s1 >= s3 {
            c
        } else {
            a
        }
    }

    /// `NavmeshBase.Linecast(graph, origin, end, hint, out hit, null)`.
    #[must_use]
    pub fn navmesh_linecast(&self, origin: V3, end: V3, hint: Option<NodeId>) -> Hit {
        let rc = self.recast.as_ref().expect("navmesh");
        let tr = |p: V3| cs::add(p, rc.translation);
        let hint = hint.filter(|&h| (h as usize) < self.tris.len());
        let Some(mut node) = hint.or_else(|| self.navmesh_get_nearest_unconstrained(origin).node)
        else {
            return Hit { point: origin, hit: true };
        };
        let o = self.tri_closest_xz_graph(node as usize, origin);
        let hit_origin = tr(cs::from_int3(o));
        if !self.walkable(node) {
            return Hit { point: hit_origin, hit: true };
        }
        let e = cs::to_int3(cs::sub(end, rc.translation));
        if o == e {
            return Hit { point: hit_origin, hit: false };
        }
        let lookup = shape_edge_lookup();
        let mut iterations = 0;
        loop {
            iterations += 1;
            if iterations > 2000 {
                return Hit { point: hit_origin, hit: true };
            }
            let i = node as usize;
            let [v0, v1, v2] = self.tri_vertices_graph(i);
            let code = side_xz(o, e, v0) as usize
                | (side_xz(o, e, v1) as usize) << 2
                | (side_xz(o, e, v2) as usize) << 4;
            let edge = lookup[code];
            let (ea, eb) = match edge {
                0 => (v0, v1),
                1 => (v1, v2),
                _ => (v2, v0),
            };
            if side_xz(ea, eb, e) != Side::Left {
                return Hit { point: end, hit: false };
            }
            if edge == 255 {
                return Hit { point: hit_origin, hit: true };
            }
            let mut next = None;
            for c in &self.tris[i].conns {
                if c.shape_edge != edge
                    || (c.node as usize) >= self.tris.len()
                    || !self.walkable(c.node)
                {
                    continue;
                }
                let other = c.node as usize;
                let back = self.tris[other]
                    .conns
                    .iter()
                    .find(|cc| cc.node == node)
                    .map(|cc| cc.shape_edge);
                if let Some(be) = back {
                    let be = usize::from(be);
                    let ov = self.tri_vertices_graph(other);
                    let s1 = side_xz(o, e, ov[be]);
                    let s2 = side_xz(o, e, ov[(be + 1) % 3]);
                    let ok = (s1 == Side::Right || s1 == Side::Colinear)
                        && (s2 == Side::Left || s2 == Side::Colinear);
                    if ok {
                        next = Some(c.node);
                        break;
                    }
                }
            }
            match next {
                Some(n) => node = n,
                None => {
                    let p = line_intersection_point_xz(
                        cs::from_int3(ea),
                        cs::from_int3(eb),
                        cs::from_int3(o),
                        cs::from_int3(e),
                    );
                    return Hit { point: tr(p), hit: true };
                }
            }
        }
    }

    /// `ObstacleAnalyzer.GetNearestNode(pos)` (no hint).
    #[must_use]
    pub fn obstacle_nearest(&self, pos: V3) -> Nearest {
        let mut c = Constraint {
            distance_xz: true,
            graph_mask: 1 << self.recast.as_ref().map_or(0, |r| r.graph_index),
            ..Constraint::default_()
        };
        let result = self.get_nearest(pos, &c);
        let Some(n) = result.node else { return result };
        let off = |n: NodeId| (r(d(self.position(n)[1] as f32) * d(0.001_f32) - d(pos[1]))).abs();
        if off(n) > 3.0 {
            // graphs[0].GetNearest(pos, NNConstraint.Default): an unconstrained forced
            // navmesh search.
            let nearest = self.navmesh_nearest_force(pos, None);
            let Some(nn) = nearest.node else { return result };
            c.constrain_area = true;
            c.area = i64::from(self.area[nn as usize]);
            let nearest2 = self.get_nearest(pos, &c);
            // The original tests `result` again here (not `nearest2`), so this never
            // replaces it.
            if off(n) < 3.0 {
                return nearest2;
            }
        }
        result
    }

    /// `ObstacleAnalyzer.TraceAlongNavmesh`.
    #[must_use]
    pub fn trace_along_navmesh(&self, start: V3, end: V3) -> V3 {
        let nn = self.obstacle_nearest(start);
        let dx = start[0] - nn.position[0];
        let dz = start[2] - nn.position[2];
        if r(d(dx) * d(dx) + d(dz) * d(dz)) > 0.0001 {
            return nn.position;
        }
        let Some(node) = nn.node else { return nn.position };
        self.navmesh_linecast(start, end, Some(node)).point
    }

    /// `PathPostprocess.RemoveExcessPoints`.
    #[must_use]
    pub fn remove_excess_points(&self, path: &[V3]) -> Vec<V3> {
        let mut stack: Vec<V3> = Vec::new();
        for &v in path {
            while stack.len() >= 2
                && cs::approx_eq(self.trace_along_navmesh(stack[stack.len() - 2], v), v)
            {
                stack.pop();
            }
            stack.push(v);
        }
        stack
    }

    /// `GridGraph.Linecast(GridNodeBase, GridNodeBase)`: true if blocked.
    fn grid_linecast_nodes(&self, from: usize, to: usize) -> bool {
        let g = self.grid.as_ref().expect("grid");
        let (fx, fz) = (from as i32 % g.width, from as i32 / g.width);
        let (tx, tz) = (to as i32 % g.width, to as i32 / g.width);
        let a = [tx - fx, tz - fz];
        let sign = |v: i32| v.signum();
        let num = i64::from(a[0]) * i64::from(sign(a[1])) - i64::from(sign(a[0])) * i64::from(a[1]);
        let num2 = if a[0] <= 0 && a[1] > 0 {
            1
        } else if a[0] < 0 && a[1] <= 0 {
            2
        } else if a[0] >= 0 && a[1] < 0 {
            3
        } else {
            0
        };
        let num3 = (num2 + 1) & 3;
        let num4 = (num2 + 2) & 3;
        let num5 = if a[0] != 0 && a[1] != 0 { 4 + ((num2 + 1) & 3) } else { -1 };
        let mut b = [0i32, 0];
        let mut cur = Some(from);
        while let Some(c) = cur {
            if c == to {
                break;
            }
            let n6 =
                (i64::from(a[0]) * i64::from(b[1]) - i64::from(b[0]) * i64::from(a[1])) * 2 + num;
            let mut dir = if n6 < 0 { num4 } else { num3 };
            if n6 == 0 && num5 != -1 {
                dir = num5;
            }
            let dir = dir as usize;
            cur = if g.has_connection(c, dir) {
                Some((c as i64 + i64::from(g.neighbour_offsets[dir])) as usize)
            } else {
                None
            };
            b[0] += g.neighbour_x[dir];
            b[1] += g.neighbour_z[dir];
        }
        cur != Some(to)
    }

    /// `GridGraph.Linecast(Vector3 from, Vector3 to, ...)`: true if blocked.
    fn grid_linecast(&self, from: V3, to: V3) -> bool {
        let g = self.grid.as_ref().expect("grid");
        let mut a = g.inverse_transform(from);
        let mut b = g.inverse_transform(to);
        let (w, dd) = (g.width as f32, g.depth as f32);
        if a[0] < 0.0
            || a[2] < 0.0
            || a[0] > w
            || a[2] > dd
            || b[0] < 0.0
            || b[2] < 0.0
            || b[0] > w
            || b[2] > dd
        {
            let c = [[0.0, 0.0, 0.0], [0.0, 0.0, dd], [w, 0.0, dd], [w, 0.0, 0.0]];
            let mut hits = 0;
            for k in 0..4 {
                let (s2, e2) = (c[k], c[(k + 1) % 4]);
                if let Some(p) = segment_intersection_point_xz(a, b, s2, e2) {
                    hits += 1;
                    // VectorMath.RightOrColinearXZ(s2, e2, a).
                    let right = (d(e2[0]) - d(s2[0])) * (d(a[2]) - d(s2[2]))
                        - (d(a[0]) - d(s2[0])) * (d(e2[2]) - d(s2[2]))
                        <= 0.0;
                    if !right {
                        a = p;
                    } else {
                        b = p;
                    }
                }
            }
            if hits == 0 {
                return false;
            }
        }
        let n1 = self.grid_nearest(g.transform(a)).node.map(|n| (n - g.first) as usize);
        let n2 = self.grid_nearest(g.transform(b)).node.map(|n| (n - g.first) as usize);
        let (Some(n1), Some(n2)) = (n1, n2) else { return true };
        if !g.walkable(n1) {
            return true;
        }
        let v = [a[0] - 0.5, a[2] - 0.5];
        let v2 = [b[0] - 0.5, b[2] - 0.5];
        let dv = [v2[0] - v[0], v2[1] - v[1]];
        let sgn = |x: f32| if x >= 0.0 { 1.0f32 } else { -1.0 };
        let half = r((d(dv[0]) * d(sgn(dv[1])) - d(sgn(dv[0])) * d(dv[1])) * 0.5);
        let num2 = (if dv[1] >= 0.0 { 0 } else { 3 }) ^ (if dv[0] >= 0.0 { 0 } else { 1 });
        let num3 = (num2 + 1) & 3;
        let num4 = (num2 + 2) & 3;
        let mut cur = n1;
        while cur != n2 {
            let p = [(cur as i32 % g.width) as f32, (cur as i32 / g.width) as f32];
            let rel = [p[0] - v[0], p[1] - v[1]];
            let cm = r(d(dv[0]) * d(rel[1]) - d(rel[0]) * d(dv[1]));
            let dir = if d(cm) + d(half) < 0.0 { num4 } else { num3 } as usize;
            if g.has_connection(cur, dir) {
                cur = (cur as i64 + i64::from(g.neighbour_offsets[dir])) as usize;
                continue;
            }
            return true;
        }
        false
    }

    /// `RaycastModifier.ValidateLine` with graph raycasting only.
    fn validate_line(
        &self,
        n1: Option<NodeId>,
        n2: Option<NodeId>,
        v1: V3,
        v2: V3,
        c: &Constraint,
    ) -> bool {
        let known = n1.is_some() && n2.is_some();
        let n1 = n1.or_else(|| self.get_nearest(v1, c).node);
        let n2 = n2.or_else(|| self.get_nearest(v2, c).node);
        let (Some(a), Some(b)) = (n1, n2) else { return true };
        if self.graph_index(a) != self.graph_index(b) {
            return false;
        }
        match (self.is_cell(a), self.is_cell(b)) {
            (Some(x), Some(y)) if known => !self.grid_linecast_nodes(x, y),
            (Some(_), _) => !self.grid_linecast(v1, v2),
            _ => !self.navmesh_linecast(v1, v2, Some(a)).hit,
        }
    }

    fn apply_greedy(&self, path: &[NodeId], points: &[V3], c: &Constraint) -> Vec<V3> {
        let same = points.len() == path.len();
        let node_at = |i: usize, p: V3| {
            (same && cs::approx_eq(p, self.position_v3(path[i]))).then(|| path[i])
        };
        let mut out = Vec::new();
        let mut i = 0;
        while i < points.len() {
            let v = points[i];
            let n = node_at(i, v);
            out.push(v);
            let mut num = 1usize;
            let mut num2 = 2usize;
            loop {
                let k = i + num2;
                if k >= points.len() {
                    num2 = points.len() - i;
                    break;
                }
                let v2 = points[k];
                if !self.validate_line(n, node_at(k, v2), v, v2, c) {
                    break;
                }
                num = num2;
                num2 *= 2;
            }
            while num + 1 < num2 {
                let mid = (num + num2) / 2;
                let k = i + mid;
                let v3 = points[k];
                if self.validate_line(n, node_at(k, v3), v, v3, c) {
                    num = mid;
                } else {
                    num2 = mid;
                }
            }
            i += num;
        }
        out
    }

    /// `RaycastModifier.Apply` (graph raycasting, quality Medium).
    #[must_use]
    pub fn raycast_modifier(&self, path: &[NodeId], points: &[V3], c: &Constraint) -> Vec<V3> {
        if points.is_empty() {
            return Vec::new();
        }
        let first = points[0];
        let last = points[points.len() - 1];
        if self.validate_line(None, None, first, last, c) {
            return vec![first, last];
        }
        let mut b = points.to_vec();
        for it in 0..2 {
            if it != 0 {
                let mut sub = Vec::new();
                for k in 0..b.len() - 1 {
                    for j in 0..3 {
                        sub.push(cs::lerp(b[k], b[k + 1], r(d(j as f32) / d(3.0_f32))));
                    }
                }
                sub.push(b[b.len() - 1]);
                b = sub;
                b.reverse();
            }
            b = self.apply_greedy(path, &b, c);
        }
        b.reverse();
        b
    }
}

/// `VectorMath.SegmentIntersectionPointXZ`.
fn segment_intersection_point_xz(s1: V3, e1: V3, s2: V3, e2: V3) -> Option<V3> {
    let v = cs::sub(e1, s1);
    let w = cs::sub(e2, s2);
    let n = r(d(w[2]) * d(v[0]) - d(w[0]) * d(v[2]));
    if n == 0.0 {
        return None;
    }
    let n2 = r(d(w[0]) * (d(s1[2]) - d(s2[2])) - d(w[2]) * (d(s1[0]) - d(s2[0])));
    let n3 = r(d(v[0]) * (d(s1[2]) - d(s2[2])) - d(v[2]) * (d(s1[0]) - d(s2[0])));
    let t = r(d(n2) / d(n));
    let u = r(d(n3) / d(n));
    if t < 0.0 || t > 1.0 || u < 0.0 || u > 1.0 {
        return None;
    }
    Some(cs::add(s1, cs::mul(v, t)))
}
