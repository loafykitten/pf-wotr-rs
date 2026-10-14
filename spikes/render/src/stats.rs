//! Small statistics helpers shared by the experiments (percentiles, summaries, CSV).

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

/// Summary of a sample of durations in milliseconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub n: usize,
    pub mean: f64,
    pub min: f64,
    pub p01: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub p999: f64,
    pub max: f64,
    pub stddev: f64,
}

/// Nearest-rank percentile (`q` in 0..=1) of an already sorted slice.
pub fn percentile_sorted(sorted: &[f64], q: f64) -> f64 {
    assert!(!sorted.is_empty(), "percentile of an empty sample");
    let q = q.clamp(0.0, 1.0);
    // Nearest-rank: the smallest value with at least q of the sample at or below it.
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Summarise a sample. Returns `None` for an empty sample. NaNs are dropped.
pub fn summarize(sample: &[f64]) -> Option<Summary> {
    let mut v: Vec<f64> = sample.iter().copied().filter(|x| !x.is_nan()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    let mean = v.iter().sum::<f64>() / n as f64;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n as f64;
    Some(Summary {
        n,
        mean,
        min: v[0],
        p01: percentile_sorted(&v, 0.01),
        p50: percentile_sorted(&v, 0.50),
        p95: percentile_sorted(&v, 0.95),
        p99: percentile_sorted(&v, 0.99),
        p999: percentile_sorted(&v, 0.999),
        max: v[n - 1],
        stddev: var.sqrt(),
    })
}

/// Successive differences (`x[i+1] - x[i]`).
pub fn intervals(times: &[f64]) -> Vec<f64> {
    times.windows(2).map(|w| w[1] - w[0]).collect()
}

/// Count values strictly above `threshold`.
pub fn count_above(sample: &[f64], threshold: f64) -> usize {
    sample.iter().filter(|&&x| x > threshold).count()
}

impl Summary {
    pub const CSV_HEADER: &'static str = "n,mean,min,p01,p50,p95,p99,p999,max,stddev";

    pub fn csv_row(&self) -> String {
        format!(
            "{},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4},{:.4}",
            self.n,
            self.mean,
            self.min,
            self.p01,
            self.p50,
            self.p95,
            self.p99,
            self.p999,
            self.max,
            self.stddev
        )
    }

    /// One human-readable line, values in ms.
    pub fn line(&self) -> String {
        let mut s = String::new();
        let _ = write!(
            s,
            "n={} mean={:.3} p01={:.3} p50={:.3} p95={:.3} p99={:.3} p99.9={:.3} max={:.3} sd={:.3}",
            self.n,
            self.mean,
            self.p01,
            self.p50,
            self.p95,
            self.p99,
            self.p999,
            self.max,
            self.stddev
        );
        s
    }
}

/// Write `header` and `rows` to a CSV file, creating parent directories.
pub fn write_csv(path: &Path, header: &str, rows: &[String]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    writeln!(f, "{header}")?;
    for r in rows {
        writeln!(f, "{r}")?;
    }
    f.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_rank_percentiles() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile_sorted(&v, 0.5), 50.0);
        assert_eq!(percentile_sorted(&v, 0.99), 99.0);
        assert_eq!(percentile_sorted(&v, 1.0), 100.0);
        assert_eq!(percentile_sorted(&v, 0.0), 1.0);
        assert_eq!(percentile_sorted(&[7.0], 0.999), 7.0);
    }

    #[test]
    fn summary_basics() {
        let s = summarize(&[4.0, 2.0, f64::NAN, 6.0]).unwrap();
        assert_eq!(s.n, 3);
        assert_eq!(s.min, 2.0);
        assert_eq!(s.max, 6.0);
        assert!((s.mean - 4.0).abs() < 1e-12);
        assert!(summarize(&[]).is_none());
    }

    #[test]
    fn intervals_and_counts() {
        let iv = intervals(&[0.0, 4.0, 8.5, 12.0]);
        assert_eq!(iv, vec![4.0, 4.5, 3.5]);
        assert_eq!(count_above(&iv, 4.0), 1);
    }
}
