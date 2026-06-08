//! Raw-weight → basis-point normalisation with a minimum-exploration floor.
//!
//! Every bandit allocator ([`super::thompson`], [`super::epsilon`],
//! [`super::ucb`]) produces a *raw* `f64` weight per arm. This module converts
//! those into a [`RolloutDistribution`] whose `percentage_bp` values:
//!
//! * sum to **exactly** `10_000`,
//! * give **every** arm at least `min_exploration_bp` (the exploration floor —
//!   even a zero-weight arm), and
//! * are otherwise proportional to the raw weights.
//!
//! ## Algorithm (largest-remainder / Hamilton apportionment)
//!
//! 1. Reserve the floor: `reserved = min_exploration_bp · n`.
//! 2. Distribute `remaining = 10_000 − reserved` proportionally to the
//!    (non-negative) raw weights. Each arm's ideal extra share is
//!    `remaining · wᵢ / Σw`; its integer base is `floor(ideal)`.
//! 3. The leftover bp (`remaining − Σ floor(ideal)`) is handed out one bp at a
//!    time to the arms with the **largest fractional remainder** (ties broken by
//!    input order), so the total lands on exactly `10_000`.
//!
//! When every raw weight is ≤ 0 (or non-finite), `remaining` is split as evenly
//! as possible across arms (again largest-remainder), so the distribution stays
//! well-defined.
//!
//! Pure math: no RNG, no I/O.

use crate::rollout::{RolloutAllocation, RolloutDistribution};

/// Error normalising raw bandit weights into a basis-point distribution.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NormalizeError {
    /// No arms were supplied.
    #[error("cannot normalise an empty set of arms")]
    NoArms,
    /// The exploration floor cannot be honoured for every arm:
    /// `min_exploration_bp · n` exceeds 10_000.
    #[error(
        "min_exploration_bp {min_exploration_bp} × {n} arms = {required} exceeds 10000 total bp"
    )]
    FloorTooHigh {
        /// The requested per-arm floor.
        min_exploration_bp: u32,
        /// The number of arms.
        n: usize,
        /// The total bp the floor would require (`min_exploration_bp · n`).
        required: u64,
    },
}

