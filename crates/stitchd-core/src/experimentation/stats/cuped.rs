//! CUPED — Controlled-experiment Using Pre-Experiment Data.
//!
//! Variance-reduction technique: adjust each post-period metric value `Y` by
//! subtracting a pre-period covariate term:
//!
//! ```text
//!     Y' = Y − θ · (X_pre − mean(X_pre))
//!     θ  = cov(Y, X_pre) / var(X_pre)
//! ```
//!
//! When `Y` and `X_pre` are correlated, `Y'` has lower variance than `Y` while
//! preserving the expectation. The standard A/B-test pipeline then operates
//! on `Y'` instead of `Y` to tighten confidence intervals.
//!
//! See Deng et al. 2013, "Improving the sensitivity of online controlled
//! experiments by utilizing pre-experiment data".
//!
//! ## Edge cases
//! - **Empty observations** → returns `theta=0`, `variance_reduction_pct=0`,
//!   empty `adjusted_values`. No panic.
//! - **`var(X_pre) = 0`** (constant pre-period) → `theta=0`, `Y' = Y`,
//!   `variance_reduction_pct=0`. CUPED degenerates to a no-op.
//! - **Negative variance reduction** (covariate uncorrelated or anti-correlated
//!   with `Y` in finite samples) → returned as-is. Callers decide whether to
//!   fall back to raw `Y` based on the sign of `variance_reduction_pct`.

use serde::{Deserialize, Serialize};

// ── Public types ─────────────────────────────────────────────────────────────

/// One CUPED datapoint — the post-period metric value `Y` and its
/// pre-period covariate `X_pre` for the same unit (context).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CupedObservation {
    /// Post-period metric value.
    pub y: f64,
    /// Pre-period covariate value for the same context.
    pub x_pre: f64,
}

/// Output of [`apply_cuped`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CupedResult {
    /// Adjusted post-period values `Y'` in the same order as the input.
    pub adjusted_values: Vec<f64>,
    /// Adjustment coefficient `θ = cov(Y, X_pre) / var(X_pre)`. Zero when
    /// `var(X_pre) = 0` or the input is empty.
    pub theta: f64,
    /// `(var(Y) − var(Y')) / var(Y) * 100`, in percent. Positive when CUPED
    /// reduced variance. Can be negative if the covariate is uncorrelated or
    /// anti-correlated with `Y` in finite samples — callers should fall back
    /// to raw `Y` in that case. Zero when there is no data or `var(Y) = 0`.
    pub variance_reduction_pct: f64,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Apply CUPED variance reduction to a set of post/pre observation pairs.
