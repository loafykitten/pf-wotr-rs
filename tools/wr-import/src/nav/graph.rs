//! Decoding the A* Pathfinding Project (4.3.10) graph cache that `.nav` bundles hold
//! as a `TextAsset` (`NavMeshManager` passes its bytes to
//! `AstarPath.active.data.DeserializeGraphs`).
//!
//! The cache is a zip: `meta.json` (version, graph GUIDs and types), `graph<N>.json`
//! (settings), `graph<N>_extra.binary` (nodes), `graph_references.binary` (node index
//! per node, in `GetNodes` order), `graph<N>_references.binary` (connections) and
//! `node_link2.binary`. Layouts follow `AstarSerializer`, `NavmeshBase`, `GridGraph`,
//! `GraphNode`, `MeshNode`, `TriangleMeshNode` and `GridNode`.

use std::collections::HashMap;
use std::io::{Cursor, Read};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;

use crate::unity::reader::{Endian, Reader};

/// Integer coordinates in millimetres (`Int3`, precision 1000).
pub type Int3 = [i32; 3];

pub const INT3_PRECISION: f32 = 1000.0;

/// `GraphNode.Flags` bits kept by serialization (`& 0xFFF80001`).
pub const FLAG_WALKABLE: u32 = 1;
pub const FLAG_TAG_SHIFT: u32 = 19;
pub const FLAG_TAG_MASK: u32 = 0x00F8_0000;
pub const FLAG_GRAPH_SHIFT: u32 = 24;

/// One connection from a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Connection {
    /// Global node index (`GraphNode.NodeIndex` as serialized).
    pub node: i32,
    /// Traversal cost in Int3 units (millimetres).
    pub cost: u32,
    /// Edge of this triangle shared with the other node, or 255 if none.
    pub shape_edge: u8,
}

