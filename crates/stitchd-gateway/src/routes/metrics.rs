//! Metric route handlers — admin CRUD + preview for environment-scoped
//! [`stitchd_core::metric::MetricDefinition`]s.
//!
//! All routes live under the JWT-auth tree:
//! * `metric:read` for GET (list / get) and preview reads
//! * `metric:write` for POST / PATCH / DELETE
//!
//! # Wire format
//! The response shape is the domain [`MetricDefinition`] serialized directly
//! (avoids a parallel `AdminMetricJson` DTO — metrics are admin-only and the
//! core serde tags already match the proto + JSON wire format).
//!
//! Request bodies are local types that mirror the proto request messages
//! plus a `kind`-discriminated config field, then we translate JSON → proto
//! request → gRPC → proto response → domain type → JSON in this module.
//!
//! # Optimistic locking
//! PATCH carries an `expected_version` field; the analytics-service returns
//! `Status::aborted` on a mismatch. We map that to HTTP `409 Conflict`
//! locally in this module (rather than extending the crate-wide
//! `From<tonic::Status>` mapping — keeps the side-effect scoped).

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_core::{
    id::{EnvironmentId, MetricId},
    metric::{
        AggregationConfig, AggregationOperator, FunnelConfig, FunnelStep, GoalDirection,
        MetricDefinition, MetricKind, RatioConfig,
    },
};

use stitchd_proto::analytics::v1::{
    AggregationConfig as ProtoAggregationConfig, CreateMetricRequest, DeleteMetricRequest,
    FunnelConfig as ProtoFunnelConfig, FunnelStep as ProtoFunnelStep, GetMetricRequest,
    ListMetricsRequest, MetricDefinition as ProtoMetric, PreviewMetricRequest,
    RatioConfig as ProtoRatioConfig, UpdateMetricRequest, create_metric_request, metric_definition,
    preview_metric_request, update_metric_request,
};

use crate::error::GatewayError;
use crate::state::GatewayState;

use super::require_permission;

// ─── REST request DTOs ────────────────────────────────────────────────────────

/// Query parameters for `GET /v1/metrics`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListMetricsQuery {
    /// Environment UUID — required.
    pub env_id: String,
    /// 0-based offset; defaults to 0 when unset.
    pub offset: Option<u64>,
    /// Page size; defaults to 50 when unset, capped at 200 server-side.
    pub limit: Option<u64>,
    /// Optional kind filter (`aggregation` | `ratio` | `funnel`).
    pub kind: Option<String>,
    /// Optional event-key filter — restricts results to metrics that
    /// directly reference this event (aggregation `event_key` or any
    /// funnel `steps[].event_key`). Used by the EventDetail page's
    /// "Metrics referencing this event" back-link.
    pub event_key: Option<String>,
}

/// Wire-format aggregation config — matches the proto `AggregationConfig`
/// plus accepts `where_clause` as a JSON value (we stringify it before
/// passing to gRPC, matching the analytics-service contract).
#[derive(Debug, Deserialize, ToSchema)]
pub struct AggregationConfigBody {
    pub event_key: String,
    /// `count` | `sum` | `avg` | `p50` | `p90` | `p99` | `uniq`
    pub aggregator: String,
    #[serde(default)]
    pub on_field: Option<String>,
    /// JsonLogic where-clause — serialized as JSON before passing to gRPC.
    #[serde(default)]
    #[schema(value_type = Object, nullable = true)]
    pub where_clause: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct RatioConfigBody {
    pub numerator_metric_id: String,
    pub denominator_metric_id: String,
    #[serde(default)]
    pub min_denominator: i64,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct FunnelStepBody {
    pub event_key: String,
    #[serde(default)]
    #[schema(value_type = Object, nullable = true)]
    pub where_clause: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct FunnelConfigBody {
    pub steps: Vec<FunnelStepBody>,
    pub window_seconds: i64,
    #[serde(default)]
    pub count_repeats: bool,
}

/// Kind-discriminated config — mirrors the wire shape used by the core
/// [`MetricKind`] (tag = `kind`, snake_case).
#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MetricKindBody {
    Aggregation(AggregationConfigBody),
    Ratio(RatioConfigBody),
    Funnel(FunnelConfigBody),
}

/// `POST /v1/metrics` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateMetricBody {
    pub environment_id: String,
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Kind-specific config (flattened tag `kind`).
    #[serde(flatten)]
    pub kind: MetricKindBody,
    /// `increase` | `decrease` | `neutral`.
    pub goal_direction: String,
}

/// `PATCH /v1/metrics/{id}` request body.
///
/// `expected_version` is required for optimistic locking — a mismatch is
/// returned as `409 Conflict`. All other fields are optional — omit `kind`
/// to keep the existing metric kind unchanged.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateMetricBody {
    pub expected_version: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Optional kind update — omit to preserve existing kind.
    #[serde(flatten, default)]
    pub kind: Option<MetricKindBody>,
    #[serde(default)]
    pub goal_direction: Option<String>,
}

/// `POST /v1/metrics/{id}/preview` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PreviewMetricBody {
    /// Days of history to query (default 7 server-side when unset/zero).
    #[serde(default)]
    pub days: u32,
}

/// One preview bucket (single scalar per day).
#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewBucketJson {
    pub day: String,
    pub value: f64,
}

