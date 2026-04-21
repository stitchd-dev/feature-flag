//! OIDC / OAuth2 authorization-code flow handlers.
//!
//! # Endpoints (unauthenticated)
//! - `GET /auth/oidc/{org_slug}/{provider_id}/authorize`
//!   Redirects the user to the provider's authorization URL.
//! - `GET /auth/oidc/{org_slug}/{provider_id}/callback?code=...&state=...`
//!   Exchanges the code for tokens, upserts the user, and issues a JWT pair.

use std::time::{Duration, Instant};

use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use serde::{Deserialize, Serialize};
use stitchd_core::{
    auth::{OidcProvider, OrgRole, jwt::JwtEngine},
    id::AuthProviderId,
};

use crate::AppState;
use crate::api::auth::providers::decrypt_provider_client_secret;

// ---------------------------------------------------------------------------
// TTL for OIDC state cache entries
// ---------------------------------------------------------------------------

const STATE_TTL: Duration = Duration::from_secs(600); // 10 minutes

// ---------------------------------------------------------------------------
// Path / query extractors
// ---------------------------------------------------------------------------

/// Combined path parameters for OIDC routes.
#[derive(Deserialize)]
pub struct OidcPathParams {
    /// Organisation slug (currently unused beyond routing; `provider_id` identifies the org).
    pub org_slug: String,
    /// UUID of the [`AuthProvider`] record to use.
    pub provider_id: AuthProviderId,
}

