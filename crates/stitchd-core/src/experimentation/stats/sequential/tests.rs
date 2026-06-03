//! Tests for the sequential (always-valid) statistics engine.
//!
//! The headline guarantees are *statistical*, so the simulation tests use a
//! seeded RNG (a Linear Congruential Generator, matching the idiom in
//! [`super::super::bayesian`]) with modest sim counts and generous tolerances
//! to stay deterministic and non-flaky.

use super::*;
use crate::experimentation::stats::{Percentiles, VariantStats};

// ── Seeded RNG (matches the bayesian.rs LCG idiom) ────────────────────────────

/// Minimal LCG (Knuth parameters), same as `bayesian.rs`.
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
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    /// Uniform in `(0, 1)` (avoids exactly 0 so `ln` is safe in Box-Muller).
    fn next_f64(&mut self) -> f64 {
        let u = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        u.max(1e-300)
    }

    /// One standard-normal draw via Box-Muller.
    fn next_normal(&mut self) -> f64 {
        let u1 = self.next_f64();
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }
}

// ── Naive (fixed-horizon) per-look z-test, for the inflation contrast ─────────

/// Standard-normal CDF via the same erf approximation used in `frequentist.rs`.
fn norm_cdf(z: f64) -> f64 {
    fn erf(x: f64) -> f64 {
        let sign = if x < 0.0 { -1.0 } else { 1.0 };
        let x = x.abs();
        let t = 1.0 / (1.0 + 0.3275911 * x);
        let poly = t
            * (0.254829592
                + t * (-0.284496736 + t * (1.421413741 + t * (-1.453152027 + t * 1.061405429))));
        sign * (1.0 - poly * (-x * x).exp())
    }
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

/// Two-tailed naive z-test p-value for `delta_hat / se`.
fn naive_z_p(delta_hat: f64, se: f64) -> f64 {
    // `se` here is always `sqrt(non-negative)`, so it is never NaN; guard zero
    // / (impossible) negative.
    if se <= 0.0 {
        return 1.0;
    }
    let z = (delta_hat / se).abs();
    2.0 * norm_cdf(-z)
}

// ── Simulation harness ────────────────────────────────────────────────────────

/// Per-arm sample size at look index `look` (0-based), on a geometric-ish
/// schedule from `start` up to `max`.
fn look_sizes(num_looks: usize, start: i64, step: i64) -> Vec<i64> {
    (0..num_looks).map(|i| start + step * i as i64).collect()
}

/// Run one simulated experiment with a fixed true effect on a mean-difference
/// metric. At each look we have `n` iid normal observations per arm with unit
/// variance; the control mean is 0 and the treatment mean is `true_effect`.
///
/// Returns, per look, the running-min always-valid p (sequential) and the naive
/// per-look z p. Uses incremental sufficient statistics so successive looks are
/// nested (a real peeking schedule), not independent samples.
struct SimOutcome {
    /// Did the running-min always-valid p ever drop ≤ alpha across all looks?
    seq_rejected: bool,
    /// Did the naive per-look z p ever drop ≤ alpha across all looks?
    naive_rejected: bool,
    /// Did the confidence sequence cover the true effect at *every* look?
    cs_covered_all: bool,
    /// Running-min p sequence (to check monotonicity).
    running_p: Vec<f64>,
}

fn simulate_one(
    rng: &mut Lcg,
    true_effect: f64,
    sizes: &[i64],
    cfg: &SequentialConfig,
) -> SimOutcome {
    // Incremental accumulators per arm.
    let mut sum_c = 0.0;
    let mut sumsq_c = 0.0;
    let mut sum_t = 0.0;
    let mut sumsq_t = 0.0;
    let mut n_so_far: i64 = 0;

    let mut prev_p = 1.0;
    let mut seq_rejected = false;
    let mut naive_rejected = false;
    let mut cs_covered_all = true;
    let mut running_p = Vec::with_capacity(sizes.len());

    for &n in sizes {
        // Draw the *new* observations to grow each arm up to n.
        for _ in n_so_far..n {
            let x_c = rng.next_normal(); // control ~ N(0, 1)
            let x_t = true_effect + rng.next_normal(); // treatment ~ N(eff, 1)
            sum_c += x_c;
            sumsq_c += x_c * x_c;
            sum_t += x_t;
            sumsq_t += x_t * x_t;
        }
        n_so_far = n;

        let nf = n as f64;
        let mean_c = sum_c / nf;
        let mean_t = sum_t / nf;
        // Sample variance (n-1 denominator) per arm.
        let var_c = (sumsq_c - nf * mean_c * mean_c) / (nf - 1.0);
        let var_t = (sumsq_t - nf * mean_t * mean_t) / (nf - 1.0);

        let delta_hat = mean_t - mean_c;
        let se = (var_c / nf + var_t / nf).sqrt();

        let res = sequential_test(delta_hat, se, n, cfg, prev_p);
        prev_p = res.always_valid_p;
        running_p.push(res.always_valid_p);

        if res.p_crossed {
            seq_rejected = true;
        }
        // CS coverage: does the (finite) interval contain the true effect?
        if !res.insufficient_data {
            let covered = res.ci_lower <= true_effect && true_effect <= res.ci_upper;
            if !covered {
                cs_covered_all = false;
            }
        }

        if naive_z_p(delta_hat, se) <= cfg.alpha {
            naive_rejected = true;
        }
    }

    SimOutcome {
        seq_rejected,
        naive_rejected,
        cs_covered_all,
        running_p,
    }
}

// ── 1. No-inflation under H₀ (the headline guarantee) ─────────────────────────

#[test]
fn no_inflation_under_h0() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 1,
    };
    let sims = 2000;
    let sizes = look_sizes(20, 30, 30); // 20 looks, n = 30..600 per arm
    let mut rng = Lcg::new(0xC0FFEE);

    let mut rejections = 0;
    for _ in 0..sims {
        let out = simulate_one(&mut rng, 0.0, &sizes, &cfg);
        if out.seq_rejected {
            rejections += 1;
        }
    }
    let rate = rejections as f64 / sims as f64;
    assert!(
        rate <= cfg.alpha + 0.02,
        "always-valid H0 rejection rate {rate} should be ≤ alpha+0.02 ({})",
        cfg.alpha + 0.02
    );
}

