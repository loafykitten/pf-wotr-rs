//! The A* search as A* Pathfinding Project 4.3.10 runs it: `PathHandler` and `PathNode`
//! bookkeeping, the 4-ary `BinaryHeap` with its tie-breaking, `TriangleMeshNode.Open`
//! (connections backwards, endpoint special costs, `UpdateRecursiveG` re-opening),
//! `GridNode.Open` (directions 0..7), and `ABPath`/`XPath` preparation, initialisation,
//! stepping and tracing (`GetNearest` endpoints with `PathNNConstraint`, the grid
//! end-point special case, `CompleteWith`, `ChangeEndNode`).

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
    /// `XPath` with `ABPathEndingCondition` semantics (`ABPathWithClearance` with a
    /// `MaxDistanceEndingCondition` of `float.MaxValue`).
    X,
}

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
}

struct Search<'w> {
    w: &'w World,
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

impl Search<'_> {
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

    fn can_traverse(&self, n: NodeId) -> bool {
        self.w.walkable(n) && (self.enabled_tags >> self.w.tag(n)) & 1 != 0
    }

    /// `Path.CalculateHScore` (Euclidean, scale 1; no embedding, `ModifyHScore` is the
    /// identity here).
    fn h_score(&self, n: NodeId) -> u32 {
        let c = cs::cost_magnitude(cs::int3_sub(self.h_target, self.w.position(n)));
        cs::to_u32(d(c as f32) * 1.0)
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
        if !self.can_traverse(s) {
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
        if !self.can_traverse(e) {
            self.fail("The node closest to the end point could not be traversed");
        } else if self.w.area[s as usize] != self.w.area[e as usize] {
            self.fail("There is no valid path to the target");
        } else if !self.grid_special_case(e) {
            self.h_target = cs::to_int3(self.end_point);
            self.nodes[e as usize].set_flag1(true);
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

    fn target_found(&self, n: u32) -> bool {
        Some(n) == self.end_node
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
            PathKind::X => {
                if self.target_found(s) {
                    self.change_end_node(s);
                    self.trace(s);
                    self.set_complete(State::Complete);
                }
            }
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
                PathKind::X => {
                    if self.target_found(cur) {
                        self.set_complete(State::Complete);
                        break;
                    }
                }
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
/// `BlockUntilCalculated` would.
#[must_use]
pub fn run(w: &World, req: &Request) -> SearchResult {
    let mut constraint = Constraint::path_default();
    constraint.graph_mask = req.graph_mask;
    let mut s = Search {
        w,
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
        if s.state == State::NotCalculated {
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
}
