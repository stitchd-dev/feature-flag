//! OIDC login REST routes.
//!
//! Routes:
//! - `POST /v1/auth/oidc/{provider_id}/authorize` — initiate OIDC for a specific provider
//! - `POST /v1/orgs/{org_id}/auth/oidc/authorize` — initiate OIDC for first enabled org provider
//! - `GET  /v1/auth/oidc/{provider_id}/callback`  — exchange code → JWT + refresh token

use axum::{
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::auth::v1::{
    OidcAuthorizeRequest, OidcCallbackRequest, oidc_authorize_request::Scope,
};

use crate::{error::GatewayError, state::GatewayState};

// ─── Request/response shapes ──────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct OidcAuthorizeBody {
    pub redirect_uri: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OidcAuthorizeJson {
    pub redirect_url: String,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct OidcCallbackQuery {
    pub code: String,
    pub state: String,
    pub redirect_uri: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OidcTokenJson {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub user_id: String,
    pub org_id: String,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `POST /v1/auth/oidc/{provider_id}/authorize`
#[utoipa::path(
    post,
    path = "/v1/auth/oidc/{provider_id}/authorize",
    tag = "oidc",
    params(("provider_id" = String, Path, description = "Provider ID")),
    request_body = OidcAuthorizeBody,
    responses(
        (status = 200, description = "Redirect URL", body = OidcAuthorizeJson),
        (status = 404, description = "Provider not found"),
    )
)]
pub async fn oidc_authorize_by_provider(
    State(state): State<Arc<GatewayState>>,
    Path(provider_id): Path<String>,
    Json(body): Json<OidcAuthorizeBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(OidcAuthorizeRequest {
        scope: Some(Scope::ProviderId(provider_id)),
        redirect_uri: body.redirect_uri,
    });
    let mut client = state.oidc_login_client.lock().await;
    let resp = client
        .oidc_authorize(req)
        .await
        .map_err(GatewayError::from)?;
    let redirect_url = resp.into_inner().redirect_url;
    Ok(Json(OidcAuthorizeJson { redirect_url }))
}

/// `POST /v1/orgs/{org_id}/auth/oidc/authorize`
#[utoipa::path(
    post,
    path = "/v1/orgs/{org_id}/auth/oidc/authorize",
    tag = "oidc",
    params(("org_id" = String, Path, description = "Organisation ID")),
    request_body = OidcAuthorizeBody,
    responses(
        (status = 200, description = "Redirect URL", body = OidcAuthorizeJson),
        (status = 404, description = "No enabled OIDC provider for org"),
    )
)]
pub async fn oidc_authorize_by_org(
    State(state): State<Arc<GatewayState>>,
    Path(org_id): Path<String>,
    Json(body): Json<OidcAuthorizeBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(OidcAuthorizeRequest {
        scope: Some(Scope::OrgId(org_id)),
        redirect_uri: body.redirect_uri,
    });
    let mut client = state.oidc_login_client.lock().await;
    let resp = client
        .oidc_authorize(req)
        .await
        .map_err(GatewayError::from)?;
    let redirect_url = resp.into_inner().redirect_url;
    Ok(Json(OidcAuthorizeJson { redirect_url }))
}

/// `GET /v1/auth/oidc/{provider_id}/callback`
#[utoipa::path(
    get,
    path = "/v1/auth/oidc/{provider_id}/callback",
    tag = "oidc",
    params(
        ("provider_id" = String, Path, description = "Provider ID"),
        ("code" = String, Query, description = "Authorization code from IdP"),
        ("state" = String, Query, description = "CSRF state from IdP"),
        ("redirect_uri" = String, Query, description = "Redirect URI used in authorize"),
    ),
    responses(
        (status = 200, description = "JWT + refresh token", body = OidcTokenJson),
        (status = 401, description = "Invalid or expired state"),
    )
)]
pub async fn oidc_callback(
    State(state): State<Arc<GatewayState>>,
    Path(provider_id): Path<String>,
    Query(query): Query<OidcCallbackQuery>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(OidcCallbackRequest {
        provider_id,
        code: query.code,
        state: query.state,
        redirect_uri: query.redirect_uri,
    });
    let mut client = state.oidc_login_client.lock().await;
    let resp = client
        .oidc_callback(req)
        .await
        .map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok(Json(OidcTokenJson {
        access_token: r.access_token,
        refresh_token: r.refresh_token,
        expires_in: r.expires_in,
        user_id: r.user_id,
        org_id: r.org_id,
    }))
}