// ── 2. Fixed-horizon inflation contrast ───────────────────────────────────────

#[test]
fn naive_peeking_inflates_far_above_alpha() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 1,
    };
    let sims = 2000;
    let sizes = look_sizes(20, 30, 30);
    let mut rng = Lcg::new(0xBADF00D);

    let mut naive_rejections = 0;
    let mut seq_rejections = 0;
    for _ in 0..sims {
        let out = simulate_one(&mut rng, 0.0, &sizes, &cfg);
        if out.naive_rejected {
            naive_rejections += 1;
        }
        if out.seq_rejected {
            seq_rejections += 1;
        }
    }
    let naive_rate = naive_rejections as f64 / sims as f64;
    let seq_rate = seq_rejections as f64 / sims as f64;

    assert!(
        naive_rate > 0.15,
        "naive per-look peeking should grossly inflate (>0.15); got {naive_rate}"
    );
    assert!(
        seq_rate < naive_rate,
        "sequential rate {seq_rate} must be far below naive rate {naive_rate}"
    );
}

// ── 3. Power under H₁ ─────────────────────────────────────────────────────────

#[test]
fn power_under_h1() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 1,
    };
    let sims = 1000;
    // Effect of 0.3 SD; with up to ~1200/arm the mSPRT should detect it almost
    // always somewhere along the peeking schedule.
    let sizes = look_sizes(20, 60, 60);
    let mut rng = Lcg::new(0x5EED1);

    let mut detected = 0;
    for _ in 0..sims {
        let out = simulate_one(&mut rng, 0.3, &sizes, &cfg);
        if out.seq_rejected {
            detected += 1;
        }
    }
    let power = detected as f64 / sims as f64;
    assert!(
        power > 0.8,
        "power under a real effect should exceed 0.8; got {power}"
    );
}

// ── 4. Confidence-sequence coverage ───────────────────────────────────────────

#[test]
fn confidence_sequence_covers_truth_at_all_looks() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 1,
    };
    let sims = 1000;
    let true_effect = 0.2;
    let sizes = look_sizes(20, 40, 40);
    let mut rng = Lcg::new(0xC07E6);

    let mut all_covered = 0;
    for _ in 0..sims {
        let out = simulate_one(&mut rng, true_effect, &sizes, &cfg);
        if out.cs_covered_all {
            all_covered += 1;
        }
    }
    let coverage = all_covered as f64 / sims as f64;
    // Anytime-valid coverage: P(CS contains truth at *every* look) ≥ 1 − alpha.
    assert!(
        coverage >= 1.0 - cfg.alpha - 0.02,
        "simultaneous CS coverage {coverage} should be ≥ {} (1-alpha-tol)",
        1.0 - cfg.alpha - 0.02
    );
}

