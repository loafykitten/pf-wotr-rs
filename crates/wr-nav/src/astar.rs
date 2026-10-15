//! The A* search as A* Pathfinding Project 4.3.10 runs it: `PathHandler` and `PathNode`
//! bookkeeping, the 4-ary `BinaryHeap` with its tie-breaking, `TriangleMeshNode.Open`
//! (connections backwards, endpoint special costs, `UpdateRecursiveG` re-opening),
//! `GridNode.Open` (directions 0..7), and `ABPath`/`XPath` preparation, initialisation,
//! stepping and tracing (`GetNearest` endpoints with `PathNNConstraint`, the grid
//! end-point special case, `CompleteWith`, `ChangeEndNode`), and Owlcat's
//! `ABPathWithClearance` (an `XPath` that is its own `ITraversalProvider`: the cells
//! around the start node within the seeker's clearance ignore dynamic blocking; its
//! ending conditions `MaxDistanceEndingCondition` and `LosPathConditionForGrid`, whose
//! line-of-sight answers are the caller's, and `ModifyHScore`).

use super::cs::{self, V3, d};
use super::nearest::Constraint;
use super::world::{Kind, NodeId, World};
use crate::Int3;

const NONE: u32 = u32::MAX;
const NOT_IN_HEAP: u16 = u16::MAX;

/// `PathNode`.
#[derive(Debug, Clone, Copy)]
struct PathNode {
    parent: u32,
    path_id: u16,
    heap_index: u16,
    /// Low 28 bits: cost; bit 28: flag1; bit 29: flag2.
    flags: u32,
    g: u32,
    h: u32,
}

impl Default for PathNode {
    fn default() -> Self {
        Self { parent: NONE, path_id: 0, heap_index: NOT_IN_HEAP, flags: 0, g: 0, h: 0 }
    }
}

impl PathNode {
    fn cost(&self) -> u32 {
        self.flags & 0x0FFF_FFFF
    }
    fn set_cost(&mut self, c: u32) {
        // `flags = (flags & 0xF0000000) | value` (an oversized value leaks into the flags,
        // as in the original).
        self.flags = (self.flags & 0xF000_0000) | c;
    }
    fn flag1(&self) -> bool {
        self.flags & 0x1000_0000 != 0
    }
    fn set_flag1(&mut self, v: bool) {
        self.flags = (self.flags & 0xEFFF_FFFF) | if v { 0x1000_0000 } else { 0 };
    }
    fn flag2(&self) -> bool {
        self.flags & 0x2000_0000 != 0
    }
    fn set_flag2(&mut self, v: bool) {
        self.flags = (self.flags & 0xDFFF_FFFF) | if v { 0x2000_0000 } else { 0 };
    }
    fn f(&self) -> u32 {
        self.g.wrapping_add(self.h)
    }
}

/// `BinaryHeap` (D = 4, sorting ties by G).
#[derive(Debug, Default)]
struct Heap {
    /// (F at insertion, node).
    items: Vec<(u32, u32)>,
}

impl Heap {
    fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    fn add(&mut self, nodes: &mut [PathNode], n: u32) {
        let hi = nodes[n as usize].heap_index;
        if hi != NOT_IN_HEAP {
            let t = self.items[hi as usize];
            self.decrease_key(nodes, t, hi as usize);
            return;
        }
        self.items.push((0, n));
        let idx = self.items.len() - 1;
        self.decrease_key(nodes, (0, n), idx);
    }

    fn decrease_key(&mut self, nodes: &mut [PathNode], t: (u32, u32), index: usize) {
        let mut i = index;
        let f = nodes[t.1 as usize].f();
        let g = nodes[t.1 as usize].g;
        while i != 0 {
            let p = (i - 1) / 4;
            let (pf, pn) = self.items[p];
            if f >= pf && (f != pf || g <= nodes[pn as usize].g) {
                break;
            }
            self.items[i] = self.items[p];
            nodes[self.items[i].1 as usize].heap_index = i as u16;
            i = p;
        }
        self.items[i] = (f, t.1);
        nodes[t.1 as usize].heap_index = i as u16;
    }

