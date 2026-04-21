//! tonic gRPC implementation of [`AuthService`].
//!
//! Accepts a [`CredentialRequest`] with either a `bearer_token` or `sdk_key`
//! field set, dispatches to the appropriate validation path, and returns an
//! [`RbacContext`] proto message.

use std::sync::Arc;

use tonic::{Request, Response, Status};

use stitchd_db::{AuthUserRepository, SdkKeyRepository};
use stitchd_proto::auth::v1::{
    CredentialRequest, RbacContext,
    auth_service_server::AuthService,
    credential_request::Credential,
};

use crate::{
    jwt::validate_bearer_token,
    rbac::{rbac_context_from_jwt, rbac_context_from_sdk_key},
    sdk_key::validate_sdk_key,
};

/// tonic gRPC service implementation for `AuthService`.
pub struct AuthServiceImpl {
    auth_user_repo: Arc<dyn AuthUserRepository>,
    sdk_key_repo: Arc<dyn SdkKeyRepository>,
}

impl AuthServiceImpl {
    /// Create a new [`AuthServiceImpl`] backed by the given repositories.
    #[must_use]
    pub fn new(
        auth_user_repo: Arc<dyn AuthUserRepository>,
        sdk_key_repo: Arc<dyn SdkKeyRepository>,
    ) -> Self {
        Self {
            auth_user_repo,
            sdk_key_repo,
        }
    }
}

