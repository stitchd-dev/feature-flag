//! Segment types used by the segmentation engine.
//!
//! Segments are environment-scoped. Rule content and list members
//! are stored separately and loaded by the segmentation track.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::id::{EnvironmentId, SegmentId};
use crate::rule_engine::error::RuleEngineError;
use crate::rule_engine::types::Rule;

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

/// A list-based segment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListBasedSegment {
    /// Unique identifier.
    pub id: SegmentId,
    /// Per-context-type include/exclude lists.
    pub lists: HashMap<String, ContextList>,
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
