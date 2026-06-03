//! Shared driver for the **Bayesian interaction posteriors** of every metric
//! family (binary Beta-Binomial, continuous Normal-Normal, ratio delta-method).
//!
//! All three workers ([`super::bayes_binary::binary_bayes`],
//! [`super::bayes_continuous::continuous_bayes`] /
//! [`super::bayes_continuous::ratio_bayes`]) share structurally identical
//! machinery and differ ONLY in the per-cell point estimate + posterior variance.
//! This module factors out the common steps so each worker provides just its
//! per-cell posterior (a collapse over the participating factors yielding a
//! [`CellPost`]); enumeration, contrast construction, and the Normal summary are
//! all driven here.
//!
//! ## Steps (shared, metric-agnostic)
//!
//! 1. **Enumerate** exactly the terms for the requested `order`
//!    ([`enumerate_terms`]): `Main{0..order}`, every `TwoWay{a<b}` in `0..order`,
//!    and — when `order >= 3` — the `ThreeWay{0,1,2}`.
//! 2. **Build the linear-contrast coefficients** per term
//!    ([`contrast_coeffs`]): `Main = top − 0`; `TwoWay = ` mean of the
//!    `(Lₐ−1)(L_b−1)` anchored elementary 2×2 difference-in-differences; `ThreeWay`
//!    = the 2×2×2 difference-in-differences-of-differences.
//! 3. **Collapse** cells onto the participating factors (worker-supplied closure).
//! 4. **Form the contrast posterior** `Normal(Σ coef·estᵢ, Σ coef²·varᵢ)` over the
//!    distinct participating cells ([`run_terms`]).
//! 5. **Summarise** that Normal ([`summarise`]):
//!    `prob = Φ(|mean|/sd)`, `expected = mean`, `ci = mean ± 1.96·sd`; a term is
//!    omitted when a participating factor has < 2 levels, a participating cell is
//!    degenerate / absent, or the contrast sd is 0 / non-finite.
//!
//! Determinism: there is no RNG and every accumulation iterates the contrast's
//! `BTreeMap` in a fixed key order, so repeated calls are bit-identical.

use std::collections::BTreeMap;

use super::{BayesianInteraction, TermKind};

/// Two-sided 95 % normal quantile (z₀.₉₇₅) for the central credible interval.
pub(super) const Z_95: f64 = 1.96;

/// A per-cell point estimate and its posterior variance, as produced by a
/// worker's collapse closure.
#[derive(Clone, Copy)]
pub(super) struct CellPost {
    pub est: f64,
    pub var: f64,
}

/// Enumerate exactly the terms for the requested `order`, independent of the
/// data's factor count: `Main{0..order}`, every `TwoWay{a<b}` in `0..order`, and
/// — when `order >= 3` — the `ThreeWay{0,1,2}`. This matches the Frequentist
/// workers ([`super::anova::continuous_terms`] / [`super::ratio::ratio_terms`] /
/// [`super::loglinear::binary_terms`]) so the routing layer joins a consistent
/// term set across inference models.
///
/// (Terms whose factor index exceeds what the data carries are still dropped
/// downstream: [`contrast_coeffs`] returns `None` for an absent factor, so they
/// never reach the cell collapse.)
pub(super) fn enumerate_terms(order: usize) -> Vec<TermKind> {
    let mut terms = Vec::new();
    for f in 0..order {
        terms.push(TermKind::Main { factor: f });
    }
    for a in 0..order {
        for b in (a + 1)..order {
            terms.push(TermKind::TwoWay { a, b });
        }
    }
    if order >= 3 {
        terms.push(TermKind::ThreeWay { a: 0, b: 1, c: 2 });
    }
    terms
}

/// The factor indices a term participates in (in tuple order).
pub(super) fn term_factors(kind: &TermKind) -> Vec<usize> {
    match *kind {
        TermKind::Main { factor } => vec![factor],
        TermKind::TwoWay { a, b } => vec![a, b],
        TermKind::ThreeWay { a, b, c } => vec![a, b, c],
    }
}

/// Number of factors (interaction order of the *data*) from the cell level
/// tuple lengths. Returns `None` for an empty / ragged grid.
pub(super) fn factor_count(level_lens: impl Iterator<Item = usize>) -> Option<usize> {
    let mut k = None;
    for len in level_lens {
        match k {
            None => k = Some(len),
            Some(prev) if prev == len => {}
            Some(_) => return None, // ragged level tuples — cannot decompose.
        }
    }
    k.filter(|&k| k >= 1)
}

/// Maximum level index (+1 ⇒ number of levels) present on each factor.
pub(super) fn level_dims<'a>(
    level_tuples: impl Iterator<Item = &'a [usize]>,
    k: usize,
) -> Vec<usize> {
    let mut dims = vec![0usize; k];
    for levels in level_tuples {
        for (d, &lv) in levels.iter().enumerate() {
            if lv + 1 > dims[d] {
                dims[d] = lv + 1;
            }
        }
    }
    dims
}

