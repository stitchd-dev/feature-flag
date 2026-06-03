//! (P2.T3) Frequentist **multi-factor ANOVA decomposition** for continuous
//! (revenue / duration / numeric) metrics.
//!
//! ## Contract (signature fixed by the seam; body implemented by the worker)
//!
//! `continuous_terms(cells, order)` returns one [`TermResult`] per term, with
//! `bayes: None`:
//! - main effects (`TermKind::Main`) — between-level F-tests
//! - pairwise interactions (`TermKind::TwoWay`)
//! - for `order == 3`, the three-way interaction (`TermKind::ThreeWay`)
//!
//! ## Method
//!
//! A single **common error term** is computed from the full k-factor cell
//! structure (per-cell within-variance pooled across every populated cell):
//! `SS_error = Σ_cells (Σx² − (Σx)²/n)`, `df_error = N − (#non-empty cells)`.
//! Every F-test below uses this same `MS_error = SS_error / df_error`
//! denominator, so the terms share one residual estimate (the standard balanced
//! factorial-ANOVA partition). If `df_error ≤ 0` (no within-cell replication) or
//! `SS_error ≤ 0` (zero residual variance), every term is
//! [`super::InteractionResult::insufficient`].
//!
//! - **Main{i}:** one-way between-level SS on factor `i`'s marginal level means,
//!   `SS_i = Σ_l n_l·(mean_l − grand_mean)²`, `df_i = L_i − 1`.
//! - **TwoWay{a,b}:** the additive-model interaction SS on the table collapsed
//!   onto the `(a, b)` grid,
//!   `SS_AB = Σ_cells n·(cell_mean − row_mean − col_mean + grand_mean)²` with
//!   `df_AB = (Lₐ−1)(L_b−1)`, tested against the *same* common error. At
//!   `order == 2` the collapsed `(a, b)` grid IS the full table and the common
//!   error IS [`super::continuous_interaction`]'s error, so the F / p / df are
//!   bit-for-bit identical to the regression baseline (gate P2.T7); at
//!   `order == 3` the numerator is the *marginal* 2-way interaction SS but the
//!   denominator is the shared pooled within-cell error (NOT the collapsed
//!   grid's own error), so every term in the partition shares one residual
//!   estimate.
//! - **ThreeWay{0,1,2}** (order 3): the residual interaction SS
//!   `SS₃ = SS_cells − SS_A − SS_B − SS_C − SS_AB − SS_BC − SS_AC`, with
//!   `df₃ = (Lₐ−1)(L_b−1)(L_c−1)`.
//!
//! `significant` is `p_value < super::ALPHA` and not insufficient. Distribution
//! tail from the parent: `super::fdist_sf`.

use super::{NdContinuousCell, TermKind, TermResult};

/// See module contract. Returns one [`TermResult`] per main effect, pairwise
/// interaction, and (for `order == 3`) the three-way interaction.
///
/// The full ANOVA SS partition is computed in essentially one pass: the common
/// pooled within-cell error and each term's between-cell SS are evaluated once
/// and shared. Every term — main, pairwise, three-way — is F-tested against the
/// *same* `MS_error`, so the partition is internally consistent (no term uses a
/// different residual estimate than another).
pub fn continuous_terms(cells: &[NdContinuousCell], order: usize) -> Vec<TermResult> {
    let mut out = Vec::with_capacity(if order == 3 { 7 } else { 3 });

    // The common error term shared by *every* F-test below.
    let err = ErrorTerm::from_cells(cells, order);

    // One grand mean over the full table, shared by every between-cell SS so the
    // order-3 residual `SS₃ = SS_cells − ΣSS_main − ΣSS_2way` is an exact
    // partition. At order 2 the (a,b) collapse equals the full table, so this is
    // the same grand mean the legacy 2-way routine uses (bit-for-bit gate P2.T7).
    let grand_mean = grand_mean(cells, order);

    // Main effects: one per factor. Each SS is computed once and reused by the
    // order-3 residual.
    let mut main_ss = [0.0f64; 3];
    for (i, slot) in main_ss.iter_mut().enumerate().take(order) {
        let (res, ss) = main_term(cells, i, &err);
        *slot = ss;
        out.push(res);
    }

    // Pairwise interactions: one per unordered pair a < b. Each marginal 2-way
    // interaction SS is computed once and reused by the order-3 residual.
    let mut two_way_ss_for = |a: usize, b: usize| -> f64 {
        let (res, ss) = two_way_term(cells, a, b, grand_mean, &err);
        out.push(res);
        ss
    };
    let mut ss_pairs = [0.0f64; 3]; // [AB, AC, BC]
    let mut p = 0usize;
    for a in 0..order {
        for b in (a + 1)..order {
            ss_pairs[p] = two_way_ss_for(a, b);
            p += 1;
        }
    }

    // Three-way interaction (order 3 only): the residual after removing every
    // main and pairwise term, reusing the SS already computed above.
    if order == 3 {
        // ss_pairs is filled in (a<b) iteration order: AB, AC, BC.
        out.push(three_way_term(
            cells,
            grand_mean,
            main_ss[0] + main_ss[1] + main_ss[2],
            ss_pairs[0] + ss_pairs[1] + ss_pairs[2],
            &err,
        ));
    }

    out
}