// ── 5. Running-min monotonicity ───────────────────────────────────────────────

#[test]
fn running_min_p_is_monotone_non_increasing() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 1,
    };
    let sizes = look_sizes(25, 20, 25);
    let mut rng = Lcg::new(0x3171C);

    // Check across a mix of H0 and H1 simulations.
    for (seed_eff, &effect) in [0.0, 0.15, 0.4].iter().enumerate() {
        let mut local = Lcg::new(rng.next_u64() ^ seed_eff as u64);
        for _ in 0..200 {
            let out = simulate_one(&mut local, effect, &sizes, &cfg);
            for w in out.running_p.windows(2) {
                assert!(
                    w[1] <= w[0] + 1e-12,
                    "running-min p must be non-increasing: {} then {}",
                    w[0],
                    w[1]
                );
            }
        }
    }
}

#[test]
fn sequential_test_running_min_never_increases_unit() {
    // Direct unit check of the running-min plumbing: feed a sequence where a
    // later look would have a *higher* per-look p; the reported value sticks at
    // the previous minimum.
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 1,
    };
    // Look 1: strong effect → small p.
    let r1 = sequential_test(0.5, 0.05, 100, &cfg, 1.0);
    assert!(r1.always_valid_p < 0.5);
    // Look 2: estimate regressed toward 0 → larger per-look p, but running-min
    // must not increase.
    let r2 = sequential_test(0.0, 0.05, 200, &cfg, r1.always_valid_p);
    assert!(
        r2.always_valid_p <= r1.always_valid_p + 1e-15,
        "running-min increased: {} -> {}",
        r1.always_valid_p,
        r2.always_valid_p
    );
    assert_eq!(r2.always_valid_p, r1.always_valid_p);
}

// ── Core-engine direct properties ─────────────────────────────────────────────

#[test]
fn p_look_is_one_when_no_evidence() {
    // delta_hat = 0 → lambda = sqrt(se²/(se²+τ²)) < 1 → 1/lambda > 1 → p = 1.
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 1,
    };
    let res = sequential_test(0.0, 0.1, 1000, &cfg, 1.0);
    assert!(!res.insufficient_data);
    assert!((res.always_valid_p - 1.0).abs() < 1e-12);
    assert!(!res.p_crossed);
}

#[test]
fn ci_brackets_estimate_and_widens_with_smaller_tau() {
    let cfg_wide = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 1,
    };
    let cfg_narrow = SequentialConfig {
        alpha: 0.05,
        tau_squared: 0.01,
        min_sample_size: 1,
    };
    let r_wide = sequential_test(0.3, 0.1, 500, &cfg_wide, 1.0);
    let r_narrow = sequential_test(0.3, 0.1, 500, &cfg_narrow, 1.0);

    // CI is centred on delta_hat.
    let mid_wide = (r_wide.ci_lower + r_wide.ci_upper) / 2.0;
    assert!((mid_wide - 0.3).abs() < 1e-9, "CI not centred on delta_hat");

    let width_wide = r_wide.ci_upper - r_wide.ci_lower;
    let width_narrow = r_narrow.ci_upper - r_narrow.ci_lower;
    assert!(
        width_narrow > width_wide,
        "smaller τ² should give a wider anytime CI: narrow={width_narrow} wide={width_wide}"
    );
}

#[test]
fn strong_evidence_crosses_alpha() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 1,
    };
    // delta_hat 6 SEs out — overwhelming.
    let res = sequential_test(0.6, 0.1, 1000, &cfg, 1.0);
    assert!(res.p_crossed, "p={}", res.always_valid_p);
    assert!(res.always_valid_p < 0.05);
    // CI should exclude 0 for such strong evidence.
    assert!(
        res.ci_lower > 0.0,
        "CI lower {} should exclude 0",
        res.ci_lower
    );
}

// ── Edge cases ────────────────────────────────────────────────────────────────

#[test]
fn below_min_sample_size_is_insufficient() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 100,
    };
    let res = sequential_test(0.5, 0.05, 99, &cfg, 1.0);
    assert!(res.insufficient_data);
    assert_eq!(res.always_valid_p, 1.0);
    assert!(!res.p_crossed);
    assert_eq!(res.ci_lower, f64::NEG_INFINITY);
    assert_eq!(res.ci_upper, f64::INFINITY);
    assert_eq!(res.method, "msprt");
}

