//! Sample-Ratio Mismatch (SRM) detection via Pearson's chi-square goodness-of-fit test.
//!
//! Given the *expected* allocation for each variant (derived from
//! `traffic_allocation * variant_weight`) and the *observed* assignment counts,
//! computes a chi-square statistic and an upper-tail p-value to flag allocation
//! imbalance.
//!
//! Health thresholds (spec §3):
//! - `Red`    — `p < 0.001` (severe mismatch; investigate immediately)
//! - `Yellow` — `0.001 ≤ p < 0.05` (mild mismatch; monitor)
//! - `Green`  — `p ≥ 0.05` (no detectable mismatch)
//!
//! ## Algorithm
//!
//! Chi-square statistic: `χ² = Σ ((observed − expected)² / expected)`.
//!
//! Degrees of freedom: `df = N − 1`, where `N` is the number of variants.
//!
//! The upper-tail p-value is the survival function of the χ² distribution
//! evaluated at `χ²`. Internally this is the regularized upper incomplete
//! gamma function `Q(df/2, χ²/2)`, computed via a series expansion (for small
//! `χ²/df`) and a continued fraction (for large `χ²/df`).
//!
//! ## Edge cases
//!
//! - **Single variant (N = 1)** — chi-square is undefined with 0 df. Returns
//!   `Green` with `p = 1.0`.
//! - **Zero observations** — no data to test. Returns `Green` with `p = 1.0`.
//! - **`expected == 0` for any variant** — variants with zero expected
//!   allocation are themselves a configuration error. Returns `Red` with
//!   `p = 0.0`.
//!
//! All math is implemented locally (Numerical Recipes §6.2) to avoid an
//! external dependency on `statrs`.

use serde::{Deserialize, Serialize};

// ── Public types ─────────────────────────────────────────────────────────────

/// Per-variant SRM data point — observed assignment count + expected count
/// (typically `traffic_allocation * weight * total_observed`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SrmObservation {
    /// Variant identifier (e.g. `"control"`, `"variant_b"`).
    pub variant_key: String,
    /// Number of contexts actually assigned to this variant.
    pub observed: u64,
    /// Expected number of contexts under the configured allocation.
    pub expected: f64,
}

/// Per-variant SRM breakdown reported in [`SrmResult::per_variant`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SrmPerVariant {
    /// Variant identifier.
    pub variant_key: String,
    /// Number of contexts actually assigned.
    pub observed: u64,
    /// Expected number of contexts.
    pub expected: f64,
    /// `(observed − expected) / expected * 100`, in percent. `0.0` when
    /// `expected == 0.0` to avoid division-by-zero NaN.
    pub deviation_pct: f64,
}

/// Health classification for an SRM result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SrmHealth {
    /// No detectable mismatch (`p ≥ 0.05`).
    Green,
    /// Mild mismatch (`0.001 ≤ p < 0.05`).
    Yellow,
    /// Severe mismatch (`p < 0.001`).
    Red,
}

