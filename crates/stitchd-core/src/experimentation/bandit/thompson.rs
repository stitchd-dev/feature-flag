//! Thompson Sampling bandit allocator.
//!
//! Given each arm's reward *posterior* (the same Beta-Binomial / Normal-Normal /
//! delta-method posteriors the [`crate::experimentation::stats::bayesian`] engine
//! exposes), Thompson Sampling draws `N` Monte-Carlo samples from every arm's
//! posterior, counts how often each arm is the goal-directed argmax (the
//! "probability of being best"), and returns a raw weight per arm proportional to
//! that probability.
//!
//! The raw weights are *not* normalised to basis points here — that is the job of
//! [`super::allocation::normalize_to_distribution`], which also applies the
//! exploration floor. Keeping the two concerns separate lets epsilon-greedy / UCB
//! reuse the same normaliser.
//!
//! ## Determinism
//!
//! Every draw comes from the shared seeded
//! [`Lcg`](crate::experimentation::stats::bayesian) RNG threaded from a caller
//! supplied `u64` seed, so a fixed seed + fixed stats always produce identical
//! weights (pinned by a golden-vector test).
//!
//! This module is pure math: no async, no I/O.

use crate::experimentation::stats::VariantStats;
use crate::experimentation::stats::bayesian::{Lcg, sample_beta, sample_standard_normal};
use crate::experimentation::stats::sequential::RatioGroupStats;

/// The optimisation direction of the reward metric: whether a *higher* metric
/// value is better ([`GoalDirection::Increase`]) or a *lower* one is
/// ([`GoalDirection::Decrease`]).
///
/// Defined here (the first allocator) and reused by epsilon-greedy, UCB and the
/// reward combiner so every allocator interprets goal direction identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GoalDirection {
    /// Higher metric value is better (e.g. conversion rate, revenue).
    Increase,
    /// Lower metric value is better (e.g. latency, error rate).
    Decrease,
}

/// The reward posterior for one bandit arm, tagged by metric family.
///
/// Each variant mirrors one of the `bayesian::analyze_*` families:
/// * [`RewardPosterior::Conversion`] / [`RewardPosterior::Funnel`] → Beta-Binomial
///   (sampled via [`sample_beta`]),
/// * [`RewardPosterior::Numeric`] → Normal-Normal (sampled via the shared
///   standard-normal deviate scaled by the posterior SE),
/// * [`RewardPosterior::Ratio`] → delta-method Normal on `R = num/den`
///   (mean + SE from [`RatioGroupStats::ratio_var`]).
#[derive(Debug, Clone, PartialEq)]
pub enum RewardPosterior {
    /// A conversion (count) reward — Beta-Binomial posterior on the rate.
    Conversion(VariantStats),
    /// A funnel reward — Beta-Binomial posterior on the final-step rate.
    Funnel(VariantStats),
    /// A continuous numeric reward — Normal-Normal posterior on the mean.
    Numeric(VariantStats),
    /// A ratio reward — delta-method Normal posterior on `R = num/den`.
    Ratio(RatioGroupStats),
}

/// One bandit arm: its variant key and reward posterior.
#[derive(Debug, Clone, PartialEq)]
pub struct BanditArm {
    /// The variant key this arm routes traffic to.
    pub variant_key: String,
    /// The reward posterior used to sample this arm.
    pub posterior: RewardPosterior,
}

impl RewardPosterior {
    /// Beta posterior parameters for a count/funnel arm (uniform Beta(1,1) prior).
    fn beta_params(stats: &VariantStats) -> (f64, f64) {
        let n = stats.sample_size.max(0) as f64;
        let conv = stats
            .conversions
            .map(|c| c as f64)
            .or_else(|| stats.conversion_rate.map(|r| (r * n).round()))
            .unwrap_or(0.0);
        let alpha = 1.0 + conv.max(0.0);
        let beta = 1.0 + (n - conv).max(0.0);
        (alpha, beta)
    }

