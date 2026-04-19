//! REST handlers for event definition registration.
//!
//! Endpoints:
//! - `POST   /v1/environments/{env_id}/event-definitions`          — register
//! - `GET    /v1/environments/{env_id}/event-definitions`          — list active
//! - `DELETE /v1/environments/{env_id}/event-definitions/{key}`    — soft-delete

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use stitchd_core::{
    event::{EventDefinition, EventValueType},
    id::{EnvironmentId, EventDefinitionId},
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
    /// Optimistic concurrency or unique constraint conflict.
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
        }
    }
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// Request body for registering a new event definition.
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct CreateEventDefinitionRequest {
    /// URL-safe string key, unique within the environment.
    pub key: String,
    /// The metric value type this event will carry.
    pub value_type: EventValueType,
}

/// Response body for a single event definition.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct EventDefinitionResponse {
    /// Unique identifier.
    pub id: EventDefinitionId,
    /// The environment this definition belongs to.
    pub environment_id: EnvironmentId,
    /// URL-safe string key, unique within the environment.
    pub key: String,
    /// The metric value type this event carries.
    pub value_type: EventValueType,
    /// ISO-8601 creation timestamp.
    pub created_at: chrono::DateTime<Utc>,
    /// ISO-8601 last-updated timestamp.
    pub updated_at: chrono::DateTime<Utc>,
    /// Optimistic-concurrency version counter.
    pub version: i64,
}

impl From<EventDefinition> for EventDefinitionResponse {
    fn from(d: EventDefinition) -> Self {
        Self {
            id: d.id,
            environment_id: d.environment_id,
            key: d.key,
            value_type: d.value_type,
            created_at: d.created_at,
            updated_at: d.updated_at,
            version: d.version,
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /v1/environments/{env_id}/event-definitions` — Register an event definition.
///
/// # Errors
/// Returns 409 if a definition with the same key already exists in the environment.
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/event-definitions",
    params(("env_id" = String, Path, description = "Environment UUID")),
    request_body = CreateEventDefinitionRequest,
    responses(
        (status = 201, description = "Event definition registered", body = EventDefinitionResponse),
        (status = 409, description = "Key already exists in this environment"),
        (status = 500, description = "Internal error"),
    ),
    tag = "event-definitions"
)]
pub async fn create_event_definition(
    Path(env_id): Path<EnvironmentId>,
    State(state): State<AppState>,
    Json(body): Json<CreateEventDefinitionRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let now = Utc::now();
    let def = EventDefinition {
        id: EventDefinitionId::new(),
        environment_id: env_id,
        key: body.key,
        value_type: body.value_type,
        created_at: now,
        updated_at: now,
        deleted_at: None,
        version: 1,
    };

    state.event_definition_repo.create(&def).await?;

    Ok((
        StatusCode::CREATED,
        Json(EventDefinitionResponse::from(def)),
    ))
}

/// `GET /v1/environments/{env_id}/event-definitions` — List active event definitions.
///
/// # Errors
/// Returns 500 if the repository query fails.
#[utoipa::path(
    get,
    path = "/v1/environments/{env_id}/event-definitions",
    params(("env_id" = String, Path, description = "Environment UUID")),
    responses(
        (status = 200, description = "List of event definitions", body = Vec<EventDefinitionResponse>),
        (status = 500, description = "Internal error"),
    ),
    tag = "event-definitions"
)]
pub async fn list_event_definitions(
    Path(env_id): Path<EnvironmentId>,
    State(state): State<AppState>,
) -> Result<Json<Vec<EventDefinitionResponse>>, ApiError> {
    let defs = state
        .event_definition_repo
        .list_by_environment(env_id)
        .await?;
    Ok(Json(defs.into_iter().map(EventDefinitionResponse::from).collect()))
}

/// `DELETE /v1/environments/{env_id}/event-definitions/{key}` — Soft-delete by key.
///
/// # Errors
/// Returns 404 if no active definition with that key exists.
#[utoipa::path(
    delete,
    path = "/v1/environments/{env_id}/event-definitions/{key}",
    params(
        ("env_id" = String, Path, description = "Environment UUID"),
        ("key" = String, Path, description = "Event definition key"),
    ),
    responses(
        (status = 204, description = "Event definition deleted"),
        (status = 404, description = "Not found"),
        (status = 500, description = "Internal error"),
    ),
    tag = "event-definitions"
)]
pub async fn delete_event_definition(
    Path((env_id, key)): Path<(EnvironmentId, String)>,
    State(state): State<AppState>,
) -> Result<StatusCode, ApiError> {
    let def = state
        .event_definition_repo
        .find_by_key(&key, env_id)
        .await?;

    state
        .event_definition_repo
        .soft_delete(def.id)
        .await?;

    Ok(StatusCode::NO_CONTENT)
}