#[tonic::async_trait]
impl AuthService for AuthServiceImpl {
    async fn validate_credential(
        &self,
        request: Request<CredentialRequest>,
    ) -> Result<Response<RbacContext>, Status> {
        let req = request.into_inner();

        match req.credential {
            Some(Credential::BearerToken(token)) => {
                let validated = validate_bearer_token(&token, &self.auth_user_repo)
                    .await
                    .map_err(Status::from)?;
                let ctx = rbac_context_from_jwt(&validated);
                Ok(Response::new(ctx))
            }
            Some(Credential::SdkKey(raw_key)) => {
                let sdk_ctx = validate_sdk_key(&raw_key, &self.sdk_key_repo)
                    .await
                    .map_err(Status::from)?;
                let ctx = rbac_context_from_sdk_key(&sdk_ctx);
                Ok(Response::new(ctx))
            }
            None => Err(Status::invalid_argument(
                "credential field must be set (bearer_token or sdk_key)",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::Arc;
    use stitchd_core::{
        auth::{OrgRole, User, UserStatus, jwt::JwtEngine},
        id::{EnvironmentId, OrganisationId, SdkKeyId, UserId},
        tenant::SdkKey,
    };
    use stitchd_db::{AuthUserRepository, RepositoryError, SdkKeyRepository};

    // ── Stub repositories ────────────────────────────────────────────────────

    struct StubAuthUserRepo {
        user: Option<User>,
    }

    #[tonic::async_trait]
    impl AuthUserRepository for StubAuthUserRepo {
        async fn create(
            &self,
            email: &str,
            _: &str,
            _: Option<&str>,
        ) -> Result<User, RepositoryError> {
            Err(RepositoryError::NotFound {
                id: email.to_string(),
            })
        }

        async fn find_by_email(&self, _: &str) -> Result<Option<User>, RepositoryError> {
            Ok(self.user.clone())
        }

        async fn find_by_id(&self, _: UserId) -> Result<Option<User>, RepositoryError> {
            Ok(self.user.clone())
        }

        async fn rotate_token_secret(&self, id: UserId) -> Result<uuid::Uuid, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn update_status(
            &self,
            id: UserId,
            _: UserStatus,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn update_password_hash(
            &self,
            id: UserId,
            _: &str,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn update_profile(
            &self,
            id: UserId,
            _: &str,
            _: Option<&str>,
        ) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_org_users(
            &self,
            _: stitchd_core::id::OrganisationId,
        ) -> Result<Vec<(User, OrgRole)>, RepositoryError> {
            Ok(vec![])
        }
    }

    struct StubSdkKeyRepo {
        active_keys: Vec<SdkKey>,
    }

    #[tonic::async_trait]
    impl SdkKeyRepository for StubSdkKeyRepo {
        async fn find_by_id(&self, id: SdkKeyId) -> Result<SdkKey, RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn list_by_environment(
            &self,
            _: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(vec![])
        }

        async fn create(&self, _: &SdkKey) -> Result<(), RepositoryError> {
            Ok(())
        }

        async fn revoke(&self, id: SdkKeyId) -> Result<(), RepositoryError> {
            Err(RepositoryError::NotFound { id: id.to_string() })
        }

        async fn find_active_by_environment(
            &self,
            env_id: EnvironmentId,
        ) -> Result<Vec<SdkKey>, RepositoryError> {
            Ok(self
                .active_keys
                .iter()
                .filter(|k| k.environment_id == env_id)
                .cloned()
                .collect())
        }

        async fn find_active_by_hash(&self, hash: &str) -> Result<SdkKey, RepositoryError> {
            self.active_keys
                .iter()
                .find(|k| k.key_hash == hash)
                .cloned()
                .ok_or_else(|| RepositoryError::NotFound {
                    id: hash.to_string(),
                })
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn make_active_user() -> User {
        User {
            id: UserId::new(),
            email: "user@example.com".to_string(),
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
            &user.token_secret,
        )
        .unwrap();
        (token, org_id)
    }

    fn make_service(
        user: Option<User>,
        sdk_keys: Vec<SdkKey>,
    ) -> AuthServiceImpl {
        AuthServiceImpl::new(
            Arc::new(StubAuthUserRepo { user }),
            Arc::new(StubSdkKeyRepo {
                active_keys: sdk_keys,
            }),
        )
    }

    // ── JWT tests ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn bearer_token_valid_user_returns_rbac_context() {
        let user = make_active_user();
        let (token, org_id) = issue_token(&user);
        let svc = make_service(Some(user.clone()), vec![]);

        let req = Request::new(CredentialRequest {
            credential: Some(Credential::BearerToken(token)),
        });
        let resp = svc.validate_credential(req).await.unwrap();
        let ctx = resp.into_inner();

        assert_eq!(ctx.tenant_id, org_id.to_string());
        assert_eq!(ctx.subject, user.id.to_string());
        assert!(!ctx.roles.is_empty());
    }

    #[tokio::test]
    async fn bearer_token_invalid_returns_unauthenticated() {
        let svc = make_service(None, vec![]);
        let req = Request::new(CredentialRequest {
            credential: Some(Credential::BearerToken("invalid.token.here".to_string())),
        });
        let err = svc.validate_credential(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn bearer_token_unknown_user_returns_unauthenticated() {
        let user = make_active_user();
        let (token, _) = issue_token(&user);
        // Repo returns None
        let svc = make_service(None, vec![]);

        let req = Request::new(CredentialRequest {
            credential: Some(Credential::BearerToken(token)),
        });
        let err = svc.validate_credential(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    #[tokio::test]
    async fn bearer_token_deactivated_user_returns_unauthenticated() {
        let mut user = make_active_user();
        let (token, _) = issue_token(&user);
        user.status = UserStatus::Deactivated;
        let svc = make_service(Some(user), vec![]);

        let req = Request::new(CredentialRequest {
            credential: Some(Credential::BearerToken(token)),
        });
        let err = svc.validate_credential(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    // ── SDK key tests ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn sdk_key_valid_returns_rbac_context() {
        let env_id = EnvironmentId::new();
        let raw_key = "my-test-sdk-key";
        let key = SdkKey {
            id: SdkKeyId::new(),
            environment_id: env_id,
            key_hash: crate::sdk_key::hash_sdk_key(raw_key),
            is_active: true,
            created_at: Utc::now(),
            revoked_at: None,
        };
        let svc = make_service(None, vec![key]);

        let req = Request::new(CredentialRequest {
            credential: Some(Credential::SdkKey(raw_key.to_string())),
        });
        let resp = svc.validate_credential(req).await.unwrap();
        let ctx = resp.into_inner();

        assert_eq!(ctx.environment_id, env_id.to_string());
        assert_eq!(ctx.roles, vec!["sdk".to_string()]);
    }

    #[tokio::test]
    async fn sdk_key_invalid_returns_unauthenticated() {
        let svc = make_service(None, vec![]);
        let req = Request::new(CredentialRequest {
            credential: Some(Credential::SdkKey("unknown-key".to_string())),
        });
        let err = svc.validate_credential(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    // ── Missing credential ────────────────────────────────────────────────────

    #[tokio::test]
    async fn missing_credential_returns_invalid_argument() {
        let svc = make_service(None, vec![]);
        let req = Request::new(CredentialRequest { credential: None });
        let err = svc.validate_credential(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }
}