/// Response from `POST /v1/metrics/{id}/preview`.
///
/// Phase 3 returns an empty `buckets` array plus a `warning` telling the
/// caller the ClickHouse wiring is pending (delivered in Phase 4).
#[derive(Debug, Serialize, ToSchema)]
pub struct PreviewMetricResponseJson {
    pub buckets: Vec<PreviewBucketJson>,
    /// Present whenever the analytics layer cannot produce real data yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Paginated list response — mirrors the proto `ListMetricsResponse`.
#[derive(Debug, Serialize, ToSchema)]
pub struct ListMetricsResponseJson {
    pub items: Vec<MetricDefinition>,
    pub total: u64,
    pub offset: u64,
    pub limit: u64,
}

// ─── Body → proto conversion helpers ──────────────────────────────────────────

fn agg_body_to_proto(b: AggregationConfigBody) -> ProtoAggregationConfig {
    ProtoAggregationConfig {
        event_key: b.event_key,
        aggregator: b.aggregator,
        on_field: b.on_field,
        where_clause_json: b.where_clause.map(|v| v.to_string()),
    }
}

fn ratio_body_to_proto(b: RatioConfigBody) -> ProtoRatioConfig {
    ProtoRatioConfig {
        numerator_metric_id: b.numerator_metric_id,
        denominator_metric_id: b.denominator_metric_id,
        min_denominator: b.min_denominator,
    }
}

fn funnel_body_to_proto(b: FunnelConfigBody) -> ProtoFunnelConfig {
    ProtoFunnelConfig {
        steps: b
            .steps
            .into_iter()
            .map(|s| ProtoFunnelStep {
                event_key: s.event_key,
                where_clause_json: s.where_clause.map(|v| v.to_string()),
            })
            .collect(),
        window_seconds: b.window_seconds,
        count_repeats: b.count_repeats,
    }
}

fn kind_body_to_create_proto(k: MetricKindBody) -> create_metric_request::Kind {
    match k {
        MetricKindBody::Aggregation(c) => {
            create_metric_request::Kind::Aggregation(agg_body_to_proto(c))
        }
        MetricKindBody::Ratio(c) => create_metric_request::Kind::Ratio(ratio_body_to_proto(c)),
        MetricKindBody::Funnel(c) => create_metric_request::Kind::Funnel(funnel_body_to_proto(c)),
    }
}

fn kind_body_to_update_proto(k: MetricKindBody) -> update_metric_request::Kind {
    match k {
        MetricKindBody::Aggregation(c) => {
            update_metric_request::Kind::Aggregation(agg_body_to_proto(c))
        }
        MetricKindBody::Ratio(c) => update_metric_request::Kind::Ratio(ratio_body_to_proto(c)),
        MetricKindBody::Funnel(c) => update_metric_request::Kind::Funnel(funnel_body_to_proto(c)),
    }
}

// ─── Proto → domain conversion helpers ────────────────────────────────────────

fn parse_aggregator(s: &str) -> Result<AggregationOperator, GatewayError> {
    match s {
        "count" => Ok(AggregationOperator::Count),
        "sum" => Ok(AggregationOperator::Sum),
        "avg" => Ok(AggregationOperator::Avg),
        "p50" => Ok(AggregationOperator::P50),
        "p90" => Ok(AggregationOperator::P90),
        "p99" => Ok(AggregationOperator::P99),
        "uniq" => Ok(AggregationOperator::Uniq),
        other => Err(GatewayError::Upstream(format!(
            "analytics returned unknown aggregator `{other}`"
        ))),
    }
}

fn parse_goal_direction(s: &str) -> Result<GoalDirection, GatewayError> {
    match s {
        "increase" => Ok(GoalDirection::Increase),
        "decrease" => Ok(GoalDirection::Decrease),
        "neutral" => Ok(GoalDirection::Neutral),
        other => Err(GatewayError::Upstream(format!(
            "analytics returned unknown goal_direction `{other}`"
        ))),
    }
}

fn parse_where_clause_json(s: Option<&str>) -> Result<Option<serde_json::Value>, GatewayError> {
    match s {
        None | Some("") => Ok(None),
        Some(raw) => serde_json::from_str(raw).map(Some).map_err(|e| {
            GatewayError::Upstream(format!("analytics returned non-JSON where_clause: {e}"))
        }),
    }
}

fn parse_metric_id(s: &str) -> Result<MetricId, GatewayError> {
    uuid::Uuid::parse_str(s)
        .map(MetricId::from_uuid)
        .map_err(|e| GatewayError::Upstream(format!("invalid metric id from analytics: {e}")))
}

fn parse_env_id(s: &str) -> Result<EnvironmentId, GatewayError> {
    uuid::Uuid::parse_str(s)
        .map(EnvironmentId::from_uuid)
        .map_err(|e| GatewayError::Upstream(format!("invalid environment id from analytics: {e}")))
}

fn proto_agg_to_domain(c: ProtoAggregationConfig) -> Result<AggregationConfig, GatewayError> {
    Ok(AggregationConfig {
        event_key: c.event_key,
        aggregator: parse_aggregator(&c.aggregator)?,
        on_field: c.on_field,
        where_clause: parse_where_clause_json(c.where_clause_json.as_deref())?,
    })
}

fn proto_ratio_to_domain(c: ProtoRatioConfig) -> Result<RatioConfig, GatewayError> {
    Ok(RatioConfig {
        numerator_metric_id: parse_metric_id(&c.numerator_metric_id)?,
        denominator_metric_id: parse_metric_id(&c.denominator_metric_id)?,
        min_denominator: c.min_denominator,
    })
}

fn proto_funnel_to_domain(c: ProtoFunnelConfig) -> Result<FunnelConfig, GatewayError> {
    let mut steps = Vec::with_capacity(c.steps.len());
    for s in c.steps {
        steps.push(FunnelStep {
            event_key: s.event_key,
            where_clause: parse_where_clause_json(s.where_clause_json.as_deref())?,
        });
    }
    Ok(FunnelConfig {
        steps,
        window_seconds: c.window_seconds,
        count_repeats: c.count_repeats,
    })
}

fn proto_to_domain(p: ProtoMetric) -> Result<MetricDefinition, GatewayError> {
    let kind = match p.kind {
        Some(metric_definition::Kind::Aggregation(c)) => {
            MetricKind::Aggregation(proto_agg_to_domain(c)?)
        }
        Some(metric_definition::Kind::Ratio(c)) => MetricKind::Ratio(proto_ratio_to_domain(c)?),
        Some(metric_definition::Kind::Funnel(c)) => MetricKind::Funnel(proto_funnel_to_domain(c)?),
        None => {
            return Err(GatewayError::Upstream(
                "analytics returned MetricDefinition with no kind".to_string(),
            ));
        }
    };
    Ok(MetricDefinition {
        id: parse_metric_id(&p.id)?,
        environment_id: parse_env_id(&p.environment_id)?,
        key: p.key,
        name: p.name,
        description: p.description,
        kind,
        goal_direction: parse_goal_direction(&p.goal_direction)?,
        version: p.version,
        created_at: chrono::DateTime::parse_from_rfc3339(&p.created_at)
            .map(|d| d.with_timezone(&chrono::Utc))
            .map_err(|e| GatewayError::Upstream(format!("bad created_at: {e}")))?,
        updated_at: chrono::DateTime::parse_from_rfc3339(&p.updated_at)
            .map(|d| d.with_timezone(&chrono::Utc))
            .map_err(|e| GatewayError::Upstream(format!("bad updated_at: {e}")))?,
        deleted_at: p
            .deleted_at
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| chrono::DateTime::parse_from_rfc3339(s).map(|d| d.with_timezone(&chrono::Utc)))
            .transpose()
            .map_err(|e| GatewayError::Upstream(format!("bad deleted_at: {e}")))?,
    })
}

