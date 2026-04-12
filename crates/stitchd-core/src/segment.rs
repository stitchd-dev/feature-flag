//! Segment types used by the segmentation engine.
//!
//! Segments are environment-scoped. Rule content and list members
//! are stored separately and loaded by the segmentation track.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::context::Context;
use crate::id::{EnvironmentId, SegmentId};
use crate::rule_engine::condition::Condition;
use crate::rule_engine::error::RuleEngineError;
use crate::rule_engine::eval_expr::evaluate_expr;
use crate::rule_engine::types::{ConditionExpr, EvaluationInput, Rule};

/// Whether a segment is rule-based or key-list-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum SegmentType {
    /// Evaluated by matching a rule tree against a `Context`.
    Rule,
    /// Evaluated by checking whether a context key appears in an explicit list.
    List,
}

impl SegmentDefinition {
    /// Evaluate the segment against the provided contexts.
    pub fn evaluate(&self, contexts: &[Context]) -> Result<MatchResult, SegmentEvaluatorError> {
        match self {
            Self::RuleBased(s) => s.evaluate(contexts),
            Self::ListBased(s) => Ok(s.evaluate(contexts)),
        }
    }
}

/// A segment definition used for evaluation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SegmentDefinition {
    /// Evaluated by matching a rule tree.
    RuleBased(RuleBasedSegment),
    /// Evaluated by checking whether a context key appears in an explicit list.
    ListBased(ListBasedSegment),
}

/// A rule-based segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleBasedSegment {
    /// Unique identifier.
    pub id: SegmentId,
    /// Rules to evaluate, in order.
    pub rules: Vec<Rule>,
}

impl RuleBasedSegment {
    /// Evaluate the segment rules.
    pub fn evaluate(&self, contexts: &[Context]) -> Result<MatchResult, SegmentEvaluatorError> {
        let input = EvaluationInput::new(contexts);

        for (i, rule) in self.rules.iter().enumerate() {
            if contains_segment_condition(&rule.condition) {
                return Err(SegmentEvaluatorError::InvalidSegmentRule);
            }

            if evaluate_expr(&rule.condition, &input)? {
                return Ok(MatchResult {
                    matched: true,
                    trace: MatchTrace::RuleBased {
                        matched_rule_index: Some(i),
                    },
                });
            }
        }

        Ok(MatchResult {
            matched: false,
            trace: MatchTrace::RuleBased {
                matched_rule_index: None,
            },
        })
    }
}

/// Helper to check if a condition expression contains segment-based conditions.
fn contains_segment_condition(expr: &ConditionExpr) -> bool {
    match expr {
        ConditionExpr::Leaf(Condition::InSegment(_))
        | ConditionExpr::Leaf(Condition::NotInSegment(_)) => true,
        ConditionExpr::Leaf(_) => false,
        ConditionExpr::And(exprs) | ConditionExpr::Or(exprs) => {
            exprs.iter().any(contains_segment_condition)
        }
        ConditionExpr::Not(expr) => contains_segment_condition(expr),
    }
}

/// A list-based segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListBasedSegment {
    /// Unique identifier.
    pub id: SegmentId,
    /// Per-context-type include/exclude lists.
    pub lists: HashMap<String, ContextList>,
}

