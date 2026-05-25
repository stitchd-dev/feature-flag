//! Percentage distribution over flag variants.
//!
//! [`RolloutDistribution`] represents the fall-through configuration of a
//! feature flag — when the flag is enabled and no custom rule matches,
//! evaluation hashes the context into one of the listed allocations rather
//! than serving the single `default_variant_id`.
//!
//! Validation rules (enforced by [`RolloutDistribution::validate`]):
//! * `allocations` non-empty
//! * every `percentage` in the half-open range `(0.0, 100.0]`
//! * `variant_key` is unique across all allocations
//! * the sum of all percentages equals `100.0 ± 0.01`

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Tolerance for the percentage-sum equality check.
const SUM_TOLERANCE: f64 = 0.01;

/// Reasons a [`RolloutDistribution`] may fail validation.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum RolloutDistributionError {
    /// At least one allocation is required.
    #[error("rollout distribution must have at least one allocation")]
    Empty,
    /// One of the percentages was outside `(0.0, 100.0]`.
    #[error("allocation percentage {percentage} for variant {variant_key} must be in (0, 100]")]
    PercentageOutOfRange {
        /// The variant key whose percentage is out of range.
        variant_key: String,
        /// The bad value.
        percentage: f64,
    },
    /// The same variant key appeared more than once.
    #[error("duplicate variant_key {variant_key} in rollout distribution")]
    DuplicateVariant {
        /// The duplicated variant key.
        variant_key: String,
    },
    /// The sum of percentages did not equal 100.
    #[error("allocation percentages must sum to 100 (got {actual})")]
    SumMismatch {
        /// The observed sum.
        actual: f64,
    },
}

/// A single allocation entry in a [`RolloutDistribution`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RolloutAllocation {
    /// The variant key this allocation routes traffic to.
    pub variant_key: String,
    /// Percentage of traffic routed to this variant (in `(0, 100]`).
    pub percentage: f64,
}

/// A complete percentage distribution over variants used for the
/// default-rule fall-through.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RolloutDistribution {
    /// Ordered list of allocations.
    pub allocations: Vec<RolloutAllocation>,
}

impl RolloutDistribution {
    /// Assign a variant from this distribution given a hashed percentage in
    /// `[0.0, 100.0)`.
    ///
    /// Walks the allocations in declaration order, accumulating percentages
    /// until the running cumulative total exceeds `percentage`. Returns the
    /// `variant_key` of the matching allocation.
    ///
    /// **Hash input convention.** The caller is responsible for producing
    /// `percentage` via the same hashing primitive used by the
    /// `rule_engine::percentage` module — i.e. `calculate_allocation(flag_key,
    /// env_id_str, &target_values)` — so default-rule and percentage-rule
    /// rollouts share cohort assignment for the same context.
    ///
    /// Returns `None` only on a fully malformed distribution (validate()
    /// failed but caller persisted anyway). For a validated distribution this
    /// always returns `Some`.
    #[must_use]
    pub fn assign_variant_key(&self, percentage: f64) -> Option<&str> {
        // Clamp percentage to [0.0, 100.0); calculate_allocation already
        // returns values in this range but be defensive.
        let pct = percentage.clamp(0.0, 100.0 - f64::EPSILON);
        let mut cumulative = 0.0_f64;
        for alloc in &self.allocations {
            cumulative += alloc.percentage;
            if pct < cumulative {
                return Some(alloc.variant_key.as_str());
            }
        }
        // Fall-through guard: with a validated sum of ~100.0 this is only
        // reachable if the input percentage is at the upper bound; return the
        // last allocation.
        self.allocations.last().map(|a| a.variant_key.as_str())
    }

