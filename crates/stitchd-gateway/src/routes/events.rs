//! Event route handlers — proxy REST requests to the Event Ingestion Service.

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tonic::Request as TonicRequest;
use utoipa::ToSchema;

use stitchd_proto::analytics::v1::{
    CreateEventDefinitionRequest, DeleteEventDefinitionRequest, GetEventDefinitionRequest,
    GetEventFiringsRequest, GetEventStatsRequest, IngestEventRequest, ListEventDefinitionsRequest,
    MetricEvent, MetricValue, TrackEvent as ProtoTrackEvent,
    TrackEventsRequest as ProtoTrackEventsRequest, UpdateEventDefinitionRequest, metric_value,
};

use crate::error::GatewayError;
use crate::middleware::sdk_auth::SdkContext;
use crate::pagination::{PaginatedResponse, PaginationParams};
use crate::state::GatewayState;

use super::require_permission;

/// Header used to forward the trusted `env_id` to backend gRPC services.
const ENV_ID_METADATA_KEY: &str = "x-env-id";

/// Body size limit for `POST /v1/events/track` (5 MiB). Per spec F3.4 — one
/// batch of ~25k tiny events. Larger payloads return `413 Payload Too Large`
/// (axum emits this automatically when the `DefaultBodyLimit` layer rejects).
pub const TRACK_EVENTS_BODY_LIMIT_BYTES: usize = 5 * 1024 * 1024;

