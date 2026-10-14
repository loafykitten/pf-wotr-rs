//! Nearest-node queries: `AstarPath.GetNearest`, `NavmeshBase.GetNearestForce` with its
//! per-tile `BBTree` (3D and XZ variants), and `GridGraph.GetNearest`/`GetNearestForce`,
//! with `NNConstraint.Suitable`. Float expressions follow Mono (see [`super::cs`]).

use super::cs::{self, V3, d, r};
use super::world::{Kind, NodeId, World};

/// `NNConstraint` (and `PathNNConstraint`, which only differs in `constrainArea`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Constraint {
    pub graph_mask: i32,
    pub constrain_area: bool,
    pub area: i64,
    pub constrain_walkability: bool,
    pub walkable: bool,
    pub distance_xz: bool,
    pub distance_xz_keep_y: bool,
    pub constrain_tags: bool,
    pub tags: i32,
    pub constrain_distance: bool,
    pub constrain_ally_blocked: bool,
}

impl Constraint {
    /// `NNConstraint.Default`.
    #[must_use]
    pub fn default_() -> Self {
        Self {
            graph_mask: -1,
            constrain_area: false,
            area: -1,
            constrain_walkability: true,
            walkable: true,
            distance_xz: false,
            distance_xz_keep_y: false,
            constrain_tags: true,
            tags: -1,
            constrain_distance: true,
            constrain_ally_blocked: true,
        }
    }

    /// `NNConstraint.None`.
    #[must_use]
    pub fn none() -> Self {
        Self {
            constrain_walkability: false,
            constrain_area: false,
            constrain_tags: false,
            constrain_distance: false,
            graph_mask: -1,
            ..Self::default_()
        }
    }

    /// `PathNNConstraint.Default` after `Reset`.
    #[must_use]
    pub fn path_default() -> Self {
        Self { constrain_area: true, ..Self::default_() }
    }

    #[must_use]
    pub fn suitable_graph(&self, graph_index: u32) -> bool {
        (self.graph_mask >> graph_index) & 1 != 0
    }

    /// `NNConstraint.Suitable` (no ally blocking in a static snapshot).
    #[must_use]
    pub fn suitable(&self, w: &World, n: NodeId) -> bool {
        if self.constrain_walkability && w.walkable(n) != self.walkable {
            return false;
        }
        if self.constrain_area && self.area >= 0 && i64::from(w.area[n as usize]) != self.area {
            return false;
        }
        if self.constrain_tags && (self.tags >> w.tag(n)) & 1 == 0 {
            return false;
        }
        true
    }
}

/// `NNInfo`: node and clamped position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nearest {
    pub node: Option<NodeId>,
    pub position: V3,
}

impl Nearest {
    const NONE: Nearest = Nearest { node: None, position: [0.0; 3] };
}

/// `Polygon.ClosestPointOnTriangle` (Vector3 version).
#[must_use]
pub fn closest_point_on_triangle(a: V3, b: V3, c: V3, p: V3) -> V3 {
    let ab = cs::sub(b, a);
    let ac = cs::sub(c, a);
    let ap = cs::sub(p, a);
    let n1 = cs::dot(ab, ap);
    let n2 = cs::dot(ac, ap);
    if n1 <= 0.0 && n2 <= 0.0 {
        return a;
    }
    let bp = cs::sub(p, b);
    let n3 = cs::dot(ab, bp);
    let n4 = cs::dot(ac, bp);
    if n3 >= 0.0 && n4 <= n3 {
        return b;
    }
    let n5 = r(d(n1) * d(n4) - d(n3) * d(n2));
    if n1 >= 0.0 && n3 <= 0.0 && n5 <= 0.0 {
        let t = r(d(n1) / (d(n1) - d(n3)));
        return cs::add(a, cs::mul(ab, t));
    }
    let cp = cs::sub(p, c);
    let n7 = cs::dot(ab, cp);
    let n8 = cs::dot(ac, cp);
    if n8 >= 0.0 && n7 <= n8 {
        return c;
    }
    let n9 = r(d(n7) * d(n2) - d(n1) * d(n8));
    if n2 >= 0.0 && n8 <= 0.0 && n9 <= 0.0 {
        let t = r(d(n2) / (d(n2) - d(n8)));
        return cs::add(a, cs::mul(ac, t));
    }
    let n11 = r(d(n3) * d(n8) - d(n7) * d(n4));
    if d(n4) - d(n3) >= 0.0 && d(n7) - d(n8) >= 0.0 && n11 <= 0.0 {
        let t = r((d(n4) - d(n3)) / (d(n4) - d(n3) + (d(n7) - d(n8))));
        return cs::add(b, cs::mul(cs::sub(c, b), t));
    }
    let n13 = r(1.0 / (d(n11) + d(n9) + d(n5)));
    let n14 = r(d(n9) * d(n13));
    let n15 = r(d(n5) * d(n13));
    cs::add(cs::add(a, cs::mul(ab, n14)), cs::mul(ac, n15))
}

