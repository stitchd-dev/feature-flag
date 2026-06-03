//! (P2.T5) **Bayesian interaction posteriors** for binary (conversion / funnel)
//! metrics — Beta-Binomial cell model.
//!
//! ## Contract (signature fixed by the seam; body implemented by the worker)
//!
//! `binary_bayes(cells, order)` returns a `(TermKind, BayesianInteraction)` for
//! every term that [`super::loglinear::binary_terms`] produces — the routing
//! layer joins them by [`super::TermKind`]. Terms too sparse for a meaningful
//! posterior are omitted (the routing layer leaves their `bayes` as `None`).
//!
//! Model each cell rate with a Beta(α0+successes, β0+failures) posterior (weak
//! prior, e.g. Beta(1,1)). The interaction effect is the
//! difference-in-differences of cell rates (generalised per term). Summarise
//! the posterior of that contrast — `prob` = P(|effect| outside a small ROPE),
//! `expected` = posterior mean, `ci_low`/`ci_high` = central credible interval.
//!
//! Determinism: do NOT use a random RNG. Derive the posterior summary
//! analytically, by deterministic quadrature, or with a PRNG seeded from the
//! cell sufficient statistics, so repeated runs are identical. Beta-tail
//! inversion can reuse `super::betai` / `super::lgamma`.

use super::{BayesianInteraction, NdBinaryCell, TermKind};

/// See module contract. **Stub** — returns no posteriors until implemented (P2.T5).
pub fn binary_bayes(cells: &[NdBinaryCell], order: usize) -> Vec<(TermKind, BayesianInteraction)> {
    let _ = (cells, order);
    Vec::new()
}
