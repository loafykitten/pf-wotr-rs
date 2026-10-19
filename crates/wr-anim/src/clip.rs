//! Unity generic `AnimationClip`s: transform curves stored as a streamed clip (cubic keyframes),
//! a dense clip (uniform samples) and a constant clip, bound to bones by the CRC-32 of their
//! hierarchy path.
//!
//! A clip's curves are numbered in one space: the streamed curves first, then the dense ones,
//! then the constant ones. The clip's `genericBindings` list its properties in the same order,
//! each taking as many consecutive curves as it has components (position and scale 3, rotation
//! quaternion 4, euler angles 3, any other property 1).
//!
//! Sampling a clip at a time gives one float per curve:
//! - streamed: the latest key at or before the time, evaluated as the cubic
//!   `((c0 t + c1) t + c2) t + c3` of the time since that key (the key's coefficients describe the
//!   segment up to the curve's next key; `c3` is the key's value);
//! - dense: linear interpolation between the two samples around `(time - begin) * rate`;
//! - constant: the stored value.
//!
//! `docs/poc7/animation.md` records how this was checked.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// `GenericBinding.attribute` of a transform: local position.
pub const ATTR_POSITION: u32 = 1;
/// Local rotation as a quaternion (x, y, z, w).
pub const ATTR_ROTATION: u32 = 2;
/// Local scale.
pub const ATTR_SCALE: u32 = 3;
/// Local rotation as euler angles in degrees.
pub const ATTR_EULER: u32 = 4;

/// Unity class id of `Transform`, the only `GenericBinding.typeID` the pose uses.
pub const TYPE_TRANSFORM: u32 = 4;

/// One animated property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    /// CRC-32 of the bone's path from the animator's root (0: the root itself).
    pub path: u32,
    /// `ATTR_*` for a transform; other properties' attribute hashes are kept but unused.
    pub attribute: u32,
    /// Unity class id of the animated component (`TYPE_TRANSFORM`).
    pub type_id: u32,
}

impl Binding {
    /// How many curves the property takes.
    #[must_use]
    pub fn components(&self) -> usize {
        if self.type_id != TYPE_TRANSFORM {
            return 1;
        }
        match self.attribute {
            ATTR_ROTATION => 4,
            ATTR_POSITION | ATTR_SCALE | ATTR_EULER => 3,
            _ => 1,
        }
    }
}

/// One key of a streamed curve: its time and the cubic of the segment that starts here.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Key {
    pub time: f32,
    pub coeff: [f32; 4],
}

/// The streamed curves, one key list per curve, sorted by time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Streamed {
    pub curves: Vec<Vec<Key>>,
}

/// The dense curves: `frames` samples of `curves` values each, frame-major, at `rate` Hz from
/// `begin`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Dense {
    pub begin: f32,
    pub rate: f32,
    pub frames: u32,
    pub curves: u32,
    pub samples: Vec<f32>,
}

/// A decoded clip.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub name: String,
    /// `m_StartTime` and `m_StopTime`.
    pub start: f32,
    pub stop: f32,
    /// `m_LoopTime`: the clip wraps instead of holding its last pose.
    pub looping: bool,
    pub bindings: Vec<Binding>,
    pub streamed: Streamed,
    pub dense: Dense,
    pub constant: Vec<f32>,
}

/// The words of Unity's `StreamedClip.data`: frames of `time, key count, keys`, each key
/// `curve index, c0, c1, c2, c3`, until the data ends.
///
/// # Errors
///
/// Data that ends inside a frame or a key, or a curve index past `curve_count`.
pub fn decode_streamed(words: &[u32], curve_count: usize) -> Result<Streamed> {
    let mut curves: Vec<Vec<Key>> = vec![Vec::new(); curve_count];
    let mut i = 0;
    while i < words.len() {
        ensure!(i + 2 <= words.len(), "streamed clip: truncated frame header at word {i}");
        let time = f32::from_bits(words[i]);
        let n = words[i + 1] as usize;
        i += 2;
        ensure!(i + n * 5 <= words.len(), "streamed clip: truncated frame at word {i}");
        for k in 0..n {
            let w = &words[i + k * 5..i + k * 5 + 5];
            let c = w[0] as usize;
            ensure!(c < curve_count, "streamed clip: curve {c} of {curve_count}");
            curves[c].push(Key {
                time,
                coeff: [
                    f32::from_bits(w[1]),
                    f32::from_bits(w[2]),
                    f32::from_bits(w[3]),
                    f32::from_bits(w[4]),
                ],
            });
        }
        i += n * 5;
    }
    Ok(Streamed { curves })
}

