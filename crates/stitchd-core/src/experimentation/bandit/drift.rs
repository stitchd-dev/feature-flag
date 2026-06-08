//! Reward-drift detector for autonomous optimization campaigns (FR8).
//!
//! After a campaign iteration commits to a winner, the world can change: the
//! committed winner's reward may degrade until a challenger overtakes it. A
//! campaign's drift detector watches the *committed winner's* posterior
//! probability-of-being-best and reopens exploration (spawns a fresh iteration)
//! when that probability falls **below** the configured `drift_threshold` — i.e.
//! the winner is no longer confidently the best arm.
//!
//! This reuses the same Monte-Carlo probability-best machinery as
//! [`super::convergence`] (and hence [`super::thompson::thompson_weights`]) so
//! "best" means the same thing for convergence and drift. Pure + deterministic
//! under a seed; no async, no I/O.

use super::convergence::{DEFAULT_CONVERGENCE_SAMPLES, probability_best};
use super::thompson::{BanditArm, GoalDirection};

/// The verdict of a drift check against a previously-committed winner.
#[derive(Debug, Clone, PartialEq)]
pub struct DriftVerdict {
    /// True when the committed winner has drifted: its current
    /// probability-of-being-best is **below** `drift_threshold`.
    pub drifted: bool,
    /// The committed winner's *current* probability-of-being-best.
    pub winner_prob: f64,
    /// The current leading challenger (highest prob-best among the non-winner
    /// arms) and its probability, if any other arm exists.
    pub challenger: Option<(String, f64)>,
}

/// Detect reward drift away from a previously-committed `committed_winner`.
///
/// Computes the current probability-of-being-best for every arm and reports
/// `drifted = true` when the committed winner's probability is **strictly below**
/// `drift_threshold`. The threshold is the floor of confidence below which the
/// committed winner is considered no longer trustworthy.
///
/// * Returns `drifted = false` when the committed winner is not present among the
///   current arms (nothing to compare — caller should treat as "no drift" and
///   keep the commit).
/// * Returns `drifted = false` for fewer than two arms (no challenger possible).
///
/// Deterministic under `seed`.
#[must_use]
pub fn detect_drift(
    arms: &[BanditArm],
    goal: GoalDirection,
    committed_winner: &str,
    drift_threshold: f64,
    seed: u64,
) -> DriftVerdict {
    if arms.len() < 2 {
        return DriftVerdict {
            drifted: false,
            winner_prob: 1.0,
            challenger: None,
        };
    }

    let probs = probability_best(arms, goal, DEFAULT_CONVERGENCE_SAMPLES, seed);

    let winner_prob = probs
        .iter()
        .find(|(k, _)| k == committed_winner)
        .map(|(_, p)| *p);

    // The committed winner is no longer in the arm set: nothing to drift from.
    let Some(winner_prob) = winner_prob else {
        return DriftVerdict {
            drifted: false,
            winner_prob: 0.0,
            challenger: None,
        };
    };

    // Leading challenger = highest-probability arm that is NOT the committed
    // winner (ties → first in input order).
    let challenger = probs.iter().filter(|(k, _)| k != committed_winner).fold(
        None,
        |acc: Option<(String, f64)>, (k, p)| match acc {
            Some((_, best_p)) if best_p >= *p => acc,
            _ => Some((k.clone(), *p)),
        },
    );

    DriftVerdict {
        drifted: winner_prob < drift_threshold,
        winner_prob,
        challenger,
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
    fn committed_winner_still_best_no_drift() {
        // Winner clearly still dominant: prob-best ~1.0, well above 0.5.
        let arms = vec![count_arm("win", 1000, 400), count_arm("ctrl", 1000, 100)];
        let v = detect_drift(&arms, GoalDirection::Increase, "win", 0.5, 7);
        assert!(!v.drifted, "winner still best should not drift");
        assert!(v.winner_prob > 0.5);
        assert_eq!(v.challenger.as_ref().unwrap().0, "ctrl");
    }

    #[test]
    fn challenger_overtakes_triggers_drift() {
        // The committed winner is now the WORSE arm: its prob-best collapses
        // below the 0.5 threshold → drift.
        let arms = vec![count_arm("win", 1000, 100), count_arm("rival", 1000, 400)];
        let v = detect_drift(&arms, GoalDirection::Increase, "win", 0.5, 11);
        assert!(v.drifted, "overtaken winner should drift");
        assert!(v.winner_prob < 0.5);
        assert_eq!(v.challenger.as_ref().unwrap().0, "rival");
    }

    #[test]
    fn missing_winner_no_drift() {
        // The committed winner is no longer among the arms: nothing to drift from.
        let arms = vec![count_arm("a", 1000, 100), count_arm("b", 1000, 400)];
        let v = detect_drift(&arms, GoalDirection::Increase, "gone", 0.5, 1);
        assert!(!v.drifted);
        assert_eq!(v.winner_prob, 0.0);
        assert_eq!(v.challenger, None);
    }

    #[test]
    fn single_arm_no_drift() {
        let arms = vec![count_arm("solo", 1000, 100)];
        let v = detect_drift(&arms, GoalDirection::Increase, "solo", 0.9, 1);
        assert!(!v.drifted);
        assert_eq!(v.challenger, None);
    }

    #[test]
    fn deterministic_under_seed() {
        let arms = vec![count_arm("win", 1000, 100), count_arm("rival", 1000, 400)];
        let a = detect_drift(&arms, GoalDirection::Increase, "win", 0.5, 99);
        let b = detect_drift(&arms, GoalDirection::Increase, "win", 0.5, 99);
        assert_eq!(a, b);
    }

    #[test]
    fn threshold_governs_sensitivity() {
        // A winner at moderate confidence: drifts under a strict threshold but
        // not a lenient one.
        let arms = vec![count_arm("win", 300, 45), count_arm("rival", 300, 51)];
        let strict = detect_drift(&arms, GoalDirection::Increase, "win", 0.99, 3);
        let lenient = detect_drift(&arms, GoalDirection::Increase, "win", 0.01, 3);
        assert!(strict.drifted, "strict threshold flags drift");
        assert!(!lenient.drifted, "lenient threshold tolerates it");
    }
}
