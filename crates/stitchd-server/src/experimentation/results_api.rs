//! Response types and route handlers for the experiment results API.
//!
//! These types mirror [`stitchd_db::experiment_results::ExperimentResultRow`]
//! but expose typed fields with utoipa schema annotations for OpenAPI doc gen.
//!
//! # Endpoints
//! - `GET  /v1/environments/{env_id}/experiments/{id}/results`
//! - `GET  /v1/environments/{env_id}/experiments/{id}/iterations/{iter_id}/results`
//! - `POST /v1/environments/{env_id}/experiments/{id}/results/recompute`

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// Full statistical results for one experiment iteration, covering all metrics.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ExperimentResultsResponse {
    /// The experiment these results belong to.
    pub experiment_id: Uuid,
    /// The specific iteration these results were computed for.
    pub iteration_id: Uuid,
    /// When these results were last computed.
    pub computed_at: DateTime<Utc>,
    /// Per-metric statistical results.
    pub metrics: Vec<MetricResultResponse>,
}

/// Statistical results for a single metric within an experiment iteration.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MetricResultResponse {
    /// Metric key (e.g. `"checkout_completed"`).
    pub metric_key: String,
    /// Metric type discriminant: `"count"`, `"numeric"`, `"percentile"`, or `"funnel"`.
    pub metric_type: String,
    /// Per-variant sample statistics (JSONB object).
    pub variant_stats: serde_json::Value,
    /// Frequentist analysis result, if computed; `null` for Bayesian-only runs.
    pub frequentist_result: Option<serde_json::Value>,
    /// Bayesian analysis result, if computed; `null` for frequentist-only runs.
    pub bayesian_result: Option<serde_json::Value>,
    /// Human-readable recommendation string (e.g. `"ship_treatment"`, `"needs_more_data"`).
    pub recommendation: String,
}

/// Response body for a fire-and-forget recompute request.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RecomputeResponse {
    /// Always `"accepted"` — the computation has been queued asynchronously.
    pub status: String,
    /// The experiment that will be recomputed.
    pub experiment_id: Uuid,
    /// The iteration for which results will be recomputed.
    pub iteration_id: Uuid,
}

// ---------------------------------------------------------------------------
// Conversions from DB row types
// ---------------------------------------------------------------------------

use stitchd_db::experiment_results::ExperimentResultRow;

impl MetricResultResponse {
    /// Build a `MetricResultResponse` from a persisted [`ExperimentResultRow`].
    #[must_use]
    pub fn from_row(row: &ExperimentResultRow) -> Self {
        Self {
            metric_key: row.metric_key.clone(),
            metric_type: row.metric_type.clone(),
            variant_stats: row.variant_stats.clone(),
            frequentist_result: row.frequentist_result.clone(),
            bayesian_result: row.bayesian_result.clone(),
            recommendation: row.recommendation.clone(),
        }
    }
}