impl ListBasedSegment {
    /// Evaluate the segment against the provided contexts.
    pub fn evaluate(&self, contexts: &[Context]) -> MatchResult {
        let mut included_type = None;
        let mut found_any_context = false;

        for (context_type, list) in &self.lists {
            if let Some(ctx) = contexts.iter().find(|c| &c.context_type == context_type) {
                found_any_context = true;

                // Exclude takes precedence
                if list.exclude.contains(&ctx.key) {
                    return MatchResult {
                        matched: false,
                        trace: MatchTrace::ListBased {
                            context_type: Some(context_type.clone()),
                            reason: ListMatchReason::Excluded,
                        },
                    };
                }

                // If not excluded, check for inclusion
                if included_type.is_none() && list.include.contains(&ctx.key) {
                    included_type = Some(context_type.clone());
                }
            }
        }

        if let Some(ctx_type) = included_type {
            MatchResult {
                matched: true,
                trace: MatchTrace::ListBased {
                    context_type: Some(ctx_type),
                    reason: ListMatchReason::Included,
                },
            }
        } else if found_any_context {
            MatchResult {
                matched: false,
                trace: MatchTrace::ListBased {
                    context_type: None,
                    reason: ListMatchReason::NoMatch,
                },
            }
        } else {
            MatchResult {
                matched: false,
                trace: MatchTrace::ListBased {
                    context_type: None,
                    reason: ListMatchReason::NoContext,
                },
            }
        }
    }
}

/// Include and exclude lists for a specific context type.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContextList {
    /// Keys to include in the segment.
    pub include: HashSet<String>,
    /// Keys to exclude from the segment (takes precedence over include).
    pub exclude: HashSet<String>,
}

/// The result of evaluating a segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchResult {
    /// Whether the context matched the segment.
    pub matched: bool,
    /// Trace information for debugging the match result.
    pub trace: MatchTrace,
}

/// Debugging trace for a segment match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MatchTrace {
    /// Trace for a rule-based segment.
    RuleBased {
        /// The index of the rule that matched, if any.
        matched_rule_index: Option<usize>,
    },
    /// Trace for a list-based segment.
    ListBased {
        /// The context type that triggered the match, if any.
        context_type: Option<String>,
        /// The reason for the match result.
        reason: ListMatchReason,
    },
}

/// Reason for a list-based segment match result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ListMatchReason {
    /// The key was found in the include list.
    Included,
    /// The key was found in the exclude list.
    Excluded,
    /// The key was not found in either list.
    NoMatch,
    /// No matching context type was found in the input.
    NoContext,
}

/// Errors that can occur during segment evaluation.
#[derive(Debug, thiserror::Error)]
pub enum SegmentEvaluatorError {
    /// An error occurred in the rule engine.
    #[error("Rule engine error: {0}")]
    RuleEngine(#[from] RuleEngineError),
    /// A rule contains an invalid condition for a segment.
    #[error("Invalid segment rule: segments cannot depend on other segments")]
    InvalidSegmentRule,
}

/// A segment record stored in the database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    /// Unique identifier.
    pub id: SegmentId,
    /// The environment this segment belongs to.
    pub environment_id: EnvironmentId,
    /// URL-safe string key (unique within the environment).
    pub key: String,
    /// Whether this is a rule-based or list-based segment.
    pub segment_type: SegmentType,
    /// When this record was created.
    pub created_at: DateTime<Utc>,
    /// When this record was last modified.
    pub updated_at: DateTime<Utc>,
    /// Set when the segment is soft-deleted; `None` while active.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Optimistic-concurrency version counter.
    pub version: i64,
}

/// Global evaluator for segments.
pub struct SegmentEvaluator;

impl SegmentEvaluator {
    /// Evaluate a single segment.
    pub fn evaluate_one(
        contexts: &[Context],
        segment: &SegmentDefinition,
    ) -> Result<MatchResult, SegmentEvaluatorError> {
        segment.evaluate(contexts)
    }

    /// Evaluate all segments independently.
    pub fn evaluate_all(
        contexts: &[Context],
        segments: &[SegmentDefinition],
    ) -> Result<HashMap<SegmentId, MatchResult>, SegmentEvaluatorError> {
        let mut results = HashMap::with_capacity(segments.len());
        for segment in segments {
            let id = match segment {
                SegmentDefinition::RuleBased(s) => s.id,
                SegmentDefinition::ListBased(s) => s.id,
            };
            results.insert(id, segment.evaluate(contexts)?);
        }
        Ok(results)
    }
}
