use crate::context::ParameterValue;
use std::io::Cursor;
use murmur3::murmur3_x64_128;

/// Computes a consistent hash for a given context parameter and maps it to a percentage.
///
/// The input to the hash is the concatenation of `context_type`, `parameter_key`, and `parameter_value`.
/// The output is a percentage between 0.000 and 100.000 with 0.1% granularity (mapped to 100,000 buckets).
pub fn compute_hash_percentage(
    context_type: &str,
    parameter_key: &str,
    parameter_value: &ParameterValue,
) -> f64 {
    let input = format!("{}{}{}", context_type, parameter_key, parameter_value);
    let mut cursor = Cursor::new(input);

    // Use Murmur3 128-bit for high-quality distribution.
    // seed is 0.
    let hash = murmur3_x64_128(&mut cursor, 0).unwrap_or(0);

    // Map to 100,000 buckets (0 to 99,999) and then to 0.000 to 99.999 percentage.
    // Note: 100,000 buckets with 100.000 max percentage means 0.001% per bucket.
    // The prompt specifies 0.1% granularity = 0 to 100,000 buckets, which is slightly ambiguous
    // but we follow the explicit bucket count.
    ((hash % 100_000) as f64) / 1000.0
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
            assert!(h >= 0.0 && h < 100.0);
            results.push(h);
        }

        // Check if we have some variety (basic check)
        let mut unique: Vec<_> = results.iter().map(|&x| (x * 1000.0) as u64).collect();
        unique.sort();
        unique.dedup();
        assert!(unique.len() > 950); // Should be very unique for 1000 inputs
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
