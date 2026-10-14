//! Basic path queries on a decoded graph cache: A* over navmesh triangles with the
//! funnel algorithm, and A* over the grid graph.
//!
//! This is the POC-0 baseline for comparing against the live game. It follows A*'s
//! data (stored connection costs, Euclidean heuristic in Int3 units; node penalties are
//! ignored because the shipped `GraphNode.Penalty` always returns 0) but not yet its
//! 4-ary heap tie-breaking, backward connection order, endpoint special costs,
//! re-opening of closed nodes, nearest-node search, tag masks, navmesh cuts, or
//! Owlcat's clearance and modifiers (see `docs/poc0/extraction.md`). Differences found
//! by the oracle drive the next iteration.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use serde::Serialize;

use super::graph::{FLAG_WALKABLE, GraphCache, GridGraph, INT3_PRECISION, Int3, RecastGraph};

/// A query result.
#[derive(Debug, Clone, Serialize)]
pub struct PathResult {
    pub graph: &'static str,
    pub start: [f32; 3],
    pub end: [f32; 3],
    pub start_node: Option<usize>,
    pub end_node: Option<usize>,
    /// Node indices (local to our decoded graph) from start to end.
    pub nodes: Vec<usize>,
    /// Runtime-stable node identities for the oracle: recast `(tile index, index in
    /// tile.nodes)`, grid `(0, NodeInGridIndex)`. A*'s `NodeIndex` is reassigned on
    /// load, so it can't be compared.
    pub node_keys: Vec<(u32, u32)>,
    /// A* cost (Int3 units: millimetres plus penalties).
    pub cost: u64,
    /// Smoothed corner points (funnel) for navmesh paths; node centres for grid paths.
    pub points: Vec<[f32; 3]>,
    /// Length of `points` in metres.
    pub length: f32,
    pub expanded: usize,
    pub found: bool,
}

fn to_int3(p: [f32; 3]) -> Int3 {
    p.map(|c| (f64::from(c) * f64::from(INT3_PRECISION)).round_ties_even() as i32)
}

fn to_m(p: Int3) -> [f32; 3] {
    p.map(|c| c as f32 / INT3_PRECISION)
}

/// `Int3.costMagnitude`: the magnitude computed in double, narrowed to `float`, then
/// `Math.Round` (ties to even).
fn cost_dist(a: Int3, b: Int3) -> u64 {
    let d = [0, 1, 2].map(|i| f64::from(a[i]) - f64::from(b[i]));
    let m = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32;
    f64::from(m).round_ties_even() as u64
}

fn poly_len(points: &[[f32; 3]]) -> f32 {
    points
        .windows(2)
        .map(|w| {
            ((w[1][0] - w[0][0]).powi(2)
                + (w[1][1] - w[0][1]).powi(2)
                + (w[1][2] - w[0][2]).powi(2))
            .sqrt()
        })
        .sum()
}

/// 2D cross product on the XZ plane of `(b - a) × (c - a)`; positive when `c` is to the
/// left of `a → b` in Unity's left-handed, Y-up frame seen from above.
fn cross_xz(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    let (bx, bz) = (b[0] - a[0], b[2] - a[2]);
    let (cx, cz) = (c[0] - a[0], c[2] - a[2]);
    bx * cz - bz * cx
}

