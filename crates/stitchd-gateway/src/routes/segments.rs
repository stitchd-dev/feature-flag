//! Segment route handlers — admin CRUD for environment-scoped Segments.
//!
//! All routes require a valid JWT (injected by [`auth_middleware`]).
//! Write routes additionally require the `segment:write` permission;
//! read routes require `segment:read`.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::segments::v1::{
    CreateAdminSegmentRequest, DeleteAdminSegmentRequest, GetAdminSegmentRequest,
    ListAdminSegmentsRequest, LookupSegmentEntryRequest, PatchSegmentEntriesRequest,
    UpdateAdminSegmentRequest,
};

use crate::error::GatewayError;
use crate::pagination::{CursorPage, CursorParams};
use crate::state::GatewayState;

// ─── REST types ───────────────────────────────────────────────────────────────

/// Query parameters for `GET /v1/segments`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListSegmentsQuery {
    pub env_id: Option<String>,
    #[serde(flatten)]
    pub cursor: CursorParams,
}

/// Request body for creating a segment.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SegmentCreateRequest {
    pub name: String,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    /// ConditionExpr as a JSON value.
    #[schema(value_type = Object, nullable = true)]
    pub condition_expr: Option<serde_json::Value>,
    pub user_list: Option<Vec<String>>,
    pub excluded_keys: Option<Vec<String>>,
    /// "list" or "rule"
    pub segment_type: Option<String>,
    /// Context kind this list targets (e.g. "user", "org", "device"). Defaults to "user".
    pub context_type: Option<String>,
    /// Environment UUID — accepted from the body when the route has no path param.
    pub env_id: Option<String>,
}

/// Request body for updating a segment.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SegmentUpdateRequest {
    pub name: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    /// ConditionExpr as a JSON value.
    #[schema(value_type = Object, nullable = true)]
    pub condition_expr: Option<serde_json::Value>,
    pub user_list: Option<Vec<String>>,
    pub excluded_keys: Option<Vec<String>>,
    pub version: Option<u64>,
    /// Context kind this list targets (e.g. "user", "org", "device"). Defaults to "user".
    pub context_type: Option<String>,
}

/// Full admin representation of a segment.
#[derive(Debug, Serialize, ToSchema)]
pub struct AdminSegmentJson {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    #[schema(value_type = Object, nullable = true)]
    pub condition_expr: Option<serde_json::Value>,
    /// Deprecated: always empty. Use `include_count` instead.
    pub user_list: Vec<String>,
    /// Deprecated: always empty. Use `exclude_count` instead.
    pub excluded_keys: Vec<String>,
    /// Count of include-list entries (list-based segments only).
    pub include_count: u32,
    /// Count of exclude-list entries (list-based segments only).
    pub exclude_count: u32,
    /// Number of top-level condition nodes (convenience for the UI).
    pub condition_count: usize,
    pub version: u64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    /// "list" or "rule"
    pub segment_type: String,
    /// Context kind for list-based segments (e.g. "user", "org", "device").
    pub context_type: Option<String>,
}

// ─── Entry patch / lookup types ───────────────────────────────────────────────

/// Request body for `POST /v1/segments/{segment_id}/entries`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PatchEntriesRequest {
    pub keys: Vec<String>,
    /// "include" or "exclude"
    pub list_type: String,
    /// "add", "remove", or "replace"
    pub action: String,
}

/// Response from `POST /v1/segments/{segment_id}/entries`.
#[derive(Debug, Serialize, ToSchema)]
pub struct PatchEntriesResponse {
    pub include_count: u32,
    pub exclude_count: u32,
}

/// Query params for `GET /v1/segments/{segment_id}/entries/lookup`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct LookupEntryQuery {
    pub key: String,
}

