//! JWT bearer-token validation.
//!
//! Reuses [`stitchd_core::auth::jwt::JwtEngine`] for decode + signature
//! verification.  The caller supplies an `AuthUserRepository` to fetch the
//! per-user `token_secret` needed for full verification.

use std::sync::Arc;

use stitchd_core::{
    auth::{OrgRole, User, UserStatus, jwt::JwtEngine},
    id::UserId,
};
use stitchd_db::{AuthUserRepository, RepositoryError};
use tonic::Status;

/// Error variants from JWT validation.
#[derive(Debug, thiserror::Error)]
pub enum JwtValidationError {
    /// The token is structurally malformed — cannot decode claims.
    #[error("malformed JWT: {0}")]
    Malformed(String),

    /// No user was found with the `sub` claim.
    #[error("user not found")]
    UserNotFound,

    /// The user account has been deactivated.
    #[error("user account deactivated")]
    Deactivated,

    /// The signature or expiry check failed.
    #[error("invalid token: {0}")]
    Invalid(String),

    /// An internal/unexpected repository error occurred.
    #[error("internal error: {0}")]
    Internal(String),
}

impl From<JwtValidationError> for Status {
    fn from(e: JwtValidationError) -> Self {
        match e {
            JwtValidationError::Malformed(_)
            | JwtValidationError::UserNotFound
            | JwtValidationError::Invalid(_)
            | JwtValidationError::Deactivated => Self::unauthenticated(e.to_string()),
            JwtValidationError::Internal(msg) => Self::internal(msg),
        }
    }
}

/// A validated JWT result ready for RBAC context assembly.
#[derive(Debug, Clone)]
pub struct ValidatedJwt {
    /// The authenticated user record.
    pub user: User,
    /// The organisation ID the token was issued for.
    pub org_id: String,
    /// The organisation role from the token claims.
    pub org_role: OrgRole,
    /// Whether the token was issued for a System-org user.
    pub is_system: bool,
}

