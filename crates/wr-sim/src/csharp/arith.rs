//! C# arithmetic as the game's Mono executes it.
//!
//! - **Integers** are `unchecked` in the game's code (no `checked` blocks on the rules
//!   paths ported so far), so `+ - *` wrap modulo 2^32 / 2^64. Division truncates toward
//!   zero, like Rust's; dividing by zero, or `int.MinValue / -1`, throws in C#
//!   (`DivideByZeroException` / `OverflowException`), which we surface as [`CsError`] so
//!   a port can reproduce the exception path instead of panicking.
//! - **Floats:** pf-wotr-silicon's DXT port measured that the game's Mono evaluates `float`
//!   expressions in double precision and rounds to `float` only when a value is stored
//!   (a local, field or argument). Strict `f32` arithmetic mismatched. So a port computes
//!   an expression in `f64` from `f32` inputs and rounds once with [`store_f32`]. This
//!   matters for division and for chains of operations; a single `f32 * f32` is exact in
//!   double, so the two models only diverge where the double result needs rounding twice.
//! - **Float → int casts:** Mono on arm64 emits `fcvtzs`, which truncates toward zero and
//!   saturates (NaN → 0). Rust's `as` cast does exactly that [inferred from the ISA; not yet
//!   checked against the running game].
//! - **Rounding:** `Math.Round(double)` and `Mathf.RoundToInt` round half to even
//!   (`MidpointRounding.ToEven`); `Math.Round(x, MidpointRounding.AwayFromZero)` rounds half
//!   away from zero; `Mathf.FloorToInt`/`CeilToInt` are `(int)Math.Floor/Ceiling`.

/// An exception the C# code would throw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsError {
    DivideByZero,
    Overflow,
}

impl std::fmt::Display for CsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DivideByZero => f.write_str("System.DivideByZeroException"),
            Self::Overflow => f.write_str("System.OverflowException"),
        }
    }
}

impl std::error::Error for CsError {}

/// `a + b` on `int`, unchecked.
#[must_use]
pub fn add(a: i32, b: i32) -> i32 {
    a.wrapping_add(b)
}

/// `a - b` on `int`, unchecked.
#[must_use]
pub fn sub(a: i32, b: i32) -> i32 {
    a.wrapping_sub(b)
}

/// `a * b` on `int`, unchecked.
#[must_use]
pub fn mul(a: i32, b: i32) -> i32 {
    a.wrapping_mul(b)
}

/// `-a` on `int`, unchecked (`-int.MinValue == int.MinValue`).
#[must_use]
pub fn neg(a: i32) -> i32 {
    a.wrapping_neg()
}

/// `a / b` on `int`: truncates toward zero; throws on zero and on `MinValue / -1`.
///
/// # Errors
///
/// [`CsError::DivideByZero`] or [`CsError::Overflow`].
pub fn div(a: i32, b: i32) -> Result<i32, CsError> {
    if b == 0 {
        return Err(CsError::DivideByZero);
    }
    a.checked_div(b).ok_or(CsError::Overflow)
}

/// `a % b` on `int`: the sign follows the dividend; throws on zero. (`MinValue % -1` is 0 in
/// C# on Mono, which matches `wrapping_rem`.)
///
/// # Errors
///
/// [`CsError::DivideByZero`].
pub fn rem(a: i32, b: i32) -> Result<i32, CsError> {
    if b == 0 {
        return Err(CsError::DivideByZero);
    }
    Ok(a.wrapping_rem(b))
}

/// Stores a double-precision intermediate into a `float` (round to nearest, ties to even):
/// the point where Mono rounds a `float` expression.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn store_f32(x: f64) -> f32 {
    x as f32
}

/// `(int)f` for a `float` or `double` on arm64: truncation toward zero, saturating, NaN → 0.
#[must_use]
#[allow(clippy::cast_possible_truncation)]
pub fn to_int(x: f64) -> i32 {
    x as i32
}

/// `Math.Round(x)` / `Mathf.Round`: half to even.
#[must_use]
pub fn round_half_even(x: f64) -> f64 {
    x.round_ties_even()
}

