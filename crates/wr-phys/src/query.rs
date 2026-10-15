//! The queries as the game makes them: `Physics.Raycast(origin, Vector3.down, out hit, max,
//! mask)` (Unity's `PhysicsScene.Raycast` with the default query settings over `PxScene::raycast`)
//! and `PhysicUtils.HeuristicDiscCastDown` (Owlcat's managed code, eight such rays).
//!
//! Unity's native `Raycast` passes the ray to `PxScene::raycast` with hit flags
//! `ePOSITION | eNORMAL | eUV` (`eMESH_BOTH_SIDES` only with `Physics.queriesHitBackfaces`,
//! false by default) and a filter that keeps the shapes whose layer is in the mask (and
//! triggers per `queriesHitTriggers`); a +inf distance becomes `FLT_MAX`. PhysX then tests the
//! shapes whose bounds the ray crosses, each with the distance shrunk to the closest hit so
//! far, and returns the closest. With one shape hit the result doesn't depend on that order;
//! with more it may (a shrunk distance changes a height field's trace), so that case fails
//! closed, as does any query that may touch a collider kind not verified yet.

use std::fmt;

use crate::conservative::{self, M4};
use crate::hf::RayHit;
use crate::px::{Transform, Vec3};
use crate::{ShapeKind, World};

/// The closest hit: `RaycastHit.distance` and `.point`, and the shape's index in the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RaycastHit {
    pub distance: f32,
    pub point: [f32; 3],
    pub shape: usize,
}

/// Why a query can't be answered exactly.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryError {
    /// The ray may touch a collider whose query isn't verified against the game yet.
    Unverified { shape: String },
    /// The answer would depend on something not modelled (named).
    Unsupported(String),
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unverified { shape } => {
                write!(f, "the ray may touch {shape}, whose query isn't verified yet")
            }
            Self::Unsupported(what) => write!(f, "not modelled: {what}"),
        }
    }
}

impl std::error::Error for QueryError {}

enum Prepared {
    /// The field's local bounds (PhysX keeps them with the height field).
    HeightField {
        bounds: crate::px::Bounds,
    },
    Box {
        m: M4,
        center: [f64; 3],
        half: [f64; 3],
    },
    Mesh {
        tris: Vec<[[f64; 3]; 3]>,
        eps: f64,
        lo: [f64; 3],
        hi: [f64; 3],
    },
    Other {
        lo: [f64; 3],
        hi: [f64; 3],
    },
}

/// A world ready for queries (unverified shapes moved to world space once).
pub struct Scene<'w> {
    world: &'w World,
    prepared: Vec<Prepared>,
}

impl fmt::Debug for Scene<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Scene").field("shapes", &self.world.shapes.len()).finish()
    }
}

fn f64x3(a: [f32; 3]) -> [f64; 3] {
    a.map(f64::from)
}

impl<'w> Scene<'w> {
    #[must_use]
    pub fn new(world: &'w World) -> Self {
        let prepared = world
            .shapes
            .iter()
            .map(|s| match &s.kind {
                ShapeKind::HeightField { field, .. } => {
                    Prepared::HeightField { bounds: field.local_bounds() }
                }
                ShapeKind::Box { local_to_world, center, size } => {
                    let m = M4::from_f32(local_to_world);
                    let center = f64x3(*center);
                    let half = f64x3(*size).map(|x| x.abs() * 0.5);
                    Prepared::Box { m, center, half }
                }
                ShapeKind::Mesh { local_to_world, vertices, triangles, geom_epsilon } => {
                    let m = M4::from_f32(local_to_world);
                    let w: Vec<[f64; 3]> = vertices.iter().map(|v| m.point(f64x3(*v))).collect();
                    let tris: Vec<[[f64; 3]; 3]> = triangles
                        .iter()
                        .map(|t| [w[t[0] as usize], w[t[1] as usize], w[t[2] as usize]])
                        .collect();
                    let mut lo = [f64::INFINITY; 3];
                    let mut hi = [f64::NEG_INFINITY; 3];
                    for p in tris.iter().flatten() {
                        for k in 0..3 {
                            lo[k] = lo[k].min(p[k]);
                            hi[k] = hi[k].max(p[k]);
                        }
                    }
                    // Grown so the prefilter keeps every touch the triangle test may count.
                    let eps = f64::from(*geom_epsilon);
                    let grow = tris
                        .iter()
                        .map(|t| conservative::triangle_grow(*t, eps))
                        .fold(0.0, f64::max);
                    Prepared::Mesh { tris, eps, lo: lo.map(|x| x - grow), hi: hi.map(|x| x + grow) }
                }
                ShapeKind::Other { min, max, .. } => {
                    Prepared::Other { lo: f64x3(*min), hi: f64x3(*max) }
                }
            })
            .collect();
        Self { world, prepared }
    }

    #[must_use]
    pub fn world(&self) -> &World {
        self.world
    }