    /// Validate the distribution shape and return `Ok(())` if every invariant
    /// is satisfied.
    ///
    /// # Errors
    ///
    /// Returns [`RolloutDistributionError`] describing the first invariant
    /// that was not met.
    pub fn validate(&self) -> Result<(), RolloutDistributionError> {
        if self.allocations.is_empty() {
            return Err(RolloutDistributionError::Empty);
        }

        let mut seen: std::collections::HashSet<&str> =
            std::collections::HashSet::with_capacity(self.allocations.len());
        let mut sum = 0.0_f64;

        for alloc in &self.allocations {
            if !(alloc.percentage > 0.0 && alloc.percentage <= 100.0) {
                return Err(RolloutDistributionError::PercentageOutOfRange {
                    variant_key: alloc.variant_key.clone(),
                    percentage: alloc.percentage,
                });
            }
            if !seen.insert(alloc.variant_key.as_str()) {
                return Err(RolloutDistributionError::DuplicateVariant {
                    variant_key: alloc.variant_key.clone(),
                });
            }
            sum += alloc.percentage;
        }

        if (sum - 100.0).abs() > SUM_TOLERANCE {
            return Err(RolloutDistributionError::SumMismatch { actual: sum });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alloc(key: &str, pct: f64) -> RolloutAllocation {
        RolloutAllocation {
            variant_key: key.to_string(),
            percentage: pct,
        }
    }

    // ── Basis-point contract (Red — these fail until Task 1.3 lands) ────────

    fn alloc_bp(key: &str, bp: u32) -> RolloutAllocation {
        RolloutAllocation {
            variant_key: key.to_string(),
            percentage_bp: bp,
        }
    }

    #[test]
    fn validate_accepts_50_50_bp() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("control", 5000), alloc_bp("treatment", 5000)],
        };
        assert!(dist.validate().is_ok());
    }

    #[test]
    fn validate_rejects_sum_not_10000() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("a", 4000), alloc_bp("b", 3000)],
        };
        assert!(matches!(dist.validate(), Err(RolloutDistributionError::SumMismatch { actual: 7000 })));
    }

    #[test]
    fn validate_accepts_minimum_bp() {
        // 1 bp = 0.01%; 9999 + 1 = 10000
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("main", 9999), alloc_bp("canary", 1)],
        };
        assert!(dist.validate().is_ok());
    }

    #[test]
    fn validate_accepts_full_single_allocation_bp() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("only", 10000)],
        };
        assert!(dist.validate().is_ok());
    }

    #[test]
    fn validate_rejects_zero_bp() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("a", 0), alloc_bp("b", 10000)],
        };
        assert!(matches!(
            dist.validate(),
            Err(RolloutDistributionError::PercentageOutOfRange { .. })
        ));
    }

    #[test]
    fn validate_rejects_over_10000_bp() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("only", 10001)],
        };
        assert!(matches!(
            dist.validate(),
            Err(RolloutDistributionError::PercentageOutOfRange { .. })
        ));
    }

    #[test]
    fn assign_variant_key_takes_u32() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("control", 5000), alloc_bp("treatment", 5000)],
        };
        let bp: u32 = 2500;
        assert_eq!(dist.assign_variant_key(bp), Some("control"));
        let bp2: u32 = 7500;
        assert_eq!(dist.assign_variant_key(bp2), Some("treatment"));
    }

    #[test]
    fn assign_variant_key_boundary_bp() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("a", 3000), alloc_bp("b", 7000)],
        };
        // exactly at boundary: 3000 → first bucket ends, second begins
        assert_eq!(dist.assign_variant_key(2999_u32), Some("a"));
        assert_eq!(dist.assign_variant_key(3000_u32), Some("b"));
        assert_eq!(dist.assign_variant_key(9999_u32), Some("b"));
    }

    #[test]
    fn assign_variant_balanced_over_bp_range() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("a", 5000), alloc_bp("b", 5000)],
        };
        let mut a = 0u32;
        let mut b = 0u32;
        for i in 0u32..10_000 {
            match dist.assign_variant_key(i) {
                Some("a") => a += 1,
                Some("b") => b += 1,
                _ => panic!("unexpected variant"),
            }
        }
        assert_eq!(a, 5000);
        assert_eq!(b, 5000);
    }

    #[test]
    fn validate_accepts_balanced_two_variant_distribution() {
        let dist = RolloutDistribution {
            allocations: vec![alloc("control", 50.0), alloc("treatment", 50.0)],
        };
        assert!(dist.validate().is_ok());
    }

    #[test]
    fn validate_accepts_within_tolerance_sum() {
        let dist = RolloutDistribution {
            allocations: vec![alloc("a", 33.33), alloc("b", 33.33), alloc("c", 33.34)],
        };
        assert!(dist.validate().is_ok());
    }

    #[test]
    fn validate_accepts_single_full_allocation() {
        let dist = RolloutDistribution {
            allocations: vec![alloc("only", 100.0)],
        };
        assert!(dist.validate().is_ok());
    }

    #[test]
    fn validate_rejects_empty_allocations() {
        let dist = RolloutDistribution {
            allocations: vec![],
        };
        assert_eq!(dist.validate(), Err(RolloutDistributionError::Empty));
    }

    #[test]
    fn validate_rejects_zero_percentage() {
        let dist = RolloutDistribution {
            allocations: vec![alloc("a", 0.0), alloc("b", 100.0)],
        };
        assert!(matches!(
            dist.validate(),
            Err(RolloutDistributionError::PercentageOutOfRange { .. })
        ));
    }

    #[test]
    fn validate_rejects_negative_percentage() {
        let dist = RolloutDistribution {
            allocations: vec![alloc("a", -1.0), alloc("b", 101.0)],
        };
        assert!(matches!(
            dist.validate(),
            Err(RolloutDistributionError::PercentageOutOfRange { .. })
        ));
    }

    #[test]
    fn validate_rejects_over_100_percentage() {
        let dist = RolloutDistribution {
            allocations: vec![alloc("a", 101.0)],
        };
        assert!(matches!(
            dist.validate(),
            Err(RolloutDistributionError::PercentageOutOfRange { .. })
        ));
    }

    #[test]
    fn validate_rejects_duplicate_variant_key() {
        let dist = RolloutDistribution {
            allocations: vec![alloc("dup", 50.0), alloc("dup", 50.0)],
        };
        assert!(matches!(
            dist.validate(),
            Err(RolloutDistributionError::DuplicateVariant { .. })
        ));
    }

    #[test]
    fn validate_rejects_sum_below_100() {
        let dist = RolloutDistribution {
            allocations: vec![alloc("a", 30.0), alloc("b", 30.0)],
        };
        assert!(matches!(
            dist.validate(),
            Err(RolloutDistributionError::SumMismatch { .. })
        ));
    }

    #[test]
    fn validate_rejects_sum_above_100() {
        let dist = RolloutDistribution {
            allocations: vec![alloc("a", 60.0), alloc("b", 60.0)],
        };
        assert!(matches!(
            dist.validate(),
            Err(RolloutDistributionError::SumMismatch { .. })
        ));
    }

    #[test]
    fn validate_treats_tolerance_as_strict_boundary() {
        // 100 + 0.02 > tolerance → must fail.
        let dist = RolloutDistribution {
            allocations: vec![alloc("a", 50.0), alloc("b", 50.02)],
        };
        assert!(matches!(
            dist.validate(),
            Err(RolloutDistributionError::SumMismatch { .. })
        ));
    }

    // ── assign_variant_key (Phase 2 Task 2.2) ───────────────────────────────

    #[test]
    fn assign_variant_returns_first_allocation_for_low_percentage() {
        let dist = RolloutDistribution {
            allocations: vec![alloc("control", 50.0), alloc("treatment", 50.0)],
        };
        assert_eq!(dist.assign_variant_key(0.0), Some("control"));
        assert_eq!(dist.assign_variant_key(25.0), Some("control"));
        assert_eq!(dist.assign_variant_key(49.999), Some("control"));
    }

    #[test]
    fn assign_variant_returns_second_allocation_at_boundary_and_above() {
        let dist = RolloutDistribution {
            allocations: vec![alloc("control", 50.0), alloc("treatment", 50.0)],
        };
        assert_eq!(dist.assign_variant_key(50.0), Some("treatment"));
        assert_eq!(dist.assign_variant_key(75.0), Some("treatment"));
        assert_eq!(dist.assign_variant_key(99.999), Some("treatment"));
    }

    #[test]
    fn assign_variant_walks_three_way_distribution() {
        let dist = RolloutDistribution {
            allocations: vec![alloc("a", 33.3), alloc("b", 33.3), alloc("c", 33.4)],
        };
        assert_eq!(dist.assign_variant_key(10.0), Some("a"));
        assert_eq!(dist.assign_variant_key(40.0), Some("b"));
        assert_eq!(dist.assign_variant_key(70.0), Some("c"));
    }

    #[test]
    fn assign_variant_handles_single_allocation() {
        let dist = RolloutDistribution {
            allocations: vec![alloc("only", 100.0)],
        };
        assert_eq!(dist.assign_variant_key(0.0), Some("only"));
        assert_eq!(dist.assign_variant_key(50.0), Some("only"));
        assert_eq!(dist.assign_variant_key(99.999), Some("only"));
    }

    #[test]
    fn assign_variant_empty_distribution_returns_none() {
        // Defensive: an unvalidated empty distribution can't assign.
        // In practice, validate() would have rejected it before persistence.
        let dist = RolloutDistribution {
            allocations: vec![],
        };
        assert_eq!(dist.assign_variant_key(0.0), None);
    }

    #[test]
    fn assign_variant_distribution_is_balanced_over_inputs() {
        // Statistical sanity: 50/50 over evenly-spaced inputs in [0, 100)
        // produces an approximately balanced split.
        let dist = RolloutDistribution {
            allocations: vec![alloc("a", 50.0), alloc("b", 50.0)],
        };
        let mut a_count = 0;
        let mut b_count = 0;
        for i in 0..1000 {
            let pct = (i as f64) / 10.0; // 0.0, 0.1, 0.2, ..., 99.9
            match dist.assign_variant_key(pct) {
                Some("a") => a_count += 1,
                Some("b") => b_count += 1,
                _ => panic!("unexpected output"),
            }
        }
        assert_eq!(a_count, 500);
        assert_eq!(b_count, 500);
    }
}