    fn remove(&mut self, nodes: &mut [PathNode]) -> u32 {
        let top = self.items[0].1;
        nodes[top as usize].heap_index = NOT_IN_HEAP;
        let count = self.items.len() - 1;
        if count == 0 {
            self.items.pop();
            return top;
        }
        let t = self.items[count];
        self.items.pop();
        let g = nodes[t.1 as usize].g;
        let mut num = 0usize;
        loop {
            let num2 = num;
            let mut f_best = t.0;
            let c = num2 * 4 + 1;
            if c <= count {
                let gat = |k: usize, items: &Vec<(u32, u32)>| nodes[items[k].1 as usize].g;
                let cur_g = |num: usize, items: &Vec<(u32, u32)>| {
                    if num == num2 { g } else { gat(num, items) }
                };
                for k in 0..4 {
                    let idx = c + k;
                    if idx < count {
                        let fk = self.items[idx].0;
                        let better = if k == 0 {
                            fk < f_best || (fk == f_best && gat(idx, &self.items) < g)
                        } else {
                            fk < f_best
                                || (fk == f_best && gat(idx, &self.items) < cur_g(num, &self.items))
                        };
                        if better {
                            f_best = fk;
                            num = idx;
                        }
                    }
                }
            }
            if num2 == num {
                break;
            }
            self.items[num2] = self.items[num];
            nodes[self.items[num2].1 as usize].heap_index = num2 as u16;
        }
        self.items[num] = t;
        nodes[t.1 as usize].heap_index = num as u16;
        top
    }
}

/// Which `Path` class runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// `ABPath`.
    Ab,
    /// `XPath` (`ABPathWithClearance` with [`Request::clearance`]), ending as
    /// [`Request::ending`] says.
    X,
}

/// An `XPath`'s `endingCondition`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ending {
    /// `ABPathEndingCondition`: the end node.
    Ab,
    /// `ABPathWithClearance.MaxDistanceEndingCondition`: the end node, or a node at least
    /// `max` from the original start.
    MaxDistance { max: f32 },
    /// `ABPathWithClearance.LosPathConditionForGrid`.
    Los { approach_radius: f32, check_los: bool, ignored_blocker_radius: f32, max: f32 },
}

/// One `LineOfSightGeometry.HasObstacle(from, to, fudgeRadius)` call the search makes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LosQuery {
    pub from: V3,
    pub to: V3,
    pub fudge: f32,
    /// The node it was asked for.
    pub node: NodeId,
}

/// The line-of-sight answers a search gets: `Some(has_obstacle)`, or `None` when unknown
/// (the search then stops, reported in [`SearchResult::los_unknown`]).
pub type LosFn<'a> = dyn FnMut(&LosQuery) -> Option<bool> + 'a;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum State {
    NotCalculated,
    Error,
    Complete,
    Partial,
}

/// The outcome of one search, with the fields the oracle records.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub state: State,
    pub error: String,
    pub searched_nodes: i32,
    pub start_point: V3,
    pub end_point: V3,
    pub original_start: V3,
    pub original_end: V3,
    pub start_node: Option<NodeId>,
    pub end_node: Option<NodeId>,
    pub path: Vec<NodeId>,
    pub vector_path: Vec<V3>,
    pub end_node_g: Option<u32>,
    /// The path's `nnConstraint` after `Prepare` (area and tags set), for modifiers.
    pub constraint: Constraint,
    /// Line-of-sight queries made, in order, with the answers used.
    pub los: Vec<(LosQuery, bool)>,
    /// The query the search stopped at for want of an answer.
    pub los_unknown: Option<LosQuery>,
}

/// A path request.
#[derive(Debug, Clone, Copy)]
pub struct Request {
    pub kind: PathKind,
    pub start: V3,
    pub end: V3,
    pub graph_mask: i32,
    pub enabled_tags: i32,
    pub calculate_partial: bool,
    /// `ABPathWithClearance.clearance` (the start node's region, `Bounds` size); `None`:
    /// a plain `ABPath`/`XPath` (no traversal provider).
    pub clearance: Option<V3>,
    /// The `XPath`'s ending condition (ignored for `ABPath`).
    pub ending: Ending,
    /// `nnConstraint.distanceXZ` (the distance limit of `AstarPath.GetNearest` ignores height).
    pub distance_xz: bool,
}

