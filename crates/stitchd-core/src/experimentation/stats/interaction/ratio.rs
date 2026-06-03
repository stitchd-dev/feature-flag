//! (P2.T4) Frequentist **ratio-metric interaction** via the delta method.
//!
//! ## Contract (signature fixed by the seam; body implemented by the worker)
//!
//! `ratio_terms(cells, order)` returns one [`TermResult`] per term
//! (`Main` / `TwoWay` / `ThreeWay`), with `bayes: None`.
//!
//! A ratio metric's per-cell point estimate is `R = num_sum / den_sum`. The
//! delta method gives `Var(R) ≈ (1/den_mean²)·(Var(num) − 2·R·Cov(num,den) +
//! R²·Var(den)) / n`, with `Var`/`Cov` recovered from the cell's
//! `num_sq_sum` / `den_sq_sum` / `num_den_sum` second moments. Form the
//! interaction contrast on the cell ratios (difference-in-differences for 2×2,
//! generalised for higher dimensions), divide by the pooled delta-method SE,
//! and take a two-sided normal-tail p-value (`super::z_to_p`). Cells whose
//! denominator falls below the metric's `min_denominator` (the caller zeroes /
//! drops them before calling, but guard anyway), empty cells, or non-finite SE
//! → [`super::InteractionResult::insufficient`].
//!
//! ## Methods
//!
//! Each per-cell ratio `R` carries a delta-method variance `Var(R)` and hence an
//! inverse-variance weight `w = 1/Var(R)`. From those building blocks:
//!
//! - **Main { factor }** — collapse onto that one factor and run a fixed-effect
//!   homogeneity (Cochran's `Q`) test of the level ratios against their
//!   inverse-variance-weighted pooled ratio. `Q ~ χ²` on `df = L − 1`.
//! - **TwoWay { a, b }** — collapse onto the `(a, b)` grid. For a `2×2` grid the
//!   difference-in-differences contrast `δ = (R11 − R10) − (R01 − R00)` with a
//!   pooled delta-method SE gives an exact `z`-test (`df = 1`). For a general
//!   `La×Lb` grid, fit the additive model `R ≈ μ + αₐ + β_b` by inverse-variance
//!   weighted least squares and take the residual quadratic form
//!   `Q = Σ w·resid²` on `df = (La−1)(Lb−1)`.
//! - **ThreeWay { a, b, c }** (order 3) — collapse onto the `(a, b, c)` grid. For
//!   `2×2×2` the difference-in-differences-of-differences `z`-contrast over the
//!   eight corners (`df = 1`); otherwise the residual `Q` of the
//!   additive-plus-all-pairwise model, `df = (La−1)(Lb−1)(Lc−1)`.
//!
//! Any collapsed cell with `den_sum ≤ 0`, `n < 2`, or a non-finite / non-positive
//! delta-method variance makes the relevant term
//! [`super::InteractionResult::insufficient`].

use super::{InteractionResult, NdRatioCell, TermKind, TermResult};

/// `x > 0` as a finite, strictly-positive test (also rejects `NaN`). Wrapped in
/// a function so the negated guards below stay readable without tripping
/// clippy's `neg_cmp_op_on_partial_ord` on the bare `!(x > 0.0)` form.
#[inline]
fn positive(x: f64) -> bool {
    x > 0.0
}

/// Aggregated ratio sufficient statistics for a (collapsed) cell.
///
/// A collapse simply sums every field across the factors being marginalised
/// out, so the same struct represents a raw cell and any marginal of it.
#[derive(Debug, Clone, Copy, Default)]
struct RatioAgg {
    n: u64,
    num_sum: f64,
    den_sum: f64,
    num_sq_sum: f64,
    den_sq_sum: f64,
    num_den_sum: f64,
}

impl RatioAgg {
    /// Fold another cell's sufficient statistics into this aggregate.
    fn add(&mut self, c: &NdRatioCell) {
        self.n = self.n.saturating_add(c.n);
        self.num_sum += c.num_sum;
        self.den_sum += c.den_sum;
        self.num_sq_sum += c.num_sq_sum;
        self.den_sq_sum += c.den_sq_sum;
        self.num_den_sum += c.num_den_sum;
    }

