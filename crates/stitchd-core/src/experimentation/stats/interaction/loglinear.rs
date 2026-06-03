//! (P2.T2) Frequentist **log-linear hierarchical decomposition** for binary
//! (conversion / funnel) metrics over a k-factor contingency grid.
//!
//! ## Contract (signature fixed by the seam; body implemented by the worker)
//!
//! `binary_terms(cells, order)` returns one [`TermResult`] per decomposition
//! term, with `bayes: None` (Bayesian posteriors are attached later, by
//! [`super::bayes_binary`], keyed on [`super::TermKind`]):
//! - all main effects — `TermKind::Main { factor }` for each factor `0..order`
//! - all pairwise interactions — `TermKind::TwoWay { a, b }` for each pair
//! - for `order == 3`, the three-way interaction — `TermKind::ThreeWay { .. }`
//!
//! Each term's `freq` is the Frequentist log-linear test of that term: fit the
//! hierarchical log-linear model that excludes the term (the no-term null),
//! e.g. via iterative proportional fitting over the success/failure × factor
//! grid, and compare to the saturated fit with a Pearson (or likelihood-ratio)
//! chi-square on the correct interaction df. Degenerate / too-sparse terms use
//! [`super::InteractionResult::insufficient`].
//!
//! The order-2 path MUST reproduce [`super::binary_interaction`] (regression
//! gate, P2.T7). Distribution helpers available from the parent module:
//! `super::chi_square_sf`.
//!
//! ### Method
//!
//! Write `O` for the binary outcome (success / failure) factor and `A, B, C`
//! for the experiment factors. Each emitted term tests the dependence of the
//! outcome on a sub-structure of the factors:
//!
//! - **`Main { factor: i }`** — homogeneity of the success rate across factor
//!   `i`'s levels: a Pearson χ² of the `O × i` table on `Lᵢ − 1` df.
//! - **`TwoWay { a, b }`** — the `O × a × b` dependence (the outcome reacts to
//!   the `a × b` interaction). Implemented by collapsing onto the `(a, b)` grid
//!   and **delegating to [`super::binary_interaction`]**, so at `order == 2`
//!   the result is byte-identical to the legacy two-way test (regression gate).
//! - **`ThreeWay { 0, 1, 2 }`** — the `O × A × B × C` four-way dependence (the
//!   `A × B` interaction's effect on the outcome itself varies across `C`).
//!   Fit the no-four-way log-linear model `[OAB][OAC][OBC]` to the
//!   success/failure counts by iterative proportional fitting (IPF) over the
//!   three outcome-bearing two-way margins, then Pearson χ² of observed vs
//!   fitted on `(La − 1)(Lb − 1)(Lc − 1)` df.

use super::{NdBinaryCell, TermKind, TermResult};

/// Minimum total trials below which no term is testable (matches the legacy
/// [`super::binary_interaction`] guard).
const MIN_TOTAL_N: u64 = 20;

/// Decompose a k-factor binary contingency grid into per-term Frequentist
/// log-linear tests. See the module docs for the method and the emitted-term
/// contract. All returned terms carry `bayes: None`.
pub fn binary_terms(cells: &[NdBinaryCell], order: usize) -> Vec<TermResult> {
    let mut out = Vec::new();

    // Main effects: one per factor.
    for i in 0..order {
        out.push(TermResult {
            kind: TermKind::Main { factor: i },
            freq: main_effect(cells, i),
            bayes: None,
        });
    }

    // Pairwise interactions: one per unordered pair, in (a < b) order.
    for a in 0..order {
        for b in (a + 1)..order {
            out.push(TermResult {
                kind: TermKind::TwoWay { a, b },
                freq: two_way(cells, a, b),
                bayes: None,
            });
        }
    }

    // Three-way interaction (only defined at order 3).
    if order == 3 {
        out.push(TermResult {
            kind: TermKind::ThreeWay { a: 0, b: 1, c: 2 },
            freq: three_way(cells),
            bayes: None,
        });
    }

    out
}

