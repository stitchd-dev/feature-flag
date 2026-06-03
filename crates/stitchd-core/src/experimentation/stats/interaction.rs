//! Two-way (cross-experiment) interaction significance tests.
//!
//! When two experiments A and B run concurrently over overlapping traffic, a
//! unit lands in one *cell* of the A-factor × B-factor grid (e.g. A's
//! `control`/`treatment` crossed with B's `control`/`treatment`). An
//! *interaction* exists when the effect of one experiment depends on the
//! variant a unit saw in the other — i.e. the per-cell outcomes are
//! **non-additive**.
//!
//! This module provides two pure, I/O-free tests:
//!
//! - [`binary_interaction`] — for conversion (binary success/failure) metrics.
//!   For the 2×2 case it computes the Wald interaction contrast on proportions
//!   `(p11 − p10) − (p01 − p00)` and a two-sided normal-CDF p-value; for the
//!   general R×C case it runs a Pearson chi-square test on the no-interaction
//!   (additive) expected counts over both successes and failures.
//! - [`continuous_interaction`] — for numeric (revenue/duration) metrics. A
//!   two-way ANOVA interaction F-test using only per-cell `(n, sum, sum_sq)`
//!   sufficient statistics.
//!
//! Both return the shared [`InteractionResult`] shape. All distribution math
//! reuses [`super::srm::chi_square_sf`] for chi-square tails; the normal CDF
//! and the regularized incomplete beta (for the F-distribution tail) are
//! implemented locally here to keep the module self-contained.

pub(crate) use super::srm::chi_square_sf;

// ── N-way decomposition submodules (Phase 2 worker-wave) ─────────────────────
// Each owns one metric-family × inference-model. They share the contract types
// and distribution helpers defined in this module (accessible via `super::`).
mod anova;
mod bayes_binary;
mod bayes_continuous;
mod loglinear;
mod ratio;

// Flat facade over the private submodules — the public N-way entry points.
pub use anova::continuous_terms;
pub use bayes_binary::binary_bayes;
pub use bayes_continuous::{continuous_bayes, ratio_bayes};
pub use loglinear::binary_terms;
pub use ratio::ratio_terms;

/// Significance level for the `significant` flag (two-sided, alpha = 0.05).
pub(crate) const ALPHA: f64 = 0.05;

// ── Public input/output types ──────────────────────────────────────────────

/// One cell of the A-factor × B-factor grid for a **binary** (conversion)
/// metric.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BinaryCell {
    /// Zero-based level index on the A factor (e.g. variant of experiment A).
    pub a_level: usize,
    /// Zero-based level index on the B factor (e.g. variant of experiment B).
    pub b_level: usize,
    /// Number of units (trials) observed in this cell.
    pub n: u64,
    /// Number of successes (conversions) observed in this cell.
    pub successes: u64,
}

/// One cell of the A-factor × B-factor grid for a **continuous** (numeric)
/// metric, summarised by its sufficient statistics.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContinuousCell {
    /// Zero-based level index on the A factor.
    pub a_level: usize,
    /// Zero-based level index on the B factor.
    pub b_level: usize,
    /// Number of observations in this cell.
    pub n: u64,
    /// Sum of the metric values in this cell (`Σ x`).
    pub sum: f64,
    /// Sum of squared metric values in this cell (`Σ x²`).
    pub sum_sq: f64,
}

/// Result of a two-way interaction significance test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct InteractionResult {
    /// Effect-size estimate. For the binary 2×2 case this is the interaction
    /// contrast `(p11 − p10) − (p01 − p00)`; for the general binary R×C case
    /// it is the Pearson chi-square statistic; for the continuous case it is
    /// the ANOVA interaction F statistic (or the largest cell-mean
    /// interaction deviation when the F-test is degenerate). `NaN` when
    /// `insufficient_data` is true.
    pub estimate: f64,
    /// The test statistic (z, chi-square, or F depending on the path). `NaN`
    /// when `insufficient_data` is true.
    pub statistic: f64,
    /// Two-sided p-value for the null hypothesis of *no interaction*. `NaN`
    /// when `insufficient_data` is true.
    pub p_value: f64,
    /// Degrees of freedom of the interaction term, `(R−1)(C−1)`.
    pub df: u32,
    /// Whether the interaction is significant at `alpha = 0.05`. Always
    /// `false` when `insufficient_data` is true.
    pub significant: bool,
    /// Whether the inputs were too sparse/degenerate to run the test (a
    /// near-zero margin, too few units, or zero residual variance). When true,
    /// `estimate`/`statistic`/`p_value` are `NaN` and `significant` is
    /// `false`.
    pub insufficient_data: bool,
}

