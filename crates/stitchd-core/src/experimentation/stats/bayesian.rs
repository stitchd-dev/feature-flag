//! Bayesian statistical analysis engine.
//!
//! Provides Bayesian inference for all four metric types:
//! - **Count** — Beta-Binomial conjugate posterior
//! - **Numeric** — Normal-Normal conjugate (analytical difference distribution)
//! - **Percentile** — Bootstrap posterior approximation
//! - **Funnel** — Beta-Binomial on final-step conversion rate

use serde::{Deserialize, Serialize};

use super::sequential::RatioGroupStats;
use super::{BayesianResult, ConfidenceInterval, VariantStats, Z95, norm_cdf};

// ── Per-variant Bayesian result (spec §3) ─────────────────────────────────────

/// Spec-aligned per-variant Bayesian result, as surfaced in the experiment
/// results JSON. One row per variant in the experiment — including the
/// control, for which `probability_to_beat_control = 0.5` and
/// `expected_lift = 0.0` (it's compared against itself).
///
/// Distinct from the legacy [`BayesianResult`] (in `stats::mod`) which is a
/// single aggregate per-metric struct; this is one entry per variant, matching
/// the shape the UI consumes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BayesianVariantResult {
    /// Variant identifier.
    pub variant_key: String,
    /// Mean of the posterior over this variant's metric value (rate for
    /// Beta-Binomial, mean for Normal-Normal).
    pub posterior_mean: f64,
    /// 95 % credible-interval lower bound on the posterior.
    pub posterior_ci_lower: f64,
    /// 95 % credible-interval upper bound on the posterior.
    pub posterior_ci_upper: f64,
    /// Probability that this variant beats the control on its metric.
    /// For the control row itself, by convention 0.5.
    pub probability_to_beat_control: f64,
    /// Expected `treatment − control` lift. For the control row, 0.0.
    pub expected_lift: f64,
}

// ── Simple LCG RNG ───────────────────────────────────────────────────────────

/// A minimal Linear Congruential Generator for Monte Carlo sampling.
///
/// Parameters from Numerical Recipes (Knuth).
///
/// `pub(crate)` so the [`crate::experimentation::bandit`] allocators can sample
/// from the SAME seeded RNG + Beta/Gamma machinery the Bayesian engine uses (a
/// single source of truth for posterior sampling; no second RNG dependency).
pub(crate) struct Lcg {
    state: u64,
}

impl Lcg {
    /// Construct an LCG seeded with `seed`.
    pub(crate) fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Returns the next pseudo-random u64.
    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.state
    }

    /// Returns a pseudo-random f64 in [0, 1).
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Trait used as the `Rng` bound in sampling helpers so tests can substitute a seeded RNG.
///
/// `pub(crate)` so the bandit allocators can call [`sample_beta`] / [`sample_gamma`]
/// against the shared [`Lcg`].
pub(crate) trait Rng {
    /// Returns a pseudo-random f64 in `[0, 1)`.
    fn next_f64(&mut self) -> f64;
}

impl Rng for Lcg {
    fn next_f64(&mut self) -> f64 {
        self.next_f64()
    }
}

// ── Gamma / Beta sampling ────────────────────────────────────────────────────

/// Sample from Gamma(shape, rate=1) using the Marsaglia-Tsang method.
///
/// Handles shape < 1 via the Ahrens–Dieter boost: Gamma(a) = Gamma(a+1) * U^(1/a).
pub(crate) fn sample_gamma(shape: f64, rng: &mut impl Rng) -> f64 {
    if shape < 1.0 {
        // Boost: sample Gamma(shape+1) then scale
        let u = rng.next_f64();
        return sample_gamma(shape + 1.0, rng) * u.powf(1.0 / shape);
    }

    // Marsaglia-Tsang: d = shape - 1/3, c = 1 / sqrt(9d)
    let d = shape - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();

    loop {
        // Draw a standard normal via Box-Muller
        let u1 = rng.next_f64().max(1e-300); // avoid log(0)
        let u2 = rng.next_f64();
        let z = (2.0 * std::f64::consts::PI * u2).cos() * (-2.0 * u1.ln()).sqrt();

        let v = 1.0 + c * z;
        if v <= 0.0 {
            continue;
        }
        let v3 = v * v * v;
        let u = rng.next_f64().max(1e-300);

        // Accept/reject
        if u < 1.0 - 0.0331 * (z * z) * (z * z) {
            return d * v3;
        }
        if u.ln() < 0.5 * z * z + d * (1.0 - v3 + v3.ln()) {
            return d * v3;
        }
    }
}

/// Sample from Beta(alpha, beta) using the ratio-of-Gammas method.
pub(crate) fn sample_beta(alpha: f64, beta: f64, rng: &mut impl Rng) -> f64 {
    let x = sample_gamma(alpha, rng);
    let y = sample_gamma(beta, rng);
    let sum = x + y;
    if sum == 0.0 { 0.5 } else { x / sum }
}

/// Draw a single standard-normal `N(0, 1)` deviate via the Box-Muller transform.
///
/// `pub(crate)` so the bandit allocators can sample Normal-Normal / delta-method
/// reward posteriors from the SAME [`Lcg`] used everywhere else in the stats
/// core. Uses the same cosine branch already used inside [`sample_gamma`].
pub(crate) fn sample_standard_normal(rng: &mut impl Rng) -> f64 {
    let u1 = rng.next_f64().max(1e-300); // avoid log(0)
    let u2 = rng.next_f64();
    (2.0 * std::f64::consts::PI * u2).cos() * (-2.0 * u1.ln()).sqrt()
}

// ── Percentile helper ────────────────────────────────────────────────────────

/// Compute the `p`-th percentile (0..=100) of a sorted slice via linear interpolation.
fn percentile_sorted(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let n = sorted.len() as f64;
    let index = p / 100.0 * (n - 1.0);
    let lo = index.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    let frac = index - lo as f64;
    sorted[lo] + frac * (sorted[hi] - sorted[lo])
}

