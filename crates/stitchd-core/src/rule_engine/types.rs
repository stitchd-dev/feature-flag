use crate::context::Context;
use crate::id::{FlagId, RuleId, SegmentId, VariantId};
use crate::rule_engine::condition::Condition;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

// ── ConditionExpr ─────────────────────────────────────────────────────────────

/// A recursive condition expression supporting arbitrary nesting of AND / OR / NOT.
///
/// # Evaluation rules
/// - `And([])` evaluates to `true`  (vacuously true)
/// - `Or([])` evaluates to `false` (vacuously false)
/// - `And` short-circuits on the first `false` child
/// - `Or` short-circuits on the first `true` child
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "openapi", schema(no_recursion))]
pub enum ConditionExpr {
    /// A single leaf condition test.
    Leaf(Condition),
    /// All children must evaluate to `true`.
    And(Vec<ConditionExpr>),
    /// At least one child must evaluate to `true`.
    Or(Vec<ConditionExpr>),
    /// Inverts the inner expression.
    Not(Box<ConditionExpr>),
}

// ── PercentageTarget / TargetField ────────────────────────────────────────────

/// Identifies which field on which context to hash for percentage allocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum TargetField {
    /// Use `Context::key`.
    Key,
    /// Use `Context::parameters[name]` (stringified via `Display`).
    Parameter(String),
}

/// One contributor to the percentage hash input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PercentageTarget {
    /// Which context type to look up.
    pub context_type: String,
    /// Which field within that context to use.
    pub field: TargetField,
}

// ── ExclusionGate ─────────────────────────────────────────────────────────────

/// A mutually-exclusive-group gate resident on a rule's percentage allocation.
///
/// When present, a context is admitted to the rule's percentage distribution
/// only if its exclusion-group bucket (computed from `group_salt`) falls in the
/// allocated `[bucket_lo, bucket_hi)` basis-point range. This is the
/// spec-mandated location for the gate: it lives on the rule output, not on the
/// experiment, so a context can be excluded before any percentage bucketing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ExclusionGate {
    /// Salt used to derive the context's exclusion-group bucket (shared by all
    /// members of the same exclusion group).
    pub group_salt: String,
    /// Inclusive lower bound of the allocated bucket range, in basis points.
    pub bucket_lo: u16,
    /// Exclusive upper bound of the allocated bucket range, in basis points.
    pub bucket_hi: u16,
}

// ── RuleOutput ────────────────────────────────────────────────────────────────

/// The outcome produced when a rule matches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum RuleOutput {
    /// Assign the evaluation directly to this variant.
    Variant(VariantId),
    /// Hash one or more context fields to bucket into a weighted variant set.
    Percentage {
        /// At least one target is required; values are hashed in declaration order.
        targets: Vec<PercentageTarget>,
        /// `(variant_id, weight)` pairs; weights are basis points and must sum to 10000.
        weights: Vec<(VariantId, u32)>,
        /// Optional mutually-exclusive-group gate. When `Some`, contexts whose
        /// exclusion-group bucket falls outside the allocated range are excluded
        /// from this distribution. Absent in legacy serialized rules → `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exclusion_gate: Option<ExclusionGate>,
    },
}

// ── Rule ──────────────────────────────────────────────────────────────────────

/// A single rule: a condition expression paired with an output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Rule {
    pub id: RuleId,
    /// Optional human-readable label. Ignored by the evaluator; UI metadata only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub condition: ConditionExpr,
    pub output: RuleOutput,
}

impl ConditionExpr {
    /// Collect all `SegmentId`s referenced by `InSegment` or `NotInSegment` leaves.
    pub fn collect_segment_ids(&self, out: &mut HashSet<SegmentId>) {
        match self {
            Self::Leaf(Condition::InSegment(id)) | Self::Leaf(Condition::NotInSegment(id)) => {
                out.insert(*id);
            }
            Self::Leaf(_) => {}
            Self::And(children) | Self::Or(children) => {
                for child in children {
                    child.collect_segment_ids(out);
                }
            }
            Self::Not(inner) => inner.collect_segment_ids(out),
        }
    }
}

// ── EvaluationInput ───────────────────────────────────────────────────────────

