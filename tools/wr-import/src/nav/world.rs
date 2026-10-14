//! The decoded graphs as A* holds them at runtime: one flat node space over all graphs
//! (navmesh triangles first, then grid cells), each triangle's connections resolved to
//! node ids, every tile's `BBTree`, the graph transforms, and connected areas.
//!
//! Follows `NavmeshBase.DeserializeExtraInfo`, `BBTree.RebuildFrom`,
//! `GridGraph.CalculateTransform`/`SetUpOffsetsAndCosts` and `GraphTransform` in A*
//! Pathfinding Project 4.3.10.

use std::collections::HashMap;

use anyhow::{Context, Result, bail, ensure};

use super::cs::{self, M4, V3};
use super::graph::{FLAG_TAG_MASK, FLAG_TAG_SHIFT, FLAG_WALKABLE, Graph, GraphCache, Int3};

/// A node id in [`World`]: triangles `0..tris.len()`, then grid cells.
pub type NodeId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Conn {
    pub node: NodeId,
    pub cost: u32,
    pub shape_edge: u8,
}

#[derive(Debug, Clone)]
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
#[derive(Debug, Clone, Copy)]
pub struct BBox {
    /// `xmin, zmin, xmax, zmax` in Int3 units (world space).
    pub rect: [i32; 4],
    /// Leaf: offset into `lookup` (4 slots); internal: -1.
    pub node_offset: i32,
    pub left: i32,
    pub right: i32,
}

#[derive(Debug, Clone, Default)]
pub struct BBTree {
    pub boxes: Vec<BBox>,
    pub lookup: Vec<Option<NodeId>>,
}

#[derive(Debug, Clone)]
pub struct TileRt {
    pub x: i32,
    pub z: i32,
    pub verts: Vec<Int3>,
    pub verts_graph: Vec<Int3>,
    /// Node ids of `tile.nodes`, in order.
    pub nodes: Vec<NodeId>,
    pub bbtree: BBTree,
}

#[derive(Debug, Clone)]
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

#[derive(Debug, Clone)]
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
    pub erode_iterations: i32,
    pub erosion_first_tag: i32,
    /// `GraphCollision.up`.
    pub up: V3,
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
}

/// All graphs of a cache, as searched at runtime.
#[derive(Debug, Clone)]
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

fn f32_at(v: &serde_json::Value, k: &str) -> Option<f32> {
    v.get(k).and_then(serde_json::Value::as_f64).map(|x| x as f32)
}

fn vec3_at(v: &serde_json::Value, k: &str) -> Option<V3> {
    let o = v.get(k)?;
    Some([f32_at(o, "x")?, f32_at(o, "y")?, f32_at(o, "z")?])
}

