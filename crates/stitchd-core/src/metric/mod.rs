//! Metric definitions — composable primitives for experimentation.
//!
//! A [`MetricDefinition`] is an environment-scoped, kind-discriminated
//! resource that experiments reference (via `metric_ids[]`) rather than
//! raw event keys. The kind is one of:
//!
//! - [`MetricKind::Aggregation`] — count/sum/avg/percentile/uniq over a
//!   single event stream
//! - [`MetricKind::Ratio`] — derived metric: `numerator / denominator`
//!   over two aggregation metrics
//! - [`MetricKind::Funnel`] — ordered sequence of event steps with a
//!   conversion window (ClickHouse `windowFunnel`)
//!
//! See the [`kinds`] module for per-kind config types and validators.

pub mod kinds;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::id::{EnvironmentId, MetricId};

pub use kinds::{
    AggregationConfig, AggregationOperator, FunnelConfig, FunnelStep, MetricKind, RatioConfig,
};

// ── GoalDirection ─────────────────────────────────────────────────────────────

/// Direction in which the metric value is intended to move.
///
/// Used by the experiment UI to colour up/down arrows and by the stats
/// service to determine whether a positive lift is a "win" or a
/// "regression".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum GoalDirection {
    /// Higher is better — e.g. conversion rate, revenue.
    Increase,
    /// Lower is better — e.g. latency, error rate.
    Decrease,
    /// No inherent direction — used for tracking-only metrics.
    Neutral,
}

// ── MetricDefinition ──────────────────────────────────────────────────────────

/// A composable metric definition.
///
/// Stored in the `metric_definitions` PG table (one row per definition),
/// uniquely keyed by `(environment_id, key)` for non-soft-deleted rows.
/// The `kind` field carries the kind-specific config inline (serde
/// `tag = "kind"`), matching the wire format used by the public REST
/// API and the protobuf oneof representation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct MetricDefinition {
    /// Globally unique identifier.
    pub id: MetricId,
    /// Owning environment.
    pub environment_id: EnvironmentId,
    /// URL-safe slug, unique per environment among non-deleted metrics.
    pub key: String,
    /// Human-friendly display name.
    pub name: String,
    /// Optional long-form description shown in the admin UI.
    pub description: Option<String>,
    /// The metric primitive (aggregation / ratio / funnel) and its
    /// kind-specific config.
    #[serde(flatten)]
    pub kind: MetricKind,
    /// Direction in which the metric value is intended to move.
    pub goal_direction: GoalDirection,
    /// Optimistic-locking version — incremented by the repo on every
    /// successful update; updates with stale `version` are rejected.
    /// Stored as `BIGINT` in PG; matches the convention used by
    /// `EventDefinition`, `Segment`, and other versioned entities.
    pub version: i64,
    /// Creation timestamp (UTC).
    pub created_at: DateTime<Utc>,
    /// Last-modification timestamp (UTC).
    pub updated_at: DateTime<Utc>,
    /// Soft-delete timestamp; `None` means the metric is live.
    pub deleted_at: Option<DateTime<Utc>>,
}

impl MetricDefinition {
    /// Validate the metric's shape invariants — calls the kind-specific
    /// `validate()` plus generic field-level checks (key length, etc).
    pub fn validate(&self) -> Result<(), MetricValidationError> {
        if self.key.trim().is_empty() {
            return Err(MetricValidationError::KeyEmpty);
        }
        if self.key.len() > 120 {
            return Err(MetricValidationError::KeyTooLong);
        }
        if self.name.trim().is_empty() {
            return Err(MetricValidationError::NameEmpty);
        }
        self.kind.validate()?;
        Ok(())
    }

