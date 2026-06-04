//! (P2.T6) **Bayesian interaction posteriors** for continuous and ratio metrics
//! — Normal-Normal cell model.
//!
//! ## Contract (signatures fixed by the seam; bodies implemented by the worker)
//!
//! - `continuous_bayes(cells, order)` — Normal-Normal posterior on each cell
//!   mean (variance from the cell's `sum`/`sum_sq`), interaction contrast =
//!   difference-in-differences of cell means (generalised per term).
//! - `ratio_bayes(cells, order)` — same Normal approximation applied to the
//!   per-cell ratio `num_sum/den_sum` with a delta-method posterior variance.
//!
//! Both return a `(TermKind, BayesianInteraction)` for every term that the
//! corresponding Frequentist worker emits; the routing layer joins by
//! [`super::TermKind`] and leaves omitted terms' `bayes` as `None`.
//! `BayesianInteraction`: `prob` = `Φ(|effect| / sd)` (posterior mass away from
//! 0), `expected` = posterior mean of the contrast, `ci_low`/`ci_high` =
//! `expected ± 1.96·sd` (central 95% credible interval). Terms whose contrast
//! standard deviation is 0 / non-finite, or that reference a degenerate /
//! too-sparse collapsed cell, are omitted.
//!
//! ## Model
//!
//! Each cell estimate is given an (approximately) Normal posterior under a weak
//! / flat prior:
//! - **continuous:** `mean = sum/n`, `var = s²/n` with the within-cell sample
//!   variance `s² = (sum_sq − sum²/n)/(n−1)`. Requires `n ≥ 2` and a finite
//!   `s² > 0`.
//! - **ratio:** `R = num_sum/den_sum` with the delta-method variance
//!   `VarR = (1/md²)·(var_num − 2R·cov + R²·var_den)/n`, the moments recovered
//!   from the cell's second-moment sufficient statistics. Requires
//!   `den_sum > 0`, `md > 0`, `n ≥ 2`, and a finite `VarR > 0`.
//!
//! The interaction effect for a term is a **linear contrast** of the per-cell
//! estimates (cell means for continuous, cell ratios for ratio). Because the
//! cells are independent, the contrast posterior is
//! `Normal(Σ coef·est, Σ coef²·var)`. Contrasts match the binary worker:
//! - `Main{i}`: `est(top level) − est(level 0)` of the collapsed factor `i`.
//! - `TwoWay{a,b}`: difference-in-differences on the collapsed `(a, b)` grid; for
//!   grids larger than `2×2`, the mean of the elementary `2×2` interaction
//!   contrasts anchored at level 0.
//! - `ThreeWay{0,1,2}`: difference-in-differences-of-differences over the
//!   `2×2×2` corners anchored at level 0.
//!
//! Collapsing sums the sufficient statistics across the non-participating
//! factors (continuous: `n`/`sum`/`sum_sq`; ratio: all six stats).
//!
//! Determinism: no RNG — every summary is a closed-form Normal quantity, so
//! repeated runs are bit-identical.

use super::bayes_common::{CellPost, enumerate_terms, factor_count, level_dims, run_terms};
use super::{BayesianInteraction, NdContinuousCell, NdRatioCell, TermKind};

/// Normal-Normal posterior for one continuous cell mean, or `None` when the
/// cell is degenerate (too few observations / non-finite or zero variance).
fn continuous_cell_post(n: f64, sum: f64, sum_sq: f64) -> Option<CellPost> {
    // `n` is a sum of integer counts, always finite ⇒ a plain comparison is safe.
    if n < 2.0 {
        return None;
    }
    let mean = sum / n;
    // Within-cell sample variance s² = (Σx² − (Σx)²/n) / (n − 1).
    let s2 = (sum_sq - sum * sum / n) / (n - 1.0);
    if !s2.is_finite() || s2 <= 0.0 {
        return None;
    }
    let var = s2 / n;
    if !var.is_finite() || var <= 0.0 {
        return None;
    }
    Some(CellPost { est: mean, var })
}

