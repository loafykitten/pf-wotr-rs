//! Conservative ray tests in `f64` for the collider kinds whose PhysX query isn't verified yet:
//! "may this ray touch the shape within `[0, max]`?", with margins wide enough that a `false`
//! is certain. A query that may touch one of them fails closed.

/// World-space margin (metres) added around every unverified shape.
pub const MARGIN: f64 = 1e-3;

/// A column-major 4x4 matrix in `f64`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct M4(pub [f64; 16]);

impl M4 {
    #[must_use]
    pub fn from_f32(m: &[f32; 16]) -> Self {
        Self(m.map(f64::from))
    }

    #[must_use]
    pub fn point(&self, p: [f64; 3]) -> [f64; 3] {
        let m = &self.0;
        [
            m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
            m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
            m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
        ]
    }
}

/// The world-space bounds of `local_to_world` applied to the box `center ± half`.
#[must_use]
pub fn box_bounds(m: &M4, center: [f64; 3], half: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for corner in 0..8 {
        let p = [
            center[0] + if corner & 1 == 0 { -half[0] } else { half[0] },
            center[1] + if corner & 2 == 0 { -half[1] } else { half[1] },
            center[2] + if corner & 4 == 0 { -half[2] } else { half[2] },
        ];
        let w = m.point(p);
        for k in 0..3 {
            lo[k] = lo[k].min(w[k]);
            hi[k] = hi[k].max(w[k]);
        }
    }
    (lo, hi)
}

