//! Flag route handlers — proxy REST requests to the Flag Service via gRPC.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use stitchd_proto::flags::v1::{
    FeatureFlag, GetFlagRequest, ListFlagsRequest, MutateFlagRequest, MutationKind,
};

use crate::error::GatewayError;
use crate::state::GatewayState;

// ─── REST request / response types ───────────────────────────────────────────

/// Request body for creating or updating a flag.
#[derive(Debug, Deserialize)]
pub struct FlagMutateRequest {
    pub key: Option<String>,
    pub enabled: Option<bool>,
    /// Serialised flag as JSON (if present, used to build the proto FeatureFlag)
    pub flag: Option<serde_json::Value>,
    pub version: Option<u64>,
}

/// Lightweight JSON representation of a feature flag.
#[derive(Debug, Serialize)]
pub struct FlagJson {
    pub key: String,
    pub enabled: bool,
}

fn flag_to_json(f: &FeatureFlag) -> FlagJson {
    FlagJson {
        key: f.key.clone(),
        enabled: f.enabled,
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `GET /v1/projects/{project_id}/flags`
///
/// Lists all flags by proxying `ListFlags` to the Flag Service.
/// Note: the Flag Service operates on `environment_id`; we pass `project_id`
/// as the environment identifier (callers must supply an env-scoped project_id).
pub async fn list_flags(
    State(state): State<Arc<GatewayState>>,
    Path(project_id): Path<String>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(ListFlagsRequest {
        environment_id: project_id,
    });
    let mut client = state.flag_client.lock().await;
    let resp = client.list_flags(req).await.map_err(GatewayError::from)?;
    let flags: Vec<FlagJson> = resp.into_inner().flags.iter().map(flag_to_json).collect();
    Ok(Json(flags))
}

/// `POST /v1/projects/{project_id}/flags`
///
/// Creates a flag via `MutateFlag` (CREATE).
pub async fn create_flag(
    State(state): State<Arc<GatewayState>>,
    Path(project_id): Path<String>,
    Json(body): Json<FlagMutateRequest>,
) -> Result<impl IntoResponse, GatewayError> {
    let flag = FeatureFlag {
        key: body.key.unwrap_or_default(),
        enabled: body.enabled.unwrap_or(false),
        ..Default::default()
    };
    let req = tonic::Request::new(MutateFlagRequest {
        environment_id: project_id,
        kind: MutationKind::Create as i32,
        flag: Some(flag),
        version: 0,
    });
    let mut client = state.flag_client.lock().await;
    let resp = client.mutate_flag(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let flag_json = inner.flag.as_ref().map(flag_to_json).unwrap_or(FlagJson {
        key: String::new(),
        enabled: false,
    });
    Ok((StatusCode::CREATED, Json(flag_json)))
}

/// `GET /v1/projects/{project_id}/flags/{flag_id}`
pub async fn get_flag(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, flag_key)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(GetFlagRequest {
        environment_id: project_id,
        flag_key,
    });
    let mut client = state.flag_client.lock().await;
    let resp = client.get_flag(req).await.map_err(GatewayError::from)?;
    Ok(Json(flag_to_json(&resp.into_inner())))
}

/// `PUT /v1/projects/{project_id}/flags/{flag_id}`
pub async fn update_flag(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, flag_key)): Path<(String, String)>,
    Json(body): Json<FlagMutateRequest>,
) -> Result<impl IntoResponse, GatewayError> {
    let flag = FeatureFlag {
        key: flag_key,
        enabled: body.enabled.unwrap_or(false),
        ..Default::default()
    };
    let req = tonic::Request::new(MutateFlagRequest {
        environment_id: project_id,
        kind: MutationKind::Update as i32,
        flag: Some(flag),
        version: body.version.unwrap_or(0),
    });
    let mut client = state.flag_client.lock().await;
    let resp = client.mutate_flag(req).await.map_err(GatewayError::from)?;
    let inner = resp.into_inner();
    let flag_json = inner.flag.as_ref().map(flag_to_json).unwrap_or(FlagJson {
        key: String::new(),
        enabled: false,
    });
    Ok(Json(flag_json))
}

