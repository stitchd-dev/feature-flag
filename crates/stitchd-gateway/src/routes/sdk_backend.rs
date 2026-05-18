//! SDK-facing REST routes for the new clean SDK (`sdks/rust/`).
//!
//! These routes implement the gateway-hosted REST surface specified in
//! `sdks/spec/openapi/sdk.yaml`:
//!
//! - `POST /v1/sdk/segments/list:batch` → batch list-segment membership check
//! - `POST /v1/sdk/events:batch`        → flag evaluation event ingestion (202)
//!
//! All routes here MUST be wrapped in the `sdk_auth_middleware` from
//! [`crate::middleware::sdk_auth`] so the request reaches the handler with an
//! `SdkContext` extension carrying the resolved `environment_id`.
//!
//! ### Why a separate routes module from `routes::sdk`?
//!
//! `routes::sdk` was written for the old SDK we're replacing. Those routes
//! use environment-id path parameters and the older `auth_middleware`. The
//! new SDK contract resolves environment from the SDK key (via the gateway's
//! trust-boundary middleware) and uses fixed paths under `/v1/sdk/`. Keeping
//! them in a separate module makes the contract boundary clear in code review.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use tonic::Request as TonicRequest;

use stitchd_proto::sdk::v1::{
    BatchCheckListMembershipRequest, FlagEvaluationEvent as ProtoFlagEvaluationEvent,
    IngestSdkEvalLogRequest, MembershipQuery as ProtoMembershipQuery,
};

use crate::error::GatewayError;
use crate::middleware::sdk_auth::SdkContext;
use crate::state::GatewayState;

const ENV_ID_METADATA_KEY: &str = "x-env-id";

// ============================================================================
// REST DTOs
// ============================================================================

/// Body of `POST /v1/sdk/segments/list:batch`.
#[derive(Debug, Deserialize)]
pub struct BatchListMembershipBody {
    pub queries: Vec<MembershipQueryBody>,
}

