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

use super::{NdRatioCell, TermResult};

/// See module contract. **Stub** — returns no terms until implemented (P2.T4).
pub fn ratio_terms(cells: &[NdRatioCell], order: usize) -> Vec<TermResult> {
    let _ = (cells, order);
    Vec::new()
}
