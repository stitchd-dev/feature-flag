//! Axum extractors for JWT authentication and role-based authorisation.
//!
//! # Usage
//!
//! ```ignore
//! async fn my_handler(user: AuthenticatedUser) -> impl IntoResponse { ... }
//! async fn admin_only(RequireOrgRole(role): RequireOrgRole) -> impl IntoResponse { ... }
//! ```

use axum::{
    extract::{FromRef, FromRequestParts, Path},
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use serde_json::json;
use stitchd_core::{
    auth::{EnvRole, OrgRole, ProjectRole, UserStatus, jwt::JwtEngine},
    id::{EnvironmentId, OrganisationId, ProjectId, UserId},
};
use crate::AppState;

// ---------------------------------------------------------------------------
// AuthenticatedUser
// ---------------------------------------------------------------------------

/// Successfully authenticated caller, injected via extractor.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    /// The authenticated user's ID.
    pub user_id: UserId,
    /// The organisation the token was issued for.
    pub org_id: OrganisationId,
    /// The user's email address.
    pub email: String,
    /// The user's organisation-level role.
    pub org_role: OrgRole,
}

impl<S> FromRequestParts<S> for AuthenticatedUser
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);

        // 1. Extract Bearer token from Authorization header.
        let token = extract_bearer_token(parts)?;

        // 2. Decode without verification to get `sub` (user_id).
        let unverified = JwtEngine::decode_unverified(&token)
            .map_err(|_| AuthError::InvalidToken)?;

        let user_id: UserId = unverified
            .sub
            .parse::<uuid::Uuid>()
            .map(UserId::from_uuid)
            .map_err(|_| AuthError::InvalidToken)?;

        // 3. Fetch user from DB to get token_secret.
        let user = app_state
            .auth_user_repo
            .find_by_id(user_id)
            .await
            .map_err(|_| AuthError::InvalidToken)?
            .ok_or(AuthError::InvalidToken)?;

        // 4. Verify JWT with the per-user token_secret.
        let claims = JwtEngine::verify(&token, &user.token_secret)
            .map_err(|_| AuthError::InvalidToken)?;

        // 5. Check user is active.
        if user.status != UserStatus::Active {
            return Err(AuthError::Deactivated);
        }

        let org_id: OrganisationId = claims
            .org_id
            .parse::<uuid::Uuid>()
            .map(OrganisationId::from_uuid)
            .map_err(|_| AuthError::InvalidToken)?;

        Ok(Self {
            user_id: user.id,
            org_id,
            email: claims.email,
            org_role: claims.org_role,
        })
    }
}

// ---------------------------------------------------------------------------
// RequireOrgRole
// ---------------------------------------------------------------------------

/// Extractor that requires a minimum [`OrgRole`].
///
/// Wraps [`AuthenticatedUser`] and returns 403 if the caller's role is
/// below the required level.
pub struct RequireOrgRole(pub AuthenticatedUser);

impl RequireOrgRole {
    /// The minimum role required; callers must override this in each handler.
    #[must_use]
    pub const fn authenticated_user(&self) -> &AuthenticatedUser {
        &self.0
    }
}

/// Marker extractor that enforces `OrgAdmin`.
pub struct RequireOrgAdmin(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for RequireOrgRole
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // This variant grants access to any authenticated user — callers that
        // need a specific minimum role should use `check_org_role` directly,
        // or use a concrete wrapper like `RequireOrgAdmin`.
        let user = AuthenticatedUser::from_request_parts(parts, state).await?;
        Ok(Self(user))
    }
}

impl<S> FromRequestParts<S> for RequireOrgAdmin
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = AuthenticatedUser::from_request_parts(parts, state).await?;
        if user.org_role < OrgRole::OrgAdmin {
            return Err(AuthError::InsufficientRole);
        }
        Ok(Self(user))
    }
}

/// Check that `user.org_role >= required` — utility used by handlers.
///
/// # Errors
/// Returns [`AuthError::InsufficientRole`] if the check fails.
pub fn check_org_role(user: &AuthenticatedUser, required: OrgRole) -> Result<(), AuthError> {
    if user.org_role >= required {
        Ok(())
    } else {
        Err(AuthError::InsufficientRole)
    }
}