impl Streamed {
    /// The curve's value at `t`: the segment of the latest key at or before `t` (the first key's
    /// segment before it), as a cubic of the time since that key. A key before every sample time
    /// (Unity stores the initial values at `-f32::MAX`) is evaluated at zero elapsed time.
    #[must_use]
    pub fn eval(&self, curve: usize, t: f32) -> f32 {
        let keys = &self.curves[curve];
        let Some(first) = keys.first() else { return 0.0 };
        let idx = keys.partition_point(|k| k.time <= t).saturating_sub(1);
        let k = keys.get(idx).unwrap_or(first);
        let dt = if k.time < -1.0e30 { 0.0 } else { t - k.time };
        let c = k.coeff;
        ((c[0] * dt + c[1]) * dt + c[2]) * dt + c[3]
    }
}

impl Dense {
    /// The curve's value at `t` (linear between the neighbouring samples, held at the ends).
    #[must_use]
    pub fn eval(&self, curve: usize, t: f32) -> f32 {
        if self.frames == 0 {
            return 0.0;
        }
        let f = ((t - self.begin) * self.rate).max(0.0);
        let last = self.frames as usize - 1;
        let i0 = (f.floor() as usize).min(last);
        let i1 = (i0 + 1).min(last);
        let w = (f - i0 as f32).clamp(0.0, 1.0);
        let at = |i: usize| self.samples[i * self.curves as usize + curve];
        at(i0) + (at(i1) - at(i0)) * w
    }
}

impl Clip {
    /// Total curves: streamed, dense and constant.
    #[must_use]
    pub fn curve_count(&self) -> usize {
        self.streamed.curves.len() + self.dense.curves as usize + self.constant.len()
    }

    /// Checks the clip's internal consistency (what a reader must guarantee before sampling).
    ///
    /// # Errors
    ///
    /// Bindings that need more curves than the clip has, or a dense table of the wrong size.
    pub fn validate(&self) -> Result<()> {
        let need: usize = self.bindings.iter().map(Binding::components).sum();
        ensure!(
            need == self.curve_count(),
            "{}: bindings need {need} curves, the clip has {}",
            self.name,
            self.curve_count()
        );
        ensure!(
            self.dense.samples.len() == self.dense.frames as usize * self.dense.curves as usize,
            "{}: dense table of {} values for {} x {}",
            self.name,
            self.dense.samples.len(),
            self.dense.frames,
            self.dense.curves
        );
        ensure!(self.stop >= self.start, "{}: stops before it starts", self.name);
        ensure!(
            self.start.is_finite() && self.stop.is_finite(),
            "{}: start or stop isn't a number",
            self.name
        );
        let finite = self.constant.iter().all(|v| v.is_finite())
            && self.dense.samples.iter().all(|v| v.is_finite())
            && self.dense.begin.is_finite()
            && self.dense.rate.is_finite()
            && self.streamed.curves.iter().flatten().all(|k| {
                // The initial-value key sits at -f32::MAX, which is finite.
                k.time.is_finite() && k.coeff.iter().all(|c| c.is_finite())
            });
        ensure!(finite, "{}: a curve value or coefficient isn't a number", self.name);
        Ok(())
    }

