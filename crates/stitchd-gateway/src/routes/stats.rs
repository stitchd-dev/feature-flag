//! Stats route handlers — proxy REST requests to the Stats Service via gRPC.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::Serialize;
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::stats::v1::{GetJobStatusRequest, TriggerRecomputeRequest};

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