struct Search<'w, 'l> {
    w: &'w World,
    ending: Ending,
    clearance: Option<V3>,
    /// `startNodeNeighbors`, as a membership mask over grid cells.
    start_neighbors: Option<Vec<bool>>,
    los_fn: &'l mut LosFn<'l>,
    los: Vec<(LosQuery, bool)>,
    los_unknown: Option<LosQuery>,
    nodes: Vec<PathNode>,
    heap: Heap,
    path_id: u16,
    kind: PathKind,
    enabled_tags: i32,
    start_node: Option<NodeId>,
    end_node: Option<NodeId>,
    start_point: V3,
    end_point: V3,
    original_start: V3,
    original_end: V3,
    start_int_point: Int3,
    h_target: Int3,
    /// `hTargetNode != null` (set with the end node in `Prepare`).
    h_target_node: bool,
    state: State,
    error: String,
    searched: i32,
    path: Vec<NodeId>,
    vector_path: Vec<V3>,
    current: u32,
    partial_best: u32,
    calculate_partial: bool,
    constraint: Constraint,
}

impl Search<'_, '_> {
    fn fail(&mut self, msg: &str) {
        self.state = State::Error;
        if self.error.is_empty() {
            self.error = msg.to_owned();
        } else {
            self.error = format!("{}\n{msg}", self.error);
        }
    }

    fn set_complete(&mut self, s: State) {
        if self.state != State::Error {
            self.state = s;
        }
    }

    /// `Path.CanTraverse` without a traversal provider (`DefaultITraversalProvider`).
    fn default_can_traverse(&self, n: NodeId) -> bool {
        self.w.walkable(n) && (self.enabled_tags >> self.w.tag(n)) & 1 != 0
    }

    /// `Path.CanTraverse`: `ABPathWithClearance.CanTraverse` once `Prepare` set it as the
    /// provider (the start's neighbours need only be statically walkable).
    fn can_traverse(&self, n: NodeId) -> bool {
        if let (Some(mask), Some(c)) = (&self.start_neighbors, self.w.is_cell(n))
            && mask[c]
        {
            return self.w.static_walkable(n) && (self.enabled_tags >> self.w.tag(n)) & 1 != 0;
        }
        self.default_can_traverse(n)
    }

    /// `Path.CalculateHScore` (Euclidean, scale 1; the embedding is off, so its `Max` is
    /// the identity), then `ModifyHScore` once the end node is the heuristic target.
    fn h_score(&self, n: NodeId) -> u32 {
        let c = cs::cost_magnitude(cs::int3_sub(self.h_target, self.w.position(n)));
        let h = cs::to_u32(d(c as f32) * 1.0);
        if !self.h_target_node {
            return h;
        }
        // ABPathWithClearance.ModifyHScore: `(uint)(ApproachRadius * 1000f)` off, at least 0.
        if let (PathKind::X, Ending::Los { approach_radius, .. }) = (self.kind, self.ending) {
            let num = cs::to_u32(d(approach_radius) * 1000.0);
            return h.saturating_sub(num);
        }
        h
    }

    /// `ABPath.GetConnectionSpecialCost`.
    fn special_cost(&self, a: NodeId, b: NodeId, current: u32) -> u32 {
        let pa = self.w.position(a);
        let pb = self.w.position(b);
        let scale =
            || f64::from(current) * 1.0 / f64::from(cs::cost_magnitude(cs::int3_sub(pa, pb)));
        let cm = |p: Int3| f64::from(cs::cost_magnitude(p));
        if let (Some(s), Some(e)) = (self.start_node, self.end_node) {
            if a == s {
                let other = if b == e { self.h_target } else { pb };
                return cs::to_u32(cm(cs::int3_sub(self.start_int_point, other)) * scale());
            }
            if b == s {
                let other = if a == e { self.h_target } else { pa };
                return cs::to_u32(cm(cs::int3_sub(self.start_int_point, other)) * scale());
            }
            if a == e {
                return cs::to_u32(cm(cs::int3_sub(self.h_target, pb)) * scale());
            }
            if b == e {
                return cs::to_u32(cm(cs::int3_sub(self.h_target, pa)) * scale());
            }
        } else {
            if Some(a) == self.start_node {
                return cs::to_u32(cm(cs::int3_sub(self.start_int_point, pb)) * scale());
            }
            if Some(b) == self.start_node {
                return cs::to_u32(cm(cs::int3_sub(self.start_int_point, pa)) * scale());
            }
        }
        current
    }