/// Trial-weighted grand mean over every populated `order`-arity cell, or `0.0`
/// when the table is empty (in which case all terms are insufficient anyway).
fn grand_mean(cells: &[NdContinuousCell], order: usize) -> f64 {
    let mut total_n = 0.0f64;
    let mut grand_sum = 0.0f64;
    for c in cells {
        if c.levels.len() != order || c.n == 0 {
            continue;
        }
        total_n += c.n as f64;
        grand_sum += c.sum;
    }
    if total_n <= 0.0 {
        return 0.0;
    }
    grand_sum / total_n
}

// ── Shared error term ────────────────────────────────────────────────────────

/// The pooled within-cell error term, computed once from the full k-factor
/// table and reused by *every* F-test (main, all pairwise, three-way). `valid`
/// is false when there is no replication (`df ≤ 0`) or no residual variance
/// (`ss ≤ 0`), in which case dependent terms are insufficient.
struct ErrorTerm {
    ss: f64,
    df: f64,
    valid: bool,
}

impl ErrorTerm {
    fn from_cells(cells: &[NdContinuousCell], order: usize) -> Self {
        let mut ss = 0.0f64;
        let mut total_n = 0.0f64;
        let mut non_empty = 0u64;
        for c in cells {
            // Cells with the wrong arity or no observations contribute nothing
            // to the pooled within-variance and are not counted as a fitted
            // cell mean (so they don't consume an error df).
            if c.levels.len() != order || c.n == 0 {
                continue;
            }
            let n = c.n as f64;
            ss += c.sum_sq - c.sum * c.sum / n;
            total_n += n;
            non_empty += 1;
        }
        // Floating-point cancellation can produce a tiny negative SS.
        if ss < 0.0 {
            ss = 0.0;
        }
        let df = total_n - non_empty as f64;
        let valid = df > 0.0 && ss > 0.0;
        ErrorTerm { ss, df, valid }
    }

    /// Mean square of the error term (only meaningful when `valid`).
    #[inline]
    fn ms(&self) -> f64 {
        self.ss / self.df
    }
}

// ── Main effect ──────────────────────────────────────────────────────────────

/// One-way between-level F-test on factor `factor`'s trial-weighted marginal
/// level means, tested against the common error term. Returns the result plus
/// the between-level `SS` (reused by the order-3 residual; `0.0` when the term
/// is structurally degenerate).
fn main_term(cells: &[NdContinuousCell], factor: usize, err: &ErrorTerm) -> (TermResult, f64) {
    let kind = TermKind::Main { factor };

    // Collapse onto the single factor: marginal (n, sum) per level.
    let levels = match marginal_1d(cells, factor) {
        Some(m) => m,
        None => return (term(kind, super::InteractionResult::insufficient(0)), 0.0),
    };
    let n_levels = levels.len();
    let df_factor = (n_levels as u32).saturating_sub(1);

    if n_levels < 2 {
        return (
            term(kind, super::InteractionResult::insufficient(df_factor)),
            0.0,
        );
    }

    let total_n: f64 = levels.iter().map(|(n, _)| n).sum();
    let grand_sum: f64 = levels.iter().map(|(_, s)| s).sum();
    if total_n <= 0.0 {
        return (
            term(kind, super::InteractionResult::insufficient(df_factor)),
            0.0,
        );
    }
    let grand_mean = grand_sum / total_n;

    // SS_factor = Σ_l n_l · (mean_l − grand_mean)².
    let mut ss_factor = 0.0f64;
    for &(n_l, sum_l) in &levels {
        if n_l <= 0.0 {
            continue;
        }
        let dev = sum_l / n_l - grand_mean;
        ss_factor += n_l * dev * dev;
    }
    if ss_factor < 0.0 {
        ss_factor = 0.0;
    }

    (f_test(kind, ss_factor, df_factor, err), ss_factor)
}

// ── Pairwise interaction ───────────────────────────────────────────────────