impl World {
    /// Builds the runtime view. `astar` holds `AstarPath` settings
    /// (`maxNearestNodeDistance`, `fullGetNearestSearch`) where known.
    pub fn build(cache: &GraphCache, max_nearest: f32, full_get_nearest: bool) -> Result<Self> {
        let mut tris = Vec::new();
        let mut recast = None;
        let mut grid = None;
        for (gi, g) in cache.graphs.iter().enumerate() {
            let gidx = u32::try_from(gi)?;
            match g {
                Graph::Recast(r) => {
                    ensure!(recast.is_none(), "more than one navmesh graph");
                    let s = &r.settings;
                    let rot = vec3_at(s, "rotation").unwrap_or([0.0; 3]);
                    ensure!(rot == [0.0; 3], "rotated navmesh graphs are not supported");
                    let center = vec3_at(s, "forcedBoundsCenter").context("forcedBoundsCenter")?;
                    let size = vec3_at(s, "forcedBoundsSize").context("forcedBoundsSize")?;
                    // TRS(center, identity, one) * TRS(-size * 0.5f, identity, one).
                    let m = cs::mat_mul(
                        &cs::trs(center, [0.0, 0.0, 0.0, 1.0], [1.0; 3]),
                        &cs::trs(
                            cs::mul([-size[0], -size[1], -size[2]], 0.5),
                            [0.0, 0.0, 0.0, 1.0],
                            [1.0; 3],
                        ),
                    );
                    let translation = cs::mul_point3x4(&m, [0.0; 3]);
                    let cell = f32_at(s, "cellSize").context("cellSize")?;
                    let tsx = s.get("tileSizeX").and_then(serde_json::Value::as_i64).unwrap_or(0);
                    let tsz = s.get("tileSizeZ").and_then(serde_json::Value::as_i64).unwrap_or(0);
                    let base = u32::try_from(tris.len())?;
                    let index: HashMap<i32, NodeId> = r
                        .nodes
                        .iter()
                        .enumerate()
                        .map(|(i, n)| (n.node_index, base + i as u32))
                        .collect();
                    let mut tiles = Vec::new();
                    for t in &r.tiles {
                        let ids: Vec<NodeId> = (t.first_node..t.first_node + t.node_count)
                            .map(|i| base + i as u32)
                            .collect();
                        tiles.push(TileRt {
                            x: t.x,
                            z: t.z,
                            verts: t.verts.clone(),
                            verts_graph: t.verts_graph_space.clone(),
                            nodes: ids,
                            bbtree: BBTree::default(),
                        });
                    }
                    for (tile_i, t) in r.tiles.iter().enumerate() {
                        for (k, n) in
                            r.nodes[t.first_node..t.first_node + t.node_count].iter().enumerate()
                        {
                            let conns = n
                                .connections
                                .iter()
                                .map(|c| {
                                    Ok(Conn {
                                        node: *index.get(&c.node).with_context(|| {
                                            format!("connection to unknown node {}", c.node)
                                        })?,
                                        cost: c.cost,
                                        shape_edge: c.shape_edge,
                                    })
                                })
                                .collect::<Result<Vec<_>>>()?;
                            tris.push(Tri {
                                tile: u32::try_from(tile_i)?,
                                index_in_tile: u32::try_from(k)?,
                                v: n.v,
                                position: n.position,
                                flags: n.flags,
                                conns,
                            });
                        }
                    }
                    let mut rc = Recast {
                        graph_index: gidx,
                        tile_x: r.tile_x_count,
                        tile_z: r.tile_z_count,
                        tiles,
                        translation,
                        i3translation: cs::to_int3(translation),
                        tile_world_size_x: cs::r(f64::from(tsx as f32) * f64::from(cell)),
                        tile_world_size_z: cs::r(f64::from(tsz as f32) * f64::from(cell)),
                        nearest_search_only_xz: s
                            .get("nearestSearchOnlyXZ")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                    };
                    for ti in 0..rc.tiles.len() {
                        let tree = BBTree::build(&rc, &tris, &rc.tiles[ti].nodes);
                        rc.tiles[ti].bbtree = tree;
                    }
                    recast = Some(rc);
                }
                Graph::Grid(gg) => {
                    ensure!(grid.is_none(), "more than one grid graph");
                    grid = Some((gidx, gg));
                }
                Graph::Unsupported { type_name } => bail!("unsupported graph {type_name}"),
            }
        }
        let first = u32::try_from(tris.len())?;
        let grid = match grid {
            Some((gidx, gg)) => Some(build_grid(gidx, first, gg)?),
            None => None,
        };
        let mut w = World {
            tris,
            recast,
            grid,
            area: Vec::new(),
            max_nearest_node_distance: max_nearest,
            full_get_nearest_search: full_get_nearest,
        };
        w.area = w.compute_areas();
        Ok(w)
    }

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

