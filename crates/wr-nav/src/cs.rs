//! C# and Unity arithmetic as the game's Mono runs it.
//!
//! Mono evaluates `float` expressions in double precision and rounds to `float` only
//! when a value is stored (locals, fields, arguments, return values); see PLAN's "C#
//! arithmetic semantics". Measured on the oracle: path lengths summed with
//! `Vector3.Distance` match 230/230 this way, 192/230 with strict `float` arithmetic.
//! So every helper here computes its expression in `f64` and rounds once where the C#
//! stores. Comparisons inside an expression are done on the unrounded `f64` value.

/// A Unity `Vector3`.
pub type V3 = [f32; 3];

/// Rounds a double to the `float` it is stored as.
#[inline]
#[must_use]
pub fn r(x: f64) -> f32 {
    x as f32
}

#[inline]
#[must_use]
pub fn d(x: f32) -> f64 {
    f64::from(x)
}

#[must_use]
pub fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[must_use]
pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `Vector3 * float` (one rounding per component: same as `f32` multiplication).
#[must_use]
pub fn mul(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

#[must_use]
pub fn div(a: V3, s: f32) -> V3 {
    [a[0] / s, a[1] / s, a[2] / s]
}

/// `Vector3.Dot`: the sum in double, rounded on return.
#[must_use]
pub fn dot(a: V3, b: V3) -> f32 {
    r(d(a[0]) * d(b[0]) + d(a[1]) * d(b[1]) + d(a[2]) * d(b[2]))
}

/// `Vector3.sqrMagnitude`.
#[must_use]
pub fn sqr_magnitude(a: V3) -> f32 {
    dot(a, a)
}

/// `Vector3.magnitude`: `(float)Math.Sqrt(x * x + y * y + z * z)`.
#[must_use]
pub fn magnitude(a: V3) -> f32 {
    r((d(a[0]) * d(a[0]) + d(a[1]) * d(a[1]) + d(a[2]) * d(a[2])).sqrt())
}

/// `Vector3.Distance`.
#[must_use]
pub fn distance(a: V3, b: V3) -> f32 {
    magnitude(sub(a, b))
}

/// `Vector3.Normalize`.
#[must_use]
pub fn normalize(a: V3) -> V3 {
    let m = magnitude(a);
    if m > 1e-5 { div(a, m) } else { [0.0; 3] }
}

/// `Vector3.Cross`.
#[must_use]
pub fn cross(a: V3, b: V3) -> V3 {
    [
        r(d(a[1]) * d(b[2]) - d(a[2]) * d(b[1])),
        r(d(a[2]) * d(b[0]) - d(a[0]) * d(b[2])),
        r(d(a[0]) * d(b[1]) - d(a[1]) * d(b[0])),
    ]
}

/// `Mathf.Clamp01`.
#[must_use]
pub fn clamp01(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}

/// `Mathf.Clamp(value, min, max)`.
#[must_use]
pub fn clamp(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// `Vector3.Lerp` (clamped).
#[must_use]
pub fn lerp(a: V3, b: V3, t: f32) -> V3 {
    let t = d(clamp01(t));
    [0, 1, 2].map(|i| r(d(a[i]) + (d(b[i]) - d(a[i])) * t))
}

/// `Vector3 ==`: squared distance below `9.9999994E-11f` (evaluated in double).
#[must_use]
pub fn approx_eq(a: V3, b: V3) -> bool {
    let x = d(a[0] - b[0]);
    let y = d(a[1] - b[1]);
    let z = d(a[2] - b[2]);
    x * x + y * y + z * z < d(9.999_999_4e-11_f32)
}

/// `Vector2 ==` (same rule in two dimensions).
#[must_use]
pub fn approx_eq2(a: [f32; 2], b: [f32; 2]) -> bool {
    let x = d(a[0] - b[0]);
    let y = d(a[1] - b[1]);
    x * x + y * y < d(9.999_999_4e-11_f32)
}

/// `Math.Round(double)` then `(int)`: ties to even.
#[must_use]
pub fn round_to_i32(x: f64) -> i32 {
    x.round_ties_even() as i32
}

/// `(Int3)Vector3`: `(int)Math.Round(v * 1000f)`, the product in double.
#[must_use]
pub fn to_int3(v: V3) -> [i32; 3] {
    v.map(|c| round_to_i32(d(c) * d(1000.0_f32)))
}

/// `(Vector3)Int3`: `(float)x * 0.001f` per component.
#[must_use]
pub fn from_int3(p: [i32; 3]) -> V3 {
    p.map(|c| r(d(c as f32) * d(0.001_f32)))
}

/// `Int3.magnitude`: double sqrt narrowed to float.
#[must_use]
pub fn int3_magnitude(p: [i32; 3]) -> f32 {
    let [x, y, z] = p.map(f64::from);
    r((x * x + y * y + z * z).sqrt())
}

/// `Int3.costMagnitude`: `(int)Math.Round(magnitude)`.
#[must_use]
pub fn cost_magnitude(p: [i32; 3]) -> i32 {
    round_to_i32(d(int3_magnitude(p)))
}

#[must_use]
pub fn int3_sub(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0].wrapping_sub(b[0]), a[1].wrapping_sub(b[1]), a[2].wrapping_sub(b[2])]
}

