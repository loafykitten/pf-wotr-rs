//! The queries as the game makes them: `Physics.Raycast(origin, Vector3.down, out hit, max,
//! mask)` (Unity's `PhysicsScene.Raycast` with the default query settings over `PxScene::raycast`)
//! and `PhysicUtils.HeuristicDiscCastDown` (Owlcat's managed code, eight such rays).
//!
//! Unity's native `Raycast` passes the ray to `PxScene::raycast` with hit flags
//! `ePOSITION | eNORMAL | eUV` (`eMESH_BOTH_SIDES` only with `Physics.queriesHitBackfaces`,
//! false by default) and a filter that keeps the shapes whose layer is in the mask (and
//! triggers per `queriesHitTriggers`); a +inf distance becomes `FLT_MAX`. PhysX then tests the
//! shapes whose bounds the ray crosses, in its pruner's order, each with the distance shrunk to
//! the closest hit so far, keeping a hit when its distance is `<=` that distance. The pruner's
//! order isn't modelled: the query is run in every order of the shapes that can hit (the height
//! field, which a shorter ray can make hit, and each mesh that hits the whole ray: a mesh's
//! raycast only loses hits as the ray shortens), and a query whose answer depends on the order
//! fails closed, as does any query that may touch a collider kind not verified yet.

use std::borrow::Cow;
use std::fmt;

use crate::bv4::MeshScale;
use crate::conservative::{self, M4};
use crate::px::{Quat, Transform, Vec3};
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
        /// The game's `f32` placement against ours ([`conservative::transform_error`]).
        err: f64,
        lo: [f64; 3],
        hi: [f64; 3],
    },
    /// A mesh with its recorded transform: the BV4 raycast as the player runs it, under every
    /// sign of the recorded zeros (the recording doesn't keep them).
    MeshExact {
        poses: Vec<Transform>,
        scale: MeshScale,
        layer: u32,
    },
    Other {
        lo: [f64; 3],
        hi: [f64; 3],
    },
}