impl ExperimentResultsResponse {
    /// Build an `ExperimentResultsResponse` from a non-empty slice of
    /// [`ExperimentResultRow`]s that all share the same
    /// `(experiment_id, iteration_id)`.
    ///
    /// `computed_at` is taken as the *minimum* `computed_at` across all rows
    /// (i.e. the time at which the oldest metric was last recomputed).
    ///
    /// # Panics
    /// Panics if `rows` is empty.
    #[must_use]
    pub fn from_rows(rows: &[ExperimentResultRow]) -> Self {
        assert!(!rows.is_empty(), "rows must not be empty");
        let experiment_id = rows[0].experiment_id;
        let iteration_id = rows[0].iteration_id;
        let computed_at = rows
            .iter()
            .map(|r| r.computed_at)
            .min()
            .expect("non-empty slice always has a minimum");
        let metrics = rows.iter().map(MetricResultResponse::from_row).collect();
        Self {
            experiment_id,
            iteration_id,
            computed_at,
            metrics,
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use stitchd_core::id::{EnvironmentId, ExperimentId, ExperimentIterationId};

use crate::AppState;
use crate::api::experiments::handlers::ApiError;

/// `GET /v1/environments/{env_id}/experiments/{id}/results`
///
/// Fetches the most-recent iteration's results.  If the results are stale a
/// background recompute is fired-and-forgotten; the (possibly-stale) current
/// data is still returned immediately.
///
/// Returns 404 when no results have ever been computed for this experiment.
///
/// # Errors
/// Returns 404 if no results exist; 500 on database error.
#[utoipa::path(
    get,
    path = "/v1/environments/{env_id}/experiments/{id}/results",
    params(
        ("env_id" = String, Path, description = "Environment UUID"),
        ("id" = String, Path, description = "Experiment UUID"),
    ),
    responses(
        (status = 200, description = "Latest iteration results", body = ExperimentResultsResponse),
        (status = 404, description = "No results found"),
        (status = 500, description = "Internal error"),
    ),
    tag = "experiment-results"
)]
#[tracing::instrument(skip(state))]
pub async fn get_latest_results(
    State(state): State<AppState>,
    Path((_, experiment_id)): Path<(EnvironmentId, ExperimentId)>,
) -> Result<impl IntoResponse, ApiError> {
    let rows = state
        .results_repo
        .fetch_latest(experiment_id.as_uuid())
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    if rows.is_empty() {
        return Err(ApiError::NotFound(format!(
            "no results found for experiment {experiment_id}"
        )));
    }

    let iteration_id = rows[0].iteration_id;

    // Fire-and-forget recompute if results are stale.
    if matches!(
        state
            .results_repo
            .is_stale(experiment_id.as_uuid(), iteration_id)
            .await,
        Ok(true)
    ) {
        maybe_spawn_recompute(&state, experiment_id.as_uuid(), iteration_id);
    }

    let response = ExperimentResultsResponse::from_rows(&rows);
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `GET /v1/environments/{env_id}/experiments/{id}/iterations/{iter_id}/results`
///
/// Fetches results for a specific iteration.
///
/// # Errors
/// Returns 404 if no results exist for the given iteration; 500 on database error.
#[utoipa::path(
    get,
    path = "/v1/environments/{env_id}/experiments/{id}/iterations/{iter_id}/results",
    params(
        ("env_id" = String, Path, description = "Environment UUID"),
        ("id" = String, Path, description = "Experiment UUID"),
        ("iter_id" = String, Path, description = "Iteration UUID"),
    ),
    responses(
        (status = 200, description = "Iteration results", body = ExperimentResultsResponse),
        (status = 404, description = "No results found"),
        (status = 500, description = "Internal error"),
    ),
    tag = "experiment-results"
)]
#[tracing::instrument(skip(state))]
pub async fn get_iteration_results(
    State(state): State<AppState>,
    Path((_, experiment_id, iteration_id)): Path<(
        EnvironmentId,
        ExperimentId,
        ExperimentIterationId,
    )>,
) -> Result<impl IntoResponse, ApiError> {
    let rows = state
        .results_repo
        .fetch_by_iteration(experiment_id.as_uuid(), iteration_id.as_uuid())
        .await
        .map_err(|e| ApiError::Database(e.to_string()))?;

    if rows.is_empty() {
        return Err(ApiError::NotFound(format!(
            "no results found for experiment {experiment_id} iteration {iteration_id}"
        )));
    }

    let response = ExperimentResultsResponse::from_rows(&rows);
    Ok((StatusCode::OK, Json(response)).into_response())
}

/// `POST /v1/environments/{env_id}/experiments/{id}/results/recompute`
///
/// Enqueues a fire-and-forget statistical recompute for the experiment's current
/// iteration.  Returns 202 Accepted immediately.
///
/// # Errors
/// Returns 404 if the experiment has no iterations yet; 500 on database error.
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/experiments/{id}/results/recompute",
    params(
        ("env_id" = String, Path, description = "Environment UUID"),
        ("id" = String, Path, description = "Experiment UUID"),
    ),
    responses(
        (status = 202, description = "Recompute accepted", body = RecomputeResponse),
        (status = 404, description = "No active iteration found"),
        (status = 500, description = "Internal error"),
    ),
    tag = "experiment-results"
)]
#[tracing::instrument(skip(state))]
pub async fn recompute_results(
    State(state): State<AppState>,
    Path((_, experiment_id)): Path<(EnvironmentId, ExperimentId)>,
) -> Result<impl IntoResponse, ApiError> {
    // Determine the latest iteration by looking at existing iterations.
    let iterations = state.experiment_repo.list_iterations(experiment_id).await?;

    let latest = iterations
        .into_iter()
        .max_by_key(|i| i.iteration_number)
        .ok_or_else(|| {
            ApiError::NotFound(format!("experiment {experiment_id} has no iterations"))
        })?;

    // Spawn recompute job fire-and-forget (best-effort; no ClickHouse = no-op).
    maybe_spawn_recompute(&state, experiment_id.as_uuid(), latest.id.as_uuid());

    let response = RecomputeResponse {
        status: "accepted".to_string(),
        experiment_id: experiment_id.as_uuid(),
        iteration_id: latest.id.as_uuid(),
    };

    Ok((StatusCode::ACCEPTED, Json(response)).into_response())
}

