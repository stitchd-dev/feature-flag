//! Foundation types for the unified `evaluate_flag` entry point.
//!
//! These types are the canonical inputs and outputs shared by every caller of
//! variant evaluation (the flag service's preview path and the Rust SDK).
//! They live in `stitchd-core` so both consumers can speak the same shape
//! without duplicating orchestration logic.
//!
//! See the `flag_eval_unify_20260522` track for the broader design.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::id::{FlagId, SegmentId};
use crate::variants::VariantValue;

use super::preview::{RolloutDebug, RuleTrace};

// ── Percentage-hash input schema ─────────────────────────────────────────────

/// Selector for one component of a percentage-allocation hash input.
///
/// A [`HashInputSpec`] is an ordered list of these selectors; each selector
/// resolves against the evaluation context bundle to produce a string value
/// that contributes to the hash input. Selectors that fail to resolve
/// (missing context type or missing parameter) contribute a deterministic
/// empty-string sentinel — the existing preview behaviour is preserved so
/// hash buckets remain stable across the migration.
///
/// The selector list supports **cross-context hashing**: a single
/// `HashInputSpec` may pull values from multiple context types in one
/// evaluation, e.g. `user.key` + `user.params.name` + `device.params.os` +
/// `application.key`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HashSelector {
    /// Hash the `key` of the context with the given `context_type`, if present.
    ContextKey {
        /// The context type whose `key` should be included in the hash.
        context_type: String,
    },
    /// Hash a named parameter of the context with the given `context_type`.
    ContextParameter {
        /// The context type to look up.
        context_type: String,
        /// The parameter name within that context.
        parameter: String,
    },
}

/// Ordered specification of which context fields contribute to a
/// percentage-allocation hash.
///
/// **Selector order is significant** for hash stability — re-ordering the
/// selector list will produce a different bucket assignment for the same
/// context bundle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct HashInputSpec {
    /// Ordered list of selectors. Empty lists are invalid; validation lives
    /// at the boundary that accepts user-authored configs (the REST DTO and
    /// PG schema CHECK constraint).
    pub selectors: Vec<HashSelector>,
}

impl HashInputSpec {
    /// Construct a new `HashInputSpec` wrapping the given selector list.
    #[must_use]
    pub const fn new(selectors: Vec<HashSelector>) -> Self {
        Self { selectors }
    }

    /// Returns `true` if the selector list is empty.
    ///
    /// An empty spec is invalid at the API boundary but the type itself
    /// allows construction so test fixtures and error-path tests can build
    /// one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.selectors.is_empty()
    }

    /// Returns the number of selectors in the spec.
    #[must_use]
    pub fn len(&self) -> usize {
        self.selectors.len()
    }
}

// ── Trace level ──────────────────────────────────────────────────────────────

/// Trace-detail level requested by the caller of [`evaluate_flag`].
///
/// `Off` is the hot-path mode used by the Rust SDK — no trace structures are
/// allocated and [`FlagEvaluationResult::trace`] is always `None`.
///
/// `Full` is the rich mode used by the flag service's evaluate-preview path
/// and by SDK consumers calling the explicit reasoning entry point. Every
/// rule and condition outcome is captured, plus per-result rollout-debug
/// detail.
///
/// [`evaluate_flag`]: crate::evaluation::engine::evaluate_flag
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceLevel {
    /// No trace artifacts. Hot-path default.
    Off,
    /// Full rule + condition + rollout-debug trace.
    Full,
}

// ── List-segment membership index ────────────────────────────────────────────

/// Per-context list-segment membership lookup.
///
/// Maps `(context_type, context_key)` to the set of list-based segment IDs
/// that contain that context. The caller of [`evaluate_flag`] is responsible
/// for populating this from whichever storage layer is appropriate
/// (ScyllaDB batch lookup for the flag service, LFU membership cache for
/// the SDK). The unified core evaluator does no I/O; this index is the
/// pre-computed answer.
///
/// [`evaluate_flag`]: crate::evaluation::engine::evaluate_flag
#[derive(Debug, Clone, Default)]
pub struct ListMembershipIndex {
    inner: HashMap<(String, String), HashSet<SegmentId>>,
}

impl ListMembershipIndex {
    /// Construct an empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a membership entry for `(context_type, context_key)`.
    pub fn insert(
        &mut self,
        context_type: impl Into<String>,
        context_key: impl Into<String>,
        segments: HashSet<SegmentId>,
    ) {
        self.inner
            .insert((context_type.into(), context_key.into()), segments);
    }

    /// Look up the list-segment membership for the given context.
    ///
    /// Returns `None` if the caller has not registered a membership entry
    /// for this context, which the evaluator treats as "no list segments
    /// match" — equivalent to an empty set.
    #[must_use]
    pub fn get(&self, context_type: &str, context_key: &str) -> Option<&HashSet<SegmentId>> {
        // Use a borrowed key tuple via HashMap's flexible Borrow impl —
        // here we just allocate the tuple to keep the type signature simple
        // and contained. Cost is negligible against the dominant work.
        self.inner
            .get(&(context_type.to_owned(), context_key.to_owned()))
    }

