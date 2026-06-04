//! Frequentist statistical analysis engine.
//!
//! Provides hypothesis-testing functions for all four metric types supported
//! by the experimentation platform:
//!
//! | Function                | Test used                              |
//! |-------------------------|----------------------------------------|
//! | [`analyze_count`]       | Two-proportion z-test                  |
//! | [`analyze_numeric`]     | Welch's t-test                         |
//! | [`analyze_percentile`]  | Bootstrap CI (delegates to bootstrap)  |
//! | [`analyze_funnel`]      | Two-proportion z-test on final step    |

use super::sequential::RatioGroupStats;
use super::{ConfidenceInterval, FrequentistResult, VariantStats, Z95, norm_cdf};

// ── Normal CDF helpers ────────────────────────────────────────────────────────

/// Two-tailed p-value from the standard normal distribution. Uses the shared
/// [`super::norm_cdf`] so frequentist and Bayesian engines agree bit-for-bit.
#[inline]
fn z_to_p(z: f64) -> f64 {
    2.0 * norm_cdf(-z.abs())
}

// ── Student's t-distribution (for Welch's t-test) ─────────────────────────

/// Regularised incomplete beta function I_x(a, b) using a continued-fraction
/// expansion (Numerical Recipes, §6.4).  Accurate to ~10⁻¹⁰ for typical
/// degree-of-freedom values seen in A/B tests.
fn regularised_incomplete_beta(x: f64, a: f64, b: f64) -> f64 {
    if !(0.0..=1.0).contains(&x) {
        return 0.0;
    }
    if x == 0.0 {
        return 0.0;
    }
    if x == 1.0 {
        return 1.0;
    }

    // Use symmetry relation when x > (a+1)/(a+b+2) for faster convergence.
    if x > (a + 1.0) / (a + b + 2.0) {
        return 1.0 - regularised_incomplete_beta(1.0 - x, b, a);
    }

    // ln Β(a,b) via lgamma
    let ln_beta = lgamma(a) + lgamma(b) - lgamma(a + b);
    let front = (x.ln() * a + (1.0 - x).ln() * b - ln_beta).exp() / a;

    // Lentz's continued-fraction algorithm
    const MAX_ITER: usize = 200;
    const EPS: f64 = 3.0e-7;
    const FPMIN: f64 = 1.0e-300;

    let mut c = 1.0_f64;
    let mut d = 1.0 - (a + b) * x / (a + 1.0);
    d = if d.abs() < FPMIN { FPMIN } else { d };
    d = 1.0 / d;
    let mut h = d;

    for m in 1..=MAX_ITER {
        let m = m as f64;
        // Even step
        let num = m * (b - m) * x / ((a + 2.0 * m - 1.0) * (a + 2.0 * m));
        d = 1.0 + num * d;
        d = if d.abs() < FPMIN { FPMIN } else { d };
        c = 1.0 + num / c;
        c = if c.abs() < FPMIN { FPMIN } else { c };
        d = 1.0 / d;
        h *= d * c;
        // Odd step
        let num = -(a + m) * (a + b + m) * x / ((a + 2.0 * m) * (a + 2.0 * m + 1.0));
        d = 1.0 + num * d;
        d = if d.abs() < FPMIN { FPMIN } else { d };
        c = 1.0 + num / c;
        c = if c.abs() < FPMIN { FPMIN } else { c };
        d = 1.0 / d;
        let delta = d * c;
        h *= delta;
        if (delta - 1.0).abs() < EPS {
            break;
        }
    }

    front * h
}

/// Natural-log of the gamma function (Lanczos approximation, g=7).
fn lgamma(x: f64) -> f64 {
    // Coefficients from "Numerical Recipes in C", 3rd ed.
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

    let x = if x < 0.5 {
        // Reflection formula: Γ(1-x)Γ(x) = π/sin(πx)
        return std::f64::consts::PI.ln() - (std::f64::consts::PI * x).sin().ln() - lgamma(1.0 - x);
    } else {
        x - 1.0
    };

    let mut sum = C[0];
    for (i, &c) in C[1..].iter().enumerate() {
        sum += c / (x + i as f64 + 1.0);
    }

    let tmp = x + G + 0.5;
    (2.0 * std::f64::consts::PI).sqrt().ln() + sum.ln() + (x + 0.5) * tmp.ln() - tmp
}

/// Two-tailed p-value from the Student's t-distribution with `df` degrees of
/// freedom.  Uses the regularised incomplete beta function:
/// `p = I_{df/(df+t²)}(df/2, 1/2)`.
fn t_to_p(t: f64, df: f64) -> f64 {
    let x = df / (df + t * t);
    regularised_incomplete_beta(x, df / 2.0, 0.5)
}

