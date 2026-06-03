//! (P2.T5) **Bayesian interaction posteriors** for binary (conversion / funnel)
//! metrics — Beta-Binomial cell model.
//!
//! ## Contract (signature fixed by the seam; body implemented by the worker)
//!
//! [`binary_bayes`]`(cells, order)` returns a `(TermKind, BayesianInteraction)`
//! for every term that [`super::loglinear::binary_terms`] produces — the
//! routing layer joins them by [`super::TermKind`]. Terms too sparse for a
//! meaningful posterior are omitted (the routing layer leaves their `bayes` as
//! `None`).
//!
//! ## Model (deterministic — closed-form Normal approximation, NO RNG)
//!
//! Each cell rate gets an independent **Beta(α, β)** posterior under a Beta(1,1)
//! (uniform) prior: `α = successes + 1`, `β = (n − successes) + 1`. Its posterior
//! mean is `m = α/(α+β)` and variance `v = αβ / ((α+β)²(α+β+1))`.
//!
//! Every interaction effect is a **linear contrast** of cell rates (a
//! difference-in-differences, generalised per term). Because the cells are
//! independent, the contrast posterior is approximately Normal with
//! `mean = Σ coefᵢ·mᵢ` and `variance = Σ coefᵢ²·vᵢ` over the participating cells.
//! We summarise that Normal:
//!
//! - `expected` = contrast mean,
//! - `ci_low` / `ci_high` = `expected ± 1.96·sd` (central 95 % credible interval),
//! - `prob` = posterior probability the effect is non-null *in its estimated
//!   direction* = `Φ(|expected|/sd)` via [`super::norm_cdf`]. A clear interaction
//!   drives `prob → 1`; pure noise → `prob ≈ 0.5`.
//!
//! ### Contrast definitions per term
//!
//! Cells are first **collapsed** (summed) over the factors a term does not
//! involve, yielding pooled `(n, successes)` per participating-level
//! combination. A pooled group's Beta posterior uses its summed counts.
//!
//! - **`Main { factor: i }`** — a representative main-effect contrast: the rate
//!   at factor `i`'s *top* level minus the rate at its level 0, collapsing over
//!   all other factors. Coefficients `{ +1 @ top, −1 @ 0 }`.
//! - **`TwoWay { a, b }`** — collapse to an `Lₐ × L_b` table. For 2×2 this is the
//!   difference-in-differences `(p₁₁−p₁₀)−(p₀₁−p₀₀)`, coeffs `{+1 @ (1,1),(0,0);
//!   −1 @ (1,0),(0,1)}`. For larger tables we average the `(Lₐ−1)(L_b−1)`
//!   elementary 2×2 interaction contrasts anchored at level 0 — the averaged
//!   coefficients are accumulated per pooled cell so the variance stays correct.
//! - **`ThreeWay { 0, 1, 2 }`** — the difference-in-differences-of-differences
//!   over the 2×2×2 corners (each factor at level 0 or its top level; factors
//!   with >2 levels are anchored at level 0). The corner coefficient is the
//!   product of the per-factor difference signs, i.e. `(−1)^(#factors at 0)`.
//!
//! A term is **omitted** when any participating pooled cell is empty (`n == 0`)
//! or when the contrast sd is zero / non-finite.
//!
//! Determinism: there is no RNG and no iteration-order-dependent float
//! accumulation, so repeated calls are bit-identical.

use super::{BayesianInteraction, NdBinaryCell, TermKind};
use std::collections::BTreeMap;

/// Two-sided 95 % normal quantile (z₀.₉₇₅) for the central credible interval.
const Z_95: f64 = 1.96;

/// Posterior mean and variance of a single cell rate under a Beta(1,1) prior.
///
/// `α = successes + 1`, `β = failures + 1`. Both are ≥ 1, so the variance is
/// always strictly positive and finite (even for an unobserved pooled cell).
/// Failures use a saturating subtraction so a malformed `successes > n` cell can
/// never underflow `u64`.
#[inline]
fn beta_posterior(n: u64, successes: u64) -> (f64, f64) {
    let successes = successes.min(n);
    let s = successes as f64;
    let f = (n - successes) as f64;
    let alpha = s + 1.0;
    let beta = f + 1.0;
    let ab = alpha + beta;
    let mean = alpha / ab;
    let var = (alpha * beta) / (ab * ab * (ab + 1.0));
    (mean, var)
}