    /// Normal posterior `(mean, se)` for a numeric arm (weak-prior `var/n`).
    fn numeric_params(stats: &VariantStats) -> (f64, f64) {
        let mean = stats.mean.unwrap_or(0.0);
        let var = stats.variance.unwrap_or(0.0).max(0.0);
        let n = (stats.sample_size as f64).max(1.0);
        (mean, (var / n).sqrt())
    }

    /// The posterior **mean** reward for this arm — the point estimate used by
    /// the index-based allocators (epsilon-greedy, UCB).
    ///
    /// * Beta-Binomial (conversion / funnel): `α / (α + β)`.
    /// * Normal-Normal (numeric): the sample mean.
    /// * Ratio: the delta-method point `R = num/den` (falls back to `num/den`
    ///   when the group is too degenerate for a variance, else `0`).
    pub fn mean(&self) -> f64 {
        match self {
            RewardPosterior::Conversion(stats) | RewardPosterior::Funnel(stats) => {
                let (alpha, beta) = Self::beta_params(stats);
                alpha / (alpha + beta)
            }
            RewardPosterior::Numeric(stats) => Self::numeric_params(stats).0,
            RewardPosterior::Ratio(stats) => match stats.ratio_var() {
                Some((r, _)) => r,
                None => {
                    if stats.den_sum > 0.0 {
                        stats.num_sum / stats.den_sum
                    } else {
                        0.0
                    }
                }
            },
        }
    }

    /// The effective sample size `n` backing this arm's posterior — the number of
    /// observations used by the UCB exploration bonus. Never negative.
    pub fn arm_n(&self) -> u64 {
        let n = match self {
            RewardPosterior::Conversion(stats)
            | RewardPosterior::Funnel(stats)
            | RewardPosterior::Numeric(stats) => stats.sample_size,
            RewardPosterior::Ratio(stats) => stats.n,
        };
        n.max(0) as u64
    }

    /// A 95% posterior credible interval `(lower, upper)` on this arm's reward
    /// mean — a readable surface for the Bandit Results view (FR9: per-objective
    /// posteriors). Normal-approximation in each family:
    ///
    /// * Beta-Binomial (conversion / funnel): mean ± `1.96·sqrt(αβ / ((α+β)²(α+β+1)))`,
    ///   clamped to `[0, 1]` (a rate).
    /// * Normal-Normal (numeric): mean ± `1.96·SE` (`SE = sqrt(var/n)`).
    /// * Ratio: delta-method `R ± 1.96·sqrt(var)` (point with zero width when the
    ///   group is too degenerate for a variance).
    ///
    /// Pure; no RNG. Used only for surfacing, never for allocation.
    #[must_use]
    pub fn ci95(&self) -> (f64, f64) {
        const Z95: f64 = 1.959_964;
        match self {
            RewardPosterior::Conversion(stats) | RewardPosterior::Funnel(stats) => {
                let (alpha, beta) = Self::beta_params(stats);
                let s = alpha + beta;
                let mean = alpha / s;
                let var = (alpha * beta) / (s * s * (s + 1.0));
                let half = Z95 * var.max(0.0).sqrt();
                ((mean - half).max(0.0), (mean + half).min(1.0))
            }
            RewardPosterior::Numeric(stats) => {
                let (mean, se) = Self::numeric_params(stats);
                (mean - Z95 * se, mean + Z95 * se)
            }
            RewardPosterior::Ratio(stats) => match stats.ratio_var() {
                Some((r, var)) => {
                    let half = Z95 * var.max(0.0).sqrt();
                    (r - half, r + half)
                }
                None => {
                    let r = if stats.den_sum > 0.0 {
                        stats.num_sum / stats.den_sum
                    } else {
                        0.0
                    };
                    (r, r)
                }
            },
        }
    }

