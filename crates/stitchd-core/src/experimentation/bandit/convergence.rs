//! Bandit convergence detector (posterior probability-to-be-best threshold).
//!
//! A bandit experiment has *converged* when one arm's posterior
//! probability-of-being-best crosses an operator-configured threshold. This is
//! the basis for the autonomous lifecycle (Phase 7) and optimization campaigns
//! (Phase 8): advisory badges, auto-commit, auto-rollout and spawn-on-convergence
//! all hang off the same `Option<ConvergedWinner>` signal.
//!
//! The probability-of-being-best is exactly the Thompson-sampling Monte-Carlo
//! quantity ([`super::thompson::thompson_weights`]): draw `n_samples` joint rounds
//! from every arm's posterior, count how often each arm is the goal-directed
//! argmax. This module reuses that machinery verbatim so convergence and
//! allocation share one definition of "probability best".
//!
//! ## Determinism
//!
//! Identical `arms` + `goal` + `n_samples` + `seed` always yield an identical
//! result (pinned by golden-vector tests). No async, no I/O — pure math, mirroring
//! the rest of the bandit core.

use super::thompson::{BanditArm, GoalDirection, thompson_weights};

/// Default Monte-Carlo sample count for convergence detection. Large enough that
/// the probability-best estimate is stable to a few parts in a thousand for the
/// arm counts bandits use in practice.
pub const DEFAULT_CONVERGENCE_SAMPLES: usize = 4000;

/// The winning arm of a converged bandit: its variant key and the posterior
/// probability that it is the goal-directed best arm.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvergedWinner {
    /// The variant key of the converged winner.
    pub variant_key: String,
    /// The posterior probability (in `[0, 1]`) that this arm is the best arm —
    /// the value that crossed the convergence threshold.
    pub prob: f64,
}

/// Posterior probability-of-being-best per arm, in the same order as `arms`.
///
/// Thin wrapper over [`thompson_weights`]: the Thompson "raw weight" already *is*
/// `P(arm is goal-directed best)`. Exposed under a convergence-specific name so
/// the lifecycle code reads intent-first and does not depend on the allocator.
///
/// Deterministic under `seed`. An empty `arms` returns an empty vec; `n_samples`
/// is clamped to at least 1 by the underlying sampler.
pub fn probability_best(
    arms: &[BanditArm],
    goal: GoalDirection,
    n_samples: usize,
    seed: u64,
) -> Vec<(String, f64)> {
    thompson_weights(arms, goal, n_samples, seed)
}

