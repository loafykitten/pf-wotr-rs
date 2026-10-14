//! Analysis of Wwise output captures (WAV): loudness over time and stereo balance.
//!
//! Nobody listens during the spike, so the capture is the proof of audio: it must be
//! non-silent, its audible span must match the cue's duration, and the left/right balance
//! must follow a moving source.

use std::path::Path;

use anyhow::{Context, Result, ensure};

/// A decoded capture, as interleaved `f32` samples.
#[derive(Debug, Clone)]
pub struct Capture {
    /// Sample rate, Hz.
    pub sample_rate: u32,
    /// Channel count.
    pub channels: u16,
    /// Interleaved samples, full scale = 1.0.
    pub samples: Vec<f32>,
}

/// Loudness of one analysis window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    /// Window start, seconds from the capture start.
    pub start: f64,
    /// RMS of the left channel (channel 0), dBFS.
    pub left_db: f64,
    /// RMS of the right channel (channel 1, or channel 0 for mono), dBFS.
    pub right_db: f64,
}

impl Window {
    /// Louder channel's level, dBFS.
    #[must_use]
    pub fn level_db(&self) -> f64 {
        self.left_db.max(self.right_db)
    }

    /// Right minus left, dB (positive = panned right).
    #[must_use]
    pub fn balance_db(&self) -> f64 {
        self.right_db - self.left_db
    }
}

/// Silence floor for dB conversion.
pub const FLOOR_DB: f64 = -120.0;

fn to_db(rms: f64) -> f64 {
    if rms <= 0.0 { FLOOR_DB } else { (20.0 * rms.log10()).max(FLOOR_DB) }
}

impl Capture {
    /// Reads a WAV file (16/24/32-bit integer or 32-bit float).
    ///
    /// # Errors
    ///
    /// If the file can't be read or decoded.
    pub fn load(path: &Path) -> Result<Self> {
        let mut r =
            hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
        let spec = r.spec();
        let samples = match spec.sample_format {
            hound::SampleFormat::Float => r.samples::<f32>().collect::<Result<Vec<_>, _>>()?,
            hound::SampleFormat::Int => {
                let scale = f32::powi(2.0, i32::from(spec.bits_per_sample) - 1);
                r.samples::<i32>()
                    .map(|s| s.map(|v| v as f32 / scale))
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        Ok(Self { sample_rate: spec.sample_rate, channels: spec.channels, samples })
    }

    /// Frames (samples per channel).
    #[must_use]
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }

    /// Duration, seconds.
    #[must_use]
    pub fn duration(&self) -> f64 {
        self.frames() as f64 / f64::from(self.sample_rate)
    }

    /// Peak absolute sample, dBFS.
    #[must_use]
    pub fn peak_db(&self) -> f64 {
        to_db(self.samples.iter().fold(0f32, |m, s| m.max(s.abs())).into())
    }

    /// Largest absolute sample between `start` and `end` seconds (all channels). A NaN or
    /// infinite sample counts as infinitely loud, so it can't pass a silence check.
    #[must_use]
    pub fn peak_between(&self, start: f64, end: f64) -> f32 {
        let ch = usize::from(self.channels.max(1));
        let frame =
            |t: f64| ((t.max(0.0) * f64::from(self.sample_rate)) as usize).min(self.frames());
        self.samples[frame(start) * ch..frame(end) * ch]
            .iter()
            .fold(0f32, |m, s| if s.is_finite() { m.max(s.abs()) } else { f32::INFINITY })
    }

    /// RMS loudness per window of `seconds`.
    ///
    /// # Errors
    ///
    /// If the window is shorter than one frame.
    pub fn windows(&self, seconds: f64) -> Result<Vec<Window>> {
        let ch = usize::from(self.channels.max(1));
        let len = (seconds * f64::from(self.sample_rate)) as usize;
        ensure!(len > 0, "window shorter than a frame");
        let right = if ch > 1 { 1 } else { 0 };
        Ok(self
            .samples
            .chunks(len * ch)
            .enumerate()
            .map(|(i, chunk)| {
                let n = (chunk.len() / ch).max(1) as f64;
                let (mut l, mut r) = (0f64, 0f64);
                for frame in chunk.chunks_exact(ch) {
                    l += f64::from(frame[0]).powi(2);
                    r += f64::from(frame[right]).powi(2);
                }
                Window {
                    start: (i * len) as f64 / f64::from(self.sample_rate),
                    left_db: to_db((l / n).sqrt()),
                    right_db: to_db((r / n).sqrt()),
                }
            })
            .collect())
    }

    /// First and last window louder than `threshold_db`, as `(start, end)` seconds.
    ///
    /// # Errors
    ///
    /// As [`Self::windows`].
    pub fn audible_span(&self, window: f64, threshold_db: f64) -> Result<Option<(f64, f64)>> {
        let w = self.windows(window)?;
        let first = w.iter().find(|w| w.level_db() > threshold_db);
        let last = w.iter().rev().find(|w| w.level_db() > threshold_db);
        Ok(first.zip(last).map(|(a, b)| (a.start, b.start + window)))
    }
}

#[cfg(test)]
mod tests {
    use super::Capture;

    fn tone(rate: u32, seconds: f64, left: f32, right: f32, silence_before: f64) -> Capture {
        let mut samples = Vec::new();
        let pad = (silence_before * f64::from(rate)) as usize;
        samples.resize(pad * 2, 0.0);
        let n = (seconds * f64::from(rate)) as usize;
        for i in 0..n {
            let s = (i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin();
            samples.push(s * left);
            samples.push(s * right);
        }
        Capture { sample_rate: rate, channels: 2, samples }
    }

    #[test]
    fn measures_span_and_balance() {
        let c = tone(48_000, 2.0, 0.1, 0.5, 1.0);
        assert!((c.duration() - 3.0).abs() < 1e-9);
        let (start, end) = c.audible_span(0.1, -60.0).unwrap().unwrap();
        assert!((start - 1.0).abs() < 0.11 && (end - 3.0).abs() < 0.11, "{start} {end}");
        let w = c.windows(0.5).unwrap();
        let loud = w.last().unwrap();
        // 0.5 vs 0.1 amplitude = +14 dB to the right.
        assert!((loud.balance_db() - 13.98).abs() < 0.1, "{}", loud.balance_db());
        assert!((c.peak_db() - 20.0 * 0.5f64.log10()).abs() < 0.01);
        assert_eq!(c.peak_between(0.0, 0.9), 0.0);
        assert!(c.peak_between(1.5, 2.0) > 0.4);
        let mut nan = c.clone();
        nan.samples[10] = f32::NAN;
        assert_eq!(nan.peak_between(0.0, 0.9), f32::INFINITY);
    }

    #[test]
    fn silence_has_no_span() {
        let c = Capture { sample_rate: 48_000, channels: 2, samples: vec![0.0; 9600] };
        assert_eq!(c.audible_span(0.1, -60.0).unwrap(), None);
    }

    #[test]
    fn reads_float_wav() {
        let dir = std::env::temp_dir().join(format!("wwise-spike-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for _ in 0..480 {
            w.write_sample(0.25f32).unwrap();
            w.write_sample(-0.25f32).unwrap();
        }
        w.finalize().unwrap();
        let c = Capture::load(&path).unwrap();
        assert_eq!((c.channels, c.frames()), (2, 480));
        assert!((c.peak_db() - 20.0 * 0.25f64.log10()).abs() < 1e-3);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
