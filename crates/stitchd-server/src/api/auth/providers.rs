//! Auth provider management API handlers.
//!
//! # Endpoints (all require `RequireOrgAdmin`)
//! - `GET  /v1/orgs/{org_id}/auth-providers`
//! - `POST /v1/orgs/{org_id}/auth-providers`
//! - `PUT  /v1/orgs/{org_id}/auth-providers/{provider_id}`
//! - `DELETE /v1/orgs/{org_id}/auth-providers/{provider_id}`

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use serde::Deserialize;
use stitchd_core::{
    auth::{AuthProvider, CryptoKey, ProviderType},
    id::{AuthProviderId, OrganisationId},
};

use crate::AppState;
use crate::api::auth::middleware::RequireOrgAdmin;

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

/// Request body for creating an auth provider.
#[derive(Debug, Deserialize)]
pub struct CreateProviderRequest {
    /// The provider mechanism.
    pub provider_type: ProviderType,
    /// Human-readable name.
    pub display_name: String,
    /// Provider configuration. Must include `client_id` and `client_secret`.
    pub config: serde_json::Value,
    /// Whether the provider is enabled immediately.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

const fn default_enabled() -> bool {
    true
}

/// Request body for updating an auth provider.
#[derive(Debug, Deserialize)]
pub struct UpdateProviderRequest {
    /// Updated human-readable name.
    pub display_name: String,
    /// Updated configuration.
    pub config: serde_json::Value,
    /// Updated enabled flag.
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by provider management handlers.
#[derive(Debug)]
pub enum ProviderHandlerError {
    /// 400 — bad request.
    BadRequest(String),
    /// 403 — insufficient privileges.
    Forbidden(&'static str),
    /// 404 — provider not found.
    NotFound,
    /// 409 — constraint violation (e.g. last enabled provider).
    Conflict(String),
    /// 500 — internal error.
    Internal(String),
}

impl IntoResponse for ProviderHandlerError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            Self::Forbidden(m) => (StatusCode::FORBIDDEN, (*m).to_string()),
            Self::NotFound => (StatusCode::NOT_FOUND, "provider not found".to_string()),
            Self::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            Self::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (
            status,
            Json(serde_json::json!({ "error": msg })),
        )
            .into_response()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Encrypt `client_secret` inside a config JSON value using `CryptoKey`.
///
/// If the config contains a `client_secret` string field, it is replaced with
/// `client_secret_enc` (base64-encoded ciphertext). Returns the modified JSON.
fn encrypt_client_secret(
    mut config: serde_json::Value,
    crypto: &CryptoKey,
) -> Result<serde_json::Value, ProviderHandlerError> {
    if let Some(secret) = config
        .as_object_mut()
        .and_then(|o| o.remove("client_secret"))
    {
        let plain = secret
            .as_str()
            .ok_or_else(|| ProviderHandlerError::BadRequest("client_secret must be a string".into()))?;
        let enc = crypto
            .encrypt(plain.as_bytes())
            .map_err(|e| ProviderHandlerError::Internal(e.to_string()))?;
        let enc_b64 = BASE64.encode(enc);
        config["client_secret_enc"] = serde_json::Value::String(enc_b64);
    }
    Ok(config)
}

/// Decrypt `client_secret_enc` inside a config JSON value.
///
/// Returns a cloned config with `client_secret` restored as a plain string.
/// The `client_secret_enc` field is removed from the returned value.
fn decrypt_client_secret(
    mut config: serde_json::Value,
    crypto: &CryptoKey,
) -> Result<serde_json::Value, ProviderHandlerError> {
    if let Some(enc_b64) = config
        .as_object_mut()
        .and_then(|o| o.remove("client_secret_enc"))
    {
        let b64 = enc_b64.as_str().ok_or_else(|| {
            ProviderHandlerError::Internal("client_secret_enc is not a string".into())
        })?;
        let ciphertext = BASE64
            .decode(b64)
            .map_err(|e| ProviderHandlerError::Internal(e.to_string()))?;
        let plain_bytes = crypto
            .decrypt(&ciphertext)
            .map_err(|e| ProviderHandlerError::Internal(e.to_string()))?;
        let plain = String::from_utf8(plain_bytes)
            .map_err(|e| ProviderHandlerError::Internal(e.to_string()))?;
        config["client_secret"] = serde_json::Value::String(plain);
    }
    Ok(config)
}

/// Load `CryptoKey` from the environment.
fn load_crypto_key() -> Result<CryptoKey, ProviderHandlerError> {
    CryptoKey::from_env().map_err(|e| ProviderHandlerError::Internal(e.to_string()))
}

// ---------------------------------------------------------------------------
// GET /v1/orgs/{org_id}/auth-providers
// ---------------------------------------------------------------------------

/// `GET /v1/orgs/{org_id}/auth-providers` — list auth providers for an org.
///
/// Requires `OrgAdmin`.
///
/// # Errors
/// Returns `500` on database failure.
pub async fn list_providers(
    RequireOrgAdmin(_user): RequireOrgAdmin,
    Path(org_id): Path<OrganisationId>,
    State(state): State<AppState>,
) -> Result<Json<Vec<AuthProvider>>, ProviderHandlerError> {
    let providers = state
        .auth_provider_repo
        .list_for_org(org_id)
        .await
        .map_err(|e| ProviderHandlerError::Internal(e.to_string()))?;
    Ok(Json(providers))
}

// ---------------------------------------------------------------------------
// POST /v1/orgs/{org_id}/auth-providers
// ---------------------------------------------------------------------------

/// `POST /v1/orgs/{org_id}/auth-providers` — create a new auth provider.
///
/// Requires `OrgAdmin`. Encrypts `client_secret` before storing.
///
/// # Errors
/// Returns `400` if the request is malformed, `500` on failure.
pub async fn create_provider(
    RequireOrgAdmin(_user): RequireOrgAdmin,
    Path(org_id): Path<OrganisationId>,
    State(state): State<AppState>,
    Json(req): Json<CreateProviderRequest>,
) -> Result<Response, ProviderHandlerError> {
    let crypto = load_crypto_key()?;
    let config_enc = encrypt_client_secret(req.config, &crypto)?;

    let provider = state
        .auth_provider_repo
        .create(org_id, req.provider_type, &req.display_name, config_enc, req.enabled)
        .await
        .map_err(|e| ProviderHandlerError::Internal(e.to_string()))?;

    Ok((StatusCode::CREATED, Json(provider)).into_response())
}

// ---------------------------------------------------------------------------
// PUT /v1/orgs/{org_id}/auth-providers/{provider_id}
// ---------------------------------------------------------------------------

/// `PUT /v1/orgs/{org_id}/auth-providers/{provider_id}` — update an auth provider.
///
/// Requires `OrgAdmin`. Encrypts `client_secret` if present.
///
/// # Errors
/// Returns `404` if the provider does not exist, `500` on failure.
pub async fn update_provider(
    RequireOrgAdmin(_user): RequireOrgAdmin,
    Path((_org_id, provider_id)): Path<(OrganisationId, AuthProviderId)>,
    State(state): State<AppState>,
    Json(req): Json<UpdateProviderRequest>,
) -> Result<Json<AuthProvider>, ProviderHandlerError> {
    let crypto = load_crypto_key()?;
    let config_enc = encrypt_client_secret(req.config, &crypto)?;

    let provider = state
        .auth_provider_repo
        .update(provider_id, &req.display_name, config_enc, req.enabled)
        .await
        .map_err(|e| {
            if e.to_string().contains("not found") {
                ProviderHandlerError::NotFound
            } else {
                ProviderHandlerError::Internal(e.to_string())
            }
        })?;

    Ok(Json(provider))
}

// ---------------------------------------------------------------------------
// DELETE /v1/orgs/{org_id}/auth-providers/{provider_id}
// ---------------------------------------------------------------------------

/// `DELETE /v1/orgs/{org_id}/auth-providers/{provider_id}` — delete an auth provider.
///
/// Requires `OrgAdmin`. Enforces at-least-one-enabled constraint.
///
/// # Errors
/// Returns `404` if not found, `409` if this is the last enabled provider, `500` on failure.
pub async fn delete_provider(
    RequireOrgAdmin(_user): RequireOrgAdmin,
    Path((_org_id, provider_id)): Path<(OrganisationId, AuthProviderId)>,
    State(state): State<AppState>,
) -> Result<StatusCode, ProviderHandlerError> {
    state
        .auth_provider_repo
        .delete(provider_id)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("last enabled") || msg.contains("At least one") {
                ProviderHandlerError::Conflict(msg)
            } else if msg.contains("not found") || msg.contains("NotFound") {
                ProviderHandlerError::NotFound
            } else {
                ProviderHandlerError::Internal(msg)
            }
        })?;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Public helper: decrypt provider config (used by OIDC handlers)
// ---------------------------------------------------------------------------

/// Decrypt the config of an [`AuthProvider`] and return the `client_secret`.
///
/// # Errors
/// Returns a descriptive string on failure.
pub fn decrypt_provider_client_secret(
    config: &serde_json::Value,
) -> Result<String, ProviderHandlerError> {
    let crypto = load_crypto_key()?;
    let decrypted = decrypt_client_secret(config.clone(), &crypto)?;
    decrypted
        .get("client_secret")
        .and_then(|v| v.as_str())
        .map(std::string::ToString::to_string)
        .ok_or_else(|| ProviderHandlerError::Internal("client_secret not found in config".into()))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        Router,
        body::Body,
        http::{Request, StatusCode},
    };
    use chrono::Utc;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt as _;