/// Largest level index present on factor `f`, plus one (the dense level count),
/// or 0 when no cell carries that factor.
fn levels_on(cells: &[NdBinaryCell], f: usize) -> usize {
    cells
        .iter()
        .filter_map(|c| c.levels.get(f).copied())
        .max()
        .map_or(0, |m| m + 1)
}

// ── Main effect ──────────────────────────────────────────────────────────────

/// Pearson χ² test of homogeneity of the success rate across factor `i`'s
/// levels. Collapses `cells` onto factor `i` (summing trials and successes over
/// every other factor), then tests `H₀: every level shares the grand rate p̄`.
fn main_effect(cells: &[NdBinaryCell], i: usize) -> InteractionResultLocal {
    let levels = levels_on(cells, i);
    let df = levels.saturating_sub(1) as u32;
    if levels < 2 {
        return insufficient(df);
    }

    // Collapse onto factor i.
    let mut n = vec![0u64; levels];
    let mut s = vec![0u64; levels];
    for c in cells {
        let l = c.levels[i];
        n[l] = n[l].saturating_add(c.n);
        s[l] = s[l].saturating_add(c.successes);
    }

    let total_n: u64 = n.iter().sum();
    if total_n < MIN_TOTAL_N {
        return insufficient(df);
    }
    let total_s: u64 = s.iter().sum();

    let grand_n = total_n as f64;
    let grand_rate = total_s as f64 / grand_n;
    // No outcome variance → nothing to test the level differences against.
    if grand_rate <= 0.0 || grand_rate >= 1.0 {
        return insufficient(df);
    }

    // X² = Σ_l [ (s_l − E_s)²/E_s + (f_l − E_f)²/E_f ], with E_s = n_l·p̄,
    // E_f = n_l·(1−p̄). Any non-positive expected count makes the cell's term
    // undefined → abstain.
    let mut chi_sq = 0.0f64;
    for l in 0..levels {
        let n_l = n[l] as f64;
        let exp_s = n_l * grand_rate;
        let exp_f = n_l * (1.0 - grand_rate);
        if exp_s <= 0.0 || exp_f <= 0.0 {
            return insufficient(df);
        }
        let obs_s = s[l] as f64;
        let obs_f = n_l - obs_s;
        let ds = obs_s - exp_s;
        let df_ = obs_f - exp_f;
        chi_sq += ds * ds / exp_s + df_ * df_ / exp_f;
    }

    if !chi_sq.is_finite() {
        return insufficient(df);
    }
    let p_value = super::chi_square_sf(chi_sq, df as f64);
    InteractionResultLocal {
        estimate: chi_sq,
        statistic: chi_sq,
        p_value,
        df,
        significant: p_value < super::ALPHA,
        insufficient_data: false,
    }
}

// ── Two-way interaction (delegates to the legacy pairwise test) ───────────────

/// Pairwise `O × a × b` interaction. Collapses `cells` onto the `(a, b)` grid
/// (summing trials and successes over all other factors) and delegates to
/// [`super::binary_interaction`] so the order-2 path is byte-identical to the
/// legacy two-way test.
fn two_way(cells: &[NdBinaryCell], a: usize, b: usize) -> InteractionResultLocal {
    let la = levels_on(cells, a);
    let lb = levels_on(cells, b);
    // Mirror the legacy guard: no testable interaction term without ≥2 levels
    // on each factor. (`binary_interaction` returns df 0 here; matching that.)
    if la < 2 || lb < 2 {
        return insufficient(0);
    }

    // Collapse onto the (a, b) grid, accumulating into a dense table so repeated
    // / missing N-d cells fold together before handing off.
    let mut n = vec![vec![0u64; lb]; la];
    let mut s = vec![vec![0u64; lb]; la];
    for c in cells {
        let ai = c.levels[a];
        let bi = c.levels[b];
        n[ai][bi] = n[ai][bi].saturating_add(c.n);
        s[ai][bi] = s[ai][bi].saturating_add(c.successes);
    }

    let mut collapsed = Vec::with_capacity(la * lb);
    for ai in 0..la {
        for bi in 0..lb {
            collapsed.push(super::BinaryCell {
                a_level: ai,
                b_level: bi,
                n: n[ai][bi],
                successes: s[ai][bi],
            });
        }
    }

    super::binary_interaction(&collapsed)
}