/// Pairwise interaction on factors `a < b`: the additive-model interaction SS on
/// the table collapsed onto the `(a, b)` grid, F-tested against the *common*
/// error term (NOT the collapsed grid's own residual).
///
/// Returns the result plus the marginal 2-way interaction `SS` (reused by the
/// order-3 residual; `0.0` when the term is degenerate / abstains).
///
/// The numerator (`SS_AB`, `df_AB`, grand/row/col means) is computed exactly as
/// [`super::continuous_interaction`] computes its interaction SS, so at order 2 —
/// where the `(a, b)` grid IS the whole table and the common error IS that
/// routine's pooled error — the F / p / df are bit-for-bit identical to the
/// legacy two-way test (regression gate P2.T7). At order 3 only the denominator
/// differs: the shared pooled within-cell error replaces the marginal grid's
/// own error, so the term participates in one consistent partition.
fn two_way_term(
    cells: &[NdContinuousCell],
    a: usize,
    b: usize,
    grand_mean: f64,
    err: &ErrorTerm,
) -> (TermResult, f64) {
    let kind = TermKind::TwoWay { a, b };

    // Collapse onto the (a, b) grid: dense `(n, sum)` per (a-level, b-level).
    let mut rows = 0usize;
    let mut cols = 0usize;
    for c in cells {
        if c.levels.len() <= a.max(b) || c.n == 0 {
            continue;
        }
        rows = rows.max(c.levels[a] + 1);
        cols = cols.max(c.levels[b] + 1);
    }
    // Mirror the legacy guard: with <2 levels on either factor there is no
    // interaction df (legacy returns `insufficient(0)`).
    if rows < 2 || cols < 2 {
        return (term(kind, super::InteractionResult::insufficient(0)), 0.0);
    }
    let df_inter = ((rows - 1) * (cols - 1)) as u32;

    let mut cell_n = vec![vec![0.0f64; cols]; rows];
    let mut cell_sum = vec![vec![0.0f64; cols]; rows];
    for c in cells {
        if c.levels.len() <= a.max(b) || c.n == 0 {
            continue;
        }
        cell_n[c.levels[a]][c.levels[b]] += c.n as f64;
        cell_sum[c.levels[a]][c.levels[b]] += c.sum;
    }
    // An empty cell makes the additive decomposition unidentifiable — legacy
    // abstains here, so we do too (preserving the bit-for-bit gate at order 2).
    if cell_n.iter().flatten().any(|&x| x <= 0.0) {
        return (
            term(kind, super::InteractionResult::insufficient(df_inter)),
            0.0,
        );
    }

    // Row / column marginals on the collapsed grid (means precomputed, matching
    // `continuous_interaction`'s evaluation order for bit-for-bit equivalence).
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

    // SS_AB = Σ_cells n · (cell_mean − row_mean − col_mean + grand_mean)².
    let mut ss_inter = 0.0f64;
    for r in 0..rows {
        for c in 0..cols {
            let cell_mean = cell_sum[r][c] / cell_n[r][c];
            let resid = cell_mean - row_mean[r] - col_mean[c] + grand_mean;
            ss_inter += cell_n[r][c] * resid * resid;
        }
    }
    if ss_inter < 0.0 {
        ss_inter = 0.0;
    }

    (f_test(kind, ss_inter, df_inter, err), ss_inter)
}

// ── Three-way interaction ────────────────────────────────────────────────────