///
/// See the module-level documentation for the algorithm and edge-case
/// behaviour.
pub fn apply_cuped(observations: &[CupedObservation]) -> CupedResult {
    if observations.is_empty() {
        return CupedResult {
            adjusted_values: vec![],
            theta: 0.0,
            variance_reduction_pct: 0.0,
        };
    }

    let n = observations.len() as f64;
    let mean_y: f64 = observations.iter().map(|o| o.y).sum::<f64>() / n;
    let mean_x: f64 = observations.iter().map(|o| o.x_pre).sum::<f64>() / n;

    // Compute cov(Y, X_pre) and var(X_pre) via sample formulas (n-1
    // denominator). Cancels in θ — the denominator drops out — but we still
    // need the centered sums.
    let mut sum_cov = 0.0_f64;
    let mut sum_var_x = 0.0_f64;
    for o in observations {
        let dy = o.y - mean_y;
        let dx = o.x_pre - mean_x;
        sum_cov += dy * dx;
        sum_var_x += dx * dx;
    }

    let theta = if sum_var_x == 0.0 {
        0.0
    } else {
        sum_cov / sum_var_x
    };

    // Y' = Y - θ * (X_pre - mean(X_pre))
    let adjusted_values: Vec<f64> = observations
        .iter()
        .map(|o| o.y - theta * (o.x_pre - mean_x))
        .collect();

    // Sample variance of Y and Y'. Use n-1 denominator for consistency
    // (cancels in the percentage ratio, but matches standard sample-variance
    // convention).
    let var_y = sample_variance(&observations.iter().map(|o| o.y).collect::<Vec<_>>());
    let var_y_prime = sample_variance(&adjusted_values);

    let variance_reduction_pct = if var_y == 0.0 {
        0.0
    } else {
        (var_y - var_y_prime) / var_y * 100.0
    };

    CupedResult {
        adjusted_values,
        theta,
        variance_reduction_pct,
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Sample variance with `n-1` denominator. Returns 0 for slices of length < 2.
fn sample_variance(xs: &[f64]) -> f64 {
    if xs.len() < 2 {
        return 0.0;
    }
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let sum_sq: f64 = xs.iter().map(|x| (x - mean).powi(2)).sum();
    sum_sq / (n - 1.0)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal seeded LCG so tests get deterministic correlated draws without
    /// pulling in `rand` from a worktree dep.
    struct Lcg {
        state: u64,
    }
    impl Lcg {
        fn new(seed: u64) -> Self {
            Self { state: seed }
        }
        fn next_u64(&mut self) -> u64 {
            self.state = self
                .state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.state
        }
        fn next_uniform(&mut self) -> f64 {
            (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
        }
        /// Standard normal via Box-Muller.
        fn next_normal(&mut self) -> f64 {
            let u1 = self.next_uniform().max(1e-300);
            let u2 = self.next_uniform();
            (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
        }
    }

    /// Generate `n` correlated (Y, X_pre) pairs with population correlation
    /// `rho`. Y ~ N(0,1), X_pre = rho*Y + sqrt(1-rho^2)*Z where Z ⊥ Y.
    fn correlated_pairs(n: usize, rho: f64, seed: u64) -> Vec<CupedObservation> {
        let mut rng = Lcg::new(seed);
        let scale = (1.0 - rho * rho).sqrt();
        (0..n)
            .map(|_| {
                let y = rng.next_normal();
                let z = rng.next_normal();
                CupedObservation {
                    y,
                    x_pre: rho * y + scale * z,
                }
            })
            .collect()
    }

    // ── Edge cases ──────────────────────────────────────────────────────────

    #[test]
    fn cuped_empty_observations_returns_zeros() {
        let r = apply_cuped(&[]);
        assert!(r.adjusted_values.is_empty());
        assert_eq!(r.theta, 0.0);
        assert_eq!(r.variance_reduction_pct, 0.0);
    }

    #[test]
    fn cuped_single_observation_no_variance_reduction() {
        // One data point: var = 0; theta = 0/0 → treat as 0; Y' = Y.
        let r = apply_cuped(&[CupedObservation { y: 5.0, x_pre: 3.0 }]);
        assert_eq!(r.theta, 0.0);
        assert_eq!(r.adjusted_values, vec![5.0]);
        assert_eq!(r.variance_reduction_pct, 0.0);
    }

    /// Constant X_pre → theta = 0; Y' = Y; variance_reduction = 0.
    #[test]
    fn cuped_constant_xpre_yields_theta_zero() {
        let observations: Vec<CupedObservation> = (1..=10)
            .map(|i| CupedObservation {
                y: i as f64,
                x_pre: 7.0, // constant
            })
            .collect();
        let r = apply_cuped(&observations);
        assert_eq!(r.theta, 0.0);
        // Y' must equal Y when theta = 0.
        for (orig, adj) in observations.iter().zip(r.adjusted_values.iter()) {
            assert!((orig.y - adj).abs() < 1e-12);
        }
        assert!(r.variance_reduction_pct.abs() < 1e-12);
    }

    // ── Strong-correlation fixture: ≥ 15 % variance reduction ────────────────

    /// Seeded fixture with rho = 0.7+ → CUPED reduces variance by ≥ 15 %.
    #[test]
    fn cuped_strong_correlation_reduces_variance_by_15pct() {
        let observations = correlated_pairs(1000, 0.7, 12345);
        let r = apply_cuped(&observations);
        assert!(
            r.variance_reduction_pct >= 15.0,
            "expected variance_reduction_pct ≥ 15, got {}",
            r.variance_reduction_pct
        );
        // theta should be close to rho (≈ 0.7) since both Y and X_pre are
        // N(0,1) and Cov(Y, X_pre) = rho.
        assert!(
            (r.theta - 0.7).abs() < 0.1,
            "expected theta ≈ 0.7, got {}",
            r.theta
        );
    }

    /// A higher rho gives even larger reduction.
    #[test]
    fn cuped_very_strong_correlation_reduces_variance_substantially() {
        let observations = correlated_pairs(1000, 0.9, 999);
        let r = apply_cuped(&observations);
        // var(Y') / var(Y) ≈ 1 - rho^2 = 0.19 → reduction ≈ 81 %.
        assert!(
            r.variance_reduction_pct > 75.0,
            "expected variance_reduction_pct > 75, got {}",
            r.variance_reduction_pct
        );
    }

    /// Uncorrelated fixture → variance_reduction_pct near zero (small
    /// negative values are possible in finite samples — see module docs).
    #[test]
    fn cuped_uncorrelated_yields_near_zero_reduction() {
        let observations = correlated_pairs(1000, 0.0, 7777);
        let r = apply_cuped(&observations);
        assert!(
            r.variance_reduction_pct.abs() < 3.0,
            "expected near-zero variance_reduction_pct, got {}",
            r.variance_reduction_pct
        );
    }

    /// Adjusted values preserve the mean: E[Y'] = E[Y] (proof: subtracting
    /// θ·(X_pre − mean(X_pre)) sums to zero across the sample).
    #[test]
    fn cuped_adjusted_values_preserve_sample_mean() {
        let observations = correlated_pairs(500, 0.5, 42);
        let r = apply_cuped(&observations);
        let mean_y: f64 = observations.iter().map(|o| o.y).sum::<f64>() / 500.0;
        let mean_y_prime: f64 = r.adjusted_values.iter().sum::<f64>() / 500.0;
        assert!(
            (mean_y - mean_y_prime).abs() < 1e-9,
            "CUPED must preserve the sample mean: {} vs {}",
            mean_y,
            mean_y_prime
        );
    }

    /// Adjusted values length matches the input.
    #[test]
    fn cuped_output_length_matches_input() {
        let observations = correlated_pairs(123, 0.6, 1);
        let r = apply_cuped(&observations);
        assert_eq!(r.adjusted_values.len(), 123);
    }

    /// Serialises with snake_case keys for the UI.
    #[test]
    fn cuped_result_serializes_to_snake_case() {
        let r = CupedResult {
            adjusted_values: vec![1.0, 2.0],
            theta: 0.5,
            variance_reduction_pct: 25.0,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("adjusted_values").is_some());
        assert!(v.get("theta").is_some());
        assert!(v.get("variance_reduction_pct").is_some());
    }
}
