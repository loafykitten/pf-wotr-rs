//! Navigation diffs: the live graphs (`nav-graph.json`) against the decoded cache, and
//! the live path queries (`nav-queries.json`) against our A* port.

use serde_json::{Value as J, json};

use super::json::{self as dj, cmp_f, cmp_fs, cmp_points, eq};
use super::{Ours, Outcome, Section};
use crate::nav::graph::{FLAG_TAG_MASK, FLAG_WALKABLE, Graph};
use crate::nav::query::{NearestOut, PathOut};
use crate::nav::world::World;

/// Standard base64 (the dump's packed grid arrays).
fn base64(s: &str) -> Vec<u8> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        })
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for &c in s.as_bytes() {
        let Some(v) = val(c) else { continue };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

/// Graph-index-adjusted persistent flags: what `DeserializeNode` leaves
/// (`& 0xFFF80001`, graph index from the load context).
fn persistent(flags: u32, gi: u32) -> u32 {
    (flags & 0x00F8_0001) | (gi << 24)
}

fn cmp_settings(sec: &mut Section, graph: &str, ours: &J, theirs: &J) {
    let Some(t) = theirs.as_object() else { return };
    for (k, tv) in t {
        let Some(ov) = ours.get(k) else {
            sec.tally("settings.only_in_live", Outcome::Expected);
            continue;
        };
        let o = match (ov, tv) {
            (J::Number(_), J::Number(_)) => {
                cmp_f(dj::f(ov).unwrap_or(f32::NAN), dj::f(tv).unwrap_or(f32::NAN))
            }
            (J::Bool(a), J::Bool(b)) => eq(a, b),
            (J::String(a), J::String(b)) => eq(a, b),
            (J::Object(o), J::Array(a)) if a.len() == o.len() => {
                // Vectors: {x, y, z} in the cache, [x, y, z] in the dump.
                let ov: Vec<f32> =
                    ["x", "y", "z", "w"].iter().filter_map(|c| o.get(*c).and_then(dj::f)).collect();
                let tv: Vec<f32> = a.iter().filter_map(dj::f).collect();
                cmp_fs(&ov, &tv)
            }
            // Enums: an integer in the cache, a name in the dump.
            (J::Number(_), J::String(_)) => Outcome::Expected,
            _ => Outcome::Mismatch,
        };
        sec.record("settings", o, || json!({"graph": graph, "field": k, "ours": ov, "live": tv}));
    }
}

/// Live graphs vs the decoded cache.
pub fn diff_graph(dump: &J, ours: &Ours, sec: &mut Section) {
    let w = &ours.world;
    let astar = &dump["astar"];
    sec.record("astar.heuristic", eq(&dj::s(astar, "heuristic"), &Some("Euclidean")), || {
        astar.clone()
    });
    sec.record(
        "astar.max_nearest_node_distance",
        cmp_f(
            w.max_nearest_node_distance,
            dj::f(&astar["max_nearest_node_distance"]).unwrap_or(f32::NAN),
        ),
        || astar.clone(),
    );
    let graphs = dj::arr(dump, "graphs");
    sec.record(
        "graph_count",
        eq(&graphs.len(), &ours.cache.graphs.len()),
        || json!({"live": graphs.len()}),
    );
    for (gi, (g, ourg)) in graphs.iter().zip(&ours.cache.graphs).enumerate() {
        let gi = gi as u32;
        let ty = dj::s(g, "type").unwrap_or_default();
        sec.record(
            "type",
            eq(&ty, &ours.cache.type_names[gi as usize].as_str()),
            || json!({"graph": gi, "live": ty}),
        );
        match ourg {
            Graph::Recast(r) => {
                cmp_settings(sec, "recast", &r.settings, &g["settings"]);
                diff_recast(g, w, gi, sec);
            }
            Graph::Grid(gg) => {
                cmp_settings(sec, "grid", &gg.settings, &g["settings"]);
                diff_grid(g, w, gi, sec);
            }
            Graph::Unsupported { .. } => {}
        }
    }
    sec.note(
        "graph_fingerprint (nav-queries.json) hashes runtime-only state: GraphNode.NodeIndex of \
         connection targets and the hierarchical-node bits in raw flags, so it isn't reproduced; \
         every input it covers is compared here directly.",
    );
}

fn diff_recast(g: &J, w: &World, gi: u32, sec: &mut Section) {
    let Some(rc) = &w.recast else { return };
    let node_count = g["node_count"].as_u64().unwrap_or(0) as usize;
    sec.record(
        "recast.node_count",
        eq(&node_count, &w.tris.len()),
        || json!({"live": node_count, "ours": w.tris.len()}),
    );
    sec.record(
        "recast.tile_counts",
        eq(
            &(g["tile_x_count"].as_i64(), g["tile_z_count"].as_i64()),
            &(Some(i64::from(rc.tile_x)), Some(i64::from(rc.tile_z))),
        ),
        || json!({"live": [g["tile_x_count"], g["tile_z_count"]]}),
    );
    if let Some(m) = dj::matrix_cols(&g["transform"]) {
        let ours = [
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
            0.0,
            rc.translation[0],
            rc.translation[1],
            rc.translation[2],
            1.0,
        ];
        sec.record(
            "recast.transform",
            cmp_fs(&ours, &m),
            || json!({"live": g["transform"], "ours_translation": rc.translation}),
        );
    }
    let tiles = dj::arr(g, "tiles");
    sec.record("recast.tiles", eq(&tiles.len(), &rc.tiles.len()), || json!({"live": tiles.len()}));
    for (ti, (t, ot)) in tiles.iter().zip(&rc.tiles).enumerate() {
        let xyz = (t["x"].as_i64(), t["z"].as_i64());
        sec.record(
            "recast.tile.xz",
            eq(&xyz, &(Some(i64::from(ot.x)), Some(i64::from(ot.z)))),
            || json!({"tile": ti, "live": [t["x"], t["z"]]}),
        );
        let verts: Vec<i64> = dj::arr(t, "verts").iter().filter_map(J::as_i64).collect();
        let ov: Vec<i64> = ot.verts.iter().flat_map(|v| v.map(i64::from)).collect();
        sec.record(
            "recast.tile.verts",
            eq(&verts, &ov),
            || json!({"tile": ti, "live_len": verts.len(), "ours_len": ov.len()}),
        );
        let tris: Vec<i64> = dj::arr(t, "tris").iter().filter_map(J::as_i64).collect();
        let nodes = dj::arr(t, "nodes");
        sec.record(
            "recast.tile.node_count",
            eq(&nodes.len(), &ot.nodes.len()),
            || json!({"tile": ti, "live": nodes.len(), "ours": ot.nodes.len()}),
        );
        let mut otris = Vec::new();
        for (ni, (n, &id)) in nodes.iter().zip(&ot.nodes).enumerate() {
            let tri = &w.tris[id as usize];
            otris.extend(tri.v.map(|v| i64::from(v & 0xFFF)));
            let Some(a) = n.as_array() else { continue };
            let vi: Vec<i64> = a.iter().take(3).filter_map(J::as_i64).collect();
            sec.record(
                "recast.node.vertices",
                eq(&vi, &tri.v.map(|v| i64::from(v & 0xFFF)).to_vec()),
                || json!({"tile": ti, "node": ni, "live": vi}),
            );
            let raw = a.get(3).and_then(J::as_u64).unwrap_or(0) as u32;
            let pf = persistent(tri.flags, gi);
            sec.record(
                "recast.node.flags_persistent",
                eq(&(raw & 0xFFF8_0001), &pf),
                || json!({"tile": ti, "node": ni, "live_raw": raw, "ours": pf}),
            );
            let tag = a.get(4).and_then(J::as_u64).unwrap_or(99) as u32;
            sec.record(
                "recast.node.tag",
                eq(&tag, &((tri.flags & FLAG_TAG_MASK) >> 19)),
                || json!({"tile": ti, "node": ni, "live": tag}),
            );
            let walk = a.get(5).and_then(J::as_bool).unwrap_or(false);
            sec.record(
                "recast.node.walkable",
                eq(&walk, &(tri.flags & FLAG_WALKABLE != 0)),
                || json!({"tile": ti, "node": ni, "live": walk}),
            );
            let pen = a.get(6).and_then(J::as_u64).unwrap_or(99);
            sec.record(
                "recast.node.penalty",
                eq(&pen, &0),
                || json!({"tile": ti, "node": ni, "live": pen}),
            );
            let conns: Vec<i64> = a
                .get(7)
                .and_then(J::as_array)
                .map(|c| c.iter().filter_map(J::as_i64).collect())
                .unwrap_or_default();
            let oc: Vec<i64> = tri
                .conns
                .iter()
                .flat_map(|c| {
                    let k = w.key(c.node);
                    [k[1], k[2], i64::from(c.cost), i64::from(c.shape_edge)]
                })
                .collect();
            sec.record(
                "recast.node.connections",
                eq(&conns, &oc),
                || json!({"tile": ti, "node": ni, "live": conns, "ours": oc}),
            );
        }
        sec.record("recast.tile.tris", eq(&tris, &otris), || json!({"tile": ti}));
    }
    // Connection symmetry (both directions present), which area computation assumes.
    let mut asym = 0;
    for (i, t) in w.tris.iter().enumerate() {
        for c in &t.conns {
            if !w.tris[c.node as usize].conns.iter().any(|b| b.node as usize == i) {
                asym += 1;
            }
        }
    }
    sec.note(format!("recast: {asym} one-way connections"));
}

fn diff_grid(g: &J, w: &World, gi: u32, sec: &mut Section) {
    let Some(gr) = &w.grid else { return };
    let n = (gr.width * gr.depth) as usize;
    sec.record(
        "grid.dimensions",
        eq(
            &(g["width"].as_i64(), g["depth"].as_i64()),
            &(Some(i64::from(gr.width)), Some(i64::from(gr.depth))),
        ),
        || json!({"live": [g["width"], g["depth"]]}),
    );
    sec.record(
        "grid.node_size",
        cmp_f(gr.node_size, dj::f(&g["node_size"]).unwrap_or(f32::NAN)),
        || g["node_size"].clone(),
    );
    sec.record(
        "grid.erode_iterations",
        eq(&g["erode_iterations"].as_i64(), &Some(i64::from(gr.erode_iterations))),
        || g["erode_iterations"].clone(),
    );
    if let Some(m) = dj::matrix_cols(&g["transform"]) {
        let mut ours = [0.0; 16];
        for r in 0..4 {
            for c in 0..4 {
                ours[c * 4 + r] = gr.matrix[r][c];
            }
        }
        sec.record(
            "grid.transform",
            cmp_fs(&ours, &m),
            || json!({"live": g["transform"], "ours": gr.matrix}),
        );
    }
    let costs: Vec<u64> = dj::arr(g, "neighbour_costs").iter().filter_map(J::as_u64).collect();
    sec.record(
        "grid.neighbour_costs",
        eq(&costs, &gr.neighbour_costs.iter().map(|&c| u64::from(c)).collect()),
        || json!({"live": costs}),
    );
    let offs: Vec<i64> = dj::arr(g, "neighbour_offsets").iter().filter_map(J::as_i64).collect();
    sec.record(
        "grid.neighbour_offsets",
        eq(&offs, &gr.neighbour_offsets.iter().map(|&c| i64::from(c)).collect()),
        || json!({"live": offs}),
    );
    let pos = base64(dj::s(g, "positions_int3_le_base64").unwrap_or_default());
    let flags = base64(dj::s(g, "flags_u32_le_base64").unwrap_or_default());
    let gflags = base64(dj::s(g, "grid_flags_u16_le_base64").unwrap_or_default());
    let dyn_ = base64(dj::s(g, "dynamic_ally_blocked_i8x2_base64").unwrap_or_default());
    let eff = base64(dj::s(g, "walkable_effective_u8_base64").unwrap_or_default());
    if pos.len() != n * 12 || flags.len() != n * 4 || gflags.len() != n * 2 {
        sec.record(
            "grid.arrays",
            Outcome::Mismatch,
            || json!({"positions": pos.len(), "flags": flags.len(), "grid_flags": gflags.len()}),
        );
        return;
    }
    let i32_at = |b: &[u8], o: usize| i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    for c in 0..n {
        let lp = [i32_at(&pos, c * 12), i32_at(&pos, c * 12 + 4), i32_at(&pos, c * 12 + 8)];
        sec.record(
            "grid.node.position",
            eq(&lp, &gr.positions[c]),
            || json!({"cell": c, "live": lp, "ours": gr.positions[c]}),
        );
        let lf = i32_at(&flags, c * 4) as u32;
        let pf = persistent(gr.flags[c], gi);
        sec.record(
            "grid.node.flags_persistent",
            eq(&(lf & 0xFFF8_0001), &pf),
            || json!({"cell": c, "live_raw": lf, "ours": pf}),
        );
        let lg = u16::from_le_bytes([gflags[c * 2], gflags[c * 2 + 1]]);
        sec.record(
            "grid.node.grid_flags",
            eq(&lg, &gr.grid_flags[c]),
            || json!({"cell": c, "live": lg, "ours": gr.grid_flags[c]}),
        );
        if dyn_.len() == n * 2 {
            let z = dyn_[c * 2] == 0 && dyn_[c * 2 + 1] == 0;
            sec.record(
                "grid.node.no_dynamic_or_ally_blocking",
                eq(&z, &true),
                || json!({"cell": c}),
            );
        }
        if eff.len() == n {
            sec.record(
                "grid.node.walkable_effective",
                eq(&(eff[c] != 0), &(gr.flags[c] & 1 != 0)),
                || json!({"cell": c}),
            );
        }
    }
}

fn path_fields(kind: &str, label: &str, o: &PathOut, t: &J, sec: &mut Section) -> bool {
    let mut all = true;
    let mut rec = |field: &str, out: Outcome, ours: J, theirs: J| {
        if out != Outcome::Exact {
            all = false;
        }
        sec.record(
            &format!("{kind}.{field}"),
            out,
            || json!({"query": label, "ours": ours, "live": theirs}),
        );
    };
    let ts = dj::s(t, "state").unwrap_or_default();
    rec("state", eq(&o.state, &ts), json!(o.state), json!(ts));
    let terr = dj::s(t, "error").unwrap_or_default();
    rec("error", eq(&o.error.as_str(), &terr), json!(o.error), json!(terr));
    let sn = t["searched_nodes"].as_i64();
    rec(
        "searched_nodes",
        eq(&Some(i64::from(o.searched_nodes)), &sn),
        json!(o.searched_nodes),
        t["searched_nodes"].clone(),
    );
    let g = t["end_node_g"].as_u64();
    rec(
        "end_node_g",
        eq(&o.end_node_g.map(u64::from), &g),
        json!(o.end_node_g),
        t["end_node_g"].clone(),
    );
    let sk = dj::key(&t["start_node"]);
    rec("start_node", eq(&o.start_node, &sk), json!(o.start_node), t["start_node"].clone());
    let ek = dj::key(&t["end_node"]);
    rec("end_node", eq(&o.end_node, &ek), json!(o.end_node), t["end_node"].clone());
    if let Some(sp) = dj::fa::<3>(&t["start_point"]) {
        rec(
            "start_point",
            cmp_fs(&o.start_point, &sp),
            json!(o.start_point),
            t["start_point"].clone(),
        );
    }
    if let Some(ep) = dj::fa::<3>(&t["end_point"]) {
        rec("end_point", cmp_fs(&o.end_point, &ep), json!(o.end_point), t["end_point"].clone());
    }
    let nodes: Vec<Option<[i64; 3]>> = dj::arr(t, "nodes").iter().map(dj::key).collect();
    let on: Vec<Option<[i64; 3]>> = o.nodes.iter().map(|k| Some(*k)).collect();
    let first_diff =
        nodes.iter().zip(&on).position(|(a, b)| a != b).unwrap_or(nodes.len().min(on.len()));
    rec(
        "nodes",
        eq(&on, &nodes),
        json!({"count": on.len(), "first_difference": first_diff}),
        json!({"count": nodes.len()}),
    );
    let sc: Vec<i64> = dj::arr(t, "step_costs").iter().filter_map(J::as_i64).collect();
    rec("step_costs", eq(&o.step_costs, &sc), json!(o.step_costs.len()), json!(sc.len()));
    let sum = t["connection_cost_sum"].as_i64();
    rec(
        "connection_cost_sum",
        eq(&Some(o.connection_cost_sum), &sum),
        json!(o.connection_cost_sum),
        t["connection_cost_sum"].clone(),
    );
    let pts: Vec<[f32; 3]> = dj::arr(t, "points").iter().filter_map(dj::fa::<3>).collect();
    rec("points", cmp_points(&o.points, &pts), json!(o.points), json!(pts));
    if let Some(l) = dj::f(&t["length"]) {
        rec("length", cmp_f(o.length, l), json!(o.length), t["length"].clone());
    }
    if let Some(after) = t.get("points_after_remove_excess_points").and_then(J::as_array) {
        let pts: Vec<[f32; 3]> = after.iter().filter_map(dj::fa::<3>).collect();
        let ours = o.points_after_remove_excess_points.clone().unwrap_or_default();
        rec("points_after_remove_excess_points", cmp_points(&ours, &pts), json!(ours), json!(pts));
    }
    all
}

fn nearest_fields(what: &str, name: &str, o: &NearestOut, t: &J, sec: &mut Section) {
    let k = dj::key(&t["node"]);
    sec.record(
        &format!("{what}.node"),
        eq(&o.node, &k),
        || json!({"point": name, "ours": o.node, "live": t["node"]}),
    );
    if let Some(p) = dj::fa::<3>(&t["position"]) {
        sec.record(
            &format!("{what}.position"),
            cmp_fs(&o.position, &p),
            || json!({"point": name, "ours": o.position, "live": p}),
        );
    }
    if let Some(dd) = dj::f(&t["distance"]) {
        sec.record(
            &format!("{what}.distance"),
            cmp_f(o.distance, dd),
            || json!({"point": name, "ours": o.distance, "live": dd}),
        );
    }
}

/// Live queries vs our A* port.
pub fn diff_queries(dump: &J, ours: &Ours, sec: &mut Section) {
    let w = &ours.world;
    let mesh_gi = w.recast.as_ref().map_or(0, |r| r.graph_index);
    let grid_gi = w.grid.as_ref().map_or(1, |g| g.graph_index);
    for (list, label_key) in [("endpoints", "name"), ("route_points", "sample")] {
        for e in dj::arr(dump, list) {
            let name = e[label_key].to_string();
            let Some(p) = dj::fa::<3>(&e["point"]) else {
                sec.record("coverage.malformed_point", Outcome::Mismatch, || e.clone());
                continue;
            };
            nearest_fields(
                &format!("{list}.nearest_navmesh"),
                &name,
                &w.nearest_out(p, mesh_gi),
                &e["nearest_navmesh"],
                sec,
            );
            nearest_fields(
                &format!("{list}.nearest_grid"),
                &name,
                &w.nearest_out(p, grid_gi),
                &e["nearest_grid"],
                sec,
            );
        }
    }
    for list in ["endpoints", "route_points", "queries", "extractor_queries"] {
        if dj::arr(dump, list).is_empty() {
            sec.record("coverage.missing_section", Outcome::Mismatch, || json!({"section": list}));
        }
    }
    for list in ["queries", "extractor_queries"] {
        for q in dj::arr(dump, list) {
            let label = dj::s(q, "label").unwrap_or_default();
            let (Some(a), Some(b)) = (dj::fa::<3>(&q["start"]), dj::fa::<3>(&q["end"])) else {
                sec.record(
                    "coverage.malformed_query",
                    Outcome::Mismatch,
                    || json!({"label": label}),
                );
                continue;
            };
            let o = w.query_all(label, a, b);
            for (kind, ours) in [
                ("navmesh_raw", &o.navmesh_raw),
                ("navmesh_agent", &o.navmesh_agent),
                ("grid_raw", &o.grid_raw),
                ("tb_clearance_0.5", &o.tb_clearance_05),
                ("tb_clearance_1.0", &o.tb_clearance_10),
            ] {
                let (Some(ours), Some(t)) = (ours, q.get(kind)) else {
                    // A kind missing on either side is lost coverage, not agreement.
                    sec.record("coverage.missing_query_kind", Outcome::Mismatch, || {
                        json!({"label": label, "kind": kind, "ours": ours.is_some(), "live": q.get(kind).is_some()})
                    });
                    continue;
                };
                let all = path_fields(kind, label, ours, t, sec);
                sec.tally(
                    &format!("{kind}.all_fields"),
                    if all { Outcome::Exact } else { Outcome::Mismatch },
                );
            }
        }
    }
    let changed = dump["graph_changed_during_queries"].as_bool();
    sec.record("graph_unchanged_during_queries", eq(&changed, &Some(false)), || json!(changed));
}