    /// Returns `true` if no membership entries have been registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Returns the number of registered `(context_type, context_key)`
    /// entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

// ── Evaluation outcome + result ──────────────────────────────────────────────

/// Outcome category for a single context's evaluation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvalOutcome {
    /// Flag was disabled; the default variant was returned.
    FlagDisabled,
    /// A custom rule matched at the given positional index.
    RuleMatch {
        /// Zero-based index of the rule in the flag's rule list.
        rule_index: usize,
    },
    /// No custom rule matched; the flag's `default_variant_id` was returned.
    DefaultFallthrough,
    /// No custom rule matched; a variant was selected from the flag's
    /// `default_rule_distribution` via hash-based assignment.
    DefaultRuleDistribution,
    /// A prerequisite flag did not resolve to its required variant (or was
    /// disabled / absent). The flag's rules were skipped and the configured
    /// fallback variant (or off/disabled default) was returned.
    PrerequisiteFailed {
        /// The first prerequisite flag whose gate check failed.
        prerequisite_flag_id: FlagId,
    },
}

/// Trace detail recorded when the prerequisite gate fails for a flag.
///
/// Names the failing prerequisite flag and the fallback variant that was
/// returned in place of the flag's own rule evaluation. Carries no
/// context/parameter values, so it never leaks `privateParameters`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PrerequisiteFailureTrace {
    /// The prerequisite flag whose gate check failed (the first failing one
    /// when multiple prerequisites are configured).
    pub prerequisite_flag_id: FlagId,
    /// The variant the prerequisite flag was required to resolve to for the
    /// gate to pass.
    pub required_variant_id: crate::id::VariantId,
    /// The variant the prerequisite flag actually resolved to, if any.
    /// `None` means the prerequisite flag was disabled or absent from the
    /// resolved cross-flag map (both treated as "unmet").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_variant_id: Option<crate::id::VariantId>,
    /// The `key` of the fallback variant that was returned.
    pub fallback_variant_key: String,
}

/// Bundle of trace artifacts returned when [`TraceLevel::Full`] is requested.
///
/// Reuses the existing [`RuleTrace`] / [`RolloutDebug`] types from
/// [`crate::evaluation::preview`] so the preview rewire in Phase 5 produces a
/// byte-equivalent response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvaluationTrace {
    /// One entry per rule in the flag's rule list, in declaration order.
    /// Captures matched, no-match, and skipped (post-match) outcomes.
    pub rule_traces: Vec<RuleTrace>,
    /// Rollout-debug detail for the matched percentage rule (or
    /// default-rule-distribution path). `None` when the outcome did not
    /// involve hash-based assignment.
    pub rollout_debug: Option<RolloutDebug>,
    /// UUID of the matched rule, if any — distinct from
    /// `EvalOutcome::RuleMatch.rule_index` because the index is positional
    /// while the UUID is the stable identifier used by experiment routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_rule_id: Option<crate::id::RuleId>,
    /// Human-friendly rule name, if the matched rule has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_rule_name: Option<String>,
    /// Populated when the prerequisite gate failed: names the failing
    /// prerequisite flag and the fallback variant taken. `None` when the gate
    /// passed (or no prerequisites are configured).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerequisite_failure: Option<PrerequisiteFailureTrace>,
}

