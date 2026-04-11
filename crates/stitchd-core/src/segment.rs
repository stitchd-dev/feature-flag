//! Segment types used by the segmentation engine.
//!
//! Segments are environment-scoped. Rule content and list members
//! are stored separately and loaded by the segmentation track.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::id::{EnvironmentId, SegmentId};

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

/// A segment definition stored in the database.
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
