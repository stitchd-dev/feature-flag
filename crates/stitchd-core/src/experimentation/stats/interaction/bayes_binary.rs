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
//! - `ci_low` / `ci_high` = `expected ± Z95·sd` (central 95 % credible interval,
//!   with the shared `Z95 = Φ⁻¹(0.975) ≈ 1.959964`),
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

use super::bayes_common::{CellPost, enumerate_terms, run_terms};
use super::{BayesianInteraction, NdBinaryCell, TermKind};

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

/// See module contract. Returns Bayesian Beta-Binomial interaction posteriors,
/// one per emitted term (sparse/degenerate terms omitted).
///
/// Enumeration, contrast construction, and the Normal summary are delegated to
/// [`super::bayes_common`]; this worker supplies only the per-cell Beta posterior
/// and the binary cell collapse. The collapse pools `(n, successes)` over the
/// non-participating factors (saturating, so a malformed `successes > n` cell can
/// never underflow), then forms the pooled Beta posterior; an empty pooled cell
/// (`n == 0`) yields `None`, which omits the term.
pub fn binary_bayes(cells: &[NdBinaryCell], order: usize) -> Vec<(TermKind, BayesianInteraction)> {
    if cells.is_empty() || order == 0 {
        return Vec::new();
    }

    // Per-factor level counts over exactly `order` factors (max index + 1). A
    // factor absent from a cell's tuple contributes 0 levels, so a single-level
    // or missing factor simply yields no contrast for that term.
    let mut dims = vec![0usize; order];
    for cell in cells {
        for (d, count) in dims.iter_mut().enumerate() {
            if let Some(&lvl) = cell.levels.get(d) {
                *count = (*count).max(lvl + 1);
            }
        }
    }

    let terms = enumerate_terms(order);
    run_terms(&terms, &dims, |factors, key| {
        // Pool (n, successes) over every cell matching the participating-level
        // key, collapsing (summing) over all other factors.
        let mut n = 0u64;
        let mut s = 0u64;
        for cell in cells {
            let matches = factors
                .iter()
                .zip(key)
                .all(|(&f, &want)| cell.levels.get(f).copied() == Some(want));
            if matches {
                n = n.saturating_add(cell.n);
                s = s.saturating_add(cell.successes);
            }
        }
        if n == 0 {
            // Empty participating cell → posterior contrast undefined here.
            return None;
        }
        let (est, var) = beta_posterior(n, s);
        Some(CellPost { est, var })
    })
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

    /// Golden bit-for-bit snapshot of `binary_bayes` on a fully-populated 2×2×2
    /// grid at order 3 (mains + all pairwise + the three-way). Pins every
    /// posterior field's exact `f64` bits so any unintended change to the numeric
    /// output is caught immediately.
    ///
    /// The `prob` / `expected` bits are unchanged from the original snapshot
    /// (review #14, pre shared-driver refactor); the `ci_low` / `ci_high` bits
    /// were re-captured when the credible-interval multiplier was unified from
    /// the local `1.96` onto the shared `Z95 = Φ⁻¹(0.975) ≈ 1.959964` — the only
    /// intended numeric shift (≈0.002 % on the CI half-width).
    #[test]
    fn binary_bayes_golden_bits_order3() {
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
        // (kind, prob_bits, expected_bits, ci_low_bits, ci_high_bits)
        let expected: [(TermKind, u64, u64, u64, u64); 7] = [
            (
                TermKind::Main { factor: 0 },
                4607182418800017408,
                4597381955900730207,
                4596639788474320140,
                4598124123327140274,
            ),
            (
                TermKind::Main { factor: 1 },
                4607182418800017408,
                4592540797275680476,
                4591015057102861097,
                4593869078683202887,
            ),
            (
                TermKind::Main { factor: 2 },
                4607182418800017408,
                4591190561284963524,
                4589661004272818350,
                4592720118297108698,
            ),
            (
                TermKind::TwoWay { a: 0, b: 1 },
                4607182418800017163,
                4594790491735442412,
                4592979336954307367,
                4596255505034778180,
            ),
            (
                TermKind::TwoWay { a: 0, b: 2 },
                4607018327897214214,
                4586457989054090256,
                4568506984224327904,
                4590762821128397769,
            ),
            (
                TermKind::TwoWay { a: 1, b: 2 },
                4607181717041538442,
                4590511790965868192,
                4585794434242570741,
                4593542354665183302,
            ),
            (
                TermKind::ThreeWay { a: 0, b: 1, c: 2 },
                4607029618757127072,
                4590953736851014000,
                4574286726923736080,
                4595218689454320336,
            ),
        ];

        let got = binary_bayes(&cells, 3);
        assert_eq!(got.len(), expected.len(), "term count drifted");
        for ((kind, bi), (ek, ep, ee, el, eh)) in got.iter().zip(expected.iter()) {
            assert_eq!(kind, ek, "term identity/order drifted");
            assert_eq!(bi.prob.to_bits(), *ep, "{kind:?} prob bits drifted");
            assert_eq!(bi.expected.to_bits(), *ee, "{kind:?} expected bits drifted");
            assert_eq!(bi.ci_low.to_bits(), *el, "{kind:?} ci_low bits drifted");
            assert_eq!(bi.ci_high.to_bits(), *eh, "{kind:?} ci_high bits drifted");
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

    // ── order-4 (general NWay contrast) ───────────────────────────────────────

    /// order==4 yields the full hierarchical set incl. the top four-way NWay term
    /// and all four three-way subsets; a planted four-way drives high `prob`.
    #[test]
    fn order_four_returns_fourway_and_subsets() {
        // Full 2×2×2×2; plant a four-way: the (1,1,1,*) three-way bump exists
        // only at d=1, absent at d=0 → the four-way contrast is large.
        let mut cells = Vec::new();
        for a in 0..2 {
            for b in 0..2 {
                for c in 0..2 {
                    for d in 0..2 {
                        let lifted = d == 1 && (a, b, c) == (1, 1, 1);
                        let s = if lifted { 400 } else { 80 };
                        cells.push(cell(&[a, b, c, d], 800, s));
                    }
                }
            }
        }
        let terms = binary_bayes(&cells, 4);
        // The top four-way NWay term is present and detected.
        let four = find(&terms, TermKind::of(&[0, 1, 2, 3])).expect("four-way present");
        assert!(four.prob.is_finite() && four.expected.is_finite());
        assert!(four.prob > 0.9, "planted four-way prob={}", four.prob);
        // All four three-way subsets surface too.
        for trip in [[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]] {
            assert!(
                find(&terms, TermKind::of(&trip)).is_some(),
                "missing three-way {trip:?}"
            );
        }
    }

    /// CI is symmetric about `expected` at ±Z95·sd, and width is positive.
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
