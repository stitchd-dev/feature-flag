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
//! Partition the total sum of squares into main / 2-way / 3-way / error
//! components using only per-cell sufficient statistics `(n, sum, sum_sq)`, and
//! emit an F-test per term (`MS_term / MS_error` on the correct df). Empty
//! cells, no within-cell replication (`df_err <= 0`), or zero error variance →
//! [`super::InteractionResult::insufficient`].
//!
//! The order-2 interaction term MUST reproduce [`super::continuous_interaction`]
//! (regression gate, P2.T7). Distribution helper from the parent: `super::fdist_sf`.

use super::{NdContinuousCell, TermResult};

/// See module contract. **Stub** — returns no terms until implemented (P2.T3).
pub fn continuous_terms(cells: &[NdContinuousCell], order: usize) -> Vec<TermResult> {
    let _ = (cells, order);
    Vec::new()
}