    /// `Physics.Raycast(origin, Vector3.down, out hit, max, mask)`: the closest hit.
    pub fn raycast_down(
        &self,
        origin: [f32; 3],
        max: f32,
        mask: i32,
    ) -> Result<Option<RaycastHit>, QueryError> {
        if !origin.iter().all(|x| x.is_finite()) || max.is_nan() || max < 0.0 {
            return Err(QueryError::Unsupported(format!(
                "a raycast from {origin:?} over {max} m (PhysX rejects it)"
            )));
        }
        // Unity: `maxDistance == +inf` becomes `FLT_MAX`.
        let max = if max == f32::INFINITY { f32::MAX } else { max };
        // `Vector3.down` normalised by `PhysicsScene.Raycast` and `Ray` stays (0, -1, 0).
        let dir = Vec3::new(0.0, -1.0, 0.0);
        let o = Vec3::from_array(origin);
        let mut hits: Vec<(usize, RayHit)> = Vec::new();
        for (i, (s, p)) in self.world.shapes.iter().zip(&self.prepared).enumerate() {
            let inside = in_mask(s.layer, mask);
            if s.visual_layer_move && inside != in_mask(VISUAL_LAYER, mask) {
                return Err(QueryError::Unsupported(format!(
                    "{}: in the mask on layer {} and not on {VISUAL_LAYER} (the load's layer move)",
                    s.name, s.layer
                )));
            }
            if !inside {
                continue;
            }
            if s.is_trigger {
                return Err(QueryError::Unsupported(format!(
                    "trigger collider {} in the mask (Physics.queriesHitTriggers)",
                    s.name
                )));
            }
            match (&s.kind, p) {
                (ShapeKind::HeightField { pose, field }, Prepared::HeightField { bounds }) => {
                    // `MultiQueryCallback` keeps a blocking hit only when `distance <= shrunk`
                    // (`max` here): faceHit's distance, recomputed from the rounded point, can
                    // exceed the traced length.
                    if let Some(h) = field.raycast_in(*bounds, *pose, o, dir, max, false)
                        && h.distance <= max
                    {
                        hits.push((i, h));
                    }
                }
                (_, prep) => {
                    if may_touch(prep, f64x3(origin), f64::from(max)) {
                        return Err(QueryError::Unverified { shape: s.name.clone() });
                    }
                }
            }
        }
        match hits.as_slice() {
            [] => Ok(None),
            [(i, h)] => Ok(Some(RaycastHit {
                distance: h.distance,
                point: h.position.to_array(),
                shape: *i,
            })),
            _ => Err(QueryError::Unsupported(format!(
                "a ray hitting {} height fields (the scene query's shape order)",
                hits.len()
            ))),
        }
    }

    /// `PhysicUtils.HeuristicDiscCastDown(origin, radius, out hit, max, mask)`: of the eight
    /// rays down from `origin + point * radius`, the first with the smallest distance.
    pub fn heuristic_disc_cast_down(
        &self,
        origin: [f32; 3],
        radius: f32,
        max: f32,
        mask: i32,
    ) -> Result<Option<RaycastHit>, QueryError> {
        let mut best: Option<RaycastHit> = None;
        for v in disc_points() {
            // `origin + vector * radius`: Mono rounds each component to float at the store,
            // the same as float arithmetic for a single operation.
            let o =
                [origin[0] + v[0] * radius, origin[1] + v[1] * radius, origin[2] + v[2] * radius];
            // `!raycastHit.HasValue || raycastHit.Value.distance > hitInfo2.distance`.
            if let Some(h) = self.raycast_down(o, max, mask)?
                && best.is_none_or(|b| b.distance > h.distance)
            {
                best = Some(h);
            }
        }
        Ok(best)
    }
}

/// `Layers.OptimizeShadow`, where the load moves `*_!Visual` objects.
const VISUAL_LAYER: u32 = 11;

fn in_mask(layer: u32, mask: i32) -> bool {
    layer < 32 && (mask as u32 >> layer) & 1 != 0
}

fn may_touch(p: &Prepared, o: [f64; 3], max: f64) -> bool {
    const DOWN: [f64; 3] = [0.0, -1.0, 0.0];
    match p {
        Prepared::HeightField { .. } => false,
        Prepared::Box { m, center, half } => {
            conservative::ray_hits_box(o, DOWN, max, m, *center, *half)
        }
        Prepared::Mesh { tris, eps, lo, hi } => {
            conservative::ray_hits_aabb(o, DOWN, max, *lo, *hi, 0.0)
                // Unsure: the mesh's grown bounds, which the ray crosses, enclose every node BV4
                // would test.
                && tris.iter().any(|t| {
                    conservative::ray_touches_triangle(o, DOWN, max, *t, *eps)
                        != conservative::Touch::No
                })
        }
        Prepared::Other { lo, hi } => conservative::ray_hits_aabb(o, DOWN, max, *lo, *hi, 0.0),
    }
}

/// `PhysicUtils.s_NormalizedDiscPoints` as its static constructor fills them: four points at
/// `sin(3π/4)` and four at `sin(π/2)` from the centre, at angles 0, π/4, π/2, 3π/4
/// (`Mathf.Sin`/`Mathf.Cos` are `(float)Math.Sin((double)x)`, libm's; the angle accumulates in
/// float; C#'s constant folding gives `(float)Math.PI * 3f / 4f` and `(float)Math.PI / 4f`).
#[must_use]
pub fn disc_points() -> [[f32; 3]; 8] {
    let pi = std::f64::consts::PI as f32;
    let sin = |x: f32| f64::from(x).sin() as f32;
    let cos = |x: f32| f64::from(x).cos() as f32;
    let mut out = [[0.0f32; 3]; 8];
    for (start, sin_y) in [(0, sin(pi * 3.0 / 4.0)), (4, sin(pi / 2.0))] {
        let mut num = 0.0f32;
        for p in out.iter_mut().skip(start).take(4) {
            let (c, s) = (cos(num), sin(num));
            *p = [sin_y * c, 0.0, sin_y * s];
            num += pi / 4.0;
        }
    }
    out
}

/// A height field shape alone, for checks: `raycast_heightField` as the scene query calls it.
#[must_use]
pub fn raycast_height_field(
    pose: Transform,
    field: &crate::hf::HeightField,
    origin: [f32; 3],
    dir: [f32; 3],
    max: f32,
) -> Option<RayHit> {
    field.raycast(pose, Vec3::from_array(origin), Vec3::from_array(dir), max, false)
}
