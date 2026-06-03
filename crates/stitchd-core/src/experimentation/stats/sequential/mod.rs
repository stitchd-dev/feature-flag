//! Sequential (always-valid) statistical analysis engine.
//!
//! Classical fixed-horizon tests (see [`super::frequentist`]) are only valid
//! when the sample size is fixed in advance. Peeking at a running experiment
//! and stopping the moment `p < alpha` inflates the Type-I error rate far
//! beyond the nominal level. This module implements **always-valid inference**
//! so an experiment can be inspected at *any* time — and as many times as you
//! like — without inflating the false-positive rate.
//!
//! # The mixture SPRT
//!
//! Both constructions below derive from a single normal-mixture core. Given an
//! asymptotically-normal effect estimate `delta_hat` with variance `se²`, we
//! test `H₀: effect = 0` against the mixing prior `effect ~ N(0, τ²)`. The
//! mixture likelihood ratio is
//!
//! ```text
//! lambda = sqrt(se² / (se² + τ²)) · exp( delta_hat² · τ² / (2 · se² · (se² + τ²)) )
//! ```
//!
//! which is a non-negative martingale under `H₀`. Two dual quantities follow:
//!
//! * **Always-valid p-value.** `p_look = min(1, 1/lambda)` is valid at a single
//!   look; the reported value is the *running minimum* across all looks so far,
//!   `p = min(prev_p, p_look)`, which is monotone non-increasing and remains a
//!   valid anytime p-value (Ville's inequality bounds `P(inf_t p_t ≤ α) ≤ α`).
//! * **Confidence sequence.** Inverting the test gives an anytime-valid
//!   `(1 − α)` confidence interval for the effect that covers the truth at
//!   *every* look simultaneously with probability `≥ 1 − α`:
//!
//!   ```text
//!   radius = sqrt( (2 · se² · (se² + τ²) / τ²) · ln( sqrt((se² + τ²) / se²) / α ) )
//!   CI = [delta_hat − radius, delta_hat + radius]
//!   ```
//!
//! # Per-family adapters
//!
//! The point estimate (`delta_hat`) and its standard error are computed from
//! [`VariantStats`] using the **same formulas** as the matching
//! [`super::frequentist`] function, so the fixed-horizon and sequential views
//! agree on the effect estimate (they differ only in how they convert it into a
//! decision). Adapters are provided for count/Bernoulli, numeric/mean,
//! funnel/final-step-rate, and ratio (delta method) metrics.
//!
//! # Multiplicity
//!
//! [`split_alpha`] splits `alpha` across `K − 1` treatment-vs-control
//! comparisons (Bonferroni), mirroring [`super::frequentist::bonferroni_correct`]
//! but applied to the threshold rather than the p-value, which is the natural
//! way to keep the always-valid guarantee family-wise.

use super::VariantStats;

// ── Config & result types ─────────────────────────────────────────────────────

/// Configuration for a sequential (always-valid) test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SequentialConfig {
    /// Family-wise (or per-comparison) significance level in `(0, 1)`.
    pub alpha: f64,
    /// Mixing-prior variance `τ² > 0`. Larger values concentrate power on
    /// larger effects (the test is most sensitive to effects of size `≈ τ`);
    /// smaller values favour detecting small effects but need more data.
    pub tau_squared: f64,
    /// Minimum per-variant sample size before a result is considered
    /// trustworthy. Below this the adapters return `insufficient_data = true`.
    pub min_sample_size: i64,
}

impl Default for SequentialConfig {
    /// A reasonable default: 5 % significance, unit mixing variance, and a
    /// 100-unit minimum sample size.
    fn default() -> Self {
        Self {
            alpha: 0.05,
            tau_squared: 1.0,
            min_sample_size: 100,
        }
    }
}

