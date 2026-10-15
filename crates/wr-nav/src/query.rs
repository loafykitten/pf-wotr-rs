//! The five query kinds the oracle records, run on our port, with output in the
//! oracle's shape (`NavDump.WritePath`):
//!
//! - `navmesh_raw`: `ABPath` on the navmesh graph, no modifiers;
//! - `navmesh_agent`: the same through a seeker set up as `UnitMovementAgent.Init` in
//!   real time (StartEnd with the original start point, two funnels), plus Owlcat's
//!   `RemoveExcessPoints`;
//! - `grid_raw`: `ABPath` on the grid graph, no modifiers;
//! - `tb_clearance_0.5` / `tb_clearance_1.0`: `ABPathWithClearance` on the grid with the
//!   turn-based tag mask, StartEnd, one funnel and the graph-raycast modifier.

use serde::Serialize;

use super::astar::{self, PathKind, Request, SearchResult, State};
use super::cs::{self, V3, d, r};
use super::nearest::Constraint;
use super::world::{Kind, NodeId, World};

/// One path, as the oracle writes it.
#[derive(Debug, Clone, Serialize)]
pub struct PathOut {
    pub end_node_g: Option<u32>,
    pub state: &'static str,
    pub error: String,
    pub searched_nodes: i32,
    pub start_point: V3,
    pub end_point: V3,
    pub original_start: V3,
    pub original_end: V3,
    pub start_node: Option<[i64; 3]>,
    pub end_node: Option<[i64; 3]>,
    pub node_count: usize,
    pub nodes: Vec<[i64; 3]>,
    pub step_costs: Vec<i64>,
    pub connection_cost_sum: i64,
    pub points: Vec<V3>,
    pub length: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub points_after_remove_excess_points: Option<Vec<V3>>,
}

/// A nearest-node result as the oracle writes it.
#[derive(Debug, Clone, Serialize)]
pub struct NearestOut {
    pub node: Option<[i64; 3]>,
    pub position: V3,
    pub distance: f32,
}

/// All kinds for one query.
#[derive(Debug, Clone, Serialize)]
pub struct QueryOut {
    pub label: String,
    pub start: V3,
    pub end: V3,
    pub navmesh_raw: Option<PathOut>,
    pub navmesh_agent: Option<PathOut>,
    pub grid_raw: Option<PathOut>,
    #[serde(rename = "tb_clearance_0.5")]
    pub tb_clearance_05: Option<PathOut>,
    #[serde(rename = "tb_clearance_1.0")]
    pub tb_clearance_10: Option<PathOut>,
}

/// `Path.GetTotalLength`.
#[must_use]
pub fn total_length(p: &[V3]) -> f32 {
    let mut n = 0.0f32;
    for w in p.windows(2) {
        n = r(d(n) + d(cs::distance(w[0], w[1])));
    }
    n
}

fn state_name(s: State) -> &'static str {
    match s {
        State::NotCalculated => "NotCalculated",
        State::Error => "Error",
        State::Complete => "Complete",
        State::Partial => "Partial",
    }
}

impl World {
    /// `NavDump.ConnectionCost`.
    #[must_use]
    pub fn connection_cost(&self, from: NodeId, to: NodeId) -> i64 {
        match (self.kind(from), self.kind(to)) {
            (Kind::Tri(i), _) => {
                self.tris[i].conns.iter().find(|c| c.node == to).map_or(-1, |c| i64::from(c.cost))
            }
            (Kind::Cell(a), Kind::Cell(_)) => {
                let g = self.grid.as_ref().expect("grid");
                for dir in 0..8 {
                    if g.has_connection(a, dir)
                        && g.first + (a as i64 + i64::from(g.neighbour_offsets[dir])) as u32 == to
                    {
                        return i64::from(g.neighbour_costs[dir]);
                    }
                }
                -1
            }
            _ => -1,
        }
    }

    fn path_out(&self, s: &SearchResult, points: Vec<V3>, after: Option<Vec<V3>>) -> PathOut {
        let step_costs: Vec<i64> =
            s.path.windows(2).map(|w| self.connection_cost(w[0], w[1])).collect();
        PathOut {
            end_node_g: s.end_node_g,
            state: state_name(s.state),
            error: s.error.clone(),
            searched_nodes: s.searched_nodes,
            start_point: s.start_point,
            end_point: s.end_point,
            original_start: s.original_start,
            original_end: s.original_end,
            start_node: s.start_node.map(|n| self.key(n)),
            end_node: s.end_node.map(|n| self.key(n)),
            node_count: s.path.len(),
            nodes: s.path.iter().map(|&n| self.key(n)).collect(),
            connection_cost_sum: step_costs.iter().map(|&c| c.max(0)).sum(),
            step_costs,
            length: total_length(&points),
            points,
            points_after_remove_excess_points: after,
        }
    }

