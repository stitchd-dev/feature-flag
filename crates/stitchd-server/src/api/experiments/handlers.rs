//! REST handlers for experiment management.
//!
//! Endpoints:
//! - `POST   /v1/environments/{env_id}/experiments`                      — create
//! - `GET    /v1/environments/{env_id}/experiments`                      — list
//! - `GET    /v1/environments/{env_id}/experiments/{id}`                 — get
//! - `PATCH  /v1/environments/{env_id}/experiments/{id}`                 — update
//! - `DELETE /v1/environments/{env_id}/experiments/{id}`                 — soft-delete
//! - `POST   /v1/environments/{env_id}/experiments/{id}/transitions`     — lifecycle transition
//! - `GET    /v1/environments/{env_id}/experiments/{id}/iterations`      — list iterations

use axum::{
    Json,
    extract::{Path, Query, State},
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

use crate::AppState;

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
    /// - `traffic_allocation`, when provided, must be in the range \[0.1, 100.0\].
    ///
    /// # Errors
    /// Returns `Err` with a description if validation fails.
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
    /// Current lifecycle status serialized as a `snake_case` string.
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
            status: serde_json::to_value(e.status)
                .ok()
                .and_then(|v| v.as_str().map(ToString::to_string))
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
// Query parameters
// ---------------------------------------------------------------------------

/// Optional query parameters for listing experiments.
#[derive(Debug, Deserialize)]
pub struct ListExperimentsQuery {
    /// Filter by experiment status.
    pub status: Option<ExperimentStatus>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /v1/environments/{env_id}/experiments` — Create a new experiment.
///
/// # Errors
/// Returns 422 if validation fails; 500 on database error.
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/experiments",
    params(("env_id" = String, Path, description = "Environment UUID")),
    request_body = CreateExperimentRequest,
    responses(
        (status = 201, description = "Experiment created", body = ExperimentResponse),
        (status = 422, description = "Validation error"),
        (status = 500, description = "Internal error"),
    ),
    tag = "experiments"
)]
#[tracing::instrument(skip(state))]
pub async fn create_experiment(
    State(state): State<AppState>,
    Path(env_id): Path<EnvironmentId>,
    Json(req): Json<CreateExperimentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    if let Err(msg) = req.validate() {
        return Ok((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response());
    }

    let now = Utc::now();
    let experiment = Experiment {
        id: ExperimentId::new(),
        environment_id: env_id,
        flag_rule_id: req.flag_rule_id,
        name: req.name,
        description: req.description,
        hypothesis: req.hypothesis,
        metric_keys: req.metric_keys,
        traffic_allocation: req.traffic_allocation.unwrap_or(100.0),
        min_sample_size: req.min_sample_size,
        scheduled_start_at: req.scheduled_start_at,
        scheduled_end_at: req.scheduled_end_at,
        status: ExperimentStatus::Draft,
        created_at: now,
        updated_at: now,
        deleted_at: None,
        version: 1,
    };

    state.experiment_repo.create(&experiment).await?;

    Ok((StatusCode::CREATED, Json(ExperimentResponse::from(experiment))).into_response())
}

/// `GET /v1/environments/{env_id}/experiments` — List experiments in an environment.
///
/// # Errors
/// Returns 500 on database error.
#[utoipa::path(
    get,
    path = "/v1/environments/{env_id}/experiments",
    params(
        ("env_id" = String, Path, description = "Environment UUID"),
        ("status" = Option<String>, Query, description = "Filter by status"),
    ),
    responses(
        (status = 200, description = "List of experiments", body = Vec<ExperimentResponse>),
        (status = 500, description = "Internal error"),
    ),
    tag = "experiments"
)]
#[tracing::instrument(skip(state))]
pub async fn list_experiments(
    State(state): State<AppState>,
    Path(env_id): Path<EnvironmentId>,
    Query(params): Query<ListExperimentsQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let experiments = state
        .experiment_repo
        .list_by_environment(env_id, params.status)
        .await?;

    let responses: Vec<ExperimentResponse> = experiments.into_iter().map(Into::into).collect();
    Ok((StatusCode::OK, Json(responses)).into_response())
}

/// `GET /v1/environments/{env_id}/experiments/{id}` — Get a single experiment.
///
/// # Errors
/// Returns 404 if not found; 500 on database error.
#[utoipa::path(
    get,
    path = "/v1/environments/{env_id}/experiments/{id}",
    params(
        ("env_id" = String, Path, description = "Environment UUID"),
        ("id" = String, Path, description = "Experiment UUID"),
    ),
    responses(
        (status = 200, description = "Experiment found", body = ExperimentResponse),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error"),
    ),
    tag = "experiments"
)]
#[tracing::instrument(skip(state))]
pub async fn get_experiment(
    State(state): State<AppState>,
    Path((_, id)): Path<(EnvironmentId, ExperimentId)>,
) -> Result<impl IntoResponse, ApiError> {
    let experiment = state.experiment_repo.find_by_id(id).await?;
    Ok((StatusCode::OK, Json(ExperimentResponse::from(experiment))).into_response())
}

/// `PATCH /v1/environments/{env_id}/experiments/{id}` — Update an experiment.
///
/// # Errors
/// Returns 404 if not found; 409 on version conflict or invalid state; 500 on database error.
#[utoipa::path(
    patch,
    path = "/v1/environments/{env_id}/experiments/{id}",
    params(
        ("env_id" = String, Path, description = "Environment UUID"),
        ("id" = String, Path, description = "Experiment UUID"),
    ),
    request_body = UpdateExperimentRequest,
    responses(
        (status = 200, description = "Experiment updated", body = ExperimentResponse),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict"),
        (status = 500, description = "Internal error"),
    ),
    tag = "experiments"
)]
#[tracing::instrument(skip(state))]
pub async fn update_experiment(
    State(state): State<AppState>,
    Path((_, id)): Path<(EnvironmentId, ExperimentId)>,
    Json(req): Json<UpdateExperimentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let existing = state.experiment_repo.find_by_id(id).await?;

    let updated = Experiment {
        name: req.name.unwrap_or(existing.name),
        description: req.description.or(existing.description),
        hypothesis: req.hypothesis.or(existing.hypothesis),
        metric_keys: req.metric_keys.unwrap_or(existing.metric_keys),
        traffic_allocation: req
            .traffic_allocation
            .unwrap_or(existing.traffic_allocation),
        min_sample_size: req.min_sample_size.or(existing.min_sample_size),
        scheduled_start_at: req.scheduled_start_at.or(existing.scheduled_start_at),
        scheduled_end_at: req.scheduled_end_at.or(existing.scheduled_end_at),
        version: req.version,
        updated_at: Utc::now(),
        ..existing
    };

    let saved = state.experiment_repo.update(&updated).await?;
    Ok((StatusCode::OK, Json(ExperimentResponse::from(saved))).into_response())
}

/// `DELETE /v1/environments/{env_id}/experiments/{id}` — Soft-delete an experiment.
///
/// # Errors
/// Returns 404 if not found; 409 if experiment is currently running; 500 on database error.
#[utoipa::path(
    delete,
    path = "/v1/environments/{env_id}/experiments/{id}",
    params(
        ("env_id" = String, Path, description = "Environment UUID"),
        ("id" = String, Path, description = "Experiment UUID"),
    ),
    responses(
        (status = 204, description = "Experiment deleted"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Conflict — experiment is running"),
        (status = 500, description = "Internal error"),
    ),
    tag = "experiments"
)]
#[tracing::instrument(skip(state))]
pub async fn delete_experiment(
    State(state): State<AppState>,
    Path((_, id)): Path<(EnvironmentId, ExperimentId)>,
) -> Result<impl IntoResponse, ApiError> {
    state.experiment_repo.soft_delete(id).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// `POST /v1/environments/{env_id}/experiments/{id}/transitions` — Transition experiment lifecycle.
///
/// # Errors
/// Returns 404 if not found; 409 on invalid transition; 500 on database error.
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/experiments/{id}/transitions",
    params(
        ("env_id" = String, Path, description = "Environment UUID"),
        ("id" = String, Path, description = "Experiment UUID"),
    ),
    request_body = TransitionRequest,
    responses(
        (status = 200, description = "Experiment transitioned", body = ExperimentResponse),
        (status = 404, description = "Not found"),
        (status = 409, description = "Invalid transition"),
        (status = 500, description = "Internal error"),
    ),
    tag = "experiments"
)]
#[tracing::instrument(skip(state))]
pub async fn transition_experiment(
    State(state): State<AppState>,
    Path((_, id)): Path<(EnvironmentId, ExperimentId)>,
    Json(req): Json<TransitionRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let experiment = state
        .experiment_repo
        .apply_transition(id, req.to, None)
        .await?;
    Ok((StatusCode::OK, Json(ExperimentResponse::from(experiment))).into_response())
}

/// `GET /v1/environments/{env_id}/experiments/{id}/iterations` — List iterations for an experiment.
///
/// # Errors
/// Returns 404 if not found; 500 on database error.
#[utoipa::path(
    get,
    path = "/v1/environments/{env_id}/experiments/{id}/iterations",
    params(
        ("env_id" = String, Path, description = "Environment UUID"),
        ("id" = String, Path, description = "Experiment UUID"),
    ),
    responses(
        (status = 200, description = "List of iterations", body = Vec<ExperimentIterationResponse>),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error"),
    ),
    tag = "experiments"
)]
#[tracing::instrument(skip(state))]
pub async fn list_iterations(
    State(state): State<AppState>,
    Path((_, id)): Path<(EnvironmentId, ExperimentId)>,
) -> Result<impl IntoResponse, ApiError> {
    let iterations = state.experiment_repo.list_iterations(id).await?;
    let responses: Vec<ExperimentIterationResponse> =
        iterations.into_iter().map(Into::into).collect();
    Ok((StatusCode::OK, Json(responses)).into_response())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode, header},
        routing::{get, post},
    };
    use metrics_exporter_prometheus::PrometheusBuilder;
    use sqlx::PgPool;
    use std::sync::Arc;
    use tower::ServiceExt as _;

    // ---------------------------------------------------------------------------
    // Stub repository for HTTP integration tests
    // ---------------------------------------------------------------------------

    struct StubExperimentRepository;

    #[async_trait]
    impl stitchd_db::ExperimentRepository for StubExperimentRepository {
        async fn find_by_id(
            &self,
            id: ExperimentId,
        ) -> Result<Experiment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
            _status_filter: Option<ExperimentStatus>,
        ) -> Result<Vec<Experiment>, RepositoryError> {
            Ok(vec![])
        }

        async fn create(&self, _experiment: &Experiment) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn update(
            &self,
            experiment: &Experiment,
        ) -> Result<Experiment, RepositoryError> {
            Ok(experiment.clone())
        }

        async fn soft_delete(&self, id: ExperimentId) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_iterations(
            &self,
            _experiment_id: ExperimentId,
        ) -> Result<Vec<stitchd_core::experimentation::ExperimentIteration>, RepositoryError>
        {
            Ok(vec![])
        }

        async fn apply_transition(
            &self,
            id: ExperimentId,
            _to: ExperimentStatus,
            _actor_id: Option<stitchd_core::id::UserId>,
        ) -> Result<Experiment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
    }

    /// Stub that succeeds for create/apply_transition (returns a synthetic experiment).
    struct SuccessExperimentRepository {
        env_id: EnvironmentId,
        flag_rule_id: RuleId,
    }

    #[async_trait]
    impl stitchd_db::ExperimentRepository for SuccessExperimentRepository {
        async fn find_by_id(&self, id: ExperimentId) -> Result<Experiment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_by_environment(
            &self,
            _env_id: EnvironmentId,
            _status_filter: Option<ExperimentStatus>,
        ) -> Result<Vec<Experiment>, RepositoryError> {
            Ok(vec![])
        }

        async fn create(&self, experiment: &Experiment) -> Result<(), RepositoryError> {
            let _ = experiment;
            Ok(())
        }

        async fn update(&self, experiment: &Experiment) -> Result<Experiment, RepositoryError> {
            Ok(experiment.clone())
        }

        async fn soft_delete(&self, _id: ExperimentId) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn list_iterations(
            &self,
            _experiment_id: ExperimentId,
        ) -> Result<Vec<stitchd_core::experimentation::ExperimentIteration>, RepositoryError>
        {
            Ok(vec![])
        }

        async fn apply_transition(
            &self,
            _id: ExperimentId,
            to: ExperimentStatus,
            _actor_id: Option<stitchd_core::id::UserId>,
        ) -> Result<Experiment, RepositoryError> {
            Ok(Experiment {
                id: ExperimentId::new(),
                environment_id: self.env_id,
                flag_rule_id: self.flag_rule_id,
                name: "Test Experiment".to_string(),
                description: None,
                hypothesis: None,
                metric_keys: vec!["m1".to_string()],
                traffic_allocation: 100.0,
                min_sample_size: None,
                scheduled_start_at: None,
                scheduled_end_at: None,
                status: to,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                deleted_at: None,
                version: 2,
            })
        }
    }

    fn make_stub_app_state(
        experiment_repo: Arc<dyn stitchd_db::ExperimentRepository>,
    ) -> crate::AppState {
        use stitchd_db::{
            FlagRepository, RepositoryError, SegmentRepository, SdkKeyRepository,
            VariantRepository, EventDefinitionRepository,
        };
        use stitchd_core::{
            flag::{FlagRecord, FlagHashingConfig, FlagRule},
            id::{FlagId, FlagKey, ProjectId, SdkKeyId, SegmentId, VariantId, EventDefinitionId},
            segment::{ListBasedSegment, RuleBasedSegment, Segment},
            tenant::SdkKey,
            flag::Variant,
            event::EventDefinition,
        };
        use std::collections::HashMap;

        struct NullRepo;

        #[async_trait]
        impl FlagRepository for NullRepo {
            async fn find_by_id(&self, id: FlagId) -> Result<FlagRecord, RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_by_key(&self, key: &FlagKey, _: ProjectId) -> Result<FlagRecord, RepositoryError> {
                Err(RepositoryError::NotFound { id: key.to_string() })
            }
            async fn list_by_project(&self, _: ProjectId) -> Result<Vec<FlagRecord>, RepositoryError> { Ok(vec![]) }
            async fn list_by_environment(&self, _: EnvironmentId) -> Result<Vec<FlagRecord>, RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &FlagRecord) -> Result<(), RepositoryError> { Ok(()) }
            async fn update(&self, f: &FlagRecord) -> Result<FlagRecord, RepositoryError> { Ok(f.clone()) }
            async fn soft_delete(&self, id: FlagId) -> Result<(), RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_hashing_config(&self, _: FlagId) -> Result<Vec<FlagHashingConfig>, RepositoryError> { Ok(vec![]) }
            async fn upsert_hashing_config(&self, _: FlagId, _: &[FlagHashingConfig]) -> Result<(), RepositoryError> { Ok(()) }
            async fn find_rules(&self, _: FlagId) -> Result<Vec<FlagRule>, RepositoryError> { Ok(vec![]) }
            async fn upsert_rules(&self, _: FlagId, _: &[FlagRule]) -> Result<(), RepositoryError> { Ok(()) }
        }

        #[async_trait]
        impl VariantRepository for NullRepo {
            async fn find_by_flag(&self, _: FlagId) -> Result<Vec<Variant>, RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: FlagId, _: &Variant) -> Result<(), RepositoryError> { Ok(()) }
            async fn update(&self, v: &Variant) -> Result<Variant, RepositoryError> { Ok(v.clone()) }
            async fn delete(&self, _: VariantId) -> Result<(), RepositoryError> { Ok(()) }
        }

        #[async_trait]
        impl SegmentRepository for NullRepo {
            async fn find_by_id(&self, id: SegmentId) -> Result<Segment, RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_by_key(&self, key: &str, _: EnvironmentId) -> Result<Segment, RepositoryError> {
                Err(RepositoryError::NotFound { id: key.to_string() })
            }
            async fn list_by_environment(&self, _: EnvironmentId) -> Result<Vec<Segment>, RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &Segment) -> Result<(), RepositoryError> { Ok(()) }
            async fn update(&self, s: &Segment) -> Result<Segment, RepositoryError> { Ok(s.clone()) }
            async fn find_with_rules(&self, id: SegmentId) -> Result<RuleBasedSegment, RepositoryError> {
                Ok(RuleBasedSegment { id, rules: vec![] })
            }
            async fn find_with_list(&self, id: SegmentId) -> Result<ListBasedSegment, RepositoryError> {
                Ok(ListBasedSegment { id, lists: HashMap::new() })
            }
            async fn upsert_rules(&self, _: SegmentId, _: &[stitchd_core::rule_engine::types::Rule]) -> Result<(), RepositoryError> { Ok(()) }
            async fn set_list_entries(&self, _: SegmentId, _: &str, _: &[String], _: &[String]) -> Result<(), RepositoryError> { Ok(()) }
            async fn soft_delete(&self, _: SegmentId) -> Result<(), RepositoryError> { Ok(()) }
            async fn check_list_membership(&self, _: EnvironmentId, _: &str, _: &str, keys: &[String]) -> Result<HashMap<String, bool>, RepositoryError> {
                Ok(keys.iter().map(|k| (k.clone(), false)).collect())
            }
            async fn batch_check_list_membership(&self, _: EnvironmentId, _: &[(String, String)], _: &[String]) -> Result<Vec<stitchd_db::ContextMembership>, RepositoryError> { Ok(vec![]) }
        }

        #[async_trait]
        impl SdkKeyRepository for NullRepo {
            async fn find_by_id(&self, id: SdkKeyId) -> Result<SdkKey, RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn list_by_environment(&self, _: EnvironmentId) -> Result<Vec<SdkKey>, RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &SdkKey) -> Result<(), RepositoryError> { Ok(()) }
            async fn revoke(&self, id: SdkKeyId) -> Result<(), RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_active_by_environment(&self, _: EnvironmentId) -> Result<Vec<SdkKey>, RepositoryError> { Ok(vec![]) }
            async fn find_active_by_hash(&self, h: &str) -> Result<SdkKey, RepositoryError> {
                Err(RepositoryError::NotFound { id: h.to_string() })
            }
        }

        #[async_trait]
        impl EventDefinitionRepository for NullRepo {
            async fn find_by_id(&self, id: EventDefinitionId) -> Result<EventDefinition, RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_by_key(&self, key: &str, _: EnvironmentId) -> Result<EventDefinition, RepositoryError> {
                Err(RepositoryError::NotFound { id: key.to_string() })
            }
            async fn list_by_environment(&self, _: EnvironmentId) -> Result<Vec<EventDefinition>, RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &EventDefinition) -> Result<(), RepositoryError> { Ok(()) }
            async fn update(&self, d: &EventDefinition) -> Result<EventDefinition, RepositoryError> { Ok(d.clone()) }
            async fn soft_delete(&self, id: EventDefinitionId) -> Result<(), RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
        }

        let null = Arc::new(NullRepo);
        crate::AppState {
            db: PgPool::connect_lazy("postgres://stitchd:stitchd@localhost:5432/stitchd_test")
                .expect("lazy pool"),
            metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
            segment_repo: null.clone(),
            flag_repo: null.clone(),
            variant_repo: null.clone(),
            sdk_key_repo: null.clone(),
            event_definition_repo: null,
            experiment_repo,
            event_writer: None,
        }
    }

    fn build_experiment_router(experiment_repo: Arc<dyn stitchd_db::ExperimentRepository>) -> Router {
        let state = make_stub_app_state(experiment_repo);
        Router::new()
            .route(
                "/v1/environments/{env_id}/experiments",
                get(list_experiments).post(create_experiment),
            )
            .route(
                "/v1/environments/{env_id}/experiments/{id}",
                get(get_experiment)
                    .patch(update_experiment)
                    .delete(delete_experiment),
            )
            .route(
                "/v1/environments/{env_id}/experiments/{id}/transitions",
                post(transition_experiment),
            )
            .route(
                "/v1/environments/{env_id}/experiments/{id}/iterations",
                get(list_iterations),
            )
            .with_state(state)
    }

    // ---------------------------------------------------------------------------
    // HTTP integration tests
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn post_experiments_with_valid_body_returns_201() {
        let env_id = EnvironmentId::new();
        let flag_rule_id = RuleId::new();
        let repo = Arc::new(SuccessExperimentRepository { env_id, flag_rule_id });
        let app = build_experiment_router(repo);

        let body = serde_json::json!({
            "flag_rule_id": flag_rule_id,
            "name": "Test Experiment",
            "metric_keys": ["checkout_completed"],
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/environments/{env_id}/experiments"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn post_experiments_with_empty_metric_keys_returns_422() {
        let env_id = EnvironmentId::new();
        let flag_rule_id = RuleId::new();
        let repo = Arc::new(SuccessExperimentRepository { env_id, flag_rule_id });
        let app = build_experiment_router(repo);

        let body = serde_json::json!({
            "flag_rule_id": flag_rule_id,
            "name": "Test Experiment",
            "metric_keys": [],
        });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/environments/{env_id}/experiments"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn get_experiment_not_found_returns_404() {
        let env_id = EnvironmentId::new();
        let exp_id = ExperimentId::new();
        let repo = Arc::new(StubExperimentRepository);
        let app = build_experiment_router(repo);

        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/environments/{env_id}/experiments/{exp_id}"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn post_transitions_returns_200() {
        let env_id = EnvironmentId::new();
        let exp_id = ExperimentId::new();
        let flag_rule_id = RuleId::new();
        let repo = Arc::new(SuccessExperimentRepository { env_id, flag_rule_id });
        let app = build_experiment_router(repo);

        let body = serde_json::json!({ "to": "running" });

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/environments/{env_id}/experiments/{exp_id}/transitions"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

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