/// Query parameters sent back by the authorization server.
#[derive(Deserialize)]
pub struct CallbackQuery {
    /// Authorization code returned by the provider.
    pub code: String,
    /// CSRF state value — must match what we stored.
    pub state: String,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by OIDC flow handlers.
#[derive(Debug)]
pub enum OidcHandlerError {
    /// 400 — bad request or invalid state.
    BadRequest(String),
    /// 404 — provider not found or not enabled.
    NotFound(String),
    /// 500 — internal error.
    Internal(String),
}

impl IntoResponse for OidcHandlerError {
    fn into_response(self) -> Response {
        let (status, msg) = match &self {
            Self::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            Self::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            Self::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal error".to_string(),
            ),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

// ---------------------------------------------------------------------------
// Response type (callback success)
// ---------------------------------------------------------------------------

/// Response returned on a successful OIDC callback.
#[derive(Debug, Serialize)]
pub struct OidcTokenResponse {
    /// Short-lived JWT access token.
    pub access_token: String,
    /// Opaque refresh token (raw hex).
    pub refresh_token: String,
    /// Always `"Bearer"`.
    pub token_type: &'static str,
    /// Access token lifetime in seconds.
    pub expires_in: u64,
}

// ---------------------------------------------------------------------------
// Helper: build OidcProvider from a stored AuthProvider record
// ---------------------------------------------------------------------------

/// Build an [`OidcProvider`] from a stored provider record.
///
/// Reads `client_id` from config and decrypts `client_secret`.
/// For `oidc` providers, uses the `issuer_url` from config for discovery.
/// For `github`-typed providers (stored with `ProviderType::Oidc`), keyed by
/// `"github"` in `provider_kind` config field.
///
/// # Errors
/// Returns a descriptive string on failure.
async fn build_oidc_provider(
    provider: &stitchd_core::auth::AuthProvider,
) -> Result<OidcProvider, OidcHandlerError> {
    let config = &provider.config;

    let client_id = config
        .get("client_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| OidcHandlerError::Internal("missing client_id in provider config".into()))?
        .to_string();

    let client_secret = decrypt_provider_client_secret(config)
        .map_err(|e| OidcHandlerError::Internal(format!("decrypt error: {e:?}")))?;

    // Determine whether this is a GitHub provider or a generic OIDC provider
    // by checking for a `provider_kind` = "github" field in config.
    let provider_kind = config
        .get("provider_kind")
        .and_then(|v| v.as_str())
        .unwrap_or("oidc");

    if provider_kind == "github" {
        return Ok(OidcProvider::github(&client_id, &client_secret));
    }

    // Generic OIDC — check for an explicit issuer_url in config first;
    // fall back to Google if `provider_kind` == "google".
    if let Some(issuer_url) = config.get("issuer_url").and_then(|v| v.as_str()) {
        let oidc = OidcProvider::from_discovery(issuer_url, &client_id, &client_secret)
            .await
            .map_err(|e| OidcHandlerError::Internal(format!("OIDC discovery failed: {e}")))?;
        return Ok(oidc);
    }

    if provider_kind == "google" {
        return Ok(OidcProvider::google(&client_id, &client_secret));
    }

    Err(OidcHandlerError::Internal(
        "cannot determine OIDC provider: no issuer_url and provider_kind is not 'google' or 'github'".into(),
    ))
}

// ---------------------------------------------------------------------------
// GET /auth/oidc/{org_slug}/{provider_id}/authorize
// ---------------------------------------------------------------------------

/// `GET /auth/oidc/{org_slug}/{provider_id}/authorize`
///
/// 1. Load the provider from DB.
/// 2. Build an [`OidcProvider`] (decrypt secrets, optionally discover).
/// 3. Generate PKCE challenge + CSRF state.
/// 4. Store `state → (pkce_verifier, provider_id, issued_at)` in the short-lived cache.
/// 5. Redirect the browser to the authorization URL.
///
/// # Errors
/// Returns `404` if the provider is not found or disabled, `500` on failure.
pub async fn authorize(
    Path(params): Path<OidcPathParams>,
    State(state): State<AppState>,
) -> Result<Response, OidcHandlerError> {
    let _ = &params.org_slug; // available for future slug-based lookup

    // 1. Load provider.
    let provider_record = state
        .auth_provider_repo
        .find_by_id(params.provider_id)
        .await
        .map_err(|e| OidcHandlerError::Internal(e.to_string()))?
        .ok_or_else(|| OidcHandlerError::NotFound("auth provider not found".into()))?;

    if !provider_record.enabled {
        return Err(OidcHandlerError::NotFound("auth provider is disabled".into()));
    }

    // 2. Build OidcProvider.
    let oidc = build_oidc_provider(&provider_record).await?;

    // 3. Build redirect URI from the request (hardcoded path suffix for callback).
    // The redirect URI is constructed deterministically so the callback handler
    // can reconstruct it without storing it in the state cache.
    let redirect_uri = format!(
        "/auth/oidc/{}/{}callback",
        params.org_slug, params.provider_id
    );

    // 4. Generate authorization URL + PKCE pair.
    let (auth_url, pkce_verifier, csrf_state) = oidc
        .authorization_url(&redirect_uri)
        .map_err(|e| OidcHandlerError::Internal(format!("authorization_url error: {e}")))?;

    // 5. Store state → (pkce_verifier, provider_id) in the in-memory cache.
    {
        let mut cache = state
            .oidc_state_cache
            .lock()
            .map_err(|_| OidcHandlerError::Internal("state cache lock poisoned".into()))?;

        // Prune stale entries while we have the lock.
        let now = Instant::now();
        cache.retain(|_, (_, _, issued_at)| now.duration_since(*issued_at) < STATE_TTL);

        cache.insert(csrf_state, (pkce_verifier, params.provider_id, Instant::now()));
    }

    // 6. Redirect.
    Ok(Redirect::temporary(auth_url.as_str()).into_response())
}

// ---------------------------------------------------------------------------
// GET /auth/oidc/{org_slug}/{provider_id}/callback
// ---------------------------------------------------------------------------

/// `GET /auth/oidc/{org_slug}/{provider_id}/callback?code=...&state=...`
///
/// 1. Look up `state` in the cache → retrieve `pkce_verifier` and `provider_id`.
/// 2. Load provider, build `OidcProvider`, exchange code for email.
/// 3. Find or create user by email.
/// 4. Ensure org membership (add with `OrgMember` role if absent).
/// 5. Issue JWT + refresh token → return token pair.
///
/// # Errors
/// Returns `400` for invalid/expired state, `404` for unknown provider, `500` on failure.
pub async fn callback(
    Path(params): Path<OidcPathParams>,
    Query(query): Query<CallbackQuery>,
    State(state): State<AppState>,
) -> Result<Response, OidcHandlerError> {
    let (pkce_verifier, cached_provider_id) = pop_state_cache(&state, &query.state)?;

    if cached_provider_id != params.provider_id {
        return Err(OidcHandlerError::BadRequest(
            "provider_id mismatch in state".into(),
        ));
    }

    let provider_record = state
        .auth_provider_repo
        .find_by_id(params.provider_id)
        .await
        .map_err(|e| OidcHandlerError::Internal(e.to_string()))?
        .ok_or_else(|| OidcHandlerError::NotFound("auth provider not found".into()))?;

    if !provider_record.enabled {
        return Err(OidcHandlerError::NotFound("auth provider is disabled".into()));
    }

    let redirect_uri = format!(
        "/auth/oidc/{}/{}callback",
        params.org_slug, params.provider_id
    );

    let oidc = build_oidc_provider(&provider_record).await?;
    let email = oidc
        .exchange_code(&query.code, &pkce_verifier, &redirect_uri)
        .await
        .map_err(|e| OidcHandlerError::Internal(format!("code exchange failed: {e}")))?;

    let user = find_or_create_user(&state, &email).await?;
    let org_id = provider_record.org_id;
    let org_role = ensure_membership(&state, user.id, org_id).await?;

    let access_token =
        JwtEngine::issue(user.id, org_id, &user.email, org_role, &user.token_secret)
            .map_err(|e| OidcHandlerError::Internal(e.to_string()))?;

    let (_token, raw_refresh) = state
        .refresh_token_repo
        .create(user.id, org_id, None, 30)
        .await
        .map_err(|e| OidcHandlerError::Internal(e.to_string()))?;

    Ok((
        StatusCode::OK,
        Json(OidcTokenResponse {
            access_token,
            refresh_token: raw_refresh,
            token_type: "Bearer",
            expires_in: 900,
        }),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// Private helpers extracted from `callback` to keep line count manageable
// ---------------------------------------------------------------------------

/// Remove a state entry from the cache, validating TTL.
fn pop_state_cache(
    state: &AppState,
    csrf_state: &str,
) -> Result<(String, AuthProviderId), OidcHandlerError> {
    let mut cache = state
        .oidc_state_cache
        .lock()
        .map_err(|_| OidcHandlerError::Internal("state cache lock poisoned".into()))?;

    let entry = cache
        .remove(csrf_state)
        .ok_or_else(|| OidcHandlerError::BadRequest("invalid or expired state".into()))?;

    drop(cache);

    let (pkce_verifier, provider_id, issued_at) = entry;
    if Instant::now().duration_since(issued_at) >= STATE_TTL {
        return Err(OidcHandlerError::BadRequest(
            "state has expired; please restart the login flow".into(),
        ));
    }
    Ok((pkce_verifier, provider_id))
}

/// Find an existing user by email or create a new passwordless one.
async fn find_or_create_user(
    state: &AppState,
    email: &str,
) -> Result<stitchd_core::auth::User, OidcHandlerError> {
    if let Some(user) = state
        .auth_user_repo
        .find_by_email(email)
        .await
        .map_err(|e| OidcHandlerError::Internal(e.to_string()))?
    {
        return Ok(user);
    }

    let display_name = email.split('@').next().unwrap_or(email);
    state
        .auth_user_repo
        .create(email, display_name, None)
        .await
        .map_err(|e| OidcHandlerError::Internal(e.to_string()))
}

/// Ensure a user is a member of the org; adds them with `OrgMember` role if not.
async fn ensure_membership(
    state: &AppState,
    user_id: stitchd_core::id::UserId,
    org_id: stitchd_core::id::OrganisationId,
) -> Result<OrgRole, OidcHandlerError> {
    if let Some(m) = state
        .membership_repo
        .find_membership(user_id, org_id)
        .await
        .map_err(|e| OidcHandlerError::Internal(e.to_string()))?
    {
        return Ok(m.role);
    }

    state
        .membership_repo
        .add_member(user_id, org_id, OrgRole::OrgMember)
        .await
        .map_err(|e| OidcHandlerError::Internal(e.to_string()))?;
    Ok(OrgRole::OrgMember)
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
        routing::get,
    };
    use std::sync::{Arc, Mutex};
    use stitchd_core::id::OrganisationId;
    use tower::ServiceExt as _;
    use uuid::Uuid;

    fn make_minimal_state() -> crate::AppState {
        use metrics_exporter_prometheus::PrometheusBuilder;
        use std::collections::HashMap;

        struct NullRepo;

        #[async_trait::async_trait]
        impl stitchd_db::AuthProviderRepository for NullRepo {
            async fn create(&self, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::ProviderType, _: &str, _: serde_json::Value, _: bool) -> Result<stitchd_core::auth::AuthProvider, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::Unexpected(anyhow::anyhow!("stub")))
            }
            async fn find_by_id(&self, _: AuthProviderId) -> Result<Option<stitchd_core::auth::AuthProvider>, stitchd_db::RepositoryError> {
                Ok(None)
            }
            async fn list_for_org(&self, _: OrganisationId) -> Result<Vec<stitchd_core::auth::AuthProvider>, stitchd_db::RepositoryError> {
                Ok(vec![])
            }
            async fn update(&self, id: AuthProviderId, _: &str, _: serde_json::Value, _: bool) -> Result<stitchd_core::auth::AuthProvider, stitchd_db::RepositoryError> {
                Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() })
            }
            async fn delete(&self, _: AuthProviderId) -> Result<(), stitchd_db::RepositoryError> {
                Ok(())
            }
        }

        #[async_trait::async_trait]
        impl stitchd_db::AuthUserRepository for NullRepo {
            async fn create(&self, e: &str, _: &str, _: Option<&str>) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: e.to_string() }) }
            async fn find_by_email(&self, _: &str) -> Result<Option<stitchd_core::auth::User>, stitchd_db::RepositoryError> { Ok(None) }
            async fn find_by_id(&self, _: stitchd_core::id::UserId) -> Result<Option<stitchd_core::auth::User>, stitchd_db::RepositoryError> { Ok(None) }
            async fn rotate_token_secret(&self, id: stitchd_core::id::UserId) -> Result<uuid::Uuid, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_status(&self, id: stitchd_core::id::UserId, _: stitchd_core::auth::UserStatus) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_password_hash(&self, id: stitchd_core::id::UserId, _: &str) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn update_profile(&self, id: stitchd_core::id::UserId, _: &str, _: Option<&str>) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        }