#[test]
fn non_positive_se_is_insufficient() {
    let cfg = SequentialConfig::default();
    for se in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let res = sequential_test(0.5, se, 1000, &cfg, 1.0);
        assert!(res.insufficient_data, "se={se} should be insufficient");
        assert_eq!(res.always_valid_p, 1.0);
    }
}

#[test]
fn non_positive_tau_is_insufficient() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 0.0,
        min_sample_size: 1,
    };
    let res = sequential_test(0.5, 0.05, 1000, &cfg, 1.0);
    assert!(res.insufficient_data);
}

#[test]
fn non_finite_delta_is_insufficient() {
    let cfg = SequentialConfig::default();
    let res = sequential_test(f64::NAN, 0.05, 1000, &cfg, 1.0);
    assert!(res.insufficient_data);
}

// ── Method label ──────────────────────────────────────────────────────────────

#[test]
fn method_is_msprt() {
    let cfg = SequentialConfig::default();
    let res = sequential_test(0.3, 0.1, 1000, &cfg, 1.0);
    assert_eq!(res.method, "msprt");
}

// ── Per-family adapters ───────────────────────────────────────────────────────

fn count_stats(n: i64, conversions: i64) -> VariantStats {
    VariantStats {
        sample_size: n,
        conversions: Some(conversions),
        mean: None,
        variance: None,
        conversion_rate: None,
        percentiles: None,
    }
}

fn numeric_stats(n: i64, mean: f64, variance: f64) -> VariantStats {
    VariantStats {
        sample_size: n,
        conversions: None,
        mean: Some(mean),
        variance: Some(variance),
        conversion_rate: None,
        percentiles: None,
    }
}

fn funnel_stats(n: i64, rate: f64) -> VariantStats {
    VariantStats {
        sample_size: n,
        conversions: None,
        mean: None,
        variance: None,
        conversion_rate: Some(rate),
        percentiles: None,
    }
}

#[test]
fn sequential_count_large_effect_crosses() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 0.01, // effects here are on the proportion scale (~0.05)
        min_sample_size: 100,
    };
    // p1 = 0.10, p2 = 0.20, n = 2000 → strong.
    let control = count_stats(2000, 200);
    let variant = count_stats(2000, 400);
    let res = sequential_count(&control, &variant, &cfg, 1.0);
    assert!(!res.insufficient_data);
    assert!(res.p_crossed, "p={}", res.always_valid_p);
    // CI on the lift should be positive-only and centred near +0.10.
    let mid = (res.ci_lower + res.ci_upper) / 2.0;
    assert!((mid - 0.10).abs() < 1e-6);
}

#[test]
fn sequential_count_no_effect_does_not_cross() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 0.01,
        min_sample_size: 100,
    };
    let control = count_stats(2000, 200);
    let variant = count_stats(2000, 200);
    let res = sequential_count(&control, &variant, &cfg, 1.0);
    assert!(!res.insufficient_data);
    assert!(!res.p_crossed);
    assert!((res.always_valid_p - 1.0).abs() < 1e-9);
}

#[test]
fn sequential_count_below_min_is_insufficient() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 0.01,
        min_sample_size: 500,
    };
    // Treatment arm only has 100 → min(n1,n2)=100 < 500.
    let control = count_stats(2000, 200);
    let variant = count_stats(100, 30);
    let res = sequential_count(&control, &variant, &cfg, 1.0);
    assert!(res.insufficient_data);
}

#[test]
fn sequential_count_missing_conversions_is_insufficient() {
    let cfg = SequentialConfig::default();
    let control = numeric_stats(1000, 1.0, 1.0); // no conversions field
    let variant = count_stats(1000, 100);
    let res = sequential_count(&control, &variant, &cfg, 1.0);
    assert!(res.insufficient_data);
}

#[test]
fn sequential_numeric_large_effect_crosses() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 100,
    };
    // mean 100 vs 110, var 100, n 500 → t ≈ 15.8, overwhelming.
    let control = numeric_stats(500, 100.0, 100.0);
    let variant = numeric_stats(500, 110.0, 100.0);
    let res = sequential_numeric(&control, &variant, &cfg, 1.0);
    assert!(res.p_crossed, "p={}", res.always_valid_p);
    let mid = (res.ci_lower + res.ci_upper) / 2.0;
    assert!((mid - 10.0).abs() < 1e-6);
}

#[test]
fn sequential_numeric_tiny_effect_does_not_cross() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 100,
    };
    let control = numeric_stats(500, 100.0, 25.0);
    let variant = numeric_stats(500, 100.01, 25.0);
    let res = sequential_numeric(&control, &variant, &cfg, 1.0);
    assert!(!res.p_crossed);
}

