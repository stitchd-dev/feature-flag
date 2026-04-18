use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use stitchd_core::{
    id::SegmentId,
    rule_engine::condition::Condition,
    rule_engine::types::{ConditionExpr, Rule},
    segment::{ContextList, SegmentDefinition, SegmentType},
};

/// Request to create a new segment.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct CreateSegmentRequest {
    /// URL-safe string key (unique within the environment).
    pub key: String,
    /// Whether this is a rule-based or list-based segment.
    pub segment_type: SegmentType,
    /// Ordered rules for rule-based segments (optional, required if type is rule).
    pub rules: Option<Vec<Rule>>,
    /// List definitions for list-based segments (optional, required if type is list).
    pub lists: Option<HashMap<String, ContextList>>,
}

/// Request to update a segment definition.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct UpdateSegmentRequest {
    /// Ordered rules for rule-based segments.
    pub rules: Option<Vec<Rule>>,
    /// List definitions for list-based segments.
    pub lists: Option<HashMap<String, ContextList>>,
    /// Optimistic-concurrency version counter.
    pub version: i64,
}

/// Full segment details returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SegmentResponse {
    /// Unique identifier.
    pub id: SegmentId,
    /// URL-safe string key.
    pub key: String,
    /// Whether this is a rule-based or list-based segment.
    pub segment_type: SegmentType,
    /// The segment's evaluation definition.
    pub definition: SegmentDefinition,
    /// Optimistic-concurrency version counter.
    pub version: i64,
}

/// Validation error for segment rules.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// A segment rule contains an invalid condition (e.g. `InSegment`).
    #[error("Segments cannot depend on other segments (InSegment/NotInSegment found in rule)")]
    InvalidSegmentRule,
    /// Required fields for the segment type are missing.
    #[error("Missing definition for segment type {0:?}")]
    MissingDefinition(SegmentType),
}

/// Validate that a set of rules does not contain segment-based conditions.
///
/// # Errors
/// Returns [`ValidationError::InvalidSegmentRule`] if any rule contains a segment-based condition.
pub fn validate_rules(rules: &[Rule]) -> Result<(), ValidationError> {
    for rule in rules {
        if contains_segment_condition(&rule.condition) {
            return Err(ValidationError::InvalidSegmentRule);
        }
    }
    Ok(())
}

fn contains_segment_condition(expr: &ConditionExpr) -> bool {
    match expr {
        ConditionExpr::Leaf(Condition::InSegment(_) | Condition::NotInSegment(_)) => true,
        ConditionExpr::Leaf(_) => false,
        ConditionExpr::And(exprs) | ConditionExpr::Or(exprs) => {
            exprs.iter().any(contains_segment_condition)
        }
        ConditionExpr::Not(expr) => contains_segment_condition(expr),
    }
}

// ---------------------------------------------------------------------------
// List-check request/response types
// ---------------------------------------------------------------------------

/// `POST /v1/environments/{env_id}/segments/list-check` request body.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct ListCheckRequest {
    /// Context type (e.g. `"user"`, `"org"`).
    pub context_type: String,
    /// Context key to check membership for.
    pub context_key: String,
    /// Segment keys to check membership against.
    pub segment_keys: Vec<String>,
}

/// `POST /v1/environments/{env_id}/segments/list-check` response body.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ListCheckResponse {
    /// Map from segment key to membership boolean.
    pub memberships: HashMap<String, bool>,
}

/// A single context entry for a batch list-check request.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct BatchContext {
    /// Context type (e.g. `"user"`, `"org"`).
    pub context_type: String,
    /// Context key.
    pub context_key: String,
}

/// `POST /v1/environments/{env_id}/segments/list-check/batch` request body.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct BatchListCheckRequest {
    /// Contexts to check membership for.
    pub contexts: Vec<BatchContext>,
    /// Segment keys to check membership against.
    pub segment_keys: Vec<String>,
}

/// Membership result for a single context in a batch response.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BatchContextMembership {
    /// Context type.
    pub context_type: String,
    /// Context key.
    pub context_key: String,
    /// Map from segment key to membership boolean.
    pub memberships: HashMap<String, bool>,
}

