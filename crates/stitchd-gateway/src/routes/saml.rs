//! SAML login REST routes.
//!
//! Routes:
//! - `POST /v1/auth/saml/{provider_id}/sso`      — initiate SAML SSO for a specific provider
//! - `POST /v1/orgs/{org_id}/auth/saml/sso`      — initiate SAML SSO for first enabled org provider
//! - `POST /v1/auth/saml/{provider_id}/callback`  — ACS callback (form-encoded, posted by IdP)

use axum::{
    Form, Json,
    extract::{Path, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::auth::v1::{SamlAcsRequest, SamlSsoRequest, saml_sso_request::Scope};

use crate::{error::GatewayError, state::GatewayState};

// ─── Request/response shapes ──────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct SamlSsoBody {
    #[serde(default)]
    pub acs_url: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SamlSsoJson {
    pub redirect_url: String,
    pub relay_state: String,
}

/// Form fields posted by the IdP to the ACS endpoint.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SamlAcsForm {
    #[serde(rename = "SAMLResponse")]
    pub saml_response: String,
    #[serde(rename = "RelayState")]
    pub relay_state: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SamlTokenJson {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub user_id: String,
    pub org_id: String,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `POST /v1/auth/saml/{provider_id}/sso`
#[utoipa::path(
    post,
    path = "/v1/auth/saml/{provider_id}/sso",
    tag = "saml",
    params(("provider_id" = String, Path, description = "Provider ID")),
    request_body = SamlSsoBody,
    responses(
        (status = 200, description = "IdP redirect URL + RelayState", body = SamlSsoJson),
        (status = 404, description = "Provider not found"),
    )
)]
pub async fn saml_sso_by_provider(
    State(state): State<Arc<GatewayState>>,
    Path(provider_id): Path<String>,
    Json(body): Json<SamlSsoBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(SamlSsoRequest {
        scope: Some(Scope::ProviderId(provider_id)),
        acs_url: body.acs_url,
    });
    let mut client = state.saml_login_client.lock().await;
    let resp = client
        .saml_sso_initiate(req)
        .await
        .map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok(Json(SamlSsoJson {
        redirect_url: r.redirect_url,
        relay_state: r.relay_state,
    }))
}

/// `POST /v1/orgs/{org_id}/auth/saml/sso`
#[utoipa::path(
    post,
    path = "/v1/orgs/{org_id}/auth/saml/sso",
    tag = "saml",
    params(("org_id" = String, Path, description = "Organisation ID")),
    request_body = SamlSsoBody,
    responses(
        (status = 200, description = "IdP redirect URL + RelayState", body = SamlSsoJson),
        (status = 404, description = "No enabled SAML provider for org"),
    )
)]
pub async fn saml_sso_by_org(
    State(state): State<Arc<GatewayState>>,
    Path(org_id): Path<String>,
    Json(body): Json<SamlSsoBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(SamlSsoRequest {
        scope: Some(Scope::OrgId(org_id)),
        acs_url: body.acs_url,
    });
    let mut client = state.saml_login_client.lock().await;
    let resp = client
        .saml_sso_initiate(req)
        .await
        .map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok(Json(SamlSsoJson {
        redirect_url: r.redirect_url,
        relay_state: r.relay_state,
    }))
}

/// `POST /v1/auth/saml/{provider_id}/callback`
///
/// The IdP form-posts `SAMLResponse` (base64) and `RelayState` to this ACS endpoint.
#[utoipa::path(
    post,
    path = "/v1/auth/saml/{provider_id}/callback",
    tag = "saml",
    params(("provider_id" = String, Path, description = "Provider ID")),
    request_body(content = SamlAcsForm, content_type = "application/x-www-form-urlencoded"),
    responses(
        (status = 200, description = "JWT + refresh token", body = SamlTokenJson),
        (status = 401, description = "Invalid or expired RelayState / SAML assertion"),
    )
)]
pub async fn saml_acs_callback(
    State(state): State<Arc<GatewayState>>,
    Path(provider_id): Path<String>,
    Form(form): Form<SamlAcsForm>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(SamlAcsRequest {
        provider_id,
        saml_response_b64: form.saml_response,
        relay_state: form.relay_state,
    });
    let mut client = state.saml_login_client.lock().await;
    let resp = client
        .saml_acs_callback(req)
        .await
        .map_err(GatewayError::from)?;
    let r = resp.into_inner();
    Ok(Json(SamlTokenJson {
        access_token: r.access_token,
        refresh_token: r.refresh_token,
        expires_in: r.expires_in,
        user_id: r.user_id,
        org_id: r.org_id,
    }))
}