#[derive(Debug, Deserialize)]
pub struct MembershipQueryBody {
    pub context_type: String,
    pub context_key: String,
    /// Segment UUIDs (string form) to check membership for.
    pub segment_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct BatchListMembershipResponseJson {
    pub results: Vec<MembershipResultJson>,
}

#[derive(Debug, Serialize)]
pub struct MembershipResultJson {
    pub context_type: String,
    pub context_key: String,
    /// Map from segment UUID (string) → is_member.
    pub memberships: HashMap<String, bool>,
}

/// Body of `POST /v1/sdk/events:batch`.
#[derive(Debug, Deserialize)]
pub struct IngestEventsBody {
    pub events: Vec<FlagEvaluationEventBody>,
}

/// REST form of `FlagEvaluationEvent`. Field names mirror
/// `sdks/spec/schemas/flag_evaluation_event.schema.json`.
///
/// Note: `environment_id` is **not** accepted from the SDK — the gateway
/// populates it from the resolved `SdkContext`. Any value supplied by the
/// SDK in this field is ignored (silently overwritten).
#[derive(Debug, Deserialize)]
pub struct FlagEvaluationEventBody {
    pub flag_key: String,
    #[serde(default)]
    pub flag_id: String,
    pub variant_key: String,
    pub context_type: String,
    pub context_key: String,
    pub evaluated_at: String,
    #[serde(default)]
    pub matched_rule_id: String,
    pub outcome: String,
    #[serde(default)]
    pub reasoning_included: bool,
    #[serde(default)]
    pub context_parameters: HashMap<String, serde_json::Value>,
}

// ============================================================================
// Handlers
// ============================================================================

/// `POST /v1/sdk/segments/list:batch`
///
/// Proxies to `SegmentationSdkBackendService::BatchCheckListMembership` with
/// the resolved `environment_id` in `x-env-id` gRPC metadata.
pub async fn segments_list_batch(
    State(state): State<Arc<GatewayState>>,
    req: Request,
) -> Result<Json<BatchListMembershipResponseJson>, GatewayError> {
    let sdk_ctx = require_sdk_context(&req)?.clone();
    let body: BatchListMembershipBody = read_json_body(req).await?;

    let proto_req = BatchCheckListMembershipRequest {
        queries: body
            .queries
            .into_iter()
            .map(|q| ProtoMembershipQuery {
                context_type: q.context_type,
                context_key: q.context_key,
                segment_ids: q.segment_ids,
            })
            .collect(),
    };

    let mut tonic_req = TonicRequest::new(proto_req);
    inject_env_id_metadata(&mut tonic_req, &sdk_ctx.environment_id)?;

    let resp = {
        let mut client = state.segmentation_sdk_backend_client.lock().await;
        client.batch_check_list_membership(tonic_req).await
    }
    .map_err(|s| GatewayError::Upstream(format!("batch_check_list_membership failed: {s}")))?;

    let inner = resp.into_inner();
    Ok(Json(BatchListMembershipResponseJson {
        results: inner
            .results
            .into_iter()
            .map(|r| MembershipResultJson {
                context_type: r.context_type,
                context_key: r.context_key,
                memberships: r.memberships,
            })
            .collect(),
    }))
}

/// `POST /v1/sdk/events:batch`
///
/// Proxies to `FlagSdkBackendService::IngestSdkEvalLog`. Always returns
/// `202 Accepted` on success regardless of how many individual events the
/// backend skipped (per spec — at-least-once delivery, not exactly-once).
pub async fn events_batch(
    State(state): State<Arc<GatewayState>>,
    req: Request,
) -> Result<StatusCode, GatewayError> {
    let sdk_ctx = require_sdk_context(&req)?.clone();
    let body: IngestEventsBody = read_json_body(req).await?;

    let events: Vec<ProtoFlagEvaluationEvent> = body
        .events
        .into_iter()
        .map(|e| {
            ProtoFlagEvaluationEvent {
                flag_key: e.flag_key,
                flag_id: e.flag_id,
                variant_key: e.variant_key,
                context_type: e.context_type,
                context_key: e.context_key,
                evaluated_at: e.evaluated_at,
                matched_rule_id: e.matched_rule_id,
                outcome: e.outcome,
                reasoning_included: e.reasoning_included,
                context_parameters: e
                    .context_parameters
                    .into_iter()
                    .filter_map(|(k, v)| json_value_to_proto(v).map(|pv| (k, pv)))
                    .collect(),
                // Per spec: gateway populates environment_id from SdkContext;
                // any value supplied in the body is ignored.
                environment_id: sdk_ctx.environment_id.clone(),
            }
        })
        .collect();

    let mut tonic_req = TonicRequest::new(IngestSdkEvalLogRequest { events });
    inject_env_id_metadata(&mut tonic_req, &sdk_ctx.environment_id)?;

    let mut client = state.flag_sdk_backend_client.lock().await;
    client
        .ingest_sdk_eval_log(tonic_req)
        .await
        .map_err(|s| GatewayError::Upstream(format!("ingest_sdk_eval_log failed: {s}")))?;

    Ok(StatusCode::ACCEPTED)
}

// ============================================================================
// Helpers
// ============================================================================

fn require_sdk_context(req: &Request) -> Result<&SdkContext, GatewayError> {
    req.extensions().get::<SdkContext>().ok_or_else(|| {
        // This indicates the route was wired without sdk_auth_middleware —
        // a server configuration bug, not a client error.
        GatewayError::Upstream(
            "SdkContext extension missing — route misconfigured (sdk_auth_middleware not applied)"
                .to_string(),
        )
    })
}

async fn read_json_body<T: for<'de> Deserialize<'de>>(req: Request) -> Result<T, GatewayError> {
    let bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|e| GatewayError::BadRequest(format!("failed to read request body: {e}")))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| GatewayError::BadRequest(format!("invalid JSON body: {e}")))
}

fn inject_env_id_metadata<T>(req: &mut TonicRequest<T>, env_id: &str) -> Result<(), GatewayError> {
    let value = env_id
        .parse()
        .map_err(|_| GatewayError::Upstream(format!("env_id is not valid metadata: {env_id:?}")))?;
    req.metadata_mut().insert(ENV_ID_METADATA_KEY, value);
    Ok(())
}