/// `POST /v1/environments/{env_id}/segments/list-check/batch` response body.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BatchListCheckResponse {
    /// One entry per requested context.
    pub results: Vec<BatchContextMembership>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use stitchd_core::{
        id::{RuleId, VariantId},
        rule_engine::{
            condition::Condition,
            types::{ConditionExpr, Rule, RuleOutput},
        },
    };

    fn make_leaf_rule(condition: Condition) -> Rule {
        Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Leaf(condition),
            output: RuleOutput::Variant(VariantId::new()),
        }
    }

    // ---------------------------------------------------------------------------
    // validate_rules
    // ---------------------------------------------------------------------------

    #[test]
    fn validate_rules_accepts_empty_rules() {
        assert!(validate_rules(&[]).is_ok());
    }

    #[test]
    fn validate_rules_accepts_non_segment_conditions() {
        use stitchd_core::context::ParameterValue;
        let rule = make_leaf_rule(Condition::Eq {
            context_type: "user".to_string(),
            param: "email".to_string(),
            value: ParameterValue::Str("test@example.com".to_string()),
        });
        assert!(validate_rules(&[rule]).is_ok());
    }

    #[test]
    fn validate_rules_rejects_in_segment_condition() {
        use stitchd_core::id::SegmentId;
        let rule = make_leaf_rule(Condition::InSegment(SegmentId::new()));
        let result = validate_rules(&[rule]);
        assert!(result.is_err());
        assert!(matches!(result, Err(ValidationError::InvalidSegmentRule)));
    }

    #[test]
    fn validate_rules_rejects_not_in_segment_condition() {
        use stitchd_core::id::SegmentId;
        let rule = make_leaf_rule(Condition::NotInSegment(SegmentId::new()));
        let result = validate_rules(&[rule]);
        assert!(result.is_err());
        assert!(matches!(result, Err(ValidationError::InvalidSegmentRule)));
    }

    #[test]
    fn validate_rules_rejects_in_segment_nested_in_and() {
        use stitchd_core::{context::ParameterValue, id::SegmentId};
        let seg_rule = make_leaf_rule(Condition::InSegment(SegmentId::new()));
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::And(vec![
                ConditionExpr::Leaf(Condition::Eq {
                    context_type: "user".to_string(),
                    param: "plan".to_string(),
                    value: ParameterValue::Str("pro".to_string()),
                }),
                seg_rule.condition,
            ]),
            output: RuleOutput::Variant(VariantId::new()),
        };
        let result = validate_rules(&[rule]);
        assert!(result.is_err());
    }

    #[test]
    fn validate_rules_rejects_in_segment_nested_in_or() {
        use stitchd_core::{context::ParameterValue, id::SegmentId};
        let seg_rule = make_leaf_rule(Condition::InSegment(SegmentId::new()));
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Or(vec![
                ConditionExpr::Leaf(Condition::Eq {
                    context_type: "user".to_string(),
                    param: "plan".to_string(),
                    value: ParameterValue::Str("pro".to_string()),
                }),
                seg_rule.condition,
            ]),
            output: RuleOutput::Variant(VariantId::new()),
        };
        let result = validate_rules(&[rule]);
        assert!(result.is_err());
    }

    #[test]
    fn validate_rules_rejects_in_segment_nested_in_not() {
        use stitchd_core::id::SegmentId;
        let seg_rule = make_leaf_rule(Condition::InSegment(SegmentId::new()));
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Not(Box::new(seg_rule.condition)),
            output: RuleOutput::Variant(VariantId::new()),
        };
        let result = validate_rules(&[rule]);
        assert!(result.is_err());
    }

    // ---------------------------------------------------------------------------
    // ValidationError messages
    // ---------------------------------------------------------------------------

    #[test]
    fn validation_error_invalid_segment_rule_has_message() {
        let err = ValidationError::InvalidSegmentRule;
        let msg = err.to_string();
        assert!(msg.contains("InSegment"));
    }

    #[test]
    fn validation_error_missing_definition_includes_type() {
        let err = ValidationError::MissingDefinition(SegmentType::Rule);
        let msg = err.to_string();
        assert!(msg.contains("Rule") || msg.to_lowercase().contains("missing"));
    }
}
