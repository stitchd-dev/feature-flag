//! Admin event-definition CRUD — `/v1/events*` routes.
//!
//! These routes close `feature-flag-wr4` — the pre-existing gap where the
//! admin UI assumed `/v1/events*` endpoints existed but the gateway only
//! had stubs at `/v1/environments/{env}/event-definitions`. They proxy to
//! the analytics-service's new `*EventDefinition` gRPC RPCs.
//!
//! Pattern mirrors `routes::metrics` — `env_id` comes from a query
//! parameter on JWT-tier admin routes (admin JWTs are org-scoped, not
//! env-scoped; the EnvSwitcher in `OrgContext` selects which env to act
//! against).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use stitchd_proto::analytics::v1::{
    CreateEventDefinitionRequest, DeleteEventDefinitionRequest, EventDefinitionMsg,
    GetEventDefinitionRequest, ListEventDefinitionsRequest, UpdateEventDefinitionRequest,
};

use crate::error::GatewayError;
use crate::state::GatewayState;

// ── Wire DTOs (admin JSON ↔ proto translation lives here) ────────────────────

/// `GET /v1/events?env_id=...` query parameters.
#[derive(Debug, Deserialize, ToSchema)]
pub struct EventsQuery {
    pub env_id: String,
    #[serde(default)]
    pub page: Option<u32>,
    #[serde(default)]
    pub per_page: Option<u32>,
    #[serde(default)]
    pub include_archived: Option<bool>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateEventBody {
    pub environment_id: String,
    pub event_key: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Optional — defaults derived from metric_type.
    #[serde(default)]
    pub value_type: Option<String>,
    pub metric_type: String,
    /// Optional JSON Schema (object). Pass null/omit for no schema.
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateEventBody {
    pub expected_version: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub value_type: Option<String>,
    #[serde(default)]
    pub metric_type: Option<String>,
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct GetByKeyQuery {
    pub env_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EventJson {
    pub id: String,
    pub environment_id: String,
    pub event_key: String,
    pub key: String, // alias — admin UI uses `key`
    pub name: String,
    pub description: Option<String>,
    pub value_type: String,
    pub metric_type: String,
    pub schema: Option<serde_json::Value>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub deleted_at: Option<String>,
    pub archived: bool,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListEventsJson {
    pub items: Vec<EventJson>,
    pub total: u64,
    pub page: u32,
    pub per_page: u32,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn proto_to_json(p: EventDefinitionMsg) -> EventJson {
    let archived = p.deleted_at.is_some();
    let schema = if p.schema_json.is_empty() {
        None
    } else {
        serde_json::from_str(&p.schema_json).ok()
    };
    EventJson {
        id: p.id,
        environment_id: p.environment_id,
        event_key: p.key.clone(),
        key: p.key,
        name: p.name,
        description: p.description,
        value_type: p.value_type,
        metric_type: p.metric_type,
        schema,
        version: p.version,
        created_at: p.created_at,
        updated_at: p.updated_at,
        deleted_at: p.deleted_at,
        archived,
    }
}

fn map_tonic(e: tonic::Status) -> GatewayError {
    use tonic::Code;
    let msg = e.message().to_string();
    match e.code() {
        Code::NotFound => GatewayError::NotFound(msg),
        Code::AlreadyExists | Code::Aborted | Code::FailedPrecondition => {
            GatewayError::Conflict(msg)
        }
        Code::InvalidArgument => GatewayError::BadRequest(msg),
        Code::Unauthenticated | Code::PermissionDenied => GatewayError::Unauthorized(msg),
        _ => GatewayError::Upstream(msg),
    }
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /v1/events?env_id={uuid}&page=N&per_page=M&include_archived=bool`
#[utoipa::path(
    get,
    path = "/v1/events",
    tag = "events",
    params(
        ("env_id" = String, Query, description = "Environment UUID"),
        ("page" = Option<u32>, Query, description = "1-indexed page; defaults to 1"),
        ("per_page" = Option<u32>, Query, description = "Page size; defaults to 50, max 200"),
        ("include_archived" = Option<bool>, Query, description = "Include soft-deleted rows")
    ),
    responses(
        (status = 200, description = "Paginated list of event definitions", body = ListEventsJson),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn list_events(
    State(state): State<Arc<GatewayState>>,
    Query(q): Query<EventsQuery>,
) -> Result<Json<ListEventsJson>, GatewayError> {
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(50).clamp(1, 200);
    let offset = u64::from((page - 1) * per_page);
    let limit = u64::from(per_page);

    let proto_req = tonic::Request::new(ListEventDefinitionsRequest {
        environment_id: q.env_id,
        offset: Some(offset),
        limit: Some(limit),
        include_archived: q.include_archived,
    });
    let mut client = state.analytics_client.lock().await;
    let resp = client
        .list_event_definitions(proto_req)
        .await
        .map_err(map_tonic)?
        .into_inner();
    Ok(Json(ListEventsJson {
        items: resp.items.into_iter().map(proto_to_json).collect(),
        total: resp.total,
        page,
        per_page,
    }))
}

/// `POST /v1/events`
#[utoipa::path(
    post,
    path = "/v1/events",
    tag = "events",
    request_body = CreateEventBody,
    responses(
        (status = 201, description = "Created", body = EventJson),
        (status = 400, description = "Invalid body"),
        (status = 401, description = "Unauthorized"),
        (status = 409, description = "Duplicate key in environment"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_event(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<CreateEventBody>,
) -> Result<(StatusCode, Json<EventJson>), GatewayError> {
    let name = body.name.unwrap_or_else(|| body.event_key.clone());
    let schema_json = body.schema.as_ref().map(serde_json::Value::to_string);
    let proto_req = tonic::Request::new(CreateEventDefinitionRequest {
        environment_id: body.environment_id,
        key: body.event_key,
        name,
        description: body.description,
        value_type: body.value_type,
        metric_type: body.metric_type,
        schema_json,
    });
    let mut client = state.analytics_client.lock().await;
    let resp = client
        .create_event_definition(proto_req)
        .await
        .map_err(map_tonic)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(proto_to_json(resp))))
}

/// `GET /v1/events/{event_key}?env_id={uuid}`
#[utoipa::path(
    get,
    path = "/v1/events/{event_key}",
    tag = "events",
    params(
        ("event_key" = String, Path, description = "Event key"),
        ("env_id" = String, Query, description = "Environment UUID"),
    ),
    responses(
        (status = 200, description = "Event definition", body = EventJson),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_event(
    State(state): State<Arc<GatewayState>>,
    Path(event_key): Path<String>,
    Query(q): Query<GetByKeyQuery>,
) -> Result<Json<EventJson>, GatewayError> {
    let proto_req = tonic::Request::new(GetEventDefinitionRequest {
        id: None,
        environment_id: Some(q.env_id),
        key: Some(event_key),
    });
    let mut client = state.analytics_client.lock().await;
    let resp = client
        .get_event_definition(proto_req)
        .await
        .map_err(map_tonic)?
        .into_inner();
    Ok(Json(proto_to_json(resp)))
}

/// `PATCH /v1/events/{event_key}?env_id={uuid}` — resolves key → id then updates.
#[utoipa::path(
    patch,
    path = "/v1/events/{event_key}",
    tag = "events",
    params(
        ("event_key" = String, Path, description = "Event key"),
        ("env_id" = String, Query, description = "Environment UUID"),
    ),
    request_body = UpdateEventBody,
    responses(
        (status = 200, description = "Updated", body = EventJson),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
        (status = 409, description = "Version conflict"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn update_event(
    State(state): State<Arc<GatewayState>>,
    Path(event_key): Path<String>,
    Query(q): Query<GetByKeyQuery>,
    Json(body): Json<UpdateEventBody>,
) -> Result<Json<EventJson>, GatewayError> {
    let mut client = state.analytics_client.lock().await;

    // Resolve key → id first.
    let existing = client
        .get_event_definition(tonic::Request::new(GetEventDefinitionRequest {
            id: None,
            environment_id: Some(q.env_id.clone()),
            key: Some(event_key),
        }))
        .await
        .map_err(map_tonic)?
        .into_inner();

    let schema_json = body.schema.as_ref().map(serde_json::Value::to_string);
    let proto_req = tonic::Request::new(UpdateEventDefinitionRequest {
        id: existing.id,
        expected_version: body.expected_version,
        name: body.name,
        description: body.description,
        value_type: body.value_type,
        metric_type: body.metric_type,
        schema_json,
    });
    let resp = client
        .update_event_definition(proto_req)
        .await
        .map_err(map_tonic)?
        .into_inner();
    Ok(Json(proto_to_json(resp)))
}

/// `DELETE /v1/events/{event_key}?env_id={uuid}` — resolves key → id then soft-deletes.
#[utoipa::path(
    delete,
    path = "/v1/events/{event_key}",
    tag = "events",
    params(
        ("event_key" = String, Path, description = "Event key"),
        ("env_id" = String, Query, description = "Environment UUID"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 404, description = "Not found"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn delete_event(
    State(state): State<Arc<GatewayState>>,
    Path(event_key): Path<String>,
    Query(q): Query<GetByKeyQuery>,
) -> Result<StatusCode, GatewayError> {
    let mut client = state.analytics_client.lock().await;

    let existing = client
        .get_event_definition(tonic::Request::new(GetEventDefinitionRequest {
            id: None,
            environment_id: Some(q.env_id),
            key: Some(event_key),
        }))
        .await
        .map_err(map_tonic)?
        .into_inner();

    client
        .delete_event_definition(tonic::Request::new(DeleteEventDefinitionRequest {
            id: existing.id,
        }))
        .await
        .map_err(map_tonic)?;
    Ok(StatusCode::NO_CONTENT)
}