        #[async_trait::async_trait]
        impl stitchd_db::OrgMembershipRepository for NullRepo {
            async fn add_member(&self, id: stitchd_core::id::UserId, _: OrganisationId, _: OrgRole) -> Result<stitchd_core::auth::OrgMembership, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_membership(&self, _: stitchd_core::id::UserId, _: OrganisationId) -> Result<Option<stitchd_core::auth::OrgMembership>, stitchd_db::RepositoryError> { Ok(None) }
            async fn list_orgs_for_user(&self, _: stitchd_core::id::UserId) -> Result<Vec<stitchd_core::auth::OrgMembership>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn remove_member(&self, _: stitchd_core::id::UserId, _: OrganisationId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update_role(&self, id: stitchd_core::id::UserId, _: OrganisationId, _: OrgRole) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        }

        #[async_trait::async_trait]
        impl stitchd_db::RefreshTokenRepository for NullRepo {
            async fn create(&self, id: stitchd_core::id::UserId, _: OrganisationId, _: Option<&str>, _: i64) -> Result<(stitchd_core::auth::RefreshToken, String), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_by_hash(&self, _: &str) -> Result<Option<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Ok(None) }
            async fn consume(&self, id: stitchd_core::id::RefreshTokenId) -> Result<Option<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn revoke(&self, _: stitchd_core::id::RefreshTokenId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn revoke_all_for_user(&self, _: stitchd_core::id::UserId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn list_active(&self, _: stitchd_core::id::UserId) -> Result<Vec<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Ok(vec![]) }
        }

