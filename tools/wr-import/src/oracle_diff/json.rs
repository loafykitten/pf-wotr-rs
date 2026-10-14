//! Reading the oracle's JSON (floats written round-trip, `NaN`/`Infinity` as strings,
//! vectors `[x,y,z]`, quaternions `[x,y,z,w]`, matrices row-major) and comparing values.

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::Value as J;

use super::Outcome;

pub fn load(p: &Path) -> Result<J> {
    let bytes = std::fs::read(p).with_context(|| format!("read {}", p.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", p.display()))
}

/// A float as the game wrote it. JSON numbers are the shortest round-trip form of the
/// `float`, so narrowing the parsed double gives the original bits back.
#[must_use]
pub fn f(v: &J) -> Option<f32> {
    match v {
        J::Number(n) => n.as_f64().map(|x| x as f32),
        J::String(s) => match s.as_str() {
            "NaN" => Some(f32::NAN),
            "Infinity" => Some(f32::INFINITY),
            "-Infinity" => Some(f32::NEG_INFINITY),
            _ => None,
        },
        _ => None,
    }
}

#[must_use]
pub fn fa<const N: usize>(v: &J) -> Option<[f32; N]> {
    let a = v.as_array()?;
    if a.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (o, x) in out.iter_mut().zip(a) {
        *o = f(x)?;
    }
    Some(out)
}

/// A row-major `[[m00..m03], ...]` matrix as column-major 16 floats (glam's order).
#[must_use]
pub fn matrix_cols(v: &J) -> Option<[f32; 16]> {
    let rows = v.as_array()?;
    if rows.len() != 4 {
        return None;
    }
    let mut out = [0.0; 16];
    for (r, row) in rows.iter().enumerate() {
        let row: [f32; 4] = fa(row)?;
        for (c, x) in row.iter().enumerate() {
            out[c * 4 + r] = *x;
        }
    }
    Some(out)
}

#[must_use]
pub fn key(v: &J) -> Option<[i64; 3]> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some([a[0].as_i64()?, a[1].as_i64()?, a[2].as_i64()?])
}

#[must_use]
pub fn s<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v.get(k).and_then(J::as_str)
}

#[must_use]
pub fn arr<'a>(v: &'a J, k: &str) -> &'a [J] {
    v.get(k).and_then(J::as_array).map_or(&[], Vec::as_slice)
}

/// Distance in units in the last place between two finite floats (±0 are 0 apart).
#[must_use]
pub fn ulps(a: f32, b: f32) -> u32 {
    let key = |x: f32| {
        let i = x.to_bits() as i32;
        if i < 0 { i32::MIN.wrapping_sub(i) } else { i }
    };
    key(a).abs_diff(key(b))
}

/// Compares floats: bit-identical; close (within 1e-5, or at most 8 ULP apart, which
/// covers float composition-order differences at world coordinates of ~100 m, or ±0);
/// or not.
#[must_use]
pub fn cmp_f(ours: f32, theirs: f32) -> Outcome {
    // The oracle's writer (Mono's round-trip "R" format) prints -0 as 0, so the sign of
    // a zero isn't observable in a dump: ±0 count as identical.
    if ours.to_bits() == theirs.to_bits() || (ours == 0.0 && theirs == 0.0) {
        Outcome::Exact
    } else if (ours - theirs).abs() <= 1e-5
        || (ours.is_finite() && theirs.is_finite() && ulps(ours, theirs) <= 8)
        || (ours.is_nan() && theirs.is_nan())
    {
        Outcome::Close
    } else {
        Outcome::Mismatch
    }
}

/// Compares float arrays component-wise; the worst outcome wins.
#[must_use]
pub fn cmp_fs(ours: &[f32], theirs: &[f32]) -> Outcome {
    if ours.len() != theirs.len() {
        return Outcome::Mismatch;
    }
    let mut worst = Outcome::Exact;
    for (a, b) in ours.iter().zip(theirs) {
        match cmp_f(*a, *b) {
            Outcome::Mismatch => return Outcome::Mismatch,
            Outcome::Close => worst = Outcome::Close,
            _ => {}
        }
    }
    worst
}

/// Compares lists of vectors (same count, each component).
#[must_use]
pub fn cmp_points(ours: &[[f32; 3]], theirs: &[[f32; 3]]) -> Outcome {
    if ours.len() != theirs.len() {
        return Outcome::Mismatch;
    }
    let mut worst = Outcome::Exact;
    for (a, b) in ours.iter().zip(theirs) {
        match cmp_fs(a, b) {
            Outcome::Mismatch => return Outcome::Mismatch,
            Outcome::Close => worst = Outcome::Close,
            _ => {}
        }
    }
    worst
}

#[must_use]
pub fn eq<T: PartialEq>(a: &T, b: &T) -> Outcome {
    if a == b { Outcome::Exact } else { Outcome::Mismatch }
}

/// Largest absolute component difference between two float lists.
#[must_use]
pub fn max_abs(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max)
}