    fn update_g(&mut self, n: u32) {
        let p = self.nodes[n as usize].parent;
        let pg = self.nodes[p as usize].g;
        let c = self.nodes[n as usize].cost();
        // + GetTraversalCost: tag penalties and node penalties are 0 here.
        self.nodes[n as usize].g = pg.wrapping_add(c);
    }

    fn update_recursive_g(&mut self, n: u32) {
        self.update_g(n);
        self.heap.add(&mut self.nodes, n);
        let mut buf = Vec::new();
        self.w.connections(n, &mut buf);
        for o in buf {
            let po = self.nodes[o as usize];
            if po.parent == n && po.path_id == self.path_id {
                self.update_recursive_g(o);
            }
        }
    }

    /// `GraphNode.Open` for the node's type.
    fn open(&mut self, n: u32) {
        match self.w.kind(n) {
            Kind::Tri(i) => self.open_tri(n, i),
            Kind::Cell(c) => self.open_cell(n, c),
        }
    }

    fn open_tri(&mut self, n: u32, i: usize) {
        let flag = self.nodes[n as usize].flag2();
        let conns = &self.w.tris[i].conns;
        for k in (0..conns.len()).rev() {
            let conn = conns[k];
            let o = conn.node;
            if !self.can_traverse(o) {
                continue;
            }
            if o == self.nodes[n as usize].parent {
                continue;
            }
            let mut cost = conn.cost;
            if flag || self.nodes[o as usize].flag2() {
                cost = self.special_cost(n, o, cost);
            }
            if self.nodes[o as usize].path_id != self.path_id {
                let h = self.h_score(o);
                let po = &mut self.nodes[o as usize];
                po.parent = n;
                po.path_id = self.path_id;
                po.set_cost(cost);
                po.h = h;
                self.update_g(o);
                self.heap.add(&mut self.nodes, o);
            } else if self.nodes[n as usize].g.wrapping_add(cost) < self.nodes[o as usize].g {
                let po = &mut self.nodes[o as usize];
                po.set_cost(cost);
                po.parent = n;
                self.update_recursive_g(o);
            }
        }
    }

    fn open_cell(&mut self, n: u32, c: usize) {
        let g = self.w.grid.as_ref().expect("grid");
        for dir in 0..8 {
            if !g.has_connection(c, dir) {
                continue;
            }
            let m = (c as i64 + i64::from(g.neighbour_offsets[dir])) as usize;
            let o = g.first + m as u32;
            if !self.can_traverse(o) {
                continue;
            }
            let cost = g.neighbour_costs[dir];
            if self.nodes[o as usize].path_id != self.path_id {
                let h = self.h_score(o);
                let po = &mut self.nodes[o as usize];
                po.parent = n;
                po.path_id = self.path_id;
                po.set_cost(cost);
                po.h = h;
                self.update_g(o);
                self.heap.add(&mut self.nodes, o);
            } else if self.nodes[n as usize].g.wrapping_add(cost) < self.nodes[o as usize].g {
                let po = &mut self.nodes[o as usize];
                po.set_cost(cost);
                po.parent = n;
                self.update_recursive_g(o);
            }
        }
    }

    /// `ABPath.EndPointGridGraphSpecialCase`.
    fn grid_special_case(&mut self, closest: NodeId) -> bool {
        if self.kind == PathKind::X {
            return false;
        }
        let Some(cell) = self.w.is_cell(closest) else { return false };
        let g = self.w.grid.as_ref().expect("grid");
        let nn = self.w.get_nearest(self.original_end, &Constraint::none());
        let Some(n2) = nn.node else { return false };
        let Some(cell2) = self.w.is_cell(n2) else { return false };
        if cell2 == cell {
            return false;
        }
        let (x, z) = (cell as i32 % g.width, cell as i32 / g.width);
        let (x2, z2) = (cell2 as i32 % g.width, cell2 as i32 / g.width);
        let adjacent = if g.eight_neighbours {
            (x - x2).abs() <= 1 && (z - z2).abs() <= 1
        } else {
            (x == x2 && (z - z2).abs() == 1) || (z == z2 && (x - x2).abs() == 1)
        };
        if !adjacent {
            return false;
        }
        let count = if g.eight_neighbours { 8 } else { 4 };
        for i in 0..count {
            let nx = x2 + g.neighbour_x[i];
            let nz = z2 + g.neighbour_z[i];
            if nx >= 0 && nz >= 0 && nx < g.width && nz < g.depth {
                let id = g.first + (nz * g.width + nx) as u32;
                self.nodes[id as usize].set_flag1(true);
            }
        }
        self.end_point = self.w.position_v3(n2);
        self.h_target = self.w.position(n2);
        self.end_node = Some(n2);
        true
    }