/// Closest point on triangle `t` to `p` (Ericson, Real-Time Collision Detection 5.1.5).
fn closest_on_triangle(p: [f64; 3], t: [[f64; 3]; 3]) -> [f64; 3] {
    let sub = |a: [f64; 3], b: [f64; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let add_s =
        |a: [f64; 3], b: [f64; 3], s: f64| [a[0] + b[0] * s, a[1] + b[1] * s, a[2] + b[2] * s];
    let (a, b, c) = (t[0], t[1], t[2]);
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return add_s(a, ab, d1 / (d1 - d3));
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return add_s(a, ac, d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let bc = sub(c, b);
        return add_s(b, bc, (d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    [a[0] + ab[0] * v + ac[0] * w, a[1] + ab[1] * v + ac[1] * w, a[2] + ab[2] * v + ac[2] * w]
}

/// The walkable triangle nearest to `p` (3D distance to the triangle), as a baseline
/// for A*'s `GetNearest`.
#[must_use]
pub fn nearest_triangle(g: &RecastGraph, p: [f32; 3]) -> Option<(usize, [f32; 3])> {
    let pi = to_int3(p).map(f64::from);
    let mut best: Option<(f64, usize, [f64; 3])> = None;
    for (i, n) in g.nodes.iter().enumerate() {
        if n.flags & FLAG_WALKABLE == 0 {
            continue;
        }
        let t = g.triangle(n).map(|v| v.map(f64::from));
        let c = closest_on_triangle(pi, t);
        let d = (0..3).map(|k| (c[k] - pi[k]).powi(2)).sum::<f64>();
        if best.as_ref().is_none_or(|b| d < b.0) {
            best = Some((d, i, c));
        }
    }
    best.map(|(_, i, c)| (i, c.map(|x| (x / f64::from(INT3_PRECISION)) as f32)))
}

/// A* over a set of nodes given neighbour and heuristic functions. Ties on `f` are
/// broken by lower `h`, then lower node index (deterministic; A*'s own heap order is
/// not reproduced yet).
fn astar<N, H>(
    start: usize,
    goal: usize,
    mut neighbours: N,
    heuristic: H,
) -> Option<(Vec<usize>, u64, usize)>
where
    N: FnMut(usize, &mut Vec<(usize, u64)>),
    H: Fn(usize) -> u64,
{
    let mut g: HashMap<usize, u64> = HashMap::new();
    let mut parent: HashMap<usize, usize> = HashMap::new();
    let mut heap = BinaryHeap::new();
    g.insert(start, 0);
    heap.push(Reverse((heuristic(start), heuristic(start), start)));
    let mut closed = std::collections::HashSet::new();
    let mut buf = Vec::new();
    let mut expanded = 0;
    while let Some(Reverse((_, _, n))) = heap.pop() {
        if !closed.insert(n) {
            continue;
        }
        expanded += 1;
        if n == goal {
            let mut path = vec![n];
            let mut cur = n;
            while let Some(&p) = parent.get(&cur) {
                path.push(p);
                cur = p;
            }
            path.reverse();
            return Some((path, g[&n], expanded));
        }
        buf.clear();
        neighbours(n, &mut buf);
        let gn = g[&n];
        for &(m, cost) in &buf {
            if closed.contains(&m) {
                continue;
            }
            let ng = gn + cost;
            if g.get(&m).is_none_or(|&old| ng < old) {
                g.insert(m, ng);
                parent.insert(m, n);
                let h = heuristic(m);
                heap.push(Reverse((ng + h, h, m)));
            }
        }
    }
    None
}

/// Portal between two adjacent triangles (`TriangleMeshNode.GetPortal`), in metres.
fn portal(g: &RecastGraph, a: usize, b: usize) -> Option<([f32; 3], [f32; 3])> {
    let na = &g.nodes[a];
    let nb_index = g.nodes[b].node_index;
    let edge =
        na.connections.iter().filter(|c| c.node == nb_index).map(|c| c.shape_edge).next_back()?;
    if edge == 255 {
        return None;
    }
    let e = usize::from(edge);
    let mut v1 = g.vertex(na.v[e]);
    let mut v2 = g.vertex(na.v[(e + 1) % 3]);
    if na.tile != g.nodes[b].tile {
        // Tile-border edges may only partly overlap: clamp to the shared span on the
        // axis along the border, as GetPortal does.
        let ta = &g.tiles[na.tile as usize];
        let tb = &g.tiles[g.nodes[b].tile as usize];
        let axis = if (ta.x - tb.x).abs() == 1 {
            2
        } else if (ta.z - tb.z).abs() == 1 {
            0
        } else {
            return None;
        };
        let nb = &g.nodes[b];
        let back = nb
            .connections
            .iter()
            .filter(|c| c.node == na.node_index)
            .map(|c| c.shape_edge)
            .next_back();
        if let Some(be) = back.filter(|&x| x != 255) {
            let be = usize::from(be);
            let w1 = g.vertex(nb.v[be]);
            let w2 = g.vertex(nb.v[(be + 1) % 3]);
            let lo = v1[axis].min(v2[axis]).max(w1[axis].min(w2[axis]));
            let hi = v1[axis].max(v2[axis]).min(w1[axis].max(w2[axis]));
            if v1[axis] < v2[axis] {
                v1[axis] = lo;
                v2[axis] = hi;
            } else {
                v1[axis] = hi;
                v2[axis] = lo;
            }
        }
    }
    Some((to_m(v1), to_m(v2)))
}

/// Simple stupid funnel algorithm over portals on the XZ plane. Each portal is oriented
/// so that `left` lies left of the direction of travel.
#[must_use]
pub fn funnel(start: [f32; 3], end: [f32; 3], portals: &[([f32; 3], [f32; 3])]) -> Vec<[f32; 3]> {
    let mut ps: Vec<([f32; 3], [f32; 3])> = Vec::with_capacity(portals.len() + 2);
    ps.push((start, start));
    ps.extend_from_slice(portals);
    ps.push((end, end));
    let mut out = vec![start];
    let (mut apex, mut left, mut right) = (start, start, start);
    let (mut left_i, mut right_i) = (0usize, 0usize);
    let mut i = 1;
    let same = |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).abs() < 1e-6 && (a[2] - b[2]).abs() < 1e-6;
    while i < ps.len() {
        let (l, r) = ps[i];
        // Tighten the right side.
        if cross_xz(apex, right, r) >= 0.0 {
            if same(apex, right) || cross_xz(apex, left, r) < 0.0 {
                right = r;
                right_i = i;
            } else {
                out.push(left);
                apex = left;
                right = apex;
                right_i = left_i;
                i = left_i + 1;
                continue;
            }
        }
        // Tighten the left side.
        if cross_xz(apex, left, l) <= 0.0 {
            if same(apex, left) || cross_xz(apex, right, l) > 0.0 {
                left = l;
                left_i = i;
            } else {
                out.push(right);
                apex = right;
                left = apex;
                left_i = right_i;
                i = right_i + 1;
                continue;
            }
        }
        i += 1;
    }
    if !out.last().is_some_and(|p| same(*p, end)) {
        out.push(end);
    }
    out
}

/// Navmesh query: nearest start/end triangles, A* over connections (stored costs), then
/// the funnel through shared edges.
#[must_use]
pub fn navmesh_path(cache: &GraphCache, start: [f32; 3], end: [f32; 3]) -> Option<PathResult> {
    let g = cache.recast()?;
    let index: HashMap<i32, usize> =
        g.nodes.iter().enumerate().map(|(i, n)| (n.node_index, i)).collect();
    let (s, sp) = nearest_triangle(g, start)?;
    let (e, ep) = nearest_triangle(g, end)?;
    let target = to_int3(ep);
    let found = astar(
        s,
        e,
        |n, out| {
            for c in &g.nodes[n].connections {
                if let Some(&m) = index.get(&c.node)
                    && g.nodes[m].flags & FLAG_WALKABLE != 0
                {
                    // `GraphNode.Penalty` always returns 0 in the shipped A* (Owlcat
                    // modification), and tag penalties default to 0.
                    out.push((m, u64::from(c.cost)));
                }
            }
        },
        |n| cost_dist(g.nodes[n].position, target),
    );
    let mut res = PathResult {
        graph: "recast",
        start,
        end,
        start_node: Some(s),
        end_node: Some(e),
        nodes: Vec::new(),
        node_keys: Vec::new(),
        cost: 0,
        points: Vec::new(),
        length: 0.0,
        expanded: 0,
        found: false,
    };
    let (nodes, cost, expanded) = found?;
    let mut portals = Vec::new();
    for w in nodes.windows(2) {
        let (mut l, mut r) = portal(g, w[0], w[1])?;
        // Orient: left must be left of the travel direction between triangle centres.
        let a = to_m(g.nodes[w[0]].position);
        let b = to_m(g.nodes[w[1]].position);
        let mid = [(l[0] + r[0]) / 2.0, (l[1] + r[1]) / 2.0, (l[2] + r[2]) / 2.0];
        let dir_end = if cross_xz(a, b, l) == 0.0 { mid } else { b };
        if cross_xz(a, dir_end, l) < cross_xz(a, dir_end, r) {
            std::mem::swap(&mut l, &mut r);
        }
        portals.push((l, r));
    }
    res.node_keys = nodes
        .iter()
        .map(|&n| {
            let tile = g.nodes[n].tile;
            let first = g.tiles.get(tile as usize).map_or(0, |t| t.first_node);
            (tile, u32::try_from(n - first).unwrap_or(u32::MAX))
        })
        .collect();
    res.points = funnel(sp, ep, &portals);
    res.length = poly_len(&res.points);
    res.nodes = nodes;
    res.cost = cost;
    res.expanded = expanded;
    res.found = true;
    Some(res)
}

/// The walkable grid node nearest to `p` (3D distance to node position).
#[must_use]
pub fn nearest_grid_node(g: &GridGraph, p: [f32; 3]) -> Option<usize> {
    let pi = to_int3(p);
    g.nodes
        .iter()
        .enumerate()
        .filter(|(_, n)| n.walkable())
        .min_by_key(|(i, n)| (cost_dist(n.position, pi), *i))
        .map(|(i, _)| i)
}

/// Grid query: 8-neighbour A* using the node connection bits and `neighbourCosts`, with
/// all tags allowed (real-time rules; turn-based excludes erosion tags by unit size).
#[must_use]
pub fn grid_path(cache: &GraphCache, start: [f32; 3], end: [f32; 3]) -> Option<PathResult> {
    let g = cache.grid()?;
    let s = nearest_grid_node(g, start)?;
    let e = nearest_grid_node(g, end)?;
    let target = g.nodes[e].position;
    let len = g.nodes.len();
    let (nodes, cost, expanded) = astar(
        s,
        e,
        |n, out| {
            let node = &g.nodes[n];
            for dir in 0..8 {
                if !node.has_connection(dir) {
                    continue;
                }
                let m = n.cast_signed() + g.neighbour_offset(dir);
                let Ok(m) = usize::try_from(m) else { continue };
                if m < len && g.nodes[m].walkable() {
                    out.push((m, u64::from(g.neighbour_cost(dir))));
                }
            }
        },
        |n| cost_dist(g.nodes[n].position, target),
    )?;
    let points: Vec<[f32; 3]> = nodes.iter().map(|&n| to_m(g.nodes[n].position)).collect();
    Some(PathResult {
        graph: "grid",
        start,
        end,
        start_node: Some(s),
        end_node: Some(e),
        length: poly_len(&points),
        points,
        node_keys: nodes.iter().map(|&n| (0, u32::try_from(n).unwrap_or(u32::MAX))).collect(),
        nodes,
        cost,
        expanded,
        found: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn funnel_goes_straight_through_a_straight_corridor() {
        // Walking +Z through two portals spanning x in [-1, 1]; left is -X.
        let portals = [([-1.0, 0.0, 1.0], [1.0, 0.0, 1.0]), ([-1.0, 0.0, 2.0], [1.0, 0.0, 2.0])];
        let p = funnel([0.0, 0.0, 0.0], [0.0, 0.0, 3.0], &portals);
        assert_eq!(p, vec![[0.0, 0.0, 0.0], [0.0, 0.0, 3.0]]);
    }

    #[test]
    fn funnel_cuts_the_corner_of_an_l_corridor() {
        // Up +Z along x∈[0,1], then turn right (+X) at z∈[2,3]. The inside corner is
        // (1, 2); the path must bend there.
        let portals = [
            ([0.0, 0.0, 1.0], [1.0, 0.0, 1.0]),
            ([0.0, 0.0, 2.0], [1.0, 0.0, 2.0]),
            ([1.0, 0.0, 3.0], [1.0, 0.0, 2.0]),
            ([2.0, 0.0, 3.0], [2.0, 0.0, 2.0]),
        ];
        let p = funnel([0.5, 0.0, 0.0], [3.0, 0.0, 2.5], &portals);
        assert_eq!(p.len(), 3);
        assert_eq!(p[1], [1.0, 0.0, 2.0]);
    }

    #[test]
    fn closest_point_on_triangle_cases() {
        let t = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let c = closest_on_triangle([0.2, 5.0, 0.2], t);
        assert!((c[0] - 0.2).abs() < 1e-12 && c[1] == 0.0 && (c[2] - 0.2).abs() < 1e-12);
        assert_eq!(closest_on_triangle([-1.0, 0.0, -1.0], t), [0.0, 0.0, 0.0]);
        let c = closest_on_triangle([1.0, 0.0, 1.0], t);
        assert!((c[0] - 0.5).abs() < 1e-12 && (c[2] - 0.5).abs() < 1e-12);
    }

    #[test]
    fn cost_magnitude_narrows_to_float_and_rounds_to_even() {
        // sqrt(5594² + 707²) = 5638.5005 in double, exactly 5638.5 as float → 5638.
        assert_eq!(cost_dist([5594, 707, 0], [0, 0, 0]), 5638);
        assert_eq!(cost_dist([3, 4, 0], [0, 0, 0]), 5);
    }

    #[test]
    fn astar_finds_cheapest_route() {
        // 0 -1- 1 -1- 3 ; 0 -5- 3 ; 0 -1- 2 -10- 3
        let edges: HashMap<usize, Vec<(usize, u64)>> =
            [(0, vec![(1, 1), (3, 5), (2, 1)]), (1, vec![(3, 1)]), (2, vec![(3, 10)]), (3, vec![])]
                .into_iter()
                .collect();
        let (path, cost, _) =
            astar(0, 3, |n, out| out.extend(edges[&n].iter().copied()), |_| 0).unwrap();
        assert_eq!(path, [0, 1, 3]);
        assert_eq!(cost, 2);
    }
}