// ---------------------------------------------------------------------------
// RequireProjectRole
// ---------------------------------------------------------------------------

/// Extractor that requires a minimum [`ProjectRole`] for the current project.
///
/// Reads `project_id` from the request path and queries `user_project_roles`.
pub struct RequireProjectRole(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for RequireProjectRole
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);
        let user = AuthenticatedUser::from_request_parts(parts, state).await?;

        // Extract project_id from path.
        let Path(project_id) = Path::<ProjectId>::from_request_parts(parts, state)
            .await
            .map_err(|_| AuthError::MissingPathParam("project_id"))?;

        // Query user's project role.
        let role_opt = fetch_project_role(&app_state.db, user.user_id, project_id).await?;

        if role_opt.is_none() {
            return Err(AuthError::InsufficientRole);
        }

        Ok(Self(user))
    }
}

/// Extractor that requires at least [`ProjectRole::ProjectAdmin`].
pub struct RequireProjectAdmin(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for RequireProjectAdmin
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);
        let user = AuthenticatedUser::from_request_parts(parts, state).await?;

        let Path(project_id) = Path::<ProjectId>::from_request_parts(parts, state)
            .await
            .map_err(|_| AuthError::MissingPathParam("project_id"))?;

        let role_opt = fetch_project_role(&app_state.db, user.user_id, project_id).await?;

        match role_opt {
            Some(role) if role >= ProjectRole::ProjectAdmin => Ok(Self(user)),
            _ => Err(AuthError::InsufficientRole),
        }
    }
}

// ---------------------------------------------------------------------------
// RequireEnvRole
// ---------------------------------------------------------------------------

/// Extractor that requires a minimum [`EnvRole`] for the current environment.
///
/// Reads `env_id` from the request path and queries `user_env_roles`.
pub struct RequireEnvRole(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for RequireEnvRole
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);
        let user = AuthenticatedUser::from_request_parts(parts, state).await?;

        let Path(env_id) = Path::<EnvironmentId>::from_request_parts(parts, state)
            .await
            .map_err(|_| AuthError::MissingPathParam("env_id"))?;

        let role_opt = fetch_env_role(&app_state.db, user.user_id, env_id).await?;

        if role_opt.is_none() {
            return Err(AuthError::InsufficientRole);
        }

        Ok(Self(user))
    }
}

/// Extractor that requires at least [`EnvRole::EnvPublisher`].
pub struct RequireEnvPublisher(pub AuthenticatedUser);

impl<S> FromRequestParts<S> for RequireEnvPublisher
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = AppState::from_ref(state);
        let user = AuthenticatedUser::from_request_parts(parts, state).await?;

        let Path(env_id) = Path::<EnvironmentId>::from_request_parts(parts, state)
            .await
            .map_err(|_| AuthError::MissingPathParam("env_id"))?;

        let role_opt = fetch_env_role(&app_state.db, user.user_id, env_id).await?;

        match role_opt {
            Some(role) if role >= EnvRole::EnvPublisher => Ok(Self(user)),
            _ => Err(AuthError::InsufficientRole),
        }
    }
}

// ---------------------------------------------------------------------------
// DB helpers
// ---------------------------------------------------------------------------

async fn fetch_project_role(
    pool: &sqlx::PgPool,
    user_id: UserId,
    project_id: ProjectId,
) -> Result<Option<ProjectRole>, AuthError> {
    let row = sqlx::query!(
        r#"
        SELECT role FROM user_project_roles
        WHERE user_id = $1 AND project_id = $2
        "#,
        user_id.as_uuid(),
        project_id.as_uuid()
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| AuthError::Internal(e.to_string()))?;

    match row {
        None => Ok(None),
        Some(r) => {
            let role = match r.role.as_str() {
                "project_admin" => ProjectRole::ProjectAdmin,
                "project_viewer" => ProjectRole::ProjectViewer,
                other => return Err(AuthError::Internal(format!("unknown project role: {other}"))),
            };
            Ok(Some(role))
        }
    }
}