/// A cell selector: per factor, `Some(level)` fixes that factor at a level while
/// `None` collapses (sums) over every level of that factor.
type Selector = Vec<Option<usize>>;

/// Pool `(n, successes)` over every cell matching `sel` (the collapsed group).
///
/// Returns `(total_n, total_successes)`; `total_n == 0` means the participating
/// group is empty.
fn pool(cells: &[NdBinaryCell], sel: &Selector) -> (u64, u64) {
    let mut n = 0u64;
    let mut s = 0u64;
    for cell in cells {
        let matches = sel.iter().enumerate().all(|(d, want)| match want {
            Some(level) => cell.levels.get(d).copied() == Some(*level),
            None => true,
        });
        if matches {
            n = n.saturating_add(cell.n);
            s = s.saturating_add(cell.successes);
        }
    }
    (n, s)
}

/// Accumulated linear-contrast weight on one pooled cell (keyed by its selector
/// constraints). Selectors are stored as a `Vec<Option<usize>>` so equal
/// constraints merge (a cell can appear in several elementary contrasts).
type Contrast = BTreeMap<Selector, f64>;

/// Add `coef` to the weight on the pooled cell described by `sel`.
fn add_term(contrast: &mut Contrast, sel: Selector, coef: f64) {
    *contrast.entry(sel).or_insert(0.0) += coef;
}

/// Evaluate a linear contrast into a Normal posterior summary, or `None` if a
/// participating pooled cell is empty or the sd is degenerate.
///
/// `mean = Σ coef·m`, `var = Σ coef²·v` over the distinct pooled cells (each
/// contributes once, with its net accumulated coefficient — this keeps the
/// independent-cell variance correct even when a cell recurs across the
/// elementary contrasts that were averaged into this one).
fn summarize(cells: &[NdBinaryCell], contrast: &Contrast) -> Option<BayesianInteraction> {
    let mut mean = 0.0f64;
    let mut var = 0.0f64;
    for (sel, &coef) in contrast {
        // A net-zero coefficient cell does not participate.
        if coef == 0.0 {
            continue;
        }
        let (n, s) = pool(cells, sel);
        if n == 0 {
            // Empty participating cell → posterior contrast undefined here.
            return None;
        }
        let (m, v) = beta_posterior(n, s);
        mean += coef * m;
        var += coef * coef * v;
    }

    let sd = var.sqrt();
    if sd <= 0.0 || !sd.is_finite() || !mean.is_finite() {
        return None;
    }

    // Posterior probability the effect is non-null in its estimated direction.
    let prob = super::norm_cdf(mean.abs() / sd);
    Some(BayesianInteraction {
        prob,
        expected: mean,
        ci_low: mean - Z_95 * sd,
        ci_high: mean + Z_95 * sd,
    })
}

/// Number of levels present on each factor (max index + 1), for `n_factors`
/// factors. A factor with no observed level has a count of 0.
fn level_counts(cells: &[NdBinaryCell], n_factors: usize) -> Vec<usize> {
    let mut counts = vec![0usize; n_factors];
    for cell in cells {
        for (d, count) in counts.iter_mut().enumerate() {
            if let Some(&lvl) = cell.levels.get(d) {
                *count = (*count).max(lvl + 1);
            }
        }
    }
    counts
}

/// Build the main-effect contrast for factor `i`: rate(top) − rate(level 0),
/// collapsed over the other factors. `None` if factor `i` has fewer than 2
/// levels (no contrast to form).
fn main_contrast(n_factors: usize, i: usize, counts: &[usize]) -> Option<Contrast> {
    let li = counts[i];
    if li < 2 {
        return None;
    }
    let top = li - 1;
    let mut contrast = Contrast::new();
    let mut sel_top: Selector = vec![None; n_factors];
    let mut sel_base: Selector = vec![None; n_factors];
    sel_top[i] = Some(top);
    sel_base[i] = Some(0);
    add_term(&mut contrast, sel_top, 1.0);
    add_term(&mut contrast, sel_base, -1.0);
    Some(contrast)
}