/// The result of a single sequential look.
#[derive(Debug, Clone, PartialEq)]
pub struct SequentialResult {
    /// Running-minimum always-valid p-value in `[0, 1]`. Monotone
    /// non-increasing across looks. This is the value to report and to compare
    /// against `alpha`. When `insufficient_data` is `true` this is `1.0`.
    pub always_valid_p: f64,
    /// Whether the always-valid p-value has crossed the significance threshold
    /// (`always_valid_p <= alpha`) — and the look had sufficient data.
    pub p_crossed: bool,
    /// Lower bound of the anytime-valid confidence sequence for the effect.
    /// [`f64::NEG_INFINITY`] when `insufficient_data` is `true`.
    pub ci_lower: f64,
    /// Upper bound of the anytime-valid confidence sequence for the effect.
    /// [`f64::INFINITY`] when `insufficient_data` is `true`.
    pub ci_upper: f64,
    /// The method used. Always `"msprt"` for this engine.
    pub method: String,
    /// `true` when the look is below `min_sample_size` or the underlying
    /// statistics are undefined (non-positive SE, non-finite math). In that
    /// case `always_valid_p = 1.0`, `p_crossed = false`, and the CI is the
    /// whole real line `(-∞, +∞)`.
    pub insufficient_data: bool,
}

impl SequentialResult {
    /// The canonical "not enough data / undefined statistics" result.
    ///
    /// `always_valid_p` is `1.0` (never significant), `p_crossed` is `false`,
    /// and the confidence sequence is the whole real line `(-∞, +∞)` (it
    /// trivially covers any true effect, preserving anytime coverage).
    fn insufficient() -> Self {
        Self {
            always_valid_p: 1.0,
            p_crossed: false,
            ci_lower: f64::NEG_INFINITY,
            ci_upper: f64::INFINITY,
            method: "msprt".to_string(),
            insufficient_data: true,
        }
    }
}

// ── Core engine ────────────────────────────────────────────────────────────────

