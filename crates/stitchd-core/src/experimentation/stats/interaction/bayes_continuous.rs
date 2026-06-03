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
//! `BayesianInteraction`: `prob` = P(|effect| outside a small ROPE), `expected`
//! = posterior mean, `ci_low`/`ci_high` = central credible interval (Normal
//! quantiles via the parent's `super::norm_cdf` and its inverse).
//!
//! Determinism: no random RNG — derive summaries analytically / by deterministic
//! quadrature so repeated runs are identical.

use super::{BayesianInteraction, NdContinuousCell, NdRatioCell, TermKind};

/// See module contract. **Stub** — returns no posteriors until implemented (P2.T6).
pub fn continuous_bayes(
    cells: &[NdContinuousCell],
    order: usize,
) -> Vec<(TermKind, BayesianInteraction)> {
    let _ = (cells, order);
    Vec::new()
}

/// See module contract. **Stub** — returns no posteriors until implemented (P2.T6).
pub fn ratio_bayes(cells: &[NdRatioCell], order: usize) -> Vec<(TermKind, BayesianInteraction)> {
    let _ = (cells, order);
    Vec::new()
}