/// `Math.Round(x, MidpointRounding.AwayFromZero)`.
#[must_use]
pub fn round_half_away(x: f64) -> f64 {
    x.round()
}

/// `Mathf.RoundToInt(f)`: `(int)Math.Round((double)f)`.
#[must_use]
pub fn round_to_int(f: f32) -> i32 {
    to_int(round_half_even(f64::from(f)))
}

/// `Mathf.FloorToInt(f)`.
#[must_use]
pub fn floor_to_int(f: f32) -> i32 {
    to_int(f64::from(f).floor())
}

/// `Mathf.CeilToInt(f)`.
#[must_use]
pub fn ceil_to_int(f: f32) -> i32 {
    to_int(f64::from(f).ceil())
}

/// `Mathf.Clamp01`.
#[must_use]
pub fn clamp01(f: f32) -> f32 {
    f.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_wrap_like_unchecked_csharp() {
        assert_eq!(add(i32::MAX, 1), i32::MIN);
        assert_eq!(sub(i32::MIN, 1), i32::MAX);
        assert_eq!(mul(0x4000_0000, 4), 0);
        assert_eq!(neg(i32::MIN), i32::MIN);
    }

    #[test]
    fn division_truncates_and_throws_like_csharp() {
        assert_eq!(div(-7, 2), Ok(-3));
        assert_eq!(div(7, -2), Ok(-3));
        assert_eq!(div(1, 0), Err(CsError::DivideByZero));
        assert_eq!(div(i32::MIN, -1), Err(CsError::Overflow));
        assert_eq!(rem(-7, 2), Ok(-1));
        assert_eq!(rem(i32::MIN, -1), Ok(0));
        // An attribute bonus is `value / 2 - 5`: 7 → -2 and 3 → -4 (truncation, not floor).
        assert_eq!(div(7, 2).unwrap() - 5, -2);
        assert_eq!(div(3, 2).unwrap() - 5, -4);
        assert_eq!(div(-1, 2).unwrap() - 5, -5);
    }

    #[test]
    fn rounding_ties() {
        assert_eq!(round_to_int(0.5), 0);
        assert_eq!(round_to_int(1.5), 2);
        assert_eq!(round_to_int(2.5), 2);
        assert_eq!(round_to_int(-2.5), -2);
        assert!((round_half_away(2.5) - 3.0).abs() < f64::EPSILON);
        assert!((round_half_away(-2.5) + 3.0).abs() < f64::EPSILON);
        assert_eq!(floor_to_int(-0.5), -1);
        assert_eq!(ceil_to_int(-0.5), 0);
    }

    #[test]
    fn float_casts_saturate_on_arm64() {
        assert_eq!(to_int(f64::NAN), 0);
        assert_eq!(to_int(1e20), i32::MAX);
        assert_eq!(to_int(-1e20), i32::MIN);
        assert_eq!(to_int(-1.9), -1);
    }

    #[test]
    fn double_evaluation_then_store_differs_from_strict_f32() {
        // `a / b * c` in float: Mono evaluates it in double and rounds once on store.
        // Find a case where the strict-f32 chain rounds differently.
        let (a, b, c) = (1.0f32, 3.0f32, 3.0f32);
        let mono = store_f32(f64::from(a) / f64::from(b) * f64::from(c));
        let strict = a / b * c;
        assert_eq!(mono.to_bits(), 1.0f32.to_bits());
        assert_eq!(strict.to_bits(), 1.0f32.to_bits());
        // `a / b + c` over a range of inputs: the models diverge for some, so a port that
        // used strict f32 would be observably wrong.
        let diverge = (1..20_000u16).any(|i| {
            let a = f32::from(i) * 0.37;
            let (b, c) = (7.0f32, 0.1f32);
            store_f32(f64::from(a) / f64::from(b) + f64::from(c)).to_bits() != (a / b + c).to_bits()
        });
        assert!(diverge, "the two models must diverge somewhere");
    }
}
