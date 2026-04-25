//! Event definition and ingestion payload types for the experimentation module.
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

// ---------------------------------------------------------------------------
// Ingestion payload types
// ---------------------------------------------------------------------------

/// A single context dimension attached to an event (e.g. `user:u1`, `session:s99`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EventContext {
    /// Dimension type (e.g. `"user"`, `"session"`).
    #[serde(rename = "_type")]
    pub context_type: String,
    /// Dimension value (e.g. `"u1"`).
    pub key: String,
}

/// The typed metric value carried by an event.
///
/// Exactly one variant must be present; the variant must match the registered
/// `EventValueType` for the event's `metric_key` or the event is rejected (422).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum EventValue {
    /// Boolean metric.
    Bool(bool),
    /// 64-bit integer metric.
    Int(i64),
    /// 64-bit floating-point metric.
    Double(f64),
}

impl EventValue {
    /// Returns the `EventValueType` that corresponds to this value variant.
    #[must_use]
    pub fn value_type(&self) -> EventValueType {
        match self {
            Self::Bool(_) => EventValueType::Bool,
            Self::Int(_) => EventValueType::Int,
            Self::Double(_) => EventValueType::Double,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_type_bool_returns_bool() {
        assert_eq!(EventValue::Bool(true).value_type(), EventValueType::Bool);
    }

    #[test]
    fn value_type_int_returns_int() {
        assert_eq!(EventValue::Int(42).value_type(), EventValueType::Int);
    }

    #[test]
    fn value_type_double_returns_double() {
        assert_eq!(EventValue::Double(1.5).value_type(), EventValueType::Double);
    }
}

/// Payload for a single event ingestion request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct EventPayload {
    /// Context dimensions for this event (e.g. user, session).
    pub contexts: Vec<EventContext>,
    /// Pre-registered metric key for this event.
    pub metric_key: String,
    /// Typed metric value; must match the registered value type.
    pub value: EventValue,
    /// Client-supplied event timestamp.
    pub timestamp: DateTime<Utc>,
}

/// Payload for a batch event ingestion request (max 500 events).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BatchEventPayload {
    /// Events to ingest; maximum 500.
    pub events: Vec<EventPayload>,
}