    /// Delta-method point estimate `R` and variance `Var(R)` for this cell.
    ///
    /// Returns `None` when the cell is degenerate (`den_sum ≤ 0`,
    /// `mean_den ≤ 0`, `n < 2`, or a non-finite / non-positive variance), in
    /// which case the enclosing term is reported insufficient.
    fn ratio_var(&self) -> Option<(f64, f64)> {
        let n = self.n as f64;
        if self.n < 2 || self.den_sum <= 0.0 {
            return None;
        }
        let mean_num = self.num_sum / n;
        let mean_den = self.den_sum / n;
        if !positive(mean_den) {
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

/// Public entry point — see the module contract.
pub fn ratio_terms(cells: &[NdRatioCell], order: usize) -> Vec<TermResult> {
    if cells.is_empty() || order < 2 {
        return Vec::new();
    }

    let mut terms = Vec::new();

    // Main effects: one per participating factor.
    for f in 0..order {
        terms.push(TermResult {
            kind: TermKind::Main { factor: f },
            freq: main_effect(cells, f),
            bayes: None,
        });
    }

    // Pairwise interactions: every unordered pair of factors.
    for a in 0..order {
        for b in (a + 1)..order {
            terms.push(TermResult {
                kind: TermKind::TwoWay { a, b },
                freq: two_way(cells, a, b),
                bayes: None,
            });
        }
    }

    // Three-way interaction (only when three experiments participate).
    if order >= 3 {
        terms.push(TermResult {
            kind: TermKind::ThreeWay { a: 0, b: 1, c: 2 },
            freq: three_way(cells, 0, 1, 2),
            bayes: None,
        });
    }

    terms
}

// ── Collapsing onto a sub-grid ───────────────────────────────────────────────

/// Collapse `cells` onto the chosen `factors` (in the given order), summing the
/// sufficient statistics across all other factors. Returns the dense per-factor
/// level counts and a dense grid (row-major over `factors`) of aggregates; a
/// grid position absent from the input is left at its `Default` (`n == 0`),
/// which `ratio_var` rejects as degenerate.
///
/// Returns `None` if any chosen factor has fewer than two observed levels (no
/// contrast to test) or the grid would be empty.
fn collapse(cells: &[NdRatioCell], factors: &[usize]) -> Option<(Vec<usize>, Vec<RatioAgg>)> {
    // Determine the dense extent (max level + 1) of each retained factor.
    let mut dims = vec![0usize; factors.len()];
    for c in cells {
        for (slot, &f) in factors.iter().enumerate() {
            let lev = *c.levels.get(f)?;
            dims[slot] = dims[slot].max(lev + 1);
        }
    }
    if dims.iter().any(|&d| d < 2) {
        return None;
    }

    let total: usize = dims.iter().product();
    if total == 0 {
        return None;
    }
    let mut grid = vec![RatioAgg::default(); total];
    for c in cells {
        let mut idx = 0usize;
        for (slot, &f) in factors.iter().enumerate() {
            idx = idx * dims[slot] + c.levels[f];
        }
        grid[idx].add(c);
    }
    Some((dims, grid))
}

/// Row-major flat index of a multi-index `coords` within a grid of `dims`.
fn flat_index(coords: &[usize], dims: &[usize]) -> usize {
    let mut idx = 0usize;
    for (slot, &c) in coords.iter().enumerate() {
        idx = idx * dims[slot] + c;
    }
    idx
}

// ── Main effect (Cochran's Q homogeneity test) ───────────────────────────────

fn main_effect(cells: &[NdRatioCell], factor: usize) -> InteractionResult {
    let Some((dims, grid)) = collapse(cells, &[factor]) else {
        return InteractionResult::insufficient(0);
    };
    let levels = dims[0];
    let df = (levels - 1) as u32;

    // Per-level ratio + inverse-variance weight; any degenerate level abstains.
    let mut rs = Vec::with_capacity(levels);
    let mut ws = Vec::with_capacity(levels);
    for cell in &grid {
        let Some((r, var)) = cell.ratio_var() else {
            return InteractionResult::insufficient(df);
        };
        rs.push(r);
        ws.push(1.0 / var);
    }

    // Inverse-variance pooled ratio, then Cochran's Q against it.
    let sum_w: f64 = ws.iter().sum();
    if !positive(sum_w) {
        return InteractionResult::insufficient(df);
    }
    let pooled: f64 = rs.iter().zip(&ws).map(|(r, w)| r * w).sum::<f64>() / sum_w;
    let q: f64 = rs
        .iter()
        .zip(&ws)
        .map(|(r, w)| {
            let d = r - pooled;
            w * d * d
        })
        .sum();

    if !q.is_finite() {
        return InteractionResult::insufficient(df);
    }
    let p_value = super::chi_square_sf(q, df as f64);
    InteractionResult {
        estimate: q,
        statistic: q,
        p_value,
        df,
        significant: p_value < super::ALPHA && p_value.is_finite(),
        insufficient_data: false,
    }
}

// ── Two-way interaction ──────────────────────────────────────────────────────

fn two_way(cells: &[NdRatioCell], a: usize, b: usize) -> InteractionResult {
    let Some((dims, grid)) = collapse(cells, &[a, b]) else {
        return InteractionResult::insufficient(1);
    };
    let (la, lb) = (dims[0], dims[1]);
    let df = ((la - 1) * (lb - 1)) as u32;

    if la == 2 && lb == 2 {
        // Exact difference-in-differences z-test on the four corner ratios.
        did_2x2(&grid, &dims, df)
    } else {
        // Residual of the inverse-variance-weighted additive fit (μ + αₐ + β_b).
        residual_q(&grid, &dims, df)
    }
}

/// 2×2 difference-in-differences contrast `δ = (R11−R10) − (R01−R00)` with a
/// pooled delta-method SE; two-sided normal-tail p-value.
fn did_2x2(grid: &[RatioAgg], dims: &[usize], df: u32) -> InteractionResult {
    let mut r = [[0.0f64; 2]; 2];
    let mut var_sum = 0.0f64;
    for i in 0..2 {
        for j in 0..2 {
            let Some((rij, vij)) = grid[flat_index(&[i, j], dims)].ratio_var() else {
                return InteractionResult::insufficient(df);
            };
            r[i][j] = rij;
            var_sum += vij;
        }
    }

    let delta = (r[1][1] - r[1][0]) - (r[0][1] - r[0][0]);
    let se = var_sum.sqrt();
    if !positive(se) || !se.is_finite() {
        return InteractionResult::insufficient(df);
    }
    let z = delta / se;
    let p_value = super::z_to_p(z);
    InteractionResult {
        estimate: delta,
        statistic: z,
        p_value,
        df,
        significant: p_value < super::ALPHA && p_value.is_finite(),
        insufficient_data: false,
    }
}

// ── Three-way interaction ────────────────────────────────────────────────────

fn three_way(cells: &[NdRatioCell], a: usize, b: usize, c: usize) -> InteractionResult {
    let Some((dims, grid)) = collapse(cells, &[a, b, c]) else {
        return InteractionResult::insufficient(1);
    };
    let (la, lb, lc) = (dims[0], dims[1], dims[2]);
    let df = ((la - 1) * (lb - 1) * (lc - 1)) as u32;

    if la == 2 && lb == 2 && lc == 2 {
        did_2x2x2(&grid, &dims, df)
    } else {
        residual_q(&grid, &dims, df)
    }
}

/// 2×2×2 difference-in-differences-of-differences contrast over the eight
/// corners with a pooled delta-method SE; two-sided normal-tail p-value.
fn did_2x2x2(grid: &[RatioAgg], dims: &[usize], df: u32) -> InteractionResult {
    let mut r = [[[0.0f64; 2]; 2]; 2];
    let mut var_sum = 0.0f64;
    for i in 0..2 {
        for j in 0..2 {
            for k in 0..2 {
                let Some((rijk, vijk)) = grid[flat_index(&[i, j, k], dims)].ratio_var() else {
                    return InteractionResult::insufficient(df);
                };
                r[i][j][k] = rijk;
                var_sum += vijk;
            }
        }
    }

    // δ3 = [(R111−R110)−(R101−R100)] − [(R011−R010)−(R001−R000)].
    let high = (r[1][1][1] - r[1][1][0]) - (r[1][0][1] - r[1][0][0]);
    let low = (r[0][1][1] - r[0][1][0]) - (r[0][0][1] - r[0][0][0]);
    let delta = high - low;

    let se = var_sum.sqrt();
    if !positive(se) || !se.is_finite() {
        return InteractionResult::insufficient(df);
    }
    let z = delta / se;
    let p_value = super::z_to_p(z);
    InteractionResult {
        estimate: delta,
        statistic: z,
        p_value,
        df,
        significant: p_value < super::ALPHA && p_value.is_finite(),
        insufficient_data: false,
    }
}

// ── General weighted-least-squares residual quadratic form ───────────────────

/// Fit, by inverse-variance weighted least squares, the model containing every
/// interaction term of order **below** the grid's dimensionality `k` (i.e. all
/// effects up to `(k−1)`-way) and return its residual quadratic form
/// `Q = Σ w·resid² ~ χ²` on `df`.
///
/// For a 2-D grid this is the additive model `μ + αₐ + β_b`; for a 3-D grid it
/// is the additive-plus-all-pairwise model. Reference (dummy) coding is used so
/// the design is full rank on a dense grid. Any degenerate cell, or a design
/// that is rank-deficient / not over-determined, yields `insufficient(df)`.
fn residual_q(grid: &[RatioAgg], dims: &[usize], df: u32) -> InteractionResult {
    let k = dims.len();

    // Decode each flat grid position back to its per-factor coordinates and
    // collect the (R, weight) of every cell; abstain on any degenerate cell.
    let total: usize = dims.iter().product();
    let mut coords = Vec::with_capacity(total);
    let mut ys = Vec::with_capacity(total);
    let mut ws = Vec::with_capacity(total);
    for (flat, cell) in grid.iter().enumerate() {
        let mut rem = flat;
        let mut co = vec![0usize; k];
        for slot in (0..k).rev() {
            co[slot] = rem % dims[slot];
            rem /= dims[slot];
        }
        let Some((r, var)) = cell.ratio_var() else {
            return InteractionResult::insufficient(df);
        };
        coords.push(co);
        ys.push(r);
        ws.push(1.0 / var);
    }

    // Build the design-matrix column descriptors: one for the intercept, then
    // one per (subset of factors of size 1..=k−1) × (per-factor non-reference
    // level choice). A column is 1 iff every factor in its subset is at its
    // chosen non-zero level, else 0.
    let columns = design_columns(dims, k);
    let p = columns.len();
    if p == 0 || total <= p {
        // Not over-determined: no residual degrees of freedom to test against.
        return InteractionResult::insufficient(df);
    }

    // Weighted normal equations: (XᵀWX) β = XᵀWy.
    let mut xtwx = vec![vec![0.0f64; p]; p];
    let mut xtwy = vec![0.0f64; p];
    for row in 0..total {
        let w = ws[row];
        let x: Vec<f64> = columns
            .iter()
            .map(|col| design_value(col, &coords[row]))
            .collect();
        for i in 0..p {
            if x[i] == 0.0 {
                continue;
            }
            let wxi = w * x[i];
            xtwy[i] += wxi * ys[row];
            for j in 0..p {
                xtwx[i][j] += wxi * x[j];
            }
        }
    }

    let Some(beta) = solve_linear(xtwx, xtwy) else {
        return InteractionResult::insufficient(df);
    };

    // Residual quadratic form Q = Σ w·(y − ŷ)².
    let mut q = 0.0f64;
    for row in 0..total {
        let pred: f64 = columns
            .iter()
            .zip(&beta)
            .map(|(col, &b)| b * design_value(col, &coords[row]))
            .sum();
        let resid = ys[row] - pred;
        q += ws[row] * resid * resid;
    }
    if q < 0.0 {
        q = 0.0; // floating-point cancellation guard
    }
    if !q.is_finite() {
        return InteractionResult::insufficient(df);
    }

    let p_value = super::chi_square_sf(q, df as f64);
    InteractionResult {
        estimate: q,
        statistic: q,
        p_value,
        df,
        significant: p_value < super::ALPHA && p_value.is_finite(),
        insufficient_data: false,
    }
}

/// One design-matrix column: the factors it constrains paired with the
/// (non-reference, i.e. ≥ 1) level each of those factors must equal for the
/// column to be 1. The empty vector is the intercept.
type DesignColumn = Vec<(usize, usize)>;

/// Enumerate the design columns for the "all effects of order `< k`" model over
/// a grid of `dims`, using reference (dummy) coding (reference level = 0).
fn design_columns(dims: &[usize], k: usize) -> Vec<DesignColumn> {
    let mut cols: Vec<DesignColumn> = vec![Vec::new()]; // intercept
    let nfac = dims.len();
    // Every non-empty subset of factors with size in 1..=k−1.
    for mask in 1u32..(1u32 << nfac) {
        let subset: Vec<usize> = (0..nfac).filter(|&f| mask & (1 << f) != 0).collect();
        if subset.is_empty() || subset.len() > k - 1 {
            continue;
        }
        // Cartesian product of non-reference levels (1..dims[f]) over the subset.
        let mut choices: Vec<DesignColumn> = vec![Vec::new()];
        for &f in &subset {
            let mut next = Vec::new();
            for partial in &choices {
                for lev in 1..dims[f] {
                    let mut c = partial.clone();
                    c.push((f, lev));
                    next.push(c);
                }
            }
            choices = next;
        }
        cols.extend(choices);
    }
    cols
}

/// Value of `column` for a cell at `coords`: 1.0 iff every constrained factor is
/// at its specified level, else 0.0. The intercept (empty column) is always 1.
fn design_value(column: &DesignColumn, coords: &[usize]) -> f64 {
    for &(f, lev) in column {
        if coords[f] != lev {
            return 0.0;
        }
    }
    1.0
}

/// Solve the dense linear system `a · x = b` by Gaussian elimination with
/// partial pivoting. Returns `None` if the matrix is (numerically) singular.
fn solve_linear(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    debug_assert_eq!(a.len(), n);
    for col in 0..n {
        // Partial pivot: largest-magnitude entry in this column at/below the
        // diagonal becomes the pivot row.
        let pivot = (col..n)
            .max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))
            .unwrap_or(col);
        if a[pivot][col].abs() < 1e-12 {
            return None; // singular / rank-deficient
        }
        a.swap(col, pivot);
        b.swap(col, pivot);

        // Eliminate below the pivot. Split the pivot row off so the rows below
        // can be mutated while reading it.
        let (head, tail) = a.split_at_mut(col + 1);
        let pivot_row = &head[col];
        let pivot_b = b[col];
        for (offset, row) in tail.iter_mut().enumerate() {
            let factor = row[col] / pivot_row[col];
            if factor == 0.0 {
                continue;
            }
            for (rc, pc) in row.iter_mut().zip(pivot_row.iter()).skip(col) {
                *rc -= factor * pc;
            }
            b[col + 1 + offset] -= factor * pivot_b;
        }
    }

    // Back-substitution.
    let mut x = vec![0.0f64; n];
    for row in (0..n).rev() {
        let mut acc = b[row];
        for c in (row + 1)..n {
            acc -= a[row][c] * x[c];
        }
        x[row] = acc / a[row][row];
    }
    if x.iter().all(|v| v.is_finite()) {
        Some(x)
    } else {
        None
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an `NdRatioCell` from explicit per-observation `(numerator,
    /// denominator)` pairs so every sufficient statistic (sums + second moments)
    /// is computed exactly the way the production aggregator would.
    fn rcell(levels: &[usize], obs: &[(f64, f64)]) -> NdRatioCell {
        let n = obs.len() as u64;
        let mut num_sum = 0.0;
        let mut den_sum = 0.0;
        let mut num_sq_sum = 0.0;
        let mut den_sq_sum = 0.0;
        let mut num_den_sum = 0.0;
        for &(num, den) in obs {
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

    /// Synthesize a cell of `n` observations whose ratio is ≈ `ratio`: each
    /// observation has denominator `den` (with a little symmetric jitter so the
    /// within-cell denominator variance is non-zero) and numerator `ratio·den`
    /// plus alternating jitter (so numerator variance and num/den covariance are
    /// likewise non-degenerate). The aggregate ratio is `ratio` exactly.
    fn cell_with_ratio(levels: &[usize], ratio: f64, den: f64, n: usize) -> NdRatioCell {
        let mut obs = Vec::with_capacity(n);
        for i in 0..n {
            let dj = if i % 2 == 0 { 1.0 } else { -1.0 };
            let d = den + dj;
            // Numerator jitter independent of the denominator jitter so cov is
            // small but the mean ratio stays exactly `ratio`.
            let nj = if (i / 2) % 2 == 0 { 1.0 } else { -1.0 };
            let num = ratio * d + nj;
            obs.push((num, d));
        }
        // The alternating numerator jitter cancels in pairs only when n is a
        // multiple of 4; trim by adjusting the last observation's numerator so
        // the aggregate ratio is exact regardless of n.
        let cur_num: f64 = obs.iter().map(|o| o.0).sum();
        let cur_den: f64 = obs.iter().map(|o| o.1).sum();
        let target_num = ratio * cur_den;
        if let Some(last) = obs.last_mut() {
            last.0 += target_num - cur_num;
        }
        rcell(levels, &obs)
    }

    fn term(terms: &[TermResult], kind: TermKind) -> &TermResult {
        terms
            .iter()
            .find(|t| t.kind == kind)
            .unwrap_or_else(|| panic!("missing term {kind:?}"))
    }

    // ── per-cell delta-method sanity ─────────────────────────────────────────

    #[test]
    fn ratio_var_matches_hand_computation() {
        // Two observations: (num, den) = (2, 4) and (4, 4).
        // num_sum=6 den_sum=8 → R=0.75. n=2.
        // mean_num=3 mean_den=4. var_num = (4+16)/2 − 9 = 1. var_den = 0.
        // cov = (8+16)/2 − 12 = 0. Var(R)=(1 − 0 + 0)/(16·2)=1/32.
        let c = rcell(&[0], &[(2.0, 4.0), (4.0, 4.0)]);
        let agg = {
            let mut a = RatioAgg::default();
            a.add(&c);
            a
        };
        let (r, var) = agg.ratio_var().expect("non-degenerate");
        assert!((r - 0.75).abs() < 1e-12, "R={r}");
        assert!((var - 1.0 / 32.0).abs() < 1e-12, "Var={var}");
    }

    #[test]
    fn ratio_var_rejects_degenerate() {
        // den_sum = 0 → None.
        let zero_den = rcell(&[0], &[(0.0, 0.0), (0.0, 0.0)]);
        let mut a = RatioAgg::default();
        a.add(&zero_den);
        assert!(a.ratio_var().is_none());

        // n < 2 → None.
        let one = rcell(&[0], &[(1.0, 2.0)]);
        let mut b = RatioAgg::default();
        b.add(&one);
        assert!(b.ratio_var().is_none());
    }

    #[test]
    fn cell_with_ratio_has_exact_ratio() {
        for &n in &[100usize, 101, 150, 200] {
            let c = cell_with_ratio(&[0, 0], 0.30, 50.0, n);
            let r = c.num_sum / c.den_sum;
            assert!((r - 0.30).abs() < 1e-9, "n={n} R={r}");
        }
    }

    // ── TwoWay 2×2 ───────────────────────────────────────────────────────────

    /// Ratio jumps only in the (1,1) cell → strong planted interaction.
    #[test]
    fn two_way_2x2_planted_interaction_is_significant() {
        let cells = [
            cell_with_ratio(&[0, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[0, 1], 0.20, 50.0, 400),
            cell_with_ratio(&[1, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[1, 1], 0.40, 50.0, 400),
        ];
        let terms = ratio_terms(&cells, 2);
        let t = term(&terms, TermKind::TwoWay { a: 0, b: 1 });
        assert!(!t.freq.insufficient_data);
        assert_eq!(t.freq.df, 1);
        assert!(t.bayes.is_none());
        // δ = (0.40 − 0.20) − (0.20 − 0.20) = 0.20.
        assert!(
            (t.freq.estimate - 0.20).abs() < 1e-6,
            "δ={}",
            t.freq.estimate
        );
        assert!(t.freq.p_value < 0.05, "p={}", t.freq.p_value);
        assert!(t.freq.significant);
    }

    /// Additive ratios across both factors → no interaction.
    /// R00=.20 R01=.30 R10=.40 R11=.50 → δ = (.50−.40) − (.30−.20) = 0.
    #[test]
    fn two_way_2x2_additive_is_not_significant() {
        let cells = [
            cell_with_ratio(&[0, 0], 0.20, 50.0, 600),
            cell_with_ratio(&[0, 1], 0.30, 50.0, 600),
            cell_with_ratio(&[1, 0], 0.40, 50.0, 600),
            cell_with_ratio(&[1, 1], 0.50, 50.0, 600),
        ];
        let terms = ratio_terms(&cells, 2);
        let t = term(&terms, TermKind::TwoWay { a: 0, b: 1 });
        assert!(!t.freq.insufficient_data);
        assert!(
            t.freq.estimate.abs() < 1e-6,
            "δ should be ≈0, got {}",
            t.freq.estimate
        );
        assert!(t.freq.p_value > 0.05, "p={}", t.freq.p_value);
        assert!(!t.freq.significant);
    }

    /// General La×Lb (3×2) grid with a planted non-additive cell → significant
    /// via the weighted-residual chi-square path.
    #[test]
    fn two_way_3x2_planted_interaction_is_significant() {
        let cells = [
            cell_with_ratio(&[0, 0], 0.20, 50.0, 500),
            cell_with_ratio(&[0, 1], 0.20, 50.0, 500),
            cell_with_ratio(&[1, 0], 0.20, 50.0, 500),
            cell_with_ratio(&[1, 1], 0.20, 50.0, 500),
            cell_with_ratio(&[2, 0], 0.20, 50.0, 500),
            cell_with_ratio(&[2, 1], 0.45, 50.0, 500), // jump only here
        ];
        let terms = ratio_terms(&cells, 2);
        let t = term(&terms, TermKind::TwoWay { a: 0, b: 1 });
        assert!(!t.freq.insufficient_data);
        assert_eq!(t.freq.df, 2); // (3−1)(2−1)
        assert!((t.freq.estimate - t.freq.statistic).abs() < 1e-12);
        assert!(
            t.freq.p_value < 0.05,
            "p={} Q={}",
            t.freq.p_value,
            t.freq.statistic
        );
        assert!(t.freq.significant);
    }

    /// General 3×2 grid whose ratios are exactly additive (R = base + row + col)
    /// → residual Q ≈ 0 → not significant.
    #[test]
    fn two_way_3x2_additive_is_not_significant() {
        // Additive on the ratio scale: r(i,j) = 0.10 + 0.10·i + 0.05·j.
        let r = |i: usize, j: usize| 0.10 + 0.10 * i as f64 + 0.05 * j as f64;
        let cells = [
            cell_with_ratio(&[0, 0], r(0, 0), 50.0, 800),
            cell_with_ratio(&[0, 1], r(0, 1), 50.0, 800),
            cell_with_ratio(&[1, 0], r(1, 0), 50.0, 800),
            cell_with_ratio(&[1, 1], r(1, 1), 50.0, 800),
            cell_with_ratio(&[2, 0], r(2, 0), 50.0, 800),
            cell_with_ratio(&[2, 1], r(2, 1), 50.0, 800),
        ];
        let terms = ratio_terms(&cells, 2);
        let t = term(&terms, TermKind::TwoWay { a: 0, b: 1 });
        assert!(!t.freq.insufficient_data);
        assert!(
            t.freq.p_value > 0.05,
            "p={} Q={}",
            t.freq.p_value,
            t.freq.statistic
        );
        assert!(!t.freq.significant);
    }

    // ── Main effect ──────────────────────────────────────────────────────────

    /// Ratio varies sharply across factor 0's levels (collapsing over factor 1)
    /// → main effect significant.
    #[test]
    fn main_effect_varying_ratio_is_significant() {
        let cells = [
            cell_with_ratio(&[0, 0], 0.20, 50.0, 500),
            cell_with_ratio(&[0, 1], 0.20, 50.0, 500),
            cell_with_ratio(&[1, 0], 0.50, 50.0, 500),
            cell_with_ratio(&[1, 1], 0.50, 50.0, 500),
        ];
        let terms = ratio_terms(&cells, 2);
        let t = term(&terms, TermKind::Main { factor: 0 });
        assert!(!t.freq.insufficient_data);
        assert_eq!(t.freq.df, 1);
        assert!(
            t.freq.p_value < 0.05,
            "p={} Q={}",
            t.freq.p_value,
            t.freq.statistic
        );
        assert!(t.freq.significant);
    }

    /// Ratio is flat across factor 0's levels → main effect not significant.
    #[test]
    fn main_effect_flat_ratio_is_not_significant() {
        let cells = [
            cell_with_ratio(&[0, 0], 0.30, 50.0, 500),
            cell_with_ratio(&[0, 1], 0.30, 50.0, 500),
            cell_with_ratio(&[1, 0], 0.30, 50.0, 500),
            cell_with_ratio(&[1, 1], 0.30, 50.0, 500),
        ];
        let terms = ratio_terms(&cells, 2);
        let t = term(&terms, TermKind::Main { factor: 0 });
        assert!(!t.freq.insufficient_data);
        assert!(
            t.freq.p_value > 0.05,
            "p={} Q={}",
            t.freq.p_value,
            t.freq.statistic
        );
        assert!(!t.freq.significant);
    }

    /// Three-level main effect with a monotone ratio gradient → significant,
    /// df = 2.
    #[test]
    fn main_effect_three_levels_is_significant() {
        let cells = [
            cell_with_ratio(&[0, 0], 0.20, 50.0, 500),
            cell_with_ratio(&[0, 1], 0.20, 50.0, 500),
            cell_with_ratio(&[1, 0], 0.35, 50.0, 500),
            cell_with_ratio(&[1, 1], 0.35, 50.0, 500),
            cell_with_ratio(&[2, 0], 0.55, 50.0, 500),
            cell_with_ratio(&[2, 1], 0.55, 50.0, 500),
        ];
        let terms = ratio_terms(&cells, 2);
        let t = term(&terms, TermKind::Main { factor: 0 });
        assert!(!t.freq.insufficient_data);
        assert_eq!(t.freq.df, 2);
        assert!(t.freq.significant, "p={}", t.freq.p_value);
    }

    // ── ThreeWay 2×2×2 ───────────────────────────────────────────────────────

    /// Planted three-way: the two-way (b,c) interaction itself flips sign
    /// between a=0 and a=1, so the difference-of-differences-of-differences is
    /// large → significant.
    #[test]
    fn three_way_2x2x2_planted_is_significant() {
        // a=0 slice: additive (no 2-way). a=1 slice: a (1,1) jump → 2-way present.
        let cells = [
            // a = 0
            cell_with_ratio(&[0, 0, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[0, 0, 1], 0.20, 50.0, 400),
            cell_with_ratio(&[0, 1, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[0, 1, 1], 0.20, 50.0, 400),
            // a = 1 (interaction only in the b=1,c=1 corner)
            cell_with_ratio(&[1, 0, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[1, 0, 1], 0.20, 50.0, 400),
            cell_with_ratio(&[1, 1, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[1, 1, 1], 0.45, 50.0, 400),
        ];
        let terms = ratio_terms(&cells, 3);
        let t = term(&terms, TermKind::ThreeWay { a: 0, b: 1, c: 2 });
        assert!(!t.freq.insufficient_data);
        assert_eq!(t.freq.df, 1);
        assert!(t.bayes.is_none());
        // δ3 = [(.45−.20)−(.20−.20)] − [(.20−.20)−(.20−.20)] = 0.25.
        assert!(
            (t.freq.estimate - 0.25).abs() < 1e-6,
            "δ3={}",
            t.freq.estimate
        );
        assert!(t.freq.p_value < 0.05, "p={}", t.freq.p_value);
        assert!(t.freq.significant);
    }

    /// No three-way: both a-slices share the *same* 2-way structure, so the
    /// three-way contrast cancels → not significant (even though lower-order
    /// interactions exist).
    #[test]
    fn three_way_2x2x2_no_three_way_is_not_significant() {
        // Same (1,1)-corner 2-way bump in BOTH a slices → δ3 = 0.
        let cells = [
            cell_with_ratio(&[0, 0, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[0, 0, 1], 0.20, 50.0, 400),
            cell_with_ratio(&[0, 1, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[0, 1, 1], 0.35, 50.0, 400),
            cell_with_ratio(&[1, 0, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[1, 0, 1], 0.20, 50.0, 400),
            cell_with_ratio(&[1, 1, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[1, 1, 1], 0.35, 50.0, 400),
        ];
        let terms = ratio_terms(&cells, 3);
        let t = term(&terms, TermKind::ThreeWay { a: 0, b: 1, c: 2 });
        assert!(!t.freq.insufficient_data);
        assert!(
            t.freq.estimate.abs() < 1e-6,
            "δ3 should be ≈0, got {}",
            t.freq.estimate
        );
        assert!(t.freq.p_value > 0.05, "p={}", t.freq.p_value);
        assert!(!t.freq.significant);
    }

    /// order==3 emits all 3 mains, all 3 pairwise, and the three-way term.
    #[test]
    fn order_three_emits_full_term_set() {
        let cells = [
            cell_with_ratio(&[0, 0, 0], 0.20, 50.0, 200),
            cell_with_ratio(&[0, 0, 1], 0.20, 50.0, 200),
            cell_with_ratio(&[0, 1, 0], 0.20, 50.0, 200),
            cell_with_ratio(&[0, 1, 1], 0.20, 50.0, 200),
            cell_with_ratio(&[1, 0, 0], 0.20, 50.0, 200),
            cell_with_ratio(&[1, 0, 1], 0.20, 50.0, 200),
            cell_with_ratio(&[1, 1, 0], 0.20, 50.0, 200),
            cell_with_ratio(&[1, 1, 1], 0.20, 50.0, 200),
        ];
        let terms = ratio_terms(&cells, 3);
        assert_eq!(terms.len(), 7);
        for kind in [
            TermKind::Main { factor: 0 },
            TermKind::Main { factor: 1 },
            TermKind::Main { factor: 2 },
            TermKind::TwoWay { a: 0, b: 1 },
            TermKind::TwoWay { a: 0, b: 2 },
            TermKind::TwoWay { a: 1, b: 2 },
            TermKind::ThreeWay { a: 0, b: 1, c: 2 },
        ] {
            let _ = term(&terms, kind);
        }
        assert!(terms.iter().all(|t| t.bayes.is_none()));
    }

    // ── Degenerate / edge cases ──────────────────────────────────────────────

    /// A zero-denominator cell makes every term that touches it insufficient.
    #[test]
    fn zero_denominator_cell_is_insufficient() {
        let mut bad = cell_with_ratio(&[1, 1], 0.40, 50.0, 400);
        bad.den_sum = 0.0; // collapse will still see den_sum ≤ 0 here
        bad.num_sum = 0.0;
        bad.den_sq_sum = 0.0;
        bad.num_sq_sum = 0.0;
        bad.num_den_sum = 0.0;
        let cells = [
            cell_with_ratio(&[0, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[0, 1], 0.20, 50.0, 400),
            cell_with_ratio(&[1, 0], 0.20, 50.0, 400),
            bad,
        ];
        let terms = ratio_terms(&cells, 2);
        let t = term(&terms, TermKind::TwoWay { a: 0, b: 1 });
        assert!(t.freq.insufficient_data);
        assert!(!t.freq.significant);
        assert!(t.freq.estimate.is_nan());
        assert!(t.freq.p_value.is_nan());
    }

    /// A cell with only one observation (n < 2) → insufficient delta variance.
    #[test]
    fn tiny_n_cell_is_insufficient() {
        let tiny = rcell(&[1, 1], &[(20.0, 50.0)]); // n = 1
        let cells = [
            cell_with_ratio(&[0, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[0, 1], 0.20, 50.0, 400),
            cell_with_ratio(&[1, 0], 0.20, 50.0, 400),
            tiny,
        ];
        let terms = ratio_terms(&cells, 2);
        let t = term(&terms, TermKind::TwoWay { a: 0, b: 1 });
        assert!(t.freq.insufficient_data);
    }

    /// A single level on a factor → no contrast → main effect insufficient.
    #[test]
    fn single_level_main_is_insufficient() {
        // Factor 1 has only level 0; its main effect cannot be tested.
        let cells = [
            cell_with_ratio(&[0, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[1, 0], 0.30, 50.0, 400),
        ];
        let terms = ratio_terms(&cells, 2);
        let t = term(&terms, TermKind::Main { factor: 1 });
        assert!(t.freq.insufficient_data);
    }

    #[test]
    fn empty_input_yields_no_terms() {
        assert!(ratio_terms(&[], 2).is_empty());
    }

    /// Determinism: identical inputs yield byte-identical outputs.
    #[test]
    fn ratio_terms_is_deterministic() {
        let cells = [
            cell_with_ratio(&[0, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[0, 1], 0.20, 50.0, 400),
            cell_with_ratio(&[1, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[1, 1], 0.40, 50.0, 400),
        ];
        let a = ratio_terms(&cells, 2);
        let b = ratio_terms(&cells, 2);
        assert_eq!(a, b);
    }

    /// The 2×2 z-path and a manual delta-method computation agree on δ and z.
    #[test]
    fn two_way_2x2_matches_manual_contrast() {
        let cells = [
            cell_with_ratio(&[0, 0], 0.20, 50.0, 400),
            cell_with_ratio(&[0, 1], 0.25, 50.0, 400),
            cell_with_ratio(&[1, 0], 0.30, 50.0, 400),
            cell_with_ratio(&[1, 1], 0.60, 50.0, 400),
        ];
        let terms = ratio_terms(&cells, 2);
        let t = term(&terms, TermKind::TwoWay { a: 0, b: 1 });

        // Manual: δ = (R11 − R10) − (R01 − R00).
        let r = |c: &NdRatioCell| c.num_sum / c.den_sum;
        let expected = (r(&cells[3]) - r(&cells[2])) - (r(&cells[1]) - r(&cells[0]));
        assert!(
            (t.freq.estimate - expected).abs() < 1e-9,
            "δ={}",
            t.freq.estimate
        );
        // z and p are mutually consistent (p = z_to_p(z)).
        assert!((t.freq.p_value - super::super::z_to_p(t.freq.statistic)).abs() < 1e-12);
    }
}
