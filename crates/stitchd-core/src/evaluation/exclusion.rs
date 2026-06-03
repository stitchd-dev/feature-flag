//! Exclusion-group bucket math.
//!
//! Mutually-exclusive experiment groups need to deterministically place a
//! context into one of `[0, 9999]` buckets so disjoint basis-point ranges can
//! be carved out without overlap. This reuses the same Murmur3 reduction as
//! flag rollout allocation ([`crate::hashing::calculate_allocation`]) so group
//! bucketing stays consistent with rollout bucketing: both reduce a 128-bit
//! Murmur3 digest via `(hash % 100_000) / 10`, yielding 0.01% precision.
//!
//! Pure, in-memory math — no I/O, no async.

use murmur3::murmur3_x64_128;
use serde::{Deserialize, Serialize};
use std::io::Cursor;

/// A half-open exclusion-group bucket range `[lo, hi)` in basis points.
///
/// `lo` is inclusive, `hi` is exclusive; an empty range (`lo == hi`) contains
/// nothing. Membership delegates to [`range_contains`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BucketRange {
    /// Inclusive lower bound, in basis points.
    pub lo: u16,
    /// Exclusive upper bound, in basis points.
    pub hi: u16,
}

impl BucketRange {
    /// Returns whether `bucket` falls in this range `[lo, hi)`.
    #[must_use]
    pub fn contains(&self, bucket: u16) -> bool {
        range_contains(bucket, self.lo, self.hi)
    }
}

/// Computes the exclusion-group bucket for a context, in `[0, 9999]`.
///
/// The bucket is derived from the canonical `(hash % 100_000) / 10` reduction
/// used throughout rollout allocation, so a context placed by this function
/// lands in the same percentile space as flag percentage rollout.
///
/// Hash input is `salt` followed by `context_key`, matching the
/// salt-then-payload ordering of [`crate::hashing::calculate_allocation`]
/// (`flag_key` + `env_id` act as the salt there, then the target values).
#[must_use]
pub fn group_bucket(context_key: &str, salt: &str) -> u16 {
    let input = format!("{salt}{context_key}");
    let mut cursor = Cursor::new(input);
    let hash = murmur3_x64_128(&mut cursor, 0).unwrap_or(0);
    // Shared reduction with flag rollout allocation; result is ≤ 9999 so it
    // always fits a u16.
    u16::try_from(crate::hashing::reduce_hash_to_basis_points(hash)).unwrap_or(0)
}

/// Returns whether `bucket` falls in the range `[lo, hi)` — `lo` inclusive,
/// `hi` exclusive.
///
/// An empty range (`lo == hi`) contains nothing. The full range `[0, 10_000)`
/// contains every value [`group_bucket`] can produce.
#[must_use]
pub fn range_contains(bucket: u16, lo: u16, hi: u16) -> bool {
    bucket >= lo && bucket < hi
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn group_bucket_is_deterministic() {
        let b1 = group_bucket("user-42", "exp-salt");
        let b2 = group_bucket("user-42", "exp-salt");
        assert_eq!(b1, b2);
    }

    #[test]
    fn group_bucket_always_in_range() {
        for i in 0..2000u32 {
            let b = group_bucket(&format!("user-{i}"), "exp-salt");
            assert!(b <= 9999, "bucket {b} out of [0, 9999]");
        }
    }

    #[test]
    fn group_bucket_different_salts_spread_buckets() {
        // Sanity (not strict): two different salts over the same set of keys
        // should not collapse onto an identical bucket sequence.
        let mut seen_a = HashSet::new();
        let mut seen_b = HashSet::new();
        let mut differ = false;
        for i in 0..1000u32 {
            let key = format!("ctx-{i}");
            let ba = group_bucket(&key, "salt-a");
            let bb = group_bucket(&key, "salt-b");
            seen_a.insert(ba);
            seen_b.insert(bb);
            if ba != bb {
                differ = true;
            }
        }
        assert!(
            differ,
            "different salts produced identical buckets for all keys"
        );
        assert!(
            seen_a.len() > 900,
            "salt-a distribution too narrow: {}",
            seen_a.len()
        );
        assert!(
            seen_b.len() > 900,
            "salt-b distribution too narrow: {}",
            seen_b.len()
        );
    }

    #[test]
    fn range_contains_lo_inclusive_hi_exclusive() {
        assert!(range_contains(10, 10, 20), "lo must be inclusive");
        assert!(!range_contains(20, 10, 20), "hi must be exclusive");
        assert!(range_contains(15, 10, 20));
        assert!(!range_contains(9, 10, 20));
    }

    #[test]
    fn range_contains_full_range_holds_all_buckets() {
        for i in 0..3000u32 {
            let b = group_bucket(&format!("user-{i}"), "salt");
            assert!(range_contains(b, 0, 10_000));
        }
    }

    #[test]
    fn range_contains_empty_range_holds_nothing() {
        for x in [0u16, 1, 5000, 9999] {
            assert!(
                !range_contains(x, x, x),
                "empty range [{x},{x}) should hold nothing"
            );
        }
    }

    #[test]
    fn bucket_range_contains_matches_range_contains() {
        let r = BucketRange { lo: 10, hi: 20 };
        for b in 0u16..30 {
            assert_eq!(
                r.contains(b),
                range_contains(b, 10, 20),
                "BucketRange::contains diverged from range_contains at {b}"
            );
        }
    }

    #[test]
    fn bucket_range_serde_round_trips() {
        let r = BucketRange { lo: 2500, hi: 5000 };
        let json = serde_json::to_string(&r).unwrap();
        let back: BucketRange = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }
}