impl InteractionResult {
    /// Construct the canonical "not enough data" result.
    pub(crate) fn insufficient(df: u32) -> Self {
        InteractionResult {
            estimate: f64::NAN,
            statistic: f64::NAN,
            p_value: f64::NAN,
            df,
            significant: false,
            insufficient_data: true,
        }
    }
}

// ── N-way decomposition contract (Phase 2 seam) ──────────────────────────────
//
// The pairwise functions above ([`binary_interaction`] / [`continuous_interaction`])
// remain the regression baseline. The N-way decomposition generalizes them: a
// candidate tuple of `order` experiments (2 or 3) is described by a k-factor grid
// of cells, and each decomposition emits a *set* of [`TermResult`]s — one per
// main effect, pairwise interaction, and (order 3) the three-way interaction.
//
// Frequentist terms are produced by [`loglinear`] (binary/funnel), [`anova`]
// (continuous), and [`ratio`] (ratio, delta method). Bayesian posteriors are
// produced independently by [`bayes_binary`] and [`bayes_continuous`] and merged
// by [`TermKind`] (so the two inference models stay decoupled across workers).

/// Which decomposition term a [`TermResult`] represents. Factor indices are
/// 0-based positions in the participating-experiment tuple (tuple order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TermKind {
    /// Single-experiment main effect.
    Main { factor: usize },
    /// Pairwise interaction between two experiments.
    TwoWay { a: usize, b: usize },
    /// Three-way interaction among three experiments.
    ThreeWay { a: usize, b: usize, c: usize },
}

impl TermKind {
    /// Interaction order of this term (1 = main, 2 = pairwise, 3 = three-way).
    #[must_use]
    pub fn order(&self) -> u8 {
        match self {
            TermKind::Main { .. } => 1,
            TermKind::TwoWay { .. } => 2,
            TermKind::ThreeWay { .. } => 3,
        }
    }
}

/// Bayesian posterior summary for one interaction term. The "effect" is the
/// interaction contrast appropriate to the metric family (e.g. a
/// difference-in-differences of cell rates for binary).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BayesianInteraction {
    /// Posterior probability the interaction effect is non-negligible
    /// (outside the region of practical equivalence around 0).
    pub prob: f64,
    /// Posterior mean of the interaction effect.
    pub expected: f64,
    /// Lower bound of the (central) credible interval.
    pub ci_low: f64,
    /// Upper bound of the (central) credible interval.
    pub ci_high: f64,
}

/// One decomposed interaction term: its identity, the Frequentist result, and
/// an optional Bayesian posterior (attached during routing once both inference
/// workers have run).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TermResult {
    /// Which effect/interaction this row represents.
    pub kind: TermKind,
    /// Frequentist test result for this term.
    pub freq: InteractionResult,
    /// Bayesian posterior for this term, when computed.
    pub bayes: Option<BayesianInteraction>,
}

/// One cell of a k-factor contingency grid for a **binary** (conversion/funnel)
/// metric. `levels[d]` is the dense 0-based level index on factor `d`.
#[derive(Debug, Clone, PartialEq)]
pub struct NdBinaryCell {
    /// Dense level index per factor, in tuple order (`len()` == interaction order).
    pub levels: Vec<usize>,
    /// Units (trials) in this cell.
    pub n: u64,
    /// Successes (conversions / funnel final-step reached) in this cell.
    pub successes: u64,
}

/// One cell of a k-factor grid for a **continuous** metric, summarised by its
/// sufficient statistics.
#[derive(Debug, Clone, PartialEq)]
pub struct NdContinuousCell {
    /// Dense level index per factor, in tuple order.
    pub levels: Vec<usize>,
    /// Observations in this cell.
    pub n: u64,
    /// Σ metric value.
    pub sum: f64,
    /// Σ metric value².
    pub sum_sq: f64,
}

/// One cell of a k-factor grid for a **ratio** metric (numerator/denominator
/// sums + the second moments the delta method needs for the contrast variance).
#[derive(Debug, Clone, PartialEq)]
pub struct NdRatioCell {
    /// Dense level index per factor, in tuple order.
    pub levels: Vec<usize>,
    /// Observations (denominator units) in this cell.
    pub n: u64,
    /// Σ numerator.
    pub num_sum: f64,
    /// Σ denominator.
    pub den_sum: f64,
    /// Σ numerator².
    pub num_sq_sum: f64,
    /// Σ denominator².
    pub den_sq_sum: f64,
    /// Σ numerator·denominator.
    pub num_den_sum: f64,
}

// ── Binary (conversion) interaction ────────────────────────────────────────