/// Does the ray `o + t d`, `t` in `[-MARGIN, max + MARGIN]`, touch the box `[lo, hi]` grown
/// by `MARGIN`? `d` is a unit vector.
#[must_use]
pub fn ray_hits_aabb(o: [f64; 3], d: [f64; 3], max: f64, lo: [f64; 3], hi: [f64; 3]) -> bool {
    let (mut t0, mut t1) = (-MARGIN, max + MARGIN);
    for k in 0..3 {
        let (a, b) = (lo[k] - MARGIN, hi[k] + MARGIN);
        if d[k] == 0.0 {
            if o[k] < a || o[k] > b {
                return false;
            }
        } else {
            let (mut ta, mut tb) = ((a - o[k]) / d[k], (b - o[k]) / d[k]);
            if ta > tb {
                std::mem::swap(&mut ta, &mut tb);
            }
            t0 = t0.max(ta);
            t1 = t1.min(tb);
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

/// Does the ray touch the (box-shaped, possibly rotated and scaled) box `center ± half` under
/// `m`? Tested in world space against the box's faces as triangles.
#[must_use]
pub fn ray_hits_box(
    o: [f64; 3],
    d: [f64; 3],
    max: f64,
    m: &M4,
    center: [f64; 3],
    half: [f64; 3],
) -> bool {
    let (lo, hi) = box_bounds(m, center, half);
    if !ray_hits_aabb(o, d, max, lo, hi) {
        return false;
    }
    let corner = |i: usize| {
        m.point([
            center[0] + if i & 1 == 0 { -half[0] } else { half[0] },
            center[1] + if i & 2 == 0 { -half[1] } else { half[1] },
            center[2] + if i & 4 == 0 { -half[2] } else { half[2] },
        ])
    };
    let c: Vec<[f64; 3]> = (0..8).map(corner).collect();
    // The six faces, two triangles each.
    const FACES: [[usize; 4]; 6] =
        [[0, 1, 3, 2], [4, 5, 7, 6], [0, 1, 5, 4], [2, 3, 7, 6], [0, 2, 6, 4], [1, 3, 7, 5]];
    // Inside the box counts as a hit (PhysX reports distance 0 there).
    if point_in_box(o, m, center, half) {
        return true;
    }
    FACES.iter().any(|f| {
        ray_hits_triangle(o, d, max, c[f[0]], c[f[1]], c[f[2]])
            || ray_hits_triangle(o, d, max, c[f[0]], c[f[2]], c[f[3]])
    })
}

fn point_in_box(p: [f64; 3], m: &M4, center: [f64; 3], half: [f64; 3]) -> bool {
    // Solve m * l = p for the local point with the 3x3 part (Cramer's rule).
    let a = &m.0;
    let r = [p[0] - a[12], p[1] - a[13], p[2] - a[14]];
    let col = |j: usize| [a[4 * j], a[4 * j + 1], a[4 * j + 2]];
    let (c0, c1, c2) = (col(0), col(1), col(2));
    let det = dot(c0, cross(c1, c2));
    if det.abs() < 1e-30 {
        return true;
    }
    let l = [dot(r, cross(c1, c2)) / det, dot(c0, cross(r, c2)) / det, dot(c0, cross(c1, r)) / det];
    (0..3).all(|k| (l[k] - center[k]).abs() <= half[k].abs() + MARGIN)
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Does the ray touch the triangle (either side), within a margin of `MARGIN` metres around
/// its edges and along the ray?
#[must_use]
pub fn ray_hits_triangle(
    o: [f64; 3],
    d: [f64; 3],
    max: f64,
    a: [f64; 3],
    b: [f64; 3],
    c: [f64; 3],
) -> bool {
    let e1 = sub(b, a);
    let e2 = sub(c, a);
    let n = cross(e1, e2);
    let nn = dot(n, n).sqrt();
    if nn == 0.0 {
        return false;
    }
    let dn = dot(d, n);
    // Distance of the origin from the plane, and where the ray crosses it.
    let s = dot(sub(a, o), n);
    if dn.abs() < 1e-12 * nn {
        // Parallel: only a ray lying in the plane can touch it; PhysX rejects these
        // (det below its epsilon), but count it anyway when within the margin.
        if (s / nn).abs() > MARGIN {
            return false;
        }
        return segment_near_triangle(o, d, max, a, b, c);
    }
    let t = s / dn;
    if t < -MARGIN || t > max + MARGIN {
        return false;
    }
    let p = [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t];
    // Distance from p (in the plane) to the triangle, compared with the margin.
    point_triangle_distance(p, a, b, c) <= MARGIN
}

fn segment_near_triangle(
    o: [f64; 3],
    d: [f64; 3],
    max: f64,
    a: [f64; 3],
    b: [f64; 3],
    c: [f64; 3],
) -> bool {
    // Sample the segment finely enough for a margin test (rare: rays in a triangle's plane).
    let steps = ((max / MARGIN).ceil() as usize).clamp(1, 1_000_000);
    (0..=steps).any(|i| {
        let t = max * i as f64 / steps as f64;
        let p = [o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t];
        point_triangle_distance(p, a, b, c) <= MARGIN
    })
}

/// Euclidean distance from `p` to the triangle `abc` (Ericson, Real-Time Collision Detection 5.1.5).
fn point_triangle_distance(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> f64 {
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    let closest = if d1 <= 0.0 && d2 <= 0.0 {
        a
    } else {
        let bp = sub(p, b);
        let d3 = dot(ab, bp);
        let d4 = dot(ac, bp);
        if d3 >= 0.0 && d4 <= d3 {
            b
        } else {
            let vc = d1 * d4 - d3 * d2;
            if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
                let v = d1 / (d1 - d3);
                [a[0] + ab[0] * v, a[1] + ab[1] * v, a[2] + ab[2] * v]
            } else {
                let cp = sub(p, c);
                let d5 = dot(ab, cp);
                let d6 = dot(ac, cp);
                if d6 >= 0.0 && d5 <= d6 {
                    c
                } else {
                    let vb = d5 * d2 - d1 * d6;
                    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
                        let w = d2 / (d2 - d6);
                        [a[0] + ac[0] * w, a[1] + ac[1] * w, a[2] + ac[2] * w]
                    } else {
                        let va = d3 * d6 - d5 * d4;
                        if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
                            let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
                            [
                                b[0] + (c[0] - b[0]) * w,
                                b[1] + (c[1] - b[1]) * w,
                                b[2] + (c[2] - b[2]) * w,
                            ]
                        } else {
                            let denom = 1.0 / (va + vb + vc);
                            let v = vb * denom;
                            let w = vc * denom;
                            [
                                a[0] + ab[0] * v + ac[0] * w,
                                a[1] + ab[1] * v + ac[1] * w,
                                a[2] + ab[2] * v + ac[2] * w,
                            ]
                        }
                    }
                }
            }
        }
    };
    let q = sub(p, closest);
    dot(q, q).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: M4 =
        M4([1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    const DOWN: [f64; 3] = [0.0, -1.0, 0.0];

    #[test]
    fn triangles_count_within_the_margin() {
        let (a, b, c) = ([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
        assert!(ray_hits_triangle([0.2, 5.0, 0.2], DOWN, 10.0, a, b, c));
        assert!(ray_hits_triangle([0.2, 5.0, 0.2], DOWN, 10.0, a, c, b), "either side");
        assert!(ray_hits_triangle([-0.0005, 5.0, 0.2], DOWN, 10.0, a, b, c), "within the margin");
        assert!(!ray_hits_triangle([-0.01, 5.0, 0.2], DOWN, 10.0, a, b, c));
        assert!(!ray_hits_triangle([0.2, 5.0, 0.2], DOWN, 4.9, a, b, c), "beyond max");
        assert!(ray_hits_triangle([0.2, 5.0, 0.2], DOWN, 4.9995, a, b, c));
        assert!(!ray_hits_triangle([0.2, -1.0, 0.2], DOWN, 10.0, a, b, c), "behind the origin");
    }

    #[test]
    fn boxes_rotated_scaled_and_from_inside() {
        // A unit box scaled by 2 in x and moved to (10, 0, 0).
        let m =
            M4([2.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 10.0, 0.0, 0.0, 1.0]);
        let (c, h) = ([0.0; 3], [0.5; 3]);
        assert!(ray_hits_box([10.9, 5.0, 0.0], DOWN, 10.0, &m, c, h));
        assert!(!ray_hits_box([11.1, 5.0, 0.0], DOWN, 10.0, &m, c, h));
        assert!(ray_hits_box([10.0, 0.0, 0.0], DOWN, 0.1, &m, c, h), "origin inside");
        assert!(!ray_hits_box([10.0, 5.0, 0.0], DOWN, 4.0, &m, c, h));
        assert!(ray_hits_aabb([0.0, 5.0, 0.0], DOWN, 10.0, [-1.0; 3], [1.0; 3]));
        assert!(!ray_hits_box([0.0, 5.0, 0.0], DOWN, 10.0, &ID, [5.0, 0.0, 0.0], h));
    }
}