/// Build the two-way contrast for factors `(a, b)`: the mean of the
/// `(Lₐ−1)(L_b−1)` elementary 2×2 interaction contrasts anchored at level 0,
/// collapsed over the other factors. `None` if either factor has < 2 levels.
fn two_way_contrast(n_factors: usize, a: usize, b: usize, counts: &[usize]) -> Option<Contrast> {
    let (la, lb) = (counts[a], counts[b]);
    if la < 2 || lb < 2 {
        return None;
    }
    let k = ((la - 1) * (lb - 1)) as f64; // number of elementary contrasts
    let w = 1.0 / k; // averaging weight
    let mut contrast = Contrast::new();
    let mk = |ai: usize, bi: usize| -> Selector {
        let mut sel = vec![None; n_factors];
        sel[a] = Some(ai);
        sel[b] = Some(bi);
        sel
    };
    for ai in 1..la {
        for bi in 1..lb {
            // Elementary 2×2 interaction contrast anchored at (0,0):
            // (p[ai][bi] − p[ai][0]) − (p[0][bi] − p[0][0]).
            add_term(&mut contrast, mk(ai, bi), w);
            add_term(&mut contrast, mk(ai, 0), -w);
            add_term(&mut contrast, mk(0, bi), -w);
            add_term(&mut contrast, mk(0, 0), w);
        }
    }
    Some(contrast)
}

/// Build the three-way contrast for factors `(a, b, c)`: the
/// difference-in-differences-of-differences over the 2×2×2 corners, each factor
/// taken at level 0 or its top level. `None` if any factor has < 2 levels.
fn three_way_contrast(
    n_factors: usize,
    a: usize,
    b: usize,
    c: usize,
    counts: &[usize],
) -> Option<Contrast> {
    let (la, lb, lc) = (counts[a], counts[b], counts[c]);
    if la < 2 || lb < 2 || lc < 2 {
        return None;
    }
    let (ta, tb, tc) = (la - 1, lb - 1, lc - 1);
    let mut contrast = Contrast::new();
    // For each factor, level 0 carries sign −1, the top level carries +1; the
    // corner coefficient is the product of the three per-factor signs.
    for (ai, sa) in [(0usize, -1.0f64), (ta, 1.0f64)] {
        for (bi, sb) in [(0usize, -1.0f64), (tb, 1.0f64)] {
            for (ci, sc) in [(0usize, -1.0f64), (tc, 1.0f64)] {
                let mut sel = vec![None; n_factors];
                sel[a] = Some(ai);
                sel[b] = Some(bi);
                sel[c] = Some(ci);
                add_term(&mut contrast, sel, sa * sb * sc);
            }
        }
    }
    Some(contrast)
}

