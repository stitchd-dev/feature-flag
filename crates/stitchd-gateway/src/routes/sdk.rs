//! SDK-facing route handlers — authenticated by `x-sdk-key`, proxy to Flag/Segmentation/Event services.
//!
//! These endpoints are called by the SDK client directly (not by human users).
//! Authentication is via the `x-sdk-key` header validated by the Auth Service.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::events::v1::{
    Event, IngestRequest, MetricValue, metric_value::Value as MetricVal,
};
use stitchd_proto::segments::v1::EvaluateMembershipRequest;

use crate::error::GatewayError;
use crate::state::GatewayState;

// ─── REST types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct EvaluateRequest {
    pub context_key: String,
    pub context_type: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct EvaluateResponse {
    pub evaluated: bool,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SdkEventBody {
    pub metric_key: String,
    pub context_key: String,
    pub context_type: String,
    #[schema(value_type = Object, nullable = true)]
    pub value: Option<serde_json::Value>,
    pub timestamp_ms: Option<i64>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct IngestResponseJson {
    pub accepted_count: u32,
    pub rejected_keys: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ListCheckRequest {
    pub segment_key: String,
    pub context_key: String,
    pub context_type: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ListCheckResponse {
    pub is_member: bool,
}

fn sdk_event_to_proto(body: &SdkEventBody) -> Event {
    let value = body.value.as_ref().and_then(|v| {
        if let Some(b) = v.as_bool() {
            Some(MetricValue {
                value: Some(MetricVal::BoolValue(b)),
            })
        } else if let Some(i) = v.as_i64() {
            Some(MetricValue {
                value: Some(MetricVal::IntValue(i)),
            })
        } else {
            v.as_f64().map(|f| MetricValue {
                value: Some(MetricVal::DoubleValue(f)),
            })
        }
    });
    Event {
        metric_key: body.metric_key.clone(),
        context_key: body.context_key.clone(),
        context_type: body.context_type.clone(),
        value,
        timestamp_ms: body.timestamp_ms.unwrap_or(0),
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `POST /v1/environments/{env_id}/evaluate`
///
/// Flag evaluation endpoint for SDK clients. Returns a simple evaluated flag result.
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/evaluate",
    tag = "flags",
    params(("env_id" = String, Path, description = "Environment ID")),
    request_body = EvaluateRequest,
    responses(
        (status = 200, description = "Evaluation result", body = EvaluateResponse),
        (status = 401, description = "Invalid or missing SDK key"),
    ),
    security(("sdk_key" = []))
)]
pub async fn evaluate(
    State(_state): State<Arc<GatewayState>>,
    Path(_env_id): Path<String>,
    Json(_body): Json<EvaluateRequest>,
) -> impl IntoResponse {
    // Flag evaluation is performed client-side using definitions synced via GetFlagDefinitions.
    // This endpoint is a no-op placeholder that confirms the SDK's auth.
    Json(EvaluateResponse { evaluated: true })
}

/// `POST /v1/environments/{env_id}/events`
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/events",
    tag = "events",
    params(("env_id" = String, Path, description = "Environment ID")),
    request_body = SdkEventBody,
    responses(
        (status = 202, description = "Event accepted", body = IngestResponseJson),
        (status = 401, description = "Invalid or missing SDK key"),
        (status = 502, description = "Event service unavailable"),
    ),
    security(("sdk_key" = []))
)]
pub async fn ingest_event(
    State(state): State<Arc<GatewayState>>,
    Path(_env_id): Path<String>,
    Json(body): Json<SdkEventBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let event = sdk_event_to_proto(&body);
    let req = tonic::Request::new(IngestRequest {
        events: vec![event],
    });
    let mut client = state.event_client.lock().await;
    let resp = client.ingest_event(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    Ok((
        StatusCode::ACCEPTED,
        Json(IngestResponseJson {
            accepted_count: inner.accepted_count,
            rejected_keys: inner.rejected_keys,
        }),
    ))
}

/// `POST /v1/environments/{env_id}/events/batch`
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/events/batch",
    tag = "events",
    params(("env_id" = String, Path, description = "Environment ID")),
    request_body(content = Vec<SdkEventBody>, description = "Batch of events"),
    responses(
        (status = 202, description = "Events accepted", body = IngestResponseJson),
        (status = 401, description = "Invalid or missing SDK key"),
        (status = 502, description = "Event service unavailable"),
    ),
    security(("sdk_key" = []))
)]
pub async fn ingest_batch_events(
    State(state): State<Arc<GatewayState>>,
    Path(_env_id): Path<String>,
    Json(bodies): Json<Vec<SdkEventBody>>,
) -> Result<impl IntoResponse, GatewayError> {
    let events: Vec<Event> = bodies.iter().map(sdk_event_to_proto).collect();
    let req = tonic::Request::new(IngestRequest { events });
    let mut client = state.event_client.lock().await;
    let resp = client.ingest_event(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    Ok((
        StatusCode::ACCEPTED,
        Json(IngestResponseJson {
            accepted_count: inner.accepted_count,
            rejected_keys: inner.rejected_keys,
        }),
    ))
}

/// `POST /v1/environments/{env_id}/segments/list-check`
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/segments/list-check",
    tag = "segments",
    params(("env_id" = String, Path, description = "Environment ID")),
    request_body = ListCheckRequest,
    responses(
        (status = 200, description = "Membership result", body = ListCheckResponse),
        (status = 401, description = "Invalid or missing SDK key"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("sdk_key" = []))
)]
pub async fn list_check_membership(
    State(state): State<Arc<GatewayState>>,
    Path(env_id): Path<String>,
    Json(body): Json<ListCheckRequest>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(EvaluateMembershipRequest {
        environment_id: env_id,
        segment_key: body.segment_key,
        context_key: body.context_key,
        context_type: body.context_type,
    });
    let mut client = state.segmentation_client.lock().await;
    let resp = client
        .evaluate_membership(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(Json(ListCheckResponse {
        is_member: resp.into_inner().is_member,
    }))
}

/// `POST /v1/environments/{env_id}/segments/list-check/batch`
#[utoipa::path(
    post,
    path = "/v1/environments/{env_id}/segments/list-check/batch",
    tag = "segments",
    params(("env_id" = String, Path, description = "Environment ID")),
    request_body(content = Vec<ListCheckRequest>, description = "Batch of membership checks"),
    responses(
        (status = 200, description = "Membership results", body = Vec<ListCheckResponse>),
        (status = 401, description = "Invalid or missing SDK key"),
        (status = 502, description = "Segmentation service unavailable"),
    ),
    security(("sdk_key" = []))
)]
pub async fn batch_list_check_membership(
    State(state): State<Arc<GatewayState>>,
    Path(env_id): Path<String>,
    Json(bodies): Json<Vec<ListCheckRequest>>,
) -> Result<impl IntoResponse, GatewayError> {
    let mut results = Vec::with_capacity(bodies.len());
    for body in &bodies {
        let req = tonic::Request::new(EvaluateMembershipRequest {
            environment_id: env_id.clone(),
            segment_key: body.segment_key.clone(),
            context_key: body.context_key.clone(),
            context_type: body.context_type.clone(),
        });
        let mut client = state.segmentation_client.lock().await;
        let resp = client
            .evaluate_membership(req)
            .await
            .map_err(GatewayError::from)?;
        results.push(ListCheckResponse {
            is_member: resp.into_inner().is_member,
        });
    }
    Ok(Json(results))
}

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::post;
    axum::Router::new()
        .route("/v1/environments/{env_id}/evaluate", post(evaluate))
        .route("/v1/environments/{env_id}/events", post(ingest_event))
        .route(
            "/v1/environments/{env_id}/events/batch",
            post(ingest_batch_events),
        )
        .route(
            "/v1/environments/{env_id}/segments/list-check",
            post(list_check_membership),
        )
        .route(
            "/v1/environments/{env_id}/segments/list-check/batch",
            post(batch_list_check_membership),
        )
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::helpers::make_stub_state;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt as _;

    #[test]
    fn sdk_event_to_proto_bool() {
        let b = SdkEventBody {
            metric_key: "k".to_string(),
            context_key: "u1".to_string(),
            context_type: "user".to_string(),
            value: Some(serde_json::json!(true)),
            timestamp_ms: Some(1000),
        };
        let e = sdk_event_to_proto(&b);
        assert_eq!(e.metric_key, "k");
        assert_eq!(e.timestamp_ms, 1000);
        assert!(matches!(
            e.value,
            Some(MetricValue {
                value: Some(MetricVal::BoolValue(true))
            })
        ));
    }

    #[test]
    fn sdk_event_to_proto_int() {
        let b = SdkEventBody {
            metric_key: "k".to_string(),
            context_key: "u1".to_string(),
            context_type: "user".to_string(),
            value: Some(serde_json::json!(42i64)),
            timestamp_ms: None,
        };
        let e = sdk_event_to_proto(&b);
        assert!(matches!(
            e.value,
            Some(MetricValue {
                value: Some(MetricVal::IntValue(42))
            })
        ));
    }

    #[test]
    fn sdk_event_to_proto_double() {
        let b = SdkEventBody {
            metric_key: "k".to_string(),
            context_key: "u1".to_string(),
            context_type: "user".to_string(),
            value: Some(serde_json::json!(std::f64::consts::PI)),
            timestamp_ms: None,
        };
        let e = sdk_event_to_proto(&b);
        assert!(matches!(
            e.value,
            Some(MetricValue {
                value: Some(MetricVal::DoubleValue(_))
            })
        ));
    }

    #[test]
    fn sdk_event_to_proto_no_value() {
        let b = SdkEventBody {
            metric_key: "k".to_string(),
            context_key: "u1".to_string(),
            context_type: "user".to_string(),
            value: None,
            timestamp_ms: None,
        };
        let e = sdk_event_to_proto(&b);
        assert!(e.value.is_none());
    }

    #[tokio::test]
    async fn evaluate_returns_200() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/evaluate")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"context_key":"u1","context_type":"user"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ingest_batch_returns_202_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/events/batch")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"[{"metric_key":"k","context_key":"u1","context_type":"user"}]"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.status() == StatusCode::ACCEPTED || resp.status() == StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn list_check_returns_200_or_502() {
        let state = make_stub_state();
        let app = test_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/environments/env-1/segments/list-check")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"segment_key":"s1","context_key":"u1","context_type":"user"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY);
    }
}
