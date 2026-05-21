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
}