/// See module contract. Returns Bayesian Beta-Binomial interaction posteriors,
/// one per emitted term (sparse/degenerate terms omitted).
pub fn binary_bayes(cells: &[NdBinaryCell], order: usize) -> Vec<(TermKind, BayesianInteraction)> {
    if cells.is_empty() || order == 0 {
        return Vec::new();
    }

    let n_factors = order;
    let counts = level_counts(cells, n_factors);
    let mut out = Vec::new();

    // Main effects: one per factor.
    for i in 0..n_factors {
        if let Some(contrast) = main_contrast(n_factors, i, &counts)
            && let Some(bi) = summarize(cells, &contrast)
        {
            out.push((TermKind::Main { factor: i }, bi));
        }
    }

    // Pairwise interactions: every (a < b) pair.
    for a in 0..n_factors {
        for b in (a + 1)..n_factors {
            if let Some(contrast) = two_way_contrast(n_factors, a, b, &counts)
                && let Some(bi) = summarize(cells, &contrast)
            {
                out.push((TermKind::TwoWay { a, b }, bi));
            }
        }
    }

    // Three-way interaction (only for order == 3): factors {0,1,2}.
    if order == 3
        && let Some(contrast) = three_way_contrast(n_factors, 0, 1, 2, &counts)
        && let Some(bi) = summarize(cells, &contrast)
    {
        out.push((TermKind::ThreeWay { a: 0, b: 1, c: 2 }, bi));
    }

    out
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an `NdBinaryCell` from a level tuple and `(n, successes)`.
    fn cell(levels: &[usize], n: u64, successes: u64) -> NdBinaryCell {
        NdBinaryCell {
            levels: levels.to_vec(),
            n,
            successes,
        }
    }

    fn find(
        terms: &[(TermKind, BayesianInteraction)],
        kind: TermKind,
    ) -> Option<&BayesianInteraction> {
        terms.iter().find(|(k, _)| *k == kind).map(|(_, b)| b)
    }

    // ── beta_posterior sanity ────────────────────────────────────────────────

    /// Beta(1,1) prior (no data) → mean 0.5, var 1/12.
    #[test]
    fn beta_posterior_uniform_prior() {
        let (m, v) = beta_posterior(0, 0);
        assert!((m - 0.5).abs() < 1e-12);
        assert!((v - 1.0 / 12.0).abs() < 1e-12);
    }

    /// Variance shrinks toward 0 and mean toward the empirical rate as n grows.
    #[test]
    fn beta_posterior_concentrates_with_data() {
        let (m, v) = beta_posterior(1000, 400);
        assert!((m - 0.4007984).abs() < 1e-3, "mean={m}");
        assert!(v < 1e-3, "var should be small, got {v}");
        assert!(v > 0.0);
    }

    // ── 2×2 strong planted interaction ───────────────────────────────────────

    /// p11 high, others equal → strong DiD, prob ≈ 1, CI excludes 0.
    #[test]
    fn two_way_strong_interaction_detected() {
        // p00=p01=p10=0.10, p11=0.40 → true DiD = (.40-.10)-(.10-.10) = 0.30.
        let cells = [
            cell(&[0, 0], 1000, 100),
            cell(&[0, 1], 1000, 100),
            cell(&[1, 0], 1000, 100),
            cell(&[1, 1], 1000, 400),
        ];
        let terms = binary_bayes(&cells, 2);
        let bi = find(&terms, TermKind::TwoWay { a: 0, b: 1 }).expect("two-way present");
        assert!(bi.prob > 0.95, "prob={}", bi.prob);
        // expected ≈ true DiD (posterior means barely shrink at n=1000).
        assert!(
            (bi.expected - 0.30).abs() < 0.01,
            "expected={}",
            bi.expected
        );
        // 95% CI excludes 0 for a clear interaction.
        assert!(bi.ci_low > 0.0, "ci_low={}", bi.ci_low);
        assert!(bi.ci_high > bi.ci_low);
    }

    /// Strong *negative* interaction → expected < 0, prob high, CI below 0.
    #[test]
    fn two_way_strong_negative_interaction_detected() {
        // p00=p01=p10=0.40, p11=0.10 → DiD = (.10-.40)-(.40-.40) = -0.30.
        let cells = [
            cell(&[0, 0], 1000, 400),
            cell(&[0, 1], 1000, 400),
            cell(&[1, 0], 1000, 400),
            cell(&[1, 1], 1000, 100),
        ];
        let terms = binary_bayes(&cells, 2);
        let bi = find(&terms, TermKind::TwoWay { a: 0, b: 1 }).expect("two-way present");
        assert!(bi.prob > 0.95, "prob={}", bi.prob);
        assert!(
            (bi.expected + 0.30).abs() < 0.01,
            "expected={}",
            bi.expected
        );
        assert!(bi.ci_high < 0.0, "ci_high={}", bi.ci_high);
    }

    // ── 2×2 additive / no interaction ────────────────────────────────────────

    /// Additive effects → DiD ≈ 0, prob ≈ 0.5, CI straddles 0.
    #[test]
    fn two_way_additive_no_interaction() {
        // p00=.10, p01=.20, p10=.30, p11=.40 → DiD = (.40-.30)-(.20-.10) = 0.
        let cells = [
            cell(&[0, 0], 2000, 200),
            cell(&[0, 1], 2000, 400),
            cell(&[1, 0], 2000, 600),
            cell(&[1, 1], 2000, 800),
        ];
        let terms = binary_bayes(&cells, 2);
        let bi = find(&terms, TermKind::TwoWay { a: 0, b: 1 }).expect("two-way present");
        assert!(
            (bi.expected).abs() < 0.01,
            "expected≈0, got {}",
            bi.expected
        );
        assert!((bi.prob - 0.5).abs() < 0.1, "prob≈0.5, got {}", bi.prob);
        assert!(bi.ci_low < 0.0 && bi.ci_high > 0.0, "CI must include 0");
    }

    /// Main effects are returned for both factors in a 2×2 with marginal lift.
    #[test]
    fn two_way_grid_returns_main_effects() {
        let cells = [
            cell(&[0, 0], 1000, 100),
            cell(&[0, 1], 1000, 200),
            cell(&[1, 0], 1000, 300),
            cell(&[1, 1], 1000, 400),
        ];
        let terms = binary_bayes(&cells, 2);
        // Factor 0 top vs base: collapsed rate (1,*) − (0,*) = .35 − .15 = +.20.
        let m0 = find(&terms, TermKind::Main { factor: 0 }).expect("main 0");
        assert!(
            (m0.expected - 0.20).abs() < 0.01,
            "m0.expected={}",
            m0.expected
        );
        assert!(m0.prob > 0.95);
        // Factor 1 top vs base: (*,1) − (*,0) = .30 − .20 = +.10.
        let m1 = find(&terms, TermKind::Main { factor: 1 }).expect("main 1");
        assert!(
            (m1.expected - 0.10).abs() < 0.01,
            "m1.expected={}",
            m1.expected
        );
        assert!(m1.prob > 0.95);
    }

    // ── larger 3×2 two-way (averaged elementary contrasts) ───────────────────

    /// A 3×2 table where the second non-base row carries an interaction is still
    /// summarised into one two-way effect, returned and finite.
    #[test]
    fn two_way_3x2_averaged_contrast_returned() {
        let cells = [
            cell(&[0, 0], 1000, 100), // .10
            cell(&[0, 1], 1000, 100), // .10
            cell(&[1, 0], 1000, 100), // .10
            cell(&[1, 1], 1000, 100), // .10  (row 1: no interaction)
            cell(&[2, 0], 1000, 100), // .10
            cell(&[2, 1], 1000, 500), // .50  (row 2: strong interaction)
        ];
        let terms = binary_bayes(&cells, 2);
        let bi = find(&terms, TermKind::TwoWay { a: 0, b: 1 }).expect("two-way present");
        // Two elementary contrasts: row1 DiD = 0, row2 DiD = (.50-.10)-(.10-.10)=.40.
        // Mean = (0 + .40)/2 = .20.
        assert!(
            (bi.expected - 0.20).abs() < 0.02,
            "expected={}",
            bi.expected
        );
        assert!(bi.expected.is_finite() && bi.ci_low.is_finite() && bi.ci_high.is_finite());
        assert!(bi.prob > 0.5);
    }

    // ── order-3 three-way + mains ────────────────────────────────────────────

    /// order==3 yields a ThreeWay{0,1,2} entry and a Main for each of 3 factors.
    #[test]
    fn order_three_returns_threeway_and_all_mains() {
        // 2×2×2 fully populated. Plant a three-way: only the (1,1,1) corner lifts.
        let cells = [
            cell(&[0, 0, 0], 800, 80),
            cell(&[0, 0, 1], 800, 80),
            cell(&[0, 1, 0], 800, 80),
            cell(&[0, 1, 1], 800, 80),
            cell(&[1, 0, 0], 800, 80),
            cell(&[1, 0, 1], 800, 80),
            cell(&[1, 1, 0], 800, 80),
            cell(&[1, 1, 1], 800, 400), // the lone lifted corner
        ];
        let terms = binary_bayes(&cells, 3);
        // ThreeWay present.
        let tw = find(&terms, TermKind::ThreeWay { a: 0, b: 1, c: 2 }).expect("three-way present");
        // DDD = [(p111-p110)-(p101-p100)] - [(p011-p010)-(p001-p000)]
        //     = [(.50-.10)-(.10-.10)] - [0] = .40.
        assert!(
            (tw.expected - 0.40).abs() < 0.02,
            "ddd expected={}",
            tw.expected
        );
        assert!(tw.prob > 0.95, "prob={}", tw.prob);
        // A main effect for each of the three factors.
        for f in 0..3 {
            assert!(
                find(&terms, TermKind::Main { factor: f }).is_some(),
                "missing main {f}"
            );
        }
        // All three pairwise interactions too.
        assert!(find(&terms, TermKind::TwoWay { a: 0, b: 1 }).is_some());
        assert!(find(&terms, TermKind::TwoWay { a: 0, b: 2 }).is_some());
        assert!(find(&terms, TermKind::TwoWay { a: 1, b: 2 }).is_some());
    }

    /// order==2 must NOT emit a three-way term.
    #[test]
    fn order_two_emits_no_threeway() {
        let cells = [
            cell(&[0, 0], 1000, 100),
            cell(&[0, 1], 1000, 100),
            cell(&[1, 0], 1000, 100),
            cell(&[1, 1], 1000, 400),
        ];
        let terms = binary_bayes(&cells, 2);
        assert!(
            terms
                .iter()
                .all(|(k, _)| !matches!(k, TermKind::ThreeWay { .. }))
        );
    }

    // ── determinism ──────────────────────────────────────────────────────────

    /// Identical inputs → bit-identical outputs (no RNG, stable accumulation).
    #[test]
    fn deterministic_bit_identical() {
        let cells = [
            cell(&[0, 0, 0], 800, 80),
            cell(&[0, 0, 1], 800, 120),
            cell(&[0, 1, 0], 800, 90),
            cell(&[0, 1, 1], 800, 160),
            cell(&[1, 0, 0], 800, 200),
            cell(&[1, 0, 1], 800, 240),
            cell(&[1, 1, 0], 800, 300),
            cell(&[1, 1, 1], 800, 440),
        ];
        let a = binary_bayes(&cells, 3);
        let b = binary_bayes(&cells, 3);
        assert_eq!(a.len(), b.len());
        for ((ka, ba), (kb, bb)) in a.iter().zip(b.iter()) {
            assert_eq!(ka, kb);
            // Bit-identical f64 fields (no tolerance) — there is no RNG.
            assert_eq!(ba.prob.to_bits(), bb.prob.to_bits());
            assert_eq!(ba.expected.to_bits(), bb.expected.to_bits());
            assert_eq!(ba.ci_low.to_bits(), bb.ci_low.to_bits());
            assert_eq!(ba.ci_high.to_bits(), bb.ci_high.to_bits());
        }
    }

    // ── degenerate / sparse omission ─────────────────────────────────────────

    /// An empty participating cell omits the affected term(s).
    #[test]
    fn empty_participating_cell_omits_term() {
        // (1,1) entirely missing → the two-way DiD has an empty participating
        // cell and must be omitted. Main{0}/Main{1} also touch (1,1) only via
        // collapsed pools that remain non-empty, so they can still be returned.
        let cells = [
            cell(&[0, 0], 1000, 100),
            cell(&[0, 1], 1000, 100),
            cell(&[1, 0], 1000, 100),
            // (1,1) absent
        ];
        let terms = binary_bayes(&cells, 2);
        assert!(
            find(&terms, TermKind::TwoWay { a: 0, b: 1 }).is_none(),
            "two-way with an empty corner must be omitted"
        );
    }

    /// Empty input → no terms.
    #[test]
    fn empty_input_no_terms() {
        assert!(binary_bayes(&[], 2).is_empty());
        assert!(binary_bayes(&[], 3).is_empty());
    }

    /// order==0 → no terms (defensive).
    #[test]
    fn order_zero_no_terms() {
        let cells = [cell(&[0], 100, 10)];
        assert!(binary_bayes(&cells, 0).is_empty());
    }

    /// A factor with a single level yields no main contrast for that factor.
    #[test]
    fn single_level_factor_has_no_main() {
        // Factor 0 has only level 0 (degenerate); factor 1 has two levels.
        let cells = [cell(&[0, 0], 1000, 100), cell(&[0, 1], 1000, 300)];
        let terms = binary_bayes(&cells, 2);
        assert!(
            find(&terms, TermKind::Main { factor: 0 }).is_none(),
            "single-level factor 0 must not produce a main effect"
        );
        // Factor 1 still has a main effect; the two-way needs a 2nd level on
        // factor 0, so it is absent.
        assert!(find(&terms, TermKind::Main { factor: 1 }).is_some());
        assert!(find(&terms, TermKind::TwoWay { a: 0, b: 1 }).is_none());
    }

    /// CI is symmetric about `expected` at ±1.96·sd, and width is positive.
    #[test]
    fn credible_interval_is_symmetric() {
        let cells = [
            cell(&[0, 0], 1000, 100),
            cell(&[0, 1], 1000, 100),
            cell(&[1, 0], 1000, 100),
            cell(&[1, 1], 1000, 400),
        ];
        let terms = binary_bayes(&cells, 2);
        let bi = find(&terms, TermKind::TwoWay { a: 0, b: 1 }).unwrap();
        let mid = 0.5 * (bi.ci_low + bi.ci_high);
        assert!(
            (mid - bi.expected).abs() < 1e-12,
            "CI not centered on expected"
        );
        assert!(bi.ci_high - bi.ci_low > 0.0);
    }
}