/// Spawn a compute job if a ClickHouse client is configured.
///
/// Intentionally discards the `JoinHandle`; errors are logged inside the task.
fn maybe_spawn_recompute(state: &AppState, experiment_id: Uuid, iteration_id: Uuid) {
    if let Some(ch) = state.ch_client.clone() {
        use crate::experimentation::compute::{ComputeResultsJob, spawn_compute_job};
        use stitchd_core::experimentation::stats::AnalysisType;

        let job = ComputeResultsJob {
            experiment_id,
            iteration_id,
            analysis_type: AnalysisType::Frequentist,
            metrics: vec![], // caller is expected to populate metrics for real runs
            date_from: chrono::Utc::now(),
            date_to: chrono::Utc::now(),
            min_sample_size: None,
            env_id: Uuid::new_v4(),
        };
        let _handle = spawn_compute_job(job, ch, state.results_repo.clone());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn make_row(metric_key: &str) -> ExperimentResultRow {
        ExperimentResultRow {
            id: Uuid::new_v4(),
            experiment_id: Uuid::new_v4(),
            iteration_id: Uuid::new_v4(),
            metric_key: metric_key.to_string(),
            metric_type: "count".to_string(),
            variant_stats: serde_json::json!({ "control": 100, "treatment": 120 }),
            frequentist_result: Some(serde_json::json!({ "p_value": 0.03 })),
            bayesian_result: None,
            recommendation: "ship_treatment".to_string(),
            computed_at: Utc::now(),
            created_at: Utc::now(),
        }
    }

    // ── Round-trip: ExperimentResultsResponse ──────────────────────────────

    #[test]
    fn experiment_results_response_roundtrip() {
        let row = make_row("checkout");
        let response = ExperimentResultsResponse::from_rows(std::slice::from_ref(&row));

        let json = serde_json::to_string(&response).expect("serialize");
        let deserialized: ExperimentResultsResponse =
            serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.experiment_id, response.experiment_id);
        assert_eq!(deserialized.iteration_id, response.iteration_id);
        assert_eq!(deserialized.metrics.len(), 1);
        assert_eq!(deserialized.metrics[0].metric_key, "checkout");
    }

    // ── Round-trip: MetricResultResponse ───────────────────────────────────

    #[test]
    fn metric_result_response_roundtrip() {
        let row = make_row("revenue");
        let metric = MetricResultResponse::from_row(&row);

        let json = serde_json::to_string(&metric).expect("serialize");
        let deserialized: MetricResultResponse = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.metric_key, "revenue");
        assert_eq!(deserialized.metric_type, "count");
        assert_eq!(deserialized.recommendation, "ship_treatment");
        assert!(deserialized.frequentist_result.is_some());
        assert!(deserialized.bayesian_result.is_none());
    }

    // ── Round-trip: RecomputeResponse ──────────────────────────────────────

    #[test]
    fn recompute_response_roundtrip() {
        let response = RecomputeResponse {
            status: "accepted".to_string(),
            experiment_id: Uuid::new_v4(),
            iteration_id: Uuid::new_v4(),
        };

        let json = serde_json::to_string(&response).expect("serialize");
        let deserialized: RecomputeResponse = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.status, "accepted");
        assert_eq!(deserialized.experiment_id, response.experiment_id);
        assert_eq!(deserialized.iteration_id, response.iteration_id);
    }

    // ── ExperimentResultsResponse::from_rows picks minimum computed_at ─────

    #[test]
    fn from_rows_picks_minimum_computed_at() {
        let base = Uuid::new_v4();
        let iter = Uuid::new_v4();
        let earlier = Utc::now() - chrono::Duration::hours(1);
        let later = Utc::now();

        let row1 = ExperimentResultRow {
            id: Uuid::new_v4(),
            experiment_id: base,
            iteration_id: iter,
            metric_key: "m1".to_string(),
            metric_type: "count".to_string(),
            variant_stats: serde_json::json!({}),
            frequentist_result: None,
            bayesian_result: None,
            recommendation: "inconclusive".to_string(),
            computed_at: earlier,
            created_at: Utc::now(),
        };
        let row2 = ExperimentResultRow {
            id: Uuid::new_v4(),
            experiment_id: base,
            iteration_id: iter,
            metric_key: "m2".to_string(),
            metric_type: "numeric".to_string(),
            variant_stats: serde_json::json!({}),
            frequentist_result: None,
            bayesian_result: None,
            recommendation: "needs_more_data".to_string(),
            computed_at: later,
            created_at: Utc::now(),
        };

        let response = ExperimentResultsResponse::from_rows(&[row1, row2]);
        assert_eq!(response.computed_at, earlier);
        assert_eq!(response.metrics.len(), 2);
    }

    // ── MetricResultResponse preserves optional JSON fields ────────────────

    #[test]
    fn metric_result_response_with_bayesian_result() {
        let mut row = make_row("funnel_metric");
        row.bayesian_result = Some(serde_json::json!({ "prob_best": 0.92 }));
        row.frequentist_result = None;
        row.metric_type = "funnel".to_string();
        row.recommendation = "needs_more_data".to_string();

        let metric = MetricResultResponse::from_row(&row);
        assert!(metric.bayesian_result.is_some());
        assert!(metric.frequentist_result.is_none());

        let json = serde_json::to_string(&metric).expect("serialize");
        let deserialized: MetricResultResponse = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deserialized.metric_type, "funnel");
        assert!(deserialized.bayesian_result.is_some());
    }

    // ── Handler integration tests ──────────────────────────────────────────

    use async_trait::async_trait;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode as HttpStatusCode, header},
        routing::{get, post},
    };
    use metrics_exporter_prometheus::PrometheusBuilder;
    use sqlx::PgPool;
    use std::sync::Arc;
    use stitchd_core::id::{EnvironmentId, ExperimentId, ExperimentIterationId};
    use stitchd_db::experiment_results::{
        ExperimentResultRow, ExperimentResultsRepository, UpsertResultRow,
    };
    use tower::ServiceExt as _;

    // ── Stub: empty results repository (always returns empty) ─────────────

    struct EmptyResultsRepo;

    #[async_trait]
    impl ExperimentResultsRepository for EmptyResultsRepo {
        async fn upsert(&self, _: &UpsertResultRow) -> Result<ExperimentResultRow, sqlx::Error> {
            Err(sqlx::Error::RowNotFound)
        }
        async fn fetch_latest(&self, _: Uuid) -> Result<Vec<ExperimentResultRow>, sqlx::Error> {
            Ok(vec![])
        }
        async fn fetch_by_iteration(
            &self,
            _: Uuid,
            _: Uuid,
        ) -> Result<Vec<ExperimentResultRow>, sqlx::Error> {
            Ok(vec![])
        }
        async fn is_stale(&self, _: Uuid, _: Uuid) -> Result<bool, sqlx::Error> {
            Ok(false)
        }
    }

    // ── Stub: repository that always returns one result row ────────────────

    struct OneRowResultsRepo {
        experiment_id: Uuid,
        iteration_id: Uuid,
    }

    impl OneRowResultsRepo {
        fn make_row(&self, metric_key: &str) -> ExperimentResultRow {
            ExperimentResultRow {
                id: Uuid::new_v4(),
                experiment_id: self.experiment_id,
                iteration_id: self.iteration_id,
                metric_key: metric_key.to_string(),
                metric_type: "count".to_string(),
                variant_stats: serde_json::json!({}),
                frequentist_result: None,
                bayesian_result: None,
                recommendation: "needs_more_data".to_string(),
                computed_at: Utc::now(),
                created_at: Utc::now(),
            }
        }
    }

    #[async_trait]
    impl ExperimentResultsRepository for OneRowResultsRepo {
        async fn upsert(&self, _: &UpsertResultRow) -> Result<ExperimentResultRow, sqlx::Error> {
            Err(sqlx::Error::RowNotFound)
        }
        async fn fetch_latest(&self, _: Uuid) -> Result<Vec<ExperimentResultRow>, sqlx::Error> {
            Ok(vec![self.make_row("checkout")])
        }
        async fn fetch_by_iteration(
            &self,
            _: Uuid,
            _: Uuid,
        ) -> Result<Vec<ExperimentResultRow>, sqlx::Error> {
            Ok(vec![self.make_row("checkout")])
        }
        async fn is_stale(&self, _: Uuid, _: Uuid) -> Result<bool, sqlx::Error> {
            Ok(false)
        }
    }

    // ── Stub ExperimentRepository ──────────────────────────────────────────

    struct StubExperimentRepoWithIter {
        iteration_id: Uuid,
    }

    #[async_trait]
    impl stitchd_db::ExperimentRepository for StubExperimentRepoWithIter {
        async fn find_by_id(
            &self,
            id: ExperimentId,
        ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
        {
            Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
        }
        async fn list_by_environment(
            &self,
            _: EnvironmentId,
            _: Option<stitchd_core::experimentation::ExperimentStatus>,
        ) -> Result<Vec<stitchd_core::experimentation::Experiment>, stitchd_db::RepositoryError>
        {
            Ok(vec![])
        }
        async fn create(
            &self,
            _: &stitchd_core::experimentation::Experiment,
        ) -> Result<(), stitchd_db::RepositoryError> {
            Ok(())
        }
        async fn update(
            &self,
            e: &stitchd_core::experimentation::Experiment,
        ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
        {
            Ok(e.clone())
        }
        async fn soft_delete(&self, id: ExperimentId) -> Result<(), stitchd_db::RepositoryError> {
            Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
        }
        async fn list_iterations(
            &self,
            experiment_id: ExperimentId,
        ) -> Result<
            Vec<stitchd_core::experimentation::ExperimentIteration>,
            stitchd_db::RepositoryError,
        > {
            Ok(vec![stitchd_core::experimentation::ExperimentIteration {
                id: ExperimentIterationId::from_uuid(self.iteration_id),
                experiment_id,
                iteration_number: 1,
                started_at: Utc::now(),
                ended_at: None,
                metric_keys: vec!["checkout".to_string()],
                traffic_allocation: 100.0,
                min_sample_size: None,
            }])
        }
        async fn apply_transition(
            &self,
            id: ExperimentId,
            _: stitchd_core::experimentation::ExperimentStatus,
            _: Option<stitchd_core::id::UserId>,
        ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
        {
            Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
        }
    }

    struct StubExperimentRepoEmpty;

    #[async_trait]
    impl stitchd_db::ExperimentRepository for StubExperimentRepoEmpty {
        async fn find_by_id(
            &self,
            id: ExperimentId,
        ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
        {
            Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
        }
        async fn list_by_environment(
            &self,
            _: EnvironmentId,
            _: Option<stitchd_core::experimentation::ExperimentStatus>,
        ) -> Result<Vec<stitchd_core::experimentation::Experiment>, stitchd_db::RepositoryError>
        {
            Ok(vec![])
        }
        async fn create(
            &self,
            _: &stitchd_core::experimentation::Experiment,
        ) -> Result<(), stitchd_db::RepositoryError> {
            Ok(())
        }
        async fn update(
            &self,
            e: &stitchd_core::experimentation::Experiment,
        ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
        {
            Ok(e.clone())
        }
        async fn soft_delete(&self, id: ExperimentId) -> Result<(), stitchd_db::RepositoryError> {
            Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
        }
        async fn list_iterations(
            &self,
            _: ExperimentId,
        ) -> Result<
            Vec<stitchd_core::experimentation::ExperimentIteration>,
            stitchd_db::RepositoryError,
        > {
            Ok(vec![])
        }
        async fn apply_transition(
            &self,
            id: ExperimentId,
            _: stitchd_core::experimentation::ExperimentStatus,
            _: Option<stitchd_core::id::UserId>,
        ) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError>
        {
            Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
        }
    }

    // ── Router builder ─────────────────────────────────────────────────────

    #[allow(clippy::too_many_lines)]
    fn build_results_router(
        results_repo: Arc<dyn ExperimentResultsRepository>,
        experiment_repo: Arc<dyn stitchd_db::ExperimentRepository>,
    ) -> Router {
        use std::collections::HashMap;
        use stitchd_core::{
            event::EventDefinition,
            flag::Variant,
            flag::{FlagHashingConfig, FlagRecord, FlagRule},
            id::{EventDefinitionId, FlagId, FlagKey, ProjectId, SdkKeyId, SegmentId, VariantId},
            segment::{ListBasedSegment, RuleBasedSegment, Segment},
            tenant::SdkKey,
        };
        use stitchd_core::auth::User;
        use stitchd_core::id::{OrganisationId, UserId};
        use stitchd_core::user::Permission;
        use stitchd_db::{
            EventDefinitionRepository, FlagRepository, RepositoryError, SdkKeyRepository,
            SegmentRepository, UserRepository, VariantRepository,
        };

        struct NullRepo;

        #[async_trait]
        impl FlagRepository for NullRepo {
            async fn find_by_id(&self, id: FlagId) -> Result<FlagRecord, RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_by_key(
                &self,
                key: &FlagKey,
                _: ProjectId,
            ) -> Result<FlagRecord, RepositoryError> {
                Err(RepositoryError::NotFound {
                    id: key.to_string(),
                })
            }
            async fn list_by_project(
                &self,
                _: ProjectId,
            ) -> Result<Vec<FlagRecord>, RepositoryError> {
                Ok(vec![])
            }
            async fn list_by_environment(
                &self,
                _: EnvironmentId,
            ) -> Result<Vec<FlagRecord>, RepositoryError> {
                Ok(vec![])
            }
            async fn create(&self, _: &FlagRecord) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn update(&self, f: &FlagRecord) -> Result<FlagRecord, RepositoryError> {
                Ok(f.clone())
            }
            async fn soft_delete(&self, id: FlagId) -> Result<(), RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_hashing_config(
                &self,
                _: FlagId,
            ) -> Result<Vec<FlagHashingConfig>, RepositoryError> {
                Ok(vec![])
            }
            async fn upsert_hashing_config(
                &self,
                _: FlagId,
                _: &[FlagHashingConfig],
            ) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn find_rules(&self, _: FlagId) -> Result<Vec<FlagRule>, RepositoryError> {
                Ok(vec![])
            }
            async fn upsert_rules(&self, _: FlagId, _: &[FlagRule]) -> Result<(), RepositoryError> {
                Ok(())
            }
        }
        #[async_trait]
        impl VariantRepository for NullRepo {
            async fn find_by_flag(&self, _: FlagId) -> Result<Vec<Variant>, RepositoryError> {
                Ok(vec![])
            }
            async fn create(&self, _: FlagId, _: &Variant) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn update(&self, v: &Variant) -> Result<Variant, RepositoryError> {
                Ok(v.clone())
            }
            async fn delete(&self, _: VariantId) -> Result<(), RepositoryError> {
                Ok(())
            }
        }
        #[async_trait]
        impl SegmentRepository for NullRepo {
            async fn find_by_id(&self, id: SegmentId) -> Result<Segment, RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_by_key(
                &self,
                key: &str,
                _: EnvironmentId,
            ) -> Result<Segment, RepositoryError> {
                Err(RepositoryError::NotFound {
                    id: key.to_string(),
                })
            }
            async fn list_by_environment(
                &self,
                _: EnvironmentId,
            ) -> Result<Vec<Segment>, RepositoryError> {
                Ok(vec![])
            }
            async fn create(&self, _: &Segment) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn update(&self, s: &Segment) -> Result<Segment, RepositoryError> {
                Ok(s.clone())
            }
            async fn find_with_rules(
                &self,
                id: SegmentId,
            ) -> Result<RuleBasedSegment, RepositoryError> {
                Ok(RuleBasedSegment { id, rules: vec![] })
            }
            async fn find_with_list(
                &self,
                id: SegmentId,
            ) -> Result<ListBasedSegment, RepositoryError> {
                Ok(ListBasedSegment {
                    id,
                    lists: HashMap::new(),
                })
            }
            async fn upsert_rules(
                &self,
                _: SegmentId,
                _: &[stitchd_core::rule_engine::types::Rule],
            ) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn set_list_entries(
                &self,
                _: SegmentId,
                _: &str,
                _: &[String],
                _: &[String],
            ) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn soft_delete(&self, _: SegmentId) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn check_list_membership(
                &self,
                _: EnvironmentId,
                _: &str,
                _: &str,
                keys: &[String],
            ) -> Result<HashMap<String, bool>, RepositoryError> {
                Ok(keys.iter().map(|k| (k.clone(), false)).collect())
            }
            async fn batch_check_list_membership(
                &self,
                _: EnvironmentId,
                _: &[(String, String)],
                _: &[String],
            ) -> Result<Vec<stitchd_db::ContextMembership>, RepositoryError> {
                Ok(vec![])
            }
        }
        #[async_trait]
        impl SdkKeyRepository for NullRepo {
            async fn find_by_id(&self, id: SdkKeyId) -> Result<SdkKey, RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn list_by_environment(
                &self,
                _: EnvironmentId,
            ) -> Result<Vec<SdkKey>, RepositoryError> {
                Ok(vec![])
            }
            async fn create(&self, _: &SdkKey) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn revoke(&self, id: SdkKeyId) -> Result<(), RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_active_by_environment(
                &self,
                _: EnvironmentId,
            ) -> Result<Vec<SdkKey>, RepositoryError> {
                Ok(vec![])
            }
            async fn find_active_by_hash(&self, h: &str) -> Result<SdkKey, RepositoryError> {
                Err(RepositoryError::NotFound { id: h.to_string() })
            }
        }
        #[async_trait]
        impl EventDefinitionRepository for NullRepo {
            async fn find_by_id(
                &self,
                id: EventDefinitionId,
            ) -> Result<EventDefinition, RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_by_key(
                &self,
                key: &str,
                _: EnvironmentId,
            ) -> Result<EventDefinition, RepositoryError> {
                Err(RepositoryError::NotFound {
                    id: key.to_string(),
                })
            }
            async fn list_by_environment(
                &self,
                _: EnvironmentId,
            ) -> Result<Vec<EventDefinition>, RepositoryError> {
                Ok(vec![])
            }
            async fn create(&self, _: &EventDefinition) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn update(
                &self,
                d: &EventDefinition,
            ) -> Result<EventDefinition, RepositoryError> {
                Ok(d.clone())
            }
            async fn soft_delete(&self, id: EventDefinitionId) -> Result<(), RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
        }

        #[async_trait]
        impl UserRepository for NullRepo {
            async fn find_by_id(&self, id: UserId) -> Result<User, RepositoryError> {
                Err(RepositoryError::NotFound { id: id.to_string() })
            }
            async fn find_by_email(&self, email: &str) -> Result<User, RepositoryError> {
                Err(RepositoryError::NotFound {
                    id: email.to_string(),
                })
            }
            async fn list_by_organisation(
                &self,
                _: OrganisationId,
            ) -> Result<Vec<User>, RepositoryError> {
                Ok(vec![])
            }
            async fn create(&self, _: &User) -> Result<(), RepositoryError> {
                Ok(())
            }
            async fn update(&self, u: &User) -> Result<User, RepositoryError> {
                Ok(u.clone())
            }
            async fn find_permissions_for_user(
                &self,
                _: UserId,
                _: stitchd_core::id::ProjectId,
            ) -> Result<Vec<Permission>, RepositoryError> {
                Ok(vec![])
            }
        }

        #[async_trait]
        impl stitchd_db::AuthUserRepository for NullRepo {
            async fn create(&self, e: &str, _: &str, _: Option<&str>) -> Result<stitchd_core::auth::User, RepositoryError> { Err(RepositoryError::NotFound { id: e.to_string() }) }
            async fn find_by_email(&self, _: &str) -> Result<Option<stitchd_core::auth::User>, RepositoryError> { Ok(None) }
            async fn find_by_id(&self, id: UserId) -> Result<Option<stitchd_core::auth::User>, RepositoryError> { let _ = id; Ok(None) }
            async fn rotate_token_secret(&self, id: UserId) -> Result<uuid::Uuid, RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_status(&self, id: UserId, _: stitchd_core::auth::UserStatus) -> Result<(), RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_password_hash(&self, id: UserId, _: &str) -> Result<(), RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_profile(&self, id: UserId, _: &str, _: Option<&str>) -> Result<(), RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
        }

        #[async_trait]
        impl stitchd_db::OrgMembershipRepository for NullRepo {
            async fn add_member(&self, id: UserId, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::OrgRole) -> Result<stitchd_core::auth::OrgMembership, RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_membership(&self, _: UserId, _: stitchd_core::id::OrganisationId) -> Result<Option<stitchd_core::auth::OrgMembership>, RepositoryError> { Ok(None) }
            async fn list_orgs_for_user(&self, _: UserId) -> Result<Vec<stitchd_core::auth::OrgMembership>, RepositoryError> { Ok(vec![]) }
            async fn remove_member(&self, id: UserId, _: stitchd_core::id::OrganisationId) -> Result<(), RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_role(&self, id: UserId, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::OrgRole) -> Result<(), RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
        }

        #[async_trait]
        impl stitchd_db::RefreshTokenRepository for NullRepo {
            async fn create(&self, id: UserId, _: stitchd_core::id::OrganisationId, _: Option<&str>, _: i64) -> Result<(stitchd_core::auth::RefreshToken, String), RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_by_hash(&self, _: &str) -> Result<Option<stitchd_core::auth::RefreshToken>, RepositoryError> { Ok(None) }
            async fn consume(&self, id: stitchd_core::id::RefreshTokenId) -> Result<Option<stitchd_core::auth::RefreshToken>, RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
            async fn revoke(&self, id: stitchd_core::id::RefreshTokenId) -> Result<(), RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
            async fn revoke_all_for_user(&self, id: UserId) -> Result<(), RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
            async fn list_active(&self, _: UserId) -> Result<Vec<stitchd_core::auth::RefreshToken>, RepositoryError> { Ok(vec![]) }
        }
        #[async_trait]
        impl stitchd_db::AuthProviderRepository for NullRepo {
            async fn create(&self, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::ProviderType, _: &str, _: serde_json::Value, _: bool) -> Result<stitchd_core::auth::AuthProvider, RepositoryError> { Err(RepositoryError::Unexpected(anyhow::anyhow!("stub"))) }
            async fn find_by_id(&self, _: stitchd_core::id::AuthProviderId) -> Result<Option<stitchd_core::auth::AuthProvider>, RepositoryError> { Ok(None) }
            async fn list_for_org(&self, _: stitchd_core::id::OrganisationId) -> Result<Vec<stitchd_core::auth::AuthProvider>, RepositoryError> { Ok(vec![]) }
            async fn update(&self, id: stitchd_core::id::AuthProviderId, _: &str, _: serde_json::Value, _: bool) -> Result<stitchd_core::auth::AuthProvider, RepositoryError> { Err(RepositoryError::NotFound { id: id.to_string() }) }
            async fn delete(&self, _: stitchd_core::id::AuthProviderId) -> Result<(), RepositoryError> { Ok(()) }
        }

        struct NullMfaRepo;
        #[async_trait::async_trait]
        impl stitchd_db::MfaRepository for NullMfaRepo {
            async fn create_challenge(&self, _: UserId, _: i64) -> Result<(stitchd_core::id::MfaChallengeId, String), RepositoryError> { Err(RepositoryError::NotFound { id: "stub".to_string() }) }
            async fn consume_challenge(&self, _: &str) -> Result<Option<stitchd_core::id::MfaChallengeId>, RepositoryError> { Ok(None) }
            async fn enable_totp(&self, _: UserId, _: Vec<u8>, _: Vec<String>) -> Result<(), RepositoryError> { Ok(()) }
            async fn disable_totp(&self, _: UserId) -> Result<(), RepositoryError> { Ok(()) }
            async fn get_totp_secret(&self, _: UserId) -> Result<Option<Vec<u8>>, RepositoryError> { Ok(None) }
            async fn consume_recovery_code(&self, _: UserId, _: &str) -> Result<bool, RepositoryError> { Ok(false) }
            async fn store_pending_totp_secret(&self, _: UserId, _: Vec<u8>) -> Result<(), RepositoryError> { Ok(()) }
            async fn get_user_id_for_challenge(&self, _: &str) -> Result<Option<UserId>, RepositoryError> { Ok(None) }
        }

        let null = Arc::new(NullRepo);
        let state = crate::AppState {
            db: PgPool::connect_lazy("postgres://stitchd:stitchd@localhost:5432/stitchd_test")
                .expect("lazy pool"),
            metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
            segment_repo: null.clone(),
            flag_repo: null.clone(),
            variant_repo: null.clone(),
            sdk_key_repo: null.clone(),
            event_definition_repo: null.clone(),
            user_repo: null.clone(),
            experiment_repo,
            results_repo,
            ch_client: None,
            event_writer: None,
            auth_user_repo: null.clone(),
            membership_repo: null.clone(),
            refresh_token_repo: null.clone(),
            mfa_repo: Arc::new(NullMfaRepo),
            auth_provider_repo: null,
            oidc_state_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        };

        Router::new()
            .route(
                "/v1/environments/{env_id}/experiments/{id}/results",
                get(get_latest_results),
            )
            .route(
                "/v1/environments/{env_id}/experiments/{id}/iterations/{iter_id}/results",
                get(get_iteration_results),
            )
            .route(
                "/v1/environments/{env_id}/experiments/{id}/results/recompute",
                post(recompute_results),
            )
            .with_state(state)
    }

    // ── GET latest results: 404 when no results exist ──────────────────────

    #[tokio::test]
    async fn get_latest_results_returns_404_when_empty() {
        let env_id = EnvironmentId::new();
        let exp_id = ExperimentId::new();
        let app = build_results_router(
            Arc::new(EmptyResultsRepo),
            Arc::new(StubExperimentRepoEmpty),
        );

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/environments/{env_id}/experiments/{exp_id}/results"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), HttpStatusCode::NOT_FOUND);
    }

    // ── GET latest results: 200 with data ─────────────────────────────────

    #[tokio::test]
    async fn get_latest_results_returns_200_with_rows() {
        let env_id = EnvironmentId::new();
        let exp_id = ExperimentId::new();
        let iter_id = Uuid::new_v4();
        let repo = Arc::new(OneRowResultsRepo {
            experiment_id: exp_id.as_uuid(),
            iteration_id: iter_id,
        });
        let app = build_results_router(repo, Arc::new(StubExperimentRepoEmpty));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/environments/{env_id}/experiments/{exp_id}/results"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), HttpStatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["metrics"].is_array());
        assert_eq!(json["metrics"].as_array().unwrap().len(), 1);
    }

    // ── GET iteration results: 404 when empty ─────────────────────────────

    #[tokio::test]
    async fn get_iteration_results_returns_404_when_empty() {
        let env_id = EnvironmentId::new();
        let exp_id = ExperimentId::new();
        let iter_id = ExperimentIterationId::new();
        let app = build_results_router(
            Arc::new(EmptyResultsRepo),
            Arc::new(StubExperimentRepoEmpty),
        );

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/environments/{env_id}/experiments/{exp_id}/iterations/{iter_id}/results"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), HttpStatusCode::NOT_FOUND);
    }

    // ── GET iteration results: 200 with data ──────────────────────────────

    #[tokio::test]
    async fn get_iteration_results_returns_200_with_rows() {
        let env_id = EnvironmentId::new();
        let exp_id = ExperimentId::new();
        let iter_id_uuid = Uuid::new_v4();
        let iter_id = ExperimentIterationId::from_uuid(iter_id_uuid);
        let repo = Arc::new(OneRowResultsRepo {
            experiment_id: exp_id.as_uuid(),
            iteration_id: iter_id_uuid,
        });
        let app = build_results_router(repo, Arc::new(StubExperimentRepoEmpty));

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/environments/{env_id}/experiments/{exp_id}/iterations/{iter_id}/results"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), HttpStatusCode::OK);
    }

    // ── POST recompute: 404 when no iterations exist ───────────────────────

    #[tokio::test]
    async fn recompute_returns_404_when_no_iterations() {
        let env_id = EnvironmentId::new();
        let exp_id = ExperimentId::new();
        let app = build_results_router(
            Arc::new(EmptyResultsRepo),
            Arc::new(StubExperimentRepoEmpty),
        );

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/environments/{env_id}/experiments/{exp_id}/results/recompute"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), HttpStatusCode::NOT_FOUND);
    }

    // ── POST recompute: 202 when iteration exists ──────────────────────────

    #[tokio::test]
    async fn recompute_returns_202_when_iteration_exists() {
        let env_id = EnvironmentId::new();
        let exp_id = ExperimentId::new();
        let iter_id = Uuid::new_v4();
        let repo_with_iter = Arc::new(StubExperimentRepoWithIter {
            iteration_id: iter_id,
        });
        let app = build_results_router(Arc::new(EmptyResultsRepo), repo_with_iter);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "/v1/environments/{env_id}/experiments/{exp_id}/results/recompute"
                    ))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), HttpStatusCode::ACCEPTED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "accepted");
    }
}