#[must_use]
pub fn int3_add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0].wrapping_add(b[0]), a[1].wrapping_add(b[1]), a[2].wrapping_add(b[2])]
}

/// `(uint)` of a double in C#: truncation toward zero (values here are non-negative
/// and in range; out-of-range conversions are unspecified in C#, saturated here).
#[must_use]
pub fn to_u32(x: f64) -> u32 {
    if x.is_nan() || x <= 0.0 {
        0
    } else if x >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        x as u32
    }
}

/// A Unity `Matrix4x4`, row-major `m[row][col]`.
pub type M4 = [[f32; 4]; 4];

pub const IDENTITY: M4 =
    [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

/// `Matrix4x4 * Matrix4x4` (managed: each element's sum in double, stored as float).
#[must_use]
pub fn mat_mul(a: &M4, b: &M4) -> M4 {
    let mut m = [[0.0; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = r((0..4).map(|k| d(a[i][k]) * d(b[k][j])).fold(0.0, |s, x| s + x));
        }
    }
    m
}

/// `Matrix4x4.MultiplyPoint3x4`.
#[must_use]
pub fn mul_point3x4(m: &M4, p: V3) -> V3 {
    [0, 1, 2]
        .map(|i| r(d(m[i][0]) * d(p[0]) + d(m[i][1]) * d(p[1]) + d(m[i][2]) * d(p[2]) + d(m[i][3])))
}

/// `Matrix4x4.MultiplyVector`.
#[must_use]
pub fn mul_vector(m: &M4, p: V3) -> V3 {
    [0, 1, 2].map(|i| r(d(m[i][0]) * d(p[0]) + d(m[i][1]) * d(p[1]) + d(m[i][2]) * d(p[2])))
}

/// Unity's native `Quaternion.Euler` (degrees, Z-X-Y order), in float: the player's
/// `EulerToQuaternion` multiplies `qy * qx`, then `* qz`, with every zero term of the
/// general product kept (they only decide the sign of a zero). Rust's `sin_cos` gives the
/// player's `__sincosf_stret` bit for bit (checked on every float in [-8, 8]).
#[must_use]
pub fn quat_euler(deg: V3) -> [f32; 4] {
    // The managed side's `euler * Mathf.Deg2Rad`, then the native `* 0.5`.
    let k = std::f32::consts::PI / 180.0;
    let (sx, cx) = (deg[0] * k * 0.5).sin_cos();
    let (sy, cy) = (deg[1] * k * 0.5).sin_cos();
    let (sz, cz) = (deg[2] * k * 0.5).sin_cos();
    let z = 0.0f32;
    let ax = sy * z + (cx * z + sx * cy);
    let ay = sx * z + (cx * sy + cy * z);
    let az = ((cx * z + cy * z) + z) - sx * sy;
    let aw = (cx * cy - sx * z) - sy * z;
    [
        (sz * ay + (cz * ax + aw * z)) - az * z,
        (az * z + (cz * ay + aw * z)) - sz * ax,
        (ax * z + (sz * aw + cz * az)) - ay * z,
        ((cz * aw - ax * z) - ay * z) - sz * az,
    ]
}

/// The player's `Vector3f` magnitude: `(x² + y²) + z²` and `sqrtf`, single precision.
#[must_use]
pub fn native_magnitude(a: V3) -> f32 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// The player's `Matrix3x3f::SetAxisAngle` (`m[row][col]`): `sincosf`, then Rodrigues' terms
/// with `t = 1 - cos` multiplying the axis products.
fn axis_angle(axis: V3, angle: f32) -> [[f32; 3]; 3] {
    let (s, c) = angle.sin_cos();
    let [x, y, z] = axis;
    let t = 1.0 - c;
    let (sx, sy, sz) = (s * x, s * y, s * z);
    [
        [c + t * (x * x), t * (x * y) - sz, sy + t * (x * z)],
        [t * (x * y) + sz, c + t * (y * y), t * (y * z) - sx],
        [t * (x * z) - sy, sx + t * (y * z), c + t * (z * z)],
    ]
}

/// Unity's native `Vector3.RotateTowards` (`RotateTowards` in the player): `MoveTowards` when
/// either vector is shorter than 1e-5 or they're within 1 - 1e-5 of parallel; else a rotation
/// of the normalised `current` about `current × target` (Unity's `OrthoNormalVectorFast` axis
/// when antiparallel) by `min(acos(dot), max_radians)`, scaled by the length clamped toward
/// `target`'s by `max_magnitude`.
#[must_use]
#[expect(
    clippy::neg_cmp_op_on_partial_ord,
    reason = "the player's branches take a NaN the way these negations do"
)]
pub fn rotate_towards(current: V3, target: V3, max_radians: f32, max_magnitude: f32) -> V3 {
    let lmag = native_magnitude(current);
    let rmag = native_magnitude(target);
    let move_towards = || {
        let d = [target[0] - current[0], target[1] - current[1], target[2] - current[2]];
        let sq = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
        // Both tests branch on `le`, which a NaN takes.
        if !(sq > max_magnitude * max_magnitude) {
            return target;
        }
        let len = sq.sqrt();
        if !(len > 1e-5) {
            return current;
        }
        [0, 1, 2].map(|i| current[i] + d[i] / len * max_magnitude)
    };
    if !(lmag > 1e-5 && rmag > 1e-5) {
        return move_towards();
    }
    let l = [current[0] / lmag, current[1] / lmag, current[2] / lmag];
    let r = [target[0] / rmag, target[1] / rmag, target[2] / rmag];
    let dot = l[0] * r[0] + l[1] * r[1] + l[2] * r[2];
    // 0x3f7fff58: 1 - 1e-5 as the player's float.
    let near = f32::from_bits(0x3f7f_ff58);
    if dot > near {
        return move_towards();
    }
    let (axis, angle) = if dot < -near {
        let axis = if l[2].abs() > std::f32::consts::FRAC_1_SQRT_2 {
            let k = 1.0 / (l[2] * l[2] + l[1] * l[1]).sqrt();
            [0.0, -l[2] * k, l[1] * k]
        } else {
            let k = 1.0 / (l[0] * l[0] + l[1] * l[1]).sqrt();
            [-l[1] * k, l[0] * k, 0.0]
        };
        (axis, max_radians)
    } else {
        let c = [l[1] * r[2] - l[2] * r[1], l[2] * r[0] - l[0] * r[2], l[0] * r[1] - l[1] * r[0]];
        let n = native_magnitude(c);
        let a = dot.acos();
        ([c[0] / n, c[1] / n, c[2] / n], if a < max_radians { a } else { max_radians })
    };
    let m = axis_angle(axis, angle);
    let rotated = [0, 1, 2].map(|i| m[i][0] * l[0] + m[i][1] * l[1] + m[i][2] * l[2]);
    let delta = rmag - lmag;
    let len = if delta > 0.0 {
        lmag + if delta > max_magnitude { max_magnitude } else { delta }
    } else {
        lmag - if -delta > max_magnitude { max_magnitude } else { -delta }
    };
    rotated.map(|x| x * len)
}