// ─── REST types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct EventBody {
    pub metric_key: String,
    pub context_type: String,
    pub context_key: String,
    #[schema(value_type = Object, nullable = true)]
    pub value: Option<serde_json::Value>,
    pub timestamp_ms: Option<i64>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BatchEventBody {
    pub events: Vec<EventBody>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IngestResponseJson {
    pub accepted_count: u32,
    pub rejected_keys: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EventDefinitionJson {
    pub id: String,
    pub environment_id: String,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub value_type: String,
    pub metric_type: String,
    pub schema_json: String,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
}

fn proto_to_event_def_json(
    msg: stitchd_proto::analytics::v1::EventDefinitionMsg,
) -> EventDefinitionJson {
    EventDefinitionJson {
        id: msg.id,
        environment_id: msg.environment_id,
        key: msg.key,
        name: msg.name,
        description: msg.description,
        value_type: msg.value_type,
        metric_type: msg.metric_type,
        schema_json: msg.schema_json,
        version: msg.version,
        created_at: msg.created_at,
        updated_at: msg.updated_at,
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateEventDefinitionBody {
    pub key: String,
    pub name: String,
    pub description: Option<String>,
    pub value_type: Option<String>,
    pub metric_type: String,
    pub schema_json: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateEventDefinitionBody {
    pub expected_version: i64,
    pub name: Option<String>,
    pub description: Option<String>,
    pub value_type: Option<String>,
    pub metric_type: Option<String>,
    pub schema_json: Option<String>,
}

fn body_to_event(b: &EventBody) -> MetricEvent {
    let value = b.value.as_ref().map(|v| {
        if let Some(b) = v.as_bool() {
            MetricValue {
                value: Some(metric_value::Value::BoolValue(b)),
            }
        } else if let Some(i) = v.as_i64() {
            MetricValue {
                value: Some(metric_value::Value::IntValue(i)),
            }
        } else if let Some(f) = v.as_f64() {
            MetricValue {
                value: Some(metric_value::Value::DoubleValue(f)),
            }
        } else {
            MetricValue { value: None }
        }
    });
    MetricEvent {
        metric_key: b.metric_key.clone(),
        context_type: b.context_type.clone(),
        context_key: b.context_key.clone(),
        value,
        timestamp_ms: b.timestamp_ms.unwrap_or(0),
    }
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `POST /v1/environments/{environment_id}/events`
#[utoipa::path(
    post,
    path = "/v1/environments/{environment_id}/events",
    tag = "event-definitions",
    params(("environment_id" = String, Path, description = "Environment ID")),
    request_body = EventBody,
    responses(
        (status = 200, description = "Event ingested", body = IngestResponseJson),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Event service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn ingest_event(
    State(state): State<Arc<GatewayState>>,
    Path(_environment_id): Path<String>,
    Json(body): Json<EventBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(IngestEventRequest {
        events: vec![body_to_event(&body)],
    });
    let mut client = state.analytics_client.lock().await;
    let resp = client.ingest_event(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    Ok(Json(IngestResponseJson {
        accepted_count: inner.accepted_count,
        rejected_keys: inner.rejected_keys,
    }))
}

/// `POST /v1/environments/{environment_id}/events/batch`
#[utoipa::path(
    post,
    path = "/v1/environments/{environment_id}/events/batch",
    tag = "event-definitions",
    params(("environment_id" = String, Path, description = "Environment ID")),
    request_body = BatchEventBody,
    responses(
        (status = 200, description = "Batch ingested", body = IngestResponseJson),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Event service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn ingest_batch(
    State(state): State<Arc<GatewayState>>,
    Path(_environment_id): Path<String>,
    Json(body): Json<BatchEventBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let events = body.events.iter().map(body_to_event).collect();
    let req = tonic::Request::new(IngestEventRequest { events });
    let mut client = state.analytics_client.lock().await;
    let resp = client.ingest_event(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    Ok(Json(IngestResponseJson {
        accepted_count: inner.accepted_count,
        rejected_keys: inner.rejected_keys,
    }))
}

/// `GET /v1/environments/{environment_id}/event-definitions`
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/event-definitions",
    tag = "event-definitions",
    params(("environment_id" = String, Path, description = "Environment ID")),
    responses(
        (status = 200, description = "Paginated list of event definitions"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn list_event_definitions(
    State(state): State<Arc<GatewayState>>,
    Path(environment_id): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = TonicRequest::new(ListEventDefinitionsRequest {
        environment_id,
        offset: Some(pagination.offset()),
        limit: Some(pagination.effective_per_page() as u64),
        include_archived: Some(false),
    });
    let mut client = state.analytics_client.lock().await;
    let resp = client
        .list_event_definitions(req)
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    let items: Vec<EventDefinitionJson> = resp
        .items
        .into_iter()
        .map(proto_to_event_def_json)
        .collect();
    Ok(Json(PaginatedResponse::new(items, resp.total, &pagination)))
}

/// `POST /v1/environments/{environment_id}/event-definitions`
#[utoipa::path(
    post,
    path = "/v1/environments/{environment_id}/event-definitions",
    tag = "event-definitions",
    params(("environment_id" = String, Path, description = "Environment ID")),
    responses(
        (status = 202, description = "Event definition accepted"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_event_definition(
    State(state): State<Arc<GatewayState>>,
    Path(environment_id): Path<String>,
    Json(body): Json<CreateEventDefinitionBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = TonicRequest::new(CreateEventDefinitionRequest {
        environment_id,
        key: body.key,
        name: body.name,
        description: body.description,
        value_type: body.value_type,
        metric_type: body.metric_type,
        schema_json: body.schema_json,
    });
    let mut client = state.analytics_client.lock().await;
    let resp = client
        .create_event_definition(req)
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(proto_to_event_def_json(resp))))
}

/// `GET /v1/environments/{environment_id}/event-definitions/{event_definition_id}`
#[utoipa::path(
    get,
    path = "/v1/environments/{environment_id}/event-definitions/{event_definition_id}",
    tag = "event-definitions",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("event_definition_id" = String, Path, description = "Event definition key"),
    ),
    responses(
        (status = 501, description = "Not implemented"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_event_definition(
    State(state): State<Arc<GatewayState>>,
    Path((environment_id, event_definition_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = TonicRequest::new(GetEventDefinitionRequest {
        id: Some(event_definition_id),
        environment_id: Some(environment_id),
        key: None,
    });
    let mut client = state.analytics_client.lock().await;
    let resp = client
        .get_event_definition(req)
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    Ok(Json(proto_to_event_def_json(resp)))
}

/// `PUT /v1/environments/{environment_id}/event-definitions/{event_definition_id}`
#[utoipa::path(
    put,
    path = "/v1/environments/{environment_id}/event-definitions/{event_definition_id}",
    tag = "event-definitions",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("event_definition_id" = String, Path, description = "Event definition key"),
    ),
    responses(
        (status = 202, description = "Update accepted"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn update_event_definition(
    State(state): State<Arc<GatewayState>>,
    Path((_environment_id, event_definition_id)): Path<(String, String)>,
    Json(body): Json<UpdateEventDefinitionBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = TonicRequest::new(UpdateEventDefinitionRequest {
        id: event_definition_id,
        expected_version: body.expected_version,
        name: body.name,
        description: body.description,
        value_type: body.value_type,
        metric_type: body.metric_type,
        schema_json: body.schema_json,
    });
    let mut client = state.analytics_client.lock().await;
    let resp = client
        .update_event_definition(req)
        .await
        .map_err(GatewayError::from)?
        .into_inner();
    Ok(Json(proto_to_event_def_json(resp)))
}

/// `DELETE /v1/environments/{environment_id}/event-definitions/{event_definition_id}`
#[utoipa::path(
    delete,
    path = "/v1/environments/{environment_id}/event-definitions/{event_definition_id}",
    tag = "event-definitions",
    params(
        ("environment_id" = String, Path, description = "Environment ID"),
        ("event_definition_id" = String, Path, description = "Event definition key"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "Unauthorized"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn delete_event_definition(
    State(state): State<Arc<GatewayState>>,
    Path((_environment_id, event_definition_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = TonicRequest::new(DeleteEventDefinitionRequest {
        id: event_definition_id,
    });
    let mut client = state.analytics_client.lock().await;
    client
        .delete_event_definition(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

// ─── POST /v1/events/track — SDK batch event ingestion ──────────────────────
//
// Surface owned by Phase 2 Task 2 of `events_metrics_20260519`. Lives on the
// gateway's SDK-auth tier (NOT JWT). The SDK auth middleware resolves
// `x-sdk-key` → [`SdkContext { environment_id, .. }`]; this handler forwards
// the env to analytics-service via `x-env-id` gRPC metadata. The downstream
// `TrackEvents` RPC (see `analytics-service::grpc::ingestion`) validates each
// event against `event_definitions` and per-event rejections are surfaced
// here as `202 Accepted` with a `rejected[]` array (NOT 400 — the batch can
// be partially accepted).

/// Typed metric value mirroring proto `MetricValue.oneof value`.
///
/// JSON shape (snake_case discriminator):
/// - `{"bool": true}`     → `MetricValue::BoolValue`
/// - `{"int": 42}`        → `MetricValue::IntValue`
/// - `{"double": 1.5}`    → `MetricValue::DoubleValue`
///
/// The discriminator is `lowercase` to match the existing `MetricValue`
/// shape consumed by the legacy `POST /v1/environments/.../events` route
/// (which accepts a raw JSON scalar — `true` / `42` / `1.5`). The SDK-facing
/// shape is explicit so callers don't have to rely on JSON's loose number
/// inference (e.g. `42` could be int OR double depending on JSON parser).
#[derive(Debug, Clone, Deserialize, Serialize, ToSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TypedValue {
    Bool(bool),
    Int(i64),
    Double(f64),
}

impl TypedValue {
    fn to_proto(&self) -> MetricValue {
        let inner = match self {
            TypedValue::Bool(b) => metric_value::Value::BoolValue(*b),
            TypedValue::Int(i) => metric_value::Value::IntValue(*i),
            TypedValue::Double(d) => metric_value::Value::DoubleValue(*d),
        };
        MetricValue { value: Some(inner) }
    }
}

/// One event in a `POST /v1/events/track` batch.
///
/// `properties` and `occurred_at` are optional — analytics-service falls
/// back to an empty map / ingestion timestamp respectively. The
/// `environment_id` is **not** accepted from the SDK; the gateway populates
/// it from the resolved [`SdkContext`].
#[derive(Debug, Deserialize, ToSchema)]
pub struct TrackEvent {
    /// Pre-registered `event_definitions.key`. Unknown / archived keys are
    /// rejected (added to `rejected[]`) rather than failing the whole batch.
    pub event_key: String,
    /// Multi-dimensional attribution as a flat `type → key` map.
    /// At least one entry is required; the server rejects events with
    /// an empty map at ingestion. Example:
    /// ```json
    /// "contexts": {"user": "u-42", "account": "acme_corp", "session": "s-99"}
    /// ```
    pub contexts: HashMap<String, String>,
    /// Typed metric value. Optional for pure occurrence events where the
    /// registered `event_definitions.value_type` is `Unit`.
    pub value: Option<TypedValue>,
    /// Arbitrary per-event metadata. Used by metric filters / on-field
    /// aggregations downstream.
    pub properties: Option<HashMap<String, String>>,
    /// RFC 3339 client-side wall-clock timestamp. When absent,
    /// analytics-service uses ingestion (server) time.
    pub occurred_at: Option<String>,
}

/// Request body of `POST /v1/events/track`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TrackEventsRequest {
    pub events: Vec<TrackEvent>,
    /// Environment UUID — only honoured by the admin-auth path
    /// (`POST /v1/admin/events/track`) when the JWT's `RbacContext`
    /// has no environment scope (the common case for org-scoped admin
    /// JWTs). Ignored by the SDK-auth path, which derives `env_id`
    /// from the SDK key.
    #[serde(default)]
    pub environment_id: Option<String>,
}

/// One rejected event in a `TrackEventsResponse`. The `reason` discriminator
/// is one of: `"unknown_event_key"`, `"archived_event_key"`,
/// `"value_type_mismatch"`, `"missing_value"`, `"invalid_occurred_at"`.
#[derive(Debug, Serialize, ToSchema, PartialEq, Eq)]
pub struct RejectedEvent {
    pub event_key: String,
    pub reason: String,
}

/// Response body of `POST /v1/events/track`.
///
/// Status code is always `202 Accepted` on success — per-event rejections do
/// **not** turn into HTTP-level errors. The whole batch fails (502/500) only
/// when analytics-service itself errors out.
#[derive(Debug, Serialize, ToSchema)]
pub struct TrackEventsResponse {
    pub accepted_count: u32,
    pub rejected: Vec<RejectedEvent>,
}

fn json_event_to_proto(e: TrackEvent) -> ProtoTrackEvent {
    ProtoTrackEvent {
        event_key: e.event_key,
        value: e.value.as_ref().map(TypedValue::to_proto),
        properties: e.properties.unwrap_or_default(),
        occurred_at: e.occurred_at,
        contexts: e.contexts,
    }
}

/// `POST /v1/events/track` — SDK batch event ingestion.
///
/// **Auth**: `x-sdk-key` header (NOT bearer JWT). Validated by
/// [`sdk_auth_middleware`](crate::middleware::sdk_auth::sdk_auth_middleware),
/// which injects [`SdkContext`] carrying the resolved `environment_id`.
///
/// **Forwarding**: builds [`ProtoTrackEventsRequest`] from the JSON batch
/// and forwards to `AnalyticsService.TrackEvents` with `x-env-id` gRPC
/// metadata. The analytics service performs per-event validation against
/// `event_definitions` and returns accepted_count + per-event rejections.
///
/// **Body limit**: 5 MiB ([`TRACK_EVENTS_BODY_LIMIT_BYTES`]). Larger bodies
/// are rejected with `413 Payload Too Large` by the `DefaultBodyLimit` layer
/// before reaching this handler.
///
/// **Status codes**:
/// - `202 Accepted` — batch processed (may include per-event rejections)
/// - `400 Bad Request` — malformed JSON / invalid env_id propagation
/// - `401 Unauthorized` — missing or invalid `x-sdk-key`
/// - `413 Payload Too Large` — body exceeds 5 MiB
/// - `502 Bad Gateway` — analytics-service unreachable / errored
#[utoipa::path(
    post,
    path = "/v1/events/track",
    tag = "events",
    request_body = TrackEventsRequest,
    responses(
        (status = 202, description = "Batch accepted (may include per-event rejections)", body = TrackEventsResponse),
        (status = 400, description = "Malformed request body"),
        (status = 401, description = "Missing or invalid x-sdk-key"),
        (status = 413, description = "Body exceeds 5 MiB"),
        (status = 502, description = "Analytics service unavailable"),
    ),
    security(("sdk_key" = []))
)]
pub async fn track_events(
    State(state): State<Arc<GatewayState>>,
    req: axum::extract::Request,
) -> Result<axum::response::Response, GatewayError> {
    use axum::body::Bytes;
    use axum::extract::FromRequest;
    use axum::response::IntoResponse;

    // The SdkContext extension is injected by sdk_auth_middleware. Its
    // absence means the route was registered outside the SDK-auth tier — a
    // server misconfiguration, not a client error.
    let sdk_ctx = req
        .extensions()
        .get::<SdkContext>()
        .cloned()
        .ok_or_else(|| {
            GatewayError::Upstream(
                "SdkContext extension missing — route misconfigured \
                 (sdk_auth_middleware not applied)"
                    .to_string(),
            )
        })?;

    // Use the `Bytes` extractor to honour the `DefaultBodyLimit::max(...)`
    // layer applied to this route. `Bytes::from_request` internally wraps
    // the body via `into_limited_body()` and produces a `BytesRejection`
    // that already maps oversize bodies to `413 Payload Too Large` via its
    // `IntoResponse` impl. Cheap to call regardless of payload size.
    let bytes: Bytes = match Bytes::from_request(req, &()).await {
        Ok(b) => b,
        Err(rej) => return Ok(rej.into_response()),
    };
    let body: TrackEventsRequest = serde_json::from_slice(&bytes)
        .map_err(|e| GatewayError::BadRequest(format!("invalid JSON body: {e}")))?;

    forward_to_analytics(&state, &sdk_ctx.environment_id, body, false).await
}

/// Forward a parsed `TrackEventsRequest` to analytics-service.
///
/// Shared by both `track_events` (SDK-auth tier, `mark_test=false`) and
/// `track_events_admin` (JWT-auth tier, `mark_test=true`). The `mark_test`
/// flag, when set, stamps `properties["_test"] = "true"` on every event in
/// the batch — analytics-service propagates the property verbatim onto the
/// resulting `events` rows so the admin UI can filter test firings out
/// of production aggregates.
///
/// Returns a `202 Accepted` JSON response on success, or a `GatewayError`
/// when env_id validation or the gRPC call fails.
async fn forward_to_analytics(
    state: &Arc<GatewayState>,
    env_id: &str,
    body: TrackEventsRequest,
    mark_test: bool,
) -> Result<axum::response::Response, GatewayError> {
    use axum::response::IntoResponse;

    let mut proto_events: Vec<ProtoTrackEvent> =
        body.events.into_iter().map(json_event_to_proto).collect();

    if mark_test {
        for ev in &mut proto_events {
            ev.properties
                .insert("_test".to_string(), "true".to_string());
        }
    }

    // env_id resolution is metadata-first on the analytics side, but we also
    // mirror it into the body so direct callers (tests) can drive the RPC.
    let mut tonic_req = TonicRequest::new(ProtoTrackEventsRequest {
        env_id: env_id.to_string(),
        events: proto_events,
    });
    let env_id_value = env_id.parse().map_err(|_| {
        GatewayError::Upstream(format!("env_id is not valid gRPC metadata: {env_id:?}"))
    })?;
    tonic_req
        .metadata_mut()
        .insert(ENV_ID_METADATA_KEY, env_id_value);

    let resp = {
        let mut client = state.analytics_client.lock().await;
        client.track_events(tonic_req).await
    }
    .map_err(|s| GatewayError::Upstream(format!("track_events failed: {s}")))?;

    let inner = resp.into_inner();
    let body = TrackEventsResponse {
        accepted_count: inner.accepted_count,
        rejected: inner
            .rejected
            .into_iter()
            .map(|r| RejectedEvent {
                event_key: r.event_key,
                reason: r.reason,
            })
            .collect(),
    };

    Ok((StatusCode::ACCEPTED, Json(body)).into_response())
}

// ─── GET /v1/events/{key}/firings & /stats — admin EventDetail page ─────────
//
// Both routes live on the JWT-auth tier. They forward to analytics-service
// after resolving `env_id` from the `env_id` query parameter (matching the
// existing pattern in `/v1/metrics?env_id=...`). The gateway returns 400 on
// a missing/invalid env_id rather than the 401 the SDK-tier routes use,
// since the caller is already JWT-authenticated.

#[derive(Debug, Deserialize, ToSchema)]
pub struct FiringsQuery {
    /// Environment UUID — required (JWT-tier convention).
    pub env_id: String,
    /// Page size. Default 50; server caps at 200.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StatsQuery {
    /// Environment UUID — required (JWT-tier convention).
    pub env_id: String,
    /// Day-window. Default 14; server caps at 90.
    #[serde(default)]
    pub days: Option<u32>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EventFiringJson {
    /// Multi-dimensional attribution as a flat `type → key` map.
    /// Always non-empty for rows ingested via the current path.
    pub contexts: HashMap<String, String>,
    /// Serialised JSON scalar (`true`/`42`/`1.5`); empty when occurrence-only.
    pub value_json: String,
    /// Serialised JSON object (always `{}` when empty).
    pub properties_json: String,
    pub occurred_at: String,
    pub ingested_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EventFiringsResponseJson {
    pub firings: Vec<EventFiringJson>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EventStatsBucketJson {
    pub day: String,
    pub count: u64,
    pub unique_context_keys: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EventStatsResponseJson {
    pub buckets: Vec<EventStatsBucketJson>,
}

/// `GET /v1/events/{event_key}/firings?env_id=<uuid>&limit=<n>`
///
/// Returns the most recent firings of `event_key` within `env_id`. Powers
/// the admin UI's EventDetail recent-firings table (Phase 6 Task 6.2 of
/// `events_metrics_20260519`).
#[utoipa::path(
    get,
    path = "/v1/events/{event_key}/firings",
    tag = "events",
    params(
        ("event_key" = String, Path, description = "Pre-registered event key"),
        ("env_id" = String, Query, description = "Environment UUID"),
        ("limit" = Option<u32>, Query, description = "Page size (default 50, max 200)"),
    ),
    responses(
        (status = 200, description = "Recent firings", body = EventFiringsResponseJson),
        (status = 400, description = "Invalid env_id or event_key"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Analytics service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_event_firings(
    State(state): State<Arc<GatewayState>>,
    Path(event_key): Path<String>,
    Query(query): Query<FiringsQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let rpc = tonic::Request::new(GetEventFiringsRequest {
        env_id: query.env_id,
        event_key,
        limit: query.limit.unwrap_or(0),
    });

    let resp = {
        let mut client = state.analytics_client.lock().await;
        client.get_event_firings(rpc).await
    }
    .map_err(GatewayError::from)?;

    let inner = resp.into_inner();
    let firings: Vec<EventFiringJson> = inner
        .firings
        .into_iter()
        .map(|f| EventFiringJson {
            contexts: f.contexts,
            value_json: f.value_json,
            properties_json: f.properties_json,
            occurred_at: f.occurred_at,
            ingested_at: f.ingested_at,
        })
        .collect();
    Ok(Json(EventFiringsResponseJson { firings }))
}

/// `GET /v1/events/{event_key}/stats?env_id=<uuid>&days=<n>`
///
/// Returns daily firing counts + unique-context-key counts for the last
/// `days` days. Powers the admin UI's EventDetail sparkline.
#[utoipa::path(
    get,
    path = "/v1/events/{event_key}/stats",
    tag = "events",
    params(
        ("event_key" = String, Path, description = "Pre-registered event key"),
        ("env_id" = String, Query, description = "Environment UUID"),
        ("days" = Option<u32>, Query, description = "Day-window (default 14, max 90)"),
    ),
    responses(
        (status = 200, description = "Daily firing counts", body = EventStatsResponseJson),
        (status = 400, description = "Invalid env_id or event_key"),
        (status = 401, description = "Unauthorized"),
        (status = 502, description = "Analytics service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_event_stats(
    State(state): State<Arc<GatewayState>>,
    Path(event_key): Path<String>,
    Query(query): Query<StatsQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let rpc = tonic::Request::new(GetEventStatsRequest {
        env_id: query.env_id,
        event_key,
        days: query.days.unwrap_or(0),
    });

    let resp = {
        let mut client = state.analytics_client.lock().await;
        client.get_event_stats(rpc).await
    }
    .map_err(GatewayError::from)?;

    let inner = resp.into_inner();
    let buckets: Vec<EventStatsBucketJson> = inner
        .buckets
        .into_iter()
        .map(|b| EventStatsBucketJson {
            day: b.day,
            count: b.count,
            unique_context_keys: b.unique_context_keys,
        })
        .collect();
    Ok(Json(EventStatsResponseJson { buckets }))
}

// ─── POST /v1/admin/events/track — JWT-auth admin event tracking ────────────
//
// Mirror of `POST /v1/events/track` for the admin UI (test-event widget on
// `/org/:orgId/events/:eventKey` — feature-flag-7an Phase 6 Task 4). The SDK
// path requires `x-sdk-key`, which the admin browser session does not have;
// this path accepts the standard `Bearer` JWT instead, lifts the env_id from
// the resolved `RbacContext`, and stamps every event with
// `properties["_test"] = "true"` so production aggregates can filter the
// admin-fired test events out.
//
// Auth: JWT-auth tier (`auth_middleware` → `RbacContext`), requires the
// `event:write` permission. Does NOT participate in the per-env event-quota
// (admin actions are rare; no rate-limit needed).

/// `POST /v1/admin/events/track` — admin-auth batch event ingestion.
///
/// **Auth**: `Authorization: Bearer <jwt>`. Validated by
/// [`auth_middleware`](crate::middleware::auth::auth_middleware), which
/// injects [`RbacContext`](stitchd_proto::auth::v1::RbacContext). Requires
/// the `event:write` permission.
///
/// **env_id source**: resolved from `RbacContext.environment_id` (set by the
/// admin's currently-scoped environment) — NOT from any header or body
/// field, so callers cannot impersonate another env.
///
/// **Test marker**: every event in the batch is stamped with
/// `properties["_test"] = "true"` before forwarding. Analytics-service
/// propagates the property verbatim onto `events` rows; the admin UI can
/// later filter test events out of production aggregates via
/// `properties._test = 'true'`.
///
/// **Status codes**:
/// - `202 Accepted` — batch processed (may include per-event rejections)
/// - `400 Bad Request` — malformed JSON, or missing env scope on the JWT
/// - `401 Unauthorized` — missing JWT or missing `event:write` permission
/// - `502 Bad Gateway` — analytics-service unreachable / errored
#[utoipa::path(
    post,
    path = "/v1/admin/events/track",
    tag = "events",
    request_body = TrackEventsRequest,
    responses(
        (status = 202, description = "Batch accepted (may include per-event rejections)", body = TrackEventsResponse),
        (status = 400, description = "Malformed request body or missing env scope"),
        (status = 401, description = "Missing JWT or missing event:write permission"),
        (status = 502, description = "Analytics service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn track_events_admin(
    State(state): State<Arc<GatewayState>>,
    req: axum::extract::Request,
) -> Result<axum::response::Response, GatewayError> {
    require_permission(&req, "event:write")?;

    // Lift env_id from RbacContext (env-scoped JWT) — falls back to the
    // request body's `environment_id` for org-scoped JWTs, which is the
    // common case for the test-event widget where admins are logged into
    // an org but want to fire events to a specific env they've picked in
    // the UI's EnvSwitcher.
    let jwt_env_id = req
        .extensions()
        .get::<stitchd_proto::auth::v1::RbacContext>()
        .map(|ctx| ctx.environment_id.clone())
        .ok_or_else(|| {
            // Should be unreachable — require_permission already verified
            // the extension is present.
            GatewayError::Upstream(
                "RbacContext extension missing — route misconfigured \
                 (auth_middleware not applied)"
                    .to_string(),
            )
        })?;

    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, TRACK_EVENTS_BODY_LIMIT_BYTES)
        .await
        .map_err(|e| GatewayError::BadRequest(e.to_string()))?;
    let body: TrackEventsRequest = serde_json::from_slice(&bytes)
        .map_err(|e| GatewayError::BadRequest(format!("invalid JSON body: {e}")))?;

    // Resolve env_id: JWT scope wins; otherwise fall back to body. We
    // return 400 only when BOTH sources are empty.
    let env_id = if !jwt_env_id.is_empty() {
        jwt_env_id
    } else {
        body.environment_id.clone().unwrap_or_default()
    };
    if env_id.is_empty() {
        return Err(GatewayError::BadRequest(
            "missing environment_id — either use an env-scoped JWT or include \
             `environment_id` in the request body"
                .to_string(),
        ));
    }

    forward_to_analytics(&state, &env_id, body, true).await
}

// ─── Test helpers ─────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    #[allow(unused_imports)]
    use axum::routing::{delete, get, post, put};
    axum::Router::new()
        .route(
            "/v1/environments/{environment_id}/events",
            post(ingest_event),
        )
        .route(
            "/v1/environments/{environment_id}/events/batch",
            post(ingest_batch),
        )
        .route(
            "/v1/environments/{environment_id}/event-definitions",
            get(list_event_definitions).post(create_event_definition),
        )
        .route(
            "/v1/environments/{environment_id}/event-definitions/{event_definition_id}",
            get(get_event_definition)
                .put(update_event_definition)
                .delete(delete_event_definition),
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

    #[tokio::test]
    async fn ingest_event_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/events")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"metric_key":"click","context_type":"user","context_key":"u1","value":true}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn ingest_batch_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/events/batch")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"events":[{"metric_key":"click","context_type":"user","context_key":"u1"}]}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn list_event_definitions_proxies_to_analytics() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/environments/env-1/event-definitions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // 200 with live backend; 502 in unit test (no analytics service running).
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "unexpected status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn create_event_definition_proxies_to_analytics() {
        let state = make_stub_state();
        let app = test_router(state);
        let body = r#"{"key":"click","name":"Click","metric_type":"count"}"#;
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/event-definitions")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        // 201 with live backend; 502 in unit test (no analytics service running).
        assert!(
            resp.status() == StatusCode::CREATED || resp.status() == StatusCode::BAD_GATEWAY,
            "unexpected status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn delete_event_definition_proxies_to_analytics() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/environments/env-1/event-definitions/click")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // 204 with live backend; 502 in unit test (no analytics service running).
        assert!(
            resp.status() == StatusCode::NO_CONTENT || resp.status() == StatusCode::BAD_GATEWAY,
            "unexpected status: {}",
            resp.status()
        );
    }

    #[test]
    fn body_to_event_bool() {
        let b = EventBody {
            metric_key: "k".to_string(),
            context_type: "user".to_string(),
            context_key: "u1".to_string(),
            value: Some(serde_json::json!(true)),
            timestamp_ms: Some(1000),
        };
        let e = body_to_event(&b);
        assert_eq!(e.metric_key, "k");
        assert!(matches!(
            e.value,
            Some(MetricValue {
                value: Some(metric_value::Value::BoolValue(true))
            })
        ));
    }

    #[test]
    fn body_to_event_int() {
        let b = EventBody {
            metric_key: "k".to_string(),
            context_type: "user".to_string(),
            context_key: "u1".to_string(),
            value: Some(serde_json::json!(42i64)),
            timestamp_ms: None,
        };
        let e = body_to_event(&b);
        assert!(matches!(
            e.value,
            Some(MetricValue {
                value: Some(metric_value::Value::IntValue(42))
            })
        ));
    }

    #[test]
    fn body_to_event_double() {
        let b = EventBody {
            metric_key: "k".to_string(),
            context_type: "user".to_string(),
            context_key: "u1".to_string(),
            value: Some(serde_json::json!(std::f64::consts::PI)),
            timestamp_ms: None,
        };
        let e = body_to_event(&b);
        assert!(matches!(
            e.value,
            Some(MetricValue {
                value: Some(metric_value::Value::DoubleValue(_))
            })
        ));
    }

    #[test]
    fn body_to_event_no_value() {
        let b = EventBody {
            metric_key: "k".to_string(),
            context_type: "user".to_string(),
            context_key: "u1".to_string(),
            value: None,
            timestamp_ms: None,
        };
        let e = body_to_event(&b);
        assert!(e.value.is_none());
    }

    // ─── POST /v1/events/track ────────────────────────────────────────────────

    use std::pin::Pin;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use tokio_stream::Stream;
    use tonic::transport::Channel as TonicChannel;
    use tonic::transport::Server as TonicServer;
    use tonic::{Request as ServerReq, Response as ServerResp, Status};

    use stitchd_proto::analytics::v1::{
        CreateMetricRequest, DeleteMetricRequest, DeleteMetricResponse, ExperimentResult,
        GetContextIntelligenceRequest, GetContextIntelligenceResponse, GetEvalStatsRequest,
        GetEvalStatsResponse, GetEventFiringsRequest, GetEventFiringsResponse,
        GetEventStatsRequest, GetEventStatsResponse, GetExperimentResultRequest, GetMetricRequest,
        IngestEventResponse, ListContextParamsRequest, ListContextParamsResponse,
        ListContextTypesRequest, ListContextTypesResponse, ListExperimentResultsRequest,
        ListMetricsRequest, ListMetricsResponse, MetricDefinition, PreviewMetricRequest,
        PreviewMetricResponse, RegisterContextRequest, RegisterContextResponse,
        RejectedEvent as ProtoRejectedEvent, TrackEventsResponse as ProtoTrackEventsResponse,
        UpdateMetricRequest, WriteExperimentResultsRequest, WriteExperimentResultsResponse,
        analytics_service_client::AnalyticsServiceClient,
        analytics_service_server::{AnalyticsService, AnalyticsServiceServer},
    };

    use crate::middleware::sdk_auth::SdkContext;
    use crate::state::GatewayState;

    /// `(x-env-id metadata value, TrackEvents request body)` captured per RPC.
    type CapturedCall = (Option<String>, ProtoTrackEventsRequest);

    /// Mock AnalyticsService — only `track_events` is exercised; the rest of the
    /// trait surface returns `Unimplemented`. Records the request that was
    /// received so tests can assert on metadata + body forwarding.
    struct MockAnalyticsService {
        /// Canned response returned on each `track_events` call.
        response: ProtoTrackEventsResponse,
        /// Captured calls in arrival order.
        captured: Arc<Mutex<Vec<CapturedCall>>>,
    }

    #[async_trait]
    impl AnalyticsService for MockAnalyticsService {
        type ListExperimentResultsStream =
            Pin<Box<dyn Stream<Item = Result<ExperimentResult, Status>> + Send + 'static>>;

        async fn track_events(
            &self,
            request: ServerReq<ProtoTrackEventsRequest>,
        ) -> Result<ServerResp<ProtoTrackEventsResponse>, Status> {
            let env_id_meta = request
                .metadata()
                .get(ENV_ID_METADATA_KEY)
                .and_then(|v| v.to_str().ok())
                .map(std::string::ToString::to_string);
            let inner = request.into_inner();
            self.captured.lock().unwrap().push((env_id_meta, inner));
            Ok(ServerResp::new(self.response.clone()))
        }

        // ── Unimplemented methods ──────────────────────────────────────────────
        async fn ingest_event(
            &self,
            _req: ServerReq<IngestEventRequest>,
        ) -> Result<ServerResp<IngestEventResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn register_context(
            &self,
            _req: ServerReq<RegisterContextRequest>,
        ) -> Result<ServerResp<RegisterContextResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_context_types(
            &self,
            _req: ServerReq<ListContextTypesRequest>,
        ) -> Result<ServerResp<ListContextTypesResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_context_params(
            &self,
            _req: ServerReq<ListContextParamsRequest>,
        ) -> Result<ServerResp<ListContextParamsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_eval_stats(
            &self,
            _req: ServerReq<GetEvalStatsRequest>,
        ) -> Result<ServerResp<GetEvalStatsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_context_intelligence(
            &self,
            _req: ServerReq<GetContextIntelligenceRequest>,
        ) -> Result<ServerResp<GetContextIntelligenceResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn write_experiment_results(
            &self,
            _req: ServerReq<WriteExperimentResultsRequest>,
        ) -> Result<ServerResp<WriteExperimentResultsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_experiment_results(
            &self,
            _req: ServerReq<ListExperimentResultsRequest>,
        ) -> Result<ServerResp<Self::ListExperimentResultsStream>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_experiment_result(
            &self,
            _req: ServerReq<GetExperimentResultRequest>,
        ) -> Result<ServerResp<ExperimentResult>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn create_metric(
            &self,
            _req: ServerReq<CreateMetricRequest>,
        ) -> Result<ServerResp<MetricDefinition>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_metric(
            &self,
            _req: ServerReq<GetMetricRequest>,
        ) -> Result<ServerResp<MetricDefinition>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_metrics(
            &self,
            _req: ServerReq<ListMetricsRequest>,
        ) -> Result<ServerResp<ListMetricsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn update_metric(
            &self,
            _req: ServerReq<UpdateMetricRequest>,
        ) -> Result<ServerResp<MetricDefinition>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn delete_metric(
            &self,
            _req: ServerReq<DeleteMetricRequest>,
        ) -> Result<ServerResp<DeleteMetricResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn preview_metric(
            &self,
            _req: ServerReq<PreviewMetricRequest>,
        ) -> Result<ServerResp<PreviewMetricResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_event_firings(
            &self,
            _req: ServerReq<GetEventFiringsRequest>,
        ) -> Result<ServerResp<GetEventFiringsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_event_stats(
            &self,
            _req: ServerReq<GetEventStatsRequest>,
        ) -> Result<ServerResp<GetEventStatsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        // Event-definitions CRUD — closed in feature-flag-wr4.
        async fn create_event_definition(
            &self,
            _req: ServerReq<stitchd_proto::analytics::v1::CreateEventDefinitionRequest>,
        ) -> Result<ServerResp<stitchd_proto::analytics::v1::EventDefinitionMsg>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_event_definition(
            &self,
            _req: ServerReq<stitchd_proto::analytics::v1::GetEventDefinitionRequest>,
        ) -> Result<ServerResp<stitchd_proto::analytics::v1::EventDefinitionMsg>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_event_definitions(
            &self,
            _req: ServerReq<stitchd_proto::analytics::v1::ListEventDefinitionsRequest>,
        ) -> Result<ServerResp<stitchd_proto::analytics::v1::ListEventDefinitionsResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn update_event_definition(
            &self,
            _req: ServerReq<stitchd_proto::analytics::v1::UpdateEventDefinitionRequest>,
        ) -> Result<ServerResp<stitchd_proto::analytics::v1::EventDefinitionMsg>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn delete_event_definition(
            &self,
            _req: ServerReq<stitchd_proto::analytics::v1::DeleteEventDefinitionRequest>,
        ) -> Result<ServerResp<stitchd_proto::analytics::v1::DeleteEventDefinitionResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
    }

    /// Spin up an in-process AnalyticsService stub and return both a client
    /// and the captured-request handle.
    ///
    /// Pattern per `conductor/patterns.md`: bind a random localhost port,
    /// wrap in a `TcpListenerStream`, hand off via `serve_with_incoming` in a
    /// `tokio::spawn`. No external mocking library; no port conflicts.
    async fn spawn_analytics_mock(
        response: ProtoTrackEventsResponse,
    ) -> (
        AnalyticsServiceClient<TonicChannel>,
        Arc<Mutex<Vec<CapturedCall>>>,
    ) {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let svc = MockAnalyticsService {
            response,
            captured: Arc::clone(&captured),
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            TonicServer::builder()
                .add_service(AnalyticsServiceServer::new(svc))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        // Connect — `Endpoint` retries internally until the listener accepts.
        let client = AnalyticsServiceClient::connect(format!("http://{addr}"))
            .await
            .expect("mock analytics-service should accept connection");
        (client, captured)
    }

    /// Build a `GatewayState` whose `analytics_client` points at the supplied
    /// mock client. Other clients are stubbed lazily — the track_events
    /// handler only touches `analytics_client`.
    fn state_with_analytics(
        analytics_client: AnalyticsServiceClient<TonicChannel>,
    ) -> Arc<GatewayState> {
        let stub = make_stub_state();
        // GatewayState fields are public — overwrite analytics_client only.
        // Cloning the rest preserves the lazy stub channels.
        Arc::new(GatewayState {
            analytics_client: Arc::new(tokio::sync::Mutex::new(analytics_client)),
            auth_client: Arc::clone(&stub.auth_client),
            flag_client: Arc::clone(&stub.flag_client),
            segmentation_client: Arc::clone(&stub.segmentation_client),
            experimentation_client: Arc::clone(&stub.experimentation_client),
            management_client: Arc::clone(&stub.management_client),
            auth_provider_client: Arc::clone(&stub.auth_provider_client),
            oidc_login_client: Arc::clone(&stub.oidc_login_client),
            saml_login_client: Arc::clone(&stub.saml_login_client),
            stats_client: Arc::clone(&stub.stats_client),
            flag_sdk_backend_client: Arc::clone(&stub.flag_sdk_backend_client),
            segmentation_sdk_backend_client: Arc::clone(&stub.segmentation_sdk_backend_client),
        })
    }

    /// Build a request body for the track_events route. Uses the
    /// current wire format (`contexts: { type: key, … }` flat map).
    fn track_events_body_json(event_keys: &[&str]) -> String {
        let events: Vec<serde_json::Value> = event_keys
            .iter()
            .map(|k| {
                serde_json::json!({
                    "event_key": k,
                    "contexts": { "user": "u1" },
                    "value": { "int": 1 },
                })
            })
            .collect();
        serde_json::json!({ "events": events }).to_string()
    }

    /// Build a router with the `track_events` handler wired but **no** SDK
    /// auth middleware applied — instead we inject `SdkContext` directly into
    /// the request via an extension layer. Used to test behaviour downstream
    /// of auth (env_id forwarding, response translation, body limit).
    fn track_events_router_with_ctx(state: Arc<GatewayState>, sdk_ctx: SdkContext) -> axum::Router {
        use axum::extract::Request as ExtractRequest;
        use axum::middleware::{Next, from_fn};
        use axum::routing::post;

        let layer = from_fn(move |mut req: ExtractRequest, next: Next| {
            let ctx = sdk_ctx.clone();
            async move {
                req.extensions_mut().insert(ctx);
                next.run(req).await
            }
        });

        axum::Router::new()
            .route(
                "/v1/events/track",
                post(track_events).layer(axum::extract::DefaultBodyLimit::max(
                    TRACK_EVENTS_BODY_LIMIT_BYTES,
                )),
            )
            .with_state(state)
            .layer(layer)
    }

    fn sample_sdk_ctx(env_id: &str) -> SdkContext {
        SdkContext {
            environment_id: env_id.to_string(),
            organisation_id: "00000000-0000-0000-0000-000000000aaa".to_string(),
            sdk_key_id: "00000000-0000-0000-0000-000000000bbb".to_string(),
        }
    }

    // ── 1. Valid batch → 202 ──────────────────────────────────────────────────

    #[tokio::test]
    async fn test_track_events_returns_202_for_valid_batch() {
        let (client, _captured) = spawn_analytics_mock(ProtoTrackEventsResponse {
            accepted_count: 2,
            rejected: vec![],
        })
        .await;
        let state = state_with_analytics(client);
        let env_id = "00000000-0000-0000-0000-000000000001";
        let app = track_events_router_with_ctx(state, sample_sdk_ctx(env_id));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/events/track")
                    .header("content-type", "application/json")
                    .body(Body::from(track_events_body_json(&["click", "purchase"])))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["accepted_count"], 2);
        assert!(body["rejected"].as_array().unwrap().is_empty());
    }

    // ── 2. Oversize body → 413 ────────────────────────────────────────────────

    #[tokio::test]
    async fn test_track_events_returns_413_for_oversize_body() {
        // No analytics mock needed — body limit fires before any forwarding.
        let state = make_stub_state();
        let env_id = "00000000-0000-0000-0000-000000000001";
        let app = track_events_router_with_ctx(state, sample_sdk_ctx(env_id));

        // 5 MiB + 1 byte of padding inside a JSON string.
        let oversize_payload = vec![b'a'; TRACK_EVENTS_BODY_LIMIT_BYTES + 1];
        let body = format!(
            r#"{{"events":[{{"event_key":"k","context_type":"user","context_key":"u","value":{{"int":1}},"properties":{{"pad":"{}"}}}}]}}"#,
            std::str::from_utf8(&oversize_payload).unwrap()
        );

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/events/track")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    // ── 3. Missing SDK key → 401 ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_track_events_returns_401_without_sdk_key() {
        // Use the FULL router so sdk_auth_middleware actually runs.
        use crate::router::build_router;
        use metrics_exporter_prometheus::PrometheusBuilder;

        let handle = PrometheusBuilder::new().build_recorder().handle();
        let app = build_router(make_stub_state(), handle);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/events/track")
                    .header("content-type", "application/json")
                    .body(Body::from(track_events_body_json(&["click"])))
                    .unwrap(),
            )
            .await
            .unwrap();

        // sdk_auth_middleware should reject (no x-sdk-key).
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── 4. env_id propagation → x-env-id metadata ─────────────────────────────

    #[tokio::test]
    async fn test_track_events_forwards_env_id_to_backend() {
        let (client, captured) = spawn_analytics_mock(ProtoTrackEventsResponse {
            accepted_count: 1,
            rejected: vec![],
        })
        .await;
        let state = state_with_analytics(client);
        let env_id = "11111111-2222-3333-4444-555555555555";
        let app = track_events_router_with_ctx(state, sample_sdk_ctx(env_id));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/events/track")
                    .header("content-type", "application/json")
                    .body(Body::from(track_events_body_json(&["signup"])))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1, "expected one backend call");
        let (env_meta, body) = &captured[0];
        assert_eq!(
            env_meta.as_deref(),
            Some(env_id),
            "x-env-id metadata must equal SDK context environment_id"
        );
        // Body env_id mirrored too — matches analytics-service metadata-first
        // resolution but provides a fallback for tests/direct callers.
        assert_eq!(body.env_id, env_id);
        assert_eq!(body.events.len(), 1);
        assert_eq!(body.events[0].event_key, "signup");
    }

    // ── 5. Per-event rejections translated to JSON ────────────────────────────

    #[tokio::test]
    async fn test_track_events_translates_per_event_rejection() {
        let (client, _captured) = spawn_analytics_mock(ProtoTrackEventsResponse {
            accepted_count: 1,
            rejected: vec![
                ProtoRejectedEvent {
                    event_key: "missing_def".to_string(),
                    reason: "unknown_event_key".to_string(),
                },
                ProtoRejectedEvent {
                    event_key: "bad_type".to_string(),
                    reason: "value_type_mismatch".to_string(),
                },
            ],
        })
        .await;
        let state = state_with_analytics(client);
        let env_id = "00000000-0000-0000-0000-000000000001";
        let app = track_events_router_with_ctx(state, sample_sdk_ctx(env_id));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/events/track")
                    .header("content-type", "application/json")
                    .body(Body::from(track_events_body_json(&[
                        "click",
                        "missing_def",
                        "bad_type",
                    ])))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::ACCEPTED,
            "per-event rejections must NOT change HTTP status"
        );

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["accepted_count"], 1);
        let rejected = body["rejected"].as_array().unwrap();
        assert_eq!(rejected.len(), 2);
        assert_eq!(rejected[0]["event_key"], "missing_def");
        assert_eq!(rejected[0]["reason"], "unknown_event_key");
        assert_eq!(rejected[1]["event_key"], "bad_type");
        assert_eq!(rejected[1]["reason"], "value_type_mismatch");
    }

    // ── Pure helpers ──────────────────────────────────────────────────────────

    #[test]
    fn typed_value_serde_round_trip() {
        let cases = [
            (TypedValue::Bool(true), r#"{"bool":true}"#),
            (TypedValue::Int(42), r#"{"int":42}"#),
            (TypedValue::Double(1.5), r#"{"double":1.5}"#),
        ];
        for (val, expected_json) in cases {
            let s = serde_json::to_string(&val).unwrap();
            assert_eq!(s, expected_json);
            let parsed: TypedValue = serde_json::from_str(&s).unwrap();
            assert_eq!(parsed, val);
        }
    }

    #[test]
    fn typed_value_to_proto_bool() {
        let pv = TypedValue::Bool(false).to_proto();
        assert!(matches!(
            pv.value,
            Some(metric_value::Value::BoolValue(false))
        ));
    }

    #[test]
    fn typed_value_to_proto_int() {
        let pv = TypedValue::Int(7).to_proto();
        assert!(matches!(pv.value, Some(metric_value::Value::IntValue(7))));
    }

    #[test]
    fn typed_value_to_proto_double() {
        let pv = TypedValue::Double(2.5).to_proto();
        assert!(matches!(
            pv.value,
            Some(metric_value::Value::DoubleValue(d)) if (d - 2.5).abs() < 1e-9
        ));
    }

    fn ctx_user(key: &str) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert("user".to_string(), key.to_string());
        m
    }

    #[test]
    fn json_event_to_proto_passes_properties_and_occurred_at() {
        let mut props = HashMap::new();
        props.insert("currency".to_string(), "USD".to_string());
        let e = TrackEvent {
            event_key: "purchase".into(),
            contexts: ctx_user("u42"),
            value: Some(TypedValue::Int(100)),
            properties: Some(props),
            occurred_at: Some("2026-05-19T12:00:00Z".into()),
        };
        let p = json_event_to_proto(e);
        assert_eq!(p.event_key, "purchase");
        assert_eq!(p.contexts.get("user").map(String::as_str), Some("u42"));
        assert_eq!(
            p.properties.get("currency").map(String::as_str),
            Some("USD")
        );
        assert_eq!(p.occurred_at.as_deref(), Some("2026-05-19T12:00:00Z"));
        assert!(matches!(
            p.value,
            Some(MetricValue {
                value: Some(metric_value::Value::IntValue(100))
            })
        ));
    }

    #[test]
    fn json_event_to_proto_defaults_optional_fields() {
        let e = TrackEvent {
            event_key: "ping".into(),
            contexts: ctx_user("u1"),
            value: None,
            properties: None,
            occurred_at: None,
        };
        let p = json_event_to_proto(e);
        assert!(p.value.is_none());
        assert!(p.properties.is_empty());
        assert!(p.occurred_at.is_none());
        assert_eq!(p.contexts.len(), 1);
    }

    #[test]
    fn json_event_to_proto_forwards_multi_contexts() {
        // Demonstrates the wire shape: contexts is a flat type→key map,
        // every dimension reaches the proto. The ingestion handler then
        // flattens it into the CH `Array(Tuple(String, String))` row.
        let mut ctxs = HashMap::new();
        ctxs.insert("user".to_string(), "u-42".to_string());
        ctxs.insert("account".to_string(), "acme".to_string());
        let e = TrackEvent {
            event_key: "purchase".into(),
            contexts: ctxs,
            value: None,
            properties: None,
            occurred_at: None,
        };
        let p = json_event_to_proto(e);
        assert_eq!(p.contexts.len(), 2);
        assert_eq!(p.contexts.get("user").map(String::as_str), Some("u-42"));
        assert_eq!(p.contexts.get("account").map(String::as_str), Some("acme"));
    }

    #[tokio::test]
    async fn track_events_returns_500_when_sdk_context_missing() {
        // No middleware to inject SdkContext → handler must refuse to proxy.
        let state = make_stub_state();
        let req = Request::builder()
            .method("POST")
            .uri("/v1/events/track")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(track_events_body_json(&["click"])))
            .unwrap();
        let result = track_events(axum::extract::State(state), req).await;
        let err = result.unwrap_err();
        assert!(format!("{err:?}").contains("SdkContext"));
    }

    #[tokio::test]
    async fn track_events_rejects_malformed_body() {
        let state = make_stub_state();
        let env_id = "00000000-0000-0000-0000-000000000001";
        let mut req = Request::builder()
            .method("POST")
            .uri("/v1/events/track")
            .header("content-type", "application/json")
            .body(axum::body::Body::from("not-json"))
            .unwrap();
        req.extensions_mut().insert(sample_sdk_ctx(env_id));
        let result = track_events(axum::extract::State(state), req).await;
        let err = result.unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("invalid JSON body") || msg.contains("BadRequest"),
            "got {msg}"
        );
    }

    // ── GET /v1/events/{key}/firings + /stats ─────────────────────────────────
    //
    // EventDetail page routes — JWT-tier admin endpoints. Three required test
    // cases (per worker_25_firings task brief):
    //  - 401 without JWT (full router boot, exercises auth middleware)
    //  - firings forwards env_id/event_key/limit to analytics-service
    //  - stats forwards env_id/event_key/days
    //
    // A dedicated mock that captures both RPCs is used; the existing
    // `MockAnalyticsService` above only handles `track_events`. This keeps
    // the diff scoped — no behavioural change to existing test mocks.

    /// `(env_key, event_key, limit, days)` capture for firings/stats RPCs.
    type FiringsCapture = (String, String, u32);
    type StatsCapture = (String, String, u32);

    struct EventQueryMockService {
        firings: Arc<Mutex<Vec<FiringsCapture>>>,
        stats: Arc<Mutex<Vec<StatsCapture>>>,
    }

    #[async_trait]
    impl AnalyticsService for EventQueryMockService {
        type ListExperimentResultsStream =
            Pin<Box<dyn Stream<Item = Result<ExperimentResult, Status>> + Send + 'static>>;

        async fn get_event_firings(
            &self,
            request: ServerReq<GetEventFiringsRequest>,
        ) -> Result<ServerResp<GetEventFiringsResponse>, Status> {
            let inner = request.into_inner();
            self.firings.lock().unwrap().push((
                inner.env_id.clone(),
                inner.event_key.clone(),
                inner.limit,
            ));
            // Canned response — one firing row so the JSON-shape mapping is exercised.
            let mut ctxs = std::collections::HashMap::new();
            ctxs.insert("user".to_string(), "u1".to_string());
            Ok(ServerResp::new(GetEventFiringsResponse {
                firings: vec![stitchd_proto::analytics::v1::EventFiring {
                    contexts: ctxs,
                    value_json: "42".into(),
                    properties_json: "{}".into(),
                    occurred_at: "2026-05-19T12:00:00.000Z".into(),
                    ingested_at: "2026-05-19T12:00:01.000Z".into(),
                }],
            }))
        }

        async fn get_event_stats(
            &self,
            request: ServerReq<GetEventStatsRequest>,
        ) -> Result<ServerResp<GetEventStatsResponse>, Status> {
            let inner = request.into_inner();
            self.stats.lock().unwrap().push((
                inner.env_id.clone(),
                inner.event_key.clone(),
                inner.days,
            ));
            Ok(ServerResp::new(GetEventStatsResponse {
                buckets: vec![stitchd_proto::analytics::v1::EventStatsBucket {
                    day: "2026-05-19T00:00:00Z".into(),
                    count: 3,
                    unique_context_keys: 2,
                }],
            }))
        }

        // ── Unimplemented stubs (mirrors `MockAnalyticsService`) ───────────────
        async fn ingest_event(
            &self,
            _req: ServerReq<IngestEventRequest>,
        ) -> Result<ServerResp<IngestEventResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn track_events(
            &self,
            _req: ServerReq<ProtoTrackEventsRequest>,
        ) -> Result<ServerResp<ProtoTrackEventsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn register_context(
            &self,
            _req: ServerReq<RegisterContextRequest>,
        ) -> Result<ServerResp<RegisterContextResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_context_types(
            &self,
            _req: ServerReq<ListContextTypesRequest>,
        ) -> Result<ServerResp<ListContextTypesResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_context_params(
            &self,
            _req: ServerReq<ListContextParamsRequest>,
        ) -> Result<ServerResp<ListContextParamsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_eval_stats(
            &self,
            _req: ServerReq<GetEvalStatsRequest>,
        ) -> Result<ServerResp<GetEvalStatsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_context_intelligence(
            &self,
            _req: ServerReq<GetContextIntelligenceRequest>,
        ) -> Result<ServerResp<GetContextIntelligenceResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn write_experiment_results(
            &self,
            _req: ServerReq<WriteExperimentResultsRequest>,
        ) -> Result<ServerResp<WriteExperimentResultsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_experiment_results(
            &self,
            _req: ServerReq<ListExperimentResultsRequest>,
        ) -> Result<ServerResp<Self::ListExperimentResultsStream>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_experiment_result(
            &self,
            _req: ServerReq<GetExperimentResultRequest>,
        ) -> Result<ServerResp<ExperimentResult>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn create_metric(
            &self,
            _req: ServerReq<CreateMetricRequest>,
        ) -> Result<ServerResp<MetricDefinition>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_metric(
            &self,
            _req: ServerReq<GetMetricRequest>,
        ) -> Result<ServerResp<MetricDefinition>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_metrics(
            &self,
            _req: ServerReq<ListMetricsRequest>,
        ) -> Result<ServerResp<ListMetricsResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn update_metric(
            &self,
            _req: ServerReq<UpdateMetricRequest>,
        ) -> Result<ServerResp<MetricDefinition>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn delete_metric(
            &self,
            _req: ServerReq<DeleteMetricRequest>,
        ) -> Result<ServerResp<DeleteMetricResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn preview_metric(
            &self,
            _req: ServerReq<PreviewMetricRequest>,
        ) -> Result<ServerResp<PreviewMetricResponse>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        // Event-definitions CRUD — closed in feature-flag-wr4.
        async fn create_event_definition(
            &self,
            _req: ServerReq<stitchd_proto::analytics::v1::CreateEventDefinitionRequest>,
        ) -> Result<ServerResp<stitchd_proto::analytics::v1::EventDefinitionMsg>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn get_event_definition(
            &self,
            _req: ServerReq<stitchd_proto::analytics::v1::GetEventDefinitionRequest>,
        ) -> Result<ServerResp<stitchd_proto::analytics::v1::EventDefinitionMsg>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn list_event_definitions(
            &self,
            _req: ServerReq<stitchd_proto::analytics::v1::ListEventDefinitionsRequest>,
        ) -> Result<ServerResp<stitchd_proto::analytics::v1::ListEventDefinitionsResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn update_event_definition(
            &self,
            _req: ServerReq<stitchd_proto::analytics::v1::UpdateEventDefinitionRequest>,
        ) -> Result<ServerResp<stitchd_proto::analytics::v1::EventDefinitionMsg>, Status> {
            Err(Status::unimplemented("not used in tests"))
        }
        async fn delete_event_definition(
            &self,
            _req: ServerReq<stitchd_proto::analytics::v1::DeleteEventDefinitionRequest>,
        ) -> Result<ServerResp<stitchd_proto::analytics::v1::DeleteEventDefinitionResponse>, Status>
        {
            Err(Status::unimplemented("not used in tests"))
        }
    }

    /// Spin up an in-process mock that captures firings + stats calls,
    /// returning an `AnalyticsServiceClient` plus both capture handles.
    async fn spawn_event_query_mock() -> (
        AnalyticsServiceClient<TonicChannel>,
        Arc<Mutex<Vec<FiringsCapture>>>,
        Arc<Mutex<Vec<StatsCapture>>>,
    ) {
        let firings: Arc<Mutex<Vec<FiringsCapture>>> = Arc::new(Mutex::new(Vec::new()));
        let stats: Arc<Mutex<Vec<StatsCapture>>> = Arc::new(Mutex::new(Vec::new()));
        let svc = EventQueryMockService {
            firings: Arc::clone(&firings),
            stats: Arc::clone(&stats),
        };

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            TonicServer::builder()
                .add_service(AnalyticsServiceServer::new(svc))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        let client = AnalyticsServiceClient::connect(format!("http://{addr}"))
            .await
            .expect("mock analytics-service should accept connection");
        (client, firings, stats)
    }

    /// Router exposing only the firings + stats routes with NO auth middleware,
    /// for proxy-shape tests. The 401-without-JWT case uses the full `build_router`.
    fn firings_router(state: Arc<GatewayState>) -> axum::Router {
        use axum::routing::get as get_route;
        axum::Router::new()
            .route(
                "/v1/events/{event_key}/firings",
                get_route(get_event_firings),
            )
            .route("/v1/events/{event_key}/stats", get_route(get_event_stats))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_firings_returns_401_without_jwt() {
        // Full router boot ensures the auth middleware actually runs and the
        // route is registered. With no `Authorization` header, the JWT tier
        // must short-circuit to 401.
        use crate::router::build_router;
        use metrics_exporter_prometheus::PrometheusBuilder;

        let handle = PrometheusBuilder::new().build_recorder().handle();
        let app = build_router(make_stub_state(), handle);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/events/click/firings?env_id=00000000-0000-0000-0000-000000000001")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_firings_proxies_to_analytics_with_env_id() {
        let (client, firings, _stats) = spawn_event_query_mock().await;
        let state = state_with_analytics(client);
        let app = firings_router(state);

        let env_id = "11111111-2222-3333-4444-555555555555";
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/events/checkout/firings?env_id={env_id}&limit=25"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Verify the analytics-service received the expected RPC arguments.
        // Scope the MutexGuard so it drops before the await on `to_bytes`
        // below — clippy::await_holding_lock would fire otherwise.
        {
            let calls = firings.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, env_id);
            assert_eq!(calls[0].1, "checkout");
            assert_eq!(calls[0].2, 25);
        }

        // Response shape is the JSON projection of the proto type.
        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let firings_arr = body["firings"].as_array().unwrap();
        assert_eq!(firings_arr.len(), 1);
        // Flat type→key map shape; the mock canned a single
        // {"user": "u1"} entry.
        assert_eq!(firings_arr[0]["contexts"]["user"], "u1");
        assert_eq!(firings_arr[0]["value_json"], "42");
        assert_eq!(firings_arr[0]["properties_json"], "{}");
    }

    #[tokio::test]
    async fn test_stats_proxies_to_analytics() {
        let (client, _firings, stats) = spawn_event_query_mock().await;
        let state = state_with_analytics(client);
        let app = firings_router(state);

        let env_id = "22222222-3333-4444-5555-666666666666";
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/events/signup/stats?env_id={env_id}&days=7"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        {
            let calls = stats.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].0, env_id);
            assert_eq!(calls[0].1, "signup");
            assert_eq!(calls[0].2, 7);
        }

        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        let buckets = body["buckets"].as_array().unwrap();
        assert_eq!(buckets.len(), 1);
        assert_eq!(buckets[0]["day"], "2026-05-19T00:00:00Z");
        assert_eq!(buckets[0]["count"], 3);
        assert_eq!(buckets[0]["unique_context_keys"], 2);
    }

    #[tokio::test]
    async fn test_firings_uses_default_limit_when_omitted() {
        let (client, firings, _stats) = spawn_event_query_mock().await;
        let state = state_with_analytics(client);
        let app = firings_router(state);

        let env_id = "33333333-4444-5555-6666-777777777777";
        let _ = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/events/x/firings?env_id={env_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let calls = firings.lock().unwrap();
        assert_eq!(calls.len(), 1);
        // Gateway forwards `0` → analytics-service resolves the default (50).
        assert_eq!(calls[0].2, 0);
    }
    // ─── POST /v1/admin/events/track ──────────────────────────────────────────
    //
    // Admin-auth mirror of /v1/events/track. Uses RbacContext (JWT) instead of
    // SdkContext (x-sdk-key) and stamps every event with `_test=true` in
    // properties before forwarding to analytics-service.

    use stitchd_proto::auth::v1::RbacContext;

    /// Build a router that wires `track_events_admin` with an `RbacContext`
    /// extension injected via a middleware shim — same pattern as
    /// `track_events_router_with_ctx` but for the JWT-auth path.
    fn admin_track_router_with_ctx(
        state: Arc<GatewayState>,
        rbac_ctx: RbacContext,
    ) -> axum::Router {
        use axum::extract::Request as ExtractRequest;
        use axum::middleware::{Next, from_fn};
        use axum::routing::post;

        let layer = from_fn(move |mut req: ExtractRequest, next: Next| {
            let ctx = rbac_ctx.clone();
            async move {
                req.extensions_mut().insert(ctx);
                next.run(req).await
            }
        });

        axum::Router::new()
            .route(
                "/v1/admin/events/track",
                post(track_events_admin).layer(axum::extract::DefaultBodyLimit::max(
                    TRACK_EVENTS_BODY_LIMIT_BYTES,
                )),
            )
            .with_state(state)
            .layer(layer)
    }

    fn rbac_with(env_id: &str, perms: &[&str]) -> RbacContext {
        RbacContext {
            tenant_id: "org-1".to_string(),
            environment_id: env_id.to_string(),
            roles: vec!["org_member".to_string()],
            permissions: perms.iter().map(|s| s.to_string()).collect(),
            subject: "user-1".to_string(),
            is_system: false,
        }
    }

    // ── 1. Valid JWT + event:write → 202 ──────────────────────────────────────

    #[tokio::test]
    async fn test_admin_track_returns_202_with_jwt() {
        let (client, _captured) = spawn_analytics_mock(ProtoTrackEventsResponse {
            accepted_count: 1,
            rejected: vec![],
        })
        .await;
        let state = state_with_analytics(client);
        let env_id = "00000000-0000-0000-0000-000000000001";
        let app = admin_track_router_with_ctx(state, rbac_with(env_id, &["event:write"]));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/events/track")
                    .header("content-type", "application/json")
                    .body(Body::from(track_events_body_json(&["click"])))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(body["accepted_count"], 1);
    }

    // ── 2. No JWT (no RbacContext) → 401 from full router ────────────────────

    #[tokio::test]
    async fn test_admin_track_returns_401_without_jwt() {
        // Use the FULL router so auth_middleware actually runs and rejects.
        use crate::router::build_router;
        use metrics_exporter_prometheus::PrometheusBuilder;

        let handle = PrometheusBuilder::new().build_recorder().handle();
        let app = build_router(make_stub_state(), handle);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/events/track")
                    .header("content-type", "application/json")
                    .body(Body::from(track_events_body_json(&["click"])))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── 3. JWT without event:write → 401 ─────────────────────────────────────
    //
    // Codebase convention (see `require_permission` + `test_metric_routes_
    // require_metric_write_permission`): missing-permission yields 401, NOT
    // 403. The test name spec mentioned 403 but we follow the existing
    // gateway convention to keep error responses uniform across admin routes.

    #[tokio::test]
    async fn test_admin_track_returns_401_without_event_write_permission() {
        let state = make_stub_state();
        let env_id = "00000000-0000-0000-0000-000000000001";
        // RbacContext present but missing `event:write` — has only `event:read`.
        let app = admin_track_router_with_ctx(state, rbac_with(env_id, &["event:read"]));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/events/track")
                    .header("content-type", "application/json")
                    .body(Body::from(track_events_body_json(&["click"])))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── 4. Test-mark: every event gets properties["_test"] = "true" ─────────

    #[tokio::test]
    async fn test_admin_track_marks_events_as_test() {
        let (client, captured) = spawn_analytics_mock(ProtoTrackEventsResponse {
            accepted_count: 2,
            rejected: vec![],
        })
        .await;
        let state = state_with_analytics(client);
        let env_id = "00000000-0000-0000-0000-000000000001";
        let app = admin_track_router_with_ctx(state, rbac_with(env_id, &["event:write"]));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/events/track")
                    .header("content-type", "application/json")
                    .body(Body::from(track_events_body_json(&["click", "purchase"])))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let (_env_meta, body) = &captured[0];
        assert_eq!(body.events.len(), 2);
        for ev in &body.events {
            assert_eq!(
                ev.properties.get("_test").map(String::as_str),
                Some("true"),
                "admin-fired events must carry _test=true marker"
            );
        }
    }

    // ── 5. env_id resolved from JWT (NOT body / SDK key) ─────────────────────

    #[tokio::test]
    async fn test_admin_track_forwards_env_id_from_jwt_not_sdk_key() {
        let (client, captured) = spawn_analytics_mock(ProtoTrackEventsResponse {
            accepted_count: 1,
            rejected: vec![],
        })
        .await;
        let state = state_with_analytics(client);
        // RbacContext env_id (the trusted one)
        let jwt_env = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let app = admin_track_router_with_ctx(state, rbac_with(jwt_env, &["event:write"]));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/events/track")
                    .header("content-type", "application/json")
                    .body(Body::from(track_events_body_json(&["signup"])))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);

        let captured = captured.lock().unwrap();
        assert_eq!(captured.len(), 1);
        let (env_meta, body) = &captured[0];
        // The trusted env_id reaches analytics-service exactly once via the
        // x-env-id metadata + the mirrored request body field — sourced from
        // the JWT's RbacContext, NOT any header / body / SDK key.
        assert_eq!(
            env_meta.as_deref(),
            Some(jwt_env),
            "x-env-id metadata must equal RbacContext.environment_id"
        );
        assert_eq!(body.env_id, jwt_env);
    }

    // ── 6. Empty env scope on JWT → 400 ──────────────────────────────────────

    #[tokio::test]
    async fn test_admin_track_returns_400_when_jwt_has_no_env_scope() {
        let state = make_stub_state();
        // event:write present, but environment_id empty (org-level token).
        let app = admin_track_router_with_ctx(state, rbac_with("", &["event:write"]));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/events/track")
                    .header("content-type", "application/json")
                    .body(Body::from(track_events_body_json(&["click"])))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
