//! `System.TimeSpan` as Mono implements it: 100 ns ticks, and `FromSeconds` (via
//! `Interval`) **rounds to whole milliseconds**, half away from zero:
//! `(long)(value * 1000 + (value >= 0 ? 0.5 : -0.5)) * 10000`, throwing on NaN and on
//! overflow. The game advances `Player.GameTime` by `TimeSpan.FromSeconds(DeltaTime)` each
//! frame (`TimeController.Tick`), so every frame's game-time step is a whole number of
//! milliseconds.

use super::arith::CsError;

pub const TICKS_PER_MILLISECOND: i64 = 10_000;
pub const TICKS_PER_SECOND: i64 = 10_000_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeSpan(pub i64);

impl TimeSpan {
    pub const ZERO: Self = Self(0);
    pub const MAX: Self = Self(i64::MAX);

    #[must_use]
    pub fn ticks(self) -> i64 {
        self.0
    }

    /// `TimeSpan.FromSeconds(double)`.
    ///
    /// # Errors
    ///
    /// NaN (`ArgumentException`, reported as overflow here) or out of range.
    pub fn from_seconds(value: f64) -> Result<Self, CsError> {
        Self::interval(value, 1000.0)
    }

    /// `TimeSpan.FromMilliseconds(double)`.
    ///
    /// # Errors
    ///
    /// As [`Self::from_seconds`].
    pub fn from_milliseconds(value: f64) -> Result<Self, CsError> {
        Self::interval(value, 1.0)
    }

    #[allow(clippy::cast_possible_truncation)]
    fn interval(value: f64, scale: f64) -> Result<Self, CsError> {
        if value.is_nan() {
            return Err(CsError::Overflow);
        }
        let num = value * scale + if value >= 0.0 { 0.5 } else { -0.5 };
        if !(-922_337_203_685_477.0..=922_337_203_685_477.0).contains(&num) {
            return Err(CsError::Overflow);
        }
        Ok(Self((num as i64) * TICKS_PER_MILLISECOND))
    }

    /// `t1 + t2`, throwing on overflow (`TimeSpan.Add`).
    ///
    /// # Errors
    ///
    /// [`CsError::Overflow`].
    pub fn checked_add(self, o: Self) -> Result<Self, CsError> {
        self.0.checked_add(o.0).map(Self).ok_or(CsError::Overflow)
    }

    /// `TotalSeconds`.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn total_seconds(self) -> f64 {
        self.0 as f64 * 1e-7
    }

    /// The constant ("c") format the saves use, `[-][d.]hh:mm:ss[.fffffff]`.
    #[must_use]
    pub fn to_c_format(self) -> String {
        wr_save::decode::fmt_timespan(self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_seconds_rounds_to_milliseconds() {
        // 1/80 s = 12.5 ms rounds up to 13 ms; 1/60 s = 16.67 ms rounds to 17 ms.
        assert_eq!(TimeSpan::from_seconds(f64::from(1.0f32 / 80.0)).unwrap().0, 13 * 10_000);
        assert_eq!(TimeSpan::from_seconds(1.0 / 60.0).unwrap().0, 17 * 10_000);
        assert_eq!(TimeSpan::from_seconds(0.0004).unwrap().0, 0);
        assert_eq!(TimeSpan::from_seconds(-0.0125).unwrap().0, -13 * 10_000);
        assert!(TimeSpan::from_seconds(f64::NAN).is_err());
        assert!(TimeSpan::from_seconds(1e300).is_err());
        assert!(TimeSpan::MAX.checked_add(TimeSpan(1)).is_err());
    }
}