/// Delta-method Normal posterior for one ratio cell, or `None` when degenerate.
///
/// Delegates to the shared [`super::super::ratio_delta_var`] (the single source
/// of truth, also used by `interaction::ratio` and
/// `sequential::RatioGroupStats::ratio_var`) so the delta-method variance lives
/// in exactly one place. `n` arrives as an integer-valued `f64` (a sum of cell
/// counts); it is converted losslessly to `i64` for the shared signature.
fn ratio_cell_post(
    n: f64,
    num_sum: f64,
    den_sum: f64,
    num_sq_sum: f64,
    den_sq_sum: f64,
    num_den_sum: f64,
) -> Option<CellPost> {
    let (r, var) = super::super::ratio_delta_var(
        n as i64,
        num_sum,
        den_sum,
        num_sq_sum,
        den_sq_sum,
        num_den_sum,
    )?;
    Some(CellPost { est: r, var })
}

/// See module contract. Normal-Normal interaction posteriors for a continuous
/// metric over a `k`-factor grid.
pub fn continuous_bayes(
    cells: &[NdContinuousCell],
    order: usize,
) -> Vec<(TermKind, BayesianInteraction)> {
    if cells.is_empty() {
        return Vec::new();
    }
    let Some(k) = factor_count(cells.iter().map(|c| c.levels.len())) else {
        return Vec::new();
    };
    let dims = level_dims(cells.iter().map(|c| c.levels.as_slice()), k);
    if dims.contains(&0) {
        return Vec::new();
    }
    let terms = enumerate_terms(order);

    // Collapse: sum (n, sum, sum_sq) across non-participating factors, keyed by
    // the participating-level tuple.
    run_terms(&terms, &dims, |factors, key| {
        let mut n = 0.0f64;
        let mut sum = 0.0f64;
        let mut sum_sq = 0.0f64;
        for cell in cells {
            if factors
                .iter()
                .zip(key)
                .all(|(&f, &want)| cell.levels[f] == want)
            {
                n += cell.n as f64;
                sum += cell.sum;
                sum_sq += cell.sum_sq;
            }
        }
        continuous_cell_post(n, sum, sum_sq)
    })
}