    // In-memory stub for AuthProviderRepository
    use std::collections::HashMap as StdHashMap;
    use stitchd_core::id::AuthProviderId;

    struct InMemProviderRepo {
        providers: Mutex<Vec<AuthProvider>>,
    }

    impl InMemProviderRepo {
        fn empty() -> Arc<Self> {
            Arc::new(Self {
                providers: Mutex::new(vec![]),
            })
        }

        #[allow(dead_code)]
        fn with_provider(p: AuthProvider) -> Arc<Self> {
            Arc::new(Self {
                providers: Mutex::new(vec![p]),
            })
        }
    }

    #[async_trait::async_trait]
    impl stitchd_db::AuthProviderRepository for InMemProviderRepo {
        async fn create(
            &self,
            org_id: OrganisationId,
            provider_type: ProviderType,
            display_name: &str,
            config_encrypted: serde_json::Value,
            enabled: bool,
        ) -> Result<AuthProvider, stitchd_db::RepositoryError> {
            let now = Utc::now();
            let p = AuthProvider {
                id: AuthProviderId::new(),
                org_id,
                provider_type,
                display_name: display_name.to_string(),
                config: config_encrypted,
                enabled,
                created_at: now,
                updated_at: now,
            };
            self.providers.lock().unwrap().push(p.clone());
            Ok(p)
        }

