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

use super::{NdBinaryCell, TermResult};

/// See module contract. **Stub** — returns no terms until implemented (P2.T2).
pub fn binary_terms(cells: &[NdBinaryCell], order: usize) -> Vec<TermResult> {
    let _ = (cells, order);
    Vec::new()
}
