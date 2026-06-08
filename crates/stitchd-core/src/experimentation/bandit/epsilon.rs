//! Epsilon-greedy bandit allocator.
//!
//! The arm with the best goal-directed posterior **mean** receives the
//! exploitation mass `1 − ε`; the exploration mass `ε` is spread *equally* across
//! every arm. The returned weights therefore are:
//!
//! ```text
//! weight[i] = ε / n          + (i == best ? (1 − ε) : 0)
//! ```
//!
//! and sum to exactly `1.0`. As with the other allocators the result is a *raw*
//! weight vector — [`super::allocation::normalize_to_distribution`] converts it to
//! basis points and enforces the exploration floor.
//!
//! Fully deterministic (no RNG): the best arm is the goal-directed argmax over the
//! posterior means, ties broken by input order (first wins). Pure math.

use super::thompson::{BanditArm, GoalDirection};

/// Epsilon-greedy raw weights over `arms`.
///
/// `epsilon` is clamped to `[0, 1]`; a non-finite `epsilon` is treated as `0`
/// (pure exploitation). With `epsilon == 1` the allocation is uniform; with
/// `epsilon == 0` all exploitable mass goes to the best arm (it still keeps its
/// `1/n` share only if the floor is applied downstream). Returns weights in the
/// same order as `arms`; an empty `arms` returns an empty vec.
pub fn epsilon_greedy_weights(
    arms: &[BanditArm],
    goal: GoalDirection,
    epsilon: f64,
) -> Vec<(String, f64)> {
    if arms.is_empty() {
        return Vec::new();
    }
    let eps = if epsilon.is_finite() {
        epsilon.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let n = arms.len() as f64;
    let best = best_arm_index(arms, goal);

    arms.iter()
        .enumerate()
        .map(|(i, arm)| {
            let explore = eps / n;
            let exploit = if i == best { 1.0 - eps } else { 0.0 };
            (arm.variant_key.clone(), explore + exploit)
        })
        .collect()
}

/// Index of the goal-directed best arm by posterior mean. Ties → first arm.
pub(super) fn best_arm_index(arms: &[BanditArm], goal: GoalDirection) -> usize {
    let mut best_idx = 0;
    let mut best_val = arms[0].posterior.mean();
    for (i, arm) in arms.iter().enumerate().skip(1) {
        let v = arm.posterior.mean();
        let better = match goal {
            GoalDirection::Increase => v > best_val,
            GoalDirection::Decrease => v < best_val,
        };
        if better {
            best_val = v;
            best_idx = i;
        }
    }
    best_idx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experimentation::bandit::thompson::RewardPosterior;
    use crate::experimentation::stats::VariantStats;

    fn count_arm(key: &str, n: i64, conv: i64) -> BanditArm {
        BanditArm {
            variant_key: key.into(),
            posterior: RewardPosterior::Conversion(VariantStats {
                sample_size: n,
                conversions: Some(conv),
                mean: None,
                variance: None,
                conversion_rate: Some(conv as f64 / n as f64),
                percentiles: None,
            }),
        }
    }

    fn w(weights: &[(String, f64)], key: &str) -> f64 {
        weights.iter().find(|(k, _)| k == key).unwrap().1
    }

    #[test]
    fn empty_returns_empty() {
        assert!(epsilon_greedy_weights(&[], GoalDirection::Increase, 0.1).is_empty());
    }

    #[test]
    fn weights_sum_to_one() {
        let arms = vec![
            count_arm("a", 1000, 100),
            count_arm("b", 1000, 200),
            count_arm("c", 1000, 50),
        ];
        let weights = epsilon_greedy_weights(&arms, GoalDirection::Increase, 0.3);
        let sum: f64 = weights.iter().map(|(_, x)| x).sum();
        assert!((sum - 1.0).abs() < 1e-12, "sum={sum}");
    }

    /// Golden vector: ε=0.3, 3 arms, best=b → b = (1-ε) + ε/3, others = ε/3.
    #[test]
    fn golden_vector_eps_03_three_arms() {
        let arms = vec![
            count_arm("a", 1000, 100),
            count_arm("b", 1000, 200),
            count_arm("c", 1000, 50),
        ];
        let weights = epsilon_greedy_weights(&arms, GoalDirection::Increase, 0.3);
        let third = 0.3 / 3.0;
        assert!((w(&weights, "a") - third).abs() < 1e-12);
        assert!((w(&weights, "b") - (0.7 + third)).abs() < 1e-12);
        assert!((w(&weights, "c") - third).abs() < 1e-12);
    }

    #[test]
    fn epsilon_one_is_uniform() {
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 200)];
        let weights = epsilon_greedy_weights(&arms, GoalDirection::Increase, 1.0);
        assert!((w(&weights, "a") - 0.5).abs() < 1e-12);
        assert!((w(&weights, "b") - 0.5).abs() < 1e-12);
    }

    #[test]
    fn epsilon_zero_all_to_best() {
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 200)];
        let weights = epsilon_greedy_weights(&arms, GoalDirection::Increase, 0.0);
        assert_eq!(w(&weights, "b"), 1.0);
        assert_eq!(w(&weights, "a"), 0.0);
    }

    #[test]
    fn decrease_goal_picks_lowest() {
        let arms = vec![count_arm("low", 1000, 50), count_arm("high", 1000, 200)];
        let weights = epsilon_greedy_weights(&arms, GoalDirection::Decrease, 0.2);
        // low is best under decrease
        assert!(w(&weights, "low") > w(&weights, "high"));
    }

    #[test]
    fn epsilon_clamped_above_one() {
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 200)];
        let weights = epsilon_greedy_weights(&arms, GoalDirection::Increase, 5.0);
        // clamped to 1.0 → uniform
        assert!((w(&weights, "a") - 0.5).abs() < 1e-12);
    }

    #[test]
    fn nonfinite_epsilon_is_exploitation() {
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 200)];
        let weights = epsilon_greedy_weights(&arms, GoalDirection::Increase, f64::NAN);
        assert_eq!(w(&weights, "b"), 1.0);
    }

    #[test]
    fn tie_breaks_to_first() {
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 100)];
        let weights = epsilon_greedy_weights(&arms, GoalDirection::Increase, 0.0);
        assert_eq!(w(&weights, "a"), 1.0);
    }
}