async fn fetch_env_role(
    pool: &sqlx::PgPool,
    user_id: UserId,
    env_id: EnvironmentId,
) -> Result<Option<EnvRole>, AuthError> {
    let row = sqlx::query!(
        r#"
        SELECT role FROM user_env_roles
        WHERE user_id = $1 AND env_id = $2
        "#,
        user_id.as_uuid(),
        env_id.as_uuid()
    )
    .fetch_optional(pool)
    .await
    .map_err(|e| AuthError::Internal(e.to_string()))?;

    match row {
        None => Ok(None),
        Some(r) => {
            let role = match r.role.as_str() {
                "env_publisher" => EnvRole::EnvPublisher,
                "env_viewer" => EnvRole::EnvViewer,
                other => return Err(AuthError::Internal(format!("unknown env role: {other}"))),
            };
            Ok(Some(role))
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: extract Bearer token
// ---------------------------------------------------------------------------

fn extract_bearer_token(parts: &Parts) -> Result<String, AuthError> {
    let auth_header = parts
        .headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .ok_or(AuthError::MissingToken)?;

    auth_header
        .strip_prefix("Bearer ")
        .map(str::to_owned)
        .ok_or(AuthError::MissingToken)
}

// ---------------------------------------------------------------------------
// AuthError
// ---------------------------------------------------------------------------

/// Errors from auth extraction.
#[derive(Debug)]
pub enum AuthError {
    /// No `Authorization: Bearer` header.
    MissingToken,
    /// Token is invalid, expired, or signed with wrong secret.
    InvalidToken,
    /// User account has been deactivated.
    Deactivated,
    /// User doesn't have the required role.
    InsufficientRole,
    /// A required path parameter is missing.
    MissingPathParam(&'static str),
    /// An internal error occurred (DB failure, etc.).
    Internal(String),
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            Self::MissingToken => (StatusCode::UNAUTHORIZED, "missing Authorization header"),
            Self::InvalidToken => (StatusCode::UNAUTHORIZED, "invalid or expired token"),
            Self::Deactivated => (StatusCode::FORBIDDEN, "user account is deactivated"),
            Self::InsufficientRole => (StatusCode::FORBIDDEN, "insufficient role"),
            Self::MissingPathParam(_) => (StatusCode::BAD_REQUEST, "missing path parameter"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        };
        (status, axum::Json(json!({ "error": message }))).into_response()
    }
}

// ---------------------------------------------------------------------------
// Unit tests (no DB required — uses stub state)
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
    use chrono::Utc;
    use std::sync::Arc;
    use stitchd_core::{
        auth::{User, UserStatus, jwt::JwtEngine},
        id::{OrganisationId, UserId},
    };
    use stitchd_db::{RepositoryError, UserRepository};
    use tower::ServiceExt as _;

    // -----------------------------------------------------------------------
    // Stub user repository for unit tests
    // -----------------------------------------------------------------------

    struct StubUserRepo {
        user: Option<User>,
    }

    #[async_trait::async_trait]
    impl UserRepository for StubUserRepo {
        async fn find_by_id(&self, id: UserId) -> Result<User, RepositoryError> {
            self.user.clone().ok_or(RepositoryError::NotFound {
                id: id.to_string(),
            })
        }
        async fn find_by_email(&self, email: &str) -> Result<User, RepositoryError> {
            self.user.clone().ok_or(RepositoryError::NotFound {
                id: email.to_string(),
            })
        }
        async fn list_by_organisation(
            &self,
            _: OrganisationId,
        ) -> Result<Vec<User>, RepositoryError> {
            Ok(self.user.iter().cloned().collect())
        }
        async fn create(&self, _: &User) -> Result<(), RepositoryError> {
            Ok(())
        }
        async fn update(&self, u: &User) -> Result<User, RepositoryError> {
            Ok(u.clone())
        }
        async fn find_permissions_for_user(
            &self,
            _: UserId,
            _: stitchd_core::id::ProjectId,
        ) -> Result<Vec<stitchd_core::user::Permission>, RepositoryError> {
            Ok(vec![])
        }
    }

    // -----------------------------------------------------------------------
    // Test helpers
    // -----------------------------------------------------------------------

    fn make_user(status: UserStatus) -> User {
        User {
            id: UserId::new(),
            email: "test@example.com".to_string(),
            display_name: "Test User".to_string(),
            avatar_url: None,
            password_hash: None,
            token_secret: uuid::Uuid::new_v4(),
            totp_secret: None,
            totp_enabled: false,
            status,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    struct StubOtherRepo;

    #[async_trait::async_trait]
    impl stitchd_db::FlagRepository for StubOtherRepo {
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

    #[async_trait::async_trait]
    impl stitchd_db::VariantRepository for StubOtherRepo {
        async fn find_by_flag(&self, _: stitchd_core::id::FlagId) -> Result<Vec<stitchd_core::flag::Variant>, stitchd_db::RepositoryError> { Ok(vec![]) }
        async fn create(&self, _: stitchd_core::id::FlagId, _: &stitchd_core::flag::Variant) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn update(&self, v: &stitchd_core::flag::Variant) -> Result<stitchd_core::flag::Variant, stitchd_db::RepositoryError> { Ok(v.clone()) }
        async fn delete(&self, _: stitchd_core::id::VariantId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
    }

    #[async_trait::async_trait]
    impl stitchd_db::SegmentRepository for StubOtherRepo {
        async fn find_by_id(&self, id: stitchd_core::id::SegmentId) -> Result<stitchd_core::segment::Segment, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn find_by_key(&self, key: &str, _: stitchd_core::id::EnvironmentId) -> Result<stitchd_core::segment::Segment, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: key.to_string() }) }
        async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::segment::Segment>, stitchd_db::RepositoryError> { Ok(vec![]) }
        async fn create(&self, _: &stitchd_core::segment::Segment) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn update(&self, s: &stitchd_core::segment::Segment) -> Result<stitchd_core::segment::Segment, stitchd_db::RepositoryError> { Ok(s.clone()) }
        async fn find_with_rules(&self, id: stitchd_core::id::SegmentId) -> Result<stitchd_core::segment::RuleBasedSegment, stitchd_db::RepositoryError> { Ok(stitchd_core::segment::RuleBasedSegment { id, rules: vec![] }) }
        async fn find_with_list(&self, id: stitchd_core::id::SegmentId) -> Result<stitchd_core::segment::ListBasedSegment, stitchd_db::RepositoryError> { Ok(stitchd_core::segment::ListBasedSegment { id, lists: std::collections::HashMap::new() }) }
        async fn upsert_rules(&self, _: stitchd_core::id::SegmentId, _: &[stitchd_core::rule_engine::types::Rule]) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn set_list_entries(&self, _: stitchd_core::id::SegmentId, _: &str, _: &[String], _: &[String]) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn soft_delete(&self, _: stitchd_core::id::SegmentId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn check_list_membership(&self, _: stitchd_core::id::EnvironmentId, _: &str, _: &str, keys: &[String]) -> Result<std::collections::HashMap<String, bool>, stitchd_db::RepositoryError> { Ok(keys.iter().map(|k| (k.clone(), false)).collect()) }
        async fn batch_check_list_membership(&self, _: stitchd_core::id::EnvironmentId, _: &[(String, String)], _: &[String]) -> Result<Vec<stitchd_db::ContextMembership>, stitchd_db::RepositoryError> { Ok(vec![]) }
    }

    #[async_trait::async_trait]
    impl stitchd_db::SdkKeyRepository for StubOtherRepo {
        async fn find_by_id(&self, id: stitchd_core::id::SdkKeyId) -> Result<stitchd_core::tenant::SdkKey, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::tenant::SdkKey>, stitchd_db::RepositoryError> { Ok(vec![]) }
        async fn create(&self, _: &stitchd_core::tenant::SdkKey) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn revoke(&self, id: stitchd_core::id::SdkKeyId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn find_active_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::tenant::SdkKey>, stitchd_db::RepositoryError> { Ok(vec![]) }
        async fn find_active_by_hash(&self, h: &str) -> Result<stitchd_core::tenant::SdkKey, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: h.to_string() }) }
    }

    #[async_trait::async_trait]
    impl stitchd_db::EventDefinitionRepository for StubOtherRepo {
        async fn find_by_id(&self, id: stitchd_core::id::EventDefinitionId) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn find_by_key(&self, key: &str, _: stitchd_core::id::EnvironmentId) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: key.to_string() }) }
        async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId) -> Result<Vec<stitchd_core::event::EventDefinition>, stitchd_db::RepositoryError> { Ok(vec![]) }
        async fn create(&self, _: &stitchd_core::event::EventDefinition) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn update(&self, d: &stitchd_core::event::EventDefinition) -> Result<stitchd_core::event::EventDefinition, stitchd_db::RepositoryError> { Ok(d.clone()) }
        async fn soft_delete(&self, id: stitchd_core::id::EventDefinitionId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
    }

    #[async_trait::async_trait]
    impl stitchd_db::ExperimentRepository for StubOtherRepo {
        async fn find_by_id(&self, id: stitchd_core::id::ExperimentId) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn list_by_environment(&self, _: stitchd_core::id::EnvironmentId, _: Option<stitchd_core::experimentation::ExperimentStatus>) -> Result<Vec<stitchd_core::experimentation::Experiment>, stitchd_db::RepositoryError> { Ok(vec![]) }
        async fn create(&self, _: &stitchd_core::experimentation::Experiment) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn update(&self, e: &stitchd_core::experimentation::Experiment) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError> { Ok(e.clone()) }
        async fn soft_delete(&self, id: stitchd_core::id::ExperimentId) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn list_iterations(&self, _: stitchd_core::id::ExperimentId) -> Result<Vec<stitchd_core::experimentation::ExperimentIteration>, stitchd_db::RepositoryError> { Ok(vec![]) }
        async fn apply_transition(&self, id: stitchd_core::id::ExperimentId, _: stitchd_core::experimentation::ExperimentStatus, _: Option<stitchd_core::id::UserId>) -> Result<stitchd_core::experimentation::Experiment, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
    }

    struct NullResults;
    #[async_trait::async_trait]
    impl stitchd_db::experiment_results::ExperimentResultsRepository for NullResults {
        async fn upsert(&self, _: &stitchd_db::experiment_results::UpsertResultRow) -> Result<stitchd_db::experiment_results::ExperimentResultRow, sqlx::Error> { Err(sqlx::Error::RowNotFound) }
        async fn fetch_latest(&self, _: uuid::Uuid) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error> { Ok(vec![]) }
        async fn fetch_by_iteration(&self, _: uuid::Uuid, _: uuid::Uuid) -> Result<Vec<stitchd_db::experiment_results::ExperimentResultRow>, sqlx::Error> { Ok(vec![]) }
        async fn is_stale(&self, _: uuid::Uuid, _: uuid::Uuid) -> Result<bool, sqlx::Error> { Ok(false) }
    }

    struct StubAuthUserRepo {
        user: Option<User>,
    }
    #[async_trait::async_trait]
    impl stitchd_db::AuthUserRepository for StubAuthUserRepo {
        async fn create(&self, email: &str, _: &str, _: Option<&str>) -> Result<stitchd_core::auth::User, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: email.to_string() }) }
        async fn find_by_email(&self, _: &str) -> Result<Option<stitchd_core::auth::User>, stitchd_db::RepositoryError> { Ok(self.user.clone()) }
        async fn find_by_id(&self, _: stitchd_core::id::UserId) -> Result<Option<stitchd_core::auth::User>, stitchd_db::RepositoryError> { Ok(self.user.clone()) }
        async fn rotate_token_secret(&self, id: stitchd_core::id::UserId) -> Result<uuid::Uuid, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn update_status(&self, id: stitchd_core::id::UserId, _: stitchd_core::auth::UserStatus) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn update_password_hash(&self, id: stitchd_core::id::UserId, _: &str) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn update_profile(&self, id: stitchd_core::id::UserId, _: &str, _: Option<&str>) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
    }

    struct StubMembershipRepo;
    #[async_trait::async_trait]
    impl stitchd_db::OrgMembershipRepository for StubMembershipRepo {
        async fn add_member(&self, user_id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::OrgRole) -> Result<stitchd_core::auth::OrgMembership, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: user_id.to_string() }) }
        async fn find_membership(&self, _: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId) -> Result<Option<stitchd_core::auth::OrgMembership>, stitchd_db::RepositoryError> { Ok(None) }
        async fn list_orgs_for_user(&self, _: stitchd_core::id::UserId) -> Result<Vec<stitchd_core::auth::OrgMembership>, stitchd_db::RepositoryError> { Ok(vec![]) }
        async fn remove_member(&self, _: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn update_role(&self, user_id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::OrgRole) -> Result<(), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: user_id.to_string() }) }
    }

    struct StubRefreshTokenRepo;
    #[async_trait::async_trait]
    impl stitchd_db::RefreshTokenRepository for StubRefreshTokenRepo {
        async fn create(&self, user_id: stitchd_core::id::UserId, _: stitchd_core::id::OrganisationId, _: Option<&str>, _: i64) -> Result<(stitchd_core::auth::RefreshToken, String), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: user_id.to_string() }) }
        async fn find_by_hash(&self, _: &str) -> Result<Option<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Ok(None) }
        async fn consume(&self, id: stitchd_core::id::RefreshTokenId) -> Result<Option<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn revoke(&self, _: stitchd_core::id::RefreshTokenId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn revoke_all_for_user(&self, _: stitchd_core::id::UserId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
        async fn list_active(&self, _: stitchd_core::id::UserId) -> Result<Vec<stitchd_core::auth::RefreshToken>, stitchd_db::RepositoryError> { Ok(vec![]) }
    }

    struct StubAuthProviderRepo;
    #[async_trait::async_trait]
    impl stitchd_db::AuthProviderRepository for StubAuthProviderRepo {
        async fn create(&self, _: stitchd_core::id::OrganisationId, _: stitchd_core::auth::ProviderType, _: &str, _: serde_json::Value, _: bool) -> Result<stitchd_core::auth::AuthProvider, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::Unexpected(anyhow::anyhow!("stub"))) }
        async fn find_by_id(&self, _: stitchd_core::id::AuthProviderId) -> Result<Option<stitchd_core::auth::AuthProvider>, stitchd_db::RepositoryError> { Ok(None) }
        async fn list_for_org(&self, _: stitchd_core::id::OrganisationId) -> Result<Vec<stitchd_core::auth::AuthProvider>, stitchd_db::RepositoryError> { Ok(vec![]) }
        async fn update(&self, id: stitchd_core::id::AuthProviderId, _: &str, _: serde_json::Value, _: bool) -> Result<stitchd_core::auth::AuthProvider, stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: id.to_string() }) }
        async fn delete(&self, _: stitchd_core::id::AuthProviderId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
    }

    fn make_app_state(user: Option<User>) -> AppState {
        use metrics_exporter_prometheus::PrometheusBuilder;

        let pool =
            sqlx::PgPool::connect_lazy("postgres://stitchd:stitchd@localhost:5432/stitchd_test")
                .expect("lazy pool");

        struct NullMfaRepo;
        #[async_trait::async_trait]
        impl stitchd_db::MfaRepository for NullMfaRepo {
            async fn create_challenge(&self, _: stitchd_core::id::UserId, _: i64) -> Result<(stitchd_core::id::MfaChallengeId, String), stitchd_db::RepositoryError> { Err(stitchd_db::RepositoryError::NotFound { id: "stub".to_string() }) }
            async fn consume_challenge(&self, _: &str) -> Result<Option<stitchd_core::id::MfaChallengeId>, stitchd_db::RepositoryError> { Ok(None) }
            async fn enable_totp(&self, _: stitchd_core::id::UserId, _: Vec<u8>, _: Vec<String>) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn disable_totp(&self, _: stitchd_core::id::UserId) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn get_totp_secret(&self, _: stitchd_core::id::UserId) -> Result<Option<Vec<u8>>, stitchd_db::RepositoryError> { Ok(None) }
            async fn consume_recovery_code(&self, _: stitchd_core::id::UserId, _: &str) -> Result<bool, stitchd_db::RepositoryError> { Ok(false) }
            async fn store_pending_totp_secret(&self, _: stitchd_core::id::UserId, _: Vec<u8>) -> Result<(), stitchd_db::RepositoryError> { Ok(()) }
            async fn get_user_id_for_challenge(&self, _: &str) -> Result<Option<stitchd_core::id::UserId>, stitchd_db::RepositoryError> { Ok(None) }
        }

        let stub = Arc::new(StubOtherRepo);
        AppState {
            db: pool,
            metrics_handle: PrometheusBuilder::new().build_recorder().handle(),
            user_repo: Arc::new(StubUserRepo { user: user.clone() }),
            auth_user_repo: Arc::new(StubAuthUserRepo { user }),
            membership_repo: Arc::new(StubMembershipRepo),
            refresh_token_repo: Arc::new(StubRefreshTokenRepo),
            mfa_repo: Arc::new(NullMfaRepo),
            auth_provider_repo: Arc::new(StubAuthProviderRepo),
            segment_repo: stub.clone(),
            flag_repo: stub.clone(),
            variant_repo: stub.clone(),
            sdk_key_repo: stub.clone(),
            event_definition_repo: stub.clone(),
            experiment_repo: stub,
            results_repo: Arc::new(NullResults),
            ch_client: None,
            event_writer: None,
            oidc_state_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            saml_state_cache: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        }
    }

    fn make_valid_token(user: &User) -> (String, OrganisationId) {
        let org_id = OrganisationId::new();
        let token = JwtEngine::issue(
            user.id,
            org_id,
            &user.email,
            OrgRole::OrgMember,
            &user.token_secret,
        )
        .unwrap();
        (token, org_id)
    }

    fn make_admin_token(user: &User) -> (String, OrganisationId) {
        let org_id = OrganisationId::new();
        let token = JwtEngine::issue(
            user.id,
            org_id,
            &user.email,
            OrgRole::OrgAdmin,
            &user.token_secret,
        )
        .unwrap();
        (token, org_id)
    }

    #[allow(dead_code)]
    async fn test_handler() -> StatusCode {
        StatusCode::OK
    }

    async fn test_admin_handler(RequireOrgAdmin(user): RequireOrgAdmin) -> StatusCode {
        let _ = user;
        StatusCode::OK
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn missing_authorization_header_returns_401() {
        let user = make_user(UserStatus::Active);
        let state = make_app_state(Some(user));

        let app = Router::new()
            .route("/test", get(|_: AuthenticatedUser| async { StatusCode::OK }))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn invalid_token_returns_401() {
        let user = make_user(UserStatus::Active);
        let state = make_app_state(Some(user));

        let app = Router::new()
            .route("/test", get(|_: AuthenticatedUser| async { StatusCode::OK }))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("Authorization", "Bearer notavalidtoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn valid_token_returns_200() {
        let user = make_user(UserStatus::Active);
        let (token, _org_id) = make_valid_token(&user);
        let state = make_app_state(Some(user));

        let app = Router::new()
            .route("/test", get(|_: AuthenticatedUser| async { StatusCode::OK }))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn deactivated_user_returns_403() {
        let user = make_user(UserStatus::Deactivated);
        let (token, _) = make_valid_token(&user);
        let state = make_app_state(Some(user));

        let app = Router::new()
            .route("/test", get(|_: AuthenticatedUser| async { StatusCode::OK }))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn org_member_accessing_admin_route_returns_403() {
        let user = make_user(UserStatus::Active);
        let (token, _) = make_valid_token(&user); // issues OrgMember token
        let state = make_app_state(Some(user));

        let app = Router::new()
            .route("/admin", get(test_admin_handler))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn org_admin_accessing_admin_route_returns_200() {
        let user = make_user(UserStatus::Active);
        let (token, _) = make_admin_token(&user); // issues OrgAdmin token
        let state = make_app_state(Some(user));

        let app = Router::new()
            .route("/admin", get(test_admin_handler))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/admin")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn expired_token_returns_401() {
        let user = make_user(UserStatus::Active);
        // Manually craft an expired token
        use stitchd_core::auth::jwt::AccessTokenClaims;
        use jsonwebtoken::{EncodingKey, Header};

        let claims = AccessTokenClaims {
            sub: user.id.to_string(),
            org_id: OrganisationId::new().to_string(),
            email: user.email.clone(),
            org_role: OrgRole::OrgMember,
            iat: 1_000_000,
            exp: 1_000_001, // far in the past
        };
        let key = EncodingKey::from_secret(user.token_secret.as_bytes());
        let expired_token =
            jsonwebtoken::encode(&Header::default(), &claims, &key).unwrap();

        let state = make_app_state(Some(user));

        let app = Router::new()
            .route("/test", get(|_: AuthenticatedUser| async { StatusCode::OK }))
            .with_state(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/test")
                    .header("Authorization", format!("Bearer {expired_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
