//! Pure, in-memory real-time bandit sampler.
//!
//! The real-time (snapshot-resident) propagation path rides the reward model on
//! the flag snapshot ([`RealtimeBanditModel`]) and samples the assigned variant
//! per-context at evaluation time — mirroring the in-memory `ExclusionGate`
//! bucket math. This module holds the one pure function the evaluator calls.
//!
//! ## Purity
//!
//! [`sample_realtime_variant`] is pure synchronous math: no async, no I/O, no
//! clock reads. It reuses the SAME seeded [`Lcg`] + Beta / standard-normal
//! samplers the Bayesian stats core uses (a single source of truth for posterior
//! sampling). The evaluator lives in the same crate, so it can call this without
//! violating the `evaluation` purity contract — that contract only forbids
//! `sqlx::` / `tokio::` / `reqwest::` / `warn!` / `error!` tokens, not pure
//! cross-module calls.
//!
//! ## Determinism
//!
//! The draw is fully determined by `(model, seed)`: a fixed model + fixed seed
//! always yields the same variant. The evaluator derives `seed` from the unit
//! context's key (a Murmur3 fold of `salt + key`, see
//! `evaluation::engine`), so the same context always resolves identically — in
//! preview and in the SDK alike.

use crate::experimentation::stats::bayesian::{Lcg, sample_beta, sample_standard_normal};
use crate::rule_engine::types::{BanditGoal, RealtimeBanditModel, RewardFamily};

/// One sampled draw per variant, paired with the variant key, in model order.
///
/// Returned alongside the chosen variant so the evaluator can surface the
/// sampled weights in a trace WITHOUT exposing any posterior parameters or
/// context values.
#[derive(Debug, Clone, PartialEq)]
pub struct SampledDraw {
    /// The variant key sampled.
    pub variant_key: String,
    /// The single posterior draw for this variant.
    pub draw: f64,
}