/// Run one sequential look of the mixture-SPRT engine on an
/// asymptotically-normal effect estimate.
///
/// # Arguments
/// * `delta_hat` — the effect estimate (treatment − control).
/// * `se` — the standard error of `delta_hat` (so `Var(delta_hat) = se²`).
///   Must be strictly positive; otherwise the look is `insufficient_data`.
/// * `n` — the (per-variant or pooled) sample size for this look. Looks with
///   `n < cfg.min_sample_size` are reported as `insufficient_data`.
/// * `cfg` — the [`SequentialConfig`] (`alpha`, `tau_squared`, `min_sample_size`).
/// * `prev_p` — the running-minimum always-valid p-value from the previous
///   look. Seed this with `1.0` at the very first look.
///
/// # Returns
/// A [`SequentialResult`] whose `always_valid_p` is `min(prev_p, p_look)`,
/// clamped to `[0, 1]`. See the [module docs](self) for the closed forms.
///
/// Any non-finite intermediate (e.g. from a degenerate `tau_squared`) collapses
/// to `insufficient_data` so the always-valid guarantee is never silently
/// violated.
#[must_use]
pub fn sequential_test(
    delta_hat: f64,
    se: f64,
    n: i64,
    cfg: &SequentialConfig,
    prev_p: f64,
) -> SequentialResult {
    // Guard rails: too little data, or undefined inputs. Finiteness is checked
    // first so the subsequent strict-positivity comparisons never see a NaN
    // (which would compare `false` and silently slip through).
    if n < cfg.min_sample_size
        || !se.is_finite()
        || !delta_hat.is_finite()
        || !cfg.tau_squared.is_finite()
        || se <= 0.0
        || cfg.tau_squared <= 0.0
        || !(cfg.alpha > 0.0 && cfg.alpha < 1.0)
    {
        return SequentialResult::insufficient();
    }

    let se2 = se * se;
    let tau2 = cfg.tau_squared;
    let sum = se2 + tau2; // se² + τ²

    // Mixture likelihood ratio under H₀ vs the N(0, τ²) mixing prior:
    //   lambda = sqrt(se²/(se²+τ²)) · exp( δ̂²·τ² / (2·se²·(se²+τ²)) )
    // Work in log space so an overwhelming effect (huge exponent) cannot
    // overflow `lambda` to +∞ and spuriously collapse to insufficient_data —
    // we want such a look to report p ≈ 0, not "no data".
    //   ln_lambda = 0.5·ln(se²/(se²+τ²)) + δ̂²·τ² / (2·se²·(se²+τ²))
    let ln_lambda = 0.5 * (se2 / sum).ln() + delta_hat * delta_hat * tau2 / (2.0 * se2 * sum);

    if !ln_lambda.is_finite() {
        return SequentialResult::insufficient();
    }

    // Always-valid p at this look = min(1, 1/lambda) = min(1, exp(-ln_lambda)),
    // then the running minimum across looks. Computing exp(-ln_lambda) is safe:
    // a large positive ln_lambda underflows to 0.0 (correct), and a negative
    // one is capped at 1.0 by the `.min(1.0)`.
    let p_look = (-ln_lambda).exp().min(1.0);
    let prev = if prev_p.is_finite() {
        prev_p.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let always_valid_p = prev.min(p_look).clamp(0.0, 1.0);

    // mSPRT-dual anytime-valid (1 − α) confidence sequence.
    //   radius = sqrt( (2·se²·(se²+τ²)/τ²) · ln( sqrt((se²+τ²)/se²) / α ) )
    let ln_arg = (sum / se2).sqrt() / cfg.alpha;
    let under_sqrt = (2.0 * se2 * sum / tau2) * ln_arg.ln();

    if !under_sqrt.is_finite() || under_sqrt <= 0.0 {
        // Confidence-sequence radius undefined (only happens for absurd alpha
        // where the ln-argument ≤ 1). Treat the whole look as insufficient so
        // we never emit a degenerate / non-covering interval.
        return SequentialResult::insufficient();
    }
    let radius = under_sqrt.sqrt();

    SequentialResult {
        always_valid_p,
        p_crossed: always_valid_p <= cfg.alpha,
        ci_lower: delta_hat - radius,
        ci_upper: delta_hat + radius,
        method: "msprt".to_string(),
        insufficient_data: false,
    }
}

// ── Per-family adapters ─────────────────────────────────────────────────────────

/// Sequential test for **count / Bernoulli** metrics (proportion difference).
///
/// Mirrors [`super::frequentist::analyze_count`]: the effect is the lift
/// `p₂ − p₁` and the standard error is the *unpooled* lift SE
/// `sqrt(p₁(1−p₁)/n₁ + p₂(1−p₂)/n₂)`. The look's sample size is
/// `min(n₁, n₂)`, so a still-small arm gates the whole comparison.
///
/// Returns `insufficient_data` when either `conversions` field is `None`, when
/// a sample size is zero, or when the SE is non-positive (e.g. both
/// proportions are 0 or 1).
#[must_use]
pub fn sequential_count(
    control: &VariantStats,
    variant: &VariantStats,
    cfg: &SequentialConfig,
    prev_p: f64,
) -> SequentialResult {
    let (n1, n2) = (control.sample_size, variant.sample_size);
    let (Some(c1), Some(c2)) = (control.conversions, variant.conversions) else {
        return SequentialResult::insufficient();
    };
    if n1 <= 0 || n2 <= 0 {
        return SequentialResult::insufficient();
    }
    let (n1f, n2f) = (n1 as f64, n2 as f64);
    let p1 = c1 as f64 / n1f;
    let p2 = c2 as f64 / n2f;
    let delta_hat = p2 - p1;
    let se = (p1 * (1.0 - p1) / n1f + p2 * (1.0 - p2) / n2f).sqrt();
    sequential_test(delta_hat, se, n1.min(n2), cfg, prev_p)
}

/// Sequential test for **continuous numeric** metrics (mean difference).
///
/// Mirrors [`super::frequentist::analyze_numeric`] (Welch): the effect is the
/// mean difference `mean₂ − mean₁` and the standard error is
/// `sqrt(var₁/n₁ + var₂/n₂)`. The look's sample size is `min(n₁, n₂)`.
///
/// Returns `insufficient_data` when either `mean`/`variance` is `None`, a
/// sample size is zero, or the SE is non-positive (both variances zero).
#[must_use]
pub fn sequential_numeric(
    control: &VariantStats,
    variant: &VariantStats,
    cfg: &SequentialConfig,
    prev_p: f64,
) -> SequentialResult {
    let (n1, n2) = (control.sample_size, variant.sample_size);
    let (Some(m1), Some(m2)) = (control.mean, variant.mean) else {
        return SequentialResult::insufficient();
    };
    let (Some(v1), Some(v2)) = (control.variance, variant.variance) else {
        return SequentialResult::insufficient();
    };
    if n1 <= 0 || n2 <= 0 {
        return SequentialResult::insufficient();
    }
    let (n1f, n2f) = (n1 as f64, n2 as f64);
    let delta_hat = m2 - m1;
    let se = (v1 / n1f + v2 / n2f).sqrt();
    sequential_test(delta_hat, se, n1.min(n2), cfg, prev_p)
}

/// Sequential test for **funnel** metrics (final-step conversion-rate diff).
///
/// Mirrors [`super::frequentist::analyze_funnel`]: the proportion is
/// `VariantStats::conversion_rate` (the end-to-end completion rate) and the
/// denominator is the top-of-funnel `sample_size`. Effect is the rate lift
/// `r₂ − r₁` with unpooled SE; look sample size is `min(n₁, n₂)`.
///
/// Returns `insufficient_data` when either `conversion_rate` is `None`, a
/// sample size is zero, or the SE is non-positive.
#[must_use]
pub fn sequential_funnel(
    control: &VariantStats,
    variant: &VariantStats,
    cfg: &SequentialConfig,
    prev_p: f64,
) -> SequentialResult {
    let (n1, n2) = (control.sample_size, variant.sample_size);
    let (Some(r1), Some(r2)) = (control.conversion_rate, variant.conversion_rate) else {
        return SequentialResult::insufficient();
    };
    if n1 <= 0 || n2 <= 0 {
        return SequentialResult::insufficient();
    }
    let (n1f, n2f) = (n1 as f64, n2 as f64);
    let delta_hat = r2 - r1;
    let se = (r1 * (1.0 - r1) / n1f + r2 * (1.0 - r2) / n2f).sqrt();
    sequential_test(delta_hat, se, n1.min(n2), cfg, prev_p)
}

/// Per-group ratio-metric sufficient statistics (one variant).
///
/// A ratio metric's per-group point estimate is `R = num_sum / den_sum`. The
/// delta method needs the within-group second moments to estimate `Var(R)`.
/// These are the exact same sufficient statistics carried by the interaction
/// ratio path (`RatioAgg`: sums + second moments), exposed here per group so a
/// two-group sequential ratio test can be run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RatioGroupStats {
    /// Number of observations (units) in the group.
    pub n: i64,
    /// Sum of the numerator over the group's units.
    pub num_sum: f64,
    /// Sum of the denominator over the group's units.
    pub den_sum: f64,
    /// Sum of squared numerators (second moment).
    pub num_sq_sum: f64,
    /// Sum of squared denominators (second moment).
    pub den_sq_sum: f64,
    /// Sum of numerator×denominator products (cross moment).
    pub num_den_sum: f64,
}

