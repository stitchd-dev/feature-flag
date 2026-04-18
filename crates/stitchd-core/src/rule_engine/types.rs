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
        /// `(variant_id, weight)` pairs; weights are tenths-of-a-percent and must sum to 1000.
        weights: Vec<(VariantId, u32)>,
    },
}

// ── Rule ──────────────────────────────────────────────────────────────────────

/// A single rule: a condition expression paired with an output.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Rule {
    pub id: RuleId,
    pub condition: ConditionExpr,
    pub output: RuleOutput,
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
}