/// Build the contrast coefficient map (`participating-level-tuple → coefficient`)
/// for a term, given the per-factor level counts `dims`.
///
/// - **`Main { factor }`** — `est(top) − est(0)` of the collapsed factor;
///   coefficients `{ +1 @ [top], −1 @ [0] }`.
/// - **`TwoWay { a, b }`** — mean of the `(Lₐ−1)(L_b−1)` elementary 2×2
///   interaction contrasts anchored at level 0:
///   `(e[i][j] − e[i][0]) − (e[0][j] − e[0][0])`. The averaged coefficients are
///   accumulated per cell so the independent-cell variance stays correct.
/// - **`ThreeWay { a, b, c }`** — difference-in-differences-of-differences over
///   the 2×2×2 corners (each factor at level 0 or its top); a corner's sign is
///   `(−1)^(#coords at level 0)`, so the all-top corner is `+1`.
///
/// Returns `None` when a participating factor has fewer than two levels (no
/// contrast is defined).
pub(super) fn contrast_coeffs(
    kind: &TermKind,
    dims: &[usize],
) -> Option<BTreeMap<Vec<usize>, f64>> {
    let mut coeffs: BTreeMap<Vec<usize>, f64> = BTreeMap::new();
    let mut add = |key: Vec<usize>, c: f64| {
        *coeffs.entry(key).or_insert(0.0) += c;
    };

    match *kind {
        TermKind::Main { factor } => {
            let levels = *dims.get(factor)?;
            if levels < 2 {
                return None;
            }
            let top = levels - 1;
            add(vec![top], 1.0);
            add(vec![0], -1.0);
        }
        TermKind::TwoWay { a, b } => {
            let na = *dims.get(a)?;
            let nb = *dims.get(b)?;
            if na < 2 || nb < 2 {
                return None;
            }
            // Mean of the elementary 2×2 interaction contrasts anchored at 0:
            //   (e[i][j] − e[i][0]) − (e[0][j] − e[0][0]).
            let pairs = ((na - 1) * (nb - 1)) as f64;
            let w = 1.0 / pairs;
            for i in 1..na {
                for j in 1..nb {
                    add(vec![i, j], w);
                    add(vec![i, 0], -w);
                    add(vec![0, j], -w);
                    add(vec![0, 0], w);
                }
            }
        }
        TermKind::ThreeWay { a, b, c } => {
            let na = *dims.get(a)?;
            let nb = *dims.get(b)?;
            let nc = *dims.get(c)?;
            if na < 2 || nb < 2 || nc < 2 {
                return None;
            }
            // Difference-in-differences-of-differences over the 2×2×2 corners
            // anchored at level 0 on each factor:
            //   [(e111−e110)−(e101−e100)] − [(e011−e010)−(e001−e000)].
            // Expanding, a corner's sign is (−1)^(number of coords at level 0),
            // i.e. the all-"top" corner is +1.
            let top = [na - 1, nb - 1, nc - 1];
            for (ia, &la) in [0, top[0]].iter().enumerate() {
                for (ib, &lb) in [0, top[1]].iter().enumerate() {
                    for (ic, &lc) in [0, top[2]].iter().enumerate() {
                        let zeros = (1 - ia) + (1 - ib) + (1 - ic); // coords at level 0.
                        let sign = if zeros % 2 == 0 { 1.0 } else { -1.0 };
                        add(vec![la, lb, lc], sign);
                    }
                }
            }
        }
    }

    Some(coeffs)
}

/// Summarise a linear-contrast posterior `Normal(mean, var)` into a
/// [`BayesianInteraction`], or `None` when the standard deviation is zero /
/// non-finite or the mean is non-finite.
pub(super) fn summarise(mean: f64, var: f64) -> Option<BayesianInteraction> {
    if !var.is_finite() || var <= 0.0 {
        return None;
    }
    let sd = var.sqrt();
    if !sd.is_finite() || sd <= 0.0 {
        return None;
    }
    if !mean.is_finite() {
        return None;
    }
    // Posterior probability the effect is non-null in its estimated direction:
    // P(|effect| away from 0) under the Normal posterior = Φ(|mean| / sd).
    let prob = super::norm_cdf(mean.abs() / sd);
    Some(BayesianInteraction {
        prob,
        expected: mean,
        ci_low: mean - Z_95 * sd,
        ci_high: mean + Z_95 * sd,
    })
}

/// Shared driver: for each term, build its contrast, collapse the grid onto the
/// participating factors via `collapse_post`, form the linear contrast
/// posterior `Normal(Σ coef·est, Σ coef²·var)`, and summarise it.
///
/// `collapse_post(factors, key)` maps a participating-level key (aligned with
/// `factors`) to a [`CellPost`], or `None` if that collapsed cell is degenerate
/// / absent (which omits the whole term). The contrast's distinct cells are
/// iterated in `BTreeMap` key order; each contributes once with its net
/// accumulated coefficient, keeping the independent-cell variance correct even
/// when a cell recurs across the elementary contrasts that were averaged.
pub(super) fn run_terms<F>(
    terms: &[TermKind],
    dims_full: &[usize],
    mut collapse_post: F,
) -> Vec<(TermKind, BayesianInteraction)>
where
    F: FnMut(&[usize], &[usize]) -> Option<CellPost>,
{
    let mut out = Vec::new();
    for kind in terms {
        let factors = term_factors(kind);
        let Some(coeffs) = contrast_coeffs(kind, dims_full) else {
            continue; // factor with < 2 levels ⇒ no contrast.
        };

        let mut mean = 0.0f64;
        let mut var = 0.0f64;
        let mut ok = true;
        for (key, &coef) in &coeffs {
            // A net-zero coefficient cell does not participate.
            if coef == 0.0 {
                continue;
            }
            match collapse_post(&factors, key) {
                Some(post) => {
                    mean += coef * post.est;
                    var += coef * coef * post.var;
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue; // degenerate / missing collapsed cell ⇒ omit the term.
        }
        if let Some(bi) = summarise(mean, var) {
            out.push((*kind, bi));
        }
    }
    out
}
