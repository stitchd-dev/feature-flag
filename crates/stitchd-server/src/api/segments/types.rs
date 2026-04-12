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