impl RatioGroupStats {
    /// Delta-method point estimate `R` and variance `Var(R)` for this group,
    /// matching the interaction ratio path exactly:
    ///
    /// ```text
    /// Var(R) ≈ (1 / mean_den²) · (var_num − 2·R·cov + R²·var_den) / n
    /// ```
    ///
    /// Returns `None` when the group is degenerate (`den_sum ≤ 0`,
    /// `mean_den ≤ 0`, `n < 2`, or a non-finite / non-positive variance).
    ///
    /// `pub(crate)` so [`super::frequentist::analyze_ratio`] /
    /// [`super::bayesian::analyze_ratio`] can build their contrasts from the
    /// SAME delta-method point + variance — this is the single source of truth
    /// for the ratio formula across the fixed-horizon, sequential, and Bayesian
    /// views (see [`super::frequentist::analyze_ratio`]).
    pub(crate) fn ratio_var(&self) -> Option<(f64, f64)> {
        if self.n < 2 || self.den_sum <= 0.0 {
            return None;
        }
        let n = self.n as f64;
        let mean_num = self.num_sum / n;
        let mean_den = self.den_sum / n;
        if !mean_den.is_finite() || mean_den <= 0.0 {
            return None;
        }
        let r = self.num_sum / self.den_sum;

        let var_num = self.num_sq_sum / n - mean_num * mean_num;
        let var_den = self.den_sq_sum / n - mean_den * mean_den;
        let cov = self.num_den_sum / n - mean_num * mean_den;

        // Delta method: Var(R) ≈ (1/mean_den²)·(var_num − 2R·cov + R²·var_den)/n.
        let var_r = (var_num - 2.0 * r * cov + r * r * var_den) / (mean_den * mean_den * n);

        if !var_r.is_finite() || var_r <= 0.0 || !r.is_finite() {
            return None;
        }
        Some((r, var_r))
    }
}