    /// How far the streamed curves are from being continuous: over every curve and every segment
    /// between two keys, the distance between the value the segment's cubic reaches at the next key
    /// and that key's own value. Unity's streamed clips store exact cubic Hermite segments, so a
    /// right reading of the coefficients has an error at float rounding level; a wrong one does not.
    /// Segments ending at a stepped key (zero cubic: held until the next key) are skipped.
    #[must_use]
    pub fn continuity_error(&self) -> f32 {
        let mut worst = 0.0f32;
        for keys in &self.streamed.curves {
            for w in keys.windows(2) {
                let (a, b) = (&w[0], &w[1]);
                if a.time < -1.0e30 || a.coeff[..3] == [0.0; 3] {
                    continue;
                }
                let dt = b.time - a.time;
                let c = a.coeff;
                let end = ((c[0] * dt + c[1]) * dt + c[2]) * dt + c[3];
                worst = worst.max((end - b.coeff[3]).abs());
            }
        }
        worst
    }

    /// Clip length in seconds.
    #[must_use]
    pub fn length(&self) -> f32 {
        self.stop - self.start
    }

    /// The clip time a playable at `time` seconds shows: wrapped into the clip when it loops,
    /// held at its ends otherwise.
    #[must_use]
    pub fn local_time(&self, time: f32) -> f32 {
        let len = self.length();
        if len <= 0.0 {
            return self.start;
        }
        if self.looping {
            self.start + time.rem_euclid(len)
        } else {
            self.start + time.clamp(0.0, len)
        }
    }

    /// Every curve's value at clip time `t`, into `out` (resized to the curve count).
    pub fn sample(&self, t: f32, out: &mut Vec<f32>) {
        out.clear();
        let ns = self.streamed.curves.len();
        for c in 0..ns {
            out.push(self.streamed.eval(c, t));
        }
        for c in 0..self.dense.curves as usize {
            out.push(self.dense.eval(c, t));
        }
        out.extend_from_slice(&self.constant);
    }

    /// Each binding's first curve index, in binding order.
    #[must_use]
    pub fn binding_offsets(&self) -> Vec<usize> {
        let mut o = 0;
        self.bindings
            .iter()
            .map(|b| {
                let at = o;
                o += b.components();
                at
            })
            .collect()
    }
}

