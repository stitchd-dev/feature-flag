//! Admin route handlers — superadmin-only operations (org lifecycle).
//!
//! These routes are protected by two middleware layers:
//!   1. `auth_middleware` — validates the JWT
//!   2. `require_system_org` — rejects callers who are NOT System-org users
//!
//! Only the platform superadmin (a member of the System organisation) may call
//! these endpoints.

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use stitchd_proto::management::v1::CreateOrgRequest;

use crate::error::GatewayError;
use crate::state::GatewayState;

// ─── REST types ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateOrgBody {
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct OrgJson {
    pub org_id:   String,
    pub org_name: String,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `POST /v1/admin/orgs`
pub async fn create_org(
    State(state): State<Arc<GatewayState>>,
    Json(body): Json<CreateOrgBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(CreateOrgRequest { name: body.name });
    let mut client = state.management_client.lock().await;
    let resp = client.create_org(req).await.map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok((StatusCode::CREATED, Json(OrgJson { org_id: r.org_id, org_name: r.org_name })))
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub fn test_router(state: Arc<GatewayState>) -> axum::Router {
    use axum::routing::post;
    axum::Router::new()
        .route("/v1/admin/orgs", post(create_org))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::{Request, StatusCode}};
    use tower::ServiceExt as _;
    use crate::tests::helpers::make_stub_state;

    #[tokio::test]
    async fn create_org_returns_201_or_502() {
        let app = test_router(make_stub_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/admin/orgs")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Acme Corp"}"#))
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
}
