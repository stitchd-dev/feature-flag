//! Auth provider management REST routes.
//!
//! All routes require `Org Admin` role (enforced via RBAC middleware).
//!
//! Routes:
//! - `POST   /v1/orgs/{org_id}/auth-providers`
//! - `GET    /v1/orgs/{org_id}/auth-providers`
//! - `GET    /v1/orgs/{org_id}/auth-providers/{id}`
//! - `PUT    /v1/orgs/{org_id}/auth-providers/{id}`
//! - `DELETE /v1/orgs/{org_id}/auth-providers/{id}`
//! - `GET    /v1/orgs/{org_id}/auth-providers/{id}/saml/metadata`

use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use stitchd_proto::auth::v1::{
    CreateAuthProviderRequest, DeleteAuthProviderRequest, GetAuthProviderRequest,
    GetSamlSpMetadataRequest, ListAuthProvidersRequest, OidcConfig as ProtoOidcConfig,
    SamlConfig as ProtoSamlConfig, UpdateAuthProviderRequest,
    create_auth_provider_request, update_auth_provider_request,
};

use crate::{error::GatewayError, state::GatewayState};

// ─── REST request/response shapes ────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct OidcConfigBody {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    #[serde(default)]
    pub scopes: Vec<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SamlConfigBody {
    pub idp_metadata_url: Option<String>,
    pub idp_metadata_xml: Option<String>,
    pub name_id_format: Option<String>,
    pub sp_entity_id: String,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "provider_type", rename_all = "lowercase")]