/// Test for a two-way interaction on a **binary** (conversion) metric.
///
/// See the module docs for the method. Returns an
/// [`InteractionResult::insufficient`] result when the grid is empty, a margin
/// is essentially zero, or the total sample is too small to test.
pub fn binary_interaction(cells: &[BinaryCell]) -> InteractionResult {
    if cells.is_empty() {
        return InteractionResult::insufficient(0);
    }

    // Determine grid dimensions from the maximum level indices present.
    let rows = cells.iter().map(|c| c.a_level).max().unwrap() + 1;
    let cols = cells.iter().map(|c| c.b_level).max().unwrap() + 1;

    if rows < 2 || cols < 2 {
        // A single row or column has no interaction term to test.
        return InteractionResult::insufficient(0);
    }

    let df = ((rows - 1) * (cols - 1)) as u32;

    // Accumulate trials and successes per cell (cells may repeat or be
    // missing; missing cells default to zero).
    let mut n = vec![vec![0u64; cols]; rows];
    let mut s = vec![vec![0u64; cols]; rows];
    for c in cells {
        n[c.a_level][c.b_level] = n[c.a_level][c.b_level].saturating_add(c.n);
        s[c.a_level][c.b_level] = s[c.a_level][c.b_level].saturating_add(c.successes);
    }

    let total_n: u64 = n.iter().flatten().sum();
    // Require a minimum total sample so we never report a spurious hit on a
    // handful of units.
    if total_n < 20 {
        return InteractionResult::insufficient(df);
    }

    if rows == 2 && cols == 2 {
        binary_2x2(&n, &s, df)
    } else {
        binary_rxc(&n, &s, rows, cols, df)
    }
}

/// 2×2 Wald interaction contrast on proportions.
fn binary_2x2(n: &[Vec<u64>], s: &[Vec<u64>], df: u32) -> InteractionResult {
    // Every cell must have observations; otherwise a proportion is undefined.
    if n.iter().flatten().any(|&nij| nij == 0) {
        return InteractionResult::insufficient(df);
    }

    let p = |r: usize, c: usize| s[r][c] as f64 / n[r][c] as f64;
    let p00 = p(0, 0);
    let p01 = p(0, 1);
    let p10 = p(1, 0);
    let p11 = p(1, 1);

    // Interaction contrast.
    let est = (p11 - p10) - (p01 - p00);

    // Wald standard error from the four independent cell proportions.
    let var = |r: usize, c: usize| {
        let pr = p(r, c);
        pr * (1.0 - pr) / n[r][c] as f64
    };
    let se = (var(0, 0) + var(0, 1) + var(1, 0) + var(1, 1)).sqrt();

    // If every cell is degenerate (all 0% or all 100%), SE is 0 and the test
    // is undefined — there is no variance to detect an interaction against.
    if se <= 0.0 || !se.is_finite() {
        return InteractionResult::insufficient(df);
    }

    let z = est / se;
    let p_value = z_to_p(z);
    InteractionResult {
        estimate: est,
        statistic: z,
        p_value,
        df,
        significant: p_value < ALPHA,
        insufficient_data: false,
    }
}