/// Full three-way interaction F-test (order 3 only): the residual cell-mean
/// variation after removing all main effects and pairwise interactions, tested
/// against the common error term.
///
/// `sum_main_ss` / `sum_pair_ss` are the already-computed `ΣSS_main` and
/// `ΣSS_2way` from this same partition (passed in to avoid recomputing them),
/// and `grand_mean` is the shared full-table grand mean. The residual is
/// `SS₃ = SS_cells − ΣSS_main − ΣSS_2way`.
fn three_way_term(
    cells: &[NdContinuousCell],
    grand_mean: f64,
    sum_main_ss: f64,
    sum_pair_ss: f64,
    err: &ErrorTerm,
) -> TermResult {
    let kind = TermKind::ThreeWay { a: 0, b: 1, c: 2 };

    // Level counts per factor (max index + 1), arity-checked to 3.
    let mut maxlvl = [0usize; 3];
    let mut total_n = 0.0f64;
    for cell in cells {
        if cell.levels.len() != 3 {
            continue;
        }
        for (d, m) in maxlvl.iter_mut().enumerate() {
            *m = (*m).max(cell.levels[d]);
        }
        if cell.n > 0 {
            total_n += cell.n as f64;
        }
    }
    let l = [maxlvl[0] + 1, maxlvl[1] + 1, maxlvl[2] + 1];
    let df3 = ((l[0] - 1) * (l[1] - 1) * (l[2] - 1)) as u32;

    // Need ≥2 levels on every factor and a valid error term.
    if l.iter().any(|&x| x < 2) || !err.valid || total_n <= 0.0 {
        return term(kind, super::InteractionResult::insufficient(df3));
    }

    // Dense 3-D cell accumulator. An empty cell makes the full-factorial mean
    // decomposition unidentifiable, so we abstain if any is missing.
    let mut cell_n = vec![0.0f64; l[0] * l[1] * l[2]];
    let mut cell_sum = vec![0.0f64; l[0] * l[1] * l[2]];
    let idx = |i: usize, j: usize, k: usize| (i * l[1] + j) * l[2] + k;
    for cell in cells {
        if cell.levels.len() != 3 || cell.n == 0 {
            continue;
        }
        let p = idx(cell.levels[0], cell.levels[1], cell.levels[2]);
        cell_n[p] += cell.n as f64;
        cell_sum[p] += cell.sum;
    }
    if cell_n.iter().any(|&x| x <= 0.0) {
        return term(kind, super::InteractionResult::insufficient(df3));
    }

    // SS_cells = Σ_cells n · (cell_mean − grand_mean)².
    let mut ss_cells = 0.0f64;
    for p in 0..cell_n.len() {
        let dev = cell_sum[p] / cell_n[p] - grand_mean;
        ss_cells += cell_n[p] * dev * dev;
    }

    // Residual three-way SS = SS_cells − ΣSS_main − ΣSS_2way (reusing the SS
    // already computed for the main and pairwise terms of this partition).
    let mut ss3 = ss_cells - sum_main_ss - sum_pair_ss;
    // Clamp tiny negative SS from floating-point cancellation.
    if ss3 < 0.0 {
        ss3 = 0.0;
    }

    f_test(kind, ss3, df3, err)
}

// ── SS helpers ───────────────────────────────────────────────────────────────

/// Trial-weighted marginal `(n, sum)` per level on `factor`. Returns `None`
/// when no populated cell carries that factor.
fn marginal_1d(cells: &[NdContinuousCell], factor: usize) -> Option<Vec<(f64, f64)>> {
    let max_level = cells
        .iter()
        .filter(|c| c.levels.len() > factor && c.n > 0)
        .map(|c| c.levels[factor])
        .max()?;
    let mut levels = vec![(0.0f64, 0.0f64); max_level + 1];
    for c in cells {
        if c.levels.len() <= factor || c.n == 0 {
            continue;
        }
        let slot = &mut levels[c.levels[factor]];
        slot.0 += c.n as f64;
        slot.1 += c.sum;
    }
    Some(levels)
}

// ── Result construction ──────────────────────────────────────────────────────

/// Build an F-test [`TermResult`] for `kind` from a term SS / df against the
/// common error term. Abstains (insufficient) on zero term-df or a non-finite F.
fn f_test(kind: TermKind, ss_term: f64, df_term: u32, err: &ErrorTerm) -> TermResult {
    if df_term == 0 || !err.valid {
        return term(kind, super::InteractionResult::insufficient(df_term));
    }
    let ms_term = ss_term / df_term as f64;
    let f = ms_term / err.ms();
    if !f.is_finite() {
        return term(kind, super::InteractionResult::insufficient(df_term));
    }
    let p_value = super::fdist_sf(f, df_term as f64, err.df);
    term(
        kind,
        super::InteractionResult {
            estimate: f,
            statistic: f,
            p_value,
            df: df_term,
            significant: p_value < super::ALPHA,
            insufficient_data: false,
        },
    )
}