/// Unity's native `Matrix4x4.TRS`, in float.
#[must_use]
pub fn trs(t: V3, q: [f32; 4], s: V3) -> M4 {
    let [x, y, z, w] = q;
    let (x2, y2, z2) = (x + x, y + y, z + z);
    let (xx, yy, zz) = (x * x2, y * y2, z * z2);
    let (xy, xz, yz) = (x * y2, x * z2, y * z2);
    let (wx, wy, wz) = (w * x2, w * y2, w * z2);
    [
        [(1.0 - (yy + zz)) * s[0], (xy - wz) * s[1], (xz + wy) * s[2], t[0]],
        [(xy + wz) * s[0], (1.0 - (xx + zz)) * s[1], (yz - wx) * s[2], t[1]],
        [(xz - wy) * s[0], (yz + wx) * s[1], (1.0 - (xx + yy)) * s[2], t[2]],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Unity's native general 4×4 inverse (Mesa's `invert_matrix_general`: Gaussian
/// elimination with partial pivoting), in float. `None` if singular. As in the player's
/// code, the first two eliminations skip a zero multiplier only on the right half, and the
/// third never does (that only decides the sign of a zero).
#[must_use]
#[expect(clippy::needless_range_loop, reason = "row operations over two columns of one matrix")]
pub fn mat_inverse(m: &M4) -> Option<M4> {
    let mut w = [[0.0f32; 8]; 4];
    for (i, row) in w.iter_mut().enumerate() {
        row[..4].copy_from_slice(&m[i]);
        row[4 + i] = 1.0;
    }
    let mut rows = [0usize, 1, 2, 3];
    for col in 0..4 {
        // Choose the pivot among the remaining rows, bottom-up as Mesa's swaps do.
        for k in (col + 1..4).rev() {
            if w[rows[k]][col].abs() > w[rows[k - 1]][col].abs() {
                rows.swap(k, k - 1);
            }
        }
        let p = rows[col];
        if w[p][col] == 0.0 {
            return None;
        }
        for &rk in &rows[col + 1..] {
            let f = w[rk][col] / w[p][col];
            for c in col + 1..8 {
                let s = w[p][c];
                if c < 4 || col == 2 || s != 0.0 {
                    w[rk][c] -= f * s;
                }
            }
        }
    }
    // Back substitution.
    for col in (0..4).rev() {
        let p = rows[col];
        let s = 1.0 / w[p][col];
        for c in 4..8 {
            w[p][c] *= s;
        }
        for &rk in &rows[..col] {
            let f = w[rk][col];
            for c in 4..8 {
                w[rk][c] -= w[p][c] * f;
            }
        }
    }
    let mut out = [[0.0; 4]; 4];
    for (i, &ri) in rows.iter().enumerate() {
        out[i].copy_from_slice(&w[ri][4..8]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_evaluation_rounds_once() {
        // 0.1f*0.1f + 0.2f*0.2f + 0.3f*0.3f rounds differently in double than in float
        // steps; dot() must use the double sum.
        let a = [0.1, 0.2, 0.3];
        let strict = 0.1f32 * 0.1 + 0.2 * 0.2 + 0.3 * 0.3;
        let mono = dot(a, a);
        assert_eq!(
            mono,
            ((0.1f32 as f64).powi(2) + (0.2f32 as f64).powi(2) + (0.3f32 as f64).powi(2)) as f32
        );
        let _ = strict;
    }

    #[test]
    fn int3_round_trips() {
        assert_eq!(to_int3([1.0005, -2.5, 0.0]), [1000, -2500, 0]);
        // `x * 0.001f` evaluated in double and narrowed: 1500 gives 1.5000001, not 1.5.
        assert_eq!(from_int3([1500, 0, -250]), [1.500_000_1, 0.0, -0.25]);
        assert_eq!(cost_magnitude([3, 4, 0]), 5);
        assert_eq!(cost_magnitude([5594, 707, 0]), 5638);
    }

    #[test]
    fn inverse_of_scaled_translation() {
        let m = [[0.5, 0.0, 0.0, 15.0], [0.0, 1.0, 0.0, 50.0], [0.0, 0.0, 0.5, 40.0], IDENTITY[3]];
        let inv = mat_inverse(&m).unwrap();
        assert_eq!(inv[0], [2.0, 0.0, 0.0, -30.0]);
        assert_eq!(inv[1], [0.0, 1.0, 0.0, -50.0]);
        assert_eq!(mul_point3x4(&inv, mul_point3x4(&m, [3.0, 4.0, 5.0])), [3.0, 4.0, 5.0]);
        assert!(mat_inverse(&[[0.0; 4]; 4]).is_none());
    }

    #[test]
    fn trs_of_identity_rotation() {
        let m = trs([1.0, 2.0, 3.0], quat_euler([0.0; 3]), [2.0, 1.0, 2.0]);
        assert_eq!(m[0], [2.0, 0.0, 0.0, 1.0]);
        assert_eq!(mat_mul(&m, &IDENTITY), m);
    }
}
