//! REST handlers for experiment management.
//!
//! Endpoints:
//! - `POST   /v1/environments/{env_id}/experiments`                      — create
//! - `GET    /v1/environments/{env_id}/experiments`                      — list
//! - `GET    /v1/environments/{env_id}/experiments/{id}`                 — get
//! - `PATCH  /v1/environments/{env_id}/experiments/{id}`                 — update
//! - `DELETE /v1/environments/{env_id}/experiments/{id}`                 — soft-delete
//! - `POST   /v1/environments/{env_id}/experiments/{id}/transitions`     — lifecycle transition

use axum::{
    Json,
    http::StatusCode,
    response::IntoResponse,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use stitchd_core::{
    experimentation::{Experiment, ExperimentIteration, ExperimentStatus},
    id::{EnvironmentId, ExperimentId, ExperimentIterationId, RuleId},
};
use stitchd_db::RepositoryError;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// API error type mapping internal errors to HTTP responses.
pub enum ApiError {
    /// Resource was not found.
    NotFound(String),
    /// Optimistic concurrency, unique constraint, or invalid state transition conflict.
    Conflict(String),
    /// Internal server or database error.
    Database(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg) = match self {
            Self::NotFound(m) => (StatusCode::NOT_FOUND, m),
            Self::Conflict(m) => (StatusCode::CONFLICT, m),
            Self::Database(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

impl From<RepositoryError> for ApiError {
    fn from(e: RepositoryError) -> Self {
        match e {
            RepositoryError::NotFound { id } => Self::NotFound(format!("not found: {id}")),
            RepositoryError::VersionConflict { expected, actual } => Self::Conflict(format!(
                "version conflict: expected {expected}, actual {actual}"
            )),
            RepositoryError::UniqueViolation { field } => {
                Self::Conflict(format!("unique violation on: {field}"))
            }
            RepositoryError::Database(e) => Self::Database(e.to_string()),
            RepositoryError::Unexpected(e) => Self::Database(e.to_string()),
            RepositoryError::InvalidState { reason } => Self::Conflict(reason),
        }
    }
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

/// Request body for creating a new experiment.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateExperimentRequest {
    /// The flag rule this experiment is bound to.
    pub flag_rule_id: RuleId,
    /// Human-readable name for the experiment.
    pub name: String,
    /// Optional description of what the experiment tests.
    pub description: Option<String>,
    /// Optional hypothesis statement for the experiment.
    pub hypothesis: Option<String>,
    /// Pre-registered event definition keys to use as metrics (at least one required).
    pub metric_keys: Vec<String>,
    /// Percentage of rule-matched contexts to enrol (0.1–100.0); defaults to 100.0.
    pub traffic_allocation: Option<f64>,
    /// Optional informational minimum sample size guardrail.
    pub min_sample_size: Option<i64>,
    /// Optional ISO-8601 scheduled start time (informational only).
    pub scheduled_start_at: Option<DateTime<Utc>>,
    /// Optional ISO-8601 scheduled end time (informational only).
    pub scheduled_end_at: Option<DateTime<Utc>>,
}

impl CreateExperimentRequest {
    /// Validates the request, returning `Err(String)` with a human-readable message on failure.
    ///
    /// Rules:
    /// - `metric_keys` must contain at least one entry.
    /// - `traffic_allocation`, when provided, must be in the range [0.1, 100.0].
    pub fn validate(&self) -> Result<(), String> {
        if self.metric_keys.is_empty() {
            return Err("metric_keys must contain at least one entry".to_string());
        }
        if let Some(alloc) = self.traffic_allocation {
            if !(0.1..=100.0).contains(&alloc) {
                return Err(format!(
                    "traffic_allocation must be between 0.1 and 100.0, got {alloc}"
                ));
            }
        }
        Ok(())
    }
}

/// Request body for updating an existing experiment.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct UpdateExperimentRequest {
    /// Updated human-readable name for the experiment.
    pub name: Option<String>,
    /// Updated description of the experiment.
    pub description: Option<String>,
    /// Updated hypothesis statement.
    pub hypothesis: Option<String>,
    /// Updated list of metric event definition keys (at least one required if provided).
    pub metric_keys: Option<Vec<String>>,
    /// Updated traffic allocation percentage (0.1–100.0).
    pub traffic_allocation: Option<f64>,
    /// Updated minimum sample size guardrail.
    pub min_sample_size: Option<i64>,
    /// Updated scheduled start time.
    pub scheduled_start_at: Option<DateTime<Utc>>,
    /// Updated scheduled end time.
    pub scheduled_end_at: Option<DateTime<Utc>>,
    /// Current version for optimistic locking (required).
    pub version: i64,
}

/// Request body for transitioning an experiment to a new lifecycle status.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct TransitionRequest {
    /// The target lifecycle status to transition to.
    pub to: ExperimentStatus,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Response body for a single experiment.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExperimentResponse {
    /// Unique identifier of the experiment.
    pub id: ExperimentId,
    /// The environment this experiment belongs to.
    pub env_id: EnvironmentId,
    /// The flag rule this experiment is bound to.
    pub flag_rule_id: RuleId,
    /// Human-readable name.
    pub name: String,
    /// Optional description of the experiment.
    pub description: Option<String>,
    /// Optional hypothesis statement.
    pub hypothesis: Option<String>,
    /// Current lifecycle status serialized as a snake_case string.
    pub status: String,
    /// Pre-registered event definition keys used as metrics.
    pub metric_keys: Vec<String>,
    /// Percentage of rule-matched contexts enrolled (0.1–100.0).
    pub traffic_allocation: f64,
    /// Optional informational minimum sample size guardrail.
    pub min_sample_size: Option<i64>,
    /// Optional ISO-8601 scheduled start time.
    pub scheduled_start_at: Option<DateTime<Utc>>,
    /// Optional ISO-8601 scheduled end time.
    pub scheduled_end_at: Option<DateTime<Utc>>,
    /// Optimistic-concurrency version counter.
    pub version: i64,
    /// ISO-8601 creation timestamp.
    pub created_at: DateTime<Utc>,
    /// ISO-8601 last-updated timestamp.
    pub updated_at: DateTime<Utc>,
}

impl From<Experiment> for ExperimentResponse {
    fn from(e: Experiment) -> Self {
        Self {
            id: e.id,
            env_id: e.environment_id,
            flag_rule_id: e.flag_rule_id,
            name: e.name,
            description: e.description,
            hypothesis: e.hypothesis,
            status: serde_json::to_value(&e.status)
                .ok()
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| format!("{:?}", e.status).to_lowercase()),
            metric_keys: e.metric_keys,
            traffic_allocation: e.traffic_allocation,
            min_sample_size: e.min_sample_size,
            scheduled_start_at: e.scheduled_start_at,
            scheduled_end_at: e.scheduled_end_at,
            version: e.version,
            created_at: e.created_at,
            updated_at: e.updated_at,
        }
    }
}

/// Response body for a single experiment iteration snapshot.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct ExperimentIterationResponse {
    /// Unique identifier of the iteration.
    pub id: ExperimentIterationId,
    /// The experiment this iteration belongs to.
    pub experiment_id: ExperimentId,
    /// Sequential iteration number within the experiment (1-based).
    pub iteration_number: i32,
    /// ISO-8601 timestamp of when this iteration started.
    pub started_at: DateTime<Utc>,
    /// ISO-8601 timestamp of when this iteration ended; `null` while running.
    pub ended_at: Option<DateTime<Utc>>,
    /// Snapshot of metric keys at iteration start.
    pub metric_keys: Vec<String>,
    /// Snapshot of traffic allocation at iteration start.
    pub traffic_allocation: f64,
    /// Snapshot of minimum sample size at iteration start.
    pub min_sample_size: Option<i64>,
}

impl From<ExperimentIteration> for ExperimentIterationResponse {
    fn from(i: ExperimentIteration) -> Self {
        Self {
            id: i.id,
            experiment_id: i.experiment_id,
            iteration_number: i.iteration_number,
            started_at: i.started_at,
            ended_at: i.ended_at,
            metric_keys: i.metric_keys,
            traffic_allocation: i.traffic_allocation,
            min_sample_size: i.min_sample_size,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn base_request() -> CreateExperimentRequest {
        CreateExperimentRequest {
            flag_rule_id: RuleId::new(),
            name: "My Experiment".to_string(),
            description: None,
            hypothesis: None,
            metric_keys: vec!["checkout_completed".to_string()],
            traffic_allocation: None,
            min_sample_size: None,
            scheduled_start_at: None,
            scheduled_end_at: None,
        }
    }

    #[test]
    fn test_create_request_validates_empty_metric_keys() {
        let req = CreateExperimentRequest {
            metric_keys: vec![],
            ..base_request()
        };
        let result = req.validate();
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .contains("metric_keys must contain at least one entry"));
    }

    #[test]
    fn test_create_request_validates_traffic_allocation_bounds() {
        // Too low
        let req_low = CreateExperimentRequest {
            traffic_allocation: Some(0.0),
            ..base_request()
        };
        let result_low = req_low.validate();
        assert!(result_low.is_err(), "0.0 should be invalid");

        // Too high
        let req_high = CreateExperimentRequest {
            traffic_allocation: Some(100.1),
            ..base_request()
        };
        let result_high = req_high.validate();
        assert!(result_high.is_err(), "100.1 should be invalid");

        // Negative
        let req_neg = CreateExperimentRequest {
            traffic_allocation: Some(-1.0),
            ..base_request()
        };
        let result_neg = req_neg.validate();
        assert!(result_neg.is_err(), "-1.0 should be invalid");
    }

    #[test]
    fn test_create_request_valid() {
        // Default (no allocation specified)
        let req = base_request();
        assert!(req.validate().is_ok());

        // Min boundary
        let req_min = CreateExperimentRequest {
            traffic_allocation: Some(0.1),
            ..base_request()
        };
        assert!(req_min.validate().is_ok());

        // Max boundary
        let req_max = CreateExperimentRequest {
            traffic_allocation: Some(100.0),
            ..base_request()
        };
        assert!(req_max.validate().is_ok());

        // Multiple metrics
        let req_multi = CreateExperimentRequest {
            metric_keys: vec!["metric_a".to_string(), "metric_b".to_string()],
            ..base_request()
        };
        assert!(req_multi.validate().is_ok());
    }

    #[test]
    fn test_experiment_response_from_experiment() {
        use stitchd_core::id::EnvironmentId;

        let exp = Experiment {
            id: ExperimentId::new(),
            environment_id: EnvironmentId::new(),
            flag_rule_id: RuleId::new(),
            name: "Test".to_string(),
            description: Some("desc".to_string()),
            hypothesis: None,
            metric_keys: vec!["m1".to_string()],
            traffic_allocation: 50.0,
            min_sample_size: Some(200),
            scheduled_start_at: None,
            scheduled_end_at: None,
            status: ExperimentStatus::Draft,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            deleted_at: None,
            version: 1,
        };

        let response = ExperimentResponse::from(exp);
        assert_eq!(response.status, "draft");
        assert_eq!(response.traffic_allocation, 50.0);
        assert_eq!(response.version, 1);
    }

    #[test]
    fn test_experiment_iteration_response_from_iteration() {
        use stitchd_core::id::ExperimentIterationId;

        let iter = ExperimentIteration {
            id: ExperimentIterationId::new(),
            experiment_id: ExperimentId::new(),
            iteration_number: 2,
            started_at: Utc::now(),
            ended_at: None,
            metric_keys: vec!["m1".to_string()],
            traffic_allocation: 75.0,
            min_sample_size: None,
        };

        let response = ExperimentIterationResponse::from(iter);
        assert_eq!(response.iteration_number, 2);
        assert_eq!(response.traffic_allocation, 75.0);
        assert!(response.ended_at.is_none());
    }
}