/// Response from `GET /v1/segments/{segment_id}/entries/lookup`.
#[derive(Debug, Serialize, ToSchema)]
pub struct LookupEntryResponse {
    pub in_include: bool,
    pub in_exclude: bool,
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Count the number of top-level conditions in the given expr JSON.
/// For UI convenience — shows how many rules the segment has.
fn count_conditions(expr: &serde_json::Value) -> usize {
    if expr.is_null() {
        return 0;
    }
    // If the expr is an object with "And" / "Or" / "Not" at the top level, count children.
    if let Some(children) = expr.get("And").and_then(|v| v.as_array()) {
        return children.len();
    }
    if let Some(children) = expr.get("Or").and_then(|v| v.as_array()) {
        return children.len();
    }
    // Any other non-null value (Leaf / Not / …) counts as 1.
    1
}

/// Convert an `AdminSegment` proto into the gateway's `AdminSegmentJson`.
fn proto_to_admin_json(seg: &stitchd_proto::segments::v1::AdminSegment) -> AdminSegmentJson {
    let condition_expr: Option<serde_json::Value> = if seg.condition_expr.is_empty() {
        None
    } else {
        serde_json::from_slice(&seg.condition_expr).ok()
    };

    let condition_count = condition_expr.as_ref().map(count_conditions).unwrap_or(0);

    let created_at = if seg.created_at_ms != 0 {
        chrono::DateTime::from_timestamp_millis(seg.created_at_ms)
            .map(|dt: chrono::DateTime<chrono::Utc>| dt.to_rfc3339())
    } else {
        None
    };
    let updated_at = if seg.updated_at_ms != 0 {
        chrono::DateTime::from_timestamp_millis(seg.updated_at_ms)
            .map(|dt: chrono::DateTime<chrono::Utc>| dt.to_rfc3339())
    } else {
        None
    };

    AdminSegmentJson {
        id: seg.id.clone(),
        name: seg.name.clone(),
        description: if seg.description.is_empty() {
            None
        } else {
            Some(seg.description.clone())
        },
        tags: seg.tags.clone(),
        condition_expr,
        user_list: vec![],
        excluded_keys: vec![],
        include_count: seg.include_count,
        exclude_count: seg.exclude_count,
        condition_count,
        version: seg.version,
        created_at,
        updated_at,
        segment_type: if seg.segment_type.is_empty() {
            "rule".to_string()
        } else {
            seg.segment_type.clone()
        },
        context_type: if seg.context_type.is_empty() {
            None
        } else {
            Some(seg.context_type.clone())
        },
    }
}

use super::require_permission;

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /v1/segments?env_id=<uuid>` — list all segments for an environment.
#[utoipa::path(
    get,
    path = "/v1/segments",
    tag = "segments",
    params(
        ("env_id" = String, Query, description = "Environment ID (query param)"),
        ("cursor" = Option<String>, Query, description = "Opaque pagination cursor from a previous response"),
        ("limit" = Option<u32>, Query, description = "Page size (default 50, max 200)"),
    ),
    responses(
        (status = 200, description = "Cursor-paginated list of segments"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing segment:read"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn list_segments(
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<ListSegmentsQuery>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "segment:read")?;

    let environment_id = query.env_id.unwrap_or_default();
    let rpc = tonic::Request::new(ListAdminSegmentsRequest {
        environment_id,
        cursor: query.cursor.cursor.clone().unwrap_or_default(),
        limit: query.cursor.effective_limit(),
    });
    let mut client = state.segmentation_client.lock().await;
    let inner = client
        .list_admin_segments(rpc)
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    let items: Vec<AdminSegmentJson> = inner.segments.iter().map(proto_to_admin_json).collect();
    Ok(Json(CursorPage::from_token(items, inner.next_cursor)))
}

/// `POST /v1/segments` — create a new segment.
#[utoipa::path(
    post,
    path = "/v1/segments",
    tag = "segments",
    request_body = SegmentCreateRequest,
    responses(
        (status = 201, description = "Segment created", body = AdminSegmentJson),
        (status = 400, description = "Malformed condition_expr JSON"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing segment:write"),
        (status = 409, description = "Name already exists in this environment"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_segment(
    State(state): State<Arc<GatewayState>>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "segment:write")?;

    // Extract body from the request.
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 4 * 1024 * 1024)
        .await
        .map_err(|e| GatewayError::BadRequest(e.to_string()))?;
    let body: SegmentCreateRequest = serde_json::from_slice(&bytes)
        .map_err(|e| GatewayError::BadRequest(format!("invalid request body: {e}")))?;
    let _ = parts;

    // Validate and encode condition_expr.
    let condition_expr = encode_condition_expr(body.condition_expr)?;

    let rpc = tonic::Request::new(CreateAdminSegmentRequest {
        environment_id: body.env_id.unwrap_or_default(),
        name: body.name,
        description: body.description.unwrap_or_default(),
        tags: body.tags.unwrap_or_default(),
        condition_expr,
        user_list: body.user_list.unwrap_or_default(),
        excluded_keys: body.excluded_keys.unwrap_or_default(),
        segment_type: body.segment_type.unwrap_or_default(),
        context_type: body.context_type.unwrap_or_default(),
    });
    let mut client = state.segmentation_client.lock().await;
    let resp = client
        .create_admin_segment(rpc)
        .await
        .map_err(GatewayError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(proto_to_admin_json(&resp.into_inner())),
    ))
}

/// `GET /v1/environments/{environment_id}/segments` — list all segments for an environment.
pub async fn list_segments_in_env(
    State(state): State<Arc<GatewayState>>,
    Path(environment_id): Path<String>,
    Query(cursor): Query<CursorParams>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "segment:read")?;
    let rpc = tonic::Request::new(ListAdminSegmentsRequest {
        environment_id,
        cursor: cursor.cursor.clone().unwrap_or_default(),
        limit: cursor.effective_limit(),
    });
    let mut client = state.segmentation_client.lock().await;
    let inner = client
        .list_admin_segments(rpc)
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    let items: Vec<AdminSegmentJson> = inner.segments.iter().map(proto_to_admin_json).collect();
    Ok(Json(CursorPage::from_token(items, inner.next_cursor)))
}

/// `POST /v1/environments/{environment_id}/segments` — create a new segment scoped to an environment.
#[utoipa::path(
    post,
    path = "/v1/environments/{environment_id}/segments",
    tag = "segments",
    params(("environment_id" = String, Path, description = "Environment ID")),
    request_body = SegmentCreateRequest,
    responses(
        (status = 201, description = "Segment created", body = AdminSegmentJson),
        (status = 400, description = "Malformed condition_expr JSON"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing segment:write"),
        (status = 409, description = "Name already exists in this environment"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_segment_in_env(
    State(state): State<Arc<GatewayState>>,
    Path(environment_id): Path<String>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "segment:write")?;

    let (_parts, body_raw) = req.into_parts();
    let bytes = axum::body::to_bytes(body_raw, 4 * 1024 * 1024)
        .await
        .map_err(|e| GatewayError::BadRequest(e.to_string()))?;
    let body: SegmentCreateRequest = serde_json::from_slice(&bytes)
        .map_err(|e| GatewayError::BadRequest(format!("invalid request body: {e}")))?;

    let condition_expr = encode_condition_expr(body.condition_expr)?;

    let rpc = tonic::Request::new(CreateAdminSegmentRequest {
        environment_id,
        name: body.name,
        description: body.description.unwrap_or_default(),
        tags: body.tags.unwrap_or_default(),
        condition_expr,
        user_list: body.user_list.unwrap_or_default(),
        excluded_keys: body.excluded_keys.unwrap_or_default(),
        segment_type: body.segment_type.unwrap_or_default(),
        context_type: body.context_type.unwrap_or_default(),
    });
    let mut client = state.segmentation_client.lock().await;
    let resp = client
        .create_admin_segment(rpc)
        .await
        .map_err(GatewayError::from)?;
    Ok((
        StatusCode::CREATED,
        Json(proto_to_admin_json(&resp.into_inner())),
    ))
}

/// `GET /v1/segments/{segment_id}` — get one segment by ID.
#[utoipa::path(
    get,
    path = "/v1/segments/{segment_id}",
    tag = "segments",
    params(("segment_id" = String, Path, description = "Segment UUID")),
    responses(
        (status = 200, description = "Segment", body = AdminSegmentJson),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing segment:read"),
        (status = 404, description = "Segment not found"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_segment(
    State(state): State<Arc<GatewayState>>,
    Path(segment_id): Path<String>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "segment:read")?;

    let org_id = req
        .extensions()
        .get::<stitchd_proto::auth::v1::RbacContext>()
        .map(|c| c.tenant_id.clone())
        .unwrap_or_default();

    let rpc = tonic::Request::new(GetAdminSegmentRequest { segment_id, org_id });
    let mut client = state.segmentation_client.lock().await;
    let resp = client
        .get_admin_segment(rpc)
        .await
        .map_err(GatewayError::from)?;
    Ok(Json(proto_to_admin_json(&resp.into_inner())))
}

/// `PUT /v1/segments/{segment_id}` — update a segment.
#[utoipa::path(
    put,
    path = "/v1/segments/{segment_id}",
    tag = "segments",
    params(("segment_id" = String, Path, description = "Segment UUID")),
    request_body = SegmentUpdateRequest,
    responses(
        (status = 200, description = "Updated segment", body = AdminSegmentJson),
        (status = 400, description = "Malformed condition_expr JSON"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing segment:write"),
        (status = 404, description = "Segment not found"),
        (status = 409, description = "Name already exists in this environment"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn update_segment(
    State(state): State<Arc<GatewayState>>,
    Path(segment_id): Path<String>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "segment:write")?;

    let (_parts, body_raw) = req.into_parts();
    let bytes = axum::body::to_bytes(body_raw, 4 * 1024 * 1024)
        .await
        .map_err(|e| GatewayError::BadRequest(e.to_string()))?;
    let body: SegmentUpdateRequest = serde_json::from_slice(&bytes)
        .map_err(|e| GatewayError::BadRequest(format!("invalid request body: {e}")))?;

    let condition_expr = encode_condition_expr(body.condition_expr)?;

    let rpc = tonic::Request::new(UpdateAdminSegmentRequest {
        segment_id,
        name: body.name.unwrap_or_default(),
        description: body.description.unwrap_or_default(),
        tags: body.tags.unwrap_or_default(),
        condition_expr,
        user_list: body.user_list.unwrap_or_default(),
        excluded_keys: body.excluded_keys.unwrap_or_default(),
        version: body.version.unwrap_or(0),
        context_type: body.context_type.unwrap_or_default(),
    });
    let mut client = state.segmentation_client.lock().await;
    let resp = client
        .update_admin_segment(rpc)
        .await
        .map_err(GatewayError::from)?;
    Ok(Json(proto_to_admin_json(&resp.into_inner())))
}

/// `DELETE /v1/segments/{segment_id}` — delete a segment.
#[utoipa::path(
    delete,
    path = "/v1/segments/{segment_id}",
    tag = "segments",
    params(("segment_id" = String, Path, description = "Segment UUID")),
    responses(
        (status = 204, description = "Segment deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing segment:write"),
        (status = 404, description = "Segment not found"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn delete_segment(
    State(state): State<Arc<GatewayState>>,
    Path(segment_id): Path<String>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "segment:write")?;

    let org_id = req
        .extensions()
        .get::<stitchd_proto::auth::v1::RbacContext>()
        .map(|c| c.tenant_id.clone())
        .unwrap_or_default();

    let rpc = tonic::Request::new(DeleteAdminSegmentRequest { segment_id, org_id });
    let mut client = state.segmentation_client.lock().await;
    client
        .delete_admin_segment(rpc)
        .await
        .map_err(GatewayError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/segments/{segment_id}/entries` — patch (add/remove/replace) list entries.
#[utoipa::path(
    post,
    path = "/v1/segments/{segment_id}/entries",
    tag = "segments",
    params(("segment_id" = String, Path, description = "Segment UUID")),
    request_body = PatchEntriesRequest,
    responses(
        (status = 200, description = "Updated counts", body = PatchEntriesResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing segment:write"),
        (status = 404, description = "Segment not found"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn patch_segment_entries(
    State(state): State<Arc<GatewayState>>,
    Path(segment_id): Path<String>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "segment:write")?;

    let org_id = req
        .extensions()
        .get::<stitchd_proto::auth::v1::RbacContext>()
        .map(|c| c.tenant_id.clone())
        .unwrap_or_default();

    let (_parts, body_raw) = req.into_parts();
    let bytes = axum::body::to_bytes(body_raw, 4 * 1024 * 1024)
        .await
        .map_err(|e| GatewayError::BadRequest(e.to_string()))?;
    let body: PatchEntriesRequest = serde_json::from_slice(&bytes)
        .map_err(|e| GatewayError::BadRequest(format!("invalid request body: {e}")))?;

    let rpc = tonic::Request::new(PatchSegmentEntriesRequest {
        segment_id,
        keys: body.keys,
        list_type: body.list_type,
        action: body.action,
        org_id,
    });
    let mut client = state.segmentation_client.lock().await;
    let resp = client
        .patch_segment_entries(rpc)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    Ok(Json(PatchEntriesResponse {
        include_count: inner.include_count,
        exclude_count: inner.exclude_count,
    }))
}

/// `GET /v1/segments/{segment_id}/entries/lookup?key=...` — look up a single key.
#[utoipa::path(
    get,
    path = "/v1/segments/{segment_id}/entries/lookup",
    tag = "segments",
    params(
        ("segment_id" = String, Path, description = "Segment UUID"),
        ("key" = String, Query, description = "Key to look up"),
    ),
    responses(
        (status = 200, description = "Lookup result", body = LookupEntryResponse),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing segment:read"),
        (status = 404, description = "Segment not found"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn lookup_segment_entry(
    State(state): State<Arc<GatewayState>>,
    Path(segment_id): Path<String>,
    Query(query): Query<LookupEntryQuery>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "segment:read")?;

    let org_id = req
        .extensions()
        .get::<stitchd_proto::auth::v1::RbacContext>()
        .map(|c| c.tenant_id.clone())
        .unwrap_or_default();

    let rpc = tonic::Request::new(LookupSegmentEntryRequest {
        segment_id,
        key: query.key,
        org_id,
    });
    let mut client = state.segmentation_client.lock().await;
    let resp = client
        .lookup_segment_entry(rpc)
        .await
        .map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    Ok(Json(LookupEntryResponse {
        in_include: inner.in_include,
        in_exclude: inner.in_exclude,
    }))
}

// ─── Shared helpers ───────────────────────────────────────────────────────────

/// Serialise an optional `condition_expr` JSON value to bytes for the proto.
/// Validation of forbidden operators now happens server-side in the
/// segmentation-service (GL-11). Returns `GatewayError::BadRequest` only
/// if the value cannot be serialised.
fn encode_condition_expr(expr: Option<serde_json::Value>) -> Result<Vec<u8>, GatewayError> {
    match expr {
        None => Ok(Vec::new()),
        Some(v) => serde_json::to_vec(&v)
            .map_err(|e| GatewayError::BadRequest(format!("malformed condition_expr: {e}"))),
    }
}

/// Ops that are not permitted inside a segment's own `condition_expr`.
///
/// Kept for unit-test coverage only; enforcement now lives in the
/// segmentation-service gRPC handler (GL-11).
#[cfg(test)]
const SEGMENT_FORBIDDEN_OPS: &[&str] = &["InSegment", "NotInSegment", "FlagEvaluatedAs"];

/// Walk a `ConditionExpr` JSON tree and return an error if any leaf uses a
/// forbidden operator (see [`SEGMENT_FORBIDDEN_OPS`]).
///
/// Kept for unit-test coverage only; enforcement now lives in the
/// segmentation-service gRPC handler (GL-11).
#[cfg(test)]
fn validate_segment_condition_expr(expr: &serde_json::Value) -> Result<(), GatewayError> {
    if expr.is_null() {
        return Ok(());
    }
    // Leaf node: {"Leaf": <condition>}
    if let Some(leaf) = expr.get("Leaf") {
        if let Some(obj) = leaf.as_object() {
            for op in obj.keys() {
                if SEGMENT_FORBIDDEN_OPS.contains(&op.as_str()) {
                    return Err(GatewayError::BadRequest(format!(
                        "condition operator '{op}' is not allowed in segment rules \
                         (segments cannot reference other segments or flag evaluations)"
                    )));
                }
            }
        }
        return Ok(());
    }
    // And / Or: recurse into children array
    for key in &["And", "Or"] {
        if let Some(arr) = expr.get(key).and_then(|v| v.as_array()) {
            for child in arr {
                validate_segment_condition_expr(child)?;
            }
            return Ok(());
        }
    }
    // Not: recurse into the single inner expression
    if let Some(inner) = expr.get("Not") {
        return validate_segment_condition_expr(inner);
    }
    Ok(())
}

// ─── Test helpers ─────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    #[allow(unused_imports)]
    use axum::routing::{delete, get, post, put};
    axum::Router::new()
        .route("/v1/segments", get(list_segments).post(create_segment))
        .route(
            "/v1/segments/{segment_id}",
            get(get_segment).put(update_segment).delete(delete_segment),
        )
        .route(
            "/v1/environments/{environment_id}/segments",
            post(create_segment_in_env),
        )
        .route(
            "/v1/segments/{segment_id}/entries",
            post(patch_segment_entries),
        )
        .route(
            "/v1/segments/{segment_id}/entries/lookup",
            get(lookup_segment_entry),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt as _;

    use crate::tests::helpers::make_stub_state;

    // ─── Happy paths (stub returns 502 as downstream is unreachable) ──────────

    #[tokio::test]
    async fn list_segments_with_permission_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        // Inject RbacContext with segment:read permission.
        let mut req = Request::builder()
            .uri("/v1/segments?env_id=env-1")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["segment:read"]));
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn list_segments_without_permission_returns_401() {
        let state = make_stub_state();
        let app = test_router(state);
        let mut req = Request::builder()
            .uri("/v1/segments?env_id=env-1")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(make_rbac_with_perms(&[]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn list_segments_without_rbac_returns_401() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/segments?env_id=env-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_segment_with_write_permission_returns_201_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let mut req = Request::builder()
            .method("POST")
            .uri("/v1/segments")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"beta-users","tags":["internal"]}"#))
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["segment:write"]));
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status() == StatusCode::CREATED || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn create_segment_without_write_permission_returns_401() {
        let state = make_stub_state();
        let app = test_router(state);
        let mut req = Request::builder()
            .method("POST")
            .uri("/v1/segments")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"beta-users"}"#))
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["segment:read"]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_segment_with_read_permission_returns_200_404_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let mut req = Request::builder()
            .uri("/v1/segments/550e8400-e29b-41d4-a716-446655440000")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["segment:read"]));
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status() == StatusCode::OK
                || resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn get_segment_without_permission_returns_401() {
        let state = make_stub_state();
        let app = test_router(state);
        let mut req = Request::builder()
            .uri("/v1/segments/550e8400-e29b-41d4-a716-446655440000")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(make_rbac_with_perms(&[]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn update_segment_with_write_permission_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let mut req = Request::builder()
            .method("PUT")
            .uri("/v1/segments/550e8400-e29b-41d4-a716-446655440000")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"updated","version":1}"#))
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["segment:write"]));
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn update_segment_without_write_permission_returns_401() {
        let state = make_stub_state();
        let app = test_router(state);
        let mut req = Request::builder()
            .method("PUT")
            .uri("/v1/segments/550e8400-e29b-41d4-a716-446655440000")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"updated","version":1}"#))
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["segment:read"]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn delete_segment_with_write_permission_returns_204_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let mut req = Request::builder()
            .method("DELETE")
            .uri("/v1/segments/550e8400-e29b-41d4-a716-446655440000")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["segment:write"]));
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status() == StatusCode::NO_CONTENT || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn delete_segment_without_write_permission_returns_401() {
        let state = make_stub_state();
        let app = test_router(state);
        let mut req = Request::builder()
            .method("DELETE")
            .uri("/v1/segments/550e8400-e29b-41d4-a716-446655440000")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["segment:read"]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ─── Bad condition JSON ───────────────────────────────────────────────────

    #[test]
    fn encode_condition_expr_none_is_empty_vec() {
        let result = encode_condition_expr(None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn encode_condition_expr_valid_leaf_roundtrips() {
        let expr = serde_json::json!({"Leaf": {"Eq": {"context_type": "user", "param": "plan", "value": "pro"}}});
        let bytes = encode_condition_expr(Some(expr.clone())).unwrap();
        let back: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(expr, back);
    }

    // ─── validate_segment_condition_expr ─────────────────────────────────────

    #[test]
    fn validate_allows_eq_leaf() {
        let expr = serde_json::json!({"Leaf": {"Eq": {"context_type": "user", "param": "plan", "value": "pro"}}});
        assert!(validate_segment_condition_expr(&expr).is_ok());
    }

    #[test]
    fn validate_rejects_in_segment_leaf() {
        let expr = serde_json::json!({"Leaf": {"InSegment": "seg-id"}});
        let err = validate_segment_condition_expr(&expr).unwrap_err();
        assert!(err.to_string().contains("InSegment"));
    }

    #[test]
    fn validate_rejects_not_in_segment_leaf() {
        let expr = serde_json::json!({"Leaf": {"NotInSegment": "seg-id"}});
        let err = validate_segment_condition_expr(&expr).unwrap_err();
        assert!(err.to_string().contains("NotInSegment"));
    }

    #[test]
    fn validate_rejects_flag_evaluated_as_leaf() {
        let expr =
            serde_json::json!({"Leaf": {"FlagEvaluatedAs": {"flag_id": "f1", "variant_id": "on"}}});
        let err = validate_segment_condition_expr(&expr).unwrap_err();
        assert!(err.to_string().contains("FlagEvaluatedAs"));
    }

    #[test]
    fn validate_rejects_forbidden_op_nested_in_and() {
        let expr = serde_json::json!({
            "And": [
                {"Leaf": {"Eq": {"context_type": "user", "param": "plan", "value": "pro"}}},
                {"Leaf": {"InSegment": "seg-id"}}
            ]
        });
        assert!(validate_segment_condition_expr(&expr).is_err());
    }

    #[test]
    fn validate_rejects_forbidden_op_nested_in_not() {
        let expr = serde_json::json!({"Not": {"Leaf": {"FlagEvaluatedAs": {"flag_id": "f", "variant_id": "on"}}}});
        assert!(validate_segment_condition_expr(&expr).is_err());
    }

    #[test]
    fn validate_accepts_complex_allowed_expr() {
        let expr = serde_json::json!({
            "And": [
                {"Leaf": {"Eq": {"context_type": "user", "param": "plan", "value": "pro"}}},
                {"Not": {"Leaf": {"Contains": {"context_type": "user", "param": "email", "substr": "@test.com"}}}},
                {"Or": [
                    {"Leaf": {"Gte": {"context_type": "user", "param": "age", "value": 18}}},
                    {"Leaf": {"SemverGte": {"context_type": "app", "param": "version", "value": "2.0.0"}}}
                ]}
            ]
        });
        assert!(validate_segment_condition_expr(&expr).is_ok());
    }

    #[test]
    fn validate_null_expr_is_ok() {
        assert!(validate_segment_condition_expr(&serde_json::Value::Null).is_ok());
    }

    // ─── count_conditions helper ──────────────────────────────────────────────

    #[test]
    fn count_conditions_null_is_zero() {
        assert_eq!(count_conditions(&serde_json::Value::Null), 0);
    }

    #[test]
    fn count_conditions_and_counts_children() {
        let expr = serde_json::json!({"And": [1, 2, 3]});
        assert_eq!(count_conditions(&expr), 3);
    }

    #[test]
    fn count_conditions_or_counts_children() {
        let expr = serde_json::json!({"Or": [1, 2]});
        assert_eq!(count_conditions(&expr), 2);
    }

    #[test]
    fn count_conditions_leaf_is_one() {
        let expr = serde_json::json!({"Leaf": "something"});
        assert_eq!(count_conditions(&expr), 1);
    }

    // ─── RBAC: org_member gets segment:read only ──────────────────────────────

    #[tokio::test]
    async fn org_member_can_list_segments() {
        let state = make_stub_state();
        let app = test_router(state);
        let mut req = Request::builder()
            .uri("/v1/segments?env_id=env-1")
            .body(Body::empty())
            .unwrap();
        // org_member has segment:read
        req.extensions_mut().insert(make_rbac_with_perms(&[
            "environment:read",
            "flag:read",
            "segment:read",
        ]));
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn org_member_cannot_create_segment() {
        let state = make_stub_state();
        let app = test_router(state);
        let mut req = Request::builder()
            .method("POST")
            .uri("/v1/segments")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"name":"test"}"#))
            .unwrap();
        // org_member does NOT have segment:write
        req.extensions_mut().insert(make_rbac_with_perms(&[
            "environment:read",
            "flag:read",
            "segment:read",
        ]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ─── Helper ──────────────────────────────────────────────────────────────

    fn make_rbac_with_perms(perms: &[&str]) -> stitchd_proto::auth::v1::RbacContext {
        stitchd_proto::auth::v1::RbacContext {
            tenant_id: "org-1".to_string(),
            environment_id: String::new(),
            roles: vec!["org_member".to_string()],
            permissions: perms.iter().map(|s| s.to_string()).collect(),
            subject: "user-1".to_string(),
            is_system: false,
        }
    }
}