// ── Public analysis functions ────────────────────────────────────────────────

/// Bayesian analysis of a **count/conversion** metric using Beta-Binomial posteriors.
///
/// Prior: Beta(1, 1) (uniform).
/// Posterior: Beta(1 + conversions, 1 + sample_size - conversions).
/// Uses 10,000 Monte Carlo samples to estimate `prob_best`, the credible interval,
/// and the expected loss.
pub fn analyze_count(control: &VariantStats, variant: &VariantStats) -> BayesianResult {
    let n_c = control.sample_size as f64;
    let conv_c = control.conversions.unwrap_or(0) as f64;
    let n_v = variant.sample_size as f64;
    let conv_v = variant.conversions.unwrap_or(0) as f64;

    // Posterior parameters (Beta with uniform prior)
    let alpha_c = 1.0 + conv_c;
    let beta_c = 1.0 + (n_c - conv_c).max(0.0);
    let alpha_v = 1.0 + conv_v;
    let beta_v = 1.0 + (n_v - conv_v).max(0.0);

    beta_binomial_mc(alpha_c, beta_c, alpha_v, beta_v, 10_000, 42)
}

/// A `Normal(diff, se²)` posterior on a lift, summarised as a [`BayesianResult`].
///
/// Shared by [`analyze_numeric`] and [`analyze_ratio`] (which differ only in how
/// they derive `diff` / `se`): the treatment-minus-control effect is modelled as
/// `N(diff, se²)`, giving
/// - `prob_best = P(effect > 0) = 1 − Φ(−diff / se)`,
/// - a 95 % credible interval `diff ± Z95·se`,
/// - and a closed-form expected loss `E[max(−effect, 0)] = se·φ(d) − diff·(1 −
///   Φ(d))` with `d = diff / se`, clamped to `≥ 0`.
///
/// The `se == 0` (point-mass posterior) branch degrades gracefully: `prob_best`
/// is the sign indicator of `diff`, the CI collapses to `[diff, diff]`, and the
/// expected loss is `max(−diff, 0)`.
fn bayes_normal_contrast(diff: f64, se: f64) -> BayesianResult {
    // P(effect > 0) = 1 - Φ(-diff / se)
    let prob_best = if se == 0.0 {
        if diff > 0.0 {
            1.0
        } else if diff < 0.0 {
            0.0
        } else {
            0.5
        }
    } else {
        1.0 - norm_cdf(-diff / se)
    };

    // 95% credible interval for the effect.
    let lower = diff - Z95 * se;
    let upper = diff + Z95 * se;

    // Expected loss = E[max(-effect, 0)] for effect ~ N(diff, se²).
    // = se·φ(d) - diff·(1 - Φ(d)) with d = diff / se (φ = standard normal PDF).
    let expected_loss = if se == 0.0 {
        (-diff).max(0.0)
    } else {
        let d = diff / se;
        let phi_d = (-0.5 * d * d).exp() / (2.0 * std::f64::consts::PI).sqrt();
        se * phi_d + (-diff) * (1.0 - norm_cdf(d))
    };

    BayesianResult {
        prob_best,
        credible_interval: ConfidenceInterval { lower, upper },
        expected_loss: expected_loss.max(0.0),
    }
}

/// Bayesian analysis of a **numeric** metric using a Normal-Normal conjugate approximation.
///
/// Uses empirical mean and variance. `prob_best` is derived from the CDF of the
/// difference distribution N(mean_v - mean_c, sqrt(var_v/n_v + var_c/n_c)).
pub fn analyze_numeric(control: &VariantStats, variant: &VariantStats) -> BayesianResult {
    let n_c = control.sample_size as f64;
    let n_v = variant.sample_size as f64;
    let mean_c = control.mean.unwrap_or(0.0);
    let mean_v = variant.mean.unwrap_or(0.0);
    let var_c = control.variance.unwrap_or(0.0).max(0.0);
    let var_v = variant.variance.unwrap_or(0.0).max(0.0);

    // Standard error of the difference
    let se2 = var_v / n_v.max(1.0) + var_c / n_c.max(1.0);
    let se = se2.sqrt();

    let mean_diff = mean_v - mean_c;

    bayes_normal_contrast(mean_diff, se)
}

/// Bayesian analysis of a **percentile** metric via bootstrap posterior approximation.
///
/// Draws 1,000 bootstrap samples from each group, computes the `percentile` on each,
/// and estimates `prob_best`, the credible interval, and the expected loss from the
/// empirical distribution of differences.
pub fn analyze_percentile(
    control_samples: &[f64],
    variant_samples: &[f64],
    percentile: f64,
) -> BayesianResult {
    const N_BOOT: usize = 1_000;
    let seed = 42u64;
    let mut rng = Lcg::new(seed);

    if control_samples.is_empty() || variant_samples.is_empty() {
        return BayesianResult {
            prob_best: 0.5,
            credible_interval: ConfidenceInterval {
                lower: 0.0,
                upper: 0.0,
            },
            expected_loss: 0.0,
        };
    }

    let n_c = control_samples.len();
    let n_v = variant_samples.len();

    // Pre-sort for percentile computation
    let mut ctrl_sorted = control_samples.to_vec();
    let mut var_sorted = variant_samples.to_vec();
    ctrl_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    var_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let mut differences = Vec::with_capacity(N_BOOT);

    for _ in 0..N_BOOT {
        // Bootstrap resample control
        let mut boot_c = Vec::with_capacity(n_c);
        for _ in 0..n_c {
            let idx = (rng.next_f64() * n_c as f64) as usize % n_c;
            boot_c.push(control_samples[idx]);
        }
        boot_c.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        // Bootstrap resample variant
        let mut boot_v = Vec::with_capacity(n_v);
        for _ in 0..n_v {
            let idx = (rng.next_f64() * n_v as f64) as usize % n_v;
            boot_v.push(variant_samples[idx]);
        }
        boot_v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let perc_c = percentile_sorted(&boot_c, percentile);
        let perc_v = percentile_sorted(&boot_v, percentile);
        differences.push(perc_v - perc_c);
    }

    // prob_best = fraction where variant > control
    let wins = differences.iter().filter(|&&d| d > 0.0).count();
    let prob_best = wins as f64 / N_BOOT as f64;

    // Credible interval: 2.5th and 97.5th percentile of differences
    let mut diff_sorted = differences.clone();
    diff_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lower = percentile_sorted(&diff_sorted, 2.5);
    let upper = percentile_sorted(&diff_sorted, 97.5);

    // Expected loss = mean of max(-diff, 0)
    let expected_loss = differences.iter().map(|&d| (-d).max(0.0)).sum::<f64>() / N_BOOT as f64;

    BayesianResult {
        prob_best,
        credible_interval: ConfidenceInterval { lower, upper },
        expected_loss,
    }
}