// ── Three-way interaction (IPF on the no-four-way log-linear model) ───────────

/// Tolerance / iteration budget for iterative proportional fitting.
const IPF_TOL: f64 = 1e-8;
const IPF_MAX_ITER: usize = 200;

/// The `O × A × B × C` four-way interaction: does the `A × B` interaction's
/// effect on the outcome itself vary across `C`?
///
/// Fits the no-four-way hierarchical log-linear model `[OAB][OAC][OBC]` to the
/// success/failure counts by IPF over the three outcome-bearing two-way margins
/// (`O×A×B`, `O×A×C`, `O×B×C`), then a Pearson χ² of observed vs fitted on
/// `(La−1)(Lb−1)(Lc−1)` df. Abstains on any structural degeneracy (a factor
/// with <2 levels, too few units, an empty margin, or IPF failing to converge /
/// producing a non-finite fit).
fn three_way(cells: &[NdBinaryCell]) -> InteractionResultLocal {
    let la = levels_on(cells, 0);
    let lb = levels_on(cells, 1);
    let lc = levels_on(cells, 2);
    let df = (la.saturating_sub(1) * lb.saturating_sub(1) * lc.saturating_sub(1)) as u32;
    if la < 2 || lb < 2 || lc < 2 {
        return insufficient(df);
    }

    // Work on a flat O×A×B×C grid (outcome o = 0 failure / 1 success) addressed
    // by `idx`. A flat layout keeps the IPF rescale loops free of nested-Vec
    // indexing and lets the summed-out axis participate in real arithmetic.
    let cells_per_outcome = la * lb * lc;
    let idx = |o: usize, a: usize, b: usize, c: usize| ((o * la + a) * lb + b) * lc + c;

    // Observed success/failure counts.
    let mut obs = vec![0.0f64; 2 * cells_per_outcome];
    let mut total_n = 0u64;
    let mut total_s = 0u64;
    for cell in cells {
        let (a, b, c) = (cell.levels[0], cell.levels[1], cell.levels[2]);
        // Clamp a malformed `successes > n` cell (mirrors `bayes_binary`): the
        // failure count would otherwise underflow `u64` (panic in debug / wrap
        // in release). The clamped success count keeps obs/margins consistent.
        let successes = cell.successes.min(cell.n);
        let fail = cell.n - successes;
        obs[idx(1, a, b, c)] += successes as f64;
        obs[idx(0, a, b, c)] += fail as f64;
        total_n = total_n.saturating_add(cell.n);
        total_s = total_s.saturating_add(successes);
    }

    if total_n < MIN_TOTAL_N {
        return insufficient(df);
    }
    // No outcome variance at all → the 4-way interaction is not estimable.
    if total_s == 0 || total_s == total_n {
        return insufficient(df);
    }

    // Observed outcome-bearing two-way margins (the third factor summed out),
    // flat-indexed in their own layouts.
    let mut m_oab = vec![0.0f64; 2 * la * lb];
    let mut m_oac = vec![0.0f64; 2 * la * lc];
    let mut m_obc = vec![0.0f64; 2 * lb * lc];
    let oab = |o: usize, a: usize, b: usize| (o * la + a) * lb + b;
    let oac = |o: usize, a: usize, c: usize| (o * la + a) * lc + c;
    let obc = |o: usize, b: usize, c: usize| (o * lb + b) * lc + c;
    for o in 0..2 {
        for a in 0..la {
            for b in 0..lb {
                for c in 0..lc {
                    let v = obs[idx(o, a, b, c)];
                    m_oab[oab(o, a, b)] += v;
                    m_oac[oac(o, a, c)] += v;
                    m_obc[obc(o, b, c)] += v;
                }
            }
        }
    }

    // An empty margin cell makes the fit unidentifiable: IPF would scale a 0
    // margin by an indeterminate ratio. Abstain. (Margins over the full table
    // include both outcomes, so a structurally absent O×A×B combination — e.g.
    // a cell that is never a success — blocks the test.)
    let any_zero = |m: &[f64]| m.iter().any(|&x| x <= 0.0);
    if any_zero(&m_oab) || any_zero(&m_oac) || any_zero(&m_obc) {
        return insufficient(df);
    }

    // IPF: start from a uniform table and rescale to each margin in turn until
    // the maximum per-cell change falls below tolerance.
    let mut fit = vec![1.0f64; 2 * cells_per_outcome];
    let mut converged = false;
    for _ in 0..IPF_MAX_ITER {
        let mut max_change = 0.0f64;

        // Fit to O×A×B margin (sum over C).
        for o in 0..2 {
            for a in 0..la {
                for b in 0..lb {
                    let cur: f64 = (0..lc).map(|c| fit[idx(o, a, b, c)]).sum();
                    if cur <= 0.0 {
                        return insufficient(df);
                    }
                    let scale = m_oab[oab(o, a, b)] / cur;
                    for c in 0..lc {
                        let cell = &mut fit[idx(o, a, b, c)];
                        let after = *cell * scale;
                        max_change = max_change.max((after - *cell).abs());
                        *cell = after;
                    }
                }
            }
        }

        // Fit to O×A×C margin (sum over B).
        for o in 0..2 {
            for a in 0..la {
                for c in 0..lc {
                    let cur: f64 = (0..lb).map(|b| fit[idx(o, a, b, c)]).sum();
                    if cur <= 0.0 {
                        return insufficient(df);
                    }
                    let scale = m_oac[oac(o, a, c)] / cur;
                    for b in 0..lb {
                        let cell = &mut fit[idx(o, a, b, c)];
                        let after = *cell * scale;
                        max_change = max_change.max((after - *cell).abs());
                        *cell = after;
                    }
                }
            }
        }

        // Fit to O×B×C margin (sum over A).
        for o in 0..2 {
            for b in 0..lb {
                for c in 0..lc {
                    let cur: f64 = (0..la).map(|a| fit[idx(o, a, b, c)]).sum();
                    if cur <= 0.0 {
                        return insufficient(df);
                    }
                    let scale = m_obc[obc(o, b, c)] / cur;
                    for a in 0..la {
                        let cell = &mut fit[idx(o, a, b, c)];
                        let after = *cell * scale;
                        max_change = max_change.max((after - *cell).abs());
                        *cell = after;
                    }
                }
            }
        }

        if max_change < IPF_TOL {
            converged = true;
            break;
        }
    }

    if !converged {
        return insufficient(df);
    }

    // Pearson χ² of observed vs fitted over every (outcome × cell) entry —
    // `fit` and `obs` share the same flat layout, so iterate in lockstep.
    let mut chi_sq = 0.0f64;
    for (i, &e) in fit.iter().enumerate() {
        if !e.is_finite() {
            return insufficient(df);
        }
        if e > 0.0 {
            let d = obs[i] - e;
            chi_sq += d * d / e;
        }
    }

    if !chi_sq.is_finite() {
        return insufficient(df);
    }
    let p_value = super::chi_square_sf(chi_sq, df as f64);
    InteractionResultLocal {
        estimate: chi_sq,
        statistic: chi_sq,
        p_value,
        df,
        significant: p_value < super::ALPHA,
        insufficient_data: false,
    }
}

