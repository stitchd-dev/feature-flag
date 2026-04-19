//! Event definition types for the experimentation module.
//!
//! Event definitions are pre-registered per environment. Only known keys are
//! accepted at the ingestion boundary; unknown keys → 422.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::id::{EnvironmentId, EventDefinitionId};

/// The metric value type an event definition accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum EventValueType {
    /// Boolean metric (e.g. conversion flag).
    Bool,
    /// 64-bit integer metric (e.g. click count).
    Int,
    /// 64-bit floating-point metric (e.g. revenue).
    Double,
}

/// A pre-registered event definition scoped to an environment.
///
/// Ingestion rejects any event whose `metric_key` is not registered here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EventDefinition {
    /// Unique identifier.
    pub id: EventDefinitionId,
    /// The environment this definition belongs to.
    pub environment_id: EnvironmentId,
    /// URL-safe string key, unique within the environment.
    pub key: String,
    /// The type of metric value this event carries.
    pub value_type: EventValueType,
    /// When this record was created.
    pub created_at: DateTime<Utc>,
    /// When this record was last modified.
    pub updated_at: DateTime<Utc>,
    /// Set when the definition is soft-deleted; `None` while active.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Optimistic-concurrency version counter.
    pub version: i64,
}