/// A navmesh triangle (`TriangleMeshNode`).
#[derive(Debug, Clone, Serialize)]
pub struct TriNode {
    pub node_index: i32,
    pub penalty: u32,
    pub flags: u32,
    pub tile: u32,
    /// Vertex indices into the tile's `verts` (low 12 bits) with the tile index above.
    pub v: [i32; 3],
    /// `UpdatePositionFromVertices`: rounded average of the vertices.
    pub position: Int3,
    pub connections: Vec<Connection>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Tile {
    pub x: i32,
    pub z: i32,
    pub w: i32,
    pub d: i32,
    pub tris: Vec<i32>,
    /// World-space vertices.
    pub verts: Vec<Int3>,
    pub verts_graph_space: Vec<Int3>,
    /// Range of this tile's nodes in [`RecastGraph::nodes`].
    pub first_node: usize,
    pub node_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecastGraph {
    pub settings: serde_json::Value,
    pub tile_x_count: i32,
    pub tile_z_count: i32,
    pub tiles: Vec<Tile>,
    pub nodes: Vec<TriNode>,
}

impl RecastGraph {
    /// World-space vertex for a node vertex index.
    #[must_use]
    pub fn vertex(&self, index: i32) -> Int3 {
        let tile = ((index >> 12) & 0x7FFFF) as usize;
        self.tiles[tile].verts[(index & 0xFFF) as usize]
    }

    #[must_use]
    pub fn triangle(&self, node: &TriNode) -> [Int3; 3] {
        [self.vertex(node.v[0]), self.vertex(node.v[1]), self.vertex(node.v[2])]
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct GridNode {
    pub penalty: u32,
    pub flags: u32,
    pub position: Int3,
    pub grid_flags: u16,
}

impl GridNode {
    #[must_use]
    pub fn walkable(&self) -> bool {
        self.flags & FLAG_WALKABLE != 0
    }

    #[must_use]
    pub fn has_connection(&self, dir: usize) -> bool {
        self.grid_flags & (1 << dir) != 0
    }

    #[must_use]
    pub fn tag(&self) -> u32 {
        (self.flags & FLAG_TAG_MASK) >> FLAG_TAG_SHIFT
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct GridGraph {
    pub settings: serde_json::Value,
    pub width: usize,
    pub depth: usize,
    pub node_size: f32,
    pub nodes: Vec<GridNode>,
}

impl GridGraph {
    /// `GridGraph.neighbourOffsets` (in grid index units).
    #[must_use]
    pub fn neighbour_offset(&self, dir: usize) -> isize {
        let w = self.width.cast_signed();
        [-w, 1, w, -1, -w + 1, w + 1, w - 1, -w - 1][dir]
    }

    /// `GridGraph.neighbourCosts`: straight and diagonal step costs (Int3 units).
    #[must_use]
    pub fn neighbour_cost(&self, dir: usize) -> u32 {
        let uniform = self
            .settings
            .get("uniformEdgeCosts")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let straight = round_to_int(self.node_size * 1000.0);
        let diag =
            if uniform { straight } else { round_to_int(self.node_size * 2f32.sqrt() * 1000.0) };
        if dir < 4 { straight } else { diag }
    }
}

/// `Mathf.RoundToInt` (banker's rounding via `Math.Round`), clamped to `u32`.
fn round_to_int(x: f32) -> u32 {
    let r = f64::from(x).round_ties_even();
    if r <= 0.0 { 0 } else { r as u32 }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum Graph {
    Recast(RecastGraph),
    Grid(GridGraph),
    Unsupported { type_name: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphCache {
    pub version: String,
    pub guids: Vec<String>,
    pub type_names: Vec<String>,
    pub graphs: Vec<Graph>,
    /// `graph_references.binary`: max node index, then node index per node.
    pub max_node_index: i32,
    pub node_link2_count: i32,
    pub entries: Vec<(String, u64)>,
}

impl GraphCache {
    /// Global node index → (graph, local node index).
    #[must_use]
    pub fn node_map(&self) -> HashMap<i32, (usize, usize)> {
        let mut m = HashMap::new();
        for (gi, g) in self.graphs.iter().enumerate() {
            if let Graph::Recast(r) = g {
                for (i, n) in r.nodes.iter().enumerate() {
                    m.insert(n.node_index, (gi, i));
                }
            }
        }
        m
    }

    #[must_use]
    pub fn recast(&self) -> Option<&RecastGraph> {
        self.graphs.iter().find_map(|g| if let Graph::Recast(r) = g { Some(r) } else { None })
    }

    #[must_use]
    pub fn grid(&self) -> Option<&GridGraph> {
        self.graphs.iter().find_map(|g| if let Graph::Grid(r) = g { Some(r) } else { None })
    }
}

/// `Int3 * 0.333333f` per component: `(int)Math.Round((float)x * 0.333333f)`. The product
/// is evaluated in double here, matching what the game's Mono does with float
/// expressions (see PLAN, C# arithmetic semantics); this must be confirmed against the
/// live game.
#[must_use]
pub fn third(sum: [i64; 3]) -> Int3 {
    let k = f64::from(0.333_333_f32);
    sum.map(|c| (c as f64 * k).round_ties_even() as i32)
}

fn entry(zip: &mut zip::ZipArchive<Cursor<&[u8]>>, name: &str) -> Result<Option<Vec<u8>>> {
    let Ok(mut f) = zip.by_name(name) else { return Ok(None) };
    let mut v = Vec::new();
    f.read_to_end(&mut v)?;
    Ok(Some(v))
}

/// The version string in `meta.json` is written unquoted (`"version": 4.3.10`), which
/// isn't valid JSON, so meta is parsed by hand.
fn parse_meta(text: &str) -> Result<(String, Vec<String>, Vec<String>)> {
    let version = text
        .split("\"version\":")
        .nth(1)
        .and_then(|r| r.split(',').next())
        .map(|s| s.trim().trim_matches('"').to_owned())
        .context("meta.json without version")?;
    let fixed = text.replacen(
        &format!("\"version\": {version}"),
        &format!("\"version\": \"{version}\""),
        1,
    );
    let v: serde_json::Value = serde_json::from_str(&fixed).context("meta.json")?;
    let strings = |k: &str| -> Vec<String> {
        v.get(k)
            .and_then(serde_json::Value::as_array)
            .map(|a| a.iter().map(|x| x.as_str().unwrap_or_default().to_owned()).collect())
            .unwrap_or_default()
    };
    Ok((version, strings("guids"), strings("typeNames")))
}

/// A* writes graph settings with its own TinyJson serializer; values are valid JSON.
fn parse_settings(bytes: &[u8]) -> Result<serde_json::Value> {
    Ok(serde_json::from_slice(bytes)?)
}

fn read_node_base(r: &mut Reader<'_>) -> Result<(u32, u32)> {
    let penalty = r.u32()?;
    let flags = r.u32()? & 0xFFF8_0001;
    Ok((penalty, flags))
}

fn read_int3(r: &mut Reader<'_>) -> Result<Int3> {
    Ok([r.i32()?, r.i32()?, r.i32()?])
}

fn decode_recast(settings: serde_json::Value, extra: &[u8], major: u32) -> Result<RecastGraph> {
    let mut r = Reader::new(extra, Endian::Little);
    let tile_x_count = r.i32()?;
    if tile_x_count < 0 {
        return Ok(RecastGraph {
            settings,
            tile_x_count,
            tile_z_count: 0,
            tiles: Vec::new(),
            nodes: Vec::new(),
        });
    }
    let tile_z_count = r.i32()?;
    let mut tiles = Vec::new();
    let mut nodes = Vec::new();
    for z in 0..tile_z_count {
        for x in 0..tile_x_count {
            let tx = r.i32()?;
            let tz = r.i32()?;
            ensure!(tx >= 0 && tz >= 0, "invalid tile coordinates");
            if tx != x || tz != z {
                // Part of a multi-tile tile written earlier; A* aliases it. Our arrays keep
                // only real tiles, so record nothing.
                continue;
            }
            let w = r.i32()?;
            let d = r.i32()?;
            let ntris = r.len_i32(4)?;
            ensure!(ntris % 3 == 0, "triangle index count {ntris} not divisible by 3");
            let tris = (0..ntris).map(|_| r.i32()).collect::<Result<Vec<_>>>()?;
            let nverts = r.len_i32(12)?;
            let verts = (0..nverts).map(|_| read_int3(&mut r)).collect::<Result<Vec<_>>>()?;
            let verts_graph_space = if major >= 4 {
                let n = r.len_i32(12)?;
                ensure!(n == nverts, "vertsInGraphSpace length differs");
                (0..n).map(|_| read_int3(&mut r)).collect::<Result<Vec<_>>>()?
            } else {
                Vec::new()
            };
            let nnodes = r.len_i32(20)?;
            ensure!(nnodes * 3 == tris.len(), "node count does not match triangles");
            let tile_index = u32::try_from(x + z * tile_x_count)?;
            let first_node = nodes.len();
            let tile_bits = (tile_index as i32) << 12;
            for n in 0..nnodes {
                let (penalty, flags) = read_node_base(&mut r)?;
                // Serialized v0..v2 are overwritten from `tris` by DeserializeExtraInfo.
                let _ = (r.i32()?, r.i32()?, r.i32()?);
                let v = [
                    tris[n * 3] | tile_bits,
                    tris[n * 3 + 1] | tile_bits,
                    tris[n * 3 + 2] | tile_bits,
                ];
                let p = [
                    verts[(v[0] & 0xFFF) as usize],
                    verts[(v[1] & 0xFFF) as usize],
                    verts[(v[2] & 0xFFF) as usize],
                ];
                let sum =
                    [0, 1, 2].map(|c| i64::from(p[0][c]) + i64::from(p[1][c]) + i64::from(p[2][c]));
                nodes.push(TriNode {
                    node_index: -1,
                    penalty,
                    flags,
                    tile: tile_index,
                    v,
                    position: third(sum),
                    connections: Vec::new(),
                });
            }
            tiles.push(Tile {
                x,
                z,
                w,
                d,
                tris,
                verts,
                verts_graph_space,
                first_node,
                node_count: nnodes,
            });
        }
    }
    ensure!(r.remaining() == 0, "{} trailing bytes in recast extra data", r.remaining());
    // `tiles` must be indexable by tile index for vertex lookups: only valid when every
    // tile is 1×1, which `vertex()` relies on.
    ensure!(
        tiles.len() == (tile_x_count * tile_z_count) as usize,
        "multi-tile tiles are not supported yet"
    );
    Ok(RecastGraph { settings, tile_x_count, tile_z_count, tiles, nodes })
}

/// `Mathf.Approximately`.
fn approximately(a: f32, b: f32) -> bool {
    (b - a).abs() < (1e-6 * a.abs().max(b.abs())).max(f32::MIN_POSITIVE * 8.0)
}

/// `GridGraph.CalculateDimensions`, literally: absolute sizes, node size raised so no
/// side exceeds 1024 nodes (the original overwrites the x-based value with the y-based
/// one), sizes clamped to one node, then floor, or ceil when the quotient is
/// approximately integral. Returns width, depth and the effective node size.
#[must_use]
pub fn calculate_dimensions(sx: f32, sy: f32, node_size: f32) -> (usize, usize, f32) {
    let (x, y) = (sx.abs(), sy.abs());
    let ns = node_size.max(y / 1024.0);
    let x = if x < ns { ns } else { x };
    let y = if y < ns { ns } else { y };
    let dim = |v: f32| {
        let q = v / ns;
        let d = if approximately(q, q.ceil()) { q.ceil() } else { q.floor() };
        d.max(0.0) as usize
    };
    (dim(x), dim(y), ns)
}

fn decode_grid(settings: serde_json::Value, extra: &[u8]) -> Result<GridGraph> {
    let mut r = Reader::new(extra, Endian::Little);
    let n = r.i32()?;
    let node_size =
        settings.get("nodeSize").and_then(serde_json::Value::as_f64).unwrap_or(1.0) as f32;
    let size = settings.get("unclampedSize");
    let sx =
        size.and_then(|s| s.get("x")).and_then(serde_json::Value::as_f64).unwrap_or(0.0) as f32;
    let sz =
        size.and_then(|s| s.get("y")).and_then(serde_json::Value::as_f64).unwrap_or(0.0) as f32;
    let (width, depth, node_size) = calculate_dimensions(sx, sz, node_size);
    if n < 0 {
        return Ok(GridGraph { settings, width, depth, node_size, nodes: Vec::new() });
    }
    let n = usize::try_from(n)?;
    ensure!(n * 22 <= r.remaining(), "grid node count exceeds data");
    let mut nodes = Vec::with_capacity(n);
    for _ in 0..n {
        let (penalty, flags) = read_node_base(&mut r)?;
        let position = read_int3(&mut r)?;
        let grid_flags = r.u16()?;
        nodes.push(GridNode { penalty, flags, position, grid_flags });
    }
    ensure!(r.remaining() == 0, "{} trailing bytes in grid extra data", r.remaining());
    ensure!(width * depth == nodes.len(), "grid {width}x{depth} != {} nodes", nodes.len());
    Ok(GridGraph { settings, width, depth, node_size, nodes })
}

/// Decodes a graph cache (the `.nav` TextAsset's bytes).
pub fn decode(bytes: &[u8]) -> Result<GraphCache> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("graph cache is not a zip")?;
    let entries = (0..zip.len())
        .map(|i| zip.by_index(i).and_then(|f| Ok((f.name()?.into_owned(), f.size()))))
        .collect::<Result<Vec<_>, _>>()?;
    let meta = entry(&mut zip, "meta.json")?.context("no meta.json (binary meta not supported)")?;
    let (version, guids, type_names) = parse_meta(&String::from_utf8_lossy(&meta))?;
    let major: u32 = version.split('.').next().and_then(|m| m.parse().ok()).unwrap_or(0);
    let mut graphs = Vec::new();
    for (i, t) in type_names.iter().enumerate() {
        let settings = match entry(&mut zip, &format!("graph{i}.json"))? {
            Some(b) => parse_settings(&b)?,
            None => bail!("graph{i}.json missing (binary settings not supported)"),
        };
        let got =
            settings.pointer("/guid/value").and_then(serde_json::Value::as_str).unwrap_or_default();
        ensure!(got == guids[i], "graph {i} guid {got} != meta {}", guids[i]);
        let extra = entry(&mut zip, &format!("graph{i}_extra.binary"))?.unwrap_or_default();
        graphs.push(match t.as_str() {
            "Pathfinding.RecastGraph" | "Pathfinding.NavMeshGraph" => Graph::Recast(
                decode_recast(settings, &extra, major)
                    .with_context(|| format!("graph {i} ({t})"))?,
            ),
            "Pathfinding.GridGraph" => Graph::Grid(
                decode_grid(settings, &extra).with_context(|| format!("graph {i} ({t})"))?,
            ),
            other => Graph::Unsupported { type_name: other.to_owned() },
        });
    }
    // Node indices, in GetNodes order over all graphs.
    let refs = entry(&mut zip, "graph_references.binary")?.context("no graph_references.binary")?;
    let mut r = Reader::new(&refs, Endian::Little);
    let max_node_index = r.i32()?;
    for g in &mut graphs {
        match g {
            Graph::Recast(rg) => {
                for n in &mut rg.nodes {
                    n.node_index = r.i32()?;
                }
            }
            Graph::Grid(gg) => {
                for _ in &gg.nodes {
                    r.i32()?;
                }
            }
            Graph::Unsupported { type_name } => {
                bail!("cannot map node indices past unsupported graph {type_name}")
            }
        }
    }
    ensure!(r.remaining() == 0, "graph_references.binary has {} extra bytes", r.remaining());
    // Connections (MeshNode.DeserializeReferences); grid nodes serialize none.
    let v41 = version_at_least(&version, 4, 1);
    for (i, g) in graphs.iter_mut().enumerate() {
        let Graph::Recast(rg) = g else { continue };
        let data = entry(&mut zip, &format!("graph{i}_references.binary"))?.unwrap_or_default();
        let mut r = Reader::new(&data, Endian::Little);
        for n in &mut rg.nodes {
            let count = r.i32()?;
            if count == -1 {
                continue;
            }
            let count = usize::try_from(count).context("negative connection count")?;
            ensure!(count * 8 <= r.remaining(), "connection count exceeds data");
            for _ in 0..count {
                let node = r.i32()?;
                let cost = r.u32()?;
                let shape_edge = if v41 { r.u8()? } else { 255 };
                n.connections.push(Connection { node, cost, shape_edge });
            }
        }
        ensure!(r.remaining() == 0, "graph{i}_references.binary has {} extra bytes", r.remaining());
    }
    let links = entry(&mut zip, "node_link2.binary")?.unwrap_or_default();
    let node_link2_count =
        if links.len() >= 4 { Reader::new(&links, Endian::Little).i32()? } else { 0 };
    Ok(GraphCache { version, guids, type_names, graphs, max_node_index, node_link2_count, entries })
}

fn version_at_least(v: &str, major: u32, minor: u32) -> bool {
    let mut it = v.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let (a, b) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    (a, b) >= (major, minor)
}

/// Bounds and counts for the report.
#[derive(Debug, Clone, Serialize)]
pub struct GraphSummary {
    pub index: usize,
    pub type_name: String,
    pub nodes: usize,
    pub walkable_nodes: usize,
    pub connections: usize,
    pub tiles: Option<usize>,
    pub vertices: Option<usize>,
    pub grid_size: Option<(usize, usize)>,
    /// World-space AABB in metres: min, max.
    pub bounds: Option<([f32; 3], [f32; 3])>,
    pub tags: Vec<(u32, usize)>,
    pub nonzero_penalties: usize,
    /// Sizes of connected components (walkable nodes), largest first.
    pub components: Vec<usize>,
}

/// Connected-component sizes over walkable nodes, largest first.
#[must_use]
pub fn component_sizes(
    n: usize,
    walkable: impl Fn(usize) -> bool,
    neighbours: impl Fn(usize, &mut Vec<usize>),
) -> Vec<usize> {
    let mut seen = vec![false; n];
    let mut sizes = Vec::new();
    let mut stack = Vec::new();
    let mut buf = Vec::new();
    for s in 0..n {
        if seen[s] || !walkable(s) {
            continue;
        }
        seen[s] = true;
        stack.push(s);
        let mut size = 0;
        while let Some(x) = stack.pop() {
            size += 1;
            buf.clear();
            neighbours(x, &mut buf);
            for &y in &buf {
                if y < n && !seen[y] && walkable(y) {
                    seen[y] = true;
                    stack.push(y);
                }
            }
        }
        sizes.push(size);
    }
    sizes.sort_unstable_by(|a, b| b.cmp(a));
    sizes
}

#[must_use]
pub fn summarize(cache: &GraphCache) -> Vec<GraphSummary> {
    let to_m = |p: Int3| p.map(|c| c as f32 / INT3_PRECISION);
    let mut out = Vec::new();
    for (i, g) in cache.graphs.iter().enumerate() {
        let mut mn = [f32::INFINITY; 3];
        let mut mx = [f32::NEG_INFINITY; 3];
        let mut grow = |p: [f32; 3]| {
            for c in 0..3 {
                mn[c] = mn[c].min(p[c]);
                mx[c] = mx[c].max(p[c]);
            }
        };
        let mut tags: HashMap<u32, usize> = HashMap::new();
        let s = match g {
            Graph::Recast(r) => {
                for t in &r.tiles {
                    for v in &t.verts {
                        grow(to_m(*v));
                    }
                }
                for n in &r.nodes {
                    *tags.entry((n.flags & FLAG_TAG_MASK) >> FLAG_TAG_SHIFT).or_default() += 1;
                }
                GraphSummary {
                    index: i,
                    type_name: cache.type_names[i].clone(),
                    nodes: r.nodes.len(),
                    walkable_nodes: r.nodes.iter().filter(|n| n.flags & FLAG_WALKABLE != 0).count(),
                    connections: r.nodes.iter().map(|n| n.connections.len()).sum(),
                    tiles: Some(r.tiles.len()),
                    vertices: Some(r.tiles.iter().map(|t| t.verts.len()).sum()),
                    grid_size: None,
                    bounds: None,
                    tags: Vec::new(),
                    nonzero_penalties: r.nodes.iter().filter(|n| n.penalty != 0).count(),
                    components: {
                        let idx: HashMap<i32, usize> =
                            r.nodes.iter().enumerate().map(|(i, n)| (n.node_index, i)).collect();
                        component_sizes(
                            r.nodes.len(),
                            |i| r.nodes[i].flags & FLAG_WALKABLE != 0,
                            |i, out| {
                                out.extend(
                                    r.nodes[i]
                                        .connections
                                        .iter()
                                        .filter_map(|c| idx.get(&c.node).copied()),
                                )
                            },
                        )
                    },
                }
            }
            Graph::Grid(gg) => {
                for n in gg.nodes.iter().filter(|n| n.walkable()) {
                    grow(to_m(n.position));
                    *tags.entry(n.tag()).or_default() += 1;
                }
                GraphSummary {
                    index: i,
                    type_name: cache.type_names[i].clone(),
                    nodes: gg.nodes.len(),
                    walkable_nodes: gg.nodes.iter().filter(|n| n.walkable()).count(),
                    connections: gg
                        .nodes
                        .iter()
                        .map(|n| (n.grid_flags & 0xFF).count_ones() as usize)
                        .sum(),
                    tiles: None,
                    vertices: None,
                    grid_size: Some((gg.width, gg.depth)),
                    bounds: None,
                    tags: Vec::new(),
                    nonzero_penalties: gg.nodes.iter().filter(|n| n.penalty != 0).count(),
                    components: component_sizes(
                        gg.nodes.len(),
                        |i| gg.nodes[i].walkable(),
                        |i, out| {
                            for dir in 0..8 {
                                if gg.nodes[i].has_connection(dir)
                                    && let Ok(m) =
                                        usize::try_from(i.cast_signed() + gg.neighbour_offset(dir))
                                {
                                    out.push(m);
                                }
                            }
                        },
                    ),
                }
            }
            Graph::Unsupported { type_name } => GraphSummary {
                index: i,
                type_name: type_name.clone(),
                nodes: 0,
                walkable_nodes: 0,
                connections: 0,
                tiles: None,
                vertices: None,
                grid_size: None,
                bounds: None,
                tags: Vec::new(),
                nonzero_penalties: 0,
                components: Vec::new(),
            },
        };
        let mut tags: Vec<_> = tags.into_iter().collect();
        tags.sort_unstable();
        let bounds = mn[0].is_finite().then_some((mn, mx));
        out.push(GraphSummary { bounds, tags, ..s });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_with_bare_version_parses() {
        let m = r#"{"version": 4.3.10, "graphs": 2, "guids": ["a", "b"], "typeNames": ["Pathfinding.RecastGraph", "Pathfinding.GridGraph"]}"#;
        let (v, g, t) = parse_meta(m).unwrap();
        assert_eq!(v, "4.3.10");
        assert_eq!(g, ["a", "b"]);
        assert_eq!(t[1], "Pathfinding.GridGraph");
        assert!(version_at_least(&v, 4, 1));
        assert!(!version_at_least("3.9.0", 4, 1));
    }

    #[test]
    fn third_rounds_half_to_even_in_double() {
        assert_eq!(third([3000, 0, -3000]), [1000, 0, -1000]);
        // 1 * 0.333333 rounds to 0; 2 → 1 (0.666666).
        assert_eq!(third([1, 2, 0]), [0, 1, 0]);
    }

    fn grid_settings() -> serde_json::Value {
        serde_json::json!({"nodeSize": 0.5, "unclampedSize": {"x": 1.0, "y": 0.5}, "uniformEdgeCosts": false})
    }

    #[test]
    fn decodes_grid_nodes() {
        let mut b = Vec::new();
        b.extend_from_slice(&2i32.to_le_bytes());
        for i in 0..2i32 {
            b.extend_from_slice(&7u32.to_le_bytes());
            b.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
            for c in [i * 500, 0, 250] {
                b.extend_from_slice(&c.to_le_bytes());
            }
            b.extend_from_slice(&0b10u16.to_le_bytes());
        }
        let g = decode_grid(grid_settings(), &b).unwrap();
        assert_eq!((g.width, g.depth), (2, 1));
        assert_eq!(g.nodes[1].position, [500, 0, 250]);
        // Serialization keeps only the persistent flag bits.
        assert_eq!(g.nodes[0].flags, 0xFFF8_0001);
        assert!(g.nodes[0].has_connection(1));
        assert_eq!(g.neighbour_cost(0), 500);
        assert_eq!(g.neighbour_cost(4), 707);
        assert!(decode_grid(grid_settings(), &b[..b.len() - 1]).is_err());
        // Approximately integral quotients round up, as in CalculateDimensions.
        assert_eq!(calculate_dimensions(139.999_99, 130.0, 0.5), (280, 260, 0.5));
        assert_eq!(calculate_dimensions(-140.0, 130.0, 0.5).0, 280);
        assert_eq!(calculate_dimensions(140.2, 130.0, 0.5).0, 280);
    }
}
