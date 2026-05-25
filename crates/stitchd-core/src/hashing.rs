use crate::context::ParameterValue;
use murmur3::murmur3_x64_128;
use std::io::Cursor;

/// Computes a consistent hash for a given context parameter and maps it to a percentage.
///
/// The input to the hash is the concatenation of `context_type`, `parameter_key`, and `parameter_value`.
/// The output is a percentage between 0.000 and 100.000 with 0.1% granularity (mapped to 100,000 buckets).
/// Used for segment membership checks — returns `f64` for compatibility with segment rule evaluation.
pub fn compute_hash_percentage(
    context_type: &str,
    parameter_key: &str,
    parameter_value: &ParameterValue,
) -> f64 {
    let input = format!("{}{}{}", context_type, parameter_key, parameter_value);
    let mut cursor = Cursor::new(input);
    let hash = murmur3_x64_128(&mut cursor, 0).unwrap_or(0);
    ((hash % 100_000) as f64) / 1000.0
}

/// Calculates the allocation bucket for a flag rollout based on multiple targets.
///
/// Returns a value in `[0, 9999]` representing basis points (1 = 0.01%).
/// 10,000 buckets give 0.01% allocation precision.
///
/// Hash input: `flag_key` + `env_id` + concatenated target values.
pub fn calculate_allocation(flag_key: &str, env_id: &str, targets: &[String]) -> u32 {
    let mut input = format!("{}{}", flag_key, env_id);
    for t in targets {
        input.push_str(t);
    }
    let mut cursor = Cursor::new(input);
    let hash = murmur3_x64_128(&mut cursor, 0).unwrap_or(0);
    (hash % 10_000) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ParameterValue;

    #[test]
    fn test_compute_hash_percentage_determinism() {
        let val = ParameterValue::Str("user-123".to_string());
        let h1 = compute_hash_percentage("user", "id", &val);
        let h2 = compute_hash_percentage("user", "id", &val);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_compute_hash_percentage_distribution() {
        let mut results = Vec::new();
        for i in 0..1000 {
            let val = ParameterValue::Int(i);
            let h = compute_hash_percentage("user", "id", &val);
            assert!((0.0..100.0).contains(&h));
            results.push(h);
        }

        // Check variety (basic check)
        let mut unique: Vec<_> = results.iter().map(|&x| (x * 1000.0) as u64).collect();
        unique.sort();
        unique.dedup();
        assert!(unique.len() > 950);
    }

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
        assert!(seen.len() > 900, "expected varied distribution, got {} unique values", seen.len());
    }

    #[test]
    fn test_different_inputs_produce_different_hashes() {
        let val1 = ParameterValue::Str("user-1".to_string());
        let val2 = ParameterValue::Str("user-2".to_string());

        let h1 = compute_hash_percentage("user", "id", &val1);
        let h2 = compute_hash_percentage("user", "id", &val2);
        assert_ne!(h1, h2);

        let h3 = compute_hash_percentage("user", "other", &val1);
        assert_ne!(h1, h3);

        let h4 = compute_hash_percentage("other", "id", &val1);
        assert_ne!(h1, h4);
    }
}