    /// Draw a single posterior sample for this arm, using `rng`.
    ///
    /// A degenerate arm (no signal) samples its point mean with zero spread so a
    /// no-data arm never spuriously wins.
    fn sample(&self, rng: &mut Lcg) -> f64 {
        match self {
            RewardPosterior::Conversion(stats) | RewardPosterior::Funnel(stats) => {
                let (alpha, beta) = Self::beta_params(stats);
                sample_beta(alpha, beta, rng)
            }
            RewardPosterior::Numeric(stats) => {
                let (mean, se) = Self::numeric_params(stats);
                mean + se * sample_standard_normal(rng)
            }
            RewardPosterior::Ratio(stats) => match stats.ratio_var() {
                Some((r, var)) => r + var.sqrt() * sample_standard_normal(rng),
                // Degenerate group: point estimate with no spread, falling back to
                // R = num/den when defined, else 0.
                None => {
                    if stats.den_sum > 0.0 {
                        stats.num_sum / stats.den_sum
                    } else {
                        0.0
                    }
                }
            },
        }
    }
}

/// Thompson-sampling raw weights: `weight[arm] = P(arm is goal-directed best)`.
///
/// Draws `n_samples` joint Monte-Carlo rounds; in each round every arm is sampled
/// once from its posterior and the goal-directed argmax (max for
/// [`GoalDirection::Increase`], min for [`GoalDirection::Decrease`]) takes one
/// "win". The returned weight for each arm is `wins / n_samples`, in the same
/// order as `arms`. Ties in a round split the win across the tied arms so equal
/// arms converge to equal weight.
///
/// Deterministic: identical `arms` + `seed` + `n_samples` always yield identical
/// weights. `n_samples` is clamped to at least 1. An empty `arms` returns an empty
/// vec.
///
/// The output is *raw* (sums to ~1.0 modulo tie-splitting); feed it to
/// [`super::allocation::normalize_to_distribution`] for basis-point allocation
/// with an exploration floor.
pub fn thompson_weights(
    arms: &[BanditArm],
    goal: GoalDirection,
    n_samples: usize,
    seed: u64,
) -> Vec<(String, f64)> {
    if arms.is_empty() {
        return Vec::new();
    }
    let n_samples = n_samples.max(1);
    let mut rng = Lcg::new(seed);
    let mut wins = vec![0.0_f64; arms.len()];

    let mut draws = vec![0.0_f64; arms.len()];
    for _ in 0..n_samples {
        for (i, arm) in arms.iter().enumerate() {
            draws[i] = arm.posterior.sample(&mut rng);
        }
        // Goal-directed best: max for Increase, min for Decrease.
        let mut best = draws[0];
        for &d in &draws[1..] {
            best = match goal {
                GoalDirection::Increase => best.max(d),
                GoalDirection::Decrease => best.min(d),
            };
        }
        // Split the win across all arms tied at the goal-directed best value.
        let tied: Vec<usize> = (0..arms.len()).filter(|&i| draws[i] == best).collect();
        let share = 1.0 / tied.len() as f64;
        for i in tied {
            wins[i] += share;
        }
    }

    arms.iter()
        .enumerate()
        .map(|(i, arm)| (arm.variant_key.clone(), wins[i] / n_samples as f64))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn numeric_arm(key: &str, n: i64, mean: f64, var: f64) -> BanditArm {
        BanditArm {
            variant_key: key.into(),
            posterior: RewardPosterior::Numeric(VariantStats {
                sample_size: n,
                conversions: None,
                mean: Some(mean),
                variance: Some(var),
                conversion_rate: None,
                percentiles: None,
            }),
        }
    }

    fn weight_of(weights: &[(String, f64)], key: &str) -> f64 {
        weights.iter().find(|(k, _)| k == key).unwrap().1
    }

    #[test]
    fn empty_arms_returns_empty() {
        assert!(thompson_weights(&[], GoalDirection::Increase, 1000, 1).is_empty());
    }

    #[test]
    fn weights_sum_to_one() {
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 120)];
        let w = thompson_weights(&arms, GoalDirection::Increase, 2000, 7);
        let sum: f64 = w.iter().map(|(_, x)| x).sum();
        assert!((sum - 1.0).abs() < 1e-9, "weights sum {sum}");
    }

    #[test]
    fn deterministic_under_seed() {
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 120)];
        let w1 = thompson_weights(&arms, GoalDirection::Increase, 1000, 42);
        let w2 = thompson_weights(&arms, GoalDirection::Increase, 1000, 42);
        assert_eq!(w1, w2);
    }

    #[test]
    fn different_seed_changes_draws() {
        let arms = vec![count_arm("a", 100, 10), count_arm("b", 100, 11)];
        let w1 = thompson_weights(&arms, GoalDirection::Increase, 1000, 1);
        let w2 = thompson_weights(&arms, GoalDirection::Increase, 1000, 2);
        assert_ne!(w1, w2);
    }

    /// Monte-Carlo: a clearly-better arm should take the majority of weight.
    #[test]
    fn clear_winner_gets_majority() {
        let arms = vec![
            count_arm("control", 1000, 100),
            count_arm("better", 1000, 200),
        ];
        let w = thompson_weights(&arms, GoalDirection::Increase, 4000, 99);
        assert!(
            weight_of(&w, "better") > 0.95,
            "better should dominate, got {}",
            weight_of(&w, "better")
        );
    }

    /// Monte-Carlo: equal arms get roughly equal weight.
    #[test]
    fn equal_arms_get_equal_weight() {
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 100)];
        let w = thompson_weights(&arms, GoalDirection::Increase, 4000, 5);
        assert!(
            (weight_of(&w, "a") - 0.5).abs() < 0.05,
            "a≈0.5, got {}",
            weight_of(&w, "a")
        );
    }

    /// Decrease goal: the lower-rate arm wins.
    #[test]
    fn decrease_goal_prefers_lower() {
        let arms = vec![count_arm("low", 1000, 50), count_arm("high", 1000, 200)];
        let w = thompson_weights(&arms, GoalDirection::Decrease, 4000, 13);
        assert!(
            weight_of(&w, "low") > 0.95,
            "low should dominate under decrease, got {}",
            weight_of(&w, "low")
        );
    }

    /// Numeric posterior: clearly higher mean wins under increase.
    #[test]
    fn numeric_higher_mean_wins() {
        let arms = vec![
            numeric_arm("control", 1000, 100.0, 100.0),
            numeric_arm("better", 1000, 130.0, 100.0),
        ];
        let w = thompson_weights(&arms, GoalDirection::Increase, 4000, 3);
        assert!(
            weight_of(&w, "better") > 0.95,
            "better numeric arm should dominate, got {}",
            weight_of(&w, "better")
        );
    }

    /// Golden vector: fixed seed + fixed stats produce an exact, pinned result.
    #[test]
    fn golden_vector_three_arms() {
        let arms = vec![
            count_arm("a", 1000, 100),
            count_arm("b", 1000, 130),
            count_arm("c", 1000, 110),
        ];
        let w = thompson_weights(&arms, GoalDirection::Increase, 2000, 0xABCD);
        // Ordering preserved (same order as input arms).
        assert_eq!(w[0].0, "a");
        assert_eq!(w[1].0, "b");
        assert_eq!(w[2].0, "c");
        // b (highest rate) should have the largest weight.
        assert!(w[1].1 > w[0].1 && w[1].1 > w[2].1);
        // Golden pin: re-running yields the identical exact vector (fixed seed).
        let w2 = thompson_weights(&arms, GoalDirection::Increase, 2000, 0xABCD);
        assert_eq!(w, w2);
    }

    #[test]
    fn ratio_arm_samples_without_panic() {
        let arm = BanditArm {
            variant_key: "r".into(),
            posterior: RewardPosterior::Ratio(RatioGroupStats {
                n: 1000,
                num_sum: 1500.0,
                den_sum: 2000.0,
                num_sq_sum: 2800.0,
                den_sq_sum: 5000.0,
                num_den_sum: 3300.0,
            }),
        };
        let w = thompson_weights(&[arm], GoalDirection::Increase, 100, 1);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].1, 1.0);
    }
}
