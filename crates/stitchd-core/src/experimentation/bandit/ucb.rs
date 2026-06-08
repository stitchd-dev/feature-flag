//! UCB1 (upper-confidence-bound) bandit allocator.
//!
//! Each arm gets an optimism-under-uncertainty index
//!
//! ```text
//! index[i] = direction(mean[i]) + c · sqrt( ln(total_n) / n[i] )
//! ```
//!
//! where `direction(·)` is the mean for [`GoalDirection::Increase`] and its
//! negation for [`GoalDirection::Decrease`] (so a *lower* metric value yields a
//! *higher* index when lower is better), `c > 0` is the exploration coefficient,
//! `total_n` is the summed arm counts, and `n[i]` is arm `i`'s count.
//!
//! ## Allocation rule (deterministic, documented)
//!
//! We use the classic **winner-take-all** UCB1 rule: the single max-index arm
//! receives all the raw exploitation mass (weight `1.0`), every other arm `0.0`.
//! This is the simplest deterministic UCB rule; the per-arm exploration floor is
//! applied downstream by [`super::allocation::normalize_to_distribution`], so no
//! arm is ever starved despite winner-take-all. Ties on the index break to the
//! first arm in input order.
//!
//! An arm with `n == 0` gets an **infinite** index (forced exploration: an unseen
//! arm is always tried first), matching standard UCB1.
//!
//! Pure math: no RNG, no I/O.

use super::thompson::{BanditArm, GoalDirection};

/// UCB1 raw weights over `arms`: winner-take-all on the UCB index.
///
/// `c` is the exploration coefficient; a non-finite or non-positive `c` is
/// treated as `0` (pure exploitation on the mean). Returns weights in the same
/// order as `arms`; an empty `arms` returns an empty vec.
pub fn ucb_weights(arms: &[BanditArm], goal: GoalDirection, c: f64) -> Vec<(String, f64)> {
    if arms.is_empty() {
        return Vec::new();
    }
    let best = best_index_arm(arms, goal, c);
    arms.iter()
        .enumerate()
        .map(|(i, arm)| (arm.variant_key.clone(), if i == best { 1.0 } else { 0.0 }))
        .collect()
}

/// The UCB index for arm `i` (exposed for testing / surfacing).
fn ucb_index(arm: &BanditArm, goal: GoalDirection, c: f64, total_n: f64) -> f64 {
    let n = arm.posterior.arm_n();
    let directed_mean = match goal {
        GoalDirection::Increase => arm.posterior.mean(),
        GoalDirection::Decrease => -arm.posterior.mean(),
    };
    if n == 0 {
        // Forced exploration: an unseen arm always has the highest index.
        return f64::INFINITY;
    }
    let cc = if c.is_finite() && c > 0.0 { c } else { 0.0 };
    let bonus = if total_n > 0.0 {
        cc * (total_n.ln() / n as f64).sqrt()
    } else {
        0.0
    };
    directed_mean + bonus
}

/// Index of the max-UCB-index arm. Ties (incl. multiple `+inf` unseen arms) →
/// first in input order.
fn best_index_arm(arms: &[BanditArm], goal: GoalDirection, c: f64) -> usize {
    let total_n: f64 = arms.iter().map(|a| a.posterior.arm_n() as f64).sum();
    let mut best_idx = 0;
    let mut best_val = ucb_index(&arms[0], goal, c, total_n);
    for (i, arm) in arms.iter().enumerate().skip(1) {
        let v = ucb_index(arm, goal, c, total_n);
        if v > best_val {
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
                conversion_rate: Some(if n > 0 { conv as f64 / n as f64 } else { 0.0 }),
                percentiles: None,
            }),
        }
    }

    fn w(weights: &[(String, f64)], key: &str) -> f64 {
        weights.iter().find(|(k, _)| k == key).unwrap().1
    }

    #[test]
    fn empty_returns_empty() {
        assert!(ucb_weights(&[], GoalDirection::Increase, 1.0).is_empty());
    }

    #[test]
    fn winner_take_all_sums_to_one() {
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 200)];
        let weights = ucb_weights(&arms, GoalDirection::Increase, 1.0);
        let sum: f64 = weights.iter().map(|(_, x)| x).sum();
        assert!((sum - 1.0).abs() < 1e-12);
    }

    /// With small c and big samples, the bonus is tiny → highest mean wins.
    #[test]
    fn high_mean_wins_with_small_c() {
        let arms = vec![
            count_arm("a", 100_000, 10_000),
            count_arm("b", 100_000, 30_000),
        ];
        let weights = ucb_weights(&arms, GoalDirection::Increase, 0.01);
        assert_eq!(w(&weights, "b"), 1.0);
        assert_eq!(w(&weights, "a"), 0.0);
    }

    /// An unseen arm (n=0) gets +inf index → forced exploration wins.
    #[test]
    fn zero_n_arm_forced_explored() {
        let arms = vec![count_arm("seen", 10_000, 5_000), count_arm("unseen", 0, 0)];
        let weights = ucb_weights(&arms, GoalDirection::Increase, 1.0);
        assert_eq!(w(&weights, "unseen"), 1.0);
    }

    /// Large c makes the less-sampled arm's bonus dominate.
    #[test]
    fn large_c_favors_less_sampled_arm() {
        // a: high mean, well-sampled; b: lower mean, far fewer samples.
        let arms = vec![count_arm("a", 100_000, 60_000), count_arm("b", 100, 40)];
        let small = ucb_weights(&arms, GoalDirection::Increase, 0.0001);
        assert_eq!(w(&small, "a"), 1.0); // pure exploitation → a
        let big = ucb_weights(&arms, GoalDirection::Increase, 100.0);
        assert_eq!(w(&big, "b"), 1.0); // huge bonus → less-sampled b
    }

    #[test]
    fn decrease_goal_prefers_lower_mean() {
        let arms = vec![
            count_arm("low", 100_000, 1_000),
            count_arm("high", 100_000, 50_000),
        ];
        let weights = ucb_weights(&arms, GoalDirection::Decrease, 0.001);
        assert_eq!(w(&weights, "low"), 1.0);
    }

    #[test]
    fn nonpositive_c_is_pure_exploitation() {
        let arms = vec![count_arm("a", 100, 10), count_arm("b", 100_000, 60_000)];
        let weights = ucb_weights(&arms, GoalDirection::Increase, 0.0);
        // pure mean: b (0.6) beats a (~0.107)
        assert_eq!(w(&weights, "b"), 1.0);
    }

    #[test]
    fn tie_breaks_to_first() {
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 100)];
        let weights = ucb_weights(&arms, GoalDirection::Increase, 1.0);
        assert_eq!(w(&weights, "a"), 1.0);
    }

    /// Golden vector: deterministic, repeated calls identical.
    #[test]
    fn deterministic() {
        let arms = vec![count_arm("a", 5000, 1000), count_arm("b", 5000, 1100)];
        let w1 = ucb_weights(&arms, GoalDirection::Increase, 2.0);
        let w2 = ucb_weights(&arms, GoalDirection::Increase, 2.0);
        assert_eq!(w1, w2);
    }
}