// ─── tonic::Status → GatewayError (extends crate-default mapping) ─────────────

/// Map a gRPC error to a `GatewayError`. Unlike the crate-wide
/// `From<tonic::Status>`, this helper also maps `Aborted` (used by the
/// analytics-service for optimistic-locking conflicts) to `Conflict`,
/// yielding HTTP `409`.
fn status_to_gw_err(s: tonic::Status) -> GatewayError {
    if matches!(s.code(), tonic::Code::Aborted) {
        return GatewayError::Conflict(s.message().to_string());
    }
    GatewayError::from(s)
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `POST /v1/metrics` — create a new metric.
#[utoipa::path(
    post,
    path = "/v1/metrics",
    tag = "metrics",
    request_body = CreateMetricBody,
    responses(
        (status = 201, description = "Metric created", body = MetricDefinition),
        (status = 400, description = "Invalid metric config (e.g. funnel with < 2 steps)"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing metric:write"),
        (status = 409, description = "Metric key already exists in this environment"),
        (status = 502, description = "Analytics service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn create_metric(
    State(state): State<Arc<GatewayState>>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "metric:write")?;

    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 4 * 1024 * 1024)
        .await
        .map_err(|e| GatewayError::BadRequest(e.to_string()))?;
    let body: CreateMetricBody = serde_json::from_slice(&bytes)
        .map_err(|e| GatewayError::BadRequest(format!("invalid request body: {e}")))?;

    let rpc = tonic::Request::new(CreateMetricRequest {
        environment_id: body.environment_id,
        key: body.key,
        name: body.name,
        description: body.description,
        goal_direction: body.goal_direction,
        kind: Some(kind_body_to_create_proto(body.kind)),
    });
    let mut client = state.analytics_client.lock().await;
    let resp = client.create_metric(rpc).await.map_err(status_to_gw_err)?;
    let domain = proto_to_domain(resp.into_inner())?;
    Ok((StatusCode::CREATED, Json(domain)))
}

/// `GET /v1/metrics?env_id=<uuid>&offset=&limit=&kind=` — list metrics.
#[utoipa::path(
    get,
    path = "/v1/metrics",
    tag = "metrics",
    params(
        ("env_id" = String, Query, description = "Environment ID"),
        ("offset" = Option<u64>, Query, description = "0-based offset (default 0)"),
        ("limit" = Option<u64>, Query, description = "Page size (default 50, max 200)"),
        ("kind" = Option<String>, Query,
            description = "Filter by kind: aggregation | ratio | funnel"),
        ("event_key" = Option<String>, Query,
            description = "Filter to metrics directly referencing this event key"),
    ),
    responses(
        (status = 200, description = "Paginated metrics", body = ListMetricsResponseJson),
        (status = 400, description = "Invalid env_id"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing metric:read"),
        (status = 502, description = "Analytics service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn list_metrics(
    State(state): State<Arc<GatewayState>>,
    Query(query): Query<ListMetricsQuery>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "metric:read")?;

    let rpc = tonic::Request::new(ListMetricsRequest {
        environment_id: query.env_id,
        offset: query.offset,
        limit: query.limit,
        kind: query.kind,
        event_key: query.event_key,
    });
    let mut client = state.analytics_client.lock().await;
    let resp = client.list_metrics(rpc).await.map_err(status_to_gw_err)?;
    let inner = resp.into_inner();
    let mut items = Vec::with_capacity(inner.items.len());
    for p in inner.items {
        items.push(proto_to_domain(p)?);
    }
    Ok(Json(ListMetricsResponseJson {
        items,
        total: inner.total,
        offset: inner.offset,
        limit: inner.limit,
    }))
}

/// `GET /v1/metrics/{id}` — fetch one metric.
#[utoipa::path(
    get,
    path = "/v1/metrics/{id}",
    tag = "metrics",
    params(("id" = String, Path, description = "Metric UUID")),
    responses(
        (status = 200, description = "Metric", body = MetricDefinition),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing metric:read"),
        (status = 404, description = "Metric not found"),
        (status = 502, description = "Analytics service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn get_metric(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "metric:read")?;

    let rpc = tonic::Request::new(GetMetricRequest {
        id: Some(id),
        environment_id: None,
        key: None,
    });
    let mut client = state.analytics_client.lock().await;
    let resp = client.get_metric(rpc).await.map_err(status_to_gw_err)?;
    let domain = proto_to_domain(resp.into_inner())?;
    Ok(Json(domain))
}

/// `PATCH /v1/metrics/{id}` — update a metric (optimistic locking).
#[utoipa::path(
    patch,
    path = "/v1/metrics/{id}",
    tag = "metrics",
    params(("id" = String, Path, description = "Metric UUID")),
    request_body = UpdateMetricBody,
    responses(
        (status = 200, description = "Updated metric", body = MetricDefinition),
        (status = 400, description = "Invalid update payload"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing metric:write"),
        (status = 404, description = "Metric not found"),
        (status = 409, description = "Version conflict — concurrent edit detected"),
        (status = 502, description = "Analytics service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn update_metric(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "metric:write")?;

    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 4 * 1024 * 1024)
        .await
        .map_err(|e| GatewayError::BadRequest(e.to_string()))?;
    let body: UpdateMetricBody = serde_json::from_slice(&bytes)
        .map_err(|e| GatewayError::BadRequest(format!("invalid request body: {e}")))?;

    let rpc = tonic::Request::new(UpdateMetricRequest {
        id,
        expected_version: body.expected_version,
        name: body.name,
        description: body.description,
        goal_direction: body.goal_direction,
        kind: body.kind.map(kind_body_to_update_proto),
    });
    let mut client = state.analytics_client.lock().await;
    let resp = client.update_metric(rpc).await.map_err(status_to_gw_err)?;
    let domain = proto_to_domain(resp.into_inner())?;
    Ok(Json(domain))
}

/// `DELETE /v1/metrics/{id}` — soft-delete a metric.
#[utoipa::path(
    delete,
    path = "/v1/metrics/{id}",
    tag = "metrics",
    params(("id" = String, Path, description = "Metric UUID")),
    responses(
        (status = 204, description = "Metric deleted"),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing metric:write"),
        (status = 404, description = "Metric not found"),
        (status = 502, description = "Analytics service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn delete_metric(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "metric:write")?;

    let rpc = tonic::Request::new(DeleteMetricRequest { id });
    let mut client = state.analytics_client.lock().await;
    client.delete_metric(rpc).await.map_err(status_to_gw_err)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/metrics/{id}/preview` — preview metric over last N days.
///
/// Runs the kind-specific ClickHouse query against `events` and
/// returns one bucket per day in the window (zero-filled where the
/// query returned no row). See
/// `stitchd_stats_service::queries::preview` for the SQL builders.
#[utoipa::path(
    post,
    path = "/v1/metrics/{id}/preview",
    tag = "metrics",
    params(("id" = String, Path, description = "Metric UUID")),
    request_body = PreviewMetricBody,
    responses(
        (status = 200, description = "Preview buckets (empty for Phase 3)",
            body = PreviewMetricResponseJson),
        (status = 401, description = "Unauthorized"),
        (status = 403, description = "Forbidden — missing metric:read"),
        (status = 502, description = "Analytics service unavailable"),
    ),
    security(("bearer_jwt" = []))
)]
pub async fn preview_metric(
    State(state): State<Arc<GatewayState>>,
    Path(id): Path<String>,
    req: axum::extract::Request,
) -> Result<impl IntoResponse, GatewayError> {
    require_permission(&req, "metric:read")?;

    let (_parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .map_err(|e| GatewayError::BadRequest(e.to_string()))?;
    // Empty body is allowed — preview defaults to days=7 server-side.
    let body: PreviewMetricBody = if bytes.is_empty() {
        PreviewMetricBody { days: 0 }
    } else {
        serde_json::from_slice(&bytes)
            .map_err(|e| GatewayError::BadRequest(format!("invalid request body: {e}")))?
    };

    let rpc = tonic::Request::new(PreviewMetricRequest {
        target: Some(preview_metric_request::Target::Id(id)),
        days: body.days,
    });
    let mut client = state.analytics_client.lock().await;
    let resp = client.preview_metric(rpc).await.map_err(status_to_gw_err)?;
    let inner = resp.into_inner();
    let buckets: Vec<PreviewBucketJson> = inner
        .buckets
        .into_iter()
        .map(|b| PreviewBucketJson {
            day: b.day,
            value: b.value,
        })
        .collect();
    // The Phase 4 ClickHouse pipeline now drives this response — an empty
    // bucket list legitimately means "no events in the window", not "the
    // backend isn't implemented yet". `warning` stays in the response
    // shape (proto compat) but is always None.
    Ok(Json(PreviewMetricResponseJson {
        buckets,
        warning: None,
    }))
}

// ─── Test helpers ─────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::{get, post};
    axum::Router::new()
        .route("/v1/metrics", get(list_metrics).post(create_metric))
        .route(
            "/v1/metrics/{id}",
            get(get_metric).patch(update_metric).delete(delete_metric),
        )
        .route("/v1/metrics/{id}/preview", post(preview_metric))
        .with_state(state)
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use tonic::transport::Channel;
    use tonic::{Response, Status};
    use tower::ServiceExt as _;
    use uuid::Uuid;

    use stitchd_proto::analytics::v1::{
        AggregationConfig as ProtoAggregationConfig, CreateMetricRequest, DeleteMetricRequest,
        DeleteMetricResponse, GetMetricRequest, IngestEventRequest, IngestEventResponse,
        ListContextParamsRequest, ListContextParamsResponse, ListContextTypesRequest,
        ListContextTypesResponse, ListExperimentResultsRequest, ListMetricsRequest,
        ListMetricsResponse, MetricDefinition as ProtoMetric, PreviewBucket, PreviewMetricRequest,
        PreviewMetricResponse, RegisterContextRequest, RegisterContextResponse, TrackEventsRequest,
        TrackEventsResponse, UpdateMetricRequest,
        analytics_service_client::AnalyticsServiceClient,
        analytics_service_server::{AnalyticsService as Svc, AnalyticsServiceServer},
        metric_definition,
    };

    // ── Mock analytics service ─────────────────────────────────────────────────
    //
    // Each handler is parameterised by a `Behaviour` enum so individual tests
    // can pre-load responses (Ok / Status::not_found / Status::aborted …).
    // Captured request args go into `Mutex<Option<...>>` so assertions can
    // inspect what the gateway sent.

    #[derive(Clone, Default)]
    struct Captured {
        create: Arc<Mutex<Option<CreateMetricRequest>>>,
        list: Arc<Mutex<Option<ListMetricsRequest>>>,
        get: Arc<Mutex<Option<GetMetricRequest>>>,
        update: Arc<Mutex<Option<UpdateMetricRequest>>>,
        delete: Arc<Mutex<Option<DeleteMetricRequest>>>,
        preview: Arc<Mutex<Option<PreviewMetricRequest>>>,
    }

    #[derive(Clone)]
    enum CreateBehaviour {
        Ok(Box<ProtoMetric>),
        InvalidArg(String),
    }

    #[derive(Clone)]
    enum GetBehaviour {
        Ok(Box<ProtoMetric>),
        NotFound,
    }

    #[derive(Clone, Default)]
    struct Mock {
        captured: Captured,
        create: Arc<Mutex<Option<CreateBehaviour>>>,
        list: Arc<Mutex<Option<ListMetricsResponse>>>,
        get: Arc<Mutex<Option<GetBehaviour>>>,
        update: Arc<Mutex<Option<Result<ProtoMetric, Status>>>>,
        delete: Arc<Mutex<Option<Result<(), Status>>>>,
        preview: Arc<Mutex<Option<PreviewMetricResponse>>>,
    }

    #[tonic::async_trait]
    impl Svc for Mock {
        type ListExperimentResultsStream = tokio_stream::wrappers::ReceiverStream<
            Result<stitchd_proto::analytics::v1::ExperimentResult, Status>,
        >;

        // ── Stubs for unused RPCs ─────────────────────────────────────────────
        async fn ingest_event(
            &self,
            _req: tonic::Request<IngestEventRequest>,
        ) -> Result<Response<IngestEventResponse>, Status> {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn track_events(
            &self,
            _req: tonic::Request<TrackEventsRequest>,
        ) -> Result<Response<TrackEventsResponse>, Status> {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn register_context(
            &self,
            _req: tonic::Request<RegisterContextRequest>,
        ) -> Result<Response<RegisterContextResponse>, Status> {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn list_context_types(
            &self,
            _req: tonic::Request<ListContextTypesRequest>,
        ) -> Result<Response<ListContextTypesResponse>, Status> {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn list_context_params(
            &self,
            _req: tonic::Request<ListContextParamsRequest>,
        ) -> Result<Response<ListContextParamsResponse>, Status> {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn get_eval_stats(
            &self,
            _req: tonic::Request<stitchd_proto::analytics::v1::GetEvalStatsRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::GetEvalStatsResponse>, Status> {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn get_context_intelligence(
            &self,
            _req: tonic::Request<stitchd_proto::analytics::v1::GetContextIntelligenceRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::GetContextIntelligenceResponse>, Status>
        {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn write_experiment_results(
            &self,
            _req: tonic::Request<stitchd_proto::analytics::v1::WriteExperimentResultsRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::WriteExperimentResultsResponse>, Status>
        {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn list_experiment_results(
            &self,
            _req: tonic::Request<ListExperimentResultsRequest>,
        ) -> Result<Response<Self::ListExperimentResultsStream>, Status> {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn get_experiment_result(
            &self,
            _req: tonic::Request<stitchd_proto::analytics::v1::GetExperimentResultRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::ExperimentResult>, Status> {
            Err(Status::unimplemented("not used in metric tests"))
        }

        // ── Metric handlers ───────────────────────────────────────────────────
        async fn create_metric(
            &self,
            req: tonic::Request<CreateMetricRequest>,
        ) -> Result<Response<ProtoMetric>, Status> {
            *self.captured.create.lock().await = Some(req.into_inner());
            match self
                .create
                .lock()
                .await
                .clone()
                .expect("test forgot to load create behaviour")
            {
                CreateBehaviour::Ok(p) => Ok(Response::new(*p)),
                CreateBehaviour::InvalidArg(msg) => Err(Status::invalid_argument(msg)),
            }
        }
        async fn get_metric(
            &self,
            req: tonic::Request<GetMetricRequest>,
        ) -> Result<Response<ProtoMetric>, Status> {
            *self.captured.get.lock().await = Some(req.into_inner());
            match self
                .get
                .lock()
                .await
                .clone()
                .expect("test forgot to load get behaviour")
            {
                GetBehaviour::Ok(p) => Ok(Response::new(*p)),
                GetBehaviour::NotFound => Err(Status::not_found("metric not found")),
            }
        }
        async fn list_metrics(
            &self,
            req: tonic::Request<ListMetricsRequest>,
        ) -> Result<Response<ListMetricsResponse>, Status> {
            *self.captured.list.lock().await = Some(req.into_inner());
            Ok(Response::new(
                self.list
                    .lock()
                    .await
                    .clone()
                    .expect("test forgot to load list response"),
            ))
        }
        async fn update_metric(
            &self,
            req: tonic::Request<UpdateMetricRequest>,
        ) -> Result<Response<ProtoMetric>, Status> {
            *self.captured.update.lock().await = Some(req.into_inner());
            match self
                .update
                .lock()
                .await
                .clone()
                .expect("test forgot to load update behaviour")
            {
                Ok(p) => Ok(Response::new(p)),
                Err(s) => Err(s),
            }
        }
        async fn delete_metric(
            &self,
            req: tonic::Request<DeleteMetricRequest>,
        ) -> Result<Response<DeleteMetricResponse>, Status> {
            *self.captured.delete.lock().await = Some(req.into_inner());
            match self
                .delete
                .lock()
                .await
                .clone()
                .expect("test forgot to load delete behaviour")
            {
                Ok(()) => Ok(Response::new(DeleteMetricResponse {})),
                Err(s) => Err(s),
            }
        }
        async fn preview_metric(
            &self,
            req: tonic::Request<PreviewMetricRequest>,
        ) -> Result<Response<PreviewMetricResponse>, Status> {
            *self.captured.preview.lock().await = Some(req.into_inner());
            Ok(Response::new(
                self.preview
                    .lock()
                    .await
                    .clone()
                    .expect("test forgot to load preview response"),
            ))
        }
        async fn get_event_firings(
            &self,
            _req: tonic::Request<stitchd_proto::analytics::v1::GetEventFiringsRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::GetEventFiringsResponse>, Status>
        {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn get_event_stats(
            &self,
            _req: tonic::Request<stitchd_proto::analytics::v1::GetEventStatsRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::GetEventStatsResponse>, Status> {
            Err(Status::unimplemented("not used in metric tests"))
        }
        // Event-definitions CRUD — closed in feature-flag-wr4.
        async fn create_event_definition(
            &self,
            _req: tonic::Request<stitchd_proto::analytics::v1::CreateEventDefinitionRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::EventDefinitionMsg>, Status> {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn get_event_definition(
            &self,
            _req: tonic::Request<stitchd_proto::analytics::v1::GetEventDefinitionRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::EventDefinitionMsg>, Status> {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn list_event_definitions(
            &self,
            _req: tonic::Request<stitchd_proto::analytics::v1::ListEventDefinitionsRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::ListEventDefinitionsResponse>, Status>
        {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn update_event_definition(
            &self,
            _req: tonic::Request<stitchd_proto::analytics::v1::UpdateEventDefinitionRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::EventDefinitionMsg>, Status> {
            Err(Status::unimplemented("not used in metric tests"))
        }
        async fn delete_event_definition(
            &self,
            _req: tonic::Request<stitchd_proto::analytics::v1::DeleteEventDefinitionRequest>,
        ) -> Result<Response<stitchd_proto::analytics::v1::DeleteEventDefinitionResponse>, Status>
        {
            Err(Status::unimplemented("not used in metric tests"))
        }
    }

    /// Spin up an in-process gRPC mock and return the `AnalyticsServiceClient`
    /// pointing at it, plus a handle on the `Mock` so tests can program its
    /// behaviour.
    async fn spawn_mock() -> (AnalyticsServiceClient<Channel>, Mock) {
        use tonic::transport::Server;
        let mock = Mock::default();
        let svc = mock.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            Server::builder()
                .add_service(AnalyticsServiceServer::new(svc))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let client = AnalyticsServiceClient::connect(format!("http://{addr}"))
            .await
            .unwrap();
        (client, mock)
    }

    /// Build a `GatewayState` where the `analytics_client` is wired to the
    /// in-process mock and every other client is a lazy stub.
    async fn make_state_with_mock() -> (Arc<GatewayState>, Mock) {
        let (analytics, mock) = spawn_mock().await;

        use stitchd_proto::auth::v1::{
            auth_provider_service_client::AuthProviderServiceClient,
            auth_service_client::AuthServiceClient,
            oidc_login_service_client::OidcLoginServiceClient,
            saml_login_service_client::SamlLoginServiceClient,
        };
        use stitchd_proto::experiments::v1::experimentation_service_client::ExperimentationServiceClient;
        use stitchd_proto::flags::v1::flag_service_client::FlagServiceClient;
        use stitchd_proto::management::v1::management_service_client::ManagementServiceClient;
        use stitchd_proto::segments::v1::segmentation_service_client::SegmentationServiceClient;
        use stitchd_proto::stats::v1::stats_service_client::StatsServiceClient;

        let flag_channel = Channel::from_static("http://127.0.0.1:2").connect_lazy();
        let seg_channel = Channel::from_static("http://127.0.0.1:3").connect_lazy();
        let state = GatewayState::from_channels(
            AuthServiceClient::new(Channel::from_static("http://127.0.0.1:1").connect_lazy()),
            FlagServiceClient::new(flag_channel.clone()),
            flag_channel,
            SegmentationServiceClient::new(seg_channel.clone()),
            seg_channel,
            analytics,
            ExperimentationServiceClient::new(
                Channel::from_static("http://127.0.0.1:5").connect_lazy(),
            ),
            ManagementServiceClient::new(Channel::from_static("http://127.0.0.1:6").connect_lazy()),
            AuthProviderServiceClient::new(
                Channel::from_static("http://127.0.0.1:7").connect_lazy(),
            ),
            OidcLoginServiceClient::new(Channel::from_static("http://127.0.0.1:8").connect_lazy()),
            SamlLoginServiceClient::new(Channel::from_static("http://127.0.0.1:9").connect_lazy()),
            StatsServiceClient::new(Channel::from_static("http://127.0.0.1:10").connect_lazy()),
        );
        (Arc::new(state), mock)
    }

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

    fn sample_metric(env: Uuid, id: Uuid, key: &str) -> ProtoMetric {
        ProtoMetric {
            id: id.to_string(),
            environment_id: env.to_string(),
            key: key.to_string(),
            name: format!("Metric {key}"),
            description: Some("for test".into()),
            kind: Some(metric_definition::Kind::Aggregation(
                ProtoAggregationConfig {
                    event_key: "checkout".into(),
                    aggregator: "count".into(),
                    on_field: None,
                    where_clause_json: None,
                },
            )),
            goal_direction: "increase".into(),
            version: 1,
            created_at: "2026-05-19T00:00:00Z".into(),
            updated_at: "2026-05-19T00:00:00Z".into(),
            deleted_at: None,
        }
    }

    // ── test_create_metric_returns_201 ─────────────────────────────────────────

    #[tokio::test]
    async fn test_create_metric_returns_201() {
        let (state, mock) = make_state_with_mock().await;
        let env = Uuid::new_v4();
        let id = Uuid::new_v4();
        *mock.create.lock().await = Some(CreateBehaviour::Ok(Box::new(sample_metric(
            env,
            id,
            "checkout_rate",
        ))));

        let app = test_router(state);
        let body = serde_json::json!({
            "environment_id": env.to_string(),
            "key": "checkout_rate",
            "name": "Checkout Rate",
            "goal_direction": "increase",
            "kind": "aggregation",
            "event_key": "checkout",
            "aggregator": "count"
        });
        let mut req = Request::builder()
            .method("POST")
            .uri("/v1/metrics")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["metric:write"]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        let captured = mock.captured.create.lock().await.clone().unwrap();
        assert_eq!(captured.environment_id, env.to_string());
        assert_eq!(captured.key, "checkout_rate");
        assert_eq!(captured.goal_direction, "increase");
        assert!(matches!(
            captured.kind,
            Some(create_metric_request::Kind::Aggregation(_))
        ));
    }

    // ── test_create_metric_returns_400_on_validation_failure ─────────────────

    #[tokio::test]
    async fn test_create_metric_returns_400_on_validation_failure() {
        let (state, mock) = make_state_with_mock().await;
        *mock.create.lock().await = Some(CreateBehaviour::InvalidArg(
            "funnel must have at least 2 steps (got 1)".into(),
        ));

        let app = test_router(state);
        let body = serde_json::json!({
            "environment_id": Uuid::new_v4().to_string(),
            "key": "bad_funnel",
            "name": "Bad Funnel",
            "goal_direction": "increase",
            "kind": "funnel",
            "steps": [{ "event_key": "only" }],
            "window_seconds": 60,
            "count_repeats": false
        });
        let mut req = Request::builder()
            .method("POST")
            .uri("/v1/metrics")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["metric:write"]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── test_get_metric_returns_200_with_domain_shape ─────────────────────────

    #[tokio::test]
    async fn test_get_metric_returns_200_with_domain_shape() {
        let (state, mock) = make_state_with_mock().await;
        let env = Uuid::new_v4();
        let id = Uuid::new_v4();
        *mock.get.lock().await = Some(GetBehaviour::Ok(Box::new(sample_metric(env, id, "ok"))));

        let app = test_router(state);
        let mut req = Request::builder()
            .uri(format!("/v1/metrics/{id}"))
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["metric:read"]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        // The wire shape mirrors the core MetricDefinition: `kind` discriminator,
        // typed UUIDs as strings, RFC3339 timestamps.
        assert_eq!(parsed["id"], id.to_string());
        assert_eq!(parsed["environment_id"], env.to_string());
        assert_eq!(parsed["kind"], "aggregation");
        assert_eq!(parsed["goal_direction"], "increase");

        let captured = mock.captured.get.lock().await.clone().unwrap();
        assert_eq!(captured.id.as_deref(), Some(id.to_string()).as_deref());
    }

    // ── test_get_metric_returns_404_for_missing ───────────────────────────────

    #[tokio::test]
    async fn test_get_metric_returns_404_for_missing() {
        let (state, mock) = make_state_with_mock().await;
        *mock.get.lock().await = Some(GetBehaviour::NotFound);

        let app = test_router(state);
        let id = Uuid::new_v4();
        let mut req = Request::builder()
            .uri(format!("/v1/metrics/{id}"))
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["metric:read"]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── test_list_metrics_paginated ───────────────────────────────────────────

    #[tokio::test]
    async fn test_list_metrics_paginated() {
        let (state, mock) = make_state_with_mock().await;
        let env = Uuid::new_v4();
        let m1 = sample_metric(env, Uuid::new_v4(), "m1");
        let m2 = sample_metric(env, Uuid::new_v4(), "m2");
        *mock.list.lock().await = Some(ListMetricsResponse {
            items: vec![m1, m2],
            total: 5,
            offset: 0,
            limit: 2,
        });

        let app = test_router(state);
        let mut req = Request::builder()
            .uri(format!(
                "/v1/metrics?env_id={env}&offset=0&limit=2&kind=aggregation"
            ))
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["metric:read"]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let captured = mock.captured.list.lock().await.clone().unwrap();
        assert_eq!(captured.environment_id, env.to_string());
        assert_eq!(captured.offset, Some(0));
        assert_eq!(captured.limit, Some(2));
        assert_eq!(captured.kind.as_deref(), Some("aggregation"));

        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(parsed["total"], 5);
        assert_eq!(parsed["offset"], 0);
        assert_eq!(parsed["limit"], 2);
        assert_eq!(parsed["items"].as_array().unwrap().len(), 2);
        // Domain MetricDefinition serialises with `kind` discriminator.
        assert_eq!(parsed["items"][0]["kind"], "aggregation");
    }

    // ── test_update_metric_returns_409_on_stale_version ──────────────────────

    #[tokio::test]
    async fn test_update_metric_returns_409_on_stale_version() {
        let (state, mock) = make_state_with_mock().await;
        *mock.update.lock().await = Some(Err(Status::aborted(
            "version conflict — expected=1, actual=2",
        )));

        let app = test_router(state);
        let id = Uuid::new_v4();
        let body = serde_json::json!({
            "expected_version": 1,
            "name": "new name",
            "kind": "aggregation",
            "event_key": "checkout",
            "aggregator": "count"
        });
        let mut req = Request::builder()
            .method("PATCH")
            .uri(format!("/v1/metrics/{id}"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["metric:write"]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    // ── test_delete_metric_returns_204 ────────────────────────────────────────

    #[tokio::test]
    async fn test_delete_metric_returns_204() {
        let (state, mock) = make_state_with_mock().await;
        *mock.delete.lock().await = Some(Ok(()));

        let app = test_router(state);
        let id = Uuid::new_v4();
        let mut req = Request::builder()
            .method("DELETE")
            .uri(format!("/v1/metrics/{id}"))
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["metric:write"]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);

        let captured = mock.captured.delete.lock().await.clone().unwrap();
        assert_eq!(captured.id, id.to_string());
    }

    // ── test_preview_metric_empty_buckets_have_null_warning ──────────────────
    //
    // Post-feature-flag-pjw: an empty `buckets` list means "no events in
    // the window", not "Phase 4 isn't implemented". The `warning` field
    // stays on the wire (utoipa schema compat) but is always `null`.

    #[tokio::test]
    async fn test_preview_metric_empty_buckets_have_null_warning() {
        let (state, mock) = make_state_with_mock().await;
        *mock.preview.lock().await = Some(PreviewMetricResponse {
            buckets: Vec::<PreviewBucket>::new(),
        });

        let app = test_router(state);
        let id = Uuid::new_v4();
        let mut req = Request::builder()
            .method("POST")
            .uri(format!("/v1/metrics/{id}/preview"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"days":7}"#))
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["metric:read"]));
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
        assert_eq!(parsed["buckets"].as_array().unwrap().len(), 0);
        assert!(
            parsed["warning"].is_null(),
            "Phase 4: warning is null even on empty buckets; empty means 'no events'"
        );

        let captured = mock.captured.preview.lock().await.clone().unwrap();
        assert_eq!(captured.days, 7);
        assert!(matches!(
            captured.target,
            Some(preview_metric_request::Target::Id(_))
        ));
    }

    // ── test_metric_routes_require_metric_write_permission ───────────────────

    #[tokio::test]
    async fn test_metric_routes_require_metric_write_permission() {
        // metric:read alone must reject every mutating route.
        let app = || async {
            let (state, mock) = make_state_with_mock().await;
            // Pre-load behaviours so any leak through would surface, not panic.
            *mock.create.lock().await = Some(CreateBehaviour::Ok(Box::new(sample_metric(
                Uuid::new_v4(),
                Uuid::new_v4(),
                "k",
            ))));
            *mock.update.lock().await =
                Some(Ok(sample_metric(Uuid::new_v4(), Uuid::new_v4(), "k")));
            *mock.delete.lock().await = Some(Ok(()));
            test_router(state)
        };

        // POST without metric:write → 401
        let mut req = Request::builder()
            .method("POST")
            .uri("/v1/metrics")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "environment_id": Uuid::new_v4().to_string(),
                    "key": "k",
                    "name": "n",
                    "goal_direction": "increase",
                    "kind": "aggregation",
                    "event_key": "e",
                    "aggregator": "count"
                })
                .to_string(),
            ))
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["metric:read"]));
        let resp = app().await.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // PATCH without metric:write → 401
        let mut req = Request::builder()
            .method("PATCH")
            .uri(format!("/v1/metrics/{}", Uuid::new_v4()))
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({
                    "expected_version": 1,
                    "kind": "aggregation",
                    "event_key": "e",
                    "aggregator": "count"
                })
                .to_string(),
            ))
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["metric:read"]));
        let resp = app().await.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // DELETE without metric:write → 401
        let mut req = Request::builder()
            .method("DELETE")
            .uri(format!("/v1/metrics/{}", Uuid::new_v4()))
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(make_rbac_with_perms(&["metric:read"]));
        let resp = app().await.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // POST with no RBAC at all → 401
        let req = Request::builder()
            .method("POST")
            .uri("/v1/metrics")
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap();
        let resp = app().await.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    #[test]
    fn status_to_gw_err_maps_aborted_to_conflict() {
        let s = Status::aborted("stale");
        match status_to_gw_err(s) {
            GatewayError::Conflict(_) => {}
            other => panic!("expected Conflict, got {other:?}"),
        }
    }

    #[test]
    fn status_to_gw_err_delegates_to_default_mapping() {
        let s = Status::not_found("missing");
        assert!(matches!(status_to_gw_err(s), GatewayError::NotFound(_)));
    }

    #[test]
    fn agg_body_serialises_where_clause_to_string() {
        let body = AggregationConfigBody {
            event_key: "click".into(),
            aggregator: "count".into(),
            on_field: None,
            where_clause: Some(serde_json::json!({"==":[1, 1]})),
        };
        let p = agg_body_to_proto(body);
        assert_eq!(p.where_clause_json.as_deref(), Some("{\"==\":[1,1]}"));
    }

    #[test]
    fn proto_to_domain_round_trips_aggregation() {
        let env = Uuid::new_v4();
        let id = Uuid::new_v4();
        let p = sample_metric(env, id, "demo");
        let d = proto_to_domain(p).unwrap();
        assert_eq!(d.environment_id.to_string(), env.to_string());
        assert_eq!(d.id.to_string(), id.to_string());
        assert_eq!(d.key, "demo");
        assert_eq!(d.goal_direction, GoalDirection::Increase);
        assert!(matches!(d.kind, MetricKind::Aggregation(_)));
    }
}