/// Aggregate result of an SRM test on a set of variants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SrmResult {
    /// Per-variant breakdowns (in the order supplied by the caller).
    pub per_variant: Vec<SrmPerVariant>,
    /// Pearson chi-square statistic across all variants.
    pub overall_chi_sq: f64,
    /// Upper-tail p-value of the chi-square statistic under the null
    /// hypothesis of "observed matches expected".
    pub overall_chi_sq_p: f64,
    /// Health classification derived from `overall_chi_sq_p` (see crate-level
    /// docs for thresholds).
    pub health: SrmHealth,
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Run a Pearson chi-square goodness-of-fit test against the given
/// per-variant observed/expected counts.
///
/// See the module-level documentation for the exact algorithm, health
/// thresholds, and edge-case behaviour.
pub fn compute_srm(observations: &[SrmObservation]) -> SrmResult {
    // Edge case: no variants supplied → return a benign Green result.
    if observations.is_empty() {
        return SrmResult {
            per_variant: vec![],
            overall_chi_sq: 0.0,
            overall_chi_sq_p: 1.0,
            health: SrmHealth::Green,
        };
    }

    // Per-variant breakdowns and chi-square accumulator.
    let mut per_variant = Vec::with_capacity(observations.len());
    let mut chi_sq = 0.0_f64;
    let mut total_observed: u64 = 0;
    let mut zero_expected_seen = false;

    for obs in observations {
        let deviation_pct = if obs.expected == 0.0 {
            // Caller-side configuration bug: expected = 0 means an unallocated
            // variant. Record but defer the Red verdict until after the loop.
            zero_expected_seen = true;
            0.0
        } else {
            (obs.observed as f64 - obs.expected) / obs.expected * 100.0
        };

        if obs.expected > 0.0 {
            let diff = obs.observed as f64 - obs.expected;
            chi_sq += diff * diff / obs.expected;
        }
        total_observed = total_observed.saturating_add(obs.observed);

        per_variant.push(SrmPerVariant {
            variant_key: obs.variant_key.clone(),
            observed: obs.observed,
            expected: obs.expected,
            deviation_pct,
        });
    }

    // Edge case: zero-allocation variant in the design → Red.
    if zero_expected_seen {
        // Only flag Red if the zero-expected variant actually saw assignments,
        // OR if there were assignments at all (configuration is suspect either
        // way, but the strongest signal is "assignments happened to a variant
        // that should have been getting 0").
        let any_observed_on_zero = observations
            .iter()
            .any(|o| o.expected == 0.0 && o.observed > 0);
        if any_observed_on_zero || total_observed > 0 {
            return SrmResult {
                per_variant,
                overall_chi_sq: f64::INFINITY,
                overall_chi_sq_p: 0.0,
                health: SrmHealth::Red,
            };
        }
        // No observations at all → harmless; treat as Green below.
    }

    // Edge cases: not enough variants to run a chi-square, or no data.
    if observations.len() < 2 || total_observed == 0 {
        return SrmResult {
            per_variant,
            overall_chi_sq: 0.0,
            overall_chi_sq_p: 1.0,
            health: SrmHealth::Green,
        };
    }

    let df = (observations.len() - 1) as f64;
    let p_value = chi_square_sf(chi_sq, df);

    let health = if p_value < 0.001 {
        SrmHealth::Red
    } else if p_value < 0.05 {
        SrmHealth::Yellow
    } else {
        SrmHealth::Green
    };

    SrmResult {
        per_variant,
        overall_chi_sq: chi_sq,
        overall_chi_sq_p: p_value,
        health,
    }
}

// ── Chi-square survival function ─────────────────────────────────────────────

/// Survival function of the χ²(df) distribution at `x` — i.e. the upper-tail
/// p-value `P(X ≥ x)`. Equivalent to `Q(df/2, x/2)` where `Q` is the
/// regularized upper incomplete gamma function.
fn chi_square_sf(x: f64, df: f64) -> f64 {
    if x <= 0.0 || df <= 0.0 {
        return 1.0;
    }
    if !x.is_finite() {
        return 0.0;
    }
    regularized_gamma_q(df / 2.0, x / 2.0)
}

/// Regularized upper incomplete gamma function `Q(a, x) = Γ(a, x) / Γ(a)`.
///
/// Switches between the series representation of `P(a, x)` (for `x < a + 1`)
/// and the continued-fraction representation of `Q(a, x)` (for `x ≥ a + 1`),
/// following Numerical Recipes §6.2.
fn regularized_gamma_q(a: f64, x: f64) -> f64 {
    if x < 0.0 || a <= 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        return 1.0;
    }
    if x < a + 1.0 {
        1.0 - gamma_series(a, x)
    } else {
        gamma_continued_fraction(a, x)
    }
}

/// Series expansion of `P(a, x)` — converges for `x < a + 1`.
fn gamma_series(a: f64, x: f64) -> f64 {
    const MAX_ITER: usize = 200;
    const EPS: f64 = 3.0e-15;

    let g_ln = lgamma(a);
    let mut ap = a;
    let mut sum = 1.0 / a;
    let mut del = sum;
    for _ in 0..MAX_ITER {
        ap += 1.0;
        del *= x / ap;
        sum += del;
        if del.abs() < sum.abs() * EPS {
            break;
        }
    }
    sum * (-x + a * x.ln() - g_ln).exp()
}

/// Continued-fraction expansion of `Q(a, x)` — converges for `x ≥ a + 1`.
fn gamma_continued_fraction(a: f64, x: f64) -> f64 {
    const MAX_ITER: usize = 200;
    const EPS: f64 = 3.0e-15;
    const FPMIN: f64 = 1.0e-300;

    let g_ln = lgamma(a);
    let mut b = x + 1.0 - a;
    let mut c = 1.0 / FPMIN;
    let mut d = 1.0 / b;
    let mut h = d;
    for i in 1..=MAX_ITER {
        let an = -(i as f64) * (i as f64 - a);
        b += 2.0;
        d = an * d + b;
        if d.abs() < FPMIN {
            d = FPMIN;
        }
        c = b + an / c;
        if c.abs() < FPMIN {
            c = FPMIN;
        }
        d = 1.0 / d;
        let del = d * c;
        h *= del;
        if (del - 1.0).abs() < EPS {
            break;
        }
    }
    h * (-x + a * x.ln() - g_ln).exp()
}