    fn prepare(&mut self) {
        self.constraint.tags = self.enabled_tags;
        let nn = self.w.get_nearest(self.start_point, &self.constraint);
        // PathNNConstraint.SetStart.
        match nn.node {
            Some(n) => self.constraint.area = i64::from(self.w.area[n as usize]),
            None => self.constraint.constrain_area = false,
        }
        self.start_point = nn.position;
        self.start_int_point = cs::to_int3(self.start_point);
        self.start_node = nn.node;
        let Some(s) = self.start_node else {
            self.fail("Couldn't find a node close to the start point");
            return;
        };
        if !self.default_can_traverse(s) {
            self.fail("The node closest to the start point could not be traversed");
            return;
        }
        let nn2 = self.w.get_nearest(self.end_point, &self.constraint);
        self.end_point = nn2.position;
        self.end_node = nn2.node;
        let Some(e) = self.end_node else {
            self.fail("Couldn't find a node close to the end point");
            return;
        };
        if !self.default_can_traverse(e) {
            self.fail("The node closest to the end point could not be traversed");
        } else if self.w.area[s as usize] != self.w.area[e as usize] {
            self.fail("There is no valid path to the target");
        } else if !self.grid_special_case(e) {
            self.h_target = cs::to_int3(self.end_point);
            self.h_target_node = true;
            self.nodes[e as usize].set_flag1(true);
        }
        if self.state != State::Error
            && let Some(size) = self.clearance
        {
            // ABPathWithClearance.Prepare: the provider, and the grid cells within
            // `new Bounds((Vector3)startNode.position, clearance)`.
            if let (Some(g), Some(_)) = (&self.w.grid, self.w.is_cell(s)) {
                let b = crate::update::Bounds::new(self.w.position_v3(s), size);
                let mut mask = vec![false; g.positions.len()];
                for c in g.nodes_in_bounds(&b) {
                    mask[c] = true;
                }
                self.start_neighbors = Some(mask);
            }
        }
    }

    fn trace(&mut self, from: u32) {
        let mut v = Vec::new();
        let mut p = from;
        while p != NONE {
            v.push(p);
            p = self.nodes[p as usize].parent;
            if v.len() > 2048 {
                break;
            }
        }
        v.reverse();
        self.vector_path = v.iter().map(|&n| self.w.position_v3(n)).collect();
        self.path = v;
    }

    /// `ABPath.CompleteWith`.
    fn complete_with(&mut self, n: u32) {
        if self.end_node != Some(n) {
            if let Some(cell) = self.w.is_cell(n) {
                self.end_point = self.w.grid_closest_point_on_node(cell, self.original_end);
                self.end_node = Some(n);
            } else {
                self.fail("Some path is not cleaning up the flag1 field. This is a bug.");
                return;
            }
        }
        self.set_complete(State::Complete);
    }

    /// `XPath.ChangeEndNode`.
    fn change_end_node(&mut self, target: u32) {
        if let Some(e) = self.end_node
            && Some(e) != self.start_node
        {
            self.nodes[e as usize].set_flag1(false);
            self.nodes[e as usize].set_flag2(false);
        }
        self.end_node = Some(target);
        self.end_point = self.w.position_v3(target);
    }