/// Per-context result returned by [`evaluate_flag`].
///
/// [`evaluate_flag`]: crate::evaluation::engine::evaluate_flag
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlagEvaluationResult {
    /// The variant `key` that was selected.
    pub variant_key: String,
    /// The variant `value` payload (typed per the flag's `value_type`).
    pub variant_value: VariantValue,
    /// Outcome classification — disabled / rule-match / default fallthrough.
    pub outcome: EvalOutcome,
    /// `None` when [`TraceLevel::Off`]; populated when [`TraceLevel::Full`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<EvaluationTrace>,
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_selector_context_key_equality() {
        let a = HashSelector::ContextKey {
            context_type: "user".into(),
        };
        let b = HashSelector::ContextKey {
            context_type: "user".into(),
        };
        let c = HashSelector::ContextKey {
            context_type: "device".into(),
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn hash_selector_context_parameter_equality() {
        let a = HashSelector::ContextParameter {
            context_type: "user".into(),
            parameter: "name".into(),
        };
        let b = HashSelector::ContextParameter {
            context_type: "user".into(),
            parameter: "name".into(),
        };
        let c = HashSelector::ContextParameter {
            context_type: "user".into(),
            parameter: "email".into(),
        };
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn hash_selector_key_and_parameter_are_distinct_variants() {
        let key = HashSelector::ContextKey {
            context_type: "user".into(),
        };
        let param = HashSelector::ContextParameter {
            context_type: "user".into(),
            parameter: "key".into(),
        };
        assert_ne!(key, param);
    }

    #[test]
    fn hash_selector_serde_round_trip_context_key() {
        let original = HashSelector::ContextKey {
            context_type: "user".into(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: HashSelector = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
        assert!(json.contains("\"context_key\""));
    }

    #[test]
    fn hash_selector_serde_round_trip_context_parameter() {
        let original = HashSelector::ContextParameter {
            context_type: "device".into(),
            parameter: "os".into(),
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: HashSelector = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
        assert!(json.contains("\"context_parameter\""));
    }

    #[test]
    fn hash_input_spec_new_preserves_order() {
        let selectors = vec![
            HashSelector::ContextKey {
                context_type: "user".into(),
            },
            HashSelector::ContextParameter {
                context_type: "device".into(),
                parameter: "os".into(),
            },
        ];
        let spec = HashInputSpec::new(selectors.clone());
        assert_eq!(spec.selectors, selectors);
        assert_eq!(spec.len(), 2);
        assert!(!spec.is_empty());
    }

    #[test]
    fn hash_input_spec_empty() {
        let spec = HashInputSpec::new(vec![]);
        assert!(spec.is_empty());
        assert_eq!(spec.len(), 0);
    }

    #[test]
    fn hash_input_spec_serde_round_trip_cross_context() {
        let spec = HashInputSpec::new(vec![
            HashSelector::ContextKey {
                context_type: "user".into(),
            },
            HashSelector::ContextParameter {
                context_type: "user".into(),
                parameter: "name".into(),
            },
            HashSelector::ContextKey {
                context_type: "device".into(),
            },
            HashSelector::ContextParameter {
                context_type: "device".into(),
                parameter: "os".into(),
            },
            HashSelector::ContextKey {
                context_type: "application".into(),
            },
        ]);
        let json = serde_json::to_string(&spec).unwrap();
        let back: HashInputSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(spec, back);
    }

    #[test]
    fn hash_input_spec_order_matters_for_equality() {
        let a = HashInputSpec::new(vec![
            HashSelector::ContextKey {
                context_type: "user".into(),
            },
            HashSelector::ContextKey {
                context_type: "device".into(),
            },
        ]);
        let b = HashInputSpec::new(vec![
            HashSelector::ContextKey {
                context_type: "device".into(),
            },
            HashSelector::ContextKey {
                context_type: "user".into(),
            },
        ]);
        assert_ne!(a, b, "selector order must affect equality");
    }

    #[test]
    fn trace_level_is_copy_and_comparable() {
        let off = TraceLevel::Off;
        let off_copy = off;
        assert_eq!(off, off_copy);
        assert_ne!(off, TraceLevel::Full);
    }

    #[test]
    fn trace_level_serde_round_trip() {
        let json = serde_json::to_string(&TraceLevel::Full).unwrap();
        assert_eq!(json, "\"full\"");
        let back: TraceLevel = serde_json::from_str(&json).unwrap();
        assert_eq!(back, TraceLevel::Full);
    }

    #[test]
    fn list_membership_index_default_is_empty() {
        let idx = ListMembershipIndex::default();
        assert!(idx.is_empty());
        assert_eq!(idx.len(), 0);
        assert!(idx.get("user", "alice").is_none());
    }

    #[test]
    fn list_membership_index_insert_then_get() {
        let mut idx = ListMembershipIndex::new();
        let seg = SegmentId::new();
        let mut set = HashSet::new();
        set.insert(seg);
        idx.insert("user", "alice", set.clone());
        assert!(!idx.is_empty());
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.get("user", "alice"), Some(&set));
        assert!(idx.get("user", "bob").is_none());
        assert!(idx.get("device", "alice").is_none());
    }

    #[test]
    fn list_membership_index_overwrites_on_duplicate_key() {
        let mut idx = ListMembershipIndex::new();
        let seg_a = SegmentId::new();
        let seg_b = SegmentId::new();

        let mut first = HashSet::new();
        first.insert(seg_a);
        idx.insert("user", "alice", first);

        let mut second = HashSet::new();
        second.insert(seg_b);
        idx.insert("user", "alice", second.clone());

        assert_eq!(idx.len(), 1);
        assert_eq!(idx.get("user", "alice"), Some(&second));
    }

    #[test]
    fn eval_outcome_variants_serde_round_trip() {
        for outcome in [
            EvalOutcome::FlagDisabled,
            EvalOutcome::RuleMatch { rule_index: 3 },
            EvalOutcome::DefaultFallthrough,
            EvalOutcome::DefaultRuleDistribution,
        ] {
            let json = serde_json::to_string(&outcome).unwrap();
            let back: EvalOutcome = serde_json::from_str(&json).unwrap();
            assert_eq!(outcome, back);
        }
    }

    #[test]
    fn flag_evaluation_result_skips_none_trace_in_json() {
        let result = FlagEvaluationResult {
            variant_key: "control".into(),
            variant_value: VariantValue::StrValue("c".into()),
            outcome: EvalOutcome::DefaultFallthrough,
            trace: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        // `trace: None` is skipped — confirms the wire format stays lean
        // on the SDK hot path.
        assert!(!json.contains("\"trace\""), "json was: {json}");
    }
}