/// All inputs required for a single rule-engine evaluation pass.
///
/// `contexts` is borrowed to avoid cloning at evaluation time.
pub struct EvaluationInput<'a> {
    /// Evaluation contexts, one per `context_type`; no two share the same type.
    pub contexts: &'a [Context],
    /// Segments that the evaluation subject has been pre-resolved into.
    pub resolved_segments: HashSet<SegmentId>,
    /// Results of previously evaluated flags (for cross-flag conditions).
    pub evaluated_flags: HashMap<FlagId, VariantId>,
}

impl<'a> EvaluationInput<'a> {
    /// Construct a new evaluation input.
    pub fn new(contexts: &'a [Context]) -> Self {
        Self {
            contexts,
            resolved_segments: HashSet::new(),
            evaluated_flags: HashMap::new(),
        }
    }

    /// Find the context with the given `context_type`, if any.
    pub fn find_context(&self, context_type: &str) -> Option<&Context> {
        self.contexts
            .iter()
            .find(|c| c.context_type == context_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ParameterValue;
    use crate::id::SegmentId;
    use crate::rule_engine::condition::Condition;

    fn user_ctx() -> Context {
        Context::new("user", "u-1").with_parameter("plan", ParameterValue::Str("pro".to_string()))
    }

    #[test]
    fn evaluation_input_find_context() {
        let ctx = user_ctx();
        let input = EvaluationInput::new(std::slice::from_ref(&ctx));
        assert!(input.find_context("user").is_some());
        assert!(input.find_context("org").is_none());
    }

    #[test]
    fn condition_expr_leaf_roundtrip() {
        let expr = ConditionExpr::Leaf(Condition::InSegment(SegmentId::new()));
        let json = serde_json::to_string(&expr).unwrap();
        let back: ConditionExpr = serde_json::from_str(&json).unwrap();
        assert_eq!(expr, back);
    }

    #[test]
    fn condition_expr_not_box() {
        let inner = ConditionExpr::And(vec![]);
        let not = ConditionExpr::Not(Box::new(inner));
        assert!(matches!(not, ConditionExpr::Not(_)));
    }

    #[test]
    fn rule_output_variant_roundtrip() {
        let output = RuleOutput::Variant(VariantId::new());
        let json = serde_json::to_string(&output).unwrap();
        let back: RuleOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, back);
    }

    #[test]
    fn percentage_target_field_key() {
        let t = PercentageTarget {
            context_type: "user".to_string(),
            field: TargetField::Key,
        };
        assert_eq!(t.field, TargetField::Key);
    }

    #[test]
    fn percentage_target_field_parameter() {
        let t = PercentageTarget {
            context_type: "org".to_string(),
            field: TargetField::Parameter("account_tier".to_string()),
        };
        assert!(matches!(t.field, TargetField::Parameter(_)));
    }

    #[test]
    fn exclusion_gate_serde_round_trips() {
        let gate = ExclusionGate {
            group_salt: "grp-salt".to_string(),
            bucket_lo: 0,
            bucket_hi: 2500,
        };
        let json = serde_json::to_string(&gate).unwrap();
        let back: ExclusionGate = serde_json::from_str(&json).unwrap();
        assert_eq!(gate, back);
    }

    #[test]
    fn percentage_deserializes_without_exclusion_gate() {
        // Legacy serialized JSONB lacking `exclusion_gate` → None.
        let json = r#"{"Percentage":{"targets":[],"weights":[]}}"#;
        let output: RuleOutput = serde_json::from_str(json).unwrap();
        match output {
            RuleOutput::Percentage {
                exclusion_gate, ..
            } => assert!(exclusion_gate.is_none()),
            other => panic!("expected Percentage, got {other:?}"),
        }
    }

    #[test]
    fn percentage_skips_serializing_none_exclusion_gate() {
        let output = RuleOutput::Percentage {
            targets: vec![],
            weights: vec![],
            exclusion_gate: None,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(
            !json.contains("exclusion_gate"),
            "None gate should be skipped: {json}"
        );
    }

    #[test]
    fn percentage_round_trips_with_exclusion_gate() {
        let output = RuleOutput::Percentage {
            targets: vec![],
            weights: vec![(VariantId::new(), 10000)],
            exclusion_gate: Some(ExclusionGate {
                group_salt: "s".to_string(),
                bucket_lo: 2500,
                bucket_hi: 5000,
            }),
        };
        let json = serde_json::to_string(&output).unwrap();
        let back: RuleOutput = serde_json::from_str(&json).unwrap();
        assert_eq!(output, back);
    }
}