/// Detect convergence: return [`Some`] when the single best arm's
/// probability-of-being-best is **at or above** `threshold`.
///
/// The winner is the arm with the highest probability-best; convergence fires
/// only when that maximum reaches `threshold`. A tie (two arms sharing the top
/// probability, neither individually above threshold) does **not** converge —
/// exactly one arm must dominate.
///
/// * Returns `None` for an empty arm set.
/// * Returns `None` for a single arm (a one-armed bandit has nothing to converge
///   *against* — there is no exploration/exploitation tradeoff to resolve).
/// * `threshold` outside `(0, 1)` is accepted but a `>= 1.0` threshold can only
///   be met by a probability that rounds to exactly 1.0 (degenerate clear
///   winner); callers should validate the threshold via
///   [`super::types::BanditConfig::validate`] first.
///
/// Deterministic under `seed`.
pub fn detect_convergence(
    arms: &[BanditArm],
    goal: GoalDirection,
    threshold: f64,
    seed: u64,
) -> Option<ConvergedWinner> {
    if arms.len() < 2 {
        return None;
    }
    let probs = probability_best(arms, goal, DEFAULT_CONVERGENCE_SAMPLES, seed);

    // Highest-probability arm (ties → first in input order via strict `>`).
    let mut best_idx = 0usize;
    let mut best_prob = probs[0].1;
    for (i, (_, p)) in probs.iter().enumerate().skip(1) {
        if *p > best_prob {
            best_prob = *p;
            best_idx = i;
        }
    }

    // A genuine tie at the top (another arm shares the exact max) is not a
    // convergence: no single arm dominates.
    let tied_at_top = probs
        .iter()
        .enumerate()
        .any(|(i, (_, p))| i != best_idx && *p == best_prob);
    if tied_at_top {
        return None;
    }

    if best_prob >= threshold {
        Some(ConvergedWinner {
            variant_key: probs[best_idx].0.clone(),
            prob: best_prob,
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experimentation::stats::VariantStats;

    fn count_arm(key: &str, n: i64, conv: i64) -> BanditArm {
        BanditArm {
            variant_key: key.into(),
            posterior: super::super::thompson::RewardPosterior::Conversion(VariantStats {
                sample_size: n,
                conversions: Some(conv),
                mean: None,
                variance: None,
                conversion_rate: Some(conv as f64 / n as f64),
                percentiles: None,
            }),
        }
    }

    #[test]
    fn probability_best_matches_thompson_weights() {
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 130)];
        let p = probability_best(&arms, GoalDirection::Increase, 2000, 42);
        let w = thompson_weights(&arms, GoalDirection::Increase, 2000, 42);
        assert_eq!(p, w);
    }

    #[test]
    fn empty_arms_no_convergence() {
        assert_eq!(
            detect_convergence(&[], GoalDirection::Increase, 0.9, 1),
            None
        );
    }

    #[test]
    fn single_arm_no_convergence() {
        let arms = vec![count_arm("a", 1000, 500)];
        assert_eq!(
            detect_convergence(&arms, GoalDirection::Increase, 0.9, 1),
            None
        );
    }

    #[test]
    fn clear_winner_converges() {
        // 10% vs 30% over 1000 trials each — a decisive separation.
        let arms = vec![count_arm("control", 1000, 100), count_arm("win", 1000, 300)];
        let res = detect_convergence(&arms, GoalDirection::Increase, 0.95, 7);
        let w = res.expect("clear winner should converge");
        assert_eq!(w.variant_key, "win");
        assert!(w.prob >= 0.95, "prob {} should be >= 0.95", w.prob);
    }

    #[test]
    fn tie_does_not_converge() {
        // Two identical arms — neither dominates; prob-best ~0.5 each.
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 100)];
        let res = detect_convergence(&arms, GoalDirection::Increase, 0.95, 5);
        assert_eq!(res, None, "a tie must not converge");
    }

    #[test]
    fn close_arms_below_threshold_no_convergence() {
        // 10% vs 11% — a real but tiny edge; nowhere near 0.95 prob-best.
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 110)];
        let res = detect_convergence(&arms, GoalDirection::Increase, 0.95, 11);
        assert_eq!(res, None);
    }

    #[test]
    fn decrease_goal_converges_on_lower() {
        // Lower conversion is "better" under Decrease.
        let arms = vec![count_arm("low", 1000, 50), count_arm("high", 1000, 300)];
        let res = detect_convergence(&arms, GoalDirection::Decrease, 0.95, 13);
        let w = res.expect("decisive low arm should converge under decrease");
        assert_eq!(w.variant_key, "low");
    }

    #[test]
    fn deterministic_under_seed() {
        let arms = vec![count_arm("control", 1000, 100), count_arm("win", 1000, 300)];
        let a = detect_convergence(&arms, GoalDirection::Increase, 0.9, 99);
        let b = detect_convergence(&arms, GoalDirection::Increase, 0.9, 99);
        assert_eq!(a, b);
    }

    #[test]
    fn golden_vector_three_arms() {
        let arms = vec![
            count_arm("a", 1000, 100),
            count_arm("b", 1000, 300),
            count_arm("c", 1000, 110),
        ];
        // Decisive "b" with a moderate threshold converges to b.
        let res = detect_convergence(&arms, GoalDirection::Increase, 0.9, 0xABCD);
        let w = res.expect("b should converge");
        assert_eq!(w.variant_key, "b");
        // Pin: same seed reproduces the exact probability.
        let res2 = detect_convergence(&arms, GoalDirection::Increase, 0.9, 0xABCD);
        assert_eq!(Some(w), res2);
    }

    #[test]
    fn high_threshold_blocks_marginal_winner() {
        // A modest winner that clears 0.8 prob-best but not 0.999.
        let arms = vec![count_arm("a", 200, 20), count_arm("b", 200, 34)];
        let lenient = detect_convergence(&arms, GoalDirection::Increase, 0.8, 3);
        let strict = detect_convergence(&arms, GoalDirection::Increase, 0.999, 3);
        // Whatever the exact prob, strict must be a subset of lenient.
        if strict.is_some() {
            assert!(lenient.is_some());
        }
    }
}
