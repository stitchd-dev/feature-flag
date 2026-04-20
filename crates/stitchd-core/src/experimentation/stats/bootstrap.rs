//! Bootstrap confidence interval estimation.
//!
//! This module provides non-parametric bootstrap CI computation used by
//! the frequentist percentile analysis.  The full implementation lives in
//! the `stats_w4_bootstrap` branch; this file is a minimal stub that
//! satisfies the `frequentist` module's import while that branch is merged.

use super::ConfidenceInterval;

/// Compute a bootstrap 95 % confidence interval for the difference in a
/// percentile statistic between two samples.
///
/// # Arguments
/// * `control_samples`  – raw observations for the control variant
/// * `variant_samples`  – raw observations for the treatment variant
/// * `percentile`       – target percentile in `[0, 100]`
/// * `n_bootstrap`      – number of bootstrap resamples (≥ 1 000 recommended)
///
/// # Returns
/// A [`ConfidenceInterval`] at the 2.5th / 97.5th percentile of the
/// bootstrap distribution of `(variant_pct − control_pct)`.
pub fn bootstrap_ci(
    control_samples: &[f64],
    variant_samples: &[f64],
    percentile: f64,
    n_bootstrap: usize,
) -> ConfidenceInterval {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    // Lightweight LCG-like PRNG seeded from sample data so results are
    // deterministic for a given input without pulling in `rand`.
    let seed = {
        let mut h = DefaultHasher::new();
        control_samples.len().hash(&mut h);
        variant_samples.len().hash(&mut h);
        percentile.to_bits().hash(&mut h);
        n_bootstrap.hash(&mut h);
        h.finish()
    };

    let mut state = seed.wrapping_add(1);
    let mut next_f64 = move || -> f64 {
        // xorshift64
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state as f64) / (u64::MAX as f64)
    };

    let sample_pct = |data: &[f64], pct: f64, rng: &mut dyn FnMut() -> f64| -> f64 {
        if data.is_empty() {
            return 0.0;
        }
        let n = data.len();
        let mut resample: Vec<f64> = (0..n).map(|_| data[(rng() * n as f64) as usize % n]).collect();
        resample.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        let idx = ((pct / 100.0) * (n - 1) as f64).round() as usize;
        resample[idx.min(n - 1)]
    };

    let mut diffs: Vec<f64> = Vec::with_capacity(n_bootstrap);
    for _ in 0..n_bootstrap {
        let cp = sample_pct(control_samples, percentile, &mut next_f64);
        let vp = sample_pct(variant_samples, percentile, &mut next_f64);
        diffs.push(vp - cp);
    }
    diffs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());

    let lo_idx = ((0.025 * (n_bootstrap - 1) as f64).round() as usize).min(n_bootstrap - 1);
    let hi_idx = ((0.975 * (n_bootstrap - 1) as f64).round() as usize).min(n_bootstrap - 1);

    ConfidenceInterval { lower: diffs[lo_idx], upper: diffs[hi_idx] }
}