    /// `StartEndModifier.Apply` with `exactStartPoint = Original` (no raycasts) and
    /// `exactEndPoint = ClosestOnNode`.
    fn start_end(s: &SearchResult, mut v: Vec<V3>) -> Vec<V3> {
        if v.is_empty() {
            return v;
        }
        if v.len() == 1 {
            v.push(v[0]);
        }
        v[0] = s.original_start;
        let last = v.len() - 1;
        v[last] = s.end_point;
        v
    }

    fn mask_of(&self, gi: u32) -> i32 {
        1 << gi
    }

    /// `navmesh_raw`.
    #[must_use]
    pub fn query_navmesh_raw(&self, start: V3, end: V3) -> Option<PathOut> {
        let gi = self.recast.as_ref()?.graph_index;
        let s = astar::run(
            self,
            &Request {
                kind: PathKind::Ab,
                start,
                end,
                graph_mask: self.mask_of(gi),
                enabled_tags: -1,
                calculate_partial: false,
            },
        );
        let pts = s.vector_path.clone();
        Some(self.path_out(&s, pts, None))
    }

    /// `navmesh_agent`.
    #[must_use]
    pub fn query_navmesh_agent(&self, start: V3, end: V3) -> Option<PathOut> {
        self.recast.as_ref()?;
        // Seeker.StartPath(p, callback, 1): graph mask 1 (graph 0).
        let s = astar::run(
            self,
            &Request {
                kind: PathKind::Ab,
                start,
                end,
                graph_mask: 1,
                enabled_tags: -1,
                calculate_partial: false,
            },
        );
        if s.state == State::Error {
            return Some(self.path_out(&s, s.vector_path.clone(), None));
        }
        let mut v = Self::start_end(&s, s.vector_path.clone());
        v = self.funnel_modifier(&s.path, &v);
        v = self.funnel_modifier(&s.path, &v);
        let after = self.remove_excess_points(&v);
        Some(self.path_out(&s, v, Some(after)))
    }

    /// `grid_raw`.
    #[must_use]
    pub fn query_grid_raw(&self, start: V3, end: V3) -> Option<PathOut> {
        let gi = self.grid.as_ref()?.graph_index;
        let s = astar::run(
            self,
            &Request {
                kind: PathKind::Ab,
                start,
                end,
                graph_mask: self.mask_of(gi),
                enabled_tags: -1,
                calculate_partial: false,
            },
        );
        let pts = s.vector_path.clone();
        Some(self.path_out(&s, pts, None))
    }

    /// `NavDump.TurnBasedTags`.
    #[must_use]
    pub fn turn_based_tags(&self, corpulence: f32) -> i32 {
        let Some(g) = &self.grid else { return -1 };
        let mut size = d(r(d(corpulence) / d(g.node_size))).round_ties_even() as i32;
        size = size.max(1);
        if size > g.erode_iterations {
            size = g.erode_iterations;
        }
        let mut mask = i32::MAX;
        for i in 0..size - 1 {
            mask &= !(1 << (i + g.erosion_first_tag));
        }
        mask
    }

    /// `tb_clearance_*`.
    #[must_use]
    pub fn query_turn_based(&self, start: V3, end: V3, corpulence: f32) -> Option<PathOut> {
        self.grid.as_ref()?;
        // seeker.StartPath(p, callback, 2): graph mask 2 (graph 1).
        let s = astar::run(
            self,
            &Request {
                kind: PathKind::X,
                start,
                end,
                graph_mask: 2,
                enabled_tags: self.turn_based_tags(corpulence),
                calculate_partial: false,
            },
        );
        if s.state == State::Error {
            return Some(self.path_out(&s, s.vector_path.clone(), None));
        }
        let mut v = Self::start_end(&s, s.vector_path.clone());
        v = self.funnel_modifier(&s.path, &v);
        v = self.raycast_modifier(&s.path, &v, &s.constraint);
        Some(self.path_out(&s, v, None))
    }

    /// All five kinds for one query.
    #[must_use]
    pub fn query_all(&self, label: &str, start: V3, end: V3) -> QueryOut {
        QueryOut {
            label: label.to_owned(),
            start,
            end,
            navmesh_raw: self.query_navmesh_raw(start, end),
            navmesh_agent: self.query_navmesh_agent(start, end),
            grid_raw: self.query_grid_raw(start, end),
            tb_clearance_05: self.query_turn_based(start, end, 0.5),
            tb_clearance_10: self.query_turn_based(start, end, 1.0),
        }
    }

    /// `NavDump.WriteNearest`: `NNConstraint.Default` restricted to one graph.
    #[must_use]
    pub fn nearest_out(&self, p: V3, graph_index: u32) -> NearestOut {
        let c = Constraint { graph_mask: 1 << graph_index, ..Constraint::default_() };
        let nn = self.get_nearest(p, &c);
        NearestOut {
            node: nn.node.map(|n| self.key(n)),
            position: nn.position,
            distance: cs::distance(p, nn.position),
        }
    }
}