/// Bayesian analysis of a **funnel** metric using Beta-Binomial posteriors.
///
/// Approximates conversions as `(conversion_rate * sample_size) as i64` when raw
/// conversion counts are not populated, then delegates to the same Beta-Binomial
/// Monte Carlo estimator used by [`analyze_count`].
pub fn analyze_funnel(control: &VariantStats, variant: &VariantStats) -> BayesianResult {
    let approx_conversions = |stats: &VariantStats| -> f64 {
        if let Some(c) = stats.conversions {
            return c as f64;
        }
        if let Some(rate) = stats.conversion_rate {
            return (rate * stats.sample_size as f64).round();
        }
        0.0
    };

    let n_c = control.sample_size as f64;
    let conv_c = approx_conversions(control);
    let n_v = variant.sample_size as f64;
    let conv_v = approx_conversions(variant);

    let alpha_c = 1.0 + conv_c;
    let beta_c = 1.0 + (n_c - conv_c).max(0.0);
    let alpha_v = 1.0 + conv_v;
    let beta_v = 1.0 + (n_v - conv_v).max(0.0);

    beta_binomial_mc(alpha_c, beta_c, alpha_v, beta_v, 10_000, 42)
}

/// Bayesian analysis of a **ratio** metric — a normal posterior on the
/// delta-method estimate / SE.
///
/// Ratio sufficient statistics are not present on [`VariantStats`], so — like
/// [`super::frequentist::analyze_ratio`] and
/// [`super::sequential::sequential_ratio`] — this takes explicit per-group
/// [`RatioGroupStats`] and reuses [`RatioGroupStats::ratio_var`] (the single
/// source of truth for the delta-method point + variance). The effect is
/// `diff = R_t − R_c` with `SE = sqrt(Var(R_c) + Var(R_t))`. We then place a
/// `N(diff, SE²)` posterior on the lift: `prob_best = P(R_t > R_c)`, the 95 %
/// credible interval is `diff ± Z95·SE`, and the expected loss
/// `E[max(R_c − R_t, 0)]` uses the closed-form normal tail expectation. The
/// `N(diff, SE²)` summary is computed by the shared [`bayes_normal_contrast`]
/// (the same helper [`analyze_numeric`] uses), so the two paths cannot drift.
///
/// Returns the neutral `prob_best = 0.5`, zero-width-CI result when either
/// group is degenerate (see [`RatioGroupStats::ratio_var`]).
pub fn analyze_ratio(control: &RatioGroupStats, variant: &RatioGroupStats) -> BayesianResult {
    let (Some((r_c, var_c)), Some((r_t, var_t))) = (control.ratio_var(), variant.ratio_var())
    else {
        return BayesianResult {
            prob_best: 0.5,
            credible_interval: ConfidenceInterval {
                lower: 0.0,
                upper: 0.0,
            },
            expected_loss: 0.0,
        };
    };
    let diff = r_t - r_c;
    let se = (var_c + var_t).sqrt();
    bayes_normal_contrast(diff, se)
}

// ── Shared Monte Carlo helper ─────────────────────────────────────────────────

/// Estimate `prob_best`, credible interval, and expected loss for two Beta posteriors
/// via Monte Carlo with `n_samples` draws and a fixed RNG `seed`.
fn beta_binomial_mc(
    alpha_c: f64,
    beta_c: f64,
    alpha_v: f64,
    beta_v: f64,
    n_samples: usize,
    seed: u64,
) -> BayesianResult {
    let mut rng = Lcg::new(seed);
    let mut differences = Vec::with_capacity(n_samples);

    for _ in 0..n_samples {
        let s_c = sample_beta(alpha_c, beta_c, &mut rng);
        let s_v = sample_beta(alpha_v, beta_v, &mut rng);
        differences.push(s_v - s_c);
    }

    // prob_best = P(variant > control)
    let wins = differences.iter().filter(|&&d| d > 0.0).count();
    let prob_best = wins as f64 / n_samples as f64;

    // Credible interval of differences
    let mut diff_sorted = differences.clone();
    diff_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let lower = percentile_sorted(&diff_sorted, 2.5);
    let upper = percentile_sorted(&diff_sorted, 97.5);

    // Expected loss = E[max(control_rate - variant_rate, 0)] = mean of max(-diff, 0)
    let expected_loss = differences.iter().map(|&d| (-d).max(0.0)).sum::<f64>() / n_samples as f64;

    BayesianResult {
        prob_best,
        credible_interval: ConfidenceInterval { lower, upper },
        expected_loss,
    }
}

// ── Per-variant input shapes ─────────────────────────────────────────────────

/// One Beta-Binomial datapoint for [`beta_binomial`].
#[derive(Debug, Clone)]
pub struct BetaBinomialInput {
    /// Variant identifier.
    pub variant_key: String,
    /// Number of successes / conversions.
    pub successes: u64,
    /// Total number of trials / exposed contexts.
    pub trials: u64,
}

