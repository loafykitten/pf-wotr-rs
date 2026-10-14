//! `System.Decimal` as the game's Mono runtime executes it, for the operations the AI uses:
//! `(decimal)float`, literals, `*`, `-` (and `+`), `Math.Abs`, the comparisons and
//! `ToString()` in the invariant culture.
//!
//! The arithmetic is not managed code in Mono but icalls in C, a port of CoreCLR's old
//! `decimal.cpp` (`mono/metadata/decimal-ms.c`, read from Unity's fork,
//! `https://raw.githubusercontent.com/Unity-Technologies/mono/unity-2020.3-mbe/mono/metadata/decimal-ms.c`).
//! Ported function by function, including the `goto` structure and every rounding rule:
//!
//! - `(decimal)float` is `mono_decimal_from_float` (`VarDecFromR4`; the exponent bias is 126,
//!   `MONO_SINGLE_BIAS`): the float is widened to a
//!   double, scaled to a 7-digit integer with a power of 10 estimated from the binary exponent
//!   (`exp * 19728 >> 16`, corrected once), rounded half to even on the 8th digit, and the
//!   trailing zeros of the 7 digits are factored out (powers 4, 2, 1) so `0.8f` is `0.8`
//!   (scale 1) and `1f` is `1` (scale 0). Exponents below -94 give `0`; above 96 overflow.
//!   Negative zero and negative values that round to zero are positive `0`.
//! - `*` is `mono_decimal_multiply_result` (`VarDecMul`): an exact 192-bit product, then
//!   `ScaleResult` divides by powers of 10 when the scale exceeds 28 or the mantissa 96 bits,
//!   rounding half to even with a sticky bit. A product of zero with both upper words zero keeps
//!   the summed scale (`0.0m * 0.8m` is `0.00`); a wide zero product, or a scale over 47, is
//!   plain `0`.
//! - `-` and `+` are `mono_decimal_addsub` (`DecAddSub`): scale alignment by multiplying the
//!   smaller-scale operand up, 96-bit add or subtract with a sign flip, `ScaleResult` when the
//!   aligned sum needs more than 96 bits. `a - a` for a negative `a` is negative zero.
//! - Comparison (`mono_decimal_compare`) subtracts through `DecAddSub` and reads the sign; here
//!   it compares exactly instead. The two agree except when the subtraction overflows, where
//!   Mono reads an uninitialised result (the C leaves `result` unset on that path).
//! - `Math.Abs` clears the sign and keeps the scale.
//!
//! `ToString()` is Mono's own `NumberFormatter` (`mcs/class/corlib/System/NumberFormatter.cs`,
//! `FormatGeneral` with precision -1 on a decimal): all digits of the mantissa with the point
//! `scale` places from the right, a leading `0` before the point, trailing zeros kept as the
//! scale says (`0.80m` is `0.80`, `0.0m` is `0.0`), and zero is always printed unsigned
//! (`_positive = true` for a zero mantissa), so negative zero prints `0`.
//!
//! The test vectors come from CoreCLR's managed `DecCalc` (`dotnet` 11), a port of the same C++.
//! It agrees with the Mono C on every vector except the sign of one zero: `0.00000m - 0m` is
//! negative zero in the Mono C (the operand swap flips the result's sign) and positive zero in
//! CoreCLR. Mono wins here; both print `0.00000` and compare equal to `0`.

use std::cmp::Ordering;
use std::fmt;

use super::arith::CsError;

const NEG: u8 = 0x80;
const DEC_SCALE_MAX: i32 = 28;
const POWER10_MAX: i32 = 9;
const TEN_TO_NINE: u32 = 1_000_000_000;
const TEN_TO_TEN_DIV_4: u32 = 2_500_000_000;

const POWER10: [u32; 10] =
    [1, 10, 100, 1_000, 10_000, 100_000, 1_000_000, 10_000_000, 100_000_000, 1_000_000_000];

/// `System.Decimal`: a 96-bit unsigned mantissa, a scale 0..=28 and a sign. Equality and
/// ordering are numeric (`0.80m == 0.8m`); use [`Decimal::bits`] for the exact representation.
#[derive(Debug, Clone, Copy)]
pub struct Decimal {
    lo: u32,
    mid: u32,
    hi: u32,
    /// 0 or `NEG`.
    sign: u8,
    scale: u8,
}

impl Default for Decimal {
    /// `default(decimal)`: 0.
    fn default() -> Self {
        Self::ZERO
    }
}

impl Decimal {
    pub const ZERO: Self = Self::from_parts(0, 0, 0, false, 0);
    pub const ONE: Self = Self::lit(1, 0);

    /// A literal or the `Decimal(lo, mid, hi, isNegative, scale)` constructor.
    ///
    /// # Panics
    ///
    /// If `scale` is above 28 (the C# constructor throws `ArgumentOutOfRangeException`).
    #[must_use]
    pub const fn from_parts(lo: u32, mid: u32, hi: u32, negative: bool, scale: u8) -> Self {
        assert!(scale <= 28, "decimal scale above 28");
        Self { lo, mid, hi, sign: if negative { NEG } else { 0 }, scale }
    }

    /// The literal `mantissa * 10^-scale`, e.g. `Decimal::lit(1, 8)` is `0.00000001m` and
    /// `Decimal::lit(0, 1)` is `0.0m`.
    #[must_use]
    pub const fn lit(mantissa: u64, scale: u8) -> Self {
        Self::from_parts(mantissa as u32, (mantissa >> 32) as u32, 0, false, scale)
    }

    /// `decimal.GetBits`: lo, mid, hi and the flags word (scale in bits 16..24, sign in bit 31).
    #[must_use]
    #[allow(clippy::cast_possible_wrap)]
    pub fn bits(self) -> [i32; 4] {
        [
            self.lo as i32,
            self.mid as i32,
            self.hi as i32,
            ((u32::from(self.scale) << 16) | (u32::from(self.sign) << 24)) as i32,
        ]
    }

    #[must_use]
    pub fn scale(self) -> u8 {
        self.scale
    }

    /// The sign bit (true for negative zero as well).
    #[must_use]
    pub fn is_negative(self) -> bool {
        self.sign != 0
    }

    #[must_use]
    pub fn is_zero(self) -> bool {
        self.lo | self.mid | self.hi == 0
    }

    /// `Math.Abs(decimal)`: the sign cleared, the scale kept.
    #[must_use]
    pub fn abs(self) -> Self {
        Self { sign: 0, ..self }
    }

    fn lo64(self) -> u64 {
        u64::from(self.mid) << 32 | u64::from(self.lo)
    }

    fn set_lo64(&mut self, v: u64) {
        self.lo = v as u32;
        self.mid = (v >> 32) as u32;
    }

    /// `(decimal)f` (`new Decimal(float)`, `mono_decimal_init_single`).
    ///
    /// # Errors
    ///
    /// [`CsError::Overflow`] for NaN, infinities and magnitudes of 2^97 and more.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_precision_loss)]
    pub fn from_f32(f: f32) -> Result<Self, CsError> {
        let bits = f.to_bits();
        let exp = ((bits >> 23) & 0xff) as i32 - 126;
        if exp < -94 {
            return Ok(Self::ZERO);
        }
        if exp > 96 {
            return Err(CsError::Overflow);
        }
        let mut dbl = f64::from(f).abs();
        let mut power = 6 - ((exp * 19728) >> 16);
        if power >= 0 {
            if power > DEC_SCALE_MAX {
                power = DEC_SCALE_MAX;
            }
            dbl *= 10f64.powi(power);
        } else if power != -1 || dbl >= 1e7 {
            dbl /= 10f64.powi(-power);
        } else {
            power = 0;
        }
        if dbl < 1e6 && power < DEC_SCALE_MAX {
            dbl *= 10.0;
            power += 1;
        }
        let mut mant = dbl as i32 as u32;
        dbl -= f64::from(mant);
        if dbl > 0.5 || (dbl == 0.5 && mant & 1 != 0) {
            mant += 1;
        }
        if mant == 0 {
            return Ok(Self::ZERO);
        }
        let sign = if bits >> 31 != 0 { NEG } else { 0 };
        if power < 0 {
            let wide = u128::from(mant) * 10u128.pow((-power) as u32);
            if wide >> 96 != 0 {
                return Err(CsError::Overflow);
            }
            return Ok(Self {
                lo: wide as u32,
                mid: (wide >> 32) as u32,
                hi: (wide >> 64) as u32,
                sign,
                scale: 0,
            });
        }
        // Factor out trailing zeros of the 7 digits: powers 4, 2, 1.
        let mut lmax = power.min(6);
        let mut cur = 4;
        while cur > 0 {
            if cur <= lmax {
                let p = 10u32.pow(cur as u32);
                if mant.is_multiple_of(p) {
                    mant /= p;
                    power -= cur;
                    lmax -= cur;
                }
            }
            cur >>= 1;
        }
        Ok(Self { lo: mant, mid: 0, hi: 0, sign, scale: power as u8 })
    }

