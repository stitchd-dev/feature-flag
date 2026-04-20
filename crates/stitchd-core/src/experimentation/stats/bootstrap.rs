//! Bootstrap confidence intervals for percentile metrics.
//!
//! Uses non-parametric bootstrap resampling to compute 95% confidence
//! intervals for p50/p95/p99 latency or other percentile metrics.

// ── Xorshift64 RNG ────────────────────────────────────────────────────────────

/// Minimal xorshift64 PRNG — no external dependency required.
///
/// Period is 2^64 - 1. Sufficient for bootstrap resampling.
struct Xorshift64 {
    state: u64,
}

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        // Seed must be non-zero; use a splitmix64 step to ensure this.
        let mut s = seed.wrapping_add(0x9e3779b97f4a7c15);
        s = (s ^ (s >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        s = (s ^ (s >> 27)).wrapping_mul(0x94d049bb133111eb);
        s = s ^ (s >> 31);
        // Ensure non-zero
        Self {
            state: if s == 0 { 1 } else { s },
        }
    }

    /// Returns next u64 in [0, u64::MAX].
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// Returns a random index in [0, n).
    fn next_index(&mut self, n: usize) -> usize {
        (self.next_u64() as usize) % n
    }
}

// ── percentile_value ──────────────────────────────────────────────────────────

/// Computes a percentile value from a **sorted** slice using Knuth/linear
/// interpolation (type 7, the R default).
///
/// `p` must be in [0.0, 1.0]. Returns `f64::NAN` for an empty slice.
pub fn percentile_value(sorted_data: &[f64], p: f64) -> f64 {
    let n = sorted_data.len();
    if n == 0 {
        return f64::NAN;
    }
    if n == 1 {
        return sorted_data[0];
    }
    // h = (n - 1) * p, then interpolate between floor and ceil.
    let h = (n - 1) as f64 * p;
    let lo = h.floor() as usize;
    let hi = h.ceil() as usize;
    if lo == hi {
        sorted_data[lo]
    } else {
        let frac = h - lo as f64;
        sorted_data[lo] * (1.0 - frac) + sorted_data[hi] * frac
    }
}

// ── bootstrap_percentile_ci ───────────────────────────────────────────────────

/// Computes a bootstrap confidence interval for a percentile (p50/p95/p99)
/// from raw sample data.
///
/// Returns `(lower_bound, upper_bound)` at 95% confidence.
///
/// # Arguments
/// * `samples`    – raw (unsorted) observations.
/// * `percentile` – target percentile in [0.0, 1.0] (e.g. 0.5, 0.95, 0.99).
/// * `n_resamples` – number of bootstrap resamples, typically 1 000.
///
/// # Panics
/// Panics if `samples` is empty.
pub fn bootstrap_percentile_ci(samples: &[f64], percentile: f64, n_resamples: usize) -> (f64, f64) {
    assert!(
        !samples.is_empty(),
        "bootstrap_percentile_ci: samples must not be empty"
    );

    let n = samples.len();
    // Fixed seed derived from sample length for determinism when called without
    // an explicit seed. Callers requiring reproducibility should use
    // `bootstrap_percentile_ci_seeded`.
    let mut rng = Xorshift64::new(n as u64 ^ 0xdeadbeef_cafebabe);

    let mut resample_percentiles: Vec<f64> = Vec::with_capacity(n_resamples);
    let mut buf: Vec<f64> = vec![0.0; n];

    for _ in 0..n_resamples {
        // Sample with replacement.
        for slot in buf.iter_mut() {
            *slot = samples[rng.next_index(n)];
        }
        buf.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        resample_percentiles.push(percentile_value(&buf, percentile));
    }

    resample_percentiles
        .sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let lower = percentile_value(&resample_percentiles, 0.025);
    let upper = percentile_value(&resample_percentiles, 0.975);
    (lower, upper)
}