/// Draw one posterior sample per variant from `model` (seeded by `seed`) and
/// return the goal-directed argmax variant key plus every draw.
///
/// Returns `None` when the model has no variants (nothing to sample). Each
/// variant is sampled once, in declaration order, from the SAME [`Lcg`] stream —
/// so the result is deterministic in `(model, seed)`. Ties resolve to the first
/// variant in declaration order (stable).
///
/// * [`RewardFamily::Beta`] arms draw a rate from `Beta(alpha, beta)`
///   (degenerate params fall back to a `0.5` midpoint, matching [`sample_beta`]).
/// * [`RewardFamily::Normal`] arms draw `mu + sqrt(sigma2) * Z` for a standard
///   normal `Z` (non-positive `sigma2` collapses to the point mean `mu`).
///
/// Pure: no async, no I/O, no clock.
#[must_use]
pub fn sample_realtime_variant(
    model: &RealtimeBanditModel,
    seed: u64,
) -> Option<(String, Vec<SampledDraw>)> {
    if model.variants.is_empty() {
        return None;
    }
    let mut rng = Lcg::new(seed);
    let mut draws: Vec<SampledDraw> = Vec::with_capacity(model.variants.len());
    for v in &model.variants {
        let draw = match model.family {
            RewardFamily::Beta => {
                // Beta posterior; weak-prior safety so a zeroed arm still draws.
                let alpha = if v.alpha > 0.0 { v.alpha } else { 1.0 };
                let beta = if v.beta > 0.0 { v.beta } else { 1.0 };
                sample_beta(alpha, beta, &mut rng)
            }
            RewardFamily::Normal => {
                let sd = v.sigma2.max(0.0).sqrt();
                if sd > 0.0 {
                    v.mu + sd * sample_standard_normal(&mut rng)
                } else {
                    // No spread: a point draw at the mean (RNG stream untouched
                    // for a degenerate arm).
                    v.mu
                }
            }
        };
        draws.push(SampledDraw {
            variant_key: v.variant_key.clone(),
            draw,
        });
    }

    // Goal-directed argmax (Increase) / argmin (Decrease); first wins ties.
    let mut best_idx = 0usize;
    for i in 1..draws.len() {
        let better = match model.goal {
            BanditGoal::Increase => draws[i].draw > draws[best_idx].draw,
            BanditGoal::Decrease => draws[i].draw < draws[best_idx].draw,
        };
        if better {
            best_idx = i;
        }
    }
    Some((draws[best_idx].variant_key.clone(), draws))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule_engine::types::VariantPosterior;

    fn beta_model(arms: &[(&str, f64, f64)]) -> RealtimeBanditModel {
        RealtimeBanditModel {
            salt: "s".into(),
            unit_context_type: "user".into(),
            family: RewardFamily::Beta,
            goal: BanditGoal::Increase,
            variants: arms
                .iter()
                .map(|(k, a, b)| VariantPosterior {
                    variant_key: (*k).into(),
                    alpha: *a,
                    beta: *b,
                    mu: 0.0,
                    sigma2: 0.0,
                })
                .collect(),
        }
    }

    fn normal_model(arms: &[(&str, f64, f64)], goal: BanditGoal) -> RealtimeBanditModel {
        RealtimeBanditModel {
            salt: "s".into(),
            unit_context_type: "user".into(),
            family: RewardFamily::Normal,
            goal,
            variants: arms
                .iter()
                .map(|(k, mu, s2)| VariantPosterior {
                    variant_key: (*k).into(),
                    alpha: 0.0,
                    beta: 0.0,
                    mu: *mu,
                    sigma2: *s2,
                })
                .collect(),
        }
    }

    #[test]
    fn empty_model_returns_none() {
        let m = beta_model(&[]);
        assert!(sample_realtime_variant(&m, 1).is_none());
    }

    #[test]
    fn deterministic_in_seed() {
        let m = beta_model(&[("a", 50.0, 50.0), ("b", 80.0, 20.0)]);
        let r1 = sample_realtime_variant(&m, 12345);
        let r2 = sample_realtime_variant(&m, 12345);
        assert_eq!(r1, r2);
    }

    #[test]
    fn clear_winner_dominates_over_contexts() {
        // "better" has a much higher Beta mean; across many seeds it should win
        // the large majority of contexts.
        let m = beta_model(&[("control", 100.0, 900.0), ("better", 900.0, 100.0)]);
        let mut better = 0;
        let n = 2000;
        for seed in 0..n {
            let (chosen, _) = sample_realtime_variant(&m, seed).unwrap();
            if chosen == "better" {
                better += 1;
            }
        }
        assert!(
            better as f64 / n as f64 > 0.9,
            "better should win most contexts, got {better}/{n}"
        );
    }

    #[test]
    fn normal_increase_prefers_higher_mean() {
        let m = normal_model(
            &[("low", 1.0, 0.01), ("high", 5.0, 0.01)],
            BanditGoal::Increase,
        );
        let mut high = 0;
        for seed in 0..500 {
            if sample_realtime_variant(&m, seed).unwrap().0 == "high" {
                high += 1;
            }
        }
        assert!(
            high > 480,
            "tight high-mean arm should dominate, got {high}/500"
        );
    }

    #[test]
    fn normal_decrease_prefers_lower_mean() {
        let m = normal_model(
            &[("low", 1.0, 0.01), ("high", 5.0, 0.01)],
            BanditGoal::Decrease,
        );
        let mut low = 0;
        for seed in 0..500 {
            if sample_realtime_variant(&m, seed).unwrap().0 == "low" {
                low += 1;
            }
        }
        assert!(
            low > 480,
            "tight low-mean arm should win under decrease, got {low}/500"
        );
    }

    #[test]
    fn point_normal_is_pure_argmax() {
        // sigma2 == 0 on every arm → deterministic argmax regardless of seed.
        let m = normal_model(&[("a", 1.0, 0.0), ("b", 2.0, 0.0)], BanditGoal::Increase);
        for seed in [0u64, 7, 999] {
            assert_eq!(sample_realtime_variant(&m, seed).unwrap().0, "b");
        }
    }

    #[test]
    fn draws_are_in_model_order() {
        let m = beta_model(&[("a", 5.0, 5.0), ("b", 5.0, 5.0), ("c", 5.0, 5.0)]);
        let (_, draws) = sample_realtime_variant(&m, 3).unwrap();
        assert_eq!(
            draws
                .iter()
                .map(|d| d.variant_key.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b", "c"]
        );
    }
}
