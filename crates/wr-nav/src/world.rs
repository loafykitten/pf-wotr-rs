//! The decoded graphs as A* holds them at runtime: one flat node space over all graphs
//! (navmesh triangles first, then grid cells), each triangle's connections resolved to
//! node ids, every tile's `BBTree`, the graph transforms, and connected areas.
//!
//! Follows `NavmeshBase.DeserializeExtraInfo`, `BBTree.RebuildFrom`,
//! `GridGraph.CalculateTransform`/`SetUpOffsetsAndCosts` and `GraphTransform` in A*
//! Pathfinding Project 4.3.10.

use super::cs::{self, M4, V3};
use serde::{Deserialize, Serialize};

use crate::{FLAG_TAG_MASK, FLAG_TAG_SHIFT, FLAG_WALKABLE, Int3};

/// A node id in [`World`]: triangles `0..tris.len()`, then grid cells.
pub type NodeId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conn {
    pub node: NodeId,
    pub cost: u32,
    pub shape_edge: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tri {
    pub tile: u32,
    pub index_in_tile: u32,
    /// Vertex indices with the tile index in the bits above 12 (`v0..v2`).
    pub v: [i32; 3],
    pub position: Int3,
    /// Serialized flags (`& 0xFFF80001`): walkable bit and tag.
    pub flags: u32,
    pub conns: Vec<Conn>,
}

/// One `BBTree` box.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BBox {
    /// `xmin, zmin, xmax, zmax` in Int3 units (world space).
    pub rect: [i32; 4],
    /// Leaf: offset into `lookup` (4 slots); internal: -1.
    pub node_offset: i32,
    pub left: i32,
    pub right: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BBTree {
    pub boxes: Vec<BBox>,
    pub lookup: Vec<Option<NodeId>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TileRt {
    pub x: i32,
    pub z: i32,
    pub verts: Vec<Int3>,
    pub verts_graph: Vec<Int3>,
    /// Node ids of `tile.nodes`, in order.
    pub nodes: Vec<NodeId>,
    pub bbtree: BBTree,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recast {
    pub graph_index: u32,
    pub tile_x: i32,
    pub tile_z: i32,
    pub tiles: Vec<TileRt>,
    /// The graph transform is a pure translation here (`onlyTranslational`).
    pub translation: V3,
    pub i3translation: Int3,
    pub tile_world_size_x: f32,
    pub tile_world_size_z: f32,
    pub nearest_search_only_xz: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grid {
    pub graph_index: u32,
    /// First node id of the grid.
    pub first: NodeId,
    pub width: i32,
    pub depth: i32,
    pub node_size: f32,
    pub positions: Vec<Int3>,
    pub flags: Vec<u32>,
    pub grid_flags: Vec<u16>,
    pub matrix: M4,
    pub inverse: M4,
    pub only_translational: bool,
    pub translation: V3,
    pub neighbour_offsets: [i32; 8],
    pub neighbour_costs: [u32; 8],
    pub neighbour_x: [i32; 8],
    pub neighbour_z: [i32; 8],
    pub eight_neighbours: bool,
    /// `GridGraph.erodeIterations`: the serialized value; turn-based combat rewrites it
    /// (`CombatController.UpdateNavigationGridTags`).
    pub erode_iterations: i32,
    pub erosion_first_tag: i32,
    pub erosion_use_tags: bool,
    /// `GridGraph.erosionOnlyAroundCellsWithTag` (Owlcat's; 0: around every cell).
    pub erosion_only_around_cells_with_tag: u32,
    pub cut_corners: bool,
    /// `GridGraph.maxClimb` (`IsValidConnection`; `collision.use2D` is false).
    pub max_climb: f32,
    /// `GraphCollision.up`: `(Transform(up) - Transform(zero)).normalized` (`GetPortal`).
    pub up: V3,
    /// `GraphTransform.up`: `matrix.MultiplyVector(Vector3.up).normalized`
    /// (`WorldUpAtGraphPosition`, `IsValidConnection`).
    pub transform_up: V3,
    /// `GraphNode.DynamicBlocked` per cell (runtime only; empty: all zero).
    #[serde(skip)]
    pub dynamic_blocked: Vec<i8>,
    /// `GraphNode.AllyBlocked` per cell (runtime only; empty: all zero).
    #[serde(skip)]
    pub ally_blocked: Vec<i8>,
}

impl Grid {
    /// `GraphTransform.Transform(Vector3)`.
    #[must_use]
    pub fn transform(&self, p: V3) -> V3 {
        if self.only_translational {
            cs::add(p, self.translation)
        } else {
            cs::mul_point3x4(&self.matrix, p)
        }
    }

    /// `GraphTransform.InverseTransform(Vector3)`.
    #[must_use]
    pub fn inverse_transform(&self, p: V3) -> V3 {
        if self.only_translational {
            cs::sub(p, self.translation)
        } else {
            cs::mul_point3x4(&self.inverse, p)
        }
    }

    #[must_use]
    pub fn has_connection(&self, cell: usize, dir: usize) -> bool {
        (self.grid_flags[cell] >> dir) & 1 != 0
    }

    #[must_use]
    pub fn dynamic_blocked(&self, cell: usize) -> i8 {
        self.dynamic_blocked.get(cell).copied().unwrap_or(0)
    }

    #[must_use]
    pub fn ally_blocked(&self, cell: usize) -> i8 {
        self.ally_blocked.get(cell).copied().unwrap_or(0)
    }

    /// `GraphNode.StaticWalkable`.
    #[must_use]
    pub fn static_walkable(&self, cell: usize) -> bool {
        self.flags[cell] & FLAG_WALKABLE != 0
    }

    /// `GraphNode.Walkable`: the walkable flag and no dynamic blocker.
    #[must_use]
    pub fn walkable(&self, cell: usize) -> bool {
        self.static_walkable(cell) && self.dynamic_blocked(cell) == 0
    }
}

/// All graphs of a cache, as searched at runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct World {
    pub tris: Vec<Tri>,
    pub recast: Option<Recast>,
    pub grid: Option<Grid>,
    /// Connected component per node (0 for unwalkable nodes), as `GraphNode.Area`.
    pub area: Vec<u32>,
    /// `AstarPath.maxNearestNodeDistance`.
    pub max_nearest_node_distance: f32,
    /// `AstarPath.fullGetNearestSearch`.
    pub full_get_nearest_search: bool,
}

/// Node graph kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Tri(usize),
    Cell(usize),
}

impl World {
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.tris.len() + self.grid.as_ref().map_or(0, |g| g.positions.len())
    }

    #[must_use]
    pub fn kind(&self, n: NodeId) -> Kind {
        let i = n as usize;
        if i < self.tris.len() { Kind::Tri(i) } else { Kind::Cell(i - self.tris.len()) }
    }

    #[must_use]
    pub fn flags(&self, n: NodeId) -> u32 {
        match self.kind(n) {
            Kind::Tri(i) => self.tris[i].flags,
            Kind::Cell(c) => self.grid.as_ref().map_or(0, |g| g.flags[c]),
        }
    }

    #[must_use]
    pub fn position(&self, n: NodeId) -> Int3 {
        match self.kind(n) {
            Kind::Tri(i) => self.tris[i].position,
            Kind::Cell(c) => self.grid.as_ref().map_or([0; 3], |g| g.positions[c]),
        }
    }

    /// `GraphNode.Walkable`: the walkable flag, and for grid cells no dynamic blocker
    /// (`DynamicBlocked == 0`, Owlcat's addition; navmesh nodes are never blocked so).
    #[must_use]
    pub fn walkable(&self, n: NodeId) -> bool {
        match self.kind(n) {
            Kind::Tri(i) => self.tris[i].flags & FLAG_WALKABLE != 0,
            Kind::Cell(c) => self.grid.as_ref().is_some_and(|g| g.walkable(c)),
        }
    }

    /// `GraphNode.StaticWalkable`.
    #[must_use]
    pub fn static_walkable(&self, n: NodeId) -> bool {
        self.flags(n) & FLAG_WALKABLE != 0
    }

    /// `GraphNode.AllyBlocked`.
    #[must_use]
    pub fn ally_blocked(&self, n: NodeId) -> i8 {
        match self.kind(n) {
            Kind::Tri(_) => 0,
            Kind::Cell(c) => self.grid.as_ref().map_or(0, |g| g.ally_blocked(c)),
        }
    }

    #[must_use]
    pub fn tag(&self, n: NodeId) -> u32 {
        (self.flags(n) & FLAG_TAG_MASK) >> FLAG_TAG_SHIFT
    }

    #[must_use]
    pub fn graph_index(&self, n: NodeId) -> u32 {
        match self.kind(n) {
            Kind::Tri(_) => self.recast.as_ref().map_or(0, |r| r.graph_index),
            Kind::Cell(_) => self.grid.as_ref().map_or(0, |g| g.graph_index),
        }
    }

    /// The oracle's node key: navmesh `[graph, tile, index in tile]`, grid
    /// `[graph, 0, NodeInGridIndex]`.
    #[must_use]
    pub fn key(&self, n: NodeId) -> [i64; 3] {
        match self.kind(n) {
            Kind::Tri(i) => [
                i64::from(self.graph_index(n)),
                i64::from(self.tris[i].tile),
                i64::from(self.tris[i].index_in_tile),
            ],
            Kind::Cell(c) => [i64::from(self.graph_index(n)), 0, c as i64],
        }
    }

    /// The node for an oracle key.
    #[must_use]
    pub fn node_of_key(&self, k: [i64; 3]) -> Option<NodeId> {
        if let Some(r) = &self.recast
            && i64::from(r.graph_index) == k[0]
        {
            let t = r.tiles.get(usize::try_from(k[1]).ok()?)?;
            return t.nodes.get(usize::try_from(k[2]).ok()?).copied();
        }
        if let Some(g) = &self.grid
            && i64::from(g.graph_index) == k[0]
        {
            let c = usize::try_from(k[2]).ok()?;
            return (c < g.positions.len()).then_some(g.first + c as u32);
        }
        None
    }

    /// Connections in `GetConnections` order.
    pub fn connections(&self, n: NodeId, out: &mut Vec<NodeId>) {
        match self.kind(n) {
            Kind::Tri(i) => out.extend(self.tris[i].conns.iter().map(|c| c.node)),
            Kind::Cell(c) => {
                let g = self.grid.as_ref().expect("cell without grid");
                for dir in 0..8 {
                    if g.has_connection(c, dir) {
                        let m = c as i64 + i64::from(g.neighbour_offsets[dir]);
                        if let Ok(m) = usize::try_from(m)
                            && m < g.positions.len()
                        {
                            out.push(g.first + m as u32);
                        }
                    }
                }
            }
        }
    }

    /// Connected components over walkable nodes (both connection directions), numbered
    /// from 1 in node order. `HierarchicalGraph` numbers differently, but A* only
    /// compares areas for equality.
    /// The connected component of every node (`GraphNode.Area`).
    #[must_use]
    pub fn compute_areas(&self) -> Vec<u32> {
        let n = self.node_count();
        let mut adj: Vec<Vec<NodeId>> = vec![Vec::new(); n];
        let mut buf = Vec::new();
        for a in 0..n {
            let a = a as NodeId;
            buf.clear();
            self.connections(a, &mut buf);
            for &b in &buf {
                adj[a as usize].push(b);
                adj[b as usize].push(a);
            }
        }
        let mut area = vec![0u32; n];
        let mut next = 1;
        let mut stack = Vec::new();
        for s in 0..n {
            if area[s] != 0 || !self.walkable(s as NodeId) {
                continue;
            }
            area[s] = next;
            stack.push(s);
            while let Some(x) = stack.pop() {
                for &y in &adj[x] {
                    let y = y as usize;
                    if area[y] == 0 && self.walkable(y as NodeId) {
                        area[y] = next;
                        stack.push(y);
                    }
                }
            }
            next += 1;
        }
        area
    }

    /// World-space vertex of a triangle vertex index.
    #[must_use]
    pub fn vertex(&self, index: i32) -> Int3 {
        let r = self.recast.as_ref().expect("vertex without navmesh");
        r.tiles[((index >> 12) & 0x7FFFF) as usize].verts[(index & 0xFFF) as usize]
    }

    #[must_use]
    pub fn vertex_graph(&self, index: i32) -> Int3 {
        let r = self.recast.as_ref().expect("vertex without navmesh");
        r.tiles[((index >> 12) & 0x7FFFF) as usize].verts_graph[(index & 0xFFF) as usize]
    }

    #[must_use]
    pub fn tri_vertices(&self, i: usize) -> [Int3; 3] {
        self.tris[i].v.map(|v| self.vertex(v))
    }

    #[must_use]
    pub fn tri_vertices_graph(&self, i: usize) -> [Int3; 3] {
        self.tris[i].v.map(|v| self.vertex_graph(v))
    }
}

impl BBTree {
    /// `BBTree.RebuildFrom(tile.nodes)`.
    /// `BBTree.RebuildFrom` over a tile's nodes.
    #[must_use]
    pub fn build(rc: &Recast, tris: &[Tri], nodes: &[NodeId]) -> Self {
        let mut t = BBTree::default();
        if nodes.is_empty() {
            return t;
        }
        let vertex =
            |idx: i32| rc.tiles[((idx >> 12) & 0x7FFFF) as usize].verts[(idx & 0xFFF) as usize];
        let bounds: Vec<[i32; 4]> = nodes
            .iter()
            .map(|&n| {
                let v = tris[n as usize].v.map(vertex);
                let mut r = [v[0][0], v[0][2], v[0][0], v[0][2]];
                for p in &v[1..] {
                    r = [r[0].min(p[0]), r[1].min(p[2]), r[2].max(p[0]), r[3].max(p[2])];
                }
                r
            })
            .collect();
        let mut perm: Vec<usize> = (0..nodes.len()).collect();
        let mut leaf_nodes = 0;
        t.rebuild(tris, nodes, &mut perm, &bounds, 0, nodes.len(), false, &mut leaf_nodes);
        t
    }

    #[expect(clippy::too_many_arguments, reason = "mirrors RebuildFromInternal")]
    fn rebuild(
        &mut self,
        tris: &[Tri],
        nodes: &[NodeId],
        perm: &mut [usize],
        bounds: &[[i32; 4]],
        from: usize,
        to: usize,
        odd: bool,
        leaf_nodes: &mut i32,
    ) -> i32 {
        let mut rect = bounds[perm[from]];
        for &p in &perm[from + 1..to] {
            let b = bounds[p];
            rect = [rect[0].min(b[0]), rect[1].min(b[1]), rect[2].max(b[2]), rect[3].max(b[3])];
        }
        let bx = self.boxes.len() as i32;
        self.boxes.push(BBox { rect, node_offset: -1, left: -1, right: -1 });
        if to - from <= 4 {
            let off = *leaf_nodes * 4;
            self.boxes[bx as usize].node_offset = off;
            *leaf_nodes += 1;
            let need = (off + 4) as usize;
            if self.lookup.len() < need {
                self.lookup.resize(need, None);
            }
            for i in 0..4 {
                self.lookup[off as usize + i] = (i < to - from).then(|| nodes[perm[from + i]]);
            }
            return bx;
        }
        let pos = |p: usize, axis: usize| tris[nodes[p] as usize].position[axis];
        let split = |perm: &mut [usize], axis: usize, divider: i32| -> usize {
            let mut num = to;
            let mut i = from;
            while i < num {
                if pos(perm[i], axis) > divider {
                    num -= 1;
                    perm.swap(num, i);
                } else {
                    i += 1;
                }
            }
            num
        };
        // C# integer division truncates toward zero.
        let mid = |a: i32, b: i32| (a + b) / 2;
        let mut s = if odd {
            split(perm, 0, mid(rect[0], rect[2]))
        } else {
            split(perm, 2, mid(rect[1], rect[3]))
        };
        if s == from || s == to {
            s = if odd {
                split(perm, 2, mid(rect[1], rect[3]))
            } else {
                split(perm, 0, mid(rect[0], rect[2]))
            };
            if s == from || s == to {
                s = (from + to) / 2;
            }
        }
        let l = self.rebuild(tris, nodes, perm, bounds, from, s, !odd, leaf_nodes);
        let r = self.rebuild(tris, nodes, perm, bounds, s, to, !odd, leaf_nodes);
        self.boxes[bx as usize].left = l;
        self.boxes[bx as usize].right = r;
        bx
    }
}
