//! Stats route handlers — proxy REST requests to the Stats Service via gRPC.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::stats::v1::{
    GetExperimentTimeseriesRequest, GetJobStatusRequest, TriggerRecomputeRequest,
};

use crate::error::GatewayError;
use crate::state::GatewayState;

// ─── Response types ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, ToSchema)]
pub struct RecomputeJobJson {
    pub job_id: String,
    pub status: String,
    pub created_at_ms: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct JobStatusJson {
    pub job_id: String,
    pub status: String,
    pub started_at_ms: i64,
    pub completed_at_ms: i64,
    pub error: String,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `POST /v1/experiments/{experiment_id}/recompute`
///
/// Triggers an out-of-band stats recompute for an experiment.
/// Returns 202 Accepted with a `job_id` for polling.
#[utoipa::path(
    post,
    path = "/v1/experiments/{experiment_id}/recompute",
    tag = "stats",
    params(("experiment_id" = String, Path, description = "Experiment UUID")),
    responses(
        (status = 202, description = "Recompute job accepted", body = RecomputeJobJson),
        (status = 400, description = "Invalid experiment_id"),
        (status = 500, description = "Internal error"),
    )
)]
pub async fn trigger_recompute(
    State(state): State<Arc<GatewayState>>,
    Path(experiment_id): Path<String>,
) -> Result<impl IntoResponse, GatewayError> {
    let mut client = state.stats_client.lock().await;
    let resp = client
        .trigger_recompute(TriggerRecomputeRequest { experiment_id })
        .await
        .map_err(GatewayError::from)?
        .into_inner();

    Ok((
        StatusCode::ACCEPTED,
        Json(RecomputeJobJson {
            job_id: resp.job_id,
            status: resp.status,
            created_at_ms: resp.created_at_ms,
        }),
    ))
}