/// One Normal-Normal datapoint for [`normal_normal`].
#[derive(Debug, Clone)]
pub struct NormalNormalInput {
    /// Variant identifier.
    pub variant_key: String,
    /// Sample mean of the metric.
    pub mean: f64,
    /// Sample variance of the metric (not standard deviation).
    pub variance: f64,
    /// Sample size.
    pub n: u64,
}

// ── Spec-aligned public API ──────────────────────────────────────────────────

/// Number of Monte Carlo samples used when estimating
/// `probability_to_beat_control` and `expected_lift` against a control
/// posterior. 10,000 samples are sufficient for the per-variant outputs we
/// surface (~1 % stderr).
const MC_SAMPLES: usize = 10_000;

/// RNG seed used for reproducibility across calls. Tests pin against this
/// seed; production callers see deterministic outputs for a fixed input.
const MC_SEED: u64 = 0x00C0_FFEE_5EED;

/// Bayesian inference for a proportion metric (conversion, retention, …)
/// using a Beta-Binomial conjugate posterior.
///
/// Prior: Beta(1, 1) (uniform). Posterior: Beta(1 + successes, 1 + trials -
/// successes). The first entry in `inputs` is treated as the control; one
/// row is returned per input in the order supplied.
///
/// Probability-to-beat-control and expected lift are estimated via
/// `MC_SAMPLES` Monte Carlo draws against the control posterior. The control
/// row gets `probability_to_beat_control = 0.5` and `expected_lift = 0.0` by
/// convention.
///
/// Returns an empty `Vec` if `inputs` is empty.
pub fn beta_binomial(inputs: &[BetaBinomialInput]) -> Vec<BayesianVariantResult> {
    if inputs.is_empty() {
        return vec![];
    }
    let mut rng = Lcg::new(MC_SEED);

    // Pre-sample control posterior to compute per-variant probabilities + lifts.
    let control = &inputs[0];
    let (alpha_c, beta_c) = beta_posterior(control.successes, control.trials);
    let control_samples: Vec<f64> = (0..MC_SAMPLES)
        .map(|_| sample_beta(alpha_c, beta_c, &mut rng))
        .collect();

    inputs
        .iter()
        .enumerate()
        .map(|(idx, input)| {
            let (alpha, beta) = beta_posterior(input.successes, input.trials);

            // Posterior mean = α / (α + β).
            let posterior_mean = alpha / (alpha + beta);

            // 95 % credible interval via MC quantiles on the posterior itself.
            let mut samples: Vec<f64> = (0..MC_SAMPLES)
                .map(|_| sample_beta(alpha, beta, &mut rng))
                .collect();
            samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let lower = percentile_sorted(&samples, 2.5);
            let upper = percentile_sorted(&samples, 97.5);

            let (probability_to_beat_control, expected_lift) = if idx == 0 {
                (0.5, 0.0)
            } else {
                // Reuse control's pre-drawn samples; pair each control draw with
                // a fresh variant draw to estimate P(variant > control) and
                // E[variant - control].
                let mut wins = 0_usize;
                let mut sum_lift = 0.0_f64;
                for &c in &control_samples {
                    let v = sample_beta(alpha, beta, &mut rng);
                    if v > c {
                        wins += 1;
                    }
                    sum_lift += v - c;
                }
                let ptb = wins as f64 / control_samples.len() as f64;
                let lift = sum_lift / control_samples.len() as f64;
                (ptb, lift)
            };

            BayesianVariantResult {
                variant_key: input.variant_key.clone(),
                posterior_mean,
                posterior_ci_lower: lower,
                posterior_ci_upper: upper,
                probability_to_beat_control,
                expected_lift,
            }
        })
        .collect()
}

/// Bayesian inference for a continuous metric using a Normal-Normal
/// approximation.
///
/// Uses a weak (effectively flat) prior so the posterior is dominated by the
/// data: `post_mean = sample_mean`, `post_var = sample_var / n`. The
/// difference distribution `treatment − control` is then
/// `N(mean_t − mean_c, post_var_t + post_var_c)`, from which we derive the
/// probability-to-beat and 95 % credible interval analytically.
///
/// The first entry in `inputs` is the control. The control row gets
/// `probability_to_beat_control = 0.5` and `expected_lift = 0.0`.
///
/// Returns an empty `Vec` if `inputs` is empty.
pub fn normal_normal(inputs: &[NormalNormalInput]) -> Vec<BayesianVariantResult> {
    if inputs.is_empty() {
        return vec![];
    }
    // 95 % z-multiplier from the shared `super::Z95` (imported at module top).
    let control = &inputs[0];
    let control_post_var = posterior_variance(control.variance, control.n);

    inputs
        .iter()
        .enumerate()
        .map(|(idx, input)| {
            let posterior_mean = input.mean;
            let post_var = posterior_variance(input.variance, input.n);
            let se = post_var.sqrt();
            let lower = posterior_mean - Z95 * se;
            let upper = posterior_mean + Z95 * se;

            let (probability_to_beat_control, expected_lift) = if idx == 0 {
                (0.5, 0.0)
            } else {
                let diff = input.mean - control.mean;
                let diff_var = post_var + control_post_var;
                let diff_se = diff_var.sqrt();
                let ptb = if diff_se == 0.0 {
                    if diff > 0.0 {
                        1.0
                    } else if diff < 0.0 {
                        0.0
                    } else {
                        0.5
                    }
                } else {
                    1.0 - norm_cdf(-diff / diff_se)
                };
                (ptb, diff)
            };

            BayesianVariantResult {
                variant_key: input.variant_key.clone(),
                posterior_mean,
                posterior_ci_lower: lower,
                posterior_ci_upper: upper,
                probability_to_beat_control,
                expected_lift,
            }
        })
        .collect()
}

/// Posterior parameters for Beta(1+successes, 1+trials-successes) — clamped
/// so a zero-trials input still produces a defined Beta(1, 1).
fn beta_posterior(successes: u64, trials: u64) -> (f64, f64) {
    let s = successes as f64;
    let n = trials as f64;
    let alpha = 1.0 + s;
    let beta = 1.0 + (n - s).max(0.0);
    (alpha, beta)
}