/// General R×C Pearson chi-square test for the interaction term.
///
/// Fits no-interaction (additive) expected successes per cell from the overall
/// success rate scaled by the row/column structure, then compares observed vs
/// expected over **both** successes and failures.
fn binary_rxc(
    n: &[Vec<u64>],
    s: &[Vec<u64>],
    rows: usize,
    cols: usize,
    df: u32,
) -> InteractionResult {
    // Under the no-interaction null, P(success | cell) is modelled as a
    // product of row and column success propensities. The simplest unbiased
    // expected-success count under independence of "success" from the cell
    // structure (given the row & column totals of trials) is:
    //   E_success[r][c] = n[r][c] * (overall success rate)
    // generalised to row/column rates via the standard contingency-table
    // expected count on the success/failure × cell layout. We use the
    // marginal success rate per row and per column relative to the grand rate.
    let grand_n: f64 = n.iter().flatten().map(|&x| x as f64).sum();
    let grand_s: f64 = s.iter().flatten().map(|&x| x as f64).sum();
    if grand_n <= 0.0 {
        return InteractionResult::insufficient(df);
    }
    let grand_rate = grand_s / grand_n;
    if grand_rate <= 0.0 || grand_rate >= 1.0 {
        // No outcome variance at all — cannot detect an interaction.
        return InteractionResult::insufficient(df);
    }

    // Row and column success rates (trial-weighted).
    let mut row_n = vec![0.0f64; rows];
    let mut row_s = vec![0.0f64; rows];
    let mut col_n = vec![0.0f64; cols];
    let mut col_s = vec![0.0f64; cols];
    for r in 0..rows {
        for c in 0..cols {
            row_n[r] += n[r][c] as f64;
            row_s[r] += s[r][c] as f64;
            col_n[c] += n[r][c] as f64;
            col_s[c] += s[r][c] as f64;
        }
    }

    // Any empty row or column margin makes the additive fit undefined.
    if row_n.iter().any(|&x| x <= 0.0) || col_n.iter().any(|&x| x <= 0.0) {
        return InteractionResult::insufficient(df);
    }

    // Expected success rate per cell under the additive (no-interaction)
    // log-free model: r_cell = row_rate * col_rate / grand_rate, clamped to a
    // valid probability. This is the multiplicative-margins fit standard for
    // contingency-table interaction tests.
    let mut chi_sq = 0.0f64;
    for r in 0..rows {
        let row_rate = row_s[r] / row_n[r];
        for c in 0..cols {
            let nij = n[r][c] as f64;
            if nij <= 0.0 {
                continue;
            }
            let col_rate = col_s[c] / col_n[c];
            let exp_rate = row_rate * col_rate / grand_rate;
            // The multiplicative-margins fit can predict a rate outside [0,1] for
            // skewed grids. Clamping would silently bias the statistic toward
            // non-significance (masking real interactions); instead abstain — the
            // additive model does not fit this table, so the interaction cannot be
            // tested reliably with this approximation.
            if !(0.0..=1.0).contains(&exp_rate) {
                return InteractionResult::insufficient(df);
            }
            let exp_succ = nij * exp_rate;
            let exp_fail = nij * (1.0 - exp_rate);
            let obs_succ = s[r][c] as f64;
            let obs_fail = nij - obs_succ;

            if exp_succ > 0.0 {
                let d = obs_succ - exp_succ;
                chi_sq += d * d / exp_succ;
            }
            if exp_fail > 0.0 {
                let d = obs_fail - exp_fail;
                chi_sq += d * d / exp_fail;
            }
        }
    }

    let p_value = chi_square_sf(chi_sq, df as f64);
    InteractionResult {
        estimate: chi_sq,
        statistic: chi_sq,
        p_value,
        df,
        significant: p_value < ALPHA,
        insufficient_data: false,
    }
}

// ── Continuous (numeric) interaction ───────────────────────────────────────

/// Test for a two-way interaction on a **continuous** (numeric) metric via a
/// two-way ANOVA interaction F-test.
///
/// Uses only the per-cell sufficient statistics `(n, sum, sum_sq)`. Returns an
/// [`InteractionResult::insufficient`] result when the design is empty, has
/// fewer than two levels on a factor, has no residual degrees of freedom, or
/// has zero within-cell variance.
pub fn continuous_interaction(cells: &[ContinuousCell]) -> InteractionResult {
    if cells.is_empty() {
        return InteractionResult::insufficient(0);
    }

    let rows = cells.iter().map(|c| c.a_level).max().unwrap() + 1;
    let cols = cells.iter().map(|c| c.b_level).max().unwrap() + 1;
    if rows < 2 || cols < 2 {
        return InteractionResult::insufficient(0);
    }

    let df_inter = ((rows - 1) * (cols - 1)) as u32;

    // Accumulate per-cell sufficient statistics.
    let mut cell_n = vec![vec![0.0f64; cols]; rows];
    let mut cell_sum = vec![vec![0.0f64; cols]; rows];
    let mut cell_ss = vec![vec![0.0f64; cols]; rows];
    for c in cells {
        cell_n[c.a_level][c.b_level] += c.n as f64;
        cell_sum[c.a_level][c.b_level] += c.sum;
        cell_ss[c.a_level][c.b_level] += c.sum_sq;
    }

    // Every cell must be non-empty for a balanced interaction decomposition;
    // an empty cell makes the additive model unidentifiable.
    if cell_n.iter().flatten().any(|&x| x <= 0.0) {
        return InteractionResult::insufficient(df_inter);
    }

    let total_n: f64 = cell_n.iter().flatten().sum();
    let cell_count = (rows * cols) as f64;
    let df_err = total_n - cell_count;
    if df_err <= 0.0 {
        // No replication within cells → no error variance to test against.
        return InteractionResult::insufficient(df_inter);
    }

    // Grand mean.
    let grand_sum: f64 = cell_sum.iter().flatten().sum();
    let grand_mean = grand_sum / total_n;

    // Cell, row, and column means (all trial-weighted by cell n).
    let cell_mean = |r: usize, c: usize| cell_sum[r][c] / cell_n[r][c];

    let mut row_n = vec![0.0f64; rows];
    let mut row_sum = vec![0.0f64; rows];
    let mut col_n = vec![0.0f64; cols];
    let mut col_sum = vec![0.0f64; cols];
    for r in 0..rows {
        for c in 0..cols {
            row_n[r] += cell_n[r][c];
            row_sum[r] += cell_sum[r][c];
            col_n[c] += cell_n[r][c];
            col_sum[c] += cell_sum[r][c];
        }
    }
    let row_mean: Vec<f64> = (0..rows).map(|r| row_sum[r] / row_n[r]).collect();
    let col_mean: Vec<f64> = (0..cols).map(|c| col_sum[c] / col_n[c]).collect();

    // SS_error = Σ_cells (Σ x² − (Σ x)² / n_cell).
    let mut ss_error = 0.0f64;
    for r in 0..rows {
        for c in 0..cols {
            let nij = cell_n[r][c];
            ss_error += cell_ss[r][c] - cell_sum[r][c] * cell_sum[r][c] / nij;
        }
    }
    // Guard against tiny negative values from floating-point cancellation.
    if ss_error < 0.0 {
        ss_error = 0.0;
    }

    // SS_interaction = Σ_cells n_cell * (cell_mean − row_mean − col_mean + grand_mean)².
    let mut ss_inter = 0.0f64;
    let mut max_dev = 0.0f64;
    for r in 0..rows {
        for c in 0..cols {
            let resid = cell_mean(r, c) - row_mean[r] - col_mean[c] + grand_mean;
            ss_inter += cell_n[r][c] * resid * resid;
            if resid.abs() > max_dev.abs() {
                max_dev = resid;
            }
        }
    }

    // Zero residual variance → F is undefined (or +inf); treat as
    // insufficient rather than reporting a spurious infinite F.
    if ss_error <= 0.0 {
        return InteractionResult::insufficient(df_inter);
    }

    let ms_inter = ss_inter / df_inter as f64;
    let ms_error = ss_error / df_err;
    let f = ms_inter / ms_error;

    if !f.is_finite() {
        return InteractionResult::insufficient(df_inter);
    }

    let p_value = fdist_sf(f, df_inter as f64, df_err);
    // estimate: report F when finite & meaningful, else largest interaction
    // deviation. We use F as the primary estimate per the spec.
    let estimate = if f.is_finite() { f } else { max_dev };
    InteractionResult {
        estimate,
        statistic: f,
        p_value,
        df: df_inter,
        significant: p_value < ALPHA,
        insufficient_data: false,
    }
}

