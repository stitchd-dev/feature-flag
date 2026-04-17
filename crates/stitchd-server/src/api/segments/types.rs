use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use stitchd_core::{
    id::SegmentId,
    rule_engine::condition::Condition,
    rule_engine::types::{ConditionExpr, Rule},
    segment::{ContextList, SegmentDefinition, SegmentType},
};

/// Request to create a new segment.
#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateSegmentRequest {
    /// Ordered rules for rule-based segments.
    pub rules: Option<Vec<Rule>>,
    /// List definitions for list-based segments.
    pub lists: Option<HashMap<String, ContextList>>,
    /// Optimistic-concurrency version counter.
    pub version: i64,
}

/// Full segment details returned by the API.
#[derive(Debug, Clone, Serialize)]
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
#[derive(Debug, Clone, Deserialize)]
pub struct ListCheckRequest {
    /// Context type (e.g. `"user"`, `"org"`).
    pub context_type: String,
    /// Context key to check membership for.
    pub context_key: String,
    /// Segment keys to check membership against.
    pub segment_keys: Vec<String>,
}

/// `POST /v1/environments/{env_id}/segments/list-check` response body.
#[derive(Debug, Clone, Serialize)]
pub struct ListCheckResponse {
    /// Map from segment key to membership boolean.
    pub memberships: HashMap<String, bool>,
}

/// A single context entry for a batch list-check request.
#[derive(Debug, Clone, Deserialize)]
pub struct BatchContext {
    /// Context type (e.g. `"user"`, `"org"`).
    pub context_type: String,
    /// Context key.
    pub context_key: String,
}

/// `POST /v1/environments/{env_id}/segments/list-check/batch` request body.
#[derive(Debug, Clone, Deserialize)]
pub struct BatchListCheckRequest {
    /// Contexts to check membership for.
    pub contexts: Vec<BatchContext>,
    /// Segment keys to check membership against.
    pub segment_keys: Vec<String>,
}

/// Membership result for a single context in a batch response.
#[derive(Debug, Clone, Serialize)]
pub struct BatchContextMembership {
    /// Context type.
    pub context_type: String,
    /// Context key.
    pub context_key: String,
    /// Map from segment key to membership boolean.
    pub memberships: HashMap<String, bool>,
}

/// `POST /v1/environments/{env_id}/segments/list-check/batch` response body.
#[derive(Debug, Clone, Serialize)]
pub struct BatchListCheckResponse {
    /// One entry per requested context.
    pub results: Vec<BatchContextMembership>,
}