fn dot2(a: [f32; 2], b: [f32; 2]) -> f32 {
    r(d(a[0]) * d(b[0]) + d(a[1]) * d(b[1]))
}

/// `Polygon.ClosestPointOnTriangleXZ`.
#[must_use]
pub fn closest_point_on_triangle_xz(a: V3, b: V3, c: V3, p: V3) -> V3 {
    let lhs = [b[0] - a[0], b[2] - a[2]];
    let lhs2 = [c[0] - a[0], c[2] - a[2]];
    let rhs = [p[0] - a[0], p[2] - a[2]];
    let n1 = dot2(lhs, rhs);
    let n2 = dot2(lhs2, rhs);
    if n1 <= 0.0 && n2 <= 0.0 {
        return a;
    }
    let rhs2 = [p[0] - b[0], p[2] - b[2]];
    let n3 = dot2(lhs, rhs2);
    let n4 = dot2(lhs2, rhs2);
    if n3 >= 0.0 && n4 <= n3 {
        return b;
    }
    // (1f - t) * a + t * b: each scalar*vector product stored, then summed.
    let blend = |u: V3, v: V3, t: f32| cs::add(cs::mul(u, 1.0 - t), cs::mul(v, t));
    let n5 = r(d(n1) * d(n4) - d(n3) * d(n2));
    if n1 >= 0.0 && n3 <= 0.0 && n5 <= 0.0 {
        let t = r(d(n1) / (d(n1) - d(n3)));
        return blend(a, b, t);
    }
    let rhs3 = [p[0] - c[0], p[2] - c[2]];
    let n7 = dot2(lhs, rhs3);
    let n8 = dot2(lhs2, rhs3);
    if n8 >= 0.0 && n7 <= n8 {
        return c;
    }
    let n9 = r(d(n7) * d(n2) - d(n1) * d(n8));
    if n2 >= 0.0 && n8 <= 0.0 && n9 <= 0.0 {
        let t = r(d(n2) / (d(n2) - d(n8)));
        return blend(a, c, t);
    }
    let n11 = r(d(n3) * d(n8) - d(n7) * d(n4));
    if d(n4) - d(n3) >= 0.0 && d(n7) - d(n8) >= 0.0 && n11 <= 0.0 {
        let t = r((d(n4) - d(n3)) / (d(n4) - d(n3) + (d(n7) - d(n8))));
        return cs::add(b, cs::mul(cs::sub(c, b), t));
    }
    let n13 = r(1.0 / (d(n11) + d(n9) + d(n5)));
    let n14 = r(d(n9) * d(n13));
    let n15 = r(d(n5) * d(n13));
    let y = r((1.0 - d(n14) - d(n15)) * d(a[1]) + d(n14) * d(b[1]) + d(n15) * d(c[1]));
    [p[0], y, p[2]]
}

/// `BBTree.SquaredRectPointDistance`.
fn sq_rect_point_distance(rect: [i32; 4], p: V3) -> f32 {
    let k = |v: i32| r(d(v as f32) * d(0.001_f32));
    let x = p[0].max(k(rect[0])).min(k(rect[2]));
    let z = p[2].max(k(rect[1])).min(k(rect[3]));
    r((d(x) - d(p[0])) * (d(x) - d(p[0])) + (d(z) - d(p[2])) * (d(z) - d(p[2])))
}

/// `NNInfoInternal` while a tile search runs: the constrained node and position.
#[derive(Debug, Clone, Copy)]
struct Found {
    node: Option<NodeId>,
    pos: V3,
}

impl World {
    /// `TriangleMeshNode.ClosestPointOnNode`.
    #[must_use]
    pub fn tri_closest_point(&self, i: usize, p: V3) -> V3 {
        let [a, b, c] = self.tri_vertices(i).map(cs::from_int3);
        closest_point_on_triangle(a, b, c, p)
    }

    /// `TriangleMeshNode.ClosestPointOnNodeXZ`.
    #[must_use]
    pub fn tri_closest_point_xz(&self, i: usize, p: V3) -> V3 {
        let [a, b, c] = self.tri_vertices(i).map(cs::from_int3);
        closest_point_on_triangle_xz(a, b, c, p)
    }