/// Validate a bearer token by:
/// 1. Decoding claims without signature verification to extract `sub`.
/// 2. Loading the user from the repository to get `token_secret`.
/// 3. Fully verifying the token (signature + expiry).
/// 4. Asserting the user account is active.
///
/// # Errors
/// Returns a [`JwtValidationError`] describing why validation failed.
pub async fn validate_bearer_token(
    token: &str,
    auth_user_repo: &Arc<dyn AuthUserRepository>,
) -> Result<ValidatedJwt, JwtValidationError> {
    // Step 1 — decode unverified to get sub (user_id).
    let unverified =
        JwtEngine::decode_unverified(token).map_err(|e| JwtValidationError::Malformed(e.to_string()))?;

    let user_id: UserId = unverified
        .sub
        .parse::<uuid::Uuid>()
        .map(UserId::from_uuid)
        .map_err(|_| JwtValidationError::Malformed("sub is not a valid UUID".to_string()))?;

    // Step 2 — fetch user from repository.
    let user = auth_user_repo
        .find_by_id(user_id)
        .await
        .map_err(|e| match e {
            RepositoryError::NotFound { .. } => JwtValidationError::UserNotFound,
            other => JwtValidationError::Internal(other.to_string()),
        })?
        .ok_or(JwtValidationError::UserNotFound)?;

    // Step 3 — full verification with per-user secret.
    let claims = JwtEngine::verify(token, &user.token_secret)
        .map_err(|e| JwtValidationError::Invalid(e.to_string()))?;

    // Step 4 — assert active.
    if user.status != UserStatus::Active {
        return Err(JwtValidationError::Deactivated);
    }

    Ok(ValidatedJwt {
        user,
        org_id: claims.org_id,
        org_role: claims.org_role,
        is_system: claims.is_system,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::Arc;
    use stitchd_core::{
        auth::{OrgRole, User, UserStatus, jwt::JwtEngine},
        id::{OrganisationId, UserId},
    };
    use stitchd_db::{AuthUserRepository, RepositoryError};

    // ── Stub AuthUserRepository ──────────────────────────────────────────────

    struct StubAuthUserRepo {
        user: Option<User>,
    }

    #[tonic::async_trait]
    impl AuthUserRepository for StubAuthUserRepo {
        async fn create(
            &self,
            email: &str,
            _hash: &str,
            _display_name: Option<&str>,
        ) -> Result<User, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: email.to_string(),
            })
        }

        async fn find_by_email(&self, _email: &str) -> Result<Option<User>, RepositoryError> {
            Ok(self.user.clone())
        }

        async fn find_by_id(&self, _id: UserId) -> Result<Option<User>, RepositoryError> {
            Ok(self.user.clone())
        }

        async fn rotate_token_secret(
            &self,
            id: UserId,
        ) -> Result<uuid::Uuid, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn update_status(
            &self,
            id: UserId,
            _status: UserStatus,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn update_password_hash(
            &self,
            id: UserId,
            _hash: &str,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn update_profile(
            &self,
            id: UserId,
            _display_name: &str,
            _avatar_url: Option<&str>,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_org_users(
            &self,
            _org_id: stitchd_core::id::OrganisationId,
        ) -> Result<Vec<(User, stitchd_core::auth::OrgRole)>, RepositoryError> {
            Ok(vec![])
        }
    }

    fn make_active_user() -> User {
        User {
            id: UserId::new(),
            email: "test@example.com".to_string(),
            display_name: "Test User".to_string(),
            avatar_url: None,
            password_hash: None,
            token_secret: uuid::Uuid::new_v4(),
            totp_secret: None,
            totp_enabled: false,
            status: UserStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn issue_token(user: &User) -> (String, OrganisationId) {
        let org_id = OrganisationId::new();
        let token = JwtEngine::issue(
            user.id,
            org_id,
            &user.email,
            OrgRole::OrgMember,
            false,
            &user.token_secret,
        )
        .unwrap();
        (token, org_id)
    }

    // ── Tests — TDD Red phase: written first, implementation in jwt.rs ────────

    #[tokio::test]
    async fn valid_token_for_active_user_succeeds() {
        let user = make_active_user();
        let (token, org_id) = issue_token(&user);
        let repo: Arc<dyn AuthUserRepository> =
            Arc::new(StubAuthUserRepo { user: Some(user.clone()) });

        let result = validate_bearer_token(&token, &repo).await;
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        let validated = result.unwrap();
        assert_eq!(validated.user.id, user.id);
        assert_eq!(validated.org_id, org_id.to_string());
    }

    #[tokio::test]
    async fn malformed_token_returns_malformed_error() {
        let repo: Arc<dyn AuthUserRepository> =
            Arc::new(StubAuthUserRepo { user: None });

        let result = validate_bearer_token("not.a.jwt", &repo).await;
        assert!(matches!(result, Err(JwtValidationError::Malformed(_))));
    }

    #[tokio::test]
    async fn unknown_user_returns_user_not_found() {
        let user = make_active_user();
        let (token, _) = issue_token(&user);
        // Repo returns None — user doesn't exist
        let repo: Arc<dyn AuthUserRepository> =
            Arc::new(StubAuthUserRepo { user: None });

        let result = validate_bearer_token(&token, &repo).await;
        assert!(matches!(result, Err(JwtValidationError::UserNotFound)));
    }

    #[tokio::test]
    async fn deactivated_user_returns_deactivated_error() {
        let mut user = make_active_user();
        let (token, _) = issue_token(&user);
        user.status = UserStatus::Deactivated;
        let repo: Arc<dyn AuthUserRepository> =
            Arc::new(StubAuthUserRepo { user: Some(user) });

        let result = validate_bearer_token(&token, &repo).await;
        assert!(matches!(result, Err(JwtValidationError::Deactivated)));
    }

    #[tokio::test]
    async fn wrong_secret_returns_invalid_error() {
        let user = make_active_user();
        let (token, _) = issue_token(&user);
        // Store a *different* user with a different secret — same ID trick
        let mut wrong_user = user.clone();
        wrong_user.token_secret = uuid::Uuid::new_v4();
        let repo: Arc<dyn AuthUserRepository> =
            Arc::new(StubAuthUserRepo { user: Some(wrong_user) });

        let result = validate_bearer_token(&token, &repo).await;
        assert!(matches!(result, Err(JwtValidationError::Invalid(_))));
    }

    #[tokio::test]
    async fn jwt_validation_error_converts_to_unauthenticated_status() {
        let err = JwtValidationError::Malformed("bad".to_string());
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn jwt_internal_error_converts_to_internal_status() {
        let err = JwtValidationError::Internal("db down".to_string());
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::Internal);
    }

    #[tokio::test]
    async fn deactivated_error_converts_to_unauthenticated_status() {
        let err = JwtValidationError::Deactivated;
        let status = Status::from(err);
        assert_eq!(status.code(), tonic::Code::Unauthenticated);
    }
}