/// Normalise `raw_weights` into a [`RolloutDistribution`] summing to exactly
/// `10_000` bp with every arm guaranteed at least `min_exploration_bp`.
///
/// Variant order is preserved. See the module docs for the largest-remainder
/// algorithm.
///
/// # Errors
///
/// * [`NormalizeError::NoArms`] if `raw_weights` is empty.
/// * [`NormalizeError::FloorTooHigh`] if `min_exploration_bp · n > 10_000`.
pub fn normalize_to_distribution(
    raw_weights: &[(String, f64)],
    min_exploration_bp: u32,
) -> Result<RolloutDistribution, NormalizeError> {
    let n = raw_weights.len();
    if n == 0 {
        return Err(NormalizeError::NoArms);
    }
    let required = min_exploration_bp as u64 * n as u64;
    if required > 10_000 {
        return Err(NormalizeError::FloorTooHigh {
            min_exploration_bp,
            n,
            required,
        });
    }

    let floor = min_exploration_bp;
    let remaining: u32 = 10_000 - floor * n as u32;

    // Sanitised non-negative, finite weights.
    let weights: Vec<f64> = raw_weights
        .iter()
        .map(|(_, w)| if w.is_finite() && *w > 0.0 { *w } else { 0.0 })
        .collect();
    let total: f64 = weights.iter().sum();

    // Ideal extra share per arm (proportional, or uniform when no positive mass).
    let ideals: Vec<f64> = if total > 0.0 {
        weights
            .iter()
            .map(|w| remaining as f64 * w / total)
            .collect()
    } else {
        let even = remaining as f64 / n as f64;
        vec![even; n]
    };

    // Integer base = floor(ideal); track fractional remainders.
    let mut bp: Vec<u32> = ideals.iter().map(|x| x.floor() as u32).collect();
    let assigned: u32 = bp.iter().sum();
    let mut leftover = remaining - assigned;

    // Hand out leftover bp by largest fractional remainder, ties → input order.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let fa = ideals[a] - ideals[a].floor();
        let fb = ideals[b] - ideals[b].floor();
        // Descending remainder; stable on equal remainder preserves input order
        // because `a < b` for equal keys under a stable sort.
        fb.partial_cmp(&fa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(&b))
    });
    let mut k = 0usize;
    while leftover > 0 {
        bp[order[k % n]] += 1;
        leftover -= 1;
        k += 1;
    }

    // Add the reserved floor to every arm.
    let allocations = raw_weights
        .iter()
        .enumerate()
        .map(|(i, (key, _))| RolloutAllocation {
            variant_key: key.clone(),
            percentage_bp: bp[i] + floor,
        })
        .collect();

    Ok(RolloutDistribution { allocations })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(pairs: &[(&str, f64)]) -> Vec<(String, f64)> {
        pairs.iter().map(|(k, w)| (k.to_string(), *w)).collect()
    }

    fn sum_bp(d: &RolloutDistribution) -> u32 {
        d.allocations.iter().map(|a| a.percentage_bp).sum()
    }

    fn bp_of(d: &RolloutDistribution, key: &str) -> u32 {
        d.allocations
            .iter()
            .find(|a| a.variant_key == key)
            .unwrap()
            .percentage_bp
    }

    #[test]
    fn empty_is_error() {
        assert_eq!(
            normalize_to_distribution(&[], 0),
            Err(NormalizeError::NoArms)
        );
    }

    #[test]
    fn floor_too_high_is_error() {
        // 3 arms × 4000 = 12000 > 10000
        let w = raw(&[("a", 1.0), ("b", 1.0), ("c", 1.0)]);
        assert!(matches!(
            normalize_to_distribution(&w, 4000),
            Err(NormalizeError::FloorTooHigh {
                required: 12000,
                ..
            })
        ));
    }

    #[test]
    fn floor_exactly_fills_is_ok() {
        // 2 arms × 5000 = 10000 exactly → all mass is floor, even split.
        let w = raw(&[("a", 9.0), ("b", 1.0)]);
        let d = normalize_to_distribution(&w, 5000).unwrap();
        assert_eq!(sum_bp(&d), 10_000);
        assert_eq!(bp_of(&d, "a"), 5000);
        assert_eq!(bp_of(&d, "b"), 5000);
    }

    #[test]
    fn single_arm_gets_everything() {
        let w = raw(&[("only", 0.3)]);
        let d = normalize_to_distribution(&w, 0).unwrap();
        assert_eq!(sum_bp(&d), 10_000);
        assert_eq!(bp_of(&d, "only"), 10_000);
    }

    #[test]
    fn proportional_split_no_floor() {
        // 75/25 split → 7500/2500.
        let w = raw(&[("a", 0.75), ("b", 0.25)]);
        let d = normalize_to_distribution(&w, 0).unwrap();
        assert_eq!(sum_bp(&d), 10_000);
        assert_eq!(bp_of(&d, "a"), 7500);
        assert_eq!(bp_of(&d, "b"), 2500);
    }

    #[test]
    fn zero_weight_arm_still_gets_floor() {
        let w = raw(&[("a", 1.0), ("dead", 0.0)]);
        let d = normalize_to_distribution(&w, 500).unwrap();
        assert_eq!(sum_bp(&d), 10_000);
        assert_eq!(bp_of(&d, "dead"), 500); // floor only
        assert_eq!(bp_of(&d, "a"), 9500);
    }

    #[test]
    fn floor_applied_to_every_arm() {
        let w = raw(&[("a", 5.0), ("b", 3.0), ("c", 2.0)]);
        let floor = 1000;
        let d = normalize_to_distribution(&w, floor).unwrap();
        assert_eq!(sum_bp(&d), 10_000);
        for a in &d.allocations {
            assert!(a.percentage_bp >= floor, "{} below floor", a.variant_key);
        }
    }

    #[test]
    fn rounding_drift_fixed_three_equal() {
        // 3 equal arms, no floor: 3333+3333+3334 = 10000 (largest remainder).
        let w = raw(&[("a", 1.0), ("b", 1.0), ("c", 1.0)]);
        let d = normalize_to_distribution(&w, 0).unwrap();
        assert_eq!(sum_bp(&d), 10_000);
        let bps: Vec<u32> = d.allocations.iter().map(|a| a.percentage_bp).collect();
        // Each within 1 bp of even.
        for v in &bps {
            assert!(*v == 3333 || *v == 3334, "got {v}");
        }
    }

    #[test]
    fn all_zero_weights_split_evenly() {
        let w = raw(&[("a", 0.0), ("b", 0.0), ("c", 0.0)]);
        let d = normalize_to_distribution(&w, 0).unwrap();
        assert_eq!(sum_bp(&d), 10_000);
    }

    #[test]
    fn nonfinite_weights_treated_as_zero() {
        let w = raw(&[("a", f64::NAN), ("b", f64::INFINITY), ("c", 1.0)]);
        let d = normalize_to_distribution(&w, 100).unwrap();
        assert_eq!(sum_bp(&d), 10_000);
        // c is the only positive finite weight → it takes the lion's share.
        assert!(bp_of(&d, "c") > bp_of(&d, "a"));
        assert!(bp_of(&d, "c") > bp_of(&d, "b"));
        assert_eq!(bp_of(&d, "a"), 100); // floor only
        assert_eq!(bp_of(&d, "b"), 100); // floor only
    }

    // ── Property-style exhaustive checks ──────────────────────────────────────

    #[test]
    fn property_sum_is_always_exactly_10000() {
        // Sweep arm counts, floors, and a variety of weight vectors.
        let weight_sets: Vec<Vec<f64>> = vec![
            vec![1.0],
            vec![1.0, 1.0],
            vec![0.9, 0.1],
            vec![0.5, 0.3, 0.2],
            vec![0.0, 0.0, 1.0],
            vec![1.0, 2.0, 3.0, 4.0],
            vec![0.001, 0.999],
            vec![0.333, 0.333, 0.334],
            vec![7.0, 7.0, 7.0, 7.0, 7.0, 7.0, 7.0],
            vec![0.0; 5],
        ];
        for ws in &weight_sets {
            let n = ws.len();
            let pairs: Vec<(String, f64)> = ws
                .iter()
                .enumerate()
                .map(|(i, w)| (format!("v{i}"), *w))
                .collect();
            // Try every floor that fits.
            let max_floor = (10_000 / n as u32).min(5000);
            for floor in [0u32, 1, 50, 100, 500, max_floor] {
                if floor as u64 * n as u64 > 10_000 {
                    continue;
                }
                let d = normalize_to_distribution(&pairs, floor).unwrap();
                assert_eq!(
                    sum_bp(&d),
                    10_000,
                    "sum != 10000 for ws={ws:?} floor={floor}"
                );
                for a in &d.allocations {
                    assert!(
                        a.percentage_bp >= floor,
                        "{} below floor {floor} for ws={ws:?}",
                        a.variant_key
                    );
                }
            }
        }
    }

    #[test]
    fn property_monotonic_higher_weight_ge_bp() {
        // With no floor and strictly increasing weights, bp is non-decreasing.
        let w = raw(&[("a", 1.0), ("b", 2.0), ("c", 3.0), ("d", 10.0)]);
        let d = normalize_to_distribution(&w, 0).unwrap();
        assert!(bp_of(&d, "a") <= bp_of(&d, "b"));
        assert!(bp_of(&d, "b") <= bp_of(&d, "c"));
        assert!(bp_of(&d, "c") <= bp_of(&d, "d"));
    }

    #[test]
    fn many_arms_each_gets_floor() {
        // 20 arms, floor 100 → 2000 reserved, 8000 distributed.
        let pairs: Vec<(String, f64)> = (0..20).map(|i| (format!("v{i}"), 1.0)).collect();
        let d = normalize_to_distribution(&pairs, 100).unwrap();
        assert_eq!(sum_bp(&d), 10_000);
        for a in &d.allocations {
            assert!(a.percentage_bp >= 100);
        }
    }

    #[test]
    fn output_validates_as_rollout_when_floor_positive() {
        // With a positive floor every arm is > 0, so the standard
        // RolloutDistribution::validate passes.
        let w = raw(&[("a", 5.0), ("b", 0.0), ("c", 2.0)]);
        let d = normalize_to_distribution(&w, 100).unwrap();
        assert!(
            d.validate().is_ok(),
            "should be a valid rollout distribution"
        );
    }

    #[test]
    fn order_is_preserved() {
        let w = raw(&[("z", 1.0), ("a", 2.0), ("m", 3.0)]);
        let d = normalize_to_distribution(&w, 0).unwrap();
        let keys: Vec<&str> = d
            .allocations
            .iter()
            .map(|a| a.variant_key.as_str())
            .collect();
        assert_eq!(keys, vec!["z", "a", "m"]);
    }
}