    /// `a * b` (`decimal.op_Multiply`, `VarDecMul`).
    ///
    /// # Errors
    ///
    /// [`CsError::Overflow`] when the product does not fit even after scale reduction.
    #[allow(clippy::cast_possible_truncation, clippy::many_single_char_names)]
    pub fn checked_mul(self, right: Self) -> Result<Self, CsError> {
        let left = self;
        let mut scale = i32::from(left.scale) + i32::from(right.scale);
        let sign = right.sign ^ left.sign;
        if (left.hi | left.mid | right.hi | right.mid) == 0 {
            // Upper 64 bits are zero.
            let mut tmp = u64::from(left.lo) * u64::from(right.lo);
            if scale > DEC_SCALE_MAX {
                scale -= DEC_SCALE_MAX;
                if scale > 19 {
                    return Ok(Self::ZERO);
                }
                let (rem_lo, mut pwr);
                if scale > POWER10_MAX {
                    // Divide by 1E10 first: 2.5E9 now, then the next divisor times 4.
                    rem_lo = (tmp % u64::from(TEN_TO_TEN_DIV_4)) as u32;
                    tmp /= u64::from(TEN_TO_TEN_DIV_4);
                    pwr = POWER10[(scale - 10) as usize] << 2;
                } else {
                    rem_lo = 0;
                    pwr = POWER10[scale as usize];
                }
                let rem_hi = (tmp % u64::from(pwr)) as u32;
                tmp /= u64::from(pwr);
                pwr >>= 1;
                if rem_hi >= pwr && (rem_hi > pwr || (rem_lo | (tmp as u32 & 1)) != 0) {
                    tmp += 1;
                }
                scale = DEC_SCALE_MAX;
            }
            return Ok(Self {
                lo: tmp as u32,
                mid: (tmp >> 32) as u32,
                hi: 0,
                sign,
                scale: scale as u8,
            });
        }
        // The 192-bit product, as the C accumulates it (exact either way).
        let l = [left.lo, left.mid, left.hi];
        let r = [right.lo, right.mid, right.hi];
        let mut prod = [0u32; 7];
        for (i, &a) in l.iter().enumerate() {
            let mut carry = 0u64;
            for (j, &b) in r.iter().enumerate() {
                let t = u64::from(a) * u64::from(b) + u64::from(prod[i + j]) + carry;
                prod[i + j] = t as u32;
                carry = t >> 32;
            }
            prod[i + 3] = carry as u32;
        }
        let Some(hi_prod) = prod[..6].iter().rposition(|&w| w != 0) else {
            return Ok(Self::ZERO);
        };
        let Some(scale) = scale_result(&mut prod, hi_prod as i32, scale) else {
            return Err(CsError::Overflow);
        };
        Ok(Self { lo: prod[0], mid: prod[1], hi: prod[2], sign, scale: scale as u8 })
    }

    /// `a - b` (`decimal.op_Subtraction`).
    ///
    /// # Errors
    ///
    /// [`CsError::Overflow`].
    pub fn checked_sub(self, right: Self) -> Result<Self, CsError> {
        add_sub(self, right, NEG)
    }

    /// `a + b` (`decimal.op_Addition`).
    ///
    /// # Errors
    ///
    /// [`CsError::Overflow`].
    pub fn checked_add(self, right: Self) -> Result<Self, CsError> {
        add_sub(self, right, 0)
    }

    /// The mantissa times `10^up` as six little-endian words (96 bits * 10^28 fits).
    fn scaled_words(self, up: u32) -> [u32; 6] {
        let mut w = [self.lo, self.mid, self.hi, 0, 0, 0];
        let mut left = up;
        while left > 0 {
            let step = left.min(9);
            let p = u64::from(POWER10[step as usize]);
            let mut carry = 0u64;
            for x in &mut w {
                let t = u64::from(*x) * p + carry;
                *x = t as u32;
                carry = t >> 32;
            }
            left -= step;
        }
        w
    }
}

impl PartialEq for Decimal {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Decimal {}

impl PartialOrd for Decimal {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Decimal {
    /// `decimal.Compare`: numeric, zeros of any sign and scale equal.
    fn cmp(&self, other: &Self) -> Ordering {
        let sign = |d: &Self| -> i8 {
            if d.is_zero() {
                0
            } else if d.sign != 0 {
                -1
            } else {
                1
            }
        };
        let (ls, rs) = (sign(self), sign(other));
        if ls != rs {
            return ls.cmp(&rs);
        }
        if ls == 0 {
            return Ordering::Equal;
        }
        let a = self.scaled_words(u32::from(self.scale.max(other.scale) - self.scale));
        let b = other.scaled_words(u32::from(self.scale.max(other.scale) - other.scale));
        let mag = a.iter().rev().cmp(b.iter().rev());
        if ls < 0 { mag.reverse() } else { mag }
    }
}

/// `ToString()` / `ToString(CultureInfo.InvariantCulture)`.
impl fmt::Display for Decimal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mant = u128::from(self.hi) << 64 | u128::from(self.mid) << 32 | u128::from(self.lo);
        let digits = mant.to_string();
        let scale = usize::from(self.scale);
        let mut out = String::new();
        if self.sign != 0 && mant != 0 {
            out.push('-');
        }
        if scale == 0 {
            out.push_str(&digits);
        } else if digits.len() > scale {
            let (int, frac) = digits.split_at(digits.len() - scale);
            out.push_str(int);
            out.push('.');
            out.push_str(frac);
        } else {
            out.push_str("0.");
            out.push_str(&"0".repeat(scale - digits.len()));
            out.push_str(&digits);
        }
        f.write_str(&out)
    }
}

/// `ScaleResult`: divide the up-to-192-bit value `res` (highest non-zero word `hi_res`) by
/// powers of 10 until it fits 96 bits at scale <= 28, rounding half to even with a sticky
/// bit. The new scale, or `None` on overflow.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap)]
fn scale_result(res: &mut [u32], mut hi_res: i32, mut scale: i32) -> Option<i32> {
    let mut new_scale = hi_res * 32 - 64 - 1;
    if new_scale > 0 {
        // Bit position of the MSB, as the C finds it.
        let mut tmp = res[hi_res as usize];
        for (mask, shift) in
            [(0xFFFF_0000u32, 16), (0xFF00_0000, 8), (0xF000_0000, 4), (0xC000_0000, 2)]
        {
            if tmp & mask == 0 {
                new_scale -= shift;
                tmp <<= shift;
            }
        }
        if tmp & 0x8000_0000 == 0 {
            new_scale -= 1;
        }
        new_scale = ((new_scale * 77) >> 8) + 1;
        if new_scale > scale {
            return None;
        }
    } else {
        new_scale = 0;
    }
    if new_scale < scale - DEC_SCALE_MAX {
        new_scale = scale - DEC_SCALE_MAX;
    }
    if new_scale == 0 {
        return Some(scale);
    }
    scale -= new_scale;
    let mut sticky = 0u32;
    let mut rem = 0u32;
    loop {
        sticky |= rem;
        let mut pwr =
            if new_scale > POWER10_MAX { TEN_TO_NINE } else { POWER10[new_scale as usize] };
        // First quotient.
        let top = res[hi_res as usize];
        res[hi_res as usize] = top / pwr;
        rem = top % pwr;
        let mut cur = hi_res - 1;
        if cur >= 0 {
            if res[hi_res as usize] == 0 {
                hi_res -= 1;
            }
            while cur >= 0 {
                let n = u64::from(rem) << 32 | u64::from(res[cur as usize]);
                res[cur as usize] = (n / u64::from(pwr)) as u32;
                rem = (n % u64::from(pwr)) as u32;
                cur -= 1;
            }
        }
        new_scale -= POWER10_MAX;
        if new_scale > 0 {
            continue;
        }
        if hi_res > 2 {
            new_scale = 1;
            scale -= 1;
            continue;
        }
        // Round: remainder above half, or exactly half with an odd result or a sticky bit.
        pwr >>= 1;
        if pwr <= rem && (pwr < rem || (res[0] & 1) | sticky != 0) {
            let mut c = 0usize;
            res[0] = res[0].wrapping_add(1);
            while res[c] == 0 {
                c += 1;
                res[c] = res[c].wrapping_add(1);
            }
            if c > 2 {
                // The rounding carried beyond 96 bits: divide by 10 more.
                hi_res = c as i32;
                sticky = 0;
                rem = 0;
                new_scale = 1;
                scale -= 1;
                continue;
            }
        }
        if scale < 0 {
            return None;
        }
        return Some(scale);
    }
}