/// Sequential test for **ratio** metrics via the delta method.
///
/// Ratio sufficient statistics are not present on [`VariantStats`], so — like
/// the interaction ratio path — this adapter takes explicit per-group
/// [`RatioGroupStats`]. The effect is the difference of per-group ratios
/// `R₂ − R₁`; because the two groups are independent, the variance of the
/// difference is `Var(R₁) + Var(R₂)`, each computed by the same delta-method
/// formula as `interaction::ratio`. The look's sample size is `min(n₁, n₂)`.
///
/// Returns `insufficient_data` when either group is degenerate (see
/// [`RatioGroupStats::ratio_var`]).
#[must_use]
pub fn sequential_ratio(
    control: &RatioGroupStats,
    variant: &RatioGroupStats,
    cfg: &SequentialConfig,
    prev_p: f64,
) -> SequentialResult {
    let (Some((r1, var1)), Some((r2, var2))) = (control.ratio_var(), variant.ratio_var()) else {
        return SequentialResult::insufficient();
    };
    let delta_hat = r2 - r1;
    let se = (var1 + var2).sqrt();
    sequential_test(delta_hat, se, control.n.min(variant.n), cfg, prev_p)
}

// ── Multiplicity ─────────────────────────────────────────────────────────────

/// Split `alpha` across the treatment-vs-control comparisons of a `K`-variant
/// experiment (Bonferroni), returning a per-comparison [`SequentialConfig`].
///
/// In a `K`-variant experiment there are `K − 1` non-control variants, hence
/// `K − 1` comparisons; each gets `alpha / (K − 1)`. This is the always-valid
/// analogue of [`super::frequentist::bonferroni_correct`] (which multiplies the
/// p-value by the number of comparisons): applying the divisor to the threshold
/// keeps the family-wise always-valid guarantee, since
/// `P(∃ comparison: p ≤ α/(K−1)) ≤ Σ α/(K−1) = α`.
///
/// `num_variants` is clamped to `≥ 2` (a single variant is its own control and
/// has zero comparisons; we treat the minimum sensible case as 2 variants → 1
/// comparison, leaving `alpha` unchanged). `tau_squared` and `min_sample_size`
/// are carried through unchanged.
#[must_use]
pub fn split_alpha(base: &SequentialConfig, num_variants: i64) -> SequentialConfig {
    let comparisons = (num_variants.max(2) - 1) as f64;
    SequentialConfig {
        alpha: base.alpha / comparisons,
        tau_squared: base.tau_squared,
        min_sample_size: base.min_sample_size,
    }
}

#[cfg(test)]
mod tests;