// ── Local distribution helpers ─────────────────────────────────────────────

/// Approximation of the error function (Abramowitz & Stegun 7.1.26).
/// Maximum absolute error < 1.5 × 10⁻⁷.
fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0_f64 } else { 1.0_f64 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    sign * (1.0 - poly * (-x * x).exp())
}

/// Standard normal CDF: Φ(z) = ½(1 + erf(z/√2)).
#[inline]
pub(crate) fn norm_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

/// Two-tailed p-value from the standard normal distribution.
#[inline]
pub(crate) fn z_to_p(z: f64) -> f64 {
    2.0 * norm_cdf(-z.abs())
}

/// Natural-log of the gamma function (Lanczos approximation, g = 7).
pub(crate) fn lgamma(x: f64) -> f64 {
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

/// Regularized incomplete beta function `I_x(a, b)` via Lentz's
/// continued-fraction algorithm (Numerical Recipes §6.4).
pub(crate) fn betai(x: f64, a: f64, b: f64) -> f64 {
    if !(0.0..=1.0).contains(&x) {
        return f64::NAN;
    }
    if x == 0.0 {
        return 0.0;
    }
    if x == 1.0 {
        return 1.0;
    }
    // Symmetry relation for faster convergence.
    if x > (a + 1.0) / (a + b + 2.0) {
        return 1.0 - betai(1.0 - x, b, a);
    }

    let ln_beta = lgamma(a) + lgamma(b) - lgamma(a + b);
    let front = (x.ln() * a + (1.0 - x).ln() * b - ln_beta).exp() / a;

    const MAX_ITER: usize = 300;
    const EPS: f64 = 1.0e-12;
    const FPMIN: f64 = 1.0e-300;

    let mut c = 1.0_f64;
    let mut d = 1.0 - (a + b) * x / (a + 1.0);
    d = if d.abs() < FPMIN { FPMIN } else { d };
    d = 1.0 / d;
    let mut h = d;

    for m in 1..=MAX_ITER {
        let m = m as f64;
        // Even step.
        let num = m * (b - m) * x / ((a + 2.0 * m - 1.0) * (a + 2.0 * m));
        d = 1.0 + num * d;
        d = if d.abs() < FPMIN { FPMIN } else { d };
        c = 1.0 + num / c;
        c = if c.abs() < FPMIN { FPMIN } else { c };
        d = 1.0 / d;
        h *= d * c;
        // Odd step.
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

/// Upper-tail (survival) function of the F-distribution with `(d1, d2)` degrees
/// of freedom evaluated at `f`: `P(F ≥ f)`.
///
/// Uses the relation `P(F ≥ f) = I_{d2/(d2 + d1·f)}(d2/2, d1/2)`.
pub(crate) fn fdist_sf(f: f64, d1: f64, d2: f64) -> f64 {
    if f <= 0.0 {
        return 1.0;
    }
    if !f.is_finite() {
        return 0.0;
    }
    if d1 <= 0.0 || d2 <= 0.0 {
        return f64::NAN;
    }
    let x = d2 / (d2 + d1 * f);
    betai(x, d2 / 2.0, d1 / 2.0)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn bcell(a: usize, b: usize, n: u64, s: u64) -> BinaryCell {
        BinaryCell {
            a_level: a,
            b_level: b,
            n,
            successes: s,
        }
    }

    /// Build a continuous cell from explicit values (computes n/sum/sum_sq).
    fn ccell_from_vals(a: usize, b: usize, vals: &[f64]) -> ContinuousCell {
        let n = vals.len() as u64;
        let sum: f64 = vals.iter().sum();
        let sum_sq: f64 = vals.iter().map(|v| v * v).sum();
        ContinuousCell {
            a_level: a,
            b_level: b,
            n,
            sum,
            sum_sq,
        }
    }

    /// Build a continuous cell with mean `m` and tiny jitter so within-cell
    /// variance is small but non-zero, repeated `n` times.
    fn ccell_mean(a: usize, b: usize, m: f64, n: u64) -> ContinuousCell {
        let mut vals = Vec::with_capacity(n as usize);
        for i in 0..n {
            // Symmetric jitter that sums to ~0, giving a controlled small variance.
            let jitter = if i % 2 == 0 { 1.0 } else { -1.0 };
            vals.push(m + jitter);
        }
        ccell_from_vals(a, b, &vals)
    }

    // ── distribution helper sanity ──────────────────────────────────────────

    #[test]
    fn norm_cdf_at_196_is_975() {
        assert!((norm_cdf(1.96) - 0.975).abs() < 1e-3);
    }

    #[test]
    fn z_to_p_at_196_is_05() {
        assert!((z_to_p(1.96) - 0.05).abs() < 1e-3);
    }

    /// F = 1 with equal df has p in the middle of the distribution (~0.5 for
    /// large equal df). Sanity: p is a valid probability and decreasing in F.
    #[test]
    fn fdist_sf_is_valid_probability_and_monotone() {
        let p_small = fdist_sf(0.5, 2.0, 50.0);
        let p_big = fdist_sf(8.0, 2.0, 50.0);
        assert!((0.0..=1.0).contains(&p_small));
        assert!((0.0..=1.0).contains(&p_big));
        assert!(p_big < p_small, "p({p_big}) should be < p({p_small})");
    }

    /// Known F critical value: F(2, 50) at the 0.05 upper tail ≈ 3.18.
    #[test]
    fn fdist_sf_critical_value_df_2_50() {
        let p = fdist_sf(3.183, 2.0, 50.0);
        assert!((p - 0.05).abs() < 0.005, "expected ≈0.05, got {p}");
    }

    /// F ≤ 0 → p = 1.
    #[test]
    fn fdist_sf_at_zero_is_one() {
        assert_eq!(fdist_sf(0.0, 2.0, 10.0), 1.0);
    }

    // ── P5.T1: binary 2×2 ───────────────────────────────────────────────────

    /// (a) Strong planted interaction: p00=p01=p10=.10, p11=.40 → significant.
    #[test]
    fn binary_2x2_strong_interaction_is_significant() {
        let cells = [
            bcell(0, 0, 1000, 100), // .10
            bcell(0, 1, 1000, 100), // .10
            bcell(1, 0, 1000, 100), // .10
            bcell(1, 1, 1000, 400), // .40
        ];
        let r = binary_interaction(&cells);
        assert!(!r.insufficient_data);
        assert_eq!(r.df, 1);
        // est = (.40-.10) - (.10-.10) = 0.30
        assert!((r.estimate - 0.30).abs() < 1e-9, "est={}", r.estimate);
        assert!(r.p_value < 0.05, "p={}", r.p_value);
        assert!(r.significant);
    }

    /// (b) Purely additive effects → NOT significant (no false positive).
    /// p00=.10, p01=.20, p10=.30, p11=.40 → contrast = (.40-.30)-(.20-.10)=0.
    #[test]
    fn binary_2x2_additive_is_not_significant() {
        let cells = [
            bcell(0, 0, 5000, 500),  // .10
            bcell(0, 1, 5000, 1000), // .20
            bcell(1, 0, 5000, 1500), // .30
            bcell(1, 1, 5000, 2000), // .40
        ];
        let r = binary_interaction(&cells);
        assert!(!r.insufficient_data);
        assert!(
            r.estimate.abs() < 1e-9,
            "est should be ≈0, got {}",
            r.estimate
        );
        assert!(r.p_value > 0.05, "p={}", r.p_value);
        assert!(!r.significant);
    }

    /// (c) Tiny samples → insufficient_data, not a spurious hit.
    #[test]
    fn binary_2x2_tiny_samples_is_insufficient() {
        let cells = [
            bcell(0, 0, 2, 0),
            bcell(0, 1, 2, 0),
            bcell(1, 0, 2, 0),
            bcell(1, 1, 2, 2),
        ];
        let r = binary_interaction(&cells);
        assert!(r.insufficient_data);
        assert!(!r.significant);
        assert!(r.estimate.is_nan());
        assert!(r.p_value.is_nan());
    }

    /// Empty cell in 2×2 → insufficient (proportion undefined), even with
    /// enough total volume elsewhere.
    #[test]
    fn binary_2x2_empty_cell_is_insufficient() {
        let cells = [
            bcell(0, 0, 100, 10),
            bcell(0, 1, 100, 10),
            bcell(1, 0, 100, 10),
            bcell(1, 1, 0, 0), // missing cell
        ];
        let r = binary_interaction(&cells);
        assert!(r.insufficient_data);
    }

    /// All cells 0% → no variance → insufficient (SE = 0).
    #[test]
    fn binary_2x2_no_variance_is_insufficient() {
        let cells = [
            bcell(0, 0, 100, 0),
            bcell(0, 1, 100, 0),
            bcell(1, 0, 100, 0),
            bcell(1, 1, 100, 0),
        ];
        let r = binary_interaction(&cells);
        assert!(r.insufficient_data);
    }

    /// (d) Determinism: same input → identical output.
    #[test]
    fn binary_interaction_is_deterministic() {
        let cells = [
            bcell(0, 0, 1000, 100),
            bcell(0, 1, 1000, 100),
            bcell(1, 0, 1000, 100),
            bcell(1, 1, 1000, 400),
        ];
        let a = binary_interaction(&cells);
        let b = binary_interaction(&cells);
        assert_eq!(a, b);
    }

    // ── P5.T1: binary R×C (chi-square path) ─────────────────────────────────

    /// 3×2 with a strong non-additive cell → significant chi-square.
    #[test]
    fn binary_rxc_strong_interaction_is_significant() {
        // Rows 0,1 flat at .10 across both columns; row 2 jumps to .50 only in
        // column 1 — a clear interaction.
        let cells = [
            bcell(0, 0, 1000, 100),
            bcell(0, 1, 1000, 100),
            bcell(1, 0, 1000, 100),
            bcell(1, 1, 1000, 100),
            bcell(2, 0, 1000, 100),
            bcell(2, 1, 1000, 500),
        ];
        let r = binary_interaction(&cells);
        assert!(!r.insufficient_data);
        assert_eq!(r.df, 2); // (3-1)(2-1)
        assert!(r.p_value < 0.05, "p={}", r.p_value);
        assert!(r.significant);
        // estimate is the chi-square statistic in the R×C path.
        assert!((r.estimate - r.statistic).abs() < 1e-12);
    }

    /// 3×2 multiplicative (no-interaction) rates → not significant.
    #[test]
    fn binary_rxc_additive_is_not_significant() {
        // Build cells whose rates follow row_rate * col_rate / grand_rate
        // exactly, so the chi-square interaction term is ~0.
        // Row rates: .10, .20, .30 ; col multiplier baked via construction.
        // Simplest: identical column ratio across rows → no interaction.
        // col1 rate = 1.5 * col0 rate within each row.
        let mk = |a: usize, b: usize, rate: f64| bcell(a, b, 10_000, (rate * 10_000.0) as u64);
        let cells = [
            mk(0, 0, 0.10),
            mk(0, 1, 0.15),
            mk(1, 0, 0.20),
            mk(1, 1, 0.30),
            mk(2, 0, 0.30),
            mk(2, 1, 0.45),
        ];
        let r = binary_interaction(&cells);
        assert!(!r.insufficient_data);
        assert!(r.p_value > 0.05, "p={} chi2={}", r.p_value, r.statistic);
        assert!(!r.significant);
    }

    /// R×C tiny total → insufficient.
    #[test]
    fn binary_rxc_tiny_total_is_insufficient() {
        let cells = [
            bcell(0, 0, 2, 1),
            bcell(0, 1, 2, 1),
            bcell(1, 0, 2, 1),
            bcell(1, 1, 2, 1),
            bcell(2, 0, 1, 0),
            bcell(2, 1, 1, 0),
        ];
        let r = binary_interaction(&cells);
        assert!(r.insufficient_data);
    }

    /// Single column → no interaction term → insufficient.
    #[test]
    fn binary_single_column_is_insufficient() {
        let cells = [bcell(0, 0, 1000, 100), bcell(1, 0, 1000, 200)];
        let r = binary_interaction(&cells);
        assert!(r.insufficient_data);
    }

    #[test]
    fn binary_empty_is_insufficient() {
        let r = binary_interaction(&[]);
        assert!(r.insufficient_data);
    }

    // ── P5.T2: continuous ANOVA F-test ──────────────────────────────────────

    /// Planted continuous interaction: cell means non-additive → significant.
    /// Means: (0,0)=10, (0,1)=10, (1,0)=10, (1,1)=30 — a big interaction.
    #[test]
    fn continuous_strong_interaction_is_significant() {
        let cells = [
            ccell_mean(0, 0, 10.0, 50),
            ccell_mean(0, 1, 10.0, 50),
            ccell_mean(1, 0, 10.0, 50),
            ccell_mean(1, 1, 30.0, 50),
        ];
        let r = continuous_interaction(&cells);
        assert!(!r.insufficient_data);
        assert_eq!(r.df, 1);
        assert!(r.p_value < 0.05, "p={} F={}", r.p_value, r.statistic);
        assert!(r.significant);
    }

    /// Additive cell means → not significant.
    /// Means: (0,0)=10, (0,1)=15, (1,0)=20, (1,1)=25 → row+col additive.
    #[test]
    fn continuous_additive_is_not_significant() {
        let cells = [
            ccell_mean(0, 0, 10.0, 80),
            ccell_mean(0, 1, 15.0, 80),
            ccell_mean(1, 0, 20.0, 80),
            ccell_mean(1, 1, 25.0, 80),
        ];
        let r = continuous_interaction(&cells);
        assert!(!r.insufficient_data);
        assert!(r.p_value > 0.05, "p={} F={}", r.p_value, r.statistic);
        assert!(!r.significant);
    }

    /// Degenerate: df_err <= 0 (one observation per cell) → insufficient.
    #[test]
    fn continuous_no_replication_is_insufficient() {
        let cells = [
            ccell_from_vals(0, 0, &[10.0]),
            ccell_from_vals(0, 1, &[12.0]),
            ccell_from_vals(1, 0, &[14.0]),
            ccell_from_vals(1, 1, &[20.0]),
        ];
        let r = continuous_interaction(&cells);
        assert!(r.insufficient_data);
        assert!(!r.significant);
        assert!(r.statistic.is_nan());
    }

    /// Degenerate: zero within-cell variance → insufficient (no error MS).
    #[test]
    fn continuous_zero_within_variance_is_insufficient() {
        // Every observation in a cell is identical → SS_error = 0.
        let cells = [
            ccell_from_vals(0, 0, &[10.0, 10.0, 10.0]),
            ccell_from_vals(0, 1, &[12.0, 12.0, 12.0]),
            ccell_from_vals(1, 0, &[14.0, 14.0, 14.0]),
            ccell_from_vals(1, 1, &[25.0, 25.0, 25.0]),
        ];
        let r = continuous_interaction(&cells);
        assert!(r.insufficient_data);
    }

    /// Empty cell → unidentifiable additive model → insufficient.
    #[test]
    fn continuous_empty_cell_is_insufficient() {
        let cells = [
            ccell_mean(0, 0, 10.0, 20),
            ccell_mean(0, 1, 12.0, 20),
            ccell_mean(1, 0, 14.0, 20),
            // (1,1) missing
        ];
        let r = continuous_interaction(&cells);
        assert!(r.insufficient_data);
    }

    #[test]
    fn continuous_empty_is_insufficient() {
        let r = continuous_interaction(&[]);
        assert!(r.insufficient_data);
    }

    /// Single row → no interaction term → insufficient.
    #[test]
    fn continuous_single_row_is_insufficient() {
        let cells = [ccell_mean(0, 0, 10.0, 20), ccell_mean(0, 1, 12.0, 20)];
        let r = continuous_interaction(&cells);
        assert!(r.insufficient_data);
    }

    /// Determinism for the continuous path.
    #[test]
    fn continuous_interaction_is_deterministic() {
        let cells = [
            ccell_mean(0, 0, 10.0, 50),
            ccell_mean(0, 1, 10.0, 50),
            ccell_mean(1, 0, 10.0, 50),
            ccell_mean(1, 1, 30.0, 50),
        ];
        let a = continuous_interaction(&cells);
        let b = continuous_interaction(&cells);
        assert_eq!(a, b);
    }
}