/// Wrap a frequentist result as a Bayesian-free [`TermResult`].
#[inline]
fn term(kind: TermKind, freq: super::InteractionResult) -> TermResult {
    TermResult {
        kind,
        freq,
        bayes: None,
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experimentation::stats::interaction::ContinuousCell;

    // ── cell builders ────────────────────────────────────────────────────────

    /// An n-factor continuous cell from explicit values (n / Σx / Σx²).
    fn cell_vals(levels: &[usize], vals: &[f64]) -> NdContinuousCell {
        let n = vals.len() as u64;
        let sum: f64 = vals.iter().sum();
        let sum_sq: f64 = vals.iter().map(|v| v * v).sum();
        NdContinuousCell {
            levels: levels.to_vec(),
            n,
            sum,
            sum_sq,
        }
    }

    /// A cell whose values are centred on `mean` with realistic, non-trivial
    /// within-cell spread (a fixed symmetric ±spread pattern → deterministic,
    /// non-zero variance), repeated `n` times. `n >= 40` per the contract.
    fn cell_mean(levels: &[usize], mean: f64, n: u64) -> NdContinuousCell {
        assert!(n >= 2);
        // Repeating ±s/±2s pattern: mean preserved (sums to ~0 over a period of
        // 4), variance ≈ 2.5·s². Deterministic so tests are reproducible.
        let s = 3.0;
        let pattern = [s, -s, 2.0 * s, -2.0 * s];
        let mut vals = Vec::with_capacity(n as usize);
        for i in 0..n {
            vals.push(mean + pattern[(i as usize) % 4]);
        }
        cell_vals(levels, &vals)
    }

    /// The legacy 2-way cell builder, mirroring the planted means used by the
    /// regression baseline in `interaction.rs`.
    fn ccell_mean_2d(a: usize, b: usize, mean: f64, n: u64) -> ContinuousCell {
        let c = cell_mean(&[a, b], mean, n);
        ContinuousCell {
            a_level: a,
            b_level: b,
            n: c.n,
            sum: c.sum,
            sum_sq: c.sum_sq,
        }
    }

    fn find(terms: &[TermResult], kind: TermKind) -> &TermResult {
        terms
            .iter()
            .find(|t| t.kind == kind)
            .unwrap_or_else(|| panic!("missing term {kind:?}"))
    }

    // ── order-2 TwoWay reproduces the legacy fn exactly ──────────────────────

    /// Planted 2-way interaction: means (0,0)=(0,1)=(1,0)=10, (1,1)=30. The
    /// order-2 `TwoWay` term must equal `continuous_interaction` bit-for-bit
    /// (regression gate P2.T7) and be significant.
    #[test]
    fn order2_twoway_matches_legacy_on_planted_interaction() {
        let nd = [
            cell_mean(&[0, 0], 10.0, 60),
            cell_mean(&[0, 1], 10.0, 60),
            cell_mean(&[1, 0], 10.0, 60),
            cell_mean(&[1, 1], 30.0, 60),
        ];
        let legacy_cells = [
            ccell_mean_2d(0, 0, 10.0, 60),
            ccell_mean_2d(0, 1, 10.0, 60),
            ccell_mean_2d(1, 0, 10.0, 60),
            ccell_mean_2d(1, 1, 30.0, 60),
        ];

        let terms = continuous_terms(&nd, 2);
        let two = find(&terms, TermKind::TwoWay { a: 0, b: 1 });
        let legacy = super::super::continuous_interaction(&legacy_cells);

        assert_eq!(two.freq.estimate, legacy.estimate);
        assert_eq!(two.freq.p_value, legacy.p_value);
        assert_eq!(two.freq.df, legacy.df);
        assert_eq!(two.freq.statistic, legacy.statistic);
        assert_eq!(two.freq.insufficient_data, legacy.insufficient_data);
        assert!(two.bayes.is_none());
        assert!(!two.freq.insufficient_data);
        assert!(two.freq.significant, "planted interaction should be sig");
    }

    /// Purely additive means (10, 15, 20, 25): the order-2 `TwoWay` term must
    /// equal the legacy fn exactly and be NOT significant (no false positive).
    #[test]
    fn order2_twoway_matches_legacy_on_additive() {
        let nd = [
            cell_mean(&[0, 0], 10.0, 80),
            cell_mean(&[0, 1], 15.0, 80),
            cell_mean(&[1, 0], 20.0, 80),
            cell_mean(&[1, 1], 25.0, 80),
        ];
        let legacy_cells = [
            ccell_mean_2d(0, 0, 10.0, 80),
            ccell_mean_2d(0, 1, 15.0, 80),
            ccell_mean_2d(1, 0, 20.0, 80),
            ccell_mean_2d(1, 1, 25.0, 80),
        ];

        let terms = continuous_terms(&nd, 2);
        let two = find(&terms, TermKind::TwoWay { a: 0, b: 1 });
        let legacy = super::super::continuous_interaction(&legacy_cells);

        assert_eq!(two.freq.estimate, legacy.estimate);
        assert_eq!(two.freq.p_value, legacy.p_value);
        assert_eq!(two.freq.df, legacy.df);
        assert_eq!(two.freq.statistic, legacy.statistic);
        assert!(!two.freq.insufficient_data);
        assert!(
            !two.freq.significant,
            "additive design must not be significant"
        );
    }

    /// Order 2 emits exactly: Main{0}, Main{1}, TwoWay{0,1} (no three-way).
    #[test]
    fn order2_emits_expected_term_set() {
        let nd = [
            cell_mean(&[0, 0], 10.0, 50),
            cell_mean(&[0, 1], 12.0, 50),
            cell_mean(&[1, 0], 14.0, 50),
            cell_mean(&[1, 1], 16.0, 50),
        ];
        let terms = continuous_terms(&nd, 2);
        let kinds: Vec<TermKind> = terms.iter().map(|t| t.kind).collect();
        assert_eq!(kinds.len(), 3);
        assert!(kinds.contains(&TermKind::Main { factor: 0 }));
        assert!(kinds.contains(&TermKind::Main { factor: 1 }));
        assert!(kinds.contains(&TermKind::TwoWay { a: 0, b: 1 }));
        assert!(terms.iter().all(|t| t.bayes.is_none()));
    }

    // ── order-3 three-way interaction ────────────────────────────────────────

    /// Order 3 emits exactly 3 mains + 3 pairwise + 1 three-way = 7 terms.
    #[test]
    fn order3_emits_seven_terms() {
        let nd = full_2x2x2(|_, _, _| 10.0, 50);
        let terms = continuous_terms(&nd, 3);
        assert_eq!(terms.len(), 7);
        for k in [
            TermKind::Main { factor: 0 },
            TermKind::Main { factor: 1 },
            TermKind::Main { factor: 2 },
            TermKind::TwoWay { a: 0, b: 1 },
            TermKind::TwoWay { a: 0, b: 2 },
            TermKind::TwoWay { a: 1, b: 2 },
            TermKind::ThreeWay { a: 0, b: 1, c: 2 },
        ] {
            assert!(terms.iter().any(|t| t.kind == k), "missing {k:?}");
        }
        assert!(terms.iter().all(|t| t.bayes.is_none()));
    }

    /// Build a full 2×2×2 design; cell mean from the closure, fixed `n` & spread.
    fn full_2x2x2(mean: impl Fn(usize, usize, usize) -> f64, n: u64) -> Vec<NdContinuousCell> {
        let mut v = Vec::with_capacity(8);
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..2 {
                    v.push(cell_mean(&[i, j, k], mean(i, j, k), n));
                }
            }
        }
        v
    }

    /// Planted 3-way interaction is significant. The mean model is purely the
    /// triple product `8·(i·j·k)`: all main effects, AB/AC/BC interactions are
    /// zero in expectation, leaving only the 3-way term.
    #[test]
    fn order3_planted_threeway_is_significant() {
        let nd = full_2x2x2(|i, j, k| 10.0 + 8.0 * (i * j * k) as f64, 60);
        let terms = continuous_terms(&nd, 3);
        let three = find(&terms, TermKind::ThreeWay { a: 0, b: 1, c: 2 });
        assert!(!three.freq.insufficient_data);
        assert_eq!(three.freq.df, 1); // (2-1)^3
        assert!(
            three.freq.significant,
            "planted 3-way should be significant: p={} F={}",
            three.freq.p_value, three.freq.statistic
        );
    }

    /// No three-way: a fully additive + pairwise mean model (main + AB + AC + BC
    /// terms, but the triple residual is zero) is NOT significant on the
    /// three-way term.
    #[test]
    fn order3_no_threeway_is_not_significant() {
        // mean = base + a-main + b-main + c-main + AB + AC + BC, no ABC.
        // Encode each factor as 0/1; pairwise terms are products of two factors.
        let model = |i: usize, j: usize, k: usize| {
            let (i, j, k) = (i as f64, j as f64, k as f64);
            10.0 + 2.0 * i + 3.0 * j + 4.0 * k // mains
                + 5.0 * i * j + 6.0 * i * k + 7.0 * j * k // pairwise
        };
        let nd = full_2x2x2(model, 80);
        let terms = continuous_terms(&nd, 3);
        let three = find(&terms, TermKind::ThreeWay { a: 0, b: 1, c: 2 });
        assert!(!three.freq.insufficient_data);
        assert!(
            !three.freq.significant,
            "additive+pairwise model has no 3-way: p={} F={}",
            three.freq.p_value, three.freq.statistic
        );
    }

    /// At order 3, the `TwoWay{0,1}` term's *numerator* is the marginal 2-way
    /// interaction SS — identical to what `continuous_interaction` computes on
    /// the table collapsed over factor 2 — but its *denominator* is the shared
    /// pooled within-cell error from the full table, NOT the collapsed grid's
    /// own (interaction-contaminated) error. This is exactly the partition fix:
    /// every term shares one `MS_error`. So the df matches the marginal
    /// interaction, and `F · MS_error` recovers the same `SS_inter/df` from both
    /// routines (proving identical numerator, different denominator).
    #[test]
    fn order3_twoway_uses_marginal_ss_with_common_error() {
        // Planted 3-way (only the (1,1,1) corner bumped) makes the collapsed
        // (a,b) grid's error differ from the full-grid common error: the (1,1)
        // collapsed cell pools two unequal-mean c-levels, inflating its
        // within-cell SS with the 3-way signal. The full-grid common error does
        // not. So legacy-on-collapsed and the new TwoWay use different
        // denominators by construction.
        let nd = full_2x2x2(|i, j, k| 10.0 + 8.0 * (i * j * k) as f64, 60);
        let terms = continuous_terms(&nd, 3);
        let two = find(&terms, TermKind::TwoWay { a: 0, b: 1 });

        // Collapse over factor 2 by hand and call the legacy routine.
        let mut collapsed: Vec<ContinuousCell> = Vec::new();
        for c in &nd {
            let (a, b) = (c.levels[0], c.levels[1]);
            if let Some(s) = collapsed
                .iter_mut()
                .find(|cc| cc.a_level == a && cc.b_level == b)
            {
                s.n += c.n;
                s.sum += c.sum;
                s.sum_sq += c.sum_sq;
            } else {
                collapsed.push(ContinuousCell {
                    a_level: a,
                    b_level: b,
                    n: c.n,
                    sum: c.sum,
                    sum_sq: c.sum_sq,
                });
            }
        }
        let legacy = super::super::continuous_interaction(&collapsed);

        // The interaction df (marginal 2-way) is unchanged.
        assert_eq!(two.freq.df, legacy.df);
        assert!(!two.freq.insufficient_data && !legacy.insufficient_data);

        // Pooled within-cell error MS over the FULL table (what every term in
        // the order-3 partition is tested against).
        let ms_err_common = {
            let mut ss = 0.0;
            let mut n = 0.0;
            let mut cells = 0.0;
            for c in &nd {
                ss += c.sum_sq - c.sum * c.sum / c.n as f64;
                n += c.n as f64;
                cells += 1.0;
            }
            ss / (n - cells)
        };
        // Pooled within-cell error MS over the COLLAPSED (a,b) grid (what the
        // legacy routine uses — contaminated by the 3-way signal here).
        let ms_err_collapsed = {
            let mut ss = 0.0;
            let mut n = 0.0;
            let mut cells = 0.0;
            for c in &collapsed {
                ss += c.sum_sq - c.sum * c.sum / c.n as f64;
                n += c.n as f64;
                cells += 1.0;
            }
            ss / (n - cells)
        };
        // The denominators genuinely differ (the fix matters on this data).
        assert!(
            (ms_err_common - ms_err_collapsed).abs() > 1e-6,
            "test is only meaningful when the two errors differ: common={ms_err_common} collapsed={ms_err_collapsed}"
        );

        // F · MS_error == SS_inter / df for each routine; the numerator SS_inter
        // is the marginal interaction SS in both, so these must coincide.
        let ssdf_two = two.freq.statistic * ms_err_common;
        let ssdf_legacy = legacy.statistic * ms_err_collapsed;
        assert!(
            (ssdf_two - ssdf_legacy).abs() <= 1e-6 * ssdf_legacy.abs().max(1.0),
            "marginal interaction SS must match: two={ssdf_two} legacy={ssdf_legacy}"
        );

        // And the new term's F is exactly its marginal SS over the common error.
        let expected_f = ssdf_legacy / ms_err_common;
        assert!(
            (two.freq.statistic - expected_f).abs() <= 1e-6 * expected_f.abs().max(1.0),
            "TwoWay F must use the common error: got={} expected={}",
            two.freq.statistic,
            expected_f
        );
    }

    // ── main effects ─────────────────────────────────────────────────────────

    /// A factor whose marginal means differ strongly is significant; a flat
    /// factor (identical marginal means) is not.
    #[test]
    fn main_effect_varying_vs_flat() {
        // Factor 0 varies (level 0 ≈ 10, level 1 ≈ 40); factor 1 is flat across
        // its levels (marginal means equal). Strong within-row replication.
        let nd = [
            cell_mean(&[0, 0], 10.0, 80),
            cell_mean(&[0, 1], 10.0, 80),
            cell_mean(&[1, 0], 40.0, 80),
            cell_mean(&[1, 1], 40.0, 80),
        ];
        let terms = continuous_terms(&nd, 2);

        let m0 = find(&terms, TermKind::Main { factor: 0 });
        assert!(!m0.freq.insufficient_data);
        assert_eq!(m0.freq.df, 1);
        assert!(
            m0.freq.significant,
            "varying factor 0 should be significant: p={}",
            m0.freq.p_value
        );

        let m1 = find(&terms, TermKind::Main { factor: 1 });
        assert!(!m1.freq.insufficient_data);
        assert!(
            !m1.freq.significant,
            "flat factor 1 should not be significant: p={}",
            m1.freq.p_value
        );
        // A flat factor's between-level SS is ~0 → F ~0 → p ~1.
        assert!(
            m1.freq.estimate.abs() < 1e-6,
            "F should be ≈0 for flat factor"
        );
    }

    /// A three-level factor yields df = 2 on its main effect.
    #[test]
    fn main_effect_three_levels_df_is_two() {
        let nd = [
            cell_mean(&[0, 0], 10.0, 50),
            cell_mean(&[0, 1], 12.0, 50),
            cell_mean(&[1, 0], 20.0, 50),
            cell_mean(&[1, 1], 22.0, 50),
            cell_mean(&[2, 0], 30.0, 50),
            cell_mean(&[2, 1], 32.0, 50),
        ];
        let terms = continuous_terms(&nd, 2);
        let m0 = find(&terms, TermKind::Main { factor: 0 });
        assert_eq!(m0.freq.df, 2);
        assert!(m0.freq.significant);
    }

    // ── insufficient-data paths ──────────────────────────────────────────────

    /// One observation per cell → df_error ≤ 0 → every term insufficient.
    #[test]
    fn no_replication_all_terms_insufficient() {
        let nd = [
            cell_vals(&[0, 0], &[10.0]),
            cell_vals(&[0, 1], &[12.0]),
            cell_vals(&[1, 0], &[14.0]),
            cell_vals(&[1, 1], &[20.0]),
        ];
        let terms = continuous_terms(&nd, 2);
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

    /// Zero within-cell variance → SS_error = 0 → every term insufficient.
    #[test]
    fn zero_within_variance_all_terms_insufficient() {
        let nd = [
            cell_vals(&[0, 0], &[10.0, 10.0, 10.0, 10.0]),
            cell_vals(&[0, 1], &[12.0, 12.0, 12.0, 12.0]),
            cell_vals(&[1, 0], &[14.0, 14.0, 14.0, 14.0]),
            cell_vals(&[1, 1], &[25.0, 25.0, 25.0, 25.0]),
        ];
        let terms = continuous_terms(&nd, 2);
        for t in &terms {
            assert!(
                t.freq.insufficient_data,
                "{:?} should be insufficient",
                t.kind
            );
        }
    }

    /// An empty cell makes the 3-way term unidentifiable → that term is
    /// insufficient (the marginal lower-order terms can still resolve).
    #[test]
    fn order3_empty_cell_threeway_insufficient() {
        // Full 2×2×2 minus the (1,1,1) cell.
        let mut nd = Vec::new();
        for i in 0..2 {
            for j in 0..2 {
                for k in 0..2 {
                    if (i, j, k) == (1, 1, 1) {
                        continue;
                    }
                    nd.push(cell_mean(&[i, j, k], 10.0 + (i + j + k) as f64, 50));
                }
            }
        }
        let terms = continuous_terms(&nd, 3);
        let three = find(&terms, TermKind::ThreeWay { a: 0, b: 1, c: 2 });
        assert!(three.freq.insufficient_data);
        assert!(!three.freq.significant);
    }

    /// A single level on a factor → that main effect (and any interaction using
    /// it) is insufficient (no contrast / no interaction df).
    #[test]
    fn single_level_factor_main_is_insufficient() {
        // Factor 0 has only level 0; factor 1 has two levels.
        let nd = [cell_mean(&[0, 0], 10.0, 60), cell_mean(&[0, 1], 20.0, 60)];
        let terms = continuous_terms(&nd, 2);
        let m0 = find(&terms, TermKind::Main { factor: 0 });
        assert!(m0.freq.insufficient_data, "single-level factor 0");
        // The 2-way term has a 1-level factor → legacy routine returns insufficient.
        let two = find(&terms, TermKind::TwoWay { a: 0, b: 1 });
        assert!(two.freq.insufficient_data);
    }

    // ── determinism ──────────────────────────────────────────────────────────

    #[test]
    fn is_deterministic() {
        let nd = full_2x2x2(|i, j, k| 10.0 + 8.0 * (i * j * k) as f64, 60);
        let a = continuous_terms(&nd, 3);
        let b = continuous_terms(&nd, 3);
        assert_eq!(a, b);
    }
}