    #[expect(clippy::too_many_arguments, reason = "mirrors the recursive C# helper")]
    fn search_box(
        &self,
        tile: usize,
        boxi: usize,
        p: V3,
        best_sq: &mut f32,
        c: Option<&Constraint>,
        found: &mut Found,
        xz: bool,
    ) {
        let rc = self.recast.as_ref().expect("navmesh");
        let tree = &rc.tiles[tile].bbtree;
        let bx = tree.boxes[boxi];
        if bx.node_offset >= 0 {
            for i in 0..4 {
                let Some(n) = tree.lookup[bx.node_offset as usize + i] else { break };
                let ti = n as usize;
                if xz {
                    if c.is_none_or(|c| c.suitable(self, n)) {
                        let mut v = self.tri_closest_point_xz(ti, p);
                        // `constraint.distanceXZkeepY` (a null constraint would throw).
                        if c.is_some_and(|c| c.distance_xz_keep_y) {
                            v[1] = self.tri_closest_point(ti, v)[1];
                        }
                        let num = r((d(v[0]) - d(p[0])) * (d(v[0]) - d(p[0]))
                            + (d(v[2]) - d(p[2])) * (d(v[2]) - d(p[2])));
                        let better = found.node.is_none()
                            || d(num) < d(*best_sq) - d(1e-6_f32)
                            || (d(num) <= d(*best_sq) + d(1e-6_f32)
                                && (v[1] - p[1]).abs() < (found.pos[1] - p[1]).abs());
                        if better {
                            found.node = Some(n);
                            found.pos = v;
                            *best_sq = num;
                        }
                    }
                } else {
                    let v = self.tri_closest_point(ti, p);
                    let sq = cs::sqr_magnitude(cs::sub(v, p));
                    if sq < *best_sq && c.is_none_or(|c| c.suitable(self, n)) {
                        found.node = Some(n);
                        found.pos = v;
                        *best_sq = sq;
                    }
                }
            }
        } else {
            let (mut first, mut second) = (bx.left as usize, bx.right as usize);
            let mut fd = sq_rect_point_distance(tree.boxes[first].rect, p);
            let mut sd = sq_rect_point_distance(tree.boxes[second].rect, p);
            if sd < fd {
                std::mem::swap(&mut first, &mut second);
                std::mem::swap(&mut fd, &mut sd);
            }
            let go = |dist: f32, best: f32| if xz { dist <= best } else { dist < best };
            if go(fd, *best_sq) {
                self.search_box(tile, first, p, best_sq, c, found, xz);
            }
            if go(sd, *best_sq) {
                self.search_box(tile, second, p, best_sq, c, found, xz);
            }
        }
    }

    /// `BBTree.QueryClosest` / `QueryClosestXZ`.
    fn query_closest(
        &self,
        tile: usize,
        p: V3,
        c: Option<&Constraint>,
        distance: &mut f32,
        found: &mut Found,
        xz: bool,
    ) {
        let rc = self.recast.as_ref().expect("navmesh");
        let tree = &rc.tiles[tile].bbtree;
        let mut best_sq = r(d(*distance) * d(*distance));
        let start = best_sq;
        if !tree.boxes.is_empty() && sq_rect_point_distance(tree.boxes[0].rect, p) < best_sq {
            self.search_box(tile, 0, p, &mut best_sq, c, found, xz);
            if best_sq < start {
                *distance = r(d(best_sq).sqrt());
            }
        }
    }