/// Posterior variance for the Normal-Normal weak-prior approximation:
/// `var / n`. `n = 0` is treated as `n = 1` to avoid division-by-zero (the
/// posterior is then equal to the sample variance).
fn posterior_variance(variance: f64, n: u64) -> f64 {
    let n = (n as f64).max(1.0);
    variance.max(0.0) / n
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_count_stats(sample_size: i64, conversions: i64) -> VariantStats {
        VariantStats {
            sample_size,
            conversions: Some(conversions),
            mean: None,
            variance: None,
            conversion_rate: Some(conversions as f64 / sample_size as f64),
            percentiles: None,
        }
    }

    fn make_numeric_stats(sample_size: i64, mean: f64, variance: f64) -> VariantStats {
        VariantStats {
            sample_size,
            conversions: None,
            mean: Some(mean),
            variance: Some(variance),
            conversion_rate: None,
            percentiles: None,
        }
    }

    // ── analyze_count ─────────────────────────────────────────────────────────

    /// Clear winner: 150/1000 vs 100/1000 — variant is clearly better, prob_best > 0.99
    #[test]
    fn count_clear_winner_prob_best_exceeds_0_99() {
        let control = make_count_stats(1000, 100);
        let variant = make_count_stats(1000, 150);
        let result = analyze_count(&control, &variant);
        assert!(
            result.prob_best > 0.99,
            "expected prob_best > 0.99, got {}",
            result.prob_best
        );
    }

    /// Near-equal: 11 vs 10 conversions out of 100 — prob_best should be near 0.5
    #[test]
    fn count_near_equal_prob_best_near_0_5() {
        let control = make_count_stats(100, 10);
        let variant = make_count_stats(100, 11);
        let result = analyze_count(&control, &variant);
        assert!(
            result.prob_best > 0.3 && result.prob_best < 0.7,
            "expected prob_best near 0.5, got {}",
            result.prob_best
        );
    }

    #[test]
    fn count_credible_interval_is_ordered() {
        let control = make_count_stats(1000, 100);
        let variant = make_count_stats(1000, 150);
        let result = analyze_count(&control, &variant);
        assert!(
            result.credible_interval.lower < result.credible_interval.upper,
            "lower={} should be < upper={}",
            result.credible_interval.lower,
            result.credible_interval.upper
        );
    }

    #[test]
    fn count_clear_winner_credible_interval_excludes_zero() {
        let control = make_count_stats(1000, 100);
        let variant = make_count_stats(1000, 150);
        let result = analyze_count(&control, &variant);
        // The 95% CI for a clear positive effect should have lower > 0
        assert!(
            result.credible_interval.lower > 0.0,
            "expected lower > 0 for clear winner, got {}",
            result.credible_interval.lower
        );
    }

    #[test]
    fn count_expected_loss_near_zero_for_clear_winner() {
        let control = make_count_stats(1000, 100);
        let variant = make_count_stats(1000, 150);
        let result = analyze_count(&control, &variant);
        assert!(
            result.expected_loss < 0.01,
            "expected expected_loss < 0.01 for clear winner, got {}",
            result.expected_loss
        );
    }

    #[test]
    fn count_prob_best_sums_to_one_approximately() {
        // When control and variant use same params, prob_best should be near 0.5
        let control = make_count_stats(1000, 100);
        let variant = make_count_stats(1000, 100);
        let result = analyze_count(&control, &variant);
        assert!(
            (result.prob_best - 0.5).abs() < 0.05,
            "expected prob_best ≈ 0.5 for identical groups, got {}",
            result.prob_best
        );
    }

    // ── analyze_numeric ───────────────────────────────────────────────────────

    /// Large, clear numeric difference: variant is far superior
    #[test]
    fn numeric_clear_winner_prob_best_exceeds_0_99() {
        // control: mean=100, var=100, n=1000
        // variant: mean=110, var=100, n=1000
        // SE = sqrt(100/1000 + 100/1000) = sqrt(0.2) ≈ 0.447
        // diff = 10 >> SE → prob_best ≈ 1.0
        let control = make_numeric_stats(1000, 100.0, 100.0);
        let variant = make_numeric_stats(1000, 110.0, 100.0);
        let result = analyze_numeric(&control, &variant);
        assert!(
            result.prob_best > 0.99,
            "expected prob_best > 0.99, got {}",
            result.prob_best
        );
    }

    #[test]
    fn numeric_near_equal_prob_best_near_0_5() {
        // diff = 0.1, SE = sqrt(25/1000 + 25/1000) = sqrt(0.05) ≈ 0.224
        // d = 0.1 / 0.224 ≈ 0.45 → prob_best ≈ 0.67; still well within 0.3–0.7
        let control = make_numeric_stats(1000, 50.0, 25.0);
        let variant = make_numeric_stats(1000, 50.1, 25.0);
        let result = analyze_numeric(&control, &variant);
        assert!(
            result.prob_best > 0.3 && result.prob_best < 0.7,
            "expected prob_best near 0.5, got {}",
            result.prob_best
        );
    }

    #[test]
    fn numeric_credible_interval_is_ordered() {
        let control = make_numeric_stats(1000, 100.0, 100.0);
        let variant = make_numeric_stats(1000, 110.0, 100.0);
        let result = analyze_numeric(&control, &variant);
        assert!(
            result.credible_interval.lower < result.credible_interval.upper,
            "lower={} should be < upper={}",
            result.credible_interval.lower,
            result.credible_interval.upper
        );
    }

    #[test]
    fn numeric_credible_interval_centered_on_diff() {
        let control = make_numeric_stats(1000, 100.0, 100.0);
        let variant = make_numeric_stats(1000, 110.0, 100.0);
        let result = analyze_numeric(&control, &variant);
        let mid = (result.credible_interval.lower + result.credible_interval.upper) / 2.0;
        assert!(
            (mid - 10.0).abs() < 0.1,
            "expected CI midpoint ≈ 10.0, got {}",
            mid
        );
    }

    #[test]
    fn numeric_expected_loss_is_nonnegative() {
        let control = make_numeric_stats(500, 50.0, 20.0);
        let variant = make_numeric_stats(500, 55.0, 20.0);
        let result = analyze_numeric(&control, &variant);
        assert!(
            result.expected_loss >= 0.0,
            "expected_loss should be non-negative, got {}",
            result.expected_loss
        );
    }

    #[test]
    fn numeric_zero_variance_deterministic() {
        let control = make_numeric_stats(100, 5.0, 0.0);
        let variant = make_numeric_stats(100, 10.0, 0.0);
        let result = analyze_numeric(&control, &variant);
        // With zero variance and positive diff, variant should always win
        assert_eq!(result.prob_best, 1.0);
        assert_eq!(result.expected_loss, 0.0);
    }

    // ── analyze_percentile ────────────────────────────────────────────────────

    fn make_uniform_samples(n: usize, lo: f64, hi: f64, seed: u64) -> Vec<f64> {
        let mut rng = Lcg::new(seed);
        (0..n).map(|_| lo + rng.next_f64() * (hi - lo)).collect()
    }

    #[test]
    fn percentile_clear_winner_prob_best_high() {
        // variant samples are clearly larger — variant p50 should beat control p50
        let control = make_uniform_samples(500, 0.0, 10.0, 1);
        let variant = make_uniform_samples(500, 15.0, 25.0, 2);
        let result = analyze_percentile(&control, &variant, 50.0);
        assert!(
            result.prob_best > 0.95,
            "expected prob_best > 0.95, got {}",
            result.prob_best
        );
    }

    #[test]
    fn percentile_identical_samples_prob_best_near_0_5() {
        // Using exact same distribution, prob_best should be near 0.5
        let samples: Vec<f64> = (0..200).map(|i| i as f64).collect();
        let result = analyze_percentile(&samples, &samples, 50.0);
        assert!(
            result.prob_best >= 0.3 && result.prob_best <= 0.7,
            "expected prob_best near 0.5, got {}",
            result.prob_best
        );
    }

    #[test]
    fn percentile_empty_samples_returns_default() {
        let result = analyze_percentile(&[], &[], 50.0);
        assert_eq!(result.prob_best, 0.5);
    }

    #[test]
    fn percentile_credible_interval_ordered() {
        let control = make_uniform_samples(300, 0.0, 10.0, 3);
        let variant = make_uniform_samples(300, 5.0, 15.0, 4);
        let result = analyze_percentile(&control, &variant, 50.0);
        assert!(
            result.credible_interval.lower < result.credible_interval.upper,
            "lower={} should be < upper={}",
            result.credible_interval.lower,
            result.credible_interval.upper
        );
    }

    // ── analyze_funnel ────────────────────────────────────────────────────────

    fn make_funnel_stats_from_rate(sample_size: i64, conversion_rate: f64) -> VariantStats {
        VariantStats {
            sample_size,
            conversions: None, // force the rate-based approximation path
            mean: None,
            variance: None,
            conversion_rate: Some(conversion_rate),
            percentiles: None,
        }
    }

    #[test]
    fn funnel_clear_winner_prob_best_exceeds_0_99() {
        let control = make_count_stats(1000, 100);
        let variant = make_count_stats(1000, 150);
        let result = analyze_funnel(&control, &variant);
        assert!(
            result.prob_best > 0.99,
            "expected prob_best > 0.99, got {}",
            result.prob_best
        );
    }

    #[test]
    fn funnel_rate_approximation_matches_count_closely() {
        // Using conversion_rate path should give similar results to direct count path
        let control_count = make_count_stats(1000, 100);
        let variant_count = make_count_stats(1000, 150);
        let result_count = analyze_funnel(&control_count, &variant_count);

        let control_rate = make_funnel_stats_from_rate(1000, 0.10);
        let variant_rate = make_funnel_stats_from_rate(1000, 0.15);
        let result_rate = analyze_funnel(&control_rate, &variant_rate);

        assert!(
            (result_count.prob_best - result_rate.prob_best).abs() < 0.01,
            "rate and count paths should agree closely: count={}, rate={}",
            result_count.prob_best,
            result_rate.prob_best
        );
    }

    #[test]
    fn funnel_expected_loss_is_nonnegative() {
        let control = make_count_stats(500, 50);
        let variant = make_count_stats(500, 60);
        let result = analyze_funnel(&control, &variant);
        assert!(
            result.expected_loss >= 0.0,
            "expected_loss must be >= 0, got {}",
            result.expected_loss
        );
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    #[test]
    fn normal_cdf_at_zero_is_half() {
        // Bayesian now uses the shared `super::norm_cdf`.
        assert!((norm_cdf(0.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn normal_cdf_at_plus_infinity_ish() {
        assert!((norm_cdf(10.0) - 1.0) < 1e-6);
    }

    #[test]
    fn normal_cdf_symmetry() {
        for x in [0.5, 1.0, 1.645, 1.96, 2.576] {
            let sum = norm_cdf(x) + norm_cdf(-x);
            assert!((sum - 1.0).abs() < 1e-6, "symmetry failed at x={x}: {sum}");
        }
    }

    #[test]
    fn sample_beta_mean_approx() {
        let mut rng = Lcg::new(99);
        let (alpha, beta) = (3.0, 7.0);
        let expected_mean = alpha / (alpha + beta); // 0.3
        let n = 50_000usize;
        let mean: f64 = (0..n)
            .map(|_| sample_beta(alpha, beta, &mut rng))
            .sum::<f64>()
            / n as f64;
        assert!(
            (mean - expected_mean).abs() < 0.01,
            "Beta mean ≈ {expected_mean}, got {mean}"
        );
    }

    #[test]
    fn percentile_sorted_basic() {
        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        assert!((percentile_sorted(&data, 50.0) - 3.0).abs() < 1e-9);
        assert!((percentile_sorted(&data, 0.0) - 1.0).abs() < 1e-9);
        assert!((percentile_sorted(&data, 100.0) - 5.0).abs() < 1e-9);
    }

    // ── percentile_sorted edge cases ──────────────────────────────────────────

    #[test]
    fn percentile_sorted_empty_returns_zero() {
        // Covers the `if sorted.is_empty()` branch (line 125)
        assert_eq!(percentile_sorted(&[], 50.0), 0.0);
    }

    #[test]
    fn percentile_sorted_single_element_returns_that_element() {
        // Covers the `if sorted.len() == 1` branch (line 128)
        assert_eq!(percentile_sorted(&[42.0], 50.0), 42.0);
        assert_eq!(percentile_sorted(&[7.5], 0.0), 7.5);
        assert_eq!(percentile_sorted(&[7.5], 100.0), 7.5);
    }

    // ── sample_gamma with shape < 1.0 ────────────────────────────────────────

    #[test]
    fn sample_gamma_small_shape_returns_positive() {
        // Covers lines 81-82 (shape < 1 boost path)
        let mut rng = Lcg::new(1234);
        let n = 10_000usize;
        let mut all_positive = true;
        let mut sum = 0.0f64;
        for _ in 0..n {
            let v = sample_gamma(0.5, &mut rng);
            if v <= 0.0 {
                all_positive = false;
            }
            sum += v;
        }
        assert!(
            all_positive,
            "all samples from Gamma(0.5) should be positive"
        );
        // Gamma(0.5) has mean = shape = 0.5
        let mean = sum / n as f64;
        assert!(
            (mean - 0.5).abs() < 0.05,
            "Gamma(0.5) mean ≈ 0.5, got {mean}"
        );
    }

    // ── beta_binomial spec-aligned API ────────────────────────────────────────

    #[test]
    fn beta_binomial_empty_returns_empty() {
        let r = beta_binomial(&[]);
        assert!(r.is_empty());
    }

    /// 100/1000 vs 110/1000 — treatment likely wins but not certain.
    /// PtB should land in [0.7, 0.9].
    #[test]
    fn beta_binomial_modest_lift_ptb_in_range() {
        let inputs = vec![
            BetaBinomialInput {
                variant_key: "control".into(),
                successes: 100,
                trials: 1000,
            },
            BetaBinomialInput {
                variant_key: "treatment".into(),
                successes: 110,
                trials: 1000,
            },
        ];
        let r = beta_binomial(&inputs);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].variant_key, "control");
        assert_eq!(r[0].probability_to_beat_control, 0.5);
        assert_eq!(r[0].expected_lift, 0.0);
        assert!(
            r[1].probability_to_beat_control > 0.7 && r[1].probability_to_beat_control < 0.9,
            "PtB should be in (0.7, 0.9), got {}",
            r[1].probability_to_beat_control
        );
        // Posterior mean ≈ (1 + successes) / (2 + trials) = 111/1002 ≈ 0.1108
        assert!((r[1].posterior_mean - 0.1108).abs() < 0.001);
        // CI bounded and ordered.
        assert!(r[1].posterior_ci_lower < r[1].posterior_ci_upper);
        // Expected lift positive (~0.01).
        assert!(r[1].expected_lift > 0.0 && r[1].expected_lift < 0.05);
    }

    /// 100/1000 vs 200/1000 — huge effect, PtB virtually 1.
    #[test]
    fn beta_binomial_huge_lift_ptb_near_one() {
        let inputs = vec![
            BetaBinomialInput {
                variant_key: "control".into(),
                successes: 100,
                trials: 1000,
            },
            BetaBinomialInput {
                variant_key: "treatment".into(),
                successes: 200,
                trials: 1000,
            },
        ];
        let r = beta_binomial(&inputs);
        assert!(
            r[1].probability_to_beat_control > 0.999,
            "PtB should be > 0.999, got {}",
            r[1].probability_to_beat_control
        );
        assert!(r[1].expected_lift > 0.08 && r[1].expected_lift < 0.12);
    }

    /// Three-variant Beta-Binomial: control + 2 treatments. Both rows
    /// returned, both compared to control.
    #[test]
    fn beta_binomial_multi_variant() {
        let inputs = vec![
            BetaBinomialInput {
                variant_key: "control".into(),
                successes: 100,
                trials: 1000,
            },
            BetaBinomialInput {
                variant_key: "treat_a".into(),
                successes: 150,
                trials: 1000,
            },
            BetaBinomialInput {
                variant_key: "treat_b".into(),
                successes: 130,
                trials: 1000,
            },
        ];
        let r = beta_binomial(&inputs);
        assert_eq!(r.len(), 3);
        assert!(r[1].probability_to_beat_control > r[2].probability_to_beat_control);
        assert!(r[1].expected_lift > r[2].expected_lift);
    }

    // ── normal_normal spec-aligned API ────────────────────────────────────────

    #[test]
    fn normal_normal_empty_returns_empty() {
        assert!(normal_normal(&[]).is_empty());
    }

    /// Equal means → PtB ≈ 0.5.
    #[test]
    fn normal_normal_equal_means_ptb_near_half() {
        let inputs = vec![
            NormalNormalInput {
                variant_key: "control".into(),
                mean: 50.0,
                variance: 100.0,
                n: 1000,
            },
            NormalNormalInput {
                variant_key: "treatment".into(),
                mean: 50.0,
                variance: 100.0,
                n: 1000,
            },
        ];
        let r = normal_normal(&inputs);
        assert!(
            (r[1].probability_to_beat_control - 0.5).abs() < 1e-6,
            "expected PtB ≈ 0.5, got {}",
            r[1].probability_to_beat_control
        );
        assert_eq!(r[1].expected_lift, 0.0);
    }

    /// Treatment is +1σ above control where σ is the difference-distribution
    /// stderr — PtB should be ≈ 0.84 (Φ(1)).
    #[test]
    fn normal_normal_one_sigma_lift_ptb_near_84() {
        // post_var_c = 100 / 1000 = 0.1; post_var_t = 100 / 1000 = 0.1
        // diff_var = 0.2; diff_se = sqrt(0.2) ≈ 0.4472
        // Set mean_t - mean_c = diff_se → PtB = Φ(1) ≈ 0.8413.
        let diff_se = (0.2_f64).sqrt();
        let inputs = vec![
            NormalNormalInput {
                variant_key: "control".into(),
                mean: 50.0,
                variance: 100.0,
                n: 1000,
            },
            NormalNormalInput {
                variant_key: "treatment".into(),
                mean: 50.0 + diff_se,
                variance: 100.0,
                n: 1000,
            },
        ];
        let r = normal_normal(&inputs);
        assert!(
            (r[1].probability_to_beat_control - 0.8413).abs() < 0.005,
            "expected PtB ≈ 0.8413, got {}",
            r[1].probability_to_beat_control
        );
    }

    /// Control row has PtB = 0.5, expected_lift = 0; posterior_mean = sample_mean.
    #[test]
    fn normal_normal_control_row_has_fixed_zero_lift() {
        let inputs = vec![NormalNormalInput {
            variant_key: "control".into(),
            mean: 42.0,
            variance: 16.0,
            n: 100,
        }];
        let r = normal_normal(&inputs);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].probability_to_beat_control, 0.5);
        assert_eq!(r[0].expected_lift, 0.0);
        assert_eq!(r[0].posterior_mean, 42.0);
        // CI: post_se = sqrt(16 / 100) = 0.4 → ± 1.96 * 0.4 = ± 0.784
        assert!((r[0].posterior_ci_lower - 41.216).abs() < 0.01);
        assert!((r[0].posterior_ci_upper - 42.784).abs() < 0.01);
    }

    /// BayesianVariantResult round-trips through serde JSON with snake_case
    /// keys matching the spec.
    #[test]
    fn bayesian_variant_result_serializes_to_snake_case() {
        let r = BayesianVariantResult {
            variant_key: "treatment".into(),
            posterior_mean: 0.12,
            posterior_ci_lower: 0.08,
            posterior_ci_upper: 0.16,
            probability_to_beat_control: 0.82,
            expected_lift: 0.02,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("variant_key").is_some());
        assert!(v.get("posterior_mean").is_some());
        assert!(v.get("posterior_ci_lower").is_some());
        assert!(v.get("posterior_ci_upper").is_some());
        assert!(v.get("probability_to_beat_control").is_some());
        assert!(v.get("expected_lift").is_some());
    }

    // ── analyze_ratio (delta-method normal posterior) ────────────────────────

    /// Clear positive ratio lift (R_c = 0.5, R_t = 0.75, spread) → prob_best
    /// near 1, expected_loss near 0, CI midpoint = diff = 0.25. Mirrors the
    /// prior inline `compute::ratio_bayesian` behaviour.
    #[test]
    fn ratio_clear_winner_prob_best_high() {
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
        assert!(r.prob_best > 0.99, "prob_best={}", r.prob_best);
        assert!(r.expected_loss < 0.01, "expected_loss={}", r.expected_loss);
        let mid = (r.credible_interval.lower + r.credible_interval.upper) / 2.0;
        assert!(
            (mid - 0.25).abs() < 1e-9,
            "CI midpoint {mid} should be 0.25"
        );
    }

    /// Degenerate groups → neutral prob_best = 0.5, zero-width CI, zero loss.
    #[test]
    fn ratio_degenerate_is_neutral() {
        let degenerate = RatioGroupStats {
            n: 1,
            num_sum: 1.0,
            den_sum: 2.0,
            num_sq_sum: 1.0,
            den_sq_sum: 4.0,
            num_den_sum: 2.0,
        };
        let r = analyze_ratio(&degenerate, &degenerate);
        assert_eq!(r.prob_best, 0.5);
        assert_eq!(r.credible_interval.lower, 0.0);
        assert_eq!(r.credible_interval.upper, 0.0);
        assert_eq!(r.expected_loss, 0.0);
    }

    /// Identical non-degenerate groups → prob_best = 0.5, expected_loss > 0
    /// (a coin-flip lift has a positive expected downside), CI centred on 0.
    #[test]
    fn ratio_identical_groups_prob_best_half() {
        let g = RatioGroupStats {
            n: 100,
            num_sum: 50.0,
            den_sum: 100.0,
            num_sq_sum: 30.0,
            den_sq_sum: 120.0,
            num_den_sum: 55.0,
        };
        let r = analyze_ratio(&g, &g);
        assert!(
            (r.prob_best - 0.5).abs() < 1e-9,
            "prob_best={}",
            r.prob_best
        );
        let mid = (r.credible_interval.lower + r.credible_interval.upper) / 2.0;
        assert!((mid - 0.0).abs() < 1e-12, "CI midpoint {mid} should be 0.0");
        assert!(r.expected_loss > 0.0);
    }

    // ── analyze_funnel with no conversions and no conversion_rate ────────────

    #[test]
    fn funnel_no_conversions_no_rate_falls_back_to_zero() {
        // Covers the `0.0` fallback in `approx_conversions` (line 300)
        let empty_stats = |n: i64| VariantStats {
            sample_size: n,
            conversions: None,
            mean: None,
            variance: None,
            conversion_rate: None,
            percentiles: None,
        };
        let control = empty_stats(1000);
        let variant = empty_stats(1000);
        let result = analyze_funnel(&control, &variant);
        // Both groups effectively have 0 conversions → prob_best ≈ 0.5
        assert!(
            result.prob_best > 0.3 && result.prob_best < 0.7,
            "prob_best with zero conversions both sides should be near 0.5, got {}",
            result.prob_best
        );
        assert!(result.expected_loss >= 0.0);
    }
}