#[test]
fn sequential_numeric_below_min_is_insufficient() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 1000,
    };
    let control = numeric_stats(500, 100.0, 100.0);
    let variant = numeric_stats(500, 110.0, 100.0);
    let res = sequential_numeric(&control, &variant, &cfg, 1.0);
    assert!(res.insufficient_data);
}

#[test]
fn sequential_numeric_missing_fields_is_insufficient() {
    let cfg = SequentialConfig::default();
    let control = count_stats(1000, 100); // no mean/variance
    let variant = count_stats(1000, 120);
    let res = sequential_numeric(&control, &variant, &cfg, 1.0);
    assert!(res.insufficient_data);
}

#[test]
fn sequential_funnel_matches_count_on_same_proportions() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 0.01,
        min_sample_size: 100,
    };
    let n = 2000;
    let (c1, c2) = (200, 400);
    let count_res = sequential_count(&count_stats(n, c1), &count_stats(n, c2), &cfg, 1.0);
    let funnel_res = sequential_funnel(
        &funnel_stats(n, c1 as f64 / n as f64),
        &funnel_stats(n, c2 as f64 / n as f64),
        &cfg,
        1.0,
    );
    assert!((count_res.always_valid_p - funnel_res.always_valid_p).abs() < 1e-9);
    assert_eq!(count_res.p_crossed, funnel_res.p_crossed);
    assert!((count_res.ci_lower - funnel_res.ci_lower).abs() < 1e-9);
    assert!((count_res.ci_upper - funnel_res.ci_upper).abs() < 1e-9);
}

#[test]
fn sequential_funnel_below_min_is_insufficient() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 0.01,
        min_sample_size: 5000,
    };
    let res = sequential_funnel(
        &funnel_stats(2000, 0.1),
        &funnel_stats(2000, 0.2),
        &cfg,
        1.0,
    );
    assert!(res.insufficient_data);
}

// ── Ratio adapter ─────────────────────────────────────────────────────────────

/// Build `RatioGroupStats` from explicit `(numerator, denominator)` pairs.
fn ratio_from_pairs(pairs: &[(f64, f64)]) -> RatioGroupStats {
    let mut g = RatioGroupStats {
        n: pairs.len() as i64,
        num_sum: 0.0,
        den_sum: 0.0,
        num_sq_sum: 0.0,
        den_sq_sum: 0.0,
        num_den_sum: 0.0,
    };
    for &(num, den) in pairs {
        g.num_sum += num;
        g.den_sum += den;
        g.num_sq_sum += num * num;
        g.den_sq_sum += den * den;
        g.num_den_sum += num * den;
    }
    g
}

#[test]
fn sequential_ratio_detects_large_difference() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 100,
    };
    let mut rng = Lcg::new(0x2A710);
    // Control ratio ≈ 0.5, variant ratio ≈ 1.5; denominators ~5, jittered.
    let mut c_pairs = Vec::new();
    let mut v_pairs = Vec::new();
    for _ in 0..400 {
        let den_c = 5.0 + rng.next_normal() * 0.5;
        let den_v = 5.0 + rng.next_normal() * 0.5;
        c_pairs.push((0.5 * den_c + rng.next_normal() * 0.2, den_c));
        v_pairs.push((1.5 * den_v + rng.next_normal() * 0.2, den_v));
    }
    let res = sequential_ratio(
        &ratio_from_pairs(&c_pairs),
        &ratio_from_pairs(&v_pairs),
        &cfg,
        1.0,
    );
    assert!(!res.insufficient_data);
    assert!(
        res.p_crossed,
        "ratio diff should be detected; p={}",
        res.always_valid_p
    );
    // Effect ≈ 1.0; CI should be centred near +1.0 and exclude 0.
    let mid = (res.ci_lower + res.ci_upper) / 2.0;
    assert!((mid - 1.0).abs() < 0.2, "CI midpoint {mid} not near 1.0");
    assert!(res.ci_lower > 0.0);
}