    /// `GraphNode.Walkable` (no dynamic blocking in a static snapshot).
    #[must_use]
    pub fn walkable(&self, n: NodeId) -> bool {
        self.flags(n) & FLAG_WALKABLE != 0
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
    fn compute_areas(&self) -> Vec<u32> {
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

fn build_grid(gidx: u32, first: NodeId, gg: &super::graph::GridGraph) -> Result<Grid> {
    let s = &gg.settings;
    let width = i32::try_from(gg.width)?;
    let depth = i32::try_from(gg.depth)?;
    let ns = gg.node_size;
    let center = vec3_at(s, "center").context("grid center")?;
    let rotation = vec3_at(s, "rotation").unwrap_or([0.0; 3]);
    let aspect = f32_at(s, "aspectRatio").unwrap_or(1.0);
    let iso = f32_at(s, "isometricAngle").unwrap_or(0.0);
    // GridGraph.CalculateTransform.
    let mut m = cs::trs([0.0; 3], cs::quat_euler([0.0, 45.0, 0.0]), [1.0; 3]);
    let cos = cs::r(f64::from((std::f32::consts::PI / 180.0 * iso).cos()));
    m = cs::mat_mul(&cs::trs([0.0; 3], [0.0, 0.0, 0.0, 1.0], [cos, 1.0, 1.0]), &m);
    m = cs::mat_mul(&cs::trs([0.0; 3], cs::quat_euler([0.0, -45.0, 0.0]), [1.0; 3]), &m);
    let q = cs::quat_euler(rotation);
    let a = cs::mat_mul(&cs::trs(center, q, [aspect, 1.0, 1.0]), &m);
    let half = cs::mul([-(width as f32 * ns), -0.0, -(depth as f32 * ns)], 0.5);
    let origin = cs::mul_point3x4(&a, half);
    let matrix = cs::mat_mul(&cs::trs(origin, q, [ns * aspect, 1.0, ns]), &m);
    let inverse = cs::mat_inverse(&matrix).context("singular grid transform")?;
    let only_translational = (0..3)
        .all(|c| (0..4).all(|r| matrix[r][c] == if r == c { 1.0 } else { 0.0 }))
        && matrix[3][3] == 1.0;
    let translation = cs::mul_point3x4(&matrix, [0.0; 3]);
    let uniform = s.get("uniformEdgeCosts").and_then(serde_json::Value::as_bool).unwrap_or(false);
    let straight = round_to_int_u32(ns * 1000.0);
    let diag = if uniform {
        straight
    } else {
        round_to_int_u32(cs::r(f64::from(ns) * f64::from(2f32.sqrt())) * 1000.0)
    };
    let neighbours = s.get("neighbours").and_then(serde_json::Value::as_i64).unwrap_or(1);
    let up = {
        let a = if only_translational {
            cs::add([0.0, 1.0, 0.0], translation)
        } else {
            cs::mul_point3x4(&matrix, [0.0, 1.0, 0.0])
        };
        let b = if only_translational { translation } else { cs::mul_point3x4(&matrix, [0.0; 3]) };
        cs::normalize(cs::sub(a, b))
    };
    Ok(Grid {
        graph_index: gidx,
        first,
        width,
        depth,
        node_size: ns,
        positions: gg.nodes.iter().map(|n| n.position).collect(),
        flags: gg.nodes.iter().map(|n| n.flags).collect(),
        grid_flags: gg.nodes.iter().map(|n| n.grid_flags).collect(),
        matrix,
        inverse,
        only_translational,
        translation,
        neighbour_offsets: [-width, 1, width, -1, -width + 1, width + 1, width - 1, -width - 1],
        neighbour_costs: [straight, straight, straight, straight, diag, diag, diag, diag],
        neighbour_x: [0, 1, 0, -1, 1, 1, -1, -1],
        neighbour_z: [-1, 0, 1, 0, -1, 1, 1, -1],
        eight_neighbours: neighbours == 1,
        erode_iterations: s.get("erodeIterations").and_then(serde_json::Value::as_i64).unwrap_or(0)
            as i32,
        erosion_first_tag: s.get("erosionFirstTag").and_then(serde_json::Value::as_i64).unwrap_or(1)
            as i32,
        up,
    })
}

/// `(uint)Mathf.RoundToInt(x)` where `x` is the float argument (product evaluated in
/// double, as Mono does, then rounded to float when passed).
fn round_to_int_u32(x: f32) -> u32 {
    let r = f64::from(x).round_ties_even();
    if r <= 0.0 { 0 } else { r as u32 }
}

impl BBTree {
    /// `BBTree.RebuildFrom(tile.nodes)`.
    fn build(rc: &Recast, tris: &[Tri], nodes: &[NodeId]) -> Self {
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
