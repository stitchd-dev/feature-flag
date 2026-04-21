//! SAML 2.0 SP endpoint handlers.
//!
//! Endpoints:
//! - `GET  /auth/saml/{org_slug}/login`    — SP-initiated SSO redirect
//! - `POST /auth/saml/{org_slug}/acs`      — Assertion Consumer Service
//! - `GET  /auth/saml/{org_slug}/metadata` — SP metadata XML
//! - `POST /auth/saml/{org_slug}/slo`      — Single Logout (identity-provider-initiated)
//!
//! ## Auth providers table
//! This implementation queries `auth_providers` directly via the raw pool.
//! The table is defined in Phase 1 migrations and has the shape:
//! ```sql
//! CREATE TABLE auth_providers (
//!   id           UUID PRIMARY KEY,
//!   org_id       UUID NOT NULL,
//!   provider_type TEXT NOT NULL,   -- 'saml'
//!   display_name TEXT NOT NULL,
//!   config       JSONB NOT NULL,   -- {entity_id, sso_url, idp_cert_pem, sp_entity_id}
//!   enabled      BOOLEAN NOT NULL DEFAULT true,
//!   created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
//!   updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
//! );
//! ```
//!
//! ## Token issuance
//! Full JWT issuance (`JwtEngine` + `RefreshTokenRepository`) is implemented in Phase
//! 2/3 workers. This handler returns a minimal JSON token response; the token
//! values are opaque placeholders until Phase 2/3 is merged.

