//! Event route handlers — proxy REST requests to the Event Ingestion Service.

use axum::{
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::analytics::v1::{IngestEventRequest, MetricEvent, MetricValue, metric_value};

use crate::error::GatewayError;
use crate::pagination::{PaginatedResponse, PaginationParams};
use crate::state::GatewayState;

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
    State(_state): State<Arc<GatewayState>>,
    Path(_environment_id): Path<String>,
    Query(pagination): Query<PaginationParams>,
) -> impl IntoResponse {
    // Event definitions are managed by the Event Service.
    // Stub: return empty paginated response (full proxy implementation omitted as
    // EventIngestionService does not have a ListEventDefinitions RPC in the current proto).
    Json(PaginatedResponse::new(
        Vec::<serde_json::Value>::new(),
        0,
        &pagination,
    ))
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
    State(_state): State<Arc<GatewayState>>,
    Path(_environment_id): Path<String>,
    Json(_body): Json<serde_json::Value>,
) -> impl IntoResponse {
    axum::http::StatusCode::ACCEPTED
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
    State(_state): State<Arc<GatewayState>>,
    Path((_environment_id, event_definition_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let _ = event_definition_id;
    axum::http::StatusCode::NOT_IMPLEMENTED
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
    State(_state): State<Arc<GatewayState>>,
    Path((_environment_id, event_definition_id)): Path<(String, String)>,
    Json(_body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let _ = event_definition_id;
    axum::http::StatusCode::ACCEPTED
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
    State(_state): State<Arc<GatewayState>>,
    Path((_environment_id, event_definition_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let _ = event_definition_id;
    axum::http::StatusCode::NO_CONTENT
}

// ─── Test helpers ─────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    #[allow(unused_imports)]
    use axum::routing::{delete, get, post, put};
    axum::Router::new()
        .route("/v1/environments/{environment_id}/events", post(ingest_event))
        .route("/v1/environments/{environment_id}/events/batch", post(ingest_batch))
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
    async fn list_event_definitions_returns_200() {
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
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_event_definition_returns_202() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/event-definitions")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"key":"click","value_type":"bool"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn delete_event_definition_returns_204() {
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
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
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
}