/// Negate the 96-bit magnitude and flip the sign (the C's `SignFlip` label).
fn sign_flip(res: &mut Decimal) {
    res.set_lo64(res.lo64().wrapping_neg());
    res.hi = !res.hi;
    if res.lo64() == 0 {
        res.hi = res.hi.wrapping_add(1);
    }
    res.sign ^= NEG;
}

/// The `AlignedAdd` label: add or subtract two operands of the same scale into `res` (which
/// already carries the scale and the sign of `left`).
fn aligned_add(
    left: &Decimal,
    right: &Decimal,
    res: &mut Decimal,
    sign: u8,
) -> Result<(), CsError> {
    if sign != 0 {
        res.set_lo64(left.lo64().wrapping_sub(right.lo64()));
        res.hi = left.hi.wrapping_sub(right.hi);
        if res.lo64() > left.lo64() {
            res.hi = res.hi.wrapping_sub(1);
            if res.hi >= left.hi {
                sign_flip(res);
            }
        } else if res.hi > left.hi {
            sign_flip(res);
        }
    } else {
        res.set_lo64(left.lo64().wrapping_add(right.lo64()));
        res.hi = left.hi.wrapping_add(right.hi);
        let mut carried = false;
        if res.lo64() < left.lo64() {
            res.hi = res.hi.wrapping_add(1);
            if res.hi <= left.hi {
                carried = true;
            }
        } else if res.hi < left.hi {
            carried = true;
        }
        if carried {
            // The sum carried above 96 bits: divide by 10, dropping the scale.
            if res.scale == 0 {
                return Err(CsError::Overflow);
            }
            res.scale -= 1;
            let mut n = (1u64 << 32) | u64::from(res.hi);
            res.hi = (n / 10) as u32;
            n = (n % 10) << 32 | u64::from(res.mid);
            res.mid = (n / 10) as u32;
            n = (n % 10) << 32 | u64::from(res.lo);
            res.lo = (n / 10) as u32;
            let rem = (n % 10) as u32;
            if rem >= 5 && (rem > 5 || res.lo & 1 != 0) {
                res.set_lo64(res.lo64().wrapping_add(1));
                if res.lo64() == 0 {
                    res.hi = res.hi.wrapping_add(1);
                }
            }
        }
    }
    Ok(())
}