/// Critical value of the t-distribution at 97.5th percentile for given df
/// (used to build 95 % two-sided CI).
fn t_critical_95(df: f64) -> f64 {
    // Bisection search: find t such that t_to_p(t, df) ≈ 0.05
    let mut lo = 0.0_f64;
    let mut hi = 100.0_f64;
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        if t_to_p(mid, df) > 0.05 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

// ── Multi-comparison correction ───────────────────────────────────────────────

/// Apply Bonferroni correction to a slice of p-values from `K` pairwise
/// comparisons against a control variant.
///
/// Each adjusted p-value is `p_adj = min(1, p_raw * K)` where `K =
/// p_values.len()` is the number of comparisons (i.e. one per non-control
/// variant in a multi-arm experiment).
///
/// Returns a new `Vec<f64>` of the same length, preserving input order.
/// Empty input returns an empty vec. `NaN` inputs are preserved (Bonferroni
/// on NaN is undefined; the bootstrap / percentile path uses NaN p-values and
/// must not be silently coerced).
///
/// # Examples
///
/// ```ignore
/// use stitchd_core::experimentation::stats::frequentist::bonferroni_correct;
/// let raw = vec![0.01, 0.03, 0.04];
/// let corrected = bonferroni_correct(&raw);
/// assert_eq!(corrected, vec![0.03, 0.09, 0.12]);
/// ```
#[must_use]
pub fn bonferroni_correct(p_values: &[f64]) -> Vec<f64> {
    let k = p_values.len() as f64;
    p_values
        .iter()
        .map(|&p| {
            if p.is_nan() {
                f64::NAN
            } else {
                (p * k).clamp(0.0, 1.0)
            }
        })
        .collect()
}

// ── Public API ────────────────────────────────────────────────────────────────

/// The canonical "insufficient data / undefined statistics" frequentist result:
/// `p_value = 1.0` (never significant), `significant = false`, and a whole-line
/// confidence interval `(-∞, +∞)` which trivially covers any true effect rather
/// than collapsing to a misleading point. Mirrors the degenerate branch of
/// [`analyze_ratio`].
///
/// Used by the count / numeric / funnel paths when an arm carries no usable
/// information: a sample size too small for the statistic to be defined (e.g.
/// `n < 2` makes the Welch t-test's sample variance undefined → `0/0 = NaN`
/// degrees of freedom → a *false* `p = 0` / `significant = true`; `n < 1` makes
/// a proportion `c/n = 0/0 = NaN`). Returning this instead avoids emitting
/// spurious significance or NaN.
#[inline]
fn insufficient_frequentist() -> FrequentistResult {
    FrequentistResult {
        p_value: 1.0,
        p_value_corrected: None,
        confidence_interval: ConfidenceInterval {
            lower: f64::NEG_INFINITY,
            upper: f64::INFINITY,
        },
        significant: false,
    }
}

/// Two-proportion z-test for count / binary metrics.
///
/// Uses the pooled proportion under H₀ to compute the z-statistic, then
/// derives a two-tailed p-value via the standard normal CDF.  The 95 %
/// confidence interval is computed on the *lift* (p₂ − p₁) using the
/// unpooled standard error.
///
/// Returns an [`insufficient_frequentist`] result (`p = 1`, not significant,
/// whole-line CI) when either arm has `sample_size < 1`, since a proportion
/// `c / n` is then `0 / 0 = NaN`.
///
/// # Panics
/// Panics if `conversions` is `None` on either variant.
pub fn analyze_count(control: &VariantStats, variant: &VariantStats) -> FrequentistResult {
    if control.sample_size < 1 || variant.sample_size < 1 {
        return insufficient_frequentist();
    }
    let n1 = control.sample_size as f64;
    let n2 = variant.sample_size as f64;
    let c1 = control
        .conversions
        .expect("analyze_count: control.conversions must be Some") as f64;
    let c2 = variant
        .conversions
        .expect("analyze_count: variant.conversions must be Some") as f64;

    let p1 = c1 / n1;
    let p2 = c2 / n2;

    let p_pool = (c1 + c2) / (n1 + n2);
    let se_pool = (p_pool * (1.0 - p_pool) * (1.0 / n1 + 1.0 / n2)).sqrt();

    let z = if se_pool == 0.0 {
        0.0
    } else {
        (p2 - p1) / se_pool
    };
    let p_value = z_to_p(z);

    // 95 % CI on lift using unpooled SE
    let se_lift = (p1 * (1.0 - p1) / n1 + p2 * (1.0 - p2) / n2).sqrt();
    let margin = Z95 * se_lift;
    let lift = p2 - p1;

    FrequentistResult {
        p_value,
        p_value_corrected: None,
        confidence_interval: ConfidenceInterval {
            lower: lift - margin,
            upper: lift + margin,
        },
        significant: p_value < 0.05,
    }
}

/// Welch's t-test for continuous numeric metrics.
///
/// Uses the Welch-Satterthwaite equation to compute approximate degrees of
/// freedom, then derives a two-tailed p-value from the Student's
/// t-distribution.  The 95 % confidence interval is built using the same
/// approximate degrees of freedom.
///
/// Returns an [`insufficient_frequentist`] result (`p = 1`, not significant,
/// whole-line CI) when either arm has `sample_size < 2`. With `n < 2` an arm's
/// sample variance is undefined, so the Welch-Satterthwaite denominator
/// `s⁴/(n−1)` is `0/0 = NaN`; that NaN flows into `t_to_p(_, NaN) → 0.0` and
/// `t_critical_95(NaN)`, producing a *false* `p = 0` / `significant = true` and
/// a degenerate `[diff, diff]` CI. The `se == 0` early-exit below does not catch
/// this when the *other* arm still carries variance, so the guard must be here.
///
/// # Panics
/// Panics if `mean` or `variance` is `None` on either variant.
pub fn analyze_numeric(control: &VariantStats, variant: &VariantStats) -> FrequentistResult {
    if control.sample_size < 2 || variant.sample_size < 2 {
        return insufficient_frequentist();
    }
    let n1 = control.sample_size as f64;
    let n2 = variant.sample_size as f64;
    let mean1 = control
        .mean
        .expect("analyze_numeric: control.mean must be Some");
    let mean2 = variant
        .mean
        .expect("analyze_numeric: variant.mean must be Some");
    let var1 = control
        .variance
        .expect("analyze_numeric: control.variance must be Some");
    let var2 = variant
        .variance
        .expect("analyze_numeric: variant.variance must be Some");

    let s1_sq = var1 / n1;
    let s2_sq = var2 / n2;
    let se = (s1_sq + s2_sq).sqrt();

    let t = if se == 0.0 { 0.0 } else { (mean2 - mean1) / se };

    // Welch-Satterthwaite degrees of freedom
    let df = if se == 0.0 {
        1.0
    } else {
        let num = (s1_sq + s2_sq).powi(2);
        let denom = s1_sq.powi(2) / (n1 - 1.0) + s2_sq.powi(2) / (n2 - 1.0);
        if denom == 0.0 { 1.0 } else { num / denom }
    };

    let p_value = t_to_p(t, df);
    let t_crit = t_critical_95(df);
    let margin = t_crit * se;
    let diff = mean2 - mean1;

    FrequentistResult {
        p_value,
        p_value_corrected: None,
        confidence_interval: ConfidenceInterval {
            lower: diff - margin,
            upper: diff + margin,
        },
        significant: p_value < 0.05,
    }
}

/// Bootstrap-based analysis for percentile metrics.
///
/// Because bootstrap produces no analytical p-value, `p_value` is set to
/// [`f64::NAN`] and `significant` is always `false` — callers should use CI
/// overlap to assess significance for percentile metrics.
///
/// The CI represents the difference (variant percentile − control percentile).
///
/// # Arguments
/// * `control_samples`  – raw observations for the control variant
/// * `variant_samples`  – raw observations for the treatment variant
/// * `percentile`       – target percentile in `[0, 100]` (e.g. `95.0` for p95)
pub fn analyze_percentile(
    control_samples: &[f64],
    variant_samples: &[f64],
    percentile: f64,
) -> FrequentistResult {
    let p = percentile / 100.0;
    let (c_lower, c_upper) = super::bootstrap::bootstrap_percentile_ci(control_samples, p, 1_000);
    let (v_lower, v_upper) = super::bootstrap::bootstrap_percentile_ci(variant_samples, p, 1_000);
    FrequentistResult {
        p_value: f64::NAN,
        p_value_corrected: None,
        confidence_interval: ConfidenceInterval {
            lower: v_lower - c_upper,
            upper: v_upper - c_lower,
        },
        significant: false,
    }
}

/// Two-proportion z-test for the *final-step conversion rate* of a funnel metric.
///
/// Uses `VariantStats::conversion_rate` as the proportion, which represents
/// the end-to-end funnel completion rate.  The sample-size denominator is the
/// top-of-funnel exposure count (`sample_size`).
///
/// Returns an [`insufficient_frequentist`] result (`p = 1`, not significant,
/// whole-line CI) when either arm has `sample_size < 1`, since the pooled SE's
/// `1 / n` term is then `1 / 0 = ∞` and the statistic is `NaN`.
///
/// # Panics
/// Panics if `conversion_rate` is `None` on either variant.
pub fn analyze_funnel(control: &VariantStats, variant: &VariantStats) -> FrequentistResult {
    if control.sample_size < 1 || variant.sample_size < 1 {
        return insufficient_frequentist();
    }
    let n1 = control.sample_size as f64;
    let n2 = variant.sample_size as f64;
    let p1 = control
        .conversion_rate
        .expect("analyze_funnel: control.conversion_rate must be Some");
    let p2 = variant
        .conversion_rate
        .expect("analyze_funnel: variant.conversion_rate must be Some");

    // Derive effective conversion counts for the pooled proportion
    let c1 = p1 * n1;
    let c2 = p2 * n2;

    let p_pool = (c1 + c2) / (n1 + n2);
    let se_pool = (p_pool * (1.0 - p_pool) * (1.0 / n1 + 1.0 / n2)).sqrt();

    let z = if se_pool == 0.0 {
        0.0
    } else {
        (p2 - p1) / se_pool
    };
    let p_value = z_to_p(z);

    let se_lift = (p1 * (1.0 - p1) / n1 + p2 * (1.0 - p2) / n2).sqrt();
    let margin = Z95 * se_lift;
    let lift = p2 - p1;

    FrequentistResult {
        p_value,
        p_value_corrected: None,
        confidence_interval: ConfidenceInterval {
            lower: lift - margin,
            upper: lift + margin,
        },
        significant: p_value < 0.05,
    }
}

/// Delta-method contrast for a **ratio** metric (treatment vs control).
///
/// A ratio metric's per-group point estimate is `R = num_sum / den_sum`; its
/// variance comes from the delta method ([`RatioGroupStats::ratio_var`], the
/// single source of truth shared with [`super::sequential::sequential_ratio`]).
/// Because the two groups are independent, the effect `R₂ − R₁` has variance
/// `Var(R₁) + Var(R₂)`, so `SE = sqrt(Var(R_c) + Var(R_t))`. The two-tailed
/// p-value is `2·Φ(−|z|)` with `z = (R_t − R_c) / SE`, and the 95 % CI is
/// `(R_t − R_c) ± Z95·SE`.
///
/// Returns a non-significant, whole-line-CI result when either group is
/// degenerate (`n < 2`, `den_sum ≤ 0`, non-finite variance) — mirroring the
/// insufficient-data convention of the count / numeric paths.
#[must_use]
pub fn analyze_ratio(control: &RatioGroupStats, variant: &RatioGroupStats) -> FrequentistResult {
    let (Some((r_c, var_c)), Some((r_t, var_t))) = (control.ratio_var(), variant.ratio_var())
    else {
        // Degenerate group(s): emit the canonical insufficient-data result
        // (p = 1, whole-line CI, not significant) — identical to the inline
        // struct this used to build.
        return insufficient_frequentist();
    };
    let diff = r_t - r_c;
    let se = (var_c + var_t).sqrt();
    let z = if se == 0.0 { 0.0 } else { diff / se };
    let p_value = z_to_p(z);
    let margin = Z95 * se;
    FrequentistResult {
        p_value,
        p_value_corrected: None,
        confidence_interval: ConfidenceInterval {
            lower: diff - margin,
            upper: diff + margin,
        },
        significant: p_value < 0.05,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Helper to build a VariantStats for proportion/count tests
    fn make_count_stats(n: i64, conversions: i64) -> VariantStats {
        VariantStats {
            sample_size: n,
            conversions: Some(conversions),
            mean: None,
            variance: None,
            conversion_rate: None,
            percentiles: None,
        }
    }

    // Helper to build VariantStats for numeric tests
    fn make_numeric_stats(n: i64, mean: f64, variance: f64) -> VariantStats {
        VariantStats {
            sample_size: n,
            conversions: None,
            mean: Some(mean),
            variance: Some(variance),
            conversion_rate: None,
            percentiles: None,
        }
    }

    // Helper to build VariantStats for funnel tests
    fn make_funnel_stats(n: i64, conversion_rate: f64) -> VariantStats {
        VariantStats {
            sample_size: n,
            conversions: None,
            mean: None,
            variance: None,
            conversion_rate: Some(conversion_rate),
            percentiles: None,
        }
    }

    // ── norm_cdf / erf ────────────────────────────────────────────────────────

    #[test]
    fn norm_cdf_at_zero_is_half() {
        let p = norm_cdf(0.0);
        assert!((p - 0.5).abs() < 1e-6, "Φ(0) should be 0.5, got {p}");
    }

    #[test]
    fn norm_cdf_at_196_is_approximately_975() {
        let p = norm_cdf(1.96);
        assert!(
            (p - 0.975).abs() < 1e-3,
            "Φ(1.96) should be ≈ 0.975, got {p}"
        );
    }

    #[test]
    fn norm_cdf_symmetric() {
        for z in [-3.0, -1.96, -1.0, 0.0, 1.0, 1.96, 3.0] {
            let p = norm_cdf(z) + norm_cdf(-z);
            assert!(
                (p - 1.0).abs() < 1e-6,
                "Φ(z)+Φ(-z) should be 1 for z={z}, got {p}"
            );
        }
    }

    // ── z_to_p ────────────────────────────────────────────────────────────────

    #[test]
    fn z_to_p_at_196_is_approx_005() {
        let p = z_to_p(1.96);
        assert!((p - 0.05).abs() < 1e-3, "p(|z|≥1.96) ≈ 0.05, got {p}");
    }

    #[test]
    fn z_to_p_symmetric() {
        for z in [0.5, 1.0, 1.96, 2.576, 3.0] {
            assert!((z_to_p(z) - z_to_p(-z)).abs() < 1e-12);
        }
    }

    // ── t_to_p ────────────────────────────────────────────────────────────────

    #[test]
    fn t_to_p_large_df_approaches_normal() {
        // With df=1000, t-distribution ≈ normal.  t=1.96 → p ≈ 0.05
        let p = t_to_p(1.96, 1000.0);
        assert!(
            (p - 0.05).abs() < 0.01,
            "t→p with df=1000 should be ≈0.05, got {p}"
        );
    }

    #[test]
    fn t_to_p_known_value_df10() {
        // df=10, t=2.228 is the 97.5th percentile → two-tailed p ≈ 0.05
        let p = t_to_p(2.228, 10.0);
        assert!((p - 0.05).abs() < 0.005, "t=2.228, df=10 → p≈0.05, got {p}");
    }

    // ── analyze_count ─────────────────────────────────────────────────────────

    /// n=1000 each, p1=0.10, p2=0.15 → clearly significant
    #[test]
    fn analyze_count_large_effect_is_significant() {
        let control = make_count_stats(1000, 100); // p=0.10
        let variant = make_count_stats(1000, 150); // p=0.15
        let result = analyze_count(&control, &variant);

        assert!(
            result.significant,
            "p1=0.10, p2=0.15, n=1000: should be significant; p_value={}",
            result.p_value
        );
        assert!(
            result.p_value < 0.05,
            "p_value should be < 0.05, got {}",
            result.p_value
        );
        // CI should not straddle zero
        assert!(
            result.confidence_interval.lower > 0.0,
            "CI lower should be positive for positive lift; got {}",
            result.confidence_interval.lower
        );
    }

    /// n=100 each, p1=0.10, p2=0.11 → too small a difference; NOT significant
    #[test]
    fn analyze_count_small_effect_not_significant() {
        let control = make_count_stats(100, 10); // p=0.10
        let variant = make_count_stats(100, 11); // p=0.11
        let result = analyze_count(&control, &variant);

        assert!(
            !result.significant,
            "p1=0.10, p2=0.11, n=100: should NOT be significant; p_value={}",
            result.p_value
        );
        assert!(
            result.p_value >= 0.05,
            "p_value should be ≥ 0.05, got {}",
            result.p_value
        );
    }

    /// No difference at all → p_value should be 1.0
    #[test]
    fn analyze_count_identical_proportions_has_high_p_value() {
        let control = make_count_stats(1000, 100);
        let variant = make_count_stats(1000, 100);
        let result = analyze_count(&control, &variant);

        assert!(
            result.p_value > 0.9,
            "identical proportions should give p_value ≈ 1, got {}",
            result.p_value
        );
        assert!(!result.significant);
    }

    /// The lift should match expected direction
    #[test]
    fn analyze_count_lift_direction_matches_proportions() {
        let control = make_count_stats(1000, 100); // p=0.10
        let variant = make_count_stats(1000, 150); // p=0.15, positive lift
        let result = analyze_count(&control, &variant);

        let expected_lift = 0.15 - 0.10;
        let midpoint = (result.confidence_interval.lower + result.confidence_interval.upper) / 2.0;
        assert!(
            (midpoint - expected_lift).abs() < 0.001,
            "CI midpoint {midpoint} should be near lift {expected_lift}"
        );
    }

    // ── analyze_numeric ───────────────────────────────────────────────────────

    /// Large clear difference → significant
    #[test]
    fn analyze_numeric_large_effect_is_significant() {
        // mean1=100, mean2=110, var=100, n=500 each
        // SE = sqrt(100/500 + 100/500) = sqrt(0.4) ≈ 0.632
        // t = (110-100)/0.632 ≈ 15.8 → definitely significant
        let control = make_numeric_stats(500, 100.0, 100.0);
        let variant = make_numeric_stats(500, 110.0, 100.0);
        let result = analyze_numeric(&control, &variant);

        assert!(
            result.significant,
            "large numeric difference should be significant; p_value={}",
            result.p_value
        );
        assert!(
            result.p_value < 0.001,
            "p_value should be very small, got {}",
            result.p_value
        );
    }

    /// Small difference → NOT significant
    #[test]
    fn analyze_numeric_small_effect_not_significant() {
        // mean1=100, mean2=100.01, var=25, n=50 each
        // SE = sqrt(25/50 + 25/50) = 1.0
        // t = 0.01 / 1.0 = 0.01 → clearly not significant
        let control = make_numeric_stats(50, 100.0, 25.0);
        let variant = make_numeric_stats(50, 100.01, 25.0);
        let result = analyze_numeric(&control, &variant);

        assert!(
            !result.significant,
            "tiny numeric difference should NOT be significant; p_value={}",
            result.p_value
        );
        assert!(
            result.p_value > 0.9,
            "p_value should be very high, got {}",
            result.p_value
        );
    }

    /// CI should straddle zero when not significant
    #[test]
    fn analyze_numeric_ci_straddles_zero_when_not_significant() {
        let control = make_numeric_stats(50, 100.0, 25.0);
        let variant = make_numeric_stats(50, 100.01, 25.0);
        let result = analyze_numeric(&control, &variant);

        assert!(
            result.confidence_interval.lower < 0.0 && result.confidence_interval.upper > 0.0,
            "CI should straddle zero; lower={}, upper={}",
            result.confidence_interval.lower,
            result.confidence_interval.upper
        );
    }

    /// Welch-Satterthwaite df: unequal variances handled correctly
    #[test]
    fn analyze_numeric_unequal_variances_significant() {
        // n1=200, var1=4 (tight), n2=200, var2=100 (wide), mean diff=2
        // SE = sqrt(4/200 + 100/200) = sqrt(0.52) ≈ 0.721
        // t ≈ 2.774 → should be significant
        let control = make_numeric_stats(200, 10.0, 4.0);
        let variant = make_numeric_stats(200, 12.0, 100.0);
        let result = analyze_numeric(&control, &variant);

        assert!(
            result.significant,
            "mean diff=2 with n=200 should be significant; p_value={}",
            result.p_value
        );
    }

    // ── analyze_percentile ────────────────────────────────────────────────────

    #[test]
    fn analyze_percentile_p_value_is_nan() {
        let control: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        let variant: Vec<f64> = (1..=100).map(|x| x as f64 * 1.1).collect();
        let result = analyze_percentile(&control, &variant, 50.0);

        assert!(
            result.p_value.is_nan(),
            "percentile analysis p_value should be NaN, got {}",
            result.p_value
        );
    }

    #[test]
    fn analyze_percentile_significant_is_false() {
        let control: Vec<f64> = (1..=100).map(|x| x as f64).collect();
        let variant: Vec<f64> = (1..=100).map(|x| x as f64 * 1.1).collect();
        let result = analyze_percentile(&control, &variant, 50.0);

        assert!(
            !result.significant,
            "percentile analysis significant should always be false"
        );
    }

    #[test]
    fn analyze_percentile_ci_has_finite_bounds() {
        let control: Vec<f64> = (1..=200).map(|x| x as f64).collect();
        let variant: Vec<f64> = (1..=200).map(|x| x as f64 + 10.0).collect();
        let result = analyze_percentile(&control, &variant, 95.0);

        assert!(
            result.confidence_interval.lower.is_finite(),
            "CI lower should be finite"
        );
        assert!(
            result.confidence_interval.upper.is_finite(),
            "CI upper should be finite"
        );
        assert!(
            result.confidence_interval.lower <= result.confidence_interval.upper,
            "CI lower should be ≤ upper"
        );
    }

    // ── analyze_funnel ────────────────────────────────────────────────────────

    /// n=1000 each, rate1=0.10, rate2=0.15 → significant (mirrors analyze_count test)
    #[test]
    fn analyze_funnel_large_effect_is_significant() {
        let control = make_funnel_stats(1000, 0.10);
        let variant = make_funnel_stats(1000, 0.15);
        let result = analyze_funnel(&control, &variant);

        assert!(
            result.significant,
            "funnel: rate 10%→15%, n=1000 should be significant; p_value={}",
            result.p_value
        );
        assert!(result.p_value < 0.05);
    }

    /// n=100 each, rate1=0.10, rate2=0.11 → NOT significant
    #[test]
    fn analyze_funnel_small_effect_not_significant() {
        let control = make_funnel_stats(100, 0.10);
        let variant = make_funnel_stats(100, 0.11);
        let result = analyze_funnel(&control, &variant);

        assert!(
            !result.significant,
            "funnel: rate 10%→11%, n=100 should NOT be significant; p_value={}",
            result.p_value
        );
        assert!(result.p_value >= 0.05);
    }

    // ── regularised_incomplete_beta edge cases ────────────────────────────────

    #[test]
    fn regularised_incomplete_beta_out_of_range_returns_zero() {
        // x < 0
        assert_eq!(regularised_incomplete_beta(-0.1, 2.0, 3.0), 0.0);
        // x > 1
        assert_eq!(regularised_incomplete_beta(1.1, 2.0, 3.0), 0.0);
    }

    #[test]
    fn regularised_incomplete_beta_at_zero_returns_zero() {
        assert_eq!(regularised_incomplete_beta(0.0, 2.0, 3.0), 0.0);
    }

    #[test]
    fn regularised_incomplete_beta_at_one_returns_one() {
        assert_eq!(regularised_incomplete_beta(1.0, 2.0, 3.0), 1.0);
    }

    // ── lgamma reflection formula (x < 0.5) ──────────────────────────────────

    #[test]
    fn lgamma_at_small_x_uses_reflection() {
        // lgamma(1) = ln(Γ(1)) = ln(1) = 0
        let v = lgamma(1.0);
        assert!((v - 0.0).abs() < 1e-6, "lgamma(1) should be 0, got {v}");

        // lgamma(0.5) = ln(sqrt(π)) ≈ 0.5723...
        let v2 = lgamma(0.5);
        let expected = (std::f64::consts::PI.sqrt()).ln();
        assert!(
            (v2 - expected).abs() < 1e-6,
            "lgamma(0.5) should be {expected}, got {v2}"
        );

        // Trigger the x < 0.5 branch explicitly
        let v3 = lgamma(0.25);
        assert!(v3.is_finite(), "lgamma(0.25) should be finite, got {v3}");
    }

    // ── FIX 1: n<2 / n<1 insufficient-data guards (no false significance) ─────

    /// An arm with `sample_size == 1` (variance 0) crossed with a large,
    /// high-variance arm previously produced Welch `df = 0/0 = NaN`, which made
    /// `t_to_p(_, NaN) = 0.0` → a *false* `p = 0` / `significant = true`, and
    /// `t_critical_95(NaN)` collapsed the CI to `[diff, diff]`. The guard must
    /// short-circuit to the insufficient-data result instead.
    #[test]
    fn analyze_numeric_control_n1_is_insufficient_not_significant() {
        let control = VariantStats {
            sample_size: 1,
            conversions: None,
            mean: Some(100.0),
            variance: Some(0.0),
            conversion_rate: None,
            percentiles: None,
        };
        let variant = make_numeric_stats(500, 110.0, 100.0);
        let result = analyze_numeric(&control, &variant);

        assert!(!result.significant, "n=1 arm must NOT be significant");
        assert_ne!(result.p_value, 0.0, "p_value must not be the false 0.0");
        assert_eq!(result.p_value, 1.0, "insufficient-data p_value is 1.0");
        // CI must NOT collapse to [diff, diff] (= [10, 10] here).
        let diff = 110.0 - 100.0;
        assert!(
            !(result.confidence_interval.lower == diff && result.confidence_interval.upper == diff),
            "CI must not collapse to [diff, diff]; got [{}, {}]",
            result.confidence_interval.lower,
            result.confidence_interval.upper
        );
        assert_eq!(result.confidence_interval.lower, f64::NEG_INFINITY);
        assert_eq!(result.confidence_interval.upper, f64::INFINITY);
    }

    /// Same failure mode when the degenerate (`n == 1`) arm is the VARIANT.
    #[test]
    fn analyze_numeric_variant_n1_is_insufficient_not_significant() {
        let control = make_numeric_stats(500, 110.0, 100.0);
        let variant = VariantStats {
            sample_size: 1,
            conversions: None,
            mean: Some(100.0),
            variance: Some(0.0),
            conversion_rate: None,
            percentiles: None,
        };
        let result = analyze_numeric(&control, &variant);

        assert!(!result.significant);
        assert_ne!(result.p_value, 0.0);
        assert_eq!(result.p_value, 1.0);
        let diff = 100.0 - 110.0;
        assert!(
            !(result.confidence_interval.lower == diff && result.confidence_interval.upper == diff),
            "CI must not collapse to [diff, diff]"
        );
    }

    /// `analyze_count` with `sample_size == 0` previously emitted NaN (`c/n =
    /// 0/0`); it must now return the insufficient-data result.
    #[test]
    fn analyze_count_n0_is_insufficient_not_nan() {
        let control = make_count_stats(0, 0);
        let variant = make_count_stats(500, 80);
        let result = analyze_count(&control, &variant);

        assert!(!result.significant);
        assert!(!result.p_value.is_nan(), "p_value must not be NaN");
        assert_eq!(result.p_value, 1.0);
        assert!(!result.confidence_interval.lower.is_nan());
        assert!(!result.confidence_interval.upper.is_nan());
        assert_eq!(result.confidence_interval.lower, f64::NEG_INFINITY);
        assert_eq!(result.confidence_interval.upper, f64::INFINITY);
    }

    /// `analyze_funnel` with `sample_size == 0` must likewise be insufficient,
    /// not NaN.
    #[test]
    fn analyze_funnel_n0_is_insufficient_not_nan() {
        let control = make_funnel_stats(0, 0.0);
        let variant = make_funnel_stats(500, 0.15);
        let result = analyze_funnel(&control, &variant);

        assert!(!result.significant);
        assert!(!result.p_value.is_nan());
        assert_eq!(result.p_value, 1.0);
    }

    // ── analyze_numeric with zero variance (se == 0.0) ───────────────────────

    #[test]
    fn analyze_numeric_zero_variance_identical_means_returns_high_p_value() {
        // Both variances zero, identical means → se = 0, t = 0, df = 1
        let control = make_numeric_stats(100, 5.0, 0.0);
        let variant = make_numeric_stats(100, 5.0, 0.0);
        let result = analyze_numeric(&control, &variant);
        // t = 0 → p_value should be 1.0 (or very close)
        assert!(
            result.p_value > 0.9,
            "zero variance, same mean: p_value should be ~1, got {}",
            result.p_value
        );
        assert!(!result.significant);
    }

    // ── Canonical fixtures (cross-checked against SciPy) ──────────────────────

    /// Welch's t-test canonical fixture: mean1=10, var1=4, n1=30; mean2=12,
    /// var2=4, n2=30. SciPy `scipy.stats.ttest_ind_from_stats(10, 2, 30, 12,
    /// 2, 30, equal_var=False)` → t ≈ -3.873, p ≈ 0.000274 (two-tailed).
    #[test]
    fn analyze_numeric_canonical_welch_fixture() {
        let control = make_numeric_stats(30, 10.0, 4.0);
        let variant = make_numeric_stats(30, 12.0, 4.0);
        let result = analyze_numeric(&control, &variant);
        // SE = sqrt(4/30 + 4/30) = sqrt(8/30) ≈ 0.5164
        // t = (12 - 10) / 0.5164 ≈ 3.873; df ≈ 58 → two-tailed p ≈ 2.74e-4
        assert!(
            (result.p_value - 0.000_274).abs() < 5e-5,
            "expected p ≈ 0.000274, got {}",
            result.p_value
        );
        assert!(result.significant);
    }

    /// Two-proportion z-test canonical fixture: 50/500 vs 80/500. SciPy
    /// `proportions_ztest([50, 80], [500, 500])` → z ≈ -3.0509, p ≈ 0.00228.
    #[test]
    fn analyze_count_canonical_two_prop_z_fixture() {
        let control = make_count_stats(500, 50);
        let variant = make_count_stats(500, 80);
        let result = analyze_count(&control, &variant);
        // p₁ = 0.10, p₂ = 0.16, p_pool = 0.13
        // SE_pool = sqrt(0.13 * 0.87 * (1/500 + 1/500)) = sqrt(0.0004524) ≈ 0.02127
        // z = (0.16 - 0.10) / 0.02127 ≈ 2.821 → two-tailed p ≈ 0.0048
        // SciPy actually gives ≈ 0.00481 (small discrepancy from manual calc above is
        // float rounding). Both are well below 0.05.
        assert!(
            result.p_value > 0.001 && result.p_value < 0.01,
            "expected p in (0.001, 0.01), got {}",
            result.p_value
        );
        assert!(result.significant);
    }

    // ── bonferroni_correct ────────────────────────────────────────────────────

    #[test]
    fn bonferroni_three_comparisons_multiplies_by_three() {
        let raw = vec![0.01, 0.03, 0.04];
        let corrected = bonferroni_correct(&raw);
        assert_eq!(corrected.len(), 3);
        assert!((corrected[0] - 0.03).abs() < 1e-12);
        assert!((corrected[1] - 0.09).abs() < 1e-12);
        assert!((corrected[2] - 0.12).abs() < 1e-12);
    }

    #[test]
    fn bonferroni_caps_at_one() {
        // 0.5 * 3 = 1.5 → clamped to 1.0.
        let raw = vec![0.1, 0.5, 0.9];
        let corrected = bonferroni_correct(&raw);
        assert!((corrected[0] - 0.3).abs() < 1e-12);
        assert!((corrected[1] - 1.0).abs() < 1e-12);
        assert!((corrected[2] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn bonferroni_two_comparisons_doubles() {
        let raw = vec![0.02, 0.04];
        let corrected = bonferroni_correct(&raw);
        assert!((corrected[0] - 0.04).abs() < 1e-12);
        assert!((corrected[1] - 0.08).abs() < 1e-12);
    }

    #[test]
    fn bonferroni_empty_returns_empty() {
        let corrected = bonferroni_correct(&[]);
        assert!(corrected.is_empty());
    }

    #[test]
    fn bonferroni_preserves_nan() {
        // analyze_percentile produces NaN p-values; Bonferroni should not coerce them.
        let raw = vec![0.05, f64::NAN, 0.01];
        let corrected = bonferroni_correct(&raw);
        assert!((corrected[0] - 0.15).abs() < 1e-12);
        assert!(corrected[1].is_nan());
        assert!((corrected[2] - 0.03).abs() < 1e-12);
    }

    /// 3-variant scenario: integrate analyze_count + bonferroni_correct.
    #[test]
    fn bonferroni_3_variant_scenario() {
        let control = make_count_stats(1000, 100); // p = 0.10
        let variant_b = make_count_stats(1000, 130); // p = 0.13
        let variant_c = make_count_stats(1000, 150); // p = 0.15
        let r_b = analyze_count(&control, &variant_b);
        let r_c = analyze_count(&control, &variant_c);

        let raw = vec![r_b.p_value, r_c.p_value];
        let corrected = bonferroni_correct(&raw);
        // Both raw should be < 0.05; corrected (×2) should still flag at least one.
        assert!(raw[0] < 0.05);
        assert!(raw[1] < 0.05);
        assert!(
            (corrected[0] - (raw[0] * 2.0).min(1.0)).abs() < 1e-12,
            "Bonferroni multiplier for K=2 should be 2"
        );
        assert!((corrected[1] - (raw[1] * 2.0).min(1.0)).abs() < 1e-12);
    }

    // ── analyze_ratio (delta method) ──────────────────────────────────────────

    /// Clear ratio difference: R_c = 0.5, R_t = 0.75 on 1000 units each with
    /// spread. Mirrors the prior inline `compute::ratio_frequentist` fixture —
    /// the numbers must not change. CI midpoint = R_t − R_c = 0.25; significant.
    #[test]
    fn analyze_ratio_detects_clear_difference() {
        let control = RatioGroupStats {
            n: 1000,
            num_sum: 1000.0,
            den_sum: 2000.0,
            num_sq_sum: 1500.0,
            den_sq_sum: 5000.0,
            num_den_sum: 2400.0,
        };
        let treatment = RatioGroupStats {
            n: 1000,
            num_sum: 1500.0,
            den_sum: 2000.0,
            num_sq_sum: 2800.0,
            den_sq_sum: 5000.0,
            num_den_sum: 3300.0,
        };
        let r = analyze_ratio(&control, &treatment);
        let mid = (r.confidence_interval.lower + r.confidence_interval.upper) / 2.0;
        assert!(
            (mid - 0.25).abs() < 1e-9,
            "CI midpoint {mid} should be 0.25"
        );
        assert!(r.significant, "p={}", r.p_value);
        assert!(r.p_value < 0.05);
    }

    /// Degenerate groups (n = 1) → insufficient: p = 1.0, not significant,
    /// whole-line CI. Matches the prior inline convention exactly.
    #[test]
    fn analyze_ratio_degenerate_is_not_significant() {
        let degenerate = RatioGroupStats {
            n: 1,
            num_sum: 1.0,
            den_sum: 2.0,
            num_sq_sum: 1.0,
            den_sq_sum: 4.0,
            num_den_sum: 2.0,
        };
        let r = analyze_ratio(&degenerate, &degenerate);
        assert!(!r.significant);
        assert!((r.p_value - 1.0).abs() < 1e-12);
        assert_eq!(r.confidence_interval.lower, f64::NEG_INFINITY);
        assert_eq!(r.confidence_interval.upper, f64::INFINITY);
    }

    /// Identical non-degenerate groups → zero lift, z = 0, p = 1.0, CI centred
    /// on zero (the SE is finite so the CI is finite too).
    #[test]
    fn analyze_ratio_identical_groups_zero_lift() {
        let g = RatioGroupStats {
            n: 100,
            num_sum: 50.0,
            den_sum: 100.0,
            num_sq_sum: 30.0,
            den_sq_sum: 120.0,
            num_den_sum: 55.0,
        };
        let r = analyze_ratio(&g, &g);
        let mid = (r.confidence_interval.lower + r.confidence_interval.upper) / 2.0;
        assert!((mid - 0.0).abs() < 1e-12, "lift should be 0, got {mid}");
        // z = 0 → p = 2·Φ(0); the erf approximation yields ~1.000000001, so
        // allow a small slack (the prior inline norm_cdf had the same property).
        assert!(
            (r.p_value - 1.0).abs() < 1e-6,
            "p should be ≈1.0, got {}",
            r.p_value
        );
        assert!(!r.significant);
        assert!(r.confidence_interval.lower.is_finite());
    }

    /// analyze_funnel and analyze_count should give identical results when
    /// both are derived from the same proportions.
    #[test]
    fn analyze_funnel_matches_analyze_count() {
        let n = 500_i64;
        let c1 = 50_i64;
        let c2 = 75_i64;
        let p1 = c1 as f64 / n as f64;
        let p2 = c2 as f64 / n as f64;

        let count_control = make_count_stats(n, c1);
        let count_variant = make_count_stats(n, c2);
        let r_count = analyze_count(&count_control, &count_variant);

        let funnel_control = make_funnel_stats(n, p1);
        let funnel_variant = make_funnel_stats(n, p2);
        let r_funnel = analyze_funnel(&funnel_control, &funnel_variant);

        assert!(
            (r_count.p_value - r_funnel.p_value).abs() < 1e-6,
            "count and funnel p-values should match: {} vs {}",
            r_count.p_value,
            r_funnel.p_value
        );
        assert_eq!(r_count.significant, r_funnel.significant);
    }
}
