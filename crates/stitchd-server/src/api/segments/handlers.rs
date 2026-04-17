use crate::AppState;
use crate::api::sdk_auth::SdkAuth;
use crate::api::segments::types::{
    BatchContextMembership, BatchListCheckRequest, BatchListCheckResponse, CreateSegmentRequest,
    ListCheckRequest, ListCheckResponse, SegmentResponse, UpdateSegmentRequest, ValidationError,
    validate_rules,
};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use chrono::Utc;
use stitchd_core::{
    id::{EnvironmentId, SegmentId},
    segment::{Segment, SegmentDefinition, SegmentType},
};
use stitchd_db::RepositoryError;

/// API error type for mapping internal errors to HTTP responses.
pub enum ApiError {
    /// Resource not found.
    NotFound(String),
    /// Optimistic concurrency or unique constraint conflict.
    Conflict(String),
    /// Invalid input or business rule violation.
    BadRequest(String),
    /// Internal database error.
    Database(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg) = match self {
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            Self::Conflict(msg) => (StatusCode::CONFLICT, msg),
            Self::BadRequest(msg) => (StatusCode::UNPROCESSABLE_ENTITY, msg),
            Self::Database(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
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

impl From<ValidationError> for ApiError {
    fn from(e: ValidationError) -> Self {
        Self::BadRequest(e.to_string())
    }
}

/// `GET /v1/environments/{env_id}/segments` — List active segments.
///
/// # Errors
/// Returns [`ApiError::Database`] if the repository fails to list segments.
pub async fn list_segments(
    Path(env_id): Path<EnvironmentId>,
    State(state): State<AppState>,
) -> Result<Json<Vec<Segment>>, ApiError> {
    let segments = state.segment_repo.list_by_environment(env_id).await?;
    Ok(Json(segments))
}

/// `POST /v1/environments/{env_id}/segments` — Create segment.
///
/// # Errors
/// Returns [`ApiError::BadRequest`] if validation fails or [`ApiError::Conflict`] if the key exists.
pub async fn create_segment(
    Path(env_id): Path<EnvironmentId>,
    State(state): State<AppState>,
    Json(req): Json<CreateSegmentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let segment = Segment {
        id: SegmentId::new(),
        environment_id: env_id,
        key: req.key,
        segment_type: req.segment_type,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        deleted_at: None,
        version: 1,
    };

    state.segment_repo.create(&segment).await?;

    match segment.segment_type {
        SegmentType::Rule => {
            let rules = req
                .rules
                .ok_or(ValidationError::MissingDefinition(SegmentType::Rule))?;
            validate_rules(&rules)?;
            state.segment_repo.upsert_rules(segment.id, &rules).await?;
        }
        SegmentType::List => {
            let lists = req
                .lists
                .ok_or(ValidationError::MissingDefinition(SegmentType::List))?;
            for (context_type, list) in lists {
                state
                    .segment_repo
                    .set_list_entries(
                        segment.id,
                        &context_type,
                        &list.include.into_iter().collect::<Vec<_>>(),
                        &list.exclude.into_iter().collect::<Vec<_>>(),
                    )
                    .await?;
            }
        }
    }

    Ok((StatusCode::CREATED, Json(segment)))
}

/// `GET /v1/environments/{env_id}/segments/{seg_id}` — Get segment + definition.
///
/// # Errors
/// Returns [`ApiError::NotFound`] if the segment does not exist.
pub async fn get_segment(
    Path((_env_id, seg_id)): Path<(EnvironmentId, SegmentId)>,
    State(state): State<AppState>,
) -> Result<Json<SegmentResponse>, ApiError> {
    let segment = state.segment_repo.find_by_id(seg_id).await?;

    let definition = match segment.segment_type {
        SegmentType::Rule => {
            SegmentDefinition::RuleBased(state.segment_repo.find_with_rules(seg_id).await?)
        }
        SegmentType::List => {
            SegmentDefinition::ListBased(state.segment_repo.find_with_list(seg_id).await?)
        }
    };

    Ok(Json(SegmentResponse {
        id: segment.id,
        key: segment.key,
        segment_type: segment.segment_type,
        definition,
        version: segment.version,
    }))
}

/// `PUT /v1/environments/{env_id}/segments/{seg_id}` — Replace definition.
///
/// # Errors
/// Returns [`ApiError::Conflict`] if the version mismatch or [`ApiError::NotFound`] if missing.
pub async fn update_segment(
    Path((_env_id, seg_id)): Path<(EnvironmentId, SegmentId)>,
    State(state): State<AppState>,
    Json(req): Json<UpdateSegmentRequest>,
) -> Result<impl IntoResponse, ApiError> {
    let mut segment = state.segment_repo.find_by_id(seg_id).await?;

    if segment.version != req.version {
        return Err(ApiError::Conflict(format!(
            "version conflict: expected {}, actual {}",
            req.version, segment.version
        )));
    }

    match segment.segment_type {
        SegmentType::Rule => {
            let rules = req
                .rules
                .ok_or(ValidationError::MissingDefinition(SegmentType::Rule))?;
            validate_rules(&rules)?;
            state.segment_repo.upsert_rules(seg_id, &rules).await?;
        }
        SegmentType::List => {
            let lists = req
                .lists
                .ok_or(ValidationError::MissingDefinition(SegmentType::List))?;
            for (context_type, list) in lists {
                state
                    .segment_repo
                    .set_list_entries(
                        seg_id,
                        &context_type,
                        &list.include.into_iter().collect::<Vec<_>>(),
                        &list.exclude.into_iter().collect::<Vec<_>>(),
                    )
                    .await?;
            }
        }
    }

    // reload segment to get updated version/timestamp
    segment = state.segment_repo.find_by_id(seg_id).await?;

    Ok(Json(segment))
}

/// `DELETE /v1/environments/{env_id}/segments/{seg_id}` — Soft-delete.
///
/// # Errors
/// Returns [`ApiError::NotFound`] if the segment does not exist.
pub async fn delete_segment(
    Path((_env_id, seg_id)): Path<(EnvironmentId, SegmentId)>,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ApiError> {
    state.segment_repo.soft_delete(seg_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/environments/{env_id}/segments/list-check` — Check list-segment membership.
///
/// Requires a valid `x-sdk-key` header for the target environment.
///
/// # Errors
/// Returns [`ApiError::BadRequest`] for empty segment keys, [`ApiError::Database`] on failure.
pub async fn list_check_membership(
    _auth: SdkAuth,
    Path(env_id): Path<EnvironmentId>,
    State(state): State<AppState>,
    Json(req): Json<ListCheckRequest>,
) -> Result<Json<ListCheckResponse>, ApiError> {
    if req.segment_keys.is_empty() {
        return Ok(Json(ListCheckResponse {
            memberships: std::collections::HashMap::new(),
        }));
    }

    let memberships = state
        .segment_repo
        .check_list_membership(env_id, &req.context_type, &req.context_key, &req.segment_keys)
        .await?;

    Ok(Json(ListCheckResponse { memberships }))
}

/// `POST /v1/environments/{env_id}/segments/list-check/batch` — Batch list-segment membership.
///
/// Requires a valid `x-sdk-key` header for the target environment.
///
/// # Errors
/// Returns [`ApiError::Database`] on failure.
pub async fn batch_list_check_membership(
    _auth: SdkAuth,
    Path(env_id): Path<EnvironmentId>,
    State(state): State<AppState>,
    Json(req): Json<BatchListCheckRequest>,
) -> Result<Json<BatchListCheckResponse>, ApiError> {
    if req.segment_keys.is_empty() || req.contexts.is_empty() {
        return Ok(Json(BatchListCheckResponse { results: Vec::new() }));
    }

    let ctx_pairs: Vec<(String, String)> = req
        .contexts
        .iter()
        .map(|c| (c.context_type.clone(), c.context_key.clone()))
        .collect();

    let raw = state
        .segment_repo
        .batch_check_list_membership(env_id, &ctx_pairs, &req.segment_keys)
        .await?;

    let results = raw
        .into_iter()
        .map(|cm| BatchContextMembership {
            context_type: cm.context_type,
            context_key: cm.context_key,
            memberships: cm.memberships,
        })
        .collect();

    Ok(Json(BatchListCheckResponse { results }))
}