/// `mono_decimal_addsub`: `sign` is 0 for addition, `NEG` for subtraction.
#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap, clippy::cast_sign_loss)]
fn add_sub(mut left: Decimal, mut right: Decimal, sign: u8) -> Result<Decimal, CsError> {
    let sign = sign ^ ((right.sign ^ left.sign) & NEG);
    let mut res = Decimal::ZERO;
    if right.scale == left.scale {
        res.scale = left.scale;
        res.sign = left.sign;
        aligned_add(&left, &right, &mut res, sign)?;
        return Ok(res);
    }
    // Guess that the right operand has the larger scale; the result takes the larger scale.
    res.scale = right.scale;
    res.sign = left.sign;
    let mut scale = i32::from(res.scale) - i32::from(left.scale);
    if scale < 0 {
        scale = -scale;
        res.scale = left.scale;
        res.sign ^= sign;
        std::mem::swap(&mut left, &mut right);
    }
    // `left` is multiplied by 10^scale to reach the scale of `right`: up to 192 bits.
    let mut num = [0u32; 6];
    let mut hi_prod: i32;
    if scale <= POWER10_MAX {
        let pwr = u64::from(POWER10[scale as usize]);
        let t0 = u64::from(left.lo) * pwr;
        let t1 = u64::from(left.mid) * pwr + (t0 >> 32);
        let t2 = u64::from(left.hi) * pwr + (t1 >> 32);
        if t2 >> 32 == 0 {
            let tmp = Decimal { lo: t0 as u32, mid: t1 as u32, hi: t2 as u32, sign: 0, scale: 0 };
            aligned_add(&tmp, &right, &mut res, sign)?;
            return Ok(res);
        }
        num[0] = t0 as u32;
        num[1] = t1 as u32;
        num[2] = t2 as u32;
        num[3] = (t2 >> 32) as u32;
        hi_prod = 3;
    } else {
        num[0] = left.lo;
        num[1] = left.mid;
        num[2] = left.hi;
        hi_prod = 2;
        if num[2] == 0 {
            hi_prod = 1;
            if num[1] == 0 {
                hi_prod = 0;
                if num[0] == 0 {
                    // Left arg is zero, return right.
                    res.set_lo64(right.lo64());
                    res.hi = right.hi;
                    res.sign ^= sign;
                    return Ok(res);
                }
            }
        }
        while scale > 0 {
            let pwr = if scale > POWER10_MAX { TEN_TO_NINE } else { POWER10[scale as usize] };
            let mut carry = 0u32;
            for x in &mut num[..=hi_prod as usize] {
                let t = u64::from(*x) * u64::from(pwr) + u64::from(carry);
                *x = t as u32;
                carry = (t >> 32) as u32;
            }
            if carry != 0 {
                hi_prod += 1;
                num[hi_prod as usize] = carry;
            }
            scale -= POWER10_MAX;
        }
    }
    let tmp64 = u64::from(num[1]) << 32 | u64::from(num[0]);
    if sign != 0 {
        res.set_lo64(tmp64.wrapping_sub(right.lo64()));
        res.hi = num[2].wrapping_sub(right.hi);
        let mut long_sub = false;
        if res.lo64() > tmp64 {
            res.hi = res.hi.wrapping_sub(1);
            if res.hi >= num[2] {
                long_sub = true;
            }
        } else if res.hi > num[2] {
            long_sub = true;
        }
        if long_sub {
            if hi_prod <= 2 {
                // Subtracted in the wrong order: flip.
                sign_flip(&mut res);
            } else {
                let mut cur = 3;
                loop {
                    let old = num[cur];
                    num[cur] = old.wrapping_sub(1);
                    cur += 1;
                    if old != 0 {
                        break;
                    }
                }
                if num[hi_prod as usize] == 0 {
                    hi_prod -= 1;
                }
            }
        }
    } else {
        res.set_lo64(tmp64.wrapping_add(right.lo64()));
        res.hi = num[2].wrapping_add(right.hi);
        let mut long_add = false;
        if res.lo64() < tmp64 {
            res.hi = res.hi.wrapping_add(1);
            if res.hi <= num[2] {
                long_add = true;
            }
        } else if res.hi < num[2] {
            long_add = true;
        }
        if long_add {
            let mut cur = 3usize;
            loop {
                if hi_prod < cur as i32 {
                    num[cur] = 1;
                    hi_prod = cur as i32;
                    break;
                }
                num[cur] = num[cur].wrapping_add(1);
                let wrapped = num[cur] == 0;
                cur += 1;
                if !wrapped {
                    break;
                }
            }
        }
    }
    if hi_prod > 2 {
        num[0] = res.lo;
        num[1] = res.mid;
        num[2] = res.hi;
        let Some(s) = scale_result(&mut num, hi_prod, i32::from(res.scale)) else {
            return Err(CsError::Overflow);
        };
        res.scale = s as u8;
        res.lo = num[0];
        res.mid = num[1];
        res.hi = num[2];
    }
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(m: u64, s: u8) -> Decimal {
        Decimal::lit(m, s)
    }

    #[test]
    fn literals_print_with_their_scale() {
        assert_eq!(d(80, 2).to_string(), "0.80");
        assert_eq!(d(0, 1).to_string(), "0.0");
        assert_eq!(d(1, 0).to_string(), "1");
        assert_eq!(d(1, 8).to_string(), "0.00000001");
        assert_eq!(Decimal::from_parts(5, 0, 0, true, 2).to_string(), "-0.05");
        // negative zero prints unsigned (Mono's NumberFormatter forces _positive for zero)
        assert_eq!(Decimal::from_parts(0, 0, 0, true, 3).to_string(), "0.000");
        assert_eq!(d(80, 2), d(8, 1));
        assert!(d(8, 1) < d(81, 2) && d(81, 2) > d(8, 1) && d(8, 1) <= d(80, 2));
        assert_eq!(Decimal::from_parts(0, 0, 0, true, 0), Decimal::ZERO);
    }

    #[test]
    fn float_conversion_keeps_seven_digits() {
        let bits = |f: f32| Decimal::from_f32(f).unwrap().bits();
        assert_eq!(bits(0.8), [8, 0, 0, 1 << 16]);
        assert_eq!(bits(1.0), [1, 0, 0, 0]);
        // 0.01f is 10 with scale 3: the factoring stops after 4 and 2, as in the C
        assert_eq!(Decimal::from_f32(0.01).unwrap().to_string(), "0.010");
        assert_eq!(Decimal::from_f32(-0.0).unwrap().bits(), [0, 0, 0, 0]);
        assert!(Decimal::from_f32(f32::NAN).is_err());
        assert!(Decimal::from_f32(f32::MAX).is_err());
    }

    #[test]
    fn negative_zero_from_swapped_sub() {
        // The Mono C (`mono_decimal_addsub`) flips the result's sign when it swaps the operands
        // (the smaller scale on the left) before the 96-bit subtraction, so `0.00000m - 0m` has
        // the sign bit set; CoreCLR's managed port returns +0 here. Numerically and in
        // `ToString()` the two are the same zero.
        let z = d(0, 5).checked_sub(d(0, 0)).unwrap();
        assert_eq!(z.bits(), [0, 0, 0, i32::MIN | (5 << 16)]);
        assert_eq!(z.to_string(), "0.00000");
        assert_eq!(z, Decimal::ZERO);
    }

    #[test]
    fn overflow_is_an_error() {
        let max = Decimal::from_parts(u32::MAX, u32::MAX, u32::MAX, false, 0);
        assert_eq!(max.checked_mul(d(2, 0)), Err(CsError::Overflow));
        assert_eq!(max.checked_add(d(1, 0)), Err(CsError::Overflow));
    }

    /// `expr|lo mid hi flags|string`, `expr|OVF` or `expr|cmp:n`; `expr` is RPN with `f:<f32
    /// bits>`, `l:<mantissa>:<scale>` and `w:<lo>:<mid>:<hi>:<scale>:<negative>` operands.
    /// Generated with CoreCLR's `DecCalc` (dotnet 11): `decimal.GetBits` and
    /// `ToString(CultureInfo.InvariantCulture)`.
    fn eval(expr: &str) -> Result<Decimal, CsError> {
        let mut st: Vec<Decimal> = Vec::new();
        for t in expr.split(' ') {
            let p: Vec<&str> = t.split(':').collect();
            let hex = |s: &str| u32::from_str_radix(s, 16).unwrap();
            match p[0] {
                "f" => st.push(Decimal::from_f32(f32::from_bits(hex(p[1])))?),
                "l" => {
                    let m: i64 = p[1].parse().unwrap();
                    let mut x = d(m.unsigned_abs(), p[2].parse().unwrap());
                    x.sign = if m < 0 { NEG } else { 0 };
                    st.push(x);
                }
                "w" => st.push(Decimal::from_parts(
                    hex(p[1]),
                    hex(p[2]),
                    hex(p[3]),
                    p[5] == "1",
                    p[4].parse().unwrap(),
                )),
                "abs" => {
                    let a = st.pop().unwrap();
                    st.push(a.abs());
                }
                op => {
                    let b = st.pop().unwrap();
                    let a = st.pop().unwrap();
                    st.push(match op {
                        "*" => a.checked_mul(b)?,
                        "-" => a.checked_sub(b)?,
                        "+" => a.checked_add(b)?,
                        "cmp" => {
                            let o = a.cmp(&b);
                            Decimal::from_parts(
                                u32::from(o != Ordering::Equal),
                                0,
                                0,
                                o == Ordering::Less,
                                0,
                            )
                        }
                        _ => panic!("op {op}"),
                    });
                }
            }
        }
        Ok(st.pop().unwrap())
    }

    #[test]
    fn matches_coreclr_vectors() {
        let mut n = 0;
        let mut zero_sign_diffs = 0;
        for line in VECTORS.lines() {
            let mut f = line.split('|');
            let expr = f.next().unwrap();
            let want = f.next().unwrap();
            let got = eval(expr);
            if want == "OVF" {
                assert_eq!(got, Err(CsError::Overflow), "{expr}");
            } else if let Some(c) = want.strip_prefix("cmp:") {
                assert_eq!(got.unwrap().to_string(), c, "{expr}");
            } else {
                let got = got.unwrap_or_else(|e| panic!("{expr}: {e}"));
                let b = got.bits();
                let have = format!("{:08x} {:08x} {:08x} {:08x}", b[0], b[1], b[2], b[3]);
                if got.is_zero() && have != want {
                    // The one known CoreCLR/Mono difference (see `negative_zero_from_swapped_sub`).
                    assert_eq!(got.abs().bits(), [0, 0, 0, i32::from(got.scale) << 16], "{expr}");
                    assert!(want.ends_with(&format!("{:04x}0000", got.scale)), "{expr}");
                    zero_sign_diffs += 1;
                } else {
                    assert_eq!(have, want, "{expr}");
                }
                assert_eq!(got.to_string(), f.next().unwrap(), "{expr}");
            }
            n += 1;
        }
        assert!(n > 250);
        assert_eq!(zero_sign_diffs, 1);
    }

    const VECTORS: &str = "\
f:00000000|00000000 00000000 00000000 00000000|0\n\
f:80000000|00000000 00000000 00000000 00000000|0\n\
f:3f800000|00000001 00000000 00000000 00000000|1\n\
f:3f4ccccd|00000008 00000000 00000000 00010000|0.8\n\
f:3a83126f|00000001 00000000 00000000 00030000|0.001\n\
f:3f400000|0000004b 00000000 00000000 00020000|0.75\n\
f:3f8ccccd|0000000b 00000000 00000000 00010000|1.1\n\
f:3f000000|00000005 00000000 00000000 00010000|0.5\n\
f:3dcccccd|00000001 00000000 00000000 00010000|0.1\n\
f:3c23d70a|0000000a 00000000 00000000 00030000|0.010\n\
f:3e800000|00000019 00000000 00000000 00020000|0.25\n\
f:33d6bf95|00000001 00000000 00000000 00070000|0.0000001\n\
f:3fc00000|0000000f 00000000 00000000 00010000|1.5\n\
f:3f7fffff|0098967f 00000000 00000000 00070000|0.9999999\n\
f:3f800001|00000001 00000000 00000000 00000000|1\n\
f:47f1205a|0012d687 00000000 00000000 00010000|123456.7\n\
f:4b18967f|0098967f 00000000 00000000 00000000|9999999\n\
f:4b189680|00989680 00000000 00000000 00000000|10000000\n\
f:4b3c614e|00bc6150 00000000 00000000 00000000|12345680\n\
f:501502f9|540be400 00000002 00000000 00000000|10000000000\n\
f:7f7fc99e|OVF\n\
f:00000001|00000000 00000000 00000000 00000000|0\n\
f:0da24260|00000000 00000000 00000000 00000000|0\n\
f:10fd87b6|00000001 00000000 00000000 001c0000|0.0000000000000000000000000001\n\
f:129e74d2|00000001 00000000 00000000 001b0000|0.000000000000000000000000001\n\
f:6f7f4345|98000000 515792cb ff4344b5 00000000|79000000000000000000000000000\n\
f:6f813f39|OVF\n\
f:000ae398|00000000 00000000 00000000 00000000|0\n\
f:3e99999a|00000003 00000000 00000000 00010000|0.3\n\
f:3e4ccccd|00000002 00000000 00000000 00010000|0.2\n\
f:3f393475|006e6407 00000000 00000000 00070000|0.7234567\n\
f:3901742e|0012d688 00000000 00000000 000a0000|0.0001234568\n\
f:bf000000|00000005 00000000 00000000 80010000|-0.5\n\
f:bf8ccccd|0000000b 00000000 00000000 80010000|-1.1\n\
f:ba83126f|00000001 00000000 00000000 80030000|-0.001\n\
f:42c80000|00000064 00000000 00000000 00000000|100\n\
f:449a522b|0012d688 00000000 00000000 00030000|1234.568\n\
f:ffc00000|OVF\n\
f:7f800000|OVF\n\
f:3f000008|004c4b45 00000000 00000000 00070000|0.5000005\n\
f:3f800004|00000001 00000000 00000000 00000000|1\n\
f:4b000000|00800000 00000000 00000000 00000000|8388608\n\
f:4a800000|00400000 00000000 00000000 00000000|4194304\n\
f:3dfcd6e4|0012d687 00000000 00000000 00070000|0.1234567\n\
f:3dfcd6f2|0012d689 00000000 00000000 00070000|0.1234569\n\
f:40200000|00000019 00000000 00000000 00010000|2.5\n\
f:40600000|00000023 00000000 00000000 00010000|3.5\n\
f:49800004|00100000 00000000 00000000 00000000|1048576\n\
f:4980000c|00100002 00000000 00000000 00000000|1048578\n\
f:3f800001|00000001 00000000 00000000 00000000|1\n\
f:4b800000|01000004 00000000 00000000 00000000|16777220\n\
f:4c000000|01fffffe 00000000 00000000 00000000|33554430\n\
f:3d965e14|00700853 00000000 00000000 00080000|0.07342163\n\
f:3d9e0e2b|0075c2aa 00000000 00000000 00080000|0.07717546\n\
f:3f5a2c83|00820ab1 00000000 00000000 00070000|0.8522417\n\
f:3f0fefd4|0055cb04 00000000 00000000 00070000|0.5622532\n\
f:3f6092c6|0085db2c 00000000 00000000 00070000|0.8772396\n\
f:3f68f853|008adc67 00000000 00000000 00070000|0.9100391\n\
f:3e3aef9d|001bdb0b 00000000 00000000 00070000|0.1825547\n\
f:3f4f4b5c|007b8e9c 00000000 00000000 00070000|0.8097436\n\
f:3e928dc9|00045e1e 00000000 00000000 00060000|0.286238\n\
f:3f0e7db1|0054ee66 00000000 00000000 00070000|0.5566054\n\
f:3e81b0af|0026a693 00000000 00000000 00070000|0.2533011\n\
f:3eda099a|0040faf1 00000000 00000000 00070000|0.4258545\n\
f:3e7347d4|0024406b 00000000 00000000 00070000|0.2375787\n\
f:3cbb87f3|0022ee2d 00000000 00000000 00080000|0.02289197\n\
f:a8a11d1e|001b4b32 00000000 00000000 80140000|-0.00000000000001788722\n\
f:ac7c5f3f|0036b977 00000000 00000000 80120000|-0.000000000003586423\n\
f:571c45bf|c0b89a00 00009c45 00000000 00000000|171823400000000\n\
f:a6a30ece|001143b1 00000000 00000000 80150000|-0.000000000000001131441\n\
f:b6f5d79a|006fcbcc 00000000 00000000 800c0000|-0.000007326668\n\
f:c5267fe8|0028a63a 00000000 00000000 80030000|-2663.994\n\
f:cee1398d|709cc7f8 00000000 00000000 80000000|-1889323000\n\
f:395c2d68|00200a3f 00000000 00000000 000a0000|0.0002099775\n\
f:4bed13e1|01da27c0 00000000 00000000 00000000|31074240\n\
f:411f55f8|0097f458 00000000 00000000 00060000|9.958488\n\
f:00205bed|00000000 00000000 00000000 00000000|0\n\
f:0062a8e1|00000000 00000000 00000000 00000000|0\n\
f:00005481|00000000 00000000 00000000 00000000|0\n\
f:003df0d5|00000000 00000000 00000000 00000000|0\n\
f:003956af|00000000 00000000 00000000 00000000|0\n\
f:003e4455|00000000 00000000 00000000 00000000|0\n\
f:005b14f7|00000000 00000000 00000000 00000000|0\n\
f:0039c27c|00000000 00000000 00000000 00000000|0\n\
f:00228dcc|00000000 00000000 00000000 00000000|0\n\
f:00243aab|00000000 00000000 00000000 00000000|0\n\
f:4d3c0250|0bc02528 00000000 00000000 00000000|197141800\n\
f:31f4c121|006cb15b 00000000 00000000 000f0000|0.000000007123291\n\
f:1e066810|006c929d 00000000 00000000 001b0000|0.000000000000000000007115421\n\
f:4f7619e0|f619e078 00000000 00000000 00000000|4128891000\n\
f:39a489c8|002fe30d 00000000 00000000 000a0000|0.0003138317\n\
f:0cdf948a|00000000 00000000 00000000 00000000|0\n\
f:5529e50b|50666b80 00000a9e 00000000 00000000|11675070000000\n\
f:6c9b668b|cec00000 1373c84f 04db3459 00000000|1502942000000000000000000000\n\
f:1501ab3a|00000106 00000000 00000000 001c0000|0.0000000000000000000000000262\n\
f:2d361645|000fcb25 00000000 00000000 00110000|0.00000000001035045\n\
l:0:0|00000000 00000000 00000000 00000000|0\n\
l:0:1|00000000 00000000 00000000 00010000|0.0\n\
l:1:1|00000001 00000000 00000000 00010000|0.1\n\
l:1:2|00000001 00000000 00000000 00020000|0.01\n\
l:1:3|00000001 00000000 00000000 00030000|0.001\n\
l:1:4|00000001 00000000 00000000 00040000|0.0001\n\
l:1:5|00000001 00000000 00000000 00050000|0.00001\n\
l:1:6|00000001 00000000 00000000 00060000|0.000001\n\
l:1:8|00000001 00000000 00000000 00080000|0.00000001\n\
l:-1:1|00000001 00000000 00000000 80010000|-0.1\n\
l:-5:3|00000005 00000000 00000000 80030000|-0.005\n\
l:80:2|00000050 00000000 00000000 00020000|0.80\n\
l:0:28|00000000 00000000 00000000 001c0000|0.0000000000000000000000000000\n\
l:1:28|00000001 00000000 00000000 001c0000|0.0000000000000000000000000001\n\
l:-0:3|00000000 00000000 00000000 00030000|0.000\n\
w:ffffffff:ffffffff:ffffffff:0:0|ffffffff ffffffff ffffffff 00000000|79228162514264337593543950335\n\
w:ffffffff:ffffffff:ffffffff:28:1|ffffffff ffffffff ffffffff 801c0000|-7.9228162514264337593543950335\n\
w:00000000:00000000:00000000:3:1|00000000 00000000 00000000 80030000|0.000\n\
l:1:0 l:8:1 *|00000008 00000000 00000000 00010000|0.8\n\
l:1:0 l:8:1 * l:1:3 *|00000008 00000000 00000000 00040000|0.0008\n\
l:1:0 l:8:1 * l:1:3 * l:1:8 *|00000008 00000000 00000000 000c0000|0.000000000008\n\
l:1:0 l:8:1 * l:1:3 * l:1:8 * f:3f393475 *|03732038 00000000 00000000 00130000|0.0000000000057876536\n\
l:1:0 l:8:1 * l:1:3 * l:1:8 * f:3f393475 * l:1:8 *|03732038 00000000 00000000 001b0000|0.000000000000000000057876536\n\
f:3f393475 f:3f393475 * f:3f393475 * f:3f393475 * f:3f393475 *|e52e4369 76a05d11 06675144 001c0000|0.1981813192960021095045284713\n\
l:0:1 f:3f4ccccd *|00000000 00000000 00000000 00020000|0.00\n\
l:0:1 l:1:8 *|00000000 00000000 00000000 00090000|0.000000000\n\
l:0:28 l:1:1 *|00000000 00000000 00000000 001c0000|0.0000000000000000000000000000\n\
l:0:20 l:0:20 *|00000000 00000000 00000000 001c0000|0.0000000000000000000000000000\n\
l:-0:2 l:5:0 *|00000000 00000000 00000000 00020000|0.00\n\
l:3:0 l:-0:2 *|00000000 00000000 00000000 00020000|0.00\n\
f:3f66d0f2 f:3cb1c4ca * l:1:2 * l:1:2 * l:1:5 * l:1:2 *|749a7e58 000011cb 00000000 001a0000|0.00000000000019565532315224\n\
f:3f48449c f:3ef74a72 * l:1:3 * l:1:3 *|4e61ad7a 0000225d 00000000 00140000|0.00000037784142327162\n\
f:3e923958 f:3daa91d4 * l:1:6 *|18d4a594 000015a2 00000000 00150000|0.000000023785945474452\n\
f:3e8d5f1b f:3f77c195 * l:1:4 * l:1:3 * l:1:8 *|2e4560cb 0000026e 00000000 001c0000|0.0000000000000002672245956811\n\
f:3f588ebc f:3cbbccd4 * l:1:2 * l:1:8 *|85c7c8d0 000001c3 00000000 00180000|0.000000000001939274713296\n\
f:3f83b166 f:3f1bcb68 * l:1:5 * l:1:5 *|d2d0d647 000005b1 00000000 00170000|0.00000000006261304251975\n\
f:3dfae3e5 f:3e8f9cb9 * l:1:3 * l:1:5 *|013255ae 00000050 00000000 00150000|0.000000000343617459630\n\
f:3f33ed37 f:3dbec1a3 * l:1:8 * l:1:1 * l:1:6 * l:1:8 *|00001992 00000000 00000000 001c0000|0.0000000000000000000000006546\n\
f:3f6f3191 f:3ed4b122 * l:1:3 * l:1:1 *|22b76e5f 0000234d 00000000 00120000|0.000038814201900639\n\
f:3f359f81 f:3f825edf * l:1:2 *|3e9404b4 000000a8 00000000 000e0000|0.00722604393652\n\
f:3f601b00 f:3ee93254 * l:1:6 * l:1:2 * l:1:6 *|55d82344 000003a0 00000000 001b0000|0.000000000000003987169878852\n\
f:3d6bee3d f:3eab9894 * l:1:6 * l:1:6 * l:1:1 * l:1:5 *|0b81a766 00000000 00000000 001c0000|0.0000000000000000000193046374\n\
f:3f185eb4 f:3ba7280b * l:1:4 * l:1:1 * l:1:3 *|3ced6eb7 00001b9d 00000000 00180000|0.000000000030362146008759\n\
f:3f5fe960 f:3f562fcf * l:1:3 * l:1:1 *|d7b20dfc 000006a7 00000000 00110000|0.00007317948075516\n\
w:ffffffff:ffffffff:ffffffff:0:0 l:1:1 *|ffffffff ffffffff ffffffff 00010000|7922816251426433759354395033.5\n\
w:ffffffff:ffffffff:ffffffff:0:0 l:2:0 *|OVF\n\
w:ffffffff:ffffffff:ffffffff:0:0 l:11:1 *|OVF\n\
w:ffffffff:ffffffff:ffffffff:0:0 w:ffffffff:ffffffff:ffffffff:0:0 *|OVF\n\
w:ffffffff:ffffffff:ffffffff:28:0 w:ffffffff:ffffffff:ffffffff:28:0 *|096ee456 359a3b3e cad2f7f5 001b0000|62.771017353866807638357894230\n\
w:ffffffff:ffffffff:ffffffff:14:0 w:ffffffff:ffffffff:ffffffff:14:0 *|OVF\n\
w:ffffffff:ffffffff:ffffffff:1:0 l:99999:5 *|7bdcf02f 53b8e4b8 ffff583a 00010000|7922737023263919495016801489.5\n\
w:12345678:9abcdef0:0fedcba9:10:0 w:00000007:0000000f:00000001:12:1 *|6690b393 487d3888 1d623483 80030000|-9093787999945216830949340.051\n\
w:15f17845:9f00e5a:1c09f7bc:20:0 w:7442800f:2c2bd948:b4e48a6:27:0 *|37993229 20a58696 621b8677 00140000|303628069.54245095386747843113\n\
w:5b6bf96b:0:0:9:0 w:422c785:40e31888:13e7a855:16:1 *|a840ec8a 49d42d85 1e87bbef 80100000|-944864277691.2384352474688650\n\
w:abce1db:3f7091a7:358a3dfb:0:0 w:44e53909:6468c98c:14163a89:0:0 *|OVF\n\
w:5f87b8ed:6d7d5c6c:231b90c0:2:1 w:70c47a53:0:0:5:1 *|OVF\n\
w:69575677:67361863:39fe42e8:7:0 w:72f4fa85:bc4a46e:122ecdf3:4:0 *|OVF\n\
w:f3e4a9d:723cdf7c:38a50e0a:22:1 w:1525b033:333df3e5:19bba70b:19:0 *|f72570a0 32a5cc1c 2d1c9da1 800d0000|-1396141975002289.1608874512544\n\
w:60dec256:2e62cb2a:3cc9bb8d:0:0 w:687b04be:731527fe:a9f61ec:24:0 *|OVF\n\
w:43469d6a:17c546f1:11c364bb:18:1 w:10085ba9:3920949b:200596d2:17:0 *|9ee4ceec 53ad69fe b009ee08 80080000|-544813661377597313417.98461164\n\
w:6a7119d0:1f91d420:36a56cb8:19:1 w:31ddc4:7a7179a9:1cbd1e1f:26:0 *|1c09b018 613162f3 309a7cc6 80110000|-150420442805.47906314016763928\n\
w:6557f241:657560d:22ccdd53:13:0 w:23499faf:314873e8:546e10d:2:0 *|OVF\n\
w:55d8f947:2ef64212:2cc26a92:18:0 w:50a5d20a:72b95db:119acc1e:2:1 *|OVF\n\
w:cc55796:0:0:18:1 w:71bb5ca7:7166e21d:3971a51d:20:0 *|4a085287 4de33e77 013b1523 801c0000|-0.0380911454123189275114099335\n\
w:129817c6:7e72a016:1995de36:28:0 w:5da5130:2aea7bd5:29106b5:6:0 *|218c6f6d 2f83326e cb3955f2 00080000|628947716341378281619.65813613\n\
w:7dd22d05:27f7e19f:d0ba1f5:4:1 w:4b77ed38:752502fc:14e5d3cf:14:0 *|OVF\n\
w:409bf7c3:404c5820:34dc3e95:5:1 w:4949a188:6a06ce5d:9a5806f:0:0 *|OVF\n\
w:3018ea4c:0:0:14:0 w:27178075:0:0:19:0 *|37180c09 000004d0 00000000 001c0000|0.0000000000000005292324031497\n\
w:25bb5635:2098870:c19cd0d:1:1 w:7dcd480f:64cd75f6:14ed36b7:14:0 *|OVF\n\
w:25eef4e5:0:0:21:1 w:78472993:0:0:25:0 *|00000001 00000000 00000000 801c0000|-0.0000000000000000000000000001\n\
w:48b4d783:239d74f3:21923f1:26:1 w:77b6fd42:63f4375a:282ec807:6:0 *|4b4f0c48 cd5f8506 1a17daa8 80050000|-80754481389510713863663.15592\n\
w:725281a7:0:0:27:1 w:6ff27965:410516a:18228ef9:28:0 *|55ebcbc4 00000003 00000000 801c0000|-0.0000000000000000014326418372\n\
l:4294967295:20 l:4294967295:20 *|01197998 00000000 00000000 001c0000|0.0000000000000000000018446744\n\
l:4294967295:25 l:4294967295:25 *|00000000 00000000 00000000 00000000|0\n\
l:4294967295:14 l:4294967295:15 *|66666666 19999999 00000000 001c0000|0.0000000001844674406511961702\n\
l:99999999:28 l:5:1 *|02faf080 00000000 00000000 001c0000|0.0000000000000000000050000000\n\
l:5:28 l:5:1 *|00000002 00000000 00000000 001c0000|0.0000000000000000000000000002\n\
l:15:28 l:5:1 *|00000008 00000000 00000000 001c0000|0.0000000000000000000000000008\n\
l:5:28 l:15:1 *|00000008 00000000 00000000 001c0000|0.0000000000000000000000000008\n\
l:25:28 l:5:1 *|0000000c 00000000 00000000 001c0000|0.0000000000000000000000000012\n\
l:5:20 l:1:9 *|00000000 00000000 00000000 001c0000|0.0000000000000000000000000000\n\
l:15:20 l:1:9 *|00000002 00000000 00000000 001c0000|0.0000000000000000000000000002\n\
l:5:19 l:1:10 *|00000000 00000000 00000000 001c0000|0.0000000000000000000000000000\n\
l:5:28 l:1:19 *|00000000 00000000 00000000 001c0000|0.0000000000000000000000000000\n\
l:5:28 l:1:20 *|00000000 00000000 00000000 00000000|0\n\
f:3f800000 f:3f4ccccd -|00000002 00000000 00000000 00010000|0.2\n\
f:3f400000 f:3ba3d70a -|000002e9 00000000 00000000 00030000|0.745\n\
f:3f4ccccd f:3f800000 -|00000002 00000000 00000000 80010000|-0.2\n\
f:3f4ccccd f:3f4ccccd -|00000000 00000000 00000000 00010000|0.0\n\
f:bf4ccccd f:bf4ccccd -|00000000 00000000 00000000 80010000|0.0\n\
l:1:0 l:1:28 -|0fffffff 3e250261 204fce5e 001c0000|0.9999999999999999999999999999\n\
l:1:28 l:1:0 -|0fffffff 3e250261 204fce5e 801c0000|-0.9999999999999999999999999999\n\
l:0:3 l:1:1 -|00000064 00000000 00000000 80030000|-0.100\n\
l:1:0 l:0:5 -|000186a0 00000000 00000000 00050000|1.00000\n\
l:0:5 l:0:0 -|00000000 00000000 00000000 00050000|0.00000\n\
l:-1:0 l:1:0 -|00000002 00000000 00000000 80000000|-2\n\
l:-1:0 l:-1:0 -|00000000 00000000 00000000 80000000|0\n\
l:1:0 l:-1:0 -|00000002 00000000 00000000 00000000|2\n\
l:-1:3 l:1:0 -|000003e9 00000000 00000000 80030000|-1.001\n\
l:5:0 l:5:0 -|00000000 00000000 00000000 00000000|0\n\
l:1:1 l:100:2 -|0000005a 00000000 00000000 80020000|-0.90\n\
l:5:1 l:-5:1 -|0000000a 00000000 00000000 00010000|1.0\n\
w:ffffffff:ffffffff:ffffffff:0:0 l:1:28 -|ffffffff ffffffff ffffffff 00000000|79228162514264337593543950335\n\
w:ffffffff:ffffffff:ffffffff:0:0 l:-1:28 -|ffffffff ffffffff ffffffff 00000000|79228162514264337593543950335\n\
w:ffffffff:ffffffff:ffffffff:0:0 l:-1:0 -|OVF\n\
w:ffffffff:ffffffff:ffffffff:0:0 l:-5:1 -|OVF\n\
w:ffffffff:ffffffff:ffffffff:0:0 l:-5:1 +|fffffffe ffffffff ffffffff 00000000|79228162514264337593543950334\n\
w:ffffffff:ffffffff:ffffffff:1:0 l:-5:1 +|fffffffa ffffffff ffffffff 00010000|7922816251426433759354395033.0\n\
w:ffffffff:ffffffff:ffffffff:1:0 l:6:1 +|9999999a 99999999 19999999 00000000|7922816251426433759354395034\n\
l:1:28 w:ffffffff:ffffffff:ffffffff:0:0 -|ffffffff ffffffff ffffffff 80000000|-79228162514264337593543950335\n\
l:1:28 w:ffffffff:ffffffff:ffffffff:0:1 -|ffffffff ffffffff ffffffff 00000000|79228162514264337593543950335\n\
l:1:12 w:ffffffff:ffffffff:ffffffff:0:0 -|ffffffff ffffffff ffffffff 80000000|-79228162514264337593543950335\n\
l:0:28 w:ffffffff:ffffffff:ffffffff:0:0 -|ffffffff ffffffff ffffffff 80000000|-79228162514264337593543950335\n\
l:0:28 w:ffffffff:ffffffff:ffffffff:0:0 +|ffffffff ffffffff ffffffff 00000000|79228162514264337593543950335\n\
l:5:28 w:ffffffff:ffffffff:ffffffff:0:0 +|ffffffff ffffffff ffffffff 00000000|79228162514264337593543950335\n\
w:6beb351b:14c48880:2be6cdee:20:0 w:bf3c8ac:40a6e26f:3f5f6aae:2:1 -|35cb21f2 40a6e272 3f5f6aae 00020000|196129073568474419538309370.74\n\
w:8eb215a:0:0:15:1 w:441a7be8:0:0:24:1 +|9a8f7fe8 0213952b 00000000 80180000|-0.000000149627227142586344\n\
w:70ae3f10:0:0:7:1 w:1df6467a:1df29578:29bb035c:24:1 -|4f56467a b8e9ab21 291ea326 00180000|12725.923637300938378195650170\n\
w:1974da7e:7629d57b:30dd3a8f:3:1 w:2c64ac05:0:0:21:0 +|1974da7e 7629d57b 30dd3a8f 80030000|-15122729621214376621493246.590\n\
w:58006eae:fb6ada0:7a96fde:3:0 w:399ef05e:1d3441cb:291def17:5:1 -|8f61046f 53419ba4 50baf695 00040000|2498482544487482674964024.6383\n\
w:61a0b740:5b90a600:74924f7:14:1 w:1b0a92ec:0:0:9:1 +|641012c0 5b90cf43 074924f7 800e0000|-22548212217184.54266760270528\n\
w:32db1497:0:0:25:0 w:37d53bbe:154c460e:cc8345:17:1 -|cf4b598d 51cb5d8d 4fe346fc 00130000|2472407715.7065996102881991053\n\
w:555d3182:0:0:19:1 w:6068b338:5b6e792b:1b54fb34:9:1 +|6068b338 5b6e792b 1b54fb34 80090000|-8458831313830174352.187372344\n\
w:16a7eb7b:1ed99626:167dbdd1:22:1 w:650c915c:0:0:4:1 -|4ab7eb7b 6925df16 11036752 80160000|-526535.9868379166908442209147\n\
w:d956dfa:0:0:23:0 w:5cdb9908:7d855b84:224af70d:15:1 +|5cdb9906 7d855b84 224af70d 800f0000|-10613117517950.892706857982214\n\
w:1b9b3b41:0:0:7:1 w:73f9a30d:0:0:5:1 -|31e875d3 0000002d 00000000 00070000|19411.0846419\n\
w:1bc6eeed:4c6790b0:29cf5ee7:11:1 w:c681386:14017e71:3269044a:1:1 +|5f8418e1 25f68e7f 3269044a 80010000|-1560120795938731293405951408.1\n\
w:303fb4b1:20ff0648:3a79bf00:13:0 w:2a52c9d5:739aaed7:20dfc34:3:1 -|1877017e f7edff05 cd768486 00050000|635877061021949256956425.67038\n\
w:2445e0a3:4000fac9:d5344d6:5:0 w:5c2462df:0:0:1:0 +|719c0793 400108d8 0d5344d6 00050000|41239710438415442437574.51155\n\
w:44d31154:1a9b51e6:273e9cfe:18:0 w:28885c27:0:0:6:1 -|7bb88154 f7d9e3fa 273e9d22 00120000|12145610840.435230191792980308\n\
w:cc29157:6f42286b:3a097acf:5:1 w:16964d77:4495f7b6:25251f36:0:0 +|ab7d48ca 451ea72d 2524f92d 00000000|11495643397234214104205183178\n\
w:358f05a0:647ef993:18c89e4c:9:1 w:f7e8a4e:5c51e770:1053d747:7:0 -|204c8136 720c89bc a5c0df04 80080000|-512981935476186962279.86374966\n\
w:5c2bf4f3:5a582c1a:320131f1:21:1 w:4b1252b0:0:0:3:0 +|3f6bf4f3 5aa8f3f1 2def5e22 80150000|-14216203.249019852195122377971\n\
w:625d32e4:3503b837:25f70295:16:0 w:71e9d3ac:0:0:7:0 -|e7df7ae4 1a7df3e2 25f70295 00100000|1174956223604.1363643936242404\n\
w:df318c8:0:0:19:0 w:62d1424:5772a7be:342573a6:22:0 +|83c5e164 5772a7f4 342573a6 00160000|1613849.6906642014595407274340\n\
w:27222e1d:71e70679:85d81b7:12:1 w:3d4a03e:0:0:6:1 -|fd08229d 71e6cc05 085d81b7 800c0000|-2588922749032848.433147814557\n\
w:745885f2:0:0:11:1 w:221c6af:7c8d8461:3daa7653:25:1 +|18f246af 1328d4bc 3daa9fa9 80190000|-1908.4856963587648072541423279\n\
w:6a6813c3:9194366:229cd9a9:17:1 w:754bdfd4:7a042379:27fcd413:24:0 -|19584bbd 1fa59793 229cd9ec 80110000|-107121118710.24394040736500669\n\
w:1e64d87:0:0:13:0 w:2448b4c9:50a8eb5e:cd0a57:9:1 +|2c61c195 81fbf0c6 50180a1b 800b0000|-247878627364716198.68771336597\n\
w:63a68539:2d7fc18d:fb07486:5:0 w:67dc79ba:99f8fc2:2e0edd8b:6:1 -|4c5dadf4 d09d1f48 caf36ac8 00060000|62810245233306263222774.246900\n\
w:51f957bb:1e08eab7:1656a993:25:1 w:31adf3a7:9d07cc4:383af482:3:1 +|31b88037 09d07cc4 383af482 80030000|-17402432903738724439981326.391\n\
w:6c5694be:60e4f931:1f49eb7b:24:0 w:7e3b50b5:54de9c86:2cfda544:19:0 -|d1a9e02c f3508f52 2cfd90c2 80130000|-1392388228.1476238668875817004\n\
w:2b6cccf2:0:0:8:1 w:1227a9bf:465a1864:2e8ed778:11:1 +|b3283b0f 465a190d 2e8ed778 800b0000|-144089954456396482.00205286159\n\
w:3bd54188:9e3958f:21e46ca1:7:1 w:4668a728:f06d2ff:20822cbd:14:1 -|5f034531 7f98bde1 21e46c6a 80070000|-1048915239110585186958.2542129\n\
w:335fa1c3:3c56ee73:1ffc0f09:14:1 w:6a3a3133:1aaee2a7:3646d5b9:11:0 +|af0a397c 7cabb9f7 363ea594 000b0000|167879258607400154.75609188732\n\
w:1a80807d:0:0:27:1 w:77a8d304:2a1bd0a1:ec7667b:6:0 -|ac983e28 a516264e 93ca00cf 80070000|-4573850328967164103880.1354280\n\
w:250fb7a9:34b04167:26f16080:16:1 w:e75f55e:4af4778a:3bf4bbe4:24:0 +|bfa1879d 259926e1 26f16076 80100000|-1205223702234.5633828178069405\n\
w:17c4eac5:76a9a4bf:1b63e65c:20:0 w:672d8e04:4533a57:2767d60b:4:0 -|091d5b7e 04533992 2767d60b 80040000|-1219544553210593949874161.9582\n\
w:43dae6dc:7af8ca64:c198ec4:3:0 w:4b6b3c5c:0:0:5:0 +|ae17bdd4 cdb7e7ea 78ff93ac 00040000|3744717463809967289605552.4820\n\
w:174736ca:0:0:26:0 w:47044d24:1f652fff:61c76a4:1:0 -|c62b0368 39f3dff8 3d1ca269 80020000|-189132024865055389702031286.80\n\
w:3c745c4f:28abb2d0:16c06fb9:7:0 w:151c7866:71e2d7d4:1ad7c3ce:0:1 +|995902cb 461a7307 1ad7c3a8 80000000|-8307453272140757431951622859\n\
w:2362d863:0:0:22:0 w:32a1e3ac:2fb5711:19cb3bc8:12:1 -|32a1e3ac 02fb5711 19cb3bc8 000c0000|7982819496101561.630168572844\n\
w:7f039b2f:155e8636:2fb136fa:25:0 w:64a7a7fc:47c85e05:3c0d2686:11:0 +|470ea379 47c8e443 3c0d2686 000b0000|185849985518983843.75997178745\n\
w:72611b91:1b2f340c:26bbf578:0:1 w:1cc1073:59886487:365727b9:1:0 -|f28f1d36 a4233e19 2c2b130a 80000000|-13669414163640592666326932790\n\
w:67e50447:0:0:7:1 w:6cc34c52:93d9638:37015e2b:0:1 +|6cc34d00 093d9638 37015e2b 80000000|-17023329162318831814206246144\n\
w:29a817a0:0:0:17:0 w:8095cea:378e7815:69d97ec:15:1 -|548809e7 2b90b0d2 4227ef3a 00100000|2047428847381.3510551694215655\n\
w:9e5038:4e32e710:165616c6:16:0 w:7ba6f4e5:57ea8052:2f9a6f3e:7:0 +|492ced84 b7d981a0 2f9a6f3e 00070000|1473249537744936452788.9837444\n\
w:7521b2c3:4a6133c4:21cd75e3:16:1 w:5b0511b2:0:0:20:1 -|751f5e41 4a6133c4 21cd75e3 80100000|-1046139182677.4408679709957697\n\
w:a1ff21e:6c0ded44:38c38e35:14:0 w:53a29a53:4a3d985d:26269a57:10:1 +|cd2ed621 49b13410 26252655 800a0000|-1180534165174653188.5248861729\n\
w:1462e9f9:19b21d37:1e4a53e4:23:1 w:d96a152:16183c05:307620a1:12:0 -|23829853 176547c1 307620a1 800c0000|-14998087805478169.767659608147\n\
w:2300a679:7ac5eb7e:a85b026:13:0 w:12e217e5:0:0:20:1 +|5e067f7d cbbb32ed 6938e180 000e0000|325646907854619.21960291630973\n\
w:4412f4f2:409e4048:14ac1875:26:1 w:447e5ff:14f7461f:259befa2:3:1 -|0446ec15 14f7461f 259befa2 00030000|11639460500902734150085569.557\n\
w:35fe49cb:0:0:23:0 w:57850993:2d3a18bd:30a30ac1:3:0 +|57850993 2d3a18bd 30a30ac1 00030000|15052386167167127258514393.491\n\
f:3f5486a6 f:3bdda678 - l:1:8 *|3114524b 00000000 00000000 00110000|0.00000000823415371\n\
f:3f5486a6 f:3bdda678 f:3f5486a6 * -|d7e4c48c 001d4b5d 00000000 00100000|0.8245640750744716\n\
f:3ecf9582 f:3ae69999 - l:1:8 *|180fa3d2 00000000 00000000 00110000|0.00000000403678162\n\
f:3ecf9582 f:3ae69999 f:3ecf9582 * -|f4c6b15a 000e60f1 00000000 00100000|0.4047241983996250\n\
f:3e2151b0 f:3b8471f9 - l:1:8 *|09262ae5 00000000 00000000 00110000|0.00000000153496293\n\
f:3e2151b0 f:3b8471f9 f:3e2151b0 * -|a516335e 00059302 00000000 00100000|0.1569014452466526\n\
f:3e6197d9 f:3ae152b7 - l:1:8 *|0d075e61 00000000 00000000 00110000|0.00000000218586721\n\
f:3e6197d9 f:3ae152b7 f:3e6197d9 * -|a0f767e2 0007d039 00000000 00100000|0.2199270769256418\n\
f:3f73e03d f:3bc2fae6 - l:1:8 *|05a488d0 00000000 00000000 00100000|0.0000000094669008\n\
f:3f73e03d f:3bc2fae6 f:3f73e03d * -|12cde6a0 00035d44 00000000 000f0000|0.946971884775072\n\
f:3f6d5e52 f:3a57a9ee - l:1:8 *|282d165f 00000002 00000000 00120000|0.000000009263978079\n\
f:3f6d5e52 f:3a57a9ee f:3f6d5e52 * -|dd94ff2b 014924d5 00000000 00110000|0.92645768301969195\n\
f:3f814b67 f:3bda59fc - l:1:8 *|3bcf7049 00000000 00000000 00110000|0.00000001003450441\n\
f:3f814b67 f:3bda59fc f:3f814b67 * -|520434b2 00039092 00000000 000f0000|1.003383045764274\n\
f:3f3c5e26 f:3c1eac09 - l:1:8 *|2b47d1b5 00000000 00000000 00110000|0.00000000726127029\n\
f:3f3c5e26 f:3c1eac09 f:3f3c5e26 * -|53ad73b4 0019e35b 00000000 00100000|0.7286855803171764\n\
f:3f06928f f:3bbaad94 - l:1:8 *|1efe3786 00000000 00000000 00110000|0.00000000519976838\n\
f:3f06928f f:3bbaad94 f:3f06928f * -|02d8543c 001291bd 00000000 00100000|0.5226790563370044\n\
f:3e810ffd f:3ba56ae7 - l:1:8 *|0eb9550c 00000000 00000000 00110000|0.00000000247026956\n\
f:3e810ffd f:3ba56ae7 f:3e810ffd * -|48053650 0008e909 00000000 00100000|0.2508025885963856\n\
f:3e1cc2f5 f:3bc27326 - l:1:8 *|08c561d3 00000000 00000000 00110000|0.00000000147153363\n\
f:3e1cc2f5 f:3bc27326 f:3e1cc2f5 * -|7ad34f1d 0005680f 00000000 00100000|0.1521790578020125\n\
f:3f658381 f:3c122ed6 - l:1:8 *|34e7f00d 00000000 00000000 00110000|0.00000000887615501\n\
f:3f658381 f:3c122ed6 f:3f658381 * -|31258b8a 001f9136 00000000 00100000|0.8885386216835978\n\
f:3f7ba111 f:3b00d049 - l:1:8 *|05d8d3d6 00000000 00000000 00100000|0.0000000098096086\n\
f:3f7ba111 f:3b00d049 f:3f7ba111 * -|8fdca060 00037c35 00000000 000f0000|0.980994418843744\n\
f:3f1750ca f:3c092202 - l:1:8 *|22bb658f 00000000 00000000 00110000|0.00000000582706575\n\
f:3f1750ca f:3c092202 f:3f1750ca * -|77dda25f 0014d2d0 00000000 00100000|0.5861292340257375\n\
l:-5:2 abs|00000005 00000000 00000000 00020000|0.05\n\
l:-0:3 abs|00000000 00000000 00000000 00030000|0.000\n\
f:bf4ccccd abs|00000008 00000000 00000000 00010000|0.8\n\
l:80:2 l:8:1 cmp|cmp:0\n\
l:8:1 l:80:2 cmp|cmp:0\n\
l:1:0 l:2:0 cmp|cmp:-1\n\
l:-1:0 l:1:0 cmp|cmp:-1\n\
l:-3:1 l:-2:1 cmp|cmp:-1\n\
l:0:3 w:0:0:0:0:1 cmp|cmp:0\n\
l:-5:1 l:0:0 cmp|cmp:-1\n\
w:ffffffff:ffffffff:ffffffff:0:0 l:1:28 cmp|cmp:1\n\
l:1:28 w:ffffffff:ffffffff:ffffffff:0:0 cmp|cmp:-1\n\
w:ffffffff:ffffffff:ffffffff:0:1 l:1:28 cmp|cmp:-1\n\
w:ffffffff:ffffffff:ffffffff:1:0 w:ffffffff:ffffffff:ffffffff:0:0 cmp|cmp:-1\n\
f:3ed258b6 f:3eb2b7e9 cmp|cmp:1\n\
f:3f5fa4cb f:3f289bb4 cmp|cmp:1\n\
f:3f4fe0d5 f:3e1978cc cmp|cmp:1\n\
f:3e25a8e3 f:3f4bfe33 cmp|cmp:-1\n\
f:3ee1e182 f:3ea7b2bb cmp|cmp:1\n\
f:3f8bc274 f:3f4f05a1 cmp|cmp:1\n\
f:3e32aaed f:3f396ed5 cmp|cmp:-1\n\
f:3e1d3073 f:3e0077a7 cmp|cmp:1\n\
f:3e0bdf46 f:3e0bdf46 cmp|cmp:0\n\
f:3e4cfbd6 f:3e4cfbd6 cmp|cmp:0
";
}