/// Unity's CRC-32 of a bone path (`zlib`'s polynomial 0xEDB88320, initial value and final xor
/// 0xFFFFFFFF), which `GenericBinding.path` stores.
#[must_use]
pub fn path_hash(path: &str) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in path.as_bytes() {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(f: f32) -> u32 {
        f.to_bits()
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(path_hash("123456789"), 0xCBF4_3926);
        assert_eq!(path_hash(""), 0);
    }

    /// One curve: a key at -inf for the initial value, keys at 0 and 1 (a cubic segment 0 -> 1 on
    /// the way, then a held value).
    fn words() -> Vec<u32> {
        vec![
            // frame at -f32::MAX: curve 0, constant 5
            bits(-f32::MAX),
            1,
            0,
            bits(0.0),
            bits(0.0),
            bits(0.0),
            bits(5.0),
            // frame at 0: curve 0, v(t) = 2 t^3 - 3 t^2 + 5 (value 5 at 0, 4 at 1)... c3 = 5
            bits(0.0),
            1,
            0,
            bits(2.0),
            bits(-3.0),
            bits(0.0),
            bits(5.0),
            // frame at 1: curve 0 holds 4
            bits(1.0),
            1,
            0,
            bits(0.0),
            bits(0.0),
            bits(0.0),
            bits(4.0),
        ]
    }

    #[test]
    fn streamed_curves_are_cubic_per_segment() {
        let s = decode_streamed(&words(), 1).unwrap();
        assert_eq!(s.curves[0].len(), 3);
        assert_eq!(s.eval(0, -10.0), 5.0);
        assert_eq!(s.eval(0, 0.0), 5.0);
        assert!((s.eval(0, 0.5) - (2.0 * 0.125 - 3.0 * 0.25 + 5.0)).abs() < 1e-6);
        // The segment ends at the next key's value: continuity.
        assert!((s.eval(0, 0.999_999) - 4.0).abs() < 1e-4);
        assert_eq!(s.eval(0, 1.0), 4.0);
        assert_eq!(s.eval(0, 7.0), 4.0);
    }

    #[test]
    fn continuity_error_flags_a_wrong_reading() {
        let good = Clip {
            name: "c".into(),
            stop: 1.0,
            streamed: decode_streamed(&words(), 1).unwrap(),
            ..Clip::default()
        };
        assert!(good.continuity_error() < 1e-5, "{}", good.continuity_error());
        // The same key with another cubic doesn't reach the next key's value.
        let mut bad = good.clone();
        bad.streamed.curves[0][1].coeff = [3.0, 0.0, 0.0, 5.0];
        assert!(bad.continuity_error() > 1.0);
    }

    #[test]
    fn truncated_or_out_of_range_streams_are_errors() {
        let mut w = words();
        w.pop();
        assert!(decode_streamed(&w, 1).is_err());
        assert!(decode_streamed(&words(), 0).is_err());
    }

    #[test]
    fn dense_curves_interpolate_and_hold() {
        let d = Dense {
            begin: 0.0,
            rate: 2.0,
            frames: 3,
            curves: 2,
            samples: vec![0.0, 10.0, 1.0, 20.0, 5.0, 30.0],
        };
        assert_eq!(d.eval(0, 0.0), 0.0);
        assert_eq!(d.eval(0, 0.25), 0.5);
        assert_eq!(d.eval(1, 0.75), 25.0);
        assert_eq!(d.eval(0, 1.0), 5.0);
        assert_eq!(d.eval(0, 9.0), 5.0);
        assert_eq!(d.eval(0, -1.0), 0.0);
    }

    #[test]
    fn bindings_number_the_curves_in_order() {
        let b = |attribute, type_id| Binding { path: 1, attribute, type_id };
        let clip = Clip {
            name: "c".into(),
            stop: 1.0,
            bindings: vec![b(ATTR_POSITION, 4), b(ATTR_ROTATION, 4), b(77, 137), b(ATTR_SCALE, 4)],
            streamed: Streamed { curves: vec![Vec::new(); 3] },
            dense: Dense { curves: 4, frames: 1, samples: vec![0.0; 4], ..Dense::default() },
            constant: vec![0.0; 4],
            ..Clip::default()
        };
        clip.validate().unwrap();
        assert_eq!(clip.binding_offsets(), vec![0, 3, 7, 8]);
        let mut short = clip.clone();
        short.constant.pop();
        assert!(short.validate().is_err());
    }

    #[test]
    fn non_finite_curve_values_are_rejected() {
        let good = Clip {
            name: "c".into(),
            stop: 1.0,
            bindings: vec![Binding { path: 1, attribute: ATTR_POSITION, type_id: 4 }],
            constant: vec![0.0, 1.0, 2.0],
            ..Clip::default()
        };
        good.validate().unwrap();
        let mut c = good.clone();
        c.constant[1] = f32::NAN;
        assert!(c.validate().is_err());
        let mut d = good.clone();
        d.constant.clear();
        d.dense = Dense {
            rate: 30.0,
            frames: 1,
            curves: 3,
            samples: vec![0.0, f32::INFINITY, 0.0],
            ..Dense::default()
        };
        assert!(d.validate().is_err());
        let mut s = good.clone();
        s.constant.clear();
        s.streamed = decode_streamed(&words(), 1).unwrap();
        s.bindings = vec![Binding { path: 1, attribute: 77, type_id: 137 }];
        s.validate().unwrap();
        s.streamed.curves[0][1].coeff[2] = f32::NAN;
        assert!(s.validate().is_err());
        let mut t = good;
        t.stop = f32::NAN;
        assert!(t.validate().is_err());
    }

    #[test]
    fn looping_wraps_and_one_shots_hold() {
        let mut c = Clip { stop: 2.0, looping: true, ..Clip::default() };
        assert!((c.local_time(5.5) - 1.5).abs() < 1e-6);
        c.looping = false;
        assert_eq!(c.local_time(5.5), 2.0);
        assert_eq!(c.local_time(-1.0), 0.0);
    }
}