#[test]
fn sequential_ratio_no_difference_does_not_cross() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 100,
    };
    let mut rng = Lcg::new(0x9311E2);
    let mut c_pairs = Vec::new();
    let mut v_pairs = Vec::new();
    for _ in 0..400 {
        let den_c = 5.0 + rng.next_normal() * 0.5;
        let den_v = 5.0 + rng.next_normal() * 0.5;
        c_pairs.push((0.8 * den_c + rng.next_normal() * 0.2, den_c));
        v_pairs.push((0.8 * den_v + rng.next_normal() * 0.2, den_v));
    }
    let res = sequential_ratio(
        &ratio_from_pairs(&c_pairs),
        &ratio_from_pairs(&v_pairs),
        &cfg,
        1.0,
    );
    assert!(!res.insufficient_data);
    assert!(
        !res.p_crossed,
        "no true diff should not cross; p={}",
        res.always_valid_p
    );
}

#[test]
fn sequential_ratio_degenerate_group_is_insufficient() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 1,
    };
    // n < 2 → degenerate.
    let one = ratio_from_pairs(&[(1.0, 2.0)]);
    let ok = ratio_from_pairs(&[(1.0, 2.0), (1.1, 2.1), (0.9, 1.9)]);
    assert!(sequential_ratio(&one, &ok, &cfg, 1.0).insufficient_data);

    // den_sum ≤ 0 → degenerate.
    let zero_den = ratio_from_pairs(&[(0.0, 0.0), (0.0, 0.0)]);
    assert!(sequential_ratio(&zero_den, &ok, &cfg, 1.0).insufficient_data);
}

#[test]
fn sequential_ratio_below_min_sample_is_insufficient() {
    let cfg = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 1000,
    };
    let c = ratio_from_pairs(&[(1.0, 2.0), (1.1, 2.1), (0.9, 1.9)]);
    let v = ratio_from_pairs(&[(3.0, 2.0), (3.1, 2.1), (2.9, 1.9)]);
    // n = 3 < 1000.
    assert!(sequential_ratio(&c, &v, &cfg, 1.0).insufficient_data);
}

// ── Multiplicity: split_alpha ─────────────────────────────────────────────────

#[test]
fn split_alpha_divides_by_k_minus_one() {
    let base = SequentialConfig {
        alpha: 0.06,
        tau_squared: 2.0,
        min_sample_size: 50,
    };
    // 4 variants → 3 comparisons → alpha/3.
    let c = split_alpha(&base, 4);
    assert!((c.alpha - 0.02).abs() < 1e-12);
    // Carried through unchanged.
    assert_eq!(c.tau_squared, 2.0);
    assert_eq!(c.min_sample_size, 50);
}

#[test]
fn split_alpha_two_variants_unchanged() {
    let base = SequentialConfig::default();
    let c = split_alpha(&base, 2);
    assert!((c.alpha - base.alpha).abs() < 1e-12);
}

#[test]
fn split_alpha_clamps_below_two() {
    let base = SequentialConfig::default();
    // 1 (or 0) variants treated as 2 → 1 comparison → unchanged.
    let c1 = split_alpha(&base, 1);
    let c0 = split_alpha(&base, 0);
    assert!((c1.alpha - base.alpha).abs() < 1e-12);
    assert!((c0.alpha - base.alpha).abs() < 1e-12);
}

#[test]
fn split_alpha_controls_family_wise_h0_rejection() {
    // With K=4 variants and per-comparison alpha = 0.05/3, the family-wise
    // rejection rate (ANY of the 3 comparisons fires) should stay near 0.05.
    let base = SequentialConfig {
        alpha: 0.05,
        tau_squared: 1.0,
        min_sample_size: 1,
    };
    let cfg = split_alpha(&base, 4);
    let sizes = look_sizes(15, 40, 40);
    let sims = 1500;
    let mut rng = Lcg::new(0xFA1117);

    let mut family_rejections = 0;
    for _ in 0..sims {
        // Three independent treatment-vs-control comparisons, all null.
        let any = (0..3).any(|_| simulate_one(&mut rng, 0.0, &sizes, &cfg).seq_rejected);
        if any {
            family_rejections += 1;
        }
    }
    let rate = family_rejections as f64 / sims as f64;
    assert!(
        rate <= base.alpha + 0.02,
        "family-wise H0 rejection {rate} should stay ≤ {} after split_alpha",
        base.alpha + 0.02
    );
}

// ── Unused-import guard (Percentiles is part of VariantStats surface) ─────────

#[test]
fn percentiles_type_is_constructible() {
    // Keeps the `Percentiles` import meaningful and documents that ratio/seq
    // do not use it (percentile metrics go through bootstrap, not mSPRT).
    let p = Percentiles {
        p50: 1.0,
        p95: 2.0,
        p99: 3.0,
    };
    assert_eq!(p.p50, 1.0);
}
