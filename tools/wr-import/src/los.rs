//! The area's line-of-sight inputs for the sim: `wr-import los-pack` turns the scenes'
//! `FogOfWarBlocker` components into `wr_los::LosFile` (`cache/sim/los/<guid>.wrlos`).
//!
//! A blocker registers in `LineOfSightGeometry` when its component is enabled on an active
//! GameObject (`OnEnable` -> `Rebuild` -> `BuildPoints`):
//!
//! - with a `FogOfWarBlockerPolygon`: its `TransformedPoints` (`transform.TransformPoint` of
//!   each local point), `Closed` (`poly.Closed || TwoSided`), `HeightMinMax.y = max y + Height`;
//!   `TwoSided` mirrors the points back (`length * 2 - 1` of them);
//! - else with a `MeshFilter`: the mesh's vertices transformed, their convex hull on the ground
//!   plane (`Polygon.ConvexHullXZ`, a gift wrap), closed, `HeightMinMax.y = max y`;
//! - else the points are null and the registration throws: refused here.
//!
//! `TransformPoint` is Unity's own hierarchy walk, not a product with the world matrix:
//! `wr_phys::boxes::transform_point` over the GameObject's local transforms (the player's
//! `Transform::TransformPoint`, checked bit for bit against it by `wr-phys`'s `native_check` and
//! against `poc5-tb-140`'s `los_geom`: every point of its 88 segments).

use anyhow::{Context, Result, bail, ensure};
use wr_los::{Blocker, FogArea, LosFile};

use crate::area::Universe;
use crate::scene::{GoSnapshot, SceneData};
use crate::unity::value::Value;

const BLOCKER: &str =
    "Owlcat.Runtime.Visual.RenderPipeline.RendererFeatures.FogOfWar.FogOfWarBlocker";
const POLYGON: &str =
    "Owlcat.Runtime.Visual.RenderPipeline.RendererFeatures.FogOfWar.FogOfWarBlockerPolygon";
const UPDATER: &str =
    "Owlcat.Runtime.Visual.RenderPipeline.RendererFeatures.FogOfWar.FogOfWarBlockerUpdater";
const AREA: &str = "Owlcat.Runtime.Visual.RenderPipeline.RendererFeatures.FogOfWar.FogOfWarArea";

/// What the build saw, for the report.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct LosReport {
    pub blockers: usize,
    pub polygons: usize,
    pub meshes: usize,
    /// Left out: disabled, or on an inactive GameObject.
    pub skipped_inactive: usize,
    pub updatable: usize,
    pub areas: usize,
    /// Per blocker: its scene path, source, points and height.
    pub list: Vec<String>,
}

/// `VectorMath.RightOrColinearXZ(a, b, p)`.
fn right_or_colinear_xz(a: [f32; 3], b: [f32; 3], p: [f32; 3]) -> bool {
    let d = f64::from;
    (d(b[0]) - d(a[0])) * (d(p[2]) - d(a[2])) - (d(p[0]) - d(a[0])) * (d(b[2]) - d(a[2])) <= 0.0
}

/// `Polygon.ConvexHullXZ(points, hull)`.
fn convex_hull_xz(points: &[[f32; 3]]) -> Result<Vec<[f32; 3]>> {
    let mut hull = Vec::new();
    if points.is_empty() {
        return Ok(hull);
    }
    let mut num = 0usize;
    for i in 1..points.len() {
        if points[i][0] < points[num][0] {
            num = i;
        }
    }
    let start = num;
    let mut steps = 0;
    loop {
        hull.push(points[num]);
        let mut num4 = 0usize;
        for j in 0..points.len() {
            if num4 == num || !right_or_colinear_xz(points[num], points[num4], points[j]) {
                num4 = j;
            }
        }
        num = num4;
        steps += 1;
        // (The game logs "Infinite Loop in Convex Hull Calculation" and stops there.)
        ensure!(steps <= 10_000, "ConvexHullXZ does not end");
        if num == start {
            break;
        }
    }
    Ok(hull)
}

fn class_of<'a>(s: &'a SceneData, g: &GoSnapshot, name: &str) -> Option<&'a Value> {
    s.components_of(g.path_id).into_iter().find(|(c, _)| *c == name).map(|(_, v)| v)
}

