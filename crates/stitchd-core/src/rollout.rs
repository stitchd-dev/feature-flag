//! Percentage distribution over flag variants.
//!
//! [`RolloutDistribution`] represents the fall-through configuration of a
//! feature flag — when the flag is enabled and no custom rule matches,
//! evaluation hashes the context into one of the listed allocations rather
//! than serving the single `default_variant_id`.
//!
//! Validation rules (enforced by [`RolloutDistribution::validate`]):
//! * `allocations` non-empty
//! * every `percentage_bp` in the range `(0, 10_000]` (1 = 0.01%, 10000 = 100%)
//! * `variant_key` is unique across all allocations
//! * the sum of all `percentage_bp` values equals exactly `10_000`

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Reasons a [`RolloutDistribution`] may fail validation.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum RolloutDistributionError {
    /// At least one allocation is required.
    #[error("rollout distribution must have at least one allocation")]
    Empty,
    /// One of the basis-point values was outside `(0, 10_000]`.
    #[error("allocation percentage_bp {percentage_bp} for variant {variant_key} must be in (0, 10000]")]
    PercentageOutOfRange {
        /// The variant key whose basis-point value is out of range.
        variant_key: String,
        /// The bad value in basis points.
        percentage_bp: u32,
    },
    /// The same variant key appeared more than once.
    #[error("duplicate variant_key {variant_key} in rollout distribution")]
    DuplicateVariant {
        /// The duplicated variant key.
        variant_key: String,
    },
    /// The sum of basis-point values did not equal 10_000.
    #[error("allocation basis points must sum to 10000 (got {actual})")]
    SumMismatch {
        /// The observed sum.
        actual: u32,
    },
}

/// A single allocation entry in a [`RolloutDistribution`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RolloutAllocation {
    /// The variant key this allocation routes traffic to.
    pub variant_key: String,
    /// Basis points of traffic routed to this variant (1 = 0.01%, 10000 = 100%).
    pub percentage_bp: u32,
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
    /// Assign a variant given a basis-point bucket in `[0, 9999]` from
    /// [`crate::hashing::calculate_allocation`].
    ///
    /// Walks allocations in declaration order, accumulating `percentage_bp`
    /// values until the running total exceeds `bp`. Returns the `variant_key`
    /// of the matching bucket.
    ///
    /// Returns `None` only on an empty (invalid) distribution.
    #[must_use]
    pub fn assign_variant_key(&self, bp: u32) -> Option<&str> {
        let mut cumulative: u32 = 0;
        for alloc in &self.allocations {
            cumulative += alloc.percentage_bp;
            if bp < cumulative {
                return Some(alloc.variant_key.as_str());
            }
        }
        // Fall-through guard: reachable only when bp == 9999 and cumulative
        // sum is exactly 10000 (validated). Return last allocation.
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
        let mut sum: u32 = 0;

        for alloc in &self.allocations {
            if alloc.percentage_bp == 0 || alloc.percentage_bp > 10_000 {
                return Err(RolloutDistributionError::PercentageOutOfRange {
                    variant_key: alloc.variant_key.clone(),
                    percentage_bp: alloc.percentage_bp,
                });
            }
            if !seen.insert(alloc.variant_key.as_str()) {
                return Err(RolloutDistributionError::DuplicateVariant {
                    variant_key: alloc.variant_key.clone(),
                });
            }
            sum += alloc.percentage_bp;
        }

        if sum != 10_000 {
            return Err(RolloutDistributionError::SumMismatch { actual: sum });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            allocations: vec![alloc_bp("control", 5000), alloc_bp("treatment", 5000)],
        };
        assert!(dist.validate().is_ok());
    }

    #[test]
    fn validate_accepts_three_way_distribution() {
        // 3333 + 3333 + 3334 = 10000
        let dist = RolloutDistribution {
            allocations: vec![
                alloc_bp("a", 3333),
                alloc_bp("b", 3333),
                alloc_bp("c", 3334),
            ],
        };
        assert!(dist.validate().is_ok());
    }

    #[test]
    fn validate_accepts_single_full_allocation() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("only", 10000)],
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
    fn validate_rejects_duplicate_variant_key() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("dup", 5000), alloc_bp("dup", 5000)],
        };
        assert!(matches!(
            dist.validate(),
            Err(RolloutDistributionError::DuplicateVariant { .. })
        ));
    }

    #[test]
    fn validate_rejects_sum_below_10000() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("a", 3000), alloc_bp("b", 3000)],
        };
        assert!(matches!(
            dist.validate(),
            Err(RolloutDistributionError::SumMismatch { actual: 6000 })
        ));
    }

    #[test]
    fn validate_rejects_sum_above_10000() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("a", 6000), alloc_bp("b", 6000)],
        };
        assert!(matches!(
            dist.validate(),
            Err(RolloutDistributionError::SumMismatch { actual: 12000 })
        ));
    }

    #[test]
    fn assign_variant_returns_first_allocation_for_low_bp() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("control", 5000), alloc_bp("treatment", 5000)],
        };
        assert_eq!(dist.assign_variant_key(0), Some("control"));
        assert_eq!(dist.assign_variant_key(2500), Some("control"));
        assert_eq!(dist.assign_variant_key(4999), Some("control"));
    }

    #[test]
    fn assign_variant_returns_second_allocation_at_boundary() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("control", 5000), alloc_bp("treatment", 5000)],
        };
        assert_eq!(dist.assign_variant_key(5000), Some("treatment"));
        assert_eq!(dist.assign_variant_key(7500), Some("treatment"));
        assert_eq!(dist.assign_variant_key(9999), Some("treatment"));
    }

    #[test]
    fn assign_variant_walks_three_way_distribution() {
        let dist = RolloutDistribution {
            allocations: vec![
                alloc_bp("a", 3333),
                alloc_bp("b", 3333),
                alloc_bp("c", 3334),
            ],
        };
        assert_eq!(dist.assign_variant_key(1000), Some("a"));
        assert_eq!(dist.assign_variant_key(4000), Some("b"));
        assert_eq!(dist.assign_variant_key(7000), Some("c"));
    }

    #[test]
    fn assign_variant_handles_single_allocation() {
        let dist = RolloutDistribution {
            allocations: vec![alloc_bp("only", 10000)],
        };
        assert_eq!(dist.assign_variant_key(0), Some("only"));
        assert_eq!(dist.assign_variant_key(5000), Some("only"));
        assert_eq!(dist.assign_variant_key(9999), Some("only"));
    }

    #[test]
    fn assign_variant_empty_distribution_returns_none() {
        let dist = RolloutDistribution { allocations: vec![] };
        assert_eq!(dist.assign_variant_key(0), None);
    }
}
