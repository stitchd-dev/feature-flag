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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SegmentDefinition {
    /// Evaluated by matching a rule tree.
    RuleBased(RuleBasedSegment),
    /// Evaluated by checking whether a context key appears in an explicit list.
    ListBased(ListBasedSegment),
}

/// A rule-based segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ContextList {
    /// Keys to include in the segment.
    pub include: HashSet<String>,
    /// Keys to exclude from the segment (takes precedence over include).
    pub exclude: HashSet<String>,
}

/// The result of evaluating a segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MatchResult {
    /// Whether the context matched the segment.
    pub matched: bool,
    /// Trace information for debugging the match result.
    pub trace: MatchTrace,
}

/// Debugging trace for a segment match.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Segment {
    /// Unique identifier.
    pub id: SegmentId,
    /// The environment this segment belongs to.
    pub environment_id: EnvironmentId,
    /// URL-safe string key (unique within the environment).
    pub key: String,
    /// Human-readable display name.
    pub name: String,
    /// Optional description of the segment.
    pub description: String,
    /// Arbitrary tags for filtering/grouping.
    pub tags: Vec<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ParameterValue;
    use crate::id::{RuleId, SegmentId};
    use crate::rule_engine::condition::Condition;
    use crate::rule_engine::types::{ConditionExpr, RuleOutput};

    fn user_ctx(key: &str) -> Context {
        Context::new("user", key)
    }

    fn org_ctx(key: &str) -> Context {
        Context::new("org", key)
    }

    // ── List-based Tests ─────────────────────────────────────────────────────

    #[test]
    fn list_based_exclude_wins_over_include() {
        let mut lists = HashMap::new();
        lists.insert(
            "user".to_string(),
            ContextList {
                include: ["u1".to_string()].into_iter().collect(),
                exclude: ["u1".to_string()].into_iter().collect(),
            },
        );

        let segment = ListBasedSegment {
            id: SegmentId::new(),
            lists,
        };

        let result = segment.evaluate(&[user_ctx("u1")]);
        assert!(!result.matched);
        assert!(matches!(
            result.trace,
            MatchTrace::ListBased {
                reason: ListMatchReason::Excluded,
                ..
            }
        ));
    }

    #[test]
    fn list_based_key_in_neither_list_is_no_match() {
        let mut lists = HashMap::new();
        lists.insert(
            "user".to_string(),
            ContextList {
                include: ["u1".to_string()].into_iter().collect(),
                exclude: ["u2".to_string()].into_iter().collect(),
            },
        );

        let segment = ListBasedSegment {
            id: SegmentId::new(),
            lists,
        };

        let result = segment.evaluate(&[user_ctx("u3")]);
        assert!(!result.matched);
        assert!(matches!(
            result.trace,
            MatchTrace::ListBased {
                reason: ListMatchReason::NoMatch,
                ..
            }
        ));
    }

    #[test]
    fn list_based_no_matching_context_is_no_context() {
        let mut lists = HashMap::new();
        lists.insert(
            "user".to_string(),
            ContextList {
                include: ["u1".to_string()].into_iter().collect(),
                exclude: HashSet::new(),
            },
        );

        let segment = ListBasedSegment {
            id: SegmentId::new(),
            lists,
        };

        let result = segment.evaluate(&[org_ctx("o1")]);
        assert!(!result.matched);
        assert!(matches!(
            result.trace,
            MatchTrace::ListBased {
                reason: ListMatchReason::NoContext,
                ..
            }
        ));
    }

    #[test]
    fn list_based_any_context_match_wins() {
        let mut lists = HashMap::new();
        lists.insert(
            "user".to_string(),
            ContextList {
                include: ["u1".to_string()].into_iter().collect(),
                exclude: HashSet::new(),
            },
        );
        lists.insert(
            "org".to_string(),
            ContextList {
                include: ["o1".to_string()].into_iter().collect(),
                exclude: HashSet::new(),
            },
        );

        let segment = ListBasedSegment {
            id: SegmentId::new(),
            lists,
        };

        // Matches via org
        let result = segment.evaluate(&[user_ctx("u2"), org_ctx("o1")]);
        assert!(result.matched);
        assert!(matches!(
            result.trace,
            MatchTrace::ListBased {
                reason: ListMatchReason::Included,
                context_type: Some(ref t),
            } if t == "org"
        ));
    }

    // ── Rule-based Tests ─────────────────────────────────────────────────────

    fn eq_rule(context_type: &str, param: &str, value: &str) -> Rule {
        Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Leaf(Condition::Eq {
                context_type: context_type.to_string(),
                param: param.to_string(),
                value: ParameterValue::Str(value.to_string()),
            }),
            output: RuleOutput::Variant(crate::id::VariantId::new()),
        }
    }

    #[test]
    fn rule_based_first_matching_rule_wins() {
        let rules = vec![
            eq_rule("user", "plan", "pro"),
            eq_rule("user", "role", "admin"),
        ];
        let segment = RuleBasedSegment {
            id: SegmentId::new(),
            rules,
        };

        let ctx = user_ctx("u1")
            .with_parameter("plan", ParameterValue::Str("pro".into()))
            .with_parameter("role", ParameterValue::Str("admin".into()));

        let result = segment.evaluate(&[ctx]).unwrap();
        assert!(result.matched);
        assert!(matches!(
            result.trace,
            MatchTrace::RuleBased {
                matched_rule_index: Some(0)
            }
        ));
    }

    #[test]
    fn rule_based_no_match_returns_none() {
        let rules = vec![eq_rule("user", "plan", "pro")];
        let segment = RuleBasedSegment {
            id: SegmentId::new(),
            rules,
        };

        let ctx = user_ctx("u1").with_parameter("plan", ParameterValue::Str("free".into()));

        let result = segment.evaluate(&[ctx]).unwrap();
        assert!(!result.matched);
        assert!(matches!(
            result.trace,
            MatchTrace::RuleBased {
                matched_rule_index: None
            }
        ));
    }

    #[test]
    fn rule_based_in_segment_is_invalid() {
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Leaf(Condition::InSegment(SegmentId::new())),
            output: RuleOutput::Variant(crate::id::VariantId::new()),
        };
        let segment = RuleBasedSegment {
            id: SegmentId::new(),
            rules: vec![rule],
        };

        let result = segment.evaluate(&[]);
        assert!(matches!(
            result,
            Err(SegmentEvaluatorError::InvalidSegmentRule)
        ));
    }

    // ── contains_segment_condition with And/Or nesting ───────────────────────

    #[test]
    fn rule_based_and_with_nested_in_segment_is_invalid() {
        // And([Eq(...), InSegment(...)]) — the And arm must detect nested InSegment
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::And(vec![
                ConditionExpr::Leaf(Condition::Eq {
                    context_type: "user".into(),
                    param: "plan".into(),
                    value: ParameterValue::Str("pro".into()),
                }),
                ConditionExpr::Leaf(Condition::InSegment(SegmentId::new())),
            ]),
            output: RuleOutput::Variant(crate::id::VariantId::new()),
        };
        let segment = RuleBasedSegment {
            id: SegmentId::new(),
            rules: vec![rule],
        };
        let result = segment.evaluate(&[]);
        assert!(matches!(
            result,
            Err(SegmentEvaluatorError::InvalidSegmentRule)
        ));
    }

    #[test]
    fn rule_based_or_with_nested_not_in_segment_is_invalid() {
        // Or([Eq(...), NotInSegment(...)]) — the Or arm must detect nested NotInSegment
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Or(vec![
                ConditionExpr::Leaf(Condition::Eq {
                    context_type: "user".into(),
                    param: "plan".into(),
                    value: ParameterValue::Str("pro".into()),
                }),
                ConditionExpr::Leaf(Condition::NotInSegment(SegmentId::new())),
            ]),
            output: RuleOutput::Variant(crate::id::VariantId::new()),
        };
        let segment = RuleBasedSegment {
            id: SegmentId::new(),
            rules: vec![rule],
        };
        let result = segment.evaluate(&[]);
        assert!(matches!(
            result,
            Err(SegmentEvaluatorError::InvalidSegmentRule)
        ));
    }

    #[test]
    fn rule_based_not_wrapping_in_segment_is_invalid() {
        // Not(InSegment(...)) — the Not arm in contains_segment_condition must detect it
        let rule = Rule {
            id: RuleId::new(),
            condition: ConditionExpr::Not(Box::new(ConditionExpr::Leaf(Condition::InSegment(
                SegmentId::new(),
            )))),
            output: RuleOutput::Variant(crate::id::VariantId::new()),
        };
        let segment = RuleBasedSegment {
            id: SegmentId::new(),
            rules: vec![rule],
        };
        let result = segment.evaluate(&[]);
        assert!(matches!(
            result,
            Err(SegmentEvaluatorError::InvalidSegmentRule)
        ));
    }

    // ── SegmentEvaluator::evaluate_one ───────────────────────────────────────

    #[test]
    fn evaluate_one_list_based_matches() {
        let seg_id = SegmentId::new();
        let segment = SegmentDefinition::ListBased(ListBasedSegment {
            id: seg_id,
            lists: HashMap::from([(
                "user".to_string(),
                ContextList {
                    include: ["u1".to_string()].into_iter().collect(),
                    exclude: HashSet::new(),
                },
            )]),
        });
        let ctx = user_ctx("u1");
        let result = SegmentEvaluator::evaluate_one(&[ctx], &segment).unwrap();
        assert!(result.matched);
    }

    #[test]
    fn evaluate_one_rule_based_no_match() {
        let seg_id = SegmentId::new();
        let segment = SegmentDefinition::RuleBased(RuleBasedSegment {
            id: seg_id,
            rules: vec![eq_rule("user", "plan", "pro")],
        });
        let ctx = user_ctx("u1").with_parameter("plan", ParameterValue::Str("free".into()));
        let result = SegmentEvaluator::evaluate_one(&[ctx], &segment).unwrap();
        assert!(!result.matched);
    }

    // ── Global Evaluator Tests ───────────────────────────────────────────────

    #[test]
    fn evaluate_all_evaluates_independently() {
        let s1_id = SegmentId::new();
        let s2_id = SegmentId::new();

        let s1 = SegmentDefinition::ListBased(ListBasedSegment {
            id: s1_id,
            lists: HashMap::from([(
                "user".to_string(),
                ContextList {
                    include: ["u1".to_string()].into_iter().collect(),
                    exclude: HashSet::new(),
                },
            )]),
        });

        let s2 = SegmentDefinition::RuleBased(RuleBasedSegment {
            id: s2_id,
            rules: vec![eq_rule("user", "plan", "pro")],
        });

        let ctx = user_ctx("u1").with_parameter("plan", ParameterValue::Str("free".into()));

        let results = SegmentEvaluator::evaluate_all(&[ctx], &[s1, s2]).unwrap();

        assert!(results[&s1_id].matched); // Matched via list
        assert!(!results[&s2_id].matched); // No match via rule (plan is free)
    }
}