    /// `NavmeshBase.GetNearestForce`.
    #[must_use]
    #[expect(
        clippy::neg_cmp_op_on_partial_ord,
        reason = "loop conditions mirror the C# (NaN semantics)"
    )]
    pub fn navmesh_nearest_force(&self, position: V3, c: Option<&Constraint>) -> Nearest {
        let Some(rc) = &self.recast else { return Nearest::NONE };
        let local = cs::sub(position, rc.translation);
        let tx = r(d(local[0]) / d(rc.tile_world_size_x)) as i32;
        let tz = r(d(local[2]) / d(rc.tile_world_size_z)) as i32;
        let tx = tx.clamp(0, rc.tile_x - 1);
        let tz = tz.clamp(0, rc.tile_z - 1);
        let mut num = rc.tile_x.max(rc.tile_z);
        let mut found = Found { node: None, pos: [0.0; 3] };
        let mut distance = f32::INFINITY;
        let xz = rc.nearest_search_only_xz || c.is_some_and(|c| c.distance_xz);
        let size = rc.tile_world_size_x.max(rc.tile_world_size_x);
        let mut i = 0;
        while i < num && !(d(distance) < d((i - 2) as f32) * d(size)) {
            let end = (i + tz + 1).min(rc.tile_z);
            let mut j = (-i + tz).max(0);
            while j < end {
                let n3 = (i - (j - tz).abs()).abs();
                let mut n4 = n3;
                loop {
                    let x = -n4 + tx;
                    if x >= 0 && x < rc.tile_x {
                        let tile = (x + j * rc.tile_x) as usize;
                        self.query_closest(tile, position, c, &mut distance, &mut found, xz);
                        if xz && distance < f32::INFINITY {
                            num = num.min(i + 2);
                        }
                    }
                    n4 = -n4;
                    if n4 == n3 {
                        break;
                    }
                }
                j += 1;
            }
            i += 1;
        }
        Nearest { node: found.node, position: found.pos }
    }

    /// `GridGraph.GetNearestForce`.
    #[must_use]
    #[expect(
        clippy::neg_cmp_op_on_partial_ord,
        reason = "loop conditions mirror the C# (NaN semantics)"
    )]
    pub fn grid_nearest_force(&self, position: V3, c: Option<&Constraint>) -> Nearest {
        let Some(g) = &self.grid else { return Nearest::NONE };
        let local = g.inverse_transform(position);
        let (x, z) = (local[0], local[2]);
        let cx = (x as i32).clamp(0, g.width - 1);
        let cz = (z as i32).clamp(0, g.depth - 1);
        let cell_y = |cell: usize| g.inverse_transform(cs::from_int3(g.positions[cell]))[1];
        let clamped = |j: i32, k: i32, cell: usize| {
            g.transform([
                cs::clamp(x, j as f32, j as f32 + 1.0),
                cell_y(cell),
                cs::clamp(z, k as f32, k as f32 + 1.0),
            ])
        };
        let sq =
            |cell: usize| cs::sqr_magnitude(cs::sub(cs::from_int3(g.positions[cell]), position));
        let first = (cx + cz * g.width) as usize;
        let mut best: Option<usize> = None;
        let mut best_sq = f32::INFINITY;
        let mut left = 2;
        let mut pos = [0.0; 3];
        if c.is_none_or(|c| c.suitable(self, g.first + first as u32)) {
            best = Some(first);
            best_sq = sq(first);
            pos = clamped(cx, cz, first);
        }
        if best.is_some() {
            left -= 1;
        }
        let lim = if c.is_none_or(|c| c.constrain_distance) {
            self.max_nearest_node_distance
        } else {
            f32::INFINITY
        };
        let lim_sq = r(d(lim) * d(lim));
        let mut i = 1;
        while !(d(g.node_size) * d(i as f32) > d(lim)) {
            let mut any = false;
            let mut visit =
                |j: i32, k: i32, best: &mut Option<usize>, best_sq: &mut f32, pos: &mut V3| {
                    if j < 0 || k < 0 || j >= g.width || k >= g.depth {
                        return;
                    }
                    any = true;
                    let cell = (j + k * g.width) as usize;
                    if c.is_none_or(|c| c.suitable(self, g.first + cell as u32)) {
                        let s = sq(cell);
                        if s < *best_sq && s < lim_sq {
                            *best_sq = s;
                            *best = Some(cell);
                            *pos = clamped(j, k, cell);
                        }
                    }
                };
            for j in cx - i..=cx + i {
                visit(j, cz + i, &mut best, &mut best_sq, &mut pos);
            }
            for j in cx - i..=cx + i {
                visit(j, cz - i, &mut best, &mut best_sq, &mut pos);
            }
            for k in cz - i + 1..cz + i {
                visit(cx - i, k, &mut best, &mut best_sq, &mut pos);
            }
            for k in cz - i + 1..cz + i {
                visit(cx + i, k, &mut best, &mut best_sq, &mut pos);
            }
            if best.is_some() {
                if left == 0 {
                    break;
                }
                left -= 1;
            }
            if !any {
                break;
            }
            i += 1;
        }
        Nearest { node: best.map(|b| g.first + b as u32), position: pos }
    }

    /// `GridGraph.GetNearest` (the unforced, cell-only lookup).
    #[must_use]
    pub fn grid_nearest(&self, position: V3) -> Nearest {
        let Some(g) = &self.grid else { return Nearest::NONE };
        let local = g.inverse_transform(position);
        let (x, z) = (local[0], local[2]);
        let cx = (x as i32).clamp(0, g.width - 1);
        let cz = (z as i32).clamp(0, g.depth - 1);
        let cell = (cz * g.width + cx) as usize;
        let y = g.inverse_transform(cs::from_int3(g.positions[cell]))[1];
        Nearest {
            node: Some(g.first + cell as u32),
            position: g.transform([
                cs::clamp(x, cx as f32, cx as f32 + 1.0),
                y,
                cs::clamp(z, cz as f32, cz as f32 + 1.0),
            ]),
        }
    }

    /// `AstarPath.GetNearest(position, constraint)`.
    #[must_use]
    pub fn get_nearest(&self, position: V3, c: &Constraint) -> Nearest {
        let mut best = f32::INFINITY;
        let mut info = Nearest::NONE;
        let mut graph: Option<u32> = None;
        let mut graphs: Vec<(u32, bool)> = Vec::new();
        if let Some(rc) = &self.recast {
            graphs.push((rc.graph_index, true));
        }
        if let Some(g) = &self.grid {
            graphs.push((g.graph_index, false));
        }
        graphs.sort_unstable();
        for &(gi, is_mesh) in &graphs {
            if !c.suitable_graph(gi) {
                continue;
            }
            let nn = self.graph_nearest(is_mesh, position, c, self.full_get_nearest_search);
            if nn.node.is_some() {
                let m = cs::magnitude(cs::sub(nn.position, position));
                if m < best {
                    best = m;
                    info = nn;
                    graph = Some(gi);
                }
            }
        }
        let Some(gi) = graph else { return Nearest::NONE };
        if !self.full_get_nearest_search
            && let Some(n) = info.node
            && !c.suitable(self, n)
        {
            let is_mesh = self.recast.as_ref().is_some_and(|r| r.graph_index == gi);
            let f = self.graph_nearest(is_mesh, position, c, true);
            if f.node.is_some() {
                info = f;
            }
        }
        let Some(n) = info.node else { return Nearest::NONE };
        if !c.suitable(self, n) {
            return Nearest::NONE;
        }
        if c.constrain_distance {
            let mut v = cs::sub(info.position, position);
            if c.distance_xz && !c.distance_xz_keep_y {
                v[1] = 0.0;
            }
            let max_sq = r(d(self.max_nearest_node_distance) * d(self.max_nearest_node_distance));
            if cs::sqr_magnitude(v) > max_sq {
                return Nearest::NONE;
            }
        }
        info
    }

    fn graph_nearest(&self, is_mesh: bool, position: V3, c: &Constraint, force: bool) -> Nearest {
        match (is_mesh, force) {
            (true, true) => self.navmesh_nearest_force(position, Some(c)),
            // NavmeshBase.GetNearest: a forced search without the constraint (or with
            // the XZ-only one).
            (true, false) => {
                if c.distance_xz {
                    let xz = Constraint { distance_xz: true, ..Constraint::default_() };
                    self.navmesh_nearest_force(position, Some(&xz))
                } else {
                    self.navmesh_nearest_force(position, None)
                }
            }
            (false, true) => self.grid_nearest_force(position, Some(c)),
            (false, false) => self.grid_nearest(position),
        }
    }

    /// `NavmeshBase.GetNearest(position, NNConstraint.None)` (used by linecasts).
    #[must_use]
    pub fn navmesh_get_nearest_unconstrained(&self, position: V3) -> Nearest {
        self.navmesh_nearest_force(position, None)
    }

    /// `GridNode.ClosestPointOnNode`.
    #[must_use]
    pub fn grid_closest_point_on_node(&self, cell: usize, p: V3) -> V3 {
        let g = self.grid.as_ref().expect("grid");
        let lp = g.inverse_transform(p);
        let x = cell as i32 % g.width;
        let z = cell as i32 / g.width;
        let y = g.inverse_transform(cs::from_int3(g.positions[cell]))[1];
        g.transform([
            cs::clamp(lp[0], x as f32, x as f32 + 1.0),
            y,
            cs::clamp(lp[2], z as f32, z as f32 + 1.0),
        ])
    }

    /// Node position as a Vector3.
    #[must_use]
    pub fn position_v3(&self, n: NodeId) -> V3 {
        cs::from_int3(self.position(n))
    }

    /// Whether a node is a grid cell.
    #[must_use]
    pub fn is_cell(&self, n: NodeId) -> Option<usize> {
        match self.kind(n) {
            Kind::Cell(c) => Some(c),
            Kind::Tri(_) => None,
        }
    }
}