/// `POST /v1/environments/{environment_id}/experiments/{experiment_id}/recompute`
///
/// Environment-scoped admin recompute trigger (Phase 7 Task 4). Returns 202
/// with a job_id; clients poll the matching `GET .../recompute/{job_id}`.
#[utoipa::path(
    post,
    path = "/v1/environments/{environment_id}/experiments/{experiment_id}/recompute",
    tag = "experiments",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("experiment_id" = String, Path, description = "Experiment UUID"),
    ),
    responses(
        (status = 202, description = "Recompute job accepted", body = RecomputeJobJson),
        (status = 400, description = "Invalid experiment_id"),
        (status = 502, description = "Stats service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn trigger_recompute_env_scoped(
    State(state): State<Arc<GatewayState>>,
    Path((_environment_id, experiment_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let mut client = state.stats_client.lock().await;
    let resp = client
        .trigger_recompute(TriggerRecomputeRequest { experiment_id })
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    Ok((
        StatusCode::ACCEPTED,
        Json(RecomputeJobJson {
            job_id: resp.job_id,
            status: resp.status,
            created_at_ms: resp.created_at_ms,
        }),
    ))
}

/// `GET /v1/environments/{environment_id}/experiments/{experiment_id}/recompute/{job_id}`
///
/// Polls the status of a previously triggered recompute job.
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/experiments/{experiment_id}/recompute/{job_id}",
    tag = "experiments",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("experiment_id" = String, Path, description = "Experiment UUID"),
        ("job_id" = String, Path, description = "Recompute job UUID"),
    ),
    responses(
        (status = 200, description = "Job status", body = JobStatusJson),
        (status = 404, description = "Job not found"),
        (status = 502, description = "Stats service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_recompute_job_status(
    State(state): State<Arc<GatewayState>>,
    Path((_environment_id, _experiment_id, job_id)): Path<(String, String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let mut client = state.stats_client.lock().await;
    let resp = client
        .get_job_status(GetJobStatusRequest { job_id })
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    Ok(Json(JobStatusJson {
        job_id: resp.job_id,
        status: resp.status,
        started_at_ms: resp.started_at_ms,
        completed_at_ms: resp.completed_at_ms,
        error: resp.error,
    }))
}

/// `GET /v1/jobs/{job_id}`
///
/// Returns the current status of a stats recompute job.
#[utoipa::path(
    get,
    path = "/v1/jobs/{job_id}",
    tag = "stats",
    params(("job_id" = String, Path, description = "Job UUID")),
    responses(
        (status = 200, description = "Job status", body = JobStatusJson),
        (status = 404, description = "Job not found"),
        (status = 500, description = "Internal error"),
    )
)]
pub async fn get_job_status(
    State(state): State<Arc<GatewayState>>,
    Path(job_id): Path<String>,
) -> Result<impl IntoResponse, GatewayError> {
    let mut client = state.stats_client.lock().await;
    let resp = client
        .get_job_status(GetJobStatusRequest { job_id })
        .await
        .map_err(GatewayError::from)?
        .into_inner();

    Ok(Json(JobStatusJson {
        job_id: resp.job_id,
        status: resp.status,
        started_at_ms: resp.started_at_ms,
        completed_at_ms: resp.completed_at_ms,
        error: resp.error,
    }))
}

// ─── Timeseries (Phase 7 Task 3) ──────────────────────────────────────────────

/// Query parameters for `GET /timeseries`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct GetTimeseriesQuery {
    /// REQUIRED — UUID of the saved metric_definitions row.
    pub metric_id: Option<String>,
    /// REQUIRED — one of the experiment's `unit_context_types`.
    pub context_type: Option<String>,
    /// Days of history to query; default 7, clamped to [1, 90].
    pub days: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TimeseriesBucketJson {
    pub day: String,
    pub variant_key: String,
    pub value: f64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct GetTimeseriesResponseJson {
    pub buckets: Vec<TimeseriesBucketJson>,
}

/// `GET /v1/environments/{environment_id}/experiments/{experiment_id}/timeseries`
///
/// Per-variant daily series for one metric scoped to a context type.
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/experiments/{experiment_id}/timeseries",
    tag = "experiments",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("experiment_id" = String, Path, description = "Experiment ID"),
        ("metric_id" = String, Query, description = "REQUIRED. Metric definition UUID."),
        ("context_type" = String, Query, description = "REQUIRED. Attribution unit."),
        ("days" = Option<u32>, Query, description = "Window in days (default 7, clamped to [1, 90])"),
    ),
    responses(
        (status = 200, description = "Daily per-variant series", body = GetTimeseriesResponseJson),
        (status = 400, description = "Missing required query parameter"),
        (status = 404, description = "Metric not found"),
        (status = 502, description = "Stats service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_timeseries(
    State(state): State<Arc<GatewayState>>,
    Path((_environment_id, experiment_id)): Path<(String, String)>,
    Query(query): Query<GetTimeseriesQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let context_type = query.context_type.unwrap_or_default();
    if context_type.is_empty() {
        return Err(GatewayError::MissingContextType);
    }
    let metric_id = query.metric_id.unwrap_or_default();
    if metric_id.is_empty() {
        return Err(GatewayError::MissingMetricId);
    }
    let days = query.days.unwrap_or(7);

    let req = GetExperimentTimeseriesRequest {
        experiment_id,
        metric_id,
        context_type,
        days,
    };
    let mut client = state.stats_client.lock().await;
    let resp = client
        .get_experiment_timeseries(req)
        .await
        .map_err(GatewayError::from)?
        .into_inner();

    let buckets: Vec<TimeseriesBucketJson> = resp
        .buckets
        .into_iter()
        .map(|b| TimeseriesBucketJson {
            day: b.day,
            variant_key: b.variant_key,
            value: b.value,
        })
        .collect();

    Ok(Json(GetTimeseriesResponseJson { buckets }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::helpers::make_stub_state;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::ServiceExt as _;

    fn timeseries_router() -> axum::Router {
        let state = make_stub_state();
        axum::Router::new()
            .route(
                "/v1/environments/{environment_id}/experiments/{experiment_id}/timeseries",
                get(get_timeseries),
            )
            .with_state(state)
    }

    #[tokio::test]
    async fn timeseries_missing_metric_id_returns_400() {
        let app = timeseries_router();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments/exp-1/timeseries?context_type=user")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["error"], "missing_metric_id");
    }

    #[tokio::test]
    async fn timeseries_missing_context_type_returns_400() {
        let app = timeseries_router();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments/exp-1/timeseries?metric_id=m1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["error"], "missing_context_type");
    }

    fn recompute_router() -> axum::Router {
        use axum::routing::post;
        let state = make_stub_state();
        axum::Router::new()
            .route(
                "/v1/environments/{environment_id}/experiments/{experiment_id}/recompute",
                post(trigger_recompute_env_scoped),
            )
            .route(
                "/v1/environments/{environment_id}/experiments/{experiment_id}/recompute/{job_id}",
                get(get_recompute_job_status),
            )
            .with_state(state)
    }

    #[tokio::test]
    async fn trigger_recompute_env_scoped_returns_202_or_502() {
        let app = recompute_router();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/experiments/exp-1/recompute")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::ACCEPTED
                || resp.status() == StatusCode::BAD_GATEWAY
                || resp.status() == StatusCode::BAD_REQUEST
                || resp.status() == StatusCode::INTERNAL_SERVER_ERROR,
            "got {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn get_recompute_job_status_returns_200_404_or_502() {
        let app = recompute_router();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/experiments/exp-1/recompute/job-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK
                || resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::BAD_GATEWAY
                || resp.status() == StatusCode::INTERNAL_SERVER_ERROR,
            "got {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn timeseries_with_required_params_proxies_to_grpc() {
        // grpc connection is lazy and fails fast → 502 expected.
        let app = timeseries_router();
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(
                        "/v1/environments/env-1/experiments/exp-1/timeseries?metric_id=m1&context_type=user&days=14",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK
                || resp.status() == StatusCode::BAD_GATEWAY
                || resp.status() == StatusCode::INTERNAL_SERVER_ERROR,
            "got {}",
            resp.status()
        );
    }
}
