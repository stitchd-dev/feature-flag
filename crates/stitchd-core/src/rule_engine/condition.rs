use crate::context::ParameterValue;
use crate::id::{FlagId, SegmentId, VariantId};
use serde::{Deserialize, Serialize};

/// A single leaf condition that tests one value from an evaluation context.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Condition {
    // ── Equality / Inequality ─────────────────────────────────────────────
    /// Exact match: `context[context_type].parameters[param] == value`.
    Eq {
        context_type: String,
        param: String,
        value: ParameterValue,
    },
    /// Not-equal: `context[context_type].parameters[param] != value`.
    Ne {
        context_type: String,
        param: String,
        value: ParameterValue,
    },

    // ── Numeric Comparisons (Int, Double, SemVer) ─────────────────────────
    /// Less-than.
    Lt {
        context_type: String,
        param: String,
        value: ParameterValue,
    },
    /// Less-than-or-equal.
    Lte {
        context_type: String,
        param: String,
        value: ParameterValue,
    },
    /// Greater-than.
    Gt {
        context_type: String,
        param: String,
        value: ParameterValue,
    },
    /// Greater-than-or-equal.
    Gte {
        context_type: String,
        param: String,
        value: ParameterValue,
    },

    // ── String Operators (Str only) ───────────────────────────────────────
    /// `value.contains(substr)`.
    Contains {
        context_type: String,
        param: String,
        substr: String,
    },
    /// `value.starts_with(prefix)`.
    StartsWith {
        context_type: String,
        param: String,
        prefix: String,
    },
    /// `value.ends_with(suffix)`.
    EndsWith {
        context_type: String,
        param: String,
        suffix: String,
    },

    // ── SemVer Comparisons (SemVer only) ─────────────────────────────────
    /// `>=` — parameter version satisfies the given requirement.
    SemverGte {
        context_type: String,
        param: String,
        /// Reference version string, e.g. `"1.2.0"`.
        version: String,
    },
    /// `~` — patch-compatible (same major + minor, patch ≥ reference).
    SemverTilde {
        context_type: String,
        param: String,
        version: String,
    },
    /// `^` — minor-compatible (same major, minor+patch ≥ reference).
    SemverCaret {
        context_type: String,
        param: String,
        version: String,
    },

    // ── Segment Membership ────────────────────────────────────────────────
    /// True when the segment ID is in the resolved segments set.
    InSegment(SegmentId),
    /// True when the segment ID is **not** in the resolved segments set.
    NotInSegment(SegmentId),

    // ── Cross-Flag Conditions ─────────────────────────────────────────────
    /// True when the specified flag resolved to the specified variant.
    FlagEvaluatedAs {
        flag_id: FlagId,
        variant_id: VariantId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ParameterValue;
    use crate::id::{FlagId, SegmentId, VariantId};

    // ── Serialization roundtrips for all variants ─────────────────────────────

    #[test]
    fn eq_roundtrip() {
        let cond = Condition::Eq {
            context_type: "user".into(),
            param: "plan".into(),
            value: ParameterValue::Str("pro".into()),
        };
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn ne_roundtrip() {
        let cond = Condition::Ne {
            context_type: "user".into(),
            param: "plan".into(),
            value: ParameterValue::Bool(false),
        };
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn lt_roundtrip() {
        let cond = Condition::Lt {
            context_type: "user".into(),
            param: "age".into(),
            value: ParameterValue::Int(18),
        };
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn lte_roundtrip() {
        let cond = Condition::Lte {
            context_type: "user".into(),
            param: "age".into(),
            value: ParameterValue::Int(18),
        };
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn gt_roundtrip() {
        let cond = Condition::Gt {
            context_type: "user".into(),
            param: "score".into(),
            value: ParameterValue::Double(0.5),
        };
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn gte_roundtrip() {
        let cond = Condition::Gte {
            context_type: "user".into(),
            param: "score".into(),
            value: ParameterValue::Double(0.5),
        };
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn contains_roundtrip() {
        let cond = Condition::Contains {
            context_type: "user".into(),
            param: "email".into(),
            substr: "acme".into(),
        };
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn starts_with_roundtrip() {
        let cond = Condition::StartsWith {
            context_type: "user".into(),
            param: "name".into(),
            prefix: "Al".into(),
        };
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn ends_with_roundtrip() {
        let cond = Condition::EndsWith {
            context_type: "user".into(),
            param: "domain".into(),
            suffix: ".com".into(),
        };
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn semver_gte_roundtrip() {
        let cond = Condition::SemverGte {
            context_type: "user".into(),
            param: "version".into(),
            version: "1.2.0".into(),
        };
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn semver_tilde_roundtrip() {
        let cond = Condition::SemverTilde {
            context_type: "user".into(),
            param: "version".into(),
            version: "1.2.3".into(),
        };
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn semver_caret_roundtrip() {
        let cond = Condition::SemverCaret {
            context_type: "user".into(),
            param: "version".into(),
            version: "1.2.3".into(),
        };
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn in_segment_roundtrip() {
        let seg = SegmentId::new();
        let cond = Condition::InSegment(seg);
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn not_in_segment_roundtrip() {
        let seg = SegmentId::new();
        let cond = Condition::NotInSegment(seg);
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn flag_evaluated_as_roundtrip() {
        let flag_id = FlagId::new();
        let variant_id = VariantId::new();
        let cond = Condition::FlagEvaluatedAs { flag_id, variant_id };
        let json = serde_json::to_string(&cond).unwrap();
        let back: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, back);
    }

    #[test]
    fn condition_debug_format() {
        let cond = Condition::Eq {
            context_type: "user".into(),
            param: "plan".into(),
            value: ParameterValue::Str("pro".into()),
        };
        let debug = format!("{:?}", cond);
        assert!(debug.contains("Eq"));
        assert!(debug.contains("user"));
        assert!(debug.contains("plan"));
    }

    #[test]
    fn condition_clone_and_eq() {
        let cond = Condition::InSegment(SegmentId::new());
        let cloned = cond.clone();
        assert_eq!(cond, cloned);

        let other = Condition::InSegment(SegmentId::new());
        assert_ne!(cond, other);
    }
}