pub enum CreateProviderBody {
    Oidc {
        display_name: String,
        #[serde(default = "default_true")]
        enabled: bool,
        config: OidcConfigBody,
    },
    Saml {
        display_name: String,
        #[serde(default = "default_true")]
        enabled: bool,
        config: SamlConfigBody,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateProviderBody {
    pub display_name: Option<String>,
    pub enabled: Option<bool>,
    pub config: Option<UpdateConfigBody>,
}

#[derive(Debug, Deserialize, ToSchema)]
#[serde(tag = "provider_type", rename_all = "lowercase")]
pub enum UpdateConfigBody {
    Oidc(OidcConfigBody),
    Saml(SamlConfigBody),
}

#[derive(Debug, Serialize, ToSchema)]
pub struct OidcConfigJson {
    pub issuer_url: String,
    pub client_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SamlConfigJson {
    pub idp_metadata_url: Option<String>,
    pub name_id_format: String,
    pub sp_entity_id: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuthProviderJson {
    pub id: String,
    pub org_id: String,
    pub provider_type: String,
    pub display_name: String,
    pub enabled: bool,
    pub acs_url: Option<String>,
    pub oidc: Option<OidcConfigJson>,
    pub saml: Option<SamlConfigJson>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct SamlMetadataJson {
    pub xml: String,
}

// ─── Proto conversion ─────────────────────────────────────────────────────────

fn proto_response_to_json(p: stitchd_proto::auth::v1::AuthProviderResponse) -> AuthProviderJson {
    use stitchd_proto::auth::v1::auth_provider_response::Config;

    let (oidc, saml) = match p.config {
        Some(Config::Oidc(o)) => (
            Some(OidcConfigJson {
                issuer_url: o.issuer_url,
                client_id: o.client_id,
            }),
            None,
        ),
        Some(Config::Saml(s)) => (
            None,
            Some(SamlConfigJson {
                idp_metadata_url: if s.idp_metadata_url.is_empty() {
                    None
                } else {
                    Some(s.idp_metadata_url)
                },
                name_id_format: s.name_id_format,
                sp_entity_id: s.sp_entity_id,
            }),
        ),
        None => (None, None),
    };

    AuthProviderJson {
        id: p.id,
        org_id: p.org_id,
        provider_type: p.provider_type,
        display_name: p.display_name,
        enabled: p.enabled,
        acs_url: if p.acs_url.is_empty() { None } else { Some(p.acs_url) },
        oidc,
        saml,
        created_at: p.created_at,
        updated_at: p.updated_at,
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// `POST /v1/orgs/{org_id}/auth-providers`
#[utoipa::path(
    post,
    path = "/v1/orgs/{org_id}/auth-providers",
    tag = "auth-providers",
    params(("org_id" = String, Path, description = "Organisation ID")),
    request_body = CreateProviderBody,
    responses(
        (status = 201, description = "Provider created", body = AuthProviderJson),
        (status = 400, description = "Invalid input"),
        (status = 403, description = "Insufficient permissions"),
    )
)]
pub async fn create_auth_provider(
    State(state): State<Arc<GatewayState>>,
    Path(org_id): Path<String>,
    Json(body): Json<CreateProviderBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let (display_name, enabled, config) = match body {
        CreateProviderBody::Oidc { display_name, enabled, config } => (
            display_name,
            enabled,
            Some(create_auth_provider_request::Config::Oidc(ProtoOidcConfig {
                issuer_url: config.issuer_url,
                client_id: config.client_id,
                client_secret: config.client_secret,
                scopes: config.scopes,
            })),
        ),
        CreateProviderBody::Saml { display_name, enabled, config } => (
            display_name,
            enabled,
            Some(create_auth_provider_request::Config::Saml(ProtoSamlConfig {
                idp_metadata_url: config.idp_metadata_url.unwrap_or_default(),
                idp_metadata_xml: config.idp_metadata_xml.unwrap_or_default(),
                name_id_format: config.name_id_format.unwrap_or_default(),
                sp_entity_id: config.sp_entity_id,
            })),
        ),
    };

    let req = tonic::Request::new(CreateAuthProviderRequest {
        org_id,
        display_name,
        enabled,
        config,
    });

    let mut client = state.auth_provider_client.lock().await;
    let resp = client
        .create_auth_provider(req)
        .await
        .map_err(GatewayError::from)?;
    let p = resp.into_inner().provider.unwrap();
    Ok((axum::http::StatusCode::CREATED, Json(proto_response_to_json(p))))
}

/// `GET /v1/orgs/{org_id}/auth-providers`
#[utoipa::path(
    get,
    path = "/v1/orgs/{org_id}/auth-providers",
    tag = "auth-providers",
    params(("org_id" = String, Path, description = "Organisation ID")),
    responses(
        (status = 200, description = "Provider list", body = Vec<AuthProviderJson>),
    )
)]
pub async fn list_auth_providers(
    State(state): State<Arc<GatewayState>>,
    Path(org_id): Path<String>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(ListAuthProvidersRequest { org_id });
    let mut client = state.auth_provider_client.lock().await;
    let resp = client
        .list_auth_providers(req)
        .await
        .map_err(GatewayError::from)?;
    let providers: Vec<AuthProviderJson> = resp
        .into_inner()
        .providers
        .into_iter()
        .map(proto_response_to_json)
        .collect();
    Ok(Json(providers))
}

/// `GET /v1/orgs/{org_id}/auth-providers/{id}`
#[utoipa::path(
    get,
    path = "/v1/orgs/{org_id}/auth-providers/{id}",
    tag = "auth-providers",
    params(
        ("org_id" = String, Path, description = "Organisation ID"),
        ("id" = String, Path, description = "Provider ID"),
    ),
    responses(
        (status = 200, description = "Provider details", body = AuthProviderJson),
        (status = 404, description = "Not found"),
    )
)]
pub async fn get_auth_provider(
    State(state): State<Arc<GatewayState>>,
    Path((_org_id, provider_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(GetAuthProviderRequest { provider_id });
    let mut client = state.auth_provider_client.lock().await;
    let resp = client
        .get_auth_provider(req)
        .await
        .map_err(GatewayError::from)?;
    let p = resp.into_inner().provider.unwrap();
    Ok(Json(proto_response_to_json(p)))
}

/// `PUT /v1/orgs/{org_id}/auth-providers/{id}`
#[utoipa::path(
    put,
    path = "/v1/orgs/{org_id}/auth-providers/{id}",
    tag = "auth-providers",
    params(
        ("org_id" = String, Path, description = "Organisation ID"),
        ("id" = String, Path, description = "Provider ID"),
    ),
    request_body = UpdateProviderBody,
    responses(
        (status = 200, description = "Provider updated", body = AuthProviderJson),
        (status = 404, description = "Not found"),
    )
)]
pub async fn update_auth_provider(
    State(state): State<Arc<GatewayState>>,
    Path((_org_id, provider_id)): Path<(String, String)>,
    Json(body): Json<UpdateProviderBody>,
) -> Result<impl IntoResponse, GatewayError> {
    let config = body.config.map(|c| match c {
        UpdateConfigBody::Oidc(o) => {
            update_auth_provider_request::Config::Oidc(ProtoOidcConfig {
                issuer_url: o.issuer_url,
                client_id: o.client_id,
                client_secret: o.client_secret,
                scopes: o.scopes,
            })
        }
        UpdateConfigBody::Saml(s) => {
            update_auth_provider_request::Config::Saml(ProtoSamlConfig {
                idp_metadata_url: s.idp_metadata_url.unwrap_or_default(),
                idp_metadata_xml: s.idp_metadata_xml.unwrap_or_default(),
                name_id_format: s.name_id_format.unwrap_or_default(),
                sp_entity_id: s.sp_entity_id,
            })
        }
    });

    let req = tonic::Request::new(UpdateAuthProviderRequest {
        provider_id,
        display_name: body.display_name.unwrap_or_default(),
        enabled: body.enabled.unwrap_or(true),
        config,
    });

    let mut client = state.auth_provider_client.lock().await;
    let resp = client
        .update_auth_provider(req)
        .await
        .map_err(GatewayError::from)?;
    let p = resp.into_inner().provider.unwrap();
    Ok(Json(proto_response_to_json(p)))
}

/// `DELETE /v1/orgs/{org_id}/auth-providers/{id}`
#[utoipa::path(
    delete,
    path = "/v1/orgs/{org_id}/auth-providers/{id}",
    tag = "auth-providers",
    params(
        ("org_id" = String, Path, description = "Organisation ID"),
        ("id" = String, Path, description = "Provider ID"),
    ),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found"),
    )
)]
pub async fn delete_auth_provider(
    State(state): State<Arc<GatewayState>>,
    Path((_org_id, provider_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(DeleteAuthProviderRequest { provider_id });
    let mut client = state.auth_provider_client.lock().await;
    client
        .delete_auth_provider(req)
        .await
        .map_err(GatewayError::from)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

/// `GET /v1/orgs/{org_id}/auth-providers/{id}/saml/metadata`
#[utoipa::path(
    get,
    path = "/v1/orgs/{org_id}/auth-providers/{id}/saml/metadata",
    tag = "auth-providers",
    params(
        ("org_id" = String, Path, description = "Organisation ID"),
        ("id" = String, Path, description = "Provider ID"),
    ),
    responses(
        (status = 200, description = "SAML SP metadata XML"),
        (status = 404, description = "Not found"),
    )
)]
pub async fn get_saml_sp_metadata(
    State(state): State<Arc<GatewayState>>,
    Path((_org_id, provider_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, GatewayError> {
    let req = tonic::Request::new(GetSamlSpMetadataRequest { provider_id });
    let mut client = state.auth_provider_client.lock().await;
    let resp = client
        .get_saml_sp_metadata(req)
        .await
        .map_err(GatewayError::from)?;
    let xml = resp.into_inner().xml;
    Ok((
        axum::http::StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/xml")],
        xml,
    ))
}
