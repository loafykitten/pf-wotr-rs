//! Builds the runtime [`World`] from A*'s graph cache, as `NavmeshBase.DeserializeExtraInfo`,
//! `GridGraph.CalculateTransform`/`SetUpOffsetsAndCosts` and `GraphTransform` do in A*
//! Pathfinding Project 4.3.10.

use std::collections::HashMap;

use anyhow::{Context, Result, bail, ensure};
use wr_nav::cs::{self, V3};
use wr_nav::world::{BBTree, Conn, Grid, NodeId, Recast, TileRt, Tri, World};

use super::graph::{Graph, GraphCache, GridGraph};

fn f32_at(v: &serde_json::Value, k: &str) -> Option<f32> {
    v.get(k).and_then(serde_json::Value::as_f64).map(|x| x as f32)
}

fn vec3_at(v: &serde_json::Value, k: &str) -> Option<V3> {
    let o = v.get(k)?;
    Some([f32_at(o, "x")?, f32_at(o, "y")?, f32_at(o, "z")?])
}

/// Builds the runtime view. `astar` holds `AstarPath` settings
/// (`maxNearestNodeDistance`, `fullGetNearestSearch`) where known.
pub fn build_world(cache: &GraphCache, max_nearest: f32, full_get_nearest: bool) -> Result<World> {
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

fn build_grid(gidx: u32, first: NodeId, gg: &GridGraph) -> Result<Grid> {
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