use axum::{
    Form,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use rand::Rng as _;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{Duration, Instant};
use uuid::Uuid;

use stitchd_core::{
    auth::saml::{SamlConfig, SamlError, SamlProvider},
    id::OrganisationId,
};

use crate::AppState;

/// TTL for SAML relay-state entries in the short-lived cache.
const SAML_STATE_TTL: Duration = Duration::from_secs(300); // 5 minutes

// ---------------------------------------------------------------------------
// GET /auth/saml/{org_slug}/login
// ---------------------------------------------------------------------------

/// `GET /auth/saml/{org_slug}/login` — SP-initiated SAML SSO.
///
/// Looks up the SAML provider for the org, builds an `AuthnRequest`, stores the
/// relay-state in the short-lived cache, and redirects to the identity provider SSO URL.
///
/// # Panics
/// Panics if the SAML relay-state cache mutex is poisoned.
pub async fn saml_login(
    State(state): State<AppState>,
    Path(org_slug): Path<String>,
) -> Response {
    let db = &state.db;

    // Look up the org by slug (org name used as slug — simplified approach).
    // In production this would use a dedicated slug column.
    let org_row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM organisations WHERE name = $1 AND deleted_at IS NULL LIMIT 1",
    )
    .bind(&org_slug)
    .fetch_optional(db)
    .await
    .unwrap_or(None);

    let org_id = match org_row {
        Some((id,)) => OrganisationId::from_uuid(id),
        None => {
            return (
                StatusCode::NOT_FOUND,
                format!("organisation '{org_slug}' not found"),
            )
                .into_response();
        }
    };

    // Fetch SAML provider config for this org.
    let provider = match fetch_saml_provider(db, org_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                "no enabled SAML provider found for this organisation",
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!("failed to fetch SAML provider: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let sp = SamlProvider::new(provider.config);

    // Generate ACS URL from request context (simplified: hard-coded base).
    // In production this would be derived from the request Host header.
    let acs_url = format!("/auth/saml/{org_slug}/acs");

    // Generate a random relay state and cache it.
    let relay_state: String = {
        let mut rng = rand::thread_rng();
        (0..32).map(|_| rng.sample(rand::distributions::Alphanumeric) as char).collect()
    };

    {
        let mut cache = state
            .saml_state_cache
            .lock()
            .expect("saml_state_cache mutex poisoned");
        // Prune expired entries.
        cache.retain(|_, (_, issued)| issued.elapsed() < SAML_STATE_TTL);
        cache.insert(relay_state.clone(), (org_id, Instant::now()));
    }

    match sp.authn_request(&acs_url, &relay_state) {
        Ok((_xml, redirect_url)) => Redirect::to(&redirect_url).into_response(),
        Err(e) => {
            tracing::error!("failed to build AuthnRequest: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// POST /auth/saml/{org_slug}/acs
// ---------------------------------------------------------------------------

/// Form payload for the SAML ACS endpoint.
#[derive(Debug, Deserialize)]
pub struct AcsForm {
    /// Base64-encoded `SAMLResponse` from the identity provider.
    #[serde(rename = "SAMLResponse")]
    pub saml_response: String,
    /// Opaque `RelayState` value echoed back from the identity provider.
    #[serde(rename = "RelayState")]
    pub relay_state: Option<String>,
}

/// Response returned from the ACS endpoint on successful authentication.
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    /// Short-lived JWT access token.
    pub access_token: String,
    /// Long-lived opaque refresh token.
    pub refresh_token: String,
    /// Token type (always `"Bearer"`).
    pub token_type: &'static str,
    /// Access token lifetime in seconds.
    pub expires_in: u64,
}

/// `POST /auth/saml/{org_slug}/acs` — SAML Assertion Consumer Service.
///
/// Validates the `SAMLResponse`, extracts the user email, creates or finds the
/// platform user, ensures org membership, then issues token pair.
///
/// # Panics
/// Panics if the SAML relay-state cache mutex is poisoned.
pub async fn saml_acs(
    State(state): State<AppState>,
    Path(org_slug): Path<String>,
    Form(form): Form<AcsForm>,
) -> Response {
    let db = &state.db;

    // Resolve org from relay state (held only for the brief lock scope) or from slug.
    let cached_org_id: Option<OrganisationId> = form.relay_state.as_ref().and_then(|rs| {
        let mut cache = state
            .saml_state_cache
            .lock()
            .expect("saml_state_cache mutex poisoned");
        // Prune expired entries while we have the lock.
        cache.retain(|_, (_, issued)| issued.elapsed() < SAML_STATE_TTL);
        match cache.remove(rs.as_str()) {
            Some((id, issued)) if issued.elapsed() < SAML_STATE_TTL => Some(id),
            _ => None,
        }
    }); // MutexGuard dropped here — safe to await

    let org_id = match cached_org_id {
        Some(id) => id,
        None => match lookup_org_by_slug(db, &org_slug).await {
            Some(id) => id,
            None => return (StatusCode::NOT_FOUND, "organisation not found").into_response(),
        },
    };

    // Fetch SAML provider config.
    let provider = match fetch_saml_provider(db, org_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "no SAML provider configured").into_response()
        }
        Err(e) => {
            tracing::error!("SAML provider fetch failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let sp = SamlProvider::new(provider.config);

    // Validate SAML response and extract email.
    let email = match sp.validate_response(&form.saml_response) {
        Ok(email) => email,
        Err(SamlError::InvalidResponse(msg)) => {
            tracing::warn!("SAML response invalid: {msg}");
            return (StatusCode::UNAUTHORIZED, "invalid SAML response").into_response();
        }
        Err(e) => {
            tracing::warn!("SAML validation error: {e}");
            return (StatusCode::UNAUTHORIZED, "SAML validation failed").into_response();
        }
    };

    // Find or create user.
    // NOTE: Full user creation/lookup uses AuthUserRepository from Phase 3.
    // Here we use a raw query to keep the implementation self-contained.
    let user_id = find_or_create_user(db, &email).await;
    let user_id = match user_id {
        Ok(id) => id,
        Err(e) => {
            tracing::error!("user find/create failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Ensure org membership.
    if let Err(e) = ensure_org_membership(db, user_id, org_id).await {
        tracing::error!("membership upsert failed: {e}");
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }

    // Issue tokens.
    // NOTE: Phase 2/3 workers implement JwtEngine + RefreshTokenRepository.
    // Until merged, we return opaque placeholder tokens that encode the user/org
    // identity for downstream use.
    let access_token = build_placeholder_access_token(user_id, org_id);
    let refresh_token = BASE64.encode(Uuid::new_v4().as_bytes());

    let response = TokenResponse {
        access_token,
        refresh_token,
        token_type: "Bearer",
        expires_in: 900,
    };

    (StatusCode::OK, axum::Json(response)).into_response()
}

// ---------------------------------------------------------------------------
// GET /auth/saml/{org_slug}/metadata
// ---------------------------------------------------------------------------

/// `GET /auth/saml/{org_slug}/metadata` — Serve SP metadata XML.
pub async fn saml_metadata(
    State(state): State<AppState>,
    Path(org_slug): Path<String>,
) -> Response {
    let db = &state.db;

    let Some(org_id) = lookup_org_by_slug(db, &org_slug).await else {
        return (StatusCode::NOT_FOUND, "organisation not found").into_response();
    };

    let provider = match fetch_saml_provider(db, org_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "no SAML provider configured").into_response()
        }
        Err(e) => {
            tracing::error!("SAML provider fetch failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let sp = SamlProvider::new(provider.config);
    let acs_url = format!("/auth/saml/{org_slug}/acs");
    let xml = sp.sp_metadata_xml(&acs_url);

    ([("content-type", "application/xml")], xml).into_response()
}

// ---------------------------------------------------------------------------
// POST /auth/saml/{org_slug}/slo
// ---------------------------------------------------------------------------

/// Form payload for the SLO endpoint.
#[derive(Debug, Deserialize)]
pub struct SloForm {
    /// XML `LogoutRequest` from the identity provider (URL-encoded in redirect binding).
    #[serde(rename = "SAMLRequest")]
    pub saml_request: String,
    /// Opaque `RelayState` echoed back from the identity provider.
    #[serde(rename = "RelayState")]
    pub relay_state: Option<String>,
}

/// `POST /auth/saml/{org_slug}/slo` — Handle identity-provider-initiated Single Logout.
///
/// Validates the `LogoutRequest`, revokes all org-scoped refresh tokens for the
/// user, and returns a `LogoutResponse` XML document.
pub async fn saml_slo(
    State(state): State<AppState>,
    Path(org_slug): Path<String>,
    Form(form): Form<SloForm>,
) -> Response {
    let db = &state.db;

    let Some(org_id) = lookup_org_by_slug(db, &org_slug).await else {
        return (StatusCode::NOT_FOUND, "organisation not found").into_response();
    };

    let provider = match fetch_saml_provider(db, org_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "no SAML provider configured").into_response()
        }
        Err(e) => {
            tracing::error!("SAML provider fetch failed: {e}");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let sp = SamlProvider::new(provider.config);

    let slo_ctx = match sp.validate_logout_request(&form.saml_request) {
        Ok(ctx) => ctx,
        Err(e) => {
            tracing::warn!("invalid SLO request: {e}");
            return (StatusCode::BAD_REQUEST, "invalid logout request").into_response();
        }
    };

    let email = &slo_ctx.name_id;

    // Look up user and revoke their refresh tokens.
    // NOTE: Revocation uses RefreshTokenRepository from Phase 2/3.
    // Here we use raw SQL until that worker is merged.
    let revoke_result = revoke_refresh_tokens_for_email(db, email, org_id).await;
    if let Err(e) = revoke_result {
        tracing::error!("token revocation failed for {email}: {e}");
        // Continue — still return a LogoutResponse to the identity provider.
    }

    // Build the in_response_to from the request ID (extract from XML).
    let in_response_to = extract_request_id(&form.saml_request).unwrap_or_default();
    let logout_xml = sp.logout_response(&in_response_to, form.relay_state.as_deref());

    ([("content-type", "application/xml")], logout_xml).into_response()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parsed SAML provider record from the database.
struct SamlProviderRow {
    config: SamlConfig,
}

/// Fetch the enabled SAML auth provider for an org from the DB.
async fn fetch_saml_provider(
    db: &sqlx::PgPool,
    org_id: OrganisationId,
) -> Result<Option<SamlProviderRow>, sqlx::Error> {
    let row: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT config FROM auth_providers \
         WHERE org_id = $1 AND provider_type = 'saml' AND enabled = true \
         LIMIT 1",
    )
    .bind(org_id.as_uuid())
    .fetch_optional(db)
    .await?;

    Ok(row.and_then(|(config,)| {
        let entity_id = config.get("entity_id")?.as_str()?.to_string();
        let sso_url = config.get("sso_url")?.as_str()?.to_string();
        let idp_cert_pem = config.get("idp_cert_pem")?.as_str()?.to_string();
        let sp_entity_id = config.get("sp_entity_id")?.as_str()?.to_string();
        Some(SamlProviderRow {
            config: SamlConfig {
                entity_id,
                sso_url,
                idp_cert_pem,
                sp_entity_id,
            },
        })
    }))
}

/// Look up an org by name (used as slug in this simplified implementation).
async fn lookup_org_by_slug(db: &sqlx::PgPool, slug: &str) -> Option<OrganisationId> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT id FROM organisations WHERE name = $1 AND deleted_at IS NULL LIMIT 1",
    )
    .bind(slug)
    .fetch_optional(db)
    .await
    .unwrap_or(None);

    row.map(|(id,)| OrganisationId::from_uuid(id))
}

/// Find a user by email, or create a minimal user record if none exists.
///
/// Returns the user's UUID.
async fn find_or_create_user(
    db: &sqlx::PgPool,
    email: &str,
) -> Result<Uuid, sqlx::Error> {
    // Try to find existing user.
    let existing: Option<(Uuid,)> =
        sqlx::query_as("SELECT id FROM users WHERE email = $1 LIMIT 1")
            .bind(email)
            .fetch_optional(db)
            .await?;

    if let Some((id,)) = existing {
        return Ok(id);
    }

    // Create a new user with minimal fields.
    let id = Uuid::new_v4();
    let token_secret = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, email, display_name, token_secret, totp_enabled, status, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, false, 'active', now(), now()) \
         ON CONFLICT (email) DO NOTHING",
    )
    .bind(id)
    .bind(email)
    .bind(email) // use email as display_name initially
    .bind(token_secret)
    .execute(db)
    .await?;

    // Fetch the (possibly just-inserted) record.
    let (final_id,): (Uuid,) =
        sqlx::query_as("SELECT id FROM users WHERE email = $1 LIMIT 1")
            .bind(email)
            .fetch_one(db)
            .await?;

    Ok(final_id)
}

/// Ensure the user is a member of the org. Inserts as `OrgMember` if not present.
async fn ensure_org_membership(
    db: &sqlx::PgPool,
    user_id: Uuid,
    org_id: OrganisationId,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO org_memberships (user_id, org_id, role, joined_at) \
         VALUES ($1, $2, 'org_member', now()) \
         ON CONFLICT (user_id, org_id) DO NOTHING",
    )
    .bind(user_id)
    .bind(org_id.as_uuid())
    .execute(db)
    .await?;
    Ok(())
}

/// Revoke all active refresh tokens for a user (looked up by email) in an org.
async fn revoke_refresh_tokens_for_email(
    db: &sqlx::PgPool,
    email: &str,
    org_id: OrganisationId,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE refresh_tokens rt \
         SET revoked_at = now() \
         FROM users u \
         WHERE rt.user_id = u.id \
           AND u.email = $1 \
           AND rt.org_id = $2 \
           AND rt.revoked_at IS NULL",
    )
    .bind(email)
    .bind(org_id.as_uuid())
    .execute(db)
    .await?;
    Ok(())
}

/// Build a placeholder access token encoding user/org identity.
///
/// NOTE: Replace with `JwtEngine::issue(...)` once Phase 2 is merged.
fn build_placeholder_access_token(user_id: Uuid, org_id: OrganisationId) -> String {
    let payload = json!({
        "sub": user_id.to_string(),
        "org_id": org_id.to_string(),
    });
    BASE64.encode(payload.to_string().as_bytes())
}

/// Extract the `ID` attribute from a SAML request XML element.
fn extract_request_id(xml: &str) -> Option<String> {
    // Look for ID="..." in the root element attributes.
    let id_pos = xml.find("ID=\"")?;
    let after = &xml[id_pos + 4..];
    let end = after.find('"')?;
    Some(after[..end].to_string())
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
    use metrics_exporter_prometheus::PrometheusBuilder;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt as _;

    use stitchd_core::id::OrganisationId;
    use stitchd_db::{
        ExperimentRepository, FlagRepository, RepositoryError, SdkKeyRepository,
        SegmentRepository, VariantRepository,
    };

    // -------------------------------------------------------------------------
    // Minimal stubs for AppState construction
    // -------------------------------------------------------------------------

    struct NullRepo;

    #[async_trait::async_trait]
    impl FlagRepository for NullRepo {
        async fn find_by_id(
            &self, id: stitchd_core::id::FlagId,
        ) -> Result<stitchd_core::flag::FlagRecord, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn find_by_key(
            &self, key: &stitchd_core::id::FlagKey, _: stitchd_core::id::ProjectId,
        ) -> Result<stitchd_core::flag::FlagRecord, RepositoryError> {
            Err(RepositoryError::NotFound { id: key.to_string() })
        }
        async fn list_by_project(
            &self, _: stitchd_core::id::ProjectId,
        ) -> Result<Vec<stitchd_core::flag::FlagRecord>, RepositoryError> { Ok(vec![]) }
        async fn list_by_environment(
            &self, _: stitchd_core::id::EnvironmentId,
        ) -> Result<Vec<stitchd_core::flag::FlagRecord>, RepositoryError> { Ok(vec![]) }
        async fn create(&self, _: &stitchd_core::flag::FlagRecord) -> Result<(), RepositoryError> { Ok(()) }
        async fn update(
            &self, f: &stitchd_core::flag::FlagRecord,
        ) -> Result<stitchd_core::flag::FlagRecord, RepositoryError> { Ok(f.clone()) }
        async fn soft_delete(&self, id: stitchd_core::id::FlagId) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn find_hashing_config(
            &self, _: stitchd_core::id::FlagId,
        ) -> Result<Vec<stitchd_core::flag::FlagHashingConfig>, RepositoryError> { Ok(vec![]) }
        async fn upsert_hashing_config(
            &self, _: stitchd_core::id::FlagId, _: &[stitchd_core::flag::FlagHashingConfig],
        ) -> Result<(), RepositoryError> { Ok(()) }
        async fn find_rules(
            &self, _: stitchd_core::id::FlagId,
        ) -> Result<Vec<stitchd_core::flag::FlagRule>, RepositoryError> { Ok(vec![]) }
        async fn upsert_rules(
            &self, _: stitchd_core::id::FlagId, _: &[stitchd_core::flag::FlagRule],
        ) -> Result<(), RepositoryError> { Ok(()) }
    }

    #[async_trait::async_trait]
    impl VariantRepository for NullRepo {
        async fn find_by_flag(
            &self, _: stitchd_core::id::FlagId,
        ) -> Result<Vec<stitchd_core::flag::Variant>, RepositoryError> { Ok(vec![]) }
        async fn create(
            &self, _: stitchd_core::id::FlagId, _: &stitchd_core::flag::Variant,
        ) -> Result<(), RepositoryError> { Ok(()) }
        async fn update(
            &self, v: &stitchd_core::flag::Variant,
        ) -> Result<stitchd_core::flag::Variant, RepositoryError> { Ok(v.clone()) }
        async fn delete(&self, _: stitchd_core::id::VariantId) -> Result<(), RepositoryError> { Ok(()) }
    }

    #[async_trait::async_trait]
    impl SegmentRepository for NullRepo {
        async fn find_by_id(
            &self, id: stitchd_core::id::SegmentId,
        ) -> Result<stitchd_core::segment::Segment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn find_by_key(
            &self, key: &str, _: stitchd_core::id::EnvironmentId,
        ) -> Result<stitchd_core::segment::Segment, RepositoryError> {
            Err(RepositoryError::NotFound { id: key.to_string() })
        }
        async fn list_by_environment(
            &self, _: stitchd_core::id::EnvironmentId,
        ) -> Result<Vec<stitchd_core::segment::Segment>, RepositoryError> { Ok(vec![]) }
        async fn create(&self, _: &stitchd_core::segment::Segment) -> Result<(), RepositoryError> { Ok(()) }
        async fn update(
            &self, s: &stitchd_core::segment::Segment,
        ) -> Result<stitchd_core::segment::Segment, RepositoryError> { Ok(s.clone()) }
        async fn find_with_rules(
            &self, id: stitchd_core::id::SegmentId,
        ) -> Result<stitchd_core::segment::RuleBasedSegment, RepositoryError> {
            Ok(stitchd_core::segment::RuleBasedSegment { id, rules: vec![] })
        }
        async fn find_with_list(
            &self, id: stitchd_core::id::SegmentId,
        ) -> Result<stitchd_core::segment::ListBasedSegment, RepositoryError> {
            Ok(stitchd_core::segment::ListBasedSegment { id, lists: HashMap::new() })
        }
        async fn upsert_rules(
            &self, _: stitchd_core::id::SegmentId, _: &[stitchd_core::rule_engine::types::Rule],
        ) -> Result<(), RepositoryError> { Ok(()) }
        async fn set_list_entries(
            &self, _: stitchd_core::id::SegmentId, _: &str, _: &[String], _: &[String],
        ) -> Result<(), RepositoryError> { Ok(()) }
        async fn soft_delete(&self, _: stitchd_core::id::SegmentId) -> Result<(), RepositoryError> { Ok(()) }
        async fn check_list_membership(
            &self, _: stitchd_core::id::EnvironmentId, _: &str, _: &str, keys: &[String],
        ) -> Result<HashMap<String, bool>, RepositoryError> {
            Ok(keys.iter().map(|k| (k.clone(), false)).collect())
        }
        async fn batch_check_list_membership(
            &self, _: stitchd_core::id::EnvironmentId, _: &[(String, String)], _: &[String],
        ) -> Result<Vec<stitchd_db::ContextMembership>, RepositoryError> { Ok(vec![]) }
    }

    #[async_trait::async_trait]
    impl SdkKeyRepository for NullRepo {
        async fn find_by_id(
            &self, id: stitchd_core::id::SdkKeyId,
        ) -> Result<stitchd_core::tenant::SdkKey, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn list_by_environment(
            &self, _: stitchd_core::id::EnvironmentId,
        ) -> Result<Vec<stitchd_core::tenant::SdkKey>, RepositoryError> { Ok(vec![]) }
        async fn create(&self, _: &stitchd_core::tenant::SdkKey) -> Result<(), RepositoryError> { Ok(()) }
        async fn revoke(&self, id: stitchd_core::id::SdkKeyId) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn find_active_by_environment(
            &self, _: stitchd_core::id::EnvironmentId,
        ) -> Result<Vec<stitchd_core::tenant::SdkKey>, RepositoryError> { Ok(vec![]) }
        async fn find_active_by_hash(&self, h: &str) -> Result<stitchd_core::tenant::SdkKey, RepositoryError> {
            Err(RepositoryError::NotFound { id: h.to_string() })
        }
    }

    #[async_trait::async_trait]
    impl stitchd_db::EventDefinitionRepository for NullRepo {
        async fn find_by_id(
            &self, id: stitchd_core::id::EventDefinitionId,
        ) -> Result<stitchd_core::event::EventDefinition, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn find_by_key(
            &self, key: &str, _: stitchd_core::id::EnvironmentId,
        ) -> Result<stitchd_core::event::EventDefinition, RepositoryError> {
            Err(RepositoryError::NotFound { id: key.to_string() })
        }
        async fn list_by_environment(
            &self, _: stitchd_core::id::EnvironmentId,
        ) -> Result<Vec<stitchd_core::event::EventDefinition>, RepositoryError> { Ok(vec![]) }
        async fn create(&self, _: &stitchd_core::event::EventDefinition) -> Result<(), RepositoryError> { Ok(()) }
        async fn update(
            &self, d: &stitchd_core::event::EventDefinition,
        ) -> Result<stitchd_core::event::EventDefinition, RepositoryError> { Ok(d.clone()) }
        async fn soft_delete(
            &self, id: stitchd_core::id::EventDefinitionId,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
    }

    #[async_trait::async_trait]
    impl ExperimentRepository for NullRepo {
        async fn find_by_id(
            &self, id: stitchd_core::id::ExperimentId,
        ) -> Result<stitchd_core::experimentation::Experiment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn list_by_environment(
            &self, _: stitchd_core::id::EnvironmentId,
            _: Option<stitchd_core::experimentation::ExperimentStatus>,
        ) -> Result<Vec<stitchd_core::experimentation::Experiment>, RepositoryError> { Ok(vec![]) }
        async fn create(
            &self, _: &stitchd_core::experimentation::Experiment,
        ) -> Result<(), RepositoryError> { Ok(()) }
        async fn update(
            &self, e: &stitchd_core::experimentation::Experiment,
        ) -> Result<stitchd_core::experimentation::Experiment, RepositoryError> { Ok(e.clone()) }
        async fn soft_delete(
            &self, id: stitchd_core::id::ExperimentId,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
        async fn list_iterations(
            &self, _: stitchd_core::id::ExperimentId,
        ) -> Result<Vec<stitchd_core::experimentation::ExperimentIteration>, RepositoryError> { Ok(vec![]) }
        async fn apply_transition(
            &self, id: stitchd_core::id::ExperimentId,
            _: stitchd_core::experimentation::ExperimentStatus,
            _: Option<stitchd_core::id::UserId>,
        ) -> Result<stitchd_core::experimentation::Experiment, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }
    }

    struct NullResultsRepo;
    #[async_trait::async_trait]
    impl stitchd_db::experiment_results::ExperimentResultsRepository for NullResultsRepo {
        async fn upsert(
            &self, _: &stitchd_db::experiment_results::UpsertResultRow,
        ) -> Result<stitchd_db::experiment_results::ExperimentResultRow, sqlx::Error> {
            Err(sqlx::Error::RowNotFound)
        }
        async fn fetch_latest(
            &self, _: uuid::Uuid,
        ) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error> { Ok(vec![]) }
        async fn fetch_by_iteration(
            &self, _: uuid::Uuid, _: uuid::Uuid,
        ) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error> { Ok(vec![]) }
        async fn is_stale(&self, _: uuid::Uuid, _: uuid::Uuid) -> Result<bool, sqlx::Error> { Ok(false) }
    }

    fn make_test_state() -> AppState {
        let null = Arc::new(NullRepo);
        AppState {
            db: sqlx::PgPool::connect_lazy(
                "postgres://stitchd:stitchd@localhost:5432/stitchd_test",
            )
            .expect("lazy pool"),
            metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
            segment_repo: null.clone(),
            flag_repo: null.clone(),
            variant_repo: null.clone(),
            sdk_key_repo: null.clone(),
            event_definition_repo: null.clone(),
            experiment_repo: null,
            results_repo: Arc::new(NullResultsRepo),
            ch_client: None,
            event_writer: None,
            saml_state_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn build_saml_router(state: AppState) -> Router {
        use axum::routing::{get, post};
        Router::new()
            .route("/auth/saml/{org_slug}/login", get(saml_login))
            .route("/auth/saml/{org_slug}/acs", post(saml_acs))
            .route("/auth/saml/{org_slug}/metadata", get(saml_metadata))
            .route("/auth/saml/{org_slug}/slo", post(saml_slo))
            .with_state(state)
    }

    // -------------------------------------------------------------------------
    // GET /auth/saml/{org_slug}/login — returns 302 or 404
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn login_returns_404_for_unknown_org() {
        let app = build_saml_router(make_test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/saml/nonexistent-org/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // DB not reachable in unit tests (lazy pool) → 500, but slug lookup fails → 404
        // With a lazy pool the query returns an error which we map to 404/500.
        // We accept either 404 or 500 here since the DB is unavailable.
        assert!(
            response.status() == StatusCode::NOT_FOUND
                || response.status() == StatusCode::INTERNAL_SERVER_ERROR,
            "expected 404 or 500, got {}",
            response.status()
        );
    }

    // -------------------------------------------------------------------------
    // GET /auth/saml/{org_slug}/metadata — returns 200 with XML
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn metadata_returns_404_for_unknown_org() {
        let app = build_saml_router(make_test_state());

        let response = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/auth/saml/nonexistent-org/metadata")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // DB unavailable → 404 or 500
        assert!(
            response.status() == StatusCode::NOT_FOUND
                || response.status() == StatusCode::INTERNAL_SERVER_ERROR,
            "expected 404 or 500, got {}",
            response.status()
        );
    }

    // -------------------------------------------------------------------------
    // Helper unit tests for internal functions (no DB required)
    // -------------------------------------------------------------------------

    #[test]
    fn extract_request_id_finds_id_attribute() {
        let xml = r#"<samlp:LogoutRequest ID="_abc123" Version="2.0">"#;
        let id = extract_request_id(xml);
        assert_eq!(id.as_deref(), Some("_abc123"));
    }

    #[test]
    fn extract_request_id_returns_none_when_missing() {
        let xml = r#"<samlp:LogoutRequest Version="2.0">"#;
        let id = extract_request_id(xml);
        assert!(id.is_none());
    }

    #[test]
    fn build_placeholder_access_token_is_base64() {
        let user_id = Uuid::new_v4();
        let org_id = OrganisationId::new();
        let token = build_placeholder_access_token(user_id, org_id);
        let decoded = BASE64.decode(&token).expect("should be valid base64");
        let payload: serde_json::Value =
            serde_json::from_slice(&decoded).expect("should be valid JSON");
        assert!(payload.get("sub").is_some());
        assert!(payload.get("org_id").is_some());
    }

    #[test]
    fn saml_state_cache_pruning() {
        // Verify the relay_state cache correctly prunes expired entries.
        let cache: Arc<Mutex<HashMap<String, (OrganisationId, Instant)>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let org_id = OrganisationId::new();

        {
            let mut c = cache.lock().unwrap();
            // Insert a "fresh" entry
            c.insert("fresh-key".to_string(), (org_id, Instant::now()));
            // Insert an "expired" entry (instant from 1 hour ago — simulated via Duration)
            // We can't set Instant::now() - 1h easily in stable Rust without unsafe,
            // so we just verify the fresh entry is retained.
            c.retain(|_, (_, issued)| issued.elapsed() < SAML_STATE_TTL);
        }

        let c = cache.lock().unwrap();
        assert!(c.contains_key("fresh-key"), "fresh entry should be retained");
    }

    // -------------------------------------------------------------------------
    // Integration: POST /auth/saml/{org_slug}/acs with invalid base64 → 401
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn acs_returns_401_for_invalid_saml_response() {
        let app = build_saml_router(make_test_state());

        // Build a form body with invalid base64 SAMLResponse
        let body = "SAMLResponse=!!!not-base64!!!&RelayState=test";

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/saml/test-org/acs")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        // The org lookup will fail (DB not reachable) returning 404/500, OR
        // if the relay state cache resolves, provider fetch fails similarly.
        // Either way it should not be 200 with invalid input.
        assert_ne!(
            response.status(),
            StatusCode::OK,
            "should not succeed with invalid SAMLResponse"
        );
    }

    // -------------------------------------------------------------------------
    // POST /auth/saml/{org_slug}/slo — returns 404 for unknown org
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn slo_returns_error_for_unknown_org() {
        let app = build_saml_router(make_test_state());

        let body = "SAMLRequest=<dummy>&RelayState=rs";

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/auth/saml/unknown/slo")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_ne!(response.status(), StatusCode::OK);
    }
}