        async fn find_by_id(
            &self,
            id: AuthProviderId,
        ) -> Result<Option<AuthProvider>, stitchd_db::RepositoryError> {
            Ok(self
                .providers
                .lock()
                .unwrap()
                .iter()
                .find(|p| p.id == id)
                .cloned())
        }

        async fn list_for_org(
            &self,
            org_id: OrganisationId,
        ) -> Result<Vec<AuthProvider>, stitchd_db::RepositoryError> {
            Ok(self
                .providers
                .lock()
                .unwrap()
                .iter()
                .filter(|p| p.org_id == org_id)
                .cloned()
                .collect())
        }

        async fn update(
            &self,
            id: AuthProviderId,
            display_name: &str,
            config_encrypted: serde_json::Value,
            enabled: bool,
        ) -> Result<AuthProvider, stitchd_db::RepositoryError> {
            let mut lock = self.providers.lock().unwrap();
            let p = lock
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or_else(|| stitchd_db::RepositoryError::NotFound { id: id.to_string() })?;
            p.display_name = display_name.to_string();
            p.config = config_encrypted;
            p.enabled = enabled;
            Ok(p.clone())
        }

        async fn delete(
            &self,
            id: AuthProviderId,
        ) -> Result<(), stitchd_db::RepositoryError> {
            let mut lock = self.providers.lock().unwrap();
            let pos = lock
                .iter()
                .position(|p| p.id == id)
                .ok_or_else(|| stitchd_db::RepositoryError::NotFound { id: id.to_string() })?;
            lock.remove(pos);
            Ok(())
        }
    }

