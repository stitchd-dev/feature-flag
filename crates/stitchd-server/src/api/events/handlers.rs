//! REST handlers for event ingestion.
//!
//! Endpoints:
//! - `POST /v1/environments/{env_id}/events`        — single event, 202 on success
//! - `POST /v1/environments/{env_id}/events/batch`  — up to 500 events, 202 on success

use axum::{
    Json,
    http::StatusCode,
    response::IntoResponse,
};
use serde_json::json;
use stitchd_core::event::{BatchEventPayload, EventPayload};

use crate::{AppState, api::sdk_auth::SdkAuth};

use super::ingestion::{IngestionError, ingest_batch, ingest_event};

/// Maximum number of events allowed in a single batch request.
const BATCH_MAX: usize = 500;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// API error type for event ingestion handlers.
pub enum ApiError {
    /// Submitted event key is not registered for this environment.
    UnknownKey(String),
    /// Submitted value type does not match the registered definition.
    TypeMismatch(String),
    /// Batch size exceeds the allowed maximum.
    BatchTooLarge,
    /// Internal error.
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg) = match self {
            Self::UnknownKey(m) => (StatusCode::UNPROCESSABLE_ENTITY, m),
            Self::TypeMismatch(m) => (StatusCode::UNPROCESSABLE_ENTITY, m),
            Self::BatchTooLarge => (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("batch exceeds maximum of {BATCH_MAX} events"),
            ),
            Self::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

impl From<IngestionError> for ApiError {
    fn from(e: IngestionError) -> Self {
        match e {
            IngestionError::UnknownKey { key } => Self::UnknownKey(format!("unknown key: {key}")),
            IngestionError::TypeMismatch { key, expected, got } => Self::TypeMismatch(format!(
                "type mismatch for '{key}': expected {expected}, got {got}"
            )),
            IngestionError::Repository(r) => Self::Internal(r.to_string()),
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /v1/environments/{env_id}/events` — Ingest a single event.
///
/// Validates the `metric_key` against registered definitions and checks the
/// value type. Returns 202 immediately; the ClickHouse write is async.
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/events",
    params(("env_id" = String, Path, description = "Environment UUID")),
    request_body = EventPayload,
    responses(
        (status = 202, description = "Event accepted for ingestion"),
        (status = 401, description = "Missing or invalid SDK key"),
        (status = 422, description = "Unknown key or type mismatch"),
        (status = 500, description = "Internal error"),
    ),
    tag = "events",
    security(("sdk_key" = []))
)]
pub async fn ingest_single_event(
    auth: SdkAuth,
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(payload): Json<EventPayload>,
) -> Result<StatusCode, ApiError> {
    ingest_event(
        &state.event_definition_repo,
        state.event_writer.as_ref(),
        auth.environment_id,
        payload,
    )
    .await
    .map_err(ApiError::from)?;

    Ok(StatusCode::ACCEPTED)
}

/// `POST /v1/environments/{env_id}/events/batch` — Ingest up to 500 events.
///
/// All events are validated before any write is initiated. A single invalid
/// event causes the entire batch to be rejected with 422.
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/events/batch",
    params(("env_id" = String, Path, description = "Environment UUID")),
    request_body = BatchEventPayload,
    responses(
        (status = 202, description = "Batch accepted for ingestion"),
        (status = 401, description = "Missing or invalid SDK key"),
        (status = 422, description = "Batch too large, unknown key, or type mismatch"),
        (status = 500, description = "Internal error"),
    ),
    tag = "events",
    security(("sdk_key" = []))
)]
pub async fn ingest_batch_events(
    auth: SdkAuth,
    axum::extract::State(state): axum::extract::State<AppState>,
    Json(batch): Json<BatchEventPayload>,
) -> Result<StatusCode, ApiError> {
    if batch.events.len() > BATCH_MAX {
        return Err(ApiError::BatchTooLarge);
    }

    ingest_batch(
        &state.event_definition_repo,
        state.event_writer.as_ref(),
        auth.environment_id,
        batch,
    )
    .await
    .map_err(ApiError::from)?;

    Ok(StatusCode::ACCEPTED)
}