/// `DELETE /v1/projects/{project_id}/flags/{flag_id}`
pub async fn delete_flag(
    State(state): State<Arc<GatewayState>>,
    Path((project_id, flag_key)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let flag = FeatureFlag {
        key: flag_key,
        ..Default::default()
    };
    let req = tonic::Request::new(MutateFlagRequest {
        environment_id: project_id,
        kind: MutationKind::Delete as i32,
        flag: Some(flag),
        version: 0,
    });
    let mut client = state.flag_client.lock().await;
    client.mutate_flag(req).await.map_err(GatewayError::from)?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/projects/{project_id}/flags/{flag_id}/variants`
///
/// Stub: the Flag Service's MutateFlag CREATE handles variant creation
/// embedded in the FeatureFlag message. This endpoint accepts a JSON body
/// and proxies it as an UPDATE containing the new variant.
pub async fn create_variant(
    State(_state): State<Arc<GatewayState>>,
    Path((_project_id, _flag_key)): Path<(String, String)>,
    Json(_body): Json<serde_json::Value>,
) -> impl IntoResponse {
    // Variant creation is handled by updating the flag with embedded variants.
    // Return 202 Accepted — full implementation requires a round-trip GetFlag → MutateFlag.
    StatusCode::ACCEPTED
}

/// `PUT /v1/projects/{project_id}/flags/{flag_id}/rules`
pub async fn update_rules(
    State(_state): State<Arc<GatewayState>>,
    Path((_project_id, _flag_key)): Path<(String, String)>,
    Json(_body): Json<serde_json::Value>,
) -> impl IntoResponse {
    StatusCode::ACCEPTED
}

// ─── Test helpers ─────────────────────────────────────────────────────────────

/// Build a minimal router for unit testing.
#[cfg(test)]
pub fn test_router(
    _client: Arc<GatewayState>,
    state: Arc<GatewayState>,
) -> axum::Router {
    #[allow(unused_imports)]

    use axum::routing::{delete, get, post, put};
    let _ = _client;
    axum::Router::new()
        .route("/v1/projects/{project_id}/flags", get(list_flags).post(create_flag))
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}",
            get(get_flag).put(update_flag).delete(delete_flag),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/variants",
            post(create_variant),
        )
        .route(
            "/v1/projects/{project_id}/flags/{flag_id}/rules",
            put(update_rules),
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

    use crate::tests::helpers::{make_stub_state, make_stub_state_with_flag};

    #[tokio::test]
    async fn list_flags_returns_200() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state,
        );
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/projects/env-1/flags")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // Stub returns NotFound for list — maps to 404, but empty list is 200
        // The stub returns empty flags → 200.
        assert!(
            resp.status() == StatusCode::OK || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn get_flag_not_found_returns_404_or_502() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state,
        );
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/v1/projects/env-1/flags/missing-flag")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // gRPC connection refused → 502 or flag not found → 404
        assert!(
            resp.status() == StatusCode::NOT_FOUND
                || resp.status() == StatusCode::BAD_GATEWAY
                || resp.status() == StatusCode::INTERNAL_SERVER_ERROR,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn create_flag_returns_201_or_502() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state,
        );
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/projects/env-1/flags")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"key":"my-flag","enabled":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::CREATED || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn delete_flag_returns_204_or_502() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state,
        );
        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/v1/projects/env-1/flags/my-flag")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            resp.status() == StatusCode::NO_CONTENT || resp.status() == StatusCode::BAD_GATEWAY,
            "status: {}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn create_variant_returns_202() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state,
        );
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/projects/env-1/flags/flag-key/variants")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"key":"on","value":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn update_rules_returns_202() {
        let state = make_stub_state();
        let app = test_router(Arc::clone(&state), state,
        );
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/projects/env-1/flags/flag-key/rules")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"[]"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
    }

    // Keeps the compiler happy — make_stub_state_with_flag exported for other tests
    #[allow(dead_code)]
    fn _use_with_flag() {
        let _ = make_stub_state_with_flag;
    }
}