        struct NullUserRepo;
        #[async_trait::async_trait]
        impl stitchd_db::UserRepository for NullUserRepo {
            async fn find_by_id(&self, id: stitchd_core::id::UserId) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_by_email(&self, e: &str) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: e.to_string() }) }
            async fn list_by_organisation(&self, _: OrganisationId) -> Result<Vec<stitchd_core::auth::User>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &stitchd_core::auth::User) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update(&self, u: &stitchd_core::auth::User) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Ok(u.clone()) }
            async fn find_permissions_for_user(&self, _: stitchd_core::id::UserId, _: stitchd_core::id::ProjectId) -> Result<Vec<stitchd_core::user::Permission>, stitchd_db::RepositoryError> { Ok(vec![]) }
        }

        // Minimal stub repos for AppState fields not used in OIDC tests
        macro_rules! null_flag_repo { () => {{
            struct NullFlag;
            #[async_trait::async_trait]
            impl stitchd_db::FlagRepository for NullFlag {
                async fn find_by_id(&self, id: stitchd_core::id::FlagId) -> Result<stitchd_core::flag::FlagRecord, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
                async fn find_by_key(&self, k: &stitchd_core::id::FlagKey, _: stitchd_core::id::ProjectId) -> Result<stitchd_core::flag::FlagRecord, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: k.to_string() }) }
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
            Arc::new(NullFlag) as Arc<dyn stitchd_db::FlagRepository>
        }}; }

        let null = Arc::new(NullRepo);
        let db = sqlx::PgPool::connect_lazy("postgres://stitchd:stitchd@localhost/stitchd_test")
            .expect("lazy");

        struct NullVariant;
        #[async_trait::async_trait]
        impl stitchd_db::VariantRepository for NullVariant {
            async fn find_by_flag(&self, _: stitchd_core::id::FlagId) -> Result<Vec<stitchd_core::flag::Variant>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: stitchd_core::id::FlagId, _: &stitchd_core::flag::Variant) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update(&self, v: &stitchd_core::flag::Variant) -> Result<stitchd_core::flag::Variant, stitchd_db::RepositoryError> { Ok(v.clone()) }
            async fn delete(&self, _: stitchd_core::id::VariantId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        }
        struct NullSdk;
        #[async_trait::async_trait]
        impl stitchd_db::SdkKeyRepository for NullSdk {
            async fn find_by_id(&self, id: stitchd_core::id::SdkKeyId) -> Result<stitchd_core::tenant::SdkKey, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::tenant::SdkKey>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &stitchd_core::tenant::SdkKey) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn revoke(&self, id: stitchd_core::id::SdkKeyId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_active_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::tenant::SdkKey>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn find_active_by_hash(&self, h: &str) -> Result<stitchd_core::tenant::SdkKey, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: h.to_string() }) }
        }
        struct NullEvt;
        #[async_trait::async_trait]
        impl stitchd_db::EventDefinitionRepository for NullEvt {
            async fn find_by_id(&self, id: stitchd_core::id::EventDefinitionId) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_by_key(&self, k: &str, _: stitchd_core::id::EnvironmentId) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: k.to_string() }) }
            async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::event::EventDefinition>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &stitchd_core::event::EventDefinition) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update(&self, d: &stitchd_core::event::EventDefinition) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError> { Ok(d.clone()) }
            async fn soft_delete(&self, id: stitchd_core::id::EventDefinitionId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        }
        struct NullExp;
        #[async_trait::async_trait]
        impl stitchd_db::ExperimentRepository for NullExp {
            async fn find_by_id(&self, id: stitchd_core::id::ExperimentId) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId, _: Option<stitchd_core::experimentation::ExperimentStatus>) -> Result<Vec<stitchd_core::experimentation::Experiment>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &stitchd_core::experimentation::Experiment) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update(&self, e: &stitchd_core::experimentation::Experiment) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError> { Ok(e.clone()) }
            async fn soft_delete(&self, id: stitchd_core::id::ExperimentId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn list_iterations(&self, _: stitchd_core::id::ExperimentId) -> Result<Vec<stitchd_core::experimentation::ExperimentIteration>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn apply_transition(&self, id: stitchd_core::id::ExperimentId, _: stitchd_core::experimentation::ExperimentStatus, _: Option<stitchd_core::id::UserId>) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        }
        struct NullRes;
        #[async_trait::async_trait]
        impl stitchd_db::experiment_results::ExperimentResultsRepository for NullRes {
            async fn upsert(&self, _: &stitchd_db::experiment_results::UpsertResultRow) -> Result<stitchd_db::experiment_results::ExperimentResultRow, sqlx::Error> { Err(sqlx::Error::RowNotFound) }
            async fn fetch_latest(&self, _: Uuid) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error> { Ok(vec![]) }
            async fn fetch_by_iteration(&self, _: Uuid, _: Uuid) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error> { Ok(vec![]) }
            async fn is_stale(&self, _: Uuid, _: Uuid) -> Result<bool, sqlx::Error> { Ok(false) }
        }
        struct NullSeg;
        #[async_trait::async_trait]
        impl stitchd_db::SegmentRepository for NullSeg {
            async fn find_by_id(&self, id: stitchd_core::id::SegmentId) -> Result<stitchd_core::segment::Segment, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
            async fn find_by_key(&self, k: &str, _: stitchd_core::id::EnvironmentId) -> Result<stitchd_core::segment::Segment, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: k.to_string() }) }
            async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::segment::Segment>, stitchd_db::RepositoryError> { Ok(vec![]) }
            async fn create(&self, _: &stitchd_core::segment::Segment) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn update(&self, s: &stitchd_core::segment::Segment) -> Result<stitchd_core::segment::Segment, stitchd_db::RepositoryError> { Ok(s.clone()) }
            async fn find_with_rules(&self, id: stitchd_core::id::SegmentId) -> Result<stitchd_core::segment::RuleBasedSegment, stitchd_db::RepositoryError> { Ok(stitchd_core::segment::RuleBasedSegment { id, rules: vec![] }) }
            async fn find_with_list(&self, id: stitchd_core::id::SegmentId) -> Result<stitchd_core::segment::ListBasedSegment, stitchd_db::RepositoryError> { Ok(stitchd_core::segment::ListBasedSegment { id, lists: HashMap::new() }) }
            async fn upsert_rules(&self, _: stitchd_core::id::SegmentId, _: &[stitchd_core::rule_engine::types::Rule]) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn set_list_entries(&self, _: stitchd_core::id::SegmentId, _: &str, _: &[String], _: &[String]) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn soft_delete(&self, _: stitchd_core::id::SegmentId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn check_list_membership(&self, _: stitchd_core::id::EnvironmentId, _: &str, _: &str, keys: &[String]) -> Result<HashMap<String, bool>, stitchd_db::RepositoryError> { Ok(keys.iter().map(|k| (k.clone(), false)).collect()) }
            async fn batch_check_list_membership(&self, _: stitchd_core::id::EnvironmentId, _: &[(String, String)], _: &[String]) -> Result<Vec<stitchd_db::ContextMembership>, stitchd_db::RepositoryError> { Ok(vec![]) }
        }

        crate::AppState {
            db,
            metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
            user_repo: Arc::new(NullUserRepo),
            auth_user_repo: null.clone(),
            membership_repo: null.clone(),
            refresh_token_repo: null.clone(),
            auth_provider_repo: null,
            segment_repo: Arc::new(NullSeg),
            flag_repo: null_flag_repo!(),
            variant_repo: Arc::new(NullVariant),
            sdk_key_repo: Arc::new(NullSdk),
            event_definition_repo: Arc::new(NullEvt),
            experiment_repo: Arc::new(NullExp),
            results_repo: Arc::new(NullRes),
            ch_client: None,
            event_writer: None,
            oidc_state_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[tokio::test]
    async fn authorize_returns_404_for_unknown_provider() {
        let state = make_minimal_state();
        let provider_id = AuthProviderId::new();
        let org_slug = "my-org".to_string();

        let app = Router::new()
            .route(
                "/auth/oidc/{org_slug}/{provider_id}/authorize",
                get(authorize),
            )
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/auth/oidc/{org_slug}/{provider_id}/authorize"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn callback_returns_400_for_invalid_state() {
        let state = make_minimal_state();
        let provider_id = AuthProviderId::new();
        let org_slug = "my-org".to_string();

        let app = Router::new()
            .route(
                "/auth/oidc/{org_slug}/{provider_id}/callback",
                get(callback),
            )
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/auth/oidc/{org_slug}/{provider_id}/callback?code=abc&state=bad-state"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn state_cache_evicts_expired_entries() {
        // Simulate cache pruning logic inline — verifies the TTL check.
        use std::collections::HashMap;
        let mut cache: HashMap<String, (String, AuthProviderId, Instant)> = HashMap::new();
        let expired_at = Instant::now()
            .checked_sub(Duration::from_secs(700))
            .unwrap_or_else(Instant::now);
        cache.insert(
            "old-state".to_string(),
            ("verifier".to_string(), AuthProviderId::new(), expired_at),
        );
        cache.insert(
            "new-state".to_string(),
            ("verifier2".to_string(), AuthProviderId::new(), Instant::now()),
        );

        let now = Instant::now();
        cache.retain(|_, (_, _, issued_at)| now.duration_since(*issued_at) < STATE_TTL);

        assert!(!cache.contains_key("old-state"), "expired entry should be evicted");
        assert!(cache.contains_key("new-state"), "fresh entry should be retained");
    }
}