    /// `endingCondition.TargetFound(node)`; `None` when a line-of-sight answer is unknown.
    fn target_found(&mut self, n: u32) -> Option<bool> {
        let at_end = Some(n) == self.end_node;
        let max_reached = |s: &Self, max: f32| {
            // ((Vector3)node.position - originalStartPoint).sqrMagnitude >= max * max.
            let v = cs::sub(s.w.position_v3(n), s.original_start);
            d(cs::sqr_magnitude(v)) >= d(max) * d(max)
        };
        match self.ending {
            Ending::Ab => Some(at_end),
            Ending::MaxDistance { max } => Some(at_end || max_reached(self, max)),
            Ending::Los { approach_radius, check_los, ignored_blocker_radius, max } => {
                let v = if Some(n) == self.start_node {
                    self.original_start
                } else {
                    self.w.position_v3(n)
                };
                let num = cs::sqr_mechanics_distance(v, self.original_end);
                if at_end || max_reached(self, max) {
                    return Some(true);
                }
                if self.w.ally_blocked(n) > 0 {
                    return Some(false);
                }
                if d(num) > d(approach_radius) * d(approach_radius) {
                    return Some(false);
                }
                if check_los {
                    let q = LosQuery {
                        from: v,
                        to: self.original_end,
                        fudge: ignored_blocker_radius,
                        node: n,
                    };
                    let Some(blocked) = (self.los_fn)(&q) else {
                        self.los_unknown = Some(q);
                        return None;
                    };
                    self.los.push((q, blocked));
                    if blocked {
                        return Some(false);
                    }
                }
                Some(true)
            }
        }
    }

    fn initialize(&mut self) {
        let s = self.start_node.expect("start");
        self.nodes[s as usize].set_flag2(true);
        if let Some(e) = self.end_node {
            self.nodes[e as usize].set_flag2(true);
        }
        let h = self.h_score(s);
        {
            let p = &mut self.nodes[s as usize];
            p.path_id = self.path_id;
            p.parent = NONE;
            p.set_cost(0);
            p.g = 0;
            p.h = h;
        }
        match self.kind {
            PathKind::Ab => {
                if self.nodes[s as usize].flag1() {
                    self.complete_with(s);
                    self.trace(s);
                }
            }
            PathKind::X => match self.target_found(s) {
                None => return,
                Some(true) => {
                    self.change_end_node(s);
                    self.trace(s);
                    self.set_complete(State::Complete);
                }
                Some(false) => {}
            },
        }
        if self.state == State::Complete {
            return;
        }
        self.open(s);
        self.searched += 1;
        self.partial_best = s;
        if self.heap.is_empty() {
            if self.calculate_partial && self.kind == PathKind::Ab {
                self.set_complete(State::Partial);
                self.trace(self.partial_best);
            } else {
                self.fail("No open points, the start node didn't open any nodes");
            }
        } else {
            self.current = self.heap.remove(&mut self.nodes);
        }
    }

    fn calculate(&mut self) {
        while self.state == State::NotCalculated {
            self.searched += 1;
            let cur = self.current;
            match self.kind {
                PathKind::Ab => {
                    if self.nodes[cur as usize].flag1() {
                        self.complete_with(cur);
                        break;
                    }
                    if self.nodes[cur as usize].h < self.nodes[self.partial_best as usize].h {
                        self.partial_best = cur;
                    }
                }
                PathKind::X => match self.target_found(cur) {
                    None => return,
                    Some(true) => {
                        self.set_complete(State::Complete);
                        break;
                    }
                    Some(false) => {}
                },
            }
            self.open(cur);
            if self.heap.is_empty() {
                if self.kind == PathKind::Ab && self.calculate_partial {
                    self.set_complete(State::Partial);
                    self.trace(self.partial_best);
                } else {
                    self.fail("Searched whole area but could not find target");
                }
                return;
            }
            self.current = self.heap.remove(&mut self.nodes);
            if self.searched > 1_000_000 {
                self.fail("Probable infinite loop. Over 1,000,000 nodes searched");
                return;
            }
        }
        match self.state {
            State::Complete => {
                if self.kind == PathKind::X {
                    self.change_end_node(self.current);
                }
                self.trace(self.current);
            }
            _ if self.kind == PathKind::Ab && self.calculate_partial => {
                self.set_complete(State::Partial);
                self.trace(self.partial_best);
            }
            _ => {}
        }
    }
}

/// Runs one path request to completion, as `AstarPath.StartPath` +
/// `BlockUntilCalculated` would (no line-of-sight answers: a query stops the search).
#[must_use]
pub fn run(w: &World, req: &Request) -> SearchResult {
    run_with_los(w, req, &mut |_| None)
}

