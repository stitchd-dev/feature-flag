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