    /// Returns `true` if the metric has been soft-deleted.
    #[must_use]
    pub const fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

// ── MetricValidationError ─────────────────────────────────────────────────────

/// Typed errors returned by `MetricDefinition::validate` and the
/// per-kind `validate()` methods.
#[derive(Debug, Error, PartialEq)]
pub enum MetricValidationError {
    /// `key` was empty or whitespace-only.
    #[error("metric key cannot be empty")]
    KeyEmpty,
    /// `key` exceeded the 120-char limit (the PG column max).
    #[error("metric key is too long (max 120 chars)")]
    KeyTooLong,
    /// `name` was empty or whitespace-only.
    #[error("metric name cannot be empty")]
    NameEmpty,
    /// Aggregation `on_field` is required for the given aggregator.
    #[error("aggregation on_field is required for aggregator `{0:?}`")]
    AggregationFieldRequired(AggregationOperator),
    /// Funnel must have at least 2 steps.
    #[error("funnel must have at least 2 steps (got {0})")]
    FunnelTooFewSteps(usize),
    /// Funnel `window_seconds` must be strictly positive.
    #[error("funnel window_seconds must be strictly positive (got {0})")]
    FunnelNonPositiveWindow(i64),
    /// Ratio numerator and denominator must reference distinct metrics.
    #[error("ratio numerator and denominator must be distinct")]
    RatioSameMetric,
    /// Ratio `min_denominator` must be non-negative.
    #[error("ratio min_denominator must be non-negative (got {0})")]
    RatioNegativeMinDenominator(i64),
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_valid_metric() -> MetricDefinition {
        MetricDefinition {
            id: MetricId::new(),
            environment_id: EnvironmentId::new(),
            key: "checkout_completion".into(),
            name: "Checkout Completion".into(),
            description: Some("Count of completed checkouts.".into()),
            kind: MetricKind::Aggregation(AggregationConfig {
                event_key: "checkout_completed".into(),
                aggregator: AggregationOperator::Count,
                on_field: None,
                where_clause: None,
            }),
            goal_direction: GoalDirection::Increase,
            version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
        }
    }

    #[test]
    fn valid_metric_passes_validation() {
        let m = make_valid_metric();
        assert!(m.validate().is_ok());
    }

    #[test]
    fn metric_with_empty_key_fails() {
        let mut m = make_valid_metric();
        m.key = "   ".into();
        assert_eq!(m.validate().unwrap_err(), MetricValidationError::KeyEmpty);
    }

    #[test]
    fn metric_with_too_long_key_fails() {
        let mut m = make_valid_metric();
        m.key = "a".repeat(121);
        assert_eq!(m.validate().unwrap_err(), MetricValidationError::KeyTooLong);
    }

    #[test]
    fn metric_with_empty_name_fails() {
        let mut m = make_valid_metric();
        m.name = "  ".into();
        assert_eq!(m.validate().unwrap_err(), MetricValidationError::NameEmpty);
    }

    #[test]
    fn metric_validate_delegates_to_kind() {
        // Funnel with only 1 step — kind validator should fire.
        let mut m = make_valid_metric();
        m.kind = MetricKind::Funnel(FunnelConfig {
            steps: vec![FunnelStep {
                event_key: "only".into(),
                where_clause: None,
            }],
            window_seconds: 60,
            count_repeats: false,
        });
        assert_eq!(
            m.validate().unwrap_err(),
            MetricValidationError::FunnelTooFewSteps(1),
        );
    }

    #[test]
    fn is_deleted_reflects_timestamp() {
        let mut m = make_valid_metric();
        assert!(!m.is_deleted());
        m.deleted_at = Some(Utc::now());
        assert!(m.is_deleted());
    }

    #[test]
    fn goal_direction_serde() {
        for (v, s) in [
            (GoalDirection::Increase, "increase"),
            (GoalDirection::Decrease, "decrease"),
            (GoalDirection::Neutral, "neutral"),
        ] {
            assert_eq!(serde_json::to_value(v).unwrap(), serde_json::json!(s));
            let back: GoalDirection = serde_json::from_value(serde_json::json!(s)).unwrap();
            assert_eq!(back, v);
        }
    }

    #[test]
    fn metric_definition_serde_includes_kind_tag() {
        let m = make_valid_metric();
        let j = serde_json::to_value(&m).unwrap();
        // Kind is flattened in — top-level should carry `kind` tag.
        assert_eq!(j["kind"], "aggregation");
        // Round-trip preserves all fields.
        let back: MetricDefinition = serde_json::from_value(j).unwrap();
        assert_eq!(back, m);
    }
}