/// Convert serde_json::Value → proto ParameterValue. Returns None for
/// unsupported variants (null, array, object) — those values are dropped
/// from the forwarded event rather than causing the whole batch to fail.
fn json_value_to_proto(v: serde_json::Value) -> Option<stitchd_proto::common::v1::ParameterValue> {
    use stitchd_proto::common::v1::parameter_value::Value;
    let inner = match v {
        serde_json::Value::Bool(b) => Some(Value::BoolValue(b)),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Value::IntValue)
            .or_else(|| n.as_f64().map(Value::DoubleValue)),
        serde_json::Value::String(s) => Some(Value::StringValue(s)),
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            None
        }
    };
    inner.map(|v| stitchd_proto::common::v1::ParameterValue { value: Some(v) })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_value_to_proto_bool() {
        use stitchd_proto::common::v1::parameter_value::Value;
        let pv = json_value_to_proto(serde_json::Value::Bool(true)).unwrap();
        assert!(matches!(pv.value, Some(Value::BoolValue(true))));
    }

    #[test]
    fn json_value_to_proto_int() {
        use stitchd_proto::common::v1::parameter_value::Value;
        let pv = json_value_to_proto(serde_json::json!(42)).unwrap();
        assert!(matches!(pv.value, Some(Value::IntValue(42))));
    }

    #[test]
    fn json_value_to_proto_float() {
        use stitchd_proto::common::v1::parameter_value::Value;
        let pv = json_value_to_proto(serde_json::json!(2.5)).unwrap();
        assert!(matches!(pv.value, Some(Value::DoubleValue(d)) if (d - 2.5).abs() < 1e-9));
    }

    #[test]
    fn json_value_to_proto_string() {
        use stitchd_proto::common::v1::parameter_value::Value;
        let pv = json_value_to_proto(serde_json::json!("hi")).unwrap();
        assert!(matches!(pv.value, Some(Value::StringValue(s)) if s == "hi"));
    }

    #[test]
    fn json_value_to_proto_drops_null_array_object() {
        assert!(json_value_to_proto(serde_json::Value::Null).is_none());
        assert!(json_value_to_proto(serde_json::json!([1, 2])).is_none());
        assert!(json_value_to_proto(serde_json::json!({"a":1})).is_none());
    }

    #[tokio::test]
    async fn segments_list_batch_returns_500_when_sdk_context_missing() {
        // The handler MUST refuse to proxy if SdkContext isn't in extensions —
        // that means the route is misconfigured (sdk_auth_middleware wasn't
        // applied). This guards against a regression where someone forgets
        // to layer the middleware on a new SDK route.
        let state = crate::tests::helpers::make_stub_state();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/sdk/segments/list:batch")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(r#"{"queries":[]}"#))
            .unwrap();
        let result = segments_list_batch(axum::extract::State(state), req).await;
        let err = result.unwrap_err();
        assert!(format!("{err:?}").contains("SdkContext"));
    }

    #[tokio::test]
    async fn events_batch_returns_500_when_sdk_context_missing() {
        let state = crate::tests::helpers::make_stub_state();
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/sdk/events:batch")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(r#"{"events":[]}"#))
            .unwrap();
        let result = events_batch(axum::extract::State(state), req).await;
        let err = result.unwrap_err();
        assert!(format!("{err:?}").contains("SdkContext"));
    }

    #[tokio::test]
    async fn segments_list_batch_parses_body_when_context_present() {
        // With SdkContext present and a valid body, the handler attempts to
        // proxy. The stub state has no real backend, so the gRPC call will
        // fail at connect time — but we know the body parsed and the proxy
        // was attempted (the error message refers to backend call, not body
        // parse).
        let state = crate::tests::helpers::make_stub_state();
        let mut req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/sdk/segments/list:batch")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(
                r#"{"queries":[{"context_type":"user","context_key":"alice","segment_ids":[]}]}"#,
            ))
            .unwrap();
        req.extensions_mut().insert(SdkContext {
            environment_id: "00000000-0000-0000-0000-000000000001".into(),
            organisation_id: "org".into(),
            sdk_key_id: "key".into(),
        });
        let result = segments_list_batch(axum::extract::State(state), req).await;
        let err = result.unwrap_err();
        let msg = format!("{err:?}");
        // Should be a backend-call failure, NOT a body-parse or missing-context error.
        assert!(msg.contains("batch_check_list_membership"), "got {msg}");
    }

    #[tokio::test]
    async fn events_batch_rejects_malformed_body() {
        let state = crate::tests::helpers::make_stub_state();
        let mut req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/sdk/events:batch")
            .header("content-type", "application/json")
            .body(axum::body::Body::from("not-json"))
            .unwrap();
        req.extensions_mut().insert(SdkContext {
            environment_id: "00000000-0000-0000-0000-000000000001".into(),
            organisation_id: "org".into(),
            sdk_key_id: "key".into(),
        });
        let result = events_batch(axum::extract::State(state), req).await;
        let err = result.unwrap_err();
        // BadRequest, not Internal — body parse failure is a client error.
        let msg = format!("{err:?}");
        assert!(
            msg.contains("invalid JSON body") || msg.contains("BadRequest"),
            "got {msg}"
        );
    }
}
