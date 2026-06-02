use murmur3::murmur3_x64_128;
use std::io::Cursor;

/// Calculates the allocation bucket for a flag rollout based on multiple targets.
///
/// Returns a value in `[0, 9999]` representing basis points (1 = 0.01%).
/// 10,000 buckets give 0.01% allocation precision.
///
/// The bucket is derived from the canonical `hash % 100_000` reduction,
/// rescaled to basis points by dividing by 10 (`percent * 100`). This keeps every layer on one hash
/// reduction: a context that hashes to percentile 51.1% lands in bucket 5110
/// here and 511 under the legacy 0.1% reference vectors. (The earlier
/// `hash % 10_000` form was a different modulus, not a finer-grained version
/// of the same reduction, and silently re-bucketed every context.)
///
/// Canonical reduction of a 128-bit Murmur3 digest to a basis-point bucket in
/// `[0, 9999]` (`(hash % 100_000) / 10`, i.e. 0.01% precision).
///
/// This is the SINGLE definition of the bucket reduction shared by flag rollout
/// allocation ([`calculate_allocation`]) and exclusion-group bucketing
/// ([`crate::evaluation::exclusion::group_bucket`]) — keep both layers on one
/// reduction so a context's percentile placement is consistent across them.
#[must_use]
pub fn reduce_hash_to_basis_points(hash: u128) -> u32 {
    // `(x % 100_000) / 10` is at most 9999, so the conversion never truncates.
    u32::try_from((hash % 100_000) / 10).unwrap_or(0)
}

/// Hash input: `flag_key` + `env_id` + concatenated target values.
pub fn calculate_allocation(flag_key: &str, env_id: &str, targets: &[String]) -> u32 {
    let mut input = format!("{}{}", flag_key, env_id);
    for t in targets {
        input.push_str(t);
    }
    let mut cursor = Cursor::new(input);
    let hash = murmur3_x64_128(&mut cursor, 0).unwrap_or(0);
    reduce_hash_to_basis_points(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Basis-point contract (Red — these fail until Task 1.2 lands) ────────

    #[test]
    fn calculate_allocation_returns_u32_in_bp_range() {
        // calculate_allocation must return u32 in [0, 9999]
        let bp: u32 = calculate_allocation("my-flag", "env-abc", &["user-1".to_string()]);
        assert!(bp < 10_000, "basis-point value must be < 10000, got {bp}");
    }

    #[test]
    fn calculate_allocation_is_deterministic_as_u32() {
        let v1: u32 = calculate_allocation("flag", "env", &["ctx".to_string()]);
        let v2: u32 = calculate_allocation("flag", "env", &["ctx".to_string()]);
        assert_eq!(v1, v2);
    }

    #[test]
    fn calculate_allocation_covers_bp_range() {
        // 1000 distinct inputs should produce varied u32 outputs across [0, 9999]
        let mut seen = std::collections::HashSet::new();
        for i in 0..1000u32 {
            let bp: u32 = calculate_allocation("flag", "env", &[i.to_string()]);
            seen.insert(bp);
        }
        assert!(
            seen.len() > 900,
            "expected varied distribution, got {} unique values",
            seen.len()
        );
    }
}