/// Builds the area's line-of-sight file from its scenes.
pub fn build(
    universe: &Universe,
    scenes: &[SceneData],
    guid: &str,
) -> Result<(LosFile, LosReport)> {
    let mut rep = LosReport::default();
    let mut blockers = Vec::new();
    let mut areas = Vec::new();
    let mut extended = 0u32;
    for s in scenes {
        for g in s.game_objects() {
            let comps = s.components_of(g.path_id);
            for (class, v) in &comps {
                if *class == AREA
                    && g.active_in_hierarchy
                    && v.get("m_Enabled").and_then(Value::as_bool) != Some(false)
                {
                    areas.push(FogArea {
                        scene: s.scene.clone(),
                        path: g.path.clone(),
                        position: g.world.position,
                    });
                    rep.areas += 1;
                }
                if class.ends_with("SpawnerGuardSettings")
                    && v.get("ExtendedVisionArea")
                        .and_then(Value::as_pptr)
                        .is_some_and(|p| !p.is_null())
                {
                    extended += 1;
                }
            }
            // (Each enabled component registers on its own.)
            for (_, v) in comps.iter().filter(|(c, _)| *c == BLOCKER) {
                let name = format!("{}:{}", s.scene, g.path);
                if !g.active_in_hierarchy
                    || v.get("m_Enabled").and_then(Value::as_bool) == Some(false)
                {
                    rep.skipped_inactive += 1;
                    continue;
                }
                let chain = s.transform_chain(g.path_id);
                let two_sided = v.get("TwoSided").and_then(Value::as_bool).unwrap_or(false);
                let updatable = comps.iter().any(|(c, _)| *c == UPDATER);
                let (points, closed, height_y, source) = if let Some(poly) =
                    class_of(s, &g, POLYGON)
                {
                    let raw = poly
                        .get("m_Points")
                        .and_then(Value::as_array)
                        .with_context(|| format!("{name}: m_Points"))?;
                    let local: Vec<[f32; 3]> = raw
                        .iter()
                        .map(|p| p.vec3().with_context(|| format!("{name}: a point")))
                        .collect::<Result<_>>()?;
                    let world: Vec<[f32; 3]> =
                        local.iter().map(|p| wr_phys::boxes::transform_point(&chain, *p)).collect();
                    let mut lo = [f32::MAX; 3];
                    let mut hi = [f32::MIN; 3];
                    for p in &world {
                        for k in 0..3 {
                            lo[k] = lo[k].min(p[k]);
                            hi[k] = hi[k].max(p[k]);
                        }
                    }
                    let height = poly.get("Height").and_then(Value::as_f32).unwrap_or(0.0);
                    let n = if two_sided { world.len() * 2 - 1 } else { world.len() };
                    let mut pts = vec![[0.0f32; 2]; n];
                    for (j, p) in world.iter().enumerate() {
                        pts[j] = [p[0], p[2]];
                        if two_sided && j > 0 {
                            let at = pts.len() - j;
                            pts[at] = [p[0], p[2]];
                        }
                    }
                    let closed =
                        poly.get("m_Closed").and_then(Value::as_bool).unwrap_or(false) || two_sided;
                    rep.polygons += 1;
                    (pts, closed, hi[1] + height, "polygon")
                } else if let Some(filter) = class_of(s, &g, "MeshFilter") {
                    let p = filter
                        .get("m_Mesh")
                        .and_then(Value::as_pptr)
                        .with_context(|| format!("{name}: m_Mesh"))?;
                    let o = universe
                        .resolve(s.file, p)?
                        .with_context(|| format!("{name}: the mesh isn't found"))?;
                    let mv = o.read()?;
                    let decoded = crate::pack::mesh::decode(universe, &mv)
                        .with_context(|| format!("{name}: the mesh"))?;
                    let local = crate::phys::positions(&decoded);
                    ensure!(!local.is_empty(), "{name}: a mesh without readable positions");
                    let world: Vec<[f32; 3]> =
                        local.iter().map(|p| wr_phys::boxes::transform_point(&chain, *p)).collect();
                    let mut hi_y = f32::MIN;
                    for p in &world {
                        hi_y = hi_y.max(p[1]);
                    }
                    let hull = convex_hull_xz(&world)?;
                    rep.meshes += 1;
                    (hull.iter().map(|p| [p[0], p[2]]).collect(), true, hi_y, "mesh")
                } else {
                    bail!(
                        "{name}: a blocker with neither a polygon nor a mesh (its registration throws)"
                    );
                };
                ensure!(
                    points.iter().flatten().all(|x| x.is_finite()) && height_y.is_finite(),
                    "{name}: a point or height that isn't finite"
                );
                rep.list.push(format!(
                    "{name} [{source}] {} points {points:?}, closed {closed}, height {height_y}{}",
                    points.len(),
                    if updatable { ", updatable" } else { "" }
                ));
                rep.updatable += usize::from(updatable);
                blockers.push(Blocker {
                    scene: s.scene.clone(),
                    path: g.path.clone(),
                    sibling_path: g.sibling_path.clone(),
                    points,
                    closed,
                    height_y,
                    source: source.to_owned(),
                    updatable,
                });
            }
        }
    }
    rep.blockers = blockers.len();
    let mut bundles: Vec<String> = scenes.iter().map(|s| s.bundle.clone()).collect();
    bundles.dedup();
    Ok((
        LosFile {
            area: guid.to_owned(),
            scenes: bundles,
            blockers,
            areas,
            extended_vision_spawners: extended,
        },
        rep,
    ))
}