/// Natural-log of the gamma function (Lanczos approximation, g=7).
///
/// Duplicated here from `frequentist.rs` (which keeps its copy as a
/// `pub(crate)`-less private helper) so `srm.rs` stays a self-contained
/// module. The two implementations agree to roughly 1e-13 for `a > 0.5`.
fn lgamma(x: f64) -> f64 {
    const G: f64 = 7.0;
    const C: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];

    if x < 0.5 {
        // Reflection formula: Γ(1-x)Γ(x) = π / sin(πx)
        return std::f64::consts::PI.ln() - (std::f64::consts::PI * x).sin().ln() - lgamma(1.0 - x);
    }

    let x = x - 1.0;
    let mut sum = C[0];
    for (i, &c) in C[1..].iter().enumerate() {
        sum += c / (x + i as f64 + 1.0);
    }
    let tmp = x + G + 0.5;
    (2.0 * std::f64::consts::PI).sqrt().ln() + sum.ln() + (x + 0.5) * tmp.ln() - tmp
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(key: &str, observed: u64, expected: f64) -> SrmObservation {
        SrmObservation {
            variant_key: key.to_string(),
            observed,
            expected,
        }
    }

    // ── chi_square_sf sanity ─────────────────────────────────────────────────

    /// χ² = df has p ≈ 1 - (some fixed value); for df=1, χ²=1 → p ≈ 0.3173.
    #[test]
    fn chi_square_sf_df1_at_one() {
        let p = chi_square_sf(1.0, 1.0);
        assert!((p - 0.3173).abs() < 0.005, "expected ≈0.3173, got {p}");
    }

    /// For df=1, χ² = 3.841 → p ≈ 0.05 (the 95th percentile).
    #[test]
    fn chi_square_sf_df1_critical_at_05() {
        let p = chi_square_sf(3.841, 1.0);
        assert!((p - 0.05).abs() < 0.005, "expected ≈0.05, got {p}");
    }

    /// For df=2, χ² = 5.991 → p ≈ 0.05.
    #[test]
    fn chi_square_sf_df2_critical_at_05() {
        let p = chi_square_sf(5.991, 2.0);
        assert!((p - 0.05).abs() < 0.005, "expected ≈0.05, got {p}");
    }

    /// p-value at χ² = 0 must be 1.
    #[test]
    fn chi_square_sf_at_zero_is_one() {
        assert_eq!(chi_square_sf(0.0, 1.0), 1.0);
        assert_eq!(chi_square_sf(0.0, 2.0), 1.0);
    }

    /// Very large χ² → p → 0.
    #[test]
    fn chi_square_sf_at_large_x_is_zero() {
        let p = chi_square_sf(100.0, 1.0);
        assert!(p < 1e-20, "expected ≈0, got {p}");
    }

    // ── Spec-mandated fixtures ──────────────────────────────────────────────

    /// 50/50 with 1000 each → χ² ≈ 0, p ≈ 1, Green.
    #[test]
    fn srm_balanced_50_50_is_green() {
        let result = compute_srm(&[obs("a", 1000, 1000.0), obs("b", 1000, 1000.0)]);
        assert!(
            result.overall_chi_sq < 1e-9,
            "chi_sq should be 0, got {}",
            result.overall_chi_sq
        );
        assert!(
            result.overall_chi_sq_p > 0.99,
            "p should be ≈1, got {}",
            result.overall_chi_sq_p
        );
        assert_eq!(result.health, SrmHealth::Green);
    }

    /// 50/50 with 800/1200 (expected 1000/1000) → χ² = 80, p << 0.001 → Red.
    #[test]
    fn srm_heavy_mismatch_800_1200_is_red() {
        let result = compute_srm(&[obs("a", 800, 1000.0), obs("b", 1200, 1000.0)]);
        // χ² = (200² + 200²) / 1000 = 80
        assert!(
            (result.overall_chi_sq - 80.0).abs() < 1e-6,
            "chi_sq should be 80, got {}",
            result.overall_chi_sq
        );
        assert!(
            result.overall_chi_sq_p < 0.001,
            "p should be < 0.001, got {}",
            result.overall_chi_sq_p
        );
        assert_eq!(result.health, SrmHealth::Red);
    }

    /// 50/50 with 950/1050 → χ² = 5, p ≈ 0.025, Yellow.
    #[test]
    fn srm_mild_mismatch_950_1050_is_yellow() {
        let result = compute_srm(&[obs("a", 950, 1000.0), obs("b", 1050, 1000.0)]);
        // χ² = (50² + 50²) / 1000 = 5
        assert!(
            (result.overall_chi_sq - 5.0).abs() < 1e-6,
            "chi_sq should be 5, got {}",
            result.overall_chi_sq
        );
        assert!(
            result.overall_chi_sq_p >= 0.001 && result.overall_chi_sq_p < 0.05,
            "p should be in [0.001, 0.05), got {}",
            result.overall_chi_sq_p
        );
        assert_eq!(result.health, SrmHealth::Yellow);
    }

    /// Single variant → SRM is undefined → Green, p = 1.0.
    #[test]
    fn srm_single_variant_is_green() {
        let result = compute_srm(&[obs("only", 1000, 1000.0)]);
        assert_eq!(result.overall_chi_sq_p, 1.0);
        assert_eq!(result.health, SrmHealth::Green);
    }

    /// Empty input → Green, p = 1.0.
    #[test]
    fn srm_empty_input_is_green() {
        let result = compute_srm(&[]);
        assert_eq!(result.overall_chi_sq_p, 1.0);
        assert_eq!(result.health, SrmHealth::Green);
        assert!(result.per_variant.is_empty());
    }

    /// Zero observations across all variants → Green, p = 1.0.
    #[test]
    fn srm_zero_observations_is_green() {
        let result = compute_srm(&[obs("a", 0, 0.0), obs("b", 0, 0.0)]);
        assert_eq!(result.overall_chi_sq_p, 1.0);
        assert_eq!(result.health, SrmHealth::Green);
    }

    /// Zero-allocation variant (expected = 0) with non-zero observations on
    /// another variant → Red.
    #[test]
    fn srm_zero_allocation_variant_is_red() {
        let result = compute_srm(&[obs("a", 1000, 1000.0), obs("rogue", 500, 0.0)]);
        assert_eq!(result.overall_chi_sq_p, 0.0);
        assert_eq!(result.health, SrmHealth::Red);
        // The rogue variant should still appear in per_variant.
        assert_eq!(result.per_variant.len(), 2);
        assert_eq!(result.per_variant[1].variant_key, "rogue");
        assert_eq!(result.per_variant[1].expected, 0.0);
    }

    /// Even a non-rogue zero-expected variant when other variants have
    /// observations: this is still a configuration bug. Red.
    #[test]
    fn srm_zero_allocation_with_observed_on_others_is_red() {
        let result = compute_srm(&[obs("a", 1000, 1000.0), obs("dead", 0, 0.0)]);
        assert_eq!(result.health, SrmHealth::Red);
    }

    /// Deviation percentage in per_variant rows is correct.
    #[test]
    fn srm_per_variant_deviation_pct() {
        let result = compute_srm(&[obs("a", 900, 1000.0), obs("b", 1100, 1000.0)]);
        // 900 vs 1000 expected → -10 %
        assert!(
            (result.per_variant[0].deviation_pct + 10.0).abs() < 1e-6,
            "expected -10%, got {}",
            result.per_variant[0].deviation_pct
        );
        // 1100 vs 1000 expected → +10 %
        assert!(
            (result.per_variant[1].deviation_pct - 10.0).abs() < 1e-6,
            "expected +10%, got {}",
            result.per_variant[1].deviation_pct
        );
    }

    /// Three variants, balanced. df = 2 → Green.
    #[test]
    fn srm_three_balanced_variants_is_green() {
        let result = compute_srm(&[
            obs("a", 1000, 1000.0),
            obs("b", 1000, 1000.0),
            obs("c", 1000, 1000.0),
        ]);
        assert!(result.overall_chi_sq < 1e-9);
        assert!(result.overall_chi_sq_p > 0.99);
        assert_eq!(result.health, SrmHealth::Green);
    }

    /// SrmObservation / SrmResult round-trip through serde JSON.
    #[test]
    fn srm_result_serializes_to_snake_case() {
        let r = compute_srm(&[obs("a", 1000, 1000.0), obs("b", 1000, 1000.0)]);
        let json = serde_json::to_string(&r).unwrap();
        assert!(json.contains("\"per_variant\""));
        assert!(json.contains("\"overall_chi_sq\""));
        assert!(json.contains("\"overall_chi_sq_p\""));
        assert!(json.contains("\"health\""));
        assert!(json.contains("\"green\""));
    }

    /// `regularized_gamma_q(a, 0) = 1`.
    #[test]
    fn regularized_gamma_q_at_zero_is_one() {
        assert_eq!(regularized_gamma_q(2.0, 0.0), 1.0);
        assert_eq!(regularized_gamma_q(5.0, 0.0), 1.0);
    }

    /// `lgamma(1) = 0`, `lgamma(0.5) = ln(√π)`.
    #[test]
    fn lgamma_known_values() {
        assert!((lgamma(1.0) - 0.0).abs() < 1e-10);
        let expected = std::f64::consts::PI.sqrt().ln();
        assert!((lgamma(0.5) - expected).abs() < 1e-6);
    }
}