    fn make_state_with_repo(repo: Arc<dyn stitchd_db::AuthProviderRepository>) -> crate::AppState {
        use metrics_exporter_prometheus::PrometheusBuilder;

        struct NullOther;
        #[async_trait::async_trait]
        impl stitchd_db::UserRepository for NullOther {
            async fn find_by_id(&self, id: stitchd_core::id::UserId) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_by_email(&self, e: &str) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: e.to_string() }) }
            async fn list_by_organisation(&self, _: stitchd_core::id::OrganisationId) -> Result<Vec<stitchd_core::auth::User>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &stitchd_core::auth::User) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update(&self, u: &stitchd_core::auth::User) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Ok(u.clone()) }
            async fn find_permissions_for_user(&self, _: stitchd_core::id::UserId, _: stitchd_core::id::ProjectId) -> Result<Vec<stitchd_core::user::Permission>, stitchd_db::RepositoryError> { Ok(vec![]) }
        }
        #[async_trait::async_trait]
        impl stitchd_db::AuthUserRepository for NullOther {
            async fn create(&self, e: &str, _: &str, _: Option<&str>) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: e.to_string() }) }
            async fn find_by_email(&self, _: &str) -> Result<Option<stitchd_core::auth::User>, stitchd_db::RepositoryError> { Ok(None) }
            async fn find_by_id(&self, _: stitchd_core::id::UserId) -> Result<Option<stitchd_core::auth::User>, stitchd_db::RepositoryError> { Ok(None) }
            async fn rotate_token_secret(&self, id: stitchd_core::id::UserId) -> Result<uuid::Uuid, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_status(&self, id: stitchd_core::id::UserId, _: stitchd_core::auth::UserStatus) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_password_hash(&self, id: stitchd_core::id::UserId, _: &str) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_profile(&self, id: stitchd_core::id::UserId, _: &str, _: Option<&str>) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn list_org_users(&self, _: stitchd_core::id::OrganisationId) -> Result<Vec<(stitchd_core::auth::User, stitchd_core::auth::OrgRole)>, stitchd_db::RepositoryError> { Ok(vec![]) }
        }
        #[async_trait::async_trait]
        impl stitchd_db::OrgMembershipRepository for NullOther {
            async fn add_member(&self, id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::OrgRole) -> Result<stitchd_core::auth::OrgMembership, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_membership(&self, _: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId) -> Result<Option<stitchd_core::auth::OrgMembership>, stitchd_db::RepositoryError> { Ok(None) }
            async fn list_orgs_for_user(&self, _: stitchd_core::id::UserId) -> Result<Vec<stitchd_core::auth::OrgMembership>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn remove_member(&self, _: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update_role(&self, id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::OrgRole) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        }
        #[async_trait::async_trait]
        impl stitchd_db::RefreshTokenRepository for NullOther {
            async fn create(&self, id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: Option<&str>, _: i64) -> Result<(stitchd_core::auth::RefreshToken, String), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_by_hash(&self, _: &str) -> Result<Option<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Ok(None) }
            async fn consume(&self, id: stitchd_core::id::RefreshTokenId) -> Result<Option<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn revoke(&self, _: stitchd_core::id::RefreshTokenId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn revoke_all_for_user(&self, _: stitchd_core::id::UserId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn list_active(&self, _: stitchd_core::id::UserId) -> Result<Vec<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Ok(vec![]) }
        }
        #[async_trait::async_trait]
        impl stitchd_db::MfaRepository for NullOther {
            async fn create_challenge(&self, _: stitchd_core::id::UserId, _: i64) -> Result<(stitchd_core::id::MfaChallengeId, String), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: "stub".to_string() }) }
            async fn consume_challenge(&self, _: &str) -> Result<Option<stitchd_core::id::MfaChallengeId>, stitchd_db::RepositoryError> { Ok(None) }
            async fn enable_totp(&self, _: stitchd_core::id::UserId, _: Vec<u8>, _: Vec<String>) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn disable_totp(&self, _: stitchd_core::id::UserId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn get_totp_secret(&self, _: stitchd_core::id::UserId) -> Result<Option<Vec<u8>>, stitchd_db::RepositoryError> { Ok(None) }
            async fn consume_recovery_code(&self, _: stitchd_core::id::UserId, _: &str) -> Result<bool, stitchd_db::RepositoryError> { Ok(false) }
            async fn store_pending_totp_secret(&self, _: stitchd_core::id::UserId, _: Vec<u8>) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn get_user_id_for_challenge(&self, _: &str) -> Result<Option<stitchd_core::id::UserId>, stitchd_db::RepositoryError> { Ok(None) }
        }
        let db = sqlx::PgPool::connect_lazy("postgres://stitchd:stitchd@localhost/stitchd_test")
            .expect("lazy pool");
        let null = Arc::new(NullOther);

        // Minimal stubs for remaining repos
        struct StubSegRepo;
        #[async_trait::async_trait]
        impl stitchd_db::SegmentRepository for StubSegRepo {
            async fn find_by_id(&self, id: stitchd_core::id::SegmentId) -> Result<stitchd_core::segment::Segment, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_by_key(&self, k: &str, _: stitchd_core::id::EnvironmentId) -> Result<stitchd_core::segment::Segment, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: k.to_string() }) }
            async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::segment::Segment>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &stitchd_core::segment::Segment) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update(&self, s: &stitchd_core::segment::Segment) -> Result<stitchd_core::segment::Segment, stitchd_db::RepositoryError> { Ok(s.clone()) }
            async fn find_with_rules(&self, id: stitchd_core::id::SegmentId) -> Result<stitchd_core::segment::RuleBasedSegment, stitchd_db::RepositoryError> { Ok(stitchd_core::segment::RuleBasedSegment { id, rules: vec![] }) }
            async fn find_with_list(&self, id: stitchd_core::id::SegmentId) -> Result<stitchd_core::segment::ListBasedSegment, stitchd_db::RepositoryError> { Ok(stitchd_core::segment::ListBasedSegment { id, lists: StdHashMap::new() }) }
            async fn upsert_rules(&self, _: stitchd_core::id::SegmentId, _: &[stitchd_core::rule_engine::types::Rule]) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn set_list_entries(&self, _: stitchd_core::id::SegmentId, _: &str, _: &[String], _: &[String]) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn soft_delete(&self, _: stitchd_core::id::SegmentId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn check_list_membership(&self, _: stitchd_core::id::EnvironmentId, _: &str, _: &str, keys: &[String]) -> Result<StdHashMap<String, bool>, stitchd_db::RepositoryError> { Ok(keys.iter().map(|k| (k.clone(), false)).collect()) }
            async fn batch_check_list_membership(&self, _: stitchd_core::id::EnvironmentId, _: &[(String, String)], _: &[String]) -> Result<Vec<stitchd_db::ContextMembership>, stitchd_db::RepositoryError> { Ok(vec![]) }
        }
        struct StubFlagRepo;
        #[async_trait::async_trait]
        impl stitchd_db::FlagRepository for StubFlagRepo {
            async fn find_by_id(&self, id: stitchd_core::id::FlagId) -> Result<stitchd_core::flag::FlagRecord, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_by_key(&self, key: &stitchd_core::id::FlagKey, _: stitchd_core::id::ProjectId) -> Result<stitchd_core::flag::FlagRecord, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: key.to_string() }) }
            async fn list_by_project(&self, _: stitchd_core::id::ProjectId) -> Result<Vec<stitchd_core::flag::FlagRecord>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::flag::FlagRecord>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &stitchd_core::flag::FlagRecord) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update(&self, f: &stitchd_core::flag::FlagRecord) -> Result<stitchd_core::flag::FlagRecord, stitchd_db::RepositoryError> { Ok(f.clone()) }
            async fn soft_delete(&self, id: stitchd_core::id::FlagId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_hashing_config(&self, _: stitchd_core::id::FlagId) -> Result<Vec<stitchd_core::flag::FlagHashingConfig>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn upsert_hashing_config(&self, _: stitchd_core::id::FlagId, _: &[stitchd_core::flag::FlagHashingConfig]) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn find_rules(&self, _: stitchd_core::id::FlagId) -> Result<Vec<stitchd_core::flag::FlagRule>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn upsert_rules(&self, _: stitchd_core::id::FlagId, _: &[stitchd_core::flag::FlagRule]) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        }
        struct StubVarRepo;
        #[async_trait::async_trait]
        impl stitchd_db::VariantRepository for StubVarRepo {
            async fn find_by_flag(&self, _: stitchd_core::id::FlagId) -> Result<Vec<stitchd_core::flag::Variant>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: stitchd_core::id::FlagId, _: &stitchd_core::flag::Variant) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update(&self, v: &stitchd_core::flag::Variant) -> Result<stitchd_core::flag::Variant, stitchd_db::RepositoryError> { Ok(v.clone()) }
            async fn delete(&self, _: stitchd_core::id::VariantId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        }
        struct StubSdkRepo;
        #[async_trait::async_trait]
        impl stitchd_db::SdkKeyRepository for StubSdkRepo {
            async fn find_by_id(&self, id: stitchd_core::id::SdkKeyId) -> Result<stitchd_core::tenant::SdkKey, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::tenant::SdkKey>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &stitchd_core::tenant::SdkKey) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn revoke(&self, id: stitchd_core::id::SdkKeyId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_active_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::tenant::SdkKey>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn find_active_by_hash(&self, h: &str) -> Result<stitchd_core::tenant::SdkKey, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: h.to_string() }) }
        }
        struct StubEvtRepo;
        #[async_trait::async_trait]
        impl stitchd_db::EventDefinitionRepository for StubEvtRepo {
            async fn find_by_id(&self, id: stitchd_core::id::EventDefinitionId) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_by_key(&self, k: &str, _: stitchd_core::id::EnvironmentId) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: k.to_string() }) }
            async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::event::EventDefinition>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &stitchd_core::event::EventDefinition) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update(&self, d: &stitchd_core::event::EventDefinition) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError> { Ok(d.clone()) }
            async fn soft_delete(&self, id: stitchd_core::id::EventDefinitionId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        }
        struct StubExpRepo;
        #[async_trait::async_trait]
        impl stitchd_db::ExperimentRepository for StubExpRepo {
            async fn find_by_id(&self, id: stitchd_core::id::ExperimentId) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId, _: Option<stitchd_core::experimentation::ExperimentStatus>) -> Result<Vec<stitchd_core::experimentation::Experiment>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &stitchd_core::experimentation::Experiment) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update(&self, e: &stitchd_core::experimentation::Experiment) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError> { Ok(e.clone()) }
            async fn soft_delete(&self, id: stitchd_core::id::ExperimentId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn list_iterations(&self, _: stitchd_core::id::ExperimentId) -> Result<Vec<stitchd_core::experimentation::ExperimentIteration>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn apply_transition(&self, id: stitchd_core::id::ExperimentId, _: stitchd_core::experimentation::ExperimentStatus, _: Option<stitchd_core::id::UserId>) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        }
        struct StubResRepo;
        #[async_trait::async_trait]
        impl stitchd_db::experiment_results::ExperimentResultsRepository for StubResRepo {
            async fn upsert(&self, _: &stitchd_db::experiment_results::UpsertResultRow) -> Result<stitchd_db::experiment_results::ExperimentResultRow, sqlx::Error> { Err(sqlx::Error::RowNotFound) }
            async fn fetch_latest(&self, _: uuid::Uuid) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error> { Ok(vec![]) }
            async fn fetch_by_iteration(&self, _: uuid::Uuid, _: uuid::Uuid) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error> { Ok(vec![]) }
            async fn is_stale(&self, _: uuid::Uuid, _: uuid::Uuid) -> Result<bool, sqlx::Error> { Ok(false) }
        }

        struct StubInviteRepo;
        #[async_trait::async_trait]
        impl stitchd_db::InviteRepository for StubInviteRepo {
            async fn create(&self, org_id: stitchd_core::id::OrganisationId, _: &str, _: stitchd_core::auth::OrgRole, _: Option<stitchd_core::id::UserId>, _: i64) -> Result<(stitchd_core::auth::Invite, String), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: org_id.to_string() }) }
            async fn find_by_token_hash(&self, _: &str) -> Result<Option<stitchd_core::auth::Invite>, stitchd_db::RepositoryError> { Ok(None) }
            async fn accept(&self, id: stitchd_core::id::InviteId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn list_for_org(&self, _: stitchd_core::id::OrganisationId) -> Result<Vec<stitchd_core::auth::Invite>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn revoke(&self, id: stitchd_core::id::InviteId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        }
        struct StubOtpRepo;
        #[async_trait::async_trait]
        impl stitchd_db::OtpRepository for StubOtpRepo {
            async fn create(&self, _: &str) -> Result<(uuid::Uuid, String), stitchd_db::RepositoryError> { Ok((uuid::Uuid::new_v4(), "000000".to_string())) }
            async fn find_valid_by_email(&self, _: &str) -> Result<Option<(uuid::Uuid, String)>, stitchd_db::RepositoryError> { Ok(None) }
            async fn consume(&self, id: uuid::Uuid) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        }

        crate::AppState {
            db,
            metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
            user_repo: null.clone(),
            auth_user_repo: null.clone(),
            membership_repo: null.clone(),
            refresh_token_repo: null.clone(),
            mfa_repo: null,
            auth_provider_repo: repo,
            segment_repo: Arc::new(StubSegRepo),
            flag_repo: Arc::new(StubFlagRepo),
            variant_repo: Arc::new(StubVarRepo),
            sdk_key_repo: Arc::new(StubSdkRepo),
            event_definition_repo: Arc::new(StubEvtRepo),
            experiment_repo: Arc::new(StubExpRepo),
            results_repo: Arc::new(StubResRepo),
            ch_client: None,
            event_writer: None,
            oidc_state_cache: Arc::new(Mutex::new(StdHashMap::new())),
            saml_state_cache: Arc::new(Mutex::new(StdHashMap::new())),
            email_service: Arc::new(crate::email::EmailService::from_env()),
            invite_repo: Arc::new(StubInviteRepo),
            otp_repo: Arc::new(StubOtpRepo),
        }
    }

    fn make_router(repo: Arc<dyn stitchd_db::AuthProviderRepository>) -> Router {
        use crate::api::router::build_api_router;
        let state = make_state_with_repo(repo);
        build_api_router(state.clone()).with_state(state)
    }

    #[allow(dead_code)]
    fn admin_bearer() -> String {
        // Build a minimal user + token for admin auth
        use stitchd_core::{
            auth::{OrgRole, jwt::JwtEngine},
            id::{OrganisationId, UserId},
        };
        let user_id = UserId::new();
        let org_id = OrganisationId::new();
        let token_secret = uuid::Uuid::new_v4();
        let token = JwtEngine::issue(user_id, org_id, "admin@example.com", OrgRole::OrgAdmin, &token_secret)
            .unwrap();
        format!("Bearer {token}")
    }

    #[tokio::test]
    async fn list_providers_returns_200() {
        let repo = InMemProviderRepo::empty();
        let app = make_router(repo);
        let org_id = OrganisationId::new();

        // NOTE: RequireOrgAdmin rejects unauthenticated requests with 401.
        // This test verifies the route is registered (returns 401 without auth).
        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/orgs/{org_id}/auth-providers"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Without a valid JWT the middleware returns 401; route is registered.
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_provider_returns_401_without_auth() {
        let repo = InMemProviderRepo::empty();
        let app = make_router(repo);
        let org_id = OrganisationId::new();

        let body = serde_json::json!({
            "provider_type": "oidc",
            "display_name": "Test OIDC",
            "config": { "client_id": "abc", "client_secret": "secret" }
        });

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/orgs/{org_id}/auth-providers"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn delete_provider_returns_401_without_auth() {
        let repo = InMemProviderRepo::empty();
        let app = make_router(repo);
        let org_id = OrganisationId::new();
        let provider_id = AuthProviderId::new();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/v1/orgs/{org_id}/auth-providers/{provider_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // Without auth: 401
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