/// [`run`] with the line-of-sight answers `los` gives (`Some(has_obstacle)`).
#[must_use]
pub fn run_with_los<'l>(w: &World, req: &Request, los: &'l mut LosFn<'l>) -> SearchResult {
    let mut constraint = Constraint::path_default();
    constraint.graph_mask = req.graph_mask;
    constraint.distance_xz = req.distance_xz;
    let mut s = Search {
        w,
        ending: req.ending,
        clearance: if req.kind == PathKind::X { req.clearance } else { None },
        start_neighbors: None,
        los_fn: los,
        los: Vec::new(),
        los_unknown: None,
        h_target_node: false,
        nodes: vec![PathNode::default(); w.node_count()],
        heap: Heap::default(),
        path_id: 1,
        kind: req.kind,
        enabled_tags: req.enabled_tags,
        start_node: None,
        end_node: None,
        start_point: req.start,
        end_point: req.end,
        original_start: req.start,
        original_end: req.end,
        start_int_point: cs::to_int3(req.start),
        h_target: cs::to_int3(req.end),
        state: State::NotCalculated,
        error: String::new(),
        searched: 0,
        path: Vec::new(),
        vector_path: Vec::new(),
        current: NONE,
        partial_best: NONE,
        calculate_partial: req.calculate_partial,
        constraint,
    };
    s.prepare();
    if s.state == State::NotCalculated {
        s.initialize();
        if s.state == State::NotCalculated && s.los_unknown.is_none() {
            s.calculate();
        }
    }
    let end_node_g = match (s.state, s.end_node) {
        (State::Complete, Some(e)) if s.nodes[e as usize].path_id == s.path_id => {
            Some(s.nodes[e as usize].g)
        }
        _ => None,
    };
    SearchResult {
        state: s.state,
        error: s.error,
        searched_nodes: s.searched,
        start_point: s.start_point,
        end_point: s.end_point,
        original_start: s.original_start,
        original_end: s.original_end,
        start_node: s.start_node,
        end_node: s.end_node,
        path: s.path,
        vector_path: s.vector_path,
        end_node_g,
        constraint: s.constraint,
        los: s.los,
        los_unknown: s.los_unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap_pops_lowest_f_then_lowest_g() {
        let mut nodes = vec![PathNode::default(); 6];
        let mut h = Heap::default();
        // (g, h): F = 10 for nodes 0..3 with different G, node 4 F = 5, node 5 F = 12.
        for (i, (g, hh)) in [(4, 6), (2, 8), (7, 3), (2, 8), (1, 4), (6, 6)].into_iter().enumerate()
        {
            nodes[i].g = g;
            nodes[i].h = hh;
            h.add(&mut nodes, i as u32);
        }
        assert_eq!(h.remove(&mut nodes), 4);
        // Among F = 10, removal prefers lower G; equal G keeps heap order.
        let next = h.remove(&mut nodes);
        assert!(next == 1 || next == 3);
        let mut rest: Vec<u32> = (0..4).map(|_| h.remove(&mut nodes)).collect();
        assert_eq!(rest.pop(), Some(5));
        assert!(rest.contains(&0) && rest.contains(&2) && (rest.contains(&1) || rest.contains(&3)));
        assert!(h.is_empty());
    }

    #[test]
    fn decrease_key_moves_a_node_up() {
        let mut nodes = vec![PathNode::default(); 3];
        let mut h = Heap::default();
        for (i, f) in [5u32, 6, 7].into_iter().enumerate() {
            nodes[i].h = f;
            h.add(&mut nodes, i as u32);
        }
        nodes[2].h = 1;
        h.add(&mut nodes, 2);
        assert_eq!(h.remove(&mut nodes), 2);
        assert_eq!(h.remove(&mut nodes), 0);
    }

    fn cell(x: i32, z: i32) -> V3 {
        [x as f32 + 0.5, 0.0, z as f32 + 0.5]
    }

    fn clearance_request(start: V3, end: V3, clearance: V3, ending: Ending) -> Request {
        Request {
            kind: PathKind::X,
            start,
            end,
            graph_mask: 1,
            enabled_tags: i32::MAX,
            calculate_partial: false,
            clearance: Some(clearance),
            ending,
            distance_xz: false,
        }
    }

    /// `distanceXZ`: `AstarPath.GetNearest`'s distance limit ignores the height gap.
    #[test]
    fn distance_xz_drops_the_height_from_the_nearest_node_limit() {
        let mut w = crate::update::tests::flat(5, 5);
        w.max_nearest_node_distance = 1.0;
        let above = [2.5, 2.0, 2.5];
        let mut req = clearance_request(
            above,
            cell(2, 4),
            [1.0, 0.0, 1.0],
            Ending::MaxDistance { max: f32::MAX },
        );
        let r = run(&w, &req);
        assert_eq!(r.state, State::Error);
        assert_eq!(r.error, "Couldn't find a node close to the start point");
        req.distance_xz = true;
        let r = run(&w, &req);
        assert_eq!(r.state, State::Complete, "{}", r.error);
    }

    /// `ABPathWithClearance.CanTraverse`: a cell around the start node (within the clearance
    /// box, at the start's height) is traversed when only statically walkable; elsewhere the
    /// dynamic blocking counts.
    #[test]
    fn the_start_nodes_neighbours_ignore_dynamic_blocking() {
        let mut w = crate::update::tests::flat(5, 5);
        // Blocked without recomputing connections: the provider alone decides.
        let centre = w.position_v3(12);
        let g = w.grid.as_mut().unwrap();
        g.dynamic_blocked = vec![0; 25];
        g.dynamic_blocked[5 + 2] = 1;
        g.dynamic_blocked[3 * 5 + 2] = 1;
        // `(Vector3)Int3` scales by `0.001f`, a little over a thousandth: the box around
        // (2.5, 2.5) of size 2 holds the cells at 1.5000001 but not those at 3.5000002.
        let b = crate::update::Bounds::new(centre, [2.0, 0.0, 2.0]);
        assert_eq!(g.nodes_in_bounds(&b), [6, 11, 7, 12]);
        let ending = Ending::MaxDistance { max: f32::MAX };
        let path = |w: &World, end: V3, clear: f32| {
            let r = run(w, &clearance_request(cell(2, 2), end, [clear, 0.0, clear], ending));
            assert_eq!(r.state, State::Complete, "{}", r.error);
            r.path.iter().map(|&n| w.position(n)).collect::<Vec<_>>()
        };
        let through = path(&w, cell(2, 0), 2.0);
        assert_eq!(through, [[2500, 0, 2500], [2500, 0, 1500], [2500, 0, 500]]);
        for (end, clear) in [(cell(2, 0), 0.0), (cell(2, 4), 2.0)] {
            let around = path(&w, end, clear);
            assert_eq!(around.len(), 3);
            assert_ne!(around[1][0], 2500);
        }
    }

    /// `LosPathConditionForGrid`: a node within the approach radius ends the search once its
    /// line of sight is clear; an ally-blocked one never does (nor is it the end node), and
    /// `ModifyHScore` takes the radius off the heuristic.
    #[test]
    fn the_los_ending_condition_skips_ally_blocked_nodes() {
        let mut w = crate::update::tests::flat(7, 7);
        let g = w.grid.as_mut().unwrap();
        g.ally_blocked = vec![0; 49];
        let ally = 3 * 7 + 3;
        g.ally_blocked[ally as usize] = 1;
        let ending = Ending::Los {
            approach_radius: 2.1,
            check_los: true,
            ignored_blocker_radius: 0.0,
            max: f32::MAX,
        };
        // Along z = 3 towards (5, 3): (3, 3) is first in range but ally-blocked, so (4, 3)
        // asks and ends the search.
        let req = clearance_request(cell(0, 3), cell(5, 3), [1.0, 0.0, 1.0], ending);
        let mut asked = Vec::new();
        let r = run_with_los(&w, &req, &mut |q| {
            asked.push(q.node);
            Some(false)
        });
        assert_eq!(r.state, State::Complete);
        assert_eq!(asked, [3 * 7 + 4]);
        assert_eq!(r.end_node, Some(3 * 7 + 4));
        // The end point on the ally-blocked cell: the nearest node is another.
        let on_ally = clearance_request(cell(0, 3), cell(3, 3), [1.0, 0.0, 1.0], ending);
        let r = run_with_los(&w, &on_ally, &mut |_| Some(true));
        assert!(r.end_node.is_some_and(|e| e != ally));
        // Every line of sight blocked: the search runs on to the end node.
        let r = run_with_los(&w, &req, &mut |_| Some(true));
        assert_eq!(r.state, State::Complete);
        assert!(!r.los.is_empty() && r.los.iter().all(|(q, b)| *b && q.node != ally));
        // Unknown answers stop the search.
        let r = run(&w, &req);
        assert!(r.los_unknown.is_some() && r.state == State::NotCalculated);
    }
}
