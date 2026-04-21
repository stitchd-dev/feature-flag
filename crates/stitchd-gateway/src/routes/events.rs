//! Event route handlers — proxy REST requests to the Event Ingestion Service.

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use stitchd_proto::events::v1::{Event, IngestRequest, MetricValue, metric_value};

use crate::error::GatewayError;
use crate::state::GatewayState;

// ─── REST types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EventBody {
    pub metric_key: String,
    pub context_type: String,
    pub context_key: String,
    pub value: Option<serde_json::Value>,
    pub timestamp_ms: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct BatchEventBody {
    pub events: Vec<EventBody>,
}

#[derive(Debug, Serialize)]
pub struct IngestResponseJson {
    pub accepted_count: u32,
    pub rejected_keys: Vec<String>,
}

fn body_to_event(b: &EventBody) -> Event {
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
    Event {
        metric_key: b.metric_key.clone(),
        context_type: b.context_type.clone(),
        context_key: b.context_key.clone(),
        value,
        timestamp_ms: b.timestamp_ms.unwrap_or(0),
    }
}

// ─── Handlers ─────────────────────────────────────────────────────────────────

/// `POST /v1/environments/{env_id}/events`
pub async fn ingest_event(
    State(state): State<Arc<GatewayState>>,
    Path(_env_id): Path<String>,
    Json(body): Json<EventBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(IngestRequest {
        events: vec![body_to_event(&body)],
    });
    let mut client = state.event_client.lock().await;
    let resp = client.ingest_event(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    Ok(Json(IngestResponseJson {
        accepted_count: inner.accepted_count,
        rejected_keys: inner.rejected_keys,
    }))
}

/// `POST /v1/environments/{env_id}/events/batch`
pub async fn ingest_batch(
    State(state): State<Arc<GatewayState>>,
    Path(_env_id): Path<String>,
    Json(body): Json<BatchEventBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let events = body.events.iter().map(body_to_event).collect();
    let req = tonic::Request::new(IngestRequest { events });
    let mut client = state.event_client.lock().await;
    let resp = client.ingest_event(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    Ok(Json(IngestResponseJson {
        accepted_count: inner.accepted_count,
        rejected_keys: inner.rejected_keys,
    }))
}

/// `GET /v1/environments/{env_id}/event-definitions`
pub async fn list_event_definitions(
    State(_state): State<Arc<GatewayState>>,
    Path(_env_id): Path<String>,
) -> impl IntoResponse {
    // Event definitions are managed by the Event Service.
    // Stub: return empty list (full proxy implementation omitted as EventIngestionService
    // does not have a ListEventDefinitions RPC in the current proto).
    Json(serde_json::json!({ "event_definitions": [] }))
}

/// `POST /v1/environments/{env_id}/event-definitions`
pub async fn create_event_definition(
    State(_state): State<Arc<GatewayState>>,
    Path(_env_id): Path<String>,
    Json(_body): Json<serde_json::Value>,
) -> impl IntoResponse {
    axum::http::StatusCode::ACCEPTED
}

/// `GET /v1/environments/{env_id}/event-definitions/{def_id}`
pub async fn get_event_definition(
    State(_state): State<Arc<GatewayState>>,
    Path((_env_id, def_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let _ = def_id;
    axum::http::StatusCode::NOT_IMPLEMENTED
}

/// `PUT /v1/environments/{env_id}/event-definitions/{def_id}`
pub async fn update_event_definition(
    State(_state): State<Arc<GatewayState>>,
    Path((_env_id, def_id)): Path<(String, String)>,
    Json(_body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let _ = def_id;
    axum::http::StatusCode::ACCEPTED
}

/// `DELETE /v1/environments/{env_id}/event-definitions/{def_id}`
pub async fn delete_event_definition(
    State(_state): State<Arc<GatewayState>>,
    Path((_env_id, def_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let _ = def_id;
    axum::http::StatusCode::NO_CONTENT
}

// ─── Test helpers ─────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    #[allow(unused_imports)]
    use axum::routing::{delete, get, post, put};
    axum::Router::new()
        .route("/v1/environments/{env_id}/events", post(ingest_event))
        .route("/v1/environments/{env_id}/events/batch", post(ingest_batch))
        .route(
            "/v1/environments/{env_id}/event-definitions",
            get(list_event_definitions).post(create_event_definition),
        )
        .route(
            "/v1/environments/{env_id}/event-definitions/{def_id}",
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