/// A world ready for queries (unverified shapes moved to world space once), borrowing its
/// world ([`Scene::new`]) or owning it ([`Scene::owned`], for a simulation that keeps it).
pub struct Scene<'w> {
    world: Cow<'w, World>,
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
        let prepared = Self::prepare(world);
        Self { world: Cow::Borrowed(world), prepared }
    }

    /// A scene that owns its world.
    #[must_use]
    pub fn owned(world: World) -> Scene<'static> {
        let prepared = Self::prepare(&world);
        Scene { world: Cow::Owned(world), prepared }
    }

    fn prepare(world: &World) -> Vec<Prepared> {
        world
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
                ShapeKind::Mesh { runtime: Some(r), .. } => Prepared::MeshExact {
                    poses: zero_sign_variants(r.position, r.rotation),
                    scale: MeshScale {
                        scale: Vec3::from_array(r.lossy_scale),
                        rotation: Quat::IDENTITY,
                    },
                    layer: r.layer,
                },
                ShapeKind::Mesh { local_to_world, mesh, runtime: None, .. } => {
                    let (vertices, triangles) = (&mesh.vertices, &mesh.triangles);
                    let geom_epsilon = &mesh.geom_epsilon;
                    let m = M4::from_f32(local_to_world);
                    let w: Vec<[f64; 3]> = vertices.iter().map(|v| m.point(f64x3(*v))).collect();
                    let tris: Vec<[[f64; 3]; 3]> = triangles
                        .iter()
                        .map(|t| [w[t[0] as usize], w[t[1] as usize], w[t[2] as usize]])
                        .collect();
                    let mut lo = [f64::INFINITY; 3];
                    let mut hi = [f64::NEG_INFINITY; 3];
                    // Over every cooked vertex, used or not (PhysX's bounds are the vertices').
                    for p in &w {
                        for k in 0..3 {
                            lo[k] = lo[k].min(p[k]);
                            hi[k] = hi[k].max(p[k]);
                        }
                    }
                    // Grown so the prefilter keeps every touch the triangle test may count, and
                    // every node PhysX may test for an unsure one: each triangle's allowance, plus
                    // a hundredth of the mesh's extent (the pruner's 1.01 inflation, BV4's
                    // quantized node bounds).
                    let eps = f64::from(*geom_epsilon);
                    let local = vertices
                        .iter()
                        .flatten()
                        .fold(0.0f64, |acc, &x| acc.max(f64::from(x).abs()))
                        * 3f64.sqrt();
                    let err = conservative::transform_error(&m, local);
                    let extent = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max);
                    let grow = tris
                        .iter()
                        .map(|t| conservative::triangle_grow(*t, eps))
                        .fold(0.0, f64::max)
                        + 0.01 * extent
                        + err;
                    Prepared::Mesh {
                        tris,
                        eps,
                        err,
                        lo: lo.map(|x| x - grow),
                        hi: hi.map(|x| x + grow),
                    }
                }
                ShapeKind::Other { min, max, .. } => {
                    Prepared::Other { lo: f64x3(*min), hi: f64x3(*max) }
                }
            })
            .collect()
    }

    #[must_use]
    pub fn world(&self) -> &World {
        &self.world
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
        // With several height fields, the scene query's order shrinks the distance one passes
        // the next, which can turn a miss into a closer hit: not modelled.
        let fields = self
            .world
            .shapes
            .iter()
            .filter(|s| matches!(s.kind, ShapeKind::HeightField { .. }) && in_mask(s.layer, mask))
            .count();
        if fields > 1 {
            return Err(QueryError::Unsupported(format!(
                "{fields} height fields in the mask (the scene query's shape order)"
            )));
        }
        // The shapes that can hit: the height field (a shorter ray can turn its miss into a
        // hit) and each mesh that hits the whole ray.
        let mut candidates: Vec<usize> = Vec::new();
        for (i, (s, p)) in self.world.shapes.iter().zip(&self.prepared).enumerate() {
            let inside = if let Prepared::MeshExact { layer, .. } = p {
                // The recorded layer, after the load's move.
                in_mask(*layer, mask)
            } else {
                let inside = in_mask(s.layer, mask);
                if s.visual_layer_move && inside != in_mask(VISUAL_LAYER, mask) {
                    return Err(QueryError::Unsupported(format!(
                        "{}: in the mask on layer {} and not on {VISUAL_LAYER} (the load's layer move)",
                        s.name, s.layer
                    )));
                }
                inside
            };
            if !inside {
                continue;
            }
            if s.is_trigger {
                return Err(QueryError::Unsupported(format!(
                    "trigger collider {} in the mask (Physics.queriesHitTriggers)",
                    s.name
                )));
            }
            match p {
                Prepared::HeightField { .. } => candidates.push(i),
                Prepared::MeshExact { .. } => {
                    if self.shape_hit(i, o, dir, max)?.is_some() {
                        candidates.push(i);
                    }
                }
                prep => {
                    if may_touch(prep, f64x3(origin), f64::from(max)) {
                        return Err(QueryError::Unverified { shape: s.name.clone() });
                    }
                }
            }
        }
        if candidates.len() > MAX_CANDIDATES {
            return Err(QueryError::Unsupported(format!(
                "{} shapes the ray may hit (the scene query's shape order)",
                candidates.len()
            )));
        }
        // `MultiQueryCallback` in every order: each shape gets the distance shrunk to the closest
        // hit so far and its hit is kept when `distance <= shrunk` (the height field's distance,
        // recomputed from the rounded point, can exceed the traced length).
        let mut answer: Option<Option<RaycastHit>> = None;
        let mut order = candidates.clone();
        let mut more = true;
        while more {
            let mut shrunk = max;
            let mut best: Option<RaycastHit> = None;
            for &i in &order {
                if let Some((distance, point)) = self.shape_hit(i, o, dir, shrunk)?
                    && distance <= shrunk
                {
                    best = Some(RaycastHit { distance, point, shape: i });
                    shrunk = distance;
                }
            }
            match &answer {
                None => answer = Some(best),
                Some(a) if same_hit(*a, best) => {}
                Some(a) => {
                    return Err(QueryError::Unsupported(format!(
                        "the scene query's shape order decides the hit ({a:?} or {best:?})"
                    )));
                }
            }
            more = next_permutation(&mut order);
        }
        Ok(answer.flatten())
    }

    /// Shape `i`'s raycast as `MultiQueryCallback` makes it, `max` the shrunk distance: the
    /// height field's or the mesh's under every sign of its recorded zeros (all equal, or the
    /// query fails closed).
    fn shape_hit(
        &self,
        i: usize,
        o: Vec3,
        dir: Vec3,
        max: f32,
    ) -> Result<Option<(f32, [f32; 3])>, QueryError> {
        let s = &self.world.shapes[i];
        match (&s.kind, &self.prepared[i]) {
            (ShapeKind::HeightField { pose, field }, Prepared::HeightField { bounds }) => Ok(field
                .raycast_in(*bounds, *pose, o, dir, max, false)
                .map(|h| (h.distance, h.position.to_array()))),
            (ShapeKind::Mesh { mesh, .. }, Prepared::MeshExact { poses, scale, .. }) => {
                let mut first: Option<Option<(f32, [f32; 3])>> = None;
                for pose in poses {
                    // Unity's hit flags: `ePOSITION | eNORMAL | eUV`; the geometry single-sided.
                    let h = crate::bv4::raycast_triangle_mesh(
                        mesh, scale, false, *pose, o, dir, max, HIT_FLAGS,
                    )
                    .map(|h| (h.distance, h.position.to_array()));
                    match first {
                        None => first = Some(h),
                        Some(f) if key(f) == key(h) => {}
                        Some(_) => {
                            return Err(QueryError::Unsupported(format!(
                                "the sign of a zero in {}'s recorded transform decides the hit",
                                s.name
                            )));
                        }
                    }
                }
                Ok(first.flatten())
            }
            _ => unreachable!("only height fields and recorded meshes are computed"),
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

/// Unity's raycast hit flags: `ePOSITION | eNORMAL | eUV` (back faces not hit).
const HIT_FLAGS: u16 = 0xb;

/// The most shapes whose orders a query tries (5! orders).
const MAX_CANDIDATES: usize = 5;

/// A hit compared bit for bit (the shape too: the caller checks the collider).
fn key(h: Option<(f32, [f32; 3])>) -> Option<(u32, [u32; 3])> {
    h.map(|(d, p)| (d.to_bits(), p.map(f32::to_bits)))
}

fn same_hit(a: Option<RaycastHit>, b: Option<RaycastHit>) -> bool {
    a.map(|h| (h.shape, key(Some((h.distance, h.point)))))
        == b.map(|h| (h.shape, key(Some((h.distance, h.point)))))
}

/// The next permutation in lexicographic order; false (and `v` back to sorted) after the last.
pub(crate) fn next_permutation(v: &mut [usize]) -> bool {
    let Some(i) = (1..v.len()).rev().find(|&i| v[i - 1] < v[i]) else {
        v.reverse();
        return false;
    };
    let j = (i..v.len()).rev().find(|&j| v[j] > v[i - 1]).expect("v[i] qualifies");
    v.swap(i - 1, j);
    v[i..].reverse();
    true
}

/// The pose PhysX may hold for a recorded position and rotation: every sign of each zero
/// component (the recording writes a zero without its sign).
pub(crate) fn zero_sign_variants(position: [f32; 3], rotation: [f32; 4]) -> Vec<Transform> {
    let mut v: Vec<f32> = rotation.iter().chain(&position).copied().collect();
    let zeros: Vec<usize> = (0..v.len()).filter(|&k| v[k] == 0.0).collect();
    let mut out = Vec::with_capacity(1 << zeros.len());
    for signs in 0u32..1 << zeros.len() {
        for (b, &k) in zeros.iter().enumerate() {
            v[k] = if signs >> b & 1 == 0 { 0.0 } else { -0.0 };
        }
        out.push(Transform {
            q: Quat { x: v[0], y: v[1], z: v[2], w: v[3] },
            p: Vec3::new(v[4], v[5], v[6]),
        });
    }
    out
}

fn may_touch(p: &Prepared, o: [f64; 3], max: f64) -> bool {
    const DOWN: [f64; 3] = [0.0, -1.0, 0.0];
    match p {
        Prepared::HeightField { .. } | Prepared::MeshExact { .. } => false,
        Prepared::Box { m, center, half } => {
            conservative::ray_hits_box(o, DOWN, max, m, *center, *half)
        }
        Prepared::Mesh { tris, eps, err, lo, hi } => {
            // An unsure triangle defers to the mesh's grown bounds (every node BV4 may test);
            // the prefilter is wide enough for every sure touch.
            let enclosed = conservative::ray_hits_aabb(o, DOWN, max, *lo, *hi, 0.0);
            let grow = conservative::UNSURE_SLACKS * conservative::slack(o, max);
            (enclosed || conservative::ray_hits_aabb(o, DOWN, max, *lo, *hi, grow))
                && tris.iter().any(|t| {
                    match conservative::ray_touches_triangle(o, DOWN, max, *t, *eps, *err) {
                        conservative::Touch::Yes => true,
                        conservative::Touch::Unsure => enclosed,
                        conservative::Touch::No => false,
                    }
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
) -> Option<crate::hf::RayHit> {
    field.raycast(pose, Vec3::from_array(origin), Vec3::from_array(dir), max, false)
}