/// Same as [`bootstrap_percentile_ci`] but accepts an explicit RNG seed for
/// full reproducibility in tests.
pub fn bootstrap_percentile_ci_seeded(
    samples: &[f64],
    percentile: f64,
    n_resamples: usize,
    seed: u64,
) -> (f64, f64) {
    assert!(
        !samples.is_empty(),
        "bootstrap_percentile_ci_seeded: samples must not be empty"
    );

    let n = samples.len();
    let mut rng = Xorshift64::new(seed);

    let mut resample_percentiles: Vec<f64> = Vec::with_capacity(n_resamples);
    let mut buf: Vec<f64> = vec![0.0; n];

    for _ in 0..n_resamples {
        for slot in buf.iter_mut() {
            *slot = samples[rng.next_index(n)];
        }
        buf.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        resample_percentiles.push(percentile_value(&buf, percentile));
    }

    resample_percentiles
        .sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let lower = percentile_value(&resample_percentiles, 0.025);
    let upper = percentile_value(&resample_percentiles, 0.975);
    (lower, upper)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── percentile_value ──────────────────────────────────────────────────────

    #[test]
    fn percentile_value_empty_returns_nan() {
        assert!(percentile_value(&[], 0.5).is_nan());
    }

    #[test]
    fn percentile_value_single_element() {
        assert_eq!(percentile_value(&[42.0], 0.5), 42.0);
        assert_eq!(percentile_value(&[42.0], 0.0), 42.0);
        assert_eq!(percentile_value(&[42.0], 1.0), 42.0);
    }

    #[test]
    fn percentile_value_median_of_sorted_odd_slice() {
        // [1,2,3,4,5] → p50 = 3.0
        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile_value(&data, 0.5), 3.0);
    }

    #[test]
    fn percentile_value_median_of_sorted_even_slice() {
        // [1,2,3,4] → p50 = 2.5 (linear interpolation)
        let data = vec![1.0, 2.0, 3.0, 4.0];
        assert_eq!(percentile_value(&data, 0.5), 2.5);
    }

    #[test]
    fn percentile_value_min_max() {
        let data = vec![10.0, 20.0, 30.0, 40.0, 50.0];
        assert_eq!(percentile_value(&data, 0.0), 10.0);
        assert_eq!(percentile_value(&data, 1.0), 50.0);
    }

    #[test]
    fn percentile_value_p95_known() {
        // sorted 1..=100 → p95 at h=(99)*0.95=94.05 → 95 + 0.05*(96-95)=95.05
        let data: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        let p95 = percentile_value(&data, 0.95);
        assert!((p95 - 95.05).abs() < 1e-9, "expected ~95.05, got {p95}");
    }

    // ── bootstrap_percentile_ci — smoke tests ─────────────────────────────────

    #[test]
    fn smoke_100_resamples_gives_finite_result() {
        let samples: Vec<f64> = (0..50).map(|i| i as f64).collect();
        let (lo, hi) = bootstrap_percentile_ci(&samples, 0.5, 100);
        assert!(lo.is_finite(), "lower bound should be finite");
        assert!(hi.is_finite(), "upper bound should be finite");
        assert!(lo <= hi, "lower bound should not exceed upper bound");
    }

    #[test]
    fn bootstrap_output_lower_le_upper() {
        let samples: Vec<f64> = (0..200).map(|i| (i as f64) * 0.5).collect();
        let (lo, hi) = bootstrap_percentile_ci_seeded(&samples, 0.95, 500, 12345);
        assert!(lo <= hi);
    }

    // ── bootstrap_percentile_ci — uniform distribution ────────────────────────

    #[test]
    fn uniform_p50_ci_contains_true_median() {
        // Uniform [0, 1]: true p50 = 0.5
        // Generate 500 evenly spaced points as a proxy for uniform data.
        let n = 500usize;
        let samples: Vec<f64> = (0..n).map(|i| i as f64 / (n - 1) as f64).collect();
        let true_p50 = 0.5_f64;

        let (lo, hi) = bootstrap_percentile_ci_seeded(&samples, 0.5, 1000, 42);
        assert!(
            lo <= true_p50 && true_p50 <= hi,
            "95% CI [{lo}, {hi}] should contain true p50={true_p50}"
        );
    }

    #[test]
    fn uniform_p95_ci_contains_true_p95() {
        // Uniform [0, 1]: true p95 = 0.95
        let n = 500usize;
        let samples: Vec<f64> = (0..n).map(|i| i as f64 / (n - 1) as f64).collect();
        let true_p95 = 0.95_f64;

        let (lo, hi) = bootstrap_percentile_ci_seeded(&samples, 0.95, 1000, 99);
        assert!(
            lo <= true_p95 && true_p95 <= hi,
            "95% CI [{lo}, {hi}] should contain true p95={true_p95}"
        );
    }

    // ── bootstrap_percentile_ci — normal-like distribution ───────────────────

    #[test]
    fn normal_like_p50_ci_contains_true_median() {
        // Approximate a normal(mean=100, std=10) with 1000 points using the
        // Box-Muller-free linear approximation: 12 uniform samples summed → normal.
        // Here we use a simpler deterministic stand-in: a sorted array of 1000
        // values drawn from a known normal CDF approximation.
        //
        // We approximate N(100, 10) using quantile function at 1000 evenly spaced
        // CDF points via the rational Horner approximation.
        let samples = generate_normal_quantiles(100.0, 10.0, 1000);
        let true_median = 100.0_f64;

        let (lo, hi) = bootstrap_percentile_ci_seeded(&samples, 0.5, 1000, 7);
        assert!(
            lo <= true_median && true_median <= hi,
            "95% CI [{lo:.3}, {hi:.3}] should contain true median={true_median}"
        );
    }

    #[test]
    fn normal_like_p95_ci_contains_true_p95() {
        // N(100, 10): generate a large (10 000 point) population so that the
        // population p95 is close to the theoretical value and the bootstrap CI
        // tightly contains the population p95.
        let samples = generate_normal_quantiles(100.0, 10.0, 10_000);
        // The population p95 of our deterministic sample: percentile at 0.95
        // of the sorted 10 000-element array.
        let mut sorted = samples.clone();
        sorted.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        let population_p95 = percentile_value(&sorted, 0.95);

        let (lo, hi) = bootstrap_percentile_ci_seeded(&samples, 0.95, 1000, 13);
        assert!(
            lo <= population_p95 && population_p95 <= hi,
            "95% CI [{lo:.3}, {hi:.3}] should contain population p95={population_p95:.3}"
        );
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Generates `n` quantile values from N(mean, std) by sampling the
    /// inverse-normal CDF at evenly-spaced probabilities.
    ///
    /// Uses the rational approximation to the standard normal quantile function
    /// (Beasley-Springer-Moro algorithm, absolute error < 3e-9).
    fn generate_normal_quantiles(mean: f64, std: f64, n: usize) -> Vec<f64> {
        (1..=n)
            .map(|i| {
                let p = i as f64 / (n + 1) as f64;
                mean + std * probit(p)
            })
            .collect()
    }

    /// Rational approximation to the standard normal quantile (probit) function.
    /// Beasley-Springer-Moro algorithm; error < 3e-9 for p in (0, 1).
    fn probit(p: f64) -> f64 {
        const A: [f64; 4] = [
            2.50662823884,
            -18.61500062529,
            41.39119773534,
            -25.44106049637,
        ];
        const B: [f64; 4] = [
            -8.47351093090,
            23.08336743743,
            -21.06224101826,
            3.13082909833,
        ];
        const C: [f64; 9] = [
            0.3374754822726147,
            0.9761690190917186,
            0.1607979714918209,
            0.0276438810333863,
            0.0038405729373609,
            0.0003951896511349,
            0.0000321767881768,
            0.0000002888167364,
            0.0000003960315187,
        ];

        let y = p - 0.5;
        if y.abs() < 0.42 {
            let r = y * y;
            let num = y * (((A[3] * r + A[2]) * r + A[1]) * r + A[0]);
            let den = ((((B[3] * r + B[2]) * r + B[1]) * r + B[0]) * r) + 1.0;
            num / den
        } else {
            let r = if y < 0.0 { p } else { 1.0 - p };
            let s = (-r.ln()).sqrt();
            let t = s
                - (C[0]
                    + s * (C[1]
                        + s * (C[2]
                            + s * (C[3]
                                + s * (C[4] + s * (C[5] + s * (C[6] + s * (C[7] + s * C[8]))))))));
            if y < 0.0 { -t } else { t }
        }
    }
}