/// See module contract. Normal-Normal interaction posteriors for a ratio metric
/// (delta-method per-cell variance) over a `k`-factor grid.
pub fn ratio_bayes(cells: &[NdRatioCell], order: usize) -> Vec<(TermKind, BayesianInteraction)> {
    if cells.is_empty() {
        return Vec::new();
    }
    let Some(k) = factor_count(cells.iter().map(|c| c.levels.len())) else {
        return Vec::new();
    };
    let dims = level_dims(cells.iter().map(|c| c.levels.as_slice()), k);
    if dims.contains(&0) {
        return Vec::new();
    }
    let terms = enumerate_terms(order);

    // Collapse: sum all six sufficient stats across non-participating factors.
    run_terms(&terms, &dims, |factors, key| {
        let mut n = 0.0f64;
        let mut num_sum = 0.0f64;
        let mut den_sum = 0.0f64;
        let mut num_sq_sum = 0.0f64;
        let mut den_sq_sum = 0.0f64;
        let mut num_den_sum = 0.0f64;
        for cell in cells {
            if factors
                .iter()
                .zip(key)
                .all(|(&f, &want)| cell.levels[f] == want)
            {
                n += cell.n as f64;
                num_sum += cell.num_sum;
                den_sum += cell.den_sum;
                num_sq_sum += cell.num_sq_sum;
                den_sq_sum += cell.den_sq_sum;
                num_den_sum += cell.num_den_sum;
            }
        }
        ratio_cell_post(n, num_sum, den_sum, num_sq_sum, den_sq_sum, num_den_sum)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Builders ────────────────────────────────────────────────────────────

    /// Build a continuous cell from a list of observed values.
    fn cont_cell(levels: &[usize], values: &[f64]) -> NdContinuousCell {
        let n = values.len() as u64;
        let sum = values.iter().sum();
        let sum_sq = values.iter().map(|v| v * v).sum();
        NdContinuousCell {
            levels: levels.to_vec(),
            n,
            sum,
            sum_sq,
        }
    }

    /// Build a ratio cell from a list of `(numerator, denominator)` pairs.
    fn ratio_cell(levels: &[usize], pairs: &[(f64, f64)]) -> NdRatioCell {
        let n = pairs.len() as u64;
        let mut num_sum = 0.0;
        let mut den_sum = 0.0;
        let mut num_sq_sum = 0.0;
        let mut den_sq_sum = 0.0;
        let mut num_den_sum = 0.0;
        for &(num, den) in pairs {
            num_sum += num;
            den_sum += den;
            num_sq_sum += num * num;
            den_sq_sum += den * den;
            num_den_sum += num * den;
        }
        NdRatioCell {
            levels: levels.to_vec(),
            n,
            num_sum,
            den_sum,
            num_sq_sum,
            den_sq_sum,
            num_den_sum,
        }
    }

    /// Deterministic pseudo-noise in roughly [-amp, amp] from a small LCG, so
    /// cells carry realistic within-cell variance without an RNG dependency.
    fn jitter(seed: u64, amp: f64) -> impl Iterator<Item = f64> {
        let mut state = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        std::iter::repeat_with(move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let u = (state >> 11) as f64 / (1u64 << 53) as f64; // in [0,1)
            (u - 0.5) * 2.0 * amp
        })
    }

    /// Continuous values: `count` points centred on `mean` with bounded jitter.
    fn cont_values(seed: u64, count: usize, mean: f64, amp: f64) -> Vec<f64> {
        jitter(seed, amp).take(count).map(|e| mean + e).collect()
    }

    fn find(
        out: &[(TermKind, BayesianInteraction)],
        kind: TermKind,
    ) -> Option<&BayesianInteraction> {
        out.iter().find(|(k, _)| *k == kind).map(|(_, b)| b)
    }

    // ── continuous_bayes ─────────────────────────────────────────────────────

    #[test]
    fn continuous_planted_interaction_high_prob() {
        // 2×2 means with a strong difference-in-differences:
        //   (m11 − m10) − (m01 − m00) = (20 − 10) − (10 − 10) = 10.
        let cells = vec![
            cont_cell(&[0, 0], &cont_values(1, 60, 10.0, 1.0)),
            cont_cell(&[0, 1], &cont_values(2, 60, 10.0, 1.0)),
            cont_cell(&[1, 0], &cont_values(3, 60, 10.0, 1.0)),
            cont_cell(&[1, 1], &cont_values(4, 60, 20.0, 1.0)),
        ];
        let out = continuous_bayes(&cells, 2);
        let tw = find(&out, TermKind::TwoWay { a: 0, b: 1 }).expect("two-way present");
        assert!(tw.prob > 0.95, "prob={}", tw.prob);
        assert!((tw.expected - 10.0).abs() < 0.5, "expected={}", tw.expected);
        // CI excludes 0.
        assert!(
            tw.ci_low > 0.0 && tw.ci_high > 0.0,
            "ci=[{},{}]",
            tw.ci_low,
            tw.ci_high
        );
    }

    #[test]
    fn continuous_additive_means_prob_half() {
        // Additive: m = base + row_eff + col_eff, no interaction.
        //   m00=10, m01=14, m10=13, m11=17 ⇒ DiD = (17−13)−(14−10) = 0.
        let cells = vec![
            cont_cell(&[0, 0], &cont_values(11, 80, 10.0, 1.0)),
            cont_cell(&[0, 1], &cont_values(12, 80, 14.0, 1.0)),
            cont_cell(&[1, 0], &cont_values(13, 80, 13.0, 1.0)),
            cont_cell(&[1, 1], &cont_values(14, 80, 17.0, 1.0)),
        ];
        let out = continuous_bayes(&cells, 2);
        let tw = find(&out, TermKind::TwoWay { a: 0, b: 1 }).expect("two-way present");
        assert!((tw.expected).abs() < 0.5, "expected={}", tw.expected);
        assert!((tw.prob - 0.5).abs() < 0.2, "prob={}", tw.prob);
        // CI includes 0.
        assert!(
            tw.ci_low < 0.0 && tw.ci_high > 0.0,
            "ci=[{},{}]",
            tw.ci_low,
            tw.ci_high
        );
    }

    #[test]
    fn continuous_main_effects_returned() {
        let cells = vec![
            cont_cell(&[0, 0], &cont_values(21, 60, 10.0, 1.0)),
            cont_cell(&[0, 1], &cont_values(22, 60, 12.0, 1.0)),
            cont_cell(&[1, 0], &cont_values(23, 60, 18.0, 1.0)),
            cont_cell(&[1, 1], &cont_values(24, 60, 20.0, 1.0)),
        ];
        let out = continuous_bayes(&cells, 2);
        // Main{0}: collapse over factor 1 → top-vs-0 ≈ ((18+20)/2 − (10+12)/2) = 8.
        let m0 = find(&out, TermKind::Main { factor: 0 }).expect("main 0 present");
        assert!(
            (m0.expected - 8.0).abs() < 0.6,
            "main0 expected={}",
            m0.expected
        );
        assert!(m0.prob > 0.95, "main0 prob={}", m0.prob);
        // Main{1}: ((12+20)/2 − (10+18)/2) = 2.
        let m1 = find(&out, TermKind::Main { factor: 1 }).expect("main 1 present");
        assert!(
            (m1.expected - 2.0).abs() < 0.6,
            "main1 expected={}",
            m1.expected
        );
    }

    #[test]
    fn continuous_three_way_returned() {
        // 2×2×2 grid; ThreeWay must be present at order 3, and Main/TwoWay too.
        let mut cells = Vec::new();
        let mut seed = 100u64;
        for a in 0..2 {
            for b in 0..2 {
                for c in 0..2 {
                    // Plant a 3-way: bump the (1,1,1) corner.
                    let mean = 10.0 + if (a, b, c) == (1, 1, 1) { 8.0 } else { 0.0 };
                    cells.push(cont_cell(&[a, b, c], &cont_values(seed, 50, mean, 1.0)));
                    seed += 1;
                }
            }
        }
        let out = continuous_bayes(&cells, 3);
        let tw = find(&out, TermKind::ThreeWay { a: 0, b: 1, c: 2 }).expect("three-way present");
        // DiDiD over corners with only (1,1,1) bumped = +8.
        assert!(
            (tw.expected - 8.0).abs() < 0.6,
            "3way expected={}",
            tw.expected
        );
        assert!(tw.prob > 0.95, "3way prob={}", tw.prob);
        // All three mains + all three pairwise present.
        assert!(find(&out, TermKind::Main { factor: 2 }).is_some());
        assert!(find(&out, TermKind::TwoWay { a: 1, b: 2 }).is_some());
    }

    #[test]
    fn continuous_order_two_omits_three_way() {
        let mut cells = Vec::new();
        let mut seed = 200u64;
        for a in 0..2 {
            for b in 0..2 {
                for c in 0..2 {
                    cells.push(cont_cell(
                        &[a, b, c],
                        &cont_values(seed, 50, 10.0 + a as f64, 1.0),
                    ));
                    seed += 1;
                }
            }
        }
        let out = continuous_bayes(&cells, 2);
        assert!(find(&out, TermKind::ThreeWay { a: 0, b: 1, c: 2 }).is_none());
        assert!(find(&out, TermKind::TwoWay { a: 0, b: 1 }).is_some());
    }

    /// (#10) Term enumeration is gated by the requested `order`, NOT the data's
    /// factor count `k`: at `order == 2`, a grid whose cells happen to carry 3
    /// levels must still emit ONLY `Main{0}`, `Main{1}`, `TwoWay{0,1}` — no
    /// three-way, no `Main{2}`, no `TwoWay` touching factor 2 (matching the
    /// Frequentist workers and `bayes_binary`).
    #[test]
    fn continuous_order_gates_terms_by_order_not_data_factor_count() {
        // k = 3 in the data, but we ask for order 2.
        let mut cells = Vec::new();
        let mut seed = 900u64;
        for a in 0..2 {
            for b in 0..2 {
                for c in 0..2 {
                    let mean = 10.0 + a as f64 + 2.0 * b as f64 + 3.0 * c as f64;
                    cells.push(cont_cell(&[a, b, c], &cont_values(seed, 60, mean, 1.0)));
                    seed += 1;
                }
            }
        }
        let out = continuous_bayes(&cells, 2);

        // Exactly the order-2 term set, regardless of the data carrying 3 factors.
        assert!(find(&out, TermKind::Main { factor: 0 }).is_some());
        assert!(find(&out, TermKind::Main { factor: 1 }).is_some());
        assert!(find(&out, TermKind::TwoWay { a: 0, b: 1 }).is_some());
        // Nothing involving factor 2, and no three-way.
        assert!(find(&out, TermKind::Main { factor: 2 }).is_none());
        assert!(find(&out, TermKind::TwoWay { a: 0, b: 2 }).is_none());
        assert!(find(&out, TermKind::TwoWay { a: 1, b: 2 }).is_none());
        assert!(find(&out, TermKind::ThreeWay { a: 0, b: 1, c: 2 }).is_none());
        // No emitted term references a factor index >= order.
        assert!(out.iter().all(|(k, _)| match *k {
            TermKind::Main { factor } => factor < 2,
            TermKind::TwoWay { a, b } => a < 2 && b < 2,
            TermKind::ThreeWay { .. } => false,
        }));
    }

    #[test]
    fn continuous_larger_grid_two_way_averaged() {
        // 3×2 grid: the averaged elementary-2×2 contrast over i∈{1,2}, j∈{1}.
        // m[0][0]=10 m[0][1]=10 ; m[1][0]=10 m[1][1]=16 ; m[2][0]=10 m[2][1]=14
        // contrast_i1 = (16−10)−(10−10)=6 ; contrast_i2 = (14−10)−(10−10)=4
        // average = 5.
        let cells = vec![
            cont_cell(&[0, 0], &cont_values(31, 60, 10.0, 1.0)),
            cont_cell(&[0, 1], &cont_values(32, 60, 10.0, 1.0)),
            cont_cell(&[1, 0], &cont_values(33, 60, 10.0, 1.0)),
            cont_cell(&[1, 1], &cont_values(34, 60, 16.0, 1.0)),
            cont_cell(&[2, 0], &cont_values(35, 60, 10.0, 1.0)),
            cont_cell(&[2, 1], &cont_values(36, 60, 14.0, 1.0)),
        ];
        let out = continuous_bayes(&cells, 2);
        let tw = find(&out, TermKind::TwoWay { a: 0, b: 1 }).expect("two-way present");
        assert!((tw.expected - 5.0).abs() < 0.6, "expected={}", tw.expected);
    }

    #[test]
    fn continuous_degenerate_cell_omits_term() {
        // Cell (1,1) has n=1 (no within-cell variance). The 2×2 TwoWay uses that
        // cell uncollapsed ⇒ omitted. The mains collapse over the other factor,
        // pooling (1,1) with a well-populated cell, so they stay well-defined.
        let cells = vec![
            cont_cell(&[0, 0], &cont_values(41, 60, 10.0, 1.0)),
            cont_cell(&[0, 1], &cont_values(42, 60, 12.0, 1.0)),
            cont_cell(&[1, 0], &cont_values(43, 60, 14.0, 1.0)),
            cont_cell(&[1, 1], &[20.0]), // n = 1 ⇒ degenerate as a standalone cell.
        ];
        let out = continuous_bayes(&cells, 2);
        assert!(find(&out, TermKind::TwoWay { a: 0, b: 1 }).is_none());
        // Main{0} collapses factor 1 (cells (1,0)+(1,1) pooled) ⇒ defined.
        assert!(find(&out, TermKind::Main { factor: 0 }).is_some());
    }

    #[test]
    fn continuous_degenerate_uncollapsed_main_omitted() {
        // A degenerate cell that the main effect cannot pool away (single level
        // on the *other* factor) ⇒ the main on factor 0 is omitted.
        let cells = vec![
            cont_cell(&[0, 0], &cont_values(71, 60, 10.0, 1.0)),
            cont_cell(&[1, 0], &[20.0]), // n=1; only level 0 on factor 1 ⇒ no pooling.
        ];
        let out = continuous_bayes(&cells, 2);
        assert!(find(&out, TermKind::Main { factor: 0 }).is_none());
    }

    #[test]
    fn continuous_empty_and_single_level_handled() {
        assert!(continuous_bayes(&[], 2).is_empty());
        // Single level on factor 1 ⇒ no two-way / no Main{1}, but Main{0} ok.
        let cells = vec![
            cont_cell(&[0, 0], &cont_values(51, 60, 10.0, 1.0)),
            cont_cell(&[1, 0], &cont_values(52, 60, 16.0, 1.0)),
        ];
        let out = continuous_bayes(&cells, 2);
        assert!(find(&out, TermKind::Main { factor: 0 }).is_some());
        assert!(find(&out, TermKind::Main { factor: 1 }).is_none());
        assert!(find(&out, TermKind::TwoWay { a: 0, b: 1 }).is_none());
    }

    #[test]
    fn continuous_deterministic() {
        let cells = vec![
            cont_cell(&[0, 0], &cont_values(61, 60, 10.0, 1.0)),
            cont_cell(&[0, 1], &cont_values(62, 60, 10.0, 1.0)),
            cont_cell(&[1, 0], &cont_values(63, 60, 10.0, 1.0)),
            cont_cell(&[1, 1], &cont_values(64, 60, 20.0, 1.0)),
        ];
        let a = continuous_bayes(&cells, 2);
        let b = continuous_bayes(&cells, 2);
        assert_eq!(a.len(), b.len());
        for ((ka, va), (kb, vb)) in a.iter().zip(b.iter()) {
            assert_eq!(ka, kb);
            // Bit-identical: closed-form, no RNG.
            assert_eq!(va.expected.to_bits(), vb.expected.to_bits());
            assert_eq!(va.prob.to_bits(), vb.prob.to_bits());
            assert_eq!(va.ci_low.to_bits(), vb.ci_low.to_bits());
            assert_eq!(va.ci_high.to_bits(), vb.ci_high.to_bits());
        }
    }

    // ── ratio_bayes ──────────────────────────────────────────────────────────

    /// Build a ratio cell of `count` units, each `(rate + jitter, 1.0)` so the
    /// cell ratio ≈ `rate` with realistic (non-zero, finite) delta variance.
    fn ratio_at(seed: u64, count: usize, rate: f64, amp: f64) -> Vec<(f64, f64)> {
        jitter(seed, amp)
            .take(count)
            .map(|e| (rate + e, 1.0))
            .collect()
    }

    #[test]
    fn ratio_planted_interaction_high_prob() {
        // Cell ratios (num/den with den≈1): DiD = (0.6−0.3)−(0.3−0.3)=0.3.
        let cells = vec![
            ratio_cell(&[0, 0], &ratio_at(1, 120, 0.30, 0.05)),
            ratio_cell(&[0, 1], &ratio_at(2, 120, 0.30, 0.05)),
            ratio_cell(&[1, 0], &ratio_at(3, 120, 0.30, 0.05)),
            ratio_cell(&[1, 1], &ratio_at(4, 120, 0.60, 0.05)),
        ];
        let out = ratio_bayes(&cells, 2);
        let tw = find(&out, TermKind::TwoWay { a: 0, b: 1 }).expect("two-way present");
        assert!(tw.prob > 0.95, "prob={}", tw.prob);
        assert!(
            (tw.expected - 0.30).abs() < 0.03,
            "expected={}",
            tw.expected
        );
        assert!(tw.ci_low > 0.0, "ci_low={}", tw.ci_low);
    }

    #[test]
    fn ratio_additive_prob_half() {
        // No interaction in ratios: r00=0.30 r01=0.40 r10=0.50 r11=0.60.
        //   DiD = (0.60−0.50)−(0.40−0.30) = 0.
        let cells = vec![
            ratio_cell(&[0, 0], &ratio_at(11, 150, 0.30, 0.05)),
            ratio_cell(&[0, 1], &ratio_at(12, 150, 0.40, 0.05)),
            ratio_cell(&[1, 0], &ratio_at(13, 150, 0.50, 0.05)),
            ratio_cell(&[1, 1], &ratio_at(14, 150, 0.60, 0.05)),
        ];
        let out = ratio_bayes(&cells, 2);
        let tw = find(&out, TermKind::TwoWay { a: 0, b: 1 }).expect("two-way present");
        assert!(tw.expected.abs() < 0.03, "expected={}", tw.expected);
        assert!((tw.prob - 0.5).abs() < 0.2, "prob={}", tw.prob);
        assert!(
            tw.ci_low < 0.0 && tw.ci_high > 0.0,
            "ci=[{},{}]",
            tw.ci_low,
            tw.ci_high
        );
    }

    #[test]
    fn ratio_main_effects_returned() {
        let cells = vec![
            ratio_cell(&[0, 0], &ratio_at(21, 120, 0.30, 0.05)),
            ratio_cell(&[0, 1], &ratio_at(22, 120, 0.32, 0.05)),
            ratio_cell(&[1, 0], &ratio_at(23, 120, 0.50, 0.05)),
            ratio_cell(&[1, 1], &ratio_at(24, 120, 0.52, 0.05)),
        ];
        let out = ratio_bayes(&cells, 2);
        let m0 = find(&out, TermKind::Main { factor: 0 }).expect("main 0 present");
        // ((0.50+0.52)/2 − (0.30+0.32)/2) = 0.20.
        assert!(
            (m0.expected - 0.20).abs() < 0.03,
            "main0 expected={}",
            m0.expected
        );
        assert!(m0.prob > 0.95, "main0 prob={}", m0.prob);
    }

    #[test]
    fn ratio_three_way_returned() {
        let mut cells = Vec::new();
        let mut seed = 300u64;
        for a in 0..2 {
            for b in 0..2 {
                for c in 0..2 {
                    let rate = 0.30 + if (a, b, c) == (1, 1, 1) { 0.30 } else { 0.0 };
                    cells.push(ratio_cell(&[a, b, c], &ratio_at(seed, 120, rate, 0.05)));
                    seed += 1;
                }
            }
        }
        let out = ratio_bayes(&cells, 3);
        let tw = find(&out, TermKind::ThreeWay { a: 0, b: 1, c: 2 }).expect("three-way present");
        assert!(
            (tw.expected - 0.30).abs() < 0.04,
            "3way expected={}",
            tw.expected
        );
        assert!(tw.prob > 0.9, "3way prob={}", tw.prob);
        assert!(find(&out, TermKind::Main { factor: 0 }).is_some());
    }

    /// (#10) Ratio enumeration is gated by `order`, not the data's factor count:
    /// at `order == 2` over a 3-factor grid, no term may touch factor 2.
    #[test]
    fn ratio_order_gates_terms_by_order_not_data_factor_count() {
        let mut cells = Vec::new();
        let mut seed = 950u64;
        for a in 0..2 {
            for b in 0..2 {
                for c in 0..2 {
                    let rate = 0.30 + 0.05 * a as f64 + 0.04 * b as f64 + 0.03 * c as f64;
                    cells.push(ratio_cell(&[a, b, c], &ratio_at(seed, 120, rate, 0.05)));
                    seed += 1;
                }
            }
        }
        let out = ratio_bayes(&cells, 2);
        assert!(find(&out, TermKind::TwoWay { a: 0, b: 1 }).is_some());
        assert!(find(&out, TermKind::Main { factor: 2 }).is_none());
        assert!(find(&out, TermKind::TwoWay { a: 0, b: 2 }).is_none());
        assert!(find(&out, TermKind::TwoWay { a: 1, b: 2 }).is_none());
        assert!(find(&out, TermKind::ThreeWay { a: 0, b: 1, c: 2 }).is_none());
    }

    #[test]
    fn ratio_degenerate_cell_omitted() {
        // Zero-denominator cell ⇒ ratio undefined ⇒ TwoWay omitted.
        let cells = vec![
            ratio_cell(&[0, 0], &ratio_at(41, 120, 0.30, 0.05)),
            ratio_cell(&[0, 1], &ratio_at(42, 120, 0.30, 0.05)),
            ratio_cell(&[1, 0], &ratio_at(43, 120, 0.30, 0.05)),
            ratio_cell(&[1, 1], &[(0.0, 0.0); 120]), // den_sum = 0 ⇒ degenerate.
        ];
        let out = ratio_bayes(&cells, 2);
        assert!(find(&out, TermKind::TwoWay { a: 0, b: 1 }).is_none());
    }

    #[test]
    fn ratio_empty_handled() {
        assert!(ratio_bayes(&[], 2).is_empty());
    }

    #[test]
    fn ratio_deterministic() {
        let cells = vec![
            ratio_cell(&[0, 0], &ratio_at(51, 120, 0.30, 0.05)),
            ratio_cell(&[0, 1], &ratio_at(52, 120, 0.30, 0.05)),
            ratio_cell(&[1, 0], &ratio_at(53, 120, 0.30, 0.05)),
            ratio_cell(&[1, 1], &ratio_at(54, 120, 0.60, 0.05)),
        ];
        let a = ratio_bayes(&cells, 2);
        let b = ratio_bayes(&cells, 2);
        assert_eq!(a.len(), b.len());
        for ((ka, va), (kb, vb)) in a.iter().zip(b.iter()) {
            assert_eq!(ka, kb);
            assert_eq!(va.expected.to_bits(), vb.expected.to_bits());
            assert_eq!(va.prob.to_bits(), vb.prob.to_bits());
            assert_eq!(va.ci_low.to_bits(), vb.ci_low.to_bits());
            assert_eq!(va.ci_high.to_bits(), vb.ci_high.to_bits());
        }
    }
}