// ── Local aliases for the shared result type / constructor ────────────────────
//
// `binary_terms` returns the shared `InteractionResult`; these aliases keep the
// per-term builders readable without re-importing names at every call site.

type InteractionResultLocal = super::InteractionResult;

#[inline]
fn insufficient(df: u32) -> InteractionResultLocal {
    super::InteractionResult::insufficient(df)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experimentation::stats::interaction::BinaryCell;

    // ── builders ──────────────────────────────────────────────────────────────

    /// N-dim binary cell from explicit levels.
    fn cell(levels: &[usize], n: u64, s: u64) -> NdBinaryCell {
        NdBinaryCell {
            levels: levels.to_vec(),
            n,
            successes: s,
        }
    }

    fn bcell(a: usize, b: usize, n: u64, s: u64) -> BinaryCell {
        BinaryCell {
            a_level: a,
            b_level: b,
            n,
            successes: s,
        }
    }

    /// Pull the single term of a given kind out of a decomposition.
    fn pick(terms: &[TermResult], kind: TermKind) -> TermResult {
        let found: Vec<_> = terms.iter().filter(|t| t.kind == kind).collect();
        assert_eq!(found.len(), 1, "expected exactly one {kind:?} term");
        *found[0]
    }

    // ── shape of the decomposition ──────────────────────────────────────────

    /// order-2 emits 2 mains + 1 two-way; order-3 emits 3 mains + 3 two-ways +
    /// 1 three-way; and every term carries `bayes: None`.
    #[test]
    fn emits_the_full_term_set_per_order() {
        let o2 = [
            cell(&[0, 0], 1000, 100),
            cell(&[0, 1], 1000, 100),
            cell(&[1, 0], 1000, 100),
            cell(&[1, 1], 1000, 400),
        ];
        let t2 = binary_terms(&o2, 2);
        assert_eq!(t2.len(), 3);
        assert!(t2.iter().all(|t| t.bayes.is_none()));
        assert!(t2.iter().any(|t| t.kind == TermKind::Main { factor: 0 }));
        assert!(t2.iter().any(|t| t.kind == TermKind::Main { factor: 1 }));
        assert!(t2.iter().any(|t| t.kind == TermKind::TwoWay { a: 0, b: 1 }));
        assert!(
            t2.iter()
                .all(|t| !matches!(t.kind, TermKind::ThreeWay { .. }))
        );

        let o3 = [cell(&[0, 0, 0], 500, 50), cell(&[1, 1, 1], 500, 50)];
        let t3 = binary_terms(&o3, 3);
        // 3 main + 3 two-way + 1 three-way = 7
        assert_eq!(t3.len(), 7);
        assert!(t3.iter().all(|t| t.bayes.is_none()));
        for f in 0..3 {
            assert!(t3.iter().any(|t| t.kind == TermKind::Main { factor: f }));
        }
        for (a, b) in [(0, 1), (0, 2), (1, 2)] {
            assert!(t3.iter().any(|t| t.kind == TermKind::TwoWay { a, b }));
        }
        assert!(
            t3.iter()
                .any(|t| t.kind == TermKind::ThreeWay { a: 0, b: 1, c: 2 })
        );
    }

    // ── regression gate: order-2 TwoWay == legacy binary_interaction ──────────

    /// A planted 2×2 interaction: the TwoWay term reproduces
    /// `binary_interaction` field-for-field (estimate / p_value / df /
    /// significant), and is significant.
    #[test]
    fn two_way_matches_legacy_on_planted_2x2() {
        let nd = [
            cell(&[0, 0], 1000, 100), // .10
            cell(&[0, 1], 1000, 100), // .10
            cell(&[1, 0], 1000, 100), // .10
            cell(&[1, 1], 1000, 400), // .40
        ];
        let legacy = super::super::binary_interaction(&[
            bcell(0, 0, 1000, 100),
            bcell(0, 1, 1000, 100),
            bcell(1, 0, 1000, 100),
            bcell(1, 1, 1000, 400),
        ]);

        let terms = binary_terms(&nd, 2);
        let tw = pick(&terms, TermKind::TwoWay { a: 0, b: 1 }).freq;

        assert_eq!(tw, legacy, "TwoWay must be byte-identical to legacy");
        assert_eq!(tw.estimate, legacy.estimate);
        assert_eq!(tw.p_value, legacy.p_value);
        assert_eq!(tw.df, legacy.df);
        assert_eq!(tw.significant, legacy.significant);
        assert!(tw.significant);
    }

    /// A purely additive 2×2: TwoWay still matches legacy, and is NOT
    /// significant.
    #[test]
    fn two_way_matches_legacy_on_additive_2x2() {
        let nd = [
            cell(&[0, 0], 5000, 500),  // .10
            cell(&[0, 1], 5000, 1000), // .20
            cell(&[1, 0], 5000, 1500), // .30
            cell(&[1, 1], 5000, 2000), // .40
        ];
        let legacy = super::super::binary_interaction(&[
            bcell(0, 0, 5000, 500),
            bcell(0, 1, 5000, 1000),
            bcell(1, 0, 5000, 1500),
            bcell(1, 1, 5000, 2000),
        ]);

        let terms = binary_terms(&nd, 2);
        let tw = pick(&terms, TermKind::TwoWay { a: 0, b: 1 }).freq;

        assert_eq!(tw, legacy);
        assert!(!tw.significant);
    }

    /// At order 3, a TwoWay term collapses the third factor before delegating;
    /// the result must equal `binary_interaction` on the hand-collapsed grid.
    #[test]
    fn two_way_at_order3_collapses_third_factor_to_match_legacy() {
        // Two C levels; the (a,b) marginal counts are the sum across C.
        let nd = [
            cell(&[0, 0, 0], 600, 60),
            cell(&[0, 0, 1], 400, 40),
            cell(&[0, 1, 0], 600, 120),
            cell(&[0, 1, 1], 400, 80),
            cell(&[1, 0, 0], 600, 180),
            cell(&[1, 0, 1], 400, 120),
            cell(&[1, 1, 0], 600, 90),
            cell(&[1, 1, 1], 400, 60),
        ];
        // Hand-collapse onto (a=0, b=1): sum n & successes across c.
        // (0,0): n=1000 s=100 ; (0,1): n=1000 s=200 ;
        // (1,0): n=1000 s=300 ; (1,1): n=1000 s=150
        let legacy = super::super::binary_interaction(&[
            bcell(0, 0, 1000, 100),
            bcell(0, 1, 1000, 200),
            bcell(1, 0, 1000, 300),
            bcell(1, 1, 1000, 150),
        ]);

        let terms = binary_terms(&nd, 3);
        let tw = pick(&terms, TermKind::TwoWay { a: 0, b: 1 }).freq;
        assert_eq!(tw, legacy);
    }

    // ── main effects ──────────────────────────────────────────────────────────

    /// A factor whose success rate clearly varies across levels is significant.
    #[test]
    fn main_effect_varying_rate_is_significant() {
        // Factor 0: level 0 ≈ .10, level 1 ≈ .40 (collapsed over factor 1).
        let nd = [
            cell(&[0, 0], 1000, 100),
            cell(&[0, 1], 1000, 100),
            cell(&[1, 0], 1000, 400),
            cell(&[1, 1], 1000, 400),
        ];
        let m = pick(&binary_terms(&nd, 2), TermKind::Main { factor: 0 }).freq;
        assert!(!m.insufficient_data);
        assert_eq!(m.df, 1);
        assert!(m.p_value < 0.05, "p={}", m.p_value);
        assert!(m.significant);
        assert!((m.estimate - m.statistic).abs() < 1e-12);
    }

    /// A flat factor (identical success rate across levels) is NOT significant.
    #[test]
    fn main_effect_flat_rate_is_not_significant() {
        // Factor 0 is flat at .20 across both levels; only factor 1 varies.
        let nd = [
            cell(&[0, 0], 2000, 200), // .10
            cell(&[0, 1], 2000, 600), // .30  -> level0 total .20
            cell(&[1, 0], 2000, 200), // .10
            cell(&[1, 1], 2000, 600), // .30  -> level1 total .20
        ];
        let m = pick(&binary_terms(&nd, 2), TermKind::Main { factor: 0 }).freq;
        assert!(!m.insufficient_data);
        assert!(m.p_value > 0.05, "p={} chi2={}", m.p_value, m.statistic);
        assert!(!m.significant);
    }

    /// Main effect with a single level → insufficient (no df to test).
    #[test]
    fn main_effect_single_level_is_insufficient() {
        let nd = [cell(&[0, 0], 1000, 100), cell(&[0, 1], 1000, 300)];
        // Factor 0 has only level 0 present.
        let m = pick(&binary_terms(&nd, 2), TermKind::Main { factor: 0 }).freq;
        assert!(m.insufficient_data);
        assert!(!m.significant);
        assert!(m.estimate.is_nan());
    }

    /// Main effect with no outcome variance (grand rate 0) → insufficient.
    #[test]
    fn main_effect_no_outcome_variance_is_insufficient() {
        let nd = [cell(&[0, 0], 1000, 0), cell(&[1, 0], 1000, 0)];
        let m = pick(&binary_terms(&nd, 2), TermKind::Main { factor: 0 }).freq;
        assert!(m.insufficient_data);
    }

    // ── three-way interaction ─────────────────────────────────────────────────

    /// Planted 3-way: the sign of the A×B interaction flips between C levels →
    /// the ThreeWay term is significant.
    #[test]
    fn three_way_planted_interaction_is_significant() {
        // C=0: A×B interaction is +0.30 (p11 high). C=1: it is −0.30 (p11 low).
        // Pairwise-with-outcome margins are (near) balanced, so only the 4-way
        // term carries the signal.
        let n = 2000;
        let nd = [
            // C = 0
            cell(&[0, 0, 0], n, (0.10 * n as f64) as u64),
            cell(&[0, 1, 0], n, (0.10 * n as f64) as u64),
            cell(&[1, 0, 0], n, (0.10 * n as f64) as u64),
            cell(&[1, 1, 0], n, (0.40 * n as f64) as u64),
            // C = 1 (interaction reversed)
            cell(&[0, 0, 1], n, (0.40 * n as f64) as u64),
            cell(&[0, 1, 1], n, (0.40 * n as f64) as u64),
            cell(&[1, 0, 1], n, (0.40 * n as f64) as u64),
            cell(&[1, 1, 1], n, (0.10 * n as f64) as u64),
        ];
        let tw = pick(
            &binary_terms(&nd, 3),
            TermKind::ThreeWay { a: 0, b: 1, c: 2 },
        )
        .freq;
        assert!(!tw.insufficient_data, "should be testable");
        assert_eq!(tw.df, 1); // (2-1)(2-1)(2-1)
        assert!(tw.p_value < 0.05, "p={} chi2={}", tw.p_value, tw.statistic);
        assert!(tw.significant);
        assert!((tw.estimate - tw.statistic).abs() < 1e-12);
    }

    /// Purely-pairwise table (no genuine 3-way): build rates from a strictly
    /// additive-in-logits structure so the `[OAB][OAC][OBC]` model fits → NOT
    /// significant.
    #[test]
    fn three_way_no_interaction_is_not_significant() {
        // Construct cell success rates with main effects on A, B, C and pairwise
        // structure but NO 4-way term: rate depends on a+b+c only (additive on
        // the rate scale within [0,1]), which has no 3-way-with-outcome part.
        let n = 2000;
        let rate = |a: usize, b: usize, c: usize| {
            0.10 + 0.05 * a as f64 + 0.07 * b as f64 + 0.06 * c as f64
        };
        let mut nd = Vec::new();
        for a in 0..2 {
            for b in 0..2 {
                for c in 0..2 {
                    let s = (rate(a, b, c) * n as f64).round() as u64;
                    nd.push(cell(&[a, b, c], n, s));
                }
            }
        }
        let tw = pick(
            &binary_terms(&nd, 3),
            TermKind::ThreeWay { a: 0, b: 1, c: 2 },
        )
        .freq;
        assert!(!tw.insufficient_data);
        assert!(tw.p_value > 0.05, "p={} chi2={}", tw.p_value, tw.statistic);
        assert!(!tw.significant);
    }

    /// Three-way with a degenerate factor (only 1 level on C) → insufficient.
    #[test]
    fn three_way_degenerate_factor_is_insufficient() {
        let nd = [
            cell(&[0, 0, 0], 1000, 100),
            cell(&[0, 1, 0], 1000, 200),
            cell(&[1, 0, 0], 1000, 300),
            cell(&[1, 1, 0], 1000, 150),
        ];
        let tw = pick(
            &binary_terms(&nd, 3),
            TermKind::ThreeWay { a: 0, b: 1, c: 2 },
        )
        .freq;
        assert!(tw.insufficient_data);
        assert!(!tw.significant);
        assert!(tw.estimate.is_nan());
    }

    /// Three-way with an empty outcome margin (a cell never succeeds) →
    /// insufficient (unidentifiable fit).
    #[test]
    fn three_way_empty_margin_is_insufficient() {
        // (1,1,1) has zero successes AND every other cell at 100% → the failure
        // margin for several O×A×B combos is empty.
        let nd = [
            cell(&[0, 0, 0], 1000, 1000),
            cell(&[0, 1, 0], 1000, 1000),
            cell(&[1, 0, 0], 1000, 1000),
            cell(&[1, 1, 0], 1000, 1000),
            cell(&[0, 0, 1], 1000, 1000),
            cell(&[0, 1, 1], 1000, 1000),
            cell(&[1, 0, 1], 1000, 1000),
            cell(&[1, 1, 1], 1000, 0),
        ];
        let tw = pick(
            &binary_terms(&nd, 3),
            TermKind::ThreeWay { a: 0, b: 1, c: 2 },
        )
        .freq;
        assert!(tw.insufficient_data);
    }

    /// A malformed cell with `successes > n` must not underflow `u64` (panic in
    /// debug / wrap in release) when the three-way path computes failures. The
    /// success count is clamped to `n`, so the decomposition still runs and the
    /// three-way term resolves to a finite (non-panicking) result.
    #[test]
    fn three_way_successes_exceeding_n_does_not_underflow() {
        // (0,0,0) is malformed: successes (1500) > n (1000).
        let nd = [
            cell(&[0, 0, 0], 1000, 1500),
            cell(&[0, 1, 0], 1000, 400),
            cell(&[1, 0, 0], 1000, 300),
            cell(&[1, 1, 0], 1000, 150),
            cell(&[0, 0, 1], 1000, 600),
            cell(&[0, 1, 1], 1000, 500),
            cell(&[1, 0, 1], 1000, 350),
            cell(&[1, 1, 1], 1000, 200),
        ];
        // Must not panic; every term must carry a finite-or-NaN (never wrapped)
        // result.
        let terms = binary_terms(&nd, 3);
        assert_eq!(terms.len(), 7);
        let tw = pick(&terms, TermKind::ThreeWay { a: 0, b: 1, c: 2 }).freq;
        // Clamped to a well-formed table ⇒ testable, with a finite statistic.
        assert!(!tw.insufficient_data);
        assert!(tw.statistic.is_finite());
    }

    // ── insufficient-data propagation across the whole decomposition ──────────

    /// Tiny per-cell n → every term insufficient (NaN estimate, not
    /// significant).
    #[test]
    fn tiny_samples_make_every_term_insufficient() {
        let nd = [
            cell(&[0, 0], 2, 0),
            cell(&[0, 1], 2, 0),
            cell(&[1, 0], 2, 0),
            cell(&[1, 1], 2, 2),
        ];
        let terms = binary_terms(&nd, 2);
        assert_eq!(terms.len(), 3);
        for t in &terms {
            assert!(
                t.freq.insufficient_data,
                "{:?} should be insufficient",
                t.kind
            );
            assert!(!t.freq.significant);
            assert!(t.freq.estimate.is_nan());
            assert!(t.freq.p_value.is_nan());
        }
    }

    /// An empty grid yields the right *shape* with every term insufficient.
    #[test]
    fn empty_grid_yields_insufficient_terms() {
        let terms = binary_terms(&[], 2);
        assert_eq!(terms.len(), 3);
        for t in &terms {
            assert!(t.freq.insufficient_data);
        }
    }

    // ── determinism ──────────────────────────────────────────────────────────

    /// Same input twice → identical output (including the IPF three-way path).
    #[test]
    fn decomposition_is_deterministic() {
        let n = 1500;
        let nd = [
            cell(&[0, 0, 0], n, 150),
            cell(&[0, 1, 0], n, 150),
            cell(&[1, 0, 0], n, 150),
            cell(&[1, 1, 0], n, 600),
            cell(&[0, 0, 1], n, 600),
            cell(&[0, 1, 1], n, 600),
            cell(&[1, 0, 1], n, 600),
            cell(&[1, 1, 1], n, 150),
        ];
        let a = binary_terms(&nd, 3);
        let b = binary_terms(&nd, 3);
        assert_eq!(a, b);
    }
}
