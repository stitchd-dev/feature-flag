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

use super::{ConfidenceInterval, FrequentistResult, VariantStats};

// ── Normal CDF helpers ────────────────────────────────────────────────────────

/// Approximation of the error function using Horner's method (Abramowitz &
/// Stegun, formula 7.1.26).  Maximum absolute error < 1.5 × 10⁻⁷.
fn erf(x: f64) -> f64 {
    // Preserve sign
    let sign = if x < 0.0 { -1.0_f64 } else { 1.0_f64 };
    let x = x.abs();

    let t = 1.0 / (1.0 + 0.3275911 * x);
    let poly = t
        * (0.254829592
            + t * (-0.284496736
                + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
    sign * (1.0 - poly * (-x * x).exp())
}

/// Standard normal CDF:  Φ(z) = 0.5 · (1 + erf(z / √2))
#[inline]
fn norm_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

/// Two-tailed p-value from the standard normal distribution.
#[inline]
fn z_to_p(z: f64) -> f64 {
    2.0 * norm_cdf(-z.abs())
}

// ── Student's t-distribution (for Welch's t-test) ─────────────────────────

/// Regularised incomplete beta function I_x(a, b) using a continued-fraction
/// expansion (Numerical Recipes, §6.4).  Accurate to ~10⁻¹⁰ for typical
/// degree-of-freedom values seen in A/B tests.
fn regularised_incomplete_beta(x: f64, a: f64, b: f64) -> f64 {
    if x < 0.0 || x > 1.0 {
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
        return std::f64::consts::PI.ln()
            - (std::f64::consts::PI * x).sin().ln()
            - lgamma(1.0 - x);
    } else {
        x - 1.0
    };

    let mut sum = C[0];
    for (i, &c) in C[1..].iter().enumerate() {
        sum += c / (x + i as f64 + 1.0);
    }

    let tmp = x + G + 0.5;
    (2.0 * std::f64::consts::PI).sqrt().ln()
        + sum.ln()
        + (x + 0.5) * tmp.ln()
        - tmp
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

// ── Public API ────────────────────────────────────────────────────────────────

/// Two-proportion z-test for count / binary metrics.
///
/// Uses the pooled proportion under H₀ to compute the z-statistic, then
/// derives a two-tailed p-value via the standard normal CDF.  The 95 %
/// confidence interval is computed on the *lift* (p₂ − p₁) using the
/// unpooled standard error.
///
/// # Panics
/// Panics if `control.sample_size` or `variant.sample_size` is zero, or if
/// `conversions` is `None` on either variant.
pub fn analyze_count(control: &VariantStats, variant: &VariantStats) -> FrequentistResult {
    let n1 = control.sample_size as f64;
    let n2 = variant.sample_size as f64;
    let c1 = control.conversions.expect("analyze_count: control.conversions must be Some") as f64;
    let c2 = variant.conversions.expect("analyze_count: variant.conversions must be Some") as f64;

    let p1 = c1 / n1;
    let p2 = c2 / n2;

    let p_pool = (c1 + c2) / (n1 + n2);
    let se_pool = (p_pool * (1.0 - p_pool) * (1.0 / n1 + 1.0 / n2)).sqrt();

    let z = if se_pool == 0.0 { 0.0 } else { (p2 - p1) / se_pool };
    let p_value = z_to_p(z);

    // 95 % CI on lift using unpooled SE
    let se_lift = (p1 * (1.0 - p1) / n1 + p2 * (1.0 - p2) / n2).sqrt();
    let margin = 1.96 * se_lift;
    let lift = p2 - p1;

    FrequentistResult {
        p_value,
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
/// # Panics
/// Panics if `mean` or `variance` is `None` on either variant, or if
/// `sample_size` is zero.
pub fn analyze_numeric(control: &VariantStats, variant: &VariantStats) -> FrequentistResult {
    let n1 = control.sample_size as f64;
    let n2 = variant.sample_size as f64;
    let mean1 = control.mean.expect("analyze_numeric: control.mean must be Some");
    let mean2 = variant.mean.expect("analyze_numeric: variant.mean must be Some");
    let var1 = control.variance.expect("analyze_numeric: control.variance must be Some");
    let var2 = variant.variance.expect("analyze_numeric: variant.variance must be Some");

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
        confidence_interval: ConfidenceInterval {
            lower: diff - margin,
            upper: diff + margin,
        },
        significant: p_value < 0.05,
    }
}

/// Bootstrap-based analysis for percentile metrics.
///
/// Delegates to [`super::bootstrap::bootstrap_ci`] for the confidence
/// interval.  Because bootstrap produces no analytical p-value, `p_value` is
/// set to [`f64::NAN`] and `significant` is always `false` — callers should
/// use CI overlap to assess significance for percentile metrics.
///
/// # Arguments
/// * `control_samples`  – raw observations for the control variant
/// * `variant_samples`  – raw observations for the treatment variant
/// * `percentile`       – target percentile in `[0, 100]`
pub fn analyze_percentile(
    control_samples: &[f64],
    variant_samples: &[f64],
    percentile: f64,
) -> FrequentistResult {
    let ci = super::bootstrap::bootstrap_ci(control_samples, variant_samples, percentile, 1_000);
    FrequentistResult {
        p_value: f64::NAN,
        confidence_interval: ci,
        significant: false,
    }
}

/// Two-proportion z-test for the *final-step conversion rate* of a funnel metric.
///
/// Uses `VariantStats::conversion_rate` as the proportion, which represents
/// the end-to-end funnel completion rate.  The sample-size denominator is the
/// top-of-funnel exposure count (`sample_size`).
///
/// # Panics
/// Panics if `conversion_rate` is `None` on either variant, or if
/// `sample_size` is zero.
pub fn analyze_funnel(control: &VariantStats, variant: &VariantStats) -> FrequentistResult {
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

    let z = if se_pool == 0.0 { 0.0 } else { (p2 - p1) / se_pool };
    let p_value = z_to_p(z);

    let se_lift = (p1 * (1.0 - p1) / n1 + p2 * (1.0 - p2) / n2).sqrt();
    let margin = 1.96 * se_lift;
    let lift = p2 - p1;

    FrequentistResult {
        p_value,
        confidence_interval: ConfidenceInterval {
            lower: lift - margin,
            upper: lift + margin,
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
        assert!((p - 0.975).abs() < 1e-3, "Φ(1.96) should be ≈ 0.975, got {p}");
    }

    #[test]
    fn norm_cdf_symmetric() {
        for z in [-3.0, -1.96, -1.0, 0.0, 1.0, 1.96, 3.0] {
            let p = norm_cdf(z) + norm_cdf(-z);
            assert!((p - 1.0).abs() < 1e-6, "Φ(z)+Φ(-z) should be 1 for z={z}, got {p}");
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
        assert!((p - 0.05).abs() < 0.01, "t→p with df=1000 should be ≈0.05, got {p}");
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
        let midpoint =
            (result.confidence_interval.lower + result.confidence_interval.upper) / 2.0;
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
        assert!(result.p_value < 0.001, "p_value should be very small, got {}", result.p_value);
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
        assert!(result.p_value > 0.9, "p_value should be very high, got {}", result.p_value);
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
